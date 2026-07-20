// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Post-transform lowering for LTM link-score equations (Track A stage 1).
//!
//! The `agg → target` and `PerElement` link-score generators in `ltm_augment`
//! were inverted by the "transform-first" restructuring: the ceteris-paribus
//! wrap (`wrap_changed_first_ast`) now runs on the target's OWN equation text
//! -- hoisted reducers still spelled `SUM(...)`, the live source's occurrence
//! held at its actual shape -- and the concrete-form rewrite runs AFTERWARD, on
//! the already-wrapped `Expr0`. This module owns those post-transform lowerings:
//!
//! - [`substitute_reducers_in_expr0`] rewrites each hoisted reducer subtree in
//!   the wrapped AST to its synthetic aggregate node's bare name (the held-live
//!   reducer to `agg`, each frozen `PREVIOUS(SUM(..))` to `PREVIOUS(agg)`);
//! - [`rewrite_per_element_source_refs`] row-pins every live/frozen source
//!   occurrence of a `PerElement` edge to its concrete per-element subscript,
//!   supported by [`PerElementRefCtx`], [`per_element_row_for_target`] (the
//!   single row derivation the link-score NAME and equation both consume), and
//!   [`qualify_axis_element`].
//!
//! Running the wrap on the own text and lowering afterward keeps the emitted
//! equation byte-identical while making the wrap occurrence-addressable -- it is
//! now keyed on the target's OWN occurrence stream. That is the whole point of
//! the inversion: stage 2/3 can retarget the wrap onto the `db::ltm_ir`
//! occurrence IR without touching these lowerings, because they deliberately do
//! NOT re-classify (each is a pure text-keyed / shape-keyed AST rewrite). See
//! the generators in `ltm_augment` -- `generate_agg_to_scalar_target_equation`,
//! `generate_scalar_to_element_equation`, and `generate_per_element_link_equation`
//! -- for the wrap-then-lower composition that calls these.
//!
//! Split out of `ltm_augment.rs` to keep that file under the project
//! line-count lint; included via `#[path]`, so `super::*` resolves the parent's
//! private items.

use std::collections::HashMap;

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::canonicalize;
use crate::common::{Canonical, Ident, RawIdent};

use super::{
    IteratedDimCtx, classify_expr0_per_element_axes, expr0_iterated_axis_lines_up,
    qualify_element_csv,
};

#[cfg(test)]
use super::PartialEquationError;
#[cfg(test)]
use crate::lexer::LexerType;

/// Read-only context for [`rewrite_per_element_source_refs`]: everything the
/// walker needs to substitute the live source's subscript indices for one
/// `(site, target element)` instantiation of a `PerElement` link score
/// (GH #525, T6 of the shape-expressiveness design).
pub(super) struct PerElementRefCtx<'a> {
    /// The live source variable (canonical).
    pub(super) from: &'a Ident<Canonical>,
    /// The emitting site's per-axis access vector
    /// ([`RefShape::PerElement`]'s `axes`).
    pub(super) site_axes: &'a [crate::ltm_agg::AxisRead],
    /// The row this `(site, e)` instantiation reads -- BARE element names,
    /// one per source axis (parallel to `site_axes`). The wrap already held
    /// the `site_axes`-shaped occurrence live (its shape equals `site_axes`);
    /// this lowering rewrites that live occurrence to exactly these bare
    /// indices so it re-prints as the historical `from[<row>]`.
    pub(super) row_parts_bare: &'a [String],
    /// Element-name lists per source axis (position-strict resolution).
    pub(super) source_dim_elements: &'a [Vec<String>],
    /// The source's declared dimensions (for index qualification).
    pub(super) from_dims: &'a [crate::dimensions::Dimension],
    /// Target-iterated dim (canonical) -> (element of `e` for that dim,
    /// its index within the dim) -- the projection data `e` supplies.
    pub(super) target_elem_by_dim: &'a HashMap<String, (String, usize)>,
    /// The iterated-dim recognition context (live source's axes + target
    /// iterated dims + mapping context).
    pub(super) iter_ctx: &'a IteratedDimCtx<'a>,
    pub(super) dim_ctx: &'a crate::dimensions::DimensionsContext,
}

