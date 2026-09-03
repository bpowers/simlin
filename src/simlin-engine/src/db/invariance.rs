// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-model run-invariance classification (GH #712, stage B1).
//!
//! `model_flows_invariant` decides which of a module's flow-phase variables are
//! *run-invariant* -- their value is identical at every timestep, so they can be
//! evaluated once per `run_to` (B2) rather than per step. The verdict has two
//! halves, each owned once:
//!
//!  * `VarFragmentResult.flow_locally_invariant` -- the compiler-local walk
//!    (`crate::compiler::invariance`) over the engine's own lowered `Vec<Expr>`,
//!    computed inside the already-cached `compile_var_fragment` (no second
//!    lowering): whether the expression holds a time-dependent builtin, a
//!    lagged read, a module evaluation or a module input;
//!  * `VariableDeps.deps` -- the variable's `DepRef`s, the one dependency
//!    relation: the names its flow phase reads currently. An `INIT` read is
//!    the frozen initial buffer and a `PREVIOUS` read already makes the local
//!    half variant, so only the `Dt`/`Current` reads enter the fixpoint; a
//!    called table enters it too, because a lookup-only holder is in no
//!    runlist and never invariant, so a reader of a foreign table stays
//!    per-step.
//!
//! This query is then just the fixpoint over the dependency graph:
//! `invariant(v) iff locally_invariant(v) && reads(v) ⊆ invariant-set`. The
//! reads are the SOURCE relation, so a conditional whose literal condition the
//! compiler folds away keeps the reads of both arms: such a flow may be
//! under-hoisted, never wrongly hoisted ("Phase 8.5 semantic divergences").
//! The end-to-end bit-constancy oracle (`tests/integration/simulate.rs`
//! `oracle_*`) pins the production mechanism.
//!
//! The flow runlist (`ModelDepGraphResult.runlist_flows`) is a topological
//! order: every non-stock/non-module dt dependency precedes its reader. So a
//! single ordered pass reaches a fixpoint -- when variable `v` is classified,
//! every dependency whose verdict it needs has already been classified.
//!
//! Conservatism (soundness over completeness):
//!  * Only the ROOT module is classified; submodules return an empty set (B1/B2
//!    hoist only the root flow phase). A non-root call therefore costs nothing.
//!  * A variable that is part of a resolved recurrence SCC is classified
//!    VARIANT (it reads a co-member's current value within the dt phase; the
//!    combined-fragment lowering is not separable into the per-variable
//!    statement list the classifier walks). This never produces a false
//!    positive.
//!  * Any read that does not positively name an invariant variable of this
//!    model -- a stock, a module instance's output, an unclassified name -- is
//!    variant. Default-variant throughout.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::common::{Canonical, Ident};
use crate::db::dep_graph::build_var_info;
use crate::db::{
    Db, DepPhase, ModuleInputSet, SourceModel, SourceProject, compile_var_fragment,
    model_dependency_graph, variable_direct_dependencies,
};
use crate::variable::DepLag;

