// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-variable fragment emission for explicit variables
//! (`compile_var_fragment`) and for the implicit helpers a parse synthesizes
//! (`compile_implicit_var_fragment`), plus the implicit-helper constructor of
//! `compiler::fragment::FragmentInput` (`implicit_fragment_input`).
//!
//! Both emitters are one shape: build the variable's `FragmentInput` (the
//! explicit constructor lives in the sibling `db/var_fragment.rs`), lower each
//! runlist-gated phase with `compiler::fragment::lower_fragment`, and emit it
//! through the single emission tail `db::assemble::
//! compile_phase_to_per_var_bytecodes` under `FragmentInput::emit_ctx`. The
//! LTM emitters in `db/ltm/compile.rs` are the same shape over their own two
//! constructors.

use std::collections::{BTreeSet, HashMap};

use salsa::Accumulator;

use super::*;
use crate::common::{Canonical, Ident, IdentMap};
use crate::compiler::fragment::{DepShape, FragmentInput, lower_fragment};
use crate::db::var_fragment::{
    ExplicitFragment, dep_head, explicit_fragment_input, is_implicit_global, model_dep_shape,
};

// Test-only per-thread record of which fragment-compiler bodies ran.
//
// Pointer equality of a memo does NOT prove a query body did not run: salsa
// backdates a re-executed query whose value compares equal and keeps the memo
// address (the trap `db::stages` documents at length). For the fragment
// compilers that matters even more than it did there, because a fragment is
// *designed* to be layout-independent -- so a layout-only edit produces an
// EQUAL fragment whether or not the expensive compile re-ran, and every
// pointer-based test passes either way. Recording each body entry, with the
// name it ran for, is the only evidence that separates "reused the memo" from
// "recompiled it and found it equal".
//
// Names, not just counts: the acceptance criterion under test is per-variable
// ("an *unchanged* fragment is reused"), so an aggregate count cannot say
// whether the one re-execution was the edited variable or an unrelated one.
//
// Thread-local rather than a global atomic, for the same reasons as
// `db::stages`: no lock, and a parallel test run cannot charge one test's
// work to another. The same caveat applies -- the record happens INSIDE the
// body, on whatever thread salsa ran it, so anyone introducing query
// parallelism here must move this to a shared atomic in the same change.
// `reset_fragment_executions()` at the start of a measured region is what
// isolates a test under `--test-threads=1`, where every test shares a thread.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FragmentExecKind {
    /// `compile_var_fragment` -- salsa-tracked, one cache entry per
    /// `(variable, model, project, module inputs)`.
    Explicit,
    /// `compile_implicit_var_fragment` -- salsa-tracked, one cache entry per
    /// `(model, project, helper name, module inputs)`.
    Implicit,
    /// `compile_ltm_var_fragment` -- salsa-tracked, keyed by `(from, to)` link.
    Ltm,
    /// `compile_ltm_equation_fragment` -- the LTM fragment-compile BODY,
    /// recorded wherever it runs. Every LTM path funnels through it (the
    /// `(from, to)`-keyed one and the per-index `compile_ltm_fragment_at` one),
    /// so this counts real compiles rather than cache lookups -- which is what
    /// makes "the diagnostic pass reuses assembly's work" measurable at all.
    LtmBody,
}

#[cfg(test)]
thread_local! {
    /// `None` = not recording. Recording is armed by `reset_fragment_executions`
    /// and disarmed by `fragment_executions`, so that the ~5k tests that never
    /// measure anything pay nothing and cannot accumulate an unbounded log on a
    /// libtest worker thread they happen to share with a measuring test.
    static FRAGMENT_EXECUTIONS: std::cell::RefCell<Option<Vec<(FragmentExecKind, String)>>> =
        const { std::cell::RefCell::new(None) };
}

/// Start (or restart) recording on this thread, discarding anything already
/// recorded (test-only). Call it after the fixture is built and primed, so
/// setup work is not charged to the measured region.
#[cfg(test)]
pub(crate) fn reset_fragment_executions() {
    FRAGMENT_EXECUTIONS.with(|c| *c.borrow_mut() = Some(Vec::new()));
}

