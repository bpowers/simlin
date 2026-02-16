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
use crate::ltm::{CausalGraph, Link, Loop, detect_loops};
use crate::project::Project;
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};
use unicode_xid::UnicodeXID;

// Type alias for clarity
type SyntheticVariables = Vec<(Ident<Canonical>, datamodel::Variable)>;

/// Replace whole-word occurrences of `pattern` with `replacement` in `text`.
/// A word boundary is defined as the position where a Unicode identifier character
/// (XID_Continue or underscore) meets a non-identifier character (or start/end of string).
fn replace_whole_word(text: &str, pattern: &str, replacement: &str) -> String {
    if pattern.is_empty() {
        return text.to_string();
    }

    let mut result = String::with_capacity(text.len());
    let mut remaining = text;
    // Track the last character we processed to maintain boundary context across iterations
    let mut prev_char: Option<char> = None;

    while let Some(pos) = remaining.find(pattern) {
        // Check if this is a word boundary match
        let before_ok = if pos == 0 {
            // At start of remaining slice - use tracked prev_char for context
            prev_char.is_none_or(|c| !is_word_char(c))
        } else {
            let prev_c = remaining[..pos].chars().last().unwrap();
            !is_word_char(prev_c)
        };

        let after_pos = pos + pattern.len();
        let after_ok = if after_pos >= remaining.len() {
            true
        } else {
            let next_char = remaining[after_pos..].chars().next().unwrap();
            !is_word_char(next_char)
        };

        if before_ok && after_ok {
            // This is a whole-word match, replace it
            result.push_str(&remaining[..pos]);
            result.push_str(replacement);
            // Update prev_char to the last char of replacement
            prev_char = replacement.chars().last();
            remaining = &remaining[after_pos..];
        } else {
            // Not a whole-word match. Advance past the first character of where
            // we found the pattern to continue searching with proper context.
            let char_at_pos = remaining[pos..].chars().next().unwrap();
            let char_len = char_at_pos.len_utf8();
            result.push_str(&remaining[..pos + char_len]);
            prev_char = Some(char_at_pos);
            remaining = &remaining[pos + char_len..];
        }
    }

    result.push_str(remaining);
    result
}

/// Check if a character is a word character for identifier boundaries.
/// Uses Unicode XID_Continue to match Simlin's identifier rules, plus underscore.
fn is_word_char(c: char) -> bool {
    UnicodeXID::is_xid_continue(c) || c == '_'
}

/// Augment a project with LTM synthetic variables
/// Returns a map of model name to synthetic variables to add
pub fn generate_ltm_variables(
    project: &Project,
) -> Result<HashMap<Ident<Canonical>, SyntheticVariables>> {
    generate_ltm_variables_inner(project, false)
}

/// Augment a project with link score variables for ALL causal links (discovery mode).
///
/// Unlike `generate_ltm_variables()` which only generates variables for links
/// participating in detected loops, this generates link score variables for every
/// causal connection in the model. Loop score and relative loop score variables
/// are NOT generated -- those are computed post-simulation by `discover_loops()`.
pub fn generate_ltm_variables_all_links(
    project: &Project,
) -> Result<HashMap<Ident<Canonical>, SyntheticVariables>> {
    generate_ltm_variables_inner(project, true)
}

