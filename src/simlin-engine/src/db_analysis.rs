// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Causal graph analysis tracked functions.
//!
//! Extracted from db.rs for file-size management. Contains:
//! - CausalEdgesResult, LoopCircuitsResult, CyclePartitionsResult
//! - DetectedLoop, DetectedLoopsResult (polarity-aware loop detection)
//! - model_causal_edges, model_loop_circuits, model_cycle_partitions
//! - model_detected_loops (matches LTM augmentation loop IDs)
//! - reconstruct_model_variables, reconstruct_single_variable

use std::collections::{BTreeSet, HashMap};

use crate::canonicalize;
use crate::datamodel;

use super::{
    Db, SourceModel, SourceProject, SourceVariableKind, parse_source_variable,
    source_dims_to_datamodel, variable_direct_dependencies,
};

/// Causal edge structure for a model, built from variable dependency sets
/// and structural info (stock inflows/outflows, module refs).
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct CausalEdgesResult {
    /// Adjacency list: from_var -> {to_var1, to_var2, ...}
    pub edges: HashMap<String, BTreeSet<String>>,
    /// Stock variables in the model
    pub stocks: BTreeSet<String>,
    /// Module var_name -> model_name for dynamic modules
    pub dynamic_modules: HashMap<String, String>,
}

/// Deduplicated loop circuits as node name lists.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct LoopCircuitsResult {
    pub circuits: Vec<Vec<String>>,
}

/// A detected feedback loop with polarity and deterministic ID.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct DetectedLoop {
    /// Deterministic ID: r1, r2, ... (reinforcing), b1, b2, ... (balancing),
    /// u1, u2, ... (undetermined).
    pub id: String,
    /// Variable names in the loop, in circuit order.
    pub variables: Vec<String>,
    /// Loop polarity.
    pub polarity: DetectedLoopPolarity,
}

/// Loop polarity as determined by structural analysis of link signs.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub enum DetectedLoopPolarity {
    Reinforcing,
    Balancing,
    Undetermined,
}

/// Result of full loop detection with polarity and IDs.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct DetectedLoopsResult {
    pub loops: Vec<DetectedLoop>,
}

/// Stock-to-stock cycle partitions.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct CyclePartitionsResult {
    pub partitions: Vec<Vec<String>>,
    pub stock_partition: HashMap<String, usize>,
}

/// Normalize a dependency/reference name by stripping a leading middot
/// (XMILE parent-scope refs like `.area` canonicalize to `·area`) and then
/// truncating at the first remaining middot to collapse `module·output`
/// qualifiers down to the module variable name.
pub(super) fn normalize_module_ref_str(s: &str) -> String {
    let effective = s.strip_prefix('\u{00B7}').unwrap_or(s);
    if let Some(pos) = effective.find('\u{00B7}') {
        effective[..pos].to_string()
    } else {
        effective.to_string()
    }
}

/// Construct a lightweight CausalGraph from a CausalEdgesResult.
/// Variables and module_graphs are empty -- suitable for graph algorithms
/// (circuit finding, SCC computation) but not for polarity analysis.
pub(crate) fn causal_graph_from_edges(result: &CausalEdgesResult) -> crate::ltm::CausalGraph {
    use crate::common::{Canonical, Ident};
    use std::collections::HashSet;

    let edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = result
        .edges
        .iter()
        .map(|(from, tos)| {
            (
                Ident::new(from),
                tos.iter().map(|t| Ident::new(t)).collect(),
            )
        })
        .collect();
    let stocks: HashSet<Ident<Canonical>> = result.stocks.iter().map(|s| Ident::new(s)).collect();

    crate::ltm::CausalGraph {
        edges,
        stocks,
        variables: HashMap::new(),
        module_graphs: HashMap::new(),
    }
}

