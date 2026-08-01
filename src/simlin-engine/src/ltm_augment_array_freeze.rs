// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Materializing an LTM array-slice freeze as its own synthetic variable
//! (GH #995 option B, implemented in the wrap rather than at parse time).
//!
//! A ceteris-paribus partial freezes every non-live reference at `PREVIOUS`.
//! When the frozen reference is an ARRAY SLICE (`arr[pin, *]`,
//! `arr[pin, *:Sub]`), the inline spelling `PREVIOUS(<slice>)` cannot compile:
//! codegen requires an array-valued operand to be a view over storage, and
//! there is no LoadPrev-of-a-view. The wrap used to either decline the score
//! loudly (`UnfreezablePartial`, GH #743) or -- on the per-target-element
//! emitter path, which never doom-checked -- emit a fragment that failed
//! codegen and read a constant 0.
//!
//! [`materialize_array_freezes`] rewrites each such `PREVIOUS(<slice>)` into a
//! reference to a synthesized `$⁚ltm⁚freeze⁚…` helper: an `Equation::Arrayed`
//! aux with one arm per slice row, each arm `PREVIOUS(arr[pin, axis·elem])` --
//! a statically-subscripted read that compiles to `LoadPrev` today. The
//! helper's whole-array reference is a variable, hence a view over storage by
//! construction: exactly codegen's array-operand contract. No new opcode, no
//! VM change, no wasm change.
//!
//! WHY THE WRAP, AND WHY THE AXIS DIMENSION'S NAME. PR #1001 recorded the
//! blocker for the parse-time version of this design: `builtins_visitor` has
//! no variable -> declared-dims map, so it can neither name the axis behind a
//! bare `*` nor spell a row that is correct for every subdimension alignment.
//! The wrap HAS that map (`IteratedDimCtx::dep_dims`, threaded for the GH #526
//! other-dep check), and it matters twice over:
//!
//! * a bare `*` axis is nameable here (the dep's declared dims give it), so
//!   the bare-`*` shapes materialize too, not just the `*:Sub` spellings;
//! * each arm's subscript is qualified against the AXIS dimension
//!   (`arr[target·t3]`), and a qualified `dim·element` index resolves
//!   POSITIONALLY within the dimension it names (PR #1001;
//!   `dimensions::resolve_axis_index_position`). Position-within-axis IS the
//!   name-correct row for any alignment, while the subrange-qualified spelling
//!   a parse-time helper would synthesize (`arr[subx·t3]`) reads a wrong row
//!   whenever a named subdimension is not a positional prefix of its parent.
//!   The `*:Sub` slice itself reads rows by NAME containment in Sub's declared
//!   order (`SubdimensionRelation::parent_offsets`), so the helper's arms --
//!   Sub's elements in Sub's order, each at its name's position in the axis --
//!   match the slice row-for-row.
//!
//! A second materialization handles the frozen-VIEW-POSITION class: a
//! `PREVIOUS(<ref>)` with NO slice axes sitting where a vector builtin needs
//! a view over storage (`VECTOR ELM MAP(PREVIOUS(base[c2]), offs)`). There
//! the WHOLE dep is frozen (`materialize_whole_dep`) and the reference keeps
//! the frozen call's original indices, so the helper's storage mirrors the
//! dep's 1:1 and ELM MAP's full-storage base semantics survive. A frozen
//! slice in a view position takes the row-projected slice arm instead, whose
//! `full_source_len` is the ROW length rather than the dep's whole extent --
//! a narrower out-of-range window than the live slice's; see the note on
//! that arm.
//!
//! What is deliberately NOT materialized (the freeze keeps its existing loud
//! decline / compile failure): a slice whose kept index is not statically
//! resolvable (a dynamic pin -- each arm needs a fixed slot), a `*:Sub` whose
//! `Sub` is not a subdimension of the axis (a mid-edit inconsistency), a
//! subscript arity that does not match the dep's declared dims, a dep absent
//! from `dep_dims` (no declared dims to name axes with), and a slice-valued
//! `PREVIOUS` argument that is not a direct subscript (`PREVIOUS(a[*] + b[*])`
//! -- no single variable's rows to enumerate).
//!
//! In its own file only to keep `ltm_augment.rs` under the project line-count
//! lint; `#[path]`-mounted as a child module, so callers name it
//! `crate::ltm_augment::*`.

use std::collections::HashMap;

