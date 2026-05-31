// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loop/link analysis FFI functions.
//!
//! Functions for extracting feedback loops, causal links with LTM scores,
//! and relative loop scores from a simulation.

use simlin_engine::common::{Canonical, Ident};
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
/// survives the db lock drop -- the FFI boundary takes ownership
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
/// db; callers (the VM FFI and the future from-wasm FFI) drop the db
/// lock immediately after this core returns, so this function
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

    let detected = engine::db::model_detected_loops(&*db_locked, source_model, source_project);

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
    let owned = analyze_links_core(&*db_locked, source_model, source_project, results);
    drop(state_guard);
    drop(db_locked);

    owned_links_to_ffi(owned, out_error)
}

/// Reinterpret a byte buffer of f64 values into a `&[f64]`.
///
/// The from-wasm FFIs accept the result slab as a `(*const u8, usize)` pair
/// for consistency with the other byte-buffer-input functions in this crate
/// (notably `simlin_project_open_protobuf`, which takes `*const u8` + len).
/// f64 alignment is 8 bytes; wasm linear memory hands the host a tightly-packed
/// little-endian array.  We validate that `slab_len` is a multiple of 8 and
/// that the pointer is non-null; the slice itself is then `slab_len / 8`
/// `f64` elements long.
///
/// `slab_ptr` is allowed to be 8-byte misaligned in pathological hosts; we
/// copy through `f64::from_le_bytes` over an 8-byte byte slice rather than
/// reinterpreting in place, so the returned `Vec<f64>` is the canonical
/// owned form and any alignment concerns vanish at the boundary.
unsafe fn slab_from_bytes(slab_ptr: *const u8, slab_len: usize) -> Result<Vec<f64>, SimlinError> {
    if slab_ptr.is_null() {
        return Err(SimlinError::new(SimlinErrorCode::Generic)
            .with_message("wasm result slab pointer must not be NULL"));
    }
    if !slab_len.is_multiple_of(8) {
        return Err(
            SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                "wasm result slab byte length {slab_len} is not a multiple of 8 (f64 size)"
            )),
        );
    }
    // Only the alignment multiple-of-8 invariant is checked here; the geometry
    // check that the element count equals `n_chunks * n_slots` lives in
    // `results_from_layout_and_slab`, which has access to the WasmLayout.
    let n_f64 = slab_len / 8;
    let bytes = std::slice::from_raw_parts(slab_ptr, slab_len);
    let mut out = Vec::with_capacity(n_f64);
    for chunk in bytes.chunks_exact(8) {
        out.push(f64::from_le_bytes(chunk.try_into().unwrap()));
    }
    Ok(out)
}

