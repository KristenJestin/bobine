//! The few libmpv client calls the probe needs, over the raw bindings.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;

use libmpv2_sys as sys;

/// An mpv instance. The client API is thread-safe, so the handle may be shared between the UI and
/// the render thread.
pub struct Mpv(*mut sys::mpv_handle);

unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    pub fn new() -> Result<Mpv, String> {
        let handle = unsafe { sys::mpv_create() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        Ok(Mpv(handle))
    }

    pub fn handle(&self) -> *mut sys::mpv_handle {
        self.0
    }

    pub fn set_option(&self, name: &str, value: &str) -> Result<(), String> {
        let (name_c, value_c) = (c(name), c(value));
        check(
            unsafe { sys::mpv_set_option_string(self.0, name_c.as_ptr(), value_c.as_ptr()) },
            name,
        )
    }

    pub fn initialize(&self) -> Result<(), String> {
        check(unsafe { sys::mpv_initialize(self.0) }, "mpv_initialize")
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|arg| c(arg)).collect();
        let mut pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(ptr::null());
        check(
            unsafe { sys::mpv_command(self.0, pointers.as_mut_ptr()) },
            args[0],
        )
    }

    /// A property as mpv formats it, or `None` when it is unavailable.
    pub fn get(&self, name: &str) -> Option<String> {
        let name = c(name);
        let value = unsafe { sys::mpv_get_property_string(self.0, name.as_ptr()) };
        if value.is_null() {
            return None;
        }
        let text = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe { sys::mpv_free(value as *mut c_void) };
        Some(text)
    }

    pub fn get_i64(&self, name: &str) -> i64 {
        self.get(name).and_then(|v| v.parse().ok()).unwrap_or(-1)
    }

    /// Empties the event queue without waiting: the probe reads properties, not events.
    pub fn drain_events(&self) {
        while unsafe { (*sys::mpv_wait_event(self.0, 0.0)).event_id }
            != sys::mpv_event_id_MPV_EVENT_NONE
        {}
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe { sys::mpv_terminate_destroy(self.0) };
    }
}

pub fn c(text: &str) -> CString {
    CString::new(text).expect("no NUL byte in an mpv string")
}

pub fn check(code: i32, what: &str) -> Result<(), String> {
    if code >= 0 {
        return Ok(());
    }
    let reason = unsafe { CStr::from_ptr(sys::mpv_error_string(code)) };
    Err(format!("{what}: {}", reason.to_string_lossy()))
}
