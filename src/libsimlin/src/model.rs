// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Model query FFI functions.
//!
//! Functions for inspecting models: reference counting, listing variables,
//! querying dependencies, retrieving causal links, and getting LaTeX equations.

use simlin_engine::{self as engine, canonicalize, datamodel};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::ptr;
use std::sync::MutexGuard;

use crate::ffi::SimlinLinks;
use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::memory::simlin_malloc;
use crate::{
    clear_out_error, drop_c_string, require_model, store_anyhow_error, store_error,
    SimlinErrorCode, SimlinModel,
};

pub const SIMLIN_VARTYPE_STOCK: u32 = 1 << 0;
pub const SIMLIN_VARTYPE_FLOW: u32 = 1 << 1;
pub const SIMLIN_VARTYPE_AUX: u32 = 1 << 2;
pub const SIMLIN_VARTYPE_MODULE: u32 = 1 << 3;

fn matches_type_mask(var: &datamodel::Variable, type_mask: u32) -> bool {
    if type_mask == 0 {
        return true;
    }
    match var {
        datamodel::Variable::Stock(_) => type_mask & SIMLIN_VARTYPE_STOCK != 0,
        datamodel::Variable::Flow(_) => type_mask & SIMLIN_VARTYPE_FLOW != 0,
        datamodel::Variable::Aux(_) => type_mask & SIMLIN_VARTYPE_AUX != 0,
        datamodel::Variable::Module(_) => type_mask & SIMLIN_VARTYPE_MODULE != 0,
    }
}

/// Parse an optional C filter string into a canonicalized Rust string.
///
/// Returns `Ok(None)` for NULL or empty filters, `Ok(Some(..))` for valid
/// non-empty filters, and `Err(SimlinError)` for invalid UTF-8.
unsafe fn parse_filter(filter: *const c_char) -> Result<Option<String>, SimlinError> {
    if filter.is_null() {
        return Ok(None);
    }
    match CStr::from_ptr(filter).to_str() {
        Ok("") => Ok(None),
        Ok(s) => Ok(Some(canonicalize(s).into_owned())),
        Err(_) => Err(SimlinError::new(SimlinErrorCode::Generic)
            .with_message("filter string is not valid UTF-8")),
    }
}

/// Allocate an FFI output buffer and copy `bytes` into it.
///
/// On success, writes the buffer pointer and length to `out_buffer`/`out_len`
/// and returns `true`. On allocation failure, stores an error and returns `false`.
///
/// # Safety
/// `out_buffer` and `out_len` must be valid, non-null pointers.
unsafe fn write_bytes_to_ffi_output(
    bytes: &[u8],
    out_buffer: *mut *mut u8,
    out_len: *mut usize,
    out_error: *mut *mut SimlinError,
    context: &str,
) -> bool {
    let len = bytes.len();
    let buf = simlin_malloc(len);
    if buf.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message(format!("allocation failed while serializing {context}")),
        );
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
    *out_buffer = buf;
    *out_len = len;
    true
}

