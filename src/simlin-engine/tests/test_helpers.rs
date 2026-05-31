// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Shared test helpers for integration tests.
//!
//! Extracted from `simulate.rs` so that multiple integration test files
//! (simulate.rs, simulate_systems.rs, etc.) can share the comparison logic.
//!
//! pattern: Mixed (unavoidable)
//! Reason: `ensure_results*` is a pure comparator (Functional Core), while
//! `ensure_wasm_matches` is an Imperative Shell (it drives the salsa compile
//! pipeline and executes the emitted wasm under the DLR-FT interpreter). They
//! live together because this is the single shared test-helper module the
//! implementation plan centralizes comparison logic in, and the wasm shell's
//! only job is to feed the pure comparator. The slab -> `Results` conversion is
//! extracted as a pure function (`wasm_results_from_slab`) to keep the I/O
//! boundary explicit.

use checked::Store;
use float_cmp::approx_eq;
use simlin_engine::common::{Canonical, Ident};
use simlin_engine::datamodel;
use simlin_engine::db::LtmSyntheticVar;
use simlin_engine::ltm::CausalGraph;
use simlin_engine::wasmgen::{WasmGenError, WasmLayout, compile_simulation};
use simlin_engine::{Results, SimSpecs, Vm};
use wasm::validate;

/// Tolerance for `$⁚ltm⁚*` series-parity assertions between the wasm backend
/// and the bytecode VM. The synthetic LTM equations are emitted by the same
/// salsa pipeline and lowered by the same per-opcode emitter into both
/// backends, so the columns should agree to floating-point round-off (the
/// remaining difference is only the integration loop's accumulation order,
/// which is identical here). This is far tighter than the 0.05 rel-loop-score
/// tolerance used in `simulate_ltm.rs` (which compares against an external
/// oracle whose intermediate algebra differs). A model that needs looser
/// should document why inline rather than weakening the constant.
///
/// **Tolerance shape**: relative-or-absolute — `max(1.0, |vm|.max(|wasm|)) * tol`.
/// The absolute floor (1.0) handles values near zero where a pure relative
/// epsilon collapses; the relative branch handles large magnitudes where the
/// absolute floor would be too strict.
///
/// **Loop scores stay at the same tight value**: a loop score is the product
/// of its signed link scores. For corpus models like `arms_race_3party` the
/// chain length is a small constant; the underlying `$⁚ltm⁚link_score⁚*`
/// series already agree within this tolerance per `assert_ltm_slabs_match`,
/// so the chained product stays at the same relative scale. Both backends
/// feed the same salsa-compiled opcode sequence into `discover_loops_with_graph`,
/// so per-operation rounding is bit-identical and there is no compounding
/// divergence.
///
/// **Heavy-twin note** (`#[ignore]`d C-LEARN and World3): the wasm backend
/// open-codes the transcendentals `exp`/`ln`/`sin`/`cos`/`tan`/`atan`/
/// `asin`/`acos`/`log10`/`pow` in `src/simlin-engine/src/wasmgen/math.rs`,
/// which can differ from Rust libm by a few ULP. For a link-score equation
/// that routes through a transcendental, the propagation through a chain of
/// `k` links stays well within the `1e-6` relative floor; the heavy twins
/// are where this would surface in practice and they are `#[ignore]`d, so the
/// risk is contained.
#[allow(dead_code)]
pub const LTM_SERIES_TOLERANCE: f64 = 1e-6;

/// Columns that are vendor-specific or otherwise not important for
/// simulation correctness.
const IGNORABLE_COLS: &[&str] = &["saveper", "initial_time", "final_time", "time_step"];

/// Check if a variable name is a Vensim-specific internal delay/smooth variable.
/// These have formats like "#d8>DELAY3#[A1]" or "#d8>DELAY3>RT2#[A1]".
fn is_vensim_internal_module_var(name: &str) -> bool {
    name.starts_with('#') && name.contains('>')
}

