// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The ceteris-paribus wrap's SUBSCRIPT-INDEX pass: what happens to the things
//! between the brackets, as opposed to the reference the brackets hang off.
//!
//! A subscript index is the one position where "is this a causal reference?" has
//! a different answer than everywhere else in an equation. A bare identifier
//! there may be an element selector, a dimension name the apply-to-all expansion
//! resolves per element, or a genuine variable read -- and only the last is a
//! ceteris-paribus dependency. Guessing wrong is expensive in both directions:
//! freezing a selector emits a `PREVIOUS`-capture helper that cannot compile
//! (a silently-zeroed score), and leaving a read live breaks the isolation the
//! partial exists to express.
//!
//! Everything that decides that question lives here: [`index_axis_verdict`] (the
//! axis-aware verdict, which supersedes the project-wide guards when the axis is
//! known), [`qualify_element_index`] (the project-wide element guard), and
//! [`wrap_index_non_matching_in_previous`] itself.
//!
//! Split out of `ltm_augment.rs` only to keep that file under the project
//! line-count lint; included via `#[path]`, so `super::*` resolves the parent's
//! private items and callers keep naming these `crate::ltm_augment::*`.

use crate::ast::{Expr0, IndexExpr0};
use crate::common::{Canonical, Ident, RawIdent, canonicalize};

use super::post_transform::qualify_axis_element;
use super::{IteratedDimCtx, WrapCtx, WrapOutcome, child_path, wrap_non_matching_in_previous};

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
/// Qualification requires knowing *which* dimension the element belongs to, and
/// this helper does not know the subscripted variable's declared dimensions: it
/// qualifies only names that exactly one PROJECT dimension declares
/// (`dimension_uniquely_containing_element`), and names shared by several
/// dimensions keep the conservative wrapping behavior.
///
/// **That is why this is now a FALLBACK.** When the axis IS known,
/// [`index_axis_verdict`] answers the same question against the indexed
/// variable's own axis and this helper is not consulted at all. Ranging over
/// the project's dimensions instead of the axis's own elements is not merely
/// imprecise -- it disagrees with `compiler::subscript::normalize_subscripts3`,
/// so a variable colliding with an UNRELATED dimension's element name is
/// qualified onto that dimension and the frozen read lands on a slot the
/// `PREVIOUS(target)` anchor never touched.
///
/// **It still runs on two PRODUCTION paths, not only at the test entry points**
/// -- every `LOOKUP` table index and everything under
/// `generate_per_element_link_equation` -- so the defect above is present tense
/// there. [`index_axis_verdict`]'s rustdoc enumerates the three shapes and says
/// why each reaches this.
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

/// What a bare-identifier index NAMES on the axis it indexes -- the engine's own
/// precedence rule, applied to the wrap.
///
/// `axis` is the declared `Dimension` at this index's position of the
/// subscripted variable, looked up in [`axis_dim_at`]. When it is known,
/// every "is this a selector or a runtime read?" question below is answered by
/// [`crate::dimensions::resolve_axis_index_name`] -- the SAME predicate
/// `compiler::subscript::normalize_subscripts3` implements and that GH #986
/// unified `ltm_agg::classify_axis_access` and
/// `post_transform::pin_dimension_name_indices` onto. The wrap was the third
/// consumer and was left on two project-wide predicates
/// (`dimension_uniquely_containing_element` and `is_element_of_any_dimension`),
/// which range over EVERY dimension in the project while the compiler ranges
/// over the axis's own declared elements.
///
/// That disagreement was a wrong NUMBER, not a missed optimization. A model
/// variable whose canonical name happens to be an element of some UNRELATED
/// dimension (a `Scenario = [base, high, low]` beside a variable named `base`)
/// read as a runtime value to the simulation and as a static selector to the
/// wrap, so the ceteris-paribus partial left it LIVE: the "frozen" partial moved
/// with it and the link score reported real influence for an edge with no causal
/// dependence at all. Worse, `qualify_element_index` then rewrote it to
/// `otherdim·name`, naming an element of a dimension the subscripted variable is
/// not declared over -- which still compiles, and reads a different slot than the
/// anchor did.
///
/// Returning `None` means the axis is unknown and the caller keeps the
/// pre-existing project-wide behaviour. **That is NOT merely the test entry
/// points, and it is NOT conservative**: on a collision the fallback does not
/// decline to answer, it answers wrongly. Three shapes reach it, and two are
/// production:
///
/// - a `LOOKUP` **table index** -- always, because a graphical-function holder is
///   by construction absent from `dep_dims` (GH #606 keeps it off the dependency
///   graph). See `freeze_lookup_table_indices`, which passes `None` explicitly.
/// - **`generate_per_element_link_equation`**, which threads `dep_dims: None`.
/// - the text-in/text-out test entry points, which have no table to thread.
///
/// So the wrap is not one consumer of this precedence rule but three call-site
/// families with two different answers. GH #986's unification reaches the
/// arrayed / scalar / A2A / stock-flow emitters and no others; the remaining two
/// are named at their call sites and are their own change.
fn index_axis_verdict(
    name: &str,
    axis: Option<&crate::dimensions::Dimension>,
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> Option<crate::dimensions::AxisIndexName> {
    let axis = axis?;
    Some(crate::dimensions::resolve_axis_index_name(
        name,
        axis,
        |dim| {
            iter_ctx.is_some_and(|ic| {
                ic.target_iterated_dims
                    .iter()
                    .any(|d| d.eq_ignore_ascii_case(dim))
            })
        },
    ))
}

/// The declared dimension at index position `pos` of `ident`.
///
/// Reads `IteratedDimCtx::dep_dims`, the target's dep -> declared-dimensions
/// table the db layer already threads here for the GH #526 other-dep
/// correspondence check. Reusing it rather than adding a second table is what
/// keeps the two questions -- "does this dep's axis correspond positionally?"
/// and "what does this index NAME on that axis?" -- answered from one source;
/// a second table built by a second route is how the wrap and the compiler came
/// to disagree in the first place.
///
/// A dep absent from the table (a scalar, an implicit/synthetic name, or a
/// caller that threads no `iter_ctx`) yields `None`, and the caller falls back.
pub(super) fn axis_dim_at<'a>(
    ctx: &WrapCtx<'a>,
    ident: &Ident<Canonical>,
    pos: usize,
) -> Option<&'a crate::dimensions::Dimension> {
    ctx.iter_ctx?.dep_dims?.get(ident.as_str())?.get(pos)
}