/// Gets all causal links in a model with LTM link-score series derived from
/// a wasm-produced result slab.
///
/// This is the wasm-backend twin of `simlin_analyze_get_links`: instead of
/// reading the `Results` off a `SimlinSim`'s `SimState`, it rebuilds them
/// from a `(slab, WasmLayout)` pair produced by running the blob returned
/// from `simlin_model_compile_to_wasm(model, ltm_enabled=true, ..)`.  Both
/// FFI functions funnel through the same `analyze_links_core` so the link
/// set and per-link score series agree to within the underlying VM/wasm
/// numeric tolerance.
///
/// The slab is the host-extracted bytes starting at the blob's
/// `results_offset` (the f64-array image of the results region, little-endian).
/// Its byte length encodes how many rows the blob has actually written:
/// `saved_steps * n_slots * 8`, where `saved_steps` is the live `G_SAVED`
/// counter the blob exposes (which equals `n_chunks` after a full run but is
/// 0 for a fresh or just-reset sim and `< n_chunks` mid-run via `run_to`).
/// Passing the slab at its saved length -- not its `n_chunks * n_slots * 8`
/// capacity -- keeps the analytic core from seeing uninit/stale tail rows
/// and mirrors what `simlin_sim_get_series` already does on the VM side.
/// The layout buffer is the bytes returned in `simlin_model_compile_to_wasm`'s
/// `out_layout`.  Both buffers are owned by the caller and only read; this
/// function copies them as needed.
///
/// Because the links analysis is structure-driven (the unique `(from, to)`
/// edges come from `model_causal_edges`, which has no LTM dependency), this
/// function does not need to toggle `ltm_enabled` on the salsa db -- it
/// only needs the wasm-produced score columns from the slab.  The
/// `recompute_ltm_snapshots` dance happens only in the rel-loop-score
/// counterpart.
///
/// # Safety
/// - `model` must be a valid pointer to a `SimlinModel`.
/// - `slab_ptr` must be a non-NULL pointer to `slab_len` valid bytes; the
///   buffer is read but not retained.
/// - `layout_ptr` must be a non-NULL pointer to `layout_len` valid bytes
///   produced by `WasmLayout::serialize` (i.e. the `out_layout` buffer of
///   `simlin_model_compile_to_wasm`).
/// - The returned `SimlinLinks` must be freed with `simlin_free_links`.
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_links_from_wasm_results(
    model: *mut SimlinModel,
    slab_ptr: *const u8,
    slab_len: usize,
    layout_ptr: *const u8,
    layout_len: usize,
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

    // Deserialize the layout first: a malformed layout is the cheaper-to-fail
    // path, so we surface it before touching the slab or locking the db.
    if layout_ptr.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("wasm layout pointer must not be NULL"),
        );
        return ptr::null_mut();
    }
    let layout_bytes = std::slice::from_raw_parts(layout_ptr, layout_len);
    let layout = match engine::wasmgen::WasmLayout::deserialize(layout_bytes) {
        Some(l) => l,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("failed to deserialize wasm layout buffer"),
            );
            return ptr::null_mut();
        }
    };

    let slab_vec = match slab_from_bytes(slab_ptr, slab_len) {
        Ok(v) => v,
        Err(err) => {
            store_error(out_error, err);
            return ptr::null_mut();
        }
    };

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

    let results = match results_from_layout_and_slab(
        &*db_locked,
        source_model,
        source_project,
        &layout,
        &slab_vec,
    ) {
        Ok(r) => r,
        Err(err) => {
            store_error(out_error, err);
            return ptr::null_mut();
        }
    };

    let owned = analyze_links_core(&*db_locked, source_model, source_project, Some(&results));
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

/// The partition of loop `pv` at slot `k`.  For an arrayed loop this is
/// `pv[k]` (out-of-range slots and genuinely-`None` partitions both yield
/// `None`); for a scalar loop (`len <= 1`) the single partition `pv[0]` is
/// broadcast across every slot it is compared in -- a scalar loop has no
/// elements, so it carries its one partition into every slot.  This is the
/// same `slot_partition` convention `ltm_post::compute_rel_loop_scores_per_element`
/// uses to bucket loops into the `(partition, slot)` grid.
fn slot_partition_at(pv: &[Option<usize>], k: usize) -> Option<usize> {
    if pv.len() <= 1 {
        pv.first().copied().flatten()
    } else {
        pv.get(k).copied().flatten()
    }
}

/// Compute the per-(partition, slot) denominator series, lazily populating
/// the cache.  A loop `other` is a member of bucket `(partition_key,
/// element_k)` iff its slot-`element_k` partition equals `partition_key`;
/// each member then contributes `|loop_score[other, k']|` where `k'` is
/// `effective_slot(n_slots[other], element_k)` -- 0 for a broadcast scalar
/// member, `element_k` for an arrayed member with `element_k < n_slots`, and
/// skipped entirely for an arrayed member past its own slots.  This
/// reproduces `ltm_post::compute_rel_loop_scores_per_element`'s bucket sums
/// exactly via the streaming `compute_partition_denominator_for_element`
/// helper, just amortized across repeated FFI queries on the same bucket.
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

