// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loop/link analysis FFI functions.
//!
//! Functions for extracting feedback loops, causal links with LTM scores,
//! and relative loop scores from a simulation.

use simlin_engine::db::{SourceModel, SourceProject};
use simlin_engine::{self as engine, canonicalize};
use std::collections::HashMap;
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

/// Backend-agnostic per-link result emitted by [`analyze_links_core`].
///
/// Owned `String`s and an owned `Vec<f64>` score series, so the value
/// survives the db/sync_state lock drop -- the FFI boundary takes ownership
/// of the strings (into `CString`) and the score buffer (into a `Box<[f64]>`
/// freed by `simlin_free_links`).
///
/// This shape is shared by the VM-backed FFI (`simlin_analyze_get_links`)
/// and by the from-wasm-results FFI added in a later task; concentrating
/// the structure-and-scoring logic in one core (driven by `Option<&Results>`)
/// guarantees the two backends cannot diverge.  See the divergence note in
/// docs/implementation-plans/2026-05-26-wasm-ltm/phase_02.md (line 75) for
/// why the design's single over-broad core is split into two focused cores
/// (links here; relative-loop-score below): the links analysis is driven
/// purely by structure + `Option<&Results>` and has no use for the LTM
/// snapshots that the relative-loop-score core needs.
pub(crate) struct OwnedLink {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) polarity: engine::ltm::LinkPolarity,
    pub(crate) score: Option<Vec<f64>>,
}

/// Resolve the model's unique causal edges and, when `results` is `Some`,
/// look up each edge's LTM link-score series.
///
/// `model_causal_edges` returns `&CausalEdgesResult` borrowed against the
/// db; callers (the VM FFI and the future from-wasm FFI) drop the db /
/// sync_state locks immediately after this core returns, so this function
/// materializes `unique_links` into owned `(String, String)` pairs while
/// the borrow is still live and *only then* iterates over `results`.
/// `compute_link_polarities` returns owned data, so the polarity map
/// outlives the lock drop without further copying.
///
/// `results` is `Option` because non-LTM sims have no score series; the
/// from-wasm callers always pass `Some(&results)` since they hold the
/// rebuilt `Results` on the stack and only reach this core when LTM was
/// part of the wasm compile.
pub(crate) fn analyze_links_core(
    db: &dyn engine::db::Db,
    model: SourceModel,
    project: SourceProject,
    results: Option<&engine::Results>,
) -> Vec<OwnedLink> {
    let causal = engine::db::model_causal_edges(db, model, project);
    let polarities = engine::db::compute_link_polarities(db, model, project);

    // Materialize edges into owned Strings before touching `results`, so the
    // db borrow held by `causal` is no longer needed past this point.  The
    // caller can (and does) drop its locks the moment this function returns.
    let mut unique_links: Vec<(String, String)> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for (from_name, to_set) in &causal.edges {
        for to_name in to_set {
            let key = (from_name.clone(), to_name.clone());
            if seen.insert(key.clone()) {
                unique_links.push(key);
            }
        }
    }

    unique_links
        .into_iter()
        .map(|(from, to)| {
            let score = results.and_then(|r| {
                let link_score_var =
                    format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}\u{2192}{to}");
                let var_ident = canonicalize(&link_score_var);
                r.offsets
                    .get(&*var_ident)
                    .map(|&off| r.iter().map(|row| row[off]).collect::<Vec<f64>>())
            });
            let polarity = polarities
                .get(&(from.clone(), to.clone()))
                .copied()
                .unwrap_or(engine::ltm::LinkPolarity::Unknown);
            OwnedLink {
                from,
                to,
                polarity,
                score,
            }
        })
        .collect()
}

