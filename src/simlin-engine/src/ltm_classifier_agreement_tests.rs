// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Track-A differential gate: the `#[cfg(test)]` Expr0 access-shape classifier
//! stays in step with the production occurrence IR on every non-reducer
//! reference occurrence.
//!
//! # What this gate now guards
//!
//! PRODUCTION decides an LTM causal edge's access shape ONCE, on the target's
//! `Expr2` AST ([`crate::db::ltm_ir`]: `resolve_literal_index` /
//! `classify_subscript_shape` / `classify_iterated_dim_shape`, composed by the
//! walker `collect_all_reference_sites` and cached in the salsa query
//! `model_ltm_reference_sites`). Both consumers -- causal-edge emission AND the
//! ceteris-paribus wrap -- read that single classification: the wrap tracks each
//! occurrence's structural `SiteId` path and looks its shape up in
//! `model_ltm_reference_sites`' occurrence stream (`OccurrenceLookup`), so it can
//! no longer drift from the edge emitter (the historical two-family silent-zero
//! class -- GH #759 dimension-name indices, GH #913 printer/parser asymmetry, the
//! `pop[01]` indexed-literal canonicalization -- is structurally impossible).
//!
//! The Expr0 classifier family is `#[cfg(test)]` ENTIRELY, and it now lives in
//! `ltm_augment_wrap_test_support` beside its only consumer: it reconstructs an
//! occurrence stream on a parsed `Expr0` so the focused wrap unit tests can drive
//! the real production wrap without a db. Those tests exercise production code,
//! so the reconstructed stream must carry the same shapes and paths the IR would.
//! This gate is what proves it: it classifies each occurrence with BOTH families
//! over the corpus and asserts they agree, so the `#[cfg(test)]` reconstruction
//! cannot silently diverge from the IR and feed the wrap a shape production never
//! would.
//!
//! The gate also carries the property that makes the print->reparse deletion
//! byte-neutral ([`assert_lowering_matches_reparse`]): every fixture slot is
//! checked for structural equality between the direct `Expr2` -> `Expr0` lowering
//! production now feeds the wrap and the printed-then-reparsed form it used to.
//!
//! # What this gate compares, and why it is drift-complete
//!
//! Per `(from, to)` causal edge it compares two multisets of access shapes,
//! restricted to the NON-REDUCER occurrences of `from` in `to`. That
//! restriction is not because a reducer-argument shape mismatch is harmless
//! (it can zero a score too -- see the exclusion rationale below); it is
//! because both classifier families are reducer-context-free, so the
//! non-reducer sweep already exercises every classifier code path a reducer
//! argument would reach:
//!
//! * the **Expr2** multiset comes from the production walker
//!   `collect_all_reference_sites` (the exact per-occurrence classifier
//!   `model_ltm_reference_sites` is built on -- no reimplementation), and
//! * the **Expr0** multiset comes from classifying each occurrence of `from`
//!   in the target equation *printed with the production print path
//!   (`crate::patch::expr2_to_string`) and reparsed to `Expr0`* via
//!   `classify_expr0_subscript_shape`, with the SAME `IteratedDimCtx` /
//!   `source_dim_elements` the production caller (`shaped_link_score`
//!   -> `generate_link_score_equation_for_link`) builds.
//!
//! Printing and reparsing before the Expr0 classification puts printer/parser
//! asymmetry (the GH #913 class) inside the gate's blast radius: a print path
//! that drops or reshapes an occurrence changes the Expr0 multiset (a dropped
//! occurrence, or a moved shape bucket) and the edge fails. The dimension-name
//! (GH #759) and `pop[01]` canonicalization classes are caught directly:
//! either family misclassifying an index moves a shape between the `FixedIndex`
//! / `PerElement` / `Bare` / `DynamicIndex` buckets on one side only.
//!
//! ## IR-side artifacts that are NOT drift, and how they are accounted for
//!
//! * **`ThroughAgg` duplication + the in-reducer `Wildcard`->`DynamicIndex`
//!   reclassification.** Both live in `model_ltm_reference_sites`' post-walk
//!   routing, *downstream* of either classifier family -- neither has an
//!   Expr0 mirror. This gate compares the RAW walker output
//!   (`collect_all_reference_sites`, which carries `in_reducer` but no routing
//!   or reclassification) against the raw Expr0 classifier, so those routing
//!   artifacts never enter the comparison. Reducer-argument occurrences
//!   (`in_reducer == true`) are excluded on both sides -- but NOT because
//!   their shape is inconsequential. A `Direct`, not-hoisted reducer argument
//!   (the dynamic-index carve-out, `SUM(pop[idx,*])`) keeps its shape all the
//!   way to `emit_per_shape_link_scores`, which feeds it as `live_shape` into
//!   the wrap; the wrap's reducer arm then keeps the reducer live only if the
//!   in-reducer occurrence's IR shape EQUALS that `live_shape` (production reads
//!   it from the occurrence lookup; the `#[cfg(test)]` reconstruction classifies
//!   it with `classify_expr0_subscript_shape`), so a reconstruction drift on
//!   that occurrence would feed the wrap a shape the IR never produced. The
//!   exclusion is sound for a different reason: both classifier families are
//!   reducer-context-FREE (`classify_subscript_shape` /
//!   `classify_iterated_dim_shape` / `resolve_literal_index` on Expr2,
//!   `classify_expr0_subscript_shape` / `resolve_literal_element_index` on
//!   Expr0 -- none takes an `in_reducer` input), so any drift on a
//!   reducer-argument subscript necessarily manifests identically on the SAME
//!   syntactic form outside a reducer, which the non-reducer sweep covers
//!   (`agree_dynamic_index_expression` exercises the very `pop[idx]` /
//!   `pop[idx + 1]` classification path a `SUM(pop[idx])` argument would take).
//!   The `in_reducer` flag drives only the post-walk routing (`ThroughAgg`,
//!   the `Wildcard`->`DynamicIndex` reclassification) -- downstream of both
//!   classifiers -- and the separate source->agg / agg->target emitter pair is
//!   its own mirror (a separate box). Excluding reducer arguments therefore
//!   loses no coverage of the three historical classes, which are all
//!   non-reducer direct references.
//! * **Sites with no syntactic occurrence.** A structural edge (flow->stock,
//!   module wiring) has no entry in `collect_all_reference_sites` at all, so
//!   it is simply never compared.
//! * **`PerElement` routing.** A `Direct` `PerElement` site is routed to the
//!   per-(row, element) emitter rather than the per-shape loop, but BOTH
//!   families still *classify* it as `PerElement` -- so it is compared here
//!   (the classification is the mirror; the routing is not).
//!
//! ## The one deliberate classifier asymmetry, and why it is out of scope
//!
//! The AC1.4 all-`StarRange` rule (`SUM(x[*:Dim])` -> `Wildcard`) exists only
//! on the Expr2 side; the Expr0 classifier yields `DynamicIndex` for an
//! all-`StarRange` subscript. This is the one GENUINE raw-classifier
//! divergence -- unlike the drift classes the gate catches, here the two
//! families are *meant* to disagree on the raw shape. It is confined to
//! array-valued reducer arguments: every all-`StarRange` reference in the
//! sweep sits inside a reducer (its natural context) and is excluded as
//! `in_reducer`, so it never reaches the non-reducer comparison. (Downstream
//! the divergence is harmless anyway: a not-hoisted such argument is
//! reclassified `Wildcard` -> `DynamicIndex` by `model_ltm_reference_sites`,
//! which is exactly the `DynamicIndex` the Expr0 side already produces, so the
//! consumed `live_shape` and the in-reducer Expr0 shape still agree.) A bare
//! non-reducer whole-array `arr[*]` classifies `Wildcard` on both sides and
//! would agree; the sweep does not construct a bare non-reducer all-`StarRange`
//! reference, the single shape where the families genuinely diverge.
//!
//! ## Keeping the gate non-vacuous
//!
//! The per-edge agreement assertion passes *trivially* when both families
//! produce an EMPTY multiset for an edge, so a symmetric harness-rot refactor
//! -- most plausibly flipping both `in_reducer` filters -- could leave every
//! non-reducer edge comparing empty-vs-empty and silently vacuate the whole
//! gate while all tests stay green (the edge-set anchor, `ir_edges ==
//! walker_edges`, is computed from the RAW source union *before* the filter,
//! so it cannot see this). To make the gate durably non-vacuous,
//! [`assert_classifier_families_agree`] RETURNS the per-edge non-reducer Expr2
//! multisets it compared, and the load-bearing fixtures [`pin_edge`] a
//! concrete expected multiset -- one per historical drift class (the
//! `pop[01]` FixedIndex canonicalization, the GH #759 dimension-name
//! `PerElement`, the GH #525 mixed `PerElement`, the GH #913 arrayed-slot
//! FixedIndex). Any rot that stops collecting those occurrences fails a pin
//! loudly rather than passing on an empty comparison. The one fixture that is
//! *legitimately* empty at the non-reducer surface
//! (`agree_reducer_wildcard_is_excluded_but_classified_wildcard`, whose only
//! reference sits inside a reducer) pins that emptiness explicitly, so an
//! empty compared multiset is asserted to be intentional exactly where it is.

