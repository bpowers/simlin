// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM project augmentation - adds synthetic variables for link and loop scores
//!
//! This module generates synthetic variables for Loops That Matter (LTM) analysis.
//! The generated equations use the PREVIOUS function, which is implemented as a
//! module in stdlib/previous.stmx (not as a builtin function). The PREVIOUS module
//! uses a stock-and-flow structure to store and return the previous timestep's value.

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::canonicalize;
use crate::common::{Canonical, Ident, Result};
use crate::datamodel::{self, Equation};
use crate::lexer::LexerType;
use crate::ltm::{
    CausalGraph, CyclePartitions, Link, Loop, ModuleLtmRole, classify_module_for_ltm, detect_loops,
    normalize_module_ref,
};
use crate::project::Project;
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};

// Type alias for clarity
type SyntheticVariables = Vec<(Ident<Canonical>, datamodel::Variable)>;

/// Map from module model name to the set of input ports that have composite
/// link score variables (i.e., ports with causal pathways to the output).
type CompositePortMap = HashMap<Ident<Canonical>, HashSet<Ident<Canonical>>>;

/// Recursively walk an Expr0 tree, wrapping variable references that appear in
/// `deps` with `PREVIOUS(...)`.  Function names in App nodes are never touched,
/// so a variable named `max` won't corrupt `MAX(max, s)`.
fn wrap_deps_in_previous(expr: Expr0, deps: &HashSet<Ident<Canonical>>) -> Expr0 {
    match expr {
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(&canonicalize(ident.as_str()));
            if deps.contains(&canonical) {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| wrap_deps_in_previous(a, deps))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => {
            Expr0::Op1(op, Box::new(wrap_deps_in_previous(*inner, deps)), loc)
        }
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_deps_in_previous(*lhs, deps)),
            Box::new(wrap_deps_in_previous(*rhs, deps)),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(wrap_deps_in_previous(*cond, deps)),
            Box::new(wrap_deps_in_previous(*then_expr, deps)),
            Box::new(wrap_deps_in_previous(*else_expr, deps)),
            loc,
        ),
        Expr0::Subscript(ident, indices, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| wrap_index_deps_in_previous(idx, deps))
                .collect();
            let canonical = Ident::new(&canonicalize(ident.as_str()));
            let subscript = Expr0::Subscript(ident, indices, loc);
            if deps.contains(&canonical) {
                Expr0::App(
                    UntypedBuiltinFn("PREVIOUS".to_string(), vec![subscript]),
                    loc,
                )
            } else {
                subscript
            }
        }
        Expr0::Const(..) => expr,
    }
}

fn wrap_index_deps_in_previous(index: IndexExpr0, deps: &HashSet<Ident<Canonical>>) -> IndexExpr0 {
    match index {
        IndexExpr0::Expr(e) => IndexExpr0::Expr(wrap_deps_in_previous(e, deps)),
        IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
            wrap_deps_in_previous(l, deps),
            wrap_deps_in_previous(r, deps),
            loc,
        ),
        other => other,
    }
}

/// Parse an equation, wrap all dependency references except `exclude` in PREVIOUS(),
/// and return the resulting equation text.  Falls back to lowercased original text
/// if parsing fails.
fn build_partial_equation(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    exclude: &Ident<Canonical>,
) -> String {
    let deps_to_wrap: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| *d != exclude && normalize_module_ref(d) != *exclude)
        .cloned()
        .collect();

    if deps_to_wrap.is_empty() {
        return equation_text.to_lowercase();
    }

    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return equation_text.to_lowercase();
    };

    let transformed = wrap_deps_in_previous(ast, &deps_to_wrap);
    print_eqn(&transformed)
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

/// Pre-compute which input ports on each dynamic stdlib module have causal
/// pathways to the output. Only these ports get composite link score variables.
fn compute_composite_ports(project: &Project) -> CompositePortMap {
    let mut result = HashMap::new();
    for (model_name, model) in &project.models {
        if !model.implicit {
            continue;
        }
        if classify_module_for_ltm(model_name, model) != ModuleLtmRole::DynamicModule {
            continue;
        }
        let graph = match CausalGraph::from_model(model, project) {
            Ok(g) => g,
            Err(_) => continue,
        };
        let output_ident = Ident::new("output");
        let pathways = graph.enumerate_module_pathways(&output_ident);
        let ports: HashSet<Ident<Canonical>> = pathways.keys().cloned().collect();
        if !ports.is_empty() {
            result.insert(model_name.clone(), ports);
        }
    }
    result
}

