// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loops That Matter (LTM) implementation for loop dominance analysis

use std::collections::{HashMap, HashSet};

use crate::ast::{Ast, BinaryOp, Expr2};
use crate::common::{Canonical, Ident, Result};
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::{Variable, identifier_set};

/// Polarity of a causal link
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkPolarity {
    Positive, // Increase in 'from' causes increase in 'to'
    Negative, // Increase in 'from' causes decrease in 'to'
    Unknown,  // Cannot determine polarity statically
}

/// Represents a causal link between two variables
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub from: Ident<Canonical>,
    pub to: Ident<Canonical>,
    pub polarity: LinkPolarity,
}

/// Represents a feedback loop
#[derive(Debug, Clone)]
pub struct Loop {
    pub id: String,
    pub links: Vec<Link>,
    pub stocks: Vec<Ident<Canonical>>,
    pub polarity: LoopPolarity,
}

/// Loop polarity (Reinforcing or Balancing)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopPolarity {
    Reinforcing, // R loop - even number of negative links
    Balancing,   // B loop - odd number of negative links
}

/// Get direct dependencies from a Variable
fn get_variable_dependencies(var: &Variable) -> Vec<Ident<Canonical>> {
    // Get the main equation AST
    let ast = var.ast();

    match ast {
        Some(ast) => {
            // We don't have dimensions info here, so pass empty vec
            // We also don't have module inputs, so pass None
            identifier_set(ast, &[], None).into_iter().collect()
        }
        None => vec![],
    }
}

/// Graph representation for loop detection
pub struct CausalGraph {
    /// Adjacency list representation
    edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
    /// Set of stocks in the model
    stocks: HashSet<Ident<Canonical>>,
    /// Variables in the model for polarity analysis
    variables: HashMap<Ident<Canonical>, Variable>,
}

impl CausalGraph {
    /// Build a causal graph from a model
    pub fn from_model(model: &ModelStage1) -> Result<Self> {
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let mut stocks = HashSet::new();
        let mut variables = HashMap::new();

        // Build edges from variable dependencies
        for (var_name, var) in &model.variables {
            // Store variable for polarity analysis
            variables.insert(var_name.clone(), var.clone());

            // Record if this is a stock
            if matches!(var, Variable::Stock { .. }) {
                stocks.insert(var_name.clone());
            }

            // Get dependencies and create edges
            let deps = get_variable_dependencies(var);
            for dep in deps {
                // Create edge from dependency to variable
                edges
                    .entry(dep.clone())
                    .or_default()
                    .push(var_name.clone());
            }

            // For stocks, also add edges from inflows and outflows
            if let Variable::Stock {
                inflows, outflows, ..
            } = var
            {
                for flow in inflows.iter().chain(outflows.iter()) {
                    edges
                        .entry(flow.clone())
                        .or_default()
                        .push(var_name.clone());
                }
            }
        }

        Ok(CausalGraph {
            edges,
            stocks,
            variables,
        })
    }

    /// Find all elementary circuits (feedback loops) using Johnson's algorithm
    pub fn find_loops(&self) -> Vec<Loop> {
        let mut loops = Vec::new();
        let mut loop_id = 0;

        // Get all nodes
        let mut nodes: Vec<_> = self.edges.keys().cloned().collect();
        nodes.sort_by(|a, b| a.as_str().cmp(b.as_str())); // Stable ordering

        // Johnson's algorithm for finding elementary circuits
        for start_node in &nodes {
            let circuits = self.find_circuits_from(start_node);
            for circuit in circuits {
                if circuit.len() > 1 {
                    // Ignore self-loops for now
                    let links = self.circuit_to_links(&circuit);
                    let stocks = self.find_stocks_in_loop(&circuit);
                    let polarity = self.calculate_polarity(&links);

                    loop_id += 1;
                    let id = if polarity == LoopPolarity::Reinforcing {
                        format!("R{loop_id}")
                    } else {
                        format!("B{loop_id}")
                    };

                    loops.push(Loop {
                        id,
                        links,
                        stocks,
                        polarity,
                    });
                }
            }
        }

        // Remove duplicate loops (same set of nodes)
        self.deduplicate_loops(loops)
    }

