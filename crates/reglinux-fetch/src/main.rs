use clap::{Parser, Subcommand};
use reglinux_fetch::{
    build_client, fetch_release, join_parts, sha256_file, ensure_parent, Manifest, Meta, Part,
};
use serde_json::json;
use sha2::Digest;
use std::collections::{HashMap, VecDeque};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(author, version, about)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Fetch {
        #[arg(long, default_value = "REG-Linux/REG-Linux")]
        repo: String,
        #[arg(long)]
        tag: String,
        #[arg(long)]
        target: String,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        json: bool,
        #[arg(long)]
        no_json: bool,
        #[arg(long, default_value_t = 4)]
        concurrency: usize,
    },
}

#[derive(Debug)]
enum Event {
    Part { name: String, done: u64, total: u64 },
    Error { message: String, context: serde_json::Value },
}

fn stdout_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

fn emit_json(value: serde_json::Value) {
    println!("{}", value.to_string());
}

fn emit_stage(json: bool, stage: &str, extra: Option<serde_json::Value>) {
    if json {
        let mut obj = serde_json::Map::new();
        obj.insert("type".to_string(), json!("stage"));
        obj.insert("stage".to_string(), json!(stage));
        if let Some(extra) = extra {
            if let Some(map) = extra.as_object() {
                for (k, v) in map {
                    obj.insert(k.clone(), v.clone());
                }
            }
        }
        emit_json(serde_json::Value::Object(obj));
    } else {
        eprintln!("[stage] {stage}");
    }
}

fn emit_error(json: bool, message: &str, context: serde_json::Value) {
    if json {
        emit_json(json!({"type":"error","message":message,"context":context}));
    } else {
        eprintln!("ERROR: {message}");
    }
}

fn emit_done(json: bool, output: &Path) {
    if json {
        emit_json(json!({"type":"done","output":output.to_string_lossy()}));
    } else {
        eprintln!("Done: {}", output.display());
    }
}

fn manifest_for_target(manifest: &Manifest, target: &str) -> Result<Meta, String> {
    manifest
        .images
        .get(target)
        .cloned()
        .ok_or_else(|| format!("Target {target} not found in manifest"))
}

fn build_asset_map(release: &reglinux_fetch::Release) -> HashMap<String, String> {
    release
        .assets
        .iter()
        .map(|asset| (asset.name.clone(), asset.browser_download_url.clone()))
        .collect()
}

fn download_part(
    client: &reqwest::blocking::Client,
    part: &Part,
    url: &str,
    out_path: &Path,
    tx: &mpsc::Sender<Event>,
    failed: &AtomicBool,
) -> Result<(), String> {
    if failed.load(Ordering::SeqCst) {
        return Ok(());
    }

    if out_path.exists() {
        match sha256_file(out_path) {
            Ok(existing) if existing.eq_ignore_ascii_case(&part.sha256) => {
                let _ = tx.send(Event::Part {
                    name: part.name.clone(),
                    done: part.size,
                    total: part.size,
                });
                return Ok(());
            }
            _ => {
                let _ = fs::remove_file(out_path);
            }
        }
    }

    ensure_parent(out_path).map_err(|e| e.to_string())?;
    let mut resp = client.get(url).send().map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("Download failed for {}: {}", part.name, resp.status()));
    }

    let mut file = File::create(out_path).map_err(|e| e.to_string())?;
    let mut hasher = sha2::Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_emit = Instant::now();

    loop {
        let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if last_emit.elapsed() >= Duration::from_millis(200) {
            let _ = tx.send(Event::Part {
                name: part.name.clone(),
                done,
                total: part.size,
            });
            last_emit = Instant::now();
        }
    }
    file.flush().map_err(|e| e.to_string())?;

    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&part.sha256) {
        let _ = fs::remove_file(out_path);
        return Err(format!("SHA256 mismatch for {}", part.name));
    }

    let _ = tx.send(Event::Part {
        name: part.name.clone(),
        done: part.size,
        total: part.size,
    });

    Ok(())
}

