// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Dt-phase dependency-graph cycle relation + SCC introspection accessor.
//!
//! This module owns the single shared definition of the **dt-phase cycle
//! relation** (`dt_walk_successors`) and the `VarInfo` map builder
//! (`build_var_info`) that `crate::db::model_dependency_graph_impl`'s
//! `compute_inner` consumes. `dt_walk_successors` is consumed by both the
//! production cycle detector and the `#[cfg(test)]` SCC introspection
//! accessor (`dt_cycle_sccs`). Defining the relation once and using it in
//! both places means the accessor observes the engine's actual cycle
//! relation rather than a re-derivation that could silently drift from it.
//!
//! This is a top-level module (a sibling of `db`, like `db_ltm_ir` /
//! `db_macro_registry`) rather than a submodule of `db.rs` purely to keep
//! `db.rs` under the per-file line cap; callers in `db` use
//! `crate::db_dep_graph::{VarInfo, build_var_info, dt_walk_successors}`.

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::canonicalize;
// `Canonical`/`Ident` are used in production by the element-cycle
// refinement (`resolve_recurrence_sccs` and friends), not only by the
// `#[cfg(test)]` SCC accessors.
use crate::common::{Canonical, Ident};
use crate::db::{
    Db, SourceModel, SourceProject, SourceVariableKind, model_module_ident_context,
    variable_direct_dependencies_with_context,
    variable_direct_dependencies_with_context_and_inputs,
};

#[cfg(test)]
use crate::db::{CompilationDiagnostic, DiagnosticError, model_dependency_graph};

/// Per-variable dependency facts used to build the model dependency
/// graph.
///
/// Lives at module scope (rather than fn-local in
/// `model_dependency_graph_impl`) so the shared `build_var_info` builder
/// and the `dt_walk_successors` cycle-relation primitive can both name
/// it; `dt_walk_successors` is consumed by both the production cycle
/// detector and the `#[cfg(test)]` `dt_cycle_sccs` introspection
/// accessor.
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
/// consumed by both the production cycle detector (`compute_inner`, dt
/// branch) and the `#[cfg(test)]` SCC introspection accessor
/// (`dt_cycle_sccs`). Defining it once and using it in both places is
/// what makes the accessor's relation the engine's relation by
/// construction, with no opportunity for a re-derivation to drift.
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

/// The init-phase cycle-successor set of `name`: exactly the deps
/// `compute_inner`'s normal-node loop iterates for cycle detection in the
/// **init** phase.
///
/// This is the single shared definition of the init-phase cycle relation
/// -- the exact analogue of `dt_walk_successors` -- consumed by both the
/// production cycle detector (`compute_inner`, init branch) and the
/// init-phase per-element recurrence resolution. Defining it once and
/// using it in both places makes the resolution's relation the engine's
/// relation by construction, with no opportunity for a re-derivation to
/// drift (the same "single shared relation, never re-derive" pattern
/// `dt_walk_successors` follows).
///
/// Returns `[]` when `name`:
/// * is absent from `var_info` (a malformed/unknown entry -- no panic;
///   `compute_inner` likewise early-returns `Ok(())` for an unknown name,
///   and the dep loop skips unknown deps before recursing),
/// * is a Module (`compute_inner` returns for a module *before*
///   `processing.insert` in BOTH phases, so a module is never on the DFS
///   stack and can never carry a cycle in the init phase either).
///
/// Crucially, a **Stock is NOT an init-phase sink** (unlike
/// `dt_walk_successors`, where `info.is_stock` short-circuits to `[]`):
/// `compute_inner`'s stock sink is `info.is_stock && !is_initial`, so it
/// does not fire in the init phase. A stock's initial value is a genuine
/// init-relation node, so a stock whose init equation references itself
/// is a real init self-loop and its init deps are its cycle successors.
///
/// Otherwise returns `var_info[name].initial_deps` filtered ONLY to deps
/// `d` with `var_info.contains_key(d)`: unknown deps dropped (error
/// reported elsewhere) -- **no stock filter and no stock sink** (a
/// stock-targeted init dep is a real init dependency, kept). This exactly
/// reproduces the inlined init logic `compute_inner` runs
/// (`info.initial_deps.iter().filter(|dep|
/// var_info.contains_key(dep))`). `initial_previous_referenced_vars` are
/// already absent (stripped when `var_info.initial_deps` is built in
/// `build_var_info`). Returned slices borrow `var_info`'s `initial_deps`
/// keys and iterate in `BTreeSet` (sorted) order, so the relation is
/// byte-stable across runs.
pub(crate) fn init_walk_successors<'a>(
    var_info: &'a HashMap<String, VarInfo>,
    name: &str,
) -> Vec<&'a str> {
    let Some(info) = var_info.get(name) else {
        return Vec::new();
    };
    // Only the module early-return applies in the init phase; the stock
    // sink is dt-only (`!is_initial`-gated in `compute_inner`).
    if info.is_module {
        return Vec::new();
    }
    info.initial_deps
        .iter()
        .filter(|dep| var_info.contains_key(dep.as_str()))
        .map(|dep| dep.as_str())
        .collect()
}

/// Build the per-variable `VarInfo` map (plus the set of variables
/// referenced by `INIT()`) for `model` under the given module-input
/// wiring.
///
/// Shared verbatim by `model_dependency_graph_impl` and the
/// `#[cfg(test)]` `dt_cycle_sccs` accessor so the accessor observes the
/// exact `var_info` the engine builds -- never a reconstruction.
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

/// Strongly-connected components of the real dt-phase cycle relation
/// (`dt_walk_successors`), for the `#[cfg(test)]` cycle-introspection
/// accessor.
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
/// uncapped iterative Tarjan (`crate::ltm::scc_components`) over the
/// adjacency defined by `dt_walk_successors` for every node. Because this
/// accessor and `compute_inner` consume the same `dt_walk_successors`,
/// the reported SCC set is the engine's dt-phase cycle relation by
/// construction -- nothing is re-derived. The accompanying tests
/// cross-check `multi` against the engine actually raising
/// `ErrorCode::CircularDependency`.
///
/// `#[cfg(test)]` accessor only. Uses the default (no module-input)
/// wiring -- the same `model_dependency_graph` the `simulates_clearn`
/// path compiles.
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

