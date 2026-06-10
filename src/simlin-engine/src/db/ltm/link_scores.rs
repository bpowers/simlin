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

use super::compile::link_score_equation_text_shaped;
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
    // edges are the diagonal". `mapped_element_correspondence` deliberately
    // accepts BOTH declaration directions (the bare-reference case needs
    // the source-declared one), but `classify_iterated_dim_shape` accepts a
    // SUBSCRIPTED mapped reference only in the forward direction: a
    // reverse-declared subscripted reference classifies `DynamicIndex` (GH
    // #757) and the element graph emits the conservative CROSS-PRODUCT, so
    // retargeting its (Bare-named, since Wildcard/DynamicIndex collapse
    // onto the Bare name) score to the target's dims would shape per-slot
    // DIAGONAL partials that the off-diagonal loop links then read by
    // target-element subscript -- silent wrong-slot values. Denied the
    // retarget, such an edge instead takes the GH #758 loud skip in
    // `emit_per_shape_link_scores` (no link-score variable, loop scores
    // through the edge dropped, one Warning). A mixed edge (a Bare site AND
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
/// Returns `None` for scalar-to-scalar, same-dimension A2A, broadcast
/// (`from_dims ⊆ to_dims`), mismatched dimensions, module-involved
/// edges, and any edge where the reducer cannot be classified. Returns
/// `Some(vec![])` for SIZE edges (constant reducer, no scores).
pub(super) fn try_cross_dimensional_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
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
    let body_ctx = crate::ltm_augment::ReducerBodyCtx {
        body_text: &classified.body_text,
        live_source: from,
        arrayed_dep_dims: &arrayed_dep_dims,
        model_deps: &model_deps,
        row_dim_names: &row_dim_names,
        dims_ctx: Some(project_dimensions_context(db, project)),
    };

    // Compute the cartesian product of all source dimensions to get
    // per-element subscripts. For a single dimension, this is just the
    // element names. For multi-dimensional sources (e.g., x[Region,Age]),
    // this produces tuples like "nyc,adult", "nyc,child", etc.
    let dim_element_lists: Vec<Vec<String>> = from_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
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
pub(super) fn try_scalar_to_arrayed_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    to: &str,
    model: SourceModel,
    project: SourceProject,
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
                None,
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
                cross_vars.extend(build_var(element, &elem_text, &elem_deps, &deps_to_sub));
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
                cross_vars.extend(build_var(element, &elem_text, &elem_deps, &deps_to_sub));
            }
        }
        Ast::Scalar(_) => unreachable!("target is arrayed"),
    }
    Some(cross_vars)
}

/// Accumulate the AC3.4 `Warning` for a disjoint-dim arrayed -> arrayed
/// edge that is not statically scoreable (the target's per-element
/// equations reference the source via a dynamic index, so which target
/// slots depend on which source elements can't be decided at compile
/// time). The edge gets *no* link-score variable, and -- like the GH #758
/// dim-incompatible class -- the caller records it in `unscoreable_edges`
/// so loop scores traversing it are dropped (their product could only be
/// a guaranteed-zero stub). Sub-model *pathway* scores referencing the
/// missing name still get the fragment compiler's zero-contribution
/// stub-dep fallback. Both are far less misleading than the scalarized
/// stand-in the pre-#510 path produced.
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
         dynamic index, which is not statically scoreable; this edge will have no \
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
/// with `model_ltm_fragment_diagnostics`: that pass only catches synthetic
/// equations that fail to *compile*, whereas the silent-fallback bug this
/// replaces produced an equation that compiled cleanly while computing a
/// constant `|Δz/Δz| = 1` magnitude.
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
/// GH #311 parse failure, or a GH #743 unfreezable partial (neither
/// ceteris-paribus convention can be rendered as a compilable equation).
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
    }
}