/// Convert a vector of `OwnedLink` into the C-ABI `*mut SimlinLinks`.
///
/// On a `CString::new` failure (interior NUL) the partial allocations are
/// freed via `drop_links_vec`, a generic error is reported through
/// `out_error`, and the function returns `ptr::null_mut()`.
///
/// Score arrays are allocated as `Box<[f64]>` (via `as_mut_ptr` +
/// `mem::forget`) so the existing `simlin_free_links` -> `drop_link` ->
/// `drop_f64_array` ownership chain frees them correctly.
unsafe fn owned_links_to_ffi(
    links: Vec<OwnedLink>,
    out_error: *mut *mut SimlinError,
) -> *mut SimlinLinks {
    if links.is_empty() {
        return Box::into_raw(Box::new(SimlinLinks {
            links: ptr::null_mut(),
            count: 0,
        }));
    }

    let mut c_links: Vec<SimlinLink> = Vec::with_capacity(links.len());
    for owned in links {
        let from = match CString::new(owned.from.as_str()) {
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
        let to = match CString::new(owned.to.as_str()) {
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

        let (score_ptr, score_len) = match owned.score {
            Some(scores) => {
                let score_len = scores.len();
                let mut boxed = scores.into_boxed_slice();
                let score_ptr = boxed.as_mut_ptr();
                std::mem::forget(boxed);
                (score_ptr, score_len)
            }
            None => (ptr::null_mut(), 0),
        };

        let polarity = match owned.polarity {
            engine::ltm::LinkPolarity::Positive => SimlinLinkPolarity::Positive,
            engine::ltm::LinkPolarity::Negative => SimlinLinkPolarity::Negative,
            _ => SimlinLinkPolarity::Unknown,
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
    let mut boxed = c_links.into_boxed_slice();
    let links_ptr = boxed.as_mut_ptr();
    std::mem::forget(boxed);

    Box::into_raw(Box::new(SimlinLinks {
        links: links_ptr,
        count,
    }))
}

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
        // The C ABI exposes only the legacy three-way polarity surface
        // (Reinforcing / Balancing / Undetermined), so MostlyReinforcing /
        // MostlyBalancing fold into their dominant cousin here.  The
        // polarity_confidence ratio is dropped at this boundary because
        // the FFI struct has no field for it; native Rust callers that
        // need confidence go through `engine::db::DetectedLoop` directly.
        let polarity = match loop_item.polarity {
            engine::db::DetectedLoopPolarity::Reinforcing
            | engine::db::DetectedLoopPolarity::MostlyReinforcing => {
                SimlinLoopPolarity::Reinforcing
            }
            engine::db::DetectedLoopPolarity::Balancing
            | engine::db::DetectedLoopPolarity::MostlyBalancing => SimlinLoopPolarity::Balancing,
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

    // Hold the sim state lock only as long as needed to evaluate the
    // (enable_ltm && state.results.is_some()) gate and borrow `&Results`.
    // `analyze_links_core`'s structure-resolution step still needs the db
    // borrow, but the polarity map and the unique-links list are owned, so
    // by the time this function returns to its callers all three locks are
    // dropped along with this scope.
    let state_guard = sim_ref.state.lock().unwrap();
    let results: Option<&engine::Results> = if sim_ref.enable_ltm {
        state_guard.results.as_ref()
    } else {
        None
    };
    let owned = analyze_links_core(&*db_locked, synced_model.source, sync.project, results);
    drop(state_guard);
    drop(sync_state);
    drop(db_locked);

    owned_links_to_ffi(owned, out_error)
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
    let raw_loop_id = match CStr::from_ptr(loop_id).to_str() {
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

    // Parse the loop ID -- callers may pass a bare ID (`r1`) or a
    // subscripted form (`r1[Boston]`, `r1[Boston, 2]`) to address a
    // specific element of an arrayed loop.  Issue #463.
    let parsed = match parse_subscripted_loop_id(raw_loop_id) {
        Ok(p) => p,
        Err(e) => {
            let msg = match e {
                LoopIdParseError::Malformed => format!(
                    "loop_id '{raw_loop_id}' is malformed: expected `id` or `id[subscript, ...]`"
                ),
                LoopIdParseError::EmptyBrackets => format!(
                    "loop_id '{raw_loop_id}' has empty brackets; specify at least one subscript"
                ),
                LoopIdParseError::UnsupportedSyntax => {
                    format!("loop_id '{raw_loop_id}' uses unsupported subscript syntax")
                }
                LoopIdParseError::EmptySubscript => {
                    format!("loop_id '{raw_loop_id}' has an empty subscript inside brackets")
                }
            };
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message(msg),
            );
            return;
        }
    };

    // `rel_loop_score` is no longer materialized as a VM-computed
    // variable (it caused O(P²) compile-time text blowup on dense
    // models; see
    // docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md).  We
    // derive it post-hoc from the `loop_score` series the VM wrote,
    // using the cycle-partition snapshot and per-loop slot metadata
    // captured on SimState at sim_new time.  Reading from snapshots
    // (rather than re-querying `model_ltm_variables` against the
    // current project DB) keeps score lookups consistent with the
    // VM's results even when the project has been patched since the
    // simulation was created -- model renames, variable deletions,
    // or loop-structure changes in later patches cannot invalidate
    // a query whose results already exist.
    let mut state_guard = sim_ref.state.lock().unwrap();
    let state = &mut *state_guard;

    // `loop_partitions` maps each loop id to its per-slot cycle-partition
    // vector (length 1 for a scalar/cross-element/mixed loop, one entry per
    // element for an A2A loop).  This lookup confirms the loop exists; the
    // partition key for a query is read from the *queried slot*, not slot 0,
    // so an element-wise-uncoupled A2A loop normalizes per element (matching
    // `ltm_post::compute_rel_loop_scores_per_element`'s `(partition, slot)`
    // bucketing).
    if !state.loop_partitions.contains_key(parsed.base) {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                "loop '{}' does not have relative score data",
                parsed.base
            )),
        );
        return;
    }

    // Look up the loop's dim metadata.  Loops without an entry are
    // treated as scalar (n_slots=1) via the `unwrap_or` fallback below
    // so legacy bare-ID callers on scalar models continue to work
    // even if the snapshot wasn't populated for some reason.
    let element_meta = state.loop_element_index.get(parsed.base).cloned();
    let n_slots = element_meta.as_ref().map(|m| m.n_slots).unwrap_or(1).max(1);

    // Resolve the requested element_index based on the parsed
    // subscripts and the loop's actual dimensionality.  Three cases:
    //   1. No subscripts on a scalar loop -> element 0.
    //   2. No subscripts on an arrayed loop -> aggregate via argmax-abs
    //      across all slots.  Encoded as `None` here; the dispatch
    //      below recognizes it.
    //   3. Subscripts -> resolve to a specific slot via LoopElementIndex.
    let element_index: Option<usize> = if parsed.subscripts.is_empty() {
        if n_slots <= 1 {
            Some(0)
        } else {
            None // arrayed bare ID: aggregator
        }
    } else {
        let Some(meta) = element_meta.as_ref() else {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                    "loop '{}' is not arrayed; subscripts are not allowed",
                    parsed.base
                )),
            );
            return;
        };
        match meta.resolve(&parsed.subscripts) {
            Ok(idx) => Some(idx),
            Err(e) => {
                let msg = match e {
                    engine::ltm_post::ResolveError::DimCountMismatch { expected, got } => {
                        if expected == 0 {
                            format!(
                                "loop '{}' is not arrayed; subscripts are not allowed",
                                parsed.base
                            )
                        } else {
                            format!(
                                "loop '{}' has {} dimension(s) but {} subscript(s) were provided",
                                parsed.base, expected, got
                            )
                        }
                    }
                    engine::ltm_post::ResolveError::ElementNotFound { dim, value } => format!(
                        "loop '{}' dimension '{}' has no element '{}'",
                        parsed.base, dim, value
                    ),
                    engine::ltm_post::ResolveError::IndexOutOfRange { dim, value, max } => {
                        format!(
                            "loop '{}' dimension '{}' index '{}' is out of range (1..={})",
                            parsed.base, dim, value, max
                        )
                    }
                    engine::ltm_post::ResolveError::InvalidIntegerSubscript { dim, value } => {
                        format!(
                            "loop '{}' dimension '{}' expects a 1-based integer subscript, got '{}'",
                            parsed.base, dim, value
                        )
                    }
                };
                store_error(
                    out_error,
                    SimlinError::new(SimlinErrorCode::Generic).with_message(msg),
                );
                return;
            }
        }
    };

    let Some(ref results) = state.results else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has no results; run the simulation first"),
        );
        return;
    };

    // The partition of loop `pv` at slot `k`.  For an arrayed loop this is
    // `pv[k]` (out-of-range slots and genuinely-`None` partitions both yield
    // `None`); for a scalar loop (`len <= 1`) the single partition `pv[0]` is
    // broadcast across every slot it is compared in -- a scalar loop has no
    // elements, so it carries its one partition into every slot.  This is the
    // same `slot_partition` convention `ltm_post::compute_rel_loop_scores_per_element`
    // uses to bucket loops into the `(partition, slot)` grid.
    fn slot_partition_at(pv: &[Option<usize>], k: usize) -> Option<usize> {
        if pv.len() <= 1 {
            pv.first().copied().flatten()
        } else {
            pv.get(k).copied().flatten()
        }
    }

    // Compute the per-(partition, slot) denominator series, lazily populating
    // the cache.  A loop `other` is a member of bucket `(partition_key,
    // element_k)` iff its slot-`element_k` partition equals `partition_key`;
    // each member then contributes `|loop_score[other, k']|` where `k'` is
    // `effective_slot(n_slots[other], element_k)` -- 0 for a broadcast scalar
    // member, `element_k` for an arrayed member with `element_k < n_slots`, and
    // skipped entirely for an arrayed member past its own slots.  This
    // reproduces `ltm_post::compute_rel_loop_scores_per_element`'s bucket sums
    // exactly via the streaming `compute_partition_denominator_for_element`
    // helper, just amortized across repeated FFI queries on the same bucket.
    fn ensure_denom_for_element(
        cache: &mut HashMap<(Option<usize>, usize), Vec<f64>>,
        results: &engine::Results,
        loop_partitions: &HashMap<String, Vec<Option<usize>>>,
        element_index_map: &HashMap<String, engine::ltm_post::LoopElementIndex>,
        partition_key: Option<usize>,
        element_k: usize,
    ) -> Vec<f64> {
        if let Some(cached) = cache.get(&(partition_key, element_k)) {
            return cached.clone();
        }
        let members: Vec<(&str, usize)> = loop_partitions
            .iter()
            .filter_map(|(id, pv)| {
                if slot_partition_at(pv, element_k) == partition_key {
                    let n = element_index_map
                        .get(id)
                        .map(|m| m.n_slots)
                        .unwrap_or(1)
                        .max(1);
                    Some((id.as_str(), n))
                } else {
                    None
                }
            })
            .collect();
        let denom = engine::ltm_post::compute_partition_denominator_for_element(
            results,
            members.iter().copied(),
            element_k,
        );
        cache.insert((partition_key, element_k), denom.clone());
        denom
    }

    // Borrow-split so cache mutation doesn't conflict with the
    // results / partition / element-index reads.
    let series = match element_index {
        Some(k) => {
            // Group the denominator by the *queried slot's* partition (slot 0
            // for a scalar loop), so an uncoupled A2A loop normalizes per
            // element rather than against a pooled slot-0 bucket.
            let partition_key = slot_partition_at(&state.loop_partitions[parsed.base], k);
            let denom = ensure_denom_for_element(
                &mut state.cached_partition_denominators,
                results,
                &state.loop_partitions,
                &state.loop_element_index,
                partition_key,
                k,
            );
            match engine::ltm_post::compute_rel_loop_score_for_element(
                results,
                parsed.base,
                n_slots,
                k,
                &denom,
            ) {
                Some(s) => s,
                None => {
                    store_error(
                        out_error,
                        SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                            "loop '{}' does not have relative score data",
                            parsed.base
                        )),
                    );
                    return;
                }
            }
        }
        None => {
            // Argmax-abs aggregator over all slots; each slot's denominator is
            // keyed on *that slot's* partition (matching the per-element
            // helper), not slot 0's.
            let mut denoms: Vec<Vec<f64>> = Vec::with_capacity(n_slots);
            for k in 0..n_slots {
                let partition_key = slot_partition_at(&state.loop_partitions[parsed.base], k);
                let denom = ensure_denom_for_element(
                    &mut state.cached_partition_denominators,
                    results,
                    &state.loop_partitions,
                    &state.loop_element_index,
                    partition_key,
                    k,
                );
                denoms.push(denom);
            }
            let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();
            match engine::ltm_post::compute_rel_loop_score_argmax_abs(
                results,
                parsed.base,
                n_slots,
                &denom_refs,
            ) {
                Some(s) => s,
                None => {
                    store_error(
                        out_error,
                        SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                            "loop '{}' does not have relative score data",
                            parsed.base
                        )),
                    );
                    return;
                }
            }
        }
    };

    let count = std::cmp::min(series.len(), len);
    for (i, v) in series.iter().take(count).enumerate() {
        *results_ptr.add(i) = *v;
    }
    *out_written = count;
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