/// Pure consistency predicate (functional core), re-pointed to the
/// **element-level** cycle-resolution invariant.
///
/// The old invariant -- "the instrumentation reports some dt cycle" iff
/// "the engine raised `CircularDependency`" -- became false by design
/// once the element-cycle refinement landed: a single-variable
/// self-recurrence (`ecc[tNext]=ecc[tPrev]+1`) is still an instrumented
/// dt self-loop (the *whole-variable* `dt_walk_successors` relation is
/// unchanged), yet its induced *element* graph is acyclic, so the engine
/// resolves it (it appears in `ModelDepGraphResult.resolved_sccs`) and
/// does **not** raise `CircularDependency`. The re-pointed invariant is:
///
/// > For each instrumented SCC, the engine raises `CircularDependency`
/// > iff that SCC is **not** in `resolved_sccs` (an instrumented SCC
/// > whose induced element graph is acyclic AND element-sourceable is
/// > resolved and produces no diagnostic; one that is element-cyclic OR
/// > not element-sourceable is unresolved and the engine flags it).
///
/// `resolve_recurrence_sccs` resolves an SCC only when *every*
/// offending SCC is resolvable (`!has_unresolved`), so the engine is
/// all-or-nothing per model: either `resolved_sccs` covers every
/// instrumented SCC and no diagnostic is raised, or `resolved_sccs` is
/// empty and the diagnostic is raised. Under that behavior the
/// per-SCC iff collapses to the two checks below, which together are
/// exactly the re-pointed invariant for every state the engine can
/// produce:
///
/// 1. `engine_raises_circular == any instrumented SCC is NOT in
///    resolved_sccs` -- catches both an invented cycle the engine
///    neither resolved nor flagged and a missed cycle the engine flagged
///    but the instrumentation did not surface.
/// 2. every `ResolvedScc`'s members ARE an instrumented SCC -- catches
///    the refinement resolving something the shared dt relation never
///    saw as a cycle (the two relations drifted; the whole reason this
///    cross-check exists).
///
/// **Phase-scoping note.** This predicate cross-checks only the **dt**
/// path: `sccs` is the dt instrumentation (`dt_cycle_sccs` over
/// `dt_walk_successors`), so check (2) is scoped to `phase == Dt`
/// resolved SCCs. Phase 2 Task 3 added `phase: Initial` resolution for
/// init-only recurrences (a per-element recurrence in a stock's initial
/// value), which are *structurally distinct* from dt -- a stock breaks
/// the dt chain, so an init-only SCC is correctly NOT dt-instrumented
/// and is intentionally excluded from check (2). The init-phase
/// resolved-SCC set is instead policed by its own structural argument
/// (the init cycle gate breaks only init-element-acyclic single-variable
/// members' self-edges; any residual genuine init cycle still raises
/// `CircularDependency`, exercised directly by the init-phase tests). A
/// dedicated init-phase *instrumentation* + symmetric cross-check
/// (mirroring `dt_cycle_sccs`/this predicate for the init relation)
/// remains a later-task obligation; do not treat the absence of an init
/// cross-check as a guarantee that the init path needs none.
///
/// Returns `Some(reason)` iff the engine and the refinement diverge on
/// the same compiled model (=> stop, do not gate: the instrumentation or
/// the refinement is wrong); `None` iff consistent.
#[cfg(test)]
fn dt_cycle_sccs_consistency_violation(
    sccs: &DtCycleSccs,
    resolved_sccs: &[crate::db::ResolvedScc],
    engine_raises_circular: bool,
) -> Option<String> {
    // Each instrumented SCC as a member set: a multi-node SCC is already
    // a set; a self-loop `v` is the size-1 SCC `{v}`.
    let instrumented: Vec<BTreeSet<Ident<Canonical>>> = sccs
        .multi
        .iter()
        .cloned()
        .chain(
            sccs.self_loops
                .iter()
                .map(|v| std::iter::once(v.clone()).collect()),
        )
        .collect();
    let resolved_member_sets: Vec<&BTreeSet<Ident<Canonical>>> =
        resolved_sccs.iter().map(|s| &s.members).collect();

    // (1) An instrumented SCC the engine resolved is NOT a cycle; one it
    // did not resolve IS. Because the engine is all-or-nothing per model
    // (`resolve_recurrence_sccs` resolves nothing unless every
    // offending SCC is resolvable), "some instrumented SCC is unresolved"
    // is exactly the condition under which the engine raises the
    // diagnostic.
    let some_instrumented_unresolved = instrumented
        .iter()
        .any(|s| !resolved_member_sets.contains(&s));
    if engine_raises_circular != some_instrumented_unresolved {
        return Some(format!(
            "dt-phase SCC instrumentation diverges from the engine's \
             element-cycle resolution on the SAME compiled model: the \
             engine {} CircularDependency, but {} instrumented SCC is \
             absent from resolved_sccs (engine_raises_circular={}, \
             some_instrumented_unresolved={}; multi={:?}, \
             self_loops={:?}, resolved_sccs={:?}). The instrumentation \
             or the element-cycle refinement is wrong -- stop, do not \
             gate on a mis-derived relation.",
            if engine_raises_circular {
                "raised"
            } else {
                "did NOT raise"
            },
            if some_instrumented_unresolved {
                "some"
            } else {
                "no"
            },
            engine_raises_circular,
            some_instrumented_unresolved,
            sccs.multi,
            sccs.self_loops,
            resolved_sccs,
        ));
    }

    // (2) Every resolved **dt-phase** SCC must be one the dt
    // instrumentation actually surfaced. A `phase: Dt` `ResolvedScc`
    // whose members are not an instrumented SCC means the refinement
    // resolved a dt "cycle" the shared dt relation never saw -- the two
    // relations drifted.
    //
    // This is deliberately scoped to `phase == Dt`. A `phase: Initial`
    // `ResolvedScc` (Phase 2 Task 3 -- a per-element recurrence in a
    // stock's initial value) is *structurally distinct* from dt: a
    // stock breaks the dt chain, so `dt_walk_successors` reports NO dt
    // SCC for it. Cross-checking an init-only verdict against the dt
    // instrumentation would falsely flag a correct resolution. The dt
    // cross-check stays exactly as strong for the dt path (an
    // un-instrumented `phase: Dt` SCC is still a hard divergence); the
    // init-phase resolved-SCC set has its own structural argument (the
    // init cycle gate breaks only init-element-acyclic members'
    // self-edges; any residual genuine init cycle still raises
    // `CircularDependency`, exercised by the init-phase tests) and a
    // dedicated init instrumentation/cross-check is a later-task
    // obligation.
    if let Some(orphan) = resolved_sccs
        .iter()
        .filter(|s| s.phase == crate::db::SccPhase::Dt)
        .find(|s| !instrumented.contains(&s.members))
    {
        return Some(format!(
            "the element-cycle refinement resolved a dt-phase SCC the \
             shared dt-phase instrumentation never surfaced as a cycle \
             (resolved members={:?}; instrumented multi={:?}, \
             self_loops={:?}). The shared dt relation and the refinement \
             drifted -- stop, do not gate on a mis-derived relation.",
            orphan.members, sccs.multi, sccs.self_loops
        ));
    }

    None
}

