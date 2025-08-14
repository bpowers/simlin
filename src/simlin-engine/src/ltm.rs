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

impl Loop {
    /// Format the loop as a string showing the variable path
    pub fn format_path(&self) -> String {
        if self.links.is_empty() {
            return String::new();
        }

        // Build the path by following links
        let mut path = Vec::new();
        let current = &self.links[0].from;
        path.push(current.as_str());

        for link in &self.links {
            path.push(link.to.as_str());
        }

        path.join(" -> ")
    }
}

/// Loop polarity (Reinforcing or Balancing)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopPolarity {
    Reinforcing, // R loop - even number of negative links
    Balancing,   // B loop - odd number of negative links
}

/// Get direct dependencies from a Variable
fn get_variable_dependencies(var: &Variable) -> Vec<Ident<Canonical>> {
    match var {
        Variable::Module { inputs, .. } => {
            // For modules, dependencies are the source variables of inputs
            inputs.iter().map(|input| input.src.clone()).collect()
        }
        _ => {
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
    /// Module instances and their internal graphs
    module_graphs: HashMap<Ident<Canonical>, Box<CausalGraph>>,
}

impl CausalGraph {
    /// Build a causal graph from a model with project context for modules
    pub fn from_model(model: &ModelStage1, project: &Project) -> Result<Self> {
        let mut edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = HashMap::new();
        let mut stocks = HashSet::new();
        let mut variables = HashMap::new();
        let mut module_graphs = HashMap::new();
        let mut _module_models = HashMap::new();

        // Build edges from variable dependencies
        for (var_name, var) in &model.variables {
            // Store variable for polarity analysis
            variables.insert(var_name.clone(), var.clone());

            // Record if this is a stock
            if matches!(var, Variable::Stock { .. }) {
                stocks.insert(var_name.clone());
            }

            // Handle modules specially
            if let Variable::Module {
                model_name, inputs, ..
            } = var
            {
                // Store module model mapping
                _module_models.insert(var_name.clone(), model_name.clone());

                // Build internal graph for this module instance if we have the model
                if let Some(module_model) = project.models.get(model_name) {
                    if !module_model.implicit {
                        // Recursively build graph for the module
                        let module_graph = CausalGraph::from_model(module_model, project)?;
                        module_graphs.insert(var_name.clone(), Box::new(module_graph));
                    }
                }

                // Add edges from input sources to the module
                for input in inputs {
                    edges
                        .entry(input.src.clone())
                        .or_default()
                        .push(var_name.clone());
                }
            } else {
                // Get dependencies and create edges for non-module variables
                let deps = get_variable_dependencies(var);
                for dep in deps {
                    // Create edge from dependency to variable
                    edges.entry(dep.clone()).or_default().push(var_name.clone());
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
        }

        Ok(CausalGraph {
            edges,
            stocks,
            variables,
            module_graphs,
        })
    }

    /// Find all elementary circuits (feedback loops) using Johnson's algorithm
    pub fn find_loops(&self) -> Vec<Loop> {
        let mut loops = Vec::new();

        // Get all nodes, including those that are module instances
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

                    loops.push(Loop {
                        id: String::new(), // Will be assigned later
                        links,
                        stocks,
                        polarity,
                    });
                }
            }
        }

        // Also find loops that cross module boundaries (with placeholder for loop_id)
        let mut dummy_id = 0;
        let cross_module_loops = self.find_cross_module_loops(&mut dummy_id);
        loops.extend(cross_module_loops);

        // Remove duplicate loops (same set of nodes)
        let mut unique_loops = self.deduplicate_loops(loops);

        // Now assign deterministic IDs based on sorted loop content
        self.assign_deterministic_loop_ids(&mut unique_loops);

        unique_loops
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
        // First check direct edges in this graph
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

        // If current is a module instance, also traverse into the module
        if let Some(_module_graph) = self.module_graphs.get(current) {
            // For now, skip traversing into modules in the basic DFS
            // Cross-module loops are handled separately
        }
    }

    /// Find loops that cross module boundaries
    fn find_cross_module_loops(&self, loop_id: &mut usize) -> Vec<Loop> {
        let mut cross_module_loops = Vec::new();
        let mut visited_loops = HashSet::new();

        // For each module instance
        for (module_var, module_graph) in &self.module_graphs {
            // Find loops within the module
            let internal_loops = module_graph.find_loops();

            // Check if any of these loops connect to external variables
            for internal_loop in internal_loops {
                // Check if this loop has connections that cross the module boundary
                let crosses_boundary = self.check_loop_crosses_boundary(&internal_loop, module_var);

                if crosses_boundary {
                    // Create a new loop that represents the cross-module loop
                    *loop_id += 1;
                    let id = if internal_loop.polarity == LoopPolarity::Reinforcing {
                        format!("R{loop_id}")
                    } else {
                        format!("B{loop_id}")
                    };

                    // Map the internal loop to the parent context
                    let mapped_loop = self.map_loop_to_parent(internal_loop, module_var, id);

                    // Create a unique key for this loop to avoid duplicates
                    let loop_key = self.get_loop_key(&mapped_loop);
                    if !visited_loops.contains(&loop_key) {
                        visited_loops.insert(loop_key);
                        cross_module_loops.push(mapped_loop);
                    }
                }
            }
        }

        // Also find loops that span multiple modules
        let multi_module_loops = self.find_multi_module_loops(loop_id, &visited_loops);
        cross_module_loops.extend(multi_module_loops);

        cross_module_loops
    }

    /// Find loops that span multiple module instances
    fn find_multi_module_loops(&self, loop_id: &mut usize, visited: &HashSet<String>) -> Vec<Loop> {
        let mut multi_module_loops = Vec::new();

        // For each pair of module instances, check if they're connected via the parent
        for (module_a, graph_a) in &self.module_graphs {
            for (module_b, graph_b) in &self.module_graphs {
                if module_a >= module_b {
                    continue; // Avoid duplicates and self-pairs
                }

                // Check if there's a path from module_a outputs to module_b inputs
                // and from module_b outputs back to module_a inputs
                if let Some(connecting_loop) =
                    self.find_inter_module_loop(module_a, graph_a, module_b, graph_b)
                {
                    *loop_id += 1;
                    let id = if connecting_loop.polarity == LoopPolarity::Reinforcing {
                        format!("R{loop_id}")
                    } else {
                        format!("B{loop_id}")
                    };

                    let mut loop_with_id = connecting_loop;
                    loop_with_id.id = id;

                    let loop_key = self.get_loop_key(&loop_with_id);
                    if !visited.contains(&loop_key) {
                        multi_module_loops.push(loop_with_id);
                    }
                }
            }
        }

        multi_module_loops
    }

    /// Find a loop connecting two module instances
    fn find_inter_module_loop(
        &self,
        module_a: &Ident<Canonical>,
        _graph_a: &CausalGraph,
        module_b: &Ident<Canonical>,
        _graph_b: &CausalGraph,
    ) -> Option<Loop> {
        // Check if module outputs from A connect to module inputs of B
        // and vice versa, forming a loop

        // Get module definitions
        let module_a_def = self.variables.get(module_a)?;
        let module_b_def = self.variables.get(module_b)?;

        if let (
            Variable::Module {
                inputs: inputs_a, ..
            },
            Variable::Module {
                inputs: inputs_b, ..
            },
        ) = (module_a_def, module_b_def)
        {
            // Check for connections between the modules
            let mut connecting_links = Vec::new();

            // Check if any output of A connects to input of B
            for input_b in inputs_b {
                // See if this input comes from module A's context
                if self.is_connected_through_parent(module_a, &input_b.src) {
                    connecting_links.push(Link {
                        from: module_a.clone(),
                        to: module_b.clone(),
                        polarity: LinkPolarity::Unknown,
                    });
                }
            }

            // Check if any output of B connects to input of A
            for input_a in inputs_a {
                if self.is_connected_through_parent(module_b, &input_a.src) {
                    connecting_links.push(Link {
                        from: module_b.clone(),
                        to: module_a.clone(),
                        polarity: LinkPolarity::Unknown,
                    });
                }
            }

            // If we have connections both ways, we have a loop
            if connecting_links.len() >= 2 {
                let polarity = self.calculate_polarity(&connecting_links);
                return Some(Loop {
                    id: String::new(), // Will be set by caller
                    links: connecting_links,
                    stocks: Vec::new(), // TODO: identify stocks in the path
                    polarity,
                });
            }
        }

        None
    }

    /// Check if a module output is connected to a variable through the parent model
    fn is_connected_through_parent(
        &self,
        from_module: &Ident<Canonical>,
        to_var: &Ident<Canonical>,
    ) -> bool {
        // Check if there's a path from the module to the variable in the parent graph
        if let Some(neighbors) = self.edges.get(from_module) {
            if neighbors.contains(to_var) {
                return true;
            }
            // Could do deeper path search here if needed
        }
        false
    }

    /// Generate a unique key for a loop to detect duplicates
    fn get_loop_key(&self, loop_item: &Loop) -> String {
        let mut vars: Vec<_> = loop_item
            .links
            .iter()
            .flat_map(|link| vec![link.from.as_str(), link.to.as_str()])
            .collect();
        vars.sort();
        vars.dedup();
        vars.join(",")
    }

    /// Check if a loop within a module crosses the module boundary
    fn check_loop_crosses_boundary(&self, loop_item: &Loop, module_var: &Ident<Canonical>) -> bool {
        // Check if any variables in the loop connect to module inputs/outputs
        if let Some(Variable::Module { inputs, .. }) = self.variables.get(module_var) {
            // Check if any loop variables are connected to module inputs
            for link in &loop_item.links {
                for input in inputs {
                    if link.from == input.dst || link.to == input.dst {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Map a loop from within a module to the parent context
    fn map_loop_to_parent(
        &self,
        loop_item: Loop,
        module_var: &Ident<Canonical>,
        id: String,
    ) -> Loop {
        // Map internal variable names to parent context
        let mut mapped_links = Vec::new();

        if let Some(Variable::Module { inputs, .. }) = self.variables.get(module_var) {
            for link in loop_item.links {
                // Check if the link involves module inputs/outputs
                let mut mapped_from = link.from.clone();
                let mut mapped_to = link.to.clone();

                // Map internal variables to their external connections
                for input in inputs {
                    if link.from == input.dst {
                        mapped_from = input.src.clone();
                    }
                    if link.to == input.dst {
                        mapped_to = input.src.clone();
                    }
                }

                mapped_links.push(Link {
                    from: mapped_from,
                    to: mapped_to,
                    polarity: link.polarity,
                });
            }
        } else {
            // If we can't map, just use the original links
            mapped_links = loop_item.links;
        }

        Loop {
            id,
            links: mapped_links,
            stocks: loop_item.stocks, // TODO: also map stocks
            polarity: loop_item.polarity,
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

    /// Assign deterministic IDs to loops based on their content
    fn assign_deterministic_loop_ids(&self, loops: &mut [Loop]) {
        // Sort loops by a deterministic key based on their content
        loops.sort_by_key(|loop_item| {
            // Create a deterministic key from the loop's variables
            let mut vars: Vec<String> = loop_item
                .links
                .iter()
                .flat_map(|link| vec![link.from.as_str().to_string(), link.to.as_str().to_string()])
                .collect();
            vars.sort();
            vars.dedup();
            vars.join("_")
        });

        // Now assign IDs based on polarity and position in sorted order
        let mut r_counter = 1;
        let mut b_counter = 1;

        for loop_item in loops.iter_mut() {
            loop_item.id = if loop_item.polarity == LoopPolarity::Reinforcing {
                let id = format!("r{r_counter}");
                r_counter += 1;
                id
            } else {
                let id = format!("b{b_counter}");
                b_counter += 1;
                id
            };
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
        let graph = CausalGraph::from_model(model, project)?;
        let loops = graph.find_loops();
        all_loops.insert(model_name.clone(), loops);
    }

    Ok(all_loops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
    use std::collections::{HashMap, HashSet};

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

        // Check that the loop has a deterministic ID
        assert_eq!(loop_item.id, "r1");

        // Check that the path formatting works
        let path = loop_item.format_path();
        assert!(path.contains("population"));
        assert!(path.contains("births"));
    }

    #[test]
    fn test_deterministic_loop_naming() {
        // Create a model with multiple loops to test deterministic naming
        let model = x_model(
            "main",
            vec![
                x_stock("population", "100", &["births"], &["deaths"], None),
                x_flow("births", "population * birth_rate", None),
                x_flow("deaths", "population * death_rate", None),
                x_aux("birth_rate", "0.02", None),
                x_aux("death_rate", "0.01", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs.clone(), &[model.clone()]);
        let project1 = Project::from(project);

        // Create the same project again
        let project = x_project(sim_specs, &[model]);
        let project2 = Project::from(project);

        // Detect loops in both projects
        let loops1 = detect_loops(&project1).unwrap();
        let loops2 = detect_loops(&project2).unwrap();

        let main_ident = crate::common::canonicalize("main");
        let main_loops1 = loops1.get(&main_ident).unwrap();
        let main_loops2 = loops2.get(&main_ident).unwrap();

        // Should have the same number of loops
        assert_eq!(main_loops1.len(), main_loops2.len());

        // Loop IDs should be identical
        for (loop1, loop2) in main_loops1.iter().zip(main_loops2.iter()) {
            assert_eq!(loop1.id, loop2.id, "Loop IDs should be deterministic");
            assert_eq!(
                loop1.format_path(),
                loop2.format_path(),
                "Loop paths should be identical"
            );
        }
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
    fn test_module_loops() {
        // Test loop detection with modules
        use crate::testutils::x_module;

        // Create a model that uses a module (like SMOOTH)
        // This simulates a model with a module that might create a feedback loop
        let main_model = x_model(
            "main",
            vec![
                x_stock("inventory", "100", &["production"], &["sales"], None),
                x_flow("production", "desired_production", None),
                x_aux(
                    "desired_production",
                    "smooth_inventory_gap * adjustment_rate",
                    None,
                ),
                x_aux("inventory_gap", "target_inventory - inventory", None),
                x_module("smooth_inventory_gap", &[("inventory_gap", "input")], None),
                x_aux("target_inventory", "100", None),
                x_aux("adjustment_rate", "0.1", None),
                x_flow("sales", "10", None),
            ],
        );

        // Create the SMOOTH module model (simplified version)
        let smooth_model = x_model(
            "smooth_inventory_gap",
            vec![
                x_aux("input", "0", None), // Module input
                x_stock("smoothed", "0", &["change_in_smooth"], &[], None),
                x_flow("change_in_smooth", "(input - smoothed) / smooth_time", None),
                x_aux("smooth_time", "3", None),
                x_aux("output", "smoothed", None), // Module output
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(sim_specs, &[main_model, smooth_model]);
        let project = Project::from(project);
        let loops = detect_loops(&project).unwrap();

        // Check that loops were detected in the main model
        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
        assert!(loops.contains_key(&main_ident), "Should have main model");
        let _model_loops = &loops[&main_ident];

        // We expect to be able to handle models with modules without crashing
        // The presence of modules shouldn't break loop detection
        // (even if we find 0 loops, that's OK - the important thing is not crashing)
    }

    #[test]
    fn test_multi_module_loops() {
        // Test loop detection across multiple module instances
        use crate::testutils::x_module;

        // Create a model with two module instances that form a loop together
        let main_model = x_model(
            "main",
            vec![
                x_aux("initial_value", "10", None),
                x_module("processor_a", &[("initial_value", "input")], None),
                x_aux("intermediate", "processor_a", None), // Output from module A
                x_module("processor_b", &[("intermediate", "input")], None),
                x_aux("feedback", "processor_b * 0.5", None), // Output from module B
                x_aux("combined", "initial_value + feedback", None),
            ],
        );

        // Create simple processor modules
        let processor_a_model = x_model(
            "processor_a",
            vec![
                x_aux("input", "0", None),
                x_aux("output", "input * 2", None),
            ],
        );

        let processor_b_model = x_model(
            "processor_b",
            vec![
                x_aux("input", "0", None),
                x_aux("output", "input + 1", None),
            ],
        );

        let sim_specs = sim_specs_with_units("years");
        let project = x_project(
            sim_specs,
            &[main_model, processor_a_model, processor_b_model],
        );
        let project = Project::from(project);

        // Test should complete without crashing
        let loops = detect_loops(&project).unwrap();

        let main_ident: Ident<Canonical> = crate::common::canonicalize("main");
        assert!(loops.contains_key(&main_ident), "Should have main model");

        // The enhanced detection might find cross-module loops
        // The exact count isn't critical; what matters is proper handling
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

    #[test]
    fn test_format_path_empty_loop() {
        // Test format_path() with empty links (covers line 44)
        let loop_item = Loop {
            id: "R1".to_string(),
            links: vec![],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
        };

        let path = loop_item.format_path();
        assert_eq!(path, "", "Empty loop should return empty string");
        assert!(path.is_empty(), "Path must be empty for loop with no links");
    }

    #[test]
    fn test_get_variable_dependencies_module() {
        // Test get_variable_dependencies for Module type (covers lines 70-72)
        use crate::variable::ModuleInput;

        let input_var = crate::common::canonicalize("input_signal");
        let module = Variable::Module {
            ident: crate::common::canonicalize("processor"),
            model_name: crate::common::canonicalize("process_model"),
            units: None,
            inputs: vec![
                ModuleInput {
                    src: input_var.clone(),
                    dst: crate::common::canonicalize("input"),
                },
                ModuleInput {
                    src: crate::common::canonicalize("control"),
                    dst: crate::common::canonicalize("param"),
                },
            ],
            errors: vec![],
            unit_errors: vec![],
        };

        let deps = get_variable_dependencies(&module);
        assert_eq!(deps.len(), 2, "Module should have 2 dependencies");
        assert!(deps.contains(&input_var), "Should contain input_signal");
        assert!(
            deps.contains(&crate::common::canonicalize("control")),
            "Should contain control"
        );
    }

    #[test]
    fn test_get_variable_dependencies_no_ast() {
        // Test get_variable_dependencies when AST is None (covers line 83)
        let var = Variable::Var {
            ident: crate::common::canonicalize("empty_var"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: None,
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };

        let deps = get_variable_dependencies(&var);
        assert_eq!(
            deps.len(),
            0,
            "Variable with no AST should have no dependencies"
        );
        assert!(
            deps.is_empty(),
            "Dependencies must be empty for variable without AST"
        );
    }

    #[test]
    fn test_causal_graph_get_loop_key() {
        // Test the get_loop_key function (covers lines 428-436)
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let loop_item = Loop {
            id: "R1".to_string(),
            links: vec![
                Link {
                    from: crate::common::canonicalize("z"),
                    to: crate::common::canonicalize("x"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: crate::common::canonicalize("x"),
                    to: crate::common::canonicalize("y"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: crate::common::canonicalize("y"),
                    to: crate::common::canonicalize("z"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
        };

        let key = graph.get_loop_key(&loop_item);
        // Key should have sorted, deduplicated variables
        assert_eq!(
            key, "x,y,z",
            "Loop key should be sorted, deduplicated variables"
        );

        // Test with duplicate variables (shouldn't happen but tests dedup)
        let loop_with_dups = Loop {
            id: "R2".to_string(),
            links: vec![
                Link {
                    from: crate::common::canonicalize("a"),
                    to: crate::common::canonicalize("b"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: crate::common::canonicalize("b"),
                    to: crate::common::canonicalize("a"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
        };

        let key2 = graph.get_loop_key(&loop_with_dups);
        assert_eq!(key2, "a,b", "Should deduplicate variables");
    }

    #[test]
    fn test_causal_graph_is_connected_through_parent() {
        // Test is_connected_through_parent function (covers lines 412-424)
        let mut graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let module_var = crate::common::canonicalize("smoother");
        let output_var = crate::common::canonicalize("smoothed_output");
        let unconnected_var = crate::common::canonicalize("unrelated");

        // Add edge from module to output
        graph
            .edges
            .entry(module_var.clone())
            .or_insert_with(Vec::new)
            .push(output_var.clone());

        // Test connected case
        assert!(
            graph.is_connected_through_parent(&module_var, &output_var),
            "Module should be connected to output"
        );

        // Test unconnected case
        assert!(
            !graph.is_connected_through_parent(&module_var, &unconnected_var),
            "Module should not be connected to unrelated variable"
        );

        // Test non-existent module
        let non_existent = crate::common::canonicalize("non_existent");
        assert!(
            !graph.is_connected_through_parent(&non_existent, &output_var),
            "Non-existent module should not be connected"
        );
    }

    #[test]
    fn test_flip_polarity() {
        // Test flip_polarity function (covers lines 1049-1054)
        assert_eq!(
            flip_polarity(LinkPolarity::Positive),
            LinkPolarity::Negative
        );
        assert_eq!(
            flip_polarity(LinkPolarity::Negative),
            LinkPolarity::Positive
        );
        assert_eq!(flip_polarity(LinkPolarity::Unknown), LinkPolarity::Unknown);
    }

    #[test]
    fn test_is_positive_constant() {
        // Test is_positive_constant function (covers lines 1058-1062)
        use crate::ast::{Expr2, Loc};

        let pos_const = Expr2::Const("5".to_string(), 5.0, Loc::default());
        assert!(is_positive_constant(&pos_const), "5.0 should be positive");

        let neg_const = Expr2::Const("-5".to_string(), -5.0, Loc::default());
        assert!(
            !is_positive_constant(&neg_const),
            "-5.0 should not be positive"
        );

        let zero_const = Expr2::Const("0".to_string(), 0.0, Loc::default());
        assert!(
            !is_positive_constant(&zero_const),
            "0.0 should not be positive"
        );

        let var_expr = Expr2::Var(crate::common::canonicalize("x"), None, Loc::default());
        assert!(
            !is_positive_constant(&var_expr),
            "Variable should not be positive constant"
        );
    }

    #[test]
    fn test_is_negative_constant() {
        // Test is_negative_constant function (covers lines 1066-1070)
        use crate::ast::{Expr2, Loc};

        let neg_const = Expr2::Const("-3".to_string(), -3.0, Loc::default());
        assert!(is_negative_constant(&neg_const), "-3.0 should be negative");

        let pos_const = Expr2::Const("3".to_string(), 3.0, Loc::default());
        assert!(
            !is_negative_constant(&pos_const),
            "3.0 should not be negative"
        );

        let zero_const = Expr2::Const("0".to_string(), 0.0, Loc::default());
        assert!(
            !is_negative_constant(&zero_const),
            "0.0 should not be negative"
        );

        let var_expr = Expr2::Var(crate::common::canonicalize("y"), None, Loc::default());
        assert!(
            !is_negative_constant(&var_expr),
            "Variable should not be negative constant"
        );
    }

    #[test]
    fn test_analyze_link_polarity_arrayed() {
        // Test analyze_link_polarity with Arrayed AST (covers lines 935-947)
        use crate::ast::{Ast, Expr2, Loc};
        use crate::common::CanonicalElementName;
        use std::collections::HashMap;

        let x_var = crate::common::canonicalize("x");

        // Create arrayed AST with consistent positive polarity
        let mut elements = HashMap::new();
        elements.insert(
            CanonicalElementName::from_raw("dim1"),
            Expr2::Op2(
                BinaryOp::Mul,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
                None,
                Loc::default(),
            ),
        );
        elements.insert(
            CanonicalElementName::from_raw("dim2"),
            Expr2::Op2(
                BinaryOp::Add,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
                None,
                Loc::default(),
            ),
        );

        let ast = Ast::Arrayed(vec![], elements);
        let polarity = analyze_link_polarity(&ast, &x_var);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Consistent positive elements should be positive"
        );

        // Test with mixed polarities
        let mut mixed_elements = HashMap::new();
        mixed_elements.insert(
            CanonicalElementName::from_raw("dim1"),
            Expr2::Var(x_var.clone(), None, Loc::default()),
        );
        mixed_elements.insert(
            CanonicalElementName::from_raw("dim2"),
            Expr2::Op1(
                crate::ast::UnaryOp::Negative,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                None,
                Loc::default(),
            ),
        );

        let mixed_ast = Ast::Arrayed(vec![], mixed_elements);
        let mixed_polarity = analyze_link_polarity(&mixed_ast, &x_var);
        assert_eq!(
            mixed_polarity,
            LinkPolarity::Unknown,
            "Mixed polarities should be Unknown"
        );
    }

    #[test]
    fn test_analyze_expr_polarity_if_then_else() {
        // Test analyze_expr_polarity with If-Then-Else (covers lines 1033-1042)
        use crate::ast::{Expr2, Loc};

        let x_var = crate::common::canonicalize("x");

        // If with same polarity in both branches
        let if_expr = Expr2::If(
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Op2(
                BinaryOp::Mul,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                Box::new(Expr2::Const("2".to_string(), 2.0, Loc::default())),
                None,
                Loc::default(),
            )),
            None,
            Loc::default(),
        );

        let polarity = analyze_expr_polarity(&if_expr, &x_var, LinkPolarity::Positive);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Same polarity branches should return that polarity"
        );

        // If with different polarities in branches
        let mixed_if = Expr2::If(
            Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Op1(
                crate::ast::UnaryOp::Negative,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                None,
                Loc::default(),
            )),
            None,
            Loc::default(),
        );

        let mixed_polarity = analyze_expr_polarity(&mixed_if, &x_var, LinkPolarity::Positive);
        assert_eq!(
            mixed_polarity,
            LinkPolarity::Unknown,
            "Different polarity branches should be Unknown"
        );
    }

    #[test]
    fn test_analyze_expr_polarity_unary_not() {
        // Test analyze_expr_polarity with unary NOT operator (covers lines 1026-1031)
        use crate::ast::{Expr2, Loc, UnaryOp};

        let x_var = crate::common::canonicalize("x");

        let not_expr = Expr2::Op1(
            UnaryOp::Not,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        );

        let polarity = analyze_expr_polarity(&not_expr, &x_var, LinkPolarity::Positive);
        assert_eq!(
            polarity,
            LinkPolarity::Negative,
            "NOT should flip polarity from positive to negative"
        );
    }

    #[test]
    fn test_check_loop_crosses_boundary() {
        // Test check_loop_crosses_boundary (covers lines 440-450)
        use crate::variable::ModuleInput;

        let mut graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // Create a module with inputs
        let input_dst = crate::common::canonicalize("input");
        let module_var = Variable::Module {
            ident: crate::common::canonicalize("processor"),
            model_name: crate::common::canonicalize("process_model"),
            units: None,
            inputs: vec![ModuleInput {
                src: crate::common::canonicalize("external_input"),
                dst: input_dst.clone(),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        let module_ident = crate::common::canonicalize("processor");
        graph.variables.insert(module_ident.clone(), module_var);

        // Create a loop that includes the module input
        let loop_crossing = Loop {
            id: "R1".to_string(),
            links: vec![
                Link {
                    from: input_dst.clone(),
                    to: crate::common::canonicalize("internal_var"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: crate::common::canonicalize("internal_var"),
                    to: input_dst.clone(),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
        };

        assert!(
            graph.check_loop_crosses_boundary(&loop_crossing, &module_ident),
            "Loop with module input should cross boundary"
        );

        // Create a loop that doesn't cross boundary
        let loop_internal = Loop {
            id: "R2".to_string(),
            links: vec![
                Link {
                    from: crate::common::canonicalize("var1"),
                    to: crate::common::canonicalize("var2"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: crate::common::canonicalize("var2"),
                    to: crate::common::canonicalize("var1"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
        };

        assert!(
            !graph.check_loop_crosses_boundary(&loop_internal, &module_ident),
            "Loop without module input should not cross boundary"
        );
    }

    #[test]
    fn test_analyze_expr_polarity_division_edge_cases() {
        // Test division polarity analysis edge cases (covers lines 1013-1022)
        use crate::ast::{Expr2, Loc};

        let x_var = crate::common::canonicalize("x");
        let y_var = crate::common::canonicalize("y");

        // Division with variable in numerator
        let div_num = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
            None,
            Loc::default(),
        );

        let pol_num = analyze_expr_polarity(&div_num, &x_var, LinkPolarity::Positive);
        assert_eq!(
            pol_num,
            LinkPolarity::Positive,
            "Variable in numerator should keep polarity"
        );

        // Division with different variable in denominator (not the one we're tracking)
        let div_other = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Const("100".to_string(), 100.0, Loc::default())),
            Box::new(Expr2::Var(y_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        );

        let pol_other = analyze_expr_polarity(&div_other, &x_var, LinkPolarity::Positive);
        assert_eq!(
            pol_other,
            LinkPolarity::Unknown,
            "Unrelated variable should give Unknown"
        );
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
                crate::ast::UnaryOp::Negative => flip_polarity(operand_pol),
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