use crate::ast::{Expr0, IndexExpr0};
use crate::builtins::UntypedBuiltinFn;
use crate::common::canonicalize;
use crate::dimensions::{Dimension, DimensionsContext};

use super::quote_ident;

/// One materialized array-slice freeze: the synthetic variable the partial
/// now references in place of `PREVIOUS(<slice>)`.
///
/// `dims` are the helper's axis dimension names (canonical; `get_dimensions`
/// resolves canonically, so datamodel casing is not required). `arms` is one
/// `(element-subscript CSV, arm equation text)` per slice row, in row-major
/// slice order -- which is what makes the helper's whole-array view line up
/// element-for-element with the co-argument slices it sits beside.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) struct ArrayFreezeHelper {
    pub(crate) name: String,
    pub(crate) dims: Vec<String>,
    pub(crate) arms: Vec<(String, String)>,
}

/// The reserved name prefix for materialized freeze helpers.
pub(crate) const FREEZE_HELPER_PREFIX: &str = "$\u{205A}ltm\u{205A}freeze\u{205A}";

/// One axis of a materializable slice, post-classification.
enum FreezeAxis {
    /// A statically-kept index: the verbatim text baked into every arm.
    Kept(String),
    /// A sliced axis: the helper gains this axis. `dim_name` is the helper's
    /// dimension for the axis (the full axis dim for `*`, the subrange for
    /// `*:Sub`); `arm_indices` is the per-row index text (already
    /// axis-qualified for named dims, a 1-based number for indexed dims), in
    /// the order the slice's view iterates its rows; `name_part` is the
    /// axis's contribution to the content-derived helper name.
    Sliced {
        dim_name: String,
        arm_elements: Vec<String>,
        arm_indices: Vec<String>,
        name_part: String,
    },
}

/// Classify one subscript index against its declared axis dimension.
/// `None` means the slice is not materializable (see the module docs).
fn classify_axis(
    idx: &IndexExpr0,
    axis: &Dimension,
    dims_ctx: &DimensionsContext,
) -> Option<FreezeAxis> {
    match idx {
        IndexExpr0::Wildcard(_) => Some(sliced_axis_full(axis)),
        IndexExpr0::StarRange(sub, _) => {
            let sub_canonical = canonicalize(sub.as_str());
            if sub_canonical.as_ref() == axis.canonical_name().as_str() {
                // `*:Axis` spelled with the axis's own name: the full extent.
                return Some(sliced_axis_full(axis));
            }
            // A proper subdimension: the helper's axis is the subrange, and
            // each arm reads the row the subrange element NAMES in the parent
            // axis. `get_subdimension_relation` is the same containment the
            // compiled `*:Sub` slice resolves through
            // (`compiler::subscript::normalize_subscripts3`), so a `None`
            // here is a slice the simulation cannot read either.
            let sub_dim_name =
                crate::common::CanonicalDimensionName::from_raw(sub_canonical.as_ref());
            dims_ctx.get_subdimension_relation(&sub_dim_name, axis.canonical_name())?;
            let sub_dim = dims_ctx.get(&sub_dim_name)?;
            let (arm_elements, arm_indices) = match (sub_dim, axis) {
                (Dimension::Named(_, sub_named), Dimension::Named(axis_name, _)) => {
                    let _ = axis_name;
                    let elements: Vec<String> = sub_named
                        .elements
                        .iter()
                        .map(|e| e.as_str().to_string())
                        .collect();
                    let indices = elements
                        .iter()
                        .map(|e| format!("{}\u{B7}{e}", axis.canonical_name().as_str()))
                        .collect();
                    (elements, indices)
                }
                // An INDEXED subrange of an INDEXED axis reads the first N
                // rows; its "elements" are the 1-based positions. (The
                // mixed named-sub-of-indexed-axis pairing cannot reach here:
                // `get_subdimension_relation` above declines mixed kinds.)
                (Dimension::Indexed(_, size), _) => {
                    let elements: Vec<String> = (1..=*size).map(|i| i.to_string()).collect();
                    let indices = elements.clone();
                    (elements, indices)
                }
                // A named subrange of an indexed axis has no containment
                // relation (`get_subdimension_relation` above already
                // declined), so this arm is unreachable; decline defensively.
                (Dimension::Named(_, _), Dimension::Indexed(_, _)) => return None,
            };
            Some(FreezeAxis::Sliced {
                dim_name: sub_canonical.as_ref().to_string(),
                arm_elements,
                arm_indices,
                name_part: format!("*\u{205A}{}", sub_canonical.as_ref()),
            })
        }
        IndexExpr0::Expr(e) => {
            let kept = static_index_text(e, axis, dims_ctx)?;
            Some(FreezeAxis::Kept(kept))
        }
        // Ranges and `@N` positions are conservative declines: nothing emits
        // them into a frozen partial today.
        IndexExpr0::Range(_, _, _) | IndexExpr0::DimPosition(_, _) => None,
    }
}

