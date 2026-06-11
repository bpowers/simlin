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
/// `db::ltm_ir::classify_iterated_dim_shape` (GH #511).
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
/// limitation tracked as GH #526. (NOT the GH #762 reducer-body work,
/// which covers the source→agg per-row partials in
/// [`generate_nonlinear_body_partial`]; this is the other-dep
/// iterated-subscript collapse in target-equation partials.)
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
/// Mirrors `db::ltm_ir::resolve_literal_index`'s classification logic but at
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
/// `db::ltm_ir::resolve_literal_index` (the Expr2 sibling) so both
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
            // Expr2 sibling (`db::ltm_ir::resolve_literal_index`)
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
    // `db::ltm_ir::classify_iterated_dim_shape`. Checked before the
    // literal-element pass because a dimension name (`Region`) is not a
    // literal element, so it would otherwise fall to `DynamicIndex`.
    if is_live_source_iterated_dim_subscript(indices, iter_ctx) {
        return RefShape::Bare;
    }
    let mut elems = Vec::with_capacity(indices.len());
    for (i, idx) in indices.iter().enumerate() {
        // Use the same resolver as `is_literal_element_index` so this
        // classifier and the Expr2 sibling
        // (`db::ltm_ir::resolve_literal_index`) agree on what counts
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
///
/// `dims_ctx` is the project-wide dimensions context used by
/// [`qualify_element_index`] to recognize (and qualify) subscript indices
/// that name dimension elements -- so they are never PREVIOUS-wrapped as if
/// they were causal references (GH #587). `None` (test-only callers, or
/// paths without project dims in scope) disables qualification, keeping the
/// conservative wrapping behavior.
#[allow(clippy::too_many_arguments)]
fn wrap_non_matching_in_previous(
    expr: Expr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
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
                        dims_ctx,
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
                    dims_ctx,
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
                                dims_ctx,
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
                        dims_ctx,
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
                return Expr0::App(UntypedBuiltinFn(name, args), loc);
            }
            // A LOOKUP call's first argument names a graphical-function table
            // (a lookup-only variable, or the WITH-LOOKUP self-reference); it
            // is static data the compiler resolves to a table id, not a causal
            // value reference. Wrapping it in PREVIOUS produces
            // `lookup(PREVIOUS(table), ...)`, which cannot compile (a
            // table-only variable has no value slot), so the whole link-score
            // fragment silently zeroes -- the failure mode behind WRLD3's
            // identically-zero table-mediated link scores. Hold the table
            // argument verbatim and transform only the index argument(s).
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "lookup" | "lookup_forward" | "lookup_backward"
            ) && !args.is_empty()
            {
                let mut args_iter = args.into_iter();
                let table_arg = args_iter.next().expect("checked non-empty");
                let mut new_args = vec![table_arg];
                new_args.extend(args_iter.map(|a| {
                    wrap_non_matching_in_previous(
                        a,
                        live_source,
                        live_shape,
                        other_deps,
                        source_dim_elements,
                        iter_ctx,
                        dims_ctx,
                        live_ref,
                    )
                }));
                return Expr0::App(UntypedBuiltinFn(name, new_args), loc);
            }
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
                        dims_ctx,
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
                dims_ctx,
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
                dims_ctx,
                live_ref,
            )),
            Box::new(wrap_non_matching_in_previous(
                *rhs,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                dims_ctx,
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
                dims_ctx,
                live_ref,
            )),
            Box::new(wrap_non_matching_in_previous(
                *then_expr,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                dims_ctx,
                live_ref,
            )),
            Box::new(wrap_non_matching_in_previous(
                *else_expr,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                dims_ctx,
                live_ref,
            )),
            loc,
        ),
    }
}

