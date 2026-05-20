// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! End-to-end discovery-mode LTM tests for large real-world models
//! (World3, C-LEARN), plus a fast tractable-model companion that proves
//! the contract those tests assert is actually satisfiable.
//!
//! ## Background
//!
//! LTM has two loop-finding modes. The *exhaustive* mode (Johnson
//! elementary-circuit enumeration) is gated by `MAX_LTM_SCC_NODES = 50`:
//! a model whose variable-level causal-graph SCC exceeds that threshold
//! auto-flips to *discovery* mode -- the strongest-path heuristic from
//! Eberlein & Schoenberg, "Finding the Loops That Matter" (2020),
//! Appendix I, implemented in `ltm_finding::discover_loops_with_graph`.
//! World3's variable-level SCC is 166, so it auto-flips to discovery.
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
//! site behind `analysis::analyze_model`). It deliberately does NOT use
//! the `ltm_finding::discover_loops` convenience wrapper: that wrapper
//! runs on the *variable-level* graph with empty `ltm_vars` / `dims`,
//! which is a strict minor of the production graph -- a discovery
//! algorithm made tractable on the variable graph could still be
//! intractable on the element graph, and a variable-level test would not
//! catch that.
//!
//! ## The 2-step startup guard
//!
//! LTM link scores are `PREVIOUS()`-based, so the first two saved
//! timesteps (indices 0 and 1) are startup-degenerate. Step 0's link
//! scores can be NaN -- `discover_loops_with_graph` skips step 0 in the
//! discovery DFS for exactly that reason -- and steps 0-1 carry no
//! positive loop contribution, so `rank_and_filter` drops every loop
//! whose only timesteps are those two. Step index 2 is the first
//! genuinely discoverable timestep. `FIRST_DISCOVERABLE_STEP` and
//! `TRUNCATED_STEP_COUNT` encode this; the discovery tests truncate
//! results to `TRUNCATED_STEP_COUNT` (3) so the window holds exactly one
//! discoverable timestep, and `assert_discovery_contract` only inspects
//! score values at step indices `>= FIRST_DISCOVERABLE_STEP`. (The same
//! "step 2 is the first real step" fact is relied on by the existing
//! discovery test in `simulate_ltm.rs`, which iterates `for step in
//! 2..`.)
//!
//! ## Finding 3: discovery does not scale to World3
//!
//! As of `dbc0844e` (Finding 1's `1/dt` link-score fix landed), running
//! discovery on World3 does **not** terminate within a practical budget.
//! Measured on a release build of `dbc0844e`:
//!
//! - Full discovery, all 401 saved timesteps, variable-level convenience
//!   path (`discover_loops`): ran 390+ seconds and allocated 22+ GB of
//!   RAM before a hard kill, having produced no result.
//! - A single discoverable timestep, **element-level production path**
//!   (`discover_loops_with_graph`, results truncated to
//!   `TRUNCATED_STEP_COUNT` -- what `world3_discovery_single_timestep`
//!   exercises): did not finish within the test's 20 s time budget,
//!   reaching ~4 GB RSS on the measuring machine. A faster machine
//!   allocates more within 20 s and instead trips the RSS ceiling --
//!   either way the repro fails cleanly and bounded.
//!
//! The phases leading up to discovery are all fast (parse ~4 ms,
//! discovery-mode compile ~140 ms, simulate 401 steps ~180 ms); the cost
//! is entirely inside discovery. The Finding 1 fix did **not** make
//! World3 discovery tractable.
//!
//! The single-timestep result is the decisive root-cause signal: the
//! blow-up is in `SearchGraph::check_outbound_uses` -- the strongest-path
//! DFS for *one* timestep -- not merely in the 401x per-timestep
//! multiplier in `discover_loops_with_graph`. The `best_score` pruning in
//! that DFS only bounds work when path-score products shrink along
//! paths; on World3's dense 166-node SCC, with link scores straddling
//! 1.0, the pruning degrades and the DFS re-explores subtrees
//! super-polynomially, accumulating loops (and their `Ident`/`String`
//! dedup keys) without bound until the final `MAX_LOOPS` truncation that
//! never gets reached. For comparison, Eberlein & Schoenberg report
//! 10-20 s for Urban Dynamics, a model *larger* than World3 -- so the
//! gap is algorithmic, not fundamental.
//!
//! ## Test layout
//!
//! - `world3_discovery_single_timestep` (`#[ignore]`d): the bounded,
//!   runnable repro. Drives the production path over a single
//!   discoverable timestep and asserts the contract discovery *should*
//!   satisfy (`assert_discovery_contract`). It currently FAILS at the
//!   time/RSS bound; it will go GREEN when Finding 3 (GH #540) is fixed.
//! - `discovery_contract_holds_on_tractable_arrayed_model` (NOT
//!   `#[ignore]`d, fast): runs the *same* element-level recipe and the
//!   *same* `assert_discovery_contract` on a small tractable arrayed
//!   model. It PASSES today -- proving the contract the World3 test
//!   asserts is satisfiable, that the World3 test would go green on a
//!   #540 fix rather than fail on bad assertions, and serving as a live
//!   guard: if the startup-guard width changes it fails loudly and fast.
//! - `clearn_ltm_discovery_compiles` (`#[ignore]`d): asserts C-LEARN
//!   compiles via the incremental path with LTM discovery enabled (a
//!   clean `Ok`, no panic), re-verifying GH #363. It asserts only the
//!   compile result, not discovery tractability.
//!
//! Tracked as GH #540 (linked from epic #488).

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use simlin_engine::common::{Canonical, Ident};
use simlin_engine::datamodel;
use simlin_engine::db::{
    LtmSyntheticVar, SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
    model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
    set_project_ltm_discovery_mode, set_project_ltm_enabled, sync_from_datamodel_incremental,
};
use simlin_engine::ltm::CausalGraph;
use simlin_engine::test_common::TestProject;
use simlin_engine::{Results, Vm, ltm_finding, open_vensim};

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