/// The full-extent slice axis for `*` (or `*:Axis` spelled with the axis's
/// own name).
fn sliced_axis_full(axis: &Dimension) -> FreezeAxis {
    let (arm_elements, arm_indices) = match axis {
        Dimension::Named(name, named) => {
            let elements: Vec<String> = named
                .elements
                .iter()
                .map(|e| e.as_str().to_string())
                .collect();
            let indices = elements
                .iter()
                .map(|e| format!("{}\u{B7}{e}", name.as_str()))
                .collect();
            (elements, indices)
        }
        Dimension::Indexed(_, size) => {
            let elements: Vec<String> = (1..=*size).map(|i| i.to_string()).collect();
            let indices = elements.clone();
            (elements, indices)
        }
    };
    FreezeAxis::Sliced {
        dim_name: axis.canonical_name().as_str().to_string(),
        arm_elements,
        arm_indices,
        name_part: "*".to_string(),
    }
}

/// The verbatim text for a statically-resolvable kept index, or `None` for a
/// dynamic one.
///
/// Accepted: a numeric constant; a qualified `dimension·element` reference
/// (which `constify_dimensions` folds to a constant); a bare identifier the
/// AXIS dimension declares as an element (qualified against the axis here, so
/// the arm cannot be re-read as a variable reference downstream). A bare name
/// that is not an element of the axis is a runtime read -- each arm would need
/// a per-step value -- so the slice stays unmaterializable.
fn static_index_text(e: &Expr0, axis: &Dimension, dims_ctx: &DimensionsContext) -> Option<String> {
    match e {
        Expr0::Const(text, _, _) => Some(text.clone()),
        Expr0::Var(ident, _) => {
            let canonical = canonicalize(ident.as_str());
            if dims_ctx.lookup(&canonical).is_some() {
                // Already-qualified `dim·element`.
                return Some(canonical.into_owned());
            }
            if let Dimension::Named(axis_name, named) = axis {
                let elem = crate::common::CanonicalElementName::from_raw(&canonical);
                if named.indexed_elements.contains_key(&elem) {
                    return Some(format!("{}\u{B7}{}", axis_name.as_str(), elem.as_str()));
                }
            }
            None
        }
        _ => None,
    }
}

/// Is `expr` a slice-valued direct subscript -- the shape the row-projected
/// materialization handles?
fn is_direct_slice_subscript(expr: &Expr0) -> bool {
    matches!(
        expr,
        Expr0::Subscript(_, indices, _)
            if indices
                .iter()
                .any(|idx| matches!(idx, IndexExpr0::Wildcard(_) | IndexExpr0::StarRange(_, _)))
    )
}

/// Build the WHOLE-DEP freeze helper for `base` (the frozen-view-position
/// class): every element of the dep frozen, `dims` = the dep's full declared
/// dimensions (empty for a scalar dep -- a one-arm scalar helper).
///
/// Unlike the row-projected slice helpers, the reference keeps the frozen
/// call's ORIGINAL indices (or wildcards for a bare arrayed reference), so
/// the helper's storage mirrors the dep's storage 1:1 -- which is what
/// preserves `VECTOR ELM MAP`'s full-storage base semantics
/// (`codegen::full_source_len` of a subscript is the whole variable's
/// extent, and the helper's extent equals the dep's).
fn materialize_whole_dep(
    base: &crate::common::RawIdent,
    fallback: Option<&Expr0>,
    dep_dims: &HashMap<String, Vec<Dimension>>,
) -> Option<ArrayFreezeHelper> {
    let base_canonical = canonicalize(base.as_str());
    let declared = dep_dims.get(base_canonical.as_ref())?;
    let fallback_text = match fallback {
        None => None,
        Some(Expr0::Const(text, _, _)) => Some(text.clone()),
        Some(_) => return None,
    };
    let sanitize = |text: &str| text.replace('\u{B7}', "\u{205A}");
    let name = format!(
        "{FREEZE_HELPER_PREFIX}{}",
        sanitize(base_canonical.as_ref())
    );
    let axes: Vec<FreezeAxis> = declared.iter().map(sliced_axis_full).collect();
    let dims: Vec<String> = axes
        .iter()
        .filter_map(|axis| match axis {
            FreezeAxis::Sliced { dim_name, .. } => Some(dim_name.clone()),
            FreezeAxis::Kept(_) => None,
        })
        .collect();
    let base_q = quote_ident(base_canonical.as_ref());
    if dims.is_empty() {
        // Scalar dep: a one-arm scalar helper (`h = PREVIOUS(off)`).
        let body = match &fallback_text {
            Some(fb) => format!("PREVIOUS({base_q}, {fb})"),
            None => format!("PREVIOUS({base_q})"),
        };
        return Some(ArrayFreezeHelper {
            name,
            dims,
            arms: vec![(String::new(), body)],
        });
    }
    let arms = cartesian_arms(&axes, &base_q, fallback_text.as_deref());
    Some(ArrayFreezeHelper { name, dims, arms })
}

