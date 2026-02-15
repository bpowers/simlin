// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Simulation lifecycle FFI functions.
//!
//! Creating simulations, reference counting, running (to a time, to end,
//! initials), resetting, setting/clearing constant values, and reading values
//! and time series from simulation results.

use simlin_engine::common::Ident;
use simlin_engine::{self as engine, Vm};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double};
use std::ptr;
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::{
    clear_out_error, compile_simulation, drop_c_string, ffi_error_from_engine, require_model,
    require_sim, store_error, store_ffi_error, SimState, SimlinErrorCode, SimlinModel, SimlinSim,
};

/// Internal/instrumentation variables (e.g. LTM `$time`, `$dt`) start
/// with `$` and should not be visible to callers.
fn is_internal_var(name: &str) -> bool {
    name.starts_with('$')
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
            crate::store_anyhow_error(out_error, err);
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

    crate::model_ref(model);

    // Compile the simulation and cache the CompiledSimulation for reset reuse.
    let (compiled, vm, vm_error) = match compile_simulation(&project_variant, &model_ref.model_name)
    {
        Ok(compiled) => match Vm::new(compiled.clone()) {
            Ok(vm) => (Some(compiled), Some(vm), None),
            Err(err) => (Some(compiled), None, Some(err)),
        },
        Err(err) => (None, None, Some(err)),
    };
    let sim = Box::new(SimlinSim {
        model: model_ref as *const _,
        enable_ltm,
        state: Mutex::new(SimState {
            compiled,
            vm,
            vm_error,
            results: None,
            overrides: HashMap::new(),
        }),
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
    crate::sim_ref(sim);
}

/// Decrements the reference count and frees the simulation if it reaches zero
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_unref(sim: *mut SimlinSim) {
    crate::sim_unref(sim);
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
    } else if let Some(ref err) = state.vm_error {
        // Return the actual VM creation error instead of a generic message
        store_ffi_error(out_error, ffi_error_from_engine(err));
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
        // Return the actual VM creation error if available, otherwise generic message
        if let Some(ref err) = state.vm_error {
            store_ffi_error(out_error, ffi_error_from_engine(err));
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("simulation has not been initialised with a VM"),
            );
        }
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

    let mut state = sim_ref.state.lock().unwrap();
    state.results = None;

    if let Some(ref mut vm) = state.vm {
        // Fast path: reuse existing VM allocation
        vm.reset();
    } else if let Some(ref compiled) = state.compiled {
        // Recreate VM from cached compiled simulation
        match Vm::new(compiled.clone()) {
            Ok(mut new_vm) => {
                for (&off, &val) in &state.overrides {
                    if let Err(err) = new_vm.set_value_by_offset(off, val) {
                        store_ffi_error(out_error, ffi_error_from_engine(&err));
                        return;
                    }
                }
                state.vm = Some(new_vm);
                state.vm_error = None;
            }
            Err(err) => {
                store_ffi_error(out_error, ffi_error_from_engine(&err));
            }
        }
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation was never successfully compiled"),
        );
    }
}

/// Runs just the initial-value evaluation phase of the simulation.
///
/// After calling this, `simlin_sim_get_value` can read the t=0 values.
/// Calling this multiple times is safe (it is idempotent).
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_run_initials(
    sim: *mut SimlinSim,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let mut state = sim_ref.state.lock().unwrap();
    if let Some(ref mut vm) = state.vm {
        if let Err(err) = vm.run_initials() {
            store_ffi_error(out_error, ffi_error_from_engine(&err));
        }
    } else if let Some(ref err) = state.vm_error {
        store_ffi_error(out_error, ffi_error_from_engine(err));
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has not been initialised with a VM"),
        );
    }
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
        Ok(s) => Ident::new(s),
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