/// Index of the first genuinely discoverable saved timestep.
///
/// LTM link scores are `PREVIOUS()`-based: steps 0 and 1 are
/// startup-degenerate (step 0's link scores can be NaN; steps 0-1 carry
/// no positive loop contribution). Step index 2 is the first timestep
/// whose link scores -- and therefore discovered loop scores -- are
/// meaningful. See the module-level "2-step startup guard" section.
const FIRST_DISCOVERABLE_STEP: usize = 2;

/// Number of saved timesteps to keep when truncating results for a
/// single-discoverable-timestep discovery run: the two startup-guard
/// steps (0, 1) plus the first discoverable timestep (`= 2`). A window
/// of exactly this size is the minimal honest repro -- it forces
/// `discover_loops_with_graph` through one real discoverable DFS pass
/// and is the smallest window in which a *correct* discovery still
/// returns a non-empty loop set.
const TRUNCATED_STEP_COUNT: usize = FIRST_DISCOVERABLE_STEP + 1;

/// How often `run_with_timeout`'s main thread wakes to re-check the
/// deadline and the process RSS.
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Owned, `'static` inputs for the element-level discovery production
/// path. `discover_loops_with_graph` borrows all of these by reference;
/// bundling them as owned values lets a worker-thread closure (see
/// `run_with_timeout`) take the whole set by move, decoupled from the
/// `SimlinDb` they were derived from.
struct DiscoveryInputs {
    results: Results,
    causal_graph: CausalGraph,
    stocks: Vec<Ident<Canonical>>,
    ltm_vars: Vec<LtmSyntheticVar>,
    dims: Vec<datamodel::Dimension>,
}

/// Compile (LTM discovery mode), simulate, and assemble the
/// element-level discovery inputs for an arbitrary datamodel project.
///
/// This mirrors the production discovery path in
/// `analysis.rs::run_ltm_pipeline` exactly: the element-level
/// `CausalGraph` (via `model_element_causal_edges` +
/// `causal_graph_from_element_edges`), the element-level `stocks` list,
/// the `LtmSyntheticVar` metadata, and the project dimensions -- the four
/// arguments `discover_loops_with_graph` receives in production. The
/// salsa-borrowed values are cloned into owned ones so the caller can
/// move them into a worker thread.
///
/// Generic over the project so the same recipe drives both the World3
/// fixture and the tractable companion fixture -- the companion test is
/// only a faithful proxy for the World3 test if it runs the *identical*
/// assembly path.
fn discovery_inputs(datamodel_project: &datamodel::Project) -> DiscoveryInputs {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel_project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("project should compile with LTM discovery enabled");

    let mut vm = Vm::new(compiled).expect("LTM VM construction should succeed");
    vm.run_to_end()
        .expect("LTM simulation should run to completion");
    let results = vm.into_results();

    // Assemble the element-level discovery inputs exactly as the
    // production path does -- see `analysis.rs::run_ltm_pipeline`. These
    // four salsa-tracked results are `returns(ref)` (they borrow `db`);
    // clone them into owned values so the caller can move the bundle into
    // a worker thread that outlives `db`.
    let source_model = sync.models["main"].source_model;
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    let causal_graph = causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> =
        element_edges.stocks.iter().map(|s| Ident::new(s)).collect();
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project)
        .vars
        .clone();
    let dims = project_datamodel_dims(&db, sync.project).clone();

    DiscoveryInputs {
        results,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
    }
}

