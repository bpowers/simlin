// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Project lifecycle FFI functions.
//!
//! Opening projects from various formats (protobuf, JSON, XMILE, Vensim),
//! reference counting, querying models, and checking simulatability.

use anyhow::{anyhow, Result};
use simlin_engine::buffa::Message;
use simlin_engine::{self as engine, serde as engine_serde};
use std::ffi::{CStr, CString};
use std::io::BufReader;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::ffi;
use crate::ffi_error::{FfiError, SimlinError};
use crate::ffi_try;
use crate::patch::gather_error_details_with_db;
use crate::{
    build_simlin_error, clear_out_error, drop_c_string, require_project, store_anyhow_error,
    store_error, store_warnings, SimlinErrorCode, SimlinModel, SimlinProject,
};

/// Open a project from binary protobuf data
///
/// Deserializes a project from Simlin's native protobuf format. This is the
/// recommended format for loading previously saved projects, as it preserves
/// all project data with perfect fidelity.
///
/// Returns NULL and populates `out_error` on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_protobuf(
    data: *const u8,
    len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);

    let result: Result<*mut SimlinProject> = (|| {
        if data.is_null() {
            return Err(FfiError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL")
                .into());
        }

        let slice = unsafe { std::slice::from_raw_parts(data, len) };
        // buffa's default decode limits apply, including its 32 MiB budget on
        // the memory repeated fields may materialize. That budget is what keeps a
        // crafted payload from amplifying into gigabytes, and real projects sit
        // far below it: C-LEARN, the largest model in the test corpus, needs
        // under a twentieth of it.
        let pb_project =
            engine::project_io::Project::decode_from_slice(slice).map_err(|decode_err| {
                FfiError::new(SimlinErrorCode::ProtobufDecode)
                    .with_message(format!("failed to decode project protobuf: {decode_err}"))
            })?;

        // Bytes that decode are not thereby a project: one that lacks what
        // every writer writes (an empty buffer, a truncated save) is refused
        // with what it lacks.
        let datamodel_project: engine::datamodel::Project = engine_serde::deserialize(pb_project)
            .map_err(|err| {
            FfiError::new(SimlinErrorCode::ProtobufDecode)
                .with_message(format!("the protobuf is not a project: {}", err.reason()))
        })?;
        Ok(Box::into_raw(Box::new(SimlinProject::new(
            datamodel_project,
        ))))
    })();

    match result {
        Ok(ptr) => ptr,
        Err(err) => {
            store_anyhow_error(out_error, err);
            ptr::null_mut()
        }
    }
}

/// Open a project from JSON data
///
/// Deserializes a project from JSON format. Supports two formats:
/// - `SimlinJsonFormat::Native` (0): Simlin's native JSON representation
/// - `SimlinJsonFormat::Sdai` (1): System Dynamics AI (SDAI) interchange format
///
/// Returns NULL and populates `out_error` on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes of UTF-8 JSON
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
/// - `format` must be a valid discriminant (0 or 1), otherwise an error is returned
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_json(
    data: *const u8,
    len: usize,
    format: u32,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);

    let result: Result<*mut SimlinProject> = (|| {
        if data.is_null() {
            return Err(FfiError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL")
                .into());
        }

        let format = ffi::SimlinJsonFormat::try_from(format).map_err(|()| {
            FfiError::new(SimlinErrorCode::Generic)
                .with_message(format!("invalid JSON format discriminant: {format}"))
        })?;

        let slice = unsafe { std::slice::from_raw_parts(data, len) };
        let json_str = std::str::from_utf8(slice).map_err(|utf8_err| {
            FfiError::new(SimlinErrorCode::Generic)
                .with_message(format!("input JSON is not valid UTF-8: {utf8_err}"))
        })?;

        let datamodel_project: engine::datamodel::Project = match format {
            ffi::SimlinJsonFormat::Native => {
                let json_project: engine::json::Project = engine::json::Project::from_reader(
                    json_str.as_bytes(),
                )
                .map_err(|engine_err: engine::Error| {
                    FfiError::new(SimlinErrorCode::Generic).with_message(engine_err.to_string())
                })?;
                json_project.into()
            }
            ffi::SimlinJsonFormat::Sdai => {
                let sdai_model: engine::json_sdai::SdaiModel =
                    engine::json_sdai::SdaiModel::from_reader(json_str.as_bytes()).map_err(
                        |engine_err: engine::Error| {
                            FfiError::new(SimlinErrorCode::Generic)
                                .with_message(engine_err.to_string())
                        },
                    )?;
                sdai_model.into()
            }
        };

        Ok(Box::into_raw(Box::new(SimlinProject::new(
            datamodel_project,
        ))))
    })();

    match result {
        Ok(ptr) => ptr,
        Err(err) => {
            store_anyhow_error(out_error, err);
            ptr::null_mut()
        }
    }
}

