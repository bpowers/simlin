// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::common::{Canonical, Error, Ident};
use crate::datamodel;
use crate::dimensions::DimensionsContext;
use crate::model::ModelStage1;
use std::sync::Arc;

#[cfg(any(test, feature = "testing"))]
use {
    crate::canonicalize, crate::model::ScopeStage0, crate::units::Context,
    std::collections::BTreeSet,
};

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

/// Runs unit inference and unit checking for a model during
/// `from_salsa` construction. Matches the behavior of the salsa
/// `check_model_units` tracked function but stores results on the
/// model struct instead of accumulating via the salsa accumulator.
#[cfg(any(test, feature = "testing"))]
fn run_default_model_checks(
    models: &HashMap<Ident<Canonical>, &ModelStage1>,
    units_ctx: &Context,
    model: &mut ModelStage1,
) {
    let has_declared_units = model.variables.values().any(|var| var.units().is_some());

    let inferred_units =
        crate::units_infer::infer(models, units_ctx, model).unwrap_or_else(|err| {
            if has_declared_units
                && let crate::common::UnitError::InferenceError { code, .. } = &err
                && *code == crate::common::ErrorCode::UnitMismatch
            {
                let mut warnings = model.unit_warnings.take().unwrap_or_default();
                warnings.push(crate::common::Error {
                    kind: crate::common::ErrorKind::Model,
                    code: *code,
                    details: Some(format!("{}", err)),
                });
                model.unit_warnings = Some(warnings);
            }
            Default::default()
        });
    model.check_units(units_ctx, &inferred_units)
}

/// Retained only for tests and the AST interpreter cross-validation path
/// (AC4.6). Production compilation uses `compile_project_incremental`.
#[cfg(any(test, feature = "testing"))]
impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        Self::from_datamodel(project_datamodel)
    }
}

#[cfg(any(test, feature = "testing"))]
impl Project {
    /// Convenience constructor: creates a local salsa DB, syncs the
    /// datamodel, and builds the Project via `from_salsa` with default
    /// unit inference/checking.
    pub(crate) fn from_datamodel(project_datamodel: datamodel::Project) -> Self {
        let db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
        Self::from_salsa(
            project_datamodel,
            &db,
            sync.project,
            run_default_model_checks,
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
            project_units_context,
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
        let dims_ctx = DimensionsContext::from(dm_dims.as_slice());

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
            let module_ctx = model_module_ident_context(db, *src_model, extra_module_idents);
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
                    dimensions: &dims_ctx,
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
            dimensions_ctx: dims_ctx,
        }
    }
}

// Test-only LTM helpers: with_ltm(), with_ltm_all_links(), and their
// supporting functions. No production callers remain; retained for the
// AST interpreter cross-validation path (AC4.6).
#[cfg(any(test, feature = "testing"))]
use crate::common::{ErrorCode, ErrorKind};
#[cfg(any(test, feature = "testing"))]
use crate::datamodel::Equation;
#[cfg(any(test, feature = "testing"))]
use crate::ltm_augment::{generate_ltm_variables, generate_ltm_variables_all_links};
#[cfg(any(test, feature = "testing"))]
use crate::variable::Variable;

#[cfg(any(test, feature = "testing"))]
impl Project {
    pub fn with_ltm(self) -> crate::common::Result<Self> {
        abort_if_arrayed(&self)?;

        let ltm_vars = generate_ltm_variables(&self)?;
        if ltm_vars.is_empty() {
            return Ok(self);
        }

        Ok(Project::from_datamodel(inject_ltm_vars(
            self.datamodel,
            &ltm_vars,
        )))
    }

    pub fn with_ltm_all_links(self) -> crate::common::Result<Self> {
        abort_if_arrayed(&self)?;

        let ltm_vars = generate_ltm_variables_all_links(&self)?;
        if ltm_vars.is_empty() {
            return Ok(self);
        }

        Ok(Project::from_datamodel(inject_ltm_vars(
            self.datamodel,
            &ltm_vars,
        )))
    }
}

