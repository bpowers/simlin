// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Causal graph analysis tracked functions.
//!
//! Extracted from db.rs for file-size management. Contains:
//! - CausalEdgesResult, LoopCircuitsResult, CyclePartitionsResult
//! - ElementCausalEdgesResult, ElementDependencyKind (element-level graph)
//! - DetectedLoop, DetectedLoopsResult (polarity-aware loop detection)
//! - model_causal_edges, model_element_causal_edges, model_loop_circuits,
//!   model_cycle_partitions
//! - model_element_loop_circuits, model_element_cycle_partitions
//!   (element-level loop and partition analysis)
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
fn format_element_name(var_name: &str, element: &str) -> String {
    format!("{var_name}[{element}]")
}

/// Format an element-level node name for multi-dimensional arrays.
/// Returns `name[e1,e2,...]` (e.g., `migration[NYC,Boston]`).
fn format_multi_element_name(var_name: &str, elements: &[&str]) -> String {
    format!("{}[{}]", var_name, elements.join(","))
}

/// How a source variable is accessed at a single AST reference site.
///
/// Distinguishes bare references (in scalar or A2A context), wildcard
/// reducers (e.g., inside `SUM(x[*])`), fixed-index references
/// (e.g., `x[NYC]`), and dynamic-index references (e.g., `x[i+1]` where
/// `i` is a position iterator). The shape determines element-edge
/// emission and per-reference partial-equation construction.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum RefShape {
    /// `Expr2::Var(source, ...)` — bare variable reference. In an A2A
    /// context with an arrayed source, this is same-element. In a scalar
    /// context with a scalar source, this is a plain scalar dep.
    Bare,
    /// `Expr2::Subscript(source, [literal_elem_or_int_lit, ...])` —
    /// every index is a literal element name or integer literal. The
    /// `Vec<String>` carries the resolved element names per dimension
    /// in source order (canonical lowercase).
    FixedIndex(Vec<String>),
    /// `Expr2::Subscript(source, indices)` where at least one index is
    /// `IndexExpr2::Wildcard`. Conservative full cross-product.
    Wildcard,
    /// `Expr2::Subscript(source, indices)` where at least one index is
    /// a non-literal expression (`@N`, `Range`, `StarRange`, or
    /// arbitrary `Expr`). Conservative full cross-product.
    DynamicIndex,
}

/// One occurrence of a source variable in a target's AST.
#[derive(Debug, Clone)]
pub(crate) struct ReferenceSite {
    // Fields are read by Task 2's tests but not by production code yet;
    // Task 4's pivot will consume them in `model_element_causal_edges`.
    #[allow(dead_code)]
    pub source: String,
    #[allow(dead_code)]
    pub shape: RefShape,
}

/// How a source variable is referenced in a target's equation.
///
/// When expanding variable-level causal edges to element-level edges,
/// the dependency kind determines the expansion pattern:
/// - `Scalar`: one-to-one or broadcast (no subscripts involved)
/// - `SameElement`: A2A same-element reference (bare `Var` node with array bounds)
/// - `CrossElement`: reducer over all elements (e.g., `SUM(population[*])`)
///   or fixed-index reference to a specific element (e.g., `population[Boston]`)
#[derive(Debug, Clone, PartialEq, Eq)]
enum ElementDependencyKind {
    /// Scalar reference: source appears as a bare variable with no subscripts
    Scalar,
    /// Same-element A2A reference: source appears as a bare `Var` node with
    /// `ArrayBounds` (at the Expr2 level, A2A same-element references are NOT
    /// lowered to `Subscript` nodes; subscript expansion happens in the Expr3 phase).
    SameElement,
    /// Cross-element reference: source appears with a wildcard subscript
    /// (e.g., `population[*]` inside a reducer like SUM or MEAN), or with a
    /// non-wildcard explicit subscript (e.g., `population[Boston]`) which is
    /// a fixed-index reference to a specific element. Both patterns create
    /// non-diagonal edges in the element graph.
    CrossElement,
}

/// Classify how a source variable is referenced in a target variable's equation.
///
/// Walks the target variable's lowered AST (`Expr2` level) looking for
/// references to the source identifier. The classification is:
/// - `CrossElement` if the source appears inside an `Expr2::Subscript` node
///   with any `IndexExpr2::Wildcard` index (from `x[*]` syntax), OR inside
///   a `Subscript` with all non-wildcard indices (fixed-index reference like
///   `source[Boston]` — at the Expr2 level, same-element A2A references stay
///   as bare `Var` nodes, so explicit `Subscript` nodes are always fixed-index)
/// - `SameElement` if the source is arrayed and appears as a bare `Expr2::Var`
///   in an A2A equation context (at Expr2 level, A2A variable references
///   retain their Var form; subscript expansion happens later in Expr3)
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
                // Non-wildcard explicit subscript (e.g., `source[Boston]`).
                // At the Expr2 level, same-element A2A references stay as
                // bare Var nodes (documented in classify_element_dependency);
                // explicit Subscript nodes with non-wildcard indices are
                // fixed-index references to specific elements. Classify as
                // CrossElement because the target depends on a specific
                // source element, not the corresponding same-index element.
                // CrossElement expansion creates all NxN edges, which is a
                // superset of the true dependency; the link scores for
                // non-referenced source elements will be effectively zero.
                *result = ElementDependencyKind::CrossElement;
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