    /// Find all circuits starting from a given node using DFS
    fn find_circuits_from(&self, start: &Ident<Canonical>) -> Vec<Vec<Ident<Canonical>>> {
        let mut circuits = Vec::new();
        let mut path = vec![start.clone()];
        let mut visited = HashSet::new();
        visited.insert(start.clone());

        self.dfs_circuits(start, start, &mut path, &mut visited, &mut circuits);

        circuits
    }

    /// DFS helper for finding circuits
    fn dfs_circuits(
        &self,
        start: &Ident<Canonical>,
        current: &Ident<Canonical>,
        path: &mut Vec<Ident<Canonical>>,
        visited: &mut HashSet<Ident<Canonical>>,
        circuits: &mut Vec<Vec<Ident<Canonical>>>,
    ) {
        if let Some(neighbors) = self.edges.get(current) {
            for neighbor in neighbors {
                if neighbor == start && path.len() > 1 {
                    // Found a circuit back to start
                    circuits.push(path.clone());
                } else if !visited.contains(neighbor) && neighbor.as_str() >= start.as_str() {
                    // Only visit nodes that come after start (to avoid duplicates)
                    visited.insert(neighbor.clone());
                    path.push(neighbor.clone());
                    self.dfs_circuits(start, neighbor, path, visited, circuits);
                    path.pop();
                    visited.remove(neighbor);
                }
            }
        }
    }

    /// Convert a circuit (list of nodes) to a list of links
    fn circuit_to_links(&self, circuit: &[Ident<Canonical>]) -> Vec<Link> {
        let mut links = Vec::new();
        for i in 0..circuit.len() {
            let from = &circuit[i];
            let to = &circuit[(i + 1) % circuit.len()];
            let polarity = self.get_link_polarity(from, to);
            links.push(Link {
                from: from.clone(),
                to: to.clone(),
                polarity,
            });
        }
        links
    }

    /// Find stocks in a loop
    fn find_stocks_in_loop(&self, circuit: &[Ident<Canonical>]) -> Vec<Ident<Canonical>> {
        circuit
            .iter()
            .filter(|node| self.stocks.contains(*node))
            .cloned()
            .collect()
    }

    /// Get the polarity of a single link
    fn get_link_polarity(&self, from: &Ident<Canonical>, to: &Ident<Canonical>) -> LinkPolarity {
        // Get the equation of the 'to' variable
        if let Some(to_var) = self.variables.get(to) {
            if let Some(ast) = to_var.ast() {
                // Analyze how 'from' appears in the equation
                return analyze_link_polarity(ast, from);
            }
        }
        LinkPolarity::Unknown
    }

    /// Calculate loop polarity based on link polarities
    fn calculate_polarity(&self, links: &[Link]) -> LoopPolarity {
        // Count negative links
        let negative_count = links
            .iter()
            .filter(|link| link.polarity == LinkPolarity::Negative)
            .count();

        // Even number of negative links = Reinforcing
        // Odd number of negative links = Balancing
        if negative_count % 2 == 0 {
            LoopPolarity::Reinforcing
        } else {
            LoopPolarity::Balancing
        }
    }

    /// Remove duplicate loops (same set of nodes in different order)
    fn deduplicate_loops(&self, loops: Vec<Loop>) -> Vec<Loop> {
        let mut unique_loops = Vec::new();
        let mut seen_sets = HashSet::new();

        for loop_item in loops {
            let mut node_set: Vec<_> = loop_item
                .links
                .iter()
                .map(|link| link.from.as_str())
                .collect();
            node_set.sort();
            let key = node_set.join(",");

            if !seen_sets.contains(&key) {
                seen_sets.insert(key);
                unique_loops.push(loop_item);
            }
        }

        unique_loops
    }
}