/// Check if a variable name is an implicit module variable created by
/// builtins_visitor for SMOOTH/DELAY/TREND/etc. These names start with
/// "$\u{205A}" (dollar sign + two dot punctuation) and are internal
/// implementation details whose initial values may legitimately differ
/// across compilation paths due to evaluation order differences.
fn is_implicit_module_var(name: &str) -> bool {
    name.starts_with("$\u{205A}")
}

/// Whether `ident` names an element of one of the excluded base variables.
/// A base name `"y"` excludes the scalar `y` and every arrayed element
/// `y[a1]`, `y[a2]`, ... (the `.dat`/results key form), so a single
/// not-yet-supported variable can be carved out of a comparison without
/// weakening the gate for every other variable.
fn is_excluded_var(ident: &str, excluded: &[&str]) -> bool {
    excluded.iter().any(|&base| {
        ident == base
            || ident
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('['))
    })
}

/// Compare expected results against simulation output.
///
/// Iterates expected variable keys only, so extra variables in `results`
/// (modules, internal flows, etc.) don't cause failures. Uses absolute
/// epsilon of 2e-3 for non-Vensim data, with relative comparison for
/// Vensim-sourced data.
#[allow(dead_code)]
pub fn ensure_results(expected: &Results, results: &Results) {
    ensure_results_excluding(expected, results, &[]);
}

/// Like [`ensure_results`], but skips every expected variable whose base
/// name is in `excluded` (exact name or `name[...]` element). Used to keep
/// a genuine-Vensim regression gate hard for every variable *except* a
/// single, separately-tracked unsupported one -- the comparison stays an
/// unconditional equality check for all other variables.
pub fn ensure_results_excluding(expected: &Results, results: &Results, excluded: &[&str]) {
    assert_eq!(expected.step_count, results.step_count);
    assert_eq!(expected.iter().len(), results.iter().len());

    let expected_results = expected;

    let mut step = 0;
    for (expected_row, results_row) in expected.iter().zip(results.iter()) {
        for ident in expected.offsets.keys() {
            if is_excluded_var(ident.as_str(), excluded) {
                continue;
            }
            let expected = expected_row[expected.offsets[ident]];
            if !results.offsets.contains_key(ident)
                && (IGNORABLE_COLS.contains(&ident.as_str())
                    || is_vensim_internal_module_var(ident.as_str()))
            {
                continue;
            }
            // Skip implicit module variables (from SMOOTH/DELAY/TREND
            // expansion). These internal variables may legitimately have
            // different initial values across compilation paths due to
            // evaluation order differences.
            if is_implicit_module_var(ident.as_str()) {
                continue;
            }
            if !results.offsets.contains_key(ident) {
                panic!("output missing variable '{ident}'");
            }
            let off = results.offsets[ident];
            let actual = results_row[off];

            let around_zero = approx_eq!(f64, expected, 0.0, epsilon = 3e-6)
                && approx_eq!(f64, actual, 0.0, epsilon = 1e-6);

            if !around_zero {
                let (exp_cmp, act_cmp, epsilon) = if results.is_vensim || expected_results.is_vensim
                {
                    // Vensim outputs ~6 significant figures. Use relative comparison
                    // to handle large magnitudes (where small relative errors become
                    // large absolute errors). For small values, maintain the original
                    // absolute tolerance of 2e-3 so we don't become too strict.
                    let max_val = expected.abs().max(actual.abs()).max(1e-10);
                    let relative_eps = max_val * 5e-6;
                    (expected, actual, relative_eps.max(2e-3))
                } else {
                    (expected, actual, 2e-3)
                };

                if !approx_eq!(f64, exp_cmp, act_cmp, epsilon = epsilon) {
                    eprintln!("step {step}: {ident}: {expected} (expected) != {actual} (actual)");
                    panic!("not equal");
                }
            }
        }

        step += 1;
    }

    assert_eq!(expected.step_count, step);

    // UNKNOWN is a sentinel value we use -- it should never show up
    // unless we've wrongly sized our data slices
    assert!(
        !results
            .offsets
            .contains_key(&Ident::<Canonical>::from_str_unchecked("UNKNOWN"))
    );
}

