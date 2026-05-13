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
use crate::common::{Canonical, Ident};
use crate::datamodel::{self, Equation};
use crate::lexer::LexerType;
use crate::ltm::{
    CyclePartitions, Loop, normalize_module_ref, split_node_subscript, strip_subscript,
};
use crate::variable::{Variable, identifier_set};
use std::collections::{HashMap, HashSet};

use crate::db::RefShape;

/// Context for recognizing GH #511 iterated-dimension source references in
/// the partial-equation builder: the live source's declared dimension names
/// (canonical, in declaration order; same length as `source_dim_elements`),
/// the target equation's iterated dimension names (canonical, in the order
/// they appear on `Ast::ApplyToAll`/`Ast::Arrayed`), and a `DimensionsContext`
/// for the AC3.5 mapped-dimension case. `build_partial_equation_shaped` is
/// passed `None` by callers whose live source is a scalar (or an aggregate
/// node) -- those have no source subscripts, so iterated-dim recognition
/// never applies.
pub(crate) struct IteratedDimCtx<'a> {
    pub source_dim_names: &'a [String],
    pub target_iterated_dims: &'a [String],
    pub dim_ctx: Option<&'a crate::dimensions::DimensionsContext>,
}

/// Recognize an *iterated-dimension* `Expr0` subscript on the *live source*
/// -- one whose indices are exactly the target equation's iterated
/// dimensions, in the position matching the live source's declared
/// dimension order -- the Expr0-AST sibling of
/// `db_ltm_ir::classify_iterated_dim_shape` (GH #511).
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
fn is_live_source_iterated_dim_subscript(
    indices: &[IndexExpr0],
    ctx: Option<&IteratedDimCtx<'_>>,
) -> bool {
    use crate::common::CanonicalDimensionName;
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
        let src_name = &ctx.source_dim_names[i];
        if &d == src_name {
            continue;
        }
        // AC3.5: a mapped dimension is treated the same way -- don't
        // special-case it, just don't exclude it. (No mapping context => no
        // mapped-dimension recognition; the by-name check above still applies.)
        if let Some(dim_ctx) = ctx.dim_ctx {
            let d_canon = CanonicalDimensionName::from_raw(&d);
            let src_canon = CanonicalDimensionName::from_raw(src_name);
            if dim_ctx.has_mapping_to(&d_canon, &src_canon) {
                continue;
            }
        }
        return false;
    }
    true
}

/// Recognize an iterated-dimension subscript on a *non-live-source*
/// dependency (e.g. `pop[Region,Age]` in `growth[Region,Age] =
/// row_sum[Region] * c * pop[Region,Age]` while building the partial for
/// `(row_sum, growth)`). We don't know the dep's declared dimensions here
/// (the partial builder works on equation text + a dep *name* set), so this
/// is the conservative check: every index is a bare `Var` naming one of the
/// target's iterated dimensions, and there are at most as many indices as
/// the target has iterated dimensions. When it matches,
/// `wrap_non_matching_in_previous` collapses the subscript to a bare
/// `Var(dep)` before wrapping it in `PREVIOUS()` -- avoiding the
/// `PREVIOUS(Subscript(...))` codegen assertion.
///
/// For the *natural* (non-transposed, same-position) case -- the dep is
/// declared over exactly those iteration dimensions in the same order, so
/// each `dep[d_i]` in slot `(d_0, d_1, ...)` reads element `(d_0, d_1,
/// ...)` -- the frozen `PREVIOUS(dep)` picks the same element
/// `PREVIOUS(dep[...])` would, and the collapse is exact.
///
/// For *non-natural-position* array deps the collapse is
/// conservative-by-design (never a codegen error, and the link-score SIGN
/// factor -- the sign of `dep`'s value -- is preserved when `dep` is
/// positive) but magnitude-imprecise. The *transposed* sub-case
/// (`arr[D2,D1]` inside an A2A-over-`D1×D2` equation where `arr` is
/// declared `D1×D2`): in slot `(d1, d2)` the equation reads `arr[d2,d1]`,
/// but `PREVIOUS(arr)` freezes `arr[d1,d2]_prev` -- the wrong element, so
/// the magnitude of the link-score contribution is off (the sign is still
/// right if `arr` is positive). The *mapped-but-position-mismatched*
/// sub-case (`dep[D2]` where `dep` is over `D1` and `D2` maps to `D1`)
/// similarly over-collapses to the `D1`-element rather than the mapped
/// `D2`-element. Such models are already not statically scoreable in
/// pre-#511 LTM; the precise non-natural-position handling is a known
/// limitation tracked separately.
fn is_other_dep_iterated_dim_subscript(
    indices: &[IndexExpr0],
    ctx: Option<&IteratedDimCtx<'_>>,
) -> bool {
    let Some(ctx) = ctx else { return false };
    if indices.is_empty() || indices.len() > ctx.target_iterated_dims.len() {
        return false;
    }
    indices.iter().all(|idx| match idx {
        IndexExpr0::Expr(Expr0::Var(name, _)) => {
            let d = canonicalize(name.as_str());
            ctx.target_iterated_dims
                .iter()
                .any(|t| t.as_str() == d.as_ref())
        }
        _ => false,
    })
}

/// Classify an `Expr0` subscript's shape based on its indices.
///
/// Mirrors `db_analysis::resolve_literal_index`'s classification logic but at
/// the `Expr0` (parsed-AST) level — used by `wrap_non_matching_in_previous`
/// before subscripts have been lowered to `Expr2`. Each input string in
/// `source_dim_elements` is the canonical lowercase element name for the
/// corresponding source dimension, in source-declared order.
///
/// Rules:
/// - any `IndexExpr0::Wildcard` → `RefShape::Wildcard`
/// - all indices are literal element names that match the source's
///   declared elements (or parseable integer literals for indexed
///   dimensions) → `RefShape::FixedIndex(canonical_names)`
/// - otherwise (StarRange, DimPosition, Range, non-literal Expr, or a
///   literal that doesn't match) → `RefShape::DynamicIndex`
///
/// The match tries each index against the dimension at that position
/// first, then falls back to scanning all dimensions. This keeps the
/// classifier robust when callers pass dimensions in source-declared
/// order but the subscript indices may not align 1:1 with dimension
/// positions in unusual cases. Defensive `DynamicIndex` for unknown
/// names ensures the worst case is over-conservative wrapping rather
/// than incorrectly matching the live shape.
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
fn is_literal_element_index(
    idx: &IndexExpr0,
    position: usize,
    source_dim_elements: &[Vec<String>],
) -> bool {
    resolve_literal_element_index(idx, position, source_dim_elements).is_some()
}

