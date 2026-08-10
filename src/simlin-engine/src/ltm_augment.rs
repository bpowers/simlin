// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM project augmentation - adds synthetic variables for link and loop scores
//!
//! This module generates synthetic variables for Loops That Matter (LTM) analysis.
//! The generated equations use the intrinsic two-argument `PREVIOUS(value, initial)`
//! function. First- and second-timestep guards are expressed explicitly with
//! `TIME = INITIAL_TIME` and `PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME`.

use crate::ast::{Expr0, IndexExpr0, print_eqn};
use crate::builtins::UntypedBuiltinFn;
use crate::canonicalize;
use crate::common::{Canonical, Ident, RawIdent};
use crate::datamodel::{self, Equation};
use crate::lexer::LexerType;
use crate::ltm::{Loop, normalize_module_ref, split_node_subscript, strip_subscript};
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};

use crate::db::LtmEquation;
use crate::db::RefShape;
use crate::db::ltm_ir::{
    OccurrenceAxis, OccurrenceSite, OtherDepVerdict, derive_other_dep_verdict,
};

/// The wrap's read side of the occurrence IR ([`SlotOccurrences`] /
/// [`OccurrenceLookup`]), in its own file only to keep this one under the
/// project line-count lint. Re-exported so callers keep naming them
/// `crate::ltm_augment::*`.
#[path = "ltm_augment_occurrence.rs"]
mod occurrence;

pub(crate) use occurrence::{OccurrenceLookup, SlotOccurrences};

/// The implicit WITH-LOOKUP rules (GH #910), in their own file only to keep this
/// one under the project line-count lint. Re-exported below so callers keep
/// naming them `crate::ltm_augment::*`.
#[path = "ltm_augment_with_lookup.rs"]
mod with_lookup;

pub(crate) use with_lookup::{
    WithLookupSlotRefs, WithLookupWrap, with_lookup_reducer_owner_wrap, with_lookup_table_ref,
};

/// The post-transform lowering machinery (Track A stage 1), in its own file
/// only to keep this one under the project line-count lint. These lowerings run
/// AFTER the ceteris-paribus wrap on the target's own equation, so the wrap
/// stays occurrence-addressable for stage 2/3; see the module rustdoc. The
/// parent-only helpers are imported for the `generate_*` callers below; the
/// externally-referenced row derivation and the test-only text wrapper are
/// re-exported so callers keep naming them `crate::ltm_augment::*`.
#[path = "ltm_augment_post_transform.rs"]
mod post_transform;

#[cfg(test)]
pub(crate) use post_transform::substitute_reducers_in_equation;
use post_transform::{PerElementRefCtx, qualify_axis_element, substitute_reducers_in_expr0};
pub(crate) use post_transform::{dep_element_pins, per_element_row_for_target};

/// Context for recognizing GH #511 iterated-dimension source references in
/// the partial-equation builder: the live source's declared dimension names
/// (canonical, in declaration order; same length as `source_dim_elements`) and
/// the target equation's iterated dimension names (canonical, in the order
/// they appear on `Ast::ApplyToAll`/`Ast::Arrayed`). `build_partial_equation_shaped`
/// is passed `None` by callers whose live source is a scalar (or an aggregate
/// node) -- those have no source subscripts, so iterated-dim recognition
/// never applies.
///
/// It used to carry a `DimensionsContext` too, for the AC3.5 mapped-dimension
/// case. That was a duplicate of [`WrapCtx::dims_ctx`] -- production threaded the
/// same context into both -- kept alive solely by the Expr0 per-axis classifiers.
/// With those gone from production the field went with them; the remaining
/// `#[cfg(test)]` classifiers take the one `dims_ctx` explicitly.
pub(crate) struct IteratedDimCtx<'a> {
    pub source_dim_names: &'a [String],
    pub target_iterated_dims: &'a [String],
    /// Declared dimensions of the target's NON-LIVE array deps, keyed by
    /// canonical dep name (GH #526). Threaded by the db-bearing per-shape
    /// link-score path so the other-dep verdict ([`other_dep_verdict`], via the
    /// occurrence IR's per-axis classification) can require position-and-mapping
    /// correspondence before collapsing an iterated-dim subscript on a non-live
    /// dep to a bare `PREVIOUS(dep)`. `None` (or a dep absent from the map -- an
    /// implicit/synthetic name with no resolvable declaration) keeps the
    /// historical permissive collapse: declaring an unresolvable dep a
    /// mismatch would loud-skip edges that are correct today.
    pub dep_dims: Option<&'a HashMap<String, Vec<crate::dimensions::Dimension>>>,
}

/// Does `name` (case-insensitively) name an array-reducing builtin in the
/// form that collapses an array dimension? `SUM`/`STDDEV`/`SIZE`/`RANK`
/// reduce at any arity (`RANK(arr, n)`, etc.); `MEAN`/`MIN`/`MAX` reduce an
/// array dimension only in their single-argument form (their multi-argument
/// forms are element-wise). The lowercasing is defensive belt-and-suspenders:
/// parsed `Expr0` builtin names are already lowercase by construction (the
/// parser lowercases function-call identifiers; LTM-generated uppercase
/// reducer text is re-parsed before any of these predicates see it).
/// A thin reader of [`crate::ltm_agg::reducer_kind_from_name`]
/// -- the one reducer table -- so this `Expr0`-walk-time check and the agg
/// enumerator agree on the set (including `SIZE`, which is recognized here
/// even though it is never hoisted).
fn is_array_reducer_name(name: &str, arity: usize) -> bool {
    crate::ltm_agg::reducer_kind_from_name(&name.to_ascii_lowercase(), arity).is_some()
}

/// Whether any subexpression of `expr` prints exactly as `reducer_text` -- the
/// [`WrapCtx::live_reducer_text`] containment test (Track A stage 1, finding 2).
///
/// The GH #517 arm of [`wrap_non_matching_in_previous`] freezes a whole array
/// reducer that carries no live reference. But when the hoisted reducer held
/// LIVE (matched by text at the top of the wrap) is itself NESTED inside a
/// DECLINED (non-hoisted) outer reducer -- `SUM(matrix[D,*] * SUM(pop[*]))`,
/// where the inner `SUM(pop[*])` became the agg -- the whole outer reducer would
/// freeze before the recursion ever reaches the inner one, silently converting
/// HEAD's live-agg-inside-a-frozen-slice partial (which fails to compile: a loud
/// warned zero) into a structurally-always-zero frozen partial. This predicate
/// lets that enclosing reducer recurse instead of freeze, so the inner
/// held-live reducer is reached (and later substituted to its agg name),
/// preserving HEAD's text and diagnostic surface. Only an `App` can be the
/// hoisted reducer, so the `print_eqn` comparison is confined to `App` nodes.
fn expr0_contains_reducer_text(expr: &Expr0, reducer_text: &str) -> bool {
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => false,
        Expr0::Subscript(_, indices, _) => indices.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => expr0_contains_reducer_text(e, reducer_text),
            IndexExpr0::Range(l, r, _) => {
                expr0_contains_reducer_text(l, reducer_text)
                    || expr0_contains_reducer_text(r, reducer_text)
            }
            _ => false,
        }),
        Expr0::App(UntypedBuiltinFn(_, args), _) => {
            print_eqn(expr) == reducer_text
                || args
                    .iter()
                    .any(|a| expr0_contains_reducer_text(a, reducer_text))
        }
        Expr0::Op1(_, inner, _) => expr0_contains_reducer_text(inner, reducer_text),
        Expr0::Op2(_, l, r, _) => {
            expr0_contains_reducer_text(l, reducer_text)
                || expr0_contains_reducer_text(r, reducer_text)
        }
        Expr0::If(c, t, e, _) => {
            expr0_contains_reducer_text(c, reducer_text)
                || expr0_contains_reducer_text(t, reducer_text)
                || expr0_contains_reducer_text(e, reducer_text)
        }
    }
}

/// Out-channels of [`wrap_non_matching_in_previous`], threaded through the
/// recursion as one mutable sink.
#[derive(Default)]
struct WrapOutcome {
    /// The *first* `live_source` occurrence left live (in document order,
    /// after the transform); see the `live_ref` paragraph on
    /// [`wrap_non_matching_in_previous`].
    live_ref: Option<Expr0>,
    /// GH #526: set when a KNOWN position-mismatched other-dep iterated
    /// subscript was encountered ([`OtherDepVerdict::Mismatch`]).
    /// The changed-first partial is then unusable -- collapsing would
    /// freeze the wrong element, and not collapsing leaves a
    /// `PREVIOUS(Subscript(dim-name indices))` whose capture helper cannot
    /// compile -- so callers abandon it: `shaped_guard_form_text` falls to
    /// the changed-last convention (which keeps the dep live and
    /// verbatim), and `build_partial_equation_shaped_with_live_ref`
    /// returns the loud `UnfreezablePartial` error.
    other_dep_mismatch: bool,
    /// Set when a live-source subscript node had NO occurrence at its tracked
    /// structural path even though the slot's occurrence stream is non-empty --
    /// a walker desync (the reparsed-`Expr0` walk drifted from the IR's `Expr2`
    /// `SiteId` numbering, which `assert_occurrence_stream_aligns` proves cannot
    /// happen on the covered corpus but a NOVEL production shape might). On a
    /// miss the shape lookup returns `None`, `node_shape == Some(live_shape)`
    /// fails, and the live reference would be silently FROZEN -- a
    /// `PREVIOUS(...)` that compiles cleanly and zeroes the score. That silent
    /// zero is exactly the failure the single-classifier flip is meant to
    /// eliminate, so the miss is surfaced LOUDLY instead: callers abandon the
    /// partial (`shaped_guard_form_text` returns `Err` without even trying the
    /// changed-last dual, which reads the SAME desynced stream, and
    /// `build_partial_equation_shaped_with_live_ref` returns
    /// `UnfreezablePartial`), and the db-bearing emitters turn that into a
    /// skip-and-warn.
    ///
    /// The pin-only descents set it for a second, non-desync reason: a source
    /// subscript the IR records NOTHING for (a `LOOKUP` table argument) that
    /// `post_transform::pin_dimension_name_indices` could not lower by name
    /// either. Same contract, same reason -- an un-pinned dimension-name
    /// subscript in a scalar fragment is the same silent zero.
    missing_occurrence: bool,
}

/// The immutable parameters of the ceteris-paribus wrap
/// ([`wrap_non_matching_in_previous`] / [`wrap_index_non_matching_in_previous`]),
/// bundled so the deep recursion threads one `&WrapCtx` instead of a long
/// positional argument list. Every field is a `Copy` reference, so the body
/// destructures it back into the historically-named locals with no clones --
/// and the whole struct is `Copy`, which lets the `LOOKUP` table-argument
/// descent rebind exactly one field (see [`OccurrenceLookup::empty`]).
#[derive(Clone, Copy)]
struct WrapCtx<'a> {
    /// The source variable whose live shape is held out of `PREVIOUS`
    /// wrapping; all its other occurrences (and every other-dep reference)
    /// are wrapped.
    live_source: &'a Ident<Canonical>,
    /// Which access shape of `live_source` stays live; other shapes wrap.
    live_shape: &'a RefShape,
    /// Track A stage 1 (opt-in): a hoisted reducer subexpression to hold LIVE
    /// verbatim, matched by its canonical `print_eqn` text. When set, an `App`
    /// whose printed form equals this is the live thing (recorded as
    /// `live_ref`, left verbatim, never recursed into), and every OTHER
    /// recognized reducer freezes whole (the GH #517 path). This lets the
    /// caller run the wrap on the target's OWN equation and substitute all
    /// reducers to their agg names AFTER the wrap -- the held-live one to a
    /// bare agg name, the frozen co-reducers to `PREVIOUS(agg)` -- rather than
    /// wrapping already-agg-substituted text (the inverted composition). `None`
    /// for the ordinary ident-live callers, who are byte-for-byte unaffected.
    live_reducer_text: Option<&'a str>,
    /// Canonical idents of the non-`live_source` deps that must be wrapped;
    /// names outside this set (and not `live_source`) are left alone.
    other_deps: &'a HashSet<Ident<Canonical>>,
    /// GH #511 iterated-dimension context (`None` for a scalar live source).
    iter_ctx: Option<&'a IteratedDimCtx<'a>>,
    /// Project dims context for [`qualify_element_index`] (GH #587).
    dims_ctx: Option<&'a crate::dimensions::DimensionsContext>,
    /// The occurrence IR for the slot being wrapped -- the single classifier
    /// family. Every per-occurrence access-shape / iterated-collapse /
    /// other-dep-verdict decision the wrap makes is a lookup here by the
    /// occurrence's structural path (tracked as the wrap descends), not a
    /// re-derivation on the reparsed `Expr0`.
    occ: &'a OccurrenceLookup<'a>,
    /// True while the descent is inside a subscript INDEX expression, which is
    /// the one position where a frozen read's FIRST-DT value has to be chosen
    /// rather than defaulted. See [`freeze_at_previous`] (GH #975).
    in_subscript_index: bool,
    /// The `PerElement` row-pinning context (GH #525, T6), `Some` only for
    /// [`generate_per_element_link_equation`].
    ///
    /// When set, the wrap ALSO lowers each live-source reference to its concrete
    /// per-element subscript as it goes. That used to be a separate pass over
    /// the WRAPPED tree, which had to re-derive each occurrence's per-axis
    /// access with an Expr0 classifier because a `SiteId` computed on the
    /// original AST cannot address a tree the wrap has inserted `PREVIOUS` nodes
    /// into. Folding it into the wrap deletes that classifier: here the
    /// occurrence is still reachable by path, and -- decisively -- the wrap is
    /// the only place that knows whether it is about to FREEZE the reference,
    /// which is what selects the bare-row spelling for the live occurrence and
    /// the qualified-row spelling for every other one. See
    /// [`post_transform::pin_source_subscript_indices`].
    pin: Option<&'a PerElementRefCtx<'a>>,
}

/// The first-DT initial value of every `PREVIOUS` the LTM walkers SYNTHESIZE
/// ([`freeze_at_previous`], GH #975), in its own file only to keep this one
/// under the project line-count lint.
#[path = "ltm_augment_freeze.rs"]
mod freeze;

use freeze::freeze_at_previous;

/// Materializing an array-slice freeze as its own synthetic variable
/// (GH #995 option B), in its own file only to keep this one under the
/// project line-count lint.
#[path = "ltm_augment_array_freeze.rs"]
mod array_freeze;

pub(crate) use array_freeze::{ArrayFreezeHelper, FREEZE_HELPER_PREFIX, materialize_array_freezes};

/// Deciding when a per-element link-score arm is provably `PREVIOUS(target)`
/// and may therefore be OMITTED rather than materialized (GH #977), in its own
/// file only to keep this one under the project line-count lint.
#[path = "ltm_augment_zero_slot.rs"]
mod zero_slot;

pub(crate) use zero_slot::ZeroSlotPolicy;
use zero_slot::partial_is_provably_previous_target;

/// Append child index `i` to `path`, yielding the child node's structural path.
/// The wrap's recursion mirrors `db::ltm_ir::walk_all_in_expr`'s child-index
/// construction exactly, so the path at any node equals that occurrence's
/// `SiteId` (minus the slot prefix, which [`SlotOccurrences::for_slot`] already
/// stripped) -- the invariant
/// `classifier_agreement_tests::assert_occurrence_stream_aligns` proves
/// corpus-wide. Cloning per descent is cheap: LTM equations are short.
///
/// `i` fits a `u16` by the LTM front door, not by luck: `model_ltm_reference_sites`
/// refuses a target equation needing more than
/// [`MAX_SITE_CHILDREN`](crate::db::ltm_ir::MAX_SITE_CHILDREN) children at one
/// level, and `model_ltm_variables` then emits no LTM variable for that model at
/// all -- so the wrap never runs on an equation whose child indices could
/// overflow. **The front door is the whole of the soundness argument here.**
///
/// The conversion saturates rather than wrapping, which is strictly better --
/// wrapping maps child 65,536 onto child 0, so it can alias an ARBITRARY earlier
/// sibling. But saturating is not a safety net: this change deleted the reserved
/// unaddressable-child sentinel that used to hold `u16::MAX` back, so `u16::MAX`
/// is now an ordinary, addressable child index (pinned by `db::ltm_ir::ltm_ir_tests`'
/// `the_production_limit_is_the_whole_u16_range`). A violated precondition would
/// therefore land on sibling 65,535's real recorded `SiteId` -- exactly the alias
/// the sentinel used to make impossible. Do not read the saturation as
/// protection; read it as "the failure mode is one specific collision instead of
/// an arbitrary one, and the front door is what keeps it unreachable".
///
/// It is not a `panic`/`expect` because release builds use `panic = abort`:
/// aborting the host process is a worse answer than an unreachable collision on
/// a model for which no link score is generated at all.
fn child_path(path: &[u16], i: usize) -> Vec<u16> {
    let mut v = Vec::with_capacity(path.len() + 1);
    v.extend_from_slice(path);
    v.push(u16::try_from(i).unwrap_or(u16::MAX));
    v
}

/// The `Collapse` / `Mismatch` / `NotIterated` verdict for an iterated-dimension
/// subscript on a NON-live-source dependency, derived from the occurrence IR's
/// per-axis classification (`node_occ.axes`) plus the two arity facts the
/// occurrence does not carry: the dep's declared arity (from `iter_ctx.dep_dims`)
/// and the target's iterated-dim count. Delegates to the single-sourced
/// [`derive_other_dep_verdict`] -- the SAME rule `db::ltm_ir` feeds the edge
/// emitter -- so the wrap and the emitter cannot disagree on the verdict.
///
/// `None` (no recorded occurrence, or no `iter_ctx` -- the scalar/agg callers
/// with no iterated-dimension space) yields `NotIterated`: there is no iterated
/// collapse to perform.
fn other_dep_verdict(
    node_occ: Option<&OccurrenceSite>,
    dep: &str,
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> OtherDepVerdict {
    let (Some(occ), Some(ic)) = (node_occ, iter_ctx) else {
        return OtherDepVerdict::NotIterated;
    };
    let dep_arity = ic.dep_dims.and_then(|m| m.get(dep)).map(|d| d.len());
    derive_other_dep_verdict(&occ.axes, dep_arity, ic.target_iterated_dims.len())
}

/// Walk an `Expr0` tree and wrap variable references in `PREVIOUS()` except
/// those whose access shape matches the live shape for the given source,
/// recording into `out.live_ref` the *first* `live_source` occurrence left
/// live (in document order, after the transform) and into
/// `out.other_dep_mismatch` whether a GH #526 mismatched other-dep
/// subscript doomed the changed-first form.
///
/// `ctx.live_source` identifies the source variable whose live shape is held
/// out from `PREVIOUS` wrapping. `ctx.live_shape` declares which AST
/// occurrences of that source remain live; all other occurrences (and all
/// references to other sources in the same expression) are wrapped.
///
/// `ctx.other_deps` is the set of canonical idents for non-`live_source`
/// dependencies that must be wrapped; nodes referencing names not in this
/// set and not equal to `live_source` are left alone (function names and
/// unknown identifiers). Indices of subscripts are recursively transformed
/// even when the outer subscript matches the live shape, so nested
/// references like `arr[other_var]` still get wrapped.
///
/// `ctx.iter_ctx` carries the GH #511 iterated-dimension context (the live
/// source's declared dimension names + the target equation's iterated
/// dimensions + a `DimensionsContext` for the mapped case). An
/// iterated-dimension subscript on the live source is normalized to a bare
/// `Var` *before* the live/PREVIOUS dispatch -- so `row_sum[D1]` (a
/// same-element reference over the target's own `D1`) becomes either the live
/// ref (`live_shape == Bare`) or `PREVIOUS(Var(row_sum))` (a `Var` arg, which
/// codegen accepts, vs the `PREVIOUS(Subscript(...))` the pre-#511 code
/// produced). The normalization itself is driven by the occurrence IR's shape
/// (`ctx.occ`), not by `iter_ctx`; `iter_ctx` remains the other-dep verdict's
/// arity/mapping source and the dimension-name index guard's fallback. Pass
/// `None` for callers whose live source is scalar (no source subscripts).
///
/// `ctx.occ` is the occurrence IR for the slot being wrapped -- the single
/// classifier family. `path` is the structural child-index path of `expr`
/// within that slot (empty at the slot root), tracked as the recursion
/// descends so it equals the occurrence's `SiteId`; see [`OccurrenceLookup`]
/// and [`child_path`].
///
/// `out.live_ref` ends up holding the bare `Var(live_source)` for a `Bare`
/// shape, or the (already index-transformed) `Subscript(live_source, ...)`
/// for `FixedIndex`/`Wildcard`/`DynamicIndex`. Callers use this captured
/// subtree to build the link-score's source-side normalizer: it is the
/// source reference *as the partial isolates it*, so `Δ(live_ref)` is the
/// exact source velocity feeding the `SIGN` factor -- crucially, a
/// per-element / per-slice expression rather than the (possibly
/// multi-dimensional) bare `live_source`, which would be a dimension error
/// in a scalar link-score equation.
///
/// `ctx.dims_ctx` is the project-wide dimensions context used by
/// [`qualify_element_index`] to recognize (and qualify) subscript indices
/// that name dimension elements -- so they are never PREVIOUS-wrapped as if
/// they were causal references (GH #587). `None` (test-only callers, or
/// paths without project dims in scope) disables qualification, keeping the
/// conservative wrapping behavior.
fn wrap_non_matching_in_previous(
    expr: Expr0,
    ctx: &WrapCtx<'_>,
    out: &mut WrapOutcome,
    path: &[u16],
    frozen: bool,
) -> Expr0 {
    // `dims_ctx` is consumed only by `wrap_index_non_matching_in_previous` (via
    // `ctx`); `source_dim_elements` / `iter_ctx` are no longer read here (the
    // occurrence IR carries the shape) so they are not bound.
    let &WrapCtx {
        live_source,
        live_shape,
        live_reducer_text,
        other_deps,
        in_subscript_index,
        ..
    } = ctx;
    // Track A stage 1: hold a designated hoisted reducer subexpression LIVE
    // verbatim (matched by canonical text). Only an `App` can be a reducer, and
    // the post-transform substitution renames it to its agg name in place. The
    // check is skipped for the ordinary ident-live callers (`None`), so their
    // output is byte-identical. See [`WrapCtx::live_reducer_text`].
    if let Some(reducer_text) = live_reducer_text
        && matches!(expr, Expr0::App(..))
        && print_eqn(&expr) == reducer_text
    {
        if out.live_ref.is_none() {
            out.live_ref = Some(expr.clone());
        }
        return expr;
    }
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(ident.as_str());
            if &canonical == live_source {
                // `PerElement` row pinning: a BARE reference to the source (the
                // mixed `Bare`+`PerElement` edge's other site) reads the target
                // element's projection onto the source's own axes. Pin it FIRST,
                // then take the ordinary live/wrap decision on the result -- a
                // `PerElement` live shape never matches a bare reference, so the
                // pinned subscript is what gets frozen, and a `PREVIOUS` of a
                // qualified element subscript is the direct LoadPrev the scalar
                // fragment needs.
                let expr = match ctx.pin.and_then(post_transform::pin_bare_source_ref) {
                    Some(indices) => Expr0::Subscript(ident.clone(), indices, loc),
                    None => expr,
                };
                // The bare-Var occurrence matches `Bare`. Any other live
                // shape (FixedIndex / Wildcard / DynamicIndex) doesn't
                // match a bare reference, so we wrap.
                if matches!(live_shape, RefShape::Bare) {
                    if out.live_ref.is_none() {
                        out.live_ref = Some(expr.clone());
                    }
                    expr
                } else {
                    freeze_at_previous(expr, loc, in_subscript_index)
                }
            } else if other_deps.contains(&canonical) {
                freeze_at_previous(expr, loc, in_subscript_index)
            } else {
                expr
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            let canonical = Ident::new(ident.as_str());
            // The occurrence IR's classification of THIS subscript node -- the
            // single classifier family. `db::ltm_ir` decided the access shape
            // and per-axis reads on the target's `Expr2` AST; the wrap consults
            // that here by the structural path it tracks (which equals the
            // occurrence's `SiteId`, corpus-proven by
            // `assert_occurrence_stream_aligns`) rather than re-deriving on the
            // reparsed `Expr0`. A `None` means this node is not a recorded
            // causal reference (so it is neither the live source nor an
            // iterated other-dep the wrap collapses).
            let node_occ = ctx.occ.get(path);
            let node_shape = node_occ.map(|o| &o.shape);
            // Loud desync guard (finding 1): the walker records an occurrence
            // for EVERY live-source subscript head, so a `None` here on a
            // NON-empty stream means the wrap's tracked path drifted from the
            // IR's `SiteId` -- the shape lookup would silently miss, the live
            // reference would freeze, and the score would zero. Flag it; the
            // caller abandons the partial and warns rather than emitting the
            // silent zero. (A genuinely source-free slot -- an EXCEPT default
            // that never references `from`, or an agg-source generator whose
            // synthetic-agg live source is never a walked node -- leaves the
            // stream empty, so `is_empty()` excludes those legitimate misses.)
            if &canonical == live_source && node_occ.is_none() && !ctx.occ.is_empty() {
                out.missing_occurrence = true;
            }
            // GH #511: an iterated-dimension subscript on the LIVE source reads
            // the same element a bare reference would in each slot, so
            // `db::ltm_ir` classifies it `Bare` (`classify_iterated_dim_shape`,
            // all axes `Iterated`; a plain subscript is never `Bare`).
            // Normalize it to a bare `Var` -- *before* the live/PREVIOUS
            // dispatch -- so `PREVIOUS(Var)` (which codegen accepts) replaces
            // the `PREVIOUS(Subscript(...))` that trips the codegen assertion.
            //
            // Suppressed for a `PerElement` live shape: the per-element path
            // (`generate_per_element_link_equation`) emits a SCALAR equation
            // per (row, target element), so a live-source occurrence is frozen
            // at `PREVIOUS` and then row-pinned AFTER the wrap
            // (`post_transform::pin_source_subscript_indices`) -- collapsing to bare here
            // would discard the subscript that pin needs, leaving an over-arity
            // `PREVIOUS(pop)` for a positionally-mapped occurrence that fails to
            // compile and silently zeroes the score. Skip the collapse so the
            // subscript survives; the post-transform pin resolves each iterated
            // axis (mapped axes through the correspondence) to this element's
            // own coordinate (byte-identical to bare for the unmapped case).
            if &canonical == live_source {
                if !matches!(live_shape, RefShape::PerElement { .. })
                    && node_shape == Some(&RefShape::Bare)
                {
                    return wrap_non_matching_in_previous(
                        Expr0::Var(ident, loc),
                        ctx,
                        out,
                        path,
                        frozen,
                    );
                }
            } else if other_deps.contains(&canonical)
                && !matches!(live_shape, RefShape::PerElement { .. })
            {
                // The other-dep iterated-subscript verdict (`w[Age]` -> bare
                // `PREVIOUS(w)` on `Collapse`), derived from the occurrence's
                // per-axis classification via the single-sourced
                // `derive_other_dep_verdict` -- the SAME rule the edge emitter's
                // IR feeds, so the wrap and the emitter cannot disagree. Only
                // sound when the emitted partial keeps the dep bare-and-iterated
                // (the A2A / scalar paths); the `PerElement` guard above skips
                // it (that path pins every arrayed dep per element afterward, so
                // a bare collapse would let a full-target-tuple pin over-subscript
                // a subset-dims dep -- an uncompilable, silently-zeroed fragment).
                match other_dep_verdict(node_occ, canonical.as_str(), ctx.iter_ctx) {
                    OtherDepVerdict::Collapse => {
                        return wrap_non_matching_in_previous(
                            Expr0::Var(ident, loc),
                            ctx,
                            out,
                            path,
                            frozen,
                        );
                    }
                    OtherDepVerdict::Mismatch => {
                        // GH #526: collapsing would freeze the WRONG element.
                        // Record the doom and fall through to the normal
                        // wrap; the caller abandons this changed-first form
                        // (changed-last fallback, or the loud error), so the
                        // PREVIOUS(Subscript(..)) produced below is never
                        // emitted.
                        out.other_dep_mismatch = true;
                    }
                    OtherDepVerdict::NotIterated => {}
                }
            }
            if &canonical == live_source && node_shape == Some(live_shape) {
                // Live reference: the OUTER subscript stays unwrapped.
                // Decide per-index whether to recurse:
                //
                //   - A literal element SELECTOR (`db::ltm_ir` marks it
                //     `OccurrenceAxis::Pinned`, and the walk pushes no `SiteId`
                //     under it) is a runtime dimension reference; leave it
                //     verbatim so a variable/element name collision doesn't wrap
                //     it, and so the tracked path lines up with the walk.
                //
                //   - Wildcard tokens (`*`) have no inner content to wrap;
                //     recursing is a no-op.
                //
                //   - Non-literal indices (expressions like `idx + helper` in
                //     `RefShape::DynamicIndex`) are computational content;
                //     recurse so any `other_deps` referenced inside get held at
                //     PREVIOUS for ceteris-paribus.
                //
                // Without the per-index split, DynamicIndex live refs would skip
                // wrapping inner deps and the partial would no longer be
                // ceteris-paribus.
                //
                // `PerElement` row pinning takes over entirely: the occurrence's
                // axes are all `Iterated`/`Pinned` by construction (that IS the
                // shape), so there is no dynamic index to recurse into, and the
                // indices become this `(site, element)` instantiation's row --
                // spelled BARE here because `frozen` is false, i.e. the wrap is
                // leaving this occurrence live. That bare-vs-qualified choice is
                // the one thing only the wrap can decide, and it is why the
                // pinning lives here rather than in a pass over the wrapped tree.
                let indices: Vec<IndexExpr0> = match ctx.pin {
                    Some(pin_ctx) => post_transform::pin_source_subscript_indices(
                        indices,
                        node_occ,
                        pin_ctx,
                        !frozen,
                        |i, idx| {
                            wrap_index_non_matching_in_previous(
                                idx,
                                ctx,
                                out,
                                true,
                                &child_path(path, i),
                                frozen,
                                axis_dim_at(ctx, &canonical, i),
                            )
                        },
                    ),
                    None => {
                        let occ_axes = node_occ.map(|o| o.axes.as_slice()).unwrap_or(&[]);
                        indices
                            .into_iter()
                            .enumerate()
                            .map(|(i, idx)| {
                                if matches!(occ_axes.get(i), Some(OccurrenceAxis::Pinned(_))) {
                                    idx
                                } else {
                                    wrap_index_non_matching_in_previous(
                                        idx,
                                        ctx,
                                        out,
                                        false,
                                        &child_path(path, i),
                                        frozen,
                                        axis_dim_at(ctx, &canonical, i),
                                    )
                                }
                            })
                            .collect()
                    }
                };
                let subscript = Expr0::Subscript(ident, indices, loc);
                if out.live_ref.is_none() {
                    out.live_ref = Some(subscript.clone());
                }
                return subscript;
            }
            // Non-live reference: recurse into indices so any nested
            // user-variable references get wrapped, then build the new
            // subscript. If the outer ident is itself a dep, wrap the
            // whole thing -- decided FIRST, because a wrapped subscript's
            // indices are inside a frozen subtree and the `PerElement` pinning
            // spells those qualified.
            let will_wrap = &canonical == live_source || other_deps.contains(&canonical);
            let child_frozen = frozen || will_wrap;
            // `PerElement` row pinning for a FROZEN occurrence of the LIVE
            // SOURCE ITSELF: rewrite every describable axis to ITS OWN row for
            // this target element, QUALIFIED (`region·boston`), so the freeze
            // compiles to a direct LoadPrev. Qualification has to come from the
            // SOURCE's declared dims, which is why the wrap's own generic
            // `qualify_element_index` is suppressed here (`skip` below): that
            // helper only qualifies a name exactly one PROJECT dimension
            // declares, so on an AMBIGUOUS element (a name several dims declare,
            // like C-LEARN's regions) it would leave `pop[boston, age·old]`
            // half-qualified. The pinning knows the owner axis for every index
            // and qualifies all of them consistently.
            //
            // A genuinely-DYNAMIC index (`idx` in `pop[Region, idx]`) is not
            // describable per axis, so the pinning hands it back to the wrap's
            // index pass and it still gets its `PREVIOUS(idx)` lag. Suppressing
            // the whole index pass instead would silently drop that lag, changing
            // both the emitted text and the compiled score series.
            let skip_index_qualification =
                &canonical == live_source && matches!(live_shape, RefShape::PerElement { .. });
            let recurse_index = |i: usize, idx: IndexExpr0, out: &mut WrapOutcome| {
                wrap_index_non_matching_in_previous(
                    idx,
                    ctx,
                    out,
                    skip_index_qualification,
                    &child_path(path, i),
                    child_frozen,
                    axis_dim_at(ctx, &canonical, i),
                )
            };
            let indices: Vec<IndexExpr0> = match ctx.pin.filter(|_| &canonical == live_source) {
                Some(pin_ctx) => post_transform::pin_source_subscript_indices(
                    indices,
                    node_occ,
                    pin_ctx,
                    // Frozen: never the live reference, so never the bare row.
                    false,
                    |i, idx| recurse_index(i, idx, out),
                ),
                None => indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, idx)| recurse_index(i, idx, out))
                    .collect(),
            };
            let subscript = Expr0::Subscript(ident, indices, loc);
            if will_wrap {
                freeze_at_previous(subscript, loc, in_subscript_index)
            } else {
                subscript
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            // A PREVIOUS(...) / INIT(...) call from the original equation:
            // everything inside it is already lagged (read at the prior step)
            // or frozen (read at t=0), so it is already ceteris-paribus -- the
            // current-step perturbation cannot affect it. Wrapping its
            // contents again would read values from TWO steps ago
            // (semantically wrong) and force a nested-PREVIOUS helper chain
            // (one synthesized helper variable per occurrence; on
            // SAMPLE-IF-TRUE-heavy models like C-LEARN this was the dominant
            // helper source). Leave the whole call untouched.
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                // The wrap adds nothing inside, but the `PerElement` row pinning
                // still has to reach the source references in there: an already-
                // lagged read is still a read of a concrete element, and leaving
                // its dimension-name subscript in a scalar fragment either fails
                // to compile or reads the wrong element. The pin-only descent
                // does exactly that lowering with the same path cursor and the
                // same occurrence IR, and wraps nothing.
                let call = Expr0::App(UntypedBuiltinFn(name, args), loc);
                return match ctx.pin {
                    Some(pin_ctx) => post_transform::pin_only_source_refs(
                        call,
                        pin_ctx,
                        ctx.occ,
                        path,
                        &mut out.missing_occurrence,
                    ),
                    None => call,
                };
            }
            // A LOOKUP call's first argument names a graphical-function table
            // (a lookup-only variable, or the WITH-LOOKUP self-reference); the
            // table HEAD is static data the compiler resolves to a table id, not
            // a causal value reference. Wrapping it in PREVIOUS produces
            // `lookup(PREVIOUS(table), ...)`, which cannot compile (a
            // table-only variable has no value slot), so the whole link-score
            // fragment silently zeroes -- the failure mode behind WRLD3's
            // identically-zero table-mediated link scores. Hold the HEAD
            // verbatim.
            //
            // Its subscript INDEX expressions are a different thing entirely
            // (GH #984): `compiler::codegen::extract_table_info`'s
            // `Expr::Subscript` arm builds the element offset out of the live
            // index `Expr`s, so `LOOKUP(g[idx], x)` reads `idx` at the CURRENT
            // step. Held verbatim, every partial of that target varied with
            // `idx`, misattributing its movement to whichever source the partial
            // isolates -- a wrong number with no diagnostic. So the indices go
            // through the wrap's own index pass like any other other-dep.
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "lookup" | "lookup_forward" | "lookup_backward"
            ) && !args.is_empty()
            {
                // Child indices match the `Expr2` walk, which counts the skipped
                // `LookupTable` slot as child 0.
                let new_args = args
                    .into_iter()
                    .enumerate()
                    .map(|(i, a)| {
                        if i == 0 {
                            let a = freeze_lookup_table_indices(
                                a,
                                ctx,
                                out,
                                &child_path(path, i),
                                frozen,
                            );
                            // The head is still held verbatim, but a `PerElement`
                            // lowering must reach every source reference inside
                            // the argument (the head itself can BE the source):
                            // an un-pinned dimension-name subscript cannot
                            // resolve in a scalar fragment.
                            match ctx.pin {
                                Some(pin_ctx) => post_transform::pin_only_source_refs(
                                    a,
                                    pin_ctx,
                                    &OccurrenceLookup::empty(),
                                    &child_path(path, i),
                                    &mut out.missing_occurrence,
                                ),
                                None => a,
                            }
                        } else {
                            wrap_non_matching_in_previous(a, ctx, out, &child_path(path, i), frozen)
                        }
                    })
                    .collect();
                return Expr0::App(UntypedBuiltinFn(name, new_args), loc);
            }
            // GH #517: an array-reducer subexpression (`SUM(pop[*])`,
            // `MEAN(...)`, `SUM(m[D1,*])`, ...) that does not itself carry
            // the live reference is "other content" for ceteris-paribus
            // purposes. Wrap the whole reducer in `PREVIOUS` --
            // `PREVIOUS(SUM(pop[*]))`, which is `PREVIOUS` of a scalar (the
            // reducer's result, even a partial reduce, is scalar in the
            // enclosing apply-to-all context) and evaluates fine -- rather
            // than recursing into it and emitting `SUM(PREVIOUS(pop[*]))`.
            // The wrap is the GH #517 semantics -- freeze the reducer's
            // RESULT -- and is kept for that reason. Its original
            // justification is stale: the inner form used to be a stubbed
            // `0.0` at every step because codegen had no array-`PREVIOUS`
            // path, and GH #995 phase C3 gave it one (a view over
            // `prev_values`), so both forms compile now.
            // If the live reference *is* inside this reducer (the now
            // test-only `RefShape::Wildcard` path where `SUM(pop[*])` is the
            // live thing), recurse normally so the live `pop[*]` stays
            // unwrapped. Likewise (Track A stage 1, finding 2) if the reducer
            // held LIVE by `live_reducer_text` is NESTED inside this one -- a
            // hoisted `SUM(pop[*])` inside a DECLINED outer `SUM(matrix[D,*] *
            // SUM(pop[*]))` -- recurse so the inner reducer is reached and held
            // live; freezing the outer whole would drop the live reference
            // entirely (a structural zero, vs HEAD's live-agg-inside-a-frozen-
            // slice partial). The top-of-function guard has already declined to
            // hold THIS reducer live (its own text does not equal
            // `live_reducer_text`), so this only affects an enclosing reducer.
            let holds_live_reducer = live_reducer_text
                .is_some_and(|text| args.iter().any(|a| expr0_contains_reducer_text(a, text)));
            if is_array_reducer_name(&name, args.len())
                && !holds_live_reducer
                && !ctx
                    .occ
                    .subtree_has_live_shape(path, live_source, live_shape)
            {
                let reducer = Expr0::App(UntypedBuiltinFn(name, args), loc);
                // Frozen WHOLE, so the wrap never descends -- but the reducer can
                // still hold source references the `PerElement` lowering must pin
                // (an index-nested occurrence is excluded from
                // `subtree_has_live_shape`, so a reducer whose ONLY matching-shape
                // occurrence sits in a subscript index freezes whole with that
                // occurrence inside it). Pin-only descent, always qualified: it is
                // inside a freeze, so nothing in here is the live reference.
                let reducer = match ctx.pin {
                    Some(pin_ctx) => post_transform::pin_only_source_refs(
                        reducer,
                        pin_ctx,
                        ctx.occ,
                        path,
                        &mut out.missing_occurrence,
                    ),
                    None => reducer,
                };
                return freeze_at_previous(reducer, loc, in_subscript_index);
            }
            let args = args
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    wrap_non_matching_in_previous(a, ctx, out, &child_path(path, i), frozen)
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(wrap_non_matching_in_previous(
                *inner,
                ctx,
                out,
                &child_path(path, 0),
                frozen,
            )),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_non_matching_in_previous(
                *lhs,
                ctx,
                out,
                &child_path(path, 0),
                frozen,
            )),
            Box::new(wrap_non_matching_in_previous(
                *rhs,
                ctx,
                out,
                &child_path(path, 1),
                frozen,
            )),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(wrap_non_matching_in_previous(
                *cond,
                ctx,
                out,
                &child_path(path, 0),
                frozen,
            )),
            Box::new(wrap_non_matching_in_previous(
                *then_expr,
                ctx,
                out,
                &child_path(path, 1),
                frozen,
            )),
            Box::new(wrap_non_matching_in_previous(
                *else_expr,
                ctx,
                out,
                &child_path(path, 2),
                frozen,
            )),
            loc,
        ),
    }
}

