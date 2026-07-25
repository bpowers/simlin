// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compilation diagnostics: the salsa `CompilationDiagnostic` accumulator,
//! the typed `Diagnostic` value (severity + per-model/per-variable context),
//! the per-model triggering query `model_all_diagnostics`, and the drain
//! helpers `collect_model_diagnostics` / `collect_all_diagnostics`.
//!
//! `model_all_diagnostics` is the single per-model query that drives every
//! PER-MODEL diagnostic source: it triggers `compile_var_fragment` per variable
//! (the emission half lives in `db.rs`), the unit-check pass, and -- when LTM
//! is enabled -- the LTM fragment-diagnostic pass. The two `collect_*` helpers
//! drain the accumulated `CompilationDiagnostic`s for one model or the whole
//! synced project.
//!
//! Three diagnostics do NOT come from the accumulator. `collect_all_diagnostics`
//! emits each directly from a memoized derivation, and they differ in how many
//! rows each produces -- do not collapse them into one rule:
//!
//!   - the **macro-registry build error** (`project_macro_registry`): at most
//!     ONE row per project, naming no model and no variable. A project has one
//!     macro set, and it is either valid or not.
//!   - the **unit-definition errors** (`project_units_context_result`): one row
//!     per declaration that failed to parse, so several are normal. They name no
//!     model -- a project has one `units` list, belonging to none of them -- but
//!     they carry the offending unit's name as `variable`.
//!   - the **module-reference cycle** (`project_module_graph`): one row per
//!     MODEL that can reach a cycle, carrying that model's name, and that model's
//!     per-model passes are skipped. This one is deliberately per-model and must
//!     stay that way: a model reaching no cycle is still processed normally, so
//!     an unrelated draft cycle elsewhere in the project cannot hide a valid
//!     model's diagnostics (GH #806). Reporting it once project-wide would
//!     revert that.
//!
//! What the first two have in common is not their row count but their ORIGIN:
//! each is derived once per project, and each used to be accumulated from inside
//! its deriving query's body. That was wrong twice over. Every model's
//! diagnostic subtree reaches those queries, so the DFS found the single
//! accumulated value once per model and reported N copies; and a value
//! accumulated inside a memo body is only discoverable while the pruning flags
//! along the whole path stay `Any`, so after an unrelated revision bump the
//! deep-verify path recomputed them as `Empty` and the diagnostic vanished from
//! the collection entirely. Reading the memoized value sidesteps both.
//!
//! `model_all_diagnostics` performs an untracked read so it always
//! re-executes: see the in-body comment for why that is load-bearing for
//! diagnostic stability across unrelated salsa revision bumps. Without it,
//! salsa's accumulator-DFS pruning silently drops previously-collected
//! diagnostics whenever the query is validated-but-not-re-executed.

use super::*;
use crate::common::{EquationError, Error, UnitError};

/// `Debug` is derived rather than left off: every caller that drains this
/// accumulator wants to print the result in an assertion message, and without
/// it each one has to map through `.0.clone()` into the printable `Diagnostic`
/// first. The inner `Diagnostic` is already `Debug`, so the derive costs
/// nothing.
#[salsa::accumulator]
#[derive(Debug)]
pub struct CompilationDiagnostic(pub Diagnostic);

/// A single compilation diagnostic emitted by tracked functions.
/// Carries enough context (model name, optional variable name) for
/// downstream formatting without re-walking the model tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Copy)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Diagnostic {
    pub model: String,
    pub variable: Option<String>,
    pub error: DiagnosticError,
    pub severity: DiagnosticSeverity,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum DiagnosticError {
    Equation(EquationError),
    Model(Error),
    Unit(UnitError),
    Assembly(String),
}