/// Try to materialize one `PREVIOUS(<slice>[, fallback])` call. `None` leaves
/// the call as-is (the caller's existing doom checks / compile failures keep
/// their behavior).
fn materialize_one(
    base: &crate::common::RawIdent,
    indices: &[IndexExpr0],
    fallback: Option<&Expr0>,
    dep_dims: &HashMap<String, Vec<Dimension>>,
    dims_ctx: &DimensionsContext,
) -> Option<ArrayFreezeHelper> {
    let base_canonical = canonicalize(base.as_str());
    let declared = dep_dims.get(base_canonical.as_ref())?;
    if declared.len() != indices.len() {
        return None;
    }
    // A non-constant fallback cannot be baked into per-row arms (its own
    // references would need the same row substitution); only the desugared
    // `0` / an explicit constant is carried. NOTE the helper NAME does not
    // encode the fallback: today every synthesized freeze is unary
    // (`freeze_at_previous` value position), so two same-slice freezes with
    // DIFFERENT constant fallbacks cannot arise -- if a caller ever produces
    // one, the model_ltm_variables dedup debug-asserts on the content
    // mismatch rather than silently keeping the first.
    let fallback_text = match fallback {
        None => None,
        Some(Expr0::Const(text, _, _)) => Some(text.clone()),
        Some(_) => return None,
    };

    let axes: Vec<FreezeAxis> = indices
        .iter()
        .zip(declared)
        .map(|(idx, axis)| classify_axis(idx, axis, dims_ctx))
        .collect::<Option<Vec<_>>>()?;

    // Helper name: content-derived, so identical freezes in different
    // partials share one helper and the name is a pure function of salsa
    // inputs (a counter would not be). Every character class here survives
    // `canonicalize` (which rewrites only `.`, whitespace, doubled
    // backslashes, and case), so the name round-trips assembly's
    // canonicalization and the quoted-ident lexer unchanged.
    //
    // A kept index's `·` is rewritten to `⁚` IN THE NAME ONLY (the arm
    // bodies keep the real qualified form): `·` is the module-output
    // separator, and every dep-resolution site that sees the helper's name
    // as a dependency splits on it (`compile_ltm_equation_fragment`'s
    // `effective.find('·')`), which would misread the helper as a
    // `module·port` reference and resolve no metadata for it -- the
    // fragment then fails to lower with `does_not_exist`.
    let sanitize = |text: &str| text.replace('\u{B7}', "\u{205A}");
    let name_parts: Vec<String> = axes
        .iter()
        .map(|axis| match axis {
            FreezeAxis::Kept(text) => sanitize(text),
            FreezeAxis::Sliced { name_part, .. } => sanitize(name_part),
        })
        .collect();
    let name = format!(
        "{FREEZE_HELPER_PREFIX}{}[{}]",
        sanitize(base_canonical.as_ref()),
        name_parts.join(",")
    );

    let dims: Vec<String> = axes
        .iter()
        .filter_map(|axis| match axis {
            FreezeAxis::Sliced { dim_name, .. } => Some(dim_name.clone()),
            FreezeAxis::Kept(_) => None,
        })
        .collect();
    debug_assert!(!dims.is_empty(), "caller guards on a slice being present");

    let base_q = quote_ident(base_canonical.as_ref());
    let arms = cartesian_arms(&axes, &base_q, fallback_text.as_deref());

    Some(ArrayFreezeHelper { name, dims, arms })
}

