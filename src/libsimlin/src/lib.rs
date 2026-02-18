// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! C FFI library wrapping simlin-engine.
//!
//! This crate exposes the simulation engine to C, Go, and WASM consumers
//! through a set of `extern "C"` functions.  The public surface is split
//! across several focused modules:
//!
//! | Module          | Responsibility                                      |
//! |-----------------|-----------------------------------------------------|
//! | `memory`        | `simlin_malloc`, `simlin_free`, `simlin_free_string` |
//! | `error_api`     | Error inspection helpers (code, message, details)   |
//! | `project`       | Project lifecycle (open, ref/unref, query models)   |
//! | `model`         | Model queries (variables, links, LaTeX equations)   |
//! | `simulation`    | Simulation lifecycle (create, run, set values, reset) |
//! | `serialization` | Serialize to protobuf, JSON, XMILE, SVG             |
//! | `analysis`      | Feedback-loop / causal-link analysis, LTM scores    |
//! | `patch`         | JSON patch application and error collection          |
//!
//! Shared types (enums, structs, helpers) live here in `lib.rs` and are
//! imported by the modules via `crate::`.

use anyhow::{Error as AnyError, Result};
use simlin_engine::{self as engine};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

// These imports are used by the test module via `use super::*`.
#[cfg(test)]
use prost::Message;
#[cfg(test)]
use simlin_engine::serde as engine_serde;
#[cfg(test)]
use std::ffi::CStr;
#[cfg(test)]
use std::mem::align_of;
#[cfg(test)]
use std::os::raw::c_double;
#[cfg(test)]
use std::sync::atomic::Ordering;

// ── internal modules ───────────────────────────────────────────────────
pub mod errors;
mod ffi;
mod ffi_error;

mod analysis;
mod error_api;
mod memory;
mod model;
mod patch;
pub mod project;
mod serialization;
mod simulation;

// ── re-exports ─────────────────────────────────────────────────────────
// Re-export every `#[no_mangle] pub extern "C"` function so that the
// final cdylib / staticlib symbol table is complete, and so that
// `use super::*;` in the test module keeps working.

pub use analysis::*;
pub use error_api::*;
pub use memory::*;
pub use model::*;
pub use patch::simlin_project_apply_patch;
pub use project::*;
pub use serialization::*;
pub use simulation::*;

pub use ffi::{
    SimlinLink, SimlinLinkPolarity, SimlinLinks, SimlinLoop, SimlinLoopPolarity, SimlinLoops,
};
pub use ffi_error::{ErrorDetail as ErrorDetailData, FfiError, SimlinError};

// ── shared types ───────────────────────────────────────────────────────

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
    UnitMismatch = 33,
    BadOverride = 34,
}

impl TryFrom<u32> for SimlinErrorCode {
    type Error = ();

