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
use crate::ast::LoweringScope;
use crate::common::{Canonical, Ident, IdentMap};
use crate::compiler::fragment::{DepShape, FragmentInput, lower_fragment};
use crate::db::var_fragment::{
    DeclaredName, ExplicitFragment, ResolvedHeads, explicit_fragment_input, is_implicit_global,
};

// Test-only per-thread record of which fragment-compiler bodies ran.
//
// Pointer equality of a memo does NOT prove a query body did not run: salsa
// backdates a re-executed query whose value compares equal and keeps the memo
// address. For the fragment compilers that matters especially, because a
// fragment is
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
// Thread-local rather than a global atomic: no lock, and a parallel test run
// cannot charge one test's work to another. The record happens INSIDE the
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

    // Every diagnostic the constructor raised is replayed in its order; a
    // fatal one leaves no input, and the variable compiles to nothing.
    let ExplicitFragment { diagnostics, input } =
        explicit_fragment_input(db, var, model, project, module_input_names);
    for diag in diagnostics {
        diag.accumulate(db);
    }
    let input = input?;

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
                Diagnostic {
                    model: model.name(db).clone(),
                    variable: Some(ident.clone()),
                    owner: None,
                    severity: DiagnosticSeverity::Error,
                    error: DiagnosticError::Assembly(format!(
                        "variable '{ident}' failed to compile: {reason}"
                    )),
                }
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
        Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var.ident(db).clone()),
            owner: None,
            severity: DiagnosticSeverity::Error,
            error: DiagnosticError::Equation(err.clone().into()),
        }
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

    // The compiler-local half of run invariance (GH #712), stored on the
    // salsa-cached result so `model_flows_invariant`'s fixpoint reads it
    // without re-lowering. Only meaningful for vars in the flows runlist.
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

/// Why [`implicit_fragment_input`] produced no input.
pub(crate) enum ImplicitInputError {
    /// This parse synthesized no helper of that name: nothing to compile, and
    /// nothing to attribute beyond the caller's batch message.
    Absent,
    /// The helper's body did not parse or lower; these are its diagnostics,
    /// whose spans index the PARENT's equation text (where the helper's
    /// subtree was written), so they are reported on the parent.
    Lowering(Vec<DiagnosticError>),
}

/// Every name a helper's equation resolves through: the head of each of its
/// reads from the parent's dependency extraction
/// (`variable_direct_dependencies(parent).implicit_vars`, built input-agnostic
/// so both branches of an `isModuleInput(...)` conditional stay compilable),
/// the lookup tables it calls (layout references, not data-flow deps -- issue
/// #606), and a stock helper's inflows and outflows.
fn implicit_referenced_names<MI, E>(
    db: &dyn Db,
    meta: &ImplicitVarMeta,
    project: SourceProject,
    helper: &crate::variable::Variable<MI, E>,
) -> BTreeSet<Ident<Canonical>> {
    let parent_deps = variable_direct_dependencies(
        db,
        meta.parent_source_var,
        project,
        ModuleInputSet::empty(db),
    );
    let mut names: BTreeSet<Ident<Canonical>> = parent_deps
        .implicit_vars
        .iter()
        .filter(|iv| canonicalize(&iv.name) == meta.name)
        .flat_map(|iv| {
            iv.deps
                .heads()
                .into_iter()
                .chain(iv.referenced_tables.iter())
                .cloned()
        })
        .collect();
    if let crate::variable::VarKind::Stock {
        inflows, outflows, ..
    } = &helper.kind
    {
        names.extend(inflows.iter().chain(outflows.iter()).cloned());
    }
    names
}