/// Backend-agnostic relative-loop-score time series for a *resolved*
/// (base, element_index, n_slots) query.
///
/// `element_index` follows the FFI dispatch convention: `Some(k)` requests
/// a single slot (a scalar loop's only slot, or an arrayed loop's specific
/// element resolved from a `r1[Boston]`-style subscript); `None` requests
/// the argmax-abs aggregator across all `n_slots` (a bare ID on an arrayed
/// loop).  This split is the *only* analytic dispatch in libsimlin's
/// rel-loop-score path, so concentrating it here ensures the VM FFI
/// (`simlin_analyze_get_relative_loop_score`) and the from-wasm FFI added
/// in a later task drive identical math.
///
/// `cache` is a `&mut HashMap<(Option<usize>, usize), Vec<f64>>` shaped
/// exactly like `SimState::cached_partition_denominators`.  The VM FFI
/// passes the persistent on-state cache (so repeated queries amortize);
/// the from-wasm FFI passes a stack-local empty cache (no persistent
/// sim).  The split-borrow against `&mut state` is preserved -- callers
/// borrow `results`, `loop_partitions`, and `loop_element_index` from
/// `state` immutably while passing `&mut state.cached_partition_denominators`
/// as the cache argument.
///
/// Returns `None` only for the engine's own missing-data signal
/// (`compute_rel_loop_score_for_element` / `compute_rel_loop_score_argmax_abs`
/// both `None` when `loop_score_{loop_id}` is absent from `results.offsets`);
/// the FFI shell maps that to a `DoesNotExist` error.  Lookup of `loop_id`
/// in `loop_partitions` is the FFI shell's responsibility (its absence is
/// also `DoesNotExist`, but with a different message), so the core
/// indexes `loop_partitions[loop_id]` directly.
///
/// Divergence from the design doc's single-core proposal: see the note
/// on `OwnedLink` and the phase plan -- splitting into two focused cores
/// (this one + the links core) more honestly satisfies AC5.1 than a
/// single signature carrying snapshots that the links analysis never
/// reads.
pub(crate) fn rel_loop_score_series(
    results: &engine::Results,
    loop_partitions: &HashMap<String, Vec<Option<usize>>>,
    loop_element_index: &HashMap<String, engine::ltm_post::LoopElementIndex>,
    cache: &mut HashMap<(Option<usize>, usize), Vec<f64>>,
    loop_id: &str,
    element_index: Option<usize>,
    n_slots: usize,
) -> Option<Vec<f64>> {
    let partitions = loop_partitions.get(loop_id)?;
    match element_index {
        Some(k) => {
            // Group the denominator by the *queried slot's* partition (slot 0
            // for a scalar loop), so an uncoupled A2A loop normalizes per
            // element rather than against a pooled slot-0 bucket.
            let partition_key = slot_partition_at(partitions, k);
            let denom = ensure_denom_for_element(
                cache,
                results,
                loop_partitions,
                loop_element_index,
                partition_key,
                k,
            );
            engine::ltm_post::compute_rel_loop_score_for_element(
                results, loop_id, n_slots, k, &denom,
            )
        }
        None => {
            // Argmax-abs aggregator over all slots; each slot's denominator is
            // keyed on *that slot's* partition (matching the per-element
            // helper), not slot 0's.
            let mut denoms: Vec<Vec<f64>> = Vec::with_capacity(n_slots);
            for k in 0..n_slots {
                let partition_key = slot_partition_at(partitions, k);
                let denom = ensure_denom_for_element(
                    cache,
                    results,
                    loop_partitions,
                    loop_element_index,
                    partition_key,
                    k,
                );
                denoms.push(denom);
            }
            let denom_refs: Vec<&[f64]> = denoms.iter().map(|d| d.as_slice()).collect();
            engine::ltm_post::compute_rel_loop_score_argmax_abs(
                results,
                loop_id,
                n_slots,
                &denom_refs,
            )
        }
    }
}

