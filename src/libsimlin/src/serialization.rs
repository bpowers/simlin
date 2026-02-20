// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Serialization FFI functions.
//!
//! Functions for serializing projects to protobuf, JSON, XMILE, SVG, and
//! PNG formats. The memory for the output buffers is allocated via
//! `simlin_malloc` so that callers free it with `simlin_free`.

use prost::Message;
use simlin_engine::{self as engine, serde as engine_serde};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

use crate::ffi;
use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::memory::simlin_malloc;
use crate::{
    clear_out_error, require_project, store_anyhow_error, store_error, SimlinErrorCode,
    SimlinProject,
};

/// Serialize a project to binary protobuf format
///
/// Serializes the project's datamodel to Simlin's native protobuf format.
/// This is the recommended format for saving and restoring projects, as it
/// preserves all project data with perfect fidelity. The serialized bytes
/// can be loaded later with `simlin_project_open_protobuf`.
///
/// Caller must free output with `simlin_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `out_buffer` and `out_len` must be valid pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize_protobuf(
    project: *mut SimlinProject,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buffer.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }

    // Clear output pointers upfront so callers that ignore errors don't free stale pointers
    *out_buffer = ptr::null_mut();
    *out_len = 0;

    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    let project_locked = proj.project.lock().unwrap();
    let pb_project = engine_serde::serialize(&project_locked.datamodel);

    let mut bytes = Vec::new();
    if pb_project.encode(&mut bytes).is_err() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::ProtobufDecode)
                .with_message("failed to encode project protobuf"),
        );
        return;
    }

    let len = bytes.len();
    let buf = simlin_malloc(len);
    if buf.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("allocation failed while serializing project"),
        );
        return;
    }

    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);

    *out_buffer = buf;
    *out_len = len;
}

/// Serializes a project to JSON format.
///
/// # Safety
/// - `project` must point to a valid `SimlinProject`.
/// - `out_buffer` and `out_len` must be valid pointers where the serialized
///   bytes and length will be written.
/// - `out_error` must be a valid pointer for receiving error details and may
///   be set to null on success.
///
/// # Thread Safety
/// - This function is thread-safe for concurrent calls with the same `project` pointer.
/// - The underlying `engine::Project` uses `Arc<ModelStage1>` and is protected by a `Mutex`.
/// - Multiple threads may safely access the same project concurrently.
/// - Different projects may also be serialized concurrently from different threads safely.
///
/// # Ownership
/// - Serialization creates a deep copy of the project datamodel via `clone()`.
/// - The original `project` remains fully usable after serialization.
/// - The returned buffer is exclusively owned by the caller and MUST be freed with `simlin_free`.
/// - The caller is responsible for freeing the buffer even if subsequent operations fail.
///
/// # Buffer Lifetime
/// - The serialized JSON buffer remains valid until `simlin_free` is called on it.
/// - Multiple serializations can be performed concurrently (separate buffers are independent).
/// - It is safe to serialize the same project multiple times.
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize_json(
    project: *mut SimlinProject,
    format: u32,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buffer.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }

    *out_buffer = ptr::null_mut();
    *out_len = 0;

    let format = match ffi::SimlinJsonFormat::try_from(format) {
        Ok(f) => f,
        Err(()) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("invalid JSON format discriminant: {format}")),
            );
            return;
        }
    };

    let project_ref = match require_project(project) {
        Ok(proj) => proj,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    let project_locked = project_ref.project.lock().unwrap();
    let bytes = match format {
        ffi::SimlinJsonFormat::Native => {
            let json_project: engine::json::Project = project_locked.datamodel.clone().into();
            match serde_json::to_vec(&json_project) {
                Ok(data) => data,
                Err(err) => {
                    store_error(
                        out_error,
                        SimlinError::new(SimlinErrorCode::Generic)
                            .with_message(format!("failed to encode native JSON project: {err}")),
                    );
                    return;
                }
            }
        }
        ffi::SimlinJsonFormat::Sdai => {
            let sdai_model: engine::json_sdai::SdaiModel = project_locked.datamodel.clone().into();
            match serde_json::to_vec(&sdai_model) {
                Ok(data) => data,
                Err(err) => {
                    store_error(
                        out_error,
                        SimlinError::new(SimlinErrorCode::Generic)
                            .with_message(format!("failed to encode SDAI JSON model: {err}")),
                    );
                    return;
                }
            }
        }
    };

    let len = bytes.len();
    let buf = simlin_malloc(len);
    if buf.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("allocation failed while serializing project"),
        );
        return;
    }

    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);

    *out_buffer = buf;
    *out_len = len;
}

