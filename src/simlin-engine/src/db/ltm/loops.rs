// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Loop assembly for LTM: turning the tiered circuit-enumeration result
//! into `Loop` structs.
//!
//! This module owns model output-port discovery, the read-slice / cartesian
//! subscript helpers the link-score emitters share, the fast/slow-path loop
//! builders (`build_loops_from_tiered`, `build_element_level_loops`), and the
//! cross-element-through-aggregate loop recovery cluster
//! (`recover_cross_agg_loops`, `recover_agg_hop_polarities`).

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident};
use crate::datamodel;
use crate::ltm::strip_subscript;

use crate::db::{
    Db, LoopCircuitsResult, ModuleIdentContext, ModuleInputSet, SourceModel, SourceProject,
    SourceVariable, SourceVariableKind, TieredCircuitsResult, model_causal_edges,
    model_module_ident_context, parse_source_variable_with_module_context, variable_dimensions,
    variable_direct_dependencies,
};

/// Find the output ports for a model by scanning other models' variable
/// dependencies for module·var references that target this model.
///
/// When variable X depends on `module_var·internal_var` and `module_var`
/// maps to this model (via `dynamic_modules`), then `internal_var` is
/// an output port. The result is passed to `enumerate_pathways_to_outputs`
/// so that composite scores are generated for the correct output ports.
pub(super) fn find_model_output_ports(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<Ident<Canonical>> {
    let model_name = model.name(db);
    let project_models = project.models(db);
    let middot = '\u{00B7}';
    // The old no-arg `variable_direct_dependencies` used a literally-empty
    // module-ident context and the `None`-inputs path; reproduce that exactly.
    let empty_ctx = ModuleIdentContext::new(db, vec![]);
    let empty_inputs = ModuleInputSet::empty(db);
    let mut output_ports: HashSet<Ident<Canonical>> = HashSet::new();

    for (_, other_model) in project_models.iter() {
        if other_model == &model {
            continue;
        }
        let other_edges = model_causal_edges(db, *other_model, project);

        // Build a set of module variable names that reference this model
        let module_var_names: HashSet<&String> = other_edges
            .dynamic_modules
            .iter()
            .filter(|(_var_name, mn)| mn.as_str() == model_name.as_str())
            .map(|(var_name, _mn)| var_name)
            .collect();

        if module_var_names.is_empty() {
            continue;
        }

        // Scan dependencies for module·internal_var references
        let other_vars = other_model.variables(db);
        let module_ctx = model_module_ident_context(db, *other_model, project, vec![]);
        for (_, source_var) in other_vars.iter() {
            let deps =
                variable_direct_dependencies(db, *source_var, project, empty_ctx, empty_inputs);
            for dep in &deps.dt_deps {
                if let Some(dot_pos) = dep.find(middot) {
                    let module_part = &dep[..dot_pos];
                    let internal_var = &dep[dot_pos + middot.len_utf8()..];
                    if module_var_names.contains(&module_part.to_string()) {
                        output_ports.insert(Ident::new(internal_var));
                    }
                }
            }

            // Also check implicit variable deps (SMOOTH/DELAY expansion
            // creates helper auxes whose deps may reference module outputs)
            let parsed =
                parse_source_variable_with_module_context(db, *source_var, project, module_ctx);
            for implicit_dm_var in &parsed.implicit_vars {
                if let datamodel::Variable::Module(_) = implicit_dm_var {
                    continue;
                }
                let deps =
                    variable_direct_dependencies(db, *source_var, project, empty_ctx, empty_inputs);
                for iv_dep in &deps.implicit_vars {
                    for dep in &iv_dep.dt_deps {
                        if let Some(dot_pos) = dep.find(middot) {
                            let module_part = &dep[..dot_pos];
                            let internal_var = &dep[dot_pos + middot.len_utf8()..];
                            if module_var_names.contains(&module_part.to_string()) {
                                output_ports.insert(Ident::new(internal_var));
                            }
                        }
                    }
                }
            }
        }
    }

    output_ports.into_iter().collect()
}

/// Whether the edge `from -> to` is a *partial reduce*: `from` is arrayed,
/// `to` is arrayed with strictly fewer dimensions, and every `to` dimension
/// is one of `from`'s (matched by name). That is exactly the shape
/// `try_cross_dimensional_link_scores` emits per-`(reduced-elem,
/// result-elem)` scalar link scores for (`matrix[D1,D2] -> agg[D1]`). Same
/// dimensions, broadcast, mismatched dims, and module-involved edges are
/// not partial reduces. (Whether `to`'s equation actually applies a
/// reducing builtin to `from` is not checked here -- the loop-link builder
/// only needs to know to keep both element subscripts; the equation-text
/// path classifies the reducer.)
///
/// This shape-only check is *not* superseded by the aggregate-node reroute:
/// that reroute (`enumerate_agg_nodes` + `model_element_causal_edges`) only
/// covers *scalar synthetic* aggs hoisted out of a larger expression. A
/// whole-RHS arrayed-result reducer (`agg[D1] = SUM(matrix[D1,*])`) is a
/// *variable-backed* agg whose edges still come from the normal reference
/// walker, so the loop-link builder still needs this predicate to know to
/// keep both element subscripts on the `matrix[d1,d2] -> agg[d1]` link.
fn is_partial_reduce_edge(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    project: SourceProject,
) -> bool {
    let from_sv = match source_vars.get(from) {
        Some(sv) if sv.kind(db) != SourceVariableKind::Module => sv,
        _ => return false,
    };
    let to_sv = match source_vars.get(to) {
        Some(sv) if sv.kind(db) != SourceVariableKind::Module => sv,
        _ => return false,
    };
    let from_dims = variable_dimensions(db, *from_sv, project);
    let to_dims = variable_dimensions(db, *to_sv, project);
    if from_dims.is_empty() || to_dims.is_empty() || to_dims.len() >= from_dims.len() {
        return false;
    }
    let from_names: Vec<&str> = from_dims.iter().map(|d| d.name()).collect();
    to_dims.iter().all(|td| from_names.contains(&td.name()))
}

/// Compute the cartesian product of element name lists as comma-joined
/// subscript strings.
///
/// For a single dimension `[["nyc", "boston"]]`, returns `["nyc", "boston"]`.
/// For two dimensions `[["nyc", "boston"], ["adult", "child"]]`, returns
/// `["nyc,adult", "nyc,child", "boston,adult", "boston,child"]`.
pub(super) fn cartesian_subscripts(dim_element_lists: &[Vec<String>]) -> Vec<String> {
    if dim_element_lists.is_empty() {
        return vec![];
    }
    let mut result: Vec<String> = dim_element_lists[0].clone();
    for dim_elements in &dim_element_lists[1..] {
        let mut expanded = Vec::with_capacity(result.len() * dim_elements.len());
        for existing in &result {
            for elem in dim_elements {
                expanded.push(format!("{existing},{elem}"));
            }
        }
        result = expanded;
    }
    result
}

/// One source row a hoisted reducer reads, paired with the agg result slot it
/// feeds and the co-reduced rows (the rows mapping to the *same* slot -- the
/// `from`-slice the reducer combines for that slot, used as the
/// `all_elements` argument for the per-element link-score equation builders).
pub(super) struct ReadSliceRow {
    /// The source element subscript (comma-joined element names).
    pub(super) row: String,
    /// The agg result slot's subscript (the `Iterated` axes' elements,
    /// comma-joined; empty when the agg is scalar).
    pub(super) slot: String,
    /// All source rows mapping to `slot`, in row-major order.
    pub(super) coreduced: Vec<String>,
}

/// Enumerate the source rows a hoisted reducer reads, given the agg's
/// `read_slice` (one [`crate::ltm_agg::AxisRead`] per `from`'s axis -- which
/// holds because `from` is one of `agg`'s `source_vars`) and `from`'s
/// dimension element lists. A `Pinned` axis is fixed to its single element; an
/// `Iterated` or `Reduced` axis ranges over every element of that axis. The
/// agg result slot for a row is its `Iterated` coordinates in order.
///
/// `None` when `read_slice` doesn't have one entry per `from` axis (it always
/// should for a hoisted agg whose `source_vars` contains `from`): the caller
/// then falls back to the conservative "every source element, scalar agg" form.
pub(super) fn read_slice_rows(
    read_slice: &[crate::ltm_agg::AxisRead],
    from_dim_element_lists: &[Vec<String>],
) -> Option<Vec<ReadSliceRow>> {
    use crate::ltm_agg::AxisRead;
    if read_slice.len() != from_dim_element_lists.len() {
        return None;
    }
    // Per axis: the element list to iterate, plus whether the axis contributes
    // a coordinate to the result slot.
    let per_axis: Vec<(Vec<String>, bool)> = read_slice
        .iter()
        .zip(from_dim_element_lists)
        .map(|(a, elems)| match a {
            AxisRead::Pinned(e) => (vec![e.clone()], false),
            AxisRead::Iterated(_) => (elems.clone(), true),
            AxisRead::Reduced => (elems.clone(), false),
        })
        .collect();
    // Cartesian product, tracking each row's full element tuple and its slot
    // coordinates.
    let mut rows: Vec<(Vec<String>, Vec<String>)> = vec![(Vec::new(), Vec::new())];
    for (elems, contributes_to_slot) in &per_axis {
        let mut next: Vec<(Vec<String>, Vec<String>)> =
            Vec::with_capacity(rows.len() * elems.len());
        for (row, slot) in &rows {
            for e in elems {
                let mut new_row = row.clone();
                new_row.push(e.clone());
                let mut new_slot = slot.clone();
                if *contributes_to_slot {
                    new_slot.push(e.clone());
                }
                next.push((new_row, new_slot));
            }
        }
        rows = next;
    }
    // Group rows by slot to build each row's `coreduced` set.
    let mut by_slot: HashMap<String, Vec<String>> = HashMap::new();
    for (row, slot) in &rows {
        by_slot
            .entry(slot.join(","))
            .or_default()
            .push(row.join(","));
    }
    Some(
        rows.into_iter()
            .map(|(row, slot)| {
                let slot = slot.join(",");
                let row = row.join(",");
                let coreduced = by_slot[&slot].clone();
                ReadSliceRow {
                    row,
                    slot,
                    coreduced,
                }
            })
            .collect(),
    )
}

/// Build the element-level `Loop::stocks` list for a cycle.
///
/// For a scalar cycle (`dimensions` empty), the stocks are the
/// variable-level stock idents unchanged -- a genuinely scalar variable's
/// name *is* its (degenerate, subscript-free) element-level node name.
///
/// For an A2A cycle (`dimensions` non-empty) the stocks cover the loop's
/// *entire* dimension element space: one `"{var}[{elem-tuple}]"` per element
/// of the dimension space (in the runtime's row-major slot order, via
/// [`crate::ltm::loop_dimension_element_tuples`]), for each variable in
/// `var_stocks`.  This is the granularity the `Loop` docstring's invariant
/// requires so `CyclePartitions::partition_for_loop` can resolve a partition
/// *per slot* against `model_element_cycle_partitions`'s element-keyed
/// `stock_partition` map.  If `dm_dims` doesn't cover the cycle's declared
/// dimensions (a mid-edit inconsistency), the variable-level stocks are
/// returned unchanged -- `partition_for_loop` then falls back to whatever
/// suffixes are present (none, here), bucketing the loop into the `None`
/// group, the same degradation the pre-element-level code exhibited.
fn build_a2a_loop_stocks(
    var_stocks: &[Ident<Canonical>],
    dimensions: &[String],
    dm_dims: &[crate::datamodel::Dimension],
) -> Vec<Ident<Canonical>> {
    if dimensions.is_empty() {
        return var_stocks.to_vec();
    }
    let tuples = crate::ltm::loop_dimension_element_tuples(dimensions, dm_dims);
    if tuples.is_empty() {
        return var_stocks.to_vec();
    }
    let mut stocks = Vec::with_capacity(tuples.len() * var_stocks.len());
    for tuple in &tuples {
        for s in var_stocks {
            stocks.push(Ident::new(&format!("{}[{}]", s.as_str(), tuple)));
        }
    }
    stocks
}

/// Build `Loop` structs from the tiered loop-enumeration result.
///
/// The fast path (`tiered.fast_path`) carries variable-level cycles
/// already classified as PureScalar or PureSameElementA2A; each one
/// emits a single `Loop` directly. The slow path
/// (`tiered.slow_path`) carries element-level circuits over the
/// cross-element subgraph; those flow through the same per-circuit
/// grouping logic the legacy `build_element_level_loops` uses.
///
/// The merged Loop list is passed to `assign_loop_ids` once so loop
/// IDs (`r1, b1, ...`) are assigned over the unified set, matching
/// the legacy ordering: `assign_loop_ids` sorts by content-derived
/// key (sorted distinct var names) before numbering, so the final IDs
/// are stable regardless of which path produced each Loop.
///
/// `agg_loop_budget` and the returned `Vec<String>` thread the cross-element-
/// through-aggregate loop-count budget / truncated-aggregate-node names
/// through to `build_element_level_loops` -> `recover_cross_agg_loops` (only
/// the slow path can carry agg nodes); the caller surfaces the flag (and the
/// names) on `LtmVariablesResult::agg_recovery_truncated` and the truncation
/// `Warning`. The returned vector is empty iff nothing was clipped.
pub(crate) fn build_loops_from_tiered(
    tiered: &TieredCircuitsResult,
    var_graph: &crate::ltm::CausalGraph,
    source_vars: &HashMap<String, SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    agg_loop_budget: usize,
) -> (Vec<crate::ltm::Loop>, Vec<String>) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{Loop, assign_loop_ids};

    let mut all_loops: Vec<Loop> = Vec::new();

    // Fast-path: each cycle materializes directly into one Loop. The
    // FastPathCircuit's `dimensions` field carries the shared
    // arrayed dimensions (empty for PureScalar). The links / stocks /
    // polarity are derived from the variable-level cycle exactly as
    // the legacy pure-dimension branch did.
    for fp in &tiered.fast_path {
        if fp.variables.is_empty() {
            continue;
        }
        let var_level_nodes: Vec<Ident<Canonical>> = fp
            .variables
            .iter()
            .map(|s| Ident::new(s.as_str()))
            .collect();
        let links = var_graph.circuit_to_links(&var_level_nodes);
        let var_stocks = var_graph.find_stocks_in_loop(&var_level_nodes);
        let polarity = var_graph.calculate_polarity(&links);

        // Map the canonical dimension names to original datamodel
        // names so equation parsing on the loop-score variable
        // resolves the dimension by string match. Mirrors the legacy
        // pure-dimension branch's mapping logic.
        //
        // Fallback: if a canonical name in `fp.dimensions` is missing
        // from `dm_dims`, fall back to the canonical name itself.
        // This matches `build_element_level_loops` (the slow-path
        // consumer below) so incremental or partially-invalid model
        // states -- where the tiered enumerator's cached dim closure
        // can outrun a still-being-edited datamodel dim list -- surface
        // as a downstream analysis warning rather than a hard panic
        // that takes down the whole LTM pipeline. We assert in debug
        // builds so the mismatch stays observable when the model
        // really is internally consistent.
        let dimensions: Vec<String> = fp
            .dimensions
            .iter()
            .map(|canonical| {
                let resolved = dm_dims
                    .iter()
                    .find(|dm| crate::common::canonicalize(dm.name()).as_ref() == canonical)
                    .map(|dm| dm.name().to_string());
                debug_assert!(
                    resolved.is_some(),
                    "fast-path A2A cycle references dimension {canonical:?} that is not in \
                     the project's datamodel dimensions {known:?}; falling back to canonical \
                     name. This usually means the source project's dim list and the parsed \
                     variable dims got out of sync mid-edit.",
                    known = dm_dims.iter().map(|d| d.name()).collect::<Vec<_>>(),
                );
                resolved.unwrap_or_else(|| canonical.to_string())
            })
            .collect();

        // For a PureSameElementA2A cycle the stocks are element-level over
        // the dimension space (per the `Loop` docstring's invariant); for a
        // PureScalar cycle (`dimensions` empty) they're the variable-level
        // names unchanged.
        let stocks = build_a2a_loop_stocks(&var_stocks, &dimensions, dm_dims);

        all_loops.push(Loop {
            id: String::new(),
            links,
            stocks,
            polarity,
            dimensions,
        });
    }

    // Slow-path: feed the element-level circuit list through the
    // existing per-circuit grouping logic. This emits cross-element
    // and mixed scalar loops the same way the legacy code did. The
    // helper does its own `assign_loop_ids`; we strip the IDs
    // afterward because we re-run id assignment over the merged set
    // to keep numbering consistent.
    let mut truncated_aggs: Vec<String> = Vec::new();
    if !tiered.slow_path.is_empty() {
        let (mut slow_path_loops, t) = build_element_level_loops(
            &tiered.slow_path,
            var_graph,
            source_vars,
            db,
            project,
            dm_dims,
            agg_loop_budget,
        );
        truncated_aggs = t;
        for l in &mut slow_path_loops {
            l.id.clear();
        }
        all_loops.extend(slow_path_loops);
    }

    assign_loop_ids(&mut all_loops);
    (all_loops, truncated_aggs)
}

