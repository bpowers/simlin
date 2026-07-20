// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `#[cfg(test)]` occurrence-IR reconstruction for the ceteris-paribus wrap
//! unit tests.
//!
//! Production drives the wrap from the salsa occurrence IR
//! (`db::ltm_ir::model_ltm_reference_sites`). The focused text-in/text-out wrap
//! unit tests (`build_partial_equation_shaped` and the direct generator tests)
//! have no db, so these helpers rebuild an equivalent per-slot occurrence stream
//! on the reparsed `Expr0` -- using the `#[cfg(test)]` Expr0 classifiers the
//! alignment gate (`ltm_classifier_agreement_tests`) proves stay in step with
//! the IR -- so those tests keep exercising the real production wrap with
//! byte-identical inputs. Split out of `ltm_augment.rs` only to keep that file
//! under the project line-count lint; included via `#[path]`, so `super::*`
//! resolves the parent's private items.

use super::*;
use crate::db::ltm_ir::OccurrenceRef;

/// Reconstruct the occurrence-IR stream ([`OccurrenceSite`]s) for one target
/// equation's parsed `Expr0`, for the `#[cfg(test)]` wrap unit tests
/// ([`build_partial_equation_shaped`]). Production gets the stream from
/// `db::ltm_ir::model_ltm_reference_sites` (the `Expr2` walk); this rebuilds an
/// equivalent stream on the parsed `Expr0` so those focused text-in/text-out
/// tests keep exercising the real production wrap with byte-identical inputs.
///
/// The paths start at slot `0` (mirroring `walk_all_in_expr`'s single-body slot
/// push), so [`OccurrenceLookup::for_slot`]`(.., 0)` rebases them to the
/// slot-local paths the wrap tracks. Occurrences are recorded for every ident in
/// `deps` ∪ `{live_source}` (the idents the wrap acts on); the live source's
/// shape/axes come from the Expr0 classifier, an other-dep's axes from
/// [`other_dep_occurrence_axes`] (only its verdict is consumed).
#[cfg(test)]
pub(crate) fn build_wrap_test_occurrences(
    ast: &Expr0,
    live_source: &Ident<Canonical>,
    deps: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
) -> Vec<OccurrenceSite> {
    let mut recorded = deps.clone();
    recorded.insert(live_source.clone());
    let mut out = Vec::new();
    // Slot 0 for a scalar/A2A body (matching `walk_all_in_expr`).
    let mut path = vec![0u16];
    walk_wrap_test_occurrences(
        ast,
        live_source,
        &recorded,
        source_dim_elements,
        iter_ctx,
        false,
        &mut path,
        &mut out,
    );
    out
}

/// Build the occurrence stream for a whole target `Variable` (all slots), for
/// the `#[cfg(test)]` generator tests that construct a `Variable` directly (no
/// db). Mirrors `walk_all_in_expr`'s slot numbering: a scalar/A2A body is slot
/// `0`; an `Ast::Arrayed` target's per-element slots are canonical
/// element-key-sorted, then the default. Records references to `from` (the live
/// source); those tests reference only the source, and `iter_ctx = None` is
/// sufficient for their FixedIndex / Bare shapes.
#[cfg(test)]
pub(crate) fn test_occurrences_for_var(
    to_var: &Variable,
    from: &Ident<Canonical>,
    source_dim_elements: &[Vec<String>],
) -> Vec<OccurrenceSite> {
    use crate::ast::Ast;
    let recorded: HashSet<Ident<Canonical>> = std::iter::once(from.clone()).collect();
    let mut out = Vec::new();
    let Some(ast) = to_var.ast() else {
        return out;
    };
    let walk_slot = |expr: &crate::ast::Expr2, slot: u16, out: &mut Vec<OccurrenceSite>| {
        let e0 = crate::patch::expr2_to_expr0(expr);
        let mut path = vec![slot];
        walk_wrap_test_occurrences(
            &e0,
            from,
            &recorded,
            source_dim_elements,
            None,
            false,
            &mut path,
            out,
        );
    };
    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
            walk_slot(expr, 0, &mut out);
        }
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            let mut keys: Vec<_> = per_elem.keys().collect();
            keys.sort();
            for (slot, k) in keys.iter().enumerate() {
                walk_slot(&per_elem[*k], slot as u16, &mut out);
            }
            if let Some(def) = default_expr {
                walk_slot(def, keys.len() as u16, &mut out);
            }
        }
    }
    out
}

