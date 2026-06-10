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
//!   exception: a whole-RHS reducer with a MAPPED iterated axis (GH #534)
//!   mints a synthetic agg instead -- see the carve-out comment in
//!   [`walk_var_equation`]. Each such
//!   variable is its own distinct agg node -- variable-backed aggs are never
//!   deduped and never reused by an inline use of the same reducer text (an
//!   inline use must get its own *synthetic* node, since the element-graph
//!   reroute and the link-score emitter both filter to `is_synthetic` aggs;
//!   reusing the variable-backed node would silently leave the inline reducer
//!   on the conservative direct-scoring path, with the outcome depending on
//!   whether the whole-RHS reducer happened to be declared first).
//!
//! Each agg carries a [`AggNode::read_slice`] -- one [`AxisRead`] per source
//! axis -- recording *which rows* the reducer reads, so the element-graph
//! reroute and the per-element reducer link scores route only those rows.
//! Whole-extent reducers (`SUM(pop[*])`, `SUM(matrix[*,*])`) have an all-
//! `Reduced` slice; sliced reducers (`SUM(pop[NYC,*])` ⇒ `[Pinned(nyc),
//! Reduced]`, `SUM(matrix[D1,*])` over an A2A-`D1` body ⇒ `[Iterated(d1),
//! Reduced]` and an arrayed agg over `D1`) are hoisted too -- including a
//! positionally-MAPPED iterated axis (`SUM(matrix[State,*])` over a
//! `matrix[Region,..]` source with a `State→Region` mapping, GH #534), where
//! the `Iterated` axis carries the (target, source) dim pair and the agg is
//! arrayed over the TARGET dim. The carve-outs: a reducer over a *dynamic
//! index* (`SUM(pop[idx,*])`, `idx` non-literal) is not statically
//! describable, and a mapped iterated axis whose mapping is element-mapped
//! (GH #756), reverse-declared (GH #757), or non-positional is declined --
//! `compute_read_slice` returns `None`, the reducer is not hoisted, and its
//! reference stays on the conservative path.
//!
//! Whole-RHS partial reduces (`row_sum[D1] = SUM(matrix[D1,*])`) *are*
//! recognized -- the variable is the agg, `result_dims` carries its dims, and
//! `read_slice` records the `Iterated`/`Reduced` axis split. The element
//! graph routes them by the read slice too (GH #752,
//! [`variable_backed_partial_reduce_agg`]): each source row feeds only its
//! own `row_sum[<slot>]` element node -- matching the per-`(row, slot)` link
//! scores `try_cross_dimensional_link_scores` emits -- never the phantom
//! off-diagonal cross-product (whose loop scores referenced names that were
//! never emitted and stubbed to 0). Whole-extent variable-backed reducers
//! (`total = SUM(pop[*])`, including the broadcast `share[R] = SUM(pop[*])`)
//! keep the normal reference walker's reduction/broadcast edges, which are
//! already the true reads for those shapes; the gate's other exclusions
//! (partial reduce broadcast over extra target dims / permuted axes --
//! GH #764 -- and Pinned-bearing mixed slices, see
//! [`variable_backed_partial_reduce_agg`]) keep the conservative
//! cross-product, a SUPERSET of the true reads on the loud warned path.

use std::collections::HashMap;

