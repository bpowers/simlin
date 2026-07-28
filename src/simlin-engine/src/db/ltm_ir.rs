// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! LTM reference-site classification IR.
//!
//! `model_ltm_reference_sites` is the single salsa-tracked place a causal
//! edge's access shape *and* aggregate-node routing are decided. It consumes
//! `enumerate_agg_nodes` (which stays the sole "is this subexpression a
//! hoistable maximal reducer" decider) and `reconstruct_model_variables`,
//! walks each variable's `Expr2` AST exactly once, and buckets every
//! `Var` / `Subscript` reference by its `(from, to)` causal edge into a
//! `Vec<ClassifiedSite>` carrying the per-reference `shape`,
//! `target_element`, and `routing` (`Direct` or `ThroughAgg`).
//!
//! `model_element_causal_edges`, `model_edge_shapes`, and `model_ltm_variables`
//! are pure readers of this IR -- none re-walks the AST for shape/routing,
//! none restates the `routed_aggs` filter.
//!
//! The `Expr2` AST-walker helpers (`collect_all_reference_sites`,
//! `classify_subscript_shape`, `resolve_literal_index`) moved here from
//! `db/analysis.rs` (their previous home before the IR existed). `RefShape`,
//! `emit_edges_for_reference`, and the element-name expansion helpers stay in
//! `db/analysis.rs`; this module imports `RefShape` via `crate::db::RefShape`.
//!
//! This is a submodule of `db` (a child of `db.rs`) kept in its own file
//! purely to keep `db.rs` under the per-file line cap; callers in the `db`
//! submodules use `crate::db::ltm_ir::...`.

use std::collections::HashMap;

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::db::{Db, RefShape, SourceModel, SourceProject, reconstruct_model_variables};

// ── AST-walker helpers (moved from db/analysis.rs) ─────────────────────────

/// One occurrence of a source variable in a target's AST -- the IR builder's
/// internal per-variable intermediate, before the reducer context + the
/// hoisting decision are folded into [`ClassifiedSite::routing`].
///
/// `target_element` is set only when the reference appears inside an
/// `Ast::Arrayed` per-element expression: the value is the canonical
/// element name (single-dim) or comma-separated tuple (multi-dim) of the
/// target element being defined. For `Ast::Scalar` and `Ast::ApplyToAll`
/// it stays `None` (the reference contributes to every target element
/// according to the shape's normal broadcast/diagonal rules).
///
/// `in_reducer` is true iff [`reducer_keys`] is non-empty: the reference site
/// occurs syntactically inside an aggregate-routed builtin call
/// (`SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`, plus array-valued `RANK`). `SIZE` and
/// the 2-arg `MIN`/`MAX` are not routed. It is the coarse signal for "this
/// site belongs to an aggregate read".
///
/// `reducer_keys` carries the canonical printed text of every enclosing
/// hoistable reducer, outermost to innermost. Routing must match a site to an
/// aggregate node by this key, not just by `(from, to)`: GH #793 showed that a
/// declined sibling reducer read of `from` can share an edge with a hoisted
/// sibling reducer. The declined site's contribution must remain direct and
/// get loudly dropped, not be absorbed into the sibling agg's halves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceSite {
    pub shape: RefShape,
    pub target_element: Option<String>,
    pub in_reducer: bool,
    pub reducer_keys: Vec<String>,
}

/// Resolve a single subscript index to a literal element name (canonical
/// lowercase) if it matches one of the source's dimensions, or `None`
/// for any other shape (wildcard, range, position, non-literal
/// expression, or a literal that doesn't match a known element).
///
/// Used by [`classify_subscript_shape`] to classify `Subscript` shapes:
/// every index in a `FixedIndex` must resolve via this helper. If any
/// index fails to resolve, the subscript falls back to `DynamicIndex` --
/// or `Wildcard` if a wildcard is present (wildcards are checked first
/// in the caller).
///
/// Element names parse as `Expr2::Var(ident, ...)` (the parser keeps the
/// raw element identifier as a Var; dimension-resolution into a numeric
/// offset happens later, in Expr3 lowering). Integer literals (used for
/// indexed dimensions like `1`, `2`) parse as `Expr2::Const`. We accept
/// both forms.
///
/// Note: `source_dims` is the source variable's *full* dimension list.
/// In multidimensional subscripts the caller doesn't know which
/// dimension a literal belongs to; we accept the first dimension whose
/// element registry contains the canonical name. Literal indices that
/// don't match any known element classify defensively as `DynamicIndex`,
/// so the worst case is over-conservative (full cross-product) edges.
fn resolve_literal_index(
    idx: &crate::ast::IndexExpr2,
    source_dims: &[crate::dimensions::Dimension],
) -> Option<String> {
    use crate::ast::{Expr2, IndexExpr2};

    // Element names appear as `Var(ident, ...)`; integer literals appear
    // as `Const(text, value, _)`. Anything else (wildcards, ranges, dim
    // positions, or compound expressions) is not a literal element.
    let canonical = match idx {
        IndexExpr2::Expr(Expr2::Var(ident, _, _)) => ident.as_str().to_string(),
        IndexExpr2::Expr(Expr2::Const(text, _, _)) => canonicalize(text).into_owned(),
        _ => return None,
    };

    for dim in source_dims {
        match dim {
            crate::dimensions::Dimension::Named(_, named) => {
                if named.elements.iter().any(|e| e.as_str() == canonical) {
                    return Some(canonical);
                }
            }
            crate::dimensions::Dimension::Indexed(_, size) => {
                // Indexed dimensions accept integer literals in the
                // range [1, size]. Canonicalize via parse-then-format
                // so non-canonical forms like `pop[01]` reduce to `"1"`
                // -- matching `dimension_element_names`'s `"1".."N"`
                // output and the Expr0 sibling
                // (`ltm_augment::resolve_literal_element_index`).
                // Returning the original text would let `pop[01]`
                // serialize as `FixedIndex(["01"])` while the partial
                // builder reduces to `FixedIndex(["1"])`, the shape
                // comparison would fail, and the live ref would be
                // wrapped in `PREVIOUS()`.
                if let Ok(n) = canonical.parse::<u32>()
                    && n >= 1
                    && n <= *size
                {
                    return Some(n.to_string());
                }
            }
        }
    }
    None
}

/// Classify a subscript's indices into a [`RefShape`].
///
/// Precedence:
/// 1. Any `IndexExpr2::Wildcard(_)` index ⇒ `Wildcard` (conservative full
///    cross-product unless rerouted through an agg).
/// 2. Every index is `IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _)`
///    ⇒ `Wildcard`. This is the AC1.4 fix: `enumerate_agg_nodes`'s
///    `compute_read_slice` already maps `Wildcard(_)` *and* `StarRange(_, _)`
///    to `AxisRead::Reduced`, so `SUM(x[*..*])` / `SUM(x[*:Dim])` *is*
///    hoisted -- but the previous `classify_subscript_shape` only matched
///    `Wildcard(_)`, so an all-`StarRange` reducer reference classified as
///    `DynamicIndex`. The `route_through_agg` reroute papered over it (the
///    site is `in_reducer`, so it routes to the agg and the `DynamicIndex`
///    shape never reached the cross-product fallback) -- but it left a
///    latent disagreement. Classifying an all-full-extent subscript as
///    `Wildcard` unifies the two: such a reference routes through the agg
///    with no stray `DynamicIndex` direct edge, and `emit_per_shape_link_scores`
///    suppresses its (now-`Wildcard`) shape rather than emitting a stray
///    Bare-named link score.
/// 3. Otherwise every index must resolve via [`resolve_literal_index`] for
///    the shape to be `FixedIndex`.
/// 4. Any other index pattern (a *partial* `StarRange` mixed with literal
///    indices, a `DimPosition`, a `Range`, an unrecognized literal) ⇒
///    `DynamicIndex`. (A partial-`StarRange` slice like `SUM(matrix[D1,
///    *:Dim])` keeps the coarse `DynamicIndex` shape HERE, but the reducer
///    *is* hoisted -- `compute_read_slice` carries the per-axis truth,
///    including a proper-subdimension subset since GH #766 -- and a
///    `ThroughAgg`-routed site's shape is ignored, so the coarse classifier
///    shape is routing-irrelevant: a documented residual, not a behavior
///    gap.)
fn classify_subscript_shape(
    indices: &[crate::ast::IndexExpr2],
    source_dims: &[crate::dimensions::Dimension],
) -> RefShape {
    use crate::ast::IndexExpr2;

    if indices.iter().any(|i| matches!(i, IndexExpr2::Wildcard(_))) {
        return RefShape::Wildcard;
    }
    // AC1.4: a subscript whose indices are *all* full-extent (`*` or `*:Dim`)
    // is the reducer-style whole-extent access -- treat it as `Wildcard`,
    // matching `enumerate_agg_nodes`'s `compute_read_slice` (every such axis
    // is `AxisRead::Reduced`, so the reducer is hoisted). (The `any
    // Wildcard(_)` case above already returned; this only adds the
    // all-`StarRange` and mixed-`Wildcard`/`StarRange` cases. `indices` is
    // never empty for a `Subscript`.)
    if !indices.is_empty()
        && indices
            .iter()
            .all(|i| matches!(i, IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _)))
    {
        return RefShape::Wildcard;
    }

    let mut resolved: Vec<String> = Vec::with_capacity(indices.len());
    for idx in indices {
        match resolve_literal_index(idx, source_dims) {
            Some(name) => resolved.push(name),
            None => return RefShape::DynamicIndex,
        }
    }
    RefShape::FixedIndex(resolved)
}