#[cfg(any(test, feature = "testing"))]
fn inject_ltm_vars(
    mut datamodel: datamodel::Project,
    ltm_vars: &HashMap<Ident<Canonical>, Vec<(Ident<Canonical>, datamodel::Variable)>>,
) -> datamodel::Project {
    let existing_model_names: std::collections::HashSet<String> = datamodel
        .models
        .iter()
        .map(|m| canonicalize(&m.name).into_owned())
        .collect();

    for model in &mut datamodel.models {
        let model_name = canonicalize(&model.name);
        if let Some(synthetic_vars) = ltm_vars.get(&*model_name) {
            for (_, var) in synthetic_vars {
                model.variables.push(var.clone());
            }
        }
    }

    for (model_name, synthetic_vars) in ltm_vars {
        let name_str = model_name.as_str();
        if let Some(func_name) = name_str.strip_prefix("stdlib\u{205A}") {
            let canonical_name = canonicalize(name_str).into_owned();
            if existing_model_names.contains(&canonical_name) {
                continue;
            }
            if let Some(mut stdlib_dm) = crate::stdlib::get(func_name) {
                stdlib_dm.name = name_str.to_string();
                for (_, var) in synthetic_vars {
                    stdlib_dm.variables.push(var.clone());
                }
                datamodel.models.push(stdlib_dm);
            }
        }
    }

    datamodel
}

