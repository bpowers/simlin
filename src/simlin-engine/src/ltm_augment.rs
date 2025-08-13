// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM project augmentation - adds synthetic variables for link and loop scores

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident, Result};
use crate::datamodel::Equation;
use crate::ltm::{Link, Loop, detect_loops};
use crate::project::Project;
use crate::variable::{Variable, identifier_set};

/// Augment a project with LTM synthetic variables
/// Returns a map of model name to synthetic variables to add
pub fn generate_ltm_variables(
    project: &Project,
) -> Result<HashMap<Ident<Canonical>, Vec<(Ident<Canonical>, Variable)>>> {
    // First, detect all loops in the project
    let loops = detect_loops(project)?;

    let mut result = HashMap::new();

    // For each model, generate synthetic variables
    for (model_name, model_loops) in &loops {
        if let Some(model) = project.models.get(model_name) {
            // Skip implicit models
            if model.implicit {
                continue;
            }

            let mut synthetic_vars = Vec::new();

            // Collect all unique links from all loops
            let mut all_links = HashSet::new();
            for loop_item in model_loops {
                for link in &loop_item.links {
                    all_links.insert(link.clone());
                }
            }

            // Generate link score variables
            let link_score_vars = generate_link_score_variables(&all_links, &model.variables);

            // Generate loop score variables
            let loop_score_vars = generate_loop_score_variables(model_loops);

            // Collect all synthetic variables
            for (var_name, var) in link_score_vars {
                synthetic_vars.push((var_name, var));
            }

            for (var_name, var) in loop_score_vars {
                synthetic_vars.push((var_name, var));
            }

            if !synthetic_vars.is_empty() {
                result.insert(model_name.clone(), synthetic_vars);
            }
        }
    }

    Ok(result)
}

/// Generate link score variables for all links
fn generate_link_score_variables(
    links: &HashSet<Link>,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> HashMap<Ident<Canonical>, Variable> {
    let mut link_vars = HashMap::new();

    for link in links {
        let var_name = format!(
            "_ltm_link_{}_{}",
            sanitize_for_var_name(link.from.as_str()),
            sanitize_for_var_name(link.to.as_str())
        );

        // Get the equation of the 'to' variable
        if let Some(to_var) = variables.get(&link.to) {
            let equation = generate_link_score_equation(&link.from, &link.to, to_var, variables);

            // Create the synthetic variable
            let ltm_var = create_aux_variable(&var_name, &equation);
            link_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
        }
    }

    link_vars
}

/// Generate loop score variables for all loops
fn generate_loop_score_variables(loops: &[Loop]) -> HashMap<Ident<Canonical>, Variable> {
    let mut loop_vars = HashMap::new();

    // First, generate absolute loop scores
    for loop_item in loops {
        let var_name = format!("_ltm_loop_{}", loop_item.id);

        // Generate equation as product of link scores
        let equation = generate_loop_score_equation(loop_item);

        // Create the synthetic variable
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
    }

    // Then, generate relative loop scores if there are multiple loops
    if loops.len() > 1 {
        for loop_item in loops {
            let var_name = format!("_ltm_rel_loop_{}", loop_item.id);

            // Generate equation for relative loop score
            let equation = generate_relative_loop_score_equation(&loop_item.id, loops);

            // Create the synthetic variable
            let ltm_var = create_aux_variable(&var_name, &equation);
            loop_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
        }
    }

    loop_vars
}

/// Generate the equation for a link score variable
fn generate_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    // Check if this is a flow-to-stock link
    let is_flow_to_stock = matches!(to_var, Variable::Stock { .. })
        && matches!(
            all_vars.get(from),
            Some(Variable::Var { is_flow: true, .. })
        );

    // Check if this is a stock-to-flow link
    let is_stock_to_flow = matches!(all_vars.get(from), Some(Variable::Stock { .. }))
        && matches!(to_var, Variable::Var { is_flow: true, .. });

    if is_flow_to_stock {
        // Use flow-to-stock formula
        generate_flow_to_stock_equation(from.as_str(), to.as_str(), to_var)
    } else if is_stock_to_flow {
        // Use stock-to-flow formula
        generate_stock_to_flow_equation(from, to, to_var, all_vars)
    } else {
        // Use standard auxiliary-to-auxiliary formula
        generate_auxiliary_to_auxiliary_equation(from, to, to_var)
    }
}

