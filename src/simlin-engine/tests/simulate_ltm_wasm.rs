// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM-on-wasm parity harness.
//!
//! Phase 1 deliverable for wasm-ltm: proves that compiling a scalar LTM
//! model with `ltm_enabled = true` puts the `$⁚ltm⁚link_score⁚*` and
//! `$⁚ltm⁚loop_score⁚*` synthetic series into the emitted `WasmLayout`,
//! that running the blob under the DLR-FT interpreter produces those
//! columns matching the bytecode VM within `LTM_SERIES_TOLERANCE`, and
//! that the scalar LTM corpus lowers at or above the
//! `MIN_LTM_MODELS_LOWERED` floor (the monotonically-rising count of LTM
//! models that successfully lower to wasm).
//!
//! Required features: `file_io` (the corpus is loaded from XMILE files on
//! disk via `xmile::project_from_reader`, mirroring `simulate_ltm.rs`).

mod test_helpers;

use std::fs::File;
use std::io::BufReader;

use simlin_engine::datamodel;
use simlin_engine::db::{
    SimlinDb, model_ltm_variables, set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use simlin_engine::wasmgen::{WasmLayout, compile_datamodel_to_artifact};
use simlin_engine::xmile;

use test_helpers::{assert_ltm_slabs_match, vm_results_for_ltm, wasm_results_for_ltm};

/// Shared LTM-synthetic-variable name prefix used by every link/loop/agg
/// score: `"$\u{205A}ltm\u{205A}"` (dollar sign + two-dot punctuation).
/// Matches `ltm_augment::link_score_var_name` and `ltm_post::loop_score_ident`.
const LTM_PREFIX: &str = "$\u{205A}ltm\u{205A}";

/// `$⁚ltm⁚link_score⁚{from}\u{2192}{to}` (per-edge link scores) --
/// `link_score_var_name` in `src/simlin-engine/src/ltm_augment.rs`.
const LTM_LINK_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}link_score\u{205A}";

/// `$⁚ltm⁚loop_score⁚{loop_id}` (per-loop scores) -- `loop_score_ident`
/// in `src/simlin-engine/src/ltm_post.rs`.
const LTM_LOOP_SCORE_PREFIX: &str = "$\u{205A}ltm\u{205A}loop_score\u{205A}";

/// Open a model file under `test/` (relative paths mirror `simulate_ltm.rs`)
/// and parse it via the XMILE reader.
fn load(model_rel_path: &str) -> datamodel::Project {
    let path = format!("../../test/{model_rel_path}");
    let f = File::open(&path).unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
    let mut f = BufReader::new(f);
    xmile::project_from_reader(&mut f)
        .unwrap_or_else(|e| panic!("failed to parse XMILE at {path}: {e:?}"))
}

/// Scan `layout.var_offsets` for the LTM synthetic series, returning
/// `(has_link_scores, has_loop_scores)`. A `true` in either slot means at
/// least one entry of that family was found.
fn layout_has_ltm_series(layout: &WasmLayout) -> (bool, bool) {
    let mut has_link = false;
    let mut has_loop = false;
    for (name, _) in &layout.var_offsets {
        if name.starts_with(LTM_LINK_SCORE_PREFIX) {
            has_link = true;
        }
        if name.starts_with(LTM_LOOP_SCORE_PREFIX) {
            has_loop = true;
        }
        if has_link && has_loop {
            break;
        }
    }
    (has_link, has_loop)
}

// ---------------------------------------------------------------------------
// AC1.1 / AC1.5: layout-shape gates
// ---------------------------------------------------------------------------

/// AC1.1: with `ltm_enabled = true`, a scalar LTM model's emitted layout
/// must carry both the per-link and per-loop synthetic series.
#[test]
fn layout_carries_ltm_series_when_enabled() {
    let project = load("logistic_growth_ltm/logistic_growth.stmx");
    let artifact = compile_datamodel_to_artifact(&project, "main", true, false)
        .expect("compile_datamodel_to_artifact should succeed with LTM enabled");
    let (has_link, has_loop) = layout_has_ltm_series(&artifact.layout);
    assert!(
        has_link,
        "expected $⁚ltm⁚link_score⁚* in WasmLayout.var_offsets when LTM is enabled"
    );
    assert!(
        has_loop,
        "expected $⁚ltm⁚loop_score⁚* in WasmLayout.var_offsets when LTM is enabled"
    );
}

