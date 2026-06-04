// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-variable bytecode emission: the *emission half* of per-variable
//! compilation. The *lowering half* (parse + lower to `Vec<Expr>`) lives in
//! the sibling `db/var_fragment.rs` (`lower_var_fragment`); this module
//! consumes that and emits + symbolizes the per-phase bytecode.
//!
//! `compile_var_fragment` is the salsa-tracked per-variable fragment
//! compiler for explicit variables. `compile_implicit_var_fragment` /
//! `compile_implicit_var_phase_bytecodes` do the same for the implicit
//! (SMOOTH/DELAY/TREND) helpers, sharing the `lower_implicit_var`
//! parent->implicit->parse->lower prefix. The compile+symbolize tail and the
//! production element-graph source (`compile_phase_to_per_var_bytecodes`,
//! `var_phase_symbolic_fragment_prod`) live in the sibling `db/assemble.rs`.

use std::collections::{BTreeSet, HashMap};

use salsa::Accumulator;

use super::*;
use crate::common::{Canonical, Ident};

#[salsa::tracked(returns(ref))]
pub fn compile_var_fragment<'db>(
    db: &'db dyn Db,
    var: SourceVariable,
    model: SourceModel,
    project: SourceProject,
    module_inputs: ModuleInputSet<'db>,
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::{CompiledVarFragment, PerVarBytecodes};
    use crate::db::var_fragment::{LoweredVarFragment, lower_var_fragment};

    let var_ident = var.ident(db).clone();
    let var_ident_canonical: Ident<Canonical> = Ident::new(&var_ident);

    // The interned input set stores the sorted canonical names; the plain
    // lowering helpers (`lower_var_fragment`/`build_caller_module_refs`) still
    // take `&[String]`, so read it back as a slice.
    let module_input_names = module_inputs.names(db);

    // Caller-owned, lowering-independent context (built only from
    // project/variable data, never from the lowered equation). Read the
    // salsa-cached project-global dimension context and converted dims
    // (returns(ref)) rather than rebuilding them on every variable -- this
    // fragment compiler is invoked once per variable, and the context is
    // project-global and immutable. Building it canonicalizes every dimension
    // element name, so caching it removes a dominant per-variable allocation.
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);
    let model_name_ident = Ident::new(model.name(db));
    let inputs = canonical_module_input_set(module_input_names);
    let module_models = model_module_map(db, model, project).clone();

    let lowered = lower_var_fragment(
        db,
        var,
        model,
        project,
        module_input_names,
        converted_dims,
        dim_context,
        &model_name_ident,
        &module_models,
        &inputs,
    );

    let (unit_diags, per_phase_lowered, tables, offsets, rmap, mini_offset) = match lowered {
        LoweredVarFragment::Fatal {
            unit_diags,
            fatal_diags,
        } => {
            // Non-fatal unit diagnostics were recorded before the fatal
            // site; replay them first to preserve emission order, then
            // the fatal diagnostic(s), then bail out (whole-variable None).
            for diag in unit_diags {
                CompilationDiagnostic(diag).accumulate(db);
            }
            for diag in fatal_diags {
                CompilationDiagnostic(diag).accumulate(db);
            }
            return None;
        }
        LoweredVarFragment::Lowered {
            unit_diags,
            per_phase_lowered,
            tables,
            offsets,
            rmap,
            mini_offset,
        } => (
            unit_diags,
            per_phase_lowered,
            tables,
            offsets,
            rmap,
            mini_offset,
        ),
    };

    // Malformed-unit diagnostics are non-fatal: record them and continue.
    for diag in unit_diags {
        CompilationDiagnostic(diag).accumulate(db);
    }

    // Determine which runlists this variable belongs to
    let dep_graph = model_dependency_graph(db, model, project, module_inputs);
    let is_stock = var.kind(db) == SourceVariableKind::Stock;
    let is_module = var.kind(db) == SourceVariableKind::Module;
    let is_module_input = inputs.contains(&var_ident_canonical);

    let module_refs = crate::db::var_fragment::build_caller_module_refs(
        db,
        var,
        model,
        project,
        module_input_names,
    );

    // Compile for each phase and symbolize. The closure now delegates to
    // the factored `compile_phase_to_per_var_bytecodes` so the SCC
    // element-graph builder reuses the EXACT production compile+symbolize
    // path (no re-derivation); the per-variable production behavior is
    // byte-identical to the former inline closure (same minimal `Module`,
    // same temp extraction, same symbolization, same `None` arms).
    let compile_phase = |exprs: &[crate::compiler::Expr]| -> Option<PerVarBytecodes> {
        compile_phase_to_per_var_bytecodes(
            exprs,
            &offsets,
            &rmap,
            &tables,
            &module_refs,
            mini_offset,
            converted_dims,
            dim_context,
            &model_name_ident,
            &var_ident_canonical,
            &inputs,
        )
    };

    // Runlists use canonical names, so compare with the canonical form.
    let var_ident_str = var_ident_canonical.as_str().to_string();

    // Accumulate a diagnostic when per-variable compilation (Var::new)
    // fails. Without this, errors like DoesNotExist (unknown dependency)
    // are silently dropped and never appear in collect_all_diagnostics.
    let accumulate_var_compile_error = |err: &crate::Error| {
        CompilationDiagnostic(Diagnostic {
            model: model.name(db).clone(),
            variable: Some(var.ident(db).clone()),
            error: DiagnosticError::Equation(crate::common::EquationError {
                start: 0,
                end: 0,
                code: err.code,
            }),
            severity: DiagnosticSeverity::Error,
        })
        .accumulate(db);
    };

    // Initial phase: stocks and their deps get compiled with is_initial=true
    let initial_bytecodes = if dep_graph.runlist_initials.contains(&var_ident_str) {
        match &per_phase_lowered.initial {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(err) => {
                accumulate_var_compile_error(err);
                None
            }
        }
    } else {
        None
    };

    // Flow phase: non-stock vars AND stock-typed module inputs get compiled
    // with is_initial=false. Stock-typed module inputs need LoadModuleInput ->
    // AssignCurr in the flows phase to propagate the parent-provided value
    // each timestep (matching the monolithic path's `instantiation.contains(id)
    // || !var.is_stock()` filter).
    let in_flows_runlist =
        (!is_stock || is_module_input) && dep_graph.runlist_flows.contains(&var_ident_str);
    let flow_bytecodes = if in_flows_runlist {
        match &per_phase_lowered.noninitial {
            Ok(var_result) => compile_phase(&var_result.ast),
            Err(err) => {
                accumulate_var_compile_error(err);
                None
            }
        }
    } else {
        None
    };

    // Pre-compute flow invariance support for `model_flows_invariant` (GH
    // #712). Stored on the salsa-cached result so the topological fixpoint
    // pass in `model_flows_invariant` can read it without re-calling
    // `lower_var_fragment`. Only meaningful for vars in the flows runlist.
    let flow_invariance = if in_flows_runlist {
        crate::db::assemble::compute_flow_invariance_support(
            &per_phase_lowered.noninitial,
            &offsets,
            &model_name_ident,
            &var_ident_canonical,
        )
    } else {
        None
    };

    // Stock phase: stocks and modules get compiled with is_initial=false
    let stock_bytecodes =
        if (is_stock || is_module) && dep_graph.runlist_stocks.contains(&var_ident_str) {
            match &per_phase_lowered.noninitial {
                Ok(var_result) => compile_phase(&var_result.ast),
                Err(err) => {
                    accumulate_var_compile_error(err);
                    None
                }
            }
        } else {
            None
        };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: var_ident,
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
        flow_invariance,
    })
}