/// Resolve a single subscript index to a literal element name (canonical
/// lowercase) if it matches one of the source's dimensions, or `None`
/// for any other shape (wildcard, range, position, non-literal
/// expression, or a literal that doesn't match a known element).
///
/// Used by `collect_reference_sites` to classify `Subscript` shapes:
/// every index in a `FixedIndex` must resolve via this helper. If any
/// index fails to resolve, the subscript falls back to `DynamicIndex` --
/// or `Wildcard` if a wildcard is present (wildcards are checked first
/// in the caller).
///
/// Element names parse as `Expr2::Var(ident, ...)` (the parser keeps the
/// raw element identifier as a Var; dimension-resolution into a numeric
/// offset happens later, in Expr3 lowering). Integer literals (used for
/// indexed dimensions like `1`, `2`) parse as `Expr2::Const`. We accept
/// both forms.
///
/// Note: `source_dims` is the source variable's *full* dimension list.
/// In multidimensional subscripts the caller doesn't know which
/// dimension a literal belongs to; we accept the first dimension whose
/// element registry contains the canonical name. Literal indices that
/// don't match any known element classify defensively as `DynamicIndex`,
/// so the worst case is over-conservative (full cross-product) edges.
#[allow(dead_code)] // exercised by Task 2's tests and Task 4's pivot
fn resolve_literal_index(
    idx: &crate::ast::IndexExpr2,
    source_dims: &[crate::dimensions::Dimension],
) -> Option<String> {
    use crate::ast::{Expr2, IndexExpr2};

    // Element names appear as `Var(ident, ...)`; integer literals appear
    // as `Const(text, value, _)`. Anything else (wildcards, ranges, dim
    // positions, or compound expressions) is not a literal element.
    let canonical = match idx {
        IndexExpr2::Expr(Expr2::Var(ident, _, _)) => ident.as_str().to_string(),
        IndexExpr2::Expr(Expr2::Const(text, _, _)) => canonicalize(text).into_owned(),
        _ => return None,
    };

    for dim in source_dims {
        match dim {
            crate::dimensions::Dimension::Named(_, named) => {
                if named.elements.iter().any(|e| e.as_str() == canonical) {
                    return Some(canonical);
                }
            }
            crate::dimensions::Dimension::Indexed(_, size) => {
                // Indexed dimensions accept integer literals in the range [1, size].
                if let Ok(n) = canonical.parse::<u32>()
                    && n >= 1
                    && n <= *size
                {
                    return Some(canonical);
                }
            }
        }
    }
    None
}

/// Walk a target variable's AST and emit one `ReferenceSite` per occurrence
/// of `source_ident`. Mirrors the recursion pattern of `classify_in_expr`
/// but accumulates per-site shapes instead of folding them into a single
/// classification.
///
/// Subscript shape classification rules:
/// - any `IndexExpr2::Wildcard(_)` index → `Wildcard`
/// - all indices resolve via `resolve_literal_index` → `FixedIndex(names)`
/// - any other pattern (`StarRange`, `DimPosition`, `Range`, non-literal
///   `Expr`, or a literal that doesn't match a known element name) →
///   `DynamicIndex`
///
/// Bare `Var` references push `RefShape::Bare`. The shape is independent
/// of whether the source is arrayed -- edge emission resolves
/// scalar-vs-arrayed semantics from the source/target dimension lists.
///
/// `App` arguments are walked via `walk_builtin_expr`; `BuiltinContents::Ident`
/// matches contribute a `Bare` site (the builtin doesn't subscript its
/// ident argument). The walker also recurses into each `Subscript` index
/// expression so nested references like `source_outer[source_inner[*]]`
/// emit a site for the inner reference.
///
/// Return order is the AST-walk order. Duplicate sites with identical
/// `(source, shape)` are kept; downstream emission deduplicates edges
/// implicitly via the `BTreeSet` value type, but the per-site count may
/// matter for callers that use sites as a metric.
#[allow(dead_code)] // wired into model_element_causal_edges in Task 4
fn collect_reference_sites(
    target_var: &crate::variable::Variable,
    source_ident: &str,
    source_is_arrayed: bool,
    source_dims: &[crate::dimensions::Dimension],
) -> Vec<ReferenceSite> {
    let Some(ast) = target_var.ast() else {
        return Vec::new();
    };

    let mut sites = Vec::new();
    match ast {
        crate::ast::Ast::Scalar(expr) | crate::ast::Ast::ApplyToAll(_, expr) => {
            collect_in_expr(
                expr,
                source_ident,
                source_is_arrayed,
                source_dims,
                &mut sites,
            );
        }
        crate::ast::Ast::Arrayed(_, subscript_map, default_expr, _) => {
            for expr in subscript_map.values() {
                collect_in_expr(
                    expr,
                    source_ident,
                    source_is_arrayed,
                    source_dims,
                    &mut sites,
                );
            }
            if let Some(default) = default_expr {
                collect_in_expr(
                    default,
                    source_ident,
                    source_is_arrayed,
                    source_dims,
                    &mut sites,
                );
            }
        }
    }
    sites
}

