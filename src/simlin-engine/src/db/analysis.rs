// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Causal graph analysis tracked functions.
//!
//! Extracted from db.rs for file-size management. Contains:
//! - CausalEdgesResult, LoopCircuitsResult, CyclePartitionsResult
//! - ElementCausalEdgesResult, RefShape (element-level graph); the
//!   reference-site classification (`ClassifiedSite`, the AST walker, the
//!   agg-routing decision) lives in `db/ltm_ir.rs` and is consumed here via
//!   `model_ltm_reference_sites`
//! - `emit_edges_for_reference` and the element-name expansion helpers
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
    Db, ModuleIdentContext, ModuleInputSet, SourceModel, SourceProject, SourceVariableKind,
    build_module_inputs, model_module_ident_context, parse_source_variable_with_module_context,
    project_datamodel_dims, project_dimensions_context, variable_direct_dependencies,
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
/// emission ([`emit_edges_for_reference`]) and per-reference
/// partial-equation construction.
///
/// The shape does *not* by itself decide whether a reference is rerouted
/// through a hoisted `$⁚ltm⁚agg⁚{n}` aggregate node -- that is recorded in
/// `ltm_ir::ClassifiedSite::routing` (`ThroughAgg` iff the site is
/// syntactically inside a hoisted reducer *and* a synthetic agg of `to`
/// reads `from`). A `to` equation can hold both `SUM(pop[*])` (routed via
/// the agg) and a direct `pop[idx]` (kept as a conservative cross-product
/// edge), and both produce a Wildcard/DynamicIndex site; the IR's `routing`
/// is what tells them apart for `model_element_causal_edges` and the
/// link-score emitter.
///
/// Post-cross-element-aggregate-scoring (the `$⁚ltm⁚agg⁚{n}` work) and after
/// #514 (sliced-reducer hoisting), `Wildcard` / `DynamicIndex` no longer
/// drive a per-shape `⁚wildcard` / `⁚dynamic` link-score variant. Every
/// statically-describable inlined reducer -- whole-extent (`SUM(pop[*])`) or
/// sliced (`SUM(pop[NYC, *])`, `SUM(matrix[D1, *])`) -- is hoisted into a
/// `$⁚ltm⁚agg⁚{n}` node and scored by the agg's two halves. A site that is
/// *not* a hoisted reducer's argument -- a bare dynamic index (`arr[i+1]`, a
/// range), the dynamic-index reducer carve-out (`SUM(pop[idx, *])`, `idx`
/// non-literal, reclassified to `DynamicIndex`), a mapped-dimension sliced
/// reducer (`SUM(matrix[State, *])` over `matrix[Region, D2]` with a
/// `State→Region` mapping; `enumerate_agg_nodes` declines the remapped axis,
/// so the `Wildcard` reference stays `Direct`), or a direct `pop[idx]`
/// alongside a `SUM(pop[*])` -- keeps a conservative edge and a Bare-named
/// link score.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update)]
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
    /// `IndexExpr2::Wildcard`, or every index is `Wildcard` / `StarRange`
    /// (the reducer-style whole-extent access). A reducer reference with
    /// this shape that `enumerate_agg_nodes` hoisted into a `$⁚ltm⁚agg⁚{n}`
    /// node is routed `ThroughAgg` (the shape is then ignored); a *whole-RHS*
    /// reducer's argument (`total = SUM(population[*])`,
    /// `row_sum[D1] = SUM(matrix[D1,*])`) keeps this shape on its `Direct`
    /// site, where it projects to the conservative reduction / cross-product
    /// into the (variable-backed-agg) target. The not-hoistable dynamic-index
    /// reducer carve-out (`SUM(pop[idx,*])`) is reclassified by `ltm_ir`
    /// as `DynamicIndex` rather than kept here (#514), so a `Direct`
    /// `Wildcard` site never carries an *un*-hoisted sliced reducer.
    Wildcard,
    /// `Expr2::Subscript(source, indices)` where at least one index is
    /// a non-literal expression (`@N`, `Range`, an arbitrary `Expr`, or a
    /// *partial* `StarRange` mixed with literal indices) -- *or* the
    /// not-hoistable dynamic-index reducer carve-out (`SUM(pop[idx,*])`,
    /// reclassified here from `Wildcard` by `ltm_ir` so the conservative
    /// cross-product path is `DynamicIndex`-only in `emit_edges_for_reference`).
    /// Conservative full cross-product. (A hoisted *synthetic*-agg reducer
    /// reference never has routing `Direct` with this shape -- it's
    /// `ThroughAgg`.)
    DynamicIndex,
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
///
/// A `ThroughAgg`-routed reference never reaches here -- those are routed
/// through a synthetic aggregate node by `emit_agg_routed_edges` (only the
/// read-slice rows). After #514 the `Direct` not-hoistable-reducer carve-out
/// (`SUM(pop[idx,*])`) is reclassified by the IR as `DynamicIndex`, so a
/// `Direct` `Wildcard` site is now only a variable-backed reducer's whole-RHS
/// argument (`total = SUM(population[*])`, `row_sum[D1] = SUM(matrix[D1,*])`)
/// or a (rare) non-reducer whole-array reference; the conservative cross
/// product is the right semantics for all of those.
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
            // inside an arrayed per-element expression. `DynamicIndex`
            // here is `arr[i+1]`, a range, or the not-hoistable-reducer
            // carve-out (`SUM(pop[idx,*])`, reclassified from `Wildcard` by
            // the IR); `Wildcard` here is only a variable-backed reducer's
            // whole-RHS argument (a reduction into a scalar/lower-rank `to`)
            // or a rare non-reducer whole-array reference -- a hoisted
            // *synthetic*-agg reducer reference is routed through the agg
            // (`emit_agg_routed_edges`) and never lands on this arm.
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

