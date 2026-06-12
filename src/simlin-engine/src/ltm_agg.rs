// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Aggregate-node enumeration for LTM (Loops That Matter).
//!
//! An "aggregate node" is the conceptual stand-in for an inlined array-reducer
//! subexpression (`SUM(pop[*])`, `MEAN(...)`, ...). Phase 5 of the
//! cross-element-aggregate-scoring design treats each *maximal* reducer
//! subexpression in a model's equations as an implicit synthetic auxiliary
//! named `$⁚ltm⁚agg⁚{n}`, so that causality routes `source[d] → agg → target`
//! instead of all-pairs `source[d] → target[e]`.
//!
//! Two consumers share this enumeration:
//! - `model_element_causal_edges` reroutes a Wildcard/DynamicIndex reducer
//!   reference through the agg node.
//! - `model_ltm_variables` emits the `$⁚ltm⁚agg⁚{n}` auxiliaries plus the two
//!   link-score families.
//!
//! Because both consumers must see *identical* agg names, the enumeration is
//! salsa-tracked and fully deterministic: variables are visited in canonical
//! sorted order, each variable's AST is walked left-to-right depth-first, and
//! synthetic names are assigned `$⁚ltm⁚agg⁚0`, `1`, ... in first-encounter
//! order. AST-identical *synthetic* reducer subexpressions dedupe to a single
//! agg node (canonicalization is via printed equation text, since `Expr2` is
//! not `Eq`). Variable-backed aggs are never deduped (see below).
//!
//! Two kinds of aggregate node:
//! - **Synthetic** (`is_synthetic == true`): the reducer is a *sub-expression*
//!   of a larger equation (`share[r] = pop[r] / SUM(pop[*])`). A
//!   `$⁚ltm⁚agg⁚{n}` auxiliary is minted to hold its value. Two inline uses
//!   of the same reducer text share one synthetic node (dedup by canonical
//!   text via `synthetic_by_key`).
//! - **Variable-backed** (`is_synthetic == false`): the reducer is the
//!   *entire* dt-equation of a scalar or apply-to-all variable
//!   (`total_population = SUM(population[*])`, `row_sum[D1] = SUM(matrix[D1,*])`).
//!   That variable *is* the aggregate node; no synthetic is minted. One
//!   exception: a whole-RHS reducer whose shape the variable-backed
//!   machinery cannot express -- a MAPPED iterated axis (GH #534) or
//!   NON-ALIGNED result dims (broadcast/permuted, GH #764) -- mints a
//!   synthetic agg instead; see
//!   [`variable_backed_shape_is_expressible`]. Each such
//!   variable is its own distinct agg node -- variable-backed aggs are never
//!   deduped and never reused by an inline use of the same reducer text (an
//!   inline use must get its own *synthetic* node, since the element-graph
//!   reroute and the link-score emitter both filter to `is_synthetic` aggs;
//!   reusing the variable-backed node would silently leave the inline reducer
//!   on the conservative direct-scoring path, with the outcome depending on
//!   whether the whole-RHS reducer happened to be declared first).
//!
//! Each agg carries a [`AggNode::sources`] -- one [`AggSource`] per source
//! variable, each with its own read slice (one [`AxisRead`] per that
//! source's axes) -- recording *which rows* the reducer reads, so the
//! element-graph reroute and the per-element reducer link scores route only
//! those rows.
//! Whole-extent reducers (`SUM(pop[*])`, `SUM(matrix[*,*])`) have an all-
//! `Reduced` slice; sliced reducers (`SUM(pop[NYC,*])` ⇒ `[Pinned(nyc),
//! Reduced]`, `SUM(matrix[D1,*])` over an A2A-`D1` body ⇒ `[Iterated(d1),
//! Reduced]` and an arrayed agg over `D1`) are hoisted too -- including a
//! positionally-MAPPED iterated axis (`SUM(matrix[State,*])` over a
//! `matrix[Region,..]` source with a `State→Region` mapping, GH #534), where
//! the `Iterated` axis carries the (target, source) dim pair and the agg is
//! arrayed over the TARGET dim, and a StarRange over a PROPER subdimension
//! (`SUM(arr[*:Sub])`, GH #766), where the `Reduced` axis carries the
//! subdimension's element subset. A MULTI-SOURCE reducer is accepted per
//! invariant I1 of the shape-expressiveness design ([`accept_source_slices`],
//! GH #767 / T5): all CO-SOURCES (`Reduced`-bearing slices) must carry the
//! identical canonical slice, and an ITERATED-DIM PROJECTION FEEDER --
//! a source whose slice is all-`Iterated` over exactly the canonical
//! slice's iterated target dims, in order, unmapped (`frac[D1]` in
//! `SUM(matrix[D1,*] * frac[D1])`) -- is accepted with ITS OWN slice (it
//! is per-result-slot constant, the arrayed generalization of the GH #737
//! scalar feeder). The carve-outs: a reducer over a *dynamic index*
//! (`SUM(pop[idx,*])`, `idx` non-literal) is not statically describable, a
//! mapped iterated axis whose mapping is element-mapped (GH #756) or
//! non-positional is declined (a positional mapping is accepted in EITHER
//! declaration direction since GH #757), a StarRange
//! naming a NON-subdimension (a mid-edit inconsistency that must not
//! silently widen to the full extent) is declined -- `compute_read_slice`
//! returns `None`, the reducer is not hoisted, and its reference stays on
//! the conservative path -- and so are co-sources with differing slices,
//! one variable read with two different slices (I3b), and a no-`Reduced`
//! source outside the projection rule (a Pinned-bearing, dim-subset,
//! permuted, or mapped mix; see [`accept_source_slices`]). `RANK` is
//! recognized as a reducer but never hoisted (GH #771): it is ARRAY-valued,
//! so an agg node -- "a scalar value per result slot" -- has no value to
//! hold for it; see [`reducer_is_hoistable`].
//!
//! Whole-RHS reduces with a non-trivial slice *are* recognized -- the
//! variable is the agg, `result_dims` carries the `Iterated` axes' dims, and
//! its source's read slice records the per-axis split. The element graph
//! routes them by the read slice too (GH #752, generalized by GH #765 / T3
//! of the shape-expressiveness design, [`variable_backed_reduce_agg`]): for
//! an aligned partial reduce (`row_sum[D1] = SUM(matrix[D1,*])`,
//! Pinned/subset axes included) each source READ row feeds only its own
//! `row_sum[<slot>]` element node, and for a scalar-result slice on a
//! SCALAR owner (`total = SUM(pop[nyc,*])`, `total = SUM(arr[*:Sub])`) the
//! read rows feed the bare `total` node -- in both cases matching the
//! per-read-row link scores `try_cross_dimensional_link_scores` derives
//! from the SAME `read_slice_rows` (invariant I4), never the phantom
//! cross-product or an inflated full-extent divisor. Whole-extent
//! variable-backed reducers (`total = SUM(pop[*])`, including the broadcast
//! `share[R] = SUM(pop[*])`) keep the normal reference walker's
//! reduction/broadcast edges, which are already the true reads for those
//! shapes. A whole-RHS partial reduce whose result dims are NON-ALIGNED
//! with the owner's dims -- broadcast over extra target dims or permuted
//! axes (GH #764 / T4 of the shape-expressiveness design) -- mints a
//! SYNTHETIC agg instead, like the mapped GH #534 case (see
//! [`variable_backed_shape_is_expressible`], the one minting condition).
//! The gate's remaining decline -- the ARRAYED-owner scalar-result
//! Pinned/subset slice (`share[R] = SUM(pop[nyc,*])`, no `Iterated` axis,
//! GH #777) -- keeps the conservative cross-product, a SUPERSET of the
//! true reads, with its scores loudly skipped (the GH #758 treatment)
//! rather than silently wrong (see [`variable_backed_reduce_agg`]).

use std::collections::HashMap;

use crate::ast::{Ast, Expr2, IndexExpr2};
use crate::builtins::BuiltinFn;
use crate::common::{Canonical, Ident, canonicalize};
use crate::db::{
    Db, LtmLinkId, SourceModel, SourceProject, project_datamodel_dims, project_dimensions_context,
    reconstruct_model_variables,
};

/// Prefix for synthetic aggregate-node names: `$⁚ltm⁚agg⁚{n}`.
///
/// The `⁚` is U+205A (TWO DOT PUNCTUATION), matching the separator used for
/// every other LTM synthetic-variable family (`$⁚ltm⁚link_score⁚...`, etc.).
pub(crate) const AGG_NAME_PREFIX: &str = "$\u{205A}ltm\u{205A}agg\u{205A}";

/// Build the canonical name for the `n`th synthetic aggregate node.
pub(crate) fn synthetic_agg_name(n: usize) -> String {
    format!("{AGG_NAME_PREFIX}{n}")
}

// --- Array-reducer recognition ---------------------------------------------
//
// This is the single place the LTM machinery decides "is this builtin an array
// reducer, and if so what algebraic shape does it have?". Every consumer --
// the agg enumerator's hoisting test, the element-graph walker's
// `in_reducer` marker, the cross-dimensional link-score generator, and the
// `Expr0`-walking partial-equation builder -- reads `reducer_kind` (or one of
// the thin predicates below) rather than restating the set.

/// Algebraic classification of an array-reducing builtin, used to pick a
/// link-score generation strategy when an arrayed variable feeds a scalar (or
/// lower-rank) target through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReducerKind {
    /// `SUM`, single-argument `MEAN`: the partial derivative is algebraically
    /// simple.
    /// `SUM`: partial = PREVIOUS(target) + (source[d] - PREVIOUS(source[d]))
    /// `MEAN`: same as `SUM` but divided by the number of elements.
    Linear,
    /// Single-argument `MIN`/`MAX`, `STDDEV`, `RANK`: must enumerate all
    /// elements explicitly, wrapping every element except the current one in
    /// `PREVIOUS`.
    Nonlinear,
    /// `SIZE`: output is constant (depends only on dimension cardinality).
    /// Link score is always 0; skip generation entirely. `SIZE` is
    /// *recognized* as a reducer but never hoisted (see [`reducer_is_hoistable`]).
    Constant,
}

/// The canonical lowercase-name + arity decider for the array-reducer set.
///
/// `SUM`/`STDDEV`/`SIZE`/`RANK` reduce an array dimension at any arity
/// (`RANK(arr, dir)` is still a reducer); `MEAN`/`MIN`/`MAX` reduce a
/// dimension only in their single-argument form (their multi-argument forms
/// are scalar element-wise operations -- a 2-arg `MIN(a, b)` is `min(a, b)`,
/// a multi-arg `MEAN(a, b, c)` is `(a + b + c) / 3`).
pub(crate) fn reducer_kind_from_name(name: &str, arity: usize) -> Option<ReducerKind> {
    match name {
        "sum" => Some(ReducerKind::Linear),
        "mean" if arity == 1 => Some(ReducerKind::Linear),
        "min" | "max" if arity == 1 => Some(ReducerKind::Nonlinear),
        "stddev" => Some(ReducerKind::Nonlinear),
        "rank" => Some(ReducerKind::Nonlinear),
        "size" => Some(ReducerKind::Constant),
        _ => None,
    }
}

/// `true` when `name`/`arity` (lowercase, like [`reducer_kind_from_name`])
/// names an array builtin whose RESULT is a scalar -- the genuinely reducing
/// set (`SUM`, 1-arg `MEAN`/`MIN`/`MAX`, `STDDEV`, `SIZE`).
///
/// Callers lowercase the name before calling; that normalization is
/// defensive belt-and-suspenders, not load-bearing -- parsed `Expr0`
/// function names are already lowercase by construction (the parser
/// lowercases function-call identifiers, and LTM-generated uppercase
/// reducer text is re-parsed before any of these predicates see it).
///
/// `RANK` is recognized as a reducer by [`reducer_kind_from_name`], but
/// `RANK(arr, dir)` is ARRAY-valued -- the rank of each element, Vensim's
/// VECTOR RANK -- so any consumer deciding "does this subtree collapse to a
/// scalar" must exclude it (GH #742): treating a frozen
/// `PREVIOUS(RANK(arr, dir))` subtree as scalar routes the capture into a
/// per-element *scalar* helper whose equation is ill-typed (array-valued in
/// scalar context), the helper fragment fails, and the consuming score
/// silently corrupts. The three consumers are
/// `builtins_visitor::arg_has_bare_var_ref` (the GH #541 arrayed-capture
/// gate), `ltm_augment::expr_is_array_slice_valued` (the GH #743
/// unfreezable-`PREVIOUS` detector), and [`reducer_is_hoistable`] (the agg
/// hoisting gate, GH #771): an agg node *is* "a scalar value per result
/// slot", and RANK has no such value to name.
///
/// Residual of the GH #771 de-hoist: a `RANK(pop, dir)` reference
/// classifies by its syntactic shape (a bare `pop` is `Bare` -> diagonal
/// edges), so loops through the rank ORDERING -- element r's rank changing
/// because element s moved past it -- are not enumerated. This replaces the
/// strictly-worse pre-#771 state (cross-element loops enumerated through an
/// ill-shaped scalar agg whose every score was warned-zero); the
/// alternative -- reclassifying value-shaped reducer args as `DynamicIndex`
/// -- would recreate the GH #525 phantom pathology (cross-product edges
/// reading diagonal scores) and was rejected.
pub(crate) fn reducer_collapses_to_scalar(name: &str, arity: usize) -> bool {
    reducer_kind_from_name(name, arity).is_some() && name != "rank"
}

/// [`reducer_kind_from_name`] applied to a `BuiltinFn`.
///
/// Generic over the contained expression type because it only inspects the
/// builtin's identity and arity, never the arguments themselves -- so
/// `BuiltinFn<Expr2>` (the element-graph walker, `classify_reducer`) and any
/// future `BuiltinFn<Expr0>` caller share one implementation.
pub(crate) fn reducer_kind<E>(builtin: &BuiltinFn<E>) -> Option<ReducerKind> {
    reducer_kind_from_name(builtin.name(), builtin_reducer_arity(builtin))
}

/// The arity [`reducer_kind_from_name`] / [`reducer_collapses_to_scalar`]
/// key on. Only `MEAN`/`MIN`/`MAX` are arity-sensitive; for everything else
/// the deciders ignore the arity argument.
fn builtin_reducer_arity<E>(builtin: &BuiltinFn<E>) -> usize {
    match builtin {
        BuiltinFn::Mean(args) => args.len(),
        BuiltinFn::Min(_, opt) | BuiltinFn::Max(_, opt) => 1 + opt.is_some() as usize,
        _ => 1,
    }
}

/// `true` when `builtin` is a recognized array reducer that is *hoisted* into
/// an aggregate node -- i.e. recognized AND not [`ReducerKind::Constant`] AND
/// scalar-valued ([`reducer_collapses_to_scalar`], invariant I5 of the
/// shape-expressiveness design).
///
/// `SIZE` is recognized as a reducer but never hoisted (its link score is
/// always 0), and it never sets the element-graph walker's `in_reducer`
/// marker. `RANK` is recognized but never hoisted either (GH #771): it is
/// ARRAY-valued, so a scalar agg node for it has an ill-typed equation that
/// cannot compile -- an agg node exists to give a scalar reduction a name,
/// and RANK has no scalar reduction to name. Its references stay `Direct`
/// (a bare arg classifies `Bare` -> diagonal edges, scored by the GH #742
/// arrayed-capture machinery). This is the predicate the reference-site
/// IR's AST walk (`db::ltm_ir::walk_all_in_expr`) uses to flip
/// `child_in_reducer`, and that [`reducer_source_vars`] uses to gate which
/// subexpressions become aggregate nodes, so the agg enumerator, the
/// element graph, and the link-score generator all agree on the hoisted set.
pub(crate) fn reducer_is_hoistable<E>(builtin: &BuiltinFn<E>) -> bool {
    let arity = builtin_reducer_arity(builtin);
    matches!(
        reducer_kind_from_name(builtin.name(), arity),
        Some(ReducerKind::Linear | ReducerKind::Nonlinear)
    ) && reducer_collapses_to_scalar(builtin.name(), arity)
}

/// How one *source axis* of a hoisted reducer is consumed.
///
/// A reducer reference into an arrayed source (`SUM(pop[NYC, *])`,
/// `SUM(matrix[D1, *])`, `SUM(pop[*])`) reads each axis of the source in one
/// of three ways. [`AggNode::read_slice`] carries one entry per source axis,
/// in the source's declared dimension order, which is the structural truth
/// the element graph and link-score emitters need (the canonical equation
/// text alone is ambiguous about *which rows* a slice reads):
/// - [`AxisRead::Pinned`] -- a single literal element of that axis is read
///   (`pop[NYC, *]`'s first axis). Carries the canonical element name.
/// - [`AxisRead::Iterated`] -- the axis is iterated over the enclosing
///   variable's apply-to-all dimension space and the agg result varies per
///   element of it (`matrix[D1, *]`'s first axis inside an A2A-over-`D1`
///   body). Carries the (target, source) canonical dimension pair -- equal
///   for the literal case, different for a positionally-MAPPED sliced
///   reducer (GH #534) -- and the target dim appears in
///   [`AggNode::result_dims`] (datamodel-cased).
/// - [`AxisRead::Reduced`] -- the axis is reduced away (`SUM(pop[*])`, the
///   `*` in `SUM(pop[NYC, *])`, `SUM(arr[*:Sub])`). With `subset: None`
///   every element of that axis feeds the agg result slot; with
///   `subset: Some(elems)` (a StarRange over a PROPER subdimension,
///   GH #766) only the subdimension's elements do.
///
/// `PartialOrd`/`Ord`/`Hash` ride along because `RefShape::PerElement`
/// (GH #525, T6 of the shape-expressiveness design) embeds an
/// `AxisRead` vector and `RefShape` lives in `BTreeSet`s /
/// `HashSet`-keyed dedup maps downstream.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, salsa::Update)]
pub enum AxisRead {
    /// A single literal element of this source axis is read (`pop[NYC, *]`).
    /// Carries the canonical element name.
    Pinned(String),
    /// This source axis is iterated over the enclosing variable's
    /// apply-to-all dimension space (`matrix[D1, *]` inside an
    /// A2A-over-`D1` body, or `matrix[State, *]` over a `matrix[Region, ..]`
    /// source with a positional `State→Region` mapping -- GH #534).
    Iterated {
        /// Canonical name of the TARGET equation's iterated dimension --
        /// the agg's result axis for this slot coordinate.
        dim: String,
        /// Canonical name of the SOURCE's declared dimension on this axis.
        /// Equals `dim` in the literal case; differs for a positionally
        /// mapped sliced reducer, where each source row feeds the slot of
        /// its positionally-corresponding target element (see
        /// [`iterated_axis_slot_elements`]).
        source_dim: String,
    },
    /// The axis is reduced away by the reducer. `subset: None` = the full
    /// extent (`SUM(pop[*])`, `SUM(arr[*:D])` where `D` is the axis's own
    /// dimension): every element of the axis feeds the agg result slot.
    /// `subset: Some(elems)` = a StarRange over a PROPER subdimension of
    /// the axis's dimension (`SUM(arr[*:Sub])`, GH #766): only the
    /// subdimension's elements (canonical names, in subdimension-declared
    /// order, resolved at enumeration time via
    /// [`crate::dimensions::SubdimensionRelation`]) feed the slot.
    /// Invariant I3: a `Some` subset is non-empty and a proper subset of
    /// the axis's elements -- a subdimension covering the whole axis
    /// normalizes to `None` so the full-extent representation is unique.
    Reduced { subset: Option<Vec<String>> },
}

/// The agg result-slot coordinate (an element of the `Iterated` axis's
/// TARGET dimension `target_dim`) for each source element of
/// `source_axis_elems`, index-aligned.
///
/// - Literal case (`target_dim == source_dim`): the identity -- slot
///   coordinate == source element.
/// - Mapped case (GH #534): the PREIMAGE inversion of
///   [`crate::dimensions::DimensionsContext::mapped_element_correspondence`]
///   `(target_dim, source_dim)` -- that helper is indexed by TARGET element
///   position and yields the source element read for it, so the slot for a
///   given source element is the target element whose correspondence entry
///   names it. The helper's positional-only gate (an explicit element map
///   returns `None` -- GH #756) makes the correspondence a bijection
///   (index-identity, equal cardinality), so every source element has
///   exactly one preimage; the inversion is still written generally and
///   declines (returns `None`) if a source element has zero or multiple
///   preimages, mirroring `expand_same_element`'s general-shape inversion.
///
/// `None` means "no usable slot remap": `compute_read_slice` then declines
/// to hoist (classification), and the emitters fall back to their
/// conservative forms (expansion) -- the same function gates both, so the
/// two can never disagree about which mapped axes are remappable.
pub(crate) fn iterated_axis_slot_elements(
    target_dim: &str,
    source_dim: &str,
    source_axis_elems: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<Vec<String>> {
    use crate::common::CanonicalDimensionName;
    if target_dim == source_dim {
        return Some(source_axis_elems.to_vec());
    }
    let t = CanonicalDimensionName::from_raw(target_dim);
    let s = CanonicalDimensionName::from_raw(source_dim);
    let corr = dim_ctx.mapped_element_correspondence(&t, &s)?;
    let target_named = match dim_ctx.get(&t)? {
        crate::dimensions::Dimension::Named(_, named) => named,
        crate::dimensions::Dimension::Indexed(_, _) => return None,
    };
    // `corr` is indexed by target element position (declared order), so it
    // is parallel to `target_named.elements` by construction.
    debug_assert_eq!(corr.len(), target_named.elements.len());
    source_axis_elems
        .iter()
        .map(|e| {
            let mut found: Option<usize> = None;
            for (p, src_elem) in corr.iter().enumerate() {
                if src_elem.as_str() == e {
                    if found.is_some() {
                        // Non-bijective (a many-to-one correspondence):
                        // a single source row would feed several slots,
                        // which the one-slot-per-row machinery can't
                        // express. Decline.
                        return None;
                    }
                    found = Some(p);
                }
            }
            found.map(|p| target_named.elements[p].as_str().to_string())
        })
        .collect()
}

/// One source variable of an aggregate node, carrying its OWN read slice
/// (the per-source representation of the shape-expressiveness design, T2 --
/// GH #767's data-model half).
///
/// `read_slice` has one [`AxisRead`] per THIS source's declared axes
/// (invariant I2), so a SCALAR source -- a feeder like `scale` in
/// `SUM(pop[*] * scale)`, GH #737 -- carries an empty slice. Under the I1
/// acceptance ([`accept_source_slices`], T5 of the design / GH #767) every
/// arrayed CO-SOURCE carries the identical *canonical* slice, while an
/// ITERATED-DIM PROJECTION FEEDER (`frac` in
/// `SUM(matrix[D1,*] * frac[D1])`) carries its own all-`Iterated`
/// projection slice -- see [`AggNode::source_is_projection_feeder`].
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct AggSource {
    /// Canonical model-variable name.
    pub var: String,
    /// One entry per this source's axes (in the source's declared dimension
    /// order): which rows of it the reducer actually reads. Empty for a
    /// scalar source. Drives the element-graph reroute
    /// (`source[<pinned>,<iterated>,<reduced→rep>] → agg[<iterated>]`) and
    /// the per-element reducer link scores (only the read rows get a link
    /// score). All-`Reduced` means a whole-extent reduce; see [`AxisRead`].
    pub read_slice: Vec<AxisRead>,
}

/// One aggregate node: the stand-in for a maximal reducer subexpression.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct AggNode {
    /// The aggregate node's name. For a synthetic agg this is
    /// `$⁚ltm⁚agg⁚{n}`; for a variable-backed agg this is the owning
    /// variable's canonical name (`total_population`, `row_sum`, ...).
    pub name: String,
    /// The reducer subexpression rendered as equation text, e.g.
    /// `"sum(pop[*])"`. This is the canonical (`Loc`-insensitive) key the
    /// node was deduped on; `expr2_to_string` lowercases idents and
    /// normalizes whitespace, so textually-distinct-but-AST-identical
    /// subexpressions collapse to one node.
    pub equation_text: String,
    /// The aggregate's result-axis dimension names, in datamodel casing
    /// (e.g. `["D1"]` for `row_sum[D1] = SUM(matrix[D1,*])` or for a
    /// synthetic agg minted from `x[D1] = ... + SUM(matrix[D1,*])`). Empty
    /// for a scalar reducer (`SUM(pop[*])`, `SUM(pop[NYC,*])`). These are
    /// the canonical slice's [`AxisRead::Iterated`] axes' dims, in order.
    pub result_dims: Vec<String>,
    /// The model variables the reducer reads, each with its own read slice
    /// (see [`AggSource`]). SORTED by canonical variable name and
    /// deduplicated (invariant I3b: one entry per variable -- `sources` is
    /// keyed by name downstream, so the enumerator declines a hoist whose
    /// references would give one variable two different slices), making
    /// salsa cache equality and emission order deterministic regardless of
    /// AST occurrence order. For `SUM(a[*] + b[*])` this is
    /// `[a, b]`, each carrying the shared `[Reduced]` slice.
    pub sources: Vec<AggSource>,
    /// `true` when a `$⁚ltm⁚agg⁚{n}` auxiliary must be minted to hold this
    /// value; `false` when the owning variable already *is* the aggregate
    /// node (its entire dt-equation is exactly this reducer).
    pub is_synthetic: bool,
}