    fn try_from(value: u32) -> std::result::Result<Self, Self::Error> {
        match value {
            0 => Ok(SimlinErrorCode::NoError),
            1 => Ok(SimlinErrorCode::DoesNotExist),
            2 => Ok(SimlinErrorCode::XmlDeserialization),
            3 => Ok(SimlinErrorCode::VensimConversion),
            4 => Ok(SimlinErrorCode::ProtobufDecode),
            5 => Ok(SimlinErrorCode::InvalidToken),
            6 => Ok(SimlinErrorCode::UnrecognizedEof),
            7 => Ok(SimlinErrorCode::UnrecognizedToken),
            8 => Ok(SimlinErrorCode::ExtraToken),
            9 => Ok(SimlinErrorCode::UnclosedComment),
            10 => Ok(SimlinErrorCode::UnclosedQuotedIdent),
            11 => Ok(SimlinErrorCode::ExpectedNumber),
            12 => Ok(SimlinErrorCode::UnknownBuiltin),
            13 => Ok(SimlinErrorCode::BadBuiltinArgs),
            14 => Ok(SimlinErrorCode::EmptyEquation),
            15 => Ok(SimlinErrorCode::BadModuleInputDst),
            16 => Ok(SimlinErrorCode::BadModuleInputSrc),
            17 => Ok(SimlinErrorCode::NotSimulatable),
            18 => Ok(SimlinErrorCode::BadTable),
            19 => Ok(SimlinErrorCode::BadSimSpecs),
            20 => Ok(SimlinErrorCode::NoAbsoluteReferences),
            21 => Ok(SimlinErrorCode::CircularDependency),
            22 => Ok(SimlinErrorCode::ArraysNotImplemented),
            23 => Ok(SimlinErrorCode::MultiDimensionalArraysNotImplemented),
            24 => Ok(SimlinErrorCode::BadDimensionName),
            25 => Ok(SimlinErrorCode::BadModelName),
            26 => Ok(SimlinErrorCode::MismatchedDimensions),
            27 => Ok(SimlinErrorCode::ArrayReferenceNeedsExplicitSubscripts),
            28 => Ok(SimlinErrorCode::DuplicateVariable),
            29 => Ok(SimlinErrorCode::UnknownDependency),
            30 => Ok(SimlinErrorCode::VariablesHaveErrors),
            31 => Ok(SimlinErrorCode::UnitDefinitionErrors),
            32 => Ok(SimlinErrorCode::Generic),
            33 => Ok(SimlinErrorCode::UnitMismatch),
            34 => Ok(SimlinErrorCode::BadOverride),
            _ => Err(()),
        }
    }
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
            engine::ErrorCode::UnitMismatch => SimlinErrorCode::UnitMismatch,
            engine::ErrorCode::TodoWildcard => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoStarRange => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoRange => SimlinErrorCode::Generic,
            engine::ErrorCode::TodoArrayBuiltin => SimlinErrorCode::Generic,
            engine::ErrorCode::CantSubscriptScalar => SimlinErrorCode::Generic,
            engine::ErrorCode::DimensionInScalarContext => SimlinErrorCode::Generic,
            engine::ErrorCode::BadOverride => SimlinErrorCode::BadOverride,
        }
    }
}

/// Error kind categorizing where in the project the error originates.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimlinErrorKind {
    Project = 0,
    Model = 1,
    #[default]
    Variable = 2,
    Units = 3,
    Simulation = 4,
}

/// Unit error kind for distinguishing types of unit-related errors.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SimlinUnitErrorKind {
    /// Not a unit error
    #[default]
    NotApplicable = 0,
    /// Syntax error in unit string definition
    Definition = 1,
    /// Dimensional analysis mismatch
    Consistency = 2,
    /// Inference error spanning multiple variables
    Inference = 3,
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
    pub kind: SimlinErrorKind,
    pub unit_error_kind: SimlinUnitErrorKind,
}

/// Opaque project structure
pub struct SimlinProject {
    pub(crate) project: Mutex<engine::Project>,
    pub(crate) ref_count: AtomicUsize,
}

/// Opaque model structure
pub struct SimlinModel {
    pub(crate) project: *const SimlinProject,
    pub(crate) model_name: Arc<String>,
    pub(crate) ref_count: AtomicUsize,
}

/// Internal state for SimlinSim
pub(crate) struct SimState {
    pub(crate) compiled: Option<engine::CompiledSimulation>,
    pub(crate) vm: Option<engine::Vm>,
    /// Stores the error from VM creation if it failed.
    /// This allows us to surface the actual error when users try to run
    /// the simulation, instead of a generic "VM not initialized" message.
    pub(crate) vm_error: Option<engine::Error>,
    pub(crate) results: Option<engine::Results>,
    /// Constant value overrides survive VM consumption (run_to_end consumes the VM).
    /// Re-applied to new VMs created on reset.
    pub(crate) overrides: HashMap<usize, f64>,
}

/// Opaque simulation structure
pub struct SimlinSim {
    pub(crate) model: *const SimlinModel,
    pub(crate) enable_ltm: bool,
    pub(crate) state: Mutex<SimState>,
    pub(crate) ref_count: AtomicUsize,
}

