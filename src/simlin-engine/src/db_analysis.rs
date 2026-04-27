// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Causal graph analysis tracked functions.
//!
//! Extracted from db.rs for file-size management. Contains:
//! - CausalEdgesResult, LoopCircuitsResult, CyclePartitionsResult
//! - ElementCausalEdgesResult, RefShape, ReferenceSite (element-level graph)
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
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::Update)]
pub enum RefShape {
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
///
/// The walker emits one site per reference. Callers iterating the
/// variable-level edge map already know the source ident, so the site
/// only needs to carry the per-reference `shape` and (for arrayed
/// per-element targets) the `target_element` it was discovered under.
///
/// `target_element` is set only when the reference appears inside an
/// `Ast::Arrayed` per-element expression: the value is the canonical
/// element name (single-dim) or comma-separated tuple (multi-dim) of the
/// target element being defined. For `Ast::Scalar` and `Ast::ApplyToAll`
/// it stays `None` (the reference contributes to every target element
/// according to the shape's normal broadcast/diagonal rules).
#[derive(Debug, Clone)]
pub(crate) struct ReferenceSite {
    pub shape: RefShape,
    pub target_element: Option<String>,
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
                // Indexed dimensions accept integer literals in the
                // range [1, size]. Canonicalize via parse-then-format
                // so non-canonical forms like `pop[01]` reduce to `"1"`
                // -- matching `dimension_element_names`'s `"1".."N"`
                // output and the Expr0 sibling
                // (`ltm_augment::resolve_literal_element_index`).
                // Returning the original text would let `pop[01]`
                // serialize as `FixedIndex(["01"])` while the partial
                // builder reduces to `FixedIndex(["1"])`, the shape
                // comparison would fail, and the live ref would be
                // wrapped in `PREVIOUS()`.
                if let Ok(n) = canonical.parse::<u32>()
                    && n >= 1
                    && n <= *size
                {
                    return Some(n.to_string());
                }
            }
        }
    }
    None
}