/// Recognize a *statically-describable per-axis* subscript -- one whose
/// every index is either an iterated-dimension name lined up with the
/// source's axis at that position or a literal element of that axis -- and
/// classify it:
///
/// - **all axes `Iterated`** ⇒ [`RefShape::Bare`] (the
///   same-element-on-shared-dims reference, GH #511: `row_sum[Region]`
///   inside `growth[Region,Age]` reads the same `Region` element, which
///   `emit_edges_for_reference`'s `Bare` arm projects via
///   `expand_same_element`);
/// - **mixed `Iterated` + `Pinned`** ⇒ [`RefShape::PerElement`] (GH #525,
///   T6 of the shape-expressiveness design: `pop[Region, young]` inside an
///   A2A-over-`Region` equation reads the same `Region` element pinned at
///   `Age = young` -- the element graph emits the diagonal-with-pinned-axes
///   edges and emission produces per-(row, full-target-element) scalar
///   scores, killing the former `DynamicIndex` cross-product's phantom
///   loops at enumeration time);
/// - **all axes `Pinned`** ⇒ `None`, falling through to
///   [`classify_subscript_shape`]'s `FixedIndex` (the canonicalization rule
///   that keeps every existing `FixedIndex` name untouched).
///
/// The per-axis decision is [`crate::ltm_agg::classify_axis_access`] -- the
/// SAME classifier `compute_read_slice` applies to reducer arguments, so
/// the reducer path and the direct-reference path can never disagree about
/// an axis. The one direct-reference divergence is a post-filter: an
/// [`AxisRead::Reduced`] result (a `*` / StarRange index) returns `None`
/// here -- a non-reducer reference never collapses an axis -- so wildcard
/// shapes keep their `classify_subscript_shape` classification.
///
/// A mapped iterated index (`State[i]` over a source declared with
/// `Region[i]`) is accepted when `classify_axis_access`'s
/// `iterated_axis_slot_elements` / `mapped_element_correspondence` gate
/// yields a usable positional remap -- in EITHER declaration direction
/// (GH #757; explicit element maps decline per the GH #756 positional-only
/// gate, keeping the conservative shape). A position-mismatched subscript
/// like `row_sum[D2]` inside `growth[D1,D2]` where `row_sum` is over `D1`
/// is a *genuine* cross-element reference -- no axis classifies -- so it
/// returns `None` and keeps its `DynamicIndex` classification.
///
/// Returns `None` when the subscript is not statically describable per
/// axis; the caller then falls back to [`classify_subscript_shape`].
fn classify_iterated_dim_shape(
    indices: &[crate::ast::IndexExpr2],
    source_dims: &[crate::dimensions::Dimension],
    target_iterated_dims: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<RefShape> {
    use crate::ltm_agg::{AxisRead, classify_axis_access};

    // Need one index per source dimension; an empty subscript is never a
    // `Subscript` node, and a longer/shorter one is not statically
    // describable per axis (a partial slice or a dimensionally-mismatched
    // reference).
    if indices.is_empty() || indices.len() != source_dims.len() {
        return None;
    }
    let axes: Vec<AxisRead> = indices
        .iter()
        .zip(source_dims)
        .map(|(idx, axis_dim)| classify_axis_access(idx, axis_dim, target_iterated_dims, dim_ctx))
        .collect::<Option<_>>()?;
    // Post-filter: a direct (non-reducer) reference never collapses an
    // axis, so any `Reduced` axis (a `*` / StarRange index) falls back to
    // the coarse classifier (`Wildcard` for all-full-extent subscripts,
    // `DynamicIndex` for partial-StarRange mixes -- both unchanged).
    if axes.iter().any(|a| matches!(a, AxisRead::Reduced { .. })) {
        return None;
    }
    let n_iterated = axes
        .iter()
        .filter(|a| matches!(a, AxisRead::Iterated { .. }))
        .count();
    if n_iterated == 0 {
        // All-`Pinned` canonicalizes to `FixedIndex` via the caller's
        // `classify_subscript_shape` fallback (identical resolution rules).
        return None;
    }
    if n_iterated == axes.len() {
        return Some(RefShape::Bare);
    }
    Some(RefShape::PerElement { axes })
}

// ── Single-pass all-sources walk ───────────────────────────────────────────

/// Read-only walk context shared by every recursive call of
/// [`walk_all_in_expr`] for a single target variable: the model's variable
/// map (so a referenced ident can be confirmed to be a model variable), the
/// target equation's iterated dimension names (canonical, in source order;
/// empty for an `Ast::Scalar` target), and a [`DimensionsContext`] for the
/// AC3.5 mapped-dimension iterated-subscript check. Bundling these keeps
/// `walk_all_in_expr`'s signature short (the only *mutable* state -- the
/// `lookup_dims` cache and the `sites` accumulator -- stays out of band).
struct WalkCtx<'a> {
    variables: &'a HashMap<Ident<Canonical>, crate::variable::Variable>,
    /// The target equation's iterated dimensions (canonical names, in the
    /// order they appear on `Ast::ApplyToAll` / `Ast::Arrayed`). Empty for
    /// `Ast::Scalar` -- a scalar target has no iterated-dimension subscript.
    target_iterated_dims: Vec<String>,
    dim_ctx: &'a crate::dimensions::DimensionsContext,
}

/// Walk a target's AST once and bucket every reference to a model variable
/// (by source canonical name) into [`ReferenceSite`]s.
///
/// This is the production walker the IR builds on: rather than walking once
/// per `(from, to)` edge, it walks each `to`'s AST a single time and records
/// sites for every `from` it references. Subscript shapes are classified
/// per-source via [`classify_iterated_dim_shape`] (the GH #511 iterated-
/// dimension same-element case) falling back to [`classify_subscript_shape`]
/// (`lookup_dims` resolves a referenced variable's dimensions on demand for
/// the literal-subscript / position checks); `in_reducer` propagates through
/// `builtin_routes_through_agg(builtin)` (SIZE excluded -- its result doesn't
/// depend on element values; RANK included via its array-valued agg path).
/// Walk order is
/// left-to-right DFS over the AST, matching `enumerate_agg_nodes`, so the
/// per-source site `Vec`s are deterministic (a salsa requirement on the
/// cached IR result).
///
/// This convenience wrapper discards the [`RawOccurrence`] stream (the finer
/// per-occurrence enumeration) that shares the same single walk; production
/// (`model_ltm_reference_sites`) uses
/// [`collect_all_reference_sites_and_occurrences`] directly. Only the per-edge
/// tests and the A1 classifier-agreement gate consume this narrow view, so it
/// is test-only.
#[cfg(test)]
pub(crate) fn collect_all_reference_sites(
    target_var: &crate::variable::Variable,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
    dim_ctx: &crate::dimensions::DimensionsContext,
    lookup_dims: &mut impl FnMut(&str) -> Vec<crate::dimensions::Dimension>,
) -> HashMap<String, Vec<ReferenceSite>> {
    collect_all_reference_sites_and_occurrences(target_var, variables, dim_ctx, lookup_dims).0
}

// ── The LTM front door: `SiteId` addressability ────────────────────────────

/// The number of children one [`SiteId`] path component can tell apart.
///
/// A component is a `u16`, so an equation needing more than this many distinct
/// child positions at one level cannot be addressed: two occurrences would share
/// a path, and the ceteris-paribus wrap would then hold or freeze the WRONG
/// reference and emit a plausible, wrong link score -- silent corruption, the
/// failure class this IR exists to eliminate.
///
/// Rather than making that case safe by threading a reserved sentinel through
/// the IR and the wrap, LTM refuses such a model at the front door: the walk
/// records no occurrence for the offending equation ([`WalkAccum::record_occurrences`])
/// and `model_ltm_variables` emits no LTM variable for the model at all, with a
/// `Warning` naming the variable. Every `SiteId` component the walk pushes is
/// therefore in range by construction -- which is what lets the pushes be plain
/// conversions and the consumer's `ltm_augment::child_path` be total.
///
/// Widening the component to `u32` was the alternative. It is rejected: the
/// occurrence IR is salsa-cached per model and every occurrence carries a boxed
/// path, so widening doubles that footprint on every model to serve a width no
/// real model reaches -- and it would not remove the case, only move it.
pub(crate) const MAX_SITE_CHILDREN: usize = u16::MAX as usize + 1;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`site_children_limit`], installed by
    /// [`SiteChildrenLimitGuard`]. Lets a test trip the front door with a tiny
    /// fixture instead of building an equation wide enough to trip the
    /// production constant (per docs/dev/rust.md#test-time-budgets).
    static SITE_CHILDREN_LIMIT_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The `SiteId` child-count limit in force. [`MAX_SITE_CHILDREN`] in production
/// builds; in `#[cfg(test)]` builds an active [`SiteChildrenLimitGuard`]
/// override takes precedence.
pub(crate) fn site_children_limit() -> usize {
    #[cfg(test)]
    {
        if let Some(limit) = SITE_CHILDREN_LIMIT_OVERRIDE.with(|c| c.get()) {
            return limit;
        }
    }
    MAX_SITE_CHILDREN
}