/// The dt-phase SCC set, returned only after it is cross-checked against
/// the engine's real element-cycle resolution on the same compiled model.
///
/// Cross-checks the instrumented SCC set against BOTH the engine's
/// `CircularDependency` flagging AND its `ModelDepGraphResult
/// .resolved_sccs` (the element-cycle refinement's verdict), via
/// `dt_cycle_sccs_consistency_violation`'s re-pointed element-level
/// invariant: an instrumented SCC is resolved (in `resolved_sccs`, no
/// diagnostic) iff its induced element graph is acyclic and
/// element-sourceable; otherwise the engine flags it. Panics (do not gate
/// on a mis-derived relation) on any divergence. A consumer therefore
/// gets a relation that is the engine's by construction (shared
/// `dt_walk_successors`) and additionally cross-checked on every
/// invocation. (Phase-1 scoping: the init-phase relation is acyclic by
/// construction for every harness fixture today; Phase 2 generalizes
/// this -- see `dt_cycle_sccs_consistency_violation`.)
///
/// `#[cfg(test)]` accessor only (like `dt_cycle_sccs`).
#[cfg(test)]
pub(crate) fn dt_cycle_sccs_engine_consistent(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> DtCycleSccs {
    let sccs = dt_cycle_sccs(db, model, project);
    let dep_graph = model_dependency_graph(db, model, project);
    let resolved_sccs = dep_graph.resolved_sccs.clone();
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
    if let Some(reason) =
        dt_cycle_sccs_consistency_violation(&sccs, &resolved_sccs, engine_raises_circular)
    {
        panic!("{reason}");
    }
    sccs
}

/// The engine's own production-lowered per-element `Vec<Expr>` for
/// `var_name` in the requested phase, or `None` when it cannot be element-
/// sourced (no `SourceVariable`, `LoweredVarFragment::Fatal`, or the
/// phase's `Var::new` errored). `None` is the loud-safe signal: the
/// element-cycle refinement keeps the conservative `CircularDependency`
/// rather than emit a wrong run order. Sourced via
/// `crate::db_var_fragment::lower_var_fragment` -- the exact per-variable
/// lowering the production caller `crate::db::compile_var_fragment` runs --
/// never a re-derivation, with the caller-owned, lowering-independent
/// context constructed byte-identically to that caller (same helpers, same
/// order) and the default no-module-input wiring `build_var_info(.., &[])`
/// uses. Phase 3 extends the no-`SourceVariable` arm with parent-
/// `implicit_vars` sourcing; Phase 1 only needs the real-`SourceVariable`
/// happy path.
///
/// This is the **production** (non-panicking) sibling of the
/// `#[cfg(test)]` `var_noninitial_lowered_exprs`, which deliberately
/// *panics* on any incomplete sourcing (it backs `array_producing_vars`,
/// where a silent skip would under-count -- a false negative). That panic
/// wrapper is correct for its test-only consumer and is left unchanged;
/// production code reachable from the cycle gate cannot panic, so this
/// accessor returns `None` instead. The two share `lower_var_fragment` so
/// neither re-derives the lowering.
///
/// **Currently consumed only by its own `#[cfg(test)]` tests.** Phase 2
/// Subcomponent B (GH #575) rebuilt the SCC element graph on the
/// cross-member-comparable *symbolic* representation
/// (`symbolic_phase_element_order` via `var_phase_symbolic_fragment_prod`),
/// so the prior `Expr`-based `phase_element_order` consumer of this
/// accessor was removed. It is intentionally left in place (NOT deleted)
/// because Phase 3 extends its no-`SourceVariable` arm with parent-
/// `implicit_vars` sourcing and restores a production consumer; deleting
/// it now would force Phase 3 to reconstruct the byte-identical
/// context-mirroring it already gets right. The
/// `cfg_attr(not(test), allow(dead_code))` suppresses the otherwise-
/// correct unused warning for the non-test build until then.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn var_phase_lowered_exprs_prod(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: &str,
    phase: crate::db::SccPhase,
) -> Option<Vec<crate::compiler::Expr>> {
    use crate::common::Ident;
    use crate::db_var_fragment::{LoweredVarFragment, lower_var_fragment};

    let source_vars = model.variables(db);
    // No `SourceVariable` (an implicit SMOOTH/DELAY/INIT helper, or a
    // parent-sourced name): Phase 1 does not parent-source -- return
    // `None` (loud-safe), never panic. Phase 3 extends this arm.
    let sv = source_vars.get(var_name)?;

    // Caller-owned, lowering-independent context, built EXACTLY as
    // `crate::db::compile_var_fragment` builds it (mirrors the
    // `#[cfg(test)]` `var_noninitial_lowered_exprs` byte-for-byte).
    let dm_dims = crate::db::source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());
    let converted_dims: Vec<crate::dimensions::Dimension> = dm_dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();
    let model_name_ident = Ident::new(model.name(db));
    let inputs: BTreeSet<Ident<crate::common::Canonical>> = BTreeSet::new();
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
        } => {
            // `SccPhase::Dt` selects the non-initial (dt/flow) lowering;
            // `SccPhase::Initial` selects the initial lowering (reserved
            // for Phase 2's init-cycle resolution, but the accessor
            // handles it now so the contract is total over `SccPhase`).
            let phase_var = match phase {
                crate::db::SccPhase::Dt => per_phase_lowered.noninitial,
                crate::db::SccPhase::Initial => per_phase_lowered.initial,
            };
            // The phase's `Var::new` errored => cannot source its
            // production lowered exprs => `None` (loud-safe). The test
            // wrapper panics here instead.
            phase_var.ok().map(|v| v.ast)
        }
        // The variable did not lower at all => `None` (loud-safe).
        LoweredVarFragment::Fatal { .. } => None,
    }
}

