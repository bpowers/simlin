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
///
/// The returned `Vec` is sorted (GH #680): the source set is a `HashSet`,
/// so without the sort the iteration order is process-nondeterministic.
/// Pathway indices (`$⁚ltm⁚path⁚{port}⁚{idx}`) span outputs in this order,
/// and the parent's per-exit-port pathway selection (PR #684) recomputes
/// the same pathway map and must agree index-for-index with the sub-model's
/// own emission -- which only holds when both sort identically here.
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

    let mut output_ports: Vec<Ident<Canonical>> = output_ports.into_iter().collect();
    output_ports.sort();
    output_ports
}

/// Decide a sub-model's LTM output ports the way BOTH the sub-model's own
/// emission and the parent's per-exit-port override recompute must decide
/// them, so the two derivations are byte-identical by construction.
///
/// The pathway indices the sub-model emits as `$⁚ltm⁚path⁚{port}⁚{idx}` are
/// referenced cross-module by index in the parent's
/// `$⁚ltm⁚link_score⁚{x}→{m}⁚via⁚{exit}` override alias. The override and the
/// emission must therefore agree on the output-port set (and its order, hence
/// the sort inside `find_model_output_ports`) -- if they ever diverged, the
/// parent would alias a pathway var the sub-model never emitted (or emitted at
/// a different index). Funneling both sites through this one decision makes the
/// stdlib special-case (`output` convention) and the user-model port scan
/// impossible to skew apart.
///
/// This is also the authoritative source for the discovery-mode per-exit-port
/// recompute (GH #698 / PR #705): `analyze_model` builds a
/// `{sub_model -> ports}` map from this same decision and threads it into
/// `discover_loops_with_graph`, so the discovery recompute enumerates pathway
/// indices against the IDENTICAL project-wide sorted port set the sub-model
/// emitted against -- never a parent-scoped re-derivation that could shift the
/// indices when another project model reads an additional output port.
pub(crate) fn sub_model_output_ports(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<Ident<Canonical>> {
    // Stdlib models are always read through the `output` convention, so their
    // single LTM output port is `output` rather than a scanned port name.
    if model.name(db).starts_with("stdlib\u{205A}") {
        return vec![Ident::new("output")];
    }
    find_model_output_ports(db, model, project)
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
pub(super) fn build_a2a_loop_stocks(
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
    //
    // Building straight from the variable-level circuit is sound here
    // because `classify_cycle` guarantees no fast-path cycle traverses a
    // `ThroughAgg`-routed edge (GH #737): such a cycle's loop must instead
    // route `from → $⁚ltm⁚agg⁚{n} → to` (only the agg-half link scores are
    // scoreable), which only the element-level slow path below produces.
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
            slot_links: vec![],
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

            // Capture each circuit's element-subscripted link cycle, keyed by
            // its slot tuple (the subscript of node 0, the variable whose
            // dimensions define the loop's `dimensions`). This is what lets
            // the loop-score generator emit per-slot equations when the
            // group's link scores only exist as per-element FixedIndex /
            // per-target-element names (per-element-equation models, GH #653)
            // -- the ApplyToAll form would otherwise reference one arbitrary
            // element's link score for every slot. The generator still
            // prefers ApplyToAll whenever every link resolves to a Bare A2A
            // name, so Bare-shaped models keep their compact form.
            //
            // Skip the capture (leaving the legacy behavior) when two
            // circuits map to the same slot tuple -- the partial-collapse
            // case where node 0's variable carries fewer dimensions than the
            // cycle's full element space, so per-slot equations would be
            // ambiguous.
            let mut slot_links: Vec<(String, Vec<crate::ltm::Link>)> = Vec::new();
            let mut slot_keys_seen: std::collections::HashSet<String> =
                std::collections::HashSet::new();
            let mut slots_collide = false;
            for circuit in &circuits_in_group {
                let slot_tuple = match circuit[0].find('[') {
                    Some(start) => circuit[0][start + 1..circuit[0].len() - 1].to_string(),
                    // All nodes are subscripted in this branch; defensive.
                    None => continue,
                };
                if !slot_keys_seen.insert(slot_tuple.clone()) {
                    slots_collide = true;
                    break;
                }
                let circuit_var_links = var_graph.circuit_to_links(
                    &circuit
                        .iter()
                        .map(|n| Ident::new(strip_subscript(n)))
                        .collect::<Vec<_>>(),
                );
                let links = build_element_subscripted_links(
                    circuit,
                    &circuit_var_links,
                    source_vars,
                    db,
                    project,
                );
                slot_links.push((slot_tuple, links));
            }
            if slots_collide {
                slot_links.clear();
            }

            all_loops.push(Loop {
                id: String::new(),
                links,
                stocks,
                polarity,
                dimensions,
                slot_links,
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
                    slot_links: vec![],
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
                    slot_links: vec![],
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
/// `Σ_{m=2}^{k} C(k,m) = 2^k - k - 1` (one canonical loop per disjoint
/// petal subset, GH #676) -- still exponential in `k`, so a budget is
/// mandatory. When recovery hits this budget it stops, sets the truncation
/// flag, and the caller emits a `Warning`; the deterministic petal priority
/// (fewest internal nodes first, then a stable joined-name tiebreaker)
/// makes *which* loops survive truncation reproducible. The prior implicit
/// ceiling was `2^8 - 8 - 1 = 247` per agg (the old `MAX_AGG_PETALS = 8`
/// hard drop); 256 keeps roughly that order of magnitude as a model-wide
/// total.
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
pub(crate) fn cross_agg_loop_budget() -> usize {
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

/// A single agg petal for the mode-agnostic stitching core
/// [`stitch_cross_agg_petals`]: the node sequence rotated to start at the
/// aggregate node (`[A, x_1, ..., x_m]`) plus its internal node set.
///
/// Generic over the node representation so both the exhaustive path (which
/// works on `&str` circuit nodes) and the discovery path (which works on owned
/// `Ident<Canonical>` element nodes) feed the *same* combinatorial enumerator
/// -- the only way to guarantee discovery recovers exactly the loops exhaustive
/// does (GH #696). `K` is the agg-grouping key (a `&str` agg name in either
/// mode); the petals are pre-grouped by it so the stitcher never re-decides
/// which agg a petal belongs to. `A` is the *element-level* agg node -- for an
/// arrayed synthetic agg that means the subscripted `$⁚ltm⁚agg⁚{n}[<elem>]`
/// (so petals through `agg[a]` vs `agg[b]` go through *different* nodes and are
/// correctly never combined; `is_synthetic_agg_name` recognizes both forms).
pub(crate) struct StitchPetal<T> {
    /// `[agg, x_1, ..., x_m]` -- the agg followed by the m internal nodes.
    pub(crate) nodes: Vec<T>,
    /// `{x_1, ..., x_m}` -- the internal (non-agg) nodes; two petals are
    /// disjoint iff these sets do not overlap.
    pub(crate) internal: HashSet<T>,
}

/// The mode-agnostic combinatorial core of cross-element-through-aggregate loop
/// recovery (GH #515 for exhaustive, GH #696 for discovery): given each
/// synthetic agg's pairwise-distinct petals, enumerate the cross-agg loops as
/// stitched node sequences, under a model-wide loop-count budget.
///
/// For each agg with `≥ 2` petals: sort the petals by the deterministic
/// priority (fewest internal nodes first, then a stable joined-name
/// tiebreaker), keep the smallest [`MAX_AGG_PETALS`] (clipping flags the agg as
/// truncated), then walk pairwise-disjoint petal subsets of size `≥ 2`
/// smallest-cardinality-first; for each subset emit ONE canonical stitched
/// sequence `[A, p1_x..., A, p2_x..., ...]` -- the chosen petals concatenated
/// in priority order (the caller turns each sequence into its own loop --
/// `build_element_…_links`/`circuit_to_links` reconstitutes the
/// `... → A → p1_x → A → p2_x → ... → A` cycle).
///
/// One loop per disjoint petal subset (GH #676): for a fixed subset, EVERY
/// cyclic ordering of the petals produces the same edge multiset -- each
/// petal contributes the same `agg→head`, internal, and `tail→agg` edges
/// regardless of its position in the concatenation -- and the loop score is
/// a commutative product over that multiset, so all orderings share one
/// `loop_score`. Distinct orderings are distinct directed circuits but are
/// indistinguishable for dominance analysis; emitting them would only burn
/// the loop budget on duplicates and truncate genuinely-distinct subsets
/// earlier.
///
/// A running count is checked against `budget`; once hit, enumeration
/// stops and every not-yet-enumerated agg with `≥ 2` petals is reported as
/// truncated. The deterministic agg/petal/subset walk makes the
/// *truncated* output reproducible rather than HashMap-iteration dependent.
///
/// Returns the stitched node sequences (each starting at an agg) and the
/// (sorted, deduped) keys of the aggs whose enumeration was clipped. The
/// agg-key `K` is supplied via a sorted `Vec<(K, Vec<StitchPetal<T>>)>` so the
/// enumeration order is the caller's responsibility to make deterministic
/// (both modes sort the keys before calling).
///
/// Pure: no db, no I/O. The only shared invariant with the exhaustive path is
/// the petal priority / subset walk, which now lives here once.
pub(crate) fn stitch_cross_agg_petals<T, K>(
    petals_by_agg: Vec<(K, Vec<StitchPetal<T>>)>,
    budget: usize,
) -> (Vec<Vec<T>>, Vec<K>)
where
    T: Clone + Eq + std::hash::Hash + Ord,
    K: Clone + Ord,
{
    let mut stitched: Vec<Vec<T>> = Vec::new();
    let mut truncated: std::collections::BTreeSet<K> = std::collections::BTreeSet::new();
    let mut emitted: usize = 0;
    // The (key, petal-count) of every agg, kept so that if the global budget
    // fires mid-enumeration we can fold every *later* agg that had >= 2 petals
    // (and so could have contributed loops) into `truncated`.
    let petal_counts: Vec<(K, usize)> = petals_by_agg
        .iter()
        .map(|(k, p)| (k.clone(), p.len()))
        .collect();
    // If the global budget fires mid-enumeration, every agg after the one we
    // were on is un-enumerated; this records that position so they can be
    // folded into `truncated` afterward.
    let mut budget_clip_idx: Option<usize> = None;
    'outer: for (agg_idx, (_agg, mut petals)) in petals_by_agg.into_iter().enumerate() {
        if petals.len() < 2 {
            continue;
        }
        // Deterministic priority: fewest internal nodes first (smaller petals
        // combine into more loops), then a stable tiebreaker (the petal's node
        // sequence). After dedup-on-internal-set the petals have distinct
        // `internal` sets and hence distinct `nodes` sequences, so this is a
        // total order.
        petals.sort_by(|a, b| {
            a.internal
                .len()
                .cmp(&b.internal.len())
                .then_with(|| a.nodes.cmp(&b.nodes))
        });
        if petals.len() > MAX_AGG_PETALS {
            // Drop the larger petals; the recovered set for this agg is now
            // incomplete. Conservatively flags truncation whenever the soft
            // petal cap drops >= 1 petal (over-reporting incompleteness is the
            // safe direction).
            petals.truncate(MAX_AGG_PETALS);
            truncated.insert(_agg.clone());
        }
        let k = petals.len();

        // Walk the 2^k subset masks smallest-cardinality-first (so the few-
        // petal loops -- which survive a tight budget -- come out first), and
        // within each cardinality in increasing mask order. `k ≤ MAX_AGG_PETALS`
        // keeps `1u32 << k` small.
        let mut masks: Vec<u32> = (0u32..(1u32 << k)).collect();
        masks.sort_by_key(|&m| (m.count_ones(), m));
        for mask in masks {
            if mask.count_ones() < 2 {
                continue;
            }
            let chosen: Vec<usize> = (0..k).filter(|&i| (mask >> i) & 1 == 1).collect();
            // Pairwise-disjoint internal node sets.
            let mut union: HashSet<&T> = HashSet::new();
            if chosen
                .iter()
                .any(|&i| !petals[i].internal.iter().all(|n| union.insert(n)))
            {
                continue;
            }
            // One canonical ordering per subset: the chosen petals in
            // priority order (`chosen` is ascending over the sorted petals).
            // All cyclic orderings of a fixed subset share the same edge
            // multiset and hence the same loop score (see the fn doc), so a
            // single representative suffices.
            let seq: Vec<T> = chosen
                .iter()
                .flat_map(|&i| petals[i].nodes.iter().cloned())
                .collect();
            stitched.push(seq);
            emitted += 1;
            if emitted >= budget {
                truncated.insert(_agg.clone());
                budget_clip_idx = Some(agg_idx);
                break 'outer;
            }
        }
    }

    // The global budget clipped mid-enumeration: every agg sorted after the
    // clip point is un-enumerated. Report those that had >= 2 petals (an agg
    // with < 2 petals would have been skipped regardless of budget).
    if let Some(clip_idx) = budget_clip_idx {
        for (key, count) in petal_counts.into_iter().skip(clip_idx + 1) {
            if count >= 2 {
                truncated.insert(key);
            }
        }
    }
    (stitched, truncated.into_iter().collect())
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
/// `nodes` (each of which starts with `A`) in the canonical priority order
/// (one loop per subset -- see [`stitch_cross_agg_petals`]):
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

    // agg name -> its (deduped) petals (built as `&str`, then handed to the
    // shared stitching core).
    let petals_by_agg = collect_agg_petals(circuit_strs, |n| n);

    // Deterministic agg iteration order so the budget clips reproducibly.
    let mut sorted: Vec<(&str, Vec<StitchPetal<&str>>)> = petals_by_agg.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));

    let (stitched, truncated) = stitch_cross_agg_petals(sorted, agg_loop_budget);

    let mut recovered: Vec<Loop> = Vec::with_capacity(stitched.len());
    for seq in stitched {
        let var_level_nodes: Vec<Ident<Canonical>> =
            seq.iter().map(|n| Ident::new(strip_subscript(n))).collect();
        let var_links = var_graph.circuit_to_links(&var_level_nodes);
        let links = build_element_subscripted_links(&seq, &var_links, source_vars, db, project);
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
            slot_links: vec![],
        });
    }

    let truncated_aggs: Vec<String> = truncated.into_iter().map(|s| s.to_string()).collect();
    (recovered, truncated_aggs)
}

/// Group elementary circuits' single-agg petals by their agg, deduped on the
/// rotation-invariant internal node set.
///
/// A circuit that visits exactly one synthetic agg node (Johnson / the
/// discovery DFS emit simple cycles, so "one agg in the node list" means
/// "visited once") is a *petal*: rotate it so the agg is first, and the rest
/// is the petal's internal nodes. A circuit touching zero or two-plus distinct
/// aggs is not a petal (the latter is already a complete cross-agg loop).
///
/// `node_str` projects a circuit node `T` to its `&str` name so
/// `is_synthetic_agg_name` (and the agg-key grouping) can run uniformly over
/// both the exhaustive `&str` circuits and the discovery `Ident<Canonical>`
/// paths. The returned petals carry the original `T` nodes.
pub(crate) fn collect_agg_petals<'a, T, F>(
    circuits: &'a [Vec<T>],
    node_str: F,
) -> HashMap<&'a str, Vec<StitchPetal<T>>>
where
    T: Clone + Eq + std::hash::Hash,
    F: Fn(&'a T) -> &'a str,
{
    let mut petals_by_agg: HashMap<&'a str, Vec<StitchPetal<T>>> = HashMap::new();
    for circuit in circuits {
        let aggs: Vec<&'a str> = circuit
            .iter()
            .map(&node_str)
            .filter(|n| crate::ltm_agg::is_synthetic_agg_name(n))
            .collect();
        if aggs.len() != 1 {
            continue;
        }
        let agg = aggs[0];
        let Some(pos) = circuit.iter().position(|n| node_str(n) == agg) else {
            continue;
        };
        let n = circuit.len();
        let nodes: Vec<T> = (0..n).map(|j| circuit[(pos + j) % n].clone()).collect();
        let internal: HashSet<T> = nodes[1..].iter().cloned().collect();
        let entry = petals_by_agg.entry(agg).or_default();
        // Dedup on the internal set (Johnson can emit rotations of the same
        // simple cycle in some graphs; the internal set is rotation-invariant).
        if entry.iter().any(|p| p.internal == internal) {
            continue;
        }
        entry.push(StitchPetal { nodes, internal });
    }
    petals_by_agg
}

/// Polarity of one `source → agg` hop: the *discriminating* analysis of the
/// agg's own lowered body with respect to the source variable
/// ([`crate::ltm::CausalGraph::source_to_agg_polarity`], the
/// positive-by-convention Mul rule), applied uniformly to scalar feeders and
/// arrayed rows / co-sources (GH #737 review follow-ups I1/I1b):
///
/// - `SUM(pop[*])` / `SUM(pop[*] * scale)` w.r.t. `pop` → Positive (the
///   genuinely-monotone row cases keep their label),
/// - `SUM(pop[*] * scale)` w.r.t. `scale` → Positive,
/// - `SUM(pop[*] * (1 - weight[*]))` w.r.t. `weight` → Negative (the I1b
///   co-source case the old blanket monotone-Positive arm mislabeled --
///   "monotone in each summand" is not "monotone in every variable the
///   summand body composes"),
/// - indeterminate bodies (compound co-factors, STDDEV/RANK, lookups of
///   unknown direction) → Unknown, never a confident wrong label.
///
/// `recover_agg_hop_polarities` is the single consumer; the scored, pinned,
/// and detected surfaces all run that one recovery pass (the detected
/// surface splices its ThroughAgg-routed edges into explicit agg hops
/// first), so the hop polarity -- and hence the polarity-prefixed loop ids
/// the runtime join is keyed on -- is decided in exactly one place.
fn source_to_agg_hop_polarity(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    var_graph: &crate::ltm::CausalGraph,
    from_var_level: &str,
    agg: &crate::ltm_agg::AggNode,
) -> crate::ltm::LinkPolarity {
    use crate::ltm::LinkPolarity;

    // Reconstruct the agg's lowered AST from its equation text (the agg is
    // not a model variable, so the graph's variable map has no AST for it).
    // Mirrors `emit_source_to_agg_link_scores`' reconstruction.
    let agg_eqn = if agg.result_dims.is_empty() {
        datamodel::Equation::Scalar(agg.equation_text.clone())
    } else {
        datamodel::Equation::ApplyToAll(agg.result_dims.clone(), agg.equation_text.clone())
    };
    let Some(agg_var) =
        super::parse::reconstruct_ltm_var_lowered(db, &agg.name, &agg_eqn, model, project)
    else {
        return LinkPolarity::Unknown;
    };
    let Some(agg_ast) = agg_var.ast() else {
        return LinkPolarity::Unknown;
    };
    let source = Ident::<Canonical>::new(from_var_level);
    var_graph.source_to_agg_polarity(&source, agg_ast)
}

/// Recover the polarity of synthetic-aggregate-node hops in `loops` (GH #516).
///
/// The loop builders derive every link's polarity from the *variable-level*
/// causal graph, but synthetic `$⁚ltm⁚agg⁚{n}` nodes exist only in the
/// *element* graph -- so `analyze_link_polarity` finds no reference to the
/// agg's name in either endpoint's equation and a hop into or out of an agg
/// comes back `Unknown`, forcing every agg-traversing loop to `Undetermined`.
/// For the derivable cases, patch it here:
///
/// - `source → agg`: [`source_to_agg_hop_polarity`] (the discriminating
///   `analyze_source_to_agg_polarity` body analysis, applied uniformly to
///   arrayed rows and scalar feeders alike; indeterminate bodies stay
///   `Unknown`, never a blanket monotone label).
/// - `agg → consumer`: the polarity of `consumer`'s equation with respect to
///   the reducer subexpression, computed by substituting the reducer with the
///   agg name and running ordinary static polarity analysis
///   (`CausalGraph::agg_consumer_polarity`).
///
/// Any loop whose links change is re-classified via `calculate_polarity`.
/// If anything was patched, loop IDs are re-assigned (the `r`/`b`/`u` prefix
/// is polarity-derived).
pub(crate) fn recover_agg_hop_polarities(
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
            // `source → agg`: the agg is this link's target.
            if let Some((_, agg)) = synthetic.iter().find(|(name, _)| name == &link.to) {
                let from_var_level = strip_subscript(link.from.as_str());
                let p =
                    source_to_agg_hop_polarity(db, model, project, var_graph, from_var_level, agg);
                if p != LinkPolarity::Unknown {
                    link.polarity = p;
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

/// Expand each variable-level loop's `ThroughAgg`-routed edges into explicit
/// agg-node hops, one loop variant per routed agg -- the detected-FFI-surface
/// bijection with the scored loop set (GH #737 round-2 review, C1b).
///
/// `model_detected_loops` enumerates loops on the variable-level graph, whose
/// links never traverse agg nodes: an edge like `scale → grow` (where `grow`
/// hoists `SUM(pop[*] * scale)`) carries the whole through-agg routing as ONE
/// link. The scored surface (`model_ltm_variables`) instead enumerates the
/// element graph, where that edge is `scale → $⁚ltm⁚agg⁚{n} → grow` -- one
/// loop PER routed agg when the same feeder is read by several hoisted
/// reducers of the target. The polarity-prefixed loop ids `assign_loop_ids`
/// derives are the key the runtime join reads `$⁚ltm⁚loop_score⁚{id}` with
/// (`reclassify_loops_from_results`, pysimlin's `get_relative_loop_score`),
/// so a count or polarity mismatch between the surfaces makes a detected
/// loop read ANOTHER loop's series. Composing the hop polarities onto the
/// single variable-level link (the round-1 fix) was not enough: a multi-agg
/// edge still left the detected surface one loop short (id collision), and
/// hop polarities that DISAGREE across the routed aggs collapsed to one
/// Unknown loop where the scored surface has two definite ones.
///
/// So: rebuild each loop as the cartesian product, over its links, of that
/// link's routing variants -- the direct link itself when the edge has any
/// `Direct` site (a mixed Direct+ThroughAgg edge genuinely has both
/// pathways; the element graph emits both, so Johnson finds both loop
/// variants on the scored side too), plus one `from → agg → to` splice per
/// distinct routed agg. Spliced hops start `Unknown`; the caller runs the
/// SAME `recover_agg_hop_polarities` pass the scored surface uses, so the
/// polarities -- and with both surfaces' links now speaking the same node
/// language, the `loop_id_sort_key` orderings -- agree by construction.
///
/// Bijection boundary, precisely: for a cycle whose variables are all
/// SCALAR, this expansion is isomorphic to the element-graph circuits the
/// scored surface enumerates (scalar nodes don't expand per element; every
/// agg hoisted from a scalar-consumer equation is itself scalar; and the
/// scored cross-agg petal stitching never fires for scalar cycles -- all of
/// one agg's petals pass through the agg's single host variable, so no two
/// petals are disjoint). Cycles involving ARRAYED variables remain
/// variable-level here while the scored surface enumerates them per element
/// -- the pre-existing divergent class (`reclassify_loops_from_results`
/// skips ids with no score series; durable cross-surface identity for them
/// is the stock set, not the id).
///
/// Defensive bound: a loop whose variant product exceeds
/// [`MAX_DETECTED_AGG_VARIANTS_PER_LOOP`] is kept unexpanded (its ids may
/// then diverge from the scored surface for that pathological model, which
/// enumerates under its own `MAX_LTM_CIRCUITS` budget).
pub(crate) fn expand_loops_through_routed_aggs(
    loops: Vec<crate::ltm::Loop>,
    var_graph: &crate::ltm::CausalGraph,
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> Vec<crate::ltm::Loop> {
    use crate::common::{Canonical, Ident};
    use crate::ltm::{Link, LinkPolarity, Loop};

    let aggs = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    if !aggs.aggs.iter().any(|a| a.is_synthetic) {
        return loops;
    }
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);

    /// One way a variable-level link can be realized.
    enum LinkVariant {
        /// Keep the original link (a Direct pathway exists, or no routing).
        Direct,
        /// Splice through the routed agg at this index in `aggs.aggs`.
        ViaAgg(usize),
    }

    let mut expanded: Vec<Loop> = Vec::with_capacity(loops.len());
    for lp in loops {
        // Per link, the realization variants in deterministic order
        // (Direct first, then aggs in first-occurrence site order).
        let per_link: Vec<Vec<LinkVariant>> = lp
            .links
            .iter()
            .map(|link| {
                let from_var_level = strip_subscript(link.from.as_str());
                let to_var_level = strip_subscript(link.to.as_str());
                let Some(sites) = ir
                    .sites
                    .get(&(from_var_level.to_string(), to_var_level.to_string()))
                else {
                    return vec![LinkVariant::Direct];
                };
                let mut has_direct = sites.is_empty();
                let mut agg_idxs: Vec<usize> = Vec::new();
                for s in sites {
                    match &s.routing {
                        crate::db::ltm_ir::SiteRouting::ThroughAgg { agg } => {
                            if !agg_idxs.contains(&agg.0) {
                                agg_idxs.push(agg.0);
                            }
                        }
                        crate::db::ltm_ir::SiteRouting::Direct => has_direct = true,
                    }
                }
                let mut variants: Vec<LinkVariant> = Vec::with_capacity(1 + agg_idxs.len());
                if has_direct || agg_idxs.is_empty() {
                    variants.push(LinkVariant::Direct);
                }
                variants.extend(agg_idxs.into_iter().map(LinkVariant::ViaAgg));
                variants
            })
            .collect();

        // A loop none of whose links route through an agg is kept verbatim
        // (ids/polarities untouched). NOTE: a single pure-ThroughAgg variant
        // still has a product of 1, so the gate is "any non-Direct variant",
        // not the product size.
        let all_direct = per_link
            .iter()
            .all(|v| v.len() == 1 && matches!(v[0], LinkVariant::Direct));
        if all_direct {
            expanded.push(lp);
            continue;
        }
        let n_variants: usize = per_link.iter().map(|v| v.len()).product();
        if n_variants > MAX_DETECTED_AGG_VARIANTS_PER_LOOP {
            expanded.push(lp);
            continue;
        }

        // Cartesian product over the per-link variants, in row-major order
        // (deterministic: per_link orders are deterministic and the product
        // walk is positional).
        let mut choices: Vec<usize> = vec![0; per_link.len()];
        loop {
            let mut links: Vec<Link> = Vec::with_capacity(lp.links.len());
            for (i, link) in lp.links.iter().enumerate() {
                match per_link[i][choices[i]] {
                    LinkVariant::Direct => links.push(link.clone()),
                    LinkVariant::ViaAgg(idx) => {
                        let agg_ident = Ident::<Canonical>::new(aggs.aggs[idx].name.as_str());
                        // Spliced hops start Unknown; the caller's
                        // `recover_agg_hop_polarities` pass patches them
                        // exactly as it does for the scored surface's loops.
                        links.push(Link {
                            from: link.from.clone(),
                            to: agg_ident.clone(),
                            polarity: LinkPolarity::Unknown,
                        });
                        links.push(Link {
                            from: agg_ident,
                            to: link.to.clone(),
                            polarity: LinkPolarity::Unknown,
                        });
                    }
                }
            }
            let polarity = var_graph.calculate_polarity(&links);
            expanded.push(Loop {
                id: String::new(),
                links,
                stocks: lp.stocks.clone(),
                polarity,
                dimensions: lp.dimensions.clone(),
                slot_links: vec![],
            });

            // Advance the mixed-radix counter.
            let mut pos = 0;
            loop {
                if pos == per_link.len() {
                    break;
                }
                choices[pos] += 1;
                if choices[pos] < per_link[pos].len() {
                    break;
                }
                choices[pos] = 0;
                pos += 1;
            }
            if pos == per_link.len() {
                break;
            }
        }
    }

    expanded
}

/// Defensive cap on the per-loop routing-variant product in
/// [`expand_loops_through_routed_aggs`]: a loop with `k` ThroughAgg-routed
/// links of `a_i` aggs each expands into `Π (a_i + direct_i)` variants. Real
/// models have a handful of reducers per equation and short cycles, so this
/// is far above anything reachable; a loop over the cap stays unexpanded
/// rather than blowing up the FFI loop list.
const MAX_DETECTED_AGG_VARIANTS_PER_LOOP: usize = 64;
