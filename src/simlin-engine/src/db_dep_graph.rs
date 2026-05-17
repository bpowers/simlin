// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dt-phase dependency-graph cycle relation + Condition-2 SCC accessor.
//!
//! This module owns the single shared definition of the **dt-phase cycle
//! relation** (`dt_walk_successors`) and the `VarInfo` map builder
//! (`build_var_info`) that `crate::db::model_dependency_graph_impl`'s
//! `compute_inner` consumes. `dt_walk_successors` is consumed by BOTH the
//! production cycle detector AND the `#[cfg(test)]` Condition-2 SCC
//! introspection accessor (`dt_cycle_sccs`); one definition used twice is
//! what makes the Condition-2 gate's relation the engine's relation *by
//! construction* (B1-design.md §10b). The same primitive is what B1's
//! gate-1 (task #14) reuses, so it is production code, not scaffolding.
//!
//! This is a top-level module (a sibling of `db`, like `db_ltm_ir` /
//! `db_macro_registry`) rather than a submodule of `db.rs` purely to keep
//! `db.rs` under the per-file line cap; callers in `db` use
//! `crate::db_dep_graph::{VarInfo, build_var_info, dt_walk_successors}`.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::canonicalize;
use crate::db::{
    Db, SourceModel, SourceProject, SourceVariableKind, model_module_ident_context,
    variable_direct_dependencies_with_context,
    variable_direct_dependencies_with_context_and_inputs,
};

#[cfg(test)]
use crate::common::{Canonical, Ident};
#[cfg(test)]
use crate::db::{CompilationDiagnostic, DiagnosticError, model_dependency_graph};

/// Per-variable dependency facts used to build the model dependency
/// graph.
///
/// Hoisted to module scope (it was a fn-local struct inside
/// `model_dependency_graph_impl`) so the shared `build_var_info` builder
/// and the `dt_walk_successors` cycle-relation primitive can name it.
/// `dt_walk_successors` is consumed by BOTH the production cycle detector
/// and the `#[cfg(test)]` `dt_cycle_sccs` introspection accessor.
pub(crate) struct VarInfo {
    pub(crate) is_stock: bool,
    pub(crate) is_module: bool,
    pub(crate) dt_deps: BTreeSet<String>,
    pub(crate) initial_deps: BTreeSet<String>,
}

/// The dt-phase cycle-successor set of `name`: exactly the deps
/// `compute_inner`'s normal-node loop iterates for cycle detection in the
/// dt phase.
///
/// This is the single shared definition of the dt-phase cycle relation,
/// consumed by BOTH the production cycle detector (`compute_inner`, dt
/// branch) AND the `#[cfg(test)]` SCC introspection accessor
/// (`dt_cycle_sccs`). One definition used twice is what makes the
/// Condition-2 gate's relation the engine's relation *by construction*
/// (B1-design.md §10b -- this ends the "re-derive the relation and get it
/// subtly wrong" footgun class); it is also the gate-1 primitive B1
/// itself needs, hence production code, not test scaffolding.
///
/// Returns `[]` when `name`:
/// * is absent from `var_info` (a malformed/unknown entry -- no panic;
///   `compute_inner` likewise early-returns `Ok(())` for an unknown name,
///   and the dep loop skips unknown deps before recursing),
/// * is a Stock (a stock is a dt-phase sink -- the
///   `info.is_stock && !is_initial` early-return in `compute_inner`),
/// * is a Module (`compute_inner` returns for a module *before*
///   `processing.insert`, so a module is never on the DFS stack and can
///   never carry a cycle).
///
/// Otherwise returns `var_info[name].dt_deps` filtered to deps `d` with
/// `var_info.contains_key(d) && !var_info[d].is_stock`: unknown deps
/// dropped (error reported elsewhere) and stock-targeted deps dropped (a
/// stock breaks the dt chain). Module-targeted deps are KEPT -- a module
/// node has no successors so Tarjan cannot route a cycle through it,
/// matching `compute_inner` exactly (its `!dep_info.is_module` guard
/// governs only transitive *absorption*, not which deps the loop
/// iterates). Lagged deps are already absent (pruned when
/// `var_info.dt_deps` is built). Returned slices borrow `var_info`'s
/// `dt_deps` keys and iterate in `BTreeSet` (sorted) order, so the
/// relation is byte-stable across runs.
pub(crate) fn dt_walk_successors<'a>(
    var_info: &'a HashMap<String, VarInfo>,
    name: &str,
) -> Vec<&'a str> {
    let Some(info) = var_info.get(name) else {
        return Vec::new();
    };
    if info.is_stock || info.is_module {
        return Vec::new();
    }
    info.dt_deps
        .iter()
        .filter(|dep| {
            var_info
                .get(dep.as_str())
                .map(|d| !d.is_stock)
                .unwrap_or(false)
        })
        .map(|dep| dep.as_str())
        .collect()
}