/// Reconstruct an `engine::Results` from a wasm-blob result slab and its
/// matching `WasmLayout`.
///
/// The slab is the *already-extracted* f64 region the host strided out of the
/// wasm linear memory (i.e. the bytes at the blob's `results_offset`).  The
/// serialized `WasmLayout`'s `results_offset` itself is a wasm-linear-memory
/// byte offset and is irrelevant once the slab has been lifted out of the wasm
/// side -- callers pass the slab as a flat `[f64]`.
///
/// The slab encodes the *valid* (saved) row count, not the blob's full
/// `n_chunks` capacity: it must be a multiple of `n_slots`, and the resulting
/// row count (`slab.len() / n_slots`) must not exceed `n_chunks`.  The wasm
/// blob's results region is allocated for `n_chunks` rows but the live
/// `G_SAVED` counter records how many of those rows the simulation has
/// actually written -- the rest are uninit/stale (e.g. on a fresh sim, after
/// `reset`, or mid-run via `run_to`).  Callers (typescript `simGetLinks`,
/// the libsimlin tests, etc.) extract `saved_steps * n_slots * 8` bytes
/// rather than the slab's full capacity so the analytic core only sees rows
/// the blob has computed, matching the strided `simGetSeries` contract.
///
/// `Specs` cannot be looked up through a single salsa query (the spec input
/// is split between `SourceProject::sim_specs` and `SourceModel::model_sim_specs`,
/// the per-model override path `assemble_simulation` takes); we mirror its
/// branch here, reading the chosen `datamodel::SimSpecs` off the input and
/// building `engine::SimSpecs::from` it.  The shape mirrors `Vm::into_results()`:
/// `offsets` is the layout's canonical-name → slot map, `step_size` is
/// `n_slots`, `step_count` is `slab.len() / n_slots`, `is_vensim` is false.
/// Neither analytic core (`analyze_links_core`, `rel_loop_score_series`)
/// reads `results.specs`, so the reconstructed `Specs` is only there to
/// keep the `Results` value structurally well-formed -- there is no analytic
/// sensitivity to its contents from these FFIs.
pub(crate) fn results_from_layout_and_slab(
    db: &dyn engine::db::Db,
    model: SourceModel,
    project: SourceProject,
    layout: &engine::wasmgen::WasmLayout,
    slab: &[f64],
) -> Result<engine::Results, SimlinError> {
    let capacity = layout.n_chunks.checked_mul(layout.n_slots).ok_or_else(|| {
        SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
            "wasm layout geometry overflow: n_chunks={} * n_slots={}",
            layout.n_chunks, layout.n_slots
        ))
    })?;
    if layout.n_slots == 0 {
        return Err(SimlinError::new(SimlinErrorCode::Generic)
            .with_message("wasm layout has zero n_slots (no variables to analyze)"));
    }
    if !slab.len().is_multiple_of(layout.n_slots) {
        return Err(
            SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                "wasm result slab length {} f64 elements is not a multiple of n_slots {}",
                slab.len(),
                layout.n_slots,
            )),
        );
    }
    if slab.len() > capacity {
        return Err(SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
            "wasm result slab length {} f64 elements exceeds capacity n_chunks * n_slots = {} * {} = {}",
            slab.len(),
            layout.n_chunks,
            layout.n_slots,
            capacity
        )));
    }
    let step_count = slab.len() / layout.n_slots;

    let offsets: HashMap<Ident<Canonical>, usize> = layout
        .var_offsets
        .iter()
        .map(|(name, off)| (Ident::<Canonical>::new(name), *off))
        .collect();

    // Mirror `assemble_simulation`'s `Specs` branch: prefer the per-model
    // override when present, else fall back to the project-level default.  The
    // salsa inputs now store `datamodel::SimSpecs` directly, so we read the
    // chosen one off the input and build `Specs` from it.  Neither analytic core
    // reads `Specs`, but the field must be present and structurally valid.
    let specs_dm = match model.model_sim_specs(db) {
        Some(ms) => ms,
        None => project.sim_specs(db),
    };
    let specs = engine::SimSpecs::from(specs_dm);

    Ok(engine::Results {
        offsets,
        data: slab.to_vec().into_boxed_slice(),
        step_size: layout.n_slots,
        step_count,
        specs,
        is_vensim: false,
    })
}

/// Salsa reset guard: flip `ltm_enabled` to a chosen value on construction
/// and unconditionally restore it on drop.
///
/// The from-wasm rel-loop FFI runs the same salsa queries
/// `simlin_sim_new` uses to capture `(loop_partitions, loop_element_index)`
/// (`model_ltm_variables` + `project_datamodel_dims` + `build_loop_element_index`),
/// which only return non-empty results when the `SourceProject` input has
/// `ltm_enabled = true`.  We set it true for the duration of those queries
/// and restore it before returning, mirroring `simlin_sim_new`'s
/// `set_project_ltm_enabled(.., true)` ... `set_project_ltm_enabled(.., false)`
/// bookends (`simulation.rs:70` / `:136-138`).
///
/// The `SourceProject` salsa input is shared across every consumer of the
/// project (patch validation, subsequent sim_new calls, the other analyze
/// FFIs); leaking `ltm_enabled = true` past the snapshot recompute would
/// silently change the next consumer's analysis output.  A scope guard
/// (rather than an explicit `set_project_ltm_enabled(.., false)` line
/// somewhere down the function) makes it impossible for an early return
/// or a panic in the middle of the queries to skip the reset.
struct LtmEnabledGuard<'a> {
    db: &'a mut engine::db::SimlinDb,
    project: SourceProject,
    restore_to: bool,
}