/// Recursively walk an `Expr2` tree, pushing one `ReferenceSite` for each
/// reference to `source_ident`. See `collect_reference_sites` for the
/// shape-classification rules.
///
/// `source_is_arrayed` is threaded through for callers that need it
/// during recursion-local rewriting (currently a no-op here -- shape
/// classification is determined by the AST node and `source_dims`),
/// matching the documented public signature.
#[allow(dead_code, clippy::only_used_in_recursion)] // helper for collect_reference_sites
fn collect_in_expr(
    expr: &crate::ast::Expr2,
    source_ident: &str,
    source_is_arrayed: bool,
    source_dims: &[crate::dimensions::Dimension],
    sites: &mut Vec<ReferenceSite>,
) {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::builtins::{BuiltinContents, walk_builtin_expr};

    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _array_bounds, _) => {
            if ident.as_str() == source_ident {
                sites.push(ReferenceSite {
                    source: source_ident.to_string(),
                    shape: RefShape::Bare,
                });
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            if ident.as_str() == source_ident {
                let shape = classify_subscript_shape(indices, source_dims);
                sites.push(ReferenceSite {
                    source: source_ident.to_string(),
                    shape,
                });
            }
            // Always recurse into index expressions so nested references
            // like `source_outer[source_inner[*]]` (or arbitrary index
            // arithmetic mentioning the source) still emit per-site
            // entries. This matches the existing classify_in_expr
            // behavior for non-matching subscript heads.
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => {
                        collect_in_expr(e, source_ident, source_is_arrayed, source_dims, sites);
                    }
                    IndexExpr2::Range(l, r, _) => {
                        collect_in_expr(l, source_ident, source_is_arrayed, source_dims, sites);
                        collect_in_expr(r, source_ident, source_is_arrayed, source_dims, sites);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            walk_builtin_expr(builtin, |contents| match contents {
                BuiltinContents::Ident(id, _) => {
                    if id == source_ident {
                        sites.push(ReferenceSite {
                            source: source_ident.to_string(),
                            shape: RefShape::Bare,
                        });
                    }
                }
                BuiltinContents::Expr(sub_expr) => {
                    collect_in_expr(
                        sub_expr,
                        source_ident,
                        source_is_arrayed,
                        source_dims,
                        sites,
                    );
                }
            });
        }
        Expr2::Op1(_, operand, _, _) => {
            collect_in_expr(operand, source_ident, source_is_arrayed, source_dims, sites);
        }
        Expr2::Op2(_, left, right, _, _) => {
            collect_in_expr(left, source_ident, source_is_arrayed, source_dims, sites);
            collect_in_expr(right, source_ident, source_is_arrayed, source_dims, sites);
        }
        Expr2::If(cond, then_expr, else_expr, _, _) => {
            collect_in_expr(cond, source_ident, source_is_arrayed, source_dims, sites);
            collect_in_expr(
                then_expr,
                source_ident,
                source_is_arrayed,
                source_dims,
                sites,
            );
            collect_in_expr(
                else_expr,
                source_ident,
                source_is_arrayed,
                source_dims,
                sites,
            );
        }
    }
}

/// Classify a subscript's indices into a `RefShape`.
///
/// Wildcard takes precedence: if any index is `IndexExpr2::Wildcard`,
/// the shape is `Wildcard` (conservative full cross-product).
/// Otherwise, every index must resolve via `resolve_literal_index` for
/// the shape to be `FixedIndex`. Any other index pattern (or an
/// unrecognized literal) falls back to `DynamicIndex`.
#[allow(dead_code)] // helper for collect_in_expr
fn classify_subscript_shape(
    indices: &[crate::ast::IndexExpr2],
    source_dims: &[crate::dimensions::Dimension],
) -> RefShape {
    use crate::ast::IndexExpr2;

    if indices.iter().any(|i| matches!(i, IndexExpr2::Wildcard(_))) {
        return RefShape::Wildcard;
    }

    let mut resolved: Vec<String> = Vec::with_capacity(indices.len());
    for idx in indices {
        match resolve_literal_index(idx, source_dims) {
            Some(name) => resolved.push(name),
            None => return RefShape::DynamicIndex,
        }
    }
    RefShape::FixedIndex(resolved)
}

/// Collect element names from a dimension as owned strings.
///
/// Delegates to the canonical implementation in `ltm_augment`.
fn dimension_element_names(dim: &crate::dimensions::Dimension) -> Vec<String> {
    crate::ltm_augment::dimension_element_names(dim)
}

/// Expand a single variable-level edge into element-level edges.
///
/// Uses the source/target dimensions and dependency classification to
/// determine the expansion pattern. The rules are:
///
/// | from_dims | to_dims    | dep_kind     | Expansion                                    |
/// |-----------|------------|--------------|----------------------------------------------|
/// | []        | []         | Scalar       | from -> to (unchanged)                       |
/// | []        | [D...]     | Scalar       | from -> to[d] for each element d             |
/// | [D...]    | []         | any          | from[d] -> to for each element d             |
/// | [D]       | [D]        | SameElement  | from[d] -> to[d] for each d                  |
/// | [D]       | [D]        | CrossElement | from[d] -> to[e] for all d,e (full cross)    |
/// | [D1,D2]   | [D1]       | SameElement  | from[d1,d2] -> to[d1] for all (d1,d2)        |
///
/// When both source and target are arrayed and dep_kind is CrossElement,
/// every source element connects to every target element (a SUM(x[*])
/// inside an A2A equation means each source element contributes to the
/// scalar reduction, which then feeds all target elements).
fn expand_edge_to_elements(
    from_name: &str,
    to_name: &str,
    from_dims: &[crate::dimensions::Dimension],
    to_dims: &[crate::dimensions::Dimension],
    dep_kind: ElementDependencyKind,
    element_edges: &mut HashMap<String, BTreeSet<String>>,
) {
    let from_is_scalar = from_dims.is_empty();
    let to_is_scalar = to_dims.is_empty();

    // Case 1: Both scalar -- pass through unchanged
    if from_is_scalar && to_is_scalar {
        element_edges
            .entry(from_name.to_string())
            .or_default()
            .insert(to_name.to_string());
        return;
    }

    // Case 2: Scalar source, arrayed target -- broadcast
    // from -> to[d] for each element d across all target dimensions
    if from_is_scalar {
        let to_elements = cartesian_element_names(to_name, to_dims);
        for to_elem in to_elements {
            element_edges
                .entry(from_name.to_string())
                .or_default()
                .insert(to_elem);
        }
        return;
    }

    // Case 3: Arrayed source, scalar target -- reduction
    // from[d] -> to for each element d across all source dimensions
    if to_is_scalar {
        let from_elements = cartesian_element_names(from_name, from_dims);
        for from_elem in from_elements {
            element_edges
                .entry(from_elem)
                .or_default()
                .insert(to_name.to_string());
        }
        return;
    }

    // Both arrayed: expansion depends on dep_kind
    match dep_kind {
        ElementDependencyKind::Scalar => {
            // Scalar reference in an arrayed context (e.g., a scalar constant
            // that the dependency tracker found -- shouldn't normally happen
            // when both are arrayed, but handle defensively). Treat as
            // broadcast: from[d] -> to[e] for all d,e.
            let from_elements = cartesian_element_names(from_name, from_dims);
            let to_elements = cartesian_element_names(to_name, to_dims);
            for from_elem in &from_elements {
                for to_elem in &to_elements {
                    element_edges
                        .entry(from_elem.clone())
                        .or_default()
                        .insert(to_elem.clone());
                }
            }
        }
        ElementDependencyKind::SameElement => {
            // Same-element mapping. Shared dimensions are iterated element-wise;
            // non-shared dimensions produce a partial collapse.
            //
            // Simple case: identical dimension lists -> from[d] -> to[d] per element.
            // Partial collapse: from[D1,D2] -> to[D1] -> from[d1,d2] -> to[d1].
            //
            // We generate the cartesian product of source elements and map each
            // source element tuple to the target element tuple by keeping only
            // the dimensions present in the target.
            expand_same_element(from_name, to_name, from_dims, to_dims, element_edges);
        }
        ElementDependencyKind::CrossElement => {
            // Cross-element: every source element connects to every target element.
            // This represents a reducer like SUM(from[*]) inside the target's
            // equation: each source element contributes to the reduction, whose
            // scalar result feeds all target elements.
            let from_elements = cartesian_element_names(from_name, from_dims);
            let to_elements = cartesian_element_names(to_name, to_dims);
            for from_elem in &from_elements {
                for to_elem in &to_elements {
                    element_edges
                        .entry(from_elem.clone())
                        .or_default()
                        .insert(to_elem.clone());
                }
            }
        }
    }
}