/// Emit the element edges for a reference routed through a hoisted aggregate
/// node: `source[<read slice>] → agg[<iterated>]` then `agg[<iterated>] →
/// to[e]`, where `agg.read_slice` (one [`AxisRead`] per source axis) decides
/// which source rows feed each agg result slot and `agg.result_dims` (the
/// `Iterated` axes' dims) decides how the agg fans out into `to`.
///
/// - A [`AxisRead::Pinned`] axis fixes one element of the source on that axis.
/// - An [`AxisRead::Iterated`] axis ranges; its element selects the agg result
///   slot's coordinate for that dimension.
/// - A [`AxisRead::Reduced`] axis ranges over *every* element (each one feeds
///   the same agg result slot). For the *element graph* a representative
///   element would suffice for reachability, but emitting one edge per element
///   matches `cross_element_loop_through_sum_reducer`'s whole-extent
///   expectation and the per-element link scores need them all anyway.
///
/// When `read_slice` is all-`Reduced` (`result_dims` empty) the agg is scalar:
/// every source element feeds `agg`, and `agg` broadcasts to every `to`
/// element -- the pre-Phase-4 behavior. `target_element` (a per-element
/// `Ast::Arrayed` slot) pins the `agg → to` half to that single target.
///
/// Defensive: if `read_slice` doesn't have one entry per source axis (it
/// always should for a hoisted agg whose `source_vars` includes `from`), fall
/// back to the conservative "every source element → agg" form so a stale
/// invariant can't drop edges.
fn emit_agg_routed_edges(
    from_name: &str,
    to_name: &str,
    from_dims: &[crate::dimensions::Dimension],
    to_dims: &[crate::dimensions::Dimension],
    agg: &crate::ltm_agg::AggNode,
    target_element: Option<&str>,
    element_edges: &mut HashMap<String, BTreeSet<String>>,
) {
    use crate::ltm_agg::AxisRead;

    // The agg's result-axis dimensions (`AggNode::result_dims`, datamodel-
    // cased), resolved to the `Dimension` objects -- so `cartesian_element_names`
    // / `expand_same_element` and the agg-slot subscripting below can operate
    // on them directly. The agg's result dims are always dimensions the target
    // `to` iterates over (they come from the target equation's iterated
    // dimensions), and -- when the reducer reads its arrayed source by the
    // matching iterated subscript -- a subset of the arrayed source's dims too,
    // so we can recover the `Dimension` from `from_dims` (preferred -- it's the
    // source row axis) or from `to_dims` (the only place to look when `from` is
    // a *scalar* feeder of the agg). `read_slice_ok` keys the source-row layout
    // machinery below off the well-formed slice; it is independent of where the
    // `Iterated` `Dimension`s come from.
    let read_slice_ok = !from_dims.is_empty() && agg.read_slice.len() == from_dims.len();
    let resolve_result_dim = |name: &str| -> Option<crate::dimensions::Dimension> {
        let canon = canonicalize(name);
        from_dims
            .iter()
            .chain(to_dims.iter())
            .find(|d| d.name() == canon.as_ref())
            .cloned()
    };
    let iterated_dims: Vec<crate::dimensions::Dimension> = agg
        .result_dims
        .iter()
        .filter_map(|rd| resolve_result_dim(rd))
        .collect();
    debug_assert_eq!(
        iterated_dims.len(),
        agg.result_dims.len(),
        "every agg result dim ({:?}) must resolve to a Dimension carried by `from` ({:?}) or `to` ({:?})",
        agg.result_dims,
        from_dims.iter().map(|d| d.name()).collect::<Vec<_>>(),
        to_dims.iter().map(|d| d.name()).collect::<Vec<_>>(),
    );

    // The agg node name for a given result-slot coordinate tuple. A scalar agg
    // (`iterated_dims` empty) keeps its bare name; an arrayed agg is
    // subscripted with the iterated elements in order.
    let agg_node_name = |slot: &[String]| -> String {
        if slot.is_empty() {
            agg.name.clone()
        } else {
            format!("{}[{}]", agg.name, slot.join(","))
        }
    };

    if from_dims.is_empty() {
        // A *scalar* feeder of the (possibly arrayed) agg -- this arises when
        // the reducer's argument references a scalar variable, e.g.
        // `growth[D1] = base + SUM(matrix[D1,*] * scale)` where `scale` is
        // scalar (the `scale → $⁚ltm⁚agg⁚0` edge). A scalar feeder is not
        // subscripted, so it cannot pick out one agg result slot; it feeds
        // *every* slot. Emit `scale → agg[<each Iterated combo>]` (or
        // `scale → agg` when the agg is scalar), bypassing the `axis_plans`
        // row machinery below (which, fed an empty `from_dims`, would build a
        // single empty `row_elems` and emit a malformed `scale[]` node via
        // `format_multi_element_name(_, &[])`).
        let entry = element_edges.entry(from_name.to_string()).or_default();
        if iterated_dims.is_empty() {
            entry.insert(agg.name.clone());
        } else {
            let mut slots: Vec<Vec<String>> = vec![Vec::new()];
            for d in &iterated_dims {
                let elems = dimension_element_names(d);
                let mut next: Vec<Vec<String>> = Vec::with_capacity(slots.len() * elems.len());
                for slot in &slots {
                    for elem in &elems {
                        let mut s = slot.clone();
                        s.push(elem.clone());
                        next.push(s);
                    }
                }
                slots = next;
            }
            for slot in &slots {
                entry.insert(agg_node_name(slot));
            }
        }
    } else {
        // Per source axis, the element-name list to iterate when enumerating
        // source rows: a `Pinned` axis is fixed to one element; an `Iterated`
        // or `Reduced` axis ranges over every element. The position of an
        // `Iterated` axis within the result tuple is tracked so each source
        // row maps to the matching agg result slot.
        struct AxisPlan {
            elems: Vec<String>,
            /// `Some(j)` if this axis is the `j`th `Iterated` axis (its
            /// element becomes coordinate `j` of the agg result slot);
            /// `None` otherwise.
            iterated_pos: Option<usize>,
        }
        let mut axis_plans: Vec<AxisPlan> = Vec::with_capacity(from_dims.len());
        let mut next_iterated_pos = 0usize;
        if read_slice_ok {
            for (a, d) in agg.read_slice.iter().zip(from_dims) {
                let plan = match a {
                    AxisRead::Pinned(elem) => AxisPlan {
                        elems: vec![elem.clone()],
                        iterated_pos: None,
                    },
                    AxisRead::Iterated(_) => {
                        let pos = next_iterated_pos;
                        next_iterated_pos += 1;
                        AxisPlan {
                            elems: dimension_element_names(d),
                            iterated_pos: Some(pos),
                        }
                    }
                    AxisRead::Reduced => AxisPlan {
                        elems: dimension_element_names(d),
                        iterated_pos: None,
                    },
                };
                axis_plans.push(plan);
            }
        } else {
            // Conservative fallback: every source element, scalar agg.
            for d in from_dims {
                axis_plans.push(AxisPlan {
                    elems: dimension_element_names(d),
                    iterated_pos: None,
                });
            }
        }

        // Source → agg edges: cartesian product over the per-axis element
        // lists, routing each row to the agg result slot picked out by its
        // `Iterated` coordinates. (`next_iterated_pos` is 0 in the
        // conservative fallback.)
        let n_iterated = if read_slice_ok { next_iterated_pos } else { 0 };
        let mut rows: Vec<(Vec<String>, Vec<String>)> =
            vec![(Vec::new(), vec![String::new(); n_iterated])];
        for plan in &axis_plans {
            let mut next_rows: Vec<(Vec<String>, Vec<String>)> =
                Vec::with_capacity(rows.len() * plan.elems.len());
            for (row_elems, slot) in &rows {
                for elem in &plan.elems {
                    let mut new_row = row_elems.clone();
                    new_row.push(elem.clone());
                    let mut new_slot = slot.clone();
                    if let Some(j) = plan.iterated_pos {
                        new_slot[j] = elem.clone();
                    }
                    next_rows.push((new_row, new_slot));
                }
            }
            rows = next_rows;
        }
        for (row_elems, slot) in &rows {
            let from_node = if row_elems.len() == 1 {
                format_element_name(from_name, &row_elems[0])
            } else {
                let refs: Vec<&str> = row_elems.iter().map(String::as_str).collect();
                format_multi_element_name(from_name, &refs)
            };
            element_edges
                .entry(from_node)
                .or_default()
                .insert(agg_node_name(slot));
        }
    }

    // Agg → to edges. With no `Iterated` axes the agg is scalar and broadcasts
    // to every target element (or the single pinned target). Otherwise the agg
    // is arrayed over `iterated_dims`; it fans into `to` by the same
    // same-element-on-shared-dims projection a `Bare` reference would
    // (`expand_same_element`). An `Iterated` axis can only arise from an A2A
    // target (a per-element `Ast::Arrayed` slot has no iterated dims, and a
    // scalar target none either), so when `iterated_dims` is non-empty `to` is
    // arrayed and `target_element` is `None`.
    if iterated_dims.is_empty() {
        let to_nodes: Vec<String> = if to_dims.is_empty() {
            vec![to_name.to_string()]
        } else if let Some(elem) = target_element {
            vec![format!("{to_name}[{elem}]")]
        } else {
            cartesian_element_names(to_name, to_dims)
        };
        let entry = element_edges.entry(agg.name.clone()).or_default();
        for to_node in to_nodes {
            entry.insert(to_node);
        }
    } else {
        // Arrayed agg → arrayed `to`: project per the `Bare` arm.
        // `expand_same_element` formats source nodes as `name[elems]`, so
        // passing the agg's real name lands the edges on `agg[<slot>]`.
        debug_assert!(
            !to_dims.is_empty() && target_element.is_none(),
            "an Iterated-axis agg implies an A2A target"
        );
        expand_same_element(&agg.name, to_name, &iterated_dims, to_dims, element_edges);
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
#[derive(Clone, Debug, PartialEq, salsa::Update)]
pub struct DetectedLoop {
    /// Deterministic ID: r1, r2, ... (reinforcing/mostly-reinforcing),
    /// b1, b2, ... (balancing/mostly-balancing), u1, u2, ... (undetermined).
    pub id: String,
    /// Variable names in the loop, in circuit order.
    pub variables: Vec<String>,
    /// Loop polarity.
    pub polarity: DetectedLoopPolarity,
    /// Polarity-confidence ratio in `[0.0, 1.0]`.
    ///
    /// For the structural pipeline (`model_detected_loops`), this is `1.0`
    /// when every link in the loop has a determined sign and `0.0` when any
    /// link is unknown (the U case).  When runtime loop scores feed into
    /// classification (e.g. via [`crate::ltm::LoopPolarity::from_runtime_scores`]),
    /// the ratio comes from `|r - |b|| / (r + |b|)` over the loop-score
    /// series and can take values strictly between 0 and 1, distinguishing
    /// `Rux`/`Bux` ("mostly R/B") from `U`.
    pub polarity_confidence: f64,
}

/// Loop polarity as determined by structural analysis of link signs (and,
/// where available, by the runtime loop-score series).
///
/// `MostlyReinforcing` / `MostlyBalancing` correspond to the LTM
/// literature's "Rux" / "Bux" labels: the loop has expressed both
/// polarities at runtime but one polarity dominates with confidence at or
/// above [`crate::ltm::POLARITY_CONFIDENCE_THRESHOLD`]. The structural
/// `model_detected_loops` pipeline never produces these variants -- it has
/// no runtime data -- but downstream consumers must handle them when the
/// detected loops are enriched with simulated scores.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, salsa::Update)]
pub enum DetectedLoopPolarity {
    Reinforcing,
    Balancing,
    /// "Rux" -- mixed-sign runtime scores, predominantly reinforcing.
    MostlyReinforcing,
    /// "Bux" -- mixed-sign runtime scores, predominantly balancing.
    MostlyBalancing,
    Undetermined,
}

/// Result of full loop detection with polarity and IDs.
///
/// `Eq` cannot be derived because each `DetectedLoop` carries an `f64`
/// `polarity_confidence`; use `PartialEq` for value comparison and the
/// existing structural fields (`id`, `variables`, `polarity`) when an
/// equivalence on a stable subset is required.
#[derive(Clone, Debug, PartialEq, salsa::Update)]
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
    let module_ctx = model_module_ident_context(db, model, project, vec![]);
    // The old no-arg `variable_direct_dependencies` used a literally-empty
    // module-ident context (NOT `module_ctx`) and the `None`-inputs path;
    // reproduce that exactly with the empty context and empty input set.
    let empty_ctx = ModuleIdentContext::new(db, vec![]);
    let empty_inputs = ModuleInputSet::empty(db);
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
                let deps =
                    variable_direct_dependencies(db, *source_var, project, empty_ctx, empty_inputs);
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
                    let deps = variable_direct_dependencies(
                        db,
                        *source_var,
                        project,
                        empty_ctx,
                        empty_inputs,
                    );
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

/// Per-edge classification: the set of `RefShape`s observed at any AST
/// reference site of `from` in `to`'s equation.
///
/// Keyed at variable level (not element level). For each variable-level
/// edge `(from, to)` from `model_causal_edges`, this records the multiset
/// of distinct shapes the source takes on at every reference site -- one
/// entry per `(from, to)` pair, value is a `BTreeSet` (deduplicated and
/// canonically ordered) so consumers can iterate deterministically.
///
/// Used by tiered loop enumeration in `model_loop_circuits_tiered` to
/// classify each variable-level cycle as `PureScalar`,
/// `PureSameElementA2A`, or `CrossElementOrMixed` without re-walking
/// every target's AST. The cycle classifier needs only the *set* of
/// shapes per edge; per-site duplicates contribute the same answer.
///
/// Edges that have no AST reference -- structural flow->stock edges
/// where the stock equation is just the initial value -- map to
/// `{Bare}`. The flow is structurally a same-element diagonal into
/// the stock; treating it as Bare matches what
/// `model_element_causal_edges` does for the same case.
///
/// Module input edges (a `to` whose kind is `Module`) and edges into
/// implicit module-instance variables also map to `{Bare}`: modules are
/// scalar nodes in the causal graph, so per-shape distinctions don't
/// apply. Unable-to-reconstruct edges (a defensive fallback that
/// shouldn't happen for well-formed models) likewise map to `{Bare}`.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct EdgeShapesResult {
    /// Map from variable-level edge `(from, to)` to the set of
    /// `RefShape`s observed at any reference site of `from` in `to`'s
    /// AST. Keys mirror the `(from, to)` pairs from
    /// `model_causal_edges`.
    pub edge_shapes: HashMap<(String, String), BTreeSet<RefShape>>,
}

/// Tag every variable-level edge with the set of `RefShape`s observed
/// at any AST reference site of the edge's source in the edge's target.
///
/// This is the per-edge classification that drives tiered loop
/// enumeration: cycles can be classified as pure-A2A, pure-scalar, or
/// cross-element/mixed by inspecting the shape set on each cycle edge.
///
/// A projection of `model_ltm_reference_sites` (the shared reference-site
/// IR): the shape set per `(from, to)` edge is just the distinct `shape`
/// fields of that edge's `ClassifiedSite`s -- this function does no AST
/// walk of its own. The structural / module short-circuit (no AST
/// reference exists, so the IR has no entry) maps to `{Bare}`, matching
/// `model_element_causal_edges`'s treatment of the same edges.
#[salsa::tracked(returns(ref))]
pub fn model_edge_shapes(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> EdgeShapesResult {
    let variable_edges = model_causal_edges(db, model, project);
    let source_vars = model.variables(db);
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);

    // Build a set of structural flow->stock edges so we can label them
    // `{Bare}` directly: stock equations contain only the initial value,
    // so the flow name never appears in the stock's AST and the IR has no
    // entry for the edge. The structural edge is semantically a
    // same-element diagonal, matching the Bare classification used by
    // `model_element_causal_edges` for the same case.
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

    let mut edge_shapes: HashMap<(String, String), BTreeSet<RefShape>> = HashMap::new();
    for (from_name, to_set) in &variable_edges.edges {
        for to_name in to_set {
            // Module edges and structural flow->stock edges short-circuit
            // to {Bare}. Module targets are scalar nodes in the causal
            // graph; structural stock edges (and any other edge with no
            // AST reference -- e.g. a synthesized dep, or an
            // unreconstructable target) have no IR entry, which also maps
            // to {Bare} below, but flagging the structural/module edges
            // here keeps the intent explicit.
            let to_is_module = source_vars
                .get(to_name)
                .map(|sv| sv.kind(db) == super::SourceVariableKind::Module)
                .unwrap_or(false);
            if to_is_module
                || structural_flow_to_stock.contains(&(from_name.clone(), to_name.clone()))
            {
                let mut set = BTreeSet::new();
                set.insert(RefShape::Bare);
                edge_shapes.insert((from_name.clone(), to_name.clone()), set);
                continue;
            }

            // Project the IR: the shape set is the distinct `shape` fields
            // of this edge's classified sites. An edge that exists in the
            // variable graph but has no AST reference (or whose target
            // couldn't be reconstructed) has no IR entry -> default to
            // {Bare} so the cycle classifier sees a same-element shape
            // rather than an empty set (which would be ambiguous).
            let mut set: BTreeSet<RefShape> = ir
                .sites
                .get(&(from_name.clone(), to_name.clone()))
                .map(|sites| sites.iter().map(|s| s.shape.clone()).collect())
                .unwrap_or_default();
            if set.is_empty() {
                set.insert(RefShape::Bare);
            }
            edge_shapes.insert((from_name.clone(), to_name.clone()), set);
        }
    }

    EdgeShapesResult { edge_shapes }
}

/// Classification of a variable-level cycle for tiered loop enumeration.
///
/// Drives the decision in `model_loop_circuits_tiered` of whether a
/// cycle can be emitted as a single `Loop` directly (fast path) or
/// must descend into element-level Johnson on the cycle's induced
/// subgraph (slow path).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CycleClass {
    /// Every variable in the cycle is scalar and every traversed edge
    /// has a `Bare` reference. The cycle exists exactly once at scalar
    /// granularity; emit one scalar `Loop` and skip the element-level
    /// enumerator.
    PureScalar,
    /// Every variable in the cycle is arrayed over the same dimension
    /// list and every traversed edge has only `Bare` references. The
    /// cycle exists at every element of the shared dimensions; emit one
    /// A2A `Loop` whose `dimensions` field carries those dimensions'
    /// canonical names (lex-ordered as they appear on the participating
    /// variables) and skip the element-level enumerator.
    PureSameElementA2A {
        /// Canonical (lower-case) dimension names, in source order from
        /// the participating variables' dimension list. The list is
        /// identical for every variable in the cycle (otherwise the
        /// cycle classifies as `CrossElementOrMixed`).
        dimensions: Vec<String>,
    },
    /// At least one edge has a non-Bare shape (Wildcard, FixedIndex, or
    /// DynamicIndex), or the cycle mixes scalar and arrayed nodes, or
    /// the cycle's arrayed nodes don't share the same dimension list.
    /// The cycle requires element-level enumeration on the slow-path
    /// subgraph induced by its variables.
    CrossElementOrMixed,
}