/// Qualify one element of one axis (`"nyc"` over `Region` ->
/// `"region\u{B7}nyc"`), via the same defensive rules as
/// [`qualify_element_csv`].
pub(super) fn qualify_axis_element(elem: &str, dim: &crate::dimensions::Dimension) -> String {
    qualify_element_csv(elem, std::slice::from_ref(dim))
}

/// The source row a per-axis access vector reads for one full target
/// element: project the target element onto the `Iterated` axes
/// (slot-remapped through `mapped_element_correspondence` for a
/// positionally-mapped pair -- the correspondence is indexed by TARGET
/// element position and yields the source element the executed simulation
/// reads) and fill `Pinned` axes with their literals. One bare element
/// name per axis, in source-axis order. `None` when an `Iterated` dim is
/// missing from the target projection or the mapped remap is unusable (a
/// mid-edit inconsistency; callers degrade conservatively) -- and for any
/// `Reduced` axis, which the `PerElement` invariant excludes.
///
/// This is the SINGLE row derivation for the `PerElement` family's
/// emission: the link-score NAME's row (computed by
/// `emit_per_element_link_scores`) and the equation's live-reference row
/// (computed by [`rewrite_per_element_source_refs`]) both come from here,
/// so they cannot disagree.
pub(crate) fn per_element_row_for_target(
    axes: &[crate::ltm_agg::AxisRead],
    target_elem_by_dim: &HashMap<String, (String, usize)>,
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<Vec<String>> {
    use crate::common::CanonicalDimensionName;
    use crate::ltm_agg::AxisRead;
    axes.iter()
        .map(|ax| match ax {
            AxisRead::Pinned(e) => Some(e.clone()),
            AxisRead::Iterated { dim, source_dim } => {
                let (elem, idx) = target_elem_by_dim.get(dim)?;
                if dim == source_dim {
                    Some(elem.clone())
                } else {
                    let corr = dim_ctx.mapped_element_correspondence(
                        &CanonicalDimensionName::from_raw(dim),
                        &CanonicalDimensionName::from_raw(source_dim),
                    )?;
                    corr.get(*idx).map(|e| e.as_str().to_string())
                }
            }
            AxisRead::Reduced { .. } => None,
        })
        .collect()
}

/// Row-pin every reference to the live source inside a `PerElement` link
/// score's target body so the final scalar equation is well-formed for one
/// `(site, target element)` instantiation -- the index-substitution mechanism
/// of `pin_body_to_row` (the GH #744 reducer-body machinery) lifted to
/// target-equation bodies (T6 of the shape-expressiveness design).
///
/// This runs as a POST-transform lowering of the already-wrapped AST (Track A
/// stage 1): the ceteris-paribus wrap (`wrap_changed_first_ast` with
/// `PerElement { site_axes }` held live) already held the emitting site's
/// occurrence live and froze every other reference at `PREVIOUS`; this pass
/// then substitutes each source occurrence's iterated-dim indices for concrete
/// elements. The distinction the wrap already encoded -- live occurrence not
/// nested in a wrap-inserted `PREVIOUS`, frozen occurrences nested inside one
/// -- lines up with the `force_qualified` flag below, so the lowering reads
/// "live vs frozen" off `PREVIOUS`-nesting rather than re-deciding it:
///
/// - an occurrence whose per-axis classification EQUALS the emitting site's
///   `axes` (and is not `force_qualified`, i.e. the wrap left it live) is
///   rewritten to the row's BARE element indices, so it re-prints as the
///   historical `from[<row>]`;
/// - any other fully-classifiable occurrence (a different `PerElement`
///   shape, an all-`Iterated` Bare-shaped subscript, an all-`Pinned`
///   literal subscript) is rewritten to ITS OWN row for this target
///   element, QUALIFIED (`region\u{B7}a`) so its wrap-inserted `PREVIOUS(...)`
///   freeze compiles to a direct LoadPrev in the scalar fragment;
/// - a BARE `Var` occurrence of the source (the mixed `Bare`+`PerElement`
///   edge's other site) is pinned to the target element's projection onto
///   the source's own axes when every axis resolves (same-element
///   semantics), qualified -- else left as the wrap's conservative freeze;
/// - a partially-classifiable subscript (a wildcard slice, a dynamic
///   index) gets only its resolvable iterated-dim indices substituted
///   (qualified; meaning-preserving -- the iterated dim IS that element
///   in this slot), leaving the rest as the wrap left it.
///
/// Inside `PREVIOUS(...)`/`INIT(...)` calls (both the wrap-inserted freezes and
/// any already in the source equation) the live-match rewrite is suppressed
/// (`force_qualified`): the contents are lagged/frozen and could never be the
/// live reference -- qualified substitution keeps the frozen read compiling to
/// a direct slot.
pub(super) fn rewrite_per_element_source_refs(
    expr: Expr0,
    ctx: &PerElementRefCtx<'_>,
    force_qualified: bool,
) -> Expr0 {
    let qualify_row = |row: &[String]| -> Vec<IndexExpr0> {
        row.iter()
            .zip(ctx.from_dims)
            .map(|(part, dim)| {
                IndexExpr0::Expr(Expr0::Var(
                    RawIdent::new_from_str(&qualify_axis_element(part, dim)),
                    crate::ast::Loc::default(),
                ))
            })
            .collect()
    };
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) != ctx.from {
                return expr;
            }
            // Same-element pin of a bare source reference: each axis reads
            // the target element's coordinate for that axis's own dim.
            let bare_axes: Vec<crate::ltm_agg::AxisRead> = ctx
                .from_dims
                .iter()
                .map(|d| crate::ltm_agg::AxisRead::Iterated {
                    dim: d.name().to_string(),
                    source_dim: d.name().to_string(),
                })
                .collect();
            match per_element_row_for_target(&bare_axes, ctx.target_elem_by_dim, ctx.dim_ctx) {
                Some(row) => Expr0::Subscript(ident.clone(), qualify_row(&row), loc),
                None => expr,
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) != ctx.from {
                // Another variable's subscript: recurse into expression
                // indices (a nested source reference can hide there) AND into a
                // range's two endpoints, which are also `Expr0`
                // (`other[from[D, young]:3]`). The IR walker records occurrences
                // under both (`IndexExpr2::Range` pushes children 0 and 1), and
                // the sibling `substitute_reducers_in_expr0` descends both, so
                // skipping them here left a recorded source occurrence
                // un-pinned: its dimension-name subscript survived into the
                // scalar equation, which either fails to compile or reads the
                // wrong element.
                let indices =
                    indices
                        .into_iter()
                        .map(|idx| match idx {
                            IndexExpr0::Expr(e) => IndexExpr0::Expr(
                                rewrite_per_element_source_refs(e, ctx, force_qualified),
                            ),
                            IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
                                rewrite_per_element_source_refs(l, ctx, force_qualified),
                                rewrite_per_element_source_refs(r, ctx, force_qualified),
                                loc,
                            ),
                            // Wildcard / star-range / `@N` carry no `Expr0`.
                            other => other,
                        })
                        .collect();
                return Expr0::Subscript(ident, indices, loc);
            }
            if let Some(occ_axes) =
                classify_expr0_per_element_axes(&indices, ctx.source_dim_elements, ctx.iter_ctx)
            {
                if !force_qualified && occ_axes == ctx.site_axes {
                    // The emitting site's own shape: lower the occurrence
                    // (already held live or frozen by the preceding
                    // ceteris-paribus wrap) to the concrete row this score
                    // targets, spelled as bare elements.
                    let indices = ctx
                        .row_parts_bare
                        .iter()
                        .map(|p| {
                            IndexExpr0::Expr(Expr0::Var(
                                RawIdent::new_from_str(p),
                                crate::ast::Loc::default(),
                            ))
                        })
                        .collect();
                    return Expr0::Subscript(ident, indices, loc);
                }
                if let Some(row) =
                    per_element_row_for_target(&occ_axes, ctx.target_elem_by_dim, ctx.dim_ctx)
                {
                    return Expr0::Subscript(ident, qualify_row(&row), loc);
                }
                return Expr0::Subscript(ident, indices, loc);
            }
            // Partially classifiable: substitute only the resolvable
            // iterated-dim indices (qualified), leave the rest (wildcards,
            // literals, dynamic expressions) for the wrap's conservative
            // handling.
            let indices = indices
                .into_iter()
                .enumerate()
                .map(|(i, idx)| {
                    let substituted = match &idx {
                        IndexExpr0::Expr(Expr0::Var(name, _)) => {
                            let d = canonicalize(name.as_str()).into_owned();
                            if i < ctx.from_dims.len()
                                && ctx.iter_ctx.target_iterated_dims.contains(&d)
                                && expr0_iterated_axis_lines_up(
                                    &d,
                                    i,
                                    ctx.source_dim_elements,
                                    ctx.iter_ctx,
                                )
                            {
                                let ax = crate::ltm_agg::AxisRead::Iterated {
                                    dim: d,
                                    source_dim: ctx.iter_ctx.source_dim_names[i].clone(),
                                };
                                per_element_row_for_target(
                                    std::slice::from_ref(&ax),
                                    ctx.target_elem_by_dim,
                                    ctx.dim_ctx,
                                )
                                .map(|row| qualify_axis_element(&row[0], &ctx.from_dims[i]))
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    match substituted {
                        Some(part) => IndexExpr0::Expr(Expr0::Var(
                            RawIdent::new_from_str(&part),
                            crate::ast::Loc::default(),
                        )),
                        // Not a resolvable iterated-dim index. A nested source
                        // reference can still hide inside a range endpoint
                        // (`from[a:from[b]]`), and the IR records it, so descend
                        // rather than leaving it un-pinned -- same reason as the
                        // other-variable branch above. An `Expr` index that did
                        // not substitute is left to the wrap's conservative
                        // handling (recursing would double-pin the source's own
                        // axis).
                        None => match idx {
                            IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                                rewrite_per_element_source_refs(l, ctx, force_qualified),
                                rewrite_per_element_source_refs(r, ctx, force_qualified),
                                rloc,
                            ),
                            other => other,
                        },
                    }
                })
                .collect();
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let lagged = name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init");
            let args = args
                .into_iter()
                .map(|a| rewrite_per_element_source_refs(a, ctx, force_qualified || lagged))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(rewrite_per_element_source_refs(
                *inner,
                ctx,
                force_qualified,
            )),
            loc,
        ),
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(
            op,
            Box::new(rewrite_per_element_source_refs(*l, ctx, force_qualified)),
            Box::new(rewrite_per_element_source_refs(*r, ctx, force_qualified)),
            loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(rewrite_per_element_source_refs(*c, ctx, force_qualified)),
            Box::new(rewrite_per_element_source_refs(*t, ctx, force_qualified)),
            Box::new(rewrite_per_element_source_refs(*f, ctx, force_qualified)),
            loc,
        ),
    }
}