/// Per-model tracked function that triggers diagnostic accumulation from
/// all compilation stages. The salsa accumulator is the sole error source
/// for diagnostic reporting -- this function does not read struct fields.
///
/// Triggers three diagnostic sources:
/// 1. `compile_var_fragment` for each variable -- accumulates parse-level
///    equation errors (EmptyEquation, syntax errors), unit definition
///    syntax errors (bad unit strings), and compilation-level errors
///    (BadTable, MismatchedDimensions, etc.)
/// 2. `check_model_units` -- accumulates unit inference/checking warnings
/// 3. When LTM is enabled, `model_ltm_fragment_diagnostics` -- accumulates
///    LTM assembly diagnostics: the auto-flip warning that surfaces when
///    the element-level largest SCC exceeds `MAX_LTM_SCC_NODES` (emitted
///    by `model_ltm_variables`, which the fragment-diagnostic pass drives
///    internally), and a compile-failure warning for any LTM synthetic
///    variable whose fragment fails to compile. Gated on `ltm_enabled` so
///    we don't run LTM synthesis on projects that never requested it.
/// 4. `emit_conveyor_spec_warnings` -- the unconditional compile-time
///    conveyor advisories: `ConveyorTransitNotDtMultiple` (a constant transit
///    time that is not an integer multiple of dt) and
///    `ConveyorLeakFractionsExceedOne` (constant linear leak fractions
///    summing above 1), docs/design/conveyors.md §4.1 / §5.1.
/// 4b. `emit_unknown_element_subscript_warnings` -- the unconditional
///    `UnknownElementSubscript` advisory: a non-apply-to-all arrayed
///    variable's `<element>` entry whose subscript names no declared
///    element combination is silently dropped everywhere downstream, so a
///    typo'd subscript simulates plausibly-but-wrong with no signal (GH #905).
/// 5. When LTM is enabled, `emit_conveyor_ltm_degraded_warnings` and
///    `emit_queue_ltm_degraded_warnings` -- one `Warning` per conveyor stock
///    and per queue stock in THIS model, because LTM's flow-to-stock link-score
///    formula assumes plain INTEG but both are non-INTEG stock types
///    (docs/design/conveyors.md §9.6, docs/design/queues.md §10.5). Emitted here
///    rather than inside `model_ltm_variables` so each fires exactly once even
///    for a module-referenced sub-model (see those functions' rustdoc for the
///    double-drain they avoid).
#[salsa::tracked]
pub fn model_all_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    // Force this query to re-execute on every revision rather than being
    // validated-but-skipped.
    //
    // The two `collect_*` helpers drain diagnostics via
    // `model_all_diagnostics::accumulated::<CompilationDiagnostic>(..)`. salsa
    // 0.26's `accumulated_by` does a DFS that prunes any dependency subtree
    // whose root memo's `accumulated_inputs` flag is `Empty`. That flag is set
    // to `Any` only while the query *executes* (when it reads a child whose
    // memo already holds accumulated values, e.g. `check_model_units`). When an
    // UNRELATED salsa input changes (a `SetLoopName` patch touching only
    // `SourceModel.pinned_loops`, a sim-spec edit, ...) the revision bumps but
    // none of this query's tracked inputs change, so salsa validates the memo
    // without re-executing it -- and the deep-verify path recomputes the
    // pruning flag from each input's `maybe_changed_after` result, which
    // reports `Empty` for a self-accumulating child (a memo's
    // `accumulated_inputs` reflects only its *inputs*, never whether the memo
    // itself accumulated). The flag collapses to `Empty`, the DFS prunes the
    // whole subtree, and the previously-collected diagnostics silently vanish
    // on the next collection (engine `test_diagnostics_stable_across_*`;
    // libsimlin saw `get_errors` zero out after an unrelated patch). The inner
    // memos still hold their accumulated maps, so re-executing this trigger --
    // a cheap O(num_vars) walk of already-memoized children -- is enough to
    // refresh the flag to `Any` and let the DFS descend. An untracked read
    // makes this query ineligible for shallow/deep validation, so it always
    // re-executes (salsa `Database::report_untracked_read`: "queries which
    // report untracked reads will be re-executed in the next revision").
    db.report_untracked_read();

    // Duplicate canonical variable idents (GH #885): two variables whose
    // names canonicalize to the same identifier silently collapse into one on
    // every canonical-keyed map downstream (last-in-document-order wins in
    // sync), so the simulated model would not be the model the user wrote.
    // Error severity -- and `compile_project_incremental` fails hard on the
    // same query -- because a silently-wrong simulation is a model-integrity
    // failure like a duplicate macro name, not a partial-result advisory.
    emit_duplicate_variable_diagnostics(db, model);

    let source_vars = model.variables(db);

    // Trigger compile_var_fragment for each variable. This is a superset
    // of parse_source_variable_with_module_context: it first accumulates
    // unit definition syntax errors from the parsed variable, then checks
    // for equation parse errors, then proceeds with compilation which can
    // surface additional errors like BadTable, MismatchedDimensions, etc.
    //
    // The symbolic fragment is role-independent (`time`/`dt` lower to
    // `LoadGlobalVar` at fixed slots, never through the layout), so this
    // diagnostic pass produces byte-identical fragments to assembly and the
    // two SHARE one salsa cache entry per variable -- the win from dropping
    // `is_root`. The module inputs are empty because we are not in an
    // assembly context: this is purely for error detection.
    let empty_inputs = ModuleInputSet::empty(db);
    for (_var_name, source_var) in source_vars.iter() {
        let _fragment = compile_var_fragment(db, *source_var, model, project, empty_inputs);
    }

    // Trigger unit checking. This is a separate tracked function so
    // that unit inference results are individually cached and
    // invalidated only when unit-relevant inputs change. It lives in the
    // `db::units` submodule (kept out of `db.rs` for the per-file line
    // cap).
    crate::db::units::check_model_units(db, model, project);

    // Validate each explicit module variable's input wiring (GH #806 sibling):
    // a reference whose `dst` names no input of the target model, or whose bare
    // `src` names no variable in this model, is silently dropped at assembly and
    // the port reads its default -- a quietly-wrong simulation. The salsa path
    // had lost the legacy `BadModuleInputDst`/`BadModuleInputSrc` check.
    model_module_wiring_diagnostics(db, model, project);

    // Conveyor compile-time spec advisories (docs/design/conveyors.md §4.1 /
    // §5.1): the DT-quantized-transit and constant-leak-fractions-sum
    // Warnings. Unconditional -- NOT inside the `ltm_enabled` gate below --
    // because they describe the simulation itself, not an analysis overlay.
    // Emitted from this per-model trigger for the same exactly-once reason as
    // the LTM-degraded twins, and because the special conveyor/queue build
    // path (`queue_compile::build_compiled`) returns a single hard `Err` with
    // no warnings channel: the salsa accumulator is the only route a
    // conveyor Warning can take to `collect_all_diagnostics` /
    // `simlin_project_get_errors` (GH #873).
    emit_conveyor_spec_warnings(db, model, project);

    // Unknown element subscripts on non-apply-to-all arrays (GH #905):
    // an `<element>` entry naming no declared element is silently dropped
    // by every downstream consumer; surface it as a Warning here so the
    // typo'd equation does not vanish without a trace. Unconditional, like
    // the conveyor spec advisories above.
    emit_unknown_element_subscript_warnings(db, model, project);

    // When LTM is enabled, also trigger the LTM diagnostic pass so that
    // diagnostics accumulated by the LTM pipeline surface through
    // `collect_all_diagnostics`: the auto-flip-to-discovery warning from
    // `model_ltm_variables` and the synthetic-fragment compile-failure
    // warning from `model_ltm_fragment_diagnostics`.
    // `model_ltm_fragment_diagnostics` drives `model_ltm_variables`
    // internally, so the auto-flip warning rides along. Without this
    // call the warnings are invisible even though the LTM pipeline
    // already emitted them. Gated on `ltm_enabled` so projects that never
    // requested LTM pay no LTM synthesis cost here. The diagnostic-
    // collection FFI path (`simlin_project_get_errors`) transiently
    // re-enables `ltm_enabled` for callers who created an LTM simulation,
    // so these warnings reach `simlin-mcp`/`libsimlin`/pysimlin (GH #466).
    if project.ltm_enabled(db) {
        model_ltm_fragment_diagnostics(db, model, project);
        emit_conveyor_ltm_degraded_warnings(db, model);
        emit_queue_ltm_degraded_warnings(db, model);
    }
}

