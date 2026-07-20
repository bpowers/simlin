// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Concrete-form lowerings for LTM link-score equations.
//!
//! The `agg → target` and `PerElement` link-score generators in `ltm_augment`
//! were inverted by the "transform-first" restructuring: the ceteris-paribus
//! wrap (`wrap_changed_first_ast`) runs on the target's OWN equation -- hoisted
//! reducers still spelled `SUM(...)`, the live source's occurrence held at its
//! actual shape -- and the concrete-form rewrite happens around it. This module
//! owns those lowerings:
//!
//! - [`substitute_reducers_in_expr0`] rewrites each hoisted reducer subtree in
//!   the wrapped AST to its synthetic aggregate node's bare name (the held-live
//!   reducer to `agg`, each frozen `PREVIOUS(SUM(..))` to `PREVIOUS(agg)`);
//! - the `PerElement` ROW PINNING -- [`pin_source_subscript_indices`] for one
//!   occurrence and [`pin_only_source_refs`] for a subtree the wrap froze --
//!   supported by [`PerElementRefCtx`], [`per_element_row_for_target`] (the
//!   single row derivation the link-score NAME and equation both consume), and
//!   [`qualify_axis_element`]. These are called FROM the ceteris-paribus wrap as
//!   it descends, not as a pass over its output: the wrap is the only place that
//!   knows both the occurrence (by path) and whether it is about to freeze the
//!   reference, and that second fact is what selects the bare-row spelling for
//!   the live occurrence over the qualified-row spelling for every other one.
//!   Running them afterward forced the lowering to re-derive each occurrence's
//!   per-axis access with an Expr0 classifier, since a `SiteId` computed on the
//!   original AST cannot address a tree the wrap has inserted `PREVIOUS` nodes
//!   into; that classifier is now deleted.
//!
//! Running the wrap on the target's own equation is what makes it
//! occurrence-addressable: it is keyed on the target's OWN occurrence stream, so
//! every decision comes from `db::ltm_ir`'s one classification. NONE of the
//! lowerings here re-classifies -- the agg substitution is a pure text-keyed AST
//! rewrite, and the row pinning reads `OccurrenceSite::axes`. See the generators
//! in `ltm_augment` -- `generate_agg_to_scalar_target_equation`,
//! `generate_scalar_to_element_equation`, and `generate_per_element_link_equation`
//! -- for the compositions that call these.
//!
//! Split out of `ltm_augment.rs` to keep that file under the project
//! line-count lint; included via `#[path]`, so `super::*` resolves the parent's
//! private items.

use std::collections::HashMap;

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::common::{Canonical, Ident, RawIdent};

use super::{OccurrenceLookup, qualify_element_csv};
use crate::db::ltm_ir::{OccurrenceAxis, OccurrenceSite};

#[cfg(test)]
use super::PartialEquationError;
#[cfg(test)]
use crate::lexer::LexerType;

/// Read-only context for the `PerElement` row-pinning lowering: everything it
/// needs to substitute the live source's subscript indices for one
/// `(site, target element)` instantiation of a `PerElement` link score
/// (GH #525, T6 of the shape-expressiveness design).
///
/// Notably absent since the lowering became IR-driven: the source's per-axis
/// element-name lists and the iterated-dim recognition context. Those were the
/// inputs to the Expr0 per-axis CLASSIFIER this lowering used to run; the
/// per-axis truth now comes off the occurrence IR (`OccurrenceSite::axes`), so
/// only the projection data survives.
pub(super) struct PerElementRefCtx<'a> {
    /// The live source variable (canonical).
    pub(super) from: &'a Ident<Canonical>,
    /// The emitting site's per-axis access vector
    /// ([`RefShape::PerElement`]'s `axes`).
    pub(super) site_axes: &'a [crate::ltm_agg::AxisRead],
    /// The row this `(site, e)` instantiation reads -- BARE element names,
    /// one per source axis (parallel to `site_axes`). The wrap holds the
    /// `site_axes`-shaped occurrence live (its shape equals `site_axes`); the
    /// lowering rewrites that live occurrence to exactly these bare indices so
    /// it prints as the historical `from[<row>]`.
    pub(super) row_parts_bare: &'a [String],
    /// The source's declared dimensions (for index qualification).
    pub(super) from_dims: &'a [crate::dimensions::Dimension],
    /// Target-iterated dim (canonical) -> (element of `e` for that dim,
    /// its index within the dim) -- the projection data `e` supplies.
    pub(super) target_elem_by_dim: &'a HashMap<String, (String, usize)>,
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
/// (computed by [`pin_source_subscript_indices`]) both come from here,
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