/// Build the causal edge structure for a model from salsa-tracked
/// dependency sets and structural variable info.
///
/// Reads `variable_direct_dependencies` (establishing salsa dep on dep
/// sets) and `parse_source_variable` (for implicit variable details like
/// module input refs). Salsa backdating ensures that when equation text
/// changes without changing the resulting edge structure, the cached
/// result is reused and downstream graph algorithms are skipped.
#[salsa::tracked(returns(ref))]
pub fn model_causal_edges(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> CausalEdgesResult {
    let source_vars = model.variables(db);
    let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut stocks = BTreeSet::new();
    let mut dynamic_modules = HashMap::new();

    for (name, source_var) in source_vars.iter() {
        let kind = source_var.kind(db);

        match kind {
            SourceVariableKind::Stock => {
                stocks.insert(name.clone());
                for flow in source_var
                    .inflows(db)
                    .iter()
                    .chain(source_var.outflows(db).iter())
                {
                    let canonical_flow = canonicalize(flow).into_owned();
                    edges
                        .entry(canonical_flow)
                        .or_default()
                        .insert(name.clone());
                }
            }
            SourceVariableKind::Module => {
                let self_prefix = format!("{name}\u{00B7}");
                for mr in source_var.module_refs(db).iter() {
                    let canonical_src = canonicalize(&mr.src).into_owned();
                    // Skip output refs where src is within the module's own
                    // namespace (Stella imports include these); normalizing
                    // them would create false self-loops.
                    if canonical_src.starts_with(&self_prefix) {
                        continue;
                    }
                    let normalized = normalize_module_ref_str(&canonical_src);
                    edges.entry(normalized).or_default().insert(name.clone());
                }
                let model_name = source_var.model_name(db);
                if !model_name.is_empty() {
                    dynamic_modules.insert(name.clone(), model_name.clone());
                }
            }
            _ => {
                let deps = variable_direct_dependencies(db, *source_var, project);
                for dep in &deps.dt_deps {
                    let normalized = normalize_module_ref_str(dep);
                    edges.entry(normalized).or_default().insert(name.clone());
                }
            }
        }

        // Include implicit variables (module instances from SMOOTH/DELAY expansion)
        let parsed = parse_source_variable(db, *source_var, project);
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();

            match implicit_dm_var {
                datamodel::Variable::Stock(s) => {
                    stocks.insert(imp_name.clone());
                    for flow in s.inflows.iter().chain(s.outflows.iter()) {
                        let canonical_flow = canonicalize(flow).into_owned();
                        edges
                            .entry(canonical_flow)
                            .or_default()
                            .insert(imp_name.clone());
                    }
                }
                datamodel::Variable::Module(m) => {
                    let self_prefix = format!("{imp_name}\u{00B7}");
                    for mr in &m.references {
                        let canonical_src = canonicalize(&mr.src).into_owned();
                        if canonical_src.starts_with(&self_prefix) {
                            continue;
                        }
                        let normalized = normalize_module_ref_str(&canonical_src);
                        edges
                            .entry(normalized)
                            .or_default()
                            .insert(imp_name.clone());
                    }
                    dynamic_modules.insert(imp_name.clone(), m.model_name.clone());
                }
                _ => {
                    // For implicit flows/auxes, get deps from the parent's
                    // variable_direct_dependencies result.
                    let deps = variable_direct_dependencies(db, *source_var, project);
                    if let Some(implicit_dep) =
                        deps.implicit_vars.iter().find(|iv| iv.name == imp_name)
                    {
                        for dep in &implicit_dep.dt_deps {
                            let normalized = normalize_module_ref_str(dep);
                            edges
                                .entry(normalized)
                                .or_default()
                                .insert(imp_name.clone());
                        }
                    }
                }
            }
        }
    }

    CausalEdgesResult {
        edges,
        stocks,
        dynamic_modules,
    }
}