impl AggNode {
    /// `true` when `var` (canonical) is one of this agg's source variables
    /// (arrayed co-source or scalar feeder alike) -- the name-keyed
    /// membership test the reference-site IR's routing filter and the
    /// GH #752 gate share.
    pub fn reads_var(&self, var: &str) -> bool {
        self.sources.iter().any(|s| s.var == var)
    }

    /// The read slice of source `var`, or the EMPTY slice when `var` is not
    /// a source of this agg. A scalar source's slice is empty too -- and since
    /// GH #783 both row-enumeration consumers (`emit_agg_routed_edges`' element
    /// edges and the link scores) read it through the ONE
    /// `read_slice_row_parts` derivation, so the empty slice means the same
    /// thing on both surfaces: no per-row machinery applies, degrade to the
    /// caller's conservative fallback / scalar arm.
    pub fn source_read_slice(&self, var: &str) -> &[AxisRead] {
        self.sources
            .iter()
            .find(|s| s.var == var)
            .map(|s| s.read_slice.as_slice())
            .unwrap_or(&[])
    }

    /// The *canonical* slice (invariant I1 of the shape-expressiveness
    /// design): the shared slice of the agg's CO-SOURCES -- the first
    /// source slice carrying a [`AxisRead::Reduced`] axis (all co-sources
    /// carry identical slices by the I1 acceptance, so "first" is
    /// order-independent). Consumers whose decision is about the
    /// *reducer's* shape rather than one source's rows (the
    /// [`variable_backed_reduce_agg`] gate) key on it.
    ///
    /// The "first slice with a Reduced axis" definition is the T5 contract
    /// fix: under projection-feeder acceptance (GH #767) an arrayed source
    /// may carry an all-`Iterated` feeder slice, and "first non-empty"
    /// would let an alphabetically-first feeder (e.g. `frac` in
    /// `SUM(matrix[D1,*] * frac[D1])`) satisfy the gate's axis checks for
    /// the wrong shape. The fallback to the first non-empty slice covers
    /// the degenerate no-co-source agg (every arrayed source all-`Iterated`,
    /// e.g. a scalar-valued `SUM(frac[D1])` arg) -- accepted under the
    /// identical-slices rule exactly as before T5, so the gate keeps
    /// reading the shared slice for it. Empty for a -- by construction
    /// impossible -- agg with no arrayed source (scalar feeders carry
    /// empty slices).
    pub fn canonical_read_slice(&self) -> &[AxisRead] {
        let slices = || self.sources.iter().map(|s| s.read_slice.as_slice());
        slices()
            .find(|rs| rs.iter().any(|ax| matches!(ax, AxisRead::Reduced { .. })))
            .or_else(|| slices().find(|rs| !rs.is_empty()))
            .unwrap_or(&[])
    }

    /// `true` when `var` is an accepted ITERATED-DIM PROJECTION FEEDER of
    /// this agg (the I1 feeder clause, GH #767 / T5 of the
    /// shape-expressiveness design): its own slice is non-empty and
    /// all-`Iterated` (a projection of the canonical slice onto the shared
    /// iterated axes -- per-result-slot constant), while the canonical
    /// slice carries a `Reduced` axis (a genuine reduction exists for the
    /// feeder to feed; the canonical slice may also carry `Pinned` axes --
    /// the Iterated-only requirement is on the feeder's slice, not the
    /// canonical one). The acceptance in `combined_read_slice` guarantees
    /// an accepted feeder's Iterated target dims equal the canonical
    /// slice's, in order, and are unmapped -- so a feeder's
    /// `read_slice_rows` rows are 1:1 with the agg's result slots.
    ///
    /// The canonical-`Reduced` requirement keeps the degenerate
    /// no-co-source agg (`SUM(frac[D1])`, all sources all-`Iterated`) OFF
    /// the feeder emitters: it rides the pre-T5 paths byte-identically.
    pub fn source_is_projection_feeder(&self, var: &str) -> bool {
        let slice = self.source_read_slice(var);
        !slice.is_empty()
            && slice
                .iter()
                .all(|ax| matches!(ax, AxisRead::Iterated { .. }))
            && self
                .canonical_read_slice()
                .iter()
                .any(|ax| matches!(ax, AxisRead::Reduced { .. }))
    }
}

/// The result of enumerating every aggregate node in a model.
///
/// Deterministic by construction so salsa caches it stably: `aggs` is in
/// first-encounter order over the canonical-sorted variable list,
/// `synthetic_by_key` maps the canonical reducer text to the index of the
/// *synthetic* agg minted for it, and `by_var` maps each variable's
/// canonical name to the indices of the aggs that appear in its equation
/// (so the element-graph reroute can ask "which agg of `to` reads `from`?").
///
/// Dedup-by-key applies to *synthetic* aggs only. Two inline uses of the
/// same reducer text collapse to one `$⁚ltm⁚agg⁚{n}` node. A *variable-
/// backed* agg (the whole dt-equation of a scalar/A2A variable is exactly
/// one reducer) is never deduped -- each such variable genuinely is its own
/// aggregate node, so two whole-RHS reducers with identical text yield two
/// distinct variable-backed aggs, and an inline use of a reducer never
/// reuses a variable-backed agg of the same text (which would otherwise be
/// filtered out by the `is_synthetic` checks downstream, leaving the inline
/// reducer on the conservative direct-scoring path -- a name-ordering bug).
#[derive(Clone, Debug, PartialEq, Eq, Default, salsa::Update)]
pub struct AggNodesResult {
    /// Aggregate nodes in first-encounter (deterministic) order.
    pub aggs: Vec<AggNode>,
    /// Canonical reducer text -> index into `aggs` of the *synthetic* agg
    /// minted for that text. Variable-backed aggs do not participate.
    pub synthetic_by_key: HashMap<String, usize>,
    /// Variable canonical name -> indices into `aggs` of the aggregate
    /// subexpressions occurring in that variable's dt-equation (both
    /// synthetic and variable-backed). A synthetic agg that appears in two
    /// variables' equations (AST-identical → deduped) is referenced from
    /// both variables' entries.
    pub by_var: HashMap<String, Vec<usize>>,
}

impl AggNodesResult {
    /// Look up the *synthetic* aggregate node minted for a canonical
    /// reducer text. Returns `None` for a text that only ever appears as a
    /// variable's whole dt-equation (variable-backed aggs are not keyed
    /// here -- look them up via [`Self::aggs_in_var`] on the owning
    /// variable instead).
    pub fn agg_for_key(&self, key: &str) -> Option<&AggNode> {
        self.synthetic_by_key.get(key).map(|&i| &self.aggs[i])
    }

    /// Iterate the aggregate nodes occurring in `var_name`'s dt-equation.
    pub fn aggs_in_var<'a>(&'a self, var_name: &str) -> impl Iterator<Item = &'a AggNode> {
        self.by_var
            .get(var_name)
            .into_iter()
            .flat_map(move |idxs| idxs.iter().map(move |&i| &self.aggs[i]))
    }
}

/// Enumerate every aggregate node (maximal reducer subexpression) in `model`.
///
/// Salsa-tracked: a pure function of `(db, model, project)` consuming the same
/// reconstructed ASTs the element-graph walker uses, so both consumers see an
/// identical map.
#[salsa::tracked(returns(ref))]
pub fn enumerate_agg_nodes(
    db: &dyn Db,
    model: SourceModel,
    project: SourceProject,
) -> AggNodesResult {
    let variables = reconstruct_model_variables(db, model, project);
    let dm_dims = project_datamodel_dims(db, project);
    // Dimension context for the GH #534 mapped-iterated-axis recognition
    // (`compute_read_slice`'s `has_mapping_to` direction gate +
    // `iterated_axis_slot_elements`' positional correspondence). Salsa-cached
    // off the project's dimensions input, so the enumeration is recomputed
    // when a dimension's mappings change.
    let dim_ctx = crate::db::project_dimensions_context(db, project);

    // Visit variables in canonical-sorted order for deterministic synthetic
    // naming. `reconstruct_model_variables` returns a HashMap, so the order
    // is not otherwise stable.
    let mut var_names: Vec<&Ident<Canonical>> = variables.keys().collect();
    var_names.sort();

    let mut result = AggNodesResult::default();
    let mut next_synthetic_n: usize = 0usize;

    for var_name in var_names {
        let var = &variables[var_name];
        let Some(ast) = var.ast() else {
            // Stocks (init-only AST) and modules have no dt-equation to walk.
            continue;
        };
        let var_name_str = var_name.as_str().to_string();
        let dm_dims_ref = dm_dims.as_slice();

        match ast {
            Ast::Scalar(expr) => {
                // Scalar target: no iterated dimensions, so any sliced reducer
                // can only `Pinned`/`Reduced` its source axes.
                let ctx = AggWalkCtx {
                    variables: &variables,
                    target_iterated_dims: &[],
                    dm_dims: dm_dims_ref,
                    dim_ctx,
                };
                walk_var_equation(
                    expr,
                    &var_name_str,
                    &ctx,
                    &mut result,
                    &mut next_synthetic_n,
                );
            }
            Ast::ApplyToAll(dims, expr) => {
                // The A2A dimensions are this target's iterated dimensions
                // (canonical names, in declared order) -- a `SUM(matrix[D1,*])`
                // slice keyed by one of them is an arrayed agg over it.
                let target_iterated_dims: Vec<String> =
                    dims.iter().map(|d| d.name().to_string()).collect();
                let ctx = AggWalkCtx {
                    variables: &variables,
                    target_iterated_dims: &target_iterated_dims,
                    dm_dims: dm_dims_ref,
                    dim_ctx,
                };
                walk_var_equation(
                    expr,
                    &var_name_str,
                    &ctx,
                    &mut result,
                    &mut next_synthetic_n,
                );
            }
            Ast::Arrayed(_, per_elem, default_expr, _) => {
                // Per-element equations: each slot is its own (possibly
                // distinct) equation for a *specific* element, so there is no
                // iterated dimension in scope -- a sliced reducer in a slot
                // can only `Pinned`/`Reduced` its source axes. A reducer that
                // *is* an element's whole RHS still mints a synthetic agg here
                // (the variable as a whole is not the aggregate -- different
                // elements may reduce differently). Visit slots in canonical
                // element-key order for determinism.
                let ctx = AggWalkCtx {
                    variables: &variables,
                    target_iterated_dims: &[],
                    dm_dims: dm_dims_ref,
                    dim_ctx,
                };
                let mut elem_keys: Vec<_> = per_elem.keys().collect();
                elem_keys.sort();
                for k in elem_keys {
                    walk_subexpr_for_aggs(
                        &per_elem[k],
                        &var_name_str,
                        &ctx,
                        &mut result,
                        &mut next_synthetic_n,
                        /* in_reducer = */ false,
                    );
                }
                if let Some(default) = default_expr {
                    walk_subexpr_for_aggs(
                        default,
                        &var_name_str,
                        &ctx,
                        &mut result,
                        &mut next_synthetic_n,
                        false,
                    );
                }
            }
        }
    }

    result
}

/// Read-only walk context shared by [`walk_var_equation`] /
/// [`walk_subexpr_for_aggs`] for a single target variable: the model's
/// variable map, the target equation's iterated dimension names (canonical,
/// in source order; empty for `Ast::Scalar` and per-element `Ast::Arrayed`
/// slots), the datamodel dimension list (used to map an `Iterated`
/// axis's canonical dim name back to datamodel casing for
/// `AggNode::result_dims`), and the project's `DimensionsContext` (the
/// GH #534 mapped-iterated-axis gate). Bundling these keeps the walkers'
/// signatures short; the mutable `result`/`next_synthetic_n` stay out of
/// band.
struct AggWalkCtx<'a> {
    variables: &'a HashMap<Ident<Canonical>, crate::variable::Variable>,
    target_iterated_dims: &'a [String],
    dm_dims: &'a [crate::datamodel::Dimension],
    dim_ctx: &'a crate::dimensions::DimensionsContext,
}

/// Walk the whole-RHS expression of a `Scalar` / `ApplyToAll` variable.
///
/// If the expression is *exactly* one maximal reducer App, the variable
/// itself is the aggregate node (no synthetic minted). Otherwise the
/// expression is walked for sub-expression reducers via
/// [`walk_subexpr_for_aggs`].
fn walk_var_equation(
    expr: &Expr2,
    var_name: &str,
    ctx: &AggWalkCtx<'_>,
    result: &mut AggNodesResult,
    next_synthetic_n: &mut usize,
) {
    if let Expr2::App(builtin, _, _) = expr
        && let Some(source_vars) = reducer_source_vars(builtin, ctx.variables)
        && let Some(slices) = combined_read_slice(builtin, ctx)
        // A whole-RHS reducer whose slice/result shape the variable-backed
        // machinery cannot express is NOT variable-backed: it falls through
        // to `walk_subexpr_for_aggs`, which mints a *synthetic* agg for the
        // same reducer text (at the cost of one synthetic aux duplicating
        // the variable's value) and rides the well-tested two-half scoring
        // + the GH #528 agg-to-target projection. See
        // [`variable_backed_shape_is_expressible`] for the one minting
        // condition (the GH #534 mapped carve-out, generalized to the
        // GH #764 broadcast/permuted result shapes by T4 of the
        // shape-expressiveness design). The expressibility check keys on
        // the CANONICAL (co-source) slice -- a projection feeder's
        // all-`Iterated` slice (GH #767) says nothing about the reducer's
        // result shape.
        && variable_backed_shape_is_expressible(&slices.canonical, ctx.target_iterated_dims)
        // `None` (a structurally-impossible missing per-var slice; see
        // `agg_sources`' rustdoc) falls through to `walk_subexpr_for_aggs`,
        // whose own `agg_sources` call declines identically -- the
        // reference stays on the conservative Direct path.
        && let Some(sources) = agg_sources(source_vars, &slices, ctx)
    {
        // Whole-RHS reducer: the variable IS the aggregate node. The agg
        // node's result shape is the *reducer's* result shape (the `Iterated`
        // axes' dims), not the owning variable's: a full reduce
        // (`share[Region] = SUM(pop[*])`) has `result_dims == []` even though
        // it is broadcast to an arrayed variable (every element holds the same
        // value); a partial reduce keyed by the active A2A dimension
        // (`rowsum[D1] = SUM(matrix[D1, *])`) keeps `[D1]` as its result dims.
        let key = crate::patch::expr2_to_string(expr);
        let result_dims = result_dims_from_read_slice(&slices.canonical, ctx.dm_dims);
        // DECLINE the degenerate square-source shape (repeated result dim,
        // GH #778/#785): the per-axis emission paths pin subscript indices by
        // dim name and disagree across the duplicated occurrence. Declining
        // here keeps the reducer off ALL of them with one decision; the
        // reference falls through to `walk_subexpr_for_aggs`, which declines
        // identically, leaving it on the conservative Direct path. See
        // [`result_dims_has_repeated_dim`].
        if !result_dims_has_repeated_dim(&result_dims) {
            register_agg(
                result,
                next_synthetic_n,
                &key,
                var_name,
                AggKind::VariableBacked {
                    var_name: var_name.to_string(),
                    result_dims,
                },
                sources,
            );
            return;
        }
    }
    walk_subexpr_for_aggs(
        expr,
        var_name,
        ctx,
        result,
        next_synthetic_n,
        /* in_reducer = */ false,
    );
}

/// The ONE minting condition for whole-RHS reducers (shape-expressiveness
/// design, T4): `true` when the variable-backed machinery -- the
/// [`variable_backed_reduce_agg`] gate, `try_cross_dimensional_link_scores`'
/// per-`(row, slot)` derivation, and `emit_agg_routed_edges`' source→slot
/// routing, all of which key slots by NAME against the owning variable's
/// element nodes -- can express this slice with the variable itself as the
/// aggregate node. `false` routes the reducer through
/// `walk_subexpr_for_aggs`, which mints a *synthetic* agg arrayed over the
/// slice's `Iterated` target dims instead.
///
/// Not expressible (⇒ synthetic):
/// - **Mapped iterated axis** (GH #534, `out[State] = SUM(matrix[State,*])`
///   over a positionally-mapped `State→Region` pair): the variable-backed
///   link-score path matches result axes against source axes BY NAME, so a
///   remapped pair falls off it onto `emit_per_shape_link_scores`'
///   `Wildcard` partial -- whose PREVIOUS-wrapping mangles the iterated
///   index into the non-compiling `matrix[PREVIOUS(state),*]` (a
///   silently-stubbed constant-0 score). The mapped clause is NOT subsumed
///   by the alignment clause below: in the CANONICAL GH #534 shape
///   (`out[State] = SUM(matrix[State,*])`, the owner's only dim is
///   `State`) the result dims ARE aligned -- the `Iterated` axis carries
///   the TARGET dim -- so the alignment comparison alone would wrongly
///   call it expressible; there the remap, not the shape, is what the
///   name-keyed path cannot express. But mapped does NOT imply aligned:
///   the two conditions co-occur (`out[State,D3] = SUM(matrix[State,*])`
///   is mapped AND broadcast). The mapped check simply fires first, and
///   the synthetic machinery handles the intersection cleanly -- the
///   source half remaps rows to slots and the GH #528 projection
///   broadcasts the agg over the extra owner dims (pinned end-to-end by
///   `whole_rhs_mapped_broadcast_intersection_scores_cleanly`). Do not
///   reorder or merge the clauses on an assumed mapped ⇒ aligned.
/// - **Non-aligned result dims** (GH #764): the `Iterated` axes' target
///   dims, in slice order, differ from the owner's declared dims -- a
///   BROADCAST (`out[D1,D3] = SUM(matrix[D1,*])`, strict subset) or a
///   PERMUTATION (`out[D2,D1] = SUM(cube[D1,D2,*])`, different order). A
///   per-`(row, slot)` slot must name a complete `to` element in declared
///   order, which neither shape's slots do; the synthetic agg's slots are
///   keyed by `result_dims` order and the GH #528 projection
///   (`emit_agg_to_target_link_scores`' per-ident pins /
///   `expand_same_element`'s name-matched projection) handles the
///   broadcast fan-out and the reordering.
///
/// Expressible (⇒ the variable IS the agg, byte-identical to pre-T4):
/// - an ALIGNED partial reduce (`Iterated` dims == the owner's dims, in
///   order -- Pinned/subset axes included);
/// - any slice with NO `Iterated` axis: the full-extent reduce
///   (`total = SUM(pop[*])`, `share[R] = SUM(pop[*])` -- the inert
///   reference-walker family), the scalar-owner Pinned/subset slice
///   (`total = SUM(pop[nyc,*])`, admitted by the gate), and the
///   ARRAYED-owner Pinned/subset BROADCAST slice
///   (`share[R] = SUM(pop[nyc,*])`, GH #777 -- the variable IS the agg
///   here too, with `result_dims` empty; the gate's broadcast arm and
///   `emit_broadcast_reduce_link_scores` fan its single value across the
///   owner's full element set, the design's section-3 `PerElement` rule
///   applied to a reducer owner).
///
/// `target_iterated_dims` are the owner's A2A dims (canonical, declared
/// order; empty for a scalar owner). An `Iterated` axis's `dim` is always
/// one of them by construction (`classify_axis_access` only mints
/// `Iterated` for a target iterated dim), so "non-aligned" here can also
/// mean a duplicated dim against a single-occurrence owner
/// (`out[D1] = SUM(sq[D1,D1,*])`, routed through `walk_subexpr_for_aggs`
/// where the square-source decline fires). Note an owner declared over the
/// SAME dim twice (`out2[D1,D1] = SUM(cube[D1,D1,*])`) genuinely compiles
/// and simulates (each slot reads its own full row), and its
/// `iterated_dims == target_iterated_dims` makes this function return
/// `true` -- it is `walk_var_equation`'s `result_dims_has_repeated_dim`
/// check (GH #778/#785, live and load-bearing, NOT defense-in-depth) that
/// declines the mint for that spelling, so neither a variable-backed nor a
/// synthetic agg is ever registered for a repeated-result-dim reduce.
fn variable_backed_shape_is_expressible(
    read_slice: &[AxisRead],
    target_iterated_dims: &[String],
) -> bool {
    let mut iterated_dims: Vec<&str> = Vec::new();
    for axis in read_slice {
        if let AxisRead::Iterated { dim, source_dim } = axis {
            if dim != source_dim {
                return false; // mapped pair (GH #534)
            }
            iterated_dims.push(dim.as_str());
        }
    }
    // Scalar-result slices (no Iterated axis) are always expressible-or-
    // gate-declined as the variable itself; an Iterated-armed slice must
    // align exactly with the owner's declared dims (GH #764).
    iterated_dims.is_empty()
        || iterated_dims
            .iter()
            .copied()
            .eq(target_iterated_dims.iter().map(String::as_str))
}