/// Parse, compile, simulate World3 and assemble its element-level
/// discovery inputs. Thin wrapper over `discovery_inputs`.
///
/// Every phase here is fast (sub-second total); the intractable phase is
/// `discover_loops_with_graph` itself, which the caller drives.
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
/// `discover_loops_with_graph` iterates `for step in 1..step_count`,
/// rebuilding the per-timestep search graph and re-running the
/// strongest-path DFS from every stock at each step. A truncated copy
/// lets a test exercise the per-timestep DFS on real link scores while
/// bounding the work to one discoverable timestep -- the decisive lever
/// that isolates "one timestep's DFS is super-polynomial" from "the 401x
/// per-timestep multiplier dominates".
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

/// Outcome of running a worker closure under `run_with_timeout`.
enum WorkerOutcome<T> {
    /// The closure returned a value within both the time budget and the
    /// RSS ceiling.
    Completed(T),
    /// The closure did not return within the time budget. The worker
    /// thread is leaked -- still running -- until the process exits.
    TimedOut,
    /// The process resident set exceeded the RSS ceiling while the
    /// closure was still running. The worker thread is leaked until the
    /// process exits.
    ExceededMemory { rss_bytes: u64 },
    /// The closure panicked. The worker thread has finished, so it is
    /// NOT leaked.
    Panicked,
}

/// Run `f` on a worker thread, bounding it by BOTH a wall-clock budget
/// and a process resident-set-size ceiling, and reporting *why* it
/// stopped.
///
/// Rust cannot kill a running thread, so a worker that trips the time
/// budget or the RSS ceiling is *leaked*: it keeps running -- and, for a
/// runaway `discover_loops_with_graph`, keeps allocating at 100+ MB/s on
/// World3 -- until the test process exits. The RSS ceiling is what makes
/// the leak safe regardless of machine speed: a pure wall-clock bound is
/// not a memory bound, because a faster machine simply allocates more
/// within the same budget. With the ceiling, the main thread gives up
/// (and the test process exits, reclaiming the worker) before the box is
/// at risk of an OOM kill. A leaked worker is acceptable here only
/// because the test that uses this is `#[ignore]`d and run by hand.
///
/// This extends the timeout pattern in `tests/wrld3_ltm_panic.rs` with
/// the RSS bound and with panic/timeout disambiguation: a panic inside
/// `f` drops the sender, which a naive `recv_timeout(...).ok()` would
/// misreport as a timeout.
fn run_with_timeout<T: Send + 'static>(
    budget: Duration,
    rss_ceiling_bytes: u64,
    f: impl FnOnce() -> T + Send + 'static,
) -> WorkerOutcome<T> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        // The receiver may already be gone (the main thread gave up on a
        // timeout or RSS trip); ignore the send error in that case.
        let _ = tx.send(f());
    });

    let deadline = Instant::now() + budget;
    loop {
        match rx.recv_timeout(POLL_INTERVAL) {
            Ok(value) => return WorkerOutcome::Completed(value),
            // The sender was dropped without a send: `f` unwound (panicked).
            Err(mpsc::RecvTimeoutError::Disconnected) => return WorkerOutcome::Panicked,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(rss) = current_rss_bytes()
                    && rss > rss_ceiling_bytes
                {
                    return WorkerOutcome::ExceededMemory { rss_bytes: rss };
                }
                if Instant::now() >= deadline {
                    return WorkerOutcome::TimedOut;
                }
            }
        }
    }
}

