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
use simlin_engine::ltm_finding::{self, FoundLoop};
use simlin_engine::wasmgen::{WasmGenError, WasmLayout, compile_datamodel_to_artifact};
use simlin_engine::xmile;

use test_helpers::{
    LTM_SERIES_TOLERANCE, assert_ltm_slabs_match, ltm_discovery_inputs, vm_results_for_ltm,
    wasm_results_for_ltm, wasm_results_for_ltm_discovery,
};

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
/// The whole-slab comparator in `assert_ltm_slabs_match` covers each
/// `$⁚ltm⁚*` form that the current fixtures actually emit: Bare A2A
/// strided slots in `arrayed_population`; Bare A2A + FixedIndex name-baked
/// slots in `cross_element`. The comparator is *capable* of covering
/// scalar→arrayed and `$⁚ltm⁚agg⁚*` synthetic-agg columns if a future
/// fixture emits them -- adding such a fixture would extend coverage to
/// those forms without changing the comparator itself. The extra guard
/// below is the authoritative "this model actually emits a multi-element
/// LTM var" check: a regression that collapses an A2A target's link/loop
/// score to a single slot would still pass `assert_ltm_slabs_match` (both
/// backends would agree on a scalar value) but fail this guard.
///
/// The dimensions check is sourced from the salsa-tracked
/// `LtmVariablesResult.vars`, the same surface that drives slot allocation
/// for arrayed LTM vars (a loop-score `Equation` over a non-empty `dims`
/// list -- `ApplyToAll` or per-slot `Arrayed` -- is laid out by
/// `assemble_module` as N contiguous slots), so this is exactly the
/// authoritative shape signal.
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
    // Builds a third SimlinDb; sharing with vm_results_for_ltm could be
    // a Phase-5 polish (small fixtures so cost is negligible).
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
/// plus a whole-extent `SUM(population[*])` reducer. The reducer is
/// variable-backed (the variable itself is the aggregate), so no synthetic
/// `$⁚ltm⁚agg⁚*` node is emitted; what IS emitted are FixedIndex name-baked
/// element forms (`{from}[{e}]→{to}`) alongside the Bare A2A slots.
#[test]
fn series_cross_element_matches_vm() {
    assert_ltm_series_match_arrayed("cross_element_ltm/cross_element.stmx");
}

// ---------------------------------------------------------------------------
// AC4.2: end-state floor gate
// ---------------------------------------------------------------------------

/// The end-state expected-supported LTM corpus: every model listed here
/// MUST lower to wasm (and match the VM via its `series_*` peer). Phase 1
/// seeded this with the three scalar `.stmx` models; Phase 4 ratchets it
/// up to include the two arrayed/cross-element models, turning the floor
/// into a true regression net -- any `Unsupported` from a listed model
/// fails the suite (wasm-ltm.AC4.2), and so does any model added to the
/// per-model `series_*` tests but missing here (the two lists are kept in
/// sync by convention; the `series_*` peer for each entry below names it
/// in its `#[test]` docstring).
///
/// `MIN_LTM_MODELS_LOWERED` rises in lockstep with this list, so a future
/// expansion need only append a path and bump the constant.
const EXPECTED_SUPPORTED_LTM_MODELS: &[&str] = &[
    // Scalar (Phase 1):
    "logistic_growth_ltm/logistic_growth.stmx",
    "arms_race_3party/arms_race.stmx",
    "decoupled_stocks/decoupled.stmx",
    // Arrayed / cross-element (Phase 4):
    "arrayed_population_ltm/arrayed_population.stmx",
    "cross_element_ltm/cross_element.stmx",
];

/// Monotonically rising floor on the count of LTM corpus models that lower
/// to wasm. Equal to `EXPECTED_SUPPORTED_LTM_MODELS.len()` -- the floor and
/// the per-model `Ok` assertion below now move together, so a regression
/// that drops any expected-supported model fails the suite (both falls
/// below the floor and breaks the per-model `Ok` assertion). The value is
/// only raised, never lowered; if a corpus model unexpectedly stops
/// lowering, investigate the root cause rather than relax this constant.
const MIN_LTM_MODELS_LOWERED: usize = EXPECTED_SUPPORTED_LTM_MODELS.len();