/// Freeze the runtime value reads in a `LOOKUP` TABLE argument's subscript
/// indices, leaving the table HEAD (and every static element selector) exactly
/// as written (GH #984).
///
/// This is the whole of the fix, and it is one rule for every caller: the
/// wrap's own index pass ([`wrap_index_non_matching_in_previous`]) decides what
/// a table index is, exactly as it does for an ordinary subscript. Static
/// selectors -- a literal element, a dimension name, `@N`, a numeric literal --
/// hit its guards and come back untouched; a genuine value read (`idx`,
/// `idx + 1`) reaches the recursive wrap and comes back frozen.
///
/// Four deliberate choices:
///
/// - only an `Expr0::Subscript` has indices to freeze. A bare `Var` table is
///   already static, and anything else is `BadTable` at codegen (the table
///   argument must select exactly one element), so there is nothing here to
///   decide about it;
/// - the descent's dep set is WIDENED with the idents appearing in these
///   indices, and without that the freeze does not fire at all. The wrap freezes
///   an ident only when it is in `other_deps`, and `other_deps` comes from
///   `variable::classify_dependencies`, whose `BuiltinContents::LookupTable` arm
///   records the table's ident and **never walks the table expression** -- so an
///   index variable referenced ONLY inside a table argument (the issue's own
///   `LOOKUP(g[idx], x)`) is not a dependency and was returned live. Widening is
///   scoped to this argument's own indices, so nothing else in the equation sees
///   the added names, and the element / dimension-name guards in
///   [`wrap_index_non_matching_in_previous`] run BEFORE the `other_deps` check --
///   so adding an element or dimension name to the set cannot make it wrap. (The
///   dep set itself is left alone: dropping a table index from a variable's
///   dependencies is arguably an engine bug in its own right, but it is a
///   runlist-ordering question, not this rule's, and fixing it there is a
///   separate change);
/// - element QUALIFICATION is suppressed (`skip_element_qualification`). A
///   literal element index is static either way, and leaving it verbatim keeps
///   this change to the reads it is about: on the `PerElement` path the row
///   pinning already qualifies it from the source's own declared dims (which is
///   the only qualification that is right for an ambiguous element name), and on
///   the other paths the target's own equation carries the same spelling;
/// - inside an enclosing FREEZE nothing is done. `frozen` here means the whole
///   `LOOKUP` sits in a subtree the wrap is about to lag (a wrapped other-dep's
///   subscript index is the only way to get here -- a pre-existing
///   `PREVIOUS`/`INIT` and a whole-frozen reducer never reach this arm at all),
///   so the index read is already ceteris-paribus and a second `PREVIOUS` would
///   read two steps back.
///
/// Because this discharges the index everywhere the arm runs -- for ANY index
/// ident, not only one that happens to be a dependency elsewhere -- and the
/// enclosing freeze discharges it everywhere the arm does not,
/// `post_transform::pin_dimension_name_indices` no longer has to REFUSE a table
/// argument carrying a runtime index -- it keeps it and says nothing. That
/// refusal, its `frozen` plumbing, and the warned skip it produced are deleted.
fn freeze_lookup_table_indices(
    arg: Expr0,
    ctx: &WrapCtx<'_>,
    out: &mut WrapOutcome,
    path: &[u16],
    frozen: bool,
) -> Expr0 {
    let Expr0::Subscript(ident, indices, loc) = arg else {
        return arg;
    };
    if frozen {
        return Expr0::Subscript(ident, indices, loc);
    }
    // Every ident under these indices is a read the wrap must be able to freeze,
    // whether or not the dependency extractor knows about it (see the rustdoc).
    let mut index_deps: HashSet<Ident<Canonical>> = ctx.other_deps.clone();
    for idx in &indices {
        let mut add = |e: &Expr0| {
            index_deps.extend(expr_reference_idents(e).into_iter().map(|n| Ident::new(&n)));
        };
        match idx {
            IndexExpr0::Expr(e) => add(e),
            IndexExpr0::Range(l, r, _) => {
                add(l);
                add(r);
            }
            // Wildcard / star-range / `@N` carry no `Expr0`.
            _ => {}
        }
    }
    // The IR records no occurrence anywhere under a table argument, so the
    // descent uses an empty lookup rather than this slot's -- see
    // [`OccurrenceLookup::empty`] for why a real one would be unsound.
    let table_ctx = WrapCtx {
        occ: &OccurrenceLookup::empty(),
        other_deps: &index_deps,
        ..*ctx
    };
    let indices = indices
        .into_iter()
        .enumerate()
        .map(|(i, idx)| {
            wrap_index_non_matching_in_previous(
                idx,
                &table_ctx,
                out,
                true,
                &child_path(path, i),
                frozen,
                // A literal `None`, and a KNOWN GAP: a table holder is by
                // construction absent from `dep_dims` (GH #606), so an
                // `axis_dim_at` call here could only ever return `None` and
                // would read as coverage for a path that is not fixed -- GH #984
                // still reproduces here. See `index_axis_verdict`.
                None,
            )
        })
        .collect();
    Expr0::Subscript(ident, indices, loc)
}

#[path = "ltm_augment_partial_error.rs"]
mod partial_error;
use partial_error::contains_rank_like_builtin;
pub(crate) use partial_error::{PartialEquationError, PartialEquationErrorKind};

/// Build a partial equation for a per-shape link score.
///
/// Parses `equation_text`, computes the set of "other" deps (everything
/// in `deps` other than `live_source`, also dropping module-prefixed
/// references that normalize to `live_source`), and then walks the AST
/// wrapping every reference to those other deps in `PREVIOUS()`. The
/// reference to `live_source` is left live only at occurrences whose
/// shape matches `live_shape`; other occurrences of `live_source` are
/// wrapped too.
///
/// The function always canonicalizes via parse + `print_eqn`, even when
/// no wrapping happens, so the result is always in the canonical equation
/// format expected by downstream parsing. The performance impact is
/// negligible because LTM equations are short.
///
/// Returns `Err([`PartialEquationError`])` when `equation_text` does not
/// parse (genuine parse error, or an empty/whitespace equation). A
/// successfully-parsed equation that simply has no `other_deps` to wrap is
/// NOT a failure -- it is its own ceteris-paribus partial (e.g. a constant)
/// and returns `Ok` with the re-printed text unchanged.
///
/// `iter_ctx` is the GH #511 iterated-dimension context (the target's
/// iterated dims + the source's declared dim names + a `DimensionsContext`);
/// pass `None` when the live source is scalar (no source subscripts to
/// recognize). See [`wrap_non_matching_in_previous`] and [`IteratedDimCtx`].
///
/// Test-only since Track A stage 1: production callers now drive the wrap
/// through [`wrap_changed_first_ast`] directly (they need the transformed AST
/// for the post-transform row-pinning / agg-substitution lowerings, not just
/// printed text). This thin text-level wrapper is retained purely as the
/// unit-tested entry point for the per-shape wrap behavior.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_partial_equation_shaped(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Result<String, PartialEquationError> {
    build_partial_equation_shaped_with_live_ref(
        equation_text,
        deps,
        live_source,
        live_shape,
        source_dim_elements,
        iter_ctx,
        dims_ctx,
    )
    .map(|(text, _live_ref)| text)
}

/// Like `build_partial_equation_shaped`, but also returns the *live
/// source reference* the partial isolates: the single occurrence of
/// `live_source` that the PREVIOUS-wrapping transform left un-wrapped,
/// with any inner index sub-expressions already PREVIOUS-rewritten.
///
/// For a `Bare` shape this is a bare `Var(live_source)`; for `FixedIndex`,
/// `Wildcard`, or `DynamicIndex` it is the index-transformed
/// `Subscript(live_source, ...)` -- i.e. `arr[PREVIOUS(idx)]`, `pop[NYC,*]`,
/// etc. Callers that build a source-side normalizer (`source - PREVIOUS(source)`
/// in `link_score_guard_form`) need this so they can scalarize a `Wildcard` /
/// `DynamicIndex` source slice (`SUM(arr[PREVIOUS(idx)])`) instead of spelling
/// the bare arrayed name (which is a dimension error in a scalar link-score
/// equation, yielding an uncompilable fragment and an identically-zero score).
///
/// Returns `None` for the second element when the parsed equation contains
/// no left-live `live_source` occurrence at all.
///
/// Returns `Err([`PartialEquationError`])` when `equation_text` fails to
/// parse -- see `build_partial_equation_shaped` for why this is a loud
/// error rather than a silent lowercased-input fallback (GH #311) -- or
/// when the changed-first wrap hit a GH #526 mismatched other-dep
/// subscript (`WrapOutcome::other_dep_mismatch`): the collapse would have
/// frozen the WRONG element, so the partial fails with the loud
/// `UnfreezablePartial` instead of a silent magnitude error. (Callers that
/// can fall back to the changed-last convention route through
/// [`shaped_guard_form_text`], which does so for this doom class.)
///
/// Test-only since Track A stage 1 (see `build_partial_equation_shaped`).
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_partial_equation_shaped_with_live_ref(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Result<(String, Option<Expr0>), PartialEquationError> {
    // This is a genuine TEXT entry point (the unit tests spell the target
    // equation as a string), so the parse happens here, once, rather than inside
    // the transform: production hands `wrap_changed_first_ast` an `Expr0` lowered
    // straight from the target's `Expr2`.
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
    };
    // Reconstruct the occurrence IR the production wrap consumes (the production
    // callers get it from `model_ltm_reference_sites`; this text-level test entry
    // rebuilds an equivalent stream on the parsed Expr0, using the `#[cfg(test)]`
    // Expr0 classifiers the alignment gate proves stay in step with the IR).
    // Slot-0 body, matching `wrap_changed_first_ast`.
    let occurrences = build_wrap_test_occurrences(
        &ast,
        live_source,
        deps,
        source_dim_elements,
        iter_ctx,
        dims_ctx,
    );
    let slot_occurrences = SlotOccurrences::new(&occurrences);
    let occ = slot_occurrences.for_slot(0);
    let (transformed, out) = wrap_changed_first_ast(
        &ast,
        deps,
        live_source,
        live_shape,
        None,
        iter_ctx,
        dims_ctx,
        &occ,
        None,
    );
    if out.other_dep_mismatch || out.missing_occurrence {
        return Err(PartialEquationError::unfreezable(equation_text));
    }
    Ok((print_eqn(&transformed), out.live_ref))
}

#[cfg(test)]
#[path = "ltm_augment_wrap_test_support.rs"]
mod wrap_test_support;
#[cfg(test)]
pub(crate) use wrap_test_support::{
    build_wrap_test_occurrences, classify_expr0_subscript_shape, is_literal_element_index,
    live_source_occurrence_axis, resolve_literal_element_index, test_occurrences_for_var,
};

/// The shared changed-first transform: filter `deps` down to the
/// other-deps set and PREVIOUS-wrap `target_expr` via
/// [`wrap_non_matching_in_previous`] -- returning the transformed AST (not
/// printed text) plus the [`WrapOutcome`] out-channels (the captured live
/// reference and the GH #526 mismatch doom flag). The single
/// implementation behind both `build_partial_equation_shaped_with_live_ref`
/// (which prints it) and [`shaped_guard_form_text`] (which doom-checks the
/// AST before printing), so the two can never drift on dep filtering or the
/// wrap itself.
///
/// `target_expr` is the target equation's own AST, lowered from its `Expr2`
/// by [`crate::patch::expr2_to_expr0`] -- **not** parsed from printed text.
/// Track A's generation half deleted that round trip: `patch::expr2_to_string`
/// IS `print_eqn(expr2_to_expr0(..))`, so parsing its output back was a parse of
/// our own print of the very tree we already had in this shape. Two things
/// follow, and the second is the load-bearing one:
///
/// 1. There is no parse to fail here, so this is infallible (the caller's
///    `PartialEquationError::Parse` channel now fires only where a genuine
///    source-format text boundary remains -- see
///    [`scalar_or_a2a_target_expr`]).
/// 2. The tree the wrap walks is structurally IDENTICAL to the `Expr2` tree
///    `db::ltm_ir::walk_all_in_expr` computed the occurrence `SiteId`s on, so
///    the wrap's tracked child-index path equals the occurrence's `SiteId` **by
///    construction** rather than by a corpus-proven print/reparse isomorphism.
///    The property that makes the swap byte-neutral is pinned by
///    `classifier_agreement_tests::assert_lowering_matches_reparse`.
///
/// `live_reducer_text` (Track A stage 1) opts the wrap into holding a
/// designated hoisted reducer subexpression LIVE verbatim (matched by
/// canonical text) instead of an ident -- see [`WrapCtx::live_reducer_text`].
/// `None` is the ordinary ident-live behavior; every non-agg caller passes it.
#[allow(clippy::too_many_arguments)]
fn wrap_changed_first_ast(
    target_expr: &Expr0,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    live_reducer_text: Option<&str>,
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
    occ: &OccurrenceLookup<'_>,
    pin: Option<&PerElementRefCtx<'_>>,
) -> (Expr0, WrapOutcome) {
    let other_deps: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| *d != live_source && normalize_module_ref(d) != *live_source)
        .cloned()
        .collect();

    let ast = target_expr.clone();

    let ctx = WrapCtx {
        live_source,
        live_shape,
        live_reducer_text,
        other_deps: &other_deps,
        iter_ctx,
        dims_ctx,
        occ,
        // The walk starts at the slot's root expression, never inside an index.
        in_subscript_index: false,
        pin,
    };
    let mut out = WrapOutcome::default();
    // The wrap walks the slot's expression from its root; the occurrence
    // lookup was already rebased to slot-local paths, so the root path is
    // empty.
    let transformed = wrap_non_matching_in_previous(ast, &ctx, &mut out, &[], false);
    (transformed, out)
}

/// Is `expr` *array-slice-valued* -- does it contain a wildcard/star-range
/// subscript axis that no enclosing array reducer collapses? Such an
/// expression evaluates to an array view, not a scalar.
///
/// Used by [`contains_unfreezable_previous`] to decide whether a `PREVIOUS`
/// argument is one this layer will spell INLINE. A reducer application
/// (`SUM(matrix[d1,*])`) collapses the slice to a scalar, so a wildcard
/// *inside* a reducer is fine (`PREVIOUS(SUM(arr[*]))` is the deliberate
/// GH #517 whole-reducer freeze); a slice that no reducer collapses is routed
/// to the materialized freeze helper ([`crate::ltm_augment_array_freeze`],
/// GH #1003) or declined.
///
/// The original reason -- "`PREVIOUS` of an array view has no codegen path" --
/// is stale as of GH #995 phase C3, which gave it one (a view over
/// `prev_values`). The routing is unchanged because the helper buys something
/// the inline spelling does not: its arms are qualified with the AXIS
/// dimension, so a named subdimension that is not a positional prefix of its
/// parent still reads the name-correct row (PR #1001). Retiring the helper in
/// favour of the inline form is an open simplification, and would have to
/// carry that guarantee.
fn expr_is_array_slice_valued(expr: &Expr0) -> bool {
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => false,
        Expr0::Subscript(_, indices, _) => indices
            .iter()
            .any(|idx| matches!(idx, IndexExpr0::Wildcard(_) | IndexExpr0::StarRange(_, _))),
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            // A scalar-collapsing reducer's result is scalar regardless of
            // slices inside it. RANK is in the reducer table but is
            // ARRAY-valued (GH #742), so it is transparent here: a slice in
            // its argument stays uncollapsed (`PREVIOUS(rank(matrix[d1,*],1))`
            // is unfreezable -- the slice-bearing capture lands in a scalar
            // helper, where `rank(...)` is ill-typed), while a bare-name
            // argument (`PREVIOUS(rank(pop, 1))`) stays freezable because
            // `make_temp_arg` captures it into an ARRAYED helper (the GH #541
            // path, extended to array-valued builtins by the same GH #742
            // predicate in `arg_has_bare_var_ref`).
            if crate::ltm_agg::reducer_collapses_to_scalar(&name.to_ascii_lowercase(), args.len()) {
                false
            } else {
                args.iter().any(expr_is_array_slice_valued)
            }
        }
        Expr0::Op1(_, inner, _) => expr_is_array_slice_valued(inner),
        Expr0::Op2(_, l, r, _) => expr_is_array_slice_valued(l) || expr_is_array_slice_valued(r),
        Expr0::If(c, t, e, _) => {
            expr_is_array_slice_valued(c)
                || expr_is_array_slice_valued(t)
                || expr_is_array_slice_valued(e)
        }
    }
}

/// Does the (already PREVIOUS-wrapped) partial contain a `PREVIOUS(...)`
/// call whose argument is array-slice-valued (see
/// [`expr_is_array_slice_valued`])?
///
/// GH #743's original reading was that such a partial can never evaluate
/// correctly, because `PREVIOUS` of an array view had no codegen path: as a
/// *user* equation it was a hard `NotSimulatable` compile error, and as an LTM
/// link-score fragment the doomed `PREVIOUS` was routed through a synthesized
/// implicit helper (`$⁚$⁚ltm⁚…⁚arg0`) whose fragment failed to compile SILENTLY
/// -- it kept a layout slot with no bytecode and read a constant 0 -- so the
/// partial silently lost the frozen term while the outer score still compiled,
/// producing plausible-looking garbage (the constant `-1/growth-rate` scores of
/// GH #743).
///
/// BOTH halves of that premise have moved and the routing is kept on other
/// grounds. GH #1003 materializes most of these as a `$⁚ltm⁚freeze⁚…` helper
/// ([`crate::ltm_augment_array_freeze`]) whose arms are qualified against the
/// AXIS dimension, and GH #995 phase C3 gave the inline spelling a path of its
/// own (a view over `prev_values`). What this predicate still routes is the
/// residue neither reaches -- a slice this layer will not spell inline and no
/// helper can materialize. The partial-equation builders therefore treat this
/// shape as a routing decision: fall back to the changed-last attribution, or
/// fail loudly.
fn contains_unfreezable_previous(expr: &Expr0) -> bool {
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => false,
        Expr0::Subscript(_, indices, _) => indices.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => contains_unfreezable_previous(e),
            IndexExpr0::Range(l, r, _) => {
                contains_unfreezable_previous(l) || contains_unfreezable_previous(r)
            }
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => false,
        }),
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            if name.eq_ignore_ascii_case("previous")
                && args.first().is_some_and(expr_is_array_slice_valued)
            {
                return true;
            }
            args.iter().any(contains_unfreezable_previous)
        }
        Expr0::Op1(_, inner, _) => contains_unfreezable_previous(inner),
        Expr0::Op2(_, l, r, _) => {
            contains_unfreezable_previous(l) || contains_unfreezable_previous(r)
        }
        Expr0::If(c, t, e, _) => {
            contains_unfreezable_previous(c)
                || contains_unfreezable_previous(t)
                || contains_unfreezable_previous(e)
        }
    }
}

/// Does `expr` reference `source` as a BARE `Var` (NOT subscripted) nested
/// inside the argument of an array-reducing builtin (`SUM`, `MEAN`, `MIN`,
/// `MAX`, `STDDEV`)? This is the GH #779 silent-wrong-number shape: a bare
/// reference to an ARRAYED variable inside an UN-HOISTED reducer.
///
/// The bare spelling's EXECUTION semantics are themselves anomalous
/// (GH #789): an asymmetric probe of `growth[D1] = SUM(matrix[D1,*] * frac)`
/// shows the engine computes `growth[r] = |D1| * Σ_d2 matrix[r,d2] * frac[r]`
/// -- a spurious `|D1|` factor, NOT a clean per-slot iteration of the bare
/// `frac`. The changed-last partial the GH #743 chooser would build --
/// `sum(matrix[d1,*] * PREVIOUS(source))`, compiled per target slot --
/// provably disagrees with whatever execution computes for the bare
/// spelling (a sustained ~3x link/loop-score error for SUM in the canonical
/// symmetric repro), so the score is silently wrong. The SUBSCRIPTED feeder
/// spelling (`source[D1]`) is hoisted into an aggregate node and scored
/// correctly (GH #767/T5); the read-slice vocabulary cannot express a BARE
/// reducer-feeder read, so the reducer stays un-hoisted and the changed-last
/// fallback is reached -- where this shape must be DECLINED loudly
/// (GH #779), not given the silent wrong number.
///
/// The walk is reducer-context-aware (`in_reducer`): only a bare `source`
/// occurrence WITHIN a recognized reducer's argument matters. A bare arrayed
/// `source` OUTSIDE any reducer (`growth[D1] = source * 2`) is the
/// bread-and-butter `Bare` A2A case -- its changed-FIRST partial keeps the
/// reference live and compiles, so it never reaches the changed-last leg and
/// must not be touched. References already inside a `PREVIOUS(...)`/`INIT(...)`
/// call are skipped (already lagged/frozen, not a live read this partial
/// must account for). The reducer set comes from
/// [`crate::ltm_agg::reducer_collapses_to_scalar`], so it also includes SIZE
/// -- harmless: an equation whose only reducer is SIZE keeps the changed-first
/// convention (the whole reducer is freezable as `PREVIOUS(size(...))`,
/// because [`expr_is_array_slice_valued`] reads the SAME predicate), so it
/// does not reach this gate. RANK is excluded by that predicate: it is
/// array-valued and uses its own agg-routing path (GH #771/#776), so its
/// bare arg is not this scalar-reducer feeder shape.
///
/// This is deliberately NOT `db::ltm_ir::OccurrenceSite::in_reducer`, which
/// looks like the same "is this reference inside a reducer?" question but is
/// the LTM ROUTING one ("did an aggregate node get minted for this call?") and
/// therefore inverts on exactly SIZE and RANK. Consuming the IR bit here would
/// flip a bare arrayed source inside `RANK(...)` from scored (via the GH #742
/// arrayed-capture path) to loudly declined -- a user-visible score change
/// with no argument behind it. Assessed in GH #982 and left as two predicates;
/// `ltm_agg::reducer_collapses_to_scalar`'s doc carries the comparison and
/// `ltm_agg::REDUCER_DECISION_TABLE` pins both of them row by row, so neither
/// can drift.
fn references_bare_source_inside_reducer(
    expr: &Expr0,
    source: &Ident<Canonical>,
    in_reducer: bool,
) -> bool {
    match expr {
        Expr0::Const(..) => false,
        Expr0::Var(ident, _) => in_reducer && &Ident::<Canonical>::new(ident.as_str()) == source,
        // A subscripted reference is NOT the bare feeder shape -- it is
        // either the hoisted feeder spelling (`source[D1]`) or an explicit
        // per-element read, both handled by their own paths. Recurse into
        // index expressions only to catch a bare `source` used as an index
        // (defensive; not a reachable feeder shape today).
        Expr0::Subscript(_, indices, _) => indices.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => references_bare_source_inside_reducer(e, source, in_reducer),
            IndexExpr0::Range(l, r, _) => {
                references_bare_source_inside_reducer(l, source, in_reducer)
                    || references_bare_source_inside_reducer(r, source, in_reducer)
            }
            IndexExpr0::Wildcard(_)
            | IndexExpr0::StarRange(_, _)
            | IndexExpr0::DimPosition(_, _) => false,
        }),
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            // Contents of PREVIOUS/INIT are already lagged; a bare source
            // there is not a live read this partial must account for.
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                return false;
            }
            // A scalar-collapsing array reducer sets the in-reducer marker
            // for its argument; nested reducers keep it set.
            let child_in_reducer = in_reducer
                || crate::ltm_agg::reducer_collapses_to_scalar(
                    &name.to_ascii_lowercase(),
                    args.len(),
                );
            args.iter()
                .any(|a| references_bare_source_inside_reducer(a, source, child_in_reducer))
        }
        Expr0::Op1(_, inner, _) => references_bare_source_inside_reducer(inner, source, in_reducer),
        Expr0::Op2(_, l, r, _) => {
            references_bare_source_inside_reducer(l, source, in_reducer)
                || references_bare_source_inside_reducer(r, source, in_reducer)
        }
        Expr0::If(c, t, e, _) => {
            references_bare_source_inside_reducer(c, source, in_reducer)
                || references_bare_source_inside_reducer(t, source, in_reducer)
                || references_bare_source_inside_reducer(e, source, in_reducer)
        }
    }
}

