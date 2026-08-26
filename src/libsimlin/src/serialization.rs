// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Serialization FFI functions.
//!
//! Functions for serializing projects to protobuf, JSON, XMILE, Vensim MDL,
//! systems, SVG, and PNG formats. The memory for the output buffers is allocated via
//! `simlin_malloc` so that callers free it with `simlin_free`.

use prost::Message;
use simlin_engine::{self as engine, serde as engine_serde};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;

use crate::ffi;
use crate::ffi_error::{ErrorDetail, SimlinError};
use crate::ffi_try;
use crate::memory::simlin_malloc;
use crate::{
    build_simlin_error, clear_out_error, require_project, store_anyhow_error, store_error,
    write_bytes_to_ffi_output, SimlinErrorCode, SimlinErrorKind, SimlinErrorSeverity,
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

    let datamodel_locked = proj.datamodel.lock().unwrap();
    let pb_project = match engine_serde::serialize(&datamodel_locked) {
        Ok(pb) => pb,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("serialization validation failed: {}", err)),
            );
            return;
        }
    };

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
/// - The project's datamodel is held in a `Mutex`, so concurrent readers serialize on it.
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
    include_stdlib: bool,
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

    // When include_stdlib is true, enrich a clone with stdlib model
    // definitions so the TypeScript diagram editor can display and
    // navigate into stdlib modules. When false (e.g. for persistence),
    // serialize the datamodel as-is to avoid storing engine internals.
    let datamodel = project_ref.datamodel.lock().unwrap().clone();
    let datamodel = if include_stdlib {
        let mut enriched = datamodel;
        enriched.ensure_referenced_stdlib_models();
        enriched
    } else {
        datamodel
    };
    let bytes = match format {
        ffi::SimlinJsonFormat::Native => {
            let json_project: engine::json::Project = datamodel.into();
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
            let sdai_model: engine::json_sdai::SdaiModel = datamodel.into();
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

    let datamodel_locked = proj.datamodel.lock().unwrap();
    match simlin_engine::to_xmile(&datamodel_locked) {
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

/// Serialize a project to Vensim MDL format
///
/// Exports the project's single model (plus any macro-marked models, emitted
/// as `:MACRO:` blocks) as Vensim MDL text, including the sketch section for
/// a model that has a diagram view. The output buffer contains UTF-8 text.
///
/// The MDL surface cannot represent every Simlin construct. The engine's
/// lossiness contract (`simlin_engine::mdl::project_to_mdl_with_warnings`)
/// splits the gap in two, and this function surfaces both halves separately:
///
/// - **Hard errors** (a project with more than one ordinary model, an
///   ordinary module instance, an unreconstructable macro cluster) fail the
///   export: `out_error` is set and no buffer is produced.
/// - **Lossiness warnings** (a dropped non-negative flag, a discrete or
///   extrapolating lookup emitted in the closest representable form, a
///   truncated group name, ...) do NOT fail the export. The text is still
///   written, and each warning is reported as a `Warning`-severity detail on
///   `out_collected_errors` (an aggregate `SimlinError`, freed with
///   `simlin_error_free`; NULL when there were no warnings). Pass NULL for
///   `out_collected_errors` to discard the warnings. This mirrors how
///   `simlin_project_apply_patch` separates a rejection (`out_error`) from
///   the diagnostics it collected along the way (`out_collected_errors`).
///
/// Caller must free the output buffer with `simlin_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `out_buffer` and `out_len` must be valid pointers
/// - `out_collected_errors` may be null
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize_mdl(
    project: *mut SimlinProject,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !out_collected_errors.is_null() {
        *out_collected_errors = ptr::null_mut();
    }
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

    let proj = ffi_try!(out_error, require_project(project));

    let datamodel_locked = proj.datamodel.lock().unwrap();
    let (mdl_text, warnings) = match simlin_engine::to_mdl_with_warnings(&datamodel_locked) {
        Ok(result) => result,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("failed to export MDL: {err}")),
            );
            return;
        }
    };
    drop(datamodel_locked);

    if !write_bytes_to_ffi_output(mdl_text.as_bytes(), out_buffer, out_len, out_error, "MDL") {
        return;
    }

    if out_collected_errors.is_null() || warnings.is_empty() {
        return;
    }

    // An `ExportWarning` carries only a message naming the affected variable,
    // dimension, or group; there is no engine `ErrorCode` for lossiness, so
    // each rides the wire `Generic` code with `Warning` severity -- the
    // severity is what tells a caller the export succeeded.
    let details: Vec<ErrorDetail> = warnings
        .into_iter()
        .map(|w| ErrorDetail {
            message: Some(format!("MDL export: {}", w.message)),
            kind: SimlinErrorKind::Model,
            severity: SimlinErrorSeverity::Warning,
            details: Some(w.message),
            ..ErrorDetail::new(SimlinErrorCode::Generic)
        })
        .collect();
    *out_collected_errors = build_simlin_error(SimlinErrorCode::Generic, &details).into_raw();
}