/// Compile the model to a self-contained WebAssembly module plus its layout.
///
/// The emitted module exports its own linear `memory` and a `run` function
/// that executes the whole simulation in one call, writing step-major result
/// snapshots into a results region of its memory. This is an alternative to
/// the bytecode VM intended for fast, repeated re-simulation (e.g. interactive
/// parameter scrubbing): the host instantiates the module once and calls `run`
/// on every change.
///
/// Two buffers are returned via the malloc-return convention, each freed
/// separately with `simlin_free`:
/// - `out_wasm`/`out_wasm_len`: the wasm blob.
/// - `out_layout`/`out_layout_len`: a self-describing, length-prefixed layout
///   buffer (all integers little-endian): `n_slots` (u64), `n_chunks` (u64),
///   `results_offset` (u64), `count` (u32), then per entry `name_len` (u32) +
///   UTF-8 name + `offset` (u64). A host strides one variable's `n_chunks`-long
///   series from the results region using `results_offset`, `n_slots`, and the
///   variable's `offset` from this map.
///
/// Works from the model's datamodel alone -- no `SimlinSim` is required. Any
/// compile or codegen failure stores a `SimlinError` (never panics across the
/// boundary) and leaves both output buffers NULL.
///
/// `ltm_enabled` and `ltm_discovery_mode` flip the same flags
/// `simlin_project_enable_ltm` sets on a `SimlinProject`, but locally for this
/// compile: the produced blob's layout includes the `$\u{205A}ltm\u{205A}*`
/// synthetic series iff `ltm_enabled` is true.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `out_wasm`, `out_wasm_len`, `out_layout`, and `out_layout_len` must be
///   valid, non-null pointers
/// - `out_error` may be null
#[no_mangle]
pub unsafe extern "C" fn simlin_model_compile_to_wasm(
    model: *mut SimlinModel,
    ltm_enabled: bool,
    ltm_discovery_mode: bool,
    out_wasm: *mut *mut u8,
    out_wasm_len: *mut usize,
    out_layout: *mut *mut u8,
    out_layout_len: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_wasm.is_null()
        || out_wasm_len.is_null()
        || out_layout.is_null()
        || out_layout_len.is_null()
    {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("output pointers must not be NULL"),
        );
        return;
    }
    *out_wasm = ptr::null_mut();
    *out_wasm_len = 0;
    *out_layout = ptr::null_mut();
    *out_layout_len = 0;

    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return;
        }
    };

    // The compiled-model wasm is regenerated from the project's datamodel; it
    // does not depend on the VM `SimState`, so this works even before a
    // `SimlinSim` has been created for the model.
    let project_ref = &*model_ref.project;
    let datamodel = project_ref.datamodel.lock().unwrap();

    let artifact = match engine::wasmgen::compile_datamodel_to_artifact(
        &datamodel,
        model_ref.model_name.as_str(),
        ltm_enabled,
        ltm_discovery_mode,
    ) {
        Ok(artifact) => artifact,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("wasm code generation failed: {err}")),
            );
            return;
        }
    };

    let layout_bytes = artifact.layout.serialize();

    // Write the wasm blob first. On its allocation failure `write_bytes_to_ffi_output`
    // stores the error and returns false; bail before touching the layout buffer.
    if !write_bytes_to_ffi_output(
        &artifact.wasm,
        out_wasm,
        out_wasm_len,
        out_error,
        "model wasm",
    ) {
        return;
    }
    // If the layout allocation fails, free the wasm buffer already handed out so
    // the caller is never left with one buffer set and the other NULL-but-leaked.
    if !write_bytes_to_ffi_output(
        &layout_bytes,
        out_layout,
        out_layout_len,
        out_error,
        "model wasm layout",
    ) {
        crate::memory::simlin_free(*out_wasm);
        *out_wasm = ptr::null_mut();
        *out_wasm_len = 0;
    }
}

/// Find a model by name in a locked datamodel.
pub(crate) fn find_model_in_datamodel<'a>(
    datamodel: &'a MutexGuard<'_, datamodel::Project>,
    model_name: &str,
) -> Option<&'a datamodel::Model> {
    let canonical = canonicalize(model_name);
    datamodel
        .models
        .iter()
        .find(|m| *canonicalize(&m.name) == *canonical)
}

/// Increments the reference count of a model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_ref(model: *mut SimlinModel) {
    crate::model_ref(model);
}

/// Decrements the reference count and frees the model if it reaches zero
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_unref(model: *mut SimlinModel) {
    crate::model_unref(model);
}

/// Returns the resolved display name of this model.
///
/// The returned string is owned by the caller and must be freed with
/// `simlin_free_string`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_name(
    model: *mut SimlinModel,
    out_error: *mut *mut SimlinError,
) -> *mut c_char {
    clear_out_error(out_error);
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    match CString::new(model_ref.model_name.as_str()) {
        Ok(cs) => cs.into_raw(),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("model name contains interior NUL byte"),
            );
            ptr::null_mut()
        }
    }
}

