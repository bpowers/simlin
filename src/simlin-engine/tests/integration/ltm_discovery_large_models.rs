// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end discovery-mode LTM tests for large real-world models
//! (World3, C-LEARN), plus a fast tractable-model companion that proves
//! the contract those tests assert is actually satisfiable.
//!
//! These tests pin that LTM discovery is *tractable* on the
//! element-level production path: the World3 test below runs a single
//! discoverable timestep end to end and the companion runs the identical
//! recipe on a small arrayed model.
//!
//! ## Background
//!
//! LTM has two loop-finding modes. The *exhaustive* mode (Johnson
//! elementary-circuit enumeration) is gated by `MAX_LTM_SCC_NODES = 50`:
//! a model whose variable-level causal-graph SCC exceeds that threshold
//! auto-flips to *discovery* mode -- post-simulation loop finding over the
//! recorded link scores, implemented in
//! `ltm_finding::discover_loops_with_graph`. World3's variable-level SCC is
//! 166, so it auto-flips to discovery.
//!
//! Nothing else in the suite exercises discovery on a large model:
//! `tests/wrld3_ltm_panic.rs` only checks that LTM *compilation*
//! finishes, never that discovery runs.
//!
//! ## Production path
//!
//! `world3_discovery_single_timestep` drives the *actual production
//! path*: `ltm_finding::discover_loops_with_graph` with the
//! **element-level** causal graph and populated `ltm_vars` / `dims`,
//! assembled exactly as `analysis.rs::run_ltm_pipeline` does (the call
//! site behind `analysis::analyze_model`). It deliberately does NOT run on
//! the *variable-level* graph with empty `ltm_vars` / `dims`, which is a
//! strict minor of the production graph -- a discovery algorithm made
//! tractable on the variable graph could still be intractable on the
//! element graph, and a variable-level test would not catch that.
//!
//! ## The startup guard
//!
//! LTM link scores are `PREVIOUS()`-based, so the first saved timestep
//! (index 0) is startup-degenerate: every score is pinned to 0 there by its
//! `TIME <= INITIAL_TIME` guard, step 0's link scores can be NaN -- both
//! discovery generators skip step 0 for exactly that reason -- and it
//! carries no positive loop contribution, so `rank_and_filter` drops every
//! loop whose only timestep is that one. Step index 1 is the first
//! genuinely discoverable timestep: every score reads one step of history,
//! the flow-to-stock score included (it is `|Δflow / Δnet|` over the
//! stock's net-flow aux, with no stock history behind it).
//! `FIRST_DISCOVERABLE_STEP` and `TRUNCATED_STEP_COUNT` encode this; the
//! discovery tests truncate results to `TRUNCATED_STEP_COUNT` (2) so the
//! window holds exactly one discoverable timestep, and
//! `assert_discovery_contract` only inspects score values at step indices
//! `>= FIRST_DISCOVERABLE_STEP`.
//!
//! ## Discovery is tractable on World3 (was GH #540, now closed)
//!
//! Discovery on World3 used to be intractable: on a release build of
//! `dbc0844e` (after Finding 1's `1/dt` link-score fix), full discovery
//! over all 401 saved timesteps ran 390+ seconds and allocated 22+ GB
//! before a hard kill, and even a single discoverable timestep of the
//! element-level production path did not finish within a 20 s budget.
//! The blow-up was in the candidate search: it pruned on the assumption
//! that path-score products shrink as paths extend, and on World3's dense
//! 166-node SCC with link scores straddling 1.0 that assumption does not
//! hold, so the search re-explored subtrees super-polynomially and
//! accumulated loops without bound.
//!
//! The discovery rewrite (commit `081e9848`) fixed this; GH #540 is
//! closed. World3 discovery completes quickly: the single discoverable
//! timestep of the element-level production path that
//! `world3_discovery_single_timestep` exercises finishes in roughly 20 ms
//! of pure discovery work (a sub-second test run end to end, dominated by
//! the MDL parse and the 401-step simulate, not by discovery). The phases
//! leading up to discovery are likewise fast (parse, discovery-mode
//! compile, simulate). For reference, Eberlein & Schoenberg report
//! 10-20 s for Urban Dynamics, a model *larger* than World3.
//!
//! ## Test layout
//!
//! - `world3_discovery_single_timestep` (NOT `#[ignore]`d): drives the
//!   element-level production path over a single discoverable timestep of
//!   World3 and asserts the contract discovery must satisfy
//!   (`assert_discovery_contract`). It pins that discovery *is* tractable:
//!   it both runs and passes, and a regression to non-termination would
//!   trip its generous wall-clock guard. Kept un-`#[ignore]`d because the
//!   whole run is sub-second on a debug build (within the per-test budget).
//! - `discovery_contract_holds_on_tractable_arrayed_model` (NOT
//!   `#[ignore]`d, fast): runs the *same* element-level recipe and the
//!   *same* `assert_discovery_contract` on a small tractable arrayed
//!   model. It is the live guard on the startup-guard constants: if the
//!   `PREVIOUS`-based startup-guard width changes it fails loudly and
//!   fast, and it exercises the per-element (A2A) expansion path on a
//!   model small enough to keep the assertion cheap even if World3's
//!   fixture ever drifts.
//! - `clearn_leading_loops_keep_their_shares` (`#[ignore]`d): the loops that
//!   lead C-LEARN's run, as `analysis::analyze_model` reports them, against a
//!   table of their shares. It is the test of loop analysis on a model of
//!   this size: a change that zeroes a link score, loses a loop or reads the
//!   wrong element moves a row.
//! - `clearn_ltm_discovery_compiles` (`#[ignore]`d): asserts C-LEARN
//!   compiles via the incremental path with LTM discovery enabled (a
//!   clean `Ok`, no panic), re-verifying GH #363. It asserts only the
//!   compile result, not discovery tractability. It stays `#[ignore]`d
//!   for its runtime class (parsing C-LEARN's 1.4 MB MDL plus a
//!   discovery-mode compile runs tens of seconds in the debug build), not
//!   for any discovery-intractability reason.