/// The genuinely-shared prefix of synthetic-helper sourcing: resolve a
/// model's implicit variable from its parent's `implicit_vars`, parse it,
/// and lower it to a `crate::variable::Variable`.
///
/// This is the *single shared relation* (DRY -- "never re-derive") for
/// "given an `ImplicitVarMeta`, produce the helper's parsed + lowered
/// form". It is the exact `model_implicit_var_info`-fed chain
/// `parent → parsed.implicit_vars[index] → parse_var → lower_variable`
/// (the non-module branch builds via `lower_variable`; the module branch
/// constructs a `Variable::Module` directly because `lower_variable` with
/// an empty models map fails `resolve_module_input`). It is consumed by
/// both `compile_implicit_var_fragment` (the production per-variable
/// fragment compiler) and `var_phase_symbolic_fragment_prod`'s
/// no-`SourceVariable` arm (element-cycle Phase 3 Task 2 / AC3.1:
/// parent-sourcing a synthetic helper that lands in a recurrence SCC), so
/// the accessor's relation is the engine's relation by construction.
///
/// Returns the helper's canonical name and the lowered variable. The
/// parent's `ParsedVariableResult` is intentionally NOT returned: callers
/// that also need it re-call the salsa-`returns(ref)`-cached
/// `parse_source_variable_with_module_context` (a cache hit -- a borrow,
/// zero clone), exactly as the pre-extraction code did. Loud-safe `None`
/// (never panics): the implicit index is absent, the module branch's
/// datamodel variable is not actually a `Module`, or the implicit var has
/// equation errors. (`lower_variable` itself is total -- any lowering
/// error surfaces as a `LoweredVarFragment::Fatal` / `Var::new` error
/// downstream, not here.)
fn lower_implicit_var<'db>(
    db: &'db dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_ident_context: ModuleIdentContext<'db>,
) -> Option<(String, crate::variable::Variable)> {
    let parsed = parse_source_variable_with_module_context(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
    );
    let implicit_dm_var = parsed.implicit_vars.get(meta.index_in_parent)?;
    let implicit_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

    let dm_dims = project_datamodel_dims(db, project);
    let dim_context = project_dimensions_context(db, project);

    let units_ctx = project_units_context(db, project);

    let mut dummy_implicits = Vec::new();
    let parsed_implicit = crate::variable::parse_var(
        dm_dims,
        implicit_dm_var,
        &mut dummy_implicits,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
    );

    if parsed_implicit
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }

    // Module-type implicit vars need direct Module construction (lower_variable
    // with empty models map causes resolve_module_input to fail).
    let lowered = if meta.is_module {
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let module_inputs: Vec<crate::variable::ModuleInput> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let ident_prefix = format!("{}·", canonicalize(&implicit_name));
                    let src = canonicalize(&mr.src);
                    let dst = canonicalize(&mr.dst);
                    if src.starts_with(&ident_prefix) {
                        return None;
                    }
                    let dst_stripped = dst.strip_prefix(&ident_prefix)?;
                    let src_str = if model.name(db) == "main" && src.starts_with('·') {
                        &src['·'.len_utf8()..]
                    } else {
                        &src
                    };
                    Some(crate::variable::ModuleInput {
                        src: Ident::new(src_str),
                        dst: Ident::new(dst_stripped),
                    })
                })
                .collect();
            crate::variable::Variable::Module {
                ident: Ident::new(&implicit_name),
                model_name: Ident::new(&dm_module.model_name),
                units: None,
                inputs: module_inputs,
                errors: vec![],
                unit_errors: vec![],
            }
        } else {
            return None;
        }
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
        // otherwise leave a helper with `ast == None` that
        // `compile_implicit_var_phase_bytecodes` -> `Var::new` rejects as
        // `EmptyEquation`. Bail out with `None` so the error rides out via the
        // caller's aggregate `missing_vars` string (GH #466 tracks surfacing
        // assembly-stage errors through the per-variable diagnostic API).
        if lowered.equation_errors().is_some() {
            return None;
        }

        lowered
    };

    Some((implicit_name, lowered))
}