/// Walk a target variable's AST and emit one `ReferenceSite` per occurrence
/// of `source_ident`, accumulating per-site shapes for downstream edge
/// emission.
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
/// Return the unique `RefShape`s under which `source_ident` is referenced
/// in `target_var`'s AST.
///
/// Sibling of [`collect_reference_sites`] that drops the per-site
/// `target_element` and source-name fields. Used by `model_ltm_variables`
/// to enumerate the shapes for which a per-shape link score must be
/// emitted (one `LtmSyntheticVar` per `(from, to, shape)` tuple).
///
/// Order is the AST-walk order of first occurrence; duplicates are
/// removed. Returns an empty vec when the source isn't referenced.
pub(crate) fn collect_reference_shapes(
    target_var: &crate::variable::Variable,
    source_ident: &str,
    source_is_arrayed: bool,
    source_dims: &[crate::dimensions::Dimension],
) -> Vec<RefShape> {
    let sites = collect_reference_sites(target_var, source_ident, source_is_arrayed, source_dims);
    let mut shapes: Vec<RefShape> = Vec::new();
    for site in sites {
        if !shapes.iter().any(|s| s == &site.shape) {
            shapes.push(site.shape);
        }
    }
    shapes
}

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
            // Scalar/A2A equations: every reference site contributes to
            // every target element according to its shape's broadcast or
            // diagonal rule. `target_element = None` lets the emitter
            // apply the default rules.
            collect_in_expr(
                expr,
                source_ident,
                source_is_arrayed,
                source_dims,
                None,
                &mut sites,
            );
        }
        crate::ast::Ast::Arrayed(_, subscript_map, default_expr, _) => {
            // Per-element expressions: each reference site is pinned to the
            // specific target element being defined. The emitter restricts
            // edge emission to that element only.
            for (target_elem, expr) in subscript_map.iter() {
                collect_in_expr(
                    expr,
                    source_ident,
                    source_is_arrayed,
                    source_dims,
                    Some(target_elem.as_str()),
                    &mut sites,
                );
            }
            // The EXCEPT default applies to elements not explicitly listed.
            // We don't know the exact target element set here, but the
            // default expression's references contribute to every other
            // target element. Treating `target_element = None` makes the
            // emitter broadcast across the full target dimension, which
            // is a conservative superset that preserves the variable-level
            // projection invariant.
            if let Some(default) = default_expr {
                collect_in_expr(
                    default,
                    source_ident,
                    source_is_arrayed,
                    source_dims,
                    None,
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
#[allow(clippy::only_used_in_recursion)] // helper for collect_reference_sites
fn collect_in_expr(
    expr: &crate::ast::Expr2,
    source_ident: &str,
    source_is_arrayed: bool,
    source_dims: &[crate::dimensions::Dimension],
    target_element: Option<&str>,
    sites: &mut Vec<ReferenceSite>,
) {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::builtins::{BuiltinContents, walk_builtin_expr};

    let make_site = |shape: RefShape| -> ReferenceSite {
        ReferenceSite {
            shape,
            target_element: target_element.map(|s| s.to_string()),
        }
    };

    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _array_bounds, _) => {
            if ident.as_str() == source_ident {
                sites.push(make_site(RefShape::Bare));
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            if ident.as_str() == source_ident {
                let shape = classify_subscript_shape(indices, source_dims);
                sites.push(make_site(shape));
            }
            // Always recurse into index expressions so nested references
            // like `source_outer[source_inner[*]]` (or arbitrary index
            // arithmetic mentioning the source) still emit per-site
            // entries for the inner reference.
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => {
                        collect_in_expr(
                            e,
                            source_ident,
                            source_is_arrayed,
                            source_dims,
                            target_element,
                            sites,
                        );
                    }
                    IndexExpr2::Range(l, r, _) => {
                        collect_in_expr(
                            l,
                            source_ident,
                            source_is_arrayed,
                            source_dims,
                            target_element,
                            sites,
                        );
                        collect_in_expr(
                            r,
                            source_ident,
                            source_is_arrayed,
                            source_dims,
                            target_element,
                            sites,
                        );
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
                        sites.push(make_site(RefShape::Bare));
                    }
                }
                BuiltinContents::Expr(sub_expr) => {
                    collect_in_expr(
                        sub_expr,
                        source_ident,
                        source_is_arrayed,
                        source_dims,
                        target_element,
                        sites,
                    );
                }
            });
        }
        Expr2::Op1(_, operand, _, _) => {
            collect_in_expr(
                operand,
                source_ident,
                source_is_arrayed,
                source_dims,
                target_element,
                sites,
            );
        }
        Expr2::Op2(_, left, right, _, _) => {
            collect_in_expr(
                left,
                source_ident,
                source_is_arrayed,
                source_dims,
                target_element,
                sites,
            );
            collect_in_expr(
                right,
                source_ident,
                source_is_arrayed,
                source_dims,
                target_element,
                sites,
            );
        }
        Expr2::If(cond, then_expr, else_expr, _, _) => {
            collect_in_expr(
                cond,
                source_ident,
                source_is_arrayed,
                source_dims,
                target_element,
                sites,
            );
            collect_in_expr(
                then_expr,
                source_ident,
                source_is_arrayed,
                source_dims,
                target_element,
                sites,
            );
            collect_in_expr(
                else_expr,
                source_ident,
                source_is_arrayed,
                source_dims,
                target_element,
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

/// Emit element edges for a single AST reference site.
///
/// The AST walker classifies each reference site into a `RefShape` and
/// passes `(from_name, to_name, from_dims, to_dims, shape, target_element)`
/// to this helper, which translates the shape into the appropriate
/// element-level edges and unions them into `element_edges`.
///
/// `target_element` is `Some(elem)` when the reference appears inside an
/// `Ast::Arrayed` per-element expression: the target node set is then
/// pinned to that single element tuple (parsed from `elem`'s comma-
/// separated form for multi-dim arrays). When `None`, the reference
/// applies to every target element according to its shape's normal
/// broadcast/diagonal rule (Scalar/A2A semantics).
///
/// Truth table (matches design plan; rows below assume `target_element`
/// is `None` -- the per-element narrowing only changes which target
/// element names appear on the right-hand side):
/// | `from_dims` | `to_dims`  | `shape`                       | Edges emitted                                 |
/// |-------------|------------|-------------------------------|-----------------------------------------------|
/// | []          | []         | Bare                          | `from -> to`                                  |
/// | []          | non-empty  | Bare                          | `from -> to[d]` for each cartesian d          |
/// | non-empty   | []         | Bare                          | `from[d] -> to` for each cartesian d          |
/// | non-empty   | non-empty (same dims)  | Bare              | `from[d] -> to[d]` per shared element         |
/// | non-empty   | non-empty (partial collapse) | Bare        | `from[d1,d2] -> to[d1]` (delegates to `expand_same_element`)|
/// | non-empty   | any        | Wildcard / DynamicIndex       | full cross product (NxM)                      |
/// | non-empty   | []         | FixedIndex(elems)             | `from[elems] -> to` (one edge)                |
/// | non-empty   | non-empty  | FixedIndex(elems)             | `from[elems] -> to[d]` for each cartesian d   |
///
/// `FixedIndex` carries the resolved per-dimension element names in
/// source order; multi-dim fixed yields `from[e1,e2]`. Mixed
/// fixed+wildcard subscripts classify upstream as `Wildcard` (or
/// `DynamicIndex`), so this helper does not need to handle a
/// "partial fixed" branch -- it only sees fully-resolved
/// `FixedIndex(elems)` payloads or the conservative full-cross shapes.
fn emit_edges_for_reference(
    from_name: &str,
    to_name: &str,
    from_dims: &[crate::dimensions::Dimension],
    to_dims: &[crate::dimensions::Dimension],
    shape: &RefShape,
    target_element: Option<&str>,
    element_edges: &mut HashMap<String, BTreeSet<String>>,
) {
    let from_is_scalar = from_dims.is_empty();
    let to_is_scalar = to_dims.is_empty();

    // Compute the per-site target node set. With `target_element` set we
    // restrict to a single target; otherwise, we use the full cartesian
    // product. The single-target case mirrors `format_multi_element_name`
    // by accepting comma-separated multi-dim subscripts as-is (the
    // canonical form of `Arrayed`'s element key already matches that).
    let target_nodes: Vec<String> = if to_is_scalar {
        vec![to_name.to_string()]
    } else if let Some(elem) = target_element {
        // The element key from `Ast::Arrayed` is a comma-separated tuple
        // of canonical element names (e.g. "nyc" or "nyc,adult"). Format
        // the target node directly without re-cartesian-producting.
        vec![format!("{}[{}]", to_name, elem)]
    } else {
        cartesian_element_names(to_name, to_dims)
    };

    // Scalar source short-circuits: shape doesn't matter (a scalar source
    // has no subscript form). Either pass-through or broadcast.
    if from_is_scalar {
        for to_node in &target_nodes {
            element_edges
                .entry(from_name.to_string())
                .or_default()
                .insert(to_node.clone());
        }
        return;
    }

    // Arrayed source. The shape determines which source elements appear
    // and how they connect to the target.
    match shape {
        RefShape::Bare => {
            // Same-element semantics. With a scalar target this is a
            // reduction (every from element feeds the single to). With
            // an arrayed target (matching dims), this is the diagonal;
            // with partial-collapse dims, expand_same_element handles
            // the projection.
            //
            // When `target_element` is set (arrayed equation per-element
            // expression), the bare reference still represents same-
            // element semantics: only the source element matching the
            // target element contributes. We delegate to
            // `expand_same_element` and restrict the result to the
            // pinned target node afterward by intersection with
            // `target_nodes`.
            if to_is_scalar {
                for from_elem in cartesian_element_names(from_name, from_dims) {
                    element_edges
                        .entry(from_elem)
                        .or_default()
                        .insert(to_name.to_string());
                }
            } else if target_element.is_some() {
                // Per-element bare reference: the same-element diagonal
                // applies to the single pinned target. We compute the
                // full diagonal into a scratch map and then keep only
                // edges whose target appears in `target_nodes`.
                let mut scratch: HashMap<String, BTreeSet<String>> = HashMap::new();
                expand_same_element(from_name, to_name, from_dims, to_dims, &mut scratch);
                let target_set: BTreeSet<String> = target_nodes.iter().cloned().collect();
                for (from_node, tos) in scratch {
                    let filtered: BTreeSet<String> =
                        tos.into_iter().filter(|t| target_set.contains(t)).collect();
                    if !filtered.is_empty() {
                        let entry = element_edges.entry(from_node).or_default();
                        for t in filtered {
                            entry.insert(t);
                        }
                    }
                }
            } else {
                expand_same_element(from_name, to_name, from_dims, to_dims, element_edges);
            }
        }
        RefShape::FixedIndex(elems) => {
            // The source is pinned to a single element tuple. Build
            // exactly one source key and emit edges to every target
            // node (which `target_nodes` already narrows when the
            // reference is inside an arrayed per-element expression).
            let from_node = if elems.len() == 1 {
                format_element_name(from_name, &elems[0])
            } else {
                let elem_refs: Vec<&str> = elems.iter().map(String::as_str).collect();
                format_multi_element_name(from_name, &elem_refs)
            };

            let entry = element_edges.entry(from_node).or_default();
            for to_node in &target_nodes {
                entry.insert(to_node.clone());
            }
        }
        RefShape::Wildcard | RefShape::DynamicIndex => {
            // Conservative full cross product over source elements.
            // `target_nodes` already restricts the target side when
            // inside an arrayed per-element expression.
            let from_elements = cartesian_element_names(from_name, from_dims);
            for from_elem in &from_elements {
                let entry = element_edges.entry(from_elem.clone()).or_default();
                for to_node in &target_nodes {
                    entry.insert(to_node.clone());
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

    // Expand each variable-level edge to element-level edges using the
    // AST-walking per-reference walker. For each (source, target) pair we
    // collect every reference site of `source` in `target`'s AST, classify
    // each into a `RefShape`, and emit the per-shape element edges. This
    // replaces the older single-classification-per-pair scheme that
    // over-expanded fixed-index references to the full N x N cross product.
    for (from_name, to_set) in &variable_edges.edges {
        let from_dims = lookup_dims(from_name, &mut dim_cache);
        for to_name in to_set {
            let to_dims = lookup_dims(to_name, &mut dim_cache);

            // Fast path: both scalar -> direct edge.
            if from_dims.is_empty() && to_dims.is_empty() {
                element_edges
                    .entry(from_name.clone())
                    .or_default()
                    .insert(to_name.clone());
                continue;
            }

            // Structural flow->stock edges: the stock's equation is just the
            // initial value, so the flow name never appears in the stock's
            // AST. Without this bypass, the walker would find no reference
            // sites and emit no edges. Both arrayed -> emit SameElement
            // diagonal directly.
            if structural_flow_to_stock.contains(&(from_name.clone(), to_name.clone()))
                && !from_dims.is_empty()
                && !to_dims.is_empty()
            {
                emit_edges_for_reference(
                    from_name,
                    to_name,
                    &from_dims,
                    &to_dims,
                    &RefShape::Bare,
                    None,
                    &mut element_edges,
                );
                continue;
            }

            // AST-based emission: collect reference sites and emit one set
            // of element edges per site. Multiple sites of the same shape
            // dedupe naturally because `element_edges` values are sets.
            let target_var = match reconstruct_single_variable(db, model, project, to_name) {
                Some(v) => v,
                None => {
                    // Couldn't reconstruct (shouldn't happen for well-formed
                    // models). Fall back to scalar broadcast emission so the
                    // variable-level projection invariant still holds.
                    emit_edges_for_reference(
                        from_name,
                        to_name,
                        &from_dims,
                        &to_dims,
                        &RefShape::Bare,
                        None,
                        &mut element_edges,
                    );
                    continue;
                }
            };
            let source_is_arrayed = !from_dims.is_empty();
            let sites =
                collect_reference_sites(&target_var, from_name, source_is_arrayed, &from_dims);

            if sites.is_empty() {
                // Defensive: the variable-level edge exists but the AST has
                // no reference. This can happen with structural edges or
                // synthesized references. Fall back to scalar broadcast so
                // the variable-level projection invariant still holds.
                emit_edges_for_reference(
                    from_name,
                    to_name,
                    &from_dims,
                    &to_dims,
                    &RefShape::Bare,
                    None,
                    &mut element_edges,
                );
                continue;
            }

            for site in sites {
                emit_edges_for_reference(
                    from_name,
                    to_name,
                    &from_dims,
                    &to_dims,
                    &site.shape,
                    site.target_element.as_deref(),
                    &mut element_edges,
                );
            }
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
mod emit_edges_for_reference_tests {
    use super::*;
    use crate::common::{CanonicalDimensionName, CanonicalElementName};
    use crate::dimensions::{Dimension, NamedDimension};
    use std::collections::HashMap as StdHashMap;

    /// Build a single-dim `Named` dimension from raw element names.
    /// Mirrors `make_named_dimension` in `ltm_augment.rs::tests` -- inlined
    /// here because that helper is private to the other test module.
    fn make_named_dimension(name: &str, elements: &[&str]) -> Dimension {
        let canonical_elements: Vec<CanonicalElementName> = elements
            .iter()
            .map(|e| CanonicalElementName::from_raw(e))
            .collect();
        let indexed: StdHashMap<CanonicalElementName, usize> = canonical_elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), i + 1))
            .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements: canonical_elements,
                indexed_elements: indexed,
                maps_to: None,
                mappings: vec![],
            },
        )
    }

    /// Scalar source -> scalar target with `Bare` shape: a single
    /// from -> to edge, no expansion.
    #[test]
    fn scalar_to_scalar_bare_passthrough() {
        let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();
        emit_edges_for_reference("a", "b", &[], &[], &RefShape::Bare, None, &mut edges);

        let from = edges.get("a").expect("expected 'a' as a source key");
        assert_eq!(from.len(), 1);
        assert!(from.contains("b"));
    }

    /// Arrayed source -> arrayed target with `FixedIndex(["nyc"])`: only
    /// `pop[nyc]` should appear as a source key, and it must connect to
    /// every target element. `pop[boston]` must NOT appear as a source.
    #[test]
    fn fixed_index_to_arrayed_target() {
        let region = make_named_dimension("Region", &["NYC", "Boston"]);
        let dims = std::slice::from_ref(&region);
        let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();

        emit_edges_for_reference(
            "pop",
            "rel",
            dims,
            dims,
            &RefShape::FixedIndex(vec!["nyc".to_string()]),
            None,
            &mut edges,
        );

        let from = edges.get("pop[nyc]").expect("from key 'pop[nyc]'");
        assert!(from.contains("rel[nyc]"), "missing rel[nyc] in {from:?}");
        assert!(
            from.contains("rel[boston]"),
            "missing rel[boston] in {from:?}"
        );
        assert_eq!(from.len(), 2, "expected exactly 2 outgoing edges");
        assert!(
            !edges.contains_key("pop[boston]"),
            "pop[boston] must not appear as a source for FixedIndex(nyc)"
        );
    }

    /// Arrayed source -> arrayed target with `Bare` shape on identical
    /// dimensions: per-element diagonal `pop[d] -> rel[d]`. No off-diagonal
    /// edges.
    #[test]
    fn bare_same_dim_diagonal() {
        let region = make_named_dimension("Region", &["NYC", "Boston"]);
        let dims = std::slice::from_ref(&region);
        let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();

        emit_edges_for_reference("pop", "rel", dims, dims, &RefShape::Bare, None, &mut edges);

        let nyc = edges.get("pop[nyc]").expect("from key 'pop[nyc]'");
        assert_eq!(nyc.len(), 1, "diagonal: one outgoing edge");
        assert!(nyc.contains("rel[nyc]"));

        let boston = edges.get("pop[boston]").expect("from key 'pop[boston]'");
        assert_eq!(boston.len(), 1, "diagonal: one outgoing edge");
        assert!(boston.contains("rel[boston]"));
    }

    /// `target_element` narrows the FixedIndex emission to the pinned target.
    /// With `target_element = Some("boston")`, only `pop[nyc] -> rel[boston]`
    /// is emitted; the NYC target broadcast is suppressed. This mirrors the
    /// per-element `Ast::Arrayed` case used by the cross-element fixture.
    #[test]
    fn fixed_index_with_target_element_pins_to_one_target() {
        let region = make_named_dimension("Region", &["NYC", "Boston"]);
        let dims = std::slice::from_ref(&region);
        let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();

        emit_edges_for_reference(
            "pop",
            "rel",
            dims,
            dims,
            &RefShape::FixedIndex(vec!["nyc".to_string()]),
            Some("boston"),
            &mut edges,
        );

        let from = edges.get("pop[nyc]").expect("from key 'pop[nyc]'");
        assert_eq!(from.len(), 1, "expected exactly 1 outgoing edge");
        assert!(from.contains("rel[boston]"));
        assert!(!from.contains("rel[nyc]"));
    }

    /// `RefShape::Bare` with `target_element = Some("boston")` on identical
    /// dimensions: only the diagonal edge `pop[boston] -> rel[boston]` survives;
    /// the other diagonal edge `pop[nyc] -> rel[nyc]` is excluded because it
    /// does not reach the pinned target. This exercises the scratch-map +
    /// intersection path in the `Bare` branch of `emit_edges_for_reference`.
    #[test]
    fn bare_with_target_element_keeps_only_pinned_diagonal_edge() {
        let region = make_named_dimension("Region", &["NYC", "Boston"]);
        let dims = std::slice::from_ref(&region);
        let mut edges: HashMap<String, BTreeSet<String>> = HashMap::new();

        emit_edges_for_reference(
            "pop",
            "rel",
            dims,
            dims,
            &RefShape::Bare,
            Some("boston"),
            &mut edges,
        );

        // Only the boston diagonal edge should be present.
        let from_boston = edges
            .get("pop[boston]")
            .expect("pop[boston] must be a source");
        assert_eq!(
            from_boston.len(),
            1,
            "expected exactly one outgoing edge from pop[boston]"
        );
        assert!(
            from_boston.contains("rel[boston]"),
            "expected pop[boston] -> rel[boston]"
        );

        // pop[nyc] should either be absent or have no edges into rel[boston];
        // the diagonal for nyc is rel[nyc], which is not the pinned target.
        if let Some(from_nyc) = edges.get("pop[nyc]") {
            assert!(
                !from_nyc.contains("rel[boston]"),
                "pop[nyc] must not reach rel[boston] via Bare diagonal"
            );
            assert!(
                !from_nyc.contains("rel[nyc]"),
                "pop[nyc] -> rel[nyc] must be excluded when target_element = boston"
            );
        }
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