/// Get the number of element slots a loop's `loop_score` series occupies.
///
/// For scalar loops this is 1; for arrayed (A2A) loops it equals the
/// product of the loop's dimension lengths.  Used by callers (pysimlin,
/// the TS engine) to detect whether a loop supports subscripted access
/// (`r1[Boston]`) or only bare ID access.
///
/// Errors with `DoesNotExist` if the loop_id is not present in the
/// snapshot captured at `simlin_sim_new` time -- typically because the
/// sim was created with `enable_ltm = false`, the loop was added in a
/// later patch (the snapshot is bound to compilation-era loops), or
/// the LTM pipeline auto-flipped to discovery mode (which doesn't
/// emit loop_score variables).
///
/// # Safety
/// - `sim` must be a valid pointer to a SimlinSim
/// - `loop_id` must be a valid null-terminated C string
/// - `out_element_count` must be a valid pointer to a usize
/// - `out_error` may be null or a valid pointer to a SimlinError pointer
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_get_loop_element_count(
    sim: *mut SimlinSim,
    loop_id: *const c_char,
    out_element_count: *mut usize,
    out_error: *mut *mut SimlinError,
) {
    clear_out_error(out_error);
    if out_element_count.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("out_element_count pointer must not be NULL"),
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
    let loop_id_str = match CStr::from_ptr(loop_id).to_str() {
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
    let state = sim_ref.state.lock().unwrap();
    match state.loop_element_index.get(loop_id_str) {
        Some(meta) => *out_element_count = meta.n_slots,
        None => {
            *out_element_count = 0;
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "loop '{loop_id_str}' is not present in the LTM snapshot"
                )),
            );
        }
    }
}