use std::time::{Duration, Instant};

use simlin_engine::analysis::{LoopSummary, analyze_model};
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::datamodel;
use simlin_engine::db::{
    LtmSyntheticVar, SimlinDb, compile_project_incremental, set_project_ltm_discovery_mode,
    sync_from_datamodel_incremental,
};
use simlin_engine::ltm::CausalGraph;
use simlin_engine::test_common::TestProject;
use simlin_engine::{Results, ltm_finding, open_vensim};

use crate::test_helpers::ltm_discovery_inputs;

/// World3-03 in Vensim MDL form. Paths are relative to the
/// `simlin-engine` crate directory (where `cargo test` runs).
const WORLD3_MDL: &str = "../../test/metasd/WRLD3-03/wrld3-03.mdl";

/// C-LEARN v77, a large Vensim model. It compiles via the incremental
/// path with LTM discovery enabled (Vensim macro support is complete --
/// `SAMPLE UNTIL`, `SSHAPE` and the rest inline through the converter, so
/// the old GH #349 "macros not inlined" blocker no longer applies). GH
/// #363 (the incremental compiler historically panicking on C-LEARN
/// rather than returning a clean error) is re-verified by
/// `clearn_ltm_discovery_compiles`: the panic does not reproduce on this
/// path.
const CLEARN_MDL: &str = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";

/// C-LEARN as it opens.
fn open_clearn() -> datamodel::Project {
    let mdl = std::fs::read_to_string(CLEARN_MDL)
        .unwrap_or_else(|e| panic!("failed to read {CLEARN_MDL}: {e}"));
    open_vensim(&mdl).expect("open_vensim should parse C-LEARN v77 for Vensim.mdl")
}

/// Index of the first genuinely discoverable saved timestep.
///
/// LTM link scores are `PREVIOUS()`-based: steps 0 and 1 are
/// startup-degenerate (step 0's link scores are guarded to 0 and can be
/// NaN; it carries no positive loop contribution). Step index 1 is the
/// first timestep whose link scores -- and therefore discovered loop scores
/// -- are meaningful. See the module-level "startup guard" section.
const FIRST_DISCOVERABLE_STEP: usize = 1;

