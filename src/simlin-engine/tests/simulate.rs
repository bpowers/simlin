// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

mod test_helpers;

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;

#[cfg(feature = "file_io")]
use simlin_engine::FilesystemDataProvider;
use simlin_engine::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
use simlin_engine::serde::{deserialize, serialize};
use simlin_engine::{Method, Results, SimSpecs as Specs, Vm, project_io};
use simlin_engine::{load_csv, load_dat, open_vensim, open_vensim_with_data, xmile};

use test_helpers::{WasmRunOutcome, ensure_results, ensure_results_excluding, ensure_wasm_matches};

const OUTPUT_FILES: &[(&str, u8)] = &[("output.csv", b','), ("output.tab", b'\t')];

static TEST_MODELS: &[&str] = &[
    // failing testcases (various reasons)
    // "test/test-models/tests/arguments/test_arguments.xmile",
    // "test/test-models/tests/delay_parentheses/test_delay_parentheses.xmile",
    // "test/test-models/tests/delay_pipeline/test_pipeline_delays.xmile",
    // "test/test-models/tests/rounding/test_rounding.xmile",
    // "test/test-models/tests/special_characters/test_special_variable_names.xmile",
    // "test/test-models/tests/stocks_with_expressions/test_stock_with_expression.xmile",

    // failing testcase: xmutil doesn't handle this correctly
    // "test/test-models/tests/subscript_mixed_assembly/test_subscript_mixed_assembly.xmile",
    //
    "test/test-models/samples/arrays/a2a/a2a.stmx",
    "test/test-models/samples/arrays/non-a2a/non-a2a.stmx",
    "test/test-models/samples/bpowers-hares_and_lynxes_modules/model.xmile",
    "test/test-models/samples/SIR/SIR.xmile",
    "test/test-models/samples/SIR/SIR_reciprocal-dt.xmile",
    "test/test-models/samples/teacup/teacup_w_diagram.xmile",
    "test/test-models/samples/teacup/teacup.xmile",
    "test/test-models/tests/abs/test_abs.xmile",
    "test/test-models/tests/builtin_max/builtin_max.xmile",
    "test/test-models/tests/builtin_mean/builtin_mean.xmile",
    "test/test-models/tests/builtin_min/builtin_min.xmile",
    "test/test-models/tests/builtin_int/builtin_int.xmile",
    "test/test-models/tests/chained_initialization/test_chained_initialization.xmile",
    "test/test-models/tests/comparisons/comparisons.xmile",
    "test/test-models/tests/constant_expressions/test_constant_expressions.xmile",
    "test/test-models/tests/delays2/delays.xmile",
    "test/test-models/tests/euler_step_vs_saveper/test_euler_step_vs_saveper.xmile",
    "test/test-models/tests/eval_order/eval_order.xmile",
    "test/test-models/tests/exponentiation/exponentiation.xmile",
    "test/test-models/tests/exp/test_exp.xmile",
    "test/test-models/tests/function_capitalization/test_function_capitalization.xmile",
    "test/test-models/tests/game/test_game.xmile",
    "test/test-models/tests/if_stmt/if_stmt.xmile",
    "test/test-models/tests/input_functions/test_inputs.xmile",
    "test/test-models/tests/limits/test_limits.xmile",
    "test/test-models/tests/line_breaks/test_line_breaks.xmile",
    "test/test-models/tests/line_continuation/test_line_continuation.xmile",
    "test/test-models/tests/ln/test_ln.xmile",
    "test/test-models/tests/logicals/test_logicals.xmile",
    "test/test-models/tests/log/test_log.xmile",
    "test/test-models/tests/lookups_inline_bounded/test_lookups_inline_bounded.xmile",
    "test/test-models/tests/lookups_inline/test_lookups_inline.xmile",
    "test/test-models/tests/lookups/test_lookups_no-indirect.xmile",
    // "test/test-models/tests/lookups/test_lookups.xmile",
    "test/test-models/tests/lookups_simlin/test_lookups.xmile",
    "test/test-models/tests/lookups_with_expr/test_lookups_with_expr.xmile",
    // Macro fixtures: each `<macro>` element imports as a macro-marked model
    // (Phase 5 Task 1 reader), the invocation expands and simulates against
    // `output.tab` (Phase 3), and `simulate_path_with` re-serializes the
    // project to XMILE and asserts a byte-stable round-trip (Phase 5 Task 2
    // writer). `macro_multi_macros` exercises two `<macro>` elements;
    // `macro_stock` a stock-bearing macro body. (The `.stmx` variants and the
    // `macro_cross_reference`/`macro_trailing_definition` dirs have no
    // `<macro>` element, so they are not wired here.)
    "test/test-models/tests/macro_expression/test_macro_expression.xmile",
    "test/test-models/tests/macro_multi_expression/test_macro_multi_expression.xmile",
    "test/test-models/tests/macro_multi_macros/test_macro_multi_macros.xmile",
    "test/test-models/tests/macro_stock/test_macro_stock.xmile",
    "test/test-models/tests/model_doc/model_doc.xmile",
    "test/test-models/tests/number_handling/test_number_handling.xmile",
    "test/test-models/tests/parentheses/test_parens.xmile",
    "test/test-models/tests/reference_capitalization/test_reference_capitalization.xmile",
    "test/test-models/tests/smooth_and_stock/test_smooth_and_stock.xmile",
    "test/test-models/tests/sqrt/test_sqrt.xmile",
    "test/test-models/tests/stocks_with_expressions/test_stock_with_expression.xmile",
    "test/test-models/tests/subscript_1d_arrays/test_subscript_1d_arrays.xmile",
    "test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
    "test/test-models/tests/subscript_3d_arrays/test_subscript_3d_arrays.xmile",
    "test/test-models/tests/subscript_docs/subscript_docs.xmile",
    "test/test-models/tests/subscript_individually_defined_1_of_2d_arrays_from_floats/subscript_individually_defined_1_of_2d_arrays_from_floats.xmile",
    "test/test-models/tests/subscript_individually_defined_1_of_2d_arrays/subscript_individually_defined_1_of_2d_arrays.xmile",
    "test/test-models/tests/subscript_multiples/test_multiple_subscripts.xmile",
    "test/test-models/tests/subscript_selection/subscript_selection.xmile",
    "test/test-models/tests/trend/test_trend.xmile",
    "test/test-models/tests/trig/test_trig.xmile",
    "test/test-models/tests/xidz_zidz/xidz_zidz.xmile",
    "test/test-models/tests/unicode_characters/unicode_test_model.xmile",
];

/// Monotonically-rising floor on how many `TEST_MODELS` the wasm backend runs
/// to VM parity (an outcome of `Ran` from `ensure_wasm_matches`). Pinned to the
/// count Phase 1 actually achieves; each subsequent phase widens the supported
/// feature set and RAISES this floor. Dropping below it is a regression
/// (wasm-backend AC3.1 / AC3.3): a model that used to clear the wasm backend no
/// longer does.
///
/// As of Phase 2 the backend covers the full *scalar* opcode set: the
/// scalar-core opcodes (`LoadConstant`/`LoadVar`/`LoadGlobalVar`, the
/// `Add`/`Sub`/`Mul`/`Div` and comparison `Op2`s, `Not`/`SetCond`/`If`,
/// `AssignCurr`/`AssignNext`, plus the `AssignConstCurr`/`BinOpAssign*` peephole
/// superinstructions), the `^`/`MOD`/`=`/`AND`/`OR` operators
/// (`Op2::Exp`/`Mod`/`Eq`/`And`/`Or`, with equality and truthiness routed
/// through a wasm `approx_eq` helper matching `crate::float::approx_eq`), and the
/// entire scalar `BuiltinId` set via `Opcode::Apply` -- the open-coded
/// transcendentals (`exp`/`ln`/`log10`/`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/
/// `pow`) plus `abs`/`sqrt`/`int`/`min`/`max`/`sign`/`quantum`/`safediv`/
/// `sshape` and the time-driven `step`/`ramp`/`pulse`. Phase 3 adds the scalar
/// `Opcode::Lookup` in all three modes (Interpolate / Forward / Backward): the
/// graphical-function tables are laid into linear memory with a per-table
/// directory, and three wasm helpers reproduce the VM's
/// `lookup`/`lookup_forward`/`lookup_backward`. A corpus model runs to parity
/// when its *post-element-expansion* flat opcode stream is entirely in that set.
/// That includes arrayed apply-to-all / subscript models that expand to purely
/// scalar per-element opcodes (no array-reducer or `LookupArray` opcode),
/// because the emitter walks the flattened opcode stream. Models that reach for
/// nested modules / macros (`wasmgen: submodules are not supported`),
/// array-reducer opcodes, or RK2/RK4 are `Skipped` until their phases land.
///
/// Phase 3 achieves 50 of the 58 active `TEST_MODELS` (up from Phase 2's 45):
/// the five graphical-function models that previously skipped on
/// `Opcode::Lookup` (`lookups_inline`, `lookups_inline_bounded`,
/// `lookups/test_lookups_no-indirect`, `lookups_simlin/test_lookups`,
/// `lookups_with_expr`) now `Ran`. The remaining 8 skip on the
/// still-out-of-scope constructs: nested modules / macros
/// (`bpowers-hares_and_lynxes_modules`, `delays2`, `smooth_and_stock`, `trend`,
/// and the four `macro_*` fixtures, each `wasmgen: submodules are not
/// supported`). Observed via `wasm_parity_floor` (run it with `-- --nocapture`
/// to see the per-model skip reasons).
const WASM_SUPPORTED_FLOOR: usize = 50;

/// AC3.1 / AC3.3 rising-floor gate: run every (non-`#[ignore]`-class) corpus
/// model in `TEST_MODELS` through the wasm backend and assert at least
/// `WASM_SUPPORTED_FLOOR` of them run to VM parity. `expected` is the VM's own
/// output (the parse + `compile_vm` + run path), so this is a direct
/// wasm-vs-VM check independent of the on-disk reference files; the per-model
/// inline hook (`wasm_parity_hook`) separately checks every supported model
/// against its on-disk `expected`.
///
/// Iterating the full `TEST_MODELS` list under the un-JITed DLR-FT interpreter
/// stays well within the suite's wall-clock budget at Phase 1 scope (the
/// supported models are small scalar models; unsupported ones bail at
/// `compile_simulation` before any interpreter run), so the gate covers the
/// whole list rather than a subset.
#[test]
fn wasm_parity_floor() {
    let mut ran = 0usize;
    let mut skipped = 0usize;
    for &path in TEST_MODELS {
        let file_path = format!("../../{path}");
        match wasm_parity_outcome_for_path(&file_path) {
            WasmRunOutcome::Ran => ran += 1,
            WasmRunOutcome::Skipped(msg) => {
                skipped += 1;
                eprintln!("wasm skipped {path}: {msg}");
            }
        }
    }
    eprintln!(
        "wasm_parity_floor: {ran} of {} corpus models ran to VM parity ({skipped} skipped); floor {WASM_SUPPORTED_FLOOR}",
        TEST_MODELS.len()
    );
    assert!(
        ran >= WASM_SUPPORTED_FLOOR,
        "wasm parity regression: only {ran} of {} corpus models ran to VM parity, \
         below the pinned floor of {WASM_SUPPORTED_FLOOR}. If this is an intended \
         narrowing, lower the floor deliberately; otherwise a model that used to \
         clear the wasm backend no longer does.",
        TEST_MODELS.len()
    );
}

/// Parse the XMILE/STMX model at `path`, run it through the VM for an `expected`
/// baseline, and return whether the wasm backend reproduces it (`Ran`) or skips
/// it as unsupported (`Skipped`). Used only by `wasm_parity_floor`. A parse or
/// VM failure is surfaced as `Skipped` (the VM corpus tests gate those paths
/// directly; the floor gate only counts wasm-vs-VM parity, never re-litigates
/// VM correctness).
fn wasm_parity_outcome_for_path(path: &str) -> WasmRunOutcome {
    let datamodel = {
        let Ok(f) = File::open(path) else {
            return WasmRunOutcome::Skipped(format!("could not open {path}"));
        };
        let mut f = BufReader::new(f);
        match xmile::project_from_reader(&mut f) {
            Ok(p) => p,
            Err(e) => return WasmRunOutcome::Skipped(format!("parse failed: {e}")),
        }
    };

    let expected = vm_results(&datamodel);
    ensure_wasm_matches(&datamodel, "main", &expected, &[])
}

/// Compile a datamodel project to a VM simulation using the incremental
/// salsa-backed path.
fn compile_vm(
    datamodel_project: &simlin_engine::datamodel::Project,
) -> simlin_engine::CompiledSimulation {
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, datamodel_project, None);
    compile_project_incremental(&db, sync.project, "main").unwrap()
}

