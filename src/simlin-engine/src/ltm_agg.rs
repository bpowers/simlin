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
//! not `Hash` and so cannot key a map directly). Variable-backed aggs are never
//! deduped (see below).
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
//! mapped iterated axis with no DECLARED correspondence, or a cardinality
//! mismatch, is declined (a declared mapping is accepted in EITHER
//! declaration direction since GH #757, and since GH #997 an explicit
//! element map too -- this spelling folds to an ordinal, so the map is
//! honoured as a declaration but never read), a `MappedRead` axis (GH #997:
//! its executed rule admits a many-to-one correspondence the one-slot-per-row
//! remap cannot invert), a StarRange
//! naming a NON-subdimension (a mid-edit inconsistency that must not
//! silently widen to the full extent) is declined -- `compute_read_slice`
//! returns `None`, the reducer is not hoisted, and its reference stays on
//! the conservative path -- and so are co-sources with differing slices,
//! one variable read with two different slices (I3b), and a no-`Reduced`
//! source outside the projection rule (a Pinned-bearing, dim-subset,
//! permuted, or mapped mix; see [`accept_source_slices`]). `RANK` is
//! recognized as ARRAY-valued (GH #771) and is represented separately by an
//! arrayed synthetic agg whose output axes are the ranked argument's
//! non-pinned axes; each ranked source row feeds every rank-output slot in
//! its iterated context (GH #776).
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

use std::collections::{HashMap, HashSet};

