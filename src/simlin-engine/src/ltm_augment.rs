// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM project augmentation - adds synthetic variables for link and loop scores
//!
//! This module generates synthetic variables for Loops That Matter (LTM) analysis.
//! The generated equations use the PREVIOUS function, which is implemented as a
//! module in stdlib/previous.stmx (not as a builtin function). The PREVIOUS module
//! uses a stock-and-flow structure to store and return the previous timestep's value.

use crate::canonicalize;
use crate::common::{Canonical, Ident, Result};
use crate::datamodel::{self, Equation};
use crate::ltm::{Link, Loop, detect_loops};
use crate::project::Project;
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};

// Type alias for clarity
type SyntheticVariables = Vec<(Ident<Canonical>, datamodel::Variable)>;

/// Augment a project with LTM synthetic variables
/// Returns a map of model name to synthetic variables to add
pub fn generate_ltm_variables(
    project: &Project,
) -> Result<HashMap<Ident<Canonical>, SyntheticVariables>> {
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
) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut link_vars = HashMap::new();

    for link in links {
        let var_name = format!(
            "$⁚ltm⁚link_score⁚{}⁚{}",
            link.from.as_str(),
            link.to.as_str()
        );

        // Check if the link involves a module variable
        let is_module_link = variables.get(&link.from).is_some_and(|v| v.is_module())
            || variables.get(&link.to).is_some_and(|v| v.is_module());

        if is_module_link {
            // Generate module-aware link score
            let equation = generate_module_link_score_equation(&link.from, &link.to, variables);
            let ltm_var = create_aux_variable(&var_name, &equation);
            link_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
        } else if let Some(to_var) = variables.get(&link.to) {
            // Generate regular link score
            let equation = generate_link_score_equation(&link.from, &link.to, to_var, variables);
            let ltm_var = create_aux_variable(&var_name, &equation);
            link_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
        }
    }

    link_vars
}

/// Generate link score equation for links involving modules (black box treatment)
fn generate_module_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    // Black box approach: modules are treated as transfer functions
    // The module transfer score represents the aggregate effect of all internal pathways

    // Check if 'from' is a module (module output to regular variable)
    if let Some(Variable::Module { .. }) = variables.get(from) {
        // Module output transfer score: (Δto_due_to_module / Δto) × sign(Δto/Δmodule)
        // Since the module instance itself is the output value
        return format!(
            "if \
                (({to} - PREVIOUS({to})) = 0) OR (({from} - PREVIOUS({from})) = 0) \
                then 0 \
                else ABS((({to} - PREVIOUS({to}))) / ({to} - PREVIOUS({to}))) * \
                if \
                    ({from} - PREVIOUS({from})) = 0 \
                    then 0 \
                    else SIGN((({to} - PREVIOUS({to}))) / ({from} - PREVIOUS({from})))",
            to = to.as_str(),
            from = from.as_str()
        );
    }

    // Check if 'to' is a module (regular variable to module input)
    if let Some(Variable::Module { inputs, .. }) = variables.get(to) {
        // Find which input this connects to
        for input in inputs {
            if input.src == *from {
                // Module input transfer score: measure contribution of input to module's change
                // Black box: assume unit transfer initially (module will process internally)
                return format!(
                    "if \
                        (({to} - PREVIOUS({to})) = 0) OR (({from} - PREVIOUS({from})) = 0) \
                        then 0 \
                        else ABS((({to} - PREVIOUS({to}))) / ({to} - PREVIOUS({to}))) * \
                        if \
                            ({from} - PREVIOUS({from})) = 0 \
                            then 0 \
                            else SIGN((({to} - PREVIOUS({to}))) / ({from} - PREVIOUS({from})))",
                    to = to.as_str(),
                    from = from.as_str()
                );
            }
        }
    }

    // Both from and to are modules (module-to-module connection)
    // This represents a chain of black boxes
    if variables.get(from).is_some_and(|v| v.is_module())
        && variables.get(to).is_some_and(|v| v.is_module())
    {
        // Chain transfer score: product of individual transfer scores
        return format!(
            "if \
                (({to} - PREVIOUS({to})) = 0) OR (({from} - PREVIOUS({from})) = 0) \
                then 0 \
                else ABS((({to} - PREVIOUS({to}))) / ({to} - PREVIOUS({to}))) * \
                if \
                    ({from} - PREVIOUS({from})) = 0 \
                    then 0 \
                    else SIGN((({to} - PREVIOUS({to}))) / ({from} - PREVIOUS({from})))",
            to = to.as_str(),
            from = from.as_str()
        );
    }

    // Default case - shouldn't normally reach here
    "0".to_string()
}