/// RAII guard (test-only) that lowers [`site_children_limit`] for the current
/// thread for the guard's lifetime, restoring the previous value on drop -- so a
/// panicking test does not leak the override to the next test reusing the
/// thread.
///
/// `model_ltm_reference_sites` and `model_ltm_variables` are salsa-memoized, so
/// the guard must outlive every call in the test whose limit it controls (a
/// later call on the same `db` would otherwise return the memoized
/// tiny-limit result regardless of the override state).
#[cfg(test)]
pub(crate) struct SiteChildrenLimitGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl SiteChildrenLimitGuard {
    pub(crate) fn new(limit: usize) -> Self {
        let prev = SITE_CHILDREN_LIMIT_OVERRIDE.with(|c| c.replace(Some(limit)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for SiteChildrenLimitGuard {
    fn drop(&mut self) {
        SITE_CHILDREN_LIMIT_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Which child axis of a [`SiteId`] path an equation overran.
///
/// The rows are exactly the walk's own variable-width `push` sites. Every OTHER
/// push is a literal bounded by the node's shape -- an `Ast::Scalar` /
/// `Ast::ApplyToAll` target's single slot `0`, `Op1`'s one operand, `Op2`'s two,
/// `Expr2::If`'s three, and an `IndexExpr2::Range`'s two halves -- so those axes
/// need no check and have no arm here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, salsa::Update)]
pub(crate) enum SiteWidthAxis {
    /// An `Ast::Arrayed` target's equation slots: one per `<element>` equation,
    /// plus the trailing default slot.
    ArrayedSlots,
    /// One builtin call's ordered contents. `BuiltinFn::Mean` is the only
    /// variadic variant (every other holds at most five fixed children), so
    /// `MEAN(a, b, ...)` is the only equation shape that can reach this.
    BuiltinContents,
    /// One `Expr2::Subscript`'s index list. The parser accepts any number of
    /// comma-separated indices and `Expr2::from` does NOT narrow it to the
    /// subscripted variable's declared arity (it simply ignores indices past
    /// `dims.len()`), so this axis is bounded by the AST alone.
    SubscriptIndices,
}

impl SiteWidthAxis {
    /// Human-readable plural for the diagnostic message.
    pub(crate) fn describe(self) -> &'static str {
        match self {
            SiteWidthAxis::ArrayedSlots => "equation slots (per-element equations plus a default)",
            SiteWidthAxis::BuiltinContents => "arguments to one builtin call",
            SiteWidthAxis::SubscriptIndices => "indices in one subscript",
        }
    }
}

/// Why a model's equations cannot be addressed by a [`SiteId`], as reported by
/// the LTM front door.
///
/// Carries the first offending variable in the walk's own deterministic order
/// (variables canonical-sorted, then left-to-right DFS), so the emitted
/// diagnostic is reproducible across processes.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct SiteWidthRejection {
    /// Canonical name of the target variable whose equation is too wide.
    pub variable: String,
    /// Which axis overran.
    pub axis: SiteWidthAxis,
    /// How many children that axis needed.
    pub count: usize,
    /// The limit in force when the rejection was recorded.
    pub limit: usize,
}

/// The first place `ast` needs more `SiteId` child positions at one level than
/// `limit` can tell apart, in the walk's own left-to-right order.
///
/// Path-free by construction: it counts children and never builds a path. That
/// is what makes it a FRONT door -- a truncated path the walk had already
/// recorded would be the aliasing bug itself, not a check against it.
///
/// Its descent is a strict SUPERSET of [`walk_all_in_expr`]'s, and that is the
/// reason this is a separate pre-pass rather than a counter carried on the walk
/// itself: the CONSUMER's path cursor (`ltm_augment::child_path`) descends
/// through nodes the producer never enters, so a check that saw only what the
/// producer sees could not bound the consumer's paths. Two places:
///
/// - a `LOOKUP` **table argument**. The walk pushes a path component for it and
///   then records nothing (`BuiltinContents::LookupTable(_) => {}`) without
///   recursing -- static table data is not a causal edge. The wrap DOES recurse:
///   `ltm_augment::wrap_non_matching_in_previous`'s LOOKUP arm hands the table
///   argument to `post_transform::pin_only_source_refs` at `child_path(path, i)`,
///   and that function's own `App` arm then descends into every argument at
///   `child_path(path, i)` again. So a builtin nested under a table argument is
///   reachable by consumer path indices and by no producer path at all.
/// - every **subscript index**. The walk skips an index that resolves to a
///   literal element of the subscripted variable's axis; the wrap's skip is a
///   different predicate (the occurrence's `Pinned` axes, or what
///   `pin_source_subscript_indices` leaves unresolved), so the two sets are not
///   the same and only the union bounds both.
///
/// Descending in both is therefore load-bearing, not conservatism. It is also
/// sound in the other direction: descending where NEITHER side goes could only
/// over-reject a node no path names, never miss one.
fn ast_site_width_rejection(
    ast: &crate::ast::Ast<crate::ast::Expr2>,
    limit: usize,
) -> Option<(SiteWidthAxis, usize)> {
    use crate::ast::Ast;

    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => expr_site_width_rejection(expr, limit),
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            // Slots are numbered in canonical element-key-sorted order with the
            // default equation the slot AFTER the last element. Reserve that
            // trailing slot unconditionally: `db::ltm::link_scores`'
            // `ArrayedSlotMap` computes it as `keys.len()` whether or not a
            // default equation exists, and routes an unlisted element there.
            let slots = per_elem.len() + 1;
            if slots > limit {
                return Some((SiteWidthAxis::ArrayedSlots, slots));
            }
            // Visit slots in the walk's own order so the reported rejection is
            // deterministic (a `HashMap` iteration order is not).
            let mut elem_keys: Vec<_> = per_elem.keys().collect();
            elem_keys.sort();
            for k in elem_keys {
                if let Some(rejection) = expr_site_width_rejection(&per_elem[k], limit) {
                    return Some(rejection);
                }
            }
            default_expr
                .as_ref()
                .and_then(|expr| expr_site_width_rejection(expr, limit))
        }
    }
}

/// [`ast_site_width_rejection`]'s recursive half over one expression tree.
///
/// The `match` is exhaustive with no catch-all arm, so a new `Expr2` variant is
/// a compile error here rather than a silently unchecked axis.
fn expr_site_width_rejection(
    expr: &crate::ast::Expr2,
    limit: usize,
) -> Option<(SiteWidthAxis, usize)> {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::builtins::{BuiltinContents, walk_builtin_expr};

    match expr {
        Expr2::Const(..) | Expr2::Var(..) => None,
        Expr2::Subscript(_, indices, _, _) => {
            if indices.len() > limit {
                return Some((SiteWidthAxis::SubscriptIndices, indices.len()));
            }
            indices.iter().find_map(|idx| match idx {
                IndexExpr2::Expr(e) => expr_site_width_rejection(e, limit),
                IndexExpr2::Range(l, r, _) => expr_site_width_rejection(l, limit)
                    .or_else(|| expr_site_width_rejection(r, limit)),
                IndexExpr2::Wildcard(_)
                | IndexExpr2::StarRange(_, _)
                | IndexExpr2::DimPosition(_, _) => None,
            })
        }
        Expr2::App(builtin, _, _) => {
            // Count with the walker's OWN enumerator, so the front door and the
            // walk cannot disagree about how many children a builtin has.
            let mut children: Vec<&Expr2> = Vec::new();
            let mut n: usize = 0;
            walk_builtin_expr(builtin, |contents| {
                n += 1;
                match contents {
                    BuiltinContents::Expr(e) | BuiltinContents::LookupTable(e) => children.push(e),
                    BuiltinContents::Ident(_, _) => {}
                }
            });
            if n > limit {
                return Some((SiteWidthAxis::BuiltinContents, n));
            }
            children
                .into_iter()
                .find_map(|e| expr_site_width_rejection(e, limit))
        }
        Expr2::Op1(_, operand, _, _) => expr_site_width_rejection(operand, limit),
        Expr2::Op2(_, left, right, _, _) => expr_site_width_rejection(left, limit)
            .or_else(|| expr_site_width_rejection(right, limit)),
        Expr2::If(cond, then_e, else_e, _, _) => expr_site_width_rejection(cond, limit)
            .or_else(|| expr_site_width_rejection(then_e, limit))
            .or_else(|| expr_site_width_rejection(else_e, limit)),
    }
}

/// The bundled accumulators the single walk feeds: the per-source
/// [`ReferenceSite`] buckets (the existing per-edge view) and the flat,
/// document-ordered [`RawOccurrence`] stream (the per-occurrence view). The
/// `path` is the running structural [`SiteId`] child-index path (slot prefix
/// plus descent chain); it is `push`/`pop`ed as the walk descends, so at the
/// moment an occurrence is recorded it names exactly that occurrence's node.
struct WalkAccum<'a> {
    sites: &'a mut HashMap<String, Vec<ReferenceSite>>,
    occurrences: &'a mut Vec<RawOccurrence>,
    path: Vec<u16>,
    /// `false` when the LTM front door rejected this target's equation as too
    /// wide for a [`SiteId`] to address (see [`ast_site_width_rejection`]).
    ///
    /// Only the OCCURRENCE view is dropped; `push_ref_site` keeps running, so
    /// the per-edge view -- and therefore `model_edge_shapes` and the element
    /// causal graph -- still sees every reference with its real shape. That view
    /// is name-keyed and carries no path, so it is unaffected by the width;
    /// dropping it too would leave the IR with no entry for the edge, and
    /// consumers default a missing entry to a single `Bare` site, which would
    /// MISCLASSIFY a `FixedIndex`/`DynamicIndex` reference and emit wrong
    /// element edges. The occurrence view instead records NOTHING, so no two
    /// occurrences of a rejected equation can share a `SiteId` -- and
    /// `model_ltm_variables` refuses the whole model, so no consumer ever reads
    /// an occurrence stream that is missing entries.
    record_occurrences: bool,
}

impl WalkAccum<'_> {
    /// Record the per-EDGE view of a reference (name-keyed, feeds
    /// `model_edge_shapes` and the element causal graph).
    ///
    /// ASYMMETRY WITH [`WalkAccum::push_occurrence`] -- do not collapse the two.
    /// A missing entry means different things in the two views. Here, absence
    /// does NOT mean "no reference": consumers that find no IR entry for an edge
    /// fall back to a single `Bare` site, so skipping a `FixedIndex`/`DynamicIndex`
    /// reference MISCLASSIFIES it and emits wrong element edges and link scores.
    /// In the occurrence view, absence is safe -- the wrap treats a miss as "not
    /// a recorded causal reference" and a front-door-rejected equation records
    /// none at all. That is why `record_occurrences` gates only
    /// `push_occurrence`.
    fn push_ref_site(
        &mut self,
        from: &str,
        shape: RefShape,
        target_element: Option<&str>,
        reducer_keys: &[String],
    ) {
        self.sites
            .entry(from.to_string())
            .or_default()
            .push(ReferenceSite {
                shape,
                target_element: target_element.map(|s| s.to_string()),
                in_reducer: !reducer_keys.is_empty(),
                reducer_keys: reducer_keys.to_vec(),
            });
    }