/// Build element-subscripted `Link`s for one element-level circuit.
///
/// For circuit nodes `[n_0, ..., n_{k-1}]` (each `n_i` either `var` or
/// `var[e_i]`), each link `n_i -> n_{i+1}` keeps the element subscript on
/// the side(s) the loop-score equation needs to pin a per-element link
/// score:
///
///   - `from = n_i` (subscript kept) when `n_i` is subscripted. A
///     per-source-element FixedIndex / cross-dimensional link score is
///     named `{from}[{e_i}]->{to}` (the bracketed `from` form
///     `try_cross_dimensional_link_scores` and FixedIndex emission
///     produce); `generate_loop_score_equation`'s resolver also falls
///     back to the variable-level `from` form when the bracketed name
///     wasn't emitted (e.g. a structural flow->stock A2A link score is
///     `{strip(from)}->{to}`).
///   - `to = n_{i+1}` (subscript kept) when `n_{i+1}` is subscripted AND
///     its variable is dimensioned (so the link score is A2A and the loop
///     visits one element of it -- `generate_loop_score_equation` then
///     emits `"...→{to}"[e_{i+1}]`). A synthetic aggregate node
///     (`$⁚ltm⁚agg⁚{n}[<slot>]`) is treated like a dimensioned variable
///     here even though it isn't in `source_vars`: when the agg is arrayed
///     its slot rides in the link-score names `emit_source_to_agg_link_scores`
///     / `emit_agg_to_target_link_scores` emit (`{src}[<row>]→{agg}[<slot>]`,
///     `{agg}[<slot>]→{tgt}[<elem>]`), so `loop_link_score_ref` needs the
///     `[<slot>]` on the agg endpoint to resolve those names. Otherwise
///     `to = strip(n_{i+1})` (the link score is scalar / cross-dimensional,
///     referenced without a subscript).
///
/// `var_links` carries the variable-level links for the same circuit
/// (from `circuit_to_links` on the stripped node sequence); link `i`'s
/// polarity is taken from `var_links[i]` (the variable-level static
/// polarity for that hop), defaulting to `Unknown` if the lengths ever
/// disagree (they shouldn't).
fn build_element_subscripted_links(
    circuit: &[&str],
    var_links: &[crate::ltm::Link],
    source_vars: &HashMap<String, SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
) -> Vec<crate::ltm::Link> {
    let mut links = Vec::with_capacity(circuit.len());
    for i in 0..circuit.len() {
        let from_raw = circuit[i];
        let to_raw = circuit[(i + 1) % circuit.len()];
        let polarity = if i < var_links.len() {
            var_links[i].polarity
        } else {
            crate::ltm::LinkPolarity::Unknown
        };
        let link_from = if from_raw.contains('[') {
            from_raw
        } else {
            strip_subscript(from_raw)
        };
        let to_var_level = strip_subscript(to_raw);
        let to_is_arrayed = crate::ltm_agg::is_synthetic_agg_name(to_var_level)
            || source_vars
                .get(to_var_level)
                .map(|sv| {
                    sv.kind(db) != SourceVariableKind::Module
                        && !variable_dimensions(db, *sv, project).is_empty()
                })
                .unwrap_or(false);
        let link_to = if to_raw.contains('[') && to_is_arrayed {
            to_raw
        } else {
            to_var_level
        };
        links.push(crate::ltm::Link {
            from: Ident::new(link_from),
            to: Ident::new(link_to),
            polarity,
        });
    }
    links
}