// ── shared helpers (pub(crate)) ────────────────────────────────────────

pub(crate) fn clear_out_error(out_error: *mut *mut SimlinError) {
    if out_error.is_null() {
        return;
    }
    unsafe {
        *out_error = ptr::null_mut();
    }
}

pub(crate) fn store_error(out_error: *mut *mut SimlinError, error: SimlinError) {
    if out_error.is_null() {
        return;
    }
    unsafe {
        *out_error = error.into_raw();
    }
}

pub(crate) fn store_ffi_error(out_error: *mut *mut SimlinError, error: FfiError) {
    store_error(out_error, error.into_simlin_error());
}

pub(crate) fn error_from_anyhow(err: AnyError) -> SimlinError {
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

pub(crate) fn store_anyhow_error(out_error: *mut *mut SimlinError, err: AnyError) {
    store_error(out_error, error_from_anyhow(err));
}

pub(crate) unsafe fn drop_c_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        let _ = CString::from_raw(ptr);
    }
}

pub(crate) unsafe fn drop_c_string_array(ptr: *mut *mut c_char, count: usize) {
    if ptr.is_null() || count == 0 {
        return;
    }
    let vars = std::slice::from_raw_parts_mut(ptr, count);
    for var in vars {
        drop_c_string(*var);
    }
    let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, count));
}

pub(crate) unsafe fn drop_f64_array(ptr: *mut f64, count: usize) {
    if ptr.is_null() || count == 0 {
        return;
    }
    let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, count));
}

pub(crate) unsafe fn drop_loop(loop_item: &mut SimlinLoop) {
    drop_c_string(loop_item.id);
    drop_c_string_array(loop_item.variables, loop_item.var_count);
}

pub(crate) unsafe fn drop_link(link: &mut SimlinLink) {
    drop_c_string(link.from);
    drop_c_string(link.to);
    drop_f64_array(link.score, link.score_len);
}

pub(crate) unsafe fn drop_loops_vec(loops: &mut Vec<SimlinLoop>) {
    for mut loop_item in loops.drain(..) {
        drop_loop(&mut loop_item);
    }
}

pub(crate) unsafe fn drop_links_vec(links: &mut Vec<SimlinLink>) {
    for mut link in links.drain(..) {
        drop_link(&mut link);
    }
}

pub(crate) fn build_simlin_error(
    code: SimlinErrorCode,
    details: &[ErrorDetailData],
) -> SimlinError {
    let mut error = SimlinError::new(code);

    // Set top-level message from first detail's message, or construct from code
    let message = details
        .iter()
        .find_map(|d| d.message.clone())
        .unwrap_or_else(|| format!("{:?}", code));
    error.set_message(Some(message));

    error.extend_details(details.iter().cloned());
    error
}

/// Macro for unwrapping a `Result` inside an FFI function, storing the
/// error into `out_error` and returning early on `Err`.
#[macro_export]
macro_rules! ffi_try {
    ($out_error:expr, $expr:expr) => {
        match $expr {
            Ok(value) => value,
            Err(err) => {
                $crate::store_anyhow_error($out_error, err);
                return;
            }
        }
    };
}

pub(crate) unsafe fn require_project<'a>(project: *mut SimlinProject) -> Result<&'a SimlinProject> {
    if project.is_null() {
        Err(FfiError::new(SimlinErrorCode::Generic)
            .with_message("project pointer must not be NULL")
            .into())
    } else {
        Ok(&*project)
    }
}

pub(crate) unsafe fn require_model<'a>(model: *mut SimlinModel) -> Result<&'a SimlinModel> {
    if model.is_null() {
        Err(FfiError::new(SimlinErrorCode::Generic)
            .with_message("model pointer must not be NULL")
            .into())
    } else {
        Ok(&*model)
    }
}

pub(crate) unsafe fn require_sim<'a>(sim: *mut SimlinSim) -> Result<&'a SimlinSim> {
    if sim.is_null() {
        Err(FfiError::new(SimlinErrorCode::Generic)
            .with_message("simulation pointer must not be NULL")
            .into())
    } else {
        Ok(&*sim)
    }
}

