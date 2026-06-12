// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Link-score emission for LTM synthetic variables.
//!
//! These helpers were lifted verbatim out of `model_ltm_variables`'s body
//! (they were top-level `fn` items nested in the function, capturing nothing
//! from the enclosing scope). They decide a causal edge's link-score
//! dimensions and emit the per-edge / per-shape / through-aggregate
//! `LtmSyntheticVar`s; `model_ltm_variables` (in the parent module) calls
//! them as `link_scores::*`.

use std::collections::{HashMap, HashSet};

use crate::common::{Canonical, Ident};
use crate::datamodel;

use crate::db::{
    CompilationDiagnostic, Db, Diagnostic, DiagnosticError, DiagnosticSeverity, LtmLinkId,
    LtmSyntheticVar, RefShape, SourceModel, SourceProject, SourceVariable, SourceVariableKind,
    project_dimensions_context, reconstruct_single_variable, variable_dimensions,
};

use super::compile::{ShapedLinkScore, link_score_equation_text_shaped};
use super::loops::{ReadSliceRow, cartesian_subscripts, read_slice_rows};
use super::parse::{
    ltm_equation_dimensions, reconstruct_ltm_var_lowered, retarget_ltm_equation_dims,
};

/// Determine the dimensions a link score should carry.
///
/// Returns the target's dimension names when the edge is
/// same-dimension A2A or scalar-to-arrayed. Returns empty for
/// scalar edges, module-involved links (modules are scalar nodes),
/// and arrayed-to-scalar edges (cross-dimensional; handled by
/// `try_cross_dimensional_link_scores` which generates N separate
/// scalar variables).
///
/// `model` is consulted (via `model_edge_shapes`) only by the
/// mapped-dimension compatibility arm, which requires the edge to have a
/// `Bare`-classified reference site -- the exact condition under which the
/// element graph expands the edge as the mapped DIAGONAL rather than the
/// conservative cross-product (see the GH #527 comment in the body).
///
/// The returned names use the original datamodel casing (e.g.,
/// "Region" not "region") because `parse_ltm_equation` feeds them
/// into `Equation::ApplyToAll`, which `get_dimensions` resolves by
/// exact string match against the project's datamodel dimensions.
pub(super) fn link_score_dimensions(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
) -> Vec<String> {
    let to_sv = match source_vars.get(to) {
        Some(sv) => sv,
        // Implicit variables (SMOOTH/DELAY expansions) may not be
        // in source_vars; treat as scalar.
        None => return vec![],
    };
    // Module variables are scalar nodes in the causal graph.
    if to_sv.kind(db) == SourceVariableKind::Module {
        return vec![];
    }
    let to_dims = variable_dimensions(db, *to_sv, project);
    if to_dims.is_empty() {
        return vec![];
    }

    let from_dims = source_vars
        .get(from)
        .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
        .map(|sv| variable_dimensions(db, *sv, project).clone())
        .unwrap_or_default();

    // Scalar source -> arrayed target: NOT handled here. The main
    // link-score loop routes these to `try_scalar_to_arrayed_link_scores`
    // (one scalar link score per target element) before
    // `emit_per_shape_link_scores` is reached. Returning empty here is
    // the safe fallback if that routing is ever bypassed (e.g. the
    // target failed to lower): a scalar Bare link score
    // (`{from}→{to}`, no dims) parses to the useless-but-harmless edge
    // `(from, to)`, whereas a Bare-A2A var would make
    // `expand_a2a_link_offsets` invent a phantom `from[elem]` node that
    // breaks loops through `from` in the search graph.
    if from_dims.is_empty() {
        return vec![];
    }

    // Same-dimension A2A: both have identical dimension(s).
    // Partial-collapse: source has more dimensions than target, but all
    //   target dimensions are present in the source (e.g., source[D1,D2]
    //   -> target[D1]). The link score gets the target's (shared) dims.
    //
    // NOTE: When from_dims == to_dims and the dependency is CrossElement
    // (e.g., `share[R] = population[R] / SUM(population[*])`), this
    // creates only N diagonal link scores (one per element). Off-diagonal
    // link scores (e.g., population[boston] -> share[nyc]) are not
    // generated; the wildcard reducer already aggregates the
    // cross-element contribution into the diagonal A2A link score, and
    // build_element_level_loops uses those diagonal values when
    // emitting scalar Loops for the surviving cross-element circuits.
    //
    // Check whether this edge should use the target's dimensions for
    // the link score. This covers:
    // - Same-dimension A2A: from_dims == to_dims
    // - Partial-collapse: to_dims ⊆ from_dims (e.g., [D1,D2]→[D1])
    // - Broadcast: from_dims ⊆ to_dims (e.g., [D1]→[D1,D2])
    // - Mapped dimensions (GH #527): a differently-named pair related by
    //   a declared dimension mapping counts as corresponding, the same
    //   element correspondence the element graph's diagonal projection
    //   uses. Without this the mapped Bare A2A edge got a SCALAR link
    //   score whose equation referenced arrayed variables in scalar
    //   context (a compile failure, silently stubbed to 0) while the
    //   loop-score equations subscript the Bare name per slot -- the
    //   arrayed form both compiles (the equation's references resolve
    //   through the same mapping the model's own equations use) and
    //   resolves those per-slot references.
    //
    // In all these cases, the link score inherits the target's
    // dimensions so per-element values are computed via A2A expansion.
    let dim_ctx = project_dimensions_context(db, project);
    // PR #761 review (r3389029131): the mapped arm is additionally gated on
    // the edge having a `Bare`-classified reference site -- the exact
    // condition under which `expand_same_element` emits the mapped DIAGONAL
    // element edges, so "score arrayed over the target's dims" ⟺ "element
    // edges are the diagonal". Since GH #757 the classifier
    // (`classify_iterated_dim_shape` via `classify_axis_access`) gates its
    // mapped arm on the SAME `mapped_element_correspondence` data, BOTH
    // declaration directions, so a positionally-mapped subscripted
    // reference (forward- or reverse-declared) classifies `Bare` and passes
    // this gate with the diagonal it deserves. The remaining shapes the
    // gate excludes are the ELEMENT-mapped pairs (declined by the
    // GH #756 positional-only rule, classified `DynamicIndex`, cross-product
    // element edges): retargeting such an edge's (Bare-named, since
    // Wildcard/DynamicIndex collapse onto the Bare name) score to the
    // target's dims would shape per-slot DIAGONAL partials that the
    // off-diagonal loop links then read by target-element subscript --
    // silent wrong-slot values. Denied the retarget, such an edge instead
    // takes the GH #758 loud skip in `emit_per_shape_link_scores` (no
    // link-score variable, loop scores through the edge dropped, one
    // Warning). A mixed edge (a Bare site AND
    // a DynamicIndex site on the same `(from, to)`) keeps the arrayed score
    // -- the Bare site needs it -- while its cross-product links still read
    // diagonal slots; that is the pre-existing mixed-shape conservatism
    // family, not changed here. The same-NAME arms below stay
    // shape-independent. Absent-edge lookups (a `(from, to)` not in the
    // causal graph, which the emitters never produce) fail the gate, which
    // errs toward the conservative scalar.
    let edge_has_bare_site = crate::db::model_edge_shapes(db, model, project)
        .edge_shapes
        .get(&(from.to_string(), to.to_string()))
        .is_some_and(|shapes| shapes.contains(&RefShape::Bare));
    let dims_correspond =
        |td: &crate::dimensions::Dimension, fd: &crate::dimensions::Dimension| -> bool {
            td.name() == fd.name()
                || (edge_has_bare_site
                    && dim_ctx
                        .mapped_element_correspondence(td.canonical_name(), fd.canonical_name())
                        .is_some())
        };
    let dims_compatible = from_dims == *to_dims
        || to_dims
            .iter()
            .all(|td| from_dims.iter().any(|fd| dims_correspond(td, fd)))
        || from_dims
            .iter()
            .all(|fd| to_dims.iter().any(|td| dims_correspond(td, fd)));

    if dims_compatible {
        // Map canonical dimension names back to their original
        // datamodel names for correct equation parsing.
        to_dims
            .iter()
            .map(|d| {
                let canonical = d.name();
                dm_dims
                    .iter()
                    .find(|dm| crate::common::canonicalize(dm.name()).as_ref() == canonical)
                    .map(|dm| dm.name().to_string())
                    .unwrap_or_else(|| canonical.to_string())
            })
            .collect()
    } else {
        // Cross-dimensional (arrayed-to-scalar, or mismatched
        // dimensions). These edges are handled by
        // try_cross_dimensional_link_scores which generates N
        // separate scalar variables instead of one arrayed variable.
        // Return empty here so the normal A2A path is skipped.
        vec![]
    }
}

