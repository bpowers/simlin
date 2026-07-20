// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `#[cfg(test)]` occurrence-IR reconstruction for the ceteris-paribus wrap
//! unit tests.
//!
//! Production drives the wrap from the salsa occurrence IR
//! (`db::ltm_ir::model_ltm_reference_sites`). The focused text-in/text-out wrap
//! unit tests (`build_partial_equation_shaped` and the direct generator tests)
//! have no db, so these helpers rebuild an equivalent per-slot occurrence stream
//! on the reparsed `Expr0` -- using the `#[cfg(test)]` Expr0 classifiers the
//! alignment gate (`ltm_classifier_agreement_tests`) proves stay in step with
//! the IR -- so those tests keep exercising the real production wrap with
//! byte-identical inputs. Split out of `ltm_augment.rs` only to keep that file
//! under the project line-count lint; included via `#[path]`, so `super::*`
//! resolves the parent's private items.

use super::*;
use crate::db::ltm_ir::OccurrenceRef;

/// Reconstruct the occurrence-IR stream ([`OccurrenceSite`]s) for one target
/// equation's parsed `Expr0`, for the `#[cfg(test)]` wrap unit tests
/// ([`build_partial_equation_shaped`]). Production gets the stream from
/// `db::ltm_ir::model_ltm_reference_sites` (the `Expr2` walk); this rebuilds an
/// equivalent stream on the parsed `Expr0` so those focused text-in/text-out
/// tests keep exercising the real production wrap with byte-identical inputs.
///
/// The paths start at slot `0` (mirroring `walk_all_in_expr`'s single-body slot
/// push), so [`OccurrenceLookup::for_slot`]`(.., 0)` rebases them to the
/// slot-local paths the wrap tracks. Occurrences are recorded for every ident in
/// `deps` ∪ `{live_source}` (the idents the wrap acts on); the live source's
/// shape/axes come from the Expr0 classifier, an other-dep's axes from
/// [`other_dep_occurrence_axes`] (only its verdict is consumed).
#[cfg(test)]
pub(crate) fn build_wrap_test_occurrences(
    ast: &Expr0,
    live_source: &Ident<Canonical>,
    deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Vec<OccurrenceSite> {
    let mut recorded = deps.clone();
    recorded.insert(live_source.clone());
    let mut out = Vec::new();
    // Slot 0 for a scalar/A2A body (matching `walk_all_in_expr`).
    let mut path = vec![0u16];
    walk_wrap_test_occurrences(
        ast,
        live_source,
        &recorded,
        source_dim_elements,
        iter_ctx,
        dim_ctx,
        false,
        &mut path,
        &mut out,
    );
    out
}

/// Build the occurrence stream for a whole target `Variable` (all slots), for
/// the `#[cfg(test)]` generator tests that construct a `Variable` directly (no
/// db). Mirrors `walk_all_in_expr`'s slot numbering: a scalar/A2A body is slot
/// `0`; an `Ast::Arrayed` target's per-element slots are canonical
/// element-key-sorted, then the default. Records references to `from` (the live
/// source); those tests reference only the source, and `iter_ctx = None` is
/// sufficient for their FixedIndex / Bare shapes.
#[cfg(test)]
pub(crate) fn test_occurrences_for_var(
    to_var: &Variable,
    from: &Ident<Canonical>,
    source_dim_elements: &[Vec<String>],
) -> Vec<OccurrenceSite> {
    use crate::ast::Ast;
    let recorded: HashSet<Ident<Canonical>> = std::iter::once(from.clone()).collect();
    let mut out = Vec::new();
    let Some(ast) = to_var.ast() else {
        return out;
    };
    let walk_slot = |expr: &crate::ast::Expr2, slot: u16, out: &mut Vec<OccurrenceSite>| {
        let e0 = crate::patch::expr2_to_expr0(expr);
        let mut path = vec![slot];
        walk_wrap_test_occurrences(
            &e0,
            from,
            &recorded,
            source_dim_elements,
            None,
            None,
            false,
            &mut path,
            out,
        );
    };
    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
            walk_slot(expr, 0, &mut out);
        }
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            let mut keys: Vec<_> = per_elem.keys().collect();
            keys.sort();
            for (slot, k) in keys.iter().enumerate() {
                walk_slot(&per_elem[*k], slot as u16, &mut out);
            }
            if let Some(def) = default_expr {
                walk_slot(def, keys.len() as u16, &mut out);
            }
        }
    }
    out
}

