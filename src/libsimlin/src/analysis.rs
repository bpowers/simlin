// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loop/link analysis FFI functions.
//!
//! Functions for extracting feedback loops, causal links with LTM scores,
//! and relative loop scores from a simulation.

use simlin_engine::{self as engine, canonicalize};
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double};
use std::ptr;

use crate::ffi::{
    SimlinLink, SimlinLinkPolarity, SimlinLinks, SimlinLoop, SimlinLoopPolarity, SimlinLoops,
};
use crate::ffi_error::SimlinError;
use crate::ffi_try;
use crate::{
    clear_out_error, drop_c_string, drop_link, drop_links_vec, drop_loop, drop_loops_vec,
    require_model, require_sim, store_anyhow_error, store_error, SimlinErrorCode, SimlinModel,
    SimlinSim,
};

/// Get the feedback loops detected in a model
///
/// # Safety
/// - `model` must be a valid pointer to a SimlinModel
/// - The returned SimlinLoops must be freed with simlin_free_loops
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_loops(
    model: *mut SimlinModel,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinLoops {
    clear_out_error(out_error);
    let model_ref = match require_model(model) {
        Ok(m) => m,
        Err(err) => {
            store_anyhow_error(out_error, err);
            return ptr::null_mut();
        }
    };
    // Use salsa db for loop detection with polarity and deterministic IDs
    let db_locked = (*model_ref.project).db.lock().unwrap();
    let sync_state = (*model_ref.project).sync_state.lock().unwrap();
    let sync = match sync_state.as_ref() {
        Some(s) => s.to_sync_result(),
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message("project not initialized"),
            );
            return ptr::null_mut();
        }
    };

    let canonical_model = canonicalize(&model_ref.model_name);
    let synced_model = match sync.models.get(canonical_model.as_ref()) {
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

    let detected = engine::db::model_detected_loops(&*db_locked, synced_model.source, sync.project);

    if detected.loops.is_empty() {
        let result = Box::new(SimlinLoops {
            loops: ptr::null_mut(),
            count: 0,
        });
        return Box::into_raw(result);
    }
    let mut c_loops = Vec::with_capacity(detected.loops.len());
    for loop_item in &detected.loops {
        let id = match CString::new(loop_item.id.as_str()) {
            Ok(s) => s.into_raw(),
            Err(_) => {
                drop_loops_vec(&mut c_loops);
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic)
                        .with_message("loop id contains interior NUL byte"),
                );
                return ptr::null_mut();
            }
        };

        let mut var_names: Vec<*mut c_char> = Vec::with_capacity(loop_item.variables.len());
        for name in &loop_item.variables {
            match CString::new(name.as_str()) {
                Ok(s) => var_names.push(s.into_raw()),
                Err(_) => {
                    drop_c_string(id);
                    for ptr in &var_names {
                        drop_c_string(*ptr);
                    }
                    drop_loops_vec(&mut c_loops);
                    store_error(
                        out_error,
                        SimlinError::new(SimlinErrorCode::Generic)
                            .with_message("loop variable name contains interior NUL byte"),
                    );
                    return ptr::null_mut();
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
        let polarity = match loop_item.polarity {
            engine::db::DetectedLoopPolarity::Reinforcing => SimlinLoopPolarity::Reinforcing,
            engine::db::DetectedLoopPolarity::Balancing => SimlinLoopPolarity::Balancing,
            engine::db::DetectedLoopPolarity::Undetermined => SimlinLoopPolarity::Undetermined,
        };
        c_loops.push(SimlinLoop {
            id,
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
            drop_loop(loop_item);
        }
        let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(loops.loops, loops.count));
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

    // Use salsa db for causal edge extraction
    let db_locked = (*model_ref.project).db.lock().unwrap();
    let sync_state = (*model_ref.project).sync_state.lock().unwrap();
    let sync = match sync_state.as_ref() {
        Some(s) => s.to_sync_result(),
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message("project not initialized"),
            );
            return ptr::null_mut();
        }
    };

    let canonical_model = canonicalize(&model_ref.model_name);
    let synced_model = match sync.models.get(canonical_model.as_ref()) {
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

    let causal = engine::db::model_causal_edges(&*db_locked, synced_model.source, sync.project);

    // Build unique links from causal edges (from_var -> {to_var, ...})
    let mut unique_links = std::collections::HashMap::new();
    for (from_name, to_set) in &causal.edges {
        for to_name in to_set {
            let key = (from_name.clone(), to_name.clone());
            unique_links
                .entry(key)
                .or_insert((from_name.clone(), to_name.clone()));
        }
    }

    // Drop locks before accessing sim state for LTM scores
    drop(sync_state);
    drop(db_locked);

    if unique_links.is_empty() {
        return Box::into_raw(Box::new(SimlinLinks {
            links: ptr::null_mut(),
            count: 0,
        }));
    }

    let has_ltm_scores = {
        let state = sim_ref.state.lock().unwrap();
        sim_ref.enable_ltm && state.results.is_some()
    };

    let mut c_links = Vec::with_capacity(unique_links.len());
    for (_, (from_name, to_name)) in unique_links {
        let from = match CString::new(from_name.as_str()) {
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
        let to = match CString::new(to_name.as_str()) {
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

        let (score_ptr, score_len) = if has_ltm_scores {
            let link_score_var = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
                from_name.as_str(),
                to_name.as_str()
            );
            let var_ident = canonicalize(&link_score_var);

            let state = sim_ref.state.lock().unwrap();
            if let Some(ref results) = state.results {
                if let Some(&offset) = results.offsets.get(&*var_ident) {
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
            polarity: SimlinLinkPolarity::Unknown,
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
            drop_link(link);
        }
        let _ = Box::from_raw(std::ptr::slice_from_raw_parts_mut(links.links, links.count));
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

    let var_name = format!("$\u{205A}ltm\u{205A}rel_loop_score\u{205A}{loop_id}");
    let var_ident = canonicalize(&var_name);

    let state = sim_ref.state.lock().unwrap();
    if let Some(ref results) = state.results {
        if let Some(&offset) = results.offsets.get(&*var_ident) {
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
