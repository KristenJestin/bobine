//! libmpv itself, over its raw bindings: the `MpvClient` of the real player. mpv writes nothing to
//! the terminal and keeps no log: its log lines name the stream, whose address carries an
//! access token.

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr;

use libmpv2_sys as sys;

use super::PlayerError;
use super::mpv::{EndReason, MpvClient, MpvEvent, OBSERVED};

/// An mpv instance. Its client API is thread-safe: the render thread shares it.
pub struct Libmpv(*mut sys::mpv_handle);

// SAFETY: every libmpv client call may be made from any thread (client.h, "Thread safety").
unsafe impl Send for Libmpv {}
unsafe impl Sync for Libmpv {}

impl Libmpv {
    /// An mpv ready for the render API: no window of its own, no terminal, no configuration
    /// file, hardware decoding when it is safe. `options` come last (the headless checks add
    /// `ao=null`).
    #[allow(
        dead_code,
        reason = "made by whoever opens the player: see `Shell::play`"
    )]
    pub fn new(options: &[(&str, &str)]) -> Result<Libmpv, PlayerError> {
        // SAFETY: `mpv_create` has no precondition; a null handle is checked.
        let handle = unsafe { sys::mpv_create() };
        if handle.is_null() {
            return Err(PlayerError("mpv could not start".into()));
        }
        let mpv = Libmpv(handle);
        for (name, value) in [
            ("config", "no"),
            ("terminal", "no"),
            ("msg-level", "all=no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            ("osc", "no"),
            ("idle", "yes"),
            ("vo", "libmpv"),
            ("hwdec", "auto-safe"),
        ]
        .iter()
        .chain(options)
        {
            let (name_c, value_c) = (text(name), text(value));
            // SAFETY: the handle is alive and both strings are NUL-terminated.
            check(unsafe {
                sys::mpv_set_option_string(handle, name_c.as_ptr(), value_c.as_ptr())
            })?;
        }
        // SAFETY: the handle is alive and configured.
        check(unsafe { sys::mpv_initialize(handle) })?;
        for (index, name) in OBSERVED.iter().enumerate() {
            let name = text(name);
            // SAFETY: the handle is alive; the name is NUL-terminated and copied by mpv.
            check(unsafe {
                sys::mpv_observe_property(
                    handle,
                    index as u64,
                    name.as_ptr(),
                    sys::mpv_format_MPV_FORMAT_NONE,
                )
            })?;
        }
        Ok(mpv)
    }

    pub fn handle(&self) -> *mut sys::mpv_handle {
        self.0
    }
}

impl MpvClient for Libmpv {
    fn set(&self, name: &str, value: &str) -> Result<(), PlayerError> {
        let (name, value) = (text(name), text(value));
        // SAFETY: the handle is alive and both strings are NUL-terminated.
        check(unsafe { sys::mpv_set_property_string(self.0, name.as_ptr(), value.as_ptr()) })
    }

    fn command(&self, args: &[&str]) -> Result<(), PlayerError> {
        let owned: Vec<CString> = args.iter().map(|arg| text(arg)).collect();
        let mut pointers: Vec<*const c_char> = owned.iter().map(|arg| arg.as_ptr()).collect();
        pointers.push(ptr::null());
        // SAFETY: a NULL-terminated array of NUL-terminated strings that outlive the call.
        check(unsafe { sys::mpv_command(self.0, pointers.as_mut_ptr()) })
    }

    fn get(&self, name: &str) -> Option<String> {
        let name = text(name);
        // SAFETY: the handle is alive; a non-null result is a string mpv allocated, freed here.
        unsafe {
            let value = sys::mpv_get_property_string(self.0, name.as_ptr());
            if value.is_null() {
                return None;
            }
            let owned = CStr::from_ptr(value).to_string_lossy().into_owned();
            sys::mpv_free(value as *mut c_void);
            Some(owned)
        }
    }

    fn next_event(&self) -> Option<MpvEvent> {
        loop {
            // SAFETY: with a zero timeout, `mpv_wait_event` returns at once; the event stays
            // valid until the next call, and is copied out before it.
            let event = unsafe { &*sys::mpv_wait_event(self.0, 0.) };
            match event.event_id {
                sys::mpv_event_id_MPV_EVENT_NONE => return None,
                sys::mpv_event_id_MPV_EVENT_PROPERTY_CHANGE => {
                    let index = event.reply_userdata as usize;
                    if let Some(name) = OBSERVED.get(index) {
                        return Some(MpvEvent::Changed(name.to_string()));
                    }
                }
                sys::mpv_event_id_MPV_EVENT_END_FILE if !event.data.is_null() => {
                    // SAFETY: an END_FILE event's data is an `mpv_event_end_file`.
                    let end = unsafe { &*(event.data as *const sys::mpv_event_end_file) };
                    let reason = match end.reason {
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_EOF => EndReason::Eof,
                        sys::mpv_end_file_reason_MPV_END_FILE_REASON_ERROR => {
                            EndReason::Error(error_text(end.error))
                        }
                        _ => EndReason::Stopped,
                    };
                    return Some(MpvEvent::EndFile(reason));
                }
                _ => {}
            }
        }
    }
}

impl Drop for Libmpv {
    fn drop(&mut self) {
        // SAFETY: the last owner; the render context, which holds a reference, is gone.
        unsafe { sys::mpv_terminate_destroy(self.0) };
    }
}

fn text(value: &str) -> CString {
    // A NUL byte cannot come from an address, a header or a property name worth sending: it
    // becomes an empty string, which mpv refuses on its own.
    CString::new(value).unwrap_or_default()
}

/// mpv's error code in words: never its log.
fn error_text(code: i32) -> String {
    // SAFETY: `mpv_error_string` returns a static string for any code.
    unsafe { CStr::from_ptr(sys::mpv_error_string(code)) }
        .to_string_lossy()
        .into_owned()
}

pub fn check(code: i32) -> Result<(), PlayerError> {
    if code >= 0 {
        Ok(())
    } else {
        Err(PlayerError(error_text(code)))
    }
}