/// Recursively walk an expression looking for *maximal* reducer
/// subexpressions (a reducer App not nested inside another reducer App).
///
/// `in_reducer` is `true` once we have descended into a reducer's argument:
/// any reducer found there is *not* maximal and is skipped (only the
/// outermost reducer becomes an agg), but the walk still continues into it to
/// collect the outer agg's source variables -- handled by the caller via
/// [`reducer_source_vars`], so here we simply stop minting once inside a
/// reducer.
fn walk_subexpr_for_aggs(
    expr: &Expr2,
    owner_var: &str,
    ctx: &AggWalkCtx<'_>,
    result: &mut AggNodesResult,
    next_synthetic_n: &mut usize,
    in_reducer: bool,
) {
    match expr {
        Expr2::Const(..) | Expr2::Var(..) => {}
        Expr2::Subscript(_, indices, _, _) => {
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => walk_subexpr_for_aggs(
                        e,
                        owner_var,
                        ctx,
                        result,
                        next_synthetic_n,
                        in_reducer,
                    ),
                    IndexExpr2::Range(l, r, _) => {
                        walk_subexpr_for_aggs(
                            l,
                            owner_var,
                            ctx,
                            result,
                            next_synthetic_n,
                            in_reducer,
                        );
                        walk_subexpr_for_aggs(
                            r,
                            owner_var,
                            ctx,
                            result,
                            next_synthetic_n,
                            in_reducer,
                        );
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            // A maximal reducer subexpression is hoisted into a synthetic agg
            // iff every one of its arrayed source references reads a
            // *statically describable* slice -- `compute_read_slice` is `Some`
            // for each -- and the slices pass the I1 acceptance
            // (`accept_source_slices`: identical co-source slices plus
            // projection feeders, GH #767). That covers the whole-extent case
            // (`SUM(pop[*])` ⇒ all-`Reduced`), the slice cases
            // (`SUM(pop[NYC,*])` ⇒ `[Pinned(nyc), Reduced]`,
            // `SUM(matrix[D1,*])` over an A2A-`D1` body ⇒
            // `[Iterated(d1), Reduced]` → an arrayed agg over `D1`), and
            // declines only the dynamic-index carve-out (`SUM(pop[idx,*])`,
            // `idx` non-literal ⇒ not statically describable). A *whole-RHS*
            // reducer (`agg[D1] = SUM(matrix[D1, *])`) is recognized too, but
            // as a variable-backed agg via `walk_var_equation`, not here.
            // Hoist-eligibility prefix, computed in dependency order so the
            // slice/result-dims derivation runs only for an actual hoistable
            // reducer App (`reducer_source_vars` is `None` for every other
            // builtin -- it would be wasted work on the non-reducer majority).
            let source_vars = (!in_reducer)
                .then(|| reducer_source_vars(builtin, ctx.variables))
                .flatten();
            let slices = source_vars
                .is_some()
                .then(|| combined_read_slice(builtin, ctx))
                .flatten();
            let result_dims = slices
                .as_ref()
                .map(|s| result_dims_from_read_slice(&s.canonical, ctx.dm_dims));
            // DECLINE the degenerate square-source shape (repeated result
            // dim, GH #778/#785): the per-axis emission paths pin subscript
            // indices by dim name and disagree across the duplicated
            // occurrence. Declining here routes the reducer onto the same
            // `else` descent the not-statically-describable carve-outs take,
            // with `in_reducer` unchanged so the source references keep their
            // conservative Direct shape. See [`result_dims_has_repeated_dim`].
            let square_source = result_dims
                .as_deref()
                .is_some_and(result_dims_has_repeated_dim);
            if !square_source
                && let Some(source_vars) = source_vars
                && let Some(slices) = slices
                && let Some(result_dims) = result_dims
                // `None` (a structurally-impossible missing per-var slice;
                // see `agg_sources`' rustdoc) declines the hoist: the `else`
                // arm descends with `in_reducer` unchanged, exactly like the
                // not-statically-describable carve-outs.
                && let Some(sources) = agg_sources(source_vars, &slices, ctx)
            {
                let key = crate::patch::expr2_to_string(expr);
                register_agg(
                    result,
                    next_synthetic_n,
                    &key,
                    owner_var,
                    AggKind::Synthetic { result_dims },
                    sources,
                );
                // Descend with `in_reducer = true` so nested reducers are
                // not separately minted, but index expressions etc. are
                // still traversed.
                builtin.for_each_expr_ref(|sub| {
                    walk_subexpr_for_aggs(
                        sub,
                        owner_var,
                        ctx,
                        result,
                        next_synthetic_n,
                        /* in_reducer = */ true,
                    )
                });
            } else {
                builtin.for_each_expr_ref(|sub| {
                    walk_subexpr_for_aggs(sub, owner_var, ctx, result, next_synthetic_n, in_reducer)
                });
            }
        }
        Expr2::Op1(_, operand, _, _) => walk_subexpr_for_aggs(
            operand,
            owner_var,
            ctx,
            result,
            next_synthetic_n,
            in_reducer,
        ),
        Expr2::Op2(_, left, right, _, _) => {
            walk_subexpr_for_aggs(left, owner_var, ctx, result, next_synthetic_n, in_reducer);
            walk_subexpr_for_aggs(right, owner_var, ctx, result, next_synthetic_n, in_reducer);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            walk_subexpr_for_aggs(cond, owner_var, ctx, result, next_synthetic_n, in_reducer);
            walk_subexpr_for_aggs(then_e, owner_var, ctx, result, next_synthetic_n, in_reducer);
            walk_subexpr_for_aggs(else_e, owner_var, ctx, result, next_synthetic_n, in_reducer);
        }
    }
}

/// What sort of aggregate node a reducer subexpression maps to.
enum AggKind {
    /// A `$⁚ltm⁚agg⁚{n}` auxiliary must be minted.
    Synthetic { result_dims: Vec<String> },
    /// The owning variable already is the aggregate node.
    VariableBacked {
        var_name: String,
        result_dims: Vec<String>,
    },
}

/// Build the per-source [`AggSource`] list for a hoisted reducer: one entry
/// per distinct source variable, SORTED by canonical name (invariant I3b --
/// deterministic salsa cache equality and emission order regardless of AST
/// occurrence order). Each ARRAYED source carries its OWN accepted slice
/// from [`CombinedReadSlices::per_var`] -- the canonical co-source slice
/// for a co-source, the all-`Iterated` projection slice for a feeder
/// (GH #767); each SCALAR source (a feeder, GH #737) carries an empty
/// slice (it has no axes -- invariant I2).
///
/// Returns `None` -- the caller declines the hoist, keeping the reference
/// on the conservative Direct path -- if an arrayed source has no
/// `per_var` entry. That cannot happen by construction
/// (`reducer_source_vars`/`collect_var_refs` and
/// `collect_arrayed_source_slices` walk the identical reference surface
/// with the same arrayed predicate), so the decline is purely defensive
/// (PR #784 review): the previous canonical-slice fallback would have
/// mislabelled a projection feeder -- whose slice differs from canonical
/// BY DESIGN -- as a co-source, silently corrupting the per-`(row, slot)`
/// link scores downstream.
fn agg_sources(
    source_vars: Vec<String>,
    slices: &CombinedReadSlices,
    ctx: &AggWalkCtx<'_>,
) -> Option<Vec<AggSource>> {
    // `reducer_source_vars` already sorts + dedups; re-establishing the
    // invariant locally keeps it independent of the caller.
    let mut names = source_vars;
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|var| {
            let arrayed = ctx
                .variables
                .get(&Ident::<Canonical>::new(&var))
                .and_then(|v| v.get_dimensions())
                .map(|d| !d.is_empty())
                .unwrap_or(false);
            let read_slice = if arrayed {
                slices.per_var.get(&var).cloned()?
            } else {
                Vec::new()
            };
            Some(AggSource { var, read_slice })
        })
        .collect()
}

/// Register an aggregate node for `key` (canonical reducer text) and record
/// the `owner_var` -> agg-index association.
///
/// Synthetic aggs dedup on `key` (two inline uses of the same reducer
/// collapse to one `$⁚ltm⁚agg⁚{n}`). Variable-backed aggs are never deduped
/// -- each whole-RHS-reducer variable is its own distinct agg node, and an
/// inline use never reuses a variable-backed agg of the same text (that
/// would leave the inline reducer off the synthetic-agg path the downstream
/// `is_synthetic` filters require).
///
/// Determinism: `next_synthetic_n` is incremented only on a *new* synthetic
/// mint, in first-encounter order over the canonical-sorted variable list,
/// so two consumers walking the same ASTs see identical names.
fn register_agg(
    result: &mut AggNodesResult,
    next_synthetic_n: &mut usize,
    key: &str,
    owner_var: &str,
    kind: AggKind,
    sources: Vec<AggSource>,
) {
    let idx = match kind {
        AggKind::Synthetic { result_dims } => {
            if let Some(&existing) = result.synthetic_by_key.get(key) {
                existing
            } else {
                let name = synthetic_agg_name(*next_synthetic_n);
                *next_synthetic_n += 1;
                result.aggs.push(AggNode {
                    name,
                    equation_text: key.to_string(),
                    result_dims,
                    sources,
                    is_synthetic: true,
                });
                let idx = result.aggs.len() - 1;
                result.synthetic_by_key.insert(key.to_string(), idx);
                idx
            }
        }
        AggKind::VariableBacked {
            var_name,
            result_dims,
        } => {
            // Each whole-RHS-reducer variable is its own aggregate node;
            // never deduped, and not entered in `synthetic_by_key`.
            result.aggs.push(AggNode {
                name: var_name,
                equation_text: key.to_string(),
                result_dims,
                sources,
                is_synthetic: false,
            });
            result.aggs.len() - 1
        }
    };
    let entry = result.by_var.entry(owner_var.to_string()).or_default();
    if !entry.contains(&idx) {
        entry.push(idx);
    }
}

/// If `builtin` is an array-reducing function (per [`reducer_is_hoistable`])
/// applied to at least one arrayed model variable, return the set of
/// model-variable names it reads (recursively, across the reducer's
/// arguments). Otherwise return `None`.
///
/// `SIZE` is intentionally excluded by `reducer_is_hoistable` -- its link
/// score is always 0, mirroring `try_cross_dimensional_link_scores`'s
/// `Some(vec![])` for SIZE -- so a `SIZE(...)` subexpression is not hoisted.
///
/// A reducer is only recognized when at least one of its source variables is
/// arrayed (a scalar argument to `SUM`/`MEAN` is a no-op the parser would
/// normally reject anyway, and is never hoisted).
fn reducer_source_vars(
    builtin: &BuiltinFn<Expr2>,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
) -> Option<Vec<String>> {
    if !reducer_is_hoistable(builtin) {
        return None;
    }

    let mut sources: Vec<String> = Vec::new();
    builtin.for_each_expr_ref(|arg| collect_var_refs(arg, &mut sources));
    // `collect_var_refs` picks up every identifier appearing in the
    // expression, which inside a subscript includes dimension names
    // (`matrix[D1, *]`) and literal element names (`pop[NYC]`). Keep only
    // identifiers that are actually model variables.
    sources.retain(|name| variables.contains_key(&Ident::<Canonical>::new(name)));
    if sources.is_empty() {
        return None;
    }
    // Require at least one arrayed source. Module variables are scalar nodes
    // in the causal graph and never count as an arrayed reducer source.
    let has_arrayed_source = sources.iter().any(|name| {
        variables
            .get(&Ident::<Canonical>::new(name))
            .and_then(|v| v.get_dimensions())
            .map(|dims| !dims.is_empty())
            .unwrap_or(false)
    });
    if !has_arrayed_source {
        return None;
    }
    sources.sort();
    sources.dedup();
    Some(sources)
}

/// Compute the *read slice* of one reference (`arg_expr`) into an arrayed
/// model variable: one [`AxisRead`] per source axis (in the source's declared
/// dimension order), describing which rows of the source the reference reads.
/// `None` means "not statically describable" -- the reference is not a direct
/// `Subscript`/`Var` on an arrayed model variable, or it indexes an axis with
/// a non-literal index (`pop[idx, *]`, a `Range`, a `@N` position), so the
/// enclosing reducer is not hoisted (the dynamic-index carve-out).
///
/// Per source axis `i`, the access is decided by [`classify_axis_access`]
/// (one shared per-axis classifier; see its rustdoc for the per-index
/// rules).
///
/// A bare `Expr2::Var(source, ..)` arg (no subscript) on an arrayed source ⇒
/// all-`Reduced` (`[Reduced{subset: None}; source.dims.len()]`). A reference
/// to a *scalar* model variable ⇒ `None` (it's not a reducer source). A
/// `Subscript` whose index count doesn't match the source's dimension count
/// ⇒ `None` (conservative -- a partial subscript is not the case Phase 4
/// hoists).
fn compute_read_slice(arg_expr: &Expr2, ctx: &AggWalkCtx<'_>) -> Option<Vec<AxisRead>> {
    let variables = ctx.variables;
    let source_dims = |ident: &Ident<Canonical>| -> Option<&[crate::dimensions::Dimension]> {
        let dims = variables.get(ident).and_then(|v| v.get_dimensions())?;
        if dims.is_empty() { None } else { Some(dims) }
    };

    match arg_expr {
        Expr2::Var(ident, _, _) => {
            // A bare arrayed-variable arg reads the whole array.
            let dims = source_dims(ident)?;
            Some(vec![AxisRead::Reduced { subset: None }; dims.len()])
        }
        Expr2::Subscript(ident, indices, _, _) => {
            let dims = source_dims(ident)?;
            // A partial subscript (fewer/more indices than the source has
            // dimensions) is not the case Phase 4 hoists -- stay conservative.
            if indices.len() != dims.len() {
                return None;
            }
            indices
                .iter()
                .zip(dims)
                .map(|(idx, axis_dim)| {
                    classify_axis_access(idx, axis_dim, ctx.target_iterated_dims, ctx.dim_ctx)
                })
                .collect()
        }
        _ => None,
    }
}