use super::*;
use crate::ast::Ast;
use crate::common::{Canonical, Ident};
use crate::db::ltm_ir::{
    OccurrenceRef, OccurrenceSite, ReferenceSite, collect_all_reference_sites,
    model_ltm_reference_sites,
};
use crate::db::{
    DiagnosticSeverity, SimlinDb, collect_all_diagnostics, model_lowered_variables,
    project_dimensions_context, sync_from_datamodel,
};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::ltm_agg::{AxisRead, ReducerKind, reducer_kind_from_name};
use crate::test_common::TestProject;
use crate::variable::Variable;
use std::collections::BTreeMap;

/// One occurrence of a source variable found in a target equation's reparsed
/// `Expr0` AST: the source's canonical name, its subscript indices
/// (`None` for a bare `Var` reference, which is `RefShape::Bare`), and whether
/// it sits inside an aggregate-routed reducer.
///
/// Mirrors the fields the Expr2 walker (`collect_all_reference_sites`) records
/// per site (`ReferenceSite`), so the two occurrence streams line up.
struct Expr0Occurrence {
    source: String,
    indices: Option<Vec<IndexExpr0>>,
    in_reducer: bool,
    /// The structural child-index path from the target's slot root down to
    /// this occurrence node, built to mirror `walk_all_in_expr`'s `SiteId`
    /// construction exactly (slot prefix + descent chain). Stage 2's wrap
    /// consults the occurrence IR by this path, so it must equal the IR's
    /// `SiteId`; the alignment sweep pins that.
    path: Vec<u16>,
}

/// Whether `name`/`arity` names a reducer whose references route through an
/// aggregate node -- the name-based twin of
/// [`crate::ltm_agg::builtin_routes_through_agg`] (`reducer_is_hoistable` OR
/// `RANK`). Both reduce to "the reducer kind is `Linear` or `Nonlinear`":
/// `SIZE` is `Constant` (never routed, its arg is not `in_reducer`), and
/// `RANK` is `Nonlinear` (array-valued, but still aggregate-routed). Used so
/// the Expr0 occurrence walk marks `in_reducer` from the SAME set the Expr2
/// walker (`collect_all_reference_sites`) does -- otherwise `SIZE`'s argument
/// would be filtered on one side only.
fn expr0_routes_through_agg(name: &str, arity: usize) -> bool {
    matches!(
        reducer_kind_from_name(&name.to_ascii_lowercase(), arity),
        Some(ReducerKind::Linear | ReducerKind::Nonlinear)
    )
}

/// The equivalence [`expr0_routes_through_agg`]'s doc asserts, as a test.
///
/// This walk marks `in_reducer` from a NAME, so it cannot call
/// `builtin_routes_through_agg` (which needs a `BuiltinFn`) and restates the
/// rule instead. A restatement that drifts would not fail the gate loudly --
/// it would quietly change WHICH occurrences the gate compares, weakening it
/// rather than breaking it. So the two are checked against each other over the
/// shared decision table (GH #982), whose 11 rows reach every arm of
/// `reducer_kind_from_name`.
#[test]
fn expr0_reducer_routing_twin_matches_production() {
    for row in crate::ltm_agg::REDUCER_DECISION_TABLE {
        assert_eq!(
            expr0_routes_through_agg(row.name, row.arity),
            row.routes_through_agg,
            "{}/{}: the Expr0 twin must agree with builtin_routes_through_agg",
            row.name,
            row.arity
        );
    }
}

/// Walk a reparsed `Expr0` tree left-to-right depth-first, recording every
/// occurrence of a model variable (a head `Var`/`Subscript` whose ident is a
/// key of `variables`), mirroring the structure of the Expr2 production walker
/// `collect_all_reference_sites`:
///
/// * a bare `Var` head is a `Bare` occurrence;
/// * a `Subscript` head records its indices, THEN recurses into the
///   non-element-selector ones (so an index-nested model variable --
///   `other[from]` -- is recorded too, exactly as the Expr2 walker recurses
///   index expressions). An index that resolves to a literal element of the
///   subscripted variable's axis is an element SELECTOR, not a causal
///   reference, and is skipped on BOTH families (the A2a element-vs-variable
///   collision resolution -- see the `Subscript` arm below);
/// * a reducer `App` makes its arguments `in_reducer` (sticky through nested
///   reducers), matching the Expr2 walker's `reducer_keys` propagation;
/// * a `LOOKUP` call's first (table) argument is skipped, matching the Expr2
///   walker's `BuiltinContents::LookupTable` skip.
///
/// The occurrence set is intentionally the same as `collect_all_reference_sites`
/// so the two families' per-edge multisets are comparable; the shapes
/// themselves are classified afterward by the two families' own classifiers.
fn collect_expr0_occurrences(
    expr: &Expr0,
    variables: &crate::variable::LoweredVariableMap,
    in_reducer: bool,
    path: &mut Vec<u16>,
    out: &mut Vec<Expr0Occurrence>,
) {
    match expr {
        Expr0::Const(..) => {}
        Expr0::Var(raw, _) => {
            let canonical = Ident::<Canonical>::new(raw.as_str());
            if variables.contains_key(&canonical) {
                out.push(Expr0Occurrence {
                    source: canonical.as_str().to_string(),
                    indices: None,
                    in_reducer,
                    path: path.clone(),
                });
            }
        }
        Expr0::Subscript(raw, indices, _) => {
            let canonical = Ident::<Canonical>::new(raw.as_str());
            if variables.contains_key(&canonical) {
                out.push(Expr0Occurrence {
                    source: canonical.as_str().to_string(),
                    indices: Some(indices.to_vec()),
                    in_reducer,
                    path: path.clone(),
                });
            }
            // Mirror the Expr2 walker's element-selector skip: an index that
            // resolves to a literal element of the subscripted variable's axis
            // is an element SELECTOR (execution resolves it to a static offset,
            // element taking priority over any like-named variable), not a
            // causal reference -- so it is NOT an occurrence, even when it
            // collides with a like-named variable (`arr[nyc]` with a variable
            // `nyc`). This uses the Expr0 sibling resolver
            // (`resolve_literal_element_index`), so this side of the gate skips
            // exactly the token the production transform
            // (`wrap_index_non_matching_in_previous`) leaves verbatim -- both
            // classifier families agree it is not a site (the A2a element-vs-
            // variable-collision resolution).
            let source_dim_elements: Vec<Vec<String>> =
                source_dims_of(variables, canonical.as_str())
                    .iter()
                    .map(dimension_element_names)
                    .collect();
            for (i, idx) in indices.iter().enumerate() {
                if resolve_literal_element_index(idx, i, &source_dim_elements).is_some() {
                    continue;
                }
                path.push(i as u16);
                match idx {
                    IndexExpr0::Expr(e) => {
                        collect_expr0_occurrences(e, variables, in_reducer, path, out)
                    }
                    IndexExpr0::Range(l, r, _) => {
                        path.push(0);
                        collect_expr0_occurrences(l, variables, in_reducer, path, out);
                        path.pop();
                        path.push(1);
                        collect_expr0_occurrences(r, variables, in_reducer, path, out);
                        path.pop();
                    }
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
                path.pop();
            }
        }
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            let lname = name.to_ascii_lowercase();
            let child_in_reducer = in_reducer || expr0_routes_through_agg(&lname, args.len());
            let skip_first = matches!(
                lname.as_str(),
                "lookup" | "lookup_forward" | "lookup_backward"
            ) && !args.is_empty();
            for (i, arg) in args.iter().enumerate() {
                if skip_first && i == 0 {
                    continue;
                }
                path.push(i as u16);
                collect_expr0_occurrences(arg, variables, child_in_reducer, path, out);
                path.pop();
            }
        }
        Expr0::Op1(_, inner, _) => {
            path.push(0);
            collect_expr0_occurrences(inner, variables, in_reducer, path, out);
            path.pop();
        }
        Expr0::Op2(_, l, r, _) => {
            path.push(0);
            collect_expr0_occurrences(l, variables, in_reducer, path, out);
            path.pop();
            path.push(1);
            collect_expr0_occurrences(r, variables, in_reducer, path, out);
            path.pop();
        }
        Expr0::If(cond, then_e, else_e, _) => {
            for (child, sub) in [cond, then_e, else_e].into_iter().enumerate() {
                path.push(child as u16);
                collect_expr0_occurrences(sub, variables, in_reducer, path, out);
                path.pop();
            }
        }
    }
}