fn load_expected_results(xmile_path: &str) -> Option<Results> {
    let xmile_name = std::path::Path::new(xmile_path).file_name().unwrap();
    let dir_path = &xmile_path[0..(xmile_path.len() - xmile_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    for (output_file, delimiter) in OUTPUT_FILES.iter() {
        let output_path = dir_path.join(output_file);
        if !output_path.exists() {
            continue;
        }
        return Some(load_csv(&output_path.to_string_lossy(), *delimiter).unwrap());
    }

    let dat_file = xmile_path.replace(".xmile", ".dat");
    let dat_path = std::path::Path::new(&dat_file);
    if dat_path.exists() {
        return Some(load_dat(&dat_file).unwrap());
    }

    None
}

/// Minimum fraction of `vdf_expected` variables that must match a `results`
/// ident before a comparison is considered non-vacuous. Below this floor, the
/// per-step tolerance loop runs over so few variables that `failures == 0` is
/// meaningless (the legacy comparator vacuously "passed" a comparison sharing
/// 0 or 1 ident). Both broad reference models exercise this far above the
/// floor: `simulates_wrld3_03` already asserts `offsets.len() > 200`, and
/// C-LEARN's `Ref.vdf` has ~3484 variables, ~3482 of which match a simulation
/// ident -- so a 10%-of-VDF floor (~348) is comfortably below the true matched
/// count yet far above 0/1.
const MIN_MATCHED_FRACTION: f64 = 0.10;

/// Hard floor on matched variables, applied when the VDF reference itself is
/// small. Keeps the synthetic guard test honest (a one-of-sixteen-matched
/// comparison must trip even though 16/10 rounds low) without constraining the
/// real broad-model comparisons, which clear it by orders of magnitude.
const MIN_MATCHED_ABSOLUTE: usize = 10;

/// Maximum fraction of compared (finite-reference) cells that may be skipped
/// because either side is IEEE NaN. `build_results` initializes unrecovered
/// VDF spans to `f64::NAN` (`vdf.rs`), and a simulation defect can emit NaN
/// columns; a high global NaN-skipped fraction means the comparison is largely
/// not actually comparing anything. The Vensim `:NA:` sentinel is the *finite*
/// `-2^109` (never NaN), so legitimate `:NA:` cells flow to the comparator, not
/// this guard. A correct broad comparison skips ~0% (verified against C-LEARN's
/// re-measure); 10% is a generous ceiling that still catches a degenerate run.
const MAX_NAN_SKIPPED_FRACTION: f64 = 0.10;

/// Cross-simulator relative tolerance (1%): VDF stores f32 (~7 digits) and
/// Vensim's integration may differ slightly from ours.
const VDF_RTOL: f64 = 0.01;

/// Per-series absolute-floor coefficient for the `isclose` criterion. The
/// absolute floor is `K_ATOL * peak`, where `peak` is the series' largest
/// reference magnitude, so a cell whose reference is a literal 0 tolerates
/// sim jitter up to `K_ATOL * peak` (the pure relative error is ~100% there and
/// would spuriously fail). Chosen small enough that a genuine >1% divergence on
/// a meaningful value still fails: at `K_ATOL = 1e-4` a near-zero cell tolerates
/// 0.01% of the series peak, far below any real divergence (validated against
/// C-LEARN's re-measure -- the genuine residual stays flagged). This is a
/// principled correction of the comparison at zero, NOT a relaxation for
/// meaningful values.
const K_ATOL: f64 = 1e-4;

/// Whether `ident` names an element of one of the excluded base variables.
/// A base name `"y"` excludes the scalar `y` and every arrayed element
/// `y[a1]`, `y[a2]`, ... (the VDF results key form), so a known-residual
/// variable can be carved out of the comparison without weakening the 1%
/// gate for any other variable. Mirrors `test_helpers::is_excluded_var`
/// (the CSV-comparison path) so both gates share identical base-name
/// semantics. This is a documented, tracked exclusion (every base maps to a
/// cluster in GH #590 / #591), NOT a tolerance change.
fn vdf_ident_is_excluded(ident: &str, excluded: &[&str]) -> bool {
    excluded.iter().any(|&base| {
        ident == base
            || ident
                .strip_prefix(base)
                .is_some_and(|rest| rest.starts_with('['))
    })
}

/// The base-variable name of a (possibly arrayed) VDF results ident: `"y"` for
/// the scalar `y`, and `"y"` for every element ident `y[a1]`, `y[a2]`, ... .
/// This is the inverse of [`vdf_ident_is_excluded`]'s base-name match (which
/// strips a trailing `[elem,...]` subscript), so the set of base names produced
/// here over the failing idents is exactly the set [`EXPECTED_VDF_RESIDUAL`]
/// must carve out. Used by `clearn_residual_exactness` to guard that the
/// exclusion stays neither over- nor under-broad.
fn vdf_ident_base_name(ident: &str) -> &str {
    match ident.find('[') {
        Some(idx) => &ident[..idx],
        None => ident,
    }
}

/// Per-ident comparison outcome, shared by the comparator (`ensure_vdf_results*`)
/// and the residual diagnostic so both apply byte-identical per-cell logic. The
/// pass/fail verdict for one matched `Ref.vdf` ident is fully described here.
#[derive(Default, Clone, Copy)]
struct VdfIdentStats {
    /// Cells that exceeded the near-zero-robust 1% tolerance (a real mismatch).
    failures: u64,
    /// `:NA:`-sentinel cells reconciled against a near-zero VDF reference.
    na_reconciled: u64,
    /// Cells skipped because either side was IEEE NaN.
    nan_skipped: u64,
    /// Cells actually compared (neither side NaN).
    compared: u64,
    /// True when the series was NaN at *every* step (an all-NaN core series).
    all_nan: bool,
    /// Largest per-cell relative error observed (for the diagnostic summary).
    max_rel_error: f64,
    /// The step at which `max_rel_error` occurred.
    max_rel_step: usize,
}

/// Classify one matched `(vdf_off, sim_off)` ident across all steps, applying
/// the full `ensure_vdf_results` per-cell contract: per-series peak/`atol`,
/// NaN-skip accounting, `:NA:`-sentinel reconciliation, and the near-zero-robust
/// `isclose` tolerance. Returning a struct (rather than mutating shared
/// accumulators inline) lets the comparator, its exclusion sibling, and the
/// temporary residual diagnostic agree exactly on which idents fail.
fn classify_vdf_ident(
    vdf_expected: &Results,
    results: &Results,
    vdf_off: usize,
    sim_off: usize,
) -> VdfIdentStats {
    let na = simlin_engine::float::NA;
    let step_count = vdf_expected.step_count;

    // Per-series peak reference magnitude drives the absolute floor for the
    // near-zero-robust `isclose` criterion. Finite reference cells only --
    // a NaN reference contributes nothing to the series' scale.
    let mut peak: f64 = 0.0;
    for step in 0..step_count {
        let expected = vdf_expected.data[step * vdf_expected.step_size + vdf_off];
        if expected.is_finite() {
            peak = peak.max(expected.abs());
        }
    }
    let atol = K_ATOL * peak;

    let mut stats = VdfIdentStats::default();
    let mut series_nan_skipped: u64 = 0;

    for step in 0..step_count {
        let expected = vdf_expected.data[step * vdf_expected.step_size + vdf_off];
        let actual = results.data[step * results.step_size + sim_off];

        if expected.is_nan() || actual.is_nan() {
            series_nan_skipped += 1;
            stats.nan_skipped += 1;
            continue;
        }
        stats.compared += 1;

        // `:NA:`-sentinel reconciliation: a SIM `:NA:` cell (the finite
        // -2^109 sentinel) is Vensim's "missing data", rendered as 0 in the
        // VDF. Reconcile it against a near-zero reference; flag it as a real
        // mismatch against a genuinely non-zero reference.
        if simlin_engine::float::approx_eq(actual, na) {
            if expected.abs() <= atol {
                stats.na_reconciled += 1;
            } else {
                stats.failures += 1;
            }
            continue;
        }

        // Near-zero-robust isclose: |e - a| <= atol + rtol * max(|e|, |a|).
        let scale = expected.abs().max(actual.abs());
        let allowed = atol + VDF_RTOL * scale;
        let abs_err = (expected - actual).abs();

        // Track a relative error for the diagnostic summary (clamped scale
        // mirrors the legacy report; the pass/fail decision is isclose).
        let rel_err = abs_err / scale.max(1e-10);
        if rel_err > stats.max_rel_error {
            stats.max_rel_error = rel_err;
            stats.max_rel_step = step;
        }

        if abs_err > allowed {
            stats.failures += 1;
        }
    }

    stats.all_nan = step_count > 0 && series_nan_skipped == step_count as u64;
    stats
}

/// Compare VDF reference data against simulation results with cross-simulator
/// tolerance. See [`ensure_vdf_results_excluding`] for the full contract; this
/// wrapper excludes nothing (the hard 1% gate applies to every matched var).
fn ensure_vdf_results(vdf_expected: &Results, results: &Results) {
    ensure_vdf_results_excluding(vdf_expected, results, &[]);
}

/// Compare VDF reference data against simulation results with cross-simulator
/// tolerance, skipping every matched ident whose base name is in `excluded`.
///
/// Contract (AC8.2 -- this comparator must not vacuously pass):
/// - **Matched-variable floor:** at least
///   `max(MIN_MATCHED_ABSOLUTE, MIN_MATCHED_FRACTION * |vdf vars|)` `Ref.vdf`
///   variables must match a `results` ident *after exclusion*, or the comparison
///   panics. A near-empty intersection can no longer "pass" by running an empty
///   loop, and the exclusion set cannot create a vacuous pass by carving the
///   matched count below the floor.
/// - **NaN guard:** any matched *core* series that is entirely NaN, or a global
///   NaN-skipped fraction above `MAX_NAN_SKIPPED_FRACTION`, panics. These are
///   *additional* failure conditions, never relaxations.
/// - **`:NA:`-sentinel reconciliation:** Simlin keeps Vensim's `:NA:` as the
///   finite sentinel `crate::float::NA` (`-2^109`); Vensim renders `:NA:` as 0
///   in the VDF. So a SIM `:NA:` cell matches a near-zero VDF cell (counted as
///   reconciled), but a SIM `:NA:` cell against a genuinely non-zero VDF value
///   is a real mismatch (a spurious `:NA:`) and fails. The engine output is
///   never mapped `:NA:`->0; only this comparator interprets the sentinel.
/// - **Near-zero-robust tolerance:** a cell matches when
///   `|e - a| <= atol + VDF_RTOL * max(|e|, |a|)`, with a per-series absolute
///   floor `atol = K_ATOL * peak` (`peak` = the series' largest reference
///   magnitude). This is the standard `isclose` criterion; it fixes the literal-0
///   relative-error breakdown (~100% for any jitter against a 0 reference) while
///   keeping a genuine >1% divergence on a meaningful value a failure.
/// - **Exclusion (`excluded`):** a matched ident whose base name is in `excluded`
///   is SKIPPED entirely -- it counts toward neither `matched`, the failure
///   count, nor the NaN guards. This is a TRANSPARENT, documented, tracked
///   known-residual carve-out (every excluded base maps to a cluster in GH #590 /
///   #591; see `EXPECTED_VDF_RESIDUAL`), NOT a tolerance loosening: the hard 1%
///   comparison stays unconditional for every NON-excluded variable, and the
///   matched floor is checked AFTER exclusion so the carve-out cannot vacuously
///   pass.
///
/// Variables present in `results` but not in `vdf_expected` are skipped (they
/// may be internal module variables without VDF entries).
fn ensure_vdf_results_excluding(vdf_expected: &Results, results: &Results, excluded: &[&str]) {
    assert_eq!(vdf_expected.step_count, results.step_count);

    let mut matched = 0;
    let mut excluded_matched = 0;
    let mut max_rel_error: f64 = 0.0;
    let mut max_rel_ident = String::new();
    let mut failures = 0;
    let mut na_reconciled: u64 = 0;
    // Global NaN-skip accounting across all matched cells.
    let mut total_compared: u64 = 0;
    let mut total_nan_skipped: u64 = 0;
    // Names of matched core series that were NaN at *every* step.
    let mut all_nan_series: Vec<String> = Vec::new();
    let step_count = vdf_expected.step_count;

    for ident in vdf_expected.offsets.keys() {
        if !results.offsets.contains_key(ident) {
            continue;
        }
        // Known-residual carve-out: skip an excluded base's idents BEFORE any
        // accounting so they touch neither the matched floor nor the guards.
        if vdf_ident_is_excluded(ident.as_str(), excluded) {
            excluded_matched += 1;
            continue;
        }
        let vdf_off = vdf_expected.offsets[ident];
        let sim_off = results.offsets[ident];
        matched += 1;

        let stats = classify_vdf_ident(vdf_expected, results, vdf_off, sim_off);
        na_reconciled += stats.na_reconciled;
        total_compared += stats.compared;
        total_nan_skipped += stats.nan_skipped;
        if stats.all_nan {
            all_nan_series.push(ident.to_string());
        }
        if stats.max_rel_error > max_rel_error {
            max_rel_error = stats.max_rel_error;
            max_rel_ident = format!("{ident} (step {})", stats.max_rel_step);
        }
        if stats.failures > 0 {
            failures += stats.failures;
            eprintln!(
                "FAIL {ident}: {} cell(s) exceeded tolerance (max rel_err {:.6} at step {})",
                stats.failures, stats.max_rel_error, stats.max_rel_step
            );
        }
    }

    let nan_fraction = if total_compared + total_nan_skipped > 0 {
        total_nan_skipped as f64 / (total_compared + total_nan_skipped) as f64
    } else {
        0.0
    };
    eprintln!(
        "VDF comparison: {matched} variables matched (after exclusion) across {step_count} time steps"
    );
    eprintln!(
        "  excluded (known-residual, tracked in #590/#591) idents skipped: {excluded_matched}"
    );
    eprintln!(
        "  matched floor (checked AFTER exclusion) = max({MIN_MATCHED_ABSOLUTE}, {MIN_MATCHED_FRACTION} * {}) = {}",
        vdf_expected.offsets.len(),
        min_matched(vdf_expected.offsets.len())
    );
    eprintln!("  Max relative error: {max_rel_error:.6} at {max_rel_ident}");
    eprintln!("  :NA:-sentinel cells reconciled to VDF 0: {na_reconciled}");
    eprintln!(
        "  NaN-skipped cells: {total_nan_skipped} of {} compared+skipped ({:.4})",
        total_compared + total_nan_skipped,
        nan_fraction
    );

    // Matched-variable floor (AC8.2): reject a vacuous near-empty comparison.
    let floor = min_matched(vdf_expected.offsets.len());
    assert!(
        matched >= floor,
        "VDF comparison vacuous: only {matched} of {} VDF variables matched a \
         simulation ident (floor {floor}); the comparison is not meaningfully \
         exercising the model",
        vdf_expected.offsets.len()
    );

    // NaN guard (AC8.2): an entirely-NaN core series, or an excessive global
    // NaN-skipped fraction, means the comparison is not actually comparing.
    assert!(
        all_nan_series.is_empty(),
        "VDF comparison degenerate: {} matched core series were entirely NaN \
         (e.g. {:?}); a NaN column compares nothing",
        all_nan_series.len(),
        &all_nan_series[..all_nan_series.len().min(5)]
    );
    assert!(
        nan_fraction <= MAX_NAN_SKIPPED_FRACTION,
        "VDF comparison degenerate: {:.2}% of matched cells were NaN-skipped \
         (ceiling {:.2}%); the comparison is largely not comparing anything",
        nan_fraction * 100.0,
        MAX_NAN_SKIPPED_FRACTION * 100.0
    );

    if failures > 0 {
        eprintln!("  {failures} comparisons exceeded tolerance");
        panic!("VDF comparison failed with {failures} tolerance violations");
    }
}

/// The matched-variable floor for a VDF reference with `vdf_var_count`
/// variables (see [`MIN_MATCHED_FRACTION`] / [`MIN_MATCHED_ABSOLUTE`]).
fn min_matched(vdf_var_count: usize) -> usize {
    MIN_MATCHED_ABSOLUTE.max((vdf_var_count as f64 * MIN_MATCHED_FRACTION) as usize)
}

/// Build a synthetic [`Results`] from `(name, series)` pairs for the
/// `ensure_vdf_results` guard test. Each series must have `step_count` values;
/// columns are laid out densely in argument order (so `step_size == names.len()`).
#[cfg(test)]
fn synthetic_results(columns: &[(&str, Vec<f64>)]) -> Results {
    use simlin_engine::common::{Canonical, Ident};

    let step_count = columns.first().map(|(_, s)| s.len()).unwrap_or(0);
    for (name, series) in columns {
        assert_eq!(
            series.len(),
            step_count,
            "synthetic_results: column {name} has {} steps, expected {step_count}",
            series.len()
        );
    }
    let step_size = columns.len();

    let mut offsets: HashMap<Ident<Canonical>, usize> = HashMap::new();
    let mut data = vec![0.0_f64; step_count * step_size];
    for (col, (name, series)) in columns.iter().enumerate() {
        offsets.insert(Ident::<Canonical>::new(name), col);
        for (step, value) in series.iter().enumerate() {
            data[step * step_size + col] = *value;
        }
    }

    Results {
        offsets,
        data: data.into_boxed_slice(),
        step_size,
        step_count,
        specs: Specs {
            start: 0.0,
            stop: (step_count.max(1) - 1) as f64,
            dt: 1.0,
            save_step: 1.0,
            method: Method::Euler,
            n_chunks: step_count,
        },
        is_vensim: false,
    }
}

/// AC4.5: a NaN-vs-`:NA:` series (Vensim writes literal IEEE NaN where Simlin
/// writes the finite `:NA:` sentinel `-2^109`, e.g. C-LEARN's
/// `slr_inches_from_2000`) must be handled by the comparator's NaN-skip path
/// and must NOT enter the failure set, so it never needs an
/// `EXPECTED_VDF_RESIDUAL` entry. This pins that contract on a SYNTHETIC series
/// (no C-LEARN parse, no C-LEARN name), so it is independent of the hero model:
///   - every cell whose VDF side is IEEE NaN is counted `nan_skipped`, never
///     `failures` (the `:NA:`-sentinel SIM value is irrelevant -- the NaN guard
///     fires first);
///   - the late finite steps that DO match within tolerance are `compared` with
///     zero `failures`;
///   - because at least one finite step is compared, the series is NOT `all_nan`.
///
/// Together (`failures == 0 && !all_nan`) means `clearn_residual_exactness`'s
/// membership predicate (`failures > 0 || all_nan`) is false, so such a series
/// is never added to the residual set. (An ENTIRELY-NaN core series is a
/// different, separately-guarded degeneracy -- see the all-NaN assertions in
/// `ensure_vdf_results_excluding` and scenario (2) of the vacuous-comparison
/// test; the realistic NaN-vs-`:NA:` case has finite tail steps and is partial.)
#[test]
fn classify_vdf_ident_nan_vs_na_skips_without_failing() {
    let na = simlin_engine::float::NA;
    // `float::NA` is the finite Vensim `:NA:` sentinel, NOT IEEE NaN.
    assert!(na.is_finite(), "the :NA: sentinel must be finite, not NaN");
    assert!(
        (na - (-(2.0_f64).powi(109))).abs() < 1e18,
        ":NA: sentinel is -2^109"
    );

    // Model `slr_inches_from_2000`: early steps are NaN on the VDF side (Vensim
    // literal IEEE NaN) while SIM carries the finite `:NA:` sentinel; the late
    // steps are finite and match within the 1% tolerance.
    let vdf = synthetic_results(&[(
        "synthetic_nan_na_series",
        vec![f64::NAN, f64::NAN, f64::NAN, 2.5, 5.0, 7.5],
    )]);
    let sim = synthetic_results(&[(
        "synthetic_nan_na_series",
        // Where VDF is NaN, SIM is the finite `:NA:` sentinel; where VDF is
        // finite, SIM matches it (within 1%).
        vec![na, na, na, 2.5, 5.0, 7.5],
    )]);
    let vdf_off = vdf.offsets[&simlin_engine::common::Ident::new("synthetic_nan_na_series")];
    let sim_off = sim.offsets[&simlin_engine::common::Ident::new("synthetic_nan_na_series")];

    let stats = classify_vdf_ident(&vdf, &sim, vdf_off, sim_off);

    // The three NaN-on-VDF-side cells are skipped (the `:NA:` SIM value never
    // reaches the tolerance check); the three finite matching cells compare clean.
    assert_eq!(
        stats.nan_skipped, 3,
        "the NaN-on-VDF cells must be NaN-skipped"
    );
    assert_eq!(stats.compared, 3, "the finite tail cells must be compared");
    assert_eq!(
        stats.failures, 0,
        "a NaN-vs-:NA: series with matching finite tail must produce NO failures"
    );
    assert!(
        !stats.all_nan,
        "a partial-NaN series (finite tail) is NOT all-NaN"
    );
    // No spurious `:NA:` reconciliation: the SIM `:NA:` cells were NaN-skipped
    // (VDF NaN), so they never reach the `:NA:`-sentinel reconciliation branch.
    assert_eq!(stats.na_reconciled, 0);

    // The membership predicate `clearn_residual_exactness` uses to build the
    // live residual set: this series is excluded from it, so it never needs an
    // `EXPECTED_VDF_RESIDUAL` carve-out (AC4.5). The exclusion holds *because*
    // the series has a finite tail (like `slr_inches_from_2000`); the test name
    // names only this partial-NaN / finite-tail case, NOT the general
    // NaN-vs-:NA: contract.
    let enters_residual = stats.failures > 0 || stats.all_nan;
    assert!(
        !enters_residual,
        "a partial-NaN (finite-tail) NaN-vs-:NA: series must NOT enter the residual failure set"
    );

    // Boundary made executable: an ENTIRELY-NaN VDF series (Vensim literal NaN
    // at every step) is the separately-guarded exception. With no finite cell
    // to compare, `all_nan` is set and the SAME membership predicate flips true,
    // so such a series WOULD enter the residual set (it is handled by the
    // all-NaN guards in `ensure_vdf_results_excluding`, not by this finite-tail
    // skip path). This pins exactly why the test's scope is the finite-tail case.
    let all_nan_vdf = synthetic_results(&[(
        "synthetic_all_nan_series",
        vec![f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN, f64::NAN],
    )]);
    let all_na_sim =
        synthetic_results(&[("synthetic_all_nan_series", vec![na, na, na, na, na, na])]);
    let all_nan_off =
        all_nan_vdf.offsets[&simlin_engine::common::Ident::new("synthetic_all_nan_series")];
    let all_na_sim_off =
        all_na_sim.offsets[&simlin_engine::common::Ident::new("synthetic_all_nan_series")];
    let all_nan_stats = classify_vdf_ident(&all_nan_vdf, &all_na_sim, all_nan_off, all_na_sim_off);
    assert!(
        all_nan_stats.all_nan,
        "an entirely-NaN VDF series must be flagged all_nan (the separately-guarded exception)"
    );
    assert!(
        all_nan_stats.failures > 0 || all_nan_stats.all_nan,
        "an all-NaN series WOULD enter the residual set -- it is NOT covered by the finite-tail skip"
    );
}

/// AC4.4 guard: the C-LEARN VDF carve-out (`EXPECTED_VDF_RESIDUAL`) is a
/// documented membership EXCLUSION, never a tolerance loosening. This pins the
/// five comparator constants to their exact values AND proves the matched-variable
/// floor still PANICS when too few variables match (so the gate -- which checks
/// the floor AFTER exclusion -- can never pass vacuously by carving the matched
/// set below the floor). If a future change loosens any constant or weakens the
/// floor, this test fails before any C-LEARN gate is even run. Uses synthetic
/// inputs only (no C-LEARN parse, no C-LEARN name).
#[test]
fn vdf_comparator_constants_and_floor_are_pinned() {
    // 1. Pin the five comparator constants (AC4.4): a drift in any of these is a
    //    silent tolerance/floor change that must be a deliberate, reviewed edit.
    assert_eq!(VDF_RTOL, 0.01, "the 1% cross-simulator tolerance is fixed");
    assert_eq!(
        K_ATOL, 1e-4,
        "the per-series absolute-floor coefficient is fixed"
    );
    assert_eq!(
        MIN_MATCHED_FRACTION, 0.10,
        "the matched-fraction floor is fixed"
    );
    assert_eq!(
        MIN_MATCHED_ABSOLUTE, 10,
        "the absolute matched floor is fixed"
    );
    assert_eq!(
        MAX_NAN_SKIPPED_FRACTION, 0.10,
        "the global NaN-skipped ceiling is fixed"
    );

    // 2. The floor formula itself is `max(absolute, fraction * count)`.
    assert_eq!(
        min_matched(16),
        10,
        "16 vars: absolute floor (10) dominates"
    );
    assert_eq!(
        min_matched(5000),
        500,
        "5000 vars: fractional floor (10%) dominates"
    );

    // 3. The gate cannot pass vacuously: a comparison whose matched count is
    //    below the floor must PANIC even though every matched cell is in
    //    tolerance (the legacy `failures == 0` would pass it). 16 reference
    //    variables (floor = 10) with only ONE shared, in-tolerance ident.
    let names: &[&str] = &[
        "v00", "v01", "v02", "v03", "v04", "v05", "v06", "v07", "v08", "v09", "v10", "v11", "v12",
        "v13", "v14", "v15",
    ];
    let expected: Vec<(&str, Vec<f64>)> = names.iter().map(|n| (*n, vec![1.0, 1.0, 1.0])).collect();
    let sim_below: Vec<(&str, Vec<f64>)> = vec![("v00", vec![1.0, 1.0, 1.0])];
    let e = synthetic_results(&expected);
    let a = synthetic_results(&sim_below);
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let below = std::panic::catch_unwind(|| ensure_vdf_results(&e, &a));
    // A control: enough matched vars, all in tolerance, must NOT panic -- proving
    // the panic above is the FLOOR firing, not an unrelated failure.
    let a_ok = synthetic_results(&expected);
    let e_ok = synthetic_results(&expected);
    let ok = std::panic::catch_unwind(|| ensure_vdf_results(&e_ok, &a_ok));
    std::panic::set_hook(prev_hook);
    assert!(
        below.is_err(),
        "a below-floor comparison (1 of 16 matched) must PANIC, not pass vacuously"
    );
    assert!(
        ok.is_ok(),
        "an above-floor, in-tolerance comparison must pass (control)"
    );
}

/// AC8.2: the hardened `ensure_vdf_results` must FAIL (panic), rather than
/// vacuously pass, on a near-empty or degenerate comparison, while the
/// user-directed comparator reconciles the legitimate `:NA:`-sentinel and
/// near-zero cases. Each scenario builds synthetic `Results` literals (no
/// C-LEARN parse -- fast, runs in the default capped suite) and asserts the
/// panic/no-panic outcome via `catch_unwind`. This is a legitimate
/// test-of-a-panicking-assertion (distinct from the production `catch_unwind`
/// retired in Phase 6): `ensure_vdf_results` signals failure by panicking, so
/// the only way to assert "it failed" from a sibling test is to catch it.
#[test]
fn ensure_vdf_results_rejects_vacuous_comparisons() {
    // catch_unwind would otherwise dump each intentional panic's backtrace to
    // stderr, drowning the test log; silence the default hook for the duration.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let outcome = std::panic::catch_unwind(run_vacuous_comparison_scenarios);
    std::panic::set_hook(prev_hook);
    if let Err(payload) = outcome {
        std::panic::resume_unwind(payload);
    }
}

fn run_vacuous_comparison_scenarios() {
    let na = simlin_engine::float::NA;

    // A broad VDF reference: many variables, several steps each. The matched
    // floor is a fraction of this count, so a comparison that shares almost no
    // idents falls below it.
    let many = |fill: f64| -> Vec<(&'static str, Vec<f64>)> {
        let names: &[&str] = &[
            "alpha", "bravo", "charlie", "delta", "echo", "foxtrot", "golf", "hotel", "india",
            "juliet", "kilo", "lima", "mike", "november", "oscar", "papa",
        ];
        names
            .iter()
            .map(|n| (*n, vec![fill, fill, fill, fill]))
            .collect()
    };

    let asserts_panic = |label: &str, expected: &[(&str, Vec<f64>)], sim: &[(&str, Vec<f64>)]| {
        let e = synthetic_results(expected);
        let a = synthetic_results(sim);
        let r = std::panic::catch_unwind(|| ensure_vdf_results(&e, &a));
        assert!(
            r.is_err(),
            "{label}: expected ensure_vdf_results to PANIC, but it passed"
        );
    };
    let asserts_ok = |label: &str, expected: &[(&str, Vec<f64>)], sim: &[(&str, Vec<f64>)]| {
        let e = synthetic_results(expected);
        let a = synthetic_results(sim);
        let r = std::panic::catch_unwind(|| ensure_vdf_results(&e, &a));
        assert!(
            r.is_ok(),
            "{label}: expected ensure_vdf_results to PASS, but it panicked"
        );
    };

    // (1) Below-floor: VDF has 16 idents; sim shares only ONE (`alpha`). The
    //     per-step loop runs for one matched var -- finite, in tolerance -- so
    //     the legacy `failures == 0` check passes vacuously. The matched-floor
    //     guard must reject it.
    asserts_panic(
        "below-floor (1 of 16 matched)",
        &many(1.0),
        &[("alpha", vec![1.0, 1.0, 1.0, 1.0])],
    );

    // (2) Entirely-NaN core series: every shared ident matches, but one matched
    //     series is NaN at every step (the `build_results` all-NaN-unrecovered-
    //     span case). The legacy loop NaN-skips every cell -> `failures == 0`,
    //     vacuous pass. The NaN guard must reject an all-NaN core series.
    let mut expected_nan = many(2.0);
    let mut sim_nan = many(2.0);
    // Make `alpha`'s sim series entirely NaN.
    sim_nan[0].1 = vec![f64::NAN; 4];
    asserts_panic("entirely-NaN core series", &expected_nan, &sim_nan);

    // (3) Excessive NaN-skipped fraction: many matched cells are NaN-skipped
    //     (here, ~half of all matched cells across vars), even though no single
    //     series is entirely NaN. The global NaN-skipped-fraction guard must
    //     reject it.
    expected_nan = many(2.0);
    sim_nan = many(2.0);
    // Push the global NaN-skipped fraction past the threshold by NaN-ing 3 of
    // 4 steps in 12 of 16 vars (~56% of cells skipped).
    for (_n, series) in sim_nan.iter_mut().take(12) {
        series[0] = f64::NAN;
        series[1] = f64::NAN;
        series[2] = f64::NAN;
    }
    asserts_panic("excessive NaN-skipped fraction", &expected_nan, &sim_nan);

    // (4) Positive control: enough matched vars, all finite, all in tolerance.
    //     Guards must NOT false-trip.
    asserts_ok("positive control (well-formed)", &many(3.0), &many(3.0));

    // (5) `:NA:`-sentinel == Vensim 0: SIM is the finite `:NA:` sentinel where
    //     VDF renders `:NA:` as 0. This must reconcile as a MATCH, not fail.
    let mut expected_na = many(3.0);
    let mut sim_na = many(3.0);
    expected_na[0].1 = vec![0.0; 4]; // VDF renders :NA: as 0
    sim_na[0].1 = vec![na; 4]; // Simlin keeps the -2^109 sentinel
    asserts_ok(":NA:-sentinel vs VDF 0 (reconciled)", &expected_na, &sim_na);

    // (6) Spurious `:NA:`: SIM is `:NA:` but VDF has a genuine non-zero value.
    //     Simlin is spuriously `:NA:` where Vensim has data -- a REAL mismatch
    //     that must be caught, never silently reconciled.
    expected_na = many(3.0);
    sim_na = many(3.0);
    expected_na[0].1 = vec![42.0; 4]; // VDF genuinely non-zero
    sim_na[0].1 = vec![na; 4]; // Simlin spuriously :NA:
    asserts_panic(
        "spurious :NA: (SIM NA vs VDF nonzero)",
        &expected_na,
        &sim_na,
    );

    // (7a) Near-zero robustness, benign: VDF is a literal 0 and SIM is tiny
    //      jitter within the per-series absolute floor. The pure relative error
    //      would be ~100%; the abs+rel criterion must let it pass.
    let mut expected_nz = many(3.0);
    let mut sim_nz = many(3.0);
    // Series whose peak magnitude is ~10; a 1e-6 jitter at the 0 cell is far
    // below k*peak for any reasonable k. Use a non-zero peak elsewhere so the
    // per-series abs floor is meaningful.
    expected_nz[0].1 = vec![10.0, 0.0, 5.0, 0.0];
    sim_nz[0].1 = vec![10.0, 1e-6, 5.0, -1e-6];
    asserts_ok("near-zero jitter within abs floor", &expected_nz, &sim_nz);

    // (7b) Near-zero but MEANINGFUL: VDF is 0 yet SIM is a large value (same
    //      order as the series peak). The abs floor must NOT swallow this -- it
    //      is a genuine divergence and must fail.
    expected_nz = many(3.0);
    sim_nz = many(3.0);
    expected_nz[0].1 = vec![10.0, 0.0, 5.0, 0.0];
    sim_nz[0].1 = vec![10.0, 8.0, 5.0, 0.0]; // 8.0 at a cell where VDF is 0
    asserts_panic("near-zero but meaningful divergence", &expected_nz, &sim_nz);
}

/// Run the named model of `datamodel` through the VM and return its
/// `Results`, used as the `expected` baseline both the focused
/// `ensure_wasm_matches` tests and `wasm_parity_floor` compare wasm output
/// against. Mirrors the corpus VM path (`compile_vm` -> `Vm::new` ->
/// `run_to_end`).
fn vm_results(datamodel: &simlin_engine::datamodel::Project) -> Results {
    let compiled = compile_vm(datamodel);
    let mut vm = Vm::new(compiled).unwrap();
    vm.run_to_end().unwrap();
    vm.into_results()
}

/// AC1.1: a scalar Euler model the wasm backend supports runs through
/// `ensure_wasm_matches` and clears the same `ensure_results` comparator the VM
/// clears (the helper panics internally on any divergence), so the outcome is
/// `Ran`.
#[test]
fn ensure_wasm_matches_runs_supported_scalar_model() {
    let datamodel = simlin_engine::test_common::TestProject::new("simple")
        .with_sim_time(0.0, 10.0, 1.0)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();

    let expected = vm_results(&datamodel);
    let outcome = ensure_wasm_matches(&datamodel, "main", &expected, &[]);
    assert!(
        matches!(outcome, WasmRunOutcome::Ran),
        "a supported scalar model must run through the wasm backend, got {outcome:?}"
    );
}

/// AC3.1: a model using a not-yet-supported construct is SKIPPED, not failed --
/// `compile_simulation` returns `WasmGenError::Unsupported` and the helper
/// surfaces it as `Skipped(msg)` carrying that message.
///
/// The `^` operator (`Op2::Exp`) used to be the example here, but it is
/// supported as of Phase 2 Task 3; RK4 integration is a stable still-unsupported
/// construct (`compile_simulation` rejects any non-Euler method until the RK
/// phase lands), so it now drives the `Skipped` path.
#[test]
fn ensure_wasm_matches_skips_unsupported_model() {
    let datamodel = simlin_engine::test_common::TestProject::new("unsupported")
        .with_sim_time(0.0, 5.0, 1.0)
        .with_sim_method(simlin_engine::datamodel::SimMethod::RungeKutta4)
        .aux("inflow_rate", "2", None)
        .stock("level", "0", &["inflow"], &[], None)
        .flow("inflow", "inflow_rate", None)
        .build_datamodel();

    let expected = vm_results(&datamodel);
    let outcome = ensure_wasm_matches(&datamodel, "main", &expected, &[]);
    match outcome {
        WasmRunOutcome::Skipped(msg) => {
            assert!(
                !msg.is_empty(),
                "a Skipped outcome should carry the Unsupported message"
            );
        }
        WasmRunOutcome::Ran => {
            panic!("a model using unsupported RK4 integration must be Skipped, not Ran")
        }
    }
}

type CompileFn = fn(&simlin_engine::datamodel::Project) -> simlin_engine::CompiledSimulation;

fn simulate_path(xmile_path: &str) {
    simulate_path_with(xmile_path, compile_vm);
}

fn simulate_path_with(xmile_path: &str, compile: CompileFn) {
    simulate_path_with_excluding(xmile_path, compile, &[]);
}

/// Like [`simulate_path_with`], but carves a set of base-variable names out
/// of the comparison *and* the compiled model. Each excluded variable is
/// (a) removed from every datamodel model before compilation -- so a single
/// not-yet-supported variable does not block the whole model from
/// compiling -- and (b) skipped in every comparison path (VM, protobuf
/// round-trip, XMILE round-trip). Every *other* variable stays a hard
/// genuine-Vensim equality gate. Used to keep `vector.xmile`'s ELM MAP
/// base/full-source variables (`c`/`f`/`g`) as hard gates while excluding
/// only `y` (GitHub #578), rather than weakening the whole comparison.
fn simulate_path_with_excluding(xmile_path: &str, compile: CompileFn, excluded: &[&str]) {
    eprintln!("model: {xmile_path}");

    let datamodel_project = {
        let f = File::open(xmile_path).unwrap();
        let mut f = BufReader::new(f);

        let datamodel_project = xmile::project_from_reader(&mut f);
        if let Err(ref err) = datamodel_project {
            eprintln!("model '{xmile_path}' error: {err}");
        }
        let mut datamodel_project = datamodel_project.unwrap();
        // Drop excluded variables from every model. An excluded variable is
        // one we intentionally do not gate on yet (tracked separately); if
        // it fails to compile it would otherwise abort the whole project
        // and prevent gating the variables we DO support.
        for model in &mut datamodel_project.models {
            model
                .variables
                .retain(|v| !excluded.contains(&v.get_ident()));
        }
        datamodel_project
    };

    // simulate the model using our bytecode VM
    let results = {
        let compiled_sim = compile(&datamodel_project);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };

    let expected = load_expected_results(xmile_path).unwrap();
    ensure_results_excluding(&expected, &results, excluded);

    // serialize our project through protobufs and ensure we don't see problems
    let results_proto = {
        use simlin_engine::prost::Message;

        let pb_project_inner = serialize(&datamodel_project).unwrap();
        let pb_project = &pb_project_inner;
        let mut buf = Vec::with_capacity(pb_project.encoded_len());
        pb_project.encode(&mut buf).unwrap();

        let datamodel_project2 = deserialize(project_io::Project::decode(&*buf).unwrap());
        assert_eq!(datamodel_project, datamodel_project2);
        let compiled_sim = compile(&datamodel_project2);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        vm.into_results()
    };
    ensure_results_excluding(&expected, &results_proto, excluded);

    // serialize our project back to XMILE
    let serialized_xmile = xmile::project_to_xmile(&datamodel_project).unwrap();

    // and then read it back in from the XMILE string and simulate it
    let (roundtripped_project, results_xmile) = {
        let mut xmile_reader = BufReader::new(serialized_xmile.as_bytes());
        let roundtripped_project = xmile::project_from_reader(&mut xmile_reader).unwrap();

        let compiled_sim = compile(&roundtripped_project);
        let mut vm = Vm::new(compiled_sim).unwrap();
        vm.run_to_end().unwrap();
        (roundtripped_project, vm.into_results())
    };
    ensure_results_excluding(&expected, &results_xmile, excluded);

    // finally ensure that if we re-serialize to XMILE the results are
    // byte-for-byte identical (we aren't losing any information)
    let serialized_xmile2 = xmile::project_to_xmile(&roundtripped_project).unwrap();
    assert_eq!(&serialized_xmile, &serialized_xmile2);

    // wasm-backend parity: after the VM comparisons pass, run the model through
    // the wasm backend once and assert it clears the SAME comparator against the
    // same `expected`. A supported model that diverges panics inside the helper;
    // an out-of-scope construct is skipped (counted against the rising floor in
    // `wasm_parity_floor`, not failed here). See AC1.1 / AC3.1.
    wasm_parity_hook(&datamodel_project, &expected, excluded);
}

/// Run one already-parsed model through the wasm backend and assert parity (the
/// helper panics on a supported-but-divergent model). A `Skipped` outcome (an
/// out-of-scope construct) is logged, not failed -- the inline corpus coverage
/// stays opportunistic, while `wasm_parity_floor` pins the supported count.
fn wasm_parity_hook(
    datamodel: &simlin_engine::datamodel::Project,
    expected: &Results,
    excluded: &[&str],
) {
    if let WasmRunOutcome::Skipped(msg) = ensure_wasm_matches(datamodel, "main", expected, excluded)
    {
        eprintln!("  wasm backend skipped (unsupported): {msg}");
    }
}

fn load_expected_results_for_mdl(mdl_path: &str) -> Option<Results> {
    let mdl_name = std::path::Path::new(mdl_path).file_name().unwrap();
    let dir_path = &mdl_path[0..(mdl_path.len() - mdl_name.len())];
    let dir_path = std::path::Path::new(dir_path);

    for (output_file, delimiter) in OUTPUT_FILES.iter() {
        let output_path = dir_path.join(output_file);
        if !output_path.exists() {
            continue;
        }
        return Some(load_csv(&output_path.to_string_lossy(), *delimiter).unwrap());
    }

    let dat_file = mdl_path.replace(".mdl", ".dat");
    let dat_path = std::path::Path::new(&dat_file);
    if dat_path.exists() {
        return Some(load_dat(&dat_file).unwrap());
    }

    None
}

/// Simulate a Vensim MDL file via the native parser, running the VM
/// and comparing against expected output.
fn simulate_mdl_path(mdl_path: &str) {
    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    let expected = load_expected_results_for_mdl(mdl_path)
        .unwrap_or_else(|| panic!("no reference data found for {mdl_path}"));
    ensure_results(&expected, &results);

    wasm_parity_hook(&datamodel_project, &expected, &[]);
}

/// Simulate a Vensim MDL file that references external data files.
/// Uses FilesystemDataProvider to resolve GET DIRECT references.
#[cfg(feature = "file_io")]
fn simulate_mdl_path_with_data(mdl_path: &str) {
    eprintln!("model (vensim mdl with data): {mdl_path}");

    let mdl_abs = std::path::Path::new(mdl_path);
    let model_dir = mdl_abs
        .parent()
        .unwrap_or_else(|| panic!("no parent dir for {mdl_path}"));

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let provider = FilesystemDataProvider::new(model_dir);
    let datamodel_project = open_vensim_with_data(&contents, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    let expected = load_expected_results_for_mdl(mdl_path)
        .unwrap_or_else(|| panic!("no reference data found for {mdl_path}"));
    ensure_results(&expected, &results);

    wasm_parity_hook(&datamodel_project, &expected, &[]);
}

#[test]
fn simulates_models_correctly() {
    for &path in TEST_MODELS {
        let file_path = format!("../../{path}");
        simulate_path(file_path.as_str());
    }
}

#[test]
fn simulates_aliases() {
    simulate_path("../../test/alias1/alias1.stmx");
}

#[test]
fn simulates_init_builtin() {
    simulate_path("../../test/builtin_init/builtin_init.stmx");
}

#[test]
fn simulates_arrays() {
    simulate_path("../../test/arrays1/arrays.stmx");
}

#[test]
fn simulates_array_sum_simple() {
    simulate_path("../../test/array_sum_simple/array_sum_simple.xmile");
}

#[test]
fn simulates_array_sum_expr() {
    simulate_path("../../test/array_sum_expr/array_sum_expr.xmile");
}

#[test]
fn simulates_array_multi_source() {
    // Tests multi-array expressions like SUM(a[*] + b[*])
    // This exercises the LoadIterViewTop opcode which loads from each array's
    // own view rather than a shared iteration view.
    simulate_path("../../test/array_multi_source/array_multi_source.xmile");
}

#[test]
fn simulates_array_broadcast() {
    // Tests cross-dimension broadcasting like sales[Region,Product] * price[Region]
    // where price is broadcast over the Product dimension.
    // This verifies that dimension IDs are correctly matched during iteration.
    simulate_path("../../test/array_broadcast/array_broadcast.xmile");
}

#[test]
fn simulates_modules() {
    simulate_path("../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx");
}

#[test]
fn simulates_modules2() {
    simulate_path("../../test/modules2/modules2.xmile");
}

#[test]
fn simulates_circular_dep_1() {
    simulate_path("../../test/circular-dep-1/model.stmx");
}

#[test]
fn simulates_previous() {
    simulate_path("../../test/previous/model.stmx");
}

#[test]
fn simulates_modules_with_complex_idents() {
    simulate_path("../../test/modules_with_complex_idents/modules_with_complex_idents.stmx");
}

#[test]
fn simulates_step_into_smth1() {
    simulate_path("../../test/step_into_smth1/model.stmx");
}

#[test]
fn simulates_subscript_index_name_values() {
    simulate_path("../../test/subscript_index_name_values/model.stmx");
}

#[test]
fn simulates_active_initial() {
    simulate_path("../../test/sdeverywhere/models/active_initial/active_initial.xmile");
}

#[test]
fn simulates_lookup() {
    simulate_path("../../test/sdeverywhere/models/lookup/lookup.xmile");
}

// Ignored: xmutil drops EXCEPT semantics and subscript mappings when converting
// MDL to XMILE. The XMILE file has incorrect/incomplete equations.
#[test]
#[ignore]
fn simulates_except_xmile() {
    simulate_path("../../test/sdeverywhere/models/except/except.xmile");
}

#[test]
fn simulates_except() {
    simulate_mdl_path("../../test/sdeverywhere/models/except/except.mdl");
}

#[test]
fn simulates_except2() {
    simulate_mdl_path("../../test/sdeverywhere/models/except2/except2.mdl");
}

#[test]
fn simulates_sum() {
    simulate_path("../../test/sdeverywhere/models/sum/sum.xmile");
}

/// End-to-end test for EXCEPT through the MDL->simulation pipeline.
/// Uses a model without cross-dimension mappings so it doesn't hit the
/// DimD->DimA mapping limitation that blocks the full except/except2 models.
#[test]
fn simulates_except_basic_mdl() {
    let mdl = "\
{UTF-8}
DimA: A1, A2, A3 ~~|
SubA: A2, A3 ~~|
g[DimA] :EXCEPT: [A1] = 7 ~~|
g[A1] = 10 ~~|
h[DimA] :EXCEPT: [SubA] = 8 ~~|
p[DimA] :EXCEPT: [A1] = 2 ~~|
p[A1] = 5 ~~|
s[A3] = 13 ~~|
s[SubA] :EXCEPT: [A3] = 14 ~~|
u[DimA] :EXCEPT: [A1] = 1 ~~|
u[A1] = 99 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let datamodel_project =
        open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse except_basic mdl: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    let get = |name: &str| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().next().unwrap()[off]
    };

    // g[DimA] :EXCEPT: [A1] = 7, g[A1] = 10
    assert!((get("g[a1]") - 10.0).abs() < 1e-10, "g[A1] should be 10");
    assert!((get("g[a2]") - 7.0).abs() < 1e-10, "g[A2] should be 7");
    assert!((get("g[a3]") - 7.0).abs() < 1e-10, "g[A3] should be 7");

    // h[DimA] :EXCEPT: [SubA] = 8 (no overrides for A2, A3)
    assert!((get("h[a1]") - 8.0).abs() < 1e-10, "h[A1] should be 8");
    assert!(
        (get("h[a2]") - 0.0).abs() < 1e-10,
        "h[A2] should be 0 (undefined)"
    );
    assert!(
        (get("h[a3]") - 0.0).abs() < 1e-10,
        "h[A3] should be 0 (undefined)"
    );

    // p[DimA] :EXCEPT: [A1] = 2, p[A1] = 5
    assert!((get("p[a1]") - 5.0).abs() < 1e-10, "p[A1] should be 5");
    assert!((get("p[a2]") - 2.0).abs() < 1e-10, "p[A2] should be 2");
    assert!((get("p[a3]") - 2.0).abs() < 1e-10, "p[A3] should be 2");

    // s[A3] = 13, s[SubA] :EXCEPT: [A3] = 14 => s[A2]=14, s[A3]=13
    assert!((get("s[a2]") - 14.0).abs() < 1e-10, "s[A2] should be 14");
    assert!((get("s[a3]") - 13.0).abs() < 1e-10, "s[A3] should be 13");

    // u[DimA] :EXCEPT: [A1] = 1, u[A1] = 99
    assert!((get("u[a1]") - 99.0).abs() < 1e-10, "u[A1] should be 99");
    assert!((get("u[a2]") - 1.0).abs() < 1e-10, "u[A2] should be 1");
    assert!((get("u[a3]") - 1.0).abs() < 1e-10, "u[A3] should be 1");
}

/// Vensim `:NA:` is a *finite* sentinel value (`-2^109`, the "missing data"
/// marker), NOT IEEE NaN. The canonical Vensim idiom is the existence test
/// `IF THEN ELSE(x = :NA:, fallback, x)`, which only works because `:NA:` is a
/// finite number ordinary `=` equality can match. Representing it as NaN poisons
/// every downstream arithmetic expression (NaN is absorbing), which is the engine
/// bug behind C-LEARN's residual all-NaN cascade.
///
/// This test pins the corrected semantics end-to-end through the real
/// MDL -> XMILE -> compile -> VM pipeline: `probe` produces `:NA:` for `Time > 5`,
/// the existence test recovers a finite fallback there, and `:NA:` arithmetic
/// stays finite. Under the old NaN representation `na_plus = :NA: + 10` is NaN,
/// so this is RED before the fix.
#[test]
fn na_existence_test_and_arithmetic_finite() {
    let mdl = "\
{UTF-8}
probe =
	IF THEN ELSE(Time > 5, :NA:, Time)
	~~|
out =
	IF THEN ELSE(probe = :NA:, -1, probe)
	~~|
na_plus =
	:NA: + 10
	~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 10 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let datamodel_project =
        open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse na test mdl: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    let off = |name: &str| -> usize {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        results.offsets[&ident]
    };
    let at =
        |name: &str, step: usize| -> f64 { results.data[step * results.step_size + off(name)] };

    // The single canonical Vensim `:NA:` sentinel: a finite number, never NaN.
    let na = simlin_engine::float::NA;
    assert!(na.is_finite(), ":NA: sentinel must be finite");

    // 11 saved steps for Time 0..=10.
    assert_eq!(results.step_count, 11, "expected 11 saved steps");

    for step in 0..results.step_count {
        let time = at("time", step);
        let probe = at("probe", step);
        let out = at("out", step);
        let na_plus = at("na_plus", step);

        // `:NA:` arithmetic is finite (NaN would poison this -- the core bug).
        assert!(
            na_plus.is_finite(),
            "na_plus (:NA: + 10) must be finite at t={time}, got {na_plus}"
        );
        assert!(
            (na_plus - (na + 10.0)).abs() < 1e-10,
            "na_plus must equal -2^109 + 10 at t={time}, got {na_plus}"
        );

        if time > 5.0 {
            // probe evaluates to the finite :NA: sentinel for Time > 5.
            assert!(
                (probe - na).abs() < 1e-10,
                "probe must be the :NA: sentinel for t={time}, got {probe}"
            );
            // The existence test fires: out takes the fallback (-1), finite.
            assert!(
                (out - (-1.0)).abs() < 1e-10,
                "existence test must select fallback (-1) for t={time}, got {out}"
            );
        } else {
            // probe is the genuine Time value; existence test does NOT fire.
            assert!(
                (probe - time).abs() < 1e-10,
                "probe must equal Time for t={time}, got {probe}"
            );
            assert!(
                (out - time).abs() < 1e-10,
                "out must equal probe (=Time) for t={time}, got {out}"
            );
        }
    }
}

#[test]
fn simulates_2d_array() {
    simulate_path(
        "../../test/test-models/tests/subscript_2d_arrays/test_subscript_2d_arrays.xmile",
    );
}

// Commented out: test_generator approach is useful for discovery but generates many tests.
// Use simulates_arrayed_models_correctly below for the curated list.
// #[test_generator::test_resources("test/sdeverywhere/models/**/*.xmile")]
// fn simulates_sdeverywhere(resource: &str) {
//     let resource = format!("../../{}", resource);
//     simulate_path(&resource);
// }

/// SDEverywhere test models from test/sdeverywhere/models/**/*.xmile
/// These are Vensim models converted to XMILE format.
static TEST_SDEVERYWHERE_MODELS: &[&str] = &[
    // Passing tests
    "test/sdeverywhere/models/active_initial/active_initial.xmile",
    "test/sdeverywhere/models/comments/comments.xmile",
    "test/sdeverywhere/models/delay/delay.xmile",
    "test/sdeverywhere/models/elmcount/elmcount.xmile",
    "test/sdeverywhere/models/index/index.xmile",
    "test/sdeverywhere/models/initial/initial.xmile",
    "test/sdeverywhere/models/lookup/lookup.xmile",
    "test/sdeverywhere/models/pulsetrain/pulsetrain.xmile",
    "test/sdeverywhere/models/sir/sir.xmile",
    "test/sdeverywhere/models/smooth/smooth.xmile",
    "test/sdeverywhere/models/smooth3/smooth3.xmile",
    "test/sdeverywhere/models/specialchars/specialchars.xmile",
    "test/sdeverywhere/models/subalias/subalias.xmile",
    "test/sdeverywhere/models/trend/trend.xmile",
    //
    // xmutil strips GET DIRECT CONSTANTS data during conversion. Tested via MDL path.
    // "test/sdeverywhere/models/directconst/directconst.xmile",
    "test/sdeverywhere/models/longeqns/longeqns.xmile",
    "test/sdeverywhere/models/npv/npv.xmile",
    "test/sdeverywhere/models/sample/sample.xmile",
    "test/sdeverywhere/models/sum/sum.xmile",
    //
    // --- XMILE-path limitations (xmutil conversion issues) ---
    //
    // Tested via simulates_allocate_xmile
    // "test/sdeverywhere/models/allocate/allocate.xmile",
    //
    // xmutil converts DELAY FIXED into delay1 approximation which produces NaN.
    // Tested via MDL path.
    // "test/sdeverywhere/models/delayfixed/delayfixed.xmile",
    // "test/sdeverywhere/models/delayfixed2/delayfixed2.xmile",
    //
    // xmutil strips GET DIRECT CONSTANTS during XMILE conversion, leaving
    // empty equations. Not yet fully testable via MDL path: the MDL parser
    // now handles +/- signed literals in number lists and mixed fixed-element/
    // dimension subscripts, but multiple TabbedArray definitions for the same
    // variable (e.g. z[C1,...] and z[C2,...]) are not yet merged into a single
    // Arrayed equation.
    // "test/sdeverywhere/models/arrays_cname/arrays_cname.xmile",
    // "test/sdeverywhere/models/arrays_varname/arrays_varname.xmile",
    //
    // xmutil strips GET DIRECT DATA during conversion, leaving empty equations.
    // Tested via the MDL path (simulates_directdata_mdl etc.).
    // "test/sdeverywhere/models/directdata/directdata.xmile",
    //
    // xmutil strips GET DIRECT LOOKUPS during conversion
    // "test/sdeverywhere/models/directlookups/directlookups.xmile",
    //
    // xmutil strips GET DIRECT SUBSCRIPT during conversion
    // "test/sdeverywhere/models/directsubs/directsubs.xmile",
    //
    // xmutil drops EXCEPT semantics and subscript mappings in XMILE conversion.
    // MDL path tests exist (simulates_except, simulates_except2) but remain
    // #[ignore] due to MismatchedDimensions errors and missing output variables
    // (z[a1] absent). Resolving these requires further work on dimension mapping
    // for variables with EXCEPT and subscript-mapped dimensions.
    // "test/sdeverywhere/models/except/except.xmile",
    // "test/sdeverywhere/models/except2/except2.xmile",
    //
    // xmutil strips GET XLS DATA during conversion. The MDL file has variables
    // with no equations (e.g. D Values, BC Values) that Vensim populates from
    // companion extdata_data.dat via implicit per-run data loading, which the
    // engine does not auto-discover. Not testable via MDL path.
    // "test/sdeverywhere/models/extdata/extdata.xmile",
    //
    // xmutil strips GET DATA BETWEEN TIMES calls, leaving broken data variable
    // references. The MDL path also cannot simulate this model: the normalizer
    // wraps GET DATA BETWEEN TIMES in opaque {GET DATA(...)} references, which
    // the XMILE equation lexer silently discards as comments, producing empty
    // equations. Additionally, Values[DimA] requires external data from
    // getdata_data.dat via implicit per-run loading. Not testable via MDL path.
    // "test/sdeverywhere/models/getdata/getdata.xmile",
    //
    // xmutil drops subscript mappings in XMILE conversion.
    // Tested via MDL path.
    // "test/sdeverywhere/models/mapping/mapping.xmile",
    // "test/sdeverywhere/models/multimap/multimap.xmile",
    //
    // xmutil doesn't inline external data from prune_data.dat into XMILE.
    // The MDL file has variables with no equations (A Values, BC Values, D Values,
    // etc.) that Vensim populates from prune_data.dat via implicit per-run data
    // loading, which the engine does not auto-discover. Not testable via MDL path.
    // "test/sdeverywhere/models/prune/prune.xmile",
    //
    // xmutil expands QUANTUM(x,q) -> (q)*INT((x)/(q)), but INT is floor
    // per XMILE spec while Vensim INTEGER is truncation-toward-zero.
    // This gives wrong results for negative inputs. Tested via MDL path.
    // "test/sdeverywhere/models/quantum/quantum.xmile",
    //
    // xmutil drops subscript mappings. Tested via MDL path.
    // "test/sdeverywhere/models/subscript/subscript.xmile",
    //
    // --- Engine limitations ---
    //
    // NotSimulatable: element-level circular dependency (ce[t2] depends on ecc[t1],
    // ecc[t1] depends on ce[t1]) -- requires element-level dependency resolution
    // "test/sdeverywhere/models/ref/ref.xmile",
    //
    // NotSimulatable: element-level circular dependency via both XMILE and MDL paths
    // "test/sdeverywhere/models/interleaved/interleaved.xmile",
    //
    // Vensim implicit data variable loading: "A Values[DimA]" has no equation
    // in the MDL -- values come from the companion sumif_data.dat file via Vensim's
    // implicit per-run data loading, distinct from GET DIRECT DATA. The engine has no
    // mechanism to auto-discover and load companion .dat files by convention.
    // The SUM(IF ...) arithmetic pattern itself is covered by
    // array_tests::sum_of_conditional_tests.
    // "test/sdeverywhere/models/sumif/sumif.xmile",
    //
    // VECTOR ELM MAP now matches genuine Vensim (per-element base + full
    // source array, out-of-range -> :NA:, no modulo). vector.xmile is
    // exercised through all three comparison paths by the dedicated
    // `simulates_vector_xmile_genuine` test below, NOT here: that test keeps
    // c/f/g (the ELM MAP base/full-source variables) and every other
    // variable as hard genuine-Vensim gates against
    // test/sdeverywhere/models/vector/vector.dat, narrowing the comparison
    // to exclude only two pre-existing, separately-tracked, out-of-scope
    // variables -- `y` (GitHub #578: scalar-source/expression-offset ELM MAP
    // does not compile) and `p` (GitHub #576: dormant/unverified 2-D VECTOR
    // SORT ORDER fixture data; a different builtin, unchanged here). This
    // list runs an unconditional full comparison, which cannot carve out
    // those variables; the narrowed gate lives in its own test instead of
    // weakening every model's comparison.
    // "test/sdeverywhere/models/vector/vector.xmile",  // -> simulates_vector_xmile_genuine
    //
    // --- Permanently excluded (not test models) ---
    //
    // Preprocessing test files with no simulation output
    // "test/sdeverywhere/models/flatten/expected.xmile",
    // "test/sdeverywhere/models/flatten/input1.xmile",
    // "test/sdeverywhere/models/flatten/input2.xmile",
    // "test/sdeverywhere/models/preprocess/expected.xmile",
    // "test/sdeverywhere/models/preprocess/input.xmile",
    //
    // Nested model directory duplicate, no .dat
    // "test/sdeverywhere/models/sir/model/sir.xmile",
];

#[test]
fn simulates_arrayed_models_correctly() {
    for &path in TEST_SDEVERYWHERE_MODELS {
        let file_path = format!("../../{path}");
        simulate_path(file_path.as_str());
    }
}

/// Genuine-Vensim regression gate for VECTOR ELM MAP cross-dimension
/// resolution (element-cycle-resolution.AC6.3 / AC6.2). `vector.xmile` is
/// compared against the real-Vensim `vector.dat` through all three
/// `ensure_results` paths (VM, protobuf round-trip, XMILE round-trip).
///
/// Every variable is a hard genuine-Vensim equality gate. In particular the
/// ELM MAP base/full-source variables this phase fixes must match
/// `vector.dat` exactly: `c[A1..A3] = 10 + VECTOR ELM MAP(b[B1], a[DimA])`
/// is `11, 12, 12`; `f[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], a[DimA])` is
/// `1, 5, 6` (broadcast across DimB); and
/// `g[DimA,DimB] = VECTOR ELM MAP(d[DimA,B1], e[DimA,DimB])` is
/// `1, 4, 5, 2, 3, 6`. So do every non-ELM-MAP variable the model also
/// exercises (1-D VSO `l`/`m`, VECTOR SELECT `q`/`r`/`s`, reducers
/// `u`/`v`/`w`, and the rest).
///
/// Exactly two variables are carved out, both pre-existing and
/// separately-tracked gaps unrelated to the ELM MAP base/full-source fix
/// (per the phase file's "prefer full inclusion; narrow only with a tracked
/// issue" guidance). First, `y[DimA] = VECTOR ELM MAP(x[three], (DimA-1))`
/// (GitHub #578): a scalar source plus an arithmetic (expression) offset
/// from which ELM MAP cannot yet infer a result shape, so `y` does not
/// compile at all -- a compiler shape-inference gap, NOT the base/stride
/// numeric bug fixed here; its genuine value `y[A1]=3,y[A2]=4,y[A3]=5` is
/// in `vector.dat`, and closing #578 lets `y` rejoin this gate. Second,
/// `p[DimA,DimB] = VECTOR SORT ORDER(o[DimA,DimB], ASCENDING)` (GitHub
/// #576): a genuinely 2-D VSO whose `vector.dat` `p` block is internally
/// inconsistent / encodes an sdeverywhere per-row semantic, with
/// genuine-Vensim multi-dimensional VSO semantics unverified by any live
/// fixture -- a different builtin (VSO, unchanged by this phase) out of
/// Phase 5's ELM MAP scope.
///
/// Excluded variables are dropped from the compiled model (so #578's
/// non-compiling `y` cannot abort the project) and skipped in every
/// comparison; the genuine gate on `c`/`f`/`g` (and all other variables)
/// is NOT weakened.
#[test]
fn simulates_vector_xmile_genuine() {
    simulate_path_with_excluding(
        "../../test/sdeverywhere/models/vector/vector.xmile",
        compile_vm,
        // y: GitHub #578 (scalar-source/expression-offset ELM MAP compile
        // gap). p: GitHub #576 (dormant/unverified 2-D VSO fixture data).
        // Both pre-existing and out of Phase 5's ELM MAP scope.
        &["y", "p"],
    );
}

#[test]
fn simulates_lookup_arrayed() {
    simulate_path("../../test/lookup_arrayed/lookup_arrayed.xmile");
}

#[test]
fn simulates_delay_arrayed() {
    simulate_path("../../test/sdeverywhere/models/delay/delay.xmile");
}

#[test]
fn simulates_smooth3() {
    simulate_path("../../test/sdeverywhere/models/smooth3/smooth3.xmile");
}

#[test]
fn simulates_smooth_with_dim_mappings() {
    simulate_path("../../test/sdeverywhere/models/smooth/smooth.xmile");
}

#[test]
fn simulates_subscript_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/subscript/subscript.mdl");
}

#[test]
fn simulates_mapping_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/mapping/mapping.mdl");
}