impl<'a> LtmEnabledGuard<'a> {
    fn enable(
        db: &'a mut engine::db::SimlinDb,
        project: SourceProject,
        desired: bool,
    ) -> LtmEnabledGuard<'a> {
        let restore_to = project.ltm_enabled(db);
        engine::db::set_project_ltm_enabled(db, project, desired);
        LtmEnabledGuard {
            db,
            project,
            restore_to,
        }
    }

    fn db(&self) -> &engine::db::SimlinDb {
        self.db
    }
}

impl<'a> Drop for LtmEnabledGuard<'a> {
    fn drop(&mut self) {
        // Panic-safe: `set_project_ltm_enabled` only mutates the salsa input when
        // the flag actually changed (the inner `if ltm_enabled(db) != value` guard),
        // so a no-op restore (flag already matched) never touches salsa at all.
        // On a valid db handle -- which the FFI has been using throughout the guard's
        // lifetime -- the setter does not panic.
        engine::db::set_project_ltm_enabled(self.db, self.project, self.restore_to);
    }
}

/// The `(loop_partitions, loop_element_index)` pair `simlin_sim_new` snapshots
/// off the salsa db (and `recompute_ltm_snapshots` re-derives for the from-wasm
/// path) -- the per-slot cycle-partition vector for each loop and the per-loop
/// dimension metadata `rel_loop_score_series` needs to resolve a subscripted
/// loop id and walk the partition denominator cache.  The fields' types match
/// `SimState::loop_partitions` and `SimState::loop_element_index`.
pub(crate) type LtmSnapshots = (
    HashMap<String, Vec<Option<usize>>>,
    HashMap<String, engine::ltm_post::LoopElementIndex>,
);

/// Recompute the per-loop `(loop_partitions, loop_element_index)` snapshots
/// the rel-loop-score core needs.
///
/// Mirrors the snapshot capture in `simlin_sim_new` (simulation.rs:84-89):
/// `model_ltm_variables` only emits a non-empty `loop_partitions` map when
/// the `SourceProject` salsa input has `ltm_enabled = true`, so we toggle
/// the flag for the duration of the queries.  Because the flag lives on a
/// shared `SourceProject` consumed by every other operation against the
/// project (patch validation, subsequent `simlin_sim_new` calls, etc.),
/// always restoring it is non-negotiable -- the `LtmEnabledGuard` makes the
/// reset structurally unmissable, even on a panic in the salsa queries.
///
/// Returns empty maps when the model isn't present in the sync result; the
/// caller's downstream `rel_loop_score_series` then naturally fails the
/// `loop_partitions.get(loop_id)` lookup and the FFI surface that with a
/// "loop unknown" error, matching the VM FFI's behavior.
pub(crate) fn recompute_ltm_snapshots(
    db: &mut engine::db::SimlinDb,
    project: SourceProject,
    model: SourceModel,
    model_name: &str,
) -> LtmSnapshots {
    let guard = LtmEnabledGuard::enable(db, project, true);
    let ltm_vars = engine::db::model_ltm_variables(guard.db(), model, project);
    let project_dims = engine::db::project_datamodel_dims(guard.db(), project);
    let element_index = engine::ltm_post::build_loop_element_index(&ltm_vars.vars, project_dims);
    // The caller has already resolved `model` from `model_name`, so the name
    // has no work to do inside this function.  Assert that the two agree in
    // debug builds to make the invariant machine-checkable.
    debug_assert_eq!(model.name(guard.db()), model_name);
    let snapshots = (ltm_vars.loop_partitions.clone(), element_index);
    // Drop the guard explicitly so the `ltm_enabled` reset happens before
    // returning -- the explicit drop is redundant with Rust's scope rules
    // but documents the ordering at the call site.
    drop(guard);
    snapshots
}

/// Resolved form of a loop-id query: `(base_id, element_index, n_slots)`.
///
/// `element_index` follows the rel-loop-score-core dispatch convention
/// (`Some(k)` = specific slot; `None` = argmax-abs aggregator across all
/// slots of an arrayed bare-id loop).  `n_slots` is the loop's slot count
/// (1 for scalar, the dim-element-space size for an arrayed loop).
pub(crate) struct ResolvedLoopQuery<'a> {
    pub base: &'a str,
    pub element_index: Option<usize>,
    pub n_slots: usize,
}