/// The Expr2 dimension list of a source variable (empty for a scalar / absent
/// source), the shared input both `source_dim_names` and `source_dim_elements`
/// derive from.
fn source_dims_of(variables: &crate::variable::LoweredVariableMap, source: &str) -> Vec<Dimension> {
    variables
        .get(&Ident::<Canonical>::new(source))
        .and_then(|v| v.get_dimensions())
        .map(|d| d.to_vec())
        .unwrap_or_default()
}

/// The target equation's iterated dimension names (canonical, in declared
/// order; empty for a scalar target) -- the same list the production caller
/// puts in `IteratedDimCtx::target_iterated_dims`.
fn target_iterated_dims_of(to_var: &Variable) -> Vec<String> {
    match to_var.ast() {
        Some(Ast::ApplyToAll(dims, _)) | Some(Ast::Arrayed(dims, _, _, _)) => {
            dims.iter().map(|d| d.name().to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// Classify one reparsed Expr0 occurrence of `source` with the `#[cfg(test)]`
/// Expr0 classifier, using the SAME context construction the production wrap's
/// occurrence lookup was built against. A bare occurrence is `Bare` by
/// construction (matching the Expr2 walker's `Var` arm). `dep_dims` is `None`:
/// it feeds only the *non-live-dep* verdict (`other_dep_occurrence_axes` +
/// `derive_other_dep_verdict`), never the live source's shape, which is all
/// this classifies.
fn classify_expr0_occurrence(
    occ: &Expr0Occurrence,
    to_var: &Variable,
    variables: &crate::variable::LoweredVariableMap,
    dim_ctx: &DimensionsContext,
) -> (RefShape, Vec<OccurrenceAxis>) {
    let Some(indices) = &occ.indices else {
        return (RefShape::Bare, Vec::new());
    };
    let source_dims = source_dims_of(variables, &occ.source);
    let source_dim_names: Vec<String> = source_dims.iter().map(|d| d.name().to_string()).collect();
    let source_dim_elements: Vec<Vec<String>> =
        source_dims.iter().map(dimension_element_names).collect();
    let target_iterated_dims = target_iterated_dims_of(to_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dep_dims: None,
    };
    let shape = classify_expr0_subscript_shape(
        indices,
        &source_dim_elements,
        Some(&iter_ctx),
        Some(dim_ctx),
    );
    // The per-axis vector the `#[cfg(test)]` occurrence builder synthesizes, via
    // the SAME function it uses -- so the gate proves what the builder actually
    // produces, not a third derivation of it.
    let axes = indices
        .iter()
        .enumerate()
        .map(|(i, idx)| {
            live_source_occurrence_axis(
                idx,
                i,
                &source_dim_elements,
                Some(&iter_ctx),
                Some(dim_ctx),
            )
        })
        .collect();
    (shape, axes)
}

/// The reparsed `Expr0` occurrences of every model variable in one target
/// equation, built from the target's Expr2 AST printed with the PRODUCTION
/// print path (`crate::patch::expr2_to_string`) and reparsed -- the same text
/// `shaped_link_score` feeds the Expr0 partial builder. An
/// `Ast::Arrayed` target is printed and walked per element slot (plus the
/// default), mirroring `build_arrayed_link_score_equation`'s per-slot
/// `expr2_to_string`; `collect_all_reference_sites` walks the same slots on
/// the Expr2 side, so the occurrence streams align.
///
/// A slot whose printed text fails to reparse is a loud failure (the gate must
/// fail, not skip): such a print/parse asymmetry is exactly the GH #913 drift
/// class this gate exists to catch.
fn expr0_occurrences_for_target(
    to_name: &str,
    to_var: &Variable,
    variables: &crate::variable::LoweredVariableMap,
) -> Vec<Expr0Occurrence> {
    let mut out = Vec::new();
    let Some(ast) = to_var.ast() else {
        return out;
    };
    // `slot` is the first `SiteId` path element (`walk_all_in_expr`: slot 0 for
    // a scalar/A2A body, the sorted element index for each `Ast::Arrayed` slot,
    // then the default slot after the last element).
    let mut walk_slot = |slot: u16, expr: &crate::ast::Expr2| {
        // Every fixture slot doubles as a corpus sample for the round-trip
        // fidelity property the print->reparse deletion rests on.
        assert_lowering_matches_reparse(&format!("target '{to_name}' slot {slot}"), expr);
        let text = crate::patch::expr2_to_string(expr);
        let parsed = Expr0::new(&text, crate::lexer::LexerType::Equation).unwrap_or_else(|e| {
            panic!("target '{to_name}' printed text failed to reparse (GH #913 drift class): text={text:?}, err={e:?}")
        });
        if let Some(expr0) = parsed {
            let mut path = vec![slot];
            collect_expr0_occurrences(&expr0, variables, false, &mut path, &mut out);
        }
    };
    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
            walk_slot(0, expr);
        }
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            // Deterministic slot order (matches the sorted walk in
            // `collect_all_reference_sites`); order does not affect the
            // per-edge multiset but keeps failure output stable.
            let mut keys: Vec<_> = per_elem.keys().collect();
            keys.sort();
            for (slot, k) in keys.iter().enumerate() {
                walk_slot(slot as u16, &per_elem[*k]);
            }
            if let Some(default) = default_expr {
                walk_slot(keys.len() as u16, default);
            }
        }
    }
    out
}

/// Rebuild `expr` with every `Loc` reset to the default and every raw
/// identifier canonicalized, so two `Expr0` trees can be compared for
/// STRUCTURAL equality with the derived `PartialEq`.
///
/// Two coordinates are deliberately erased, because they are exactly the two an
/// `Expr0` consumer never reads directly:
///
/// * **`Loc`** -- a direct `Expr2` -> `Expr0` lowering carries the ORIGINAL
///   model text's spans while a print->reparse carries the printed text's. No
///   LTM consumer reads them.
/// * **Raw identifier SPELLING** -- `RawIdent` is pre-canonical by definition.
///   `Ident::<Canonical>::to_source_repr` renders the module separator `·` back
///   as `.`, and the parser hands back a quoted name WITH its quotes, so a
///   module-output composite is `$⁚m⁚0⁚smth1.output` on the lowering side and
///   `"$⁚m⁚0⁚smth1·output"` on the reparse side. Every consumer reads such a
///   head through `canonicalize` / `Ident::new` and prints it through
///   `print_ident` (which canonicalizes), so the two spellings are the same
///   identifier everywhere it matters -- including in the emitted text.
fn strip_locs(expr: &Expr0) -> Expr0 {
    let d = crate::ast::Loc::default();
    match expr {
        Expr0::Const(s, v, _) => Expr0::Const(s.clone(), *v, d),
        Expr0::Var(id, _) => Expr0::Var(canonical_raw(id), d),
        Expr0::App(UntypedBuiltinFn(name, args), _) => Expr0::App(
            UntypedBuiltinFn(name.clone(), args.iter().map(strip_locs).collect()),
            d,
        ),
        Expr0::Subscript(id, indices, _) => Expr0::Subscript(
            canonical_raw(id),
            indices.iter().map(strip_index_locs).collect(),
            d,
        ),
        Expr0::Op1(op, inner, _) => Expr0::Op1(*op, Box::new(strip_locs(inner)), d),
        Expr0::Op2(op, l, r, _) => {
            Expr0::Op2(*op, Box::new(strip_locs(l)), Box::new(strip_locs(r)), d)
        }
        Expr0::If(c, t, e, _) => Expr0::If(
            Box::new(strip_locs(c)),
            Box::new(strip_locs(t)),
            Box::new(strip_locs(e)),
            d,
        ),
    }
}

/// The canonical spelling of a raw identifier, as every `Expr0` consumer reads
/// it. See [`strip_locs`] for why spelling is normalized away.
fn canonical_raw(id: &crate::common::RawIdent) -> crate::common::RawIdent {
    crate::common::RawIdent::new_from_str(canonicalize(id.as_str()).as_ref())
}

fn strip_index_locs(index: &IndexExpr0) -> IndexExpr0 {
    let d = crate::ast::Loc::default();
    match index {
        IndexExpr0::Wildcard(_) => IndexExpr0::Wildcard(d),
        IndexExpr0::StarRange(id, _) => IndexExpr0::StarRange(canonical_raw(id), d),
        IndexExpr0::Range(l, r, _) => {
            IndexExpr0::Range(Box::new(strip_locs(l)), Box::new(strip_locs(r)), d)
        }
        IndexExpr0::DimPosition(n, _) => IndexExpr0::DimPosition(*n, d),
        IndexExpr0::Expr(e) => IndexExpr0::Expr(strip_locs(e)),
    }
}

/// Assert that lowering `expr` straight to `Expr0` (`patch::expr2_to_expr0`) is
/// STRUCTURALLY identical to printing it and re-parsing the text -- the property
/// that makes the print->reparse round trip deletable without changing a single
/// generated byte.
///
/// Every LTM equation transform runs on an `Expr0`, and today that `Expr0` comes
/// from `Expr0::new(expr2_to_string(expr))` -- a print followed by a parse of our
/// own output. The direct lowering is the same function minus the two string
/// steps (`expr2_to_string` IS `print_eqn(expr2_to_expr0(..))`), so if the two
/// results are structurally equal then swapping them cannot change any wrap
/// decision, any occurrence path, or any printed byte.
///
/// Note the asymmetry in what a failure would mean: the DIRECT lowering is
/// structurally isomorphic to the `Expr2` tree by construction, so it is the one
/// that matches `db::ltm_ir::walk_all_in_expr`'s `SiteId` paths. A mismatch here
/// therefore reports a print/reparse infidelity in TODAY's path, not a defect in
/// the replacement.
#[track_caller]
fn assert_lowering_matches_reparse(what: &str, expr: &crate::ast::Expr2) {
    let lowered = crate::patch::expr2_to_expr0(expr);
    let text = crate::ast::print_eqn(&lowered);
    let reparsed = Expr0::new(&text, crate::lexer::LexerType::Equation)
        .unwrap_or_else(|e| panic!("{what}: printed text {text:?} failed to reparse: {e:?}"))
        .unwrap_or_else(|| panic!("{what}: printed text {text:?} parsed to nothing"));
    assert_eq!(
        strip_locs(&lowered),
        strip_locs(&reparsed),
        "{what}: the direct Expr2->Expr0 lowering and the print->reparse round trip \
         disagree structurally on {text:?}; deleting the round trip would change the \
         tree the LTM wrap walks"
    );
}

/// Sort a shape multiset for order-insensitive comparison. `RefShape` derives
/// `Ord`, so a stable sort gives a canonical multiset key.
fn sorted(shapes: &[RefShape]) -> Vec<RefShape> {
    let mut v = shapes.to_vec();
    v.sort();
    v
}

/// The comparison core (functional): do two non-reducer shape multisets agree?
/// Split out so the harness AND the sensitivity negative test
/// (`comparison_reports_mismatched_shapes`) exercise the identical decision.
fn edge_shapes_agree(expr2: &[RefShape], expr0: &[RefShape]) -> bool {
    sorted(expr2) == sorted(expr0)
}

/// The per-edge NON-REDUCER Expr2 shape multiset the harness actually compared,
/// keyed by `(from, to)` and stored sorted (so a pin is order-insensitive).
/// Returned by [`assert_classifier_families_agree`] so a fixture can PIN the
/// exact expected multiset -- the gate's permanent non-vacuity anchor.
type ComparedShapes = BTreeMap<(String, String), Vec<RefShape>>;

/// Pin an edge's compared non-reducer Expr2 multiset to `expected`.
///
/// This is the load-bearing complement to the per-edge agreement assertion in
/// [`assert_classifier_families_agree`]: that assertion passes *vacuously* when
/// BOTH families produce an empty multiset for an edge, so a symmetric harness
/// refactor that stops collecting the occurrences it should compare (e.g.
/// flipping both `in_reducer` filters) would leave every non-reducer edge
/// comparing empty-vs-empty and silently vacuate the gate while all tests stay
/// green. A pin against a non-empty `expected` fails the instant that happens,
/// so at least one fixture always asserts a concrete, non-empty shape multiset.
///
/// The edge must have been compared (panics loudly if absent, rather than
/// letting a pin pass because the edge disappeared from the sweep).
fn pin_edge(compared: &ComparedShapes, from: &str, to: &str, expected: &[RefShape]) {
    let actual = compared
        .get(&(from.to_string(), to.to_string()))
        .unwrap_or_else(|| {
            panic!(
                "gate compared no edge {from} -> {to}; edges compared: {:?}",
                compared.keys().collect::<Vec<_>>()
            )
        });
    assert_eq!(
        actual,
        &sorted(expected),
        "edge {from} -> {to}: compared non-reducer Expr2 multiset does not match the pinned \
         expectation (a drift, or the harness stopped collecting this edge's occurrences)"
    );
}

/// Assert the occurrence IR's per-occurrence stream for `to` aligns
/// position-for-position with the reparsed-Expr0 stream the ceteris-paribus
/// wrap runs on -- the corpus proof of the SiteId-zip bridge stage 2 consumes.
///
/// Both streams are a left-to-right DFS over the target's slots (a scalar/A2A
/// target is one slot; an `Ast::Arrayed` target's slots are sorted then the
/// default). The IR stream (`model_ltm_reference_sites(..).occurrences[to]`)
/// comes from the Expr2 walk; the Expr0 stream from the production-printed,
/// reparsed text. Filtering the IR stream to `OccurrenceRef::Variable` (the
/// Expr0 walk records no module-qualified occurrence) leaves two streams that
/// must be equal length and, at each position, name the same source with the
/// same `in_reducer` context and -- off the reducer path -- the same shape. A
/// length or per-position mismatch is precisely the GH #913 / walker-divergence
/// desync a positional or SiteId zip must never silently tolerate, so it fails
/// loudly here.
fn assert_occurrence_stream_aligns(
    to_str: &str,
    to_var: &Variable,
    variables: &crate::variable::LoweredVariableMap,
    dim_ctx: &DimensionsContext,
    ir: &crate::db::ltm_ir::LtmReferenceSitesResult,
    expr0_occs: &[Expr0Occurrence],
) {
    let ir_var_occs: Vec<&OccurrenceSite> = ir
        .occurrences
        .get(to_str)
        .map(|v| {
            v.iter()
                .filter(|o| matches!(o.reference, OccurrenceRef::Variable(_)))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ir_var_occs.len(),
        expr0_occs.len(),
        "occurrence-stream LENGTH mismatch for target '{to_str}' (IR Expr2 walk vs reparsed \
         Expr0 walk): the per-occurrence SiteId zip the stage-2 deletion relies on would \
         desync.\n  IR sources: {:?}\n  Expr0 sources: {:?}",
        ir_var_occs
            .iter()
            .map(|o| match &o.reference {
                OccurrenceRef::Variable(s) => s.as_str(),
                _ => "?",
            })
            .collect::<Vec<_>>(),
        expr0_occs
            .iter()
            .map(|o| o.source.as_str())
            .collect::<Vec<_>>(),
    );
    for (ir_occ, e0_occ) in ir_var_occs.iter().zip(expr0_occs) {
        let OccurrenceRef::Variable(ir_src) = &ir_occ.reference else {
            unreachable!("filtered to Variable above");
        };
        assert_eq!(
            ir_src, &e0_occ.source,
            "occurrence-stream SOURCE mismatch for target '{to_str}' (IR vs reparsed Expr0)"
        );
        // The SiteId-keyed bridge stage 2 consumes: the wrap looks the
        // occurrence up by the structural child-index path it walks. That path
        // must equal the IR's `SiteId`, computed on the Expr2 AST -- so the
        // print->reparse round trip must be child-index-isomorphic at every
        // occurrence node. A mismatch would make the wrap's SiteId lookup miss
        // (or hit the wrong occurrence) despite the streams aligning positionally.
        assert_eq!(
            &ir_occ.site_id.0[..],
            &e0_occ.path[..],
            "occurrence-stream SiteId PATH mismatch for {ir_src} -> {to_str}: the reparsed-Expr0 \
             child-index path does not equal the IR's Expr2 SiteId, so a SiteId-keyed wrap lookup \
             would desync"
        );
        assert_eq!(
            ir_occ.in_reducer, e0_occ.in_reducer,
            "occurrence-stream in_reducer mismatch for {ir_src} -> {to_str}"
        );
        if !ir_occ.in_reducer {
            let (e0_shape, e0_axes) = classify_expr0_occurrence(e0_occ, to_var, variables, dim_ctx);
            assert_eq!(
                ir_occ.shape, e0_shape,
                "occurrence-stream SHAPE mismatch for {ir_src} -> {to_str}: the wrap's IR lookup \
                 would return a shape the Expr0 classifier disagrees with"
            );
            // The PER-AXIS classification, not just the coarse shape. Since the
            // `PerElement` row pinning moved into the ceteris-paribus wrap it reads
            // `OccurrenceSite::axes` to decide which index is a projected
            // coordinate and which is a fixed literal, so the reconstruction the
            // wrap unit tests feed the real wrap has to agree here too -- a
            // divergence would exercise a pinning production never performs.
            assert_eq!(
                ir_occ.axes, e0_axes,
                "occurrence-stream AXES mismatch for {ir_src} -> {to_str}: the row pinning reads \
                 these, so the test-side reconstruction would drive a lowering production never \
                 would"
            );
        }
    }
}

/// Assert the occurrence-IR / reparsed-Expr0 stream alignment (the SiteId-zip
/// bridge) for EVERY target of `tp`'s `main` model, WITHOUT the per-edge
/// multiset gate. Lets the bridge be stress-tested on shapes the multiset gate
/// does not construct (module outputs, `PREVIOUS`/`INIT`-lagged references,
/// index-nested references, `LOOKUP` tables), where only the per-occurrence
/// alignment is the relevant property.
fn assert_occurrence_streams_align(tp: &TestProject) {
    let datamodel = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let project = sync.project;

    // Non-vacuity guard 1 (fail loudly, never skip silently): a fixture
    // equation that fails to parse leaves its target with NO AST, so
    // `expr0_occurrences_for_target` returns empty and every alignment
    // assertion below passes trivially (0 == 0). Catch that at the source: an
    // Error-severity diagnostic surfaces exactly a parse failure (the
    // grammar-rejected `IF`-as-operand shape was the concrete instance).
    let errors: Vec<_> = collect_all_diagnostics(&db, project)
        .into_iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "alignment fixture has compile errors -- a target that fails to parse would \
         silently vacate the alignment sweep (empty occurrence stream): {errors:?}"
    );

    let variables = model_lowered_variables(&db, model, project);
    let dim_ctx = project_dimensions_context(&db, project);
    let ir = model_ltm_reference_sites(&db, model, project);

    let mut total_occurrences = 0usize;
    let mut to_names: Vec<&Ident<Canonical>> = variables.keys().collect();
    to_names.sort();
    for to_name in to_names {
        let to_var = &variables[to_name];
        let expr0_occs = expr0_occurrences_for_target(to_name.as_str(), to_var, &variables);
        total_occurrences += expr0_occs.len();
        assert_occurrence_stream_aligns(
            to_name.as_str(),
            to_var,
            &variables,
            dim_ctx,
            ir,
            &expr0_occs,
        );
    }

    // Non-vacuity guard 2: even with every equation parsing, a fixture whose
    // targets reference no model variables (all-constant) produces an empty
    // occurrence stream model-wide, so the per-target length assertions all
    // compare 0 == 0. Every alignment fixture exists to exercise the SiteId-path
    // / shape zip on REAL occurrences, so require at least one.
    assert!(
        total_occurrences > 0,
        "alignment fixture produced NO occurrences model-wide: the SiteId-path / shape \
         assertions never run (every per-target stream is empty, so the length checks \
         pass 0 == 0)"
    );
}

/// Assert [`assert_lowering_matches_reparse`] for every slot of every variable
/// of `tp`'s `main` model, and return how many slots were checked so a caller
/// can prove the sweep was not vacuous.
fn assert_lowering_matches_reparse_everywhere(tp: &TestProject) -> usize {
    let datamodel = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let project = sync.project;

    // A fixture equation that fails to parse leaves its variable with NO AST, so
    // it contributes no slot and would silently shrink the sweep.
    let errors: Vec<_> = collect_all_diagnostics(&db, project)
        .into_iter()
        .filter(|d| d.severity == DiagnosticSeverity::Error)
        .collect();
    assert!(
        errors.is_empty(),
        "round-trip fixture has compile errors -- a variable that fails to lower \
         contributes no slot to the sweep: {errors:?}"
    );

    let variables = model_lowered_variables(&db, model, project);
    let mut names: Vec<&Ident<Canonical>> = variables.keys().collect();
    names.sort();
    let mut slots = 0usize;
    for name in names {
        let Some(ast) = variables[name].ast() else {
            continue;
        };
        let mut check = |slot: &str, expr: &crate::ast::Expr2| {
            assert_lowering_matches_reparse(&format!("{name} {slot}"), expr);
            slots += 1;
        };
        match ast {
            Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => check("body", expr),
            Ast::Arrayed(_, per_elem, default_expr, _) => {
                let mut keys: Vec<_> = per_elem.keys().collect();
                keys.sort();
                for k in keys {
                    check(k.as_str(), &per_elem[k]);
                }
                if let Some(default) = default_expr {
                    check("<default>", default);
                }
            }
        }
    }
    slots
}

#[test]
fn direct_lowering_matches_reparse_on_print_sensitive_shapes() {
    // The print->reparse deletion is byte-neutral only if the direct
    // `Expr2` -> `Expr0` lowering IS the reparse of the printed form. The shapes
    // most likely to break that are the ones where `print_eqn` renders something
    // the parser could plausibly re-associate differently: a negated constant
    // (`-3`, which could re-lex as a single negative literal), the zero-argument
    // builtins (printed `time()`, which could re-parse as a bare `Var`), the
    // word-spelled operators (`not`, `mod`), the two-character comparisons
    // (`<>`, `>=`), right-associative `^`, an `If` nested as an operand, the
    // variadic builtins, `LOOKUP`'s table argument, and every subscript index
    // form (wildcard, star-range, range, `@N` position, literal, dynamic).
    let curve = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![0.0, 5.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 5.0 },
    };
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Sub", &["nyc"])
        .indexed_dimension("Slot", 4)
        .scalar_aux("a", "2")
        .scalar_aux("b", "3")
        .scalar_aux("c", "4")
        .array_aux("pop[Region]", "10")
        .array_aux("wide[Slot]", "1")
        .scalar_aux("idx", "2")
        .aux_with_gf("curve", "a", curve)
        // Unary minus on a constant AND on a compound operand.
        .scalar_aux("negs", "-3 + -a * -(b + c)")
        // Zero-argument builtins.
        .scalar_aux("times", "TIME + TIME_STEP + INITIAL_TIME + FINAL_TIME + PI")
        // Word operators, two-character comparisons, right-associative `^`,
        // and an `If` in operand position.
        .scalar_aux("ops", "a mod b + a ^ b ^ c")
        .scalar_aux("cmps", "if (a <> b) AND (a >= c) then 1 else 0")
        .scalar_aux("nots", "if NOT (a > b) then 1 else 0")
        .scalar_aux(
            "nested_if",
            "a * (if a > b then (if b > c then 1 else 2) else 3)",
        )
        // Variadic / optional-argument builtins and the LOOKUP table slot.
        .scalar_aux(
            "builtins",
            "MAX(a, b) + MIN(a, b) + MEAN(a, b, c) + PULSE(a, b) + PULSE(a, b, c) \
             + SAFEDIV(a, b) + SAFEDIV(a, b, c) + LOOKUP(curve, a) + ABS(-a) + SQRT(a)",
        )
        // Every subscript index form.
        .scalar_aux("reduce_all", "SUM(pop[*])")
        .scalar_aux("reduce_sub", "SUM(pop[*:Sub])")
        .scalar_aux("literal_idx", "pop[nyc]")
        .scalar_aux("dyn_idx", "wide[idx + 1]")
        .scalar_aux("range_idx", "SUM(wide[1:3])")
        .scalar_aux("pos_idx", "wide[@2]")
        // A name the lexer cannot read bare, so `print_ident` quotes it.
        .array_aux("\"1pop\"[Region]", "5")
        .array_aux("quoted_reader[Region]", "\"1pop\" * 2");

    let slots = assert_lowering_matches_reparse_everywhere(&tp);
    // Non-vacuity: the sweep must actually have visited the fixture's variables
    // (a fixture whose equations all failed to lower would pass trivially).
    assert!(
        slots >= 20,
        "expected the print-sensitive fixture to contribute >= 20 slots, got {slots}"
    );
}