/// Substitute each recognized reducer subexpression in `equation_text` with a
/// (quoted) reference to its aggregate node.
///
/// `reducers` maps the canonical reducer-subexpression text (exactly as
/// `crate::patch::expr2_to_string` / `print_eqn` renders it -- lowercased,
/// whitespace-normalized) to the agg node's name. `equation_text` is parsed
/// to `Expr0`, and any subexpression of it whose `print_eqn` equals one of
/// those keys is replaced by a `Var(agg_name)` node, then the whole tree is
/// re-printed. The match is on the parsed AST subtree, not a substring of the
/// text, so a reducer text that is a textual prefix of a *different* reducer
/// subexpression (`sum(p[*])` vs `sum(p[*] + 1)`) is never falsely matched.
///
/// Returns `Err([`PartialEquationError`])` when `equation_text` does not parse
/// (a genuine parse error, or an empty/whitespace equation that yields no AST)
/// *and there are reducers to substitute*: with no AST there is no reducer
/// subexpression to replace, so returning the input unchanged would let the
/// inline reducer survive into the `agg → target` partial -- a partial that
/// references the live reducer instead of the hoisted aggregate node, a
/// wrong-but-clean-compiling link score (the agg-substitution-omission sibling
/// of the GH #311 PREVIOUS-omission hazard; GH #661). The db-bearing caller
/// converts the error into a `Warning` (via `emit_ltm_partial_equation_warning`)
/// and skips the variable. The failure is effectively unreachable in production
/// (the input is a `print_eqn` re-print of an already-parsed AST), so this is
/// defense-in-depth.
///
/// The empty-`reducers` case is a pure pass-through that never parses (there
/// is nothing to substitute), so it returns `Ok` with the text unchanged even
/// for otherwise-unparseable input.
///
/// Test-only since Track A stage 1: the agg-substitution is now a POST-transform
/// lowering that runs on the wrapped AST via [`substitute_reducers_in_expr0`]
/// (the generators call that directly), so the text-level wrapper survives only
/// as the unit-tested entry point for the substitution behavior.
#[cfg(test)]
pub(crate) fn substitute_reducers_in_equation(
    equation_text: &str,
    reducers: &HashMap<String, String>,
) -> Result<String, PartialEquationError> {
    if reducers.is_empty() {
        return Ok(equation_text.to_string());
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
    };
    Ok(print_eqn(&substitute_reducers_in_expr0(ast, reducers)))
}