/// Detect all loops in a project
pub fn detect_loops(project: &Project) -> Result<HashMap<Ident<Canonical>, Vec<Loop>>> {
    let mut all_loops = HashMap::new();

    for (model_name, model) in &project.models {
        // Skip implicit models (from stdlib)
        if model.implicit {
            continue;
        }
        let graph = CausalGraph::from_model(model)?;
        let loops = graph.find_loops();
        all_loops.insert(model_name.clone(), loops);
    }

    Ok(all_loops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

    #[test]
    fn test_simple_reinforcing_loop() {
        // Create a simple reinforcing loop: population -> births -> population
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
        let loops = detect_loops(&project).unwrap();

        // The model name should match what we provided to x_model
        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
        assert!(loops.contains_key(&main_ident), "Should have main model");
        let model_loops = &loops[&main_ident];
        assert_eq!(model_loops.len(), 1);

        let loop_item = &model_loops[0];
        assert_eq!(loop_item.links.len(), 2);
        assert_eq!(loop_item.stocks.len(), 1);
        assert_eq!(loop_item.stocks[0].as_str(), "population");
    }

    #[test]
    fn test_no_loops() {
        // Create a model with no loops
        let model = x_model(
            "main",
            vec![
                x_aux("input", "10", None),
                x_aux("output", "input * 2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);
        let loops = detect_loops(&project).unwrap();

        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
        assert!(loops.contains_key(&main_ident), "Should have main model");
        let model_loops = &loops[&main_ident];
        assert_eq!(model_loops.len(), 0);
    }

    #[test]
    fn test_balancing_loop() {
        // Create a balancing loop: goal -> gap -> adjustment -> level -> gap
        // gap = goal - level (negative link from level to gap)
        let model = x_model(
            "main",
            vec![
                x_stock("level", "100", &["adjustment"], &[], None),
                x_flow("adjustment", "gap / adjustment_time", None),
                x_aux("gap", "goal - level", None),
                x_aux("goal", "200", None),
                x_aux("adjustment_time", "5", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);
        let loops = detect_loops(&project).unwrap();

        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
        assert!(loops.contains_key(&main_ident), "Should have main model");
        let model_loops = &loops[&main_ident];

        // Should find the balancing loop
        assert!(model_loops.len() > 0);

        // Check that at least one loop is balancing
        let has_balancing = model_loops
            .iter()
            .any(|loop_item| loop_item.polarity == LoopPolarity::Balancing);
        assert!(has_balancing, "Should have detected a balancing loop");
    }

    #[test]
    fn test_link_polarity_detection() {
        // Test polarity detection in simple expressions
        use crate::ast::{Ast, Expr2};
        use crate::common::canonicalize;

        // Test positive link: y = x * 2
        let x_var = canonicalize("x");
        let expr = Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
            Box::new(Expr2::Const(
                "2".to_string(),
                2.0,
                crate::ast::Loc::default(),
            )),
            None,
            crate::ast::Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var);
        assert_eq!(polarity, LinkPolarity::Positive);

        // Test negative link: y = -x
        let expr = Expr2::Op2(
            BinaryOp::Sub,
            Box::new(Expr2::Const(
                "0".to_string(),
                0.0,
                crate::ast::Loc::default(),
            )),
            Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
            None,
            crate::ast::Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var);
        assert_eq!(polarity, LinkPolarity::Negative);

        // Test negative link via multiplication: y = x * -3
        let expr = Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(x_var.clone(), None, crate::ast::Loc::default())),
            Box::new(Expr2::Const(
                "-3".to_string(),
                -3.0,
                crate::ast::Loc::default(),
            )),
            None,
            crate::ast::Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var);
        assert_eq!(polarity, LinkPolarity::Negative);
    }
}

/// Analyze the polarity of how a variable appears in an equation
fn analyze_link_polarity(ast: &Ast<Expr2>, from_var: &Ident<Canonical>) -> LinkPolarity {
    match ast {
        Ast::Scalar(expr) => analyze_expr_polarity(expr, from_var, LinkPolarity::Positive),
        Ast::ApplyToAll(_, expr) => analyze_expr_polarity(expr, from_var, LinkPolarity::Positive),
        Ast::Arrayed(_, elements) => {
            // For arrayed equations, check all elements
            let mut polarity = LinkPolarity::Unknown;
            for expr in elements.values() {
                let elem_polarity = analyze_expr_polarity(expr, from_var, LinkPolarity::Positive);
                if polarity == LinkPolarity::Unknown {
                    polarity = elem_polarity;
                } else if polarity != elem_polarity && elem_polarity != LinkPolarity::Unknown {
                    // Mixed polarities
                    return LinkPolarity::Unknown;
                }
            }
            polarity
        }
    }
}

/// Recursively analyze expression polarity
fn analyze_expr_polarity(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
) -> LinkPolarity {
    match expr {
        Expr2::Const(_, _, _) => LinkPolarity::Unknown,
        Expr2::Var(ident, _, _) => {
            if ident == from_var {
                current_polarity
            } else {
                LinkPolarity::Unknown
            }
        }
        Expr2::Op2(op, left, right, _, _) => {
            let left_pol = analyze_expr_polarity(left, from_var, current_polarity);
            let right_pol = analyze_expr_polarity(right, from_var, current_polarity);

            match op {
                BinaryOp::Add => {
                    // Addition preserves polarity
                    if left_pol != LinkPolarity::Unknown {
                        left_pol
                    } else {
                        right_pol
                    }
                }
                BinaryOp::Sub => {
                    // Subtraction flips polarity of right operand
                    if left_pol != LinkPolarity::Unknown {
                        left_pol
                    } else if right_pol != LinkPolarity::Unknown {
                        flip_polarity(right_pol)
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                BinaryOp::Mul => {
                    // Multiplication: need to check sign of other operand
                    // For simplicity, assume positive if constant > 0
                    if left_pol != LinkPolarity::Unknown {
                        if is_positive_constant(right) {
                            left_pol
                        } else if is_negative_constant(right) {
                            flip_polarity(left_pol)
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else if right_pol != LinkPolarity::Unknown {
                        if is_positive_constant(left) {
                            right_pol
                        } else if is_negative_constant(left) {
                            flip_polarity(right_pol)
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                BinaryOp::Div => {
                    // Division by variable in denominator flips polarity
                    if left_pol != LinkPolarity::Unknown {
                        left_pol
                    } else if right_pol != LinkPolarity::Unknown {
                        flip_polarity(right_pol)
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::Op1(op, operand, _, _) => {
            let operand_pol = analyze_expr_polarity(operand, from_var, current_polarity);
            match op {
                crate::ast::UnaryOp::Not => flip_polarity(operand_pol),
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::If(_, true_branch, false_branch, _, _) => {
            // For IF-THEN-ELSE, check both branches
            let true_pol = analyze_expr_polarity(true_branch, from_var, current_polarity);
            let false_pol = analyze_expr_polarity(false_branch, from_var, current_polarity);

            if true_pol == false_pol {
                true_pol
            } else {
                LinkPolarity::Unknown
            }
        }
        _ => LinkPolarity::Unknown,
    }
}

/// Flip the polarity
fn flip_polarity(pol: LinkPolarity) -> LinkPolarity {
    match pol {
        LinkPolarity::Positive => LinkPolarity::Negative,
        LinkPolarity::Negative => LinkPolarity::Positive,
        LinkPolarity::Unknown => LinkPolarity::Unknown,
    }
}

/// Check if expression is a positive constant
fn is_positive_constant(expr: &Expr2) -> bool {
    match expr {
        Expr2::Const(_, n, _) => *n > 0.0,
        _ => false,
    }
}

/// Check if expression is a negative constant
fn is_negative_constant(expr: &Expr2) -> bool {
    match expr {
        Expr2::Const(_, n, _) => *n < 0.0,
        _ => false,
    }
}