fn generate_ltm_variables_inner(
    project: &Project,
    all_links_mode: bool,
) -> Result<HashMap<Ident<Canonical>, SyntheticVariables>> {
    let mut result = HashMap::new();

    if all_links_mode {
        // Discovery mode: generate link score variables for ALL causal links
        for (model_name, model) in &project.models {
            if model.implicit {
                continue;
            }

            let graph = CausalGraph::from_model(model, project)?;
            let links: HashSet<Link> = graph.all_links().into_iter().collect();

            let link_score_vars = generate_link_score_variables(&links, &model.variables);

            let synthetic_vars: Vec<_> = link_score_vars.into_iter().collect();
            if !synthetic_vars.is_empty() {
                result.insert(model_name.clone(), synthetic_vars);
            }
        }
    } else {
        // Existing behavior: detect loops, generate for loop links only
        let loops = detect_loops(project)?;

        for (model_name, model_loops) in &loops {
            if let Some(model) = project.models.get(model_name) {
                if model.implicit {
                    continue;
                }

                let mut synthetic_vars = Vec::new();

                let mut loop_links = HashSet::new();
                for loop_item in model_loops {
                    for link in &loop_item.links {
                        loop_links.insert(link.clone());
                    }
                }

                let link_score_vars = generate_link_score_variables(&loop_links, &model.variables);
                let loop_score_vars = generate_loop_score_variables(model_loops);

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
            link_vars.insert(Ident::new(&var_name), ltm_var);
        } else if let Some(to_var) = variables.get(&link.to) {
            // Generate regular link score
            let equation = generate_link_score_equation(&link.from, &link.to, to_var, variables);
            let ltm_var = create_aux_variable(&var_name, &equation);
            link_vars.insert(Ident::new(&var_name), ltm_var);
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
                (TIME = PREVIOUS(TIME)) \
                then 0/0 \
                else if \
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
                        (TIME = PREVIOUS(TIME)) \
                        then 0/0 \
                        else if \
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
                (TIME = PREVIOUS(TIME)) \
                then 0/0 \
                else if \
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
        loop_vars.insert(Ident::new(&var_name), ltm_var);
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
        loop_vars.insert(Ident::new(&var_name), ltm_var);
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
            let replacement = format!("PREVIOUS({})", dep.as_str());
            partial_eq = replace_whole_word(&partial_eq, dep.as_str(), &replacement);
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

    // Return NaN at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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

    // Return NaN for the first two timesteps when we don't have enough history for second-order differences
    format!(
        "if \
            (TIME = PREVIOUS(TIME)) OR (PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))) \
            then 0/0 \
            else SAFEDIV({numerator}, {denominator}, 0)"
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
            let replacement = format!("PREVIOUS({})", dep.as_str());
            partial_eq = replace_whole_word(&partial_eq, dep.as_str(), &replacement);
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

    // Return NaN at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
        ident: canonicalize(name).into_owned(),
        equation: datamodel::Equation::Scalar(equation.to_string(), None),
        documentation: "LTM".to_string(),
        units: None, // LTM scores are dimensionless by design, no need to declare
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
                    from: Ident::new("x"),
                    to: Ident::new("y"),
                    polarity: crate::ltm::LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("y"),
                    to: Ident::new("x"),
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
        let main_ident = Ident::new("main");
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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = Ident::new("x");
        let to = Ident::new("y");
        let y_var = all_vars.get(&to).expect("Y variable should exist");

        let equation = generate_link_score_equation(&from, &to, y_var, all_vars);

        // Verify the EXACT equation structure
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = Ident::new("inflow_rate");
        let to = Ident::new("water_in_tank");
        let stock_var = all_vars.get(&to).expect("Stock variable should exist");

        let equation = generate_link_score_equation(&from, &to, stock_var, all_vars);

        // Verify the EXACT equation structure for flow-to-stock
        // Uses PREVIOUS in numerator to align timing with denominator
        // Returns NaN for first two timesteps when insufficient history
        let expected = "if \
            (TIME = PREVIOUS(TIME)) OR (PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))) \
            then 0/0 \
            else SAFEDIV(\
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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = Ident::new("outflow_rate");
        let to = Ident::new("water_in_tank");
        let stock_var = all_vars.get(&to).expect("Stock variable should exist");

        let equation = generate_link_score_equation(&from, &to, stock_var, all_vars);

        // Verify the EXACT equation structure for outflow-to-stock (negative sign)
        // Uses PREVIOUS in numerator to align timing with denominator
        // Returns NaN for first two timesteps when insufficient history
        let expected = "if \
            (TIME = PREVIOUS(TIME)) OR (PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))) \
            then 0/0 \
            else SAFEDIV(\
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

        let main_ident = Ident::new("main");
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
            ident: Ident::new("smoother"),
            model_name: Ident::new("smooth"),
            units: None,
            inputs: vec![ModuleInput {
                src: Ident::new("raw_input"),
                dst: Ident::new("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("smoother"), module_var);

        // Create an input variable
        let input_var = Variable::Var {
            ident: Ident::new("raw_input"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("10 + SIN(TIME)".to_string(), None)),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("raw_input"), input_var);

        // Create a link from raw_input to the module
        let mut links = HashSet::new();
        links.insert(Link {
            from: Ident::new("raw_input"),
            to: Ident::new("smoother"),
            polarity: LinkPolarity::Positive,
        });

        // Generate link score variables
        let link_vars = generate_link_score_variables(&links, &variables);

        // Check that a link score variable was created
        assert!(!link_vars.is_empty(), "Should generate module link score");

        // Check the variable name
        let expected_name = Ident::new("$⁚ltm⁚link_score⁚raw_input⁚smoother");
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
            ident: Ident::new("smoother"),
            model_name: Ident::new("smooth"),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        };

        // Create a dependent variable
        let dependent_var = Variable::Var {
            ident: Ident::new("processed"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("smoother * 2".to_string(), None)),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("smoother"), module_var);
        variables.insert(Ident::new("processed"), dependent_var);

        let from = Ident::new("smoother");
        let to = Ident::new("processed");

        let equation = generate_module_link_score_equation(&from, &to, &variables);

        // Verify the EXACT equation structure for module-to-variable link
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
            ident: Ident::new("raw_data"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("TIME * 2".to_string(), None)),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        // Create a module variable with an input
        let module_var = Variable::Module {
            ident: Ident::new("processor"),
            model_name: Ident::new("process"),
            units: None,
            inputs: vec![ModuleInput {
                src: Ident::new("raw_data"),
                dst: Ident::new("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("raw_data"), input_var);
        variables.insert(Ident::new("processor"), module_var);

        let from = Ident::new("raw_data");
        let to = Ident::new("processor");

        let equation = generate_module_link_score_equation(&from, &to, &variables);

        // Verify the EXACT equation structure for variable-to-module link
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
            ident: Ident::new("filter_a"),
            model_name: Ident::new("filter"),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        };

        // Create second module (input from first)
        let module_b = Variable::Module {
            ident: Ident::new("filter_b"),
            model_name: Ident::new("filter"),
            units: None,
            inputs: vec![ModuleInput {
                src: Ident::new("filter_a"),
                dst: Ident::new("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("filter_a"), module_a);
        variables.insert(Ident::new("filter_b"), module_b);

        let from = Ident::new("filter_a");
        let to = Ident::new("filter_b");

        let equation = generate_module_link_score_equation(&from, &to, &variables);

        // Verify the EXACT equation structure for module-to-module link
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
            ident: Ident::new("processor"),
            model_name: Ident::new("process_model"),
            units: None,
            inputs: vec![ModuleInput {
                src: Ident::new("input_value"),
                dst: Ident::new("input"),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("processor"), module_var);

        // Create variables that connect to the module
        let input_var = Variable::Var {
            ident: Ident::new("input_value"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("10".to_string(), None)),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let output_var = Variable::Var {
            ident: Ident::new("output_value"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("processor * 2".to_string(), None)),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        variables.insert(Ident::new("input_value"), input_var);
        variables.insert(Ident::new("output_value"), output_var);

        // Test that we can generate module link score equations correctly
        use crate::ltm::{Link, LinkPolarity};
        use std::collections::HashSet;

        // Create links involving the module
        let mut links = HashSet::new();

        // Link from input_value to processor (module input)
        links.insert(Link {
            from: Ident::new("input_value"),
            to: Ident::new("processor"),
            polarity: LinkPolarity::Positive,
        });

        // Link from processor to output_value (module output)
        links.insert(Link {
            from: Ident::new("processor"),
            to: Ident::new("output_value"),
            polarity: LinkPolarity::Positive,
        });

        // Generate link score variables
        let link_vars = generate_link_score_variables(&links, &variables);

        // Check that module link scores were generated
        assert_eq!(link_vars.len(), 2, "Should generate 2 link score variables");

        let has_input_link =
            link_vars.contains_key(&Ident::new("$⁚ltm⁚link_score⁚input_value⁚processor"));
        let has_output_link =
            link_vars.contains_key(&Ident::new("$⁚ltm⁚link_score⁚processor⁚output_value"));

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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = Ident::new("population");
        let to = Ident::new("deaths");
        let flow_var = all_vars.get(&to).expect("Flow variable should exist");

        let equation = generate_link_score_equation(&from, &to, flow_var, all_vars);

        // Verify the EXACT equation structure for stock-to-flow
        // Sign term uses first-order stock change per LTM paper formula
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
        // Create a test project with stock and flow.
        // Use a rate variable with units 1/Month to make the model dimensionally correct:
        // production (widgets/Month) = inventory (widgets) * rate (1/Month)
        let project = TestProject::new("test_basic_inflow")
            .unit("widgets", None)
            .aux_with_units("rate", "0.1", Some("1/Month"))
            .stock_with_units("inventory", "100", &["production"], &[], Some("widgets"))
            .flow_with_units("production", "inventory * rate", Some("widgets/Month"))
            .compile()
            .expect("Project should compile");

        // Get the model and its variables
        let model = project
            .models
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = Ident::new("inventory");
        let flow = Ident::new("production");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // Verify the EXACT equation structure using SAFEDIV
        // Sign term uses first-order stock change per LTM paper formula
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        // Non-stock dependencies (like rate) get wrapped in PREVIOUS()
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
                ((production - PREVIOUS(production)) = 0) OR \
                ((inventory - PREVIOUS(inventory)) = 0) \
                then 0 \
                else ABS(SAFEDIV(((inventory * PREVIOUS(rate)) - PREVIOUS(production)), (production - PREVIOUS(production)), 0)) * \
                SIGN(SAFEDIV(((inventory * PREVIOUS(rate)) - PREVIOUS(production)), \
                    (inventory - PREVIOUS(inventory)), 0))";

        assert_eq!(
            equation, expected,
            "Stock-to-flow equation must match exact format with first-order stock diff for sign"
        );
    }

    #[test]
    fn test_generate_stock_to_flow_equation_outflow() {
        // Create a test project with stock and outflow.
        // Use a time constant with units Month to make the model dimensionally correct:
        // drainage (gallons/Month) = water_tank (gallons) / drain_time (Month)
        let project = TestProject::new("test_outflow")
            .unit("gallons", None)
            .aux_with_units("drain_time", "10", Some("Month"))
            .stock_with_units("water_tank", "100", &[], &["drainage"], Some("gallons"))
            .flow_with_units("drainage", "water_tank / drain_time", Some("gallons/Month"))
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = Ident::new("water_tank");
        let flow = Ident::new("drainage");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // Verify the EXACT equation structure using SAFEDIV
        // Sign term uses first-order stock change per LTM paper formula
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        // Non-stock dependencies (like drain_time) get wrapped in PREVIOUS()
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
                ((drainage - PREVIOUS(drainage)) = 0) OR \
                ((water_tank - PREVIOUS(water_tank)) = 0) \
                then 0 \
                else ABS(SAFEDIV(((water_tank / PREVIOUS(drain_time)) - PREVIOUS(drainage)), (drainage - PREVIOUS(drainage)), 0)) * \
                SIGN(SAFEDIV(((water_tank / PREVIOUS(drain_time)) - PREVIOUS(drainage)), \
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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = Ident::new("population");
        let flow = Ident::new("births");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // Verify the EXACT equation using SAFEDIV - note that non-stock dependencies get PREVIOUS()
        // Sign term uses first-order stock change per LTM paper formula
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = Ident::new("unrelated_stock");
        let flow = Ident::new("constant_flow");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // When flow doesn't depend on stock, partial equation is just the constant
        // Sign term uses first-order stock change per LTM paper formula
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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
        use crate::lexer::LexerType;

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
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let stock = Ident::new("S");
        let flow = Ident::new("inflow");
        let flow_var = all_vars.get(&flow).expect("Flow variable should exist");

        let equation = generate_stock_to_flow_equation(&stock, &flow, flow_var, all_vars);

        // This test validates the correct LTM paper formula implementation
        // Note: 'S' becomes lowercase 's' in stock references but stays uppercase in partial equation
        // Sign term uses first-order stock change per LTM paper formula
        // Returns NaN at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0/0 \
            else if \
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

    mod replace_whole_word_tests {
        use super::super::replace_whole_word;

        #[test]
        fn test_simple_replacement() {
            assert_eq!(
                replace_whole_word("x + y", "x", "PREVIOUS(x)"),
                "PREVIOUS(x) + y"
            );
        }

        #[test]
        fn test_no_partial_match_prefix() {
            // "xy" should NOT match when looking for "x"
            assert_eq!(replace_whole_word("xy + y", "x", "PREVIOUS(x)"), "xy + y");
        }

        #[test]
        fn test_no_partial_match_suffix() {
            // "ax" should NOT match when looking for "x"
            assert_eq!(replace_whole_word("ax + y", "x", "PREVIOUS(x)"), "ax + y");
        }

        #[test]
        fn test_multiple_occurrences() {
            assert_eq!(replace_whole_word("x * x + x", "x", "z"), "z * z + z");
        }

        #[test]
        fn test_at_start() {
            assert_eq!(replace_whole_word("x + 1", "x", "y"), "y + 1");
        }

        #[test]
        fn test_at_end() {
            assert_eq!(replace_whole_word("1 + x", "x", "y"), "1 + y");
        }

        #[test]
        fn test_whole_string() {
            assert_eq!(replace_whole_word("x", "x", "y"), "y");
        }

        #[test]
        fn test_with_underscore_boundary() {
            // Underscores are word characters, so x_1 should NOT match "x"
            assert_eq!(replace_whole_word("x_1 + x", "x", "y"), "x_1 + y");
        }

        #[test]
        fn test_with_digit_boundary() {
            // Digits are word characters, so x1 should NOT match "x"
            assert_eq!(replace_whole_word("x1 + x", "x", "y"), "x1 + y");
        }

        #[test]
        fn test_surrounded_by_operators() {
            assert_eq!(replace_whole_word("a*x+b", "x", "y"), "a*y+b");
        }

        #[test]
        fn test_in_parentheses() {
            assert_eq!(replace_whole_word("(x)", "x", "y"), "(y)");
        }

        #[test]
        fn test_complex_equation() {
            let result = replace_whole_word(
                "population * birth_rate",
                "birth_rate",
                "PREVIOUS(birth_rate)",
            );
            assert_eq!(result, "population * PREVIOUS(birth_rate)");
        }

        #[test]
        fn test_no_match() {
            assert_eq!(replace_whole_word("abc", "x", "y"), "abc");
        }

        #[test]
        fn test_empty_string() {
            assert_eq!(replace_whole_word("", "x", "y"), "");
        }

        #[test]
        fn test_unicode_variable_name() {
            assert_eq!(replace_whole_word("Å + b", "Å", "alpha"), "alpha + b");
        }

        #[test]
        fn test_unicode_word_boundary() {
            // Unicode letters adjacent to other Unicode letters should NOT be replaced
            // because they form a single identifier
            assert_eq!(replace_whole_word("Åβ + γ", "Å", "alpha"), "Åβ + γ");
            // But standalone Unicode identifiers should be replaced
            assert_eq!(replace_whole_word("Å + β", "Å", "alpha"), "alpha + β");
        }

        #[test]
        fn test_mixed_ascii_unicode_boundary() {
            // Unicode letter followed by ASCII should not match partial
            assert_eq!(replace_whole_word("Åbc + x", "Å", "alpha"), "Åbc + x");
            // ASCII followed by Unicode should not match partial
            assert_eq!(replace_whole_word("aÅ + x", "Å", "alpha"), "aÅ + x");
        }

        #[test]
        fn test_repeated_pattern_preserves_boundary_context() {
            // "xx" should not have either x replaced - both are adjacent to word chars
            assert_eq!(replace_whole_word("xx", "x", "y"), "xx");
            // "xx + x" should only replace the standalone x
            assert_eq!(replace_whole_word("xx + x", "x", "y"), "xx + y");
            // Pattern appearing as prefix of longer identifier
            assert_eq!(replace_whole_word("x1x + x", "x", "y"), "x1x + y");
        }

        #[test]
        fn test_overlapping_pattern_occurrences() {
            // "abab" with pattern "ab" - neither should be replaced
            // First "ab" is followed by 'a', second "ab" is preceded by 'b'
            assert_eq!(replace_whole_word("abab", "ab", "XY"), "abab");
            // "ab + ab" - both should be replaced
            assert_eq!(replace_whole_word("ab + ab", "ab", "XY"), "XY + XY");
        }
    }

    #[test]
    fn test_generate_ltm_variables_all_links() {
        // Test that all_links mode generates link score variables for ALL causal
        // links, not just those in loops. The logistic growth model has links
        // like carrying_capacity -> fraction_of_carrying_capacity_used that are
        // NOT in any loop but should get link score variables in all_links mode.
        use crate::project::Project;
        use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &[], None),
                x_flow("births", "population * birth_rate", None),
                x_aux("birth_rate", "fractional_growth_rate", None),
                x_aux("fractional_growth_rate", "0.02 * (1 - fraction_used)", None),
                x_aux("fraction_used", "population / carrying_capacity", None),
                x_aux("carrying_capacity", "1000", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);

        // Standard mode: only loop links get link score variables
        let standard_vars = generate_ltm_variables(&project).unwrap();
        // All-links mode: every causal link gets a link score variable
        let all_link_vars = generate_ltm_variables_all_links(&project).unwrap();

        let main_ident = Ident::new("main");

        // All-links mode should have MORE link score variables than standard mode
        let standard_link_count = standard_vars
            .get(&main_ident)
            .map(|v| {
                v.iter()
                    .filter(|(name, _)| name.as_str().contains("link_score"))
                    .count()
            })
            .unwrap_or(0);

        let all_link_count = all_link_vars
            .get(&main_ident)
            .map(|v| {
                v.iter()
                    .filter(|(name, _)| name.as_str().contains("link_score"))
                    .count()
            })
            .unwrap_or(0);

        assert!(
            all_link_count >= standard_link_count,
            "All-links mode should have at least as many link scores ({}) as standard mode ({})",
            all_link_count,
            standard_link_count
        );

        // All-links mode should NOT have loop score variables
        let has_loop_scores = all_link_vars
            .get(&main_ident)
            .map(|v| {
                v.iter()
                    .any(|(name, _)| name.as_str().contains("abs_loop_score"))
            })
            .unwrap_or(false);

        assert!(
            !has_loop_scores,
            "All-links mode should NOT generate loop score variables"
        );

        // Standard mode SHOULD have loop score variables
        let has_standard_loop_scores = standard_vars
            .get(&main_ident)
            .map(|v| {
                v.iter()
                    .any(|(name, _)| name.as_str().contains("abs_loop_score"))
            })
            .unwrap_or(false);

        assert!(
            has_standard_loop_scores,
            "Standard mode should generate loop score variables"
        );
    }
}