/// Enumerate the exact set of *element offsets within `base.name`* that a
/// symbolic static view addresses.
///
/// This is the symbolic-space analogue of the prior `Expr`-level
/// `collect_read_slots`'s `StaticSubscript` enumeration. After
/// `resolve_static_view`, a `SymbolicStaticView` whose base is
/// `Var(SymVarRef { name, element_offset })` resolves to absolute slots
/// `layout_offset(name) + element_offset + view.offset + Σ coord·stride`
/// (row-major over `dims`, applying `strides`/`offset`). The element
/// offset *relative to the variable* is therefore
/// `element_offset + view.offset + Σ coord·stride` -- the layout offset
/// cancels, which is exactly the layout independence the symbolic layer
/// provides. The enumeration is **exact** (array sizes are small), so a
/// genuinely element-acyclic model (e.g. `ref.mdl`) still resolves; an
/// extra element only ever forces a conservative `CircularDependency`,
/// never a wrong run order (the loud-safe over-approximation contract the
/// prior `collect_read_slots` documented, preserved here).
fn static_view_element_offsets(view: &crate::compiler::symbolic::SymbolicStaticView) -> Vec<usize> {
    let base_elem = match &view.base {
        crate::compiler::symbolic::SymStaticViewBase::Var(v) => v.element_offset,
        // A temp-backed view threads scratch storage, not a current-value
        // recurrence read (the prior `collect_read_slots` likewise did not
        // treat `TempArray*` as a read).
        crate::compiler::symbolic::SymStaticViewBase::Temp(_) => return Vec::new(),
    };
    let ndims = view.dims.len();
    let mut out: Vec<usize> = Vec::new();
    if ndims == 0 {
        out.push(base_elem + view.offset as usize);
        return out;
    }
    let total: usize = view.dims.iter().map(|d| *d as usize).product();
    for linear in 0..total {
        let mut rem = linear;
        // The element offset relative to `base.name` (the layout offset
        // cancels vs. `resolve_static_view`).
        let mut elem = base_elem as isize + view.offset as isize;
        for d in (0..ndims).rev() {
            let dim = view.dims[d] as usize;
            let coord = rem % dim;
            rem /= dim;
            elem += coord as isize * view.strides[d] as isize;
        }
        if elem >= 0 {
            out.push(elem as usize);
        }
    }
    out
}

/// A `(member, element)` node in the SCC-induced element graph, encoded
/// byte-stably for `crate::ltm::scc_components` (which sorts members
/// lexicographically and components by smallest member). The element
/// index is zero-padded so lexicographic order matches numeric order; the
/// `\u{241F}` (SYMBOL FOR UNIT SEPARATOR) joiner cannot occur in a
/// canonical identifier (canonicalization never emits it; the engine's
/// synthetic separators are `\u{B7}`/`\u{2192}`/`\u{205A}`), so the
/// encoding is injective.
///
/// This is an **opaque graph key**, NOT a canonical identifier. It is
/// deliberately built with `Ident::from_str_unchecked` even though
/// `{member}\u{241F}{element:010}` is not a valid canonical identifier
/// (U+241F is not a canonicalization output), because the value is only
/// ever used as an opaque map/set key inside
/// `symbolic_phase_element_order`'s local element graph -- it is decoded
/// back to `(member, element_index)` via
/// `split_once('\u{241F}')` and NEVER escapes this module (it is never
/// stored on a salsa value, compared against a real variable name, or
/// resolved as an identifier). U+241F is chosen specifically because it is
/// an injective separator that cannot collide with any real canonical
/// member name or with the engine's other synthetic separators, so the
/// `(member, element)` -> key mapping is a bijection on the keys this
/// graph contains. Using the typed `Ident<Canonical>` wrapper (rather than
/// a bare `String`) is purely to satisfy `scc_components`' key type; the
/// canonical-identifier invariant `from_str_unchecked` normally promises
/// is intentionally not required here.
fn element_node_key(
    member: &str,
    element: usize,
) -> crate::common::Ident<crate::common::Canonical> {
    crate::common::Ident::from_str_unchecked(&format!("{member}\u{241F}{element:010}"))
}

/// The verdict of refining one SCC into its induced element graph.
enum SccVerdict {
    /// Element-acyclic + every member element-sourceable: resolved with
    /// the deterministic per-element topological order.
    Resolved(crate::db::ResolvedScc),
    /// Element-cyclic (a genuine element self-loop or a genuine
    /// multi-variable element cycle the symbolic builder detects) or not
    /// element-sourceable: keep the conservative `CircularDependency`.
    Unresolved,
}

