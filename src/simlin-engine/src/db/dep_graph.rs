// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Model dependency graph: the cycle gate, its shared dt/init cycle
//! relations, the element-cycle (recurrence-SCC) refinement, and the
//! `#[cfg(test)]` SCC introspection accessor.
//!
//! This module owns the single shared definitions of the **dt-phase**
//! (`dt_walk_successors`) and **init-phase** (`init_walk_successors`)
//! cycle relations, the `VarInfo` map builder (`build_var_info`), the
//! recurrence-SCC element-acyclicity refinement
//! (`resolve_recurrence_sccs` and friends), and the production cycle gate
//! itself (`model_dependency_graph_impl` -- the transitive-closure DFS,
//! the SCC-aware back-edge break, the SCC-as-collapsed-node accumulation,
//! and the SCC-contiguous topological runlist sort). The thin
//! `#[salsa::tracked]` wrapper (`crate::db::model_dependency_graph`, keyed
//! on an interned `ModuleInputSet`) stays in `db.rs` because the
//! `ModelDepGraphResult` salsa input/return types do; it delegates straight
//! to `model_dependency_graph_impl` here.
//!
//! Each cycle relation has exactly one definition, consumed by BOTH the
//! production cycle gate and the `#[cfg(test)]` SCC introspection
//! accessor (`dt_cycle_sccs`), so the accessor observes the engine's
//! actual relation rather than a re-derivation that could silently drift.
//! Co-locating the gate with the relation it consumes keeps that
//! "single shared relation, never re-derive" invariant structural.
//!
//! This is a submodule of `db` (a child of `db.rs`, like `ltm_ir` /
//! `macro_registry`) kept in its own file purely to keep `db.rs` under the
//! per-file line cap; callers reach it via `crate::db::dep_graph::...`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Accumulator;

use crate::canonicalize;
// `Canonical`/`Ident` are used in production by the element-cycle
// refinement (`resolve_recurrence_sccs` and friends), not only by the
// `#[cfg(test)]` SCC accessors.
use crate::common::{Canonical, Ident};
use crate::db::{
    CompilationDiagnostic, Db, Diagnostic, DiagnosticError, DiagnosticSeverity,
    ModelDepGraphResult, ModuleInputSet, ResolvedScc, SccPhase, SourceModel, SourceProject,
    SourceVariableKind, VariableDeps, model_module_ident_context, variable_direct_dependencies,
};

