use libc::{c_char, c_int, c_void};
use std::ffi::{CStr, CString};
use std::fmt;
use std::ptr;
use std::sync::Mutex;

#[derive(Debug, Clone)]
pub struct UsbImagerError {
    message: String,
}

impl UsbImagerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self { message: message.into() }
    }
}

impl fmt::Display for UsbImagerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for UsbImagerError {}

#[repr(C)]
struct RlDevice {
    id: *mut c_char,
    label: *mut c_char,
    size_bytes: u64,
    is_removable: c_int,
}

#[repr(C)]
struct RlJob {
    _private: [u8; 1],
}

type ProgressCb = Option<unsafe extern "C" fn(*mut c_void, u64, u64, *const c_char)>;
type ErrorCb = Option<unsafe extern "C" fn(*mut c_void, *const c_char)>;

#[cfg(target_os = "linux")]
extern "C" {
    fn rl_list_devices(show_all: c_int, out_devices: *mut *mut RlDevice, out_len: *mut usize) -> c_int;
    fn rl_free_devices(devices: *mut RlDevice, len: usize);

    fn rl_write_image_zst(
        image_path: *const c_char,
        device_id: *const c_char,
        verify: c_int,
        progress_cb: ProgressCb,
        error_cb: ErrorCb,
        user: *mut c_void,
    ) -> *mut RlJob;
    fn rl_cancel(job: *mut RlJob) -> c_int;
    fn rl_wait(job: *mut RlJob) -> c_int;
    fn rl_free(job: *mut RlJob);
    fn rl_last_error() -> *const c_char;
}

#[cfg(not(target_os = "linux"))]
unsafe fn rl_last_error() -> *const c_char {
    ptr::null()
}

fn last_error_string() -> UsbImagerError {
    unsafe {
        let ptr = rl_last_error();
        if ptr.is_null() {
            UsbImagerError::new("USBImager error")
        } else {
            let msg = CStr::from_ptr(ptr).to_string_lossy().into_owned();
            UsbImagerError::new(msg)
        }
    }
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub label: String,
    pub size_bytes: u64,
    pub is_removable: bool,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub message: String,
}

struct CallbackState {
    progress: Mutex<Option<Box<dyn FnMut(Progress) + Send>>>,
    error: Mutex<Option<Box<dyn FnMut(String) + Send>>>,
}

unsafe extern "C" fn progress_trampoline(user: *mut c_void, done: u64, total: u64, message: *const c_char) {
    if user.is_null() {
        return;
    }
    let state = &*(user as *mut CallbackState);
    let msg = if message.is_null() {
        String::new()
    } else {
        CStr::from_ptr(message).to_string_lossy().into_owned()
    };
    let progress = Progress { done, total, message: msg };
    let _ = std::panic::catch_unwind(|| {
        if let Ok(mut guard) = state.progress.lock() {
            if let Some(cb) = guard.as_mut() {
                cb(progress);
            }
        }
    });
}

unsafe extern "C" fn error_trampoline(user: *mut c_void, message: *const c_char) {
    if user.is_null() {
        return;
    }
    let state = &*(user as *mut CallbackState);
    let msg = if message.is_null() {
        String::new()
    } else {
        CStr::from_ptr(message).to_string_lossy().into_owned()
    };
    let _ = std::panic::catch_unwind(|| {
        if let Ok(mut guard) = state.error.lock() {
            if let Some(cb) = guard.as_mut() {
                cb(msg);
            }
        }
    });
}

