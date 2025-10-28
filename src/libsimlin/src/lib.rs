// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.
use anyhow::{Error as AnyError, Result};
use prost::Message;
use serde::Deserialize;
use simlin_engine::common::ErrorCode;
use simlin_engine::ltm::{detect_loops, LoopPolarity};
use simlin_engine::{self as engine, canonicalize, serde as engine_serde, Vm};
use std::alloc::{alloc, dealloc, Layout};
use std::ffi::{CStr, CString};
use std::io::BufReader;
use std::os::raw::{c_char, c_double};
use std::ptr;
use std::str::FromStr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub mod errors;
mod ffi;
mod ffi_error;
pub use ffi::{
    SimlinLink, SimlinLinkPolarity, SimlinLinks, SimlinLoop, SimlinLoopPolarity, SimlinLoops,
};
pub use ffi_error::{ErrorDetail as ErrorDetailData, FfiError, SimlinError};

/// Error codes for the C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinErrorCode {
    /// Success - no error
    NoError = 0,
    DoesNotExist = 1,
    XmlDeserialization = 2,
    VensimConversion = 3,
    ProtobufDecode = 4,
    InvalidToken = 5,
    UnrecognizedEof = 6,
    UnrecognizedToken = 7,
    ExtraToken = 8,
    UnclosedComment = 9,
    UnclosedQuotedIdent = 10,
    ExpectedNumber = 11,
    UnknownBuiltin = 12,
    BadBuiltinArgs = 13,
    EmptyEquation = 14,
    BadModuleInputDst = 15,
    BadModuleInputSrc = 16,
    NotSimulatable = 17,
    BadTable = 18,
    BadSimSpecs = 19,
    NoAbsoluteReferences = 20,
    CircularDependency = 21,
    ArraysNotImplemented = 22,
    MultiDimensionalArraysNotImplemented = 23,
    BadDimensionName = 24,
    BadModelName = 25,
    MismatchedDimensions = 26,
    ArrayReferenceNeedsExplicitSubscripts = 27,
    DuplicateVariable = 28,
    UnknownDependency = 29,
    VariablesHaveErrors = 30,
    UnitDefinitionErrors = 31,
    Generic = 32,
}

/// Error detail structure containing contextual information for failures.
#[repr(C)]
pub struct SimlinErrorDetail {
    pub code: SimlinErrorCode,
    pub message: *const c_char,
    pub model_name: *const c_char,
    pub variable_name: *const c_char,
    pub start_offset: u16,
    pub end_offset: u16,
}

impl From<engine::ErrorCode> for SimlinErrorCode {
    fn from(code: engine::ErrorCode) -> Self {
        match code {
            engine::ErrorCode::NoError => SimlinErrorCode::NoError,
            engine::ErrorCode::DoesNotExist => SimlinErrorCode::DoesNotExist,
            engine::ErrorCode::XmlDeserialization => SimlinErrorCode::XmlDeserialization,
            engine::ErrorCode::VensimConversion => SimlinErrorCode::VensimConversion,
            engine::ErrorCode::ProtobufDecode => SimlinErrorCode::ProtobufDecode,
            engine::ErrorCode::InvalidToken => SimlinErrorCode::InvalidToken,
            engine::ErrorCode::UnrecognizedEof => SimlinErrorCode::UnrecognizedEof,
            engine::ErrorCode::UnrecognizedToken => SimlinErrorCode::UnrecognizedToken,
            engine::ErrorCode::ExtraToken => SimlinErrorCode::ExtraToken,
            engine::ErrorCode::UnclosedComment => SimlinErrorCode::UnclosedComment,
            engine::ErrorCode::UnclosedQuotedIdent => SimlinErrorCode::UnclosedQuotedIdent,
            engine::ErrorCode::ExpectedNumber => SimlinErrorCode::ExpectedNumber,
            engine::ErrorCode::UnknownBuiltin => SimlinErrorCode::UnknownBuiltin,
            engine::ErrorCode::BadBuiltinArgs => SimlinErrorCode::BadBuiltinArgs,
            engine::ErrorCode::EmptyEquation => SimlinErrorCode::EmptyEquation,
            engine::ErrorCode::BadModuleInputDst => SimlinErrorCode::BadModuleInputDst,
            engine::ErrorCode::BadModuleInputSrc => SimlinErrorCode::BadModuleInputSrc,
            engine::ErrorCode::NotSimulatable => SimlinErrorCode::NotSimulatable,
            engine::ErrorCode::BadTable => SimlinErrorCode::BadTable,
            engine::ErrorCode::BadSimSpecs => SimlinErrorCode::BadSimSpecs,
            engine::ErrorCode::NoAbsoluteReferences => SimlinErrorCode::NoAbsoluteReferences,
            engine::ErrorCode::CircularDependency => SimlinErrorCode::CircularDependency,
            engine::ErrorCode::ArraysNotImplemented => SimlinErrorCode::ArraysNotImplemented,
            engine::ErrorCode::MultiDimensionalArraysNotImplemented => {
                SimlinErrorCode::MultiDimensionalArraysNotImplemented
            }
            engine::ErrorCode::BadDimensionName => SimlinErrorCode::BadDimensionName,
            engine::ErrorCode::BadModelName => SimlinErrorCode::BadModelName,
            engine::ErrorCode::MismatchedDimensions => SimlinErrorCode::MismatchedDimensions,
            engine::ErrorCode::ArrayReferenceNeedsExplicitSubscripts => {
                SimlinErrorCode::ArrayReferenceNeedsExplicitSubscripts
            }
            engine::ErrorCode::DuplicateVariable => SimlinErrorCode::DuplicateVariable,
            engine::ErrorCode::UnknownDependency => SimlinErrorCode::UnknownDependency,
            engine::ErrorCode::VariablesHaveErrors => SimlinErrorCode::VariablesHaveErrors,
            engine::ErrorCode::UnitDefinitionErrors => SimlinErrorCode::UnitDefinitionErrors,
            engine::ErrorCode::Generic => SimlinErrorCode::Generic,
            engine::ErrorCode::NoAppInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoSubscriptInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoIfInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoUnaryOpInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::BadBinaryOpInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::NoConstInUnits => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedInteger => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedIntegerOne => SimlinErrorCode::Generic,
            engine::ErrorCode::DuplicateUnit => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedModule => SimlinErrorCode::Generic,
            engine::ErrorCode::ExpectedIdent => SimlinErrorCode::Generic,
            engine::ErrorCode::UnitMismatch => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoWildcard => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoStarRange => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoRange => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoArrayBuiltin => SimlinErrorCode::Generic,
            engine::ErrorCode::CantSubscriptScalar => SimlinErrorCode::Generic,
            engine::ErrorCode::DimensionInScalarContext => SimlinErrorCode::Generic,
        }
    }
}
/// Opaque project structure
pub struct SimlinProject {
    project: Mutex<engine::Project>,
    ref_count: AtomicUsize,
}

/// Opaque model structure
pub struct SimlinModel {
    project: *const SimlinProject,
    model_name: Arc<String>,
    ref_count: AtomicUsize,
}

/// Internal state for SimlinSim
struct SimState {
    vm: Option<Vm>,
    results: Option<engine::Results>,
}

/// Opaque simulation structure
pub struct SimlinSim {
    model: *const SimlinModel,
    enable_ltm: bool,
    state: Mutex<SimState>,
    ref_count: AtomicUsize,
}

fn clear_out_error(out_error: *mut *mut SimlinError) {
    if out_error.is_null() {
        return;
    }
    unsafe {
        *out_error = ptr::null_mut();
    }
}

fn store_error(out_error: *mut *mut SimlinError, error: SimlinError) {
    if out_error.is_null() {
        return;
    }
    unsafe {
        *out_error = error.into_raw();
    }
}

fn store_ffi_error(out_error: *mut *mut SimlinError, error: FfiError) {
    store_error(out_error, error.into_simlin_error());
}

fn error_from_anyhow(err: AnyError) -> SimlinError {
    if let Some(ffi_error) = err
        .chain()
        .find_map(|cause| cause.downcast_ref::<FfiError>())
    {
        return ffi_error.clone().into_simlin_error();
    }

    let mut error = SimlinError::new(SimlinErrorCode::Generic);
    error.set_message(Some(err.to_string()));
    error
}

fn store_anyhow_error(out_error: *mut *mut SimlinError, err: AnyError) {
    store_error(out_error, error_from_anyhow(err));
}

fn build_simlin_error(code: SimlinErrorCode, details: &[ErrorDetailData]) -> SimlinError {
    let mut error = SimlinError::new(code);
    error.extend_details(details.iter().cloned());
    error
}

macro_rules! ffi_try {
    ($out_error:expr, $expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => {
                store_anyhow_error($out_error, err);
                return;
            }
        }
    };
}

unsafe fn require_project<'a>(project: *mut SimlinProject) -> Result<&'a mut SimlinProject> {
    if project.is_null() {
        Err(FfiError::new(SimlinErrorCode::Generic)
            .with_message("project pointer must not be NULL")
            .into())
    } else {
        Ok(&mut *project)
    }
}

unsafe fn require_model<'a>(model: *mut SimlinModel) -> Result<&'a mut SimlinModel> {
    if model.is_null() {
        Err(FfiError::new(SimlinErrorCode::Generic)
            .with_message("model pointer must not be NULL")
            .into())
    } else {
        Ok(&mut *model)
    }
}

unsafe fn require_sim<'a>(sim: *mut SimlinSim) -> Result<&'a mut SimlinSim> {
    if sim.is_null() {
        Err(FfiError::new(SimlinErrorCode::Generic)
            .with_message("simulation pointer must not be NULL")
            .into())
    } else {
        Ok(&mut *sim)
    }
}

fn ffi_error_from_engine(error: &engine::Error) -> FfiError {
    FfiError::new(SimlinErrorCode::from(error.code)).with_message(error.to_string())
}
/// simlin_error_str returns a string representation of an error code.
/// The returned string must not be freed or modified.
#[no_mangle]
pub extern "C" fn simlin_error_str(err: SimlinErrorCode) -> *const c_char {
    let s: &'static str = match err {
        SimlinErrorCode::NoError => "no_error\0",
        SimlinErrorCode::DoesNotExist => "does_not_exist\0",
        SimlinErrorCode::XmlDeserialization => "xml_deserialization\0",
        SimlinErrorCode::VensimConversion => "vensim_conversion\0",
        SimlinErrorCode::ProtobufDecode => "protobuf_decode\0",
        SimlinErrorCode::InvalidToken => "invalid_token\0",
        SimlinErrorCode::UnrecognizedEof => "unrecognized_eof\0",
        SimlinErrorCode::UnrecognizedToken => "unrecognized_token\0",
        SimlinErrorCode::ExtraToken => "extra_token\0",
        SimlinErrorCode::UnclosedComment => "unclosed_comment\0",
        SimlinErrorCode::UnclosedQuotedIdent => "unclosed_quoted_ident\0",
        SimlinErrorCode::ExpectedNumber => "expected_number\0",
        SimlinErrorCode::UnknownBuiltin => "unknown_builtin\0",
        SimlinErrorCode::BadBuiltinArgs => "bad_builtin_args\0",
        SimlinErrorCode::EmptyEquation => "empty_equation\0",
        SimlinErrorCode::BadModuleInputDst => "bad_module_input_dst\0",
        SimlinErrorCode::BadModuleInputSrc => "bad_module_input_src\0",
        SimlinErrorCode::NotSimulatable => "not_simulatable\0",
        SimlinErrorCode::BadTable => "bad_table\0",
        SimlinErrorCode::BadSimSpecs => "bad_sim_specs\0",
        SimlinErrorCode::NoAbsoluteReferences => "no_absolute_references\0",
        SimlinErrorCode::CircularDependency => "circular_dependency\0",
        SimlinErrorCode::ArraysNotImplemented => "arrays_not_implemented\0",
        SimlinErrorCode::MultiDimensionalArraysNotImplemented => {
            "multi_dimensional_arrays_not_implemented\0"
        }
        SimlinErrorCode::BadDimensionName => "bad_dimension_name\0",
        SimlinErrorCode::BadModelName => "bad_model_name\0",
        SimlinErrorCode::MismatchedDimensions => "mismatched_dimensions\0",
        SimlinErrorCode::ArrayReferenceNeedsExplicitSubscripts => {
            "array_reference_needs_explicit_subscripts\0"
        }
        SimlinErrorCode::DuplicateVariable => "duplicate_variable\0",
        SimlinErrorCode::UnknownDependency => "unknown_dependency\0",
        SimlinErrorCode::VariablesHaveErrors => "variables_have_errors\0",
        SimlinErrorCode::UnitDefinitionErrors => "unit_definition_errors\0",
        SimlinErrorCode::Generic => "generic\0",
    };
    s.as_ptr() as *const c_char
}

/// # Safety
///
/// The pointer must have been created by a simlin function that returns a `*mut SimlinError`,
/// must not be null, and must not have been freed already.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_free(err: *mut SimlinError) {
    if err.is_null() {
        return;
    }
    let _ = SimlinError::from_raw(err);
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_code(err: *const SimlinError) -> SimlinErrorCode {
    if err.is_null() {
        return SimlinErrorCode::Generic;
    }
    (*err).code()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
/// The returned string pointer is valid only as long as the error object is not freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_message(err: *const SimlinError) -> *const c_char {
    if err.is_null() {
        return ptr::null();
    }
    (*err).message_ptr()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_detail_count(err: *const SimlinError) -> usize {
    if err.is_null() {
        return 0;
    }
    (*err).detail_count()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
/// The returned array pointer is valid only as long as the error object is not freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_details(
    err: *const SimlinError,
) -> *const SimlinErrorDetail {
    if err.is_null() {
        return ptr::null();
    }
    (*err).details_ptr()
}

/// # Safety
///
/// The pointer must be either null or a valid `SimlinError` pointer that has not been freed.
/// The returned detail pointer is valid only as long as the error object is not freed.
#[no_mangle]
pub unsafe extern "C" fn simlin_error_get_detail(
    err: *const SimlinError,
    index: usize,
) -> *const SimlinErrorDetail {
    if err.is_null() {
        return ptr::null();
    }
    (*err).detail_at(index)
}
/// simlin_project_open opens a project from protobuf data.
/// Returns NULL and populates `out_error` on failure.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_open(
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

/// simlin_project_json_open opens a project from JSON data.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes of UTF-8 JSON
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_json_open(
    data: *const u8,
    len: usize,
    format: ffi::SimlinJsonFormat,
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
        let json_str = std::str::from_utf8(slice).map_err(|utf8_err| {
            FfiError::new(SimlinErrorCode::Generic)
                .with_message(format!("input JSON is not valid UTF-8: {utf8_err}"))
        })?;

        let datamodel_project: engine::datamodel::Project = match format {
            ffi::SimlinJsonFormat::Native => {
                let json_project: engine::json::Project = engine::json::Project::from_str(json_str)
                    .map_err(|engine_err| {
                        FfiError::new(SimlinErrorCode::Generic).with_message(engine_err.to_string())
                    })?;
                json_project.into()
            }
            ffi::SimlinJsonFormat::Sdai => {
                let sdai_model: engine::json_sdai::SdaiModel =
                    engine::json_sdai::SdaiModel::from_str(json_str).map_err(|engine_err| {
                        FfiError::new(SimlinErrorCode::Generic).with_message(engine_err.to_string())
                    })?;
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
/// Increments the reference count of a project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_ref(project: *mut SimlinProject) {
    if !project.is_null() {
        (*project).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}
/// Decrements the reference count and frees the project if it reaches zero
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
#[no_mangle]
pub unsafe extern "C" fn simlin_project_unref(project: *mut SimlinProject) {
    if project.is_null() {
        return;
    }
    let prev_count = (*project).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let _ = Box::from_raw(project);
    }
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

    for (i, model) in models.iter().take(count).enumerate() {
        let c_string = match CString::new(model.name.clone()) {
            Ok(s) => s,
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "model name contains interior NUL byte and cannot be converted",
                    ),
                );
                return;
            }
        };
        *result.add(i) = c_string.into_raw();
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
/// - `model_name` must be a valid C string
///
/// # Returns
/// - 0 on success
/// - SimlinErrorCode::Generic if project or model_name is null or empty
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
/// - `model_name` may be null (uses default model)
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
        model_name: Arc::new(requested_name.unwrap()),
        ref_count: AtomicUsize::new(1),
    };

    Box::into_raw(Box::new(model))
}

/// Increments the reference count of a model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_ref(model: *mut SimlinModel) {
    if !model.is_null() {
        (*model).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}

/// Decrements the reference count and frees the model if it reaches zero
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_unref(model: *mut SimlinModel) {
    if model.is_null() {
        return;
    }
    let prev_count = (*model).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let model = Box::from_raw(model);
        // Decrement project reference count
        simlin_project_unref(model.project as *mut SimlinProject);
    }
}

/// Gets the number of variables in the model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_count(
    model: *mut SimlinModel,
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

    let model_ref = ffi_try!(out_error, require_model(model));
    let project_locked = (*model_ref.project).project.lock().unwrap();
    let offsets =
        engine::interpreter::calc_flattened_offsets(&project_locked, &model_ref.model_name);
    *out_count = offsets.len();
}

/// Gets the variable names from the model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_names(
    model: *mut SimlinModel,
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

    let model_ref = ffi_try!(out_error, require_model(model));
    let project_locked = (*model_ref.project).project.lock().unwrap();
    let offsets =
        engine::interpreter::calc_flattened_offsets(&project_locked, &model_ref.model_name);

    if max == 0 {
        *out_written = offsets.len();
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

    let mut names: Vec<_> = offsets.keys().collect();
    names.sort();

    let count = names.len().min(max);
    for (i, name) in names.iter().take(count).enumerate() {
        let c_string = match CString::new(name.as_str()) {
            Ok(s) => s,
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "variable name contains interior NUL byte and cannot be converted",
                    ),
                );
                return;
            }
        };
        *result.add(i) = c_string.into_raw();
    }

    *out_written = count;
}