/// Generate auxiliary-to-auxiliary link score equation
fn generate_auxiliary_to_auxiliary_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    to_var: &Variable,
) -> String {
    // Get the equation text of the 'to' variable
    let to_equation = match to_var {
        Variable::Stock {
            eqn: Some(Equation::Scalar(eq, _)),
            ..
        } => eq.clone(),
        Variable::Var {
            eqn: Some(Equation::Scalar(eq, _)),
            ..
        } => eq.clone(),
        _ => "0".to_string(), // Default if no equation
    };

    // Get dependencies of the 'to' variable
    let deps = if let Some(ast) = to_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    // Build the partial equation: substitute PREVIOUS(dep) for all deps except 'from'
    let mut partial_eq = to_equation.clone();
    for dep in &deps {
        if dep != from {
            // Replace whole word occurrences of the dependency
            let pattern = format!(r"\b{}\b", regex::escape(dep.as_str()));
            let replacement = format!("PREVIOUS({})", dep.as_str());
            if let Ok(re) = regex::Regex::new(&pattern) {
                partial_eq = re
                    .replace_all(&partial_eq, replacement.as_str())
                    .to_string();
            }
        }
    }

    format!(
        "IF THEN ELSE(\
            (({to} - PREVIOUS({to})) = 0) :OR: (({from} - PREVIOUS({from})) = 0), \
            0, \
            ABS((({partial_eq}) - PREVIOUS({to})) / ({to} - PREVIOUS({to}))) * \
            IF THEN ELSE(\
                ({from} - PREVIOUS({from})) = 0, \
                0, \
                SIGN((({partial_eq}) - PREVIOUS({to})) / ({from} - PREVIOUS({from})))\
            )\
        )",
        to = to.as_str(),
        from = from.as_str(),
        partial_eq = partial_eq
    )
}

/// Generate flow-to-stock link score equation
fn generate_flow_to_stock_equation(flow: &str, stock: &str, stock_var: &Variable) -> String {
    // Check if this flow is an inflow or outflow
    let is_inflow = if let Variable::Stock { inflows, .. } = stock_var {
        inflows.iter().any(|f| f.as_str() == flow)
    } else {
        true // Default to inflow
    };

    let sign = if is_inflow { "" } else { "-" };

    format!(
        "IF THEN ELSE(\
            (({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock})))) = 0, \
            0, \
            {sign}(({flow} - PREVIOUS({flow})) / \
                (({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock})))))\
        )"
    )
}

/// Generate stock-to-flow link score equation
fn generate_stock_to_flow_equation(
    stock: &Ident<Canonical>,
    flow: &Ident<Canonical>,
    flow_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    // For stock-to-flow, we need to calculate how the stock influences the flow
    // This is similar to auxiliary-to-auxiliary but we know the 'from' is a stock

    // Get the flow equation text
    let flow_equation = match flow_var {
        Variable::Var {
            eqn: Some(Equation::Scalar(eq, _)),
            ..
        } => eq.clone(),
        _ => "0".to_string(),
    };

    // Get dependencies of the flow variable
    let deps = if let Some(ast) = flow_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    // Build the partial equation: substitute PREVIOUS(dep) for all deps except 'stock'
    let mut partial_eq = flow_equation.clone();
    for dep in &deps {
        if dep != stock {
            // Replace whole word occurrences of the dependency
            let pattern = format!(r"\b{}\b", regex::escape(dep.as_str()));
            let replacement = format!("PREVIOUS({})", dep.as_str());
            if let Ok(re) = regex::Regex::new(&pattern) {
                partial_eq = re
                    .replace_all(&partial_eq, replacement.as_str())
                    .to_string();
            }
        }
    }

    // Check if this flow affects the stock (is it an inflow or outflow?)
    let stock_var = all_vars.get(stock);
    let is_affecting_stock = if let Some(Variable::Stock {
        inflows, outflows, ..
    }) = stock_var
    {
        inflows.contains(flow) || outflows.contains(flow)
    } else {
        false
    };

    if is_affecting_stock {
        // Use a formula that considers the feedback nature
        format!(
            "IF THEN ELSE(\
                (({flow} - PREVIOUS({flow})) = 0) :OR: (({stock} - PREVIOUS({stock})) = 0), \
                0, \
                ABS((({partial_eq}) - PREVIOUS({flow})) / ({flow} - PREVIOUS({flow}))) * \
                IF THEN ELSE(\
                    ({stock} - PREVIOUS({stock})) = 0, \
                    0, \
                    SIGN((({partial_eq}) - PREVIOUS({flow})) / ({stock} - PREVIOUS({stock})))\
                )\
            )",
            flow = flow.as_str(),
            stock = stock.as_str(),
            partial_eq = partial_eq
        )
    } else {
        // Stock influences flow but flow doesn't feed back to stock - use standard formula
        format!(
            "IF THEN ELSE(\
                (({flow} - PREVIOUS({flow})) = 0) :OR: (({stock} - PREVIOUS({stock})) = 0), \
                0, \
                ABS((({partial_eq}) - PREVIOUS({flow})) / ({flow} - PREVIOUS({flow}))) * \
                IF THEN ELSE(\
                    ({stock} - PREVIOUS({stock})) = 0, \
                    0, \
                    SIGN((({partial_eq}) - PREVIOUS({flow})) / ({stock} - PREVIOUS({stock})))\
                )\
            )",
            flow = flow.as_str(),
            stock = stock.as_str(),
            partial_eq = partial_eq
        )
    }
}