use crate::ast::{Ast, BinaryOp, Expr0, Expr2, IndexExpr2};
use crate::builtins::BuiltinFn;
use crate::common::{Canonical, Ident, canonicalize};
use crate::db::{
    Db, LtmLinkId, SourceModel, SourceProject, model_lowered_variables, project_datamodel_dims,
    project_dimensions_context,
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
/// silently corrupts. The two consumers are
/// `ltm_augment::expr_is_array_slice_valued` (the GH #743
/// unfreezable-`PREVIOUS` detector) and scalar-reducer agg minting
/// ([`reducer_is_hoistable`], GH #771); the engine's own capture needs no
/// such gate, because a snapshot-only apply-to-all body is captured
/// structurally and its `RANK` lowers under the capture's own dimensions. LTM still routes RANK references
/// through synthetic aggs, but those aggs are marked array-valued and their
/// source→agg half uses the RANK-specific all-read-rows-to-all-output-slots
/// treatment rather than the scalar reducer row→slot treatment (GH #776).
///
/// # Not the same question as [`builtin_routes_through_agg`]
///
/// The two predicates answer questions that LOOK alike -- "is this reference
/// inside a reducer?" -- and their answers are inverted on exactly `SIZE` and
/// `RANK`. That inversion is the DEFINITION of the difference, not a
/// disagreement (GH #982): they read the one [`reducer_kind_from_name`] table
/// along two orthogonal axes.
///
/// * This predicate is about the reducer's RESULT TYPE: does the subtree
///   collapse to a scalar? `SIZE` does (it is a count), `RANK` does not (it is
///   array-valued). Its consumers -- the two freeze/capture gates named above,
///   plus the GH #779 bare-reducer-feeder decline in
///   `ltm_augment::references_bare_source_inside_reducer` -- are all deciding
///   whether an expression can live in a SCALAR slot.
/// * [`builtin_routes_through_agg`] is about LTM ROUTING: did
///   [`enumerate_agg_nodes`] mint an aggregate node for this call? `SIZE` did
///   not (`ReducerKind::Constant` is never hoisted -- its link score is
///   identically 0), `RANK` did (an array-valued agg, GH #776).
///
/// Both cells of the inversion are pinned in both directions by
/// `reducer_kind_classifies_every_array_reducer`, so an edit that moves either
/// predicate's membership is a test failure rather than a silent drift.
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

/// The reducer decision table as DATA, for the tests that pin it.
///
/// One row per `(name, arity)` pair needed to reach every arm of
/// [`reducer_kind_from_name`]: its six `Some` arms name seven functions
/// (`min | max` share an arm), two of those arms carry an `arity == 1` guard
/// so `mean`/`min`/`max` each need a failing-arity row as well, and the
/// catch-all needs one row -- 7 + 3 + 1 = 11 rows. Each row carries the kind
/// and all three derived predicates, so the `SIZE`/`RANK` inversion between
/// [`reducer_collapses_to_scalar`] and [`builtin_routes_through_agg`]
/// (GH #982) is pinned in BOTH directions rather than asserted from one side.
///
/// Shared with `ltm_augment::classifier_agreement_tests`, whose name-based
/// twin of `builtin_routes_through_agg` must agree row for row.
#[cfg(test)]
pub(crate) const REDUCER_DECISION_TABLE: &[ReducerDecisionRow] = &[
    ReducerDecisionRow::new("sum", 1, Some(ReducerKind::Linear), true, true, true),
    ReducerDecisionRow::new("mean", 1, Some(ReducerKind::Linear), true, true, true),
    ReducerDecisionRow::new("mean", 2, None, false, false, false),
    ReducerDecisionRow::new("min", 1, Some(ReducerKind::Nonlinear), true, true, true),
    ReducerDecisionRow::new("min", 2, None, false, false, false),
    ReducerDecisionRow::new("max", 1, Some(ReducerKind::Nonlinear), true, true, true),
    ReducerDecisionRow::new("max", 2, None, false, false, false),
    ReducerDecisionRow::new("stddev", 1, Some(ReducerKind::Nonlinear), true, true, true),
    // The two inverted cells: array-valued but agg-routed ...
    ReducerDecisionRow::new("rank", 1, Some(ReducerKind::Nonlinear), false, false, true),
    // ... and scalar-valued but never routed.
    ReducerDecisionRow::new("size", 1, Some(ReducerKind::Constant), true, false, false),
    ReducerDecisionRow::new("abs", 1, None, false, false, false),
];

/// One row of [`REDUCER_DECISION_TABLE`].
#[cfg(test)]
pub(crate) struct ReducerDecisionRow {
    pub name: &'static str,
    pub arity: usize,
    pub kind: Option<ReducerKind>,
    /// [`reducer_collapses_to_scalar`]: does the subtree evaluate to a scalar?
    pub collapses_to_scalar: bool,
    /// [`reducer_is_hoistable`]: does a scalar-reducer agg get minted?
    pub is_hoistable: bool,
    /// [`builtin_routes_through_agg`]: do references inside it route to an agg?
    pub routes_through_agg: bool,
}

#[cfg(test)]
impl ReducerDecisionRow {
    const fn new(
        name: &'static str,
        arity: usize,
        kind: Option<ReducerKind>,
        collapses_to_scalar: bool,
        is_hoistable: bool,
        routes_through_agg: bool,
    ) -> Self {
        ReducerDecisionRow {
            name,
            arity,
            kind,
            collapses_to_scalar,
            is_hoistable,
            routes_through_agg,
        }
    }

    /// The `BuiltinFn` this row's `(name, arity)` names, so the builtin-keyed
    /// predicates can be checked against the same row as the name-keyed ones.
    /// Panics on an unknown row, which keeps the table and this constructor in
    /// step.
    pub(crate) fn builtin(&self) -> BuiltinFn<i32> {
        match (self.name, self.arity) {
            ("sum", 1) => BuiltinFn::Sum(Box::new(0)),
            ("mean", n) => BuiltinFn::Mean((0..n as i32).collect()),
            ("min", 1) => BuiltinFn::Min(Box::new(0), None),
            ("min", 2) => BuiltinFn::Min(Box::new(0), Some(Box::new(1))),
            ("max", 1) => BuiltinFn::Max(Box::new(0), None),
            ("max", 2) => BuiltinFn::Max(Box::new(0), Some(Box::new(1))),
            ("stddev", 1) => BuiltinFn::Stddev(Box::new(0)),
            // `RANK(arr, dir)` reports arity 1: `builtin_reducer_arity` counts
            // only the reduced argument, and the deciders ignore arity here.
            ("rank", 1) => BuiltinFn::Rank(Box::new(0), Box::new(1)),
            ("size", 1) => BuiltinFn::Size(Box::new(0)),
            ("abs", 1) => BuiltinFn::Abs(Box::new(0)),
            other => unreachable!("no BuiltinFn for decision-table row {other:?}"),
        }
    }
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
/// always 0). `RANK` is recognized but not scalar-hoistable (GH #771): it is
/// ARRAY-valued, so it uses the separate array-valued agg path. This
/// predicate therefore gates only scalar-valued reducer aggs; the
/// reference-site IR uses [`builtin_routes_through_agg`] so RANK arguments
/// are still marked as aggregate-routed when an array-valued RANK agg was
/// minted (GH #776).
pub(crate) fn reducer_is_hoistable<E>(builtin: &BuiltinFn<E>) -> bool {
    let arity = builtin_reducer_arity(builtin);
    matches!(
        reducer_kind_from_name(builtin.name(), arity),
        Some(ReducerKind::Linear | ReducerKind::Nonlinear)
    ) && reducer_collapses_to_scalar(builtin.name(), arity)
}

/// `true` when references inside `builtin` may route through an aggregate
/// node -- the LTM ROUTING question, and the sole setter of
/// `db::ltm_ir::OccurrenceSite::in_reducer`.
///
/// It is the disjunction of [`agg_candidate_for_builtin`]'s two minting
/// branches, read from the branches themselves rather than restated:
/// [`reducer_is_hoistable`] is the scalar-reducer branch and
/// [`array_valued_rank_arg`] is the array-valued one. Stating it that way is
/// what keeps "can this call mint an agg" and "do references in it route to
/// one" from drifting apart.
///
/// See [`reducer_collapses_to_scalar`] for why the two "is this inside a
/// reducer?" predicates deliberately disagree on `SIZE` and `RANK` (GH #982).
pub(crate) fn builtin_routes_through_agg<E>(builtin: &BuiltinFn<E>) -> bool {
    reducer_is_hoistable(builtin) || array_valued_rank_arg(builtin).is_some()
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
/// - [`AxisRead::MappedRead`] -- the axis is likewise iterated over the
///   target's dimension space, but the subscript names a NON-ACTIVE dimension
///   (`x[Region]` under a `State`-iterating equation), which execution resolves
///   name-first and then through the declared element map rather than by
///   ordinal (GH #997). It is a DIRECT-reference verdict only:
///   [`compute_read_slice`] declines a slice containing one, so no aggregate
///   node ever carries it.
///
/// `PartialOrd`/`Ord`/`Hash` ride along because `RefShape::PerElement`
/// (GH #525, T6 of the shape-expressiveness design) embeds an
/// `AxisRead` vector and `RefShape` lives in `BTreeSet`s /
/// `HashSet`-keyed dedup maps downstream.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    /// This source axis is iterated over the target's dimension space, but the
    /// subscript spells a dimension the equation does NOT iterate -- typically
    /// the source's own (`ff_stop_growth_year_aggregated[Aggregated Regions]`
    /// inside a `COP`-iterating equation, C-LEARN's shape and GH #997's).
    ///
    /// Structurally the same pair as [`AxisRead::Iterated`]; the difference is
    /// which DESCRIBER a consumer must use, which is why it is a separate
    /// variant rather than a flag. EXECUTION resolves both the same way --
    /// every dimension-named subscript reaches `IndexOp::ActiveDimRef` and
    /// `compiler::subscript::build_view_from_ops` resolves it name-first, then
    /// through the declared map -- but the two describers have not converged:
    /// `positional_correspondence`, which the `Iterated` consumers read, still
    /// returns the diagonal, and its own rustdoc states the window where that
    /// differs from the executed read (a permuting element map at equal
    /// cardinality) and what closing it costs. Every consumer must pick the
    /// matching correspondence (`executed_read_correspondence` here,
    /// `positional_correspondence` for `Iterated`), which is what a
    /// separate variant makes a compile error rather than a silent
    /// mis-attribution.
    ///
    /// Reachable only from the DIRECT-reference classifier: `compute_read_slice`
    /// declines a reducer slice containing one, so the aggregate machinery's
    /// slot remaps -- which are the preimage of a BIJECTION -- never meet the
    /// many-to-one correspondence this variant admits.
    MappedRead {
        /// Canonical name of the TARGET equation's iterated dimension this axis
        /// is paired with, as `DimensionsContext::mapped_read_partner_dim`
        /// decides.
        dim: String,
        /// Canonical name of the SOURCE's declared dimension on this axis.
        source_dim: String,
    },
}

/// The agg result-slot coordinate (an element of the `Iterated` axis's
/// TARGET dimension `target_dim`) for each source element of
/// `source_axis_elems`, index-aligned.
///
/// - Literal case (`target_dim == source_dim`): the identity -- slot
///   coordinate == source element.
/// - Mapped case (GH #534): the PREIMAGE inversion of
///   [`crate::dimensions::DimensionsContext::positional_correspondence`]
///   `(target_dim, source_dim)` -- that helper is indexed by TARGET element
///   position and yields the source element read for it, so the slot for a
///   given source element is the target element whose correspondence entry
///   names it.
///
///   The POSITIONAL correspondence is the one this helper is written
///   against: it serves an [`AxisRead::Iterated`] axis, whose index spells a
///   dimension the equation ITERATES, and `positional_correspondence` is the
///   describer those consumers read (GH #997). That describer differs from
///   execution for a permuting element map -- the gap is stated on the function
///   itself -- but it is a bijection (index-identity, equal cardinality), so
///   every source element has exactly one preimage; the
///   inversion is still written generally and declines (returns `None`) if a
///   source element has zero or multiple preimages, mirroring
///   `expand_same_element`'s general-shape inversion. That generality is what
///   keeps the MANY-TO-ONE correspondence out: an `AxisRead::MappedRead` axis,
///   whose executed rule admits one, never reaches an agg read slice at all
///   (`compute_read_slice` declines it).
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
    let corr = dim_ctx.positional_correspondence(&t, &s)?;
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct AggNode {
    /// The aggregate node's name. For a synthetic agg this is
    /// `$⁚ltm⁚agg⁚{n}`; for a variable-backed agg this is the owning
    /// variable's canonical name (`total_population`, `row_sum`, ...).
    pub name: String,
    /// The reducer's identity: its canonical printed form, e.g.
    /// `"sum(pop[*])"` (`expr2_to_string`, which lowercases idents and
    /// normalizes whitespace, so textually-distinct-but-AST-identical
    /// subexpressions collapse to one node). It is the `Loc`-insensitive key
    /// the node was deduped on and the key every consumer that has to find
    /// this reducer INSIDE another tree compares against -- the reference-site
    /// IR's routing (`ReferenceSite::reducer_keys`), the ceteris-paribus
    /// wrap's live-reducer match and its agg-name substitution, and the
    /// agg-to-consumer polarity substitution -- because `Expr2` is `Eq` but
    /// not `Hash`, and a tree holds the same reducer at other `Loc`s.
    ///
    /// A key, never an equation: nothing parses it. The reducer's body is
    /// [`AggNode::reducer`], and [`AggNode::reducer_expr0`] is its typed
    /// projection for the equation generators.
    pub reducer_key: String,
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
    /// `true` for a synthetic helper representing array-valued `RANK`.
    ///
    /// A scalar reducer's read-slice rows feed one scalar result slot each
    /// (`SUM(matrix[D1,*])`: every `matrix[d1,*]` row feeds `agg[d1]`).
    /// `RANK` is different: every ranked source row can change every rank
    /// output element in the same iterated context, so the source→agg half
    /// fans each read row out across all non-pinned result-axis slots.
    pub array_valued_rank: bool,
    /// The reducer call itself: the very `BuiltinFn<Expr2>` this enumerator
    /// classified when it decided the hoist, of which `reducer_key` is the
    /// printed rendering.
    ///
    /// It is here so that no downstream consumer has to recover the reducer's
    /// kind, name, or body by parsing `reducer_key` (GH #983):
    /// [`crate::ltm_augment::classify_reducer_in_builtin`] reads the first two
    /// and hands back the third, `db::ltm::loops::source_to_agg_hop_polarity`
    /// analyses it directly, and the agg's own equation and the feeder link
    /// scores are generated from its [`AggNode::reducer_expr0`] projection.
    ///
    /// The polarity and classification readers filter to SYNTHETIC aggs
    /// (`recover_agg_hop_polarities` on `is_synthetic`; every
    /// `emit_source_to_agg_link_scores` call site on `is_synthetic_agg_name` or
    /// the IR's synthetic-only `routed_aggs`); the feeder generators reach the
    /// variable-backed arm's copy through `scalar_feeder_of_variable_backed_agg`
    /// and `try_cross_dimensional_link_scores`. One shape for both arms -- an
    /// `Option` here would add a branch to every reader to encode a fact about
    /// who happens to call them.
    ///
    /// # What the stored form does and does not normalize
    ///
    /// Stored in [`Expr2::strip_loc_and_bounds`] form, which removes two of the
    /// three ways this field could make the salsa-cached `AggNodesResult`
    /// compare unequal to an identical rebuild; the third is closed at the root
    /// instead.
    ///
    /// * `Loc` -- removed, and load-bearing. Two AST-identical occurrences in
    ///   different equations carry different `Loc`s, so storing them raw would
    ///   make *which occurrence won the dedup* observable and would stop
    ///   `enumerate_agg_nodes` backdating across an edit that only moves an
    ///   equation's byte offsets. Neither reader looks at a `Loc`
    ///   ([`crate::patch::expr2_to_expr0`] carries them along unread; the
    ///   polarity analyzer matches on none), so removing them changes no answer.
    ///   Pinned by `the_carried_reducer_is_normalized_so_offset_only_edits_backdate`.
    /// * `ArrayBounds` -- removed, and load-bearing: the ASTs this enumerator
    ///   walks are the per-variable lowering memos
    ///   (`db::model_lowered_variables`), lowered under their dependencies'
    ///   shapes, so an arrayed subexpression carries a bound holding the temp
    ///   id the lowering context handed out in equation order. Kept, that id
    ///   would make the cached value sensitive to unrelated edits elsewhere in
    ///   the owning equation. Neither reader looks at one --
    ///   `expr2_to_expr0` drops them outright. Pinned by the same test.
    /// * The float literal on `Expr2::Const` -- **not normalized here, and not
    ///   normalizable here:** dropping a `nan` literal would change what the
    ///   equation means. It is closed at the ROOT instead. The literal is an
    ///   [`crate::ast::Literal`], compared by BIT PATTERN, so a model whose
    ///   hoisted reducer contains a `nan` (`out = 1 + SUM(pop[*] * nan)`)
    ///   enumerates to a value equal to an identical rebuild and backdates like
    ///   any other. With a bare `f64` it could not (`NaN != NaN`), and every
    ///   revision bump re-executed `model_element_causal_edges` /
    ///   `model_ltm_reference_sites` / `model_ltm_variables` -- the GH #987/#981
    ///   class. Pinned by
    ///   `a_nan_literal_in_a_reducer_does_not_defeat_agg_backdating`.
    pub reducer: BuiltinFn<Expr2>,
}

impl AggNode {
    /// The reducer as the typed `Expr0` the equation generators consume -- the
    /// agg's own equation and the frozen re-evaluation of the feeder link
    /// scores. A projection of [`AggNode::reducer`] (`patch::builtin_to_untyped`,
    /// the same map `expr2_to_expr0` applies to every lowered subtree the
    /// generators wrap), so it prints as `reducer_key` and never goes through
    /// the lexer.
    pub(crate) fn reducer_expr0(&self) -> Expr0 {
        Expr0::App(
            crate::patch::builtin_to_untyped(&self.reducer),
            crate::ast::Loc::default(),
        )
    }

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
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Default)]
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
    let variables = model_lowered_variables(db, model, project);
    let dm_dims = project_datamodel_dims(db, project);
    // Dimension context for the GH #534 mapped-iterated-axis recognition
    // (`compute_read_slice`'s `has_mapping_to` direction gate +
    // `iterated_axis_slot_elements`' positional correspondence). Salsa-cached
    // off the project's dimensions input, so the enumeration is recomputed
    // when a dimension's mappings change.
    let dim_ctx = crate::db::project_dimensions_context(db, project);

    // Visit variables in canonical-sorted order for deterministic synthetic
    // naming. `model_lowered_variables` returns a HashMap, so the order
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
    variables: &'a crate::variable::LoweredVariableMap,
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
        && let Some(candidate) = agg_candidate_for_builtin(builtin, ctx)
        // Array-valued RANK helpers are always synthetic. The owning variable
        // is the final consumer of the rank array, not the rank helper itself;
        // keeping the normal agg→target half preserves the same projection
        // path as an inline rank subexpression.
        && !candidate.array_valued_rank
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
        && variable_backed_shape_is_expressible(&candidate.slices.canonical, ctx.target_iterated_dims)
        // `None` (a structurally-impossible missing per-var slice; see
        // `agg_sources`' rustdoc) falls through to `walk_subexpr_for_aggs`,
        // whose own `agg_sources` call declines identically -- the
        // reference stays on the conservative Direct path.
        && let Some(sources) = agg_sources(candidate.source_vars, &candidate.slices, ctx)
    {
        // Whole-RHS reducer: the variable IS the aggregate node. The agg
        // node's result shape is the *reducer's* result shape (the `Iterated`
        // axes' dims), not the owning variable's: a full reduce
        // (`share[Region] = SUM(pop[*])`) has `result_dims == []` even though
        // it is broadcast to an arrayed variable (every element holds the same
        // value); a partial reduce keyed by the active A2A dimension
        // (`rowsum[D1] = SUM(matrix[D1, *])`) keeps `[D1]` as its result dims.
        let key = crate::patch::expr2_to_string(expr);
        let result_dims = candidate.result_dims;
        let reducer = candidate.reducer;
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
                    array_valued_rank: false,
                    reducer,
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
            let candidate = (!in_reducer)
                .then(|| agg_candidate_for_builtin(builtin, ctx))
                .flatten();
            // DECLINE the degenerate square-source shape (repeated result
            // dim, GH #778/#785): the per-axis emission paths pin subscript
            // indices by dim name and disagree across the duplicated
            // occurrence. Declining here routes the reducer onto the same
            // `else` descent the not-statically-describable carve-outs take,
            // with `in_reducer` unchanged so the source references keep their
            // conservative Direct shape. See [`result_dims_has_repeated_dim`].
            let square_source = candidate
                .as_ref()
                .map(|c| c.result_dims.as_slice())
                .is_some_and(result_dims_has_repeated_dim);
            if !square_source
                && let Some(candidate) = candidate
                // `None` (a structurally-impossible missing per-var slice;
                // see `agg_sources`' rustdoc) declines the hoist: the `else`
                // arm descends with `in_reducer` unchanged, exactly like the
                // not-statically-describable carve-outs.
                && let Some(sources) = agg_sources(candidate.source_vars, &candidate.slices, ctx)
            {
                let key = crate::patch::expr2_to_string(expr);
                register_agg(
                    result,
                    next_synthetic_n,
                    &key,
                    owner_var,
                    AggKind::Synthetic {
                        result_dims: candidate.result_dims,
                        array_valued_rank: candidate.array_valued_rank,
                        reducer: candidate.reducer,
                    },
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

/// A reducer-like builtin that can be represented by an [`AggNode`].
struct AggCandidate {
    source_vars: Vec<String>,
    slices: CombinedReadSlices,
    result_dims: Vec<String>,
    array_valued_rank: bool,
    /// The classified reducer call, normalized for storage on the node
    /// (see [`AggNode::reducer`]).
    reducer: BuiltinFn<Expr2>,
}

fn agg_candidate_for_builtin(
    builtin: &BuiltinFn<Expr2>,
    ctx: &AggWalkCtx<'_>,
) -> Option<AggCandidate> {
    // Cloned only once the builtin has been accepted as a candidate, so the
    // non-reducer majority of App nodes pays nothing.
    let normalized = || {
        builtin
            .clone()
            .map(Expr2::strip_loc_and_bounds)
            .strip_own_locs()
    };
    if let Some(rank_arg) = array_valued_rank_arg(builtin) {
        let source_vars = rank_source_vars(rank_arg, ctx.variables)?;
        let slices = rank_combined_read_slice(rank_arg, ctx)?;
        let result_dims = rank_result_dims_from_read_slice(&slices, ctx, &source_vars)?;
        if result_dims.is_empty() {
            return None;
        }
        return Some(AggCandidate {
            source_vars,
            slices,
            result_dims,
            array_valued_rank: true,
            reducer: normalized(),
        });
    }

    let source_vars = reducer_source_vars(builtin, ctx.variables)?;
    let slices = combined_read_slice(builtin, ctx)?;
    let result_dims = result_dims_from_read_slice(&slices.canonical, ctx.dm_dims);
    Some(AggCandidate {
        source_vars,
        slices,
        result_dims,
        array_valued_rank: false,
        reducer: normalized(),
    })
}

/// The ranked argument of an ARRAY-VALUED reducer, or `None` for every other
/// builtin -- the array-valued half of [`agg_candidate_for_builtin`]'s
/// two-branch minting decision (the other half being
/// [`reducer_is_hoistable`]).
///
/// Generic over the contained expression type because it inspects only the
/// builtin's identity: [`builtin_routes_through_agg`] reads it so that "which
/// builtins can mint an array-valued agg" is stated once, here, rather than
/// restated as a second `matches!` beside the routing predicate.
fn array_valued_rank_arg<E>(builtin: &BuiltinFn<E>) -> Option<&E> {
    match builtin {
        BuiltinFn::Rank(arg, _) => Some(arg),
        _ => None,
    }
}

/// Model variables referenced by RANK's ranked argument.
///
/// The direction argument is intentionally excluded from the agg's sources:
/// it remains a direct dependency of the target expression. The RANK helper
/// represents how the ranked array values feed rank slots, and the source
/// half's scoring machinery assumes that relationship.
fn rank_source_vars(
    rank_arg: &Expr2,
    variables: &crate::variable::LoweredVariableMap,
) -> Option<Vec<String>> {
    let mut sources: Vec<String> = Vec::new();
    collect_var_refs(rank_arg, &mut sources);
    sources.retain(|name| variables.contains_key(&Ident::<Canonical>::new(name)));
    if sources.is_empty() {
        return None;
    }
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

fn rank_combined_read_slice(rank_arg: &Expr2, ctx: &AggWalkCtx<'_>) -> Option<CombinedReadSlices> {
    let mut refs: Vec<(String, Vec<AxisRead>)> = Vec::new();
    let mut ok = true;
    collect_arrayed_source_slices(rank_arg, ctx, &mut refs, &mut ok);
    if !ok || refs.is_empty() {
        return None;
    }
    accept_source_slices(refs)
}

/// RANK's output shape is the ranked argument's non-pinned axis shape.
///
/// Scalar reducers use only `Iterated` axes as result dims because the
/// `Reduced` axes collapse away. RANK is array-valued, so the full output
/// helper is dimensioned over both the surrounding iterated axes and the
/// ranked axes; only literal `Pinned` axes disappear.
fn rank_result_dims_from_read_slice(
    slices: &CombinedReadSlices,
    ctx: &AggWalkCtx<'_>,
    source_vars: &[String],
) -> Option<Vec<String>> {
    // `per_var` is a HashMap; use the sorted source list so mapped axes with
    // equivalent canonical slices always choose the same display dimension.
    let source_var = source_vars.iter().find(|var| {
        slices
            .per_var
            .get(var.as_str())
            .is_some_and(|slice| slice.as_slice() == slices.canonical.as_slice())
    })?;
    let source_dims = ctx
        .variables
        .get(&Ident::<Canonical>::new(source_var))
        .and_then(|v| v.get_dimensions())?;
    if source_dims.len() != slices.canonical.len() {
        return None;
    }
    let mut result_dims = Vec::new();
    for (axis, source_dim) in slices.canonical.iter().zip(source_dims) {
        match axis {
            AxisRead::Pinned(_) => {}
            AxisRead::Iterated { dim, .. } => {
                result_dims.push(canonical_dim_to_datamodel(dim, ctx.dm_dims));
            }
            AxisRead::Reduced { subset } => {
                result_dims.push(rank_reduced_axis_result_dim(
                    source_dim,
                    subset.as_deref(),
                    ctx,
                ));
            }
            // Unreachable: `compute_read_slice` declines a slice carrying a
            // `MappedRead` axis, so no agg node holds one (GH #997). Declining
            // rather than guessing keeps that a conservative fallback if the
            // hoisting gate ever widens.
            AxisRead::MappedRead { .. } => return None,
        }
    }
    Some(result_dims)
}

fn rank_reduced_axis_result_dim(
    source_dim: &crate::dimensions::Dimension,
    subset: Option<&[String]>,
    ctx: &AggWalkCtx<'_>,
) -> String {
    let Some(subset) = subset else {
        return canonical_dim_to_datamodel(source_dim.name(), ctx.dm_dims);
    };
    let source_elems = crate::ltm_augment::dimension_element_names(source_dim);
    let source_canon = source_dim.canonical_name();
    for dm_dim in ctx.dm_dims {
        let candidate = crate::common::CanonicalDimensionName::from_raw(dm_dim.name());
        let Some(rel) = ctx
            .dim_ctx
            .get_subdimension_relation(&candidate, source_canon)
        else {
            continue;
        };
        let candidate_subset: Option<Vec<String>> = rel
            .parent_offsets
            .iter()
            .map(|&o| source_elems.get(o).cloned())
            .collect();
        if candidate_subset.as_deref() == Some(subset) {
            return dm_dim.name().to_string();
        }
    }
    canonical_dim_to_datamodel(source_dim.name(), ctx.dm_dims)
}

/// What sort of aggregate node a reducer subexpression maps to.
enum AggKind {
    /// A `$⁚ltm⁚agg⁚{n}` auxiliary must be minted.
    Synthetic {
        result_dims: Vec<String>,
        array_valued_rank: bool,
        reducer: BuiltinFn<Expr2>,
    },
    /// The owning variable already is the aggregate node.
    VariableBacked {
        var_name: String,
        result_dims: Vec<String>,
        array_valued_rank: bool,
        reducer: BuiltinFn<Expr2>,
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
        AggKind::Synthetic {
            result_dims,
            array_valued_rank,
            reducer,
        } => {
            if let Some(&existing) = result.synthetic_by_key.get(key) {
                existing
            } else {
                let name = synthetic_agg_name(*next_synthetic_n);
                *next_synthetic_n += 1;
                result.aggs.push(AggNode {
                    name,
                    reducer_key: key.to_string(),
                    result_dims,
                    sources,
                    is_synthetic: true,
                    array_valued_rank,
                    reducer,
                });
                let idx = result.aggs.len() - 1;
                result.synthetic_by_key.insert(key.to_string(), idx);
                idx
            }
        }
        AggKind::VariableBacked {
            var_name,
            result_dims,
            array_valued_rank,
            reducer,
        } => {
            // Each whole-RHS-reducer variable is its own aggregate node;
            // never deduped, and not entered in `synthetic_by_key`.
            result.aggs.push(AggNode {
                name: var_name,
                reducer_key: key.to_string(),
                result_dims,
                sources,
                is_synthetic: false,
                array_valued_rank,
                reducer,
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
    variables: &crate::variable::LoweredVariableMap,
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
            let slice: Vec<AxisRead> = indices
                .iter()
                .zip(dims)
                .map(|(idx, axis_dim)| {
                    classify_axis_access(idx, axis_dim, ctx.target_iterated_dims, ctx.dim_ctx)
                })
                .collect::<Option<_>>()?;
            // A `MappedRead` axis (GH #997) is a DIRECT-reference verdict only.
            // Hoisting one would put a possibly MANY-TO-ONE correspondence into
            // machinery whose slot remap is the preimage of a bijection
            // (`iterated_axis_slot_elements`), so such a reducer keeps the
            // conservative un-hoisted path it had before #997 -- unchanged
            // behaviour, stated here rather than left to fall out of a missing
            // arm somewhere downstream.
            if slice
                .iter()
                .any(|ax| matches!(ax, AxisRead::MappedRead { .. }))
            {
                return None;
            }
            Some(slice)
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
///   `positional_correspondence` and therefore accepts a dimension MAPPING
///   declared in EITHER direction (GH #757 widened the former
///   `has_mapping_to(d, src)` forward-declared-only gate), an explicit element
///   map included, since this spelling is folded to an ordinal and never reads
///   the map (GH #997)
///   ⇒ [`AxisRead::Iterated`] carrying the `(d, src)` pair (GH #534). The
///   three `Iterated`-axis consumers (`emit_agg_routed_edges`,
///   `read_slice_rows` behind `emit_source_to_agg_link_scores`, and
///   `emit_agg_to_target_link_scores` via `result_dims`) remap each source
///   row to the slot of its positionally-corresponding target element
///   through the same helper. Declined (⇒ `None`, conservative): an
///   UNDECLARED pair (GH #527's rule -- the described diagonal follows a
///   correspondence the model declares, even though execution would read
///   positionally anyway), a position-mismatched pair, and a cardinality
///   mismatch. An explicit element map is NOT declined here; per the bullet
///   above, this spelling folds to an ordinal and never reads it.
///   (`classify_iterated_dim_shape` consumes this classifier directly
///   since T6, so the direct-reference path and the reducer path accept
///   the identical mapped set by construction.)
/// - `IndexExpr2::Expr(Expr2::Var(elem, ..))` or `Expr2::Const` resolving to
///   a literal element / 1-based index of the axis's dimension ⇒
///   [`AxisRead::Pinned`] carrying that element's canonical name.
/// - `IndexExpr2::Expr(Expr2::Var(d, ..))` where `d` names a dimension the
///   target does NOT iterate -- typically the SOURCE's own
///   (`x[Region]` under a `State`-iterating equation, GH #997) -- and
///   execution pairs it with exactly one of the target's iterated dims
///   through a declared mapping ⇒ [`AxisRead::MappedRead`] carrying the
///   `(partner, src)` pair. The pairing is
///   [`crate::dimensions::DimensionsContext::mapped_read_partner_dim`],
///   mirroring `compiler::subscript::normalize_subscripts3`; the usability
///   gate is `executed_read_correspondence`, the name-first-then-element-map
///   rule this spelling gets. Declined (⇒ `None`, conservative): no partner,
///   an AMBIGUOUS pairing (two viable iterated dims), and an unusable
///   per-element correspondence. Note this arm is checked LAST, after the
///   element and iterated-dimension readings, so it can never change what a
///   colliding name selects.
/// - anything else (`DimPosition`, `Range`, a non-literal `Expr`, a
///   `Var`/`Const` that resolves to none of the above) ⇒ `None`.
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
            // The element-vs-dimension-name precedence is the SHARED
            // `dimensions::resolve_axis_index_name` -- the compiler's own
            // element-first order (GH #986), which
            // `ltm_augment_post_transform::pin_dimension_name_indices` reads
            // too, so the two rules cannot disagree about which row a colliding
            // name selects.
            match crate::dimensions::resolve_axis_index_name(name_str, axis_dim, |n| {
                target_iterated_dims.iter().any(|t| t == n)
            }) {
                crate::dimensions::AxisIndexName::Element(elem) => Some(AxisRead::Pinned(elem)),
                // An iterated-dimension index: the axis is iterated over the
                // target's dimension space (and the agg result varies per
                // element of it) iff it lines up with the source's axis dim by
                // name or by a positional mapping (GH #534).
                crate::dimensions::AxisIndexName::IteratedDim => {
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
                        // `positional_correspondence`, which accepts BOTH
                        // declaration directions (GH #757 -- the former
                        // `has_mapping_to(d, src)` forward-only pre-gate was
                        // dropped) and, since GH #997, an explicit element map
                        // too: this index spells a dimension the equation
                        // ITERATES, which execution folds to an ordinal without
                        // reading the map. A plain position mismatch or an
                        // undeclared pair still declines, keeping the reference
                        // on the conservative path.
                        let elems = crate::ltm_augment::dimension_element_names(axis_dim);
                        iterated_axis_slot_elements(name_str, src_dim_name, &elems, dim_ctx).map(
                            |_| AxisRead::Iterated {
                                dim: name_str.to_string(),
                                source_dim: src_dim_name.to_string(),
                            },
                        )
                    }
                }
                // The index names neither an element of this axis nor a
                // dimension the equation iterates. It may still be a
                // NON-ACTIVE dimension execution pairs with one of the
                // target's iterated dims through a declared mapping
                // (`x[Region]` under a `State`-iterating equation, GH #997) --
                // the spelling `compiler::subscript::normalize_subscripts3`
                // turns into an `IndexOp::ActiveDimRef` and resolves
                // name-first, then through the element map. `MappedRead` is
                // that verdict, and it declines when the pairing is absent or
                // ambiguous, or when the per-element correspondence is not
                // usable -- in which case the reference keeps the conservative
                // shape it had before.
                crate::dimensions::AxisIndexName::Unresolved => {
                    let index_dim = crate::common::CanonicalDimensionName::from_raw(name_str);
                    let partner =
                        dim_ctx.mapped_read_partner_dim(&index_dim, target_iterated_dims)?;
                    dim_ctx.executed_read_correspondence(&partner, axis_dim.canonical_name())?;
                    Some(AxisRead::MappedRead {
                        dim: partner.as_str().to_string(),
                        source_dim: src_dim_name.to_string(),
                    })
                }
            }
        }
        IndexExpr2::Expr(Expr2::Const(..)) => {
            resolve_literal_axis_index(idx, axis_dim).map(AxisRead::Pinned)
        }
        IndexExpr2::Expr(_) => None,
    }
}

/// Resolve a single subscript index to a literal element name (canonical
/// lowercase) of `dim`, or `None` for any other shape. The per-axis sibling of
/// `db::ltm_ir::resolve_literal_index`, and it must stay in lockstep with it:
/// this one decides an `AxisRead::Pinned`, that one an `OccurrenceSite`'s
/// `RefShape`, and a disagreement desynchronizes `ClassifiedSite::shape` from
/// `OccurrenceSite::axes` for the same reference.
///
/// The same two-spelling split applies. An `Expr2::Var` is a bare element name
/// and resolves BY NAME against this axis. An `Expr2::Const` is a numeric
/// literal or a constified qualified `dimension·element` reference, and resolves
/// POSITIONALLY via [`crate::dimensions::resolve_axis_index_position`] --
/// see that function for why by-name would describe a row the simulation never
/// reads.
///
/// Unlike the `ltm_ir` sibling, the name half is already scoped to one axis
/// here, because every caller knows which axis it is classifying.
fn resolve_literal_axis_index(
    idx: &IndexExpr2,
    dim: &crate::dimensions::Dimension,
) -> Option<String> {
    let canonical = match idx {
        IndexExpr2::Expr(Expr2::Var(ident, _, _)) => ident.as_str().to_string(),
        IndexExpr2::Expr(Expr2::Const(_, value, _)) => {
            return crate::dimensions::resolve_axis_index_position(value.value(), dim);
        }
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
    if reducer_has_active_bare_arrayed_arg(builtin, ctx) {
        return None;
    }
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

fn reducer_has_active_bare_arrayed_arg(builtin: &BuiltinFn<Expr2>, ctx: &AggWalkCtx<'_>) -> bool {
    if ctx.target_iterated_dims.is_empty() {
        return false;
    }
    let mut found = false;
    builtin.for_each_expr_ref(|arg| {
        if !found && let Expr2::Var(ident, _, _) = arg {
            found = bare_arrayed_var_overlaps_target(ident, ctx);
        }
    });
    found
}

fn bare_arrayed_var_overlaps_target(ident: &Ident<Canonical>, ctx: &AggWalkCtx<'_>) -> bool {
    let Some(dims) = ctx.variables.get(ident).and_then(|v| v.get_dimensions()) else {
        return false;
    };
    if dims.is_empty() {
        return false;
    }
    dims.iter().any(|dim| {
        let source = dim.canonical_name();
        ctx.target_iterated_dims.iter().any(|target| {
            let target = crate::common::CanonicalDimensionName::from_raw(target);
            source == &target
                || ctx.dim_ctx.has_mapping_to(source, &target)
                || ctx.dim_ctx.has_mapping_to(&target, source)
                || ctx.dim_ctx.has_mapping_to_parent_of(source, &target)
                || ctx.dim_ctx.has_mapping_to_parent_of(&target, source)
        })
    })
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
                // Unreachable in an agg slice (see `compute_read_slice`); the
                // `Some(None)` declines the whole combination if it ever is.
                AxisRead::MappedRead { .. } => Some(None),
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
            // A `MappedRead` axis cannot reach an agg (`compute_read_slice`
            // declines it, GH #997); it contributes no result axis, as this
            // function's return type leaves no way to decline.
            AxisRead::Pinned(_) | AxisRead::Reduced { .. } | AxisRead::MappedRead { .. } => None,
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

/// Cartesian product of element-name lists, preserving each tuple as parts.
pub(crate) fn cartesian_element_parts(element_lists: &[Vec<String>]) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = vec![Vec::new()];
    for elems in element_lists {
        let mut next = Vec::with_capacity(out.len() * elems.len());
        for prefix in &out {
            for elem in elems {
                let mut tuple = prefix.clone();
                tuple.push(elem.clone());
                next.push(tuple);
            }
        }
        out = next;
    }
    out
}

/// The RANK output slots a source row can affect.
///
/// RANK is array-valued, so each non-pinned ranked axis can contribute every
/// output slot. The output slot elements must come from the helper's
/// [`AggNode::result_dims`] dimensions, not the current source's declared
/// dimensions: mapped sibling sources may carry different element names, and
/// proper StarRange inputs use the subdimension view. When the ranked view
/// also has reduced axes, `Iterated` axes are context axes and stay fixed to
/// the row's already-remapped `iterated_slot_parts`; with no reduced axes,
/// active-dimension spellings like `RANK(pop[D], 1)` rank across the iterated
/// axis itself and fan out over the whole result dimension.
pub(crate) fn rank_output_slot_parts_for_row(
    read_slice: &[AxisRead],
    result_dim_element_lists: &[Vec<String>],
    iterated_slot_parts: &[String],
) -> Option<Vec<Vec<String>>> {
    let has_reduced_axis = read_slice
        .iter()
        .any(|axis| matches!(axis, AxisRead::Reduced { .. }));
    let mut result_axis_elements = result_dim_element_lists.iter();
    let mut iterated_slots = iterated_slot_parts.iter();
    let mut per_output_axis: Vec<Vec<String>> = Vec::new();
    for axis in read_slice {
        match axis {
            AxisRead::Pinned(_) => {}
            AxisRead::Iterated { .. } => {
                let elems = result_axis_elements.next()?;
                if has_reduced_axis {
                    per_output_axis.push(vec![iterated_slots.next()?.clone()]);
                } else {
                    per_output_axis.push(elems.clone());
                }
            }
            AxisRead::Reduced { .. } => {
                per_output_axis.push(result_axis_elements.next()?.clone());
            }
            // Unreachable in an agg slice (see `compute_read_slice`).
            AxisRead::MappedRead { .. } => return None,
        }
    }
    if result_axis_elements.next().is_some() {
        return None;
    }
    if has_reduced_axis && iterated_slots.next().is_some() {
        return None;
    }
    Some(cartesian_element_parts(&per_output_axis))
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
#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// GH #792 unified per-element-owner rule: `to` is a PER-ELEMENT-EQUATION
    /// (`Ast::Arrayed`) owner and at least one slot body reads `from` inside a
    /// reducer. At both consultation sites the edge's routed-agg set is empty,
    /// so every such read is un-hoisted -- and a per-element owner has NO
    /// whole-edge derivation that can represent per-slot reducer reads: the
    /// cartesian arm needs a single dt-expression and the Bare per-shape
    /// stand-in conflates all slots (it simulated to a silent ~-0.0). The
    /// caller must DECLINE the edge loudly REGARDLESS of whether the reads are
    /// strict, full-extent, dim-named, or dynamic-index -- the strict/full
    /// distinction validates the cartesian projection, which does not exist
    /// for this owner shape. Carries the first statically-describable read
    /// slice (deterministic sorted-slot walk order) for the diagnostic, or
    /// `None` when every read is dim-named/dynamic.
    PerElementReducerRead(Option<Vec<AxisRead>>),
    /// Not statically describable (a dynamic index `pop[idx,*]`, a declined
    /// mapping, a `@N`/`Range`), OR `from` is not a direct subscript/var
    /// reducer source in `to`'s equation (e.g. a bare or literal-subscript
    /// reference outside any reducer -- the disjoint-dim FixedIndex family).
    /// The conservative cartesian cross-product is the DOCUMENTED behavior
    /// for the scalar/A2A dynamic-index family, so the caller keeps it (no
    /// decline).
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
            // Both spellings render as the dimension NAME the index carries,
            // which for a `MappedRead` is the source's own -- the index the
            // equation actually spells.
            AxisRead::Iterated { source_dim, .. } | AxisRead::MappedRead { source_dim, .. } => {
                source_dim.clone()
            }
            AxisRead::Reduced { subset: None } => "*".to_string(),
            AxisRead::Reduced {
                subset: Some(elems),
            } => format!("*:{{{}}}", elems.join(",")),
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// Return the first maximal reducer text in `to` whose bare arrayed argument
/// overlaps an Apply-To-All target dimension and participates in `from`'s
/// partial-equation term. That spelling is unsafe for LTM today: hoisting
/// would evaluate the reducer outside the target's active element context,
/// while ordinary feeder partials in the same non-additive term would freeze
/// that same wrong whole-array reducer value.
#[salsa::tracked(returns(ref))]
pub(crate) fn unhoisted_bare_arrayed_reducer_arg(
    db: &dyn Db,
    from: String,
    to: String,
    model: SourceModel,
    project: SourceProject,
) -> Option<String> {
    let from_canon = canonicalize(&from).into_owned();
    let variables = model_lowered_variables(db, model, project);
    let dm_dims = project_datamodel_dims(db, project);
    let dim_ctx = project_dimensions_context(db, project);
    let to_var = variables.get(&Ident::<Canonical>::new(&to))?;
    let ast = to_var.ast()?;
    let Ast::ApplyToAll(dims, expr) = ast else {
        return None;
    };
    let target_iterated_dims: Vec<String> = dims.iter().map(|d| d.name().to_string()).collect();
    let ctx = AggWalkCtx {
        variables: &variables,
        target_iterated_dims: &target_iterated_dims,
        dm_dims: dm_dims.as_slice(),
        dim_ctx,
    };
    first_active_bare_arrayed_reducer_affecting_source(expr, &ctx, &from_canon)
}

fn first_active_bare_arrayed_reducer_affecting_source(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    from_canon: &str,
) -> Option<String> {
    // Only + and - preserve an independent ceteris-paribus partial for each
    // branch. Products, divisions, functions, and conditionals couple the
    // changed source to any unsafe reducer in the same enclosing term.
    match expr {
        Expr2::Op2(BinaryOp::Add | BinaryOp::Sub, left, right, _, _) => {
            let left_found = if expr_references_source(left, from_canon) {
                first_active_bare_arrayed_reducer_affecting_source(left, ctx, from_canon)
            } else {
                None
            };
            left_found.or_else(|| {
                if expr_references_source(right, from_canon) {
                    first_active_bare_arrayed_reducer_affecting_source(right, ctx, from_canon)
                } else {
                    None
                }
            })
        }
        _ if expr_references_source(expr, from_canon) => {
            first_active_bare_arrayed_reducer(expr, ctx, false)
        }
        _ => None,
    }
}

fn expr_references_source(expr: &Expr2, from_canon: &str) -> bool {
    let mut names = Vec::new();
    collect_var_refs(expr, &mut names);
    names.iter().any(|n| canonicalize(n).as_ref() == from_canon)
}

fn first_active_bare_arrayed_reducer(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    in_reducer: bool,
) -> Option<String> {
    match expr {
        Expr2::App(builtin, _, _) if !in_reducer && reducer_is_hoistable(builtin) => {
            if reducer_has_active_bare_arrayed_arg(builtin, ctx) {
                return Some(crate::patch::expr2_to_string(expr));
            }
            let mut found = None;
            builtin.for_each_expr_ref(|sub| {
                if found.is_none() {
                    found = first_active_bare_arrayed_reducer(sub, ctx, true);
                }
            });
            found
        }
        Expr2::App(builtin, _, _) => {
            let mut found = None;
            builtin.for_each_expr_ref(|sub| {
                if found.is_none() {
                    found = first_active_bare_arrayed_reducer(sub, ctx, in_reducer);
                }
            });
            found
        }
        Expr2::Subscript(_, indices, _, _) => {
            for idx in indices {
                let found = match idx {
                    IndexExpr2::Expr(e) => first_active_bare_arrayed_reducer(e, ctx, in_reducer),
                    IndexExpr2::Range(l, r, _) => {
                        first_active_bare_arrayed_reducer(l, ctx, in_reducer)
                            .or_else(|| first_active_bare_arrayed_reducer(r, ctx, in_reducer))
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        Expr2::Op1(_, inner, _, _) => first_active_bare_arrayed_reducer(inner, ctx, in_reducer),
        Expr2::Op2(_, left, right, _, _) => {
            first_active_bare_arrayed_reducer(left, ctx, in_reducer)
                .or_else(|| first_active_bare_arrayed_reducer(right, ctx, in_reducer))
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            first_active_bare_arrayed_reducer(cond, ctx, in_reducer)
                .or_else(|| first_active_bare_arrayed_reducer(then_e, ctx, in_reducer))
                .or_else(|| first_active_bare_arrayed_reducer(else_e, ctx, in_reducer))
        }
        Expr2::Const(..) | Expr2::Var(..) => None,
    }
}

/// Classify how the NOT-hoisted reducer in `to`'s equation reads its arrayed
/// source `from`, for the GH #791 cartesian-derivation decline (whole-RHS
/// scalar/A2A owner) AND the GH #792 per-element-owner decline.
///
/// For a `Scalar`/`ApplyToAll` owner the verdict is over its single
/// dt-expression: the maximal reducer Apps that read `from` are classified by
/// the SAME per-axis classifier (`compute_read_slice` over
/// `classify_axis_access`) the hoisting path uses, with the SAME iterated-dim
/// context (`enumerate_agg_nodes`' Scalar/A2A arms), so for these owners the
/// decline predicate and the agg-minting predicate agree axis-for-axis about
/// whether a read is full-extent. `from` may appear in a reducer more than
/// once (a self-product) or in two different slices; the single-expr
/// classifier returns `StrictSlice` if ANY of `from`'s reads is a strict slice
/// and `NotDescribable` if any is not statically describable -- either way the
/// cartesian projection cannot soundly attribute that source.
///
/// For an `Ast::Arrayed` (per-element-equation) owner -- the GH #792 shape --
/// the rule is deliberately COARSER than the per-axis classification: ANY slot
/// reading `from` inside a reducer yields [`PerElementReducerRead`] (a
/// decline), regardless of strict/full-extent/dim-named/dynamic. The per-axis
/// strict-vs-full distinction exists to validate the CARTESIAN projection,
/// which requires a single dt-expression a per-element owner does not have;
/// the only derivation left for these edges is the Bare per-shape stand-in,
/// which conflates all slots and is wrong for every un-hoisted reducer read
/// (verified empirically: a full-extent-read fixture's stand-in simulated to
/// the same silent ~-0.0 as the strict one). See the `Ast::Arrayed` arm.
///
/// [`PerElementReducerRead`]: UnhoistedSourceRead::PerElementReducerRead
///
/// Salsa-tracked, keyed on the interned [`LtmLinkId`] (the
/// per-link `compile_ltm_var_fragment` idiom): the body's `model_lowered_variables`
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
    let variables = model_lowered_variables(db, model, project);
    let dm_dims = project_datamodel_dims(db, project);
    let dim_ctx = project_dimensions_context(db, project);
    let agg_nodes = enumerate_agg_nodes(db, model, project);

    let Some(to_var) = variables.get(&Ident::<Canonical>::new(to)) else {
        return UnhoistedSourceRead::NotDescribable;
    };
    let Some(ast) = to_var.ast() else {
        return UnhoistedSourceRead::NotDescribable;
    };
    // Mirror `enumerate_agg_nodes`' per-AST context exactly: the A2A dims are
    // the target's iterated dimensions; a scalar owner has none; and a
    // PER-ELEMENT owner's slots also have NONE in scope (`enumerate_agg_nodes`'
    // `Ast::Arrayed` arm walks slots with empty `target_iterated_dims` -- each
    // slot is an equation for a specific element, so a dim-named index like
    // `pop[Region,*]` is NOT an iterated read there; classifying it `Iterated`
    // here would call the read full-extent while execution pins the dim to the
    // slot's element, the GH #792 dim-name finding).
    let from_canon = canonicalize(from).into_owned();
    let hoisted_synthetic_keys: HashSet<String> = agg_nodes
        .by_var
        .get(to)
        .into_iter()
        .flat_map(|idxs| idxs.iter().map(|&i| &agg_nodes.aggs[i]))
        .filter(|agg| agg.is_synthetic && agg.reads_var(&from_canon))
        .map(|agg| agg.reducer_key.clone())
        .collect();

    match ast {
        Ast::Scalar(expr) => {
            let ctx = AggWalkCtx {
                variables: &variables,
                target_iterated_dims: &[],
                dm_dims: dm_dims.as_slice(),
                dim_ctx,
            };
            classify_expr_source_read(expr, &ctx, &from_canon, &hoisted_synthetic_keys)
        }
        Ast::ApplyToAll(dims, expr) => {
            let target_iterated_dims: Vec<String> =
                dims.iter().map(|d| d.name().to_string()).collect();
            let ctx = AggWalkCtx {
                variables: &variables,
                target_iterated_dims: &target_iterated_dims,
                dm_dims: dm_dims.as_slice(),
                dim_ctx,
            };
            classify_expr_source_read(expr, &ctx, &from_canon, &hoisted_synthetic_keys)
        }
        // GH #792: a PER-ELEMENT-EQUATION owner (`share[nyc] = SUM(pop[nyc,*] *
        // w[*])` per slot) has no single dt-expression -- un-hoisted reducer
        // reads live in the SLOT bodies, and the edge has NO whole-edge
        // derivation that can represent them: `classify_reducer` needs a single
        // expression (so the cartesian arm is unreachable except through an
        // EXCEPT default), and the Bare per-shape stand-in conflates all slots
        // (it simulated to a silent ~-0.0 for strict, dim-named, AND
        // full-extent slot reads alike -- the full-extent verdict only
        // validates the cartesian projection, which does not exist here). The
        // unified rule is therefore: if ANY slot (or the EXCEPT default) reads
        // `from` inside a maximal reducer -- describable or not -- the edge is
        // `PerElementReducerRead` and the caller declines it loudly. Only an
        // owner whose slots reference `from` exclusively OUTSIDE reducers
        // (bare refs, the disjoint-dim FixedIndex family) stays
        // `NotDescribable` and keeps its existing emission path. The first
        // statically-describable slice (sorted-slot walk order, deterministic)
        // rides along for the diagnostic. Longer-term, per-slot pinned slices
        // are statically describable (each slot pins its row), so a per-slot
        // hoist could score this shape exactly -- tracked as a follow-up
        // enhancement; see the PR's follow-up issue list.
        Ast::Arrayed(_, subscript_map, default_expr, _) => {
            let ctx = AggWalkCtx {
                variables: &variables,
                target_iterated_dims: &[],
                dm_dims: dm_dims.as_slice(),
                dim_ctx,
            };
            let mut keys: Vec<_> = subscript_map.keys().collect();
            keys.sort();
            let slot_exprs = keys
                .into_iter()
                .map(|k| &subscript_map[k])
                .chain(default_expr.iter());
            let mut any_reducer_read = false;
            let mut representative: Option<Vec<AxisRead>> = None;
            for expr in slot_exprs {
                let mut slices: Vec<Option<Vec<AxisRead>>> = Vec::new();
                collect_from_read_slices_in_reducers(
                    expr,
                    &ctx,
                    &from_canon,
                    &hoisted_synthetic_keys,
                    false,
                    &mut slices,
                );
                if !slices.is_empty() {
                    any_reducer_read = true;
                    if representative.is_none() {
                        representative = slices.into_iter().flatten().next();
                    }
                }
            }
            if any_reducer_read {
                UnhoistedSourceRead::PerElementReducerRead(representative)
            } else {
                UnhoistedSourceRead::NotDescribable
            }
        }
    }
}

/// Classify how a SINGLE Scalar/A2A owner expression `expr` reads its arrayed
/// source `from_canon` for the GH #791 cartesian decline: collect every read
/// slice of `from` inside `expr`'s maximal reducers that is not already
/// represented by a synthetic aggregate node, then reduce those residual reads
/// to one [`UnhoistedSourceRead`] (strict-and-never-full-extent =>
/// `StrictSlice`).
/// The per-element (`Ast::Arrayed`) owner deliberately does NOT use this
/// reduction -- its arm in [`unhoisted_reducer_source_read`] declines on ANY
/// reducer read of `from`, because the strict-vs-full distinction this
/// function draws only validates the cartesian projection, which requires a
/// single dt-expression (GH #792).
fn classify_expr_source_read(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    from_canon: &str,
    hoisted_synthetic_keys: &HashSet<String>,
) -> UnhoistedSourceRead {
    // Collect every read slice of `from` inside `expr`'s maximal reducers.
    let mut slices: Vec<Option<Vec<AxisRead>>> = Vec::new();
    collect_from_read_slices_in_reducers(
        expr,
        ctx,
        from_canon,
        hoisted_synthetic_keys,
        false,
        &mut slices,
    );
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

/// Walk `expr` for maximal array-reducer Apps and, for each residual reducer
/// that references `from_canon` as an arrayed source, push `from`'s
/// [`compute_read_slice`] into `out` (`None` for a not-statically-describable
/// read). Reducers whose canonical text appears in `hoisted_synthetic_keys`
/// are skipped: their contribution is already carried by the synthetic
/// `source -> agg -> target` halves, and GH #793 requires the strict-slice
/// verdict to consider only un-hoisted sibling reads. Only the OUTERMOST
/// reducer is consulted (`in_reducer` suppresses nested ones), since the inner
/// reducer's reads are already covered by the outer slice computation.
fn collect_from_read_slices_in_reducers(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    from_canon: &str,
    hoisted_synthetic_keys: &HashSet<String>,
    in_reducer: bool,
    out: &mut Vec<Option<Vec<AxisRead>>>,
) {
    match expr {
        Expr2::App(builtin, _, _) if !in_reducer && reducer_is_hoistable(builtin) => {
            let key = crate::patch::expr2_to_string(expr);
            let reducer_is_synthetic_hoisted = hoisted_synthetic_keys.contains(&key);
            // A maximal residual reducer: collect every read of `from` among
            // its args unless a synthetic agg already represents this reducer.
            if !reducer_is_synthetic_hoisted {
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
            }
            // Descend with `in_reducer = true` so nested reducers are not
            // re-collected, but index subexpressions are still traversed.
            builtin.for_each_expr_ref(|sub| {
                collect_from_read_slices_in_reducers(
                    sub,
                    ctx,
                    from_canon,
                    hoisted_synthetic_keys,
                    true,
                    out,
                )
            });
        }
        Expr2::App(builtin, _, _) => {
            builtin.for_each_expr_ref(|sub| {
                collect_from_read_slices_in_reducers(
                    sub,
                    ctx,
                    from_canon,
                    hoisted_synthetic_keys,
                    in_reducer,
                    out,
                )
            });
        }
        Expr2::Subscript(_, indices, _, _) => {
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => collect_from_read_slices_in_reducers(
                        e,
                        ctx,
                        from_canon,
                        hoisted_synthetic_keys,
                        in_reducer,
                        out,
                    ),
                    IndexExpr2::Range(l, r, _) => {
                        collect_from_read_slices_in_reducers(
                            l,
                            ctx,
                            from_canon,
                            hoisted_synthetic_keys,
                            in_reducer,
                            out,
                        );
                        collect_from_read_slices_in_reducers(
                            r,
                            ctx,
                            from_canon,
                            hoisted_synthetic_keys,
                            in_reducer,
                            out,
                        );
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::Op1(_, operand, _, _) => collect_from_read_slices_in_reducers(
            operand,
            ctx,
            from_canon,
            hoisted_synthetic_keys,
            in_reducer,
            out,
        ),
        Expr2::Op2(_, left, right, _, _) => {
            collect_from_read_slices_in_reducers(
                left,
                ctx,
                from_canon,
                hoisted_synthetic_keys,
                in_reducer,
                out,
            );
            collect_from_read_slices_in_reducers(
                right,
                ctx,
                from_canon,
                hoisted_synthetic_keys,
                in_reducer,
                out,
            );
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            collect_from_read_slices_in_reducers(
                cond,
                ctx,
                from_canon,
                hoisted_synthetic_keys,
                in_reducer,
                out,
            );
            collect_from_read_slices_in_reducers(
                then_e,
                ctx,
                from_canon,
                hoisted_synthetic_keys,
                in_reducer,
                out,
            );
            collect_from_read_slices_in_reducers(
                else_e,
                ctx,
                from_canon,
                hoisted_synthetic_keys,
                in_reducer,
                out,
            );
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
#[path = "ltm_agg_tests.rs"]
mod tests;