    /// Record the per-OCCURRENCE view of a reference (SiteId-keyed, consumed by
    /// the ceteris-paribus wrap via `OccurrenceLookup`).
    ///
    /// ASYMMETRY WITH [`WalkAccum::push_ref_site`] -- see its docs. Absence here
    /// is safe (the wrap treats a miss as "not a recorded causal reference", and
    /// a live-source subscript miss additionally trips the loud
    /// `missing_occurrence` guard), whereas absence in the per-edge view
    /// silently misclassifies the edge as `Bare`. Suppression therefore applies
    /// to this view only.
    fn push_occurrence(
        &mut self,
        reference: OccurrenceRef,
        shape: RefShape,
        axes: Vec<OccurrenceAxis>,
        reducer_keys: &[String],
        index_nested: bool,
    ) {
        // A front-door-rejected equation records NO occurrence: its paths could
        // not tell every child apart, and two occurrences sharing a `SiteId` is
        // exactly the aliasing the identity exists to prevent. See
        // `record_occurrences`.
        if !self.record_occurrences {
            return;
        }
        self.occurrences.push(RawOccurrence {
            site_id: SiteId(self.path.clone().into_boxed_slice()),
            reference,
            shape,
            axes,
            in_reducer: !reducer_keys.is_empty(),
            index_nested,
        });
    }
}

/// The three views one target-equation walk produces: the per-EDGE
/// [`ReferenceSite`] buckets (name-keyed, path-free), the flat document-ordered
/// [`RawOccurrence`] stream (`SiteId`-keyed), and the LTM front door's verdict on
/// that equation (`Some((axis, count))` when it needs more children at one level
/// than a `SiteId` component can tell apart).
type WalkedTarget = (
    HashMap<String, Vec<ReferenceSite>>,
    Vec<RawOccurrence>,
    Option<(SiteWidthAxis, usize)>,
);

/// Walk a target's AST once, producing BOTH the per-source [`ReferenceSite`]
/// map (the per-edge view current consumers read) AND the flat, document-order
/// [`RawOccurrence`] stream (the per-occurrence view the ceteris-paribus
/// transform will consume in A2b). Both come from the same single left-to-right
/// DFS, so there is no added pass. The occurrence stream is a superset of the
/// causal references in the map -- it also enumerates module-qualified output
/// composites (which are not model-variable keys) -- and it OMITS an index
/// token that is a literal element selector (the A2a bug fix; see the
/// `Subscript` arm of [`walk_all_in_expr`]).
///
/// The third return is the LTM front door's verdict on THIS equation
/// ([`ast_site_width_rejection`], run before the walk starts). When it is
/// `Some`, the occurrence stream is empty by construction -- no `SiteId` is
/// minted for an equation whose paths could not tell every child apart -- while
/// the per-edge map is complete as always.
fn collect_all_reference_sites_and_occurrences(
    target_var: &crate::variable::Variable,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
    dim_ctx: &crate::dimensions::DimensionsContext,
    lookup_dims: &mut impl FnMut(&str) -> Vec<crate::dimensions::Dimension>,
) -> WalkedTarget {
    let mut sites: HashMap<String, Vec<ReferenceSite>> = HashMap::new();
    let mut occurrences: Vec<RawOccurrence> = Vec::new();
    let Some(ast) = target_var.ast() else {
        return (sites, occurrences, None);
    };
    // The front door, before a single path component is pushed.
    let width_rejection = ast_site_width_rejection(ast, site_children_limit());
    // The target equation's iterated dimensions drive the #511 iterated-
    // subscript recognition; `Ast::Scalar` has none.
    let target_iterated_dims: Vec<String> = match ast {
        crate::ast::Ast::Scalar(_) => Vec::new(),
        crate::ast::Ast::ApplyToAll(dims, _) | crate::ast::Ast::Arrayed(dims, _, _, _) => {
            dims.iter().map(|d| d.name().to_string()).collect()
        }
    };
    let ctx = WalkCtx {
        variables,
        target_iterated_dims,
        dim_ctx,
    };
    // The `WalkAccum` borrows `sites`/`occurrences`; scope it so those borrows
    // end before we move the two out at the end.
    {
        let mut acc = WalkAccum {
            sites: &mut sites,
            occurrences: &mut occurrences,
            path: Vec::new(),
            record_occurrences: width_rejection.is_none(),
        };
        match ast {
            crate::ast::Ast::Scalar(expr) | crate::ast::Ast::ApplyToAll(_, expr) => {
                // Slot 0: the single body of a scalar / apply-to-all target.
                let mut reducer_keys = Vec::new();
                acc.path.push(0);
                walk_all_in_expr(
                    expr,
                    &ctx,
                    lookup_dims,
                    None,
                    &mut reducer_keys,
                    false,
                    &mut acc,
                );
                acc.path.pop();
            }
            crate::ast::Ast::Arrayed(_, subscript_map, default_expr, _) => {
                // Per-element expressions: visit slots in canonical element-key
                // order so the per-source site Vecs (and the occurrence stream /
                // its `SiteId`s) are deterministic. The slot index is the first
                // `SiteId` path element; the front door above refused this
                // equation if it needs more slots than `MAX_SITE_CHILDREN` can
                // tell apart, so both conversions are in range by construction.
                let mut elem_keys: Vec<_> = subscript_map.keys().collect();
                elem_keys.sort();
                for (slot, k) in elem_keys.iter().enumerate() {
                    let mut reducer_keys = Vec::new();
                    acc.path.push(slot as u16);
                    walk_all_in_expr(
                        &subscript_map[*k],
                        &ctx,
                        lookup_dims,
                        Some(k.as_str()),
                        &mut reducer_keys,
                        false,
                        &mut acc,
                    );
                    acc.path.pop();
                }
                if let Some(default) = default_expr {
                    // The default expression is the slot after the last element.
                    let mut reducer_keys = Vec::new();
                    acc.path.push(elem_keys.len() as u16);
                    walk_all_in_expr(
                        default,
                        &ctx,
                        lookup_dims,
                        None,
                        &mut reducer_keys,
                        false,
                        &mut acc,
                    );
                    acc.path.pop();
                }
            }
        }
    }
    (sites, occurrences, width_rejection)
}

/// If `ident` is a module-qualified output composite (`module·port`, e.g.
/// `mod·out1` or a SMOOTH/DELAY-expanded `$⁚s⁚0⁚smth1·output`) whose head
/// names a model-variable of kind Module, return `(module, port)` (both as
/// they appear in the composite). `module·port` is never itself a
/// model-variable key, so the walker records no `ReferenceSite` for it; this
/// lets the occurrence stream enumerate it as an [`OccurrenceRef::ModuleOutput`]
/// -- the deterministic, document-ordered IR source of truth for the by-name
/// live channel `db::module_link_score_equation` selects today via a
/// per-process-random HashSet `.find()` (GH #971).
fn module_output_parts(ident: &str, ctx: &WalkCtx<'_>) -> Option<(String, String)> {
    let pos = ident.find('\u{00B7}')?;
    let module = &ident[..pos];
    let port = &ident[pos + '\u{00B7}'.len_utf8()..];
    if port.is_empty() {
        return None;
    }
    let module_ident = Ident::<Canonical>::new(module);
    ctx.variables
        .get(&module_ident)
        .filter(|v| v.is_module())
        .map(|_| (module.to_string(), port.to_string()))
}