/// Build the per-variable `VarInfo` map (plus the set of variables
/// referenced by `INIT()`) for `model` under the given module-input
/// wiring.
///
/// Shared verbatim by `model_dependency_graph_impl` and the
/// `#[cfg(test)]` `dt_cycle_sccs` accessor so the Condition-2 gate
/// observes the *exact* `var_info` the engine builds -- never a
/// reconstruction (B1-design.md §10b).
pub(crate) fn build_var_info(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> (HashMap<String, VarInfo>, HashSet<String>) {
    let source_vars = model.variables(db);
    let module_input_names = module_input_names.to_vec();
    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.clone());

    let mut var_info: HashMap<String, VarInfo> = HashMap::new();
    let mut all_init_referenced: HashSet<String> = HashSet::new();

    let normalize_dep = |dep: &str| -> String {
        let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
        if let Some(dot_pos) = effective.find('\u{00B7}') {
            effective[..dot_pos].to_string()
        } else {
            effective.to_string()
        }
    };
    let normalize_deps = |deps: &BTreeSet<String>| -> BTreeSet<String> {
        deps.iter().map(|d| normalize_dep(d)).collect()
    };

    let project_models = project.models(db);

    for (name, source_var) in source_vars.iter() {
        let deps = if module_input_names.is_empty() {
            variable_direct_dependencies_with_context(
                db,
                *source_var,
                project,
                module_ident_context,
            )
        } else {
            variable_direct_dependencies_with_context_and_inputs(
                db,
                *source_var,
                project,
                module_ident_context,
                module_input_names.clone(),
            )
        };
        let init_only_dt = deps.dt_init_only_referenced_vars.clone();
        let lagged_dt_previous = deps.dt_previous_referenced_vars.clone();
        let lagged_initial_previous = deps.initial_previous_referenced_vars.clone();
        let kind = source_var.kind(db);
        let mut dt_deps = if kind == SourceVariableKind::Module {
            deps.dt_deps
                .iter()
                .filter(|dep| {
                    let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
                    if let Some(dot_pos) = effective.find('\u{00B7}') {
                        let module_name = &effective[..dot_pos];
                        let var_name = &effective[dot_pos + '\u{00B7}'.len_utf8()..];
                        let sub_canonical = canonicalize(module_name);
                        if let Some(sub_model) = project_models.get(sub_canonical.as_ref()) {
                            let sub_vars = sub_model.variables(db);
                            if let Some(sub_var) = sub_vars.get(var_name) {
                                return sub_var.kind(db) != SourceVariableKind::Stock;
                            }
                        }
                        true
                    } else {
                        true
                    }
                })
                .cloned()
                .collect()
        } else {
            deps.dt_deps.clone()
        };
        dt_deps.retain(|dep| !init_only_dt.contains(dep));
        dt_deps.retain(|dep| !lagged_dt_previous.contains(dep));
        let mut initial_deps = deps.initial_deps.clone();
        initial_deps.retain(|dep| !lagged_initial_previous.contains(dep));

        var_info.insert(
            name.clone(),
            VarInfo {
                is_stock: kind == SourceVariableKind::Stock,
                is_module: kind == SourceVariableKind::Module,
                dt_deps: normalize_deps(&dt_deps),
                initial_deps: normalize_deps(&initial_deps),
            },
        );
        all_init_referenced.extend(deps.init_referenced_vars.iter().cloned());

        // Include implicit variables from this variable's deps result.
        // Since we read this from variable_direct_dependencies (not
        // parse_source_variable_with_module_context), salsa's backdating
        // ensures that if the deps + implicit vars haven't changed, this
        // function is cached.
        for implicit in &deps.implicit_vars {
            let mut dt_deps = implicit.dt_deps.clone();
            dt_deps.retain(|dep| !implicit.dt_init_only_referenced_vars.contains(dep));
            dt_deps.retain(|dep| !implicit.dt_previous_referenced_vars.contains(dep));
            let mut initial_deps = implicit.initial_deps.clone();
            initial_deps.retain(|dep| !implicit.initial_previous_referenced_vars.contains(dep));
            var_info.insert(
                implicit.name.clone(),
                VarInfo {
                    is_stock: implicit.is_stock,
                    is_module: implicit.is_module,
                    dt_deps: normalize_deps(&dt_deps),
                    initial_deps: normalize_deps(&initial_deps),
                },
            );
        }
    }

    (var_info, all_init_referenced)
}