/// Generate element-level node names for the cartesian product of all dimensions.
///
/// For a variable `x` with dimensions `[D1, D2]` where D1 = {a, b} and D2 = {1, 2},
/// produces: `["x[a,1]", "x[a,2]", "x[b,1]", "x[b,2]"]`.
///
/// For a single dimension `[D]` where D = {NYC, Boston}, produces:
/// `["x[NYC]", "x[Boston]"]`.
fn cartesian_element_names(var_name: &str, dims: &[crate::dimensions::Dimension]) -> Vec<String> {
    if dims.is_empty() {
        return vec![var_name.to_string()];
    }

    // Build element name lists for each dimension
    let dim_elements: Vec<Vec<String>> = dims.iter().map(dimension_element_names).collect();

    // Compute cartesian product
    let mut tuples: Vec<Vec<&str>> = vec![vec![]];
    for elements in &dim_elements {
        let mut new_tuples = Vec::with_capacity(tuples.len() * elements.len());
        for existing in &tuples {
            for elem in elements {
                let mut extended = existing.clone();
                extended.push(elem.as_str());
                new_tuples.push(extended);
            }
        }
        tuples = new_tuples;
    }

    tuples
        .into_iter()
        .map(|elems| {
            if elems.len() == 1 {
                format_element_name(var_name, elems[0])
            } else {
                format_multi_element_name(var_name, &elems)
            }
        })
        .collect()
}

/// Expand same-element edges with possible partial dimension collapse.
///
/// For each source element tuple, constructs the target element tuple by
/// matching shared dimension names. Dimensions in the source that are not
/// present in the target are collapsed (their elements are iterated but
/// do not appear in the target subscript).
///
/// Example: from[D1,D2] -> to[D1] with SameElement produces
/// from[d1,d2] -> to[d1] for all (d1,d2).
fn expand_same_element(
    from_name: &str,
    to_name: &str,
    from_dims: &[crate::dimensions::Dimension],
    to_dims: &[crate::dimensions::Dimension],
    element_edges: &mut HashMap<String, BTreeSet<String>>,
) {
    // Build a map of target dimension name -> position for matching
    let to_dim_positions: HashMap<&str, usize> = to_dims
        .iter()
        .enumerate()
        .map(|(i, d)| (d.name(), i))
        .collect();

    // For each source dimension, record which target dimension position
    // it corresponds to (if any). Dimensions in the source not found in
    // the target are "collapsed" (iterated but not projected).
    let from_to_target_pos: Vec<Option<usize>> = from_dims
        .iter()
        .map(|d| to_dim_positions.get(d.name()).copied())
        .collect();

    // Build element name lists for each dimension
    let from_dim_elements: Vec<Vec<String>> =
        from_dims.iter().map(dimension_element_names).collect();
    let to_dim_elements: Vec<Vec<String>> = to_dims.iter().map(dimension_element_names).collect();
    let to_dim_count = to_dims.len();

    // Compute cartesian product of source elements
    let mut from_tuples: Vec<Vec<usize>> = vec![vec![]];
    for elements in &from_dim_elements {
        let mut new_tuples = Vec::with_capacity(from_tuples.len() * elements.len());
        for existing in &from_tuples {
            for idx in 0..elements.len() {
                let mut extended = existing.clone();
                extended.push(idx);
                new_tuples.push(extended);
            }
        }
        from_tuples = new_tuples;
    }

    for from_indices in &from_tuples {
        // Build source element name
        let from_elems: Vec<&str> = from_indices
            .iter()
            .enumerate()
            .map(|(dim_idx, &elem_idx)| from_dim_elements[dim_idx][elem_idx].as_str())
            .collect();
        let from_node = if from_elems.len() == 1 {
            format_element_name(from_name, from_elems[0])
        } else {
            format_multi_element_name(from_name, &from_elems)
        };

        // Build target element name by projecting shared dimensions
        let mut to_elems: Vec<&str> = vec![""; to_dim_count];
        let mut all_mapped = true;
        for (src_dim_idx, target_pos) in from_to_target_pos.iter().enumerate() {
            if let Some(pos) = target_pos {
                let src_elem_idx = from_indices[src_dim_idx];
                // Use the element name from the target dimension at the
                // corresponding position. If the source element index is
                // out of range for the target dimension (dimension size
                // mismatch), fall back to the source element name.
                to_elems[*pos] = if src_elem_idx < to_dim_elements[*pos].len() {
                    &to_dim_elements[*pos][src_elem_idx]
                } else {
                    from_dim_elements[src_dim_idx][src_elem_idx].as_str()
                };
            }
        }

        // Check if all target dimensions got filled from shared source dims
        for elem in to_elems.iter().take(to_dim_count) {
            if elem.is_empty() {
                all_mapped = false;
                break;
            }
        }

        if all_mapped {
            let to_node = if to_elems.len() == 1 {
                format_element_name(to_name, to_elems[0])
            } else {
                format_multi_element_name(to_name, &to_elems)
            };
            element_edges.entry(from_node).or_default().insert(to_node);
        } else {
            // If some target dimensions are not covered by the source,
            // we need to iterate over those target dimensions too (broadcast).
            // Collect the unfilled target dimension indices and their elements.
            let unfilled: Vec<(usize, &Vec<String>)> = (0..to_dim_count)
                .filter(|&pos| to_elems[pos].is_empty())
                .map(|pos| (pos, &to_dim_elements[pos]))
                .collect();

            // Cartesian product of unfilled target dimensions
            let mut unfilled_tuples: Vec<Vec<(usize, usize)>> = vec![vec![]];
            for &(pos, elements) in &unfilled {
                let mut new_tuples = Vec::with_capacity(unfilled_tuples.len() * elements.len());
                for existing in &unfilled_tuples {
                    for elem_idx in 0..elements.len() {
                        let mut extended = existing.clone();
                        extended.push((pos, elem_idx));
                        new_tuples.push(extended);
                    }
                }
                unfilled_tuples = new_tuples;
            }

            for fill in &unfilled_tuples {
                let mut filled = to_elems.clone();
                for &(pos, elem_idx) in fill {
                    filled[pos] = &to_dim_elements[pos][elem_idx];
                }
                let to_node = if filled.len() == 1 {
                    format_element_name(to_name, filled[0])
                } else {
                    format_multi_element_name(to_name, &filled)
                };
                element_edges
                    .entry(from_node.clone())
                    .or_default()
                    .insert(to_node);
            }
        }
    }
}