/// Parse a user-supplied `loop_id` string (`r1`, `r1[Boston]`, `r1[Boston, 2]`)
/// and resolve its subscripts against the LTM snapshots.
///
/// Shared by `simlin_analyze_get_relative_loop_score` (VM-backed; reads the
/// snapshots from a `SimState`) and `simlin_analyze_rel_loop_score_from_wasm_results`
/// (from-wasm; reads them from a `recompute_ltm_snapshots` call).  Both call
/// sites need identical messages for malformed loop ids and unknown loops,
/// so concentrating the resolution here keeps the two FFIs in lockstep on
/// the error surface as well as the analytic dispatch.
pub(crate) fn resolve_loop_query<'a>(
    raw_loop_id: &'a str,
    loop_partitions: &HashMap<String, Vec<Option<usize>>>,
    loop_element_index: &HashMap<String, engine::ltm_post::LoopElementIndex>,
) -> Result<ResolvedLoopQuery<'a>, SimlinError> {
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
            return Err(SimlinError::new(SimlinErrorCode::Generic).with_message(msg));
        }
    };

    if !loop_partitions.contains_key(parsed.base) {
        return Err(
            SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                "loop '{}' does not have relative score data",
                parsed.base
            )),
        );
    }

    let element_meta = loop_element_index.get(parsed.base).cloned();
    let n_slots = element_meta.as_ref().map(|m| m.n_slots).unwrap_or(1).max(1);

    let element_index: Option<usize> = if parsed.subscripts.is_empty() {
        if n_slots <= 1 {
            Some(0)
        } else {
            None
        }
    } else {
        let Some(meta) = element_meta.as_ref() else {
            return Err(
                SimlinError::new(SimlinErrorCode::Generic).with_message(format!(
                    "loop '{}' is not arrayed; subscripts are not allowed",
                    parsed.base
                )),
            );
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
                            "loop '{}' dimension '{}' index '{}' is out of range (1..={max})",
                            parsed.base, dim, value
                        )
                    }
                    engine::ltm_post::ResolveError::InvalidIntegerSubscript { dim, value } => {
                        format!(
                            "loop '{}' dimension '{}' expects a 1-based integer subscript, got '{}'",
                            parsed.base, dim, value
                        )
                    }
                };
                return Err(SimlinError::new(SimlinErrorCode::Generic).with_message(msg));
            }
        }
    };

    Ok(ResolvedLoopQuery {
        base: parsed.base,
        element_index,
        n_slots,
    })
}