/// End-state floor gate (wasm-ltm.AC4.2): every model in
/// [`EXPECTED_SUPPORTED_LTM_MODELS`] MUST lower to wasm with LTM enabled.
/// A model that returns `Err` (`WasmGenError::Unsupported` rendered, or an
/// incremental-compile failure) is reported via `eprintln!` and then fails
/// the suite -- no "rollout skip" leniency; the list now names exactly the
/// models the wasm backend is expected to handle.
///
/// The `lowered >= MIN_LTM_MODELS_LOWERED` floor check is currently
/// structurally redundant: because `MIN_LTM_MODELS_LOWERED` is derived
/// directly from `EXPECTED_SUPPORTED_LTM_MODELS.len()`, removing an entry
/// from the list also shrinks the const, so the floor would still pass even
/// after the deletion.  The check is kept as defense-in-depth for the
/// scenario where the const is later decoupled from the list (e.g. pinned
/// as a hard numeric literal in a future refactor), at which point it
/// becomes a real regression net again.
///
/// Heavy models (`#[ignore]`) are reserved for the discovery / large-model
/// phases (e.g. C-LEARN, World3); the listed corpus runs well under the
/// per-test budget so none need ignoring (see
/// `tests/ltm_discovery_large_models.rs` for the `#[ignore]` precedent).
#[test]
fn ltm_corpus_floor_gate() {
    let mut lowered = 0usize;
    let mut failures: Vec<(&str, String)> = Vec::new();

    for &model_rel in EXPECTED_SUPPORTED_LTM_MODELS {
        let project = load(model_rel);
        match wasm_results_for_ltm(&project, "main") {
            Ok(_) => {
                lowered += 1;
            }
            Err(msg) => {
                failures.push((model_rel, msg));
            }
        }
    }

    for (model, msg) in &failures {
        eprintln!("ltm_corpus_floor_gate: {model} did not lower to wasm: {msg}");
    }

    assert!(
        failures.is_empty(),
        "LTM-on-wasm regression: {} of {} expected-supported models failed to lower: {failures:?}",
        failures.len(),
        EXPECTED_SUPPORTED_LTM_MODELS.len()
    );
    assert!(
        lowered >= MIN_LTM_MODELS_LOWERED,
        "LTM-on-wasm corpus shrank: only {lowered} of {} expected-supported models lowered, \
         floor is {MIN_LTM_MODELS_LOWERED}",
        EXPECTED_SUPPORTED_LTM_MODELS.len()
    );
}

// ---------------------------------------------------------------------------
// AC3.1: Unsupported LTM model surfaces a clean WasmGenError (no panic)
// ---------------------------------------------------------------------------

/// AC3.1: an LTM model the wasm backend cannot lower returns a clean
/// `WasmGenError::Unsupported` from `compile_datamodel_to_artifact` -- never a
/// panic, never a silently-wrong blob. The fixture combines a small one-stock
/// feedback loop (so LTM genuinely emits link/loop scores) with a non-constant
/// subscript range (`SUM(source[lo:hi])`, GH #612) that the fully-unrolled
/// emitter can't express, mirroring the FFI-side
/// `compile_to_wasm_unsupported_model_surfaces_error` (`libsimlin/tests/wasm.rs`)
/// but loaded from a real XMILE file so the same fixture serves the TS twin.
///
/// The companion `vm_results_for_ltm` assertion proves the model is fine on
/// the bytecode VM -- the limitation is wasm-backend-specific, not a
/// structural model error -- so AC3.2's TS clause (the VM path still
/// simulates) has a single source of truth.
#[test]
fn unsupported_ltm_model_returns_wasmgen_error() {
    let project = load("ltm_dynamic_range_unsupported/model.stmx");

    match compile_datamodel_to_artifact(&project, "main", true, false) {
        Ok(_) => panic!("wasm compile of a dynamic-range LTM model must fail, not succeed"),
        Err(WasmGenError::Unsupported(msg)) => {
            assert!(
                !msg.is_empty(),
                "WasmGenError::Unsupported must carry a non-empty message"
            );
        }
    }

    // The same model simulates fine on the bytecode VM with LTM on (this is
    // a wasm-backend-only limitation; if it ever regresses to "broken on the
    // VM too" the AC3.2 TS clause loses its oracle).
    let vm = vm_results_for_ltm(&project, "main");
    assert!(
        vm.step_count > 0,
        "VM with LTM on must produce at least one saved step for the fixture"
    );
}