/// The set of a module's flow-phase variables that are run-invariant, by
/// canonical name. Empty for submodules and for any model with no invariant
/// flow variable.
///
/// Salsa-tracked, keyed identically to `assemble_module` / `compile_var_fragment`
/// (`model` + `project` + `module_inputs`), so the partition `assemble_module`
/// applies reads the same verdict that was computed for this exact module
/// instance.
#[salsa::tracked(returns(clone))]
pub(crate) fn model_flows_invariant<'db>(
    db: &'db dyn Db,
    model: SourceModel,
    project: SourceProject,
    is_root: bool,
    module_inputs: ModuleInputSet<'db>,
) -> Arc<BTreeSet<String>> {
    // Only the root module is hoisted (B1/B2 scope). A submodule's entire flow
    // program stays dynamic. This is the single authoritative guard; the
    // external caller (`assemble_module`) calls us unconditionally and relies
    // on this check.
    if !is_root {
        return Arc::new(BTreeSet::new());
    }

    let module_input_names = module_inputs.names(db);
    let dep_graph = model_dependency_graph(db, model, project, module_inputs);
    // A model with a genuine cycle is rejected at assembly; classifying it is
    // pointless (and its runlists are empty), so bail to the safe empty set.
    if dep_graph.has_cycle {
        return Arc::new(BTreeSet::new());
    }

    let (var_info, _init_referenced) = build_var_info(db, model, project, module_input_names);

    // Members of a resolved recurrence SCC are conservatively variant.
    let scc_members: BTreeSet<&str> = dep_graph
        .resolved_sccs
        .iter()
        .flat_map(|scc| scc.members.iter().map(|m| m.as_str()))
        .collect();

    // Map a source-variable name to its `SourceVariable` (only explicit source
    // vars have `compile_var_fragment` entries; implicit/LTM/synthetic helpers
    // are absent and stay variant by omission).
    let source_vars = model.variables(db);

    // The accumulated verdict, threaded through the topological pass.
    let mut invariant: BTreeSet<String> = BTreeSet::new();

    for var_name in &dep_graph.runlist_flows {
        // Resolved-SCC members: conservatively variant.
        if scc_members.contains(var_name.as_str()) {
            continue;
        }

        let var_canonical: Ident<Canonical> = Ident::new(var_name);

        // Skip stocks and modules outright (a stock is not a flow var; a module
        // instance is conservatively variant). `var_info` carries the kind.
        if let Some(info) = var_info.get(&var_canonical)
            && (info.is_stock || info.is_module || info.is_table_only)
        {
            continue;
        }

        // Only explicit source variables are classified; an implicit helper or
        // an LTM synthetic var (absent from `model.variables`) stays variant.
        let Some(svar) = source_vars.get(var_name.as_str()) else {
            continue;
        };

        // The compiler-local half comes off the already-cached fragment (a
        // salsa cache hit -- `assemble_module` triggers compilation before
        // this query runs), the reads off the dependency memo.
        let Some(result) = compile_var_fragment(db, *svar, model, project, module_inputs) else {
            // Compilation failed; treat as variant by omission.
            continue;
        };
        let Some(locally_invariant) = result.flow_locally_invariant else {
            // Variable is not in the flows runlist or noninitial lowering failed.
            continue;
        };
        let deps = variable_direct_dependencies(db, *svar, project, module_inputs);

        // A variable is invariant iff:
        // (1) its own expression contains no TIME/PULSE/etc. (locally invariant),
        // (2) every variable its flow phase reads currently is already
        //     classified invariant, and every table it calls is.
        //
        // A read of the variable itself is a self-reference (a WITH LOOKUP
        // variable calls its own table), never a dependency. A stock, a module
        // instance and a lookup-only holder are never in `invariant` (the loop
        // skips them), so the transitive variant propagation is automatic.
        let names_itself = |name: &Ident<Canonical>| *name == var_canonical;
        let reads_invariant = deps
            .deps
            .phase(DepPhase::Dt)
            .filter(|dep| dep.lag == DepLag::Current)
            .all(|dep| {
                dep.target.is_local()
                    && (names_itself(&dep.target.variable)
                        || invariant.contains(dep.target.variable.as_str()))
            })
            && deps
                .referenced_tables
                .iter()
                .all(|table| names_itself(table) || invariant.contains(table.as_str()));
        if locally_invariant && reads_invariant {
            invariant.insert(var_name.clone());
        }
    }

    Arc::new(invariant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Compute the salsa-path run-invariant flow var-name set for a `main`
    /// model built from `tp`.
    fn salsa_invariant_set(tp: &TestProject) -> BTreeSet<String> {
        let db = SimlinDb::default();
        let project_dm = tp.build_datamodel();
        let result = sync_from_datamodel(&db, &project_dm);
        let model = result.models["main"].source;
        let inv =
            model_flows_invariant(&db, model, result.project, true, ModuleInputSet::empty(&db));
        (*inv).clone()
    }

    /// The verdict's two halves crossed through production: every lag of a
    /// read (current, previous, initial), alone and mixed on one referent,
    /// beside the local half (a time-dependent builtin). Rows derive their
    /// `DepRef`s through `variable_direct_dependencies` and their verdict
    /// through `model_flows_invariant`; nothing is hand-built.
    #[test]
    fn invariance_propagates_only_current_dt_reads() {
        let tp = TestProject::new("main")
            .with_sim_time(0.0, 5.0, 1.0)
            // invariant constant chain
            .aux("k", "10", None)
            .aux("derived", "k * 3 + 1", None)
            .aux("pure", "SQRT(k) + EXP(0)", None)
            // dynamic: TIME and stock reads
            .aux("dynamic", "TIME", None)
            .aux("reads_stock", "level + 1", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "dynamic + reads_stock + derived", None)
            // snapshot and lagged reads
            .aux("initial_only", "INIT(dynamic)", None)
            .aux("previous_only", "PREVIOUS(k, 0)", None)
            .aux("current_k_initial_dynamic", "k + INIT(dynamic)", None)
            .aux(
                "current_and_initial_dynamic",
                "dynamic + INIT(dynamic)",
                None,
            )
            .aux("previous_and_initial_k", "PREVIOUS(k, INIT(k))", None);
        let invariant = salsa_invariant_set(&tp);

        for name in [
            "k",
            "derived",
            "pure",
            "initial_only",
            "current_k_initial_dynamic",
        ] {
            assert!(invariant.contains(name), "{name} must be run-invariant");
        }
        for name in [
            "dynamic",
            "reads_stock",
            "inflow",
            "current_and_initial_dynamic",
            "previous_only",
            "previous_and_initial_k",
        ] {
            assert!(!invariant.contains(name), "{name} must stay dynamic");
        }
    }

    /// The reads are the SOURCE relation: a conditional whose literal
    /// condition the compiler folds away keeps both arms' reads, so a
    /// dynamic read in the discarded arm keeps the reader per-step. The
    /// fragment's local half sees the folded body (invariant), the memo's
    /// reads see both arms; the verdict follows the reads. Pinned as the
    /// under-hoist it is ("Phase 8.5 semantic divergences").
    #[test]
    fn constant_selected_dynamic_branches_are_conservatively_variant() {
        let tp = TestProject::new("main")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("k", "10", None)
            .aux("dynamic", "TIME", None)
            .aux("select_true", "IF 1 THEN k ELSE dynamic", None)
            .aux("select_false", "IF 0 THEN dynamic ELSE k", None);
        let project_dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let synced = sync_from_datamodel(&db, &project_dm);
        let model = synced.models["main"].source;
        let no_inputs = ModuleInputSet::empty(&db);

        for reader in ["select_true", "select_false"] {
            let source = synced.models["main"].variables[reader].source;
            let current: BTreeSet<&str> =
                variable_direct_dependencies(&db, source, synced.project, no_inputs)
                    .deps
                    .phase(DepPhase::Dt)
                    .filter(|dep| dep.lag == DepLag::Current)
                    .map(|dep| dep.target.variable.as_str())
                    .collect();
            assert_eq!(current, ["dynamic", "k"].into_iter().collect());
            let fragment = compile_var_fragment(&db, source, model, synced.project, no_inputs)
                .as_ref()
                .expect("production fragment");
            assert_eq!(
                fragment.flow_locally_invariant,
                Some(true),
                "{reader}: the fold leaves an invariant body"
            );
        }

        let invariant = model_flows_invariant(&db, model, synced.project, true, no_inputs);
        assert!(invariant.contains("k"));
        for name in ["dynamic", "select_true", "select_false"] {
            assert!(!invariant.contains(name), "{name} must stay dynamic");
        }
    }

    /// A lookup's table holder is a called table, not a read: it is never
    /// classified (a lookup-only holder is in no runlist), so a reader of a
    /// foreign table stays per-step whatever its index, while a WITH LOOKUP
    /// variable calling its own table follows its input. A module output
    /// read is a read of another model's variable and stays per-step.
    #[test]
    fn table_holders_and_module_outputs_are_never_invariant_reads() {
        let table = datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0]),
            y_points: vec![3.0, 4.0],
            x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: datamodel::GraphicalFunctionScale { min: 3.0, max: 4.0 },
        };
        let mut project = TestProject::new("main")
            .with_sim_time(0.0, 2.0, 1.0)
            .aux("k", "0.5", None)
            .aux_with_gf("table", "", table.clone())
            .aux("lookup_const", "LOOKUP(table, k)", None)
            .aux_with_gf("own_table", "k", table)
            .aux("module_out", "sub.out", None)
            .build_datamodel();
        project.models[0]
            .variables
            .push(crate::testutils::x_module_named("sub", "child", &[], None));
        project.models.push(crate::testutils::x_model(
            "child",
            vec![crate::testutils::x_aux("out", "1", None)],
        ));
        let db = SimlinDb::default();
        let synced = sync_from_datamodel(&db, &project);
        let model = synced.models["main"].source;
        let invariant =
            model_flows_invariant(&db, model, synced.project, true, ModuleInputSet::empty(&db));
        assert!(invariant.contains("k"));
        assert!(
            invariant.contains("own_table"),
            "a WITH LOOKUP variable's own table is a self-reference"
        );
        for name in ["table", "lookup_const", "module_out", "sub"] {
            assert!(!invariant.contains(name), "{name} must stay dynamic");
        }
    }

    /// A non-root module is never classified (B1/B2 scope is the root only).
    #[test]
    fn nonroot_module_yields_empty_set() {
        let db = SimlinDb::default();
        let tp = TestProject::new("main")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("k", "10", None);
        let project_dm = tp.build_datamodel();
        let result = sync_from_datamodel(&db, &project_dm);
        let model = result.models["main"].source;
        // `is_root = false` -> empty regardless of contents.
        let inv = model_flows_invariant(
            &db,
            model,
            result.project,
            false,
            ModuleInputSet::empty(&db),
        );
        assert!(inv.is_empty());
    }
}
