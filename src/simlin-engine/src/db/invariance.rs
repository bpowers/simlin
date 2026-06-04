// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-model run-invariance classification (GH #712, stage B1).
//!
//! `model_flows_invariant` decides which of a module's flow-phase variables are
//! *run-invariant* -- their value is identical at every timestep, so they can be
//! evaluated once per `run_to` (B2) rather than per step. It is the imperative
//! shell around the pure classifier (`crate::compiler::invariance`): it lowers
//! each flow variable through the EXACT production lowering
//! (`lower_var_fragment` -- the same call `compile_var_fragment` makes, so the
//! verdict is over the engine's own lowered `Vec<Expr>`), builds the
//! offset-classification callback from that variable's mini-layout plus the
//! per-model verdict accumulated so far, and runs the shared classifier.
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
#[salsa::tracked]
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
        // runs) rather than re-calling `lower_var_fragment`. The
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
    use crate::compiler::invariance::{OffsetClass, exprs_are_invariant};
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

    /// Compute the monolithic-path run-invariant flow var-name set by running
    /// the SAME shared classifier over the test-only `Module`'s model-global
    /// lowered exprs. The offset callback resolves a model-global offset to its
    /// owning variable via the `Module`'s metadata, then classifies the owner by
    /// kind (stock/module -> variant) and by the accumulated invariant set,
    /// mirroring the salsa callback. Restricted to scalar variables (no array
    /// temps) so `Module::get_flow_exprs` captures each variable's full flow
    /// statement list.
    fn monolithic_invariant_set(tp: &TestProject, runlist_order: &[String]) -> BTreeSet<String> {
        use crate::common::{Canonical, Ident};

        let module = tp.build_module().expect("build monolithic module");
        let model_ident = module.ident.clone();
        let model_offsets = module
            .offsets
            .get(&model_ident)
            .expect("model offsets")
            .clone();

        // Stocks and modules in this model (by canonical name): a referenced
        // owner of these kinds is variant. We derive stock/module membership
        // from the datamodel (the monolithic Module does not carry kind flags
        // in its offset map).
        let project_dm = tp.build_datamodel();
        let main_model = project_dm
            .models
            .iter()
            .find(|m| Ident::<Canonical>::new(&m.name) == model_ident)
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
            let exprs: Vec<Expr> = module
                .get_flow_exprs(var_name)
                .into_iter()
                .cloned()
                .collect();
            if exprs.is_empty() {
                continue;
            }

            let classify_offset = |off: usize| -> OffsetClass {
                let owner = model_offsets
                    .iter()
                    .find(|(_, (base, size))| off >= *base && off < *base + *size)
                    .map(|(name, _)| name.as_str().to_string());
                let Some(owner) = owner else {
                    return OffsetClass::Variant;
                };
                if owner == *var_name {
                    return OffsetClass::Invariant;
                }
                if stock_or_module.contains(&owner) {
                    return OffsetClass::Variant;
                }
                if invariant.contains(&owner) {
                    OffsetClass::Invariant
                } else {
                    OffsetClass::Variant
                }
            };

            if exprs_are_invariant(&exprs, &classify_offset) {
                invariant.insert(var_name.clone());
            }
        }
        invariant
    }

    /// The salsa and monolithic paths -- both running the SAME shared classifier
    /// over each path's lowered exprs -- agree on which flow variables are
    /// run-invariant. This guards against the two paths' offset callbacks
    /// drifting.
    #[test]
    fn salsa_and_monolithic_paths_agree() {
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

        // Build the monolithic runlist order from the dep graph so both paths
        // classify the same variable universe.
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
        let mono = monolithic_invariant_set(&tp, &dep_graph.runlist_flows);

        assert_eq!(
            salsa, mono,
            "salsa and monolithic invariant sets disagree:\n  salsa: {salsa:?}\n  mono:  {mono:?}"
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