/// The per-axis [`crate::ltm_agg::AxisRead`] slice of a live-source subscript
/// occurrence, or `None` when the occurrence is not fully describable per axis.
///
/// This is the IR-driven replacement for the retired Expr0 per-axis classifier
/// (`classify_expr0_per_element_axes`). The equivalence is exact rather than
/// approximate: that classifier accepted an index only when it was an
/// iterated-dimension name lined up with the source's axis at that position, or
/// a literal element of that axis, with the arity matching on all of
/// indices / source dims / source element lists. The IR reaches the SAME
/// per-axis verdict through [`crate::ltm_agg::classify_axis_access`] -- the
/// shared classifier the retired `expr0_iterated_axis_lines_up` mirrored gate
/// for gate -- so "every axis `Iterated` or `Pinned`, arity equal to the
/// source's declared arity" is the same predicate. A `Reduced` axis (a wildcard
/// or star-range index), a `MismatchedIterated` one, and a `Dynamic` one all
/// fall out here exactly as they produced `None` there. An over-arity index can
/// never be `Iterated` (the IR only consults `classify_axis_access` where the
/// source has an axis at that position), so the old `i < from_dims.len()` guard
/// is implied rather than dropped.
fn axes_as_read_slice(occ: &OccurrenceSite, arity: usize) -> Option<Vec<crate::ltm_agg::AxisRead>> {
    use crate::ltm_agg::AxisRead;
    if occ.axes.is_empty() || occ.axes.len() != arity {
        return None;
    }
    occ.axes
        .iter()
        .map(|a| match a {
            OccurrenceAxis::Pinned(e) => Some(AxisRead::Pinned(e.clone())),
            OccurrenceAxis::Iterated { dim, source_dim } => Some(AxisRead::Iterated {
                dim: dim.clone(),
                source_dim: source_dim.clone(),
            }),
            OccurrenceAxis::Reduced { .. }
            | OccurrenceAxis::MismatchedIterated { .. }
            | OccurrenceAxis::Dynamic => None,
        })
        .collect()
}

/// The row indices for one occurrence of the live source, QUALIFIED
/// (`region\u{B7}a`) so a frozen read compiles to a direct LoadPrev in the
/// scalar fragment.
fn qualified_row_indices(row: &[String], ctx: &PerElementRefCtx<'_>) -> Vec<IndexExpr0> {
    row.iter()
        .zip(ctx.from_dims)
        .map(|(part, dim)| {
            IndexExpr0::Expr(Expr0::Var(
                RawIdent::new_from_str(&qualify_axis_element(part, dim)),
                crate::ast::Loc::default(),
            ))
        })
        .collect()
}