/// Current process resident set size in bytes, read from
/// `/proc/self/statm` (Linux). Returns `None` on any other platform or
/// on a read/parse failure; `run_with_timeout` then falls back to the
/// wall-clock budget alone.
fn current_rss_bytes() -> Option<u64> {
    let statm = std::fs::read_to_string("/proc/self/statm").ok()?;
    // `/proc/self/statm` fields are whitespace-separated; field index 1
    // is the resident set size in pages.
    let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    // Page size is 4096 on every target this repo builds for (x86-64 and
    // aarch64 Linux); hard-code it rather than pull in a `libc` dependency
    // just for a test helper. A wrong page size only scales the ceiling by
    // a constant -- it cannot defeat the bound.
    Some(resident_pages * 4096)
}

/// The structural contract every successful `discover_loops_with_graph`
/// run must satisfy on a results window truncated to
/// `TRUNCATED_STEP_COUNT`.
///
/// This is the *shared* assertion logic for both discovery tests: the
/// `#[ignore]`d World3 repro asserts it (and currently cannot reach it,
/// because discovery does not terminate -- Finding 3), and the fast
/// tractable companion test asserts the very same thing and PASSES,
/// proving the contract is satisfiable and that the World3 test would go
/// green on a #540 fix.
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
/// FIRST_DISCOVERABLE_STEP`: steps 0-1 are startup-degenerate and step 0
/// in particular is allowed to be NaN by the LTM algorithm (see the
/// module-level "2-step startup guard" section), so asserting finiteness
/// there would be asserting on behavior the algorithm treats as
/// undefined.
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

/// The contract `discover_loops_with_graph` should satisfy on World3,
/// exercised over a single discoverable timestep of the **element-level
/// production path** (a bounded variant of the full end-to-end run --
/// see the module docs).
///
/// FIXME(Finding 3, GH #540): this test currently FAILS.
/// `discover_loops_with_graph` does not terminate even over a window
/// holding a *single* discoverable timestep of World3: it exhausts the
/// time budget below (reaching ~4 GB RSS on the measuring machine) or,
/// on a faster box, trips the RSS ceiling first. The full 401-timestep
/// run is far worse. It is `#[ignore]`d so it does not stall
/// `cargo test`; it is kept as the runnable repro and the executable
/// statement of the contract, and will go GREEN once the discovery
/// algorithm is made tractable -- see
/// `discovery_contract_holds_on_tractable_arrayed_model`, which proves
/// the post-fix assertions (`assert_discovery_contract`) are satisfiable
/// today on a tractable model via the identical recipe.
///
/// Why a single discoverable timestep rather than the full run: the
/// per-timestep DFS (`SearchGraph::check_outbound_uses`) is itself the
/// blow-up, so one discoverable timestep is a sufficient -- and far
/// safer -- repro. The full run would leak a worker thread allocating
/// tens of GB before the process tears down; one truncated window, plus
/// the RSS ceiling in `run_with_timeout`, bounds that.
///
/// Why the element-level path: `analysis::analyze_model` (the production
/// caller) runs `discover_loops_with_graph` on the element-level graph
/// with populated `ltm_vars` / `dims`. The `ltm_finding::discover_loops`
/// convenience wrapper runs on the variable-level graph with empty
/// metadata -- a strict minor of the production graph -- so a
/// variable-level test could go green on a fix that still leaves the
/// production element-level path intractable.
///
/// Run explicitly with:
///     cargo test --release -p simlin-engine \
///         --test ltm_discovery_large_models -- --ignored --nocapture \
///         world3_discovery_single_timestep
#[test]
#[ignore]
fn world3_discovery_single_timestep() {
    let inputs = world3_discovery_inputs();
    let total_steps = inputs.results.step_count;
    assert!(
        total_steps >= TRUNCATED_STEP_COUNT,
        "World3 must simulate at least {TRUNCATED_STEP_COUNT} saved timesteps, got {total_steps}"
    );

    // Truncate to steps 0, 1, 2: the two startup-guard steps plus the
    // first genuinely discoverable timestep (step 2). This forces
    // `discover_loops_with_graph` through exactly one real discoverable
    // DFS pass -- a sufficient repro, since the per-timestep DFS is
    // itself the blow-up (see module docs) -- while keeping the leaked
    // worker bounded.
    let truncated = truncate_results(&inputs.results, TRUNCATED_STEP_COUNT);
    let DiscoveryInputs {
        results: _,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
    } = inputs;

    // Time budget: hugely generous for a *fixed* implementation (a single
    // timestep of a tractable discovery completes in well under a second
    // -- see the companion test), yet modest enough to bound the leaked
    // worker. RSS ceiling: a machine-independent backstop so a fast box
    // that allocates quickly still fails cleanly rather than OOM-killing
    // the process. Whichever trips first wins. See `run_with_timeout`.
    let budget = Duration::from_secs(20);
    let rss_ceiling_bytes: u64 = 6 * 1024 * 1024 * 1024; // 6 GiB

    let t = Instant::now();
    let outcome = run_with_timeout(budget, rss_ceiling_bytes, move || {
        ltm_finding::discover_loops_with_graph(&truncated, &causal_graph, &stocks, &ltm_vars, &dims)
    });
    let elapsed = t.elapsed();

    let found = match outcome {
        WorkerOutcome::Completed(Ok(found)) => found,
        WorkerOutcome::Completed(Err(e)) => {
            panic!("discover_loops_with_graph returned an Err on World3: {e:?}")
        }
        WorkerOutcome::TimedOut => panic!(
            "FIXME(Finding 3, GH #540): discover_loops_with_graph did not finish on a \
             single discoverable timestep of World3 (model has {total_steps} saved \
             timesteps) within {budget:?}. This is the known discovery-intractability \
             finding."
        ),
        WorkerOutcome::ExceededMemory { rss_bytes } => panic!(
            "FIXME(Finding 3, GH #540): discover_loops_with_graph exceeded the \
             {rss_ceiling_bytes}-byte RSS ceiling (process at {rss_bytes} bytes) on a \
             single discoverable timestep of World3 after {elapsed:?}. This is the \
             known discovery-intractability finding."
        ),
        WorkerOutcome::Panicked => panic!(
            "discover_loops_with_graph PANICKED on a single discoverable timestep of \
             World3 -- this is unexpected (the known Finding 3 symptom is \
             non-termination, not a panic). Investigate before treating it as the \
             intractability finding."
        ),
    };

    eprintln!(
        "World3 single-timestep element-level discovery: {} loops in {:?}",
        found.len(),
        elapsed
    );

    // The same structural contract the tractable companion test proves is
    // satisfiable today. When #540 is fixed, this assertion is what makes
    // the test go GREEN.
    assert_discovery_contract(&found);
}