/// Build one SCC's induced per-element graph **for a given phase** from
/// the cross-member-comparable SYMBOLIC representation and, if it is
/// element-acyclic and every member is element-sourceable in that phase,
/// return the deterministic per-element topological order. `None` means
/// "not resolvable in this phase" (not element-sourceable, an element
/// self-loop, or an element multi-SCC) -- the loud-safe signal.
///
/// **Why symbolic (GH #575).** The prior builder keyed the element graph
/// on raw `Expr::AssignCurr` operands, which are *per-variable mini-slots*
/// (`lower_var_fragment` builds a fresh per-variable layout: every
/// member's own variable sits at `crate::vm::IMPLICIT_VAR_COUNT`, its own
/// deps after it). Those slots are NOT model-global, so for a multi-member
/// SCC every member's write-slots collided and every cross-member read
/// landed on the *reading* member's private dep mini-slots -- ZERO
/// cross-member edges, a wrong order, and (fatally) a genuine
/// multi-variable element cycle resolved as acyclic (unsound, masked only
/// by the old `members.len() != 1` short-circuit). This builder instead
/// consumes each member's *symbolic* `PerVarBytecodes`
/// (`var_phase_symbolic_fragment_prod`, the exact production
/// compile+symbolize path -- never a re-derivation), where every variable
/// reference is a layout-independent `SymVarRef { name, element_offset }`.
/// `SymVarRef.name` is the canonical variable name (the mini-layout keys
/// are `Ident<Canonical>` -- see `layout_from_metadata`), so it is
/// directly comparable to an SCC member's `Ident<Canonical>`. The N=1 and
/// N>=2 cases are the same builder; N=1 is byte-identical to before (a
/// single member's `AssignCurr(member_base+e, rhs)` symbolizes to one
/// write op with `element_offset == e`, and the reads the prior
/// `collect_read_slots` mapped to `(member, e')` via the mini-`rmap`
/// become exactly the symbolic reads with `name == member,
/// element_offset == e'`; same `element_node_key`, same
/// `scc_components`, same sorted Kahn => same `element_order`).
///
/// The edges are the literal current-value data-flow reads of each
/// element's symbolic segment -- deliberately NOT the LTM
/// `model_element_causal_edges` graph (no lagged/feedback edges invented).
///
/// **PREVIOUS/lagged-read safety -- where the protection actually is.**
/// This graph does NOT inherit PREVIOUS-stripping: a read-opcode list
/// that includes `SymLoadPrev` is treated as an ordinary current-value
/// read edge here (exactly as the prior `Expr`-level builder collected the
/// `PREVIOUS`-argument slot through `for_each_expr_ref`). The reason a
/// PREVIOUS-only self-recurrence (e.g. `x[tNext]=PREVIOUS(x[tPrev],0)`)
/// is nonetheless safe is SCC *identification* upstream:
/// `build_var_info` strips `dt_previous_referenced_vars` from `dt_deps`,
/// so `dt_walk_successors` reports NO whole-variable self-edge for a
/// PREVIOUS-only recurrence, so `resolve_recurrence_sccs` never identifies
/// it as an SCC and this function is never invoked for it. For any SCC
/// that IS identified, including `SymLoadPrev` as an edge is the loud-safe
/// over-approximation direction: it can only ADD an element edge and
/// force a conservative `CircularDependency`, never DROP one and let a
/// genuine cycle through. dt stock-breaking is genuinely inherited (it is
/// reflected in the symbolic bytecode `lower_var_fragment` +
/// `compile_phase_to_per_var_bytecodes` produce, not re-implemented).
fn symbolic_phase_element_order(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    members: &BTreeSet<Ident<Canonical>>,
    phase: crate::db::SccPhase,
) -> Option<Vec<(Ident<Canonical>, usize)>> {
    use crate::compiler::symbolic::SymbolicOpcode;

    // The set of member canonical names, for the "is this read an in-SCC
    // member?" test. `SymVarRef.name` is the canonical variable name
    // (mini-layout keys are `Ident<Canonical>`), so a member's
    // `as_str()` compares directly.
    let member_names: BTreeSet<&str> = members.iter().map(|m| m.as_str()).collect();

    // Build the induced element graph by segmenting each member's
    // symbolic code on its per-element write opcode. For each member
    // element (M, e), every `SymVarRef` read in its segment whose name is
    // an in-SCC member (M', e') contributes a data-flow edge
    // (M', e') -> (M, e). Any member whose symbolic fragment cannot be
    // sourced => not element-sourceable => unresolved (loud-safe).
    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
    let mut self_loop = false;
    // `members` is a BTreeSet, so this iterates in sorted member order;
    // combined with the sorted Kahn below the result is byte-stable.
    for member in members {
        let frag = crate::db::var_phase_symbolic_fragment_prod(
            db,
            model,
            project,
            member.as_str(),
            phase.clone(),
        )?;
        let member_name = member.as_str();

        // Reads accumulated since the previous per-element write of THIS
        // member, as (read-name, read-element) pairs. A read is an
        // in-SCC edge source only if its name is an SCC member.
        let mut pending_reads: BTreeSet<(String, usize)> = BTreeSet::new();
        // True once at least one per-element write of this member has
        // been seen: a malformed fragment with no write for the member
        // means it is not element-sourceable in the simple per-element
        // shape this refinement assumes (loud-safe: keep
        // `CircularDependency`).
        let mut saw_write = false;

        for op in &frag.symbolic.code {
            match op {
                // ── Per-element WRITE of this member: terminate the
                // current element segment, define node (member, elem),
                // and wire every pending in-SCC read as a predecessor.
                SymbolicOpcode::AssignCurr { var }
                | SymbolicOpcode::AssignConstCurr { var, .. }
                | SymbolicOpcode::BinOpAssignCurr { var, .. }
                    if var.name == member_name =>
                {
                    saw_write = true;
                    let node = element_node_key(member_name, var.element_offset);
                    edges.entry(node.clone()).or_default();
                    // Deterministic successor order: BTreeSet pending
                    // reads -> sorted by the read node's encoded key.
                    let mut preds: BTreeSet<Ident<Canonical>> = BTreeSet::new();
                    for (rname, relem) in &pending_reads {
                        if member_names.contains(rname.as_str()) {
                            preds.insert(element_node_key(rname, *relem));
                        }
                    }
                    for pred in preds {
                        if pred == node {
                            // A node reading its own slot is a size-1 SCC
                            // Tarjan does NOT surface as a >=2 component,
                            // so detect element self-loops directly from
                            // adjacency (mirrors `dt_cycle_sccs`).
                            self_loop = true;
                        }
                        edges.entry(pred).or_default().push(node.clone());
                    }
                    pending_reads.clear();
                }
                // ── Reads consumed by the current element segment.
                SymbolicOpcode::LoadVar { var }
                | SymbolicOpcode::SymLoadPrev { var }
                | SymbolicOpcode::SymLoadInitial { var }
                | SymbolicOpcode::LoadSubscript { var }
                | SymbolicOpcode::PushVarView { var, .. }
                | SymbolicOpcode::PushVarViewDirect { var, .. } => {
                    pending_reads.insert((var.name.clone(), var.element_offset));
                }
                SymbolicOpcode::PushStaticView { view_id } => {
                    // Resolve the static view's base; if it is a model
                    // variable, enumerate the EXACT element set it
                    // addresses (the symbolic-space analogue of the prior
                    // `collect_read_slots` `StaticSubscript` enumeration,
                    // so a genuinely element-acyclic model still
                    // resolves). An out-of-range `view_id` is a malformed
                    // fragment (loud-safe: unresolved).
                    let view = frag.static_views.get(*view_id as usize)?;
                    if let crate::compiler::symbolic::SymStaticViewBase::Var(v) = &view.base {
                        for elem in static_view_element_offsets(view) {
                            pending_reads.insert((v.name.clone(), elem));
                        }
                    }
                }
                // Other write targets (a different member, or `AssignNext`
                // / `BinOpAssignNext` -- a stock-update, not a per-element
                // current-value write of THIS member) do not terminate
                // this member's element segment and carry no read; ignore.
                _ => {}
            }
        }

        if !saw_write {
            return None;
        }
    }

    // Element-acyclic iff no element self-loop AND no element multi-SCC,
    // via the promoted `crate::ltm::scc_components`.
    let element_multi_scc = crate::ltm::scc_components(&edges)
        .into_iter()
        .any(|c| c.len() >= 2);
    if self_loop || element_multi_scc {
        return None;
    }

    // Acyclic: emit the deterministic per-element topological order via
    // Kahn's algorithm, tie-broken by (member canonical name, element
    // index) so the order is byte-stable across runs.
    let all_nodes: BTreeSet<Ident<Canonical>> = edges.keys().cloned().collect();
    let mut indegree: HashMap<Ident<Canonical>, usize> =
        all_nodes.iter().map(|n| (n.clone(), 0usize)).collect();
    for succs in edges.values() {
        for s in succs {
            *indegree.entry(s.clone()).or_insert(0) += 1;
        }
    }
    // Decode helper: recover (member, element_index) from an encoded
    // node so the topological order carries the real names/offsets.
    let decode = |node: &Ident<Canonical>| -> (Ident<Canonical>, usize) {
        let s = node.as_str();
        // Split on the injective `\u{241F}` joiner.
        let (member, idx) = s
            .split_once('\u{241F}')
            .expect("element node key is `{member}\u{241F}{index}` by construction");
        (
            Ident::from_str_unchecked(member),
            idx.parse::<usize>()
                .expect("element index is zero-padded decimal by construction"),
        )
    };
    let mut ready: BTreeSet<Ident<Canonical>> = indegree
        .iter()
        .filter(|(_, d)| **d == 0)
        .map(|(n, _)| n.clone())
        .collect();
    let mut order: Vec<(Ident<Canonical>, usize)> = Vec::new();
    while let Some(node) = ready.iter().next().cloned() {
        ready.remove(&node);
        order.push(decode(&node));
        if let Some(succs) = edges.get(&node) {
            // Deterministic relaxation order.
            let mut succs_sorted: Vec<Ident<Canonical>> = succs.clone();
            succs_sorted.sort_by(|a, b| a.as_str().cmp(b.as_str()));
            for s in succs_sorted {
                let d = indegree
                    .get_mut(&s)
                    .expect("successor has an indegree entry");
                *d -= 1;
                if *d == 0 {
                    ready.insert(s);
                }
            }
        }
    }
    // The graph was proven acyclic above, so Kahn drains every node.
    if order.len() != all_nodes.len() {
        return None;
    }
    Some(order)
}