/// GH #486: the wasm LTM compile path shares the salsa pipeline, so a non-Euler
/// integration method with LTM enabled surfaces the same Euler-assumption guard
/// as a clean `WasmGenError` -- no silently-wrong LTM slab.
#[test]
fn ltm_non_euler_returns_wasmgen_error() {
    use simlin_engine::test_common::TestProject;

    let project = TestProject::new("ltm_rk4_wasm")
        .with_sim_time(0.0, 10.0, 1.0)
        .with_sim_method(datamodel::SimMethod::RungeKutta4)
        .stock("population", "100", &["births"], &[], None)
        .flow("births", "population * 0.02", None)
        .build_datamodel();

    match compile_datamodel_to_artifact(&project, "main", true, false) {
        Ok(_) => panic!("wasm compile of an RK4 LTM model must fail, not succeed"),
        Err(WasmGenError::Unsupported(msg)) => assert!(
            msg.contains("Euler"),
            "the error must reference the Euler assumption, got: {msg}"
        ),
    }

    // The same model lowers cleanly with LTM disabled (the guard fires only
    // when LTM is requested).
    compile_datamodel_to_artifact(&project, "main", false, false)
        .expect("RK4 model without LTM must lower to wasm");
}

// ---------------------------------------------------------------------------
// AC2.5: discovery-mode loop parity (wasm vs VM)
// ---------------------------------------------------------------------------

/// Canonical-rotation key for a discovered loop, built from its
/// trimmed link sequence (the `(from, to)` ordered tuples reported by
/// `discover_loops_with_graph`).
///
/// Loop polarity is intentionally NOT part of the key: structural
/// polarity is identical between backends (same `causal_graph`), and
/// runtime polarity is derived from `scores` so a small float drift on
/// the boundary could otherwise flip a `MostlyReinforcing` vs
/// `Reinforcing` classification and falsely diverge the identity check.
/// `Loop.id` is also excluded because `rank_and_filter` assigns IDs
/// after a `sort_by(avg_abs_score)` whose tie-breaking is order-of-
/// discovery -- not a stable identity surface.
///
/// The canonical rotation makes the key invariant to the discovery
/// algorithm's choice of starting node on the cycle: two runs that walk
/// the same cycle starting at different stocks still produce the same
/// key.
fn loop_identity_key(loop_info: &simlin_engine::ltm::Loop) -> Vec<(String, String)> {
    let edges: Vec<(String, String)> = loop_info
        .links
        .iter()
        .map(|l| (l.from.as_str().to_string(), l.to.as_str().to_string()))
        .collect();
    canonical_rotation_of_edges(&edges)
}

/// Lexicographically-smallest rotation of an edge sequence. Mirrors the
/// crate-private `simlin_engine::ltm::canonical_rotation` semantics
/// (used inside `discover_loops_with_graph` for its own dedup) so two
/// discovery runs that walk the same cycle starting at different points
/// produce the same identity key.
///
/// Cycles where every edge tuple is identical (degenerate self-loops)
/// return the original sequence unchanged; for normal cycles the unique
/// minimum rotation is well-defined.
fn canonical_rotation_of_edges(edges: &[(String, String)]) -> Vec<(String, String)> {
    if edges.is_empty() {
        return Vec::new();
    }
    let n = edges.len();
    let mut best: Vec<(String, String)> = edges.to_vec();
    for start in 1..n {
        let candidate: Vec<(String, String)> =
            (0..n).map(|i| edges[(start + i) % n].clone()).collect();
        if candidate < best {
            best = candidate;
        }
    }
    best
}

