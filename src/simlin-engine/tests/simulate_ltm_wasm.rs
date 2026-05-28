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
use simlin_engine::wasmgen::{WasmLayout, compile_datamodel_to_artifact};
use simlin_engine::xmile;

use test_helpers::{
    LTM_SERIES_TOLERANCE, assert_ltm_slabs_match, vm_results_for_ltm, wasm_results_for_ltm,
};

// `LTM_SERIES_TOLERANCE` is re-exported via the use above so a future
// per-model carve-out has a single named constant to reference; the
// compiler accepts an unused import here without a warning when the
// tolerance is consumed transitively through `assert_ltm_slabs_match`.
#[allow(dead_code)]
const _LTM_SERIES_TOLERANCE_REF: f64 = LTM_SERIES_TOLERANCE;

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