/// Refine one offending SCC (`members`) for the given `phase` into its
/// exact per-element graph and render the element-acyclicity verdict.
///
/// Subcomponent B (GH #575): N=1 and N>=2 are the SAME builder. The
/// element graph is built on the cross-member-comparable SYMBOLIC
/// representation (`symbolic_phase_element_order`), so a multi-member
/// recurrence SCC whose induced element graph is acyclic and
/// element-sourceable resolves exactly like the single-variable case, and
/// a genuine multi-variable element cycle (`a[i]=b[i];b[i]=a[i]`) is
/// detected and stays `Unresolved` (loud-safe). There is no
/// `members.len() != 1` short-circuit (the prior mini-slot builder
/// required one because it could not build cross-member edges and would
/// have resolved a real cycle as acyclic).
///
/// **`SccPhase::Dt` (the dt path).** A single-variable dt self-recurrence's
/// whole-variable self-edge appears in BOTH the dt and the init
/// dependency relations (it is the *same equation*; e.g.
/// `ecc[tNext]=ecc[tPrev]+1` is `ecc`'s init AST too), so
/// `model_dependency_graph_impl` runs the cycle gate over both. Breaking
/// only the dt self-edge would let the model through while a genuine
/// *init* cycle on the same equation is silently masked. So the dt
/// verdict verifies element-acyclicity for `SccPhase::Dt` **and**, as a
/// precondition, `SccPhase::Initial`; only then is the SCC `Resolved`
/// with the dt per-element order and `phase: SccPhase::Dt`. This is the
/// minimal correctness extension for the same-equation aux
/// self-recurrence, NOT init-cycle resolution.
///
/// **`SccPhase::Initial` (the init path -- Phase 2 Task 3).** Targets an
/// init recurrence that is *structurally distinct* from dt: a stock's
/// dt-equation is its flow (a stock breaks the dt chain --
/// `dt_walk_successors` returns `[]`), while its init-equation is its
/// initial value, so a stock whose initial value is a per-element
/// recurrence has an init self-loop with **no corresponding dt cycle**.
/// Here only the **init** induced element graph is relevant: the dt
/// precondition the `Dt` branch applies would be *wrong* (a stock has no
/// dt element graph -- its dt lowering is `AssignNext`, not the
/// per-element `AssignCurr` the element graph reads -- so requiring dt
/// element-acyclicity would spuriously reject every init-only
/// recurrence). The init verdict therefore verifies `SccPhase::Initial`
/// only, and emits the init per-element order with
/// `phase: SccPhase::Initial`. The both-relations aux self-recurrence is
/// NOT double-resolved here: `model_dependency_graph_impl` excludes init
/// SCCs whose members the dt path already resolved before consuming the
/// init verdict, so `{ecc}` stays a single `phase: Dt` `ResolvedScc`.
///
/// Both branches reuse the same phase-parameterized
/// `symbolic_phase_element_order` builder over the engine's own
/// production-compiled symbolic bytecode -- no init-only
/// re-implementation.
fn refine_scc_to_element_verdict(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    members: &BTreeSet<Ident<Canonical>>,
    phase: crate::db::SccPhase,
) -> SccVerdict {
    use crate::db::SccPhase;

    // Subcomponent B (GH #575): N=1 and N>=2 are unified. The element
    // graph is built from the cross-member-comparable SYMBOLIC
    // representation, so a multi-member SCC is just the N>=2 case of the
    // same builder -- no `members.len() != 1` short-circuit. (The prior
    // mini-slot builder forced this short-circuit because it was
    // structurally incapable of cross-member edges and would otherwise
    // have resolved a genuine multi-variable element cycle as acyclic.)
    match phase {
        SccPhase::Dt => {
            // The dt induced element graph must be acyclic +
            // element-sourceable.
            let dt_order =
                match symbolic_phase_element_order(db, model, project, members, SccPhase::Dt) {
                    Some(o) => o,
                    None => return SccVerdict::Unresolved,
                };
            // ...AND so must the init induced element graph: a
            // single-variable dt self-recurrence's self-edge is
            // structurally present in the init relation too (same
            // equation), so the init cycle gate would independently
            // reject the model. Only resolve when the recurrence is
            // well-founded in BOTH phases (loud-safe: an
            // init-element-cyclic member stays `CircularDependency`).
            if symbolic_phase_element_order(db, model, project, members, SccPhase::Initial)
                .is_none()
            {
                return SccVerdict::Unresolved;
            }
            SccVerdict::Resolved(crate::db::ResolvedScc {
                members: members.clone(),
                element_order: dt_order,
                phase: SccPhase::Dt,
            })
        }
        SccPhase::Initial => {
            // The init induced element graph must be acyclic +
            // element-sourceable. NO dt precondition here: an init-only
            // (stock-backed) recurrence has no dt self-edge and no dt
            // per-element `AssignCurr` graph, so requiring dt
            // element-acyclicity would spuriously reject it (loud-safe:
            // an init-element-cyclic member stays `CircularDependency`).
            let init_order = match symbolic_phase_element_order(
                db,
                model,
                project,
                members,
                SccPhase::Initial,
            ) {
                Some(o) => o,
                None => return SccVerdict::Unresolved,
            };
            SccVerdict::Resolved(crate::db::ResolvedScc {
                members: members.clone(),
                element_order: init_order,
                phase: SccPhase::Initial,
            })
        }
    }
}