/// Generate per-element link score variables for a reducer edge, or
/// return `None` if the edge is not a reduce.
///
/// Two shapes are handled:
///   * **Full reduce** -- an arrayed source feeds a *scalar* target
///     through an array-reducing builtin (`total = SUM(pop[*])`). Each
///     source element gets its own scalar link score
///     `$⁚ltm⁚link_score⁚pop[e]→total` measuring how much varying that
///     single element affects the scalar target while holding all other
///     elements at `PREVIOUS`.
///   * **Partial reduce** -- an arrayed source feeds an *arrayed-result*
///     reducer whose dims are a strict subset of the source's dims
///     (`agg[D1] = SUM(matrix[D1,*])` collapses only `D2`). For each
///     `(d1, d2)` pair the relevant target is only `agg[d1]`, so the
///     link score is the per-`(d1, d2)` scalar variable
///     `$⁚ltm⁚link_score⁚matrix[d1,d2]→agg[d1]` (both axes ride in the
///     source subscript; only the surviving axis in the target
///     subscript). The ceteris-paribus partial holds the rest of the
///     `matrix[d1,*]` slice at `PREVIOUS`. All emitted vars are scalar
///     (`dimensions = vec![]`), consistent with the full-reduce naming
///     -- `parse_link_offsets` keeps element-level-on-both-sides names
///     as a single passthrough edge, so no parser change is needed.
///
/// **Row derivation (GH #765 / shape-expressiveness T3, invariant I4)**:
/// when `to` IS a variable-backed aggregate node reading `from` and the
/// shared [`crate::ltm_agg::variable_backed_reduce_agg`] gate admits it,
/// the rows / slots / co-reduced sets come from `read_slice_rows` over
/// `from`'s read slice -- `Pinned` axes fixed to their literal element,
/// subset-`Reduced` axes enumerated over the subset only -- so the divisor
/// is the true read count and unread rows get NO score, matching the
/// element edges `emit_agg_routed_edges` derives from the same slice.
/// Since GH #783 that match is structural, not coincidental: both surfaces
/// walk the ONE `read_slice_row_parts` derivation (`read_slice_rows` is its
/// comma-joined + co-reduced projection), so a change to the slice vocabulary
/// updates both in lockstep (invariant I4). For
/// an all-`Iterated`/full-extent slice the read rows ARE the cartesian
/// rows (byte-identical to the pre-T3 derivation). An ITERATED-DIM
/// PROJECTION FEEDER edge into a variable-backed reduce (GH #767 / T5:
/// `frac -> growth` for `growth[D1] = SUM(matrix[D1,*] * frac[D1])`,
/// whose dims EQUAL the owner's so the partial-reduce shape check below
/// would decline it) is routed FIRST, by the feeder's own slice, to
/// per-`(row, slot)` changed-last scores ([`iterated_feeder_row_scores`]).
/// The full `from_dims` cartesian product below remains only for edges
/// with NO variable-backed agg at all: the not-hoisted conservative family
/// (dynamic-index reducers, declined mappings, and the slice combinations
/// the I1 acceptance declines -- differing co-sources, non-projection
/// feeders like the GH #743 family's Pinned-axis residue), byte-identical
/// to pre-T3. (The GH #764 broadcast/permuted result shapes
/// used to ride it too; since T4 they mint SYNTHETIC aggs at enumeration,
/// so their edges route through the two-half agg emitters and never reach
/// this function.)
///
/// The BROADCAST-REDUCE shape (GH #777): an ARRAYED-owner scalar-result
/// Pinned/subset slice (`share[Region] = SUM(pop[nyc,*])` -- a
/// variable-backed agg with no `Iterated` axis whose single scalar value
/// broadcasts over `to`'s dims). The dedicated branch near the top of this
/// function ([`emit_broadcast_reduce_link_scores`]) emits the
/// per-(read-row, full-target-element) scalar scores
/// (`pop[nyc,p]→share[nyc]`, `pop[nyc,p]→share[boston]`, ...): each read
/// row feeds every target slot with the same value, so the row is fixed by
/// the slice and the slot ranges over `to`'s entire element set, in
/// lockstep with `emit_agg_routed_edges`' broadcast fan-out. It fires
/// BEFORE the partial-reduce `result_axis_names` containment check, so the
/// RELATED-dim spelling (`share[Region]`) and the DISJOINT-dim spelling
/// (`share[D9]`, `D9` not a source dim) are scored identically. (Pre-T3
/// this shape emitted full-cartesian per-`(row, slot)` garbage; pre-fix it
/// took a loud GH #758 skip -- both are retired.)
///
/// Returns `None` for scalar-to-scalar, same-dimension A2A, broadcast
/// (`from_dims ⊆ to_dims`), mismatched dimensions, module-involved
/// edges, and any edge where the reducer cannot be classified. Returns
/// `Some(vec![])` for SIZE edges (constant reducer, no scores) and the
/// remaining loud declines (the GH #780 projection-feeder doom and the
/// GH #778/#785 duplicated-dim skip).
#[allow(clippy::too_many_arguments)] // threads salsa keys + emission context
pub(super) fn try_cross_dimensional_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    agg_nodes: &crate::ltm_agg::AggNodesResult,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> Option<Vec<LtmSyntheticVar>> {
    // Only applies when the source is arrayed.
    let from_sv = source_vars.get(from)?;
    if from_sv.kind(db) == SourceVariableKind::Module {
        return None;
    }
    let from_dims = variable_dimensions(db, *from_sv, project);
    if from_dims.is_empty() {
        return None;
    }

    let to_sv = source_vars.get(to)?;
    if to_sv.kind(db) == SourceVariableKind::Module {
        return None;
    }
    let to_dims = variable_dimensions(db, *to_sv, project);

    // T5 (GH #767): an ITERATED-DIM PROJECTION FEEDER edge into a
    // variable-backed reduce -- `frac → growth` for
    // `growth[D1] = SUM(matrix[D1,*] * frac[D1])`. The feeder's dims EQUAL
    // the owner's, so the partial-reduce shape derivation below would
    // decline the edge before the agg branch is reached; route it by its
    // own read slice first (per-row changed-last scores, 1:1
    // rows-to-slots), replacing the pre-T5 Bare changed-last conservative
    // score for this shape. Gated on the SAME `variable_backed_reduce_agg`
    // decision the element graph and the loop builder consult, so the
    // emitted per-`(row, slot)` names are exactly the hops the per-circuit
    // loops reference. A `None` from the row derivation (stale slice
    // invariant) falls through to the pre-T5 paths.
    if let Some(vb_agg) = agg_nodes
        .aggs_in_var(to)
        .find(|a| !a.is_synthetic && a.name == to && a.reads_var(from))
        && vb_agg.source_is_projection_feeder(from)
        && crate::ltm_agg::variable_backed_reduce_agg(agg_nodes, from, to, to_dims).is_some()
        && let Some(feeder_vars) = iterated_feeder_row_scores(
            db,
            model,
            project,
            from,
            from_dims,
            vb_agg,
            unscoreable_edges,
        )
    {
        // `Some(vec![])` can be the GH #780 loud doom: the edge was
        // recorded in `unscoreable_edges` (here `vb_agg.name == to`) and
        // returning it keeps the dispatcher from minting a wrong-shaped
        // stand-in -- same contract as the other loud declines below.
        return Some(feeder_vars);
    }

    // BROADCAST REDUCE (GH #777): a VARIABLE-BACKED agg whose slice has NO
    // `Iterated` axis (Pinned and/or subset-`Reduced` only) but whose owner
    // `to` is ARRAYED -- `share[Region] = SUM(pop[nyc,*])`. The reducer
    // collapses to ONE scalar value that broadcasts over every `to[e]`, so
    // each read row (`pop[nyc,p]`, `pop[nyc,q]`) feeds every target element:
    // emit `{from}[{row}]→{to}[{e}]` for the cartesian product of read rows
    // and the FULL target element set, each scored by the per-row reducer
    // partial with the target reference pinned to `e` (the same builder the
    // aligned partial-reduce arm uses, with `result_element = e` and the
    // co-reduced slice the ALL read rows -- every row contributes to every
    // slot). The read rows are independent of `to`'s dims, so the RELATED-dim
    // spelling (`share[Region]`, `Region` a source dim) and the DISJOINT-dim
    // spelling (`share[D9]`, `D9` not a source dim) are handled identically
    // -- this branch fires BEFORE the partial-reduce `result_axis_names`
    // containment check that would early-return `None` for the disjoint
    // spelling. Gated on the SAME `variable_backed_reduce_agg` decision the
    // element graph's broadcast fan-out (`emit_agg_routed_edges`) and the
    // loop builder consult, so the emitted per-(row, e) names are exactly the
    // hops the per-circuit loops reference. This replaces the pre-fix GH #758
    // loud skip (its `emit_unscoreable_broadcast_reduce_edge_warning` is
    // deleted), which itself replaced the pre-T3 full-cartesian garbage.
    if !to_dims.is_empty()
        && let Some(vb_agg) = agg_nodes
            .aggs_in_var(to)
            .find(|a| !a.is_synthetic && a.name == to && a.reads_var(from))
        && vb_agg.result_dims.is_empty()
        && crate::ltm_agg::variable_backed_reduce_agg(agg_nodes, from, to, to_dims).is_some()
    {
        return emit_broadcast_reduce_link_scores(
            db,
            source_vars,
            from,
            to,
            from_dims,
            to_dims,
            vb_agg,
            model,
            project,
        );
    }

    // Determine whether this edge is a full reduce (scalar target) or a
    // partial reduce (arrayed result over a strict subset of the
    // source's axes). The "result axis" names are the target's dims for
    // a partial reduce (empty for a full reduce); the implied reduced
    // axes are `from_dims` minus the result axes -- we never need the
    // reduced-axis names explicitly because the co-reduced source slice
    // is derived directly from the source element tuples.
    let result_axis_names: Vec<String> = if to_dims.is_empty() {
        vec![]
    } else {
        // Partial reduce requires every target dim to be a source dim
        // and strictly fewer target dims than source dims (so at least
        // one axis collapses). Same-dim A2A, broadcast, and mismatched
        // dims all fall through to `None` (handled by other paths).
        let from_names: Vec<&str> = from_dims.iter().map(|d| d.name()).collect();
        let to_names: Vec<&str> = to_dims.iter().map(|d| d.name()).collect();
        if to_names.len() >= from_names.len() || !to_names.iter().all(|tn| from_names.contains(tn))
        {
            return None;
        }
        to_names.iter().map(|s| s.to_string()).collect()
    };

    // The source is a reducer argument. Classify the reducing function
    // in the target's equation.
    let to_var = reconstruct_single_variable(db, model, project, to)?;
    let classified = crate::ltm_augment::classify_reducer(&to_var, from)?;

    if classified.kind == crate::ltm_augment::ReducerKind::Constant {
        // SIZE is constant; link score is always 0. Skip entirely.
        return Some(vec![]);
    }

    // The body-aware partial context (GH #744): without it the linear
    // shortcut would score each row as the bare source element, ignoring
    // any coefficient the reducer body applies to the source
    // (`SUM(pop[*] * (1 - weight[*]))` w.r.t. `weight` has the
    // sign-flipping coefficient `-pop[e]`).
    let (arrayed_dep_dims, model_deps) =
        reducer_body_ctx_parts(db, source_vars, project, &classified.body_text);
    let row_dim_names: Vec<String> = from_dims.iter().map(|d| d.name().to_string()).collect();
    // The live source's accepted slice, when `to` IS a variable-backed agg
    // reading `from`: lets the body partial resolve a mismatched-arity
    // feeder dep's index at the slice's ITERATED axis position -- sound and
    // LOAD-BEARING for a repeated-dim co-source like `matrix[D1,D1]` read as
    // `SUM(matrix[*, D1] * frac[D1])` (slice `[Reduced, Iterated]`,
    // `result_dims = [D1]`: still minted, the GH #767 live shape), where a
    // by-name lookup is ambiguous (see `resolve_mismatched_index_position`).
    // Only the DOUBLY-Iterated case (two Iterated axes over the same dim,
    // result_dims repeated) is declined at agg minting (GH #778/#785,
    // `result_dims_has_repeated_dim`) and so never reaches here. The
    // un-hoisted cartesian family has no agg and keeps `None` (unique
    // by-name resolution with the ambiguity bail).
    let live_read_slice = agg_nodes
        .aggs_in_var(to)
        .find(|a| !a.is_synthetic && a.name == to && a.reads_var(from))
        .map(|a| a.source_read_slice(from));
    let body_ctx = crate::ltm_augment::ReducerBodyCtx {
        body_text: &classified.body_text,
        live_source: from,
        arrayed_dep_dims: &arrayed_dep_dims,
        model_deps: &model_deps,
        row_dim_names: &row_dim_names,
        dims_ctx: Some(project_dimensions_context(db, project)),
        live_read_slice,
    };

    // Compute the cartesian product of all source dimensions to get
    // per-element subscripts. For a single dimension, this is just the
    // element names. For multi-dimensional sources (e.g., x[Region,Age]),
    // this produces tuples like "nyc,adult", "nyc,child", etc.
    let dim_element_lists: Vec<Vec<String>> = from_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();

    // The T3 read-slice derivation (see the rustdoc). The
    // ARRAYED-owner broadcast slice (scalar `result_dims`, arrayed `to`) is
    // already handled by the broadcast-reduce branch near the top of this
    // function, so the agg reaching here is either an aligned partial reduce
    // (Iterated-armed `read_slice_rows` path) or, on a SCALAR owner, the
    // scalar-result slice admission. A `vb_agg` whose gate decision declines
    // (the trivial full-extent slice) falls through to the pre-T3 cartesian
    // derivation byte-identically. (The GH #764 non-aligned shapes no longer
    // appear here: since T4 they mint synthetic aggs, so no variable-backed
    // agg exists for them.)
    // The ARRAYED-owner scalar-result Pinned/subset broadcast slice
    // (`share[Region] = SUM(pop[nyc,*])`) does NOT reach here: it is handled
    // by the broadcast-reduce branch near the top of this function (which
    // returns before the `result_axis_names` derivation). A `vb_agg` the gate
    // declines (the trivial full-extent slice), or one whose `read_slice_rows`
    // declines (a stale arity/remap invariant, or a source whose declared
    // dims repeat a result axis -- the live GH #767 `matrix[D1,D1]` shape),
    // falls through to the conservative cartesian landing below rather than
    // emitting mis-slotted scores (the GH #778/#785 loud skip is strictly
    // safer there than a cartesian whose projection is ambiguous). Mirrors
    // `emit_source_to_agg_link_scores`' fallback.
    if let Some(vb_agg) = agg_nodes
        .aggs_in_var(to)
        .find(|a| !a.is_synthetic && a.name == to && a.reads_var(from))
        && crate::ltm_agg::variable_backed_reduce_agg(agg_nodes, from, to, to_dims).is_some()
        && let Some(rows) = read_slice_rows(
            vb_agg.source_read_slice(from),
            &dim_element_lists,
            project_dimensions_context(db, project),
        )
    {
        let mut cross_vars = Vec::with_capacity(rows.len());
        for ReadSliceRow {
            row,
            slot,
            coreduced,
        } in &rows
        {
            // Equation text uses qualified `dim·element` references
            // (direct LoadPrev, no helper auxes); names keep the
            // bare element form (the user-facing / discovery-parsed
            // identity) -- both exactly as the cartesian branches
            // below.
            let qualified_row = crate::ltm_augment::qualify_element_csv(row, from_dims);
            let qualified_coreduced: Vec<String> = coreduced
                .iter()
                .map(|e| crate::ltm_augment::qualify_element_csv(e, from_dims))
                .collect();
            let (var_name, equation) = if slot.is_empty() {
                // Scalar result slot: the scalar-owner admission
                // (`total = SUM(pop[nyc,*])`) -- the bare `to` name
                // IS the slot.
                (
                    format!(
                        "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}",
                        from, row, to
                    ),
                    crate::ltm_augment::generate_element_to_scalar_equation(
                        from,
                        to,
                        &qualified_row,
                        &qualified_coreduced,
                        &classified.kind,
                        classified.name,
                        classified.is_bare,
                        Some(&body_ctx),
                    ),
                )
            } else {
                // Aligned partial reduce: the slot names a complete
                // `to` element (gate invariant), in `to`-dim order.
                (
                    format!(
                        "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
                        from, row, to, slot
                    ),
                    crate::ltm_augment::generate_element_to_reduced_equation(
                        from,
                        to,
                        &qualified_row,
                        &crate::ltm_augment::qualify_element_csv(slot, to_dims),
                        &qualified_coreduced,
                        &classified.kind,
                        classified.name,
                        classified.is_bare,
                        Some(&body_ctx),
                    ),
                )
            };
            cross_vars.push(LtmSyntheticVar {
                name: var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![], // scalar -- one variable per read row
                // bracketed name -> routed direct by `assemble_module`.
                compile_directly: false,
            });
        }
        return Some(cross_vars);
    }

    // GH #778/#785: a DEGENERATE SQUARE-SOURCE reducer (`from`'s dims repeat a
    // dimension that survives as a result axis -- `cube[D1,D1,*] -> x[D1]`)
    // reaches HERE post-decline: its agg is no longer minted
    // (`result_dims_has_repeated_dim`), so the agg branch above did not handle
    // it, and it falls into the conservative cartesian partial-reduce branch
    // below. That branch projects each source tuple onto the result axes
    // through a `from_pos` name->position map, which collapses the duplicated
    // dim to ONE position (last-match) -- an ambiguous projection: for the
    // iterated-diagonal spelling (`x[D1] = SUM(cube[D1,D1,*])`) it would
    // additionally emit confident per-`(row, result)` scores for OFF-DIAGONAL
    // source rows the executed A2A simulation never reads, and for the
    // repeated-dim-owner (`x[D1,D1] = ...`) and whole-extent-broadcast
    // (`SUM(cube[*,*,*])`) spellings the per-element attribution is equally
    // undisambiguable even though the simulation reads every row. The element
    // graph routes this same edge as the conservative full cross-product (a
    // sound superset), so closing the score side with the GH #758 loud skip
    // keeps edges and scores in lockstep: no link-score variable, the edge
    // recorded unscoreable so loops through it are dropped, one clear
    // diagnostic instead of plausible-looking wrong numbers.
    //
    // Placed AFTER the agg branch deliberately: a repeated-dim SOURCE whose
    // result is NOT a duplicated dim (`growth[D1] = SUM(matrix[*, D1] * ...)`
    // over `matrix[D1,D1]`, slice `[Reduced, Iterated]`, `result_dims = [D1]`)
    // IS hoisted and correctly scored by the agg branch (GH #767) -- it returns
    // before reaching here, so this skip fires ONLY for the genuinely
    // un-hoisted square shape whose duplicated dim survives as a result axis.
    if !result_axis_names.is_empty() {
        let from_names: Vec<&str> = from_dims.iter().map(|d| d.name()).collect();
        let result_axis_is_duplicated = result_axis_names
            .iter()
            .any(|n| from_names.iter().filter(|fn_| **fn_ == n.as_str()).count() > 1);
        if result_axis_is_duplicated {
            if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                emit_unscoreable_duplicated_dim_source_warning(db, model, from, to);
            }
            return Some(vec![]);
        }
    }

    // GH #791: the I1-DECLINED STRICT-SLICE family. We are at the legacy
    // cartesian derivation because no usable variable-backed agg was minted for
    // this edge (the agg branch above did not fire). The cartesian projection
    // ranges over the FULL `from` extent by declared dimension positions, which
    // is sound ONLY when the reducer reads the full extent of `from`. When
    // `to`'s reducer reads a STRICT slice of `from` -- a `Pinned` element or a
    // subset-`Reduced` axis, as in the multi-source mismatched-co-source shape
    // `share[Region] = SUM(pop[nyc,*] * w[*])` (`pop`'s slice
    // `[Pinned(nyc), Reduced]` declines the I1 acceptance, so no agg is minted)
    // -- the projection invents scores for `from`'s UNREAD rows
    // (`pop[boston,*]`) and mis-divides the read rows. Closed with the GH
    // #758/#780 loud skip: no link-score variable, the edge recorded so loops
    // through it drop. The verdict re-derives `from`'s read slice via the SAME
    // per-axis classifier the hoisting path uses for Scalar/A2A owners (so the
    // two agree axis-for-axis there). The shared helper ALSO declines the
    // GH #792 per-element-owner family, which can only reach this cartesian
    // arm through an EXCEPT default expression (`classify_reducer` needs a
    // single expression); declining it here keeps that spelling consistent
    // with the per-shape fallthrough below. A NOT-DESCRIBABLE read (the
    // dynamic-index `SUM(pop[idx,*])` carve-out and declined mappings) keeps
    // the conservative cartesian -- its documented intended behavior -- as
    // does a genuine full-extent read on a Scalar/A2A owner (the aligned
    // `SUM(matrix[D1,*])` diagonal a feeder decline can strand here).
    if decline_unhoisted_reducer_edge(db, model, project, from, to, unscoreable_edges) {
        return Some(vec![]);
    }

    let source_elements = cartesian_subscripts(&dim_element_lists);

    if result_axis_names.is_empty() {
        // Full reduce: one scalar link score per source element.
        // Equation text uses the qualified `dim·element` form so its
        // PREVIOUS(source[elem]) references compile to direct LoadPrevs
        // instead of synthesizing one helper aux per occurrence; the
        // variable NAME keeps the bare element form (it is the
        // user-facing / discovery-parsed identity).
        let qualified_elements: Vec<String> = source_elements
            .iter()
            .map(|e| crate::ltm_augment::qualify_element_csv(e, from_dims))
            .collect();
        let mut cross_vars = Vec::with_capacity(source_elements.len());
        for (element, qualified_element) in source_elements.iter().zip(&qualified_elements) {
            let var_name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}",
                from, element, to
            );
            let equation = crate::ltm_augment::generate_element_to_scalar_equation(
                from,
                to,
                qualified_element,
                &qualified_elements,
                &classified.kind,
                classified.name,
                classified.is_bare,
                Some(&body_ctx),
            );
            cross_vars.push(LtmSyntheticVar {
                name: var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![], // scalar -- one variable per element
                // bracketed name -> routed direct by `assemble_module`'s
                // element-subscript check; the flag is irrelevant here.
                compile_directly: false,
            });
        }
        return Some(cross_vars);
    }

    // Partial reduce: project each source element tuple onto the
    // surviving axes (in target-dim order) to get the result element,
    // then group source elements by result element so each group is the
    // `matrix[d1,*]` slice the reducer combines for that row. The MEAN
    // divisor and the nonlinear expansion both operate over that slice
    // only (other rows are irrelevant to `agg[d1]`).
    let from_pos: HashMap<&str, usize> = from_dims
        .iter()
        .enumerate()
        .map(|(i, d)| (d.name(), i))
        .collect();
    // For each surviving target dim, the index into a split source
    // element tuple where that dim's element name lives. Built from the
    // membership check above, so every name resolves.
    let result_positions: Vec<usize> = result_axis_names
        .iter()
        .map(|n| from_pos[n.as_str()])
        .collect();
    let project_to_result = |source_elem: &str| -> String {
        let parts: Vec<&str> = source_elem.split(',').collect();
        result_positions
            .iter()
            .map(|&p| parts[p])
            .collect::<Vec<_>>()
            .join(",")
    };

    // result_element -> the source element tuples that share it, in
    // row-major source order (deterministic).
    let mut slices: HashMap<String, Vec<String>> = HashMap::new();
    for se in &source_elements {
        slices
            .entry(project_to_result(se))
            .or_default()
            .push(se.clone());
    }

    let mut cross_vars = Vec::with_capacity(source_elements.len());
    for source_elem in &source_elements {
        let result_elem = project_to_result(source_elem);
        let coreduced = &slices[&result_elem];
        let var_name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
            from, source_elem, to, result_elem
        );
        // Equation text uses qualified `dim·element` references (direct
        // LoadPrev, no helper auxes); the name keeps the bare form.
        let qualified_coreduced: Vec<String> = coreduced
            .iter()
            .map(|e| crate::ltm_augment::qualify_element_csv(e, from_dims))
            .collect();
        let equation = crate::ltm_augment::generate_element_to_reduced_equation(
            from,
            to,
            &crate::ltm_augment::qualify_element_csv(source_elem, from_dims),
            &crate::ltm_augment::qualify_element_csv(&result_elem, to_dims),
            &qualified_coreduced,
            &classified.kind,
            classified.name,
            classified.is_bare,
            Some(&body_ctx),
        );
        cross_vars.push(LtmSyntheticVar {
            name: var_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![], // scalar -- one variable per (reduced-elem, result-elem)
            // bracketed name -> routed direct by `assemble_module`.
            compile_directly: false,
        });
    }
    Some(cross_vars)
}

/// Emit the per-(read-row, full-target-element) link scores for an
/// ARRAYED-owner scalar-result BROADCAST reduce (GH #777):
/// `share[Region] = SUM(pop[nyc,*])`. The reducer reads a Pinned/subset
/// slice (`pop[nyc,*]`) with NO `Iterated` axis, collapsing it to ONE scalar
/// value that broadcasts over every `share[e]`. Every target slot reads the
/// SAME slice, so for each read row (`pop[nyc,p]`, `pop[nyc,q]`) and each
/// FULL target element `e` we emit the scalar variable
/// `$⁚ltm⁚link_score⁚{from}[{row}]→{to}[{e}]` -- the EXISTING per-(row, slot)
/// grammar `loop_link_score_ref` and discovery's `parse_link_offsets`
/// already resolve.
///
/// The equation is the per-row reducer partial with the target reference
/// pinned to `e`: [`crate::ltm_augment::generate_element_to_reduced_equation`]
/// with `result_element = e` and the co-reduced slice the FULL set of read
/// rows (every read row contributes to every target slot, so the divisor /
/// nonlinear expansion is over the whole slice). For an A2A target the body
/// is identical per slot; only the target reference (`share[e]`) and the
/// per-row source reference differ -- the same builder the aligned
/// partial-reduce arm uses, just with `e` ranging over `to`'s entire element
/// set rather than the source's projection.
///
/// The read rows are independent of `to`'s dims, so the RELATED-dim spelling
/// (`share[Region]`, `Region` a source dim) and the DISJOINT-dim spelling
/// (`share[D9]`, `D9` not a source dim) emit identically. SIZE (constant
/// reducer) skips with `Some(vec![])`. A `read_slice_rows` decline returns
/// `None` (the `?`), propagating out of `try_cross_dimensional_link_scores`'
/// broadcast branch so the dispatcher's later landings apply -- defense only:
/// the decline is unreachable for a slice the gate admits here (a
/// Pinned/Reduced-only slice has no `Iterated` remap to fail, and arity is
/// invariant I2), so the broadcast edges and scores cannot actually diverge.
#[allow(clippy::too_many_arguments)] // threads salsa keys + emission context
fn emit_broadcast_reduce_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    from_dims: &[crate::dimensions::Dimension],
    to_dims: &[crate::dimensions::Dimension],
    vb_agg: &crate::ltm_agg::AggNode,
    model: SourceModel,
    project: SourceProject,
) -> Option<Vec<LtmSyntheticVar>> {
    let to_var = reconstruct_single_variable(db, model, project, to)?;
    let classified = crate::ltm_augment::classify_reducer(&to_var, from)?;
    if classified.kind == crate::ltm_augment::ReducerKind::Constant {
        // SIZE is constant; link score is always 0. Skip entirely.
        return Some(vec![]);
    }

    // The body-aware partial context (GH #744): the reducer body may apply a
    // coefficient to the source (`SUM(pop[nyc,*] * w[nyc,*])`), which the
    // changed-first partial must account for. Mirrors
    // `try_cross_dimensional_link_scores`' setup.
    let (arrayed_dep_dims, model_deps) =
        reducer_body_ctx_parts(db, source_vars, project, &classified.body_text);
    let row_dim_names: Vec<String> = from_dims.iter().map(|d| d.name().to_string()).collect();
    let body_ctx = crate::ltm_augment::ReducerBodyCtx {
        body_text: &classified.body_text,
        live_source: from,
        arrayed_dep_dims: &arrayed_dep_dims,
        model_deps: &model_deps,
        row_dim_names: &row_dim_names,
        dims_ctx: Some(project_dimensions_context(db, project)),
        live_read_slice: Some(vb_agg.source_read_slice(from)),
    };

    // The read rows the slice covers (`pop[nyc,p]`, `pop[nyc,q]`). With no
    // `Iterated` axis every row's `slot` is empty; the co-reduced set is the
    // whole row list. Derive them from the SAME `read_slice_rows` the element
    // graph uses, so the per-(row, e) names match the broadcast edges exactly.
    let dim_element_lists: Vec<Vec<String>> = from_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    let rows = read_slice_rows(
        vb_agg.source_read_slice(from),
        &dim_element_lists,
        project_dimensions_context(db, project),
    )?;
    // Every read row contributes to every target slot -- the co-reduced slice
    // is the full row list (qualified once).
    let all_rows: Vec<String> = rows.iter().map(|r| r.row.clone()).collect();
    let qualified_all_rows: Vec<String> = all_rows
        .iter()
        .map(|r| crate::ltm_augment::qualify_element_csv(r, from_dims))
        .collect();

    let to_dim_element_lists: Vec<Vec<String>> = to_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    let target_elements = cartesian_subscripts(&to_dim_element_lists);

    let mut cross_vars = Vec::with_capacity(rows.len() * target_elements.len());
    for ReadSliceRow { row, .. } in &rows {
        let qualified_row = crate::ltm_augment::qualify_element_csv(row, from_dims);
        for element in &target_elements {
            let var_name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
                from, row, to, element
            );
            let equation = crate::ltm_augment::generate_element_to_reduced_equation(
                from,
                to,
                &qualified_row,
                &crate::ltm_augment::qualify_element_csv(element, to_dims),
                &qualified_all_rows,
                &classified.kind,
                classified.name,
                classified.is_bare,
                Some(&body_ctx),
            );
            cross_vars.push(LtmSyntheticVar {
                name: var_name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![], // scalar -- one variable per (read-row, target-element)
                // bracketed name -> routed direct by `assemble_module`.
                compile_directly: false,
            });
        }
    }
    Some(cross_vars)
}

