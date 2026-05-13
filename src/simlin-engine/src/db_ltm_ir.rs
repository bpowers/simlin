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
//! `db_analysis.rs` (their previous home before the IR existed). `RefShape`,
//! `emit_edges_for_reference`, and the element-name expansion helpers stay in
//! `db_analysis.rs`; this module imports `RefShape` via `crate::db::RefShape`.
//!
//! This is a top-level module (a sibling of `db`, like `ltm_agg`) rather than
//! a submodule of `db.rs` purely to keep `db.rs` under the per-file line cap;
//! callers in the `db` submodules use `crate::db_ltm_ir::...`.

use std::collections::HashMap;

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::db::{Db, RefShape, SourceModel, SourceProject, reconstruct_model_variables};

// ── AST-walker helpers (moved from db_analysis.rs) ─────────────────────────

/// One occurrence of a source variable in a target's AST -- the IR builder's
/// internal per-variable intermediate, before `in_reducer` + the hoisting
/// decision are folded into [`ClassifiedSite::routing`].
///
/// `target_element` is set only when the reference appears inside an
/// `Ast::Arrayed` per-element expression: the value is the canonical
/// element name (single-dim) or comma-separated tuple (multi-dim) of the
/// target element being defined. For `Ast::Scalar` and `Ast::ApplyToAll`
/// it stays `None` (the reference contributes to every target element
/// according to the shape's normal broadcast/diagonal rules).
///
/// `in_reducer` is true iff the reference site occurs syntactically inside
/// an array-reducing builtin call (`SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`/`RANK`
/// -- the `crate::ltm_agg::reducer_is_hoistable` set; `SIZE` and the 2-arg
/// `MIN`/`MAX` are *not* hoisted reducers). It is the precise signal for
/// "should this reference be rerouted through a hoisted aggregate node",
/// which the access `shape` alone cannot answer: a target with *both*
/// `SUM(pop[*])` and a direct `pop[idx]` produces a `DynamicIndex` site for
/// the *direct* `pop[idx]` reference too, and that one must keep its own
/// conservative element edge / Bare link score rather than collapsing into
/// the agg.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReferenceSite {
    pub shape: RefShape,
    pub target_element: Option<String>,
    pub in_reducer: bool,
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
///    *:Dim])` stays conservatively `DynamicIndex` -- the slice-reduce is
///    not hoisted yet; tracked as tech debt.)
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