/// Number of saved timesteps to keep when truncating results for a
/// single-discoverable-timestep discovery run: the two startup-guard
/// steps (0, 1) plus the first discoverable timestep (`= 2`). A window
/// of exactly this size forces `discover_loops_with_graph` through one
/// real discoverable step and is the smallest window in which a correct
/// discovery still returns a non-empty loop set.
const TRUNCATED_STEP_COUNT: usize = FIRST_DISCOVERABLE_STEP + 1;

/// Owned, `'static` inputs for the element-level discovery production
/// path. `discover_loops_with_graph` borrows all of these by reference;
/// bundling them as owned values keeps them as one decoupled unit,
/// independent of the `SimlinDb` they were derived from.
struct DiscoveryInputs {
    results: Results,
    causal_graph: CausalGraph,
    stocks: Vec<Ident<Canonical>>,
    ltm_vars: Vec<LtmSyntheticVar>,
    dims: Vec<datamodel::Dimension>,
    expansion: simlin_engine::ltm_finding::LinkExpansionContext,
    sub_model_output_ports: simlin_engine::ltm_finding::SubModelOutputPorts,
}

/// Compile (LTM discovery mode), simulate, and assemble the
/// element-level discovery inputs for an arbitrary datamodel project.
///
/// Thin wrapper over the shared `test_helpers::ltm_discovery_inputs`
/// builder: the structural-input body lives in exactly one place
/// (across `simulate_ltm_wasm.rs` and this file's binary), satisfying
/// the anti-divergence principle at the harness level so the wasm-vs-VM
/// parity test in Phase 5 truly compares identical inputs to what this
/// VM-side bundle exercises.
///
/// Always targets the canonical "main" model; the World3 fixture and
/// the tractable companion fixture both bind their top-level model to
/// that name.
fn discovery_inputs(datamodel_project: &datamodel::Project) -> DiscoveryInputs {
    let shared = ltm_discovery_inputs(datamodel_project, "main");
    DiscoveryInputs {
        results: shared.vm_results,
        causal_graph: shared.causal_graph,
        stocks: shared.stocks,
        ltm_vars: shared.ltm_vars,
        dims: shared.dims,
        expansion: shared.expansion,
        sub_model_output_ports: shared.sub_model_output_ports,
    }
}

/// Parse, compile, simulate World3 and assemble its element-level
/// discovery inputs. Thin wrapper over `discovery_inputs`.
///
/// Every phase here is fast (sub-second total), including the
/// `discover_loops_with_graph` pass the caller drives.
fn world3_discovery_inputs() -> DiscoveryInputs {
    let mdl = std::fs::read_to_string(WORLD3_MDL).expect(
        "failed to read wrld3-03.mdl -- run tests from the repo root or the simlin-engine crate",
    );
    let datamodel_project = open_vensim(&mdl).expect("open_vensim should parse wrld3-03.mdl");
    discovery_inputs(&datamodel_project)
}

/// Return a copy of `results` truncated to the first `n_steps` saved
/// timesteps.
///
/// Discovery's candidate generators both range over `1..step_count` of the
/// recorded series. A truncated copy lets a test exercise one discoverable
/// timestep on real link scores while keeping the run within the per-test
/// time budget.
fn truncate_results(results: &Results, n_steps: usize) -> Results {
    let n = n_steps.min(results.step_count);
    let data: Box<[f64]> = results.data[..n * results.step_size]
        .to_vec()
        .into_boxed_slice();
    Results {
        offsets: results.offsets.clone(),
        data,
        step_size: results.step_size,
        step_count: n,
        specs: results.specs.clone(),
        is_vensim: results.is_vensim,
    }
}