/// Result of parsing a subscripted loop ID like `r1[Boston, 2]` -> (`"r1"`, `["Boston", "2"]`).
///
/// The returned slices borrow from `input`; subscripts are trimmed of
/// surrounding whitespace but preserved in their original case.  Element-name
/// canonicalization happens later in the resolver step.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ParsedLoopId<'a> {
    pub base: &'a str,
    pub subscripts: Vec<&'a str>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoopIdParseError {
    /// Trailing or unmatched brackets, like `r1[` or `r1]`.
    Malformed,
    /// `r1[]` -- empty subscript lists are not allowed.
    EmptyBrackets,
    /// `r1[a][b]` or `[r1]` -- nesting / leading bracket rejected.
    UnsupportedSyntax,
    /// `r1[a,]` or `r1[a,,b]` -- empty subscripts inside the bracket.
    EmptySubscript,
}

/// Parse a loop ID with optional bracketed subscripts.
///
/// - `"r1"` -> ParsedLoopId { base: "r1", subscripts: [] }
/// - `"r1[Boston]"` -> { base: "r1", subscripts: ["Boston"] }
/// - `"r1[Boston, 2]"` -> { base: "r1", subscripts: ["Boston", "2"] }
/// - whitespace inside brackets is trimmed
/// - returns Err for malformed input (unclosed brackets, nested, empty)
///
/// The base ID and the subscripts are returned as borrowed slices into
/// `input`; canonicalization happens at the resolver step.
pub(crate) fn parse_subscripted_loop_id(input: &str) -> Result<ParsedLoopId<'_>, LoopIdParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(LoopIdParseError::Malformed);
    }
    if trimmed.starts_with('[') {
        return Err(LoopIdParseError::UnsupportedSyntax);
    }
    let Some(open_pos) = trimmed.find('[') else {
        // No brackets at all.  Reject lingering `]` on a bare ID.
        if trimmed.contains(']') {
            return Err(LoopIdParseError::Malformed);
        }
        return Ok(ParsedLoopId {
            base: trimmed,
            subscripts: Vec::new(),
        });
    };
    let base = &trimmed[..open_pos];
    let after_open = &trimmed[open_pos + 1..];
    let Some(close_pos) = after_open.rfind(']') else {
        return Err(LoopIdParseError::Malformed);
    };
    // Reject anything past the closing bracket -- e.g. `r1[a]b` or `r1[a][b]`.
    if !after_open[close_pos + 1..].trim().is_empty() {
        return Err(LoopIdParseError::UnsupportedSyntax);
    }
    let inner = &after_open[..close_pos];
    // Reject nested brackets inside the subscript list.
    if inner.contains('[') {
        return Err(LoopIdParseError::UnsupportedSyntax);
    }
    if inner.trim().is_empty() {
        return Err(LoopIdParseError::EmptyBrackets);
    }
    let mut subscripts: Vec<&str> = Vec::new();
    for part in inner.split(',') {
        let trimmed_part = part.trim();
        if trimmed_part.is_empty() {
            return Err(LoopIdParseError::EmptySubscript);
        }
        subscripts.push(trimmed_part);
    }
    Ok(ParsedLoopId { base, subscripts })
}