pub(super) fn wrap_index_non_matching_in_previous(
    index: IndexExpr0,
    ctx: &WrapCtx<'_>,
    out: &mut WrapOutcome,
    skip_element_qualification: bool,
    path: &[u16],
    frozen: bool,
    axis: Option<&crate::dimensions::Dimension>,
) -> IndexExpr0 {
    // Only `dims_ctx` (element/dimension recognition) and `iter_ctx` (the
    // iterated-dim-name guard) are read directly here; the full wrap context
    // rides `ctx` into the recursive `wrap_non_matching_in_previous` calls.
    let &WrapCtx {
        dims_ctx, iter_ctx, ..
    } = ctx;
    // The axis-resolved verdict, when the axis is known. It SUPERSEDES the two
    // project-wide element guards below -- see [`index_axis_verdict`].
    let axis_verdict = match &index {
        IndexExpr0::Expr(Expr0::Var(name, _)) => index_axis_verdict(name.as_str(), axis, iter_ctx),
        _ => None,
    };
    if let Some(verdict) = &axis_verdict {
        use crate::dimensions::AxisIndexName;
        match verdict {
            // An element THIS axis declares: a static selector. Qualify it with
            // the axis's own dimension -- the only qualification that is right
            // for a name several dimensions declare, and the one the pre-GH #986
            // `dimension_uniquely_containing_element` could not produce.
            //
            // Through `qualify_axis_element`, the ONE qualifier, rather than
            // spelling `dim·elem` again here: an INDEXED axis has no such
            // spelling (`DimensionsContext::lookup` resolves it only for a NAMED
            // dimension), so this used to emit `d·1` for a position and the
            // capture helper that froze it could not compile -- two link scores
            // reading a constant 0 behind `Assembly` warnings. That helper leaves
            // an indexed axis's position verbatim, which is what resolves.
            AxisIndexName::Element(elem) => {
                if skip_element_qualification {
                    return index;
                }
                let IndexExpr0::Expr(Expr0::Var(_, loc)) = &index else {
                    unreachable!("axis_verdict is Some only for a bare Var index")
                };
                let axis = axis.expect("verdict implies an axis");
                return IndexExpr0::Expr(Expr0::Var(
                    RawIdent::new_from_str(&qualify_axis_element(elem.as_str(), axis)),
                    *loc,
                ));
            }
            // A dimension the enclosing equation iterates: an iteration
            // reference, left verbatim exactly as the guard below does.
            AxisIndexName::IteratedDim => return index,
            // Neither: a runtime read. Fall through to the recursive wrap, which
            // freezes it if it is a dep. This is the arm the project-wide
            // predicates got wrong.
            AxisIndexName::Unresolved => {}
        }
    }
    // The project-wide fallbacks, reached only when the axis is UNKNOWN (see
    // [`index_axis_verdict`]). They answer the same questions over the project's
    // whole element/dimension namespace instead of the axis's own, which is
    // sound only when there is no axis to consult.
    let axis_unknown = axis_verdict.is_none();
    // An index that unambiguously names a dimension element is an element
    // selector, never a causal reference: qualify it and leave it unwrapped
    // (GH #587). This must be checked BEFORE the recursive wrap below, which
    // would otherwise treat the element name as a dep reference.
    //
    // `skip_element_qualification` is set on the `PerElement` frozen-live-source
    // path (Track A stage 1, finding 1): there the row-pinning lowering owns
    // source-index qualification from `from_dims`, so a literal element index is
    // left bare here (the element-verbatim guards below still keep it unwrapped)
    // and the lowering qualifies it consistently. The recursive wrap still runs
    // for a genuinely-dynamic index below, so a frozen `pop[Region, idx]` keeps
    // its `PREVIOUS(idx)` lag.
    if axis_unknown
        && !skip_element_qualification
        && let Some(qualified) = qualify_element_index(&index, dims_ctx)
    {
        return qualified;
    }
    // An ALREADY-`dim·element`-qualified index is a static element selector
    // whatever the qualification mode: it carries its own dimension and resolves
    // to a constant offset. `qualify_element_index` returns it verbatim too, but
    // that call is suppressed on the paths that own qualification themselves, so
    // the guard has to stand on its own. It was latent before GH #984 -- the
    // suppressed path only wrapped an ident present in `other_deps`, and no
    // variable is named `dim·elem` -- and became reachable when the table-index
    // freeze started widening that set with the index's own idents.
    if let IndexExpr0::Expr(Expr0::Var(name, _)) = &index
        && let Some(ctx) = dims_ctx
        && ctx.lookup(&canonicalize(name.as_str())).is_some()
    {
        return index;
    }
    // An index that names a dimension element which *cannot* be qualified
    // (declared by multiple dimensions at different positions, e.g. C-LEARN's
    // region elements) is still left verbatim rather than PREVIOUS-wrapped.
    // Like `qualify_element_index` above, this ranges over the whole project and
    // so runs only when the axis is unknown -- see `index_axis_verdict`.
    // Wrapping it would make the subscript dynamic (`dep[PREVIOUS(elem)]`),
    // forcing a synthesized helper aux per call site -- the dominant residual
    // helper source on large arrayed models (GH #654) -- and is also
    // semantically wrong for a genuinely-dynamic index (the index would be
    // read from two steps ago instead of one). The downstream parse decides:
    // a non-shadowed element compiles to a static subscript (direct
    // LoadPrev), a genuinely-dynamic index still synthesizes its helper
    // there, with single-lag semantics.
    if axis_unknown
        && let IndexExpr0::Expr(Expr0::Var(name, _)) = &index
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
    //
    // Unlike the two element guards above, this one is NOT gated on
    // `axis_unknown`, and the asymmetry is the point. Those range over the
    // project's whole ELEMENT namespace, which can disagree with the axis about
    // which dimension an element belongs to -- so when the axis is known its
    // verdict must win. A dimension NAME admits no such disagreement: reaching
    // here with a known axis means the verdict was `Unresolved`, i.e. the axis
    // does not declare this name as an element and the target does not iterate
    // it, and a dimension name cannot also be a variable name (the XMILE spec,
    // `docs/reference/xmile-v1.0.html` §3.7.1: dimension names "need to be
    // unique and accessible across a whole-model (including submodels). In
    // addition, they must be distinct from model variables names within the
    // whole-model") -- so nothing it could be is a causal value. Gating it left
    // exactly that case (a dep declared over a dimension the target does not
    // iterate, spelled with its own dimension name, reachable through a
    // dimension mapping) freezing a dimension name once GH #986 resolved axes;
    // `db::ltm_tests::a_dimension_name_index_is_not_frozen_when_the_axis_is_known`
    // is that shape.
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
    // capture it -- pass a throwaway live-ref sink. The GH #526
    // `other_dep_mismatch` doom DOES propagate: an index-nested mismatched
    // collapse dooms the changed-first partial just the same.
    let mut idx_out = WrapOutcome::default();
    // Everything below here contributes to an INDEX value, so every freeze
    // performed under it takes the un-lagged operand as its first-DT initial
    // value rather than the desugared `0`, which is out of range for a 1-based
    // subscript (GH #975; see [`freeze_at_previous`]).
    let index_ctx = WrapCtx {
        in_subscript_index: true,
        ..*ctx
    };
    let ctx = &index_ctx;
    // The index expression is at `path` (the walk pushes the index position
    // before descending); a `Range`'s two operands are children 0 and 1 of
    // that, mirroring `walk_all_in_expr`.
    let result = match index {
        IndexExpr0::Expr(e) => IndexExpr0::Expr(wrap_non_matching_in_previous(
            e,
            ctx,
            &mut idx_out,
            path,
            frozen,
        )),
        IndexExpr0::Range(l, r, loc) => IndexExpr0::Range(
            wrap_non_matching_in_previous(l, ctx, &mut idx_out, &child_path(path, 0), frozen),
            wrap_non_matching_in_previous(r, ctx, &mut idx_out, &child_path(path, 1), frozen),
            loc,
        ),
        other => other,
    };
    out.other_dep_mismatch |= idx_out.other_dep_mismatch;
    // A live-source subscript nested inside an index (`other[from[x]]`) desyncs
    // the same way; propagate its loud miss flag out of the throwaway sink.
    out.missing_occurrence |= idx_out.missing_occurrence;
    result
}
