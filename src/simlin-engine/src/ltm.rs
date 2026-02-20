// Copyright 2026 The Simlin Authors. All rights reserved.
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkPolarity {
    Positive, // Increase in 'from' causes increase in 'to'
    Negative, // Increase in 'from' causes decrease in 'to'
    Unknown,  // Cannot determine polarity statically
}

/// Represents a causal link between two variables
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Link {
    pub from: Ident<Canonical>,
    pub to: Ident<Canonical>,
    pub polarity: LinkPolarity,
}

/// Represents a feedback loop
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
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

/// Loop polarity classification
///
/// The structural polarity is determined by counting negative links:
/// - Even number of negative links → Reinforcing
/// - Odd number of negative links → Balancing
/// - ANY link with unknown polarity → Undetermined
///
/// At runtime, if the loop score changes sign during simulation, the polarity
/// is also classified as Undetermined.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub enum LoopPolarity {
    /// R loop - amplifies changes (positive loop score)
    /// Structurally: even number of negative links
    Reinforcing,
    /// B loop - counteracts changes (negative loop score)
    /// Structurally: odd number of negative links
    Balancing,
    /// U loop - polarity cannot be determined or changes during simulation
    /// Structurally: any link has unknown polarity
    /// At runtime: loop score has both positive and negative values
    Undetermined,
}

impl LoopPolarity {
    /// Classify loop polarity based on actual runtime loop score values.
    ///
    /// This function examines the loop score values from a simulation run
    /// and determines the appropriate polarity:
    /// - All valid (non-NaN, non-zero) scores positive → Reinforcing
    /// - All valid scores negative → Balancing
    /// - Mix of positive and negative → Undetermined
    /// - No valid scores → returns None
    pub fn from_runtime_scores(scores: &[f64]) -> Option<Self> {
        let valid_scores: Vec<f64> = scores
            .iter()
            .copied()
            .filter(|v| !v.is_nan() && *v != 0.0)
            .collect();

        if valid_scores.is_empty() {
            return None;
        }

        let has_positive = valid_scores.iter().any(|v| *v > 0.0);
        let has_negative = valid_scores.iter().any(|v| *v < 0.0);

        match (has_positive, has_negative) {
            (true, false) => Some(LoopPolarity::Reinforcing),
            (false, true) => Some(LoopPolarity::Balancing),
            (true, true) => Some(LoopPolarity::Undetermined),
            (false, false) => None, // All zeros after filtering
        }
    }