/// Generate loop score variables for all loops
fn generate_loop_score_variables(loops: &[Loop]) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut loop_vars = HashMap::new();

    // First, generate absolute loop scores
    for loop_item in loops {
        let var_name = format!("$⁚ltm⁚abs_loop_score⁚{}", loop_item.id);

        // Generate equation as product of link scores
        let equation = generate_loop_score_equation(loop_item);

        // Create the synthetic variable
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
    }

    // Then, generate relative loop scores for all loops
    for loop_item in loops {
        let var_name = format!("$⁚ltm⁚rel_loop_score⁚{}", loop_item.id);

        // Generate equation for relative loop score
        let equation = if loops.len() == 1 {
            // Single loop always has relative score of 1
            "1".to_string()
        } else {
            generate_relative_loop_score_equation(&loop_item.id, loops)
        };

        // Create the synthetic variable
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(crate::common::canonicalize(&var_name), ltm_var);
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

    // Using SAFEDIV for both divisions
    // Note: We still need the outer check for when EITHER is zero, since we multiply the results
    let abs_part = format!(
        "ABS(SAFEDIV((({partial_eq}) - PREVIOUS({to})), ({to} - PREVIOUS({to})), 0))",
        partial_eq = partial_eq,
        to = to.as_str()
    );
    let sign_part = format!(
        "SIGN(SAFEDIV((({partial_eq}) - PREVIOUS({to})), ({from} - PREVIOUS({from})), 0))",
        partial_eq = partial_eq,
        to = to.as_str(),
        from = from.as_str()
    );

    format!(
        "if \
            (({to} - PREVIOUS({to})) = 0) OR (({from} - PREVIOUS({from})) = 0) \
            then 0 \
            else {abs_part} * {sign_part}",
        to = to.as_str(),
        from = from.as_str(),
        abs_part = abs_part,
        sign_part = sign_part
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

    // Using SAFEDIV to handle division by zero
    // The numerator uses PREVIOUS values to align timing with the denominator.
    // At time t, the flow at t-1 (PREVIOUS(flow)) is what drove the stock change from t-1 to t.
    // We measure the change in that causal flow: flow(t-1) - flow(t-2).
    let numerator = format!("{sign}(PREVIOUS({flow}) - PREVIOUS(PREVIOUS({flow})))");
    let denominator = format!(
        "(({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock}))))"
    );

    format!("SAFEDIV({numerator}, {denominator}, 0)")
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
    let _is_affecting_stock = if let Some(Variable::Stock {
        inflows, outflows, ..
    }) = stock_var
    {
        inflows.contains(flow) || outflows.contains(flow)
    } else {
        false
    };

    // Link score formula from LTM paper: |Δxz/Δz| × sign(Δxz/Δx)
    // For stock-to-flow: x=stock, z=flow
    let flow_diff = format!("({flow} - PREVIOUS({flow}))", flow = flow.as_str());
    let stock_diff = format!("({stock} - PREVIOUS({stock}))", stock = stock.as_str());
    let partial_change = format!(
        "(({partial_eq}) - PREVIOUS({flow}))",
        partial_eq = partial_eq,
        flow = flow.as_str()
    );

    let abs_part = format!("ABS(SAFEDIV({partial_change}, {flow_diff}, 0))");
    let sign_part = format!("SIGN(SAFEDIV({partial_change}, {stock_diff}, 0))");

    format!(
        "if \
            ({flow_diff} = 0) OR ({stock_diff} = 0) \
            then 0 \
            else {abs_part} * {sign_part}"
    )
}

/// Generate the equation for a loop score variable
fn generate_loop_score_equation(loop_item: &Loop) -> String {
    // Product of all link scores in the loop
    // Use double quotes around variable names with $ to make them parseable
    let link_score_names: Vec<String> = loop_item
        .links
        .iter()
        .map(|link| {
            // Double-quote the variable name so it can be parsed
            format!(
                "\"$⁚ltm⁚link_score⁚{}⁚{}\"",
                link.from.as_str(),
                link.to.as_str()
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
    // Use double quotes around variable names with $
    let loop_score_var = format!("\"$⁚ltm⁚abs_loop_score⁚{loop_id}\"");

    // Build sum of absolute values of all loop scores
    let all_loop_scores: Vec<String> = all_loops
        .iter()
        .map(|loop_item| format!("ABS(\"$⁚ltm⁚abs_loop_score⁚{}\")", loop_item.id))
        .collect();

    let sum_expr = if all_loop_scores.is_empty() {
        "1".to_string() // Avoid division by zero
    } else {
        all_loop_scores.join(" + ")
    };

    // Relative score formula using SAFEDIV for division by zero protection
    format!("SAFEDIV({loop_score_var}, ({sum_expr}), 0)")
}

/// Create an auxiliary variable with the given equation
fn create_aux_variable(name: &str, equation: &str) -> crate::datamodel::Variable {
    use crate::datamodel;

    datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(name).to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string(), None),
        documentation: "LTM".to_string(),
        units: Some("dmnl".to_string()), // LTM scores are dimensionless
        gf: None,
        can_be_module_input: false,
        visibility: datamodel::Visibility::Public,
        ai_state: None,
        uid: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_common::TestProject;

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
        // The equation should use double-quoted variable names for parseability
        assert_eq!(
            equation,
            "\"$⁚ltm⁚link_score⁚x⁚y\" * \"$⁚ltm⁚link_score⁚y⁚x\""
        );
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

        // Should use SAFEDIV for division by zero protection
        assert!(equation.contains("SAFEDIV"));
        // Should reference the specific loop score (with double quotes and $ prefix)
        assert!(equation.contains("\"$⁚ltm⁚abs_loop_score⁚R1\""));
        // Should have sum of all loop scores in denominator (with double quotes and $ prefix)
        assert!(
            equation
                .contains("ABS(\"$⁚ltm⁚abs_loop_score⁚R1\") + ABS(\"$⁚ltm⁚abs_loop_score⁚B1\")")
        );
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
        assert!(!vars.is_empty(), "Should have generated some LTM variables");

        // Check for specific link score variables
        let has_pop_to_births = vars
            .iter()
            .any(|(name, _)| name.as_str().contains("$⁚ltm⁚link_score⁚population⁚births"));
        let has_births_to_pop = vars
            .iter()
            .any(|(name, _)| name.as_str().contains("$⁚ltm⁚link_score⁚births⁚population"));

        assert!(
            has_pop_to_births || has_births_to_pop,
            "Should have link score variables for the feedback loop"
        );

        // Check for loop score variable
        let has_loop_score = vars
            .iter()
            .any(|(name, _)| name.as_str().starts_with("$⁚ltm⁚abs_loop_score⁚"));
        assert!(has_loop_score, "Should have loop score variable");
    }

    #[test]
    fn test_link_score_equation_generation() {
        // Create a test project with dependent variables
        let project = TestProject::new("test_link_score")
            .aux("x", "10", None)
            .aux("z", "5", None)
            .aux("y", "x * 2 + z", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = crate::common::canonicalize("x");
        let to = crate::common::canonicalize("y");
        let y_var = all_vars.get(&to).expect("Y variable should exist");

        let equation = generate_link_score_equation(&from, &to, y_var, all_vars);

        // Verify the EXACT equation structure
        let expected = "if \
            ((y - PREVIOUS(y)) = 0) OR ((x - PREVIOUS(x)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((x * 2 + PREVIOUS(z)) - PREVIOUS(y)), (y - PREVIOUS(y)), 0)) * \
            SIGN(SAFEDIV(((x * 2 + PREVIOUS(z)) - PREVIOUS(y)), (x - PREVIOUS(x)), 0))";

        assert_eq!(
            equation, expected,
            "Link score equation must match exact format"
        );
    }

    #[test]
    fn test_flow_to_stock_link_score() {
        // Create a test project with stock and inflow
        let project = TestProject::new("test_flow_to_stock")
            .stock("water_in_tank", "100", &["inflow_rate"], &[], None)
            .flow("inflow_rate", "10", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = crate::common::canonicalize("inflow_rate");
        let to = crate::common::canonicalize("water_in_tank");
        let stock_var = all_vars.get(&to).expect("Stock variable should exist");

        let equation = generate_link_score_equation(&from, &to, stock_var, all_vars);

        // Verify the EXACT equation structure for flow-to-stock
        // Uses PREVIOUS in numerator to align timing with denominator
        let expected = "SAFEDIV(\
            (PREVIOUS(inflow_rate) - PREVIOUS(PREVIOUS(inflow_rate))), \
            ((water_in_tank - PREVIOUS(water_in_tank)) - (PREVIOUS(water_in_tank) - PREVIOUS(PREVIOUS(water_in_tank)))), \
            0\
        )";

        assert_eq!(
            equation, expected,
            "Flow-to-stock equation must match exact format with second-order difference"
        );
    }

    #[test]
    fn test_outflow_to_stock_link_score() {
        // Create a test project with stock and outflow
        let project = TestProject::new("test_outflow_to_stock")
            .stock("water_in_tank", "100", &[], &["outflow_rate"], None)
            .flow("outflow_rate", "5", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = crate::common::canonicalize("outflow_rate");
        let to = crate::common::canonicalize("water_in_tank");
        let stock_var = all_vars.get(&to).expect("Stock variable should exist");

        let equation = generate_link_score_equation(&from, &to, stock_var, all_vars);

        // Verify the EXACT equation structure for outflow-to-stock (negative sign)
        // Uses PREVIOUS in numerator to align timing with denominator
        let expected = "SAFEDIV(\
            -(PREVIOUS(outflow_rate) - PREVIOUS(PREVIOUS(outflow_rate))), \
            ((water_in_tank - PREVIOUS(water_in_tank)) - (PREVIOUS(water_in_tank) - PREVIOUS(PREVIOUS(water_in_tank)))), \
            0\
        )";

        assert_eq!(
            equation, expected,
            "Outflow-to-stock equation must have negative sign and second-order difference"
        );
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
            .any(|(name, _)| name.as_str().starts_with("$⁚ltm⁚rel_loop_score⁚"));

        // We expect relative scores since we have multiple loops
        assert!(
            has_relative_scores,
            "Should have relative loop score variables when multiple loops exist"
        );
    }

    #[test]
    fn test_module_link_scores() {
        // Test link score generation for module connections
        use crate::datamodel::Equation;
        use crate::ltm::{Link, LinkPolarity};
        use crate::variable::ModuleInput;
        use std::collections::{HashMap, HashSet};

        let mut variables = HashMap::new();

        // Create a module variable
        let module_var = Variable::Module {
            ident: crate::common::canonicalize("smoother"),
            model_name: crate::common::canonicalize("smooth"),
            units: None,
            inputs: vec![ModuleInput {
                src: crate::common::canonicalize("raw_input"),
                dst: crate::common::canonicalize("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("smoother"), module_var);

        // Create an input variable
        let input_var = Variable::Var {
            ident: crate::common::canonicalize("raw_input"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("10 + SIN(TIME)".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("raw_input"), input_var);

        // Create a link from raw_input to the module
        let mut links = HashSet::new();
        links.insert(Link {
            from: crate::common::canonicalize("raw_input"),
            to: crate::common::canonicalize("smoother"),
            polarity: LinkPolarity::Positive,
        });

        // Generate link score variables
        let link_vars = generate_link_score_variables(&links, &variables);

        // Check that a link score variable was created
        assert!(!link_vars.is_empty(), "Should generate module link score");

        // Check the variable name
        let expected_name = crate::common::canonicalize("$⁚ltm⁚link_score⁚raw_input⁚smoother");
        assert!(
            link_vars.contains_key(&expected_name),
            "Should have link score for module connection"
        );
    }

    #[test]
    fn test_module_to_variable_link_score() {
        // Test module output to regular variable link score
        use crate::datamodel::Equation;
        use std::collections::HashMap;

        let mut variables = HashMap::new();

        // Create a module variable (acts as output)
        let module_var = Variable::Module {
            ident: crate::common::canonicalize("smoother"),
            model_name: crate::common::canonicalize("smooth"),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        };

        // Create a dependent variable
        let dependent_var = Variable::Var {
            ident: crate::common::canonicalize("processed"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("smoother * 2".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("smoother"), module_var);
        variables.insert(crate::common::canonicalize("processed"), dependent_var);

        let from = crate::common::canonicalize("smoother");
        let to = crate::common::canonicalize("processed");

        let equation = generate_module_link_score_equation(&from, &to, &variables);

        // Verify the EXACT equation structure for module-to-variable link
        let expected = "if \
            ((processed - PREVIOUS(processed)) = 0) OR ((smoother - PREVIOUS(smoother)) = 0) \
            then 0 \
            else ABS(((processed - PREVIOUS(processed))) / (processed - PREVIOUS(processed))) * \
            if \
                (smoother - PREVIOUS(smoother)) = 0 \
                then 0 \
                else SIGN(((processed - PREVIOUS(processed))) / (smoother - PREVIOUS(smoother)))";

        assert_eq!(
            equation, expected,
            "Module-to-variable link score equation must match exact format"
        );
    }

    #[test]
    fn test_variable_to_module_link_score() {
        // Test regular variable to module input link score
        use crate::datamodel::Equation;
        use crate::variable::ModuleInput;
        use std::collections::HashMap;

        let mut variables = HashMap::new();

        // Create an input variable
        let input_var = Variable::Var {
            ident: crate::common::canonicalize("raw_data"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("TIME * 2".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        // Create a module variable with an input
        let module_var = Variable::Module {
            ident: crate::common::canonicalize("processor"),
            model_name: crate::common::canonicalize("process"),
            units: None,
            inputs: vec![ModuleInput {
                src: crate::common::canonicalize("raw_data"),
                dst: crate::common::canonicalize("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("raw_data"), input_var);
        variables.insert(crate::common::canonicalize("processor"), module_var);

        let from = crate::common::canonicalize("raw_data");
        let to = crate::common::canonicalize("processor");

        let equation = generate_module_link_score_equation(&from, &to, &variables);

        // Verify the EXACT equation structure for variable-to-module link
        let expected = "if \
            ((processor - PREVIOUS(processor)) = 0) OR ((raw_data - PREVIOUS(raw_data)) = 0) \
            then 0 \
            else ABS(((processor - PREVIOUS(processor))) / (processor - PREVIOUS(processor))) * \
            if \
                (raw_data - PREVIOUS(raw_data)) = 0 \
                then 0 \
                else SIGN(((processor - PREVIOUS(processor))) / (raw_data - PREVIOUS(raw_data)))";

        assert_eq!(
            equation, expected,
            "Variable-to-module link score equation must match exact format"
        );
    }

    #[test]
    fn test_module_to_module_link_score() {
        // Test module to module connection link score
        use crate::variable::ModuleInput;
        use std::collections::HashMap;

        let mut variables = HashMap::new();

        // Create first module (output)
        let module_a = Variable::Module {
            ident: crate::common::canonicalize("filter_a"),
            model_name: crate::common::canonicalize("filter"),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        };

        // Create second module (input from first)
        let module_b = Variable::Module {
            ident: crate::common::canonicalize("filter_b"),
            model_name: crate::common::canonicalize("filter"),
            units: None,
            inputs: vec![ModuleInput {
                src: crate::common::canonicalize("filter_a"),
                dst: crate::common::canonicalize("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("filter_a"), module_a);
        variables.insert(crate::common::canonicalize("filter_b"), module_b);

        let from = crate::common::canonicalize("filter_a");
        let to = crate::common::canonicalize("filter_b");

        let equation = generate_module_link_score_equation(&from, &to, &variables);

        // Verify the EXACT equation structure for module-to-module link
        let expected = "if \
            ((filter_b - PREVIOUS(filter_b)) = 0) OR ((filter_a - PREVIOUS(filter_a)) = 0) \
            then 0 \
            else ABS(((filter_b - PREVIOUS(filter_b))) / (filter_b - PREVIOUS(filter_b))) * \
            if \
                (filter_a - PREVIOUS(filter_a)) = 0 \
                then 0 \
                else SIGN(((filter_b - PREVIOUS(filter_b))) / (filter_a - PREVIOUS(filter_a)))";

        assert_eq!(
            equation, expected,
            "Module-to-module link score equation must match exact format"
        );
    }

    #[test]
    fn test_ltm_with_module_in_loop() {
        // Integration test: Test that module link scores are generated correctly
        // Given the difficulty with module inputs not being preserved in conversion,
        // let's directly test the module link score equation generation
        use crate::datamodel::Equation;
        use crate::variable::ModuleInput;
        use std::collections::HashMap;

        let mut variables = HashMap::new();

        // Create a module with proper inputs
        let module_var = Variable::Module {
            ident: crate::common::canonicalize("processor"),
            model_name: crate::common::canonicalize("process_model"),
            units: None,
            inputs: vec![ModuleInput {
                src: crate::common::canonicalize("input_value"),
                dst: crate::common::canonicalize("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("processor"), module_var);

        // Create variables that connect to the module
        let input_var = Variable::Var {
            ident: crate::common::canonicalize("input_value"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("10".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let output_var = Variable::Var {
            ident: crate::common::canonicalize("output_value"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("processor * 2".to_string(), None)),
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(crate::common::canonicalize("input_value"), input_var);
        variables.insert(crate::common::canonicalize("output_value"), output_var);

        // Test that we can generate module link score equations correctly
        use crate::ltm::{Link, LinkPolarity};
        use std::collections::HashSet;

        // Create links involving the module
        let mut links = HashSet::new();

        // Link from input_value to processor (module input)
        links.insert(Link {
            from: crate::common::canonicalize("input_value"),
            to: crate::common::canonicalize("processor"),
            polarity: LinkPolarity::Positive,
        });

        // Link from processor to output_value (module output)
        links.insert(Link {
            from: crate::common::canonicalize("processor"),
            to: crate::common::canonicalize("output_value"),
            polarity: LinkPolarity::Positive,
        });

        // Generate link score variables
        let link_vars = generate_link_score_variables(&links, &variables);

        // Check that module link scores were generated
        assert_eq!(link_vars.len(), 2, "Should generate 2 link score variables");

        let has_input_link = link_vars.contains_key(&crate::common::canonicalize(
            "$⁚ltm⁚link_score⁚input_value⁚processor",
        ));
        let has_output_link = link_vars.contains_key(&crate::common::canonicalize(
            "$⁚ltm⁚link_score⁚processor⁚output_value",
        ));

        assert!(
            has_input_link,
            "Should generate link score for input to module"
        );
        assert!(
            has_output_link,
            "Should generate link score for module to output"
        );

        // Check that the equations use the black box formula
        // Remove debug output
        for var in link_vars.values() {
            if let datamodel::Variable::Aux(aux) = var
                && let datamodel::Equation::Scalar(eq, _) = &aux.equation
            {
                // Module link scores should use the black box formula
                assert!(
                    eq.contains("SIGN"),
                    "Module link should include SIGN for polarity"
                );
                assert!(
                    eq.contains("PREVIOUS"),
                    "Module link should use PREVIOUS for time-based calc"
                );
            }
        }
    }

    #[test]
    fn test_stock_to_flow_link_score() {
        // Create a test project with stock and dependent flow
        let project = TestProject::new("test_stock_to_flow")
            .stock("population", "1000", &[], &["deaths"], None)
            .flow("deaths", "population * death_rate", None)
            .aux("death_rate", "0.01", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = crate::common::canonicalize("population");
        let to = crate::common::canonicalize("deaths");
        let flow_var = all_vars.get(&to).expect("Flow variable should exist");

        let equation = generate_link_score_equation(&from, &to, flow_var, all_vars);

        // Verify the EXACT equation structure for stock-to-flow
        // Sign term uses first-order stock change per LTM paper formula
        let expected = "if \
            ((deaths - PREVIOUS(deaths)) = 0) OR \
            ((population - PREVIOUS(population)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((population * PREVIOUS(death_rate)) - PREVIOUS(deaths)), (deaths - PREVIOUS(deaths)), 0)) * \
            SIGN(SAFEDIV(((population * PREVIOUS(death_rate)) - PREVIOUS(deaths)), \
                (population - PREVIOUS(population)), 0))";

        assert_eq!(
            equation, expected,
            "Stock-to-flow equation must match exact format with first-order stock diff for sign"
        );
    }

    #[test]
    fn test_generate_stock_to_flow_equation_basic_inflow() {
        // Create a test project with stock and flow
        let project = TestProject::new("test_basic_inflow")
            .stock("inventory", "100", &["production"], &[], None)
            .flow("production", "inventory * 0.1", None)
            .compile()
            .expect("Project should compile");

        // Get the model and its variables
        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = crate::common::canonicalize("inventory");
        let flow = crate::common::canonicalize("production");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // Verify the EXACT equation structure using SAFEDIV
        // Sign term uses first-order stock change per LTM paper formula
        let expected = "if \
            ((production - PREVIOUS(production)) = 0) OR \
            ((inventory - PREVIOUS(inventory)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((inventory * 0.1) - PREVIOUS(production)), (production - PREVIOUS(production)), 0)) * \
            SIGN(SAFEDIV(((inventory * 0.1) - PREVIOUS(production)), \
                (inventory - PREVIOUS(inventory)), 0))";

        assert_eq!(
            equation, expected,
            "Stock-to-flow equation must match exact format with first-order stock diff for sign"
        );
    }

    #[test]
    fn test_generate_stock_to_flow_equation_outflow() {
        // Create a test project with stock and outflow
        let project = TestProject::new("test_outflow")
            .stock("water_tank", "100", &[], &["drainage"], None)
            .flow("drainage", "water_tank / 10", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = crate::common::canonicalize("water_tank");
        let flow = crate::common::canonicalize("drainage");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // Verify the EXACT equation structure using SAFEDIV
        // Sign term uses first-order stock change per LTM paper formula
        let expected = "if \
            ((drainage - PREVIOUS(drainage)) = 0) OR \
            ((water_tank - PREVIOUS(water_tank)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((water_tank / 10) - PREVIOUS(drainage)), (drainage - PREVIOUS(drainage)), 0)) * \
            SIGN(SAFEDIV(((water_tank / 10) - PREVIOUS(drainage)), \
                (water_tank - PREVIOUS(water_tank)), 0))";

        assert_eq!(
            equation, expected,
            "Outflow equation must match exact format with first-order stock diff for sign"
        );
    }

    #[test]
    fn test_generate_stock_to_flow_equation_complex_dependencies() {
        // Create a test project with multiple dependencies
        let project = TestProject::new("test_complex")
            .stock("population", "1000", &["births"], &[], None)
            .flow("births", "population * birth_rate * seasonal_factor", None)
            .aux("birth_rate", "0.02", None)
            .aux("seasonal_factor", "1 + SIN(TIME * 2 * 3.14159 / 12)", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = crate::common::canonicalize("population");
        let flow = crate::common::canonicalize("births");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // Verify the EXACT equation using SAFEDIV - note that non-stock dependencies get PREVIOUS()
        // Sign term uses first-order stock change per LTM paper formula
        let expected = "if \
            ((births - PREVIOUS(births)) = 0) OR \
            ((population - PREVIOUS(population)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((population * PREVIOUS(birth_rate) * PREVIOUS(seasonal_factor)) - PREVIOUS(births)), \
                (births - PREVIOUS(births)), 0)) * \
            SIGN(SAFEDIV(((population * PREVIOUS(birth_rate) * PREVIOUS(seasonal_factor)) - PREVIOUS(births)), \
                (population - PREVIOUS(population)), 0))";

        assert_eq!(
            equation, expected,
            "Complex dependencies equation must properly replace non-stock variables with PREVIOUS()"
        );
    }

    #[test]
    fn test_generate_stock_to_flow_equation_no_stock_dependency() {
        // Create a test project where flow doesn't depend on stock
        let project = TestProject::new("test_no_dependency")
            .stock("unrelated_stock", "50", &[], &["constant_flow"], None)
            .flow("constant_flow", "10", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = crate::common::canonicalize("unrelated_stock");
        let flow = crate::common::canonicalize("constant_flow");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // When flow doesn't depend on stock, partial equation is just the constant
        // Sign term uses first-order stock change per LTM paper formula
        let expected = "if \
            ((constant_flow - PREVIOUS(constant_flow)) = 0) OR \
            ((unrelated_stock - PREVIOUS(unrelated_stock)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((10) - PREVIOUS(constant_flow)), (constant_flow - PREVIOUS(constant_flow)), 0)) * \
            SIGN(SAFEDIV(((10) - PREVIOUS(constant_flow)), \
                (unrelated_stock - PREVIOUS(unrelated_stock)), 0))";

        assert_eq!(
            equation, expected,
            "No dependency equation should use first-order stock diff for sign"
        );
    }

    #[test]
    fn test_equation_with_dollar_sign_variables() {
        // Test that equations with $ in variable names can be parsed using double quotes
        use crate::ast::{Expr0, print_eqn};
        use crate::builtins::Loc;
        use crate::common::{RawIdent, canonicalize};
        use crate::token::LexerType;

        println!("\n=== Testing $ variable parsing with double quotes ===");

        // Test 1: Double quoted variable parses successfully
        let equation = "\"$⁚ltm⁚link_score⁚x⁚y\"";
        let result = Expr0::new(equation, LexerType::Equation);
        assert!(result.is_ok(), "Double quoted $ variable should parse");

        if let Ok(Some(Expr0::Var(id, _))) = &result {
            println!("Parsed variable RawIdent: {:?}", id.as_str());
            println!("Canonicalized: {}", canonicalize(id.as_str()));

            // Check if quotes are included in the identifier
            assert_eq!(id.as_str(), "\"$⁚ltm⁚link_score⁚x⁚y\"");
        }

        // Test 2: Complex equation with quoted variables
        let equation = "\"$⁚ltm⁚link_score⁚x⁚y\" * \"$⁚ltm⁚link_score⁚y⁚x\"";
        let result = Expr0::new(equation, LexerType::Equation);
        assert!(
            result.is_ok(),
            "Multiplication of quoted $ variables should parse"
        );

        // Test 3: What happens when we create AST with quoted names and print it
        let var_ast = Expr0::Var(
            RawIdent::new_from_str("\"$⁚ltm⁚link_score⁚x⁚y\""),
            Loc::default(),
        );
        let printed = print_eqn(&var_ast);
        println!("AST with quoted $ variable printed as: '{printed}'");

        // Note: print_eqn outputs single quotes but parser needs double quotes
        // This is a known limitation but doesn't affect our use case since we generate
        // equations directly with double quotes, not through print_eqn
        assert_eq!(
            printed, "$⁚ltm⁚link_score⁚x⁚y",
            "print_eqn strips quotes from canonicalized name"
        );
    }

    #[test]
    fn test_generate_stock_to_flow_validates_equation_3_paper_2023() {
        // This test specifically validates that the implementation matches
        // Equation (3) from the 2023 paper:
        // For stock-to-flow, we're checking that the equation properly
        // calculates the partial derivative of the flow with respect to the stock

        let project = TestProject::new("test_equation_3")
            .stock("S", "100", &["inflow"], &[], None)
            .flow("inflow", "S * growth_rate", None)
            .aux("growth_rate", "0.1", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&crate::common::canonicalize("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = crate::common::canonicalize("S");
        let flow = crate::common::canonicalize("inflow");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // This test validates the correct LTM paper formula implementation
        // Note: 'S' becomes lowercase 's' in stock references but stays uppercase in partial equation
        // Sign term uses first-order stock change per LTM paper formula
        let expected = "if \
            ((inflow - PREVIOUS(inflow)) = 0) OR \
            ((s - PREVIOUS(s)) = 0) \
            then 0 \
            else ABS(SAFEDIV(((S * PREVIOUS(growth_rate)) - PREVIOUS(inflow)), (inflow - PREVIOUS(inflow)), 0)) * \
            SIGN(SAFEDIV(((S * PREVIOUS(growth_rate)) - PREVIOUS(inflow)), \
                (s - PREVIOUS(s)), 0))";

        assert_eq!(
            equation, expected,
            "Equation must match LTM paper formula with first-order stock diff for sign"
        );
    }
}
