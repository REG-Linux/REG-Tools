use reglinux_fetch::{build_client, fetch_manifest, Manifest};
use slint::{ModelRc, SharedString, VecModel};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use usbimager_sys::{Device, Progress, WriteJob};

slint::include_modules!();

#[derive(Default)]
struct AppState {
    manifest: Option<Manifest>,
    targets: Vec<String>,
    devices: Vec<Device>,
    download_child: Option<Child>,
    flash_job: Option<WriteJob>,
    last_image: Option<PathBuf>,
}

fn fetch_bin_path() -> PathBuf {
    if let Ok(path) = std::env::var("REGLINUX_FETCH") {
        return PathBuf::from(path);
    }
    if let Ok(mut exe) = std::env::current_exe() {
        if cfg!(windows) {
            exe.set_file_name("reglinux-fetch.exe");
        } else {
            exe.set_file_name("reglinux-fetch");
        }
        if exe.exists() {
            return exe;
        }
    }
    PathBuf::from("reglinux-fetch")
}

fn set_model(list: Vec<String>) -> ModelRc<SharedString> {
    let model = VecModel::from(list.into_iter().map(SharedString::from).collect::<Vec<_>>());
    ModelRc::from(std::rc::Rc::new(model))
}

fn is_system_disk(device_path: &str) -> bool {
    let Ok(data) = fs::read_to_string("/proc/self/mountinfo") else { return false; };
    for line in data.lines() {
        let mut parts = line.split(" - ");
        let Some(left) = parts.next() else { continue; };
        let Some(right) = parts.next() else { continue; };
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        if left_fields.len() < 5 {
            continue;
        }
        let mount_point = left_fields[4];
        if mount_point != "/" && mount_point != "/boot" && !mount_point.starts_with("/boot/") {
            continue;
        }
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if right_fields.len() < 2 {
            continue;
        }
        let source = right_fields[1];
        if source.starts_with(device_path) {
            return true;
        }
        if device_path.starts_with("/dev/") {
            let base = device_path.trim_end_matches(|c: char| c.is_ascii_digit());
            if base != device_path && source.starts_with(base) {
                return true;
            }
        }
    }
    false
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ui = AppWindow::new()?;
    ui.set_repo("REG-Linux/REG-Linux".into());
    ui.set_tag("".into());
    ui.set_output_dir("./downloads".into());
    ui.set_verify(true);
    ui.set_show_all_disks(false);
    ui.set_download_progress(0.0);
    ui.set_flash_progress(0.0);
    ui.set_about_open(false);
    ui.set_about_text(
        fs::read_to_string("LICENSES/USBImager-MIT-LICENSE.txt")
            .unwrap_or_else(|_| "USBImager MIT license text not found.".to_string())
            .into(),
    );

    let state = Arc::new(Mutex::new(AppState::default()));

    let ui_fetch = ui.as_weak();
    let state_fetch = Arc::clone(&state);
    ui.on_fetch_manifest(move || {
        let Some(ui_strong) = ui_fetch.upgrade() else { return; };
        let repo = ui_strong.get_repo().to_string();
        let tag = ui_strong.get_tag().to_string();
        if tag.trim().is_empty() {
            let ui = ui_fetch.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_download_status("Tag is required".into());
                }
            });
            return;
        }
        let ui = ui_fetch.clone();
        let state = Arc::clone(&state_fetch);
        thread::spawn(move || {
            let token = std::env::var("GITHUB_TOKEN").ok();
            let client = match build_client(token.as_deref()) {
                Ok(client) => client,
                Err(err) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_download_status(err.to_string().into());
                        }
                    });
                    return;
                }
            };
            match fetch_manifest(&client, &repo, &tag) {
                Ok(manifest) => {
                    let mut targets: Vec<String> = manifest.images.keys().cloned().collect();
                    targets.sort();
                    let mut guard = state.lock().unwrap();
                    guard.manifest = Some(manifest);
                    guard.targets = targets.clone();
                    drop(guard);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_target_list(set_model(targets));
                            ui.set_target_index(0);
                            ui.set_download_status("Manifest loaded".into());
                        }
                    });
                }
                Err(err) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_download_status(err.to_string().into());
                        }
                    });
                }
            }
        });
    });

    let ui_devices = ui.as_weak();
    let state_devices = Arc::clone(&state);
    ui.on_refresh_devices(move || {
        let Some(ui_strong) = ui_devices.upgrade() else { return; };
        let show_all = ui_strong.get_show_all_disks();
        let ui = ui_devices.clone();
        let state = Arc::clone(&state_devices);
        thread::spawn(move || match usbimager_sys::list_devices(show_all) {
            Ok(devices) => {
                let labels = devices.iter().map(|d| d.label.clone()).collect::<Vec<_>>();
                let mut guard = state.lock().unwrap();
                guard.devices = devices;
                drop(guard);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui.upgrade() {
                        ui.set_device_list(set_model(labels));
                        ui.set_device_index(0);
                        ui.set_flash_status("Devices refreshed".into());
                        ui.set_safety_required(false);
                        ui.set_confirm_text("".into());
                        ui.set_safety_message("".into());
                    }
                });
            }
            Err(err) => {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui.upgrade() {
                        ui.set_flash_status(err.to_string().into());
                    }
                });
            }
        });
    });

    let ui_download = ui.as_weak();
    let state_download = Arc::clone(&state);
    ui.on_download(move || {
        let Some(ui_strong) = ui_download.upgrade() else { return; };
        let repo = ui_strong.get_repo().to_string();
        let tag = ui_strong.get_tag().to_string();
        let out_dir = ui_strong.get_output_dir().to_string();
        let target_index = ui_strong.get_target_index();
        let target = {
            let guard = state_download.lock().unwrap();
            guard.targets.get(target_index as usize).cloned()
        };
        let Some(target) = target else {
            let ui = ui_download.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_download_status("Select a target".into());
                }
            });
            return;
        };

        let fetch_bin = fetch_bin_path();
        let ui = ui_download.clone();
        let state = Arc::clone(&state_download);
        thread::spawn(move || {
            let mut cmd = Command::new(&fetch_bin);
            cmd.arg("fetch")
                .arg("--repo")
                .arg(&repo)
                .arg("--tag")
                .arg(&tag)
                .arg("--target")
                .arg(&target)
                .arg("--out")
                .arg(&out_dir)
                .arg("--json")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(err) => {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_download_status(err.to_string().into());
                        }
                    });
                    return;
                }
            };

            let stdout = child.stdout.take();
            {
                let mut guard = state.lock().unwrap();
                guard.download_child = Some(child);
            }

            let Some(stdout) = stdout else {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = ui.upgrade() {
                        ui.set_download_status("Failed to read downloader output".into());
                    }
                });
                return;
            };

            let ui_busy = ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_busy.upgrade() {
                    ui.set_busy_download(true);
                    ui.set_download_status("Downloading...".into());
                    ui.set_download_progress(0.0);
                }
            });

            let reader = BufReader::new(stdout);
            let mut last_output: Option<PathBuf> = None;
            for line in reader.lines().flatten() {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                    if let Some(kind) = value.get("type").and_then(|v| v.as_str()) {
                        match kind {
                            "stage" => {
                                if let Some(stage) = value.get("stage").and_then(|v| v.as_str()) {
                                    let msg = format!("Stage: {stage}");
                                    let ui = ui.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(ui) = ui.upgrade() {
                                            ui.set_download_status(msg.into());
                                        }
                                    });
                                }
                            }
                            "part" => {
                                let done = value.get("done").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let total = value.get("total").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let progress = if total > 0.0 { done / total } else { 0.0 };
                                let ui = ui.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = ui.upgrade() {
                                        ui.set_download_progress(progress as f32);
                                    }
                                });
                            }
                            "done" => {
                                if let Some(output) = value.get("output").and_then(|v| v.as_str()) {
                                    last_output = Some(PathBuf::from(output));
                                }
                                let ui = ui.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(ui) = ui.upgrade() {
                                        ui.set_download_progress(1.0);
                                        ui.set_download_status("Download complete".into());
                                    }
                                });
                            }
                            "error" => {
                                if let Some(message) = value.get("message").and_then(|v| v.as_str()) {
                                    let msg = format!("Download error: {message}");
                                    let ui = ui.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(ui) = ui.upgrade() {
                                            ui.set_download_status(msg.into());
                                        }
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            let exit_status = {
                let mut guard = state.lock().unwrap();
                if let Some(mut child) = guard.download_child.take() {
                    child.wait().ok()
                } else {
                    None
                }
            };

            if let Some(path) = last_output {
                let mut guard = state.lock().unwrap();
                guard.last_image = Some(path);
            }

            let ui = ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_busy_download(false);
                    if let Some(status) = exit_status {
                        if !status.success() {
                            ui.set_download_status("Download failed".into());
                        }
                    }
                }
            });
        });
    });

    let ui_flash = ui.as_weak();
    let state_flash = Arc::clone(&state);
    ui.on_flash(move || {
        let Some(ui_strong) = ui_flash.upgrade() else { return; };
        let verify = ui_strong.get_verify();
        let device_index = ui_strong.get_device_index();
        let confirm = ui_strong.get_confirm_text().to_string();

        let (device, image_path) = {
            let guard = state_flash.lock().unwrap();
            let device = guard.devices.get(device_index as usize).cloned();
            let image_path = guard.last_image.clone();
            (device, image_path)
        };

        let Some(device) = device else {
            let ui = ui_flash.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_flash_status("Select a device".into());
                }
            });
            return;
        };

        let Some(image_path) = image_path else {
            let ui = ui_flash.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_flash_status("Download an image first".into());
                }
            });
            return;
        };

        let needs_confirm = !device.is_removable || is_system_disk(&device.id);
        if needs_confirm && confirm.trim() != "ERASE" {
            let msg = if !device.is_removable {
                "Selected disk is not removable. Confirm to proceed."
            } else {
                "Selected disk may be a system disk. Confirm to proceed."
            };
            let ui = ui_flash.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_safety_required(true);
                    ui.set_safety_message(msg.into());
                    ui.set_flash_status("Confirmation required".into());
                }
            });
            return;
        }

        let ui_main = ui_flash.clone();
        let ui_progress = ui_flash.clone();
        let ui_error = ui_flash.clone();
        let ui_finish = ui_flash.clone();
        let state = Arc::clone(&state_flash);

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui_main.upgrade() {
                ui.set_busy_flash(true);
                ui.set_flash_progress(0.0);
                ui.set_flash_status("Flashing...".into());
                ui.set_safety_required(false);
            }
        });

        thread::spawn(move || {
            let job = match usbimager_sys::write_image_zst(
                image_path.to_string_lossy().as_ref(),
                &device.id,
                verify,
                Some(Box::new(move |progress: Progress| {
                    let done = progress.done as f64;
                    let total = progress.total as f64;
                    let pct = if total > 0.0 { done / total } else { 0.0 };
                    let msg = progress.message.clone();
                    let ui = ui_progress.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_flash_progress(pct as f32);
                            if !msg.is_empty() {
                                ui.set_flash_status(msg.into());
                            }
                        }
                    });
                })),
                Some(Box::new(move |msg| {
                    let ui = ui_error.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_flash_status(format!("Flash error: {msg}").into());
                        }
                    });
                })),
            ) {
                Ok(job) => job,
                Err(err) => {
                    let ui = ui_finish.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_flash_status(err.to_string().into());
                            ui.set_busy_flash(false);
                        }
                    });
                    return;
                }
            };

            {
                let mut guard = state.lock().unwrap();
                guard.flash_job = Some(job);
            }

            let result = {
                let mut guard = state.lock().unwrap();
                guard.flash_job.take().map(|job| job.wait())
            }
            .unwrap_or_else(|| Err(usbimager_sys::UsbImagerError::new("Flash job missing")));

            let ui = ui_finish.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_busy_flash(false);
                    match result {
                        Ok(_) => {
                            ui.set_flash_progress(1.0);
                            ui.set_flash_status("Flash complete".into());
                        }
                        Err(err) => {
                            ui.set_flash_status(err.to_string().into());
                        }
                    }
                }
            });
        });
    });

    let ui_cancel = ui.as_weak();
    let state_cancel = Arc::clone(&state);
    ui.on_cancel(move || {
        let mut guard = state_cancel.lock().unwrap();
        if let Some(mut child) = guard.download_child.take() {
            let _ = child.kill();
        }
        if let Some(job) = guard.flash_job.take() {
            let _ = job.cancel();
        }
        drop(guard);
        let ui = ui_cancel.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                ui.set_busy_download(false);
                ui.set_busy_flash(false);
                ui.set_download_status("Cancelled".into());
                ui.set_flash_status("Cancelled".into());
            }
        });
    });

    ui.run()?;
    Ok(())
}