/// Freeze ONLY the matching-shape occurrences of `live_source` at
/// `PREVIOUS`, leaving every other reference current -- the "changed-last"
/// attribution dual of [`wrap_non_matching_in_previous`] (cf.
/// [`generate_scalar_feeder_to_agg_equation`], which established the
/// convention for scalar feeders of hoisted reducers).
///
/// `frozen_ref` records the first matching occurrence (pre-wrap, in
/// document order) so the caller can build the source-side normalizer; a
/// live-source iterated-dim subscript (`frac[D1]` under an A2A-over-`D1`
/// target) is normalized to a bare `Var` before wrapping -- `PREVIOUS(frac)`
/// compiles per-element (GH #541), while `PREVIOUS(frac[D1])` trips the
/// codegen assertion (the same GH #511 normalization
/// `wrap_non_matching_in_previous` applies).
///
/// References already inside a `PREVIOUS(...)`/`INIT(...)` call are left
/// untouched (already lagged/frozen; double-wrapping would read two steps
/// back). Non-matching occurrences of `live_source` -- and all other
/// references -- stay current: their influence is attributed by their own
/// link-score variables.
///
/// Boundary: unlike its changed-first dual, this walker never recurses
/// into subscript INDEX expressions -- a `live_source` occurrence in an
/// index position of another reference (`other_arr[live_source]`) stays
/// live (current) in the changed-last partial. That matches the dual's
/// convention that an index-nested occurrence is never the captured live
/// ref, but means such an occurrence is not frozen here either; no
/// reachable shape exercises this today (the fallback only fires when the
/// changed-first partial is unfreezable, which requires the live ref
/// inside a reducer next to a sliced co-source).
fn wrap_live_shaped_in_previous(
    expr: Expr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    frozen_ref: &mut Option<Expr0>,
    occ: &OccurrenceLookup<'_>,
    path: &[u16],
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) == live_source
                && matches!(live_shape, RefShape::Bare)
            {
                if frozen_ref.is_none() {
                    *frozen_ref = Some(expr.clone());
                }
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) == live_source {
                let node_shape = occ.get(path).map(|o| &o.shape);
                // GH #511 normalization: an iterated-dim subscript reads the
                // same element a bare reference would in each slot, and
                // `db::ltm_ir` classifies such a site `Bare` (all axes
                // `Iterated`; a plain subscript is never `Bare`). This walker
                // does not descend into subscript indices, so the head is the
                // only occurrence looked up at `path`.
                if matches!(live_shape, RefShape::Bare) && node_shape == Some(&RefShape::Bare) {
                    let bare = Expr0::Var(ident, loc);
                    if frozen_ref.is_none() {
                        *frozen_ref = Some(bare.clone());
                    }
                    return Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![bare]), loc);
                }
                if node_shape == Some(live_shape) {
                    let subscript = Expr0::Subscript(ident, indices, loc);
                    if frozen_ref.is_none() {
                        *frozen_ref = Some(subscript.clone());
                    }
                    return Expr0::App(
                        UntypedBuiltinFn("PREVIOUS".to_string(), vec![subscript]),
                        loc,
                    );
                }
                return Expr0::Subscript(ident, indices, loc);
            }
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            // Contents of PREVIOUS/INIT are already lagged/frozen.
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                return Expr0::App(UntypedBuiltinFn(name, args), loc);
            }
            let args = args
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    wrap_live_shaped_in_previous(
                        a,
                        live_source,
                        live_shape,
                        frozen_ref,
                        occ,
                        &child_path(path, i),
                    )
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(wrap_live_shaped_in_previous(
                *inner,
                live_source,
                live_shape,
                frozen_ref,
                occ,
                &child_path(path, 0),
            )),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_live_shaped_in_previous(
                *lhs,
                live_source,
                live_shape,
                frozen_ref,
                occ,
                &child_path(path, 0),
            )),
            Box::new(wrap_live_shaped_in_previous(
                *rhs,
                live_source,
                live_shape,
                frozen_ref,
                occ,
                &child_path(path, 1),
            )),
            loc,
        ),
        Expr0::If(c, t, e, loc) => Expr0::If(
            Box::new(wrap_live_shaped_in_previous(
                *c,
                live_source,
                live_shape,
                frozen_ref,
                occ,
                &child_path(path, 0),
            )),
            Box::new(wrap_live_shaped_in_previous(
                *t,
                live_source,
                live_shape,
                frozen_ref,
                occ,
                &child_path(path, 1),
            )),
            Box::new(wrap_live_shaped_in_previous(
                *e,
                live_source,
                live_shape,
                frozen_ref,
                occ,
                &child_path(path, 2),
            )),
            loc,
        ),
    }
}

/// Build the guard-form link-score text for one target equation, choosing
/// the ceteris-paribus attribution convention (GH #743):
///
/// 1. **Changed-first** (the default; byte-identical to the historical
///    output): hold the matching-shape `from` occurrences live and freeze
///    everything else at `PREVIOUS`, numerator
///    `(partial - PREVIOUS(target))`.
/// 2. **Changed-last**, when the changed-first partial would embed
///    `PREVIOUS` of an array slice (see [`contains_unfreezable_previous`])
///    or hit a GH #526 mismatched other-dep iterated subscript
///    ([`WrapOutcome::other_dep_mismatch`] -- collapsing would freeze the
///    WRONG element; changed-last instead keeps the transposed dep LIVE
///    and verbatim, so it compiles exactly like the target's own equation
///    and the numerator attributes only the live source's change):
///    freeze ONLY the matching `from` occurrences and keep everything else
///    current, numerator `(target - frozen)`. This is the
///    [`generate_scalar_feeder_to_agg_equation`] convention -- a
///    first-order-equal discrete attribution of `Δz` to `Δx` (see that
///    function's rustdoc and the convention note in
///    `docs/reference/ltm--loops-that-matter.md`) -- and is what makes an
///    UN-HOISTED feeder-bearing reducer genuinely scoreable: the wildcard
///    co-source slice stays verbatim (compiling exactly like the target's
///    own equation) and only the feeder is lagged. (The original GH #743
///    fixture, `growth[D1] = SUM(matrix[D1,*] * frac[D1])`, is HOISTED
///    since the GH #767 / T5 feeder clause and takes the per-`(row, slot)`
///    [`generate_iterated_feeder_to_agg_equation`] form instead; this
///    chooser still serves the shapes the I1 acceptance declines, e.g. the
///    Pinned-axis mix `SUM(matrix[D1,*] * w[D1, c1])`.)
/// 3. `Err(UnfreezablePartial)` when both conventions are doomed (or
///    changed-last has no matching occurrence to freeze, which would
///    silently score a constant 0): the caller skips the score variable
///    and surfaces a `Warning` -- loud degradation, never the
///    silently-stubbed-helper garbage the pre-fix path produced.
///
/// `gf_table_ref` is the implicit WITH-LOOKUP wrap (GH #910): when the
/// target is a value-bearing tables-carrying variable, its compiled value
/// is `LOOKUP(self_gf, input)` while `equation_text` is only the RAW
/// input, so both the changed-first partial and the changed-last frozen
/// evaluation are fed through `LOOKUP({gf_table_ref}, ...)` to keep the
/// numerator commensurable with the (gf-output-units) target deltas. The
/// reference is a *layout* reference (`classify_dependencies` records it
/// in `referenced_tables`, not `all`), so it adds no causal edge. `None`
/// leaves the partial unwrapped (an ordinary target).
///
/// `zero_slot_policy` decides what happens when the changed-first wrap froze
/// EVERY occurrence of the source ([`WrapOutcome::live_ref`] is `None`), so
/// the partial is the fully-frozen target and the guard form it would build
/// evaluates to ~0. Under [`ZeroSlotPolicy::OmitStructuralZero`] the arm is
/// dropped (`Ok(None)`) instead of materialized; see that variant's docs.
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn shaped_guard_form_text(
    target_expr: &Expr0,
    deps: &HashSet<Ident<Canonical>>,
    from: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
    target_ref: &str,
    gf_table_ref: Option<&str>,
    occ: &OccurrenceLookup<'_>,
    zero_slot_policy: ZeroSlotPolicy,
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<Option<String>, PartialEquationError> {
    let gf_wrap = |partial: String| -> String {
        match gf_table_ref {
            Some(table_ref) => format!("LOOKUP({table_ref}, {partial})"),
            None => partial,
        }
    };
    // The GH #995 array-freeze materializer: rewrite each `PREVIOUS(<slice>)`
    // this leg's wrap produced into a freeze-helper reference, collecting the
    // helpers into a LEG-LOCAL vec -- only the leg actually emitted extends
    // the caller's collector, so an abandoned leg mints no orphan variables.
    // Needs both the dep -> declared-dims table (to name a bare `*` axis and
    // qualify each arm against the AXIS dimension) and the dimensions
    // context; absent either, the expression is returned untouched and the
    // pre-existing doom checks decide.
    //
    // The table is AUGMENTED (on a local clone -- `iter_ctx.dep_dims` feeds
    // the GH #526 verdict and must keep its arrayed-only contract) with the
    // remaining deps as empty-dims entries, so a frozen SCALAR reference in a
    // view-position argument can materialize as a scalar helper. A dep whose
    // arrayedness the table cannot see (an arrayed implicit var) is thereby
    // mislabeled scalar and its helper fails to compile LOUDLY -- the same
    // failure mode the un-materialized freeze had.
    let dep_dims_for_freeze = iter_ctx.and_then(|c| c.dep_dims).map(|dd| {
        let mut m = dd.clone();
        for d in deps {
            m.entry(d.as_str().to_string()).or_default();
        }
        m
    });
    let materialize = |expr: Expr0, helpers: &mut Vec<ArrayFreezeHelper>| -> Expr0 {
        match (dep_dims_for_freeze.as_ref(), dims_ctx) {
            (Some(dd), Some(dc)) => materialize_array_freezes(expr, dd, dc, helpers),
            _ => expr,
        }
    };
    // The diagnostic text a loud skip names. Printed only on a failure path:
    // the transform itself consumes the AST, so the source spelling is needed
    // solely to make the warning name the offending equation.
    let err_text = || print_eqn(target_expr);
    let (changed_first, out) = wrap_changed_first_ast(
        target_expr,
        deps,
        from,
        shape,
        None,
        iter_ctx,
        dims_ctx,
        occ,
        None,
    );
    // A walker desync (finding 1): the changed-first partial silently froze the
    // live reference, and the changed-last dual would read the SAME desynced
    // occurrence stream, so BOTH conventions are corrupt. Skip loudly rather
    // than emit either silent zero.
    if out.missing_occurrence {
        return Err(PartialEquationError::unfreezable(&err_text()));
    }
    // Materialize BEFORE the doom check: a `PREVIOUS(<slice>)` the helper can
    // express is no longer a doom. What the materializer declines stays in
    // the tree verbatim, so `contains_unfreezable_previous` still catches it.
    let mut first_leg_helpers = Vec::new();
    let changed_first = materialize(changed_first, &mut first_leg_helpers);
    if !out.other_dep_mismatch && !contains_unfreezable_previous(&changed_first) {
        // GH #977: the wrap produced a partial that reads nothing which can have
        // changed since the previous step, so it recomputes `PREVIOUS(target)`
        // and the guard form's numerator is identically zero. Drop the arm
        // rather than print, parse, lower and execute a full equation to arrive
        // at the constant an absent slot already lowers to.
        //
        // The test runs on the MATERIALIZED partial, after the array-freeze
        // rewrite, so it judges the tree that would actually be emitted rather
        // than the one before helper substitution.
        //
        // The check sits INSIDE the changed-first success block, not before it:
        // an arm that also trips the doom checks must keep falling through to
        // the changed-last leg, which rejects it with `Err(UnfreezablePartial)`
        // and so declares the whole edge unscoreable (the #758/#780 contract,
        // which drops dependent loop scores). Omitting it earlier would quietly
        // keep that edge scoreable and change which loops get dropped.
        //
        // `first_leg_helpers` is deliberately NOT appended: the arm that would
        // have referenced those freeze helpers is gone, so appending them would
        // mint variables no equation reads.
        if zero_slot_policy == ZeroSlotPolicy::OmitStructuralZero
            && partial_is_provably_previous_target(&changed_first)
        {
            return Ok(None);
        }
        let source_ref = source_ref_for_guard(
            from,
            shape,
            out.live_ref.as_ref(),
            source_dim_names,
            source_dim_elements,
        );
        freeze_helpers.append(&mut first_leg_helpers);
        return Ok(Some(link_score_guard_form(
            &gf_wrap(print_eqn(&changed_first)),
            target_ref,
            &source_ref,
        )));
    }

    // Changed-last fallback: freeze only the live source, starting from the
    // SAME pristine target AST the changed-first leg wrapped. This used to
    // re-parse the equation text here -- justified at the time as a cheap second
    // parse on a rare doomed path, which was true of the cost but not of the
    // structure: the "cheap re-parse" was reconstructing a tree the caller
    // already owned, and it was the only reason this function needed the text at
    // all. Taking the AST as the parameter removes both the parse and the
    // possibility that the two conventions ever walk different trees.
    let ast = target_expr.clone();

    // GH #779: decline the BARE-spelled feeder of an un-hoisted multi-source
    // reducer. When the live source is referenced BARE (unsubscripted) and is
    // ARRAYED (it has declared dimensions), the spelling's own execution
    // semantics are anomalous (GH #789: the engine computes a spurious
    // iterated-dim-cardinality factor, not a clean per-slot read of the bare
    // reference) -- and the changed-last partial below, which freezes the
    // bare reference and compiles per target slot, provably disagrees with
    // whatever execution computes, producing a SILENT wrong score (a
    // sustained ~3x error for SUM in the canonical repro). The subscripted
    // spelling `source[D1]` is hoisted and scored correctly (GH #767/T5);
    // the bare spelling cannot be expressed by the read-slice vocabulary, so
    // it must be declined LOUDLY (the GH #780 `Unscoreable` plumbing records
    // the edge and drops dependent loop scores) rather than scored wrong
    // silently. This is the only point the shape is reachable: a bare
    // arrayed source OUTSIDE a reducer keeps the live reference in its
    // changed-FIRST partial (which compiles), so it never reaches this
    // fallback.
    if matches!(shape, RefShape::Bare)
        && !source_dim_names.is_empty()
        && references_bare_source_inside_reducer(&ast, from, false)
    {
        return Err(PartialEquationError::bare_reducer_feeder(&err_text()));
    }

    let mut frozen_ref: Option<Expr0> = None;
    let changed_last = wrap_live_shaped_in_previous(ast, from, shape, &mut frozen_ref, occ, &[]);
    let Some(frozen) = frozen_ref else {
        // No matching occurrence: the "frozen" equation would be the
        // target's own equation, scoring a silent constant 0.
        return Err(PartialEquationError::unfreezable(&err_text()));
    };
    // Same materialization for the changed-last leg. The source-side
    // denominator needs no materialization: `frozen_ref` records the LIVE
    // (pre-freeze) form of the occurrence, so for a slice-shaped source the
    // guard's `SUM(<live slice>)` is the current-vs-previous Δsource the
    // zero-check and SIGN factor want, and it compiles as the target's own
    // equation does.
    let mut last_leg_helpers = Vec::new();
    let changed_last = materialize(changed_last, &mut last_leg_helpers);
    if contains_unfreezable_previous(&changed_last) {
        return Err(PartialEquationError::unfreezable(&err_text()));
    }
    let source_ref = source_ref_for_guard(
        from,
        shape,
        Some(&frozen),
        source_dim_names,
        source_dim_elements,
    );
    freeze_helpers.append(&mut last_leg_helpers);
    // The frozen evaluation is a re-computation of the target's equation,
    // so it needs the same implicit WITH-LOOKUP application the target's
    // own compiled value gets (GH #910).
    let numerator = format!("({target_ref} - ({}))", gf_wrap(print_eqn(&changed_last)));
    Ok(Some(link_score_guard_form_with_numerator(
        &numerator,
        target_ref,
        &source_ref,
    )))
}

/// Wrap every reference to `target` in `PREVIOUS()` -- the *inverse* of
/// [`wrap_non_matching_in_previous`]: freeze ONLY the named variable, keep
/// every other reference live (current-step).
///
/// Used by [`generate_scalar_feeder_to_agg_equation`] to build the
/// "feeder frozen" evaluation of a hoisted reducer's equation. References
/// already inside a `PREVIOUS(...)`/`INIT(...)` call are left untouched
/// (their contents are already lagged/frozen; double-wrapping would read
/// two steps back). Subscript index expressions are recursed into so a
/// `arr[target + 1]` style index reference is frozen too; the outer
/// subscripted variable itself is wrapped only when it names `target`
/// (defensive -- the feeder this is used for is scalar and so is always a
/// bare `Var` reference). An index-position freeze takes the un-lagged operand
/// as its first-DT initial value, exactly as the changed-first walker does
/// ([`freeze_at_previous`], GH #975) -- `in_subscript_index` carries that
/// position down the descent.
fn wrap_matching_in_previous(
    expr: Expr0,
    target: &Ident<Canonical>,
    in_subscript_index: bool,
) -> Expr0 {
    let recurse = |e: Expr0| wrap_matching_in_previous(e, target, in_subscript_index);
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) == target {
                freeze_at_previous(expr, loc, in_subscript_index)
            } else {
                expr
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            let indices: Vec<IndexExpr0> = indices
                .into_iter()
                .map(|idx| match idx {
                    IndexExpr0::Expr(e) => {
                        IndexExpr0::Expr(wrap_matching_in_previous(e, target, true))
                    }
                    other => other,
                })
                .collect();
            let subscript = Expr0::Subscript(ident.clone(), indices, loc);
            if &Ident::<Canonical>::new(ident.as_str()) == target {
                freeze_at_previous(subscript, loc, in_subscript_index)
            } else {
                subscript
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            // Contents of PREVIOUS/INIT are already lagged/frozen.
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                return Expr0::App(UntypedBuiltinFn(name, args), loc);
            }
            let args = args.into_iter().map(recurse).collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, arg, loc) => Expr0::Op1(op, Box::new(recurse(*arg)), loc),
        Expr0::Op2(op, l, r, loc) => {
            Expr0::Op2(op, Box::new(recurse(*l)), Box::new(recurse(*r)), loc)
        }
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(recurse(*c)),
            Box::new(recurse(*t)),
            Box::new(recurse(*f)),
            loc,
        ),
    }
}

/// Generate the link-score equation for a *scalar feeder* of a hoisted
/// reducer: the `feeder → $⁚ltm⁚agg⁚{n}` half of an edge the reference-site
/// IR routed `ThroughAgg`, where the feeder is a scalar variable referenced
/// inside the reducer's argument (`scale` in `SUM(pop[*] * scale)`).
///
/// The standard guard form ([`link_score_guard_form`]) measures the
/// "changed-first" partial `Δ_x z = z(x_t, w_{t-1}) - z_{t-1}` by holding
/// every *other* dependency at `PREVIOUS`. For a scalar feeder of a reducer,
/// rendering that partial as inline equation text does not compile: the
/// reducer's arrayed argument would be frozen as a lagged whole-array read
/// (`SUM(PREVIOUS(pop[*]) * scale)`), which the engine rejects (the GH
/// #541-class wildcard-subscripted `PREVIOUS` capture -- the same shape that
/// keeps the direct `scale→grow` link score uncompilable). Changed-first
/// COULD still be expressed at extra cost -- e.g. one synthesized
/// per-element frozen helper per arrayed reference
/// (`prevpop[Region] = PREVIOUS(pop[Region])`, then
/// `SUM(prevpop[*] * scale)`), a helper-aux emission machinery this path
/// doesn't have today -- so this is a cost/complexity tradeoff, not an
/// impossibility.
///
/// Instead this half uses the algebraically-dual "changed-last" attribution:
/// `Δ_x z = z_t - z(x_{t-1}, w_t)` -- evaluate the reducer with
/// ONLY the feeder frozen at `PREVIOUS` (a scalar `LoadPrev`, always
/// compilable; every array reference stays exactly as in the agg's own
/// equation, which compiles by construction) and subtract from the agg's
/// current value. Both conventions are first-order-equal discrete
/// attributions of `Δz` to `Δx` (LTM scores are inherently path-dependent
/// approximations); for a SUM/MEAN body the two differ only in which step's
/// co-factor weights the feeder's change. For a bilinear body
/// (`SUM(pop[*] * scale)`) the feeder's changed-last half is exactly
/// complementary to the rows' changed-first halves --
/// `Σ_e Δ_pop[e] z + Δ_scale z = Δz` holds identically -- so the mixed
/// convention loses nothing there. The deviation is called out in
/// `docs/reference/ltm--loops-that-matter.md` alongside the numerator-timing
/// convention note.
///
/// The emitted text follows `link_score_guard_form`'s guard structure
/// (zero at the initial step, zero when `Δtarget` or `Δsource` is zero,
/// single-numerator `SAFEDIV` form) with the changed-last numerator
/// `(agg - frozen)` in place of `(partial - PREVIOUS(agg))`.
///
/// Returns `Err` when `agg_equation_text` does not parse -- same loud-failure
/// contract as `build_partial_equation_shaped` (GH #311).
/// `gf_table_ref` is the implicit WITH-LOOKUP wrap (GH #910). `frozen` is a
/// full re-evaluation of the agg's own equation, i.e. gf-INPUT units, while
/// the numerator subtracts it from the agg's (gf-OUTPUT) current value --
/// so a variable-backed agg that is itself a with-lookup target feeds the
/// frozen evaluation through its table. A synthetic `$⁚ltm⁚agg⁚{n}` carries
/// no gf, so its caller passes `None`.
pub(crate) fn generate_scalar_feeder_to_agg_equation(
    feeder: &str,
    agg_name: &str,
    agg_equation_text: &str,
    gf_table_ref: Option<&str>,
) -> Result<String, PartialEquationError> {
    let Ok(Some(ast)) = Expr0::new(agg_equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(agg_equation_text));
    };
    let feeder_ident = Ident::<Canonical>::new(feeder);
    let frozen = print_eqn(&wrap_matching_in_previous(ast, &feeder_ident, false));
    let frozen = match gf_table_ref {
        Some(table_ref) => format!("LOOKUP({table_ref}, {frozen})"),
        None => frozen,
    };
    let agg_q = quote_ident(agg_name);
    let feeder_q = quote_ident(feeder);
    let numerator = format!("({agg_q} - ({frozen}))");
    Ok(link_score_guard_form_with_numerator(
        &numerator, &agg_q, &feeder_q,
    ))
}

/// Generate the per-`(row, slot)` link-score equation for an ITERATED-DIM
/// PROJECTION FEEDER of a hoisted reducer (GH #767 / T5 of the
/// shape-expressiveness design): `frac` in
/// `growth[D1] = SUM(matrix[D1,*] * frac[D1])`, whose accepted slice is
/// the all-`Iterated` projection of the canonical slice, read 1:1 per agg
/// result slot.
///
/// Like the scalar feeder ([`generate_scalar_feeder_to_agg_equation`]),
/// the changed-FIRST partial is uncompilable for a feeder: it would freeze
/// the co-source's wildcard slice as a lagged whole-array read
/// (`SUM(PREVIOUS(matrix[d1·r1, *]) * frac[d1·r1])`, the GH #541 class).
/// So the per-slot equation uses the changed-LAST attribution -- evaluate
/// the reducer's equation pinned to the slot with ONLY the feeder frozen
/// (`SUM(matrix[d1·r1, *] * PREVIOUS(frac[d1·r1]))`, which compiles: the
/// co-source slice stays verbatim, the frozen feeder is a scalar
/// fixed-element `LoadPrev`) and subtract it from the agg slot's current
/// value. For a bilinear body the feeder's changed-last numerator is
/// exactly complementary to the co-source rows' changed-first numerators
/// per slot (`Σ_c Δ_matrix[r,c] + Δ_frac[r] = Δgrowth[r]` identically) --
/// the same complementarity documented on the scalar feeder, now per row.
///
/// `iterated_dims` are the canonical slot axes' dimension names (the
/// feeder's own axis dims -- acceptance guarantees they equal the
/// canonical `Iterated` target dims, in order, unmapped), and
/// `slot_parts_qualified` the slot's qualified elements, parallel to it.
/// Every subscript index in `agg_equation_text` naming one of
/// `iterated_dims` is pinned to the slot's element (the executed A2A
/// resolution for that slot), then the feeder's references are frozen.
///
/// Returns `Err` when the text does not parse (the GH #311 loud-failure
/// contract), when no feeder occurrence was frozen (the numerator would
/// be a silent constant 0 -- the GH #743 unfreezable contract), or when a
/// slot pin is AMBIGUOUS -- a repeated dim name among `iterated_dims` (a
/// degenerate square-source agg, PR #784 review) makes the by-name index
/// resolution unable to tell which slot part an index means, so a pinned
/// equation could freeze the wrong source row (a silently wrong score).
///
/// `gf_table_ref` is the implicit WITH-LOOKUP wrap (GH #910), applied to the
/// frozen re-evaluation exactly as in
/// [`generate_scalar_feeder_to_agg_equation`].
pub(crate) fn generate_iterated_feeder_to_agg_equation(
    feeder: &str,
    agg_name: &str,
    agg_equation_text: &str,
    iterated_dims: &[String],
    slot_parts_qualified: &[String],
    gf_table_ref: Option<&str>,
) -> Result<String, PartialEquationError> {
    let Ok(Some(ast)) = Expr0::new(agg_equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(agg_equation_text));
    };
    // An ambiguous slot pin is the GH #743 unfreezable class: the
    // changed-last convention cannot be rendered as a correct equation, and
    // the changed-first one was already ruled out (see above). The caller
    // warns and skips the row.
    let pinned = pin_iterated_dim_indices(ast, iterated_dims, slot_parts_qualified)
        .ok_or_else(|| PartialEquationError::unfreezable(agg_equation_text))?;
    let feeder_ident = Ident::<Canonical>::new(feeder);
    let frozen = print_eqn(&wrap_matching_in_previous(
        pinned.clone(),
        &feeder_ident,
        false,
    ));
    if frozen == print_eqn(&pinned) {
        // No feeder occurrence was frozen: the "frozen" evaluation would
        // equal the agg slot itself and the score a silent constant 0.
        return Err(PartialEquationError::unfreezable(agg_equation_text));
    }
    let frozen = match gf_table_ref {
        Some(table_ref) => format!("LOOKUP({table_ref}, {frozen})"),
        None => frozen,
    };
    let slot = slot_parts_qualified.join(",");
    let agg_ref = format!("{}[{}]", quote_ident(agg_name), slot);
    // The feeder's row equals the slot 1:1 (unmapped projection acceptance),
    // and its axes ARE the slot's dimensions, so the slot subscript is also
    // the feeder's own qualified row reference.
    let feeder_ref = format!("{}[{}]", quote_ident(feeder), slot);
    let numerator = format!("({agg_ref} - ({frozen}))");
    Ok(link_score_guard_form_with_numerator(
        &numerator,
        &agg_ref,
        &feeder_ref,
    ))
}

/// Pin every subscript index of `expr` that is a bare `Var` naming one of
/// `dims` (canonical dimension names) to the parallel element of `parts`
/// -- the slot-resolution step of
/// [`generate_iterated_feeder_to_agg_equation`]. Wildcards, StarRanges,
/// literals, and indices naming other dimensions are left untouched (a
/// co-source's `Reduced` wildcard must stay a whole-slice read), and
/// expression indices are recursed into.
///
/// Returns `None` when an index names a dim that occurs MORE THAN ONCE in
/// `dims` (a degenerate square-source agg whose slot axes repeat a dim,
/// PR #784 review): the by-name resolution cannot tell which slot part the
/// index means, and first-match would pin every occurrence to the FIRST
/// part -- silently freezing the wrong source row for any off-diagonal
/// slot. Mirrors [`resolve_mismatched_index_position`]'s uniqueness
/// defense; the caller converts `None` into the loud unfreezable error.
///
/// As of GH #778/#785 this ambiguity bail is UNREACHABLE defense-in-depth:
/// the only caller (`iterated_feeder_row_scores`) feeds the agg's
/// `result_dims` as `dims`, and a duplicated `result_dims` is now declined at
/// agg minting (`ltm_agg::result_dims_has_repeated_dim`), so no square-source
/// agg -- and hence no repeated-dim feeder slot -- ever reaches here. The bail
/// is retained as a structural guard in case a future change re-admits the
/// shape upstream; the live square-source landing is now the loud
/// cartesian-branch skip (`emit_unscoreable_duplicated_dim_source_warning`).
fn pin_iterated_dim_indices(expr: Expr0, dims: &[String], parts: &[String]) -> Option<Expr0> {
    /// The unique position of `name` in `dims`: `None` for an ambiguous
    /// (repeated) dim name, `Some(None)` for a name not in `dims` (left
    /// untouched), `Some(Some(pos))` for the unambiguous match.
    fn unique_dim_position(dims: &[String], name: &str) -> Option<Option<usize>> {
        let mut it = dims
            .iter()
            .enumerate()
            .filter_map(|(i, d)| (d.as_str() == name).then_some(i));
        match it.next() {
            None => Some(None),
            Some(pos) => it.next().is_none().then_some(Some(pos)),
        }
    }
    Some(match expr {
        Expr0::Const(..) | Expr0::Var(..) => expr,
        Expr0::Subscript(ident, indices, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    IndexExpr0::Expr(Expr0::Var(name, vloc)) => {
                        let n = canonicalize(name.as_str());
                        match unique_dim_position(dims, n.as_ref())? {
                            Some(pos) => Some(IndexExpr0::Expr(Expr0::Var(
                                RawIdent::new_from_str(&parts[pos]),
                                vloc,
                            ))),
                            None => Some(IndexExpr0::Expr(Expr0::Var(name, vloc))),
                        }
                    }
                    IndexExpr0::Expr(e) => {
                        Some(IndexExpr0::Expr(pin_iterated_dim_indices(e, dims, parts)?))
                    }
                    other => Some(other),
                })
                .collect::<Option<Vec<_>>>()?;
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => Expr0::App(
            UntypedBuiltinFn(
                name,
                args.into_iter()
                    .map(|a| pin_iterated_dim_indices(a, dims, parts))
                    .collect::<Option<Vec<_>>>()?,
            ),
            loc,
        ),
        Expr0::Op1(op, arg, loc) => Expr0::Op1(
            op,
            Box::new(pin_iterated_dim_indices(*arg, dims, parts)?),
            loc,
        ),
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(
            op,
            Box::new(pin_iterated_dim_indices(*l, dims, parts)?),
            Box::new(pin_iterated_dim_indices(*r, dims, parts)?),
            loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(pin_iterated_dim_indices(*c, dims, parts)?),
            Box::new(pin_iterated_dim_indices(*t, dims, parts)?),
            Box::new(pin_iterated_dim_indices(*f, dims, parts)?),
            loc,
        ),
    })
}

/// Where ONE dep is pinned in a scalar per-element link-score equation: the
/// element it reads at that target element, as one
/// `(canonical dimension name, element spelling)` pair per dimension **that dep
/// declares** and that the target element projects onto, in the dep's own
/// declaration order.
///
/// Carrying the dep's OWN dimensions rather than the target's is the whole
/// content of GH #974. A bare arrayed reference inside an apply-to-all body
/// reads its own axes' coordinates matched by dimension NAME, so a dep
/// declaring a strict subset of the target's dimensions (`w[Age]` under
/// `growth[Region,Age]`) must be pinned over that subset, and a dep declaring
/// the same dimensions in another order (`w[Age,Region]`) must be pinned in ITS
/// order. Pinning both with the target's full tuple produced an over-arity
/// subscript in the first case (a fragment that fails to compile, so the score
/// reads a constant 0) and a compilable, silently TRANSPOSED read in the
/// second. The projection itself is `post_transform::dep_element_pins`; this
/// type is just how the answer travels to the rewrite.
#[derive(Clone)]
pub(crate) struct DepElementPin {
    /// The resolved axes for an already-SUBSCRIPTED reference whose index names
    /// one of the dep's own dimensions (`dep[Region]`), as
    /// `(dimension name, element spelling)` in the dep's declaration order. An
    /// axis that does not project is simply absent, which is all such a
    /// reference needs -- it spells its other axes itself.
    pub(crate) axes: Vec<(String, String)>,
    /// The full row a BARE reference (`dep`) is spelled with, in the dep's
    /// declaration order, or `None` when some axis does not project (a bare
    /// reference must be spelled at the dep's full arity or not at all).
    ///
    /// A separate row rather than a `complete` flag over `axes` because the two
    /// spellings resolve by DIFFERENT rules (GH #997): a bare reference is
    /// rewritten into the iterated spelling and read positionally, while a
    /// dimension-name subscript follows the declared element map. See
    /// `post_transform::dep_element_pins`.
    pub(crate) bare_row: Option<Vec<String>>,
}