/// Classify a variable-level cycle into a `CycleClass`.
///
/// Pure helper -- no DB access. Inputs:
/// - `cycle`: the variable-level node sequence in cycle order. The
///   cycle is closed implicitly: the edge from `cycle[k-1]` back to
///   `cycle[0]` is included.
/// - `edge_shapes`: per-edge `RefShape` sets from `model_edge_shapes`.
///   Edges absent from the map are treated as `{Bare}` (defensive,
///   matches the `model_edge_shapes` fallback for unable-to-reconstruct
///   targets).
/// - `dim_lookup`: per-variable dimension list. Variables absent from
///   this lookup (which shouldn't happen for cycle nodes) are treated
///   as scalar.
///
/// Classification rules (applied in order):
///
/// 1. If any edge has a `Wildcard`, `DynamicIndex`, or `FixedIndex`
///    shape (or any non-Bare shape co-existing with Bare),
///    `CrossElementOrMixed`. A FixedIndex reference pins the cycle to a
///    specific element subscript distinct from the rest of its
///    neighbours' broadcast semantics; the cycle cannot be emitted as
///    a single A2A loop. A Wildcard reducer pulls in cross-element
///    contributions, so the cycle is structurally cross-element.
///    DynamicIndex is conservatively treated like Wildcard.
/// 2. If every variable has an empty dimension list (all scalar),
///    `PureScalar`.
/// 3. If every variable has the *same* non-empty dimension list,
///    `PureSameElementA2A` with that dimension list.
/// 4. Otherwise (mixed scalar / arrayed nodes, or arrayed nodes with
///    differing dimension lists), `CrossElementOrMixed`.
///
/// Empty cycles are degenerate; treat them as `PureScalar` for the
/// caller's convenience (they emit no Loop in practice).
pub(crate) fn classify_cycle(
    cycle: &[String],
    edge_shapes: &EdgeShapesResult,
    dim_lookup: &impl Fn(&str) -> Vec<crate::dimensions::Dimension>,
) -> CycleClass {
    if cycle.is_empty() {
        return CycleClass::PureScalar;
    }

    // Rule 1: scan all edges in cycle order. If any edge carries a
    // non-Bare shape, the cycle is cross-element / mixed.
    let n = cycle.len();
    for i in 0..n {
        let from = &cycle[i];
        let to = &cycle[(i + 1) % n];
        let key = (from.clone(), to.clone());
        let shapes = match edge_shapes.edge_shapes.get(&key) {
            Some(s) => s,
            None => continue, // missing edge -> treat as Bare
        };
        for shape in shapes {
            match shape {
                RefShape::Bare => {}
                RefShape::FixedIndex(_) | RefShape::Wildcard | RefShape::DynamicIndex => {
                    return CycleClass::CrossElementOrMixed;
                }
            }
        }
    }

    // Rule 2 / 3 / 4: dimension uniformity check.
    let first_dims = dim_lookup(&cycle[0]);
    let any_arrayed = !first_dims.is_empty()
        || cycle
            .iter()
            .skip(1)
            .any(|name| !dim_lookup(name).is_empty());
    if !any_arrayed {
        return CycleClass::PureScalar;
    }

    // Rule 3: every variable must have *the same* non-empty dimensions.
    if first_dims.is_empty() {
        return CycleClass::CrossElementOrMixed;
    }
    for name in cycle.iter().skip(1) {
        let dims = dim_lookup(name);
        if dims != first_dims {
            return CycleClass::CrossElementOrMixed;
        }
    }
    CycleClass::PureSameElementA2A {
        dimensions: first_dims.iter().map(|d| d.name().to_string()).collect(),
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

    // The reference-site classification IR decides each reference's access
    // shape and aggregate-node routing; `enumerate_agg_nodes` is consulted
    // only to resolve a `ThroughAgg` site's `AggRef` to the synthetic agg
    // (and its `read_slice` / `result_dims`). A `ThroughAgg` site routes only
    // the rows the reducer reads through the agg (`emit_agg_routed_edges`),
    // never the all-pairs cross-product; a `Direct` site uses its
    // `shape`/`target_element` via `emit_edges_for_reference` -- a
    // `DynamicIndex` shape (`arr[i+1]`, a range, the not-hoisted dynamic-index
    // reducer carve-out `SUM(pop[idx,*])`) still expands to the conservative
    // full cross-product there.
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    let agg_nodes = crate::ltm_agg::enumerate_agg_nodes(db, model, project);

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

    // Build a set of structural flow->stock edges so we can skip
    // classification for them. Stock equations contain only the initial
    // value, so the flow name never appears in the stock's AST (the IR has
    // no entry); without this bypass an arrayed stock's edge would fall
    // through to the empty-sites fallback below, which is the same
    // SameElement diagonal -- but flagging it here keeps the intent
    // explicit and matches `model_edge_shapes`.
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

    // Expand each variable-level edge to element-level edges by reading the
    // IR's classified sites for that `(from, to)` pair. `Direct` sites emit
    // per-shape element edges (deduped naturally via the `BTreeSet` value);
    // `ThroughAgg` sites route only the rows the reducer reads through the
    // synthetic agg (`source[<read slice>] → agg[<iterated>]`,
    // `agg[<iterated>] → to[e]` per `emit_agg_routed_edges`). An edge with no
    // IR entry (structural flow->stock, a module edge, an unreconstructable
    // target, or a synthesized dep with no AST reference) falls back to a
    // SameElement-diagonal `Bare` emission so the variable-level projection
    // invariant still holds.
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

            let edge_key = (from_name.clone(), to_name.clone());
            let classified = ir.sites.get(&edge_key);

            // Structural flow->stock edges, or any edge with no AST
            // reference: SameElement diagonal `Bare` emission.
            let is_structural_flow_to_stock = structural_flow_to_stock.contains(&edge_key)
                && !from_dims.is_empty()
                && !to_dims.is_empty();
            if is_structural_flow_to_stock || classified.map(Vec::is_empty).unwrap_or(true) {
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

            for site in classified.expect("classified is Some -- checked above") {
                match &site.routing {
                    crate::db::ltm_ir::SiteRouting::Direct => {
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
                    crate::db::ltm_ir::SiteRouting::ThroughAgg { agg } => {
                        // Route only the rows the reducer reads through the
                        // agg: `source[<pinned>,<iterated>,<reduced→all>] →
                        // agg[<iterated>]` per `Iterated`-axis combination,
                        // then `agg[<iterated>] → to[e]` (the agg's
                        // `result_dims` drive how it fans out into `to` --
                        // diagonal on shared dims, broadcast otherwise, the
                        // same projection the `Bare` arm does). A whole-extent
                        // reducer (`read_slice` all-`Reduced`) degenerates to
                        // the prior behavior: the agg is scalar, every source
                        // element feeds it, and it broadcasts to every `to`
                        // element. No `source[d] → to[e]` direct edge is
                        // emitted for a hoisted reducer -- only the two halves.
                        let agg_node = &agg_nodes.aggs[agg.0];
                        emit_agg_routed_edges(
                            from_name,
                            to_name,
                            &from_dims,
                            &to_dims,
                            agg_node,
                            site.target_element.as_deref(),
                            &mut element_edges,
                        );
                    }
                }
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
                // Structural classification has no runtime score data, so
                // confidence is binary: 1.0 when every link in the loop has
                // a determined polarity (R/B), 0.0 when any link is unknown
                // and the loop falls back to Undetermined.  The structural
                // path never produces MostlyReinforcing/MostlyBalancing --
                // those variants are reserved for callers that classify on
                // top of a simulated loop-score series via
                // [`LoopPolarity::from_runtime_scores`].
                let (polarity, polarity_confidence) = match l.polarity {
                    crate::ltm::LoopPolarity::Reinforcing => {
                        (DetectedLoopPolarity::Reinforcing, 1.0)
                    }
                    crate::ltm::LoopPolarity::Balancing => (DetectedLoopPolarity::Balancing, 1.0),
                    crate::ltm::LoopPolarity::MostlyReinforcing => {
                        (DetectedLoopPolarity::MostlyReinforcing, 1.0)
                    }
                    crate::ltm::LoopPolarity::MostlyBalancing => {
                        (DetectedLoopPolarity::MostlyBalancing, 1.0)
                    }
                    crate::ltm::LoopPolarity::Undetermined => {
                        (DetectedLoopPolarity::Undetermined, 0.0)
                    }
                };
                DetectedLoop {
                    id: l.id,
                    variables: vars,
                    polarity,
                    polarity_confidence,
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
///
/// **Legacy.** Pre-#482 LTM compilation ran Johnson on the full element
/// graph here, which inflated pure-A2A cycles to N circuits per cycle.
/// The LTM pipeline now uses [`model_loop_circuits_tiered`] instead,
/// which short-circuits pure-A2A and pure-scalar cycles in the fast path
/// and runs Johnson only on the cross-element / mixed slice. This
/// function is retained for diagnostic / measurement-postscript tests
/// and external consumers that still want the unfiltered element-level
/// circuit list. New LTM callers must use `model_loop_circuits_tiered`;
/// any new direct call to `model_element_loop_circuits` should be
/// reviewed against the bug recap in
/// `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`.
#[deprecated(
    since = "0.2.0",
    note = "Use `model_loop_circuits_tiered` for LTM compilation; this \
            full-element Johnson run is retained for measurement and \
            external diagnostic callers only."
)]
// `#[salsa::tracked]` expands to internal generated code that calls back
// into this function by name, which would re-trigger the `deprecated`
// warning inside the macro expansion. `#[allow(deprecated)]` on the
// outer item suppresses both the macro-internal callsite and any
// deprecation lint applied to the salsa-generated companion items, so
// the lint still fires for real external callers (re-exports in
// `db.rs`, test/example code) -- which is exactly what we want.
#[allow(deprecated)]
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

/// One variable-level cycle classified as fast-path
/// (PureScalar / PureSameElementA2A) by the tiered loop enumerator.
///
/// Materializes directly into a single `Loop` without entering
/// element-level Johnson. The shape of the emitted Loop is decided by
/// the dimensions field: empty -> scalar Loop; non-empty -> A2A Loop
/// with `dimensions` set.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct FastPathCircuit {
    /// Variable names in cycle order (canonical / lower-case).
    pub variables: Vec<String>,
    /// Empty for `PureScalar`; the shared dimension *canonical names*
    /// (lower-case) for `PureSameElementA2A`. Canonical names rather
    /// than full `Dimension` values so the result type's
    /// auto-derivable traits (`Debug`, `Eq`) don't depend on
    /// `Dimension`'s feature-gated derives. The consumer
    /// (`build_loops_from_tiered`) maps canonical names back to
    /// datamodel names via the project's `dm_dims` list.
    pub dimensions: Vec<String>,
}

/// Result of the tiered loop enumerator: variable-level cycles
/// pre-classified into fast and slow paths.
///
/// The fast path holds cycles that the cycle classifier could resolve
/// without element-level enumeration: pure scalar cycles and pure
/// same-element A2A cycles. Each entry in `fast_path` materializes
/// into a single `Loop` directly.
///
/// The slow path holds element-level circuits enumerated by Johnson
/// on the *induced subgraph* over the variables that participate in
/// any `CrossElementOrMixed` variable-level cycle. When no such cycles
/// exist the slow path is empty -- that's the headline win for pure
/// A2A or pure scalar models.
///
/// `slow_path_largest_scc` reports the largest SCC of the slow-path
/// subgraph (computed via Tarjan, cheap), regardless of whether
/// Johnson actually ran on it. Callers gate auto-flip on this value
/// instead of the *full* element-graph SCC -- the pure-A2A and
/// pure-scalar cycles never inflate the slow-path subgraph, so this
/// number is the structurally correct upper bound on the cost of
/// running slow-path Johnson.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct TieredCircuitsResult {
    /// Variable-level cycles that the classifier resolved to a single
    /// Loop without element-level Johnson.
    pub fast_path: Vec<FastPathCircuit>,
    /// Element-level circuits from the slow-path subgraph. Empty when
    /// no variable-level cycle classifies as `CrossElementOrMixed` or
    /// when the subgraph SCC exceeds the auto-flip threshold and
    /// Johnson was skipped to avoid the cost cliff.
    /// Indexed in the same canonical (lex-sorted) form as
    /// `model_element_loop_circuits` so downstream grouping logic in
    /// `build_element_level_loops` can be reused unchanged.
    pub slow_path: LoopCircuitsResult,
    /// Largest SCC in the slow-path element-level subgraph. 0 when no
    /// cycle classifies as `CrossElementOrMixed`. Callers compare
    /// this against `MAX_LTM_SCC_NODES` to decide auto-flip.
    pub slow_path_largest_scc: usize,
}

/// Tiered loop enumeration: variable-level Johnson first, then
/// element-level Johnson only on the slow-path subgraph.
///
/// Replaces the cost asymmetry of running Johnson on the full element
/// graph for pure-A2A models. With V variables over N elements:
///
/// - Today (`model_element_loop_circuits`): a pure-A2A cycle of size K
///   inflates to N element-level circuits, costing O(K * N) per cycle.
/// - With tiered enumeration: pure-A2A cycles are emitted in the
///   fast path with no per-element expansion, costing O(K). Slow-path
///   Johnson runs only on the induced subgraph, which is bounded by
///   the variables in `CrossElementOrMixed` cycles times their
///   dimension elements -- a strict subset of the full element graph.
///
/// See `docs/design-plans/2026-05-06-ltm-482-variable-level-loop-enumeration.md`
/// for the cost model and fixture-by-fixture impact predictions.
#[salsa::tracked(returns(ref))]
pub fn model_loop_circuits_tiered(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> TieredCircuitsResult {
    use std::collections::HashSet;

    let var_circuits = model_loop_circuits(db, model, project);
    let edge_shapes = model_edge_shapes(db, model, project);
    let source_vars = model.variables(db);

    // Per-variable dimension lookup. Cached locally because a variable
    // can appear in many cycles; the salsa-tracked `variable_dimensions`
    // is itself memoized but the per-call HashMap lookup avoids
    // repeated salsa cache hits.
    let mut dim_cache: HashMap<String, Vec<crate::dimensions::Dimension>> = HashMap::new();
    let mut lookup_dims = |name: &str| -> Vec<crate::dimensions::Dimension> {
        if let Some(dims) = dim_cache.get(name) {
            return dims.clone();
        }
        let dims = source_vars
            .get(name)
            .map(|sv| super::variable_dimensions(db, *sv, project).to_vec())
            .unwrap_or_default();
        dim_cache.insert(name.to_string(), dims.clone());
        dims
    };

    let mut fast_path: Vec<FastPathCircuit> = Vec::new();
    let mut slow_path_var_nodes: HashSet<String> = HashSet::new();

    for circuit in &var_circuits.circuits {
        let cycle: Vec<String> = circuit
            .iter()
            .map(|i| var_circuits.names[*i as usize].clone())
            .collect();

        // The cycle classifier needs a closure that doesn't capture
        // the mutable `dim_cache` borrow during classification. We
        // pre-fetch every cycle node's dimensions into a small map
        // first, then hand the classifier a closure that reads it.
        let mut cycle_dims: HashMap<String, Vec<crate::dimensions::Dimension>> = HashMap::new();
        for v in &cycle {
            cycle_dims.insert(v.clone(), lookup_dims(v));
        }
        let cycle_lookup = |name: &str| -> Vec<crate::dimensions::Dimension> {
            cycle_dims.get(name).cloned().unwrap_or_default()
        };

        match classify_cycle(&cycle, edge_shapes, &cycle_lookup) {
            CycleClass::PureScalar => fast_path.push(FastPathCircuit {
                variables: cycle,
                dimensions: vec![],
            }),
            CycleClass::PureSameElementA2A { dimensions } => fast_path.push(FastPathCircuit {
                variables: cycle,
                dimensions,
            }),
            CycleClass::CrossElementOrMixed => {
                for v in cycle {
                    slow_path_var_nodes.insert(v);
                }
            }
        }
    }

    // Slow-path subgraph: project the element graph onto every variable
    // that appears in *any* `CrossElementOrMixed` cycle. The induction
    // is intentionally broad -- a variable that participates in even one
    // cross-element cycle must keep its element nodes in the subgraph so
    // Johnson can re-discover the cross-element traversal -- but the
    // breadth has a side effect: a pure-A2A cycle that *shares variables*
    // with a cross-element cycle (typical when a small "aux ring" feeds
    // both a same-element loop and a longer cross-element loop) gets
    // dragged into the subgraph too. Johnson then re-finds the per-element
    // reflections of that pure-A2A cycle, which `build_element_level_loops`
    // would collapse back into an A2A `Loop` -- the same `Loop` the fast
    // path already emitted. To prevent the duplicate, we dedupe slow-path
    // circuits below against the fast-path emissions before returning.
    //
    // We compute the subgraph's largest SCC via Tarjan (cheap) before
    // running Johnson, so the caller can auto-flip on huge cross-element
    // subgraphs without paying for circuit enumeration. Johnson is
    // skipped (and `slow_path` returned empty) when the SCC exceeds
    // `MAX_LTM_SCC_NODES`; the SCC count is exposed on
    // `slow_path_largest_scc` either way.
    let (slow_path, slow_path_largest_scc) = if slow_path_var_nodes.is_empty() {
        (
            LoopCircuitsResult {
                names: Vec::new(),
                circuits: Vec::new(),
            },
            0,
        )
    } else {
        let element_edges = model_element_causal_edges(db, model, project);
        // A node is kept in the slow-path subgraph if its variable name is
        // in the cross-element/mixed set OR it is a synthetic aggregate node
        // (`$⁚ltm⁚agg⁚{n}`). Aggregate nodes have no variable-level
        // counterpart -- `strip_element_subscript` is a no-op on them -- but
        // a cross-element loop through a hoisted reducer genuinely traverses
        // the agg node (twice), so dropping it would hide that loop.
        // Including a stray agg whose neighbors are not slow-path vars just
        // adds an isolated node, harmless.
        let keep_node = |name: &str| -> bool {
            slow_path_var_nodes.contains(strip_element_subscript(name))
                || crate::ltm_agg::is_synthetic_agg_name(name)
        };
        let mut sub_edges: HashMap<String, BTreeSet<String>> = HashMap::new();
        for (from, tos) in &element_edges.edges {
            if !keep_node(from) {
                continue;
            }
            let mut filtered: BTreeSet<String> = BTreeSet::new();
            for to in tos {
                if keep_node(to) {
                    filtered.insert(to.clone());
                }
            }
            if !filtered.is_empty() {
                sub_edges.insert(from.clone(), filtered);
            }
        }
        // Stocks restricted to slow-path variables. Same projection rule:
        // keep an element-stock node only if its variable name is in
        // the slow-path set. (Agg nodes are never stocks.)
        let sub_stocks: std::collections::HashSet<crate::common::Ident<crate::common::Canonical>> =
            element_edges
                .stocks
                .iter()
                .filter(|s| slow_path_var_nodes.contains(strip_element_subscript(s.as_str())))
                .map(|s| crate::common::Ident::new(s))
                .collect();
        let sub_edge_idents: HashMap<
            crate::common::Ident<crate::common::Canonical>,
            Vec<crate::common::Ident<crate::common::Canonical>>,
        > = sub_edges
            .into_iter()
            .map(|(from, tos)| {
                (
                    crate::common::Ident::new(&from),
                    tos.into_iter()
                        .map(|t| crate::common::Ident::new(&t))
                        .collect(),
                )
            })
            .collect();
        let graph = crate::ltm::CausalGraph {
            edges: sub_edge_idents,
            stocks: sub_stocks,
            variables: HashMap::new(),
            module_graphs: HashMap::new(),
        };
        let scc = graph.largest_scc_size();
        if scc > crate::ltm::MAX_LTM_SCC_NODES {
            // Skip Johnson on a huge cross-element subgraph; the
            // caller will auto-flip on the SCC count.
            (
                LoopCircuitsResult {
                    names: Vec::new(),
                    circuits: Vec::new(),
                },
                scc,
            )
        } else {
            let (names, circuits) = graph
                .find_indexed_circuits_with_limit(usize::MAX)
                .expect("usize::MAX cannot exhaust the enumeration budget");
            // Dedup slow-path circuits whose stripped variable-level
            // node sequence matches a fast-path circuit. This drops the
            // per-element reflections of pure-A2A cycles that share
            // variables with a cross-element cycle (see the slow-path
            // subgraph-induction comment above for the topology). Each
            // dropped circuit would otherwise be re-collapsed into an
            // A2A `Loop` by `build_element_level_loops`, duplicating the
            // `Loop` the fast path already emitted.
            //
            // The match is rotation-invariant: both fast-path
            // (variable-level) and slow-path (element-level) Johnson
            // emit each cycle starting from its lex-smallest node, but
            // the rotations might still differ when two distinct slow-
            // path nodes happen to strip to the same variable name
            // (impossible for the simple-cycle case we dedupe here,
            // but we re-canonicalize defensively). Slow-path cycles
            // whose stripped names contain repeats cannot collapse onto
            // a fast-path cycle (Johnson emits simple cycles, so fast-
            // path entries always have distinct nodes); we skip the key
            // computation for those circuits.
            let fast_path_keys: HashSet<Vec<String>> = fast_path
                .iter()
                .map(|fp| canonical_cycle_rotation(&fp.variables))
                .collect();
            let mut filtered_circuits: Vec<Vec<u32>> = Vec::with_capacity(circuits.len());
            for circuit in circuits {
                // `names` holds element-suffixed labels (e.g. `a[nyc]`).
                // Stripping recovers the variable-level sequence used
                // to build fast-path keys.
                let stripped: Vec<String> = circuit
                    .iter()
                    .map(|i| strip_element_subscript(&names[*i as usize]).to_string())
                    .collect();
                let mut seen: HashSet<&str> = HashSet::with_capacity(stripped.len());
                let unique = stripped.iter().all(|s| seen.insert(s.as_str()));
                if unique {
                    let key = canonical_cycle_rotation(&stripped);
                    if fast_path_keys.contains(&key) {
                        continue;
                    }
                }
                filtered_circuits.push(circuit);
            }
            (
                LoopCircuitsResult {
                    names,
                    circuits: filtered_circuits,
                },
                scc,
            )
        }
    };

    TieredCircuitsResult {
        fast_path,
        slow_path,
        slow_path_largest_scc,
    }
}

/// Return the rotation of `nodes` that starts at the lex-smallest entry.
///
/// The fast-path / slow-path dedup needs a rotation-invariant key for a
/// directed cycle. Johnson's algorithm emits each cycle starting at its
/// lex-smallest node, so for fast-path circuits and the stripped form of
/// slow-path circuits with all-distinct entries the input is already in
/// canonical form. We re-canonicalize defensively here so the dedup
/// remains correct if Johnson's emission order ever shifts.
///
/// `nodes` with repeated entries are returned as-is: such sequences are
/// only produced by cross-element cycles visiting the same variable at
/// multiple elements, and those cannot match any fast-path entry (which
/// always has distinct nodes). Callers that compute the dedup key must
/// pre-check that the stripped sequence is unique.
fn canonical_cycle_rotation(nodes: &[String]) -> Vec<String> {
    if nodes.is_empty() {
        return Vec::new();
    }
    let mut best = 0;
    for i in 1..nodes.len() {
        if nodes[i] < nodes[best] {
            best = i;
        }
    }
    let mut rotated: Vec<String> = Vec::with_capacity(nodes.len());
    rotated.extend(nodes[best..].iter().cloned());
    rotated.extend(nodes[..best].iter().cloned());
    rotated
}

/// Strip an element-subscript suffix from a node name.
///
/// `population[nyc]` -> `population`; `population[nyc,boston]` ->
/// `population`; a name without `[` is returned unchanged. Mirrors
/// `crate::ltm::strip_subscript` (truncates at the last `[`, so a
/// hypothetical nested-bracket name would collapse fully); inlined
/// here to keep the tiered enumerator self-contained.
fn strip_element_subscript(name: &str) -> &str {
    match name.rfind('[') {
        Some(pos) => &name[..pos],
        None => name,
    }
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
    let module_ctx = model_module_ident_context(db, model, project, vec![]);
    // The datamodel dims are needed by `reconstruct_implicit_variable`; the
    // canonicalized context comes from the project-global salsa-cached query.
    let dims = project_datamodel_dims(db, project);
    let dim_context = project_dimensions_context(db, project);
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: dim_context,
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
                reconstruct_implicit_variable(db, model, dims, &scope, implicit_dm_var);
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
    let module_ctx = model_module_ident_context(db, model, project, vec![]);
    // The datamodel dims are needed by `reconstruct_implicit_variable`; the
    // canonicalized context comes from the project-global salsa-cached query.
    let dims = project_datamodel_dims(db, project);
    let dim_context = project_dimensions_context(db, project);
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: dim_context,
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
                    reconstruct_implicit_variable(db, model, dims, &scope, implicit_dm_var);
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

    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
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

/// Tests for the `polarity_confidence` field surfaced on `DetectedLoop`
/// (issue #485).  The structural pipeline can only assign 1.0 or 0.0,
/// so these tests pin both ends of that boundary; the runtime-aware
/// classification (Rux/Bux) is covered by the
/// `LoopPolarity::from_runtime_scores` unit tests in `ltm/tests.rs`.
#[cfg(test)]
mod polarity_confidence_tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::ltm::{LoopPolarity, POLARITY_CONFIDENCE_THRESHOLD};
    use crate::test_common::TestProject;

    /// Helper: detect loops for a TestProject and return the result.
    fn detect_loops(project: &TestProject) -> DetectedLoopsResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        model_detected_loops(&db, source_model, sync.project)
    }

    /// A textbook reinforcing population loop has every link with a known
    /// positive polarity, so structural detection assigns confidence 1.0.
    #[test]
    fn structural_reinforcing_loop_has_full_confidence() {
        let project = TestProject::new("pop_growth")
            .stock("population", "100", &["births"], &[], None)
            .flow("births", "population * birth_rate", None)
            .aux("birth_rate", "0.02", None);
        let result = detect_loops(&project);
        assert_eq!(result.loops.len(), 1, "exactly one loop expected");
        let loop_item = &result.loops[0];
        assert_eq!(loop_item.polarity, DetectedLoopPolarity::Reinforcing);
        assert!(
            (loop_item.polarity_confidence - 1.0).abs() < f64::EPSILON,
            "fully-determined R loops must surface confidence 1.0, got {}",
            loop_item.polarity_confidence
        );
    }

    /// A goal-seeking balancing loop with all-known link polarities also
    /// gets confidence 1.0 from the structural pipeline.
    #[test]
    fn structural_balancing_loop_has_full_confidence() {
        let project = TestProject::new("goal_seek")
            .stock("level", "100", &["adjustment"], &[], None)
            .flow("adjustment", "gap / adjustment_time", None)
            .aux("gap", "goal - level", None)
            .aux("goal", "200", None)
            .aux("adjustment_time", "5", None);
        let result = detect_loops(&project);
        let balancing = result
            .loops
            .iter()
            .find(|l| l.polarity == DetectedLoopPolarity::Balancing)
            .expect("balancing loop must be present");
        assert!(
            (balancing.polarity_confidence - 1.0).abs() < f64::EPSILON,
            "fully-determined B loops must surface confidence 1.0, got {}",
            balancing.polarity_confidence
        );
    }

    /// A loop whose link polarity cannot be determined statically yields
    /// `Undetermined` with confidence 0.0 -- the structural pipeline has
    /// no signed runtime evidence at this point.  This pins the U-side
    /// of the binary structural confidence rule.  We use a non-monotonic
    /// graphical function (lookup table) on the feedback edge to force
    /// `LinkPolarity::Unknown`; the conservative loop-polarity rule then
    /// upgrades the loop to Undetermined.
    #[test]
    fn structural_undetermined_loop_has_zero_confidence() {
        use crate::datamodel;
        use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};

        let mut model_vars = vec![
            x_stock("water", "100", &[], &["outflow"], None),
            x_flow("outflow", "water * lookup(rate, water)", None),
        ];
        let mut lookup_var = x_aux("rate", "0", None);
        if let datamodel::Variable::Aux(aux) = &mut lookup_var {
            // Increasing then decreasing: monotonicity-inferring polarity
            // analysis must concede `Unknown` here.
            aux.gf = Some(datamodel::GraphicalFunction {
                kind: datamodel::GraphicalFunctionKind::Continuous,
                x_points: Some(vec![0.0, 50.0, 100.0, 150.0]),
                y_points: vec![0.1, 0.5, 0.2, 0.6],
                x_scale: datamodel::GraphicalFunctionScale {
                    min: 0.0,
                    max: 150.0,
                },
                y_scale: datamodel::GraphicalFunctionScale { min: 0.1, max: 0.6 },
            });
        }
        model_vars.push(lookup_var);

        let model = x_model("main", model_vars);
        let datamodel = x_project(sim_specs_with_units("months"), &[model]);
        let db = SimlinDb::default();
        let sync = crate::db::sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let result = model_detected_loops(&db, source_model, sync.project);
        assert!(!result.loops.is_empty(), "should detect at least one loop");
        let undetermined = result
            .loops
            .iter()
            .find(|l| l.polarity == DetectedLoopPolarity::Undetermined)
            .expect("loop containing non-monotone lookup must be Undetermined");
        assert!(
            undetermined.polarity_confidence.abs() < f64::EPSILON,
            "structural Undetermined loops carry confidence 0.0, got {}",
            undetermined.polarity_confidence
        );
    }

    /// End-to-end check: when LTM is enabled and a small model is
    /// simulated, runtime classification of single-sign loop scores
    /// is consistent with what an LTM consumer would derive from the
    /// simulated loop_score series for unambiguous loops.
    #[test]
    fn small_simulated_model_surfaces_consistent_confidence() {
        use crate::CompiledSimulation;
        use crate::db::{
            compile_project_incremental, set_project_ltm_enabled, sync_from_datamodel_incremental,
        };
        use crate::vm::Vm;

        let datamodel_project = TestProject::new("logistic_for_confidence")
            .with_sim_time(0.0, 30.0, 0.25)
            .stock("population", "10", &["births"], &["deaths"], None)
            .flow("births", "population * birth_rate", None)
            .flow("deaths", "population * population / capacity", None)
            .aux("birth_rate", "0.1", None)
            .aux("capacity", "100", None)
            .build_datamodel();

        // Structural detection: r1 (reinforcing) + b1 (carrying capacity).
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &datamodel_project, None);
        let source_model = sync.models["main"].source_model;
        let detected = model_detected_loops(&db, source_model, sync.project);
        assert!(
            detected.loops.len() >= 2,
            "expected at least one R loop and one B loop, got {} loops",
            detected.loops.len()
        );
        for loop_item in &detected.loops {
            assert!(
                (loop_item.polarity_confidence - 1.0).abs() < f64::EPSILON,
                "all-known-link loops in this model must have structural confidence 1.0; loop {} got {}",
                loop_item.id,
                loop_item.polarity_confidence
            );
        }

        // Compile with LTM, simulate, and classify each loop's runtime
        // score series.  The reinforcing births loop and the balancing
        // carrying-capacity loop both keep a single sign throughout, so
        // both should classify with confidence 1.0.
        set_project_ltm_enabled(&mut db, sync.project, true);
        let compiled: CompiledSimulation =
            compile_project_incremental(&db, sync.project, "main").unwrap();
        let mut vm = Vm::new(compiled).unwrap();
        vm.run_to_end().unwrap();
        let results = vm.into_results();

        for loop_item in &detected.loops {
            let var_name = format!("$\u{205A}ltm\u{205A}loop_score\u{205A}{}", loop_item.id);
            let Some(&offset) = results.offsets.get(var_name.as_str()) else {
                continue;
            };
            let series: Vec<f64> = (0..results.step_count)
                .map(|step| results.data[step * results.step_size + offset])
                .collect();
            if let Some((runtime_polarity, runtime_confidence)) =
                LoopPolarity::from_runtime_scores(&series)
            {
                assert!(
                    !matches!(runtime_polarity, LoopPolarity::Undetermined),
                    "simulated single-sign loop {} must not classify as Undetermined",
                    loop_item.id
                );
                assert!(
                    runtime_confidence >= POLARITY_CONFIDENCE_THRESHOLD,
                    "loop {} runtime confidence {} should clear the {} threshold",
                    loop_item.id,
                    runtime_confidence,
                    POLARITY_CONFIDENCE_THRESHOLD
                );
            }
        }
    }
}