/// Per-index access classification for a subscript occurrence -- the extended
/// [`OccurrenceAxis`] vocabulary (one entry per index). Each axis reuses the
/// shared [`crate::ltm_agg::classify_axis_access`] classifier (so the reducer
/// and direct-reference paths never disagree) and, where that returns `None`,
/// distinguishes a position-mismatched *iterated* index (a bare `Var` naming a
/// target-iterated dimension that does not line up positionally -- the GH #526
/// `Mismatch` case) from a genuinely dynamic one (`pop[i+1]`, a range, `@N`).
/// An index that overflows the source's declared arity is handled the same
/// way (an extra iterated-dim name is an arity mismatch, else dynamic).
fn classify_occurrence_axes(
    indices: &[crate::ast::IndexExpr2],
    source_dims: &[crate::dimensions::Dimension],
    target_iterated_dims: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Vec<OccurrenceAxis> {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::ltm_agg::classify_axis_access;

    // Mirror `classify_axis_access`'s iterated-dim recognition (a bare `Var`
    // whose name is one of the target's iterated dims) so a position-mismatch
    // is recorded as `MismatchedIterated`, not collapsed into `Dynamic`.
    let mismatched_or_dynamic = |idx: &IndexExpr2| -> OccurrenceAxis {
        if let IndexExpr2::Expr(Expr2::Var(name, _, _)) = idx
            && target_iterated_dims.iter().any(|t| t == name.as_str())
        {
            return OccurrenceAxis::MismatchedIterated {
                dim: name.as_str().to_string(),
            };
        }
        OccurrenceAxis::Dynamic
    };

    indices
        .iter()
        .enumerate()
        .map(|(i, idx)| match source_dims.get(i) {
            Some(axis_dim) => {
                match classify_axis_access(idx, axis_dim, target_iterated_dims, dim_ctx) {
                    Some(ar) => OccurrenceAxis::from_axis_read(ar),
                    None => mismatched_or_dynamic(idx),
                }
            }
            // More indices than declared dims: an arity mismatch. A bare
            // iterated-dim name here is `MismatchedIterated`; anything else is
            // dynamic (there is no axis to resolve a literal against).
            None => mismatched_or_dynamic(idx),
        })
        .collect()
}

/// Recursive helper for [`collect_all_reference_sites_and_occurrences`]:
/// left-to-right DFS over an `Expr2` tree, pushing one [`ReferenceSite`] per
/// model-variable reference (bucketed by source name) AND one
/// [`RawOccurrence`] per causal reference occurrence.
///
/// `in_reducer` becomes `true` (via `reducer_keys` non-empty) once we descend
/// into a builtin that can route through an aggregate node and stays sticky (a
/// reducer nested in another reducer's arg is still inside *a* reducer); `SIZE`
/// does not route through an agg, so it never sets the flag. `index_nested`
/// becomes sticky-true once we descend into a subscript index expression.
#[allow(clippy::too_many_arguments)]
fn walk_all_in_expr(
    expr: &crate::ast::Expr2,
    ctx: &WalkCtx<'_>,
    lookup_dims: &mut impl FnMut(&str) -> Vec<crate::dimensions::Dimension>,
    target_element: Option<&str>,
    reducer_keys: &mut Vec<String>,
    index_nested: bool,
    acc: &mut WalkAccum,
) {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::builtins::{BuiltinContents, walk_builtin_expr};

    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _, _) => {
            if ctx.variables.contains_key(ident) {
                acc.push_ref_site(ident.as_str(), RefShape::Bare, target_element, reducer_keys);
                acc.push_occurrence(
                    OccurrenceRef::Variable(ident.as_str().to_string()),
                    RefShape::Bare,
                    Vec::new(),
                    reducer_keys,
                    index_nested,
                );
            } else if let Some((module, port)) = module_output_parts(ident.as_str(), ctx) {
                acc.push_occurrence(
                    OccurrenceRef::ModuleOutput {
                        module,
                        port,
                        composite: ident.as_str().to_string(),
                    },
                    RefShape::Bare,
                    Vec::new(),
                    reducer_keys,
                    index_nested,
                );
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            let from_dims = lookup_dims(ident.as_str());
            if ctx.variables.contains_key(ident) {
                // #511: an iterated-dimension subscript (`row_sum[Region]`
                // inside `growth[Region,Age]`) reads the same source element
                // for the slot being computed -- classify it `Bare` so it
                // goes through `emit_edges_for_reference`'s same-element
                // projection. A non-iterated subscript keeps its
                // literal/wildcard/dynamic classification.
                let shape = classify_iterated_dim_shape(
                    indices,
                    &from_dims,
                    &ctx.target_iterated_dims,
                    ctx.dim_ctx,
                )
                .unwrap_or_else(|| classify_subscript_shape(indices, &from_dims));
                let axes = classify_occurrence_axes(
                    indices,
                    &from_dims,
                    &ctx.target_iterated_dims,
                    ctx.dim_ctx,
                );
                acc.push_ref_site(ident.as_str(), shape.clone(), target_element, reducer_keys);
                acc.push_occurrence(
                    OccurrenceRef::Variable(ident.as_str().to_string()),
                    shape,
                    axes,
                    reducer_keys,
                    index_nested,
                );
            } else if let Some((module, port)) = module_output_parts(ident.as_str(), ctx) {
                // Classify a subscripted module output's axes the SAME way a
                // model-variable subscript's are (empty `from_dims`, since a
                // `module·port` composite is not a variable key -- so a bare
                // iterated-dim index lands `MismatchedIterated`, an over-arity
                // or non-iterated index `Dynamic`). This preserves byte parity
                // with the retired Expr0 `classify_other_dep_iterated_dim_subscript`,
                // which likewise built iterated axes for ANY subscripted
                // iterated-dim head: the wrap's `other_dep_verdict` on those
                // axes (dep arity always `None` for a non-variable head) then
                // permissively collapses `mod·out[Region]` to `PREVIOUS(mod·out)`
                // rather than freezing the uncompilable dim-name subscript. An
                // empty `axes` here (the pre-flip stage-2 state) instead derived
                // `NotIterated`, a silent divergence on an arrayed user-module
                // output referenced by an iterated subscript. Pinned at the IR
                // level by `subscripted_arrayed_module_output_axes_derive_collapse`;
                // no simulate-corpus fixture exercises the end-to-end score, so
                // both LTM suites stayed byte-green either way.
                let axes = classify_occurrence_axes(
                    indices,
                    &from_dims,
                    &ctx.target_iterated_dims,
                    ctx.dim_ctx,
                );
                acc.push_occurrence(
                    OccurrenceRef::ModuleOutput {
                        module,
                        port,
                        composite: ident.as_str().to_string(),
                    },
                    RefShape::Bare,
                    axes,
                    reducer_keys,
                    index_nested,
                );
            }
            // Recurse into the indices. An index that resolves to a literal
            // element of the subscripted variable's axis is an element
            // SELECTOR -- execution resolves it to a static offset, element
            // taking priority over any like-named variable
            // (`compiler::subscript`, verified by simulation) -- NOT a causal
            // reference, so skip it and mint no `elem -> to` site. This ends a
            // walker/transform disagreement rather than removing a
            // consumer-visible edge: the ceteris-paribus transform already
            // treated the token as an element selector, and variable-level dep
            // extraction (`variable::classify_dependencies` over the project
            // dims) already filtered it, so the pre-fix walker site was an
            // orphan no keyed consumer read. It fires only when a subscript
            // element name ALSO names a model variable (`arr[nyc]` with a
            // variable `nyc`); otherwise nothing was pushed here anyway.
            // Everything else is genuine index content: recurse with
            // `index_nested = true`, so a model-variable dynamic index
            // (`arr[from]`) is marked reachable only through a subscript index.
            //
            // An index list is NOT narrowed to the subscripted variable's
            // declared arity by `Expr2::from` (it ignores indices past
            // `dims.len()`), so this is a third variable-width axis -- covered
            // by the same front door, which makes the conversion in range.
            for (i, idx) in indices.iter().enumerate() {
                if resolve_literal_index(idx, &from_dims).is_some() {
                    continue;
                }
                acc.path.push(i as u16);
                match idx {
                    IndexExpr2::Expr(e) => walk_all_in_expr(
                        e,
                        ctx,
                        lookup_dims,
                        target_element,
                        reducer_keys,
                        true,
                        acc,
                    ),
                    IndexExpr2::Range(l, r, _) => {
                        acc.path.push(0);
                        walk_all_in_expr(
                            l,
                            ctx,
                            lookup_dims,
                            target_element,
                            reducer_keys,
                            true,
                            acc,
                        );
                        acc.path.pop();
                        acc.path.push(1);
                        walk_all_in_expr(
                            r,
                            ctx,
                            lookup_dims,
                            target_element,
                            reducer_keys,
                            true,
                            acc,
                        );
                        acc.path.pop();
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
                acc.path.pop();
            }
        }
        Expr2::App(builtin, _, _) => {
            let pushed_reducer_key = crate::ltm_agg::builtin_routes_through_agg(builtin);
            if pushed_reducer_key {
                reducer_keys.push(crate::patch::expr2_to_string(expr));
            }
            // Builtin arity is one of the three child counts not bounded by the
            // AST node's own shape (`BuiltinFn::Mean` is the only variadic
            // variant). The LTM front door -- `ast_site_width_rejection`, run
            // before this walk -- refuses an equation whose arity exceeds
            // `MAX_SITE_CHILDREN`, so the conversion below is in range by
            // construction and no two children can share a path component.
            let mut child_n: usize = 0;
            walk_builtin_expr(builtin, |contents| {
                acc.path.push(child_n as u16);
                match contents {
                    BuiltinContents::Ident(id, _) => {
                        let canonical = Ident::<Canonical>::new(id);
                        if ctx.variables.contains_key(&canonical) {
                            acc.push_ref_site(id, RefShape::Bare, target_element, reducer_keys);
                            acc.push_occurrence(
                                OccurrenceRef::Variable(id.to_string()),
                                RefShape::Bare,
                                Vec::new(),
                                reducer_keys,
                                index_nested,
                            );
                        } else if let Some((module, port)) = module_output_parts(id, ctx) {
                            acc.push_occurrence(
                                OccurrenceRef::ModuleOutput {
                                    module,
                                    port,
                                    composite: id.to_string(),
                                },
                                RefShape::Bare,
                                Vec::new(),
                                reducer_keys,
                                index_nested,
                            );
                        }
                    }
                    BuiltinContents::Expr(sub_expr) => walk_all_in_expr(
                        sub_expr,
                        ctx,
                        lookup_dims,
                        target_element,
                        reducer_keys,
                        index_nested,
                        acc,
                    ),
                    // A graphical-function table reference is static data, not
                    // a causal edge: emit no `from -> consumer` reference site
                    // for the table itself (only the index argument carries
                    // real edges).
                    BuiltinContents::LookupTable(_) => {}
                }
                acc.path.pop();
                child_n += 1;
            });
            if pushed_reducer_key {
                reducer_keys.pop();
            }
        }
        Expr2::Op1(_, operand, _, _) => {
            acc.path.push(0);
            walk_all_in_expr(
                operand,
                ctx,
                lookup_dims,
                target_element,
                reducer_keys,
                index_nested,
                acc,
            );
            acc.path.pop();
        }
        Expr2::Op2(_, left, right, _, _) => {
            acc.path.push(0);
            walk_all_in_expr(
                left,
                ctx,
                lookup_dims,
                target_element,
                reducer_keys,
                index_nested,
                acc,
            );
            acc.path.pop();
            acc.path.push(1);
            walk_all_in_expr(
                right,
                ctx,
                lookup_dims,
                target_element,
                reducer_keys,
                index_nested,
                acc,
            );
            acc.path.pop();
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            for (child, sub) in [cond, then_e, else_e].into_iter().enumerate() {
                acc.path.push(child as u16);
                walk_all_in_expr(
                    sub,
                    ctx,
                    lookup_dims,
                    target_element,
                    reducer_keys,
                    index_nested,
                    acc,
                );
                acc.path.pop();
            }
        }
    }
}

// ── The classified-site IR ─────────────────────────────────────────────────

/// One classified reference site for a `(from, to)` causal edge.
///
/// Successor of `analysis::ReferenceSite`, generalized to fold the
/// `in_reducer` flag plus the hoisting decision into [`SiteRouting`].
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct ClassifiedSite {
    /// The per-reference access shape: `Bare`, `FixedIndex(elems)`,
    /// `Wildcard`, or `DynamicIndex`.
    pub shape: RefShape,
    /// `Some(elem)` when the reference sits in an `Ast::Arrayed` per-element
    /// slot (the canonical element name / comma-separated tuple of the
    /// target element being defined); `None` for `Ast::Scalar`/`ApplyToAll`.
    pub target_element: Option<String>,
    /// How consumers should treat this reference.
    pub routing: SiteRouting,
}

/// How a [`ClassifiedSite`] feeds the element graph and link scores.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) enum SiteRouting {
    /// Consumers use `shape` / `target_element` directly: the element graph
    /// emits `emit_edges_for_reference`, the link scorer emits the per-shape
    /// link score for `shape`.
    Direct,
    /// Consumers route `from[..] → agg.name` + `agg.name → to[e]` (the
    /// synthetic aggregate-node hop). The site's `shape` is the (Wildcard-ish)
    /// syntactic shape but the element graph ignores it; the link scorer
    /// emits the two agg halves and suppresses the (always-`Wildcard`) shape
    /// from the per-shape pass.
    ///
    /// An `in_reducer` reference whose `(from, to)` edge has *multiple*
    /// synthetic aggs reading `from` is split into one `ThroughAgg` site per
    /// such agg -- exactly mirroring the old `for agg in &routed_aggs`
    /// loop in `model_element_causal_edges` (which routed every `in_reducer`
    /// site through every routed agg).
    ThroughAgg {
        /// The synthetic agg this site routes through.
        agg: AggRef,
    },
}

/// Index into `AggNodesResult.aggs`. The IR records the *synthetic* agg a
/// `ThroughAgg` site routes through; a consumer that wants the unique set of
/// routed aggs for a `(from, to)` edge dedups these itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, salsa::Update)]
pub(crate) struct AggRef(pub usize);

// ── Per-occurrence enumeration (Track A2a) ─────────────────────────────────
//
// The per-`(from, to)`-edge [`ClassifiedSite`] view above serves the Expr2
// edge/routing consumers (`model_element_causal_edges`, `model_edge_shapes`,
// `emit_per_shape_link_scores`). Track A of the "Deleting the Round Trips"
// plan additionally needs the SAME single AST walk to feed the
// ceteris-paribus transform (`ltm_augment.rs`), which today re-derives access
// shape from re-parsed `Expr0`. That consumer operates at a finer granularity
// than a `(from, to)` edge: it selects which *occurrences* of a source stay
// live and PREVIOUS-wraps the rest, per reference occurrence over the WHOLE
// target equation. The [`OccurrenceSite`] records below carry every fact the
// spec's `fig2-answer.md` §3 identifies for that switch (A2b consumes them):
// stable occurrence identity, per-axis access with a mismatched-iterated arm,
// reducer-enclosure / already-lagged / index-position context, and the
// module-qualified by-name live channel. They ride ALONGSIDE `sites`; no
// existing consumer reads them yet.

/// Stable identity of one reference occurrence within a single target
/// equation: the left-to-right child-index path from the target's slot root
/// down to the occurrence node.
///
/// The first element is the *slot* index -- for an `Ast::Scalar` /
/// `Ast::ApplyToAll` target the single body is slot `0`; for an
/// `Ast::Arrayed` target the per-element slots are numbered in canonical
/// element-key-sorted order and the (optional) default expression is the
/// slot after the last element. The remaining elements index each child on
/// the descent (operands of `Op1`/`Op2`, branches of `If`, ordered contents
/// of a builtin `App`, and the indices of a `Subscript`).
///
/// Determinism (a salsa requirement): the walk is a fixed left-to-right DFS
/// with slots visited in sorted key order, so the path is a pure function of
/// the AST -- no HashMap iteration order enters it. Two distinct occurrences
/// can share an access shape but never a `SiteId`, which is what lets the
/// transform name the normalizer (the first non-index-nested matching
/// occurrence) apart from later same-shape ones.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update)]
pub(crate) struct SiteId(pub Box<[u16]>);

/// What a per-occurrence reference *is* -- the "enumerable as a site at all"
/// decision the spec (§3) requires the unified type to settle once.
///
/// The walker records an occurrence ONLY for a genuine causal reference. A
/// subscript index that names a dimension element (`arr[nyc]`) is an element
/// *selector* resolved to a static offset at execution
/// (`compiler::subscript`, element takes priority over a like-named
/// variable), NOT a causal reference, so it is enumerated as NO occurrence at
/// all. That keeps the occurrence stream faithful to execution and in step
/// with the ceteris-paribus transform (which already treats the token as a
/// selector); it does not remove a consumer-visible edge, since variable-level
/// dep extraction already filtered the token so no keyed consumer ever read
/// the pre-fix `nyc -> to` walker site.
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) enum OccurrenceRef {
    /// A reference to a model variable (the causal `from`). Canonical name.
    Variable(String),
    /// A module-qualified output composite (`module·port`, including every
    /// SMOOTH/DELAY-expanded `$⁚s⁚0⁚smth1·output`). `module·port` is never a
    /// model-variable key, so the Expr2 walker records no `ClassifiedSite` for
    /// it and `db::module_link_score_equation` locates it BY NAME through a
    /// per-process-random `HashSet` `.find()` (GH #971). Enumerating it here,
    /// in document order, gives that by-name live channel a deterministic
    /// IR source of truth for A2b to consume.
    ModuleOutput {
        /// Canonical module-instance name (the head before the first `·`).
        module: String,
        /// Canonical output-port path (everything after the first `·`).
        port: String,
        /// The full `module·port` composite ident, verbatim.
        composite: String,
    },
}