#[test]
fn simulates_multimap_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/multimap/multimap.mdl");
}

#[test]
fn simulates_npv_xmile() {
    simulate_path("../../test/sdeverywhere/models/npv/npv.xmile");
}

#[test]
fn simulates_npv_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/npv/npv.mdl");
}

// DELAY FIXED requires ring-buffer (pipeline delay) semantics, not
// exponential smoothing (delay1).  Currently mapped to delay1 as a rough
// approximation; these tests are ignored until VM-level ring buffer state
// is implemented.
#[test]
#[ignore]
fn simulates_delayfixed_xmile() {
    simulate_path("../../test/sdeverywhere/models/delayfixed/delayfixed.xmile");
}

#[test]
#[ignore]
fn simulates_delayfixed_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/delayfixed/delayfixed.mdl");
}

#[test]
#[ignore]
fn simulates_delayfixed2_xmile() {
    simulate_path("../../test/sdeverywhere/models/delayfixed2/delayfixed2.xmile");
}

#[test]
#[ignore]
fn simulates_delayfixed2_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/delayfixed2/delayfixed2.mdl");
}

#[test]
fn simulates_sample_xmile() {
    simulate_path("../../test/sdeverywhere/models/sample/sample.xmile");
}

#[test]
fn simulates_sample_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/sample/sample.mdl");
}

#[test]
fn simulates_quantum_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/quantum/quantum.mdl");
}

#[test]
fn simulates_vector_simple_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/vector_simple/vector_simple.mdl");
}

#[test]
fn simulates_allocate_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/allocate/allocate.mdl");
}

#[test]
fn simulates_allocate_xmile() {
    simulate_path("../../test/sdeverywhere/models/allocate/allocate.xmile");
}

#[test]
fn simulates_longeqns_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/longeqns/longeqns.mdl");
}

// Ignored: xmutil strips GET DATA BETWEEN TIMES calls in XMILE conversion,
// leaving zeroed-out equations for variables that depend on the data.
#[test]
#[ignore]
fn simulates_getdata_xmile() {
    simulate_path("../../test/sdeverywhere/models/getdata/getdata.xmile");
}

// Ignored: two blocking issues prevent MDL path simulation.
// (1) The MDL normalizer wraps GET DATA BETWEEN TIMES in opaque {GET DATA(...)}
//     references, which the XMILE equation lexer discards as comments, producing
//     empty equations for variables like value_for_a1_at_time_minus_half_year_backward.
// (2) Values[DimA] has no equation in the MDL and must be populated from
//     getdata_data.dat via Vensim's implicit per-run data loading, which the
//     engine does not auto-discover. Fixing requires DataProvider integration
//     for implicit companion .dat files.
#[test]
#[ignore]
fn simulates_getdata_mdl() {
    simulate_mdl_path("../../test/sdeverywhere/models/getdata/getdata.mdl");
}

#[test]
fn bad_model_name() {
    let f = File::open(format!("../../{}", TEST_MODELS[0])).unwrap();
    let mut f = BufReader::new(f);
    let datamodel_project = xmile::project_from_reader(&mut f).unwrap();
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    assert!(compile_project_incremental(&db, sync.project, "blerg").is_err());
}

#[test]
fn verifies_ai_information_generated_then_edited() {
    verify_ai_information("../../test/ai-information/GeneratedByAIThenEdited.stmx");
}

#[test]
fn verifies_ai_information_pure_ai() {
    verify_ai_information("../../test/ai-information/PureAIModel.stmx");
}

#[test]
fn verifies_ai_information_pure_human() {
    verify_ai_information("../../test/ai-information/PureHumanModel.stmx");
}

#[test]
fn verifies_ai_information_with_modules_and_arrays() {
    verify_ai_information("../../test/ai-information/WithModulesAndArrays.stmx");
}