#[test]
fn align_already_lagged_and_index_nested_streams() {
    // Occurrences inside `PREVIOUS(...)`/`INIT(...)` (already-lagged) and inside
    // another reference's subscript index (index-nested) are the transform-only
    // contexts the reports (fig2 Q3/Q4) flag as the subtle SiteId-zip cases.
    // Both walkers must enumerate them in the SAME order for the zip to be sound.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .indexed_dimension("Slot", 4)
        .array_aux("pop[Region]", "100")
        .scalar_aux("g", "5")
        .scalar_aux("idx", "1")
        .array_aux("other[Slot]", "7")
        .array_aux("lagged[Region]", "pop + PREVIOUS(g) + INIT(g)")
        .array_aux("nested[Region]", "pop + other[idx]");
    assert_occurrence_streams_align(&tp);
}

#[test]
fn align_precedence_and_nesting_streams() {
    // The SiteId-zip bridge assumes the print->reparse round trip is
    // child-index-isomorphic to the Expr2 AST. Operator precedence,
    // parenthesization, unary minus, and `IF THEN ELSE` nesting are where a
    // printer/parser asymmetry would reshape the tree and desync the SiteId
    // path even when the flat occurrence stream still aligns positionally, so
    // stress those explicitly (the `align_*` fixtures assert full SiteId-path
    // equality, not just positional order).
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("a", "1")
        .scalar_aux("b", "2")
        .scalar_aux("c", "3")
        .scalar_aux("d", "4")
        .array_aux(
            "mixed[Region]",
            // The `IF ... THEN ... ELSE ...` operand is parenthesized because
            // the grammar accepts `IF` only at the head of an expression, never
            // as a bare binary operand (`parser::parse_expr`): the unparenthesized
            // form fails to parse, leaving `mixed` with no AST and an EMPTY
            // occurrence stream -- which would vacate this whole fixture (the
            // alignment loop would compare 0 == 0 and never run the SiteId-path /
            // shape assertions the fixture exists to stress). The parenthesized
            // form round-trips through the printer (an `If` child under an `Op2`
            // parent is always re-parenthesized, `ast::paren_if_necessary`).
            "a * (b + c) - -d + pop / (a - b * c) + (IF a > b THEN pop * c ELSE d - a)",
        );
    assert_occurrence_streams_align(&tp);
}