/// The structural contract every successful `discover_loops_with_graph`
/// run must satisfy on a results window truncated to
/// `TRUNCATED_STEP_COUNT`.
///
/// This is the *shared* assertion logic for both discovery tests: the
/// World3 test asserts it over a single discoverable timestep of the
/// element-level production path, and the fast tractable companion test
/// asserts the very same thing on a small arrayed model. Both pass; the
/// shared contract keeps them honestly comparable.
///
/// The checks are deliberately *structural* only -- a non-empty loop
/// set, each loop carrying links, and finite `(time, score)` samples at
/// the discoverable timesteps. They do NOT assert score *correctness*:
/// World3's LTM link scores into graphical-function targets currently
/// stub silently to 0 (a separate known bug, the "GF-table" finding from
/// the same deep review), which corrupts many World3 loop scores.
/// Asserting correct score values here would couple these tests to that
/// unrelated bug.
///
/// Score finiteness is checked only at step indices `>=
/// FIRST_DISCOVERABLE_STEP`: step 0 is startup-degenerate and is allowed
/// to be NaN by the LTM algorithm (see the module-level "startup guard"
/// section), so asserting finiteness there would be asserting on behavior
/// the algorithm treats as undefined.
fn assert_discovery_contract(found: &[ltm_finding::FoundLoop]) {
    assert!(
        !found.is_empty(),
        "discovery produced an empty loop set (expected at least one loop \
         from a window containing a discoverable timestep)"
    );
    for fl in found {
        assert!(
            !fl.loop_info.links.is_empty(),
            "discovered loop {} has no links",
            fl.loop_info.id
        );
        // `scores` carries one entry per saved timestep in the truncated
        // window; the window is sized so exactly one of them is
        // discoverable.
        assert_eq!(
            fl.scores.len(),
            TRUNCATED_STEP_COUNT,
            "discovered loop {} has {} score samples, expected {} \
             (one per truncated timestep)",
            fl.loop_info.id,
            fl.scores.len(),
            TRUNCATED_STEP_COUNT
        );
        for (i, (t, s)) in fl.scores.iter().enumerate().skip(FIRST_DISCOVERABLE_STEP) {
            assert!(
                t.is_finite() && s.is_finite(),
                "discovered loop {} has a non-finite (time, score) sample at \
                 discoverable step {i}: ({t}, {s})",
                fl.loop_info.id
            );
        }
        // Unconditional, not discoverable-step-scoped like the per-step
        // `scores` check above. `avg_abs_score` is a scalar, not a
        // per-step series: a NaN step drops out of both the sum and the
        // count, and the `valid_count == 0` case returns `0.0` rather
        // than dividing (`ltm_finding.rs:793-826`); and link-score
        // equations are SAFEDIV-based (the `ABS(SAFEDIV(...))` forms in
        // `ltm_augment.rs`), so no link-score value -- hence no
        // `loop_score` product, hence no summand -- is `±inf`. So it is
        // structurally finite. ("Non-NaN" is wider than "discoverable":
        // step 1 is non-NaN and does feed the average, so a
        // `.skip(FIRST_DISCOVERABLE_STEP)` framing would be wrong here
        // regardless.)
        assert!(
            fl.avg_abs_score.is_finite(),
            "discovered loop {} has a non-finite avg_abs_score",
            fl.loop_info.id
        );
    }
}