#[cfg(test)]
#[path = "element_graph_tests.rs"]
mod element_graph_tests;

#[cfg(test)]
mod tiered_circuits_tests {
    //! Integration tests for `model_loop_circuits_tiered`. Exercises
    //! the salsa pipeline on small synthetic fixtures and pins the
    //! fast-path / slow-path partition.
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    fn tiered(project: &TestProject) -> TieredCircuitsResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        model_loop_circuits_tiered(&db, source_model, source_project).clone()
    }

    /// Pure-A2A model: `population[r] -> births[r] -> population[r]`
    /// (with N=3) classifies as one fast-path PureSameElementA2A
    /// cycle with `dimensions = [Region]`. The slow-path subgraph is
    /// empty -- no element-level Johnson runs.
    #[test]
    fn pure_a2a_model_emits_one_fast_path_cycle_no_slow_path() {
        let project = TestProject::new("pure_a2a")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.1", None);

        let result = tiered(&project);

        assert_eq!(
            result.fast_path.len(),
            1,
            "expected one fast-path circuit for pure-A2A loop, got {result:?}"
        );
        let fp = &result.fast_path[0];
        let var_set: BTreeSet<&str> = fp.variables.iter().map(|s| s.as_str()).collect();
        assert_eq!(var_set, ["births", "population"].iter().copied().collect());
        assert_eq!(
            fp.dimensions.len(),
            1,
            "PureSameElementA2A cycle must carry shared dimension list"
        );
        assert_eq!(fp.dimensions[0], "region");

        assert_eq!(
            result.slow_path.len(),
            0,
            "pure-A2A model must produce no slow-path circuits"
        );
    }

    /// Pure-scalar model: a closed feedback loop between scalar
    /// variables. Classifies as one fast-path PureScalar cycle. The
    /// slow-path subgraph is empty.
    #[test]
    fn pure_scalar_loop_emits_one_fast_path_cycle_no_slow_path() {
        let project = TestProject::new("scalar_loop")
            .stock("x", "100", &["inflow"], &[], None)
            .flow("inflow", "x * 0.1", None);

        let result = tiered(&project);

        assert_eq!(
            result.fast_path.len(),
            1,
            "expected one scalar fast-path cycle, got {result:?}"
        );
        assert!(
            result.fast_path[0].dimensions.is_empty(),
            "scalar cycle must carry empty dimensions"
        );

        assert_eq!(
            result.slow_path.len(),
            0,
            "scalar model must produce no slow-path circuits"
        );
    }

    /// Wildcard reducer in a feedback loop forces the cycle into the
    /// slow-path subgraph. Pure-A2A cycles in the same model still
    /// land in the fast path.
    #[test]
    fn wildcard_reducer_lands_in_slow_path_a2a_in_fast_path() {
        // `population -> share -> population` would close a cycle, but
        // in this minimal model `share` doesn't feed back. So the only
        // cycle is the population's stock self-loop via births. We need
        // a scenario where the wildcard reducer is part of a feedback
        // loop, which requires the reducer's output to influence the
        // source.
        //
        // Build: population[r] (stock with births[r] inflow) +
        // births[r] = population * SUM(population[*]) / 100
        // (births depends on both population[r] bare and SUM(population[*])).
        // The cycle is population -> births -> population, but the
        // population->births edge has both Bare and Wildcard shapes,
        // so the cycle classifier returns CrossElementOrMixed.
        let project = TestProject::new("mixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow(
                "births[Region]",
                "population * SUM(population[*]) * 0.0001",
                None,
            );

        let result = tiered(&project);

        // No fast-path cycle for this model -- the only structural
        // cycle (population -> births -> population) classifies as
        // cross-element/mixed.
        assert_eq!(
            result.fast_path.len(),
            0,
            "wildcard-mixed model must produce no fast-path cycles, got {result:?}"
        );
        // Slow-path subgraph contains population, births element-level
        // nodes; Johnson on that subgraph must find at least one
        // element-level circuit.
        assert!(
            !result.slow_path.is_empty(),
            "wildcard-mixed model must produce slow-path circuits"
        );
    }

    /// Mixed model: a pure-A2A loop AND a cross-element loop coexist.
    /// The pure-A2A loop lands in the fast path; the cross-element
    /// loop variables drive the slow-path subgraph.
    #[test]
    fn mixed_model_partitions_correctly() {
        let project = TestProject::new("split")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.05", None)
            .scalar_aux("total", "SUM(population[*])");

        let result = tiered(&project);

        // The pure-A2A population<->births cycle is a fast-path entry.
        assert_eq!(
            result.fast_path.len(),
            1,
            "expected one fast-path entry, got {result:?}"
        );
        // total isn't part of any cycle (no variable references back to
        // population from total), so the slow-path is empty.
        assert_eq!(
            result.slow_path.len(),
            0,
            "no cross-element cycle exists; slow path must be empty"
        );
    }

    /// Structural-win demonstration: pure-A2A model with N elements.
    ///
    /// Today's `model_element_loop_circuits` enumerates one circuit
    /// per element (N circuits) even though `build_element_level_loops`
    /// collapses them all back into one A2A Loop. The tiered
    /// enumerator emits exactly one fast-path circuit and runs zero
    /// element-level Johnson on the slow-path subgraph -- O(1) work
    /// instead of O(N).
    ///
    /// This test pins the circuit-count inequality and the empty
    /// slow-path subgraph in a single fixture. The post-2026-04-25
    /// per-reference refactor already broke up the element graph into
    /// N independent SCCs of size 2 each (one per element); the new
    /// win is that we no longer pay for Johnson on each of those
    /// N SCCs.
    ///
    /// Calls the legacy `model_element_loop_circuits` (now
    /// `#[deprecated]` for new LTM callers) to compare the legacy
    /// circuit count against the tiered enumerator's fast-path output;
    /// that's the load-bearing comparison this test exists to pin.
    #[allow(deprecated)]
    #[test]
    fn pure_a2a_eliminates_per_element_circuit_redundancy() {
        const N: usize = 30;
        let elements: Vec<String> = (0..N).map(|i| format!("e{i}")).collect();
        let elem_refs: Vec<&str> = elements.iter().map(String::as_str).collect();
        let project = TestProject::new("dense_a2a_circuits")
            .named_dimension("Region", &elem_refs)
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.1", None);

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;

        // Legacy element-level Johnson: one circuit per element
        // because the per-reference refactor produces N independent
        // SCCs of size 2.
        let legacy = model_element_loop_circuits(&db, source_model, source_project);
        assert_eq!(
            legacy.len(),
            N,
            "legacy element-level Johnson must enumerate {N} circuits for N-element pure-A2A model"
        );

        // Tiered enumerator: one fast-path cycle, zero slow-path
        // circuits, zero slow-path SCC. The element-level Johnson is
        // skipped entirely.
        let tiered = model_loop_circuits_tiered(&db, source_model, source_project);
        assert_eq!(
            tiered.fast_path.len(),
            1,
            "tiered enumerator must emit exactly one fast-path A2A cycle"
        );
        assert_eq!(
            tiered.slow_path.len(),
            0,
            "tiered enumerator must run zero element-level Johnson on pure-A2A model"
        );
        assert_eq!(
            tiered.slow_path_largest_scc, 0,
            "slow-path subgraph SCC must be 0 (no cross-element / mixed cycles)"
        );

        // Variable-level SCC: 2 (population, births). The new gate
        // keys on this value, well under the 50-node threshold.
        let var_edges = model_causal_edges(&db, source_model, source_project);
        let var_scc = causal_graph_from_edges(var_edges).largest_scc_size();
        assert_eq!(
            var_scc, 2,
            "variable-level SCC must be 2 (population, births), got {var_scc}"
        );
    }
}