/// The agg-substitution POST-transform lowering (Track A stage 1): rewrite each
/// hoisted reducer subexpression in an already-wrapped `Expr0` to its synthetic
/// agg name in place, keyed by canonical `print_eqn` text (`reducers` maps
/// reducer text -> agg name). It runs on the WRAPPED AST -- AFTER the
/// ceteris-paribus wrap held one hoisted reducer live and froze the rest whole
/// -- so the held-live reducer becomes a bare agg name and each frozen
/// `PREVIOUS(SUM(..))` becomes `PREVIOUS(agg)`, byte-for-byte matching the old
/// wrap-of-agg-substituted-text composition. It deliberately does NOT
/// re-classify (a pure text match), so stage 2 can retarget the wrap onto the
/// occurrence IR without touching this lowering. The `agg → target` generators
/// call it directly; the text-level `substitute_reducers_in_equation` wrapper
/// is now test-only.
pub(super) fn substitute_reducers_in_expr0(
    expr: Expr0,
    reducers: &HashMap<String, String>,
) -> Expr0 {
    // A whole-subtree match wins before descending: a reducer App is opaque
    // -- once it matches an agg, we don't recurse into its (now-irrelevant)
    // argument.
    if let Some(agg_name) = reducers.get(&print_eqn(&expr)) {
        return Expr0::Var(
            crate::common::RawIdent::new_from_str(agg_name),
            crate::ast::Loc::default(),
        );
    }
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => expr,
        Expr0::Subscript(ident, indices, loc) => {
            // A reducer can appear as (or inside) a subscript index expression
            // -- `stock[SUM(idx[*])]` -- and `walk_subexpr_for_aggs` hoists it
            // into a synthetic agg by descending into `IndexExpr2::Expr` /
            // `IndexExpr2::Range`, so the substituter must mirror that descent.
            // Wildcard / star-range / `@N` indices carry no `Expr0`, so they
            // pass through unchanged.
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    IndexExpr0::Expr(e) => {
                        IndexExpr0::Expr(substitute_reducers_in_expr0(e, reducers))
                    }
                    IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
                        substitute_reducers_in_expr0(l, reducers),
                        substitute_reducers_in_expr0(r, reducers),
                        loc,
                    ),
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => idx,
                })
                .collect();
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| substitute_reducers_in_expr0(a, reducers))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(substitute_reducers_in_expr0(*inner, reducers)),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(substitute_reducers_in_expr0(*lhs, reducers)),
            Box::new(substitute_reducers_in_expr0(*rhs, reducers)),
            loc,
        ),
        Expr0::If(cond, then_e, else_e, loc) => Expr0::If(
            Box::new(substitute_reducers_in_expr0(*cond, reducers)),
            Box::new(substitute_reducers_in_expr0(*then_e, reducers)),
            Box::new(substitute_reducers_in_expr0(*else_e, reducers)),
            loc,
        ),
    }
}