/// Stop recording and return every body entry since the last reset, sorted so
/// the result is comparable regardless of the order assembly happened to walk
/// its maps in (test-only). Panics if recording was never armed, since an empty
/// answer from an unarmed recorder would read as "nothing recompiled".
#[cfg(test)]
pub(crate) fn fragment_executions() -> Vec<(FragmentExecKind, String)> {
    let mut execs = FRAGMENT_EXECUTIONS
        .with(|c| c.borrow_mut().take())
        .expect("fragment_executions() without a preceding reset_fragment_executions()");
    execs.sort();
    execs
}

/// Record one fragment-compiler body entry, if this thread is recording
/// (test-only).
#[cfg(test)]
pub(crate) fn note_fragment_execution(kind: FragmentExecKind, name: &str) {
    FRAGMENT_EXECUTIONS.with(|c| {
        if let Some(log) = c.borrow_mut().as_mut() {
            log.push((kind, name.to_string()));
        }
    });
}

#[salsa::tracked(returns(ref))]
pub fn compile_var_fragment<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    module_inputs: ModuleInputSet<'db>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{CompiledVarFragment, PerVarBytecodes};

    #[cfg(test)]
    note_fragment_execution(FragmentExecKind::Explicit, var.ident(db));

    let var_ident = var.ident(db).clone();
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);

    // The interned input set stores the sorted canonical names; the constructor
    // takes them as a slice.
    let module_input_names = module_inputs.names(db);

    let (unit_diags, input) =
        match explicit_fragment_input(db, var, model, project, module_input_names) {
            ExplicitFragment::Fatal {
                unit_diags,
                fatal_diags,
            } => {
                // Non-fatal unit diagnostics were recorded before the fatal
                // site; replay them first to preserve emission order, then
                // the fatal diagnostic(s), then bail out (whole-variable None).
                for diag in unit_diags.into_iter().chain(fatal_diags) {
                    CompilationDiagnostic(diag).accumulate(db);
                }
                return None;
            }
            ExplicitFragment::Ready { unit_diags, input } => (unit_diags, input),
        };

    // Malformed-unit diagnostics are non-fatal: record them and continue.
    for diag in unit_diags {
        CompilationDiagnostic(diag).accumulate(db);
    }

    // Which runlists this variable belongs to, read through the three-bit
    // projection rather than the whole `ModelDepGraphResult`: the projection
    // backdates when this variable's membership is unchanged, so an unrelated
    // variable being added, deleted or renamed does not invalidate every
    // fragment in the model (GH #964).
    let membership = var_runlist_membership(db, var, model, project, module_inputs);
    let is_stock = var.kind(db) == SourceVariableKind::Stock;
    let is_module = var.kind(db) == SourceVariableKind::Module;
    let is_module_input = input.module_inputs.contains(&var_ident_canonical);

    // Emit one phase, and make a CODEGEN rejection attributable.
    //
    // Lowering failures accumulate through `accumulate_var_compile_error`
    // below; a codegen `Err` goes through the reporting twin of the shared
    // emission tail so the refused construct can be named. Without that the
    // variable keeps its layout slot with no bytecode and the only signal is
    // `assemble_module`'s batch `failed to compile fragments for variables:
    // <names>` -- no reason, no severity, and nothing in
    // `collect_all_diagnostics` at all. That is the failure mode
    // `compiler::check_stock_updates_are_emittable`'s rustdoc describes, and
    // it is live for every codegen rejection: an ordinary hand-written
    // `out[d] = VECTOR SORT ORDER(PREVIOUS(vals[d]), 1)` would report zero
    // diagnostics while failing the build.
    //
    // The reason rides `DiagnosticError::Assembly`, not `Equation`, because a
    // codegen refusal has neither a span nor an `ErrorCode` of its own: the
    // emitter reports it as prose. `Assembly` is the variant whose payload IS
    // that prose.
    //
    // The variable name is embedded in that String even though the
    // `Diagnostic` also carries it in `variable`. Structured consumers read
    // the field, but `errors.rs`'s `Assembly` arm formats `message` as
    // "assembly {severity} in model '{model}': {msg}" and does NOT interpolate
    // `diag.variable` -- and the CLI prints only `message`. So on the surface a
    // user actually reads, the field alone names nothing; a bare reason would
    // leave the variable identified only by `assemble_module`'s separate batch
    // line. The LTM leg solves it the same way, by putting the name in the
    // string (`db/ltm/compile.rs`'s "LTM synthetic variable '{}' failed to
    // compile").
    //
    // An EMPTY phase is not a failure and is filtered before asking: a
    // standalone lookup-only table lowers to zero expressions by design
    // (`compiler::Var::new`'s `is_table_only` arm), so reporting the
    // reporting form's "nothing to emit" arm would turn a static table into
    // an error. `var_runlist_membership` already keeps such a variable out of
    // every runlist, so this is defense rather than a live filter.
    //
    // Severity is `Error`, unlike the LTM leg's `Warning`, and the asymmetry
    // is the difference between the two situations: a dropped LTM fragment
    // degrades an analysis overlay while the model still simulates, whereas a
    // dropped ORDINARY fragment means `assemble_module` fails the build. The
    // blast radius was measured rather than argued: a sweep diffing every
    // `collect_all_diagnostics` row across the whole `test/` corpus, with and
    // without this hunk, found the added rows land ONLY on projects that
    // already fail to compile -- no project that compiles gains an error.
    // That shape is what the severity rests on, and unlike a row count it
    // stays true as the corpus grows. (`DiagnosticError::Assembly` maps to
    // `FormattedErrorKind::Simulation`, which `FormattedErrors::push` does not
    // count toward `has_model_errors`/`has_variable_errors`, so the CLI's
    // redundant-`NotSimulatable` suppression is unaffected either way.)
    let emit_ctx = input.emit_ctx();
    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        if exprs.is_empty() {
            return None;
        }
        match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(&emit_ctx, exprs) {
            Ok(bytecodes) => Some(bytecodes),
            Err(reason) => {
                let ident = var.ident(db);
                CompilationDiagnostic(Diagnostic {
                    model: model.name(db).clone(),
                    variable: Some(ident.clone()),
                    error: DiagnosticError::Assembly(format!(
                        "variable '{ident}' failed to compile: {reason}"
                    )),
                    severity: DiagnosticSeverity::Error,
                })
                .accumulate(db);
                None
            }
        }
    };

    // Accumulate a diagnostic when a phase's lowering (`lower_fragment`)
    // fails. Without this, errors like DoesNotExist (unknown dependency)
    // are silently dropped and never appear in collect_all_diagnostics.
    //
    // `Equation`, not `Model`: `errors.rs` treats the two variants differently
    // on purpose. The `Equation` arm produces `FormattedErrorKind::Variable`,
    // names the variable in the summary, and -- via
    // `format_diagnostic_with_datamodel` -> `format_equation_error` -- enriches
    // the message with a source snippet from the equation text, where the
    // `Model` arm drops the variable from the summary and gets no snippet
    // (pinned by
    // `db::diagnostic_tests::test_compile_var_fragment_per_phase_var_new_failure`).
    // `EquationError::from` carries `err.details`, so the reason the lowering
    // wrote -- the stock named by `compiler::check_stock_updates_are_emittable`,
    // the identifier a `MismatchedDimensions` could not shape -- rides along
    // with it. The error has no span, so the conversion leaves `0..0`.
    let accumulate_var_compile_error = |err: &crate::Error| {
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var.ident(db).clone()),
            error: DiagnosticError::Equation(err.clone().into()),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
    };

    // Only the phases the variable's runlist membership admits are lowered; a
    // phase it is not a member of is never compiled, so its lowering outcome
    // is not a diagnostic either.
    let emit = |lowered: &Result<crate::compiler::Var, crate::Error>| match lowered {
        Ok(var) => compile_phase(&var.ast),
        Err(err) => {
            accumulate_var_compile_error(err);
            None
        }
    };

    // Initial phase: stocks and their deps get compiled with is_initial=true
    let initial_bytecodes = if membership.initials {
        emit(&lower_fragment(&input, true))
    } else {
        None
    };

    // Flow phase: non-stock vars AND stock-typed module inputs get compiled
    // with is_initial=false. Stock-typed module inputs need LoadModuleInput ->
    // AssignCurr in the flows phase to propagate the parent-provided value
    // each timestep. Stock phase: stocks and modules get compiled with
    // is_initial=false. One non-initial lowering serves both phases (a module
    // variable is a member of each).
    let in_flows_runlist = (!is_stock || is_module_input) && membership.flows;
    let in_stocks_runlist = (is_stock || is_module) && membership.stocks;
    let noninitial = (in_flows_runlist || in_stocks_runlist).then(|| lower_fragment(&input, false));

    let flow_bytecodes = match &noninitial {
        Some(lowered) if in_flows_runlist => emit(lowered),
        _ => None,
    };

    // Pre-compute the compiler-local flow verdict for
    // `model_flows_invariant` (GH #712). The salsa-cached result avoids a
    // second lowering; source `DepRef`s supply transitive identity separately.
    // Only meaningful for vars in the flows runlist.
    let flow_locally_invariant = match &noninitial {
        Some(lowered) if in_flows_runlist => {
            crate::db::assemble::flow_is_locally_invariant(lowered)
        }
        _ => None,
    };

    let stock_bytecodes = match &noninitial {
        Some(lowered) if in_stocks_runlist => emit(lowered),
        _ => None,
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: var_ident,
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
        flow_locally_invariant,
    })
}