/// Replace every reference to a pinned dep in `equation_text` with that dep's
/// own element subscript, per [`pins`](DepElementPin).
///
/// Used when collapsing a scalar-source -> arrayed-target link score into
/// per-target-element scalar variables: the target's A2A equation body
/// references arrayed deps that share the target's dimensions *bare* (the
/// A2A expansion subscripts them at runtime), but a *scalar* per-element
/// link-score variable must spell out the subscript. `pins` says which deps
/// those are and which element each one reads -- the caller computes it,
/// because deciding that needs the deps' declared dimensions and the project's
/// dimension mappings.
///
/// A bare `Var(id)` reference to a COMPLETELY-pinned dep becomes
/// `Subscript(id, <its row>)`. An already-`Subscript`ed reference keeps its
/// literal element indices, but an index naming one of the dep's OWN
/// dimensions (`dep[Region]`, the A2A iterated reference form) reads "the
/// current element" -- which in a per-element scalar equation is exactly the
/// element being pinned -- so it is substituted whether or not the pin is
/// complete. Left unpinned, such an index is unresolvable in scalar context
/// and forces a synthesized helper aux per occurrence (GH #654: ~27k of
/// C-LEARN's ~30k residual helpers came from this form). Function-name
/// identifiers and identifiers absent from `pins` are left alone. The result is
/// re-printed in the canonical equation format (via parse + `print_eqn`).
///
/// Returns `Ok(equation_text)` unchanged when `pins` is empty (nothing to
/// pin -- a legitimate no-op), and `Err([`PartialEquationError`])` when the
/// (already-PREVIOUS-wrapped) partial text fails to re-parse. The latter is
/// loud rather than a silent lowercased-input fallback for the same reason
/// as `build_partial_equation_shaped` (GH #311): an un-pinned partial may
/// not even compile, and a silent wrong equation is worse than skipping the
/// score with a warning.
///
/// Each element spelling is a single element name (`"nyc"`, or qualified
/// `"region·nyc"`) -- the same form `db::ltm::cartesian_subscripts` produces
/// (and `qualify_element_csv` qualifies) and the `parse_link_offsets` discovery
/// parser expects on the `to` side.
pub(crate) fn subscript_idents_at_element(
    equation_text: &str,
    pins: &HashMap<Ident<Canonical>, DepElementPin>,
) -> Result<String, PartialEquationError> {
    if pins.is_empty() {
        return Ok(equation_text.to_string());
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
    };
    Ok(print_eqn(&subscript_idents_in_expr0(ast, pins)))
}

/// The first subscript index in `equation_text` that names a project DIMENSION,
/// or `None` when every index resolves.
///
/// A per-element link-score partial is a SCALAR fragment, so every subscript
/// index in it must select ONE element. An index left as a bare dimension name
/// still denotes the whole axis: it cannot lower
/// (`ErrorCode::DimensionInScalarContext`), and when it sits inside a frozen
/// subtree `builtins_visitor` hoists it into a `PREVIOUS`-capture helper that
/// fails on its own WHILE THE PARENT STILL COMPILES -- so the score exists and
/// reads part of its own equation as a constant 0.
///
/// This inspects the FINISHED partial rather than predicting from the pin table,
/// and that is the point: a dep can be unpinnable and still need no pin (an
/// index that is a runtime variable read, `source[idx]`, resolves by itself), so
/// "the pin table does not cover this dep" over-declines. What matters is only
/// whether an unresolvable index survived into the text about to be compiled.
///
/// A returned `Some` is the caller's cue to decline the edge loudly rather than
/// emit a score computed around a hole.
pub(crate) fn unresolvable_dimension_index(
    equation_text: &str,
    dims_ctx: &crate::dimensions::DimensionsContext,
) -> Option<String> {
    fn walk(
        expr: &Expr0,
        dims_ctx: &crate::dimensions::DimensionsContext,
        found: &mut Option<String>,
    ) {
        if found.is_some() {
            return;
        }
        match expr {
            Expr0::Const(..) => {}
            Expr0::Var(..) => {}
            Expr0::Subscript(ident, indices, _) => {
                for idx in indices {
                    if let IndexExpr0::Expr(Expr0::Var(name, _)) = idx {
                        let canonical = canonicalize(name.as_str());
                        if dims_ctx.is_dimension_name(canonical.as_ref()) {
                            *found = Some(format!("{}[{}]", ident.as_str(), canonical));
                            return;
                        }
                    }
                    if let IndexExpr0::Expr(e) = idx {
                        walk(e, dims_ctx, found);
                    }
                }
            }
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                for a in args {
                    walk(a, dims_ctx, found);
                }
            }
            Expr0::Op1(_, l, _) => walk(l, dims_ctx, found),
            Expr0::Op2(_, l, r, _) => {
                walk(l, dims_ctx, found);
                walk(r, dims_ctx, found);
            }
            Expr0::If(c, t, f, _) => {
                walk(c, dims_ctx, found);
                walk(t, dims_ctx, found);
                walk(f, dims_ctx, found);
            }
        }
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        // Unparseable text is a different failure, reported by the caller that
        // produced it; this predicate says nothing about it.
        return None;
    };
    let mut found = None;
    walk(&ast, dims_ctx, &mut found);
    found
}

/// The `IndexExpr0` a pin entry's element spelling becomes.
fn pin_index(elem: &str) -> IndexExpr0 {
    IndexExpr0::Expr(Expr0::Var(
        crate::common::RawIdent::new_from_str(elem),
        crate::ast::Loc::default(),
    ))
}

fn subscript_idents_in_expr0(
    expr: Expr0,
    pins: &HashMap<Ident<Canonical>, DepElementPin>,
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(ident.as_str());
            // Only a COMPLETE row can spell a bare reference: a subscript
            // covering some of the dep's axes is not a legal reference at all.
            match pins.get(&canonical).and_then(|pin| pin.bare_row.as_ref()) {
                Some(row) => Expr0::Subscript(
                    ident.clone(),
                    row.iter().map(|elem| pin_index(elem)).collect(),
                    loc,
                ),
                None => expr,
            }
        }
        // An already-subscripted reference to a pinned dep: indices that are
        // *element literals* are already pinned and stay, but an index that
        // names one of that dep's own DIMENSIONS gets this element's coordinate
        // for it. The lookup is over the DEP's dimensions, so a subset-dims or
        // reordered dep resolves each of its axes correctly (GH #974) -- and an
        // axis the target does not project (`pop[Region, idx]` under a
        // `growth[Region]` target, whose `Age` axis the reference pins itself)
        // simply has no entry, which is why an INCOMPLETE pin still applies here.
        Expr0::Subscript(ident, indices, loc) => {
            let canonical = Ident::new(ident.as_str());
            let Some(pin) = pins.get(&canonical) else {
                return Expr0::Subscript(ident, indices, loc);
            };
            let indices = indices
                .into_iter()
                .map(|idx| {
                    if let IndexExpr0::Expr(Expr0::Var(name, _)) = &idx {
                        let idx_canonical = canonicalize(name.as_str());
                        if let Some((_, elem)) = pin
                            .axes
                            .iter()
                            .find(|(dim, _)| dim.as_str() == idx_canonical.as_ref())
                        {
                            return pin_index(elem);
                        }
                    }
                    idx
                })
                .collect();
            Expr0::Subscript(ident, indices, loc)
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| subscript_idents_in_expr0(a, pins))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => {
            Expr0::Op1(op, Box::new(subscript_idents_in_expr0(*inner, pins)), loc)
        }
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(subscript_idents_in_expr0(*lhs, pins)),
            Box::new(subscript_idents_in_expr0(*rhs, pins)),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(subscript_idents_in_expr0(*cond, pins)),
            Box::new(subscript_idents_in_expr0(*then_expr, pins)),
            Box::new(subscript_idents_in_expr0(*else_expr, pins)),
            loc,
        ),
    }
}

/// Generate a per-target-element scalar link-score equation for a
/// scalar-source -> arrayed-target edge (or an arrayed-agg -> arrayed-target
/// edge: the `agg → to` half a hoisted sliced reducer produces).
///
/// For target element `element` of arrayed target `to`, produces the
/// link-score guard form (`link_score_guard_form`) whose partial holds
/// the source `from` live and freezes everything else at PREVIOUS,
/// with the target reference (and any arrayed deps that share the target's
/// dimension -- including the agg name when `from` is an arrayed agg) pinned
/// to `element`. The result is `Equation::Scalar`-shaped text -- one such
/// variable is emitted per target element, named
/// `$⁚ltm⁚link_score⁚{from}→{to}[{element}]`, mirroring the arrayed->scalar
/// `{from}[{elem}]→{to}` convention from `generate_element_to_scalar_equation`.
///
/// `to_elem_eqn` is the target's OWN equation AST for this element (the hoisted
/// reducers still spelled `SUM(...)`, NOT agg-substituted -- Track A stage 1),
/// lowered straight from its `Expr2` by [`crate::patch::expr2_to_expr0`]: the
/// shared A2A body for an `Equation::ApplyToAll` target, or the matching
/// per-element slot (or the default slot) for an `Equation::Arrayed` one. `reducer_subst` maps each hoisted reducer's
/// canonical text to its agg name; it is empty for a true scalar `from`. `to_deps`
/// is the full dependency set of that equation (computed with the target's AST
/// dimensions so element-name subscripts are not mistaken for variables), plus
/// the agg names. `to_deps_to_subscript` is the pin table for the subset of
/// `to_deps` that must be element-pinned -- the arrayed deps whose every
/// declared dimension this target element projects onto, each mapped to the
/// element IT reads (see [`DepElementPin`]; the target self-reference is pinned
/// implicitly via the already-subscripted `to[element]` reference the guard
/// form is built around). The source itself is never in this table: an
/// arrayed-agg source is pinned via its `source_pins` entry instead, since an
/// agg has no declared dimensions of its own to project onto (GH #528).
///
/// `source_ref_override`: the pre-rendered (quoted, possibly element-pinned)
/// reference expression to use for the `Δsource` denominator. `None` uses the
/// bare `quote_ident(from)` -- correct for a true scalar source. The
/// arrayed-agg caller passes `Some("$⁚ltm⁚agg⁚n"[<slot>])` so the denominator
/// indexes the same agg slot the link-score name and the (subscripted-in-the-
/// partial) numerator do; a bare agg reference in a scalar equation would not
/// compile and the link score would stub to zero.
///
/// `source_pins`: the per-ident pin list (GH #751) -- for each `(ident, pin)`
/// entry, that ident's references in the partial BODY are pinned to the
/// (qualified) element tuple `pin` names. Empty for a true scalar source
/// (no pinning). The arrayed-agg caller passes one entry per ARRAYED agg
/// referenced by the substituted equation: the LIVE agg's entry carries the
/// target element's projection onto its `result_dims` axes -- the same slot
/// the link-score name and `source_ref_override` carry (GH #528) -- and
/// each frozen co-agg (a second hoisted reducer in the same target
/// equation) carries the projection onto ITS OWN `result_dims`, so its
/// `PREVIOUS(...)` freeze reads one slot instead of the ill-typed bare
/// multi-slot reference that failed fragment compile and stubbed the score
/// to 0 (GH #751). This is a SEPARATE channel from `to_deps_to_subscript`
/// because an agg is a synthetic aux with no declared dimensions: its slot
/// space is the hoisted reducer's `result_dims`, which the declared-dimension
/// projection behind `to_deps_to_subscript` cannot derive. For the diagonal
/// case (`result_dims` == `to`'s dims) the projection IS the full tuple, so
/// the equation is unchanged. A SCALAR co-agg needs no entry: its bare
/// `PREVIOUS(name)` freeze compiles as-is.
///
/// `gf_table_ref` is the implicit WITH-LOOKUP wrap for THIS target element
/// (GH #910): the partial is a full re-evaluation of the target's element
/// equation, i.e. gf-INPUT units, while the guard form ratios it against
/// gf-OUTPUT target deltas. `None` for an ordinary (gf-less) target. See
/// [`WithLookupSlotRefs`].
///
/// `occ` is the target slot's occurrence-IR lookup the ceteris-paribus wrap
/// consumes -- the SINGLE classifier family every caller threads. A SCALAR-source
/// caller (`try_scalar_to_arrayed_link_scores`) NEEDS it: `from` is a model
/// `Variable` that can appear bare inside a reducer of the target's equation, and
/// the GH #517 reducer-freeze arm consults the stream to decide whether to freeze
/// that reducer whole or recurse into it. An empty stream froze it whole,
/// silently zeroing a score HEAD scored (or loudly declined). An AGG-source
/// caller (`from = $⁚ltm⁚agg⁚n`) also threads the real stream, but it is
/// behavior-neutral there: the live source is a synthetic aggregate held live by
/// `reducer_subst` text-matching, never a recorded occurrence, so every wrap
/// lookup for it misses whether the stream is empty or not.
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_scalar_to_element_equation(
    from: &str,
    to: &str,
    element: &str,
    to_elem_eqn: &Expr0,
    reducer_subst: &HashMap<String, String>,
    to_deps: &HashSet<Ident<Canonical>>,
    to_deps_to_subscript: &HashMap<Ident<Canonical>, DepElementPin>,
    source_ref_override: Option<&str>,
    source_pins: &[(Ident<Canonical>, DepElementPin)],
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
    gf_table_ref: Option<&str>,
    occ: &OccurrenceLookup<'_>,
    dep_dims: Option<&HashMap<String, Vec<crate::dimensions::Dimension>>>,
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<String, PartialEquationError> {
    let from_canonical = Ident::new(from);
    let from_q = quote_ident(from);
    let to_q = quote_ident(to);
    let to_elem = format!("{to_q}[{element}]");

    // Composition inverted (Track A stage 1): `to_elem_eqn` is the target
    // element's OWN equation AST (reducers still spelled `SUM(...)`). When `from`
    // is an arrayed/scalar agg, the reducer that became it is held LIVE by the
    // wrap (matched by text via `reducer_subst`); a true scalar `from` (empty
    // `reducer_subst`) keeps the ident-live `RefShape::Bare` wrap. Either way
    // `source_dim_elements` is empty (no source subscripts to classify) and
    // there is no iterated-dim context. The reducer -> agg-name substitution is
    // a POST-transform lowering of the wrapped AST, so the later element-pinning
    // / `source_pins` passes see the same agg-named text they did before.
    //
    // `occ` is the target slot's real occurrence stream (threaded by every
    // caller); the wrap consults it at the reducer-freeze arm so a scalar `from`
    // read bare inside a reducer recurses exactly like the shaped per-shape path
    // does. For an agg `from` the stream is behavior-neutral (the agg is never a
    // recorded occurrence), so the agg callers thread it too rather than a
    // separate empty-stream path.
    let live_reducer_text = live_reducer_text_for_agg(reducer_subst, from);
    let (wrapped, out) = wrap_changed_first_ast(
        to_elem_eqn,
        to_deps,
        &from_canonical,
        &RefShape::Bare,
        live_reducer_text,
        None,
        dims_ctx,
        occ,
        None,
    );
    // Loud degradation over a silent zero (matching `shaped_guard_form_text` /
    // `build_partial_equation_shaped_with_live_ref`): a walker desync
    // (`missing_occurrence`) or a GH #526 mismatched other-dep
    // (`other_dep_mismatch`) makes the changed-first partial unusable. Both are
    // inert for the agg-source callers (the agg is never a walked node, so
    // `missing_occurrence` cannot fire, and `iter_ctx` is `None`), so this only
    // affects the scalar-source path.
    if out.missing_occurrence || out.other_dep_mismatch {
        return Err(PartialEquationError::unfreezable(&print_eqn(to_elem_eqn)));
    }
    // GH #995: materialize any array-slice freeze the wrap produced (a frozen
    // `def[*, r1]` other-dep, say) into its helper reference BEFORE printing;
    // this path never doom-checked, so an inline `PREVIOUS(<slice>)` used to
    // reach codegen and fail there. What the materializer declines is left
    // verbatim and keeps that (loud, `model_ltm_fragment_diagnostics`-surfaced)
    // failure. The later text passes pin only bare dep idents, so the quoted
    // helper reference is inert to them.
    let substituted = substitute_reducers_in_expr0(wrapped, reducer_subst);
    // GH #995 option C: a per-element SCALAR partial must not carry an
    // array-producing builtin -- its result cannot live in a scalar
    // fragment, and for the order-statistic subset the scalarization pins
    // the ranked array to one element, whose rank is meaningless. Checked on
    // the SUBSTITUTED AST, not the raw target equation: a rank-like call
    // covered by `reducer_subst` is gone from the partial -- the agg
    // reference carries the correctly-slotted whole-array rank (GH #742/#771)
    // and must not be declined. That rescue only applies at the AGG-HALF
    // call sites, which thread a populated `reducer_subst`; the
    // scalar-source callers pass an empty map, so a hoisted rank-like call
    // co-occurring with a scalar dep IS declined there -- an improvement
    // over the pre-decline behavior (a compiling score whose
    // `PREVIOUS(RANK(...))` capture helper failed and read constant 0), and
    // scoreable in principle if those callers ever thread the GH #751
    // frozen-co-agg substitution.
    if contains_rank_like_builtin(&substituted) {
        return Err(PartialEquationError::rank_like_partial(&print_eqn(
            to_elem_eqn,
        )));
    }
    // Element pinning runs on the AST (the same `subscript_idents_in_expr0`
    // core the text-level `subscript_idents_at_element` wraps -- print + parse
    // of our own output is a canonical fixpoint, so this is byte-identical),
    // and it runs BEFORE the GH #995 freeze materializer: an A2A-shaped slot
    // body spells a frozen slice's non-sliced axis as a bare DIMENSION name
    // (`PREVIOUS(def[*, Aggregated_Regions])`), which only the pin resolves to
    // this element's row -- materializing first would see a dynamic-looking
    // index and decline. The pin passes touch only dep idents, so the quoted
    // helper reference the materializer swaps in cannot be re-pinned.
    let mut pinned = subscript_idents_in_expr0(substituted, to_deps_to_subscript);
    // Pin each mapped ident's references (the live source's numerator
    // occurrence and any PREVIOUS-wrapped co-agg freezes alike) to its own
    // projected slot -- separate passes from the `to_deps_to_subscript` pinning
    // above, because each agg's slot is the projection onto ITS `result_dims`
    // rather than onto its declared dimensions (an agg is not a model variable
    // and has none). The passes touch disjoint idents, so application order is
    // irrelevant.
    for (ident, pin) in source_pins {
        let one: HashMap<Ident<Canonical>, DepElementPin> =
            std::iter::once((ident.clone(), pin.clone())).collect();
        pinned = subscript_idents_in_expr0(pinned, &one);
    }
    // Same scalar-dep augmentation as `shaped_guard_form_text` (see there):
    // a frozen scalar in a view-position argument needs a scalar helper.
    let freeze_dep_dims = dep_dims.map(|dd| {
        let mut m = dd.clone();
        for d in to_deps {
            m.entry(d.as_str().to_string()).or_default();
        }
        m
    });
    let pinned = match (freeze_dep_dims.as_ref(), dims_ctx) {
        (Some(dd), Some(dc)) => materialize_array_freezes(pinned, dd, dc, freeze_helpers),
        _ => pinned,
    };
    let mut partial = print_eqn(&pinned);
    // The wrap goes on LAST because it is not part of the target's equation:
    // everything above rewrites that equation within its own (gf-input) domain
    // -- freezing, element pinning, slot projection -- while the `LOOKUP` is the
    // compiler's implicit lowering (`apply_implicit_with_lookup`) applied to the
    // finished input expression. It belongs outside those passes, not before them.
    if let Some(table_ref) = gf_table_ref {
        partial = format!("LOOKUP({table_ref}, {partial})");
    }
    let source_ref = source_ref_override.unwrap_or(&from_q);
    Ok(link_score_guard_form(&partial, &to_elem, source_ref))
}

/// Generate the per-(row, full-target-element) scalar link-score equation
/// for one `PerElement` reference site (GH #525, T6 of the
/// shape-expressiveness design): the partial of target element
/// `element_qualified`'s equation w.r.t. the live source's `site_axes`
/// occurrence. The composition is `wrap(own equation) then row-pin` (Track A
/// stage 1): the ceteris-paribus wrap runs on the target element's OWN
/// equation text with the site's actual `PerElement { site_axes }` shape held
/// live, and the row-pinning is a POST-transform lowering
/// (`post_transform::pin_source_subscript_indices`, run FROM the wrap). The result:
///
/// - the live occurrence (held live by the wrap because its shape equals
///   `site_axes`) lowered to the concrete row subscript `{from}[{row}]` (a
///   real `Expr0::Subscript`, never `SUM(...)`-wrapped),
/// - every OTHER source occurrence (frozen at `PREVIOUS` by the wrap) lowered
///   to ITS row for this element (each is attributed by its own link score:
///   another `PerElement` site's scalar, the Bare A2A score of a mixed
///   edge, a `FixedIndex` site's per-element score),
/// - the target's other arrayed deps element-pinned via
///   [`subscript_idents_at_element`] (`to_deps_to_subscript` must NOT
///   contain the source -- its pinning is the lowering's job), and
/// - the guard form's target/source references pinned to
///   `to[{element}]` / `{from}[{row}]`.
///
/// `row_parts_bare` is the row [`per_element_row_for_target`] derives for
/// `site_axes` at this element -- the caller computes it once and uses it
/// for the variable NAME too, so name and equation cannot disagree.
///
/// `gf_table_ref` is this target element's implicit WITH-LOOKUP table
/// reference (`None` when the element applies no gf); the partial is a full
/// re-evaluation of the element's equation, so it must be fed through the
/// table to reach the same units as the `to[{element}]` deltas it is
/// ratioed against (GH #910; see [`with_lookup::is_implicit_with_lookup`]).
#[allow(clippy::too_many_arguments)] // threads the per-(site, element) emission context
pub(crate) fn generate_per_element_link_equation(
    from: &str,
    to: &str,
    site_axes: &[crate::ltm_agg::AxisRead],
    row_parts_bare: &[String],
    element_qualified: &str,
    to_elem_eqn: &Expr0,
    to_deps: &HashSet<Ident<Canonical>>,
    to_deps_to_subscript: &HashMap<Ident<Canonical>, DepElementPin>,
    from_dims: &[crate::dimensions::Dimension],
    target_elem_by_dim: &HashMap<String, (String, usize)>,
    target_dims: &[crate::dimensions::Dimension],
    target_elements: &[String],
    target_iterated_dims: &[String],
    dims_ctx: &crate::dimensions::DimensionsContext,
    gf_table_ref: Option<&str>,
    occ: &OccurrenceLookup<'_>,
) -> Result<String, PartialEquationError> {
    let from_canonical = Ident::<Canonical>::new(from);
    // GH #995 option C: same rank-like decline as
    // `generate_scalar_to_element_equation` -- this emitter's output is a
    // per-(row, element) SCALAR partial too.
    if contains_rank_like_builtin(to_elem_eqn) {
        return Err(PartialEquationError::rank_like_partial(&print_eqn(
            to_elem_eqn,
        )));
    }
    let source_dim_names: Vec<String> = from_dims.iter().map(|d| d.name().to_string()).collect();
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims,
        // KNOWN GAP, and the comment this replaces is why. It read "the
        // `PerElement` live shape suppresses the GH #526 other-dep collapse
        // entirely, so the verdict's `dep_dims` are never consulted; none to
        // thread" -- true when `other_dep_verdict` was the ONLY consumer.
        // `axis_dim_at` is a second one, so `None` here leaves every subscript
        // index under this emitter on the project-wide fallbacks: a silent wrong
        // number on a collision, reproduced on a compiling model. See
        // `index_axis_verdict`; threading it is its own change, because the two
        // consumers want different tables.
        dep_dims: None,
    };
    let ref_ctx = PerElementRefCtx {
        from: &from_canonical,
        site_axes,
        row_parts_bare,
        from_dims,
        target_dims,
        target_elements,
        target_elem_by_dim,
        dim_ctx: dims_ctx,
    };
    // The ceteris-paribus wrap runs on the target element's OWN equation,
    // holding the site's ACTUAL `PerElement` shape live, and row-pins each
    // source reference AS IT GOES (`ref_ctx`). Two earlier arrangements are
    // worth knowing about, because each was wrong in a different way. Pinning
    // FIRST and wrapping a synthesized `FixedIndex(row)`-shaped derived text
    // (pre-`b7898692`) produced text no occurrence stream describes, so the
    // occurrence IR could not drive the wrap at all. Wrapping first and pinning
    // AFTER (`b7898692`) fixed that, but the pass then had to re-derive every
    // occurrence's per-axis access with an Expr0 classifier, because a `SiteId`
    // computed on the original AST cannot address a tree the wrap has inserted
    // `PREVIOUS` nodes into. Pinning inside the wrap needs neither: the
    // occurrence is reachable by path, and the wrap is the only place that knows
    // whether it is about to freeze the reference -- which is exactly what picks
    // the bare row for the live occurrence and the qualified row for the rest.
    // The `PerElement` live shape
    // suppresses the GH #526 other-dep collapse (an iterated other-dep like
    // `w[Age]` keeps its subscript so the post-transform per-element pin can
    // resolve each dimension-name index -- collapsing to bare would let the
    // full-tuple pin over-subscript a subset-dims dep), so the mismatch doom
    // can never fire here; the `out.other_dep_mismatch` check below is retained
    // defensively to preserve `wrap_changed_first_ast`'s doom contract.
    //
    // `out.missing_occurrence` is checked with it, and is NOT merely defensive:
    // a live-source occurrence the IR could not record (a walker desync, or a
    // child index a `SiteId` cannot address) leaves the source looking non-live,
    // so the wrap freezes it at `PREVIOUS` and the score comes out a clean ZERO.
    // Every other wrap caller already dooms on it (`shaped_guard_form_text`,
    // `generate_scalar_to_element_equation`,
    // `build_partial_equation_shaped_with_live_ref`); this path omitting it was
    // the one hole through which that silent zero could still reach an emitter.
    let live_shape = RefShape::PerElement {
        axes: site_axes.to_vec(),
    };
    let (wrapped, out) = wrap_changed_first_ast(
        to_elem_eqn,
        to_deps,
        &from_canonical,
        &live_shape,
        None,
        Some(&iter_ctx),
        Some(dims_ctx),
        occ,
        Some(&ref_ctx),
    );
    if out.other_dep_mismatch || out.missing_occurrence {
        return Err(PartialEquationError::unfreezable(&print_eqn(to_elem_eqn)));
    }
    let partial = print_eqn(&wrapped);
    let mut partial = subscript_idents_at_element(&partial, to_deps_to_subscript)?;
    if let Some(table_ref) = gf_table_ref {
        partial = format!("LOOKUP({table_ref}, {partial})");
    }
    let row_qualified: String = row_parts_bare
        .iter()
        .zip(from_dims)
        .map(|(part, dim)| qualify_axis_element(part, dim))
        .collect::<Vec<_>>()
        .join(",");
    let to_elem = format!("{}[{element_qualified}]", quote_ident(to));
    let source_ref = format!("{}[{row_qualified}]", quote_ident(from));
    Ok(link_score_guard_form(&partial, &to_elem, &source_ref))
}

/// Generate the `agg → scalar-target` link-score equation: the partial of
/// `to`'s (scalar) equation w.r.t. the aggregate node `agg_name` held live,
/// everything else PREVIOUS. The result is `Equation::Scalar`-shaped text,
/// named `$⁚ltm⁚link_score⁚{agg}→{to}`.
///
/// Composition inverted (Track A stage 1): `to_own_eqn` is `to`'s OWN
/// equation AST (hoisted reducers still spelled `SUM(...)`), and
/// `reducer_subst` maps each hoisted reducer's canonical text to its agg name.
/// The ceteris-paribus wrap runs on that own AST with the reducer that became
/// `agg_name` held LIVE verbatim (matched by text; every co-reducer freezes
/// whole via the GH #517 path), and the agg substitution is a POST-transform
/// lowering ([`substitute_reducers_in_expr0`]) of the wrapped AST -- the
/// held-live reducer to a bare agg name, each frozen `PREVIOUS(SUM(..))` to
/// `PREVIOUS(agg)`. This keeps the wrap on the target's own equation (so the
/// occurrence IR can drive it in stage 2) with byte-identical output. `to_deps`
/// is the (over-approximating is fine) dependency set that includes `to`'s own
/// reducer-source vars and the agg names.
///
/// For an *arrayed* target the per-target-element form is produced by
/// [`generate_scalar_to_element_equation`] instead (with `from = agg_name`).
///
/// `gf_table_ref` is the target's implicit WITH-LOOKUP table reference
/// ([`with_lookup_table_ref`], `None` when the target applies no gf): the
/// partial re-evaluates the gf's *input*, so it must be fed through the table
/// to be commensurable with the `PREVIOUS(to)` anchor the guard form subtracts
/// (GH #910).
///
/// `occ` is the (scalar) target's occurrence stream. Its live source is the
/// synthetic agg (`$⁚ltm⁚agg⁚n`), which is NEVER a recorded occurrence -- it is
/// held live by `reducer_subst` text-matching, and during the wrap the reducer
/// that became it is still spelled `SUM(...)` -- so every `subtree_has_live_shape`
/// / `get` for the agg misses and the stream is behavior-neutral here (the
/// co-reducers freeze whole, the agg reducer is held live by text). The caller
/// threads the REAL stream regardless: there is one classifier family, not an
/// empty-stream shadow path.
#[allow(clippy::too_many_arguments)] // threads the agg-to-target generation context
pub(crate) fn generate_agg_to_scalar_target_equation(
    agg_name: &str,
    to_name: &str,
    to_own_eqn: &Expr0,
    reducer_subst: &HashMap<String, String>,
    to_deps: &HashSet<Ident<Canonical>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
    gf_table_ref: Option<&str>,
    occ: &OccurrenceLookup<'_>,
) -> Result<String, PartialEquationError> {
    let agg_canonical = Ident::new(agg_name);
    let agg_q = quote_ident(agg_name);
    let to_q = quote_ident(to_name);
    // The reducer subexpression that was hoisted into `agg_name` -- held live
    // verbatim by the wrap. `None` (agg not in `reducer_subst`) degrades to the
    // ident-live wrap on `agg_canonical`, which never matches the own text and
    // so freezes everything (defensive; not a production shape). The agg node
    // is a scalar -- referenced bare, no iterated-dim context.
    let live_reducer_text = live_reducer_text_for_agg(reducer_subst, agg_name);
    let (wrapped, _out) = wrap_changed_first_ast(
        to_own_eqn,
        to_deps,
        &agg_canonical,
        &RefShape::Bare,
        live_reducer_text,
        None,
        dims_ctx,
        occ,
        None,
    );
    let mut partial = print_eqn(&substitute_reducers_in_expr0(wrapped, reducer_subst));
    if let Some(table_ref) = gf_table_ref {
        partial = format!("LOOKUP({table_ref}, {partial})");
    }
    Ok(link_score_guard_form(&partial, &to_q, &agg_q))
}

/// The canonical text of the hoisted reducer that became `agg_name`, for the
/// wrap's `live_reducer_text` channel (Track A stage 1). `reducer_subst` maps
/// reducer text -> agg name; an agg is hoisted from exactly one canonical
/// reducer text, so at most one entry matches. `None` when `agg_name` is not a
/// key's value (an empty `reducer_subst`, or a true scalar `from`), which keeps
/// the historical ident-live wrap.
fn live_reducer_text_for_agg<'a>(
    reducer_subst: &'a HashMap<String, String>,
    agg_name: &str,
) -> Option<&'a str> {
    reducer_subst
        .iter()
        .find(|(_, agg)| agg.as_str() == agg_name)
        .map(|(text, _)| text.as_str())
}

