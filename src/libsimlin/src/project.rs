// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Project lifecycle FFI functions.
//!
//! Opening projects from various formats (protobuf, JSON, XMILE, Vensim),
//! reference counting, querying models, and checking simulatability.

use anyhow::{anyhow, Result};
use prost::Message;
use simlin_engine::{self as engine, serde as engine_serde, Vm};
use std::ffi::{CStr, CString};
use std::io::BufReader;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

use crate::ffi;
use crate::ffi_error::{FfiError, SimlinError};
use crate::ffi_try;
use crate::patch::gather_error_details;
use crate::{
    build_simlin_error, clear_out_error, compile_simulation, drop_c_string, require_project,
    store_anyhow_error, store_error, SimlinErrorCode, SimlinModel, SimlinProject,
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
        let pb_project = engine::project_io::Project::decode(slice).map_err(|decode_err| {
            FfiError::new(SimlinErrorCode::ProtobufDecode)
                .with_message(format!("failed to decode project protobuf: {decode_err}"))
        })?;

        let project: engine::Project = engine_serde::deserialize(pb_project).into();
        Ok(Box::into_raw(Box::new(SimlinProject {
            project: Mutex::new(project),
            ref_count: AtomicUsize::new(1),
        })))
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

        let project: engine::Project = datamodel_project.into();
        Ok(Box::into_raw(Box::new(SimlinProject {
            project: Mutex::new(project),
            ref_count: AtomicUsize::new(1),
        })))
    })();

    match result {
        Ok(ptr) => ptr,
        Err(err) => {
            store_anyhow_error(out_error, err);
            ptr::null_mut()
        }
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
    let project_locked = project_ref.project.lock().unwrap();
    *out_count = project_locked.datamodel.models.len();
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
    let project_locked = proj.project.lock().unwrap();
    let models = &project_locked.datamodel.models;

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
/// - SimlinErrorCode::DuplicateVariable if a model with that name already exists
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

    let mut project_locked = proj.project.lock().unwrap();

    if project_locked
        .datamodel
        .models
        .iter()
        .any(|model| model.name == model_name_str)
    {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DuplicateVariable)
                .with_message(format!("model '{}' already exists", model_name_str)),
        );
        return;
    }

    // Create new empty model
    let new_model = engine::datamodel::Model {
        name: model_name_str.to_string(),
        sim_specs: None,
        variables: vec![],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
    };

    // Add to datamodel
    project_locked.datamodel.models.push(new_model);

    // Rebuild the project's internal structures
    *project_locked = engine::Project::from(project_locked.datamodel.clone());
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

    let project_locked = proj.project.lock().unwrap();

    if project_locked.datamodel.models.is_empty() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DoesNotExist)
                .with_message("project does not contain any models"),
        );
        return ptr::null_mut();
    }

    let mut requested_name = if model_name.is_null() {
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

    if requested_name
        .as_deref()
        .and_then(|name| project_locked.datamodel.get_model(name))
        .is_none()
    {
        requested_name = Some(project_locked.datamodel.models[0].name.clone());
    }

    simlin_project_ref(project);
    drop(project_locked);

    let model = SimlinModel {
        project,
        model_name: std::sync::Arc::new(requested_name.unwrap()),
        ref_count: AtomicUsize::new(1),
    };

    Box::into_raw(Box::new(model))
}

/// Open a project from XMILE/STMX format data
///
/// Parses and imports a system dynamics model from XMILE format, the industry
/// standard interchange format for system dynamics models. Also supports the
/// STMX variant used by Stella.
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
        Ok(datamodel_project) => {
            let project: engine::Project = datamodel_project.into();
            let boxed = Box::new(SimlinProject {
                project: Mutex::new(project),
                ref_count: AtomicUsize::new(1),
            });
            Box::into_raw(boxed)
        }
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
/// Parses and imports a system dynamics model from Vensim's MDL format.
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
        Ok(datamodel_project) => {
            let project: engine::Project = datamodel_project.into();
            let boxed = Box::new(SimlinProject {
                project: Mutex::new(project),
                ref_count: AtomicUsize::new(1),
            });
            Box::into_raw(boxed)
        }
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

    let project_locked = proj.project.lock().unwrap();
    compile_simulation(&project_locked, model_name)
        .and_then(Vm::new)
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

    let project_locked = proj.project.lock().unwrap();
    let (all_errors, _) = gather_error_details(&project_locked);

    if all_errors.is_empty() {
        return ptr::null_mut();
    }

    let code = all_errors
        .first()
        .map(|detail| detail.code)
        .unwrap_or(SimlinErrorCode::NoError);
    build_simlin_error(code, &all_errors).into_raw()
}