// The wasm-parity helpers below are consumed only by the `simulate` corpus
// binary; the other test binaries that share this module (`simulate_systems`,
// `systems_roundtrip`, `metasd_macros`) include the file but do not run wasm
// parity, so each item is `#[allow(dead_code)]` to stay clean under
// `cargo clippy --all-targets -- -D warnings` (the same shared-helper idiom as
// `SimTier` in `metasd_macros.rs`).

/// Outcome of running a model through the wasm backend via
/// [`ensure_wasm_matches`].
///
/// `Ran` means the model was within the wasm backend's supported feature set,
/// executed under the interpreter, and CLEARED the parity comparator (the
/// helper panics internally on any divergence -- a supported-but-wrong model is
/// a hard failure, never a `Ran`). `Skipped` means `compile_simulation`
/// returned [`WasmGenError::Unsupported`] (an out-of-scope construct); the
/// message is carried so the caller decides whether that is a failure.
///
/// Phase 8 closed the corpus gate: for a model the VM SIMULATED in the default
/// suite, a `Skipped` outcome is now a HARD FAILURE -- the corpus callers
/// (`wasm_parity_hook`, the parity-floor gates, the systems harness) panic on
/// it (wasm-backend AC3.2: every core-simulation model runs through both
/// backends). The variant survives only so the `ensure_wasm_matches_skips_*`
/// unit test can still observe a *genuinely* out-of-scope construct returning a
/// clean `Unsupported` (AC1.4) -- never a panic or a silently wrong result --
/// rather than reaching the hook.
#[allow(dead_code)]
#[derive(Debug)]
pub enum WasmRunOutcome {
    Ran,
    Skipped(String),
}

/// Build a `Results` from a wasm backend's step-major results slab.
///
/// The slab is `layout.n_chunks * layout.n_slots` f64 laid out row-major by
/// saved step (the same step-major order the bytecode VM's `Results` uses), so
/// `step_size = n_slots` and `step_count = n_chunks` make `Results::iter` yield
/// one chunk per saved step. Each canonical variable name in `layout` maps back
/// to its slot offset within a chunk. `is_vensim = false`: a wasm-emitted run is
/// a Simlin computation, so it takes the absolute-tolerance branch of the
/// comparator (never the Vensim relative-tolerance branch).
///
/// Pure: no I/O, no global state -- it only reshapes already-read data, so it is
/// the Functional Core boundary of [`ensure_wasm_matches`].
#[allow(dead_code)]
fn wasm_results_from_slab(layout: &WasmLayout, slab: Vec<f64>, specs: SimSpecs) -> Results {
    let offsets = layout
        .var_offsets
        .iter()
        // The names came from `CompiledSimulation::offsets`, whose keys are
        // already `Ident<Canonical>`, so they round-trip without re-canonicalizing.
        .map(|(name, off)| (Ident::<Canonical>::from_str_unchecked(name), *off))
        .collect();

    Results {
        offsets,
        data: slab.into_boxed_slice(),
        step_size: layout.n_slots,
        step_count: layout.n_chunks,
        specs,
        is_vensim: false,
    }
}