/// Quote a canonical identifier for use in a generated equation string.
/// Identifiers the equation lexer cannot read bare need double quotes: special
/// characters (`$`, `⁚`, `·`), a name whose first character cannot START an
/// identifier (`1stock` -- a legal quoted XMILE name, but bare the lexer reads
/// the number `1` then the identifier `stock`), and a name the lexer resolves to
/// a KEYWORD (`if`, `mod`, `nan`, ...).
///
/// The leading-character and keyword rules are [`crate::ast::needs_quoting`],
/// the same predicate the `print_eqn` path's `print_ident` uses -- so the two
/// spellings of one name inside a single generated equation cannot disagree.
/// They did: this used to test only "every char is alphanumeric or `_`", which a
/// leading digit satisfies, so a guard form emitted `print_eqn`'s quoted
/// `"1stock"` in the partial beside a bare `1stock` in the `SIGN(...)` factor --
/// an unparseable equation, hence a silently-zeroed link score on a valid model.
/// A keyword satisfied that test too, and is now covered by the same delegation
/// (GH #976).
///
/// This deliberately stays MORE conservative than `print_ident` rather than
/// delegating outright: `·` (U+00B7) IS `XID_Continue`, so `needs_quoting`
/// alone would spell a module-output composite bare (`mod·out1`). That parses,
/// but it would rewrite the emitted text of every module-composite link score
/// -- a behavior change with no bug behind it. So the alphanumeric conjunct is
/// NOT redundant; a name is bare only when BOTH predicates allow it, and the
/// conjunction can only ever add quotes. `quote_ident_needs_both_of_its_conjuncts`
/// pins each conjunct on the row only it rejects.
pub(crate) fn quote_ident(ident: &str) -> String {
    let bare_spellable =
        ident.chars().all(|c| c.is_alphanumeric() || c == '_') && !crate::ast::needs_quoting(ident);
    if bare_spellable {
        ident.to_string()
    } else {
        format!("\"{ident}\"")
    }
}

/// Compute the canonical synthetic-variable name for a per-shape link score.
///
/// Naming convention:
/// - `Bare`: `$⁚ltm⁚link_score⁚{from}→{to}` — the A2A/scalar form.
/// - `FixedIndex(elems)`: `$⁚ltm⁚link_score⁚{from}[{elems_joined}]→{to}` —
///   the per-element prefixed-from form also used by
///   `try_cross_dimensional_link_scores`.
/// - `PerElement` NEVER reaches this function: its names carry BOTH a
///   from-side row and a to-side element
///   (`$⁚ltm⁚link_score⁚{from}[{row}]→{to}[{e}]`, the existing per-(row,
///   slot) grammar) and are minted directly by
///   `emit_per_element_link_scores` (GH #525, T6);
///   `emit_per_shape_link_scores` filters the shape out before its
///   name-dedup loop.
/// - `Wildcard` / `DynamicIndex`: same as `Bare`. The emitter dedups by the
///   resulting name, so any such slot collapses onto the canonical Bare name
///   rather than minting a `⁚wildcard`/`⁚dynamic` variant. Every
///   statically-describable inlined reducer -- whole-extent (`SUM(pop[*])`)
///   or sliced (`SUM(pop[NYC, *])`, `SUM(matrix[D1, *])`, and the
///   positionally-mapped `SUM(matrix[State, *])` of GH #534) -- is hoisted
///   into a `$⁚ltm⁚agg⁚{n}` node, so the only `Direct` references with these
///   shapes that reach `emit_per_shape_link_scores` are a *whole-RHS*
///   variable-backed reducer's argument (`total = SUM(population[*])`), a
///   bare dynamic index (`arr[i+1]`), the dynamic-index reducer carve-out
///   (`SUM(pop[idx, *])`), a sliced reducer the correspondence declines (an
///   UNDECLARED pair, a cardinality mismatch, or a `MappedRead` axis --
///   GH #997; a DECLARED mapping is accepted in either direction since
///   GH #757, an explicit element map included since #997),
///   or a DE-HOISTED array-valued reducer's wildcard arg
///   (`RANK(pop[*], 1)` -- GH #771: RANK is not `reducer_is_hoistable`, so
///   its wildcard-subscripted argument stays a `Direct` `Wildcard` site and
///   collapses onto the canonical Bare name here; the bare-arg spelling
///   `RANK(pop, 1)` classifies `Bare` directly).
///   A coarse conservative score is the intended semantics where the
///   endpoint dimensions correspond; when both endpoints are arrayed and
///   they do NOT (the declined mapped-reducer cases above), no compilable
///   conservative shape exists (a scalar equation cannot reference the
///   arrayed endpoints; an arrayed one would be read at wrong slots by the
///   cross-product loop links), so `emit_per_shape_link_scores` skips the
///   edge with one Warning and no link-score variable, and loop scores
///   through it are dropped (GH #758).
///
/// The Unicode separators `\u{205A}` (TWO DOT PUNCTUATION) and `\u{2192}`
/// (RIGHTWARDS ARROW) are intentional: they collide with no legal
/// identifier, so the generated names cannot be confused with user
/// variables.
pub(crate) fn link_score_var_name(from: &str, to: &str, shape: &RefShape) -> String {
    let from_part = match shape {
        RefShape::FixedIndex(elems) => format!("{}[{}]", from, elems.join(",")),
        _ => from.to_string(),
    };
    format!(
        "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}",
        from_part, to
    )
}

/// Generate absolute loop score variables for all loops.
///
/// Emits one `$⁚ltm⁚loop_score⁚{id}` entry per loop, returning the variable
/// name plus the *dimension-shaped* typed [`LtmEquation`] it should carry:
///
/// - **Scalar loops** (`dimensions` empty): `Equation::Scalar`, the product of
///   the loop's link-score references.
/// - **Dimensioned loops with empty `slot_links`** (the Bare-A2A fast path):
///   `Equation::ApplyToAll` over the loop's dimensions -- the compact form,
///   correct because every link resolves to a Bare A2A link-score variable
///   that the A2A expansion evaluates per element.
/// - **Dimensioned loops with `slot_links`** (per-element circuits whose link
///   scores only exist as per-element names -- the enumerator's A2A-collapse
///   on per-element-equation models, and dimensioned pinned loops, GH #653):
///   `Equation::Arrayed` over the loop's dimensions, one slot equation per
///   element tuple of the dimension space (row-major declared order, from
///   `dm_dims`). Slots without a backing circuit score a constant 0.
///
/// Relative loop scores are not emitted here: the per-partition
/// `rel_loop_score` was O(P²) text per partition and dominated compile memory
/// on dense models (see `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`).
/// The normalization happens post-simulation in
/// [`crate::ltm_post::compute_rel_loop_scores`].
/// Per-link reference overrides for the loop-score equation, keyed by
/// `(loop_id, link_index)` -> pre-quoted reference text (e.g. a per-exit-port
/// pathway-selection alias for a module link, PR #684). When a link has an
/// override, `loop_link_score_ref` uses it verbatim instead of resolving the
/// link's `(from, to)` to an emitted link-score name. The index is into
/// `loop_item.links` (the whole-loop cycle); the per-slot `slot_links` path
/// does not consult overrides (its module-link case degenerates to the scalar
/// one, which the whole-loop override already covers).
pub(crate) type LoopLinkOverrides = HashMap<(String, usize), String>;

pub(crate) fn generate_loop_score_variables(
    loops: &[Loop],
    emitted_link_score_names: &HashSet<String>,
    dm_dims: &[datamodel::Dimension],
    overrides: &LoopLinkOverrides,
) -> Vec<(String, LtmEquation)> {
    let mut loop_vars = Vec::with_capacity(loops.len());

    // Loop-score tracing is a benchmarking/diagnostic aid compiled in only
    // under `--features ltm_bench`; the default build's `LoopScoreTrace` is a
    // zero-sized no-op whose methods optimize away entirely (no env lookup,
    // no /proc/self/status read, no byte counter, no eprintln!). See
    // [`loop_score_trace`].
    let mut trace = loop_score_trace::LoopScoreTrace::start(loops.len());

    for (i, loop_item) in loops.iter().enumerate() {
        let var_name = format!("$⁚ltm⁚loop_score⁚{}", loop_item.id);
        let Some(equation) = generate_dimensioned_loop_score_equation(
            loop_item,
            emitted_link_score_names,
            dm_dims,
            overrides,
        ) else {
            continue;
        };
        trace.record(i + 1, &equation);
        loop_vars.push((var_name, equation));
    }

    trace.done(loops.len());

    loop_vars
}

/// Loop-score equation-text-growth / RSS tracing for the LTM compile
/// benchmark, gated entirely behind the `ltm_bench` cargo feature.
///
/// The default build's [`LoopScoreTrace`] is a zero-sized no-op so production
/// carries no `/proc/self/status` read, no env lookup, no byte counter, and no
/// `eprintln!` dead code (the historical `LTM_BENCH_TRACE` runtime env check
/// shipped all of that in every build; GH #464). The feature build re-creates
/// the instrumentation: it logs cumulative loop-score equation bytes and RSS
/// at power-of-two sample points plus every 10,000 loops. Enable it with
/// `cargo run --release --example ltm_full_bench --features ltm_bench -- <mdl>`.
mod loop_score_trace {
    #[cfg(feature = "ltm_bench")]
    pub(super) struct LoopScoreTrace {
        loop_score_bytes: u64,
    }

    #[cfg(feature = "ltm_bench")]
    impl LoopScoreTrace {
        pub(super) fn start(loop_count: usize) -> Self {
            eprintln!(
                "[ltm-trace] generate_loop_score_variables start loops={} rss_mib={:.1}",
                loop_count,
                read_rss_mib().unwrap_or(0.0),
            );
            LoopScoreTrace {
                loop_score_bytes: 0,
            }
        }

        /// Accumulate `equation`'s text bytes and, on a sample point, log the
        /// running total alongside RSS. `n` is the 1-based loop index.
        pub(super) fn record(&mut self, n: usize, equation: &crate::db::LtmEquation) {
            self.loop_score_bytes += equation_text_len(equation) as u64;
            if should_trace(n) {
                eprintln!(
                    "[ltm-trace] pass=loop_score i={} cum_loop_bytes={} rss_mib={:.1}",
                    n,
                    self.loop_score_bytes,
                    read_rss_mib().unwrap_or(0.0),
                );
            }
        }

        pub(super) fn done(&self, loop_count: usize) {
            eprintln!(
                "[ltm-trace] generate_loop_score_variables done loops={} loop_bytes={} \
                 rss_mib={:.1}",
                loop_count,
                self.loop_score_bytes,
                read_rss_mib().unwrap_or(0.0),
            );
        }
    }

    /// Total equation-text bytes of an `LtmEquation` (the diagnostic text).
    #[cfg(feature = "ltm_bench")]
    fn equation_text_len(equation: &crate::db::LtmEquation) -> usize {
        use crate::db::LtmEquation;
        match equation {
            LtmEquation::Scalar(arm) | LtmEquation::ApplyToAll(_, arm) => arm.text.len(),
            LtmEquation::Arrayed {
                elements, default, ..
            } => {
                elements
                    .iter()
                    .map(|(_, arm)| arm.text.len())
                    .sum::<usize>()
                    + default.as_ref().map(|a| a.text.len()).unwrap_or(0)
            }
        }
    }

    /// Decide whether iteration `n` (1-based) should emit a trace line.
    ///
    /// We want early iterations densely (so we see the scaling curve even if we
    /// OOM before completing the first 10_000 loops on a dense partition) and
    /// later iterations sparsely (so we don't spam the log for millions of
    /// loops). Rule: log on every power of two up to and including 8192, then
    /// every 10_000 after that. Powers of two give ~14 lines of early-curve
    /// data; 10_000 cadence gives steady-state measurements during long runs.
    #[cfg(feature = "ltm_bench")]
    fn should_trace(n: usize) -> bool {
        if n == 0 {
            return false;
        }
        if n <= 8192 {
            n.is_power_of_two()
        } else {
            n.is_multiple_of(10_000) || n.is_power_of_two()
        }
    }

    /// Resident-set size in MiB, or `None` if the kernel does not expose
    /// `/proc/self/status` (e.g. non-Linux or wasm builds). An unavailable
    /// reading degrades to a zero in the log rather than failing.
    #[cfg(all(feature = "ltm_bench", target_os = "linux"))]
    fn read_rss_mib() -> Option<f64> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest.trim().trim_end_matches(" kB").trim().parse().ok()?;
                return Some(kb as f64 / 1024.0);
            }
        }
        None
    }

    #[cfg(all(feature = "ltm_bench", not(target_os = "linux")))]
    fn read_rss_mib() -> Option<f64> {
        None
    }

    /// The default build's no-op trace: a zero-sized type whose methods are
    /// empty, so the optimizer removes every call site (and the whole tracing
    /// apparatus is `#[cfg]`-ed out of compilation, not merely dead-code
    /// eliminated).
    #[cfg(not(feature = "ltm_bench"))]
    pub(super) struct LoopScoreTrace;

    #[cfg(not(feature = "ltm_bench"))]
    impl LoopScoreTrace {
        #[inline(always)]
        pub(super) fn start(_loop_count: usize) -> Self {
            LoopScoreTrace
        }
        #[inline(always)]
        pub(super) fn record(&mut self, _n: usize, _equation: &crate::db::LtmEquation) {}
        #[inline(always)]
        pub(super) fn done(&self, _loop_count: usize) {}
    }
}

/// `true` when every variable-level link of `loop_item` resolves to an
/// emitted Bare A2A link-score name (`{from}→{to}` with subscripts stripped
/// from both ends).
///
/// When this holds, the compact `Equation::ApplyToAll` form is correct: each
/// element slot of the loop score reads its own slot of each (A2A) link-score
/// variable diagonally. When any link only exists as a per-element name
/// (FixedIndex `from[e]→to`, per-target-element `from→to[e]`), the
/// ApplyToAll form would reference one arbitrary element's variable for every
/// slot, so the per-slot `Arrayed` form (from `slot_links`) is required.
fn all_links_resolve_bare(loop_item: &Loop, emitted: &HashSet<String>) -> bool {
    loop_item.links.iter().all(|link| {
        let bare = link_score_var_name(
            strip_subscript(link.from.as_str()),
            strip_subscript(link.to.as_str()),
            &RefShape::Bare,
        );
        emitted.contains(&bare)
    })
}

/// Build the dimension-shaped typed [`LtmEquation`] for one loop's score
/// variable. See [`generate_loop_score_variables`] for the three cases.
fn generate_dimensioned_loop_score_equation(
    loop_item: &Loop,
    emitted: &HashSet<String>,
    dm_dims: &[datamodel::Dimension],
    overrides: &LoopLinkOverrides,
) -> Option<LtmEquation> {
    if loop_item.dimensions.is_empty() {
        return try_generate_loop_score_equation(loop_item, emitted, overrides)
            .map(LtmEquation::scalar);
    }
    // Prefer the compact ApplyToAll form whenever it is correct (every link
    // resolves through a Bare A2A name), regardless of whether per-slot
    // circuit info is available. Otherwise use the per-slot Arrayed form.
    // A dimensioned loop with per-element-only link scores AND no slot_links
    // (a builder that predates slot capture) keeps the legacy ApplyToAll
    // emission, but only when every link still resolves to an emitted
    // link-score name. Otherwise the loop is dropped before fragment
    // compilation can synthesize a missing-dependency zero stub.
    if loop_item.slot_links.is_empty() || all_links_resolve_bare(loop_item, emitted) {
        return try_generate_loop_score_equation(loop_item, emitted, overrides)
            .map(|text| LtmEquation::apply_to_all(loop_item.dimensions.clone(), text));
    }

    // Per-slot equations: enumerate the loop's full dimension element space
    // (row-major declared order) so the Arrayed equation is total over the
    // dimension space; element tuples without a backing circuit score 0.
    //
    // Fall back to the slot_links' own keys when `dm_dims` doesn't cover the
    // loop's declared dimensions (a mid-edit inconsistency where the cached
    // loop structure outran a still-being-edited dimension list) -- sparse
    // but deterministic, matching `partition_for_loop`'s fallback.
    let tuples = crate::ltm::loop_dimension_element_tuples(&loop_item.dimensions, dm_dims);
    let slot_keys: Vec<String> = if tuples.is_empty() {
        loop_item
            .slot_links
            .iter()
            .map(|(t, _)| t.clone())
            .collect()
    } else {
        tuples
    };
    let by_tuple: HashMap<&str, &[crate::ltm::Link]> = loop_item
        .slot_links
        .iter()
        .map(|(t, l)| (t.as_str(), l.as_slice()))
        .collect();
    let mut elements: Vec<(String, String)> = Vec::with_capacity(slot_keys.len());
    for tuple in &slot_keys {
        let text = match by_tuple.get(tuple.as_str()) {
            Some(links) => generate_link_product(links, emitted, None)?,
            None => "0".to_string(),
        };
        elements.push((tuple.clone(), text));
    }
    Some(LtmEquation::arrayed(
        loop_item.dimensions.clone(),
        elements,
        None,
        false,
    ))
}

/// The live source's declared dimension names (canonical, in declaration
/// order) -- looked up from the model's variable map; empty for a scalar
/// source or one not in the map (an implicit SMOOTH/DELAY var, scalar by
/// construction). Used to build the GH #511 [`IteratedDimCtx`].
fn source_dim_names_for(
    from: &Ident<Canonical>,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
) -> Vec<String> {
    all_vars
        .get(from)
        .and_then(|v| v.get_dimensions())
        .map(|dims| dims.iter().map(|d| d.name().to_string()).collect())
        .unwrap_or_default()
}