/// If `index` is a bare identifier that unambiguously names a dimension
/// element (per `dims_ctx`), return its qualified `dimension·element` form;
/// otherwise `None`.
///
/// Subscript indices that name dimension elements are *element selectors*,
/// not causal references. Treating them like variable references and
/// PREVIOUS-wrapping them (the pre-GH#587 behavior) turns a statically
/// resolvable index into a dynamic expression: the resulting
/// `dep[PREVIOUS(elem)]` needs a helper-aux chain whose innermost helper
/// (`$arg = elem`, a bare element name as an equation) cannot compile, so the
/// link score silently stubs to zero. Qualifying instead keeps the index a
/// compile-time constant: it can never be confused with a variable reference
/// (XMILE forbids dimension/variable name collisions, so `dim·elem` is
/// unambiguous), and `PREVIOUS(dep[dim·elem])` compiles to a direct LoadPrev
/// at the element's slot.
///
/// Qualification requires knowing *which* dimension the element belongs to.
/// The wrapper does not know the subscripted variable's declared dimensions,
/// so it only qualifies names that exactly one project dimension declares
/// (`dimension_uniquely_containing_element`); names shared by multiple
/// dimensions -- or shadowed cases the caller cannot distinguish -- keep the
/// conservative wrapping behavior.
fn qualify_element_index(
    index: &IndexExpr0,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Option<IndexExpr0> {
    let ctx = dims_ctx?;
    let IndexExpr0::Expr(Expr0::Var(name, loc)) = index else {
        return None;
    };
    let canonical = canonicalize(name.as_str());
    // Already-qualified `dim·element` references resolve via `lookup`; keep
    // them verbatim (they are already static).
    if ctx.lookup(&canonical).is_some() {
        return Some(index.clone());
    }
    let elem = crate::common::CanonicalElementName::from_raw(&canonical);
    let dim_name = ctx.dimension_uniquely_containing_element(&elem)?;
    Some(IndexExpr0::Expr(Expr0::Var(
        RawIdent::new_from_str(&format!("{}\u{B7}{}", dim_name.as_str(), canonical)),
        *loc,
    )))
}

fn wrap_index_non_matching_in_previous(
    index: IndexExpr0,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    other_deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> IndexExpr0 {
    // An index that unambiguously names a dimension element is an element
    // selector, never a causal reference: qualify it and leave it unwrapped
    // (GH #587). This must be checked BEFORE the recursive wrap below, which
    // would otherwise treat the element name as a dep reference.
    if let Some(qualified) = qualify_element_index(&index, dims_ctx) {
        return qualified;
    }
    // An index that names a dimension element which *cannot* be qualified
    // (declared by multiple dimensions at different positions, e.g. C-LEARN's
    // region elements) is still left verbatim rather than PREVIOUS-wrapped.
    // Wrapping it would make the subscript dynamic (`dep[PREVIOUS(elem)]`),
    // forcing a synthesized helper aux per call site -- the dominant residual
    // helper source on large arrayed models (GH #654) -- and is also
    // semantically wrong for a genuinely-dynamic index (the index would be
    // read from two steps ago instead of one). The downstream parse decides:
    // a non-shadowed element compiles to a static subscript (direct
    // LoadPrev), a genuinely-dynamic index still synthesizes its helper
    // there, with single-lag semantics.
    if let IndexExpr0::Expr(Expr0::Var(name, _)) = &index
        && let Some(ctx) = dims_ctx
        && ctx.is_element_of_any_dimension(&crate::common::CanonicalElementName::from_raw(
            &canonicalize(name.as_str()),
        ))
    {
        return index;
    }
    // An index that names a DIMENSION (`matrix[D1, c1]`'s `D1`,
    // `SUM(matrix[State, *])`'s `State` -- the iterated-dim reference form)
    // is a dimension selector, never a causal reference (GH #759). The two
    // guards above cover dimension *elements*; a dimension *name* is
    // neither an element nor qualifiable, so it previously fell through to
    // the recursive wrap whenever a caller's (over-collected) dep set
    // contained it: the frozen reference became `dep[PREVIOUS(d1), ..]`,
    // whose PREVIOUS-capture helper cannot compile, silently stubbing the
    // score to 0. Leave it verbatim -- the A2A expansion resolves it per
    // element downstream, exactly as in the target's own equation. The
    // `iter_ctx` leg covers callers without a project dims context (the
    // iterated/source dims are dimension names by construction).
    if let IndexExpr0::Expr(Expr0::Var(name, _)) = &index {
        let canonical = canonicalize(name.as_str());
        let names_project_dim =
            dims_ctx.is_some_and(|ctx| ctx.is_dimension_name(canonical.as_ref()));
        let names_iterated_dim = iter_ctx.is_some_and(|ctx| {
            ctx.target_iterated_dims
                .iter()
                .chain(ctx.source_dim_names.iter())
                .any(|d| d.as_str() == canonical.as_ref())
        });
        if names_project_dim || names_iterated_dim {
            return index;
        }
    }
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
            dims_ctx,
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
                dims_ctx,
                &mut None,
            ),
            wrap_non_matching_in_previous(
                r,
                live_source,
                live_shape,
                other_deps,
                source_dim_elements,
                iter_ctx,
                dims_ctx,
                &mut None,
            ),
            loc,
        ),
        other => other,
    }
}

/// A parse failure in a ceteris-paribus partial-equation builder.
///
/// The ceteris-paribus PREVIOUS-wrapping transform ([`wrap_non_matching_in_previous`])
/// can only run on a successfully-parsed `Expr0`. If `Expr0::new` returns
/// `Err` (genuinely unparseable text) or `Ok(None)` (an empty/whitespace
/// equation), there is *no* AST to wrap, so the transform cannot be applied.
///
/// Why this is an error rather than a silent fallback (GH #311): the prior
/// code returned the lowercased input text unchanged on parse failure. With
/// no PREVIOUS() wrapping, that "partial" is identical to the target's full
/// equation, so the link-score numerator `(partial - PREVIOUS(target))`
/// equals the denominator `(target - PREVIOUS(target))` and the score
/// magnitude collapses to a constant `|Δz/Δz| = 1` -- a hidden attribution
/// error that is *worse* than no score at all, and one that compiles cleanly
/// so no downstream diagnostic catches it. Returning a structured error lets
/// the (db-bearing) caller skip emitting the link-score variable and surface
/// a `Warning` naming the variable and the offending equation text, the
/// established "loud failure" pattern in this codebase
/// (cf. `emit_unscoreable_disjoint_edge_warning`).
///
/// The text being parsed is itself produced by the engine (`print_eqn` /
/// `expr2_to_string` over a compiled AST), so `Err` is effectively
/// unreachable in production; `Ok(None)` is reachable for a target with an
/// empty equation. Either way the failure is rare and unexpected -- exactly
/// the case where a silent semantics-changing fallback is most dangerous.
///
/// `UnfreezablePartial` (GH #743) is the second loud-failure class: the
/// equation parsed fine, but neither ceteris-paribus convention can be
/// rendered as a compilable equation -- the changed-first partial would
/// freeze an array slice (`PREVIOUS(matrix[d1,*])`, which has no
/// LoadPrev-of-array-view codegen path: a hard compile error in a user
/// equation, and a SILENTLY-stubbed-to-0 helper in an LTM fragment, which
/// poisoned the score into plausible-looking garbage like the constant
/// `-1/growth-rate`), and the changed-last fallback is unfreezable too (or
/// has no live occurrence to freeze). The caller skips the score and warns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PartialEquationErrorKind {
    /// The equation text failed to parse (or was empty); there is no AST
    /// to transform.
    Parse,
    /// Neither the changed-first nor the changed-last ceteris-paribus
    /// convention can be rendered as a compilable equation (GH #743).
    UnfreezablePartial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PartialEquationError {
    /// The original (pre-transform) equation text the failure is about. The
    /// db-bearing caller embeds this in the diagnostic message so the failure
    /// names the concrete offending equation.
    pub equation_text: String,
    /// Which loud-failure class this is; selects the diagnostic wording.
    pub kind: PartialEquationErrorKind,
}