/// Every name in `names` that `model` declares (the helper itself and the
/// implicit globals skipped), each resolved once through `DeclaredName`. A
/// helper's dependencies are explicit variables of the same model or other
/// helpers; a name that is neither fails to resolve at lowering.
fn implicit_referenced_heads(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    self_name: &str,
    names: &BTreeSet<Ident<Canonical>>,
) -> ResolvedHeads {
    let mut heads: ResolvedHeads = Vec::new();
    for head in names {
        if head.as_str() == self_name || is_implicit_global(head.as_str()) {
            continue;
        }
        if let Some(declared) = DeclaredName::resolve(db, model, project, head.as_str()) {
            heads.push((head.clone(), declared));
        }
    }
    heads
}

/// One implicit helper lowered once, with the names it references resolved
/// once: what [`lowered_implicit_variable`] memoizes.
#[derive(Clone, PartialEq)]
pub(crate) struct LoweredImplicit {
    /// The helper in its `Expr2` form.
    pub variable: std::sync::Arc<crate::variable::Variable>,
    /// The head of every name the helper references that the model declares
    /// ([`implicit_referenced_heads`]); `implicit_fragment_input` projects the
    /// compiler's shapes and the tables it needs from these.
    pub heads: ResolvedHeads,
}

/// One implicit helper (a SMOOTH/DELAY/TREND instance, a hoisted argument
/// aux, or a PREVIOUS/INIT capture) of `model` in its `Expr2` form, lowered
/// once under the dimensions of the names it references
/// ([`implicit_referenced_heads`]); `None` when the parent's parse
/// synthesized no helper of that name.
///
/// The one owner of a helper's lowered form, keyed on the helper's canonical
/// name -- the only identity a helper has (it exists solely inside its
/// parent's parse), and the key `model_implicit_var_info` files it under.
/// `implicit_fragment_input` borrows it and the LTM describers hold its `Arc`
/// (`db::model_lowered_variables`; an element-scoped helper's map entry is
/// the memo's element-pinned projection). A helper lowers under the shapes
/// its parent's equation lowers under, so a hoisted argument is refused
/// where, and with the code, the plain spelling is
/// (`db::lowering_scope_tests`). `lower_variable` is total (GH #580): a body
/// that does not parse or lower keeps its errors and has no AST, and the
/// fragment constructor reports those errors against the parent.
#[salsa::tracked(returns(ref))]
pub(crate) fn lowered_implicit_variable(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    implicit_var_name: String,
) -> Option<LoweredImplicit> {
    let meta = model_implicit_var_by_name(db, model, project, implicit_var_name)?;
    let parsed = parse_source_variable(db, meta.parent_source_var, project);
    let implicit_var = meta.find_in(parsed)?;
    let dim_context = project_dimensions_context(db, project);
    let helper = implicit_var.parsed_variable(dim_context);

    // A module-typed helper is its wiring, resolved by `lower_variable`'s
    // module arm like an explicit instance's, under the model's canonical
    // name; nothing is lowered under a shape, though its input sources are
    // still resolved for the instance's fragment. Any other helper lowers
    // under the dimensions of the names it references
    // (`DeclaredName::dimensions_shape`): an arrayed helper (a structural
    // apply-to-all capture) has its declared dimensions, every other helper
    // is scalar.
    let names = implicit_referenced_names(db, &meta, project, &helper);
    let heads = implicit_referenced_heads(db, model, project, &meta.name, &names);
    let mut shapes: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    if !meta.is_module {
        shapes.insert(
            Ident::new(&meta.name),
            DepShape::var(
                helper
                    .get_dimensions()
                    .map(<[crate::dimensions::Dimension]>::to_vec)
                    .unwrap_or_default(),
            ),
        );
        for (ident, declared) in &heads {
            if let Some(shape) = declared.dimensions_shape(db, project) {
                shapes.insert(ident.clone(), shape);
            }
        }
    }
    let model_ident: Ident<Canonical> = Ident::new(model.name(db));
    let scope = LoweringScope {
        dimensions: dim_context,
        shapes: &shapes,
        model_name: model_ident.as_str(),
    };
    Some(LoweredImplicit {
        variable: std::sync::Arc::new(crate::model::lower_variable(&scope, &helper)),
        heads,
    })
}

