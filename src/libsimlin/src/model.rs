// Copyright 2025 The Simlin Authors. All rights reserved.
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

use crate::ffi::{SimlinLink, SimlinLinkPolarity, SimlinLinks};
use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::memory::simlin_malloc;
use crate::{
    clear_out_error, drop_c_string, drop_links_vec, require_model, store_anyhow_error, store_error,
    SimlinErrorCode, SimlinModel,
};

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

/// Find a model by name in a locked project.
pub(crate) fn find_model_in_project<'a>(
    project: &'a MutexGuard<'_, engine::Project>,
    model_name: &str,
) -> Option<&'a datamodel::Model> {
    let canonical = canonicalize(model_name);
    project
        .datamodel
        .models
        .iter()
        .find(|m| canonicalize(&m.name) == canonical)
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
        let from = match CString::new(link.from.as_str()) {
            Ok(s) => s.into_raw(),
            Err(_) => {
                drop_links_vec(&mut c_links);
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("link source contains interior NUL byte"),
                );
                return ptr::null_mut();
            }
        };
        let to = match CString::new(link.to.as_str()) {
            Ok(s) => s.into_raw(),
            Err(_) => {
                drop_c_string(from);
                drop_links_vec(&mut c_links);
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("link destination contains interior NUL byte"),
                );
                return ptr::null_mut();
            }
        };
        c_links.push(SimlinLink {
            from,
            to,
            polarity: match link.polarity {
                engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
                engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
                engine::ltm::LinkPolarity::Unknown => SimlinLinkPolarity::Unknown,
            },
            score: ptr::null_mut(),
            score_len: 0,
        });
    }

    let links = Box::new(SimlinLinks {
        links: c_links.as_mut_ptr(),
        count: c_links.len(),
    });
    std::mem::forget(c_links);
    Box::into_raw(links)
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

    let project_locked = (*model_ref.project).project.lock().unwrap();

    let eng_model = match project_locked
        .models
        .get(&canonicalize(&model_ref.model_name))
    {
        Some(m) => m,
        None => {
            return ptr::null_mut();
        }
    };

    let var = match eng_model.variables.get(&ident_str) {
        Some(v) => v,
        None => {
            return ptr::null_mut();
        }
    };

    let ast = match var.ast() {
        Some(a) => a,
        None => {
            return ptr::null_mut();
        }
    };

    let latex = ast.to_latex();
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
    let project_locked = (*model_ref.project).project.lock().unwrap();

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

    let dm_model = match find_model_in_project(&project_locked, &model_ref.model_name) {
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
        .find(|v| canonicalize(v.get_ident()) == canonical_name)
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

/// Gets all variables from the model as a tagged JSON array.
///
/// Each element has a `"type"` discriminator (`"stock"`, `"flow"`, `"aux"`, `"module"`).
/// Caller must free the output buffer with `simlin_free`.
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - `out_buffer` and `out_len` must be valid pointers
#[no_mangle]
pub unsafe extern "C" fn simlin_model_get_vars_json(
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
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let dm_model = match find_model_in_project(&project_locked, &model_ref.model_name) {
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

    let tagged_vars: Vec<engine::json::TaggedVariable> = dm_model
        .variables
        .iter()
        .map(|v| v.clone().into())
        .collect();

    let bytes = match serde_json::to_vec(&tagged_vars) {
        Ok(b) => b,
        Err(err) => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message(format!("failed to serialize variables JSON: {err}")),
            );
            return;
        }
    };

    write_bytes_to_ffi_output(&bytes, out_buffer, out_len, out_error, "variables");
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
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let dm_model = find_model_in_project(&project_locked, &model_ref.model_name);

    // Sim specs are project-global with optional per-model overrides, so a
    // missing model name is not an error here (unlike get_var_json/get_vars_json).
    let dm_sim_specs = dm_model
        .and_then(|m| m.sim_specs.as_ref())
        .unwrap_or(&project_locked.datamodel.sim_specs);

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