/// The target equation's iterated dimension names (canonical), or `&[]`
/// when the target is scalar. Used to build the GH #511 [`IteratedDimCtx`].
fn target_iterated_dim_names_canonical(to_var: &Variable) -> Vec<String> {
    use crate::ast::Ast;
    match to_var.ast() {
        Some(Ast::ApplyToAll(dims, _)) | Some(Ast::Arrayed(dims, _, _, _)) => {
            dims.iter().map(|d| d.name().to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// Generate the equation for a link score variable.
/// Exposed as `generate_link_score_equation_for_link` for use by tracked
/// functions in `db.rs`.
///
/// Returns a typed [`LtmEquation`] whose variant matches the *target*
/// variable's shape: scalar for a scalar target,
/// apply-to-all over `target_dims` for an arrayed target (so the
/// compiler expands the formula per element). `target_dims` uses the
/// target's datamodel dimension names; the link emission loop overwrites
/// them with the link-score-dimensions policy result, which is the same
/// list for every compatible-dimension edge.
///
/// `shape` selects which AST occurrences of `from` remain live in the
/// partial equation; non-matching occurrences (and every reference to
/// other deps) are wrapped in `PREVIOUS()`. `source_dim_elements` carries
/// the source variable's dimension element names (one inner vec per
/// dimension, in source-declared order, canonical lowercase) so that
/// literal index names like `[NYC]` can be classified as `FixedIndex`
/// rather than the conservative `DynamicIndex` fallback. `dim_ctx` is the
/// project's `DimensionsContext`, threaded into the GH #511 iterated-
/// dimension recognition for the mapped-dimension case (`Some` from the
/// salsa-tracked caller; `None` is harmless -- by-name recognition still
/// applies).
///
/// `dep_dims` carries the declared dimensions of the target's non-live
/// array deps (canonical name -> dims), threaded into the GH #526
/// other-dep correspondence check; `None` keeps the historical permissive
/// collapse for every dep (legacy / db-less callers).
///
/// Flow-to-stock links use a fixed structural formula and ignore `shape`,
/// `source_dim_elements`, `dim_ctx`, and `dep_dims`.
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
pub(crate) fn generate_link_score_equation_for_link(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
    dep_dims: Option<&HashMap<String, Vec<crate::dimensions::Dimension>>>,
    to_occurrences: &[OccurrenceSite],
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<LtmEquation, PartialEquationError> {
    generate_link_score_equation(
        from,
        to,
        shape,
        source_dim_elements,
        to_var,
        all_vars,
        dim_ctx,
        dep_dims,
        to_occurrences,
        freeze_helpers,
    )
}

/// Generate the equation for a link score variable.
///
/// Returns `Err([`PartialEquationError`])` when the target's equation text
/// cannot be parsed for the ceteris-paribus partial (GH #311); the
/// db-bearing caller turns this into a `Warning` and skips the variable.
/// The flow-to-stock branch uses a fixed structural formula with no parse,
/// so it is infallible and always returns `Ok`.
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn generate_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
    dep_dims: Option<&HashMap<String, Vec<crate::dimensions::Dimension>>>,
    to_occurrences: &[OccurrenceSite],
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<LtmEquation, PartialEquationError> {
    // Check if this is a stock-to-flow link
    let is_stock_to_flow = matches!(all_vars.get(from), Some(Variable::Stock { .. }))
        && matches!(to_var, Variable::Var { is_flow: true, .. });

    // Flow-to-stock link: `to` is a stock and `from` is one of its flows.
    // Binding `flow_var` here -- rather than computing an `is_flow_to_stock`
    // bool and re-fetching the (proven-present) flow variable -- lets the
    // generator take a plain `&Variable`.
    if let Variable::Stock { .. } = to_var
        && let Some(flow_var @ Variable::Var { is_flow: true, .. }) = all_vars.get(from)
    {
        // Flow-to-stock uses a fixed structural formula -- no AST parse,
        // so neither `shape` nor `source_dim_elements` matter here. The
        // flow variable is passed in only for its declared dimensions, so
        // an arrayed flow can be referenced with an explicit subscript.
        Ok(generate_flow_to_stock_equation(
            from.as_str(),
            to.as_str(),
            flow_var,
            to_var,
        ))
    } else if is_stock_to_flow {
        // Use stock-to-flow formula
        let source_dim_names = source_dim_names_for(from, all_vars);
        generate_stock_to_flow_equation(
            from,
            to,
            shape,
            source_dim_elements,
            &source_dim_names,
            to_var,
            dim_ctx,
            dep_dims,
            to_occurrences,
            freeze_helpers,
        )
    } else {
        // Use standard auxiliary-to-auxiliary formula
        let source_dim_names = source_dim_names_for(from, all_vars);
        generate_auxiliary_to_auxiliary_equation(
            from,
            to,
            shape,
            source_dim_elements,
            &source_dim_names,
            to_var,
            dim_ctx,
            dep_dims,
            to_occurrences,
            freeze_helpers,
        )
    }
}

/// Wrap a per-input partial equation in the standard LTM link-score guard
/// form: zero at the initial timestep, zero when either Δtarget or
/// Δsource is zero, and otherwise `|Δpartial/Δtarget| * sign(Δpartial/Δsource)`,
/// where `Δpartial` is `partial - PREVIOUS(target)` (the partial measures
/// what the target *would* be with `from` live and everything else
/// frozen). `target_ref` and `source_ref` are pre-formatted reference
/// expressions (already quoted or rendered as subscripts as the caller
/// requires).
fn link_score_guard_form(partial_eq: &str, target_ref: &str, source_ref: &str) -> String {
    // The changed-first numerator: `Δ_x z = z(x_t, w_{t-1}) - z_{t-1}`,
    // rendered as `(partial - PREVIOUS(target))`.
    let numerator = format!("(({partial_eq}) - PREVIOUS({target_ref}))");
    link_score_guard_form_with_numerator(&numerator, target_ref, source_ref)
}

/// The numerator-parameterized core of [`link_score_guard_form`], shared by
/// the changed-first form (numerator `(partial - PREVIOUS(target))`), the
/// changed-last form (numerator `(target - frozen)` -- the
/// [`shaped_guard_form_text`] fallback and
/// [`generate_scalar_feeder_to_agg_equation`]), and any future attribution
/// convention with the same guard structure.
fn link_score_guard_form_with_numerator(
    numerator: &str,
    target_ref: &str,
    source_ref: &str,
) -> String {
    // The link score is |Δ_x(z) / Δ(z)| * sign(Δ_x(z) / Δ(x)) (LTM ref §3.1).
    // Within the else branch the guard guarantees Δ(z) != 0 and Δ(x) != 0, so
    // the formula is emitted in the algebraically identical single-numerator
    // form
    //
    //   ABS(SAFEDIV(N, Δz, 0)) * SIGN(SAFEDIV(N, Δx, 0))
    //     == |N|/|Δz| * sign(N) * sign(Δx)
    //     == SAFEDIV(N, ABS(Δz), 0) * SIGN(Δx)
    //
    // so the numerator N -- which embeds the (potentially large) partial
    // equation -- appears ONCE instead of twice. This halves the equation
    // text, the helper-aux count for any helper-producing construct inside
    // the partial, and the per-step evaluation cost.
    let target_diff = format!("({target_ref} - PREVIOUS({target_ref}))");
    let source_diff = format!("({source_ref} - PREVIOUS({source_ref}))");
    format!(
        "if (TIME = INITIAL_TIME) then 0 \
         else if ({target_diff} = 0) OR ({source_diff} = 0) then 0 \
         else SAFEDIV({numerator}, ABS({target_diff}), 0) * SIGN({source_diff})"
    )
}

/// The datamodel-cased dimension names of `var`'s equation, when `var`
/// is arrayed; `None` for scalar variables and modules. Link-score
/// equations are tagged with these so `parse_ltm_equation` resolves the
/// dimensions by exact-name match against the project's datamodel.
fn target_equation_dims(var: &Variable) -> Option<Vec<String>> {
    let eqn = match var {
        Variable::Stock { eqn, .. } | Variable::Var { eqn, .. } => eqn.as_ref()?,
        Variable::Module { .. } => return None,
    };
    match eqn {
        Equation::Scalar(_) => None,
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, _, _, _) => {
            (!dims.is_empty()).then(|| dims.clone())
        }
    }
}

/// Build the link-score [`Equation`] for a target with the given guard-form
/// equation `text`: `Equation::Scalar` for a scalar target,
/// `Equation::ApplyToAll(target_dims, text)` for an arrayed target.
fn link_score_equation_for_target(text: String, to_var: &Variable) -> LtmEquation {
    match target_equation_dims(to_var) {
        Some(dims) => LtmEquation::apply_to_all(dims, text),
        None => LtmEquation::scalar(text),
    }
}

/// The dimension names to tag an `Equation::Arrayed` link score with.
///
/// Prefers the datamodel-cased names off the target's `eqn` field (so a
/// directly-generated equation parses against the project's datamodel).
/// Falls back to the AST `Vec<Dimension>`'s canonical-cased names for the
/// (test-only) case where `to_var` was constructed without an `eqn`; in
/// production the emission loop's `retarget_ltm_equation_dims` overwrites
/// these with the link-score-dimensions policy result regardless.
fn arrayed_target_dim_names(
    to_var: &Variable,
    ast_dims: &[crate::dimensions::Dimension],
) -> Vec<String> {
    target_equation_dims(to_var)
        .unwrap_or_else(|| ast_dims.iter().map(|d| d.name().to_string()).collect())
}

/// Build the per-element-partial link-score [`Equation`] for an
/// `Ast::Arrayed` (per-element-equation) target.
///
/// For each `(element, expr)` slot in the target's per-element map, the
/// slot equation is the standard link-score guard form ([`link_score_guard_form`])
/// whose `{partial}` is `build_partial_equation_shaped` applied to *that
/// element's own equation text* with `live_source = from` and `live_shape =
/// shape`. So the cross-element partial derived from
/// `mp[NYC] = (pop[NYC] - pop[Boston]) * 0.01` keeps `pop[NYC]` live and
/// freezes `pop[Boston]` at PREVIOUS when this link score's shape is
/// `FixedIndex(["nyc"])`. An element whose equation does not reference
/// `from` with `shape` gets all its `from` references frozen, so that slot
/// evaluates to ~0 -- correct, because that source-element's influence on
/// that target-element flows through a *different* `(from[other], to)`
/// link-score variable (a different shape) and must not be double-counted
/// here.
///
/// `target_ref` is the pre-rendered self-reference expression (a bare name,
/// which within an `Equation::Arrayed` slot resolves element-wise). The
/// source reference is shape-aware and re-derived per slot: a `Bare` /
/// `FixedIndex` shape gives the same `from` / `from[elem]` for every slot,
/// but a `Wildcard` / `DynamicIndex` shape scalarizes *this slot's* live
/// source slice (`SUM(from[PREVIOUS(idx)])`), so a slot whose equation
/// doesn't reference `from` falls back to `SUM(from)` while a slot that
/// does gets the exact slice the partial isolated.
/// `target_ast_dims` are the target variable's AST dimensions, passed to
/// `classify_dependencies` so literal element-name subscripts (e.g.
/// `[Boston]`) are recognized as dimension references and excluded from the
/// dep set -- otherwise the PREVIOUS wrapper would treat the element name
/// as a variable reference and wrap it inside the subscript.
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn build_arrayed_link_score_equation(
    from: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    target_dim_names: Vec<String>,
    target_ast_dims: &[crate::dimensions::Dimension],
    per_elem: &HashMap<crate::common::CanonicalElementName, crate::ast::Expr2>,
    default_expr: Option<&crate::ast::Expr2>,
    apply_default_to_missing: bool,
    target_ref: &str,
    to_var: &Variable,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
    dep_dims: Option<&HashMap<String, Vec<crate::dimensions::Dimension>>>,
    to_occurrences: &[OccurrenceSite],
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<LtmEquation, PartialEquationError> {
    // The #511 iterated-dimension context for the per-slot partials: each
    // per-element slot can itself reference `from` by an iterated dimension
    // of the *target*'s dimension space. `target_ast_dims`' canonical names
    // are the iterated dims; for a literal-element slot (`growth[a,young] =
    // ...`) the recognition simply never fires (the indices are literals).
    let target_iterated_dims: Vec<String> = target_ast_dims
        .iter()
        .map(|d| d.name().to_string())
        .collect();
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dep_dims,
    };
    // A subscript like `source[m]` where `m` is an element of *`source`'s*
    // dimension `D3` (disjoint from the target's `D1 x D2`) is not filtered
    // by `classify_dependencies(..., target_ast_dims, ...)` -- `target_ast_dims`
    // covers `D1`/`D2`, not `D3` -- so `m` (and `D3` itself if spelled) leaks
    // into the dep set as a phantom variable and gets PREVIOUS-wrapped inside
    // the subscript (`source[PREVIOUS(m)]`). Strip the source's element names
    // and dimension names from the dep set so the partial sees only real deps.
    let source_dim_token_set: HashSet<&str> = source_dim_elements
        .iter()
        .flatten()
        .map(String::as_str)
        .chain(source_dim_names.iter().map(String::as_str))
        .collect();
    // Group the target's occurrences by slot ONCE, outside the per-element
    // closure below: this is the arrayed target whose N slots made the old
    // per-slot rescan quadratic (see `SlotOccurrences`).
    let slot_occurrences = SlotOccurrences::new(to_occurrences);
    // `slot` is the per-element occurrence-stream slot: `walk_all_in_expr`
    // numbers `Ast::Arrayed` slots in canonical element-key-sorted order (then
    // the default after the last element), so the wrap for element `slot`
    // consumes exactly that slot's occurrences. Both `slot as u16` conversions
    // below are in range by the LTM front door, which refuses a target needing
    // more slots than `db::ltm_ir::MAX_SITE_CHILDREN` can tell apart -- so this
    // model would have emitted no link score to reach here.
    // GH #977: a slot whose partial holds no live source reference scores a
    // structural zero, and an omitted slot already lowers to a single
    // constant-zero assign -- but ONLY when a missing slot means zero. Under
    // EXCEPT semantics it means "apply the default equation", so those targets
    // keep every arm. This is the only place the target's flag is in scope.
    let zero_slot_policy = if apply_default_to_missing {
        ZeroSlotPolicy::Materialize
    } else {
        ZeroSlotPolicy::OmitStructuralZero
    };
    let slot_equation = |expr: &crate::ast::Expr2,
                         gf_table_ref: Option<&str>,
                         slot: u16,
                         freeze_helpers: &mut Vec<ArrayFreezeHelper>|
     -> Result<Option<String>, PartialEquationError> {
        let elem_eqn = crate::patch::expr2_to_expr0(expr);
        // Per-element dependency set: walk *only this slot's* expression
        // (the union over all elements -- what `identifier_set` on the
        // whole `Ast::Arrayed` returns -- would over-freeze refs absent
        // from this slot). Pass the target's dimensions so literal
        // element-name subscripts of the *target*'s dims are filtered out;
        // strip the *source*'s dim/element names afterward (see above).
        let classified = crate::variable::classify_dependencies(
            &crate::ast::Ast::Scalar(expr.clone()),
            target_ast_dims,
            None,
        );
        let deps_e: HashSet<Ident<Canonical>> = classified
            .all
            .into_iter()
            .filter(|d| !source_dim_token_set.contains(d.as_str()))
            .collect();
        let occ = slot_occurrences.for_slot(slot);
        shaped_guard_form_text(
            &elem_eqn,
            &deps_e,
            from,
            shape,
            source_dim_elements,
            source_dim_names,
            Some(&iter_ctx),
            dim_ctx,
            target_ref,
            gf_table_ref,
            &occ,
            zero_slot_policy,
            freeze_helpers,
        )
    };

    // Visit slots in canonical element-key-sorted order (matching
    // `walk_all_in_expr`'s slot numbering), so both the emitted `Vec` and the
    // per-slot occurrence lookup are deterministic and aligned.
    // The implicit WITH-LOOKUP slot wraps (GH #910): each element's own table,
    // the shared variable-level table, or no wrap for a gf-less element.
    // Resolved once for the whole target -- see `WithLookupSlotRefs`.
    let slot_refs = WithLookupSlotRefs::new(to_var, target_ast_dims);

    let mut sorted_slots: Vec<(&crate::common::CanonicalElementName, &crate::ast::Expr2)> =
        per_elem.iter().collect();
    sorted_slots.sort_by(|a, b| a.0.cmp(b.0));

    // A slot the policy omitted is simply absent from `elements`; nothing is
    // pushed for it. That is deliberately NOT the same channel as an arm whose
    // generated text is EMPTY, which is still pushed and dropped later by
    // `LtmEquation::to_flow_ast` -- keeping the two distinct is what lets an
    // empty generated arm stay a symptom of a generator bug rather than a
    // second, silent way to zero a slot.
    let mut elements: Vec<(String, String)> = Vec::with_capacity(sorted_slots.len());
    for (slot, (elem, expr)) in sorted_slots.iter().enumerate() {
        let gf_table_ref = slot_refs.for_element(elem);
        if let Some(text) =
            slot_equation(expr, gf_table_ref.as_deref(), slot as u16, freeze_helpers)?
        {
            elements.push((elem.as_str().to_string(), text));
        }
    }

    // The default arm follows the same policy. When the policy is
    // `OmitStructuralZero` the target's `apply_default_to_missing` is false, so
    // `expand_arrayed_with_hoisting` never consults a default anyway; when it
    // is `Materialize` the arm is always built and the flatten is a no-op.
    let default_gf_table_ref = slot_refs.for_default();
    let default_slot = default_expr
        .map(|expr| {
            slot_equation(
                expr,
                default_gf_table_ref.as_deref(),
                sorted_slots.len() as u16,
                freeze_helpers,
            )
        })
        .transpose()?
        .flatten();

    Ok(LtmEquation::arrayed(
        target_dim_names,
        elements,
        default_slot,
        apply_default_to_missing,
    ))
}

/// The ceteris-paribus wrap's input AST for a Scalar/ApplyToAll target.
///
/// `Ast::Arrayed` targets are routed through
/// [`build_arrayed_link_score_equation`] before this is reached, so the
/// `Arrayed` AST arm here is dead in practice.
///
/// The lowered case ([`crate::patch::expr2_to_expr0`]) is the normal path and
/// involves no text at all -- that is the print->reparse deletion. The
/// `eqn`-TEXT fallbacks (both the `Ast::Arrayed` arm and the no-AST branch)
/// cover the degenerate case where the target failed to lower -- `ast()` is
/// `None`, or it's an `Ast::Arrayed` we didn't intercept -- but its datamodel
/// `eqn` is still a plain scalar string. Parsing that raw text gives the
/// link-score guard form *something* to differentiate, which is strictly more
/// useful than a `"0"` partial; the stock-to-flow path has always done this for
/// the same variable shape. A target with no usable scalar equation at all (a
/// stub, or an arrayed `eqn` we can't flatten here) falls through to `"0"` -- the
/// link score then degrades to the historical placeholder.
///
/// This fallback is the ONE remaining parse on this path, and it is the
/// legitimate kind: it reads USER-authored `datamodel` source text that no
/// compiled AST exists for, i.e. exactly the "unavoidable source-format
/// boundary" GH #965 carves out, not a re-parse of engine output. A genuine
/// failure there still surfaces as the loud `PartialEquationError::Parse` the
/// db-bearing caller turns into a warned skip.
fn scalar_or_a2a_target_expr(target_var: &Variable) -> Result<Expr0, PartialEquationError> {
    use crate::ast::Ast;
    if let Some(Ast::Scalar(expr) | Ast::ApplyToAll(_, expr)) = target_var.ast() {
        return Ok(crate::patch::expr2_to_expr0(expr));
    }
    let text = scalar_eqn_text_or_zero(target_var);
    match Expr0::new(&text, LexerType::Equation) {
        Ok(Some(expr)) => Ok(expr),
        _ => Err(PartialEquationError::new(&text)),
    }
}

/// The dependency set for a Scalar/A2A target's ceteris-paribus partial.
///
/// `identifier_set` is called with the target's own AST dimensions so the
/// target's dimension and element names are filtered out of the dep set --
/// with empty dims (the pre-GH#759 behavior) subscript-index identifiers
/// like the iterated dim `D1` in `matrix[D1, c1]` leaked in as phantom
/// deps, and the PREVIOUS wrapper froze them inside the subscript
/// (`matrix[PREVIOUS(d1), ..]`), dooming the fragment. The *source*'s
/// dimension and element names are then stripped as well, mirroring
/// [`build_arrayed_link_score_equation`]'s per-slot filtering: a literal of
/// a source-only dimension (`source[m]`, `m ∈ D3` disjoint from the
/// target's dims) is a dimension reference, not a causal dep.
///
/// Dimension/element names of a *co-source* dimension spelled in neither
/// the target's nor the source's dimension space can still leak in (this
/// function has no dims for them); [`wrap_index_non_matching_in_previous`]'s
/// element-name (GH #587) and dimension-name (GH #759) guards are the
/// backstop that keeps those verbatim.
///
/// Boundary: the source-token strip is name-based, so a real model variable
/// named identically to a source dimension ELEMENT, referenced OUTSIDE any
/// subscript, is over-stripped and left unfrozen (live) in the partial.
/// This is a pre-existing characteristic shared with
/// [`build_arrayed_link_score_equation`]'s identical per-slot strip and with
/// the engine's own dependency extraction (`classify_dependencies` filters
/// the same names against its dims) -- not a new failure class introduced
/// here.
fn scalar_or_a2a_target_deps(
    to_var: &Variable,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
) -> HashSet<Ident<Canonical>> {
    use crate::ast::Ast;
    let Some(ast) = to_var.ast() else {
        return HashSet::new();
    };
    let target_ast_dims: &[crate::dimensions::Dimension] = match ast {
        Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => dims,
        Ast::Scalar(_) => &[],
    };
    let source_dim_token_set: HashSet<&str> = source_dim_elements
        .iter()
        .flatten()
        .map(String::as_str)
        .chain(source_dim_names.iter().map(String::as_str))
        .collect();
    identifier_set(ast, target_ast_dims, None)
        .into_iter()
        .filter(|d| !source_dim_token_set.contains(d.as_str()))
        .collect()
}

/// The target's datamodel `eqn` text when it is a plain `Equation::Scalar`,
/// else `"0"`. See [`scalar_or_a2a_target_expr`] for why this
/// fallback exists (a variable that failed to lower).
fn scalar_eqn_text_or_zero(target_var: &Variable) -> String {
    match target_var {
        Variable::Stock {
            eqn: Some(Equation::Scalar(eq)),
            ..
        }
        | Variable::Var {
            eqn: Some(Equation::Scalar(eq)),
            ..
        } => eq.clone(),
        _ => "0".to_string(),
    }
}

/// Generate auxiliary-to-auxiliary link score equation
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn generate_auxiliary_to_auxiliary_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    to_var: &Variable,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
    dep_dims: Option<&HashMap<String, Vec<crate::dimensions::Dimension>>>,
    to_occurrences: &[OccurrenceSite],
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<LtmEquation, PartialEquationError> {
    use crate::ast::Ast;

    let to_q = quote_ident(to.as_str());

    // Per-element-equation (`Ast::Arrayed`) targets carry real per-element
    // partials: each slot's link score is the guard form built around that
    // element's own equation, so a cross-element aux keeps a meaningful
    // partial in every slot instead of a `"0"` placeholder (the legacy
    // `_ => "0"` fall-through produced the latter).
    if let Some(Ast::Arrayed(dims, per_elem, default_expr, apply_default)) = to_var.ast() {
        let target_dim_names = arrayed_target_dim_names(to_var, dims);
        return build_arrayed_link_score_equation(
            from,
            shape,
            source_dim_elements,
            source_dim_names,
            target_dim_names,
            dims,
            per_elem,
            default_expr.as_ref(),
            *apply_default,
            &to_q,
            to_var,
            dim_ctx,
            dep_dims,
            to_occurrences,
            freeze_helpers,
        );
    }

    // Get the equation text of the 'to' variable.  Prefer the AST when
    // available because the `eqn` field holds the *original* text (e.g.,
    // "SMTH1(x, 5)") while the AST holds the post-module-expansion form
    // (e.g., Var("$⁚s⁚0⁚smth1·output")).  Using the AST-derived text
    // ensures the identifiers in the equation match those in `deps`.
    let to_equation = scalar_or_a2a_target_expr(to_var)?;

    // Dependencies of the 'to' variable, with the target's and source's
    // dimension/element names filtered out (GH #759).
    let deps = scalar_or_a2a_target_deps(to_var, source_dim_elements, source_dim_names);

    // GH #511: an A2A target can reference `from` by one of the target's
    // iterated dimensions (`growth[Region,Age] = row_sum[Region] * c`).
    let target_iterated_dims = target_iterated_dim_names_canonical(to_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dep_dims,
    };
    // A scalar / `Ast::ApplyToAll` target is a single body -- slot 0 of the
    // occurrence stream.
    let slot_occurrences = SlotOccurrences::new(to_occurrences);
    let occ = slot_occurrences.for_slot(0);
    let Some(text) = shaped_guard_form_text(
        &to_equation,
        &deps,
        from,
        shape,
        source_dim_elements,
        source_dim_names,
        Some(&iter_ctx),
        dim_ctx,
        &to_q,
        // The implicit WITH-LOOKUP wrap for a tables-carrying target
        // (GH #910); `None` for an ordinary aux.
        with_lookup_table_ref(to_var).as_deref(),
        &occ,
        // This builds a whole variable's equation, not one slot of an arrayed
        // one, so a structural zero still has to be materialized: there is no
        // slot to leave absent, and dropping the variable would change the
        // emitted score set.
        ZeroSlotPolicy::Materialize,
        freeze_helpers,
    )?
    else {
        unreachable!("ZeroSlotPolicy::Materialize never omits an arm")
    };
    Ok(link_score_equation_for_target(text, to_var))
}

/// Choose the source-side reference for [`link_score_guard_form`].
///
/// For `Bare` / `FixedIndex` shapes this is [`shape_aware_source_ref`]
/// (a bare ident or a `from[elem]` subscript). For `Wildcard` /
/// `DynamicIndex` shapes -- the not-hoisted conservative-slice
/// (`SUM(pop[NYC,*])` inside a larger expr) and bare-dynamic-index
/// (`arr[idx]`, `arr[i+1]`) cases -- spelling the bare arrayed source in a
/// *scalar* link-score equation is a dimension error, so the fragment
/// fails to compile and the score is identically zero. Instead, reuse the
/// exact source slice the partial isolates (`arr[PREVIOUS(idx)]`,
/// `pop[NYC,*]`) wrapped in `SUM(...)`: `SUM` of a single element is the
/// identity, `SUM` of a slice is scalar, and the result feeds only the
/// SIGN factor and the `=0` zero-guard (both sign/zero-only), so using
/// `SUM` in place of the reducer's own algebra is harmless. If the
/// transform left no live reference (the source vanished from the
/// equation -- a parse failure is now reported as a `PartialEquationError`
/// before this is reached), fall back to `SUM(from)` -- still better than
/// a guaranteed dimension error.
fn source_ref_for_guard(
    from: &Ident<Canonical>,
    shape: &RefShape,
    live_ref: Option<&Expr0>,
    source_dim_names: &[String],
    source_dim_elements: &[Vec<String>],
) -> String {
    match shape {
        RefShape::Bare | RefShape::FixedIndex(_) => {
            shape_aware_source_ref(from.as_str(), shape, source_dim_names, source_dim_elements)
        }
        // `PerElement` never reaches the shaped per-(from, to, shape) path:
        // `emit_per_shape_link_scores` diverts it to the per-(row,
        // full-target-element) emitter (`emit_per_element_link_scores`),
        // whose equations are built by `generate_per_element_link_equation`
        // with an internal `FixedIndex` live shape. The `SUM(...)`-wrapped
        // live-slice fallback here is defensive only (sign/zero-guard-safe,
        // like the Wildcard/DynamicIndex conservative slices).
        RefShape::Wildcard | RefShape::DynamicIndex | RefShape::PerElement { .. } => match live_ref
        {
            Some(r) => format!("SUM({})", print_eqn(r)),
            None => format!("SUM({})", quote_ident(from.as_str())),
        },
    }
}

/// Render the source reference that drives the link-score's denominator
/// (the SIGN normalizer and the early-return zero-guard) for a `Bare` or
/// `FixedIndex` shape. The denominator must match the *live* source
/// reference left in `partial_eq` so SAFEDIV captures the same source the
/// partial isolates.
///
///   - `Bare` -> `from` (per-element under A2A; the partial keeps the
///     bare reference live, so per-element Δfrom is correct).
///   - `FixedIndex(elems)` -> `from[elems_joined]` rendered as a
///     subscript expression (NOT a quoted ident) so the LTM equation
///     parser interprets it as a per-element subscript matching the
///     `from[elem]` reference left live in `partial_eq`. Per-element-
///     target normalization must use Δfrom[elem], not Δfrom[r],
///     otherwise the cross-element sensitivity gets divided by the
///     wrong source delta and can flip sign or collapse to zero.
///
/// `Wildcard` / `DynamicIndex` shapes never reach this function for the
/// source-side guard: a bare arrayed `from` in a *scalar* link-score
/// equation is a dimension error (uncompilable fragment -> identically
/// zero score), so [`source_ref_for_guard`] reuses the partial's
/// isolated source slice wrapped in `SUM(...)` instead. (A *fully*
/// inlined reducer is hoisted into a `$⁚ltm⁚agg⁚{n}` node and normalized
/// by Δagg; the conservative-slice and bare-dynamic-index cases that
/// `enumerate_agg_nodes` does not hoist are what `source_ref_for_guard`
/// handles.)
fn shape_aware_source_ref(
    from: &str,
    shape: &RefShape,
    source_dim_names: &[String],
    source_dim_elements: &[Vec<String>],
) -> String {
    match shape {
        RefShape::FixedIndex(elems) if !elems.is_empty() => {
            // Subscript syntax, NOT quote_ident: a literal `pop[nyc]`
            // parses as a Subscript node (per-element reference), while
            // `"pop[nyc]"` would parse as a quoted ident referring to
            // a synthetic variable that doesn't exist.
            //
            // Each element is qualified with its positional dimension name
            // (`pop[region\u{B7}nyc]`) when it verifiably belongs to that
            // dimension, so the guard form's PREVIOUS-wrapped occurrence of
            // this reference resolves to a static slot (a direct LoadPrev)
            // instead of forcing a synthesized helper aux per occurrence.
            // Numeric elements (indexed dims) are already static; elements
            // that don't match their positional dimension fall back to the
            // bare form (defensive -- never change what the reference
            // resolves to).
            let qualified: Vec<String> = elems
                .iter()
                .enumerate()
                .map(|(i, elem)| {
                    if elem.parse::<u32>().is_ok() {
                        return elem.clone();
                    }
                    let in_positional_dim = source_dim_elements
                        .get(i)
                        .is_some_and(|dim_elems| dim_elems.iter().any(|e| e == elem));
                    match (in_positional_dim, source_dim_names.get(i)) {
                        (true, Some(dim_name)) => {
                            format!("{}\u{B7}{}", canonicalize(dim_name), elem)
                        }
                        _ => elem.clone(),
                    }
                })
                .collect();
            format!("{}[{}]", quote_ident(from), qualified.join(","))
        }
        _ => quote_ident(from),
    }
}

/// The `[Dim0,Dim1,...]` subscript suffix naming `var`'s declared
/// dimensions (datamodel casing, declaration order), or an empty string
/// when `var` is scalar. Built from the same `target_equation_dims`
/// the equation tag is derived from, so the subscript and the
/// `Equation::ApplyToAll` dimension list always agree.
fn dimension_subscript_suffix(var: &Variable) -> String {
    match target_equation_dims(var) {
        Some(dims) => format!("[{}]", dims.join(",")),
        None => String::new(),
    }
}

/// Generate flow-to-stock link score equation.
///
/// The structural inflow/outflow formula has no per-element equation
/// text -- the compiler applies it element-wise when the stock and flow
/// are arrayed -- so the result is `Equation::Scalar` for a scalar stock
/// and `Equation::ApplyToAll(stock_dims, _)` for an arrayed stock (the
/// shared formula evaluated per element).
///
/// For an arrayed stock every stock/flow reference is emitted with an
/// explicit dimension subscript (`stock[Dim]`, `flow[Dim]`) rather than a
/// bare arrayed name. A bare arrayed name nested inside
/// `PREVIOUS(PREVIOUS(...))` does not survive the apply-to-all expansion:
/// the inner `PREVIOUS(name)` is an *expression* argument, so
/// `builtins_visitor` routes it through a synthesized *scalar* helper aux
/// whose equation is `PREVIOUS(name, 0)` -- and a bare arrayed name has no
/// scalar meaning, so that helper fragment fails to compile and the LTM
/// fragment compiler silently stubs it to 0 (the score then collapses to
/// a wrong constant -- `1/9` for the canonical pop/growth model instead
/// of the isolated-loop invariant `1`). An explicit subscript keeps every
/// occurrence a scalar per-element access the helper aux can hold. Each
/// variable is subscripted by its *own* declared dimensions; a valid
/// arrayed inflow/outflow shares the stock's dimensions, so those names
/// are all bound by the `ApplyToAll` iteration. A scalar stock/flow has
/// no dimensions, so its references stay bare -- the pre-fix behavior.
///
/// NOTE: GH #541's engine-level fix (`make_temp_arg` now synthesizes an
/// arrayed helper for a bare arrayed reference) makes the bare form compile
/// too, so this generator-side subscripting is no longer load-bearing.
/// It is intentionally retained: the engine fix is a strict superset
/// (an already-subscripted reference stays on the unchanged scalar-helper
/// path), this output is pinned by dedicated tests, and re-baselining the
/// LTM equation text for every arrayed flow-to-stock link score across the
/// corpus would be a broad change with no behavioral benefit.
fn generate_flow_to_stock_equation(
    flow: &str,
    stock: &str,
    flow_var: &Variable,
    stock_var: &Variable,
) -> LtmEquation {
    // Check if this flow is an inflow or outflow
    let is_inflow = if let Variable::Stock { inflows, .. } = stock_var {
        inflows.iter().any(|f| f.as_str() == flow)
    } else {
        true // Default to inflow
    };

    let sign = if is_inflow { "" } else { "-" };

    // Reference an arrayed stock/flow by its own declared dimensions so
    // every occurrence is a scalar per-element access; see the function
    // doc for why a bare arrayed name breaks the nested-PREVIOUS terms.
    // For a scalar stock/flow the suffix is empty and the references stay
    // bare, exactly as before.
    let stock_ref = format!("{stock}{}", dimension_subscript_suffix(stock_var));
    let flow_ref = format!("{flow}{}", dimension_subscript_suffix(flow_var));

    // Per the corrected 2023 formula (Schoenberg et al., Eq. 3):
    //   LS(inflow -> S)  = |Delta(i) / (Delta(S_t) - Delta(S_{t-dt}))| * (+1)
    //   LS(outflow -> S) = |Delta(o) / (Delta(S_t) - Delta(S_{t-dt}))| * (-1)
    //
    // The polarity is structural (fixed +1/-1), not dynamic.  ABS ensures
    // the magnitude is always positive; the sign is applied outside.
    //
    // The numerator uses PREVIOUS values to align timing with the denominator.
    // At time t, the flow at t-1 (PREVIOUS(flow)) is what drove the stock change from t-1 to t.
    // We measure the change in that causal flow: flow(t-1) - flow(t-2).
    //
    // The `time_step` factor makes the score the dimensionally-correct
    // discretization of the continuous form `|di/dt / d^2S/dt^2|`
    // (Schoenberg et al. 2023, Eq. 6): the denominator below is the
    // second-order stock change `dt * (netflow(t-1) - netflow(t-2))`, which
    // already carries one `dt`; the raw flow delta in the numerator carries
    // none, so without this factor the score is `1/dt` too large and the
    // error compounds once per flow-to-stock link in a loop. The published
    // Eq. 3 omits `dt` because every worked example in the papers uses dt=1.
    let numerator =
        format!("(time_step * (PREVIOUS({flow_ref}) - PREVIOUS(PREVIOUS({flow_ref}))))");
    let denominator = format!(
        "(({stock_ref} - PREVIOUS({stock_ref})) - (PREVIOUS({stock_ref}) - PREVIOUS(PREVIOUS({stock_ref}))))"
    );

    // Return 0 for the first two timesteps when we don't have enough history for second-order differences
    let text = format!(
        "if \
            (TIME = INITIAL_TIME) OR (PREVIOUS(TIME, INITIAL_TIME) = INITIAL_TIME) \
            then 0 \
            else {sign}ABS(SAFEDIV({numerator}, {denominator}, 0))"
    );
    link_score_equation_for_target(text, stock_var)
}

/// Generate stock-to-flow link score equation.
///
/// Like the auxiliary-to-auxiliary path but the source is known to be a
/// stock. A per-element-equation (`Ast::Arrayed`) flow gets real
/// per-element partials via [`build_arrayed_link_score_equation`]; a
/// scalar or A2A flow yields `Equation::Scalar` / `Equation::ApplyToAll`
/// respectively.
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn generate_stock_to_flow_equation(
    stock: &Ident<Canonical>,
    flow: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    flow_var: &Variable,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
    dep_dims: Option<&HashMap<String, Vec<crate::dimensions::Dimension>>>,
    to_occurrences: &[OccurrenceSite],
    freeze_helpers: &mut Vec<ArrayFreezeHelper>,
) -> Result<LtmEquation, PartialEquationError> {
    // For stock-to-flow, we need to calculate how the stock influences the flow
    // This is similar to auxiliary-to-auxiliary but we know the 'from' is a stock
    use crate::ast::Ast;

    // The stock-to-flow guard form uses the flow name as the (element-wise
    // within an `Equation::Arrayed` slot) target reference.
    let target_ref = flow.as_str();

    if let Some(Ast::Arrayed(dims, per_elem, default_expr, apply_default)) = flow_var.ast() {
        let target_dim_names = arrayed_target_dim_names(flow_var, dims);
        return build_arrayed_link_score_equation(
            stock,
            shape,
            source_dim_elements,
            source_dim_names,
            target_dim_names,
            dims,
            per_elem,
            default_expr.as_ref(),
            *apply_default,
            target_ref,
            flow_var,
            dim_ctx,
            dep_dims,
            to_occurrences,
            freeze_helpers,
        );
    }

    // The flow's own equation AST.  Prefer the lowered AST when available
    // because it handles both Scalar and ApplyToAll (arrayed) equations, whereas
    // the raw `eqn` field only covers Scalar.  Without this, arrayed flows
    // fall through to "0" and produce a zero link score.
    let flow_equation = scalar_or_a2a_target_expr(flow_var)?;

    // Dependencies of the flow variable, with the flow's and stock's
    // dimension/element names filtered out (GH #759).
    let deps = scalar_or_a2a_target_deps(flow_var, source_dim_elements, source_dim_names);

    // GH #511: a flow can reference the stock by one of the flow's own
    // iterated dimensions, the same way an A2A aux can.
    let target_iterated_dims = target_iterated_dim_names_canonical(flow_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dep_dims,
    };
    // Link score formula from LTM paper: |Δxz/Δz| × sign(Δxz/Δx)
    // For stock-to-flow: x=stock, z=flow. The stock side respects
    // shape: a FixedIndex(elem) link score must normalize by
    // Δstock[elem], not the variable-level Δstock; a Wildcard /
    // DynamicIndex source slice is scalarized (`SUM(stock[PREVIOUS(idx)])`)
    // because bare arrayed `stock` in a scalar equation is a dimension
    // error -- see `source_ref_for_guard` (applied inside
    // `shaped_guard_form_text`, which also handles the GH #743
    // changed-last fallback for an unfreezable changed-first partial).
    let slot_occurrences = SlotOccurrences::new(to_occurrences);
    let occ = slot_occurrences.for_slot(0);
    let Some(text) = shaped_guard_form_text(
        &flow_equation,
        &deps,
        stock,
        shape,
        source_dim_elements,
        source_dim_names,
        Some(&iter_ctx),
        dim_ctx,
        target_ref,
        // A flow can be an implicit WITH-LOOKUP variable too (GH #910).
        with_lookup_table_ref(flow_var).as_deref(),
        &occ,
        // A whole variable's equation -- see the twin call in
        // `generate_link_score_equation_for_link`.
        ZeroSlotPolicy::Materialize,
        freeze_helpers,
    )?
    else {
        unreachable!("ZeroSlotPolicy::Materialize never omits an arm")
    };
    Ok(link_score_equation_for_target(text, flow_var))
}

/// Resolve the link-score variable name a downstream consumer (loop
/// score, pathway score, composite score) should reference for a single
/// `(from, to)` edge.
///
/// `emit_per_shape_link_scores` emits names per-shape based on what the
/// target's AST contains: `pop→share` (Bare), `pop[nyc]→share` (FixedIndex
/// via element-level `from` prefix), and so on. The downstream consumer
/// doesn't carry the access shape, so we resolve at equation-generation
/// time by trying candidate names in priority order against the set of
/// names actually emitted. (Reducer references no longer produce a
/// per-shape link score here -- a maximal inlined reducer is hoisted into
/// a `$⁚ltm⁚agg⁚{n}` node whose two halves carry their own canonical
/// names, and the conservative-slice case collapses onto the Bare name.)
///
/// `to` is always the *variable-level* target name (no subscript).
/// `from` may carry an element subscript (`"population[nyc]"`):
///   - For a per-source-element FixedIndex reference (e.g.
///     `migration_pressure[NYC] = (population[NYC] - population[Boston]) * 0.01`),
///     `emit_per_shape_link_scores` emits the bracketed-from name
///     `population[nyc]→migration_pressure`; we match that verbatim.
///   - For a diagonal A2A reference or a structural flow→stock edge
///     visited at a specific element, the emitted name uses the
///     variable-level from (`migration_in→population`, dimensioned over
///     the target's dims); we fall back to the stripped-from form.
///
/// `target_element` is the element the loop edge visits at the target
/// node (when known). It lets `find_fixed_index_emitted_name` prefer an
/// exact `{from}[{e}]→{to}` match over its alphabetical-first heuristic.
/// With `target_element = None` the resolver is byte-identical to its
/// pre-Phase-2 behavior.
///
/// Priority (when `from` is variable-level):
///
/// 1. `Bare` -- the canonical `{from}→{to}` form.
/// 2. `FixedIndex` -- a `{from}[...]→{to}` name; prefer the exact
///    `target_element` match, else the lexicographically first match.
///
/// Returns `None` when no emitted candidate matches. Loop-score generation
/// treats that as a drop condition: emitting a reference to a missing
/// synthetic link-score variable would let the fragment compiler insert a
/// zero dependency stub and silently remove one factor from the loop score.
fn try_resolve_link_score_name_for_loop(
    from: &str,
    to: &str,
    emitted: &HashSet<String>,
    target_element: Option<&str>,
) -> Option<String> {
    if from.contains('[') {
        // Bracketed-from edge. Try the FixedIndex-style name (bracket
        // kept) first, then the variable-level Bare name that an A2A or
        // structural flow→stock link score would carry.
        let verbatim = link_score_var_name(from, to, &RefShape::Bare);
        if emitted.contains(&verbatim) {
            return Some(verbatim);
        }
        let stripped = strip_subscript(from);
        let bare = link_score_var_name(stripped, to, &RefShape::Bare);
        if emitted.contains(&bare) {
            return Some(bare);
        }
        return None;
    }

    let bare = link_score_var_name(from, to, &RefShape::Bare);
    if emitted.contains(&bare) {
        return Some(bare);
    }
    if let Some(fixed) = find_fixed_index_emitted_name(from, to, emitted, target_element) {
        return Some(fixed);
    }
    None
}

/// Resolve a link-score variable name for pathway/composite consumers that
/// still intentionally rely on the fragment compiler's missing-dependency
/// fallback for unscoreable sub-model pathway edges. Loop-score generation
/// uses [`try_resolve_link_score_name_for_loop`] instead so missing names
/// drop the loop before compilation.
pub(crate) fn resolve_link_score_name_for_loop(
    from: &str,
    to: &str,
    emitted: &HashSet<String>,
    target_element: Option<&str>,
) -> String {
    if let Some(resolved) = try_resolve_link_score_name_for_loop(from, to, emitted, target_element)
    {
        return resolved;
    }
    link_score_var_name(from, to, &RefShape::Bare)
}

/// Scan `emitted` for a link-score variable name matching the FixedIndex
/// pattern `{prefix}{from}[...]→{to}` (no shape suffix).
///
/// When `target_element` is `Some(e)` and `{from}[{e}]→{to}` is in
/// `emitted`, return that exact match. Otherwise return the
/// lexicographically first match for determinism.
fn find_fixed_index_emitted_name(
    from: &str,
    to: &str,
    emitted: &HashSet<String>,
    target_element: Option<&str>,
) -> Option<String> {
    if let Some(e) = target_element {
        let exact = link_score_var_name(from, to, &RefShape::FixedIndex(vec![e.to_string()]));
        if emitted.contains(&exact) {
            return Some(exact);
        }
    }
    let prefix = format!("$\u{205A}ltm\u{205A}link_score\u{205A}{from}[");
    let suffix = format!("]\u{2192}{to}");
    let mut matches: Vec<&String> = emitted
        .iter()
        .filter(|n| {
            n.starts_with(&prefix) && n.ends_with(&suffix) && n.len() > prefix.len() + suffix.len()
        })
        .collect();
    matches.sort();
    matches.first().map(|s| (*s).clone())
}

/// Generate the equation for a loop score variable.
///
/// The loop score is the product of all link scores in the loop. The
/// per-element distinction for cross-dimensional edges (e.g.,
/// `pop[nyc]→total_pop`) lives in `link.from` itself; for everything
/// else, the access shape is implicit in which name was actually
/// emitted by `emit_per_shape_link_scores`.
///
/// A cross-element loop link carries an element subscript on `link.to`
/// (e.g. `migration_pressure[boston]`) when the target link-score
/// variable is A2A (dimensioned over the target's dims). In that case
/// the loop visits a single element of that A2A score, so the reference
/// is subscripted at the reference site: `"$⁚ltm⁚link_score⁚{from}→{to}"[e]`.
/// For pure-scalar and pure-A2A loops `link.to` is variable-level, so
/// the output is the unsubscripted product of quoted link-score names,
/// byte-identical to the pre-Phase-2 form.
///
/// `emitted_link_score_names` carries every link-score variable name the
/// caller has emitted so far. For each loop link we try the canonical
/// Bare name first (since `try_cross_dimensional_link_scores` and the
/// common Bare-AST case both produce that form) and fall back to a
/// FixedIndex per-element name when only that variant exists. A loop that
/// runs through an inlined reducer traverses the synthetic
/// `$⁚ltm⁚agg⁚{n}` node instead of a `(from, to)` reducer edge, so its
/// links are `from[d] → agg` and `agg → to[e]` -- each carrying a
/// canonical name that resolves directly. Without this resolution the
/// loop_score equation would multiply against a missing variable and the
/// fragment compiler would silently insert a stub dep, dropping the
/// link's contribution.
#[cfg(test)]
fn generate_loop_score_equation(
    loop_item: &Loop,
    emitted_link_score_names: &HashSet<String>,
    overrides: &LoopLinkOverrides,
) -> String {
    try_generate_loop_score_equation(loop_item, emitted_link_score_names, overrides)
        .unwrap_or_else(|| "0".to_string())
}

/// Checked loop-score equation generation used by synthetic-var emission.
/// `None` means at least one link in the cycle has no emitted link-score
/// variable, so emitting the loop score would compile through a missing-name
/// zero stub.
fn try_generate_loop_score_equation(
    loop_item: &Loop,
    emitted_link_score_names: &HashSet<String>,
    overrides: &LoopLinkOverrides,
) -> Option<String> {
    generate_link_product(
        &loop_item.links,
        emitted_link_score_names,
        Some((loop_item.id.as_str(), overrides)),
    )
}

/// The product-of-link-score-references text for one link cycle.
///
/// Shared by the whole-loop path ([`generate_loop_score_equation`]) and the
/// per-slot path (a dimensioned loop's `slot_links`, where each slot's
/// element-subscripted link cycle produces its own product).
fn generate_link_product(
    links: &[crate::ltm::Link],
    emitted_link_score_names: &HashSet<String>,
    loop_overrides: Option<(&str, &LoopLinkOverrides)>,
) -> Option<String> {
    let mut link_score_names = Vec::with_capacity(links.len());
    for (i, link) in links.iter().enumerate() {
        // A per-link override (PR #684: a module link's per-exit-port
        // pathway-selection alias) takes precedence over the link's
        // (from, to) name resolution. Only the whole-loop path supplies
        // an override context; the per-slot path passes `None`.
        if let Some((loop_id, overrides)) = loop_overrides
            && let Some(reference) = overrides.get(&(loop_id.to_string(), i))
        {
            link_score_names.push(reference.clone());
            continue;
        }
        link_score_names.push(loop_link_score_ref(link, emitted_link_score_names)?);
    }

    if link_score_names.is_empty() {
        Some("0".to_string())
    } else {
        Some(link_score_names.join(" * "))
    }
}

/// The reference text (already quoted, and subscripted if needed) for one
/// loop link inside a loop-score equation.
///
/// Three cases:
///
/// 1. The loop edge visits an element `e` of the target (`link.to` is
///    `to[e]`) AND a per-target-element *scalar* link score
///    `$⁚ltm⁚link_score⁚{from}→{to}[{e}]` was emitted: reference that
///    scalar variable *bare* -- the element is already in the name, so a
///    `[e]` subscript would be wrong (the variable is scalar, it has no
///    element axis to index). This covers both `try_scalar_to_arrayed_link_scores`
///    (scalar source -> arrayed target, `from` unsubscripted) and
///    `try_cross_dimensional_link_scores`'s partial-reduce arm
///    (arrayed-result reducer `matrix[d1,d2] → row_sum[d1]`, where
///    `link.from` is itself element-level and rides verbatim in the name).
///
/// 2. The loop edge visits an element `e` and the link score is a
///    *dimensioned* A2A variable (`$⁚ltm⁚link_score⁚{from}→{to}` with
///    `dimensions = [target_dims]`, from `emit_per_shape_link_scores`):
///    reference it subscripted-after-quote, `"$⁚ltm⁚link_score⁚{from}→{to}"[e]`.
///
/// 3. No visited element (pure-scalar / pure-A2A loops, or `link.to` is
///    variable-level): reference the resolved name bare.
///
/// Cases 1 and 2 are distinguished by which name `emit_per_shape_link_scores`
/// / `try_scalar_to_arrayed_link_scores` / `try_cross_dimensional_link_scores`
/// actually emitted: the element-in-name scalar variant takes priority
/// because that is the form a scalar->arrayed or arrayed-result-reducer edge
/// gets. A bracketed `link.from` (`"pop[nyc]"`) without a matching
/// element-in-name entry in `emitted` can only be a FixedIndex /
/// full-reduce cross-dimensional source, so it falls through to the
/// bracketed-from resolution in `resolve_link_score_name_for_loop`.
fn loop_link_score_ref(link: &crate::ltm::Link, emitted: &HashSet<String>) -> Option<String> {
    let (to_var_level, visited_element) = split_node_subscript(link.to.as_str());

    if let Some(elem) = visited_element {
        // Cases 1 / 1b: a per-target-element scalar link score. The name
        // shape is identical (`$⁚ltm⁚link_score⁚{from}→{to}[{e}]`) whether
        // `from` is a scalar source (case 1) or itself element-level
        // (case 1b -- an arrayed-result reducer edge `matrix[d1,d2] →
        // row_sum[d1]`); `link.from` is used verbatim either way.
        let per_elem = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}{}\u{2192}{}[{}]",
            link.from.as_str(),
            to_var_level,
            elem
        );
        if emitted.contains(&per_elem) {
            return Some(format!("\"{per_elem}\""));
        }
    }

    let name = try_resolve_link_score_name_for_loop(
        link.from.as_str(),
        to_var_level,
        emitted,
        visited_element,
    )?;
    // Double-quote the variable name so it can be parsed. Case 2: a
    // cross-element loop edge visits a single element of a dimensioned A2A
    // link score, so subscript the reference at that element. Case 3: no
    // element to pin.
    Some(match visited_element {
        Some(elem) => format!("\"{name}\"[{elem}]"),
        None => format!("\"{name}\""),
    })
}

/// Classification of array-reducing builtins for cross-dimensional link score
/// generation. Defined once in [`crate::ltm_agg`] alongside the single
/// reducer-recognition table; re-exported here so existing references compile.
pub(crate) use crate::ltm_agg::ReducerKind;

/// Collect element names from a dimension as owned strings.
///
/// For `Dimension::Named`, returns the canonical element names.
/// For `Dimension::Indexed`, returns one-based index strings ("1", "2", ...).
/// The engine uses 1-based indexing for indexed dimensions (see
/// `dimensions.rs` `SubscriptIterator` which formats as `elem + 1`).
pub(crate) fn dimension_element_names(dim: &crate::dimensions::Dimension) -> Vec<String> {
    match dim {
        crate::dimensions::Dimension::Named(_, named) => named
            .elements
            .iter()
            .map(|e| e.as_str().to_string())
            .collect(),
        crate::dimensions::Dimension::Indexed(_, size) => {
            (1..=*size).map(|i| i.to_string()).collect()
        }
    }
}

/// Qualify each part of a comma-joined element tuple with its dimension's
/// name, position-matched against `dims`: `"nyc,adult"` over `[Region, Age]`
/// becomes `"region·nyc,age·adult"`. For use in generated *equation text*
/// (link-score variable names keep the bare form).
///
/// A bare element name in equation text is ambiguous -- XMILE allows element
/// names to shadow variable names -- so `PREVIOUS(source[nyc])` cannot be
/// statically resolved at parse time and forces a synthesized helper aux per
/// occurrence (one extra variable, result slot, and per-step copy each). The
/// qualified `dimension·element` form folds to a constant during Expr1
/// lowering (`constify_dimensions`), so `PREVIOUS(source[region·nyc])`
/// compiles to a direct LoadPrev at the element's slot. On large arrayed
/// models the difference is decisive: C-LEARN's LTM instrumentation needs
/// ~140k helper slots with bare elements, far past the bytecode's 65,536-slot
/// limit.
///
/// Indexed-dimension parts (numeric subscripts) are already static and pass
/// through unchanged. A part that doesn't match its positional dimension (or
/// a tuple whose arity doesn't match `dims`) falls back to the bare form --
/// defensive: never produce a reference that resolves differently than the
/// bare original would.
pub(crate) fn qualify_element_csv(
    element_csv: &str,
    dims: &[crate::dimensions::Dimension],
) -> String {
    let parts: Vec<&str> = element_csv.split(',').collect();
    if parts.len() != dims.len() {
        return element_csv.to_string();
    }
    let qualified: Vec<String> = parts
        .iter()
        .zip(dims)
        .map(|(part, dim)| match dim {
            crate::dimensions::Dimension::Named(dim_name, named) => {
                let canonical_part = canonicalize(part);
                let elem = crate::common::CanonicalElementName::from_raw(&canonical_part);
                if named.indexed_elements.contains_key(&elem) {
                    format!("{}\u{B7}{}", dim_name.as_str(), canonical_part)
                } else {
                    part.to_string()
                }
            }
            // Numeric subscripts over indexed dims are already static.
            crate::dimensions::Dimension::Indexed(_, _) => part.to_string(),
        })
        .collect();
    qualified.join(",")
}

/// The result of [`classify_reducer`]: which array reducer the target's
/// equation applies to the source, plus the two pieces of context the
/// per-element link-score generators need to build a correct partial.
/// `Eq` is deliberately absent: [`Expr0`] carries an `f64` on `Const`, so it is
/// `PartialEq` only. No consumer needs total equality here.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct ClassifiedReducer {
    pub kind: ReducerKind,
    /// Uppercase function name (e.g. "SUM", "MIN").
    pub name: &'static str,
    /// Whether the reducer call is the target's entire top-level expression.
    /// `false` means arithmetic AROUND the reducer (`2 * SUM(...)`); it says
    /// nothing about the reducer's argument, which may itself apply a
    /// coefficient to the source (`SUM(pop[*] * scale)`) -- that is what
    /// `body_text` is for (GH #744).
    pub is_bare: bool,
    /// The reducer's array argument (its "body") -- e.g. the AST of
    /// `pop[*] * (1 - weight[*])` for `SUM(pop[*] * (1 - weight[*]))`, lowered
    /// straight from the target's `Expr2` by [`crate::patch::expr2_to_expr0`].
    /// It used to be that AST's printed TEXT, which every consumer immediately
    /// parsed back -- a parse of our own print of a tree we already held.
    pub body: Expr0,
}

/// Examine the target variable's Expr2 AST to find the array-reducing function
/// applied to the source variable and classify it.
///
/// Walks the Expr2 tree looking for `Expr2::App(builtin, ...)` nodes where
/// the builtin is an array reducer and the argument references the source
/// variable (identified by canonical name). Returns the [`ClassifiedReducer`]:
/// the `ReducerKind`, the uppercase function name (e.g., "SUM", "MIN"),
/// whether the reducer is the top-level expression (`is_bare`), and the
/// reducer argument's AST (`body`).
///
/// When `is_bare` is false, the reducer is nested inside other arithmetic
/// (e.g., `2 * SUM(population[*])`). Callers should fall back to the
/// delta-ratio approach for nested reducers, because the algebraic shortcut
/// ignores the surrounding arithmetic and produces wrong link scores.
/// Arithmetic INSIDE the reducer argument is a separate concern: the linear
/// shortcut is exact only for a bare-source body, so callers supply the
/// generators a [`ReducerBodyCtx`] built from `body` and the body-aware
/// partial handles non-unit coefficients (GH #744).
///
/// Returns `None` if no reducing builtin is found for the given source.
pub(crate) fn classify_reducer(
    target_var: &Variable,
    source_ident: &str,
) -> Option<ClassifiedReducer> {
    use crate::ast::Ast;

    let ast = target_var.ast()?;
    let expr = match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => expr,
        // For arrayed targets with per-element equations, check the default
        // expression if available.
        Ast::Arrayed(_, _, default_expr, _) => default_expr.as_ref()?,
    };

    classify_reducer_in_expr(expr, source_ident, true)
}