impl PartialEquationError {
    fn new(equation_text: &str) -> Self {
        PartialEquationError {
            equation_text: equation_text.to_string(),
            kind: PartialEquationErrorKind::Parse,
        }
    }

    fn unfreezable(equation_text: &str) -> Self {
        PartialEquationError {
            equation_text: equation_text.to_string(),
            kind: PartialEquationErrorKind::UnfreezablePartial,
        }
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
/// Returns `None` for the second element when the parsed equation contains
/// no left-live `live_source` occurrence at all.
///
/// Returns `Err([`PartialEquationError`])` when `equation_text` fails to
/// parse -- see [`build_partial_equation_shaped`] for why this is a loud
/// error rather than a silent lowercased-input fallback (GH #311).
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
    let (transformed, live_ref) = wrap_changed_first_ast(
        equation_text,
        deps,
        live_source,
        live_shape,
        source_dim_elements,
        iter_ctx,
        dims_ctx,
    )?;
    Ok((print_eqn(&transformed), live_ref))
}

/// The shared changed-first transform: filter `deps` down to the
/// other-deps set, parse `equation_text`, and PREVIOUS-wrap via
/// [`wrap_non_matching_in_previous`] -- returning the transformed AST (not
/// printed text) plus the captured live reference. The single
/// implementation behind both [`build_partial_equation_shaped_with_live_ref`]
/// (which prints it) and [`shaped_guard_form_text`] (which doom-checks the
/// AST before printing), so the two can never drift on dep filtering,
/// parse-failure handling, or the wrap itself.
///
/// A parse failure (`Err`) or an empty equation (`Ok(None)`) leaves no AST
/// to PREVIOUS-wrap, so the ceteris-paribus partial cannot be built.
/// Returning the input unchanged would silently produce a non-ceteris-
/// paribus "partial" identical to the full equation (link score magnitude
/// == 1); fail loudly instead so the caller skips the variable and warns.
#[allow(clippy::too_many_arguments)]
fn wrap_changed_first_ast(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    live_source: &Ident<Canonical>,
    live_shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Result<(Expr0, Option<Expr0>), PartialEquationError> {
    let other_deps: HashSet<Ident<Canonical>> = deps
        .iter()
        .filter(|d| *d != live_source && normalize_module_ref(d) != *live_source)
        .cloned()
        .collect();

    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
    };

    let mut live_ref: Option<Expr0> = None;
    let transformed = wrap_non_matching_in_previous(
        ast,
        live_source,
        live_shape,
        &other_deps,
        source_dim_elements,
        iter_ctx,
        dims_ctx,
        &mut live_ref,
    );
    Ok((transformed, live_ref))
}

/// Is `expr` *array-slice-valued* -- does it contain a wildcard/star-range
/// subscript axis that no enclosing array reducer collapses? Such an
/// expression evaluates to an array view, not a scalar.
///
/// Used by [`contains_unfreezable_previous`] to decide whether a `PREVIOUS`
/// argument can be frozen: `PREVIOUS` of an array view has no codegen path
/// (no LoadPrev-of-array-view), so `PREVIOUS(matrix[d1,*])` -- or any
/// expression embedding such a slice outside a reducer -- cannot compile.
/// A reducer application (`SUM(matrix[d1,*])`) collapses the slice to a
/// scalar, so a wildcard *inside* a reducer is fine (`PREVIOUS(SUM(arr[*]))`
/// is the deliberate GH #517 whole-reducer freeze).
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
/// Such a partial can never evaluate correctly (GH #743): `PREVIOUS` of an
/// array view has no codegen path. As a *user* equation it is a hard
/// `NotSimulatable` compile error; as an LTM link-score fragment the doomed
/// `PREVIOUS` is routed through a synthesized implicit helper
/// (`$⁚$⁚ltm⁚…⁚arg0`) whose fragment fails to compile SILENTLY -- it keeps
/// a layout slot with no bytecode and reads a constant 0 -- so the partial
/// silently loses the frozen term while the outer score still compiles,
/// producing plausible-looking garbage (the constant `-1/growth-rate`
/// scores of GH #743). The partial-equation builders therefore treat this
/// shape as a routing decision: fall back to the changed-last attribution,
/// or fail loudly.
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
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    frozen_ref: &mut Option<Expr0>,
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
                // GH #511 normalization: an iterated-dim subscript reads the
                // same element a bare reference would in each slot, and the
                // IR classifies such a site `Bare`.
                if matches!(live_shape, RefShape::Bare)
                    && is_live_source_iterated_dim_subscript(&indices, iter_ctx)
                {
                    let bare = Expr0::Var(ident, loc);
                    if frozen_ref.is_none() {
                        *frozen_ref = Some(bare.clone());
                    }
                    return Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![bare]), loc);
                }
                let shape = classify_expr0_subscript_shape(&indices, source_dim_elements, iter_ctx);
                if &shape == live_shape {
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
                .map(|a| {
                    wrap_live_shaped_in_previous(
                        a,
                        live_source,
                        live_shape,
                        source_dim_elements,
                        iter_ctx,
                        frozen_ref,
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
                source_dim_elements,
                iter_ctx,
                frozen_ref,
            )),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(wrap_live_shaped_in_previous(
                *lhs,
                live_source,
                live_shape,
                source_dim_elements,
                iter_ctx,
                frozen_ref,
            )),
            Box::new(wrap_live_shaped_in_previous(
                *rhs,
                live_source,
                live_shape,
                source_dim_elements,
                iter_ctx,
                frozen_ref,
            )),
            loc,
        ),
        Expr0::If(c, t, e, loc) => Expr0::If(
            Box::new(wrap_live_shaped_in_previous(
                *c,
                live_source,
                live_shape,
                source_dim_elements,
                iter_ctx,
                frozen_ref,
            )),
            Box::new(wrap_live_shaped_in_previous(
                *t,
                live_source,
                live_shape,
                source_dim_elements,
                iter_ctx,
                frozen_ref,
            )),
            Box::new(wrap_live_shaped_in_previous(
                *e,
                live_source,
                live_shape,
                source_dim_elements,
                iter_ctx,
                frozen_ref,
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
///    `PREVIOUS` of an array slice (see [`contains_unfreezable_previous`]):
///    freeze ONLY the matching `from` occurrences and keep everything else
///    current, numerator `(target - frozen)`. This is the
///    [`generate_scalar_feeder_to_agg_equation`] convention -- a
///    first-order-equal discrete attribution of `Δz` to `Δx` (see that
///    function's rustdoc and the convention note in
///    `docs/reference/ltm--loops-that-matter.md`) -- and is what makes the
///    un-hoisted iterated-dim-feeder reducer class
///    (`growth[D1] = SUM(matrix[D1,*] * frac[D1])`, the GH #743 fixture)
///    genuinely scoreable: the wildcard co-source slice stays verbatim
///    (compiling exactly like the target's own equation) and only the
///    feeder is lagged.
/// 3. `Err(UnfreezablePartial)` when both conventions are doomed (or
///    changed-last has no matching occurrence to freeze, which would
///    silently score a constant 0): the caller skips the score variable
///    and surfaces a `Warning` -- loud degradation, never the
///    silently-stubbed-helper garbage the pre-fix path produced.
#[allow(clippy::too_many_arguments)] // threads the link-score generation context
fn shaped_guard_form_text(
    equation_text: &str,
    deps: &HashSet<Ident<Canonical>>,
    from: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
    target_ref: &str,
) -> Result<String, PartialEquationError> {
    let (changed_first, live_ref) = wrap_changed_first_ast(
        equation_text,
        deps,
        from,
        shape,
        source_dim_elements,
        iter_ctx,
        dims_ctx,
    )?;
    if !contains_unfreezable_previous(&changed_first) {
        let source_ref = source_ref_for_guard(
            from,
            shape,
            live_ref.as_ref(),
            source_dim_names,
            source_dim_elements,
        );
        return Ok(link_score_guard_form(
            &print_eqn(&changed_first),
            target_ref,
            &source_ref,
        ));
    }

    // Changed-last fallback: freeze only the live source. Re-parse the
    // (already proven parseable) equation text rather than threading the
    // pristine AST out of `wrap_changed_first_ast` -- this leg is the rare
    // doomed path, and a second cheap parse keeps the shared helper's
    // signature identical to `build_partial_equation_shaped_with_live_ref`'s
    // needs.
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
    };
    let mut frozen_ref: Option<Expr0> = None;
    let changed_last = wrap_live_shaped_in_previous(
        ast,
        from,
        shape,
        source_dim_elements,
        iter_ctx,
        &mut frozen_ref,
    );
    let Some(frozen) = frozen_ref else {
        // No matching occurrence: the "frozen" equation would be the
        // target's own equation, scoring a silent constant 0.
        return Err(PartialEquationError::unfreezable(equation_text));
    };
    if contains_unfreezable_previous(&changed_last) {
        return Err(PartialEquationError::unfreezable(equation_text));
    }
    let source_ref = source_ref_for_guard(
        from,
        shape,
        Some(&frozen),
        source_dim_names,
        source_dim_elements,
    );
    let numerator = format!("({target_ref} - ({}))", print_eqn(&changed_last));
    Ok(link_score_guard_form_with_numerator(
        &numerator,
        target_ref,
        &source_ref,
    ))
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
/// bare `Var` reference).
fn wrap_matching_in_previous(expr: Expr0, target: &Ident<Canonical>) -> Expr0 {
    match expr {
        Expr0::Const(..) => expr,
        Expr0::Var(ref ident, loc) => {
            if &Ident::<Canonical>::new(ident.as_str()) == target {
                Expr0::App(UntypedBuiltinFn("PREVIOUS".to_string(), vec![expr]), loc)
            } else {
                expr
            }
        }
        Expr0::Subscript(ident, indices, loc) => {
            let indices: Vec<IndexExpr0> = indices
                .into_iter()
                .map(|idx| match idx {
                    IndexExpr0::Expr(e) => IndexExpr0::Expr(wrap_matching_in_previous(e, target)),
                    other => other,
                })
                .collect();
            let subscript = Expr0::Subscript(ident.clone(), indices, loc);
            if &Ident::<Canonical>::new(ident.as_str()) == target {
                Expr0::App(
                    UntypedBuiltinFn("PREVIOUS".to_string(), vec![subscript]),
                    loc,
                )
            } else {
                subscript
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            // Contents of PREVIOUS/INIT are already lagged/frozen.
            if name.eq_ignore_ascii_case("previous") || name.eq_ignore_ascii_case("init") {
                return Expr0::App(UntypedBuiltinFn(name, args), loc);
            }
            let args = args
                .into_iter()
                .map(|a| wrap_matching_in_previous(a, target))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, arg, loc) => {
            Expr0::Op1(op, Box::new(wrap_matching_in_previous(*arg, target)), loc)
        }
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(
            op,
            Box::new(wrap_matching_in_previous(*l, target)),
            Box::new(wrap_matching_in_previous(*r, target)),
            loc,
        ),
        Expr0::If(c, t, f, loc) => Expr0::If(
            Box::new(wrap_matching_in_previous(*c, target)),
            Box::new(wrap_matching_in_previous(*t, target)),
            Box::new(wrap_matching_in_previous(*f, target)),
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
/// contract as [`build_partial_equation_shaped`] (GH #311).
pub(crate) fn generate_scalar_feeder_to_agg_equation(
    feeder: &str,
    agg_name: &str,
    agg_equation_text: &str,
) -> Result<String, PartialEquationError> {
    let Ok(Some(ast)) = Expr0::new(agg_equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(agg_equation_text));
    };
    let feeder_ident = Ident::<Canonical>::new(feeder);
    let frozen = print_eqn(&wrap_matching_in_previous(ast, &feeder_ident));
    let agg_q = quote_ident(agg_name);
    let feeder_q = quote_ident(feeder);
    let numerator = format!("({agg_q} - ({frozen}))");
    Ok(link_score_guard_form_with_numerator(
        &numerator, &agg_q, &feeder_q,
    ))
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
/// canonical equation format (via parse + `print_eqn`).
///
/// Returns `Ok(equation_text)` unchanged when `idents` is empty (nothing to
/// pin -- a legitimate no-op), and `Err([`PartialEquationError`])` when the
/// (already-PREVIOUS-wrapped) partial text fails to re-parse. The latter is
/// loud rather than a silent lowercased-input fallback for the same reason
/// as [`build_partial_equation_shaped`] (GH #311): an un-pinned partial may
/// not even compile, and a silent wrong equation is worse than skipping the
/// score with a warning.
///
/// `element` is a single element name (`"nyc"`, or qualified `"region·nyc"`)
/// for a one-dimensional target, or a comma-joined tuple (`"nyc,adult"` /
/// `"region·nyc,age·adult"`) for a multi-dimensional one -- the same form
/// `db::ltm::cartesian_subscripts` produces (and `qualify_element_csv`
/// qualifies) and the `parse_link_offsets` discovery parser expects on the
/// `to` side.
pub(crate) fn subscript_idents_at_element(
    equation_text: &str,
    idents: &HashSet<Ident<Canonical>>,
    element: &str,
) -> Result<String, PartialEquationError> {
    if idents.is_empty() {
        return Ok(equation_text.to_string());
    }
    let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) else {
        return Err(PartialEquationError::new(equation_text));
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
    // Qualified element parts (`region·nyc`) also pin *dimension-name*
    // subscript indices: a dep referenced as `dep[Region]` (the A2A iterated
    // form) reads the same element a bare `dep` reference would, so in a
    // per-element scalar equation it must be pinned to that element. The map
    // is keyed by the canonical dimension name each qualified part names.
    let dim_to_index: Vec<(String, IndexExpr0)> = element
        .split(',')
        .zip(index_exprs.iter())
        .filter_map(|(part, idx_expr)| {
            let part = part.trim();
            part.split_once('\u{00B7}')
                .map(|(dim, _)| (canonicalize(dim).into_owned(), idx_expr.clone()))
        })
        .collect();
    Ok(print_eqn(&subscript_idents_in_expr0(
        ast,
        idents,
        &index_exprs,
        &dim_to_index,
    )))
}

fn subscript_idents_in_expr0(
    expr: Expr0,
    idents: &HashSet<Ident<Canonical>>,
    index_exprs: &[IndexExpr0],
    dim_to_index: &[(String, IndexExpr0)],
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
        // An already-subscripted reference to a pinned dep: indices that are
        // *element literals* are already pinned and stay, but an index that
        // names one of the pinned DIMENSIONS (`dep[Region]`, the A2A iterated
        // reference form) reads "the current element" -- which in a
        // per-element scalar equation is exactly the element being pinned.
        // Left unpinned, such an index is unresolvable in scalar context and
        // forces a synthesized helper aux per occurrence (GH #654: ~27k of
        // C-LEARN's ~30k residual helpers came from this form).
        Expr0::Subscript(ident, indices, loc) => {
            let canonical = Ident::new(ident.as_str());
            if !idents.contains(&canonical) || dim_to_index.is_empty() {
                return Expr0::Subscript(ident, indices, loc);
            }
            let indices = indices
                .into_iter()
                .map(|idx| {
                    if let IndexExpr0::Expr(Expr0::Var(name, _)) = &idx {
                        let idx_canonical = canonicalize(name.as_str());
                        if let Some((_, pinned)) = dim_to_index
                            .iter()
                            .find(|(dim, _)| dim.as_str() == idx_canonical.as_ref())
                        {
                            return pinned.clone();
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
                .map(|a| subscript_idents_in_expr0(a, idents, index_exprs, dim_to_index))
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Op1(op, inner, loc) => Expr0::Op1(
            op,
            Box::new(subscript_idents_in_expr0(
                *inner,
                idents,
                index_exprs,
                dim_to_index,
            )),
            loc,
        ),
        Expr0::Op2(op, lhs, rhs, loc) => Expr0::Op2(
            op,
            Box::new(subscript_idents_in_expr0(
                *lhs,
                idents,
                index_exprs,
                dim_to_index,
            )),
            Box::new(subscript_idents_in_expr0(
                *rhs,
                idents,
                index_exprs,
                dim_to_index,
            )),
            loc,
        ),
        Expr0::If(cond, then_expr, else_expr, loc) => Expr0::If(
            Box::new(subscript_idents_in_expr0(
                *cond,
                idents,
                index_exprs,
                dim_to_index,
            )),
            Box::new(subscript_idents_in_expr0(
                *then_expr,
                idents,
                index_exprs,
                dim_to_index,
            )),
            Box::new(subscript_idents_in_expr0(
                *else_expr,
                idents,
                index_exprs,
                dim_to_index,
            )),
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
/// around). The source itself is never in this set: an arrayed-agg source is
/// pinned via `source_pin_element` instead, since the full target tuple
/// over-subscripts it in the broadcast case (GH #528).
///
/// `source_ref_override`: the pre-rendered (quoted, possibly element-pinned)
/// reference expression to use for the `Δsource` denominator. `None` uses the
/// bare `quote_ident(from)` -- correct for a true scalar source. The
/// arrayed-agg caller passes `Some("$⁚ltm⁚agg⁚n"[<slot>])` so the denominator
/// indexes the same agg slot the link-score name and the (subscripted-in-the-
/// partial) numerator do; a bare agg reference in a scalar equation would not
/// compile and the link score would stub to zero.
///
/// `source_pin_element`: the (qualified) element tuple to pin `from`'s
/// references to in the partial BODY, or `None` for a true scalar source
/// (no pinning). The arrayed-agg caller passes the target element's
/// projection onto the agg's `result_dims` axes -- the same slot the
/// link-score name and `source_ref_override` carry. Pinning the agg through
/// `to_deps_to_subscript` instead would use the target's FULL element tuple,
/// which over-subscripts the agg in the broadcast case (`agg[D1]` feeding
/// `to[D1,D2]`): the fragment fails to compile and the score is stubbed to a
/// constant 0 (GH #528). For the diagonal case (`result_dims` == `to`'s
/// dims) the projection IS the full tuple, so the equation is unchanged.
#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_scalar_to_element_equation(
    from: &str,
    to: &str,
    element: &str,
    to_elem_eqn_text: &str,
    to_deps: &HashSet<Ident<Canonical>>,
    to_deps_to_subscript: &HashSet<Ident<Canonical>>,
    source_ref_override: Option<&str>,
    source_pin_element: Option<&str>,
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Result<String, PartialEquationError> {
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
        dims_ctx,
    )?;
    let partial = subscript_idents_at_element(&partial, to_deps_to_subscript, element)?;
    // Pin the source's own references (live numerator occurrence and any
    // PREVIOUS-wrapped ones alike) to its projected slot -- a separate pass
    // from the full-tuple `to_deps_to_subscript` pinning above.
    let partial = match source_pin_element {
        Some(slot) => {
            let source_only: HashSet<Ident<Canonical>> =
                std::iter::once(from_canonical.clone()).collect();
            subscript_idents_at_element(&partial, &source_only, slot)?
        }
        None => partial,
    };
    let source_ref = source_ref_override.unwrap_or(&from_q);
    Ok(link_score_guard_form(&partial, &to_elem, source_ref))
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
    dims_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Result<String, PartialEquationError> {
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
        dims_ctx,
    )?;
    Ok(link_score_guard_form(&partial, &to_q, &agg_q))
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
///   (`SUM(pop[idx, *])`), a mapped sliced reducer the correspondence
///   declines (element-mapped -- GH #756 -- or reverse-declared -- GH #757),
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
/// name plus the *dimension-shaped* `datamodel::Equation` it should carry:
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
) -> Vec<(String, datamodel::Equation)> {
    let mut loop_vars = Vec::with_capacity(loops.len());

    // Loop-score tracing is a benchmarking/diagnostic aid compiled in only
    // under `--features ltm_bench`; the default build's `LoopScoreTrace` is a
    // zero-sized no-op whose methods optimize away entirely (no env lookup,
    // no /proc/self/status read, no byte counter, no eprintln!). See
    // [`loop_score_trace`].
    let mut trace = loop_score_trace::LoopScoreTrace::start(loops.len());

    for (i, loop_item) in loops.iter().enumerate() {
        let var_name = format!("$⁚ltm⁚loop_score⁚{}", loop_item.id);
        let equation = generate_dimensioned_loop_score_equation(
            loop_item,
            emitted_link_score_names,
            dm_dims,
            overrides,
        );
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
    use crate::datamodel;

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
        pub(super) fn record(&mut self, n: usize, equation: &datamodel::Equation) {
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

    /// Total equation-text bytes of a `datamodel::Equation`.
    #[cfg(feature = "ltm_bench")]
    fn equation_text_len(equation: &datamodel::Equation) -> usize {
        match equation {
            datamodel::Equation::Scalar(text) | datamodel::Equation::ApplyToAll(_, text) => {
                text.len()
            }
            datamodel::Equation::Arrayed(_, elements, default, _) => {
                elements.iter().map(|(_, eq, _, _)| eq.len()).sum::<usize>()
                    + default.as_ref().map(String::len).unwrap_or(0)
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
        pub(super) fn record(&mut self, _n: usize, _equation: &datamodel::Equation) {}
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

/// Build the dimension-shaped `datamodel::Equation` for one loop's score
/// variable. See [`generate_loop_score_variables`] for the three cases.
fn generate_dimensioned_loop_score_equation(
    loop_item: &Loop,
    emitted: &HashSet<String>,
    dm_dims: &[datamodel::Dimension],
    overrides: &LoopLinkOverrides,
) -> datamodel::Equation {
    if loop_item.dimensions.is_empty() {
        return datamodel::Equation::Scalar(generate_loop_score_equation(
            loop_item, emitted, overrides,
        ));
    }
    // Prefer the compact ApplyToAll form whenever it is correct (every link
    // resolves through a Bare A2A name), regardless of whether per-slot
    // circuit info is available. Otherwise use the per-slot Arrayed form.
    // A dimensioned loop with per-element-only link scores AND no slot_links
    // (a builder that predates slot capture) keeps the legacy ApplyToAll
    // emission -- the fragment-diagnostics Warning remains the backstop for
    // the mis-resolution that produces.
    if loop_item.slot_links.is_empty() || all_links_resolve_bare(loop_item, emitted) {
        return datamodel::Equation::ApplyToAll(
            loop_item.dimensions.clone(),
            generate_loop_score_equation(loop_item, emitted, overrides),
        );
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
    let elements = slot_keys
        .iter()
        .map(|tuple| {
            let text = match by_tuple.get(tuple.as_str()) {
                Some(links) => generate_link_product(links, emitted, None),
                None => "0".to_string(),
            };
            (tuple.clone(), text, None, None)
        })
        .collect();
    datamodel::Equation::Arrayed(loop_item.dimensions.clone(), elements, None, false)
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
) -> Result<Equation, PartialEquationError> {
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
) -> Result<Equation, PartialEquationError> {
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
) -> Result<Equation, PartialEquationError> {
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
    let slot_equation = |expr: &crate::ast::Expr2| -> Result<String, PartialEquationError> {
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
        shaped_guard_form_text(
            &elem_eqn_text,
            &deps_e,
            from,
            shape,
            source_dim_elements,
            source_dim_names,
            Some(&iter_ctx),
            dim_ctx,
            target_ref,
        )
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
        .map(|(elem, expr)| Ok((elem.as_str().to_string(), slot_equation(expr)?, None, None)))
        .collect::<Result<_, PartialEquationError>>()?;
    elements.sort_by(|a, b| a.0.cmp(&b.0));

    let default_slot = default_expr.map(slot_equation).transpose()?;

    Ok(Equation::Arrayed(
        target_dim_names,
        elements,
        default_slot,
        apply_default_to_missing,
    ))
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
) -> Result<Equation, PartialEquationError> {
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

    // Dependencies of the 'to' variable, with the target's and source's
    // dimension/element names filtered out (GH #759).
    let deps = scalar_or_a2a_target_deps(to_var, source_dim_elements, source_dim_names);

    // GH #511: an A2A target can reference `from` by one of the target's
    // iterated dimensions (`growth[Region,Age] = row_sum[Region] * c`).
    let target_iterated_dims = target_iterated_dim_names_canonical(to_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx,
    };
    let text = shaped_guard_form_text(
        &to_equation,
        &deps,
        from,
        shape,
        source_dim_elements,
        source_dim_names,
        Some(&iter_ctx),
        dim_ctx,
        &to_q,
    )?;
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
) -> Equation {
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
fn generate_stock_to_flow_equation(
    stock: &Ident<Canonical>,
    flow: &Ident<Canonical>,
    shape: &RefShape,
    source_dim_elements: &[Vec<String>],
    source_dim_names: &[String],
    flow_var: &Variable,
    dim_ctx: Option<&crate::dimensions::DimensionsContext>,
) -> Result<Equation, PartialEquationError> {
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

    // Dependencies of the flow variable, with the flow's and stock's
    // dimension/element names filtered out (GH #759).
    let deps = scalar_or_a2a_target_deps(flow_var, source_dim_elements, source_dim_names);

    // GH #511: a flow can reference the stock by one of the flow's own
    // iterated dimensions, the same way an A2A aux can.
    let target_iterated_dims = target_iterated_dim_names_canonical(flow_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx,
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
    let text = shaped_guard_form_text(
        &flow_equation,
        &deps,
        stock,
        shape,
        source_dim_elements,
        source_dim_names,
        Some(&iter_ctx),
        dim_ctx,
        target_ref,
    )?;
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
    overrides: &LoopLinkOverrides,
) -> String {
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
) -> String {
    let link_score_names: Vec<String> = links
        .iter()
        .enumerate()
        .map(|(i, link)| {
            // A per-link override (PR #684: a module link's per-exit-port
            // pathway-selection alias) takes precedence over the link's
            // (from, to) name resolution. Only the whole-loop path supplies
            // an override context; the per-slot path passes `None`.
            if let Some((loop_id, overrides)) = loop_overrides
                && let Some(reference) = overrides.get(&(loop_id.to_string(), i))
            {
                return reference.clone();
            }
            loop_link_score_ref(link, emitted_link_score_names)
        })
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Canonical printed text of the reducer's array argument (its "body"),
    /// e.g. `pop[*] * (1 - weight[*])` for `SUM(pop[*] * (1 - weight[*]))`.
    pub body_text: String,
}

/// Examine the target variable's Expr2 AST to find the array-reducing function
/// applied to the source variable and classify it.
///
/// Walks the Expr2 tree looking for `Expr2::App(builtin, ...)` nodes where
/// the builtin is an array reducer and the argument references the source
/// variable (identified by canonical name). Returns the [`ClassifiedReducer`]:
/// the `ReducerKind`, the uppercase function name (e.g., "SUM", "MIN"),
/// whether the reducer is the top-level expression (`is_bare`), and the
/// reducer argument's canonical text (`body_text`).
///
/// When `is_bare` is false, the reducer is nested inside other arithmetic
/// (e.g., `2 * SUM(population[*])`). Callers should fall back to the
/// delta-ratio approach for nested reducers, because the algebraic shortcut
/// ignores the surrounding arithmetic and produces wrong link scores.
/// Arithmetic INSIDE the reducer argument is a separate concern: the linear
/// shortcut is exact only for a bare-source body, so callers supply the
/// generators a [`ReducerBodyCtx`] built from `body_text` and the body-aware
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
            // Check if this builtin is a reducer whose argument references
            // the source variable.
            if let Some((kind, name, body_text)) =
                classify_builtin_if_references_source(builtin, source_ident)
            {
                return Some(ClassifiedReducer {
                    kind,
                    name,
                    is_bare: is_top_level,
                    body_text,
                });
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
) -> Option<(ReducerKind, &'static str, String)> {
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
    Some((kind, upper, crate::patch::expr2_to_string(array_arg)))
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

/// Canonical head identifiers of every `Var`/`Subscript` reference in
/// `equation_text`, recursing into subscript index expressions. Function
/// names are not collected (they are `App` nodes, not `Var`s); subscript
/// *index* identifiers (dimension and element names) ARE collected, so
/// callers must intersect the result with the model-variable map before
/// treating an entry as a variable reference. Returns an empty set when the
/// text does not parse.
///
/// Used by the link-score emitters to discover which of a reducer body's
/// references are arrayed model variables (the [`ReducerBodyCtx`] inputs).
pub(crate) fn expr_reference_idents(equation_text: &str) -> HashSet<String> {
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
    if let Ok(Some(ast)) = Expr0::new(equation_text, LexerType::Equation) {
        walk(&ast, &mut out);
    }
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
    /// The reducer's array argument, canonical text (from
    /// [`ClassifiedReducer::body_text`]).
    pub body_text: &'a str,
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

/// Rewrite a reducer body so every arrayed reference reads exactly the
/// given source row: wildcard / iterated-dim / literal indices are replaced
/// by the row's (qualified) elements, and a bare arrayed-variable reference
/// gains the full row subscript. `None` when the body cannot be safely
/// pinned (an index that doesn't correspond to the row's axes, an arrayed
/// reference with a different axis count, a nested array reducer, or a
/// FIXED-literal reference to the live source -- see below) -- the caller
/// bails to the delta-ratio fallback.
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
                if n_dims != row_parts.len() || indices.len() != row_parts.len() {
                    return None;
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
    let Ok(Some(ast)) = Expr0::new(ctx.body_text, LexerType::Equation) else {
        return None;
    };
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
    let Ok(Some(ast)) = Expr0::new(ctx.body_text, LexerType::Equation) else {
        return None;
    };
    let row_parts_of =
        |elem: &str| -> Vec<String> { elem.split(',').map(|p| p.trim().to_string()).collect() };
    let current_parts = row_parts_of(current_element);
    if current_parts.len() != ctx.row_dim_names.len() {
        return None;
    }
    let pinned_current = pin_body_to_row(ast.clone(), ctx, &current_parts)?;
    if pinned_body_is_bare_source(&pinned_current, ctx.live_source, &current_parts) {
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
            // Neither the algebraic shortcut nor the body-aware partial
            // accounts for the surrounding expression. Fall back to the
            // delta-ratio approach: use the target variable directly, which
            // measures the ratio of actual target change to source element
            // change. This is approximate (like STDDEV/RANK) but avoids the
            // wrong-multiplier bug the shortcut would introduce.
            target_ref.to_string()
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
        ReducerKind::Nonlinear => match body {
            Some(ctx) => generate_nonlinear_body_partial(
                ctx,
                source_q,
                target_ref,
                current_element,
                all_elements,
                reducer_name,
            )
            .unwrap_or_else(|| target_ref.to_string()),
            None => generate_nonlinear_partial(
                source_q,
                target_ref,
                current_element,
                all_elements,
                reducer_name,
            ),
        },
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

#[cfg(test)]
#[path = "ltm_augment_tests.rs"]
mod tests;