/// Per-axis access classification for one subscript occurrence, extending
/// [`crate::ltm_agg::AxisRead`] with the position-mismatched-iterated arm the
/// coarse [`RefShape`] loses.
///
/// A transposed / arity-mismatched iterated index (`arr[D2,D1]` for `arr`
/// declared `[D1,D2]`) yields `None` from `classify_axis_access`, so the
/// subscript collapses to `RefShape::DynamicIndex` -- byte-identical to a
/// genuine dynamic index (`pop[i+1]`). Yet the transform must abandon the
/// changed-first partial for the first (freezing the wrong element is a
/// silent magnitude error, GH #526) while wrapping the second normally. The
/// [`MismatchedIterated`](OccurrenceAxis::MismatchedIterated) arm makes
/// `ltm_augment::classify_other_dep_iterated_dim_subscript`'s
/// `Collapse`/`Mismatch`/`NotIterated` verdict derivable from the IR -- but
/// the per-axis arms alone are NOT the verdict: the derivation also needs the
/// dep's declared arity and the target's iterated-dim count (both of which the
/// consuming transform holds, neither carried on the occurrence). Given a
/// subscript occurrence's `axes`, the target's iterated-dim count `T`, and the
/// dep's declared arity `A` (`None` = un-threadable: the dep is absent from
/// the variable map / has no declared dims), the verdict mirrors
/// `classify_other_dep_iterated_dim_subscript` (`ltm_augment.rs`) exactly:
///
/// 1. `NotIterated` (normal subscript handling) if `axes` is empty, or
///    `axes.len() > T` (mirrors the `indices.len() > target_iterated_dims.len()`
///    gate, ltm_augment.rs:268), or any axis is `Pinned` / `Reduced` /
///    `Dynamic` (the index is a literal element, wildcard, or dynamic
///    expression, not a bare target-iterated-dim name).
/// 2. otherwise (every axis `Iterated` or `MismatchedIterated`, `axes.len()
///    <= T`): un-threadable dep (`A` is `None`) ⇒ `Collapse` (the transform's
///    permissive fallback, ltm_augment.rs:284); else `axes.len() != A` ⇒
///    `Mismatch` (the arity check, ltm_augment.rs:288 -- checked BEFORE the
///    per-axis lineup); else all axes `Iterated` ⇒ `Collapse`, ≥1
///    `MismatchedIterated` ⇒ `Mismatch`.
///
/// The two arity guards are load-bearing, not decoration: `arr[D1]` for `arr`
/// declared `[D1,D2]` under an A2A-over-`[D1,D2]` target yields all-`Iterated`
/// axes yet is a `Mismatch` (under-arity, corner a); `arr[D1,D1]` for `arr`
/// `[D1,D2]` under an A2A-over-`[D1]` target yields a `MismatchedIterated` axis
/// yet is `NotIterated` (over-target-arity, corner b). Both are pinned by
/// `over_target_arity_iterated_subscript_is_not_iterated` /
/// `under_arity_iterated_subscript_is_mismatch_not_collapse` and the
/// `derive_other_dep_verdict` reference derivation in the tests. One entry per
/// subscript index (empty for a bare `Var` / module output).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) enum OccurrenceAxis {
    /// A literal element of this axis is read (`AxisRead::Pinned`).
    Pinned(String),
    /// Iterated over the target's dimension space, lined up by name or a
    /// positional mapping (`AxisRead::Iterated`).
    Iterated { dim: String, source_dim: String },
    /// A reduced axis (`*` / StarRange); present only inside reducer args
    /// (`AxisRead::Reduced`).
    Reduced { subset: Option<Vec<String>> },
    /// Names a target-iterated dimension but does NOT correspond positionally
    /// to this source axis (transposed, arity-mismatched, or a mapped pair
    /// without a usable positional correspondence). The coarse shape is
    /// `DynamicIndex`; the transform must NOT treat it as a genuine dynamic
    /// index (GH #526 `Mismatch`).
    MismatchedIterated { dim: String },
    /// A genuine dynamic / non-statically-describable index (`pop[i+1]`, a
    /// range, `@N`).
    Dynamic,
}

impl OccurrenceAxis {
    fn from_axis_read(ar: crate::ltm_agg::AxisRead) -> Self {
        use crate::ltm_agg::AxisRead;
        match ar {
            AxisRead::Pinned(e) => OccurrenceAxis::Pinned(e),
            AxisRead::Iterated { dim, source_dim } => OccurrenceAxis::Iterated { dim, source_dim },
            AxisRead::Reduced { subset } => OccurrenceAxis::Reduced { subset },
        }
    }
}

/// The `Collapse` / `Mismatch` / `NotIterated` verdict for an iterated-dimension
/// subscript on a NON-live-source dependency (`pop[Region,Age]` in
/// `growth[Region,Age] = row_sum[Region] * c * pop[Region,Age]` while scoring
/// `(row_sum, growth)`). See [`derive_other_dep_verdict`] and [`OccurrenceAxis`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OtherDepVerdict {
    /// Collapse the subscript to a bare `Var(dep)` before the `PREVIOUS()` wrap
    /// (the indices correspond position-for-position to the dep's declared axes,
    /// or the dep's dims are un-threadable and the historical permissive
    /// collapse applies).
    Collapse,
    /// Not an iterated-dim subscript at all (a literal, wildcard, dynamic index,
    /// a name outside the target's iterated dims, or more indices than the
    /// target has iterated dims): normal subscript handling.
    NotIterated,
    /// Every index names a target-iterated dimension, but the dep's declared
    /// axes provably do NOT correspond positionally (transposed / arity-
    /// mismatched / mapped without a usable positional correspondence).
    /// Collapsing would freeze the WRONG element (GH #526).
    Mismatch,
}

