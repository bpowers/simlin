// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Causal graph analysis tracked functions.
//!
//! Extracted from db.rs for file-size management. Contains:
//! - CausalEdgesResult, LoopCircuitsResult, CyclePartitionsResult
//! - ElementCausalEdgesResult, ElementDependencyKind (element-level graph)
//! - DetectedLoop, DetectedLoopsResult (polarity-aware loop detection)
//! - model_causal_edges, model_loop_circuits, model_cycle_partitions
//! - model_detected_loops (matches LTM augmentation loop IDs)
//! - reconstruct_model_variables, reconstruct_single_variable

use std::collections::{BTreeSet, HashMap};

use crate::canonicalize;
use crate::datamodel;

use super::{
    Db, SourceModel, SourceProject, SourceVariableKind, build_module_inputs,
    model_module_ident_context, parse_source_variable_with_module_context,
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

/// Element-level causal edge structure for a model.
///
/// Expands variable-level edges from `CausalEdgesResult` into element-level
/// edges where each array element is an independent node. Scalar variables
/// keep their plain names; arrayed variables use subscript notation
/// (e.g., `population[NYC]`). Models without arrays produce an element
/// graph identical to the variable graph.
#[allow(dead_code)] // infrastructure for upcoming element-level graph expansion
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ElementCausalEdgesResult {
    /// Adjacency list: from_element -> {to_element1, to_element2, ...}
    pub edges: HashMap<String, BTreeSet<String>>,
    /// Element-level stock nodes (e.g., `population[NYC]`, `population[Boston]`)
    pub stocks: BTreeSet<String>,
}

/// Format an element-level node name with subscript notation.
/// For scalar variables, the caller should use the name directly;
/// this function always appends the subscript.
#[allow(dead_code)] // infrastructure for upcoming element-level graph expansion
fn format_element_name(var_name: &str, element: &str) -> String {
    format!("{var_name}[{element}]")
}

/// Format an element-level node name for multi-dimensional arrays.
/// Returns `name[e1,e2,...]` (e.g., `migration[NYC,Boston]`).
#[allow(dead_code)] // infrastructure for upcoming element-level graph expansion
fn format_multi_element_name(var_name: &str, elements: &[&str]) -> String {
    format!("{}[{}]", var_name, elements.join(","))
}

/// How a source variable is referenced in a target's equation.
///
/// When expanding variable-level causal edges to element-level edges,
/// the dependency kind determines the expansion pattern:
/// - `Scalar`: one-to-one or broadcast (no subscripts involved)
/// - `SameElement`: A2A same-element reference (e.g., `population[Region]`)
/// - `CrossElement`: reducer over all elements (e.g., `SUM(population[*])`)
#[allow(dead_code)] // infrastructure for upcoming element-level graph expansion
#[derive(Debug, Clone, PartialEq, Eq)]
enum ElementDependencyKind {
    /// Scalar reference: source appears as a bare variable with no subscripts
    Scalar,
    /// Same-element A2A reference: source referenced with non-wildcard subscripts.
    /// In an A2A context, these resolve to the current element automatically.
    SameElement,
    /// Cross-element reference: source appears with a wildcard subscript
    /// (e.g., `population[*]` inside a reducer like SUM or MEAN)
    CrossElement,
}

/// Classify how a source variable is referenced in a target variable's equation.
///
/// Walks the target variable's lowered AST (`Expr2` level) looking for
/// references to the source identifier. The classification is:
/// - `CrossElement` if the source appears inside an `Expr2::Subscript` node
///   with any `IndexExpr2::Wildcard` index (from `x[*]` syntax)
/// - `SameElement` if the source appears inside an `Expr2::Subscript` node
///   with all non-wildcard indices, OR if the source is arrayed and appears
///   as a bare `Expr2::Var` in an A2A equation context (at Expr2 level,
///   A2A variable references retain their Var form; subscript expansion
///   happens later in the Expr3 phase)
/// - `Scalar` if the source appears as a bare `Expr2::Var` and is NOT arrayed
///
/// `source_is_arrayed` indicates whether the source variable has dimensions.
/// This is necessary because at the Expr2 level, arrayed variables referenced
/// in an A2A equation keep their bare Var form (the ArrayBounds may not be
/// populated when lowering with a minimal ScopeStage0 context).
///
/// If the source is referenced multiple ways (e.g., both `population` and
/// `SUM(population[*])` in the same equation), the highest-priority kind
/// wins: CrossElement > SameElement > Scalar.
///
/// Returns `Scalar` as default if the source is not found (defensive).
#[allow(dead_code)] // infrastructure for upcoming element-level graph expansion
fn classify_element_dependency(
    target_var: &crate::variable::Variable,
    source_ident: &str,
    source_is_arrayed: bool,
) -> ElementDependencyKind {
    let Some(ast) = target_var.ast() else {
        return ElementDependencyKind::Scalar;
    };

    let mut result = ElementDependencyKind::Scalar;
    let mut found = false;

    // Walk all expressions in the AST (scalar, A2A, or arrayed)
    match ast {
        crate::ast::Ast::Scalar(expr) | crate::ast::Ast::ApplyToAll(_, expr) => {
            classify_in_expr(
                expr,
                source_ident,
                source_is_arrayed,
                &mut result,
                &mut found,
            );
        }
        crate::ast::Ast::Arrayed(_, subscript_map, default_expr, _) => {
            for expr in subscript_map.values() {
                classify_in_expr(
                    expr,
                    source_ident,
                    source_is_arrayed,
                    &mut result,
                    &mut found,
                );
                if result == ElementDependencyKind::CrossElement {
                    return result; // highest priority, short-circuit
                }
            }
            if let Some(default) = default_expr {
                classify_in_expr(
                    default,
                    source_ident,
                    source_is_arrayed,
                    &mut result,
                    &mut found,
                );
            }
        }
    }

    if found {
        result
    } else {
        ElementDependencyKind::Scalar
    }
}

/// Recursively walk an `Expr2` tree, looking for references to `source_ident`.
///
/// Updates `result` to the highest-priority classification found so far.
/// Priority: CrossElement > SameElement > Scalar.
///
/// At the Expr2 level, an arrayed variable referenced without explicit subscripts
/// in an A2A equation stays as `Expr2::Var(ident, Some(ArrayBounds), _)` -- it is
/// NOT lowered to a `Subscript` node. The subscript expansion happens later in
/// the compiler (Expr3 phase). We detect SameElement by checking whether the
/// `Var` node carries `ArrayBounds` (meaning it's arrayed and will be subscript-
/// expanded element-wise at compile time).
fn classify_in_expr(
    expr: &crate::ast::Expr2,
    source_ident: &str,
    source_is_arrayed: bool,
    result: &mut ElementDependencyKind,
    found: &mut bool,
) {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::builtins::{BuiltinContents, walk_builtin_expr};

    // Short-circuit once we've found the highest-priority kind
    if *result == ElementDependencyKind::CrossElement {
        return;
    }

    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, array_bounds, _) => {
            if ident.as_str() == source_ident {
                *found = true;
                // A bare Var reference to an arrayed variable in an A2A equation
                // means same-element mapping. At Expr2 level, ArrayBounds may or
                // may not be populated (depends on the lowering context), so we
                // use the caller-provided `source_is_arrayed` flag as the primary
                // signal, with ArrayBounds as a secondary check.
                if (source_is_arrayed || array_bounds.is_some())
                    && *result == ElementDependencyKind::Scalar
                {
                    *result = ElementDependencyKind::SameElement;
                }
                // Scalar source -> Scalar (no upgrade needed)
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            if ident.as_str() == source_ident {
                *found = true;
                let has_wildcard = indices
                    .iter()
                    .any(|idx| matches!(idx, IndexExpr2::Wildcard(_)));
                if has_wildcard {
                    *result = ElementDependencyKind::CrossElement;
                    return;
                }
                // Non-wildcard subscript -> SameElement (upgrades from Scalar)
                if *result == ElementDependencyKind::Scalar {
                    *result = ElementDependencyKind::SameElement;
                }
            } else {
                // The subscripted variable is not our source, but the source
                // might appear inside the index expressions
                for idx in indices {
                    match idx {
                        IndexExpr2::Expr(e) => {
                            classify_in_expr(e, source_ident, source_is_arrayed, result, found);
                        }
                        IndexExpr2::Range(l, r, _) => {
                            classify_in_expr(l, source_ident, source_is_arrayed, result, found);
                            classify_in_expr(r, source_ident, source_is_arrayed, result, found);
                        }
                        IndexExpr2::Wildcard(_)
                        | IndexExpr2::StarRange(_, _)
                        | IndexExpr2::DimPosition(_, _) => {}
                    }
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            walk_builtin_expr(builtin, |contents| match contents {
                BuiltinContents::Ident(id, _) => {
                    if id == source_ident {
                        *found = true;
                        // Ident inside a builtin but without subscript context -> Scalar
                    }
                }
                BuiltinContents::Expr(sub_expr) => {
                    classify_in_expr(sub_expr, source_ident, source_is_arrayed, result, found);
                }
            });
        }
        Expr2::Op1(_, operand, _, _) => {
            classify_in_expr(operand, source_ident, source_is_arrayed, result, found);
        }
        Expr2::Op2(_, left, right, _, _) => {
            classify_in_expr(left, source_ident, source_is_arrayed, result, found);
            classify_in_expr(right, source_ident, source_is_arrayed, result, found);
        }
        Expr2::If(cond, then_expr, else_expr, _, _) => {
            classify_in_expr(cond, source_ident, source_is_arrayed, result, found);
            classify_in_expr(then_expr, source_ident, source_is_arrayed, result, found);
            classify_in_expr(else_expr, source_ident, source_is_arrayed, result, found);
        }
    }
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

/// Build a full CausalGraph with variables populated for polarity analysis
/// and module_graphs populated for module-containing loops.
///
/// For each dynamic module referenced by the model, recursively builds
/// the sub-model's causal graph so that polarity calculation and stock
/// enrichment can traverse module boundaries.
pub(crate) fn causal_graph_with_modules(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> crate::ltm::CausalGraph {
    use crate::common::{Canonical, Ident};
    use std::collections::HashSet;

    let edges_result = model_causal_edges(db, model, project);
    let edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = edges_result
        .edges
        .iter()
        .map(|(from, tos)| {
            (
                Ident::new(from),
                tos.iter().map(|t| Ident::new(t)).collect(),
            )
        })
        .collect();
    let stocks: HashSet<Ident<Canonical>> =
        edges_result.stocks.iter().map(|s| Ident::new(s)).collect();
    let variables = reconstruct_model_variables(db, model, project);

    let project_models = project.models(db);
    let mut module_graphs: HashMap<Ident<Canonical>, Box<crate::ltm::CausalGraph>> = HashMap::new();

    for (module_var_name, sub_model_name) in &edges_result.dynamic_modules {
        if let Some(sub_source_model) = project_models.get(sub_model_name.as_str()) {
            let sub_edges_result = model_causal_edges(db, *sub_source_model, project);
            // Only build graphs for dynamic modules (those with stocks)
            if sub_edges_result.stocks.is_empty() {
                continue;
            }
            let sub_edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>> = sub_edges_result
                .edges
                .iter()
                .map(|(from, tos)| {
                    (
                        Ident::new(from),
                        tos.iter().map(|t| Ident::new(t)).collect(),
                    )
                })
                .collect();
            let sub_stocks: HashSet<Ident<Canonical>> = sub_edges_result
                .stocks
                .iter()
                .map(|s| Ident::new(s))
                .collect();
            let sub_variables = reconstruct_model_variables(db, *sub_source_model, project);

            let sub_graph = crate::ltm::CausalGraph {
                edges: sub_edges,
                stocks: sub_stocks,
                variables: sub_variables,
                module_graphs: HashMap::new(),
            };
            module_graphs.insert(Ident::new(module_var_name), Box::new(sub_graph));
        }
    }

    crate::ltm::CausalGraph {
        edges,
        stocks,
        variables,
        module_graphs,
    }
}

/// Build the causal edge structure for a model from salsa-tracked
/// dependency sets and structural variable info.
///
/// Reads `variable_direct_dependencies` (establishing salsa dep on dep
/// sets) and `parse_source_variable_with_module_context` (for implicit variable details like
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
    let module_ctx = model_module_ident_context(db, model, vec![]);
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
        let parsed =
            parse_source_variable_with_module_context(db, *source_var, project, module_ctx);
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
    let graph = causal_graph_with_modules(db, model, project);

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

/// Compute per-link polarity for all causal edges in a model by
/// reconstructing variable ASTs from the salsa-tracked parse results
/// and analyzing how each source variable appears in the target's
/// equation.
pub fn compute_link_polarities(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<(String, String), crate::ltm::LinkPolarity> {
    let graph = causal_graph_with_modules(db, model, project);
    graph.all_link_polarities()
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
pub(crate) fn reconstruct_model_variables(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> HashMap<crate::common::Ident<crate::common::Canonical>, crate::variable::Variable> {
    use crate::common::{Canonical, Ident};

    let source_vars = model.variables(db);
    let module_ctx = model_module_ident_context(db, model, vec![]);
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
        let parsed =
            parse_source_variable_with_module_context(db, *source_var, project, module_ctx);
        let lowered = crate::model::lower_variable(&scope, &parsed.variable);
        variables.insert(Ident::new(name), lowered);

        // Add implicit variables (module instances from SMOOTH/DELAY expansion)
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            let lowered_imp =
                reconstruct_implicit_variable(db, model, &dims, &scope, implicit_dm_var);
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
    let module_ctx = model_module_ident_context(db, model, vec![]);
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
        let parsed =
            parse_source_variable_with_module_context(db, *source_var, project, module_ctx);
        let lowered = crate::model::lower_variable(&scope, &parsed.variable);
        return Some(lowered);
    }

    // Search implicit variables from all source variables
    let canonical_target: Ident<Canonical> = Ident::new(var_name);

    for (_name, source_var) in source_vars.iter() {
        let parsed =
            parse_source_variable_with_module_context(db, *source_var, project, module_ctx);
        for implicit_dm_var in &parsed.implicit_vars {
            let imp_name = canonicalize(implicit_dm_var.get_ident()).into_owned();
            if Ident::<Canonical>::new(&imp_name) == canonical_target {
                let lowered_imp =
                    reconstruct_implicit_variable(db, model, &dims, &scope, implicit_dm_var);
                return Some(lowered_imp);
            }
        }
    }

    None
}

/// Reconstruct an implicit (compiler-generated) variable from its datamodel form.
///
/// Module instances need special handling: `parse_var` does not preserve the
/// `references` list from the datamodel, so input wiring (built via
/// `build_module_inputs`) would be lost.  We short-circuit that case and
/// construct `Variable::Module` directly from the stored `ModuleReference`s.
fn reconstruct_implicit_variable(
    db: &dyn Db,
    model: SourceModel,
    dims: &[datamodel::Dimension],
    scope: &crate::model::ScopeStage0<'_>,
    implicit_dm_var: &datamodel::Variable,
) -> crate::variable::Variable {
    use crate::common::{Canonical, Ident};

    if let datamodel::Variable::Module(dm_module) = implicit_dm_var {
        let ident = Ident::<Canonical>::new(implicit_dm_var.get_ident());
        let module_var_prefix = format!("{}·", ident.as_str());
        let inputs = build_module_inputs(
            model.name(db),
            &module_var_prefix,
            dm_module
                .references
                .iter()
                .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
        );

        return crate::variable::Variable::Module {
            ident,
            model_name: Ident::new(&dm_module.model_name),
            units: None,
            inputs,
            errors: vec![],
            unit_errors: vec![],
        };
    }

    let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap_or_default();
    let mut dummy_implicits = Vec::new();
    let parsed_imp = crate::variable::parse_var(
        dims,
        implicit_dm_var,
        &mut dummy_implicits,
        &units_ctx,
        |mi| Ok(Some(mi.clone())),
    );
    crate::model::lower_variable(scope, &parsed_imp)
}

#[cfg(test)]
mod classify_element_dependency_tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Helper: build a project, sync into salsa, and classify the dependency
    /// of `source_name` as seen by `target_name`.
    ///
    /// Looks up both variables' dimensions to determine whether the source
    /// is arrayed, mirroring what the element-level graph expansion will do.
    fn classify(
        project: &TestProject,
        target_name: &str,
        source_name: &str,
    ) -> ElementDependencyKind {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        let source_vars = source_model.variables(&db);

        let target_var =
            reconstruct_single_variable(&db, source_model, source_project, target_name)
                .unwrap_or_else(|| panic!("variable '{target_name}' not found"));

        // Determine if the source variable is arrayed by checking its dimensions
        let source_is_arrayed = source_vars
            .get(source_name)
            .map(|sv| !super::super::variable_dimensions(&db, *sv, source_project).is_empty())
            .unwrap_or(false);

        classify_element_dependency(&target_var, source_name, source_is_arrayed)
    }

    #[test]
    fn scalar_reference() {
        // A simple scalar equation: growth = base * 0.1
        // "base" is referenced as a bare Var (no subscripts) and is not
        // arrayed -> Scalar
        let project = TestProject::new("scalar_ref")
            .scalar_const("base", 100.0)
            .scalar_aux("growth", "base * 0.1");

        assert_eq!(
            classify(&project, "growth", "base"),
            ElementDependencyKind::Scalar
        );
    }

    #[test]
    fn same_element_a2a_reference() {
        // A2A equation: births[Region] = population * 0.1
        // "population" is arrayed over Region and referenced without explicit
        // subscripts in an A2A context. At Expr2 level, the reference stays
        // as Expr2::Var (subscript expansion happens in Expr3), but because
        // the source is known to be arrayed, we classify as SameElement.
        let project = TestProject::new("same_element")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("population[Region]", "100")
            .array_aux("births[Region]", "population * 0.1");

        assert_eq!(
            classify(&project, "births", "population"),
            ElementDependencyKind::SameElement
        );
    }

    #[test]
    fn cross_element_wildcard_reference() {
        // total_pop = SUM(population[*])
        // "population" is referenced with a wildcard subscript -> CrossElement
        let project = TestProject::new("cross_element")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total_pop", "SUM(population[*])");

        assert_eq!(
            classify(&project, "total_pop", "population"),
            ElementDependencyKind::CrossElement
        );
    }

    #[test]
    fn a2a_with_cross_element_in_same_equation() {
        // An A2A equation that uses both same-element and cross-element:
        // share[Region] = population / SUM(population[*])
        // "population" appears both as SameElement (the numerator) and
        // CrossElement (inside SUM). CrossElement should win.
        let project = TestProject::new("mixed_dep")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("population[Region]", "100")
            .array_aux("share[Region]", "population / SUM(population[*])");

        assert_eq!(
            classify(&project, "share", "population"),
            ElementDependencyKind::CrossElement
        );
    }

    #[test]
    fn source_not_found_defaults_to_scalar() {
        // If the source ident doesn't appear in the equation at all,
        // classify_element_dependency should defensively return Scalar.
        let project = TestProject::new("not_found")
            .scalar_const("x", 1.0)
            .scalar_aux("y", "x + 1");

        assert_eq!(
            classify(&project, "y", "nonexistent"),
            ElementDependencyKind::Scalar
        );
    }

    #[test]
    fn scalar_source_in_a2a_target() {
        // growth_factor is scalar, births[Region] references it as bare Var.
        // Because growth_factor is NOT arrayed, this is Scalar.
        let project = TestProject::new("scalar_in_a2a")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .scalar_const("growth_factor", 0.1)
            .array_aux("population[Region]", "100")
            .array_aux("births[Region]", "population * growth_factor");

        assert_eq!(
            classify(&project, "births", "growth_factor"),
            ElementDependencyKind::Scalar
        );
    }
}