/// Compare two `FoundLoop`s on the same loop identity for per-timestep
/// score-series agreement. Times must be exactly equal (both backends'
/// `Results` carry specs reconstructed from the same compile, so the
/// step-to-time formula `start + save_step * step` produces bit-
/// identical results); scores must agree within
/// `LTM_SERIES_TOLERANCE` using the same relative-or-absolute
/// shape as `assert_ltm_slabs_match`.
fn assert_loop_score_series_match(model_rel_path: &str, vm: &FoundLoop, wasm: &FoundLoop) {
    assert_eq!(
        vm.scores.len(),
        wasm.scores.len(),
        "[{model_rel_path}] loop {} score-series length mismatch: vm={} wasm={}",
        vm.loop_info.id,
        vm.scores.len(),
        wasm.scores.len()
    );
    for (i, ((t_vm, s_vm), (t_wasm, s_wasm))) in
        vm.scores.iter().zip(wasm.scores.iter()).enumerate()
    {
        assert_eq!(
            t_vm, t_wasm,
            "[{model_rel_path}] loop {} step {i}: time axis diverges: vm={t_vm} wasm={t_wasm} \
             (specs should have been reconstructed identically)",
            vm.loop_info.id
        );
        // Both-NaN counts as equal (mirrors `assert_ltm_slabs_match`'s
        // dual-NaN branch). Step 0 of an LTM series can be NaN by
        // design (PREVIOUS bootstrap) and propagates into the loop
        // score product; a single-sided NaN is a real divergence.
        if s_vm.is_nan() && s_wasm.is_nan() {
            continue;
        }
        let max_abs = s_vm.abs().max(s_wasm.abs()).max(1.0);
        let epsilon = LTM_SERIES_TOLERANCE * max_abs;
        let diff = (s_vm - s_wasm).abs();
        assert!(
            diff <= epsilon,
            "[{model_rel_path}] loop {} step {i} (t={t_vm}): score diverges by {diff}: \
             vm={s_vm} wasm={s_wasm} (epsilon={epsilon})",
            vm.loop_info.id,
        );
    }
}