/// Generate the equation for a loop score variable
fn generate_loop_score_equation(loop_item: &Loop) -> String {
    // Product of all link scores in the loop
    let link_score_names: Vec<String> = loop_item
        .links
        .iter()
        .map(|link| {
            format!(
                "_ltm_link_{}_{}",
                sanitize_for_var_name(link.from.as_str()),
                sanitize_for_var_name(link.to.as_str())
            )
        })
        .collect();

    if link_score_names.is_empty() {
        "0".to_string()
    } else {
        link_score_names.join(" * ")
    }
}

/// Generate the equation for a relative loop score variable
fn generate_relative_loop_score_equation(loop_id: &str, all_loops: &[Loop]) -> String {
    // Relative loop score = abs(loop_score) / sum(abs(all_loop_scores))
    let loop_score_var = format!("_ltm_loop_{}", loop_id);

    // Build sum of absolute values of all loop scores
    let all_loop_scores: Vec<String> = all_loops
        .iter()
        .map(|loop_item| format!("ABS(_ltm_loop_{})", loop_item.id))
        .collect();

    let sum_expr = if all_loop_scores.is_empty() {
        "1".to_string() // Avoid division by zero
    } else {
        all_loop_scores.join(" + ")
    };

    // Relative score formula with protection against division by zero
    format!(
        "IF THEN ELSE(\
            ({sum}) = 0, \
            0, \
            ABS({loop_score}) / ({sum})\
        )",
        loop_score = loop_score_var,
        sum = sum_expr
    )
}

/// Create an auxiliary variable with the given equation
fn create_aux_variable(name: &str, equation: &str) -> Variable {
    // For now, create a simplified Variable directly
    // In a full implementation, this would properly parse the equation
    Variable::Var {
        ident: crate::common::canonicalize(name),
        ast: None, // Would be parsed from equation
        init_ast: None,
        eqn: Some(Equation::Scalar(equation.to_string(), None)),
        units: None,
        table: None,
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    }
}