/// Create a new project: what a modeler starts from, and what a host copies a
/// project into.
///
/// The project is named `name` (NULL for no name) and holds one model, `main`,
/// with an empty stock-and-flow view -- the editing entry points refuse a
/// model that has none -- simulated from time 0 to 100 with a time step of 1
/// by Euler's method. It is exactly the empty project the server creates for a
/// new model (`src/server/project-creation.ts`, opened from its JSON), so
/// every host starts from the same project.
///
/// Nothing is compiled or synced until a query needs it, so a host copies a
/// project with `simlin_project_replace_contents(simlin_project_new(..), src)`
/// for the price of the handle: the copy shares `src`'s datamodel.
///
/// Returns NULL and populates `out_error` when `name` is not valid UTF-8.
///
/// # Safety
/// - `name` must be NULL or a valid NUL-terminated C string
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_new(
    name: *const c_char,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);
    let name = if name.is_null() {
        String::new()
    } else {
        match CStr::from_ptr(name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("project name is not valid UTF-8"),
                );
                return ptr::null_mut();
            }
        }
    };
    Box::into_raw(Box::new(SimlinProject::new(new_datamodel(name))))
}

/// The datamodel `simlin_project_new` creates.
fn new_datamodel(name: String) -> engine::datamodel::Project {
    use engine::datamodel;
    datamodel::Project {
        name,
        sim_specs: datamodel::SimSpecs {
            start: 0.0,
            stop: 100.0,
            dt: datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        },
        dimensions: vec![],
        units: vec![],
        models: vec![datamodel::Model {
            name: "main".to_string(),
            sim_specs: None,
            variables: vec![].into(),
            views: vec![datamodel::View::StockFlow(datamodel::StockFlow {
                name: None,
                elements: vec![].into(),
                view_box: datamodel::Rect::default(),
                zoom: 1.0,
                use_lettered_polarity: false,
                font: None,
                sketch_compat: None,
            })],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

/// Increment the reference count of a project
///
/// Call this when you want to share a project handle with another component
/// that will independently manage its lifetime.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_ref(project: *mut SimlinProject) {
    crate::project_ref(project);
}

/// Decrement the reference count and free the project if it reaches zero
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_unref(project: *mut SimlinProject) {
    crate::project_unref(project);
}

/// Gets the number of models in the project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_model_count(
    project: *mut SimlinProject,
    out_count: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_count.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_count pointer must not be NULL"),
        );
        return;
    }

    let project_ref = ffi_try!(out_error, require_project(project));
    let datamodel_locked = project_ref.datamodel.lock().unwrap();
    *out_count = datamodel_locked.models.len();
}

/// Gets the project's revision: a counter every change to its contents
/// advances (a committed patch, a view edit, a replace, an added model, a
/// diagram sync), and no read does. Two calls that return the same revision
/// saw the same contents, so a host that caches anything derived from the
/// project -- an agent's last read of it, a chart of its last run -- can tell
/// whether that cache still describes it. The converse does not hold: a
/// revision may advance without a visible change.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_revision(
    project: *mut SimlinProject,
    out_revision: *mut u64,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_revision.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_revision pointer must not be NULL"),
        );
        return;
    }

    let project_ref = ffi_try!(out_error, require_project(project));
    let datamodel_locked = project_ref.datamodel.lock().unwrap();
    *out_revision = datamodel_locked.revision();
}