#[cfg(test)]
use crate::db::model_dependency_graph;

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
    /// A standalone lookup-only table: excluded from every runlist and from the
    /// saved output, because it is a static table indexed by callers, not a
    /// value-bearing variable (issue #606).
    pub(crate) is_table_only: bool,
    /// Interned canonical dt-phase deps. `Ident<Canonical>` `Ord` is
    /// lexicographic, so this `BTreeSet` iterates in the SAME order as the
    /// former `BTreeSet<String>` (byte-stability preserved), while `Clone`
    /// is an Arc-refcount bump (the O(V^2) transitive-closure clones become
    /// cheap) and `Hash`/`Eq` are value-based.
    pub(crate) dt_deps: BTreeSet<Ident<Canonical>>,
    /// Interned canonical init-phase deps (same rationale as `dt_deps`).
    pub(crate) initial_deps: BTreeSet<Ident<Canonical>>,
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
/// `var_info.dt_deps` is built). The returned references borrow
/// `var_info`'s interned `dt_deps` keys and iterate in `BTreeSet`
/// (lexicographic) order -- the same order the former `BTreeSet<String>`
/// produced -- so the relation is byte-stable across runs. Returning
/// `&Ident<Canonical>` (not `&str`) lets `compute_inner` Arc-clone a
/// successor into the transitive set instead of allocating a fresh
/// `String`.
pub(crate) fn dt_walk_successors<'a>(
    var_info: &'a FxHashMap<Ident<Canonical>, VarInfo>,
    name: &str,
) -> Vec<&'a Ident<Canonical>> {
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
/// `build_var_info`). The returned references borrow `var_info`'s interned
/// `initial_deps` keys and iterate in `BTreeSet` (lexicographic) order --
/// the same order the former `BTreeSet<String>` produced -- so the relation
/// is byte-stable across runs. Returning `&Ident<Canonical>` (not `&str`)
/// lets `compute_inner` Arc-clone a successor into the transitive set
/// instead of allocating a fresh `String`.
pub(crate) fn init_walk_successors<'a>(
    var_info: &'a FxHashMap<Ident<Canonical>, VarInfo>,
    name: &str,
) -> Vec<&'a Ident<Canonical>> {
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
) -> (
    FxHashMap<Ident<Canonical>, VarInfo>,
    FxHashSet<Ident<Canonical>>,
) {
    let source_vars = model.variables(db);
    let module_input_names = module_input_names.to_vec();
    let module_ident_context =
        model_module_ident_context(db, model, project, module_input_names.clone());
    // Intern the module-input wiring once. An empty set is the no-inputs case
    // (the old `None`-inputs path); `variable_direct_dependencies` maps it back
    // to `None` internally, so the classification is byte-identical to the old
    // empty-vs-nonempty dispatch.
    let module_inputs = ModuleInputSet::from_names(db, &module_input_names);

    let mut var_info: FxHashMap<Ident<Canonical>, VarInfo> = FxHashMap::default();
    let mut all_init_referenced: FxHashSet<Ident<Canonical>> = FxHashSet::default();

    // Normalize a raw dep string to the bare variable/module ident it
    // imposes an ordering edge on, then intern it. A `submodel·subvar`
    // dependency collapses to its leading `submodel` module name (the dt
    // chain-break filter `keep_dt_dep` runs first); a leading `·` separator
    // is stripped. Both the prefix slice and the whole string are canonical
    // substrings of an already-canonical dep, so `from_str_unchecked`
    // (intern, no re-canonicalization scan) is sound.
    let normalize_dep = |dep: &str| -> Ident<Canonical> {
        let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
        if let Some(dot_pos) = effective.find('\u{00B7}') {
            Ident::from_str_unchecked(&effective[..dot_pos])
        } else {
            Ident::from_str_unchecked(effective)
        }
    };
    let normalize_deps = |deps: &BTreeSet<String>| -> BTreeSet<Ident<Canonical>> {
        deps.iter().map(|d| normalize_dep(d)).collect()
    };

    let project_models = project.models(db);

    // Per-variable deps, computed once and reused (salsa-cached). A pre-pass
    // both seeds the instance->model map below and is reused in the main loop.
    let var_deps: Vec<(&String, &VariableDeps)> = source_vars
        .iter()
        .map(|(name, source_var)| {
            let deps = variable_direct_dependencies(
                db,
                *source_var,
                project,
                module_ident_context,
                module_inputs,
            );
            (name, deps)
        })
        .collect();

    // Map each module-INSTANCE name to its model name, so a `instance·subvar`
    // dependency can be resolved to the right submodel. A module instance name
    // is NOT itself a key in `project_models` (keyed by MODEL name). Both
    // declared module variables (`source_vars`) AND synthesized/implicit module
    // instances (e.g. a SMOOTH's `$⁚..⁚smth1⁚<elem>`, which live only in a
    // variable's `implicit_vars`) must be covered, since the
    // stock-output-reading consumer references the implicit instance's output.
    let mut module_instance_model: HashMap<String, String> = source_vars
        .iter()
        .filter(|(_, sv)| sv.kind(db) == SourceVariableKind::Module)
        .map(|(n, sv)| (n.clone(), canonicalize(sv.model_name(db)).into_owned()))
        .collect();
    for (_, deps) in &var_deps {
        for implicit in &deps.implicit_vars {
            if implicit.is_module
                && let Some(model_name) = &implicit.model_name
            {
                module_instance_model
                    .insert(implicit.name.clone(), canonicalize(model_name).into_owned());
            }
        }
    }

    for (name, deps) in &var_deps {
        let init_only_dt = deps.dt_init_only_referenced_vars.clone();
        let lagged_dt_previous = deps.dt_previous_referenced_vars.clone();
        let lagged_initial_previous = deps.initial_previous_referenced_vars.clone();
        let kind = source_vars[name.as_str()].kind(db);
        // A `submodel·subvar` dependency whose `subvar` is a Stock is read
        // from the PRIOR timestep in the dt phase (a stock breaks the
        // dependency chain), so it must NOT impose a same-step ordering edge.
        // This mirrors the legacy `model.rs::module_output_deps` gate
        // (`if ctx.is_initial || !output_var.is_stock()` -- the dt-phase case
        // omits the module dependency for a stock output). It applies to EVERY
        // reader, not just module variables: a NON-module variable that reads
        // a stock submodel output (e.g. `v = SMOOTH(...)·output`, the SMOOTH
        // output being an INTEG stock) must likewise drop the dt edge.
        // Otherwise `normalize_deps` collapses `submodel·output` to the bare
        // `submodel` module name and the reader gains a spurious `reader ->
        // module` dt edge; combined with the module being a sink in the cycle
        // relation (`dt_walk_successors`) but carrying its input src as a
        // direct dep in `dt_dependencies`, this forms an ordering cycle invisible
        // to cycle detection that `topo_sort_str` breaks arbitrarily -- sometimes
        // emitting the module BEFORE its input, so the module reads a stale input
        // each flows step (C-LEARN's `emissions_with_stopped_growth` drop-to-0,
        // #591-c1). The init phase keeps the edge (stocks do not break the chain
        // there), so only `dt_deps` is filtered.
        let keep_dt_dep = |dep: &str| -> bool {
            let effective = dep.strip_prefix('\u{00B7}').unwrap_or(dep);
            if let Some(dot_pos) = effective.find('\u{00B7}') {
                let module_name = &effective[..dot_pos];
                let var_name = &effective[dot_pos + '\u{00B7}'.len_utf8()..];
                // Resolve `module_name` to a submodel: it is either a module
                // INSTANCE (the common case -- a synthesized stdlib/macro
                // instance, keyed by its own ident) or, for a nested-module
                // reference, already a MODEL name. Try the instance map first,
                // then fall back to a direct model-name lookup.
                let sub_canonical = canonicalize(module_name);
                let sub_model = module_instance_model
                    .get(module_name)
                    .and_then(|m| project_models.get(m.as_str()))
                    .or_else(|| project_models.get(sub_canonical.as_ref()));
                if let Some(sub_model) = sub_model {
                    let sub_vars = sub_model.variables(db);
                    if let Some(sub_var) = sub_vars.get(var_name) {
                        return sub_var.kind(db) != SourceVariableKind::Stock;
                    }
                }
            }
            true
        };
        let mut dt_deps: BTreeSet<String> = deps
            .dt_deps
            .iter()
            .filter(|d| keep_dt_dep(d))
            .cloned()
            .collect();
        dt_deps.retain(|dep| !init_only_dt.contains(dep));
        dt_deps.retain(|dep| !lagged_dt_previous.contains(dep));
        let mut initial_deps = deps.initial_deps.clone();
        initial_deps.retain(|dep| !lagged_initial_previous.contains(dep));

        var_info.insert(
            // `source_vars` keys are canonical (canonicalized at sync time),
            // so interning unchecked is sound.
            Ident::from_str_unchecked(name),
            VarInfo {
                is_stock: kind == SourceVariableKind::Stock,
                is_module: kind == SourceVariableKind::Module,
                is_table_only: crate::db::source_var_is_table_only(db, source_vars[name.as_str()]),
                dt_deps: normalize_deps(&dt_deps),
                initial_deps: normalize_deps(&initial_deps),
            },
        );
        all_init_referenced.extend(
            deps.init_referenced_vars
                .iter()
                .map(|d| Ident::from_str_unchecked(d)),
        );

        // Include implicit variables from this variable's deps result.
        // Since we read this from variable_direct_dependencies (not
        // parse_source_variable_with_module_context), salsa's backdating
        // ensures that if the deps + implicit vars haven't changed, this
        // function is cached.
        for implicit in &deps.implicit_vars {
            // Same stock-submodel-output dt chain-break as above (an implicit
            // var can also read a stock submodel output).
            let mut dt_deps: BTreeSet<String> = implicit
                .dt_deps
                .iter()
                .filter(|d| keep_dt_dep(d))
                .cloned()
                .collect();
            dt_deps.retain(|dep| !implicit.dt_init_only_referenced_vars.contains(dep));
            dt_deps.retain(|dep| !implicit.dt_previous_referenced_vars.contains(dep));
            let mut initial_deps = implicit.initial_deps.clone();
            initial_deps.retain(|dep| !implicit.initial_previous_referenced_vars.contains(dep));
            var_info.insert(
                // `implicit.name` is canonicalized in `extract_implicit_var_deps`.
                Ident::from_str_unchecked(&implicit.name),
                VarInfo {
                    is_stock: implicit.is_stock,
                    is_module: implicit.is_module,
                    // Implicit SMOOTH/DELAY/TREND internals are never lookup tables.
                    is_table_only: false,
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
        let succ = dt_walk_successors(&var_info, name.as_str());
        // `succ` is now `Vec<&Ident<Canonical>>`; an `Ident` `==` is pointer
        // equality on the interned handle, so this self-edge check is exact.
        if succ.contains(&name) {
            self_loops.insert(name.clone());
        }
        edges.insert(name.clone(), succ.iter().map(|s| (*s).clone()).collect());
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
    let empty_inputs = ModuleInputSet::empty(db);
    let dep_graph = model_dependency_graph(db, model, project, empty_inputs);
    let resolved_sccs = dep_graph.resolved_sccs.clone();
    let diags = model_dependency_graph::accumulated::<CompilationDiagnostic>(
        db,
        model,
        project,
        empty_inputs,
    );
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
///
/// **Why dropping a negative relative offset cannot lose a real edge (the
/// loud-safe-relevant direction).** The loop computes the variable-
/// relative offset `relative = element_offset + view.offset + Σ
/// coord·stride` and keeps it only when `relative >= 0` (the `elem >= 0`
/// guard). A coordinate the view *legitimately addresses* maps to absolute
/// slot `entry.offset + relative` where `entry.offset = layout_offset(name)`,
/// and that slot lies inside `name`'s storage span `[entry.offset,
/// entry.offset + var_size)` by construction; subtracting `entry.offset`
/// gives `relative in [0, var_size)`, so `relative >= 0` *always* holds for
/// an addressable element. A `relative < 0` is therefore an
/// arithmetically-impossible-to-address coordinate (an out-of-bounds
/// `offset`/`stride` combination, not a slot any real read touches), so the
/// guard only discards non-reads -- it can never drop a real data-flow
/// edge. This is the *opposite* of the loud-safe-violating "drop a real
/// read" case: an over-counted element is conservative here, and the only
/// elements dropped are ones the view cannot read at all. (The original
/// `Expr`-level `collect_read_slots` instead inserted unconditionally via
/// `as usize`; the explicit `elem >= 0` filter here is the equivalent
/// addressable-only set, just made explicit in symbolic space.)
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
/// **PREVIOUS/INIT lagged-read strip -- the element graph inherits
/// `build_var_info`'s per-phase relation (NOT an over-approximation).**
/// The element graph models *current-(phase-)timestep evaluation order*.
/// A lagged/snapshot read is not a current-timestep ordering edge, so the
/// read-opcode arm inherits `build_var_info`'s exact per-phase
/// PREVIOUS/INIT strip (the element-level analogue of the variable-level
/// strip), `phase`-awarely:
/// - `SymLoadPrev` (PREVIOUS, `prev_values` snapshot, prior timestep):
///   contributes NO element edge in EITHER phase -- the analogue of
///   `build_var_info` retaining `dt_deps` against `lagged_dt_previous`
///   (`deps.dt_previous_referenced_vars`, `db/dep_graph.rs:262`) AND
///   `initial_deps` against `lagged_initial_previous`
///   (`deps.initial_previous_referenced_vars`, `:264`).
/// - `SymLoadInitial` (INIT, `initial_values` snapshot): NO edge in
///   `SccPhase::Dt` -- the analogue of `dt_deps` being retained against
///   `init_only_dt` (`deps.dt_init_only_referenced_vars`, `:261`); but
///   KEEPS the edge in `SccPhase::Initial`, because `build_var_info`
///   strips ONLY `lagged_initial_previous` from `initial_deps` (`:264`)
///   and does NOT strip INIT-refs (an `INIT(x)` read during the
///   initial-value computation is a genuine init-phase ordering edge;
///   `init_referenced_vars` feeds the Initials runlist, not a strip).
/// - `LoadVar`/`LoadSubscript`/`PushVarView`/`PushVarViewDirect` and a
///   `Var`-based `PushStaticView`: current-value reads, kept unchanged --
///   the reads `build_var_info` never strips.
///
/// **Why this is the CORRECT relation, not a new over/under-approximation
/// (the AC4 soundness argument).** A genuine current-(phase-)timestep
/// element cycle is, by definition, a cycle of *current-value* reads:
/// every edge on it is a read of some element's value *in the timestep
/// being ordered*. `SymLoadPrev` reads the `prev_values` snapshot (a
/// prior timestep) and `SymLoadInitial` reads the `initial_values`
/// snapshot (the initial timestep) -- in `SccPhase::Dt` neither is ever
/// the current dt timestep's value. Excluding them therefore CANNOT drop
/// an edge that lies on a genuine current-timestep cycle; it removes only
/// spurious lagged edges. This makes the element graph MATCH the engine's
/// actual per-phase data-flow relation (`build_var_info`) exactly --
/// precisely as Phase 1/2 made the SCC relation match the engine's --
/// rather than the prior loud-safe over-approximation (which collected
/// `SymLoadPrev`/`SymLoadInitial` as current edges and only "forced a
/// conservative `CircularDependency`"; that was over-conservative and
/// blocked C-LEARN's legitimately-identified multi-variable SCC, whose
/// `SAMPLE IF TRUE`-expanded members carry a PREVIOUS-wrapped same-element
/// self-read -- 105 spurious element-self-loops, every one a
/// `SymLoadPrev`). It is also why the upstream identification note still
/// holds: a *PREVIOUS-only* self-recurrence
/// (`x[tNext]=PREVIOUS(x[tPrev],0)`) is never even identified as an SCC
/// (`build_var_info` strips its whole-variable self-edge, so
/// `dt_walk_successors` reports none); for an SCC that IS identified via
/// an un-lagged cross-member chain, this strip is what lets its acyclic
/// current-value element graph resolve. dt stock-breaking is genuinely
/// inherited (it is reflected in the symbolic bytecode `lower_var_fragment`
/// + `compile_phase_to_per_var_bytecodes` produce, not re-implemented).
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
                // ── Current-value reads consumed by the current element
                // segment. These are the literal current-(phase-)timestep
                // data-flow reads a genuine element cycle is made of; they
                // are exactly the reads the variable-level relation keeps
                // (`build_var_info` never strips a current-value dep).
                SymbolicOpcode::LoadVar { var }
                | SymbolicOpcode::LoadSubscript { var }
                | SymbolicOpcode::PushVarView { var, .. }
                | SymbolicOpcode::PushVarViewDirect { var, .. } => {
                    pending_reads.insert((var.name.clone(), var.element_offset));
                }
                // ── PREVIOUS (`prev_values` snapshot, prior timestep):
                // NEVER a current-timestep ordering edge, in EITHER phase.
                // This is the element-level analogue of `build_var_info`
                // stripping `lagged_dt_previous`
                // (`deps.dt_previous_referenced_vars`) from `dt_deps`
                // (`db/dep_graph.rs:262`) AND `lagged_initial_previous`
                // (`deps.initial_previous_referenced_vars`) from
                // `initial_deps` (`:264`) -- both phases. Contributing no
                // edge makes the element graph MATCH the engine's actual
                // per-phase relation; it cannot drop a genuine-cycle edge
                // because a genuine current-timestep element cycle is a
                // cycle of *current-value* reads and a `SymLoadPrev` reads
                // a prior-timestep snapshot, never the current timestep's
                // value (the AC4 soundness argument; see the fn rustdoc).
                SymbolicOpcode::SymLoadPrev { .. } => {}
                // ── INIT (`initial_values` snapshot): PHASE-AWARE. In the
                // dt graph it is NOT a current-dt ordering edge -- the
                // element-level analogue of `build_var_info` stripping
                // `init_only_dt` (`deps.dt_init_only_referenced_vars`)
                // from `dt_deps` (`db/dep_graph.rs:261`). In the init
                // graph it IS a genuine init-phase dependency: an INIT(x)
                // read during the initial-value computation orders x's
                // initial value before this element, and `build_var_info`
                // strips ONLY `lagged_initial_previous` from `initial_deps`
                // (`:264`) -- it does NOT strip INIT-refs (those feed
                // `init_referenced_vars`, the Initials runlist, not a
                // strip). Excluding it in `Dt` cannot drop a genuine dt
                // cycle (same AC4 argument: it is an initial-snapshot read,
                // not a current-dt-timestep value); keeping it in
                // `Initial` is required so a genuine init element cycle
                // through INIT() is still detected.
                SymbolicOpcode::SymLoadInitial { var } => {
                    if matches!(phase, crate::db::SccPhase::Initial) {
                        pending_reads.insert((var.name.clone(), var.element_offset));
                    }
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
/// the resolved verdict only ever causes the caller's `compute_transitive`
/// to break an *intra-SCC* back-edge -- one whose two endpoints are
/// members of the SAME resolved, element-acyclic SCC (the SCC-aware
/// `same_resolved_scc` rule; the N=1 self-edge and every N>=2 cross-edge
/// are the same rule, not a flat "suppress any resolved member's edge"
/// set) -- and treat that SCC as one collapsed node. `compute_transitive`
/// re-runs over the real *with-inputs* `var_info`, and its
/// `.unwrap_or_else` arm clears `resolved_sccs` + sets `has_cycle` on any
/// residual genuine cycle, so the worst case is a *missed resolution* (a
/// conservative `CircularDependency`), never an unsound one: a back-edge
/// that is NOT within one resolved SCC -- a genuine cycle, a
/// partially-resolved SCC, or a cross-SCC edge -- is still fatal. This
/// holds for the multi-member SCCs Subcomponent B (GH #575) now resolves
/// as well as for single-variable self-recurrences: the symbolic
/// element-graph verdict only ever *adds* a conservative reject, and the
/// with-inputs re-run is the soundness backstop.
///
/// **As-built decision -- no-input wiring is loud-safe; `module_input_names`
/// plumbing is deferred (GH #573).** `element_order` is NOT consumed here
/// (it rides on the emitted `ResolvedScc`); Subcomponent B's combined-
/// fragment injection (Task 6, `assemble_module` ->
/// `var_phase_symbolic_fragment_prod`) deliberately consumes the *same*
/// no-input wiring: the symbolic per-member fragments are lowered with
/// `lower_var_fragment(.., &[], ..)` / `inputs = BTreeSet::new()` /
/// `build_caller_module_refs(.., &[])`, matching this SCC identification's
/// `build_var_info(.., &[])`, so the verdict's `element_order` and the
/// combined fragment's per-element segmentation agree by construction. The
/// real `module_input_names` are intentionally NOT plumbed into either
/// side, because the with-inputs `compute_transitive` re-run is the
/// soundness backstop for the multi-member (N>=2) SCCs Subcomponent B
/// resolves *exactly as it is for the N=1 single-variable self-recurrence
/// case*: that re-run's `.unwrap_or_else` clears `resolved_sccs` and sets
/// `has_cycle` on any residual genuine cycle (the init re-run has the
/// symmetric `Err` arm), so the only way the no-input vs with-inputs
/// wiring can differ for an input-wired sub-model's self/multi-recurrence
/// is a *conservative* `CircularDependency` (a missed resolution) -- never
/// a wrong `element_order` or a miscompile. (`ref.mdl` / `interleaved.mdl`
/// / `init_recurrence.mdl` are all flat root models with no module inputs,
/// so the corpus is unaffected regardless.) Full `module_input_names`
/// plumbing into this identification is a deferred item tracked by GH #573;
/// it is *not* required for soundness given the backstop, so the `&[]`
/// argument is loud-safe (not neutral, but never unsound) and there is no
/// outstanding MUST on Task 6.
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
            SccPhase::Dt => dt_walk_successors(&var_info, name.as_str()),
            SccPhase::Initial => init_walk_successors(&var_info, name.as_str()),
        };
        // `succ` is `Vec<&Ident<Canonical>>`; `Ident` `==` is pointer
        // equality on the interned handle, so this self-edge check is exact.
        if succ.contains(&name) {
            self_loops.insert(name.clone());
        }
        edges.insert(name.clone(), succ.iter().map(|s| (*s).clone()).collect());
    }

    // The offending SCCs, in sorted/byte-stable order: every multi-var
    // SCC (size >= 2), then every single-variable self-loop.
    //
    // Disjointness is NOT automatic. `scc_components` partitions nodes, so
    // two `multi` SCCs never overlap; but `self_loops` is built
    // independently from a *direct self-edge* `v -> v`, and a node can
    // BOTH carry a direct self-edge AND sit in a >= 2 SCC via cross-member
    // edges (C-LEARN's `emissions_with_cumulative_constraints`: a
    // `SAMPLE IF TRUE`-shaped variable that whole-variable-references
    // itself on its non-PREVIOUS path *and* closes the 22-member
    // recurrence cluster). Tarjan places such a node in its >= 2 component
    // (it is NOT a standalone size-1 SCC), and its self-edge is then an
    // *intra-SCC* edge of that larger SCC -- already evaluated in the
    // SCC's verified per-element `element_order` (Phase 2 Task 5/6), not a
    // separate recurrence. Re-emitting it as its own 1-member
    // `ResolvedScc` would (a) double-resolve it and (b) break the
    // pairwise-disjoint invariant `scc_map_from_resolved` documents and
    // relies on (its last-write-wins `map.insert` would remap the node to
    // the 1-member SCC's id, so `same_resolved_scc` would no longer
    // suppress the genuine intra-cluster back-edges incident to it -> a
    // false residual `CircularDependency`; the C-LEARN blocker's true
    // root cause, unmasked once Part A let the >= 2 SCC resolve). So a
    // `self_loops` entry that is already a `multi` member is filtered out
    // here: the >= 2 SCC subsumes it.
    let multi: Vec<BTreeSet<Ident<Canonical>>> = crate::ltm::scc_components(&edges)
        .into_iter()
        .filter(|c| c.len() >= 2)
        .map(|c| c.into_iter().collect())
        .collect();
    let multi_members: BTreeSet<&str> = multi
        .iter()
        .flat_map(|c| c.iter().map(|m| m.as_str()))
        .collect();
    self_loops.retain(|v| !multi_members.contains(v.as_str()));

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
        let exprs = var_noninitial_lowered_exprs(db, model, project, name.as_str());
        if crate::compiler::exprs_contain_array_producing_builtin(&exprs) {
            out.insert(name.clone());
        }
    }
    out
}

/// The engine's OWN per-variable production-lowered non-initial (dt/flow)
/// `Vec<Expr>` for the canonical `var_name`.
///
/// Sourced via `crate::db::var_fragment::lower_var_fragment` -- the exact
/// per-variable lowering the production caller
/// `crate::db::compile_var_fragment` runs -- with the caller-owned,
/// lowering-independent context constructed byte-identically to that
/// caller (same helpers, same order: `project_datamodel_dims` ->
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
    use crate::db::var_fragment::{LoweredVarFragment, lower_var_fragment};

    let source_vars = model.variables(db);
    let Some(sv) = source_vars.get(var_name) else {
        panic!(
            "array_producing_vars: universe var {var_name:?} has no \
             SourceVariable (an implicit SMOOTH/DELAY/INIT helper) -- \
             abort, never silent-skip (a silent skip would under-count \
             array-producing membership)"
        );
    };

    // Caller-owned, lowering-independent context, read EXACTLY as
    // `crate::db::compile_var_fragment` reads it (the salsa-cached
    // project-global dimension context + converted dims).
    let dim_context = crate::db::project_dimensions_context(db, project);
    let converted_dims = crate::db::project_converted_dimensions(db, project);
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
        converted_dims,
        dim_context,
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
thread_local! {
    /// Test-only set of canonical variable names that
    /// `crate::db::var_phase_symbolic_fragment_prod` must treat as
    /// unsourceable (return `None`), scoped by an active
    /// [`UnsourceableVarsGuard`].
    ///
    /// AC3.2 (a genuinely unsourceable in-SCC node falling back to
    /// `CircularDependency`, loud-safe, no panic) needs an in-cycle node
    /// that is neither in `source_vars` nor resolvable via
    /// `model_implicit_var_info`. Constructing such an organic orphan that
    /// also lands inside an identified recurrence SCC is not deterministic;
    /// this override is the reliable trigger (design deviation 5 of the
    /// Phase 3 plan). It forces the SAME loud-safe `None` arm a real
    /// no-`SourceVariable` node takes, so the regression exercises the real
    /// production loud-safe chain (`model_dependency_graph` ->
    /// `symbolic_phase_element_order` -> `var_phase_symbolic_fragment_prod`)
    /// rather than a unit-level shim.
    static FORCED_UNSOURCEABLE_VARS: std::cell::RefCell<std::collections::BTreeSet<String>> =
        const { std::cell::RefCell::new(std::collections::BTreeSet::new()) };
}

/// `true` iff an active [`UnsourceableVarsGuard`] has forced `var_name`
/// (canonical) to the loud-safe `None` arm of
/// `crate::db::var_phase_symbolic_fragment_prod`. Always `false` in
/// non-test builds (the call site is itself `#[cfg(test)]`).
#[cfg(test)]
pub(crate) fn var_is_forced_unsourceable(var_name: &str) -> bool {
    FORCED_UNSOURCEABLE_VARS.with(|s| s.borrow().contains(var_name))
}

/// RAII guard (test-only) that forces a set of variable names to be
/// treated as unsourceable by `crate::db::var_phase_symbolic_fragment_prod`
/// for the current thread for the guard's lifetime, restoring the previous
/// set on drop (so a panicking test does not leak the override to the next
/// test reusing the thread).
///
/// Because `model_dependency_graph` is salsa-memoized, the guard must
/// outlive every `model_dependency_graph` call in the test whose sourcing
/// it controls (a later call on the same `db` would otherwise return the
/// memoized result regardless of the override state -- the same caveat
/// `AggLoopBudgetGuard` documents).
#[cfg(test)]
pub(crate) struct UnsourceableVarsGuard {
    prev: std::collections::BTreeSet<String>,
}

#[cfg(test)]
impl UnsourceableVarsGuard {
    pub(crate) fn new(var_names: &[&str]) -> Self {
        let next: std::collections::BTreeSet<String> =
            var_names.iter().map(|s| (*s).to_string()).collect();
        let prev = FORCED_UNSOURCEABLE_VARS.with(|s| s.replace(next));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for UnsourceableVarsGuard {
    fn drop(&mut self) {
        FORCED_UNSOURCEABLE_VARS.with(|s| {
            *s.borrow_mut() = std::mem::take(&mut self.prev);
        });
    }
}

#[cfg(test)]
#[path = "dep_graph_tests.rs"]
mod dep_graph_tests;

// ── Model dependency graph (the cycle gate) ────────────────────────────
//
// `model_dependency_graph_impl` is the production consumer of this
// module's shared cycle relation (`dt_walk_successors` /
// `init_walk_successors` / `build_var_info`) and the element-cycle
// refinement (`resolve_recurrence_sccs`). It lives here, alongside the
// relation it consumes, rather than in `db.rs` -- a `db` submodule
// (like `ltm_ir` / `macro_registry`) split out purely for
// the per-file line cap. The thin `#[salsa::tracked]` wrapper
// (`model_dependency_graph`, keyed on an interned `ModuleInputSet`)
// stays in `db.rs` because the `ModelDepGraphResult` salsa types do.

/// Accumulate a model-level `CircularDependency` diagnostic for
/// `var_name` (the variable the dependency walk reported the back-edge
/// on). Factored out of `model_dependency_graph_impl` because the
/// dt-phase, the dt-phase residual-after-resolution, and the init-phase
/// cycle paths all emit the identical diagnostic; keeping one definition
/// prevents the four sites from drifting.
pub(crate) fn cycle_diagnostic(db: &dyn Db, model: SourceModel, var_name: String) {
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: Some(var_name),
        error: DiagnosticError::Model(crate::common::Error {
            kind: crate::common::ErrorKind::Model,
            code: crate::common::ErrorCode::CircularDependency,
            details: None,
        }),
        severity: DiagnosticSeverity::Error,
    })
    .accumulate(db);
}

/// Build the SCC-aware back-edge map consumed by
/// `compute_transitive`/`compute_inner`: every member of `resolved[i]`
/// (offset by `base_id`) maps to the stable SCC id `base_id + i`.
///
/// This generalizes (and *replaces*) the Phase 1 `resolvable_self_loops:
/// &BTreeSet<String>` mechanism. A single-variable self-recurrence is a
/// **1-member SCC** under this map, so the N=1 self-edge break and the
/// N>=2 cross-edge break are the *same* mechanism, not parallel paths. The
/// SCCs in `resolved` are pairwise disjoint (a multi-variable SCC and a
/// self-loop never overlap -- Tarjan reports a self-loop as its own size-1
/// component -- and distinct SCCs have distinct members), and the init
/// gate excludes init-only SCCs whose members the dt path already
/// resolved, so no member is ever assigned two ids within one phase's
/// map. The id is the SCC's index (deterministic, byte-stable), used only
/// as an opaque same-SCC discriminator by `same_resolved_scc`.
pub(crate) fn scc_map_from_resolved(
    resolved: &[ResolvedScc],
    base_id: usize,
    map: &mut BTreeMap<Ident<Canonical>, usize>,
) {
    for (i, scc) in resolved.iter().enumerate() {
        let id = base_id + i;
        for m in &scc.members {
            // `scc.members` is a `BTreeSet<Ident<Canonical>>`; clone is an
            // Arc-refcount bump.
            map.insert(m.clone(), id);
        }
    }
}

/// A back-edge `dep -> name` (where `dep` is already on the dependency
/// DFS stack) is suppressed -- it is NOT a real variable-granularity
/// ordering constraint and NOT a fatal cycle -- **iff `dep` and `name`
/// are members of the SAME resolved recurrence SCC**. A resolved SCC's
/// members are evaluated in the verified `element_order` *inside* the
/// combined per-element fragment (Phase 2 Task 5/6), so their intra-SCC
/// edges (the N=1 self-edge and every N>=2 cross-edge alike) impose no
/// whole-variable ordering. Every OTHER back-edge -- a genuine cycle, a
/// partially-resolved SCC, or a cross-SCC edge between two distinct
/// resolved SCCs -- is still a fatal `CircularDependency` (loud-safe: a
/// back-edge is suppressed only with positive proof both endpoints share
/// one resolved, element-acyclic SCC).
pub(crate) fn same_resolved_scc(
    scc_map: &BTreeMap<Ident<Canonical>, usize>,
    a: &str,
    b: &str,
) -> bool {
    // `BTreeMap<Ident<Canonical>, _>` probes by `&str` via `Borrow<str>`, so
    // callers can keep passing the bare canonical names they already hold.
    matches!(
        (scc_map.get(a), scc_map.get(b)),
        (Some(x), Some(y)) if x == y
    )
}

pub(crate) fn model_dependency_graph_impl(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    module_input_names: &[String],
) -> ModelDepGraphResult {
    let module_input_names = module_input_names.to_vec();
    let (var_info, all_init_referenced) = build_var_info(db, model, project, &module_input_names);

    // Compute transitive dependencies (simplified all_deps without
    // cross-model support).
    //
    // `scc_map` maps each member of a resolved recurrence SCC to a stable
    // SCC id (built by `scc_map_from_resolved` from
    // `resolve_recurrence_sccs`'s verdict). A resolved SCC's induced
    // element graph the element-cycle refinement proved acyclic, so its
    // members are evaluated in the verified `element_order` *inside* the
    // combined per-element fragment (Phase 2 Task 5/6) and their intra-SCC
    // edges are NOT real variable-granularity ordering constraints. A
    // single-variable self-recurrence is just a 1-member SCC under this
    // map, so the N=1 self-edge break and the N>=2 cross-edge break are
    // the *same* mechanism -- this generalizes (and replaces) the Phase 1
    // `resolvable_self_loops` set, it is not a parallel path. On the
    // acyclic happy path and whenever the conservative loud-safe fallback
    // fires `scc_map` is empty, so the cycle detector is byte-identical to
    // before (zero extra work).
    //
    // Two SCC-aware behaviors, both keyed off `scc_map`:
    //  1. *Collapsed-node transitive accumulation* (the
    //     `scc_map.get(name)` block below): every member of an SCC ends
    //     with the SAME transitive set = the union of all members'
    //     EXTERNAL (non-SCC) successors and their transitive deps, and NO
    //     SCC member appears in any member's set. The SCC is thus one
    //     condensed node, positioned after its external deps and before
    //     its external consumers, so the topological runlist never re-sees
    //     the intra-SCC cycle.
    //  2. *SCC-aware back-edge break* (the `processing.contains(dep)` site
    //     via `same_resolved_scc`): an intra-SCC back-edge is suppressed
    //     (no error) instead of fatal. Members are handled by (1) at
    //     `compute_inner` entry and never reach the normal loop, so for a
    //     resolved SCC this site is defense-in-depth; every back-edge NOT
    //     within one resolved SCC (a genuine cycle, a partially-resolved
    //     SCC, a cross-SCC edge) is still a fatal `CircularDependency`
    //     (loud-safe).
    let compute_transitive =
        |is_initial: bool,
         scc_map: &BTreeMap<Ident<Canonical>, usize>|
         -> Result<HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>, String> {
            // Hot working maps use FxHash (fixed-seed, deterministic) and
            // interned `Ident<Canonical>` keys: the O(V^2) transitive-closure
            // clones are Arc-refcount bumps and map ops stop paying SipHash.
            // `all_deps` insertion/lookup order does not leak into output --
            // every dependency *set* is a `BTreeSet` (lexicographic), and the
            // runlist sort tie-breaks by `topo_sort_str`'s visit order over a
            // pre-sorted `names` list -- so the FxHash iteration order is never
            // observable. `processing` is membership-only (DFS-stack back-edge
            // detection); its order is irrelevant.
            let mut all_deps: FxHashMap<Ident<Canonical>, Option<BTreeSet<Ident<Canonical>>>> =
                var_info.keys().map(|k| (k.clone(), None)).collect();
            let mut processing: FxHashSet<Ident<Canonical>> = FxHashSet::default();

            fn compute_inner(
                var_info: &FxHashMap<Ident<Canonical>, VarInfo>,
                all_deps: &mut FxHashMap<Ident<Canonical>, Option<BTreeSet<Ident<Canonical>>>>,
                processing: &mut FxHashSet<Ident<Canonical>>,
                name: &Ident<Canonical>,
                is_initial: bool,
                scc_map: &BTreeMap<Ident<Canonical>, usize>,
            ) -> Result<(), String> {
                if all_deps.get(name).and_then(|d| d.as_ref()).is_some() {
                    return Ok(());
                }

                let info = match var_info.get(name) {
                    Some(info) => info,
                    None => return Ok(()), // unknown variable handled at model level
                };

                // Stocks break the dependency chain in dt phase
                if info.is_stock && !is_initial {
                    all_deps.insert(name.clone(), Some(BTreeSet::new()));
                    return Ok(());
                }

                // Skip modules -- cross-model deps handled at the orchestrator level
                if info.is_module {
                    let direct = if is_initial {
                        &info.initial_deps
                    } else {
                        &info.dt_deps
                    };
                    all_deps.insert(name.clone(), Some(direct.clone()));
                    return Ok(());
                }

                // ── SCC-as-collapsed-node transitive accumulation ──────────
                //
                // If `name` is a member of a resolved recurrence SCC, treat
                // the WHOLE SCC as one condensed node: compute its collapsed
                // EXTERNAL transitive set once and assign the identical set to
                // every member. Every member therefore ends with the SAME
                // member-free transitive set = the union, over every member's
                // successors that are NOT in this SCC, of `{dep} U
                // transitive(dep)`. Intra-SCC successors are skipped entirely
                // (not inserted, not recursed, not absorbed): their order is
                // resolved inside the combined per-element fragment, so they
                // impose no whole-variable ordering and -- crucially -- must
                // not leak into any member's transitive set (else the
                // topological sort would re-see the cycle). This unifies N=1
                // (1-member SCC: the lone member's self-edge is the only
                // intra-SCC successor, skipped -- runlist byte-identical to
                // the Phase 1 self-edge mechanism, since a set containing only
                // `name` itself vs. empty produces the identical
                // `topo_sort_str` output) and N>=2.
                //
                // Soundness: a resolved SCC is a *maximal* SCC of the
                // whole-variable relation (`scc_components`) AND its induced
                // element graph was proven acyclic. By maximality no external
                // successor can transitively reach back into the SCC, so the
                // external recursion never re-enters the SCC and the collapsed
                // set is well-defined and member-free. A genuine cycle is
                // absent from `scc_map` (loud-safe verdict), so its back-edge
                // is still caught by the normal `processing` check below.
                if let Some(&scc_id) = scc_map.get(name) {
                    // Already being collapsed (re-entered via an external
                    // recursion). This cannot happen for a correctly
                    // identified maximal SCC (no external successor reaches a
                    // member); guarding it makes a mis-identified SCC
                    // loud-safe (no infinite recursion, conservative empty
                    // contribution) rather than a panic.
                    if processing.contains(name) {
                        return Ok(());
                    }
                    // Borrow the SCC's interned member idents from `scc_map`
                    // (no clone). `scc_map` is a `BTreeMap`, so this iterates in
                    // lexicographic order.
                    let members: BTreeSet<&Ident<Canonical>> = scc_map
                        .iter()
                        .filter(|&(_, &id)| id == scc_id)
                        .map(|(m, _)| m)
                        .collect();
                    // Mark every member processing so a (maximality-
                    // impossible) external dep that referenced a member is a
                    // loud-safe condition, never a silent miss.
                    for m in &members {
                        processing.insert((*m).clone());
                    }
                    let mut external: BTreeSet<Ident<Canonical>> = BTreeSet::new();
                    // `members` is a BTreeSet => sorted iteration; the
                    // external set is order-independent (a BTreeSet), so the
                    // collapsed result is byte-stable.
                    for m in &members {
                        let succ: Vec<&Ident<Canonical>> = if is_initial {
                            init_walk_successors(var_info, m.as_str())
                        } else {
                            dt_walk_successors(var_info, m.as_str())
                        };
                        for dep in succ {
                            // Intra-SCC successor: resolved inside the
                            // combined fragment, contributes no whole-variable
                            // ordering and must not enter any member's set.
                            if members.contains(&dep) {
                                continue;
                            }
                            external.insert(dep.clone());
                            if processing.contains(dep) {
                                // `dep` is external (not in `members`) yet on
                                // the DFS stack: a genuine cycle through an
                                // external var. It cannot share this SCC
                                // (`members` is the entire SCC), and two
                                // distinct resolved SCCs cannot form a cycle
                                // (they would be one SCC), so
                                // `same_resolved_scc` is necessarily false
                                // here -- still fatal (loud-safe).
                                if same_resolved_scc(scc_map, dep.as_str(), m.as_str()) {
                                    continue;
                                }
                                return Err(m.as_str().to_string());
                            }
                            if all_deps.get(dep).and_then(|d| d.as_ref()).is_none() {
                                compute_inner(
                                    var_info, all_deps, processing, dep, is_initial, scc_map,
                                )?;
                            }
                            // Same `!is_module` non-absorption guard as the
                            // normal path: a module's transitive set is not
                            // absorbed (cross-model deps handled upstream).
                            if var_info.get(dep).map(|d| !d.is_module).unwrap_or(false)
                                && let Some(Some(dep_deps)) = all_deps.get(dep)
                            {
                                external.extend(dep_deps.iter().cloned());
                            }
                        }
                    }
                    for m in &members {
                        processing.remove(*m);
                    }
                    // Assign the identical member-free set to EVERY member so
                    // the SCC is one condensed node in the topological sort.
                    for m in &members {
                        all_deps.insert((*m).clone(), Some(external.clone()));
                    }
                    return Ok(());
                }

                processing.insert(name.clone());

                // The successor set this normal node contributes to cycle
                // detection AND the `all_deps` transitive/ordering map. It
                // is sourced from the SINGLE shared cycle relation for the
                // phase: `dt_walk_successors` (dt) or `init_walk_successors`
                // (init). In the init phase stocks do NOT break the chain
                // (that filter is dt-only -- `compute_inner`'s
                // `info.is_stock && !is_initial` sink does not fire here),
                // so `init_walk_successors` is the init deps filtered only
                // to known vars. Either way this is exactly the effective
                // set the original `for dep in direct { if
                // !var_info.contains_key {continue} if !is_initial &&
                // dep_info.is_stock {continue} ... }` loop iterated, in the
                // same `BTreeSet`-sorted order, so cycle detection (first
                // back-edge) and the `all_deps` transitive map are
                // byte-identical. `init_walk_successors`'s defensive
                // absent/module guards never fire here -- the stock/module
                // early-returns above already handled those before this
                // point (the same way `dt_walk_successors`'s guards are
                // redundant at this call site). Sharing the init relation by
                // construction means the init-phase per-element recurrence
                // resolution observes the engine's actual init relation, not
                // a re-derivation. Only the iteration set is factored out;
                // the stock/module early-returns above and the `transitive`
                // accumulation below are untouched.
                let successors: Vec<&Ident<Canonical>> = if is_initial {
                    init_walk_successors(var_info, name.as_str())
                } else {
                    dt_walk_successors(var_info, name.as_str())
                };

                let mut transitive: BTreeSet<Ident<Canonical>> = BTreeSet::new();
                for dep in successors {
                    transitive.insert(dep.clone());

                    if processing.contains(dep) {
                        // An intra-SCC back-edge of a resolved recurrence SCC
                        // (`dep` and `name` share a resolved SCC) is resolved
                        // internally via the SCC's verified per-element order
                        // and is NOT a fatal cycle. This uniformly covers the
                        // N=1 self-edge (`dep == name`, 1-member SCC) and the
                        // N>=2 cross-edge (`dep != name`, same SCC). Resolved
                        // SCC members are normally handled by the collapsed-
                        // node block above and never reach this loop, so for a
                        // resolved SCC this is defense-in-depth; every OTHER
                        // back-edge -- a genuine cycle, a partially-resolved
                        // SCC, or a cross-SCC edge between two distinct
                        // resolved SCCs -- still returns the
                        // circular-dependency error (loud-safe).
                        if same_resolved_scc(scc_map, dep.as_str(), name.as_str()) {
                            continue;
                        }
                        return Err(name.as_str().to_string()); // circular dependency
                    }

                    if all_deps.get(dep).and_then(|d| d.as_ref()).is_none() {
                        compute_inner(var_info, all_deps, processing, dep, is_initial, scc_map)?;
                    }

                    // `successors` only contains known vars (dt: filtered
                    // inside `dt_walk_successors`; init: the `contains_key`
                    // filter above), so this lookup never misses. The
                    // `!dep_info.is_module` transitive non-absorption guard
                    // is preserved exactly -- it governs only whether
                    // `dep`'s transitive set is absorbed, never iteration.
                    if var_info.get(dep).map(|d| !d.is_module).unwrap_or(false)
                        && let Some(Some(dep_deps)) = all_deps.get(dep)
                    {
                        transitive.extend(dep_deps.iter().cloned());
                    }
                }

                processing.remove(name);
                all_deps.insert(name.clone(), Some(transitive));
                Ok(())
            }

            // Iterate every node once. The keys (interned idents) are cloned
            // (Arc bumps) so the borrow of `var_info` is released before the
            // mutable `all_deps`/`processing` recursion.
            let names: Vec<Ident<Canonical>> = var_info.keys().cloned().collect();
            for name in &names {
                compute_inner(
                    &var_info,
                    &mut all_deps,
                    &mut processing,
                    name,
                    is_initial,
                    scc_map,
                )?;
            }

            // Materialize the std `HashMap` (default hasher) the salsa return
            // type uses; the FxHash working map's iteration order is irrelevant
            // (each value is a `BTreeSet`, and downstream consumers either probe
            // by key or re-sort).
            Ok(all_deps
                .into_iter()
                .map(|(k, v)| (k, v.unwrap_or_default()))
                .collect())
        };

    let mut has_cycle = false;
    let mut resolved_sccs: Vec<ResolvedScc> = Vec::new();

    let no_scc: BTreeMap<Ident<Canonical>, usize> = BTreeMap::new();

    // First pass with NO resolved SCCs: the acyclic happy path returns
    // `Ok` here with zero refinement work (byte-identical to the pre-
    // Phase-1 behavior). Only a genuine back-edge triggers the element-
    // cycle refinement below.
    let dt_first = compute_transitive(false, &no_scc);

    // The SCC-aware back-edge map of the recurrence SCCs whose induced
    // element graph the refinement proved acyclic. For `SccPhase::Dt`,
    // `refine_scc_to_element_verdict` verifies BOTH the dt and the init
    // element graph (a single-variable self-recurrence's self-edge is
    // structurally present in both relations -- `ecc[tNext]=ecc[tPrev]+1`
    // is `ecc`'s init AST too -- and a multi-member dt SCC must be
    // well-founded in both phases), so the same map breaks the dt SCC's
    // intra edges in BOTH `compute_transitive` calls and the dependency
    // maps / runlists come out correct. A single-variable self-recurrence
    // is a 1-member SCC here; a multi-member SCC (GH #575) is the N>=2
    // case of the SAME map. `dt_scc_map` is empty unless the dt gate
    // actually found a fully-resolvable back-edge (acyclic happy path /
    // loud-safe fallback => zero extra work).
    let dt_scc_map: BTreeMap<Ident<Canonical>, usize> = if dt_first.is_err() {
        let resolution =
            crate::db::dep_graph::resolve_recurrence_sccs(db, model, project, SccPhase::Dt);
        if !resolution.has_unresolved && !resolution.resolved.is_empty() {
            let mut map = BTreeMap::new();
            scc_map_from_resolved(&resolution.resolved, 0, &mut map);
            resolved_sccs = resolution.resolved;
            map
        } else {
            // A genuine cycle remains (element-cyclic, not element-
            // sourceable, or a partially-resolved SCC): keep the
            // conservative `CircularDependency` (loud-safe fallback).
            BTreeMap::new()
        }
    } else {
        BTreeMap::new()
    };

    let dt_dependencies = match dt_first {
        Ok(deps) => deps,
        Err(first_cycle_var) => {
            if dt_scc_map.is_empty() {
                // Unresolved: keep the conservative `CircularDependency`.
                has_cycle = true;
                cycle_diagnostic(db, model, first_cycle_var);
                HashMap::new()
            } else {
                // Re-run with every resolved SCC treated as one collapsed
                // node (intra-SCC edges -- N=1 self-edge and N>=2 cross-
                // edges alike -- broken; every other back-edge still
                // errors). A residual genuine cycle is still loud-safe.
                compute_transitive(false, &dt_scc_map).unwrap_or_else(|var_name| {
                    has_cycle = true;
                    resolved_sccs.clear();
                    cycle_diagnostic(db, model, var_name);
                    HashMap::new()
                })
            }
        }
    };

    // ── Init-phase cycle gate (symmetric to the dt block above) ────────
    //
    // First pass with the SAME dt-resolved `dt_scc_map`: it breaks the
    // both-relations aux self-recurrence's (or multi-member dt SCC's) init
    // intra-SCC edges (`ecc[tNext]=ecc[tPrev]+1` is `ecc`'s init AST too,
    // and `refine_scc_to_element_verdict`'s dt verdict already verified
    // the dt SCC's init element graph is acyclic as a precondition). With
    // an empty `dt_scc_map` (the acyclic happy path / loud-safe fallback)
    // this is byte-identical to the original behavior and does ZERO extra
    // init refinement work.
    let init_first = compute_transitive(true, &dt_scc_map);

    let initial_dependencies = match init_first {
        Ok(deps) => deps,
        Err(first_init_cycle_var) => {
            // A back-edge remains AFTER the dt-resolved set's init
            // self-edges are broken => a *structurally distinct*
            // init-only cycle (e.g. a per-element forward recurrence in
            // a stock's initial value -- a stock breaks the dt chain so
            // this never appeared as a dt SCC). Run the init-phase
            // recurrence resolution (Phase 2 Task 3), reusing the
            // phase-parameterized builder.
            let init_resolution = crate::db::dep_graph::resolve_recurrence_sccs(
                db,
                model,
                project,
                SccPhase::Initial,
            );

            // Exclude init SCCs whose members the dt path already
            // resolved: a both-relations aux self-recurrence is
            // identified as an init self-loop too, but re-emitting it as
            // a `phase: Initial` `ResolvedScc` would DUPLICATE the dt
            // path's single `phase: Dt` SCC (regressing the Phase 1
            // self-recurrence behavior). Keep only the structurally
            // distinct init-only SCCs. (Subcomponent A scope:
            // `resolve_recurrence_sccs` already routes multi-variable
            // SCCs to `has_unresolved`; Subcomponent B resolves those.)
            let init_only_resolved: Vec<ResolvedScc> = if init_resolution.has_unresolved {
                Vec::new()
            } else {
                init_resolution
                    .resolved
                    .into_iter()
                    .filter(|s| {
                        !s.members
                            .iter()
                            .any(|m| dt_scc_map.contains_key(m.as_str()))
                    })
                    .collect()
            };

            if init_only_resolved.is_empty() {
                // Either a genuine unresolved init cycle (multi-variable
                // / element-cyclic / not element-sourceable), or every
                // identified init SCC was already dt-covered yet the
                // gate still errs (a genuine residual cycle). Loud-safe:
                // keep the conservative `CircularDependency`.
                has_cycle = true;
                cycle_diagnostic(db, model, first_init_cycle_var);
                HashMap::new()
            } else {
                // Break the init-only resolved SCCs' intra edges too (in
                // ADDITION to the dt-resolved SCCs'), then re-run. Init-
                // only SCC ids are offset past the dt SCC ids
                // (`resolved_sccs.len()` dt SCCs are recorded so far) so
                // each SCC keeps a distinct id and `same_resolved_scc`
                // never conflates a dt SCC with an init-only one. A
                // residual genuine cycle is still loud-safe (clear every
                // resolved SCC and flag -- mirrors the dt re-run's
                // `unwrap_or_else`, honoring the
                // `resolved_sccs`-empty-on-fallback invariant).
                let mut init_scc_map = dt_scc_map.clone();
                scc_map_from_resolved(&init_only_resolved, resolved_sccs.len(), &mut init_scc_map);
                match compute_transitive(true, &init_scc_map) {
                    Ok(deps) => {
                        resolved_sccs.extend(init_only_resolved);
                        deps
                    }
                    Err(var_name) => {
                        has_cycle = true;
                        resolved_sccs.clear();
                        cycle_diagnostic(db, model, var_name);
                        HashMap::new()
                    }
                }
            }
        }
    };

    // Build runlists via topological sort. The runlists themselves stay
    // `Vec<String>` (O(V), many `&str`-keyed consumers in db.rs/tests), so
    // we materialize the interned `var_info` keys back to owned `String`s at
    // this boundary. The hot O(V^2) transitive closure above stays interned.
    let var_names: Vec<String> = {
        let mut names: Vec<String> = var_info.keys().map(|k| k.as_str().to_string()).collect();
        names.sort_unstable();
        names
    };

    // Per-phase resolved-SCC groupings for contiguous runlist placement.
    // A resolved recurrence SCC must appear in the runlist as ONE
    // contiguous, byte-stable block at the SCC's topological slot, so the
    // combined per-element fragment (Phase 2 Task 6) can be injected at
    // the first member's slot with the rest skipped, landing in correct
    // relative order. `topo_sort_str` treats each SCC as one condensed
    // node (the collapsed-node transitive sets already make every member's
    // deps the identical member-free external set, so the block's
    // topological position is well-defined). A 1-member SCC (the N=1
    // single-variable self-recurrence) is a trivial one-element block, so
    // grouping is a no-op there and the runlist is byte-identical to the
    // Phase 1 mechanism.
    //
    // Flows use `dt_dependencies`, so only `Dt`-phase SCCs are grouped
    // there (an `Initial`-phase SCC is stock-backed and stocks are not in
    // the flows runlist). Initials use `initial_dependencies`, where a
    // `Dt`-phase aux SCC's members carry the SAME recurrence in their init
    // equations AND an `Initial`-phase SCC obviously recurs, so BOTH
    // phases are grouped for the initials runlist.
    let build_scc_grouping = |only_dt: bool| -> (HashMap<&str, usize>, HashMap<usize, Vec<&str>>) {
        let mut scc_of: HashMap<&str, usize> = HashMap::new();
        let mut scc_members: HashMap<usize, Vec<&str>> = HashMap::new();
        for (idx, scc) in resolved_sccs.iter().enumerate() {
            if only_dt && scc.phase != SccPhase::Dt {
                continue;
            }
            // `scc.members` is a BTreeSet, so this member list is sorted
            // and byte-stable.
            let members: Vec<&str> = scc.members.iter().map(|m| m.as_str()).collect();
            for m in &members {
                scc_of.insert(*m, idx);
            }
            scc_members.insert(idx, members);
        }
        (scc_of, scc_members)
    };
    let (flows_scc_of, flows_scc_members) = build_scc_grouping(true);
    let (init_scc_of, init_scc_members) = build_scc_grouping(false);

    let topo_sort_str = |names: Vec<&String>,
                         deps: &HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
                         scc_of: &HashMap<&str, usize>,
                         scc_members: &HashMap<usize, Vec<&str>>|
     -> Vec<String> {
        use std::collections::HashSet;
        // Build the allowed set: only variables in the filtered input list
        // should appear in the output. Dependencies are used solely for
        // ordering, not for expanding the set.
        let allowed: HashSet<&str> = names.iter().map(|n| n.as_str()).collect();
        let mut result: Vec<String> = Vec::new();
        let mut used: HashSet<String> = HashSet::new();

        // `deps` is now interned-keyed, but this sort still works in `&str`
        // space: probes go through `Borrow<str>` and each dep-set iteration
        // hands `add` the dep's `as_str()`. The output is byte-identical to
        // the former `String`-keyed map (same visit order over a pre-sorted
        // `names`, same `BTreeSet` dep order).
        fn add(
            deps: &HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>,
            allowed: &HashSet<&str>,
            scc_of: &HashMap<&str, usize>,
            scc_members: &HashMap<usize, Vec<&str>>,
            result: &mut Vec<String>,
            used: &mut HashSet<String>,
            name: &str,
        ) {
            if used.contains(name) {
                return;
            }
            // If `name` is a member of a resolved SCC, emit the WHOLE SCC
            // as one condensed node: mark every member used up-front (so a
            // member's dep recursion cannot re-enter the block), recurse
            // each member's deps -- the collapsed-node transitive sets are
            // the identical member-free external set, so this places the
            // block strictly after its external deps -- then push every
            // member (those in `allowed`) in the SCC's sorted, byte-stable
            // order as a contiguous run. Any external consumer that
            // reaches a member triggers this whole block first, so the SCC
            // also precedes its external consumers.
            if let Some(scc_id) = scc_of.get(name)
                && let Some(members) = scc_members.get(scc_id)
            {
                for m in members {
                    used.insert((*m).to_string());
                }
                for m in members {
                    if let Some(d) = deps.get(*m) {
                        for dep in d.iter() {
                            add(
                                deps,
                                allowed,
                                scc_of,
                                scc_members,
                                result,
                                used,
                                dep.as_str(),
                            );
                        }
                    }
                }
                for m in members {
                    if allowed.contains(*m) {
                        result.push((*m).to_string());
                    }
                }
                return;
            }
            used.insert(name.to_string());
            if let Some(d) = deps.get(name) {
                for dep in d.iter() {
                    add(
                        deps,
                        allowed,
                        scc_of,
                        scc_members,
                        result,
                        used,
                        dep.as_str(),
                    );
                }
            }
            // Only include variables that were in the original filtered list
            if allowed.contains(name) {
                result.push(name.to_string());
            }
        }

        for name in names {
            add(
                deps,
                &allowed,
                scc_of,
                scc_members,
                &mut result,
                &mut used,
                name,
            );
        }
        result
    };

    // Initials runlist: stocks, modules, INIT-referenced vars, and their
    // transitive deps.
    //
    // Module variables have their transitive deps short-circuited in
    // compute_transitive (only direct deps are stored). The deps of those
    // direct deps (e.g. an implicit intermediate variable depending on a
    // regular model variable) ARE fully expanded in initial_dependencies.
    // We must transitively close init_set so that every variable needed
    // during the initials phase is included in the allowed set for
    // topo_sort_str.
    //
    // Variables referenced by INIT() must also be seeded into the needed
    // set. Without this, aux-only models (no stocks/modules) using INIT(x)
    // would have an empty Initials runlist, and initial_values[x_offset]
    // would stay at zero.
    //
    // A variable whose value is *fully determined at initialization* -- no
    // current-value (dt) dependencies, but a non-empty set of initial-time
    // dependencies -- must ALSO be seeded. This is the structural signature
    // of an `INITIAL(...)`-backed variable (`v = INITIAL(x)` compiles to a
    // bare `LoadInitial`; `x` is an init-time dep but not a current-value
    // one). Such a variable can be a module/macro *primary output* that a
    // parent reads during the parent's OWN initials phase (e.g. C-LEARN's
    // `:MACRO: INIT(x) INIT = INITIAL(x)`, invoked as
    // `volumetric_heat_capacity = INITIAL(...)`); the sub-model's dep graph
    // is computed in isolation and cannot see that cross-model read. Without
    // this clause the output is compiled only into the flows phase, its
    // initials slot is never written, and the parent snapshots the
    // uninitialized slot (0 in a clean buffer, `inf`/NaN in a reused one) into
    // `initial_values`, served forever by `LoadInitial` (GH #584). General
    // principle: any variable whose value comes from `INITIAL()` and is read
    // during initials must be evaluated in the initials phase; the bounded,
    // structurally-keyed realization here is the empty-`dt_deps` /
    // non-empty-`initial_deps` set, whose initials value is provably its true
    // t=0 value (its init-time deps are themselves pulled into the runlist by
    // the transitive closure below).
    let runlist_initials = {
        use std::collections::HashSet;
        // `needed` borrows `var_names` (owned `String`s). `init_set` works in
        // `&str` space because `initial_dependencies` is now interned-keyed:
        // its dep iterator yields `&Ident<Canonical>` (taken as `.as_str()`),
        // while the seed members are `var_names`' `&str`. All entries borrow
        // strings that outlive the block (`var_names` for seeds,
        // `initial_dependencies`' interned keys for deps).
        let needed: HashSet<&str> = var_names
            .iter()
            .filter(|n| {
                var_info
                    .get(n.as_str())
                    .map(|i| {
                        // A lookup-only table produces no value, so it is never
                        // an initials-phase variable (issue #606).
                        !i.is_table_only
                            && (i.is_stock
                                || i.is_module
                                || (i.dt_deps.is_empty() && !i.initial_deps.is_empty()))
                    })
                    .unwrap_or(false)
                    || all_init_referenced.contains(n.as_str())
            })
            .map(|n| n.as_str())
            .collect();
        let mut init_set: HashSet<&str> = needed
            .iter()
            .flat_map(|n| {
                initial_dependencies
                    .get(*n)
                    .into_iter()
                    .flat_map(|deps| deps.iter().map(|d| d.as_str()))
            })
            .collect();
        init_set.extend(needed);
        // Transitively close: each item added to init_set may itself
        // have deps that also need to be in the initials runlist.
        loop {
            let additional: HashSet<&str> = init_set
                .iter()
                .flat_map(|n| {
                    initial_dependencies
                        .get(*n)
                        .into_iter()
                        .flat_map(|deps| deps.iter().map(|d| d.as_str()))
                })
                .filter(|d| !init_set.contains(d))
                .collect();
            if additional.is_empty() {
                break;
            }
            init_set.extend(additional);
        }
        // `init_set` is a `HashSet`, so its iteration order is a function of
        // the per-process HashMap RandomState seed, not of the model. Sort
        // before handing it to `topo_sort_str`, which emits `names` in visit
        // order and breaks ties (variables with no ordering dependency between
        // them -- independent constants, stocks, `INITIAL()`/`PREVIOUS()`
        // helpers) by exactly that order. Without this sort the initials
        // runlist is HashMap-iteration-order dependent: two compiles of the
        // SAME model produce different init orderings and therefore different
        // initial values for any unordered pair (GH #595). The Flows and
        // Stocks runlists already filter the pre-sorted `var_names`, so they
        // are deterministic by construction; this matches that contract for
        // the initials phase.
        let mut init_list: Vec<&str> = init_set.into_iter().collect();
        init_list.sort_unstable();
        // `topo_sort_str` takes `Vec<&String>`; materialize the sorted
        // candidate names as owned `String`s (this is the init phase, O(V),
        // not the hot transitive-closure path).
        let init_list_owned: Vec<String> = init_list.iter().map(|s| (*s).to_string()).collect();
        let init_list_refs: Vec<&String> = init_list_owned.iter().collect();
        topo_sort_str(
            init_list_refs,
            &initial_dependencies,
            &init_scc_of,
            &init_scc_members,
        )
    };

    // Flows runlist: non-stock variables, modules, AND stock-typed module inputs.
    // The monolithic path uses `instantiation.contains(id) || !var.is_stock()`
    // which includes stock-typed module inputs (e.g., a stock declared with
    // access="input" in XMILE). These need LoadModuleInput -> AssignCurr in
    // the flows phase to propagate the parent-provided value each timestep.
    let module_input_set: BTreeSet<String> = module_input_names
        .iter()
        .map(|s| canonicalize(s).into_owned())
        .collect();
    let runlist_flows = {
        let flow_names: Vec<&String> = var_names
            .iter()
            .filter(|n| {
                let is_input = module_input_set.contains(canonicalize(n).as_ref());
                var_info
                    .get(n.as_str())
                    // A lookup-only table is a static table, not a flow: it is
                    // never lowered and emits no bytecode (issue #606).
                    .map(|i| (is_input || !i.is_stock) && !i.is_table_only)
                    .unwrap_or(false)
            })
            .collect();
        topo_sort_str(
            flow_names,
            &dt_dependencies,
            &flows_scc_of,
            &flows_scc_members,
        )
    };

    // Stocks runlist: stocks and modules
    let runlist_stocks: Vec<String> = var_names
        .iter()
        .filter(|n| {
            var_info
                .get(n.as_str())
                .map(|i| i.is_stock || i.is_module)
                .unwrap_or(false)
        })
        .cloned()
        .collect();

    ModelDepGraphResult {
        dt_dependencies,
        initial_dependencies,
        runlist_initials,
        runlist_flows,
        runlist_stocks,
        has_cycle,
        // Populated by the element-cycle refinement
        // (`resolve_recurrence_sccs`) when a cycle gate's back-edge is
        // fully explained by resolvable single-variable self-recurrences:
        // the dt path emits `phase: Dt` SCCs, and the symmetric init
        // path (Phase 2 Task 3) emits `phase: Initial` SCCs for the
        // structurally-distinct init-only recurrences a stock's initial
        // value can introduce (the both-relations aux self-recurrence is
        // NOT double-counted -- the init path excludes members the dt
        // path already resolved). Empty on the acyclic happy path and
        // whenever the conservative loud-safe `CircularDependency`
        // fallback fires (zero extra work, no behavior change there).
        // This is the sole `ModelDepGraphResult` construction site (the
        // dt/init back-edge paths fall through here), so the byte-stable
        // `resolved` vector flows straight onto the salsa return value.
        resolved_sccs,
    }
}