fn verify_ai_information(xmile_path: &str) {
    let known_keys = HashMap::from([(
        "https://iseesystems.com/keys/stella01.txt",
        "AAAAC3NzaC1lZDI1NTE5AAAAIP5Rg+bCssFIB2b2F9H/lUhVBXwtrBCtyRgiiq9RYkXS",
    )]);

    eprintln!("model: {xmile_path}");

    let f = File::open(xmile_path).unwrap();
    let mut f = BufReader::new(f);

    let datamodel_project = xmile::project_from_reader(&mut f);
    if let Err(ref err) = datamodel_project {
        eprintln!("model '{xmile_path}' error: {err}");
    }

    #[allow(unused_variables)]
    let datamodel_project = datamodel_project.unwrap();

    let ai_info = datamodel_project.ai_information.as_ref().unwrap();
    let key_bytes_encoded = known_keys[ai_info.status.key_url.as_str()];

    use base64::{Engine as _, engine::general_purpose};
    let key_bytes = general_purpose::STANDARD.decode(key_bytes_encoded).unwrap();

    let openssh_pubkey = ssh_key::PublicKey::from_bytes(&key_bytes).unwrap();
    let raw_pubkey = openssh_pubkey.key_data().ed25519().unwrap();

    // OpenSSH format: skip the first 19 bytes to get to the actual 32-byte Ed25519 key
    let key = ed25519_dalek::VerifyingKey::from_bytes(&raw_pubkey.0).unwrap();

    simlin_engine::ai_info::verify(&datamodel_project, &key).unwrap()
}

#[test]
fn simulates_wrld3_03() {
    let mdl_path = "../../test/metasd/WRLD3-03/wrld3-03.mdl";

    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    // Verify VDF parsing and record-based extraction succeed on the WRLD3
    // reference data. Full series-level comparison is checked by the
    // `simulates_clearn` path; here we only confirm the decoder recovers a
    // broad column set with the right time axis.
    let vdf_path = "../../test/metasd/WRLD3-03/SCEN01.VDF";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));
    let vdf_results = vdf_file
        .to_results_via_records()
        .unwrap_or_else(|e| panic!("VDF to_results_via_records failed: {e}"));
    assert!(
        vdf_results.offsets.len() > 200,
        "WRLD3: expected broad record-based mapping, got {}",
        vdf_results.offsets.len()
    );
    assert_eq!(vdf_results.step_count, results.step_count);
}

/// Known-residual C-LEARN base-variable names excluded from the
/// `simulates_clearn` VDF gate. C-LEARN compiles via the incremental path,
/// runs to FINAL TIME, and matches `Ref.vdf` within the 1% cross-simulator
/// tolerance on the overwhelming majority of its 3482 matched idents; the
/// short, fully-attributed remainder below (21 base variables) is the proven
/// genuine residual after Phases 1-3 of the C-LEARN-residual plan. Phases 1-3
/// reconciled the bulk that earlier sat here: the inline-data graph-lookup
/// `0+0` zeroing (#590, fixed Phase 1 -- lookup-only lowers to `gf(Time)`),
/// RAMP FROM TO import linearization (Phase 2), the passthrough-INIT +
/// dt-runlist ordering (Phase 3), and the init-runlist nondeterminism
/// (`e24b0080`). The user-directed decision was to BANK that match and TRACK
/// the residual rather than chase the rest to within 1%.
///
/// This is a TRANSPARENT, documented, tracked carve-out, NOT a tolerance
/// loosening: the hard 1% comparison stays unconditional for every NON-excluded
/// variable, and the matched floor is checked AFTER exclusion (3360 matched vs a
/// 348 floor, ~9.7x -- the exclusion cannot create a vacuous pass; pinned by
/// `vdf_comparator_constants_and_floor_are_pinned`). Each base below carries a
/// one-line sourced reason under one of five categories
/// (engine-genuine-tracked / VDF-decode-artifact / benign-near-zero /
/// NaN-vs-`:NA:` / boundary); none is "unknown".
///
/// This set is GUARDED for exactness by the committed `clearn_residual_exactness`
/// regression test (run it to re-derive/verify after an engine change:
/// `cargo test --release -- --ignored clearn_residual_exactness`). That test
/// re-runs C-LEARN through the same `classify_vdf_ident` comparator with NO
/// exclusion and asserts the live failing set equals this list, failing loudly
/// (with the symmetric difference) if the residual grew (a regression) or shrank
/// (a fix that should prune the exclusion).
const EXPECTED_VDF_RESIDUAL: &[&str] = &[
    // ===== engine-genuine-tracked (0): NONE =====
    // The only engine-genuine divergence -- the init-runlist nondeterminism
    // (which permuted `depth_at_bottom`'s per-layer init values) -- was fixed in
    // `e24b0080` (deterministic init runlist). `clearn_residual_exactness` now
    // runs deterministically, and no base below is the engine producing a wrong
    // value on a meaningful magnitude. (#595 tracks the remaining deeper init-
    // ordering soundness gap; its nondeterminism half is fixed by `e24b0080`.)

    // ===== lookup-only tables (0): NONE =====
    // A bare graphical function is a *table indexed by an explicit input*, not a
    // time series. The engine no longer synthesises a phantom `gf(Time)` series
    // for a standalone lookup-only table: such a table is not a value-bearing
    // variable, so it is excluded from the runlist and from the saved output
    // (#606 RESOLVED; this was the `gf(Time)` lowering introduced for #590). The
    // VDF reader still emits a ghost column for the few descriptors it cannot
    // model-free distinguish from a real owner -- the `rs_hfc*` family (whose
    // descriptor forward-links to the wider 2-D consumer `RS HFC[COP, HFC type]`)
    // and `ref_global_emissions_from_graph_lookup` (forward link Time/0) -- but
    // since the engine now produces NO sim series for those idents, the
    // comparator simply skips them (a VDF ident with no sim counterpart is not a
    // mismatch), so NONE remain in this carve-out. Their data, where it matters,
    // is carried by the consumer variables that call them, emitted as ordinary
    // owners and matched normally.

    // ===== benign-near-zero (2): divergence ONLY on near-zero magnitudes =====
    // (cross-simulator f32/integration noise), never on a meaningful value.
    // Tracked in #591 cluster 2.
    "co2eq_gap_closing_percentage", // ratio of near-equal small values; peak ~1.1e-2 (vdf -6.8e-4 vs sim 6.9e-4).
    "diffusion_flux", // ~2% on a small early transient (~7% of cells, vdf 0 vs sim ~5e-10); matches late.
    // ===== NaN-vs-:NA: (0): NONE in this list (by construction) =====
    // Where Vensim writes literal IEEE NaN and Simlin writes the finite `:NA:`
    // sentinel (`-2^109`, e.g. `slr_inches_from_2000`), the comparator NaN-skips
    // those cells and the finite tail matches within tolerance, so the series is
    // neither all-NaN nor has failing cells and never reaches the failure set.
    // Pinned independently by `classify_vdf_ident_nan_vs_na_skips_without_failing`.
    // Tracked in #591 for the representation gap, not excluded here.

    // ===== boundary (2): match everywhere except :NA:-arithmetic cells =====
    // Vensim computes `-2*NA = -1.298e33` while Simlin keeps the bare `:NA:`
    // sentinel `-6.49e32` (`crate::float::NA`); NA-arithmetic is confirmed
    // CORRECT and explicitly out of scope (see `float::NA`). The genuine
    // documented remainder, tracked in #591 cluster 1. (`historical_gdp_lookup`
    // -- the lookup table whose only divergence was this same -2*NA tail -- is
    // now dropped as a lookup-only table, so only the genuine consumer
    // `historical_gdp` remains here.)
    "historical_gdp", // non-:NA: cells match exactly (e.g. [oecd_us] 6.667e4); only -2*NA cells diverge.
    "last_set_target_year", // INIT(VECTOR SELECT(...)); every cell is -2*NA (vdf -1.298e33 vs sim -6.49e32 sentinel).
];

// FULL end-to-end C-LEARN simulation against `Ref.vdf`. Un-stubbed (no longer a
// permanently-skipped placeholder): C-LEARN compiles via the incremental path,
// runs to FINAL TIME, and matches `Ref.vdf` within the 1% cross-simulator
// tolerance on the reconciled ~94.7% of matched idents (~96.3% per-cell). Kept `#[test] #[ignore]`
// purely for RUNTIME CLASS (C-LEARN is ~53k lines / 1.4 MB, ~5s just to parse on
// release), so the capped default `cargo test` set stays under the 3-minute cap;
// run it explicitly via `--ignored` (AC8.3).
//
// History (the formerly-listed blockers are CLEARED on this branch):
//   * C-LEARN's four macros (SAMPLE UNTIL, SSHAPE, RAMP FROM TO, INIT) parse,
//     register, and expand with zero macro-attributable diagnostics (the macro
//     work, asserted by `corpus_clearn_macros_import` and the focused
//     `simulates_macro_clearn_*` fixtures).
//   * The previously-fatal `CircularDependency` on
//     `main.previous_emissions_intensity_vs_refyr` was the FALSE whole-variable
//     cycle-gate verdict; element-level cycle resolution (Phases 1-2) dissolves
//     it. The formerly-listed `MismatchedDimensions` / `UnknownDependency` /
//     `DoesNotExist` blockers (incl. the `"goal 1.5 for temperature"` quoted-
//     period ident, #559) are likewise cleared.
//
// What remains is a short, fully-attributed residual of 4 base variables
// (Phases 1-3 reconciled the rest; the reader DROPS standalone lookup-only
// descriptors -- bare graphical-function tables, not series -- and the ENGINE
// now produces no series at all for a lookup-only table (#606 RESOLVED), so
// every lookup-only ident leaves the comparison entirely), explicitly excluded
// via `EXPECTED_VDF_RESIDUAL` under a taxonomy: 2 benign-near-zero; and 2
// `:NA:`-arithmetic boundary series (#591). The engine
// is PROVEN correct on all of them; the engine-genuine and NaN-vs-`:NA:`
// categories are empty (the sole engine-genuine divergence, init-runlist
// nondeterminism, was fixed in `e24b0080`). The exclusion is a transparent,
// documented, tracked carve-out -- NOT a tolerance loosening; the hard 1% gate
// holds for every non-excluded variable and the matched floor is checked after
// exclusion. Run with: cargo test --release -- --ignored simulates_clearn
#[test]
#[ignore]
fn simulates_clearn() {
    let (vdf_results, results) = run_clearn_vs_vdf();

    // Hard 1% gate for every NON-excluded variable; the documented, tracked
    // EXPECTED_VDF_RESIDUAL bases (#597 reader-artifact / #591 residual) are
    // skipped. The matched floor is enforced AFTER exclusion, so this cannot
    // vacuously pass.
    ensure_vdf_results_excluding(&vdf_results, &results, EXPECTED_VDF_RESIDUAL);
}

/// Compile and run C-LEARN end-to-end and parse `Ref.vdf`, returning
/// `(vdf_results, results)`. Shared by `simulates_clearn` (the 1% gate) and
/// `clearn_residual_exactness` (the exclusion-exactness guard) so both exercise
/// the byte-identical `open_vensim` -> `compile_vm` -> `run_to_end` -> parse-VDF
/// path and compare the same data. Heavy (C-LEARN is ~53k lines / 1.4 MB,
/// ~5s just to parse on release), so every caller is `#[ignore]`d.
fn run_clearn_vs_vdf() -> (Results, Results) {
    let mdl_path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";

    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    let vdf_path = "../../test/xmutil_test_models/Ref.vdf";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));
    let vdf_results = vdf_file
        .to_results_via_records()
        .unwrap_or_else(|e| panic!("VDF to_results_via_records failed: {e}"));

    (vdf_results, results)
}

/// Committed regression guard that `EXPECTED_VDF_RESIDUAL` stays EXACT: it is
/// the precise set of C-LEARN base variables that the live `classify_vdf_ident`
/// comparator flags, neither over- nor under-broad. Runs C-LEARN through the
/// same end-to-end path as `simulates_clearn`, classifies EVERY matched ident
/// through the SAME comparator (`classify_vdf_ident`) with NO exclusion, and
/// collects the base name of every ident with a failing cell -- a tolerance
/// violation or spurious `:NA:` (`stats.failures > 0`) or an entirely-NaN core
/// series (`stats.all_nan`, which would trip the comparator's NaN guard). That
/// is exactly the membership `EXPECTED_VDF_RESIDUAL` carves out (a partial-NaN
/// series is NOT all-NaN and is NOT excluded -- see `EXPECTED_VDF_RESIDUAL`
/// cluster 3). The global NaN-skipped-fraction guard is a separate aggregate
/// check enforced by `simulates_clearn` itself, not a per-base membership driver.
///
/// Asserts the live set EQUALS `EXPECTED_VDF_RESIDUAL`, failing LOUDLY with the
/// symmetric difference if the residual GREW (a regression -- new divergence to
/// investigate, file under #590/#591) or SHRANK (an engine fix -- prune the now-
/// passing base from the exclusion). Reuses `classify_vdf_ident` (no comparator
/// duplication). `#[ignore]`d for runtime class like `simulates_clearn`.
///
/// Run with:
/// cargo test --release -- --ignored clearn_residual_exactness
#[test]
#[ignore]
fn clearn_residual_exactness() {
    use std::collections::BTreeSet;

    let (vdf_expected, results) = run_clearn_vs_vdf();
    assert_eq!(vdf_expected.step_count, results.step_count);

    // Classify every matched ident with NO exclusion and collect the base name
    // of each ident that the comparator flags (failing cell or all-NaN core
    // series) -- exactly the membership EXPECTED_VDF_RESIDUAL must carve out.
    let mut live_residual: BTreeSet<String> = BTreeSet::new();
    let mut matched = 0usize;
    for ident in vdf_expected.offsets.keys() {
        let Some(&sim_off) = results.offsets.get(ident) else {
            continue;
        };
        matched += 1;
        let vdf_off = vdf_expected.offsets[ident];
        let stats = classify_vdf_ident(&vdf_expected, &results, vdf_off, sim_off);
        if stats.failures > 0 || stats.all_nan {
            live_residual.insert(vdf_ident_base_name(ident.as_str()).to_string());
        }
    }

    let expected_residual: BTreeSet<String> = EXPECTED_VDF_RESIDUAL
        .iter()
        .map(|s| s.to_string())
        .collect();

    let grew: Vec<&String> = live_residual.difference(&expected_residual).collect();
    let shrank: Vec<&String> = expected_residual.difference(&live_residual).collect();

    eprintln!(
        "clearn residual exactness: {matched} idents matched; {} live residual bases vs {} excluded",
        live_residual.len(),
        expected_residual.len()
    );
    assert!(
        grew.is_empty() && shrank.is_empty(),
        "EXPECTED_VDF_RESIDUAL is no longer exact.\n  \
         Newly-failing bases NOT in the exclusion (regression -- investigate, \
         track under #590/#591): {grew:?}\n  \
         Excluded bases that NO LONGER fail (a fix -- prune them from \
         EXPECTED_VDF_RESIDUAL): {shrank:?}"
    );
}

/// Issue #559 -- the C-LEARN `"Goal 1.5 for Temperature"` shape.
///
/// A Vensim quoted identifier containing a literal period (`"a.b"`,
/// `"goal 1.5"`) used to make the whole `main` model `NotSimulatable`:
/// `canonicalize()` was non-idempotent for quoted-period idents (first
/// pass kept a raw `.`, which `is_canonical()` rejects, so a re-canonical
/// pass mis-converted the literal period into the `·` module-hierarchy
/// separator -> the variable's own identity split into a phantom submodule
/// -> `DoesNotExist`). It tripped even for a *dead* constant referenced by
/// nobody, because it is the variable's own fragment identity that fails.
/// Run through the same incremental `compile_vm` path `simulates_clearn`
/// uses. Two shapes:
///   A. a quoted-period constant *referenced* by a live var, proving its
///      identity resolves AND is usable in equations (`y = "a.b" + 1`);
///   B. a *dead* quoted-period constant plus an unrelated live `z`,
///      proving such a constant no longer poisons compilation and the fix
///      changes no live output.
#[test]
fn simulates_quoted_period_ident() {
    // A: quoted-period constant referenced by a live variable.
    let mdl_a = "\
{UTF-8}
\"a.b\" = 5 ~~|
y = \"a.b\" + 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl_a);
    for step in 0..3 {
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - 6.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 6 (= \"a.b\"(5) + 1); the \
             quoted-period ident must resolve and be usable"
        );
    }

    // B: a DEAD quoted-period constant (the exact C-LEARN
    // `"Goal 1.5 for Temperature"` shape) plus an unrelated live `z`.
    // Before the fix this alone made the model NotSimulatable.
    let mdl_b = "\
{UTF-8}
\"goal 1.5\" = 1.5 ~~|
z = 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl_b);
    for step in 0..3 {
        let z = macro_test_value_at(&results, "z", step);
        assert!(
            (z - 1.0).abs() < 1e-9,
            "step {step}: z = {z}, expected 1; a dead quoted-period \
             constant must not make the model NotSimulatable"
        );
    }
}

/// Read a `[t1]/[t2]/[t3]`-style element series out of a `Results`.
fn element_series(results: &Results, name: &str) -> Vec<f64> {
    let id = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
    let off = *results.offsets.get(&id).unwrap_or_else(|| {
        panic!(
            "{name:?} absent; have {:?}",
            results.offsets.keys().collect::<Vec<_>>()
        )
    });
    (0..results.step_count)
        .map(|s| results.iter().nth(s).unwrap()[off])
        .collect()
}

/// Compile an inline/loaded Vensim `.mdl` through the incremental path and
/// return `(compile_is_err, diagnostics)` WITHOUT panicking -- for fixtures
/// that are intentionally NOT simulatable (cycle/recurrence repros), where
/// we assert the *diagnostic code*, not a simulation result.
fn compile_diags(mdl: &str) -> (bool, Vec<simlin_engine::db::Diagnostic>) {
    let dm = open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse mdl: {e}"));
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
    let is_err = compile_project_incremental(&db, sync.project, "main").is_err();
    (is_err, collect_project_diagnostics(&dm))
}

fn diag_code(d: &simlin_engine::db::Diagnostic) -> Option<simlin_engine::common::ErrorCode> {
    use simlin_engine::db::DiagnosticError;
    match &d.error {
        DiagnosticError::Equation(e) => Some(e.code),
        DiagnosticError::Model(e) => Some(e.code),
        _ => None,
    }
}

/// Issue #559 + element-level cycle resolution (Phase 1) -- the C-LEARN
/// `Emissions with cumulative constraints[COP,tNext] = ... ecc[COP,tPrev]`
/// self-recurrence shape, minimised to a single self-recurrent variable
/// with NO cross-variable cycle (`test/sdeverywhere/models/self_recurrence`):
/// `ecc[t1]=1; ecc[tNext]=ecc[tPrev]+1` over the subrange `t1..t3`.
///
/// Two intents, both verified here:
///
/// (#559 name-resolution guard) The native MDL converter
/// (`xmile_compat.rs::format_var_ctx`) rewrote a self-reference
/// (`name == ctx.lhs_var_canonical`) to the literal token `self`; for a
/// *subscripted* self-reference it emitted `self[..]`. The engine's
/// `builtins_visitor` Subscript arm never resolves `self`, so the literal
/// token leaked into dependency analysis as an undefined name ->
/// `UnknownDependency`. The fix emits the real canonical LHS name instead,
/// making the self-reference an ordinary reference; the `self` token must
/// never leak as `UnknownDependency`/`DoesNotExist` again.
///
/// (element-cycle-resolution.AC1.1/AC1.2 target behavior) Once `self`
/// resolves, `ecc[tNext]=ecc[tPrev]+1` is a whole-variable self-edge whose
/// *induced element graph* (`ecc[t2]<-ecc[t1]`, `ecc[t3]<-ecc[t2]`) is
/// acyclic and well-founded. The element-level cycle refinement resolves
/// it: the model compiles via the incremental path with NO
/// `CircularDependency` and simulates to the deterministic staggered
/// series `ecc[t1]=1, ecc[t2]=2, ecc[t3]=3` (constant across both saved
/// steps -- the recurrence is over the subrange, not over time). FINAL
/// TIME=1, TIME STEP=1 => 2 saved steps. `self_recurrence/` ships no
/// `.dat`, so the series is asserted in-test via `element_series`.
#[test]
fn self_recurrence_resolves_and_no_self_token_leak() {
    use simlin_engine::common::ErrorCode;

    let mdl = std::fs::read_to_string(
        "../../test/sdeverywhere/models/self_recurrence/self_recurrence.mdl",
    )
    .expect("self_recurrence.mdl fixture must exist");
    let (compile_err, diags) = compile_diags(&mdl);

    // `self_recurrence` has exactly one non-control variable (`ecc`) and
    // self-contained dims, so the ONLY possible unknown is the leaked
    // `self` token.
    let self_leak = diags.iter().any(|d| {
        matches!(
            diag_code(d),
            Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
        )
    });
    let has_circular = diags
        .iter()
        .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency));

    // (AC1.1) The induced element graph is acyclic, so the model now
    // compiles via the incremental path with no CircularDependency (this
    // inverts the pre-resolution assertions: it used to be NotSimulatable
    // with a whole-variable self-edge CircularDependency).
    assert!(
        !compile_err,
        "self_recurrence.mdl must now compile via the incremental path: \
         its single-variable self-recurrence has an acyclic induced \
         element graph and is resolved by the element-cycle refinement. \
         Diagnostics: {diags:#?}"
    );
    assert!(
        !has_circular,
        "the single-variable self-recurrence must NOT report \
         CircularDependency once the element-cycle refinement resolves it \
         (its induced element graph is acyclic). Diagnostics: {diags:#?}"
    );
    // (#559 guard, preserved) The `self` token resolves to the enclosing
    // var, so it never leaks as an unknown dependency. (Before the #559
    // fix this assertion failed: `self[tPrev]` leaked as
    // UnknownDependency because the converter emitted the literal `self`
    // and the builtins_visitor Subscript arm never resolved it.)
    assert!(
        !self_leak,
        "the literal `self` token still leaks as \
         UnknownDependency/DoesNotExist. The converter must emit the real \
         canonical LHS name, not `self`. Diagnostics: {diags:#?}"
    );

    // (AC1.2) It simulates to the well-founded staggered series
    // ecc[t1]=1, ecc[t2]=2, ecc[t3]=3 -- constant across both saved steps
    // (the recurrence is over the subrange, not over time).
    let r = run_inline_mdl(&mdl);
    assert_eq!(r.step_count, 2);
    assert_eq!(element_series(&r, "ecc[t1]"), vec![1.0, 1.0]);
    assert_eq!(element_series(&r, "ecc[t2]"), vec![2.0, 2.0]);
    assert_eq!(element_series(&r, "ecc[t3]"), vec![3.0, 3.0]);
}

/// Genuine-cycle guards: the self-reference fix must NOT weaken real
/// cycle detection. These two models have NO well-founded staggered
/// recurrence -- they are genuine cycles and MUST stay rejected (a
/// CLAUDE.md hard rule).
#[test]
fn genuine_cycles_still_rejected() {
    use simlin_engine::common::ErrorCode;

    // (1) Scalar 2-cycle `a=b+1; b=a+1`: a true algebraic loop, no `self`
    // token involved at all -> `CircularDependency`, stable before and
    // after the fix.
    let two_cycle = "\
{UTF-8}
a = b + 1 ~~|
b = a + 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err1, diags1) = compile_diags(two_cycle);
    assert!(err1, "scalar 2-cycle a=b+1;b=a+1 must be NotSimulatable");
    assert!(
        diags1
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
        "scalar 2-cycle must report CircularDependency (genuine cycle \
         detection must not be weakened). Diagnostics: {diags1:#?}"
    );
    assert!(
        !diags1.iter().any(|d| matches!(
            diag_code(d),
            Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
        )),
        "scalar 2-cycle has no undefined names; only CircularDependency \
         is expected. Diagnostics: {diags1:#?}"
    );

    // (2) Genuine SAME-element self `x[dimA]=x[dimA]+1` (NOT a shifted
    // subrange): a real same-element self-cycle that MUST stay rejected.
    // element-cycle-resolution.AC4.2: element-level cycle resolution
    // resolves the `self` token to the real name (`x`), so every element
    // reads *itself* (`x[a1]<-x[a1]`, `x[a2]<-x[a2]`). The induced element
    // graph therefore has an element self-loop -- it is element-cyclic, a
    // genuine cycle -- so the engine MUST report `CircularDependency`
    // SPECIFICALLY. This is no longer an `UnknownDependency` leak: that
    // only happened before the #559 fix, when the literal `self` token
    // leaked as an undefined name. Pin `CircularDependency` exactly (the
    // assertion below also forbids any `UnknownDependency`/`DoesNotExist`,
    // which would now be a name-resolution regression, not the expected
    // verdict).
    let same_elem = "\
{UTF-8}
dimA: a1, a2 ~~|
x[dimA] = x[dimA] + 1 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err2, diags2) = compile_diags(same_elem);
    assert!(
        err2,
        "genuine same-element self x[dimA]=x[dimA]+1 must stay \
         NotSimulatable (CLAUDE.md hard rule -- a real cycle)"
    );
    assert!(
        diags2
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
        "genuine same-element self x[dimA]=x[dimA]+1 must report \
         CircularDependency SPECIFICALLY (element-cycle-resolution.AC4.2): \
         every element reads itself, so the induced element graph has an \
         element self-loop and the element-cycle refinement keeps the \
         conservative CircularDependency. Diagnostics: {diags2:#?}"
    );
    assert!(
        !diags2.iter().any(|d| matches!(
            diag_code(d),
            Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
        )),
        "the `self` token must resolve to the real name `x`; a leaked \
         UnknownDependency/DoesNotExist here is a #559 name-resolution \
         regression, not the expected same-element-self-cycle verdict. \
         Diagnostics: {diags2:#?}"
    );
}