/// Gets the list of model names in the project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_model_names(
    project: *mut SimlinProject,
    result: *mut *mut c_char,
    max: usize,
    out_written: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_written.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_written pointer must not be NULL"),
        );
        return;
    }

    let proj = ffi_try!(out_error, require_project(project));
    let datamodel_locked = proj.datamodel.lock().unwrap();
    let models = &datamodel_locked.models;

    if max == 0 {
        *out_written = models.len();
        return;
    }

    if result.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("result pointer must not be NULL when max > 0"),
        );
        return;
    }

    let count = models.len().min(max);
    let mut allocated: Vec<*mut c_char> = Vec::with_capacity(count);

    for (i, model) in models.iter().take(count).enumerate() {
        let c_string = match CString::new(model.name.clone()) {
            Ok(s) => s,
            Err(_) => {
                for ptr in allocated {
                    drop_c_string(ptr);
                }
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "model name contains interior NUL byte and cannot be converted",
                    ),
                );
                return;
            }
        };
        let raw = c_string.into_raw();
        allocated.push(raw);
        *result.add(i) = raw;
    }

    *out_written = count;
}

/// Adds a new model to a project
///
/// Creates a new empty model with the given name and adds it to the project.
/// The model will have no variables initially.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `modelName` must be a valid C string
///
/// # Returns
/// - 0 on success
/// - SimlinErrorCode::Generic if project or modelName is null or empty
/// - SimlinErrorCode::DuplicateVariable if a model with that name already
///   exists; names are compared as the engine knows them, so a name differing
///   from an existing one only by case, spaces or underscores is taken, and
///   `main` is taken by an unnamed model
/// - SimlinErrorCode::BadModelName if the name begins with the stdlib's
///   prefix (`stdlib⁚`), which names the stdlib's own models
///
/// A refused add changes nothing, the revision included.
#[no_mangle]
pub unsafe extern "C" fn simlin_project_add_model(
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

    let mut datamodel_locked = proj.datamodel.lock().unwrap();

    // The engine's `AddModel` is the one statement of which names are taken
    // (a name whose canonical form is another model's is) and of what a new
    // model holds.
    let add = engine::ProjectPatch {
        project_ops: vec![engine::ProjectOperation::AddModel {
            name: model_name_str.to_string(),
        }],
        models: vec![],
    };
    // Staged on a copy, as `simlin_project_apply_patch` does: a mutable borrow
    // of the contents advances the revision and drops the hit indexes, so a
    // refused add must not take one.
    let mut staged = (*datamodel_locked.shared()).clone();
    if let Err(err) = engine::apply_patch(&mut staged, add) {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::from(err.code)).with_message(err.to_string()),
        );
        return;
    }
    datamodel_locked.replace(std::sync::Arc::new(staged));

    // Re-sync the persistent salsa DB incrementally, when one has been built
    // (one built later is built from this datamodel). The db owns its sync
    // state, so `db.sync` reuses the prior handles automatically; holding the
    // db lock across the call keeps concurrent readers (simlin_sim_new) from
    // observing a half-synced db.
    if let Some(mut db) = proj.built_db() {
        db.sync(&datamodel_locked);
    }

    drop(datamodel_locked);
}

/// Gets a model from a project by name
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `modelName` may be null (uses default model)
/// - The returned model must be freed with simlin_model_unref
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_model(
    project: *mut SimlinProject,
    model_name: *const c_char,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinModel {
    clear_out_error(out_error);
    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };

    let datamodel_locked = proj.datamodel.lock().unwrap();

    if datamodel_locked.models.is_empty() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DoesNotExist)
                .with_message("project does not contain any models"),
        );
        return ptr::null_mut();
    }

    let requested_name = if model_name.is_null() {
        None
    } else {
        match CStr::from_ptr(model_name).to_str() {
            Ok(s) if !s.is_empty() => Some(s.to_string()),
            Ok(_) => None,
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("model name is not valid UTF-8"),
                );
                return ptr::null_mut();
            }
        }
    };

    let resolved_name = match requested_name {
        None => datamodel_locked.models[0].name.clone(),
        Some(ref name) => match crate::model::find_model_in_datamodel(&datamodel_locked, name) {
            Some(m) => m.name.clone(),
            None => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::BadModelName)
                        .with_message(format!("model '{}' not found", name)),
                );
                return ptr::null_mut();
            }
        },
    };

    simlin_project_ref(project);
    drop(datamodel_locked);

    let model = SimlinModel {
        project,
        model_name: std::sync::Arc::new(resolved_name),
        ref_count: AtomicUsize::new(1),
    };

    Box::into_raw(Box::new(model))
}