/// Row-pin ONE subscript occurrence of the live source for a `PerElement` link
/// score, given the occurrence the IR recorded at its node
/// (GH #525, T6 of the shape-expressiveness design).
///
/// `live` is whether the ceteris-paribus wrap is leaving this occurrence LIVE --
/// its shape equals the emitting site's AND it is not inside a subtree the wrap
/// froze. Only the wrap can answer that, which is why the pinning runs inside
/// the wrap's own traversal rather than as a pass over the wrapped tree; see
/// the module docs.
///
/// - the LIVE occurrence is rewritten to the row's BARE element indices, so it
///   prints as the historical `from[<row>]`;
/// - any other fully-describable occurrence (a different `PerElement` shape, an
///   all-`Iterated` Bare-shaped subscript, an all-`Pinned` literal subscript) is
///   rewritten to ITS OWN row for this target element, QUALIFIED, so the freeze
///   the wrap puts around it compiles to a direct LoadPrev;
/// - a partially-describable subscript (a wildcard slice, a dynamic index) gets
///   only its `Iterated` indices substituted (qualified; meaning-preserving --
///   the iterated dim IS that element in this slot) and leaves the rest to
///   `recurse_index`, which is the wrap's own index pass, so a genuinely dynamic
///   index still gets its `PREVIOUS(idx)` lag.
///
/// A node with NO recorded occurrence is left untouched HERE. That is the loud
/// path, not a silent one: the wrap's own `missing_occurrence` guard fires on a
/// live-source subscript whose path misses on a non-empty stream, and the caller
/// abandons the partial with a warning. ([`pin_only_source_refs`] is the one
/// caller that first substitutes such a subscript's indices structurally, by
/// name, because a `LOOKUP` table argument legitimately has no occurrence and
/// still has to compile; see [`pin_dimension_name_indices`].)
pub(super) fn pin_source_subscript_indices(
    indices: Vec<IndexExpr0>,
    node_occ: Option<&OccurrenceSite>,
    ctx: &PerElementRefCtx<'_>,
    live: bool,
    mut recurse_index: impl FnMut(usize, IndexExpr0) -> IndexExpr0,
) -> Vec<IndexExpr0> {
    let describable = node_occ.and_then(|o| axes_as_read_slice(o, ctx.from_dims.len()));
    if let Some(occ_axes) = describable {
        if live && occ_axes == ctx.site_axes {
            return ctx
                .row_parts_bare
                .iter()
                .map(|p| {
                    IndexExpr0::Expr(Expr0::Var(
                        RawIdent::new_from_str(p),
                        crate::ast::Loc::default(),
                    ))
                })
                .collect();
        }
        if let Some(row) =
            per_element_row_for_target(&occ_axes, ctx.target_elem_by_dim, ctx.dim_ctx)
        {
            return qualified_row_indices(&row, ctx);
        }
        return indices;
    }
    // Partially describable: substitute only the axes the IR classified
    // `Iterated`, and hand every other index to the wrap's own index pass.
    let axes = node_occ.map(|o| o.axes.as_slice()).unwrap_or(&[]);
    indices
        .into_iter()
        .enumerate()
        .map(|(i, idx)| {
            let substituted = match (axes.get(i), ctx.from_dims.get(i)) {
                (Some(OccurrenceAxis::Iterated { dim, source_dim }), Some(from_dim)) => {
                    let ax = crate::ltm_agg::AxisRead::Iterated {
                        dim: dim.clone(),
                        source_dim: source_dim.clone(),
                    };
                    per_element_row_for_target(
                        std::slice::from_ref(&ax),
                        ctx.target_elem_by_dim,
                        ctx.dim_ctx,
                    )
                    .map(|row| qualify_axis_element(&row[0], from_dim))
                }
                _ => None,
            };
            match substituted {
                Some(part) => IndexExpr0::Expr(Expr0::Var(
                    RawIdent::new_from_str(&part),
                    crate::ast::Loc::default(),
                )),
                None => recurse_index(i, idx),
            }
        })
        .collect()
}

/// Row-pin a BARE `Var` reference to the live source (the mixed
/// `Bare`+`PerElement` edge's other site): each axis reads the target element's
/// coordinate for that axis's own dimension (same-element semantics), qualified.
/// `None` when some axis does not resolve -- the caller then leaves the bare
/// reference for the wrap's conservative freeze.
pub(super) fn pin_bare_source_ref(ctx: &PerElementRefCtx<'_>) -> Option<Vec<IndexExpr0>> {
    let bare_axes: Vec<crate::ltm_agg::AxisRead> = ctx
        .from_dims
        .iter()
        .map(|d| crate::ltm_agg::AxisRead::Iterated {
            dim: d.name().to_string(),
            source_dim: d.name().to_string(),
        })
        .collect();
    per_element_row_for_target(&bare_axes, ctx.target_elem_by_dim, ctx.dim_ctx)
        .map(|row| qualified_row_indices(&row, ctx))
}