#[cfg(test)]
mod classify_cycle_tests {
    //! Pure-function tests for `classify_cycle`. The classifier reads
    //! a per-edge shape map and a per-variable dim lookup; we build
    //! both inputs directly without going through the salsa pipeline
    //! so the tests are fast and self-contained.
    use super::*;
    use crate::common::{CanonicalDimensionName, CanonicalElementName};
    use crate::dimensions::{Dimension, NamedDimension};
    use std::collections::HashMap as StdHashMap;

    /// Helper: build a single-dim Named dimension whose elements are
    /// `["a", "b"]`. The `name` is the canonical dimension name.
    fn make_dim(name: &str) -> Dimension {
        let elements = vec![
            CanonicalElementName::from_raw("a"),
            CanonicalElementName::from_raw("b"),
        ];
        let indexed: StdHashMap<CanonicalElementName, usize> = elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), i + 1))
            .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements,
                indexed_elements: indexed,
                maps_to: None,
                mappings: vec![],
            },
        )
    }

    /// Helper: closure that maps every name in `arrayed` to `dims`,
    /// every other name to scalar (empty Vec).
    fn dim_lookup<'a>(
        arrayed: &'a [&'a str],
        dims: &'a [Dimension],
    ) -> impl Fn(&str) -> Vec<Dimension> + 'a {
        move |name| {
            if arrayed.contains(&name) {
                dims.to_vec()
            } else {
                Vec::new()
            }
        }
    }

    fn shapes_with(edges: &[(&str, &str, &[RefShape])]) -> EdgeShapesResult {
        let mut edge_shapes: HashMap<(String, String), BTreeSet<RefShape>> = HashMap::new();
        for (from, to, shapes) in edges {
            let set: BTreeSet<RefShape> = shapes.iter().cloned().collect();
            edge_shapes.insert((from.to_string(), to.to_string()), set);
        }
        EdgeShapesResult { edge_shapes }
    }

    #[test]
    fn pure_scalar_two_node_cycle() {
        let cycle = vec!["a".to_string(), "b".to_string()];
        let edges = shapes_with(&[("a", "b", &[RefShape::Bare]), ("b", "a", &[RefShape::Bare])]);
        let lookup = dim_lookup(&[], &[]);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::PureScalar
        );
    }

    #[test]
    fn pure_a2a_two_node_cycle_emits_dims() {
        let dim = make_dim("region");
        let cycle = vec!["pop".to_string(), "births".to_string()];
        let edges = shapes_with(&[
            ("pop", "births", &[RefShape::Bare]),
            ("births", "pop", &[RefShape::Bare]),
        ]);
        let dims = vec![dim.clone()];
        let lookup = dim_lookup(&["pop", "births"], &dims);
        match classify_cycle(&cycle, &edges, &lookup) {
            CycleClass::PureSameElementA2A { dimensions } => {
                assert_eq!(dimensions, vec!["region".to_string()]);
            }
            other => panic!("expected PureSameElementA2A, got {other:?}"),
        }
    }

    #[test]
    fn wildcard_edge_makes_cycle_cross_element() {
        let dim = make_dim("region");
        let cycle = vec!["pop".to_string(), "share".to_string()];
        // share -> pop is Bare; pop -> share is {Bare, Wildcard}. The
        // Wildcard alone forces CrossElementOrMixed regardless of any
        // co-existing Bare on the same edge.
        let edges = shapes_with(&[
            ("pop", "share", &[RefShape::Bare, RefShape::Wildcard]),
            ("share", "pop", &[RefShape::Bare]),
        ]);
        let dims = vec![dim];
        let lookup = dim_lookup(&["pop", "share"], &dims);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::CrossElementOrMixed
        );
    }

    #[test]
    fn fixed_index_edge_makes_cycle_cross_element() {
        let dim = make_dim("region");
        let cycle = vec!["pop".to_string(), "mig".to_string()];
        // mig -> pop is Bare; pop -> mig is FixedIndex(["nyc"]).
        let edges = shapes_with(&[
            (
                "pop",
                "mig",
                &[RefShape::FixedIndex(vec!["nyc".to_string()])],
            ),
            ("mig", "pop", &[RefShape::Bare]),
        ]);
        let dims = vec![dim];
        let lookup = dim_lookup(&["pop", "mig"], &dims);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::CrossElementOrMixed
        );
    }

    #[test]
    fn dynamic_index_edge_makes_cycle_cross_element() {
        let dim = make_dim("region");
        let cycle = vec!["pop".to_string(), "shifted".to_string()];
        let edges = shapes_with(&[
            ("pop", "shifted", &[RefShape::DynamicIndex]),
            ("shifted", "pop", &[RefShape::Bare]),
        ]);
        let dims = vec![dim];
        let lookup = dim_lookup(&["pop", "shifted"], &dims);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::CrossElementOrMixed
        );
    }

    #[test]
    fn mixed_scalar_and_arrayed_with_bare_only_is_cross_element() {
        // A cycle that mixes a scalar node with an arrayed node, even
        // when every edge is Bare, is not a single A2A loop: the
        // arrayed-to-scalar edge is a reduction, the scalar-to-arrayed
        // edge is a broadcast, and the cycle requires element-level
        // enumeration to enumerate the truthful shape.
        let dim = make_dim("region");
        let cycle = vec!["pop".to_string(), "scalar_state".to_string()];
        let edges = shapes_with(&[
            ("pop", "scalar_state", &[RefShape::Bare]),
            ("scalar_state", "pop", &[RefShape::Bare]),
        ]);
        let dims = vec![dim];
        let lookup = dim_lookup(&["pop"], &dims);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::CrossElementOrMixed
        );
    }

    #[test]
    fn arrayed_with_different_dims_is_cross_element() {
        // Two arrayed variables over different dimensions: the cycle
        // can't be A2A because there's no single shared dimension list
        // to expand over.
        let region = make_dim("region");
        let category = make_dim("category");
        let cycle = vec!["a".to_string(), "b".to_string()];
        let edges = shapes_with(&[("a", "b", &[RefShape::Bare]), ("b", "a", &[RefShape::Bare])]);
        // a -> region; b -> category.
        let lookup = move |name: &str| -> Vec<Dimension> {
            match name {
                "a" => vec![region.clone()],
                "b" => vec![category.clone()],
                _ => Vec::new(),
            }
        };
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::CrossElementOrMixed
        );
    }

    #[test]
    fn missing_edge_in_shape_map_treated_as_bare() {
        // Defensive: if an edge is somehow absent from the shape map,
        // the classifier defaults to treating it as Bare (matches the
        // model_edge_shapes fallback for unable-to-reconstruct edges).
        // The cycle should still classify as PureScalar / PureA2A
        // depending on the variable dims.
        let cycle = vec!["a".to_string(), "b".to_string()];
        let edges = shapes_with(&[("a", "b", &[RefShape::Bare])]);
        // b -> a edge missing from the shape map.
        let lookup = dim_lookup(&[], &[]);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::PureScalar
        );
    }

    #[test]
    fn empty_cycle_is_pure_scalar() {
        let cycle: Vec<String> = vec![];
        let edges = shapes_with(&[]);
        let lookup = dim_lookup(&[], &[]);
        assert_eq!(
            classify_cycle(&cycle, &edges, &lookup),
            CycleClass::PureScalar
        );
    }
}