/// The contract `discover_loops_with_graph` satisfies on World3,
/// exercised over a single discoverable timestep of the **element-level
/// production path** (a bounded variant of the full end-to-end run --
/// see the module docs).
///
/// This pins that World3 discovery is tractable (was GH #540, fixed by
/// the discovery rewrite in commit `081e9848` and now closed): the single
/// discoverable timestep runs in ~20 ms of discovery work and the whole
/// test is sub-second on a debug build, so it stays in the default test
/// run rather than `#[ignore]`d. A regression to the old non-terminating
/// behaviour would blow past the generous wall-clock guard below and fail
/// loudly.
///
/// Why a single discoverable timestep rather than the full 401-step run:
/// the per-timestep candidate search was the historical blow-up, so one
/// discoverable timestep is the smallest honest exercise of the production
/// search on real link scores while keeping the whole test within the
/// per-test time budget. The full-run companion below
/// (`world3_full_run_enumeration_is_complete`) is `#[ignore]`d for
/// runtime class.
///
/// Why the element-level path: `analysis::analyze_model` (the production
/// caller) runs `discover_loops_with_graph` on the element-level graph
/// with populated `ltm_vars` / `dims`. A variable-level graph with empty
/// metadata is a strict minor of the production graph, so a
/// variable-level test would not exercise the production element-level
/// path.
#[test]
fn world3_discovery_single_timestep() {
    let inputs = world3_discovery_inputs();
    let total_steps = inputs.results.step_count;
    assert!(
        total_steps >= TRUNCATED_STEP_COUNT,
        "World3 must simulate at least {TRUNCATED_STEP_COUNT} saved timesteps, got {total_steps}"
    );

    // Truncate to steps 0, 1, 2: the two startup-guard steps plus the
    // first genuinely discoverable timestep (step 2). This drives
    // `discover_loops_with_graph` through exactly one real discoverable
    // step of World3's element-level graph.
    let truncated = truncate_results(&inputs.results, TRUNCATED_STEP_COUNT);
    let DiscoveryInputs {
        results: _,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
        expansion,
        sub_model_output_ports,
    } = inputs;

    // A wall-clock regression guard, not a real time budget: discovery on
    // this window completes in tens of milliseconds (see the eprintln
    // below), so a run that takes whole seconds means discovery has
    // regressed toward the old GH #540 non-termination. Kept generous so
    // the guard never flakes on a loaded CI box.
    let budget = Duration::from_secs(20);

    let t = Instant::now();
    let found = ltm_finding::discover_loops_with_graph(
        &truncated,
        &causal_graph,
        &stocks,
        &ltm_vars,
        &dims,
        &expansion,
        &sub_model_output_ports,
        None,
    )
    .expect("discover_loops_with_graph should not error on World3")
    .loops;
    let elapsed = t.elapsed();

    eprintln!(
        "World3 single-timestep element-level discovery: {} loops in {elapsed:?}",
        found.len(),
    );
    assert!(
        elapsed < budget,
        "World3 single-timestep discovery took {elapsed:?} (> {budget:?}); discovery may \
         have regressed toward the old GH #540 non-termination"
    );

    // The same structural contract the tractable companion test proves is
    // satisfiable.
    assert_discovery_contract(&found);
}

/// Asserts `assert_discovery_contract` -- the same contract
/// `world3_discovery_single_timestep` asserts -- via the exact same
/// element-level `discover_loops_with_graph` recipe, on a small tractable
/// arrayed model.
///
/// The two discovery tests are deliberately redundant on the contract but
/// complementary on cost and signal: World3 exercises the real
/// production fixture (large, dense SCC), while this companion is tiny
/// and fast, so it keeps the shared contract under cheap continuous
/// coverage and is the natural place to assert the arrayed/per-element
/// expansion path and the startup-guard constants without depending on
/// the World3 MDL fixture. It runs the identical recipe --
/// `discovery_inputs` -> `truncate_results(.., TRUNCATED_STEP_COUNT)` ->
/// `discover_loops_with_graph` -> `assert_discovery_contract` -- on a
/// small *arrayed* model and PASSES.
///
/// The model is arrayed on purpose: arrayed variables make `dims`
/// non-empty and produce A2A (dimensioned) `LtmSyntheticVar`s, so
/// `discover_loops_with_graph` genuinely exercises its per-element link
/// score expansion path -- the same path World3 hits -- rather than a
/// scalar-only shortcut. The test asserts that `dims` / `ltm_vars` are
/// in fact populated, so a future change that quietly scalarizes the
/// fixture fails loudly here.
///
/// It is also a live guard on the startup-guard constants: if the LTM
/// `PREVIOUS`-based startup-guard width ever changes,
/// `FIRST_DISCOVERABLE_STEP` / `TRUNCATED_STEP_COUNT` go stale, the
/// truncated window stops holding a discoverable timestep, and this test
/// fails loudly and fast on a fixture small enough to diagnose quickly.
///
/// Not `#[ignore]`d: the model is tiny and discovery on it is
/// sub-millisecond, so it is safe for the `cargo test --workspace`
/// budget.
#[test]
fn discovery_contract_holds_on_tractable_arrayed_model() {
    // Small arrayed model: an isolated per-element feedback loop
    // `pop[region] -> growth[region] -> pop[region]` over a two-element
    // dimension. Arrayed so the compiled LTM link scores are A2A and
    // `discover_loops_with_graph` exercises its per-element expansion
    // path; the SCC is tiny (one 2-node cycle per element), so discovery
    // is tractable and finishes in well under a millisecond.
    let project = TestProject::new("companion")
        .with_sim_time(0.0, 6.0, 1.0)
        .named_dimension("region", &["north", "south"])
        .array_stock("pop[region]", "100", &["growth"], &[], None)
        .array_flow("growth[region]", "pop[region] * 0.1", None)
        .build_datamodel();

    let inputs = discovery_inputs(&project);
    assert!(
        inputs.results.step_count >= TRUNCATED_STEP_COUNT,
        "tractable fixture must simulate at least {TRUNCATED_STEP_COUNT} saved \
         timesteps, got {}",
        inputs.results.step_count
    );
    // The fixture is only a faithful proxy for World3 if it actually
    // drives the arrayed/element-level path: `dims` must be populated and
    // at least one LTM synthetic var must be dimensioned (A2A).
    assert!(
        !inputs.dims.is_empty(),
        "arrayed fixture must yield non-empty project dimensions"
    );
    assert!(
        inputs.ltm_vars.iter().any(|v| !v.dimensions.is_empty()),
        "arrayed fixture must yield at least one A2A (dimensioned) LTM synthetic var, \
         otherwise it does not exercise the element-level expansion path"
    );

    let truncated = truncate_results(&inputs.results, TRUNCATED_STEP_COUNT);
    let DiscoveryInputs {
        results: _,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
        expansion,
        sub_model_output_ports,
    } = inputs;

    let found = ltm_finding::discover_loops_with_graph(
        &truncated,
        &causal_graph,
        &stocks,
        &ltm_vars,
        &dims,
        &expansion,
        &sub_model_output_ports,
        None,
    )
    .expect("discovery on the tractable arrayed fixture should not error")
    .loops;

    assert_discovery_contract(&found);
}