/// Row-pin a source subscript the occurrence IR deliberately records NOTHING
/// for, by NAME alone. Returns the rewritten indices plus whether the rule
/// DISCHARGED the subscript.
///
/// This is a lowering-COMPLETENESS rule, not a classifier, and that distinction
/// is the whole reason it exists. `db::ltm_ir` records no occurrence under a
/// `LOOKUP` TABLE argument (`BuiltinContents::LookupTable(_) => {}` -- "a
/// graphical-function table reference is static data, not a causal edge"), and it
/// is RIGHT: such a reference carries no causal edge, so it earns no attribution,
/// no element edge and no score. But the lowering is still obliged to emit a
/// COMPILABLE scalar fragment, and a dimension-name subscript
/// (`effect[region, old]`) cannot resolve in one. Compilability and attribution
/// are two separate obligations, and discharging the second does not require the
/// first -- which is why this asks the IR for nothing.
///
/// It consults NO occurrence and infers NO shape. Per index, by name only:
///
/// - an index spelling the dimension the source declares AT THAT POSITION is
///   replaced by this target element's coordinate for that dimension -- exactly
///   the structural substitution [`pin_bare_source_ref`] already performs for a
///   bare `Var`, generalized to a subscript's indices;
/// - an index the source's axis at that position DECLARES as an element is a
///   literal selector, qualified with that axis (`old` -> `age·old`). It would
///   very likely resolve bare too, but the pin qualifies EVERY index of a row for
///   a reason (see `wrap_non_matching_in_previous`'s `skip_index_qualification`):
///   the wrap's generic `qualify_element_index` cannot qualify an element name
///   several dimensions declare, so a half-qualified subscript is the one spelling
///   whose compilability depends on the model's element names. Qualifying here
///   also makes this rule's output byte-identical to the pre-`391bc3c1` pass's,
///   which is the conservative thing for a regression fix to be.
///
/// There is no `RefShape` here, no axis vocabulary, and no live-vs-frozen
/// decision, and this can never make the reference live-selectable (the pin-only
/// descent records no `live_ref`). Do NOT grow it into a per-axis classifier: the
/// second classifier family was deleted on purpose (`391bc3c1`).
///
/// `discharged == false` when a bare index still NAMES a dimension the rule could
/// not resolve -- a mapped or transposed axis name, an axis whose element this
/// target element does not project, an over-arity index. Only the IR knows what
/// such an index reads, and it recorded nothing, so the caller keeps that case
/// LOUD (`WrapOutcome::missing_occurrence` -> warned skip) rather than guessing.
/// An index naming no dimension at all (a literal element, a wildcard, an
/// arithmetic expression) does NOT make the subscript unlowerable: it is already
/// as concrete as the target's own equation spelled it, and a genuinely dynamic
/// one is left to the caller's index pass.
fn pin_dimension_name_indices(
    indices: Vec<IndexExpr0>,
    ctx: &PerElementRefCtx<'_>,
) -> (Vec<IndexExpr0>, bool) {
    let mut discharged = true;
    let indices = indices
        .into_iter()
        .enumerate()
        .map(|(i, idx)| {
            let (name, loc) = match &idx {
                IndexExpr0::Expr(Expr0::Var(name, loc)) => {
                    (crate::common::canonicalize(name.as_str()).to_string(), *loc)
                }
                _ => return idx,
            };
            let Some(dim) = ctx.from_dims.get(i) else {
                // Over-arity: no axis owns this index, so nothing names its
                // owner and there is nothing to pin it from.
                if ctx.dim_ctx.is_dimension_name(&name) {
                    discharged = false;
                }
                return idx;
            };
            let pinned = if dim.name() == name {
                // The identity axis: the index names the dimension the source
                // declares here, so it reads this target element's own
                // coordinate for that dimension. Routed through the ONE row
                // derivation, so a name-directed pin and an occurrence-driven
                // one cannot spell the same row differently.
                let axis = crate::ltm_agg::AxisRead::Iterated {
                    dim: name.clone(),
                    source_dim: name.clone(),
                };
                per_element_row_for_target(
                    std::slice::from_ref(&axis),
                    ctx.target_elem_by_dim,
                    ctx.dim_ctx,
                )
                .map(|row| qualify_axis_element(&row[0], dim))
            } else if ctx.dim_ctx.is_dimension_name(&name) {
                // Some OTHER dimension's name: a mapped or transposed axis,
                // whose read only the IR can describe. Stay loud.
                None
            } else {
                // A literal element selector when this axis declares the name;
                // `qualify_axis_element` is a no-op for anything else (a
                // dynamic scalar index), which is what leaves it to the
                // caller's index pass.
                Some(qualify_axis_element(&name, dim))
            };
            match pinned {
                // Rebuild the index only when the name actually changes, so an
                // index this rule has nothing to say about keeps its own node.
                Some(part) if part != name => {
                    IndexExpr0::Expr(Expr0::Var(RawIdent::new_from_str(&part), loc))
                }
                Some(_) => idx,
                None => {
                    discharged = false;
                    idx
                }
            }
        })
        .collect();
    (indices, discharged)
}