/// Resolve a single subscript index to a literal element name, mirroring
/// `db_analysis::resolve_literal_index` (the Expr2 sibling) so both
/// classifiers agree on what counts as a "literal element". The two
/// must stay in sync: the edge emitter uses the Expr2 classifier and
/// the partial-equation builder uses this Expr0 sibling -- if they
/// disagree (for example on out-of-range integer literals), the edge
/// emitter classifies as `DynamicIndex` while the partial builder
/// classifies as `FixedIndex(...)`, the shape comparison in
/// `wrap_non_matching_in_previous` fails, and the live reference is
/// wrapped in `PREVIOUS()` -- silently zeroing the link score.
///
/// Element names appear as `Var` nodes; integer literals appear as
/// `Const` nodes whose text is the integer. Either form is validated
/// by membership in `source_dim_elements`. For an indexed dim of size
/// N, `dimension_element_names` produces `["1", "2", ..., "N"]`, so a
/// `Const("999", ...)` over an indexed dim of size 5 won't match and
/// falls through to `None`. Matching prefers the dim at the index's
/// position, falling back to any dim if not found there.
fn resolve_literal_element_index(
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
            // Expr2 sibling (`db_analysis::resolve_literal_index`)
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

fn classify_expr0_subscript_shape(
    indices: &[IndexExpr0],
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
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
    // `db_ltm_ir::classify_iterated_dim_shape`. Checked before the
    // literal-element pass because a dimension name (`Region`) is not a
    // literal element, so it would otherwise fall to `DynamicIndex`.
    if is_live_source_iterated_dim_subscript(indices, iter_ctx) {
        return RefShape::Bare;
    }
    let mut elems = Vec::with_capacity(indices.len());
    for (i, idx) in indices.iter().enumerate() {
        // Use the same resolver as `is_literal_element_index` so this
        // classifier and the Expr2 sibling
        // (`db_analysis::resolve_literal_index`) agree on what counts
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

/// Does `name` (case-insensitively) name an array-reducing builtin in the
/// form that collapses an array dimension? `SUM`/`STDDEV`/`SIZE`/`RANK`
/// reduce at any arity (`RANK(arr, n)`, etc.); `MEAN`/`MIN`/`MAX` reduce an
/// array dimension only in their single-argument form (their multi-argument
/// forms are element-wise). Parsed `Expr0` builtin names keep their source
/// casing, generated ones are uppercase, so the comparison is
/// case-insensitive. A thin reader of [`crate::ltm_agg::reducer_kind_from_name`]
/// -- the one reducer table -- so this `Expr0`-walk-time check and the agg
/// enumerator agree on the set (including `SIZE`, which is recognized here
/// even though it is never hoisted).
fn is_array_reducer_name(name: &str, arity: usize) -> bool {
    crate::ltm_agg::reducer_kind_from_name(&name.to_ascii_lowercase(), arity).is_some()
}

/// Does `expr` contain the live source reference the partial isolates -- a
/// bare `Var(live_source)` (when `live_shape` is `Bare`) or a
/// `Subscript(live_source, indices)` whose access shape equals `live_shape`?
///
/// Used by [`wrap_non_matching_in_previous`] to decide whether an enclosing
/// array-reducer App is "other content" (so the whole reducer should be
/// `PREVIOUS`-wrapped -- `PREVIOUS(SUM(arr[*]))`, which evaluates fine) or
/// genuinely holds the live reference (so it must be recursed into, e.g. the
/// test-only `RefShape::Wildcard` path where `SUM(arr[*])` *is* the live
/// thing). See GH #517: the alternative -- recursing and emitting
/// `SUM(PREVIOUS(arr[*]))` -- is silently `0.0` at every step under an
/// active apply-to-all dimension because codegen has no
/// LoadPrev-of-array-view path.
fn expr0_contains_live_match(
    expr: &Expr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> bool {
    match expr {
        Expr0::Const(..) => false,
        Expr0::Var(ident, _) => {
            matches!(live_shape, RefShape::Bare) && &Ident::new(ident.as_str()) == live_source
        }
        Expr0::Subscript(ident, indices, _) => {
            // A `live_source` occurrence reachable only through an index
            // expression (`other_arr[live_source]`) is never the captured
            // live ref -- `wrap_index_non_matching_in_previous` passes a
            // throwaway sink for those -- so we only consider a subscript
            // whose *head* is `live_source` with the matching shape. An
            // iterated-dimension subscript classifies `Bare` here too (it is
            // collapsed to its head `Var` in `wrap_non_matching_in_previous`).
            &Ident::new(ident.as_str()) == live_source
                && &classify_expr0_subscript_shape(indices, source_dim_elements, iter_ctx)
                    == live_shape
        }
        Expr0::App(UntypedBuiltinFn(_, args), _) => args.iter().any(|a| {
            expr0_contains_live_match(a, live_source, live_shape, source_dim_elements, iter_ctx)
        }),
        Expr0::Op1(_, inner, _) => expr0_contains_live_match(
            inner,
            live_source,
            live_shape,
            source_dim_elements,
            iter_ctx,
        ),
        Expr0::Op2(_, l, r, _) => {
            expr0_contains_live_match(l, live_source, live_shape, source_dim_elements, iter_ctx)
                || expr0_contains_live_match(
                    r,
                    live_source,
                    live_shape,
                    source_dim_elements,
                    iter_ctx,
                )
        }
        Expr0::If(c, t, e, _) => {
            expr0_contains_live_match(c, live_source, live_shape, source_dim_elements, iter_ctx)
                || expr0_contains_live_match(
                    t,
                    live_source,
                    live_shape,
                    source_dim_elements,
                    iter_ctx,
                )
                || expr0_contains_live_match(
                    e,
                    live_source,
                    live_shape,
                    source_dim_elements,
                    iter_ctx,
                )
        }
    }
}

/// Walk an `Expr0` tree and wrap variable references in `PREVIOUS()` except
/// those whose access shape matches the live shape for the given source,
/// recording into `live_ref` the *first* `live_source` occurrence left
/// live (in document order, after the transform).
///
/// `live_source` identifies the source variable whose live shape is held
/// out from `PREVIOUS` wrapping. `live_shape` declares which AST occurrences
/// of that source remain live; all other occurrences (and all references
/// to other sources in the same expression) are wrapped.
///
/// `other_deps` is the set of canonical idents for non-`live_source`
/// dependencies that must be wrapped; nodes referencing names not in this
/// set and not equal to `live_source` are left alone (function names and
/// unknown identifiers). Indices of subscripts are recursively transformed
/// even when the outer subscript matches the live shape, so nested
/// references like `arr[other_var]` still get wrapped.
///
/// `iter_ctx` carries the GH #511 iterated-dimension context (the live
/// source's declared dimension names + the target equation's iterated
/// dimensions + a `DimensionsContext` for the mapped case); when `Some`,
/// an iterated-dimension subscript is normalized to a bare `Var` *before*
/// the live/PREVIOUS dispatch -- so `row_sum[D1]` (a same-element reference
/// over the target's own `D1`) becomes either the live ref (`live_shape ==
/// Bare`) or `PREVIOUS(Var(row_sum))` (a `Var` arg, which codegen accepts,
/// vs the `PREVIOUS(Subscript(...))` the pre-#511 code produced). Pass
/// `None` for callers whose live source is scalar (no source subscripts).
///
/// `live_ref` ends up holding the bare `Var(live_source)` for a `Bare`
/// shape, or the (already index-transformed) `Subscript(live_source, ...)`
/// for `FixedIndex`/`Wildcard`/`DynamicIndex`. Callers use this captured
/// subtree to build the link-score's source-side normalizer: it is the
/// source reference *as the partial isolates it*, so `Δ(live_ref)` is the
/// exact source velocity feeding the `SIGN` factor -- crucially, a
/// per-element / per-slice expression rather than the (possibly
/// multi-dimensional) bare `live_source`, which would be a dimension error
/// in a scalar link-score equation. Pass `&mut None` to ignore it.
fn wrap_non_matching_in_previous(
    expr: Expr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    live_ref: &mut Option<Expr0>,
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(ident.as_str());
            if &canonical == live_source {
                // The bare-Var occurrence matches `Bare`. Any other live
                // shape (FixedIndex / Wildcard / DynamicIndex) doesn't
                // match a bare reference, so we wrap.
                if matches!(live_shape, RefShape::Bare) {
                    if live_ref.is_none() {
                        *live_ref = Some(expr.clone());
                    }
                    expr
                } else {
                    Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
                }
            } else if other_deps.contains(&canonical) {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            let canonical = Ident::new(ident.as_str());
            // GH #511: an iterated-dimension subscript reads the same element
            // a bare reference would in each slot of the target equation, so
            // normalize it to a bare `Var` here -- *before* the live/PREVIOUS
            // dispatch -- and re-run the (much simpler) `Var` logic. The
            // model equation is untouched; only the LTM partial's `Expr0` is
            // rewritten, so simulation still evaluates `live_source[d_i]`.
            // For the live source we use the precise position-matched check;
            // for a non-live dep (`pop[Region,Age]` while building the
            // `row_sum -> growth` partial) the conservative all-indices-are-
            // iterated-dims check (the dep's arity isn't known here). Either
            // way, the alternative -- `PREVIOUS(Subscript(...))` -- trips the
            // codegen assertion.
            if &canonical == live_source {
                if is_live_source_iterated_dim_subscript(&indices, iter_ctx) {
                    return wrap_non_matching_in_previous(
                        Expr0::Var(ident, loc),
                        live_source,
                        live_shape,
                        other_deps,
                        source_dim_elements,
                        iter_ctx,
                        live_ref,
                    );
                }
            } else if other_deps.contains(&canonical)
                && is_other_dep_iterated_dim_subscript(&indices, iter_ctx)
            {
                return wrap_non_matching_in_previous(
                    Expr0::Var(ident, loc),
                    live_source,
                    live_shape,
                    other_deps,
                    source_dim_elements,
                    iter_ctx,
                    live_ref,
                );
            }
            // Classify the subscript's shape using the ORIGINAL indices
            // BEFORE recursing into them. If a user variable shares a
            // name with a dimension element (e.g., a variable also named
            // `NYC`), recursing first would rewrite `Var(NYC)` as
            // `App(PREVIOUS, [Var(NYC)])`, and then classification would
            // fall through to `DynamicIndex`, breaking a live FixedIndex
            // shape match.
            let subscript_shape =
                classify_expr0_subscript_shape(&indices, source_dim_elements, iter_ctx);
            if &canonical == live_source && &subscript_shape == live_shape {
                // Live reference: the OUTER subscript stays unwrapped.
                // Decide per-index whether to recurse:
                //
                //   - Literal element refs (Var matching a dim element,
                //     or an integer literal in an indexed dim) are
                //     dimension references at runtime; leave them
                //     verbatim so a variable/element name collision
                //     doesn't wrap them.
                //
                //   - Wildcard tokens (`*`) have no inner content to
                //     wrap; recursing is a no-op so doing it for
                //     uniformity is fine.
                //
                //   - Non-literal indices (expressions like `idx + helper`
                //     in `RefShape::DynamicIndex`) are computational
                //     content; recurse so any `other_deps` referenced
                //     inside get held at PREVIOUS for ceteris-paribus.
                //
                // Without the per-index split, DynamicIndex live refs
                // would skip wrapping inner deps and the partial
                // equation would no longer be ceteris-paribus.
                let indices: Vec<IndexExpr0> = indices
                    .into_iter()
                    .enumerate()
                    .map(|(i, idx)| {
                        if is_literal_element_index(&idx, i, source_dim_elements) {
                            idx
                        } else {
                            wrap_index_non_matching_in_previous(
                                idx,
                                live_source,
                                live_shape,
                                other_deps,
                                source_dim_elements,
                                iter_ctx,
                            )
                        }
                    })
                    .collect();
                let subscript = Expr0::Subscript(ident, indices, loc);
                if live_ref.is_none() {
                    *live_ref = Some(subscript.clone());
                }
                return subscript;
            }
            // Non-live reference: recurse into indices so any nested
            // user-variable references get wrapped, then build the new
            // subscript. If the outer ident is itself a dep, wrap the
            // whole thing.
            let indices: Vec<IndexExpr0> = indices
                .into_iter()
                .map(|idx| {
                    wrap_index_non_matching_in_previous(
                        idx,
                        live_source,
                        live_shape,
                        other_deps,
                        source_dim_elements,
                        iter_ctx,
                    )
                })
                .collect();
            let subscript = Expr0::Subscript(ident, indices, loc);
            if &canonical == live_source || other_deps.contains(&canonical) {
                Expr0::App(
                    UntypedBuiltinFn("PREVIOUS".to_string(), vec![subscript]),
                    loc,
                )
            } else {
                subscript
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            // GH #517: an array-reducer subexpression (`SUM(pop[*])`,
            // `MEAN(...)`, `SUM(m[D1,*])`, ...) that does not itself carry
            // the live reference is "other content" for ceteris-paribus
            // purposes. Wrap the whole reducer in `PREVIOUS` --
            // `PREVIOUS(SUM(pop[*]))`, which is `PREVIOUS` of a scalar (the
            // reducer's result, even a partial reduce, is scalar in the
            // enclosing apply-to-all context) and evaluates fine -- rather
            // than recursing into it and emitting `SUM(PREVIOUS(pop[*]))`,
            // which is silently `0.0` at every step under an active A2A
            // dimension because codegen has no LoadPrev-of-array-view path.
            // If the live reference *is* inside this reducer (the now
            // test-only `RefShape::Wildcard` path where `SUM(pop[*])` is the
            // live thing), recurse normally so the live `pop[*]` stays
            // unwrapped.
            if is_array_reducer_name(&name, args.len())
                && !args.iter().any(|a| {
                    expr0_contains_live_match(
                        a,
                        live_source,
                        live_shape,
                        source_dim_elements,
                        iter_ctx,
                    )
                })
            {
                let reducer = Expr0::App(UntypedBuiltinFn(name, args), loc);
                return Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![reducer]), loc);
            }
            let args = args
                .into_iter()
                .map(|a| {
                    wrap_non_matching_in_previous(
                        a,
                        live_source,
                        live_shape,
                        other_deps,
                        source_dim_elements,
                        iter_ctx,
                        live_ref,
                    )
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(wrap_non_matching_in_previous(
                *inner,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                live_ref,
            )),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_non_matching_in_previous(
                *lhs,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                live_ref,
            )),
            Box::new(wrap_non_matching_in_previous(
                *rhs,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                live_ref,
            )),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(wrap_non_matching_in_previous(
                *cond,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                live_ref,
            )),
            Box::new(wrap_non_matching_in_previous(
                *then_expr,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                live_ref,
            )),
            Box::new(wrap_non_matching_in_previous(
                *else_expr,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                live_ref,
            )),
            loc,
        ),
    }
}

fn wrap_index_non_matching_in_previous(
    index: IndexExpr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> IndexExpr0 {
    // Indices are inner content of a live reference (or of a
    // PREVIOUS-wrapped one); a `live_source` occurrence reachable only
    // through an index is not the live reference itself, so do not
    // capture it -- pass a throwaway sink.
    match index {
        IndexExpr0::Expr(e) => IndexExpr0::Expr(wrap_non_matching_in_previous(
            e,
            live_source,
            live_shape,
            other_deps,
            source_dim_elements,
            iter_ctx,
            &mut None,
        )),
        IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
            wrap_non_matching_in_previous(
                l,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                &mut None,
            ),
            wrap_non_matching_in_previous(
                r,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                &mut None,
            ),
            loc,
        ),
        other => other,
    }
}

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
/// `iter_ctx` is the GH #511 iterated-dimension context (the target's
/// iterated dims + the source's declared dim names + a `DimensionsContext`);
/// pass `None` when the live source is scalar (no source subscripts to
/// recognize). See [`wrap_non_matching_in_previous`] and [`IteratedDimCtx`].
pub(crate) fn build_partial_equation_shaped(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> String {
    build_partial_equation_shaped_with_live_ref(
        equation_text,
        deps,
        live_source,
        live_shape,
        source_dim_elements,
        iter_ctx,
    )
    .0
}

/// Like [`build_partial_equation_shaped`], but also returns the *live
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
/// Returns `None` for the second element when the equation fails to parse
/// (the partial then degrades to the lowercased input) or contains no
/// left-live `live_source` occurrence at all.
pub(crate) fn build_partial_equation_shaped_with_live_ref(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> (String, Option<Expr0>) {
    let other_deps: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| *d != live_source && normalize_module_ref(d) != *live_source)
        .cloned()
        .collect();

    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return (equation_text.to_lowercase(), None);
    };

    let mut live_ref: Option<Expr0> = None;
    let transformed = wrap_non_matching_in_previous(
        ast,
        live_source,
        live_shape,
        &other_deps,
        source_dim_elements,
        iter_ctx,
        &mut live_ref,
    );
    (print_eqn(&transformed), live_ref)
}

/// Replace every bare `Var(id)` reference in `equation_text` where `id`
/// (canonicalized) is in `idents` with `Subscript(id, [element])`,
/// pinning that variable to `element`.
///
/// Used when collapsing a scalar-source -> arrayed-target link score into
/// per-target-element scalar variables: the target's A2A equation body
/// references arrayed deps that share the target's dimension *bare* (the
/// A2A expansion subscripts them at runtime), but a *scalar* per-element
/// link-score variable must spell out the subscript. `idents` is the set
/// of those deps (the caller computes it -- it needs to know which deps
/// are arrayed and share the target's dimension).
///
/// `Subscript` nodes are left untouched: a dep that already carries an
/// explicit subscript (an `Ast::Arrayed` per-element slot wrote
/// `population[NYC]`) is already element-pinned, and double-subscripting
/// would be nonsensical. Function-name identifiers and identifiers not in
/// `idents` are likewise left alone. The result is re-printed in the
/// canonical equation format (via parse + `print_eqn`); a parse failure
/// degrades to the lowercased input, matching `build_partial_equation_shaped`.
///
/// `element` is a single element name (`"nyc"`) for a one-dimensional
/// target, or a comma-joined tuple (`"nyc,adult"`) for a multi-dimensional
/// one -- the same form `db_ltm::cartesian_subscripts` produces and the
/// `parse_link_offsets` discovery parser expects on the `to` side.
pub(crate) fn subscript_idents_at_element(
    equation_text: &str,
    idents: &HashSet<Ident<Canonical>>,
    element: &str,
) -> String {
    if idents.is_empty() {
        return equation_text.to_string();
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return equation_text.to_lowercase();
    };
    let index_exprs: Vec<IndexExpr0> = element
        .split(',')
        .map(|e| {
            IndexExpr0::Expr(Expr0::Var(
                crate::common::RawIdent::new_from_str(e.trim()),
                crate::ast::Loc::default(),
            ))
        })
        .collect();
    print_eqn(&subscript_idents_in_expr0(ast, idents, &index_exprs))
}

fn subscript_idents_in_expr0(
    expr: Expr0,
    idents: &HashSet<Ident<Canonical>>,
    index_exprs: &[IndexExpr0],
) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            let canonical = Ident::new(ident.as_str());
            if idents.contains(&canonical) {
                Expr0::Subscript(ident.clone(), index_exprs.to_vec(), loc)
            } else {
                expr
            }
        }
        // Already-subscripted references are element-pinned by their own
        // index; leave them (and their indices) untouched.
        Expr0::Subscript(..) => expr,
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let args = args
                .into_iter()
                .map(|a| subscript_idents_in_expr0(a, idents, index_exprs))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(subscript_idents_in_expr0(*inner, idents, index_exprs)),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(subscript_idents_in_expr0(*lhs, idents, index_exprs)),
            Box::new(subscript_idents_in_expr0(*rhs, idents, index_exprs)),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(subscript_idents_in_expr0(*cond, idents, index_exprs)),
            Box::new(subscript_idents_in_expr0(*then_expr, idents, index_exprs)),
            Box::new(subscript_idents_in_expr0(*else_expr, idents, index_exprs)),
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
/// `to_elem_eqn_text` is the target's equation text for this element: the
/// shared A2A body for an `Equation::ApplyToAll` target, or the matching
/// per-element slot's text (or the default slot) for an `Equation::Arrayed`
/// one. `to_deps` is the full dependency set of that equation (computed with
/// the target's AST dimensions so element-name subscripts are not mistaken
/// for variables). `to_deps_to_subscript` is the subset of `to_deps` that
/// must be element-pinned -- the arrayed deps that share the target's
/// dimension (the target self-reference is pinned implicitly via the
/// already-subscripted `to[element]` reference the guard form is built
/// around).
///
/// `source_ref_override`: the pre-rendered (quoted, possibly element-pinned)
/// reference expression to use for the `Δsource` denominator. `None` uses the
/// bare `quote_ident(from)` -- correct for a true scalar source. The
/// arrayed-agg caller passes `Some("$⁚ltm⁚agg⁚n"[<slot>])` so the denominator
/// indexes the same agg slot the link-score name and the (subscripted-in-the-
/// partial) numerator do; a bare agg reference in a scalar equation would not
/// compile and the link score would stub to zero.
pub(crate) fn generate_scalar_to_element_equation(
    from: &str,
    to: &str,
    element: &str,
    to_elem_eqn_text: &str,
    to_deps: &HashSet<Ident<Canonical>>,
    to_deps_to_subscript: &HashSet<Ident<Canonical>>,
    source_ref_override: Option<&str>,
) -> String {
    let from_canonical = Ident::new(from);
    let from_q = quote_ident(from);
    let to_q = quote_ident(to);
    let to_elem = format!("{to_q}[{element}]");

    // The source (a scalar variable, or the bare agg name pre-substitution)
    // is referenced bare in `to_elem_eqn_text`, so `RefShape::Bare` holds its
    // single occurrence live, `source_dim_elements` is empty (no source
    // subscripts to classify), and there is no iterated-dim context.
    let partial = build_partial_equation_shaped(
        to_elem_eqn_text,
        to_deps,
        &from_canonical,
        &RefShape::Bare,
        &[],
        None,
    );
    let partial = subscript_idents_at_element(&partial, to_deps_to_subscript, element);
    let source_ref = source_ref_override.unwrap_or(&from_q);
    link_score_guard_form(&partial, &to_elem, source_ref)
}

/// Generate the `agg → scalar-target` link-score equation: the partial of
/// `to`'s (scalar) equation w.r.t. the aggregate node `agg_name` held live,
/// everything else PREVIOUS. `to_eqn_text` is the target's equation text with
/// every hoisted reducer subexpression already substituted by its agg name
/// (so the agg appears where `SUM(...)` was); `to_deps` is the (over-
/// approximating is fine) dependency set of that substituted text. The result
/// is `Equation::Scalar`-shaped text, named `$⁚ltm⁚link_score⁚{agg}→{to}`.
///
/// For an *arrayed* target the per-target-element form is produced by
/// [`generate_scalar_to_element_equation`] instead (with `from = agg_name`).
pub(crate) fn generate_agg_to_scalar_target_equation(
    agg_name: &str,
    to_name: &str,
    to_eqn_text: &str,
    to_deps: &HashSet<Ident<Canonical>>,
) -> String {
    let agg_canonical = Ident::new(agg_name);
    let agg_q = quote_ident(agg_name);
    let to_q = quote_ident(to_name);
    // The agg node is a scalar -- referenced bare, no iterated-dim context.
    let partial = build_partial_equation_shaped(
        to_eqn_text,
        to_deps,
        &agg_canonical,
        &RefShape::Bare,
        &[],
        None,
    );
    link_score_guard_form(&partial, &to_q, &agg_q)
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
/// subexpression (`sum(p[*])` vs `sum(p[*] + 1)`) is never falsely matched. A
/// parse failure degrades to the input text unchanged.
pub(crate) fn substitute_reducers_in_equation(
    equation_text: &str,
    reducers: &HashMap<String, String>,
) -> String {
    if reducers.is_empty() {
        return equation_text.to_string();
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return equation_text.to_string();
    };
    print_eqn(&substitute_reducers_in_expr0(ast, reducers))
}

fn substitute_reducers_in_expr0(expr: Expr0, reducers: &HashMap<String, String>) -> Expr0 {
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

/// Quote an identifier for use in an equation string.
/// Identifiers with special characters (like $, ⁚) need double quotes.
pub(crate) fn quote_ident(ident: &str) -> String {
    if ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
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
/// - `Wildcard` / `DynamicIndex`: same as `Bare`. These shapes only reach
///   `emit_per_shape_link_scores` for the rare conservative-slice reducer
///   (`x[r] = ... + SUM(pop[NYC, *])`); the emitter dedups by the
///   resulting name, so the slot collapses onto the canonical Bare name
///   rather than minting a `⁚wildcard`/`⁚dynamic` variant. Full reducers
///   are hoisted into `$⁚ltm⁚agg⁚{n}` aggregate nodes and never reach
///   this function as a Wildcard/DynamicIndex shape.
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
/// Emits one `$⁚ltm⁚loop_score⁚{id}` synthetic aux per loop (product of
/// the loop's link scores).  Relative loop scores are no longer emitted
/// here: the per-partition `rel_loop_score` was O(P²) text per partition
/// and dominated compile memory on dense models (see
/// `docs/design-plans/2026-04-18-ltm-cap-lift-diagnosis.md`).  The
/// normalization now happens post-simulation in
/// [`crate::ltm_post::compute_rel_loop_scores`].  `partitions` is still
/// accepted so the signature matches the call site's precomputed data
/// and to keep the option open for future partition-aware emission
/// (e.g., a bounded per-partition denominator aux if we ever need one).
pub(crate) fn generate_loop_score_variables(
    loops: &[Loop],
    partitions: &CyclePartitions,
    emitted_link_score_names: &HashSet<String>,
) -> HashMap<Ident<Canonical>, datamodel::Variable> {
    let mut loop_vars = HashMap::new();

    // Tracing is opt-in via LTM_BENCH_TRACE=1.  When disabled, the only
    // per-iteration overhead is an integer add for the byte counter and
    // one branch-predictor-friendly zero-compare, so production cost is
    // negligible; when enabled the tracer logs every 10_000 loops so we
    // can slope-fit equation-text growth and correlate it with RSS.
    let trace_on = std::env::var("LTM_BENCH_TRACE").is_ok();
    let mut loop_score_bytes: u64 = 0;

    if trace_on {
        eprintln!(
            "[ltm-trace] generate_loop_score_variables start loops={} partitions={} \
             rss_mib={:.1}",
            loops.len(),
            partitions.partitions.len(),
            read_rss_mib().unwrap_or(0.0),
        );
    }

    for (i, loop_item) in loops.iter().enumerate() {
        let var_name = format!("$⁚ltm⁚loop_score⁚{}", loop_item.id);
        let equation = generate_loop_score_equation(loop_item, emitted_link_score_names);
        loop_score_bytes += equation.len() as u64;
        let ltm_var = create_aux_variable(&var_name, &equation);
        loop_vars.insert(Ident::new(&var_name), ltm_var);
        if trace_on && should_trace(i + 1) {
            eprintln!(
                "[ltm-trace] pass=loop_score i={} cum_loop_bytes={} rss_mib={:.1}",
                i + 1,
                loop_score_bytes,
                read_rss_mib().unwrap_or(0.0),
            );
        }
    }

    if trace_on {
        eprintln!(
            "[ltm-trace] generate_loop_score_variables done loops={} loop_bytes={} \
             rss_mib={:.1}",
            loops.len(),
            loop_score_bytes,
            read_rss_mib().unwrap_or(0.0),
        );
    }

    loop_vars
}

/// Decide whether iteration `n` (1-based) should emit a trace line.
///
/// We want early iterations densely (so we see the scaling curve
/// even if we OOM before completing the first 10_000 loops on a dense
/// partition) and later iterations sparsely (so we don't spam the log
/// for millions of loops).  Rule: log on every power of two up to and
/// including 8192, then every 10_000 after that.  Powers of two give
/// ~14 lines of early-curve data; 10_000 cadence gives steady-state
/// measurements during long runs.
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
/// `/proc/self/status` (e.g. non-Linux or wasm builds).  Used only by
/// the `LTM_BENCH_TRACE` instrumentation above, so an unavailable
/// reading degrades to a zero in the log rather than failing.
#[cfg(target_os = "linux")]
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

#[cfg(not(target_os = "linux"))]
fn read_rss_mib() -> Option<f64> {
    None
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
/// Returns a [`datamodel::Equation`] whose variant matches the *target*
/// variable's shape: `Equation::Scalar` for a scalar target,
/// `Equation::ApplyToAll(target_dims, _)` for an arrayed target (so the
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
/// Flow-to-stock links use a fixed structural formula and ignore `shape`,
/// `source_dim_elements`, and `dim_ctx`.
pub(crate) fn generate_link_score_equation_for_link(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Equation {
    generate_link_score_equation(
        from,
        to,
        shape,
        source_dim_elements,
        to_var,
        all_vars,
        dim_ctx,
    )
}

/// Generate the equation for a link score variable
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn generate_link_score_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    to_var: &Variable,
    all_vars: &HashMap<Ident<Canonical>, Variable>,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Equation {
    // Check if this is a flow-to-stock link
    let is_flow_to_stock = matches!(to_var, Variable::Stock { .. })
        && matches!(
            all_vars.get(from),
            Some(Variable::Var { is_flow: true, .. })
        );

    // Check if this is a stock-to-flow link
    let is_stock_to_flow = matches!(all_vars.get(from), Some(Variable::Stock { .. }))
        && matches!(to_var, Variable::Var { is_flow: true, .. });

    if is_flow_to_stock {
        // Flow-to-stock uses a fixed structural formula -- no AST parse,
        // so neither `shape` nor `source_dim_elements` matter here.
        generate_flow_to_stock_equation(from.as_str(), to.as_str(), to_var)
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
    let numerator = format!("(({partial_eq}) - PREVIOUS({target_ref}))");
    let target_diff = format!("({target_ref} - PREVIOUS({target_ref}))");
    let source_diff = format!("({source_ref} - PREVIOUS({source_ref}))");
    let abs_part = format!("ABS(SAFEDIV({numerator}, {target_diff}, 0))");
    let sign_part = format!("SIGN(SAFEDIV({numerator}, {source_diff}, 0))");
    format!(
        "if (TIME = INITIAL_TIME) then 0 \
         else if ({target_diff} = 0) OR ({source_diff} = 0) then 0 \
         else {abs_part} * {sign_part}"
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
fn link_score_equation_for_target(text: String, to_var: &Variable) -> Equation {
    match target_equation_dims(to_var) {
        Some(dims) => Equation::ApplyToAll(dims, text),
        None => Equation::Scalar(text),
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
/// whose `{partial}` is [`build_partial_equation_shaped`] applied to *that
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
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Equation {
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
        dim_ctx,
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
    let slot_equation = |expr: &crate::ast::Expr2| -> String {
        let elem_eqn_text = crate::patch::expr2_to_string(expr);
        // Per-element dependency set: walk *only this slot's* expression
        // (the union over all elements -- what `identifier_set` on the
        // whole `Ast::Arrayed` returns -- would over-freeze refs absent
        // from this slot). Pass the target's dimensions so literal
        // element-name subscripts of the *target*'s dims are filtered out;
        // strip the *source*'s dim/element names afterward (see above).
        let deps_e: HashSet<Ident<Canonical>> = crate::variable::classify_dependencies(
            &crate::ast::Ast::Scalar(expr.clone()),
            target_ast_dims,
            None,
        )
        .all
        .into_iter()
        .filter(|d| !source_dim_token_set.contains(d.as_str()))
        .collect();
        let (partial_e, live_ref) = build_partial_equation_shaped_with_live_ref(
            &elem_eqn_text,
            &deps_e,
            from,
            shape,
            source_dim_elements,
            Some(&iter_ctx),
        );
        let source_ref = source_ref_for_guard(from, shape, live_ref.as_ref());
        link_score_guard_form(&partial_e, target_ref, &source_ref)
    };

    // Sort the slots by element name so the resulting `Vec` -- which lands
    // in a salsa-tracked `LtmVariablesResult` -- is deterministic
    // regardless of `HashMap` iteration order across runs.
    let mut elements: Vec<(
        String,
        String,
        Option<String>,
        Option<datamodel::GraphicalFunction>,
    )> = per_elem
        .iter()
        .map(|(elem, expr)| (elem.as_str().to_string(), slot_equation(expr), None, None))
        .collect();
    elements.sort_by(|a, b| a.0.cmp(&b.0));

    let default_slot = default_expr.map(slot_equation);

    Equation::Arrayed(
        target_dim_names,
        elements,
        default_slot,
        apply_default_to_missing,
    )
}

/// Extract the equation text of a Scalar/ApplyToAll target's AST.
///
/// `Ast::Arrayed` targets are routed through
/// [`build_arrayed_link_score_equation`] before this is reached, so the
/// `Arrayed` AST arm here is dead in practice.
///
/// The `eqn`-text fallbacks (both the `Ast::Arrayed` arm and the no-AST
/// branch) cover the degenerate case where the target failed to lower --
/// `ast()` is `None`, or it's an `Ast::Arrayed` we didn't intercept --
/// but its datamodel `eqn` is still a plain scalar string. Returning that
/// raw text gives the link-score guard form *something* to differentiate,
/// which is strictly more useful than a `"0"` partial; the stock-to-flow
/// path has always done this for the same variable shape. A target with no
/// usable scalar equation at all (a stub, or an arrayed `eqn` we can't
/// flatten here) falls through to `"0"` -- the link score then degrades to
/// the historical placeholder rather than producing a parse error.
fn scalar_or_a2a_target_equation_text(target_var: &Variable) -> String {
    use crate::ast::Ast;
    if let Some(ast) = target_var.ast() {
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => crate::patch::expr2_to_string(expr),
            _ => scalar_eqn_text_or_zero(target_var),
        }
    } else {
        scalar_eqn_text_or_zero(target_var)
    }
}

/// The target's datamodel `eqn` text when it is a plain `Equation::Scalar`,
/// else `"0"`. See [`scalar_or_a2a_target_equation_text`] for why this
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
fn generate_auxiliary_to_auxiliary_equation(
    from: &Ident<Canonical>,
    to: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    to_var: &Variable,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Equation {
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
            dim_ctx,
        );
    }

    // Get the equation text of the 'to' variable.  Prefer the AST when
    // available because the `eqn` field holds the *original* text (e.g.,
    // "SMTH1(x, 5)") while the AST holds the post-module-expansion form
    // (e.g., Var("$⁚s⁚0⁚smth1·output")).  Using the AST-derived text
    // ensures the identifiers in the equation match those in `deps`.
    let to_equation = scalar_or_a2a_target_equation_text(to_var);

    // Get dependencies of the 'to' variable
    let deps = if let Some(ast) = to_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    // GH #511: an A2A target can reference `from` by one of the target's
    // iterated dimensions (`growth[Region,Age] = row_sum[Region] * c`).
    let target_iterated_dims = target_iterated_dim_names_canonical(to_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx,
    };
    let (partial_eq, live_ref) = build_partial_equation_shaped_with_live_ref(
        &to_equation,
        &deps,
        from,
        shape,
        source_dim_elements,
        Some(&iter_ctx),
    );

    let from_source_q = source_ref_for_guard(from, shape, live_ref.as_ref());

    let text = link_score_guard_form(&partial_eq, &to_q, &from_source_q);
    link_score_equation_for_target(text, to_var)
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
/// transform left no live reference (parse failure, or the source
/// vanished from the equation), fall back to `SUM(from)` -- still better
/// than a guaranteed dimension error.
fn source_ref_for_guard(
    from: &Ident<Canonical>,
    shape: &RefShape,
    live_ref: Option<&Expr0>,
) -> String {
    match shape {
        RefShape::Bare | RefShape::FixedIndex(_) => shape_aware_source_ref(from.as_str(), shape),
        RefShape::Wildcard | RefShape::DynamicIndex => match live_ref {
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
fn shape_aware_source_ref(from: &str, shape: &RefShape) -> String {
    match shape {
        RefShape::FixedIndex(elems) if !elems.is_empty() => {
            // Subscript syntax, NOT quote_ident: a literal `pop[nyc]`
            // parses as a Subscript node (per-element reference), while
            // `"pop[nyc]"` would parse as a quoted ident referring to
            // a synthetic variable that doesn't exist.
            format!("{}[{}]", quote_ident(from), elems.join(","))
        }
        _ => quote_ident(from),
    }
}

/// Generate flow-to-stock link score equation.
///
/// The structural inflow/outflow formula has no per-element equation
/// text -- the compiler applies it element-wise when the stock and flow
/// are arrayed -- so the result is `Equation::Scalar` for a scalar stock
/// and `Equation::ApplyToAll(stock_dims, _)` for an arrayed stock (the
/// shared formula evaluated per element).
fn generate_flow_to_stock_equation(flow: &str, stock: &str, stock_var: &Variable) -> Equation {
    // Check if this flow is an inflow or outflow
    let is_inflow = if let Variable::Stock { inflows, .. } = stock_var {
        inflows.iter().any(|f| f.as_str() == flow)
    } else {
        true // Default to inflow
    };

    let sign = if is_inflow { "" } else { "-" };

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
    let numerator = format!("(PREVIOUS({flow}) - PREVIOUS(PREVIOUS({flow})))");
    let denominator = format!(
        "(({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock}))))"
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
fn generate_stock_to_flow_equation(
    stock: &Ident<Canonical>,
    flow: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    flow_var: &Variable,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Equation {
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
            dim_ctx,
        );
    }

    // Get the flow equation text.  Prefer the AST when available because
    // it handles both Scalar and ApplyToAll (arrayed) equations, whereas
    // the raw `eqn` field only covers Scalar.  Without this, arrayed flows
    // fall through to "0" and produce a zero link score.
    let flow_equation = scalar_or_a2a_target_equation_text(flow_var);

    // Get dependencies of the flow variable
    let deps = if let Some(ast) = flow_var.ast() {
        identifier_set(ast, &[], None)
    } else {
        HashSet::new()
    };

    // GH #511: a flow can reference the stock by one of the flow's own
    // iterated dimensions, the same way an A2A aux can.
    let target_iterated_dims = target_iterated_dim_names_canonical(flow_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx,
    };
    let (partial_eq, live_ref) = build_partial_equation_shaped_with_live_ref(
        &flow_equation,
        &deps,
        stock,
        shape,
        source_dim_elements,
        Some(&iter_ctx),
    );

    // Link score formula from LTM paper: |Δxz/Δz| × sign(Δxz/Δx)
    // For stock-to-flow: x=stock, z=flow. The stock side respects
    // shape: a FixedIndex(elem) link score must normalize by
    // Δstock[elem], not the variable-level Δstock; a Wildcard /
    // DynamicIndex source slice is scalarized (`SUM(stock[PREVIOUS(idx)])`)
    // because bare arrayed `stock` in a scalar equation is a dimension
    // error -- see `source_ref_for_guard`.
    let stock_source_q = source_ref_for_guard(stock, shape, live_ref.as_ref());
    let text = link_score_guard_form(&partial_eq, target_ref, &stock_source_q);
    link_score_equation_for_target(text, flow_var)
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
/// If none of the candidates is in `emitted`, return the Bare canonical
/// name anyway and let the fragment compiler's stub-dep fallback fire.
/// That matches the pre-resolver behavior on the unreachable branch.
pub(crate) fn resolve_link_score_name_for_loop(
    from: &str,
    to: &str,
    emitted: &HashSet<String>,
    target_element: Option<&str>,
) -> String {
    if from.contains('[') {
        // Bracketed-from edge. Try the FixedIndex-style name (bracket
        // kept) first, then the variable-level Bare name that an A2A or
        // structural flow→stock link score would carry.
        let verbatim = link_score_var_name(from, to, &RefShape::Bare);
        if emitted.contains(&verbatim) {
            return verbatim;
        }
        let stripped = strip_subscript(from);
        let bare = link_score_var_name(stripped, to, &RefShape::Bare);
        if emitted.contains(&bare) {
            return bare;
        }
        return verbatim;
    }

    let bare = link_score_var_name(from, to, &RefShape::Bare);
    if emitted.contains(&bare) {
        return bare;
    }
    if let Some(fixed) = find_fixed_index_emitted_name(from, to, emitted, target_element) {
        return fixed;
    }
    bare
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
fn generate_loop_score_equation(
    loop_item: &Loop,
    emitted_link_score_names: &HashSet<String>,
) -> String {
    let link_score_names: Vec<String> = loop_item
        .links
        .iter()
        .map(|link| loop_link_score_ref(link, emitted_link_score_names))
        .collect();

    if link_score_names.is_empty() {
        "0".to_string()
    } else {
        link_score_names.join(" * ")
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
fn loop_link_score_ref(link: &crate::ltm::Link, emitted: &HashSet<String>) -> String {
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
            return format!("\"{per_elem}\"");
        }
    }

    let name = resolve_link_score_name_for_loop(
        link.from.as_str(),
        to_var_level,
        emitted,
        visited_element,
    );
    // Double-quote the variable name so it can be parsed. Case 2: a
    // cross-element loop edge visits a single element of a dimensioned A2A
    // link score, so subscript the reference at that element. Case 3: no
    // element to pin.
    match visited_element {
        Some(elem) => format!("\"{name}\"[{elem}]"),
        None => format!("\"{name}\""),
    }
}

/// Create an auxiliary variable with the given equation
fn create_aux_variable(name: &str, equation: &str) -> crate::datamodel::Variable {
    use crate::datamodel;

    datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(name).into_owned(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: "LTM".to_string(),
        units: None, // LTM scores are dimensionless by design, no need to declare
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
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

/// Examine the target variable's Expr2 AST to find the array-reducing function
/// applied to the source variable and classify it.
///
/// Walks the Expr2 tree looking for `Expr2::App(builtin, ...)` nodes where
/// the builtin is an array reducer and the argument references the source
/// variable (identified by canonical name). Returns the `ReducerKind`, the
/// uppercase function name (e.g., "SUM", "MIN"), and whether the reducer is
/// the top-level expression (`is_bare`).
///
/// When `is_bare` is false, the reducer is nested inside other arithmetic
/// (e.g., `2 * SUM(population[*])`). Callers should fall back to the
/// delta-ratio approach for nested reducers, because the algebraic shortcut
/// ignores the surrounding arithmetic and produces wrong link scores.
///
/// Returns `None` if no reducing builtin is found for the given source.
pub(crate) fn classify_reducer(
    target_var: &Variable,
    source_ident: &str,
) -> Option<(ReducerKind, &'static str, bool)> {
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
) -> Option<(ReducerKind, &'static str, bool)> {
    use crate::ast::Expr2;

    match expr {
        Expr2::App(builtin, _, _) => {
            // Check if this builtin is a reducer whose argument references
            // the source variable.
            if let Some((kind, name)) = classify_builtin_if_references_source(builtin, source_ident)
            {
                return Some((kind, name, is_top_level));
            }
            // Even if this particular App node isn't the reducer we want,
            // recurse into its arguments to find nested reducers.
            // Any reducer found inside a non-reducer App is nested.
            let mut result = None;
            builtin.for_each_expr_ref(|sub_expr| {
                if result.is_none() {
                    result = classify_reducer_in_expr(sub_expr, source_ident, false);
                }
            });
            result
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

/// If `builtin` is a recognized array reducer (per
/// [`crate::ltm_agg::reducer_kind`]) whose array argument references the source
/// variable, return its `(ReducerKind, uppercase function name)`.
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
) -> Option<(ReducerKind, &'static str)> {
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
    Some((kind, upper))
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

/// Generate a per-element link score equation for an arrayed-to-scalar edge.
///
/// For element `current_element` of source variable `source_var_name`,
/// produces the partial equation where ONLY `source[current_element]` varies
/// while all other elements are held at PREVIOUS values.
///
/// `reducer_kind` determines the generation strategy:
/// - `Linear`: algebraic shortcut (SUM/MEAN) avoids enumerating all elements
/// - `Nonlinear`: explicit element expansion with selective PREVIOUS wrapping
/// - `Constant`: caller should skip generation (SIZE always produces 0)
///
/// `reducer_name` is the uppercase function name ("MIN", "MAX", "STDDEV", "RANK")
/// used for nonlinear reducers when reconstructing the function call.
///
/// `is_bare` indicates whether the reducer is the entire target equation (true)
/// or is nested inside surrounding arithmetic like `2 * SUM(...)` (false).
/// When false, the algebraic shortcut would produce wrong link scores because
/// it ignores the surrounding arithmetic. In that case, the delta-ratio
/// fallback (using the target variable directly) is used instead.
pub(crate) fn generate_element_to_scalar_equation(
    source_var_name: &str,
    target_var_name: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
    is_bare: bool,
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
/// degenerate partial reduce with an empty result axis. STDDEV/RANK and
/// nested reducers fall back to the delta-ratio form against
/// `agg[result_element]`, unchanged from the scalar case (out of scope:
/// #483).
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
fn build_element_reducer_link_score(
    source_q: &str,
    target_ref: &str,
    current_element: &str,
    all_elements: &[String],
    reducer_kind: &ReducerKind,
    reducer_name: &str,
    is_bare: bool,
) -> String {
    let source_elem = format!("{source_q}[{current_element}]");

    let partial_eq = match reducer_kind {
        ReducerKind::Constant => {
            // SIZE is constant; caller should not generate link scores.
            // Return a zero equation as a defensive fallback.
            return "0".to_string();
        }
        _ if !is_bare => {
            // The reducer is nested inside surrounding arithmetic (e.g.,
            // `2 * SUM(population[*])` or `MAX(SUM(population[*]), 0)`).
            // The algebraic shortcut would ignore the surrounding expression
            // and produce wrong link scores. Fall back to the delta-ratio
            // approach: use the target variable directly, which measures the
            // ratio of actual target change to source element change. This is
            // approximate (like STDDEV/RANK) but avoids the wrong-multiplier
            // bug that the algebraic shortcut would introduce.
            target_ref.to_string()
        }
        ReducerKind::Linear => generate_linear_partial(
            source_q,
            target_ref,
            current_element,
            all_elements.len(),
            reducer_name,
        ),
        ReducerKind::Nonlinear => generate_nonlinear_partial(
            source_q,
            target_ref,
            current_element,
            all_elements,
            reducer_name,
        ),
    };

    // Standard link score formula wrapping the partial equation.
    let abs_part = format!(
        "ABS(SAFEDIV(({partial_eq} - PREVIOUS({target_ref})), ({target_ref} - PREVIOUS({target_ref})), 0))"
    );
    let sign_part = format!(
        "SIGN(SAFEDIV(({partial_eq} - PREVIOUS({target_ref})), ({source_elem} - PREVIOUS({source_elem})), 0))"
    );

    format!(
        "if \
            (TIME = INITIAL_TIME) \
            then 0 \
            else if \
                (({target_ref} - PREVIOUS({target_ref})) = 0) OR (({source_elem} - PREVIOUS({source_elem})) = 0) \
                then 0 \
                else {abs_part} * {sign_part}"
    )
}

/// Generate the partial evaluation for a linear reducer (SUM or MEAN).
///
/// SUM: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem]))
/// MEAN: PREVIOUS(target) + (source[elem] - PREVIOUS(source[elem])) / N
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

/// Generate the partial evaluation for a nonlinear reducer.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{CanonicalDimensionName, CanonicalElementName};
    use crate::dimensions::{Dimension, NamedDimension};

    fn make_named_dimension(name: &str, elements: &[&str]) -> Dimension {
        use std::collections::HashMap;
        let canonical_elements: Vec<CanonicalElementName> = elements
            .iter()
            .map(|e| CanonicalElementName::from_raw(e))
            .collect();
        let indexed: HashMap<CanonicalElementName, usize> = canonical_elements
            .iter()
            .enumerate()
            .map(|(i, e)| (e.clone(), i))
            .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                elements: canonical_elements,
                indexed_elements: indexed,
                maps_to: None,
                mappings: vec![],
            },
        )
    }

    fn make_indexed_dimension(name: &str, size: u32) -> Dimension {
        Dimension::Indexed(CanonicalDimensionName::from_raw(name), size)
    }

    /// Build a `HashSet<Ident<Canonical>>` from string slices for use in
    /// per-shape partial-equation tests. Each input string is canonicalized
    /// via `Ident::new`, matching the wrapping path that
    /// `build_partial_equation_shaped` exercises.
    fn deps_set(idents: &[&str]) -> HashSet<Ident<Canonical>> {
        idents.iter().map(|s| Ident::new(s)).collect()
    }

    /// Source-dimension element names for the per-shape partial-equation
    /// tests using a single `Region` dimension with elements `nyc` and
    /// `boston` (canonical lowercase form, in source-declared order).
    /// Used by `classify_expr0_subscript_shape` to validate that a literal
    /// subscript like `[NYC]` resolves to a known element.
    fn region_dim_elements() -> Vec<Vec<String>> {
        vec![vec!["nyc".to_string(), "boston".to_string()]]
    }

    /// Regression test for the integer-literal bounds asymmetry between
    /// the Expr0 and Expr2 classifiers. The Expr2 classifier
    /// (`db_analysis::resolve_literal_index`) validates integer
    /// literals against the indexed dimension's size and returns None
    /// (so the shape becomes `DynamicIndex`) for out-of-range values.
    /// The Expr0 classifier here previously accepted any `u32`-parseable
    /// `Const`, so `pop[999]` over an indexed dim of size 2 would
    /// classify as `FixedIndex(["999"])` here while the edge emitter
    /// classifies it as `DynamicIndex`. The shapes wouldn't match,
    /// the live reference would be wrapped in `PREVIOUS()`, and the
    /// link score would silently zero out.
    ///
    /// Both classifiers must agree -- so out-of-range integer literals
    /// classify as `DynamicIndex`.
    #[test]
    fn classify_expr0_rejects_out_of_range_integer_literal() {
        use crate::ast::{Expr0, IndexExpr0, Loc};

        // Indexed-style source_dim_elements: position 0 is an indexed
        // dim of size 2 (elements "1", "2"). "999" is out of range.
        let dims = vec![vec!["1".to_string(), "2".to_string()]];
        let indices = vec![IndexExpr0::Expr(Expr0::Const(
            "999".to_string(),
            999.0,
            Loc::default(),
        ))];

        let shape = classify_expr0_subscript_shape(&indices, &dims, None);
        assert_eq!(
            shape,
            RefShape::DynamicIndex,
            "out-of-range integer literal must classify as DynamicIndex \
             to agree with Expr2's resolve_literal_index; got {shape:?}",
        );

        // Same fixture: is_literal_element_index must also reject.
        assert!(
            !is_literal_element_index(&indices[0], 0, &dims),
            "is_literal_element_index must reject out-of-range integer literal",
        );

        // Sanity: the same classifier still accepts an in-range integer.
        let in_range = vec![IndexExpr0::Expr(Expr0::Const(
            "1".to_string(),
            1.0,
            Loc::default(),
        ))];
        let in_range_shape = classify_expr0_subscript_shape(&in_range, &dims, None);
        assert_eq!(
            in_range_shape,
            RefShape::FixedIndex(vec!["1".to_string()]),
            "in-range integer literal must classify as FixedIndex; got {in_range_shape:?}",
        );
    }

    /// Regression test: integer-literal subscripts must canonicalize to
    /// the engine's "1"-based string form before lookup, so `pop[01]`
    /// (zero-padded) classifies as `FixedIndex(["1"])` -- the same form
    /// `dimension_element_names` produces and the same form the Expr2
    /// edge emitter (`db_analysis::resolve_literal_index`) returns
    /// after this fix. Without canonicalization, `pop[01]` would be
    /// rejected as non-literal here (string "01" doesn't match "1" in
    /// `source_dim_elements`) while the Expr2 classifier accepted it
    /// at the original "01" text -- shapes disagree, the live ref gets
    /// wrapped in `PREVIOUS()`, and the link score silently zeros.
    #[test]
    fn classify_expr0_canonicalizes_integer_literal_subscript() {
        use crate::ast::{Expr0, IndexExpr0, Loc};

        // Indexed-style source_dim_elements: position 0 is an indexed
        // dim of size 5 (elements "1".."5").
        let dims = vec![
            vec!["1", "2", "3", "4", "5"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<String>>(),
        ];
        let indices = vec![IndexExpr0::Expr(Expr0::Const(
            "01".to_string(),
            1.0,
            Loc::default(),
        ))];

        let shape = classify_expr0_subscript_shape(&indices, &dims, None);
        assert_eq!(
            shape,
            RefShape::FixedIndex(vec!["1".to_string()]),
            "zero-padded integer literal must canonicalize to '1' so the \
             Expr0 and Expr2 classifiers agree; got {shape:?}",
        );

        assert!(
            is_literal_element_index(&indices[0], 0, &dims),
            "is_literal_element_index must accept canonicalized integer literal",
        );
    }

    // -- substitute_reducers_in_equation tests --

    /// Baseline: a reducer that is the whole equation is substituted by its
    /// agg name.
    #[test]
    fn substitute_reducers_whole_equation() {
        let mut reducers = HashMap::new();
        reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
        let out = substitute_reducers_in_equation("SUM(pop[*])", &reducers);
        assert_eq!(out, "\"$⁚ltm⁚agg⁚0\"");
    }

    /// Baseline: a reducer nested in an arithmetic subexpression is
    /// substituted; the surrounding structure is preserved.
    #[test]
    fn substitute_reducers_nested_in_arithmetic() {
        let mut reducers = HashMap::new();
        reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
        let out = substitute_reducers_in_equation("base / SUM(pop[*])", &reducers);
        assert_eq!(out, "base / \"$⁚ltm⁚agg⁚0\"");
    }

    /// Regression: a reducer used as a *subscript index expression*
    /// (`stock[SUM(idx[*])]`) is hoisted into a synthetic agg by
    /// `walk_subexpr_for_aggs` (which descends into `IndexExpr2::Expr`), so
    /// `substitute_reducers_in_expr0` must likewise descend into the
    /// `IndexExpr0::Expr` index of a `Subscript` and replace it -- otherwise
    /// the agg→target link-score equation for such a target would keep the
    /// reducer text live (no live `Var(agg)`), and the partial-equation
    /// builder would never PREVIOUS-wrap or hold-live the agg correctly.
    #[test]
    fn substitute_reducers_inside_subscript_index() {
        let mut reducers = HashMap::new();
        reducers.insert("sum(idx[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
        let out = substitute_reducers_in_equation("stock[SUM(idx[*])]", &reducers);
        assert_eq!(out, "stock[\"$⁚ltm⁚agg⁚0\"]");
    }

    /// Regression: a reducer used as one bound of a *range* subscript
    /// (`stock[1:SUM(idx[*])]`) is also reachable by the agg walker
    /// (`IndexExpr2::Range`), so the substituter must descend into both
    /// `IndexExpr0::Range` bounds.
    #[test]
    fn substitute_reducers_inside_subscript_range_bound() {
        let mut reducers = HashMap::new();
        reducers.insert("sum(idx[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
        let out = substitute_reducers_in_equation("stock[1:SUM(idx[*])]", &reducers);
        assert_eq!(out, "stock[1:\"$⁚ltm⁚agg⁚0\"]");
    }

    /// A reducer nested deep inside a subscript index expression (inside an
    /// arithmetic op that is itself the index) is still substituted.
    #[test]
    fn substitute_reducers_deep_inside_subscript_index() {
        let mut reducers = HashMap::new();
        reducers.insert("sum(idx[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
        let out = substitute_reducers_in_equation("stock[SUM(idx[*]) + 1]", &reducers);
        assert_eq!(out, "stock[\"$⁚ltm⁚agg⁚0\" + 1]");
    }

    /// Wildcard / star-range / dim-position subscript indices have no
    /// sub-expression to recurse into and must pass through untouched.
    #[test]
    fn substitute_reducers_leaves_wildcard_subscript_alone() {
        let mut reducers = HashMap::new();
        reducers.insert("sum(pop[*])".to_string(), "$⁚ltm⁚agg⁚0".to_string());
        let out = substitute_reducers_in_equation("pop[*]", &reducers);
        assert_eq!(out, "pop[*]");
    }

    // -- dimension_element_names tests --

    #[test]
    fn test_dimension_element_names_named() {
        let dim = make_named_dimension("Region", &["NYC", "Boston", "LA"]);
        let names = dimension_element_names(&dim);
        assert_eq!(names, vec!["nyc", "boston", "la"]);
    }

    #[test]
    fn test_dimension_element_names_indexed() {
        // Indexed dimensions use 1-based indexing to match the engine's
        // subscript formatting (see dimensions.rs SubscriptIterator).
        let dim = make_indexed_dimension("Index", 4);
        let names = dimension_element_names(&dim);
        assert_eq!(names, vec!["1", "2", "3", "4"]);
    }

    #[test]
    fn test_dimension_element_names_empty() {
        let dim = make_named_dimension("Empty", &[]);
        let names = dimension_element_names(&dim);
        assert!(names.is_empty());
    }

    #[test]
    fn test_dimension_element_names_indexed_zero() {
        let dim = make_indexed_dimension("Zero", 0);
        let names = dimension_element_names(&dim);
        assert!(names.is_empty());
    }

    // -- ReducerKind tests --

    #[test]
    fn test_reducer_kind_equality() {
        assert_eq!(ReducerKind::Linear, ReducerKind::Linear);
        assert_eq!(ReducerKind::Nonlinear, ReducerKind::Nonlinear);
        assert_eq!(ReducerKind::Constant, ReducerKind::Constant);
        assert_ne!(ReducerKind::Linear, ReducerKind::Nonlinear);
        assert_ne!(ReducerKind::Linear, ReducerKind::Constant);
        assert_ne!(ReducerKind::Nonlinear, ReducerKind::Constant);
    }

    #[test]
    fn test_reducer_kind_clone() {
        let kind = ReducerKind::Linear;
        let cloned = kind.clone();
        assert_eq!(kind, cloned);
    }

    // -- classify_reducer tests --

    use crate::ast::{Ast, Expr2, IndexExpr2};
    use crate::builtins::{BuiltinFn, Loc};

    /// Build a Variable::Var with a hand-built Expr2 AST.
    fn var_with_expr(expr: Expr2) -> Variable {
        Variable::Var {
            ident: Ident::new("target"),
            ast: Some(Ast::Scalar(expr)),
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        }
    }

    /// Build an Expr2 representing `var_name[*]` (subscript with wildcard).
    fn subscript_wildcard(var_name: &str) -> Expr2 {
        Expr2::Subscript(
            Ident::new(var_name),
            vec![IndexExpr2::Wildcard(Loc::default())],
            None,
            Loc::default(),
        )
    }

    /// Build an Expr2 representing a plain variable reference.
    fn var_ref(name: &str) -> Expr2 {
        Expr2::Var(Ident::new(name), None, Loc::default())
    }

    #[test]
    fn test_classify_reducer_sum() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", true)));
    }

    #[test]
    fn test_classify_reducer_mean() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Mean(vec![inner]), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "MEAN", true)));
    }

    #[test]
    fn test_classify_reducer_min() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Min(Box::new(inner), None), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "MIN", true)));
    }

    #[test]
    fn test_classify_reducer_max() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Max(Box::new(inner), None), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "MAX", true)));
    }

    #[test]
    fn test_classify_reducer_stddev() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Stddev(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "STDDEV", true)));
    }

    #[test]
    fn test_classify_reducer_rank() {
        let inner = subscript_wildcard("population");
        let direction = Expr2::Const("1".to_string(), 1.0, Loc::default());
        let expr = Expr2::App(
            BuiltinFn::Rank(Box::new(inner), Box::new(direction)),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Nonlinear, "RANK", true)));
    }

    #[test]
    fn test_classify_reducer_size() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Size(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Constant, "SIZE", true)));
    }

    #[test]
    fn test_classify_reducer_no_reducer() {
        // A plain addition: x + y
        let expr = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(var_ref("x")),
            Box::new(var_ref("y")),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "x");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_wrong_source() {
        let inner = subscript_wildcard("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        // Looking for a different source variable
        let result = classify_reducer(&var, "other_var");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_nested_in_expression() {
        // 2 * SUM(population[*]) + 1
        // Reducer is NOT at the top level, so is_bare should be false.
        let inner = subscript_wildcard("population");
        let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let two = Expr2::Const("2".to_string(), 2.0, Loc::default());
        let one = Expr2::Const("1".to_string(), 1.0, Loc::default());
        let mul = Expr2::Op2(
            crate::ast::BinaryOp::Mul,
            Box::new(two),
            Box::new(sum_expr),
            None,
            Loc::default(),
        );
        let expr = Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(mul),
            Box::new(one),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", false)));
    }

    #[test]
    fn test_classify_reducer_nested_in_scalar_max() {
        // MAX(SUM(population[*]), 0) -- scalar MAX wrapping array SUM
        // The SUM is nested inside a non-reducer App, so is_bare should be false.
        let inner = subscript_wildcard("population");
        let sum_expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let zero = Expr2::Const("0".to_string(), 0.0, Loc::default());
        let expr = Expr2::App(
            BuiltinFn::Max(Box::new(sum_expr), Some(Box::new(zero))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", false)));
    }

    #[test]
    fn test_classify_reducer_var_ref_no_subscript() {
        // SUM with a plain var reference (no subscript) should still match
        let inner = var_ref("population");
        let expr = Expr2::App(BuiltinFn::Sum(Box::new(inner)), None, Loc::default());
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, Some((ReducerKind::Linear, "SUM", true)));
    }

    #[test]
    fn test_classify_reducer_no_ast() {
        // Variable without an AST
        let var: Variable = Variable::Var {
            ident: Ident::new("target"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_two_arg_min_not_reducer() {
        // MIN(x, y) with two args is NOT an array reducer
        let inner1 = var_ref("population");
        let inner2 = var_ref("threshold");
        let expr = Expr2::App(
            BuiltinFn::Min(Box::new(inner1), Some(Box::new(inner2))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    #[test]
    fn test_classify_reducer_two_arg_max_not_reducer() {
        // MAX(x, y) with two args is NOT an array reducer
        let inner1 = var_ref("population");
        let inner2 = var_ref("threshold");
        let expr = Expr2::App(
            BuiltinFn::Max(Box::new(inner1), Some(Box::new(inner2))),
            None,
            Loc::default(),
        );
        let var = var_with_expr(expr);
        let result = classify_reducer(&var, "population");
        assert_eq!(result, None);
    }

    // -- generate_element_to_scalar_equation tests --

    #[test]
    fn test_generate_sum_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "total_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // Should contain the algebraic shortcut
        assert!(eq.contains("PREVIOUS(total_pop)"), "equation: {eq}");
        assert!(eq.contains("population[nyc]"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(population[nyc])"), "equation: {eq}");
        // Should not enumerate other elements (algebraic shortcut avoids them)
        assert!(
            !eq.contains("[boston]"),
            "equation should not enumerate boston: {eq}"
        );
        assert!(
            !eq.contains("[la]"),
            "equation should not enumerate la: {eq}"
        );
    }

    #[test]
    fn test_generate_mean_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "avg_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "MEAN",
            true,
        );
        // MEAN divides by N
        assert!(eq.contains("/ 3"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(avg_pop)"), "equation: {eq}");
    }

    #[test]
    fn test_generate_min_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "min_pop",
            "nyc",
            &elements,
            &ReducerKind::Nonlinear,
            "MIN",
            true,
        );
        // Should enumerate all elements with nested binary MIN calls
        assert!(eq.contains("population[nyc]"), "equation: {eq}");
        assert!(
            eq.contains("PREVIOUS(population[boston])"),
            "equation: {eq}"
        );
        assert!(eq.contains("PREVIOUS(population[la])"), "equation: {eq}");
        // Nested binary calls: MIN(a, MIN(b, c))
        assert!(
            eq.contains(
                "MIN(population[nyc], MIN(PREVIOUS(population[boston]), PREVIOUS(population[la])))"
            ),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_max_equation() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "max_pop",
            "boston",
            &elements,
            &ReducerKind::Nonlinear,
            "MAX",
            true,
        );
        // boston is the current element, so nyc and la are wrapped
        // Nested binary calls: MAX(a, MAX(b, c))
        assert!(
            eq.contains(
                "MAX(PREVIOUS(population[nyc]), MAX(population[boston], PREVIOUS(population[la])))"
            ),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_stddev_equation() {
        // STDDEV's per-element ceteris-paribus partial: the unrolled
        // population-variance `sqrt` formula holding `s[d1]` live and the
        // other elements frozen at PREVIOUS, matching the engine's
        // population-variance (divisor N) STDDEV. The exact-string
        // assertion pins precedence and spacing so regressions are caught
        // (mirrors `test_generate_full_reduce_unchanged_after_refactor`).
        let elements = vec!["d1".to_string(), "d2".to_string(), "d3".to_string()];
        let eq = generate_element_to_scalar_equation(
            "s",
            "total",
            "d1",
            &elements,
            &ReducerKind::Nonlinear,
            "STDDEV",
            true,
        );
        assert_eq!(
            eq,
            "if (TIME = INITIAL_TIME) then 0 else if ((total - PREVIOUS(total)) = 0) OR ((s[d1] - PREVIOUS(s[d1])) = 0) then 0 else ABS(SAFEDIV((sqrt((((s[d1] - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2) + ((PREVIOUS(s[d2]) - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2) + ((PREVIOUS(s[d3]) - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2)) / 3) - PREVIOUS(total)), (total - PREVIOUS(total)), 0)) * SIGN(SAFEDIV((sqrt((((s[d1] - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2) + ((PREVIOUS(s[d2]) - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2) + ((PREVIOUS(s[d3]) - ((s[d1] + PREVIOUS(s[d2]) + PREVIOUS(s[d3])) / 3))^2)) / 3) - PREVIOUS(total)), (s[d1] - PREVIOUS(s[d1])), 0))"
        );
        // The live source element drives the partial; the other elements
        // are frozen at PREVIOUS.
        assert!(eq.contains("sqrt("), "equation: {eq}");
        assert!(eq.contains("s[d1]"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(s[d2])"), "equation: {eq}");
        assert!(eq.contains("PREVIOUS(s[d3])"), "equation: {eq}");
        // Population variance squares deviations, never cubes.
        assert!(!eq.contains("^3"), "equation: {eq}");
    }

    #[test]
    fn test_generate_stddev_single_element_is_zero() {
        // The variance of a single element is identically 0, so the
        // partial is the literal `"0"` (mirrors MIN/MAX's `args.len() == 1`
        // special case -- avoids emitting `sqrt(((... - ...)^2) / 1)`).
        let elements = vec!["d1".to_string()];
        let partial = generate_nonlinear_partial("s", "total", "d1", &elements, "STDDEV");
        assert_eq!(partial, "0");
    }

    #[test]
    fn test_generate_rank_keeps_delta_ratio() {
        // RANK is an order statistic: non-differentiable, array-argument-only,
        // and unreachable via real models (RANK returns an array, so it
        // cannot be a scalar/A2A reducer RHS). The documented conservative
        // stand-in is the delta-ratio against the target -- i.e.
        // `generate_nonlinear_partial` returns just the target reference, so
        // the surrounding link-score formula degenerates to |Δtarget/Δtarget|.
        // Pinning this here makes RANK's treatment an explicit choice, not a
        // silent fallback.
        let elements = vec!["d1".to_string(), "d2".to_string(), "d3".to_string()];
        let partial = generate_nonlinear_partial("s", "total", "d1", &elements, "RANK");
        assert_eq!(partial, quote_ident("total"));
        assert!(!partial.contains("sqrt"), "partial: {partial}");
        assert!(!partial.contains("PREVIOUS("), "partial: {partial}");
    }

    #[test]
    fn test_generate_constant_returns_zero() {
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "size_pop",
            "nyc",
            &elements,
            &ReducerKind::Constant,
            "SIZE",
            true,
        );
        assert_eq!(eq, "0");
    }

    #[test]
    fn test_generate_nested_reducer_uses_delta_ratio() {
        // When the reducer is nested (is_bare=false), the equation should
        // fall back to the delta-ratio approach (using target directly)
        // instead of the algebraic shortcut.
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let eq = generate_element_to_scalar_equation(
            "population",
            "total_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            false, // nested reducer
        );
        // Should NOT use the algebraic shortcut (PREVIOUS(target) + delta)
        assert!(
            !eq.contains("PREVIOUS(total_pop) +"),
            "should not use algebraic shortcut for nested reducer: {eq}"
        );
        // Should still have the standard link score wrapping
        assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
        assert!(eq.contains("SAFEDIV("), "equation: {eq}");
        // The partial equation uses target directly (delta-ratio approach)
        assert!(
            eq.contains("(total_pop - PREVIOUS(total_pop))"),
            "should use target variable in delta-ratio: {eq}"
        );
    }

    #[test]
    fn test_generate_link_score_wrapping() {
        let elements = vec!["a".to_string(), "b".to_string()];
        let eq = generate_element_to_scalar_equation(
            "src",
            "tgt",
            "a",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // Should have initial time guard
        assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
        // Should have zero-change guards
        assert!(eq.contains("(tgt - PREVIOUS(tgt)) = 0"), "equation: {eq}");
        assert!(
            eq.contains("(src[a] - PREVIOUS(src[a])) = 0"),
            "equation: {eq}"
        );
        // Should have ABS and SIGN parts
        assert!(eq.contains("ABS(SAFEDIV("), "equation: {eq}");
        assert!(eq.contains("SIGN(SAFEDIV("), "equation: {eq}");
    }

    #[test]
    fn test_generate_special_chars_quoted() {
        let elements = vec!["nyc".to_string()];
        let eq = generate_element_to_scalar_equation(
            "$\u{205A}ltm\u{205A}var",
            "total",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // Source name with special chars should be quoted
        assert!(eq.contains("\"$\u{205A}ltm\u{205A}var\""), "equation: {eq}");
    }

    // -- generate_element_to_reduced_equation tests (partial reduce) --
    //
    // A partial reduce `agg[D1] = SUM(matrix[D1,*])` collapses only the
    // D2 axis: for source element `matrix[d1,d2]` the relevant target is
    // `agg[d1]`, and the ceteris-paribus partial holds the other
    // `matrix[d1,*]` elements (over the reduced axis D2) at PREVIOUS. The
    // target reference (`to_q`) and the source reference (`source_elem`)
    // must both be subscripted -- by the result-axis element on the
    // target side and by the full source tuple on the source side.

    #[test]
    fn test_generate_reduced_sum_equation() {
        // agg[D1] = SUM(matrix[D1,*]), D1 = {a, b}, D2 = {x, y}.
        // For matrix[a,x] -> agg[a], the partial is the SUM algebraic
        // shortcut with the target pinned to agg[a] and the source pinned
        // to matrix[a,x]; the other reduced-axis element (matrix[a,y])
        // must NOT appear (the shortcut avoids enumerating it).
        let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
        let eq = generate_element_to_reduced_equation(
            "matrix",
            "agg",
            "a,x",
            "a",
            &coreduced,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        assert!(
            eq.contains("PREVIOUS(agg[a]) + (matrix[a,x] - PREVIOUS(matrix[a,x]))"),
            "equation: {eq}"
        );
        // Target reference is subscripted by the result element.
        assert!(
            eq.contains("(agg[a] - PREVIOUS(agg[a])) = 0"),
            "equation: {eq}"
        );
        // Source reference is the full source tuple.
        assert!(
            eq.contains("(matrix[a,x] - PREVIOUS(matrix[a,x])) = 0"),
            "equation: {eq}"
        );
        // The other reduced-axis element must not be enumerated.
        assert!(
            !eq.contains("matrix[a,y]"),
            "SUM shortcut should not enumerate matrix[a,y]: {eq}"
        );
        // No literal "(0)" partial -- a real partial expression is emitted.
        assert!(eq.contains("ABS(SAFEDIV("), "equation: {eq}");
        assert!(eq.contains("SIGN(SAFEDIV("), "equation: {eq}");
    }

    #[test]
    fn test_generate_reduced_mean_equation() {
        // MEAN divides by the *reduced-axis* cardinality (|D2| = 2),
        // not by the total number of matrix elements.
        let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
        let eq = generate_element_to_reduced_equation(
            "matrix",
            "row_mean",
            "a,x",
            "a",
            &coreduced,
            &ReducerKind::Linear,
            "MEAN",
            true,
        );
        assert!(
            eq.contains("PREVIOUS(row_mean[a]) + (matrix[a,x] - PREVIOUS(matrix[a,x])) / 2"),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_reduced_min_equation() {
        // MIN over the reduced axis: nested binary MIN calls over the
        // matrix[a,*] elements (D2 = {x, y}), with matrix[a,x] live and
        // matrix[a,y] wrapped in PREVIOUS. Elements from other rows
        // (matrix[b,*]) must NOT appear.
        let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
        let eq = generate_element_to_reduced_equation(
            "matrix",
            "row_min",
            "a,x",
            "a",
            &coreduced,
            &ReducerKind::Nonlinear,
            "MIN",
            true,
        );
        assert!(
            eq.contains("MIN(matrix[a,x], PREVIOUS(matrix[a,y]))"),
            "equation: {eq}"
        );
        // The partial's target reference is the row element.
        assert!(eq.contains("PREVIOUS(row_min[a])"), "equation: {eq}");
        // Elements from other rows must not appear.
        assert!(!eq.contains("matrix[b"), "equation: {eq}");
    }

    #[test]
    fn test_generate_reduced_max_equation() {
        // The current element rides anywhere in the nesting; here it's
        // the first of the reduced-axis elements.
        let coreduced = vec!["b,x".to_string(), "b,y".to_string()];
        let eq = generate_element_to_reduced_equation(
            "matrix",
            "row_max",
            "b,y",
            "b",
            &coreduced,
            &ReducerKind::Nonlinear,
            "MAX",
            true,
        );
        assert!(
            eq.contains("MAX(PREVIOUS(matrix[b,x]), matrix[b,y])"),
            "equation: {eq}"
        );
    }

    #[test]
    fn test_generate_reduced_constant_returns_zero() {
        let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
        let eq = generate_element_to_reduced_equation(
            "matrix",
            "row_size",
            "a,x",
            "a",
            &coreduced,
            &ReducerKind::Constant,
            "SIZE",
            true,
        );
        assert_eq!(eq, "0");
    }

    #[test]
    fn test_generate_reduced_nested_uses_delta_ratio() {
        // A nested reducer (is_bare = false) falls back to the delta-ratio
        // form referencing the row element directly -- same as the scalar
        // case, just with the target subscripted.
        let coreduced = vec!["a,x".to_string(), "a,y".to_string()];
        let eq = generate_element_to_reduced_equation(
            "matrix",
            "row_agg",
            "a,x",
            "a",
            &coreduced,
            &ReducerKind::Linear,
            "SUM",
            false,
        );
        assert!(
            !eq.contains("PREVIOUS(row_agg[a]) +"),
            "should not use the algebraic shortcut for a nested reducer: {eq}"
        );
        assert!(
            eq.contains("(row_agg[a] - PREVIOUS(row_agg[a]))"),
            "should use the row element in the delta-ratio: {eq}"
        );
        assert!(eq.contains("TIME = INITIAL_TIME"), "equation: {eq}");
    }

    #[test]
    fn test_generate_full_reduce_unchanged_after_refactor() {
        // The full-reduce path must stay byte-identical after extracting
        // the shared body for the partial-reduce case.
        let elements = vec!["nyc".to_string(), "boston".to_string(), "la".to_string()];
        let scalar_eq = generate_element_to_scalar_equation(
            "population",
            "total_pop",
            "nyc",
            &elements,
            &ReducerKind::Linear,
            "SUM",
            true,
        );
        // A full reduce is the degenerate partial reduce where the result
        // axis is empty: passing an empty result element and the full
        // element list as the "coreduced" set must reproduce the scalar
        // equation, except the target reference picks up `[]` -- so we
        // don't claim equality here, only that the scalar path's text is
        // stable (the explicit-string assertion below catches regressions).
        assert_eq!(
            scalar_eq,
            "if (TIME = INITIAL_TIME) then 0 else if ((total_pop - PREVIOUS(total_pop)) = 0) OR ((population[nyc] - PREVIOUS(population[nyc])) = 0) then 0 else ABS(SAFEDIV((PREVIOUS(total_pop) + (population[nyc] - PREVIOUS(population[nyc])) - PREVIOUS(total_pop)), (total_pop - PREVIOUS(total_pop)), 0)) * SIGN(SAFEDIV((PREVIOUS(total_pop) + (population[nyc] - PREVIOUS(population[nyc])) - PREVIOUS(total_pop)), (population[nyc] - PREVIOUS(population[nyc])), 0))"
        );
    }

    // -- build_partial_equation_shaped: per-shape partial equation tests --
    //
    // Each test below pins the exact text that
    // `build_partial_equation_shaped` must return when handed a specific
    // `RefShape`. The expected strings were captured from `print_eqn` during
    // Task 0.5 reconnaissance and are already canonicalized: identifiers and
    // element names are lowercase (`print_ident` routes through
    // `canonicalize`), parsed function names are lowercase (the parser
    // lowercases function tokens at parse time, so `SUM` round-trips as
    // `sum`), synthesized `PREVIOUS` keeps uppercase (it's constructed as a
    // literal `"PREVIOUS"` `UntypedBuiltinFn`), binary operators get a
    // single space on each side, and parens are reintroduced for precedence.
    // Whitespace canonicalization happens entirely inside `print_eqn`, so the
    // assertions can use the literal expected string without any pre-trim.
    //
    // The Bare and Wildcard tests don't need `source_dim_elements` because
    // their classification doesn't depend on element-name lookups (Bare is a
    // top-level Var; Wildcard is detected from the `[*]` index alone). The
    // FixedIndex tests pass `region_dim_elements()` so
    // `classify_expr0_subscript_shape` can validate `[NYC]` and `[Boston]`
    // against the source's declared elements; otherwise both literal indices
    // would fall back to `DynamicIndex` and both subscripts would be wrapped.

    #[test]
    fn test_partial_equation_share_bare_shape() {
        // share[R] = population / SUM(population[*])
        // For the bare-Var reference (`population`), the bare ref stays live
        // and the wildcard reducer -- "other content" for this Bare link --
        // is wrapped in PREVIOUS() *as a whole*: `PREVIOUS(sum(population[*]))`,
        // which is PREVIOUS of the scalar total and evaluates fine. The
        // earlier form `sum(PREVIOUS(population[*]))` was the GH #517 bug --
        // identically `0.0` at every step under an active A2A dimension
        // because codegen has no LoadPrev-of-array-view path.
        let equation = "population / SUM(population[*])";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let partial =
            build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[], None);
        assert_eq!(partial, "population / PREVIOUS(sum(population[*]))");
    }

    /// GH #511: an iterated-dimension source subscript (`row_sum[D1]` inside
    /// an apply-to-all-over-`D1 x D2` equation) is normalized to bare
    /// `row_sum` in the partial -- either held live (`live_shape == Bare`)
    /// or `PREVIOUS(row_sum)` (a `Var` arg, which codegen accepts), never
    /// `PREVIOUS(row_sum[d1])` (a `PREVIOUS(Subscript(...))`, which trips
    /// the codegen assertion). The model equation `row_sum[D1] * c` is
    /// untouched -- only the LTM partial's `Expr0` is normalized.
    #[test]
    fn test_partial_equation_iterated_dim_source_normalized_to_bare() {
        let equation = "row_sum[D1] * c";
        let target_iterated_dims = vec!["d1".to_string(), "d2".to_string()];
        // `row_sum` is over `D1`; `c` is scalar.
        let source_dim_names = vec!["d1".to_string()];
        let iter_ctx = IteratedDimCtx {
            source_dim_names: &source_dim_names,
            target_iterated_dims: &target_iterated_dims,
            dim_ctx: None,
        };

        // `row_sum` is the live source (Bare): `row_sum[D1]` -> bare
        // `row_sum`, held live; `c` -> `PREVIOUS(c)`.
        let deps = deps_set(&["row_sum", "c"]);
        let live = Ident::<Canonical>::new("row_sum");
        let partial = build_partial_equation_shaped(
            equation,
            &deps,
            &live,
            &RefShape::Bare,
            // `source_dim_elements` is empty: `row_sum`'s single dimension is
            // identified by name via `iter_ctx`, not by element membership.
            &[],
            Some(&iter_ctx),
        );
        assert_eq!(
            partial, "row_sum * PREVIOUS(c)",
            "the iterated-dim source `row_sum[D1]` must be held live as bare `row_sum`"
        );

        // Now `c` is the live source: `row_sum[D1]` is a non-live dep ->
        // bare `row_sum` wrapped as `PREVIOUS(row_sum)`, NOT
        // `PREVIOUS(row_sum[d1])`.
        let live_c = Ident::<Canonical>::new("c");
        let partial_c = build_partial_equation_shaped(
            equation,
            &deps,
            &live_c,
            &RefShape::Bare,
            &[],
            Some(&iter_ctx),
        );
        assert!(
            partial_c.contains("PREVIOUS(row_sum)"),
            "the iterated-dim dep `row_sum[D1]` must be frozen as PREVIOUS(row_sum); got: {partial_c}"
        );
        assert!(
            !partial_c.contains("PREVIOUS(row_sum["),
            "must NOT produce PREVIOUS(row_sum[d1]) (a PREVIOUS-of-Subscript); got: {partial_c}"
        );
    }

    #[test]
    fn test_partial_equation_reducer_wrapped_whole_with_fixed_index_live() {
        // x[R] = pop[NYC] + SUM(pop[*]) -- the FixedIndex(nyc) link keeps
        // `pop[nyc]` live; the coexisting `SUM(pop[*])` is "other content"
        // and must be PREVIOUS-wrapped as a whole, not recursed into (GH
        // #517). `dims` lets `classify_expr0_subscript_shape` recognize
        // `[NYC]` as a literal element.
        let equation = "pop[NYC] + SUM(pop[*])";
        let deps = deps_set(&["pop"]);
        let source = Ident::<Canonical>::new("pop");
        let dims = vec![vec!["nyc".to_string(), "boston".to_string()]];
        let partial = build_partial_equation_shaped(
            equation,
            &deps,
            &source,
            &RefShape::FixedIndex(vec!["nyc".to_string()]),
            &dims,
            None,
        );
        assert_eq!(partial, "pop[nyc] + PREVIOUS(sum(pop[*]))");
    }

    #[test]
    fn test_partial_equation_two_reducers_both_wrapped_whole() {
        // y = SUM(a[*]) / SUM(b[*]) with `c` as the live source: neither
        // reducer carries the live ref, so both are PREVIOUS-wrapped whole
        // (GH #517). `c` does not appear, so nothing stays live -- the point
        // here is purely that the reducers don't get `sum(PREVIOUS(...))`.
        let equation = "(c + SUM(a[*])) / SUM(b[*])";
        let deps = deps_set(&["a", "b", "c"]);
        let source = Ident::<Canonical>::new("c");
        let partial =
            build_partial_equation_shaped(equation, &deps, &source, &RefShape::Bare, &[], None);
        assert_eq!(partial, "(c + PREVIOUS(sum(a[*]))) / PREVIOUS(sum(b[*]))");
    }

    #[test]
    fn test_partial_equation_wildcard_live_shape_holds_reducer_arg() {
        // A `RefShape::Wildcard` `live_shape` keeps the `population[*]`
        // reducer argument live and wraps every other reference in
        // PREVIOUS(). Full inlined reducers are hoisted into `$⁚ltm⁚agg⁚{n}`
        // nodes, so `build_partial_equation_shaped` only sees a Wildcard
        // `live_shape` for the conservative-slice case `SUM(pop[NYC, *])`
        // that `enumerate_agg_nodes` deliberately does not hoist; the
        // textbook full-reduce shape below pins the same wrapping rule that
        // case exercises.
        let equation = "population / SUM(population[*])";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let partial =
            build_partial_equation_shaped(equation, &deps, &source, &RefShape::Wildcard, &[], None);
        assert_eq!(partial, "PREVIOUS(population) / sum(population[*])");
    }

    #[test]
    fn test_partial_equation_migration_pressure_fixed_nyc() {
        // migration_pressure[NYC] = (population[NYC] - population[Boston]) * 0.01
        // For the FixedIndex(nyc) shape, the `population[nyc]` reference stays
        // live and `population[boston]` is wrapped in PREVIOUS(). Element names
        // in the FixedIndex variant are lowercase canonical form -- they must
        // match the AST subscript text, which `print_ident` lowercases via
        // `canonicalize`.
        let equation = "(population[NYC] - population[Boston]) * 0.01";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let dims = region_dim_elements();
        let partial = build_partial_equation_shaped(
            equation,
            &deps,
            &source,
            &RefShape::FixedIndex(vec!["nyc".to_string()]),
            &dims,
            None,
        );
        assert_eq!(
            partial,
            "(population[nyc] - PREVIOUS(population[boston])) * 0.01"
        );
    }

    #[test]
    fn test_partial_equation_migration_pressure_fixed_boston() {
        // Same equation text as the NYC case -- the per-shape builder works
        // per (reference-site, shape) pair, so the input equation is the
        // host expression and the `live_shape` selects which subscripted
        // population ref survives. Here `FixedIndex(boston)` keeps
        // `population[boston]` live and wraps `population[nyc]`.
        let equation = "(population[NYC] - population[Boston]) * 0.01";
        let deps = deps_set(&["population"]);
        let source = Ident::<Canonical>::new("population");
        let dims = region_dim_elements();
        let partial = build_partial_equation_shaped(
            equation,
            &deps,
            &source,
            &RefShape::FixedIndex(vec!["boston".to_string()]),
            &dims,
            None,
        );
        assert_eq!(
            partial,
            "(PREVIOUS(population[nyc]) - population[boston]) * 0.01"
        );
    }

    // -- AC2.4: other-source refs always wrapped, unknown idents passthrough --
    //
    // The two tests below pin behavior for references that aren't the live
    // source. The first verifies that another known dep is wrapped regardless
    // of which shape is live. The second verifies that an identifier that
    // doesn't appear in `deps` (e.g., a typo or unresolved external) passes
    // through unchanged -- the per-shape builder doesn't treat unknown idents
    // as wrap candidates because they could be function names or noise that
    // downstream parsing will diagnose separately.

    #[test]
    fn partial_equation_other_source_always_wrapped() {
        // Equation has a reference to `helper` (other dep) plus the live
        // source `pop`. The `helper` reference must be wrapped regardless
        // of `live_shape`; `pop` stays live because the shape is `Bare`.
        let deps = deps_set(&["pop", "helper"]);
        let live = Ident::<Canonical>::new("pop");
        let shape = RefShape::Bare;
        let dims = region_dim_elements();

        let partial =
            build_partial_equation_shaped("pop * helper", &deps, &live, &shape, &dims, None);
        assert!(partial.contains("PREVIOUS(helper)"), "partial: {partial}");
        assert!(!partial.contains("PREVIOUS(pop)"), "partial: {partial}");
    }

    #[test]
    fn partial_equation_unknown_ident_unchanged() {
        // A reference to a variable not in `deps` (e.g., a typo or external)
        // is left alone -- it's not a known dep and shouldn't be wrapped.
        let deps = deps_set(&["pop"]);
        let live = Ident::<Canonical>::new("pop");
        let shape = RefShape::Bare;
        let dims = region_dim_elements();

        let partial =
            build_partial_equation_shaped("pop + unknown", &deps, &live, &shape, &dims, None);
        assert!(partial.contains("unknown"), "partial: {partial}");
        assert!(!partial.contains("PREVIOUS(unknown)"), "partial: {partial}");
    }

    // -- link_score_var_name: per-shape naming convention --
    //
    // The naming helper produces a stable name for each `(from, to, shape)`
    // tuple regardless of which other shapes coexist in the same model.
    // Bare uses the legacy canonical form; FixedIndex prefixes the source
    // with the bracketed element name(s); Wildcard and DynamicIndex always
    // append a stable suffix on the target side. The discovery parser
    // (Phase 3 Task 7) strips the suffix before looking up offsets.

    #[test]
    fn link_score_name_bare_canonical() {
        assert_eq!(
            link_score_var_name("pop", "births", &RefShape::Bare),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births"
        );
    }

    #[test]
    fn link_score_name_fixed_index() {
        let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
        assert_eq!(
            link_score_var_name("pop", "rel_pop", &shape),
            "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop"
        );
    }

    #[test]
    fn link_score_name_wildcard_dynamic_collapse_to_bare() {
        // The `⁚wildcard` / `⁚dynamic` per-shape suffix was retired:
        // a maximal inlined reducer is hoisted into a `$⁚ltm⁚agg⁚{n}`
        // node, and the rare conservative-slice reducer collapses onto
        // the canonical Bare name (the emitter dedups by resulting name).
        let bare = link_score_var_name("pop", "share", &RefShape::Bare);
        assert_eq!(
            link_score_var_name("pop", "share", &RefShape::Wildcard),
            bare
        );
        assert_eq!(
            link_score_var_name("pop", "share", &RefShape::DynamicIndex),
            bare
        );
    }

    // -- generate_loop_score_equation: per-element link names --
    //
    // The per-element distinction lives in `link.from` itself (e.g.,
    // `"pop[nyc]"` for cross-dimensional edges in mixed/scalar loops).
    // generate_loop_score_equation uses Bare naming uniformly, so the
    // bracketed `from` flows through verbatim and the resulting
    // reference matches the per-element link score that
    // try_cross_dimensional_link_scores emits.
    #[test]
    fn loop_score_equation_uses_element_level_from_for_per_element_links() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "r1".to_string(),
            links: vec![
                Link {
                    from: Ident::<Canonical>::new("pop[nyc]"),
                    to: Ident::<Canonical>::new("rel_pop"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::<Canonical>::new("rel_pop"),
                    to: Ident::<Canonical>::new("pop"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
            dimensions: vec![],
        };

        // Pretend both candidates were emitted as Bare; the resolver
        // will pick the canonical form via Bare naming, so the
        // bracketed from flows through verbatim.
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop".to_string());
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}rel_pop\u{2192}pop".to_string());
        let eq = generate_loop_score_equation(&loop_item, &emitted);

        // Element-level from flows through Bare naming verbatim.
        assert!(
            eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}rel_pop\""),
            "expected per-element link-score reference; got: {eq}"
        );
        // The closing link uses canonical Bare naming.
        assert!(
            eq.contains("\"$\u{205A}ltm\u{205A}link_score\u{205A}rel_pop\u{2192}pop\""),
            "expected Bare link-score reference (closing link); got: {eq}"
        );
        // Loop score is the product of the two references.
        assert!(eq.contains(" * "), "expected product join; got: {eq}");
    }

    /// Regression test: when only a `FixedIndex` variant is in `emitted`
    /// (e.g., `share[r] = pop[NYC]` -- only `pop[nyc]→share` is emitted),
    /// the resolver must pick that variant rather than fall back to the
    /// never-emitted Bare canonical name.
    #[test]
    fn resolver_picks_fixed_index_when_bare_not_emitted() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());

        let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted, None);
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share",
            "resolver should pick the FixedIndex variant when Bare is not emitted",
        );
    }

    /// Regression test: when multiple FixedIndex variants exist (e.g.,
    /// `share[r] = pop[NYC] + pop[BOSTON]`), the resolver picks
    /// deterministically (lexicographically first). This documents the
    /// edge-aliasing limitation: only one variant contributes to the
    /// loop score.
    #[test]
    fn resolver_picks_fixed_index_deterministically_with_multiple_variants() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());
        emitted
            .insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}share".to_string());

        let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted, None);
        // Lexicographic sort: "pop[boston]→share" < "pop[nyc]→share".
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[boston]\u{2192}share",
            "resolver should pick the lexicographically first FixedIndex variant",
        );
    }

    /// Regression test: Bare must win when both a Bare and a FixedIndex
    /// per-element link score exist for the same `(from, to)` edge -- the
    /// documented Bare-beats-FixedIndex edge-aliasing tie-break.
    #[test]
    fn resolver_prefers_bare_over_fixed_index() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share".to_string());
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}share".to_string());

        let chosen = resolve_link_score_name_for_loop("pop", "share", &emitted, None);
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}share",
            "Bare must win when present, regardless of any FixedIndex variant",
        );
    }

    /// Regression test: bracketed `from` (cross-dimensional case) flows
    /// through Bare naming verbatim and must resolve to the matching
    /// per-element name emitted by `try_cross_dimensional_link_scores`.
    #[test]
    fn resolver_resolves_cross_dim_bracketed_from() {
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total".to_string());

        let chosen = resolve_link_score_name_for_loop("pop[nyc]", "total", &emitted, None);
        assert_eq!(
            chosen, "$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc]\u{2192}total",
            "bracketed from + Bare should match the emitted per-element name",
        );
    }

    // -- Task 1 (ltm-503-cross-element-agg Phase 2): target_element-aware
    //    loop-score link-score reference resolution --

    /// Regression guard: `generate_loop_score_equation` is byte-identical
    /// to the pre-Phase-2 behavior when no `Link.to` carries an element
    /// subscript (which is the case for every pure-scalar / pure-A2A /
    /// mixed loop the loop builder produces today, and stays the case for
    /// pure-A2A loops after the cross-element rewrite).
    #[test]
    fn loop_score_equation_unsubscripted_to_unchanged() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "r1".to_string(),
            links: vec![
                Link {
                    from: Ident::<Canonical>::new("pop"),
                    to: Ident::<Canonical>::new("births"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::<Canonical>::new("births"),
                    to: Ident::<Canonical>::new("pop"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![],
            polarity: LoopPolarity::Reinforcing,
            dimensions: vec![],
        };
        let mut emitted = HashSet::new();
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births".to_string());
        emitted.insert("$\u{205A}ltm\u{205A}link_score\u{205A}births\u{2192}pop".to_string());

        let eq = generate_loop_score_equation(&loop_item, &emitted);
        assert_eq!(
            eq,
            "\"$\u{205A}ltm\u{205A}link_score\u{205A}pop\u{2192}births\" * \
             \"$\u{205A}ltm\u{205A}link_score\u{205A}births\u{2192}pop\"",
            "unsubscripted loop-score equation must be byte-identical to pre-Phase-2 output",
        );
    }

    /// When `target_element = None` the resolver is unchanged: it picks
    /// the lexicographically-first FixedIndex variant.
    #[test]
    fn resolver_fixed_index_no_target_element_unchanged() {
        let mut emitted = HashSet::new();
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
                .to_string(),
        );
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure"
                .to_string(),
        );

        let chosen =
            resolve_link_score_name_for_loop("population", "migration_pressure", &emitted, None);
        // Lexicographic: "population[boston]..." < "population[nyc]...".
        assert_eq!(
            chosen,
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure",
            "with target_element=None the resolver keeps the alphabetical heuristic",
        );
    }

    /// `target_element = Some(e)` makes the resolver prefer the FixedIndex
    /// variant whose source element matches `e` (an exact match), rather
    /// than guessing alphabetically.
    #[test]
    fn resolver_fixed_index_target_element_exact_match() {
        let mut emitted = HashSet::new();
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
                .to_string(),
        );
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[boston]\u{2192}migration_pressure"
                .to_string(),
        );

        let chosen = resolve_link_score_name_for_loop(
            "population",
            "migration_pressure",
            &emitted,
            Some("nyc"),
        );
        assert_eq!(
            chosen,
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure",
            "target_element=Some(\"nyc\") should select the nyc-source FixedIndex variant",
        );
    }

    /// A cross-element loop edge `population[nyc] -> migration_pressure[boston]`
    /// where the emitted A2A link score is the per-source-element FixedIndex
    /// form `population[nyc]->migration_pressure` (dimensioned over Region):
    /// the loop-score equation references it subscripted at the visited
    /// target element -- `"$⁚ltm⁚link_score⁚population[nyc]→migration_pressure"[boston]`.
    #[test]
    fn loop_score_equation_subscripts_a2a_fixed_index_link_at_visited_element() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "u1".to_string(),
            links: vec![Link {
                from: Ident::<Canonical>::new("population[nyc]"),
                to: Ident::<Canonical>::new("migration_pressure[boston]"),
                polarity: LinkPolarity::Positive,
            }],
            stocks: vec![],
            polarity: LoopPolarity::Undetermined,
            dimensions: vec![],
        };
        let mut emitted = HashSet::new();
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
                .to_string(),
        );

        let eq = generate_loop_score_equation(&loop_item, &emitted);
        assert_eq!(
            eq,
            "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston]",
            "A2A FixedIndex link score visited at element 'boston' must be subscripted [boston]",
        );
    }

    /// A cross-element loop edge `migration_in[nyc] -> population[nyc]` (a
    /// structural flow->stock edge): the emitted link score uses the
    /// *variable-level* `from` (`migration_in->population`, dimensioned
    /// over Region), so the resolver must strip the subscript off
    /// `Link.from` to find it, and the loop-score equation subscripts the
    /// reference at the visited element -- `"$⁚ltm⁚link_score⁚migration_in→population"[nyc]`.
    #[test]
    fn loop_score_equation_strips_from_for_variable_level_a2a_link() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "u1".to_string(),
            links: vec![Link {
                from: Ident::<Canonical>::new("migration_in[nyc]"),
                to: Ident::<Canonical>::new("population[nyc]"),
                polarity: LinkPolarity::Positive,
            }],
            stocks: vec![Ident::<Canonical>::new("population[nyc]")],
            polarity: LoopPolarity::Undetermined,
            dimensions: vec![],
        };
        let mut emitted = HashSet::new();
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population".to_string(),
        );

        let eq = generate_loop_score_equation(&loop_item, &emitted);
        assert_eq!(
            eq, "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]",
            "variable-level-from A2A link score visited at 'nyc' must resolve via stripped-from \
             and be subscripted [nyc]",
        );
    }

    /// Full cross-element migration loop: three edges, three subscripted
    /// references, all distinct A2A link scores, joined by ` * `.
    #[test]
    fn loop_score_equation_cross_element_migration_loop_full() {
        use crate::ltm::{Link, LinkPolarity, Loop, LoopPolarity};

        let loop_item = Loop {
            id: "u1".to_string(),
            links: vec![
                Link {
                    from: Ident::<Canonical>::new("population[nyc]"),
                    to: Ident::<Canonical>::new("migration_pressure[boston]"),
                    polarity: LinkPolarity::Positive,
                },
                Link {
                    from: Ident::<Canonical>::new("migration_pressure[boston]"),
                    to: Ident::<Canonical>::new("migration_in[nyc]"),
                    polarity: LinkPolarity::Negative,
                },
                Link {
                    from: Ident::<Canonical>::new("migration_in[nyc]"),
                    to: Ident::<Canonical>::new("population[nyc]"),
                    polarity: LinkPolarity::Positive,
                },
            ],
            stocks: vec![Ident::<Canonical>::new("population[nyc]")],
            polarity: LoopPolarity::Undetermined,
            dimensions: vec![],
        };
        let mut emitted = HashSet::new();
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure"
                .to_string(),
        );
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in"
                .to_string(),
        );
        emitted.insert(
            "$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population".to_string(),
        );

        let eq = generate_loop_score_equation(&loop_item, &emitted);
        assert_eq!(
            eq,
            "\"$\u{205A}ltm\u{205A}link_score\u{205A}population[nyc]\u{2192}migration_pressure\"[boston] * \
             \"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure[boston]\u{2192}migration_in\"[nyc] * \
             \"$\u{205A}ltm\u{205A}link_score\u{205A}migration_in\u{2192}population\"[nyc]",
            "cross-element migration loop score must walk the element-level path",
        );
        // It must NOT reference the unsubscripted A2A diagonal names where
        // the loop visits a specific element.
        assert!(
            !eq.contains(
                "\"$\u{205A}ltm\u{205A}link_score\u{205A}migration_pressure\u{2192}migration_out\""
            ),
            "must not reference the diagonal migration_out link score; got: {eq}",
        );
    }

    /// Regression test: a `DynamicIndex` live reference must still wrap
    /// inner index expressions that reference other deps. The buggy
    /// version skipped recursion for ANY live-shape match, which is
    /// correct for `FixedIndex` (literal element indices are dimension
    /// references, not deps) but wrong for `DynamicIndex` (the index is
    /// an expression that may reference deps which must be held at
    /// PREVIOUS for ceteris-paribus).
    #[test]
    fn partial_equation_dynamic_index_wraps_inner_deps() {
        // arr[idx + helper] with live_source=arr, live_shape=DynamicIndex.
        // The OUTER subscript is the live reference; idx and helper
        // inside the index expression are other deps and must be wrapped
        // in PREVIOUS for ceteris-paribus.
        let dims: Vec<Vec<String>> = vec![];
        let deps = deps_set(&["arr", "idx", "helper"]);
        let live = Ident::new("arr");
        let shape = RefShape::DynamicIndex;

        let partial =
            build_partial_equation_shaped("arr[idx + helper]", &deps, &live, &shape, &dims, None);

        assert!(
            partial.contains("PREVIOUS(idx)"),
            "idx must be wrapped in PREVIOUS for ceteris-paribus; got: {partial}",
        );
        assert!(
            partial.contains("PREVIOUS(helper)"),
            "helper must be wrapped in PREVIOUS for ceteris-paribus; got: {partial}",
        );
        // The outer arr[...] reference must stay live (no PREVIOUS wrap
        // around the whole subscript).
        assert!(
            !partial.contains("PREVIOUS(arr["),
            "live arr ref must not be wrapped; got: {partial}",
        );
    }

    /// Regression test: a literal-element subscript like `pop[NYC]` must
    /// classify as `FixedIndex(["nyc"])` even when a user variable named
    /// `nyc` exists and is in `other_deps`. The buggy implementation
    /// recursed into the indices first (wrapping `Var(NYC)` as
    /// `App(PREVIOUS, [Var(NYC)])`) and then classified the transformed
    /// indices, which fell through to `DynamicIndex` and broke the live
    /// FixedIndex match -- so the live reference got wrapped too.
    #[test]
    fn partial_equation_dimension_element_collides_with_variable_name() {
        let dims = region_dim_elements();
        // Both `pop` (live source) and `nyc` (user variable) are deps.
        // The literal subscript [NYC] must still classify as a dimension
        // element, not a wrapped variable reference.
        let deps = deps_set(&["pop", "nyc"]);
        let live = Ident::new("pop");
        let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);

        let partial = build_partial_equation_shaped("pop[NYC]", &deps, &live, &shape, &dims, None);

        // The live reference must remain unwrapped.
        assert!(
            !partial.contains("PREVIOUS(pop"),
            "live FixedIndex reference unexpectedly wrapped; got: {partial}",
        );
        // The literal element subscript must remain unwrapped (NYC is a
        // dimension element here, not a runtime variable reference).
        assert!(
            !partial.contains("PREVIOUS(nyc)"),
            "literal element subscript wrongly treated as variable; got: {partial}",
        );
    }

    // -- Arrayed-target link scores: per-element partial equations --
    //
    // For a per-element-equation (`Ast::Arrayed`) target, the link score
    // must be an `Equation::Arrayed` whose per-element slot equation is the
    // standard link-score guard form built around *that element's own*
    // partial equation -- not a `"0"` placeholder. The tests below build a
    // 2-region per-element-equation aux (`migration_pressure`) and verify
    // that the `population -> migration_pressure` link-score equation for
    // each `FixedIndex` shape carries the right partial in every slot.

    /// Build a stage-1 `Variable` (lowered, `Expr2`) for a per-element-
    /// equation (`Equation::Arrayed`) variable from raw element equation
    /// text. Routes through the same `datamodel::Variable` -> parse -> lower
    /// path production uses, so the result carries both `ast: Some(Ast::Arrayed)`
    /// and `eqn: Some(Equation::Arrayed)`.
    fn arrayed_var_from_text(
        ident: &str,
        dims: &[crate::datamodel::Dimension],
        elements: &[(&str, &str)],
        is_flow: bool,
    ) -> Variable {
        use crate::datamodel::{Aux, Equation as DmEquation, Flow, Variable as DmVariable};

        let equation = DmEquation::Arrayed(
            dims.iter().map(|d| d.name().to_string()).collect(),
            elements
                .iter()
                .map(|(e, eq)| ((*e).to_string(), (*eq).to_string(), None, None))
                .collect(),
            None,
            false,
        );
        let dm_var = if is_flow {
            DmVariable::Flow(Flow {
                ident: ident.to_string(),
                equation,
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: crate::datamodel::Compat::default(),
            })
        } else {
            DmVariable::Aux(Aux {
                ident: ident.to_string(),
                equation,
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: crate::datamodel::Compat::default(),
            })
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let mut implicit_vars = Vec::new();
        let stage0 = crate::variable::parse_var::<crate::datamodel::ModuleReference, _>(
            dims,
            &dm_var,
            &mut implicit_vars,
            &units_ctx,
            |mi| Ok(Some(mi.clone())),
        );
        let dim_ctx = crate::dimensions::DimensionsContext::from(dims);
        let models = HashMap::new();
        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: &dim_ctx,
            model_name: "test",
        };
        crate::model::lower_variable(&scope, &stage0)
    }

    /// Look up the slot equation for `element` in an `Equation::Arrayed`,
    /// failing the test loudly if the equation isn't `Arrayed` or the slot
    /// is missing.
    fn arrayed_slot<'a>(equation: &'a Equation, element: &str) -> &'a str {
        match equation {
            Equation::Arrayed(_, elements, _, _) => elements
                .iter()
                .find(|(e, _, _, _)| e == element)
                .map(|(_, eqn, _, _)| eqn.as_str())
                .unwrap_or_else(|| {
                    panic!("no slot for element {element:?} in arrayed equation: {equation:?}")
                }),
            other => panic!("expected Equation::Arrayed, got: {other:?}"),
        }
    }

    fn region_dm_dimension() -> crate::datamodel::Dimension {
        crate::datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string()],
        )
    }

    #[test]
    fn test_arrayed_link_score_population_to_migration_pressure_fixed_nyc() {
        // ltm-503-cross-element-agg.AC1.1
        // migration_pressure is a per-element-equation aux:
        //   migration_pressure[NYC]    = (population[NYC] - population[Boston]) * 0.01
        //   migration_pressure[Boston] = (population[Boston] - population[NYC]) * 0.01
        // For the `population -> migration_pressure` link with shape
        // FixedIndex(["nyc"]), the `population[nyc]` ref stays live in every
        // slot and the other-element refs are frozen at PREVIOUS().
        let dims = vec![region_dm_dimension()];
        let to_var = arrayed_var_from_text(
            "migration_pressure",
            &dims,
            &[
                ("NYC", "(population[NYC] - population[Boston]) * 0.01"),
                ("Boston", "(population[Boston] - population[NYC]) * 0.01"),
            ],
            false,
        );

        let from = Ident::<Canonical>::new("population");
        let to = Ident::<Canonical>::new("migration_pressure");
        let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
        let source_dim_elements = region_dim_elements();

        let equation = generate_auxiliary_to_auxiliary_equation(
            &from,
            &to,
            &shape,
            &source_dim_elements,
            &[],
            &to_var,
            None,
        );

        match &equation {
            Equation::Arrayed(eq_dims, _, default, _) => {
                assert_eq!(eq_dims, &["Region".to_string()]);
                assert!(default.is_none(), "no EXCEPT default expected");
            }
            other => panic!("expected Equation::Arrayed, got: {other:?}"),
        }

        let nyc_slot = arrayed_slot(&equation, "nyc");
        let boston_slot = arrayed_slot(&equation, "boston");

        // No slot may carry the `(0)` placeholder partial that the pre-fix
        // `_ => "0"` fall-through produced.
        assert!(
            !nyc_slot.contains("((0) -"),
            "nyc slot must not use a '0' partial; got: {nyc_slot}"
        );
        assert!(
            !boston_slot.contains("((0) -"),
            "boston slot must not use a '0' partial; got: {boston_slot}"
        );

        // The `{partial}` substring is the canonical per-element equation
        // with the live-shape ref kept live and the rest frozen.
        assert!(
            nyc_slot.contains("(population[nyc] - PREVIOUS(population[boston])) * 0.01"),
            "nyc slot partial mismatch; got: {nyc_slot}"
        );
        assert!(
            boston_slot.contains("(PREVIOUS(population[boston]) - population[nyc]) * 0.01"),
            "boston slot partial mismatch; got: {boston_slot}"
        );

        // The guard form references the target element-wise (bare name) and
        // the shape-aware source subscript.
        assert!(
            nyc_slot.contains("(migration_pressure - PREVIOUS(migration_pressure))"),
            "nyc slot target ref mismatch; got: {nyc_slot}"
        );
        assert!(
            nyc_slot.contains("(population[nyc] - PREVIOUS(population[nyc]))"),
            "nyc slot source ref mismatch; got: {nyc_slot}"
        );
    }

    #[test]
    fn test_arrayed_link_score_population_to_migration_pressure_fixed_boston() {
        // ltm-503-cross-element-agg.AC1.2
        // Same model; shape FixedIndex(["boston"]) keeps `population[boston]`
        // live and freezes `population[nyc]`.
        let dims = vec![region_dm_dimension()];
        let to_var = arrayed_var_from_text(
            "migration_pressure",
            &dims,
            &[
                ("NYC", "(population[NYC] - population[Boston]) * 0.01"),
                ("Boston", "(population[Boston] - population[NYC]) * 0.01"),
            ],
            false,
        );

        let from = Ident::<Canonical>::new("population");
        let to = Ident::<Canonical>::new("migration_pressure");
        let shape = RefShape::FixedIndex(vec!["boston".to_string()]);
        let source_dim_elements = region_dim_elements();

        let equation = generate_auxiliary_to_auxiliary_equation(
            &from,
            &to,
            &shape,
            &source_dim_elements,
            &[],
            &to_var,
            None,
        );

        let nyc_slot = arrayed_slot(&equation, "nyc");
        let boston_slot = arrayed_slot(&equation, "boston");

        assert!(
            !nyc_slot.contains("((0) -") && !boston_slot.contains("((0) -"),
            "no slot may use a '0' partial; nyc={nyc_slot} boston={boston_slot}"
        );
        assert!(
            nyc_slot.contains("(PREVIOUS(population[nyc]) - population[boston]) * 0.01"),
            "nyc slot partial mismatch; got: {nyc_slot}"
        );
        assert!(
            boston_slot.contains("(population[boston] - PREVIOUS(population[nyc])) * 0.01"),
            "boston slot partial mismatch; got: {boston_slot}"
        );
        // Source ref is the FixedIndex(boston) subscript, constant across slots.
        assert!(
            boston_slot.contains("(population[boston] - PREVIOUS(population[boston]))"),
            "boston slot source ref mismatch; got: {boston_slot}"
        );
    }

    #[test]
    fn test_arrayed_link_score_stock_to_flow_per_element_partials() {
        // ltm-503-cross-element-agg.AC1.3 (unit-level): a stock-to-flow link
        // score into a per-element-equation arrayed flow yields per-element
        // partials referencing the flow's actual equation contents.
        let dims = vec![crate::datamodel::Dimension::named(
            "Region".to_string(),
            vec!["NYC".to_string(), "Boston".to_string(), "LA".to_string()],
        )];
        // `births[Region]` per-element flow referencing the `population` stock.
        let births = arrayed_var_from_text(
            "births",
            &dims,
            &[
                ("NYC", "population[NYC] * 0.03"),
                ("Boston", "population[Boston] * 0.02"),
                ("LA", "population[LA] * 0.01"),
            ],
            true,
        );

        let stock = Ident::<Canonical>::new("population");
        let flow = Ident::<Canonical>::new("births");
        // Each `births[e]` references `population[e]` -- a FixedIndex ref.
        let shape = RefShape::FixedIndex(vec!["nyc".to_string()]);
        let source_dim_elements = vec![vec![
            "nyc".to_string(),
            "boston".to_string(),
            "la".to_string(),
        ]];

        let equation = generate_stock_to_flow_equation(
            &stock,
            &flow,
            &shape,
            &source_dim_elements,
            &[],
            &births,
            None,
        );

        let nyc_slot = arrayed_slot(&equation, "nyc");
        let boston_slot = arrayed_slot(&equation, "boston");
        // The NYC slot keeps population[nyc] live (shape match); the other
        // slots freeze their population refs but still reference
        // `population` -- never a bare `(0)` partial.
        assert!(
            nyc_slot.contains("population[nyc] * 0.03"),
            "nyc slot partial should keep population[nyc] live; got: {nyc_slot}"
        );
        assert!(
            boston_slot.contains("population"),
            "boston slot should reference population; got: {boston_slot}"
        );
        assert!(
            !nyc_slot.contains("((0) -") && !boston_slot.contains("((0) -"),
            "no slot may use a '0' partial; nyc={nyc_slot} boston={boston_slot}"
        );
    }

    #[test]
    fn test_scalar_and_a2a_link_scores_keep_their_shapes() {
        // Guard: the Arrayed-target path must not regress scalar or
        // ApplyToAll targets. A scalar aux target -> Equation::Scalar; an
        // ApplyToAll arrayed aux target -> Equation::ApplyToAll.
        let scalar_to = Variable::Var {
            ident: Ident::new("scalar_target"),
            ast: Some(Ast::Scalar(var_ref("driver"))),
            init_ast: None,
            eqn: Some(Equation::Scalar("driver".to_string())),
            units: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        };
        let from = Ident::<Canonical>::new("driver");
        let to = Ident::<Canonical>::new("scalar_target");
        let equation = generate_auxiliary_to_auxiliary_equation(
            &from,
            &to,
            &RefShape::Bare,
            &[],
            &[],
            &scalar_to,
            None,
        );
        assert!(
            matches!(equation, Equation::Scalar(_)),
            "scalar target must yield Equation::Scalar; got: {equation:?}"
        );

        // ApplyToAll target.
        let dims = vec![region_dm_dimension()];
        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let mut implicit = Vec::new();
        let a2a_dm = crate::datamodel::Variable::Aux(crate::datamodel::Aux {
            ident: "a2a_target".to_string(),
            equation: Equation::ApplyToAll(vec!["Region".to_string()], "driver * 0.5".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: crate::datamodel::Compat::default(),
        });
        let stage0 = crate::variable::parse_var::<crate::datamodel::ModuleReference, _>(
            &dims,
            &a2a_dm,
            &mut implicit,
            &units_ctx,
            |mi| Ok(Some(mi.clone())),
        );
        let dim_ctx = crate::dimensions::DimensionsContext::from(dims.as_slice());
        let models = HashMap::new();
        let scope = crate::model::ScopeStage0 {
            models: &models,
            dimensions: &dim_ctx,
            model_name: "test",
        };
        let a2a_to = crate::model::lower_variable(&scope, &stage0);
        let to_a2a = Ident::<Canonical>::new("a2a_target");
        let equation = generate_auxiliary_to_auxiliary_equation(
            &from,
            &to_a2a,
            &RefShape::Bare,
            &[],
            &[],
            &a2a_to,
            None,
        );
        match equation {
            Equation::ApplyToAll(d, _) => assert_eq!(d, vec!["Region".to_string()]),
            other => panic!("ApplyToAll target must yield Equation::ApplyToAll; got: {other:?}"),
        }
    }
}