/// Replace the contents of `dst` with the contents of `src`.
///
/// `dst` shares `src`'s `datamodel::Project` rather than copying it: the two
/// hold one datamodel until either is edited, and the edit copies it first, so
/// neither ever sees the other's changes. A copy of a project, such as a host
/// keeps for each undo step, therefore costs a reference count, and a project
/// that is only copied, read or written never builds a salsa database. When
/// `dst` has one, it is re-synced incrementally, so unchanged variables keep
/// their cached compile fragments. `src` is only read (its refcount is not
/// touched) and may be freed immediately afterwards.
///
/// This is the in-place reload primitive: a caller that reloads a project
/// from disk opens the new bytes with the matching `simlin_project_open_*`
/// function into a scratch project and replaces the live project's contents
/// from it, instead of building a new `SimlinProject` -- so it composes with
/// every format the open functions support and never needs a per-format
/// variant.
///
/// # Effect on live handles
///
/// A `SimlinModel` holds a pointer to its `SimlinProject` plus a model NAME,
/// not a copy of the model, so:
///
/// - Every existing `SimlinModel` handle on `dst` stays valid and observes
///   the NEW contents on its next call (variables, equations, sim specs,
///   diagnostics via `simlin_project_get_errors`, a fresh `simlin_sim_new`).
/// - A handle whose model name is absent from the replacement is not
///   invalidated: queries through it return `BadModelName`, and a sim
///   created through it fails on its first run with `NotSimulatable` naming
///   the model (`simlin_sim_new` defers compile failures to the run, per its
///   own contract) -- until a model of that name exists again, at which point
///   the same handle works once more.
/// - A `SimlinSim` created BEFORE the replace is a stale snapshot for the
///   simulation entry points (`simlin_sim_*`): it was compiled from the old
///   contents and keeps its results and its ability to `reset`/re-run against
///   that compiled program. The sim-bearing ANALYSIS entry points
///   (`simlin_analyze_get_loops_runtime`, `simlin_analyze_get_links`, ...)
///   are not snapshots: they enumerate loops/links from the project's CURRENT
///   contents and read scores out of the stale sim's results by position, so
///   after a replace they mix old results with the new model. Callers should
///   create and run a new sim after a replace before analyzing.
///
/// # Locking
///
/// `src`'s datamodel lock is taken alone, just long enough to share its
/// datamodel, and released BEFORE `dst`'s locks are acquired -- so two threads
/// replacing in opposite directions cannot deadlock, and `dst == src` does not
/// self-deadlock. Replacing `dst`'s contents with the datamodel it already
/// holds -- `dst == src`, or a copy that still shares it -- changes nothing:
/// the revision stays, and the db is not touched. `dst`'s datamodel and db
/// locks are then held together, in the datamodel-then-db order used
/// project-wide, across both the db re-sync and the datamodel swap, so no
/// concurrent reader (`simlin_sim_new`, `simlin_project_get_errors`,
/// `simlin_project_apply_patch`) observes the datamodel and the db
/// disagreeing.
///
/// # Safety
/// - `dst` must be a valid pointer to a SimlinProject
/// - `src` must be a valid pointer to a SimlinProject
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_replace_contents(
    dst: *mut SimlinProject,
    src: *const SimlinProject,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    let dst_ref = ffi_try!(out_error, require_project(dst));
    if src.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("source project pointer must not be NULL"),
        );
        return;
    }
    let src_ref = &*src;

    // Share under src's lock alone, then release it before touching dst (see
    // the locking notes above).
    let new_datamodel = src_ref.datamodel.lock().unwrap().shared();

    let mut datamodel_locked = dst_ref.datamodel.lock().unwrap();
    // The datamodel `dst` holds already (`dst == src`, or a copy that still
    // shares it) is no change: nothing to sync, and nothing to wait for the
    // db for.
    if datamodel_locked.holds(&new_datamodel) {
        return;
    }
    let mut db_locked = dst_ref.built_db();
    if let Some(db) = &mut db_locked {
        db.sync(&new_datamodel);
    }
    datamodel_locked.replace(new_datamodel);
}

/// Open a project from XMILE/STMX format data
///
/// Parses and imports a system dynamics model from XMILE format, the industry
/// standard interchange format for system dynamics models. Also supports the
/// STMX variant used by Stella. The reader does not keep everything a file can
/// hold; `simlin_import_losses` reports what it leaves out of these bytes.
///
/// Returns NULL and populates `out_error` on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_xmile(
    data: *const u8,
    len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);
    if data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL"),
        );
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);
    let mut reader = BufReader::new(slice);

    match simlin_engine::open_xmile(&mut reader) {
        Ok(datamodel_project) => Box::into_raw(Box::new(SimlinProject::new(datamodel_project))),
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("failed to import XMILE: {err}")),
            );
            ptr::null_mut()
        }
    }
}

