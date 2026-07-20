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

/// What [`pin_dimension_name_indices`] can say about ONE index of a source
/// subscript the occurrence IR records nothing for.
enum IndexVerdict {
    /// A static selector this rule rewrites: an iterated coordinate, or a literal
    /// element qualified with its own axis.
    Pinned(String),
    /// A static selector already spelled the way this rule would spell it (a
    /// numeric literal, an `@N` position, an already-`dim·elem` name).
    Static,
    /// No pin can spell it, because the SHARED row derivation
    /// ([`per_element_row_for_target`]) cannot resolve the axis: a dimension the
    /// target does not iterate, an iterated dimension with no usable positional
    /// correspondence to this source axis (unmapped, element-mapped, or a
    /// transposition), an index no axis owns. Left alone it keeps a
    /// DIMENSION-name subscript, which cannot resolve in a scalar fragment, so
    /// this is loud ALWAYS -- a compilability verdict, independent of freezing.
    Unspellable,
    /// A RUNTIME read selecting the element: a variable (`pop[Region, idx]`) or a
    /// nested expression (`pop[Region, pop[Region, old]]`). It COMPILES as it
    /// stands, so this is purely a ceteris-paribus question -- see the `frozen`
    /// discussion on [`pin_dimension_name_indices`].
    RuntimeRead,
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
/// It consults NO occurrence and infers NO shape. Per index, by name:
///
/// - an index spelling one of the TARGET's ITERATED dimensions is replaced by the
///   source element this target element reads on that axis -- derived by handing
///   an [`crate::ltm_agg::AxisRead::Iterated`] for the `(index dim, source axis
///   dim)` pair to [`per_element_row_for_target`], the SAME single row derivation
///   the occurrence-driven pin uses. That is what makes the identity axis
///   (`pop[Region, ..]` over a `Region` axis, the structural substitution
///   [`pin_bare_source_ref`] performs for a bare `Var`) and a positionally-MAPPED
///   axis (`effect[State, ..]` over a `Region` axis with a `State`/`Region`
///   mapping, either declaration direction -- GH #527 / #757) ONE arm rather than
///   two: the derivation resolves both through
///   `DimensionsContext::mapped_element_correspondence`, so this rule accepts
///   EXACTLY the mapped pairs `ltm_agg::classify_axis_access` accepts (that
///   classifier's `Iterated` arm gates on `iterated_axis_slot_elements`, the
///   preimage inversion of the same correspondence). An axis the derivation
///   declines -- no mapping, an explicit element map (GH #756: execution resolves
///   positionally and ignores it), a transposition, a dimension this target does
///   not iterate -- is `IndexVerdict::Unspellable`, and it is unspellable because
///   the SHARED derivation says so, not because the name differs;
/// - otherwise, an index the source's axis at that position DECLARES as an element
///   (or an already-`dim·elem`-qualified one) is a literal selector, qualified with
///   that axis (`old` -> `age·old`). It would very likely resolve bare too, but the
///   pin qualifies EVERY index of a row for a reason (see
///   `wrap_non_matching_in_previous`'s `skip_index_qualification`): the wrap's
///   generic `qualify_element_index` cannot qualify an element name several
///   dimensions declare, so a half-qualified subscript is the one spelling whose
///   compilability depends on the model's element names. Qualifying here also
///   makes this rule's output byte-identical to the pre-`391bc3c1` pass's, which
///   is the conservative thing for a regression fix to be. The iterated-dim arm is
///   tried FIRST, mirroring `classify_axis_access`'s own precedence, so a name that
///   is both an iterated dimension and some axis's element resolves the same way in
///   both;
/// - an index that selects a FIXED element is kept verbatim, because it needs no pin
///   and reads nothing at the current step. Three spellings qualify: a numeric
///   literal, arithmetic over numeric literals (`1 + 1` -- see
///   [`index_expr_selects_a_fixed_element`], whose base case is that literal), and an
///   `@N` POSITION index, which `compiler::context`'s subscript lowering resolves to
///   a concrete element offset in scalar context (which a link-score fragment is).
///   Spelling `@N` out as an element name here would be a SECOND implementation of
///   position syntax living outside the compiler that owns it; keeping it verbatim
///   leaves the one.
///
/// Everything else is one of the two loud verdicts on [`IndexVerdict`], and
/// `frozen` is what separates them. `IndexVerdict::Unspellable` is loud
/// unconditionally: it is a COMPILABILITY verdict. `IndexVerdict::RuntimeRead` is
/// loud only when `frozen` is false, and that is a CETERIS-PARIBUS verdict:
///
/// - a bare `LOOKUP` table argument is NOT inside a freeze -- the wrap holds it
///   verbatim rather than wrapping it, since a `PREVIOUS` of a table has no value
///   slot -- so a runtime index there stays LIVE in the emitted partial.
///   `codegen::extract_table_info` evaluates it to select the table element, so
///   the partial isolating one row would vary with the current-step value of
///   another. A descent that wraps nothing cannot fix that, and a
///   compilable-but-wrong score is worse than none (GH #311 / #661 / #743), so the
///   partial is abandoned;
/// - but the SAME `LOOKUP` nested inside a pre-existing `PREVIOUS`/`INIT`, inside
///   a whole-frozen reducer, or inside a frozen other-dep's subscript, IS already
///   lagged -- index reads included -- so ceteris paribus already holds and
///   refusing would drop a perfectly good score for no reason.
///
/// So the caller passes the freeze context it alone knows, exactly as the wrap
/// threads its own `frozen` flag beside `path`.
///
/// There is no `RefShape` here, no live-vs-frozen decision, and this can never
/// make the reference live-selectable (the pin-only descent records no
/// `live_ref`). It builds an `AxisRead` only to ASK the shared row derivation a
/// question; it never decides an access shape. Do NOT grow it into a per-axis
/// classifier: the second classifier family was deleted on purpose (`391bc3c1`),
/// and every per-axis question it needs answered is already answered by
/// [`per_element_row_for_target`] and the `DimensionsContext` beneath it.
///
/// (`ltm_agg::classify_axis_access` -- the per-axis classifier the IR itself uses
/// -- is deliberately NOT the helper consulted here, for two reasons. It consumes
/// `IndexExpr2`, and this descent walks the `Expr0` the wrap lowered. More
/// importantly it answers a DIFFERENT question: "is this axis access statically
/// describable enough to hoist a reducer / emit an element edge?" -- which is why
/// it declines `@N`, correctly for hoisting and wrongly for compilability, and why
/// it has a `Reduced` verdict that means nothing to a pin. The shared answer this
/// rule needs is one level down, at the row derivation and the correspondence, and
/// both classifiers bottom out there.)
///
/// The caller turns `discharged == false` into `WrapOutcome::missing_occurrence`,
/// i.e. a warned skip.
fn pin_dimension_name_indices(
    indices: Vec<IndexExpr0>,
    ctx: &PerElementRefCtx<'_>,
    frozen: bool,
) -> (Vec<IndexExpr0>, bool) {
    let mut discharged = true;
    let indices = indices
        .into_iter()
        .enumerate()
        .map(|(i, idx)| {
            let (name, loc) = match &idx {
                // An `@N` POSITION selector is static -- `compiler::context`
                // resolves it to a concrete element offset in scalar context -- so
                // it neither needs a pin nor reads anything at the current step.
                IndexExpr0::DimPosition(..) => return idx,
                IndexExpr0::Expr(Expr0::Var(name, loc)) => {
                    (crate::common::canonicalize(name.as_str()).to_string(), *loc)
                }
                // A numeric selector, and arithmetic over numeric selectors, is
                // static: nothing to pin, nothing to lag. See
                // [`index_expr_selects_a_fixed_element`] -- the bare `Const` is just
                // its base case.
                IndexExpr0::Expr(e) if index_expr_selects_a_fixed_element(e) => return idx,
                // Any other compound index expression selects the element at
                // runtime. (A range, wildcard or star-range cannot be a table index
                // at all -- `codegen::extract_table_info` needs a subscript selecting
                // exactly ONE element and rejects anything wider as `BadTable` -- so
                // treating them the same way costs nothing.)
                _ => {
                    return verdict_into_index(
                        IndexVerdict::RuntimeRead,
                        idx,
                        frozen,
                        &mut discharged,
                    );
                }
            };
            let Some(dim) = ctx.from_dims.get(i) else {
                // Over-arity: no axis owns this index, so nothing names its owner
                // and there is nothing to resolve it against.
                return verdict_into_index(IndexVerdict::Unspellable, idx, frozen, &mut discharged);
            };
            let verdict = if ctx.dim_ctx.lookup(&name).is_some() {
                IndexVerdict::Static
            } else if ctx.target_elem_by_dim.contains_key(&name) {
                // The index names one of the TARGET's ITERATED dimensions, so this
                // axis reads whatever element this target element projects onto it.
                // WHICH element that is -- the identity for a same-named axis, the
                // positional correspondence for a mapped one -- is the shared row
                // derivation's answer, not this rule's: it declines an unmapped or
                // element-mapped pair, and a name-directed pin therefore accepts
                // exactly the pairs the occurrence-driven one does.
                let axis = crate::ltm_agg::AxisRead::Iterated {
                    dim: name.clone(),
                    source_dim: dim.name().to_string(),
                };
                match per_element_row_for_target(
                    std::slice::from_ref(&axis),
                    ctx.target_elem_by_dim,
                    ctx.dim_ctx,
                ) {
                    Some(row) => IndexVerdict::Pinned(qualify_axis_element(&row[0], dim)),
                    None => IndexVerdict::Unspellable,
                }
            } else {
                // A literal element selector of THIS axis qualifies to `dim·elem`;
                // `qualify_axis_element` returns the name unchanged for anything
                // the axis does not declare. An unchanged name is therefore NOT a
                // static selector: either a dimension name no target coordinate
                // projects onto this axis (unspellable), or a variable read
                // selecting the element at runtime.
                let qualified = qualify_axis_element(&name, dim);
                if qualified != name {
                    IndexVerdict::Pinned(qualified)
                } else if ctx.dim_ctx.is_dimension_name(&name) {
                    IndexVerdict::Unspellable
                } else {
                    IndexVerdict::RuntimeRead
                }
            };
            let verdict = match verdict {
                // A pin that changes nothing keeps its own node.
                IndexVerdict::Pinned(part) if part == name => IndexVerdict::Static,
                other => other,
            };
            match verdict {
                IndexVerdict::Pinned(part) => {
                    IndexExpr0::Expr(Expr0::Var(RawIdent::new_from_str(&part), loc))
                }
                other => verdict_into_index(other, idx, frozen, &mut discharged),
            }
        })
        .collect();
    (indices, discharged)
}

/// Whether a subscript index expression selects a FIXED element -- built entirely
/// from numeric literals, so it reads nothing at any step and cannot vary with the
/// ceteris-paribus wrap's choice of what to hold live.
///
/// This is the predicate behind `IndexVerdict::Static` for a compound index. Its
/// base case is the bare `Const` the rule always left alone; `LOOKUP(pop[Region,
/// 1 + 1], x)` is exactly as static as `LOOKUP(pop[Region, 2], x)` and the rule used
/// to decline the first only because its catch-all never looked inside.
///
/// It is deliberately the NARROWEST sound predicate, not a general invariance test.
/// `compiler::invariance::exprs_are_invariant` is the engine's run-invariance
/// derivation, but it consumes LOWERED `Expr` plus an offset-classification
/// callback, neither of which exists in this position -- and its notion is *wider*
/// than this rule's obligation anyway: `Dt`, `StartTime` and `INIT(x)` of any
/// variable are all run-invariant, yet whether a table index may read a variable's
/// init buffer is an ATTRIBUTION question, and attribution is the wrap's vocabulary,
/// not this rule's. So this decides only the half it can decide alone.
///
/// The match is exhaustive over `Expr0` on purpose -- no catch-all -- so a new
/// variant forces a decision here rather than silently inheriting `false`:
///
/// - `Var` reads model state (or names an element, which the caller's own name arms
///   resolved before reaching this predicate);
/// - `Subscript` reads an array element;
/// - `App` is where a 0-arity builtin lands after `reify_0_arity_builtins`, and
///   `TIME` and `PI` are indistinguishable here without re-implementing the builtin
///   classification that `builtins`/`compiler::invariance` own. So the whole arm
///   stays `false`: a `PI`-indexed table declines CONSERVATIVELY (a loud skip, the
///   safe direction) rather than being sorted by a fourth copy of that knowledge.
fn index_expr_selects_a_fixed_element(expr: &Expr0) -> bool {
    match expr {
        Expr0::Const(..) => true,
        Expr0::Op1(_, inner, _) => index_expr_selects_a_fixed_element(inner),
        Expr0::Op2(_, lhs, rhs, _) => {
            index_expr_selects_a_fixed_element(lhs) && index_expr_selects_a_fixed_element(rhs)
        }
        Expr0::If(cond, then_e, else_e, _) => {
            index_expr_selects_a_fixed_element(cond)
                && index_expr_selects_a_fixed_element(then_e)
                && index_expr_selects_a_fixed_element(else_e)
        }
        Expr0::Var(..) | Expr0::Subscript(..) | Expr0::App(..) => false,
    }
}

/// Apply a non-rewriting [`IndexVerdict`]: keep the index as it stands, and clear
/// `discharged` when the verdict is loud in this freeze context (see
/// [`pin_dimension_name_indices`] -- `Unspellable` always, `RuntimeRead` only
/// outside a freeze).
fn verdict_into_index(
    verdict: IndexVerdict,
    idx: IndexExpr0,
    frozen: bool,
    discharged: &mut bool,
) -> IndexExpr0 {
    match verdict {
        IndexVerdict::Pinned(_) | IndexVerdict::Static => {}
        IndexVerdict::Unspellable => *discharged = false,
        IndexVerdict::RuntimeRead => {
            if !frozen {
                *discharged = false;
            }
        }
    }
    idx
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
    frozen: bool,
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
                                frozen,
                            )),
                            IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                                pin_only_source_refs(
                                    l,
                                    ctx,
                                    occ,
                                    &super::child_path(&idx_path, 0),
                                    unlowerable,
                                    frozen,
                                ),
                                pin_only_source_refs(
                                    r,
                                    ctx,
                                    occ,
                                    &super::child_path(&idx_path, 1),
                                    unlowerable,
                                    frozen,
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
                None => pin_dimension_name_indices(indices, ctx, frozen),
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
                            frozen,
                        )),
                        IndexExpr0::Range(l, r, rloc) => IndexExpr0::Range(
                            pin_only_source_refs(
                                l,
                                ctx,
                                occ,
                                &super::child_path(&idx_path, 0),
                                unlowerable,
                                frozen,
                            ),
                            pin_only_source_refs(
                                r,
                                ctx,
                                occ,
                                &super::child_path(&idx_path, 1),
                                unlowerable,
                                frozen,
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
                    pin_only_source_refs(
                        a,
                        ctx,
                        occ,
                        &super::child_path(path, i),
                        unlowerable,
                        frozen,
                    )
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
                frozen,
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
                frozen,
            )),
            Box::new(pin_only_source_refs(
                *r,
                ctx,
                occ,
                &super::child_path(path, 1),
                unlowerable,
                frozen,
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
                frozen,
            )),
            Box::new(pin_only_source_refs(
                *t,
                ctx,
                occ,
                &super::child_path(path, 1),
                unlowerable,
                frozen,
            )),
            Box::new(pin_only_source_refs(
                *f,
                ctx,
                occ,
                &super::child_path(path, 2),
                unlowerable,
                frozen,
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