/// Generate per-target-element link score variables for a
/// scalar-source -> arrayed-target edge, or return `None` if the edge
/// is not of that shape.
///
/// The mirror of [`try_cross_dimensional_link_scores`]: where that one
/// fires for (arrayed source, scalar target) reducers and emits one
/// scalar `LtmSyntheticVar` per *source* element named
/// `$⁚ltm⁚link_score⁚{from}[{elem}]→{to}`, this one fires for (scalar
/// source, arrayed target) edges and emits one scalar `LtmSyntheticVar`
/// per *target* element named `$⁚ltm⁚link_score⁚{from}→{to}[{elem}]`,
/// `dimensions: vec![]`.
///
/// Why not a single Bare-A2A var with `dimensions = [target_dims]`:
/// that form is undiscoverable. `parse_link_offsets`'s
/// `expand_a2a_link_offsets` subscripts *both* `from` and `to` over
/// `target_dims`, inventing a `from[elem]` node -- but `from` is scalar,
/// so the invented node doesn't match the unsubscripted `from` node
/// that other edges (e.g. an arrayed->scalar reducer feeding `from`)
/// produce, and a loop through `from` is unreachable in the search
/// graph. The per-target-element scalar name parses via the `[`-in-`to`
/// single-passthrough branch to the edge `(from, to[elem])` with no
/// parser change, and `generate_loop_score_equation` references the
/// per-element scalar variable directly.
///
/// The per-element equation is the partial of `to[elem]`'s equation
/// w.r.t. `from` live (everything else PREVIOUS), wrapped in the
/// standard link-score guard form with the target reference pinned to
/// `elem`. For an `Equation::ApplyToAll` target the body is the same
/// for every element (with the element pinned on the `to` side and on
/// the target's arrayed deps); for an `Equation::Arrayed` target it is
/// that element's own slot expression (which already carries explicit
/// subscripts). See [`crate::ltm_augment::generate_scalar_to_element_equation`].
///
/// Returns `None` for scalar-to-scalar, A2A (same-dimension),
/// arrayed-to-scalar, module-involved, and any edge where the target
/// has no usable AST (so per-element equations can't be derived) -- in
/// those cases the caller falls back to its existing emission path.
/// Returns `Some(vec![])` (GH #780) when a per-element partial dooms with a
/// `PartialEquationError`: the warning is accumulated and the edge recorded
/// in `unscoreable_edges` so dependent loop scores drop, and the caller must
/// NOT fall back to `emit_per_shape_link_scores`.
///
/// **Scalar-feeder-of-a-variable-backed-reduce (GH #790)**: when `to` IS a
/// variable-backed reduce reading `from` as a SCALAR FEEDER
/// (`growth[D1] = SUM(matrix[D1,*] * scale)`), the per-target-element
/// derivation below would emit `scale → growth[a]` / `[b]` partials that
/// reference the lagged wildcard slice (`SUM(PREVIOUS(matrix[d1·a,*]) *
/// scale)`, a GH #541-class uncompilable fragment) and degrade every loop
/// through the feeder to a warned constant-0 stub. Such an edge is routed
/// FIRST to the SAME changed-last scalar-feeder convention the synthetic-agg
/// arm uses for the SUBEXPRESSION spelling -- ONE Bare A2A score
/// `$⁚ltm⁚link_score⁚scale→growth` over the agg's `result_dims` (or over the
/// OWNER's dims for the GH #777 broadcast slice, whose `result_dims` are
/// empty -- `share[D9] = SUM(matrix[a,*] * scale)` -- since the single
/// scalar reducer value feeds every `to[e]` identically),
/// [`crate::ltm_augment::generate_scalar_feeder_to_agg_equation`] -- so the
/// two spellings of the same dataflow score identically. Gated on the shared
/// [`crate::ltm_agg::scalar_feeder_of_variable_backed_agg`] decision, so the
/// emitted name is exactly the hop the per-slot loops reference.
#[allow(clippy::too_many_arguments)] // threads salsa keys + agg nodes + emission context
pub(super) fn try_scalar_to_arrayed_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    agg_nodes: &crate::ltm_agg::AggNodesResult,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> Option<Vec<LtmSyntheticVar>> {
    // Source must be a scalar, non-module variable.
    let from_sv = source_vars.get(from)?;
    if from_sv.kind(db) == SourceVariableKind::Module {
        return None;
    }
    if !variable_dimensions(db, *from_sv, project).is_empty() {
        return None;
    }

    // Target must be an arrayed, non-module variable.
    let to_sv = source_vars.get(to)?;
    if to_sv.kind(db) == SourceVariableKind::Module {
        return None;
    }
    let to_dims = variable_dimensions(db, *to_sv, project).clone();
    if to_dims.is_empty() {
        return None;
    }

    // GH #790: a scalar feeder of a variable-backed whole-RHS reduce. The
    // per-target-element machinery below cannot express this shape (its
    // partial freezes the reducer's wildcard slice as a lagged whole-array
    // read that fails fragment compile). Route it to the changed-last
    // scalar-feeder convention -- ONE Bare A2A score dimensioned over the
    // agg's `result_dims` -- identical to the synthetic-agg arm's handling of
    // the subexpression spelling.
    if let Some(agg) =
        crate::ltm_agg::scalar_feeder_of_variable_backed_agg(agg_nodes, from, to, &to_dims)
    {
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
            from, agg.name
        );
        match crate::ltm_augment::generate_scalar_feeder_to_agg_equation(
            from,
            &agg.name,
            &agg.equation_text,
        ) {
            Ok(text) => {
                // The score is ALWAYS arrayed here (`to_dims` is non-empty in
                // this function). Two sub-shapes:
                //  - ALIGNED reduce (`result_dims` == the owner's dims): the
                //    score is A2A over `result_dims`.
                //  - BROADCAST reduce (GH #777: `result_dims` empty, owner
                //    arrayed -- `share[D9] = SUM(matrix[a,*] * scale)`): the
                //    single scalar reducer value feeds every `to[e]`
                //    identically, so the score is A2A over the OWNER's dims.
                //    Emitting `Equation::Scalar` here would reference the
                //    bare multi-slot owner in a scalar fragment -- an
                //    assembly failure whose loops stub to warned constant 0
                //    (the exact GH #790 defect, one shape over).
                // The changed-last text is ApplyToAll-compatible in both:
                // the bare owner reference element-resolves inside its own
                // A2A context, and the frozen reducer body is either
                // iterated over `result_dims` (aligned) or scalar-valued
                // (broadcast -- a Pinned/subset slice broadcasts cleanly).
                let equation_dims: Vec<String> = if agg.result_dims.is_empty() {
                    // Map the owner's canonical dim names back to their
                    // datamodel casing for correct equation parsing (the
                    // same mapping `link_score_dimensions` applies).
                    to_dims
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
                        .collect()
                } else {
                    agg.result_dims.clone()
                };
                return Some(vec![LtmSyntheticVar {
                    name,
                    equation: datamodel::Equation::ApplyToAll(equation_dims.clone(), text),
                    dimensions: equation_dims,
                    // The non-empty `dimensions` route this through the A2A
                    // arm of `compile_ltm_synthetic_fragment`, which compiles
                    // `equation` verbatim -- but set the direct flag anyway:
                    // the (from, to)-keyed salsa path would re-derive a
                    // DIFFERENT (changed-first, Bare-shaped) equation than
                    // this var carries, so falling into it under any future
                    // dims-handling change would be a silent divergence.
                    compile_directly: true,
                }]);
            }
            Err(err) => {
                // GH #780 contract: the scalar feeder's single score IS this
                // edge's entire emission; a doom leaves every loop hop through
                // `(from, to)` referencing a missing name. Record + warn on
                // first insert only so dependent loop scores drop instead of
                // stubbing, and a pinned-pass re-visit does not duplicate the
                // warning.
                if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                    emit_ltm_partial_equation_warning(db, model, &name, &err);
                }
                return Some(vec![]);
            }
        }
    }

    // A re-visit of an already-recorded doomed edge (the pinned pass dedups
    // only edges that EMITTED a var, so a doomed edge is re-visited): the
    // per-element generator would re-doom deterministically and duplicate
    // the warning (it fires inside `build_var`, before the recording).
    // Early-return the loud-decline shape instead -- the #758
    // warn-once-per-edge convention.
    if unscoreable_edges.contains(&(from.to_string(), to.to_string())) {
        return Some(vec![]);
    }

    let to_var = reconstruct_single_variable(db, model, project, to)?;
    // Without a lowered AST we can't derive per-element equations.
    // Decline and let the caller's existing path handle the (degenerate)
    // failed-to-lower target.
    let ast = to_var.ast()?;

    // The per-element equation text and dependency-set source differ
    // by AST variant:
    //   - ApplyToAll: one shared body; deps from the whole AST.
    //   - Arrayed:    per-element slot text (or the default slot);
    //                 deps from that slot's expression.
    // In both cases dependency classification is given the target's
    // AST dimensions so explicit element-name subscripts (e.g. `[NYC]`)
    // are recognized as dimension references, not variables.
    use crate::ast::Ast;
    let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
        Ast::Scalar(_) => &[],
        Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
    };

    // Which target deps must be pinned to the element in the per-element
    // scalar equation: the arrayed deps that share a dimension with the
    // target. (Scalar deps stay bare; the target self-reference is
    // pinned implicitly via the subscripted `to[elem]` in the guard
    // form built by `generate_scalar_to_element_equation`.)
    let deps_to_subscript = |deps: &HashSet<Ident<Canonical>>| -> HashSet<Ident<Canonical>> {
        deps.iter()
            .filter(|d| {
                source_vars
                    .get(d.as_str())
                    .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
                    .map(|sv| {
                        let dd = variable_dimensions(db, *sv, project);
                        !dd.is_empty()
                            && dd
                                .iter()
                                .any(|x| to_dims.iter().any(|td| td.name() == x.name()))
                    })
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    };

    let dim_element_lists: Vec<Vec<String>> = to_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    let elements = cartesian_subscripts(&dim_element_lists);

    // Build one `LtmSyntheticVar` for `element` from its equation text and
    // that text's dependency set. The element name is the only part of the
    // generated equation/name that varies between elements.
    // Returns `None` (after surfacing a `Warning`) when the per-element
    // ceteris-paribus partial fails to parse (GH #311) -- that element's link
    // score is skipped rather than emitted with a silently non-ceteris-paribus
    // equation. The `elem_text.is_empty()` zero is a legitimate no-slot value,
    // not a parse failure.
    let build_var = |element: &str,
                     elem_text: &str,
                     elem_deps: &HashSet<Ident<Canonical>>,
                     deps_to_sub: &HashSet<Ident<Canonical>>|
     -> Option<LtmSyntheticVar> {
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
            from, to, element
        );
        // Test-only seam (GH #780): honour a forced-`PartialEquationError`
        // edge here too, so the direct (non-salsa) per-element generator
        // call site is covered by the same override the shaped query uses.
        #[cfg(test)]
        if super::compile::force_partial_equation_error(from, to) {
            let err = crate::ltm_augment::PartialEquationError::new(
                "<test-forced per-element partial-equation failure (GH #780)>",
            );
            emit_ltm_partial_equation_warning(db, model, &name, &err);
            return None;
        }
        let equation = if elem_text.is_empty() {
            "0".to_string()
        } else {
            match crate::ltm_augment::generate_scalar_to_element_equation(
                from,
                to,
                // Equation text uses the qualified `dim·element` form (direct
                // LoadPrev, no helper auxes); the NAME below keeps the bare form.
                &crate::ltm_augment::qualify_element_csv(element, &to_dims),
                elem_text,
                elem_deps,
                deps_to_sub,
                // A true scalar source: the bare `quote_ident(from)`
                // denominator is correct, and there is no source subscript
                // to pin in the partial body.
                None,
                &[],
                Some(project_dimensions_context(db, project)),
            ) {
                Ok(eqn) => eqn,
                Err(err) => {
                    emit_ltm_partial_equation_warning(db, model, &name, &err);
                    return None;
                }
            }
        };
        Some(LtmSyntheticVar {
            name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![], // scalar -- one variable per target element
            // bracketed name -> routed direct by `assemble_module`.
            compile_directly: false,
        })
    };

    // GH #780: `build_var` returns `None` only on a true per-element
    // `PartialEquationError` (the empty-slot case returns `Some("0")`). One
    // doomed element dooms the whole edge -- a loop hop through it resolves
    // (`loop_link_score_ref`) to that element's name, which was never
    // emitted, so the loop would otherwise stub to a warned constant 0.
    // Record the edge in `unscoreable_edges` (the per-element `Warning`
    // already fired in `build_var`) and return `Some(vec![])` so dependent
    // loop scores are DROPPED and the dispatcher does NOT fall through to a
    // wrong-shaped per-shape stand-in -- the same loud-skip contract the
    // disjoint and dim-incompatible classes follow.
    let mut cross_vars = Vec::with_capacity(elements.len());
    match ast {
        // ApplyToAll: one shared body for every element, so its text, its
        // dependency set, and the subset to element-pin are all
        // element-invariant -- compute them once, outside the loop.
        Ast::ApplyToAll(_, expr) => {
            let elem_text = crate::patch::expr2_to_string(expr);
            let elem_deps = crate::variable::identifier_set(ast, target_ast_dims, None);
            let deps_to_sub = deps_to_subscript(&elem_deps);
            for element in &elements {
                match build_var(element, &elem_text, &elem_deps, &deps_to_sub) {
                    Some(v) => cross_vars.push(v),
                    None => {
                        unscoreable_edges.insert((from.to_string(), to.to_string()));
                        return Some(vec![]);
                    }
                }
            }
        }
        // Arrayed: each element has its own slot expression (or the default
        // slot), so the body and its dependency set genuinely differ per
        // element and must be recomputed inside the loop.
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            for element in &elements {
                let canonical_elem = crate::common::CanonicalElementName::from_raw(element);
                let slot = per_elem.get(&canonical_elem).or(default_expr.as_ref());
                let (elem_text, elem_deps): (String, HashSet<Ident<Canonical>>) = match slot {
                    Some(expr) => (
                        crate::patch::expr2_to_string(expr),
                        crate::variable::identifier_set(
                            &Ast::Scalar(expr.clone()),
                            target_ast_dims,
                            None,
                        ),
                    ),
                    // No slot and no default: the target has a hole at
                    // this element. A zero equation is the right
                    // link-score value (no sensitivity), matching the
                    // historical placeholder behaviour for un-derivable
                    // partials.
                    None => (String::new(), HashSet::new()),
                };
                let deps_to_sub = deps_to_subscript(&elem_deps);
                match build_var(element, &elem_text, &elem_deps, &deps_to_sub) {
                    Some(v) => cross_vars.push(v),
                    None => {
                        unscoreable_edges.insert((from.to_string(), to.to_string()));
                        return Some(vec![]);
                    }
                }
            }
        }
        Ast::Scalar(_) => unreachable!("target is arrayed"),
    }
    Some(cross_vars)
}

