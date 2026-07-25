// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{Canonical, Error, Ident};
use crate::datamodel;
use crate::dimensions::DimensionsContext;
use crate::model::ModelStage1;
use std::sync::Arc;

use {crate::units::Context, std::collections::BTreeSet};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Project {
    pub datamodel: datamodel::Project,
    // these are Arcs so that multiple Modules created by the compiler can
    // reference the same Model instance
    pub models: HashMap<Ident<Canonical>, Arc<ModelStage1>>,
    #[allow(dead_code)]
    model_order: Vec<Ident<Canonical>>,
    /// Project-level errors. With the `from_salsa` construction path, unit
    /// definition errors are read off `UnitsContextResult::definition_errors`
    /// -- the same memoized derivation `collect_all_diagnostics` reports from
    /// -- so callers can inspect them here and get the same set.
    pub errors: Vec<Error>,
    /// Cached dimension context for subdimension lookups
    pub dimensions_ctx: DimensionsContext,
}

impl Project {
    pub fn name(&self) -> &str {
        &self.datamodel.name
    }
}

impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        Self::from_datamodel(project_datamodel)
    }
}

impl Project {
    /// Convenience constructor: creates a local salsa DB,
    /// syncs the datamodel, and builds the Project via `from_salsa`.
    pub(crate) fn from_datamodel(project_datamodel: datamodel::Project) -> Self {
        let db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
        Self::from_salsa(
            project_datamodel,
            &db,
            sync.project,
            |_models, _units_ctx, _model| {},
        )
    }