/// C-LEARN compiles via the incremental path with LTM discovery enabled.
///
/// C-LEARN v77 uses Vensim macros (`SAMPLE UNTIL`, `SSHAPE`); macro
/// support is now complete (they inline through the converter -- see
/// `corpus_clearn_macros_import`), so the old GH #349 "macros parsed but
/// not inlined" blocker no longer applies. This test re-verifies GH #363
/// (the incremental compiler historically panicking on C-LEARN rather
/// than returning a clean error): the panic does not reproduce on this
/// path.
///
/// It parses C-LEARN, then compiles it via the incremental salsa path
/// with **LTM discovery enabled** (`LtmOverlay::On` +
/// `set_project_ltm_discovery_mode(true)` -- a heavier path than a plain
/// compile, exercising loop discovery/analysis) and asserts the compile
/// returns `Ok`. The compile is NOT wrapped in `catch_unwind`: per AC7.5
/// a panic here would be a hard, root-caused test failure (the GH #363
/// symptom), never silently caught.
///
/// Scope: this asserts only a clean compile result (AC7.4). It does not
/// run `discover_loops_with_graph` or assert discovery
/// tractability/structural sanity on C-LEARN -- that broader coverage is
/// out of scope here and would be tracked separately.
///
/// `#[ignore]`d for the `cargo test --workspace` time budget: parsing
/// C-LEARN's 1.4 MB MDL plus a discovery-mode compile runs about a second
/// optimized and 15 s on a debug build. Run with:
///     scripts/gates.sh --nocapture clearn_ltm_discovery_compiles
#[test]
#[ignore = "C-LEARN compiled in LTM discovery mode; run under the gates profile"]
fn clearn_ltm_discovery_compiles() {
    let datamodel_project = open_clearn();

    // Compile via the incremental path with LTM discovery enabled. NOT
    // wrapped in `catch_unwind`: per AC7.5 a panic here is a hard,
    // root-caused failure (the GH #363 symptom re-verified), never caught.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);

    compile_project_incremental(&db, sync.project, "main", simlin_engine::db::LtmOverlay::On)
        .unwrap_or_else(|e| {
            panic!("C-LEARN should compile with LTM discovery enabled, got Err: {e:?}")
        });
}