/// Gets the number of datamodel-level variables in the model.
///
/// # Parameters
/// - `type_mask`: bitmask of `SIMLIN_VARTYPE_STOCK | FLOW | AUX | MODULE`. 0 means all types.
/// - `filter`: canonicalized substring match. NULL or empty = no filter.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_count(
    model: *mut SimlinModel,
    type_mask: u32,
    filter: *const c_char,
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

    let filter_str = match parse_filter(filter) {
        Ok(f) => f,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };

    let model_ref = ffi_try!(out_error, require_model(model));
    let datamodel_locked = (*model_ref.project).datamodel.lock().unwrap();

    let dm_model = match find_model_in_datamodel(&datamodel_locked, &model_ref.model_name) {
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

    let count = dm_model
        .variables
        .iter()
        .filter(|v| matches_type_mask(v, type_mask))
        .filter(|v| {
            filter_str
                .as_ref()
                .is_none_or(|f| canonicalize(v.get_ident()).contains(f.as_str()))
        })
        .count();

    *out_count = count;
}

/// Gets the datamodel-level variable names from the model.
///
/// # Parameters
/// - `type_mask`: bitmask of `SIMLIN_VARTYPE_STOCK | FLOW | AUX | MODULE`. 0 means all types.
/// - `filter`: canonicalized substring match. NULL or empty = no filter.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_names(
    model: *mut SimlinModel,
    type_mask: u32,
    filter: *const c_char,
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

    let filter_str = match parse_filter(filter) {
        Ok(f) => f,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };

    let model_ref = ffi_try!(out_error, require_model(model));
    let datamodel_locked = (*model_ref.project).datamodel.lock().unwrap();

    let dm_model = match find_model_in_datamodel(&datamodel_locked, &model_ref.model_name) {
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

    let mut names: Vec<String> = dm_model
        .variables
        .iter()
        .filter(|v| matches_type_mask(v, type_mask))
        .filter(|v| {
            filter_str
                .as_ref()
                .is_none_or(|f| canonicalize(v.get_ident()).contains(f.as_str()))
        })
        .map(|v| canonicalize(v.get_ident()).into_owned())
        .collect();
    names.sort();

    if max == 0 {
        *out_written = names.len();
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

    let count = names.len().min(max);
    let mut allocated: Vec<*mut c_char> = Vec::with_capacity(count);

    for (i, name) in names.iter().take(count).enumerate() {
        let c_string = match CString::new(name.as_str()) {
            Ok(s) => s,
            Err(_) => {
                for ptr in allocated {
                    drop_c_string(ptr);
                }
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "variable name contains interior NUL byte and cannot be converted",
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

    // Use salsa db for dependency lookup
    let db_locked = (*model_ref.project).db.lock().unwrap();
    let source_project = match db_locked.current_source_project() {
        Some(sp) => sp,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message("project not initialized"),
            );
            return;
        }
    };

    let canonical_model = canonicalize(&model_ref.model_name);
    let source_model = match source_project
        .models(&*db_locked)
        .get(canonical_model.as_ref())
    {
        Some(m) => *m,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::BadModelName)
                    .with_message(format!("model '{}' not found", model_ref.model_name)),
            );
            return;
        }
    };

    let source_var = match source_model.variables(&*db_locked).get(var_name.as_ref()) {
        Some(sv) => *sv,
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

    // The empty module-ident context and empty input set reproduce the old
    // no-arg `variable_direct_dependencies` default path.
    let empty_ctx = engine::db::ModuleIdentContext::new(&*db_locked, vec![]);
    let empty_inputs = engine::db::ModuleInputSet::empty(&*db_locked);
    let var_deps = engine::db::variable_direct_dependencies(
        &*db_locked,
        source_var,
        source_project,
        empty_ctx,
        empty_inputs,
    );
    // Combine dt and initial deps from the variable itself plus any
    // implicit variables. Implicit vars arise from SMOOTH/DELAY expansion
    // and carry the transitive public deps we need.
    let source_vars = source_model.variables(&*db_locked);
    let mut all_deps = std::collections::BTreeSet::new();
    for dep in var_deps.dt_deps.iter().chain(var_deps.initial_deps.iter()) {
        all_deps.insert(dep.clone());
    }
    // For implicit module variables, also include their dependencies
    // (these are the public inputs to SMOOTH/DELAY modules).
    for implicit in &var_deps.implicit_vars {
        for dep in implicit.dt_deps.iter().chain(implicit.initial_deps.iter()) {
            all_deps.insert(dep.clone());
        }
    }
    // Filter to only include public variables -- those that exist
    // as source variables in the model. This excludes private/synthetic
    // names from SMOOTH/DELAY expansion.
    let mut deps: Vec<String> = all_deps
        .into_iter()
        .filter(|name| source_vars.contains_key(name.as_str()))
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

    let mut allocated: Vec<*mut c_char> = Vec::with_capacity(deps.len());
    for (i, dep) in deps.iter().enumerate() {
        let c_string = match CString::new(dep.as_str()) {
            Ok(s) => s,
            Err(_) => {
                for ptr in allocated {
                    drop_c_string(ptr);
                }
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(
                        "dependency name contains interior NUL byte and cannot be converted",
                    ),
                );
                return;
            }
        };
        let raw = c_string.into_raw();
        allocated.push(raw);
        *result.add(i) = raw;
    }

    *out_written = deps.len();
}