#[cfg(any(test, feature = "testing"))]
fn abort_if_arrayed(project: &Project) -> crate::common::Result<()> {
    for (model_name, model) in &project.models {
        if model.implicit {
            continue;
        }

        for (var_name, var) in &model.variables {
            let has_arrays = match var {
                Variable::Stock { eqn, .. } | Variable::Var { eqn, .. } => {
                    matches!(
                        eqn,
                        Some(Equation::ApplyToAll(..)) | Some(Equation::Arrayed(..))
                    )
                }
                _ => false,
            };

            if has_arrays {
                return Err(Error {
                    kind: ErrorKind::Model,
                    code: ErrorCode::NotSimulatable,
                    details: Some(format!(
                        "LTM analysis does not currently support array variables. \
                        Model '{}' contains array variable '{}'. \
                        Please use a version of the model without arrays.",
                        model_name.as_str(),
                        var_name.as_str()
                    )),
                });
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_definition_errors_surface_in_project_errors() {
        use crate::common::ErrorCode;
        use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

        let model = x_model("main", vec![x_aux("x", "1", None)]);
        let sim_specs = sim_specs_with_units("years");
        let mut dm = x_project(sim_specs, &[model]);
        // Add a duplicate unit definition to provoke a unit parse error.
        dm.units.push(datamodel::Unit {
            name: "widget".to_string(),
            equation: None,
            disabled: false,
            aliases: vec![],
        });
        dm.units.push(datamodel::Unit {
            name: "widget".to_string(),
            equation: None,
            disabled: false,
            aliases: vec![],
        });

        let project = Project::from(dm);
        let unit_errs: Vec<_> = project
            .errors
            .iter()
            .filter(|e| e.code == ErrorCode::UnitDefinitionErrors)
            .collect();
        assert!(
            !unit_errs.is_empty(),
            "Project.errors should contain UnitDefinitionErrors, got: {:?}",
            project.errors,
        );
        // The failing unit name must appear in the error details so
        // callers can identify which unit definition is broken.
        assert!(
            unit_errs
                .iter()
                .any(|e| e.details.as_deref().unwrap_or("").contains("widget")),
            "Error details should include the unit name 'widget', got: {:?}",
            unit_errs,
        );
    }

    #[test]
    fn test_project_with_ltm() {
        use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

        // Create a simple model with a reinforcing loop
        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &[], None),
                x_flow("births", "population * birth_rate", None),
                x_aux("birth_rate", "0.02", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project_datamodel = x_project(sim_specs, &[model]);
        let project = Project::from(project_datamodel);

        // Apply LTM instrumentation
        let ltm_project = project.with_ltm().unwrap();

        // Check that the project has been augmented with LTM variables
        let main_model = ltm_project
            .datamodel
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("Should have main model");

        // Count LTM variables
        let ltm_var_count = main_model
            .variables
            .iter()
            .filter(|v| v.get_ident().starts_with("$⁚ltm⁚"))
            .count();

        // We should have link score and loop score variables
        assert!(ltm_var_count > 0, "Should have added LTM variables");

        // Check for specific types of LTM variables
        let has_link_scores = main_model
            .variables
            .iter()
            .any(|v| v.get_ident().starts_with("$⁚ltm⁚link_score⁚"));
        let has_loop_scores = main_model
            .variables
            .iter()
            .any(|v| v.get_ident().starts_with("$⁚ltm⁚loop_score⁚"));

        assert!(has_link_scores, "Should have link score variables");
        assert!(has_loop_scores, "Should have loop score variables");
    }

    #[test]
    fn test_project_with_ltm_simulation() {
        use crate::test_common::TestProject;
        use std::sync::Arc;

        // Create a project with a simple reinforcing loop
        let project = TestProject::new("test_ltm_simulation")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * birth_rate", None)
            .aux("birth_rate", "0.02", None)
            .compile()
            .expect("Project should compile");

        // Apply LTM augmentation
        let ltm_project = project.with_ltm().expect("Should augment with LTM");

        // Build and run the simulation
        let project_rc = Arc::new(ltm_project);

        let sim = crate::interpreter::Simulation::new(&project_rc, "main")
            .expect("Should create simulation");

        let results = sim
            .run_to_end()
            .expect("Simulation should run successfully");

        // Check that LTM variables are in the results
        let var_names: Vec<_> = results.offsets.keys().map(|k| k.as_str()).collect();

        // Should have link score variables
        let link_score_vars: Vec<_> = var_names
            .iter()
            .filter(|name| name.starts_with("$⁚ltm⁚link_score⁚"))
            .collect();
        assert!(
            !link_score_vars.is_empty(),
            "Should have link score variables in simulation results"
        );

        // Should have loop score variables
        let loop_score_vars: Vec<_> = var_names
            .iter()
            .filter(|name| name.starts_with("$⁚ltm⁚loop_score⁚"))
            .collect();
        assert!(
            !loop_score_vars.is_empty(),
            "Should have loop score variables in simulation results"
        );

        // Verify specific link scores exist
        let has_pop_to_births = var_names
            .iter()
            .any(|name| name.contains("link_score⁚population→births"));
        let has_births_to_pop = var_names
            .iter()
            .any(|name| name.contains("link_score⁚births→population"));

        assert!(
            has_pop_to_births || has_births_to_pop,
            "Should have specific link score variables for the feedback loop"
        );
    }

    #[test]
    fn test_project_with_ltm_no_loops() {
        use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

        // Create a model with no loops
        let model = x_model(
            "main",
            vec![
                x_aux("input", "10", None),
                x_aux("output", "input * 2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project_datamodel = x_project(sim_specs, &[model]);
        let project = Project::from(project_datamodel);

        // Apply LTM instrumentation
        let ltm_project = project.with_ltm().unwrap();

        // Check that no LTM variables were added (no loops to instrument)
        let main_model = ltm_project
            .datamodel
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("Should have main model");

        let ltm_var_count = main_model
            .variables
            .iter()
            .filter(|v| v.get_ident().starts_with("$⁚ltm⁚"))
            .count();

        assert_eq!(
            ltm_var_count, 0,
            "Should not add LTM variables when no loops exist"
        );
    }

    #[test]
    fn test_project_with_ltm_arrays_error() {
        use crate::datamodel::{Aux, Variable as DatamodelVariable};
        use crate::testutils::{sim_specs_with_units, x_model, x_project};

        // Create a model with an array variable
        let mut model = x_model("main", vec![]);

        // Add an array variable manually
        model.variables.push(DatamodelVariable::Aux(Aux {
            ident: "array_var".to_string(),
            equation: Equation::ApplyToAll(vec!["dimension1".to_string()], "10".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));

        let sim_specs = sim_specs_with_units("years");
        let project_datamodel = x_project(sim_specs, &[model]);
        let project = Project::from(project_datamodel);

        // Try to apply LTM instrumentation - should fail
        let result = project.with_ltm();

        assert!(result.is_err(), "Should error when arrays are present");

        if let Err(e) = result {
            assert!(e.details.as_ref().unwrap().contains("array variables"));
            assert!(e.details.as_ref().unwrap().contains("array_var"));
        }
    }

    #[test]
    fn test_ltm_balancing_loop_score_polarity() {
        use crate::test_common::TestProject;
        use std::sync::Arc;

        // Create a model with a BALANCING loop: stock → gap → inflow → stock
        // This is a goal-seeking structure where inflow decreases as stock approaches goal
        let project = TestProject::new("test_ltm_polarity")
            .with_sim_time(0.0, 5.0, 0.25)
            .aux("goal", "100", None)
            .stock("level", "50", &["adjustment"], &[], None)
            .aux("gap", "goal - level", None)
            .aux("adjustment_time", "5", None)
            .flow("adjustment", "gap / adjustment_time", None)
            .compile()
            .expect("Project should compile");

        // Apply LTM augmentation
        let ltm_project = project.with_ltm().expect("Should augment with LTM");

        // Build and run the simulation
        let project_rc = Arc::new(ltm_project);
        let sim = crate::interpreter::Simulation::new(&project_rc, "main")
            .expect("Should create simulation");

        let results = sim
            .run_to_end()
            .expect("Simulation should run successfully");

        // Find the loop score variable (should be b1 for balancing)
        let loop_score_var = results
            .offsets
            .keys()
            .find(|k| k.as_str().starts_with("$⁚ltm⁚loop_score⁚"))
            .expect("Should have a loop score variable");

        // Get the offset for this variable
        let offset = results.offsets[loop_score_var];
        let num_timesteps = results.data.len() / results.offsets.len();
        let num_vars = results.offsets.len();

        // Filter out 0 (initial timesteps with no dynamics, and equilibrium)
        let valid_scores: Vec<f64> = (1..num_timesteps)
            .map(|step| results.data[step * num_vars + offset])
            .filter(|v| *v != 0.0)
            .collect();

        // For a balancing loop, all valid scores should be NEGATIVE
        // (The loop counteracts changes)
        assert!(
            !valid_scores.is_empty(),
            "Should have some valid loop score values"
        );

        for score in &valid_scores {
            assert!(
                *score < 0.0,
                "Balancing loop score should be negative, got {score}"
            );
        }
    }

    #[test]
    fn test_ltm_reinforcing_loop_score_polarity() {
        use crate::test_common::TestProject;
        use std::sync::Arc;

        // Create a model with a REINFORCING loop: population → births → population
        // This is exponential growth
        let project = TestProject::new("test_ltm_reinforcing")
            .with_sim_time(0.0, 5.0, 0.25)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * birth_rate", None)
            .aux("birth_rate", "0.1", None)
            .compile()
            .expect("Project should compile");

        // Apply LTM augmentation
        let ltm_project = project.with_ltm().expect("Should augment with LTM");

        // Build and run the simulation
        let project_rc = Arc::new(ltm_project);
        let sim = crate::interpreter::Simulation::new(&project_rc, "main")
            .expect("Should create simulation");

        let results = sim
            .run_to_end()
            .expect("Simulation should run successfully");

        // Find the loop score variable (should be r1 for reinforcing)
        let loop_score_var = results
            .offsets
            .keys()
            .find(|k| k.as_str().starts_with("$⁚ltm⁚loop_score⁚"))
            .expect("Should have a loop score variable");

        // Get the offset for this variable
        let offset = results.offsets[loop_score_var];
        let num_timesteps = results.data.len() / results.offsets.len();
        let num_vars = results.offsets.len();

        // Filter out 0 (initial timesteps with no dynamics)
        let valid_scores: Vec<f64> = (1..num_timesteps)
            .map(|step| results.data[step * num_vars + offset])
            .filter(|v| *v != 0.0)
            .collect();

        // For a reinforcing loop, all valid scores should be POSITIVE
        // (The loop amplifies changes)
        assert!(
            !valid_scores.is_empty(),
            "Should have some valid loop score values"
        );

        for score in &valid_scores {
            assert!(
                *score > 0.0,
                "Reinforcing loop score should be positive, got {score}"
            );
        }
    }

    #[test]
    fn test_project_with_ltm_all_links() {
        use crate::test_common::TestProject;

        let project = TestProject::new("test_all_links")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * birth_rate", None)
            .aux("birth_rate", "fractional_growth_rate", None)
            .aux("fractional_growth_rate", "0.02 * (1 - fraction_used)", None)
            .aux("fraction_used", "population / carrying_capacity", None)
            .aux("carrying_capacity", "1000", None)
            .compile()
            .expect("Project should compile");

        // Apply all-links LTM augmentation
        let ltm_project = project
            .with_ltm_all_links()
            .expect("Should augment with all-links LTM");

        let main_model = ltm_project
            .datamodel
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("Should have main model");

        // Should have link score variables
        let link_score_count = main_model
            .variables
            .iter()
            .filter(|v| v.get_ident().starts_with("$⁚ltm⁚link_score⁚"))
            .count();
        assert!(
            link_score_count > 0,
            "Should have link score variables in all-links mode"
        );

        // Should NOT have loop score variables (those are computed post-sim)
        let loop_score_count = main_model
            .variables
            .iter()
            .filter(|v| v.get_ident().starts_with("$⁚ltm⁚loop_score⁚"))
            .count();
        assert_eq!(
            loop_score_count, 0,
            "All-links mode should not have loop score variables"
        );

        // Should be simulatable
        let project_rc = std::sync::Arc::new(ltm_project);
        let sim = crate::interpreter::Simulation::new(&project_rc, "main")
            .expect("Should create simulation");
        let results = sim
            .run_to_end()
            .expect("Simulation with all-links LTM should run successfully");

        // Verify link score variables are in results
        let link_score_vars: Vec<_> = results
            .offsets
            .keys()
            .filter(|k| k.as_str().starts_with("$⁚ltm⁚link_score⁚"))
            .collect();
        assert!(
            !link_score_vars.is_empty(),
            "Should have link score variables in simulation results"
        );
    }
}