pub fn list_devices(show_all: bool) -> Result<Vec<Device>, UsbImagerError> {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut ptr: *mut RlDevice = ptr::null_mut();
        let mut len: usize = 0;
        let res = rl_list_devices(show_all as c_int, &mut ptr, &mut len as *mut usize);
        if res != 0 {
            return Err(last_error_string());
        }
        if ptr.is_null() || len == 0 {
            return Ok(Vec::new());
        }
        let slice = std::slice::from_raw_parts(ptr, len);
        let mut out = Vec::with_capacity(len);
        for dev in slice {
            let id = if dev.id.is_null() {
                String::new()
            } else {
                CStr::from_ptr(dev.id).to_string_lossy().into_owned()
            };
            let label = if dev.label.is_null() {
                String::new()
            } else {
                CStr::from_ptr(dev.label).to_string_lossy().into_owned()
            };
            out.push(Device {
                id,
                label,
                size_bytes: dev.size_bytes,
                is_removable: dev.is_removable != 0,
            });
        }
        rl_free_devices(ptr, len);
        Ok(out)
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = show_all;
        Err(UsbImagerError::new("USBImager engine is only available on Linux"))
    }
}

pub struct WriteJob {
    handle: *mut RlJob,
    cb_state: *mut CallbackState,
}

unsafe impl Send for WriteJob {}
unsafe impl Sync for WriteJob {}

impl WriteJob {
    pub fn cancel(&self) -> Result<(), UsbImagerError> {
        #[cfg(target_os = "linux")]
        unsafe {
            if self.handle.is_null() {
                return Err(UsbImagerError::new("Invalid job handle"));
            }
            if rl_cancel(self.handle) != 0 {
                return Err(last_error_string());
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            Err(UsbImagerError::new("USBImager engine is only available on Linux"))
        }
    }

    pub fn wait(mut self) -> Result<(), UsbImagerError> {
        #[cfg(target_os = "linux")]
        unsafe {
            if self.handle.is_null() {
                return Err(UsbImagerError::new("Invalid job handle"));
            }
            let res = rl_wait(self.handle);
            rl_free(self.handle);
            self.handle = ptr::null_mut();
            if !self.cb_state.is_null() {
                drop(Box::from_raw(self.cb_state));
                self.cb_state = ptr::null_mut();
            }
            if res == 0 {
                Ok(())
            } else {
                Err(last_error_string())
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = &mut self;
            Err(UsbImagerError::new("USBImager engine is only available on Linux"))
        }
    }
}

impl Drop for WriteJob {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        unsafe {
            if !self.handle.is_null() {
                rl_free(self.handle);
                self.handle = ptr::null_mut();
            }
            if !self.cb_state.is_null() {
                drop(Box::from_raw(self.cb_state));
                self.cb_state = ptr::null_mut();
            }
        }
    }
}

pub fn write_image_zst(
    image_path: &str,
    device_id: &str,
    verify: bool,
    progress_cb: Option<Box<dyn FnMut(Progress) + Send>>,
    error_cb: Option<Box<dyn FnMut(String) + Send>>,
) -> Result<WriteJob, UsbImagerError> {
    #[cfg(target_os = "linux")]
    unsafe {
        let image_c = CString::new(image_path).map_err(|_| UsbImagerError::new("Invalid image path"))?;
        let device_c = CString::new(device_id).map_err(|_| UsbImagerError::new("Invalid device id"))?;

        let cb_state = if progress_cb.is_some() || error_cb.is_some() {
            Box::into_raw(Box::new(CallbackState {
                progress: Mutex::new(progress_cb),
                error: Mutex::new(error_cb),
            }))
        } else {
            ptr::null_mut()
        };

        let handle = rl_write_image_zst(
            image_c.as_ptr(),
            device_c.as_ptr(),
            verify as c_int,
            if cb_state.is_null() { None } else { Some(progress_trampoline) },
            if cb_state.is_null() { None } else { Some(error_trampoline) },
            cb_state as *mut c_void,
        );
        if handle.is_null() {
            if !cb_state.is_null() {
                drop(Box::from_raw(cb_state));
            }
            return Err(last_error_string());
        }
        Ok(WriteJob { handle, cb_state })
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = image_path;
        let _ = device_id;
        let _ = verify;
        let _ = progress_cb;
        let _ = error_cb;
        Err(UsbImagerError::new("USBImager engine is only available on Linux"))
    }
}