/// The outcome of refining every offending SCC (for one phase) into its
/// induced element graph.
pub(crate) struct DtSccResolution {
    /// SCCs whose induced element graph the cycle gate proved acyclic and
    /// element-sourceable -- single-variable self-recurrence OR a
    /// multi-variable recurrence cluster (Subcomponent B / GH #575: both
    /// route through the same symbolic element-graph verdict). These are
    /// excluded from the `CircularDependency` accumulation and recorded on
    /// `ModelDepGraphResult.resolved_sccs`.
    pub(crate) resolved: Vec<crate::db::ResolvedScc>,
    /// `true` iff at least one offending SCC is NOT resolved
    /// (element-cyclic -- including a genuine multi-variable element cycle
    /// the symbolic builder detects -- or not element-sourceable). When
    /// `false`, every back-edge in this phase is fully explained by
    /// resolvable recurrences and the phase's cycle gate must NOT set
    /// `has_cycle` / accumulate `CircularDependency` (loud-safe: any doubt
    /// leaves this `true`).
    pub(crate) has_unresolved: bool,
}

/// Identify the offending SCC(s) for `phase` over the engine's own shared
/// cycle relation for that phase and render each one's
/// element-acyclicity verdict.
///
/// *Step A -- SCC identification.* Builds the whole-variable adjacency
/// for `phase` over the shared `build_var_info(.., &[])` universe: for
/// `SccPhase::Dt` the edges are `dt_walk_successors` (exactly as
/// `dt_cycle_sccs` does); for `SccPhase::Initial` they are
/// `init_walk_successors` (Phase 2 Task 2 -- the exact init-phase
/// analogue, where a stock is NOT a sink). Multi-variable SCCs via the
/// promoted `crate::ltm::scc_components` filtered to `len() >= 2`,
/// single-variable self-loops detected directly from adjacency (Tarjan
/// reports a self-loop as a size-1 component). Defining each relation
/// once and consuming it here AND in `compute_inner` makes this the
/// engine's real cycle relation for the phase by construction.
///
/// *Steps B/C -- per-SCC refinement + verdict* are delegated to
/// `refine_scc_to_element_verdict` (the exact `(member, element-offset)`
/// graph from the phase's production-lowered exprs). SCCs are iterated in
/// the sorted `scc_components` / `BTreeSet` order so `resolved` and the
/// downstream runlist are byte-stable.
///
/// On the acyclic happy path there is NO offending SCC, so this returns
/// `{ resolved: [], has_unresolved: false }` with zero refinement work.
///
/// **Init vs dt overlap (consumer's responsibility).** A single-variable
/// aux self-recurrence's self-edge is structurally present in BOTH the dt
/// and the init relation (same equation), so calling this with
/// `SccPhase::Initial` would *also* identify and resolve `{ecc}`. To
/// avoid emitting a duplicate `phase: Initial` `ResolvedScc` for a
/// member the dt path already resolved, `model_dependency_graph_impl`
/// excludes init SCCs whose members are already in the dt-resolved set
/// before consuming the init verdict (and only runs the init resolution
/// at all when the init cycle gate -- with the dt-resolved set's
/// self-edges already broken -- still reports a back-edge, i.e. a
/// *structurally distinct* init-only cycle). This function stays
/// phase-symmetric and cross-phase-agnostic.
///
/// **Scoping note -- no module-input wiring (NOT neutral).** SCC
/// identification here builds `var_info` via
/// `build_var_info(db, model, project, &[])` (empty module inputs), the
/// same default wiring `dt_cycle_sccs` uses, whereas the real
/// `model_dependency_graph_impl` path builds `var_info` with the actual
/// `&module_input_names`. For an input-wired sub-model these relations can
/// differ, so this identification is *incomplete* (it can miss an SCC that
/// only manifests once inputs are wired in). Soundness is still preserved:
/// the resolved member set only suppresses the resolved members' edges in
/// the caller's `compute_transitive`, which re-runs over the real
/// *with-inputs* `var_info`, and its `.unwrap_or_else` arm clears
/// `resolved_sccs` + sets `has_cycle` on any residual genuine cycle, so
/// the worst case is a *missed resolution* (a conservative
/// `CircularDependency`), never an unsound one. This holds for the
/// multi-member SCCs Subcomponent B (GH #575) now resolves as well as for
/// single-variable self-recurrences: the symbolic element-graph verdict
/// only ever *adds* a conservative reject, and the with-inputs re-run is
/// the soundness backstop. `element_order` is still NOT consumed here
/// (it rides on the emitted `ResolvedScc`). Subcomponent B's combined-
/// fragment injection (Task 6, which consumes `element_order` to build
/// the combined per-element fragment) MUST plumb the real
/// `module_input_names` into this identification before relying on the
/// order; do not treat the `&[]` argument as neutral.
pub(crate) fn resolve_recurrence_sccs(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    phase: crate::db::SccPhase,
) -> DtSccResolution {
    use crate::db::SccPhase;

    let (var_info, _all_init_referenced) = build_var_info(db, model, project, &[]);

    // Whole-variable adjacency = exactly the phase's shared cycle
    // relation for every node (the same construction `dt_cycle_sccs`
    // performs for dt; the init-phase analogue for init).
    let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> =
        HashMap::with_capacity(var_info.len());
    let mut self_loops: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    for name in var_info.keys() {
        let succ = match phase {
            SccPhase::Dt => dt_walk_successors(&var_info, name),
            SccPhase::Initial => init_walk_successors(&var_info, name),
        };
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

    // The offending SCCs, in sorted/byte-stable order: every multi-var
    // SCC (size >= 2), then every single-variable self-loop. A
    // multi-variable SCC's members never overlap a self-loop's (a
    // self-loop is its own size-1 component), so the two are disjoint.
    let multi: Vec<BTreeSet<Ident<Canonical>>> = crate::ltm::scc_components(&edges)
        .into_iter()
        .filter(|c| c.len() >= 2)
        .map(|c| c.into_iter().collect())
        .collect();

    let mut resolved: Vec<crate::db::ResolvedScc> = Vec::new();
    let mut has_unresolved = false;

    // Multi-variable SCCs (sorted/byte-stable): refine each into its
    // induced *symbolic* element graph for `phase`. Subcomponent B (the
    // GH #575 correctness rebuild) resolves a multi-member SCC whose
    // induced element graph is acyclic and element-sourceable -- the same
    // verdict path as the single-variable case (N=1 is just the N=1 case
    // of the same builder). A genuine multi-variable element cycle stays
    // `Unresolved` (loud-safe), because the symbolic builder's
    // cross-member `SymVarRef` edges actually detect it (the prior
    // mini-slot builder built ZERO cross-member edges and would have
    // resolved a real cycle as acyclic).
    for members in &multi {
        match refine_scc_to_element_verdict(db, model, project, members, phase.clone()) {
            SccVerdict::Resolved(scc) => resolved.push(scc),
            SccVerdict::Unresolved => has_unresolved = true,
        }
    }

    // Single-variable self-loop SCCs (sorted): refine each into its
    // induced element graph for `phase` and record the verdict.
    for v in &self_loops {
        let members: BTreeSet<Ident<Canonical>> = std::iter::once(v.clone()).collect();
        match refine_scc_to_element_verdict(db, model, project, &members, phase.clone()) {
            SccVerdict::Resolved(scc) => resolved.push(scc),
            SccVerdict::Unresolved => has_unresolved = true,
        }
    }

    DtSccResolution {
        resolved,
        has_unresolved,
    }
}

/// The set of main-model variables whose own production-lowered
/// per-element `Vec<Expr>` is, or recursively contains, an
/// array-producing builtin
/// (VectorElmMap/VectorSortOrder/Rank/AllocateAvailable/AllocateByPriority).
///
/// The universe is the identical `build_var_info(.., &[])` keyset
/// `dt_cycle_sccs` iterates, on the same `(db, model, project)` triple,
/// so a caller can intersect `{multi ∪ self_loops}` with this set over
/// one shared universe. Each variable's lowered `Vec<Expr>` is sourced
/// from the engine's own per-variable production lowering via
/// `var_noninitial_lowered_exprs` (never a re-derivation), and the
/// complete list is fed to
/// `crate::compiler::exprs_contain_array_producing_builtin`. Sourcing the
/// real lowering output -- not a hoist-set subset -- is what makes the
/// membership test complete: `var_noninitial_lowered_exprs` aborts (never
/// silent-skips) on any universe variable whose production lowered exprs
/// cannot be sourced, because a silent skip would under-count and produce
/// a false negative. Sorted/byte-stable.
#[cfg(test)]
pub(crate) fn array_producing_vars(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> BTreeSet<Ident<Canonical>> {
    // The identical universe `dt_cycle_sccs` uses -- the same
    // `build_var_info(.., &[])` keyset on the same `(db, model, project)`
    // triple -- so a caller intersects `{multi ∪ self_loops}` and this
    // set over one universe.
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
/// per-variable lowering the production caller
/// `crate::db::compile_var_fragment` runs -- with the caller-owned,
/// lowering-independent context constructed byte-identically to that
/// caller (same helpers, same order: `source_dims_to_datamodel` ->
/// `DimensionsContext`/`Dimension`, `model.name`, `model_module_map`)
/// and the default no-module-input wiring `dt_cycle_sccs` uses
/// (`build_var_info(.., &[])` => `is_root = true`, empty module inputs).
/// This is the engine's real lowering, never a re-derivation. The
/// non-initial phase is the dt phase, so membership is
/// dt-phase-consistent with the cycle set it is intersected against.
///
/// Aborts (panics -- never silent-skip) when a universe variable's
/// non-initial production lowered exprs cannot be sourced: no
/// `SourceVariable` (an implicit SMOOTH/DELAY/INIT helper -- it has no
/// `lower_var_fragment` entry), `LoweredVarFragment::Fatal` (the variable
/// did not lower at all), or the non-initial phase's `Var::new` errored.
///
/// The abort must fire on *any* incomplete sourcing, not merely a
/// whole-variable `Fatal`: an incompletely-sourced production `Vec<Expr>`
/// makes `array_producing_vars` miss an array-producing `App` the
/// complete lowering would have, a false negative that lets
/// `{multi ∪ self_loops} ∩ array_producing_vars` under-include. The
/// conservative superset (abort on any phase `Var::new` Err, including
/// an initial-only error) is preferred -- strictly safer, with no
/// spurious-abort downside on a well-formed model.
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
             SourceVariable (an implicit SMOOTH/DELAY/INIT helper) -- \
             abort, never silent-skip (a silent skip would under-count \
             array-producing membership)"
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
                 lowered exprs (abort, never silent-skip)"
            ),
        },
        LoweredVarFragment::Fatal { .. } => panic!(
            "array_producing_vars: universe var {var_name:?} failed to lower \
             (LoweredVarFragment::Fatal) -- cannot assess array-producing \
             membership (abort, never silent-skip)"
        ),
    }
}

#[cfg(test)]
#[path = "db_dep_graph_tests.rs"]
mod db_dep_graph_tests;