/// Classify ONE subscript index against one source axis -- the single
/// per-axis access classifier (shape-expressiveness design, T1/T6). Shared
/// by [`compute_read_slice`] (reducer args) AND the direct-reference
/// classifier (`db::ltm_ir::classify_iterated_dim_shape`, which rejects
/// `Reduced` results -- a non-reducer reference never collapses an axis),
/// so the reducer path and the reference path can never disagree about an
/// axis.
///
/// Returns `None` for anything not statically describable (a dynamic
/// expression, a `@N` position, a `Range`, a declined mapping, a StarRange
/// naming a non-subdimension) -- the enclosing reducer is then not hoisted
/// and its reference stays on the conservative path. Per index:
///
/// - `IndexExpr2::Wildcard(_)` ⇒ `Reduced{subset: None}` (full extent).
/// - `IndexExpr2::StarRange(D, _)` (GH #766): `D` the axis's own dimension
///   ⇒ `Reduced{subset: None}` (full extent, byte-identical to `*`); `D` a
///   PROPER subdimension of the axis's dimension ⇒ `Reduced{subset:
///   Some(elems)}`, the subdimension's elements resolved via
///   [`crate::dimensions::DimensionsContext::get_subdimension_relation`]
///   (a subdimension that covers the whole axis normalizes back to
///   `subset: None`, invariant I3); `D` neither ⇒ **decline** -- such a
///   subscript is at best a mid-edit inconsistency and must not silently
///   widen to the full extent.
/// - `IndexExpr2::Expr(Expr2::Var(d, ..))` where `d` (canonical) is one of
///   the *target equation's* iterated dimensions AND matches the source's
///   axis dimension either *by name* or via a usable
///   [`iterated_axis_slot_elements`] remap -- which consults
///   `mapped_element_correspondence` and therefore accepts a positional
///   dimension MAPPING declared in EITHER direction (GH #757 widened the
///   former `has_mapping_to(d, src)` forward-declared-only gate; the
///   correspondence helper already handled both declaration directions and
///   carries the GH #756 positional-only gate)
///   ⇒ [`AxisRead::Iterated`] carrying the `(d, src)` pair (GH #534). The
///   three `Iterated`-axis consumers (`emit_agg_routed_edges`,
///   `read_slice_rows` behind `emit_source_to_agg_link_scores`, and
///   `emit_agg_to_target_link_scores` via `result_dims`) remap each source
///   row to the slot of its positionally-corresponding target element
///   through the same helper. Declined (⇒ `None`, conservative): an
///   explicit element-mapped pair (execution resolves positionally and
///   ignores the map -- GH #756), an unmapped position-mismatched pair,
///   and a non-positional size mismatch.
///   (`classify_iterated_dim_shape` consumes this classifier directly
///   since T6, so the direct-reference path and the reducer path accept
///   the identical mapped set by construction.)
/// - `IndexExpr2::Expr(Expr2::Var(elem, ..))` or `Expr2::Const` resolving to
///   a literal element / 1-based index of the axis's dimension ⇒
///   [`AxisRead::Pinned`] carrying that element's canonical name.
/// - anything else (`DimPosition`, `Range`, a non-literal `Expr`, a
///   `Var`/`Const` that resolves to neither an iterated dim nor a literal
///   element) ⇒ `None`.
pub(crate) fn classify_axis_access(
    idx: &IndexExpr2,
    axis_dim: &crate::dimensions::Dimension,
    target_iterated_dims: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<AxisRead> {
    match idx {
        IndexExpr2::Wildcard(_) => Some(AxisRead::Reduced { subset: None }),
        IndexExpr2::StarRange(named, _) => {
            let axis_canon = axis_dim.canonical_name();
            if named == axis_canon {
                // `*:D` over the axis's own dimension: the full extent.
                return Some(AxisRead::Reduced { subset: None });
            }
            let rel = dim_ctx.get_subdimension_relation(named, axis_canon)?;
            let elems = crate::ltm_augment::dimension_element_names(axis_dim);
            if rel.parent_offsets.is_empty() || rel.parent_offsets.len() >= elems.len() {
                // Empty subdimensions don't exist in well-formed models
                // (decline defensively); a same-cardinality "sub" is the
                // same element SET as the axis (containment + equal size),
                // so it normalizes to the full extent -- keeping the
                // full-extent representation unique (invariant I3).
                return if rel.parent_offsets.len() == elems.len() {
                    Some(AxisRead::Reduced { subset: None })
                } else {
                    None
                };
            }
            let subset: Vec<String> = rel
                .parent_offsets
                .iter()
                .map(|&o| elems.get(o).cloned())
                .collect::<Option<_>>()?;
            Some(AxisRead::Reduced {
                subset: Some(subset),
            })
        }
        IndexExpr2::Range(_, _, _) | IndexExpr2::DimPosition(_, _) => None,
        IndexExpr2::Expr(Expr2::Var(name, _, _)) => {
            let name_str = name.as_str();
            let src_dim_name = axis_dim.name();
            // An iterated-dimension index: the axis is iterated over the
            // target's dimension space (and the agg result varies per
            // element of it) iff `name` is one of the target's iterated
            // dims AND it lines up with the source's axis dim by name or by
            // a positional mapping (GH #534).
            if target_iterated_dims.iter().any(|t| t == name_str) {
                if name_str == src_dim_name {
                    Some(AxisRead::Iterated {
                        dim: name_str.to_string(),
                        source_dim: src_dim_name.to_string(),
                    })
                } else {
                    // The iterated dim names a *different* source axis: a
                    // positional remap (`State→Region`, GH #534) is accepted
                    // -- carrying the (target, source) pair so the emitters
                    // remap each row to its slot -- when the slot remap
                    // exists. `iterated_axis_slot_elements` consults
                    // `mapped_element_correspondence`, which accepts BOTH
                    // declaration directions (GH #757 -- the former
                    // `has_mapping_to(d, src)` forward-only pre-gate was
                    // dropped) and declines explicit element maps (execution
                    // resolves positionally and ignores the map, the GH #756
                    // gate). Everything else -- a plain position mismatch,
                    // an element-mapped pair -- declines, keeping the
                    // reference on the conservative path.
                    let elems = crate::ltm_augment::dimension_element_names(axis_dim);
                    if iterated_axis_slot_elements(name_str, src_dim_name, &elems, dim_ctx)
                        .is_some()
                    {
                        Some(AxisRead::Iterated {
                            dim: name_str.to_string(),
                            source_dim: src_dim_name.to_string(),
                        })
                    } else {
                        None
                    }
                }
            } else {
                resolve_literal_axis_index(idx, axis_dim).map(AxisRead::Pinned)
            }
        }
        IndexExpr2::Expr(Expr2::Const(..)) => {
            resolve_literal_axis_index(idx, axis_dim).map(AxisRead::Pinned)
        }
        IndexExpr2::Expr(_) => None,
    }
}

/// Resolve a single subscript index to a literal element name (canonical
/// lowercase) of `dim`, or `None` for any other shape. The `Expr2`-side
/// sibling of `db::ltm_ir::resolve_literal_index` / `ltm_augment`'s Expr0
/// `resolve_literal_element_index`: element names parse as `Expr2::Var`
/// (the parser keeps the raw element identifier as a `Var`; numeric-offset
/// resolution happens later in Expr3 lowering); integer literals (used for
/// indexed dimensions) parse as `Expr2::Const`. For an indexed dimension the
/// literal is canonicalized via parse-then-format so `pop[01]` reduces to
/// `"1"`, matching the element names `dimension_element_names` produces.
fn resolve_literal_axis_index(
    idx: &IndexExpr2,
    dim: &crate::dimensions::Dimension,
) -> Option<String> {
    let canonical = match idx {
        IndexExpr2::Expr(Expr2::Var(ident, _, _)) => ident.as_str().to_string(),
        IndexExpr2::Expr(Expr2::Const(text, _, _)) => canonicalize(text).into_owned(),
        _ => return None,
    };
    match dim {
        crate::dimensions::Dimension::Named(_, named) => {
            if named.elements.iter().any(|e| e.as_str() == canonical) {
                Some(canonical)
            } else {
                None
            }
        }
        crate::dimensions::Dimension::Indexed(_, size) => {
            if let Ok(n) = canonical.parse::<u32>()
                && n >= 1
                && n <= *size
            {
                Some(n.to_string())
            } else {
                None
            }
        }
    }
}

/// The accepted per-source read slices of a hoisted reducer (invariant I1
/// of the shape-expressiveness design): the CANONICAL slice -- the shared
/// co-source slice, or (for a degenerate agg with no `Reduced`-bearing
/// source) the shared all-source slice -- plus each arrayed source
/// variable's own slice. Built by [`combined_read_slice`]; the walkers
/// derive the agg's result shape from `canonical` and its [`AggSource`]s
/// from `per_var`.
struct CombinedReadSlices {
    canonical: Vec<AxisRead>,
    per_var: HashMap<String, Vec<AxisRead>>,
}

/// Compute the per-source read slices of a reducer `builtin`'s arrayed
/// source references: walk its argument expressions, collect every
/// reference to an arrayed model variable, [`compute_read_slice`] each,
/// and apply the I1 acceptance ([`accept_source_slices`]). `None` -- the
/// reducer is not hoisted -- when no arrayed reference exists, when any
/// reference is not statically describable (the dynamic-index carve-out),
/// or when the references' slices fall outside the acceptance rule.
fn combined_read_slice(
    builtin: &BuiltinFn<Expr2>,
    ctx: &AggWalkCtx<'_>,
) -> Option<CombinedReadSlices> {
    let mut refs: Vec<(String, Vec<AxisRead>)> = Vec::new();
    let mut ok = true;
    builtin.for_each_expr_ref(|arg| {
        if ok {
            collect_arrayed_source_slices(arg, ctx, &mut refs, &mut ok);
        }
    });
    if !ok || refs.is_empty() {
        return None;
    }
    accept_source_slices(refs)
}

/// The I1 acceptance rule over a reducer's arrayed-reference slices (T5 of
/// the shape-expressiveness design, GH #767). Sources split into
/// *co-sources* (>= 1 [`AxisRead::Reduced`] axis) and *feeders* (none):
///
/// - **I3b (one slice per var)**: a variable referenced with two different
///   slices declines (downstream consumers key `sources` by name).
/// - **Co-sources** must all carry the IDENTICAL slice -- the *canonical
///   slice* (same `Pinned` elements, same `Iterated` pairs in axis order,
///   same `Reduced` subsets; two co-sources with different subsets would
///   disagree on the co-reduced rows per slot).
/// - **Feeders** are accepted iff the slice consists ONLY of UNMAPPED
///   `Iterated` axes whose target dims equal the canonical slice's
///   `Iterated` target dims, in order -- the projection of the canonical
///   slice onto its iterated axes, so [`crate::db`]'s `read_slice_rows`
///   derives 1:1 feeder rows-to-slots and the per-row changed-last feeder
///   equation can pin the slot element into the reducer text. The
///   Iterated-only requirement is on the FEEDER's slice; a Pinned-bearing
///   CANONICAL slice (`SUM(cube[D1, c1, *] * frac[D1])`) is in scope. The ordered
///   EQUALITY (not the design's looser "drawn from the set" subset
///   wording) is deliberate: a proper-subset feeder's rows would each
///   feed every slot they project from -- a broadcast the per-`(row,
///   slot)` machinery cannot name -- and a permuted feeder's
///   `read_slice_rows` slots (derived in the source's axis order) would
///   mis-name the `result_dims`-ordered agg slots. Both decline
///   (conservative + loud, today's behavior). A MAPPED `Iterated` axis
///   (GH #534) anywhere in the combination declines too: the feeder
///   equation pins the TARGET-dim slot element into the reducer text,
///   which is not the source row a mapped reference reads.
/// - **No co-source at all** (every arrayed source all-`Iterated`, e.g. a
///   scalar-valued `SUM(frac[D1])` argument): the pre-T5 identical-slices
///   rule applies byte-identically, with the shared slice as the
///   canonical one.
fn accept_source_slices(refs: Vec<(String, Vec<AxisRead>)>) -> Option<CombinedReadSlices> {
    use std::collections::hash_map::Entry;
    // I3b: one slice per variable.
    let mut per_var: HashMap<String, Vec<AxisRead>> = HashMap::new();
    for (var, slice) in refs {
        match per_var.entry(var) {
            Entry::Occupied(e) => {
                if *e.get() != slice {
                    return None;
                }
            }
            Entry::Vacant(e) => {
                e.insert(slice);
            }
        }
    }
    let has_reduced = |s: &[AxisRead]| s.iter().any(|ax| matches!(ax, AxisRead::Reduced { .. }));
    // All co-sources must agree on one canonical slice. (Order-independent:
    // pairwise equality is what is checked.)
    let mut canonical: Option<&[AxisRead]> = None;
    for slice in per_var.values().filter(|s| has_reduced(s)) {
        match canonical {
            None => canonical = Some(slice),
            Some(c) if c == slice.as_slice() => {}
            Some(_) => return None,
        }
    }
    let Some(canonical) = canonical else {
        // No co-source: keep the pre-T5 identical-slices rule.
        let mut slices = per_var.values();
        let first = slices.next().expect("refs is non-empty").clone();
        if slices.any(|s| *s != first) {
            return None;
        }
        return Some(CombinedReadSlices {
            canonical: first,
            per_var,
        });
    };
    let canonical = canonical.to_vec();
    // The feeder clause (see the rustdoc).
    fn unmapped_iterated_dims(s: &[AxisRead]) -> Option<Vec<&str>> {
        s.iter()
            .filter_map(|ax| match ax {
                AxisRead::Iterated { dim, source_dim } => {
                    Some((dim == source_dim).then_some(dim.as_str()))
                }
                AxisRead::Pinned(_) | AxisRead::Reduced { .. } => None,
            })
            .collect()
    }
    let feeders: Vec<&Vec<AxisRead>> = per_var.values().filter(|s| !has_reduced(s)).collect();
    if !feeders.is_empty() {
        // `None` here means a mapped Iterated axis is present.
        let canonical_dims = unmapped_iterated_dims(&canonical)?;
        for feeder in feeders {
            if feeder.len() != canonical_dims.len()
                || feeder
                    .iter()
                    .any(|ax| !matches!(ax, AxisRead::Iterated { .. }))
                || unmapped_iterated_dims(feeder).as_deref() != Some(&canonical_dims)
            {
                return None;
            }
        }
    }
    Some(CombinedReadSlices { canonical, per_var })
}

/// Recursive helper for [`combined_read_slice`]: descend `expr` (and any
/// nested subscript index expressions), pushing each arrayed-source-variable
/// reference's `(var, compute_read_slice)` pair into `refs` (and clearing
/// `ok` on a not-statically-describable `None`). Acceptance over the
/// collected pairs is [`accept_source_slices`]'s job. Scalar-variable
/// references are ignored (a scalar argument to a reducer is not a per-row
/// reducer source; it joins `sources` later with an empty slice).
fn collect_arrayed_source_slices(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    refs: &mut Vec<(String, Vec<AxisRead>)>,
    ok: &mut bool,
) {
    if !*ok {
        return;
    }
    let is_arrayed = |ident: &Ident<Canonical>| -> bool {
        ctx.variables
            .get(ident)
            .and_then(|v| v.get_dimensions())
            .map(|d| !d.is_empty())
            .unwrap_or(false)
    };
    fn push(
        ident: &Ident<Canonical>,
        slice: Option<Vec<AxisRead>>,
        refs: &mut Vec<(String, Vec<AxisRead>)>,
        ok: &mut bool,
    ) {
        match slice {
            None => *ok = false,
            Some(s) => refs.push((ident.as_str().to_string(), s)),
        }
    }
    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _, _) => {
            if ctx.variables.contains_key(ident) && is_arrayed(ident) {
                push(ident, compute_read_slice(expr, ctx), refs, ok);
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            if ctx.variables.contains_key(ident) && is_arrayed(ident) {
                push(ident, compute_read_slice(expr, ctx), refs, ok);
            }
            // Also descend into index expressions (a nested source ref).
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => collect_arrayed_source_slices(e, ctx, refs, ok),
                    IndexExpr2::Range(l, r, _) => {
                        collect_arrayed_source_slices(l, ctx, refs, ok);
                        collect_arrayed_source_slices(r, ctx, refs, ok);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            builtin.for_each_expr_ref(|sub| collect_arrayed_source_slices(sub, ctx, refs, ok));
        }
        Expr2::Op1(_, operand, _, _) => collect_arrayed_source_slices(operand, ctx, refs, ok),
        Expr2::Op2(_, left, right, _, _) => {
            collect_arrayed_source_slices(left, ctx, refs, ok);
            collect_arrayed_source_slices(right, ctx, refs, ok);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            collect_arrayed_source_slices(cond, ctx, refs, ok);
            collect_arrayed_source_slices(then_e, ctx, refs, ok);
            collect_arrayed_source_slices(else_e, ctx, refs, ok);
        }
    }
}

/// Map a read slice's [`AxisRead::Iterated`] axes to their datamodel-cased
/// dimension names, in order -- the agg's [`AggNode::result_dims`]. A
/// whole-extent reduce (all-`Reduced`) yields `[]`; a slice over an iterated
/// dim (`SUM(matrix[D1, *])` over an A2A-`D1` body, `read_slice =
/// [Iterated(d1), Reduced]`) yields `["D1"]`.
fn result_dims_from_read_slice(
    read_slice: &[AxisRead],
    dm_dims: &[crate::datamodel::Dimension],
) -> Vec<String> {
    read_slice
        .iter()
        .filter_map(|a| match a {
            // The TARGET dim of the pair: the agg variable is arrayed over
            // the target equation's iterated dimension (`State` for the
            // GH #534 mapped case), which is what the agg's own A2A
            // equation, the agg→target projection (GH #528), and the
            // element-graph slot naming all key on.
            AxisRead::Iterated { dim, .. } => Some(canonical_dim_to_datamodel(dim, dm_dims)),
            AxisRead::Pinned(_) | AxisRead::Reduced { .. } => None,
        })
        .collect()
}

/// `true` when a reducer's would-be `result_dims` (the canonical slice's
/// `Iterated` TARGET dims, in order -- see [`result_dims_from_read_slice`])
/// repeat a dimension. That is the DEGENERATE SQUARE-SOURCE shape: a reducer
/// whose iterated axes carry the same target dim twice
/// (`out[D1] = SUM(sq[D1, D1, *])` over a square `sq[D1, D1, D3]`, the inline
/// `out[D1] = base[D1] + SUM(sq[D1, D1, *])`, or with a co-source feeder
/// `x[D1] = 1 + SUM(cube[D1, D1, *] * frac[D1, D1])`).
///
/// The executed A2A simulation reads only the DIAGONAL of such a source
/// (`sq[e, e, *]` per target slot `e`), but the agg's result-slot
/// enumeration would range over the full `[D1, D1]` square. Every per-axis
/// emission path that pins a subscript index BY DIM NAME is then ambiguous
/// across the two `D1` occurrences (the first-match hazard), which is why
/// the three halves disagreed on this shape:
///
/// - the source→slot element graph fans out ALL `[D1, D1]` slots including
///   the off-diagonal ones the simulation never reads (GH #778);
/// - the link-score projection (`result_dim_positions`) collapsed both `D1`
///   occurrences to one target position, emitting diagonal-only agg→target
///   names that disagree with that edge fan-out;
/// - the co-source row partial (`pin_body_to_row`) emitted confident
///   per-`(row, slot)` scores on the phantom off-diagonal edges the
///   simulation never reads -- a SILENT wrong number on the link surface
///   (GH #785).
///
/// The feeder half was independently defended by PR #787
/// (`pin_iterated_dim_indices`' ambiguity bail), but only ONE half. Rather
/// than teach every per-axis path to resolve a repeated dim positionally
/// (the larger "diagonalize the axis into the `AxisRead` vocabulary"
/// alternative), this rare shape is DECLINED at the single agg-minting gate
/// (`enumerate_agg_nodes`' two mint sites), so all halves and both surfaces
/// (edges and scores) inherit one decision per the epic's "two-surface
/// decisions share one predicate" invariant. A declined reducer keeps its
/// references on the conservative paths; the duplicated-dim co-source's
/// remaining landing (`try_cross_dimensional_link_scores`' cartesian
/// partial-reduce branch, whose own `from_pos` map has the same first-match
/// hazard) is closed in lockstep by the loud `#758`/`#780` skip
/// (`emit_unscoreable_duplicated_dim_source_warning`), so NO surface carries
/// an unwarned wrong number for this shape.
///
/// Keyed on `result_dims` (already the canonical slice's `Iterated` target
/// dims) so synthetic AND variable-backed minting both inherit it from the
/// single place those dims are derived.
pub(crate) fn result_dims_has_repeated_dim(result_dims: &[String]) -> bool {
    result_dims
        .iter()
        .enumerate()
        .any(|(i, d)| result_dims[..i].contains(d))
}

/// `true` when `name` is a synthetic aggregate-node name (`$⁚ltm⁚agg⁚{n}`).
pub(crate) fn is_synthetic_agg_name(name: &str) -> bool {
    name.starts_with(AGG_NAME_PREFIX)
}

/// The variable-backed REDUCE aggregate node for the causal edge
/// `from -> to`, if any (GH #752, generalized by T3 of the
/// shape-expressiveness design / GH #765): `to`'s entire dt-equation is a
/// reducer reading `from` (`to` IS the agg, `is_synthetic == false`) whose
/// slice is statically describable and *non-trivial* -- at least one
/// `Pinned`, subset-`Reduced`, or `Iterated` axis -- and whose result shape
/// the per-`(row, slot)` machinery can express:
///
/// - **Aligned partial reduce** (`row_sum[D1] = SUM(matrix[D1,*])`,
///   `outf[D1] = MEAN(cube[D1,x,*])`, `out[D1] = SUM(matrix[D1,*:Sub])`):
///   at least one `Iterated` axis and `result_dims` exactly `to`'s declared
///   dims, in order -- each agg result slot names a complete `to` element,
///   so the element graph routes the read-slice rows straight to
///   `to[<slot>]` (the diagonal family whose per-`(row, slot)` link scores
///   `try_cross_dimensional_link_scores` emits from the SAME
///   `read_slice_rows` derivation, invariant I4). Pinned/subset axes are
///   admitted: the score derivation fixes `Pinned` axes and enumerates
///   subsets by construction, so the divisor is the true read count and
///   unread rows get neither edges nor scores. (The T1-era Pinned/subset
///   exclusions were deleted atomically with that derivation swap --
///   deleting them first would have re-fired the 0.25-vs-0.5
///   silent-wrong-divisor hazard the old rustdoc documented.)
/// - **Scalar-result slice** (no `Iterated` axis -- Pinned and/or
///   subset-`Reduced` only):
///   - on a SCALAR owner (`total = SUM(pop[nyc,*])`,
///     `total = SUM(arr[*:Sub])`; `to_dims.is_empty()`): the slot is the
///     bare `to` node, so `emit_agg_routed_edges` emits exactly the read
///     rows into `to`, matching the per-read-row scores.
///   - on an ARRAYED owner (`share[Region] = SUM(pop[nyc,*])`; the GH #777
///     broadcast slice): the single scalar reducer value broadcasts over
///     `to`'s dims. `emit_agg_routed_edges` fans the read rows out across
///     `to`'s FULL element set (`pop[nyc,d2] → share[e]` for every `e`),
///     and `try_cross_dimensional_link_scores`' broadcast-reduce branch
///     emits the matching per-(read-row, full-target-element) scalar
///     scores -- the design's section-3 `PerElement` rule applied to a
///     variable-backed reducer owner (the `to[e]` subscript on the name is
///     the EXISTING per-(row, slot) grammar resolvers already handle). The
///     read rows are independent of `to`'s dims (every slot reads the same
///     slice), so the RELATED-dim spelling (`share[Region]`, `Region` a
///     source dim) and the DISJOINT-dim spelling (`share[D9]`, `D9` not a
///     source dim) are expressed identically.
///
/// Because the axis checks key on the CANONICAL (co-source) slice, the
/// gate also admits a FEEDER edge of an aligned partial reduce (GH #767 /
/// T5: `frac → growth` for `growth[D1] = SUM(matrix[D1,*] * frac[D1])`,
/// where `from`'s own slice is the all-`Iterated` projection) -- the
/// consumers route THAT edge by `from`'s own slice
/// ([`AggNode::source_is_projection_feeder`] is the discriminator), so the
/// feeder's element edges, per-circuit loop routing, and per-`(row, slot)`
/// changed-last scores cover the same 1:1 rows.
///
/// This is the single gate shared by the element-graph reroute
/// (`model_element_causal_edges`' `Direct` `Wildcard`/`DynamicIndex`
/// dispatch), the loop builder (`build_element_level_loops`' per-circuit
/// routing), and `try_cross_dimensional_link_scores`' row derivation, so
/// the three can never disagree about which edges carry per-`(row, slot)`
/// scores.
///
/// `None` (callers keep their conservative paths) for:
/// - a PURE full-extent slice (all `Reduced{subset: None}`:
///   `total = SUM(pop[*])`, `share[R] = SUM(pop[*,*])`): the reference
///   walker's reduction/broadcast edges already ARE the read rows, so
///   routing it through the gate would change nothing -- skipped to keep
///   the surface byte-identical (inert).
///
/// The Iterated-arm alignment check below (`result_dims` == `to`'s dims,
/// in order) is defense-in-depth since T4: a whole-RHS reduce with
/// NON-ALIGNED result dims (broadcast `out[D1,D3] = SUM(matrix[D1,*])` /
/// permuted axes, GH #764) never registers a variable-backed agg anymore
/// -- [`variable_backed_shape_is_expressible`] routes it to a synthetic
/// agg at minting -- so every Iterated-armed variable-backed agg reaching
/// this gate is aligned by construction.
pub(crate) fn variable_backed_reduce_agg<'a>(
    aggs: &'a AggNodesResult,
    from: &str,
    to: &str,
    to_dims: &[crate::dimensions::Dimension],
) -> Option<&'a AggNode> {
    aggs.aggs_in_var(to).find(|a| {
        if a.is_synthetic || a.name != to || !a.reads_var(from) {
            return false;
        }
        // The axis checks key on the CANONICAL (co-source) slice,
        // invariant I1, rather than `from`'s own slice: the gate decides
        // the *reducer's* shape, and `from` may be a scalar feeder
        // (`out[D1] = SUM(matrix[D1,*] * scale)`, empty slice) or an
        // iterated-dim projection feeder (GH #767, all-`Iterated` slice)
        // whose own slice says nothing about the reduction's axis split.
        let slice = a.canonical_read_slice();
        // Non-trivial: a pure full-extent slice (all `Reduced{subset:
        // None}`, which also covers the impossible empty slice) is the
        // inert skip in the rustdoc.
        if !slice
            .iter()
            .any(|ax| !matches!(ax, AxisRead::Reduced { subset: None }))
        {
            return false;
        }
        if slice
            .iter()
            .any(|ax| matches!(ax, AxisRead::Iterated { .. }))
        {
            // Aligned partial reduce: each slot names a complete `to`
            // element. Non-aligned (broadcast/permuted, GH #764) result
            // dims cannot occur here since T4 -- they mint synthetic aggs
            // at enumeration -- so this check is pure defense.
            a.result_dims.len() == to_dims.len()
                && a.result_dims
                    .iter()
                    .zip(to_dims)
                    .all(|(rd, td)| canonicalize(rd).as_ref() == td.name())
        } else {
            // Scalar-result Pinned/subset slice with no `Iterated` axis. For
            // a SCALAR owner the slot is the bare `to` node; for an ARRAYED
            // owner the single scalar value broadcasts over `to`'s dims, and
            // the per-(read-row, full-target-element) machinery (GH #777,
            // shared by `emit_agg_routed_edges`' broadcast fan-out and
            // `try_cross_dimensional_link_scores`' broadcast-reduce branch)
            // names every slot. Both are admitted.
            true
        }
    })
}

/// Whether `from` is a SCALAR FEEDER of the variable-backed reduce `to`
/// (GH #790): `to` IS a variable-backed aggregate node the shared
/// [`variable_backed_reduce_agg`] gate admits, `from` is one of its sources
/// carrying an EMPTY read slice (a scalar coefficient -- `scale` in
/// `growth[D1] = SUM(matrix[D1,*] * scale)`), and the agg's canonical slice
/// carries a genuine `Reduced` axis (a real reduction exists for the scalar
/// to feed). Returns the variable-backed `AggNode` so the caller can emit
/// the single changed-last feeder score
/// ([`crate::ltm_augment::generate_scalar_feeder_to_agg_equation`],
/// dimensioned over `result_dims` -- or over the OWNER's dims for the
/// GH #777 broadcast slice, whose `result_dims` are empty while the owner is
/// arrayed), exactly as the synthetic-agg arm of
/// `emit_source_to_agg_link_scores` does for the SUBEXPRESSION spelling
/// (`0.1 + SUM(matrix[D1,*] * scale)`).
///
/// This is the scalar sibling of [`AggNode::source_is_projection_feeder`]
/// (which discriminates the ARRAYED iterated-dim projection feeder, GH #767):
/// both feed a hoisted reduce per result slot, but a scalar feeder's value is
/// constant across the whole co-reduced slice, so its single A2A score
/// suffices where the arrayed feeder needs per-`(row, slot)` scalars. Gated on
/// the SAME `variable_backed_reduce_agg` decision the element graph and the
/// loop builder consult, so the emitted Bare A2A name is exactly the hop the
/// per-slot loops reference (subscripted-after-quote by
/// `loop_link_score_ref`).
pub(crate) fn scalar_feeder_of_variable_backed_agg<'a>(
    aggs: &'a AggNodesResult,
    from: &str,
    to: &str,
    to_dims: &[crate::dimensions::Dimension],
) -> Option<&'a AggNode> {
    let agg = variable_backed_reduce_agg(aggs, from, to, to_dims)?;
    // `from` must be a SCALAR source: an empty read slice. A non-source's
    // `source_read_slice` is also empty, but `variable_backed_reduce_agg`
    // already required `reads_var(from)`, so an empty slice here means a
    // genuine scalar feeder (every arrayed co-source/feeder carries a
    // non-empty slice by invariant I2).
    if !agg.source_read_slice(from).is_empty() {
        return None;
    }
    // A genuine reduction must exist for the scalar to feed (the canonical
    // co-source slice carries a `Reduced` axis). Defense: a no-co-source agg
    // (all sources all-`Iterated`) is not a reduce a scalar feeds per slot.
    if !agg
        .canonical_read_slice()
        .iter()
        .any(|ax| matches!(ax, AxisRead::Reduced { .. }))
    {
        return None;
    }
    Some(agg)
}

/// How a NOT-hoisted reducer reads one of its arrayed sources -- the verdict
/// the legacy cartesian partial-/full-reduce derivation needs to decide
/// whether its per-`(row, slot)` projection is sound (GH #791).
///
/// `try_cross_dimensional_link_scores` only reaches the cartesian derivation
/// for an edge whose reducer minted NO usable variable-backed agg (every agg
/// lookup failed -- the I1-declined multi-source family, the dynamic-index
/// carve-out, etc.). The cartesian code then projects EVERY source element
/// onto the result axes by the source's DECLARED dimension positions and
/// scores each as if it were read. That projection is sound ONLY when the
/// reducer reads the FULL extent of `from`'s axes: a `Pinned` axis
/// (`SUM(pop[nyc,*] * w[*])`, where `pop`'s slice is `[Pinned(nyc), Reduced]`)
/// or a subset-`Reduced` axis means the read does NOT range over that axis, so
/// the projection both invents scores for UNREAD rows (`pop[boston,*]`) and
/// mis-divides the read rows (the un-pinnable mismatched-arity body dooms the
/// changed-first partial to the |dz/dz| = 1 fallback) -- a silent wrong number.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub(crate) enum UnhoistedSourceRead {
    /// The reducer reads the full extent of every `from` axis (all `Reduced`
    /// without a subset, or `Iterated` axes that range over their dimension):
    /// the cartesian rows ARE the read rows, so its projection is sound (the
    /// aligned `SUM(matrix[D1,*])` diagonal, the full-extent `SUM(pop[*])`).
    FullExtent,
    /// The reducer reads a STRICT slice of `from` -- at least one `Pinned`
    /// element or a subset-`Reduced` axis -- so the full-cartesian projection
    /// is unsound (scores unread rows / mis-divides read rows). The cartesian
    /// derivation must DECLINE this edge with the GH #758/#780 loud skip.
    /// Carries the representative strict slice (the FIRST strict read of
    /// `from` in deterministic walk order) so the diagnostic can show the
    /// user the actual slice their equation reads
    /// ([`render_read_slice_for_diagnostic`]) instead of a canned example.
    StrictSlice(Vec<AxisRead>),
    /// Not statically describable (a dynamic index `pop[idx,*]`, a declined
    /// mapping, a `@N`/`Range`), OR `from` is not a direct subscript/var
    /// reducer source in `to`'s equation. The conservative cartesian
    /// cross-product is the DOCUMENTED behavior for the dynamic-index family,
    /// so the caller keeps it (no decline).
    NotDescribable,
}

/// Render a read slice as a human-readable subscript for diagnostics --
/// `nyc,*` for `[Pinned(nyc), Reduced{None}]`. An `Iterated` axis renders as
/// its SOURCE dim's canonical name (the index the equation spells); a
/// subset-`Reduced` axis renders its resolved elements as `*:{a,b}` (the
/// [`AxisRead`] vocabulary carries the subdimension's elements, not its
/// name). Diagnostic-only: not parseable equation syntax.
pub(crate) fn render_read_slice_for_diagnostic(slice: &[AxisRead]) -> String {
    slice
        .iter()
        .map(|ax| match ax {
            AxisRead::Pinned(e) => e.clone(),
            AxisRead::Iterated { source_dim, .. } => source_dim.clone(),
            AxisRead::Reduced { subset: None } => "*".to_string(),
            AxisRead::Reduced {
                subset: Some(elems),
            } => format!("*:{{{}}}", elems.join(",")),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Classify how the NOT-hoisted reducer in `to`'s equation reads its arrayed
/// source `from`, for the GH #791 cartesian-derivation decline (whole-RHS
/// scalar/A2A owner) AND the GH #792 Bare-stand-in decline (per-element-equation
/// owner). Walks `to`'s owner expression(s) to the maximal reducer App that
/// reads `from` and runs the SAME per-axis classifier (`compute_read_slice` over
/// `classify_axis_access`) the hoisting path uses, so the decline predicate and
/// the agg-minting predicate can never disagree about whether a read is
/// full-extent.
///
/// For a `Scalar`/`ApplyToAll` owner the verdict is over its single
/// dt-expression. For an `Ast::Arrayed` (per-element-equation) owner -- the
/// GH #792 shape -- it is the per-slot combination: a strict slot declines the
/// WHOLE edge (the edge's one arrayed Bare stand-in conflates all slots, so a
/// strict slot proves it wrong), with the GH #744 full-extent escape staying
/// slot-local. See the `Ast::Arrayed` arm for the precedence rationale.
///
/// `from` may appear in a reducer more than once (a self-product) or in two
/// different slices; the single-expr classifier returns `StrictSlice` if ANY of
/// `from`'s reads in that expression is a strict slice and `NotDescribable` if
/// any is not statically describable -- either way the cartesian projection
/// cannot soundly attribute that source.
///
/// Salsa-tracked, keyed on the interned [`LtmLinkId`] (the
/// `link_score_equation_text` idiom): the body's `reconstruct_model_variables`
/// is the codebase's one UN-tracked whole-model reconstruction (O(all model
/// vars)), and this is its first per-edge caller -- tracking bounds that cost
/// to once per `(edge, revision)` so the pinned-loop pass's and discovery
/// mode's re-visits of the same edge are cache hits. Tracking was chosen over
/// threading the caller's variable map because
/// `try_cross_dimensional_link_scores` holds only `SourceVariable` handles
/// (not reconstructed `Variable`s with ASTs), so threading would have forced a
/// second dims-lookup vocabulary into the `AggWalkCtx` walkers.
#[salsa::tracked(returns(ref))]
pub(crate) fn unhoisted_reducer_source_read<'db>(
    db: &'db dyn Db,
    link: LtmLinkId<'db>,
    model: SourceModel,
    project: SourceProject,
) -> UnhoistedSourceRead {
    let from = link.link_from(db);
    let to = link.link_to(db);
    let variables = reconstruct_model_variables(db, model, project);
    let dm_dims = project_datamodel_dims(db, project);
    let dim_ctx = project_dimensions_context(db, project);

    let Some(to_var) = variables.get(&Ident::<Canonical>::new(to)) else {
        return UnhoistedSourceRead::NotDescribable;
    };
    let Some(ast) = to_var.ast() else {
        return UnhoistedSourceRead::NotDescribable;
    };
    // Mirror `enumerate_agg_nodes`' per-AST context: the A2A dims (and a
    // per-element owner's declared dims) are the target's iterated dimensions;
    // a scalar owner has none in scope.
    let target_iterated_dims: Vec<String> = match ast {
        Ast::Scalar(_) => vec![],
        Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _, _, _) => {
            dims.iter().map(|d| d.name().to_string()).collect()
        }
    };
    let ctx = AggWalkCtx {
        variables: &variables,
        target_iterated_dims: &target_iterated_dims,
        dm_dims: dm_dims.as_slice(),
        dim_ctx,
    };
    let from_canon = canonicalize(from).into_owned();

    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
            classify_expr_source_read(expr, &ctx, &from_canon)
        }
        // GH #792: a PER-ELEMENT-EQUATION owner (`share[nyc] = SUM(pop[nyc,*] *
        // w[*])`, `share[boston] = SUM(pop[boston,*] * w[*])`) has no single
        // dt-expression -- the I1-declined strict-slice reducer lives in the
        // SLOT bodies. The edge `pop -> share` never reaches the cartesian arm
        // (a per-element owner skips `try_cross_dimensional_link_scores`), so
        // before this it fell to the Bare per-shape stand-in: a single arrayed
        // `link_score:pop->share` simulating to ~-0.0 with no per-edge warning.
        // Classify each slot's read with the SAME single-expr classifier (one
        // verdict source, the epic's one-predicate invariant) and combine:
        // ANY slot that reads `from` STRICTLY (a `StrictSlice`) declines the
        // WHOLE edge, since the edge's one arrayed Bare stand-in conflates all
        // slots -- a strict slot proves it wrong for at least that slot. The
        // GH #744 full-extent escape stays SLOT-LOCAL (a slot whose reducer
        // reads `from` both full-extent and pinned, `SUM(pop[*] * pop[north])`,
        // classifies `FullExtent` like the scalar path), since one slot's
        // full read says nothing about a DIFFERENT slot's strict read. A
        // strict verdict dominates a `NotDescribable` slot: the strict slot is
        // provably wrong, so the loud skip is always sound, where the
        // dynamic-index slot's conservative cross-product is merely "documented
        // OK" -- declining the whole edge is the conservative choice the issue
        // prescribes. Longer-term, per-slot pinned slices are statically
        // describable (each slot pins its row), so a per-element hoist could
        // score this shape exactly (GH #792, out of scope here).
        Ast::Arrayed(_, subscript_map, default_expr, _) => {
            let mut combined = UnhoistedSourceRead::FullExtent;
            let mut keys: Vec<_> = subscript_map.keys().collect();
            keys.sort();
            let slot_exprs = keys
                .into_iter()
                .map(|k| &subscript_map[k])
                .chain(default_expr.iter());
            for expr in slot_exprs {
                match classify_expr_source_read(expr, &ctx, &from_canon) {
                    // A strict slot is provably wrong for the Bare stand-in;
                    // decline immediately with its representative slice.
                    strict @ UnhoistedSourceRead::StrictSlice(_) => return strict,
                    // Remember a not-describable slot but keep scanning -- a
                    // later strict slot still dominates.
                    UnhoistedSourceRead::NotDescribable => {
                        combined = UnhoistedSourceRead::NotDescribable;
                    }
                    // A full-extent (or `from`-free) slot leaves the verdict
                    // unchanged unless an earlier slot weakened it.
                    UnhoistedSourceRead::FullExtent => {}
                }
            }
            combined
        }
    }
}