/// Row-major cartesian product over the sliced axes, kept indices interleaved
/// at their positions -- the same order the slice's (or whole dep's) view
/// iterates, so helper slot k IS row k.
fn cartesian_arms(
    axes: &[FreezeAxis],
    base_q: &str,
    fallback_text: Option<&str>,
) -> Vec<(String, String)> {
    // `acc` accumulates (subscript-csv, index-csv) pairs; arm body text is
    // rendered after the product is complete.
    let mut acc: Vec<(Vec<String>, Vec<String>)> = vec![(vec![], vec![])];
    for axis in axes {
        match axis {
            FreezeAxis::Kept(text) => {
                for (_, idxs) in acc.iter_mut() {
                    idxs.push(text.clone());
                }
            }
            FreezeAxis::Sliced {
                arm_elements,
                arm_indices,
                ..
            } => {
                let mut next = Vec::with_capacity(acc.len() * arm_elements.len());
                for (subs, idxs) in &acc {
                    for (elem, arm_idx) in arm_elements.iter().zip(arm_indices) {
                        let mut subs = subs.clone();
                        let mut idxs = idxs.clone();
                        subs.push(elem.clone());
                        idxs.push(arm_idx.clone());
                        next.push((subs, idxs));
                    }
                }
                acc = next;
            }
        }
    }
    let mut arms: Vec<(String, String)> = Vec::with_capacity(acc.len());
    for (subs, idxs) in acc {
        let subscript = subs.join(",");
        let read = format!("{base_q}[{}]", idxs.join(","));
        let body = match fallback_text {
            Some(fb) => format!("PREVIOUS({read}, {fb})"),
            None => format!("PREVIOUS({read})"),
        };
        arms.push((subscript, body));
    }
    arms
}

/// Rewrite every materializable `PREVIOUS(<slice>)` in `expr` into a
/// reference to its freeze helper, collecting the helpers into `out`
/// (deduplicated by name -- names are content-derived, so equal names carry
/// equal content by construction).
///
/// Non-materializable freezes are left verbatim: the caller's existing
/// behavior for them (the `contains_unfreezable_previous` doom, or a
/// downstream compile failure surfaced by `model_ltm_fragment_diagnostics`)
/// is unchanged.
pub(crate) fn materialize_array_freezes(
    expr: Expr0,
    dep_dims: &HashMap<String, Vec<Dimension>>,
    dims_ctx: &DimensionsContext,
    out: &mut Vec<ArrayFreezeHelper>,
) -> Expr0 {
    materialize_inner(expr, dep_dims, dims_ctx, out, false)
}

/// The VIEW-POSITION argument indices of a VECTOR builtin -- the arguments
/// codegen compiles with `walk_expr_as_view` (a view over storage, never a
/// scalar value). A frozen reference landing in one of these positions is an
/// `App` where a view is required, so it must materialize as a WHOLE-DEP
/// freeze helper even when its subscript has no slice axes. Mirrors the
/// `walk_expr_as_view` call sites in `compiler::codegen`'s `AssignTemp` /
/// `VectorSelect` arms; a new vector builtin must be added in both places.
///
/// Two other `walk_expr_as_view` families are DELIBERATELY absent: the
/// scalar-collapsing reducers' argument (`emit_array_reduce` -- a frozen
/// no-slice ref under an un-hoisted reducer keeps its pre-existing loud
/// failure; hoisted ones never reach the wrap spelled out), and the
/// arrayed-GF `LOOKUP` table argument (the wrap holds a table head verbatim
/// by design -- `freeze_lookup_table_indices` freezes only its indices).
fn view_arg_positions(func: &str) -> &'static [usize] {
    match func.to_ascii_lowercase().as_str() {
        "vector_select" | "vector_elm_map" | "allocate_available" | "allocate_by_priority" => {
            &[0, 1]
        }
        "vector_sort_order" | "rank" => &[0],
        _ => &[],
    }
}