    /// Build a `Project` from a pre-synced salsa database.
    ///
    /// The two pre-layout model stages come from the cached `db::stages`
    /// queries -- the crate's single salsa-native construction of them -- so
    /// this path and the unit pass share one set of memos. The caller provides
    /// the salsa DB and `SourceProject`; the `model_cb` runs per non-stdlib
    /// model after dependency resolution (typically unit inference/checking).
    ///
    /// **Reachability.** Nothing in production calls this. Inside the crate
    /// every caller is a test (`test_common::TestProject::compile`/
    /// `build_module`, the `units_infer` inference tests, `db::stages_tests`,
    /// and the tests below); outside it, the `From<datamodel::Project>` impl is
    /// public, but the only in-repo user is the engine's own
    /// `tests/integration/simulate_ltm.rs`, which builds a `Project` solely to
    /// feed the `ltm_finding::discover_loops` convenience wrapper. The shipped
    /// analysis surface (libsimlin's `simlin_analyze_discover_loops` ->
    /// `analysis::...` -> `ltm_finding::discover_loops_with_graph`) never
    /// constructs one. So this is a "monolith as live oracle" path: it is worth
    /// keeping honest because tests compare against it, not because users run
    /// it.
    pub(crate) fn from_salsa<F>(
        project_datamodel: datamodel::Project,
        db: &dyn crate::db::Db,
        source_project: crate::db::SourceProject,
        mut model_cb: F,
    ) -> Self
    where
        F: FnMut(&HashMap<Ident<Canonical>, &ModelStage1>, &Context, &mut ModelStage1),
    {
        use crate::common::{ErrorCode, ErrorKind, topo_sort};
        use crate::db::{
            model_stage1, project_datamodel_dims, project_dimensions_context,
            project_units_context_result,
        };
        use crate::model::enumerate_modules;

        let units_result = project_units_context_result(db, source_project);
        let units_ctx = &units_result.ctx;

        // Unit definition errors come off the memoized result, so callers that
        // inspect `Project.errors` (e.g. tests) see the same set
        // `collect_all_diagnostics` reports -- and see it deterministically.
        // They used to be recovered by draining the salsa accumulator, which
        // returned them once per reachable model and returned NOTHING at all
        // once an unrelated revision bump let the DFS prune the subtree.
        let project_errors: Vec<Error> = units_result
            .definition_errors
            .iter()
            .flat_map(|(name, eq_errors)| {
                eq_errors.iter().map(move |eq_err| {
                    let unit_err = crate::common::UnitError::DefinitionError(eq_err.clone(), None);
                    Error {
                        kind: ErrorKind::Model,
                        code: ErrorCode::UnitDefinitionErrors,
                        details: Some(format!("{name}: {unit_err}")),
                    }
                })
            })
            .collect();
        let dm_dims = project_datamodel_dims(db, source_project);
        // Read the project-global dimension context from the salsa-cached query
        // rather than rebuilding it here (it is canonicalized once per project).
        let dims_ctx = project_dimensions_context(db, source_project);

        // Every model's lowered stage, read from the cached query instead of
        // built here. This used to be ~90 lines that re-derived the stdlib
        // `implicit` test, the stdlib module-ident set, the per-variable parse
        // loop, the SMOOTH/DELAY implicit-variable parse, the duplicate-ident
        // model errors, and the whole-project Stage0 -> Stage1 lowering -- a
        // second salsa-native copy of `db::stages`, which had already silently
        // drifted from the others on three fields (GH #966).
        //
        // The values are CLONED because everything below is destructive and the
        // memo is shared with every other reader: `model_deps.take()` empties
        // that field, `set_dependencies` fills `instantiations`, rewrites
        // `errors`, and pushes equation errors onto the variables THEMSELVES,
        // and `model_cb` takes `&mut ModelStage1`. A `ModelStage1` is one
        // model's lowered equations, so this is the same allocation the deleted
        // code paid for building them -- the saving is the parse and lowering
        // work, not the copy.
        let mut models_list: Vec<ModelStage1> = source_project
            .models(db)
            .values()
            .map(|src_model| model_stage1(db, *src_model, source_project).clone())
            .collect();

        // Topo-sort by model dependencies.
        let model_order = {
            let model_deps: HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>> = models_list
                .iter_mut()
                .map(|model| {
                    let deps = model.model_deps.take().unwrap();
                    (model.name.clone(), deps)
                })
                .collect();

            // Sort before the topo sort. `topo_sort` breaks ties -- models with
            // no ordering edge between them -- by visit order, so seeding it
            // straight from a `HashMap`'s keys made the model order, and every
            // `runlist_initials` derived from it, vary run to run. This is the
            // `Project`-path twin of the GH #595 fix in
            // `db::dep_graph::model_dependency_graph_impl`.
            let mut model_runlist: Vec<&Ident<Canonical>> = model_deps.keys().collect();
            model_runlist.sort_unstable();
            let model_runlist = topo_sort(model_runlist, &model_deps);
            model_runlist
                .into_iter()
                .enumerate()
                .map(|(i, n)| (n.clone(), i))
                .collect::<HashMap<Ident<Canonical>, usize>>()
        };
        models_list.sort_unstable_by(|a, b| model_order[&a.name].cmp(&model_order[&b.name]));

        let module_instantiations = {
            let models = models_list.iter().map(|m| (m.name.as_str(), m)).collect();
            enumerate_modules(&models, "main", |model| model.name.clone()).unwrap_or_default()
        };

        // Dependency resolution + model callbacks (unit inference etc.).
        {
            let no_instantiations = BTreeSet::new();
            let mut models: HashMap<Ident<Canonical>, &ModelStage1> = HashMap::new();
            for model in models_list.iter_mut() {
                let instantiations = module_instantiations
                    .get(&model.name)
                    .unwrap_or(&no_instantiations);
                model.set_dependencies(&models, dm_dims.as_slice(), instantiations);
                if !model.implicit {
                    model_cb(&models, units_ctx, model);
                }
                models.insert(model.name.clone(), model);
            }
        }

        let ordered_models = models_list
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>();

        let models = models_list
            .into_iter()
            .map(|m| (m.name.clone(), Arc::new(m)))
            .collect();

        Project {
            datamodel: project_datamodel,
            models,
            model_order: ordered_models,
            errors: project_errors,
            // Owned field: clone the cached project-global context (the
            // interned-backed dimensions clone cheaply; only the
            // relationship-cache memo is rebuilt cold).
            dimensions_ctx: (*dims_ctx).clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_definition_errors_surface_in_project_errors() {
        use crate::db::{
            DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
            sync_from_datamodel,
        };
        use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

        let model = x_model("main", vec![x_aux("x", "1", None)]);
        let sim_specs = sim_specs_with_units("years");
        let mut dm = x_project(sim_specs, &[model]);
        // Provoke a real unit-definition error: two units claim the same
        // alias `gadget`, mapping it to *different* primary names.  Identical
        // duplicate declarations are intentionally tolerated (Vensim MDL
        // footers routinely repeat `22:` lines) so we cannot use them here.
        dm.units.push(datamodel::Unit {
            name: "widget".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["gadget".to_string()],
        });
        dm.units.push(datamodel::Unit {
            name: "doodad".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["gadget".to_string()],
        });

        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &dm);
        let diagnostics = collect_all_diagnostics(&db, sync.project);

        let unit_errs: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                d.severity == DiagnosticSeverity::Error
                    && matches!(&d.error, DiagnosticError::Unit(_))
            })
            .collect();
        assert!(
            !unit_errs.is_empty(),
            "diagnostics should contain unit definition errors, got: {:?}",
            diagnostics,
        );
        // The conflicting unit name must appear in the diagnostic so
        // callers can identify which unit definition is broken.
        assert!(
            unit_errs.iter().any(|d| {
                let v = d.variable.as_deref().unwrap_or("");
                v.contains("doodad") || v.contains("widget")
            }),
            "Diagnostic variable should include a conflicting unit name, got: {:?}",
            unit_errs,
        );
    }

    /// Building the same project repeatedly must produce the same model order
    /// and the same Initials runlists.
    ///
    /// Two `HashMap`/`HashSet` iteration orders used to leak into `topo_sort`,
    /// which breaks ties by visit order: the model runlist seeded from
    /// `model_deps.keys()` here, and the Initials runlist set in
    /// `ModelStage1::set_dependencies`. On two fresh `SimlinDb`s in ONE process
    /// this produced `["sub_a","sub_b","main"]` on one construction and
    /// `["sub_b","sub_a","main"]` on the next, with `runlist_initials` flipping
    /// to match -- so an initial value depending on an unordered pair was
    /// order-dependent. This is the `Project`-path twin of GH #595, fixed the
    /// same way (sort before the topo sort).
    ///
    /// Repeated across several independent constructions because each fresh
    /// `HashMap` gets its own `RandomState`: one agreeing pair proves nothing,
    /// a run of them is what makes a surviving nondeterminism improbable.
    #[test]
    fn from_salsa_model_order_and_initials_runlists_are_deterministic() {
        use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module, x_project};
        use std::collections::BTreeMap;

        // Two sibling sub-models with NO ordering edge between them: the tie
        // `topo_sort` has to break, and the one an unsorted seed decided by
        // hash order.
        let sub_a = x_model(
            "sub_a",
            vec![x_aux("input", "0", None), x_aux("out", "input * 2", None)],
        );
        let sub_b = x_model(
            "sub_b",
            vec![x_aux("input", "0", None), x_aux("out", "input + 1", None)],
        );
        let main = x_model(
            "main",
            vec![
                x_aux("driver", "5", None),
                // Several mutually unordered auxes, so the Initials runlist has
                // ties of its own to break.
                x_aux("alpha", "1", None),
                x_aux("beta", "2", None),
                x_aux("gamma", "3", None),
                x_module("sub_a", &[("driver", "sub_a.input")], None),
                x_module("sub_b", &[("driver", "sub_b.input")], None),
                x_aux("combined", "sub_a.out + sub_b.out", None),
            ],
        );
        let dm = x_project(sim_specs_with_units("month"), &[main, sub_a, sub_b]);

        let build_once = || {
            let db = crate::db::SimlinDb::default();
            let sync = crate::db::sync_from_datamodel(&db, &dm);
            let mut cb_order: Vec<String> = Vec::new();
            let project = Project::from_salsa(dm.clone(), &db, sync.project, |_, _, model| {
                cb_order.push(model.name.to_string());
            });
            // Initials runlists for every model, keyed so the comparison does
            // not depend on map order itself.
            let runlists: BTreeMap<String, Vec<Vec<String>>> = project
                .models
                .iter()
                .map(|(name, m)| {
                    let mut per_instantiation: Vec<Vec<String>> = m
                        .instantiations
                        .as_ref()
                        .expect("set_dependencies fills instantiations")
                        .values()
                        .map(|inst| {
                            inst.runlist_initials
                                .iter()
                                .map(|i| i.to_string())
                                .collect()
                        })
                        .collect();
                    per_instantiation.sort();
                    (name.to_string(), per_instantiation)
                })
                .collect();
            (cb_order, runlists)
        };

        // Many attempts, not a handful: each fresh `HashMap` draws a new
        // `RandomState`, but the tie `topo_sort` breaks has only a few possible
        // outcomes, so a short run agrees by chance often enough to let a
        // regression through. Measured against the unsorted code, 8 attempts
        // missed it roughly one run in three; 64 has not missed it.
        let first = build_once();
        for attempt in 1..64 {
            let next = build_once();
            assert_eq!(
                first.0, next.0,
                "model_cb order must not vary run to run (attempt {attempt})"
            );
            assert_eq!(
                first.1, next.1,
                "Initials runlists must not vary run to run (attempt {attempt})"
            );
        }
    }

    /// A module referencing a model that does not exist must not panic the
    /// legacy `from_salsa` construction path (GH #806): `module_deps`'
    /// initial-branch HashMap index and `topo_sort`'s unknown-ident assertion
    /// both degrade gracefully instead of crashing with an "internal compiler
    /// error" on this user-controllable input (a freshly-drawn module, or a
    /// reference to a deleted model). The production salsa path already rejects
    /// such a project cleanly; this guards the test-only oracle path too.
    #[test]
    fn from_salsa_module_with_missing_model_does_not_panic() {
        use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

        let module = datamodel::Variable::Module(datamodel::Module {
            ident: "m".to_string(),
            model_name: "nonexistent".to_string(),
            documentation: String::new(),
            units: None,
            references: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: None,
        });
        let model = x_model("main", vec![x_aux("x", "1", None), module]);
        let dm = x_project(sim_specs_with_units("years"), &[model]);

        // Drives Project::from_datamodel -> from_salsa -> set_dependencies ->
        // module_deps / topo_sort. Before the guards these panicked on the
        // dangling model_name; now construction returns without crashing.
        let _project = Project::from(dm);
    }

    /// GH #891: the legacy `from_salsa` path builds each model's variable map
    /// from the canonical-keyed salsa sync maps, where two variables whose
    /// names canonicalize identically already collapsed last-wins. The
    /// production compile pipeline rejects such a project upstream (GH #885);
    /// this path must record the same `DuplicateVariable` model-level error so
    /// test-written models (via `TestProject::compile` and friends) are kept
    /// honest too.
    #[test]
    fn from_datamodel_records_duplicate_variable_model_error() {
        use crate::common::ErrorCode;
        use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

        let model = x_model(
            "main",
            vec![x_aux("net flow", "1", None), x_aux("net_flow", "2", None)],
        );
        let dm = x_project(sim_specs_with_units("years"), &[model]);
        let project = Project::from(dm);

        let main = &project.models[&Ident::new("main")];
        let errors = main
            .errors
            .as_ref()
            .expect("duplicate canonical idents must record a model-level error");
        let dup = errors
            .iter()
            .find(|e| e.code == ErrorCode::DuplicateVariable)
            .unwrap_or_else(|| panic!("expected a DuplicateVariable error, got: {errors:?}"));
        let msg = dup.details.as_deref().unwrap_or("");
        assert!(
            msg.contains("'net flow'") && msg.contains("'net_flow'"),
            "message should name both colliding spellings, got: {msg}"
        );

        // The TestProject::compile surface (the main test-helper consumer of
        // this path) must report the failure rather than compiling a silently
        // different model.
        let result = crate::test_common::TestProject::new("dup")
            .aux("net flow", "1", None)
            .aux("net_flow", "2", None)
            .compile();
        let errs = match result {
            Ok(_) => panic!("TestProject::compile must fail on duplicate canonical idents"),
            Err(errs) => errs,
        };
        assert!(
            errs.iter()
                .any(|(loc, code)| loc == "main" && *code == ErrorCode::DuplicateVariable),
            "expected a main-model DuplicateVariable, got: {errs:?}"
        );
    }
}
