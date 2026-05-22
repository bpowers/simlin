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
use simlin_engine::wasmgen::{WasmGenError, WasmLayout, compile_simulation};
use simlin_engine::{Results, SimSpecs};
use wasm::validate;

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
fn run_wasm_results_segmented(wasm: &[u8], layout: &WasmLayout, targets: &[f64]) -> Vec<f64> {
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