/// Compile `model_name` of `datamodel` to wasm, run it under the DLR-FT
/// interpreter, and reshape the results slab into a [`Results`] — or return the
/// `Unsupported` message if the model is outside the wasm backend's feature set.
///
/// Builds the `CompiledSimulation` exactly as the corpus VM path does
/// (simulate.rs `compile_vm`), so the wasm blob is the twin of the VM's run. An
/// incremental-compile error (a VM-side issue gated elsewhere) and an
/// `Unsupported` codegen result both return `Err(msg)`; the caller decides
/// whether that is a skip or a hard failure.
///
/// Imperative Shell: drives the salsa compile pipeline and the wasm interpreter
/// (side effects), delegating the reshape to the pure [`wasm_results_from_slab`].
/// Shared by [`ensure_wasm_matches`] (the corpus `.dat`/CSV comparator) and the
/// C-LEARN wasm twin (which compares against `Ref.vdf` instead).
#[allow(dead_code)]
pub fn wasm_results_for(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
) -> Result<Results, String> {
    use simlin_engine::db::{
        SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    let sim = compile_project_incremental(&db, sync.project, model_name)
        .map_err(|e| format!("incremental compile failed: {e:?}"))?;

    let artifact = match compile_simulation(&sim) {
        Ok(artifact) => artifact,
        Err(WasmGenError::Unsupported(msg)) => return Err(msg),
    };

    let slab = run_wasm_results(&artifact.wasm, &artifact.layout);
    let specs = SimSpecs::from(&datamodel.sim_specs);
    Ok(wasm_results_from_slab(&artifact.layout, slab, specs))
}

/// LTM-enabled VM oracle: compile `model_name` of `datamodel` with
/// `ltm_enabled = true` on its freshly-synced salsa `SourceProject`, run it
/// to completion in the bytecode VM, and return the resulting [`Results`].
///
/// Mirrors `simulate_ltm.rs::compile_ltm_incremental_with_partitions` but
/// stops at `Vm::into_results` (no `loop_partitions` book-keeping -- the
/// caller compares the `$⁚ltm⁚*` slot series directly, not the post-sim
/// relative loop scores). The `db` is owned by this function, so the flag
/// flip never leaks (same rationale as `wasmgen::compile_datamodel_to_artifact`).
///
/// Imperative Shell: drives the salsa compile pipeline and the VM.
#[allow(dead_code)]
pub fn vm_results_for_ltm(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
) -> Results {
    use simlin_engine::db::{
        SimlinDb, compile_project_incremental, set_project_ltm_enabled,
        sync_from_datamodel_incremental,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, model_name)
        .expect("LTM-enabled incremental compile should succeed for the LTM corpus");
    let mut vm = Vm::new(compiled).expect("Vm::new should succeed on a salsa-compiled model");
    vm.run_to_end()
        .expect("Vm::run_to_end should succeed on the LTM corpus");
    vm.into_results()
}

/// LTM-enabled wasm peer of [`vm_results_for_ltm`]: compile `model_name` of
/// `datamodel` with `ltm_enabled = true`, lower to wasm, run under the DLR-FT
/// interpreter, and reshape the slab into a [`Results`]. Returns
/// `Err(message)` on wasm-codegen `Unsupported` or an incremental-compile
/// failure, so the caller (the ratcheting floor gate) can classify a model
/// as "did not lower" vs. "lowered but wrong" -- the latter would have
/// produced an `Ok` and then panicked in [`assert_ltm_slabs_match`].
///
/// Mirrors the body of [`wasm_results_for`] with `set_project_ltm_enabled`
/// inserted before `compile_project_incremental`; the reshape goes through
/// the private `wasm_results_from_slab` (reachable from this sibling `pub fn`
/// in the same module).
///
/// Imperative Shell: drives the salsa compile pipeline and the wasm
/// interpreter, delegating the reshape to the pure [`wasm_results_from_slab`].
#[allow(dead_code)]
pub fn wasm_results_for_ltm(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
) -> Result<Results, String> {
    use simlin_engine::db::{
        SimlinDb, compile_project_incremental, set_project_ltm_enabled,
        sync_from_datamodel_incremental,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let sim = compile_project_incremental(&db, sync.project, model_name)
        .map_err(|e| format!("incremental compile failed: {e:?}"))?;

    let artifact = match compile_simulation(&sim) {
        Ok(artifact) => artifact,
        Err(WasmGenError::Unsupported(msg)) => return Err(msg),
    };

    let slab = run_wasm_results(&artifact.wasm, &artifact.layout);
    let specs = SimSpecs::from(&datamodel.sim_specs);
    Ok(wasm_results_from_slab(&artifact.layout, slab, specs))
}

/// Whole-slab equality assertion for two [`Results`] built from the *same*
/// `CompiledSimulation` (one via the VM, one via the wasm backend). Asserts
/// shape (`step_size`, `step_count`) first, then compares the entire data
/// slab element-wise within [`LTM_SERIES_TOLERANCE`] using a relative-or-
/// absolute tolerance matching the style of [`ensure_results`].
///
/// **Why a whole-slab compare and not a per-`$⁚ltm⁚*`-column scan.** Both
/// `Results` share their `var_offsets` (it is a verbatim copy of
/// `CompiledSimulation.offsets`), so slot `i` denotes the identical
/// variable+element on both sides. A full-slab compare therefore covers
/// every `$⁚ltm⁚link_score⁚*` and `$⁚ltm⁚loop_score⁚*` column *including
/// each element of an arrayed/cross-element LTM variable* (whose elements
/// occupy contiguous slots), with no per-variable slot-span bookkeeping --
/// and it incidentally verifies the rest of the model agrees too, which
/// keeps the gate honest.
///
/// This is the single comparator the arrayed LTM phase (Phase 4) reuses to
/// carry wasm-ltm.AC2.4 without modification.
///
/// Pure: no I/O.
#[allow(dead_code)]
pub fn assert_ltm_slabs_match(vm: &Results, wasm: &Results) {
    assert_eq!(
        vm.step_size, wasm.step_size,
        "LTM slab step_size mismatch: vm={} wasm={}",
        vm.step_size, wasm.step_size
    );
    assert_eq!(
        vm.step_count, wasm.step_count,
        "LTM slab step_count mismatch: vm={} wasm={}",
        vm.step_count, wasm.step_count
    );
    let n = vm.step_count * vm.step_size;
    assert!(
        vm.data.len() >= n && wasm.data.len() >= n,
        "LTM slab data too short: vm.len={} wasm.len={} expected>={n}",
        vm.data.len(),
        wasm.data.len()
    );
    for i in 0..n {
        let v = vm.data[i];
        let w = wasm.data[i];
        // Relative-or-absolute tolerance: the absolute floor catches values
        // near zero (where a relative epsilon collapses); the relative
        // branch catches large magnitudes (where the absolute floor would
        // be too strict). Both sides are *Simlin* runs, so we never need
        // the Vensim ~6-sig-fig allowance.
        if v.is_nan() && w.is_nan() {
            // Both produced NaN (e.g. an out-of-range vector read on a
            // step where the LTM source is :NA:): treat as equal, just
            // like `ensure_results_excluding`'s around-zero branch treats
            // dual zeros as equal.
            continue;
        }
        let max_abs = v.abs().max(w.abs()).max(1.0);
        let epsilon = LTM_SERIES_TOLERANCE * max_abs;
        if !approx_eq!(f64, v, w, epsilon = epsilon) {
            let step = i / vm.step_size;
            let slot = i % vm.step_size;
            // Recover the canonical name of the diverging slot for a
            // useful failure message (linear scan over `offsets`; this
            // only runs on a panic path so the cost is irrelevant).
            let name = vm
                .offsets
                .iter()
                .find_map(|(k, &off)| if off == slot { Some(k.as_str()) } else { None })
                .unwrap_or("<unknown>");
            panic!(
                "LTM slab mismatch at step {step} slot {slot} ({name}): \
                 vm={v} wasm={w} (epsilon={epsilon})"
            );
        }
    }
}

/// Resumable-ABI peer of [`wasm_results_for`]: compile `model_name` of
/// `datamodel` to wasm, then drive the blob through the segmented
/// `run_initials`-then-per-target-`run_to` path (rather than the single-shot
/// `run`) and reshape the final slab into a [`Results`].
///
/// `targets` is the ordered list of `run_to(t)` boundaries; the final target must
/// be the simulation's `stop` so the slab is fully populated and the result is
/// directly comparable (via [`ensure_results`]) to the single-`run`
/// [`wasm_results_for`] series. The whole-model `#[ignore]`d twins use this to
/// prove a mid-run-split run on a real model lands on the byte-identical final
/// series as a single uninterrupted run.
///
/// Imperative Shell: drives the salsa compile pipeline and the wasm interpreter,
/// delegating the reshape to the pure [`wasm_results_from_slab`].
#[allow(dead_code)]
pub fn wasm_results_for_segmented(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
    targets: &[f64],
) -> Result<Results, String> {
    use simlin_engine::db::{
        SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    let sim = compile_project_incremental(&db, sync.project, model_name)
        .map_err(|e| format!("incremental compile failed: {e:?}"))?;

    let artifact = match compile_simulation(&sim) {
        Ok(artifact) => artifact,
        Err(WasmGenError::Unsupported(msg)) => return Err(msg),
    };

    let slab = run_wasm_results_segmented(&artifact.wasm, &artifact.layout, targets);
    let specs = SimSpecs::from(&datamodel.sim_specs);
    Ok(wasm_results_from_slab(&artifact.layout, slab, specs))
}

/// Compile `model_name` of `datamodel` to wasm, run it under the DLR-FT
/// interpreter, and assert its results clear the SAME `ensure_results_excluding`
/// comparator the VM clears against `expected`.
///
/// There is no separate, tighter wasm-vs-VM threshold (per the design's
/// validation bar): "wasm-vs-VM parity" is established because both backends
/// clear the identical comparator against the identical expected outputs. A
/// model outside the wasm backend's supported feature set returns
/// [`WasmRunOutcome::Skipped`] (never a failure); a supported model whose wasm
/// output diverges panics inside `ensure_results_excluding`.
///
/// Imperative Shell: it drives the salsa compile pipeline and the wasm
/// interpreter (side effects), delegating the reshape to the pure
/// [`wasm_results_from_slab`] and the comparison to the pure
/// [`ensure_results_excluding`].
#[allow(dead_code)]
pub fn ensure_wasm_matches(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
    expected: &Results,
    excluded: &[&str],
) -> WasmRunOutcome {
    let wasm_results = match wasm_results_for(datamodel, model_name) {
        Ok(results) => results,
        Err(msg) => return WasmRunOutcome::Skipped(msg),
    };

    // The same comparator the VM clears: panics loudly on any divergence, so a
    // supported-but-wrong wasm module fails here rather than reporting Ran.
    ensure_results_excluding(expected, &wasm_results, excluded);
    WasmRunOutcome::Ran
}

/// Instantiate `wasm` under the DLR-FT `checked::Store`, invoke the exported
/// `run`, and copy `n_chunks * n_slots` f64 out of the results region (located
/// via `layout.results_offset`). This is the wasm-execution side effect of
/// [`ensure_wasm_matches`]; the bytes it returns are consumed purely afterward.
#[allow(dead_code)]
fn run_wasm_results(wasm: &[u8], layout: &WasmLayout) -> Vec<f64> {
    let info = validate(wasm).expect("generated wasm module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate wasm module")
        .module_addr;
    let run = store
        .instance_export(inst, "run")
        .expect("run export must exist")
        .as_func()
        .expect("run export must be a function");
    store
        .invoke_simple_typed::<(), ()>(run, ())
        .expect("run wasm");
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export must exist")
        .as_mem()
        .expect("memory export must be a memory");

    let n = layout.n_chunks * layout.n_slots;
    let base = layout.results_offset;
    store.mem_access_mut_slice(mem, |bytes| {
        (0..n)
            .map(|i| {
                let a = base + i * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// Drive the blob's *resumable* run ABI: instantiate `wasm`, call `run_initials`
/// once, then `run_to(t)` for each `t` in `targets` (advancing the persistent
/// step cursor held in the blob's mutable globals), and copy the whole step-major
/// results slab out (`n_chunks * n_slots` f64 at `layout.results_offset`).
///
/// This is the resumable peer of [`run_wasm_results`] (which calls the
/// single-shot `run`). A segmented drive `&[t1, t2]` must produce a slab whose
/// rows up to `t2` equal a single `run_to(t2)` and the VM driven through the same
/// `run_to` segments -- the parity the wasm-side tests assert.
#[allow(dead_code)]
pub fn run_wasm_results_segmented(wasm: &[u8], layout: &WasmLayout, targets: &[f64]) -> Vec<f64> {
    let info = validate(wasm).expect("generated wasm module must validate");
    let mut store = Store::new(());
    let inst = store
        .module_instantiate(&info, Vec::new(), None)
        .expect("instantiate wasm module")
        .module_addr;
    let run_initials = store
        .instance_export(inst, "run_initials")
        .expect("run_initials export must exist")
        .as_func()
        .expect("run_initials export must be a function");
    store
        .invoke_simple_typed::<(), ()>(run_initials, ())
        .expect("run_initials wasm");
    for &t in targets {
        let run_to = store
            .instance_export(inst, "run_to")
            .expect("run_to export must exist")
            .as_func()
            .expect("run_to export must be a function");
        store
            .invoke_simple_typed::<(f64,), ()>(run_to, (t,))
            .expect("run_to wasm");
    }
    let mem = store
        .instance_export(inst, "memory")
        .expect("memory export must exist")
        .as_mem()
        .expect("memory export must be a memory");

    let n = layout.n_chunks * layout.n_slots;
    let base = layout.results_offset;
    store.mem_access_mut_slice(mem, |bytes| {
        (0..n)
            .map(|i| {
                let a = base + i * 8;
                f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
            })
            .collect()
    })
}

/// Discovery-mode peer of [`wasm_results_for_ltm`]: compile `model_name`
/// of `datamodel` with **both** `ltm_enabled = true` and
/// `ltm_discovery_mode = true`, lower to wasm, run under the DLR-FT
/// interpreter, and reshape the slab into a [`Results`]. Returns
/// `Err(message)` on wasm-codegen `Unsupported` or an incremental-compile
/// failure.
///
/// Discovery mode causes `db::ltm::model_ltm_variables` to emit a
/// `$⁚ltm⁚link_score⁚{from}→{to}` synthetic for **every** causal edge
/// (not just loop-participating ones), so the reconstructed `Results`
/// can drive `ltm_finding::discover_loops_with_graph` end-to-end. The
/// reconstructed `Results` carries `specs` populated from the
/// datamodel, which `discover_loops_with_graph` relies on for each
/// `FoundLoop.scores` time axis (`results.specs.start + save_step * step`).
///
/// Imperative Shell: drives the salsa compile pipeline and the wasm
/// interpreter, delegating the reshape to the pure [`wasm_results_from_slab`].
#[allow(dead_code)]
pub fn wasm_results_for_ltm_discovery(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
) -> Result<Results, String> {
    use simlin_engine::db::{
        SimlinDb, compile_project_incremental, set_project_ltm_discovery_mode,
        set_project_ltm_enabled, sync_from_datamodel_incremental,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let sim = compile_project_incremental(&db, sync.project, model_name)
        .map_err(|e| format!("incremental compile failed: {e:?}"))?;

    let artifact = match compile_simulation(&sim) {
        Ok(artifact) => artifact,
        Err(WasmGenError::Unsupported(msg)) => return Err(msg),
    };

    let slab = run_wasm_results(&artifact.wasm, &artifact.layout);
    let specs = SimSpecs::from(&datamodel.sim_specs);
    Ok(wasm_results_from_slab(&artifact.layout, slab, specs))
}

/// Owned, `'static` inputs for the element-level LTM discovery
/// production path. `ltm_finding::discover_loops_with_graph` borrows all
/// of these by reference; bundling them as owned values lets a caller
/// thread the same structural inputs through both the VM `Results` (here)
/// and a wasm-backed `Results` peer for parity testing, or move the
/// whole bundle into a worker-thread closure (the existing
/// `ltm_discovery_large_models.rs` use case).
///
/// All fields are owned data, so the returned value is naturally
/// `'static` (no borrows) -- no `Box::leak` is needed to produce it.
///
/// `vm_results` is the LTM-discovery-mode VM oracle; `causal_graph`,
/// `stocks`, `ltm_vars`, and `dims` are the four backend-independent
/// structural inputs `discover_loops_with_graph` accepts in production
/// (assembled exactly as `analysis.rs::run_ltm_pipeline` does).
#[allow(dead_code)]
pub struct LtmDiscoveryInputs {
    pub vm_results: Results,
    pub causal_graph: CausalGraph,
    pub stocks: Vec<Ident<Canonical>>,
    pub ltm_vars: Vec<LtmSyntheticVar>,
    pub dims: Vec<datamodel::Dimension>,
}

/// Compile (LTM discovery mode), simulate via the bytecode VM, and
/// assemble the element-level discovery inputs for an arbitrary
/// datamodel project.
///
/// Mirrors the production discovery path in
/// `analysis.rs::run_ltm_pipeline`: the element-level `CausalGraph` (via
/// `model_element_causal_edges` + `causal_graph_from_element_edges`),
/// the element-level `stocks` list, the `LtmSyntheticVar` metadata, and
/// the project dimensions -- the four arguments
/// `ltm_finding::discover_loops_with_graph` receives in production. The
/// salsa-borrowed values are cloned into owned ones so the bundle is
/// `'static`.
///
/// Single shared builder used by both the VM-side
/// `ltm_discovery_large_models.rs` test bundle and the wasm-vs-VM parity
/// harness in `simulate_ltm_wasm.rs`: keeping the structural-input
/// assembly in exactly one place satisfies the anti-divergence
/// principle at the harness level so the parity check truly compares
/// identical inputs.
///
/// Imperative Shell: drives the salsa compile pipeline and the VM.
#[allow(dead_code)]
pub fn ltm_discovery_inputs(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
) -> LtmDiscoveryInputs {
    use simlin_engine::db::{
        SimlinDb, causal_graph_from_element_edges, compile_project_incremental,
        model_element_causal_edges, model_ltm_variables, project_datamodel_dims,
        set_project_ltm_discovery_mode, set_project_ltm_enabled, sync_from_datamodel_incremental,
    };

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    set_project_ltm_discovery_mode(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, model_name)
        .expect("project should compile with LTM discovery enabled");

    let mut vm = Vm::new(compiled).expect("LTM VM construction should succeed");
    vm.run_to_end()
        .expect("LTM simulation should run to completion");
    let vm_results = vm.into_results();

    // Assemble the element-level discovery inputs exactly as the
    // production path does -- see `analysis.rs::run_ltm_pipeline`. These
    // four salsa-tracked results are `returns(ref)` (they borrow `db`);
    // clone them into owned values so the bundle outlives `db`.
    let source_model = sync.models[model_name].source_model;
    let element_edges = model_element_causal_edges(&db, source_model, sync.project);
    let causal_graph = causal_graph_from_element_edges(element_edges);
    let stocks: Vec<Ident<Canonical>> =
        element_edges.stocks.iter().map(|s| Ident::new(s)).collect();
    let ltm_vars = model_ltm_variables(&db, source_model, sync.project)
        .vars
        .clone();
    let dims = project_datamodel_dims(&db, sync.project).clone();

    LtmDiscoveryInputs {
        vm_results,
        causal_graph,
        stocks,
        ltm_vars,
        dims,
    }
}