/// Compile a single implicit variable (generated by SMOOTH/DELAY/TREND builtins)
/// to symbolic bytecodes. Not a tracked function -- the parent variable's
/// parse result already provides salsa caching.
pub(crate) fn compile_implicit_var_fragment(
    db: &dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    dep_graph: &ModelDepGraphResult,
    module_input_names: &[String],
) -> Option<VarFragmentResult> {
    use crate::compiler::symbolic::CompiledVarFragment;

    // The implicit var's canonical name (the runlist-gate key). Resolve it
    // through the shared prefix so this and the per-phase compile agree on
    // the name by construction. `None` here is the same loud-safe signal
    // the per-phase compile returns (absent implicit index / equation
    // errors).
    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.to_vec());
    let (implicit_name, _lowered) =
        lower_implicit_var(db, meta, model, project, module_ident_context)?;
    let var_ident_str = canonicalize(&implicit_name).into_owned();

    // Runlist-gated phase selection (unchanged output behavior): the
    // Initial phase is compiled only for implicit vars in
    // `runlist_initials`; the non-initial phase feeds `flow_bytecodes`
    // (non-stock) or `stock_bytecodes` (stock/module), each gated by the
    // corresponding runlist. The per-phase compile builds its own context;
    // it is invoked at most for the gated phases (≤2), so the only cost
    // vs. the prior single-context build is a bounded extra
    // map-construction on the ≤2-phase implicit-var sub-path -- the
    // duplication-free price for a single shared per-phase relation
    // (`compile_implicit_var_phase_bytecodes`, also consumed by
    // `var_phase_symbolic_fragment_prod`'s no-`SourceVariable` arm).
    let phase = |is_initial: bool| {
        compile_implicit_var_phase_bytecodes(
            db,
            meta,
            model,
            project,
            module_input_names,
            is_initial,
        )
    };

    let initial_bytecodes = if dep_graph.runlist_initials.contains(&var_ident_str) {
        phase(true)
    } else {
        None
    };
    let flow_bytecodes = if !meta.is_stock && dep_graph.runlist_flows.contains(&var_ident_str) {
        phase(false)
    } else {
        None
    };
    let stock_bytecodes =
        if (meta.is_stock || meta.is_module) && dep_graph.runlist_stocks.contains(&var_ident_str) {
            phase(false)
        } else {
            None
        };

    Some(VarFragmentResult {
        fragment: CompiledVarFragment {
            ident: implicit_name,
            initial_bytecodes,
            flow_bytecodes,
            stock_bytecodes,
        },
        // Implicit helpers (SMOOTH/DELAY/TREND) are always dynamic; the
        // run-invariance analysis only applies to explicit source variables.
        flow_invariance: None,
    })
}