/// Regression guard for the one self-context that must keep working: a
/// self-reference INSIDE `PREVIOUS()`. The #559 fix stops emitting the
/// literal `self` in `xmile_compat.rs::format_var_ctx`; this proves it
/// does not perturb the self-in-PREVIOUS path.
///
/// Primary case is the *shifted subrange* form
/// `x[t1]=1; x[tNext]=PREVIOUS(x[tPrev],0)`. Before the fix this failed
/// to compile -- the synthetic PREVIOUS-helper fragment was built around
/// the unresolved `self[tPrev]`; after the fix the real name `x[tPrev]`
/// resolves, the helper compiles, and it simulates the well-defined
/// PREVIOUS-shift recurrence. The scalar `x=PREVIOUS(x,0)` sub-case is
/// the simplest stable form.
#[test]
fn previous_self_reference_still_resolves() {
    // Sub-case A: scalar self-in-PREVIOUS (the simplest form).
    let scalar = "\
{UTF-8}
x = PREVIOUS(x, 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err_s, diags_s) = compile_diags(scalar);
    assert!(
        !err_s,
        "scalar x=PREVIOUS(x,0) must stay simulatable after the fix. \
         Diagnostics: {diags_s:#?}"
    );
    assert_eq!(run_inline_mdl(scalar).step_count, 3);

    // Sub-case B (primary): shifted-subrange self-in-PREVIOUS. Must
    // compile AND simulate the exact PREVIOUS-shift recurrence after the
    // fix.
    let shifted = "\
{UTF-8}
Target: (t1-t3)
\t~
\t~\t\t|

tNext: (t2-t3) -> tPrev
\t~
\t~\t\t|

tPrev: (t1-t2) -> tNext
\t~
\t~\t\t|
x[t1] = 1 ~~|

x[tNext] = PREVIOUS(x[tPrev], 0) ~~|

INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err_b, diags_b) = compile_diags(shifted);
    assert!(
        !err_b,
        "shifted-subrange PREVIOUS self-ref must COMPILE after the fix \
         (resolving `self[tPrev]` -> `x[tPrev]` fixes the synthetic \
         PREVIOUS-helper that failed before). Diagnostics: {diags_b:#?}"
    );
    let r = run_inline_mdl(shifted);
    assert_eq!(r.step_count, 3);
    // x[t1]=1 (base, constant); x[t2]=PREVIOUS(x[t1],0); x[t3]=PREVIOUS(x[t2],0).
    // PREVIOUS = init at t0 else prior-DT value -> the deterministic
    // staggered-recurrence series:
    assert_eq!(element_series(&r, "x[t1]"), vec![1.0, 1.0, 1.0]);
    assert_eq!(element_series(&r, "x[t2]"), vec![0.0, 1.0, 1.0]);
    assert_eq!(element_series(&r, "x[t3]"), vec![0.0, 0.0, 1.0]);
}

/// `SAMPLE IF TRUE` expands in the converter to
/// `( IF cond THEN input ELSE PREVIOUS(SELF, init) )` via a hard-coded
/// literal `SELF` in the `"sample if true"` arm -- a *different* site
/// from the `format_var_ctx` self-reference rewrite the #559 fix changes.
/// The fix must NOT perturb this: a real `SAMPLE IF TRUE(...)` over a
/// subrange must still compile AND simulate correctly, proving the two
/// `self` sites are independent.
#[test]
fn sample_if_true_subrange_self_template_unaffected() {
    let mdl = "\
{UTF-8}
Target: (t1-t3)
\t~
\t~\t\t|

tNext: (t2-t3) -> tPrev
\t~
\t~\t\t|

tPrev: (t1-t2) -> tNext
\t~
\t~\t\t|
k = 5 ~~|

a = 7 ~~|

c = 9 ~~|

y[t1] = SAMPLE IF TRUE(Time <= k, a, c) ~~|

y[tNext] = SAMPLE IF TRUE(Time <= k, a, c) ~~|

INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (err, diags) = compile_diags(mdl);
    assert!(
        !err,
        "real SAMPLE IF TRUE over a subrange must compile after the fix \
         (the fix must not touch the hard-coded PREVIOUS(SELF,init) \
         template). Diagnostics: {diags:#?}"
    );
    let r = run_inline_mdl(mdl);
    assert_eq!(r.step_count, 3);
    // Time=0,1,2 are all <= k(5) -> condition always true -> output is the
    // sampled input a(7) at every element/step (the SELF/previous branch
    // is never taken; this exercises the template's structure intact).
    assert_eq!(element_series(&r, "y[t1]"), vec![7.0, 7.0, 7.0]);
    assert_eq!(element_series(&r, "y[t2]"), vec![7.0, 7.0, 7.0]);
    assert_eq!(element_series(&r, "y[t3]"), vec![7.0, 7.0, 7.0]);
}

/// element-cycle-resolution.AC2.1 (Phase 2 Task 7). `ref.mdl` is a
/// two-variable INTER-element recurrence: `ce[t1]=1;
/// ce[tNext]=ecc[tPrev]+1; ecc[t1]=ce[t1]+1; ecc[tNext]=ce[tNext]+1` over
/// the subrange `t1..t3`. Whole-variable `ce`<->`ecc` is a 2-cycle, but
/// the induced element graph
///   (ce,0)->(ecc,0); (ce,1)->(ecc,1); (ce,2)->(ecc,2);
///   (ecc,0)->(ce,1); (ecc,1)->(ce,2)
/// is acyclic. Before Subcomponent B (the GH #575 symbolic-ref
/// re-architecture: Task 4 verdict, Task 5 combined fragment, Task 5b
/// SCC-aware back-edge break, Task 6 injection) this multi-member SCC was
/// short-circuited to `CircularDependency` and the model did not compile.
/// Subcomponent B resolves the `{ce,ecc}` SCC, interleaves the members'
/// per-element segments in topological order, and injects one combined
/// fragment, so `ref.mdl` now compiles AND simulates to the hand-computed
/// per-element series shipped in the sibling `ref.dat`:
///   ce[t1]=1, ce[t2]=3, ce[t3]=5, ecc[t1]=2, ecc[t2]=4, ecc[t3]=6
/// (constant across both saved steps -- the recurrence is over the
/// subrange, not over time; FINAL TIME=1, TIME STEP=1 => 2 saved steps).
/// `simulate_mdl_path` loads `ref.dat` via `load_expected_results_for_mdl`
/// and compares with `ensure_results`, so this test IS AC2.1: it failed
/// before Subcomponent B (rejected as `CircularDependency`) and passes
/// now against `ref.dat`.
#[test]
fn ref_mdl_multi_variable_recurrence_simulates() {
    simulate_mdl_path("../../test/sdeverywhere/models/ref/ref.mdl");
}

/// element-cycle-resolution.AC2.2 (Phase 2 Task 8). `interleaved.mdl`:
/// `x=1; a[A1]=x; a[A2]=y; y=a[A1]; b[DimA]=a[DimA]` (`DimA: A1,A2`).
/// Whole-variable `a`<->`y` is a 2-cycle, but element-wise
/// `x -> a[A1] -> y -> a[A2]` is acyclic. Subcomponent B (GH #575)
/// resolves the multi-member `{a,y}` SCC and evaluates its members in
/// interleaved per-element order; before Subcomponent B this was rejected
/// as `CircularDependency` and the model did not compile. Every variable
/// resolves to `1.0`, so the sibling `interleaved.dat` is all `1.0`
/// across its 101 saved steps (INITIAL TIME=0, FINAL TIME=100, TIME
/// STEP=1 => 101 steps). `simulate_mdl_path` loads `interleaved.dat` via
/// `load_expected_results_for_mdl` and compares the VM output with
/// `ensure_results`, so this test IS AC2.2: it failed before
/// Subcomponent B and passes now.
#[test]
fn interleaved_mdl_element_interleave_simulates() {
    simulate_mdl_path("../../test/sdeverywhere/models/interleaved/interleaved.mdl");
}

/// element-cycle-resolution.AC2.4 (Phase 2 Task 9, the init-phase proof).
/// `init_recurrence.mdl` is a NEW fixture exercising the init-phase
/// combined-fragment path with a MULTI-member init SCC -- the first real
/// exercise of Task 6's synthetic-ident `SymbolicCompiledInitial` init
/// injection (`$⁚scc⁚init⁚{n}`) on a multi-member SCC. (Subcomponent A's
/// `init_recurrence_behind_stock_*` tests already cover the SINGLE-
/// variable init-recurrence-behind-a-stock case; AC2.4's combined-
/// fragment init path needs a MULTI-member init SCC.)
///
/// Shape: two arrayed stocks `cs[Target]` / `ecs[Target]` over the
/// subrange `t1..t3`, whose per-element INTEG **initial values** form a
/// `ref.mdl`-shaped inter-element recurrence ACROSS the two variables
/// (`cs[t1]=1; cs[tNext]=ecs[tPrev]+1; ecs[t1]=cs[t1]+1;
/// ecs[tNext]=cs[tNext]+1`), with a constant zero inflow `g[Target]=0`.
/// Each stock's dt-equation is its (acyclic) flow `g`, so the STOCK
/// BREAKS the dt chain -- there is NO dt cycle. The INIT relation,
/// however, has `cs`'s init referencing `ecs` and vice-versa: a
/// whole-variable init 2-cycle `{cs,ecs}` whose induced per-element INIT
/// graph is acyclic. So ONLY the init element graph exercises the
/// combined INIT fragment.
///
/// This test FIRST empirically confirms (per the AC2.4 mandate) that the
/// parsed fixture produces an init-ONLY MULTI-member element SCC -- the
/// production `model_dependency_graph` payload must carry exactly one
/// `ResolvedScc { phase: Initial }` with `>= 2` members (`{cs,ecs}`) and
/// `has_cycle == false` -- then simulates it via `simulate_mdl_path`
/// against the hand-computed `init_recurrence.dat`. The stocks integrate
/// a zero flow, so they hold their recurrence-computed initial values
/// constant across both saved steps (FINAL TIME=1, TIME STEP=1):
///   cs[t1]=1, cs[t2]=3, cs[t3]=5, ecs[t1]=2, ecs[t2]=4, ecs[t3]=6.
#[test]
fn init_recurrence_mdl_multi_member_init_scc_simulates() {
    use simlin_engine::common::Ident;
    use simlin_engine::db::{SccPhase, model_dependency_graph};
    use std::collections::BTreeSet;

    let path = "../../test/sdeverywhere/models/init_recurrence/init_recurrence.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // Empirical AC2.4 confirmation, tied to the REAL fixture: the parsed
    // MDL must yield an init-ONLY MULTI-member element SCC. If this
    // produced a 1-member or a dt-phase SCC the fixture would not
    // exercise the init combined-fragment path at all.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
    let sync = sync.to_sync_result();
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    assert!(
        !dep_graph.has_cycle,
        "init_recurrence.mdl: the element-acyclic MULTI-member init-only \
         recurrence behind stocks must NOT set has_cycle (resolved_sccs = \
         {:?})",
        dep_graph.resolved_sccs
    );
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "init_recurrence.mdl: exactly one ResolvedScc (the {{cs,ecs}} \
         init cluster) -- got {:?}",
        dep_graph.resolved_sccs
    );
    let scc = &dep_graph.resolved_sccs[0];
    assert_eq!(
        scc.phase,
        SccPhase::Initial,
        "init_recurrence.mdl: the resolved SCC MUST be phase == Initial \
         (a dt-phase SCC would mean the stock did not break the dt chain \
         and the fixture would not exercise the init combined-fragment \
         path) -- got {:?}",
        scc.phase
    );
    assert!(
        scc.members.len() >= 2,
        "init_recurrence.mdl: AC2.4 requires a MULTI-member init SCC (the \
         single-variable case is already covered by Subcomponent A); got \
         {} member(s): {:?}",
        scc.members.len(),
        scc.members
    );
    assert_eq!(
        scc.members,
        [Ident::new("cs"), Ident::new("ecs")]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "init_recurrence.mdl: the resolved init SCC's members must be \
         exactly {{cs,ecs}}"
    );

    // End-to-end: it compiles AND simulates to the hand-computed
    // init_recurrence.dat (the combined INIT fragment, injected as one
    // synthetic-ident SymbolicCompiledInitial, produces the correct
    // per-element initial values; the zero flow holds them constant).
    simulate_mdl_path(path);
}

/// element-cycle-resolution.AC3.1 (Phase 3 Task 3 -- the parent-sourcing
/// happy-path end-to-end proof). `helper_recurrence.mdl` is a NEW fixture:
/// a shift-mapped subrange recurrence over `Target: t1..t3` (the
/// `self_recurrence.mdl` `Target/tNext/tPrev` shape) whose per-element
/// recurrence body invokes a synthetic helper:
///
///   ecc[t1]     = 1
///   ecc[tNext]  = INITIAL(ecc[tPrev] * 2)
///
/// `INITIAL(<expr>)` (Vensim's spelling of XMILE `INIT`) over the shifted
/// subrange expands element-wise to `ecc[t2] = INITIAL(ecc[t1] * 2)` and
/// `ecc[t3] = INITIAL(ecc[t2] * 2)`. Because the `INITIAL` argument is an
/// *expression* (not a bare scalar slot), `builtins_visitor::make_temp_arg`
/// synthesizes a scalar helper aux per recurrence element -- the canonical
/// `$\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t2` /
/// `$\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t3` form (design deviation 2):
/// absent from `model.variables`, present in `model_implicit_var_info`
/// (its parent is `ecc`). Each helper's argument references `ecc`, and
/// `ecc`'s init references the helper (`ecc[tNext] = INITIAL($helper)`),
/// so the helpers land *inside* a MULTI-member init-phase recurrence SCC
/// `{ecc, $\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t2,
/// $\u{205A}ecc\u{205A}0\u{205A}arg0\u{205A}t3}` whose induced per-element
/// INIT graph is acyclic and well-founded.
///
/// This is the deliberately-constructed AC3.1 coverage: the design notes
/// AC3.1 is the *only* coverage of the parent-`implicit_vars` sourcing
/// happy path, so the fixture shape was empirically verified (per design
/// deviation 4) to genuinely push a `$\u{205A}`-prefixed helper into a
/// *resolved* SCC -- this test re-asserts that here so a future converter
/// change that stopped synthesizing the in-SCC helper would fail loudly
/// rather than silently degrade AC3.1 to a no-op.
///
/// RED (structural, no destructive git): the synthetic helpers have NO
/// `SourceVariable` (they are absent from `model.variables`). Without
/// Phase 3 Task 2's parent-`implicit_vars` sourcing,
/// `var_phase_symbolic_fragment_prod`'s no-`SourceVariable` arm returns
/// `None` for each helper (the Task 1 loud-safe contract), so
/// `symbolic_phase_element_order`'s per-member `?` short-circuits the
/// whole builder to `None`, the `{ecc, $helper..}` SCC is `Unresolved`,
/// `has_cycle` stays `true`, and the model is rejected with
/// `CircularDependency` -- exactly the verdict the committed Task 1 test
/// `unsourceable_in_scc_node_falls_back_to_circular_no_panic` pins for an
/// unsourceable in-SCC node, and the verdict every non-helper-resolving
/// candidate shape produced while this fixture's shape was being
/// empirically selected. GREEN after Task 2 (committed `cee5d063`): the
/// helper's symbolic `PerVarBytecodes` is parent-sourced from `ecc`'s
/// `implicit_vars`, the SCC resolves, `has_cycle == false`, and it
/// simulates.
///
/// The test FIRST empirically confirms (tied to the REAL parsed fixture,
/// per the AC3.1 mandate) that the dependency graph carries exactly one
/// `ResolvedScc { phase: Initial }` whose members include a
/// `$\u{205A}`-prefixed synthetic helper that is genuinely
/// no-`SourceVariable` (absent from `model.variables`) yet resolves in
/// `model_implicit_var_info` -- i.e. the fixture genuinely exercises the
/// Task 2 parent-sourcing path and not merely an ordinary recurrence --
/// then simulates it via `simulate_mdl_path` against the hand-computed
/// `helper_recurrence.dat`. The recurrence has no time dependence
/// (`INITIAL` snapshots t=0; no stocks/PREVIOUS lag), so the values are
/// constant across both saved steps (INITIAL TIME=0, FINAL TIME=1, TIME
/// STEP=1 => 2 steps): ecc[t1]=1, ecc[t2]=INITIAL(ecc[t1]*2)=2,
/// ecc[t3]=INITIAL(ecc[t2]*2)=4.
#[test]
fn helper_recurrence_mdl_synthetic_helper_in_scc_simulates() {
    use simlin_engine::common::ErrorCode;
    use simlin_engine::db::{SccPhase, model_dependency_graph, model_implicit_var_info};

    let path = "../../test/sdeverywhere/models/helper_recurrence/helper_recurrence.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &dm, None);
    let sync = sync.to_sync_result();
    let model = sync.models["main"].source;
    let dep_graph = model_dependency_graph(&db, model, sync.project);

    // (1) It compiles via the incremental path -- NO false
    // `CircularDependency`. RED before Task 2: the no-`SourceVariable`
    // helper is unsourceable -> SCC `Unresolved` -> `has_cycle` true ->
    // `CircularDependency`.
    assert!(
        !dep_graph.has_cycle,
        "helper_recurrence.mdl: the helper-bearing recurrence must NOT set \
         has_cycle once the synthetic helper is parent-sourced (AC3.1); \
         resolved_sccs = {:?}",
        dep_graph.resolved_sccs
    );
    let diags = collect_project_diagnostics(&dm);
    assert!(
        !diags
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
        "helper_recurrence.mdl: must NOT report CircularDependency once \
         the synthetic helper is sourced from its parent's implicit_vars \
         (AC3.1). Diagnostics: {diags:#?}"
    );

    // (2) Exactly one resolved SCC, init-phase (the `INITIAL()`-driven
    // recurrence is an init-relation cycle, not a dt one).
    assert_eq!(
        dep_graph.resolved_sccs.len(),
        1,
        "helper_recurrence.mdl: exactly one ResolvedScc (the \
         {{ecc, $helper..}} init cluster) -- got {:?}",
        dep_graph.resolved_sccs
    );
    let scc = &dep_graph.resolved_sccs[0];
    assert_eq!(
        scc.phase,
        SccPhase::Initial,
        "helper_recurrence.mdl: the resolved SCC MUST be phase == Initial \
         (the recurrence is driven by INITIAL(), an init-phase relation) \
         -- got {:?}",
        scc.phase
    );
    assert!(
        scc.members.len() >= 2,
        "helper_recurrence.mdl: AC3.1 requires the helper to be IN a \
         MULTI-member SCC alongside `ecc` (a 1-member SCC would mean the \
         helper is a mere forward dependency, not exercising the \
         in-SCC parent-sourcing path); got {} member(s): {:?}",
        scc.members.len(),
        scc.members
    );

    // (3) The CORE AC3.1 / Task 2 assertion: the resolved SCC genuinely
    // contains a `$\u{205A}`-prefixed synthetic-helper member that is
    // no-`SourceVariable` (absent from `model.variables`) yet resolves in
    // `model_implicit_var_info`. This is what makes the fixture exercise
    // the Task 2 parent-`implicit_vars` sourcing path rather than an
    // ordinary recurrence -- and is exactly the node whose symbolic
    // fragment is `None` (=> SCC `Unresolved` => `CircularDependency`)
    // without Task 2 (RED), `Some` (parent-sourced => SCC resolved) with
    // it (GREEN).
    let info = model_implicit_var_info(&db, model, sync.project);
    let source_vars = model.variables(&db);
    let in_scc_helpers: Vec<&str> = scc
        .members
        .iter()
        .map(|m| m.as_str())
        .filter(|m| {
            m.starts_with('$')
                && m.contains('\u{205A}')
                && source_vars.get(*m).is_none()
                && info.contains_key(*m)
        })
        .collect();
    assert!(
        !in_scc_helpers.is_empty(),
        "helper_recurrence.mdl: the resolved SCC MUST contain at least one \
         `$\u{205A}`-prefixed synthetic helper that has NO SourceVariable \
         (absent from model.variables) yet resolves in \
         model_implicit_var_info -- this is the node Phase 3 Task 2 \
         parent-sources; without it the fixture would not exercise the \
         AC3.1 happy path at all. SCC members: {:?}; implicit_var_info \
         keys: {:?}",
        scc.members,
        info.keys().collect::<Vec<_>>()
    );
    // Pin the parent: every in-SCC helper's `model_implicit_var_info`
    // entry must name `ecc` as its parent_source_var (the variable whose
    // `implicit_vars` Task 2 reaches), so the parent-sourcing chain the
    // test exercises is the intended one.
    for helper in &in_scc_helpers {
        let meta = &info[*helper];
        let parent = meta.parent_source_var.ident(&db);
        assert_eq!(
            parent.as_str(),
            "ecc",
            "in-SCC helper {helper:?} must be parented to `ecc` (the \
             recurrence variable whose implicit_vars Task 2 sources); \
             got parent {parent:?}"
        );
    }

    // (4) End-to-end: it compiles AND simulates to the hand-computed
    // helper_recurrence.dat. The synthetic helpers' symbolic
    // `PerVarBytecodes`, parent-sourced from `ecc`'s `implicit_vars`
    // (Task 2), are interleaved into the combined SCC fragment exactly
    // like a real member, producing the well-founded series
    // ecc[t1]=1, ecc[t2]=2, ecc[t3]=4 held constant across both saved
    // steps.
    simulate_mdl_path(path);
}

/// clearn-residual.AC3.2: an element-wise `INITIAL` recurrence routed through a
/// trivial passthrough macro (`:MACRO: INIT(x) = INITIAL(x)`) produces the same
/// correct per-element values as the proven bare-`INITIAL` opcode path
/// (`helper_recurrence_mdl_synthetic_helper_in_scc_simulates` above).
///
/// The `macro_init_recurrence` fixture is `helper_recurrence.mdl` with the
/// `:MACRO: INIT(x) = INITIAL(x)` block PREPENDED. The recurrence text
/// (`ecc[tNext] = INITIAL(ecc[tPrev] * 2)`) is unchanged: its `INITIAL(...)` is
/// renamed to `INIT(...)` at import and now collides with the macro, so the
/// recurrence routes through the macro instead of the `LoadInitial` opcode.
///
/// Expected (identical to `helper_recurrence`): `ecc[t1]=1, ecc[t2]=2,
/// ecc[t3]=4`, constant across both saved steps (INITIAL TIME=0, FINAL TIME=1,
/// TIME STEP=1 => 2 steps). `simulate_mdl_path` compares against the sibling
/// `macro_init_recurrence.dat`, which asserts the user-facing `ecc[..]` series
/// (`ensure_results` skips the implicit `$\u{205A}` module vars).
///
/// RED before the Phase 3 Task 4 call-site collapse: the INIT-macro collision
/// routes the recurrence through the buggy per-element synthetic module, so the
/// recurrence drops to 0/`:NA:` at t>=1 rather than holding 1/2/4.
#[test]
fn simulates_passthrough_init_macro_element_recurrence() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_init_recurrence/macro_init_recurrence.mdl",
    );
}

/// element-cycle-resolution.AC2.5 (Phase 2 Task 9, the transitioned
/// inter-variable-cycle assertion). `ref.mdl` and `interleaved.mdl` are
/// INTER-variable element-acyclic recurrence SCCs (`ce`<->`ecc` /
/// `a`<->`y`). Before Subcomponent B they were rejected with
/// `CircularDependency`; this test originally asserted exactly that.
///
/// Formerly named `ref_interleaved_inter_variable_cycles_report_circular`
/// (the Phase 1 form that asserted `CircularDependency`); this is the
/// AC2.5-transitioned successor of that test, renamed in Phase 2 Task 9.
/// The old name is recorded here so `git blame`/`git log -S` and a plain
/// grep on either name stay continuous across the rename.
/// Subcomponent B (GH #575) resolves the multi-member SCC and injects one
/// combined per-element fragment, so the fixtures now compile and
/// simulate.
///
/// Per AC2.5 this test is **transitioned** to its final end state: the
/// correct-simulation assertion is folded into the dedicated end-to-end
/// tests `ref_mdl_multi_variable_recurrence_simulates` (AC2.1, vs.
/// `ref.dat`) and `interleaved_mdl_element_interleave_simulates` (AC2.2,
/// vs. `interleaved.dat`) above -- those `simulate_mdl_path` tests ARE
/// the hand-computed-value proof, so re-asserting the series here would
/// be redundant. This test retains the still-meaningful diagnostic-level
/// guards the dedicated value tests do not pin:
///
///  1. The cycle-gate verdict transitioned correctly: NO
///     `CircularDependency` for either fixture, and the model compiles
///     end-to-end (the multi-member resolved SCC survives the dependency
///     graph -- the exact inverse of the original pre-Subcomponent-B
///     `CircularDependency` assertion this test made).
///  2. The AC2.5 leak guard, preserved verbatim from the original Phase 1
///     intent (the #559-class regression pin): the verdict change must
///     NOT spuriously inject a `self`/undefined-name
///     `UnknownDependency`/`DoesNotExist` leak into these
///     inter-variable-cycle fixtures. `ensure_results` in the value tests
///     would catch a wrong *number*, but only this guard pins the
///     specific "the cycle resolution leaked an undefined dependency
///     name" failure mode.
#[test]
fn ref_interleaved_inter_variable_cycles_simulate_no_circular_no_leak() {
    use simlin_engine::common::ErrorCode;
    for path in [
        "../../test/sdeverywhere/models/ref/ref.mdl",
        "../../test/sdeverywhere/models/interleaved/interleaved.mdl",
    ] {
        let mdl =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("missing fixture {path}: {e}"));
        let (compile_err, diags) = compile_diags(&mdl);
        assert!(
            !compile_err,
            "{path}: Subcomponent B (GH #575) must let the element-acyclic \
             multi-member recurrence SCC compile end-to-end (correct \
             simulation is asserted by the dedicated `.dat` tests above). \
             Diagnostics: {diags:#?}"
        );
        assert!(
            !diags
                .iter()
                .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency)),
            "{path}: the element-acyclic multi-member recurrence SCC must \
             survive the dependency graph -- it must NO LONGER report \
             CircularDependency (transitioned from the original \
             pre-Subcomponent-B assertion). Diagnostics: {diags:#?}"
        );
        assert!(
            !diags.iter().any(|d| matches!(
                diag_code(d),
                Some(ErrorCode::UnknownDependency) | Some(ErrorCode::DoesNotExist)
            )),
            "{path}: the cycle-gate verdict change must NOT inject a \
             `self`/undefined-name leak into a pure inter-variable-cycle \
             fixture (AC2.5 leak guard, preserved). Diagnostics: {diags:#?}"
        );
    }
}

