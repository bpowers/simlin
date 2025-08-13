// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loops That Matter (LTM) implementation for loop dominance analysis

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident, Result};
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::{Variable, identifier_set};

/// Represents a causal link between two variables
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub from: Ident<Canonical>,
    pub to: Ident<Canonical>,
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
    /// Reverse edges for efficient traversal
    reverse_edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
    /// Set of stocks in the model
    stocks: HashSet<Ident<Canonical>>,
}

impl CausalGraph {
    /// Build a causal graph from a model
    pub fn from_model(model: &ModelStage1) -> Result<Self> {
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let mut reverse_edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let mut stocks = HashSet::new();

        // Build edges from variable dependencies
        for (var_name, var) in &model.variables {
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
                    .or_insert_with(Vec::new)
                    .push(var_name.clone());

                reverse_edges
                    .entry(var_name.clone())
                    .or_insert_with(Vec::new)
                    .push(dep);
            }

            // For stocks, also add edges from inflows and outflows
            if let Variable::Stock {
                inflows, outflows, ..
            } = var
            {
                for flow in inflows.iter().chain(outflows.iter()) {
                    edges
                        .entry(flow.clone())
                        .or_insert_with(Vec::new)
                        .push(var_name.clone());

                    reverse_edges
                        .entry(var_name.clone())
                        .or_insert_with(Vec::new)
                        .push(flow.clone());
                }
            }
        }

        Ok(CausalGraph {
            edges,
            reverse_edges,
            stocks,
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
                        format!("R{}", loop_id)
                    } else {
                        format!("B{}", loop_id)
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
            links.push(Link {
                from: from.clone(),
                to: to.clone(),
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

    /// Calculate loop polarity based on link polarities
    fn calculate_polarity(&self, _links: &[Link]) -> LoopPolarity {
        // TODO: Implement proper polarity calculation
        // For now, assume all loops are reinforcing
        // This requires analyzing the equations to determine link polarities
        LoopPolarity::Reinforcing
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
            "",
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

        assert_eq!(loops.len(), 1);
        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
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
            "",
            vec![
                x_aux("input", "10", None),
                x_aux("output", "input * 2", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);
        let loops = detect_loops(&project).unwrap();

        assert_eq!(loops.len(), 1);
        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
        let model_loops = &loops[&main_ident];
        assert_eq!(model_loops.len(), 0);
    }
}
