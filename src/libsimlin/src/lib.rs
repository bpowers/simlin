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

// Native consumers of this cdylib/staticlib (pysimlin via cffi, C/C++ FFI) opt
// into mimalloc with the `mimalloc` feature: the engine compile path is
// allocation-heavy (millions of small, short-lived allocations) and mimalloc
// roughly halves allocator time vs the system malloc. Never enabled for the
// wasm32 bundle. See docs/design/engine-performance.md. This is the Rust global
// allocator and is independent of the `simlin_malloc`/`simlin_free`
// cross-boundary helpers in `memory`.
#[cfg(all(feature = "mimalloc", not(target_arch = "wasm32")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use anyhow::{Error as AnyError, Result};
use simlin_engine::{self as engine};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::c_char;
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};

#[cfg(test)]
use prost::Message;
#[cfg(test)]
use simlin_engine::serde as engine_serde;
#[cfg(test)]
use std::ffi::CStr;

// ── internal modules ───────────────────────────────────────────────────
pub mod errors;
mod ffi;
mod ffi_error;

mod analysis;
mod error_api;
mod layout;
mod memory;
mod model;
mod panic_hook;
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
pub use layout::*;
pub use memory::*;
pub use model::*;
pub use panic_hook::*;
pub use patch::simlin_project_apply_patch;
pub use project::*;
pub use serialization::*;
pub use simulation::*;

pub use ffi::{
    SimlinDiscoveredLoop, SimlinDiscoveryResult, SimlinDominantPeriod, SimlinJsonFormat,
    SimlinLink, SimlinLinkPolarity, SimlinLinks, SimlinLoop, SimlinLoopPolarity, SimlinLoops,
};
pub use ffi_error::{ErrorDetail as ErrorDetailData, FfiError, SimlinError};

// ── shared types ───────────────────────────────────────────────────────

/// Error codes for the C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
            // A duplicate macro name / macro-vs-model name collision is, at
            // the FFI granularity, the same user-facing failure as a
            // duplicate variable (a name defined twice). The precise
            // `duplicate_macro_name` distinction is preserved in the
            // engine-level `ErrorCode` and the error's `details` message;
            // collapsing here avoids a gratuitous C-ABI enum addition.
            engine::ErrorCode::DuplicateMacroName => SimlinErrorCode::DuplicateVariable,
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
            engine::ErrorCode::UnsupportedForSerialization => SimlinErrorCode::Generic,
            // A bare reference to a lookup table (used as a value without an
            // argument) is, at the FFI granularity, a generic model error; the
            // precise distinction is preserved in the engine-level `ErrorCode`
            // and the error's `details` message (issue #606).
            engine::ErrorCode::LookupReferencedWithoutArgument => SimlinErrorCode::Generic,
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
    pub datamodel: Mutex<engine::datamodel::Project>,
    /// The salsa database owns its own sync state (the salsa input handles
    /// from the last sync), so incremental re-syncs are automatic: callers
    /// use `db.sync`/`db.sync_staged`/`db.restore` and read the current
    /// `SourceProject` via `db.current_source_project()`. There is no
    /// separate `sync_state` mutex to keep in lockstep.
    pub db: Mutex<engine::db::SimlinDb>,
    pub ref_count: AtomicUsize,
}

