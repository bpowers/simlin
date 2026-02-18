// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loop/link analysis FFI functions.
//!
//! Functions for extracting feedback loops, causal links with LTM scores,
//! and relative loop scores from a simulation.

use simlin_engine::ltm::{detect_loops, LoopPolarity};
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
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let engine_model = match project_locked
        .models
        .get(&*canonicalize(&model_ref.model_name))
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

    let all_loops = match detect_loops(engine_model, &project_locked) {
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
    if all_loops.is_empty() {
        // Return empty result
        let result = Box::new(SimlinLoops {
            loops: ptr::null_mut(),
            count: 0,
        });
        return Box::into_raw(result);
    }
    let mut c_loops = Vec::with_capacity(all_loops.len());
    for loop_item in all_loops {
        let id = match CString::new(loop_item.id) {
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

        let mut var_names = Vec::with_capacity(loop_item.links.len() + 1);
        let mut seen = std::collections::HashSet::new();
        if !loop_item.links.is_empty() {
            let first = &loop_item.links[0].from;
            if seen.insert(first.clone()) {
                match CString::new(first.as_str()) {
                    Ok(s) => var_names.push(s.into_raw()),
                    Err(_) => {
                        drop_c_string(id);
                        for ptr in var_names {
                            drop_c_string(ptr);
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
            for link in &loop_item.links {
                if seen.insert(link.to.clone()) {
                    match CString::new(link.to.as_str()) {
                        Ok(s) => var_names.push(s.into_raw()),
                        Err(_) => {
                            drop_c_string(id);
                            for ptr in var_names {
                                drop_c_string(ptr);
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
            LoopPolarity::Reinforcing => SimlinLoopPolarity::Reinforcing,
            LoopPolarity::Balancing => SimlinLoopPolarity::Balancing,
            LoopPolarity::Undetermined => SimlinLoopPolarity::Undetermined,
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
    let project_locked = (*model_ref.project).project.lock().unwrap();

    let model = match project_locked
        .models
        .get(&*canonicalize(&model_ref.model_name))
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
        let polarity = match link.polarity {
            engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
            engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
            engine::ltm::LinkPolarity::Unknown => SimlinLinkPolarity::Unknown,
        };

        let (score_ptr, score_len) = if has_ltm_scores {
            let link_score_var = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{205A}{}",
                link.from.as_str(),
                link.to.as_str()
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