/// The genuinely-shared prefix of synthetic-helper sourcing: resolve a
/// model's implicit variable from its parent's `implicit_vars`, build its
/// parse-stage form, and lower it to a `crate::variable::Variable`.
///
/// This is the *single shared relation* for "given an `ImplicitVarMeta`,
/// produce the helper's parsed + lowered form": the chain `parent → the
/// parse's helper NAMED by the metadata → its parse-stage variable →
/// lower_variable` (the non-module branch builds via `lower_variable`; the
/// module branch is the instance's wiring, since a module has no equation).
/// It is consumed by
/// [`implicit_fragment_input`], which both `compile_implicit_var_fragment`
/// (the production per-variable fragment compiler) and
/// `var_phase_symbolic_fragment_prod`'s no-`SourceVariable` arm (parent-
/// sourcing a synthetic helper that lands in a recurrence SCC) build their
/// input through, so the accessor's relation is the engine's relation by
/// construction.
///
/// Returns the helper's canonical name and the lowered variable. Loud-safe
/// `None` (never panics): this parse synthesized no helper of that name, the
/// module branch's datamodel variable is not actually a `Module`, or the
/// implicit var has equation errors. (`lower_variable` itself is total -- any
/// lowering error surfaces as `None` here, see below.)
fn lower_implicit_var(
    db: &dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
) -> Option<(String, crate::variable::Variable)> {
    let parsed = parse_source_variable(db, meta.parent_source_var, project);
    let implicit_var = meta.find_in(parsed)?;
    let implicit_name = canonicalize(implicit_var.ident()).into_owned();

    let dim_context = project_dimensions_context(db, project);

    // Every helper carries parsed data, so no helper is lexed back from text.
    let parsed_implicit = implicit_var.variable_stage0(dim_context);

    if parsed_implicit
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    // A module-typed helper is its wiring; `lower_variable`'s module arm would
    // need a populated models map to validate the sources against.
    let lowered = if meta.is_module {
        let dm_module = implicit_var.module()?;
        crate::variable::Variable::module_instance(
            Ident::new(&implicit_name),
            Ident::new(dm_module.model_name()),
            build_module_inputs(
                model.name(db),
                &module_input_prefix(&implicit_name),
                dm_module
                    .references()
                    .iter()
                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
            ),
        )
    } else {
        let models = HashMap::new();
        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: dim_context,
            model_name: "",
        };
        let lowered = crate::model::lower_variable(&scope, &parsed_implicit);

        // Loud-safe (GH #580): `lower_variable` is total -- on a lowering error
        // (e.g. an un-translatable cross-dimension subscript surviving into a
        // scalar helper as `DimensionInScalarContext`) it records the error and
        // discards the AST rather than failing. The pre-lowering check above
        // only inspects the *parsed* implicit; a lowering-stage error would
        // otherwise leave a helper with `ast == None` that `lower_fragment`
        // rejects as `EmptyEquation`. Bail out with `None` so the error rides
        // out via the caller's aggregate `missing_vars` string (GH #466 tracks
        // surfacing assembly-stage errors through the per-variable diagnostic
        // API).
        if lowered.equation_errors().is_some() {
            return None;
        }

        lowered
    };

    Some((implicit_name, lowered))
}