/// Opaque model structure
pub struct SimlinModel {
    pub(crate) project: *const SimlinProject,
    pub model_name: Arc<String>,
    pub ref_count: AtomicUsize,
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
    /// Snapshot of `model_ltm_variables().loop_partitions` taken at
    /// `simlin_sim_new` time, while the db is locked and the
    /// `ltm_enabled` flag is still set.  The value is the loop's
    /// **per-slot** cycle-partition vector (length 1 for a
    /// scalar/cross-element/mixed loop, one entry per element for an
    /// A2A loop).  Binds post-sim relative-loop-score queries to the
    /// loop grouping the VM actually ran under, so the FFI stays
    /// consistent when the project is patched (rename/delete/restructure)
    /// after the simulation is created.  Empty when LTM was not enabled,
    /// when the LTM pipeline auto-flipped to discovery (which empties
    /// `loop_partitions` intentionally), or when compilation itself
    /// failed.
    ///
    /// `simlin_analyze_get_relative_loop_score` keys the rel-loop-score
    /// denominator on the *queried slot's* partition (`loop_partitions[id][k]`)
    /// -- so an element-wise-uncoupled A2A loop normalizes per element, exactly
    /// as `ltm_post::compute_rel_loop_scores_per_element` does for the engine's
    /// own consumers.  When populated alongside `loop_element_index`, the two
    /// agree on slot count for every loop (see the `debug_assert!` in
    /// `simlin_sim_new`).
    pub(crate) loop_partitions: HashMap<String, Vec<Option<usize>>>,
    /// Snapshot of per-loop dimension metadata taken at
    /// `simlin_sim_new` time.  Used by the FFI subscript resolver to
    /// turn a user-supplied loop ID like `r1[Boston]` into a slot
    /// offset within the loop_score's `n_slots`.  Empty when LTM was
    /// not enabled or compilation failed; loop_score variables that
    /// weren't generated (e.g. discovery mode) are simply absent
    /// from the map and the resolver naturally falls back to the
    /// "loop unknown" error.
    pub(crate) loop_element_index: HashMap<String, engine::ltm_post::LoopElementIndex>,
    /// Per-(partition, slot) denominator series cached across FFI calls to
    /// `simlin_analyze_get_relative_loop_score`.  The rel-loop-score
    /// definition is `loop_score / Σ|loop_score|` *within a cycle
    /// partition*, evaluated at a specific element slot for arrayed
    /// loops.  The key is `(partition-of-slot-k, k)` -- the partition
    /// component is the cycle partition of *that slot* (so an uncoupled
    /// A2A loop's slots key into different partitions), matching
    /// `ltm_post::compute_rel_loop_scores_per_element`'s bucket grid.
    /// Keying this way lets repeated FFI queries against the same
    /// bucket reuse the expensive sum.  pysimlin's `_populate_loop_behavior`
    /// walks every loop in a project; with this cache the per-partition
    /// sum is computed once per slot and reused across all member loops.
    /// Invalidated in lockstep with `results`: cleared on
    /// `simlin_sim_run_to_end`, `simlin_sim_reset`, and
    /// `simlin_sim_set_value_by_offset`.
    pub(crate) cached_partition_denominators: HashMap<(Option<usize>, usize), Vec<f64>>,
}

/// Opaque simulation structure
pub struct SimlinSim {
    pub(crate) model: *const SimlinModel,
    pub(crate) enable_ltm: bool,
    pub(crate) state: Mutex<SimState>,
    pub ref_count: AtomicUsize,
}

// ── shared helpers (pub(crate)) ────────────────────────────────────────

pub(crate) fn new_synced_db(datamodel: &engine::datamodel::Project) -> engine::db::SimlinDb {
    let mut db = engine::db::SimlinDb::default();
    db.sync(datamodel);
    db
}

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

pub(crate) unsafe fn drop_discovered_loop(loop_item: &mut ffi::SimlinDiscoveredLoop) {
    drop_c_string(loop_item.id);
    drop_c_string_array(loop_item.variables, loop_item.var_count);
    drop_f64_array(loop_item.importance, loop_item.importance_len);
}

pub(crate) unsafe fn drop_discovered_loops_vec(loops: &mut Vec<ffi::SimlinDiscoveredLoop>) {
    for mut loop_item in loops.drain(..) {
        drop_discovered_loop(&mut loop_item);
    }
}

pub(crate) unsafe fn drop_dominant_period(period: &mut ffi::SimlinDominantPeriod) {
    drop_c_string_array(period.dominant_loops, period.dominant_loop_count);
}

pub(crate) unsafe fn drop_dominant_periods_vec(periods: &mut Vec<ffi::SimlinDominantPeriod>) {
    for mut period in periods.drain(..) {
        drop_dominant_period(&mut period);
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

// ── tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use engine::test_common::TestProject;

    fn open_project_from_datamodel(project: &engine::datamodel::Project) -> *mut SimlinProject {
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

    include!("tests_concurrency.rs");
}