/// Drive `discover_loops_with_graph` over both a VM-produced `Results`
/// and a wasm-produced discovery-mode `Results` -- with the same
/// structural inputs (assembled by `ltm_discovery_inputs` so the
/// causal graph, stocks, ltm-vars, and dims come from a single source)
/// -- and assert that the discovered loop **sets** match (by canonical
/// edge-sequence key) and that every matched loop's per-timestep
/// `(time, score)` series agree.
///
/// `ltm_finding::FoundLoop.loop_info.id` is *not* part of the identity
/// key: `rank_and_filter` assigns IDs after a score-driven sort whose
/// tie-breaking is order-of-discovery, not a stable surface to lock to.
/// `loop_identity_key` keys on the trimmed link sequence (canonical
/// rotation), which `discover_loops_with_graph` itself uses for
/// internal dedup -- the most natural identity surface for a directed
/// cycle.
fn assert_discovery_matches(model_rel_path: &str) {
    let project = load(model_rel_path);
    let inputs = ltm_discovery_inputs(&project, "main");
    let wasm = wasm_results_for_ltm_discovery(&project, "main").unwrap_or_else(|msg| {
        panic!("discovery-mode LTM model {model_rel_path} should lower to wasm: {msg}")
    });

    // Sanity guard: the wasm `Results` must actually carry LTM link-
    // score series, otherwise discovery would emit an empty loop set on
    // the wasm side and a "0 loops" agreement would pass vacuously.
    let wasm_has_ltm = wasm
        .offsets
        .keys()
        .any(|k| k.as_str().starts_with(LTM_LINK_SCORE_PREFIX));
    assert!(
        wasm_has_ltm,
        "[{model_rel_path}] wasm discovery `Results` carries no $⁚ltm⁚link_score⁚* keys -- \
         silent regression to LTM-off?"
    );

    let vm_loops = ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        None,
    )
    .unwrap_or_else(|e| panic!("[{model_rel_path}] VM discovery returned Err: {e:?}"))
    .loops;

    let wasm_loops = ltm_finding::discover_loops_with_graph(
        &wasm,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        None,
    )
    .unwrap_or_else(|e| panic!("[{model_rel_path}] wasm discovery returned Err: {e:?}"))
    .loops;

    assert!(
        !vm_loops.is_empty(),
        "[{model_rel_path}] VM discovery produced an empty loop set -- nothing to compare against"
    );

    // Identity-keyed maps for set comparison and pair-up.
    use std::collections::HashMap;
    let mut vm_by_key: HashMap<Vec<(String, String)>, &FoundLoop> = HashMap::new();
    for fl in &vm_loops {
        let key = loop_identity_key(&fl.loop_info);
        vm_by_key.insert(key, fl);
    }
    let mut wasm_by_key: HashMap<Vec<(String, String)>, &FoundLoop> = HashMap::new();
    for fl in &wasm_loops {
        let key = loop_identity_key(&fl.loop_info);
        wasm_by_key.insert(key, fl);
    }

    let vm_keys: std::collections::HashSet<_> = vm_by_key.keys().cloned().collect();
    let wasm_keys: std::collections::HashSet<_> = wasm_by_key.keys().cloned().collect();

    let only_vm: Vec<_> = vm_keys.difference(&wasm_keys).cloned().collect();
    let only_wasm: Vec<_> = wasm_keys.difference(&vm_keys).cloned().collect();
    assert!(
        only_vm.is_empty() && only_wasm.is_empty(),
        "[{model_rel_path}] discovered loop sets diverge -- only-VM={only_vm:?}, only-wasm={only_wasm:?}"
    );
    assert_eq!(
        vm_loops.len(),
        wasm_loops.len(),
        "[{model_rel_path}] discovered loop counts diverge: vm={} wasm={} \
         (sets matched by canonical edge key but length mismatch suggests duplicate keys)",
        vm_loops.len(),
        wasm_loops.len()
    );

    for (key, vm_fl) in &vm_by_key {
        let wasm_fl = wasm_by_key
            .get(key)
            .expect("set difference was empty so every VM key must be in the wasm map");
        assert_loop_score_series_match(model_rel_path, vm_fl, wasm_fl);
    }
}

/// AC2.5: small-corpus parity. arms_race_3party is tractable (a
/// 3-party arms race with a handful of loops, fast to simulate and
/// discover) and exercises the full element-level discovery path
/// (`discover_loops_with_graph` over the salsa-built element graph
/// with populated `ltm_vars` / `dims`).
#[test]
fn discovery_arms_race_matches_vm() {
    assert_discovery_matches("arms_race_3party/arms_race.stmx");
}

/// AC2.5 (heavy ignored twin): C-LEARN v77 discovery parity.
///
/// `#[ignore]`d because compiling C-LEARN's 1.4 MB MDL to wasm is slow
/// (the wasm compile is heavier than the VM compile this test's
/// VM-side peer `clearn_ltm_discovery_compiles` already runs at
/// `#[ignore]`); running it counts against the 3-minute
/// `cargo test --workspace` cap if left in the default suite. Run
/// explicitly with:
///     cargo test --release -p simlin-engine \
///         --features file_io --test simulate_ltm_wasm -- --ignored \
///         --nocapture discovery_clearn_matches_vm_wasm
///
/// The model lives outside the XMILE loader's `test/` root (it is a
/// Vensim MDL under `test/xmutil_test_models/`), so it is loaded via
/// `open_vensim` directly rather than through this binary's
/// XMILE-only `load` helper.
#[test]
#[ignore = "wasm compile of 1.4 MB MDL is slow; run with `cargo test --release -- --ignored`"]
fn discovery_clearn_matches_vm_wasm() {
    let path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let mdl =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let project = simlin_engine::open_vensim(&mdl)
        .unwrap_or_else(|e| panic!("open_vensim should parse C-LEARN: {e:?}"));
    discovery_matches_helper(&project, path);
}