/// Build the fragment input of one implicit helper of `model`: its lowered
/// form ([`lowered_implicit_variable`], borrowed) plus the shape of every name
/// it references and the tables it calls.
///
/// This is the *single relation* from an `ImplicitVarMeta` to a compilable
/// helper, consumed by `compile_implicit_var_fragment` (the production
/// per-variable fragment compiler), by `var_phase_symbolic_fragment_prod`'s
/// no-`SourceVariable` arm (a helper that lands in a recurrence SCC) and by
/// the LTM describers' element-pinned projection of an element-scoped helper,
/// so every reader sees one lowering.
///
/// Loud-safe (never panics): `Absent` when this parse synthesized no helper of
/// that name, `Lowering` with the body's equation errors when it did not parse
/// or lower -- an un-translatable cross-dimension subscript surviving into a
/// scalar helper as `DimensionInScalarContext` lands on the variable's error
/// channel and discards the AST, which `lower_fragment` would otherwise reject
/// as an `EmptyEquation`, so the caller reports the errors against the parent.
pub(crate) fn implicit_fragment_input<'db>(
    db: &'db dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> Result<FragmentInput<'db>, ImplicitInputError> {
    let LoweredImplicit {
        variable: lowered,
        heads,
    } = lowered_implicit_variable(db, model, project, meta.name.clone())
        .as_ref()
        .ok_or(ImplicitInputError::Absent)?;
    let failures: Vec<DiagnosticError> = lowered.fatal_diagnostics().cloned().collect();
    if !failures.is_empty() {
        return Err(ImplicitInputError::Lowering(failures));
    }

    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    // The shape of every name the helper can reference, itself included, as
    // the compiler resolves them: a module-typed helper is its sub-model's
    // shape, an arrayed helper (a structural apply-to-all capture) occupies
    // one slot per element, and every other helper is scalar.
    let self_shape = if meta.is_module {
        module_dep_shape(db, project, meta.model_name.as_deref().unwrap_or(""))
    } else {
        DepShape::var(
            lowered
                .get_dimensions()
                .map(<[crate::dimensions::Dimension]>::to_vec)
                .unwrap_or_default(),
        )
    };
    let mut dep_shapes: IdentMap<Ident<Canonical>, DepShape> = Default::default();
    dep_shapes.insert(Ident::new(&meta.name), self_shape);
    for (ident, declared) in heads {
        dep_shapes.insert(ident.clone(), declared.shape(db, project));
    }

    // A synthesized helper carries no graphical function of its own
    // (`ImplicitVar::parsed_variable` builds it with no tables); only the
    // tables of the dependencies it reads through `LOOKUP(dep, x)` are needed.
    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    for (ident, declared) in heads {
        let DeclaredName::Source(dep_sv) = declared else {
            continue;
        };
        let dep_tables = variable_tables(db, *dep_sv, project);
        if !dep_tables.is_empty() {
            tables.insert(ident.clone(), dep_tables.clone());
        }
    }

    Ok(FragmentInput::new(
        std::borrow::Cow::Borrowed(lowered.as_ref()),
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
    // HERE. A CODEGEN refusal is a diagnostic filed under the helper's
    // physical name, whose `owner` is the parent it was synthesized for --
    // the name a consumer presents it under -- and whose message names both
    // (`errors.rs`'s `Assembly` arm never renders the `variable` field).
    // Accumulation attaches to the enclosing query: `model_all_diagnostics`'
    // implicit probe (drained by `collect_all_diagnostics`) or
    // `assemble_module` (dormant until GH #581). Severity is `Error` for the
    // same measured reason as the explicit path's (see
    // `compile_var_fragment`): the fragment's absence fails the build, and
    // the corpus sweep shape holds -- added rows land only on projects that
    // already fail to compile. A helper whose initial and flow phases refuse
    // the same construct raises two identical rows, which the drain collapses
    // to one; distinct per-phase reasons each keep their row.
    let report = |reason: String| {
        Diagnostic {
            model: model.name(db).clone(),
            variable: Some(meta.name.clone()),
            owner: Some(meta.parent_source_var.ident(db).clone()),
            severity: DiagnosticSeverity::Error,
            error: DiagnosticError::Assembly(format!(
                "implicit variable '{}' (synthesized while parsing '{}') failed to compile: \
                 {reason}",
                meta.name,
                meta.parent_source_var.ident(db)
            )),
        }
        .accumulate(db);
    };

    // A helper that exists but for which nothing can be emitted keeps its
    // place in the fragment map (its runlist entries then surface as missing
    // fragments).
    let unemitted = || {
        Some(VarFragmentResult {
            fragment: CompiledVarFragment {
                ident: meta.name.clone(),
                initial_bytecodes: None,
                flow_bytecodes: None,
                stock_bytecodes: None,
            },
            flow_locally_invariant: None,
        })
    };
    let input = match implicit_fragment_input(db, meta, model, project, module_input_names) {
        Ok(input) => input,
        Err(ImplicitInputError::Absent) => return None,
        // A body's diagnostics are the PARENT's: their spans index the
        // parent's equation text, where the argument was written, so they are
        // reported against the parent and render as a snippet under the
        // argument, exactly as a plain equation's errors do. Identical errors
        // from the per-element helpers of one parent collapse to one row.
        Err(ImplicitInputError::Lowering(failures)) => {
            let parent = meta.parent_source_var.ident(db).clone();
            for error in failures {
                Diagnostic {
                    model: model.name(db).clone(),
                    variable: Some(parent.clone()),
                    owner: None,
                    severity: DiagnosticSeverity::Error,
                    error,
                }
                .accumulate(db);
            }
            return unemitted();
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
    // `flow_bytecodes` (a capture or hoisted argument) or `stock_bytecodes`
    // (a module instance), each gated by the corresponding runlist. A
    // capture's runlists are its phase demand (`CaptureKind`), decided by
    // the dependency graph, so an INIT-only capture arrives here with no
    // flows membership and gets no flow fragment.
    // A body the COMPILER refuses is refused where the parent's plain
    // equation would be, as the parent's equation error: a subtree-bodied
    // helper is the whole or one element of the parent's body, so the
    // compiler's verdict on it is the verdict on the plain spelling, its code
    // is that spelling's code, and the argument's span in the parent's
    // equation text is where the rendering underlines it. The drain collapses
    // the per-element and per-phase repeats of one `(code, span)` into one
    // row. A module instance has no argument of its own, so its lowering
    // failure keeps the assembly row.
    let parent_argument_span = || {
        let parsed = parse_source_variable(db, meta.parent_source_var, project);
        meta.find_in(parsed)
            .and_then(crate::capture::ImplicitVar::arg)
            .map(|arg| arg.get_loc())
    };
    let emit_ctx = input.emit_ctx();
    let phase = |is_initial: bool| -> Option<PerVarBytecodes> {
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
                match parent_argument_span() {
                    Some(loc) => Diagnostic {
                        model: model.name(db).clone(),
                        variable: Some(meta.parent_source_var.ident(db).clone()),
                        owner: None,
                        severity: DiagnosticSeverity::Error,
                        error: DiagnosticError::Equation(crate::common::EquationError {
                            start: loc.start,
                            end: loc.end,
                            code: err.code,
                            details: err.details,
                        }),
                    }
                    .accumulate(db),
                    None => report(format!("could not be lowered: {err}")),
                }
                None
            }
        }
    };

    let initial_bytecodes = if membership.initials {
        phase(true)
    } else {
        None
    };
    let flow_bytecodes = if membership.flows { phase(false) } else { None };
    let stock_bytecodes = if meta.is_module && membership.stocks {
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