/// One synthesized test occurrence: only `site_id`, `reference`, `shape`,
/// `axes`, and `index_nested` are read by the wrap; the rest carry inert
/// defaults.
#[cfg(test)]
fn test_occurrence(
    ident: &Ident<Canonical>,
    path: &[u16],
    shape: RefShape,
    axes: Vec<OccurrenceAxis>,
    index_nested: bool,
) -> OccurrenceSite {
    OccurrenceSite {
        site_id: crate::db::ltm_ir::SiteId(path.to_vec().into_boxed_slice()),
        reference: OccurrenceRef::Variable(ident.as_str().to_string()),
        shape,
        axes,
        target_element: None,
        routing: crate::db::ltm_ir::OccurrenceRouting::Direct,
        in_reducer: false,
        reducer_keys: Vec::new(),
        already_lagged: false,
        index_nested,
    }
}

/// Recursive walker for [`build_wrap_test_occurrences`], mirroring
/// `db::ltm_ir::walk_all_in_expr`'s child-index path construction (and its
/// sticky `index_nested` flag) so the synthesized occurrences land at the
/// SiteId paths the wrap tracks and carry the `index_nested` bit
/// [`OccurrenceLookup::subtree_has_live_shape`] filters on.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn walk_wrap_test_occurrences(
    expr: &Expr0,
    live_source: &Ident<Canonical>,
    recorded: &HashSet<Ident<Canonical>>,
    source_dim_elements: &[Vec<String>],
    iter_ctx: Option<&IteratedDimCtx<'_>>,
    index_nested: bool,
    path: &mut Vec<u16>,
    out: &mut Vec<OccurrenceSite>,
) {
    let recurse = |sub: &Expr0, index_nested: bool, path: &mut Vec<u16>, out: &mut _| {
        walk_wrap_test_occurrences(
            sub,
            live_source,
            recorded,
            source_dim_elements,
            iter_ctx,
            index_nested,
            path,
            out,
        );
    };
    match expr {
        Expr0::Const(..) => {}
        Expr0::Var(name, _) => {
            let c = Ident::<Canonical>::new(name.as_str());
            if recorded.contains(&c) {
                out.push(test_occurrence(
                    &c,
                    path,
                    RefShape::Bare,
                    Vec::new(),
                    index_nested,
                ));
            }
        }
        Expr0::Subscript(name, indices, _) => {
            let c = Ident::<Canonical>::new(name.as_str());
            if recorded.contains(&c) {
                let (shape, axes) = if &c == live_source {
                    let shape =
                        classify_expr0_subscript_shape(indices, source_dim_elements, iter_ctx);
                    let axes = indices
                        .iter()
                        .enumerate()
                        .map(|(i, idx)| {
                            match resolve_literal_element_index(idx, i, source_dim_elements) {
                                Some(e) => OccurrenceAxis::Pinned(e),
                                None => OccurrenceAxis::Dynamic,
                            }
                        })
                        .collect();
                    (shape, axes)
                } else {
                    // Other dep: only the verdict (from its axes) is consumed.
                    (
                        RefShape::Bare,
                        other_dep_occurrence_axes(&c, indices, iter_ctx),
                    )
                };
                out.push(test_occurrence(&c, path, shape, axes, index_nested));
            }
            // Descending into a subscript index sets `index_nested` sticky-true.
            for (i, idx) in indices.iter().enumerate() {
                path.push(i as u16);
                match idx {
                    IndexExpr0::Expr(e) => recurse(e, true, path, out),
                    IndexExpr0::Range(l, r, _) => {
                        path.push(0);
                        recurse(l, true, path, out);
                        path.pop();
                        path.push(1);
                        recurse(r, true, path, out);
                        path.pop();
                    }
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
                path.pop();
            }
        }
        Expr0::App(UntypedBuiltinFn(fname, args), _) => {
            let lname = fname.to_ascii_lowercase();
            let skip_first = matches!(
                lname.as_str(),
                "lookup" | "lookup_forward" | "lookup_backward"
            ) && !args.is_empty();
            for (i, a) in args.iter().enumerate() {
                if skip_first && i == 0 {
                    continue;
                }
                path.push(i as u16);
                recurse(a, index_nested, path, out);
                path.pop();
            }
        }
        Expr0::Op1(_, inner, _) => {
            path.push(0);
            recurse(inner, index_nested, path, out);
            path.pop();
        }
        Expr0::Op2(_, l, r, _) => {
            path.push(0);
            recurse(l, index_nested, path, out);
            path.pop();
            path.push(1);
            recurse(r, index_nested, path, out);
            path.pop();
        }
        Expr0::If(c, t, e, _) => {
            for (child, sub) in [c, t, e].into_iter().enumerate() {
                path.push(child as u16);
                recurse(sub, index_nested, path, out);
                path.pop();
            }
        }
    }
}