/// Serialize a project to systems format
///
/// Exports a project to the systems format (`.txt` line-oriented notation).
/// The output buffer contains the text as UTF-8 encoded bytes.
///
/// Caller must free output with `simlin_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `out_buffer` and `out_len` must be valid pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize_systems(
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

    *out_buffer = ptr::null_mut();
    *out_len = 0;

    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    let datamodel_locked = proj.datamodel.lock().unwrap();
    match simlin_engine::to_systems(&datamodel_locked) {
        Ok(systems_str) => {
            let bytes = systems_str.into_bytes();
            let len = bytes.len();

            let buf = simlin_malloc(len);
            if buf.is_null() {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("allocation failed while exporting systems format"),
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
                    .with_message(format!("failed to export systems format: {err}")),
            );
        }
    }
}

/// When the named model has no stock-and-flow view (or only an empty one),
/// return a clone of the datamodel carrying an automatically generated
/// layout for it, so a programmatically built model renders without the
/// caller first creating a view. Returns `Ok(None)` when the existing view
/// is usable (or the model does not exist -- the renderer reports that
/// case itself, distinguishing "not found" from layout failures).
///
/// The layout is deliberately transient: rendering is a read, so the
/// generated view is never written back to the project. Callers that want
/// a persisted view use `simlin_project_diagram_sync`.
///
/// Locking: the caller holds the datamodel lock; this takes the db lock,
/// matching the datamodel-then-db order used project-wide.
fn datamodel_with_generated_layout(
    proj: &SimlinProject,
    datamodel: &engine::datamodel::Project,
    model_name: &str,
) -> Result<Option<engine::datamodel::Project>, String> {
    let Some(model) = datamodel.get_model(model_name) else {
        return Ok(None);
    };
    let has_view = model
        .views
        .first()
        .map(|engine::datamodel::View::StockFlow(sf)| !sf.elements.is_empty())
        .unwrap_or(false);
    if has_view {
        return Ok(None);
    }

    let mut db_locked = proj.db.lock().unwrap();
    let db_state = db_locked
        .current_source_project()
        .map(|sp| (&mut *db_locked, sp));
    let layout = engine::layout::generate_best_layout(datamodel, model_name, db_state)?;

    let mut with_layout = datamodel.clone();
    // get_model above succeeded, so get_model_mut cannot fail here.
    with_layout.get_model_mut(model_name).unwrap().views =
        vec![engine::datamodel::View::StockFlow(layout)];
    Ok(Some(with_layout))
}

/// Render a project model's diagram as SVG
///
/// Renders the stock-and-flow diagram for the named model to a standalone
/// SVG document (UTF-8 encoded). The output includes embedded CSS styles
/// and is suitable for display or export.
///
/// A model without a stock-and-flow view (e.g. one built programmatically
/// through the patch API) is rendered with an automatically generated
/// layout; the generated view is transient and not persisted.
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

    let datamodel_locked = proj.datamodel.lock().unwrap();
    let laid_out = match datamodel_with_generated_layout(proj, &datamodel_locked, model_name_str) {
        Ok(l) => l,
        Err(msg) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to lay out diagram: {msg}")),
            );
            return;
        }
    };
    let render_target = laid_out.as_ref().unwrap_or(&datamodel_locked);
    match simlin_engine::diagram::render_svg(render_target, model_name_str) {
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
/// A model without a stock-and-flow view (e.g. one built programmatically
/// through the patch API) is rendered with an automatically generated
/// layout; the generated view is transient and not persisted.
///
/// Only available with the `png_render` feature (on by default; the browser
/// wasm artifact is built without it to keep the resvg/text-shaping stack
/// out of the bundle browsers download).
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

    let datamodel_locked = proj.datamodel.lock().unwrap();
    let laid_out = match datamodel_with_generated_layout(proj, &datamodel_locked, model_name_str) {
        Ok(l) => l,
        Err(msg) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to lay out diagram: {msg}")),
            );
            return;
        }
    };
    let render_target = laid_out.as_ref().unwrap_or(&datamodel_locked);
    match simlin_engine::diagram::render_png(render_target, model_name_str, &opts) {
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
