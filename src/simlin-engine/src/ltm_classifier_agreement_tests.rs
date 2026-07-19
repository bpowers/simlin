// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Track-A differential gate: the two LTM access-shape classifier families
//! agree on every non-reducer reference occurrence.
//!
//! # The failure mode this gate retires
//!
//! An LTM causal edge's access shape is decided TWICE, on two ASTs, by two
//! mirrored classifier families:
//!
//! * **Expr2 side** ([`crate::db::ltm_ir`]) -- `resolve_literal_index` /
//!   `classify_subscript_shape` / `classify_iterated_dim_shape`, composed by
//!   the production walker `collect_all_reference_sites` and consumed through
//!   the salsa query `model_ltm_reference_sites`. This drives causal-edge
//!   emission and the per-shape link-score selection: `emit_per_shape_link_scores`
//!   feeds each site's `shape` into the partial builder as `live_shape`.
//! * **Expr0 side** ([`super`]) -- `resolve_literal_element_index` /
//!   `classify_expr0_subscript_shape` / `classify_expr0_per_element_axes` /
//!   `is_live_source_iterated_dim_subscript`, consumed by
//!   `wrap_non_matching_in_previous` on the *printed-and-reparsed* target
//!   equation text. This re-derives each occurrence's shape and keeps live
//!   only the occurrences whose shape EQUALS `live_shape`.
//!
//! If the families drift, no Expr0 occurrence matches `live_shape`, every
//! reference to the source is PREVIOUS-wrapped, and the link score silently
//! collapses to a constant (see the "must stay in sync" docstring on
//! `super::resolve_literal_element_index`). Historical drift bugs, all of
//! this class: GH #759 (dimension-name indices), GH #913 (printer/parser
//! asymmetry dropping attribution), the `pop[01]` indexed-literal
//! canonicalization.
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
//!   `source_dim_elements` the production caller (`link_score_equation_text_shaped`
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
//!   the partial builder; `wrap_non_matching_in_previous`'s reducer arm then
//!   keeps the reducer live only if the in-reducer occurrence's Expr0 shape
//!   (`expr0_contains_live_match` -> `classify_expr0_subscript_shape`) EQUALS
//!   that `live_shape`, so a drift on that occurrence WOULD silently zero the
//!   score. The exclusion is sound for a different reason: both classifier
//!   families are reducer-context-FREE (`classify_subscript_shape` /
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
    SimlinDb, project_dimensions_context, reconstruct_model_variables, sync_from_datamodel,
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
    variables: &HashMap<Ident<Canonical>, Variable>,
    in_reducer: bool,
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
                });
            }
        }
        Expr0::Subscript(raw, indices, _) => {
            let canonical = Ident::<Canonical>::new(raw.as_str());
            if variables.contains_key(&canonical) {
                out.push(Expr0Occurrence {
                    source: canonical.as_str().to_string(),
                    indices: Some(indices.clone()),
                    in_reducer,
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
                match idx {
                    IndexExpr0::Expr(e) => collect_expr0_occurrences(e, variables, in_reducer, out),
                    IndexExpr0::Range(l, r, _) => {
                        collect_expr0_occurrences(l, variables, in_reducer, out);
                        collect_expr0_occurrences(r, variables, in_reducer, out);
                    }
                    IndexExpr0::Wildcard(_)
                    | IndexExpr0::StarRange(_, _)
                    | IndexExpr0::DimPosition(_, _) => {}
                }
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
                collect_expr0_occurrences(arg, variables, child_in_reducer, out);
            }
        }
        Expr0::Op1(_, inner, _) => collect_expr0_occurrences(inner, variables, in_reducer, out),
        Expr0::Op2(_, l, r, _) => {
            collect_expr0_occurrences(l, variables, in_reducer, out);
            collect_expr0_occurrences(r, variables, in_reducer, out);
        }
        Expr0::If(cond, then_e, else_e, _) => {
            collect_expr0_occurrences(cond, variables, in_reducer, out);
            collect_expr0_occurrences(then_e, variables, in_reducer, out);
            collect_expr0_occurrences(else_e, variables, in_reducer, out);
        }
    }
}