/// Issue #559 (end-to-end). A 1D stock whose INTEG rate references a
/// higher-rank flow pinned to one element of the extra dimension
/// (`stock1d[scenario] = INTEG(fluxatm[scenario] - flux2d[scenario, L1], 0)`,
/// `flux2d[scenario, layers]`). The native MDL `collect_flows` dropped the
/// `[scenario, L1]` pin and wired the bare 2D `flux2d` as a named outflow
/// of the 1D stock, so the dimension checker reported
/// `mismatched_dimensions` on `stock1d` (e.g. C-LEARN's
/// `c_in_mixed_layer` / `heat_in_atmosphere_and_upper_ocean`).
///
/// Before the fix `compile_project_incremental` failed with a
/// `MismatchedDimensions` diagnostic on `stock1d`. After the fix the
/// rank-changing subscripted reference falls through to the synthetic
/// net-flow path (which preserves the 1D `flux2d[scenario, L1]` slice in
/// the rate), so the model compiles and simulates. `fluxatm`(2) -
/// `flux2d[*,L1]`(1) = 1 per step into `stock1d` (init 0): [0, 1].
#[test]
fn simulates_subscript_pinned_higher_rank_flow() {
    use simlin_engine::common::ErrorCode;
    let mdl = "\
{UTF-8}
scenario: s1, s2 ~~|
layers: (L1-L4) ~~|
flux2d[scenario, layers] = 1 ~~|
fluxatm[scenario] = 2 ~~|
stock1d[scenario] = INTEG(fluxatm[scenario] - flux2d[scenario, L1], 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (compile_err, diags) = compile_diags(mdl);
    // The bug surfaces specifically as MismatchedDimensions on the 1D
    // stock fed by the mis-wired bare 2D flow.
    assert!(
        !diags
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::MismatchedDimensions)),
        "a subscript-pinned higher-rank flow must not produce \
         MismatchedDimensions on the 1D stock. Diagnostics: {diags:#?}"
    );
    assert!(
        !compile_err,
        "stock1d with a pinned-slice flow must compile (synthetic \
         net-flow preserves the 1D `flux2d[scenario, L1]` slice). \
         Diagnostics: {diags:#?}"
    );
    // It simulates: stock1d[s] = INTEG(fluxatm(2) - flux2d[s,L1](1)) per
    // scenario element, init 0 -> [0, 1] over the 2 steps.
    let r = run_inline_mdl(mdl);
    assert_eq!(r.step_count, 2);
    assert_eq!(element_series(&r, "stock1d[s1]"), vec![0.0, 1.0]);
    assert_eq!(element_series(&r, "stock1d[s2]"), vec![0.0, 1.0]);
}

/// Issue #559 (end-to-end; e.g. C-LEARN's `c_in_deep_ocean_net_flow` /
/// `heat_in_deep_ocean_net_flow`). A 2-D stock over a shift-mapped
/// subrange whose differing per-equation flow lists force a synthesized
/// `<stock>_net_flow`. `build_synthetic_flow_equation` cloned the RAW
/// subrange-sliced rate (`dflux[scenario, upper] - dflux[scenario, lower]`)
/// into every scalar element key instead of resolving it per element, so
/// the dimension checker reported `MismatchedDimensions` on
/// `stock2d_net_flow`.
///
/// After the per-element resolution fix, that `MismatchedDimensions` is
/// gone (the parser-level shape is correct).
///
/// The end-to-end deep-ocean *value* check is deferred: the C-LEARN
/// deep-ocean is a genuine subscript-shift recurrence (`dflux` depends on
/// the stock) that only *simulates* once element-level cycle resolution
/// also lands (#559). This minimal repro's `dflux` is constant, so it has
/// no such cycle; we therefore assert ONLY the parser-level symptom
/// removal (no `MismatchedDimensions` on the synthesized net-flow) and
/// explicitly do NOT assert deep-ocean simulation *values* here.
#[test]
fn simulates_synthetic_net_flow_shape() {
    use simlin_engine::common::ErrorCode;
    let mdl = "\
{UTF-8}
scenario: s1, s2 ~~|
layers: (L1-L4) ~~|
upper: (L1-L3) -> lower ~~|
lower: (L2-L4) -> upper ~~|
bottom: L4 ~~|
dflux[scenario, L1] = 5 ~~|
dflux[scenario, lower] = 6 ~~|
stock2d[scenario, upper] = INTEG(dflux[scenario, upper] - dflux[scenario, lower], 0) ~~|
stock2d[scenario, bottom] = INTEG(dflux[scenario, bottom], 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let (_compile_err, diags) = compile_diags(mdl);
    // The bug surfaces as MismatchedDimensions on the synthesized
    // `_net_flow` (raw subrange-sliced rate cloned into scalar element
    // keys). After the fix that specific symptom is gone.
    let mismatched: Vec<_> = diags
        .iter()
        .filter(|d| diag_code(d) == Some(ErrorCode::MismatchedDimensions))
        .collect();
    assert!(
        mismatched.is_empty(),
        "the synthesized net-flow must be resolved per element so no \
         MismatchedDimensions remains (parser-level shape fix). Found: \
         {mismatched:#?}"
    );
}

/// All test models that the monolithic compiler can handle.
/// The incremental path must also handle these.
static ALL_INCREMENTALLY_COMPILABLE_MODELS: &[&str] = &[
    "../../test/alias1/alias1.stmx",
    "../../test/builtin_init/builtin_init.stmx",
    "../../test/arrays1/arrays.stmx",
    "../../test/array_sum_simple/array_sum_simple.xmile",
    "../../test/array_sum_expr/array_sum_expr.xmile",
    "../../test/array_multi_source/array_multi_source.xmile",
    "../../test/array_broadcast/array_broadcast.xmile",
    "../../test/modules_hares_and_foxes/modules_hares_and_foxes.stmx",
    "../../test/modules2/modules2.xmile",
    "../../test/circular-dep-1/model.stmx",
    "../../test/previous/model.stmx",
    "../../test/modules_with_complex_idents/modules_with_complex_idents.stmx",
    "../../test/step_into_smth1/model.stmx",
    "../../test/subscript_index_name_values/model.stmx",
    "../../test/sdeverywhere/models/active_initial/active_initial.xmile",
    "../../test/sdeverywhere/models/lookup/lookup.xmile",
    "../../test/sdeverywhere/models/sum/sum.xmile",
    "../../test/sdeverywhere/models/delay/delay.xmile",
    "../../test/sdeverywhere/models/smooth3/smooth3.xmile",
    "../../test/sdeverywhere/models/smooth/smooth.xmile",
    "../../test/lookup_arrayed/lookup_arrayed.xmile",
];

/// Verify that the salsa-based incremental compilation path successfully
/// compiles every test model that the monolithic path handles.
#[cfg(feature = "file_io")]
#[test]
fn incremental_compilation_covers_all_models() {
    let mut failures: Vec<(String, String)> = Vec::new();

    for model_path in ALL_INCREMENTALLY_COMPILABLE_MODELS
        .iter()
        .chain(TEST_MODELS.iter())
    {
        let f = match File::open(model_path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let mut f = BufReader::new(f);

        let datamodel_project = if model_path.ends_with(".stmx") || model_path.ends_with(".xmile") {
            match xmile::project_from_reader(&mut f) {
                Ok(p) => p,
                Err(_) => continue,
            }
        } else {
            continue;
        };

        let mut salsa_db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut salsa_db, &datamodel_project, None);
        let result = compile_project_incremental(&salsa_db, sync.project, "main");

        if let Err(e) = result {
            failures.push((model_path.to_string(), format!("{e}")));
        }
    }

    if !failures.is_empty() {
        eprintln!("\nIncremental compilation failures:");
        for (model, err) in &failures {
            eprintln!("  {model}: {err}");
        }
        panic!(
            "{} of {} models failed incremental compilation",
            failures.len(),
            ALL_INCREMENTALLY_COMPILABLE_MODELS.len() + TEST_MODELS.len(),
        );
    }
}

// -- External data model tests (MDL path with FilesystemDataProvider) --

// Requires Excel data support (ext_data feature), out of scope
#[cfg(feature = "ext_data")]
#[test]
#[ignore]
fn simulates_directdata_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directdata/directdata.mdl");
}

#[test]
fn simulates_directconst_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directconst/directconst.mdl");
}

#[test]
fn simulates_directlookups_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directlookups/directlookups.mdl");
}

#[test]
fn simulates_directsubs_mdl() {
    simulate_mdl_path_with_data("../../test/sdeverywhere/models/directsubs/directsubs.mdl");
}

/// End-to-end test: scalar GET DIRECT DATA from CSV, parsed through MDL
/// pipeline with FilesystemDataProvider, simulated via VM.
#[test]
fn simulates_get_direct_data_scalar_csv() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();

    // Write a simple CSV data file
    let csv_path = dir.path().join("scalar_data.csv");
    let mut f = std::fs::File::create(&csv_path).unwrap();
    write!(f, "Year,Value\n2000,100\n2010,200\n2020,300\n").unwrap();

    let mdl = "\
{UTF-8}
x := GET DIRECT DATA('scalar_data.csv', ',', 'A', 'B2') ~~|
y = x * 2 ~~|
INITIAL TIME = 2000 ~~|
FINAL TIME = 2020 ~~|
TIME STEP = 10 ~~|
SAVEPER = TIME STEP ~~|
";
    let provider = FilesystemDataProvider::new(dir.path());
    let datamodel_project = open_vensim_with_data(mdl, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    let get = |name: &str| -> Vec<f64> {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().map(|row| row[off]).collect()
    };

    let x_vals = get("x");
    assert_eq!(x_vals.len(), 3);
    assert!(
        (x_vals[0] - 100.0).abs() < 1e-6,
        "x at t=2000 should be 100"
    );
    assert!(
        (x_vals[1] - 200.0).abs() < 1e-6,
        "x at t=2010 should be 200"
    );
    assert!(
        (x_vals[2] - 300.0).abs() < 1e-6,
        "x at t=2020 should be 300"
    );

    let y_vals = get("y");
    assert!(
        (y_vals[0] - 200.0).abs() < 1e-6,
        "y at t=2000 should be 200"
    );
    assert!(
        (y_vals[2] - 600.0).abs() < 1e-6,
        "y at t=2020 should be 600"
    );
}

/// End-to-end test: scalar GET DIRECT CONSTANTS from CSV.
#[test]
fn simulates_get_direct_constants_scalar_csv() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();

    let csv_path = dir.path().join("const_data.csv");
    let mut f = std::fs::File::create(&csv_path).unwrap();
    write!(f, "label,\n,42\n").unwrap();

    let mdl = "\
{UTF-8}
a = GET DIRECT CONSTANTS('const_data.csv', ',', 'B2') ~~|
b = a + 8 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 1 ~~|
TIME STEP = 1 ~~|
SAVEPER = TIME STEP ~~|
";
    let provider = FilesystemDataProvider::new(dir.path());
    let datamodel_project = open_vensim_with_data(mdl, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    let get = |name: &str| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().next().unwrap()[off]
    };

    assert!((get("a") - 42.0).abs() < 1e-6, "a should be 42");
    assert!((get("b") - 50.0).abs() < 1e-6, "b should be 50");
}

/// End-to-end test: scalar GET DIRECT LOOKUPS from CSV.
#[test]
fn simulates_get_direct_lookups_scalar_csv() {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();

    // CSV with time in column 1 and values in column 2:
    // row 1: header
    // row 2+: data pairs (x, y)
    let csv_path = dir.path().join("lookup_data.csv");
    let mut f = std::fs::File::create(&csv_path).unwrap();
    write!(f, "time,value\n0,10\n5,20\n10,30\n").unwrap();

    let mdl = "\
{UTF-8}
x := GET DIRECT LOOKUPS('lookup_data.csv', ',', 'A', 'B2') ~~|
y = x * 2 ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 10 ~~|
TIME STEP = 5 ~~|
SAVEPER = TIME STEP ~~|
";
    let provider = FilesystemDataProvider::new(dir.path());
    let datamodel_project = open_vensim_with_data(mdl, Some(&provider))
        .unwrap_or_else(|e| panic!("failed to parse: {e}"));

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    let get = |name: &str, step: usize| -> f64 {
        let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
        let off = results.offsets[&ident];
        results.iter().nth(step).unwrap()[off]
    };

    // At time 0: x=10, y=20
    assert!((get("x", 0) - 10.0).abs() < 1e-6, "x at t=0 should be 10");
    assert!((get("y", 0) - 20.0).abs() < 1e-6, "y at t=0 should be 20");
    // At time 5: x=20, y=40
    assert!((get("x", 1) - 20.0).abs() < 1e-6, "x at t=5 should be 20");
    assert!((get("y", 1) - 40.0).abs() < 1e-6, "y at t=5 should be 40");
    // At time 10: x=30, y=60
    assert!((get("x", 2) - 30.0).abs() < 1e-6, "x at t=10 should be 30");
    assert!((get("y", 2) - 60.0).abs() < 1e-6, "y at t=10 should be 60");
}

#[test]
fn mark2_mdl_compiles_incrementally() {
    let contents =
        std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").expect("read mark2.mdl");
    let project = open_vensim(&contents).expect("parse mark2.mdl");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("mark2.mdl should compile incrementally");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM should run to completion");
}

/// Reproduce the browser path: open_vensim → protobuf serialize → protobuf
/// deserialize → compile. The app round-trips through protobuf between
/// import and simulation.
#[test]
fn mark2_mdl_compiles_after_protobuf_roundtrip() {
    use prost::Message;

    let contents =
        std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").expect("read mark2.mdl");
    let project = open_vensim(&contents).expect("parse mark2.mdl");

    // Serialize to protobuf (as the app does in NewProject.tsx)
    let pb = serialize(&project).expect("serialize to protobuf");
    let mut buf = Vec::new();
    pb.encode(&mut buf).expect("encode protobuf");

    // Deserialize from protobuf (as the app does when loading from storage)
    let pb2 = project_io::Project::decode(buf.as_slice()).expect("decode protobuf");
    let project2 = deserialize(pb2);

    // Compile the round-tripped project
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project2, None);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("mark2.mdl should compile after protobuf round-trip");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM should run to completion");
}

/// The browser's model.run() defaults analyzeLtm=true, so simNew is called
/// with enable_ltm=true. Models with SMOOTH/DELAY that participate in
/// feedback loops must compile with LTM enabled.
#[test]
fn mark2_mdl_compiles_with_ltm_enabled() {
    use simlin_engine::db::set_project_ltm_enabled;

    let contents =
        std::fs::read_to_string("../../test/bobby/vdf/econ/mark2.mdl").expect("read mark2.mdl");
    let project = open_vensim(&contents).expect("parse mark2.mdl");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    set_project_ltm_enabled(&mut db, sync.project, true);
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("mark2.mdl should compile with LTM enabled");
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM should run to completion");
}

// ===========================================================================
// Phase 3 / Task 4: single-output macro simulation fixtures and edge cases
//
// Group 1 wires the six bundled `.mdl` macro fixtures into dedicated tests,
// each running `open_vensim` -> `compile_vm` -> VM -> `ensure_results` against
// the fixture's `output.tab` (the same pipeline `simulate_mdl_path` runs).
//
// Group 2 covers the five single-output behaviors that have no bundled
// fixture, using a trivial inline `.mdl` string with hand-computed expected
// values. NOTE (GH #553): a single-argument `NAME(arg)` MDL call is rewritten
// to `LOOKUP(NAME, arg)` before macro resolution, so every inline macro below
// uses >= 2 parameters so the call survives MDL import as a macro invocation.
// ===========================================================================

/// Read a scalar variable's value at simulation step `step` (0-based) from a
/// `Results`. Panics with a clear message if the variable is absent.
fn macro_test_value_at(results: &Results, name: &str, step: usize) -> f64 {
    let ident = simlin_engine::common::Ident::<simlin_engine::common::Canonical>::new(name);
    let off = *results.offsets.get(&ident).unwrap_or_else(|| {
        panic!(
            "variable {name:?} not in results; present: {:?}",
            results.offsets.keys().collect::<Vec<_>>()
        )
    });
    results.iter().nth(step).unwrap_or_else(|| {
        panic!(
            "no step {step} in results (step_count={})",
            results.step_count
        )
    })[off]
}

/// Parse + compile + run an inline Vensim `.mdl` string through the same VM
/// path the fixture tests use, returning the `Results`.
fn run_inline_mdl(mdl: &str) -> Results {
    let datamodel_project =
        open_vensim(mdl).unwrap_or_else(|e| panic!("failed to parse inline macro mdl: {e}"));
    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    vm.into_results()
}

// --- Group 1: the six bundled `.mdl` fixtures ------------------------------

/// macros.AC2.1: a stockless single-output macro
/// (`EXPRESSION MACRO(input, parameter) = input * parameter`) simulates and
/// matches `output.tab` (`macro output = 5 * 1.1 = 5.5`).
#[test]
fn simulates_macro_expression_mdl() {
    simulate_mdl_path("../../test/test-models/tests/macro_expression/test_macro_expression.mdl");
}

/// macros.AC2.2: a stock-bearing macro
/// (`EXPRESSION MACRO = INTEG(input, parameter)`) simulates with correct
/// per-invocation integration across the 11-step `output.tab`
/// (init 1.1, +5/step: 1.1, 6.1, 11.1, ... 51.1).
#[test]
fn simulates_macro_stock_mdl() {
    simulate_mdl_path("../../test/test-models/tests/macro_stock/test_macro_stock.mdl");
}

/// macros.AC2.5: a multi-equation macro body with a macro-local helper
/// (`EXPRESSION MACRO = input * intermediate`, `intermediate = parameter * 3`)
/// simulates correctly (`5 * (1.1 * 3) = 16.5`). Additionally asserts the
/// `intermediate` helper does not leak into the `main` model's namespace.
#[test]
fn simulates_macro_multi_expression_mdl() {
    let path =
        "../../test/test-models/tests/macro_multi_expression/test_macro_multi_expression.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // The `intermediate` helper is a macro-body aux; it must live inside the
    // macro model, never in `main`. `ensure_results` only checks expected
    // columns, so we assert namespace isolation explicitly here.
    let main = datamodel_project
        .get_model("main")
        .expect("project must contain a \"main\" model");
    assert!(
        main.get_variable("intermediate").is_none(),
        "the macro-local `intermediate` helper must not leak into `main`; \
         main variables: {:?}",
        main.variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect::<Vec<_>>()
    );

    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {path}: {e}"));
    let results = vm.into_results();

    let expected = load_expected_results_for_mdl(path)
        .unwrap_or_else(|| panic!("no reference data found for {path}"));
    ensure_results(&expected, &results);
}

/// macros.AC2.6: a macro that calls another macro
/// (`EXPRESSION MACRO = SECOND MACRO(input, parameter)`,
/// `SECOND MACRO = input / parameter`) expands recursively and simulates
/// (`5 / 1.1 = 4.54545`).
#[test]
fn simulates_macro_cross_reference_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_cross_reference/test_macro_cross_reference.mdl",
    );
}

/// Two independent macros in one model: `macro output` uses
/// `EXPRESSION MACRO` (`5 * 1.1 = 5.5`) and `second macro output` uses
/// `SECOND MACRO` (`5 / 1.1 = 4.54545`).
#[test]
fn simulates_macro_multi_macros_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_multi_macros/test_macro_multi_macros.mdl",
    );
}

/// macros.AC5.5: a macro defined *after* its first use
/// (`macro output = EXPRESSION MACRO(...)` precedes the `:MACRO:` block)
/// still resolves and simulates (`5 * 1.1 = 5.5`).
#[test]
fn simulates_macro_trailing_definition_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_trailing_definition/test_macro_trailing_definition.mdl",
    );
}

// --- macros.AC6.3: focused C-LEARN-macro isolation fixtures ----------------
//
// Each of C-LEARN's three *invoked* macros (`SAMPLE UNTIL`, `SSHAPE`,
// `RAMP FROM TO`) is exercised by a small focused `.mdl` whose `:MACRO:`
// block is copied VERBATIM from `C-LEARN v77 for Vensim.mdl` and invoked
// with known constant inputs (>= 2 args so the call is not rewritten to
// `LOOKUP` -- GH #553). The expected `output.tab` is hand-computed by
// applying the macro body formula to those inputs (worked out in each
// fixture's README.md, grounded in the engine's `STEP`/`RAMP`/`INTEG`
// semantics). C-LEARN's uninvoked `INIT` macro needs no focused model --
// macros.AC6.2's "parse, register, expand" (`corpus_clearn_macros_import`)
// covers it (the macros.AC1.7 "defined but never invoked" case).
//
// No Vensim DSS reference `.vdf` is checked in for these focused fixtures
// (authoring one is a documented prerequisite/setup task per the design's
// "Test prerequisites" note, not implementation work); the formula-derived
// `output.tab` is the gate. `simulate_mdl_path` prefers `output.tab`/`.dat`
// already -- if a `.vdf` is later added, a `.vdf`-aware path would prefer it.

/// macros.AC6.3 -- C-LEARN's `SAMPLE UNTIL` macro (a stock that tracks
/// `input` until `lastTime`, then FREEZES) computes its defined behavior
/// on a *time-varying* input: `SAMPLE UNTIL(4, 5+RAMP(1,0,10), 99)` =
/// `[99, 5, 6, 7, 8, 8, 8, 8, 8]` over t = 0..8 -- it tracks the rising
/// input through t=3, then holds the sampled 8 (distinct from both the
/// init 99 and every later input 9..13, so the freeze is discriminating;
/// see the fixture README for the full derivation).
#[test]
fn simulates_macro_clearn_sample_until_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_clearn_sample_until/test_macro_clearn_sample_until.mdl",
    );
}

/// macros.AC6.3 -- C-LEARN's `SSHAPE` macro (an S-curve with a macro-local
/// `input = MIN(1, MAX(0, xin))` clamp) computes its defined behavior on
/// both `IF THEN ELSE` branches: `SSHAPE(0.8, 2) = 0.92` (upper),
/// `SSHAPE(0.3, 2) = 0.18` (lower).
#[test]
fn simulates_macro_clearn_sshape_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_clearn_sshape/test_macro_clearn_sshape.mdl",
    );
}

/// macros.AC6.3 -- C-LEARN's `RAMP FROM TO` macro (a 7-body-variable
/// from/to ramp) computes its defined behavior on the linear branch:
/// `RAMP FROM TO(2, 10, 1, 5, 1)` = `[2, 2, 4, 6, 8, 10, 10]` over
/// t = 0..6 (a clamped linear ramp from 2 to 10).
#[test]
fn simulates_macro_clearn_ramp_from_to_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_clearn_ramp_from_to/test_macro_clearn_ramp_from_to.mdl",
    );
}

/// A macro-marked model's definition, reduced to the parts that must survive
/// a cross-format conversion: its name, its `MacroSpec`, and its body
/// variables as `(ident, equation)` pairs sorted by ident (so the comparison
/// is order-independent).
#[derive(Debug, Clone, PartialEq)]
struct MacroDef {
    name: String,
    spec: simlin_engine::datamodel::MacroSpec,
    body: Vec<(String, Option<simlin_engine::datamodel::Equation>)>,
}

/// Collect every macro-marked model in `project` as a `MacroDef`, sorted by
/// macro name.
fn collect_macro_defs(project: &simlin_engine::datamodel::Project) -> Vec<MacroDef> {
    let mut defs: Vec<MacroDef> = project
        .models
        .iter()
        .filter_map(|m| {
            m.macro_spec.as_ref().map(|spec| {
                let mut body: Vec<(String, Option<simlin_engine::datamodel::Equation>)> = m
                    .variables
                    .iter()
                    .map(|v| (v.get_ident().to_string(), v.get_equation().cloned()))
                    .collect();
                body.sort_by(|a, b| a.0.cmp(&b.0));
                MacroDef {
                    name: m.name.clone(),
                    spec: spec.clone(),
                    body,
                }
            })
        })
        .collect();
    defs.sort_by(|a, b| a.name.cmp(&b.name));
    defs
}

/// macros.AC4.4: a single-output macro survives a cross-format conversion
/// `.mdl` -> datamodel -> `.xmile` -> datamodel. We `open_vensim` a
/// single-output macro `.mdl` fixture, convert the resulting
/// `datamodel::Project` to XMILE via `to_xmile`, re-import it via
/// `open_xmile`, and assert the macro definition (the macro-marked `Model` +
/// its `MacroSpec`) and the invocation are preserved -- the
/// cross-format-round-tripped project's macro models and invocation equations
/// match those of the directly-imported `.mdl` project.
#[test]
fn macro_cross_format_mdl_to_xmile_to_datamodel_preserves_macro() {
    let path = "../../test/test-models/tests/macro_expression/test_macro_expression.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));

    // .mdl -> datamodel (the reference shape).
    let from_mdl = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // datamodel -> .xmile -> datamodel (the cross-format round-trip).
    let xmile_str =
        simlin_engine::to_xmile(&from_mdl).unwrap_or_else(|e| panic!("to_xmile failed: {e}"));
    let cross_rt = {
        let mut reader = BufReader::new(xmile_str.as_bytes());
        simlin_engine::open_xmile(&mut reader)
            .unwrap_or_else(|e| panic!("open_xmile of the converted XMILE failed: {e}"))
    };

    let mdl_defs = collect_macro_defs(&from_mdl);
    let rt_defs = collect_macro_defs(&cross_rt);

    // There IS a macro definition, and it is preserved exactly across the
    // cross-format conversion (name, MacroSpec, and body equations).
    assert!(
        !mdl_defs.is_empty(),
        "the .mdl fixture must import at least one macro-marked model"
    );
    assert_eq!(
        mdl_defs, rt_defs,
        "the macro definition (macro-marked Model + MacroSpec + body) must \
         survive the .mdl -> .xmile -> datamodel cross-format conversion"
    );

    // The invocation is preserved: the `main` model's invocation aux reads
    // the same macro call equation before and after the cross-format trip.
    let invocation_eqn = |p: &simlin_engine::datamodel::Project| -> String {
        let main = p.get_model("main").expect("project has a `main` model");
        let v = main
            .get_variable("macro output")
            .expect("`main` has the `macro output` invocation variable");
        match v.get_equation() {
            Some(simlin_engine::datamodel::Equation::Scalar(s)) => s.clone(),
            other => panic!("expected a scalar invocation equation, got {other:?}"),
        }
    };
    let mdl_inv = invocation_eqn(&from_mdl);
    let rt_inv = invocation_eqn(&cross_rt);
    assert_eq!(
        mdl_inv, rt_inv,
        "the macro invocation equation must survive the cross-format \
         conversion (mdl: {mdl_inv:?}, round-tripped: {rt_inv:?})"
    );
    // And it really is an invocation of the imported macro.
    let macro_name = &mdl_defs[0].name;
    assert!(
        mdl_inv.to_lowercase().contains(macro_name.as_str()),
        "the invocation {mdl_inv:?} must call the macro {macro_name:?}"
    );
}

// --- Group 2: focused tests for behaviors with no bundled fixture ----------

/// macros.AC2.3: the same stock-bearing macro invoked at two call sites with
/// different arguments produces independent per-invocation state -- the two
/// invocations do not share a stock.
///
/// Macro `M(rate, init) = INTEG(rate, init)`.
///   x = M(1, 0):  Euler dt=1, x[k] = init + rate*k = 0 + 1*k = k
///   y = M(2, 10): y[k] = 10 + 2*k
/// (Vensim INTEG: value at step k (t=k) is init + rate*k -- same shape the
/// `macro_stock` fixture confirms: init 1.1, rate 5 -> 1.1, 6.1, ...).
/// With INITIAL TIME=0, FINAL TIME=4, TIME STEP=1: 5 steps, t=0..4.
///   x = [0, 1, 2, 3, 4]   y = [10, 12, 14, 16, 18]
#[test]
fn simulates_macro_independent_invocation_state() {
    let mdl = "\
{UTF-8}
:MACRO: M(rate, init)
M = INTEG(rate, init)
	~	stock
	~	per-invocation independent state
	|
:END OF MACRO:
x= M(1, 0) ~~|
y= M(2, 10) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    let expected_x = [0.0, 1.0, 2.0, 3.0, 4.0];
    let expected_y = [10.0, 12.0, 14.0, 16.0, 18.0];
    for step in 0..5 {
        let x = macro_test_value_at(&results, "x", step);
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (x - expected_x[step]).abs() < 1e-9,
            "step {step}: x = {x}, expected {} (M(1,0), independent stock)",
            expected_x[step]
        );
        assert!(
            (y - expected_y[step]).abs() < 1e-9,
            "step {step}: y = {y}, expected {} (M(2,10), independent stock)",
            expected_y[step]
        );
    }
}