/// The union-graph enumeration path on a FULL World3 run: candidate
/// generation must complete (`enumeration_complete`), so the reported 200
/// (the `MAX_LOOPS` cap binds -- World3's ever-simultaneously-active universe
/// holds ~150k elementary cycles, ~3k of which pass retention; the larger
/// ~330k figure is its cycle count WITHOUT the activity constraint, which the
/// enumerator never materializes) are the exact retention/ranking selection
/// rather than a sample. The 2026-08-10 ground-truth audit's smoking gun --
/// the births -> population-aging chain loops, which peak at 27% partition
/// dominance yet were entirely absent from the sampled report -- must be
/// present.
///
/// `#[ignore]`d for runtime only: a full 401-step simulate plus the
/// ~150k-circuit enumeration and its scoring passes, well over the per-test
/// budget on a debug build. Run with
/// `scripts/gates.sh world3_full_run_enumeration_is_complete`.
#[test]
#[ignore = "World3 simulated under LTM and every circuit enumerated and scored; run under the gates profile"]
fn world3_full_run_enumeration_is_complete() {
    let inputs = world3_discovery_inputs();
    let found = ltm_finding::discover_loops_with_graph(
        &inputs.results,
        &inputs.causal_graph,
        &inputs.stocks,
        &inputs.ltm_vars,
        &inputs.dims,
        &inputs.expansion,
        &inputs.sub_model_output_ports,
        None,
    )
    .expect("discovery must succeed on World3");

    assert!(
        found.enumeration_complete,
        "World3's ~150k-circuit ever-simultaneously-active universe is well \
         inside the enumeration budgets; a fallback here is a regression"
    );
    assert_eq!(
        found.loops.len(),
        200,
        "the MAX_LOOPS cap binds on World3 (~3k retention-passing loops)"
    );
    assert!(
        found.loops.iter().any(|fl| {
            fl.loop_info
                .links
                .iter()
                .any(|l| l.from.as_str() == "maturation_14_to_15")
        }),
        "a births -> population-aging loop (via maturation_14_to_15) must be \
         reported; the pre-enumeration sampler missed this family entirely \
         (peak 27% partition dominance -- see the 2026-08-10 audit)"
    );
}

/// One of the loops that lead C-LEARN's run, and what loop analysis reports of
/// it at each element of the climate-sensitivity dimension it runs over.
struct LeadingLoop {
    /// What the loop does, for a failure to say.
    name: &'static str,
    polarity: &'static str,
    /// The variables it runs through, in order.
    path: &'static [&'static str],
    /// For each element: the largest share of its partition's loop scores the
    /// loop reaches over the run, and its mean share.
    shares: &'static [(&'static str, f64, f64)],
}

/// The three loops with the largest mean share of their partition, strongest
/// first. C-LEARN runs its carbon cycle once per climate sensitivity, so each
/// is one loop per element, in that element's partition.
const CLEARN_LEADING_LOOPS: &[LeadingLoop] = &[
    LeadingLoop {
        name: "the mixed layer takes up carbon",
        polarity: "balancing",
        path: &[
            "c_in_mixed_layer",
            "flux_atm_to_ocean",
            "c_in_mixed_layer_net_flow",
        ],
        shares: &[
            ("deterministic", 0.7648, 0.2043),
            ("high_2xco2_sensitivity", 0.7535, 0.2130),
            ("low_2xco2_sensitivity", 0.7530, 0.2009),
        ],
    },
    LeadingLoop {
        name: "warming slows the biomass's uptake of carbon",
        polarity: "balancing",
        path: &[
            "total_radiative_forcing",
            "heat_in_atmosphere_and_upper_ocean_net_flow",
            "heat_in_atmosphere_and_upper_ocean",
            "temperature_change_from_preindustrial",
            "effect_of_warming_on_c_flux_to_biomass",
            "flux_atm_to_biomass",
            "c_in_biomass",
            "flux_biomass_to_humus",
            "c_in_humus",
            "flux_humus_to_atmosphere",
            "c_in_atmosphere",
            "co2_radiative_forcing",
        ],
        shares: &[
            ("deterministic", 0.6706, 0.0866),
            ("high_2xco2_sensitivity", 0.7127, 0.0877),
            ("low_2xco2_sensitivity", 0.6288, 0.0799),
        ],
    },
    LeadingLoop {
        name: "carbon cycles through biomass and humus",
        polarity: "reinforcing",
        path: &[
            "flux_biomass_to_humus",
            "c_in_humus",
            "flux_humus_to_atmosphere",
            "c_in_atmosphere",
            "flux_atm_to_biomass",
            "c_in_biomass",
        ],
        shares: &[
            ("deterministic", 0.6148, 0.0696),
            ("high_2xco2_sensitivity", 0.5579, 0.0686),
            ("low_2xco2_sensitivity", 0.4969, 0.0711),
        ],
    },
];