#[cfg(test)]
mod parse_tests {
    use super::*;

    #[test]
    fn parses_bare_id() {
        let parsed = parse_subscripted_loop_id("r1").unwrap();
        assert_eq!(parsed.base, "r1");
        assert!(parsed.subscripts.is_empty());
    }

    #[test]
    fn parses_single_subscript() {
        let parsed = parse_subscripted_loop_id("r1[Boston]").unwrap();
        assert_eq!(parsed.base, "r1");
        assert_eq!(parsed.subscripts, vec!["Boston"]);
    }

    #[test]
    fn parses_multi_subscript() {
        let parsed = parse_subscripted_loop_id("r1[Boston, 2]").unwrap();
        assert_eq!(parsed.base, "r1");
        assert_eq!(parsed.subscripts, vec!["Boston", "2"]);
    }

    #[test]
    fn trims_internal_whitespace() {
        let parsed = parse_subscripted_loop_id("r1[  Boston  ,   2  ]").unwrap();
        assert_eq!(parsed.subscripts, vec!["Boston", "2"]);
    }

    #[test]
    fn trims_outer_whitespace() {
        let parsed = parse_subscripted_loop_id("  r1[Boston]  ").unwrap();
        assert_eq!(parsed.base, "r1");
        assert_eq!(parsed.subscripts, vec!["Boston"]);
    }

