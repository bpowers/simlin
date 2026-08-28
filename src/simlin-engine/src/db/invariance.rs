// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Per-model run-invariance classification (GH #712, stage B1).
//!
//! `model_flows_invariant` decides which of a module's flow-phase variables are
//! *run-invariant* -- their value is identical at every timestep, so they can be
//! evaluated once per `run_to` (B2) rather than per step. The per-variable
//! evidence has two owners:
//!
//!  * `VarFragmentResult.flow_locally_invariant` -- the shared classifier
//!    (`crate::compiler::invariance`)
//!    run with an all-`Invariant` offset callback, so it flags only variant
//!    *builtins* (TIME / PULSE / RAMP / STEP / PREVIOUS / EvalModule /
//!    ModuleInput);
//!  * `VariableDeps.dependencies` -- the authoritative source dependency
//!    relation. Only local `Dt`/`Current` targets enter the transitive fixpoint.
//!    `Dt`/`Initial` reads use the frozen initial-values buffer, and
//!    `Dt`/`Previous` already make the local compiler verdict variant.
//!
//! This query is then just the fixpoint over the dependency graph:
//! `invariant(v) iff locally_invariant(v) && current_deps(v) ⊆ invariant-set`.
//! `current_deps` is the complete source relation, including both arms of an
//! eager conditional even when constant folding later discards one. Such a
//! flow may be safely under-hoisted, but never wrongly hoisted: validation,
//! runlist and cycle semantics remain defined by the complete source program.
//! The burden of catching variant *dependencies* (stocks, dynamic auxes,
//! module outputs) therefore rides on the phase/lag/path-typed source relation
//! rather than a second compiler-IR identity walk. The end-to-end bit-constancy oracle
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
    Db, DepPhase, ModuleInputSet, SourceModel, SourceProject, compile_var_fragment,
    model_dependency_graph, variable_direct_dependencies,
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

        // Use the already-cached fragment result for the compiler-local verdict
        // (a salsa cache hit -- assembly triggers compilation first) and the
        // authoritative source query for dependency identity.
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
        // (1) its own expression contains no TIME/PULSE/etc. (locally_pure),
        // (2) every instantaneous dt dependency is already invariant.
        //
        // INIT reads are frozen after initialization and therefore never enter
        // this relation. PREVIOUS reads already make (1) false. A qualified
        // target has no local invariant node, while stocks and modules are
        // never inserted, so all three remain default-variant.
        if locally_invariant
            && deps
                .dependencies
                .iter()
                .filter(|dep| {
                    dep.phase == DepPhase::Dt
                        && dep.lag == crate::variable::DepLag::Current
                        && !(dep.target.module_path.is_empty()
                            && dep.target.variable == var_canonical)
                })
                .all(|dep| {
                    dep.target.module_path.is_empty()
                        && invariant.contains(dep.target.variable.as_str())
                })
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
    /// set -- the fixpoint `model_flows_invariant` computes from the fragment's
    /// compiler-local verdict and the source dependency relation.
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

    /// On an ordinary scalar fixture with no constant-selected branch, the
    /// structured-source fixpoint agrees with a direct classification of each
    /// production-lowered fragment. Constant-selected branches are the
    /// deliberate conservative exception pinned separately below.
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

    /// The production invariance equation has exactly two inputs:
    ///
    /// `compiler-local invariant && every local Dt/Current target invariant`.
    ///
    /// This fixture crosses current, previous and initial occurrences both
    /// alone and mixed on one referent. It derives those rows through
    /// `variable_direct_dependencies`, then checks the model verdict rather
    /// than hand-building either side of the relation.
    #[test]
    fn invariance_propagates_only_instantaneous_dt_targets() {
        let tp = TestProject::new("invariance phase lag")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("k", "10", None)
            .aux("dynamic", "TIME", None)
            .aux("initial_only", "INIT(dynamic)", None)
            .aux("previous_only", "PREVIOUS(k, 0)", None)
            .aux("current_k_initial_dynamic", "k + INIT(dynamic)", None)
            .aux(
                "current_and_initial_dynamic",
                "dynamic + INIT(dynamic)",
                None,
            )
            .aux("previous_and_initial_k", "PREVIOUS(k, INIT(k))", None);
        let project_dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let synced = sync_from_datamodel(&db, &project_dm);
        let model = synced.models["main"].source;
        let no_inputs = ModuleInputSet::empty(&db);

        let occurrence_rows =
            |name: &str| -> BTreeSet<(DepPhase, crate::variable::DepLag, String)> {
                let source = synced.models["main"].variables[name].source;
                crate::db::variable_direct_dependencies(&db, source, synced.project, no_inputs)
                    .dependencies
                    .iter()
                    .map(|dep| (dep.phase, dep.lag, dep.target.variable.to_string()))
                    .collect()
            };
        assert_eq!(
            occurrence_rows("initial_only"),
            [DepPhase::Dt, DepPhase::Init]
                .into_iter()
                .map(|phase| (phase, crate::variable::DepLag::Initial, "dynamic".into()))
                .collect()
        );
        assert_eq!(
            occurrence_rows("current_and_initial_dynamic"),
            [DepPhase::Dt, DepPhase::Init]
                .into_iter()
                .flat_map(|phase| {
                    [
                        (phase, crate::variable::DepLag::Current, "dynamic".into()),
                        (phase, crate::variable::DepLag::Initial, "dynamic".into()),
                    ]
                })
                .collect()
        );
        assert_eq!(
            occurrence_rows("previous_and_initial_k"),
            [DepPhase::Dt, DepPhase::Init]
                .into_iter()
                .flat_map(|phase| {
                    [
                        (phase, crate::variable::DepLag::Previous, "k".into()),
                        (phase, crate::variable::DepLag::Initial, "k".into()),
                    ]
                })
                .collect()
        );

        let invariant = model_flows_invariant(&db, model, synced.project, true, no_inputs);
        assert!(invariant.contains("k"));
        assert!(
            invariant.contains("initial_only"),
            "INIT reads the frozen initial buffer, so a dynamic referent does not propagate"
        );
        assert!(invariant.contains("current_k_initial_dynamic"));
        assert!(!invariant.contains("dynamic"));
        assert!(!invariant.contains("current_and_initial_dynamic"));
        assert!(
            !invariant.contains("previous_only") && !invariant.contains("previous_and_initial_k"),
            "PREVIOUS is locally variant regardless of an invariant referent or INIT fallback"
        );
    }

    /// Constant folding may erase a branch from emitted bytecode, but the
    /// source dependency relation deliberately retains both eager branches for
    /// validation, runlists and cycle detection. Invariance consumes that
    /// complete relation, so a dynamic dependency in either arm safely keeps
    /// the reader in the per-step suffix even when the condition is constant.
    ///
    /// The rows come from production parsing/dependency resolution and the
    /// verdict comes from production fragment lowering; no dependency set or
    /// compiler expression is synthesized by the fixture.
    #[test]
    fn constant_selected_dynamic_branches_are_conservatively_variant() {
        let tp = TestProject::new("constant branch invariance")
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
            let current_targets: BTreeSet<String> =
                variable_direct_dependencies(&db, source, synced.project, no_inputs)
                    .phase(DepPhase::Dt)
                    .filter(|dep| dep.lag == crate::variable::DepLag::Current)
                    .map(|dep| dep.target.variable.to_string())
                    .collect();
            assert_eq!(
                current_targets,
                ["dynamic".to_string(), "k".to_string()]
                    .into_iter()
                    .collect(),
                "{reader}: dependency extraction must retain both eager branches"
            );

            let fragment = compile_var_fragment(&db, source, model, synced.project, no_inputs)
                .as_ref()
                .expect("production fragment");
            assert_eq!(
                fragment.flow_locally_invariant,
                Some(true),
                "{reader}: constant folding removes the dynamic branch from the emitted fragment"
            );
        }

        let invariant = model_flows_invariant(&db, model, synced.project, true, no_inputs);
        assert!(invariant.contains("k"));
        assert!(!invariant.contains("dynamic"));
        assert!(!invariant.contains("select_true"));
        assert!(!invariant.contains("select_false"));
    }

    /// Array materialization and lookup-table layout references reach the same
    /// production fragment classifier as scalar equations. The table holder is
    /// intentionally absent from `DepRef`; only the lookup index participates
    /// in local invariance, while an array value dependency propagates through
    /// the ordinary `Dt/Current` relation.
    #[test]
    fn production_array_and_lookup_rows_use_local_builtin_facts_and_structured_values() {
        let table = datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(vec![0.0, 1.0]),
            y_points: vec![3.0, 4.0],
            x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: datamodel::GraphicalFunctionScale { min: 3.0, max: 4.0 },
        };
        let tp = TestProject::new("array lookup invariance")
            .with_sim_time(0.0, 2.0, 1.0)
            .named_dimension("D", &["a", "b"])
            .array_const("arr[D]", 2.0)
            .scalar_aux("sum_arr", "SUM(arr[*])")
            .scalar_aux("sum_time", "SUM(arr[*]) + TIME")
            .aux_with_gf("table", "", table)
            .scalar_aux("lookup_const", "LOOKUP(table, 0.5)")
            .scalar_aux("lookup_time", "LOOKUP(table, TIME)");
        let project_dm = tp.build_datamodel();
        let db = SimlinDb::default();
        let synced = sync_from_datamodel(&db, &project_dm);
        let model = synced.models["main"].source;
        let no_inputs = ModuleInputSet::empty(&db);

        let sum_source = synced.models["main"].variables["sum_arr"].source;
        let sum_deps = variable_direct_dependencies(&db, sum_source, synced.project, no_inputs);
        assert!(sum_deps.phase(DepPhase::Dt).any(|dep| {
            dep.lag == crate::variable::DepLag::Current
                && dep.target.module_path.is_empty()
                && dep.target.variable.as_str() == "arr"
        }));

        let lookup_source = synced.models["main"].variables["lookup_const"].source;
        let lookup_deps =
            variable_direct_dependencies(&db, lookup_source, synced.project, no_inputs);
        assert!(lookup_deps.dependencies.is_empty());
        assert_eq!(lookup_deps.referenced_tables, ["table".to_string()].into());

        let invariant = model_flows_invariant(&db, model, synced.project, true, no_inputs);
        for name in ["arr", "sum_arr", "lookup_const"] {
            assert!(invariant.contains(name), "{name} must be run-invariant");
        }
        for name in ["sum_time", "lookup_time"] {
            assert!(!invariant.contains(name), "{name} must remain dynamic");
        }
        assert!(
            !invariant.contains("table"),
            "lookup-only holders are layout data, not flow variables"
        );
    }

    /// The two decision classes behind C-LEARN's additional invariant slots:
    /// an applied per-element graphical-function array has immutable table
    /// holders rather than value dependencies, and a pure reducer propagates
    /// the resulting array's verdict to its scalar result.
    ///
    /// The MDL is parsed and lowered through production. The assertions derive
    /// dependency rows, table holders, compiler-local verdicts, final offsets,
    /// and VM series from those production paths. Dynamic-index twins prove
    /// that table immutability does not hide a time-dependent lookup input or
    /// make its downstream reducer invariant.
    #[test]
    fn per_element_lookup_holders_and_their_reducer_are_structurally_invariant() {
        let project_dm = crate::open_vensim(
            "{UTF-8}\n\
D: A1, A2 ~~|\n\
g[A1]( (0,1),(1,2) ) ~~|\n\
g[A2]( (0,10),(1,20) ) ~~|\n\
drive = 0.5 ~~|\n\
out[D] = g[D](drive) ~~|\n\
total = SUM(out[D!]) ~~|\n\
dynamic_out[D] = g[D](TIME) ~~|\n\
dynamic_total = SUM(dynamic_out[D!]) ~~|\n\
INITIAL TIME = 0 ~~|\n\
FINAL TIME = 2 ~~|\n\
SAVEPER = 1 ~~|\n\
TIME STEP = 1 ~~|\n",
        )
        .expect("production MDL parse");
        let db = SimlinDb::default();
        let synced = sync_from_datamodel(&db, &project_dm);
        let model = synced.models["main"].source;
        let no_inputs = ModuleInputSet::empty(&db);

        let source = |name: &str| synced.models["main"].variables[name].source;
        for reader in ["out", "dynamic_out"] {
            let deps = variable_direct_dependencies(&db, source(reader), synced.project, no_inputs);
            let current: BTreeSet<String> = deps
                .phase(DepPhase::Dt)
                .filter(|dep| dep.lag == crate::variable::DepLag::Current)
                .map(|dep| dep.target.variable.to_string())
                .collect();
            assert_eq!(
                current,
                if reader == "out" {
                    ["drive".to_string()].into_iter().collect()
                } else {
                    BTreeSet::new()
                },
                "{reader}: only value dependencies belong in the source relation"
            );
            assert_eq!(
                deps.referenced_tables,
                ["g".to_string()].into_iter().collect(),
                "{reader}: every per-element table is one immutable layout holder"
            );
        }

        for (reader, expected) in [
            ("out", Some(true)),
            ("total", Some(true)),
            ("dynamic_out", Some(false)),
            ("dynamic_total", Some(true)),
        ] {
            let fragment =
                compile_var_fragment(&db, source(reader), model, synced.project, no_inputs)
                    .as_ref()
                    .expect("production fragment");
            assert_eq!(
                fragment.flow_locally_invariant, expected,
                "{reader}: compiler-local builtin/time classification"
            );
        }

        let invariant = model_flows_invariant(&db, model, synced.project, true, no_inputs);
        for name in ["drive", "out", "total"] {
            assert!(invariant.contains(name), "{name}: invariant source closure");
        }
        for name in ["g", "dynamic_out", "dynamic_total"] {
            assert!(
                !invariant.contains(name),
                "{name}: table-only or dynamic source closure"
            );
        }

        let compiled = crate::db::compile_project_incremental(&db, synced.project, "main")
            .expect("production compile");
        let invariant_offsets: BTreeSet<usize> =
            compiled.invariant_flow_offsets().into_iter().collect();
        for name in ["out[a1]", "out[a2]", "total"] {
            let off = compiled
                .get_offset(&Ident::new(name))
                .unwrap_or_else(|| panic!("missing {name}"));
            assert!(invariant_offsets.contains(&off), "{name}: invariant prefix");
        }
        for name in ["dynamic_out[a1]", "dynamic_out[a2]", "dynamic_total"] {
            let off = compiled
                .get_offset(&Ident::new(name))
                .unwrap_or_else(|| panic!("missing {name}"));
            assert!(!invariant_offsets.contains(&off), "{name}: dynamic suffix");
        }

        let mut vm = crate::Vm::new(compiled).expect("VM");
        vm.run_to_end().expect("VM run");
        for (name, expected) in [
            ("out[a1]", vec![1.5, 1.5, 1.5]),
            ("out[a2]", vec![15.0, 15.0, 15.0]),
            ("total", vec![16.5, 16.5, 16.5]),
            ("dynamic_out[a1]", vec![1.0, 2.0, 2.0]),
            ("dynamic_out[a2]", vec![10.0, 20.0, 20.0]),
            ("dynamic_total", vec![11.0, 22.0, 22.0]),
        ] {
            assert_eq!(
                vm.get_series(&Ident::new(name)).expect(name),
                expected,
                "{name}: exact lookup/reducer values"
            );
        }
    }

    /// An unrelated equation edit may rebuild the model fixpoint, but its
    /// source dependency reads remain per-variable firewall queries. The only
    /// dependency body allowed to execute is the edited variable's.
    #[test]
    fn unrelated_flow_edit_does_not_reextract_unchanged_invariance_dependencies() {
        use crate::db::exec_probe::ProbedDb;
        use crate::db::sync_from_datamodel_incremental;

        let project_with = |unrelated: &str| {
            TestProject::new("invariance dependency firewall")
                .with_sim_time(0.0, 5.0, 1.0)
                .aux("k", "10", None)
                .aux("derived", "k * 2", None)
                .aux("unrelated", unrelated, None)
                .build_datamodel()
        };
        let mut probed = ProbedDb::new();
        let base = project_with("1");
        let state1 = sync_from_datamodel_incremental(probed.db_mut(), &base, None);
        let sync1 = state1.to_sync_result();
        let model = sync1.models["main"].source;
        let first = model_flows_invariant(
            probed.db(),
            model,
            sync1.project,
            true,
            ModuleInputSet::empty(probed.db()),
        );
        assert!(first.contains("k") && first.contains("derived"));

        let edited = project_with("2");
        let state2 = sync_from_datamodel_incremental(probed.db_mut(), &edited, Some(&state1));
        let sync2 = state2.to_sync_result();
        assert!(
            sync2.models["main"].source == model,
            "incremental sync must retain the model input"
        );
        probed.reset();
        let second = model_flows_invariant(
            probed.db(),
            model,
            sync2.project,
            true,
            ModuleInputSet::empty(probed.db()),
        );
        assert!(second.contains("k") && second.contains("derived"));
        let counts = probed.counts();
        assert_eq!(
            counts
                .get("variable_direct_dependencies")
                .map(|(executions, distinct)| (*executions, *distinct)),
            Some((1, 1)),
            "only the edited `unrelated` dependency query may execute: {counts:#?}"
        );
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
