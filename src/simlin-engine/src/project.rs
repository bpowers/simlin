// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};

use crate::canonicalize;
use crate::common::{Canonical, Error, ErrorCode, ErrorKind, Ident};
use crate::datamodel::{self, Equation};
use crate::dimensions::DimensionsContext;
use crate::ltm_augment::{generate_ltm_variables, generate_ltm_variables_all_links};
use crate::model::{ModelStage0, ModelStage1, ScopeStage0};
use crate::units::Context;
use crate::variable::Variable;
use std::sync::Arc;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct Project {
    pub datamodel: datamodel::Project,
    // these are Arcs so that multiple Modules created by the compiler can
    // reference the same Model instance
    pub models: HashMap<Ident<Canonical>, Arc<ModelStage1>>,
    #[allow(dead_code)]
    model_order: Vec<Ident<Canonical>>,
    pub errors: Vec<Error>,
    /// Cached dimension context for subdimension lookups
    pub dimensions_ctx: DimensionsContext,
}

impl Project {
    pub fn name(&self) -> &str {
        &self.datamodel.name
    }

    /// Create a new project with LTM instrumentation
    pub fn with_ltm(self) -> crate::common::Result<Self> {
        // TODO: the current LTM implementation needs extensions to work with arrayed models.
        abort_if_arrayed(&self)?;

        let ltm_vars = generate_ltm_variables(&self)?;
        if ltm_vars.is_empty() {
            // No loops detected, return original Project
            return Ok(self);
        }

        let mut new_datamodel = self.datamodel.clone();

        // Augment all the models with their synthetic LTM variables
        for model in &mut new_datamodel.models {
            let model_name = canonicalize(&model.name);

            if let Some(synthetic_vars) = ltm_vars.get(&*model_name) {
                for (_, var) in synthetic_vars {
                    model.variables.push(var.clone());
                }
            }
        }

        // Rebuild the Project with the added LTM variables
        Ok(Project::from(new_datamodel))
    }

    /// Create a new project with link score variables for ALL causal links.
    ///
    /// Unlike `with_ltm()` which only instruments detected loops, this generates
    /// link score variables for every causal connection in the model. Used as a
    /// prerequisite for `ltm_finding::discover_loops()` which finds important
    /// loops heuristically from the simulation results.
    ///
    /// No loop score or relative loop score variables are generated -- those
    /// are computed post-simulation by `discover_loops()`.
    pub fn with_ltm_all_links(self) -> crate::common::Result<Self> {
        abort_if_arrayed(&self)?;

        let ltm_vars = generate_ltm_variables_all_links(&self)?;
        if ltm_vars.is_empty() {
            return Ok(self);
        }

        let mut new_datamodel = self.datamodel.clone();

        for model in &mut new_datamodel.models {
            let model_name = canonicalize(&model.name);

            if let Some(synthetic_vars) = ltm_vars.get(&*model_name) {
                for (_, var) in synthetic_vars {
                    model.variables.push(var.clone());
                }
            }
        }

        Ok(Project::from(new_datamodel))
    }
}