/// Why [`implicit_fragment_input`] produced no input.
pub(crate) enum ImplicitInputError {
    /// This parse synthesized no helper of that name, or the helper did not
    /// parse or lower: nothing to compile, and nothing to attribute beyond the
    /// caller's batch message.
    Absent,
    /// The helper's graphical-function table failed to build; the reason names
    /// the table error.
    Table(String),
}

/// Build the fragment input of one implicit helper (a SMOOTH/DELAY/TREND
/// instance, a hoisted argument aux, or a PREVIOUS/INIT capture) of `model`:
/// the helper's lowered form and the shape of every name it references.
///
/// The helper's dependencies come from the parent variable's dependency
/// extraction (`variable_direct_dependencies(parent).implicit_vars`), built
/// input-agnostic (the empty `ModuleInputSet`) so both branches of an
/// `isModuleInput(...)` conditional stay compilable. Every referenced name is
/// resolved through the per-variable firewall queries, so the helper's fragment
/// depends on exactly the names it looks up.
pub(crate) fn implicit_fragment_input<'db>(
    db: &'db dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> Result<FragmentInput<'db>, ImplicitInputError> {
    let (implicit_name, lowered) =
        lower_implicit_var(db, meta, model, project).ok_or(ImplicitInputError::Absent)?;
    let var_ident: Ident<Canonical> = Ident::new(&implicit_name);

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let parent_deps = variable_direct_dependencies(
        db,
        meta.parent_source_var,
        project,
        ModuleInputSet::empty(db),
    );
    let helper_deps = parent_deps
        .implicit_vars
        .iter()
        .find(|iv| canonicalize(&iv.name) == implicit_name);

    // Every name the helper references: both phases' data-flow dependencies,
    // the lookup tables it calls (layout references, not data-flow deps --
    // issue #606), and a stock helper's inflows and outflows.
    let mut shape_heads: BTreeSet<Ident<Canonical>> = helper_deps
        .into_iter()
        .flat_map(|iv| iv.dependencies.iter())
        .map(|dependency| dependency.target.local_node().clone())
        .collect();
    if let Some(iv) = helper_deps {
        shape_heads.extend(iv.referenced_tables.iter().map(|name| {
            let (head, _) = dep_head(name);
            Ident::new(head)
        }));
    }
    if let crate::variable::VarKind::Stock {
        inflows, outflows, ..
    } = &lowered.kind
    {
        shape_heads.extend(inflows.iter().chain(outflows.iter()).cloned());
    }

    let self_shape = if meta.is_module {
        module_dep_shape(db, project, meta.model_name.as_deref().unwrap_or(""))
    } else {
        // An arrayed helper (the GH #541 bare-arrayed-PREVIOUS capture)
        // occupies one slot per element; its dimensions are the parse's.
        DepShape::var(
            lowered
                .get_dimensions()
                .map(<[crate::dimensions::Dimension]>::to_vec)
                .unwrap_or_default(),
        )
    };
    let mut dep_shapes: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    dep_shapes.insert(var_ident.clone(), self_shape);
    for dep_name in &shape_heads {
        let head = dep_name.as_str();
        if head == implicit_name || is_implicit_global(head) || dep_shapes.contains_key(head) {
            continue;
        }
        // A helper's dependencies are explicit variables of the same model or
        // other helpers; a name that is neither fails to resolve at lowering.
        if let Some(shape) = model_dep_shape(db, model, project, head) {
            dep_shapes.insert(Ident::new(head), shape);
        }
    }

    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    let gf_tables = lowered.tables();
    if !gf_tables.is_empty() {
        match gf_tables
            .iter()
            .map(|t| crate::compiler::Table::new(&implicit_name, t))
            .collect::<crate::Result<Vec<_>>>()
        {
            Ok(ts) if !ts.is_empty() => {
                tables.insert(var_ident, ts);
            }
            Err(err) => {
                return Err(ImplicitInputError::Table(format!(
                    "its graphical-function table failed to build: {err}"
                )));
            }
            _ => {}
        }
    }
    for dep_name in &shape_heads {
        let head = dep_name.as_str();
        if tables.contains_key(head) {
            continue;
        }
        if let Some(dep_sv) = model_variable_by_name(db, model, head.to_string()) {
            let dep_tables = extract_tables_from_source_var(db, &dep_sv, project);
            if !dep_tables.is_empty() {
                tables.insert(Ident::new(head), dep_tables);
            }
        }
    }

    Ok(FragmentInput::new(
        lowered,
        dep_shapes,
        tables,
        canonical_module_input_set(module_input_names),
        Ident::new(model.name(db)),
        converted_dims,
        dim_context,
    ))
}