/// Per-model duplicate-canonical-ident groups (GH #885): for each canonical
/// ident that more than one declared variable canonicalizes to, the canonical
/// form plus every as-written spelling in declaration order.
///
/// Derived from the raw, pre-dedup `SourceModel::declared_variable_idents`
/// input -- the canonical-keyed `variables` map cannot answer this, exactly
/// like `SourceProject::macro_declarations` vs the `models` map (see
/// `db::macro_registry`). Salsa-tracked so both consumers -- the diagnostic
/// emission below and `compile_project_incremental`'s hard error -- share one
/// memoized derivation that only invalidates when the declared-ident list
/// changes.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_duplicate_variables(
    db: &dyn Db,
    model: SourceModel,
) -> Vec<(String, Vec<String>)> {
    crate::common::duplicate_variable_groups(
        model
            .declared_variable_idents(db)
            .iter()
            .map(|s| s.as_str()),
    )
}

/// Emit one Error-severity `DuplicateVariable` diagnostic per colliding
/// canonical-ident group in `model`, naming every original spelling and the
/// model (GH #885). The message text is shared with the hard compile error
/// via `common::duplicate_variable_message`, so every surface (diagnostics,
/// `compile_project_incremental`, the special-stock build path) reports
/// identically.
fn emit_duplicate_variable_diagnostics(db: &dyn Db, model: SourceModel) {
    use crate::common::{Error, ErrorCode, ErrorKind};
    use salsa::Accumulator;

    let model_name = model.name(db);
    for (canonical, spellings) in model_duplicate_variables(db, model) {
        let msg = crate::common::duplicate_variable_message(model_name, canonical, spellings);
        CompilationDiagnostic(Diagnostic {
            model: model_name.clone(),
            variable: Some(canonical.clone()),
            error: DiagnosticError::Model(Error::new(
                ErrorKind::Model,
                ErrorCode::DuplicateVariable,
                Some(msg),
            )),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
    }
}

/// Shared emitter behind the conveyor/queue LTM-degraded advisories: one
/// `Warning` per stock in `model` whose `Compat` carries the owner's marker
/// (`has_marker`).
///
/// Both stock types have non-INTEG dynamics -- a conveyor's material rides a
/// fixed-length belt and exits after the transit time, a queue is a FIFO of
/// batches whose outflow is demand-driven -- so the change from t-1 to t is
/// NOT `dt * inflow(t-1)`. LTM's flow-to-stock link-score numerator
/// (`PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))`) assumes plain INTEG under
/// Euler, so any link or loop score touching such a stock would be silently
/// wrong. The salsa DIAGNOSTIC path never expands either stock type into its
/// hidden variables + native pass (only the special-stock build path
/// `queue_compile::build_vm` does, which CLEARS the marker), so the `Compat`
/// marker is still present here and the stock would be scored as plain INTEG.
/// Degrade LOUDLY rather than emit a silently-wrong score.
///
/// The callers live in `model_all_diagnostics` -- NOT inside
/// `model_ltm_variables` -- specifically because `model_all_diagnostics` is
/// drained exactly ONCE per model by `collect_all_diagnostics` and is never
/// invoked transitively across module edges. `model_ltm_variables(parent)`
/// reaches `model_ltm_variables(child)` through module-composite link scoring,
/// so a special stock in a module-referenced sub-model would have its
/// `model_ltm_variables` memo (with the accumulated warning) in BOTH the
/// parent's and the child's accumulator DFS -- reported twice (the
/// cross-module double-drain that #866 tracks for the `model_ltm_variables`
/// warnings). Emitting from the per-model trigger fires exactly once per stock
/// regardless of module nesting.
///
/// Only under LTM: the sole callers sit in `model_all_diagnostics`'s existing
/// `ltm_enabled` branch. Carried as a `Model` error with the owner's specific
/// `code` rather than `Assembly` (which `errors::format_diagnostic` surfaces
/// as the misleading `NotSimulatable` code) so this analysis-only advisory
/// never makes the project look non-simulatable. Names are sorted so multiple
/// stocks accumulate in a deterministic order.
///
/// `noun` (`conveyor`/`queue`), `dynamics_detail` (an optional parenthetical
/// after "non-INTEG dynamics"), and `doc_ref` shape the message per owner; the
/// wording is otherwise identical by construction.
fn emit_ltm_degraded_warnings(
    db: &dyn Db,
    model: SourceModel,
    has_marker: impl Fn(&crate::datamodel::Compat) -> bool,
    code: crate::common::ErrorCode,
    noun: &str,
    dynamics_detail: &str,
    doc_ref: &str,
) {
    use crate::common::{Error, ErrorKind};
    use salsa::Accumulator;

    let mut names: Vec<String> = model
        .variables(db)
        .values()
        .filter(|sv| has_marker(sv.compat(db)))
        .map(|sv| sv.ident(db).clone())
        .collect();
    names.sort_unstable();

    let model_name = model.name(db);
    for name in names {
        let msg = format!(
            "LTM (Loops That Matter) analysis over {noun} stock '{name}' is degraded: a {noun} \
             is a stock with non-INTEG dynamics{dynamics_detail}, but the flow-to-stock \
             link-score numerator `PREVIOUS(flow) - PREVIOUS(PREVIOUS(flow))` assumes plain \
             INTEG under Euler, so any link or loop score touching '{name}' may be wrong.  \
             Treat scores involving this {noun} as advisory ({doc_ref})."
        );
        CompilationDiagnostic(Diagnostic {
            model: model_name.clone(),
            variable: Some(name),
            error: DiagnosticError::Model(Error::new(ErrorKind::Model, code, Some(msg))),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    }
}

/// Emit one `ConveyorLtmDegraded` `Warning` per conveyor stock in `model`
/// (docs/design/conveyors.md §9.6). See [`emit_ltm_degraded_warnings`] for the
/// shared rationale (non-INTEG dynamics vs. LTM's INTEG assumption, and why
/// the emission site is the per-model `model_all_diagnostics` trigger).
fn emit_conveyor_ltm_degraded_warnings(db: &dyn Db, model: SourceModel) {
    emit_ltm_degraded_warnings(
        db,
        model,
        |c| c.conveyor.is_some(),
        crate::common::ErrorCode::ConveyorLtmDegraded,
        "conveyor",
        "",
        "docs/design/conveyors.md \u{00A7}9.6",
    );
}

/// Emit one `QueueLtmDegraded` `Warning` per queue stock in `model`
/// (docs/design/queues.md §10.5). See [`emit_ltm_degraded_warnings`] for the
/// shared rationale; the queue-specific nuance is only the FIFO wording.
fn emit_queue_ltm_degraded_warnings(db: &dyn Db, model: SourceModel) {
    emit_ltm_degraded_warnings(
        db,
        model,
        |c| c.queue.is_some(),
        crate::common::ErrorCode::QueueLtmDegraded,
        "queue",
        " (a FIFO of batches)",
        "docs/design/queues.md \u{00A7}10.5",
    );
}

/// Emit the two spec-mandated compile-time conveyor advisories for each
/// conveyor stock in `model`, Warning severity (docs/design/conveyors.md
/// §9.8 table; GH #873):
///
/// - [`ErrorCode::ConveyorTransitNotDtMultiple`] (§4.1): a compile-time-
///   constant transit time that is not an integer multiple of dt. The belt is
///   DT-quantized (`conveyor::slat_count` rounds half away from zero and
///   clamps to >= 1), so the message names the conveyor and reports the
///   effective transit time `slats * dt` the run will actually use. A
///   non-constant `<len>` expression gets no warning -- its value is only
///   known at runtime, where the latch path validates it separately
///   (`ConveyorTransitNotPositive` / `ConveyorTransitTooLong`).
/// - [`ErrorCode::ConveyorLeakFractionsExceedOne`] (§5.1): the conveyor's
///   compile-time-constant LINEAR leak fractions sum above 1 (the §5.1
///   constraint is `Σ f_k <= 1`; at runtime the step-2 content clamp
///   under-leaks the LATER flows, exactly the "later leakages may get less"
///   behavior isee documents). Each flow's fraction is resolved through the
///   shared `conveyor_compile::leak_fraction_source` -- an explicit `<leak>`
///   fraction wins, else the flow's own `<eqn>` carries it (the
///   bare-`<leak/>`-plus-`<eqn>` encoding real Stella files use, §3.3) -- so
///   the advisory sums exactly the fractions the runtime applies. Exponential
///   conveyors are skipped entirely: §5.2 rates are per-time-unit and
///   overlapping rates ADD by design, so a sum above 1 is legal there. Each
///   constant term is clamped to `[0, 1]` before summing, mirroring the
///   runtime's per-fraction `clamp_fraction` (a negative constant contributes
///   zero leakage at runtime, so it must not cancel other fractions out of
///   the sum; an over-1 constant caps at 1). Non-constant fractions are
///   excluded, which can never produce a false positive: they too clamp to
///   `>= 0` at runtime, so the summed constant subset is a lower bound on the
///   runtime total.
///
/// dt resolution mirrors `assemble_simulation`'s root rule: THIS model's
/// `model_sim_specs` override wins when present, else the project sim specs.
/// A non-positive/non-finite dt emits nothing (invalid sim specs are another
/// diagnostic's concern, and `transit_dt_mismatch` guards the domain).
///
/// Unconditional -- unlike the LTM-degraded twins above, these advisories
/// describe the simulation itself. Emitted from the `model_all_diagnostics`
/// per-model trigger (drained exactly once per model, never invoked across
/// module edges) because the special conveyor/queue build path
/// (`queue_compile::build_compiled`) returns a single hard `Err` with no
/// warnings channel: this accumulator is the only route to
/// `collect_all_diagnostics` / `simlin_project_get_errors`. Carried as a
/// `Model` error with the specific code (not `Assembly`) so the advisory
/// never surfaces as a misleading `NotSimulatable`; conveyors are visited in
/// sorted-name order for deterministic accumulation.
fn emit_conveyor_spec_warnings(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use crate::common::{Error, ErrorCode, ErrorKind};
    use crate::conveyor_compile::{
        LEAK_FRACTION_SUM_TOLERANCE, LeakFractionSource, clamp_fraction, const_scalar_expr,
        leak_fraction_source, transit_dt_mismatch,
    };
    use salsa::Accumulator;

    let source_vars = model.variables(db);
    let mut conveyors: Vec<&SourceVariable> = source_vars
        .values()
        .filter(|sv| sv.compat(db).conveyor.is_some())
        .collect();
    if conveyors.is_empty() {
        return;
    }
    conveyors.sort_unstable_by_key(|sv| sv.ident(db));

    // Per-model dt: THIS model's sim_specs override, else the project's. This
    // matches `assemble_simulation`'s rule when this model is the simulated
    // ROOT; for a module-instantiated SUBMODEL the executed dt would be the
    // root's, which can differ from this per-model resolution -- but a
    // conveyor in a submodel is rejected at sim time
    // (`ConveyorInSubmodelUnsupported`, compile_project_incremental), so no
    // runnable configuration can disagree with the dt used here today.
    let dt = {
        let dt_spec = if let Some(ref model_specs) = *model.model_sim_specs(db) {
            model_specs.dt.clone()
        } else {
            project.sim_specs(db).dt.clone()
        };
        match dt_spec {
            crate::datamodel::Dt::Dt(v) => v,
            crate::datamodel::Dt::Reciprocal(v) => 1.0 / v,
        }
    };

    let model_name = model.name(db);
    let emit = |stock: &str, code: ErrorCode, msg: String| {
        CompilationDiagnostic(Diagnostic {
            model: model_name.clone(),
            variable: Some(stock.to_string()),
            error: DiagnosticError::Model(Error::new(ErrorKind::Model, code, Some(msg))),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    };

    for svar in conveyors {
        let compat = svar.compat(db);
        let Some(conv) = &compat.conveyor else {
            continue;
        };
        let stock_name = svar.ident(db);

        // §4.1: constant transit time that is not an integer multiple of dt.
        if let Some(transit) = const_scalar_expr(&conv.transit_time)
            && let Some((slats, effective)) = transit_dt_mismatch(transit, dt)
        {
            // For a transit within ~5e-5 of dt (or of the effective transit)
            // the trimmed display renders the values as the SAME string --
            // "transit time 0.3333 is not an integer multiple of dt 0.3333"
            // is self-contradictory. Fall back to the full round-trip form
            // for all three so the reader can see why the warning fired
            // (shortest-round-trip display is injective on doubles, and
            // transit == dt exactly can never warn, so full forms always
            // disambiguate).
            let (t_str, dt_str, eff_str) = {
                let t = fmt_diag_value(transit);
                let d = fmt_diag_value(dt);
                let e = fmt_diag_value(effective);
                if t == d || t == e {
                    (
                        format!("{transit}"),
                        format!("{dt}"),
                        format!("{effective}"),
                    )
                } else {
                    (t, d, e)
                }
            };
            let slat_word = if slats == 1 { "slat" } else { "slats" };
            emit(
                stock_name,
                ErrorCode::ConveyorTransitNotDtMultiple,
                format!(
                    "conveyor '{stock_name}' transit time {t_str} is not an integer \
                     multiple of dt {dt_str}: the belt is quantized to {slats} \
                     {slat_word}, an effective transit time of {eff_str} \
                     (docs/design/conveyors.md \u{00A7}4.1)"
                ),
            );
        }

        // §5.1: constant linear leak fractions summing above 1.
        if !conv.exponential_leak {
            let sum: f64 = svar
                .outflows(db)
                .iter()
                .filter_map(|out_name| {
                    // Outflow entries carry the display form; the variables
                    // map is keyed canonically.
                    let canon = crate::canonicalize(out_name);
                    let flow = source_vars.get(canon.as_ref())?;
                    // Resolve which expression carries this flow's fraction
                    // through the SAME helper the runtime expansion uses
                    // (explicit `<leak>` fraction, else the flow's own
                    // `<eqn>` -- the encoding real Stella files use, §3.3).
                    // A truly bare marker (Absent) leaks nothing; an
                    // `Arrayed` per-element fraction has no single scalar
                    // expression and is excluded like any non-constant.
                    let expr = match leak_fraction_source(
                        Some(flow.compat(db).leakage.as_ref()?),
                        flow.equation(db),
                    ) {
                        LeakFractionSource::Explicit(e) => e,
                        LeakFractionSource::FlowEquation(
                            crate::datamodel::Equation::Scalar(s)
                            | crate::datamodel::Equation::ApplyToAll(_, s),
                        ) => s,
                        _ => return None,
                    };
                    // Apply the runtime's OWN per-fraction hygiene to each
                    // constant term (`clamp_fraction`, linear arm): clamp to
                    // [0, 1] -- a negative constant contributes zero leakage,
                    // so it must not cancel other fractions out of the sum --
                    // and NaN maps to 0, because `f64::clamp` PROPAGATES NaN
                    // and a literal `nan` fraction would otherwise poison the
                    // whole sum into silence while the runtime zeroes it and
                    // leaks the rest at full rate.
                    const_scalar_expr(expr).map(|v| clamp_fraction(v, false))
                })
                .sum();
            if sum > 1.0 + LEAK_FRACTION_SUM_TOLERANCE {
                emit(
                    stock_name,
                    ErrorCode::ConveyorLeakFractionsExceedOne,
                    format!(
                        "conveyor '{stock_name}' constant linear leak fractions sum to {}, \
                         exceeding 1: later leak flows will receive less than their declared \
                         fraction, and the primary outflow may be fully starved \
                         (docs/design/conveyors.md \u{00A7}5.1)",
                        fmt_diag_value(sum),
                    ),
                );
            }
        }
    }
}

/// Emit one Warning-severity [`crate::common::ErrorCode::UnknownElementSubscript`]
/// diagnostic per distinct non-apply-to-all `<element>` entry in `model` whose
/// subscript names no declared element combination of the variable's
/// dimensions (GH #905).
///
/// Every consumer of a per-element equation list matches entries against the
/// declared elements by canonical key and SILENTLY DROPS an unmatched entry:
/// the compiler's arrayed expansion (`compiler::expand_arrayed_with_hoisting`
/// falls back to the default equation or a constant 0 for a combination with
/// no entry, and never visits an entry matching no combination), the
/// per-element graphical-function table layout
/// (`variable::reorder_arrayed_element_tables` leaves an empty placeholder
/// table), and the conveyor per-element init lists
/// (`conveyor_compile::expand_conveyors` steady-fills the belt from 0). A
/// one-character typo therefore produces a plausible-looking but wrong
/// simulation with no signal anywhere.
///
/// An entry is flagged only when BOTH production matching rules fail, i.e.
/// this check accepts the UNION of what any consumer accepts:
///
/// 1. **Per-part** (the conveyor init-list matcher,
///    [`crate::conveyor_compile::canonical_subscript_key`] semantics): split
///    on `,`, canonicalize each part, and resolve it against the
///    corresponding dimension via `Dimension::get_offset` (named elements by
///    canonical name; indexed dimensions by parsed `1..=size`). This accepts
///    whitespace-around-comma variants ("a1, b1") that rule 2 mangles.
/// 2. **Whole-string** (the plain compiler's rule -- `variable.rs`
///    `parse_equation` keys entries by
///    `CanonicalElementName::from_raw(subscript)` and
///    `compiler::expand_arrayed_with_hoisting` looks up
///    `from_raw(combination.join(","))` -- and equally the per-element
///    GF-table layout, `variable::build_tables` /
///    `reorder_arrayed_element_tables`): the entry's whole canonicalized
///    subscript equals some declared combination's comma-joined key. This
///    accepts comma-CONTAINING element names (`board{"a,b", "c"}`, entry
///    `a,b`) and quoted whole subscripts (`"a1,b1"`, whose balanced quotes
///    `canonicalize` strips) that rule 1 mis-splits -- entries the compiler
///    genuinely resolves and simulates.
///
/// So an entry that ANY consumer resolves is never flagged; when this warns,
/// every consumer really does drop the entry and "the equation is ignored"
/// is accurate. (The rule-1-vs-rule-2 split is a pre-existing inconsistency
/// between the conveyor path and the compiler/GF paths; XMILE/MDL-sourced
/// keys are normalized per-part by their readers, so the divergent spellings
/// are only reachable from API-built datamodels.)
///
/// Indexed-dimension nuance: rule 1's `get_offset` PARSES the part as an
/// integer, so alternate numeric spellings like `"01"` are accepted here
/// while every consumer matches the exact `SubscriptIterator` string key
/// (`"1"`) and silently drops them. On indexed dimensions this diagnostic is
/// therefore deliberately MORE forgiving than all consumers -- an `"01"`
/// entry is silently dropped and NOT warned. That is the accepted lenient
/// direction: a missed warning (false negative), never a false claim that a
/// used equation is ignored.
///
/// Warning -- not Error -- severity, deliberately: the vendored corpus
/// contains real imported models that currently rely on the silent drop
/// (e.g. `test/metasd/beer-game/RealBeer4-Sterman13.mdl`, where the MDL
/// importer's synthesized net-flow variable for a stock defined piecewise
/// over subranges carries parent-range element entries against
/// subrange-typed dimensions), so an Error would newly reject previously
/// loadable projects. The complementary GH #905 shape -- a DECLARED element
/// with no entry and no default, which silently evaluates to 0 -- remains
/// undiagnosed here.
///
/// An unresolvable dimension NAME is skipped entirely: the equation already
/// surfaces `BadDimensionName` through `compile_var_fragment`, and cascading
/// one warning per entry on top of it would be noise. Variables are visited
/// in sorted-name order and duplicate unknown keys deduplicated so
/// accumulation is deterministic.
fn emit_unknown_element_subscript_warnings(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) {
    use crate::common::{CanonicalElementName, Error, ErrorCode, ErrorKind};
    use crate::dimensions::Dimension;
    use salsa::Accumulator;
    use std::collections::HashSet;

    let source_vars = model.variables(db);
    let mut var_names: Vec<&String> = source_vars.keys().collect();
    var_names.sort_unstable();

    let project_dims = project.dimensions(db);
    let model_name = model.name(db);

    for var_name in var_names {
        let svar = &source_vars[var_name];
        let crate::datamodel::Equation::Arrayed(dim_names, elements, _, _) = svar.equation(db)
        else {
            continue;
        };
        if dim_names.is_empty() || elements.is_empty() {
            continue;
        }

        // Resolve the variable's dimension names against the project's
        // dimension definitions by canonical name (the same rule as
        // `variable::get_dimensions`). Any unresolved name means the
        // equation already carries BadDimensionName -- skip.
        let dims: Option<Vec<Dimension>> = dim_names
            .iter()
            .map(|name| {
                let canonical = crate::canonicalize(name);
                project_dims
                    .iter()
                    .find(|d| crate::canonicalize(d.name()) == canonical)
                    .map(Dimension::from)
            })
            .collect();
        let Some(dims) = dims else {
            continue;
        };

        // Rule-2 declared-combination key set (see the rustdoc), materialized
        // lazily: only a variable with at least one rule-1 failure pays the
        // cross-product cost, and the compiler expands the same product for
        // every arrayed variable anyway.
        let mut whole_string_keys: Option<HashSet<CanonicalElementName>> = None;

        let mut warned: HashSet<String> = HashSet::new();
        for (subscript, _, _, _) in elements {
            // Rule 1 (per-part, the conveyor matcher): membership in the
            // cross product of the dimensions' elements is per-position
            // membership, so no combination set needs materializing.
            let parts: Vec<CanonicalElementName> = subscript
                .split(',')
                .map(CanonicalElementName::from_raw)
                .collect();
            let per_part_matches = parts.len() == dims.len()
                && parts
                    .iter()
                    .zip(dims.iter())
                    .all(|(part, dim)| dim.get_offset(part).is_some());
            if per_part_matches {
                continue;
            }
            // Rule 2 (whole-string, the compiler/GF matcher): the entry's
            // whole canonicalized subscript equals some declared
            // combination's comma-joined key -- exactly how
            // `compiler::expand_arrayed_with_hoisting` resolves entries, so
            // e.g. a comma-containing element name or a quoted whole
            // subscript that rule 1 mis-splits is recognized as resolved.
            let whole_string_keys = whole_string_keys.get_or_insert_with(|| {
                crate::dimensions::SubscriptIterator::new(&dims)
                    .map(|combination| CanonicalElementName::from_raw(&combination.join(",")))
                    .collect()
            });
            if whole_string_keys.contains(&CanonicalElementName::from_raw(subscript)) {
                continue;
            }
            let canonical_key = crate::conveyor_compile::canonical_subscript_key(subscript);
            if !warned.insert(canonical_key) {
                continue;
            }
            let dims_display = dim_names.join(", ");
            let msg = format!(
                "array variable '{var_name}' has an equation for element '{subscript}', but \
                 '{subscript}' does not name an element of its dimension(s) [{dims_display}]; \
                 the equation is ignored, and any declared element without its own equation \
                 silently evaluates to 0"
            );
            CompilationDiagnostic(Diagnostic {
                model: model_name.clone(),
                variable: Some(var_name.clone()),
                error: DiagnosticError::Model(Error::new(
                    ErrorKind::Model,
                    ErrorCode::UnknownElementSubscript,
                    Some(msg),
                )),
                severity: DiagnosticSeverity::Warning,
            })
            .accumulate(db);
        }
    }
}

/// Format an f64 for an advisory message: up to 4 decimal places with
/// trailing zeros (and a bare trailing dot) trimmed, so an accumulated-sum
/// artifact like `0.7 + 0.2 + 0.4 = 1.2999999999999998` reads as "1.3"
/// instead of the shortest-round-trip tail. Falls back to the full `{}` form
/// when 4 decimals would materially distort the value (relative error above
/// 1e-3, e.g. a tiny transit like 1e-5 must not display as "0"). Display
/// only -- the untrimmed value never feeds back into computation.
fn fmt_diag_value(v: f64) -> String {
    let fixed = format!("{v:.4}");
    let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
    match trimmed.parse::<f64>() {
        Ok(r) if (r - v).abs() <= 1e-3 * v.abs() => trimmed.to_string(),
        _ => format!("{v}"),
    }
}

/// Validate the input wiring of each explicit module variable in `model`.
///
/// A module reference is `{ src, dst }` where `dst` is the module-qualified
/// `{module}·{port}` form naming an input of the target model and `src` is a
/// variable in the enclosing model. `build_module_inputs` SILENTLY DROPS a
/// reference whose `dst` does not match an existing child input -- the port then
/// reads its default and the simulation is quietly wrong, with no error. The
/// legacy monolithic path returned `BadModuleInputDst`/`BadModuleInputSrc` here;
/// the salsa path dropped the check. Re-add it as a Warning (partial-result
/// philosophy: a mis-wired input should not block the rest of the model).
///
/// Validated conservatively to avoid false positives:
/// - empty placeholder endpoints (the new-row UI pattern) are skipped;
/// - only an EXISTING target model is checked (an empty / dangling `model_name`
///   is a separate concern and the empty name is the normal freshly-drawn state);
/// - a `src` is checked only when it is a bare ident (no `·`) and not an engine
///   synthetic (`$⁚…`) -- a qualified cross-module output or temporary is left
///   to the equation checker.
#[salsa::tracked]
pub fn model_module_wiring_diagnostics(db: &dyn Db, model: SourceModel, project: SourceProject) {
    use salsa::Accumulator;

    let source_vars = model.variables(db);
    let project_models = project.models(db);
    let model_name = model.name(db);

    let mut module_names: Vec<&String> = source_vars
        .iter()
        .filter(|(_, sv)| sv.kind(db) == SourceVariableKind::Module)
        .map(|(name, _)| name)
        .collect();
    module_names.sort_unstable();

    let emit = |code: crate::common::ErrorCode, message: String| {
        CompilationDiagnostic(Diagnostic {
            model: model_name.clone(),
            variable: None,
            error: DiagnosticError::Model(Error::new(
                crate::common::ErrorKind::Model,
                code,
                Some(message),
            )),
            severity: DiagnosticSeverity::Warning,
        })
        .accumulate(db);
    };

    for module_name in module_names {
        let svar = &source_vars[module_name];
        let child_canonical = crate::canonicalize(svar.model_name(db));
        let Some(child_model) = project_models.get(child_canonical.as_ref()) else {
            continue;
        };
        let child_vars = child_model.variables(db);
        let prefix = format!("{module_name}\u{00B7}");

        for reference in svar.module_refs(db).iter() {
            let dst = crate::canonicalize(&reference.dst);
            if !dst.is_empty() {
                let resolves = dst
                    .strip_prefix(prefix.as_str())
                    .is_some_and(|port| child_vars.contains_key(port));
                if !resolves {
                    emit(
                        crate::common::ErrorCode::BadModuleInputDst,
                        format!(
                            "module '{module_name}' input wiring target '{}' does not name an input of model '{}'",
                            reference.dst, child_canonical
                        ),
                    );
                }
            }

            let src = crate::canonicalize(&reference.src);
            if !src.is_empty()
                && !src.contains('\u{00B7}')
                && !src.starts_with("$\u{205A}")
                && !source_vars.contains_key(src.as_ref())
            {
                emit(
                    crate::common::ErrorCode::BadModuleInputSrc,
                    format!(
                        "module '{module_name}' input source '{}' does not name a variable in model '{model_name}'",
                        reference.src
                    ),
                );
            }
        }
    }
}

/// Collect all `CompilationDiagnostic`s accumulated during
/// `model_all_diagnostics` for a single model.
pub fn collect_model_diagnostics(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<Diagnostic> {
    model_all_diagnostics::accumulated::<CompilationDiagnostic>(db, model, project)
        .into_iter()
        .map(|cd| cd.0.clone())
        .collect()
}

/// Collect all diagnostics for every model in a synced project.
///
/// Project-level failures are emitted here, directly from their memoized
/// derivation, rather than accumulated by whichever tracked query happens to
/// derive them. There is exactly one of each per project, so a per-model
/// accumulator drain would report N copies -- and a value accumulated inside a
/// memo body silently disappears once salsa's accumulator DFS prunes the
/// subtree it lives in (see `db::macro_registry`, and the module-level note
/// above `unit_warning_fixture` in `db/diagnostic_tests.rs`).
pub fn collect_all_diagnostics(db: &SimlinDb, project: SourceProject) -> Vec<Diagnostic> {
    let graph = project_module_graph(db, project);

    let mut all = Vec::new();

    // A unit declaration that failed to parse belongs to the project's `units`
    // list, not to any model. Read from the memoized derivation rather than
    // drained from the accumulator, for the reasons on `UnitsContextResult`.
    for (unit_name, eq_errors) in &project_units_context_result(db, project).definition_errors {
        for eq_err in eq_errors {
            all.push(Diagnostic {
                model: String::new(),
                variable: Some(unit_name.clone()),
                error: DiagnosticError::Unit(UnitError::DefinitionError(eq_err.clone(), None)),
                severity: DiagnosticSeverity::Error,
            });
        }
    }

    // An invalid macro set (recursion cycle / duplicate macro name / macro-model
    // collision) is a single project-level fact, carried as a memoized value on
    // `project_macro_registry` and read here rather than accumulated there.
    // Emitted before the per-model passes so the ordering is deterministic.
    if let Some((code, message)) = crate::db::macro_registry::project_macro_registry(db, project)
        .build_error
        .as_ref()
    {
        all.push(Diagnostic {
            model: String::new(),
            variable: None,
            error: DiagnosticError::Model(Error::new(
                crate::common::ErrorKind::Model,
                *code,
                Some(message.clone()),
            )),
            severity: DiagnosticSeverity::Error,
        });
    }

    for (name, source_model) in project.models(db) {
        // A model that can REACH a module cycle would drive its per-model passes
        // (compile_var_fragment recursing through the submodel) into the salsa
        // cycle panic. Report the cycle for that model and skip its passes. A
        // model that reaches no cycle is processed normally, so a valid model's
        // diagnostics are not hidden by an unrelated draft cycle elsewhere
        // (GH #806).
        if let Some((code, message)) = graph.cycle_error_from(name) {
            all.push(Diagnostic {
                model: name.clone(),
                variable: None,
                error: DiagnosticError::Model(Error::new(
                    crate::common::ErrorKind::Model,
                    code,
                    Some(message),
                )),
                severity: DiagnosticSeverity::Error,
            });
            continue;
        }
        all.extend(collect_model_diagnostics(db, *source_model, project));
    }
    all
}