pub(crate) fn ffi_error_from_engine(error: &engine::Error) -> FfiError {
    FfiError::new(SimlinErrorCode::from(error.code)).with_message(error.to_string())
}

// ── handle ref-counting ────────────────────────────────────────────────
//
// Centralized here so that modules managing child handles (e.g. simulation
// dropping its model, model dropping its project) don't need cross-module
// imports that would create dependency cycles.

/// Increment the project reference count.
pub(crate) unsafe fn project_ref(project: *mut SimlinProject) {
    if !project.is_null() {
        (*project)
            .ref_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Decrement the project reference count, freeing it when it reaches zero.
pub(crate) unsafe fn project_unref(project: *mut SimlinProject) {
    if project.is_null() {
        return;
    }
    let prev_count = (*project)
        .ref_count
        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        let _ = Box::from_raw(project);
    }
}

/// Increment the model reference count.
pub(crate) unsafe fn model_ref(model: *mut SimlinModel) {
    if !model.is_null() {
        (*model)
            .ref_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Decrement the model reference count, freeing it when it reaches zero.
pub(crate) unsafe fn model_unref(model: *mut SimlinModel) {
    if model.is_null() {
        return;
    }
    let prev_count = (*model)
        .ref_count
        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        let model = Box::from_raw(model);
        project_unref(model.project as *mut SimlinProject);
    }
}

/// Increment the simulation reference count.
pub(crate) unsafe fn sim_ref(sim: *mut SimlinSim) {
    if !sim.is_null() {
        (*sim)
            .ref_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Decrement the simulation reference count, freeing it when it reaches zero.
pub(crate) unsafe fn sim_unref(sim: *mut SimlinSim) {
    if sim.is_null() {
        return;
    }
    let prev_count = (*sim)
        .ref_count
        .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    if prev_count == 1 {
        std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
        let sim = Box::from_raw(sim);
        model_unref(sim.model as *mut SimlinModel);
    }
}

/// Compile a project + model name into a `CompiledSimulation`.
///
/// Pure helper with no FFI state -- shared by `project`, `patch`, and `simulation`.
pub(crate) fn compile_simulation(
    project: &engine::Project,
    model_name: &str,
) -> std::result::Result<engine::CompiledSimulation, engine::Error> {
    let compiler = engine::Simulation::new(project, model_name)?;
    compiler.compile()
}

// ── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use engine::test_common::TestProject;
    use serde_json::Value;
    #[test]
    fn test_error_str() {
        unsafe {
            let err_str = simlin_error_str(SimlinErrorCode::NoError as u32);
            assert!(!err_str.is_null());
            let s = CStr::from_ptr(err_str);
            assert_eq!(s.to_str().unwrap(), "no_error");

            // Test unknown error code returns "unknown_error"
            let unknown_str = simlin_error_str(9999);
            assert!(!unknown_str.is_null());
            let s = CStr::from_ptr(unknown_str);
            assert_eq!(s.to_str().unwrap(), "unknown_error");
        }
    }

    fn open_project_from_datamodel(project: &engine::datamodel::Project) -> *mut SimlinProject {
        let pb = engine_serde::serialize(project);
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
                ffi::SimlinJsonFormat::Native as u32,
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
                ffi::SimlinJsonFormat::Sdai as u32,
                &mut out_buffer,
                &mut out_len,
                &mut out_error,
            );

            assert!(out_error.is_null(), "expected no error serializing sdai");
            assert!(!out_buffer.is_null(), "expected SDAI JSON buffer");

            let slice = std::slice::from_raw_parts(out_buffer, out_len);
            let json_str = std::str::from_utf8(slice).expect("valid utf-8 SDAI JSON");

            let actual: Value = serde_json::from_str(json_str).expect("parsed json");
            let expected_model: engine::json_sdai::SdaiModel = datamodel.clone().into();
            let expected = serde_json::to_value(expected_model).unwrap();

            assert_eq!(actual, expected);

            simlin_free(out_buffer);
            simlin_project_unref(proj);
        }
    }

    #[test]
    fn test_project_open_roundtrip() {
        // Create a project using TestProject, serialize to protobuf, open it,
        // and verify it loads correctly.
        let test_project = TestProject::new("roundtrip_test")
            .with_sim_time(0.0, 100.0, 0.25)
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population * 0.01", None)
            .aux("birth_rate", "0.02", None);

        // Build the datamodel and serialize to protobuf
        let datamodel_project = test_project.build_datamodel();
        let project = engine_serde::serialize(&datamodel_project);

        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Verify reference counting starts at 1
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 1);

            // Verify we can access the project data through the mutex
            {
                let project_locked = (*proj).project.lock().unwrap();
                let dm = &project_locked.datamodel;
                assert_eq!(dm.models.len(), 1);
                let model = &dm.models[0];
                assert_eq!(model.variables.len(), 4); // population, births, deaths, birth_rate
            }

            // Get the default model
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
            // Model creation should increment project ref count
            assert_eq!((*proj).ref_count.load(Ordering::SeqCst), 2);

            // Create a simulation
            err = ptr::null_mut();
            let sim = simlin_sim_new(model, false, &mut err as *mut *mut SimlinError);
            assert!(err.is_null());
            assert!(!sim.is_null());

            // Run to completion
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

            // Verify time series
            let c_name = CString::new("population").unwrap();
            let mut step_count: usize = 0;
            err = ptr::null_mut();
            simlin_sim_get_stepcount(
                sim,
                &mut step_count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(step_count > 0);
            let mut series = vec![0.0f64; step_count];
            let mut written: usize = 0;
            err = ptr::null_mut();
            simlin_sim_get_series(
                sim,
                c_name.as_ptr(),
                series.as_mut_ptr(),
                step_count,
                &mut written,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert_eq!(written, step_count);
            // First value should be 100 (initial population)
            assert!((series[0] - 100.0).abs() < 1e-9);
            // Population should be growing (net birth rate > death rate: 0.02 > 0.01)
            assert!(*series.last().unwrap() > 100.0);

            // Clean up (reverse order of creation)
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
            let proj = simlin_project_open_protobuf(
                data.as_ptr(),
                data.len(),
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
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Export to XMILE
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_project_serialize_xmile(
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
                panic!(
                    "project_serialize_xmile failed with error {:?}: {}",
                    code, msg
                );
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
            let proj1 = simlin_project_open_xmile(
                data.as_ptr(),
                data.len(),
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
                panic!("project_open_xmile failed with error {:?}: {}", code, msg);
            }
            assert!(!proj1.is_null());

            // Export to XMILE
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_project_serialize_xmile(
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
                panic!(
                    "project_serialize_xmile failed with error {:?}: {}",
                    code, msg
                );
            }

            // Import the exported XMILE
            err = ptr::null_mut();
            let proj2 =
                simlin_project_open_xmile(output, output_len, &mut err as *mut *mut SimlinError);
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
                    "project_open_xmile (2nd) failed with error {:?}: {}",
                    code, msg
                );
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
            let proj =
                simlin_project_open_xmile(std::ptr::null(), 0, &mut err as *mut *mut SimlinError);
            assert!(proj.is_null());
            assert!(!err.is_null(), "Expected an error but got success");
            simlin_error_free(err);

            // Test with invalid XML
            let bad_data = b"not xml at all";
            err = ptr::null_mut();
            let proj = simlin_project_open_xmile(
                bad_data.as_ptr(),
                bad_data.len(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(proj.is_null());
            assert!(!err.is_null(), "Expected an error but got success");
            simlin_error_free(err);

            // Test with invalid MDL
            err = ptr::null_mut();
            let proj = simlin_project_open_vensim(
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
            simlin_project_serialize_xmile(
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
                                compat: None,
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
                                compat: None,
                            },
                        )),
                    },
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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
                                compat: None,
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
                                compat: None,
                            },
                        )),
                    },
                ],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
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
                            compat: None,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
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
                            compat: None,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();

        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
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
                            compat: None,
                        },
                    )),
                }],
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: None,
        };
        let mut buf = Vec::new();
        project.encode(&mut buf).unwrap();
        unsafe {
            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
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
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
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
            let loops = simlin_analyze_get_loops(model, &mut err as *mut *mut SimlinError);
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
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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
                panic!("project open failed with error {:?}: {}", code, msg);
            }
            assert!(!proj.is_null());

            // Serialize it back out
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_project_serialize_protobuf(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            assert!(!output.is_null());
            assert!(output_len > 0);

            // Verify we can open the serialized project
            let proj2 = simlin_project_open_protobuf(output, output_len, &mut err);
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
                0,
                ptr::null(),
                &mut var_count1 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            let mut var_count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model2,
                0,
                ptr::null(),
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
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            // LTM will be enabled when creating simulation

            // Serialize the project (should NOT include LTM variables)
            let mut output: *mut u8 = std::ptr::null_mut();
            let mut output_len: usize = 0;
            err = ptr::null_mut();
            simlin_project_serialize_protobuf(
                proj,
                &mut output as *mut *mut u8,
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            // Open the serialized project
            let proj2 = simlin_project_open_protobuf(output, output_len, &mut err);
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
                0,
                ptr::null(),
                &mut var_count1 as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(err.is_null());
            let mut var_count2: usize = 0;
            err = ptr::null_mut();
            simlin_model_get_var_count(
                model2,
                0,
                ptr::null(),
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
            simlin_project_serialize_protobuf(
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
                    groups: vec![],
                }],
                dimensions: vec![],
                units: vec![],
                source: None,
            };
            let mut buf = Vec::new();
            project.encode(&mut buf).unwrap();

            let mut err: *mut SimlinError = ptr::null_mut();
            let proj = simlin_project_open_protobuf(buf.as_ptr(), buf.len(), &mut err);
            assert!(!proj.is_null());

            err = ptr::null_mut();
            simlin_project_serialize_protobuf(
                proj,
                ptr::null_mut(),
                &mut output_len as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            assert!(!err.is_null());
            simlin_error_free(err);
            // Test with null output_len pointer
            err = ptr::null_mut();
            simlin_project_serialize_protobuf(
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
                                    compat: None,
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
                                    compat: None,
                                },
                            )),
                        },
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
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
                                    compat: None,
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
                                    compat: None,
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
                                    compat: None,
                                },
                            )),
                        },
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
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
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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

            // Test simlin_project_get_model with non-existent name (should return error)
            let bad_name = CString::new("nonexistent").unwrap();
            err = ptr::null_mut();
            let model_fallback = simlin_project_get_model(
                proj,
                bad_name.as_ptr(),
                &mut err as *mut *mut SimlinError,
            );
            assert!(model_fallback.is_null());
            assert!(!err.is_null());
            assert_eq!(simlin_error_get_code(err), SimlinErrorCode::BadModelName);
            simlin_error_free(err);

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
                0,
                ptr::null(),
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
                0,
                ptr::null(),
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
                0,
                ptr::null(),
                &mut count as *mut usize,
                &mut err as *mut *mut SimlinError,
            );
            // Should handle null gracefully

            let mut var_names: [*mut c_char; 2] = [ptr::null_mut(); 2];
            err = ptr::null_mut();
            simlin_model_get_var_names(
                ptr::null_mut(),
                0,
                ptr::null(),
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
            let proj = simlin_project_open_protobuf(
                buf.as_ptr(),
                buf.len(),
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

    // The remaining tests are included via the test file inclusion mechanism.
    // They test: incoming links, private variables, nested private vars,
    // project add model, JSON open, SDAI format, concurrent operations,
    // error kinds, is_simulatable, LaTeX equations, patches, malloc alignment,
    // NUL rejection, SVG rendering, simulation overrides, reset, and more.

    include!("tests_remaining.rs");
}