/// Deduplicated loop circuits in an indexed form.
///
/// Flat `Vec<Vec<String>>` was O(circuits × path_len) in owned-string
/// allocations, which dominated RSS on dense graphs like WRLD3 where a
/// single 166-node SCC produced ~1.86M circuits × 47 nodes ≈ 87M strings
/// over only ~166 distinct names.  The indexed form keeps a single shared
/// `names` table (one `String` per unique node) plus `circuits` as
/// `Vec<Vec<u32>>`; reconstructing named circuits is a one-liner lookup.
///
/// Consumers that need the legacy `Vec<Vec<String>>` view can call
/// [`LoopCircuitsResult::to_named_circuits`].  Prefer
/// [`LoopCircuitsResult::circuit_names`] or direct index iteration when
/// you only need to read the names.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct LoopCircuitsResult {
    /// Unique variable names referenced by any circuit.  The integer
    /// values inside `circuits` index into this vector.  Names are in
    /// the canonical (lex-sorted) node ordering produced by the indexed
    /// enumerator so identical models deterministically produce identical
    /// results -- a prerequisite for salsa's pointer-equal caching.
    pub names: Vec<String>,
    /// Each circuit is a deduplicated sequence of indices into `names`.
    /// Circuits are emitted in the enumerator's deterministic order.
    pub circuits: Vec<Vec<u32>>,
}

impl LoopCircuitsResult {
    /// Number of circuits.  Convenience wrapper around `circuits.len()`.
    pub fn len(&self) -> usize {
        self.circuits.len()
    }

    /// True when no circuits were found (or the enumerator exhausted its
    /// budget and returned an empty placeholder).
    pub fn is_empty(&self) -> bool {
        self.circuits.is_empty()
    }

    /// Iterate the variable names of circuit `idx` as `&str` slices
    /// without allocating a per-node `String`.
    ///
    /// Panics if `idx >= self.len()`, matching the behavior of a direct
    /// `self.circuits[idx]` index.
    pub fn circuit_names(&self, idx: usize) -> impl Iterator<Item = &str> {
        self.circuits[idx]
            .iter()
            .map(|&i| self.names[i as usize].as_str())
    }

