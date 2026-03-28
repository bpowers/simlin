// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use prost::Message;
use simlin::*;
use simlin_engine::serde as engine_serde;
use std::ffi::CStr;
use std::ptr;

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
        assert!(!proj.is_null(), "project open failed");
        if !err.is_null() {
            let code = simlin_error_get_code(err);
            let msg_ptr = simlin_error_get_message(err);
            let msg = if !msg_ptr.is_null() {
                CStr::from_ptr(msg_ptr).to_str().unwrap_or("")
            } else {
                ""
            };
            simlin_error_free(err);
            panic!("project open failed with code {:?}: {}", code, msg);
        }
        proj
    }
}