    #[test]
    fn rejects_empty_input() {
        assert_eq!(
            parse_subscripted_loop_id(""),
            Err(LoopIdParseError::Malformed)
        );
        assert_eq!(
            parse_subscripted_loop_id("   "),
            Err(LoopIdParseError::Malformed)
        );
    }

    #[test]
    fn rejects_unclosed_bracket() {
        assert_eq!(
            parse_subscripted_loop_id("r1[Boston"),
            Err(LoopIdParseError::Malformed)
        );
    }

    #[test]
    fn rejects_stray_close_bracket() {
        assert_eq!(
            parse_subscripted_loop_id("r1]"),
            Err(LoopIdParseError::Malformed)
        );
    }

    #[test]
    fn rejects_leading_bracket() {
        assert_eq!(
            parse_subscripted_loop_id("[Boston]"),
            Err(LoopIdParseError::UnsupportedSyntax)
        );
    }

    #[test]
    fn rejects_empty_brackets() {
        assert_eq!(
            parse_subscripted_loop_id("r1[]"),
            Err(LoopIdParseError::EmptyBrackets)
        );
        assert_eq!(
            parse_subscripted_loop_id("r1[   ]"),
            Err(LoopIdParseError::EmptyBrackets)
        );
    }

    #[test]
    fn rejects_empty_subscript() {
        assert_eq!(
            parse_subscripted_loop_id("r1[Boston,]"),
            Err(LoopIdParseError::EmptySubscript)
        );
        assert_eq!(
            parse_subscripted_loop_id("r1[,Boston]"),
            Err(LoopIdParseError::EmptySubscript)
        );
        assert_eq!(
            parse_subscripted_loop_id("r1[a,,b]"),
            Err(LoopIdParseError::EmptySubscript)
        );
    }

    #[test]
    fn rejects_nested_brackets() {
        assert_eq!(
            parse_subscripted_loop_id("r1[a[b]]"),
            Err(LoopIdParseError::UnsupportedSyntax)
        );
    }

    #[test]
    fn rejects_trailing_after_close() {
        assert_eq!(
            parse_subscripted_loop_id("r1[a]b"),
            Err(LoopIdParseError::UnsupportedSyntax)
        );
    }
}