/// Row-pin the live source's references inside a subtree the ceteris-paribus
/// wrap declines to descend into -- a pre-existing `PREVIOUS`/`INIT` call, the
/// GH #517 whole-frozen reducer, or a `LOOKUP` table argument.
///
/// This is the pin-only descent: it performs the lowering the wrap would have
/// performed had it recursed, driven by the SAME structural path cursor and the
/// SAME occurrence IR, and it wraps nothing. Everything it reaches is inside a
/// frozen subtree (or is static table data), so no occurrence here can be the
/// live reference -- every pin is qualified.
///
/// Why this rather than tagging the tree during the wrap (the alternative
/// `86df68bf`'s rustdoc weighed): a tag needs either a descent whose only
/// purpose is to write tags, or a `Loc`-keyed side map from node position to
/// occurrence -- a second representation of the classification, keyed on a
/// coordinate the wrap rewrites, with its own drift surface. This carries no map
/// and writes no tag; it reads the one IR by path and does the actual work.
///
/// A SUBSCRIPTED reference to the source with NO recorded occurrence has no
/// per-axis classification to pin it with, and must never be emitted un-pinned:
/// its DIMENSION-name subscript (`pop[region, young]`) cannot resolve in a scalar
/// link-score fragment -- the fragment is dropped, the variable keeps a layout
/// slot with no bytecode, and the score reads a constant 0. Two mechanisms cover
/// it, in order:
///
/// - [`pin_dimension_name_indices`] discharges it STRUCTURALLY when every
///   dimension-name index spells one of the source's own declared dims. That is
///   the reachable case -- a `LOOKUP` TABLE argument, which the IR skips as
///   static data. Compilability, not attribution: the reference is still not a
///   causal edge and still earns no score, it just has to compile.
/// - anything the rule cannot discharge sets `unlowerable`, which the caller
///   turns into `WrapOutcome::missing_occurrence`, i.e. a warned skip. The known
///   shape is fixed; the unknown class stays LOUD.
///
/// The two other pin-only descents (a pre-existing `PREVIOUS`/`INIT` call, a
/// whole-frozen reducer) are walked by the IR, so they reach neither mechanism. A
/// BARE `Var` reaches neither either: [`pin_bare_source_ref`] pins it
/// structurally from the source's own declared dims, needing no occurrence.
pub(super) fn pin_only_source_refs(
    expr: Expr0,
    ctx: &PerElementRefCtx<'_>,
    occ: &OccurrenceLookup<'_>,
    path: &[u16],
    unlowerable: &mut bool,
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) != ctx.from {
                return expr;
            }
            match pin_bare_source_ref(ctx) {
                Some(indices) => Expr0::Subscript(ident.clone(), indices, loc),
                None => expr,
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) != ctx.from {
                // Another variable's subscript: descend into expression indices
                // and range endpoints, where a nested source reference can hide
                // (`other[from[D, young]]`, `other[from[a]:3]`). The IR records
                // occurrences under both, so skipping them would leave a
                // recorded source occurrence un-pinned -- its dimension-name
                // subscript would survive into the scalar equation, which either
                // fails to compile or reads the wrong element.
                let indices = indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, idx)| {
                        let idx_path = super::child_path(path, i);
                        match idx {
                            IndexExpr0::Expr(e) => IndexExpr0::Expr(pin_only_source_refs(
                                e,
                                ctx,
                                occ,
                                &idx_path,
                                unlowerable,
                            )),
                            IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                                pin_only_source_refs(
                                    l,
                                    ctx,
                                    occ,
                                    &super::child_path(&idx_path, 0),
                                    unlowerable,
                                ),
                                pin_only_source_refs(
                                    r,
                                    ctx,
                                    occ,
                                    &super::child_path(&idx_path, 1),
                                    unlowerable,
                                ),
                                rloc,
                            ),
                            // Wildcard / star-range / `@N` carry no `Expr0`.
                            other => other,
                        }
                    })
                    .collect();
                return Expr0::Subscript(ident, indices, loc);
            }
            let node_occ = occ.get(path);
            // With no recorded occurrence there is no per-axis classification to
            // pin with, but the fragment still has to COMPILE, so the structural
            // rule substitutes the source's own dimension-name indices by name.
            // What it cannot discharge stays loud -- see the fn docs and
            // [`pin_dimension_name_indices`]. `Some` occurrences bypass it
            // entirely, so nothing the IR classifies changes spelling.
            let (indices, discharged) = match node_occ {
                Some(_) => (indices, true),
                None => pin_dimension_name_indices(indices, ctx),
            };
            if !discharged {
                *unlowerable = true;
            }
            let indices = pin_source_subscript_indices(
                indices,
                node_occ,
                ctx,
                // Inside a frozen subtree nothing can be the live reference.
                false,
                |i, idx| {
                    // An index this occurrence's axes did not resolve: descend
                    // for a nested source reference, exactly as the
                    // other-variable arm above does -- BOTH an expression index
                    // and a range bound. A source reference nested in an
                    // expression index is reachable (a dynamic table element,
                    // `LOOKUP(pop[Region, pop[Region, young]], x)`, which
                    // `codegen::extract_table_info`'s `Expr::Subscript` arm
                    // supports), and skipping it left its dimension-name
                    // subscript in the scalar fragment with nothing set loud.
                    let idx_path = super::child_path(path, i);
                    match idx {
                        IndexExpr0::Expr(e) => IndexExpr0::Expr(pin_only_source_refs(
                            e,
                            ctx,
                            occ,
                            &idx_path,
                            unlowerable,
                        )),
                        IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                            pin_only_source_refs(
                                l,
                                ctx,
                                occ,
                                &super::child_path(&idx_path, 0),
                                unlowerable,
                            ),
                            pin_only_source_refs(
                                r,
                                ctx,
                                occ,
                                &super::child_path(&idx_path, 1),
                                unlowerable,
                            ),
                            rloc,
                        ),
                        // Wildcard / star-range / `@N` carry no `Expr0`.
                        other => other,
                    }
                },
            );
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    pin_only_source_refs(a, ctx, occ, &super::child_path(path, i), unlowerable)
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(pin_only_source_refs(
                *inner,
                ctx,
                occ,
                &super::child_path(path, 0),
                unlowerable,
            )),
            loc,
        ),
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(
            op,
            Box::new(pin_only_source_refs(
                *l,
                ctx,
                occ,
                &super::child_path(path, 0),
                unlowerable,
            )),
            Box::new(pin_only_source_refs(
                *r,
                ctx,
                occ,
                &super::child_path(path, 1),
                unlowerable,
            )),
            loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(pin_only_source_refs(
                *c,
                ctx,
                occ,
                &super::child_path(path, 0),
                unlowerable,
            )),
            Box::new(pin_only_source_refs(
                *t,
                ctx,
                occ,
                &super::child_path(path, 1),
                unlowerable,
            )),
            Box::new(pin_only_source_refs(
                *f,
                ctx,
                occ,
                &super::child_path(path, 2),
                unlowerable,
            )),
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
