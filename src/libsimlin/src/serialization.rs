// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Serialization FFI functions.
//!
//! Functions for serializing projects to protobuf, JSON, XMILE, Vensim MDL,
//! systems, SVG, PNG, and scene formats. The memory for the output buffers is allocated via
//! `simlin_malloc` so that callers free it with `simlin_free`.

use simlin_engine::buffa::Message;
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
    store_warnings, write_bytes_to_ffi_output, ProjectContents, SimlinErrorCode, SimlinErrorKind,
    SimlinErrorSeverity, SimlinProject,
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

    // The fallible encode: the only failure is a project past protobuf's 2 GiB
    // message limit, and the panicking `encode_to_vec` would abort the process
    // there (release builds are `panic = abort`) instead of reporting it.
    let bytes = match pb_project.try_encode_to_vec() {
        Ok(bytes) => bytes,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::ProtobufDecode)
                    .with_message(format!("failed to encode project protobuf: {err}")),
            );
            return;
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

    // An `ExportWarning` carries only a message naming the affected variable,
    // dimension, or group.
    store_warnings(
        out_collected_errors,
        "MDL export",
        warnings.into_iter().map(|w| w.message),
        None,
    );
}

/// Check whether saving a project in a format keeps what its model means
///
/// Saves the project in `format` (a `SimlinSaveFormat`), reads the save back
/// as a host would open it, and compares the model the save holds with the
/// project's two ways (see `simlin_engine::save_check`):
///
/// - its definition: the simulation specs, each dimension's elements, parent
///   and mappings, and each variable's kind, dimensions, elements, equations
///   (in the spelling the engine resolves them by), `:EXCEPT:` default,
///   initial values, graphical functions, flows and the flags that change a
///   simulation, whether or not the project simulates;
/// - when the project simulates, every variable's series, value for value.
///
/// When the check cannot be sure a save keeps the meaning, it reports a
/// change. Units, documentation and views are not meaning; a save that
/// loses them reports them through the writers' own warnings.
///
/// Each change is a wire-`Generic` detail on the aggregate `SimlinError`
/// stored in `out_changes`: kind `Variable` with `variable_name` set when one
/// variable is at fault, else kind `Model`, with `model_name` set when the
/// change is in one model. `message` is `"<format> save: <reason>"` and
/// `details` the bare reason, such as `'demands1' is defined over dim2, not
/// dim`. The aggregate's own message counts them: `Saving as Vensim MDL
/// changes what this model means in 3 ways`.
///
/// The verdict is `out_changes` itself: NULL when the save keeps the model's
/// meaning, and otherwise the save must not be made in place. A detail's
/// severity only grades its change: `Error` when the save's results differ
/// from the project's now, `Warning` when only its definition does (an
/// equation the run never reaches, a flag it never exercises). When neither
/// the project nor its save simulates, every detail is a `Warning`, since
/// there are no results to compare.
///
/// `data_dir` is the directory the project's external data files are found
/// in, as `simlin_project_open_vensim_with_data` takes it, so an MDL save's
/// data references resolve as the project's did. Pass NULL for a project
/// opened without one. As there, it is read only when the `file_io` feature
/// is enabled.
///
/// Fails, with `out_error` set and `out_changes` NULL, when `format` is not a
/// `SimlinSaveFormat` or cannot hold the project at all (MDL holds one model),
/// as the serialize functions fail.
///
/// The check saves, reads and simulates the project and its save, so it
/// takes as long as those do: a host runs it off its main thread. It holds
/// the project's lock only to share its datamodel.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `data_dir` may be null; when non-null it must point to `data_dir_len`
///   bytes of valid UTF-8 naming a directory
/// - `out_changes` must be a valid pointer
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_check_save(
    project: *mut SimlinProject,
    format: u32,
    data_dir: *const u8,
    data_dir_len: usize,
    out_changes: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_changes.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message("out_changes must not be NULL"),
        );
        return;
    }
    *out_changes = ptr::null_mut();
    let Ok(format) = ffi::SimlinSaveFormat::try_from(format) else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message(format!("invalid save format discriminant: {format}")),
        );
        return;
    };
    let data_dir = if data_dir.is_null() {
        None
    } else {
        match std::str::from_utf8(std::slice::from_raw_parts(data_dir, data_dir_len)) {
            Ok(dir) => Some(dir),
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("data_dir is not valid UTF-8"),
                );
                return;
            }
        }
    };
    let proj = ffi_try!(out_error, require_project(project));
    let datamodel = proj.datamodel.lock().unwrap().shared();
    let format: engine::save_check::SaveFormat = format.into();
    #[cfg(feature = "file_io")]
    let provider = data_dir.map(simlin_engine::FilesystemDataProvider::new);
    #[cfg(feature = "file_io")]
    let data = provider
        .as_ref()
        .map(|p| p as &dyn simlin_engine::DataProvider);
    #[cfg(not(feature = "file_io"))]
    let data: Option<&dyn simlin_engine::DataProvider> = {
        let _ = data_dir;
        None
    };
    let changes = match engine::save_check::check_save_with_data(&datamodel, format, data) {
        Ok(changes) => changes,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("{} cannot hold this project: {err}", format.name())),
            );
            return;
        }
    };
    if changes.is_empty() {
        return;
    }
    let details: Vec<ErrorDetail> = changes
        .iter()
        .map(|change| ErrorDetail {
            message: Some(format!("{} save: {}", format.name(), change.reason)),
            model_name: change.model.clone(),
            variable_name: change.variable.clone(),
            kind: if change.variable.is_some() {
                SimlinErrorKind::Variable
            } else {
                SimlinErrorKind::Model
            },
            severity: match change.kind {
                engine::save_check::ChangeKind::Results => SimlinErrorSeverity::Error,
                engine::save_check::ChangeKind::Structure => SimlinErrorSeverity::Warning,
            },
            details: Some(change.reason.clone()),
            ..ErrorDetail::new(SimlinErrorCode::Generic)
        })
        .collect();
    let mut error = build_simlin_error(SimlinErrorCode::Generic, &details);
    let ways = if changes.len() == 1 {
        "one way".to_string()
    } else {
        format!("{} ways", changes.len())
    };
    error.set_message(Some(format!(
        "Saving as {} changes what this model means in {ways}",
        format.name()
    )));
    *out_changes = error.into_raw();
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
/// Locking: the caller holds the datamodel lock and passes the contents it
/// locked; this takes the db lock, matching the datamodel-then-db order used
/// project-wide.
fn datamodel_with_generated_layout(
    proj: &SimlinProject,
    contents: &ProjectContents,
    model_name: &str,
) -> Result<Option<engine::datamodel::Project>, String> {
    let datamodel: &engine::datamodel::Project = contents;
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

    let db_locked = proj.lock_db_with(contents);
    let db_state = db_locked
        .current_source_project()
        .map(|sp| (&*db_locked, sp));
    let layout = engine::layout::generate_best_layout(datamodel, model_name, db_state)?;

    let mut with_layout = datamodel.clone();
    // get_model above succeeded, so get_model_mut cannot fail here.
    with_layout.get_model_mut(model_name).unwrap().views =
        vec![engine::datamodel::View::StockFlow(layout)];
    Ok(Some(with_layout))
}