/// Recognize an *iterated-dimension* subscript -- one whose indices are
/// exactly the target equation's iterated dimensions, in the position
/// matching the source's declared dimension order -- and classify it as
/// [`RefShape::Bare`] (a same-element-on-shared-dims reference, GH #511).
///
/// The precise rule: the subscript `source[d_0, d_1, ...]` is `Bare` iff
///   1. it has exactly one index per source dimension (a *partially*
///      iterated subscript -- some indices iterated, some literal/wildcard
///      -- is out of scope for Phase 3 and stays with its
///      `classify_subscript_shape` result; Phase 4 handles sliced reducers),
///   2. every index `d_i` is a bare `Var` naming a dimension that is one of
///      the target equation's iterated dimensions (`target_iterated_dims`),
///      *and*
///   3. for each `i`, `d_i` is either the same dimension name as the
///      source's `i`-th declared dimension `D_i`, or a dimension that *maps
///      to* `D_i` (the AC3.5 mapped-dimension case -- `State[i]` over a
///      source declared with `Region[i]` where `State` maps to `Region`).
///
/// That is exactly "the reference iterates over the target's dimension
/// space and reads the same element of the source" -- the thing
/// `emit_edges_for_reference`'s `Bare`-arrayed arm then projects via
/// `expand_same_element` (`row_sum[d1] -> growth[d1,*]`). A
/// position-mismatched subscript like `row_sum[D2]` inside `growth[D1,D2]`
/// where `row_sum` is over `D1` is a *genuine* cross-element reference --
/// `D2` doesn't match `row_sum`'s declared `D1` -- so it returns `None` and
/// keeps its `DynamicIndex` classification (out of scope here).
///
/// Returns `None` when the subscript is not this shape; the caller then
/// falls back to [`classify_subscript_shape`].
fn classify_iterated_dim_shape(
    indices: &[crate::ast::IndexExpr2],
    source_dims: &[crate::dimensions::Dimension],
    target_iterated_dims: &[String],
    dim_ctx: &crate::dimensions::DimensionsContext,
) -> Option<RefShape> {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::common::CanonicalDimensionName;

    // Need one index per source dimension; an empty subscript is never a
    // `Subscript` node, and a longer/shorter one is not the all-iterated
    // case (a partial slice or a dimensionally-mismatched reference).
    if indices.is_empty() || indices.len() != source_dims.len() {
        return None;
    }
    for (i, idx) in indices.iter().enumerate() {
        // Each index must be a bare `Var` -- a dimension name. (After
        // dimension resolution, an iterated dimension name in a subscript
        // stays an `Expr2::Var`; element names also parse that way, but the
        // `target_iterated_dims` membership check below rejects them.)
        let d = match idx {
            IndexExpr2::Expr(Expr2::Var(ident, _, _)) => ident.as_str(),
            _ => return None,
        };
        if !target_iterated_dims.iter().any(|t| t == d) {
            return None;
        }
        let src_dim_name = source_dims[i].name();
        if d == src_dim_name {
            continue;
        }
        // AC3.5: a mapped dimension is treated the same way -- don't
        // special-case it, just don't exclude it. The element-graph side's
        // existing dimension-mapping resolution (`expand_same_element` /
        // the dimension-mapping element expansion) continues to apply
        // unchanged; we only decline to *exclude* the mapped case from the
        // iterated-dim recognition here.
        let d_canon = CanonicalDimensionName::from_raw(d);
        let src_canon = CanonicalDimensionName::from_raw(src_dim_name);
        if dim_ctx.has_mapping_to(&d_canon, &src_canon) {
            continue;
        }
        return None;
    }
    Some(RefShape::Bare)
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
/// `child_in_reducer = in_reducer || reducer_is_hoistable(builtin)` (SIZE
/// excluded -- its result doesn't depend on element values). Walk order is
/// left-to-right DFS over the AST, matching `enumerate_agg_nodes`, so the
/// per-source site `Vec`s are deterministic (a salsa requirement on the
/// cached IR result).
fn collect_all_reference_sites(
    target_var: &crate::variable::Variable,
    variables: &HashMap<Ident<Canonical>, crate::variable::Variable>,
    dim_ctx: &crate::dimensions::DimensionsContext,
    lookup_dims: &mut impl FnMut(&str) -> Vec<crate::dimensions::Dimension>,
) -> HashMap<String, Vec<ReferenceSite>> {
    let mut sites: HashMap<String, Vec<ReferenceSite>> = HashMap::new();
    let Some(ast) = target_var.ast() else {
        return sites;
    };
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
    match ast {
        crate::ast::Ast::Scalar(expr) | crate::ast::Ast::ApplyToAll(_, expr) => {
            walk_all_in_expr(expr, &ctx, lookup_dims, None, false, &mut sites);
        }
        crate::ast::Ast::Arrayed(_, subscript_map, default_expr, _) => {
            // Per-element expressions: visit slots in canonical element-key
            // order so the per-source site Vecs are deterministic.
            let mut elem_keys: Vec<_> = subscript_map.keys().collect();
            elem_keys.sort();
            for k in elem_keys {
                walk_all_in_expr(
                    &subscript_map[k],
                    &ctx,
                    lookup_dims,
                    Some(k.as_str()),
                    false,
                    &mut sites,
                );
            }
            if let Some(default) = default_expr {
                walk_all_in_expr(default, &ctx, lookup_dims, None, false, &mut sites);
            }
        }
    }
    sites
}

/// Recursive helper for [`collect_all_reference_sites`]: left-to-right DFS
/// over an `Expr2` tree, pushing one [`ReferenceSite`] per model-variable
/// reference (bucketed by source name). `in_reducer` becomes `true` once we
/// descend into a `reducer_is_hoistable` builtin's argument and stays sticky
/// (a reducer nested in another reducer's arg is still inside *a* reducer);
/// `SIZE` is not `reducer_is_hoistable`, so it never sets the flag.
fn walk_all_in_expr(
    expr: &crate::ast::Expr2,
    ctx: &WalkCtx<'_>,
    lookup_dims: &mut impl FnMut(&str) -> Vec<crate::dimensions::Dimension>,
    target_element: Option<&str>,
    in_reducer: bool,
    sites: &mut HashMap<String, Vec<ReferenceSite>>,
) {
    use crate::ast::{Expr2, IndexExpr2};
    use crate::builtins::{BuiltinContents, walk_builtin_expr};

    let push = |from: &str, shape: RefShape, sites: &mut HashMap<String, Vec<ReferenceSite>>| {
        sites
            .entry(from.to_string())
            .or_default()
            .push(ReferenceSite {
                shape,
                target_element: target_element.map(|s| s.to_string()),
                in_reducer,
            });
    };

    match expr {
        Expr2::Const(..) => {}
        Expr2::Var(ident, _, _) => {
            if ctx.variables.contains_key(ident) {
                push(ident.as_str(), RefShape::Bare, sites);
            }
        }
        Expr2::Subscript(ident, indices, _, _) => {
            if ctx.variables.contains_key(ident) {
                let from_dims = lookup_dims(ident.as_str());
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
                push(ident.as_str(), shape, sites);
            }
            for idx in indices {
                match idx {
                    IndexExpr2::Expr(e) => {
                        walk_all_in_expr(e, ctx, lookup_dims, target_element, in_reducer, sites)
                    }
                    IndexExpr2::Range(l, r, _) => {
                        walk_all_in_expr(l, ctx, lookup_dims, target_element, in_reducer, sites);
                        walk_all_in_expr(r, ctx, lookup_dims, target_element, in_reducer, sites);
                    }
                    IndexExpr2::Wildcard(_)
                    | IndexExpr2::StarRange(_, _)
                    | IndexExpr2::DimPosition(_, _) => {}
                }
            }
        }
        Expr2::App(builtin, _, _) => {
            let child_in_reducer = in_reducer || crate::ltm_agg::reducer_is_hoistable(builtin);
            walk_builtin_expr(builtin, |contents| match contents {
                BuiltinContents::Ident(id, _) => {
                    if ctx.variables.contains_key(&Ident::<Canonical>::new(id)) {
                        push(id, RefShape::Bare, sites);
                    }
                }
                BuiltinContents::Expr(sub_expr) => walk_all_in_expr(
                    sub_expr,
                    ctx,
                    lookup_dims,
                    target_element,
                    child_in_reducer,
                    sites,
                ),
            });
        }
        Expr2::Op1(_, operand, _, _) => {
            walk_all_in_expr(operand, ctx, lookup_dims, target_element, in_reducer, sites)
        }
        Expr2::Op2(_, left, right, _, _) => {
            walk_all_in_expr(left, ctx, lookup_dims, target_element, in_reducer, sites);
            walk_all_in_expr(right, ctx, lookup_dims, target_element, in_reducer, sites);
        }
        Expr2::If(cond, then_e, else_e, _, _) => {
            walk_all_in_expr(cond, ctx, lookup_dims, target_element, in_reducer, sites);
            walk_all_in_expr(then_e, ctx, lookup_dims, target_element, in_reducer, sites);
            walk_all_in_expr(else_e, ctx, lookup_dims, target_element, in_reducer, sites);
        }
    }
}

// ── The classified-site IR ─────────────────────────────────────────────────

/// One classified reference site for a `(from, to)` causal edge.
///
/// Successor of `db_analysis::ReferenceSite`, generalized to fold the
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
#[derive(Debug, Clone, Default, PartialEq, Eq, salsa::Update)]
pub(crate) struct LtmReferenceSitesResult {
    pub sites: HashMap<(String, String), Vec<ClassifiedSite>>,
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
/// `source_vars` contains `from`), and the routing decision is the
/// byte-identical `route_through_agg = !routed_aggs.is_empty() && in_reducer`
/// the old element-graph / link-score walkers each restated.
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
    // `Region[i]`, `State` maps to `Region`) needs `has_mapping_to`. The
    // datamodel dim list (and hence this context) is salsa-tracked, so the
    // IR is recomputed when a dimension's mappings change.
    let dm_dims = crate::db::project_datamodel_dims(db, project);
    let dim_ctx = crate::dimensions::DimensionsContext::from(dm_dims.as_slice());

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

    for to_name in to_names {
        let to_var = &variables[to_name];
        let to_name_str = to_name.as_str();

        let raw_by_source =
            collect_all_reference_sites(to_var, &variables, &dim_ctx, &mut lookup_dims);
        if raw_by_source.is_empty() {
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

        for (from_name, raw_sites) in raw_by_source {
            // The byte-identical `routed_aggs` filter the old walkers each
            // restated: synthetic aggs of `to` that read `from`.
            let routed_aggs: Vec<usize> = synthetic_aggs_in_to
                .iter()
                .copied()
                .filter(|&i| {
                    agg_nodes.aggs[i]
                        .source_vars
                        .iter()
                        .any(|s| s == &from_name)
                })
                .collect();

            // Whether `to` is a *variable-backed* aggregate node whose source
            // includes `from` -- i.e. `to`'s whole equation is exactly the
            // reducer (`total = SUM(population[*])`, `row_sum[D1] =
            // SUM(matrix[D1,*])`). In that case the `(from, to)` edge *is* the
            // agg edge: `emit_edges_for_reference` projects the `Wildcard`
            // syntactic shape correctly (a reduction into the scalar/lower-
            // rank `to`), so it must keep its `Wildcard` shape.
            let to_is_variable_backed_agg = agg_nodes
                .by_var
                .get(to_name_str)
                .map(|idxs| {
                    idxs.iter().any(|&i| {
                        let a = &agg_nodes.aggs[i];
                        !a.is_synthetic
                            && a.name == to_name_str
                            && a.source_vars.iter().any(|s| s == &from_name)
                    })
                })
                .unwrap_or(false);

            let mut classified: Vec<ClassifiedSite> = Vec::new();
            for raw in raw_sites {
                // `route_through_agg = !routed_aggs.is_empty() && in_reducer`.
                if raw.in_reducer && !routed_aggs.is_empty() {
                    // Mirror the old `for agg in &routed_aggs` loop: route
                    // this reference through every routed agg.
                    for &agg_idx in &routed_aggs {
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
                    // describable. Reclassify it as `DynamicIndex` so the
                    // `Wildcard` *variant* only ever means "a hoisted reducer's
                    // (ignored) syntactic shape" or "a whole-RHS variable-
                    // backed reducer's argument" and never reaches
                    // `emit_edges_for_reference`'s conservative cross-product
                    // arm via a `Direct` site (#514 AC4.5: the conservative
                    // cross-product is `DynamicIndex`-only there).
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

    LtmReferenceSitesResult { sites }
}

#[cfg(test)]
#[path = "db_ltm_ir_tests.rs"]
mod db_ltm_ir_tests;