/// Compute a loop's relative-loop-score series from a wasm-produced result
/// slab.
///
/// The wasm-backend twin of `simlin_analyze_get_relative_loop_score`.  Both
/// FFIs funnel through `rel_loop_score_series` (extracted in Subcomponent A)
/// over an `engine::Results` and the `(loop_partitions, loop_element_index)`
/// snapshots, so the per-loop time series they produce cannot diverge by
/// construction.
///
/// Unlike the links twin (task 4), the rel-loop-score path needs the
/// snapshots that only `model_ltm_variables` produces when the
/// `SourceProject` salsa input has `ltm_enabled = true`.  This function
/// runs the salsa queries through `recompute_ltm_snapshots`, which uses
/// an `LtmEnabledGuard` to set the flag for the duration of the queries
/// and unconditionally restore it on guard drop.  The reset is mandatory:
/// the flag lives on a shared `SourceProject` input consumed by every
/// other operation on the project, and leaking it would silently change
/// the next consumer's analysis.
///
/// The `loop_id` is parsed in the FFI shell (the engine-side core takes
/// a base id + `(element_index, n_slots)` pair); a bare id on a scalar
/// loop resolves to slot 0, a bare id on an arrayed loop resolves to the
/// argmax-abs aggregator across all slots, and a subscripted id
/// (`r1[Boston]`, `r1[Boston, 2]`) resolves to a specific slot via
/// `LoopElementIndex::resolve`.  See `resolve_loop_query` for the
/// resolution shared with the VM FFI.
///
/// The series is copied into `results_ptr` clamped to `len` entries; the
/// number written is reported through `out_written`, matching the out-buffer
/// semantics of `simlin_analyze_get_relative_loop_score`.  The number written
/// is bounded by the slab's row count -- callers should pass the saved-rows
/// slab (`saved_steps * n_slots * 8` bytes), not the blob's full capacity,
/// for the same reason as the links twin above.
///
/// # Safety
/// - `model` must be a valid pointer to a `SimlinModel`.
/// - `slab_ptr` / `layout_ptr` are the byte buffers produced by the wasm
///   blob's results region and `simlin_model_compile_to_wasm`'s `out_layout`,
///   respectively; both are read but not retained.
/// - `loop_id` must be a valid null-terminated C string.
/// - `results_ptr` must point to a writable array of at least `len` doubles.
/// - `out_written` must be a writable `*mut usize`.
/// - `out_error` may be null or a writable `**mut SimlinError`.
#[no_mangle]
pub unsafe extern "C" fn simlin_analyze_rel_loop_score_from_wasm_results(
    model: *mut SimlinModel,
    slab_ptr: *const u8,
    slab_len: usize,
    layout_ptr: *const u8,
    layout_len: usize,
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
    if layout_ptr.is_null() {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("wasm layout pointer must not be NULL"),
        );
        return;
    }

    let model_ref = ffi_try!(out_error, require_model(model));
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

    let layout_bytes = std::slice::from_raw_parts(layout_ptr, layout_len);
    let layout = match engine::wasmgen::WasmLayout::deserialize(layout_bytes) {
        Some(l) => l,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::Generic)
                    .with_message("failed to deserialize wasm layout buffer"),
            );
            return;
        }
    };

    let slab_vec = match slab_from_bytes(slab_ptr, slab_len) {
        Ok(v) => v,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };

    // The mutable db lock is required because the LTM-snapshot recompute
    // flips `ltm_enabled` on the salsa input (it must be true for the
    // queries to emit non-empty snapshots).  An RAII guard in
    // `recompute_ltm_snapshots` resets the flag before returning.
    let mut db_locked = (*model_ref.project).db.lock().unwrap();
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

    let (loop_partitions, loop_element_index) = recompute_ltm_snapshots(
        &mut db_locked,
        source_project,
        source_model,
        &model_ref.model_name,
    );

    let resolved = match resolve_loop_query(raw_loop_id, &loop_partitions, &loop_element_index) {
        Ok(r) => r,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };

    let results = match results_from_layout_and_slab(
        &*db_locked,
        source_model,
        source_project,
        &layout,
        &slab_vec,
    ) {
        Ok(r) => r,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };

    // No persistent simulation backs this FFI, so the partition-denominator
    // cache lives on the stack for this call.  Repeated FFI queries against
    // the same model will recompute denominators each time -- a trade-off the
    // from-wasm path accepts in exchange for not maintaining a per-call cache
    // on the wasm side (the wasm interactive-scrubbing flow re-runs the blob
    // and reaches this FFI fresh).
    let mut cache: HashMap<(Option<usize>, usize), Vec<f64>> = HashMap::new();
    let series = match rel_loop_score_series(
        &results,
        &loop_partitions,
        &loop_element_index,
        &mut cache,
        resolved.base,
        resolved.element_index,
        resolved.n_slots,
    ) {
        Some(s) => s,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "loop '{}' does not have relative score data",
                    resolved.base
                )),
            );
            return;
        }
    };

    let count = std::cmp::min(series.len(), len);
    for (i, v) in series.iter().take(count).enumerate() {
        *results_ptr.add(i) = *v;
    }
    *out_written = count;

    drop(db_locked);
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

    // Parse loop_id (bare or subscripted -- issue #463) and resolve
    // subscripts against the snapshot via the shared helper so the
    // VM and from-wasm FFIs surface identical error messages.
    let resolved = match resolve_loop_query(
        raw_loop_id,
        &state.loop_partitions,
        &state.loop_element_index,
    ) {
        Ok(r) => r,
        Err(err) => {
            store_error(out_error, err);
            return;
        }
    };

    let Some(results) = state.results.as_ref() else {
        store_error(
            out_error,
            SimlinError::new(SimlinErrorCode::Generic)
                .with_message("simulation has no results; run the simulation first"),
        );
        return;
    };

    // Split-borrow `&mut state`: the cache (mutated) is borrowed disjointly
    // from `results`/`loop_partitions`/`loop_element_index` (read-only).
    let series = match rel_loop_score_series(
        results,
        &state.loop_partitions,
        &state.loop_element_index,
        &mut state.cached_partition_denominators,
        resolved.base,
        resolved.element_index,
        resolved.n_slots,
    ) {
        Some(s) => s,
        None => {
            store_error(
                out_error,
                SimlinError::new(SimlinErrorCode::DoesNotExist).with_message(format!(
                    "loop '{}' does not have relative score data",
                    resolved.base
                )),
            );
            return;
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