use crate::ast::{Ast, Expr2, IndexExpr2};
use crate::builtins::BuiltinFn;
use crate::common::{Canonical, Ident, canonicalize};
use crate::db::{
    Db, SourceModel, SourceProject, project_datamodel_dims, reconstruct_model_variables,
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

/// [`reducer_kind_from_name`] applied to a `BuiltinFn`.
///
/// Generic over the contained expression type because it only inspects the
/// builtin's identity and arity, never the arguments themselves -- so
/// `BuiltinFn<Expr2>` (the element-graph walker, `classify_reducer`) and any
/// future `BuiltinFn<Expr0>` caller share one implementation.
pub(crate) fn reducer_kind<E>(builtin: &BuiltinFn<E>) -> Option<ReducerKind> {
    // Only `MEAN`/`MIN`/`MAX` are arity-sensitive; for everything else
    // `reducer_kind_from_name` ignores the arity argument.
    let arity = match builtin {
        BuiltinFn::Mean(args) => args.len(),
        BuiltinFn::Min(_, opt) | BuiltinFn::Max(_, opt) => 1 + opt.is_some() as usize,
        _ => 1,
    };
    reducer_kind_from_name(builtin.name(), arity)
}

/// `true` when `builtin` is a recognized array reducer that is *hoisted* into
/// an aggregate node -- i.e. recognized AND not [`ReducerKind::Constant`].
///
/// `SIZE` is recognized as a reducer but never hoisted (its link score is
/// always 0), and it never sets the element-graph walker's `in_reducer`
/// marker. This is the predicate the reference-site IR's AST walk
/// (`db::ltm_ir::walk_all_in_expr`) uses to flip `child_in_reducer`, and that
/// [`reducer_source_vars`] uses to gate which subexpressions become aggregate
/// nodes, so the agg enumerator, the element graph, and the link-score
/// generator all agree on the hoisted set.
pub(crate) fn reducer_is_hoistable<E>(builtin: &BuiltinFn<E>) -> bool {
    matches!(
        reducer_kind(builtin),
        Some(ReducerKind::Linear | ReducerKind::Nonlinear)
    )
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
/// - [`AxisRead::Reduced`] -- the whole axis is reduced away (`SUM(pop[*])`,
///   the `*` in `SUM(pop[NYC, *])`). Every element of that axis feeds the
///   single agg result slot.
#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
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
    /// The whole axis is reduced away (`SUM(pop[*])`); every element of it
    /// feeds the single agg result slot.
    Reduced,
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
    /// Canonical names of the model variables the reducer reads (sorted,
    /// deduplicated). For `SUM(a[*] + b[*])` this is `["a", "b"]`.
    pub source_vars: Vec<String>,
    /// The aggregate's result-axis dimension names, in datamodel casing
    /// (e.g. `["D1"]` for `row_sum[D1] = SUM(matrix[D1,*])` or for a
    /// synthetic agg minted from `x[D1] = ... + SUM(matrix[D1,*])`). Empty
    /// for a scalar reducer (`SUM(pop[*])`, `SUM(pop[NYC,*])`). These are
    /// the [`AxisRead::Iterated`] axes' dims, in order.
    pub result_dims: Vec<String>,
    /// One entry per source axis (in the source's declared dimension order):
    /// which rows of the arrayed source the reducer actually reads. Drives
    /// the element-graph reroute (`source[<pinned>,<iterated>,<reduced→rep>]
    /// → agg[<iterated>]`) and the per-element reducer link scores (only the
    /// read rows get a link score). For a multi-source reducer
    /// (`SUM(a[*] + b[*])`) every source ref shares this slice (the
    /// enumerator declines to hoist if they disagree). All-`Reduced` means a
    /// whole-extent reduce; see [`AxisRead`].
    pub read_slice: Vec<AxisRead>,
    /// `true` when a `$⁚ltm⁚agg⁚{n}` auxiliary must be minted to hold this
    /// value; `false` when the owning variable already *is* the aggregate
    /// node (its entire dt-equation is exactly this reducer).
    pub is_synthetic: bool,
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
        && let Some(read_slice) = combined_read_slice(builtin, ctx)
        // A whole-RHS reducer with a MAPPED iterated axis (GH #534,
        // `out[State] = SUM(matrix[State,*])` over a positionally-mapped
        // pair) is NOT variable-backed: it falls through to
        // `walk_subexpr_for_aggs`, which mints a *synthetic* agg for the
        // same reducer text. The variable-backed link-score path
        // (`try_cross_dimensional_link_scores`'s partial-reduce arm) matches
        // result axes against source axes BY NAME, so a remapped pair falls
        // off it onto `emit_per_shape_link_scores`' `Wildcard` partial --
        // whose PREVIOUS-wrapping mangles the iterated index into the
        // non-compiling `matrix[PREVIOUS(state),*]` (a silently-stubbed
        // constant-0 score). Routing through a synthetic agg instead gives
        // the whole-RHS case the same remapped two-half scoring as an
        // inline mapped reducer, at the cost of one synthetic aux
        // duplicating the variable's value.
        && !read_slice.iter().any(
            |a| matches!(a, AxisRead::Iterated { dim, source_dim } if dim != source_dim),
        )
    {
        // Whole-RHS reducer: the variable IS the aggregate node. The agg
        // node's result shape is the *reducer's* result shape (the `Iterated`
        // axes' dims), not the owning variable's: a full reduce
        // (`share[Region] = SUM(pop[*])`) has `result_dims == []` even though
        // it is broadcast to an arrayed variable (every element holds the same
        // value); a partial reduce keyed by the active A2A dimension
        // (`rowsum[D1] = SUM(matrix[D1, *])`) keeps `[D1]` as its result dims.
        let key = crate::patch::expr2_to_string(expr);
        let result_dims = result_dims_from_read_slice(&read_slice, ctx.dm_dims);
        register_agg(
            result,
            next_synthetic_n,
            &key,
            var_name,
            AggKind::VariableBacked {
                var_name: var_name.to_string(),
                result_dims,
                read_slice,
            },
            source_vars,
        );
        return;
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
            // for each (and they all agree). That covers the whole-extent case
            // (`SUM(pop[*])` ⇒ all-`Reduced`), the slice cases
            // (`SUM(pop[NYC,*])` ⇒ `[Pinned(nyc), Reduced]`,
            // `SUM(matrix[D1,*])` over an A2A-`D1` body ⇒
            // `[Iterated(d1), Reduced]` → an arrayed agg over `D1`), and
            // declines only the dynamic-index carve-out (`SUM(pop[idx,*])`,
            // `idx` non-literal ⇒ not statically describable). A *whole-RHS*
            // reducer (`agg[D1] = SUM(matrix[D1, *])`) is recognized too, but
            // as a variable-backed agg via `walk_var_equation`, not here.
            if !in_reducer
                && let Some(source_vars) = reducer_source_vars(builtin, ctx.variables)
                && let Some(read_slice) = combined_read_slice(builtin, ctx)
            {
                let key = crate::patch::expr2_to_string(expr);
                let result_dims = result_dims_from_read_slice(&read_slice, ctx.dm_dims);
                register_agg(
                    result,
                    next_synthetic_n,
                    &key,
                    owner_var,
                    AggKind::Synthetic {
                        result_dims,
                        read_slice,
                    },
                    source_vars,
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
    Synthetic {
        result_dims: Vec<String>,
        read_slice: Vec<AxisRead>,
    },
    /// The owning variable already is the aggregate node.
    VariableBacked {
        var_name: String,
        result_dims: Vec<String>,
        read_slice: Vec<AxisRead>,
    },
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
    source_vars: Vec<String>,
) {
    let mut sorted_sources = source_vars;
    sorted_sources.sort();
    sorted_sources.dedup();
    let idx = match kind {
        AggKind::Synthetic {
            result_dims,
            read_slice,
        } => {
            if let Some(&existing) = result.synthetic_by_key.get(key) {
                existing
            } else {
                let name = synthetic_agg_name(*next_synthetic_n);
                *next_synthetic_n += 1;
                result.aggs.push(AggNode {
                    name,
                    equation_text: key.to_string(),
                    source_vars: sorted_sources,
                    result_dims,
                    read_slice,
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
            read_slice,
        } => {
            // Each whole-RHS-reducer variable is its own aggregate node;
            // never deduped, and not entered in `synthetic_by_key`.
            result.aggs.push(AggNode {
                name: var_name,
                equation_text: key.to_string(),
                source_vars: sorted_sources,
                result_dims,
                read_slice,
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
/// Per source axis `i`:
/// - `IndexExpr2::Wildcard(_)` ⇒ [`AxisRead::Reduced`].
/// - `IndexExpr2::StarRange(_, _)` ⇒ [`AxisRead::Reduced`]. (Conservative
///   even when the named subdimension is a *proper* subset of the axis's own
///   dimension -- matching `classify_subscript_shape`'s AC1.4 treatment of an
///   all-`StarRange` subscript as `Wildcard`. The element-graph reroute then
///   over-approximates the unread rows, exactly as before; tightening this is
///   tracked as GH #766.)
/// - `IndexExpr2::Expr(Expr2::Var(d, ..))` where `d` (canonical) is one of
///   the *target equation's* iterated dimensions AND matches the source's
///   `i`-th declared dimension either *by name* or via a positional
///   dimension MAPPING declared on `d` toward it (`has_mapping_to(d, src)`
///   -- the same declared direction `classify_iterated_dim_shape`'s mapped
///   arm accepts -- with a usable [`iterated_axis_slot_elements`] remap,
///   which inherits `mapped_element_correspondence`'s positional-only gate)
///   ⇒ [`AxisRead::Iterated`] carrying the `(d, src)` pair (GH #534). The
///   three `Iterated`-axis consumers (`emit_agg_routed_edges`,
///   `read_slice_rows` behind `emit_source_to_agg_link_scores`, and
///   `emit_agg_to_target_link_scores` via `result_dims`) remap each source
///   row to the slot of its positionally-corresponding target element
///   through the same helper. Declined (⇒ `None`, conservative): an
///   explicit element-mapped pair (execution resolves positionally and
///   ignores the map -- GH #756), a mapping declared only in the REVERSE
///   direction (on the source's dimension; GH #757 tracks that direction's
///   classification separately -- do not widen here), an unmapped
///   position-mismatched pair, and a non-positional size mismatch.
///   (`classify_iterated_dim_shape`'s own mapped branch -- a
///   *whole*-equation-iterated subscript, not a sliced reducer argument --
///   is a separate code path and is unaffected.)
/// - `IndexExpr2::Expr(Expr2::Var(elem, ..))` or `Expr2::Const` resolving to
///   a literal element / 1-based index of the source's `i`-th dimension ⇒
///   [`AxisRead::Pinned`] carrying that element's canonical name.
/// - anything else (`DimPosition`, `Range`, a non-literal `Expr`, a
///   `Var`/`Const` that resolves to neither an iterated dim nor a literal
///   element) ⇒ `None`.
///
/// A bare `Expr2::Var(source, ..)` arg (no subscript) on an arrayed source ⇒
/// all-`Reduced` (`[Reduced; source.dims.len()]`). A reference to a *scalar*
/// model variable ⇒ `None` (it's not a reducer source). A `Subscript` whose
/// index count doesn't match the source's dimension count ⇒ `None`
/// (conservative -- a partial subscript is not the case Phase 4 hoists).
fn compute_read_slice(arg_expr: &Expr2, ctx: &AggWalkCtx<'_>) -> Option<Vec<AxisRead>> {
    let target_iterated_dims = ctx.target_iterated_dims;
    let variables = ctx.variables;
    let source_dims = |ident: &Ident<Canonical>| -> Option<&[crate::dimensions::Dimension]> {
        let dims = variables.get(ident).and_then(|v| v.get_dimensions())?;
        if dims.is_empty() { None } else { Some(dims) }
    };

    match arg_expr {
        Expr2::Var(ident, _, _) => {
            // A bare arrayed-variable arg reads the whole array.
            let dims = source_dims(ident)?;
            Some(vec![AxisRead::Reduced; dims.len()])
        }
        Expr2::Subscript(ident, indices, _, _) => {
            let dims = source_dims(ident)?;
            // A partial subscript (fewer/more indices than the source has
            // dimensions) is not the case Phase 4 hoists -- stay conservative.
            if indices.len() != dims.len() {
                return None;
            }
            let mut slice: Vec<AxisRead> = Vec::with_capacity(indices.len());
            for (i, idx) in indices.iter().enumerate() {
                let axis = match idx {
                    IndexExpr2::Wildcard(_) | IndexExpr2::StarRange(_, _) => AxisRead::Reduced,
                    IndexExpr2::Range(_, _, _) | IndexExpr2::DimPosition(_, _) => return None,
                    IndexExpr2::Expr(Expr2::Var(name, _, _)) => {
                        let name_str = name.as_str();
                        let src_dim_name = dims[i].name();
                        // An iterated-dimension index: the axis is iterated
                        // over the target's dimension space (and the agg
                        // result varies per element of it) iff `name` is one
                        // of the target's iterated dims AND it lines up with
                        // the source's i-th dim by name or by a positional
                        // mapping (GH #534).
                        if target_iterated_dims.iter().any(|t| t == name_str) {
                            if name_str == src_dim_name {
                                AxisRead::Iterated {
                                    dim: name_str.to_string(),
                                    source_dim: src_dim_name.to_string(),
                                }
                            } else {
                                // The iterated dim names a *different* source
                                // axis: a positional remap (`State→Region`,
                                // GH #534) is accepted -- carrying the
                                // (target, source) pair so the emitters remap
                                // each row to its slot -- when (a) the mapping
                                // is declared on the iterated dim toward the
                                // source's dim (the same direction
                                // `classify_iterated_dim_shape`'s mapped arm
                                // accepts; the reverse-declared direction is
                                // GH #757 and stays conservative) and (b) the
                                // slot remap exists (positional mappings
                                // only: explicit element maps decline via
                                // `mapped_element_correspondence`'s GH #756
                                // gate). Everything else -- a plain position
                                // mismatch, an element-mapped or
                                // reverse-declared pair -- declines, keeping
                                // the reference on the conservative path.
                                use crate::common::CanonicalDimensionName;
                                let d_canon = CanonicalDimensionName::from_raw(name_str);
                                let src_canon = CanonicalDimensionName::from_raw(src_dim_name);
                                let elems = crate::ltm_augment::dimension_element_names(&dims[i]);
                                if ctx.dim_ctx.has_mapping_to(&d_canon, &src_canon)
                                    && iterated_axis_slot_elements(
                                        name_str,
                                        src_dim_name,
                                        &elems,
                                        ctx.dim_ctx,
                                    )
                                    .is_some()
                                {
                                    AxisRead::Iterated {
                                        dim: name_str.to_string(),
                                        source_dim: src_dim_name.to_string(),
                                    }
                                } else {
                                    return None;
                                }
                            }
                        } else if let Some(elem) = resolve_literal_axis_index(idx, &dims[i]) {
                            AxisRead::Pinned(elem)
                        } else {
                            return None;
                        }
                    }
                    IndexExpr2::Expr(Expr2::Const(..)) => {
                        match resolve_literal_axis_index(idx, &dims[i]) {
                            Some(elem) => AxisRead::Pinned(elem),
                            None => return None,
                        }
                    }
                    IndexExpr2::Expr(_) => return None,
                };
                slice.push(axis);
            }
            Some(slice)
        }
        _ => None,
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

/// Compute the *combined* read slice of a reducer `builtin`'s arrayed source
/// references: walk its argument expressions, collect every reference to an
/// arrayed model variable, [`compute_read_slice`] each, and return the common
/// slice if (a) at least one such reference exists, (b) `compute_read_slice`
/// is `Some` for every one, and (c) they all agree. `None` otherwise -- the
/// reducer is not hoisted (the dynamic-index carve-out, or a multi-source
/// reducer whose references read incompatible slices).
fn combined_read_slice(builtin: &BuiltinFn<Expr2>, ctx: &AggWalkCtx<'_>) -> Option<Vec<AxisRead>> {
    let mut common: Option<Vec<AxisRead>> = None;
    let mut any_arrayed = false;
    let mut ok = true;
    builtin.for_each_expr_ref(|arg| {
        if ok {
            collect_arrayed_source_slices(arg, ctx, &mut common, &mut any_arrayed, &mut ok);
        }
    });
    if !ok || !any_arrayed {
        return None;
    }
    common
}

/// Recursive helper for [`combined_read_slice`]: descend `expr` (and any
/// nested subscript index expressions), folding each arrayed-source-variable
/// reference's [`compute_read_slice`] into `common` (and clearing `ok` on a
/// `None` or a disagreement). Scalar-variable references are ignored (a scalar
/// argument to a reducer is not a reducer source).
fn collect_arrayed_source_slices(
    expr: &Expr2,
    ctx: &AggWalkCtx<'_>,
    common: &mut Option<Vec<AxisRead>>,
    any_arrayed: &mut bool,
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
    let fold = |slice: Option<Vec<AxisRead>>, common: &mut Option<Vec<AxisRead>>, ok: &mut bool| {
        match slice {
            None => *ok = false,
            Some(s) => match common {
                None => *common = Some(s),
                Some(existing) if *existing == s => {}
                Some(_) => *ok = false,
            },
        }
    };
    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _, _) => {
            if ctx.variables.contains_key(ident) && is_arrayed(ident) {
                *any_arrayed = true;
                fold(compute_read_slice(expr, ctx), common, ok);
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            if ctx.variables.contains_key(ident) && is_arrayed(ident) {
                *any_arrayed = true;
                fold(compute_read_slice(expr, ctx), common, ok);
            }
            // Also descend into index expressions (a nested source ref).
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => {
                        collect_arrayed_source_slices(e, ctx, common, any_arrayed, ok)
                    }
                    IndexExpr2::Range(l, r, _) => {
                        collect_arrayed_source_slices(l, ctx, common, any_arrayed, ok);
                        collect_arrayed_source_slices(r, ctx, common, any_arrayed, ok);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            builtin.for_each_expr_ref(|sub| {
                collect_arrayed_source_slices(sub, ctx, common, any_arrayed, ok)
            });
        }
        Expr2::Op1(_, operand, _, _) => {
            collect_arrayed_source_slices(operand, ctx, common, any_arrayed, ok)
        }
        Expr2::Op2(_, left, right, _, _) => {
            collect_arrayed_source_slices(left, ctx, common, any_arrayed, ok);
            collect_arrayed_source_slices(right, ctx, common, any_arrayed, ok);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            collect_arrayed_source_slices(cond, ctx, common, any_arrayed, ok);
            collect_arrayed_source_slices(then_e, ctx, common, any_arrayed, ok);
            collect_arrayed_source_slices(else_e, ctx, common, any_arrayed, ok);
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
            AxisRead::Pinned(_) | AxisRead::Reduced => None,
        })
        .collect()
}

/// `true` when `name` is a synthetic aggregate-node name (`$⁚ltm⁚agg⁚{n}`).
pub(crate) fn is_synthetic_agg_name(name: &str) -> bool {
    name.starts_with(AGG_NAME_PREFIX)
}

/// The variable-backed PARTIAL-reduce aggregate node for the causal edge
/// `from -> to`, if any (GH #752): `to`'s entire dt-equation is a reducer
/// reading `from` (`to` IS the agg, `is_synthetic == false`) with at least
/// one [`AxisRead::Iterated`] axis, and the agg's `result_dims` are exactly
/// `to`'s declared dims, in order -- so each agg result slot names a complete
/// `to` element and the element graph can route the read-slice rows straight
/// to `to[<slot>]` (the diagonal `matrix[d1,d2] → row_sum[d1]` family whose
/// per-`(row, slot)` link scores `try_cross_dimensional_link_scores` emits).
///
/// This is the single gate shared by the element-graph reroute
/// (`model_element_causal_edges`' `Direct`-`Wildcard` dispatch) and the loop
/// builder (`build_element_level_loops`' per-circuit routing), so the two can
/// never disagree about which edges carry per-`(row, slot)` scores.
///
/// `None` (callers keep the conservative path) for:
/// - a whole-extent / pinned-only variable-backed reducer
///   (`total = SUM(pop[*])`, `share[R] = SUM(pop[*])`): no `Iterated` axis;
///   the reduction / broadcast edges the conservative path emits are already
///   the true reads.
/// - a whole-RHS partial reduce broadcast over extra target dims
///   (`out[D1,D3] = SUM(matrix[D1,*])`): `result_dims` is a strict subset of
///   `to`'s dims, so a slot does not name a complete `to` element (and the
///   per-`(row, slot)` link scores are not emitted for that shape either).
/// - a permuted-axes whole-RHS reduce: slot coordinates are in
///   `Iterated`-axis (source) order, which would mis-subscript `to`.
/// - a PINNED-bearing mixed slice (`outf[D1] = MEAN(cube[D1,x,*])`):
///   `try_cross_dimensional_link_scores` derives each slot's co-reduced
///   slice from the FULL source cartesian product, ignoring `Pinned` axes,
///   so its per-`(row, slot)` values are wrong for a pinned slice (e.g. the
///   MEAN divisor counts the unread rows -- GH #765). Accepting
///   the slice here would trade the loud conservative regime (cross-product
///   edges whose loop scores fail fragment compile with `Warning`s) for
///   silently wrong numbers; the exclusion goes when that derivation
///   respects the read slice.
pub(crate) fn variable_backed_partial_reduce_agg<'a>(
    aggs: &'a AggNodesResult,
    from: &str,
    to: &str,
    to_dims: &[crate::dimensions::Dimension],
) -> Option<&'a AggNode> {
    if to_dims.is_empty() {
        return None;
    }
    aggs.aggs_in_var(to).find(|a| {
        !a.is_synthetic
            && a.name == to
            && a.source_vars.iter().any(|s| s == from)
            && a.read_slice
                .iter()
                .any(|ax| matches!(ax, AxisRead::Iterated { .. }))
            // Every axis Iterated or Reduced: a Pinned-bearing slice is
            // excluded (see the rustdoc's last bullet).
            && a.read_slice
                .iter()
                .all(|ax| matches!(ax, AxisRead::Iterated { .. } | AxisRead::Reduced))
            && a.result_dims.len() == to_dims.len()
            && a.result_dims
                .iter()
                .zip(to_dims)
                .all(|(rd, td)| canonicalize(rd).as_ref() == td.name())
    })
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
        assert_eq!(agg.source_vars, vec!["population".to_string()]);
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
        assert_eq!(agg.source_vars, vec!["matrix".to_string()]);
        assert_eq!(agg.result_dims, vec!["D1".to_string()]);
        assert_eq!(
            agg.read_slice,
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced
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
        assert_eq!(agg.source_vars, vec!["pop".to_string()]);
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
        assert_eq!(synthetic[0].source_vars, vec!["pop".to_string()]);
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
        assert_eq!(share_agg.source_vars, vec!["pop".to_string()]);
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
        assert_eq!(synthetic[0].source_vars, vec!["a".to_string()]);
        assert_eq!(synthetic[1].name, "$\u{205A}ltm\u{205A}agg\u{205A}1");
        assert_eq!(synthetic[1].equation_text, "sum(b[*])");
        assert_eq!(synthetic[1].source_vars, vec!["b".to_string()]);
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
        assert_eq!(synthetic[0].source_vars, vec!["pop".to_string()]);
        assert_eq!(
            synthetic[0].read_slice,
            vec![AxisRead::Pinned("nyc".to_string()), AxisRead::Reduced]
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
            synthetic[0].read_slice,
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Reduced
            ]
        );
        assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
        assert_eq!(synthetic[0].source_vars, vec!["matrix".to_string()]);
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
            synthetic[0].read_slice,
            vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string()
                },
                AxisRead::Pinned("nyc".to_string()),
                AxisRead::Reduced
            ]
        );
        assert_eq!(synthetic[0].result_dims, vec!["D1".to_string()]);
        assert_eq!(synthetic[0].source_vars, vec!["matrix3d".to_string()]);
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
        assert_eq!(synthetic[0].read_slice, vec![AxisRead::Reduced]);
        assert!(synthetic[0].result_dims.is_empty());
        // `source_vars` lists every arrayed model variable in the argument.
        let mut srcs = synthetic[0].source_vars.clone();
        srcs.sort();
        assert_eq!(srcs, vec!["a".to_string(), "b".to_string()]);
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
                .all(|ag| !ag.source_vars.contains(&"a".to_string())
                    && !ag.source_vars.contains(&"b".to_string())),
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
            synthetic[0].read_slice,
            vec![
                AxisRead::Iterated {
                    dim: "state".to_string(),
                    source_dim: "region".to_string()
                },
                AxisRead::Reduced
            ]
        );
        assert_eq!(
            synthetic[0].result_dims,
            vec!["State".to_string()],
            "the agg's result axis is the TARGET equation's iterated dim"
        );
        assert_eq!(synthetic[0].source_vars, vec!["matrix".to_string()]);
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
            result
                .aggs
                .iter()
                .all(|a| !a.source_vars.contains(&"matrix".to_string())),
            "an element-mapped sliced reducer must not be hoisted; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
    }

    /// GH #534 (conservative gate, reverse-declared): a sliced reducer whose
    /// mapping is declared only in the REVERSE direction (on the source's
    /// `Region` toward `State`) stays un-hoisted, matching the direction
    /// `classify_iterated_dim_shape`'s mapped arm accepts
    /// (`has_mapping_to(iterated, source)` only; the reverse-subscripted
    /// direction is tracked by GH #757 -- not widened here).
    #[test]
    fn reverse_declared_mapped_sliced_reducer_is_not_hoisted() {
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
        assert!(
            result
                .aggs
                .iter()
                .all(|a| !a.source_vars.contains(&"matrix".to_string())),
            "a reverse-declared mapped sliced reducer must not be hoisted; got: {:?}",
            result.aggs
        );
        assert!(result.synthetic_by_key.is_empty());
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
            agg.read_slice,
            vec![
                AxisRead::Iterated {
                    dim: "state".to_string(),
                    source_dim: "region".to_string()
                },
                AxisRead::Reduced
            ]
        );
        assert_eq!(agg.result_dims, vec!["State".to_string()]);
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
            result
                .aggs
                .iter()
                .all(|a| !a.source_vars.contains(&"pop".to_string())),
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
        assert_eq!(synthetic[0].source_vars, vec!["matrix".to_string()]);
        assert_eq!(
            synthetic[0].read_slice,
            vec![AxisRead::Reduced, AxisRead::Reduced]
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

        // Hoistable: recognized AND not Constant -- SUM / 1-arg MEAN /
        // 1-arg MIN / 1-arg MAX / STDDEV / RANK. SIZE is recognized but never
        // hoisted (its link score is always 0); 2-arg MIN/MAX are not
        // recognized at all.
        assert!(reducer_is_hoistable(&sum));
        assert!(reducer_is_hoistable(&mean_1));
        assert!(reducer_is_hoistable(&min_1));
        assert!(reducer_is_hoistable(&max_1));
        assert!(reducer_is_hoistable(&stddev));
        assert!(reducer_is_hoistable(&rank));
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
}