/// Classify how a SINGLE owner expression `expr` reads its arrayed source
/// `from_canon` for the GH #791/#792 cartesian-/Bare-stand-in decline: collect
/// every read slice of `from` inside `expr`'s maximal reducers and reduce them
/// to one [`UnhoistedSourceRead`]. Shared by the scalar/A2A whole-RHS owner
/// (one call over the single dt-expression) and the per-element-equation owner
/// (one call per slot body, GH #792). Keeping the per-expr logic here -- rather
/// than inlining it twice -- is what makes the per-slot decision use the EXACT
/// same predicate as the whole-RHS one.
fn classify_expr_source_read(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    from_canon: &str,
) -> UnhoistedSourceRead {
    // Collect every read slice of `from` inside `expr`'s maximal reducers.
    let mut slices: Vec<Option<Vec<AxisRead>>> = Vec::new();
    collect_from_read_slices_in_reducers(expr, ctx, from_canon, false, &mut slices);
    if slices.is_empty() {
        // `from` is not a direct reducer source we can describe (e.g. a bare
        // dynamic index `pop[idx]` outside any reducer, or a nested-expression
        // index). Keep the conservative cartesian.
        return UnhoistedSourceRead::NotDescribable;
    }
    // An axis read covers its WHOLE extent iff it ranges over every element:
    // `Reduced{subset: None}` (the full reduce `*`) or `Iterated` (the axis
    // ranges over the target's dimension space). `Pinned` reads one element;
    // a subset-`Reduced` reads only the subdimension -- both are strict.
    let axis_is_full_extent = |ax: &AxisRead| {
        matches!(
            ax,
            AxisRead::Reduced { subset: None } | AxisRead::Iterated { .. }
        )
    };
    let mut first_strict: Option<Vec<AxisRead>> = None;
    let mut any_full_extent_read = false;
    for slice in slices {
        match slice {
            None => return UnhoistedSourceRead::NotDescribable,
            Some(axes) => {
                if axes.iter().all(axis_is_full_extent) {
                    // This read covers every row of `from` (e.g. the `pop[*]`
                    // in `SUM(pop[*] * pop[north])`): so the SAME variable's
                    // strict reads leave NO row unread.
                    any_full_extent_read = true;
                } else if first_strict.is_none() {
                    first_strict = Some(axes);
                }
            }
        }
    }
    // Decline ONLY when `from` is read STRICTLY and NEVER at full extent: then
    // some `from` rows are genuinely unread (the GH #791 silent-cartesian
    // family). When `from` ALSO has a full-extent read (the GH #744
    // `SUM(pop[*] * pop[north])` self-reference family), every row is read --
    // the per-row partial's multi-slice ambiguity is the deliberately
    // conservative delta-ratio fallback, NOT the unread-rows defect -- so keep
    // the cartesian derivation unchanged.
    match first_strict {
        Some(slice) if !any_full_extent_read => UnhoistedSourceRead::StrictSlice(slice),
        _ => UnhoistedSourceRead::FullExtent,
    }
}