/// Gets all causal links in a model
///
/// Returns all causal links detected in the model, with their statically
/// analyzed polarities. This includes flow-to-stock, stock-to-flow, and
/// auxiliary-to-auxiliary links.
///
/// The view matches `simlin_analyze_get_links`'s default
/// (`include_internal = false`): macro/module-internal synthetic nodes are
/// collapsed into composite real-variable edges. Both functions funnel
/// through the same `analyze_links_core`, so the model-level (structural,
/// score-less) and sim-level (scored) link sets cannot drift apart.
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
    // Use salsa db for causal edge extraction
    let db_locked = (*model_ref.project).db.lock().unwrap();
    let source_project = match db_locked.current_source_project() {
        Some(sp) => sp,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message("project not initialized"),
            );
            return ptr::null_mut();
        }
    };

    let canonical_model = canonicalize(&model_ref.model_name);
    let source_model = match source_project
        .models(&*db_locked)
        .get(canonical_model.as_ref())
    {
        Some(m) => *m,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::BadModelName)
                    .with_message(format!("model '{}' not found", model_ref.model_name)),
            );
            return ptr::null_mut();
        }
    };

    // Structural-only call into the shared links core: no Results (so no
    // scores), synthetic nodes collapsed.
    let owned =
        crate::analysis::analyze_links_core(&*db_locked, source_model, source_project, None, false);
    drop(db_locked);

    crate::analysis::owned_links_to_ffi(owned, out_error)
}