fn materialize_inner(
    expr: Expr0,
    dep_dims: &HashMap<String, Vec<Dimension>>,
    dims_ctx: &DimensionsContext,
    out: &mut Vec<ArrayFreezeHelper>,
    in_view_position: bool,
) -> Expr0 {
    let recurse = |e: Expr0, out: &mut Vec<ArrayFreezeHelper>| {
        materialize_inner(e, dep_dims, dims_ctx, out, false)
    };
    match expr {
        Expr0::Const(..) | Expr0::Var(..) => expr,
        // NOTE a slice freeze reached here in a VIEW position hands ELM MAP a
        // helper whose `full_source_len` is the ROW length, not the dep's
        // whole extent (the live slice reads the whole variable's storage per
        // `codegen::full_source_len`) -- out-of-range offsets go NaN earlier
        // in the partial than in the live equation. Pre-existing to the
        // whole-dep arm below and narrower than the alternative (no score at
        // all); tightening it would mean routing view-position slices to a
        // whole-dep helper subscripted with the slice.
        Expr0::App(UntypedBuiltinFn(name, args), loc)
            if name.eq_ignore_ascii_case("previous")
                && args.first().is_some_and(is_direct_slice_subscript) =>
        {
            let materialized = match &args[0] {
                Expr0::Subscript(base, indices, _) => {
                    materialize_one(base, indices, args.get(1), dep_dims, dims_ctx)
                }
                _ => None,
            };
            match materialized {
                Some(helper) => {
                    // The reference is WILDCARD-SUBSCRIPTED (`"helper"[*]`),
                    // never bare: a bare reference to a variable declared over
                    // dimension D resolves to the CURRENT element inside an
                    // apply-to-all equation iterating D, so an A2A-emitted
                    // link score whose target reduces over its own dimension
                    // (`sel[C] = SUM(active[*] * year[*])`) would read one
                    // frozen element where the partial needs the whole frozen
                    // row -- a silent wrong score. A `[*]` per helper axis is
                    // a whole-array view in EVERY context (scalar fragments
                    // included), which is the contract the materialization
                    // exists to satisfy.
                    let helper_ref = Expr0::Subscript(
                        crate::common::RawIdent::new_from_str(&helper.name),
                        helper
                            .dims
                            .iter()
                            .map(|_| IndexExpr0::Wildcard(loc))
                            .collect(),
                        loc,
                    );
                    if !out.iter().any(|h| h.name == helper.name) {
                        out.push(helper);
                    }
                    helper_ref
                }
                // Unmaterializable: keep the call verbatim. Do NOT recurse
                // into it -- a nested rewrite inside a freeze that stays
                // doomed would only obscure the decline diagnostics.
                None => Expr0::App(UntypedBuiltinFn(name, args), loc),
            }
        }
        // The frozen-VIEW-POSITION class: `PREVIOUS(<ref>)` sitting where
        // codegen requires a view over storage (a vector builtin's array
        // argument). The slice arm above did not match (no slice axes), but
        // an App is not a view either -- materialize the freeze as a
        // WHOLE-DEP helper and re-spell the reference against it: the
        // original indices for a subscripted ref (the helper's storage
        // mirrors the dep's 1:1, preserving ELM MAP's full-storage base
        // semantics), wildcards for a bare arrayed ref, bare for a scalar.
        Expr0::App(UntypedBuiltinFn(name, args), loc)
            if in_view_position && name.eq_ignore_ascii_case("previous") =>
        {
            let materialized = match args.first() {
                Some(Expr0::Subscript(base, _, _)) | Some(Expr0::Var(base, _)) => {
                    materialize_whole_dep(base, args.get(1), dep_dims)
                }
                _ => None,
            };
            match materialized {
                Some(helper) => {
                    let helper_ident = crate::common::RawIdent::new_from_str(&helper.name);
                    let helper_ref = match &args[0] {
                        Expr0::Subscript(_, indices, _) => {
                            Expr0::Subscript(helper_ident, indices.clone(), loc)
                        }
                        Expr0::Var(_, _) if !helper.dims.is_empty() => Expr0::Subscript(
                            helper_ident,
                            helper
                                .dims
                                .iter()
                                .map(|_| IndexExpr0::Wildcard(loc))
                                .collect(),
                            loc,
                        ),
                        _ => Expr0::Var(helper_ident, loc),
                    };
                    if !out.iter().any(|h| h.name == helper.name) {
                        out.push(helper);
                    }
                    helper_ref
                }
                None => Expr0::App(UntypedBuiltinFn(name, args), loc),
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), loc) => {
            let view_positions = view_arg_positions(&name);
            let args = args
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    materialize_inner(a, dep_dims, dims_ctx, out, view_positions.contains(&i))
                })
                .collect();
            Expr0::App(UntypedBuiltinFn(name, args), loc)
        }
        Expr0::Subscript(base, indices, loc) => {
            let indices = indices
                .into_iter()
                .map(|idx| match idx {
                    IndexExpr0::Expr(e) => IndexExpr0::Expr(recurse(e, out)),
                    other => other,
                })
                .collect();
            Expr0::Subscript(base, indices, loc)
        }
        Expr0::Op1(op, mut r, loc) => {
            *r = recurse(std::mem::take(&mut *r), out);
            Expr0::Op1(op, r, loc)
        }
        Expr0::Op2(op, mut l, mut r, loc) => {
            *l = recurse(std::mem::take(&mut *l), out);
            *r = recurse(std::mem::take(&mut *r), out);
            Expr0::Op2(op, l, r, loc)
        }
        Expr0::If(mut c, mut t, mut f, loc) => {
            *c = recurse(std::mem::take(&mut *c), out);
            *t = recurse(std::mem::take(&mut *t), out);
            *f = recurse(std::mem::take(&mut *f), out);
            Expr0::If(c, t, f, loc)
        }
    }
}

