// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-model run-invariance classification (GH #712, stage B1).
//!
//! `model_flows_invariant` decides which of a module's flow-phase variables are
//! *run-invariant* -- their value is identical at every timestep, so they can be
//! evaluated once per `run_to` (B2) rather than per step. The per-variable
//! evidence is precomputed inside the already-cached `compile_var_fragment`
//! (`assemble::compute_flow_invariance_support`, over the engine's own lowered
//! `Vec<Expr>` -- no second lowering) and carried on
//! `VarFragmentResult.flow_invariance` as `FlowInvarianceSupport`:
//!
//!  * `locally_pure` -- the shared classifier (`crate::compiler::invariance`)
//!    run with an all-`Invariant` offset callback, so it flags only variant
//!    *builtins* (TIME / PULSE / RAMP / STEP / PREVIOUS / EvalModule /
//!    ModuleInput);
//!  * `dep_names` -- the variables the FLOW exprs actually read
//!    (`collect_expr_refs`, reading the owning variable's name straight off
//!    each reference; `INIT()` arguments are skipped because the init buffer
//!    is frozen).
//!
//! This query is then just the fixpoint over the dependency graph:
//! `invariant(v) iff locally_pure(v) && dep_names(v) ⊆ invariant-set`, so the
//! whole burden of catching variant *dependencies* (stocks, dynamic auxes,
//! module outputs) rides on `dep_names`. That set used to be recovered by
//! reverse-mapping slot offsets through a private per-fragment layout, with a
//! `debug_assert!` guarding the silent-drop case; references now carry the name
//! and there is no lookup left to fail. The end-to-end bit-constancy oracle
//! (`tests/integration/simulate.rs` `oracle_*`) still pins the production
//! mechanism.
//!
//! The flow runlist (`ModelDepGraphResult.runlist_flows`) is a topological
//! order: every non-stock/non-module dt dependency precedes its reader. So a
//! single ordered pass reaches a fixpoint -- when variable `v` is classified,
//! every dependency whose verdict it needs has already been classified. The
//! accumulated set of invariant canonical names is the callback's source of
//! "is this dependency invariant".
//!
//! Conservatism (soundness over completeness):
//!  * Only the ROOT module is classified; submodules return an empty set (B1/B2
//!    hoist only the root flow phase). A non-root call therefore costs nothing.
//!  * A variable that is part of a resolved recurrence SCC is classified
//!    VARIANT (it reads a co-member's current value within the dt phase; the
//!    combined-fragment lowering is not separable into the per-variable
//!    statement list the classifier walks). This never produces a false
//!    positive.
//!  * Any dependency the offset callback cannot positively resolve to an
//!    invariant variable -- a stock, a module instance, an unclassified name --
//!    is treated as variant. Default-variant throughout.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::common::{Canonical, Ident};
use crate::db::dep_graph::build_var_info;
use crate::db::{
    Db, ModuleInputSet, SourceModel, SourceProject, compile_var_fragment, model_dependency_graph,
};

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

        // Use the already-cached `compile_var_fragment` result (a salsa cache
        // hit -- `assemble_module` triggers compilation before this query
        // runs) rather than re-lowering the fragment. The
        // `flow_invariance` field was pre-computed there at no extra cost.
        let Some(result) = compile_var_fragment(db, *svar, model, project, module_inputs) else {
            // Compilation failed; treat as variant by omission.
            continue;
        };
        let Some(inv_support) = &result.flow_invariance else {
            // Variable is not in the flows runlist or noninitial lowering failed.
            continue;
        };

        // A variable is invariant iff:
        // (1) its own expression contains no TIME/PULSE/etc. (locally_pure),
        // (2) every dep it references is already classified invariant.
        //
        // Stock and module deps are never in `invariant` (the loop skips
        // adding them), so the transitive variant propagation is automatic.
        if inv_support.locally_pure
            && inv_support
                .dep_names
                .iter()
                .all(|dep| invariant.contains(dep.as_str()))
        {
            invariant.insert(var_name.clone());
        }
    }

    Arc::new(invariant)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::Expr;
    use crate::compiler::invariance::{RefClass, exprs_are_invariant};
    use crate::datamodel;
    use crate::db::{ModuleInputSet, SimlinDb, sync_from_datamodel};
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

    /// The run-invariant flow var-name set obtained by running the SAME shared
    /// classifier directly over each variable's production-lowered flow exprs
    /// (`var_noninitial_lowered_exprs`), in runlist order. The callback reads
    /// the owning variable's name straight off the reference, then classifies
    /// it by kind (stock/module -> variant) and by the accumulated invariant
    /// set -- the fixpoint `model_flows_invariant` computes from the
    /// `FlowInvarianceSupport` that `compile_var_fragment` precomputes.
    /// Restricted to scalar variables (no array temps) so one variable's flow
    /// statement list is exactly its lowered exprs.
    fn direct_classifier_invariant_set(
        db: &SimlinDb,
        model: SourceModel,
        project: SourceProject,
        project_dm: &datamodel::Project,
        runlist_order: &[String],
    ) -> BTreeSet<String> {
        use crate::common::{Canonical, Ident};

        // Stocks and modules in this model (by canonical name): a referenced
        // owner of these kinds is variant. Membership comes from the datamodel,
        // which is what the classifier's callback has to know about each name.
        let main_model = project_dm
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("main model in datamodel");
        let mut stock_or_module: BTreeSet<String> = BTreeSet::new();
        for v in &main_model.variables {
            let canonical = Ident::<Canonical>::new(v.get_ident()).as_str().to_string();
            match v {
                datamodel::Variable::Stock(_) | datamodel::Variable::Module(_) => {
                    stock_or_module.insert(canonical);
                }
                _ => {}
            }
        }

        let mut invariant: BTreeSet<String> = BTreeSet::new();
        for var_name in runlist_order {
            // Skip stocks/modules outright (not classified as invariant flows).
            if stock_or_module.contains(var_name) {
                continue;
            }
            let exprs: Vec<Expr> =
                crate::db::var_noninitial_lowered_exprs(db, model, project, var_name);
            if exprs.is_empty() {
                continue;
            }

            let classify_ref = |var: &crate::compiler::VarRef| -> RefClass {
                let owner = var.name.as_str();
                if owner == var_name.as_str() {
                    return RefClass::Invariant;
                }
                if stock_or_module.contains(owner) {
                    return RefClass::Variant;
                }
                if invariant.contains(owner) {
                    RefClass::Invariant
                } else {
                    RefClass::Variant
                }
            };

            if exprs_are_invariant(&exprs, &classify_ref) {
                invariant.insert(var_name.clone());
            }
        }
        invariant
    }

    /// `model_flows_invariant`'s fixpoint over the precomputed per-fragment
    /// support agrees with the shared classifier run directly over each
    /// variable's production-lowered exprs. This guards the `locally_pure` /
    /// `dep_names` precomputation against drifting from the classifier it
    /// summarizes.
    #[test]
    fn precomputed_support_agrees_with_the_direct_classifier_run() {
        let tp = TestProject::new("main")
            .with_sim_time(0.0, 5.0, 1.0)
            // invariant constant chain
            .aux("k", "10", None)
            .aux("derived", "k * 3 + 1", None)
            .aux("pure", "SQRT(k) + EXP(0)", None)
            // dynamic: TIME and stock reads
            .aux("ramping", "TIME * 2", None)
            .aux("reads_stock", "level + 1", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "ramping + reads_stock + derived", None);

        let salsa = salsa_invariant_set(&tp);

        // Classify in the production flow runlist order, so both computations
        // walk the same variable universe in the same order.
        let db = SimlinDb::default();
        let project_dm = tp.build_datamodel();
        let result = sync_from_datamodel(&db, &project_dm);
        let model = result.models["main"].source;
        let dep_graph = crate::db::model_dependency_graph(
            &db,
            model,
            result.project,
            ModuleInputSet::empty(&db),
        );
        let direct = direct_classifier_invariant_set(
            &db,
            model,
            result.project,
            &project_dm,
            &dep_graph.runlist_flows,
        );

        assert_eq!(
            salsa, direct,
            "precomputed and direct invariant sets disagree:\n  precomputed: {salsa:?}\n  direct: {direct:?}"
        );

        // Sanity: the constant chain is invariant, the TIME/stock chain is not.
        assert!(salsa.contains("k"));
        assert!(salsa.contains("derived"));
        assert!(salsa.contains("pure"));
        assert!(!salsa.contains("ramping"));
        assert!(!salsa.contains("reads_stock"));
        assert!(!salsa.contains("inflow"));
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