/// AC1.5: with `ltm_enabled = false`, no `$⁚ltm⁚*` entries appear in the
/// emitted layout (LTM-off behavior unchanged -- nothing leaks even though
/// the same compile entry point now accepts the flag).
#[test]
fn layout_omits_ltm_series_when_disabled() {
    let project = load("logistic_growth_ltm/logistic_growth.stmx");
    let artifact = compile_datamodel_to_artifact(&project, "main", false, false)
        .expect("compile_datamodel_to_artifact should succeed with LTM disabled");
    for (name, _) in &artifact.layout.var_offsets {
        assert!(
            !name.starts_with(LTM_PREFIX),
            "unexpected LTM synthetic in LTM-off layout: {name}"
        );
    }
}

// ---------------------------------------------------------------------------
// AC1.2: scalar series-parity (wasm vs VM)
// ---------------------------------------------------------------------------

/// Drive both backends with `ltm_enabled = true` for `model_rel_path` and
/// assert their entire results slabs agree within `LTM_SERIES_TOLERANCE`.
///
/// The first guard (`wasm.offsets` carries an LTM key) is deliberately
/// distinct from the layout-shape gate above: it catches a silent regression
/// where both runs would have been LTM-off and the comparison would have
/// passed vacuously.
fn assert_ltm_series_match(model_rel_path: &str) {
    let project = load(model_rel_path);
    let vm = vm_results_for_ltm(&project, "main");
    let wasm = wasm_results_for_ltm(&project, "main").unwrap_or_else(|msg| {
        panic!("scalar LTM model {model_rel_path} should lower to wasm: {msg}")
    });

    let wasm_has_ltm = wasm
        .offsets
        .keys()
        .any(|k| k.as_str().starts_with(LTM_PREFIX));
    assert!(
        wasm_has_ltm,
        "wasm offsets for {model_rel_path} contain no $⁚ltm⁚* keys -- \
         silent regression to LTM-off?"
    );

    assert_ltm_slabs_match(&vm, &wasm);
}

// The scalar LTM corpus: the three `.stmx` models that lower cleanly today
// and whose `$⁚ltm⁚*` columns are expected to match the VM bit-for-bit.
// `hero_culture_ltm/hero_culture.sd.json` is deliberately excluded -- its
// `.sd.json` extension needs a different loader, and that follow-up is
// scoped outside Phase 1 so the corpus stays loadable through the single
// `xmile::project_from_reader` path.

#[test]
fn series_logistic_growth_matches_vm() {
    assert_ltm_series_match("logistic_growth_ltm/logistic_growth.stmx");
}

#[test]
fn series_arms_race_matches_vm() {
    assert_ltm_series_match("arms_race_3party/arms_race.stmx");
}

#[test]
fn series_decoupled_stocks_matches_vm() {
    assert_ltm_series_match("decoupled_stocks/decoupled.stmx");
}

// ---------------------------------------------------------------------------
// AC2.4: arrayed / cross-element series-parity (wasm vs VM)
// ---------------------------------------------------------------------------

/// Drive both backends for an *arrayed* LTM model and assert their entire
/// results slabs agree element-for-element within `LTM_SERIES_TOLERANCE`,
/// PLUS that at least one emitted `LtmSyntheticVar` carries a non-empty
/// `dimensions` list -- i.e. the comparator is actually exercising an
/// arrayed (multi-slot) LTM column, not silently passing on a scalar
/// reduction of the model.
///
/// The whole-slab comparator in `assert_ltm_slabs_match` already covers
/// every element of every `$⁚ltm⁚*` var (Bare A2A strided, FixedIndex
/// name-baked, scalar->arrayed, and `$⁚ltm⁚agg⁚*` synthetic-agg columns)
/// because both `Results` share their `var_offsets`. The extra guard
/// below is the authoritative "this model actually emits a multi-element
/// LTM var" check: a regression that collapses an A2A target's link/loop
/// score to a single slot would still pass `assert_ltm_slabs_match` (both
/// backends would agree on a scalar value) but fail this guard.
///
/// The dimensions check is sourced from the salsa-tracked
/// `LtmVariablesResult.vars`, the same surface that drives slot allocation
/// for arrayed LTM vars (`ltm_synthetic_equation` over a non-empty `dims`
/// produces an `ApplyToAll`, which `assemble_module` lays out as N
/// contiguous slots), so this is exactly the authoritative shape signal.
fn assert_ltm_series_match_arrayed(model_rel_path: &str) {
    let project = load(model_rel_path);
    let vm = vm_results_for_ltm(&project, "main");
    let wasm = wasm_results_for_ltm(&project, "main").unwrap_or_else(|msg| {
        panic!("arrayed LTM model {model_rel_path} should lower to wasm: {msg}")
    });

    let wasm_has_ltm = wasm
        .offsets
        .keys()
        .any(|k| k.as_str().starts_with(LTM_PREFIX));
    assert!(
        wasm_has_ltm,
        "wasm offsets for {model_rel_path} contain no $⁚ltm⁚* keys -- \
         silent regression to LTM-off?"
    );

    // The multi-slot guard: at least one `LtmSyntheticVar` must carry a
    // non-empty `dimensions` list, so the comparator can't pass vacuously
    // on a scalar reduction of an arrayed model. Read from the salsa-
    // tracked `LtmVariablesResult.vars` (the authoritative shape source)
    // rather than scanning result offsets, since a Bare A2A var occupies
    // contiguous slots under a single offset entry -- multi-slot-ness is
    // not visible from `var_offsets` alone.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let source_model = sync.models["main"].source_model;
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project);
    let multi_slot_count = ltm_vars
        .vars
        .iter()
        .filter(|v| !v.dimensions.is_empty())
        .count();
    assert!(
        multi_slot_count > 0,
        "arrayed LTM model {model_rel_path} emitted no LtmSyntheticVar \
         with element count > 1 -- the comparator would pass vacuously \
         on a scalar reduction; investigate before relaxing"
    );

    assert_ltm_slabs_match(&vm, &wasm);
}