/// How far a share may sit from its row: a hundredth of it.
///
/// Loose on purpose. The test is for a change that breaks loop analysis -- a
/// link score zeroed, a loop lost, a read of the wrong element -- and each
/// moves a share by far more. A tight pin would also be one platform's: the
/// scores run through `^` and `EXP`, and std documents the precision of
/// `f64::powf` and its kin as varying by platform.
const SHARE_TOLERANCE: f64 = 0.01;

/// Whether two node sequences are one cycle, wherever each starts.
fn same_cycle(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && (0..a.len().max(1)).any(|k| a.iter().cycle().skip(k).take(a.len()).eq(b.iter()))
}

/// The largest share of its partition a reported loop reaches, and its mean
/// share over the run's saved steps.
fn peak_and_mean_share(summary: &LoopSummary) -> (f64, f64) {
    let shares = || summary.importance.iter().map(|share| share.abs());
    let peak = shares().fold(0.0, f64::max);
    let mean = shares().sum::<f64>() / summary.importance.len() as f64;
    (peak, mean)
}

/// The loops that lead C-LEARN's run are the ones the table lists, in its
/// order, with the polarities and the shares it gives.
///
/// This is what holds loop analysis to a model of C-LEARN's size, through the
/// entry point a host calls (`analysis::analyze_model`). A leading loop's share
/// is its score against the scores of the 47 loops of its partition, so a row
/// moves when a link score on any of them is lost or read from the wrong
/// place. What it does not see is a change to scores on no loop of these
/// three partitions; `db::ltm_value_gate_tests` holds the ways a score is
/// zeroed on fixtures small enough for the default suite.
///
/// A failure lists every row that moved. To re-derive the table, run with
/// `--nocapture` and read the shares the test prints:
///     scripts/gates.sh --nocapture clearn_leading_loops_keep_their_shares
#[test]
#[ignore = "C-LEARN compiled, run and analyzed under LTM; run under the gates profile"]
fn clearn_leading_loops_keep_their_shares() {
    let project = open_clearn();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let analysis = analyze_model(&project, &mut db, sync.project, "main", None)
        .expect("C-LEARN has a main model");
    assert_eq!(analysis.analysis_error, None);
    let loops = &analysis.loop_dominance;

    let mut wrong = Vec::new();
    for (rank, row) in CLEARN_LEADING_LOOPS.iter().enumerate() {
        for &(element, want_peak, want_mean) in row.shares {
            let at = format!("{} at {element}", row.name);
            let nodes: Vec<String> = row.path.iter().map(|v| format!("{v}[{element}]")).collect();
            let Some(found) = loops.iter().find(|l| same_cycle(&l.variables, &nodes)) else {
                wrong.push(format!("{at}: not among the loops reported"));
                continue;
            };
            let (peak, mean) = peak_and_mean_share(found);
            println!(
                "{at}: {} with peak share {peak:.4}, mean share {mean:.4}",
                found.polarity
            );
            // The loops of its partition with a larger mean share are the
            // rows above its own.
            let stronger = loops
                .iter()
                .filter(|l| l.partition == found.partition && peak_and_mean_share(l).1 > mean)
                .count();
            if stronger != rank {
                wrong.push(format!(
                    "{at}: ranked {} in its partition, not {}",
                    stronger + 1,
                    rank + 1
                ));
            }
            if found.polarity != row.polarity {
                wrong.push(format!("{at}: {}, not {}", found.polarity, row.polarity));
            }
            for (what, got, want) in [("peak", peak, want_peak), ("mean", mean, want_mean)] {
                if (got - want).abs() > SHARE_TOLERANCE * want {
                    wrong.push(format!("{at}: {what} share {got:.4}, not {want}"));
                }
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "C-LEARN's leading loops are not the table's:\n  {}",
        wrong.join("\n  ")
    );
}