fn download_parts_parallel(
    client: &reqwest::blocking::Client,
    parts: &[Part],
    assets: &HashMap<String, String>,
    out_dir: &Path,
    concurrency: usize,
    json: bool,
) -> Result<Vec<PathBuf>, String> {
    let mut tasks = VecDeque::new();
    let mut paths = Vec::new();
    for part in parts {
        let url = assets
            .get(&part.name)
            .ok_or_else(|| format!("Missing asset {} in release", part.name))?
            .clone();
        let path = out_dir.join(&part.name);
        paths.push(path.clone());
        tasks.push_back((part.clone(), url, path));
    }

    let tasks = Arc::new(Mutex::new(tasks));
    let failed = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Event>();
    let error_msg = Arc::new(Mutex::new(None));

    let mut handles = Vec::new();
    let worker_count = std::cmp::max(1, concurrency);
    for _ in 0..worker_count {
        let client = client.clone();
        let tasks = Arc::clone(&tasks);
        let tx = tx.clone();
        let failed = Arc::clone(&failed);
        let error_msg = Arc::clone(&error_msg);
        handles.push(thread::spawn(move || loop {
            if failed.load(Ordering::SeqCst) {
                break;
            }
            let task = {
                let mut guard = tasks.lock().unwrap();
                guard.pop_front()
            };
            let Some((part, url, path)) = task else { break; };
            if let Err(err) = download_part(&client, &part, &url, &path, &tx, &failed) {
                failed.store(true, Ordering::SeqCst);
                *error_msg.lock().unwrap() = Some(err);
                let _ = tx.send(Event::Error {
                    message: "download_failed".to_string(),
                    context: json!({"part": part.name}),
                });
                break;
            }
        }));
    }
    drop(tx);

    for event in rx {
        match event {
            Event::Part { name, done, total } => {
                if json {
                    emit_json(json!({"type":"part","name":name,"done":done,"total":total}));
                } else {
                    eprintln!("{name}: {done}/{total}");
                }
            }
            Event::Error { message, context } => {
                emit_error(json, &message, context);
            }
        }
    }

    for handle in handles {
        let _ = handle.join();
    }

    if failed.load(Ordering::SeqCst) {
        let msg = error_msg
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| "download failed".to_string());
        return Err(msg);
    }

    Ok(paths)
}

fn verify_parts(parts: &[Part], out_dir: &Path) -> Result<(), String> {
    for part in parts {
        let path = out_dir.join(&part.name);
        let hash = sha256_file(&path).map_err(|e| e.to_string())?;
        if !hash.eq_ignore_ascii_case(&part.sha256) {
            return Err(format!("SHA256 mismatch for {}", part.name));
        }
    }
    Ok(())
}

fn run_fetch(
    repo: String,
    tag: String,
    target: String,
    out: PathBuf,
    json: bool,
    concurrency: usize,
) -> Result<(), String> {
    let token = std::env::var("GITHUB_TOKEN").ok();
    let client = build_client(token.as_deref()).map_err(|e| e.to_string())?;

    emit_stage(json, "fetch_manifest", None);
    let release = fetch_release(&client, &repo, &tag).map_err(|e| e.to_string())?;
    let assets = build_asset_map(&release);
    let manifest_asset = release
        .assets
        .iter()
        .find(|asset| asset.name == "manifest.json")
        .ok_or_else(|| "manifest.json not found in release".to_string())?;
    let manifest: Manifest = client
        .get(&manifest_asset.browser_download_url)
        .send()
        .map_err(|e| e.to_string())?
        .json()
        .map_err(|e| e.to_string())?;

    let meta = manifest_for_target(&manifest, &target).map_err(|e| e.to_string())?;

    emit_stage(
        json,
        "download_parts",
        Some(json!({"parts": meta.parts.len()})),
    );

    let part_paths = download_parts_parallel(
        &client,
        &meta.parts,
        &assets,
        &out,
        concurrency,
        json,
    )?;

    emit_stage(json, "verify_parts", None);
    verify_parts(&meta.parts, &out)?;

    let output = out.join(&meta.image);
    emit_stage(
        json,
        "join_parts",
        Some(json!({"output": output.to_string_lossy()})),
    );
    join_parts(&part_paths, &output).map_err(|e| e.to_string())?;

    if let Some(expected) = meta.sha256_zst.as_ref() {
        let got = sha256_file(&output).map_err(|e| e.to_string())?;
        if !got.eq_ignore_ascii_case(expected) {
            return Err("SHA256 mismatch for joined image".to_string());
        }
    }

    emit_done(json, &output);
    Ok(())
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Fetch {
            repo,
            tag,
            target,
            out,
            json,
            no_json,
            concurrency,
        } => {
            let json_mode = if json {
                true
            } else if no_json {
                false
            } else {
                !stdout_is_tty()
            };
            if let Err(err) = run_fetch(repo, tag, target, out, json_mode, concurrency) {
                emit_error(json_mode, &err, json!({"command":"fetch"}));
                std::process::exit(1);
            }
        }
    }
}