/// Strongly-connected components of the **real** dt-phase cycle relation
/// (`dt_walk_successors`), for the Condition-2 verification harness
/// (B1-design.md §10b).
///
/// `multi` is every SCC of size >= 2 (a true multi-node cycle);
/// `self_loops` is every node with a direct dt self-edge `v -> v` (a
/// size-1 SCC Tarjan does not surface in `multi`). Both are
/// sorted/byte-stable.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DtCycleSccs {
    pub multi: Vec<BTreeSet<Ident<Canonical>>>,
    pub self_loops: BTreeSet<Ident<Canonical>>,
}

/// Introspect the engine's dt-phase cycle relation on a compiled model.
///
/// Builds `var_info` via the exact builder `model_dependency_graph_impl`
/// uses (`build_var_info` -- never a reconstruction) and runs the
/// uncapped iterative Tarjan (`crate::ltm::scc_components`, the
/// D1/F2-hardened primitive) over the adjacency defined by
/// `dt_walk_successors` for every node. Because this accessor and
/// `compute_inner` consume the *same* `dt_walk_successors`, the reported
/// SCC set IS the engine's dt-phase cycle relation by construction
/// (B1-design.md §10b -- nothing is re-derived; one relation, used
/// twice). The accompanying tests cross-check `multi` against the engine
/// actually raising `ErrorCode::CircularDependency` (§10b step 4
/// footgun-proofing).
///
/// `#[cfg(test)]` accessor wrapper only: the production consumer of the
/// same `dt_walk_successors` + Tarjan primitive is B1's gate-1 (task
/// #14). Uses the default (no module-input) wiring -- the same
/// `model_dependency_graph` the `simulates_clearn` path compiles.
#[cfg(test)]
pub(crate) fn dt_cycle_sccs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> DtCycleSccs {
    let (var_info, _all_init_referenced) = build_var_info(db, model, project, &[]);

    // Adjacency = exactly `dt_walk_successors` for every node. var_info
    // keys are canonical (canonicalized at sync time), so wrapping them
    // unchecked is sound.
    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> =
        HashMap::with_capacity(var_info.len());
    let mut self_loops: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    for name in var_info.keys() {
        let succ = dt_walk_successors(&var_info, name);
        if succ.contains(&name.as_str()) {
            self_loops.insert(Ident::from_str_unchecked(name));
        }
        edges.insert(
            Ident::from_str_unchecked(name),
            succ.iter()
                .copied()
                .map(Ident::from_str_unchecked)
                .collect(),
        );
    }

    let multi: Vec<BTreeSet<Ident<Canonical>>> = crate::ltm::scc_components(&edges)
        .into_iter()
        .filter(|component| component.len() >= 2)
        .map(|component| component.into_iter().collect())
        .collect();

    DtCycleSccs { multi, self_loops }
}

/// Pure §10b step-4 consistency predicate (functional core).
///
/// The instrumented dt-phase SCC set is engine-consistent iff "the
/// instrumentation reports SOME dt cycle" agrees with "the engine raised
/// `CircularDependency` on the same compiled model". A dt multi-node SCC
/// or a dt self-loop necessarily makes `compute_transitive(false)` Err
/// (=> `CircularDependency`), so a reported cycle must always coincide
/// with the diagnostic (no invented false positive); and -- under the
/// harness premise that the init-phase relation is acyclic by
/// construction (B1-design.md §10a; true for every harness fixture and
/// asserted for post-B3 C-LEARN by §10c's INITIAL/const-leaf prediction)
/// -- the converse holds too (no missed cycle).
///
/// Returns `Some(reason)` iff the two diverge (=> STOP, do NOT gate: the
/// instrumentation, or the init-acyclic premise, is wrong --
/// B1-design.md §10b step 4); `None` iff consistent.
#[cfg(test)]
fn dt_cycle_sccs_consistency_violation(
    sccs: &DtCycleSccs,
    engine_raises_circular: bool,
) -> Option<String> {
    let instrumented_reports_cycle = !sccs.multi.is_empty() || !sccs.self_loops.is_empty();
    if instrumented_reports_cycle == engine_raises_circular {
        return None;
    }
    Some(format!(
        "dt-phase SCC instrumentation diverges from the engine's real \
         CircularDependency flagging on the SAME compiled model \
         (instrumented_reports_cycle={instrumented_reports_cycle}, \
         engine_raises_circular={engine_raises_circular}; \
         multi={:?}, self_loops={:?}). Per B1-design.md §10b step 4 the \
         instrumentation (or the init-acyclic premise) is wrong -- STOP, \
         do not gate on a mis-derived relation.",
        sccs.multi, sccs.self_loops
    ))
}

