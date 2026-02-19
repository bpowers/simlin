// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Layout FFI function.
//!
//! Exposes the automatic diagram layout engine through the C FFI.

use simlin_engine::{self as engine};
use std::ffi::CStr;
use std::os::raw::c_char;

use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::{clear_out_error, require_project, store_error, SimlinErrorCode, SimlinProject};

/// Generate the best automatic layout for the named model and replace its
/// views in-place.
///
/// Preserves the existing zoom level if the model already has a view with
/// zoom > 0. Works on all targets including WASM (uses a serial fallback
/// when rayon is unavailable).
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` must be a valid null-terminated UTF-8 string
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_diagram_sync(
    project: *mut SimlinProject,
    model_name: *const c_char,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);

    let proj = ffi_try!(out_error, require_project(project));

    if model_name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("model name pointer must not be NULL"),
        );
        return;
    }

    let model_name_str = match CStr::from_ptr(model_name).to_str() {
        Ok(s) if !s.is_empty() => s,
        Ok(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("model name must not be empty"),
            );
            return;
        }
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("model name is not valid UTF-8"),
            );
            return;
        }
    };

    let mut project_locked = proj.project.lock().unwrap();

    // Preserve existing zoom if the model already has a view
    let existing_zoom = project_locked
        .datamodel
        .get_model(model_name_str)
        .and_then(|m| m.views.first())
        .map(|v| match v {
            engine::datamodel::View::StockFlow(sf) => sf.zoom,
        })
        .filter(|&z| z > 0.0);

    let mut layout =
        match engine::layout::generate_best_layout(&project_locked.datamodel, model_name_str) {
            Ok(l) => l,
            Err(msg) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(msg),
                );
                return;
            }
        };

    if let Some(zoom) = existing_zoom {
        layout.zoom = zoom;
    }

    match project_locked.datamodel.get_model_mut(model_name_str) {
        Some(model) => {
            model.views = vec![engine::datamodel::View::StockFlow(layout)];
        }
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist)
                    .with_message(format!("model '{}' not found in project", model_name_str)),
            );
        }
    }
}