/// Build the mini-layout context for one implicit variable and compile a
/// single phase (`is_initial`) to symbolic `PerVarBytecodes`. NOT a tracked
/// function -- the parent variable's parse result already provides salsa
/// caching.
///
/// This is the **single shared per-phase relation** for "produce a
/// synthetic helper's symbolic `PerVarBytecodes`": consumed by
/// `compile_implicit_var_fragment` (the production per-variable assembly,
/// runlist-gated) *and* `var_phase_symbolic_fragment_prod`'s
/// no-`SourceVariable` arm (element-cycle Phase 3 Task 2 / AC3.1 --
/// parent-sourcing a synthetic helper that lands in a recurrence SCC), so
/// the element-graph accessor's bytecode is byte-identical to the
/// production fragment by construction (DRY -- "single shared relation,
/// never re-derive"). The shared `parent → implicit → parse → lower`
/// prefix is `lower_implicit_var`; the shared compile+symbolize tail is
/// `compile_phase_to_per_var_bytecodes` (the exact function the real-var
/// arm of `var_phase_symbolic_fragment_prod` and `compile_var_fragment`
/// use). The mini-layout/metadata/dep-collection glue between them is
/// intrinsic to the implicit-var shape (the `meta.is_module` branch, the
/// body-relative mini-layout, the dep-stub/sub-model collection) and is not
/// separately extractable without restructuring this function.
///
/// Loud-safe `None` (never panics): the shared prefix failed (absent
/// implicit index / equation errors), a graphical-function table failed to
/// build, the phase's `Var::new` errored, or `Module::compile()` /
/// symbolization failed -- exactly the original closure's `None` arms.
#[allow(clippy::too_many_arguments)]
pub(crate) fn compile_implicit_var_phase_bytecodes(
    db: &dyn Db,
    meta: &ImplicitVarMeta,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
    is_initial: bool,
) -> Option<crate::compiler::symbolic::PerVarBytecodes> {
    use crate::compiler::symbolic::{ReverseOffsetMap, VariableLayout};

    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.to_vec());

    // Shared parent→implicit→parse→lower prefix (the single relation, also
    // consumed by `compile_implicit_var_fragment`). `ModuleIdentContext`
    // is a `Copy` interned handle, so the one context is threaded into
    // both the shared prefix and the parse below (a single
    // `module_ident_context_for_model` build, matching the pre-extraction
    // monolith).
    let (implicit_name, lowered) =
        lower_implicit_var(db, meta, model, project, module_ident_context)?;
    // The parent's parsed result for the module-refs reconstruction /
    // dep-collection below. `parse_source_variable_with_module_context`
    // is salsa-`returns(ref)`-cached, so this is a cache-hit borrow (zero
    // clone) -- `lower_implicit_var` already populated it.
    let parsed = parse_source_variable_with_module_context(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
    );
    let implicit_dm_var = parsed.implicit_vars.get(meta.index_in_parent)?;

    // Project-global dimension context + converted dims, read from the
    // salsa-cached queries rather than rebuilt per implicit variable.
    let dim_context = project_dimensions_context(db, project);
    let converted_dims = project_converted_dimensions(db, project);

    let model_name_ident = Ident::new(model.name(db));
    let var_ident_canonical: Ident<Canonical> = Ident::new(&implicit_name);

    // Arena for sub-model stub variables allocated by build_submodel_metadata
    let arena = bumpalo::Bump::new();

    // The mini-layout is always body-relative (offset 0). The implicit
    // globals (time/dt/initial_time/final_time) are NOT inserted: they
    // lower to `LoadGlobalVar` at fixed absolute slots, never through this
    // metadata/rmap, so the symbolic fragment is role-independent. The root
    // +IMPLICIT_VAR_COUNT shift is applied later in `assemble_module` via
    // `VariableLayout::root_shifted`.
    let mut mini_metadata: HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>> =
        HashMap::new();
    let mut mini_offset = 0;

    let project_models = project.models(db);
    let self_size = if meta.is_module {
        if let Some(sub_model_name) = &meta.model_name {
            let sub_canonical = canonicalize(sub_model_name);
            project_models
                .get(sub_canonical.as_ref())
                .map(|sm| compute_layout(db, *sm, project).n_slots)
                .unwrap_or(1)
        } else {
            1
        }
    } else {
        // An *arrayed* implicit helper (the GH #541 bare-arrayed-PREVIOUS case)
        // occupies one slot per element; `meta.size` is its element count (1
        // for the scalar helpers that are every other implicit var). The
        // mini-layout self-size MUST match the `compute_layout` allocation, or
        // the helper's per-element writes spill into the next variable's slots.
        meta.size
    };
    mini_metadata.insert(
        var_ident_canonical.clone(),
        crate::compiler::VariableMetadata {
            offset: mini_offset,
            size: self_size,
            var: &lowered,
        },
    );
    mini_offset += self_size;

    // Implicit vars' deps are always explicit vars in the same model (or other implicit vars)
    // Keep dependency context conservative for implicit vars as well: both
    // branches of `if isModuleInput(...)` may still be compiled. The empty
    // `ModuleInputSet` reproduces the old `None`-inputs path.
    let deps = variable_direct_dependencies(
        db,
        meta.parent_source_var,
        project,
        module_ident_context,
        ModuleInputSet::empty(db),
    );
    let implicit_dep = deps
        .implicit_vars
        .iter()
        .find(|iv| canonicalize(&iv.name) == canonicalize(&implicit_name));

    let all_dep_names: BTreeSet<String> = if let Some(iv_deps) = implicit_dep {
        iv_deps
            .dt_deps
            .iter()
            .chain(iv_deps.initial_deps.iter())
            // Lookup tables referenced by this implicit var are layout
            // references, not data-flow deps -- include them so the fragment's
            // metadata + tables map can resolve `LOOKUP(table, x)` (#606).
            .chain(iv_deps.referenced_tables.iter())
            .cloned()
            .collect()
    } else {
        BTreeSet::new()
    };

    let mut extra_dep_names: Vec<String> = Vec::new();
    if meta.is_stock
        && let crate::variable::Variable::Stock {
            inflows, outflows, ..
        } = &lowered
    {
        for flow_name in inflows.iter().chain(outflows.iter()) {
            let canonical = flow_name.as_str().to_string();
            if !all_dep_names.contains(&canonical) {
                extra_dep_names.push(canonical);
            }
        }
    }

    let source_vars = model.variables(db);
    let implicit_info = model_implicit_var_info(db, model, project);
    let all_names: Vec<&String> = all_dep_names.iter().chain(extra_dep_names.iter()).collect();
    let mut dep_variables: Vec<(Ident<Canonical>, crate::variable::Variable, usize)> = Vec::new();
    let mut extra_module_refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
    let mut extra_submodels: HashMap<String, SourceModel> = HashMap::new();

    for dep_name in &all_names {
        let effective_name = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());

        if effective_name == implicit_name.as_str()
            || matches!(
                effective_name,
                "time" | "dt" | "initial_time" | "final_time"
            )
        {
            continue;
        }

        if let Some(dot_pos) = effective_name.find('\u{00B7}') {
            let module_var_name = &effective_name[..dot_pos];
            let module_ident: Ident<Canonical> = Ident::new(module_var_name);

            if mini_metadata.contains_key(&module_ident) {
                continue;
            }

            if let Some(mod_source_var) = source_vars.get(module_var_name) {
                if mod_source_var.kind(db) == SourceVariableKind::Module {
                    let mod_model_name = mod_source_var.model_name(db);
                    let sub_canonical = canonicalize(mod_model_name);
                    let sub_size = project_models
                        .get(sub_canonical.as_ref())
                        .map(|sm| compute_layout(db, *sm, project).n_slots)
                        .unwrap_or(1);

                    let mod_input_prefix = format!("{module_var_name}\u{00B7}");
                    let module_inputs = build_module_inputs(
                        model.name(db),
                        &mod_input_prefix,
                        mod_source_var
                            .module_refs(db)
                            .iter()
                            .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                    );

                    let mod_var = crate::variable::Variable::Module {
                        ident: module_ident.clone(),
                        model_name: Ident::new(mod_model_name),
                        units: None,
                        inputs: module_inputs.clone(),
                        errors: vec![],
                        unit_errors: vec![],
                    };
                    dep_variables.push((module_ident.clone(), mod_var, sub_size));

                    let input_set: BTreeSet<Ident<Canonical>> =
                        module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                    extra_module_refs.insert(module_ident, (Ident::new(mod_model_name), input_set));

                    if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                        extra_submodels.insert(mod_model_name.to_string(), *sub_model);
                    }
                }
            } else if let Some(im_meta) = implicit_info.get(module_var_name)
                && im_meta.is_module
                && let Some(im_model_name) = im_meta.model_name.as_deref()
            {
                let sub_canonical = canonicalize(im_model_name);
                let sub_size = project_models
                    .get(sub_canonical.as_ref())
                    .map(|sm| compute_layout(db, *sm, project).n_slots)
                    .unwrap_or(1);

                let input_prefix = format!("{module_var_name}\u{00B7}");
                let module_inputs = parsed
                    .implicit_vars
                    .iter()
                    .find_map(|iv| match iv {
                        datamodel::Variable::Module(dm_module)
                            if canonicalize(dm_module.ident.as_str()) == module_var_name =>
                        {
                            Some(build_module_inputs(
                                model.name(db),
                                &input_prefix,
                                dm_module
                                    .references
                                    .iter()
                                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
                            ))
                        }
                        _ => None,
                    })
                    .unwrap_or_default();

                let mod_var = crate::variable::Variable::Module {
                    ident: module_ident.clone(),
                    model_name: Ident::new(im_model_name),
                    units: None,
                    inputs: module_inputs.clone(),
                    errors: vec![],
                    unit_errors: vec![],
                };
                dep_variables.push((module_ident.clone(), mod_var, sub_size));

                let input_set: BTreeSet<Ident<Canonical>> =
                    module_inputs.iter().map(|mi| mi.dst.clone()).collect();
                extra_module_refs.insert(module_ident, (Ident::new(im_model_name), input_set));

                if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                    extra_submodels.insert(im_model_name.to_string(), *sub_model);
                }
            }
            continue;
        }

        let dep_ident = Ident::new(effective_name);
        if mini_metadata.contains_key(&dep_ident) {
            continue;
        }

        if let Some(dep_source_var) = source_vars.get(effective_name) {
            let dep_dims = variable_dimensions(db, *dep_source_var, project);
            let dep_size = variable_size(db, *dep_source_var, project);
            let dep_var = build_stub_variable(db, dep_source_var, &dep_ident, dep_dims);
            dep_variables.push((dep_ident, dep_var, dep_size));
        } else if let Some(implicit_meta) = implicit_info.get(effective_name) {
            // Dep is another implicit var. Almost always scalar, but an arrayed
            // implicit helper (GH #541) needs its array shape in the stub so a
            // subscript reference resolves (mirroring the source-var path in
            // `var_fragment.rs`); a scalar stub would reject the subscript.
            let is_stock = implicit_meta.is_stock;
            let dep_dims: Vec<crate::dimensions::Dimension> = if implicit_meta.dimensions.is_empty()
            {
                Vec::new()
            } else {
                implicit_meta
                    .dimensions
                    .iter()
                    .filter_map(|dim_name| {
                        converted_dims
                            .iter()
                            .find(|d| d.name() == dim_name.as_str())
                            .cloned()
                    })
                    .collect()
            };
            let dummy_ast = if dep_dims.is_empty() {
                None
            } else {
                Some(crate::ast::Ast::ApplyToAll(
                    dep_dims,
                    crate::ast::Expr2::Const("0".to_string(), 0.0, crate::ast::Loc::default()),
                ))
            };
            let dep_var = if is_stock {
                crate::variable::Variable::Stock {
                    ident: dep_ident.clone(),
                    init_ast: dummy_ast,
                    eqn: None,
                    units: None,
                    inflows: vec![],
                    outflows: vec![],
                    non_negative: false,
                    errors: vec![],
                    unit_errors: vec![],
                }
            } else {
                crate::variable::Variable::Var {
                    ident: dep_ident.clone(),
                    ast: dummy_ast,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    tables: vec![],
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                }
            };
            dep_variables.push((dep_ident, dep_var, implicit_meta.size));
        }
    }

    for (dep_ident, dep_var, dep_size) in &dep_variables {
        if !mini_metadata.contains_key(dep_ident) {
            mini_metadata.insert(
                dep_ident.clone(),
                crate::compiler::VariableMetadata {
                    offset: mini_offset,
                    size: *dep_size,
                    var: dep_var,
                },
            );
            mini_offset += dep_size;
        }
    }

    let mut all_metadata: HashMap<
        Ident<Canonical>,
        HashMap<Ident<Canonical>, crate::compiler::VariableMetadata<'_>>,
    > = HashMap::new();
    all_metadata.insert(model_name_ident.clone(), mini_metadata);

    for sub_model in extra_submodels.values() {
        build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
    }

    let mini_layout =
        crate::compiler::symbolic::layout_from_metadata(&all_metadata, &model_name_ident)
            .unwrap_or_else(|_| VariableLayout::new(HashMap::new(), 0));
    let rmap = ReverseOffsetMap::from_layout(&mini_layout);

    let mut tables: HashMap<Ident<Canonical>, Vec<crate::compiler::Table>> = HashMap::new();
    {
        let gf_tables = lowered.tables();
        if !gf_tables.is_empty() {
            let table_results: crate::Result<Vec<crate::compiler::Table>> = gf_tables
                .iter()
                .map(|t| crate::compiler::Table::new(&implicit_name, t))
                .collect();
            match table_results {
                Ok(ts) if !ts.is_empty() => {
                    tables.insert(var_ident_canonical.clone(), ts);
                }
                Err(_) => return None,
                _ => {}
            }
        }
    }

    for dep_name in &all_names {
        let effective = dep_name
            .as_str()
            .strip_prefix('\u{00B7}')
            .unwrap_or(dep_name.as_str());
        if effective.contains('\u{00B7}') {
            continue;
        }
        let dep_canonical: Ident<Canonical> = Ident::new(effective);
        if tables.contains_key(&dep_canonical) {
            continue;
        }
        if let Some(dep_sv) = source_vars.get(effective) {
            let dep_tables = extract_tables_from_source_var(db, dep_sv, project);
            if !dep_tables.is_empty() {
                tables.insert(dep_canonical, dep_tables);
            }
        }
    }

    let inputs = canonical_module_input_set(module_input_names);
    let (module_models, mut module_refs) = if meta.is_module {
        let mm = model_module_map(db, model, project).clone();

        // Build module_refs from the implicit var's datamodel::Module references,
        // stripping the module ident prefix from dst (matching compile_var_fragment
        // and enumerate_module_instances_inner).
        let mut refs: HashMap<Ident<Canonical>, crate::vm::ModuleKey> = HashMap::new();
        if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
            let input_prefix = format!("{implicit_name}\u{00B7}");
            let input_set: BTreeSet<Ident<Canonical>> = dm_module
                .references
                .iter()
                .filter_map(|mr| {
                    let dst_canonical = canonicalize(&mr.dst);
                    let bare = dst_canonical.strip_prefix(&input_prefix)?;
                    Some(Ident::new(bare))
                })
                .collect();
            refs.insert(
                var_ident_canonical.clone(),
                (Ident::new(&dm_module.model_name), input_set),
            );

            // Populate sub-model metadata
            let sub_canonical = canonicalize(&dm_module.model_name);
            if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                build_submodel_metadata(&arena, db, *sub_model, project, &mut all_metadata);
            }
        }

        (mm, refs)
    } else {
        (HashMap::new(), HashMap::new())
    };
    module_refs.extend(extra_module_refs);

    let core = crate::compiler::ContextCore {
        dimensions: converted_dims,
        dimensions_ctx: dim_context,
        model_name: &model_name_ident,
        metadata: &all_metadata,
        module_models: &module_models,
        inputs: &inputs,
    };

    let var = crate::compiler::Var::new(
        &crate::compiler::Context::new(core, &var_ident_canonical, is_initial),
        &lowered,
    )
    .ok()?;

    // Offsets in the per-variable form `compile_phase_to_per_var_bytecodes`
    // expects, built from the mini-layout `all_metadata` exactly as the
    // former inline `compile_phase` closure built them (so the shared
    // compile+symbolize tail is byte-identical to the prior per-implicit
    // behavior -- this replaces the verbatim-duplicate closure with the
    // single shared relation).
    let offsets: PerVarOffsetMap = all_metadata
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                v.iter()
                    .map(|(vk, vm)| (vk.clone(), (vm.offset, vm.size)))
                    .collect(),
            )
        })
        .collect();

    compile_phase_to_per_var_bytecodes(
        &var.ast,
        &offsets,
        &rmap,
        &tables,
        &module_refs,
        mini_offset,
        converted_dims,
        dim_context,
        &model_name_ident,
        &var_ident_canonical,
        &inputs,
    )
}