/// Build `Loop` structs from element-level circuits, grouping
/// pure-dimension circuits into shared A2A loops, scoring cross-element
/// circuits along their element-level path, and keeping mixed circuits as
/// individual scalar loops.
///
/// Pure-dimension: all circuits in a group have the same variable-level
/// node sequence (e.g., `[population, births]` for both `[population[nyc],
/// births[nyc]]` and `[population[boston], births[boston]]`). These share
/// one loop ID and produce an A2A loop score with dimensions.
///
/// Cross-element: a circuit that visits different elements at different
/// points (a per-element-equation hop reading the *other* element, or a
/// wildcard reducer). Each circuit becomes its own scalar Loop whose
/// `Link`s carry element subscripts so the loop-score equation references
/// the per-element link scores along the element path.
///
/// Mixed: any circuit containing a scalar node or where the group has
/// circuits with different variable-level structures. Each gets its own
/// scalar loop with a unique element-specific ID suffix.
///
/// `agg_loop_budget` caps how many non-elementary cross-element-through-
/// aggregate loops `recover_cross_agg_loops` materializes (across all
/// aggregate nodes); the returned `Vec<String>` names the aggregate nodes
/// whose enumeration that budget (or the per-aggregate petal cap) clipped
/// (sorted, deduped -- empty iff nothing was clipped). Callers pass
/// `cross_agg_loop_budget()` (or, in tests, a small value) and thread the
/// names up to `LtmVariablesResult::agg_recovery_truncated` and the
/// truncation `Warning`.
///
/// Visibility is `pub(crate)` so unit tests in
/// `db/ltm_unified_tests.rs` can drive this function directly to
/// inspect the element-subscripted `Link.from` / `Link.to` strings the
/// loop builder produces (e.g. `"population[nyc]"`) -- there is no
/// separate per-link shape field, and these per-link strings aren't
/// observable through the `LtmVariablesResult.vars` surface (which only
/// exposes the rendered equation strings).
pub(crate) fn build_element_level_loops(
    element_circuits: &LoopCircuitsResult,
    var_graph: &crate::ltm::CausalGraph,
    source_vars: &HashMap<String, SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    agg_loop_budget: usize,
) -> (Vec<crate::ltm::Loop>, Vec<String>) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{Loop, assign_loop_ids};

    // Materialize each circuit as a small `Vec<&str>` once so downstream
    // grouping, name stripping, and node-wise comparisons don't pay the
    // indexed-lookup cost repeatedly.  The backing storage stays in
    // `element_circuits.names`, so these slices are all borrows into the
    // existing name table rather than per-call allocations.
    let circuit_strs: Vec<Vec<&str>> = (0..element_circuits.len())
        .map(|i| element_circuits.circuit_names(i).collect())
        .collect();

    // Group element-level circuits by their variable-level node sequence.
    // The key is the joined stripped names; the value collects indices
    // into `circuit_strs` that share that variable-level structure.
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (ci, circuit) in circuit_strs.iter().enumerate() {
        let var_level_key: String = circuit
            .iter()
            .map(|n| strip_subscript(n))
            .collect::<Vec<_>>()
            .join("\x00");
        groups.entry(var_level_key).or_default().push(ci);
    }

    // Sort groups deterministically by their key.
    let mut sorted_groups: Vec<(String, Vec<usize>)> = groups.into_iter().collect();
    sorted_groups.sort_by(|a, b| a.0.cmp(&b.0));

    let mut all_loops: Vec<Loop> = Vec::new();

    for (_group_key, group_indices) in &sorted_groups {
        let circuits_in_group: Vec<&[&str]> = group_indices
            .iter()
            .map(|&ci| circuit_strs[ci].as_slice())
            .collect();
        // Determine if this is a pure-dimension group.
        //
        // A group is pure-dimension when:
        // 1. Every node in every circuit has a subscript (no scalar nodes)
        // 2. The stripped variable-level sequence has NO repeated variables
        //    (repeated variables indicate cross-element circuits, e.g.,
        //    pop[nyc]->share[boston]->...->pop[boston]->share[nyc], where
        //    "population" appears twice in the stripped sequence)
        // 3. The group has more than one circuit (multiple elements share
        //    the same structure), OR has exactly one circuit with subscripted
        //    nodes (single-element dimension is still A2A)
        //
        // When a model has no arrayed variables, circuits won't have
        // subscripts and each group has exactly one circuit -- they are
        // scalar loops.
        let representative: &[&str] = circuits_in_group[0];
        let all_subscripted = representative.iter().all(|n| n.contains('['));

        // A circuit that traverses a synthetic aggregate node
        // (`$⁚ltm⁚agg⁚{n}`) must never be collapsed into a single A2A loop:
        // the per-row source link scores (`{src}[<row>]→{agg}[<slot>]`) and
        // the agg→target link scores (`{agg}[<slot>]→{tgt}[<elem>]`) are
        // scalar variables keyed by literal element names, so there is no
        // dimensioned `{src}→{agg}` link-score variable an A2A loop-score
        // equation could reference. Route these to the element-subscripted
        // per-circuit path (one scalar `Loop` per element combination), the
        // same path cross-element loops take -- `build_element_subscripted_links`
        // keeps the agg's `[<slot>]` so `loop_link_score_ref` can resolve the
        // agg-half names. (Even though the agg circuit's leading subscript
        // elements may all agree -- e.g. `[matrix[a,x], agg[a], growth[a,x]]`
        // -- which would otherwise classify it as a same-element A2A circuit.)
        let representative_has_synthetic_agg = representative
            .iter()
            .any(|n| crate::ltm_agg::is_synthetic_agg_name(strip_subscript(n)));

        // Detect cross-element circuits that should NOT be collapsed
        // into A2A loops. Two patterns indicate cross-element:
        //
        // 1. Repeated variable names: the stripped sequence has a variable
        //    appearing more than once (e.g., pop[nyc]->share[boston]->
        //    pop[boston]->share[nyc] has pop and share each twice).
        //
        // 2. Mixed subscripts: nodes in a circuit have different element
        //    subscripts at shared dimensions. A genuine A2A circuit visits
        //    each variable at the SAME element in shared dimensions
        //    (pop[nyc]->births[nyc]->pop[nyc], all nyc). Cross-element
        //    circuits visit different elements (pop[nyc]->mp[boston]->...).
        //
        //    Partial-collapse loops are NOT cross-element: source[a,x]->
        //    target[a] has subscripts "a,x" and "a" which differ in length
        //    but share the same element "a" on the shared dimension. We
        //    compare only the first element (leading shared dimension).
        let is_cross_element = if all_subscripted {
            // Check 1: repeated variable names
            let stripped: Vec<&str> = representative.iter().map(|n| strip_subscript(n)).collect();
            let mut seen = std::collections::HashSet::new();
            let has_repeated = stripped.iter().any(|v| !seen.insert(*v));
            if has_repeated {
                true
            } else {
                // Check 2: compare the leading (first) subscript element
                // across all nodes. Nodes with partial-collapse dimensions
                // have fewer subscript components (e.g., "a" vs "a,x"), but
                // the leading element is shared. If leading elements differ,
                // it's a genuine cross-element circuit.
                circuits_in_group.iter().any(|circuit| {
                    let leading_elements: Vec<&str> = circuit
                        .iter()
                        .filter_map(|n| {
                            let start = n.find('[')?;
                            let end = n.rfind(']')?;
                            let subscript = &n[start + 1..end];
                            // Take the first comma-separated component
                            Some(subscript.split(',').next().unwrap_or(subscript))
                        })
                        .collect();
                    // If leading elements differ, it's cross-element
                    leading_elements.windows(2).any(|w| w[0] != w[1])
                })
            }
        } else {
            false
        };

        if all_subscripted
            && !is_cross_element
            && !representative_has_synthetic_agg
            && !representative.is_empty()
        {
            // Pure-dimension group: produce a single A2A loop.
            //
            // Use the variable-level graph for polarity analysis and stock
            // detection (the element-level graph has empty variables).
            let var_level_nodes: Vec<Ident<Canonical>> = representative
                .iter()
                .map(|n| Ident::new(strip_subscript(n)))
                .collect();
            // A2A loop: every link is a same-element diagonal access. The
            // loop-score equation references the canonical
            // `{from}->{to}` link score (the Bare-shape name) via the
            // variable-level link names that `circuit_to_links` produces.
            let links = var_graph.circuit_to_links(&var_level_nodes);
            let var_stocks = var_graph.find_stocks_in_loop(&var_level_nodes);
            let polarity = var_graph.calculate_polarity(&links);

            // Determine the shared dimension(s) from the subscripts.
            // Look at the first subscripted node to find which dimensions
            // it carries, then map canonical dim names to original
            // datamodel names for equation parsing.
            let first_var_name = strip_subscript(representative[0]);
            let dimensions = source_vars
                .get(first_var_name)
                .map(|sv| {
                    variable_dimensions(db, *sv, project)
                        .iter()
                        .map(|d| {
                            let canonical = d.name();
                            dm_dims
                                .iter()
                                .find(|dm| {
                                    crate::common::canonicalize(dm.name()).as_ref() == canonical
                                })
                                .map(|dm| dm.name().to_string())
                                .unwrap_or_else(|| canonical.to_string())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // Stocks must be element-level over the dimension space so
            // `partition_for_loop` can resolve a partition per slot (the
            // `Loop` docstring's invariant -- the same rule the cross-element
            // and mixed branches below already follow).
            let stocks = build_a2a_loop_stocks(&var_stocks, &dimensions, dm_dims);

            all_loops.push(Loop {
                id: String::new(),
                links,
                stocks,
                polarity,
                dimensions,
            });
        } else if is_cross_element || representative_has_synthetic_agg {
            // Cross-element circuits: a circuit that genuinely visits
            // different elements at different points -- e.g.
            //   population[nyc] -> migration_pressure[boston] ->
            //   migration_in[nyc] -> population[nyc]
            // (a per-element-equation hop that reads the *other* element),
            // or the wildcard-reducer pattern
            //   pop[nyc] -> total[boston] -> update[boston] ->
            //   pop[boston] -> total[nyc] -> update[nyc] -> pop[nyc].
            //
            // Also: an elementary circuit through a synthetic aggregate node
            // visited once (`matrix[a,x] -> $⁚ltm⁚agg⁚0[a] -> growth[a,x] ->
            // matrix[a,x]` -- the "self-aggregate" feedback of a sliced
            // reducer) lands here even when its leading subscript elements all
            // agree; the loop score still has to reference the per-element
            // agg-half link scores, which only exist as literal-element scalar
            // variables.
            //
            // Each circuit becomes its own scalar Loop (the loop-score
            // *variable* is scalar: a cross-element loop visits fixed
            // elements, it is not parameterized by a free dimension) whose
            // `Link`s carry the element subscripts so the loop-score
            // equation references the per-element link scores along the
            // element-level path: `"$⁚ltm⁚link_score⁚{from}→{to}"[e]` for
            // an A2A (dimensioned) link score visited at element `e`. See
            // `ltm_augment::generate_loop_score_equation` for how the
            // subscript and the link-score-name resolution interact.
            //
            // (Pre-#503-fix this branch instead found the "shortest unique
            // cycle" in the *stripped* node sequence and emitted a single
            // scalar Loop referencing the *diagonal* A2A link scores via
            // `circuit_to_links` -- which scored a cross-element loop as if
            // its hops were same-element diagonal sensitivities. The
            // diagonal collapse and the unique-cycle stripping are gone.)
            for circuit in &circuits_in_group {
                let element_nodes: &[&str] = circuit;
                let var_level_nodes: Vec<Ident<Canonical>> = element_nodes
                    .iter()
                    .map(|n| Ident::new(strip_subscript(n)))
                    .collect();
                let var_links = var_graph.circuit_to_links(&var_level_nodes);

                let links = build_element_subscripted_links(
                    element_nodes,
                    &var_links,
                    source_vars,
                    db,
                    project,
                );

                // Stocks must be element-level so `partition_for_loop`
                // can resolve them in `model_element_cycle_partitions::
                // stock_partition` (which is keyed element-level). The
                // `Loop` docstring's stocks-granularity invariant says any
                // loop with `dimensions.is_empty()` MUST carry element-
                // level stock names. Collect every element-level stock node
                // in the circuit (a 6-node cross-element loop can traverse
                // the same stock variable at multiple elements).
                let stocks: Vec<Ident<Canonical>> = element_nodes
                    .iter()
                    .filter(|n| var_graph.stocks.contains(&Ident::new(strip_subscript(n))))
                    .map(|n| Ident::new(n))
                    .collect();

                let polarity = var_graph.calculate_polarity(&var_links);

                all_loops.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    dimensions: vec![], // scalar: cross-element loops visit fixed elements
                });
            }
        } else {
            // Mixed or scalar group: each circuit becomes its own scalar loop.
            for circuit in circuits_in_group {
                // For mixed loops, we can still attempt polarity via the
                // variable-level graph. Strip subscripts and analyze.
                let var_level_nodes: Vec<Ident<Canonical>> = circuit
                    .iter()
                    .map(|n| Ident::new(strip_subscript(n)))
                    .collect();
                let var_links = var_graph.circuit_to_links(&var_level_nodes);
                let polarity = var_graph.calculate_polarity(&var_links);

                // Build links with names that match what the link-score
                // emission system produces, so loop-score equations
                // reference existing variables.
                //
                // Link-score emission generates names in three forms:
                //
                //   1. Cross-dimensional (arrayed-from, scalar-to):
                //      `try_cross_dimensional_link_scores` emits
                //      "$⁚ltm⁚link_score⁚{from}[{elem}]→{to}" per source
                //      element. Element-level circuit nodes encode this as
                //      "{from}[{elem}]" for the source and "{to}" for the
                //      (scalar) target. We keep the bracketed `from` and
                //      bare `to` so `resolve_link_score_name_for_loop`
                //      matches that name.
                //
                //   2. Scalar-source -> arrayed-target:
                //      `try_scalar_to_arrayed_link_scores` emits
                //      "$⁚ltm⁚link_score⁚{from}→{to}[{elem}]" per target
                //      element. Element-level circuit nodes encode this as
                //      "{from}" for the (scalar) source and "{to}[{elem}]"
                //      for the target -- so we KEEP the `to` subscript
                //      whenever the target is arrayed (Phase 2's "keep
                //      `to[e]` when the link score is dimensioned" rule,
                //      extended to the per-target-element scalar var case).
                //      `generate_loop_score_equation` then references the
                //      per-element scalar variable directly.
                //
                //   3. Same-element A2A: `emit_per_shape_link_scores`
                //      emits "$⁚ltm⁚link_score⁚{from}→{to}" with
                //      `dimensions = [target_dims]`. We keep `to[e]` (case
                //      2's rule, since the target is arrayed) and strip the
                //      `from` subscript; `generate_loop_score_equation`
                //      then subscripts the dimensioned link score at the
                //      visited element. Scalar->scalar edges keep neither.
                let element_nodes: Vec<&str> = circuit.iter().map(|n| n.as_ref()).collect();

                let mut links = Vec::with_capacity(element_nodes.len());
                for (i, _) in element_nodes.iter().enumerate() {
                    let from_raw = element_nodes[i];
                    let to_raw = element_nodes[(i + 1) % element_nodes.len()];
                    // Use the polarity from the corresponding var-level link
                    let var_link_polarity = if i < var_links.len() {
                        var_links[i].polarity
                    } else {
                        crate::ltm::LinkPolarity::Unknown
                    };
                    let from_subscripted = from_raw.contains('[');
                    let to_subscripted = to_raw.contains('[');
                    let from_var_level = strip_subscript(from_raw);
                    let to_var_level = strip_subscript(to_raw);
                    let to_is_arrayed = source_vars
                        .get(to_var_level)
                        .map(|sv| {
                            sv.kind(db) != SourceVariableKind::Module
                                && !variable_dimensions(db, *sv, project).is_empty()
                        })
                        .unwrap_or(false);
                    let (link_from, link_to) = if from_subscripted && !to_subscripted {
                        // Cross-dimensional (full reduce, arrayed-from /
                        // scalar-to): keep element-level from, bare to.
                        (from_raw, to_raw)
                    } else if from_subscripted
                        && to_subscripted
                        && is_partial_reduce_edge(
                            db,
                            source_vars,
                            from_var_level,
                            to_var_level,
                            project,
                        )
                    {
                        // Partial reduce (`matrix[d1,d2] → row_sum[d1]`): the
                        // link score is the per-`(reduced-elem, result-elem)`
                        // scalar var `$⁚ltm⁚link_score⁚{from}[d1,d2]→{to}[d1]`
                        // from `try_cross_dimensional_link_scores`, so keep
                        // BOTH subscripts -- the source carries the collapsed
                        // axis too.
                        (from_raw, to_raw)
                    } else if to_subscripted && to_is_arrayed {
                        // Scalar->arrayed or same-element A2A: keep `to[e]`,
                        // strip any `from` subscript.
                        (strip_subscript(from_raw), to_raw)
                    } else {
                        // Scalar->scalar (or an arrayed target reduced to a
                        // scalar node that lost its subscript): variable-level.
                        (strip_subscript(from_raw), to_var_level)
                    };
                    // `Link.from` keeps any bracket it carries; the
                    // downstream resolver maps a bracketed `from` to a
                    // FixedIndex/cross-dimensional name and an unbracketed
                    // one to the Bare / per-target-element form.
                    links.push(crate::ltm::Link {
                        from: Ident::new(link_from),
                        to: Ident::new(link_to),
                        polarity: var_link_polarity,
                    });
                }

                // Find stocks among element-level nodes. We check the
                // variable-level stock set by stripping subscripts from
                // the circuit's element-level names, but preserve the
                // element-level form in the result so partition_for_loop
                // (which uses element-level keys from
                // model_element_cycle_partitions) can resolve them.
                let stocks: Vec<Ident<Canonical>> = element_nodes
                    .iter()
                    .filter(|n| {
                        let var_name = strip_subscript(n);
                        var_graph.stocks.contains(&Ident::new(var_name))
                    })
                    .map(|n| Ident::new(n))
                    .collect();

                // Each arrayed circuit node that is a stock must appear in
                // `stocks` with its subscript intact. Stripping the subscript
                // here would break partition_for_loop: model_element_cycle_
                // partitions keys stock_partition on element-level names
                // (e.g. "pop[nyc]"), not variable-level names (e.g. "pop").
                debug_assert!(
                    element_nodes
                        .iter()
                        .filter(|n| n.contains('[') && {
                            let var_name = strip_subscript(n);
                            var_graph.stocks.contains(&Ident::new(var_name))
                        })
                        .all(|n| stocks.iter().any(|s| s.as_str() == *n)),
                    "mixed/scalar branch: arrayed stock node lost its subscript; \
                     element_nodes={element_nodes:?} stocks={stocks:?}"
                );

                all_loops.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    dimensions: vec![],
                });
            }
        }
    }

    // Recover cross-element loops that traverse a synthetic aggregate node
    // more than once (Phase 5, GH #515). Johnson enumerates only *elementary*
    // circuits, so a loop like `pop[nyc] → agg → share[boston] → ... →
    // pop[boston] → agg → share[nyc] → ...` -- which visits `agg` twice -- is
    // never emitted. But each agg-touching elementary circuit contributes one
    // "petal" `agg → ... → agg`; stitching `k ≥ 2` petals of the same agg
    // whose internal nodes are pairwise disjoint, in some cyclic order,
    // reconstructs exactly those non-elementary loops -- bounded by
    // `agg_loop_budget` (when it clips, the clipped aggs' names are returned
    // and the caller accumulates a `Warning` naming them).
    let (recovered, truncated_aggs) = recover_cross_agg_loops(
        &circuit_strs,
        var_graph,
        source_vars,
        db,
        project,
        agg_loop_budget,
    );
    all_loops.extend(recovered);

    assign_loop_ids(&mut all_loops);
    (all_loops, truncated_aggs)
}