/// Accumulate the AC3.4 `Warning` for a disjoint-dim arrayed -> arrayed
/// edge that has no scoreable per-element derivation: the target's
/// per-element equations reference the source via something other than
/// literal element subscripts -- a genuinely dynamic index (which target
/// slots depend on which source elements can't be decided at compile
/// time), or an un-hoisted reducer read (a statically-describable slice
/// for which no disjoint-dim emitter exists; note the GH #514
/// reclassification labels both site shapes `DynamicIndex`, so they are
/// indistinguishable here and the message names both). The edge gets *no*
/// link-score variable, and -- like the GH #758 dim-incompatible class --
/// the caller records it in `unscoreable_edges` so loop scores traversing
/// it are dropped (their product could only be a guaranteed-zero stub).
/// Sub-model *pathway* scores referencing the missing name still get the
/// fragment compiler's zero-contribution stub-dep fallback. Both are far
/// less misleading than the scalarized stand-in the pre-#510 path
/// produced.
pub(super) fn emit_unscoreable_disjoint_edge_warning(
    db: &dyn Db,
    model: SourceModel,
    from: &str,
    to: &str,
) {
    use salsa::Accumulator;
    let msg = format!(
        "LTM link score for edge {from} -> {to} could not be computed: {to} is a \
         per-element-equation arrayed variable whose equations reference {from} via a \
         dynamic index or an un-hoisted reducer read, neither of which has a \
         per-element derivation here; this edge will have no link-score variable and \
         feedback loops through it will not be scored"
    );
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: None,
        error: DiagnosticError::Assembly(msg),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// Accumulate the GH #758 `Warning` for an arrayed -> arrayed conservative
/// edge whose endpoint dimensions do not correspond (so
/// [`link_score_dimensions`] returned empty). The conservative per-shape
/// link score for such an edge has no compilable shape: the scalar form's
/// guard references the multi-slot target (and the partial body the arrayed
/// deps) in scalar context -- a fragment-compile failure stubbed to a
/// constant 0 -- while an arrayed (target-dims) form would compile but
/// mis-attribute: the element graph expands these edges as the conservative
/// CROSS-PRODUCT, so off-diagonal loop links would read the per-slot
/// DIAGONAL partial of the wrong slot (silent wrong numbers; the PR #761
/// Bare-site gate exists precisely to prevent that). The edge therefore
/// gets *no* link-score variable and the caller drops loop scores through
/// it -- one clear diagnostic instead of a cascade of per-fragment
/// warnings over guaranteed-zero stubs.
pub(super) fn emit_unscoreable_conservative_edge_warning(
    db: &dyn Db,
    model: SourceModel,
    from: &str,
    to: &str,
) {
    use salsa::Accumulator;
    let msg = format!(
        "LTM link score for edge {from} -> {to} could not be computed: both variables \
         are arrayed but their dimensions do not correspond (e.g. an element-mapped or \
         unmapped dimension pair), so the conservative score has no compilable shape; \
         this edge will have no link-score variable and feedback loops through it will \
         not be scored"
    );
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: None,
        error: DiagnosticError::Assembly(msg),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// Accumulate the GH #778/#785 `Warning` for a declined DEGENERATE
/// SQUARE-SOURCE reducer edge: `from`'s declared dims repeat a dimension
/// that survives as a result axis, so the cartesian partial-reduce
/// projection cannot tell the two occurrences apart -- a result coordinate
/// is ambiguous between them. The skip covers every spelling of the
/// predicate: the iterated-diagonal read (`x[D1] = SUM(cube[D1,D1,*])`,
/// where the projection would additionally score off-diagonal rows the
/// executed simulation never reads), the repeated-dim OWNER
/// (`x[D1,D1] = SUM(cube[D1,D1,*])`, where the simulation reads the full
/// square but the projection is still ambiguous), and the whole-extent
/// broadcast (`x[D1] = SUM(cube[*,*,*])` over `cube[D1,D1,D2]`). The
/// hoistable spellings' minting is declined upstream
/// (`result_dims_has_repeated_dim`); this is the remaining cartesian-branch
/// landing, closed with the same loud-skip discipline as the other
/// unscoreable classes (no link-score variable, the edge recorded in
/// `unscoreable_edges` so loops through it are dropped).
pub(super) fn emit_unscoreable_duplicated_dim_source_warning(
    db: &dyn Db,
    model: SourceModel,
    from: &str,
    to: &str,
) {
    use salsa::Accumulator;
    let msg = format!(
        "LTM link score for edge {from} -> {to} could not be computed: {from}'s \
         declared dimensions repeat a dimension that also survives as a result \
         axis of the reducer reading it (a square-source shape like \
         {from}[D,D,...]), so a per-element score cannot disambiguate which \
         occurrence of the repeated dimension a result coordinate refers to; \
         this edge will have no link-score variable and feedback loops through \
         it will not be scored"
    );
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: None,
        error: DiagnosticError::Assembly(msg),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// Accumulate the GH #791 `Warning` for an I1-declined NOT-hoisted reducer
/// whose read of `from` is a STRICT slice (a `Pinned` element or a
/// subset-`Reduced` axis), reached at the legacy cartesian partial-/full-reduce
/// derivation. The cartesian projection ranges over EVERY `from` element by its
/// declared dimension positions, so for a strict slice it both invents scores
/// for UNREAD rows (`pop[boston,*]` though the reducer reads only `pop[nyc,*]`)
/// and mis-divides the read rows (the un-pinnable mismatched-arity body dooms
/// the changed-first partial to the |dz/dz| = 1 fallback) -- a SILENT wrong
/// number on the link surface. Closed with the same loud-skip discipline as the
/// other unscoreable classes (no link-score variable, the edge recorded in
/// `unscoreable_edges` so loops through it are dropped). The drop is at EDGE
/// granularity, a deliberate conservatism: a per-site derivation exists in
/// principle for the mixed FixedIndex-site + strict-reducer-read shape, but
/// no current emitter implements it for the no-agg leg.
///
/// `slice` is the representative strict read carried by
/// [`crate::ltm_agg::UnhoistedSourceRead::StrictSlice`]; the message renders
/// it (`pop[nyc,*]`) so the user sees THEIR slice, not a canned example.
pub(super) fn emit_unscoreable_strict_slice_reduce_warning(
    db: &dyn Db,
    model: SourceModel,
    from: &str,
    to: &str,
    slice: &[crate::ltm_agg::AxisRead],
) {
    use salsa::Accumulator;
    let rendered = crate::ltm_agg::render_read_slice_for_diagnostic(slice);
    let msg = format!(
        "LTM link score for edge {from} -> {to} could not be computed: the reducer in \
         {to}'s equation reads only the strict slice {from}[{rendered}] (a pinned \
         element or a subdimension subset), but no variable-backed aggregate could be \
         minted for it (a multi-source reducer whose co-source slices disagree), and \
         the whole-edge cartesian derivation that remains here would score {from}'s \
         unread rows and mis-divide the read rows -- so the edge is declined instead: \
         it will have no link-score variable and feedback loops through it will not \
         be scored"
    );
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: None,
        error: DiagnosticError::Assembly(msg),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// Accumulate the GH #792 `Warning` for a PER-ELEMENT-EQUATION
/// (`Ast::Arrayed`) owner whose slot bodies read `from` inside a reducer that
/// was not hoisted into an aggregate. A per-element owner has no whole-edge
/// derivation that can represent per-slot reducer reads: the cartesian arm
/// needs a single dt-expression, and the Bare per-shape stand-in conflates
/// all slots (it simulated to a silent ~-0.0 for strict, dim-named, and
/// full-extent slot reads alike, with downstream loops consuming the silent
/// near-zero). Closed with the same loud-skip discipline as the other
/// unscoreable classes (no link-score variable, the edge recorded in
/// `unscoreable_edges` so loops through it are dropped).
///
/// `slice` is the first statically-describable read carried by
/// [`crate::ltm_agg::UnhoistedSourceRead::PerElementReducerRead`]; when
/// present the message renders it (`pop[nyc,*]`) so the user sees their own
/// slice; a dim-named or dynamic read has no describable slice to show.
pub(super) fn emit_unscoreable_per_element_reducer_warning(
    db: &dyn Db,
    model: SourceModel,
    from: &str,
    to: &str,
    slice: Option<&[crate::ltm_agg::AxisRead]>,
) {
    use salsa::Accumulator;
    let example = slice
        .map(|s| {
            format!(
                " (e.g. the slice {from}[{}])",
                crate::ltm_agg::render_read_slice_for_diagnostic(s)
            )
        })
        .unwrap_or_default();
    let msg = format!(
        "LTM link score for edge {from} -> {to} could not be computed: {to} is defined \
         with per-element equations whose bodies read {from} inside a reducer{example} \
         that could not be hoisted into an aggregate, and no remaining derivation can \
         represent the per-slot reads (a single whole-edge stand-in score would \
         misattribute them) -- so the edge is declined instead: it will have no \
         link-score variable and feedback loops through it will not be scored"
    );
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: None,
        error: DiagnosticError::Assembly(msg),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// Accumulate the GH #788 `Warning` for an Apply-To-All target whose equation
/// contains a maximal reducer with a bare arrayed argument that overlaps the
/// target's active dimensions. LTM cannot yet represent that bare spelling:
/// a synthetic aggregate would evaluate the reducer as a whole-array scalar,
/// while ordinary feeder partials would freeze that wrong reducer value.
pub(super) fn emit_unscoreable_bare_arrayed_reducer_warning(
    db: &dyn Db,
    model: SourceModel,
    from: &str,
    to: &str,
    reducer_text: &str,
) {
    use salsa::Accumulator;
    let msg = format!(
        "LTM link score for edge {from} -> {to} could not be computed: {to}'s \
         equation contains the bare arrayed reducer argument {reducer_text}, and \
         LTM cannot yet score that spelling in an Apply-To-All target without \
         treating the reducer as a whole-array aggregate value -- so the edge is \
         declined instead: it will have no link-score variable and feedback loops \
         through it will not be scored"
    );
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: None,
        error: DiagnosticError::Assembly(msg),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// Consult the GH #791/#792 verdict (`unhoisted_reducer_source_read`,
/// salsa-cached on the interned `LtmLinkId`) for `(from, to)` and -- when it
/// is a declining verdict -- record the edge in `unscoreable_edges` and warn
/// once (the insert dedups pinned-pass / discovery re-visits). Returns `true`
/// iff the edge was declined, in which case the caller must emit NO link-score
/// variable for it.
///
/// One helper used by every dispatch point that might otherwise emit a
/// whole-edge score: the cartesian arm of `try_cross_dimensional_link_scores`,
/// the routed-agg branch of `emit_link_scores_for_edge` (GH #793), and its
/// final per-shape fallthrough. The shared verdicts are `StrictSlice` (the GH
/// #791 scalar/A2A strict family) and `PerElementReducerRead` (the GH #792
/// per-element-owner family, any reducer read). The classifier ignores reducer
/// reads already represented by a synthetic agg, so a declining verdict always
/// describes a genuinely un-hoisted residual read even when the edge also has
/// agg halves.
fn decline_unhoisted_reducer_edge(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    from: &str,
    to: &str,
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> bool {
    let link_id = LtmLinkId::new(db, from.to_string(), to.to_string());
    match crate::ltm_agg::unhoisted_reducer_source_read(db, link_id, model, project) {
        crate::ltm_agg::UnhoistedSourceRead::StrictSlice(slice) => {
            if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                emit_unscoreable_strict_slice_reduce_warning(db, model, from, to, slice);
            }
            true
        }
        crate::ltm_agg::UnhoistedSourceRead::PerElementReducerRead(slice) => {
            if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                emit_unscoreable_per_element_reducer_warning(db, model, from, to, slice.as_deref());
            }
            true
        }
        crate::ltm_agg::UnhoistedSourceRead::FullExtent
        | crate::ltm_agg::UnhoistedSourceRead::NotDescribable => false,
    }
}

fn decline_bare_arrayed_reducer_target(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    from: &str,
    to: &str,
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> bool {
    let reducer = crate::ltm_agg::unhoisted_bare_arrayed_reducer_arg(
        db,
        from.to_string(),
        to.to_string(),
        model,
        project,
    );
    let Some(reducer) = reducer.as_ref() else {
        return false;
    };
    if unscoreable_edges.insert((from.to_string(), to.to_string())) {
        emit_unscoreable_bare_arrayed_reducer_warning(db, model, from, to, reducer);
    }
    true
}

/// Surface a `Warning` for a ceteris-paribus partial-equation parse failure
/// (GH #311), naming the synthetic link-score variable and the original
/// (untransformed) equation text that could not be parsed.
///
/// The ceteris-paribus PREVIOUS-wrapping transform can only run on a parsed
/// AST. When the target's equation text fails to parse (a genuine parse
/// error, or an empty equation), the link score cannot be built correctly,
/// so the caller skips emitting the variable and calls this to make the
/// degradation visible. This is the "loud failure" sibling of
/// [`emit_unscoreable_disjoint_edge_warning`] -- and it is *not* redundant
/// with `model_ltm_fragment_diagnostics`: that pass only catches LTM
/// equations (synthetic vars and implicit helpers) that fail to *compile*,
/// whereas the silent-fallback bug this replaces produced an equation that
/// compiled cleanly while computing a constant `|Δz/Δz| = 1` magnitude.
pub(crate) fn emit_ltm_partial_equation_warning(
    db: &dyn Db,
    model: SourceModel,
    variable_name: &str,
    err: &crate::ltm_augment::PartialEquationError,
) {
    use salsa::Accumulator;
    CompilationDiagnostic(Diagnostic {
        model: model.name(db).clone(),
        variable: Some(variable_name.to_string()),
        error: DiagnosticError::Assembly(ltm_partial_equation_warning_message(variable_name, err)),
        severity: DiagnosticSeverity::Warning,
    })
    .accumulate(db);
}

/// The human-readable message body for a partial-equation failure -- a
/// GH #311 parse failure, a GH #743 unfreezable partial (neither
/// ceteris-paribus convention can be rendered as a compilable equation), or
/// a GH #779 bare reducer feeder (a bare arrayed reference inside a reducer
/// argument, whose message names the subscripted-spelling workaround).
/// Pure (functional core) so the diagnostic's wording -- which names the
/// offending variable and equation text and explains the silent-garbage
/// hazard the loud skip prevents -- is testable without driving a salsa
/// accumulator.
pub(crate) fn ltm_partial_equation_warning_message(
    variable_name: &str,
    err: &crate::ltm_augment::PartialEquationError,
) -> String {
    use crate::ltm_augment::PartialEquationErrorKind;
    let equation_text = &err.equation_text;
    match err.kind {
        PartialEquationErrorKind::Parse => format!(
            "LTM link-score variable '{variable_name}' could not be generated: the \
             ceteris-paribus partial requires parsing the target's equation, but \
             '{equation_text}' did not parse. The variable is skipped rather than \
             emitted with a non-ceteris-paribus equation (which would silently \
             score a constant magnitude of 1)."
        ),
        PartialEquationErrorKind::UnfreezablePartial => format!(
            "LTM link-score variable '{variable_name}' could not be generated: the \
             ceteris-paribus partial of '{equation_text}' would freeze an array \
             slice inside PREVIOUS(), which cannot compile, and the changed-last \
             fallback could not be rendered either (its freeze would also lag an \
             array slice, or the source has no matching occurrence to freeze). The \
             variable is skipped rather than emitted with a silently-stubbed helper \
             (which would poison the score with a plausible-looking wrong constant \
             -- GH #743)."
        ),
        PartialEquationErrorKind::BareReducerFeeder => format!(
            "LTM link-score variable '{variable_name}' could not be generated: \
             '{equation_text}' references the arrayed source variable BARE \
             (without a subscript) inside an array-reducer argument, which cannot \
             be scored -- the per-element ceteris-paribus partial disagrees with \
             how the simulation evaluates the bare reference (GH #779/#789). The \
             variable is skipped (and dependent loop scores dropped) rather than \
             emitted with a silently wrong value; subscripting the reference \
             (e.g. 'frac[D1]') restores scoring."
        ),
    }
}

/// Emit per-distinct-source-element link scores for a disjoint-dim
/// arrayed -> arrayed edge whose target is a per-element-equation
/// (`Ast::Arrayed`) variable (GH #510) -- or, since GH #769, an
/// `Ast::ApplyToAll` target whose reference sites are ALL `FixedIndex`
/// (`hub[D2] = pop[a1] * 0.05`: one shared slot body holding `pop[a1]`
/// live, emitted as an `Equation::ApplyToAll` over the target's dims; any
/// non-`FixedIndex` site returns `None` so those edges keep the GH #758
/// loud skip byte-identically). Returns `None` if the edge is not of
/// either shape.
///
/// The case: `from` is arrayed, `to` is arrayed with an `Ast::Arrayed`
/// AST, `from`'s dims and `to`'s dims share *no* dimension name (so the
/// same-dim A2A / partial-collapse / broadcast / partial-reduce paths all
/// declined), and `to`'s per-element equations reference `from` only via
/// literal element subscripts (`source[m]`) of a dimension disjoint from
/// `to`'s dims. For each *distinct* referenced source element `m` we emit
/// one `LtmSyntheticVar` named `$⁚ltm⁚link_score⁚{from}[{m}]→{to}`, an
/// `Equation::Arrayed` over `to`'s dims whose partial holds `from[m]` live
/// in the slots whose equation references `from[m]` and is the trivial-zero
/// guard form (`from[m]` frozen at `PREVIOUS`) elsewhere -- exactly what
/// `build_arrayed_link_score_equation` produces for a `FixedIndex` source
/// into an `Ast::Arrayed` target (reached here via the salsa-cached
/// `link_score_equation_text_shaped`).
///
/// Returns:
///  - `Some(vec)` with one var per distinct referenced source element when
///    every reference is a literal element subscript;
///  - `Some(vec![])` when the target references `from` via a non-literal
///    index (a `DynamicIndex` site -- or, defensively, a `Wildcard`/`Bare`
///    site that can't be a literal-element reference into a disjoint-dim
///    target): a `Warning` is accumulated (once -- the edge is recorded in
///    `unscoreable_edges`, which both dedups the warning and makes
///    `model_ltm_variables` drop loop scores traversing the edge, the same
///    GH #758 treatment the dim-incompatible per-shape class gets) and no
///    link score is emitted, and the caller must *not* fall through to
///    `emit_per_shape_link_scores`;
///  - `None` when the edge isn't an arrayed -> disjoint-dim-`Ast::Arrayed`
///    edge at all -- the caller's existing emission path handles it.
///
/// We reuse the Phase-1 reference-site IR (`model_ltm_reference_sites`)
/// for `(from, to)` rather than re-walking the target's AST: each site's
/// `shape` is `FixedIndex(elems)` for `from[m]`, and `target_element`
/// records which `to` slot it sits in (unused here -- the per-slot partial
/// re-derives that from the slot equation).
pub(super) fn try_disjoint_dim_arrayed_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> Option<Vec<LtmSyntheticVar>> {
    // Both ends must be arrayed, non-module variables.
    let from_sv = source_vars.get(from)?;
    if from_sv.kind(db) == SourceVariableKind::Module {
        return None;
    }
    let from_dims = variable_dimensions(db, *from_sv, project);
    if from_dims.is_empty() {
        return None;
    }
    let to_sv = source_vars.get(to)?;
    if to_sv.kind(db) == SourceVariableKind::Module {
        return None;
    }
    let to_dims = variable_dimensions(db, *to_sv, project);
    if to_dims.is_empty() {
        return None;
    }
    // Disjoint: no shared dimension name. (Same-dim A2A, partial-collapse,
    // broadcast, and partial-reduce edges share at least one dim and are
    // handled by `link_score_dimensions` / `try_cross_dimensional_link_scores`.)
    let shares_a_dim = from_dims
        .iter()
        .any(|fd| to_dims.iter().any(|td| td.name() == fd.name()));
    if shares_a_dim {
        return None;
    }
    // The target must be a per-element-equation (`Ast::Arrayed`) variable,
    // OR -- GH #769 -- an `Ast::ApplyToAll` one whose sites are ALL
    // `FixedIndex` (gated below): every A2A slot then reads the same
    // literal `from[m]`, so a per-element construction (one shared slot
    // body holding `from[m]` live) is well-defined with no
    // wrong-slot-diagonal hazard. A whole-array disjoint-dim reference
    // into an A2A target would be a dimension error (a D3-shaped value
    // can't broadcast onto D1xD2), which is why only the literal-index
    // sub-case is recoverable; a scalar source into an arrayed target is
    // `try_scalar_to_arrayed`'s job.
    let to_var = reconstruct_single_variable(db, model, project, to)?;
    let to_ast_is_arrayed = matches!(to_var.ast(), Some(crate::ast::Ast::Arrayed(..)));
    let to_ast_is_a2a = matches!(to_var.ast(), Some(crate::ast::Ast::ApplyToAll(..)));
    if !to_ast_is_arrayed && !to_ast_is_a2a {
        return None;
    }
    // Consult the reference-site IR. A `FixedIndex(elems)` site is a
    // `from[m]` reference; anything else (a dynamic index, or -- defensively
    // -- a `Wildcard`/`Bare`/`PerElement`, which can't be a valid
    // literal-element reference into a disjoint-dim target) makes the edge
    // unscoreable.
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    let sites = ir.sites.get(&(from.to_string(), to.to_string()))?;
    if sites.is_empty() {
        return None;
    }
    // GH #769: the ApplyToAll-target widening is FixedIndex-ONLY. Any other
    // site shape returns `None` so the edge keeps today's degradation
    // byte-identically (the GH #758 loud skip in
    // `emit_per_shape_link_scores` -- one Warning, no link-score variable,
    // loop scores through the edge dropped).
    if to_ast_is_a2a
        && sites
            .iter()
            .any(|s| !matches!(s.shape, RefShape::FixedIndex(_)))
    {
        return None;
    }
    // Distinct referenced source elements, in first-occurrence order.
    let mut elem_keys: Vec<String> = Vec::new();
    for site in sites {
        match &site.shape {
            RefShape::FixedIndex(es) => {
                // The comma-joined form is the `[m]` (or `[m,k]`) the var
                // name carries and `shape_aware_source_ref` renders.
                let key = es.join(",");
                if !elem_keys.contains(&key) {
                    elem_keys.push(key);
                }
            }
            RefShape::Bare
            | RefShape::Wildcard
            | RefShape::DynamicIndex
            | RefShape::PerElement { .. } => {
                // Record the edge so loop scores through it are dropped
                // (GH #758 unification); the insert also dedups the warning
                // when the pinned-loop pass re-visits the edge (whose
                // `emitted_edges` set only tracks edges that emitted a var).
                if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                    emit_unscoreable_disjoint_edge_warning(db, model, from, to);
                }
                return Some(vec![]);
            }
        }
    }

    // One link-score variable per distinct referenced source element. The
    // equation comes from the salsa-cached shaped path, which routes the
    // `Ast::Arrayed` target through `build_arrayed_link_score_equation`
    // (an `Equation::Arrayed` over `to`'s dims, already tagged with `to`'s
    // datamodel dim names -- so `dimensions` mirrors them and the
    // `retarget` is a no-op). The `[m]` in the name routes the var
    // directly in `assemble_module`'s bracket check, so `compile_directly`
    // is irrelevant but set `false` for consistency with the other
    // bracketed cross-dimensional vars.
    let mut vars = Vec::with_capacity(elem_keys.len());
    for key in &elem_keys {
        let shape = RefShape::FixedIndex(key.split(',').map(|s| s.to_string()).collect());
        let link_id = LtmLinkId::new(db, from.to_string(), to.to_string());
        match link_score_equation_text_shaped(db, link_id, shape.clone(), model, project).clone() {
            ShapedLinkScore::Unscoreable => {
                // GH #780: a `PartialEquationError` for one element of this
                // disjoint-dim edge dooms the whole edge -- the partial that
                // failed is shared structure, and a loop hop through the
                // edge resolves to one element-named score. Record it (the
                // query already warned) so dependent loop scores drop, and
                // return `Some(vec![])` so the dispatcher does NOT fall
                // through to `emit_per_shape_link_scores` and mint a
                // wrong-shaped stand-in (mirroring the dynamic-index
                // disjoint skip at the top of this fn).
                unscoreable_edges.insert((from.to_string(), to.to_string()));
                return Some(vec![]);
            }
            ShapedLinkScore::NoVariable => continue,
            ShapedLinkScore::Scored(mut lsv) => {
                // `lsv.name` is already `link_score_var_name(from, to, &shape)`
                // from the shaped path -- no need to re-derive it here.
                let dims = ltm_equation_dimensions(&lsv.equation).to_vec();
                lsv.dimensions = dims.clone();
                lsv.equation = retarget_ltm_equation_dims(lsv.equation, &dims);
                lsv.compile_directly = false;
                vars.push(lsv);
            }
        }
    }
    Some(vars)
}

/// Emit per-shape link scores for a single (from, to) edge.
///
/// The emission is shape-driven (Phase 3): one `LtmSyntheticVar` per
/// `(from, to, shape)` tuple, named by `link_score_var_name`. The shape
/// set comes from `model_ltm_reference_sites` (the distinct `shape`
/// fields of `(from, to)`'s classified sites); module links and edges
/// with no AST reference have no IR entry and fall back to a single Bare
/// emission so the legacy behavior is preserved at structural boundaries.
///
/// `fallback_shape` is the shape to use when the IR has no entry for the
/// edge (e.g. a module edge, or an implicit synthesized reference that
/// doesn't appear in the target's AST). Callers pass `RefShape::Bare` to
/// preserve the legacy single-shape behavior.
///
/// `skip_reducer_shapes` is set when the caller has already handled the
/// `from` reference's reducer occurrences by routing them through an
/// aggregate node (Phase 5) -- i.e. when the routed-agg set for `(from,
/// to)` is non-empty. Only the `Wildcard` shape is suppressed in that
/// case: a `Wildcard` reference to `from` in `to` is the hoisted
/// reducer's argument (`SUM(pop[*])`, classified `ThroughAgg` in the
/// IR), already scored by the `source → agg` / `agg → target` halves,
/// so re-scoring it here would double-count. Every *other* shape is kept
/// -- including `DynamicIndex` (a direct `pop[idx]` alongside a hoisted
/// `SUM(pop[*])`, classified `Direct`) **and `Bare`** (a bare arrayed
/// reducer arg like `SUM(pop)`, classified `ThroughAgg` for the element
/// graph but still given its own `Bare`-named link score here -- this is
/// exactly the pre-IR behavior, where `enumerate_shapes` returned every
/// shape of `from` in `to` and `skip_reducer_shapes` dropped only
/// `Wildcard`). Equivalently: feed every distinct site `shape` to the
/// per-shape pass, removing `Wildcard` iff the routed-agg set is
/// non-empty -- which is what the element-graph routing and the
/// link-score routing "agree" on (the same `ClassifiedSite::routing`
/// data), differing only in that the link scorer additionally keeps
/// non-`Wildcard` shapes from `ThroughAgg` sites.
///
/// `Wildcard`/`DynamicIndex` shapes that reach this function share the
/// canonical `link_score_var_name` form with `Bare`, so we dedup by the
/// resulting name and keep the first occurrence -- the AST walk records
/// `Bare` before any subscripted reference, so the canonical-Bare link
/// score wins the slot when both a bare and a subscripted reference exist.
///
/// `unscoreable_edges` collects the (from, to) edges the GH #758 gate
/// declines to score (arrayed endpoints whose dimensions don't correspond
/// -- see [`emit_unscoreable_conservative_edge_warning`]); the caller
/// (`model_ltm_variables`) drops loop scores traversing them. The warning
/// is accumulated only on first insertion, so an edge re-visited by the
/// pinned-loop pass doesn't warn twice.
#[allow(clippy::too_many_arguments)] // helper threads through emission context
pub(super) fn emit_per_shape_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    fallback_shape: RefShape,
    model: SourceModel,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    skip_reducer_shapes: bool,
    vars: &mut Vec<LtmSyntheticVar>,
    unscoreable_edges: &mut HashSet<(String, String)>,
) {
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    // T6 (GH #525): `Direct`-routed `PerElement` sites take the dedicated
    // per-(row, full-target-element) emitter -- their names carry BOTH a
    // from-side row and a to-side element subscript, which the shaped
    // per-(from, to, shape) path cannot express. They are removed from the
    // per-shape list below; a `ThroughAgg`-routed `PerElement` site (the
    // ALIASED routing family: routing is per-edge `in_reducer &&
    // routed_aggs`, so a mixed subscript inside a DECLINED reducer routes
    // through a sibling hoisted agg) emits nothing here -- the agg halves
    // carry that hop's scores. In EXHAUSTIVE mode this is byte-identical to
    // pre-T6: the loop-link caller emits an agg-routed hop's scores via its
    // agg branches and never reaches this per-shape pass for the edge, so
    // no Bare-named score existed for the shape's pre-T6 `DynamicIndex`
    // spelling either (pinned by
    // `aliased_through_agg_per_element_site_emits_only_agg_halves`). In
    // DISCOVERY mode (which iterates causal edges and DOES reach this pass)
    // the pre-T6 `DynamicIndex` shape minted an extra Bare-named
    // conservative score alongside the agg halves -- a duplicate direct
    // pathway in the search graph for a hop the agg halves already carry;
    // that double-attribution retires with the shape (same pin, discovery
    // half).
    let mut per_element_sites: Vec<(Vec<crate::ltm_agg::AxisRead>, Option<String>)> = Vec::new();
    if let Some(sites) = ir.sites.get(&(from.to_string(), to.to_string())) {
        for s in sites {
            if let RefShape::PerElement { axes } = &s.shape
                && matches!(s.routing, crate::db::ltm_ir::SiteRouting::Direct)
            {
                let entry = (axes.clone(), s.target_element.clone());
                if !per_element_sites.contains(&entry) {
                    per_element_sites.push(entry);
                }
            }
        }
    }
    // Everything this call pushes belongs to the one `(from, to)` edge, so
    // a GH #780 doom anywhere below discards back to this snapshot -- no
    // already-emitted sibling-shape var may survive for an edge whose loops
    // are dropped (it would be an orphan score for hops the drop machinery
    // hides; the disjoint/scalar-to-arrayed siblings discard everything,
    // and this pass must match).
    let vars_start = vars.len();
    if !per_element_sites.is_empty()
        && emit_per_element_link_scores(
            db,
            source_vars,
            from,
            to,
            &per_element_sites,
            model,
            project,
            vars,
            unscoreable_edges,
        )
    {
        // The per-element emitter doomed (and recorded) the edge: skip the
        // shaped loop too -- its Bare/FixedIndex siblings would be orphan
        // scores on a dropped edge.
        vars.truncate(vars_start);
        return;
    }
    // The distinct `shape` fields of `(from, to)`'s classified sites,
    // in AST-walk order of first occurrence (equivalent to the per-edge
    // shape set the AST walker produced before the IR).
    let mut shapes: Vec<RefShape> = ir
        .sites
        .get(&(from.to_string(), to.to_string()))
        .map(|sites| {
            let mut v: Vec<RefShape> = Vec::new();
            for s in sites {
                if !v.contains(&s.shape) {
                    v.push(s.shape.clone());
                }
            }
            v
        })
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| vec![fallback_shape]);
    if skip_reducer_shapes {
        shapes.retain(|s| !matches!(s, RefShape::Wildcard));
    }
    // `PerElement` shapes were handled above (Direct) or belong to the agg
    // halves (ThroughAgg); either way they never enter the shaped loop.
    shapes.retain(|s| !matches!(s, RefShape::PerElement { .. }));
    // Distinct `RefShape`s can map to the same synthetic name (a
    // conservative-slice / direct-dynamic-index `Wildcard`/`DynamicIndex`
    // ref shares the canonical Bare name now that the per-shape suffix is
    // retired); keep only the first shape that produces each name.
    let mut emitted_names: HashSet<String> = HashSet::new();
    shapes.retain(|shape| {
        emitted_names.insert(crate::ltm_augment::link_score_var_name(from, to, shape))
    });
    if shapes.is_empty() {
        // Every shape was a suppressed reducer-arg Wildcard (the agg halves
        // carry the edge's scores) or a PerElement handled above: nothing
        // left for the shaped loop.
        return;
    }

    let target_dims = link_score_dimensions(db, source_vars, from, to, model, project, dm_dims);

    // GH #758: when BOTH endpoints are arrayed non-module variables but
    // `link_score_dimensions` found no correspondence (`target_dims`
    // empty), every per-shape equation this loop would emit is
    // broken-by-construction -- `retarget_ltm_equation_dims` collapses it
    // to a SCALAR equation whose guard form references the multi-slot
    // target (and whose partial body references the arrayed deps) in
    // scalar context, so the fragment never compiles and the score is
    // silently stubbed to 0 while every loop score through the edge
    // fail-warns in a cascade. The arrayed alternative is forbidden by the
    // PR #761 Bare-site gate (the element edges are the conservative
    // cross-product, so per-slot diagonal partials would be read at wrong
    // slots). Degrade loudly instead: one Warning naming the edge, no
    // link-score variable, and (via `unscoreable_edges`) no loop scores
    // through it. The declined ELEMENT-mapped sliced reducers (GH #756;
    // reverse-declared positional pairs are hoisted since GH #757) land
    // here, as do disjoint-dim ApplyToAll-target references whose sites
    // are not all FixedIndex (the GH #769 widening recovers the
    // FixedIndex-only ones) and incompatible-dim dynamic-index reducers --
    // all previously warned zero-stubs.
    let arrayed_non_module = |name: &str| -> bool {
        source_vars
            .get(name)
            .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
            .map(|sv| !variable_dimensions(db, *sv, project).is_empty())
            .unwrap_or(false)
    };
    if target_dims.is_empty() && arrayed_non_module(from) && arrayed_non_module(to) {
        if unscoreable_edges.insert((from.to_string(), to.to_string())) {
            emit_unscoreable_conservative_edge_warning(db, model, from, to);
        }
        // Discard any per-element vars pushed above for this now-unscoreable
        // edge (the same no-orphan-scores rule as the doom arms below; a
        // PerElement-bearing edge landing in this gate is likely
        // unreachable, but the invariant is cheap to keep).
        vars.truncate(vars_start);
        return;
    }

    for shape in shapes {
        let link_id = LtmLinkId::new(db, from.to_string(), to.to_string());
        match link_score_equation_text_shaped(db, link_id, shape.clone(), model, project).clone() {
            ShapedLinkScore::Unscoreable => {
                // GH #780: a `PartialEquationError` for this shape makes the
                // whole `(from, to)` edge unscoreable. The query already
                // accumulated the one loud `Warning`; record the edge so
                // loop scores traversing it are DROPPED (the #758 contract)
                // instead of referencing the never-emitted link-score name
                // and degrading to warned constant-0 stubs. Recording at
                // EDGE (not per-shape-name) granularity is the soundest
                // implementable rule: a loop hop through this edge resolves
                // (`loop_link_score_ref`) to ONE shape's name, and if that
                // is the doomed shape the loop would stub; dropping the loop
                // is conservative but never silently wrong, matching the
                // dim-incompatible and broadcast-reduce unscoreable classes.
                // Since any doomed shape dooms the edge, `break` once we see
                // one (the remaining shapes' warnings would be redundant
                // noise about the same already-recorded edge). The `Warning`
                // fired inside the salsa query (replayed via the accumulator
                // on every evaluation), so only the edge recording is needed
                // here; the insert also dedups a re-visit by the pinned-loop
                // pass. Discard any earlier shape's already-pushed var for
                // this edge (`vars_start`): the edge's loops are dropped, so
                // a surviving sibling-shape score would be an orphan --
                // matching the disjoint / scalar-to-arrayed siblings, which
                // discard everything on doom.
                unscoreable_edges.insert((from.to_string(), to.to_string()));
                vars.truncate(vars_start);
                break;
            }
            ShapedLinkScore::NoVariable => {
                // Benign structural skip (unreconstructible target /
                // composite-less module link); not an unscoreable edge.
                continue;
            }
            ShapedLinkScore::Scored(mut lsv) => {
                // Set the canonical name and dimensions per Phase 3 Task 4/5.
                lsv.name = crate::ltm_augment::link_score_var_name(from, to, &shape);
                // Every shape takes the target's dimensions: for FixedIndex
                // each per-element link score is scalar when the target is
                // scalar and arrayed when the target is arrayed; Bare (and
                // the rare conservative-slice Wildcard/DynamicIndex) inherit
                // the target's dims via the same compatibility rule.
                // link_score_dimensions already implements this for every
                // case, so one assignment suffices.
                lsv.dimensions = target_dims.clone();
                // Keep the equation's dimensionality in lockstep with the
                // `dimensions` field that layout sizing keys off of. The
                // shaped fn returns a `Scalar`/`ApplyToAll` equation tagged
                // with the *target's own* dimension names, which equal
                // `target_dims` for every compatible-dimension edge; for the
                // (rare) incompatible-dimension arrayed-target edge,
                // link_score_dimensions returns empty, so we collapse the
                // equation to scalar here -- matching the pre-existing
                // behavior where such edges produced a scalar link score.
                lsv.equation = retarget_ltm_equation_dims(lsv.equation, &target_dims);
                // A non-`Bare` shape carries a partial that the (from, to)-
                // keyed salsa compilation path (`link_score_equation_text`,
                // always `RefShape::Bare`) cannot reproduce: a
                // `Wildcard`/`DynamicIndex` reference into a scalar target
                // would have its whole subscript wrapped in `PREVIOUS()` and
                // the ceteris-paribus numerator zeroed. Force `assemble_module`
                // to compile this var's equation verbatim. (For `Bare`, the
                // salsa-cached path is correct and keeps incrementality;
                // `FixedIndex` carries an element subscript on the name and is
                // already routed directly by the bracket check, but flagging
                // it too is harmless.)
                lsv.compile_directly = !matches!(shape, RefShape::Bare);
                vars.push(lsv);
            }
        }
    }
}

/// Emit the per-(row, FULL-target-element) scalar link scores for the
/// `Direct` `PerElement` reference sites of one `(from, to)` edge (GH #525,
/// T6 of the shape-expressiveness design).
///
/// For every site (a distinct per-axis access vector, e.g.
/// `[Iterated(region), Pinned(young)]` for `pop[Region, young]`) and every
/// FULL target element `e`, the row is a *function of `e`*: project `e`
/// onto the `Iterated` axes (slot-remapped for a positionally-mapped pair)
/// and fill the `Pinned` axes with their literals
/// ([`crate::ltm_augment::per_element_row_for_target`], the single row
/// derivation shared with the equation builder). The variable is the
/// scalar `$⁚ltm⁚link_score⁚{from}[{row}]→{to}[{e}]` -- the EXISTING
/// per-(row, slot) grammar `try_cross_dimensional_link_scores`'s
/// partial-reduce arm established, which `loop_link_score_ref` and
/// discovery's `parse_link_offsets` already resolve; `e` is always the
/// full target element (never a partial to-subscript, which no resolver
/// matches). When the `Iterated` dims equal `to`'s dims this is 1:1
/// rows-to-slots; in the BROADCAST case (`Iterated` dims a strict subset
/// of `to`'s, `aux[D1,D2] = arr[D1,lit] * ...`) one row feeds every `e` it
/// projects from, mirroring `agg_name_for_target`'s projection.
///
/// The equation is the per-target-element changed-first partial built by
/// [`crate::ltm_augment::generate_per_element_link_equation`]: the live
/// reference rewritten to the real `{from}[{row}]` subscript, every other
/// source occurrence pinned-and-frozen, other arrayed deps element-pinned.
/// A site recorded inside an `Ast::Arrayed` slot (`target_element` is
/// `Some`) emits only for that element; A2A sites emit for every `e`.
///
/// A row that fails to derive (a mid-edit mapping inconsistency) or an
/// equation builder failure degrades LOUDLY (GH #780): one
/// [`emit_ltm_partial_equation_warning`] naming the first doomed
/// `(row, e)`, the `(from, to)` edge recorded in `unscoreable_edges`
/// (dropping dependent loop scores), NO per-(row, e) variable emitted for
/// the edge (the collect-local vars are discarded), and `true` returned so
/// the caller skips/discards its own emission for the edge. Edge-level
/// recording for the same reason as `iterated_feeder_row_scores`: the drop
/// machinery matches stripped `(from, to)` pairs, so per-(row, e)
/// granularity is not implementable, and a partial emission would leave
/// the doomed instances' loops as warned constant-0 stubs.
#[allow(clippy::too_many_arguments)] // threads salsa keys + emission context
fn emit_per_element_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    sites: &[(Vec<crate::ltm_agg::AxisRead>, Option<String>)],
    model: SourceModel,
    project: SourceProject,
    vars: &mut Vec<LtmSyntheticVar>,
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> bool {
    use crate::ast::Ast;
    use crate::common::{Canonical, Ident};

    /// One target element's equation parts: (body text, full dep set, the
    /// arrayed deps to element-pin).
    type ElemEqnParts = (String, HashSet<Ident<Canonical>>, HashSet<Ident<Canonical>>);

    let Some(from_sv) = source_vars.get(from) else {
        return false;
    };
    if from_sv.kind(db) == SourceVariableKind::Module {
        return false;
    }
    let from_dims = variable_dimensions(db, *from_sv, project);
    let Some(to_sv) = source_vars.get(to) else {
        return false;
    };
    if to_sv.kind(db) == SourceVariableKind::Module {
        return false;
    }
    let to_dims = variable_dimensions(db, *to_sv, project).clone();
    if from_dims.is_empty() || to_dims.is_empty() {
        // A `PerElement` site requires an arrayed source and an iterated
        // target equation; scalar endpoints mean a stale classification.
        return false;
    }
    let Some(to_var) = reconstruct_single_variable(db, model, project, to) else {
        return false;
    };
    let Some(ast) = to_var.ast() else {
        return false;
    };
    let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
        Ast::Scalar(_) => &[],
        Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
    };
    let target_iterated_dims: Vec<String> = target_ast_dims
        .iter()
        .map(|d| d.name().to_string())
        .collect();
    let dim_ctx = project_dimensions_context(db, project);

    // Arrayed deps sharing a target dim get element-pinned in the scalar
    // per-element equation (mirroring `try_scalar_to_arrayed_link_scores`);
    // the source itself is excluded -- its occurrences are pinned per-row
    // by the equation builder's rewrite pass.
    let deps_to_subscript = |deps: &HashSet<Ident<Canonical>>| -> HashSet<Ident<Canonical>> {
        deps.iter()
            .filter(|d| {
                d.as_str() != from
                    && source_vars
                        .get(d.as_str())
                        .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
                        .map(|sv| {
                            let dd = variable_dimensions(db, *sv, project);
                            !dd.is_empty()
                                && dd
                                    .iter()
                                    .any(|x| to_dims.iter().any(|td| td.name() == x.name()))
                        })
                        .unwrap_or(false)
            })
            .cloned()
            .collect()
    };

    let to_dim_element_lists: Vec<Vec<String>> = to_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    let elements = cartesian_subscripts(&to_dim_element_lists);

    // Per target element: the body text + dep sets (shared for A2A,
    // per-slot for Arrayed -- mirroring `try_scalar_to_arrayed_link_scores`).
    let a2a_parts: Option<ElemEqnParts> = if let Ast::ApplyToAll(_, expr) = ast {
        let text = crate::patch::expr2_to_string(expr);
        let deps = crate::variable::identifier_set(ast, target_ast_dims, None);
        let to_sub = deps_to_subscript(&deps);
        Some((text, deps, to_sub))
    } else {
        None
    };

    // Name-level dedup: two sites can in principle derive the same
    // (row, e) name; first emission wins (matching the per-shape pass's
    // name-dedup convention). Vars collect locally and commit only if no
    // (row, e) dooms (GH #780; see the fn doc).
    let mut emitted: HashSet<String> = HashSet::new();
    let mut edge_vars: Vec<LtmSyntheticVar> = Vec::new();

    for element in &elements {
        // The projection data `e` supplies: target dim (canonical) ->
        // (element name, index within that dim's element list).
        let parts: Vec<&str> = element.split(',').collect();
        if parts.len() != to_dims.len() {
            continue;
        }
        let mut target_elem_by_dim: HashMap<String, (String, usize)> = HashMap::new();
        for ((dim, elems), part) in to_dims.iter().zip(&to_dim_element_lists).zip(&parts) {
            let Some(idx) = elems.iter().position(|e| e == part) else {
                continue;
            };
            target_elem_by_dim.insert(dim.name().to_string(), ((*part).to_string(), idx));
        }
        let qualified_element = crate::ltm_augment::qualify_element_csv(element, &to_dims);

        // Per-slot body text / deps for an `Ast::Arrayed` target.
        let slot_parts: Option<ElemEqnParts> = match ast {
            Ast::Arrayed(_, per_elem, default_expr, _) => {
                let canonical_elem = crate::common::CanonicalElementName::from_raw(element);
                let slot = per_elem.get(&canonical_elem).or(default_expr.as_ref());
                match slot {
                    Some(expr) => {
                        let text = crate::patch::expr2_to_string(expr);
                        let deps = crate::variable::identifier_set(
                            &Ast::Scalar(expr.clone()),
                            target_ast_dims,
                            None,
                        );
                        let to_sub = deps_to_subscript(&deps);
                        Some((text, deps, to_sub))
                    }
                    // No slot, no default: a hole -- this element has no
                    // equation, so the site cannot occur in it; skip.
                    None => None,
                }
            }
            _ => None,
        };
        let Some((body_text, deps, to_sub)) = a2a_parts.as_ref().or(slot_parts.as_ref()) else {
            continue;
        };

        for (axes, site_target_element) in sites {
            // A site inside an `Ast::Arrayed` slot contributes only to its
            // own element's score.
            if let Some(te) = site_target_element
                && te != element
            {
                continue;
            }
            let Some(row_parts) =
                crate::ltm_augment::per_element_row_for_target(axes, &target_elem_by_dim, dim_ctx)
            else {
                // A mid-edit mapping inconsistency: the classified site's
                // row is underivable for this element. Loud edge doom
                // (GH #780; see the fn doc).
                let name = format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{from}[?]\u{2192}{to}[{element}]"
                );
                // Warn only on first recording (#758 convention): a
                // pinned-pass re-visit re-dooms deterministically and must
                // stay silent.
                if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                    emit_ltm_partial_equation_warning(
                        db,
                        model,
                        &name,
                        &crate::ltm_augment::PartialEquationError::new(body_text),
                    );
                }
                return true;
            };
            let row = row_parts.join(",");
            let name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{from}[{row}]\u{2192}{to}[{element}]"
            );
            if !emitted.insert(name.clone()) {
                continue;
            }
            match crate::ltm_augment::generate_per_element_link_equation(
                from,
                to,
                axes,
                &row_parts,
                &qualified_element,
                body_text,
                deps,
                to_sub,
                from_dims,
                &target_elem_by_dim,
                &target_iterated_dims,
                dim_ctx,
            ) {
                Ok(equation) => edge_vars.push(LtmSyntheticVar {
                    name,
                    equation: datamodel::Equation::Scalar(equation),
                    dimensions: vec![], // scalar -- one variable per (row, element)
                    // bracketed name -> routed direct by `assemble_module`.
                    compile_directly: false,
                }),
                Err(err) => {
                    // One doomed (row, e) dooms the edge (GH #780; fn doc);
                    // warn only on first recording (#758 convention).
                    if unscoreable_edges.insert((from.to_string(), to.to_string())) {
                        emit_ltm_partial_equation_warning(db, model, &name, &err);
                    }
                    return true;
                }
            }
        }
    }
    vars.extend(edge_vars);
    false
}