/// Find all elementary loop circuits in a model's causal graph.
///
/// Depends on `model_causal_edges`, so loop detection is cached when
/// the edge structure hasn't changed (even if equation text changed).
#[salsa::tracked(returns(ref))]
pub fn model_loop_circuits(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LoopCircuitsResult {
    let edges_result = model_causal_edges(db, model, project);
    let graph = causal_graph_from_edges(edges_result);
    let circuits = graph.find_circuit_node_lists();
    LoopCircuitsResult {
        circuits: circuits
            .into_iter()
            .map(|c| c.into_iter().map(|n| n.to_string()).collect())
            .collect(),
    }
}

/// Detect feedback loops with polarity analysis and deterministic IDs.
///
/// Builds a full CausalGraph from salsa-tracked causal edges and
/// reconstructed variable ASTs, then runs Johnson's algorithm with
/// polarity analysis. Loop IDs (r1, b1, u1, ...) match those used
/// by LTM augmentation.
pub fn model_detected_loops(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> DetectedLoopsResult {
    let edges_result = model_causal_edges(db, model, project);
    let mut graph = causal_graph_from_edges(edges_result);

    // Populate variables for polarity analysis so loop IDs match LTM
    graph.variables = reconstruct_model_variables(db, model, project);

    let loops = graph.find_loops();
    DetectedLoopsResult {
        loops: loops
            .into_iter()
            .map(|l| {
                // Extract variable names from the loop's links
                let mut vars = Vec::new();
                let mut seen = std::collections::HashSet::new();
                if !l.links.is_empty() {
                    let first = l.links[0].from.to_string();
                    if seen.insert(first.clone()) {
                        vars.push(first);
                    }
                    for link in &l.links {
                        let to = link.to.to_string();
                        if seen.insert(to.clone()) {
                            vars.push(to);
                        }
                    }
                }
                DetectedLoop {
                    id: l.id,
                    variables: vars,
                    polarity: match l.polarity {
                        crate::ltm::LoopPolarity::Reinforcing => DetectedLoopPolarity::Reinforcing,
                        crate::ltm::LoopPolarity::Balancing => DetectedLoopPolarity::Balancing,
                        crate::ltm::LoopPolarity::Undetermined => {
                            DetectedLoopPolarity::Undetermined
                        }
                    },
                }
            })
            .collect(),
    }
}

/// Compute stock-to-stock cycle partitions (SCCs) for a model.
///
/// Depends on `model_causal_edges`, so partition computation is cached
/// when the edge structure hasn't changed.
#[salsa::tracked(returns(ref))]
pub fn model_cycle_partitions(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> CyclePartitionsResult {
    let edges_result = model_causal_edges(db, model, project);
    let graph = causal_graph_from_edges(edges_result);
    let cp = graph.compute_cycle_partitions();
    CyclePartitionsResult {
        partitions: cp
            .partitions
            .into_iter()
            .map(|p| p.into_iter().map(|s| s.to_string()).collect())
            .collect(),
        stock_partition: cp
            .stock_partition
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

/// Reconstruct `Variable` objects from salsa-tracked parse results for
/// all variables in a model (including implicit variables).
pub(super) fn reconstruct_model_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<crate::common::Ident<crate::common::Canonical>, crate::variable::Variable> {
    use crate::common::{Canonical, Ident};

    let source_vars = model.variables(db);
    let dims = source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_context,
        model_name: "",
    };

    let mut variables: HashMap<Ident<Canonical>, crate::variable::Variable> = HashMap::new();

    for (name, source_var) in source_vars.iter() {
        let parsed = parse_source_variable(db, *source_var, project);
        let lowered = crate::model::lower_variable(&scope, &parsed.variable);
        variables.insert(Ident::new(name), lowered);

        // Add implicit variables (module instances from SMOOTH/DELAY expansion)
        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap_or_default();
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            let mut dummy_implicits = Vec::new();
            let parsed_imp = crate::variable::parse_var(
                &dims,
                implicit_dm_var,
                &mut dummy_implicits,
                &units_ctx,
                |mi| Ok(Some(mi.clone())),
            );
            let lowered_imp = crate::model::lower_variable(&scope, &parsed_imp);
            variables.insert(Ident::new(&imp_name), lowered_imp);
        }
    }

    variables
}

/// Reconstruct a single `Variable` by name from a model's parse results.
///
/// Checks explicit source variables first, then searches implicit variables
/// (from SMOOTH/DELAY module expansion) if the name isn't found.
/// Returns None if the name doesn't match any variable in the model.
pub(super) fn reconstruct_single_variable(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_name: &str,
) -> Option<crate::variable::Variable> {
    use crate::common::{Canonical, Ident};

    let source_vars = model.variables(db);
    let dims = source_dims_to_datamodel(project.dimensions(db));
    let dim_context = crate::dimensions::DimensionsContext::from(dims.as_slice());
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: &dim_context,
        model_name: "",
    };

    // Check explicit variables first
    if let Some(source_var) = source_vars.get(var_name) {
        let parsed = parse_source_variable(db, *source_var, project);
        let lowered = crate::model::lower_variable(&scope, &parsed.variable);
        return Some(lowered);
    }

    // Search implicit variables from all source variables
    let canonical_target: Ident<Canonical> = Ident::new(var_name);
    let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap_or_default();

    for (_name, source_var) in source_vars.iter() {
        let parsed = parse_source_variable(db, *source_var, project);
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            if Ident::<Canonical>::new(&imp_name) == canonical_target {
                let mut dummy_implicits = Vec::new();
                let parsed_imp = crate::variable::parse_var(
                    &dims,
                    implicit_dm_var,
                    &mut dummy_implicits,
                    &units_ctx,
                    |mi| Ok(Some(mi.clone())),
                );
                let lowered_imp = crate::model::lower_variable(&scope, &parsed_imp);
                return Some(lowered_imp);
            }
        }
    }

    None
}