/// Gets the incoming links (dependencies) for a variable
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `var_name` must be a valid C string
/// - `result` must be a valid pointer to an array of at least `max` char pointers (or null if max is 0)
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
///
/// # Returns
/// - If max == 0: returns the total number of dependencies (result can be null)
/// - If max is too small: returns a negative error code
/// - Otherwise: returns the number of dependencies written to result
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_incoming_links(
    model: *mut SimlinModel,
    var_name: *const c_char,
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

    if var_name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let model_ref = ffi_try!(out_error, require_model(model));
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let var_name = match CStr::from_ptr(var_name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };

    let eng_model = match project_locked
        .models
        .get(&canonicalize(&model_ref.model_name))
    {
        Some(m) => m,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::BadModelName)
                    .with_message(format!("model '{}' not found", model_ref.model_name)),
            );
            return;
        }
    };

    let var = match eng_model.variables.get(&var_name) {
        Some(v) => v,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "variable '{}' does not exist in model '{}'",
                    var_name, model_ref.model_name
                )),
            );
            return;
        }
    };

    let deps_set = match var {
        engine::Variable::Stock { init_ast, .. } => {
            if let Some(ast) = init_ast {
                engine::identifier_set(ast, &[], None)
            } else {
                std::collections::HashSet::new()
            }
        }
        engine::Variable::Var { ast, .. } => {
            if let Some(ast) = ast {
                engine::identifier_set(ast, &[], None)
            } else {
                std::collections::HashSet::new()
            }
        }
        engine::Variable::Module { inputs, .. } => {
            inputs.iter().map(|input| input.src.clone()).collect()
        }
    };

    let deps_set = engine::resolve_non_private_dependencies(eng_model, deps_set);
    let mut deps: Vec<String> = deps_set
        .into_iter()
        .map(|ident| ident.to_string())
        .collect();
    deps.sort();

    if max == 0 {
        *out_written = deps.len();
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

    if max < deps.len() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                "buffer too small for dependencies: capacity {}, required {}",
                max,
                deps.len()
            )),
        );
        return;
    }

    for (i, dep) in deps.iter().enumerate() {
        let c_string = match CString::new(dep.as_str()) {
            Ok(s) => s,
            Err(_) => {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "dependency name contains interior NUL byte and cannot be converted",
                    ),
                );
                return;
            }
        };
        *result.add(i) = c_string.into_raw();
    }

    *out_written = deps.len();
}

/// Gets all causal links in a model
///
/// Returns all causal links detected in the model.
/// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - The returned SimlinLinks must be freed with simlin_free_links
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_links(
    model: *mut SimlinModel,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinLinks {
    clear_out_error(out_error);
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let eng_model = match project_locked
        .models
        .get(&canonicalize(&model_ref.model_name))
    {
        Some(m) => m,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::BadModelName)
                    .with_message(format!("model '{}' not found", model_ref.model_name)),
            );
            return ptr::null_mut();
        }
    };

    // Collect all unique links (de-duplicate based on from-to pairs)
    let mut unique_links = std::collections::HashMap::new();
    for (var_name, var) in &eng_model.variables {
        let deps = match var {
            engine::Variable::Stock {
                inflows, outflows, ..
            } => {
                let mut deps = Vec::new();
                for flow in inflows.iter().chain(outflows.iter()) {
                    deps.push((flow.clone(), var_name.clone()));
                }
                deps
            }
            engine::Variable::Var { ast, .. } if ast.is_some() => {
                let deps = engine::identifier_set(ast.as_ref().unwrap(), &[], None);
                deps.into_iter()
                    .map(|dep| (dep, var_name.clone()))
                    .collect()
            }
            engine::Variable::Module { inputs, .. } => inputs
                .iter()
                .map(|input| (input.src.clone(), var_name.clone()))
                .collect(),
            _ => Vec::new(),
        };

        for (from, to) in deps {
            let key = (from.clone(), to.clone());
            unique_links.entry(key).or_insert(engine::ltm::Link {
                from,
                to,
                polarity: engine::ltm::LinkPolarity::Unknown,
            });
        }
    }

    if unique_links.is_empty() {
        return Box::into_raw(Box::new(SimlinLinks {
            links: ptr::null_mut(),
            count: 0,
        }));
    }

    // Convert to C structures (without LTM scores since this is model-level)
    let mut c_links = Vec::with_capacity(unique_links.len());
    for (_, link) in unique_links {
        let c_link = SimlinLink {
            from: CString::new(link.from.as_str()).unwrap().into_raw(),
            to: CString::new(link.to.as_str()).unwrap().into_raw(),
            polarity: match link.polarity {
                engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
                engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
                engine::ltm::LinkPolarity::Unknown => SimlinLinkPolarity::Unknown,
            },
            score: ptr::null_mut(),
            score_len: 0,
        };
        c_links.push(c_link);
    }

    let links = Box::new(SimlinLinks {
        links: c_links.as_mut_ptr(),
        count: c_links.len(),
    });
    std::mem::forget(c_links);
    Box::into_raw(links)
}

/// Helper function to create a VM for a given project and model
fn create_vm(project: &engine::Project, model_name: &str) -> Result<Vm, engine::Error> {
    let compiler = engine::Simulation::new(project, model_name)?;
    let compiled = compiler.compile()?;
    Vm::new(compiled)
}

/// Creates a new simulation context
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_new(
    model: *mut SimlinModel,
    enable_ltm: bool,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinSim {
    clear_out_error(out_error);
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    let project_ptr = model_ref.project;
    let project_ref = &*project_ptr;

    let cloned_project = {
        let project_locked = project_ref.project.lock().unwrap();
        project_locked.clone()
    };

    let project_variant = if enable_ltm {
        match cloned_project.with_ltm() {
            Ok(proj) => proj,
            Err(err) => {
                store_ffi_error(out_error, ffi_error_from_engine(&err));
                return ptr::null_mut();
            }
        }
    } else {
        cloned_project
    };

    simlin_model_ref(model);

    let vm = create_vm(&project_variant, &model_ref.model_name).ok();
    let sim = Box::new(SimlinSim {
        model: model_ref as *const _,
        enable_ltm,
        state: Mutex::new(SimState { vm, results: None }),
        ref_count: AtomicUsize::new(1),
    });

    Box::into_raw(sim)
}
/// Increments the reference count of a simulation
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_ref(sim: *mut SimlinSim) {
    if !sim.is_null() {
        (*sim).ref_count.fetch_add(1, Ordering::SeqCst);
    }
}
/// Decrements the reference count and frees the simulation if it reaches zero
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_unref(sim: *mut SimlinSim) {
    if sim.is_null() {
        return;
    }
    let prev_count = (*sim).ref_count.fetch_sub(1, Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(Ordering::SeqCst);
        let sim = Box::from_raw(sim);
        // Decrement model reference count
        simlin_model_unref(sim.model as *mut SimlinModel);
    }
}
/// Runs the simulation to a specified time
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_run_to(
    sim: *mut SimlinSim,
    time: c_double,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let mut state = sim_ref.state.lock().unwrap();
    if let Some(ref mut vm) = state.vm {
        if let Err(err) = vm.run_to(time) {
            store_ffi_error(out_error, ffi_error_from_engine(&err));
        }
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has not been initialised with a VM"),
        );
    }
}
/// Runs the simulation to completion
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_run_to_end(
    sim: *mut SimlinSim,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let mut state = sim_ref.state.lock().unwrap();
    if let Some(mut vm) = state.vm.take() {
        match vm.run_to_end() {
            Ok(_) => {
                state.results = Some(vm.into_results());
            }
            Err(err) => {
                state.vm = Some(vm);
                store_ffi_error(out_error, ffi_error_from_engine(&err));
            }
        }
    } else if state.results.is_none() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has not been initialised with a VM"),
        );
    }
}
/// Gets the number of time steps in the results
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_stepcount(
    sim: *mut SimlinSim,
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

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let state = sim_ref.state.lock().unwrap();
    if let Some(ref results) = state.results {
        *out_count = results.step_count;
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has no results; run the simulation first"),
        );
    }
}
/// Resets the simulation to its initial state
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_reset(sim: *mut SimlinSim, out_error: *mut *mut SimlinError) {
    clear_out_error(out_error);
    let sim_ref = ffi_try!(out_error, require_sim(sim));

    let model = &*sim_ref.model;
    let project = &*model.project;

    let project_variant = {
        let project_locked = project.project.lock().unwrap();
        if sim_ref.enable_ltm {
            match project_locked.clone().with_ltm() {
                Ok(proj) => proj,
                Err(err) => {
                    store_ffi_error(out_error, ffi_error_from_engine(&err));
                    return;
                }
            }
        } else {
            project_locked.clone()
        }
    };

    let new_vm = match create_vm(&project_variant, &model.model_name) {
        Ok(vm) => vm,
        Err(err) => {
            store_ffi_error(out_error, ffi_error_from_engine(&err));
            return;
        }
    };

    let mut state = sim_ref.state.lock().unwrap();
    state.results = None;
    state.vm = Some(new_vm);
}
/// Gets a single value from the simulation
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
/// - `result` must be a valid pointer to a double
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_value(
    sim: *mut SimlinSim,
    name: *const c_char,
    out_value: *mut c_double,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_value.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_value pointer must not be NULL"),
        );
        return;
    }
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let canon_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };

    let state = sim_ref.state.lock().unwrap();
    if let Some(ref vm) = state.vm {
        if let Some(off) = vm.get_offset(&canon_name) {
            *out_value = vm.get_value_now(off);
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::UnknownDependency).with_message(format!(
                    "variable '{}' is not available in the simulation VM",
                    canon_name
                )),
            );
        }
    } else if let Some(ref results) = state.results {
        if let Some(&offset) = results.offsets.get(&canon_name) {
            if let Some(last_row) = results.iter().next_back() {
                *out_value = last_row[offset];
            } else {
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("simulation results are empty"),
                );
            }
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::UnknownDependency).with_message(format!(
                    "variable '{}' not found in simulation results",
                    canon_name
                )),
            );
        }
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has neither VM nor results; run the simulation first"),
        );
    }
}
/// Sets a value in the simulation
///
/// This function sets values at different phases of simulation:
/// - Before first run_to: Sets initial value to be used when simulation starts
/// - During simulation (after run_to): Sets value in current data for next iteration
/// - After run_to_end: Returns error (simulation complete)
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_set_value(
    sim: *mut SimlinSim,
    name: *const c_char,
    val: c_double,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let canon_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };

    let mut state = sim_ref.state.lock().unwrap();
    if let Some(ref mut vm) = state.vm {
        if let Some(off) = vm.get_offset(&canon_name) {
            vm.set_value_now(off, val);
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::UnknownDependency).with_message(format!(
                    "variable '{}' is not available in the simulation VM",
                    canon_name
                )),
            );
        }
    } else if state.results.is_some() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::NotSimulatable)
                .with_message("simulation already completed; cannot set values"),
        );
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has not been initialised with a VM"),
        );
    }
}
/// Sets the value for a variable at the last saved timestep by offset
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_set_value_by_offset(
    sim: *mut SimlinSim,
    offset: usize,
    val: c_double,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let mut state = sim_ref.state.lock().unwrap();
    if let Some(ref mut results) = state.results {
        if results.step_count == 0 || offset >= results.step_size {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                    "offset {} is out of bounds for step size {}",
                    offset, results.step_size
                )),
            );
            return;
        }
        let idx = (results.step_count - 1) * results.step_size + offset;
        if let Some(slot) = results.data.get_mut(idx) {
            *slot = val;
            return;
        }
    }

    store_error(
        out_error,
        SimlinError::new(SimlinErrorCode::Generic)
            .with_message("simulation does not have results to update"),
    );
}
/// Gets the column offset for a variable by name
///
/// Returns the column offset for a variable name at the current context, or -1 if not found.
/// This canonicalizes the name and resolves in the VM if present, otherwise in results.
/// Intended for debugging/tests to verify name→offset resolution.
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_offset(
    sim: *mut SimlinSim,
    name: *const c_char,
    out_offset: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_offset.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_offset pointer must not be NULL"),
        );
        return;
    }
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let canon_name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };

    let state = sim_ref.state.lock().unwrap();
    if let Some(ref vm) = state.vm {
        if let Some(off) = vm.get_offset(&canon_name) {
            *out_offset = off;
            return;
        }
    } else if let Some(ref results) = state.results {
        if let Some(&off) = results.offsets.get(&canon_name) {
            *out_offset = off;
            return;
        }
    }

    store_error(
        out_error,
        SimlinError::new(SimlinErrorCode::DoesNotExist)
            .with_message(format!("variable '{}' was not found", canon_name)),
    );
}
/// Gets a time series for a variable
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `name` must be a valid C string
/// - `results_ptr` must point to allocated memory of at least `len` doubles
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_series(
    sim: *mut SimlinSim,
    name: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
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
    if results_ptr.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("results pointer must not be NULL"),
        );
        return;
    }
    if name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let name = match CStr::from_ptr(name).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };

    let state = sim_ref.state.lock().unwrap();
    if let Some(ref results) = state.results {
        if let Some(&offset) = results.offsets.get(&name) {
            let count = std::cmp::min(results.step_count, len);
            for (i, row) in results.iter().take(count).enumerate() {
                *results_ptr.add(i) = row[offset];
            }
            *out_written = count;
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist)
                    .with_message(format!("series '{}' not found in results", name)),
            );
        }
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has no results; run the simulation first"),
        );
    }
}
/// Frees a string returned by the API
///
/// # Safety
/// - `s` must be a valid pointer returned by simlin API functions that return strings
#[no_mangle]
pub unsafe extern "C" fn simlin_free_string(s: *mut c_char) {
    if !s.is_null() {
        let _ = CString::from_raw(s);
    }
}
/// Gets all feedback loops in the project
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - The returned SimlinLoops must be freed with simlin_free_loops
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_loops(
    project: *mut SimlinProject,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinLoops {
    clear_out_error(out_error);
    let project_ref = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    let project = &project_ref.project;

    let project_locked = project.lock().unwrap();
    let loops_by_model = match detect_loops(&project_locked) {
        Ok(loops) => loops,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to detect loops: {err}")),
            );
            return ptr::null_mut();
        }
    };
    // Collect all loops from all models
    let mut all_loops = Vec::new();
    for (_model_name, model_loops) in loops_by_model {
        all_loops.extend(model_loops);
    }
    if all_loops.is_empty() {
        // Return empty result
        let result = Box::new(SimlinLoops {
            loops: ptr::null_mut(),
            count: 0,
        });
        return Box::into_raw(result);
    }
    // Convert to C structures
    let mut c_loops = Vec::with_capacity(all_loops.len());
    for loop_item in all_loops {
        // Convert loop ID to C string
        let id = CString::new(loop_item.id).unwrap();
        // Convert variable names to C strings
        let mut var_names = Vec::with_capacity(loop_item.links.len() + 1);
        // Collect unique variables from the loop path
        let mut seen = std::collections::HashSet::new();
        if !loop_item.links.is_empty() {
            // Add the first variable
            let first = &loop_item.links[0].from;
            if seen.insert(first.clone()) {
                let c_str = CString::new(first.as_str()).unwrap();
                var_names.push(c_str.into_raw());
            }
            // Add all 'to' variables
            for link in &loop_item.links {
                if seen.insert(link.to.clone()) {
                    let c_str = CString::new(link.to.as_str()).unwrap();
                    var_names.push(c_str.into_raw());
                }
            }
        }
        let var_count = var_names.len();
        let variables = if var_count > 0 {
            let mut vars = var_names.into_boxed_slice();
            let ptr = vars.as_mut_ptr();
            std::mem::forget(vars);
            ptr
        } else {
            ptr::null_mut()
        };
        // Convert polarity
        let polarity = match loop_item.polarity {
            LoopPolarity::Reinforcing => SimlinLoopPolarity::Reinforcing,
            LoopPolarity::Balancing => SimlinLoopPolarity::Balancing,
        };
        c_loops.push(SimlinLoop {
            id: id.into_raw(),
            variables,
            var_count,
            polarity,
        });
    }
    let count = c_loops.len();
    let mut loops = c_loops.into_boxed_slice();
    let loops_ptr = loops.as_mut_ptr();
    std::mem::forget(loops);
    let result = Box::new(SimlinLoops {
        loops: loops_ptr,
        count,
    });
    Box::into_raw(result)
}
/// Frees a SimlinLoops structure
///
/// # Safety
/// - `loops` must be a valid pointer returned by simlin_analyze_get_loops
#[no_mangle]
pub unsafe extern "C" fn simlin_free_loops(loops: *mut SimlinLoops) {
    if loops.is_null() {
        return;
    }
    let loops = Box::from_raw(loops);
    if !loops.loops.is_null() && loops.count > 0 {
        let loop_slice = std::slice::from_raw_parts_mut(loops.loops, loops.count);
        for loop_item in loop_slice {
            // Free the loop ID
            if !loop_item.id.is_null() {
                let _ = CString::from_raw(loop_item.id);
            }
            // Free the variable names
            if !loop_item.variables.is_null() && loop_item.var_count > 0 {
                let vars = std::slice::from_raw_parts_mut(loop_item.variables, loop_item.var_count);
                for var in vars {
                    if !var.is_null() {
                        let _ = CString::from_raw(*var);
                    }
                }
                let _ = Box::from_raw(std::slice::from_raw_parts_mut(
                    loop_item.variables,
                    loop_item.var_count,
                ));
            }
        }
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(loops.loops, loops.count));
    }
}
/// Gets all causal links in a model
///
/// Returns all causal links detected in the model.
/// This includes flow-to-stock, stock-to-flow, and auxiliary-to-auxiliary links.
/// If the simulation has been run with LTM enabled, link scores will be included.
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - The returned SimlinLinks must be freed with simlin_free_links
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_links(
    sim: *mut SimlinSim,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinLinks {
    clear_out_error(out_error);
    let sim_ref = match require_sim(sim) {
        Ok(s) => s,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    let model_ref = &*sim_ref.model;
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let model = match project_locked
        .models
        .get(&canonicalize(&model_ref.model_name))
    {
        Some(m) => m,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::BadModelName)
                    .with_message(format!("model '{}' not found", model_ref.model_name)),
            );
            return ptr::null_mut();
        }
    };

    let graph = match engine::ltm::CausalGraph::from_model(model, &project_locked) {
        Ok(g) => g,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to build causal graph: {err}")),
            );
            return ptr::null_mut();
        }
    };

    let loops = graph.find_loops();

    let mut unique_links = std::collections::HashMap::new();
    for loop_item in loops {
        for link in loop_item.links {
            let key = (link.from.clone(), link.to.clone());
            unique_links.entry(key).or_insert(link);
        }
    }

    for (var_name, var) in &model.variables {
        let deps = match var {
            engine::Variable::Stock {
                inflows, outflows, ..
            } => inflows
                .iter()
                .chain(outflows.iter())
                .map(|flow| (flow.clone(), var_name.clone()))
                .collect(),
            engine::Variable::Var { ast, .. } if ast.is_some() => {
                engine::identifier_set(ast.as_ref().unwrap(), &[], None)
                    .into_iter()
                    .map(|dep| (dep, var_name.clone()))
                    .collect()
            }
            engine::Variable::Module { inputs, .. } => inputs
                .iter()
                .map(|input| (input.src.clone(), var_name.clone()))
                .collect(),
            _ => Vec::new(),
        };

        for (from, to) in deps {
            let key = (from.clone(), to.clone());
            unique_links.entry(key).or_insert(engine::ltm::Link {
                from,
                to,
                polarity: engine::ltm::LinkPolarity::Unknown,
            });
        }
    }

    if unique_links.is_empty() {
        drop(project_locked);
        return Box::into_raw(Box::new(SimlinLinks {
            links: ptr::null_mut(),
            count: 0,
        }));
    }

    drop(project_locked);

    let has_ltm_scores = {
        let state = sim_ref.state.lock().unwrap();
        sim_ref.enable_ltm && state.results.is_some()
    };

    let mut c_links = Vec::with_capacity(unique_links.len());
    for (_, link) in unique_links {
        let from = CString::new(link.from.as_str()).unwrap().into_raw();
        let to = CString::new(link.to.as_str()).unwrap().into_raw();
        let polarity = match link.polarity {
            engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
            engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
            engine::ltm::LinkPolarity::Unknown => SimlinLinkPolarity::Unknown,
        };

        let (score_ptr, score_len) = if has_ltm_scores {
            let link_score_var = format!(
                "$⁚ltm⁚link_score⁚{}⁚{}",
                link.from.as_str(),
                link.to.as_str()
            );
            let var_ident = canonicalize(&link_score_var);

            let state = sim_ref.state.lock().unwrap();
            if let Some(ref results) = state.results {
                if let Some(&offset) = results.offsets.get(&var_ident) {
                    let mut scores = Vec::with_capacity(results.step_count);
                    for row in results.iter() {
                        scores.push(row[offset]);
                    }
                    let score_len = scores.len();
                    let mut boxed = scores.into_boxed_slice();
                    let score_ptr = boxed.as_mut_ptr();
                    std::mem::forget(boxed);
                    (score_ptr, score_len)
                } else {
                    (ptr::null_mut(), 0)
                }
            } else {
                (ptr::null_mut(), 0)
            }
        } else {
            (ptr::null_mut(), 0)
        };

        c_links.push(SimlinLink {
            from,
            to,
            polarity,
            score: score_ptr,
            score_len,
        });
    }

    let count = c_links.len();
    let mut links = c_links.into_boxed_slice();
    let links_ptr = links.as_mut_ptr();
    std::mem::forget(links);

    Box::into_raw(Box::new(SimlinLinks {
        links: links_ptr,
        count,
    }))
}