/// Build the owned inputs for a [`crate::ltm_augment::ReducerBodyCtx`]:
/// for every model variable a reducer body references, its membership in
/// the freeze set (`model_deps`) and -- when arrayed -- its declared
/// dimension count (`arrayed_dep_dims`, the row-pinning gate). Identifiers
/// in the body that are not model variables (dimension/element names in
/// subscripts, TIME) are excluded, so the body partial leaves them live,
/// matching `build_partial_equation_shaped`'s deps-only freezing.
fn reducer_body_ctx_parts(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    project: SourceProject,
    body_text: &str,
) -> (HashMap<String, usize>, HashSet<String>) {
    let mut arrayed_dep_dims: HashMap<String, usize> = HashMap::new();
    let mut model_deps: HashSet<String> = HashSet::new();
    for ident in crate::ltm_augment::expr_reference_idents(body_text) {
        if let Some(sv) = source_vars.get(&ident) {
            model_deps.insert(ident.clone());
            let dims = variable_dimensions(db, *sv, project);
            if !dims.is_empty() {
                arrayed_dep_dims.insert(ident, dims.len());
            }
        }
    }
    (arrayed_dep_dims, model_deps)
}

/// Emit the per-`(row, slot)` link scores for an ITERATED-DIM PROJECTION
/// FEEDER `from` of a hoisted reducer (GH #767 / T5 of the
/// shape-expressiveness design): `frac` in
/// `growth[D1] = SUM(matrix[D1,*] * frac[D1])`. The rows come from
/// `read_slice_rows` over the FEEDER'S OWN all-`Iterated` slice -- 1:1
/// with the agg's result slots by the I1 acceptance -- and each row's
/// equation is the per-slot changed-last partial
/// ([`crate::ltm_augment::generate_iterated_feeder_to_agg_equation`]).
/// Names follow the existing per-`(row, slot)` grammar
/// (`$⁚ltm⁚link_score⁚{from}[{row}]→{agg}[{slot}]`), which
/// `loop_link_score_ref` and discovery's `parse_link_offsets` already
/// resolve.
///
/// Shared by the synthetic half (`emit_source_to_agg_link_scores`, where
/// `agg` is a `$⁚ltm⁚agg⁚{n}` aux) and the variable-backed branch of
/// `try_cross_dimensional_link_scores` (where `agg.name == to`). `None`
/// when `read_slice_rows` declines (a stale slice invariant) -- the caller
/// keeps its conservative fallback rather than emitting mis-slotted
/// scores.
///
/// A per-row generator failure (the GH #743 `UnfreezablePartial` machinery,
/// e.g. a feeder occurrence that cannot be frozen) dooms
/// the WHOLE `(from, agg)` edge (GH #780): one loud warning naming the
/// first doomed row, the edge recorded in `unscoreable_edges` (keyed
/// `(from, agg.name)` -- exactly how a loop's links spell the hop, which
/// `traverses_unscoreable` strips to), and `Some(vec![])` returned so NO
/// per-row score is emitted. Edge-level (not per-row) recording is forced
/// by the drop machinery's granularity: `traverses_unscoreable` matches
/// stripped `(from, to)` pairs, so loops through still-derivable rows
/// cannot be kept once any row's name is missing -- and a partial emission
/// would leave the doomed rows' loops as warned constant-0 stubs, the
/// exact #758-contract violation. Conservative, never silently wrong --
/// the same reasoning as the per-shape edge-granularity decision in
/// `emit_per_shape_link_scores`.
fn iterated_feeder_row_scores(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
    from: &str,
    from_dims: &[crate::dimensions::Dimension],
    agg: &crate::ltm_agg::AggNode,
    unscoreable_edges: &mut HashSet<(String, String)>,
) -> Option<Vec<LtmSyntheticVar>> {
    let slice = agg.source_read_slice(from);
    let dim_element_lists: Vec<Vec<String>> = from_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    let rows = read_slice_rows(
        slice,
        &dim_element_lists,
        project_dimensions_context(db, project),
    )?;
    // The slot axes' canonical dim names, in order -- the feeder's own axis
    // dims (acceptance guarantees they equal the canonical Iterated target
    // dims, unmapped).
    let iterated_dims: Vec<String> = slice
        .iter()
        .filter_map(|ax| match ax {
            crate::ltm_agg::AxisRead::Iterated { dim, .. } => Some(dim.clone()),
            _ => None,
        })
        .collect();
    let mut vars = Vec::with_capacity(rows.len());
    for ReadSliceRow { row, slot, .. } in &rows {
        // Names keep the bare element form (the user-facing / discovery-
        // parsed identity); equation text uses the qualified `dim·element`
        // form (direct LoadPrev, no helper auxes) -- exactly as the other
        // per-row emitters.
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
            from, row, agg.name, slot
        );
        // The feeder's axes ARE the slot axes, so qualification against
        // `from_dims` is the slot's own qualification too.
        let qualified_slot = crate::ltm_augment::qualify_element_csv(slot, from_dims);
        let slot_parts: Vec<String> = qualified_slot.split(',').map(str::to_string).collect();
        match crate::ltm_augment::generate_iterated_feeder_to_agg_equation(
            from,
            &agg.name,
            &agg.equation_text,
            &iterated_dims,
            &slot_parts,
        ) {
            Ok(equation) => vars.push(LtmSyntheticVar {
                name,
                equation: datamodel::Equation::Scalar(equation),
                dimensions: vec![], // scalar -- one variable per read row
                // bracketed name -> routed direct by `assemble_module`.
                compile_directly: false,
            }),
            Err(err) => {
                // GH #780: one doomed row dooms the whole edge (see the fn
                // doc). Record + warn-on-first-insert (#758 convention; a
                // pinned-pass re-visit stays silent), drop every row's score.
                if unscoreable_edges.insert((from.to_string(), agg.name.clone())) {
                    emit_ltm_partial_equation_warning(db, model, &name, &err);
                }
                return Some(vec![]);
            }
        }
    }
    Some(vars)
}