#[test]
fn align_reducer_and_lookup_streams() {
    // A `LOOKUP` table argument is skipped by both walkers; reducer arguments
    // are `in_reducer` on both; a multi-arg builtin (`MAX`) exercises the
    // `walk_builtin_expr` (Expr2) vs positional (Expr0) child ordering the A2b
    // bridge flags as the alignment subtlety.
    let curve = datamodel::GraphicalFunction {
        kind: datamodel::GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![0.0, 5.0],
        x_scale: datamodel::GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 5.0 },
    };
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("total", "SUM(pop[*])")
        .scalar_aux("driver", "3")
        .scalar_aux("g", "4")
        .aux_with_gf("curve", "driver", curve)
        .scalar_aux(
            "looked_up",
            "LOOKUP(curve, driver) + total + MAX(driver, g)",
        );
    assert_occurrence_streams_align(&tp);
}

#[test]
fn align_smooth_module_output_streams() {
    // A SMOOTH expands to a module whose output is a `module·port` composite:
    // the IR enumerates it as `OccurrenceRef::ModuleOutput` (filtered out of the
    // Variable stream), while the Expr0 walk records no site for it. The
    // remaining model-variable occurrences must still align, so the module
    // occurrence does not desync the zip.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 5.0, 1.0)
        .scalar_aux("level", "10")
        .scalar_aux("other", "2")
        .aux("smoothed", "SMTH1(level, 3) + other", None);
    assert_occurrence_streams_align(&tp);
}