/// Recursively search an Expr2 tree for a reducing builtin applied to
/// the source variable.
///
/// `is_top_level` tracks whether we are still at the root of the expression
/// tree. When `true` and the reducer is found at this node, `is_bare` in the
/// result is `true`. Once we recurse into sub-expressions (Op1, Op2, If,
/// non-reducer App arguments), `is_top_level` becomes `false` so any reducer
/// found deeper is correctly flagged as nested.
fn classify_reducer_in_expr(
    expr: &crate::ast::Expr2,
    source_ident: &str,
    is_top_level: bool,
) -> Option<ClassifiedReducer> {
    use crate::ast::Expr2;

    match expr {
        Expr2::App(builtin, _, _) => {
            classify_reducer_in_builtin(builtin, source_ident, is_top_level)
        }
        Expr2::Op1(_, inner, _, _) => classify_reducer_in_expr(inner, source_ident, false),
        Expr2::Op2(_, lhs, rhs, _, _) => classify_reducer_in_expr(lhs, source_ident, false)
            .or_else(|| classify_reducer_in_expr(rhs, source_ident, false)),
        Expr2::If(cond, then_e, else_e, _, _) => {
            classify_reducer_in_expr(cond, source_ident, false)
                .or_else(|| classify_reducer_in_expr(then_e, source_ident, false))
                .or_else(|| classify_reducer_in_expr(else_e, source_ident, false))
        }
        Expr2::Var(..) | Expr2::Const(..) | Expr2::Subscript(..) => None,
    }
}

/// [`classify_reducer_in_expr`]'s `App` arm, reachable directly from a
/// `BuiltinFn` the caller already holds.
///
/// This is how the link-score emitters read the reducer kind, name and body
/// off [`crate::ltm_agg::AggNode::reducer`] (GH #983): an aggregate node's
/// equation IS one reducer call, so `is_top_level = true` reproduces exactly
/// what walking a reconstructed `Ast::Scalar(App(..))` used to produce --
/// including the nested-reducer fallback below, which fires when the outer
/// reducer's array argument does not itself reference `source_ident`.
pub(crate) fn classify_reducer_in_builtin(
    builtin: &crate::builtins::BuiltinFn<crate::ast::Expr2>,
    source_ident: &str,
    is_top_level: bool,
) -> Option<ClassifiedReducer> {
    // Check if this builtin is a reducer whose argument references the
    // source variable.
    if let Some((kind, name, body)) = classify_builtin_if_references_source(builtin, source_ident) {
        return Some(ClassifiedReducer {
            kind,
            name,
            is_bare: is_top_level,
            body,
        });
    }
    // Even if this particular App node isn't the reducer we want, recurse
    // into its arguments to find nested reducers. Any reducer found inside a
    // non-reducer App is nested.
    let mut result = None;
    builtin.for_each_expr_ref(|sub_expr| {
        if result.is_none() {
            result = classify_reducer_in_expr(sub_expr, source_ident, false);
        }
    });
    result
}

/// If `builtin` is a recognized array reducer (per
/// [`crate::ltm_agg::reducer_kind`]) whose array argument references the source
/// variable, return its `(ReducerKind, uppercase function name, body text)`,
/// where the body text is the array argument's canonical printed form.
///
/// For every recognized reducer the array argument is the *first* expression
/// argument (`SUM(arr)`, `MEAN(arr)`, `MIN(arr)`, `MAX(arr)`, `STDDEV(arr)`,
/// `RANK(arr, dir)`, `SIZE(arr)`), so we check exactly that one. Multi-argument
/// `MEAN` and 2-argument `MIN`/`MAX` are scalar element-wise operations, not
/// reducers, and `reducer_kind` excludes them -- `None` here is the *correct*
/// answer, not a fallback for an impossible case. A target whose equation is
/// e.g. `result = MEAN(pop[NYC], other[Boston])` does reach [`classify_reducer`]
/// (via `try_cross_dimensional_link_scores`); with `None` here it falls through
/// to per-shape link scoring, which reads the `FixedIndex` site from the
/// classification IR and emits exactly the `pop[nyc] → result` link score the
/// equation has -- not the full-reduce-over-`pop` per-element scores the old
/// hand-rolled `Mean(any arity)` arm produced (including a spurious
/// `pop[boston] → result`).
fn classify_builtin_if_references_source(
    builtin: &crate::builtins::BuiltinFn<crate::ast::Expr2>,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str, Expr0)> {
    use crate::builtins::BuiltinFn;

    let kind = crate::ltm_agg::reducer_kind(builtin)?;

    // The recognized-reducer set is exactly `SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`/
    // `RANK`/`SIZE`, and in each the reduced array is the first argument.
    // (`for_each_expr_ref` can't be used here -- it doesn't tie the yielded
    // reference's lifetime to the borrow of `builtin`.)
    let (array_arg, upper): (&crate::ast::Expr2, &'static str) = match builtin {
        BuiltinFn::Sum(arg) => (arg, "SUM"),
        BuiltinFn::Mean(args) => (args.first()?, "MEAN"),
        BuiltinFn::Min(arg, _) => (arg, "MIN"),
        BuiltinFn::Max(arg, _) => (arg, "MAX"),
        BuiltinFn::Stddev(arg) => (arg, "STDDEV"),
        BuiltinFn::Rank(arg, _) => (arg, "RANK"),
        BuiltinFn::Size(arg) => (arg, "SIZE"),
        other => unreachable!(
            "reducer_kind admitted a non-reducer builtin: {}",
            other.name()
        ),
    };

    let canonical_source = canonicalize(source_ident);
    if !expr_references_var(array_arg, canonical_source.as_ref()) {
        return None;
    }
    Some((kind, upper, crate::patch::expr2_to_expr0(array_arg)))
}

/// Check if an Expr2 references a variable with the given canonical name,
/// either directly (Var) or via subscript (Subscript).
fn expr_references_var(expr: &crate::ast::Expr2, canonical_name: &str) -> bool {
    use crate::ast::Expr2;

    match expr {
        Expr2::Var(ident, _, _) => ident.as_str() == canonical_name,
        Expr2::Subscript(ident, _, _, _) => ident.as_str() == canonical_name,
        Expr2::App(builtin, _, _) => {
            let mut found = false;
            builtin.for_each_expr_ref(|sub_expr| {
                if !found {
                    found = expr_references_var(sub_expr, canonical_name);
                }
            });
            found
        }
        Expr2::Op1(_, inner, _, _) => expr_references_var(inner, canonical_name),
        Expr2::Op2(_, lhs, rhs, _, _) => {
            expr_references_var(lhs, canonical_name) || expr_references_var(rhs, canonical_name)
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            expr_references_var(cond, canonical_name)
                || expr_references_var(then_e, canonical_name)
                || expr_references_var(else_e, canonical_name)
        }
        Expr2::Const(..) => false,
    }
}

/// Canonical head identifiers of every `Var`/`Subscript` reference in `expr`,
/// recursing into subscript index expressions. Function names are not collected
/// (they are `App` nodes, not `Var`s); subscript *index* identifiers (dimension
/// and element names) ARE collected, so callers must intersect the result with
/// the model-variable map before treating an entry as a variable reference.
///
/// Used by the link-score emitters to discover which of a reducer body's
/// references are arrayed model variables (the [`ReducerBodyCtx`] inputs).
pub(crate) fn expr_reference_idents(expr: &Expr0) -> HashSet<String> {
    fn walk(expr: &Expr0, out: &mut HashSet<String>) {
        match expr {
            Expr0::Const(..) => {}
            Expr0::Var(ident, _) => {
                out.insert(canonicalize(ident.as_str()).into_owned());
            }
            Expr0::Subscript(ident, indices, _) => {
                out.insert(canonicalize(ident.as_str()).into_owned());
                for idx in indices {
                    match idx {
                        IndexExpr0::Expr(e) => walk(e, out),
                        IndexExpr0::Range(l, r, _) => {
                            walk(l, out);
                            walk(r, out);
                        }
                        IndexExpr0::Wildcard(_)
                        | IndexExpr0::StarRange(..)
                        | IndexExpr0::DimPosition(..) => {}
                    }
                }
            }
            Expr0::App(UntypedBuiltinFn(_, args), _) => {
                for a in args {
                    walk(a, out);
                }
            }
            Expr0::Op1(_, inner, _) => walk(inner, out),
            Expr0::Op2(_, l, r, _) => {
                walk(l, out);
                walk(r, out);
            }
            Expr0::If(c, t, f, _) => {
                walk(c, out);
                walk(t, out);
                walk(f, out);
            }
        }
    }
    let mut out = HashSet::new();
    walk(expr, &mut out);
    out
}

/// Context for the body-aware per-row linear partial (GH #744): everything
/// [`generate_linear_body_partial`] needs to evaluate a reducer's BODY at one
/// source row with the live source's reference live and every other model
/// reference frozen at `PREVIOUS`.
///
/// Built by the link-score emitters (`emit_source_to_agg_link_scores` for a
/// hoisted `$⁚ltm⁚agg⁚{n}`, `try_cross_dimensional_link_scores` for a
/// variable-backed whole-RHS reducer); all names are canonical.
pub(crate) struct ReducerBodyCtx<'a> {
    /// The reducer's array argument AST (from [`ClassifiedReducer::body`]).
    pub body: &'a Expr0,
    /// The live source variable (the row whose partial is being built).
    pub live_source: &'a str,
    /// Declared dimension count for every ARRAYED model variable referenced
    /// in the body. Pinning substitutes a reference's indices POSITIONALLY
    /// from the row tuple, which is sound because the engine's subscript
    /// resolution is itself positional: a co-source declared over a
    /// *differently named* same-size dimension (`SUM(pop[*] + other[*])`
    /// with `pop[region]`/`other[city]`) is hoisted -- `combined_read_slice`
    /// compares axis SHAPES, never dimension names -- and the resulting
    /// cross-dimension subscript (`other[region·north]`) reads the
    /// slot-aligned element, exactly as the A2A expansion of the reducer
    /// itself does. `pin_body_index` additionally validates each index
    /// against the row's axis; an unprovable correspondence bails.
    pub arrayed_dep_dims: &'a HashMap<String, usize>,
    /// Every model-variable ident the body may reference -- the freeze set.
    /// References to idents NOT in this set (TIME, function names resolved
    /// as `App`s, dimension/element names) stay live, matching
    /// `build_partial_equation_shaped`'s deps-only freezing convention.
    pub model_deps: &'a HashSet<String>,
    /// Canonical dimension names of the live source's axes, in declared
    /// order -- parallel to the row tuple.
    pub row_dim_names: &'a [String],
    /// For recognizing a positionally-MAPPED iterated-dim index (GH #534:
    /// `SUM(matrix[State,*])` over `matrix[Region,..]`); `None` disables the
    /// mapped recognition (the by-name check still applies).
    pub dims_ctx: Option<&'a crate::dimensions::DimensionsContext>,
    /// The live source's accepted read slice (one
    /// [`crate::ltm_agg::AxisRead`] per row axis, parallel to
    /// `row_dim_names`) when the reducer is a hoisted agg --
    /// `AggNode::source_read_slice(live_source)` -- or `None` on the
    /// un-hoisted conservative paths. It resolves a MISMATCHED-arity dep's
    /// dimension-name index (a GH #767 projection feeder, `frac[d1]`) to
    /// the row position whose axis is `Iterated` over that target dim --
    /// the executed A2A coordinate the index reads. The resolution must be
    /// positional, not first-match-by-name: a REPEATED-dim co-source
    /// (`matrix[D1,D1]` read as `SUM(matrix[*, D1] * frac[D1])`, slice
    /// `[Reduced, Iterated]`) has the dim name at BOTH positions, and
    /// pinning the feeder at the Reduced position freezes the wrong
    /// element -- a silently wrong score. Without a slice the by-name
    /// lookup requires the name to be UNIQUE among `row_dim_names`
    /// (ambiguity bails to the delta-ratio fallback, the pre-GH #767
    /// behavior for mismatched deps).
    pub live_read_slice: Option<&'a [crate::ltm_agg::AxisRead]>,
}

/// Substitute one subscript index of an arrayed body reference with the
/// row's element at that position (`row_part`, qualified `dim·element` or a
/// bare indexed-dim ordinal). `None` when the index cannot be proven to
/// correspond to that axis position -- the caller then bails to the
/// delta-ratio fallback rather than emitting a mis-pinned equation.
///
/// The returned `bool` is whether the index MOVES with the row -- i.e. the
/// reference reads a different element for each co-reduced row. A
/// fixed-literal index reads the same element for every row;
/// [`pin_body_to_row`] uses this to reject a live-source reference with NO
/// moving index (review I1 on GH #744: the other rows' bodies reference
/// that fixed live element, so they do not cancel against
/// `PREVIOUS(target)` and the single-row partial would silently drop their
/// contribution).
///
/// Substitutable index forms at position `j`:
/// - `*` / `*:SubDim` -- a reduced axis; the row iterates it (moves). (A
///   `StarRange` over a proper subdimension over-approximates exactly like
///   `compute_read_slice`'s conservative `Reduced` treatment.)
/// - a `Var` naming the axis's own dimension (`row_dim_names[j]`), or a
///   dimension that MAPS to it (`has_mapping_to`, the GH #534 gate) -- an
///   iterated axis (moves).
/// - a `Var`/`Const` literal element equal to the row's element at `j` -- a
///   pinned axis (fixed; re-pinned to the qualified form so
///   `PREVIOUS(...)` of the reference compiles to a direct `LoadPrev`).
fn pin_body_index(
    idx: &IndexExpr0,
    j: usize,
    ctx: &ReducerBodyCtx<'_>,
    row_parts: &[String],
) -> Option<(IndexExpr0, bool)> {
    use crate::common::CanonicalDimensionName;
    let pinned = |moves: bool| {
        (
            IndexExpr0::Expr(Expr0::Var(
                RawIdent::new_from_str(&row_parts[j]),
                crate::ast::Loc::default(),
            )),
            moves,
        )
    };
    // The row's bare element name at `j` (the part after the `dim·`
    // qualifier, or the whole part for an indexed dim).
    let row_element = row_parts[j]
        .split_once('\u{B7}')
        .map(|(_, e)| e)
        .unwrap_or(row_parts[j].as_str());
    match idx {
        IndexExpr0::Wildcard(_) | IndexExpr0::StarRange(..) => Some(pinned(true)),
        IndexExpr0::Expr(Expr0::Var(name, _)) => {
            let n = canonicalize(name.as_str());
            if n.as_ref() == ctx.row_dim_names[j].as_str() {
                return Some(pinned(true));
            }
            if let Some(dc) = ctx.dims_ctx {
                let n_dim = CanonicalDimensionName::from_raw(n.as_ref());
                let row_dim = CanonicalDimensionName::from_raw(ctx.row_dim_names[j].as_str());
                if dc.has_mapping_to(&n_dim, &row_dim) {
                    return Some(pinned(true));
                }
            }
            (n.as_ref() == row_element).then(|| pinned(false))
        }
        IndexExpr0::Expr(Expr0::Const(s, _, _)) => {
            // An indexed-dim ordinal; canonicalize via parse-then-format so
            // `pop[01]` matches the row part `"1"`.
            let n = s.parse::<u32>().ok()?;
            (n.to_string() == row_parts[j]).then(|| pinned(false))
        }
        _ => None,
    }
}

/// Resolve a MISMATCHED-arity dep's dimension-name index (canonical `name`)
/// to the row position it reads -- the GH #767 projection-feeder pin's
/// resolution step.
///
/// With the live source's accepted slice available
/// ([`ReducerBodyCtx::live_read_slice`]) the answer is the position whose
/// axis is `Iterated` over target dim `name`: the executed A2A equation
/// resolves the index to the iteration's `name`-coordinate, which is
/// exactly the row element at the slice's Iterated axis. This is robust to
/// a REPEATED dim name among the row's axes (`matrix[D1,D1]` with slice
/// `[Reduced, Iterated]`): the Reduced position shares the NAME but is the
/// co-reduced coordinate, not the slot -- pinning there freezes the wrong
/// element (a silently wrong score, the GH #767 review finding). Two
/// `Iterated` axes over the same dim cannot reach the per-row emitters
/// (such a slice's `result_dims` duplicate the dim, declined by the
/// variable-backed gate / feeder clause), but the resolution still
/// requires uniqueness defensively.
///
/// Without a slice (the un-hoisted conservative families) the name must
/// match exactly ONE of `row_dim_names`; an ambiguous name returns `None`
/// (the caller bails to the delta-ratio fallback -- the pre-GH #767
/// behavior for every mismatched dep).
fn resolve_mismatched_index_position(name: &str, ctx: &ReducerBodyCtx<'_>) -> Option<usize> {
    use crate::ltm_agg::AxisRead;
    fn unique(mut it: impl Iterator<Item = usize>) -> Option<usize> {
        let pos = it.next()?;
        it.next().is_none().then_some(pos)
    }
    match ctx.live_read_slice {
        Some(slice) => unique(slice.iter().enumerate().filter_map(|(i, ax)| match ax {
            AxisRead::Iterated { dim, .. } if dim == name => Some(i),
            _ => None,
        })),
        None => unique(
            ctx.row_dim_names
                .iter()
                .enumerate()
                .filter_map(|(i, d)| (d.as_str() == name).then_some(i)),
        ),
    }
}