/// Open a project from Vensim MDL format data
///
/// Parses and imports a system dynamics model from Vensim's MDL format. The
/// reader does not keep everything a file can hold; `simlin_import_losses`
/// reports what it leaves out of these bytes.
///
/// Returns NULL and populates `out_error` on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_vensim(
    data: *const u8,
    len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);
    if data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL"),
        );
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);
    let contents = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("MDL data is not valid UTF-8"),
            );
            return ptr::null_mut();
        }
    };

    match simlin_engine::open_vensim(contents) {
        Ok(datamodel_project) => Box::into_raw(Box::new(SimlinProject::new(datamodel_project))),
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("failed to import MDL: {err}")),
            );
            ptr::null_mut()
        }
    }
}

/// Report what a file holds that a project opened from it does not keep
///
/// The MDL and XMILE readers do not keep everything a file can hold: a
/// Vensim sketch's comments, graphs, sliders and images, and the custom
/// graphs, tables and reports the file defines; an XMILE view's objects
/// besides its diagram (a graph, a text box), interface pages, story mode, a
/// standalone graphical function. So a save of the project, over the file or
/// in any other format, leaves those out, and a host that saves needs to say
/// so when the file opens. The report is of the file, whichever function
/// opens it (`simlin_project_open_xmile`, `simlin_project_open_vensim`,
/// `simlin_project_open_vensim_with_data`), so it is one function beside the
/// opens rather than a variant of each.
///
/// What it covers is what the engine's readers report
/// (`simlin_engine::ImportWarning`): for MDL the sketch and the file's custom
/// outputs, for XMILE what the reader skips among the variables, the views
/// and the stories. A NULL report says the file loses none of those, not
/// that the project holds everything the file says.
///
/// `format` is the file's, as `SimlinSaveFormat` numbers them: `Mdl` (0) or
/// `Xmile` (1). The engine reports on no other format, and another is
/// refused with `Generic` rather than answered as if it lost nothing.
///
/// `data_dir` (NULL for none; read only with the `file_io` feature) is the
/// directory `simlin_project_open_vensim_with_data` is given. The MDL report
/// is made as the file is converted, and the conversion fails on a GET DIRECT
/// reference it cannot resolve, so a file with such references is reported
/// on only with its data, exactly as it opens only with it. An XMILE report
/// reads no data.
///
/// Each kind of loss, in each place it occurs, is one `Warning`-severity,
/// wire-`Generic`, kind-`Model` detail on the aggregate `SimlinError` stored
/// in `out_collected_errors` (NULL when the file loses nothing): for MDL, one
/// per kind and sketch view for what a modeler put on a view, and one per
/// kind over the whole sketch for what follows from what the diagram does
/// not draw (see `simlin_engine::mdl::parse_mdl_with_warnings`). `message` is
/// `"MDL import: <reason>"` or `"XMILE import: <reason>"` and `details` the
/// bare reason, such as `29 comments on view 'View 1' are not kept, such as
/// 'The World3 Model'` or `2 sliders on interface page 1 are not kept:
/// 'Birth Rate' and 'Population'`. The aggregate's own message counts the
/// losses by kind over the whole file, for a host to show where a row per
/// place would be too many: `29 comments, 3 graphs, and 7 sliders in this
/// file are not kept`. The report describes the file as it was read, and no
/// project keeps it, so a host that shows it holds it itself.
///
/// A file that does not read populates `out_error`, as the open of it does,
/// with `out_collected_errors` NULL.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `data_dir` may be null; when non-null it must point to `data_dir_len`
///   bytes of valid UTF-8 representing a directory path
/// - `out_collected_errors` must be a valid pointer
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_import_losses(
    format: u32,
    data: *const u8,
    len: usize,
    data_dir: *const u8,
    data_dir_len: usize,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_collected_errors.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_collected_errors pointer must not be NULL"),
        );
        return;
    }
    *out_collected_errors = ptr::null_mut();
    if data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL"),
        );
        return;
    }
    let slice = std::slice::from_raw_parts(data, len);
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

    let (source, read) = match ffi::SimlinSaveFormat::try_from(format) {
        Ok(ffi::SimlinSaveFormat::Mdl) => {
            let Ok(contents) = std::str::from_utf8(slice) else {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("MDL data is not valid UTF-8"),
                );
                return;
            };
            #[cfg(feature = "file_io")]
            let provider = data_dir.map(simlin_engine::FilesystemDataProvider::new);
            #[cfg(feature = "file_io")]
            let provider = provider
                .as_ref()
                .map(|provider| provider as &dyn simlin_engine::DataProvider);
            #[cfg(not(feature = "file_io"))]
            let provider = {
                let _ = data_dir;
                None
            };
            (
                "MDL",
                simlin_engine::open_vensim_with_data_and_warnings(contents, provider)
                    .map(|(_, warnings)| warnings),
            )
        }
        Ok(ffi::SimlinSaveFormat::Xmile) => (
            "XMILE",
            simlin_engine::open_xmile_with_warnings(&mut BufReader::new(slice))
                .map(|(_, warnings)| warnings),
        ),
        Ok(
            ffi::SimlinSaveFormat::Json
            | ffi::SimlinSaveFormat::JsonSdai
            | ffi::SimlinSaveFormat::Protobuf,
        )
        | Err(()) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                    "no report of what an import does not keep for format {format}: \
                     the formats reported on are MDL (0) and XMILE (1)"
                )),
            );
            return;
        }
    };
    match read {
        Ok(warnings) => {
            let summary = simlin_engine::ImportWarning::summary(&warnings);
            store_warnings(
                out_collected_errors,
                &format!("{source} import"),
                warnings.into_iter().map(|w| w.message),
                summary,
            );
        }
        Err(err) => store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::from(err.code))
                .with_message(format!("failed to import {source}: {err}")),
        ),
    }
}