/// Proves `assert_discovery_contract` -- the contract
/// `world3_discovery_single_timestep` asserts -- is satisfiable *today*,
/// via the exact same element-level `discover_loops_with_graph` recipe,
/// on a small tractable model.
///
/// Without this, the World3 test would be "RED today (intractable) and
/// unverified for GREEN-when-fixed": GH #540 makes it impossible to
/// observe a successful World3 discovery, so the post-fix assertions
/// could be subtly wrong (e.g. the truncation window too short to hold a
/// discoverable timestep) and nobody would find out until #540 is fixed.
/// This companion runs the identical recipe -- `discovery_inputs` ->
/// `truncate_results(.., TRUNCATED_STEP_COUNT)` ->
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
/// fails loudly and fast -- instead of the `#[ignore]`d World3 test
/// silently rotting.
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
    } = inputs;

    let found = ltm_finding::discover_loops_with_graph(
        &truncated,
        &causal_graph,
        &stocks,
        &ltm_vars,
        &dims,
    )
    .expect("discovery on the tractable arrayed fixture should not error");

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
/// with **LTM discovery enabled** (`set_project_ltm_enabled(true)` +
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
/// C-LEARN's 1.4 MB MDL plus a discovery-mode compile runs several
/// seconds in release and proportionally longer in the debug build CI
/// uses, so on-demand execution is appropriate. Run explicitly with:
///     cargo test --release -p simlin-engine \
///         --test ltm_discovery_large_models -- --ignored --nocapture \
///         clearn_ltm_discovery_compiles
#[test]
#[ignore]
fn clearn_ltm_discovery_compiles() {
    let mdl = match std::fs::read_to_string(CLEARN_MDL) {
        Ok(contents) => contents,
        Err(e) => panic!("failed to read {CLEARN_MDL}: {e}"),
    };

    let datamodel_project =
        open_vensim(&mdl).expect("open_vensim should parse C-LEARN v77 for Vensim.mdl");

    // Compile via the incremental path with LTM discovery enabled. NOT
    // wrapped in `catch_unwind`: per AC7.5 a panic here is a hard,
    // root-caused failure (the GH #363 symptom re-verified), never caught.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);

    compile_project_incremental(&db, sync.project, "main").unwrap_or_else(|e| {
        panic!("C-LEARN should compile with LTM discovery enabled, got Err: {e:?}")
    });
}