/// Drive both classifier families over every causal edge of `tp`'s `main`
/// model and assert per-edge non-reducer shape agreement.
///
/// The Expr2 side is the production walker `collect_all_reference_sites` (what
/// `model_ltm_reference_sites` is built on); the Expr0 side is the production
/// classifier `classify_expr0_subscript_shape` on the production-printed,
/// reparsed target text. The edge set under test is anchored to
/// `model_ltm_reference_sites` (requirement: compare *through* the production
/// IR entry): its `sites` keys must equal the `(from, to)` pairs
/// `collect_all_reference_sites` yields, so the gate can neither invent nor
/// miss an edge the IR records.
///
/// Returns the per-edge non-reducer Expr2 multisets it compared (see
/// [`ComparedShapes`]) so a caller can [`pin_edge`] a concrete expected
/// multiset -- the permanent non-vacuity anchor. The agreement assertion alone
/// passes vacuously on an empty-vs-empty edge, so the pins are what keep a
/// symmetric harness-rot refactor from silently emptying the comparison.
fn assert_classifier_families_agree(tp: &TestProject) -> ComparedShapes {
    let datamodel = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let project = sync.project;

    let variables = model_lowered_variables(&db, model, project);
    let dim_ctx = project_dimensions_context(&db, project);

    // Anchor: the edges the IR (`model_ltm_reference_sites`) records must be
    // exactly the edges the walker yields, so the gate is exercising the same
    // production classification surface the IR consumes.
    let ir = model_ltm_reference_sites(&db, model, project);
    let ir_edges: std::collections::BTreeSet<(String, String)> = ir.sites.keys().cloned().collect();

    let mut walker_edges: std::collections::BTreeSet<(String, String)> = Default::default();
    // The non-reducer Expr2 multiset compared per edge, returned so a fixture
    // can pin a concrete expectation (the non-vacuity anchor).
    let mut compared: ComparedShapes = Default::default();
    // Sorted target order keeps any failure message deterministic.
    let mut to_names: Vec<&Ident<Canonical>> = variables.keys().collect();
    to_names.sort();

    for to_name in to_names {
        let to_var = &variables[to_name];
        let to_str = to_name.as_str();

        // Expr2 side: the production per-occurrence walker.
        let mut lookup_dims = |name: &str| -> Vec<Dimension> { source_dims_of(&variables, name) };
        let expr2_sites: HashMap<String, Vec<ReferenceSite>> =
            collect_all_reference_sites(to_var, &variables, dim_ctx, &mut lookup_dims);

        // Expr0 side: classify occurrences in the production-printed, reparsed
        // target text.
        let expr0_occs = expr0_occurrences_for_target(to_str, to_var, &variables);
        let mut expr0_by_source: HashMap<String, Vec<(RefShape, bool)>> = HashMap::new();
        for occ in &expr0_occs {
            let (shape, _axes) = classify_expr0_occurrence(occ, to_var, &variables, dim_ctx);
            expr0_by_source
                .entry(occ.source.clone())
                .or_default()
                .push((shape, occ.in_reducer));
        }

        // Stage-2 groundwork: the per-occurrence SiteId-zip bridge, corpus-validated.
        //
        // Stage 2 replaces the ceteris-paribus wrap's Expr0 re-classification
        // with a lookup into the occurrence IR (`OccurrenceSite`) by the
        // occurrence's structural position, so there is ONE classifier family.
        // That zip is sound only if the IR's per-occurrence stream
        // (`occurrences[to]`, from the Expr2 walk) and the reparsed-Expr0 stream
        // the wrap runs on ALIGN position-for-position. This is exactly the A2b
        // bridge assumption, which was previously validated only on paper. Assert
        // it here across every fixture: same length (no print/reparse GH #913
        // desync, no walker divergence), same source and `in_reducer` at each
        // position, and -- for DIRECT (non-reducer) references -- the same shape
        // the wrap would read from the IR instead of re-deriving. The reducer-arg
        // `StarRange`->`Wildcard` AC1.4 asymmetry (the one deliberate divergence,
        // module doc) is confined to `in_reducer` occurrences, so shape agreement
        // is asserted only off the reducer path, matching the multiset gate above.
        assert_occurrence_stream_aligns(to_str, to_var, &variables, dim_ctx, ir, &expr0_occs);

        // Every source of an edge, from either side.
        let mut sources: std::collections::BTreeSet<String> = Default::default();
        sources.extend(expr2_sites.keys().cloned());
        sources.extend(expr0_by_source.keys().cloned());

        for source in sources {
            walker_edges.insert((source.clone(), to_str.to_string()));

            // Non-reducer shapes only (see the module doc): a reducer argument
            // is excluded because both classifier families are
            // reducer-context-free, so its drift is already covered by the same
            // syntactic form outside a reducer -- NOT because its shape is
            // inconsequential (a Direct, not-hoisted reducer arg's shape is
            // consumed as `live_shape`; the module doc traces it).
            let expr2_shapes: Vec<RefShape> = expr2_sites
                .get(&source)
                .map(|sites| {
                    sites
                        .iter()
                        .filter(|s| !s.in_reducer)
                        .map(|s| s.shape.clone())
                        .collect()
                })
                .unwrap_or_default();
            let expr0_shapes: Vec<RefShape> = expr0_by_source
                .get(&source)
                .map(|v| {
                    v.iter()
                        .filter(|(_, in_reducer)| !in_reducer)
                        .map(|(shape, _)| shape.clone())
                        .collect()
                })
                .unwrap_or_default();

            compared.insert((source.clone(), to_str.to_string()), sorted(&expr2_shapes));

            assert!(
                edge_shapes_agree(&expr2_shapes, &expr0_shapes),
                "LTM classifier drift on edge {source} -> {to_str} in model '{}':\n  \
                 Expr2 (collect_all_reference_sites / model_ltm_reference_sites) \
                 non-reducer shapes: {:?}\n  \
                 Expr0 (classify_expr0_subscript_shape on printed+reparsed text) \
                 non-reducer shapes: {:?}\n  \
                 (the families must agree so the live reference survives the \
                 ceteris-paribus PREVIOUS-wrapping; disagreement silently zeroes \
                 the {source} -> {to_str} link score)",
                model.name(&db),
                sorted(&expr2_shapes),
                sorted(&expr0_shapes),
            );
        }
    }

    assert_eq!(
        ir_edges, walker_edges,
        "the reference-site IR (model_ltm_reference_sites) and the Expr2 walker \
         disagree on the causal-edge set; the gate must exercise exactly the \
         edges the IR records"
    );

    compared
}