/// Open a Vensim MDL model with external data file support.
///
/// When `data_dir` is non-null and the `file_io` feature is enabled, a
/// `FilesystemDataProvider` is created using that directory as the base path
/// for resolving relative data file references. When `data_dir` is null,
/// a `NullDataProvider` is used (any GET DIRECT DATA references will error).
///
/// Returns NULL and populates `out_error` on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes of UTF-8 MDL text
/// - `data_dir` may be null; when non-null it must point to `data_dir_len` bytes
///   of valid UTF-8 representing a directory path
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_vensim_with_data(
    data: *const u8,
    len: usize,
    data_dir: *const u8,
    data_dir_len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);
    if data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL"),
        );
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);
    let contents = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("MDL data is not valid UTF-8"),
            );
            return ptr::null_mut();
        }
    };

    let result = if data_dir.is_null() {
        simlin_engine::open_vensim(contents)
    } else {
        let dir_slice = std::slice::from_raw_parts(data_dir, data_dir_len);
        let dir_str = match std::str::from_utf8(dir_slice) {
            Ok(s) => s,
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("data_dir is not valid UTF-8"),
                );
                return ptr::null_mut();
            }
        };

        #[cfg(feature = "file_io")]
        {
            let provider = simlin_engine::FilesystemDataProvider::new(dir_str);
            simlin_engine::open_vensim_with_data(contents, Some(&provider))
        }
        #[cfg(not(feature = "file_io"))]
        {
            let _ = dir_str;
            simlin_engine::open_vensim(contents)
        }
    };

    match result {
        Ok(datamodel_project) => Box::into_raw(Box::new(SimlinProject::new(datamodel_project))),
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("failed to import MDL: {err}")),
            );
            ptr::null_mut()
        }
    }
}

/// Open a project from systems format data
///
/// Parses and translates a system dynamics model from the systems format
/// (`.txt` line-oriented notation). Returns NULL and populates `out_error`
/// on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
/// - The returned project must be freed with `simlin_project_unref`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open_systems(
    data: *const u8,
    len: usize,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinProject {
    clear_out_error(out_error);
    if data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("data pointer must not be NULL"),
        );
        return ptr::null_mut();
    }

    let slice = std::slice::from_raw_parts(data, len);
    let contents = match std::str::from_utf8(slice) {
        Ok(s) => s,
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("systems format data is not valid UTF-8"),
            );
            return ptr::null_mut();
        }
    };

    match simlin_engine::open_systems(contents) {
        Ok(datamodel_project) => Box::into_raw(Box::new(SimlinProject::new(datamodel_project))),
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::from(err.code))
                    .with_message(format!("failed to import systems format: {err}")),
            );
            ptr::null_mut()
        }
    }
}