/// Gets the LaTeX representation of a variable's equation
///
/// Returns the equation rendered as a LaTeX string, or NULL if the variable
/// doesn't exist or doesn't have an equation (e.g., modules).
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `ident` must be a valid C string
/// - The returned string must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_latex_equation(
    model: *mut SimlinModel,
    ident: *const c_char,
    out_error: *mut *mut SimlinError,
) -> *mut c_char {
    clear_out_error(out_error);

    if ident.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("ident pointer must not be NULL"),
        );
        return ptr::null_mut();
    }

    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };

    let ident_str = match CStr::from_ptr(ident).to_str() {
        Ok(s) => canonicalize(s),
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message("ident is not valid UTF-8"),
            );
            return ptr::null_mut();
        }
    };

    // Use salsa db for LaTeX rendering from parsed AST
    let db_locked = (*model_ref.project).db.lock().unwrap();
    let source_project = match db_locked.current_source_project() {
        Some(sp) => sp,
        None => return ptr::null_mut(),
    };

    let canonical_model = canonicalize(&model_ref.model_name);
    let source_model = match source_project
        .models(&*db_locked)
        .get(canonical_model.as_ref())
    {
        Some(m) => *m,
        None => return ptr::null_mut(),
    };

    let source_var = match source_model.variables(&*db_locked).get(ident_str.as_ref()) {
        Some(sv) => *sv,
        None => return ptr::null_mut(),
    };

    let empty_ctx = engine::db::ModuleIdentContext::new(&*db_locked, vec![]);
    let parsed = engine::db::parse_source_variable_with_module_context(
        &*db_locked,
        source_var,
        source_project,
        empty_ctx,
    );
    let ast = match parsed.variable.ast() {
        Some(a) => a,
        None => return ptr::null_mut(),
    };

    // `to_latex_annotated` wraps each node in a `\htmlData{eqnloc=…}` source
    // range annotation so the equation-preview UI can map a click back to a
    // caret position; rendering it requires KaTeX's `trust` option.
    let latex = ast.to_latex_annotated();
    match CString::new(latex) {
        Ok(s) => s.into_raw(),
        Err(_) => ptr::null_mut(),
    }
}

/// Gets a single variable from the model as tagged JSON.
///
/// Returns JSON with a `"type"` discriminator (`"stock"`, `"flow"`, `"aux"`, `"module"`).
/// Caller must free the output buffer with `simlin_free`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `var_name` must be a valid C string
/// - `out_buffer` and `out_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_var_json(
    model: *mut SimlinModel,
    var_name: *const c_char,
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

    if var_name.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("variable name pointer must not be NULL"),
        );
        return;
    }

    let model_ref = ffi_try!(out_error, require_model(model));
    let datamodel_locked = (*model_ref.project).datamodel.lock().unwrap();

    let name_str = match CStr::from_ptr(var_name).to_str() {
        Ok(s) => s,
        Err(_) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("variable name is not valid UTF-8"),
            );
            return;
        }
    };
    let canonical_name = canonicalize(name_str);

    let dm_model = match find_model_in_datamodel(&datamodel_locked, &model_ref.model_name) {
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

    let dm_var = match dm_model
        .variables
        .iter()
        .find(|v| *canonicalize(v.get_ident()) == *canonical_name)
    {
        Some(v) => v,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "variable '{}' does not exist in model '{}'",
                    name_str, model_ref.model_name
                )),
            );
            return;
        }
    };

    let tagged: engine::json::TaggedVariable = dm_var.clone().into();
    let bytes = match serde_json::to_vec(&tagged) {
        Ok(b) => b,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to serialize variable JSON: {err}")),
            );
            return;
        }
    };

    write_bytes_to_ffi_output(&bytes, out_buffer, out_len, out_error, "variable");
}

/// Gets the effective sim specs for a model as JSON.
///
/// Uses model-level sim_specs if present, otherwise falls back to
/// the project-level sim_specs.
/// Caller must free the output buffer with `simlin_free`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `out_buffer` and `out_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_sim_specs_json(
    model: *mut SimlinModel,
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

    let model_ref = ffi_try!(out_error, require_model(model));
    let datamodel_locked = (*model_ref.project).datamodel.lock().unwrap();

    let dm_model = find_model_in_datamodel(&datamodel_locked, &model_ref.model_name);

    // Sim specs are project-global with optional per-model overrides, so a
    // missing model name is not an error here (unlike get_var_json).
    let dm_sim_specs = dm_model
        .and_then(|m| m.sim_specs.as_ref())
        .unwrap_or(&datamodel_locked.sim_specs);

    let json_sim_specs: engine::json::SimSpecs = dm_sim_specs.clone().into();

    let bytes = match serde_json::to_vec(&json_sim_specs) {
        Ok(b) => b,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to serialize sim specs JSON: {err}")),
            );
            return;
        }
    };

    write_bytes_to_ffi_output(&bytes, out_buffer, out_len, out_error, "sim specs");
}