// ── The fixture-driven sweep over the hard arrayed shapes ──────────────────

#[test]
fn agree_scalar_and_bare_a2a_refs() {
    // Scalar Bare (`derived = k * 3`) and A2A Bare (`births[Region] =
    // population * 0.1`, same-element).
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .scalar_aux("k", "10")
        .scalar_aux("derived", "k * 3")
        .array_aux("population[Region]", "100")
        .array_aux("births[Region]", "population * 0.1");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_fixed_index_named_element() {
    // All-pinned FixedIndex with a named element (`population[nyc]`) alongside
    // a bare same-element reference.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("population[Region]", "100")
        .array_aux("relative_pop[Region]", "population / population[nyc]");
    let compared = assert_classifier_families_agree(&tp);
    // Non-vacuity anchor: a named-element FixedIndex alongside a bare A2A
    // same-element reference.
    pin_edge(
        &compared,
        "population",
        "relative_pop",
        &[
            RefShape::Bare,
            RefShape::FixedIndex(vec!["nyc".to_string()]),
        ],
    );
}

#[test]
fn agree_indexed_dim_fixed_index_and_canonical_integer() {
    // Indexed-dimension FixedIndex with BOTH the canonical integer form (`[2]`)
    // and the non-canonical `[01]` form, which both families must reduce to
    // element "1" (the pop[01] canonicalization class). If either side stops
    // canonicalizing, `[01]` becomes DynamicIndex on that side and this fails
    // -- exactly the mutation-sensitivity probe.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .indexed_dimension("Dim", 3)
        .array_aux("pop[Dim]", "100")
        .array_aux("pick_two[Dim]", "pop / pop[2]")
        .array_aux("pick_one[Dim]", "pop / pop[01]");
    let compared = assert_classifier_families_agree(&tp);
    // Permanent non-vacuity anchor for the `pop[01]` canonicalization class:
    // both the canonical `[2]` and the non-canonical `[01]` must land in the
    // `FixedIndex` bucket alongside the bare same-element reference. This is
    // the mutation-sensitivity probe -- if either family stops canonicalizing
    // `01`, the `pick_one` occurrence moves to `DynamicIndex` on one side and
    // this pin (or the agreement assertion) fails loudly.
    pin_edge(
        &compared,
        "pop",
        "pick_one",
        &[RefShape::Bare, RefShape::FixedIndex(vec!["1".to_string()])],
    );
    pin_edge(
        &compared,
        "pop",
        "pick_two",
        &[RefShape::Bare, RefShape::FixedIndex(vec!["2".to_string()])],
    );
}

#[test]
fn agree_out_of_range_integer_literal_is_dynamic() {
    // An out-of-range integer literal (`pop[99]` over a size-3 indexed dim)
    // resolves to no element, so both families fall to DynamicIndex.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .indexed_dimension("Dim", 3)
        .array_aux("pop[Dim]", "100")
        .array_aux("oob[Dim]", "pop / pop[99]");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_iterated_dim_same_name_is_bare() {
    // GH #511: an iterated-dimension subscript on the live source
    // (`row_sum[Region]` inside a `Region x Age` target) reads the same
    // element -- Bare on both families.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("row_sum[Region]", "1")
        .array_aux("c", "2")
        .array_aux("growth[Region, Age]", "row_sum[Region] * c");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_mixed_iterated_and_pinned_is_per_element() {
    // GH #525 (T6): a mixed iterated + pinned subscript (`pop[Region, young]`
    // inside an A2A-over-`Region` target) is PerElement on both families --
    // and the `AxisRead` axes (dim / source_dim / Pinned element) must match.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("pop[Region, Age]", "100")
        .array_aux("young_share[Region]", "pop[Region, young] * 0.5");
    let compared = assert_classifier_families_agree(&tp);
    // Non-vacuity anchor for the GH #525 mixed iterated+pinned class: an
    // iterated `Region` axis mixed with a pinned `young` element is
    // `PerElement`, on BOTH families (including the `AxisRead` axis payload).
    pin_edge(
        &compared,
        "pop",
        "young_share",
        &[RefShape::PerElement {
            axes: vec![
                AxisRead::Iterated {
                    dim: "region".to_string(),
                    source_dim: "region".to_string(),
                },
                AxisRead::Pinned("young".to_string()),
            ],
        }],
    );
}

#[test]
fn agree_mapped_dim_forward_declaration_is_bare() {
    // GH #757 forward direction: source over `Region`, target iterates
    // `State`, `State` maps positionally to `Region`. `pop[State]` reads the
    // positionally-corresponding element -> Bare on both, via the shared
    // `iterated_axis_slot_elements` gate.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux("pop[Region]", "100")
        .array_aux("mapped[State]", "pop[State] * 2");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_mapped_dim_reverse_declaration_is_bare() {
    // GH #757 reverse direction: the mapping is declared `State -> Region`,
    // but the target iterates `Region` and reads a `State`-dimensioned source.
    // `executed_read_correspondence` accepts both declaration directions, so
    // both families classify `speed[Region]` as Bare.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_mapping("State", &["s1", "s2"], "Region")
        .array_aux("speed[State]", "100")
        .array_aux("reverse[Region]", "speed[Region] * 2");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_element_mapped_iterated_dim_is_bare() {
    // An EXPLICIT element map on the ITERATED spelling: `pop[State]` names the
    // dimension the equation iterates and is resolved name-first, then through
    // the map (GH #997); the declared lists reproduce that pairing (a mapped
    // pair), so both families classify Bare.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2"],
            "Region",
            &[("s1", "r2"), ("s2", "r1")],
        )
        .array_aux("pop[Region]", "100")
        .array_aux("mapped[State]", "pop[State] * 2");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_element_mapped_source_own_dim_is_per_element() {
    // GH #997's class-D shape, the OTHER spelling of the fixture above:
    // `pop[Region]` names the SOURCE's own dimension inside a `State`-iterating
    // equation, which execution resolves name-first then through the element
    // map. Both families must classify it `PerElement` with a `MappedRead`
    // axis -- the Expr0 mirror gains the same arm, so the agreement gate is
    // what keeps the test-support classifier from drifting.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2"],
            "Region",
            &[("s1", "r2"), ("s2", "r1")],
        )
        .array_aux("pop[Region]", "100")
        .array_aux("mapped[State]", "pop[Region] * 2");
    let compared = assert_classifier_families_agree(&tp);
    // The name promises a SHAPE, so pin it: agreement alone would be satisfied
    // by both families calling it `DynamicIndex`, which is what they did before
    // GH #997.
    pin_edge(
        &compared,
        "pop",
        "mapped",
        &[RefShape::PerElement {
            axes: vec![AxisRead::MappedRead {
                dim: "state".to_string(),
                source_dim: "region".to_string(),
            }],
        }],
    );
}

#[test]
fn agree_many_to_one_mapped_source_own_dim_is_per_element() {
    // The cardinality no positional derivation could describe (three target
    // elements, two source ones), reachable only through the executed rule --
    // C-LEARN's shape.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["r1", "r2"])
        .named_dimension_with_element_mapping(
            "State",
            &["s1", "s2", "s3"],
            "Region",
            &[("s1", "r1"), ("s2", "r1"), ("s3", "r2")],
        )
        .array_aux("pop[Region]", "100")
        .array_aux("mapped[State]", "pop[Region] * 2");
    let compared = assert_classifier_families_agree(&tp);
    pin_edge(
        &compared,
        "pop",
        "mapped",
        &[RefShape::PerElement {
            axes: vec![AxisRead::MappedRead {
                dim: "state".to_string(),
                source_dim: "region".to_string(),
            }],
        }],
    );
}

#[test]
fn agree_shared_element_names_mapped_source_own_dim_is_per_element() {
    // Both dimensions declare the same element names in a different order, and
    // the map is a third permutation: the executed rule stops at NAME identity.
    // The families must agree on the axis, not merely on the shape.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["e2", "e1"])
        .named_dimension_with_element_mapping(
            "State",
            &["e1", "e2"],
            "Region",
            &[("e1", "e2"), ("e2", "e1")],
        )
        .array_aux("pop[Region]", "100")
        .array_aux("mapped[State]", "pop[Region] * 2");
    let compared = assert_classifier_families_agree(&tp);
    pin_edge(
        &compared,
        "pop",
        "mapped",
        &[RefShape::PerElement {
            axes: vec![AxisRead::MappedRead {
                dim: "state".to_string(),
                source_dim: "region".to_string(),
            }],
        }],
    );
}

#[test]
fn agree_dynamic_index_expression() {
    // A non-literal dynamic index (`pop[idx]` with a scalar `idx`, and an
    // arithmetic index) is DynamicIndex on both families.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .indexed_dimension("Dim", 3)
        .scalar_aux("idx", "2")
        .array_aux("pop[Dim]", "100")
        .array_aux("dyn_ref[Dim]", "pop[idx] + pop[idx + 1]");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_dimension_name_index_gh759() {
    // GH #759: dimension-name indices. `matrix[D1, c1]` inside an A2A-over-`D1`
    // target mixes the iterated dimension name `D1` with the literal element
    // `c1` (PerElement); the pure-iterated `col[D1]` is Bare. Both families
    // must recognize the dimension name index rather than treating it as a
    // dynamic/variable reference.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("D1", &["a1", "a2"])
        .named_dimension("D2", &["c1", "c2"])
        .array_aux("matrix[D1, D2]", "100")
        .array_aux("col[D1]", "5")
        .array_aux("pick[D1]", "matrix[D1, c1] + col[D1]");
    let compared = assert_classifier_families_agree(&tp);
    // Non-vacuity anchor for the GH #759 dimension-name-index class: the
    // dimension name `D1` is an iterated axis (not a dynamic/variable index),
    // so `matrix[D1, c1]` is a mixed iterated+pinned `PerElement`, while the
    // pure-iterated `col[D1]` is `Bare`. A regression that treats a dimension
    // name as a dynamic index moves one of these to `DynamicIndex` on one side.
    pin_edge(
        &compared,
        "matrix",
        "pick",
        &[RefShape::PerElement {
            axes: vec![
                AxisRead::Iterated {
                    dim: "d1".to_string(),
                    source_dim: "d1".to_string(),
                },
                AxisRead::Pinned("c1".to_string()),
            ],
        }],
    );
    pin_edge(&compared, "col", "pick", &[RefShape::Bare]);
}

#[test]
fn agree_element_variable_name_collision() {
    // A model variable named identically to a dimension element (`nyc`): the
    // subscript `population[nyc]` selects the ELEMENT (execution resolves it to
    // a static offset, element taking priority over the like-named variable),
    // while the bare `nyc` outside the subscript is a genuine variable
    // reference. Both families must agree: `population -> collide` is a
    // FixedIndex site; `nyc -> collide` is ONE Bare site (the bare `* nyc`
    // multiplication), NOT two -- the index-nested `nyc` is an element
    // selector, not a causal reference. Before the A2a fix the Expr2 walker
    // minted an extra `nyc -> collide` Bare site from the subscript index that
    // disagreed with the transform; both families now skip it. That site was an
    // orphan -- variable-level dep extraction already filtered the element
    // token, so no keyed consumer ever read it -- not a consumer-visible edge.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .scalar_aux("nyc", "3")
        .array_aux("population[Region]", "100")
        .array_aux("collide[Region]", "population[nyc] * nyc");
    let compared = assert_classifier_families_agree(&tp);
    // Non-vacuity anchor: exactly one Bare `nyc -> collide` site (the bare
    // multiplication), not the pre-fix two. If the element-selector skip
    // regresses on either family, this pin (or the agreement assertion) fails.
    pin_edge(&compared, "nyc", "collide", &[RefShape::Bare]);
    pin_edge(
        &compared,
        "population",
        "collide",
        &[RefShape::FixedIndex(vec!["nyc".to_string()])],
    );
}

#[test]
fn agree_arrayed_per_element_slots() {
    // GH #913 surface: an `Ast::Arrayed` per-element-equation target whose
    // slots reference the source by literal element subscripts. Each slot is
    // printed and reparsed independently; both families classify the per-slot
    // `pop[nyc]` / `pop[boston]` as FixedIndex.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "100")
        .array_with_default_and_overrides(
            "diff[Region]",
            "0",
            vec![
                ("nyc", "pop[nyc] - pop[boston]"),
                ("boston", "pop[boston] - pop[nyc]"),
            ],
        );
    let compared = assert_classifier_families_agree(&tp);
    // Non-vacuity anchor for the GH #913 printer/parser class: each of the two
    // `Ast::Arrayed` slots is printed and reparsed independently, so all four
    // literal-element occurrences (`pop[nyc]`/`pop[boston]` in each slot) must
    // survive the round trip as `FixedIndex`. A print path that drops or
    // reshapes an occurrence changes this multiset on the Expr0 side.
    pin_edge(
        &compared,
        "pop",
        "diff",
        &[
            RefShape::FixedIndex(vec!["boston".to_string()]),
            RefShape::FixedIndex(vec!["boston".to_string()]),
            RefShape::FixedIndex(vec!["nyc".to_string()]),
            RefShape::FixedIndex(vec!["nyc".to_string()]),
        ],
    );
}

#[test]
fn agree_references_inside_reducers_hoisted_and_declined() {
    // References inside reducers exercise the wildcard / all-StarRange /
    // partial-StarRange / dynamic-index-reducer classifier paths. They are
    // `in_reducer` and excluded from the strict per-edge comparison -- not
    // because their shape is inconsequential, but because both classifiers are
    // reducer-context-free, so their drift is subsumed by the non-reducer
    // sweep (`agree_dynamic_index_expression`; see the module doc). They still
    // drive `collect_all_reference_sites`, and a direct (non-reducer)
    // reference alongside them (`pop / total`) must still agree.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Sub", &["nyc"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("total", "SUM(pop[*])")
        .scalar_aux("total_star", "SUM(pop[*:Region])")
        .scalar_aux("total_sub", "SUM(pop[*:Sub])")
        .scalar_aux("idxs", "1")
        .scalar_aux("dyn_reduce", "SUM(pop[idxs])")
        .array_aux("share[Region]", "pop / total");
    assert_classifier_families_agree(&tp);
}

#[test]
fn agree_reducer_wildcard_is_excluded_but_classified_wildcard() {
    // AC1.4 coverage: the all-StarRange reducer argument `pop[*:Region]`
    // classifies `Wildcard` on the Expr2 side (the whole-extent rule) and is
    // `in_reducer`, so it is excluded from the strict comparison. This pins
    // that the excluded occurrence really is the documented AC1.4 asymmetry --
    // an `in_reducer` Wildcard -- not a silently-dropped non-reducer edge.
    let tp = TestProject::new("main")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "100")
        .scalar_aux("total_star", "SUM(pop[*:Region])");

    let datamodel = tp.build_datamodel();
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &datamodel);
    let model = sync.models["main"].source;
    let project = sync.project;
    let variables = model_lowered_variables(&db, model, project);
    let dim_ctx = project_dimensions_context(&db, project);

    let to_var = &variables[&Ident::<Canonical>::new("total_star")];
    let mut lookup_dims = |name: &str| -> Vec<Dimension> { source_dims_of(&variables, name) };
    let sites = collect_all_reference_sites(to_var, &variables, dim_ctx, &mut lookup_dims);
    let pop_sites = &sites["pop"];
    assert_eq!(pop_sites.len(), 1, "sites: {pop_sites:?}");
    assert_eq!(pop_sites[0].shape, RefShape::Wildcard);
    assert!(
        pop_sites[0].in_reducer,
        "the all-StarRange reducer arg must be in_reducer so it is excluded \
         from the non-reducer comparison (the AC1.4 asymmetry lives here)"
    );

    // And the full gate still passes for this model (the excluded occurrence
    // leaves an empty non-reducer multiset on both sides). Pin that emptiness
    // explicitly: this fixture is deliberately vacuous at the non-reducer
    // comparison (the AC1.4 exclusion is its whole point), so its `pop ->
    // total_star` edge is the ONE place an empty compared multiset is correct
    // rather than a symptom of harness rot.
    let compared = assert_classifier_families_agree(&tp);
    pin_edge(&compared, "pop", "total_star", &[]);
}

// ── Sensitivity proof of the comparison logic itself ───────────────────────

#[test]
fn comparison_reports_mismatched_shapes() {
    // The comparison core must report disagreement when the two families
    // classify the same occurrence differently -- the exact drift shape the
    // pop[01] canonicalization bug produces (Expr2 FixedIndex(["1"]) vs Expr0
    // DynamicIndex). This is the permanent harness-level negative test: it
    // proves `edge_shapes_agree` (and therefore the gate) is sensitive to a
    // real classifier mismatch independent of any fixture.
    let expr2 = vec![RefShape::Bare, RefShape::FixedIndex(vec!["1".to_string()])];
    let expr0_drifted = vec![RefShape::Bare, RefShape::DynamicIndex];
    assert!(
        !edge_shapes_agree(&expr2, &expr0_drifted),
        "the comparison must flag a FixedIndex-vs-DynamicIndex drift"
    );

    // Order-insensitivity: the same multiset in a different order agrees.
    let expr0_ok = vec![RefShape::FixedIndex(vec!["1".to_string()]), RefShape::Bare];
    assert!(
        edge_shapes_agree(&expr2, &expr0_ok),
        "a reordered identical multiset must agree"
    );
}