/// Check if a project's model can be simulated
///
/// Returns true if the model can be simulated (i.e., can be compiled to a VM
/// without errors), false otherwise. This is a quick check for the UI to determine
/// if the "Run Simulation" button should be enabled.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `model_name` may be null (defaults to "main") or must be a valid UTF-8 C string
#[no_mangle]
pub unsafe extern "C" fn simlin_project_is_simulatable(
    project: *mut SimlinProject,
    model_name: *const c_char,
    out_error: *mut *mut SimlinError,
) -> bool {
    clear_out_error(out_error);
    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return false;
        }
    };

    let model_name = if model_name.is_null() {
        "main"
    } else {
        match CStr::from_ptr(model_name).to_str() {
            Ok(s) => s,
            Err(_) => {
                store_anyhow_error(out_error, anyhow!("invalid UTF-8 in model_name"));
                return false;
            }
        }
    };

    // `build_sim` reads the contents as well as the db, to route a
    // conveyor/queue model through its special expansion build path rather
    // than trip the `Conveyor/QueueNotExpanded` guard on the ordinary one.
    let (contents, mut db_locked) = proj.lock_contents_and_db();
    let Some(source_project) = db_locked.current_source_project() else {
        return false;
    };
    engine::build_sim(
        &mut db_locked,
        source_project,
        &contents,
        model_name,
        engine::db::LtmOverlay::Off,
    )
    .is_ok()
}

/// Get all errors in a project including static analysis and compilation errors
///
/// Returns NULL if no errors exist in the project. This function collects all
/// static errors (equation parsing, unit checking, etc.) and also attempts to
/// compile the "main" model to find any compilation-time errors.
///
/// The caller must free the returned error object using `simlin_error_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - The returned pointer must be freed with `simlin_error_free`
#[no_mangle]
pub unsafe extern "C" fn simlin_project_get_errors(
    project: *mut SimlinProject,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinError {
    clear_out_error(out_error);
    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };

    let (contents, mut db_locked) = proj.lock_contents_and_db();
    let source_project = match db_locked.current_source_project() {
        Some(sp) => sp,
        None => return ptr::null_mut(),
    };

    // Compilability is an INTRINSIC property of the project, assessed with LTM
    // OFF. LTM is an analysis overlay, not part of whether the model is a
    // valid, runnable simulation -- so the compile/VM-validation channel must
    // never be computed under the overlay, or an LTM-only failure (a synthetic
    // fragment the compiler refuses, say) would masquerade as a project error
    // on a model that simulates fine. `build_sim`
    // additionally routes a conveyor/queue model through its special expansion
    // build path (also LTM-off), so a valid special-stock model is not
    // mis-reported as a project error by the ordinary path's NotExpanded guard.
    let vm_error = engine::build_sim(
        &mut db_locked,
        source_project,
        &contents,
        "main",
        engine::db::LtmOverlay::Off,
    )
    .err();

    // The LTM *diagnostics* (auto-flip-to-discovery advisory, synthetic-fragment
    // compile failures, GH #311 partial-equation warnings) accumulate via
    // `model_all_diagnostics` -> `model_ltm_variables`, independent of whether
    // the LTM-enabled assembly would succeed -- so harvesting them only needs
    // the `collect_all_diagnostics` pass asked for under the overlay, NOT a
    // recompile that feeds `vm_error`. If any simulation on this project
    // requested LTM, collect the overlay's diagnostics (GH #466); the overlay
    // is an argument, so this neither disturbs the plain variant nor
    // re-verifies the database. A project that never requested LTM pays no
    // LTM synthesis cost.
    let ltm_requested = proj.ltm_requested.load(Ordering::Acquire);
    let all_errors = gather_error_details_with_db(
        &db_locked,
        source_project,
        vm_error.as_ref(),
        &contents,
        engine::db::LtmOverlay::from(ltm_requested),
    );

    if all_errors.is_empty() {
        return ptr::null_mut();
    }

    let code = all_errors
        .first()
        .map(|detail| detail.code)
        .unwrap_or(SimlinErrorCode::NoError);
    build_simlin_error(code, &all_errors).into_raw()
}