/// The Expr2 dimension list of a source variable (empty for a scalar / absent
/// source), the shared input both `source_dim_names` and `source_dim_elements`
/// derive from.
fn source_dims_of(variables: &HashMap<Ident<Canonical>, Variable>, source: &str) -> Vec<Dimension> {
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

/// Classify one reparsed Expr0 occurrence of `source` with the production
/// Expr0 classifier, using the SAME context construction the production caller
/// builds. A bare occurrence is `Bare` by construction (matching the Expr2
/// walker's `Var` arm). `dep_dims` is `None`: it feeds only the *non-live-dep*
/// collapse in `classify_other_dep_iterated_dim_subscript`, never the live
/// source's shape, which is all this classifies.
fn classify_expr0_occurrence(
    occ: &Expr0Occurrence,
    to_var: &Variable,
    variables: &HashMap<Ident<Canonical>, Variable>,
    dim_ctx: &DimensionsContext,
) -> RefShape {
    let Some(indices) = &occ.indices else {
        return RefShape::Bare;
    };
    let source_dims = source_dims_of(variables, &occ.source);
    let source_dim_names: Vec<String> = source_dims.iter().map(|d| d.name().to_string()).collect();
    let source_dim_elements: Vec<Vec<String>> =
        source_dims.iter().map(dimension_element_names).collect();
    let target_iterated_dims = target_iterated_dims_of(to_var);
    let iter_ctx = IteratedDimCtx {
        source_dim_names: &source_dim_names,
        target_iterated_dims: &target_iterated_dims,
        dim_ctx: Some(dim_ctx),
        dep_dims: None,
    };
    classify_expr0_subscript_shape(indices, &source_dim_elements, Some(&iter_ctx))
}

/// The reparsed `Expr0` occurrences of every model variable in one target
/// equation, built from the target's Expr2 AST printed with the PRODUCTION
/// print path (`crate::patch::expr2_to_string`) and reparsed -- the same text
/// `link_score_equation_text_shaped` feeds the Expr0 partial builder. An
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
    variables: &HashMap<Ident<Canonical>, Variable>,
) -> Vec<Expr0Occurrence> {
    let mut out = Vec::new();
    let Some(ast) = to_var.ast() else {
        return out;
    };
    let mut walk_text = |text: &str| {
        let parsed = Expr0::new(text, crate::lexer::LexerType::Equation).unwrap_or_else(|e| {
            panic!("target '{to_name}' printed text failed to reparse (GH #913 drift class): text={text:?}, err={e:?}")
        });
        if let Some(expr0) = parsed {
            collect_expr0_occurrences(&expr0, variables, false, &mut out);
        }
    };
    match ast {
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
            walk_text(&crate::patch::expr2_to_string(expr));
        }
        Ast::Arrayed(_, per_elem, default_expr, _) => {
            // Deterministic slot order (matches the sorted walk in
            // `collect_all_reference_sites`); order does not affect the
            // per-edge multiset but keeps failure output stable.
            let mut keys: Vec<_> = per_elem.keys().collect();
            keys.sort();
            for k in keys {
                walk_text(&crate::patch::expr2_to_string(&per_elem[k]));
            }
            if let Some(default) = default_expr {
                walk_text(&crate::patch::expr2_to_string(default));
            }
        }
    }
    out
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
    variables: &HashMap<Ident<Canonical>, Variable>,
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
        assert_eq!(
            ir_occ.in_reducer, e0_occ.in_reducer,
            "occurrence-stream in_reducer mismatch for {ir_src} -> {to_str}"
        );
        if !ir_occ.in_reducer {
            let e0_shape = classify_expr0_occurrence(e0_occ, to_var, variables, dim_ctx);
            assert_eq!(
                ir_occ.shape, e0_shape,
                "occurrence-stream SHAPE mismatch for {ir_src} -> {to_str}: the wrap's IR lookup \
                 would return a shape the Expr0 classifier disagrees with"
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

    let variables = reconstruct_model_variables(&db, model, project);
    let dim_ctx = project_dimensions_context(&db, project);
    let ir = model_ltm_reference_sites(&db, model, project);

    let mut to_names: Vec<&Ident<Canonical>> = variables.keys().collect();
    to_names.sort();
    for to_name in to_names {
        let to_var = &variables[to_name];
        let expr0_occs = expr0_occurrences_for_target(to_name.as_str(), to_var, &variables);
        assert_occurrence_stream_aligns(
            to_name.as_str(),
            to_var,
            &variables,
            dim_ctx,
            ir,
            &expr0_occs,
        );
    }
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

    let variables = reconstruct_model_variables(&db, model, project);
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
            let shape = classify_expr0_occurrence(occ, to_var, &variables, dim_ctx);
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
    // `mapped_element_correspondence` accepts both declaration directions, so
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
fn agree_element_mapped_dim_declines_to_dynamic() {
    // GH #756 positional-only gate: an EXPLICIT element map (not positional) is
    // declined by `mapped_element_correspondence`, so `pop[State]` keeps the
    // conservative DynamicIndex on both families (not Bare).
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
    let variables = reconstruct_model_variables(&db, model, project);
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
