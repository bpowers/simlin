// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Custom panic hook for WASM builds.
//!
//! With `panic = "abort"`, destructors don't run after a panic, so
//! `console_error_panic_hook` (which needs wasm-bindgen) isn't an option.
//! Instead we stash the panic message in a global buffer that survives the
//! WASM trap.  After JS catches the `RuntimeError: unreachable`, it can
//! call `simlin_get_panic_message()` to retrieve the real panic text.

use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::Mutex;

static PANIC_MESSAGE: Mutex<Option<CString>> = Mutex::new(None);

/// Install the panic hook.  Safe to call multiple times (idempotent via
/// `set_hook` replacing any prior hook).
fn install() {
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        if let Ok(mut guard) = PANIC_MESSAGE.lock() {
            // CString::new replaces interior NULs, so this won't fail
            // in any way that matters for diagnostics.
            *guard = CString::new(msg).ok();
        }
    }));
}

/// Install the panic hook so that subsequent panics stash their message
/// in a buffer readable via `simlin_get_panic_message()`.
///
/// Call once from JS after WASM instantiation.
#[no_mangle]
pub extern "C" fn simlin_init() {
    install();
}

/// Return the last panic message as a null-terminated C string, or null
/// if no panic has been recorded.  The pointer is valid until the next
/// panic or until `simlin_clear_panic_message()` is called.
///
/// # Safety
/// The returned pointer borrows the global buffer and must not be freed
/// by the caller.
#[no_mangle]
pub extern "C" fn simlin_get_panic_message() -> *const c_char {
    match PANIC_MESSAGE.lock() {
        Ok(guard) => match guard.as_ref() {
            Some(cstr) => cstr.as_ptr(),
            None => std::ptr::null(),
        },
        Err(_) => std::ptr::null(),
    }
}

/// Clear the stored panic message.
#[no_mangle]
pub extern "C" fn simlin_clear_panic_message() {
    if let Ok(mut guard) = PANIC_MESSAGE.lock() {
        *guard = None;
    }
}