/// clearn-residual.AC3.1: a scalar `INITIAL` capture routed through a trivial
/// passthrough macro (`:MACRO: INIT(x) = INITIAL(x)`) holds its value constant
/// across every saved step.
///
/// The MDL importer renames Vensim `INITIAL` -> `INIT`, so `captured =
/// INIT(growing)` -- written against the `INIT` macro -- and the macro body
/// `INIT = INITIAL(x)` (stored as `init = init(x)`) collide. `INITIAL(growing)`
/// where `growing = Time` captures `INITIAL TIME` at t0, so `captured` MUST be
/// the constant `INITIAL TIME` (here 2) at every saved step.
///
/// Control window: INITIAL TIME=2, FINAL TIME=6, TIME STEP=1, SAVEPER=1 => 5
/// saved steps (t=2,3,4,5,6). `growing` rises 2,3,4,5,6 while `captured` stays
/// pinned at 2 -- the AC3.1 user-facing invariant (a held-constant INITIAL
/// capture, distinguishable from a value that drifts with `growing`).
///
/// This test asserts only the AC3.1 invariant. It is NOT a value RED->GREEN
/// discriminator for the Task 4 call-site collapse: the pre-collapse
/// synthetic-module path already produces the constant `[2,2,2,2,2]` for a
/// scalar *non-recurrence* INITIAL-capture (the per-element ordering bug the
/// collapse fixes does not affect this scalar case). The genuine value
/// RED->GREEN discriminator is the element-wise
/// `simulates_passthrough_init_macro_element_recurrence` (AC3.2), where the
/// synthetic-module routing produces wrong per-element values (drops to
/// 0/`:NA:` at t>=1) before the collapse.
#[test]
fn simulates_passthrough_init_macro_scalar_capture_is_constant() {
    // The importer renames Vensim INITIAL to INIT, so BOTH the macro body
    // (`INIT = INITIAL(x)` -> `init = init(x)`) AND the caller's
    // `captured = INITIAL(growing)` (-> `init(growing)`) collide with the
    // like-named renamed builtin -- the #591-c1 shape. The caller must write
    // the genuine Vensim builtin `INITIAL(...)` (which the importer recognizes
    // and renames), NOT the post-rename `INIT(...)`: a single-argument
    // `INIT(arg)` written directly would hit the MDL importer's
    // 1-arg-call->LOOKUP heuristic (GH #553) and never reach macro resolution.
    let mdl = "\
{UTF-8}
:MACRO: INIT(x)
INIT = INITIAL(x)
	~	passthrough macro
	~	collides with renamed builtin
	|
:END OF MACRO:
growing = Time ~~|
captured = INITIAL(growing) ~~|
INITIAL TIME = 2 ~~|
FINAL TIME = 6 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);

    // INITIAL TIME = 2; growing = Time; captured = INITIAL(growing) frozen at t0.
    let expected_growing = [2.0, 3.0, 4.0, 5.0, 6.0];
    let captured = element_series(&results, "captured");
    assert_eq!(
        captured.len(),
        expected_growing.len(),
        "expected 5 saved steps (t=2..6), got {}",
        captured.len()
    );
    for (step, &g) in expected_growing.iter().enumerate() {
        let grew = macro_test_value_at(&results, "growing", step);
        assert!(
            (grew - g).abs() < 1e-9,
            "sanity: growing must equal Time ({g}) at step {step}, got {grew}"
        );
        // The capture is INITIAL(growing) = INITIAL TIME = 2 at EVERY step.
        assert!(
            (captured[step] - 2.0).abs() < 1e-9,
            "captured = INIT(growing) must be held constant at INITIAL TIME (2) \
             for every saved step; at step {step} (Time={g}) got {}",
            captured[step]
        );
    }
}

/// macros.AC2.4: a macro invoked with an expression-valued argument
/// (`y = M(a + b, t)`) -- the argument is evaluated in the caller's context.
///
/// Macro `M(in, p) = in * p`. Constants a=3, b=4, t=5.
///   y = (a + b) * t = (3 + 4) * 5 = 35   (constant across all steps)
#[test]
fn simulates_macro_expression_valued_argument() {
    let mdl = "\
{UTF-8}
:MACRO: M(in, p)
M = in * p
	~	product
	~	expression-valued argument
	|
:END OF MACRO:
a= 3 ~~|
b= 4 ~~|
t= 5 ~~|
y= M(a + b, t) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 3 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // (3 + 4) * 5 = 35
    for step in 0..results.step_count {
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - 35.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 35 = (a+b)*t evaluated in caller context"
        );
    }
}

/// macros.AC2.7: a macro body referencing global time via the `$` escape
/// (`Time$`) simulates with the global time values.
///
/// Macro `M(base, offset) = base + offset + Time$` (a second parameter is
/// required so the call is not rewritten to LOOKUP -- GH #553).
///   y = M(10, 0) = 10 + 0 + Time = 10 + Time
/// With INITIAL TIME=0, FINAL TIME=4, TIME STEP=1: y[k] = 10 + k.
#[test]
fn simulates_macro_time_escape() {
    // The units slot (the first `~`) is parsed as a unit expression, so it
    // must not contain a hyphen (`-` is a unit operator); use a plain token.
    let mdl = "\
{UTF-8}
:MACRO: M(base, offset)
M = base + offset + Time$
	~	dmnl
	~	time access from a macro body via the time form of the dollar escape
	|
:END OF MACRO:
y= M(10, 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // y = 10 + 0 + Time
    for step in 0..results.step_count {
        let time = macro_test_value_at(&results, "time", step);
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - (10.0 + time)).abs() < 1e-9,
            "step {step}: y = {y}, expected {} = 10 + Time({time})",
            10.0 + time
        );
    }
}