/// AC2.5 (heavy ignored twin): World3-03 discovery parity.
///
/// `#[ignore]`d because (a) wasm-compiling a 166-variable Vensim
/// model is slow, and (b) the VM-side discovery on World3 currently
/// does not terminate -- see `world3_discovery_single_timestep` for
/// the documented Finding 3 (GH #540) and the surrounding RSS/time-
/// budget bound. Until #540 is fixed the VM run alone exhausts the
/// budget, so this twin cannot complete; once #540 is fixed the
/// twin's VM and wasm runs both terminate and this test asserts they
/// agree on the discovered loop set and per-loop score series.
///
/// Run explicitly with:
///     cargo test --release -p simlin-engine \
///         --features file_io --test simulate_ltm_wasm -- --ignored \
///         --nocapture discovery_world3_matches_vm_wasm
#[test]
#[ignore = "wasm compile of 166-var World3 is slow; VM discovery is also blocked on #540"]
fn discovery_world3_matches_vm_wasm() {
    let path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";
    let mdl =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let project = simlin_engine::open_vensim(&mdl)
        .unwrap_or_else(|e| panic!("open_vensim should parse World3: {e:?}"));
    discovery_matches_helper(&project, path);
}

/// Project-borrowing peer of `assert_discovery_matches`. The arms-race
/// fast test uses `assert_discovery_matches` (which loads via the
/// XMILE-only `load` helper); the Vensim MDL heavy twins load the
/// project themselves (different file root) and pass it in here. The
/// body is otherwise the same -- the wasm-vs-VM identity-and-scores
/// comparison.
fn discovery_matches_helper(project: &datamodel::Project, label: &str) {
    let inputs = ltm_discovery_inputs(project, "main");
    let wasm = wasm_results_for_ltm_discovery(project, "main").unwrap_or_else(|msg| {
        panic!("discovery-mode LTM model {label} should lower to wasm: {msg}")
    });

    let wasm_has_ltm = wasm
        .offsets
        .keys()
        .any(|k| k.as_str().starts_with(LTM_LINK_SCORE_PREFIX));
    assert!(
        wasm_has_ltm,
        "[{label}] wasm discovery `Results` carries no $⁚ltm⁚link_score⁚* keys"
    );

    let vm_loops = ltm_finding::discover_loops_with_graph(
        &inputs.vm_results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        None,
    )
    .unwrap_or_else(|e| panic!("[{label}] VM discovery returned Err: {e:?}"))
    .loops;
    let wasm_loops = ltm_finding::discover_loops_with_graph(
        &wasm,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        None,
    )
    .unwrap_or_else(|e| panic!("[{label}] wasm discovery returned Err: {e:?}"))
    .loops;

    assert!(
        !vm_loops.is_empty(),
        "[{label}] VM discovery produced an empty loop set -- nothing to compare against"
    );

    use std::collections::{HashMap, HashSet};
    let vm_by_key: HashMap<Vec<(String, String)>, &FoundLoop> = vm_loops
        .iter()
        .map(|fl| (loop_identity_key(&fl.loop_info), fl))
        .collect();
    let wasm_by_key: HashMap<Vec<(String, String)>, &FoundLoop> = wasm_loops
        .iter()
        .map(|fl| (loop_identity_key(&fl.loop_info), fl))
        .collect();
    let vm_keys: HashSet<_> = vm_by_key.keys().cloned().collect();
    let wasm_keys: HashSet<_> = wasm_by_key.keys().cloned().collect();
    let only_vm: Vec<_> = vm_keys.difference(&wasm_keys).cloned().collect();
    let only_wasm: Vec<_> = wasm_keys.difference(&vm_keys).cloned().collect();
    assert!(
        only_vm.is_empty() && only_wasm.is_empty(),
        "[{label}] discovered loop sets diverge -- only-VM={only_vm:?}, only-wasm={only_wasm:?}"
    );
    assert_eq!(vm_loops.len(), wasm_loops.len());
    for (key, vm_fl) in &vm_by_key {
        let wasm_fl = wasm_by_key.get(key).unwrap();
        assert_loop_score_series_match(label, vm_fl, wasm_fl);
    }
}
