// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{Canonical, Error, Ident};
use crate::datamodel;
use crate::dimensions::DimensionsContext;
use crate::model::ModelStage1;
use std::sync::Arc;

use {crate::model::ScopeStage0, crate::units::Context, std::collections::BTreeSet};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Project {
    pub datamodel: datamodel::Project,
    // these are Arcs so that multiple Modules created by the compiler can
    // reference the same Model instance
    pub models: HashMap<Ident<Canonical>, Arc<ModelStage1>>,
    #[allow(dead_code)]
    model_order: Vec<Ident<Canonical>>,
    /// Project-level errors. With the `from_salsa` construction path,
    /// unit definition errors are recovered from the salsa accumulator
    /// in `project_units_context` so callers can still inspect them.
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
    /// All variable parsing comes from salsa-cached results (no
    /// redundant parsing). The caller provides the salsa DB and
    /// `SourceProject`; the `model_cb` runs per non-stdlib model
    /// after dependency resolution (typically unit inference/checking).
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
            CompilationDiagnostic, DiagnosticError, model_module_ident_context,
            parse_source_variable_with_module_context, project_datamodel_dims,
            project_dimensions_context, project_units_context,
        };
        use crate::model::{ModelStage0, VariableStage0, enumerate_modules};

        let units_ctx = project_units_context(db, source_project);

        // Recover unit definition errors from the salsa accumulator so
        // callers that inspect Project.errors (e.g. tests) still see them.
        let project_errors: Vec<Error> =
            project_units_context::accumulated::<CompilationDiagnostic>(db, source_project)
                .into_iter()
                .filter_map(|cd| match &cd.0.error {
                    DiagnosticError::Unit(unit_err) => {
                        let name = cd.0.variable.as_deref().unwrap_or("unknown");
                        Some(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::UnitDefinitionErrors,
                            details: Some(format!("{name}: {unit_err}")),
                        })
                    }
                    _ => None,
                })
                .collect();
        let dm_dims = project_datamodel_dims(db, source_project);
        // Read the project-global dimension context from the salsa-cached query
        // rather than rebuilding it here (it is canonicalized once per project).
        let dims_ctx = project_dimensions_context(db, source_project);

        // Build ModelStage0 from salsa-parsed variables for all models.
        let project_models = source_project.models(db);
        let mut all_s0: Vec<ModelStage0> = Vec::new();
        for (canonical_name, src_model) in project_models.iter() {
            // Only treat a model as implicit/stdlib if it matches one
            // of the known stdlib model names, not just any model whose
            // name starts with the stdlib prefix.
            let is_stdlib = canonical_name
                .strip_prefix("stdlib\u{205A}")
                .is_some_and(|suffix| crate::stdlib::MODEL_NAMES.contains(&suffix));
            let model_name = src_model.name(db);
            let src_vars = src_model.variables(db);
            // For stdlib models, ALL variable names must be module idents so
            // PREVIOUS(module_input) rewrites through a scalar helper aux
            // instead of reading a transient module-input slot directly.
            let extra_module_idents: Vec<String> = if is_stdlib {
                src_vars.keys().cloned().collect()
            } else {
                vec![]
            };
            let module_ctx =
                model_module_ident_context(db, *src_model, source_project, extra_module_idents);
            let mut var_list: Vec<VariableStage0> = Vec::new();
            let mut implicit_dm: Vec<datamodel::Variable> = Vec::new();
            for (_vname, svar) in src_vars.iter() {
                let parsed = parse_source_variable_with_module_context(
                    db,
                    *svar,
                    source_project,
                    module_ctx,
                );
                var_list.push(parsed.variable.clone());
                implicit_dm.extend(parsed.implicit_vars.iter().cloned());
            }
            // Parse implicit vars (SMOOTH/DELAY expansion).
            let mut nested_implicit: Vec<datamodel::Variable> = Vec::new();
            var_list.extend(implicit_dm.into_iter().map(|dm_var| {
                crate::variable::parse_var(
                    dm_dims,
                    &dm_var,
                    &mut nested_implicit,
                    units_ctx,
                    |mi| Ok(Some(mi.clone())),
                )
            }));
            debug_assert!(
                nested_implicit.is_empty(),
                "implicit vars should not produce further implicit vars"
            );
            let variables: HashMap<Ident<Canonical>, VariableStage0> = var_list
                .into_iter()
                .map(|v| (Ident::new(v.ident()), v))
                .collect();
            all_s0.push(ModelStage0 {
                ident: Ident::new(model_name),
                display_name: model_name.clone(),
                variables,
                errors: None,
                implicit: is_stdlib,
                is_macro: src_model.macro_spec(db).is_some(),
                macro_params: crate::model::macro_param_idents(src_model.macro_spec(db).as_ref()),
            });
        }

        // ModelStage0 -> ModelStage1
        let models_s0: HashMap<Ident<Canonical>, &ModelStage0> =
            all_s0.iter().map(|m| (m.ident.clone(), m)).collect();
        let mut models_list: Vec<ModelStage1> = all_s0
            .iter()
            .map(|ms0| {
                let scope = ScopeStage0 {
                    models: &models_s0,
                    dimensions: dims_ctx,
                    model_name: ms0.ident.as_str(),
                };
                ModelStage1::new(&scope, ms0)
            })
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

            let model_runlist: Vec<&Ident<Canonical>> = model_deps.keys().collect();
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
}