/// Frees a SimlinLinks structure
///
/// # Safety
/// - `links` must be valid pointer returned by simlin_analyze_get_links
#[no_mangle]
pub unsafe extern "C" fn simlin_free_links(links: *mut SimlinLinks) {
    if links.is_null() {
        return;
    }
    let links = Box::from_raw(links);
    if !links.links.is_null() && links.count > 0 {
        let link_slice = std::slice::from_raw_parts_mut(links.links, links.count);
        for link in link_slice {
            // Free the from and to strings
            if !link.from.is_null() {
                let _ = CString::from_raw(link.from);
            }
            if !link.to.is_null() {
                let _ = CString::from_raw(link.to);
            }
            // Free the score array if present
            if !link.score.is_null() && link.score_len > 0 {
                let _ = Box::from_raw(std::slice::from_raw_parts_mut(link.score, link.score_len));
            }
        }
        let _ = Box::from_raw(std::slice::from_raw_parts_mut(links.links, links.count));
    }
}

/// Gets the relative loop score time series for a specific loop
///
/// Renamed for clarity from simlin_analyze_get_rel_loop_score
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim that has been run to completion
/// - `loop_id` must be a valid C string
/// - `results` must be a valid pointer to an array of at least `len` doubles
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_relative_loop_score(
    sim: *mut SimlinSim,
    loop_id: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
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
    if results_ptr.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("results pointer must not be NULL"),
        );
        return;
    }
    if loop_id.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("loop_id pointer must not be NULL"),
        );
        return;
    }

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let loop_id = match CStr::from_ptr(loop_id).to_str() {
        Ok(s) => s,
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("loop_id is not valid UTF-8"),
            );
            return;
        }
    };

    let var_name = format!("$⁚ltm⁚rel_loop_score⁚{loop_id}");
    let var_ident = canonicalize(&var_name);

    let state = sim_ref.state.lock().unwrap();
    if let Some(ref results) = state.results {
        if let Some(&offset) = results.offsets.get(&var_ident) {
            let count = std::cmp::min(results.step_count, len);
            for (i, row) in results.iter().take(count).enumerate() {
                *results_ptr.add(i) = row[offset];
            }
            *out_written = count;
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "loop '{}' does not have relative score data",
                    loop_id
                )),
            );
        }
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has no results; run the simulation first"),
        );
    }
}

/// # Safety
///
/// - `sim` must be a valid pointer to a SimlinSim object
/// - `loop_id` must be a valid null-terminated C string
/// - `results_ptr` must point to a valid array of at least `len` doubles
/// - `out_written` must be a valid pointer to a usize
/// - `out_error` may be null or a valid pointer to a SimlinError pointer
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_rel_loop_score(
    sim: *mut SimlinSim,
    loop_id: *const c_char,
    results_ptr: *mut c_double,
    len: usize,
    out_written: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    simlin_analyze_get_relative_loop_score(sim, loop_id, results_ptr, len, out_written, out_error);
}
// Memory management functions for WASM
// We use a simple approach where we store the size before the allocated memory
#[no_mangle]
pub extern "C" fn simlin_malloc(size: usize) -> *mut u8 {
    unsafe {
        // Allocate extra space to store the size
        let total_size = size + size_of::<usize>();
        let layout = Layout::from_size_align_unchecked(total_size, align_of::<usize>());
        let ptr = alloc(layout);
        if ptr.is_null() {
            return ptr;
        }
        // Store the size at the beginning
        *(ptr as *mut usize) = size;
        // Return pointer to the user data (after the size)
        ptr.add(size_of::<usize>())
    }
}
/// Frees memory allocated by simlin_malloc
///
/// # Safety
/// - `ptr` must be a valid pointer returned by simlin_malloc, or null
/// - The pointer must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn simlin_free(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    // Get the actual allocation pointer (before the user data)
    let actual_ptr = ptr.sub(size_of::<usize>());
    // Read the size
    let size = *(actual_ptr as *mut usize);
    let total_size = size + size_of::<usize>();
    let layout = Layout::from_size_align_unchecked(total_size, align_of::<usize>());
    dealloc(actual_ptr, layout);
}
/// simlin_import_xmile opens a project from XMILE/STMX format data.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_import_xmile(
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

    match simlin_compat::open_xmile(&mut reader) {
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
/// simlin_import_mdl opens a project from Vensim MDL format data.
///
/// # Safety
/// - `data` must be a valid pointer to at least `len` bytes
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_import_mdl(
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

    match simlin_compat::open_vensim(&mut reader) {
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
/// simlin_export_xmile exports a project to XMILE format.
/// Returns 0 on success, error code on failure.
/// Caller must free output with simlin_free().
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `output` and `output_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_export_xmile(
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

    let proj = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    let project_locked = proj.project.lock().unwrap();
    match simlin_compat::to_xmile(&project_locked.datamodel) {
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

/// Serializes a project to binary protobuf format
///
/// Returns the project's datamodel serialized as protobuf bytes.
/// This is the native format expected by simlin_project_open.
/// Useful for saving projects or transferring them between systems.
///
/// Returns 0 on success, error code on failure.
/// Caller must free output with simlin_free().
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `output` and `output_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_project_serialize(
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

/// Applies a patch to the project datamodel.
///
/// The patch is encoded as a `project_io.Patch` protobuf message. The caller can
/// request a dry run (which performs validation without committing) and control
/// whether errors are permitted. When `allow_errors` is false, any static or
/// simulation error will cause the patch to be rejected.
///
/// On success returns `SimlinErrorCode::NoError`. On failure returns an error
/// code describing why the patch could not be applied. When `out_errors` is not
/// Internal helper that applies a ProjectPatch to a project.
///
/// This is the core patch application logic shared by both protobuf and JSON entry points.
/// It handles datamodel cloning, patch application, error gathering, validation, and
/// committing changes (unless dry_run is true).
unsafe fn apply_project_patch_internal(
    project_ref: &mut SimlinProject,
    patch: engine::project_io::ProjectPatch,
    dry_run: bool,
    allow_errors: bool,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    let mut staged_datamodel = {
        let project_locked = project_ref.project.lock().unwrap();
        project_locked.datamodel.clone()
    };

    if let Err(err) = engine::apply_patch(&mut staged_datamodel, &patch) {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::from(err.code))
                .with_message(format!("failed to apply patch: {err}")),
        );
        return;
    }

    let staged_project = engine::Project::from(staged_datamodel);

    let (all_errors, sim_error) = gather_error_details(&staged_project);

    let maybe_first_code = if !allow_errors {
        first_error_code(&staged_project, sim_error.as_ref())
    } else {
        None
    };

    if !out_collected_errors.is_null() && !all_errors.is_empty() {
        let code = maybe_first_code
            .or_else(|| all_errors.first().map(|detail| detail.code))
            .unwrap_or(SimlinErrorCode::NoError);
        let aggregate = build_simlin_error(code, &all_errors);
        *out_collected_errors = aggregate.into_raw();
    }

    if let Some(code) = maybe_first_code {
        let error = build_simlin_error(code, &all_errors);
        store_error(out_error, error);
        return;
    }

    if !dry_run {
        let mut project_locked = project_ref.project.lock().unwrap();
        *project_locked = staged_project;
    }
}

/// Applies a patch to the project datamodel.
///
/// On success returns without populating `out_error`. When `out_collected_errors` is
/// non-null it receives a pointer to a `SimlinError` describing all detected issues; callers
/// must free it with `simlin_error_free`.
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - `patch_data` must be a valid pointer to at least `patch_len` bytes
/// - `out_collected_errors` and `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_project_apply_patch(
    project: *mut SimlinProject,
    patch_data: *const u8,
    patch_len: usize,
    dry_run: bool,
    allow_errors: bool,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !out_collected_errors.is_null() {
        *out_collected_errors = ptr::null_mut();
    }

    let project_ref = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    if patch_len > 0 && patch_data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("patch_data pointer must not be NULL when patch_len > 0"),
        );
        return;
    }

    let patch_slice = if patch_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(patch_data, patch_len)
    };

    let patch = match engine::project_io::ProjectPatch::decode(patch_slice) {
        Ok(patch) => patch,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::ProtobufDecode)
                    .with_message(format!("failed to decode patch: {err}")),
            );
            return;
        }
    };

    apply_project_patch_internal(
        project_ref,
        patch,
        dry_run,
        allow_errors,
        out_collected_errors,
        out_error,
    );
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
    format: ffi::SimlinJsonFormat,
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