/// Emit the `source[<read row>] → agg` link-score half: one scalar
/// `$⁚ltm⁚link_score⁚{from}[<row>]→{agg}` (when the agg is scalar) or
/// `$⁚ltm⁚link_score⁚{from}[<row>]→{agg}[<slot>]` (when the agg is arrayed
/// over its `result_dims` -- the partial-reduce shape that mirrors
/// `try_cross_dimensional_link_scores`'s `{from}[{d1,d2}]→{to}[{d1}]`) per
/// source *read row* -- not every source element. For a whole-extent
/// reducer that's all of `from`'s elements; for a sliced reducer
/// (`SUM(pop[NYC,*])`, `read_slice = [Pinned(nyc), Reduced]`) it's only
/// the rows the slice reads (here, the `pop[nyc,*]` slice), so the unread
/// rows get *no* link score rather than a nonzero garbage one. Each row's
/// `coreduced` set (the rows mapping to the same agg slot -- the
/// `from`-slice combined for that slot) is the `all_elements` for the
/// MEAN divisor / nonlinear expansion. The agg's *own* equation is exactly
/// the reducer call (no surrounding arithmetic), but its argument may
/// apply a coefficient to `from`, so the SUM/MEAN row partial is built
/// from the reducer BODY via a `ReducerBodyCtx` (GH #744) rather than the
/// bare-source algebraic shortcut.
///
/// A *scalar* `from` is the GH #737 scalar-feeder case (`scale` in
/// `SUM(pop[*] * scale)`): there are no read rows, the element graph emits
/// `scale → agg` (or `scale → agg[<slot>]` per result slot for an arrayed
/// agg), and the loop builder's element-level path traverses that hop. One
/// Bare-named link score `$⁚ltm⁚link_score⁚{from}→{agg}` is emitted --
/// shaped over `result_dims` for an arrayed agg, so a loop visiting one
/// slot references it as `"{name}"[<slot>]` -- with the changed-last
/// equation from `generate_scalar_feeder_to_agg_equation` (the changed-first
/// partial would need a lagged whole-array read, which doesn't compile; see
/// that function's doc).
#[allow(clippy::too_many_arguments)] // threads salsa keys + emission context
pub(super) fn emit_source_to_agg_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    agg: &crate::ltm_agg::AggNode,
    model: SourceModel,
    project: SourceProject,
    vars: &mut Vec<LtmSyntheticVar>,
    unscoreable_edges: &mut HashSet<(String, String)>,
) {
    let Some(from_sv) = source_vars.get(from) else {
        return;
    };
    if from_sv.kind(db) == SourceVariableKind::Module {
        return;
    }
    let from_dims = variable_dimensions(db, *from_sv, project);
    if from_dims.is_empty() {
        // GH #737: a scalar feeder of the hoisted reducer. The per-read-row
        // machinery below is meaningless for a scalar source; emit the single
        // Bare-named score (dimensioned over `result_dims` when the agg is
        // arrayed) built on the changed-last attribution instead.
        let name = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
            from, agg.name
        );
        match crate::ltm_augment::generate_scalar_feeder_to_agg_equation(
            from,
            &agg.name,
            &agg.equation_text,
        ) {
            Ok(text) => {
                let equation = if agg.result_dims.is_empty() {
                    datamodel::Equation::Scalar(text)
                } else {
                    // An arrayed agg's feeder score is per-slot: the agg's own
                    // equation text is already ApplyToAll-compatible over
                    // `result_dims` (it is the agg aux's own equation shape),
                    // and the bare agg/feeder references resolve same-element
                    // / broadcast respectively in A2A context.
                    datamodel::Equation::ApplyToAll(agg.result_dims.clone(), text)
                };
                vars.push(LtmSyntheticVar {
                    name,
                    equation,
                    dimensions: agg.result_dims.clone(),
                    // agg-named target -> routed direct by the synthetic-agg
                    // check in compile_ltm_synthetic_fragment.
                    compile_directly: false,
                });
            }
            Err(err) => {
                // GH #780: the scalar feeder's single score IS this edge's
                // entire emission; a doom leaves every loop hop through
                // `(from, agg)` referencing a missing name. Record + warn on
                // first insert only (#758 convention) so dependent loop
                // scores drop instead of stubbing and a pinned-pass re-visit
                // does not duplicate the warning.
                if unscoreable_edges.insert((from.to_string(), agg.name.clone())) {
                    emit_ltm_partial_equation_warning(db, model, &name, &err);
                }
            }
        }
        return;
    }
    // GH #767 (T5): an arrayed ITERATED-DIM projection feeder of the
    // hoisted reducer (`frac` in `1 + SUM(matrix[D1,*] * frac[D1])`). The
    // per-read-row changed-FIRST machinery below builds the body partial
    // around the LIVE source's co-reduced rows, which is the co-source's
    // attribution -- a feeder's per-slot delta spans the whole co-reduced
    // slice, so its half uses the per-row changed-last equations instead
    // (the arrayed generalization of the scalar-feeder convention above).
    // A `None` from the row derivation (stale slice invariant) falls
    // through to the per-read-row machinery's own conservative fallback.
    if agg.source_is_projection_feeder(from)
        && let Some(feeder_vars) =
            iterated_feeder_row_scores(db, model, project, from, from_dims, agg, unscoreable_edges)
    {
        vars.extend(feeder_vars);
        return;
    }
    // Reconstruct a transient (parsed + lowered) `Variable` from the
    // agg's equation text so `classify_reducer` can read the reducer
    // kind/name. An arrayed agg (`result_dims` non-empty -- a sliced
    // reducer like `SUM(matrix[D1,*])` over an A2A-`D1` body) must be
    // reconstructed as an `ApplyToAll` over `result_dims`; treating
    // `matrix[d1,*]` as a scalar equation is a type error, so the lowered
    // reconstruction would fail and no per-read-row source link scores
    // would be emitted (silently zeroing the synthetic agg's loop score).
    // Mirrors the agg-aux emission above.
    let agg_eqn = if agg.result_dims.is_empty() {
        datamodel::Equation::Scalar(agg.equation_text.clone())
    } else {
        datamodel::Equation::ApplyToAll(agg.result_dims.clone(), agg.equation_text.clone())
    };
    let Some(agg_var) = reconstruct_ltm_var_lowered(db, &agg.name, &agg_eqn, model, project) else {
        return;
    };
    let Some(classified) = crate::ltm_augment::classify_reducer(&agg_var, from) else {
        return;
    };
    if classified.kind == crate::ltm_augment::ReducerKind::Constant {
        return;
    }
    // The body-aware partial context (GH #744). The agg's own equation is
    // exactly the reducer call (`classified.is_bare` is true by
    // construction), but its ARGUMENT may apply a coefficient to `from`
    // (`SUM(pop[*] * (1 - weight[*]))` w.r.t. `weight` has the
    // sign-flipping coefficient `-pop[e]`); the body context lets the
    // Linear arm build the true changed-first row partial instead of
    // asserting ∂agg/∂from[e] = 1.
    let (arrayed_dep_dims, model_deps) =
        reducer_body_ctx_parts(db, source_vars, project, &classified.body_text);
    let row_dim_names: Vec<String> = from_dims.iter().map(|d| d.name().to_string()).collect();
    let body_ctx = crate::ltm_augment::ReducerBodyCtx {
        body_text: &classified.body_text,
        live_source: from,
        arrayed_dep_dims: &arrayed_dep_dims,
        model_deps: &model_deps,
        row_dim_names: &row_dim_names,
        dims_ctx: Some(project_dimensions_context(db, project)),
        // The live source's accepted slice: resolves a mismatched-arity
        // feeder dep's index at the Iterated axis position -- sound and
        // LOAD-BEARING for a repeated-dim co-source like `matrix[D1,D1]`
        // read as `SUM(matrix[*, D1] * frac[D1])` (slice `[Reduced,
        // Iterated]`, result_dims `[D1]`: still minted, the GH #767 live
        // shape); see `resolve_mismatched_index_position`. Only the
        // DOUBLY-Iterated case (result_dims repeated) is declined at agg
        // minting (GH #778/#785) and never reaches here.
        live_read_slice: Some(agg.source_read_slice(from)),
    };
    let dim_element_lists: Vec<Vec<String>> = from_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    // The read rows: only the slice the reducer reads -- `from`'s OWN
    // (per-source) slice -- each row paired with its (possibly
    // mapping-remapped, GH #534) agg slot. If the slice doesn't line up
    // with `from`'s axes (shouldn't happen for a hoisted agg whose
    // `sources` include `from`; a non-source's empty slice trips the same
    // arity guard), fall back to all elements / scalar agg.
    let rows: Vec<ReadSliceRow> = read_slice_rows(
        agg.source_read_slice(from),
        &dim_element_lists,
        project_dimensions_context(db, project),
    )
    .unwrap_or_else(|| {
        let all = cartesian_subscripts(&dim_element_lists);
        all.iter()
            .map(|r| ReadSliceRow {
                row: r.clone(),
                slot: String::new(),
                coreduced: all.clone(),
            })
            .collect()
    });
    for ReadSliceRow {
        row,
        slot,
        coreduced,
    } in &rows
    {
        // Equation text uses qualified `dim·element` references (direct
        // LoadPrev, no helper auxes); names keep the bare element form.
        let qualified_row = crate::ltm_augment::qualify_element_csv(row, from_dims);
        let qualified_coreduced: Vec<String> = coreduced
            .iter()
            .map(|e| crate::ltm_augment::qualify_element_csv(e, from_dims))
            .collect();
        let (var_name, equation) = if slot.is_empty() {
            (
                format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}",
                    from, row, agg.name
                ),
                crate::ltm_augment::generate_element_to_scalar_equation(
                    from,
                    &agg.name,
                    &qualified_row,
                    &qualified_coreduced,
                    &classified.kind,
                    classified.name,
                    classified.is_bare,
                    Some(&body_ctx),
                ),
            )
        } else {
            (
                format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{}[{}]\u{2192}{}[{}]",
                    from, row, agg.name, slot
                ),
                crate::ltm_augment::generate_element_to_reduced_equation(
                    from,
                    &agg.name,
                    &qualified_row,
                    slot,
                    &qualified_coreduced,
                    &classified.kind,
                    classified.name,
                    classified.is_bare,
                    Some(&body_ctx),
                ),
            )
        };
        vars.push(LtmSyntheticVar {
            name: var_name,
            equation: datamodel::Equation::Scalar(equation),
            dimensions: vec![],
            // bracketed name (+ subscripted synthetic agg) -> routed direct.
            compile_directly: false,
        });
    }
}