#[cfg(test)]
mod edge_shapes_tests {
    //! Tests for `model_edge_shapes`: per-edge `RefShape` classification
    //! used as input to tiered loop enumeration. Verifies that the
    //! salsa-tracked function produces a deterministic
    //! `BTreeSet<RefShape>` per `(from, to)` variable-level edge.
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    fn edge_shapes(project: &TestProject) -> EdgeShapesResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        model_edge_shapes(&db, source_model, source_project).clone()
    }

    /// Helper: assert that the edge `(from, to)` has exactly the given
    /// shape set in the result.
    fn assert_shapes(result: &EdgeShapesResult, from: &str, to: &str, expected: &[RefShape]) {
        let key = (from.to_string(), to.to_string());
        let actual = result
            .edge_shapes
            .get(&key)
            .unwrap_or_else(|| panic!("missing edge {from} -> {to}"));
        let expected_set: BTreeSet<RefShape> = expected.iter().cloned().collect();
        assert_eq!(
            actual, &expected_set,
            "edge {from} -> {to}: expected {expected_set:?}, got {actual:?}"
        );
    }

    /// Pure-A2A model: `births[r] = population * 0.1` produces a single
    /// Bare reference. The structural flow->stock edge `births -> population`
    /// also classifies as Bare.
    #[test]
    fn pure_a2a_edges_are_all_bare() {
        let project = TestProject::new("pure_a2a")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.1", None);

        let result = edge_shapes(&project);
        assert_shapes(&result, "population", "births", &[RefShape::Bare]);
        assert_shapes(&result, "births", "population", &[RefShape::Bare]);
    }

    /// Wildcard reducer in target produces `{Wildcard}` on the edge.
    #[test]
    fn wildcard_reducer_edge_is_wildcard() {
        let project = TestProject::new("wild")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total", "SUM(population[*])");

        let result = edge_shapes(&project);
        assert_shapes(&result, "population", "total", &[RefShape::Wildcard]);
    }

    /// Mixed Bare + Wildcard target: `share[r] = pop / SUM(pop[*])` gives
    /// the edge `{Bare, Wildcard}`. The cycle classifier reads exactly
    /// this set to decide that any cycle through this edge cannot be
    /// pure-A2A (both shapes have different broadcast semantics).
    #[test]
    fn mixed_bare_and_wildcard_target() {
        let project = TestProject::new("mixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("share[Region]", "population / SUM(population[*])");

        let result = edge_shapes(&project);
        assert_shapes(
            &result,
            "population",
            "share",
            &[RefShape::Bare, RefShape::Wildcard],
        );
    }

    /// Fixed-index reference: `mig[NYC] = pop[NYC]`-style targets pin
    /// the source to a literal element. The shape set carries a
    /// `FixedIndex` entry with the resolved canonical element.
    #[test]
    fn fixed_index_target_records_resolved_element() {
        let project = TestProject::new("fixed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .scalar_aux("nyc_pop", "population[NYC]");

        let result = edge_shapes(&project);
        assert_shapes(
            &result,
            "population",
            "nyc_pop",
            &[RefShape::FixedIndex(vec!["nyc".to_string()])],
        );
    }

    /// Multiple shape kinds on one edge: `denom[r] = pop[NYC] + SUM(pop[*])`
    /// yields `{FixedIndex([nyc]), Wildcard}` (no bare ref to `pop`).
    #[test]
    fn fixed_index_plus_wildcard_no_bare() {
        let project = TestProject::new("fxw")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("population[Region]", "100")
            .array_aux("denom[Region]", "population[NYC] + SUM(population[*])");

        let result = edge_shapes(&project);
        assert_shapes(
            &result,
            "population",
            "denom",
            &[
                RefShape::FixedIndex(vec!["nyc".to_string()]),
                RefShape::Wildcard,
            ],
        );
    }

    /// Pure-scalar model: every edge is `{Bare}`. No arrayed source ->
    /// every reference parses as `Expr2::Var(...)`.
    #[test]
    fn pure_scalar_edges_are_bare() {
        let project = TestProject::new("scalar")
            .stock("x", "10", &["inflow"], &[], None)
            .flow("inflow", "rate", None)
            .scalar_const("rate", 0.5);

        let result = edge_shapes(&project);
        assert_shapes(&result, "rate", "inflow", &[RefShape::Bare]);
        assert_shapes(&result, "inflow", "x", &[RefShape::Bare]);
    }

    /// Edge keys come from `model_causal_edges`. Verify every variable
    /// edge has a shape entry (no edge gets dropped) and no extra
    /// entries are produced.
    #[test]
    fn edge_keys_match_variable_edges() {
        let project = TestProject::new("coverage")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_stock("population[Region]", "100", &["births"], &[], None)
            .array_flow("births[Region]", "population * 0.1", None)
            .scalar_aux("total", "SUM(population[*])");

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;

        let var_edges = model_causal_edges(&db, source_model, source_project);
        let shape_result = model_edge_shapes(&db, source_model, source_project);

        let mut expected_keys: BTreeSet<(String, String)> = BTreeSet::new();
        for (from, tos) in &var_edges.edges {
            for to in tos {
                expected_keys.insert((from.clone(), to.clone()));
            }
        }
        let actual_keys: BTreeSet<(String, String)> =
            shape_result.edge_shapes.keys().cloned().collect();
        assert_eq!(
            actual_keys, expected_keys,
            "model_edge_shapes keys must match model_causal_edges (from, to) pairs"
        );
    }
}