/// The single source of truth for the other-dep iterated-subscript verdict,
/// derived from a subscript occurrence's per-axis [`OccurrenceAxis`]
/// classification plus the two arity facts the occurrence does not itself carry
/// -- the dep's declared arity `dep_arity` (`None` = un-threadable: the dep is
/// absent from the variable map / has no declared dims) and the target's
/// iterated-dim count `target_iterated_count`.
///
/// The ceteris-paribus wrap (`ltm_augment::other_dep_verdict`) reads a
/// non-live-dep subscript occurrence's `axes` straight off the occurrence IR
/// (via `OccurrenceLookup`) and calls this -- there is no longer an Expr0-side
/// re-derivation of the verdict on the live path, so the wrap and the edge
/// emitter cannot drift: they consume the SAME classification. (The Expr0
/// `axes` builder survives only `#[cfg(test)]`, to reconstruct occurrences for
/// the text-level wrap unit tests, and is proven in step with `classify_occurrence_axes`
/// by the alignment gate.)
///
/// See [`OccurrenceAxis`]'s rustdoc for the full rule; the two load-bearing
/// arity corners (under-arity all-`Iterated` => `Mismatch`; over-target-arity =>
/// `NotIterated`) are pinned by `under_arity_iterated_subscript_is_mismatch_not_collapse`
/// / `over_target_arity_iterated_subscript_is_not_iterated` in `db::ltm_ir_tests`.
pub(crate) fn derive_other_dep_verdict(
    axes: &[OccurrenceAxis],
    dep_arity: Option<usize>,
    target_iterated_count: usize,
) -> OtherDepVerdict {
    // Precondition (else normal subscript handling): a non-empty subscript, no
    // more indices than the target has iterated dims, and every index a bare
    // target-iterated-dim name -- i.e. every axis `Iterated` or
    // `MismatchedIterated` (a `Pinned`/`Reduced`/`Dynamic` axis is a literal,
    // wildcard, or dynamic index).
    if axes.is_empty() || axes.len() > target_iterated_count {
        return OtherDepVerdict::NotIterated;
    }
    let all_iterated_or_mismatched = axes.iter().all(|a| {
        matches!(
            a,
            OccurrenceAxis::Iterated { .. } | OccurrenceAxis::MismatchedIterated { .. }
        )
    });
    if !all_iterated_or_mismatched {
        return OtherDepVerdict::NotIterated;
    }
    // The dep's declared arity gates the collapse: un-threadable keeps the
    // transform's permissive collapse; a differing arity is a `Mismatch`
    // (checked BEFORE the per-axis lineup so an over-declared-arity subscript
    // whose trailing indices are `MismatchedIterated` from a missing source
    // axis is a `Mismatch`, not mislabelled by the per-axis arms).
    let Some(arity) = dep_arity else {
        return OtherDepVerdict::Collapse;
    };
    if axes.len() != arity {
        return OtherDepVerdict::Mismatch;
    }
    if axes
        .iter()
        .any(|a| matches!(a, OccurrenceAxis::MismatchedIterated { .. }))
    {
        return OtherDepVerdict::Mismatch;
    }
    OtherDepVerdict::Collapse
}

/// One classified reference occurrence over a target equation, the finer
/// substrate both LTM consumers project from (edge emission is a per-edge
/// dedup of these; the transform selects a live SET by shape and names the
/// first non-index-nested occurrence as the normalizer).
///
/// Every field has a reader. The ceteris-paribus wrap reads `site_id` (by path),
/// `reference`, `shape`, `axes`, and `index_nested`;
/// `db::module_link_score_equation` reads `reference` for the document-order
/// module-output pick; the corpus gate reads `in_reducer` to scope its
/// per-occurrence comparison. **Do not add a field without one.**
///
/// Four fields once rode along here -- `target_element`, `routing`,
/// `reducer_keys`, `already_lagged` -- carried on the theory that the GH #965
/// generation half would consume them. It consumed none, so they are gone.
/// `routing` was the instructive case: it duplicated the GH #793
/// enclosing-reducer narrowing the `ClassifiedSite` loop already performs, i.e. a
/// second implementation of one rule inside the IR built to end second
/// implementations. `target_element` and `already_lagged` duplicated facts their
/// consumers already hold (the generators are handed the target element; the wrap
/// sees a `PREVIOUS`/`INIT` node directly, and must in any case account for the
/// freezes it inserts ITSELF, which no field of an occurrence can describe).
#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub(crate) struct OccurrenceSite {
    /// Stable, deterministic occurrence identity within `to`'s equation.
    pub site_id: SiteId,
    /// The reference (a model variable, or a module-qualified output).
    pub reference: OccurrenceRef,
    /// The coarse per-reference access shape -- classified EXACTLY as the
    /// `ClassifiedSite` path does (the raw walker shape, before the per-edge
    /// `Wildcard`->`DynamicIndex` reclassification), so the two views agree.
    pub shape: RefShape,
    /// Per-index access (the extended `AxisRead` vocabulary). Empty for a bare
    /// `Var` / module output. Read by the wrap for the other-dep verdict, the
    /// literal-element index guard, and the `PerElement` row pinning.
    pub axes: Vec<OccurrenceAxis>,
    /// `true` iff the occurrence sits syntactically inside an aggregate-routed
    /// reducer (`SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`/`RANK`). Consumed by the corpus
    /// gate, which scopes its per-occurrence shape comparison to non-reducer
    /// references and asserts both streams agree on reducer context.
    ///
    /// Deliberately NOT consumed by the transform's GH #779
    /// bare-reducer-feeder decline, which asks a DIFFERENT question with its
    /// own walk: "does this subtree collapse to a scalar?"
    /// (`ltm_agg::reducer_collapses_to_scalar`) rather than "did an aggregate
    /// node get minted for it?" (`builtin_routes_through_agg`, which sets this
    /// bit). The two answers are inverted on exactly `SIZE` and `RANK`, and
    /// that inversion IS the difference between the questions -- `SIZE` is a
    /// scalar count that is never hoisted, `RANK` is array-valued but gets an
    /// array-valued agg. Consuming this bit there would flip a bare source
    /// inside `RANK(...)` from scored to loudly declined, a user-visible score
    /// change with no argument behind it. Assessed and resolved that way in
    /// GH #982; `ltm_agg::reducer_collapses_to_scalar`'s doc carries the full
    /// comparison and `ltm_agg::REDUCER_DECISION_TABLE` pins both predicates,
    /// row by row, so neither can move silently.
    pub in_reducer: bool,
    /// `true` iff the occurrence is reachable ONLY through another reference's
    /// subscript index (`other_arr[from]`). Such an occurrence is excluded
    /// from live selection, from the normalizer, and from the changed-last
    /// freeze, and turns a reducer whose only live occurrence is index-nested
    /// into a whole-reducer freeze (Q4). The edge emitter ignores this.
    pub index_nested: bool,
}

/// Walker output for one occurrence -- the occurrence analogue of
/// [`ReferenceSite`]. `model_ltm_reference_sites` moves it into an
/// [`OccurrenceSite`] field for field.
struct RawOccurrence {
    site_id: SiteId,
    reference: OccurrenceRef,
    shape: RefShape,
    axes: Vec<OccurrenceAxis>,
    in_reducer: bool,
    index_nested: bool,
}

/// The reference-site classification for a model: every `(from-var, to-var)`
/// causal edge with ≥1 AST reference, mapped to its classified sites.
///
/// Keys use the same string identity as the element/causal-edge maps
/// (canonical variable names). The `sites` HashMap's *key* iteration order
/// doesn't matter (consumers that need sorted edges sort keys themselves, as
/// today), but each value `Vec<ClassifiedSite>` is in a stable left-to-right
/// DFS order over the target's AST so salsa caches the result deterministically.
///
/// An edge that exists in the variable-level causal graph but has no AST
/// reference (a structural flow→stock edge, a module edge, or a synthesized
/// reference) simply has *no* entry here -- consumers fall back to a single
/// `Bare` site for it, exactly as the pre-IR walkers' `is_empty()` /
/// module pre-checks did.
///
/// `occurrences` is the finer, per-reference-occurrence enumeration over the
/// whole target equation (Track A2a), keyed by TARGET canonical name; each
/// value `Vec<OccurrenceSite>` is in stable left-to-right DFS order (slots in
/// sorted key order). It is a *superset* of `sites`' causal references -- it
/// also enumerates the module-qualified by-name live channel (`OccurrenceRef::ModuleOutput`)
/// that has no `ClassifiedSite`. It rides alongside `sites`; no current
/// consumer reads it (A2b is the first). Like `sites`, the HashMap *key*
/// order is irrelevant (consumers sort keys themselves); only each value
/// `Vec`'s order is load-bearing for salsa determinism.
///
/// `site_width_rejection` is the LTM front door's verdict for the whole model
/// (GH #978/#979). When it is `Some`, at least one target equation needs more
/// children at one path level than a `SiteId` component can tell apart, and that
/// equation contributed NO occurrences -- so `occurrences` is incomplete and
/// `model_ltm_variables` refuses the model outright rather than scoring it from
/// a stream that cannot name every reference. `sites` is unaffected either way:
/// the per-edge view is name-keyed, carries no path, and feeds the element
/// causal graph and `model_detected_loops`, which stay correct.
#[derive(Debug, Clone, Default, PartialEq, Eq, salsa::Update)]
pub(crate) struct LtmReferenceSitesResult {
    pub sites: HashMap<(String, String), Vec<ClassifiedSite>>,
    pub occurrences: HashMap<String, Vec<OccurrenceSite>>,
    pub site_width_rejection: Option<SiteWidthRejection>,
}