/// The body every `simlin_project_render_*` entry point shares: validate the
/// output pointers, the project and the model name; lay out a model with no
/// stock-and-flow view (transiently, `datamodel_with_generated_layout`); run
/// `render` over the datamodel and the model name; and return its bytes in a
/// `simlin_malloc` buffer. `what` names the output in error messages. One
/// body, so the renderings cannot disagree about which inputs they refuse or
/// which models they lay out.
///
/// # Safety
/// The contract of the entry points that call it: `project` must be a valid
/// pointer to a SimlinProject, `model_name` a valid null-terminated UTF-8
/// string, `out_buffer` and `out_len` valid pointers; `out_error` may be null.
unsafe fn render_model_to_buffer(
    project: *mut SimlinProject,
    model_name: *const c_char,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
    what: &str,
    render: impl FnOnce(&engine::datamodel::Project, &str) -> Result<Vec<u8>, String>,
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
    match render(render_target, model_name_str) {
        Ok(bytes) => {
            let len = bytes.len();

            let buf = simlin_malloc(len);
            if buf.is_null() {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message(format!("allocation failed while rendering {what}")),
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
                    .with_message(format!("failed to render {what}: {err}")),
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
    render_model_to_buffer(
        project,
        model_name,
        out_buffer,
        out_len,
        out_error,
        "SVG",
        |datamodel, model_name| {
            simlin_engine::diagram::render_svg(datamodel, model_name).map(String::into_bytes)
        },
    );
}

/// Render a project model's diagram as a scene display list
///
/// Returns the stock-and-flow diagram for the named model as a
/// resolution-independent display list, UTF-8 JSON: each drawn element's
/// shapes (rectangles, circles, and paths of move, line, cubic and close
/// commands), label lines and sparkline slot, in draw order, with every SVG
/// arc converted to cubics and every SVG transform applied. The geometry is
/// the geometry `simlin_project_render_svg` draws; `docs/design/diagram-scene.md`
/// is the format's contract.
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
pub unsafe extern "C" fn simlin_project_render_scene(
    project: *mut SimlinProject,
    model_name: *const c_char,
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    render_model_to_buffer(
        project,
        model_name,
        out_buffer,
        out_len,
        out_error,
        "scene",
        |datamodel, model_name| {
            simlin_engine::diagram::build_scene(datamodel, model_name)
                .and_then(|scene| scene.to_json())
                .map(String::into_bytes)
        },
    );
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
    let opts = simlin_engine::diagram::PngRenderOpts {
        width: if width > 0 { Some(width) } else { None },
        height: if height > 0 { Some(height) } else { None },
    };
    render_model_to_buffer(
        project,
        model_name,
        out_buffer,
        out_len,
        out_error,
        "PNG",
        |datamodel, model_name| {
            simlin_engine::diagram::render_png(datamodel, model_name, &opts)
                .map_err(|err| err.to_string())
        },
    );
}