/// Emit the `agg → to` link-score half: the partial of `to`'s equation
/// w.r.t. `agg` held live, with every hoisted reducer subexpression in
/// `to` first substituted by its agg name (so `agg` appears live where
/// `SUM(...)` was, and any other hoisted reducers end up as
/// `PREVIOUS(agg_j)`). For an arrayed `to` this is one scalar
/// `$⁚ltm⁚link_score⁚{agg}→{to}[{e}]` per target element (mirroring the
/// scalar→arrayed convention from `try_scalar_to_arrayed_link_scores`);
/// for a scalar `to` it is a single `$⁚ltm⁚link_score⁚{agg}→{to}`. When the
/// agg is itself arrayed over its `result_dims` (a partial-reduce
/// sub-expression like `x[D1] = ... + SUM(matrix[D1,*])`), the agg side of
/// the name carries the target element's projection onto `result_dims`
/// (`{agg}[{d1}]→{to}[{e}]`), and the agg reference in the per-slot
/// equation is element-pinned to the same slot.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_agg_to_target_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    agg_nodes: &crate::ltm_agg::AggNodesResult,
    agg: &crate::ltm_agg::AggNode,
    to: &str,
    model: SourceModel,
    project: SourceProject,
    vars: &mut Vec<LtmSyntheticVar>,
    unscoreable_edges: &mut HashSet<(String, String)>,
) {
    // GH #780: a `PartialEquationError` from any of this half's generators
    // dooms the whole `(agg, to)` edge -- a loop hop through it spells
    // `(agg.name, to)` (the form `traverses_unscoreable` strips to), so a
    // missing per-element name would stub the loop to a warned constant 0.
    // Each doom site below warns, inserts the edge, and returns WITHOUT
    // committing any of the edge's already-built per-element vars (the
    // collect-local `edge_vars` pattern), keeping the #758 contract: one
    // loud warning, no link-score variable for the edge, dependent loops
    // dropped. See `iterated_feeder_row_scores`' doc for why recording is
    // edge-level, not per-element.
    let Some(to_var) = reconstruct_single_variable(db, model, project, to) else {
        return;
    };
    let Some(ast) = to_var.ast() else { return };

    // Map of canonical reducer text -> agg name for every synthetic agg
    // occurring in `to`'s equation.
    let reducer_subst: HashMap<String, String> = agg_nodes
        .aggs_in_var(to)
        .filter(|a| a.is_synthetic)
        .map(|a| (a.equation_text.clone(), a.name.clone()))
        .collect();

    let agg_canonical = Ident::<Canonical>::new(&agg.name);
    let agg_is_arrayed = !agg.result_dims.is_empty();

    // The set of arrayed deps that share `to`'s dimensions (need to be
    // element-pinned in the per-target-element scalar equation); scalar
    // deps and the agg names are not. Computed over the original target's
    // dims and dep set, extended with the agg name (harmless if it never
    // appears).
    let to_dims = source_vars
        .get(to)
        .map(|sv| variable_dimensions(db, *sv, project).clone())
        .unwrap_or_default();
    use crate::ast::Ast;
    let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
        Ast::Scalar(_) => &[],
        Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
    };
    let base_deps = crate::variable::identifier_set(ast, target_ast_dims, None);
    let mut all_deps = base_deps.clone();
    all_deps.insert(agg_canonical.clone());
    // When `to` hoists 2+ reducers (e.g. `x = SUM(a[*]) / SUM(b[*])`), the
    // substituted equation text references *every* agg name, not just the
    // one this link starts from. The other aggs must be in `all_deps` so
    // they get PREVIOUS-wrapped (ceteris paribus) -- otherwise they are
    // left live and the agg→target link score collapses to ±1.
    for other_agg in reducer_subst.values() {
        all_deps.insert(Ident::<Canonical>::new(other_agg));
    }
    let deps_to_subscript: HashSet<Ident<Canonical>> = all_deps
        .iter()
        .filter(|d| {
            source_vars
                .get(d.as_str())
                .filter(|sv| sv.kind(db) != SourceVariableKind::Module)
                .map(|sv| {
                    let dd = variable_dimensions(db, *sv, project);
                    !dd.is_empty()
                        && dd
                            .iter()
                            .any(|x| to_dims.iter().any(|td| td.name() == x.name()))
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    // An arrayed agg (`result_dims` non-empty) is element-pinned in the
    // per-target-element equation too: `$⁚ltm⁚agg⁚0` → `$⁚ltm⁚agg⁚0[<slot>]`.
    // But NOT via `deps_to_subscript`: that set pins to `to`'s FULL element
    // tuple, which is an agg's correct subscript only in the diagonal case
    // (`result_dims` == `to`'s dims) and over-subscripts the agg in the
    // broadcast case (`agg[D1]` feeding `to[D1,D2]` -- the fragment then
    // fails to compile and the score is stubbed to a constant 0, zeroing
    // every loop through the agg; GH #528). Instead EACH arrayed agg --
    // the live one AND every frozen co-agg (GH #751) -- is pinned to the
    // target element's PROJECTION onto its own `result_dims` axes (for the
    // live agg, the same slot the link-score name and the `Δsource`
    // denominator carry) via `generate_scalar_to_element_equation`'s
    // per-ident `source_pins`, computed by `source_pins_for_target` below.

    // Positions of an agg's `result_dims` within `to`'s dimensions, so a
    // target element tuple can be projected onto that agg's slot subscript
    // (for the link-score name's agg side, and for the per-ident body pins).
    let positions_in_to_dims = |result_dims: &[String]| -> Vec<usize> {
        result_dims
            .iter()
            .filter_map(|rd| {
                let canon = crate::common::canonicalize(rd);
                to_dims.iter().position(|td| td.name() == canon.as_ref())
            })
            .collect()
    };
    let result_dim_positions: Vec<usize> = positions_in_to_dims(&agg.result_dims);
    // The target element projected onto the agg's `result_dims` (the
    // agg-slot subscript), or `None` when the agg is scalar / the
    // projection is empty.
    let agg_slot_for_target = |element: &str| -> Option<String> {
        if !agg_is_arrayed {
            return None;
        }
        let parts: Vec<&str> = element.split(',').collect();
        let slot: Vec<&str> = result_dim_positions
            .iter()
            .filter_map(|&p| parts.get(p).copied())
            .collect();
        (!slot.is_empty()).then(|| slot.join(","))
    };
    // The agg side of the link-score name for a given target element: the
    // bare agg name when scalar, `agg[<slot>]` when arrayed (the target
    // element projected onto `result_dims`).
    let agg_name_for_target = |element: &str| -> String {
        match agg_slot_for_target(element) {
            None => agg.name.clone(),
            Some(slot) => format!("{}[{}]", agg.name, slot),
        }
    };
    // The agg side of the `agg → target` link-score *equation*'s `Δsource`
    // denominator for a given target element: the quoted agg name with the
    // same `[<slot>]` subscript the link-score name carries (so the
    // denominator indexes the agg slot the loop traverses). The
    // bare-agg-name form `generate_scalar_to_element_equation` would build
    // by default does not compile when the agg is arrayed (a multi-slot
    // var referenced bare in a scalar equation).
    let agg_q = crate::ltm_augment::quote_ident(&agg.name);
    let agg_source_ref_for_target = |element: &str| -> String {
        match agg_slot_for_target(element) {
            None => agg_q.clone(),
            Some(slot) => format!("{agg_q}[{slot}]"),
        }
    };
    // The QUALIFIED (`d1·a`) projection of a target element onto an agg's
    // `result_dims` axes (given their precomputed positions in `to`'s dims)
    // -- the subscript that agg's ident is pinned to in the
    // per-target-element equation BODY (one `generate_scalar_to_element_equation`
    // `source_pins` entry). Qualified like the other pinned deps so
    // `PREVIOUS(agg[...])` references compile to direct LoadPrevs. `None`
    // when the projection is empty (a scalar agg; referenced bare, nothing
    // to pin).
    let qualified_pin_for_target = |positions: &[usize], element: &str| -> Option<String> {
        let qualified = crate::ltm_augment::qualify_element_csv(element, &to_dims);
        let parts: Vec<&str> = qualified.split(',').collect();
        let slot: Vec<&str> = positions
            .iter()
            .filter_map(|&p| parts.get(p).copied())
            .collect();
        (!slot.is_empty()).then(|| slot.join(","))
    };
    // Every ARRAYED agg referenced by the substituted equation needs a body
    // pin (GH #751): the LIVE agg (held live; the GH #528 projection) and
    // every frozen CO-AGG -- another hoisted reducer of the same target,
    // whose `PREVIOUS(B)` freeze is otherwise a bare multi-slot reference in
    // a scalar equation (fragment-compile failure, silently stubbed to 0).
    // Each ident is projected onto ITS OWN `result_dims` positions.
    // `aggs_in_var` yields first-encounter order, so the pin list -- and the
    // emitted equation text -- is deterministic.
    let arrayed_agg_pin_positions: Vec<(Ident<Canonical>, Vec<usize>)> = {
        let mut pins: Vec<(Ident<Canonical>, Vec<usize>)> = Vec::new();
        if agg_is_arrayed {
            pins.push((agg_canonical.clone(), result_dim_positions.clone()));
        }
        for other in agg_nodes.aggs_in_var(to) {
            if other.is_synthetic && other.name != agg.name && !other.result_dims.is_empty() {
                pins.push((
                    Ident::<Canonical>::new(&other.name),
                    positions_in_to_dims(&other.result_dims),
                ));
            }
        }
        pins
    };
    // The per-ident pin map for one target element: each arrayed agg pinned
    // to the target element's projection onto its own result axes.
    let source_pins_for_target = |element: &str| -> Vec<(Ident<Canonical>, String)> {
        arrayed_agg_pin_positions
            .iter()
            .filter_map(|(ident, positions)| {
                qualified_pin_for_target(positions, element).map(|slot| (ident.clone(), slot))
            })
            .collect()
    };

    // Helper: substitute the reducers in a slot expr's canonical text, to
    // build the agg→target link-score equation for one target element (or the
    // scalar case when `element` is `None`). Propagates a `PartialEquationError`
    // when the reducer substitution can't parse its input -- the caller skips
    // the variable and warns rather than emitting a partial that keeps the
    // inline reducer live instead of the hoisted agg node (GH #661).
    let slot_text =
        |expr: &crate::ast::Expr2| -> Result<String, crate::ltm_augment::PartialEquationError> {
            crate::ltm_augment::substitute_reducers_in_equation(
                &crate::patch::expr2_to_string(expr),
                &reducer_subst,
            )
        };

    match ast {
        Ast::Scalar(expr) => {
            // A scalar `to` cannot reference a reducer in a way that makes
            // the agg arrayed (a scalar target has no iterated dims), so
            // the agg is always scalar here.
            debug_assert!(!agg_is_arrayed, "a scalar target implies a scalar agg");
            let name = format!(
                "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
                agg.name, to
            );
            let substituted = match slot_text(expr) {
                Ok(substituted) => substituted,
                Err(err) => {
                    if unscoreable_edges.insert((agg.name.clone(), to.to_string())) {
                        emit_ltm_partial_equation_warning(db, model, &name, &err);
                    }
                    return;
                }
            };
            match crate::ltm_augment::generate_agg_to_scalar_target_equation(
                &agg.name,
                to,
                &substituted,
                &all_deps,
                Some(project_dimensions_context(db, project)),
            ) {
                Ok(equation) => vars.push(LtmSyntheticVar {
                    name,
                    equation: datamodel::Equation::Scalar(equation),
                    dimensions: vec![],
                    // synthetic agg on the `from` side -> routed direct already.
                    compile_directly: false,
                }),
                Err(err) => {
                    if unscoreable_edges.insert((agg.name.clone(), to.to_string())) {
                        emit_ltm_partial_equation_warning(db, model, &name, &err);
                    }
                }
            }
        }
        Ast::ApplyToAll(_, expr) => {
            // One shared body; emit one per-target-element scalar var. A
            // substitution parse failure here fails the whole edge (the body
            // is shared across every element), so warn once on the base
            // `agg → to` name and skip rather than emit a partial that keeps
            // the inline reducer live (GH #661).
            let substituted = match slot_text(expr) {
                Ok(substituted) => substituted,
                Err(err) => {
                    let name = format!(
                        "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
                        agg.name, to
                    );
                    if unscoreable_edges.insert((agg.name.clone(), to.to_string())) {
                        emit_ltm_partial_equation_warning(db, model, &name, &err);
                    }
                    return;
                }
            };
            if to_dims.is_empty() {
                return;
            }
            let dim_element_lists: Vec<Vec<String>> = to_dims
                .iter()
                .map(crate::ltm_augment::dimension_element_names)
                .collect();
            let mut edge_vars: Vec<LtmSyntheticVar> = Vec::new();
            for element in &cartesian_subscripts(&dim_element_lists) {
                // The partial is built around the *bare* agg names (which is
                // what `substituted` holds); `source_pins_for_target` then
                // pins each arrayed agg -- the live one AND any frozen
                // co-agg -- to its projected `agg[<slot>]`, matching
                // `agg_name_for_target` for the live agg (the full element
                // tuple would over-subscript an agg in the broadcast case;
                // GH #528, and the frozen-co-agg sibling GH #751). The
                // `Δsource` denominator carries the live agg's slot via the
                // explicit override.
                let name = format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
                    agg_name_for_target(element),
                    to,
                    element
                );
                match crate::ltm_augment::generate_scalar_to_element_equation(
                    &agg.name,
                    to,
                    // Qualified for equation text; the name keeps the bare form.
                    &crate::ltm_augment::qualify_element_csv(element, &to_dims),
                    &substituted,
                    &all_deps,
                    &deps_to_subscript,
                    Some(&agg_source_ref_for_target(element)),
                    &source_pins_for_target(element),
                    Some(project_dimensions_context(db, project)),
                ) {
                    Ok(equation) => edge_vars.push(LtmSyntheticVar {
                        name,
                        equation: datamodel::Equation::Scalar(equation),
                        dimensions: vec![],
                        // synthetic agg on `from` + bracketed `to` -> routed direct.
                        compile_directly: false,
                    }),
                    Err(err) => {
                        // One doomed element dooms the edge; drop the
                        // already-built per-element vars (`edge_vars` never
                        // commits) and warn only on first recording (#758
                        // convention) -- see the GH #780 note at the top.
                        if unscoreable_edges.insert((agg.name.clone(), to.to_string())) {
                            emit_ltm_partial_equation_warning(db, model, &name, &err);
                        }
                        return;
                    }
                }
            }
            vars.extend(edge_vars);
        }
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            if to_dims.is_empty() {
                return;
            }
            let dim_element_lists: Vec<Vec<String>> = to_dims
                .iter()
                .map(crate::ltm_augment::dimension_element_names)
                .collect();
            let mut edge_vars: Vec<LtmSyntheticVar> = Vec::new();
            for element in &cartesian_subscripts(&dim_element_lists) {
                let canonical_elem = crate::common::CanonicalElementName::from_raw(element);
                // Thread the slot expression through directly rather than
                // relying on the invariant that `substituted.is_empty()`
                // iff there is no slot expression.
                let equation = match per_elem.get(&canonical_elem).or(default_expr.as_ref()) {
                    None => Ok("0".to_string()),
                    // A substitution parse failure rides the same `Result` the
                    // equation builder returns, so the `match equation` below
                    // converts it into the per-element `Warning` + skip (GH
                    // #661) -- no separate error handling needed here.
                    Some(slot_expr) => slot_text(slot_expr).and_then(|substituted| {
                        // Re-derive per-slot deps (the union over all slots
                        // would over-freeze refs absent from this slot),
                        // then extend with this agg's name and every other
                        // agg referenced in the (substituted) slot text so
                        // they are all PREVIOUS-wrapped (ceteris paribus).
                        let mut slot_deps = crate::variable::classify_dependencies(
                            &Ast::Scalar(slot_expr.clone()),
                            target_ast_dims,
                            None,
                        )
                        .all;
                        slot_deps.insert(agg_canonical.clone());
                        for other_agg in reducer_subst.values() {
                            slot_deps.insert(Ident::<Canonical>::new(other_agg));
                        }
                        crate::ltm_augment::generate_scalar_to_element_equation(
                            &agg.name,
                            to,
                            // Qualified for equation text; the name keeps the bare form.
                            &crate::ltm_augment::qualify_element_csv(element, &to_dims),
                            &substituted,
                            &slot_deps,
                            &deps_to_subscript,
                            Some(&agg_source_ref_for_target(element)),
                            &source_pins_for_target(element),
                            Some(project_dimensions_context(db, project)),
                        )
                    }),
                };
                let name = format!(
                    "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
                    agg_name_for_target(element),
                    to,
                    element
                );
                match equation {
                    Ok(equation) => edge_vars.push(LtmSyntheticVar {
                        name,
                        equation: datamodel::Equation::Scalar(equation),
                        dimensions: vec![],
                        // synthetic agg on `from` + bracketed `to` -> routed direct.
                        compile_directly: false,
                    }),
                    Err(err) => {
                        // One doomed element dooms the edge; drop the
                        // already-built per-element vars (`edge_vars` never
                        // commits) and warn only on first recording (#758
                        // convention) -- see the GH #780 note at the top.
                        if unscoreable_edges.insert((agg.name.clone(), to.to_string())) {
                            emit_ltm_partial_equation_warning(db, model, &name, &err);
                        }
                        return;
                    }
                }
            }
            vars.extend(edge_vars);
        }
    }
}

/// Emit all link scores for a single variable-level causal edge
/// `(from, to)`: a synthetic-agg reroute (Phase 5) if `to` hoists a
/// reducer reading `from`, else a cross-dimensional (arrayed→scalar)
/// reducer split, else a scalar→arrayed split, else the per-shape
/// emission. The agg reroute still emits the non-reducer (Bare /
/// FixedIndex) shapes of `from` in `to` via `emit_per_shape_link_scores`
/// with the reducer shapes suppressed.
///
/// `skip_agg_halves` is set by the exhaustive loop-link caller: when a
/// loop traverses the *direct* `from → to` reference (e.g. the `pop[r]`
/// numerator in `share[r] = pop[r] / SUM(pop[*])`) the routed agg's two
/// halves (`from → agg`, `agg → to`) are emitted -- if at all -- by the
/// `agg_by_name` branches of that caller when the loop also traverses the
/// reducer path, so re-emitting them here would push duplicate
/// `LtmSyntheticVar`s into the `Vec`. The discovery / sub-model caller
/// passes `false` since it iterates causal edges (not loop links) and the
/// `from → agg`/`agg → to` edges aren't separately visited there.
///
/// `unscoreable_edges` is threaded to [`emit_per_shape_link_scores`]'s
/// GH #758 gate; see its doc.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_link_scores_for_edge(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    agg_nodes: &crate::ltm_agg::AggNodesResult,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
    dm_dims: &[crate::datamodel::Dimension],
    skip_agg_halves: bool,
    vars: &mut Vec<LtmSyntheticVar>,
    unscoreable_edges: &mut HashSet<(String, String)>,
) {
    if decline_bare_arrayed_reducer_target(db, model, project, from, to, unscoreable_edges) {
        return;
    }

    // The set of synthetic aggs `(from, to)` routes through, read off
    // the reference-site IR (the unique `ThroughAgg` `AggRef`s of this
    // edge's classified sites, in first-occurrence order). This is the
    // single place the old per-edge `routed_aggs` filter
    // (`aggs_in_var(to).filter(is_synthetic && reads from)`) used to be
    // restated -- it now lives only in the IR builder; here we just
    // project the result, resolving each `AggRef` to its `AggNode` for
    // the half-emitters.
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    let routed_aggs: Vec<&crate::ltm_agg::AggNode> = {
        let mut idxs: Vec<usize> = Vec::new();
        if let Some(sites) = ir.sites.get(&(from.to_string(), to.to_string())) {
            for s in sites {
                if let crate::db::ltm_ir::SiteRouting::ThroughAgg { agg } = &s.routing
                    && !idxs.contains(&agg.0)
                {
                    idxs.push(agg.0);
                }
            }
        }
        idxs.iter().map(|&i| &agg_nodes.aggs[i]).collect()
    };
    if !routed_aggs.is_empty() {
        // GH #793: a hoisted reducer sibling does not make the whole
        // `(from, to)` edge fully scoreable. If another reducer read of
        // `from` in `to` was not represented by one of these synthetic aggs
        // (for example an I1-declined strict slice), scoring only the agg
        // halves would publish incomplete attribution with no signal. Decline
        // at edge granularity instead, using the same warning/drop contract as
        // the no-agg strict-slice path.
        if decline_unhoisted_reducer_edge(db, model, project, from, to, unscoreable_edges) {
            for agg in &routed_aggs {
                unscoreable_edges.insert((from.to_string(), agg.name.clone()));
                unscoreable_edges.insert((agg.name.clone(), to.to_string()));
            }
            return;
        }
        if !skip_agg_halves {
            for agg in &routed_aggs {
                emit_source_to_agg_link_scores(
                    db,
                    source_vars,
                    from,
                    agg,
                    model,
                    project,
                    vars,
                    unscoreable_edges,
                );
                emit_agg_to_target_link_scores(
                    db,
                    source_vars,
                    agg_nodes,
                    agg,
                    to,
                    model,
                    project,
                    vars,
                    unscoreable_edges,
                );
            }
        }
        // The Bare numerator / FixedIndex references of `from` in `to`
        // still get their own (non-reducer) link scores. (And a bare
        // arrayed reducer arg like `SUM(pop)` keeps its `Bare`-named
        // link score too -- `emit_per_shape_link_scores` drops only the
        // `Wildcard` reducer-arg shape; see its doc.)
        emit_per_shape_link_scores(
            db,
            source_vars,
            from,
            to,
            RefShape::Bare,
            model,
            project,
            dm_dims,
            /* skip_reducer_shapes = */ true,
            vars,
            unscoreable_edges,
        );
        return;
    }
    // Cross-dimensional (arrayed-to-scalar / partial-reduce / broadcast-
    // reduce) edges -- includes the *variable-backed* reducer aggs like
    // `total = SUM(pop[*])` and the read-slice-derived shapes (GH #765 /
    // GH #777). `Some(vec![])` can mean the SIZE/Constant-reducer skip, the
    // GH #780 loud doom of a projection-feeder edge (a Warning was
    // accumulated and the edge recorded in `unscoreable_edges`), or the
    // GH #778/#785 duplicated-dim loud skip -- in every case we must NOT
    // fall through to `emit_per_shape_link_scores` (which would build a
    // wrong-shaped stand-in for an edge whose loops were already dropped).
    if let Some(cross_vars) = try_cross_dimensional_link_scores(
        db,
        source_vars,
        agg_nodes,
        from,
        to,
        model,
        project,
        unscoreable_edges,
    ) {
        vars.extend(cross_vars);
        return;
    }
    // Scalar-source -> arrayed-target edges (one scalar link score per
    // target element). `Some(vec![])` can mean the GH #780 loud decline: a
    // per-element partial doomed, the `Warning` was accumulated, and the
    // edge was recorded in `unscoreable_edges` (so loop scores through it
    // drop). We must NOT fall through to `emit_per_shape_link_scores`.
    if let Some(cross_vars) = try_scalar_to_arrayed_link_scores(
        db,
        source_vars,
        agg_nodes,
        from,
        to,
        model,
        project,
        dm_dims,
        unscoreable_edges,
    ) {
        vars.extend(cross_vars);
        return;
    }
    // Disjoint-dim arrayed -> arrayed edges with a per-element-equation
    // (`Ast::Arrayed`) target (GH #510): one link score per distinct
    // referenced source element, each `Equation::Arrayed` over `to`'s
    // dims. `Some(vec![])` means the edge is genuinely unscoreable (a
    // dynamic-index source): a `Warning` was accumulated, the edge was
    // recorded in `unscoreable_edges` (so loop scores through it are
    // dropped -- the GH #758 treatment), and no link score is emitted --
    // crucially, we *don't* fall through to `emit_per_shape_link_scores`,
    // which would build a scalarized stand-in.
    if let Some(disjoint_vars) = try_disjoint_dim_arrayed_link_scores(
        db,
        source_vars,
        from,
        to,
        model,
        project,
        unscoreable_edges,
    ) {
        vars.extend(disjoint_vars);
        return;
    }
    // GH #792: the PER-ELEMENT-EQUATION (`Ast::Arrayed`) owner whose slot
    // bodies read `from` inside a reducer (`share[nyc] = SUM(pop[nyc,*] *
    // w[*])` per slot -- and equally the dim-named `SUM(pop[Region,*] * w[*])`
    // and full-extent `SUM(pop[*,*] * w[*])` spellings). Such an owner cannot
    // take the cartesian arm of `try_cross_dimensional_link_scores`
    // (`classify_reducer` needs a single dt-expression; only an EXCEPT default
    // gets it there, where the same shared helper declines it), so before this
    // it fell to `emit_per_shape_link_scores` shape Bare, which minted ONE
    // arrayed `link_score:pop->share` simulating to ~-0.0 with no per-edge
    // warning -- other enumerated loops then consumed that silent near-zero.
    // We consult the SAME verdict the cartesian arm does (salsa-cached on the
    // interned `LtmLinkId`, so a re-visit is a cache hit). We can reach this
    // gate only with `routed_aggs` empty (else we returned at the agg branch
    // above), so no reducer reading `from` in `to` was hoisted -- ANY reducer
    // read the verdict finds is genuinely un-hoisted, and for a per-element
    // owner NO remaining derivation represents it: take the GH #758/#780 loud
    // skip (one warning naming the edge, no link-score variable, the edge
    // recorded so loops through it drop). NOTE this gate is NOT per-element-
    // only: a Scalar/A2A owner whose strict-slice reducer edge falls past the
    // try_* arms to this fallthrough (e.g. an equal-dims A2A owner with a
    // reducer SUBEXPRESSION, which the cartesian arm's dim-containment check
    // rejects) is declined here too via `StrictSlice` -- deliberate, the same
    // edge-granularity decline GH #791 applies on the cartesian arm. A
    // `FullExtent` / `NotDescribable` verdict keeps the existing per-shape
    // path byte-identical -- the disjoint-dim FixedIndex family (already
    // handled above), bare out-of-reducer refs, and the GH #525 PerElement
    // family all classify `NotDescribable` (a reference outside any reducer
    // is collected by neither slice walk), so none are touched.
    if decline_unhoisted_reducer_edge(db, model, project, from, to, unscoreable_edges) {
        return;
    }
    emit_per_shape_link_scores(
        db,
        source_vars,
        from,
        to,
        RefShape::Bare,
        model,
        project,
        dm_dims,
        /* skip_reducer_shapes = */ false,
        vars,
        unscoreable_edges,
    );
}