/// §10b step-4 footgun-proofing made first-class: the dt-phase SCC set,
/// returned ONLY after it is cross-checked against the engine's REAL
/// `CircularDependency` flagging on the SAME compiled model.
///
/// Panics (STOP -- do not gate on a mis-derived relation) on any
/// divergence. The §10c/§10d adversarial Condition-2 run consumes THIS
/// (then layers the array-producing-membership assertion on top), so the
/// gate's relation is the engine's *by construction* (shared
/// `dt_walk_successors`) AND cross-checked on every invocation -- the
/// reviewer's scrutiny-target-ii binding invariant.
///
/// `#[cfg(test)]` accessor wrapper only (like `dt_cycle_sccs`).
#[cfg(test)]
pub(crate) fn dt_cycle_sccs_engine_consistent(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> DtCycleSccs {
    let sccs = dt_cycle_sccs(db, model, project);
    let _ = model_dependency_graph(db, model, project);
    let diags = model_dependency_graph::accumulated::<CompilationDiagnostic>(db, model, project);
    let engine_raises_circular = diags.iter().any(|d| {
        matches!(
            d.0.error,
            DiagnosticError::Model(crate::common::Error {
                code: crate::common::ErrorCode::CircularDependency,
                ..
            })
        )
    });
    if let Some(reason) = dt_cycle_sccs_consistency_violation(&sccs, engine_raises_circular) {
        panic!("{reason}");
    }
    sccs
}

/// §10b step-3 Layer-2: the set of main-model variables whose OWN
/// production-lowered per-element `Vec<Expr>` is, or recursively
/// contains, an array-producing builtin
/// (VectorElmMap/VectorSortOrder/Rank/AllocateAvailable/AllocateByPriority)
/// -- the §4-P5 can't-flat-split risk set. Sources each variable's
/// lowered `Vec<Expr>` from the engine's OWN per-variable production
/// compile on the SAME salsa-cached `(db, model, project)` state
/// `dt_cycle_sccs_engine_consistent` observes (constraint §26.4-(c)4,
/// the binding identical-universe correctness precondition), then applies
/// the Layer-1 predicate `crate::compiler::exprs_contain_array_producing_builtin`.
/// Sorted/byte-stable. Identical `(db, model, project)` triple to
/// `dt_cycle_sccs_engine_consistent` so the §10c/§10d RUN computes
/// `{multi ∪ self_loops} ∩ array_producing_vars` directly.
///
/// Universe = the IDENTICAL `build_var_info(.., &[])` keyset
/// `dt_cycle_sccs` iterates (C4-a). Each variable's lowered `Vec<Expr>`
/// is sourced from the engine's OWN per-variable production lowering via
/// `var_noninitial_lowered_exprs` (the post-Commit-R
/// `crate::db_var_fragment::lower_var_fragment` surface; never a
/// re-derivation -- B1-design.md §10b step 3 / §26.4 C1-provenance), and
/// the COMPLETE list is fed to the Layer-1 predicate (the `&[Expr]` type
/// enforces completeness of what Layer-1 scans; sourcing the real
/// `lower_var_fragment` output enforces completeness of what Layer-2
/// sources -- the §26.5/§26.6 spine; NEVER a hoist-set subset).
/// `var_noninitial_lowered_exprs` ABORTs (never silent-skips) on any
/// universe variable whose production lowered exprs cannot be sourced
/// (C4-b) -- a silent skip would be the §10c/§10d incomplete-sourcing
/// false-negative the spine forbids.
#[cfg(test)]
pub(crate) fn array_producing_vars(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> BTreeSet<Ident<Canonical>> {
    // C4-a: the IDENTICAL universe `dt_cycle_sccs` uses -- the same
    // `build_var_info(.., &[])` keyset on the same `(db, model, project)`
    // triple -- so the §10c/§10d RUN intersects `{multi ∪ self_loops}`
    // and this set over ONE universe (the binding identical-universe
    // precondition §26.4-(c)4).
    let (var_info, _all_init_referenced) = build_var_info(db, model, project, &[]);

    let mut out: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    for name in var_info.keys() {
        let exprs = var_noninitial_lowered_exprs(db, model, project, name);
        if crate::compiler::exprs_contain_array_producing_builtin(&exprs) {
            out.insert(Ident::from_str_unchecked(name));
        }
    }
    out
}

/// The engine's OWN per-variable production-lowered non-initial (dt/flow)
/// `Vec<Expr>` for the canonical `var_name`.
///
/// Sourced via `crate::db_var_fragment::lower_var_fragment` -- the exact
/// per-variable lowering the production caller `crate::db::compile_var_fragment`
/// runs -- with the caller-owned, lowering-independent context constructed
/// byte-identically to that caller (same helpers, same order:
/// `source_dims_to_datamodel` -> `DimensionsContext`/`Dimension`,
/// `model.name`, `model_module_map`) and the default no-module-input
/// wiring `dt_cycle_sccs` uses (`build_var_info(.., &[])` => `is_root =
/// true`, empty module inputs). This is the engine's real lowering, never
/// a re-derivation (B1-design.md §10b step 3 / §26.4 C1-provenance; the
/// §26.30 forward note pins `lower_var_fragment` as the Layer-2 source
/// surface). The non-initial phase is the dt phase the §10c/§10d RUN
/// intersects (`dt_cycle_sccs` is the dt-phase relation), so Layer-2
/// membership is dt-phase-consistent with the cycle set it is
/// intersected against.
///
/// C4-b footgun-proofing: ABORT (panic -- never silent-skip) when a
/// universe variable's non-initial production lowered exprs cannot be
/// sourced: no `SourceVariable` (an implicit SMOOTH/DELAY/INIT helper --
/// it has no `lower_var_fragment` entry; sourcing implicit vars is the
/// deferred §10c/§10d-RUN concern and is loudly surfaced here rather than
/// silently mis-classified), `LoweredVarFragment::Fatal` (the variable
/// did not lower at all), or the non-initial phase's `Var::new` errored.
///
/// §26.46 Ruling-2 (SPIRIT pinned; the literal "abort only on whole-var
/// `Fatal`" reading was rejected as unsound): an incompletely-sourced
/// production `Vec<Expr>` ⇒ `array_producing_vars` misses an
/// array-producing `App` the COMPLETE lowering would have ⇒ a
/// false-negative ⇒ `{multi ∪ self_loops} ∩ array_producing_vars`
/// under-includes ⇒ a §10c/§10d/§4-P5 false-green -- the exact footgun
/// C4-b exists to prevent. So C4-b MUST abort (loud, never
/// silent-proceed) on ANY incomplete sourcing for a C4-a-universe
/// variable, not merely whole-var `Fatal`. The conservative superset
/// (abort on any phase `Var::new` Err, incl. an initial-only error) is
/// acceptable/preferred -- strictly safer, with no spurious-abort
/// downside on a well-formed model.
#[cfg(test)]
pub(crate) fn var_noninitial_lowered_exprs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: &str,
) -> Vec<crate::compiler::Expr> {
    use crate::db_var_fragment::{LoweredVarFragment, lower_var_fragment};

    let source_vars = model.variables(db);
    let Some(sv) = source_vars.get(var_name) else {
        panic!(
            "array_producing_vars: universe var {var_name:?} has no \
             SourceVariable (an implicit SMOOTH/DELAY/INIT helper -- \
             implicit-var sourcing is the deferred §10c/§10d-RUN concern) \
             -- C4-b ABORT, never silent-skip (a silent skip is the \
             §10c/§10d incomplete-sourcing false-negative)"
        );
    };

    // Caller-owned, lowering-independent context, built EXACTLY as
    // `crate::db::compile_var_fragment` builds it.
    let dm_dims = crate::db::source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dm_dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();
    let model_name_ident = Ident::new(model.name(db));
    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let module_models = crate::db::model_module_map(db, model, project).clone();

    let lowered = lower_var_fragment(
        db,
        *sv,
        model,
        project,
        true,
        &[],
        &converted_dims,
        &dim_context,
        &model_name_ident,
        &module_models,
        &inputs,
    );

    match lowered {
        LoweredVarFragment::Lowered {
            per_phase_lowered, ..
        } => match per_phase_lowered.noninitial {
            Ok(v) => v.ast,
            Err(e) => panic!(
                "array_producing_vars: universe var {var_name:?} non-initial \
                 Var::new errored ({e:?}) -- cannot source its production \
                 lowered exprs (C4-b ABORT, never silent-skip)"
            ),
        },
        LoweredVarFragment::Fatal { .. } => panic!(
            "array_producing_vars: universe var {var_name:?} failed to lower \
             (LoweredVarFragment::Fatal) -- cannot assess array-producing \
             membership (C4-b ABORT, never silent-skip)"
        ),
    }
}

#[cfg(test)]
#[path = "db_dep_graph_tests.rs"]
mod db_dep_graph_tests;