/// Render `helper`'s printable diagnostic form (used by tests).
#[cfg(test)]
pub(crate) fn helper_summary(helper: &ArrayFreezeHelper) -> String {
    let arms: Vec<String> = helper
        .arms
        .iter()
        .map(|(sub, body)| format!("[{sub}] {body}"))
        .collect();
    format!(
        "{} over {:?}: {}",
        helper.name,
        helper.dims,
        arms.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Expr0, print_eqn};
    use crate::lexer::LexerType;

    fn parse(text: &str) -> Expr0 {
        Expr0::new(text, LexerType::Equation)
            .expect("parses")
            .expect("non-empty")
    }

    fn dims_fixture() -> (HashMap<String, Vec<Dimension>>, DimensionsContext) {
        let target = crate::datamodel::Dimension::named(
            "Target".to_string(),
            vec!["t1".to_string(), "t2".to_string(), "t3".to_string()],
        );
        let subx = crate::datamodel::Dimension::named(
            "SubX".to_string(),
            vec!["t3".to_string(), "t1".to_string()],
        );
        let cop = crate::datamodel::Dimension::named(
            "COP".to_string(),
            vec!["a".to_string(), "b".to_string()],
        );
        let ctx = DimensionsContext::from(&[target.clone(), subx, cop.clone()]);
        let mut dep_dims = HashMap::new();
        dep_dims.insert(
            "year".to_string(),
            vec![Dimension::from(&cop), Dimension::from(&target)],
        );
        (dep_dims, ctx)
    }

    /// A `*:Sub` axis over a NON-prefix named subdimension gets arms
    /// qualified against the AXIS dimension in the SUBRANGE's declared
    /// order -- the name-correct rows a subrange-qualified spelling reads
    /// wrongly.
    #[test]
    fn scattered_subrange_arms_are_axis_qualified_in_subrange_order() {
        let (dep_dims, ctx) = dims_fixture();
        let mut out = Vec::new();
        let expr = parse("PREVIOUS(year[cop\u{B7}a, *:subx])");
        let rewritten = materialize_array_freezes(expr, &dep_dims, &ctx, &mut out);

        assert_eq!(out.len(), 1, "one helper: {out:?}");
        let h = &out[0];
        assert_eq!(h.dims, vec!["subx".to_string()]);
        assert_eq!(
            h.arms,
            vec![
                (
                    "t3".to_string(),
                    "PREVIOUS(year[cop\u{B7}a,target\u{B7}t3])".to_string()
                ),
                (
                    "t1".to_string(),
                    "PREVIOUS(year[cop\u{B7}a,target\u{B7}t1])".to_string()
                ),
            ],
            "arms must be axis-qualified, in SubX's declared order: {}",
            helper_summary(h)
        );
        match rewritten {
            // Wildcard-subscripted, never bare: a bare reference resolves to
            // the CURRENT element inside an A2A equation iterating the
            // helper's dimension (the review's silent-wrong-score hazard).
            Expr0::Subscript(ident, indices, _) => {
                assert_eq!(ident.as_str(), h.name);
                assert_eq!(indices.len(), 1);
                assert!(matches!(indices[0], IndexExpr0::Wildcard(_)));
            }
            other => panic!(
                "expected the wildcard-subscripted helper reference, got {}",
                print_eqn(&other)
            ),
        }
    }

    /// A bare `*` axis materializes over the FULL axis dimension -- the
    /// half PR #1001 thought needed a parse-time variable->dims map.
    #[test]
    fn bare_wildcard_axis_materializes_over_the_axis_dim() {
        let (dep_dims, ctx) = dims_fixture();
        let mut out = Vec::new();
        let expr = parse("PREVIOUS(year[*, target\u{B7}t2])");
        materialize_array_freezes(expr, &dep_dims, &ctx, &mut out);

        assert_eq!(out.len(), 1);
        let h = &out[0];
        assert_eq!(h.dims, vec!["cop".to_string()]);
        assert_eq!(
            h.arms,
            vec![
                (
                    "a".to_string(),
                    "PREVIOUS(year[cop\u{B7}a,target\u{B7}t2])".to_string()
                ),
                (
                    "b".to_string(),
                    "PREVIOUS(year[cop\u{B7}b,target\u{B7}t2])".to_string()
                ),
            ]
        );
    }

    /// A bare element-name pin is qualified against ITS axis; a dynamic pin
    /// declines.
    #[test]
    fn bare_element_pin_qualifies_and_dynamic_pin_declines() {
        let (dep_dims, ctx) = dims_fixture();
        let mut out = Vec::new();
        materialize_array_freezes(
            parse("PREVIOUS(year[a, *:subx])"),
            &dep_dims,
            &ctx,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert!(
            out[0].arms[0].1.contains("year[cop\u{B7}a,"),
            "bare element pin must be axis-qualified: {}",
            helper_summary(&out[0])
        );

        let mut out2 = Vec::new();
        let kept = materialize_array_freezes(
            parse("PREVIOUS(year[idx, *:subx])"),
            &dep_dims,
            &ctx,
            &mut out2,
        );
        assert!(out2.is_empty(), "dynamic pin must not materialize");
        assert!(
            print_eqn(&kept).to_lowercase().contains("previous"),
            "the doomed freeze stays verbatim"
        );
    }

    /// Two identical freezes in one expression share one helper; the helper
    /// name survives canonicalization unchanged (assembly canonicalizes every
    /// LTM var name, so a name that mutates there would orphan its fragment).
    #[test]
    fn identical_freezes_dedup_and_names_are_canonical() {
        let (dep_dims, ctx) = dims_fixture();
        let mut out = Vec::new();
        materialize_array_freezes(
            parse("PREVIOUS(year[cop\u{B7}a, *:subx]) + PREVIOUS(year[cop\u{B7}a, *:subx])"),
            &dep_dims,
            &ctx,
            &mut out,
        );
        assert_eq!(out.len(), 1, "identical freezes share one helper");
        let name = &out[0].name;
        assert_eq!(
            canonicalize(name).as_ref(),
            name.as_str(),
            "helper names must be canonicalize-stable"
        );
    }

    /// The unary/const-fallback forms carry through; a non-const fallback
    /// declines.
    #[test]
    fn fallback_handling() {
        let (dep_dims, ctx) = dims_fixture();
        let mut out = Vec::new();
        materialize_array_freezes(
            parse("PREVIOUS(year[cop\u{B7}a, *:subx], 0)"),
            &dep_dims,
            &ctx,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert!(out[0].arms[0].1.ends_with(", 0)"), "{}", out[0].arms[0].1);

        let mut out2 = Vec::new();
        materialize_array_freezes(
            parse("PREVIOUS(year[cop\u{B7}a, *:subx], year[cop\u{B7}a, target\u{B7}t1])"),
            &dep_dims,
            &ctx,
            &mut out2,
        );
        assert!(out2.is_empty(), "non-const fallback must decline");
    }

    /// A dep absent from `dep_dims`, a mismatched arity, and a non-subscript
    /// slice argument all decline.
    #[test]
    fn unknown_dep_arity_mismatch_and_expression_slices_decline() {
        let (dep_dims, ctx) = dims_fixture();
        for text in [
            "PREVIOUS(other[*, target\u{B7}t1])",
            "PREVIOUS(year[*])",
            "PREVIOUS(year[cop\u{B7}a, *:subx] + 1)",
        ] {
            let mut out = Vec::new();
            materialize_array_freezes(parse(text), &dep_dims, &ctx, &mut out);
            assert!(out.is_empty(), "{text} must not materialize");
        }
    }
}