fn generate_ltm_variables_inner(
    project: &Project,
    all_links_mode: bool,
) -> Result<HashMap<Ident<Canonical>, SyntheticVariables>> {
    let mut result = HashMap::new();

    let composite_ports = compute_composite_ports(project);

    if all_links_mode {
        // Discovery mode: generate link score variables for ALL causal links
        for (model_name, model) in &project.models {
            if model.implicit {
                continue;
            }

            let graph = CausalGraph::from_model(model, project)?;
            let links: HashSet<Link> = graph.all_links().into_iter().collect();

            let link_score_vars =
                generate_link_score_variables(&links, &model.variables, &composite_ports);

            let synthetic_vars: Vec<_> = link_score_vars.into_iter().collect();
            if !synthetic_vars.is_empty() {
                result.insert(model_name.clone(), synthetic_vars);
            }
        }
    } else {
        for (model_name, model) in &project.models {
            if model.implicit {
                continue;
            }

            let model_loops = detect_loops(model, project)?;
            if model_loops.is_empty() {
                continue;
            }

            let graph = CausalGraph::from_model(model, project)?;
            let partitions = graph.compute_cycle_partitions();

            let mut synthetic_vars = Vec::new();

            let mut loop_links = HashSet::new();
            for loop_item in &model_loops {
                for link in &loop_item.links {
                    loop_links.insert(link.clone());
                }
            }

            let link_score_vars =
                generate_link_score_variables(&loop_links, &model.variables, &composite_ports);
            let loop_score_vars = generate_loop_score_variables(&model_loops, &partitions);

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

    // Generate internal link score variables for dynamic stdlib modules.
    // These live inside the stdlib model's compiled instance.
    // Only process implicit (stdlib/module) models, not user models.
    for (model_name, model) in &project.models {
        if !model.implicit {
            continue;
        }
        if classify_module_for_ltm(model_name, model) != ModuleLtmRole::DynamicModule {
            continue;
        }
        let module_vars = generate_module_internal_ltm_variables(model_name, model, project);
        if !module_vars.is_empty() {
            result
                .entry(model_name.clone())
                .or_insert_with(Vec::new)
                .extend(module_vars);
        }
    }

    Ok(result)
}

/// Generate link score variables for all links
fn generate_link_score_variables(
    links: &HashSet<Link>,
    variables: &HashMap<Ident<Canonical>, Variable>,
    composite_ports: &CompositePortMap,
) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut link_vars = HashMap::new();

    for link in links {
        let var_name = format!(
            "$⁚ltm⁚link_score⁚{}→{}",
            link.from.as_str(),
            link.to.as_str()
        );

        // Check if the link involves a module variable
        let is_module_link = variables.get(&link.from).is_some_and(|v| v.is_module())
            || variables.get(&link.to).is_some_and(|v| v.is_module());

        if is_module_link {
            let from_is_module = variables.get(&link.from).is_some_and(|v| v.is_module());
            let to_is_module = variables.get(&link.to).is_some_and(|v| v.is_module());

            if !from_is_module && to_is_module {
                // input_src -> module: use composite reference if available
                if let Some(Variable::Module {
                    model_name, inputs, ..
                }) = variables.get(&link.to)
                {
                    let port = inputs.iter().find(|i| i.src == link.from).map(|i| &i.dst);
                    let has_composite = port.is_some()
                        && composite_ports
                            .get(model_name)
                            .is_some_and(|ports| ports.contains(port.unwrap()));

                    if let (true, Some(port)) = (has_composite, port) {
                        let eq = generate_module_input_link_score_equation(&link.to, port);
                        let ltm_var = create_aux_variable(&var_name, &eq);
                        link_vars.insert(Ident::new(&var_name), ltm_var);
                    } else {
                        let eq =
                            generate_module_link_score_equation(&link.from, &link.to, variables);
                        let ltm_var = create_aux_variable(&var_name, &eq);
                        link_vars.insert(Ident::new(&var_name), ltm_var);
                    }
                }
            } else if from_is_module && !to_is_module {
                // module -> downstream: use standard ceteris-paribus formula.
                // build_partial_equation is module-ref-aware and excludes
                // interpunct refs that normalize to the module node from
                // PREVIOUS wrapping.  generate_auxiliary_to_auxiliary_equation
                // derives equation text from the AST so identifiers match
                // post-module-expansion deps.
                if let Some(to_var) = variables.get(&link.to) {
                    let equation =
                        generate_link_score_equation(&link.from, &link.to, to_var, variables);
                    let ltm_var = create_aux_variable(&var_name, &equation);
                    link_vars.insert(Ident::new(&var_name), ltm_var);
                }
            } else {
                // module -> module: no downstream equation to analyze
                let eq = generate_module_link_score_equation(&link.from, &link.to, variables);
                let ltm_var = create_aux_variable(&var_name, &eq);
                link_vars.insert(Ident::new(&var_name), ltm_var);
            }
        } else if let Some(to_var) = variables.get(&link.to) {
            // Generate regular link score
            let equation = generate_link_score_equation(&link.from, &link.to, to_var, variables);
            let ltm_var = create_aux_variable(&var_name, &equation);
            link_vars.insert(Ident::new(&var_name), ltm_var);
        }
    }

    link_vars
}

/// Quote an identifier for use in an equation string.
/// Identifiers with special characters (like $, ⁚) need double quotes.
fn quote_ident(ident: &str) -> String {
    if ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
        ident.to_string()
    } else {
        format!("\"{ident}\"")
    }
}

/// Generate link score equation for links involving modules (black box treatment)
fn generate_module_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    _variables: &HashMap<Ident<Canonical>, Variable>,
) -> String {
    let from_q = quote_ident(from.as_str());
    let to_q = quote_ident(to.as_str());

    format!(
        "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                (({to_q} - PREVIOUS({to_q})) = 0) OR (({from_q} - PREVIOUS({from_q})) = 0) \
                then 0 \
                else ABS((({to_q} - PREVIOUS({to_q}))) / ({to_q} - PREVIOUS({to_q}))) * \
                (if \
                    ({from_q} - PREVIOUS({from_q})) = 0 \
                    then 0 \
                    else SIGN((({to_q} - PREVIOUS({to_q}))) / ({from_q} - PREVIOUS({from_q}))))"
    )
}