/// Rewrite a reducer body so every arrayed reference reads exactly the
/// given source row: wildcard / iterated-dim / literal indices are replaced
/// by the row's (qualified) elements, and a bare arrayed-variable reference
/// gains the full row subscript. An arrayed reference with a DIFFERENT
/// axis count than the row (a GH #767 projection feeder, `frac[d1]` in the
/// 2-D `matrix` row partial) is pinned at the row position
/// [`resolve_mismatched_index_position`] resolves each index to. `None`
/// when the body cannot be safely pinned (an index that doesn't correspond
/// to the row's axes, a mismatched-axis reference with a non-dim-name or
/// ambiguous index, a nested array reducer, or a FIXED-literal reference
/// to the live source -- see below) -- the caller bails to the delta-ratio
/// fallback.
///
/// Review I1 on GH #744: a live-source reference whose indices are ALL
/// fixed literals (`pop[north]` in `SUM(pop[*] * pop[north])`) reads the
/// same element for every co-reduced row, so the OTHER rows' bodies also
/// reference the live element and the single-row cancellation invariant
/// (see [`generate_linear_body_partial`]) does not hold -- the partial
/// would drop those rows' `Σ_{i≠e} body_i` cross-terms. A live-source
/// reference with at least one MOVING index (`pop[nyc,*]`, the
/// pinned-slice shape) instantiates to a DIFFERENT element in each row, so
/// cancellation holds and it stays pinned.
fn pin_body_to_row(expr: Expr0, ctx: &ReducerBodyCtx<'_>, row_parts: &[String]) -> Option<Expr0> {
    match expr {
        Expr0::Const(..) => Some(expr),
        Expr0::Var(ref ident, loc) => {
            match ctx
                .arrayed_dep_dims
                .get(canonicalize(ident.as_str()).as_ref())
            {
                // A bare arrayed reference reads the whole array; pin it to
                // the row (only when its axes match the row's arity).
                Some(&n_dims) if n_dims == row_parts.len() => {
                    let indices = row_parts
                        .iter()
                        .map(|p| {
                            IndexExpr0::Expr(Expr0::Var(
                                RawIdent::new_from_str(p),
                                crate::ast::Loc::default(),
                            ))
                        })
                        .collect();
                    Some(Expr0::Subscript(ident.clone(), indices, loc))
                }
                Some(_) => None,
                None => Some(expr),
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            if let Some(&n_dims) = ctx
                .arrayed_dep_dims
                .get(canonicalize(ident.as_str()).as_ref())
            {
                if indices.len() != n_dims {
                    return None;
                }
                if n_dims != row_parts.len() {
                    // An arrayed dep with a DIFFERENT axis count than the
                    // live source's row (the GH #767 projection-feeder
                    // shape: `frac[d1]` inside the row partial of the 2-D
                    // co-source `matrix[d1,*]`). Positional substitution is
                    // meaningless, but a reference indexed SOLELY by
                    // dimension names resolvable to a UNIQUE row axis is
                    // pinnable: for the executed slot every co-reduced row
                    // shares the iterated coordinates, and a reduced-axis-
                    // named index reads exactly the row's element at that
                    // axis -- in both cases the pinned (frozen) reference
                    // is the value the executed equation reads, so the
                    // single-row cancellation invariant holds. Resolution
                    // is by [`resolve_mismatched_index_position`] (the
                    // live slice's Iterated axis when available, else a
                    // unique name match); anything else (a wildcard, a
                    // literal, a dim outside the row's axes, an AMBIGUOUS
                    // dim name) bails to the delta-ratio fallback. Such a
                    // dep can never be the live source (the live source's
                    // axis count equals the row's by construction), so the
                    // `any_moving` live-bail below is not relevant here.
                    let pinned: Vec<IndexExpr0> = indices
                        .iter()
                        .map(|idx| match idx {
                            IndexExpr0::Expr(Expr0::Var(name, _)) => {
                                let n = canonicalize(name.as_str());
                                let pos = resolve_mismatched_index_position(n.as_ref(), ctx)?;
                                Some(IndexExpr0::Expr(Expr0::Var(
                                    RawIdent::new_from_str(&row_parts[pos]),
                                    crate::ast::Loc::default(),
                                )))
                            }
                            _ => None,
                        })
                        .collect::<Option<Vec<_>>>()?;
                    return Some(Expr0::Subscript(ident, pinned, loc));
                }
                let mut any_moving = false;
                let pinned: Vec<IndexExpr0> = indices
                    .iter()
                    .enumerate()
                    .map(|(j, idx)| {
                        pin_body_index(idx, j, ctx, row_parts).map(|(p, moves)| {
                            any_moving |= moves;
                            p
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                // A live-source reference that does NOT move with the row
                // (all indices fixed literals) breaks the other-rows
                // cancellation invariant -- bail (review I1, GH #744).
                if !any_moving && canonicalize(ident.as_str()).as_ref() == ctx.live_source {
                    return None;
                }
                Some(Expr0::Subscript(ident, pinned, loc))
            } else {
                // Not an arrayed model variable (e.g. a graphical-function
                // holder); recurse into expression indices so nested
                // references are still pinned, leave other index forms.
                let pinned: Vec<IndexExpr0> = indices
                    .into_iter()
                    .map(|idx| match idx {
                        IndexExpr0::Expr(e) => {
                            pin_body_to_row(e, ctx, row_parts).map(IndexExpr0::Expr)
                        }
                        other => Some(other),
                    })
                    .collect::<Option<Vec<_>>>()?;
                Some(Expr0::Subscript(ident, pinned, loc))
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            // A nested array reducer inside the body (`SUM(pop[*] * MIN(q[*]))`)
            // reduces over the whole slice, not the row -- pinning its
            // argument to the row would change its meaning. Bail.
            if is_array_reducer_name(&name, args.len()) {
                return None;
            }
            let args = args
                .into_iter()
                .map(|a| pin_body_to_row(a, ctx, row_parts))
                .collect::<Option<Vec<_>>>()?;
            Some(Expr0::App(UntypedBuiltinFn(name, args), loc))
        }
        Expr0::Op1(op, arg, loc) => Some(Expr0::Op1(
            op,
            Box::new(pin_body_to_row(*arg, ctx, row_parts)?),
            loc,
        )),
        Expr0::Op2(op, l, r, loc) => Some(Expr0::Op2(
            op,
            Box::new(pin_body_to_row(*l, ctx, row_parts)?),
            Box::new(pin_body_to_row(*r, ctx, row_parts)?),
            loc,
        )),
        Expr0::If(c, t, f, loc) => Some(Expr0::If(
            Box::new(pin_body_to_row(*c, ctx, row_parts)?),
            Box::new(pin_body_to_row(*t, ctx, row_parts)?),
            Box::new(pin_body_to_row(*f, ctx, row_parts)?),
            loc,
        )),
    }
}

/// Wrap every model-variable reference of a row-pinned body in
/// `PREVIOUS()`, except occurrences of `keep_live` (when given). Subscript
/// indices are never recursed into: on an arrayed MODEL dep's subscript,
/// pinning has already replaced them with literal qualified elements (not
/// causal references); on a non-model head (whose expression indices
/// [`pin_body_to_row`] preserves) any index reference is left live -- the
/// same model/non-model boundary the pinning walk draws. The contents of
/// `PREVIOUS`/`INIT` calls are already lagged/frozen so they are not
/// re-wrapped (mirroring [`wrap_matching_in_previous`]).
fn freeze_pinned_body(expr: Expr0, freeze: &HashSet<String>, keep_live: Option<&str>) -> Expr0 {
    let should_freeze = |ident: &str| -> bool {
        let c = canonicalize(ident);
        freeze.contains(c.as_ref()) && Some(c.as_ref()) != keep_live
    };
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if should_freeze(ident.as_str()) {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::Subscript(ref ident, _, loc) => {
            if should_freeze(ident.as_str()) {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                return Expr0::App(UntypedBuiltinFn(name, args), loc);
            }
            let args = args
                .into_iter()
                .map(|a| freeze_pinned_body(a, freeze, keep_live))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, arg, loc) => Expr0::Op1(
            op,
            Box::new(freeze_pinned_body(*arg, freeze, keep_live)),
            loc,
        ),
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(
            op,
            Box::new(freeze_pinned_body(*l, freeze, keep_live)),
            Box::new(freeze_pinned_body(*r, freeze, keep_live)),
            loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(freeze_pinned_body(*c, freeze, keep_live)),
            Box::new(freeze_pinned_body(*t, freeze, keep_live)),
            Box::new(freeze_pinned_body(*f, freeze, keep_live)),
            loc,
        ),
    }
}

/// Is the pinned body exactly the live source pinned at the row -- i.e. the
/// original body was the bare source reference (`SUM(pop[*])`,
/// `SUM(matrix[D1,*])`)? When true the legacy linear shortcut is exact and
/// [`generate_linear_body_partial`] emits its byte-identical form.
fn pinned_body_is_bare_source(expr: &Expr0, live_source: &str, row_parts: &[String]) -> bool {
    let Expr0::Subscript(ident, indices, _) = expr else {
        return false;
    };
    if canonicalize(ident.as_str()).as_ref() != live_source || indices.len() != row_parts.len() {
        return false;
    }
    indices.iter().zip(row_parts).all(|(idx, part)| {
        matches!(idx, IndexExpr0::Expr(Expr0::Var(name, _))
            if canonicalize(name.as_str()).as_ref() == part.as_str())
    })
}

/// Does the pinned body still reference `live_source` outside
/// `PREVIOUS`/`INIT`? When it doesn't, the live and frozen evaluations are
/// identical and the partial would be a constant 0 -- a sign the pinning
/// went wrong, so the caller bails to the delta-ratio fallback.
fn pinned_body_references_live(expr: &Expr0, live_source: &str) -> bool {
    match expr {
        Expr0::Const(..) => false,
        Expr0::Var(ident, _) | Expr0::Subscript(ident, _, _) => {
            canonicalize(ident.as_str()).as_ref() == live_source
        }
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                return false;
            }
            args.iter()
                .any(|a| pinned_body_references_live(a, live_source))
        }
        Expr0::Op1(_, inner, _) => pinned_body_references_live(inner, live_source),
        Expr0::Op2(_, l, r, _) => {
            pinned_body_references_live(l, live_source)
                || pinned_body_references_live(r, live_source)
        }
        Expr0::If(c, t, f, _) => {
            pinned_body_references_live(c, live_source)
                || pinned_body_references_live(t, live_source)
                || pinned_body_references_live(f, live_source)
        }
    }
}

/// The per-co-reduced-row body terms of a reducer's changed-first partial:
/// the body pinned to each row, live at the scored row and fully frozen
/// elsewhere. `BareSource` reports the degenerate case where the pinned
/// body is exactly the source reference, so a caller can keep its legacy
/// bare-element builder byte-identical.
///
/// Shared by the nonlinear body partial ([`generate_nonlinear_body_partial`],
/// which combines the terms with MIN/MAX/STDDEV) and the GH #910 linear
/// INNER partial ([`linear_inner_partial`], which sums them) -- so the two
/// can never disagree about what "row `e` live, other rows frozen" means.
///
/// `None` when some row's body cannot be safely pinned (see
/// [`pin_body_to_row`]) or the pinned body does not reference the live
/// source at all; the caller degrades to the delta-ratio fallback.
enum PinnedRowTerms {
    BareSource,
    Terms(Vec<String>),
}

fn pinned_body_row_terms(
    ctx: &ReducerBodyCtx<'_>,
    current_element: &str,
    all_elements: &[String],
) -> Option<PinnedRowTerms> {
    let ast = ctx.body.clone();
    let row_parts_of =
        |elem: &str| -> Vec<String> { elem.split(',').map(|p| p.trim().to_string()).collect() };
    let current_parts = row_parts_of(current_element);
    if current_parts.len() != ctx.row_dim_names.len() {
        return None;
    }
    let pinned_current = pin_body_to_row(ast.clone(), ctx, &current_parts)?;
    if pinned_body_is_bare_source(&pinned_current, ctx.live_source, &current_parts) {
        return Some(PinnedRowTerms::BareSource);
    }
    if !pinned_body_references_live(&pinned_current, ctx.live_source) {
        return None;
    }
    // One term per co-reduced row: live at the scored row, fully frozen
    // elsewhere. Terms are parenthesized -- they are compound expressions
    // landing inside call arguments and `+`/`-`/`^` contexts.
    let mut terms = Vec::with_capacity(all_elements.len());
    for elem in all_elements {
        let term = if elem == current_element {
            freeze_pinned_body(
                pinned_current.clone(),
                ctx.model_deps,
                Some(ctx.live_source),
            )
        } else {
            let parts = row_parts_of(elem);
            if parts.len() != ctx.row_dim_names.len() {
                return None;
            }
            let pinned = pin_body_to_row(ast.clone(), ctx, &parts)?;
            freeze_pinned_body(pinned, ctx.model_deps, None)
        };
        terms.push(format!("({})", print_eqn(&term)));
    }
    Some(PinnedRowTerms::Terms(terms))
}

/// The changed-first partial of a LINEAR reducer expressed in the reducer's
/// OWN (pre-graphical-function) units -- a FULL re-evaluation of the reducer
/// over its co-reduced rows, with the scored row live and every other row
/// frozen at `PREVIOUS` (GH #910).
///
/// The ordinary linear builders ([`generate_linear_partial`] /
/// [`generate_linear_body_partial`]) express the same quantity INCREMENTALLY,
/// anchored on `PREVIOUS(target)`. That anchor is the target's *value*, so
/// when the target is an implicit WITH-LOOKUP variable it is in gf-OUTPUT
/// units while the delta added to it is in gf-INPUT units -- the two cannot
/// be mixed, and the resulting numerator can even carry the wrong sign. The
/// enumerated form here has no anchor, so it can be fed through the target's
/// gf to land back in gf-output units. It is exactly equal to the incremental
/// form when no gf is applied, so it is used ONLY on the gf path (the
/// gf-less emission stays byte-identical, and the enumeration's `O(N)` text
/// is paid only where it is needed).
///
/// `None` when the body cannot be pinned per row; the caller degrades to the
/// delta-ratio fallback (which needs no wrap).
fn linear_inner_partial(
    body: Option<&ReducerBodyCtx<'_>>,
    source_q: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_upper: &str,
) -> Option<String> {
    let bare_terms = || -> Vec<String> {
        all_elements
            .iter()
            .map(|elem| {
                if elem == current_element {
                    format!("({source_q}[{elem}])")
                } else {
                    format!("(PREVIOUS({source_q}[{elem}]))")
                }
            })
            .collect()
    };
    let terms = match body {
        Some(ctx) => match pinned_body_row_terms(ctx, current_element, all_elements)? {
            PinnedRowTerms::BareSource => bare_terms(),
            PinnedRowTerms::Terms(terms) => terms,
        },
        None => bare_terms(),
    };
    if terms.is_empty() {
        return None;
    }
    let sum = terms.join(" + ");
    match reducer_upper {
        "MEAN" => Some(format!("(({sum}) / {})", terms.len())),
        // SUM is the default linear case.
        _ => Some(format!("({sum})")),
    }
}

/// The body-aware changed-first per-row partial for a linear reducer
/// (SUM/MEAN) -- GH #744.
///
/// For source row `e` the true changed-first partial holds every OTHER
/// input (co-sources, scalar feeders, the other rows of the source) at
/// `PREVIOUS` and lets `source[e]` move. Every fully-frozen row then
/// contributes exactly its share of `PREVIOUS(target)` -- PROVIDED no
/// other row's body references the live element, which [`pin_body_to_row`]
/// enforces by rejecting a fixed-literal self-reference (a live-source
/// reference with no row-moving index, e.g. `pop[north]` in
/// `SUM(pop[*] * pop[north])`; every other surviving live-source reference
/// reads exactly the row's own element, so the other rows stay fully
/// frozen). Under that guarantee the partial collapses to the single-row
/// form
///
/// ```text
/// SUM:  PREVIOUS(target) + (body_e_live - body_e_frozen)
/// MEAN: PREVIOUS(target) + (body_e_live - body_e_frozen) / N
/// ```
///
/// where `body_e_live` is the reducer body pinned to row `e` with the
/// source's reference live and every other model reference frozen, and
/// `body_e_frozen` additionally freezes the source -- only scalar/
/// fixed-element `PREVIOUS` reads, so it always compiles (no lagged
/// whole-array read).
///
/// When the pinned body is exactly the bare source reference the legacy
/// [`generate_linear_partial`] string is returned byte-identically. `None`
/// means the body cannot be safely pinned to the row (see
/// [`pin_body_to_row`]); the caller falls back to the delta-ratio form.
fn generate_linear_body_partial(
    ctx: &ReducerBodyCtx<'_>,
    source_q: &str,
    target_ref: &str,
    current_element: &str,
    n_elements: usize,
    reducer_name: &str,
) -> Option<String> {
    let ast = ctx.body.clone();
    let row_parts: Vec<String> = current_element
        .split(',')
        .map(|p| p.trim().to_string())
        .collect();
    if row_parts.len() != ctx.row_dim_names.len() {
        return None;
    }
    let pinned = pin_body_to_row(ast, ctx, &row_parts)?;
    if pinned_body_is_bare_source(&pinned, ctx.live_source, &row_parts) {
        // The shortcut is exact for a bare body; keep its emission
        // byte-identical to the pre-body-aware form.
        return Some(generate_linear_partial(
            source_q,
            target_ref,
            current_element,
            n_elements,
            reducer_name,
        ));
    }
    if !pinned_body_references_live(&pinned, ctx.live_source) {
        return None;
    }
    let live = print_eqn(&freeze_pinned_body(
        pinned.clone(),
        ctx.model_deps,
        Some(ctx.live_source),
    ));
    let frozen = print_eqn(&freeze_pinned_body(pinned, ctx.model_deps, None));
    let delta = format!("(({live}) - ({frozen}))");
    match reducer_name.to_uppercase().as_str() {
        "MEAN" => Some(format!("PREVIOUS({target_ref}) + {delta} / {n_elements}")),
        // SUM is the default linear case.
        _ => Some(format!("PREVIOUS({target_ref}) + {delta}")),
    }
}

/// The body-aware changed-first per-row partial for a nonlinear reducer
/// (MIN/MAX/STDDEV) -- GH #762, the nonlinear sibling of
/// [`generate_linear_body_partial`].
///
/// For `agg = R(body(r) for r in coreduced)` with `R ∈ {MIN, MAX, STDDEV}`
/// the changed-first partial for source row `e` evaluates `R` over one
/// term per co-reduced row:
///
/// ```text
/// term_e      = body pinned to row e, source live, other model refs frozen
/// term_r, r≠e = body pinned to row r, ALL model refs frozen
/// partial     = R(term_e, term_r, ...)
/// ```
///
/// and the link-score guard form's numerator is `partial -
/// PREVIOUS(agg)`. Unlike SUM/MEAN there is no single-row collapse -- the
/// frozen rows' terms do not cancel inside MIN/MAX/STDDEV, so every
/// co-reduced row's frozen body is spelled out (exactly the structure the
/// bare-body builder already used, with `body(r)` in place of the bare
/// element). The terms contain only scalar / fixed-element `PREVIOUS`
/// reads, so they always compile. MIN/MAX nest binary calls and STDDEV
/// keeps the GH #483 unrolled population-variance form (divisor `N`,
/// inlined mean) over the body terms.
///
/// Anchor caveat (GH #763): "frozen" freezes MODEL references only, so a
/// body referencing TIME, a time builtin (PULSE/STEP/RAMP), or a nested
/// `PREVIOUS(x)` keeps that factor live in every term, and then
/// `R(all-frozen terms) != PREVIOUS(agg)` -- the anchor subtraction
/// attributes the time-drift to every row, including rows whose true
/// partial is 0 (destroying the frozen-argmin-scores-0 property of
/// MIN/MAX). For pure-model-ref bodies the anchor identity holds exactly
/// because per-variable `PREVIOUS` sampling commutes with arithmetic.
///
/// When the pinned body is the bare source reference the legacy
/// [`generate_nonlinear_partial`] is returned byte-identically; RANK is
/// body-independent (the documented delta-ratio stand-in) and delegates
/// to the legacy builder unconditionally. `None` means some co-reduced
/// row's body cannot be safely pinned (see [`pin_body_to_row`], including
/// the fixed-literal self-reference bail); the caller falls back to the
/// delta-ratio form.
fn generate_nonlinear_body_partial(
    ctx: &ReducerBodyCtx<'_>,
    source_q: &str,
    target_ref: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_name: &str,
) -> Option<String> {
    let upper = reducer_name.to_uppercase();
    if upper == "RANK" {
        // RANK is an order statistic; its delta-ratio stand-in does not
        // read the body at all (see generate_nonlinear_partial's doc).
        return Some(generate_nonlinear_partial(
            source_q,
            target_ref,
            current_element,
            all_elements,
            reducer_name,
        ));
    }
    let terms = match pinned_body_row_terms(ctx, current_element, all_elements)? {
        PinnedRowTerms::BareSource => {
            // The legacy per-element expansion is exact for a bare body; keep
            // its emission byte-identical to the pre-body-aware form.
            return Some(generate_nonlinear_partial(
                source_q,
                target_ref,
                current_element,
                all_elements,
                reducer_name,
            ));
        }
        PinnedRowTerms::Terms(terms) => terms,
    };
    match upper.as_str() {
        "MIN" | "MAX" => {
            // Nest binary calls right-to-left, mirroring the bare builder:
            // MIN(a, MIN(b, c)) for [a, b, c].
            if terms.len() == 1 {
                return Some(terms[0].clone());
            }
            let mut result = terms[terms.len() - 1].clone();
            for term in terms[..terms.len() - 1].iter().rev() {
                result = format!("{upper}({term}, {result})");
            }
            Some(result)
        }
        "STDDEV" => {
            // The GH #483 unrolled population-variance partial (divisor N,
            // matching vm.rs::Opcode::ArrayStddev; mean string-inlined)
            // over the body terms.
            let n = terms.len();
            if n <= 1 {
                // The variance of a single term is identically 0 (mirrors
                // the bare builder's single-element special case).
                return Some("0".to_string());
            }
            let mean = format!("(({}) / {n})", terms.join(" + "));
            let squared_devs: Vec<String> = terms
                .iter()
                .map(|t| format!("(({t} - {mean})^2)"))
                .collect();
            Some(format!("sqrt(({}) / {n})", squared_devs.join(" + ")))
        }
        _ => None,
    }
}

/// Generate a per-element link score equation for an arrayed-to-scalar edge.
///
/// For element `current_element` of source variable `source_var_name`,
/// produces the partial equation where ONLY `source[current_element]` varies
/// while all other elements are held at PREVIOUS values.
///
/// `reducer_kind` determines the generation strategy:
/// - `Linear`: the body-aware changed-first partial
///   ([`generate_linear_body_partial`]) when a [`ReducerBodyCtx`] is given,
///   collapsing to the algebraic shortcut for a bare-source body
/// - `Nonlinear`: explicit element expansion with selective PREVIOUS wrapping
/// - `Constant`: caller should skip generation (SIZE always produces 0)
///
/// `reducer_name` is the uppercase function name ("MIN", "MAX", "STDDEV", "RANK")
/// used for nonlinear reducers when reconstructing the function call.
///
/// `is_bare` indicates whether the reducer is the entire target equation (true)
/// or is nested inside surrounding arithmetic like `2 * SUM(...)` (false).
/// When false, neither the shortcut nor the body partial accounts for the
/// surrounding arithmetic, so the delta-ratio fallback (using the target
/// variable directly) is used instead. Arithmetic INSIDE the reducer argument
/// is the `body` context's job (GH #744): without it the Linear arm asserts
/// ∂target/∂source[e] = 1, which is wrong-magnitude (and wrong-signed for a
/// negative coefficient) whenever the body is not the bare source.
///
/// `gf_table_ref` is the implicit WITH-LOOKUP wrap when the reducer's OWNER
/// (`target_var_name`) is a tables-carrying value-bearing variable
/// (GH #910); see [`with_lookup::is_implicit_with_lookup`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_element_to_scalar_equation(
    source_var_name: &str,
    target_var_name: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
    is_bare: bool,
    body: Option<&ReducerBodyCtx<'_>>,
    gf_table_ref: Option<&str>,
) -> String {
    let source_q = quote_ident(source_var_name);
    let target_ref = quote_ident(target_var_name);
    build_element_reducer_link_score(
        &source_q,
        &target_ref,
        current_element,
        all_elements,
        reducer_kind,
        reducer_name,
        is_bare,
        body,
        gf_table_ref,
    )
}

/// Generate a per-element link score equation for a *partial* reduce edge,
/// where an arrayed source feeds an arrayed-result reducer (e.g.
/// `agg[D1] = SUM(matrix[D1,*])`) that collapses only some of the source's
/// axes.
///
/// `current_element` is the full source element tuple (e.g. `"a,x"` for
/// `matrix[a,x]`); `result_element` is its projection onto the surviving
/// (result) axes (e.g. `"a"` for `agg[a]`); `all_coreduced_elements` is the
/// set of source element tuples that share `result_element` -- i.e. the
/// `matrix[a,*]` slice that the reducer combines -- so the algebraic
/// shortcut divides MEAN by the reduced-axis cardinality and the nonlinear
/// expansion enumerates exactly that slice (other rows are irrelevant). The
/// ceteris-paribus partial therefore holds the *rest of that slice* at
/// PREVIOUS while `source[current_element]` varies, and the target
/// reference (`agg[result_element]`) and source reference
/// (`source[current_element]`) are both subscripted.
///
/// Mirrors [`generate_element_to_scalar_equation`]; the scalar case is the
/// degenerate partial reduce with an empty result axis -- both share
/// `build_element_reducer_link_score`, so the per-reducer treatment is
/// identical. SUM/MEAN get the algebraic shortcut; MIN/MAX get the nested
/// 2-arg unroll; STDDEV gets the analytic ceteris-paribus partial over the
/// co-reduced slice (#483); RANK and nested reducers fall back to the
/// delta-ratio form against `agg[result_element]`.
#[allow(clippy::too_many_arguments)] // mirrors generate_element_to_scalar_equation's signature
pub(crate) fn generate_element_to_reduced_equation(
    source_var_name: &str,
    target_var_name: &str,
    current_element: &str,
    result_element: &str,
    all_coreduced_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
    is_bare: bool,
    body: Option<&ReducerBodyCtx<'_>>,
    gf_table_ref: Option<&str>,
) -> String {
    let source_q = quote_ident(source_var_name);
    let target_ref = format!("{}[{}]", quote_ident(target_var_name), result_element);
    build_element_reducer_link_score(
        &source_q,
        &target_ref,
        current_element,
        all_coreduced_elements,
        reducer_kind,
        reducer_name,
        is_bare,
        body,
        gf_table_ref,
    )
}

/// Shared body for the per-element reducer link score equation.
///
/// `source_q` is the already-quoted source variable name; `target_ref` is
/// the already-formatted target reference (a bare quoted ident for a scalar
/// target, or `agg[result_element]` for an arrayed-result partial reduce).
/// `current_element` is the source element subscript that stays live;
/// `all_elements` is the set of source elements the reducer combines
/// (every element for a full reduce; the surviving-axis-fixed slice for a
/// partial reduce) -- its length is the MEAN divisor and the nonlinear
/// expansion iterates it.
#[allow(clippy::too_many_arguments)]
fn build_element_reducer_link_score(
    source_q: &str,
    target_ref: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
    is_bare: bool,
    body: Option<&ReducerBodyCtx<'_>>,
    gf_table_ref: Option<&str>,
) -> String {
    let source_elem = format!("{source_q}[{current_element}]");
    let upper = reducer_name.to_uppercase();

    let partial_eq = match reducer_kind {
        ReducerKind::Constant => {
            // SIZE is constant; caller should not generate link scores.
            // Return a zero equation as a defensive fallback.
            return "0".to_string();
        }
        _ if !is_bare => {
            // The reducer is nested inside surrounding arithmetic (e.g.,
            // `2 * SUM(population[*])` or `MAX(SUM(population[*]), 0)`).
            // Neither the algebraic shortcut nor the body-aware partial
            // accounts for the surrounding expression. Fall back to the
            // delta-ratio approach: use the target variable directly, which
            // measures the ratio of actual target change to source element
            // change. This is approximate (like STDDEV/RANK) but avoids the
            // wrong-multiplier bug the shortcut would introduce.
            //
            // This partial is the target's own (already gf-applied) value,
            // so it needs no implicit WITH-LOOKUP wrap -- wrapping it would
            // feed a gf-output value back through the gf.
            target_ref.to_string()
        }
        // GH #910: an implicit WITH-LOOKUP owner's compiled value is
        // `gf(reducer)`, so the linear ANCHORED partial (which adds a
        // gf-input delta to the gf-output `PREVIOUS(target)`) is
        // dimensionally incoherent -- and can invert the score's sign
        // relative to the composed link polarity. Rebuild the partial as a
        // full re-evaluation of the reducer (gf-input units) and feed it
        // through the target's own table.
        ReducerKind::Linear if gf_table_ref.is_some() => {
            match linear_inner_partial(body, source_q, current_element, all_elements, &upper) {
                Some(inner) => format!("LOOKUP({}, {inner})", gf_table_ref.unwrap()),
                // Un-pinnable body: the delta-ratio fallback, which is
                // already in gf-output units.
                None => target_ref.to_string(),
            }
        }
        // GH #744: with a body context, build the changed-first partial from
        // the reducer's BODY at this row (exact for any body linear in the
        // source, byte-identical to the shortcut for a bare body). An
        // un-pinnable body degrades to the same delta-ratio fallback the
        // nested (`!is_bare`) case uses. Without a context (test-only
        // callers) the bare-source shortcut is asserted as before.
        ReducerKind::Linear => match body {
            Some(ctx) => generate_linear_body_partial(
                ctx,
                source_q,
                target_ref,
                current_element,
                all_elements.len(),
                reducer_name,
            )
            .unwrap_or_else(|| target_ref.to_string()),
            None => generate_linear_partial(
                source_q,
                target_ref,
                current_element,
                all_elements.len(),
                reducer_name,
            ),
        },
        // GH #762 (the nonlinear sibling of the GH #744 Linear arm): with
        // a body context, build each MIN/MAX/STDDEV term from the
        // row-pinned BODY (byte-identical legacy emission for a bare
        // body; RANK delegates unconditionally). An un-pinnable body
        // degrades to the same delta-ratio fallback. Without a context
        // (test-only callers) the bare-element expansion is used as
        // before.
        ReducerKind::Nonlinear => {
            let raw = match body {
                Some(ctx) => generate_nonlinear_body_partial(
                    ctx,
                    source_q,
                    target_ref,
                    current_element,
                    all_elements,
                    reducer_name,
                ),
                None => Some(generate_nonlinear_partial(
                    source_q,
                    target_ref,
                    current_element,
                    all_elements,
                    reducer_name,
                )),
            };
            // MIN/MAX/STDDEV partials ARE a full re-evaluation of the
            // reducer (gf-input units), so a WITH-LOOKUP owner wraps them
            // (GH #910). RANK's stand-in is the delta-ratio -- the target
            // itself, already gf-output -- and must never be wrapped, and
            // neither must the un-pinnable-body degradation to the same
            // stand-in.
            match (gf_table_ref, raw) {
                (Some(table_ref), Some(inner)) if upper != "RANK" => {
                    format!("LOOKUP({table_ref}, {inner})")
                }
                (_, Some(partial)) => partial,
                (_, None) => target_ref.to_string(),
            }
        }
    };

    // Standard link score formula wrapping the partial equation, in the
    // single-numerator form (see `link_score_guard_form` for the algebra):
    // the partial appears once instead of twice.
    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                (({target_ref} - PREVIOUS({target_ref})) = 0) OR (({source_elem} - PREVIOUS({source_elem})) = 0) \
                then 0 \
                else SAFEDIV(({partial_eq} - PREVIOUS({target_ref})), ABS(({target_ref} - PREVIOUS({target_ref}))), 0) * SIGN(({source_elem} - PREVIOUS({source_elem})))"
    )
}

/// Generate the partial evaluation for a linear reducer (SUM or MEAN)
/// whose body is the BARE source reference:
///
/// SUM: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem]))
/// MEAN: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem])) / N
///
/// This asserts ∂target/∂source[elem] = 1, which is exact only when the
/// reducer's argument is the source itself (`SUM(pop[*])`). For any other
/// body the coefficient on the source is dropped -- wrong magnitude, and
/// wrong sign when the coefficient is negative (GH #744) -- so production
/// callers route through [`generate_linear_body_partial`], which collapses
/// to this form (byte-identically) for the bare case.
fn generate_linear_partial(
    source_q: &str,
    target_q: &str,
    current_element: &str,
    n_elements: usize,
    reducer_name: &str,
) -> String {
    let delta =
        format!("({source_q}[{current_element}] - PREVIOUS({source_q}[{current_element}]))");

    match reducer_name.to_uppercase().as_str() {
        "MEAN" => {
            format!("PREVIOUS({target_q}) + {delta} / {n_elements}")
        }
        // SUM is the default linear case
        _ => {
            format!("PREVIOUS({target_q}) + {delta}")
        }
    }
}

/// Generate the partial evaluation for a nonlinear reducer whose body is
/// the BARE source reference (or for RANK, whose stand-in ignores the
/// body).
///
/// Like [`generate_linear_partial`], this enumerates the bare source
/// elements, which is exact only for a bare body (`MIN(pop[*])`).
/// Production callers route through [`generate_nonlinear_body_partial`]
/// (GH #762), which builds each term from the row-pinned BODY and
/// collapses to this builder byte-identically for the bare case.
///
/// - **MIN/MAX**: nests 2-argument calls to enumerate every element with
///   selective `PREVIOUS` wrapping (`MIN(s[d], MIN(PREVIOUS(s[e]), ...))`).
/// - **STDDEV**: builds the true ceteris-paribus partial -- the unrolled
///   population-variance `sqrt` formula holding `s[d]` live and the other
///   elements frozen at `PREVIOUS`. This matches the engine's STDDEV,
///   which is population variance (divisor `N`, not `N-1`; see
///   `vm.rs::Opcode::ArrayStddev`).
/// - **RANK**: keeps the delta-ratio stand-in (`target_q` directly, so the
///   surrounding link-score formula degenerates to `|Δtarget/Δtarget|`).
///   RANK is an order statistic -- non-differentiable, array-argument-only,
///   and unreachable via real models (RANK returns an array, so it cannot
///   be a scalar/A2A reducer RHS or a partial-reduce RHS -- a dimension
///   error). The delta-ratio is the documented conservative stand-in,
///   pinned by `test_generate_rank_keeps_delta_ratio` so the choice is
///   explicit, not a silent fallback.
fn generate_nonlinear_partial(
    source_q: &str,
    target_q: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_name: &str,
) -> String {
    // The term string for source element `e`: live (`s[e]`) when it is the
    // element this partial isolates, frozen at PREVIOUS otherwise.
    let term_for = |elem: &str| -> String {
        if elem == current_element {
            format!("{source_q}[{elem}]")
        } else {
            format!("PREVIOUS({source_q}[{elem}])")
        }
    };

    match reducer_name.to_uppercase().as_str() {
        "MIN" | "MAX" => {
            // Nest binary calls: MIN(a, MIN(b, MIN(c, d))) etc.
            let args: Vec<String> = all_elements.iter().map(|e| term_for(e)).collect();
            let fn_name = reducer_name.to_uppercase();
            if args.len() == 1 {
                return args[0].clone();
            }
            // Build nested binary calls from right to left:
            // MIN(a, MIN(b, c)) for [a, b, c]
            let mut result = args[args.len() - 1].clone();
            for arg in args[..args.len() - 1].iter().rev() {
                result = format!("{fn_name}({arg}, {result})");
            }
            result
        }
        "STDDEV" => {
            // Population variance has divisor N (the engine's
            // `ArrayStddev`), so the ceteris-paribus partial for element
            // `d` is sqrt((sum_i (s'_i - m)^2) / N) with s'_i = s[d] when
            // i == d else PREVIOUS(s[i]), and m = (sum_i s'_i) / N. `m` is
            // string-inlined into each squared deviation (N is the
            // dimension cardinality, typically small; a synthetic helper
            // aux for the mean would be a synthetic-var-emission change and
            // is out of scope).
            let n = all_elements.len();
            if n <= 1 {
                // The variance of a single element is identically 0;
                // mirrors the MIN/MAX `args.len() == 1` special case
                // (avoid emitting `sqrt(((... - ...)^2) / 1)`).
                return "0".to_string();
            }
            let terms: Vec<String> = all_elements.iter().map(|e| term_for(e)).collect();
            let mean = format!("(({}) / {n})", terms.join(" + "));
            let squared_devs: Vec<String> = terms
                .iter()
                .map(|t| format!("(({t} - {mean})^2)"))
                .collect();
            format!("sqrt(({}) / {n})", squared_devs.join(" + "))
        }
        "RANK" => target_q.to_string(),
        _ => {
            unreachable!(
                "generate_nonlinear_partial only handles MIN/MAX/STDDEV/RANK; got {reducer_name}"
            )
        }
    }
}

/// The wrap's subscript-INDEX pass, in its own file only to keep this one under
/// the project line-count lint. Re-exported so callers keep naming these
/// `crate::ltm_augment::*`.
#[path = "ltm_augment_index.rs"]
mod index_pass;

use index_pass::{axis_dim_at, wrap_index_non_matching_in_previous};

#[cfg(test)]
#[path = "ltm_augment_tests.rs"]
mod tests;

// The Track-A differential gate: the Expr2 and Expr0 access-shape classifier
// families must agree per non-reducer reference occurrence, so the
// ceteris-paribus live reference survives PREVIOUS-wrapping instead of
// silently zeroing the link score. Mounted here (not in `ltm_ir`) so it sees
// this module's private Expr0 classifier (`classify_expr0_subscript_shape`)
// and `IteratedDimCtx`; it reaches the Expr2 side through `pub(crate)`
// `db::ltm_ir`.
#[cfg(test)]
#[path = "ltm_classifier_agreement_tests.rs"]
mod classifier_agreement_tests;