impl From<datamodel::Project> for Project {
    fn from(project_datamodel: datamodel::Project) -> Self {
        Self::base_from(project_datamodel, |models, units_ctx, model| {
            // Run unit inference to compute units for variables without explicit declarations.
            // The check_units call below validates inferred units against declared units.
            // Check if the model has any variables with declared units.
            // If not, we skip surfacing unit inference errors since the model
            // wasn't designed with dimensional analysis in mind.
            let has_declared_units = model.variables.values().any(|var| var.units().is_some());

            let inferred_units = crate::units_infer::infer(models, units_ctx, model)
                .unwrap_or_else(|err| {
                    // Only surface unit mismatches for models that have declared units.
                    // Store in unit_warnings (not errors) so simulation can still proceed.
                    // Unit mismatches are common in real-world models and shouldn't block simulation.
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
        })
    }
}

impl Project {
    pub(crate) fn base_from<F>(project_datamodel: datamodel::Project, mut model_cb: F) -> Self
    where
        F: FnMut(&HashMap<Ident<Canonical>, &ModelStage1>, &Context, &mut ModelStage1),
    {
        use crate::common::{ErrorCode, ErrorKind, topo_sort};
        use crate::model::enumerate_modules;

        // first, build the unit context.
        // TODO: there is probably a shared/common core of units we should
        //       pull in

        let mut project_errors = vec![];

        let units_ctx =
            Context::new_with_builtins(&project_datamodel.units, &project_datamodel.sim_specs)
                .unwrap_or_else(|errs| {
                    for (unit_name, unit_errs) in errs {
                        for err in unit_errs {
                            project_errors.push(Error {
                                kind: ErrorKind::Model,
                                code: ErrorCode::UnitDefinitionErrors,
                                details: Some(format!("{unit_name}: {err}")),
                            });
                        }
                    }
                    Default::default()
                });

        // next, pull in all the models from the stdlib
        let mut models_list: Vec<ModelStage0> = crate::stdlib::MODEL_NAMES
            .iter()
            .map(|name| crate::stdlib::get(name).unwrap())
            .map(|x_model| {
                ModelStage0::new(&x_model, &project_datamodel.dimensions, &units_ctx, true)
            })
            .collect();

        // extend the list with the models from the project/XMILE file
        models_list.extend(
            project_datamodel
                .models
                .iter()
                .map(|m| ModelStage0::new(m, &project_datamodel.dimensions, &units_ctx, false)),
        );

        let models: HashMap<Ident<Canonical>, ModelStage0> = models_list
            .iter()
            .cloned()
            .map(|m| (m.ident.clone(), m))
            .collect();

        let dims_ctx = DimensionsContext::from(&project_datamodel.dimensions);
        let mut models_list: Vec<ModelStage1> = models_list
            .into_iter()
            .map(|model| {
                let scope = ScopeStage0 {
                    models: &models,
                    dimensions: &dims_ctx,
                    model_name: model.ident.as_str(),
                };
                ModelStage1::new(&scope, &model)
            })
            .collect();

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

        // sort our model list so that the dependency resolution below works
        models_list.sort_unstable_by(|a, b| model_order[&a.name].cmp(&model_order[&b.name]));

        let module_instantiations = {
            let models = models_list.iter().map(|m| (m.name.as_str(), m)).collect();
            // FIXME: ignoring the result here because if we have errors, it doesn't really matter
            enumerate_modules(&models, "main", |model| model.name.clone()).unwrap_or_default()
        };

        // dependency resolution; we need to do this as a second pass
        // to ensure we have the information available for modules
        {
            let no_instantiations = BTreeSet::new();
            let mut models: HashMap<Ident<Canonical>, &ModelStage1> = HashMap::new();
            for model in models_list.iter_mut() {
                let instantiations = module_instantiations
                    .get(&model.name)
                    .unwrap_or(&no_instantiations);
                model.set_dependencies(&models, &project_datamodel.dimensions, instantiations);
                // things like unit inference happen through this callback
                // Skip unit inference for implicit (stdlib) models as they are generic
                // templates that only make sense when instantiated with specific inputs
                if !model.implicit {
                    model_cb(&models, &units_ctx, model);
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

/// Check if any model in the project contains array variables
fn abort_if_arrayed(project: &Project) -> crate::common::Result<()> {
    for (model_name, model) in &project.models {
        // Skip implicit (stdlib) models
        if model.implicit {
            continue;
        }

        // Check each variable for array dimensions
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
            .any(|v| v.get_ident().starts_with("$⁚ltm⁚abs_loop_score⁚"));

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
            .filter(|name| name.starts_with("$⁚ltm⁚abs_loop_score⁚"))
            .collect();
        assert!(
            !loop_score_vars.is_empty(),
            "Should have loop score variables in simulation results"
        );

        // Verify specific link scores exist
        let has_pop_to_births = var_names
            .iter()
            .any(|name| name.contains("link_score⁚population⁚births"));
        let has_births_to_pop = var_names
            .iter()
            .any(|name| name.contains("link_score⁚births⁚population"));

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
            equation: Equation::ApplyToAll(vec!["dimension1".to_string()], "10".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
            ai_state: None,
            uid: None,
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
            .find(|k| k.as_str().starts_with("$⁚ltm⁚abs_loop_score⁚"))
            .expect("Should have a loop score variable");

        // Get the offset for this variable
        let offset = results.offsets[loop_score_var];
        let num_timesteps = results.data.len() / results.offsets.len();
        let num_vars = results.offsets.len();

        // Check values after the initial NaN timesteps, filtering out 0 (equilibrium)
        let valid_scores: Vec<f64> = (1..num_timesteps)
            .map(|step| results.data[step * num_vars + offset])
            .filter(|v| !v.is_nan() && *v != 0.0)
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
            .find(|k| k.as_str().starts_with("$⁚ltm⁚abs_loop_score⁚"))
            .expect("Should have a loop score variable");

        // Get the offset for this variable
        let offset = results.offsets[loop_score_var];
        let num_timesteps = results.data.len() / results.offsets.len();
        let num_vars = results.offsets.len();

        // Check values after the initial NaN timesteps
        let valid_scores: Vec<f64> = (1..num_timesteps)
            .map(|step| results.data[step * num_vars + offset])
            .filter(|v| !v.is_nan() && *v != 0.0)
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
            .filter(|v| v.get_ident().starts_with("$⁚ltm⁚abs_loop_score⁚"))
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