/// Serialize a project to XMILE format
///
/// Exports a project to XMILE format, the industry standard interchange format
/// for system dynamics models. The output buffer contains the XML document as
/// UTF-8 encoded bytes.
///
/// Caller must free output with `simlin_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `out_buffer` and `out_len` must be valid pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize_xmile(
    project: *mut SimlinProject,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buffer.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }

    // Clear output pointers upfront so callers that ignore errors don't free stale pointers
    *out_buffer = ptr::null_mut();
    *out_len = 0;

    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    let project_locked = proj.project.lock().unwrap();
    match simlin_engine::to_xmile(&project_locked.datamodel) {
        Ok(xmile_str) => {
            let bytes = xmile_str.into_bytes();
            let len = bytes.len();

            let buf = simlin_malloc(len);
            if buf.is_null() {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("allocation failed while exporting XMILE"),
                );
                return;
            }

            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);

            *out_buffer = buf;
            *out_len = len;
        }
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("failed to export XMILE: {err}")),
            );
        }
    }
}

/// Render a project model's diagram as SVG
///
/// Renders the stock-and-flow diagram for the named model to a standalone
/// SVG document (UTF-8 encoded). The output includes embedded CSS styles
/// and is suitable for display or export.
///
/// Caller must free output with `simlin_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` must be a valid null-terminated UTF-8 string
/// - `out_buffer` and `out_len` must be valid pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_render_svg(
    project: *mut SimlinProject,
    model_name: *const c_char,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buffer.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }

    *out_buffer = ptr::null_mut();
    *out_len = 0;

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

    let project_locked = proj.project.lock().unwrap();
    match simlin_engine::diagram::render_svg(&project_locked.datamodel, model_name_str) {
        Ok(svg_str) => {
            let bytes = svg_str.into_bytes();
            let len = bytes.len();

            let buf = simlin_malloc(len);
            if buf.is_null() {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("allocation failed while rendering SVG"),
                );
                return;
            }

            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);

            *out_buffer = buf;
            *out_len = len;
        }
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to render SVG: {err}")),
            );
        }
    }
}

/// Render a project model's diagram as a PNG image
///
/// Renders the stock-and-flow diagram for the named model to a PNG image.
/// The SVG is generated internally and then rasterized with the Roboto Light
/// font embedded in the binary. Pass `width = 0` and `height = 0` to use
/// the SVG's intrinsic dimensions. When only one dimension is non-zero the
/// other is derived from the aspect ratio. When both are non-zero, `width`
/// takes precedence and `height` is derived from the aspect ratio.
///
/// Caller must free output with `simlin_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` must be a valid null-terminated UTF-8 string
/// - `out_buffer` and `out_len` must be valid pointers
/// - `out_error` may be null
#[cfg(feature = "png_render")]
#[no_mangle]
pub unsafe extern "C" fn simlin_project_render_png(
    project: *mut SimlinProject,
    model_name: *const c_char,
    width: u32,
    height: u32,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_buffer.is_null() || out_len.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }

    *out_buffer = ptr::null_mut();
    *out_len = 0;

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

    let opts = simlin_engine::diagram::PngRenderOpts {
        width: if width > 0 { Some(width) } else { None },
        height: if height > 0 { Some(height) } else { None },
    };

    let project_locked = proj.project.lock().unwrap();
    match simlin_engine::diagram::render_png(&project_locked.datamodel, model_name_str, &opts) {
        Ok(png_bytes) => {
            let len = png_bytes.len();

            let buf = simlin_malloc(len);
            if buf.is_null() {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("allocation failed while rendering PNG"),
                );
                return;
            }

            std::ptr::copy_nonoverlapping(png_bytes.as_ptr(), buf, len);

            *out_buffer = buf;
            *out_len = len;
        }
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to render PNG: {err}")),
            );
        }
    }
}