/// AC2.4: A2A (same-element) arrayed feedback loops over `Region = {NYC,
/// Boston, LA}` (N=3). Exercises the Bare A2A strided `$⁚ltm⁚*` slot
/// layout the whole-slab comparator transparently covers.
#[test]
fn series_arrayed_population_matches_vm() {
    assert_ltm_series_match_arrayed("arrayed_population_ltm/arrayed_population.stmx");
}

/// AC2.4: cross-element arrayed loops over `Region = {NYC, Boston}` (N=2)
/// plus a whole-extent `SUM(population[*])` reducer that hoists to a
/// `$⁚ltm⁚agg⁚*` synthetic node. Exercises the FixedIndex name-baked
/// element form (`{from}[{e}]→{to}`) and the agg-routed `source→agg→to`
/// link-score columns alongside the Bare slots.
#[test]
fn series_cross_element_matches_vm() {
    assert_ltm_series_match_arrayed("cross_element_ltm/cross_element.stmx");
}

// ---------------------------------------------------------------------------
// AC4.1: ratcheting floor gate
// ---------------------------------------------------------------------------

/// Monotonically rising floor on the count of LTM corpus models that lower
/// to wasm. Phase 1 covers the scalar corpus (three `.stmx` models); Phase 4
/// ratchets this up to include arrayed/cross-element models. A regression
/// that drops a previously-supported model below this floor must fail the
/// suite (wasm-ltm.AC4.2). The value is therefore only raised, never lowered;
/// if a corpus model unexpectedly stops lowering, investigate the root cause
/// rather than relax the constant.
const MIN_LTM_MODELS_LOWERED: usize = 3;

/// Full LTM corpus driven by the floor gate. Kept colocated with the per-model
/// `series_*` tests so a model added to the floor is always also added to
/// the per-model series comparator (and vice versa); a future arrayed-LTM
/// addition flows through this list.
const LTM_CORPUS: &[&str] = &[
    "logistic_growth_ltm/logistic_growth.stmx",
    "arms_race_3party/arms_race.stmx",
    "decoupled_stocks/decoupled.stmx",
];

/// Run every model in [`LTM_CORPUS`] through `wasm_results_for_ltm` and
/// assert that *at least* `MIN_LTM_MODELS_LOWERED` of them lower cleanly.
/// A model that returns `Err` (`WasmGenError::Unsupported` rendered, or an
/// incremental-compile failure) is logged via `eprintln!` and treated as a
/// skip during rollout -- so a regression on a single model surfaces here
/// without masking other progress, but the count threshold prevents a
/// silent slide back to zero.
///
/// Heavy models (`#[ignore]`) are reserved for the discovery/arrayed phases
/// (e.g. C-LEARN, World3); the scalar Phase 1 corpus runs well under the
/// per-test budget so none of these need ignoring today (see
/// `tests/ltm_discovery_large_models.rs` for the `#[ignore]` precedent).
#[test]
fn ltm_corpus_floor_gate() {
    let mut lowered = 0usize;
    let mut skipped: Vec<(&str, String)> = Vec::new();

    for &model_rel in LTM_CORPUS {
        let project = load(model_rel);
        match wasm_results_for_ltm(&project, "main") {
            Ok(_) => {
                lowered += 1;
            }
            Err(msg) => {
                skipped.push((model_rel, msg));
            }
        }
    }

    for (model, msg) in &skipped {
        eprintln!("ltm_corpus_floor_gate: {model} did not lower to wasm: {msg}");
    }

    assert!(
        lowered >= MIN_LTM_MODELS_LOWERED,
        "LTM-on-wasm corpus regression: only {lowered} of {} models lowered, \
         floor is {MIN_LTM_MODELS_LOWERED}. Skipped: {skipped:?}",
        LTM_CORPUS.len()
    );
}
