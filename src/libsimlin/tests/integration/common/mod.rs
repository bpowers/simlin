// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use prost::Message;
use simlin::*;
use simlin_engine::serde as engine_serde;
use std::ffi::CStr;
use std::ptr;

/// Consume `err`, panicking with `ctx` if it is non-null.
///
/// Copies the code and message out of the error BEFORE freeing it: reading
/// the message pointer after `simlin_error_free` is a use-after-free, and an
/// earlier revision of these tests did exactly that -- a failing test printed
/// garbage bytes instead of the diagnostic (GH #898).
///
/// # Safety
/// `err` must be null or a valid `*mut SimlinError` owned by the caller.
pub unsafe fn expect_no_error(err: *mut SimlinError, ctx: &str) {
    if err.is_null() {
        return;
    }
    let code = simlin_error_get_code(err);
    let msg_ptr = simlin_error_get_message(err);
    let msg = if msg_ptr.is_null() {
        String::new()
    } else {
        CStr::from_ptr(msg_ptr).to_string_lossy().into_owned()
    };
    simlin_error_free(err);
    panic!("{ctx} failed with error {code:?}: {msg}");
}

/// Assert `err` is non-null and carries exactly `expected`; frees it.
///
/// # Safety
/// `err` must be null or a valid `*mut SimlinError` owned by the caller.
pub unsafe fn expect_error_code(err: *mut SimlinError, expected: SimlinErrorCode, ctx: &str) {
    assert!(!err.is_null(), "{ctx}: expected an error but got success");
    let code = simlin_error_get_code(err);
    simlin_error_free(err);
    assert_eq!(code, expected, "{ctx}: unexpected error code");
}

pub fn open_project_from_datamodel(
    project: &simlin_engine::datamodel::Project,
) -> *mut SimlinProject {
    let pb = engine_serde::serialize(project).unwrap();
    let mut buf = Vec::new();
    pb.encode(&mut buf).unwrap();
    unsafe {
        let mut err: *mut SimlinError = ptr::null_mut();
        let proj = simlin_project_open_protobuf(
            buf.as_ptr(),
            buf.len(),
            &mut err as *mut *mut SimlinError,
        );
        expect_no_error(err, "project open");
        assert!(!proj.is_null(), "project open failed");
        proj
    }
}