/// Sets a persistent value for a simple constant variable by name.
///
/// The value persists across `simlin_sim_reset`. Call `simlin_sim_clear_values`
/// to remove all overrides and restore compiled defaults.
///
/// Can be called even when the VM has been consumed by `simlin_sim_run_to_end`;
/// the value will be stored and applied to the next VM created on reset.
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
        Ok(s) => Ident::new(s),
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
        match vm.set_value(&canon_name, val) {
            Ok(off) => {
                state.overrides.insert(off, val);
            }
            Err(err) => {
                store_ffi_error(out_error, ffi_error_from_engine(&err));
            }
        }
    } else if let Some(ref compiled) = state.compiled {
        if let Some(off) = compiled.get_offset(&canon_name) {
            if !compiled.is_constant_offset(off) {
                let err = engine::Error {
                    code: engine::ErrorCode::BadOverride,
                    kind: engine::ErrorKind::Simulation,
                    details: Some(format!(
                        "cannot set value of '{}': not a simple constant",
                        canon_name
                    )),
                };
                store_ffi_error(out_error, ffi_error_from_engine(&err));
                return;
            }
            state.overrides.insert(off, val);
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "variable '{}' not found in compiled simulation",
                    canon_name
                )),
            );
        }
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation was never successfully compiled"),
        );
    }
}

/// Clears all constant value overrides, restoring original compiled values.
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_clear_values(
    sim: *mut SimlinSim,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let mut state = sim_ref.state.lock().unwrap();
    state.overrides.clear();
    if let Some(ref mut vm) = state.vm {
        vm.clear_values();
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
/// Intended for debugging/tests to verify name->offset resolution.
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
        Ok(s) => Ident::new(s),
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

/// Gets the number of simulation-level variable names (flattened offsets).
///
/// Available immediately after `simlin_sim_new` (no simulation run needed).
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_var_count(
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

    if let Some(ref vm) = state.vm {
        *out_count = vm
            .names_as_strs()
            .iter()
            .filter(|n| !is_internal_var(n))
            .count();
    } else if let Some(ref results) = state.results {
        *out_count = results
            .offsets
            .keys()
            .filter(|k| !is_internal_var(k.as_str()))
            .count();
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation was never successfully compiled"),
        );
    }
}

/// Gets the simulation-level variable names (flattened offsets).
///
/// Available immediately after `simlin_sim_new` (no simulation run needed).
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `result` must be a valid pointer to an array of at least `max` char pointers
/// - The returned strings are owned by the caller and must be freed with simlin_free_string
#[no_mangle]
pub unsafe extern "C" fn simlin_sim_get_var_names(
    sim: *mut SimlinSim,
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

    let sim_ref = ffi_try!(out_error, require_sim(sim));
    let state = sim_ref.state.lock().unwrap();

    let mut names_vec: Vec<String> = if let Some(ref vm) = state.vm {
        vm.names_as_strs()
            .into_iter()
            .filter(|n| !is_internal_var(n))
            .collect()
    } else if let Some(ref results) = state.results {
        results
            .offsets
            .keys()
            .filter(|k| !is_internal_var(k.as_str()))
            .map(|k| k.as_str().to_string())
            .collect()
    } else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation was never successfully compiled"),
        );
        return;
    };

    if max == 0 {
        *out_written = names_vec.len();
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

    names_vec.sort();

    let count = names_vec.len().min(max);
    let mut allocated: Vec<*mut c_char> = Vec::with_capacity(count);

    for (i, name) in names_vec.iter().take(count).enumerate() {
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
        Ok(s) => Ident::new(s),
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
        // VM is still alive -- extract series from its live data buffer
        if let Some(series) = vm.get_series(&name) {
            let count = std::cmp::min(series.len(), len);
            for (i, &val) in series.iter().take(count).enumerate() {
                *results_ptr.add(i) = val;
            }
            *out_written = count;
        } else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist)
                    .with_message(format!("series '{}' not found in VM", name)),
            );
        }
    } else if let Some(ref results) = state.results {
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