/// Sanitize a variable name for use in generated variable names
fn sanitize_for_var_name(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_for_var_name() {
        assert_eq!(sanitize_for_var_name("simple"), "simple");
        assert_eq!(sanitize_for_var_name("with space"), "with_space");
        assert_eq!(
            sanitize_for_var_name("dots.and.dashes-here"),
            "dots_and_dashes_here"
        );
        assert_eq!(sanitize_for_var_name("special!@#$%"), "special_____");
    }

    #[test]
    fn test_generate_loop_score_equation() {
        use crate::ltm::LoopPolarity;

        let loop_item = Loop {
            id: "R1".to_string(),
            links: vec![
                Link {
                    from: crate::common::canonicalize("x"),
                    to: crate::common::canonicalize("y"),
                    polarity: crate::ltm::LinkPolarity::Positive,
                },
                Link {
                    from: crate::common::canonicalize("y"),
                    to: crate::common::canonicalize("x"),
                    polarity: crate::ltm::LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
        };

        let equation = generate_loop_score_equation(&loop_item);
        assert_eq!(equation, "_ltm_link_x_y * _ltm_link_y_x");
    }

    #[test]
    fn test_generate_relative_loop_score_equation() {
        use crate::ltm::LoopPolarity;

        let loops = vec![
            Loop {
                id: "R1".to_string(),
                links: vec![],
                stocks: vec![],
                polarity: LoopPolarity::Reinforcing,
            },
            Loop {
                id: "B1".to_string(),
                links: vec![],
                stocks: vec![],
                polarity: LoopPolarity::Balancing,
            },
        ];

        let equation = generate_relative_loop_score_equation("R1", &loops);

        // Should contain IF THEN ELSE for division by zero protection
        assert!(equation.contains("IF THEN ELSE"));
        // Should reference the specific loop score
        assert!(equation.contains("_ltm_loop_R1"));
        // Should have sum of all loop scores in denominator
        assert!(equation.contains("ABS(_ltm_loop_R1) + ABS(_ltm_loop_B1)"));
    }

    #[test]
    fn test_generate_ltm_variables_simple_loop() {
        // Create a simple model with a reinforcing loop
        use crate::project::Project;
        use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &[], None),
                x_flow("births", "population * birth_rate", None),
                x_aux("birth_rate", "0.02", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);

        // Generate LTM variables
        let ltm_vars = generate_ltm_variables(&project).unwrap();

        // Check that we have generated variables for the main model
        let main_ident = crate::common::canonicalize("main");
        assert!(
            ltm_vars.contains_key(&main_ident),
            "Should have variables for main model"
        );

        let vars = &ltm_vars[&main_ident];

        // We should have link score variables for:
        // - population -> births
        // - births -> population
        // And loop score variables for the loop

        // Check that we have at least some variables generated
        assert!(vars.len() > 0, "Should have generated some LTM variables");

        // Check for specific link score variables
        let has_pop_to_births = vars
            .iter()
            .any(|(name, _)| name.as_str().contains("_ltm_link_population_births"));
        let has_births_to_pop = vars
            .iter()
            .any(|(name, _)| name.as_str().contains("_ltm_link_births_population"));

        assert!(
            has_pop_to_births || has_births_to_pop,
            "Should have link score variables for the feedback loop"
        );

        // Check for loop score variable
        let has_loop_score = vars
            .iter()
            .any(|(name, _)| name.as_str().starts_with("_ltm_loop_"));
        assert!(has_loop_score, "Should have loop score variable");
    }

    #[test]
    fn test_link_score_equation_generation() {
        use crate::datamodel::Equation;
        use std::collections::HashMap;

        // Create a simple set of variables for testing
        let mut variables = HashMap::new();

        // y = x * 2 + z
        let y_var = Variable::Var {
            ident: crate::common::canonicalize("y"),
            ast: None, // Would normally have AST
            init_ast: None,
            eqn: Some(Equation::Scalar("x * 2 + z".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("y"), y_var.clone());

        let from = crate::common::canonicalize("x");
        let to = crate::common::canonicalize("y");

        let equation = generate_link_score_equation(&from, &to, &y_var, &variables);

        // The equation should contain IF THEN ELSE logic
        assert!(equation.contains("IF THEN ELSE"));
        // Should reference both x and y
        assert!(equation.contains("y"));
        assert!(equation.contains("x"));
        // Should use PREVIOUS for time-based calculations
        assert!(equation.contains("PREVIOUS"));
        // Should use SIGN for polarity
        assert!(equation.contains("SIGN"));
    }

    #[test]
    fn test_flow_to_stock_link_score() {
        use crate::datamodel::Equation;
        use std::collections::HashMap;

        // Create a stock with an inflow
        let stock_var = Variable::Stock {
            ident: crate::common::canonicalize("water_in_tank"),
            init_ast: None,
            eqn: Some(Equation::Scalar("100".to_string(), None)),
            units: None,
            inflows: vec![crate::common::canonicalize("inflow_rate")],
            outflows: vec![],
            non_negative: false,
            errors: vec![],
            unit_errors: vec![],
        };

        // Create an inflow variable
        let flow_var = Variable::Var {
            ident: crate::common::canonicalize("inflow_rate"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("10".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: true, // Mark as flow
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let mut variables = HashMap::new();
        variables.insert(
            crate::common::canonicalize("water_in_tank"),
            stock_var.clone(),
        );
        variables.insert(crate::common::canonicalize("inflow_rate"), flow_var);

        let from = crate::common::canonicalize("inflow_rate");
        let to = crate::common::canonicalize("water_in_tank");

        let equation = generate_link_score_equation(&from, &to, &stock_var, &variables);

        // Should use flow-to-stock formula
        assert!(equation.contains("IF THEN ELSE"));
        assert!(equation.contains("water_in_tank"));
        assert!(equation.contains("inflow_rate"));
        // Flow-to-stock uses second-order change (PREVIOUS(PREVIOUS()))
        assert!(equation.contains("PREVIOUS(PREVIOUS("));
        // Should not have negative sign for inflow
        assert!(!equation.contains("-((inflow_rate"));
    }

    #[test]
    fn test_outflow_to_stock_link_score() {
        use crate::datamodel::Equation;
        use std::collections::HashMap;

        // Create a stock with an outflow
        let stock_var = Variable::Stock {
            ident: crate::common::canonicalize("water_in_tank"),
            init_ast: None,
            eqn: Some(Equation::Scalar("100".to_string(), None)),
            units: None,
            inflows: vec![],
            outflows: vec![crate::common::canonicalize("outflow_rate")],
            non_negative: false,
            errors: vec![],
            unit_errors: vec![],
        };

        // Create an outflow variable
        let flow_var = Variable::Var {
            ident: crate::common::canonicalize("outflow_rate"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("5".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: true, // Mark as flow
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let mut variables = HashMap::new();
        variables.insert(
            crate::common::canonicalize("water_in_tank"),
            stock_var.clone(),
        );
        variables.insert(crate::common::canonicalize("outflow_rate"), flow_var);

        let from = crate::common::canonicalize("outflow_rate");
        let to = crate::common::canonicalize("water_in_tank");

        let equation = generate_link_score_equation(&from, &to, &stock_var, &variables);

        // Should use flow-to-stock formula
        assert!(equation.contains("IF THEN ELSE"));
        assert!(equation.contains("water_in_tank"));
        assert!(equation.contains("outflow_rate"));
        // Flow-to-stock uses second-order change
        assert!(equation.contains("PREVIOUS(PREVIOUS("));
        // Should have negative sign for outflow
        assert!(equation.contains("-((outflow_rate"));
    }

    #[test]
    fn test_relative_loop_scores_generation() {
        // Create a model with multiple loops
        use crate::project::Project;
        use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

        let model = x_model(
            "main",
            vec![
                // First loop: population -> births -> population (R)
                x_stock("population", "100", &["births"], &["deaths"], None),
                x_flow("births", "population * birth_rate", None),
                x_aux("birth_rate", "0.02", None),
                // Second loop: population -> deaths -> population (B)
                x_flow("deaths", "population * death_rate", None),
                x_aux("death_rate", "0.01", None),
                // Additional variable to create a third loop
                x_aux("growth_factor", "1 + (population - 100) / 100", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);

        // Generate LTM variables
        let ltm_vars = generate_ltm_variables(&project).unwrap();

        let main_ident = crate::common::canonicalize("main");
        assert!(ltm_vars.contains_key(&main_ident));

        let vars = &ltm_vars[&main_ident];

        // Check for relative loop score variables (only generated when multiple loops exist)
        let has_relative_scores = vars
            .iter()
            .any(|(name, _)| name.as_str().starts_with("_ltm_rel_loop_"));

        // We expect relative scores since we have multiple loops
        assert!(
            has_relative_scores,
            "Should have relative loop score variables when multiple loops exist"
        );
    }

    #[test]
    fn test_stock_to_flow_link_score() {
        use crate::datamodel::Equation;
        use std::collections::HashMap;

        // Create a stock
        let stock_var = Variable::Stock {
            ident: crate::common::canonicalize("population"),
            init_ast: None,
            eqn: Some(Equation::Scalar("1000".to_string(), None)),
            units: None,
            inflows: vec![],
            outflows: vec![crate::common::canonicalize("deaths")],
            non_negative: false,
            errors: vec![],
            unit_errors: vec![],
        };

        // Create a flow that depends on the stock (deaths = population * death_rate)
        let flow_var = Variable::Var {
            ident: crate::common::canonicalize("deaths"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar(
                "population * death_rate".to_string(),
                None,
            )),
            units: None,
            table: None,
            non_negative: false,
            is_flow: true,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let mut variables = HashMap::new();
        variables.insert(crate::common::canonicalize("population"), stock_var.clone());
        variables.insert(crate::common::canonicalize("deaths"), flow_var.clone());

        let from = crate::common::canonicalize("population");
        let to = crate::common::canonicalize("deaths");

        let equation = generate_link_score_equation(&from, &to, &flow_var, &variables);

        // Should use stock-to-flow formula
        assert!(equation.contains("IF THEN ELSE"));
        assert!(equation.contains("population"));
        assert!(equation.contains("deaths"));
        assert!(equation.contains("PREVIOUS"));
        // Check for SIGN function for polarity
        assert!(equation.contains("SIGN"));
    }
}