/// macros.AC2.8: a macro invocation nested inside a larger expression
/// (`y = c + M(x, t)`) expands and simulates correctly.
///
/// Macro `M(a, b) = a * b`. Constants c=100, x=3, t=5.
///   y = c + M(x, t) = 100 + (3 * 5) = 115   (constant across all steps)
#[test]
fn simulates_macro_nested_invocation() {
    let mdl = "\
{UTF-8}
:MACRO: M(a, b)
M = a * b
	~	product
	~	nested invocation inside a larger expression
	|
:END OF MACRO:
c= 100 ~~|
x= 3 ~~|
t= 5 ~~|
y= c + M(x, t) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 3 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // 100 + (3 * 5) = 115
    for step in 0..results.step_count {
        let y = macro_test_value_at(&results, "y", step);
        assert!(
            (y - 115.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 115 = c + M(x,t)"
        );
    }
}

/// macros.AC5.4 (simulation-level): a macro shadowing the `SSHAPE` builtin is
/// resolved to the macro, not the builtin, even though the model also uses
/// other builtins. Task 3 verifies this at expansion level; this confirms it
/// end-to-end through simulation.
///
/// Macro `SSHAPE(x, p) = x + p` (a real `SSHAPE` builtin exists and is a
/// 3-arg S-curve; a 2-arg call is NOT rewritten to LOOKUP).
///   y = SSHAPE(3, 4) = 3 + 4 = 7    (macro definition, NOT the builtin)
///   z = ABS(-7) = 7                 (an unrelated builtin still works)
#[test]
fn simulates_macro_shadowing_sshape_builtin() {
    let mdl = "\
{UTF-8}
:MACRO: SSHAPE(x, p)
SSHAPE = x + p
	~	shadowing macro
	~	a project macro shadows the SSHAPE builtin
	|
:END OF MACRO:
y= SSHAPE(3, 4) ~~|
z= ABS(-7) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 2 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    // The macro defines SSHAPE(x, p) = x + p, so y = 3 + 4 = 7.
    // The real SSHAPE builtin is a 3-arg S-shaped curve and would NOT
    // produce 7 for these inputs; getting 7 proves the macro shadowed it.
    // z = ABS(-7) = 7 confirms unrelated builtins still resolve normally.
    for step in 0..results.step_count {
        let y = macro_test_value_at(&results, "y", step);
        let z = macro_test_value_at(&results, "z", step);
        assert!(
            (y - 7.0).abs() < 1e-9,
            "step {step}: y = {y}, expected 7 = macro SSHAPE(3,4)=3+4 (not the builtin)"
        );
        assert!(
            (z - 7.0).abs() < 1e-9,
            "step {step}: z = {z}, expected 7 = ABS(-7) (unrelated builtin)"
        );
    }
}

// ===========================================================================
// Phase 4 / Task 1: multi-output (`:`-list) macro invocation
//
// A multi-output invocation `total = ADD3(in1, in2, in3 : the min, the max)`
// materializes at MDL import as a Variable::Module plus binding Auxes (the
// LHS reads the primary output; the `:`-list names read the additional
// outputs). The fixture is stockless so every value is constant; its
// `output.tab` lists `total`, `the min`, `the max`, and the downstream
// `spread = the max - the min` (which proves macros.AC3.2: the `:`-list names
// are referenceable by a subsequent equation and carry correct values).
//
//   total  = in1 + in2 + in3    = 7 + 2 + 5         = 14
//   the min = MIN(7, MIN(2, 5)) = 2
//   the max = MAX(7, MAX(2, 5)) = 7
//   spread  = the max - the min = 7 - 2             = 5
// ===========================================================================

/// macros.AC3.1 / macros.AC3.2: the bundled multi-output fixture parses,
/// materializes, compiles, simulates, and matches its hand-computed
/// `output.tab` (`total`/`the min`/`the max`/`spread`).
#[test]
fn simulates_macro_multi_output_mdl() {
    simulate_mdl_path(
        "../../test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl",
    );
}

// ===========================================================================
// Phase 4 / Task 2: arrayed (apply-to-all) macro invocation
//
// Phase 3 made `instantiate_implicit_modules`'s apply-to-all path
// macro-aware (`contains_module_call`), so an arrayed macro invocation
// `out[Region] = SCALE(inp[Region], factor)` rides the EXISTING per-element
// module-expansion machinery -- one independent synthetic Variable::Module
// per dimension element -- with no new mechanism. These tests verify that
// (macros.AC3.4) and the per-element-independent-stock edge (macros.AC3.5).
//
// SCALE / ACCUM each take >= 2 parameters: a 1-arg MDL call `NAME(arg)` is
// rewritten to LOOKUP before macro resolution (GH #553).
// ===========================================================================

/// macros.AC3.4: the bundled arrayed fixture parses, expands per-element,
/// compiles, simulates, and matches its hand-computed `output.tab`
/// (`out[R1]=30`, `out[R2]=60`, `out[R3]=90` = `inp[element] * factor`).
#[test]
fn simulates_macro_arrayed_mdl() {
    simulate_mdl_path("../../test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl");
}

/// macros.AC3.4 (expansion-level): the arrayed invocation
/// `out[Region] = SCALE(inp[Region], factor)` must expand into one
/// *independent* synthetic `Variable::Module` PER `Region` element
/// (subscript-suffixed idents), not a single shared instance. We assert
/// this through the full compile pipeline by inspecting the compiled
/// `Results.offsets`: each per-element macro instance contributes its own
/// `$⁚out⁚{n}⁚scale⁚{elem}·scale` body-output slot.
#[test]
fn arrayed_macro_invocation_expands_one_module_per_element() {
    let path = "../../test/test-models/tests/macro_arrayed/test_macro_arrayed.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));
    let compiled = compile_vm(&datamodel_project);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed: {e}"));
    let results = vm.into_results();

    // One synthetic macro-instance per Region element: the per-element
    // module's primary-output body slot is `$⁚out⁚{n}⁚scale⁚{elem}·scale`.
    // Collect the distinct `{elem}` suffixes.
    let mut per_element_instances: Vec<String> = results
        .offsets
        .keys()
        .filter_map(|k| {
            let s = k.as_str();
            // $⁚out⁚<n>⁚scale⁚<elem>·scale
            let rest = s.strip_prefix("$\u{205a}out\u{205a}")?;
            let rest = rest.split_once('\u{205a}')?.1; // drop the `<n>⁚`
            let elem = rest.strip_prefix("scale\u{205a}")?;
            let elem = elem.strip_suffix("\u{b7}scale")?;
            Some(elem.to_string())
        })
        .collect();
    per_element_instances.sort();
    per_element_instances.dedup();
    assert_eq!(
        per_element_instances,
        vec!["r1".to_string(), "r2".to_string(), "r3".to_string()],
        "the arrayed SCALE invocation must expand into one independent macro \
         instance per Region element (subscript-suffixed), not a shared one; \
         all offsets: {:?}",
        results
            .offsets
            .keys()
            .map(|k| k.as_str().to_string())
            .collect::<Vec<_>>()
    );

    // And the arrayed result itself has one slot per element with the
    // hand-computed value (inp[element] * factor).
    for (elem, expected) in [("r1", 30.0), ("r2", 60.0), ("r3", 90.0)] {
        let v = macro_test_value_at(&results, &format!("out[{elem}]"), 0);
        assert!(
            (v - expected).abs() < 1e-9,
            "out[{elem}] = {v}, expected {expected} (inp[{elem}] * factor)"
        );
    }
}

/// macros.AC3.5: an arrayed invocation of a *stock-bearing* macro gives each
/// dimension element its own persistent stock. Macro
/// `ACCUM(rate, init) = INTEG(rate, init)`, invoked
/// `total[Region] = ACCUM(rate[Region], 0)` with `rate = [1, 3]`.
///
/// Vensim INTEG: value at step k (t = k) is `init + rate*k`. With
/// INITIAL TIME=0, FINAL TIME=4, TIME STEP=1 (5 steps, t=0..4):
///   total[R1] = 0, 1, 2, 3, 4    (its own rate = 1)
///   total[R2] = 0, 3, 6, 9, 12   (its own rate = 3)
/// If the elements shared one stock these series could not differ -- each
/// element integrating its OWN rate proves per-element persistent state.
#[test]
fn simulates_arrayed_macro_per_element_independent_stock() {
    let mdl = "\
{UTF-8}
:MACRO: ACCUM(rate, init)
ACCUM = INTEG(rate, init)
	~	dmnl
	~	per-element independent persistent stock
	|
:END OF MACRO:
Region: R1, R2 ~~|
rate[Region]= 1, 3 ~~|
total[Region]= ACCUM(rate[Region], 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
";
    let results = run_inline_mdl(mdl);
    let expected_r1 = [0.0, 1.0, 2.0, 3.0, 4.0];
    let expected_r2 = [0.0, 3.0, 6.0, 9.0, 12.0];
    for step in 0..5 {
        let r1 = macro_test_value_at(&results, "total[r1]", step);
        let r2 = macro_test_value_at(&results, "total[r2]", step);
        assert!(
            (r1 - expected_r1[step]).abs() < 1e-9,
            "step {step}: total[r1] = {r1}, expected {} (ACCUM with its own rate=1)",
            expected_r1[step]
        );
        assert!(
            (r2 - expected_r2[step]).abs() < 1e-9,
            "step {step}: total[r2] = {r2}, expected {} (ACCUM with its own rate=3)",
            expected_r2[step]
        );
    }
}

// ===========================================================================
// Phase 4 / Task 3: early validation against the multi-output / arrayed
// corpus models (THEIL, SSTATS, C-LEARN). Tests only -- a focused early
// gate; the full tiered corpus harness + Vensim-reference comparison is
// Phase 7. The two heavy models (the SSTATS COVID model; C-LEARN, ~53k
// lines) are #[ignore]d with a documented opt-in per the rust.md
// test-time-budget rules; Theil_2011.mdl compiles+runs in ~40ms so it
// stays a regular test.
// ===========================================================================

/// All `Diagnostic`s for a datamodel project, via the salsa pipeline.
fn collect_project_diagnostics(
    dm: &simlin_engine::datamodel::Project,
) -> Vec<simlin_engine::db::Diagnostic> {
    use simlin_engine::db::{
        SimlinDb, collect_all_diagnostics, compile_project_incremental,
        sync_from_datamodel_incremental,
    };
    let mut db = SimlinDb::default();
    let sync_state = sync_from_datamodel_incremental(&mut db, dm, None);
    let sync = sync_state.to_sync_result();
    // Drive compilation so the diagnostic accumulators are populated; the
    // Result is intentionally ignored here (callers inspect diagnostics).
    let _ = compile_project_incremental(&db, sync.project, "main");
    collect_all_diagnostics(&db, &sync)
}

/// Pure predicate: is `equation` EXACTLY the `{module_ident}.{output}`
/// binding form a materialized multi-output aux carries?
///
/// Materialization emits `Equation::Scalar(format!("{module_ident}.{output}"))`
/// verbatim, where `output` is a bare macro-output identifier
/// (`spec.primary_output` / `spec.additional_outputs[i]`). The match is
/// therefore exact, not a prefix: split on the FIRST ASCII `.`; the part
/// before it must equal `module_ident` exactly, and the remainder must be a
/// *single bare identifier token* -- non-empty and composed solely of
/// canonical-identifier characters (ASCII alphanumeric or `_`).
///
/// The first-period split plus the identifier-only suffix check together
/// reject anything that is not the verbatim binding text: a hypothetical
/// multi-segment reference like `mod.sub.out` (the suffix `sub.out`
/// contains `.`), an unrelated aux that merely *references* a module output
/// inside a larger expression (`mod.out + 1` -- the suffix `out + 1`
/// contains spaces and `+`), and a different module's output
/// (`other_mod.out` -- the prefix is not `module_ident`). This avoids the
/// prior `starts_with("{mi}.")` over-count while making the predicate as
/// precise as the materialized form it recognizes.
fn is_module_output_binding(equation: &str, module_ident: &str) -> bool {
    let Some((prefix, suffix)) = equation.split_once('.') else {
        return false;
    };
    prefix == module_ident
        && !suffix.is_empty()
        && suffix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Count a model's `Variable::Module`s whose `model_name` is `macro_model`,
/// plus the binding `Aux`es whose Scalar equation is EXACTLY the
/// `<module>.<output>` binding form (ASCII period -- the datamodel
/// separator). Returns `(module_count, binding_aux_count)`.
fn count_materialized_macro(
    project: &simlin_engine::datamodel::Project,
    macro_model: &str,
) -> (usize, usize) {
    use simlin_engine::datamodel::{Equation, Variable};
    let main = project.get_model("main").expect("project has a main model");
    let module_idents: Vec<String> = main
        .variables
        .iter()
        .filter_map(|v| match v {
            Variable::Module(m) if m.model_name == macro_model => Some(m.ident.clone()),
            _ => None,
        })
        .collect();
    let binding_auxes = main
        .variables
        .iter()
        .filter(|v| match v {
            Variable::Aux(a) => match &a.equation {
                Equation::Scalar(s) => module_idents
                    .iter()
                    .any(|mi| is_module_output_binding(s, mi)),
                _ => false,
            },
            _ => false,
        })
        .count();
    (module_idents.len(), binding_auxes)
}

/// macros.AC3.3 -- THEIL. The metasd Theil model's 2-input/13-output
/// `THEIL` multi-output invocation materializes (one `Variable::Module` +
/// 1 primary + 13 additional binding `Aux`es), compiles, and runs to the
/// end. ~40ms total, so this is a regular (non-ignored) test.
#[test]
fn corpus_theil_multi_output_materializes_and_simulates() {
    let path = "../../test/metasd/theil-statistics/Theil_2011.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // THEIL macro imported with the correct 2-input/13-output spec.
    let theil = dm
        .models
        .iter()
        .find(|m| m.name == "theil" && m.macro_spec.is_some())
        .expect("THEIL macro must import as a macro-marked model");
    let spec = theil.macro_spec.as_ref().unwrap();
    assert_eq!(spec.parameters, vec!["historical", "simulated"]);
    assert_eq!(
        spec.additional_outputs.len(),
        13,
        "THEIL has 13 `:`-outputs"
    );

    // The multi-output invocation materialized: one Module + (1 primary +
    // 13 additional) binding auxes = 14.
    let (modules, bindings) = count_materialized_macro(&dm, "theil");
    assert_eq!(modules, 1, "exactly one THEIL module instance");
    assert_eq!(
        bindings, 14,
        "1 primary + 13 additional THEIL binding auxes"
    );

    // It compiles and runs to the end.
    let compiled = compile_vm(&dm);
    let mut vm = Vm::new(compiled).unwrap_or_else(|e| panic!("THEIL VM creation failed: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("THEIL VM run failed: {e}"));
    let _ = vm.into_results();
}

/// macros.AC3.3 -- SSTATS. The metasd COVID model's two
/// 2-input/10-output `SSTATS` invocations both materialize (each: one
/// `Variable::Module` + 1 primary + 10 additional binding `Aux`es).
///
/// This large real-world COVID model has UNRELATED, non-macro blockers
/// that prevent it reaching a runnable VM: its `*_data` variables are
/// unresolved `GET DIRECT/GET XLS DATA` references (no DataProvider /
/// data files are supplied here), so they compile to
/// `EmptyEquation`/`UnknownBuiltin` and `compile_project_incremental`
/// returns `not_simulatable`. Per the phase plan, the assertion is
/// therefore narrowed to "SSTATS multi-output materialization succeeded
/// and produced no macro-specific compile diagnostics"; the unrelated
/// GET-DIRECT-data blocker is reported for Phase-7 tiered-harness scope.
///
/// `#[ignore]` (large COVID model).
// Run with: cargo test --release -- --ignored corpus_sstats_multi_output_materializes
#[test]
#[ignore]
fn corpus_sstats_multi_output_materializes() {
    let path = "../../test/metasd/covid19-us-homer/homer v8/Covid19US v8.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // SSTATS macro imported with the correct 2-input/10-output spec.
    let sstats = dm
        .models
        .iter()
        .find(|m| m.name == "sstats" && m.macro_spec.is_some())
        .expect("SSTATS macro must import as a macro-marked model");
    let spec = sstats.macro_spec.as_ref().unwrap();
    assert_eq!(spec.parameters, vec!["historical", "simulated"]);
    assert_eq!(
        spec.additional_outputs.len(),
        10,
        "SSTATS has 10 `:`-outputs"
    );

    // BOTH SSTATS invocations materialized: 2 Module instances, each with
    // 1 primary + 10 additional binding auxes => 2 modules, 22 bindings.
    let (modules, bindings) = count_materialized_macro(&dm, "sstats");
    assert_eq!(
        modules, 2,
        "both SSTATS invocations must materialize as module instances"
    );
    assert_eq!(
        bindings, 22,
        "2 invocations x (1 primary + 10 additional) SSTATS binding auxes"
    );

    // No macro-specific compile diagnostic (the COVID model's only
    // blockers are the unrelated unresolved `*_data` GET-DIRECT
    // references; the SSTATS macro itself must not produce
    // UnknownBuiltin/BadModelName/BadBuiltinArgs/DuplicateMacroName).
    use simlin_engine::common::ErrorCode;
    use simlin_engine::db::DiagnosticError;
    let macro_codes = [
        ErrorCode::UnknownBuiltin,
        ErrorCode::BadModelName,
        ErrorCode::BadBuiltinArgs,
        ErrorCode::DuplicateMacroName,
        ErrorCode::CircularDependency,
    ];
    for d in collect_project_diagnostics(&dm) {
        let code = match &d.error {
            DiagnosticError::Equation(e) => Some(e.code),
            DiagnosticError::Model(e) => Some(e.code),
            _ => None,
        };
        if let Some(c) = code
            && macro_codes.contains(&c)
        {
            // The unrelated `*_data` GET-DIRECT references are the only
            // legitimate UnknownBuiltin/EmptyEquation sources; assert the
            // diagnostic is on such a variable, not on the SSTATS macro.
            let var = d.variable.clone().unwrap_or_default();
            assert!(
                var.ends_with("_data") || var.contains("_data"),
                "unexpected macro-specific diagnostic NOT on an unrelated \
                 `*_data` GET-DIRECT variable: model={} var={:?} {:?}",
                d.model,
                d.variable,
                d.error
            );
        }
    }
}

/// Return every *macro-attributable* diagnostic in `diags` for the
/// already-imported datamodel `dm` -- diagnostics that indicate macro
/// handling itself (registration, expansion, body compilation) failed, as
/// opposed to an unrelated non-macro model blocker.
///
/// After Phases 1-6 a *correctly* macro-using model produces ZERO
/// macro-attributable diagnostics: single-output macro invocations are
/// inlined into the caller and multi-output ones materialize as ordinary
/// `Variable::Module`s + binding auxes, so the only diagnostics a working
/// macro pipeline can emit are unrelated (model-logic / unit / dimension)
/// blockers. There is no macro-specific `ErrorCode`, so a diagnostic is
/// macro-attributable iff ANY of:
///
/// 1. **Macro-registry-build error** -- a project-level (`model` empty,
///    `variable` `None`) `Model` diagnostic with code `CircularDependency`
///    or `DuplicateMacroName`. `db_macro_registry::project_macro_registry`
///    emits this when `MacroRegistry::build` rejects the macro set; an empty
///    registry then un-shadows every macro builtin (the #554 cascade:
///    `SSHAPE`/`SAMPLE UNTIL`/`RAMP FROM TO` calls become
///    `BadBuiltinArgs`/`UnknownBuiltin`). This is distinct from a
///    *model-logic* circular dependency, which is attributed to a
///    model/variable.
/// 2. **Macro-template-body Error** -- an `Error`-severity diagnostic whose
///    `model` is a macro-marked model's name (its `macro_spec` is `Some`).
///    The macro body failed to compile/expand. Unit-inference *warnings* on
///    a macro body (formal-parameter port variables legitimately have no
///    units) are `Warning` severity and are an allowed non-macro unit-error
///    blocker, so they are excluded.
/// 3. **Macro-resolution-failure code** -- a diagnostic whose code is
///    `UnknownBuiltin`/`BadBuiltinArgs`/`BadModelName`/`DuplicateMacroName`
///    AND whose `model` is a macro-marked model OR which is project-level. A
///    bare `UnknownBuiltin`/`UnknownDependency`/`DoesNotExist` on an
///    ordinary `main` variable (an unrelated builtin, a model-logic
///    dependency, or Phase 3's deprioritized non-time `$` reference -- which
///    surfaces as an *ordinary* unresolved-reference diagnostic) is NOT
///    macro-attributable; the classifier must not mistake it for a macro
///    error.
fn macro_attributable_diagnostics<'a>(
    dm: &simlin_engine::datamodel::Project,
    diags: &'a [simlin_engine::db::Diagnostic],
) -> Vec<&'a simlin_engine::db::Diagnostic> {
    use simlin_engine::common::ErrorCode;
    use simlin_engine::db::{DiagnosticError, DiagnosticSeverity};

    let macro_models: std::collections::BTreeSet<&str> = dm
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.as_str())
        .collect();

    // Macro-resolution-failure codes: the symptoms of a macro call that did
    // not resolve to its macro (the registry was empty / the macro name was
    // not registered), so the call site fell through to builtin/module
    // resolution and failed there.
    let resolution_codes = [
        ErrorCode::UnknownBuiltin,
        ErrorCode::BadBuiltinArgs,
        ErrorCode::BadModelName,
        ErrorCode::DuplicateMacroName,
    ];

    let code_of = |d: &simlin_engine::db::Diagnostic| match &d.error {
        DiagnosticError::Equation(e) => Some(e.code),
        DiagnosticError::Model(e) => Some(e.code),
        _ => None,
    };
    let is_registry_build_error = |d: &simlin_engine::db::Diagnostic| {
        d.model.is_empty()
            && d.variable.is_none()
            && matches!(&d.error, DiagnosticError::Model(_))
            && matches!(
                code_of(d),
                Some(ErrorCode::CircularDependency) | Some(ErrorCode::DuplicateMacroName)
            )
    };

    // The #554 cascade is *defined by* a registry-build error: when present,
    // every macro call un-shadows and fails with a resolution-failure code.
    // So a resolution-failure code is macro-attributable when it co-occurs
    // with a registry-build error (the cascade), even on a `main` variable.
    // Absent a registry error, a lone resolution-failure code on an ordinary
    // `main` variable is an unrelated builtin/model issue, not a macro error.
    let registry_error_present = diags.iter().any(&is_registry_build_error);

    diags
        .iter()
        .filter(|d| {
            let code = code_of(d);
            let is_project_level = d.model.is_empty() && d.variable.is_none();
            let in_macro_model = macro_models.contains(d.model.as_str());

            // (1) Macro-registry-build error (the #554 cascade class).
            let registry_build_error = is_registry_build_error(d);

            // (2) Error-severity diagnostic inside a macro template body
            // (unit *warnings* on a macro body are an allowed non-macro
            // unit-error blocker -- excluded by the severity check).
            let macro_body_error = in_macro_model && d.severity == DiagnosticSeverity::Error;

            // (3) Macro-resolution-failure code on a macro model, or
            // project-level, or co-occurring with a registry-build error
            // (the #554 cascade). A bare such code on an ordinary `main`
            // variable with no registry error is an unrelated blocker.
            let resolution_failure = code.map(|c| resolution_codes.contains(&c)).unwrap_or(false)
                && (in_macro_model || is_project_level || registry_error_present);

            registry_build_error || macro_body_error || resolution_failure
        })
        .collect()
}

/// The macro-attributable classifier must (a) flag the three macro-error
/// shapes and (b) NOT flag C-LEARN's allowed non-macro blockers
/// (model-logic `CircularDependency` on a variable, dimension mismatch,
/// non-time `$` unresolved reference, a unit *warning* on a macro body).
/// This pins the classifier so neither the C-LEARN nor the metasd harness
/// assertion can silently degrade into "flags everything" or "flags
/// nothing". Uses a real macro-marked datamodel (a tiny inline `.mdl`) so
/// it is not brittle to `datamodel::Project` struct changes.
#[test]
fn macro_attributable_classifier_separates_macro_from_nonmacro() {
    use simlin_engine::common::{Error, ErrorCode, ErrorKind};
    use simlin_engine::db::{Diagnostic, DiagnosticError, DiagnosticSeverity};

    // A real macro-marked model named `m` (single-output macro `M`).
    let dm = open_vensim(
        "{UTF-8}\n\
         :MACRO: M(a, b)\n\
         M = a * b\n\t~\tdmnl\n\t~\t|\n\
         :END OF MACRO:\n\
         x= M(2, 3) ~~|\n\
         INITIAL TIME = 0 ~~|\n\
         FINAL TIME = 1 ~~|\n\
         SAVEPER = 1 ~~|\n\
         TIME STEP = 1 ~~|\n",
    )
    .expect("inline macro mdl parses");
    assert!(
        dm.models.iter().any(|m| m.macro_spec.is_some()),
        "fixture must have a macro-marked model"
    );
    let macro_model = dm
        .models
        .iter()
        .find(|m| m.macro_spec.is_some())
        .unwrap()
        .name
        .clone();

    let eq = |code: ErrorCode| {
        DiagnosticError::Equation(simlin_engine::common::EquationError {
            start: 0,
            end: 0,
            code,
        })
    };
    let model_err =
        |code: ErrorCode| DiagnosticError::Model(Error::new(ErrorKind::Model, code, None));

    // --- (a) The three macro-error shapes MUST be flagged ---
    let registry_build = Diagnostic {
        model: String::new(),
        variable: None,
        error: model_err(ErrorCode::CircularDependency),
        severity: DiagnosticSeverity::Error,
    };
    let macro_body_error = Diagnostic {
        model: macro_model.clone(),
        variable: Some("m".to_string()),
        error: eq(ErrorCode::UnknownDependency),
        severity: DiagnosticSeverity::Error,
    };
    for d in [&registry_build, &macro_body_error] {
        let flagged = macro_attributable_diagnostics(&dm, std::slice::from_ref(d));
        assert_eq!(
            flagged.len(),
            1,
            "this diagnostic must be macro-attributable: {d:?}"
        );
    }
    // The #554 cascade: a registry-build error PLUS the resulting
    // `UnknownBuiltin` on the macro-invoking `main` variable. Both must be
    // flagged (the resolution failure is macro-attributable *because* the
    // registry error is present).
    let cascade_resolution_failure = Diagnostic {
        model: "main".to_string(),
        variable: Some("x".to_string()),
        error: eq(ErrorCode::UnknownBuiltin),
        severity: DiagnosticSeverity::Error,
    };
    let cascade = [registry_build.clone(), cascade_resolution_failure.clone()];
    let flagged = macro_attributable_diagnostics(&dm, &cascade);
    assert_eq!(
        flagged.len(),
        2,
        "the #554 cascade (registry-build error + the resulting \
         `UnknownBuiltin` on the macro-invoking variable) must BOTH be \
         macro-attributable; flagged: {flagged:#?}"
    );
    // But that same `UnknownBuiltin` on `main.x` ALONE (no registry error)
    // is an unrelated builtin issue -- NOT macro-attributable.
    let lone =
        macro_attributable_diagnostics(&dm, std::slice::from_ref(&cascade_resolution_failure));
    assert!(
        lone.is_empty(),
        "a lone `UnknownBuiltin` on a `main` variable with no registry \
         error is an unrelated blocker, not macro-attributable: {lone:#?}"
    );

    // --- (b) C-LEARN's allowed NON-macro blockers must NOT be flagged ---
    let model_logic_cycle = Diagnostic {
        model: "main".to_string(),
        variable: Some("previous_emissions_intensity_vs_refyr".to_string()),
        error: model_err(ErrorCode::CircularDependency),
        severity: DiagnosticSeverity::Error,
    };
    let dim_mismatch = Diagnostic {
        model: "main".to_string(),
        variable: Some("c_in_mixed_layer".to_string()),
        error: eq(ErrorCode::MismatchedDimensions),
        severity: DiagnosticSeverity::Error,
    };
    // Phase 3's documented limitation: a non-time `$` reference surfaces as
    // an ordinary unresolved-reference diagnostic on a `main` variable.
    let non_time_dollar = Diagnostic {
        model: "main".to_string(),
        variable: Some("\"goal_1.5_for_temperature\"".to_string()),
        error: eq(ErrorCode::DoesNotExist),
        severity: DiagnosticSeverity::Error,
    };
    // A unit-inference WARNING on a macro body (formal-parameter port vars
    // have no units) -- an allowed non-macro unit-error blocker.
    let macro_body_unit_warning = Diagnostic {
        model: macro_model.clone(),
        variable: Some("m".to_string()),
        error: model_err(ErrorCode::UnitMismatch),
        severity: DiagnosticSeverity::Warning,
    };
    let nonmacro = [
        model_logic_cycle,
        dim_mismatch,
        non_time_dollar,
        macro_body_unit_warning,
    ];
    let flagged = macro_attributable_diagnostics(&dm, &nonmacro);
    assert!(
        flagged.is_empty(),
        "C-LEARN's allowed non-macro blockers must NOT be macro-attributable, \
         but the classifier flagged: {flagged:#?}"
    );
}

/// macros.AC6.2 / macros.AC1.7 -- C-LEARN's four macros (`SAMPLE UNTIL`,
/// `SSHAPE`, `RAMP FROM TO`, `INIT`) import as macro-marked models with the
/// correct `MacroSpec`s (including the uninvoked `INIT`, AC1.7), AND the
/// macro registry builds with NO macro-specific errors -- in particular no
/// false `recursive macro: init -> init` from C-LEARN's
/// `:MACRO: INIT(x) ... INIT = INITIAL(x)`.
///
/// HISTORY (#554, FIXED): the MDL importer necessarily renames the Vensim
/// `INITIAL` builtin to `INIT` (`mdl/xmile_compat.rs`; `Expr1` lowering
/// recognizes only the opcode name `init`, not `initial`), so C-LEARN's
/// uninvoked macro stores the datamodel body `init = init(x)`. The recursion
/// detector used to treat that renamed-builtin call as a recursive
/// `init -> init` macro edge and fail the WHOLE `MacroRegistry::build`,
/// which CASCADED: with an empty registry, `SSHAPE`/`SAMPLE UNTIL`/
/// `RAMP FROM TO` stopped shadowing the builtins and their call sites then
/// reported `BadBuiltinArgs`/`UnknownBuiltin`. A single false positive
/// blocked ALL of C-LEARN's macro expansion. #554 fixes this in two
/// coordinated halves sharing `module_functions::is_renamed_opcode_intrinsic`:
/// `collect_called_macros` no longer records the same-named-opcode-intrinsic
/// self-edge, and `BuiltinVisitor::walk` resolves such a call to the
/// intrinsic instead of recursing into the like-named macro. Genuine
/// recursion (`FOO = FOO(x)`, non-intrinsic) is still rejected
/// (macros.AC5.2 unweakened) -- see the `issue_554_*` tests in
/// `src/macro_expansion_tests.rs` and `src/module_functions.rs`.
///
/// This is the C-LEARN macro-expansion regression guard Phase 7 Task 1
/// builds on. It asserts the four macros import correctly AND the
/// #554 macro-attributable cascade is gone (no macro-registry
/// `CircularDependency`). It deliberately does NOT assert that all of
/// C-LEARN compiles -- C-LEARN's non-macro blockers (#552, #553, #363,
/// model-logic deps) remain out of scope -- only that no macro-specific
/// error from #554 fires. `#[ignore]` (C-LEARN is ~53k lines / 1.4 MB;
/// ~4s just to parse).
// Run with: cargo test --release -- --ignored corpus_clearn_macros_import
#[test]
#[ignore]
fn corpus_clearn_macros_import() {
    let path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";
    let contents =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let dm = open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {path}: {e}"));

    // All four C-LEARN macros import as macro-marked models with the
    // correct MacroSpecs (macros.AC1.7: the uninvoked INIT included).
    let expect: &[(&str, &[&str])] = &[
        ("sample_until", &["lasttime", "input", "initval"]),
        ("sshape", &["xin", "profile"]),
        (
            "ramp_from_to",
            &["xfrom", "xto", "tstart", "tend", "islinear"],
        ),
        ("init", &["x"]),
    ];
    for (name, params) in expect {
        let m = dm
            .models
            .iter()
            .find(|m| m.name == *name && m.macro_spec.is_some())
            .unwrap_or_else(|| {
                panic!(
                    "C-LEARN macro {:?} must import as a macro-marked model; \
                     macro models present: {:?}",
                    name,
                    dm.models
                        .iter()
                        .filter(|m| m.macro_spec.is_some())
                        .map(|m| m.name.clone())
                        .collect::<Vec<_>>()
                )
            });
        let spec = m.macro_spec.as_ref().unwrap();
        assert_eq!(
            spec.parameters,
            params.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            "C-LEARN macro {:?} parameter list",
            name
        );
        assert_eq!(
            spec.primary_output, *name,
            "C-LEARN macro {:?} primary output is its own name",
            name
        );
        // All four C-LEARN macros are single-output.
        assert!(
            spec.additional_outputs.is_empty(),
            "C-LEARN macro {:?} is single-output",
            name
        );
    }

    // macros.AC6.2: compile C-LEARN via the salsa path, collect every
    // diagnostic, and assert NO diagnostic is *macro-attributable*. C-LEARN's
    // known NON-macro blockers (circular deps, dimension mismatches, unit
    // errors, and a non-time `$` reference -- Phase 3's documented
    // limitation) are expected and explicitly allowed; the assertion is
    // specifically that macro handling itself introduced no error. The
    // classifier (`macro_attributable_diagnostics`, shared with the metasd
    // corpus harness) catches exactly: a project-level macro-registry build
    // error (the #554 cascade class -- a registry failure un-shadows
    // `SSHAPE`/`SAMPLE UNTIL`/`RAMP FROM TO`, turning every call into
    // `BadBuiltinArgs`/`UnknownBuiltin`), an Error-severity diagnostic inside
    // a macro template body, and a macro-resolution-failure error code
    // (`UnknownBuiltin`/`BadBuiltinArgs`/`BadModelName`/`DuplicateMacroName`)
    // on a macro model or project-level. A bare `UnknownDependency` /
    // `DoesNotExist` on a `main` variable (the non-time `$` case, model-logic
    // deps) is NOT macro-attributable -- the classifier deliberately does not
    // mistake it for a macro error.
    let diags = collect_project_diagnostics(&dm);
    let macro_diags = macro_attributable_diagnostics(&dm, &diags);
    assert!(
        macro_diags.is_empty(),
        "macros.AC6.2: C-LEARN must compile with NO macro-attributable \
         diagnostic (its non-macro blockers -- circular deps, dim mismatches, \
         unit errors, the non-time `$` ref -- are out of scope). The #554 fix \
         removed the false `init -> init` macro-registry recursion and its \
         `SSHAPE`/`SAMPLE UNTIL`/`RAMP FROM TO` cascade; this guards that \
         regression. Found {} macro-attributable diagnostic(s): {macro_diags:#?}",
        macro_diags.len()
    );
}

/// element-cycle-resolution Phase 6 Task 1 -- the C-LEARN incremental
/// structural value-locking gate (AC7.1, AC7.2, AC7.3, AC7.5).
///
/// This is the plan's explicit mid-plan value-locking checkpoint: it locks
/// "C-LEARN compiles via the incremental path + runs to FINAL TIME + no
/// all-NaN core series" before Phase 7's numeric tail, now that Phases 1-5
/// are in (element-level single + multi-variable SCC resolution, the dt+init
/// combined fragment, the synthetic-helper parent-sourcing safety net, and
/// the genuine-Vensim VECTOR SORT ORDER / VECTOR ELM MAP corrections all
/// converge so the false `CircularDependency` no longer gates C-LEARN and
/// NaN no longer propagates from the VECTOR ops).
///
/// - **AC7.1:** parse `C-LEARN v77 for Vensim.mdl`, compile via the
///   incremental path by calling `compile_project_incremental` *directly*
///   (not the `compile_vm` `.unwrap()` wrapper) so the `Result` can be
///   asserted clean rather than panicking. The compile must be `Ok` -- no
///   fatal `ModelError`, specifically NO `CircularDependency`. Non-fatal
///   unit-inference warnings are explicitly allowed (out of scope). On a
///   diagnostic the collected diagnostics are dumped (the
///   `corpus_clearn_macros_import` inspection idiom) so a regression is
///   legible.
/// - **AC7.2:** `Vm::new(...)` then `vm.run_to_end()` runs to FINAL TIME.
///   Deliberately NOT wrapped in `catch_unwind`: per AC7.5 a post-gate panic
///   (#363 reproducing) must be a hard, root-caused failure that propagates
///   with its backtrace, never caught/masked.
/// - **AC7.3:** define "core series" as the matched offset keyset
///   `Ref.vdf.offsets ∩ results.offsets` (there is no `Results` series
///   accessor / "core C-LEARN series" enumeration anywhere, so the matched
///   set is the principled definition -- it also dovetails Phase 7's AC8.2
///   NaN guard). Parse `Ref.vdf` via
///   `VdfFile::parse(...).to_results_via_records()` exactly as
///   `simulates_clearn` does, intersect the offset keysets, and assert that
///   for each matched ident at least one step is non-NaN (the
///   `data[step*step_size+off]` flat-index idiom from `ensure_vdf_results` /
///   `macro_test_value_at`). Fail listing any entirely-NaN matched idents.
///
/// `#[ignore]`d: C-LEARN is ~53k lines / 1.4 MB (~4-5s just to parse on
/// release, far more to compile+run); it must not run in the capped default
/// `cargo test` set. All sibling C-LEARN tests follow this convention.
// Run with: cargo test --release -- --ignored compiles_and_runs_clearn_structural
#[test]
#[ignore]
fn compiles_and_runs_clearn_structural() {
    use simlin_engine::common::ErrorCode;

    let mdl_path = "../../test/xmutil_test_models/C-LEARN v77 for Vensim.mdl";

    eprintln!("model (vensim mdl): {mdl_path}");

    let contents = std::fs::read_to_string(mdl_path)
        .unwrap_or_else(|e| panic!("failed to read {mdl_path}: {e}"));

    let datamodel_project =
        open_vensim(&contents).unwrap_or_else(|e| panic!("failed to parse {mdl_path}: {e}"));

    // AC7.1: compile via the incremental path, calling
    // `compile_project_incremental` DIRECTLY (not the `compile_vm`
    // `.unwrap()` wrapper) so the `Result` can be asserted clean rather than
    // panicking. A fatal `ModelError` -- specifically a `CircularDependency`
    // -- here is a genuine regression or an unresolved cycle-resolution gap,
    // so on failure we dump every collected diagnostic (the
    // `corpus_clearn_macros_import` inspection idiom) to make it legible.
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
    let compile_result = compile_project_incremental(&db, sync.project, "main");

    if compile_result.is_err() {
        let diags = collect_project_diagnostics(&datamodel_project);
        let has_circular = diags
            .iter()
            .any(|d| diag_code(d) == Some(ErrorCode::CircularDependency));
        panic!(
            "AC7.1: C-LEARN must compile via the incremental path with no \
             fatal ModelError (specifically NO CircularDependency -- the \
             element-level cycle resolution of Phases 1-3 must dissolve the \
             false cycle; non-fatal unit-inference warnings are allowed). \
             compile_project_incremental returned Err; has_circular_dependency \
             = {has_circular}. Collected diagnostics: {diags:#?}"
        );
    }

    // Even on `Ok`, a `CircularDependency` must never appear among the
    // collected diagnostics: the whole point of this gate is that the
    // element-level cycle resolution dissolved C-LEARN's false cycle.
    let diags = collect_project_diagnostics(&datamodel_project);
    let circular: Vec<_> = diags
        .iter()
        .filter(|d| diag_code(d) == Some(ErrorCode::CircularDependency))
        .collect();
    assert!(
        circular.is_empty(),
        "AC7.1: C-LEARN must compile with NO CircularDependency diagnostic \
         (the element-level cycle resolution of Phases 1-3 must dissolve the \
         false cycle). Found {} CircularDependency diagnostic(s): {circular:#?}",
        circular.len()
    );

    let compiled = compile_result.expect("checked Ok above");

    // AC7.2: run to FINAL TIME. NOT wrapped in `catch_unwind` -- per AC7.5 a
    // post-gate panic (#363 reproducing now that the cycle gate no longer
    // masks the deeper pipeline) must be a hard, root-caused failure that
    // propagates with its backtrace, never caught/masked.
    let mut vm =
        Vm::new(compiled).unwrap_or_else(|e| panic!("VM creation failed for {mdl_path}: {e}"));
    vm.run_to_end()
        .unwrap_or_else(|e| panic!("VM run failed for {mdl_path}: {e}"));
    let results = vm.into_results();

    // AC7.3: define "core series" as the matched offset keyset
    // `Ref.vdf.offsets ∩ results.offsets` and assert no matched series is
    // entirely NaN after the run. There is no `Results` series accessor /
    // "core C-LEARN series" enumeration anywhere, so the matched set is the
    // principled definition (it also dovetails Phase 7's AC8.2 NaN guard).
    let vdf_path = "../../test/xmutil_test_models/Ref.vdf";
    let vdf_data_bytes =
        std::fs::read(vdf_path).unwrap_or_else(|e| panic!("failed to read {vdf_path}: {e}"));
    let vdf_file = simlin_engine::vdf::VdfFile::parse(vdf_data_bytes)
        .unwrap_or_else(|e| panic!("failed to parse VDF {vdf_path}: {e}"));
    let vdf_results = vdf_file
        .to_results_via_records()
        .unwrap_or_else(|e| panic!("VDF to_results_via_records failed: {e}"));

    let step_count = results.step_count;
    let step_size = results.step_size;
    let mut matched = 0usize;
    let mut all_nan: Vec<String> = Vec::new();
    for ident in vdf_results.offsets.keys() {
        let Some(&off) = results.offsets.get(ident) else {
            continue;
        };
        matched += 1;
        let any_non_nan = (0..step_count).any(|s| !results.data[s * step_size + off].is_nan());
        if !any_non_nan {
            all_nan.push(ident.to_string());
        }
    }

    eprintln!(
        "C-LEARN structural gate: {matched} core series matched \
         (Ref.vdf ∩ results) across {step_count} steps"
    );
    assert!(
        matched > 0,
        "AC7.3: expected a non-empty matched core-series set \
         (Ref.vdf.offsets ∩ results.offsets); got 0 -- the offset keysets \
         must overlap for the NaN guard to be meaningful"
    );
    all_nan.sort();
    assert!(
        all_nan.is_empty(),
        "AC7.3: every matched core C-LEARN series (Ref.vdf ∩ results) must \
         have at least one non-NaN step after the run. {} of {matched} \
         matched series are entirely NaN: {all_nan:#?}",
        all_nan.len()
    );
}

/// Regression for the synthetic-module flows-phase ordering bug behind
/// C-LEARN's `emissions_with_stopped_growth` (#591-c1): an arrayed
/// `SAMPLE IF TRUE(cond, SMOOTH(input, dt), init)` whose SMOOTH `input` has a
/// CURRENT-value (dt-phase) dependency back on the variable itself.
///
/// The desugar is `captured[e] = IF cond THEN smth1·output ELSE PREVIOUS(self,
/// init)`. With the always-true condition the SMOOTH branch is taken every
/// step; smoothing a constant yields that constant, so every element must hold
/// its per-element constant (`base[e]`) at every saved step.
///
/// The flows-phase cycle `captured -> smth1·output -> smth1(module) -> input ->
/// captured` is hidden from CYCLE DETECTION because a module is a sink in the
/// cycle relation (`dt_walk_successors`), so the model compiles (no
/// `CircularDependency`). But `captured` reads the SMOOTH *stock* output, which
/// in the dt phase is a prior-timestep read and must NOT impose a same-step
/// ordering on the module: before the fix, the salsa runlist
/// (`db_dep_graph::build_var_info` / `topo_sort_str`) kept a `captured ->
/// smth1` dt edge anyway (the `·output` stock dep was not chain-broken for a
/// NON-module reader), creating a false ordering cycle that `topo_sort_str`
/// broke by emitting the SMOOTH module BEFORE its `input` for SOME elements
/// (the broken element is HashMap-iteration-order-dependent, hence the bug was
/// also nondeterministic). Those elements then read a stale (0) input each
/// flows step and decayed to 0 at t>=1 while holding the correct value at t0 --
/// exactly the C-LEARN `emissions_with_stopped_growth[cop_developing_b]`
/// symptom (vdf constant, sim drops to 0 at t>=1). The fix chain-breaks a stock
/// submodel-output dep for every reader in the dt phase (mirroring the legacy
/// `model.rs::module_output_deps` `!output_var.is_stock()` gate), so no false
/// ordering cycle forms and every element is ordered input-before-module.
///
/// The `input` is a separate `feedback[cop] = ACTIVE INITIAL(captured*0 + base,
/// base)`, exactly C-LEARN's shape (its `CO2 FF emissions = ACTIVE INITIAL(...,
/// RS CO2 FF(...))`): the dt (active) branch carries the feedback on `captured`
/// that triggers the flows-phase bug, while the init branch's deps are only
/// `base` -- so there is NO init-time algebraic cycle (C-LEARN's emissions has
/// none either, which is why its t0 always matched). `captured*0` keeps the
/// numeric value `base` while preserving the structural dt dependency. `apply`
/// gates the SMOOTH input through an `IF THEN ELSE` so a hoisted per-element
/// `arg0` helper is synthesized (the exact C-LEARN shape, where the mis-ordered
/// node was the `arg0` helper feeding the module). Seven elements (a COP-like
/// dimension) make the order-dependent break observable.
#[test]
fn synthetic_module_feedback_input_ordered_before_module() {
    let mdl = "\
{UTF-8}
cop: e0, e1, e2, e3, e4, e5, e6 ~~|
apply= 2 ~~|
base[cop]= 100,200,300,400,500,600,700 ~~|
feedback[cop]= ACTIVE INITIAL(captured[cop]*0 + base[cop], base[cop]) ~~|
captured[cop]= SAMPLE IF TRUE(Time <= 100, SMOOTH(IF THEN ELSE(apply=1, 0, feedback[cop]), TIME STEP), 0) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 4 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|

\\\\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 1
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|72,72,100,0
10,1,base,150,165,28,8,8,3,0,0,0,0,0,0
10,2,captured,150,200,28,8,8,3,0,0,0,0,0,0
///---\\\\\\
:L<%^E!@
1:Current.vdf
";
    let results = run_inline_mdl(mdl);
    // Each element must hold its per-element constant base[e] across every
    // saved step (smoothing a constant). A mis-ordered module reads a stale 0
    // input and decays to 0 at t>=1 (correct only at t0).
    let elems = ["e0", "e1", "e2", "e3", "e4", "e5", "e6"];
    let expected = [100.0, 200.0, 300.0, 400.0, 500.0, 600.0, 700.0];
    for (e, want) in elems.iter().zip(expected.iter()) {
        let series = element_series(&results, &format!("captured[{e}]"));
        assert_eq!(
            series.len(),
            results.step_count,
            "captured[{e}] should have one value per saved step"
        );
        for (step, &got) in series.iter().enumerate() {
            assert!(
                (got - want).abs() < 1e-9,
                "captured[{e}] must hold the smoothed constant {want} at EVERY \
                 saved step (the SMOOTH input must be ordered before its module \
                 each flows step); at step {step} got {got}, full series \
                 {series:?}"
            );
        }
    }
}