/// Applies a JSON patch to the project datamodel.
///
/// # Safety
/// - `project` must point to a valid `SimlinProject`.
/// - `patch_data` must either be null with `patch_len == 0` or reference at
///   least `patch_len` bytes containing UTF-8 JSON.
/// - `out_collected_errors` and `out_error` must be valid pointers for writing
///   error details and may be set to null on success.
///
/// # Thread Safety
/// - This function is thread-safe for concurrent calls with the same `project` pointer.
/// - The underlying `engine::Project` uses `Arc<ModelStage1>` and is protected by a `Mutex`.
/// - Multiple threads may safely modify the same project concurrently.
/// - Different projects may also be patched concurrently from different threads safely.
///
/// # Ownership and Mutation
/// - When `dry_run` is false, this function modifies the project in-place.
/// - When `dry_run` is true, the project remains unchanged and no modifications are committed.
/// - The `project` pointer remains valid and usable after this function returns.
/// - The project is not consumed or moved by this operation.
///
/// # Format Support
/// - Only `SimlinJsonFormat::Native` is supported for patches.
/// - SDAI format is only supported for full project import via `simlin_project_json_open`.
/// - Attempting to use SDAI format will return an error.
#[no_mangle]
pub unsafe extern "C" fn simlin_project_apply_patch_json(
    project: *mut SimlinProject,
    patch_data: *const u8,
    patch_len: usize,
    format: ffi::SimlinJsonFormat,
    dry_run: bool,
    allow_errors: bool,
    out_collected_errors: *mut *mut SimlinError,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if !out_collected_errors.is_null() {
        *out_collected_errors = ptr::null_mut();
    }

    if format != ffi::SimlinJsonFormat::Native {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("only native JSON format is supported"),
        );
        return;
    }

    if patch_len > 0 && patch_data.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("patch_data pointer must not be NULL when patch_len > 0"),
        );
        return;
    }

    let patch_slice = if patch_len == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(patch_data, patch_len)
    };

    let json_str = match std::str::from_utf8(patch_slice) {
        Ok(s) => s,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("input JSON is not valid UTF-8: {err}")),
            );
            return;
        }
    };

    let json_patch: JsonProjectPatch = match serde_json::from_str(json_str) {
        Ok(patch) => patch,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to parse JSON patch: {err}")),
            );
            return;
        }
    };

    let patch_proto = match convert_json_project_patch(json_patch) {
        Ok(patch) => patch,
        Err(err) => {
            store_ffi_error(out_error, err);
            return;
        }
    };

    let project_ref = match require_project(project) {
        Ok(p) => p,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    apply_project_patch_internal(
        project_ref,
        patch_proto,
        dry_run,
        allow_errors,
        out_collected_errors,
        out_error,
    );
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
enum JsonProjectOperation {
    SetSimSpecs { sim_specs: engine::json::SimSpecs },
}

#[derive(Debug, Deserialize)]
struct JsonProjectPatch {
    #[serde(default)]
    project_ops: Vec<JsonProjectOperation>,
    #[serde(default)]
    models: Vec<JsonModelPatch>,
}

#[derive(Debug, Deserialize)]
struct JsonModelPatch {
    name: String,
    #[serde(default)]
    ops: Vec<JsonModelOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
enum JsonModelOperation {
    UpsertAux {
        aux: engine::json::Auxiliary,
    },
    UpsertStock {
        stock: engine::json::Stock,
    },
    UpsertFlow {
        flow: engine::json::Flow,
    },
    UpsertModule {
        module: engine::json::Module,
    },
    DeleteVariable {
        ident: String,
    },
    RenameVariable {
        from: String,
        to: String,
    },
    UpsertView {
        index: u32,
        view: engine::json::View,
    },
    DeleteView {
        index: u32,
    },
}

fn convert_json_project_patch(
    patch: JsonProjectPatch,
) -> Result<engine::project_io::ProjectPatch, FfiError> {
    let mut project_ops = Vec::with_capacity(patch.project_ops.len());
    for op in patch.project_ops {
        let converted = convert_json_project_operation(op)?;
        project_ops.push(engine::project_io::ProjectOperation {
            op: Some(converted),
        });
    }

    let mut model_patches = Vec::with_capacity(patch.models.len());
    for model in patch.models {
        let mut ops = Vec::with_capacity(model.ops.len());
        for op in model.ops {
            let converted = convert_json_model_operation(op)?;
            ops.push(engine::project_io::ModelOperation {
                op: Some(converted),
            });
        }
        model_patches.push(engine::project_io::ModelPatch {
            name: model.name,
            ops,
        });
    }

    Ok(engine::project_io::ProjectPatch {
        project_ops,
        models: model_patches,
    })
}

fn convert_json_project_operation(
    op: JsonProjectOperation,
) -> Result<engine::project_io::project_operation::Op, FfiError> {
    use engine::datamodel;
    use engine::project_io;
    use engine::project_io::project_operation;

    let result = match op {
        JsonProjectOperation::SetSimSpecs { sim_specs } => {
            let dm_sim_specs: datamodel::SimSpecs = sim_specs.into();
            let sim_specs_pb: project_io::SimSpecs = dm_sim_specs.into();
            project_operation::Op::SetSimSpecs(project_io::SetSimSpecsOp {
                sim_specs: Some(sim_specs_pb),
            })
        }
    };

    Ok(result)
}

fn convert_json_model_operation(
    op: JsonModelOperation,
) -> Result<engine::project_io::model_operation::Op, FfiError> {
    use engine::datamodel;
    use engine::project_io;
    use engine::project_io::model_operation;

    let result = match op {
        JsonModelOperation::UpsertAux { aux } => {
            let dm_aux: datamodel::Aux = aux.into();
            let variable_pb = project_io::Variable::from(datamodel::Variable::Aux(dm_aux));
            let aux_pb = match variable_pb.v {
                Some(project_io::variable::V::Aux(aux)) => aux,
                _ => unreachable!(),
            };
            model_operation::Op::UpsertAux(project_io::UpsertAuxOp { aux: Some(aux_pb) })
        }
        JsonModelOperation::UpsertStock { stock } => {
            let dm_stock: datamodel::Stock = stock.into();
            let variable_pb = project_io::Variable::from(datamodel::Variable::Stock(dm_stock));
            let stock_pb = match variable_pb.v {
                Some(project_io::variable::V::Stock(stock)) => stock,
                _ => unreachable!(),
            };
            model_operation::Op::UpsertStock(project_io::UpsertStockOp {
                stock: Some(stock_pb),
            })
        }
        JsonModelOperation::UpsertFlow { flow } => {
            let dm_flow: datamodel::Flow = flow.into();
            let variable_pb = project_io::Variable::from(datamodel::Variable::Flow(dm_flow));
            let flow_pb = match variable_pb.v {
                Some(project_io::variable::V::Flow(flow)) => flow,
                _ => unreachable!(),
            };
            model_operation::Op::UpsertFlow(project_io::UpsertFlowOp {
                flow: Some(flow_pb),
            })
        }
        JsonModelOperation::UpsertModule { module } => {
            let dm_module: datamodel::Module = module.into();
            let variable_pb = project_io::Variable::from(datamodel::Variable::Module(dm_module));
            let module_pb = match variable_pb.v {
                Some(project_io::variable::V::Module(module)) => module,
                _ => unreachable!(),
            };
            model_operation::Op::UpsertModule(project_io::UpsertModuleOp {
                module: Some(module_pb),
            })
        }
        JsonModelOperation::DeleteVariable { ident } => {
            model_operation::Op::DeleteVariable(project_io::DeleteVariableOp { ident })
        }
        JsonModelOperation::RenameVariable { from, to } => {
            model_operation::Op::RenameVariable(project_io::RenameVariableOp { from, to })
        }
        JsonModelOperation::UpsertView { index, view } => {
            let dm_view: datamodel::View = view.into();
            let view_pb = project_io::View::from(dm_view);
            model_operation::Op::UpsertView(project_io::UpsertViewOp {
                index,
                view: Some(view_pb),
            })
        }
        JsonModelOperation::DeleteView { index } => {
            model_operation::Op::DeleteView(project_io::DeleteViewOp { index })
        }
    };

    Ok(result)
}

// Builder for error details used to populate SimlinError instances
struct ErrorDetailBuilder {
    code: SimlinErrorCode,
    message: Option<String>,
    model_name: Option<String>,
    variable_name: Option<String>,
    start_offset: u16,
    end_offset: u16,
}

impl ErrorDetailBuilder {
    fn new(code: ErrorCode) -> Self {
        Self {
            code: SimlinErrorCode::from(code),
            message: None,
            model_name: None,
            variable_name: None,
            start_offset: 0,
            end_offset: 0,
        }
    }

    fn message(mut self, msg: Option<String>) -> Self {
        self.message = msg;
        self
    }

    fn model_name(mut self, name: &str) -> Self {
        self.model_name = Some(name.to_string());
        self
    }

    fn variable_name(mut self, name: &str) -> Self {
        self.variable_name = Some(name.to_string());
        self
    }

    fn offsets(mut self, start: u16, end: u16) -> Self {
        self.start_offset = start;
        self.end_offset = end;
        self
    }

    fn build(self) -> ErrorDetailData {
        ErrorDetailData {
            code: self.code,
            message: self.message,
            model_name: self.model_name,
            variable_name: self.variable_name,
            start_offset: self.start_offset,
            end_offset: self.end_offset,
        }
    }

    fn from_formatted(error: errors::FormattedError) -> ErrorDetailData {
        let mut builder = ErrorDetailBuilder::new(error.code);
        if let Some(message) = error.message {
            builder = builder.message(Some(message));
        }
        if let Some(model_name) = error.model_name {
            builder = builder.model_name(&model_name);
        }
        if let Some(variable_name) = error.variable_name {
            builder = builder.variable_name(&variable_name);
        }
        builder
            .offsets(error.start_offset, error.end_offset)
            .build()
    }
}

fn collect_project_errors(project: &engine::Project) -> Vec<ErrorDetailData> {
    errors::collect_formatted_errors(project)
        .errors
        .into_iter()
        .map(ErrorDetailBuilder::from_formatted)
        .collect()
}

fn gather_error_details(
    project: &engine::Project,
) -> (Vec<ErrorDetailData>, Option<engine::Error>) {
    let mut all_errors = collect_project_errors(project);
    let sim_error = create_vm(project, "main").err();

    if let Some(error) = sim_error.clone() {
        let formatted = errors::format_simulation_error("main", &error);
        all_errors.push(ErrorDetailBuilder::from_formatted(formatted));
    }

    (all_errors, sim_error)
}

fn first_error_code(
    project: &engine::Project,
    sim_error: Option<&engine::Error>,
) -> Option<SimlinErrorCode> {
    if let Some(error) = project.errors.first() {
        return Some(SimlinErrorCode::from(error.code));
    }

    for model in project.models.values() {
        if let Some(errors) = &model.errors {
            if let Some(error) = errors.first() {
                return Some(SimlinErrorCode::from(error.code));
            }
        }

        if model
            .get_variable_errors()
            .values()
            .any(|errors| !errors.is_empty())
        {
            return Some(SimlinErrorCode::VariablesHaveErrors);
        }

        if model
            .get_unit_errors()
            .values()
            .any(|errors| !errors.is_empty())
        {
            return Some(SimlinErrorCode::UnitDefinitionErrors);
        }
    }

    sim_error.map(|error| SimlinErrorCode::from(error.code))
}

/// Get all errors in a project including static analysis and compilation errors
///
/// Returns NULL if no errors exist in the project. This function collects all
/// static errors (equation parsing, unit checking, etc.) and also attempts to
/// compile the "main" model to find any compilation-time errors.
///
/// The caller must free the returned error details using `simlin_free_error_details`.
///
/// # Example Usage (C)
/// ```c
/// SimlinErrorDetails* errors = simlin_project_get_errors(project);
/// if (errors != NULL) {
///     for (size_t i = 0; i < errors->count; i++) {
///         SimlinErrorDetail* error = &errors->errors[i];
///         printf("Error %d", error->code);
///         if (error->model_name != NULL) {
///             printf(" in model %s", error->model_name);
///         }
///         if (error->variable_name != NULL) {
///             printf(" for variable %s", error->variable_name);
///         }
///         printf("\n");
///     }
///     simlin_free_error_details(errors);
/// } else {
///     // Project has no errors and is ready to simulate
/// }
/// ```
///
/// # Safety
/// - `project` must be a valid pointer to a SimlinProject
/// - The returned pointer must be freed with `simlin_free_error_details`
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

#[cfg(test)]
mod tests {
    use super::*;
    use engine::test_common::TestProject;
    use serde_json::Value;
    #[test]
    fn test_error_str() {
        unsafe {
            let err_str = simlin_error_str(SimlinErrorCode::NoError);
            assert!(!err_str.is_null());
            let s = CStr::from_ptr(err_str);
            assert_eq!(s.to_str().unwrap(), "no_error");
        }
    }

    fn open_project_from_datamodel(project: &engine::datamodel::Project) -> *mut SimlinProject {
        let pb = engine_serde::serialize(project);
        let mut buf = Vec::new();
        pb.encode(&mut buf).unwrap();
        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
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

    fn aux_patch(model: &str, aux: engine::datamodel::Aux) -> Vec<u8> {
        let variable = engine::datamodel::Variable::Aux(aux);
        let aux_pb = match engine::project_io::Variable::from(variable).v.unwrap() {
            engine::project_io::variable::V::Aux(aux) => aux,
            _ => unreachable!(),
        };
        let patch = engine::project_io::ProjectPatch {
            project_ops: vec![],
            models: vec![engine::project_io::ModelPatch {
                name: model.to_string(),
                ops: vec![engine::project_io::ModelOperation {
                    op: Some(engine::project_io::model_operation::Op::UpsertAux(
                        engine::project_io::UpsertAuxOp { aux: Some(aux_pb) },
                    )),
                }],
            }],
        };
        let mut bytes = Vec::new();
        patch.encode(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn test_project_apply_patch_commits() {
        let datamodel = TestProject::new("test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let aux = engine::datamodel::Aux {
            ident: "new_aux".to_string(),
            equation: engine::datamodel::Equation::Scalar("5".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: engine::datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch_bytes = aux_patch("main", aux);

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                false,
                true,
                &mut collected_errors as *mut *mut SimlinError,
                &mut out_error as *mut *mut SimlinError,
            );
            assert!(out_error.is_null(), "expected no error");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("new_aux").is_some());
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_errors_respected() {
        let datamodel = TestProject::new("test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let aux = engine::datamodel::Aux {
            ident: "bad_aux".to_string(),
            equation: engine::datamodel::Equation::Scalar(String::new(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: engine::datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch_bytes = aux_patch("main", aux);

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                false,
                false,
                &mut collected_errors as *mut *mut SimlinError,
                &mut out_error as *mut *mut SimlinError,
            );
            assert!(!out_error.is_null(), "expected an error");
            let code = simlin_error_get_code(out_error);
            assert_eq!(code, SimlinErrorCode::VariablesHaveErrors);
            simlin_error_free(out_error);
            assert!(!collected_errors.is_null());
            simlin_error_free(collected_errors);

            // Project should remain unchanged
            {
                let project_locked = (*proj).project.lock().unwrap();
                let model = project_locked.datamodel.get_model("main").unwrap();
                assert!(model.get_variable("bad_aux").is_none());
            }

            let mut collected_errors_allow: *mut SimlinError = ptr::null_mut();
            let mut out_error_allow: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                false,
                true,
                &mut collected_errors_allow as *mut *mut SimlinError,
                &mut out_error_allow as *mut *mut SimlinError,
            );
            assert!(
                out_error_allow.is_null(),
                "expected no error when allowing errors"
            );
            assert!(!collected_errors_allow.is_null());
            simlin_error_free(collected_errors_allow);

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("bad_aux").is_some());
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_dry_run() {
        let datamodel = TestProject::new("test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let aux = engine::datamodel::Aux {
            ident: "dry_aux".to_string(),
            equation: engine::datamodel::Equation::Scalar("3".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: engine::datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
        };
        let patch_bytes = aux_patch("main", aux);

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                true,
                true,
                &mut collected_errors as *mut *mut SimlinError,
                &mut out_error as *mut *mut SimlinError,
            );
            assert!(out_error.is_null(), "expected no error");
            assert!(collected_errors.is_null());

            // Dry run should not commit changes
            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("dry_aux").is_none());
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_serialize_json_native() {
        let datamodel = TestProject::new("json_native").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Native,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error serializing json");
            assert!(!out_buffer.is_null(), "expected JSON buffer");

            let slice = std::slice::from_raw_parts(out_buffer, out_len);
            let json_str = std::str::from_utf8(slice).expect("valid utf-8 JSON");

            let actual: Value = serde_json::from_str(json_str).expect("parsed json");
            let expected_project: engine::json::Project = datamodel.clone().into();
            let expected = serde_json::to_value(expected_project).unwrap();

            assert_eq!(actual, expected);

            simlin_free(out_buffer);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_serialize_json_sdai() {
        let datamodel = TestProject::new("json_sdai").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Sdai,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(
                out_error.is_null(),
                "expected no error serializing SDAI json"
            );
            assert!(!out_buffer.is_null(), "expected SDAI JSON buffer");

            let slice = std::slice::from_raw_parts(out_buffer, out_len);
            let json_str = std::str::from_utf8(slice).expect("valid utf-8 JSON");

            let actual: Value = serde_json::from_str(json_str).expect("parsed json");
            let expected_model: engine::json_sdai::SdaiModel = datamodel.clone().into();
            let expected = serde_json::to_value(expected_model).unwrap();

            assert_eq!(actual, expected);

            simlin_free(out_buffer);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_json_roundtrip_sdai() {
        let original_datamodel = TestProject::new("sdai_roundtrip")
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "population * 0.02", None)
            .flow("deaths", "population * 0.01", None)
            .build_datamodel();
        let proj = open_project_from_datamodel(&original_datamodel);

        unsafe {
            // Serialize to SDAI format
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Sdai,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(out_error.is_null(), "serialization should succeed");
            assert!(!out_buffer.is_null());

            // Re-open from SDAI JSON
            let mut open_error: *mut SimlinError = ptr::null_mut();
            let proj2 = simlin_project_json_open(
                out_buffer,
                out_len,
                ffi::SimlinJsonFormat::Sdai,
                &mut open_error,
            );

            assert!(open_error.is_null(), "open from SDAI JSON should succeed");
            assert!(!proj2.is_null());

            // Verify the model exists and has the expected variables
            let mut get_model_error: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(proj2, ptr::null(), &mut get_model_error);
            assert!(get_model_error.is_null());
            assert!(!model.is_null());

            // Verify variables exist
            let project2_locked = (*proj2).project.lock().unwrap();
            let roundtrip_datamodel = &project2_locked.datamodel;
            let roundtrip_model = roundtrip_datamodel.get_model("main").unwrap();

            assert!(roundtrip_model.get_variable("population").is_some());
            assert!(roundtrip_model.get_variable("births").is_some());
            assert!(roundtrip_model.get_variable("deaths").is_some());
            drop(project2_locked);

            simlin_free(out_buffer);
            simlin_model_unref(model);
            simlin_project_unref(proj2);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_serialize_json_null_out_buffer() {
        let datamodel = TestProject::new("error_test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut out_len: usize = 0;
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Native,
                ptr::null_mut(),
                &mut out_len,
                &mut out_error,
            );

            assert!(!out_error.is_null(), "expected error for NULL out_buffer");
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_serialize_json_null_out_len() {
        let datamodel = TestProject::new("error_test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Native,
                &mut out_buffer,
                ptr::null_mut(),
                &mut out_error,
            );

            assert!(!out_error.is_null(), "expected error for NULL out_len");
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_serialize_json_null_project() {
        unsafe {
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                ptr::null_mut(),
                ffi::SimlinJsonFormat::Native,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(!out_error.is_null(), "expected error for NULL project");
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);
        }
    }

    #[test]
    fn test_serialize_json_both_formats_work() {
        let datamodel = TestProject::new("format_test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            // Test Native format
            let mut out_buffer: *mut u8 = ptr::null_mut();
            let mut out_len: usize = 0;
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Native,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(out_error.is_null(), "Native format should succeed");
            assert!(!out_buffer.is_null());
            assert!(out_len > 0);
            simlin_free(out_buffer);

            // Test SDAI format
            out_buffer = ptr::null_mut();
            out_len = 0;
            simlin_project_serialize_json(
                proj,
                ffi::SimlinJsonFormat::Sdai,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(out_error.is_null(), "SDAI format should succeed");
            assert!(!out_buffer.is_null());
            assert!(out_len > 0);
            simlin_free(out_buffer);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_commits() {
        let datamodel = TestProject::new("json_patch").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "json_aux",
                                    "equation": "7"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors as *mut *mut SimlinError,
                &mut out_error as *mut *mut SimlinError,
            );

            assert!(out_error.is_null(), "expected no error applying json patch");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("json_aux").is_some());
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_invalid() {
        let datamodel = TestProject::new("json_patch").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_bytes = b"{invalid";

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors as *mut *mut SimlinError,
                &mut out_error as *mut *mut SimlinError,
            );

            assert!(collected_errors.is_null());
            assert!(!out_error.is_null(), "expected error for invalid json");
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_upsert_stock() {
        let datamodel = TestProject::new("json_patch_stock").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_stock",
                            "payload": {
                                "stock": {
                                    "name": "inventory",
                                    "initial_equation": "50",
                                    "inflows": [],
                                    "outflows": []
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error upserting stock");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            let stock = model.get_variable("inventory");
            assert!(stock.is_some(), "stock should exist after upsert");
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_upsert_flow() {
        let datamodel = TestProject::new("json_patch_flow").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_flow",
                            "payload": {
                                "flow": {
                                    "name": "production",
                                    "equation": "10",
                                    "non_negative": true
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error upserting flow");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            let flow = model.get_variable("production");
            assert!(flow.is_some(), "flow should exist after upsert");
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_upsert_module() {
        // Create a project with a submodel that the module can reference
        let mut datamodel = TestProject::new("json_patch_module").build_datamodel();
        // Add a submodel to the project
        let submodel = engine::datamodel::Model {
            name: "SubModel".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
        };
        datamodel.models.push(submodel);

        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_module",
                            "payload": {
                                "module": {
                                    "name": "submodel",
                                    "model_name": "SubModel",
                                    "references": []
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error upserting module");
            // Modules referencing other models may have compilation errors, which is ok
            // when allow_errors=true

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            let module = model.get_variable("submodel");
            assert!(module.is_some(), "module should exist after upsert");
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_delete_variable() {
        let datamodel = TestProject::new("json_patch_delete")
            .aux("to_delete", "42", None)
            .build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("to_delete").is_some(),
                "variable should exist before delete"
            );
        }

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "delete_variable",
                            "payload": {
                                "ident": "to_delete"
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error deleting variable");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("to_delete").is_none(),
                "variable should not exist after delete"
            );
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_rename_variable() {
        let datamodel = TestProject::new("json_patch_rename")
            .aux("old_name", "123", None)
            .build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("old_name").is_some(),
                "old variable should exist before rename"
            );
        }

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "rename_variable",
                            "payload": {
                                "from": "old_name",
                                "to": "new_name"
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error renaming variable");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("old_name").is_none(),
                "old variable should not exist after rename"
            );
            assert!(
                model.get_variable("new_name").is_some(),
                "new variable should exist after rename"
            );
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_upsert_view() {
        let datamodel = TestProject::new("json_patch_view").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_view",
                            "payload": {
                                "index": 0,
                                "view": {
                                    "kind": "stock_flow",
                                    "elements": [],
                                    "view_box": {
                                        "x": 0,
                                        "y": 0,
                                        "width": 800,
                                        "height": 600
                                    },
                                    "zoom": 1.0
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error upserting view");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(!model.views.is_empty(), "view should exist after upsert");
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_delete_view() {
        let datamodel = TestProject::new("json_patch_delete_view").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        // First upsert a view
        let upsert_patch = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_view",
                            "payload": {
                                "index": 0,
                                "view": {
                                    "kind": "stock_flow",
                                    "elements": [],
                                    "view_box": {"x": 0, "y": 0, "width": 800, "height": 600},
                                    "zoom": 1.0
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let upsert_bytes = upsert_patch.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                upsert_bytes.as_ptr(),
                upsert_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );
            assert!(out_error.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(!model.views.is_empty(), "view should exist after upsert");
        }

        // Now delete the view
        let delete_patch = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "delete_view",
                            "payload": {
                                "index": 0
                            }
                        }
                    ]
                }
            ]
        }"#;
        let delete_bytes = delete_patch.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                delete_bytes.as_ptr(),
                delete_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error deleting view");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(model.views.is_empty(), "view should not exist after delete");
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_set_sim_specs() {
        let datamodel = TestProject::new("json_patch_sim_specs").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let original_start = unsafe { (*proj).project.lock().unwrap().datamodel.sim_specs.start };
        let original_stop = unsafe { (*proj).project.lock().unwrap().datamodel.sim_specs.stop };

        let patch_json = r#"{
            "project_ops": [
                {
                    "type": "set_sim_specs",
                    "payload": {
                        "sim_specs": {
                            "start_time": 2020.0,
                            "end_time": 2030.0,
                            "dt": "1",
                            "save_step": 1.0,
                            "method": "euler",
                            "time_units": "years"
                        }
                    }
                }
            ],
            "models": []
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            if !out_error.is_null() {
                let err_code = simlin_error_get_code(out_error);
                let err_msg = simlin_error_get_message(out_error);
                let msg_str = if !err_msg.is_null() {
                    std::ffi::CStr::from_ptr(err_msg)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    "no message".to_string()
                };
                simlin_error_free(out_error);
                panic!("error setting sim specs: {:?} - {}", err_code, msg_str);
            }
            assert!(collected_errors.is_null());

            let new_start = (*proj).project.lock().unwrap().datamodel.sim_specs.start;
            let new_stop = (*proj).project.lock().unwrap().datamodel.sim_specs.stop;

            assert_ne!(
                original_start, new_start,
                "start time should have been updated"
            );
            assert_ne!(
                original_stop, new_stop,
                "stop time should have been updated"
            );
            assert_eq!(new_start, 2020.0);
            assert_eq!(new_stop, 2030.0);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_project_and_model_ops() {
        let datamodel = TestProject::new("json_patch_combined").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "project_ops": [
                {
                    "type": "set_sim_specs",
                    "payload": {
                        "sim_specs": {
                            "start_time": 0.0,
                            "end_time": 100.0,
                            "dt": "0.5",
                            "save_step": 0.5,
                            "method": "euler",
                            "time_units": "months"
                        }
                    }
                }
            ],
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "combined_test",
                                    "equation": "42"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            if !out_error.is_null() {
                let err_code = simlin_error_get_code(out_error);
                let err_msg = simlin_error_get_message(out_error);
                let msg_str = if !err_msg.is_null() {
                    std::ffi::CStr::from_ptr(err_msg)
                        .to_string_lossy()
                        .into_owned()
                } else {
                    "no message".to_string()
                };
                simlin_error_free(out_error);
                panic!("error with combined ops: {:?} - {}", err_code, msg_str);
            }
            assert!(collected_errors.is_null());

            let new_stop = (*proj).project.lock().unwrap().datamodel.sim_specs.stop;
            assert_eq!(new_stop, 100.0, "sim specs should be updated");

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("combined_test").is_some(),
                "variable should exist"
            );
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_apply_patch_json_unsupported_format() {
        let datamodel = TestProject::new("json_patch_format").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{"models": []}"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Sdai,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(
                !out_error.is_null(),
                "expected error for SDAI format in apply_patch"
            );
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_apply_patch_json_invalid_utf8() {
        let datamodel = TestProject::new("utf8_test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let invalid_utf8: Vec<u8> = vec![0xFF, 0xFE, 0xFD];

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                invalid_utf8.as_ptr(),
                invalid_utf8.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(!out_error.is_null(), "expected error for invalid UTF-8");
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_apply_patch_json_empty_patch() {
        let datamodel = TestProject::new("empty_patch").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let empty_patch = r#"{"models": []}"#;
        let patch_bytes = empty_patch.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "empty patch should succeed as no-op");
            assert!(collected_errors.is_null());

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_apply_patch_json_dry_run_no_changes() {
        let datamodel = TestProject::new("dry_run_test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("dry_run_var").is_none(),
                "variable should not exist before patch"
            );
        }

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "dry_run_var",
                                    "equation": "99"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                true,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "dry run should succeed");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(
                model.get_variable("dry_run_var").is_none(),
                "variable should not exist after dry run"
            );
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_apply_patch_json_null_data_with_length() {
        let datamodel = TestProject::new("null_test").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                ptr::null(),
                10,
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(
                !out_error.is_null(),
                "expected error for NULL patch_data with length > 0"
            );
            assert_eq!(simlin_error_get_code(out_error), SimlinErrorCode::Generic);
            simlin_error_free(out_error);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_apply_patch_json_multiple_models() {
        // Create a project with multiple models
        let mut datamodel = TestProject::new("multi_model").build_datamodel();
        let second_model = engine::datamodel::Model {
            name: "SecondModel".to_string(),
            sim_specs: None,
            variables: vec![],
            views: vec![],
            loop_metadata: vec![],
        };
        datamodel.models.push(second_model);

        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "main_var",
                                    "equation": "1"
                                }
                            }
                        }
                    ]
                },
                {
                    "name": "SecondModel",
                    "ops": [
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "second_var",
                                    "equation": "2"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "multi-model patch should succeed");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let main_model = project_locked.datamodel.get_model("main").unwrap();
            assert!(main_model.get_variable("main_var").is_some());

            let second_model = project_locked.datamodel.get_model("SecondModel").unwrap();
            assert!(second_model.get_variable("second_var").is_some());
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_apply_patch_json_multiple_ops_per_model() {
        let datamodel = TestProject::new("multi_ops").build_datamodel();
        let proj = open_project_from_datamodel(&datamodel);

        let patch_json = r#"{
            "models": [
                {
                    "name": "main",
                    "ops": [
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "var1",
                                    "equation": "10"
                                }
                            }
                        },
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "var2",
                                    "equation": "20"
                                }
                            }
                        },
                        {
                            "type": "upsert_aux",
                            "payload": {
                                "aux": {
                                    "name": "var3",
                                    "equation": "30"
                                }
                            }
                        }
                    ]
                }
            ]
        }"#;
        let patch_bytes = patch_json.as_bytes();

        unsafe {
            let mut collected_errors: *mut SimlinError = ptr::null_mut();
            let mut out_error: *mut SimlinError = ptr::null_mut();
            simlin_project_apply_patch_json(
                proj,
                patch_bytes.as_ptr(),
                patch_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                false,
                true,
                &mut collected_errors,
                &mut out_error,
            );

            assert!(out_error.is_null(), "multiple ops should succeed");
            assert!(collected_errors.is_null());

            let project_locked = (*proj).project.lock().unwrap();
            let model = project_locked.datamodel.get_model("main").unwrap();
            assert!(model.get_variable("var1").is_some());
            assert!(model.get_variable("var2").is_some());
            assert!(model.get_variable("var3").is_some());
            drop(project_locked);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_interactive_set_get() {
        // Load the SIR project fixture
        let pb_path = std::path::Path::new("../../src/engine2/testdata/SIR_project.pb");
        if !pb_path.exists() {
            eprintln!("missing SIR_project.pb fixture; skipping");
            return;
        }
        let data = std::fs::read(pb_path).unwrap();

        unsafe {
            // Open project
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Get model
            err = ptr::null_mut();
            let model =
                simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!model.is_null());

            // Create sim
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Run to a partial time
            err = ptr::null_mut();
            simlin_sim_run_to(sim, 0.125, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to failed with error {:?}: {}", code, msg);
            }

            // Fetch var names from model
            err = ptr::null_mut();
            let mut count: usize = 0;
            simlin_model_get_var_count(
                model,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_var_count failed with error {:?}: {}", code, msg);
            }
            assert!(count > 0, "expected varcount > 0");

            let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count];
            let _written: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_names(
                model,
                name_ptrs.as_mut_ptr(),
                name_ptrs.len(),
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_var_names failed with error {:?}: {}", code, msg);
            }

            // Find canonical name that ends with "infectious"
            let mut infectious_name: Option<String> = None;
            for &p in &name_ptrs {
                if p.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
                // free the CString from get_var_names
                simlin_free_string(p as *mut c_char);
                if s.to_ascii_lowercase().ends_with("infectious") {
                    infectious_name = Some(s);
                }
            }
            let infectious = infectious_name.expect("infectious not found in names");

            // Read current value using canonical name
            let c_infectious = CString::new(infectious.clone()).unwrap();
            let mut out: c_double = 0.0;
            err = ptr::null_mut();
            simlin_sim_get_value(
                sim,
                c_infectious.as_ptr(),
                &mut out as *mut c_double,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_value failed with error {:?}: {}", code, msg);
            }

            // Set to a new value and read it back
            let new_val: f64 = 42.0;
            err = ptr::null_mut();
            simlin_sim_set_value(
                sim,
                c_infectious.as_ptr(),
                new_val as c_double,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("set_value failed with error {:?}: {}", code, msg);
            }

            let mut out2: c_double = 0.0;
            err = ptr::null_mut();
            simlin_sim_get_value(
                sim,
                c_infectious.as_ptr(),
                &mut out2 as *mut c_double,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!(
                    "get_value (after set) failed with error {:?}: {}",
                    code, msg
                );
            }
            assert!(
                (out2 - new_val).abs() <= 1e-9,
                "expected {new_val} got {out2}"
            );

            // Cleanup
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_set_value_phases() {
        // Load the SIR project fixture
        let pb_path = std::path::Path::new("../../src/engine2/testdata/SIR_project.pb");
        if !pb_path.exists() {
            eprintln!("missing SIR_project.pb fixture; skipping");
            return;
        }
        let data = std::fs::read(pb_path).unwrap();

        unsafe {
            // Open project
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Get model
            err = ptr::null_mut();
            let model =
                simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!model.is_null());

            // Test Phase 1: Set value before first run_to (initial value)
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Get variable names to find a valid variable
            err = ptr::null_mut();
            let mut count: usize = 0;
            simlin_model_get_var_count(
                model,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_var_count failed with error {:?}: {}", code, msg);
            }

            let mut name_ptrs: Vec<*mut c_char> = vec![std::ptr::null_mut(); count];
            let _written: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_names(
                model,
                name_ptrs.as_mut_ptr(),
                name_ptrs.len(),
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_var_names failed with error {:?}: {}", code, msg);
            }

            let mut test_var_name: Option<String> = None;
            for &p in &name_ptrs {
                if p.is_null() {
                    continue;
                }
                let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
                simlin_free_string(p as *mut c_char);
                if s.to_ascii_lowercase().ends_with("infectious") {
                    test_var_name = Some(s);
                    break;
                }
            }
            let test_var = test_var_name.expect("test variable not found");
            let c_test_var = CString::new(test_var.clone()).unwrap();

            // Set initial value before any run_to
            let initial_val: f64 = 100.0;
            err = ptr::null_mut();
            simlin_sim_set_value(
                sim,
                c_test_var.as_ptr(),
                initial_val,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("set_value before run failed with error {:?}: {}", code, msg);
            }

            // Verify initial value is set
            let mut out: c_double = 0.0;
            err = ptr::null_mut();
            simlin_sim_get_value(
                sim,
                c_test_var.as_ptr(),
                &mut out,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_value failed with error {:?}: {}", code, msg);
            }
            assert!(
                (out - initial_val).abs() <= 1e-9,
                "initial value not set correctly"
            );

            // Test Phase 2: Set value during simulation (after partial run)
            err = ptr::null_mut();
            simlin_sim_run_to(sim, 0.5, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to failed with error {:?}: {}", code, msg);
            }

            let during_val: f64 = 200.0;
            err = ptr::null_mut();
            simlin_sim_set_value(
                sim,
                c_test_var.as_ptr(),
                during_val,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("set_value during run failed with error {:?}: {}", code, msg);
            }

            err = ptr::null_mut();
            simlin_sim_get_value(
                sim,
                c_test_var.as_ptr(),
                &mut out,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_value failed with error {:?}: {}", code, msg);
            }
            assert!(
                (out - during_val).abs() <= 1e-9,
                "value during run not set correctly"
            );

            // Test Phase 3: Set value after run_to_end (should fail)
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to_end failed with error {:?}: {}", code, msg);
            }

            err = ptr::null_mut();
            simlin_sim_set_value(
                sim,
                c_test_var.as_ptr(),
                300.0,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected an error but got success");
            let code = simlin_error_get_code(err);
            assert_eq!(
                code,
                SimlinErrorCode::NotSimulatable,
                "set_value after completion should fail with NotSimulatable"
            );
            simlin_error_free(err);

            // Test setting unknown variable (should fail)
            let unknown = CString::new("unknown_variable_xyz").unwrap();
            err = ptr::null_mut();
            simlin_sim_set_value(
                sim,
                unknown.as_ptr(),
                999.0,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected an error but got success");
            let code = simlin_error_get_code(err);
            assert_eq!(
                code,
                SimlinErrorCode::UnknownDependency,
                "set_value for unknown variable should fail with UnknownDependency"
            );
            simlin_error_free(err);

            // Cleanup
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }
    // Model-only protobufs are not supported at the ABI layer; only Projects are accepted.
    #[test]
    fn test_project_lifecycle() {
        // Create a minimal valid protobuf project
        let project = engine::project_io::Project {
            name: "test".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();
        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());
            // Test reference counting
            simlin_project_ref(proj);
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);
            simlin_project_unref(proj);
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);
            simlin_project_unref(proj);
            // Project should be freed now
        }
    }
    #[test]
    fn test_import_xmile() {
        // Load the SIR XMILE model
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            eprintln!("missing SIR.stmx fixture; skipping");
            return;
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            // Import XMILE
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_import_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("import_xmile failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Get model and verify we can create a simulation from the imported project
            err = ptr::null_mut();
            let model =
                simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!model.is_null());

            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Run simulation to verify it's valid
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to_end failed with error {:?}: {}", code, msg);
            }

            // Check we have expected variables
            err = ptr::null_mut();
            let mut var_count: usize = 0;
            simlin_model_get_var_count(
                model,
                &mut var_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_var_count failed with error {:?}: {}", code, msg);
            }
            assert!(var_count > 0);

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_import_mdl() {
        // Load the SIR MDL model
        let mdl_path = std::path::Path::new("testdata/SIR.mdl");
        if !mdl_path.exists() {
            eprintln!("missing SIR.mdl fixture; skipping");
            return;
        }
        let data = std::fs::read(mdl_path).unwrap();

        unsafe {
            // Import MDL
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_import_mdl(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("import_mdl failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Get model and verify we can create a simulation from the imported project
            err = ptr::null_mut();
            let model =
                simlin_project_get_model(proj, std::ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!model.is_null());

            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Run simulation to verify it's valid
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to_end failed with error {:?}: {}", code, msg);
            }

            // Check we have expected variables
            err = ptr::null_mut();
            let mut var_count: usize = 0;
            simlin_model_get_var_count(
                model,
                &mut var_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("get_var_count failed with error {:?}: {}", code, msg);
            }
            assert!(var_count > 0);

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_export_xmile() {
        // Load a project from protobuf first
        let pb_path = std::path::Path::new("testdata/SIR_project.pb");
        if !pb_path.exists() {
            eprintln!("missing SIR_project.pb fixture; skipping");
            return;
        }
        let data = std::fs::read(pb_path).unwrap();

        unsafe {
            // Open project
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Export to XMILE
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_export_xmile(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("export_xmile failed with error {:?}: {}", code, msg);
            }
            assert!(!output.is_null());
            assert!(output_len > 0);

            // Verify the output is valid XMILE by trying to parse it
            let xmile_data = std::slice::from_raw_parts(output, output_len);
            let xmile_str = std::str::from_utf8(xmile_data).unwrap();
            assert!(xmile_str.contains("<?xml"));
            assert!(xmile_str.contains("<xmile"));

            // Clean up
            simlin_free(output);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_import_export_roundtrip() {
        // Load XMILE model
        let xmile_path = std::path::Path::new("testdata/SIR.stmx");
        if !xmile_path.exists() {
            eprintln!("missing SIR.stmx fixture; skipping");
            return;
        }
        let data = std::fs::read(xmile_path).unwrap();

        unsafe {
            // Import XMILE
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj1 =
                simlin_import_xmile(data.as_ptr(), data.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("import_xmile failed with error {:?}: {}", code, msg);
            }
            assert!(!proj1.is_null());

            // Export to XMILE
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_export_xmile(
                proj1,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("export_xmile failed with error {:?}: {}", code, msg);
            }

            // Import the exported XMILE
            err = ptr::null_mut();
            let proj2 = simlin_import_xmile(output, output_len, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("import_xmile (2nd) failed with error {:?}: {}", code, msg);
            }
            assert!(!proj2.is_null());

            // Get models and verify both projects can simulate
            err = ptr::null_mut();
            let model1 = simlin_project_get_model(
                proj1,
                std::ptr::null(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            err = ptr::null_mut();
            let model2 = simlin_project_get_model(
                proj2,
                std::ptr::null(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(!model1.is_null());
            assert!(err.is_null());
            assert!(!model2.is_null());

            err = ptr::null_mut();
            let sim1 = simlin_sim_new(model1, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            err = ptr::null_mut();
            let sim2 = simlin_sim_new(model2, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim1.is_null());
            assert!(!sim2.is_null());

            err = ptr::null_mut();
            simlin_sim_run_to_end(sim1, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to_end (1st) failed with error {:?}: {}", code, msg);
            }

            err = ptr::null_mut();
            simlin_sim_run_to_end(sim2, &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("sim_run_to_end (2nd) failed with error {:?}: {}", code, msg);
            }

            // Clean up
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_free(output);
            simlin_project_unref(proj1);
            simlin_project_unref(proj2);
        }
    }

    #[test]
    fn test_import_invalid_data() {
        unsafe {
            // Test with null data
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_import_xmile(std::ptr::null(), 0, &mut err as *mut *mut SimlinError);
            assert!(proj.is_null());
            assert!(!err.is_null(), "Expected an error but got success");
            simlin_error_free(err);

            // Test with invalid XML
            let bad_data = b"not xml at all";
            err = ptr::null_mut();
            let proj = simlin_import_xmile(
                bad_data.as_ptr(),
                bad_data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(proj.is_null());
            assert!(!err.is_null(), "Expected an error but got success");
            simlin_error_free(err);

            // Test with invalid MDL
            err = ptr::null_mut();
            let proj = simlin_import_mdl(
                bad_data.as_ptr(),
                bad_data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(proj.is_null());
            assert!(!err.is_null(), "Expected an error but got success");
            simlin_error_free(err);
        }
    }

    #[test]
    fn test_export_null_project() {
        unsafe {
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_export_xmile(
                std::ptr::null_mut(),
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected an error but got success");
            simlin_error_free(err);
            assert!(output.is_null());
        }
    }

    #[test]
    fn test_error_api_with_valid_project() {
        // Create a project with intentional errors
        let project = engine::project_io::Project {
            name: "test_errors".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![
                    // Variable with an equation error (unknown dependency)
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "error_var".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "unknown_var + 1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                    // Variable with bad units
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "bad_units_var".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: "bad units here!!!".to_string(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Test getting all errors
            let mut err_get_errors: *mut SimlinError = ptr::null_mut();
            let all_errors =
                simlin_project_get_errors(proj, &mut err_get_errors as *mut *mut SimlinError);
            assert!(err_get_errors.is_null());
            assert!(!all_errors.is_null());
            let count = simlin_error_get_detail_count(all_errors);
            assert!(count > 0);

            // Verify we can access error details
            let errors = simlin_error_get_details(all_errors);
            let error_slice = std::slice::from_raw_parts(errors, count);
            let mut found_unknown_dep = false;
            let mut found_bad_units = false;

            for error in error_slice {
                if error.code == SimlinErrorCode::UnknownDependency {
                    found_unknown_dep = true;
                    assert!(!error.variable_name.is_null());
                    let var_name = CStr::from_ptr(error.variable_name).to_str().unwrap();
                    assert_eq!(var_name, "error_var");
                }
                // Bad units will show up as an error during parsing
                if !error.variable_name.is_null() {
                    let var_name = CStr::from_ptr(error.variable_name).to_str().unwrap();
                    if var_name == "bad_units_var" {
                        found_bad_units = true;
                    }
                }
            }

            assert!(
                found_unknown_dep,
                "Should have found unknown dependency error"
            );
            assert!(found_bad_units, "Should have found bad units error");

            // Clean up
            simlin_error_free(all_errors);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_error_api_with_compilation_errors() {
        // Create a project with compilation errors
        let project = engine::project_io::Project {
            name: "test_compilation_errors".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![
                    // This will cause a compilation error - circular reference
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "a".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "b + 1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                    engine::project_io::Variable {
                        v: Some(engine::project_io::variable::V::Aux(
                            engine::project_io::variable::Aux {
                                ident: "b".to_string(),
                                equation: Some(engine::project_io::variable::Equation {
                                    equation: Some(
                                        engine::project_io::variable::equation::Equation::Scalar(
                                            engine::project_io::variable::ScalarEquation {
                                                equation: "a + 1".to_string(),
                                                initial_equation: None,
                                            },
                                        ),
                                    ),
                                }),
                                documentation: String::new(),
                                units: String::new(),
                                gf: None,
                                can_be_module_input: false,
                                visibility: engine::project_io::variable::Visibility::Private
                                    as i32,
                                uid: 0,
                            },
                        )),
                    },
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // The project should have compilation errors due to circular reference
            let mut err_get_errors: *mut SimlinError = ptr::null_mut();
            let all_errors =
                simlin_project_get_errors(proj, &mut err_get_errors as *mut *mut SimlinError);
            assert!(err_get_errors.is_null());
            assert!(!all_errors.is_null());
            let count = simlin_error_get_detail_count(all_errors);
            assert!(count > 0);

            // Verify we found the compilation error
            let errors = simlin_error_get_details(all_errors);
            let error_slice = std::slice::from_raw_parts(errors, count);
            let mut found_compilation_error = false;
            for error in error_slice {
                // Circular references or other compilation errors should be present
                if error.code == SimlinErrorCode::CircularDependency
                    || error.code == SimlinErrorCode::BadModelName
                    || error.code == SimlinErrorCode::Generic
                {
                    found_compilation_error = true;
                    break;
                }
            }
            assert!(
                found_compilation_error,
                "Should have found a compilation error"
            );

            // Clean up
            simlin_error_free(all_errors);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_error_api_no_errors() {
        // Create a valid project with no errors
        let project = engine::project_io::Project {
            name: "test_no_errors".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "time_var".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "time".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: String::new(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // Test that there are no errors (including compilation errors)
            err = ptr::null_mut();
            let all_errors = simlin_project_get_errors(proj, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(all_errors.is_null());

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_error_api_null_safety() {
        unsafe {
            // Test with null project
            let mut err: *mut SimlinError = ptr::null_mut();
            let errors =
                simlin_project_get_errors(ptr::null_mut(), &mut err as *mut *mut SimlinError);
            assert!(errors.is_null());

            // Test free functions with null (should not crash)
            simlin_error_free(ptr::null_mut());
            simlin_error_free(ptr::null_mut());
        }
    }

    #[test]
    fn test_error_offsets() {
        // Create a project with an error at a specific location
        let project = engine::project_io::Project {
            name: "test_offsets".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "var_with_offset_error".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "1 + unknown_var_here".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: String::new(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            let mut err_get_errors: *mut SimlinError = ptr::null_mut();
            let all_errors =
                simlin_project_get_errors(proj, &mut err_get_errors as *mut *mut SimlinError);
            assert!(err_get_errors.is_null());
            assert!(!all_errors.is_null());
            let count = simlin_error_get_detail_count(all_errors);
            assert!(count > 0);

            // Check that offsets are set (they should point to "unknown_var_here")
            let errors = simlin_error_get_details(all_errors);
            let error_slice = std::slice::from_raw_parts(errors, count);
            for error in error_slice {
                if error.code == SimlinErrorCode::UnknownDependency {
                    // The offset should point to the unknown variable reference
                    assert!(
                        error.start_offset > 0 || error.end_offset > 0,
                        "Error offsets should be set for unknown dependency"
                    );
                }
            }

            // Clean up
            simlin_error_free(all_errors);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_sim_lifecycle() {
        // Create a minimal valid protobuf project
        let project = engine::project_io::Project {
            name: "test".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![engine::project_io::Variable {
                    v: Some(engine::project_io::variable::V::Aux(
                        engine::project_io::variable::Aux {
                            ident: "time".to_string(),
                            equation: Some(engine::project_io::variable::Equation {
                                equation: Some(
                                    engine::project_io::variable::equation::Equation::Scalar(
                                        engine::project_io::variable::ScalarEquation {
                                            equation: "time".to_string(),
                                            initial_equation: None,
                                        },
                                    ),
                                ),
                            }),
                            documentation: String::new(),
                            units: String::new(),
                            gf: None,
                            can_be_module_input: false,
                            visibility: engine::project_io::variable::Visibility::Private as i32,
                            uid: 0,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();
        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());
            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());
            // Project ref count should have increased when model was created
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);

            // Test model reference counting
            simlin_model_ref(model);
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);
            simlin_model_unref(model);
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());
            // Model ref count should have increased when sim was created
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 2);

            // Test sim reference counting
            simlin_sim_ref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 2);
            simlin_sim_unref(sim);
            assert_eq!((*sim).ref_count.load(Ordering::SeqCst), 1);
            simlin_sim_unref(sim);
            // Sim should be freed now, model ref count should decrease
            assert_eq!((*model).ref_count.load(Ordering::SeqCst), 1);

            simlin_model_unref(model);
            // Model should be freed now, project ref count should decrease
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_analyze_get_links() {
        // Create a project with a reinforcing loop using TestProject
        let test_project = TestProject::new("test_links")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * birth_rate", None)
            .aux("birth_rate", "0.02", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Test without LTM enabled - should get structural links only
            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            err = ptr::null_mut();
            let links = simlin_analyze_get_links(sim, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!links.is_null());
            assert!((*links).count > 0, "Should have detected causal links");

            // Verify link structure
            let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);

            // Should have links like:
            // - birth_rate -> births
            // - population -> births
            // - births -> population
            let mut found_rate_to_births = false;
            let mut found_pop_to_births = false;
            let mut found_births_to_pop = false;

            for link in links_slice {
                assert!(!link.from.is_null());
                assert!(!link.to.is_null());

                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                if from == "birth_rate" && to == "births" {
                    found_rate_to_births = true;
                }
                if from == "population" && to == "births" {
                    found_pop_to_births = true;
                }
                if from == "births" && to == "population" {
                    found_births_to_pop = true;
                }

                // Without LTM, scores should be null
                assert!(link.score.is_null(), "Score should be null without LTM");
                assert_eq!(link.score_len, 0, "Score length should be 0 without LTM");
            }

            assert!(
                found_rate_to_births,
                "Should find birth_rate -> births link"
            );
            assert!(found_pop_to_births, "Should find population -> births link");
            assert!(found_births_to_pop, "Should find births -> population link");

            simlin_free_links(links);

            // Now test with LTM enabled
            // Create new sim with LTM enabled
            let mut err_get_model_ltm: *mut SimlinError = ptr::null_mut();
            let model_ltm = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model_ltm as *mut *mut SimlinError,
            );
            if !err_get_model_ltm.is_null() {
                simlin_error_free(err_get_model_ltm);
                panic!("get_model failed");
            }
            assert!(!model_ltm.is_null());
            err = ptr::null_mut();
            let sim_ltm = simlin_sim_new(model_ltm, true, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim_ltm.is_null());

            // Run simulation to generate score data
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim_ltm, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            // Get links with scores
            err = ptr::null_mut();
            let links_with_scores =
                simlin_analyze_get_links(sim_ltm, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!links_with_scores.is_null());
            assert!((*links_with_scores).count > 0);

            let links_slice =
                std::slice::from_raw_parts((*links_with_scores).links, (*links_with_scores).count);

            // Verify that scores are now populated
            for link in links_slice {
                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                // Links in the feedback loop should have scores
                if (from == "births" && to == "population")
                    || (from == "population" && to == "births")
                {
                    assert!(
                        !link.score.is_null(),
                        "Feedback loop links should have scores"
                    );
                    assert!(
                        link.score_len > 0,
                        "Score length should be > 0 for feedback links"
                    );

                    // Check that scores contain reasonable values
                    // Note: First timestep(s) will be NaN due to insufficient history for PREVIOUS()
                    // For flow-to-stock links, first 2 timesteps are NaN (need PREVIOUS(PREVIOUS))
                    // For other links, first timestep is NaN (need PREVIOUS)
                    let scores = std::slice::from_raw_parts(link.score, link.score_len);
                    let is_flow_to_stock = from == "births" && to == "population";
                    let skip_count = if is_flow_to_stock { 2 } else { 1 };

                    // Check first timestep(s) are NaN
                    for &score in scores.iter().take(skip_count.min(scores.len())) {
                        assert!(
                            score.is_nan(),
                            "Early timesteps should be NaN due to insufficient history"
                        );
                    }

                    // Check remaining scores are finite
                    for &score in &scores[skip_count..] {
                        assert!(
                            score.is_finite(),
                            "Score should be finite after initial timesteps"
                        );
                    }
                }
            }

            simlin_free_links(links_with_scores);

            // Clean up
            simlin_sim_unref(sim);
            simlin_sim_unref(sim_ltm);
            simlin_model_unref(model);
            simlin_model_unref(model_ltm);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_analyze_get_links_no_loops() {
        // Create a project with no feedback loops
        let test_project = TestProject::new("test_no_loops")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("input", "10", None)
            .aux("output", "input * 2", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            err = ptr::null_mut();
            let links = simlin_analyze_get_links(sim, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!links.is_null());

            // Should still find the causal link from input to output
            assert!((*links).count > 0, "Should find input -> output link");

            let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
            let mut found_link = false;
            for link in links_slice {
                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                if from == "input" && to == "output" {
                    found_link = true;
                    // Non-loop links will have Unknown polarity since we don't analyze them
                    assert_eq!(link.polarity, SimlinLinkPolarity::Unknown);
                }
            }
            assert!(found_link, "Should find input -> output link");

            simlin_free_links(links);
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_analyze_get_links_null_safety() {
        unsafe {
            // Test with null sim
            let mut err: *mut SimlinError = ptr::null_mut();
            let links =
                simlin_analyze_get_links(ptr::null_mut(), &mut err as *mut *mut SimlinError);
            assert!(links.is_null());

            // Test free with null (should not crash)
            simlin_free_links(ptr::null_mut());
        }
    }

    #[test]
    fn test_analyze_get_relative_loop_score_renamed() {
        // Create a project with a reinforcing loop
        let test_project = TestProject::new("test_renamed")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.02", None);

        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // Create simulation with LTM enabled

            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError); // Enable LTM for relative loop scores
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Run simulation
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            // Get loops to find loop ID
            err = ptr::null_mut();
            let loops = simlin_analyze_get_loops(proj, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!loops.is_null());
            assert!((*loops).count > 0);

            let loop_slice = std::slice::from_raw_parts((*loops).loops, (*loops).count);
            let loop_id = CStr::from_ptr(loop_slice[0].id).to_str().unwrap();

            // Test renamed function
            let mut step_count: usize = 0;
            err = ptr::null_mut();
            simlin_sim_get_stepcount(
                sim,
                &mut step_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            let mut scores = vec![0.0; step_count];

            let loop_id_c = CString::new(loop_id).unwrap();
            let mut written: usize = 0;
            err = ptr::null_mut();
            simlin_analyze_get_relative_loop_score(
                sim,
                loop_id_c.as_ptr(),
                scores.as_mut_ptr(),
                scores.len(),
                &mut written as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(
                err.is_null(),
                "Should successfully get relative loop scores"
            );
            assert_eq!(written, scores.len());

            // Verify scores are reasonable
            // Since there's only one loop, relative score should be 1.0
            for score in &scores {
                assert!(score.is_finite());
                assert_eq!(*score, 1.0, "Single loop should have relative score of 1.0");
            }

            simlin_free_loops(loops);
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_serialize() {
        // Create a project with some content
        let test_project = TestProject::new("test_serialize")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population * death_rate", None)
            .aux("birth_rate", "0.02", None)
            .aux("death_rate", "0.01", None);

        let datamodel_project = test_project.build_datamodel();
        let original_pb = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        original_pb.encode(&mut buf).unwrap();

        unsafe {
            // Open the project
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Serialize it back out
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_project_serialize(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(!output.is_null());
            assert!(output_len > 0);

            // Verify we can open the serialized project
            let proj2 = simlin_project_open(output, output_len, &mut err);
            assert!(!proj2.is_null());
            // Get models and create simulations from both projects and verify they work identically
            let mut err_get_model1: *mut SimlinError = ptr::null_mut();
            let model1 = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model1 as *mut *mut SimlinError,
            );
            if !err_get_model1.is_null() {
                simlin_error_free(err_get_model1);
                panic!("get_model failed");
            }
            err = ptr::null_mut();
            let model2 =
                simlin_project_get_model(proj2, ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(!model1.is_null());
            assert!(err.is_null());
            assert!(!model2.is_null());

            err = ptr::null_mut();
            let sim1 = simlin_sim_new(model1, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            err = ptr::null_mut();
            let sim2 = simlin_sim_new(model2, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim1.is_null());
            assert!(!sim2.is_null());

            // Run both simulations
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim1, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim2, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            // Check they have same number of variables and steps
            let mut var_count1: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model1,
                &mut var_count1 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            let mut var_count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model2,
                &mut var_count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(var_count1, var_count2);

            let mut step_count1: usize = 0;
            err = ptr::null_mut();
            simlin_sim_get_stepcount(
                sim1,
                &mut step_count1 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            let mut step_count2: usize = 0;
            err = ptr::null_mut();
            simlin_sim_get_stepcount(
                sim2,
                &mut step_count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(step_count1, step_count2);

            // Clean up
            simlin_free(output);
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_project_unref(proj);
            simlin_project_unref(proj2);
        }
    }

    #[test]
    fn test_project_serialize_with_ltm() {
        // Create a project with a loop
        let test_project = TestProject::new("test_serialize_ltm")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("stock", "100", &["inflow"], &[], None)
            .flow("inflow", "stock * 0.1", None);

        let datamodel_project = test_project.build_datamodel();
        let original_pb = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        original_pb.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // LTM will be enabled when creating simulation

            // Serialize the project (should NOT include LTM variables)
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_project_serialize(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            // Open the serialized project
            let proj2 = simlin_project_open(output, output_len, &mut err);
            assert!(!proj2.is_null());

            // Create sims from both
            let mut err_get_model1: *mut SimlinError = ptr::null_mut();
            let model1 = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model1 as *mut *mut SimlinError,
            );
            if !err_get_model1.is_null() {
                simlin_error_free(err_get_model1);
                panic!("get_model failed");
            }
            err = ptr::null_mut();
            let model2 =
                simlin_project_get_model(proj2, ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(!model1.is_null());
            assert!(err.is_null());
            assert!(!model2.is_null());

            err = ptr::null_mut();
            let sim1 = simlin_sim_new(model1, true, &mut err as *mut *mut SimlinError); // Has LTM
            assert!(err.is_null());
            err = ptr::null_mut();
            let sim2 = simlin_sim_new(model2, false, &mut err as *mut *mut SimlinError); // No LTM
            assert!(err.is_null());

            // Run both
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim1, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim2, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());

            // Both original models should have the same number of variables
            // (they're from the same serialized project without LTM augmentation)
            let mut var_count1: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model1,
                &mut var_count1 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            let mut var_count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model2,
                &mut var_count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(
                var_count1, var_count2,
                "Models from serialized projects should have same variable count"
            );

            // Clean up
            simlin_free(output);
            simlin_sim_unref(sim1);
            simlin_sim_unref(sim2);
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_project_unref(proj);
            simlin_project_unref(proj2);
        }
    }

    #[test]
    fn test_project_serialize_null_safety() {
        unsafe {
            // Test with null project
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_project_serialize(
                ptr::null_mut(),
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            simlin_error_free(err);
            assert!(output.is_null());

            // Test with null output pointer
            let project = engine::project_io::Project {
                name: "test".to_string(),
                sim_specs: Some(engine::project_io::SimSpecs {
                    start: 0.0,
                    stop: 10.0,
                    dt: Some(engine::project_io::Dt {
                        value: 1.0,
                        is_reciprocal: false,
                    }),
                    save_step: None,
                    sim_method: engine::project_io::SimMethod::Euler as i32,
                    time_units: None,
                }),
                models: vec![engine::project_io::Model {
                    name: "main".to_string(),
                    variables: vec![],
                    views: vec![],
                    loop_metadata: vec![],
                }],
                dimensions: vec![],
                units: vec![],
                source: None,
            };
            let mut buf = Vec::new();
            project.encode(&mut buf).unwrap();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            err = ptr::null_mut();
            simlin_project_serialize(
                proj,
                ptr::null_mut(),
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            simlin_error_free(err);
            // Test with null output_len pointer
            err = ptr::null_mut();
            simlin_project_serialize(
                proj,
                &mut output as *mut *mut u8,
                ptr::null_mut(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            simlin_error_free(err);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_model_functions() {
        // Create a project with multiple models
        let project = engine::project_io::Project {
            name: "test_multi_model".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: Some(engine::project_io::Dt {
                    value: 1.0,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![
                engine::project_io::Model {
                    name: "model1".to_string(),
                    variables: vec![
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Aux(
                                engine::project_io::variable::Aux {
                                    ident: "var1".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "1".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Aux(
                                engine::project_io::variable::Aux {
                                    ident: "var2".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "var1 * 2".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                },
                engine::project_io::Model {
                    name: "model2".to_string(),
                    variables: vec![
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Stock(
                                engine::project_io::variable::Stock {
                                    ident: "stock".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "100".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    inflows: vec!["inflow".to_string()],
                                    outflows: vec![],
                                    non_negative: false,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Flow(
                                engine::project_io::variable::Flow {
                                    ident: "inflow".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "rate * stock".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    non_negative: false,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                        engine::project_io::Variable {
                            v: Some(engine::project_io::variable::V::Aux(
                                engine::project_io::variable::Aux {
                                    ident: "rate".to_string(),
                                    equation: Some(engine::project_io::variable::Equation {
                                        equation: Some(
                                            engine::project_io::variable::equation::Equation::Scalar(
                                                engine::project_io::variable::ScalarEquation {
                                                    equation: "0.1".to_string(),
                                                    initial_equation: None,
                                                },
                                            ),
                                        ),
                                    }),
                                    documentation: String::new(),
                                    units: String::new(),
                                    gf: None,
                                    can_be_module_input: false,
                                    visibility: engine::project_io::variable::Visibility::Private as i32,
                                    uid: 0,
                                },
                            )),
                        },
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                },
            ],
            dimensions: vec![],
            units: vec![],
            source: None,
        };

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Test simlin_project_get_model_count
            let mut model_count: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_count(
                proj,
                &mut model_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(model_count, 2, "Should have 2 models");

            // Test simlin_project_get_model_names
            let mut model_names: Vec<*mut c_char> = vec![ptr::null_mut(); 2];
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_names(
                proj,
                model_names.as_mut_ptr(),
                2,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(count, 2);

            let mut names = Vec::new();
            for name_ptr in &model_names {
                assert!(!name_ptr.is_null());
                let name = CStr::from_ptr(*name_ptr).to_string_lossy().into_owned();
                names.push(name.clone());
                simlin_free_string(*name_ptr);
            }
            assert!(names.contains(&"model1".to_string()));
            assert!(names.contains(&"model2".to_string()));

            // Test simlin_project_get_model with specific name
            let model1_name = CString::new("model1").unwrap();
            err = ptr::null_mut();
            let model1 = simlin_project_get_model(
                proj,
                model1_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(!model1.is_null());
            assert!(err.is_null());
            assert_eq!((*model1).model_name.as_str(), "model1");

            // Test simlin_project_get_model with null (should get first model)
            let mut err_get_model_default: *mut SimlinError = ptr::null_mut();
            let model_default = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model_default as *mut *mut SimlinError,
            );
            if !err_get_model_default.is_null() {
                simlin_error_free(err_get_model_default);
                panic!("get_model failed");
            }
            assert!(!model_default.is_null());
            assert_eq!((*model_default).model_name.as_str(), "model1");

            // Test simlin_project_get_model with non-existent name (should get first model)
            let bad_name = CString::new("nonexistent").unwrap();
            err = ptr::null_mut();
            let model_fallback = simlin_project_get_model(
                proj,
                bad_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(!model_fallback.is_null());
            assert!(err.is_null());
            assert_eq!((*model_fallback).model_name.as_str(), "model1");

            // Test simlin_model_get_var_count
            let model2_name = CString::new("model2").unwrap();
            err = ptr::null_mut();
            let model2 = simlin_project_get_model(
                proj,
                model2_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(!model2.is_null());
            assert!(err.is_null());

            let mut var_count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model2,
                &mut var_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(
                var_count >= 3,
                "model2 should have at least 3 variables (stock, inflow, rate)"
            );

            // Test simlin_model_get_var_names
            let mut var_names: Vec<*mut c_char> = vec![ptr::null_mut(); var_count];
            let mut written: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_names(
                model2,
                var_names.as_mut_ptr(),
                var_count,
                &mut written as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(written, var_count);

            let mut var_name_strings = Vec::new();
            for name_ptr in &var_names {
                assert!(!name_ptr.is_null());
                let name = CStr::from_ptr(*name_ptr).to_string_lossy().into_owned();
                var_name_strings.push(name.clone());
                simlin_free_string(*name_ptr);
            }
            assert!(var_name_strings.contains(&"stock".to_string()));
            assert!(var_name_strings.contains(&"inflow".to_string()));
            assert!(var_name_strings.contains(&"rate".to_string()));
            // time may or may not be included depending on compilation

            // Test simlin_model_get_links
            err = ptr::null_mut();
            let links = simlin_model_get_links(model2, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!links.is_null());
            assert!((*links).count > 0, "Should have causal links");

            // Verify link structure
            let links_slice = std::slice::from_raw_parts((*links).links, (*links).count);
            let mut found_rate_to_inflow = false;
            let mut found_stock_to_inflow = false;
            let mut found_inflow_to_stock = false;

            for link in links_slice {
                assert!(!link.from.is_null());
                assert!(!link.to.is_null());

                let from = CStr::from_ptr(link.from).to_str().unwrap();
                let to = CStr::from_ptr(link.to).to_str().unwrap();

                if from == "rate" && to == "inflow" {
                    found_rate_to_inflow = true;
                }
                if from == "stock" && to == "inflow" {
                    found_stock_to_inflow = true;
                }
                if from == "inflow" && to == "stock" {
                    found_inflow_to_stock = true;
                }

                // Model-level links should not have scores
                assert!(link.score.is_null());
                assert_eq!(link.score_len, 0);
            }

            assert!(found_rate_to_inflow, "Should find rate -> inflow link");
            assert!(found_stock_to_inflow, "Should find stock -> inflow link");
            assert!(found_inflow_to_stock, "Should find inflow -> stock link");

            simlin_free_links(links);

            // Clean up
            simlin_model_unref(model1);
            simlin_model_unref(model2);
            simlin_model_unref(model_default);
            simlin_model_unref(model_fallback);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_model_null_safety() {
        unsafe {
            // Test null project
            let mut count: usize = 0;
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_project_get_model_count(
                ptr::null_mut(),
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle null gracefully

            let mut names: [*mut c_char; 2] = [ptr::null_mut(); 2];
            let _written: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_names(
                ptr::null_mut(),
                names.as_mut_ptr(),
                2,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle null gracefully

            err = ptr::null_mut();
            let model = simlin_project_get_model(
                ptr::null_mut(),
                ptr::null(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(model.is_null());
            // err might be set for null input

            // Test null model
            simlin_model_ref(ptr::null_mut());
            simlin_model_unref(ptr::null_mut());

            count = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                ptr::null_mut(),
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle null gracefully

            let mut var_names: [*mut c_char; 2] = [ptr::null_mut(); 2];
            err = ptr::null_mut();
            simlin_model_get_var_names(
                ptr::null_mut(),
                var_names.as_mut_ptr(),
                2,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle null gracefully

            err = ptr::null_mut();
            let links = simlin_model_get_links(ptr::null_mut(), &mut err as *mut *mut SimlinError);
            assert!(links.is_null());

            // Test null sim creation - should return error for NULL model
            err = ptr::null_mut();
            let sim = simlin_sim_new(ptr::null_mut(), false, &mut err as *mut *mut SimlinError);
            assert!(!err.is_null(), "Expected error for NULL model");
            assert!(sim.is_null());
            simlin_error_free(err);
        }
    }

    #[test]
    fn test_ltm_enabled_sim() {
        // Create a project with a feedback loop
        let test_project = TestProject::new("test_ltm")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.02", None);

        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());

            // Create simulation with LTM enabled
            err = ptr::null_mut();
            let sim_ltm = simlin_sim_new(model, true, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim_ltm.is_null());

            // Run simulation
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim_ltm, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            // Create another sim without LTM
            err = ptr::null_mut();
            let sim_no_ltm = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim_no_ltm.is_null());

            // Run this one too
            err = ptr::null_mut();
            simlin_sim_run_to_end(sim_no_ltm, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            // Clean up
            simlin_sim_unref(sim_ltm);
            simlin_sim_unref(sim_no_ltm);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_get_incoming_links() {
        // Create a project with a flow that depends on a rate and a stock using TestProject
        let test_project = TestProject::new("test")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("Stock", "100", &["flow"], &[], None)
            .flow("flow", "rate * Stock", None)
            .aux("rate", "0.5", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Test getting incoming links for the flow
            let flow_name = CString::new("flow").unwrap();

            // Test 1: Query the number of dependencies with max=0
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                ptr::null_mut(), // result can be null when max=0
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(count, 2, "Expected 2 dependencies for flow when querying");

            // Test 2: Try with insufficient array size (should return error)
            let mut small_links: [*mut c_char; 1] = [ptr::null_mut(); 1];
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                small_links.as_mut_ptr(),
                1, // Only room for 1, but there are 2 dependencies
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected error when array too small");
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            // Test 3: Proper usage - query then allocate
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(count, 2);

            // Allocate exact size needed
            let mut links = vec![ptr::null_mut::<c_char>(); count];
            let mut count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                links.as_mut_ptr(),
                count,
                &mut count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(
                count2, count,
                "Should return same count when array is exact size"
            );

            // Collect the dependency names
            let mut dep_names = Vec::new();
            for link in links.iter().take(count2) {
                assert!(!link.is_null());
                let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
                dep_names.push(dep_name);
                simlin_free_string(*link);
            }

            // Check that we got both "rate" and "stock" (canonicalized to lowercase)
            assert!(
                dep_names.contains(&"rate".to_string()),
                "Missing 'rate' dependency"
            );
            assert!(
                dep_names.contains(&"stock".to_string()),
                "Missing 'stock' dependency"
            );

            // Test getting incoming links for rate (should have none since it's a constant)
            let rate_name = CString::new("rate").unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                rate_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle error gracefully
            assert_eq!(count, 0, "Expected 0 dependencies for rate");

            // Test getting incoming links for stock (initial value is constant, so no deps)
            let stock_name = CString::new("Stock").unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                stock_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle error gracefully
            assert_eq!(
                count, 0,
                "Expected 0 dependencies for Stock's initial value"
            );

            // Test error cases
            // Non-existent variable
            let nonexistent = CString::new("nonexistent").unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                nonexistent.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected error for non-existent variable");
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::DoesNotExist);
            simlin_error_free(err);

            // Null pointer checks
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                ptr::null_mut(),
                flow_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected error for null model");
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                ptr::null(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null(), "Expected error for null var_name");
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            // Test that result being null with max > 0 is an error
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                flow_name.as_ptr(),
                ptr::null_mut(),
                10,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(
                !err.is_null(),
                "Expected error for null result with max > 0"
            );
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_get_incoming_links_with_private_variables() {
        // Test that private variables (starting with $⁚) are not exposed in incoming links
        // Create a model with a SMOOTH function which internally creates private variables
        let test_project = TestProject::new("test")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("input", "10", None)
            .aux("smooth_time", "3", None)
            // SMTH1 creates internal private variables like $⁚smoothed⁚0⁚smth1⁚output
            .aux("smoothed", "SMTH1(input, smooth_time)", None)
            // A variable that depends on the smoothed output
            .aux("result", "smoothed * 2", None);

        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());

            // Test getting incoming links for 'smoothed' variable
            // It should show 'input' and 'smooth_time' as dependencies,
            // but NOT any private variables like $⁚smoothed⁚0⁚smooth⁚output
            let smoothed_name = CString::new("smoothed").unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                smoothed_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle error gracefully

            // Get the actual dependencies
            let mut links = vec![ptr::null_mut::<c_char>(); count];
            let mut count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                smoothed_name.as_ptr(),
                links.as_mut_ptr(),
                count,
                &mut count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(count2, count);

            // Collect dependency names
            let mut dep_names = Vec::new();
            for link in links.iter().take(count2) {
                assert!(!link.is_null());
                let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
                dep_names.push(dep_name.clone());

                simlin_free_string(*link);
            }

            // Assert that no private variable is exposed
            for dep_name in &dep_names {
                assert!(
                    !dep_name.starts_with("$"),
                    "Private variable '{}' should not be exposed in incoming links",
                    dep_name
                );
            }

            // Should have input and smooth_time as dependencies
            assert!(
                dep_names.contains(&"input".to_string()),
                "Missing 'input' dependency"
            );
            assert!(
                dep_names.contains(&"smooth_time".to_string()),
                "Missing 'smooth_time' dependency"
            );

            // Clean up
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_get_incoming_links_nested_private_vars() {
        // Test that nested private variables are resolved transitively
        // Create a model with chained SMTH1 functions which create nested private variables
        let test_project = TestProject::new("test")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("base_input", "TIME", None)
            .aux("delay1", "2", None)
            .aux("delay2", "3", None)
            // First smoothing creates private variables
            .aux("smooth1", "SMTH1(base_input, delay1)", None)
            // Second smoothing uses first, creating more private variables
            .aux("smooth2", "SMTH1(smooth1, delay2)", None)
            // Final result uses the second smoothed value
            .aux("final_output", "smooth2 * 1.5", None);

        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());

            // Test smooth1 dependencies - should resolve to base_input and delay1
            let smooth1_name = CString::new("smooth1").unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                smooth1_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle error gracefully

            let mut links = vec![ptr::null_mut::<c_char>(); count];
            let mut count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                smooth1_name.as_ptr(),
                links.as_mut_ptr(),
                count,
                &mut count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());

            let mut smooth1_deps = Vec::new();
            for link in links.iter().take(count2) {
                let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
                smooth1_deps.push(dep_name.clone());
                assert!(
                    !dep_name.starts_with("$"),
                    "No private vars in smooth1 deps"
                );
                simlin_free_string(*link);
            }

            assert!(smooth1_deps.contains(&"base_input".to_string()));
            assert!(smooth1_deps.contains(&"delay1".to_string()));

            // Test smooth2 dependencies - should transitively resolve through smooth1's private vars
            let smooth2_name = CString::new("smooth2").unwrap();
            let mut count: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                smooth2_name.as_ptr(),
                ptr::null_mut(),
                0,
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle error gracefully

            let mut links = vec![ptr::null_mut::<c_char>(); count];
            let mut count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_incoming_links(
                model,
                smooth2_name.as_ptr(),
                links.as_mut_ptr(),
                count,
                &mut count2 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());

            let mut smooth2_deps = Vec::new();
            for link in links.iter().take(count2) {
                let dep_name = CStr::from_ptr(*link).to_string_lossy().into_owned();
                smooth2_deps.push(dep_name.clone());
                assert!(
                    !dep_name.starts_with("$"),
                    "No private vars in smooth2 deps"
                );
                simlin_free_string(*link);
            }

            // smooth2 depends on smooth1's module output, which transitively depends on
            // base_input, delay1, plus smooth2's own delay2
            assert!(
                smooth2_deps.contains(&"smooth1".to_string()),
                "Should depend on smooth1"
            );
            assert!(
                smooth2_deps.contains(&"delay2".to_string()),
                "Should depend on delay2"
            );

            // Clean up
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_add_model() {
        use prost::Message;

        // Create a minimal project with just one model
        let project = engine::project_io::Project {
            name: "test_project".to_string(),
            sim_specs: Some(engine::project_io::SimSpecs {
                start: 0.0,
                stop: 100.0,
                dt: Some(engine::project_io::Dt {
                    value: 0.25,
                    is_reciprocal: false,
                }),
                save_step: None,
                sim_method: engine::project_io::SimMethod::Euler as i32,
                time_units: None,
            }),
            models: vec![engine::project_io::Model {
                name: "main".to_string(),
                variables: vec![],
                views: vec![],
                loop_metadata: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            // Open the project
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_open(buf.as_ptr(), buf.len(), &mut err as *mut *mut SimlinError);
            if !err.is_null() {
                let code = simlin_error_get_code(err);
                let msg_ptr = simlin_error_get_message(err);
                let msg = if msg_ptr.is_null() {
                    ""
                } else {
                    CStr::from_ptr(msg_ptr).to_str().unwrap()
                };
                simlin_error_free(err);
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Verify initial model count
            let mut initial_count: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_count(
                proj,
                &mut initial_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(initial_count, 1);

            // Test adding a model
            let model_name = CString::new("new_model").unwrap();
            err = ptr::null_mut();
            simlin_project_add_model(proj, model_name.as_ptr(), &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            // Verify model count increased
            let mut new_count: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_count(
                proj,
                &mut new_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(new_count, 2);

            // Verify we can get the new model
            err = ptr::null_mut();
            let new_model = simlin_project_get_model(
                proj,
                model_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(!new_model.is_null());
            assert!(err.is_null());
            assert_eq!((*new_model).model_name.as_str(), "new_model");

            // Verify the new model can be used to create a simulation
            err = ptr::null_mut();
            let sim = simlin_sim_new(new_model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Clean up
            simlin_sim_unref(sim);
            simlin_model_unref(new_model);

            // Test adding another model
            let model_name2 = CString::new("another_model").unwrap();
            err = ptr::null_mut();
            simlin_project_add_model(
                proj,
                model_name2.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            // Verify model count
            let mut final_count: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_count(
                proj,
                &mut final_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(final_count, 3);

            // Test adding duplicate model name (should fail)
            let duplicate_name = CString::new("new_model").unwrap();
            err = ptr::null_mut();
            simlin_project_add_model(
                proj,
                duplicate_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(
                !err.is_null(),
                "Expected error when adding duplicate model name"
            );
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::DuplicateVariable);
            simlin_error_free(err);

            // Model count should not have changed
            let mut count_after_dup: usize = 0;
            err = ptr::null_mut();
            simlin_project_get_model_count(
                proj,
                &mut count_after_dup as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(count_after_dup, 3);

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_add_model_null_safety() {
        unsafe {
            // Test with null project
            let model_name = CString::new("test").unwrap();
            let mut err: *mut SimlinError = ptr::null_mut();
            simlin_project_add_model(
                ptr::null_mut(),
                model_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(
                !err.is_null(),
                "Expected error when adding model to null project"
            );
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            // Create a valid project for other null tests
            let project = engine::project_io::Project {
                name: "test".to_string(),
                sim_specs: Some(engine::project_io::SimSpecs {
                    start: 0.0,
                    stop: 10.0,
                    dt: Some(engine::project_io::Dt {
                        value: 1.0,
                        is_reciprocal: false,
                    }),
                    save_step: None,
                    sim_method: engine::project_io::SimMethod::Euler as i32,
                    time_units: None,
                }),
                models: vec![],
                dimensions: vec![],
                units: vec![],
                source: None,
            };
            let mut buf = Vec::new();
            project.encode(&mut buf).unwrap();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // Test with null model name
            err = ptr::null_mut();
            simlin_project_add_model(proj, ptr::null(), &mut err as *mut *mut SimlinError);
            assert!(
                !err.is_null(),
                "Expected error when adding model with null name"
            );
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            // Test with empty model name
            let empty_name = CString::new("").unwrap();
            err = ptr::null_mut();
            simlin_project_add_model(proj, empty_name.as_ptr(), &mut err as *mut *mut SimlinError);
            assert!(
                !err.is_null(),
                "Expected error when adding model with empty name"
            );
            let code = simlin_error_get_code(err);
            assert_eq!(code, SimlinErrorCode::Generic);
            simlin_error_free(err);

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_json_open() {
        let json_str = r#"{
            "name": "test_json_project",
            "sim_specs": {
                "start_time": 0.0,
                "end_time": 10.0,
                "dt": "1",
                "save_step": 1.0,
                "method": "euler",
                "time_units": "days"
            },
            "models": [{
                "name": "main",
                "stocks": [{
                    "uid": 1,
                    "name": "population",
                    "initial_equation": "100",
                    "inflows": [],
                    "outflows": [],
                    "units": "people",
                    "documentation": "",
                    "can_be_module_input": false,
                    "is_public": false,
                    "dimensions": []
                }],
                "flows": [],
                "auxiliaries": [{
                    "uid": 2,
                    "name": "growth_rate",
                    "equation": "0.1",
                    "units": "",
                    "documentation": "",
                    "can_be_module_input": false,
                    "is_public": false,
                    "dimensions": []
                }],
                "modules": [],
                "sim_specs": {
                    "start_time": 0.0,
                    "end_time": 10.0,
                    "dt": "1",
                    "save_step": 1.0,
                    "method": "",
                    "time_units": ""
                },
                "views": []
            }],
            "dimensions": [],
            "units": []
        }"#;

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let json_bytes = json_str.as_bytes();
            let proj = simlin_project_json_open(
                json_bytes.as_ptr(),
                json_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                &mut err,
            );

            assert!(!proj.is_null(), "project open failed");
            // Verify we can get the model
            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());

            // Verify variable count
            let mut var_count: usize = 0;
            let mut err_get_var_count: *mut SimlinError = ptr::null_mut();
            simlin_model_get_var_count(
                model,
                &mut var_count as *mut usize,
                &mut err_get_var_count as *mut *mut SimlinError,
            );
            if !err_get_var_count.is_null() {
                simlin_error_free(err_get_var_count);
                panic!("get_var_count failed");
            }
            assert!(var_count > 0, "expected variables in model");

            // Clean up
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_json_open_invalid_json() {
        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let invalid_json = b"not valid json {";
            let proj = simlin_project_json_open(
                invalid_json.as_ptr(),
                invalid_json.len(),
                ffi::SimlinJsonFormat::Native,
                &mut err,
            );

            assert!(proj.is_null(), "expected null project for invalid JSON");
            // assert_ne!(err, engine::ErrorCode::NoError as c_int);  // Obsolete assertion from old API
        }
    }

    #[test]
    fn test_project_json_open_null_input() {
        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj =
                simlin_project_json_open(ptr::null(), 0, ffi::SimlinJsonFormat::Native, &mut err);

            assert!(proj.is_null());
            // assert_eq!(err, engine::ErrorCode::Generic as c_int);  // Obsolete assertion from old API
        }
    }

    #[test]
    fn test_project_json_open_logistic_growth() {
        let json_bytes = include_bytes!("../../../test/logistic-growth.sd.json");

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_json_open(
                json_bytes.as_ptr(),
                json_bytes.len(),
                ffi::SimlinJsonFormat::Native,
                &mut err,
            );

            assert!(!proj.is_null(), "project open failed");
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_json_open_sdai_format() {
        let json_str = r#"{
            "variables": [
                {
                    "type": "stock",
                    "name": "inventory",
                    "equation": "50",
                    "units": "widgets",
                    "inflows": ["production"],
                    "outflows": ["sales"]
                },
                {
                    "type": "flow",
                    "name": "production",
                    "equation": "10",
                    "units": "widgets/month"
                },
                {
                    "type": "flow",
                    "name": "sales",
                    "equation": "8",
                    "units": "widgets/month"
                },
                {
                    "type": "variable",
                    "name": "target_inventory",
                    "equation": "100",
                    "units": "widgets"
                }
            ],
            "specs": {
                "startTime": 0.0,
                "stopTime": 10.0,
                "dt": 1.0,
                "timeUnits": "months"
            }
        }"#;

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let json_bytes = json_str.as_bytes();
            let proj = simlin_project_json_open(
                json_bytes.as_ptr(),
                json_bytes.len(),
                ffi::SimlinJsonFormat::Sdai,
                &mut err,
            );

            assert!(!proj.is_null(), "project open failed");
            // Verify we can get the model
            let mut err_get_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_get_model as *mut *mut SimlinError,
            );
            if !err_get_model.is_null() {
                simlin_error_free(err_get_model);
                panic!("get_model failed");
            }
            assert!(!model.is_null());

            // Verify variable count (at least 4 variables, may include built-ins)
            let mut var_count: usize = 0;
            let mut err_get_var_count: *mut SimlinError = ptr::null_mut();
            simlin_model_get_var_count(
                model,
                &mut var_count as *mut usize,
                &mut err_get_var_count as *mut *mut SimlinError,
            );
            if !err_get_var_count.is_null() {
                simlin_error_free(err_get_var_count);
                panic!("get_var_count failed");
            }
            assert!(
                var_count >= 4,
                "expected at least 4 variables, got {}",
                var_count
            );

            // Clean up
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_json_open_sdai_invalid() {
        let invalid_sdai = r#"{
            "variables": [
                {
                    "type": "invalid_type",
                    "name": "test"
                }
            ]
        }"#;

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let json_bytes = invalid_sdai.as_bytes();
            let proj = simlin_project_json_open(
                json_bytes.as_ptr(),
                json_bytes.len(),
                ffi::SimlinJsonFormat::Sdai,
                &mut err,
            );

            assert!(
                proj.is_null(),
                "expected null project for invalid SDAI JSON"
            );
            // assert_ne!(err, engine::ErrorCode::NoError as c_int);  // Obsolete assertion from old API
        }
    }

    #[test]
    fn test_concurrent_project_ref_unref() {
        use std::thread;

        unsafe {
            // Create a test project
            let datamodel = TestProject::new("concurrent_test").build_datamodel();
            let pb_project = engine_serde::serialize(&datamodel);
            let encoded = pb_project.encode_to_vec();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(
                encoded.as_ptr(),
                encoded.len(),
                &mut err as *mut *mut SimlinError,
            );

            if !err.is_null() {
                simlin_error_free(err);
                panic!("failed to create project");
            }
            assert!(!proj.is_null());

            // Add many references from multiple threads
            const NUM_THREADS: usize = 10;
            const REFS_PER_THREAD: usize = 100;

            let mut handles = vec![];

            // Spawn threads that will add and remove references
            for _ in 0..NUM_THREADS {
                // Cast to usize to make it Send
                let proj_addr = proj as usize;
                let handle = thread::spawn(move || {
                    let proj_ptr = proj_addr as *mut SimlinProject;
                    for _ in 0..REFS_PER_THREAD {
                        simlin_project_ref(proj_ptr);
                        simlin_project_unref(proj_ptr);
                    }
                });
                handles.push(handle);
            }

            // Wait for all threads
            for handle in handles {
                handle.join().unwrap();
            }

            // Reference count should be back to 1
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_concurrent_model_creation() {
        use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
        use std::sync::Arc;
        use std::thread;

        unsafe {
            // Create a test project
            let datamodel = TestProject::new("concurrent_model").build_datamodel();
            let pb_project = engine_serde::serialize(&datamodel);
            let encoded = pb_project.encode_to_vec();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(
                encoded.as_ptr(),
                encoded.len(),
                &mut err as *mut *mut SimlinError,
            );

            if !err.is_null() {
                simlin_error_free(err);
                panic!("failed to create project");
            }
            assert!(!proj.is_null());

            const NUM_THREADS: usize = 8;
            let success_count = Arc::new(AtomicUsize::new(0));
            let mut handles = vec![];

            // Spawn threads that create and destroy models
            for _ in 0..NUM_THREADS {
                let proj_addr = proj as usize;
                let success = Arc::clone(&success_count);
                let handle = thread::spawn(move || {
                    let proj_ptr = proj_addr as *mut SimlinProject;
                    for _ in 0..10 {
                        let mut err: *mut SimlinError = ptr::null_mut();
                        let model = simlin_project_get_model(
                            proj_ptr,
                            ptr::null(),
                            &mut err as *mut *mut SimlinError,
                        );

                        if !err.is_null() {
                            simlin_error_free(err);
                            continue;
                        }

                        if model.is_null() {
                            continue;
                        }

                        success.fetch_add(1, AtomicOrdering::SeqCst);

                        // Use the model briefly
                        let mut var_count: usize = 0;
                        let mut err_count: *mut SimlinError = ptr::null_mut();
                        simlin_model_get_var_count(
                            model,
                            &mut var_count as *mut usize,
                            &mut err_count as *mut *mut SimlinError,
                        );
                        if !err_count.is_null() {
                            simlin_error_free(err_count);
                        }

                        simlin_model_unref(model);
                    }
                });
                handles.push(handle);
            }

            // Wait for all threads
            for handle in handles {
                handle.join().unwrap();
            }

            // Should have had successful model creations
            assert!(success_count.load(AtomicOrdering::SeqCst) > 0);

            // Clean up
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_concurrent_sim_operations() {
        use std::thread;

        unsafe {
            // Create a test project with a simple model
            let datamodel = TestProject::new("concurrent_sim")
                .stock("inventory", "0", &[], &[], None)
                .flow("production", "5", None)
                .build_datamodel();
            let pb_project = engine_serde::serialize(&datamodel);
            let encoded = pb_project.encode_to_vec();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(
                encoded.as_ptr(),
                encoded.len(),
                &mut err as *mut *mut SimlinError,
            );

            if !err.is_null() {
                simlin_error_free(err);
                panic!("failed to create project");
            }
            assert!(!proj.is_null());

            // Get model
            let mut err_model: *mut SimlinError = ptr::null_mut();
            let model = simlin_project_get_model(
                proj,
                ptr::null(),
                &mut err_model as *mut *mut SimlinError,
            );
            if !err_model.is_null() {
                simlin_error_free(err_model);
                panic!("failed to get model");
            }

            const NUM_THREADS: usize = 5;
            let mut handles = vec![];

            // Spawn threads that create and run simulations
            for _ in 0..NUM_THREADS {
                let model_addr = model as usize;
                let handle = thread::spawn(move || {
                    let model_ptr = model_addr as *mut SimlinModel;
                    for _ in 0..5 {
                        let mut err_sim: *mut SimlinError = ptr::null_mut();
                        let sim =
                            simlin_sim_new(model_ptr, false, &mut err_sim as *mut *mut SimlinError);

                        if !err_sim.is_null() {
                            simlin_error_free(err_sim);
                            continue;
                        }

                        if sim.is_null() {
                            continue;
                        }

                        // Run simulation
                        let mut err_run: *mut SimlinError = ptr::null_mut();
                        simlin_sim_run_to_end(sim, &mut err_run as *mut *mut SimlinError);
                        if !err_run.is_null() {
                            simlin_error_free(err_run);
                        }

                        simlin_sim_unref(sim);
                    }
                });
                handles.push(handle);
            }

            // Wait for all threads
            for handle in handles {
                handle.join().unwrap();
            }

            // Clean up
            simlin_model_unref(model);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_stress_ref_counting() {
        use std::sync::Arc;
        use std::sync::Barrier;
        use std::thread;

        unsafe {
            // Create a test project
            let datamodel = TestProject::new("stress_test")
                .stock("s", "10", &[], &[], None)
                .build_datamodel();
            let pb_project = engine_serde::serialize(&datamodel);
            let encoded = pb_project.encode_to_vec();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open(
                encoded.as_ptr(),
                encoded.len(),
                &mut err as *mut *mut SimlinError,
            );

            if !err.is_null() {
                simlin_error_free(err);
                panic!("failed to create project");
            }
            assert!(!proj.is_null());

            const NUM_THREADS: usize = 20;
            const ITERATIONS: usize = 50;
            let barrier = Arc::new(Barrier::new(NUM_THREADS));
            let mut handles = vec![];

            // Spawn threads that stress test the ref counting
            for thread_id in 0..NUM_THREADS {
                let proj_addr = proj as usize;
                let barrier = Arc::clone(&barrier);
                let handle = thread::spawn(move || {
                    // Wait for all threads to be ready
                    barrier.wait();

                    let proj_ptr = proj_addr as *mut SimlinProject;
                    for _ in 0..ITERATIONS {
                        // Create model
                        let mut err_model: *mut SimlinError = ptr::null_mut();
                        let model = simlin_project_get_model(
                            proj_ptr,
                            ptr::null(),
                            &mut err_model as *mut *mut SimlinError,
                        );

                        if !err_model.is_null() {
                            simlin_error_free(err_model);
                            continue;
                        }

                        if model.is_null() {
                            continue;
                        }

                        // Ref and unref the model multiple times
                        for _ in 0..5 {
                            simlin_model_ref(model);
                        }
                        for _ in 0..5 {
                            simlin_model_unref(model);
                        }

                        // Create sim on every other iteration
                        if thread_id % 2 == 0 {
                            let mut err_sim: *mut SimlinError = ptr::null_mut();
                            let sim =
                                simlin_sim_new(model, false, &mut err_sim as *mut *mut SimlinError);

                            if !err_sim.is_null() {
                                simlin_error_free(err_sim);
                            } else if !sim.is_null() {
                                simlin_sim_unref(sim);
                            }
                        }

                        simlin_model_unref(model);
                    }
                });
                handles.push(handle);
            }

            // Wait for all threads
            for handle in handles {
                handle.join().unwrap();
            }

            // Final ref count should be 1
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

            // Clean up
            simlin_project_unref(proj);
        }
    }
}