/// Classify every causal-edge reference site in `model` exactly once.
///
/// Salsa-tracked: a pure function of `(db, model, project)` consuming the
/// same reconstructed ASTs and the same `enumerate_agg_nodes` result the
/// other LTM analyses use, so all consumers see an identical map.
///
/// Determinism: variables are visited in canonical-sorted order and each
/// AST is walked left-to-right depth-first, exactly like `enumerate_agg_nodes`,
/// so the `sites` values are in a stable order. The synthetic agg an
/// `in_reducer` reference routes through is found via the same `by_var`
/// indexing `enumerate_agg_nodes` exposes (a synthetic agg of `to` whose
/// `sources` include `from`), then narrowed to aggs whose canonical reducer
/// text matches one of the site's enclosing reducer keys. That site-precise
/// key check prevents a hoisted sibling reducer from claiming a declined
/// sibling read on the same `(from, to)` edge (GH #793).
///
/// This is also the LTM front door (GH #978/#979): each target equation is
/// checked for `SiteId` addressability BEFORE its walk pushes a single path
/// component, and the first failure is reported on
/// [`LtmReferenceSitesResult::site_width_rejection`]. A rejected equation
/// contributes no occurrence at all, so no recorded `SiteId` can be a path two
/// children share, and `model_ltm_variables` refuses to score the model.
#[salsa::tracked(returns(ref))]
pub(crate) fn model_ltm_reference_sites(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> LtmReferenceSitesResult {
    // `enumerate_agg_nodes` is the sole hoisting decider; the IR only
    // consults its result to map a reducer reference to the synthetic agg(s)
    // minted for `to` that read `from` (or records `Direct` when there are
    // none -- SIZE, a reducer over only scalar sources, or a not-yet-hoisted
    // sliced reducer).
    let agg_nodes = crate::ltm_agg::enumerate_agg_nodes(db, model, project);
    let variables = reconstruct_model_variables(db, model, project);

    // Dimension context for the #511 iterated-subscript recognition: the
    // mapped-dimension case (`State[i]` over a source declared with
    // `Region[i]`, `State` maps to `Region`) needs `has_mapping_to`. Read the
    // project-global context from the salsa-cached query; it depends only on
    // the salsa-tracked dimensions input, so the IR is recomputed when a
    // dimension's mappings change.
    let dim_ctx = crate::db::project_dimensions_context(db, project);

    // Per-source dimension lookup, cached: a source's dims are needed to
    // resolve literal subscripts and are reused across many edges.
    let mut dim_cache: HashMap<String, Vec<crate::dimensions::Dimension>> = HashMap::new();
    let mut lookup_dims = |name: &str| -> Vec<crate::dimensions::Dimension> {
        if let Some(dims) = dim_cache.get(name) {
            return dims.clone();
        }
        let dims = variables
            .get(&Ident::<Canonical>::new(name))
            .and_then(|v| v.get_dimensions())
            .map(|d| d.to_vec())
            .unwrap_or_default();
        dim_cache.insert(name.to_string(), dims.clone());
        dims
    };

    // Visit `to` variables in canonical-sorted order for a deterministic
    // per-edge site order. (Within a `to`, `collect_all_reference_sites`
    // walks its AST left-to-right DFS, mirroring `enumerate_agg_nodes`.)
    let mut to_names: Vec<&Ident<Canonical>> = variables.keys().collect();
    to_names.sort();

    let mut sites: HashMap<(String, String), Vec<ClassifiedSite>> = HashMap::new();
    let mut occurrences: HashMap<String, Vec<OccurrenceSite>> = HashMap::new();
    // The LTM front door's model-level verdict: the FIRST target equation (in
    // the canonical-sorted visit order above) too wide for a `SiteId` to
    // address. `model_ltm_variables` reads it and refuses the model.
    let mut site_width_rejection: Option<SiteWidthRejection> = None;

    for to_name in to_names {
        let to_var = &variables[to_name];
        let to_name_str = to_name.as_str();

        // One walk feeds BOTH views: the per-source `ReferenceSite` buckets
        // (per-edge, existing consumers) and the flat, document-ordered
        // `RawOccurrence` stream (per-occurrence, the A2b transform).
        let (raw_by_source, raw_occurrences, width_rejection) =
            collect_all_reference_sites_and_occurrences(
                to_var,
                &variables,
                dim_ctx,
                &mut lookup_dims,
            );
        // Record the rejection BEFORE the reference-free skip below: an
        // over-wide equation need not reference any model variable at all
        // (`MEAN` of 65,536 constants), and losing its verdict there would let
        // the model be scored from an occurrence stream that silently dropped
        // the whole equation.
        if site_width_rejection.is_none()
            && let Some((axis, count)) = width_rejection
        {
            site_width_rejection = Some(SiteWidthRejection {
                variable: to_name_str.to_string(),
                axis,
                count,
                limit: site_children_limit(),
            });
        }
        // A target that references ONLY module-qualified outputs has no
        // `ReferenceSite` (module·port is not a model-variable key) but does
        // carry occurrences, so gate the skip on both being empty.
        if raw_by_source.is_empty() && raw_occurrences.is_empty() {
            continue;
        }

        // Indices into `agg_nodes.aggs` of the *synthetic* aggs occurring in
        // `to`'s equation. We narrow by source per edge below.
        let synthetic_aggs_in_to: Vec<usize> = agg_nodes
            .by_var
            .get(to_name_str)
            .map(|idxs| {
                idxs.iter()
                    .copied()
                    .filter(|&i| agg_nodes.aggs[i].is_synthetic)
                    .collect()
            })
            .unwrap_or_default();

        // Finalize the per-occurrence view. The RAW walker shape is preserved --
        // the not-hoisted in-reducer `Wildcard`->`DynamicIndex` reclassification
        // below is a per-edge-consumer artifact, and the occurrence keeps
        // `in_reducer` instead.
        let occ_sites: Vec<OccurrenceSite> = raw_occurrences
            .into_iter()
            .map(|occ| OccurrenceSite {
                site_id: occ.site_id,
                reference: occ.reference,
                shape: occ.shape,
                axes: occ.axes,
                in_reducer: occ.in_reducer,
                index_nested: occ.index_nested,
            })
            .collect();
        if !occ_sites.is_empty() {
            occurrences.insert(to_name_str.to_string(), occ_sites);
        }

        for (from_name, raw_sites) in raw_by_source {
            // Synthetic aggs of `to` that read `from`. The per-site routing
            // below further narrows this by canonical reducer text; a sibling
            // agg on the same edge must not absorb this site's read (GH #793).
            let routed_aggs: Vec<usize> = synthetic_aggs_in_to
                .iter()
                .copied()
                .filter(|&i| agg_nodes.aggs[i].reads_var(&from_name))
                .collect();

            // Whether `to` is a *variable-backed* aggregate node whose source
            // includes `from` -- i.e. `to`'s whole equation is exactly the
            // reducer (`total = SUM(population[*])`, `row_sum[D1] =
            // SUM(matrix[D1,*])`). In that case the `(from, to)` edge *is* the
            // agg edge and the reference keeps its coarse syntactic shape
            // (`Wildcard`, or `DynamicIndex` for the partial-StarRange
            // residual like `SUM(matrix[D1,*:Sub])`):
            // `model_element_causal_edges` routes any non-trivial
            // statically-describable slice by its read slice (GH #752 /
            // GH #765, via `ltm_agg::variable_backed_reduce_agg`) and
            // projects the whole-extent case as the reduction/broadcast via
            // `emit_edges_for_reference`.
            let to_is_variable_backed_agg = agg_nodes
                .by_var
                .get(to_name_str)
                .map(|idxs| {
                    idxs.iter().any(|&i| {
                        let a = &agg_nodes.aggs[i];
                        !a.is_synthetic && a.name == to_name_str && a.reads_var(&from_name)
                    })
                })
                .unwrap_or(false);

            let mut classified: Vec<ClassifiedSite> = Vec::new();
            for raw in raw_sites {
                let matching_aggs: Vec<usize> = if raw.in_reducer {
                    routed_aggs
                        .iter()
                        .copied()
                        .filter(|&agg_idx| {
                            raw.reducer_keys
                                .iter()
                                .any(|key| key == &agg_nodes.aggs[agg_idx].equation_text)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                if !matching_aggs.is_empty() {
                    // Route this reference only through aggs minted for one
                    // of its enclosing reducers. A nested reducer can match
                    // its own key when the outer reducer declined; when the
                    // outer reducer hoisted, its key is the one present.
                    for agg_idx in matching_aggs {
                        classified.push(ClassifiedSite {
                            shape: raw.shape.clone(),
                            target_element: raw.target_element.clone(),
                            routing: SiteRouting::ThroughAgg {
                                agg: AggRef(agg_idx),
                            },
                        });
                    }
                } else {
                    // A `Direct` `Wildcard` reference that is `in_reducer` but
                    // was *not* hoisted (no synthetic agg routes it, and `to`
                    // isn't itself a variable-backed agg) is the not-hoistable
                    // reducer carve-out -- a reducer over a dynamic index
                    // (`SUM(pop[idx,*])`) whose read slice isn't statically
                    // describable. Reclassify it as `DynamicIndex` so a
                    // `Direct` `Wildcard` site only ever means "a hoisted
                    // reducer's (ignored) syntactic shape", "a whole-RHS
                    // variable-backed reducer's argument", a NON-hoisting
                    // builtin's wildcard arg such as `SIZE(pop[*])`, which
                    // never sets `in_reducer` and deliberately keeps
                    // `Wildcard`, or a (rare) bare
                    // non-reducer whole-array reference (`arr[*]` outside
                    // any builtin), which likewise keeps `Wildcard` and the
                    // cross-product. The original #514 AC4.5 invariant
                    // -- the conservative cross-product is `DynamicIndex`-only
                    // from `Direct` sites -- therefore narrowed in T1 of the
                    // shape-expressiveness design: it still holds for every
                    // HOISTABLE reducer's argument, which is what keeps a
                    // hoist-eligible `Wildcard` from leaking past its agg.
                    let shape = if raw.in_reducer
                        && matches!(raw.shape, RefShape::Wildcard)
                        && !to_is_variable_backed_agg
                    {
                        RefShape::DynamicIndex
                    } else {
                        raw.shape
                    };
                    classified.push(ClassifiedSite {
                        shape,
                        target_element: raw.target_element,
                        routing: SiteRouting::Direct,
                    });
                }
            }
            sites.insert((from_name, to_name_str.to_string()), classified);
        }
    }

    LtmReferenceSitesResult {
        sites,
        occurrences,
        site_width_rejection,
    }
}

#[cfg(test)]
#[path = "ltm_ir_tests.rs"]
mod ltm_ir_tests;