/// Compile a single implicit variable (generated by SMOOTH/DELAY/TREND
/// builtins, or a PREVIOUS/INIT capture) to symbolic bytecodes.
///
/// **Salsa-tracked, keyed on the helper's own canonical name.** The parent
/// variable's parse result caches the PARSE and nothing else; the lowering
/// and the per-phase codegen of a helper are this query's own work, and
/// without a memo of their own a model's helpers would be recompiled from
/// scratch each time `assemble_module` re-ran. On C-LEARN that is 651 calls
/// costing ~12% of a cold compile, and ~28% of the cost of a WARM
/// single-equation edit -- by far the largest share of a recompile that
/// should have touched one variable and its consumers.
///
/// The name is the only identity a helper has (it exists solely inside its
/// parent's parse), and it is the key `model_implicit_var_info` files it
/// under, so `model_implicit_var_by_name` resolves the metadata inside the
/// query rather than the caller passing a borrowed `&ImplicitVarMeta` that no
/// salsa key could carry. `ImplicitVarMeta::name`'s own rustdoc explains why a
/// name and not a position, and bounds the one case where a name resolves to a
/// different helper than the metadata meant -- a case that already fails to
/// compile.
///
/// The runlist gate reads `implicit_var_runlist_membership` rather than the
/// whole `ModelDepGraphResult`, for the same reason `compile_var_fragment`
/// reads `var_runlist_membership`: a three-bit projection backdates when this
/// helper's membership is unchanged, where the whole result re-executes every
/// helper's fragment whenever any variable's dependencies move.
#[salsa::tracked(returns(ref))]
pub(crate) fn compile_implicit_var_fragment<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    implicit_var_name: String,
    module_inputs: ModuleInputSet<'db>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{CompiledVarFragment, PerVarBytecodes};

    let meta = &model_implicit_var_by_name(db, model, project, implicit_var_name.clone())?;
    let module_input_names = module_inputs.names(db);

    // Recorded at body entry (before the helper is even resolved), keyed by the
    // parent variable and the helper's own name -- the identity this compiler is
    // called with. Recording after the constructor would silently omit every
    // entry that failed to lower, which is exactly the work a caching claim
    // needs to account for.
    #[cfg(test)]
    note_fragment_execution(
        FragmentExecKind::Implicit,
        &format!("{}#{}", meta.parent_source_var.ident(db), meta.name),
    );

    // Each runlist-gated phase threads the GH #1000 `why` channel: a member
    // phase that fails to compile lands in `assemble_module`'s batch
    // "failed to compile fragments" message, so the reason is accumulated
    // HERE as a diagnostic naming the helper (interpolated into the message
    // -- `errors.rs`'s `Assembly` arm never renders the `variable` field)
    // and the parent it was synthesized for. Accumulation attaches to the
    // enclosing query: `model_all_diagnostics`' implicit probe (drained by
    // `collect_all_diagnostics`) or `assemble_module` (dormant until
    // GH #581). Severity is `Error` for the same measured reason as the
    // explicit path's (see `compile_var_fragment`): the fragment's absence
    // fails the build, and the corpus sweep shape holds -- added rows land
    // only on projects that already fail to compile.
    // Identical reasons across phases collapse to ONE row (a helper whose
    // initial and flow phases refuse the same construct is one defect, and
    // duplicate rows are user-visible noise); distinct per-phase reasons
    // each get their own row.
    let mut reported_reasons: Vec<String> = Vec::new();
    let mut report = |reason: String| {
        if reported_reasons.contains(&reason) {
            return;
        }
        reported_reasons.push(reason.clone());
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(meta.name.clone()),
            error: DiagnosticError::Assembly(format!(
                "implicit variable '{}' (synthesized while parsing '{}') failed to compile: \
                 {reason}",
                meta.name,
                meta.parent_source_var.ident(db)
            )),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
    };

    let input = match implicit_fragment_input(db, meta, model, project, module_input_names) {
        Ok(input) => input,
        Err(ImplicitInputError::Absent) => return None,
        // The helper exists but nothing can be emitted for it; report why, and
        // keep the helper's place in the fragment map (its runlist entries
        // then surface as missing fragments).
        Err(ImplicitInputError::Table(reason)) => {
            report(reason);
            return Some(VarFragmentResult {
                fragment: CompiledVarFragment {
                    ident: meta.name.clone(),
                    initial_bytecodes: None,
                    flow_bytecodes: None,
                    stock_bytecodes: None,
                },
                flow_locally_invariant: None,
            });
        }
    };

    let membership = crate::db::dep_graph::implicit_var_runlist_membership(
        db,
        model,
        project,
        meta.name.clone(),
        module_inputs,
    );

    // Runlist-gated phase selection: the Initial phase is compiled only for
    // helpers in `runlist_initials`; the non-initial phase feeds
    // `flow_bytecodes` (non-stock) or `stock_bytecodes` (stock/module), each
    // gated by the corresponding runlist.
    let emit_ctx = input.emit_ctx();
    let mut phase = |is_initial: bool| -> Option<PerVarBytecodes> {
        match lower_fragment(&input, is_initial) {
            Ok(var) => {
                match crate::db::assemble::compile_phase_to_per_var_bytecodes_reporting(
                    &emit_ctx, &var.ast,
                ) {
                    Ok(bytecodes) => Some(bytecodes),
                    Err(reason) => {
                        report(reason);
                        None
                    }
                }
            }
            Err(err) => {
                report(format!("could not be lowered: {err}"));
                None
            }
        }
    };

    let initial_bytecodes = if membership.initials {
        phase(true)
    } else {
        None
    };
    let flow_bytecodes = if !meta.is_stock && membership.flows {
        phase(false)
    } else {
        None
    };
    let stock_bytecodes = if (meta.is_stock || meta.is_module) && membership.stocks {
        phase(false)
    } else {
        None
    };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: meta.name.clone(),
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
        // Implicit helpers (SMOOTH/DELAY/TREND) are always dynamic; the
        // run-invariance analysis only applies to explicit source variables.
        flow_locally_invariant: None,
    })
}