/// Hard cap on the number of non-elementary cross-element-through-aggregate
/// loops `recover_cross_agg_loops` materializes for a model (summed across
/// all synthetic aggregate nodes), Phase 5 / GH #515.
///
/// With `k` disjoint petals through one agg the recoverable loop count is
/// `Σ_{m=2}^{k} C(k,m)·orderings(m)` where `orderings(m)` is 1 for m=2 and
/// `(m-1)!/2` for m≥3 -- it grows super-exponentially, so a budget is
/// mandatory. When recovery hits this budget it stops, sets the truncation
/// flag, and the caller emits a `Warning`; the deterministic petal priority
/// (fewest internal nodes first, then a stable joined-name tiebreaker)
/// makes *which* loops survive truncation reproducible. The prior implicit
/// ceiling was `2^8 - 8 - 1 = 247` per agg (the old `MAX_AGG_PETALS = 8`
/// hard drop); 256 keeps roughly that order of magnitude as a model-wide
/// total. Because the budget is modest, recovery never reaches a subset
/// large enough for `cyclic_orderings(m)` to blow up in practice.
pub(crate) const MAX_CROSS_AGG_LOOPS: usize = 256;

/// Soft per-aggregate cap on the petals considered when stitching them into
/// loops. After sorting an agg's petals by the deterministic priority, only
/// the smallest `MAX_AGG_PETALS` are considered (the rest are dropped and
/// the truncation flag is set). This bounds the `2^k` subset enumeration to
/// `2^MAX_AGG_PETALS` regardless of how dense the agg's element-graph
/// neighborhood is. A model with more than this many *disjoint* petals
/// through one agg is at or near the `MAX_LTM_SCC_NODES` auto-flip
/// threshold anyway (each petal contributes ≥2 distinct nodes plus the
/// shared agg to one SCC), so this is a conservative belt-and-suspenders;
/// 8 matches the pre-#515 hard cap.
const MAX_AGG_PETALS: usize = 8;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_CROSS_AGG_LOOPS`], scoped by an active
    /// [`AggLoopBudgetGuard`]. Lets a test trip the loop budget with a tiny
    /// fixture instead of building one large enough to trip the production
    /// constant (per docs/dev/rust.md#test-time-budgets, the PR #461
    /// cautionary tale).
    static AGG_LOOP_BUDGET_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The cross-element-through-aggregate loop-count budget for the current
/// `model_ltm_variables` invocation.  Returns [`MAX_CROSS_AGG_LOOPS`] in
/// production builds; in `#[cfg(test)]` builds an active
/// [`AggLoopBudgetGuard`] override takes precedence.
pub(super) fn cross_agg_loop_budget() -> usize {
    #[cfg(test)]
    {
        if let Some(b) = AGG_LOOP_BUDGET_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    MAX_CROSS_AGG_LOOPS
}

/// RAII guard (test-only) that overrides [`cross_agg_loop_budget`] for the
/// current thread for the guard's lifetime, restoring the previous value on
/// drop -- so a panicking test does not leak the override to the next test
/// reusing the thread.
///
/// Because `model_ltm_variables` is salsa-memoized, the guard must outlive
/// every `model_ltm_variables` call in the test whose budget it controls
/// (a later call on the same `db` would otherwise return the memoized
/// tiny-budget result regardless of the override state).
#[cfg(test)]
pub(crate) struct AggLoopBudgetGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl AggLoopBudgetGuard {
    pub(crate) fn new(budget: usize) -> Self {
        let prev = AGG_LOOP_BUDGET_OVERRIDE.with(|c| c.replace(Some(budget)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for AggLoopBudgetGuard {
    fn drop(&mut self) {
        AGG_LOOP_BUDGET_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Distinct orderings of `[0, 1, ..., n-1]` modulo rotation (index `0`
/// pinned first to kill rotations) and modulo mirror reversal (the loop
/// score is a commutative product over the edge multiset, so a directed
/// cycle and its reverse share a score; the design enumerates only one of
/// each mirror pair). Count: `1` for `n ∈ {0, 1, 2}`, `(n-1)!/2` for
/// `n ≥ 3` (`(n-1)!` rotation classes, halved by the mirror involution --
/// which has no fixed point for `n ≥ 3` since a length-`(n-1)` permutation
/// of distinct indices is never a palindrome; for `n = 2` reversing the
/// 2-cycle gives the same sequence, so there is nothing to quotient).
///
/// "Mirror" here is the petal-sequence reversed with the first petal still
/// pinned -- `[0, p1, .., p_{n-1}] ↦ [0, p_{n-1}, .., p1]` -- so the rule
/// is: enumerate the permutations of `[1, .., n-1]` via Heap's algorithm
/// (deterministic order, which `assign_loop_ids`' stable sort relies on for
/// stable distinct loop ids), and keep a permutation `tail` iff it is
/// lexicographically `<= reverse(tail)` (equivalently: track the emitted
/// canonicals in a set -- the lex test is the cheaper involution).
///
/// Pure: depends only on `n`. Only called from `recover_cross_agg_loops`
/// with `n` = a disjoint petal-subset size (≥ 2, ≤ `MAX_AGG_PETALS`), and
/// the loop budget stops recovery long before it reaches a subset large
/// enough for `(n-1)!/2` to be a concern.
pub(crate) fn cyclic_orderings(n: usize) -> Vec<Vec<usize>> {
    if n <= 1 {
        return vec![(0..n).collect()];
    }
    // Generate every permutation of the tail `[1, .., n-1]` via Heap's
    // algorithm, in its canonical order.
    let mut tail: Vec<usize> = (1..n).collect();
    let mut perms: Vec<Vec<usize>> = Vec::new();
    heaps_permutations(tail.len(), &mut tail, &mut perms);

    let mut orderings: Vec<Vec<usize>> = Vec::with_capacity(perms.len().div_ceil(2));
    for perm in perms {
        // Skip a permutation whose mirror (the tail reversed, with 0 still
        // pinned) sorts strictly before it -- we keep one of each mirror pair.
        let rev: Vec<usize> = perm.iter().rev().copied().collect();
        if perm > rev {
            continue;
        }
        let mut ordering = Vec::with_capacity(n);
        ordering.push(0);
        ordering.extend(perm);
        orderings.push(ordering);
    }
    orderings
}

/// Recursive Heap's algorithm: append every permutation of `a[0..k]` (with
/// `a[k..]` held fixed) to `out`, in Heap's canonical order. Called with
/// `k == a.len()`.
fn heaps_permutations(k: usize, a: &mut [usize], out: &mut Vec<Vec<usize>>) {
    if k <= 1 {
        out.push(a.to_vec());
        return;
    }
    for i in 0..k {
        heaps_permutations(k - 1, a, out);
        if k.is_multiple_of(2) {
            a.swap(i, k - 1);
        } else {
            a.swap(0, k - 1);
        }
    }
}

/// One agg petal: the element-level node sequence rotated to start at the
/// aggregate node (`[A, x_1, ..., x_m]`), plus its internal node set
/// `{x_1..x_m}`. `A` is the *element-level* agg node -- for an arrayed
/// synthetic agg that means the subscripted `$⁚ltm⁚agg⁚{n}[<elem>]` (so two
/// petals through `agg[a]` vs `agg[b]` go through *different* nodes and are
/// correctly never combined; `is_synthetic_agg_name` recognizes both the
/// bare and the subscripted form via its prefix check).
struct AggPetal<'a> {
    /// `[agg, x_1, ..., x_m]` -- the agg followed by the m internal nodes.
    nodes: Vec<&'a str>,
    internal: std::collections::HashSet<&'a str>,
}

/// Reconstruct the cross-element loops that traverse a synthetic aggregate
/// node more than once, from the element-level circuit list, under a
/// loop-count budget.
///
/// For each synthetic agg `A`, an elementary circuit that visits `A` exactly
/// once is its "petal": rotated to start at `A`, the node sequence
/// `[A, x_1, ..., x_m]` (the `x_i` are the petal's *internal* nodes -- the
/// rest of the cycle). Two petals are disjoint when their internal node sets
/// don't overlap. For a pairwise-disjoint subset of `m ≥ 2` petals of `A`,
/// the recovered loop's element-level node sequence concatenates the petals'
/// `nodes` (each of which starts with `A`) in some order:
/// `[A, p1_x..., A, p2_x..., ...]` -- `build_element_subscripted_links`
/// builds `seq[i] → seq[(i+1) % n]`, so this is exactly the cyclic sequence
/// `... → A → p1_x... → A → p2_x... → ... → A` (the last internal node wraps
/// to the first petal's `A`). Links / polarities come from
/// `build_element_subscripted_links`; the loop polarity is the product of
/// the link polarities (Unknown anywhere → Undetermined; the synthetic-agg
/// hops are Unknown here and patched later by `recover_agg_hop_polarities`).
///
/// Enumeration is bounded: an agg's petals are sorted by a deterministic
/// priority (fewest internal nodes first -- smaller petals combine into more
/// loops -- then a stable joined-`nodes` tiebreaker), the smallest
/// `MAX_AGG_PETALS` are kept (clipping flags that agg as truncated), the
/// disjoint subsets are walked smallest-cardinality-first (so under
/// truncation the few-petal -- likely most interesting -- loops survive), and
/// a running count is checked against `agg_loop_budget` (a global budget
/// across all aggs; once hit, recovery stops). The deterministic agg/petal
/// order is what makes the *truncated* loop set reproducible rather than
/// HashMap-iteration-order dependent.
///
/// Returns the recovered loops and the (sorted, deduped) names of the
/// aggregate nodes whose enumeration was clipped -- empty iff nothing was
/// clipped. An agg `A` is reported as truncated when either (a) the soft
/// per-agg petal cap dropped ≥ 1 of its petals, or (b) the global loop-count
/// budget fired at any point while `A` was being enumerated *or before `A`
/// would have been reached* (the per-agg loop walks aggs in sorted order, so
/// when the budget fires at agg `X` every agg sorted after `X` -- that has
/// ≥ 2 petals and so could have contributed loops -- is also un-enumerated
/// and thus reported). This is conservative in the same direction as the
/// petal-cap flag (see below): an agg whose disjoint-petal subsets would all
/// have collapsed to zero loops anyway is still flagged if the budget never
/// reached it -- over-reporting incompleteness is the safe direction, and
/// distinguishing it would require running the very enumeration the budget
/// exists to skip.
fn recover_cross_agg_loops(
    circuit_strs: &[Vec<&str>],
    var_graph: &crate::ltm::CausalGraph,
    source_vars: &HashMap<String, SourceVariable>,
    db: &dyn Db,
    project: SourceProject,
    agg_loop_budget: usize,
) -> (Vec<crate::ltm::Loop>, Vec<String>) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{LinkPolarity, Loop, LoopPolarity};

    // agg name -> its (deduped) petals.
    let mut petals_by_agg: HashMap<&str, Vec<AggPetal>> = HashMap::new();
    for circuit in circuit_strs {
        // Synthetic agg nodes in this circuit (bare or subscripted).
        let aggs: Vec<&str> = circuit
            .iter()
            .copied()
            .filter(|n| crate::ltm_agg::is_synthetic_agg_name(n))
            .collect();
        // Only build petals from a circuit that touches exactly one agg, once
        // (Johnson emits simple cycles, so "one agg in the node list" already
        // means "visited once"; a circuit through two distinct agg nodes --
        // `agg[a]` and `agg[b]` -- is a complete loop on its own, not a petal).
        if aggs.len() != 1 {
            continue;
        }
        let agg = aggs[0];
        let Some(pos) = circuit.iter().position(|n| *n == agg) else {
            continue;
        };
        let n = circuit.len();
        // Rotate so the agg is first; the rest is the petal's internal nodes.
        let nodes: Vec<&str> = (0..n).map(|j| circuit[(pos + j) % n]).collect();
        let internal: std::collections::HashSet<&str> = nodes[1..].iter().copied().collect();
        let entry = petals_by_agg.entry(agg).or_default();
        // Dedup on the internal set (Johnson can emit rotations of the same
        // simple cycle in some graphs; the internal set is rotation-invariant).
        if entry.iter().any(|p| p.internal == internal) {
            continue;
        }
        entry.push(AggPetal { nodes, internal });
    }

    let mut recovered: Vec<Loop> = Vec::new();
    // Names of aggs whose enumeration was clipped (by the soft petal cap or by
    // the global budget firing during/before them). A BTreeSet so the result
    // is deterministic-sorted and deduped regardless of insertion order.
    let mut truncated_aggs: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut emitted: usize = 0;
    // Deterministic agg iteration order so the budget clips reproducibly.
    let mut aggs: Vec<&str> = petals_by_agg.keys().copied().collect();
    aggs.sort();
    // If the global budget fires mid-enumeration, every agg sorted after the
    // one we were on is un-enumerated; this records that position so they can
    // be folded into `truncated_aggs` afterward.
    let mut budget_clip_idx: Option<usize> = None;
    'outer: for (agg_idx, agg) in aggs.iter().enumerate() {
        let mut petals: Vec<&AggPetal> = petals_by_agg[*agg].iter().collect();
        if petals.len() < 2 {
            continue;
        }
        // Deterministic priority: fewest internal nodes first (smaller petals
        // combine into more loops), then a stable tiebreaker (the petal's
        // node sequence joined). After dedup-on-internal-set the petals have
        // distinct `internal` sets and hence distinct `nodes` sequences, so
        // this key is a total order.
        petals.sort_by_cached_key(|p| (p.internal.len(), p.nodes.join("\u{0}")));
        if petals.len() > MAX_AGG_PETALS {
            // Drop the larger petals; the recovered set for this agg is now
            // incomplete, so report it. Conservatively flags truncation
            // whenever the soft petal cap drops >= 1 petal, even if those
            // petals would have contributed no disjoint-pair loop (every
            // dropped petal's internal nodes overlap a kept petal) --
            // over-reporting incompleteness is the safe direction, and
            // checking would mean running the disjoint-subset enumeration the
            // cap exists to bound.
            petals.truncate(MAX_AGG_PETALS);
            truncated_aggs.insert((*agg).to_string());
        }
        let k = petals.len();

        // Walk the 2^k subset masks smallest-cardinality-first (so the few-
        // petal loops -- which survive a tight budget -- come out first), and
        // within each cardinality in increasing mask order (which, given the
        // priority sort above, visits the smallest-internal petals first).
        // `k ≤ MAX_AGG_PETALS` keeps `1u32 << k` small.
        let mut masks: Vec<u32> = (0u32..(1u32 << k)).collect();
        masks.sort_by_key(|&m| (m.count_ones(), m));
        for mask in masks {
            if mask.count_ones() < 2 {
                continue;
            }
            let chosen: Vec<usize> = (0..k).filter(|&i| (mask >> i) & 1 == 1).collect();
            // Pairwise-disjoint internal node sets.
            let mut union: std::collections::HashSet<&str> = std::collections::HashSet::new();
            if chosen
                .iter()
                .any(|&i| !petals[i].internal.iter().all(|n| union.insert(n)))
            {
                continue;
            }
            // Each distinct cyclic ordering of the chosen petals is its own
            // directed cycle (a different edge *sequence*; per #308 that is a
            // distinct loop) -- but all of them share the same edge *multiset*
            // (every petal always contributes its same `A → first_internal →
            // ... → last_internal → A` edges, since the sequence is cyclic),
            // so they share a `loop_score`. For m = 2 there is exactly one
            // ordering, equal to the pre-#515 per-subset output.
            let m = chosen.len();
            for ord in cyclic_orderings(m) {
                let seq: Vec<&str> = ord
                    .iter()
                    .flat_map(|&j| petals[chosen[j]].nodes.iter().copied())
                    .collect();
                let var_level_nodes: Vec<Ident<Canonical>> =
                    seq.iter().map(|n| Ident::new(strip_subscript(n))).collect();
                let var_links = var_graph.circuit_to_links(&var_level_nodes);
                let links =
                    build_element_subscripted_links(&seq, &var_links, source_vars, db, project);
                let stocks: Vec<Ident<Canonical>> = seq
                    .iter()
                    .filter(|n| var_graph.stocks.contains(&Ident::new(strip_subscript(n))))
                    .map(|n| Ident::new(n))
                    .collect();
                let polarity = if links.iter().any(|l| l.polarity == LinkPolarity::Unknown) {
                    LoopPolarity::Undetermined
                } else {
                    let neg = links
                        .iter()
                        .filter(|l| l.polarity == LinkPolarity::Negative)
                        .count();
                    if neg % 2 == 0 {
                        LoopPolarity::Reinforcing
                    } else {
                        LoopPolarity::Balancing
                    }
                };
                recovered.push(Loop {
                    id: String::new(),
                    links,
                    stocks,
                    polarity,
                    // Cross-element loops visit fixed elements -- scalar loop score.
                    dimensions: vec![],
                });
                emitted += 1;
                if emitted >= agg_loop_budget {
                    // This agg's enumeration was clipped; the ones sorted
                    // after it never ran (handled below).
                    truncated_aggs.insert((*agg).to_string());
                    budget_clip_idx = Some(agg_idx);
                    break 'outer;
                }
            }
        }
    }

    // The global budget clipped mid-enumeration: every agg sorted after the
    // clip point is un-enumerated. Report those that had >= 2 petals (and so
    // could have contributed loops); an agg with < 2 petals would have been
    // skipped regardless of budget, so it is not "truncated".
    if let Some(clip_idx) = budget_clip_idx {
        for agg in &aggs[clip_idx + 1..] {
            if petals_by_agg[*agg].len() >= 2 {
                truncated_aggs.insert((*agg).to_string());
            }
        }
    }

    (recovered, truncated_aggs.into_iter().collect())
}

/// Recover the polarity of synthetic-aggregate-node hops in `loops` (GH #516).
///
/// The loop builders derive every link's polarity from the *variable-level*
/// causal graph, but synthetic `$⁚ltm⁚agg⁚{n}` nodes exist only in the
/// *element* graph -- so `analyze_link_polarity` finds no reference to the
/// agg's name in either endpoint's equation and a hop into or out of an agg
/// comes back `Unknown`, forcing every agg-traversing loop to `Undetermined`.
/// For the common (monotone) reducers the polarity is derivable, so patch it
/// here:
///
/// - `source[d] → agg`: `SUM`/`MEAN`/`MIN`/`MAX` are monotone non-decreasing
///   in each source element (raising any one element raises-or-holds the
///   result), so the hop is `Positive`. `STDDEV`/`RANK` are not monotone --
///   left `Unknown`.
/// - `agg → consumer`: the polarity of `consumer`'s equation with respect to
///   the reducer subexpression, computed by substituting the reducer with the
///   agg name and running ordinary static polarity analysis
///   (`CausalGraph::agg_consumer_polarity`).
///
/// Any loop whose links change is re-classified via `calculate_polarity`.
/// If anything was patched, loop IDs are re-assigned (the `r`/`b`/`u` prefix
/// is polarity-derived).
pub(super) fn recover_agg_hop_polarities(
    loops: &mut [crate::ltm::Loop],
    var_graph: &crate::ltm::CausalGraph,
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) {
    use crate::common::{Canonical, Ident};
    use crate::ltm::LinkPolarity;

    let aggs = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    // Canonicalize each synthetic agg's name once so it compares directly
    // against the (canonical) `Link.from` / `Link.to` idents.
    let synthetic: Vec<(Ident<Canonical>, &crate::ltm_agg::AggNode)> = aggs
        .aggs
        .iter()
        .filter(|a| a.is_synthetic)
        .map(|a| (Ident::new(a.name.as_str()), a))
        .collect();
    if synthetic.is_empty() {
        return;
    }

    let mut any_patched = false;
    for lp in loops.iter_mut() {
        let mut patched = false;
        for link in lp.links.iter_mut() {
            if link.polarity != LinkPolarity::Unknown {
                continue;
            }
            // `source[d] → agg`: the agg is this link's target.
            if let Some((_, agg)) = synthetic.iter().find(|(name, _)| name == &link.to) {
                if crate::ltm_agg::agg_reducer_is_monotone(&agg.equation_text) {
                    link.polarity = LinkPolarity::Positive;
                    patched = true;
                }
                continue;
            }
            // `agg → consumer`: the agg is this link's source.
            if let Some((agg_ident, agg)) = synthetic.iter().find(|(name, _)| name == &link.from) {
                let consumer = Ident::new(strip_subscript(link.to.as_str()));
                let p = var_graph.agg_consumer_polarity(&consumer, &agg.equation_text, agg_ident);
                if p != LinkPolarity::Unknown {
                    link.polarity = p;
                    patched = true;
                }
            }
        }
        if patched {
            lp.polarity = var_graph.calculate_polarity(&lp.links);
            any_patched = true;
        }
    }

    if any_patched {
        crate::ltm::assign_loop_ids(loops);
    }
}