/// Walk `expr` for maximal array-reducer Apps and, for each that references
/// `from_canon` as an arrayed source, push `from`'s [`compute_read_slice`] into
/// `out` (`None` for a not-statically-describable read). Only the OUTERMOST
/// reducer is consulted (`in_reducer` suppresses nested ones), since the inner
/// reducer's reads are already covered by the outer slice computation. This is
/// STRICTER than `walk_subexpr_for_aggs`' maximal-reducer rule: the real walk
/// descends a DECLINED outer reducer with `in_reducer` unchanged (so a nested
/// reducer can still be hoisted), while this one suppresses nested collection
/// under ANY hoistable-kind outer reducer. The divergence is unobservable at
/// the GH #791 gate: in every divergent case the nested reducer IS hoisted, so
/// `from`'s read inside it is agg-routed and the edge never reaches the
/// cartesian fallthrough this verdict guards.
fn collect_from_read_slices_in_reducers(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    from_canon: &str,
    in_reducer: bool,
    out: &mut Vec<Option<Vec<AxisRead>>>,
) {
    match expr {
        Expr2::App(builtin, _, _) if !in_reducer && reducer_is_hoistable(builtin) => {
            // A maximal reducer: collect every read of `from` among its args.
            let mut refs: Vec<(String, Vec<AxisRead>)> = Vec::new();
            let mut ok = true;
            builtin.for_each_expr_ref(|arg| {
                if ok {
                    collect_arrayed_source_slices(arg, ctx, &mut refs, &mut ok);
                }
            });
            let mut saw_from = false;
            if ok {
                for (var, slice) in refs {
                    if canonicalize(&var).as_ref() == from_canon {
                        saw_from = true;
                        out.push(Some(slice));
                    }
                }
            }
            if !saw_from {
                // Either a not-describable arg cleared `ok`, or `from` is read
                // through a shape `compute_read_slice` declines. Record the
                // not-describable verdict ONLY when `from` actually appears in
                // the reducer (otherwise this reducer is irrelevant to `from`).
                let mut names: Vec<String> = Vec::new();
                builtin.for_each_expr_ref(|arg| collect_var_refs(arg, &mut names));
                if names.iter().any(|n| canonicalize(n).as_ref() == from_canon) {
                    out.push(None);
                }
            }
            // Descend with `in_reducer = true` so nested reducers are not
            // re-collected, but index subexpressions are still traversed.
            builtin.for_each_expr_ref(|sub| {
                collect_from_read_slices_in_reducers(sub, ctx, from_canon, true, out)
            });
        }
        Expr2::App(builtin, _, _) => {
            builtin.for_each_expr_ref(|sub| {
                collect_from_read_slices_in_reducers(sub, ctx, from_canon, in_reducer, out)
            });
        }
        Expr2::Subscript(_, indices, _, _) => {
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => {
                        collect_from_read_slices_in_reducers(e, ctx, from_canon, in_reducer, out)
                    }
                    IndexExpr2::Range(l, r, _) => {
                        collect_from_read_slices_in_reducers(l, ctx, from_canon, in_reducer, out);
                        collect_from_read_slices_in_reducers(r, ctx, from_canon, in_reducer, out);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::Op1(_, operand, _, _) => {
            collect_from_read_slices_in_reducers(operand, ctx, from_canon, in_reducer, out)
        }
        Expr2::Op2(_, left, right, _, _) => {
            collect_from_read_slices_in_reducers(left, ctx, from_canon, in_reducer, out);
            collect_from_read_slices_in_reducers(right, ctx, from_canon, in_reducer, out);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            collect_from_read_slices_in_reducers(cond, ctx, from_canon, in_reducer, out);
            collect_from_read_slices_in_reducers(then_e, ctx, from_canon, in_reducer, out);
            collect_from_read_slices_in_reducers(else_e, ctx, from_canon, in_reducer, out);
        }
        Expr2::Const(..) | Expr2::Var(..) => {}
    }
}

/// Collect the canonical names of all model variables referenced (directly or
/// via subscript) in `expr`, including inside nested builtins and index
/// expressions.
fn collect_var_refs(expr: &Expr2, out: &mut Vec<String>) {
    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _, _) => out.push(ident.as_str().to_string()),
        Expr2::Subscript(ident, indices, _, _) => {
            out.push(ident.as_str().to_string());
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => collect_var_refs(e, out),
                    IndexExpr2::Range(l, r, _) => {
                        collect_var_refs(l, out);
                        collect_var_refs(r, out);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => builtin.for_each_expr_ref(|sub| collect_var_refs(sub, out)),
        Expr2::Op1(_, operand, _, _) => collect_var_refs(operand, out),
        Expr2::Op2(_, left, right, _, _) => {
            collect_var_refs(left, out);
            collect_var_refs(right, out);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            collect_var_refs(cond, out);
            collect_var_refs(then_e, out);
            collect_var_refs(else_e, out);
        }
    }
}

/// Map a canonical dimension name back to its datamodel casing, falling back
/// to the canonical form if no datamodel dimension matches.
fn canonical_dim_to_datamodel(canonical: &str, dm_dims: &[crate::datamodel::Dimension]) -> String {
    dm_dims
        .iter()
        .find(|dm| canonicalize(dm.name()).as_ref() == canonical)
        .map(|dm| dm.name().to_string())
        .unwrap_or_else(|| canonical.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{SimlinDb, sync_from_datamodel};
    use crate::test_common::TestProject;

    /// Test helper: the source-variable names of an agg (sorted + deduped
    /// by the [`AggNode::sources`] construction invariant).
    fn source_names(a: &AggNode) -> Vec<&str> {
        a.sources.iter().map(|s| s.var.as_str()).collect()
    }

    /// Build a `TestProject`, sync into salsa, and return the enumerated
    /// aggregate nodes for the "main" model.
    fn agg_nodes(project: &TestProject) -> AggNodesResult {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let source_model = sync.models["main"].source;
        let source_project = sync.project;
        enumerate_agg_nodes(&db, source_model, source_project).clone()
    }

    /// Build a `TestProject` and return the GH #791 cartesian-decline verdict
    /// for the `from -> to` edge.
    fn source_read(project: &TestProject, from: &str, to: &str) -> UnhoistedSourceRead {
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let link = LtmLinkId::new(&db, from.to_string(), to.to_string());
        unhoisted_reducer_source_read(&db, link, sync.models["main"].source, sync.project).clone()
    }

    /// GH #791: a multi-source reducer whose source read is a STRICT slice
    /// (`pop[nyc,*]`, with no full-extent read of `pop`) is the silent-cartesian
    /// family -- `StrictSlice` (the caller loud-skips it), carrying the actual
    /// slice so the diagnostic renders `pop[nyc,*]` rather than a canned
    /// example.
    #[test]
    fn unhoisted_source_read_strict_slice_for_pinned_only_read() {
        let project = TestProject::new("strict_slice")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("pop[Region,D2]", "1")
            .array_aux("w[D2]", "0.5")
            .array_aux("share[Region]", "SUM(pop[nyc,*] * w[*])");
        let UnhoistedSourceRead::StrictSlice(slice) = source_read(&project, "pop", "share") else {
            panic!("the pinned-only read must classify StrictSlice");
        };
        assert_eq!(render_read_slice_for_diagnostic(&slice), "nyc,*");
    }

    /// GH #791 boundary: the SAME variable read at full extent (`pop[*]`) AND
    /// pinned (`pop[north]`) -- the GH #744 self-reference family -- leaves NO
    /// row unread, so it is `FullExtent` (the caller keeps the conservative
    /// delta-ratio cartesian, unchanged).
    #[test]
    fn unhoisted_source_read_full_extent_when_full_read_present() {
        let project = TestProject::new("self_ref")
            .named_dimension("region", &["north", "south"])
            .array_aux("pop[region]", "1")
            .scalar_aux("tp", "SUM(pop[*] * pop[north])");
        assert!(matches!(
            source_read(&project, "pop", "tp"),
            UnhoistedSourceRead::FullExtent
        ));
    }

    /// GH #791 boundary: a pure full-extent multi-source read (`matrix[D1,*]`,
    /// `[Iterated, Reduced]`) is `FullExtent` -- the #779 bare-feeder fixture's
    /// `matrix -> growth` edge keeps its correct cartesian diagonal.
    #[test]
    fn unhoisted_source_read_full_extent_for_iterated_reduced() {
        let project = TestProject::new("iter_reduced")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["c", "d"])
            .array_aux("matrix[D1,D2]", "1")
            .array_aux("frac", "0.5")
            .array_aux("growth[D1]", "SUM(matrix[D1,*] * frac)");
        assert!(matches!(
            source_read(&project, "matrix", "growth"),
            UnhoistedSourceRead::FullExtent
        ));
    }

    /// GH #791 boundary: a dynamic-index reducer (`SUM(pop[idx,*])`, `idx`
    /// non-literal) is NOT statically describable -- `NotDescribable`, so the
    /// caller keeps the DOCUMENTED conservative cartesian cross-product.
    #[test]
    fn unhoisted_source_read_not_describable_for_dynamic_index() {
        let project = TestProject::new("dyn_index")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("pop[Region,D2]", "1")
            .scalar_aux("idx", "2")
            .array_aux("share[Region]", "SUM(pop[idx,*])");
        assert!(matches!(
            source_read(&project, "pop", "share"),
            UnhoistedSourceRead::NotDescribable
        ));
    }

    /// GH #792: a PER-ELEMENT-EQUATION (`Ast::Arrayed`) owner whose every slot
    /// holds a strict-slice multi-source reducer (each `share` slot is
    /// `SUM(pop[<region>,*] * w[*])`) classifies the `pop -> share` edge
    /// `StrictSlice` -- the per-slot verdict combines to a decline. The first
    /// strict slot in sorted-key order (`boston`) supplies the representative
    /// slice.
    #[test]
    fn unhoisted_source_read_strict_slice_for_per_element_owner() {
        let project = TestProject::new("per_element_strict")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("pop[Region,D2]", "1")
            .array_aux("w[D2]", "0.5")
            .array_with_ranges_direct(
                "share",
                vec!["Region".into()],
                vec![
                    ("nyc", "SUM(pop[nyc,*] * w[*])"),
                    ("boston", "SUM(pop[boston,*] * w[*])"),
                ],
                None,
            );
        let UnhoistedSourceRead::StrictSlice(slice) = source_read(&project, "pop", "share") else {
            panic!("a per-element owner with strict-slice slots must classify StrictSlice");
        };
        // Sorted-key walk visits `boston` before `nyc`.
        assert_eq!(render_read_slice_for_diagnostic(&slice), "boston,*");
    }

    /// GH #792 any-strict precedence: ONLY ONE slot reads `pop` (strictly); the
    /// other slot does not read `pop` at all (so its single-expr verdict is
    /// `NotDescribable`). The strict slot dominates -> the WHOLE edge is
    /// `StrictSlice` (decline). This pins the conservative any-strict rule.
    #[test]
    fn unhoisted_source_read_strict_slice_dominates_non_reader_slot() {
        let project = TestProject::new("per_element_some")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("pop[Region,D2]", "1")
            .array_aux("w[D2]", "0.5")
            .array_with_ranges_direct(
                "share",
                vec!["Region".into()],
                vec![("nyc", "SUM(pop[nyc,*] * w[*])"), ("boston", "0")],
                None,
            );
        let UnhoistedSourceRead::StrictSlice(slice) = source_read(&project, "pop", "share") else {
            panic!("a strict slot must dominate a non-reader slot");
        };
        assert_eq!(render_read_slice_for_diagnostic(&slice), "nyc,*");
    }

    /// GH #792 full-extent escape stays SLOT-LOCAL: a per-element owner whose
    /// every slot reads `pop` at FULL EXTENT (`SUM(pop[*,*])`, all-`Reduced`)
    /// classifies `FullExtent` -- no strict slot, so no decline (the edge keeps
    /// whatever scoring its agg-routing / per-shape path produces).
    #[test]
    fn unhoisted_source_read_full_extent_for_per_element_full_reads() {
        let project = TestProject::new("per_element_full")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux("pop[Region,D2]", "1")
            .array_with_ranges_direct(
                "share",
                vec!["Region".into()],
                vec![("nyc", "SUM(pop[*,*])"), ("boston", "SUM(pop[*,*])")],
                None,
            );
        assert!(matches!(
            source_read(&project, "pop", "share"),
            UnhoistedSourceRead::FullExtent
        ));
    }

    /// AC4.3: a variable whose entire dt-equation is exactly one reducer call
    /// (scalar) mints no synthetic agg -- the variable itself is the agg.
    #[test]
    fn whole_rhs_scalar_reducer_is_its_own_agg() {
        let project = TestProject::new("whole_rhs")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("population[Region]", "100")
            .scalar_aux("total_population", "SUM(population[*])");

        let result = agg_nodes(&project);

        // No `$⁚ltm⁚agg⁚{n}` minted.
        assert!(
            result.aggs.iter().all(|a| !a.is_synthetic),
            "whole-RHS scalar reducer must not mint a synthetic agg; got: {:?}",
            result.aggs
        );
        // The reducer maps to a variable-backed agg named `total_population`,
        // owned by `total_population`'s equation. (Variable-backed aggs are
        // resolved via `aggs_in_var`, not `agg_for_key` -- the latter is
        // synthetic-only, since two different scalars can each be `SUM(pop[*])`.)
        let agg = result
            .aggs_in_var("total_population")
            .find(|a| a.name == "total_population")
            .expect("expected a variable-backed agg owned by `total_population`");
        assert!(!agg.is_synthetic);
        assert_eq!(source_names(agg), vec!["population"]);
        assert!(agg.result_dims.is_empty());
        // `agg_for_key` resolves only synthetic aggs, so it must not find this one.
        assert!(result.agg_for_key("sum(population[*])").is_none());
    }

    /// AC4.3 (arrayed variant): `agg[D1] = SUM(matrix[D1,*])` is whole-RHS, so
    /// the variable is the agg; `result_dims` carries `D1` and `read_slice`
    /// records the `Iterated(D1)` / `Reduced` axis split (the `D1` axis is
    /// iterated over the A2A dimension space, the second axis is reduced).
    #[test]
    fn whole_rhs_arrayed_partial_reduce_is_its_own_agg() {
        let project = TestProject::new("whole_rhs_partial")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct("agg", vec!["D1".into()], "SUM(matrix[D1, *])", None);

        let result = agg_nodes(&project);

        assert!(
            result.aggs.iter().all(|a| !a.is_synthetic),
            "whole-RHS arrayed reducer must not mint a synthetic agg; got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("agg")
            .next()
            .expect("expected an agg owned by `agg`");
        assert_eq!(agg.name, "agg");
        assert!(!agg.is_synthetic);
        assert_eq!(source_names(agg), vec!["matrix"]);
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
        assert_eq!(
            agg.canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
    }

    /// AC4.3 (arrayed full-reduce broadcast): `share[Region] = SUM(pop[*])` is
    /// a whole-RHS reducer, so the variable is the agg -- but `SUM(pop[*])` is a
    /// *full* reduce (scalar result) merely broadcast to `[Region]`, so the
    /// agg's `result_dims` is `[]`, not `[Region]`. (Contrast with
    /// `agg[D1] = SUM(matrix[D1, *])`, a partial reduce that genuinely varies
    /// per `D1`.)
    #[test]
    fn whole_rhs_arrayed_full_reduce_broadcast_has_scalar_result_dims() {
        let project = TestProject::new("whole_rhs_broadcast")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .array_aux("share[Region]", "SUM(pop[*])");

        let result = agg_nodes(&project);

        assert!(
            result.aggs.iter().all(|a| !a.is_synthetic),
            "whole-RHS reducer must not mint a synthetic agg; got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("share")
            .next()
            .expect("expected an agg owned by `share`");
        assert_eq!(agg.name, "share");
        assert!(!agg.is_synthetic);
        assert_eq!(source_names(agg), vec!["pop"]);
        assert!(
            agg.result_dims.is_empty(),
            "a full reduce broadcast to an arrayed variable has scalar result dims, got: {:?}",
            agg.result_dims
        );
    }

    /// AC4.1 (the basic mint): `share[r] = pop[r] / SUM(pop[*])` mints one
    /// synthetic agg `$⁚ltm⁚agg⁚0` for the sub-expression `SUM(pop[*])`.
    #[test]
    fn subexpression_reducer_mints_one_synthetic_agg() {
        let project = TestProject::new("share_mint")
            .named_dimension("Region", &["NYC", "Boston", "LA"])
            .array_aux("pop[Region]", "100")
            .array_aux("share[Region]", "pop / SUM(pop[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "expected exactly one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(synthetic[0].equation_text, "sum(pop[*])");
        assert_eq!(source_names(synthetic[0]), vec!["pop"]);
        assert!(synthetic[0].result_dims.is_empty());
        assert!(
            result
                .aggs_in_var("share")
                .any(|a| a.name == "$\u{205A}ltm\u{205A}agg\u{205A}0")
        );
    }

    /// P2 regression: an inline reducer (`share[r] = pop[r] / SUM(pop[*])`,
    /// which must mint a *synthetic* agg) sharing canonical text with a
    /// *whole-RHS* reducer of the same shape (`denom = SUM(pop[*])`, which
    /// is *variable-backed*) must NOT reuse the variable-backed agg --
    /// regardless of declaration order. Dedup-by-key applies to synthetic
    /// aggs only; variable-backed aggs are never deduped (a whole-RHS
    /// reducer variable is its own distinct agg node). Before the fix, with
    /// `denom` visited first (canonical-sorted: `denom` < `share`), the
    /// inline use found `by_key["sum(pop[*])"]` already populated by `denom`
    /// and reused it, so `share` got no synthetic agg and its reducer fell
    /// back to the conservative direct path.
    #[test]
    fn inline_reducer_does_not_reuse_variable_backed_agg() {
        let project = TestProject::new("inline_vs_var_backed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // `denom` (canonical-sorted first) is a whole-RHS reducer ->
            // variable-backed agg named `denom`.
            .scalar_aux("denom", "SUM(pop[*])")
            // `share` (visited after `denom`) uses the same reducer text as
            // a sub-expression -> must mint its own synthetic agg.
            .array_aux("share[Region]", "pop / SUM(pop[*])");

        let result = agg_nodes(&project);

        // The variable-backed agg `denom` exists and is not synthetic.
        // (`agg_for_key` now resolves only synthetic aggs, so look up the
        // variable-backed one through `by_var` instead.)
        let denom_agg = result
            .aggs_in_var("denom")
            .find(|a| a.name == "denom")
            .expect("expected a variable-backed agg owned by `denom`");
        assert!(
            !denom_agg.is_synthetic,
            "`denom`'s agg must be variable-backed"
        );
        assert_eq!(denom_agg.equation_text, "sum(pop[*])");

        // `share` must own a *synthetic* agg with the same reducer text.
        let share_agg = result
            .aggs_in_var("share")
            .find(|a| a.is_synthetic)
            .expect("expected a synthetic agg owned by `share`");
        assert_eq!(share_agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(share_agg.equation_text, "sum(pop[*])");
        assert_eq!(source_names(share_agg), vec!["pop"]);
        // `agg_for_key` resolves the reducer text to the *synthetic* agg.
        assert_eq!(
            result.agg_for_key("sum(pop[*])").map(|a| a.name.as_str()),
            Some("$\u{205A}ltm\u{205A}agg\u{205A}0")
        );

        // There must be exactly one synthetic agg and exactly one
        // variable-backed agg -- two distinct nodes despite identical text.
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        let var_backed_aggs: Vec<&AggNode> =
            result.aggs.iter().filter(|a| !a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "expected one synthetic agg, got: {:?}",
            result.aggs
        );
        assert_eq!(
            var_backed_aggs.len(),
            1,
            "expected one variable-backed agg, got: {:?}",
            result.aggs
        );
    }

    /// P2 regression (reverse declaration order): the same model as
    /// `inline_reducer_does_not_reuse_variable_backed_agg` but built so that
    /// the inline-use variable would be visited first if order mattered.
    /// `enumerate_agg_nodes` visits variables in canonical-sorted order, so
    /// `denom` < `share` always; this test instead uses different names
    /// (`a_share` < `z_denom`) to confirm the synthetic agg is minted when
    /// the inline use is encountered *before* the whole-RHS reducer.
    #[test]
    fn inline_reducer_mints_synthetic_when_visited_before_variable_backed() {
        let project = TestProject::new("inline_first")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // `a_share` (canonical-sorted first) uses the reducer inline.
            .array_aux("a_share[Region]", "pop / SUM(pop[*])")
            // `z_denom` (visited after) is the whole-RHS reducer.
            .scalar_aux("z_denom", "SUM(pop[*])");

        let result = agg_nodes(&project);

        let share_agg = result
            .aggs_in_var("a_share")
            .find(|a| a.is_synthetic)
            .expect("expected a synthetic agg owned by `a_share`");
        assert_eq!(share_agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(share_agg.equation_text, "sum(pop[*])");

        let denom_agg = result
            .aggs_in_var("z_denom")
            .find(|a| a.name == "z_denom")
            .expect("expected a variable-backed agg owned by `z_denom`");
        assert!(!denom_agg.is_synthetic);

        assert_eq!(result.aggs.iter().filter(|a| a.is_synthetic).count(), 1);
        assert_eq!(result.aggs.iter().filter(|a| !a.is_synthetic).count(), 1);
    }

    /// Two whole-RHS reducers with *identical* canonical text are two
    /// distinct variable-backed agg nodes (one per variable) -- never
    /// deduped, because each variable genuinely is its own aggregate.
    #[test]
    fn two_whole_rhs_reducers_same_text_are_distinct_aggs() {
        let project = TestProject::new("two_var_backed")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("total_a", "SUM(pop[*])")
            .scalar_aux("total_b", "SUM(pop[*])");

        let result = agg_nodes(&project);

        let var_backed: Vec<&AggNode> = result.aggs.iter().filter(|a| !a.is_synthetic).collect();
        assert_eq!(
            var_backed.len(),
            2,
            "two whole-RHS reducers must be two distinct variable-backed aggs; got: {:?}",
            result.aggs
        );
        let names: std::collections::HashSet<&str> =
            var_backed.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains("total_a"), "missing total_a: {names:?}");
        assert!(names.contains("total_b"), "missing total_b: {names:?}");
        // No synthetic aggs (neither reducer is a sub-expression).
        assert_eq!(result.aggs.iter().filter(|a| a.is_synthetic).count(), 0);
    }

    /// Two *inline* uses of the same reducer text still dedupe to one
    /// synthetic agg (the synthetic dedup-by-key path is preserved).
    #[test]
    fn two_inline_uses_same_text_dedupe_to_one_synthetic() {
        let project = TestProject::new("two_inline")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .array_aux("share_a[Region]", "pop / SUM(pop[*])")
            .array_aux("share_b[Region]", "pop * 2 / SUM(pop[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "two inline uses of the same reducer must dedupe to one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        // Both variables reference the same deduped synthetic agg index.
        let a_idx = result.by_var.get("share_a").cloned().unwrap_or_default();
        let b_idx = result.by_var.get("share_b").cloned().unwrap_or_default();
        assert_eq!(a_idx, b_idx);
    }

    /// AC4.4 (nested reducers): `x = SUM(a[*]) / SUM(b[*])` mints two distinct
    /// synthetic agg nodes (`$⁚ltm⁚agg⁚0` for `SUM(a[*])`, `$⁚ltm⁚agg⁚1` for
    /// `SUM(b[*])`). The `/` is not a reducer; neither `SUM` is inside the
    /// other, so both are maximal.
    #[test]
    fn nested_reducers_mint_two_aggs() {
        let project = TestProject::new("nested")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("a[Region]", "10")
            .array_aux("b[Region]", "20")
            .scalar_aux("x", "SUM(a[*]) / SUM(b[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            2,
            "expected two synthetic aggs; got: {:?}",
            result.aggs
        );
        // First-encounter (left-to-right DFS) order: SUM(a[*]) then SUM(b[*]).
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(synthetic[0].equation_text, "sum(a[*])");
        assert_eq!(source_names(synthetic[0]), vec!["a"]);
        assert_eq!(synthetic[1].name, "$\u{205A}ltm\u{205A}agg\u{205A}1");
        assert_eq!(synthetic[1].equation_text, "sum(b[*])");
        assert_eq!(source_names(synthetic[1]), vec!["b"]);
    }

    /// AC4.4 (dedup): the same reducer subexpression appearing in two
    /// variables' equations (with whitespace/casing differences in the
    /// source text) maps to one synthetic agg node referenced by both.
    #[test]
    fn ast_identical_reducers_dedupe() {
        let project = TestProject::new("dedup")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            // Two different equations both contain SUM(pop[*]); the first is
            // spelled with extra spacing and uppercase.
            .array_aux("share_a[Region]", "pop / SUM( POP [ * ] )")
            .array_aux("share_b[Region]", "pop * 2 / sum(pop[*])");

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "AST-identical reducers must dedupe to one agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].equation_text, "sum(pop[*])");
        // Both variables reference the same agg index.
        let a_idx: Vec<usize> = result.by_var.get("share_a").cloned().unwrap_or_default();
        let b_idx: Vec<usize> = result.by_var.get("share_b").cloned().unwrap_or_default();
        assert_eq!(a_idx.len(), 1);
        assert_eq!(b_idx.len(), 1);
        assert_eq!(
            a_idx, b_idx,
            "both variables must point at the same deduped agg index"
        );
    }

    /// Per-element `Ast::Arrayed` target with a different reducer per element:
    /// `x[a] = SUM(p[*]); x[b] = MEAN(p[*])` mints two synthetic agg nodes,
    /// one per element's reducer.
    #[test]
    fn per_element_arrayed_target_mints_one_agg_per_element_reducer() {
        let project = TestProject::new("per_elem")
            .named_dimension("D", &["a", "b"])
            .array_aux("p[D]", "1")
            .array_with_ranges_direct(
                "x",
                vec!["D".into()],
                vec![("a", "SUM(p[*])"), ("b", "MEAN(p[*])")],
                None,
            );

        let result = agg_nodes(&project);

        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            2,
            "per-element reducers must mint one agg per element; got: {:?}",
            result.aggs
        );
        let texts: std::collections::HashSet<&str> =
            synthetic.iter().map(|a| a.equation_text.as_str()).collect();
        assert!(texts.contains("sum(p[*])"), "missing sum(p[*]): {texts:?}");
        assert!(
            texts.contains("mean(p[*])"),
            "missing mean(p[*]): {texts:?}"
        );
        // Both are owned by `x`.
        let x_idx = result.by_var.get("x").cloned().unwrap_or_default();
        assert_eq!(x_idx.len(), 2);
    }

    /// Determinism: the same model built twice (or with variables declared in
    /// a different order) yields identical agg names assigned to the same
    /// subexpressions.
    #[test]
    fn enumeration_is_deterministic_under_variable_reordering() {
        // Two synthetic aggs: SUM(a[*]) and SUM(b[*]). Whichever variable
        // happens to be visited first is irrelevant -- we always visit in
        // canonical-name sorted order, and within an equation left-to-right.
        let build = |order_a_first: bool| {
            let mut p = TestProject::new("determinism")
                .named_dimension("Region", &["NYC", "Boston"])
                .array_aux("a[Region]", "10")
                .array_aux("b[Region]", "20");
            // `q` references SUM(a[*]) and SUM(b[*]); `r` references the same
            // pair. We add them in different orders to confirm the result is
            // identical.
            if order_a_first {
                p = p
                    .scalar_aux("q", "SUM(a[*]) + SUM(b[*])")
                    .scalar_aux("r", "SUM(a[*]) * SUM(b[*])");
            } else {
                p = p
                    .scalar_aux("r", "SUM(a[*]) * SUM(b[*])")
                    .scalar_aux("q", "SUM(a[*]) + SUM(b[*])");
            }
            agg_nodes(&p)
        };

        let r1 = build(true);
        let r2 = build(false);
        assert_eq!(
            r1.aggs, r2.aggs,
            "enumeration must be deterministic regardless of declaration order"
        );
        assert_eq!(r1.synthetic_by_key, r2.synthetic_by_key);
        // Specifically: SUM(a[*]) -> agg 0, SUM(b[*]) -> agg 1 (a < b, and
        // within q's equation SUM(a[*]) precedes SUM(b[*])).
        assert_eq!(
            r1.agg_for_key("sum(a[*])").map(|a| a.name.clone()),
            Some("$\u{205A}ltm\u{205A}agg\u{205A}0".to_string())
        );
        assert_eq!(
            r1.agg_for_key("sum(b[*])").map(|a| a.name.clone()),
            Some("$\u{205A}ltm\u{205A}agg\u{205A}1".to_string())
        );
    }

    /// A model with no reducers produces an empty result.
    #[test]
    fn model_without_reducers_has_no_aggs() {
        let project = TestProject::new("no_reducers")
            .stock("population", "100", &["births"], &["deaths"], None)
            .flow("births", "population * 0.1", None)
            .flow("deaths", "population * 0.05", None)
            .scalar_const("rate", 0.1);

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "model without reducers must have no aggs; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
        assert!(result.by_var.is_empty());
    }

    /// A reducer over a *scalar* source is not hoisted (the parser would
    /// normally reject it anyway, but be defensive).
    #[test]
    fn reducer_over_scalar_source_is_not_hoisted() {
        // `SUM(s)` where `s` is scalar -- pathological, but must not mint an
        // agg. (We also keep a real arrayed reducer to confirm the
        // enumerator still finds the legitimate one.)
        let project = TestProject::new("scalar_reducer")
            .named_dimension("Region", &["NYC", "Boston"])
            .scalar_aux("s", "5")
            .array_aux("pop[Region]", "100")
            .scalar_aux("y", "SUM(s) + SUM(pop[*])");

        let result = agg_nodes(&project);
        // Only the arrayed reducer is recognized.
        assert!(
            result.agg_for_key("sum(pop[*])").is_some(),
            "the arrayed reducer must be recognized; got: {:?}",
            result.aggs
        );
        assert!(
            result.agg_for_key("sum(s)").is_none(),
            "a reducer over a scalar source must not be hoisted; got: {:?}",
            result.aggs
        );
    }

    /// SIZE is not hoisted -- its link score is always 0, matching
    /// `try_cross_dimensional_link_scores`'s `Some(vec![])` for SIZE.
    #[test]
    fn size_reducer_is_not_hoisted() {
        let project = TestProject::new("size_reducer")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("n", "SIZE(pop[*])");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "SIZE must not be hoisted as an agg; got: {:?}",
            result.aggs
        );
    }

    /// AC4.1: a reducer over an explicit *slice* used as a sub-expression
    /// (`x[r] = ... + SUM(pop[NYC, *])`) IS hoisted into a synthetic agg --
    /// the `read_slice` descriptor records which rows it reads
    /// (`[Pinned(nyc), Reduced]` over `pop`'s `[Region, Age]` axes), so the
    /// element-graph reroute and the per-element reducer link scores route
    /// only those rows. `result_dims` is `[]` here: there is no `Iterated`
    /// axis (the `Region` on the target `x` is broadcast; the read is a
    /// single row). The `pop[NYC, Adult]` `Direct` reference is separate --
    /// not part of the agg.
    #[test]
    fn slice_reducer_subexpression_is_hoisted() {
        let project = TestProject::new("slice_subexpr")
            .named_dimension("Region", &["NYC", "Boston"])
            .named_dimension("Age", &["Adult", "Child"])
            .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
            .array_aux_direct(
                "x",
                vec!["Region".into()],
                "pop[NYC, Adult] + SUM(pop[NYC, *])",
                None,
            );

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "a slice-reducer subexpression must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(synthetic[0].name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        // `expr2_to_string` puts a space after the comma in a multi-index
        // subscript -- assert the canonical text it actually produces.
        assert_eq!(synthetic[0].equation_text, "sum(pop[nyc, *])");
        assert_eq!(source_names(synthetic[0]), vec!["pop"]);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Pinned("nyc".to_string()),
                AxisRead::Reduced { subset: None }
            ]
        );
        assert!(
            synthetic[0].result_dims.is_empty(),
            "no Iterated axis -- result dims must be empty; got: {:?}",
            synthetic[0].result_dims
        );
        assert!(
            result
                .aggs_in_var("x")
                .any(|a| a.name == "$\u{205A}ltm\u{205A}agg\u{205A}0")
        );
    }

    /// AC4.2: a *partial*-reduce slice over an iterated dimension used as a
    /// sub-expression (`x[D1] = ... + SUM(matrix[D1, *])`, `matrix[D1, D2]`,
    /// `x` A2A over `D1`) mints an arrayed synthetic agg over `D1`:
    /// `read_slice = [Iterated(d1), Reduced]`, `result_dims = [D1]`. The
    /// element graph routes `matrix[d1, d2] → agg[d1]`.
    #[test]
    fn sliced_reducer_over_iterated_dim_mints_arrayed_agg() {
        let project = TestProject::new("iterated_slice_subexpr")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "x",
                vec!["D1".into()],
                "matrix[a, x] + SUM(matrix[D1, *])",
                None,
            );

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "an iterated-dim slice-reducer subexpression must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
        assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
        assert_eq!(source_names(synthetic[0]), vec!["matrix"]);
        // `expr2_to_string` canonicalizes the iterated dim name lowercase.
        assert_eq!(synthetic[0].equation_text, "sum(matrix[d1, *])");
    }

    /// #514: a *mixed* read slice -- `Iterated` + `Pinned` + `Reduced` axes
    /// on one source. `matrix3d[D1, Region, Age]`, `x` A2A over `D1`,
    /// `x[D1] = ... + SUM(matrix3d[D1, NYC, *])`: the first axis is iterated
    /// over the target's `D1`, the second is pinned to the literal `NYC`,
    /// the third (wildcard) is reduced ⇒ `read_slice = [Iterated(d1),
    /// Pinned(nyc), Reduced]`, `result_dims = [D1]` (only the iterated axis
    /// shapes the agg). Mints one arrayed synthetic agg over `D1`.
    #[test]
    fn mixed_pinned_iterated_reduced_slice_mints_arrayed_agg() {
        let project = TestProject::new("mixed_slice_subexpr")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("Region", &["NYC", "Boston"])
            .named_dimension("Age", &["Adult", "Child"])
            .array_aux_direct(
                "matrix3d",
                vec!["D1".into(), "Region".into(), "Age".into()],
                "1",
                None,
            )
            .array_aux_direct(
                "x",
                vec!["D1".into()],
                "matrix3d[a, NYC, Adult] + SUM(matrix3d[D1, NYC, *])",
                None,
            );

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "a mixed pinned/iterated/reduced slice must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Pinned("nyc".to_string()),
                AxisRead::Reduced { subset: None }
            ]
        );
        assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
        assert_eq!(source_names(synthetic[0]), vec!["matrix3d"]);
        assert_eq!(synthetic[0].equation_text, "sum(matrix3d[d1, nyc, *])");
    }

    /// #514: a multi-source reducer whose arrayed args agree on their read
    /// slice -- `total = 1 + SUM(a[*] + b[*])`, `a`, `b` both over `D`. The
    /// reducer's argument expression references two arrayed sources; each
    /// reads its whole extent (`[Reduced]`), the slices agree, so one
    /// synthetic agg is minted carrying that combined slice and *both* source
    /// variables.
    #[test]
    fn multi_source_reducer_agreeing_slices_mints_one_agg() {
        let project = TestProject::new("multi_source_reducer")
            .named_dimension("D", &["p", "q"])
            .array_aux_direct("a", vec!["D".into()], "1", None)
            .array_aux_direct("b", vec!["D".into()], "2", None)
            .scalar_aux("total", "1 + SUM(a[*] + b[*])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "a multi-source reducer with agreeing slices must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![AxisRead::Reduced { subset: None }]
        );
        assert!(synthetic[0].result_dims.is_empty());
        // `sources` lists every arrayed model variable in the argument
        // (sorted by name), each carrying the IDENTICAL canonical slice --
        // invariant I1's identical-co-source form (T2 of the
        // shape-expressiveness design: acceptance is identical-only, so
        // per-source slices cannot yet differ).
        assert_eq!(source_names(synthetic[0]), vec!["a", "b"]);
        for s in &synthetic[0].sources {
            assert_eq!(
                s.read_slice,
                vec![AxisRead::Reduced { subset: None }],
                "every arrayed co-source must carry the canonical slice; got {:?} for {}",
                s.read_slice,
                s.var
            );
        }
    }

    /// #514 (negative guard): a multi-source reducer whose arrayed args read
    /// *incompatible* slices is NOT hoisted -- `total = 1 + SUM(a[*] + b[*])`
    /// where `a` is over `D1` and `b` is over `D2` (disjoint dims, so
    /// `[Reduced]` for `a`'s one axis vs `[Reduced]` for `b`'s -- the slices
    /// have the same shape but the *sources* differ in dimensionality; more
    /// to the point, a 1-axis `a[*]` and a 2-axis `b[*, *]` disagree on
    /// length). Use clearly-different ranks to force the disagreement: `a`
    /// over `D1`, `b` over `D1 x D2`. `combined_read_slice` returns `None`,
    /// so no agg is minted for this reducer.
    #[test]
    fn multi_source_reducer_disagreeing_slices_is_not_hoisted() {
        let project = TestProject::new("multi_source_disagree")
            .named_dimension("D1", &["p", "q"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("a", vec!["D1".into()], "1", None)
            .array_aux_direct("b", vec!["D1".into(), "D2".into()], "2", None)
            .scalar_aux("total", "1 + SUM(a[*] + b[*, *])");

        let result = agg_nodes(&project);
        assert!(
            result
                .aggs
                .iter()
                .all(|ag| !ag.reads_var("a") && !ag.reads_var("b")),
            "a multi-source reducer whose args read incompatible slices must not be hoisted; \
             got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
    }

    /// GH #534: a sliced reducer whose iterated index lines up with the
    /// source's row axis via a *positional dimension mapping*
    /// (`matrix[Region, D2]`, `State` over `{s1, s2}` with a `State→Region`
    /// mapping, target A2A over `State` with `... + SUM(matrix[State, *])`)
    /// IS hoisted: the `Iterated` axis carries the (target, source) dim
    /// pair, `result_dims` is the TARGET's iterated dim (`State` -- the
    /// dimension the agg variable is arrayed over), and the emitters remap
    /// each source row to its positionally-corresponding slot.
    /// (`classify_iterated_dim_shape`'s own mapped branch -- a
    /// whole-equation-iterated subscript, not a sliced reducer argument --
    /// is a separate path and stays `Bare`; see
    /// `db::ltm_ir_tests::ir_mapped_iterated_dim_subscript_is_bare`.)
    #[test]
    fn mapped_iterated_dim_sliced_reducer_is_hoisted_with_pair() {
        let project = TestProject::new("mapped_iterated_slice")
            .named_dimension("Region", &["r1", "r2"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
            .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "out",
                vec!["State".into()],
                "matrix[r1, x] + SUM(matrix[State, *])",
                None,
            );

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "a positionally-mapped sliced reducer must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "state".to_string(),
                    source_dim: "region".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
        assert_eq!(
            synthetic[0].result_dims,
            vec!["State".to_string()],
            "the agg's result axis is the TARGET equation's iterated dim"
        );
        assert_eq!(source_names(synthetic[0]), vec!["matrix"]);
        assert_eq!(synthetic[0].equation_text, "sum(matrix[state, *])");
    }

    /// GH #534 (conservative gate, element-mapped): a sliced reducer over an
    /// EXPLICIT element-mapped pair stays un-hoisted -- the executed A2A
    /// lowering resolves mapped references positionally and ignores the
    /// element map (GH #756), so `mapped_element_correspondence` declines
    /// and the reference keeps its conservative shape.
    #[test]
    fn element_mapped_sliced_reducer_is_not_hoisted() {
        let project = TestProject::new("element_mapped_slice")
            .named_dimension("Region", &["r1", "r2"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension_with_element_mapping(
                "State",
                &["s1", "s2"],
                "Region",
                &[("s1", "r2"), ("s2", "r1")],
            )
            .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "out",
                vec!["State".into()],
                "1 + SUM(matrix[State, *])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("matrix")),
            "an element-mapped sliced reducer must not be hoisted; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
    }

    /// GH #757 (flipped from the GH #534-era conservative pin): a sliced
    /// reducer whose POSITIONAL mapping is declared only in the REVERSE
    /// direction (on the source's `Region` toward `State`) is now hoisted --
    /// `classify_axis_access`'s mapped arm gates on
    /// `iterated_axis_slot_elements` / `mapped_element_correspondence`,
    /// which accepts both declaration directions (the compiler's
    /// `translate_via_mapping` resolves both, so declining one direction
    /// was pure over-conservatism). The slice and `result_dims` are
    /// identical to the forward-declared twin.
    #[test]
    fn reverse_declared_mapped_sliced_reducer_is_hoisted() {
        let project = TestProject::new("reverse_mapped_slice")
            .named_dimension_with_mapping("Region", &["r1", "r2"], "State")
            .named_dimension("D2", &["x", "y"])
            .named_dimension("State", &["s1", "s2"])
            .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "out",
                vec!["State".into()],
                "1 + SUM(matrix[State, *])",
                None,
            );

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "the reverse-declared positionally-mapped sliced reducer must be hoisted; got: {:?}",
            result.aggs
        );
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "state".to_string(),
                    source_dim: "region".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
        assert_eq!(synthetic[0].result_dims, vec!["State".to_string()]);
    }

    /// GH #534: the whole-RHS twin -- `out[State] = SUM(matrix[State,*])`
    /// over a positionally-mapped pair mints a SYNTHETIC agg, an exception
    /// to the variable-is-the-agg rule for whole-RHS reducers: the
    /// variable-backed link-score path (`try_cross_dimensional_link_scores`)
    /// matches result axes against source axes by name, so a remapped pair
    /// falls off it onto the `Wildcard` per-shape partial, whose
    /// PREVIOUS-wrapping mangles the iterated index into a non-compiling
    /// `matrix[PREVIOUS(state),*]` (silently stubbed to 0). The synthetic
    /// agg gives the whole-RHS case the same remapped two-half scoring as
    /// an inline mapped reducer.
    #[test]
    fn whole_rhs_mapped_partial_reduce_mints_synthetic_agg() {
        let project = TestProject::new("whole_rhs_mapped")
            .named_dimension("Region", &["r1", "r2"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
            .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
            .array_aux_direct("out", vec!["State".into()], "SUM(matrix[State, *])", None);

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| a.is_synthetic),
            "a whole-RHS MAPPED reducer must mint a synthetic agg (not variable-backed); got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("out")
            .next()
            .expect("expected a synthetic agg owned by `out`");
        assert_eq!(agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(
            agg.canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "state".to_string(),
                    source_dim: "region".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
        assert_eq!(agg.result_dims, vec!["State".to_string()]);
    }

    /// GH #764 (T4): the whole-RHS BROADCAST twin -- `out[D1,D3] =
    /// SUM(matrix[D1,*])`, `result_dims` (`[D1]`) a strict subset of the
    /// owner's dims (`[D1,D3]`) -- mints a SYNTHETIC agg, generalizing the
    /// GH #534 carve-out: the variable-backed per-`(row, slot)` machinery
    /// requires each slot to name a complete `to` element, which a
    /// broadcast slot does not. The synthetic agg is arrayed over
    /// `result_dims` and rides the two-half emitters + the GH #528
    /// agg-to-target projection.
    #[test]
    fn whole_rhs_broadcast_partial_reduce_mints_synthetic_agg() {
        let project = TestProject::new("whole_rhs_broadcast_764")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension("D3", &["p", "q"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "out",
                vec!["D1".into(), "D3".into()],
                "SUM(matrix[D1, *])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| a.is_synthetic),
            "a whole-RHS BROADCAST reducer must mint a synthetic agg (not variable-backed); \
             got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("out")
            .next()
            .expect("expected a synthetic agg owned by `out`");
        assert_eq!(agg.name, "$\u{205A}ltm\u{205A}agg\u{205A}0");
        assert_eq!(
            agg.canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
        assert_eq!(source_names(agg), vec!["matrix"]);
    }

    /// GH #764 (T4): the whole-RHS PERMUTED twin -- `out[D2,D1] =
    /// SUM(cube[D1,D2,*])`, `result_dims` (`[D1,D2]`, slice order) in a
    /// different order than the owner's dims (`[D2,D1]`) -- mints a
    /// SYNTHETIC agg too: variable-backed slot coordinates are in
    /// `Iterated`-axis order, which would mis-subscript `to`. Slots of the
    /// synthetic agg are keyed by `result_dims` order, and the GH #528
    /// projection reorders per target element.
    #[test]
    fn whole_rhs_permuted_partial_reduce_mints_synthetic_agg() {
        let project = TestProject::new("whole_rhs_permuted_764")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension("D3", &["p", "q"])
            .array_aux_direct(
                "cube",
                vec!["D1".into(), "D2".into(), "D3".into()],
                "1",
                None,
            )
            .array_aux_direct(
                "out",
                vec!["D2".into(), "D1".into()],
                "SUM(cube[D1, D2, *])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| a.is_synthetic),
            "a whole-RHS PERMUTED reducer must mint a synthetic agg (not variable-backed); \
             got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("out")
            .next()
            .expect("expected a synthetic agg owned by `out`");
        assert_eq!(
            agg.result_dims,
            vec!["D1".to_string(), "D2".to_string()],
            "result_dims stay in Iterated-axis (slice) order, not the owner's declared order"
        );
        assert_eq!(
            agg.canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Iterated {
                    dim: "d2".to_string(),
                    source_dim: "d2".to_string()
                },
                AxisRead::Reduced { subset: None }
            ]
        );
    }

    /// GH #764 ∩ GH #765 (T4): a non-aligned whole-RHS reduce that ALSO
    /// carries a `Pinned` axis (`out[D1,D2] = SUM(cube[D1,nyc,*])` over
    /// `cube[D1,Region,D2]`) mints a synthetic agg whose slice keeps the
    /// `Pinned` axis -- so the synthetic-half emitters (which are
    /// Pinned-correct via `read_slice_rows`) score only the read rows.
    /// Pre-T4 this shape rode the OLD full-cartesian link-score
    /// derivation, scoring unread (`boston`) rows.
    #[test]
    fn whole_rhs_broadcast_pinned_mix_mints_synthetic_agg() {
        let project = TestProject::new("whole_rhs_broadcast_pinned_764")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct(
                "cube",
                vec!["D1".into(), "Region".into(), "D2".into()],
                "1",
                None,
            )
            .array_aux_direct(
                "out",
                vec!["D1".into(), "D2".into()],
                "SUM(cube[D1, nyc, *])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| a.is_synthetic),
            "a Pinned-bearing non-aligned whole-RHS reducer must mint a synthetic agg; got: {:?}",
            result.aggs
        );
        let agg = result
            .aggs_in_var("out")
            .next()
            .expect("expected a synthetic agg owned by `out`");
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
        assert_eq!(
            agg.canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Pinned("nyc".to_string()),
                AxisRead::Reduced { subset: None }
            ]
        );
    }

    /// GH #534: `iterated_axis_slot_elements` -- identity for the literal
    /// case, the positional preimage for a mapped pair, `None` for an
    /// element-mapped or unmapped pair.
    #[test]
    fn iterated_axis_slot_elements_cases() {
        use crate::datamodel::{Dimension as DmDimension, DimensionMapping};
        use crate::dimensions::DimensionsContext;

        let named = |name: &str, elems: &[&str], mappings: Vec<DimensionMapping>| {
            let mut d = DmDimension::named(
                name.to_string(),
                elems.iter().map(|e| e.to_string()).collect(),
            );
            d.mappings = mappings;
            d
        };
        let positional = DimensionMapping {
            target: "Region".to_string(),
            element_map: vec![],
        };
        let element_mapped = DimensionMapping {
            target: "Region".to_string(),
            element_map: vec![
                ("s1".to_string(), "r2".to_string()),
                ("s2".to_string(), "r1".to_string()),
            ],
        };

        let region_elems = vec!["r1".to_string(), "r2".to_string()];

        // Literal: identity (no dim_ctx lookups consulted).
        let ctx = DimensionsContext::from(&[
            named("Region", &["r1", "r2"], vec![]),
            named("State", &["s1", "s2"], vec![positional.clone()]),
        ]);
        assert_eq!(
            iterated_axis_slot_elements("region", "region", &region_elems, &ctx),
            Some(region_elems.clone())
        );

        // Positional mapping: source row r1 feeds slot s1, r2 feeds s2
        // (index-identity under the positional correspondence).
        assert_eq!(
            iterated_axis_slot_elements("state", "region", &region_elems, &ctx),
            Some(vec!["s1".to_string(), "s2".to_string()])
        );

        // Explicit element map: declined (GH #756 positional-only gate).
        let ctx_elem = DimensionsContext::from(&[
            named("Region", &["r1", "r2"], vec![]),
            named("State", &["s1", "s2"], vec![element_mapped]),
        ]);
        assert_eq!(
            iterated_axis_slot_elements("state", "region", &region_elems, &ctx_elem),
            None
        );

        // Unmapped pair: declined.
        let ctx_unmapped = DimensionsContext::from(&[
            named("Region", &["r1", "r2"], vec![]),
            named("State", &["s1", "s2"], vec![]),
        ]);
        assert_eq!(
            iterated_axis_slot_elements("state", "region", &region_elems, &ctx_unmapped),
            None
        );
    }

    /// AC4.4 (the carve-out): a reducer over a *dynamic* index
    /// (`x[Region] = SUM(pop[idx, *])`, `idx` a scalar aux -- a non-literal
    /// index) is NOT statically describable: `compute_read_slice` returns
    /// `None` for the `idx` axis, so the reducer is not hoisted and its
    /// reference stays on the conservative path. Pin this narrow case.
    #[test]
    fn dynamic_index_reducer_subexpression_is_not_hoisted() {
        let project = TestProject::new("dynamic_index_reducer")
            .named_dimension("Region", &["NYC", "Boston"])
            .named_dimension("Age", &["Adult", "Child"])
            .array_aux_direct("pop", vec!["Region".into(), "Age".into()], "10", None)
            .scalar_aux("idx", "1")
            .array_aux_direct("x", vec!["Region".into()], "SUM(pop[idx, *])", None);

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("pop")),
            "a dynamic-index reducer must not be hoisted; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
    }

    /// AC4.2 (positive guard): a whole-RHS slice/partial reduce
    /// (`agg[D1] = SUM(matrix[D1, *])`) IS recognized -- but as a
    /// variable-backed agg, not a synthetic one (covered by
    /// `whole_rhs_arrayed_partial_reduce_is_its_own_agg`); and an all-
    /// wildcard reducer subexpression (`SUM(matrix[*, *])`, no literal pin)
    /// is still hoisted as a synthetic agg with an all-`Reduced` slice.
    #[test]
    fn full_wildcard_reducer_subexpression_is_still_hoisted() {
        // `SUM(matrix[*, *])` (all-wildcard, no literal pin) is a full
        // reduce and IS hoistable as a synthetic agg.
        let project = TestProject::new("full_wildcard_subexpr")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .scalar_aux("y", "5 + SUM(matrix[*, *])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "an all-wildcard reducer subexpression must mint one synthetic agg; got: {:?}",
            result.aggs
        );
        assert_eq!(source_names(synthetic[0]), vec!["matrix"]);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Reduced { subset: None },
                AxisRead::Reduced { subset: None }
            ]
        );
        assert!(synthetic[0].result_dims.is_empty());
    }

    /// AC1.2: the consolidated `reducer_kind` table classifies every array
    /// reducer the LTM machinery cares about, and `reducer_is_hoistable`
    /// derives the right hoisted subset. `reducer_kind` is
    /// generic over the contained expression type, so `BuiltinFn::<i32>`
    /// literals suffice -- it only inspects structure and arity.
    #[test]
    fn reducer_kind_classifies_every_array_reducer() {
        use crate::builtins::BuiltinFn;

        let sum = BuiltinFn::Sum(Box::new(0i32));
        let mean_1 = BuiltinFn::Mean(vec![0i32]);
        let mean_2 = BuiltinFn::Mean(vec![0i32, 1i32]);
        let min_1 = BuiltinFn::Min(Box::new(0i32), None);
        let min_2 = BuiltinFn::Min(Box::new(0i32), Some(Box::new(1i32)));
        let max_1 = BuiltinFn::Max(Box::new(0i32), None);
        let max_2 = BuiltinFn::Max(Box::new(0i32), Some(Box::new(1i32)));
        let stddev = BuiltinFn::Stddev(Box::new(0i32));
        let rank = BuiltinFn::Rank(Box::new(0i32), Box::new(1i32));
        let size = BuiltinFn::Size(Box::new(0i32));
        let abs = BuiltinFn::Abs(Box::new(0i32));

        assert_eq!(reducer_kind(&sum), Some(ReducerKind::Linear));
        assert_eq!(reducer_kind(&mean_1), Some(ReducerKind::Linear));
        // Multi-argument MEAN is a scalar mean-of-arguments, not a reducer.
        assert_eq!(reducer_kind(&mean_2), None);
        assert_eq!(reducer_kind(&min_1), Some(ReducerKind::Nonlinear));
        // 2-arg MIN/MAX are scalar binary builtins, not reducers.
        assert_eq!(reducer_kind(&min_2), None);
        assert_eq!(reducer_kind(&max_1), Some(ReducerKind::Nonlinear));
        assert_eq!(reducer_kind(&max_2), None);
        assert_eq!(reducer_kind(&stddev), Some(ReducerKind::Nonlinear));
        assert_eq!(reducer_kind(&rank), Some(ReducerKind::Nonlinear));
        assert_eq!(reducer_kind(&size), Some(ReducerKind::Constant));
        assert_eq!(reducer_kind(&abs), None);

        // Hoistable: recognized AND not Constant AND scalar-valued (I5) --
        // SUM / 1-arg MEAN / 1-arg MIN / 1-arg MAX / STDDEV. SIZE is
        // recognized but never hoisted (its link score is always 0); RANK is
        // recognized but never hoisted (array-valued -- a scalar agg node
        // for it cannot compile, GH #771); 2-arg MIN/MAX are not recognized
        // at all.
        assert!(reducer_is_hoistable(&sum));
        assert!(reducer_is_hoistable(&mean_1));
        assert!(reducer_is_hoistable(&min_1));
        assert!(reducer_is_hoistable(&max_1));
        assert!(reducer_is_hoistable(&stddev));
        assert!(!reducer_is_hoistable(&rank));
        assert!(!reducer_is_hoistable(&size));
        assert!(!reducer_is_hoistable(&mean_2));
        assert!(!reducer_is_hoistable(&min_2));
        assert!(!reducer_is_hoistable(&max_2));
        assert!(!reducer_is_hoistable(&abs));

        // `reducer_kind_from_name` is the raw lowercase + arity decider that
        // `is_array_reducer_name` reads: SIZE included; mean/min/max only at
        // arity 1; sum/stddev/rank/size at any arity.
        assert_eq!(reducer_kind_from_name("sum", 1), Some(ReducerKind::Linear));
        assert_eq!(reducer_kind_from_name("mean", 1), Some(ReducerKind::Linear));
        assert_eq!(reducer_kind_from_name("mean", 2), None);
        assert_eq!(
            reducer_kind_from_name("min", 1),
            Some(ReducerKind::Nonlinear)
        );
        assert_eq!(reducer_kind_from_name("min", 2), None);
        assert_eq!(
            reducer_kind_from_name("stddev", 7),
            Some(ReducerKind::Nonlinear)
        );
        assert_eq!(
            reducer_kind_from_name("rank", 2),
            Some(ReducerKind::Nonlinear)
        );
        assert_eq!(
            reducer_kind_from_name("size", 1),
            Some(ReducerKind::Constant)
        );
        assert_eq!(reducer_kind_from_name("abs", 1), None);
    }

    /// GH #771: a RANK subexpression is NOT hoisted -- RANK is array-valued
    /// (Vensim VECTOR RANK), so a scalar agg node for it has an ill-typed
    /// equation. `reducer_is_hoistable` requires
    /// `reducer_collapses_to_scalar` (invariant I5), keeping the `pop`
    /// reference on the Direct path (`Bare` -> diagonal edges, scored by
    /// the GH #742 arrayed-capture machinery).
    #[test]
    fn rank_subexpression_is_not_hoisted() {
        let project = TestProject::new("rank_not_hoisted")
            .named_dimension("Region", &["north", "south"])
            .array_aux("pop[Region]", "100")
            .array_aux("scale[Region]", "pop[Region] * 0.01")
            .array_aux("grow[Region]", "scale[Region] * RANK(pop, 1)");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "RANK must not mint an aggregate node; got: {:?}",
            result.aggs
        );
    }

    /// GH #771 (whole-RHS form): `r[Region] = RANK(pop, 1)` is not a
    /// variable-backed aggregate node either -- the de-hoist applies to
    /// both agg kinds (the gate is `reducer_source_vars`'
    /// `reducer_is_hoistable`, shared by both walks).
    #[test]
    fn rank_whole_rhs_is_not_variable_backed() {
        let project = TestProject::new("rank_whole_rhs")
            .named_dimension("Region", &["north", "south"])
            .array_aux("pop[Region]", "100")
            .array_aux("r[Region]", "RANK(pop, 1)");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "whole-RHS RANK must not become a variable-backed agg; got: {:?}",
            result.aggs
        );
    }

    /// GH #766: a StarRange naming a dimension that is NEITHER the axis's
    /// own dimension NOR a proper subdimension of it (at best a mid-edit
    /// inconsistency) DECLINES the hoist -- it must not silently widen to
    /// the full extent. The reducer stays on the conservative path.
    #[test]
    fn star_range_non_subdimension_declines_hoist() {
        let project = TestProject::new("star_range_decline")
            .named_dimension("Region", &["a", "b", "c"])
            .named_dimension("Other", &["p", "q"])
            .array_aux("arr[Region]", "10")
            .scalar_aux("x", "1 + MEAN(arr[*:Other])");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.is_empty(),
            "a non-subdimension StarRange must decline the hoist; got: {:?}",
            result.aggs
        );
    }

    /// GH #766: a StarRange over the axis's OWN dimension (`SUM(arr[*:Region])`
    /// where `arr` is declared over `Region`) is the full extent --
    /// `Reduced{subset: None}`, byte-identical to a plain `*`.
    #[test]
    fn star_range_own_dimension_is_full_extent() {
        let project = TestProject::new("star_range_own_dim")
            .named_dimension("Region", &["a", "b", "c"])
            .array_aux("arr[Region]", "10")
            .scalar_aux("x", "1 + SUM(arr[*:Region])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![AxisRead::Reduced { subset: None }]
        );
    }

    /// GH #766: a StarRange over a PROPER subdimension carries the
    /// subdimension's elements as the `Reduced` subset (canonical names, in
    /// subdimension-declared order, resolved via `SubdimensionRelation`).
    #[test]
    fn star_range_proper_subdimension_carries_subset() {
        let project = TestProject::new("star_range_subset")
            .named_dimension("Region", &["a", "b", "c"])
            .named_dimension("Core", &["a", "b"])
            .array_aux("arr[Region]", "10")
            .scalar_aux("x", "1 + MEAN(arr[*:Core])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![AxisRead::Reduced {
                subset: Some(vec!["a".to_string(), "b".to_string()])
            }]
        );
        assert!(synthetic[0].result_dims.is_empty());
    }

    /// GH #766 (composition): a subset StarRange composes with an iterated
    /// axis -- `out[D1] = 1 + SUM(matrix[D1, *:SubD2])` hoists a synthetic
    /// agg whose slice is `[Iterated(d1), Reduced{subset}]` and whose
    /// `result_dims` carry `D1`.
    #[test]
    fn star_range_subset_composes_with_iterated_axis() {
        let project = TestProject::new("star_range_iterated_subset")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y", "z"])
            .named_dimension("SubD2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "out",
                vec!["D1".into()],
                "1 + SUM(matrix[D1, *:SubD2])",
                None,
            );

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced {
                    subset: Some(vec!["x".to_string(), "y".to_string()])
                }
            ]
        );
        assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
    }

    /// Test helper: resolve the named dimensions of a synced project into
    /// `Dimension` objects (for the gate's `to_dims` argument).
    fn resolve_dims(
        db: &SimlinDb,
        project: crate::db::SourceProject,
        names: &[&str],
    ) -> Vec<crate::dimensions::Dimension> {
        let dim_ctx = crate::db::project_dimensions_context(db, project);
        names
            .iter()
            .map(|n| {
                dim_ctx
                    .get(&crate::common::CanonicalDimensionName::from_raw(n))
                    .unwrap_or_else(|| panic!("dimension {n} resolves"))
                    .clone()
            })
            .collect()
    }

    /// GH #766 x T3: a VARIABLE-BACKED partial reduce whose slice carries a
    /// SUBSET (`out[D1] = SUM(matrix[D1,*:SubD2])` as the whole RHS) is
    /// ACCEPTED by the reduce gate: `try_cross_dimensional_link_scores`
    /// derives co-reduced rows from the same `read_slice_rows`, so the
    /// subset edges pair with subset divisors. (Pre-T3 the slice was
    /// excluded onto the loud conservative regime because the score
    /// derivation enumerated the full cartesian.)
    #[test]
    fn variable_backed_subset_slice_is_accepted_by_reduce_gate() {
        let project = TestProject::new("vb_subset_accepted")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y", "z"])
            .named_dimension("SubD2", &["x", "y"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct("out", vec!["D1".into()], "SUM(matrix[D1, *:SubD2])", None);

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

        // The variable-backed agg exists and carries the subset...
        let agg = result
            .aggs_in_var("out")
            .find(|a| a.name == "out")
            .expect("expected a variable-backed agg owned by `out`");
        assert!(matches!(
            &agg.canonical_read_slice()[1],
            AxisRead::Reduced { subset: Some(s) } if s == &["x".to_string(), "y".to_string()]
        ));

        // ...and the gate admits it (T3 of the shape-expressiveness design).
        let to_dims = resolve_dims(&db, sync.project, &["d1"]);
        let accepted = variable_backed_reduce_agg(result, "matrix", "out", &to_dims)
            .expect("the subset-bearing aligned slice must be admitted by the reduce gate");
        assert_eq!(accepted.name, "out");
    }

    /// GH #765 x T3: a VARIABLE-BACKED Pinned-mixed aligned slice
    /// (`outf[D1] = MEAN(cube[D1,x,*])`) is ACCEPTED by the reduce gate --
    /// the T1-era Pinned exclusion is deleted atomically with the
    /// `read_slice_rows` derivation swap.
    #[test]
    fn reduce_gate_accepts_pinned_mixed_aligned_slice() {
        let project = TestProject::new("gate_pinned_mixed")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension("D3", &["p", "q"])
            .array_aux_direct(
                "cube",
                vec!["D1".into(), "D2".into(), "D3".into()],
                "1",
                None,
            )
            .array_aux_direct("outf", vec!["D1".into()], "MEAN(cube[D1, x, *])", None);

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

        let to_dims = resolve_dims(&db, sync.project, &["d1"]);
        let accepted = variable_backed_reduce_agg(result, "cube", "outf", &to_dims)
            .expect("the Pinned-mixed aligned slice must be admitted by the reduce gate");
        assert_eq!(accepted.name, "outf");
    }

    /// Section 6 (scalar owner): a scalar-result Pinned slice
    /// (`total = SUM(pop[nyc,*])`, `to_dims` empty) is admitted -- the slot
    /// is the bare `total` node, so `emit_agg_routed_edges` emits exactly
    /// the read rows into `to`, matching the per-read-row scores.
    #[test]
    fn reduce_gate_accepts_scalar_owner_pinned_slice() {
        let project = TestProject::new("gate_scalar_owner_pinned")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux_direct("pop", vec!["Region".into(), "D2".into()], "1", None)
            .scalar_aux("total", "SUM(pop[nyc, *])");

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

        let accepted = variable_backed_reduce_agg(result, "pop", "total", &[])
            .expect("the scalar-owner Pinned slice must be admitted by the reduce gate");
        assert_eq!(accepted.name, "total");
    }

    /// Section 6 (inert skip): a PURE full-extent scalar reduce
    /// (`total = SUM(pop[*])`) stays OUT of the gate -- the reference
    /// walker's reduction edges are already the true reads, so routing it
    /// through the gate would change nothing and is skipped to keep the
    /// diff inert (byte-identity).
    #[test]
    fn reduce_gate_declines_pure_full_extent_slice() {
        let project = TestProject::new("gate_full_extent")
            .named_dimension("Region", &["nyc", "boston"])
            .array_aux("pop[Region]", "1")
            .scalar_aux("total", "SUM(pop[*])");

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

        assert!(
            variable_backed_reduce_agg(result, "pop", "total", &[]).is_none(),
            "a pure full-extent slice keeps the reference walker's edges (inert skip)"
        );
    }

    /// GH #777: an ARRAYED-owner scalar-result Pinned slice
    /// (`share[Region] = SUM(pop[nyc,*])` -- no `Iterated` axis, arrayed
    /// `to`) is ADMITTED: the single scalar reducer value broadcasts over
    /// the owner's dims, and the per-(read-row, full-target-element)
    /// machinery (`emit_agg_routed_edges`' broadcast fan-out +
    /// `try_cross_dimensional_link_scores`' broadcast-reduce branch) names
    /// every slot.
    #[test]
    fn reduce_gate_admits_arrayed_owner_scalar_result_slice() {
        let project = TestProject::new("gate_broadcast_pinned")
            .named_dimension("Region", &["nyc", "boston"])
            .named_dimension("D2", &["p", "q"])
            .array_aux_direct("pop", vec!["Region".into(), "D2".into()], "1", None)
            .array_aux_direct("share", vec!["Region".into()], "SUM(pop[nyc, *])", None);

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

        let to_dims = resolve_dims(&db, sync.project, &["region"]);
        let accepted = variable_backed_reduce_agg(result, "pop", "share", &to_dims)
            .expect("the arrayed-owner broadcast slice must be admitted (GH #777)");
        assert_eq!(accepted.name, "share");
        assert!(
            accepted.result_dims.is_empty(),
            "the broadcast reducer's result is scalar (no Iterated axis); got: {:?}",
            accepted.result_dims
        );
    }

    /// GH #764 boundary (T4): a partial reduce BROADCAST over extra target
    /// dims (`out[D1,D3] = SUM(matrix[D1,*])` -- `result_dims` a strict
    /// subset of `to`'s dims) never reaches the variable-backed gate at all
    /// anymore: T4's minting condition routes it to a SYNTHETIC agg, so
    /// `variable_backed_reduce_agg` finds no variable-backed candidate (its
    /// Iterated-arm alignment check stays as defense).
    #[test]
    fn reduce_gate_declines_broadcast_result_dims() {
        let project = TestProject::new("gate_broadcast_result")
            .named_dimension("D1", &["a", "b"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension("D3", &["p", "q"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct(
                "out",
                vec!["D1".into(), "D3".into()],
                "SUM(matrix[D1, *])",
                None,
            );

        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let result = enumerate_agg_nodes(&db, sync.models["main"].source, sync.project);

        let to_dims = resolve_dims(&db, sync.project, &["d1", "d3"]);
        assert!(
            variable_backed_reduce_agg(result, "matrix", "out", &to_dims).is_none(),
            "a broadcast whole-RHS reduce must have no variable-backed agg (GH #764)"
        );
        assert!(
            result.aggs.iter().all(|a| a.is_synthetic),
            "T4 mints a synthetic agg for the broadcast shape; got: {:?}",
            result.aggs
        );
    }

    /// GH #766 / invariant I3 (uniqueness of the full-extent form): a
    /// StarRange naming a SAME-CARDINALITY "subdimension" -- including a
    /// permuted alias of the axis's element set (`Alias = [c, a, b]` over
    /// `Region = [a, b, c]`: containment + equal size means the same element
    /// SET) -- normalizes to `Reduced{subset: None}`, never a `Some` subset
    /// covering the whole axis. Reduction order is irrelevant (the reduced
    /// rows are a set), and keeping the full-extent representation unique
    /// means downstream byte-identity does not depend on which spelling the
    /// modeler used.
    #[test]
    fn star_range_same_cardinality_alias_normalizes_to_full_extent() {
        let project = TestProject::new("star_range_alias")
            .named_dimension("Region", &["a", "b", "c"])
            .named_dimension("Alias", &["c", "a", "b"])
            .array_aux("arr[Region]", "10")
            .scalar_aux("x", "1 + SUM(arr[*:Alias])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![AxisRead::Reduced { subset: None }],
            "a whole-axis alias must normalize to the unique full-extent form"
        );
    }

    /// GH #766 (indexed dimensions): a StarRange over an INDEXED
    /// subdimension (`SubIndex(3)` with declared `parent = Index(5)`, which
    /// maps to the parent's first 3 elements) resolves the subset through
    /// the same `SubdimensionRelation` path as named dimensions -- the
    /// subset elements are the canonical indexed names `"1".."3"`, matching
    /// `dimension_element_names`'s output.
    #[test]
    fn star_range_indexed_subdimension_carries_subset() {
        let project = TestProject::new("star_range_indexed_subdim")
            .indexed_dimension("Index", 5)
            .indexed_subdimension("SubIndex", 3, "Index")
            .array_aux("arr[Index]", "10")
            .scalar_aux("x", "1 + MEAN(arr[*:SubIndex])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![AxisRead::Reduced {
                subset: Some(vec!["1".to_string(), "2".to_string(), "3".to_string()])
            }]
        );
    }

    // --- T2 (shape-expressiveness design): per-source `AggNode` invariant
    // pins -- the per-source REPRESENTATION invariants (I2, I3b, sorted
    // ordering) and the declines `accept_source_slices` enforces. I1's
    // *feeder clause* pins (deferred from T2 to avoid the GH #739 vacuity
    // trap) landed with T5's RED fixtures, in the section above.

    /// T2 / I3b ordering: `sources` is sorted by canonical variable name
    /// regardless of AST occurrence order -- `SUM(b[*] + a[*])` (with `b`
    /// first in the argument) still yields `[a, b]`, so salsa cache
    /// equality and downstream emission order never depend on how the
    /// modeler spelled the argument.
    #[test]
    fn multi_source_sources_are_sorted_by_var_name() {
        let project = TestProject::new("sorted_sources")
            .named_dimension("D", &["p", "q"])
            .array_aux_direct("a", vec!["D".into()], "1", None)
            .array_aux_direct("b", vec!["D".into()], "2", None)
            .scalar_aux("total", "1 + SUM(b[*] + a[*])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            source_names(synthetic[0]),
            vec!["a", "b"],
            "sources must be sorted by canonical name, not AST occurrence order"
        );
    }

    /// T2 / I3b dedup: the same variable referenced twice with the SAME
    /// slice (`SUM(a[*] + a[*])`) collapses to ONE `AggSource` -- the
    /// by-name downstream consumers (`aggs_in_var` routing, the
    /// half-emitters) key `sources` on the variable name, so a duplicate
    /// entry would make them ambiguous.
    #[test]
    fn duplicate_var_same_slice_collapses_to_one_source() {
        let project = TestProject::new("dup_var_same_slice")
            .named_dimension("D", &["p", "q"])
            .array_aux_direct("a", vec!["D".into()], "1", None)
            .scalar_aux("total", "1 + SUM(a[*] + a[*])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(
            source_names(synthetic[0]),
            vec!["a"],
            "a variable read twice with the same slice is one AggSource"
        );
        assert_eq!(
            synthetic[0].sources[0].read_slice,
            vec![AxisRead::Reduced { subset: None }]
        );
    }

    /// T2 / I3b decline: the same variable referenced with two DIFFERENT
    /// slices (`SUM(a[*] + a[p])` -- `[Reduced]` vs `[Pinned(p)]`) declines
    /// the hoist -- since T5, `accept_source_slices`' per-variable
    /// one-slice check (the I3b clause); the pin keeps I3b from regressing
    /// under the widened per-source acceptance.
    #[test]
    fn duplicate_var_with_conflicting_slices_declines_hoist() {
        let project = TestProject::new("dup_var_conflicting")
            .named_dimension("D", &["p", "q"])
            .array_aux_direct("a", vec!["D".into()], "1", None)
            .scalar_aux("total", "1 + SUM(a[*] + a[p])");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|ag| !ag.reads_var("a")),
            "one variable with two different slices must decline the hoist; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
    }

    /// T2 / I1 decline (GREEN characterization): two co-sources with
    /// DIFFERING `Reduced` subsets (`SUM(a[*:Sub1] + b[*:Sub2])`) decline
    /// the hoist -- their co-reduced rows per slot would disagree, so no
    /// canonical slice exists. Enforced by `accept_source_slices`'
    /// co-source-identity clause (subset is part of `AxisRead` equality).
    #[test]
    fn differing_reduced_subsets_decline_hoist() {
        let project = TestProject::new("differing_subsets")
            .named_dimension("Region", &["a", "b", "c"])
            .named_dimension("Sub1", &["a", "b"])
            .named_dimension("Sub2", &["b", "c"])
            .array_aux("p[Region]", "1")
            .array_aux("q[Region]", "2")
            .scalar_aux("total", "1 + SUM(p[*:Sub1] + q[*:Sub2])");

        let result = agg_nodes(&project);
        assert!(
            result
                .aggs
                .iter()
                .all(|ag| !ag.reads_var("p") && !ag.reads_var("q")),
            "co-sources with differing Reduced subsets must decline the hoist; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
    }

    /// T2 / I1 positive twin: two co-sources with the SAME `Reduced` subset
    /// (`SUM(p[*:Sub] + q[*:Sub])`) hoist one agg whose every source
    /// carries the identical subset-bearing canonical slice.
    #[test]
    fn agreeing_reduced_subsets_hoist_with_shared_subset() {
        let project = TestProject::new("agreeing_subsets")
            .named_dimension("Region", &["a", "b", "c"])
            .named_dimension("Sub", &["a", "b"])
            .array_aux("p[Region]", "1")
            .array_aux("q[Region]", "2")
            .scalar_aux("total", "1 + SUM(p[*:Sub] + q[*:Sub])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(source_names(synthetic[0]), vec!["p", "q"]);
        let expected = vec![AxisRead::Reduced {
            subset: Some(vec!["a".to_string(), "b".to_string()]),
        }];
        for s in &synthetic[0].sources {
            assert_eq!(s.read_slice, expected, "source {} slice", s.var);
        }
    }

    /// T2 / I2 + the scalar-feeder representation: a scalar feeder of a
    /// hoisted reducer (`scale` in `SUM(pop[*] * scale)`, GH #737) IS a
    /// source -- the routing filter and the element graph's scalar-feeder
    /// arm key on membership -- and carries an EMPTY read slice (one
    /// `AxisRead` per axis, and a scalar has none), while the arrayed
    /// co-source's slice has one entry per its declared axis.
    #[test]
    fn scalar_feeder_source_carries_empty_slice() {
        let project = TestProject::new("scalar_feeder")
            .named_dimension("Region", &["NYC", "Boston"])
            .array_aux("pop[Region]", "100")
            .scalar_aux("scale", "0.5")
            .scalar_aux("total", "1 + SUM(pop[*] * scale)");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(source_names(synthetic[0]), vec!["pop", "scale"]);
        // I2: one AxisRead per the source's OWN declared axes.
        assert_eq!(
            synthetic[0].source_read_slice("pop"),
            vec![AxisRead::Reduced { subset: None }],
            "the arrayed co-source's slice has one entry per its axis"
        );
        assert!(
            synthetic[0].source_read_slice("scale").is_empty(),
            "a scalar feeder has no axes, so its slice is empty"
        );
        // The canonical slice skips the feeder's empty slice.
        assert_eq!(
            synthetic[0].canonical_read_slice(),
            vec![AxisRead::Reduced { subset: None }]
        );
        // And the defensive non-source lookup is the empty slice too.
        assert!(synthetic[0].source_read_slice("absent").is_empty());
        assert!(synthetic[0].reads_var("scale"));
        assert!(!synthetic[0].reads_var("absent"));
    }

    // -- T5 / GH #767: the I1 FEEDER clause -------------------------------
    //
    // These pins were deliberately deferred from T2 (pinning them before the
    // acceptance widened would have been vacuous -- the GH #739 trap). They
    // land with T5's RED fixtures: an iterated-dim projection feeder is
    // accepted as an `AggSource` with ITS OWN slice, the canonical slice is
    // the co-source (Reduced-bearing) slice regardless of source sort order,
    // and everything outside the projection rule still declines.

    /// T5 / I1 feeder clause (GH #767): the iterated-dim-feeder reducer
    /// `1 + SUM(matrix[D1,*] * frac[D1])` (inline => synthetic) IS hoisted:
    /// `matrix` is the co-source carrying the canonical
    /// `[Iterated, Reduced]` slice, `frac` is a projection feeder carrying
    /// its OWN `[Iterated]` slice. `frac` sorts BEFORE `matrix`, so this
    /// also pins the `canonical_read_slice` contract fix: the canonical
    /// slice is the first slice WITH a `Reduced` axis, never an
    /// alphabetically-first feeder slice.
    #[test]
    fn iterated_dim_feeder_projection_hoists_with_per_source_slices() {
        let project = TestProject::new("feeder_projection")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
            .array_aux("frac[D1]", "0.5")
            .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * frac[D1])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(
            synthetic.len(),
            1,
            "the projection-feeder reducer must be hoisted (GH #767); got: {:?}",
            result.aggs
        );
        let agg = synthetic[0];
        assert_eq!(source_names(agg), vec!["frac", "matrix"]);
        assert_eq!(
            agg.source_read_slice("frac"),
            vec![AxisRead::Iterated {
                dim: "d1".to_string(),
                source_dim: "d1".to_string()
            }],
            "the feeder carries its OWN projection slice"
        );
        assert_eq!(
            agg.source_read_slice("matrix"),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced { subset: None }
            ],
            "the co-source carries the canonical slice"
        );
        // The contract fix: even though `frac` sorts first, the canonical
        // slice is the Reduced-bearing co-source slice.
        assert_eq!(agg.canonical_read_slice(), agg.source_read_slice("matrix"));
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
    }

    /// T5 / I1: the WHOLE-RHS form of the feeder shape
    /// (`growth[D1] = SUM(matrix[D1,*] * frac[D1])`, the GH #743/#767
    /// fixture) is VARIABLE-BACKED -- the canonical (co-source) slice is
    /// aligned with the owner's dims, so the variable IS the agg and no
    /// synthetic is minted.
    #[test]
    fn iterated_dim_feeder_whole_rhs_is_variable_backed() {
        let project = TestProject::new("feeder_whole_rhs")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
            .array_aux("frac[D1]", "0.5")
            .array_aux("growth[D1]", "SUM(matrix[D1, *] * frac[D1])");

        let result = agg_nodes(&project);
        assert!(result.synthetic_by_key.is_empty(), "got: {:?}", result.aggs);
        let vb: Vec<&AggNode> = result.aggs.iter().filter(|a| !a.is_synthetic).collect();
        assert_eq!(vb.len(), 1, "got: {:?}", result.aggs);
        assert_eq!(vb[0].name, "growth");
        assert_eq!(source_names(vb[0]), vec!["frac", "matrix"]);
        assert_eq!(vb[0].result_dims, vec!["D1".to_string()]);
        assert!(vb[0].source_is_projection_feeder("frac"));
        assert!(!vb[0].source_is_projection_feeder("matrix"));
    }

    /// T5 / I1: a feeder combined with a SCALAR feeder still hoists --
    /// `SUM(matrix[D1,*] * frac[D1] * scale)` has three sources, the scalar
    /// one with an empty slice (it is NOT a projection feeder: the
    /// changed-last machinery for scalar feeders is `generate_scalar_feeder_
    /// to_agg_equation`, not the per-row form).
    #[test]
    fn projection_feeder_and_scalar_feeder_combo_hoists() {
        let project = TestProject::new("feeder_combo")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
            .array_aux("frac[D1]", "0.5")
            .scalar_aux("scale", "2")
            .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * frac[D1] * scale)");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        let agg = synthetic[0];
        assert_eq!(source_names(agg), vec!["frac", "matrix", "scale"]);
        assert!(agg.source_read_slice("scale").is_empty());
        assert!(agg.source_is_projection_feeder("frac"));
        assert!(!agg.source_is_projection_feeder("scale"));
    }

    /// T5 / I1 (review MINOR-5): a PINNED-bearing CANONICAL slice is within
    /// the feeder clause's scope -- the clause keys only on the canonical
    /// slice's Iterated target dims, so `SUM(cube[D1, c1, *] * frac[D1])`
    /// hoists with canonical `[Iterated, Pinned(c1), Reduced]` and the
    /// feeder's own `[Iterated]` projection. (It is the FEEDER's slice that
    /// must be Iterated-only, not the canonical one.)
    #[test]
    fn pinned_bearing_canonical_with_feeder_hoists() {
        let project = TestProject::new("pinned_canonical_feeder")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .named_dimension("D3", &["k1", "k2"])
            .array_aux_direct(
                "cube",
                vec!["D1".into(), "D2".into(), "D3".into()],
                "5",
                None,
            )
            .array_aux("frac[D1]", "0.5")
            .array_aux("growth[D1]", "1 + SUM(cube[D1, c1, *] * frac[D1])");

        let result = agg_nodes(&project);
        let synthetic: Vec<&AggNode> = result.aggs.iter().filter(|a| a.is_synthetic).collect();
        assert_eq!(synthetic.len(), 1, "got: {:?}", result.aggs);
        let agg = synthetic[0];
        assert_eq!(
            agg.source_read_slice("cube"),
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Pinned("c1".to_string()),
                AxisRead::Reduced { subset: None }
            ]
        );
        assert!(agg.source_is_projection_feeder("frac"));
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
    }

    /// T5 / I1 decline: a no-`Reduced` source with a PINNED axis is NOT a
    /// projection feeder (the design's clause: a feeder slice consists ONLY
    /// of `Iterated` axes) -- the hoist declines.
    #[test]
    fn feeder_with_pinned_axis_declines_hoist() {
        let project = TestProject::new("feeder_pinned")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
            .array_aux_direct("w", vec!["D1".into(), "D2".into()], "0.5", None)
            .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * w[D1, c1])");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("matrix")),
            "a Pinned-axis no-Reduced source must decline the hoist; got: {:?}",
            result.aggs
        );
    }

    /// T5 / I1 decline: a feeder whose Iterated dims are a PROPER SUBSET of
    /// the canonical slice's Iterated dims declines -- its rows are not 1:1
    /// with the agg result slots (one feeder row would feed every slot it
    /// projects from, a broadcast the per-`(row, slot)` machinery cannot
    /// name). Documented residual: the design's I1 wording ("drawn from the
    /// canonical Iterated target-dim set") is implemented as ordered
    /// EQUALITY for exactly this reason.
    #[test]
    fn feeder_with_subset_iterated_dims_declines_hoist() {
        let project = TestProject::new("feeder_subset")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .named_dimension("D3", &["x", "y"])
            .array_aux_direct(
                "cube",
                vec!["D1".into(), "D2".into(), "D3".into()],
                "5",
                None,
            )
            .array_aux("w[D1]", "0.5")
            .array_aux_direct(
                "growth",
                vec!["D1".into(), "D2".into()],
                "1 + SUM(cube[D1, D2, *] * w[D1])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("cube")),
            "a proper-subset feeder must decline the hoist; got: {:?}",
            result.aggs
        );
    }

    /// T5 / I1 decline: a feeder whose Iterated dims are a PERMUTATION of
    /// the canonical order declines -- `read_slice_rows` derives slot
    /// coordinates in the source's axis order, so a permuted feeder's slots
    /// would mis-name the agg's `result_dims`-ordered slots.
    #[test]
    fn feeder_with_permuted_iterated_dims_declines_hoist() {
        let project = TestProject::new("feeder_permuted")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .named_dimension("D3", &["x", "y"])
            .array_aux_direct(
                "cube",
                vec!["D1".into(), "D2".into(), "D3".into()],
                "5",
                None,
            )
            .array_aux_direct("w", vec!["D2".into(), "D1".into()], "0.5", None)
            .array_aux_direct(
                "growth",
                vec!["D1".into(), "D2".into()],
                "1 + SUM(cube[D1, D2, *] * w[D2, D1])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("cube")),
            "a permuted feeder must decline the hoist; got: {:?}",
            result.aggs
        );
    }

    /// T5 / I1 decline: a MAPPED Iterated axis (GH #534) anywhere in the
    /// combination declines the feeder clause -- pinning the slot element
    /// into the equation text reads the TARGET-dim element, which is not
    /// the source row a mapped reference reads, so the changed-last feeder
    /// equation would mis-pin. The mapped sliced reducer WITHOUT a feeder
    /// stays hoisted (the GH #534 path is unchanged).
    #[test]
    fn mapped_iterated_axis_with_feeder_declines_hoist() {
        let project = TestProject::new("feeder_mapped")
            .named_dimension("Region", &["r1", "r2"])
            .named_dimension("D2", &["x", "y"])
            .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
            .array_aux_direct("matrix", vec!["Region".into(), "D2".into()], "1", None)
            .array_aux_direct("frac", vec!["State".into()], "0.5", None)
            .array_aux_direct(
                "out",
                vec!["State".into()],
                "1 + SUM(matrix[State, *] * frac[State])",
                None,
            );

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("matrix")),
            "a mapped iterated axis with a feeder must decline the hoist; got: {:?}",
            result.aggs
        );
    }

    /// T5 / I3b decline: the same variable appearing as both a co-source
    /// and a feeder-shaped reference (`SUM(matrix[D1,*] * matrix[D1,c1])`)
    /// declines -- one variable, two different slices.
    #[test]
    fn duplicate_var_as_co_source_and_feeder_declines_hoist() {
        let project = TestProject::new("dup_co_source_feeder")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux_direct("matrix", vec!["D1".into(), "D2".into()], "5", None)
            .array_aux("growth[D1]", "1 + SUM(matrix[D1, *] * matrix[D1, c1])");

        let result = agg_nodes(&project);
        assert!(
            result.aggs.iter().all(|a| !a.reads_var("matrix")),
            "one variable with co-source AND feeder slices must decline; got: {:?}",
            result.aggs
        );
    }

    /// T5 / I1 decline: two CO-SOURCES (both Reduced-bearing) with
    /// differing slices still decline, exactly as before the feeder clause
    /// -- the clause widens acceptance only for no-`Reduced` projections.
    #[test]
    fn co_sources_with_differing_slices_still_decline() {
        let project = TestProject::new("co_source_differ")
            .named_dimension("D1", &["r1", "r2"])
            .named_dimension("D2", &["c1", "c2"])
            .array_aux_direct("a", vec!["D1".into(), "D2".into()], "1", None)
            .array_aux_direct("b", vec!["D2".into(), "D1".into()], "2", None)
            .array_aux("growth[D1]", "1 + SUM(a[D1, *] + b[*, D1])");

        let result = agg_nodes(&project);
        assert!(
            result
                .aggs
                .iter()
                .all(|ag| !ag.reads_var("a") && !ag.reads_var("b")),
            "co-sources with differing slices must still decline; got: {:?}",
            result.aggs
        );
    }

    /// PR #784 review (P3), purely defensive: every arrayed reducer source
    /// has a `per_var` slice by construction (`collect_var_refs` and
    /// `collect_arrayed_source_slices` walk the identical reference
    /// surface), but if that invariant ever broke, [`agg_sources`] must
    /// DECLINE the hoist (`None` -- the reference stays on the conservative
    /// Direct path) rather than silently substituting the CANONICAL slice:
    /// for a projection feeder (whose slice differs from canonical by
    /// design, GH #767) that substitution would mislabel the feeder as a
    /// co-source and corrupt the per-`(row, slot)` link scores downstream.
    #[test]
    fn agg_sources_declines_when_arrayed_source_lacks_per_var_slice() {
        let project = TestProject::new("agg_sources_invariant")
            .named_dimension("D1", &["r1", "r2"])
            .array_aux("pop[D1]", "1")
            .scalar_aux("scale", "2")
            .scalar_aux("total", "1 + SUM(pop[*] * scale)");
        let datamodel = project.build_datamodel();
        let db = SimlinDb::default();
        let sync = sync_from_datamodel(&db, &datamodel);
        let model = sync.models["main"].source;
        let variables = crate::db::reconstruct_model_variables(&db, model, sync.project);
        let dm_dims = crate::db::project_datamodel_dims(&db, sync.project);
        let dim_ctx = crate::db::project_dimensions_context(&db, sync.project);
        let ctx = AggWalkCtx {
            variables: &variables,
            target_iterated_dims: &[],
            dm_dims: dm_dims.as_slice(),
            dim_ctx,
        };
        let canonical = vec![AxisRead::Reduced { subset: None }];

        // The invariant broken by hand: `pop` (arrayed) absent from `per_var`.
        let broken = CombinedReadSlices {
            canonical: canonical.clone(),
            per_var: HashMap::new(),
        };
        assert_eq!(
            agg_sources(vec!["pop".to_string()], &broken, &ctx),
            None,
            "a missing per-var slice for an arrayed source must decline the \
             hoist, never substitute the canonical slice"
        );

        // The intact invariant: each source carries its own slice; a scalar
        // source still gets the empty slice.
        let intact = CombinedReadSlices {
            canonical: canonical.clone(),
            per_var: HashMap::from([("pop".to_string(), canonical.clone())]),
        };
        let sources = agg_sources(vec!["scale".to_string(), "pop".to_string()], &intact, &ctx)
            .expect("an intact per-var map must build the sources");
        assert_eq!(
            sources,
            vec![
                AggSource {
                    var: "pop".to_string(),
                    read_slice: canonical,
                },
                AggSource {
                    var: "scale".to_string(),
                    read_slice: vec![],
                },
            ]
        );
    }
}