/// Generate loop score variables for all loops.
///
/// Absolute loop scores are unchanged (product of link scores).
/// Relative loop scores use partition-scoped denominators: each loop's
/// relative score equation only references loops in the same partition.
/// Loops with no stocks form their own unpartitioned group.
fn generate_loop_score_variables(
    loops: &[Loop],
    partitions: &CyclePartitions,
) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut loop_vars = HashMap::new();

    // First, generate absolute loop scores (unchanged)
    for loop_item in loops {
        let var_name = format!("$⁚ltm⁚loop_score⁚{}", loop_item.id);
        let equation = generate_loop_score_equation(loop_item);
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(Ident::new(&var_name), ltm_var);
    }

    // Group loops by partition for relative score computation
    let mut partition_groups: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, loop_item) in loops.iter().enumerate() {
        let partition = partitions.partition_for_loop(loop_item);
        partition_groups.entry(partition).or_default().push(i);
    }

    // Pre-collect IDs per partition group to avoid cloning Loop structs
    let group_ids: HashMap<Option<usize>, Vec<&str>> = partition_groups
        .iter()
        .map(|(&partition, indices)| {
            let ids: Vec<&str> = indices.iter().map(|&idx| loops[idx].id.as_str()).collect();
            (partition, ids)
        })
        .collect();

    // Generate relative loop scores with partition-scoped denominators
    for loop_item in loops {
        let var_name = format!("$⁚ltm⁚rel_loop_score⁚{}", loop_item.id);
        let partition = partitions.partition_for_loop(loop_item);
        let same_group_ids = &group_ids[&partition];
        let equation = generate_relative_loop_score_equation(&loop_item.id, same_group_ids);
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
    use crate::ast::Ast;

    // Get the equation text of the 'to' variable.  Prefer the AST when
    // available because the `eqn` field holds the *original* text (e.g.,
    // "SMTH1(x, 5)") while the AST holds the post-module-expansion form
    // (e.g., Var("$⁚s⁚0⁚smth1·output")).  Using the AST-derived text
    // ensures the identifiers in the equation match those in `deps`.
    let to_equation = if let Some(ast) = to_var.ast() {
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => crate::patch::expr2_to_string(expr),
            _ => match to_var {
                Variable::Stock {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                }
                | Variable::Var {
                    eqn: Some(Equation::Scalar(eq)),
                    ..
                } => eq.clone(),
                _ => "0".to_string(),
            },
        }
    } else {
        match to_var {
            Variable::Stock {
                eqn: Some(Equation::Scalar(eq)),
                ..
            }
            | Variable::Var {
                eqn: Some(Equation::Scalar(eq)),
                ..
            } => eq.clone(),
            _ => "0".to_string(),
        }
    };

    // Get dependencies of the 'to' variable
    let deps = if let Some(ast) = to_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    let partial_eq = build_partial_equation(&to_equation, &deps, from);

    let from_q = quote_ident(from.as_str());
    let to_q = quote_ident(to.as_str());

    // Using SAFEDIV for both divisions
    // Note: We still need the outer check for when EITHER is zero, since we multiply the results
    let abs_part = format!(
        "ABS(SAFEDIV((({partial_eq}) - PREVIOUS({to_q})), ({to_q} - PREVIOUS({to_q})), 0))",
    );
    let sign_part = format!(
        "SIGN(SAFEDIV((({partial_eq}) - PREVIOUS({to_q})), ({from_q} - PREVIOUS({from_q})), 0))",
    );

    // Return 0 at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                (({to_q} - PREVIOUS({to_q})) = 0) OR (({from_q} - PREVIOUS({from_q})) = 0) \
                then 0 \
                else {abs_part} * {sign_part}",
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

    // Per the corrected 2023 formula (Schoenberg et al., Eq. 3):
    //   LS(inflow -> S)  = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)
    //   LS(outflow -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)
    //
    // The polarity is structural (fixed +1/-1), not dynamic.  ABS ensures
    // the magnitude is always positive; the sign is applied outside.
    //
    // The numerator uses PREVIOUS values to align timing with the denominator.
    // At time t, the flow at t-1 (PREVIOUS(flow)) is what drove the stock change from t-1 to t.
    // We measure the change in that causal flow: flow(t-1) - flow(t-2).
    let numerator = format!("(PREVIOUS({flow}) - PREVIOUS(PREVIOUS({flow})))");
    let denominator = format!(
        "(({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock}))))"
    );

    // Return 0 for the first two timesteps when we don't have enough history for second-order differences
    format!(
        "if \
            (TIME = PREVIOUS(TIME)) OR (PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))) \
            then 0 \
            else {sign}ABS(SAFEDIV({numerator}, {denominator}, 0))"
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
            eqn: Some(Equation::Scalar(eq)),
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

    let partial_eq = build_partial_equation(&flow_equation, &deps, stock);

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

    // Return 0 at the initial timestep when PREVIOUS values don't exist yet
    format!(
        "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
                "\"$⁚ltm⁚link_score⁚{}→{}\"",
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

/// Generate the equation for a relative loop score variable.
///
/// `same_group_ids` contains the IDs of all loops in the same partition group
/// (including this loop itself). The denominator sums only these loops.
fn generate_relative_loop_score_equation(loop_id: &str, same_group_ids: &[&str]) -> String {
    let loop_score_var = format!("\"$⁚ltm⁚loop_score⁚{loop_id}\"");

    let all_loop_scores: Vec<String> = same_group_ids
        .iter()
        .map(|id| format!("ABS(\"$⁚ltm⁚loop_score⁚{id}\")"))
        .collect();

    let sum_expr = if all_loop_scores.is_empty() {
        "1".to_string() // Avoid division by zero
    } else {
        all_loop_scores.join(" + ")
    };

    // Relative score formula using SAFEDIV for division by zero protection
    format!("SAFEDIV({loop_score_var}, ({sum_expr}), 0)")
}

/// Generate internal link score, pathway, and composite variables for a
/// dynamic stdlib module. Returns variables to add to the stdlib model.
fn generate_module_internal_ltm_variables(
    _model_name: &Ident<Canonical>,
    module_model: &crate::model::ModelStage1,
    project: &Project,
) -> SyntheticVariables {
    let mut vars = Vec::new();

    let graph = match CausalGraph::from_model(module_model, project) {
        Ok(g) => g,
        Err(_) => return vars,
    };

    // Generate internal link score variables for all causal links in the module
    let links: HashSet<Link> = graph.all_links().into_iter().collect();
    for link in &links {
        let var_name = format!("$⁚ltm⁚ilink⁚{}→{}", link.from.as_str(), link.to.as_str());
        if let Some(to_var) = module_model.variables.get(&link.to) {
            let equation =
                generate_link_score_equation(&link.from, &link.to, to_var, &module_model.variables);
            let ltm_var = create_aux_variable(&var_name, &equation);
            vars.push((Ident::new(&var_name), ltm_var));
        }
    }

    // Find the output variable (stock named "output" for stdlib modules)
    let output_ident = Ident::new("output");

    // Enumerate pathways from each input port to output
    let pathways = graph.enumerate_module_pathways(&output_ident);

    for (input_port, port_pathways) in &pathways {
        // Generate pathway score variables (product of constituent ilink scores)
        let mut pathway_names = Vec::new();
        for (idx, pathway_links) in port_pathways.iter().enumerate() {
            let path_var_name = format!("$⁚ltm⁚path⁚{}⁚{}", input_port.as_str(), idx);

            let link_score_refs: Vec<String> = pathway_links
                .iter()
                .map(|link| {
                    format!(
                        "\"$⁚ltm⁚ilink⁚{}→{}\"",
                        link.from.as_str(),
                        link.to.as_str()
                    )
                })
                .collect();

            let equation = if link_score_refs.is_empty() {
                "0".to_string()
            } else {
                link_score_refs.join(" * ")
            };

            let ltm_var = create_aux_variable(&path_var_name, &equation);
            pathway_names.push(path_var_name.clone());
            vars.push((Ident::new(&path_var_name), ltm_var));
        }

        // Generate composite score variable (max-magnitude pathway)
        let composite_name = format!("$⁚ltm⁚composite⁚{}", input_port.as_str());

        let equation = generate_max_abs_chain(&pathway_names);
        let ltm_var = create_aux_variable(&composite_name, &equation);
        vars.push((Ident::new(&composite_name), ltm_var));
    }

    vars
}

/// Generate a deterministic nested max-abs selection equation.
///
/// For N=1: just the pathway name.
/// For N=2: `if ABS(p1) >= ABS(p2) then p1 else p2`
/// For N>2: `if ABS(pN) >= ABS(max_of_rest) then pN else max_of_rest` (recursive)
fn generate_max_abs_chain(pathway_names: &[String]) -> String {
    match pathway_names.len() {
        0 => "0".to_string(),
        1 => format!("\"{}\"", pathway_names[0]),
        2 => {
            let p0 = &pathway_names[0];
            let p1 = &pathway_names[1];
            format!("if ABS(\"{p0}\") >= ABS(\"{p1}\") then \"{p0}\" else \"{p1}\"")
        }
        _ => {
            let last = &pathway_names[pathway_names.len() - 1];
            let rest = generate_max_abs_chain(&pathway_names[..pathway_names.len() - 1]);
            format!("if ABS(\"{last}\") >= ABS(({rest})) then \"{last}\" else ({rest})")
        }
    }
}

/// Generate a composite link score equation for a parent model link where
/// the target is a dynamic module. References the module's internal
/// composite score via interpunct notation.
fn generate_module_input_link_score_equation(
    module_ident: &Ident<Canonical>,
    input_port: &Ident<Canonical>,
) -> String {
    format!(
        "\"{module}\u{00B7}$⁚ltm⁚composite⁚{port}\"",
        module = module_ident.as_str(),
        port = input_port.as_str(),
    )
}

/// Create an auxiliary variable with the given equation
fn create_aux_variable(name: &str, equation: &str) -> crate::datamodel::Variable {
    use crate::datamodel;

    datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(name).into_owned(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: "LTM".to_string(),
        units: None, // LTM scores are dimensionless by design, no need to declare
        gf: None,
        can_be_module_input: false,
        visibility: datamodel::Visibility::Public,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_common::TestProject;

    #[test]
    fn test_build_partial_equation_preserves_builtins() {
        let mut deps = HashSet::new();
        deps.insert(Ident::new("x"));
        deps.insert(Ident::new("s"));
        let exclude = Ident::new("x");

        // MAX is a builtin — must not be rewritten even though "max" would
        // match after lowercasing in the old approach.  The parse roundtrip
        // lowercases function names, which is fine (case-insensitive language).
        assert_eq!(
            build_partial_equation("MAX(x, s)", &deps, &exclude),
            "max(x, PREVIOUS(s))"
        );
    }

    #[test]
    fn test_build_partial_equation_simple() {
        let mut deps = HashSet::new();
        deps.insert(Ident::new("x"));
        deps.insert(Ident::new("z"));
        let exclude = Ident::new("x");

        assert_eq!(
            build_partial_equation("x * 2 + z", &deps, &exclude),
            "x * 2 + PREVIOUS(z)"
        );
    }

    #[test]
    fn test_build_partial_equation_no_deps_to_wrap() {
        let mut deps = HashSet::new();
        deps.insert(Ident::new("x"));
        let exclude = Ident::new("x");

        assert_eq!(build_partial_equation("x * 2", &deps, &exclude), "x * 2");
    }

    #[test]
    fn test_build_partial_equation_if_then_else() {
        let mut deps = HashSet::new();
        deps.insert(Ident::new("a"));
        deps.insert(Ident::new("b"));
        deps.insert(Ident::new("c"));
        let exclude = Ident::new("a");

        assert_eq!(
            build_partial_equation("IF a > 0 THEN b ELSE c", &deps, &exclude),
            "if (a > 0) then (PREVIOUS(b)) else (PREVIOUS(c))"
        );
    }

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
            "\"$⁚ltm⁚link_score⁚x→y\" * \"$⁚ltm⁚link_score⁚y→x\""
        );
    }

    #[test]
    fn test_generate_relative_loop_score_equation() {
        let equation = generate_relative_loop_score_equation("R1", &["R1", "B1"]);

        // Should use SAFEDIV for division by zero protection
        assert!(equation.contains("SAFEDIV"));
        // Should reference the specific loop score (with double quotes and $ prefix)
        assert!(equation.contains("\"$⁚ltm⁚loop_score⁚R1\""));
        // Should have sum of all loop scores in denominator (with double quotes and $ prefix)
        assert!(equation.contains("ABS(\"$⁚ltm⁚loop_score⁚R1\") + ABS(\"$⁚ltm⁚loop_score⁚B1\")"));
    }

    #[test]
    fn test_single_loop_relative_score_uses_safediv() {
        let equation = generate_relative_loop_score_equation("B1", &["B1"]);

        // Even with a single loop, the equation should use SAFEDIV to produce
        // sign(score) rather than a hardcoded "1".
        assert!(
            equation.contains("SAFEDIV"),
            "Single-loop relative score should use SAFEDIV, got: {equation}"
        );
        assert!(equation.contains("\"$⁚ltm⁚loop_score⁚B1\""));
    }

    #[test]
    fn test_single_balancing_loop_relative_score_is_negative() {
        use crate::test_common::TestProject;
        use std::sync::Arc;

        // Goal-seeking: level -> gap -> adjustment -> level (balancing)
        let project = TestProject::new("test_single_balancing_rel")
            .with_sim_time(0.0, 5.0, 0.25)
            .aux("goal", "100", None)
            .stock("level", "50", &["adjustment"], &[], None)
            .aux("gap", "goal - level", None)
            .aux("adjustment_time", "5", None)
            .flow("adjustment", "gap / adjustment_time", None)
            .compile()
            .expect("Project should compile");

        let ltm_project = project.with_ltm().expect("Should augment with LTM");
        let project_rc = Arc::new(ltm_project);
        let sim = crate::interpreter::Simulation::new(&project_rc, "main")
            .expect("Should create simulation");
        let results = sim
            .run_to_end()
            .expect("Simulation should run successfully");

        // Find the relative loop score variable
        let rel_var = results
            .offsets
            .keys()
            .find(|k| k.as_str().starts_with("$⁚ltm⁚rel_loop_score⁚"))
            .expect("Should have a relative loop score variable");

        let offset = results.offsets[rel_var];
        let num_vars = results.step_size;

        // Initial timesteps have 0 scores (no dynamics yet); subsequent ones are -1
        let scores: Vec<f64> = (0..results.step_count)
            .map(|step| results.data[step * num_vars + offset])
            .collect();

        let nonzero_scores: Vec<f64> = scores.iter().copied().filter(|v| *v != 0.0).collect();
        assert!(
            !nonzero_scores.is_empty(),
            "Should have some non-zero relative loop score values"
        );

        for score in &nonzero_scores {
            assert!(
                (*score - -1.0).abs() < 1e-6,
                "Single balancing loop relative score should be -1, got {score}"
            );
        }
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
            .any(|(name, _)| name.as_str().contains("$⁚ltm⁚link_score⁚population→births"));
        let has_births_to_pop = vars
            .iter()
            .any(|(name, _)| name.as_str().contains("$⁚ltm⁚link_score⁚births→population"));

        assert!(
            has_pop_to_births || has_births_to_pop,
            "Should have link score variables for the feedback loop"
        );

        // Check for loop score variable
        let has_loop_score = vars
            .iter()
            .any(|(name, _)| name.as_str().starts_with("$⁚ltm⁚loop_score⁚"));
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
        // ABS wraps the ratio; sign is fixed (+1 for inflows) per corrected 2023 formula
        // Returns 0 for first two timesteps when insufficient history
        let expected = "if \
            (TIME = PREVIOUS(TIME)) OR (PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))) \
            then 0 \
            else ABS(SAFEDIV(\
                (PREVIOUS(inflow_rate) - PREVIOUS(PREVIOUS(inflow_rate))), \
                ((water_in_tank - PREVIOUS(water_in_tank)) - (PREVIOUS(water_in_tank) - PREVIOUS(PREVIOUS(water_in_tank)))), \
                0\
            ))";

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
        // ABS wraps the ratio; sign is fixed (-1 for outflows) per corrected 2023 formula
        // Returns 0 for first two timesteps when insufficient history
        let expected = "if \
            (TIME = PREVIOUS(TIME)) OR (PREVIOUS(TIME) = PREVIOUS(PREVIOUS(TIME))) \
            then 0 \
            else -ABS(SAFEDIV(\
                (PREVIOUS(outflow_rate) - PREVIOUS(PREVIOUS(outflow_rate))), \
                ((water_in_tank - PREVIOUS(water_in_tank)) - (PREVIOUS(water_in_tank) - PREVIOUS(PREVIOUS(water_in_tank)))), \
                0\
            ))";

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
            eqn: Some(Equation::Scalar("10 + SIN(TIME)".to_string())),
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
        let link_vars = generate_link_score_variables(&links, &variables, &HashMap::new());

        // Check that a link score variable was created
        assert!(!link_vars.is_empty(), "Should generate module link score");

        // Check the variable name
        let expected_name = Ident::new("$⁚ltm⁚link_score⁚raw_input→smoother");
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
            eqn: Some(Equation::Scalar("smoother * 2".to_string())),
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                ((processed - PREVIOUS(processed)) = 0) OR ((smoother - PREVIOUS(smoother)) = 0) \
                then 0 \
                else ABS(((processed - PREVIOUS(processed))) / (processed - PREVIOUS(processed))) * \
                (if \
                    (smoother - PREVIOUS(smoother)) = 0 \
                    then 0 \
                    else SIGN(((processed - PREVIOUS(processed))) / (smoother - PREVIOUS(smoother))))";

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
            eqn: Some(Equation::Scalar("TIME * 2".to_string())),
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                ((processor - PREVIOUS(processor)) = 0) OR ((raw_data - PREVIOUS(raw_data)) = 0) \
                then 0 \
                else ABS(((processor - PREVIOUS(processor))) / (processor - PREVIOUS(processor))) * \
                (if \
                    (raw_data - PREVIOUS(raw_data)) = 0 \
                    then 0 \
                    else SIGN(((processor - PREVIOUS(processor))) / (raw_data - PREVIOUS(raw_data))))";

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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                ((filter_b - PREVIOUS(filter_b)) = 0) OR ((filter_a - PREVIOUS(filter_a)) = 0) \
                then 0 \
                else ABS(((filter_b - PREVIOUS(filter_b))) / (filter_b - PREVIOUS(filter_b))) * \
                (if \
                    (filter_a - PREVIOUS(filter_a)) = 0 \
                    then 0 \
                    else SIGN(((filter_b - PREVIOUS(filter_b))) / (filter_a - PREVIOUS(filter_a))))";

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
            eqn: Some(Equation::Scalar("10".to_string())),
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
            eqn: Some(Equation::Scalar("processor * 2".to_string())),
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
        let link_vars = generate_link_score_variables(&links, &variables, &HashMap::new());

        // Check that module link scores were generated
        assert_eq!(link_vars.len(), 2, "Should generate 2 link score variables");

        let has_input_link =
            link_vars.contains_key(&Ident::new("$⁚ltm⁚link_score⁚input_value→processor"));
        let has_output_link =
            link_vars.contains_key(&Ident::new("$⁚ltm⁚link_score⁚processor→output_value"));

        assert!(
            has_input_link,
            "Should generate link score for input to module"
        );
        assert!(
            has_output_link,
            "Should generate link score for module to output"
        );

        // input->module uses black box, module->downstream uses ceteris-paribus
        for var in link_vars.values() {
            if let datamodel::Variable::Aux(aux) = var
                && let datamodel::Equation::Scalar(eq) = &aux.equation
            {
                assert!(
                    eq.contains("SIGN") || eq.contains("SAFEDIV"),
                    "Module link should include SIGN or SAFEDIV"
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        // Non-stock dependencies (like rate) get wrapped in PREVIOUS()
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        // Non-stock dependencies (like drain_time) get wrapped in PREVIOUS()
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
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
    fn test_link_score_with_builtin_name_collision() {
        // Regression: a variable named "max" must not corrupt the MAX() builtin
        // in the same equation.  The old string-based approach lowercased the
        // entire equation then did whole-word replacement, turning
        // MAX(max, s) into PREVIOUS(max)(PREVIOUS(max), s).
        let project = TestProject::new("test_builtin_collision")
            .aux("max_val", "10", None)
            .aux("s", "5", None)
            .aux("y", "MAX(max_val, s)", None)
            .compile()
            .expect("Project should compile");

        let model = project
            .models
            .get(&Ident::new("main"))
            .expect("Model should exist");
        let all_vars = &model.variables;

        let from = Ident::new("max_val");
        let to = Ident::new("y");
        let y_var = all_vars.get(&to).expect("Y variable should exist");

        let equation = generate_link_score_equation(&from, &to, y_var, all_vars);

        // The partial equation must preserve max as a function call and only
        // wrap the variable `s` in PREVIOUS, not the function name.
        // (The parse roundtrip lowercases function names — case-insensitive language.)
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                ((y - PREVIOUS(y)) = 0) OR ((max_val - PREVIOUS(max_val)) = 0) \
                then 0 \
                else ABS(SAFEDIV(((max(max_val, PREVIOUS(s))) - PREVIOUS(y)), (y - PREVIOUS(y)), 0)) * \
                SIGN(SAFEDIV(((max(max_val, PREVIOUS(s))) - PREVIOUS(y)), (max_val - PREVIOUS(max_val)), 0))";

        assert_eq!(
            equation, expected,
            "Builtin function names must not be corrupted by variable substitution"
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
        let equation = "\"$⁚ltm⁚link_score⁚x→y\"";
        let result = Expr0::new(equation, LexerType::Equation);
        assert!(result.is_ok(), "Double quoted $ variable should parse");

        if let Ok(Some(Expr0::Var(id, _))) = &result {
            println!("Parsed variable RawIdent: {:?}", id.as_str());
            println!("Canonicalized: {}", canonicalize(id.as_str()));

            // Check if quotes are included in the identifier
            assert_eq!(id.as_str(), "\"$⁚ltm⁚link_score⁚x→y\"");
        }

        // Test 2: Complex equation with quoted variables
        let equation = "\"$⁚ltm⁚link_score⁚x→y\" * \"$⁚ltm⁚link_score⁚y→x\"";
        let result = Expr0::new(equation, LexerType::Equation);
        assert!(
            result.is_ok(),
            "Multiplication of quoted $ variables should parse"
        );

        // Test 3: What happens when we create AST with quoted names and print it
        let var_ast = Expr0::Var(
            RawIdent::new_from_str("\"$⁚ltm⁚link_score⁚x→y\""),
            Loc::default(),
        );
        let printed = print_eqn(&var_ast);
        println!("AST with quoted $ variable printed as: '{printed}'");

        assert_eq!(
            printed, "\"$⁚ltm⁚link_score⁚x→y\"",
            "print_eqn re-quotes identifiers with special characters"
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
        // Sign term uses first-order stock change per LTM paper formula
        // Returns 0 at initial timestep when PREVIOUS values don't exist
        let expected = "if \
            (TIME = PREVIOUS(TIME)) \
            then 0 \
            else if \
                ((inflow - PREVIOUS(inflow)) = 0) OR \
                ((s - PREVIOUS(s)) = 0) \
                then 0 \
                else ABS(SAFEDIV(((s * PREVIOUS(growth_rate)) - PREVIOUS(inflow)), (inflow - PREVIOUS(inflow)), 0)) * \
                SIGN(SAFEDIV(((s * PREVIOUS(growth_rate)) - PREVIOUS(inflow)), \
                    (s - PREVIOUS(s)), 0))";

        assert_eq!(
            equation, expected,
            "Equation must match LTM paper formula with first-order stock diff for sign"
        );
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
                    .any(|(name, _)| name.as_str().contains("⁚loop_score⁚"))
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
                    .any(|(name, _)| name.as_str().contains("⁚loop_score⁚"))
            })
            .unwrap_or(false);

        assert!(
            has_standard_loop_scores,
            "Standard mode should generate loop score variables"
        );
    }

    #[test]
    fn test_max_abs_chain_n1() {
        let result = generate_max_abs_chain(&["$⁚ltm⁚path⁚input⁚0".to_string()]);
        assert_eq!(result, "\"$⁚ltm⁚path⁚input⁚0\"");
    }

    #[test]
    fn test_max_abs_chain_n2() {
        let result = generate_max_abs_chain(&[
            "$⁚ltm⁚path⁚input⁚0".to_string(),
            "$⁚ltm⁚path⁚input⁚1".to_string(),
        ]);
        assert!(result.contains("ABS"), "N=2 should use ABS comparison");
        assert!(result.contains("then"), "N=2 should use if/then/else");
        assert!(result.contains("$⁚ltm⁚path⁚input⁚0"));
        assert!(result.contains("$⁚ltm⁚path⁚input⁚1"));
    }

    #[test]
    fn test_max_abs_chain_n3() {
        let result = generate_max_abs_chain(&[
            "$⁚ltm⁚path⁚input⁚0".to_string(),
            "$⁚ltm⁚path⁚input⁚1".to_string(),
            "$⁚ltm⁚path⁚input⁚2".to_string(),
        ]);
        // Should be nested: if ABS(p2) >= ABS(max(p0,p1)) then p2 else max(p0,p1)
        assert!(result.contains("$⁚ltm⁚path⁚input⁚2"));
        // Verify nesting
        // The N=2 inner case has 1 "if", plus the outer comparison has 1 "if",
        // and the recursive N=2 appears twice (once in the condition, once in the else).
        // Just verify it's nested with more than 1 "if"
        let count_if = result.matches("if ").count();
        assert!(
            count_if >= 2,
            "N=3 should have nested if/then/else, got {} ifs",
            count_if
        );
    }

    #[test]
    fn test_max_abs_chain_n0() {
        let result = generate_max_abs_chain(&[]);
        assert_eq!(result, "0");
    }

    #[test]
    fn test_smth1_internal_link_scores_generated() {
        let project = TestProject::new("test_smth1_ilink")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let ltm_project = project
            .with_ltm_all_links()
            .expect("should augment with LTM");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = ltm_project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        // Check that internal link score variables were generated
        let has_ilink = smth1_model
            .variables
            .keys()
            .any(|k| k.as_str().contains("$⁚ltm⁚ilink⁚"));

        assert!(
            has_ilink,
            "Augmented smth1 model should have internal link score variables. \
             Variables: {:?}",
            smth1_model.variables.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_smth1_composite_generated() {
        let project = TestProject::new("test_smth1_comp")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let ltm_project = project
            .with_ltm_all_links()
            .expect("should augment with LTM");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = ltm_project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        let has_composite = smth1_model
            .variables
            .keys()
            .any(|k| k.as_str().contains("$⁚ltm⁚composite⁚"));

        assert!(
            has_composite,
            "Augmented smth1 model should have composite score variable. \
             Variables: {:?}",
            smth1_model.variables.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_smth1_pathway_equation() {
        let project = TestProject::new("test_smth1_path")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let ltm_project = project
            .with_ltm_all_links()
            .expect("should augment with LTM");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = ltm_project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        // Find the pathway variable
        let path_var = smth1_model
            .variables
            .iter()
            .find(|(k, _)| k.as_str().contains("$⁚ltm⁚path⁚"));

        assert!(
            path_var.is_some(),
            "Should have a pathway score variable. Variables: {:?}",
            smth1_model.variables.keys().collect::<Vec<_>>()
        );

        // The pathway equation should be a product of ilink variables.
        if let Some((_, var)) = path_var
            && let Some(eq) = var.scalar_equation()
        {
            assert!(
                eq.contains("$⁚ltm⁚ilink⁚"),
                "Pathway equation should reference internal link scores, got: {eq}"
            );
        }
    }

    #[test]
    fn test_smooth_parent_link_score_references_composite() {
        let project = TestProject::new("test_smooth_composite_ref")
            .with_sim_time(0.0, 10.0, 1.0)
            .stock("level", "50", &["adj"], &[], None)
            .aux("gap", "100 - level", None)
            .flow("adj", "SMTH1(gap, 5)", None)
            .compile()
            .expect("should compile");

        let ltm_project = project
            .with_ltm_all_links()
            .expect("should augment with LTM");

        let main_ident: Ident<Canonical> = Ident::new("main");
        let main_model = ltm_project
            .models
            .get(&main_ident)
            .expect("should have main model");

        // Find the link score variable for gap -> smth1_module
        let module_link = main_model.variables.iter().find(|(k, _)| {
            let s = k.as_str();
            s.contains("$⁚ltm⁚link_score⁚") && s.contains("smth1")
        });

        if let Some((name, var)) = module_link
            && let Some(eq) = var.scalar_equation()
        {
            // If the target is the module (gap -> module), the equation should
            // reference the composite score via interpunct notation
            if name.as_str().contains("→$⁚") {
                assert!(
                    eq.contains("composite") || eq.contains("PREVIOUS"),
                    "Module input link score should reference composite or use \
                     black-box formula, got: {eq}"
                );
            }
        }
    }

    #[test]
    fn test_module_link_score_equation_with_dollar_ident() {
        // Test that the black-box module link score equation parses when
        // variable names contain $ and ⁚
        let from = Ident::new("$⁚smoothed_level⁚0⁚smth1");
        let to = Ident::new("smoothed_level");

        let variables = HashMap::new();
        let eq = generate_module_link_score_equation(&from, &to, &variables);

        // The equation must parse successfully
        let result = crate::ast::Expr0::new(&eq, crate::lexer::LexerType::Equation);
        assert!(
            result.is_ok(),
            "Module link score equation should parse: {eq}\nError: {result:?}"
        );
        let ast = result.unwrap();
        assert!(
            ast.is_some(),
            "Module link score equation should produce an AST: {eq}"
        );
    }

    #[test]
    fn test_path_to_links_open_path() {
        // Verify path_to_links produces N-1 links for N nodes (open, not closed)
        use crate::ltm::CausalGraph;
        use std::collections::{HashMap, HashSet};

        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        edges
            .entry(Ident::new("a"))
            .or_default()
            .push(Ident::new("b"));
        edges
            .entry(Ident::new("b"))
            .or_default()
            .push(Ident::new("c"));

        let graph = CausalGraph {
            edges,
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let path = vec![Ident::new("a"), Ident::new("b"), Ident::new("c")];
        let links = graph.path_to_links(&path);

        assert_eq!(
            links.len(),
            2,
            "3-node open path should produce 2 links, not 3"
        );
        assert_eq!(links[0].from.as_str(), "a");
        assert_eq!(links[0].to.as_str(), "b");
        assert_eq!(links[1].from.as_str(), "b");
        assert_eq!(links[1].to.as_str(), "c");
    }

    #[test]
    fn test_smth1_single_pathway() {
        use crate::ltm::CausalGraph;

        let project = TestProject::new("test_smth1_pathway")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        let graph = CausalGraph::from_model(smth1_model, &project).unwrap();
        let output_ident = Ident::new("output");
        let pathways = graph.enumerate_module_pathways(&output_ident);

        // smth1 should have at least one input port with pathways to output
        assert!(
            !pathways.is_empty(),
            "smth1 should have pathways from input ports to output. \
             Edges: {:?}",
            graph
        );

        // The 'input' port should have a pathway: input -> flow -> output
        let input_ident = Ident::new("input");
        if let Some(input_paths) = pathways.get(&input_ident) {
            assert!(
                !input_paths.is_empty(),
                "input port should have at least one pathway to output"
            );
            // Each pathway link should have 2 links (input -> flow -> output)
            for path in input_paths {
                assert_eq!(
                    path.len(),
                    2,
                    "input -> flow -> output pathway should have 2 links, got {}",
                    path.len()
                );
            }
        }
    }

    #[test]
    fn test_enumerate_module_pathways_excludes_intermediates() {
        use crate::ltm::CausalGraph;

        let project = TestProject::new("test_pathway_filter")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        let graph = CausalGraph::from_model(smth1_model, &project).unwrap();
        let output_ident = Ident::new("output");
        let pathways = graph.enumerate_module_pathways(&output_ident);

        // Only true input ports (no incoming edges within the module) should
        // appear as keys -- intermediate variables like "flow" must be excluded.
        for port_name in pathways.keys() {
            assert_ne!(
                port_name.as_str(),
                "flow",
                "Intermediate variable 'flow' should not be treated as an input port"
            );
        }
    }

    #[test]
    fn test_smth1_no_composite_for_intermediate_variables() {
        let project = TestProject::new("test_no_flow_composite")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("x", "10", None)
            .aux("s", "SMTH1(x, 5)", None)
            .compile()
            .expect("should compile");

        let ltm_project = project
            .with_ltm_all_links()
            .expect("should augment with LTM");

        let smth1_ident = Ident::new("stdlib⁚smth1");
        let smth1_model = ltm_project
            .models
            .get(&smth1_ident)
            .expect("should have stdlib⁚smth1 model");

        let composite_vars: Vec<_> = smth1_model
            .variables
            .keys()
            .filter(|k| k.as_str().contains("$⁚ltm⁚composite⁚"))
            .collect();

        // Composites should only exist for true input ports, not "flow"
        for var in &composite_vars {
            assert!(
                !var.as_str().contains("composite⁚flow"),
                "Should not have composite for intermediate variable 'flow', found: {}",
                var.as_str()
            );
        }

        // But should still have composites for actual input ports
        assert!(
            !composite_vars.is_empty(),
            "Should still have composites for real input ports"
        );
    }

    #[test]
    fn test_relative_score_cross_partition_isolation() {
        use crate::ltm::{CyclePartitions, LoopPolarity};

        // 4 loops: L1,L2 share stock_a (partition 0), L3,L4 share stock_c (partition 1)
        let loops = vec![
            Loop {
                id: "r1".to_string(),
                links: vec![],
                stocks: vec![Ident::new("stock_a")],
                polarity: LoopPolarity::Reinforcing,
            },
            Loop {
                id: "b1".to_string(),
                links: vec![],
                stocks: vec![Ident::new("stock_a")],
                polarity: LoopPolarity::Balancing,
            },
            Loop {
                id: "r2".to_string(),
                links: vec![],
                stocks: vec![Ident::new("stock_c")],
                polarity: LoopPolarity::Reinforcing,
            },
            Loop {
                id: "b2".to_string(),
                links: vec![],
                stocks: vec![Ident::new("stock_c")],
                polarity: LoopPolarity::Balancing,
            },
        ];

        let partitions = CyclePartitions {
            partitions: vec![
                vec![Ident::new("stock_a"), Ident::new("stock_b")],
                vec![Ident::new("stock_c"), Ident::new("stock_d")],
            ],
            stock_partition: vec![
                (Ident::new("stock_a"), 0),
                (Ident::new("stock_b"), 0),
                (Ident::new("stock_c"), 1),
                (Ident::new("stock_d"), 1),
            ]
            .into_iter()
            .collect(),
        };

        let vars = generate_loop_score_variables(&loops, &partitions);

        // L1 (r1) relative score should reference r1 and b1, NOT r2 or b2
        let r1_rel = vars
            .get(&Ident::new("$⁚ltm⁚rel_loop_score⁚r1"))
            .expect("should have r1 relative score");
        let r1_eq = match r1_rel {
            datamodel::Variable::Aux(aux) => match &aux.equation {
                Equation::Scalar(eq) => eq.clone(),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux variable"),
        };
        assert!(
            r1_eq.contains("loop_score⁚r1"),
            "r1 rel score should reference r1"
        );
        assert!(
            r1_eq.contains("loop_score⁚b1"),
            "r1 rel score should reference b1 (same partition)"
        );
        assert!(
            !r1_eq.contains("loop_score⁚r2"),
            "r1 rel score should NOT reference r2 (different partition)"
        );
        assert!(
            !r1_eq.contains("loop_score⁚b2"),
            "r1 rel score should NOT reference b2 (different partition)"
        );

        // L3 (r2) relative score should reference r2 and b2, NOT r1 or b1
        let r2_rel = vars
            .get(&Ident::new("$⁚ltm⁚rel_loop_score⁚r2"))
            .expect("should have r2 relative score");
        let r2_eq = match r2_rel {
            datamodel::Variable::Aux(aux) => match &aux.equation {
                Equation::Scalar(eq) => eq.clone(),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux variable"),
        };
        assert!(
            r2_eq.contains("loop_score⁚r2"),
            "r2 rel score should reference r2"
        );
        assert!(
            r2_eq.contains("loop_score⁚b2"),
            "r2 rel score should reference b2 (same partition)"
        );
        assert!(
            !r2_eq.contains("loop_score⁚r1"),
            "r2 rel score should NOT reference r1 (different partition)"
        );
    }

    #[test]
    fn test_relative_score_unpartitioned_loops() {
        use crate::ltm::{CyclePartitions, LoopPolarity};

        // Mix of partitioned and unpartitioned loops
        let loops = vec![
            Loop {
                id: "r1".to_string(),
                links: vec![],
                stocks: vec![Ident::new("stock_a")],
                polarity: LoopPolarity::Reinforcing,
            },
            Loop {
                id: "u1".to_string(),
                links: vec![],
                stocks: vec![], // unpartitioned (cross-module loop)
                polarity: LoopPolarity::Undetermined,
            },
            Loop {
                id: "u2".to_string(),
                links: vec![],
                stocks: vec![], // unpartitioned
                polarity: LoopPolarity::Undetermined,
            },
        ];

        let partitions = CyclePartitions {
            partitions: vec![vec![Ident::new("stock_a")]],
            stock_partition: vec![(Ident::new("stock_a"), 0)].into_iter().collect(),
        };

        let vars = generate_loop_score_variables(&loops, &partitions);

        // r1 relative score: only references r1
        let r1_eq = match &vars[&Ident::new("$⁚ltm⁚rel_loop_score⁚r1")] {
            datamodel::Variable::Aux(aux) => match &aux.equation {
                Equation::Scalar(eq) => eq.clone(),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux variable"),
        };
        assert!(
            !r1_eq.contains("loop_score⁚u1"),
            "r1 should not reference unpartitioned u1"
        );

        // u1 relative score: references u1 and u2, not r1
        let u1_eq = match &vars[&Ident::new("$⁚ltm⁚rel_loop_score⁚u1")] {
            datamodel::Variable::Aux(aux) => match &aux.equation {
                Equation::Scalar(eq) => eq.clone(),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux variable"),
        };
        assert!(
            u1_eq.contains("loop_score⁚u1"),
            "u1 should reference itself"
        );
        assert!(
            u1_eq.contains("loop_score⁚u2"),
            "u1 should reference u2 (same unpartitioned group)"
        );
        assert!(
            !u1_eq.contains("loop_score⁚r1"),
            "u1 should NOT reference r1 (different group)"
        );
    }

    #[test]
    fn test_build_partial_equation_excludes_module_output_ref() {
        // When `exclude` is a normalized module node, interpunct refs that
        // normalize to the same module should also be excluded from PREVIOUS
        // wrapping (they represent the same causal influence).
        let mut deps = HashSet::new();
        deps.insert(Ident::new("$⁚s⁚0⁚smth1\u{00B7}output"));
        deps.insert(Ident::new("other"));

        let exclude = Ident::new("$⁚s⁚0⁚smth1");

        // Equation text uses the quoted interpunct ref as it would appear
        // after expr2_to_string round-trips the post-expansion AST.
        let eq_text = r#""$⁚s⁚0⁚smth1·output" * 0.5 + other * 0.5"#;

        let partial = build_partial_equation(eq_text, &deps, &exclude);

        assert!(
            partial.contains("PREVIOUS(other)"),
            "other should be wrapped in PREVIOUS, got: {partial}"
        );
        assert!(
            !partial.contains("PREVIOUS(\"$"),
            "module output ref should NOT be wrapped in PREVIOUS, got: {partial}"
        );
    }

    #[test]
    fn test_module_to_downstream_uses_ceteris_paribus() {
        // When a module output feeds into a downstream variable alongside
        // other inputs, the link score should use the ceteris-paribus
        // (SAFEDIV) formula, not the black-box ABS(ΔTo/ΔTo) formula.
        use crate::ast::{Ast, Expr2};
        use crate::builtins::Loc;
        use crate::ltm::{Link, LinkPolarity};

        let mut variables = HashMap::new();

        let module_var = Variable::Module {
            ident: Ident::new("$⁚combined⁚0⁚smth1"),
            model_name: Ident::new("stdlib⁚smth1"),
            units: None,
            inputs: vec![],
            errors: vec![],
            unit_errors: vec![],
        };
        variables.insert(Ident::new("$⁚combined⁚0⁚smth1"), module_var);

        // Build a downstream variable whose AST references the module output
        // plus another input: module·output * 0.5 + other * 0.5
        let module_output_ref = Ident::new("$⁚combined⁚0⁚smth1\u{00B7}output");
        let ast = Ast::Scalar(Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(Expr2::Op2(
                crate::ast::BinaryOp::Mul,
                Box::new(Expr2::Var(module_output_ref, None, Loc::default())),
                Box::new(Expr2::Const("0.5".to_string(), 0.5, Loc::default())),
                None,
                Loc::default(),
            )),
            Box::new(Expr2::Op2(
                crate::ast::BinaryOp::Mul,
                Box::new(Expr2::Var(Ident::new("other"), None, Loc::default())),
                Box::new(Expr2::Const("0.5".to_string(), 0.5, Loc::default())),
                None,
                Loc::default(),
            )),
            None,
            Loc::default(),
        ));

        let downstream_var = Variable::Var {
            ident: Ident::new("combined"),
            ast: Some(ast),
            init_ast: None,
            eqn: Some(Equation::Scalar(
                "SMTH1(x, 3) * 0.5 + other * 0.5".to_string(),
            )),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };
        variables.insert(Ident::new("combined"), downstream_var);

        let other_var = Variable::Var {
            ident: Ident::new("other"),
            ast: None,
            init_ast: None,
            eqn: Some(Equation::Scalar("TIME * 3".to_string())),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };
        variables.insert(Ident::new("other"), other_var);

        let mut links = HashSet::new();
        links.insert(Link {
            from: Ident::new("$⁚combined⁚0⁚smth1"),
            to: Ident::new("combined"),
            polarity: LinkPolarity::Positive,
        });

        let link_vars = generate_link_score_variables(&links, &variables, &HashMap::new());
        assert_eq!(link_vars.len(), 1);

        let var_name = Ident::new("$⁚ltm⁚link_score⁚$⁚combined⁚0⁚smth1→combined");
        let var = link_vars
            .get(&var_name)
            .expect("should have link score variable");

        let eq = match var {
            datamodel::Variable::Aux(aux) => match &aux.equation {
                datamodel::Equation::Scalar(s) => s.clone(),
                _ => panic!("expected scalar equation"),
            },
            _ => panic!("expected aux variable"),
        };

        assert!(
            eq.contains("SAFEDIV"),
            "module-to-downstream link should use ceteris-paribus formula with SAFEDIV, got: {eq}"
        );
        assert!(
            eq.contains("PREVIOUS(other)"),
            "ceteris-paribus should wrap 'other' in PREVIOUS, got: {eq}"
        );
    }

    #[test]
    fn test_module_link_score_equation_with_dollar_ident_aux_to_aux() {
        // Verify that the aux-to-aux path produces parseable equations when
        // the `from` identifier contains special characters ($ and ⁚).
        use crate::ast::{Ast, Expr2};
        use crate::builtins::Loc;

        let from = Ident::new("$⁚smoothed_level⁚0⁚smth1");

        let module_output_ref = Ident::new("$⁚smoothed_level⁚0⁚smth1\u{00B7}output");
        let ast = Ast::Scalar(Expr2::Var(module_output_ref, None, Loc::default()));

        let to_var = Variable::Var {
            ident: Ident::new("smoothed_level"),
            ast: Some(ast),
            init_ast: None,
            eqn: Some(Equation::Scalar("SMTH1(level, 3)".to_string())),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let to = Ident::new("smoothed_level");
        let eq = generate_auxiliary_to_auxiliary_equation(&from, &to, &to_var);

        let result = crate::ast::Expr0::new(&eq, crate::lexer::LexerType::Equation);
        assert!(
            result.is_ok(),
            "aux-to-aux equation with dollar ident should parse: {eq}\nError: {result:?}"
        );
        let ast = result.unwrap();
        assert!(
            ast.is_some(),
            "aux-to-aux equation with dollar ident should produce an AST: {eq}"
        );
    }
}