    /// Returns the conventional single-letter abbreviation for this polarity
    pub fn abbreviation(&self) -> &'static str {
        match self {
            LoopPolarity::Reinforcing => "R",
            LoopPolarity::Balancing => "B",
            LoopPolarity::Undetermined => "U",
        }
    }
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
                // Build internal graph for this module instance if we have the model
                if let Some(module_model) = project.models.get(model_name)
                    && !module_model.implicit
                {
                    // Recursively build graph for the module
                    let module_graph = CausalGraph::from_model(module_model, project)?;
                    module_graphs.insert(var_name.clone(), Box::new(module_graph));
                }

                // Add edges from input sources to the module
                for input in inputs {
                    edges
                        .entry(input.src.clone())
                        .or_default()
                        .push(var_name.clone());
                }
            } else {
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
                } else {
                    // Get dependencies and create edges for flows + auxes.  We don't want to
                    // do this for stocks because get_variable_dependencies() only looks at the
                    // equation for the stock's initial value
                    let deps = get_variable_dependencies(var);
                    for dep in deps {
                        // Create edge from dependency to variable
                        edges.entry(dep.clone()).or_default().push(var_name.clone());
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
        if let Some(neighbors) = self.edges.get(from_module)
            && neighbors.contains(to_var)
        {
            return true;
        }
        // Could do deeper path search here if needed
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
    pub fn circuit_to_links(&self, circuit: &[Ident<Canonical>]) -> Vec<Link> {
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
    pub fn find_stocks_in_loop(&self, circuit: &[Ident<Canonical>]) -> Vec<Ident<Canonical>> {
        circuit
            .iter()
            .filter(|node| self.stocks.contains(*node))
            .cloned()
            .collect()
    }

    /// Return all causal links in the model with their computed polarity.
    ///
    /// This iterates over every edge in the causal graph and builds a `Link`
    /// with polarity determined by static analysis of the equation AST. Used
    /// by discovery mode to generate link score variables for ALL causal
    /// connections, not just those in known loops.
    pub fn all_links(&self) -> Vec<Link> {
        let mut links = Vec::new();
        for (from, targets) in &self.edges {
            for to in targets {
                let polarity = self.get_link_polarity(from, to);
                links.push(Link {
                    from: from.clone(),
                    to: to.clone(),
                    polarity,
                });
            }
        }
        // Sort for deterministic ordering
        links.sort_by(|a, b| {
            a.from
                .as_str()
                .cmp(b.from.as_str())
                .then_with(|| a.to.as_str().cmp(b.to.as_str()))
        });
        links
    }

    /// Get the polarity of a single link
    fn get_link_polarity(&self, from: &Ident<Canonical>, to: &Ident<Canonical>) -> LinkPolarity {
        // Get the 'to' variable
        if let Some(to_var) = self.variables.get(to) {
            // Special case: flow -> stock relationships
            if let Variable::Stock {
                inflows, outflows, ..
            } = to_var
            {
                // Check if 'from' is an inflow (positive) or outflow (negative)
                if inflows.contains(from) {
                    return LinkPolarity::Positive;
                } else if outflows.contains(from) {
                    return LinkPolarity::Negative;
                }
                // If 'from' is not a flow for this stock, fall through to AST analysis
            }

            // General case: analyze the equation AST
            if let Some(ast) = to_var.ast() {
                // Analyze how 'from' appears in the equation
                return analyze_link_polarity(ast, from, &self.variables);
            }
        }
        LinkPolarity::Unknown
    }

    /// Calculate loop polarity based on link polarities
    pub fn calculate_polarity(&self, links: &[Link]) -> LoopPolarity {
        // If ANY link has unknown polarity, the loop is Undetermined
        let has_unknown_polarity = links
            .iter()
            .any(|link| link.polarity == LinkPolarity::Unknown);

        if has_unknown_polarity {
            return LoopPolarity::Undetermined;
        }

        // All links have known polarity - count negative links
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
        let mut u_counter = 1;

        for loop_item in loops.iter_mut() {
            loop_item.id = match loop_item.polarity {
                LoopPolarity::Reinforcing => {
                    let id = format!("r{r_counter}");
                    r_counter += 1;
                    id
                }
                LoopPolarity::Balancing => {
                    let id = format!("b{b_counter}");
                    b_counter += 1;
                    id
                }
                LoopPolarity::Undetermined => {
                    let id = format!("u{u_counter}");
                    u_counter += 1;
                    id
                }
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

/// Detect all feedback loops in a single model
pub fn detect_loops(model: &ModelStage1, project: &Project) -> Result<Vec<Loop>> {
    let graph = CausalGraph::from_model(model, project)?;
    Ok(graph.find_loops())
}

/// Analyze the polarity of how a variable appears in an equation
fn analyze_link_polarity(
    ast: &Ast<Expr2>,
    from_var: &Ident<Canonical>,
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> LinkPolarity {
    match ast {
        Ast::Scalar(expr) => analyze_expr_polarity_with_context(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
        ),
        Ast::ApplyToAll(_, expr) => analyze_expr_polarity_with_context(
            expr,
            from_var,
            LinkPolarity::Positive,
            Some(variables),
        ),
        Ast::Arrayed(_, elements) => {
            // For arrayed equations, check all elements
            let mut polarity = LinkPolarity::Unknown;
            for expr in elements.values() {
                let elem_polarity = analyze_expr_polarity_with_context(
                    expr,
                    from_var,
                    LinkPolarity::Positive,
                    Some(variables),
                );
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

/// Recursively analyze expression polarity with optional context for looking up tables
fn analyze_expr_polarity_with_context(
    expr: &Expr2,
    from_var: &Ident<Canonical>,
    current_polarity: LinkPolarity,
    variables: Option<&HashMap<Ident<Canonical>, Variable>>,
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
        Expr2::App(crate::builtins::BuiltinFn::Lookup(table_expr, index_expr, _), _, _) => {
            // Check if the argument contains our from_var
            let arg_polarity = analyze_expr_polarity_with_context(
                index_expr,
                from_var,
                LinkPolarity::Positive,
                variables,
            );

            if arg_polarity == LinkPolarity::Unknown {
                return LinkPolarity::Unknown;
            }

            // Try to find the table and analyze its monotonicity
            // TODO: support Expr2::Subscript for subscripted lookup tables (per-element gf)
            let table_name = match table_expr.as_ref() {
                Expr2::Var(name, _, _) => Some(name.as_str()),
                _ => None,
            };

            if let (Some(vars), Some(table_name)) = (variables, table_name)
                && let Some(Variable::Var { tables, .. }) =
                    vars.get(&*crate::common::canonicalize(table_name))
                && let Some(t) = tables.first()
            {
                let table_polarity = analyze_graphical_function_polarity(t);
                // Combine the polarities
                return match (arg_polarity, table_polarity) {
                    (LinkPolarity::Positive, LinkPolarity::Positive) => LinkPolarity::Positive,
                    (LinkPolarity::Positive, LinkPolarity::Negative) => LinkPolarity::Negative,
                    (LinkPolarity::Negative, LinkPolarity::Positive) => LinkPolarity::Negative,
                    (LinkPolarity::Negative, LinkPolarity::Negative) => LinkPolarity::Positive,
                    _ => LinkPolarity::Unknown,
                };
            }
            LinkPolarity::Unknown
        }
        // Monotonically increasing single-arg builtins: propagate inner polarity
        Expr2::App(crate::builtins::BuiltinFn::Exp(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Ln(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Log10(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Sqrt(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Arctan(inner), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Int(inner), _, _) => {
            analyze_expr_polarity_with_context(inner, from_var, current_polarity, variables)
        }
        // Max/Min (scalar two-arg form): non-decreasing in each argument
        Expr2::App(crate::builtins::BuiltinFn::Max(a, Some(b)), _, _)
        | Expr2::App(crate::builtins::BuiltinFn::Min(a, Some(b)), _, _) => {
            let pol_a =
                analyze_expr_polarity_with_context(a, from_var, current_polarity, variables);
            let pol_b =
                analyze_expr_polarity_with_context(b, from_var, current_polarity, variables);
            match (pol_a, pol_b) {
                // Only one arg depends on from_var: propagate its polarity
                (LinkPolarity::Unknown, known) => known,
                (known, LinkPolarity::Unknown) => known,
                // Both agree: propagate
                (a_pol, b_pol) if a_pol == b_pol => a_pol,
                // Disagree: unknown
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::App(_, _, _) => LinkPolarity::Unknown,
        Expr2::Op2(op, left, right, _, _) => {
            let left_pol =
                analyze_expr_polarity_with_context(left, from_var, current_polarity, variables);
            let right_pol =
                analyze_expr_polarity_with_context(right, from_var, current_polarity, variables);

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
                    // Multiplication: combine polarities
                    // If both have known polarity, combine them
                    if left_pol != LinkPolarity::Unknown && right_pol != LinkPolarity::Unknown {
                        // Positive * Positive = Positive
                        // Positive * Negative = Negative
                        // Negative * Positive = Negative
                        // Negative * Negative = Positive
                        match (left_pol, right_pol) {
                            (LinkPolarity::Positive, LinkPolarity::Positive) => {
                                LinkPolarity::Positive
                            }
                            (LinkPolarity::Positive, LinkPolarity::Negative) => {
                                LinkPolarity::Negative
                            }
                            (LinkPolarity::Negative, LinkPolarity::Positive) => {
                                LinkPolarity::Negative
                            }
                            (LinkPolarity::Negative, LinkPolarity::Negative) => {
                                LinkPolarity::Positive
                            }
                            _ => LinkPolarity::Unknown,
                        }
                    } else if left_pol != LinkPolarity::Unknown {
                        // Only left has polarity, check if right is a constant or constant-valued variable
                        if is_positive_constant(right)
                            || (variables.is_some()
                                && is_positive_variable(right, variables.unwrap()))
                        {
                            left_pol
                        } else if is_negative_constant(right)
                            || (variables.is_some()
                                && is_negative_variable(right, variables.unwrap()))
                        {
                            flip_polarity(left_pol)
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else if right_pol != LinkPolarity::Unknown {
                        // Only right has polarity, check if left is a constant or constant-valued variable
                        if is_positive_constant(left)
                            || (variables.is_some()
                                && is_positive_variable(left, variables.unwrap()))
                        {
                            right_pol
                        } else if is_negative_constant(left)
                            || (variables.is_some()
                                && is_negative_variable(left, variables.unwrap()))
                        {
                            flip_polarity(right_pol)
                        } else {
                            LinkPolarity::Unknown
                        }
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                BinaryOp::Div => {
                    // Division: numerator preserves polarity, denominator flips polarity
                    if left_pol != LinkPolarity::Unknown {
                        // Variable in numerator preserves polarity
                        left_pol
                    } else if right_pol != LinkPolarity::Unknown {
                        // Variable in denominator flips polarity
                        flip_polarity(right_pol)
                    } else {
                        LinkPolarity::Unknown
                    }
                }
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::Op1(op, operand, _, _) => {
            let operand_pol =
                analyze_expr_polarity_with_context(operand, from_var, current_polarity, variables);
            match op {
                crate::ast::UnaryOp::Not => flip_polarity(operand_pol),
                crate::ast::UnaryOp::Negative => flip_polarity(operand_pol),
                _ => LinkPolarity::Unknown,
            }
        }
        Expr2::If(_, true_branch, false_branch, _, _) => {
            // For IF-THEN-ELSE, check both branches
            let true_pol = analyze_expr_polarity_with_context(
                true_branch,
                from_var,
                current_polarity,
                variables,
            );
            let false_pol = analyze_expr_polarity_with_context(
                false_branch,
                from_var,
                current_polarity,
                variables,
            );

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

/// Check if a variable has a positive constant value
fn is_positive_variable(expr: &Expr2, variables: &HashMap<Ident<Canonical>, Variable>) -> bool {
    if let Expr2::Var(ident, _, _) = expr
        && let Some(var) = variables.get(ident)
        && let Some(Ast::Scalar(var_expr)) = var.ast()
    {
        // Recursively check if the variable's equation is a positive constant
        return is_positive_constant(var_expr);
    }
    false
}

/// Check if a variable has a negative constant value
fn is_negative_variable(expr: &Expr2, variables: &HashMap<Ident<Canonical>, Variable>) -> bool {
    if let Expr2::Var(ident, _, _) = expr
        && let Some(var) = variables.get(ident)
        && let Some(Ast::Scalar(var_expr)) = var.ast()
    {
        // Recursively check if the variable's equation is a negative constant
        return is_negative_constant(var_expr);
    }
    false
}

/// Analyze the polarity of a graphical function/lookup table
/// Returns Positive if monotonically increasing, Negative if monotonically decreasing, Unknown otherwise
fn analyze_graphical_function_polarity(table: &crate::variable::Table) -> LinkPolarity {
    // Need at least 2 points to determine monotonicity
    if table.x.len() < 2 || table.y.len() < 2 {
        return LinkPolarity::Unknown;
    }

    let mut all_increasing = true;
    let mut all_decreasing = true;
    let mut all_constant = true;

    // Check consecutive pairs of points
    for i in 1..table.y.len() {
        let dy = table.y[i] - table.y[i - 1];

        // Use a small epsilon for floating point comparison
        const EPSILON: f64 = 1e-10;

        if dy > EPSILON {
            all_decreasing = false;
            all_constant = false;
        } else if dy < -EPSILON {
            all_increasing = false;
            all_constant = false;
        } else {
            // dy is approximately 0 (within epsilon)
            // This doesn't break monotonicity but isn't strictly increasing/decreasing
        }
    }

    // If all changes are zero (constant function), return Unknown
    if all_constant {
        return LinkPolarity::Unknown;
    }

    // Return polarity based on monotonicity
    if all_increasing {
        LinkPolarity::Positive
    } else if all_decreasing {
        LinkPolarity::Negative
    } else {
        LinkPolarity::Unknown
    }
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
        let main_ident: Ident<Canonical> = Ident::new("main");
        let model = &project.models[&main_ident];
        let model_loops = detect_loops(model, &project).unwrap();
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
        let project = x_project(sim_specs.clone(), std::slice::from_ref(&model));
        let project1 = Project::from(project);

        // Create the same project again
        let project = x_project(sim_specs, &[model]);
        let project2 = Project::from(project);

        let main_ident: Ident<Canonical> = Ident::new("main");
        let main_loops1 = detect_loops(&project1.models[&main_ident], &project1).unwrap();
        let main_loops2 = detect_loops(&project2.models[&main_ident], &project2).unwrap();

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
        let main_ident: Ident<Canonical> = Ident::new("main");
        let model_loops = detect_loops(&project.models[&main_ident], &project).unwrap();
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
        let main_ident: Ident<Canonical> = Ident::new("main");
        let model_loops = detect_loops(&project.models[&main_ident], &project).unwrap();

        // Should find the balancing loop
        assert!(!model_loops.is_empty());

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
        let main_ident: Ident<Canonical> = Ident::new("main");
        let _model_loops = detect_loops(&project.models[&main_ident], &project).unwrap();

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
        let main_ident: Ident<Canonical> = Ident::new("main");
        let _model_loops = detect_loops(&project.models[&main_ident], &project).unwrap();

        // The enhanced detection might find cross-module loops
        // The exact count isn't critical; what matters is proper handling
    }

    #[test]
    fn test_link_polarity_detection() {
        // Test polarity detection in simple expressions
        use crate::ast::{Ast, Expr2};

        // Test positive link: y = x * 2
        let x_var = Ident::new("x");
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
        let empty_vars = HashMap::new();
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
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
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
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
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
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

        let input_var = Ident::new("input_signal");
        let module = Variable::Module {
            ident: Ident::new("processor"),
            model_name: Ident::new("process_model"),
            units: None,
            inputs: vec![
                ModuleInput {
                    src: input_var.clone(),
                    dst: Ident::new("input"),
                },
                ModuleInput {
                    src: Ident::new("control"),
                    dst: Ident::new("param"),
                },
            ],
            errors: vec![],
            unit_errors: vec![],
        };

        let deps = get_variable_dependencies(&module);
        assert_eq!(deps.len(), 2, "Module should have 2 dependencies");
        assert!(deps.contains(&input_var), "Should contain input_signal");
        assert!(
            deps.contains(&Ident::new("control")),
            "Should contain control"
        );
    }

    #[test]
    fn test_get_variable_dependencies_no_ast() {
        // Test get_variable_dependencies when AST is None (covers line 83)
        let var = Variable::Var {
            ident: Ident::new("empty_var"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
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
                    from: Ident::new("z"),
                    to: Ident::new("x"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("x"),
                    to: Ident::new("y"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("y"),
                    to: Ident::new("z"),
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
                    from: Ident::new("a"),
                    to: Ident::new("b"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("b"),
                    to: Ident::new("a"),
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

        let module_var = Ident::new("smoother");
        let output_var = Ident::new("smoothed_output");
        let unconnected_var = Ident::new("unrelated");

        // Add edge from module to output
        graph
            .edges
            .entry(module_var.clone())
            .or_default()
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
        let non_existent = Ident::new("non_existent");
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

        let var_expr = Expr2::Var(Ident::new("x"), None, Loc::default());
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

        let var_expr = Expr2::Var(Ident::new("y"), None, Loc::default());
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

        let x_var = Ident::new("x");

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
        let empty_vars = HashMap::new();
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
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
        let mixed_polarity = analyze_link_polarity(&mixed_ast, &x_var, &empty_vars);
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

        let x_var = Ident::new("x");

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

        let polarity =
            analyze_expr_polarity_with_context(&if_expr, &x_var, LinkPolarity::Positive, None);
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

        let mixed_polarity =
            analyze_expr_polarity_with_context(&mixed_if, &x_var, LinkPolarity::Positive, None);
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

        let x_var = Ident::new("x");

        let not_expr = Expr2::Op1(
            UnaryOp::Not,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            None,
            Loc::default(),
        );

        let polarity =
            analyze_expr_polarity_with_context(&not_expr, &x_var, LinkPolarity::Positive, None);
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
        let input_dst = Ident::new("input");
        let module_var = Variable::Module {
            ident: Ident::new("processor"),
            model_name: Ident::new("process_model"),
            units: None,
            inputs: vec![ModuleInput {
                src: Ident::new("external_input"),
                dst: input_dst.clone(),
            }],
            errors: vec![],
            unit_errors: vec![],
        };

        let module_ident = Ident::new("processor");
        graph.variables.insert(module_ident.clone(), module_var);

        // Create a loop that includes the module input
        let loop_crossing = Loop {
            id: "R1".to_string(),
            links: vec![
                Link {
                    from: input_dst.clone(),
                    to: Ident::new("internal_var"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("internal_var"),
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
                    from: Ident::new("var1"),
                    to: Ident::new("var2"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::new("var2"),
                    to: Ident::new("var1"),
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

        let x_var = Ident::new("x");
        let y_var = Ident::new("y");

        // Division with variable in numerator
        let div_num = Expr2::Op2(
            BinaryOp::Div,
            Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
            Box::new(Expr2::Const("10".to_string(), 10.0, Loc::default())),
            None,
            Loc::default(),
        );

        let pol_num =
            analyze_expr_polarity_with_context(&div_num, &x_var, LinkPolarity::Positive, None);
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

        let pol_other =
            analyze_expr_polarity_with_context(&div_other, &x_var, LinkPolarity::Positive, None);
        assert_eq!(
            pol_other,
            LinkPolarity::Unknown,
            "Unrelated variable should give Unknown"
        );
    }

    #[test]
    fn test_graphical_function_polarity() {
        use crate::variable::Table;

        // Test 1: Monotonically increasing function (positive polarity)
        let increasing_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0, 4.0], vec![0.0, 2.0, 4.0, 6.0, 8.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&increasing_table),
            LinkPolarity::Positive,
            "Monotonically increasing function should have positive polarity"
        );

        // Test 2: Monotonically decreasing function (negative polarity)
        let decreasing_table = Table::new_for_test(
            vec![0.0, 1.0, 2.0, 3.0, 4.0],
            vec![10.0, 8.0, 6.0, 4.0, 2.0],
        );
        assert_eq!(
            analyze_graphical_function_polarity(&decreasing_table),
            LinkPolarity::Negative,
            "Monotonically decreasing function should have negative polarity"
        );

        // Test 3: Non-monotonic function (unknown polarity)
        let non_monotonic_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0, 4.0], vec![0.0, 5.0, 3.0, 7.0, 2.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&non_monotonic_table),
            LinkPolarity::Unknown,
            "Non-monotonic function should have unknown polarity"
        );

        // Test 4: Constant function (unknown polarity - no change)
        let constant_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![5.0, 5.0, 5.0, 5.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&constant_table),
            LinkPolarity::Unknown,
            "Constant function should have unknown polarity"
        );

        // Test 5: Single point (edge case)
        let single_point_table = Table::new_for_test(vec![1.0], vec![2.0]);
        assert_eq!(
            analyze_graphical_function_polarity(&single_point_table),
            LinkPolarity::Unknown,
            "Single point should have unknown polarity"
        );

        // Test 6: Nearly constant with small variations (testing tolerance)
        let nearly_constant_table =
            Table::new_for_test(vec![0.0, 1.0, 2.0, 3.0], vec![5.0, 5.0001, 5.0002, 5.0003]);
        assert_eq!(
            analyze_graphical_function_polarity(&nearly_constant_table),
            LinkPolarity::Positive,
            "Nearly constant but increasing should have positive polarity"
        );
    }

    #[test]
    fn test_lookup_table_polarity_in_links() {
        use crate::datamodel;
        use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

        // Create a model with a lookup table
        let mut model_vars = vec![
            x_stock("water", "100", &[], &["outflow"], None),
            x_flow("outflow", "water * lookup(lookup, water)", None),
        ];

        // Create the lookup table auxiliary
        let mut lookup_var = x_aux("lookup", "0", None);
        if let datamodel::Variable::Aux(aux) = &mut lookup_var {
            aux.gf = Some(datamodel::GraphicalFunction {
                kind: datamodel::GraphicalFunctionKind::Continuous,
                x_points: Some(vec![0.0, 50.0, 100.0, 150.0]),
                y_points: vec![0.1, 0.2, 0.3, 0.4], // Monotonically increasing
                x_scale: datamodel::GraphicalFunctionScale {
                    min: 0.0,
                    max: 150.0,
                },
                y_scale: datamodel::GraphicalFunctionScale { min: 0.1, max: 0.4 },
            });
        }
        model_vars.push(lookup_var);

        let model = x_model("main", model_vars);
        let sim_specs = sim_specs_with_units("months");
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);

        // Build causal graph
        let main_ident = Ident::new("main");
        let main_model = project
            .models
            .get(&main_ident)
            .expect("Should have main model");
        let graph =
            CausalGraph::from_model(main_model, &project).expect("Should build causal graph");

        // Get the link polarity for water -> outflow (through lookup table)
        let water = Ident::new("water");
        let outflow = Ident::new("outflow");
        let polarity = graph.get_link_polarity(&water, &outflow);

        // Since lookup table is monotonically increasing and water appears positively in the equation,
        // the polarity should be positive
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Monotonically increasing lookup table should preserve positive polarity"
        );

        // Find loops and verify they have correct polarity
        let loops = graph.find_loops();
        assert_eq!(loops.len(), 1, "Should have one loop");

        let loop_item = &loops[0];
        // The loop is: water -> outflow -> water
        // water -> outflow: Positive (through increasing lookup)
        // outflow -> water: Negative (outflow decreases stock)
        // One negative link = Balancing loop
        assert_eq!(
            loop_item.polarity,
            LoopPolarity::Balancing,
            "Loop with one negative link should be balancing"
        );
    }

    #[test]
    fn test_fishbanks_loops() {
        use crate::project::Project;
        use crate::prost::Message;
        use std::fs;

        // Load the fishbanks.protobin file - path is relative to workspace root
        let proto_bytes = fs::read("../../test/fishbanks.protobin")
            .expect("Failed to read fishbanks.protobin file");

        // Decode the protobuf into project_io::Project
        let project_io = crate::project_io::Project::decode(&proto_bytes[..])
            .expect("Failed to decode fishbanks.protobin");

        // Convert to datamodel::Project then to project::Project
        let datamodel_project = crate::serde::deserialize(project_io);
        let project = Project::from(datamodel_project);

        // Find the main model (fishbanks models typically have a single main model)
        let main_model_name = project
            .models
            .keys()
            .find(|name| project.models.get(*name).is_some_and(|m| !m.implicit))
            .expect("Should have a non-implicit model");

        let main_model = project
            .models
            .get(main_model_name)
            .expect("Should be able to get main model");

        // Build the causal graph and find loops
        let graph = CausalGraph::from_model(main_model, &project)
            .expect("Should be able to build causal graph");
        let loops = graph.find_loops();

        // Assert we have exactly 3 feedback loops
        assert_eq!(
            loops.len(),
            3,
            "Fishbanks model should have exactly 3 feedback loops, found: {}",
            loops.len()
        );

        // Find the r1 loop (the one with catch and harvest_rate)
        let r1_loop = loops
            .iter()
            .find(|l| {
                l.links.iter().any(|link| {
                    link.from.as_str() == "harvest_rate" && link.to.as_str() == "fish_stock"
                })
            })
            .expect("Should find loop containing harvest_rate -> fish_stock");

        // Find the specific link: harvest_rate -> fish_stock
        let harvest_to_stock_link = r1_loop
            .links
            .iter()
            .find(|link| link.from.as_str() == "harvest_rate" && link.to.as_str() == "fish_stock")
            .expect("Should find harvest_rate -> fish_stock link");

        // Assert that harvest_rate -> fish_stock has negative polarity (it's an outflow)
        assert_eq!(
            harvest_to_stock_link.polarity,
            LinkPolarity::Negative,
            "harvest_rate -> fish_stock should have negative polarity (outflow decreases stock)"
        );

        // The loop containing harvest_rate -> fish_stock should be Undetermined
        // because some links have unknown polarity (conservative classification:
        // if ANY link is unknown, the loop is Undetermined)
        assert_eq!(
            r1_loop.polarity,
            LoopPolarity::Undetermined,
            "Loop containing harvest_rate should be Undetermined (has unknown-polarity links)"
        );
    }

    #[test]
    fn test_logistic_growth_loops() {
        use crate::project::Project;
        use crate::prost::Message;
        use std::fs;

        // Load the logistic-growth.protobin file - path is relative to workspace root
        let proto_bytes = fs::read("../../test/logistic-growth.protobin")
            .expect("Failed to read logistic-growth.protobin file");

        // Decode the protobuf into project_io::Project
        let project_io = crate::project_io::Project::decode(&proto_bytes[..])
            .expect("Failed to decode logistic-growth.protobin");

        // Convert to datamodel::Project then to project::Project
        let datamodel_project = crate::serde::deserialize(project_io);
        let project = Project::from(datamodel_project);

        // Find the main model
        let main_model_name = project
            .models
            .keys()
            .find(|name| project.models.get(*name).is_some_and(|m| !m.implicit))
            .expect("Should have a non-implicit model");

        let main_model = project
            .models
            .get(main_model_name)
            .expect("Should be able to get main model");

        // Build the causal graph and find loops
        let graph = CausalGraph::from_model(main_model, &project)
            .expect("Should be able to build causal graph");
        let loops = graph.find_loops();

        // Logistic growth should have exactly 2 loops:
        // 1. One undetermined loop (simple growth - has unknown-polarity link
        //    because population -> net_birth_rate multiplication involves
        //    fractional_growth_rate which isn't a constant)
        // 2. One balancing loop (carrying capacity constraint - all links
        //    have known polarity)
        assert_eq!(
            loops.len(),
            2,
            "Logistic growth model should have exactly 2 feedback loops, found: {}",
            loops.len()
        );

        // Count balancing and undetermined loops
        let balancing_count = loops
            .iter()
            .filter(|l| l.polarity == LoopPolarity::Balancing)
            .count();
        let undetermined_count = loops
            .iter()
            .filter(|l| l.polarity == LoopPolarity::Undetermined)
            .count();

        assert_eq!(
            balancing_count, 1,
            "Logistic growth model should have exactly 1 balancing loop, found: {}",
            balancing_count
        );

        assert_eq!(
            undetermined_count, 1,
            "Logistic growth model should have exactly 1 undetermined loop, found: {}",
            undetermined_count
        );

        // Check if the carrying capacity loop is correctly identified as balancing
        // This loop involves fractional_growth_rate which depends on fraction_of_carrying_capacity_used
        let carrying_capacity_loop = loops.iter().find(|l| {
            l.links.iter().any(|link| {
                link.from.as_str() == "fraction_of_carrying_capacity_used"
                    && link.to.as_str() == "fractional_growth_rate"
            }) || l.links.iter().any(|link| {
                link.from.as_str() == "fractional_growth_rate"
                    && link.to.as_str() == "net_birth_rate"
            })
        });

        if let Some(loop_item) = carrying_capacity_loop {
            assert_eq!(
                loop_item.polarity,
                LoopPolarity::Balancing,
                "The carrying capacity loop should be balancing, not reinforcing. Path: {}",
                loop_item.format_path()
            );
        } else {
            panic!("Could not find the carrying capacity loop in the model");
        }
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_reinforcing() {
        // All positive scores -> Reinforcing
        let scores = vec![f64::NAN, 1.0, 2.0, 3.0, 0.5];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, Some(LoopPolarity::Reinforcing));
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_balancing() {
        // All negative scores -> Balancing
        let scores = vec![f64::NAN, -1.0, -2.0, -3.0, -0.5];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, Some(LoopPolarity::Balancing));
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_undetermined() {
        // Mix of positive and negative scores -> Undetermined
        let scores = vec![f64::NAN, 1.0, -2.0, 3.0, -0.5];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, Some(LoopPolarity::Undetermined));
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_empty() {
        // Empty scores -> None
        let scores: Vec<f64> = vec![];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, None);
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_all_nan() {
        // All NaN scores -> None
        let scores = vec![f64::NAN, f64::NAN, f64::NAN];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, None);
    }

    #[test]
    fn test_loop_polarity_from_runtime_scores_all_zero() {
        // All zero scores (after filtering NaN) -> None
        let scores = vec![f64::NAN, 0.0, 0.0, 0.0];
        let polarity = LoopPolarity::from_runtime_scores(&scores);
        assert_eq!(polarity, None);
    }

    #[test]
    fn test_loop_polarity_abbreviation() {
        assert_eq!(LoopPolarity::Reinforcing.abbreviation(), "R");
        assert_eq!(LoopPolarity::Balancing.abbreviation(), "B");
        assert_eq!(LoopPolarity::Undetermined.abbreviation(), "U");
    }

    #[test]
    fn test_calculate_polarity_all_unknown_links() {
        // When all links have Unknown polarity, the loop should be Undetermined
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let links = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Unknown,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Unknown,
            },
        ];

        let polarity = graph.calculate_polarity(&links);
        assert_eq!(
            polarity,
            LoopPolarity::Undetermined,
            "Loop with all Unknown link polarities should have Undetermined polarity"
        );
    }

    #[test]
    fn test_calculate_polarity_mixed_unknown_and_known() {
        // Conservative approach: if ANY link is unknown, the loop is Undetermined
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // One negative link, one unknown -> should be Undetermined
        let links_one_negative = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Unknown,
            },
        ];

        let polarity = graph.calculate_polarity(&links_one_negative);
        assert_eq!(
            polarity,
            LoopPolarity::Undetermined,
            "Loop with any unknown link should be Undetermined"
        );

        // Two positive links, one unknown -> should also be Undetermined
        let links_two_positive = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("c"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::new("c"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Unknown,
            },
        ];

        let polarity = graph.calculate_polarity(&links_two_positive);
        assert_eq!(
            polarity,
            LoopPolarity::Undetermined,
            "Loop with any unknown link should be Undetermined"
        );
    }

    #[test]
    fn test_calculate_polarity_all_known_links() {
        // When all links have known polarity, count negative links
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        // All positive links -> Reinforcing (even number of negatives: 0)
        let links_all_positive = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Positive,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Positive,
            },
        ];

        let polarity = graph.calculate_polarity(&links_all_positive);
        assert_eq!(
            polarity,
            LoopPolarity::Reinforcing,
            "Loop with all positive links should be Reinforcing"
        );

        // One negative, one positive -> Balancing (odd number of negatives: 1)
        let links_one_negative = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Positive,
            },
        ];

        let polarity = graph.calculate_polarity(&links_one_negative);
        assert_eq!(
            polarity,
            LoopPolarity::Balancing,
            "Loop with one negative link should be Balancing"
        );

        // Two negative links -> Reinforcing (even number of negatives: 2)
        let links_two_negatives = vec![
            Link {
                from: Ident::new("a"),
                to: Ident::new("b"),
                polarity: LinkPolarity::Negative,
            },
            Link {
                from: Ident::new("b"),
                to: Ident::new("a"),
                polarity: LinkPolarity::Negative,
            },
        ];

        let polarity = graph.calculate_polarity(&links_two_negatives);
        assert_eq!(
            polarity,
            LoopPolarity::Reinforcing,
            "Loop with two negative links should be Reinforcing"
        );
    }

    #[test]
    fn test_loop_id_assignment_undetermined_polarity() {
        // Loops with Undetermined structural polarity should get "u" prefix
        let graph = CausalGraph {
            edges: HashMap::new(),
            stocks: HashSet::new(),
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };

        let mut loops = vec![
            Loop {
                id: String::new(),
                links: vec![
                    Link {
                        from: Ident::new("a"),
                        to: Ident::new("b"),
                        polarity: LinkPolarity::Unknown,
                    },
                    Link {
                        from: Ident::new("b"),
                        to: Ident::new("a"),
                        polarity: LinkPolarity::Unknown,
                    },
                ],
                stocks: vec![],
                polarity: LoopPolarity::Undetermined,
            },
            Loop {
                id: String::new(),
                links: vec![
                    Link {
                        from: Ident::new("x"),
                        to: Ident::new("y"),
                        polarity: LinkPolarity::Positive,
                    },
                    Link {
                        from: Ident::new("y"),
                        to: Ident::new("x"),
                        polarity: LinkPolarity::Positive,
                    },
                ],
                stocks: vec![],
                polarity: LoopPolarity::Reinforcing,
            },
        ];

        graph.assign_deterministic_loop_ids(&mut loops);

        // Find the undetermined loop (contains "a" and "b")
        let undetermined_loop = loops
            .iter()
            .find(|l| l.links.iter().any(|link| link.from.as_str() == "a"))
            .expect("Should find undetermined loop");

        assert!(
            undetermined_loop.id.starts_with("u"),
            "Undetermined polarity loop should have 'u' prefix, got: {}",
            undetermined_loop.id
        );

        // Find the reinforcing loop (contains "x" and "y")
        let reinforcing_loop = loops
            .iter()
            .find(|l| l.links.iter().any(|link| link.from.as_str() == "x"))
            .expect("Should find reinforcing loop");

        assert!(
            reinforcing_loop.id.starts_with("r"),
            "Reinforcing polarity loop should have 'r' prefix, got: {}",
            reinforcing_loop.id
        );
    }

    #[test]
    fn test_builtin_polarity_monotone_increasing() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let empty_vars = HashMap::new();

        // Exp(x), Ln(x), Log10(x), Sqrt(x), Arctan(x), Int(x) all propagate polarity
        let monotone_fns: Vec<(&str, BuiltinFn<Expr2>)> = vec![
            ("Exp", BuiltinFn::Exp(x_expr())),
            ("Ln", BuiltinFn::Ln(x_expr())),
            ("Log10", BuiltinFn::Log10(x_expr())),
            ("Sqrt", BuiltinFn::Sqrt(x_expr())),
            ("Arctan", BuiltinFn::Arctan(x_expr())),
            ("Int", BuiltinFn::Int(x_expr())),
        ];

        for (name, builtin) in monotone_fns {
            let expr = Expr2::App(builtin, None, Loc::default());
            let ast = Ast::Scalar(expr);
            let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
            assert_eq!(
                polarity,
                LinkPolarity::Positive,
                "{name}(x) should propagate positive polarity"
            );
        }
    }

    #[test]
    fn test_builtin_polarity_monotone_negative_inner() {
        use crate::ast::expr0::UnaryOp;
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        // -x has Negative polarity
        let neg_x = || {
            Box::new(Expr2::Op1(
                UnaryOp::Negative,
                Box::new(Expr2::Var(x_var.clone(), None, Loc::default())),
                None,
                Loc::default(),
            ))
        };
        let empty_vars = HashMap::new();

        let expr = Expr2::App(BuiltinFn::Exp(neg_x()), None, Loc::default());
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Negative,
            "Exp(-x) should have negative polarity"
        );
    }

    #[test]
    fn test_builtin_polarity_non_monotone_returns_unknown() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let empty_vars = HashMap::new();

        let non_monotone: Vec<(&str, BuiltinFn<Expr2>)> = vec![
            ("Abs", BuiltinFn::Abs(x_expr())),
            ("Sin", BuiltinFn::Sin(x_expr())),
            ("Cos", BuiltinFn::Cos(x_expr())),
            ("Sign", BuiltinFn::Sign(x_expr())),
        ];

        for (name, builtin) in non_monotone {
            let expr = Expr2::App(builtin, None, Loc::default());
            let ast = Ast::Scalar(expr);
            let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
            assert_eq!(
                polarity,
                LinkPolarity::Unknown,
                "{name}(x) should return Unknown polarity"
            );
        }
    }

    #[test]
    fn test_builtin_polarity_max_min_scalar() {
        use crate::ast::{Ast, Expr2, Loc};
        use crate::builtins::BuiltinFn;

        let x_var = Ident::new("x");
        let x_expr = || Box::new(Expr2::Var(x_var.clone(), None, Loc::default()));
        let const_5 = || Box::new(Expr2::Const("5".to_string(), 5.0, Loc::default()));
        let empty_vars = HashMap::new();

        // Max(x, 5): only x depends on from_var -> propagate x's polarity
        let expr = Expr2::App(
            BuiltinFn::Max(x_expr(), Some(const_5())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Max(x, 5) should propagate positive polarity"
        );

        // Min(5, x): only x depends on from_var -> propagate x's polarity
        let expr = Expr2::App(
            BuiltinFn::Min(const_5(), Some(x_expr())),
            None,
            Loc::default(),
        );
        let ast = Ast::Scalar(expr);
        let polarity = analyze_link_polarity(&ast, &x_var, &empty_vars);
        assert_eq!(
            polarity,
            LinkPolarity::Positive,
            "Min(5, x) should propagate positive polarity"
        );
    }

    #[test]
    fn test_all_links() {
        // Create a model with known causal structure:
        // population -> births -> population (reinforcing loop)
        // population -> deaths -> population (balancing loop)
        // birth_rate -> births (external input)
        // death_rate -> deaths (external input)
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
        let project = x_project(sim_specs, &[model]);
        let project = Project::from(project);

        let main_ident = Ident::new("main");
        let main_model = project.models.get(&main_ident).unwrap();
        let graph = CausalGraph::from_model(main_model, &project).unwrap();

        let links = graph.all_links();

        // Should have links for:
        // birth_rate -> births
        // births -> population (inflow, positive)
        // death_rate -> deaths
        // deaths -> population (outflow, negative)
        // population -> births (stock to flow)
        // population -> deaths (stock to flow)
        assert_eq!(
            links.len(),
            6,
            "Should have exactly 6 causal links, found {}",
            links.len()
        );

        // Check specific links exist with correct polarity
        let births_to_pop = links
            .iter()
            .find(|l| l.from.as_str() == "births" && l.to.as_str() == "population");
        assert!(
            births_to_pop.is_some(),
            "Should have births->population link"
        );
        assert_eq!(
            births_to_pop.unwrap().polarity,
            LinkPolarity::Positive,
            "Inflow should have positive polarity"
        );

        let deaths_to_pop = links
            .iter()
            .find(|l| l.from.as_str() == "deaths" && l.to.as_str() == "population");
        assert!(
            deaths_to_pop.is_some(),
            "Should have deaths->population link"
        );
        assert_eq!(
            deaths_to_pop.unwrap().polarity,
            LinkPolarity::Negative,
            "Outflow should have negative polarity"
        );

        // Verify deterministic ordering (sorted by from, then to)
        for i in 1..links.len() {
            let prev = &links[i - 1];
            let curr = &links[i];
            let order = prev
                .from
                .as_str()
                .cmp(curr.from.as_str())
                .then_with(|| prev.to.as_str().cmp(curr.to.as_str()));
            assert!(
                order != std::cmp::Ordering::Greater,
                "Links should be sorted: {:?} should come before {:?}",
                (prev.from.as_str(), prev.to.as_str()),
                (curr.from.as_str(), curr.to.as_str())
            );
        }
    }
}