/// Emit per-distinct-source-element link scores for a disjoint-dim
/// arrayed -> arrayed edge whose target is a per-element-equation
/// (`Ast::Arrayed`) variable (GH #510), or return `None` if the edge is
/// not of that shape.
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
    // The target must be a per-element-equation (`Ast::Arrayed`) variable.
    // An `Ast::ApplyToAll` target referencing a disjoint-dim source would
    // be a dimension error (a D3-shaped value can't broadcast onto D1xD2);
    // a scalar source into an arrayed target is `try_scalar_to_arrayed`'s job.
    let to_var = reconstruct_single_variable(db, model, project, to)?;
    if !matches!(to_var.ast(), Some(crate::ast::Ast::Arrayed(..))) {
        return None;
    }
    // Consult the reference-site IR. A `FixedIndex(elems)` site is a
    // `from[m]` reference; anything else (a dynamic index, or -- defensively
    // -- a `Wildcard`/`Bare`, which can't be a valid literal-element
    // reference into a disjoint-dim target) makes the edge unscoreable.
    let ir = crate::db::ltm_ir::model_ltm_reference_sites(db, model, project);
    let sites = ir.sites.get(&(from.to_string(), to.to_string()))?;
    if sites.is_empty() {
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
            RefShape::Bare | RefShape::Wildcard | RefShape::DynamicIndex => {
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
        if let Some(mut lsv) =
            link_score_equation_text_shaped(db, link_id, shape.clone(), model, project).clone()
        {
            // `lsv.name` is already `link_score_var_name(from, to, &shape)`
            // from the shaped path -- no need to re-derive it here.
            let dims = ltm_equation_dimensions(&lsv.equation).to_vec();
            lsv.dimensions = dims.clone();
            lsv.equation = retarget_ltm_equation_dims(lsv.equation, &dims);
            lsv.compile_directly = false;
            vars.push(lsv);
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
    // Distinct `RefShape`s can map to the same synthetic name (a
    // conservative-slice / direct-dynamic-index `Wildcard`/`DynamicIndex`
    // ref shares the canonical Bare name now that the per-shape suffix is
    // retired); keep only the first shape that produces each name.
    let mut emitted_names: HashSet<String> = HashSet::new();
    shapes.retain(|shape| {
        emitted_names.insert(crate::ltm_augment::link_score_var_name(from, to, shape))
    });
    if shapes.is_empty() {
        // Every shape was a suppressed reducer-arg Wildcard: nothing to
        // emit (the agg halves carry the edge's scores).
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
    // through it. The declined mapped sliced reducers (element-mapped GH
    // #756, reverse-declared GH #757) land here, as do disjoint-dim
    // ApplyToAll-target references and incompatible-dim dynamic-index
    // reducers -- all previously warned zero-stubs.
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
        return;
    }

    for shape in shapes {
        let link_id = LtmLinkId::new(db, from.to_string(), to.to_string());
        if let Some(mut lsv) =
            link_score_equation_text_shaped(db, link_id, shape.clone(), model, project).clone()
        {
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
pub(super) fn emit_source_to_agg_link_scores(
    db: &dyn Db,
    source_vars: &HashMap<String, SourceVariable>,
    from: &str,
    agg: &crate::ltm_agg::AggNode,
    model: SourceModel,
    project: SourceProject,
    vars: &mut Vec<LtmSyntheticVar>,
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
            Err(err) => emit_ltm_partial_equation_warning(db, model, &name, &err),
        }
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
    };
    let dim_element_lists: Vec<Vec<String>> = from_dims
        .iter()
        .map(crate::ltm_augment::dimension_element_names)
        .collect();
    // The read rows: only the slice the reducer reads, each row paired with
    // its (possibly mapping-remapped, GH #534) agg slot. If `read_slice`
    // doesn't line up with `from`'s axes (shouldn't happen for a hoisted
    // agg whose `source_vars` contains `from`), fall back to all elements
    // / scalar agg.
    let rows: Vec<ReadSliceRow> = read_slice_rows(
        &agg.read_slice,
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
) {
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
    // tuple, which is the agg's correct subscript only in the diagonal case
    // (`result_dims` == `to`'s dims) and over-subscripts the agg in the
    // broadcast case (`agg[D1]` feeding `to[D1,D2]` -- the fragment then
    // fails to compile and the score is stubbed to a constant 0, zeroing
    // every loop through the agg; GH #528). Instead the agg is pinned to the
    // target element's PROJECTION onto `result_dims`'s axes -- the same slot
    // the link-score name and the `Δsource` denominator carry -- via
    // `generate_scalar_to_element_equation`'s `source_pin_element`, computed
    // by `agg_pin_for_target` below.

    // Positions of the agg's `result_dims` within `to`'s dimensions, so a
    // target element tuple can be projected onto the agg-slot subscript
    // for the link-score name's agg side.
    let result_dim_positions: Vec<usize> = agg
        .result_dims
        .iter()
        .filter_map(|rd| {
            let canon = crate::common::canonicalize(rd);
            to_dims.iter().position(|td| td.name() == canon.as_ref())
        })
        .collect();
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
    // The QUALIFIED (`d1·a`) projection of a target element onto the agg's
    // `result_dims` axes -- the subscript the agg ident is pinned to in the
    // per-target-element equation BODY (`generate_scalar_to_element_equation`'s
    // `source_pin_element`). Qualified like the other pinned deps so
    // `PREVIOUS(agg[...])` references compile to direct LoadPrevs. `None`
    // when the agg is scalar (referenced bare; nothing to pin).
    let agg_pin_for_target = |element: &str| -> Option<String> {
        if !agg_is_arrayed {
            return None;
        }
        let qualified = crate::ltm_augment::qualify_element_csv(element, &to_dims);
        let parts: Vec<&str> = qualified.split(',').collect();
        let slot: Vec<&str> = result_dim_positions
            .iter()
            .filter_map(|&p| parts.get(p).copied())
            .collect();
        (!slot.is_empty()).then(|| slot.join(","))
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
                    emit_ltm_partial_equation_warning(db, model, &name, &err);
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
                Err(err) => emit_ltm_partial_equation_warning(db, model, &name, &err),
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
                    emit_ltm_partial_equation_warning(db, model, &name, &err);
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
            for element in &cartesian_subscripts(&dim_element_lists) {
                // The partial is built around the *bare* agg name (which is
                // what `substituted` holds); `agg_pin_for_target` then pins
                // it to the projected `agg[<slot>]` when the agg is arrayed,
                // matching `agg_name_for_target` (the full element tuple
                // would over-subscript the agg in the broadcast case; GH
                // #528). The `Δsource` denominator carries the same slot
                // via the explicit override.
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
                    agg_pin_for_target(element).as_deref(),
                    Some(project_dimensions_context(db, project)),
                ) {
                    Ok(equation) => vars.push(LtmSyntheticVar {
                        name,
                        equation: datamodel::Equation::Scalar(equation),
                        dimensions: vec![],
                        // synthetic agg on `from` + bracketed `to` -> routed direct.
                        compile_directly: false,
                    }),
                    Err(err) => emit_ltm_partial_equation_warning(db, model, &name, &err),
                }
            }
        }
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            if to_dims.is_empty() {
                return;
            }
            let dim_element_lists: Vec<Vec<String>> = to_dims
                .iter()
                .map(crate::ltm_augment::dimension_element_names)
                .collect();
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
                            agg_pin_for_target(element).as_deref(),
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
                    Ok(equation) => vars.push(LtmSyntheticVar {
                        name,
                        equation: datamodel::Equation::Scalar(equation),
                        dimensions: vec![],
                        // synthetic agg on `from` + bracketed `to` -> routed direct.
                        compile_directly: false,
                    }),
                    Err(err) => emit_ltm_partial_equation_warning(db, model, &name, &err),
                }
            }
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
        if !skip_agg_halves {
            for agg in &routed_aggs {
                emit_source_to_agg_link_scores(db, source_vars, from, agg, model, project, vars);
                emit_agg_to_target_link_scores(
                    db,
                    source_vars,
                    agg_nodes,
                    agg,
                    to,
                    model,
                    project,
                    vars,
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
    // Cross-dimensional (arrayed-to-scalar) edges -- includes the
    // *variable-backed* reducer aggs like `total = SUM(pop[*])`.
    if let Some(cross_vars) =
        try_cross_dimensional_link_scores(db, source_vars, from, to, model, project)
    {
        vars.extend(cross_vars);
        return;
    }
    // Scalar-source -> arrayed-target edges (one scalar link score per
    // target element).
    if let Some(cross_vars) =
        try_scalar_to_arrayed_link_scores(db, source_vars, from, to, model, project)
    {
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