/// The per-axis classification of one index of a LIVE-SOURCE subscript, mirroring
/// `db::ltm_ir::classify_occurrence_axes`'s arm order: an ITERATED-dimension name
/// that lines up with the source's axis at this position wins first (a dimension
/// name is not a literal element, so testing the literal arm first would
/// mis-classify it `Dynamic`), then a literal element, then dynamic.
///
/// The row-pinning lowering reads these axes to decide which index is a
/// projected coordinate and which is a fixed literal, so a builder that only
/// ever produced `Pinned`/`Dynamic` would leave every iterated axis un-pinned --
/// silently, and only in the tests, which is the worst place for a divergence
/// from production to hide.
#[cfg(test)]
fn live_source_occurrence_axis(
    idx: &IndexExpr0,
    i: usize,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> OccurrenceAxis {
    if let (Some(ic), IndexExpr0::Expr(Expr0::Var(name, _))) = (iter_ctx, idx) {
        let d = crate::canonicalize(name.as_str()).into_owned();
        if ic.target_iterated_dims.iter().any(|t| t == &d)
            && i < ic.source_dim_names.len()
            && expr0_iterated_axis_lines_up(&d, i, source_dim_elements, ic, dim_ctx)
        {
            return OccurrenceAxis::Iterated {
                dim: d,
                source_dim: ic.source_dim_names[i].clone(),
            };
        }
    }
    match resolve_literal_element_index(idx, i, source_dim_elements) {
        Some(e) => OccurrenceAxis::Pinned(e),
        None => OccurrenceAxis::Dynamic,
    }
}

/// One synthesized test occurrence: only `site_id`, `reference`, `shape`,
/// `axes`, and `index_nested` are read by the wrap; the rest carry inert
/// defaults.
#[cfg(test)]
fn test_occurrence(
    ident: &Ident<Canonical>,
    path: &[u16],
    shape: RefShape,
    axes: Vec<OccurrenceAxis>,
    index_nested: bool,
) -> OccurrenceSite {
    OccurrenceSite {
        site_id: crate::db::ltm_ir::SiteId(path.to_vec().into_boxed_slice()),
        reference: OccurrenceRef::Variable(ident.as_str().to_string()),
        shape,
        axes,
        in_reducer: false,
        index_nested,
    }
}

/// Recursive walker for [`build_wrap_test_occurrences`], mirroring
/// `db::ltm_ir::walk_all_in_expr`'s child-index path construction (and its
/// sticky `index_nested` flag) so the synthesized occurrences land at the
/// SiteId paths the wrap tracks and carry the `index_nested` bit
/// [`OccurrenceLookup::subtree_has_live_shape`] filters on.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn walk_wrap_test_occurrences(
    expr: &Expr0,
    live_source: &Ident<Canonical>,
    recorded: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
    index_nested: bool,
    path: &mut Vec<u16>,
    out: &mut Vec<OccurrenceSite>,
) {
    let recurse = |sub: &Expr0, index_nested: bool, path: &mut Vec<u16>, out: &mut _| {
        walk_wrap_test_occurrences(
            sub,
            live_source,
            recorded,
            source_dim_elements,
            iter_ctx,
            dim_ctx,
            index_nested,
            path,
            out,
        );
    };
    match expr {
        Expr0::Const(..) => {}
        Expr0::Var(name, _) => {
            let c = Ident::<Canonical>::new(name.as_str());
            if recorded.contains(&c) {
                out.push(test_occurrence(
                    &c,
                    path,
                    RefShape::Bare,
                    Vec::new(),
                    index_nested,
                ));
            }
        }
        Expr0::Subscript(name, indices, _) => {
            let c = Ident::<Canonical>::new(name.as_str());
            if recorded.contains(&c) {
                let (shape, axes) = if &c == live_source {
                    let shape = classify_expr0_subscript_shape(
                        indices,
                        source_dim_elements,
                        iter_ctx,
                        dim_ctx,
                    );
                    let axes = indices
                        .iter()
                        .enumerate()
                        .map(|(i, idx)| {
                            live_source_occurrence_axis(
                                idx,
                                i,
                                source_dim_elements,
                                iter_ctx,
                                dim_ctx,
                            )
                        })
                        .collect();
                    (shape, axes)
                } else {
                    // Other dep: only the verdict (from its axes) is consumed.
                    (
                        RefShape::Bare,
                        other_dep_occurrence_axes(&c, indices, iter_ctx, dim_ctx),
                    )
                };
                out.push(test_occurrence(&c, path, shape, axes, index_nested));
            }
            // Descending into a subscript index sets `index_nested` sticky-true.
            for (i, idx) in indices.iter().enumerate() {
                path.push(i as u16);
                match idx {
                    IndexExpr0::Expr(e) => recurse(e, true, path, out),
                    IndexExpr0::Range(l, r, _) => {
                        path.push(0);
                        recurse(l, true, path, out);
                        path.pop();
                        path.push(1);
                        recurse(r, true, path, out);
                        path.pop();
                    }
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
                path.pop();
            }
        }
        Expr0::App(UntypedBuiltinFn(fname, args), _) => {
            let lname = fname.to_ascii_lowercase();
            let skip_first = matches!(
                lname.as_str(),
                "lookup" | "lookup_forward" | "lookup_backward"
            ) && !args.is_empty();
            for (i, a) in args.iter().enumerate() {
                if skip_first && i == 0 {
                    continue;
                }
                path.push(i as u16);
                recurse(a, index_nested, path, out);
                path.pop();
            }
        }
        Expr0::Op1(_, inner, _) => {
            path.push(0);
            recurse(inner, index_nested, path, out);
            path.pop();
        }
        Expr0::Op2(_, l, r, _) => {
            path.push(0);
            recurse(l, index_nested, path, out);
            path.pop();
            path.push(1);
            recurse(r, index_nested, path, out);
            path.pop();
        }
        Expr0::If(c, t, e, _) => {
            for (child, sub) in [c, t, e].into_iter().enumerate() {
                path.push(child as u16);
                recurse(sub, index_nested, path, out);
                path.pop();
            }
        }
    }
}

// ── The `#[cfg(test)]` Expr0 access-shape classifier family ────────────────
//
// These live HERE, beside their only consumer, rather than in `ltm_augment.rs`.
// Production decides access shape exactly once, on the target's `Expr2` AST
// (`db::ltm_ir`), and both consumers -- causal-edge emission and the
// ceteris-paribus wrap -- read that one classification. What survives below is a
// reconstruction of an occurrence stream on a parsed `Expr0`, so the focused
// text-in/text-out wrap unit tests can drive the real production wrap without a
// db. Keeping it in the test-support module makes that status structural rather
// than a `#[cfg(test)]` attribute a reader has to notice, and keeps
// `ltm_augment.rs` under the project line cap.

/// Recognize an *iterated-dimension* `Expr0` subscript on the *live source*
/// -- one whose indices are exactly the target equation's iterated
/// dimensions, in the position matching the live source's declared
/// dimension order -- the Expr0-AST sibling of
/// `db::ltm_ir::classify_iterated_dim_shape` (GH #511).
///
/// `live_source[d_0, d_1, ...]` is the iterated-dim case iff:
///   1. it has exactly one index per source dimension (`indices.len() ==
///      ctx.source_dim_names.len()`), and
///   2. every index `d_i` is a bare `Var` naming a dimension that is one of
///      the target equation's iterated dimensions, *and*
///   3. for each `i`, `d_i` is either the same name as the source's `i`-th
///      declared dimension `ctx.source_dim_names[i]`, or (when a
///      `DimensionsContext` is available) a dimension that *maps to* it (the
///      AC3.5 mapped-dimension case).
///
/// When it matches, `wrap_non_matching_in_previous` collapses the subscript
/// to a bare `Var(live_source)` before the live/PREVIOUS dispatch -- it then
/// becomes the live ref (`live_shape == Bare`) or (when `live_shape != Bare`,
/// which shouldn't happen for an edge the IR classified `Bare`) a
/// `PREVIOUS(Var(live_source))` (a `Var` arg, which codegen accepts -- vs
/// the `PREVIOUS(Subscript(...))` the pre-fix code produced, which trips the
/// codegen assertion). The model equation itself is untouched -- only the
/// LTM partial's `Expr0` is normalized -- so simulation still evaluates
/// `live_source[d_i]` correctly: in this slot, `live_source[d_i]` and a bare
/// `live_source` reference inside an apply-to-all-over-the-target's-dims
/// equation pick the same element (the bare ref broadcasts/iterates that
/// dimension).
///
/// `#[cfg(test)]`: production reads the shape straight off the occurrence IR
/// (an iterated-dim subscript is classified `Bare` there); this Expr0 sibling
/// survives only for the wrap unit tests' occurrence builder and the alignment
/// gate.
#[cfg(test)]
pub(crate) fn is_live_source_iterated_dim_subscript(
    indices: &[IndexExpr0],
    source_dim_elements: &[Vec<String>],
    ctx: Option<&IteratedDimCtx<'_>>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> bool {
    let Some(ctx) = ctx else { return false };
    if indices.is_empty() || indices.len() != ctx.source_dim_names.len() {
        return false;
    }
    for (i, idx) in indices.iter().enumerate() {
        let d = match idx {
            IndexExpr0::Expr(Expr0::Var(name, _)) => canonicalize(name.as_str()).into_owned(),
            _ => return false,
        };
        if !ctx.target_iterated_dims.iter().any(|t| t == &d) {
            return false;
        }
        if !expr0_iterated_axis_lines_up(&d, i, source_dim_elements, ctx, dim_ctx) {
            return false;
        }
    }
    true
}

/// Does iterated-dimension index `d` (canonical) line up with the live
/// source's `i`-th axis -- by name, or through a usable positional-mapping
/// remap? The mapped arm consults the SAME
/// [`crate::ltm_agg::iterated_axis_slot_elements`] /
/// `mapped_element_correspondence` gate the Expr2 classifier
/// (`ltm_agg::classify_axis_access`) uses -- BOTH declaration directions
/// (GH #757), positional mappings only (GH #756) -- so the partial
/// builder's live-shape match and the reference-site IR agree by
/// construction. (No mapping context ⇒ no mapped recognition; the by-name
/// check still applies.)
#[cfg(test)]
pub(crate) fn expr0_iterated_axis_lines_up(
    d: &str,
    i: usize,
    source_dim_elements: &[Vec<String>],
    ctx: &IteratedDimCtx<'_>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> bool {
    let src_name = &ctx.source_dim_names[i];
    if d == src_name.as_str() {
        return true;
    }
    let Some(dim_ctx) = dim_ctx else {
        return false;
    };
    let Some(elems) = source_dim_elements.get(i) else {
        return false;
    };
    crate::ltm_agg::iterated_axis_slot_elements(d, src_name, elems, dim_ctx).is_some()
}

/// The per-axis [`crate::ltm_agg::AxisRead`] vector of a subscript whose
/// every index is either an iterated-dimension name lined up with the
/// source's axis at that position or a literal element of that axis
/// (position-STRICT, matching the Expr2 side's per-axis
/// `resolve_literal_axis_index`) -- the Expr0 sibling of the
/// `classify_axis_access`-derived classification
/// `db::ltm_ir::classify_iterated_dim_shape` performs, minus the `Reduced`
/// arm (a wildcard/StarRange index returns `None`; direct references never
/// collapse an axis, and the `Wildcard` precedence check already ran).
/// `None` when any index is neither -- the caller falls through to the
/// legacy literal pass / `DynamicIndex`.
///
/// `#[cfg(test)]` like the rest of this family. It used to run in PRODUCTION, in
/// the `PerElement` row-pinning lowering, because that lowering was a pass over
/// the ALREADY-WRAPPED `Expr0` -- a tree the wrap mutated by inserting
/// `PREVIOUS(...)` nodes, so its child-index structure no longer matched the
/// target's ORIGINAL `Expr2` and a `SiteId` computed on that original AST could
/// not address it. Folding the pinning INTO the wrap removed that constraint:
/// there the occurrence is still reachable by path, so the per-axis truth comes
/// off `OccurrenceSite::axes`
/// (`post_transform::pin_source_subscript_indices`) and this classifier is no
/// longer part of any production decision.
#[cfg(test)]
pub(crate) fn classify_expr0_per_element_axes(
    indices: &[IndexExpr0],
    source_dim_elements: &[Vec<String>],
    ctx: &IteratedDimCtx<'_>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Option<Vec<crate::ltm_agg::AxisRead>> {
    use crate::ltm_agg::AxisRead;
    if indices.is_empty()
        || indices.len() != ctx.source_dim_names.len()
        || indices.len() != source_dim_elements.len()
    {
        return None;
    }
    let mut axes = Vec::with_capacity(indices.len());
    for (i, idx) in indices.iter().enumerate() {
        if let IndexExpr0::Expr(Expr0::Var(name, _)) = idx {
            let d = canonicalize(name.as_str()).into_owned();
            if ctx.target_iterated_dims.iter().any(|t| t == &d) {
                if !expr0_iterated_axis_lines_up(&d, i, source_dim_elements, ctx, dim_ctx) {
                    return None;
                }
                axes.push(AxisRead::Iterated {
                    dim: d,
                    source_dim: ctx.source_dim_names[i].clone(),
                });
                continue;
            }
        }
        // Position-strict literal resolution: the Expr2 classifier resolves
        // each index against ITS axis only, so the any-dimension fallback
        // `resolve_literal_element_index` carries (for the legacy
        // FixedIndex match) must not apply here -- a cross-axis literal
        // would build a `Pinned` the IR never minted, breaking the
        // live-shape equality match.
        let candidate = match idx {
            IndexExpr0::Expr(Expr0::Var(name, _)) => canonicalize(name.as_str()).into_owned(),
            IndexExpr0::Expr(Expr0::Const(s, _, _)) => s.parse::<u32>().ok()?.to_string(),
            _ => return None,
        };
        if !source_dim_elements[i].iter().any(|e| e == &candidate) {
            return None;
        }
        axes.push(AxisRead::Pinned(candidate));
    }
    Some(axes)
}

/// Build the per-axis [`OccurrenceAxis`] classification of an iterated-dimension
/// subscript on a *non-live-source* dependency (e.g. `pop[Region,Age]` in
/// `growth[Region,Age] = row_sum[Region] * c * pop[Region,Age]`), the way
/// `db::ltm_ir::classify_occurrence_axes` classifies it on the `Expr2` AST.
///
/// This is the Expr0 sibling that lets the `#[cfg(test)]` occurrence builder
/// ([`build_wrap_test_occurrences`]) reconstruct an occurrence's `axes` for the
/// wrap unit tests. PRODUCTION reads the axes straight off the occurrence IR
/// ([`OccurrenceLookup`]) and feeds them to the single-sourced
/// [`derive_other_dep_verdict`] via [`other_dep_verdict`] -- there is no Expr0
/// re-derivation of the verdict on the live path, so the two families cannot
/// drift.
///
/// Each index that names (or positionally maps to) the dep's declared axis at
/// that position is `Iterated`, a target-iterated name that does NOT line up is
/// `MismatchedIterated` (the GH #526 case a bare collapse must not silently
/// freeze), an over-dep-arity target-iterated name is `Iterated{d,d}` (the
/// verdict's arity guard dominates it), and any non-target-iterated-Var index is
/// `Dynamic` (which makes `derive_other_dep_verdict` return `NotIterated`,
/// matching the pre-flip recognizer's short-circuit).
#[cfg(test)]
pub(crate) fn other_dep_occurrence_axes(
    dep: &Ident<Canonical>,
    indices: &[IndexExpr0],
    ctx: Option<&IteratedDimCtx<'_>>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Vec<OccurrenceAxis> {
    let Some(ctx) = ctx else {
        return Vec::new();
    };
    let dep_dims = ctx.dep_dims.and_then(|m| m.get(dep.as_str()));
    indices
        .iter()
        .enumerate()
        .map(|(i, idx)| {
            let IndexExpr0::Expr(Expr0::Var(name, _)) = idx else {
                return OccurrenceAxis::Dynamic;
            };
            let d = canonicalize(name.as_str()).into_owned();
            if !ctx.target_iterated_dims.iter().any(|t| t == &d) {
                return OccurrenceAxis::Dynamic;
            }
            match dep_dims.and_then(|dd| dd.get(i)) {
                Some(dep_dim) if other_dep_axis_lines_up(&d, dep_dim, dim_ctx) => {
                    OccurrenceAxis::Iterated {
                        dim: d,
                        source_dim: canonicalize(dep_dim.name()).into_owned(),
                    }
                }
                Some(_) => OccurrenceAxis::MismatchedIterated { dim: d },
                None => OccurrenceAxis::Iterated {
                    dim: d.clone(),
                    source_dim: d,
                },
            }
        })
        .collect()
}

/// Does iterated-dimension index `d` (canonical) line up with a non-live
/// dep's declared axis `dep_dim` -- by name, or through a usable
/// positional-mapping remap? The dep-side sibling of
/// [`expr0_iterated_axis_lines_up`], consulting the SAME
/// [`crate::ltm_agg::iterated_axis_slot_elements`] /
/// `mapped_element_correspondence` gate (both declaration directions,
/// positional mappings only) so the live-source and other-dep recognizers
/// can never disagree about which mapped pairs are usable.
#[cfg(test)]
pub(crate) fn other_dep_axis_lines_up(
    d: &str,
    dep_dim: &crate::dimensions::Dimension,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> bool {
    let dep_dim_name = canonicalize(dep_dim.name());
    if d == dep_dim_name.as_ref() {
        return true;
    }
    let Some(dim_ctx) = dim_ctx else {
        return false;
    };
    let elems = dimension_element_names(dep_dim);
    crate::ltm_agg::iterated_axis_slot_elements(d, dep_dim_name.as_ref(), &elems, dim_ctx).is_some()
}

/// Whether a single subscript index is a "literal element" reference --
/// i.e., a `Var` naming a known dimension element or an integer literal.
/// These are dimension references at runtime, not variable references,
/// and must not be PREVIOUS-wrapped even when their textual form
/// collides with a user-variable name.
///
/// `position` is the index's 0-based position in the subscript; literal
/// `Var` names are matched against the dimension at that position first
/// and then against any dimension as a fallback (mirroring
/// `classify_expr0_subscript_shape`'s match rules).
///
/// `#[cfg(test)]`: see [`resolve_literal_element_index`].
#[cfg(test)]
pub(crate) fn is_literal_element_index(
    idx: &IndexExpr0,
    position: usize,
    source_dim_elements: &[Vec<String>],
) -> bool {
    resolve_literal_element_index(idx, position, source_dim_elements).is_some()
}

/// Resolve a single subscript index to a literal element name, mirroring
/// `db::ltm_ir::resolve_literal_index` (the Expr2 sibling) so both
/// classifiers agree on what counts as a "literal element".
///
/// `#[cfg(test)]`: this Expr0 classifier no longer drives production. The
/// ceteris-paribus wrap consumes the occurrence IR ([`OccurrenceLookup`]) -- the
/// same `db::ltm_ir` classification the edge emitter uses -- so there is ONE
/// classifier family and the historical Expr0/Expr2 drift (which silently zeroed
/// a link score when the wrap re-derived a different shape than the emitter, e.g.
/// GH #759 / GH #913 / the `pop[01]` canonicalization) is structurally
/// impossible. This sibling survives only for the wrap unit tests' occurrence
/// builder ([`build_wrap_test_occurrences`]) and the alignment gate
/// (`ltm_classifier_agreement_tests`), which proves the reparsed-Expr0 walk and
/// the IR's `Expr2` walk stay path- and shape-isomorphic corpus-wide.
///
/// Element names appear as `Var` nodes; integer literals appear as
/// `Const` nodes whose text is the integer. Either form is validated
/// by membership in `source_dim_elements`. For an indexed dim of size
/// N, `dimension_element_names` produces `["1", "2", ..., "N"]`, so a
/// `Const("999", ...)` over an indexed dim of size 5 won't match and
/// falls through to `None`. Matching prefers the dim at the index's
/// position, falling back to any dim if not found there.
#[cfg(test)]
pub(crate) fn resolve_literal_element_index(
    idx: &IndexExpr0,
    position: usize,
    source_dim_elements: &[Vec<String>],
) -> Option<String> {
    let candidate = match idx {
        IndexExpr0::Expr(Expr0::Var(name, _)) => canonicalize(name.as_str()).into_owned(),
        IndexExpr0::Expr(Expr0::Const(s, _, _)) => {
            // Integer literals (only) could be element references for
            // indexed dims. Canonicalize via parse-then-format so
            // non-canonical forms like `pop[01]` reduce to `"1"` and
            // match `dimension_element_names`'s `"1".."N"` output. The
            // Expr2 sibling (`db::ltm_ir::resolve_literal_index`)
            // does the same; without canonicalization here we'd
            // disagree on `01` (Expr2 -> FixedIndex(["1"]),
            // Expr0 -> DynamicIndex), the live-shape match would
            // fail, and the partial would silently zero.
            let n = s.parse::<u32>().ok()?;
            n.to_string()
        }
        _ => return None,
    };
    let matches_position = position < source_dim_elements.len()
        && source_dim_elements[position]
            .iter()
            .any(|e| e == &candidate);
    let matches_any = !matches_position
        && source_dim_elements
            .iter()
            .any(|dim| dim.iter().any(|e| e == &candidate));
    if matches_position || matches_any {
        Some(candidate)
    } else {
        None
    }
}

/// Classify an `Expr0` subscript's shape based on its indices.
///
/// Mirrors `db::ltm_ir::resolve_literal_index`'s classification logic but at
/// the `Expr0` (parsed-AST) level. `#[cfg(test)]`: the ceteris-paribus wrap no
/// longer re-derives shape on `Expr0` (it reads the occurrence IR); this sibling
/// survives only for the wrap unit tests' occurrence builder and the alignment
/// gate -- see [`resolve_literal_element_index`]. Each input string in
/// `source_dim_elements` is the canonical lowercase element name for the
/// corresponding source dimension, in source-declared order.
///
/// Rules:
/// - any `IndexExpr0::Wildcard` → `RefShape::Wildcard`
/// - an iterated-dimension subscript on the live source (all axes iterated)
///   → `RefShape::Bare`; a mixed iterated+literal one → `RefShape::PerElement`
/// - all indices are literal element names that match the source's
///   declared elements (or parseable integer literals for indexed
///   dimensions) → `RefShape::FixedIndex(canonical_names)`
/// - otherwise (StarRange, DimPosition, Range, non-literal Expr, or a
///   literal that doesn't match) → `RefShape::DynamicIndex`
///
/// The literal pass tries each index against the dimension at that position
/// first, then falls back to scanning all dimensions. This keeps the
/// classifier robust when callers pass dimensions in source-declared
/// order but the subscript indices may not align 1:1 with dimension
/// positions in unusual cases. Defensive `DynamicIndex` for unknown
/// names ensures the worst case is over-conservative wrapping rather
/// than incorrectly matching the live shape.
#[cfg(test)]
pub(crate) fn classify_expr0_subscript_shape(
    indices: &[IndexExpr0],
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> RefShape {
    if indices
        .iter()
        .any(|idx| matches!(idx, IndexExpr0::Wildcard(_)))
    {
        return RefShape::Wildcard;
    }
    // GH #511: an iterated-dimension subscript on the live source
    // (`row_sum[Region]` inside an apply-to-all-over-`Region x Age` equation)
    // reads the same source element -- it is `Bare`, mirroring
    // `db::ltm_ir::classify_iterated_dim_shape`. Checked before the
    // literal-element pass because a dimension name (`Region`) is not a
    // literal element, so it would otherwise fall to `DynamicIndex`.
    if is_live_source_iterated_dim_subscript(indices, source_dim_elements, iter_ctx, dim_ctx) {
        return RefShape::Bare;
    }
    // GH #525 (T6): a mixed iterated+literal subscript (`pop[Region, young]`
    // inside an A2A-over-`Region` equation) is `PerElement`, mirroring
    // `classify_iterated_dim_shape`'s `classify_axis_access`-derived mixed
    // arm -- the partial builder's live-shape match must agree with the
    // reference-site IR (the documented sync requirement). All-`Pinned`
    // falls through to the literal pass (`FixedIndex`), and all-`Iterated`
    // is the `Bare` case above, so this arm fires only for a genuine mix.
    if let Some(ctx) = iter_ctx
        && let Some(axes) =
            classify_expr0_per_element_axes(indices, source_dim_elements, ctx, dim_ctx)
    {
        let n_iterated = axes
            .iter()
            .filter(|a| matches!(a, crate::ltm_agg::AxisRead::Iterated { .. }))
            .count();
        if n_iterated > 0 && n_iterated < axes.len() {
            return RefShape::PerElement { axes };
        }
    }
    let mut elems = Vec::with_capacity(indices.len());
    for (i, idx) in indices.iter().enumerate() {
        // Use the same resolver as `is_literal_element_index` so this
        // classifier and the Expr2 sibling
        // (`db::ltm_ir::resolve_literal_index`) agree on what counts
        // as a literal element. Integer literals are validated against
        // `source_dim_elements` (which contains `["1", ..., "size"]`
        // for indexed dims), so out-of-range integers like `pop[999]`
        // over a size-5 indexed dim fall through to `DynamicIndex` --
        // matching what the edge emitter sees and avoiding the
        // shape-mismatch that would zero out the partial.
        match resolve_literal_element_index(idx, i, source_dim_elements) {
            Some(elem) => elems.push(elem),
            None => return RefShape::DynamicIndex,
        }
    }
    RefShape::FixedIndex(elems)
}