    /// Materialize the legacy `Vec<Vec<String>>` view.  Allocates one
    /// `String` per referenced node; only use in tests or at API
    /// boundaries that require owned strings -- prefer `circuit_names`
    /// or index-based iteration otherwise.
    pub fn to_named_circuits(&self) -> Vec<Vec<String>> {
        self.circuits
            .iter()
            .map(|c| c.iter().map(|&i| self.names[i as usize].clone()).collect())
            .collect()
    }
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
pub fn causal_graph_from_edges(result: &CausalEdgesResult) -> crate::ltm::CausalGraph {
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

/// Build the element-level causal graph for a model.
///
/// Expands variable-level edges from `model_causal_edges` into element-level
/// edges based on each variable's dimensions and the dependency classification
/// (same-element, cross-element, or scalar). Stock names are similarly expanded
/// to per-element nodes.
///
/// When no variables in the model are arrayed, the element graph is identical
/// to the variable graph (zero overhead -- edges and stocks are cloned directly).
#[salsa::tracked(returns(ref))]
pub fn model_element_causal_edges(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> ElementCausalEdgesResult {
    let variable_edges = model_causal_edges(db, model, project);
    let source_vars = model.variables(db);

    // Check if any variable in the model is arrayed. If none are,
    // short-circuit: the element graph is identical to the variable graph.
    let any_arrayed = source_vars
        .values()
        .any(|sv| !super::variable_dimensions(db, *sv, project).is_empty());
    if !any_arrayed {
        return ElementCausalEdgesResult {
            edges: variable_edges.edges.clone(),
            stocks: variable_edges.stocks.clone(),
        };
    }

    let mut element_edges: HashMap<String, BTreeSet<String>> = HashMap::new();

    // Cache dimension lookups to avoid repeated calls for the same variable
    let mut dim_cache: HashMap<String, Vec<crate::dimensions::Dimension>> = HashMap::new();

    let lookup_dims = |name: &str,
                       cache: &mut HashMap<String, Vec<crate::dimensions::Dimension>>|
     -> Vec<crate::dimensions::Dimension> {
        if let Some(dims) = cache.get(name) {
            return dims.clone();
        }
        let dims = source_vars
            .get(name)
            .map(|sv| super::variable_dimensions(db, *sv, project).to_vec())
            .unwrap_or_default();
        cache.insert(name.to_string(), dims.clone());
        dims
    };

    // Build a set of structural flow->stock edges so we can skip AST
    // classification for them. Stock equations contain only the initial
    // value, so the flow name never appears in the stock's AST. Without
    // this check, classification defaults to Scalar, which produces
    // incorrect all-to-all expansion for arrayed stocks.
    let mut structural_flow_to_stock: BTreeSet<(String, String)> = BTreeSet::new();
    for (stock_name, source_var) in source_vars.iter() {
        if source_var.kind(db) == super::SourceVariableKind::Stock {
            for flow in source_var
                .inflows(db)
                .iter()
                .chain(source_var.outflows(db).iter())
            {
                let canonical_flow = canonicalize(flow).into_owned();
                structural_flow_to_stock.insert((canonical_flow, stock_name.clone()));
            }
        }
    }

    // Expand each variable-level edge to element-level edges
    for (from_name, to_set) in &variable_edges.edges {
        let from_dims = lookup_dims(from_name, &mut dim_cache);
        for to_name in to_set {
            let to_dims = lookup_dims(to_name, &mut dim_cache);

            // Fast path: both scalar -> direct edge
            if from_dims.is_empty() && to_dims.is_empty() {
                element_edges
                    .entry(from_name.clone())
                    .or_default()
                    .insert(to_name.clone());
                continue;
            }

            // Structural flow->stock edges use SameElement when both are
            // arrayed. The stock's equation is just the initial value, so
            // the flow name never appears in the AST; AST-based
            // classification would incorrectly default to Scalar.
            let dep_kind = if structural_flow_to_stock
                .contains(&(from_name.clone(), to_name.clone()))
                && !from_dims.is_empty()
                && !to_dims.is_empty()
            {
                ElementDependencyKind::SameElement
            } else {
                // Classify the dependency by inspecting the target's AST
                // to determine how the source appears in the equation.
                match reconstruct_single_variable(db, model, project, to_name) {
                    Some(target_var) => {
                        let source_is_arrayed = !from_dims.is_empty();
                        classify_element_dependency(&target_var, from_name, source_is_arrayed)
                    }
                    None => {
                        // If we can't reconstruct the variable (shouldn't happen
                        // for well-formed models), default to Scalar
                        ElementDependencyKind::Scalar
                    }
                }
            };

            expand_edge_to_elements(
                from_name,
                to_name,
                &from_dims,
                &to_dims,
                dep_kind,
                &mut element_edges,
            );
        }
    }

    // Expand stock names to element-level
    let mut element_stocks = BTreeSet::new();
    for stock_name in &variable_edges.stocks {
        let stock_dims = lookup_dims(stock_name, &mut dim_cache);
        if stock_dims.is_empty() {
            element_stocks.insert(stock_name.clone());
        } else {
            for elem_name in cartesian_element_names(stock_name, &stock_dims) {
                element_stocks.insert(elem_name);
            }
        }
    }

    ElementCausalEdgesResult {
        edges: element_edges,
        stocks: element_stocks,
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
    let (names, circuits) = graph
        .find_indexed_circuits_with_limit(usize::MAX)
        .expect("usize::MAX cannot exhaust the enumeration budget");
    LoopCircuitsResult { names, circuits }
}

/// Detect feedback loops with polarity analysis and deterministic IDs.
///
/// Builds a full CausalGraph from salsa-tracked causal edges and
/// reconstructed variable ASTs, then runs Johnson's algorithm with
/// polarity analysis. Loop IDs (r1, b1, u1, ...) match those used
/// by LTM augmentation.
///
/// Short-circuits to an empty result when the graph's largest SCC
/// exceeds [`crate::ltm::MAX_LTM_SCC_NODES`], for the same reason
/// the LTM pipeline's gate does: Johnson's enumeration on an SCC
/// larger than the threshold can produce millions of elementary
/// circuits (1.86M on WRLD3's 166-node SCC) and consume gigabytes
/// of intermediate state.  FFI callers (`simlin_analyze_get_loops`)
/// and the layout path (`layout::try_detect_ltm_loops_incremental`)
/// hit this function directly without going through the LTM gate,
/// so we apply the same structural guard here.  Returning empty
/// matches the pre-existing behaviour for graphs that would
/// exhaust the (now-retired) `MAX_LTM_CIRCUITS` cap.
pub fn model_detected_loops(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> DetectedLoopsResult {
    let graph = causal_graph_with_modules(db, model, project);

    if graph.largest_scc_size() > crate::ltm::MAX_LTM_SCC_NODES {
        return DetectedLoopsResult { loops: vec![] };
    }

    let loops = graph
        .find_loops_with_limit(usize::MAX)
        .expect("usize::MAX cannot exhaust the enumeration budget");
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

/// Construct a lightweight CausalGraph from an ElementCausalEdgesResult.
///
/// Same conversion as `causal_graph_from_edges` but uses element-level edges
/// and stocks. Variables and module_graphs are empty -- suitable for circuit
/// finding and SCC computation but not for polarity analysis.
pub fn causal_graph_from_element_edges(
    result: &ElementCausalEdgesResult,
) -> crate::ltm::CausalGraph {
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

/// Find all elementary loop circuits in a model's element-level causal graph.
///
/// For models with arrayed variables, this finds element-specific loops
/// (e.g., `population[NYC] -> births[NYC] -> population[NYC]`) and
/// cross-element loops (e.g., `population[NYC] -> migration -> population[Boston]`).
/// For scalar models, results are identical to `model_loop_circuits`.
#[salsa::tracked(returns(ref))]
pub fn model_element_loop_circuits(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LoopCircuitsResult {
    let element_edges = model_element_causal_edges(db, model, project);
    let graph = causal_graph_from_element_edges(element_edges);
    let (names, circuits) = graph
        .find_indexed_circuits_with_limit(usize::MAX)
        .expect("usize::MAX cannot exhaust the enumeration budget");
    LoopCircuitsResult { names, circuits }
}

/// Compute stock-to-stock cycle partitions at element granularity.
///
/// Element-level stocks like `population[NYC]` and `population[Boston]`
/// may be in the same partition (connected through cross-element feedback
/// like migration) or different partitions (if no cross-element feedback
/// exists). For scalar models, identical to `model_cycle_partitions`.
#[salsa::tracked(returns(ref))]
pub fn model_element_cycle_partitions(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> CyclePartitionsResult {
    let element_edges = model_element_causal_edges(db, model, project);
    let graph = causal_graph_from_element_edges(element_edges);
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

    #[test]
    fn fixed_index_reference_is_cross_element() {
        // relative_pop[Region] = population / population[NYC]
        // "population" in the denominator has a fixed-index subscript [NYC].
        // At the Expr2 level, same-element A2A references are bare Var nodes;
        // explicit Subscript nodes with non-wildcard indices are fixed-index
        // references. These should be classified as CrossElement, not SameElement,
        // because the dependency is from a specific source element (NYC) to all
        // target elements, not diagonal.
        let project = TestProject::new("fixed_index")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("population[Region]", "100")
            .array_aux("relative_pop[Region]", "population / population[NYC]");

        // The equation has both a bare Var "population" (SameElement) and a
        // fixed-index Subscript "population[NYC]" (CrossElement).
        // CrossElement wins since it's highest priority.
        assert_eq!(
            classify(&project, "relative_pop", "population"),
            ElementDependencyKind::CrossElement
        );
    }
}

#[cfg(test)]
mod collect_reference_sites_tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Helper: build a project, sync into salsa, and collect reference sites
    /// for `source_name` as seen by `target_name`. Resolves the source's
    /// `is_arrayed` flag and dimension list from the live salsa results so
    /// the walker can validate literal subscripts against real elements.
    fn collect(project: &TestProject, target_name: &str, source_name: &str) -> Vec<ReferenceSite> {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        let source_vars = source_model.variables(&db);

        let target_var =
            reconstruct_single_variable(&db, source_model, source_project, target_name)
                .unwrap_or_else(|| panic!("variable '{target_name}' not found"));

        let source_dims: Vec<crate::dimensions::Dimension> = source_vars
            .get(source_name)
            .map(|sv| super::super::variable_dimensions(&db, *sv, source_project).to_vec())
            .unwrap_or_default();
        let source_is_arrayed = !source_dims.is_empty();

        collect_reference_sites(&target_var, source_name, source_is_arrayed, &source_dims)
    }

    #[test]
    fn ref_site_bare_a2a() {
        // A2A equation: births[Region] = population * 0.1
        // The bare `population` reference is one occurrence with shape Bare.
        let project = TestProject::new("bare_a2a")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("births[Region]", "population * 0.1");

        let sites = collect(&project, "births", "population");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert_eq!(sites[0].source, "population");
        assert_eq!(sites[0].shape, RefShape::Bare);
    }

    #[test]
    fn ref_site_fixed_index() {
        // relative_pop[Region] = population / population[NYC]
        // Two occurrences: a bare `population` (numerator) and a
        // FixedIndex `population[NYC]` (denominator).
        let project = TestProject::new("fixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("relative_pop[Region]", "population / population[NYC]");

        let sites = collect(&project, "relative_pop", "population");
        assert_eq!(sites.len(), 2, "sites: {sites:?}");
        // AST-walk order: numerator first (bare), denominator second (FixedIndex).
        assert_eq!(sites[0].shape, RefShape::Bare);
        assert_eq!(
            sites[1].shape,
            RefShape::FixedIndex(vec!["nyc".to_string()])
        );
    }

    #[test]
    fn ref_site_wildcard_reducer() {
        // total = SUM(population[*])
        // The wildcard subscript inside the reducer produces one Wildcard site.
        let project = TestProject::new("wild")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total", "SUM(population[*])");

        let sites = collect(&project, "total", "population");
        assert_eq!(sites.len(), 1, "sites: {sites:?}");
        assert_eq!(sites[0].shape, RefShape::Wildcard);
    }

    #[test]
    fn ref_site_mixed_bare_and_wildcard() {
        // share[Region] = population / SUM(population[*])
        // Two occurrences: a bare numerator and a wildcard reducer denominator.
        let project = TestProject::new("mixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("share[Region]", "population / SUM(population[*])");

        let sites = collect(&project, "share", "population");
        assert_eq!(sites.len(), 2, "sites: {sites:?}");
        let shapes: Vec<&RefShape> = sites.iter().map(|s| &s.shape).collect();
        assert!(
            shapes.contains(&&RefShape::Bare),
            "expected Bare in {shapes:?}"
        );
        assert!(
            shapes.contains(&&RefShape::Wildcard),
            "expected Wildcard in {shapes:?}"
        );
    }
}

#[cfg(test)]
mod loop_circuits_result_tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Small feedback-loop project: population -> births -> population.
    fn feedback_project() -> TestProject {
        TestProject::new("loop_result_test")
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * 0.1", None)
    }

    fn compute_loop_circuits(project: &TestProject) -> LoopCircuitsResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        model_loop_circuits(&db, source_model, source_project).clone()
    }

    /// `to_named_circuits` must reconstruct the same owned-string lists
    /// that the legacy `Vec<Vec<String>>` shape would have produced.
    #[test]
    fn test_loop_circuits_result_lookup_matches_legacy() {
        let result = compute_loop_circuits(&feedback_project());

        let legacy: Vec<Vec<String>> = result.to_named_circuits();
        assert_eq!(legacy.len(), result.len());

        for (ci, circuit_idx) in result.circuits.iter().enumerate() {
            let names: Vec<&str> = result.circuit_names(ci).collect();
            let legacy_names: Vec<&str> = legacy[ci].iter().map(String::as_str).collect();
            assert_eq!(names, legacy_names);

            // And each index resolves to the same name.
            for (slot, &ni) in circuit_idx.iter().enumerate() {
                assert_eq!(result.names[ni as usize], legacy[ci][slot]);
            }
        }

        // The legacy loop has two nodes: population and births, both in
        // the name table exactly once.
        assert!(result.names.iter().any(|n| n == "population"));
        assert!(result.names.iter().any(|n| n == "births"));
    }

    /// The shared name table should contain no duplicates and be sorted
    /// lexicographically -- the enumerator relies on lex-sorted indices
    /// for its small-start dedup, so the exposed table must preserve
    /// that invariant.
    #[test]
    fn test_loop_circuits_result_names_are_unique_and_sorted() {
        let project = TestProject::new("multi_node_loop")
            .stock("a", "10", &["f1"], &[], None)
            .flow("f1", "a * 0.1", None)
            .stock("b", "20", &["f2"], &[], None)
            .flow("f2", "b * 0.2", None);
        let result = compute_loop_circuits(&project);

        let mut sorted = result.names.clone();
        sorted.sort();
        assert_eq!(
            result.names, sorted,
            "names should be in lex-sorted order (the enumerator's canonical ordering)"
        );

        let mut dedup = result.names.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(
            dedup.len(),
            result.names.len(),
            "names should contain no duplicates"
        );
    }

    /// A pure DAG produces zero circuits and an empty names table.
    /// Trimming names to cycle-participating nodes is what keeps the
    /// salsa LoopCircuitsResult stable under renames of acyclic
    /// variables -- see the `find_indexed_circuits_trims_names_to_cycle_participants`
    /// regression test in `ltm.rs::tests` for the positive-side invariant.
    #[test]
    fn test_loop_circuits_result_empty_on_dag() {
        let project = TestProject::new("dag_only")
            .scalar_const("a", 1.0)
            .scalar_aux("b", "a + 1")
            .scalar_aux("c", "b * 2");
        let result = compute_loop_circuits(&project);

        assert!(result.is_empty(), "pure DAG must produce zero circuits");
        assert_eq!(result.len(), 0);
        assert_eq!(result.to_named_circuits().len(), 0);
        assert!(
            result.names.is_empty(),
            "empty circuits must produce empty names table so salsa stays stable under acyclic-variable renames"
        );
    }
}

#[cfg(test)]
mod detected_loops_scc_gate_tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Build a project whose causal graph contains an SCC of size
    /// `2 * stocks_in_cycle` by wiring `stocks_in_cycle` stocks in a
    /// ring: each `f_i` depends on `s_{i-1}` and feeds `s_i`.  The
    /// resulting SCC contains both the stocks and the flows.
    fn ring_project(stocks_in_cycle: usize) -> TestProject {
        let mut p = TestProject::new("ring").with_sim_time(0.0, 1.0, 1.0);
        for i in 0..stocks_in_cycle {
            let prev = (i + stocks_in_cycle - 1) % stocks_in_cycle;
            let stock = format!("s_{i}");
            let flow = format!("f_{i}");
            let prev_stock = format!("s_{prev}");
            p = p
                .stock(&stock, "0", &[flow.as_str()], &[], None)
                .flow(&flow, &prev_stock, None);
        }
        p
    }

    fn detect_loops(project: &TestProject) -> DetectedLoopsResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        model_detected_loops(&db, source_model, sync.project)
    }

    /// A small feedback loop must still be detected -- the SCC-size
    /// gate only fires when the largest SCC exceeds
    /// `MAX_LTM_SCC_NODES`.
    #[test]
    fn small_feedback_loop_is_detected() {
        let project = ring_project(2); // 2 stocks + 2 flows = 4-node SCC
        let result = detect_loops(&project);
        assert!(
            !result.loops.is_empty(),
            "4-node SCC is well under the 50-node gate; loops must still be returned"
        );
    }

    /// An SCC larger than `MAX_LTM_SCC_NODES` must short-circuit to
    /// an empty result without paying for Johnson's enumeration.
    /// This matches the behaviour of `model_ltm_variables`'s
    /// auto-flip gate on the element-level graph, so FFI and layout
    /// consumers of `model_detected_loops` do not force full
    /// enumeration on WRLD3-shape models (166-node SCC, 1.86M
    /// circuits, seconds-to-minutes of Johnson's work) before the
    /// LTM pipeline's own gate gets a chance to fire.
    #[test]
    fn oversized_scc_short_circuits_to_empty() {
        // Ring of 30 stocks + 30 flows = 60-node SCC, comfortably
        // above the 50-node threshold.
        let project = ring_project(30);
        let result = detect_loops(&project);
        assert!(
            result.loops.is_empty(),
            "60-node SCC must trip the MAX_LTM_SCC_NODES = 50 gate, got {} loops",
            result.loops.len()
        );
    }
}

#[cfg(test)]
#[path = "db_element_graph_tests.rs"]
mod db_element_graph_tests;
