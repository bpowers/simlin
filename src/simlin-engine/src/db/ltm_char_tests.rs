// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Characterization pins for the LTM synthetic-equation generators that the
//! Track A "invert the composition" restructuring touches
//! (`generate_per_element_link_equation`, `generate_agg_to_scalar_target_equation`,
//! `generate_scalar_to_element_equation`, and the `Ast::Arrayed`-slot path in
//! `build_arrayed_link_score_equation`).
//!
//! These tests pin the EXACT generated `LtmSyntheticVar` equation text for a
//! battery of models that exercise every affected generator. They define
//! "behavior preserved": the restructuring (which moves row-pinning and
//! agg-substitution from PRE-transform rewrites to POST-transform lowerings)
//! must keep every byte of this text identical. A divergence here is either a
//! regression to fix or an adjudicated, documented, semantics-preserving change.
//!
//! The pins go through the production salsa entry point
//! (`model_ltm_variables`), so they exercise the full caller-to-generator data
//! flow (`db::ltm::link_scores`), not the generators in isolation.

use super::*;
use crate::datamodel;
use crate::test_common::TestProject;

/// Render one synthetic variable's equation as a stable, fully-structured
/// string: `Scalar`/`ApplyToAll` are their text; `Arrayed` lists each
/// `element => eqn` slot (element-sorted, as the generator emits) plus the
/// optional `default => eqn`. Dimensions are prefixed so a shape change is
/// visible in the pin.
fn render_equation(eq: &datamodel::Equation) -> String {
    match eq {
        datamodel::Equation::Scalar(s) => format!("scalar: {s}"),
        datamodel::Equation::ApplyToAll(dims, s) => {
            format!("a2a[{}]: {s}", dims.join(","))
        }
        datamodel::Equation::Arrayed(dims, elements, default, apply_default) => {
            let mut out = format!(
                "arrayed[{}] (apply_default={apply_default}):",
                dims.join(",")
            );
            for (elem, eqn, _, _) in elements {
                out.push_str(&format!("\n    {elem} => {eqn}"));
            }
            if let Some(default_eqn) = default {
                out.push_str(&format!("\n    <default> => {default_eqn}"));
            }
            out
        }
    }
}

/// Deterministic dump of every synthetic variable whose name matches
/// `filter`, sorted by name, one `name` line followed by its rendered
/// equation. Used as the characterization surface: the whole string is
/// pinned byte-for-byte.
fn dump_synthetic_vars(project: datamodel::Project, discovery: bool, filter: &str) -> String {
    use salsa::Setter;
    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    if discovery {
        source_project.set_ltm_discovery_mode(&mut db).to(true);
    }
    let ltm = model_ltm_variables(&db, model, source_project);
    let mut vars: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| v.name.contains(filter))
        .collect();
    vars.sort_by(|a, b| a.name.cmp(&b.name));
    let mut out = String::new();
    for v in vars {
        out.push_str(&v.name);
        if !v.dimensions.is_empty() {
            out.push_str(&format!("  dims=[{}]", v.dimensions.join(",")));
        }
        out.push('\n');
        out.push_str(&render_equation(&v.equation));
        out.push('\n');
    }
    out
}

/// Compare `actual` against the committed golden file `ltm_char_golden/{name}.txt`,
/// the byte-exact behavior pin. Set `UPDATE_LTM_GOLDEN=1` to (re)capture a
/// golden -- but only after the reviewer has adjudicated the change, per the
/// Track A stage-1 contract (an adjusted pin needs a documented
/// semantic-equivalence argument). A missing golden fails loudly.
#[track_caller]
fn assert_golden(name: &str, actual: &str) {
    let path = format!(
        "{}/src/db/ltm_char_golden/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var("UPDATE_LTM_GOLDEN").is_ok() {
        let dir = format!("{}/src/db/ltm_char_golden", env!("CARGO_MANIFEST_DIR"));
        std::fs::create_dir_all(&dir).expect("create golden dir");
        std::fs::write(&path, actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("missing golden {path}: {e}; run once with UPDATE_LTM_GOLDEN=1 to capture")
    });
    if actual != expected {
        eprintln!("\n===== GOLDEN MISMATCH ({name}): actual below =====");
        eprintln!("{actual}");
        eprintln!("===== end actual (expected in {path}) =====\n");
    }
    assert_eq!(actual, &expected, "golden mismatch for {name}");
}

// ---------------------------------------------------------------------------
// Model A: per-element (PerElement) link scores (GH #525 mixed iterated+literal)
//
// `growth[Region]` reads `pop[Region, young]` -- a mixed subscript: the Region
// axis is iterated (matches the A2A target dim), the Age axis is pinned to the
// literal `young`. This is `RefShape::PerElement`, routed to
// `emit_per_element_link_scores` -> `generate_per_element_link_equation`.
// ---------------------------------------------------------------------------

fn per_element_model() -> datamodel::Project {
    TestProject::new("per_element_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("pop[Region,Age]", "10")
        .array_flow("growth[Region]", "pop[Region, young] * 0.1", None)
        .array_stock("stock[Region]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_link_scores() {
    let actual = dump_synthetic_vars(per_element_model(), true, "link_score\u{205A}pop");
    assert_golden("per_element_link_scores", &actual);
}

// ---------------------------------------------------------------------------
// Model A2: per-element target over >=2 dims whose body also reads OTHER
// arrayed deps of (a) subset dims and (b) equal-arity reordered dims.
//
// `growth[Region,Age] = pop[Region, young] * w[Age] * v[Age, Region]`:
//   * `pop[Region, young]` is the emitting `PerElement` site (Region iterated,
//     Age pinned to `young`) -- the Iterated axis is a STRICT SUBSET of the
//     target's `Region x Age` dims (the broadcast case).
//   * `w[Age]` is an arrayed dep whose declared dims are a strict subset of the
//     target's (an iterated-dim subscript over a single dim).
//   * `v[Age, Region]` is an arrayed dep of EQUAL arity but REORDERED axes
//     relative to the target's `Region, Age` iteration order.
//
// This is the other-dep arm of the inverted per-element wrap. It regression-
// guards that the ceteris-paribus wrap does NOT collapse these iterated other-
// dep subscripts to a bare `PREVIOUS(dep)` (which the post-transform element
// pin would then over-subscript with the FULL target tuple -- arity 2 over a
// 1-D `w`, a fragment that fails to compile and silently zeroes the score).
// Each dep must keep its subscript so the pin resolves each dimension-name
// index to this element's own coordinate: `w[age·y]` (arity 1) and
// `v[age·y, region·r]` (declared order).
// ---------------------------------------------------------------------------

fn per_element_other_deps_model() -> datamodel::Project {
    TestProject::new("per_element_other_deps_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("pop[Region,Age]", "10")
        .array_aux("w[Age]", "0.5")
        .array_aux("v[Age,Region]", "0.25")
        .array_flow(
            "growth[Region,Age]",
            "pop[Region, young] * w[Age] * v[Age, Region]",
            None,
        )
        .array_stock("stock[Region,Age]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_other_deps() {
    let actual = dump_synthetic_vars(
        per_element_other_deps_model(),
        true,
        "link_score\u{205A}pop",
    );
    assert_golden("per_element_other_deps", &actual);
}

// ---------------------------------------------------------------------------
// Finding 1 materiality guard: a DYNAMIC feedback model whose per-element
// target body reads a SUBSET-dims arrayed dep. Unlike the constant
// characterization models above (whose text is pinned but whose scores are 0
// because nothing changes), this one drives `growth` through a real feedback
// loop so the `pop[Region,young] -> growth[Region,Age]` link score is genuinely
// non-zero -- so the pre-fix over-subscription (`w[region·r, age·y]`, arity 2
// over a 1-D `w`) is caught end-to-end: it made the fragment fail to compile
// (four `Assembly` `Warning`s) and silently zeroed every score series. This is
// the "pin gap hides a behavior change" guard: the text golden alone would not
// have distinguished the silent-0 runtime consequence.
// ---------------------------------------------------------------------------

fn per_element_subset_dep_feedback_model() -> datamodel::Project {
    TestProject::new("per_element_subset_dep_feedback")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("w[Age]", "0.5")
        // The Region-young cohort's population drives growth of every element;
        // `w[Age]` is a subset-dims dep whose pre-fix collapse broke the score.
        .array_flow(
            "growth[Region,Age]",
            "pop[Region, young] * w[Age] * 0.01",
            None,
        )
        .array_stock("pop[Region,Age]", "10", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn per_element_subset_dep_scores_are_live_not_silent_zero() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = per_element_subset_dep_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    // No LTM synthetic fragment may fail to compile: the pre-fix arity-2
    // `PREVIOUS(w[region·r, age·y])` over a 1-D `w` surfaced four of these.
    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .map(|d| d.variable.clone().unwrap_or_default())
        .collect();
    assert!(
        frag_failures.is_empty(),
        "per-element link-score fragments must all compile (no silent \
         constant-0 stub); failed: {frag_failures:?}"
    );

    // ...and the compiled scores must actually be non-zero: a fragment that
    // "compiles" to no bytecode reads a constant 0, so a series check is the
    // ground-truth guard the diagnostic check backstops.
    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed");
    let score_offsets: Vec<usize> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| k.as_str().contains("link_score\u{205A}pop"))
        .map(|(_, v)| *v)
        .collect();
    assert_eq!(
        score_offsets.len(),
        4,
        "expected four per-element pop->growth link scores in offsets"
    );
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    for off in score_offsets {
        let has_nonzero = results.iter().any(|row| row[off] != 0.0);
        assert!(
            has_nonzero,
            "per-element pop->growth link score at offset {off} is all-zero -- \
             the silent-degradation regression"
        );
    }
}

// ---------------------------------------------------------------------------
// Model A3: per-element target whose body carries an additional pop occurrence
// of DIFFERENT shape -- an all-iterated (`Bare`) live-source reference --
// alongside the emitting `PerElement` site.
//
// `mixed[Region,Age] = pop[Region, young] * pop[Region, Age]`:
//   * `pop[Region, young]` is the emitting `PerElement` site.
//   * `pop[Region, Age]` is an all-iterated (`Bare`) occurrence -- a DIFFERENT
//     shape, so the wrap freezes it (collapsing the iterated subscript to a
//     bare `PREVIOUS(pop)` and same-element-pinning it via the LIVE-SOURCE
//     path, which the other-dep fix leaves untouched) -- and it also seeds its
//     own Bare A2A score.
//
// Pins that the frozen live-source occurrence lowers to the correct
// full-arity per-element read (`PREVIOUS(pop[region·r, age·a])`) regardless of
// the other-dep collapse decision: byte-for-byte stable coverage guarding
// against future drift in the live-source path.
// ---------------------------------------------------------------------------

fn per_element_mixed_occurrences_model() -> datamodel::Project {
    TestProject::new("per_element_mixed_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("pop[Region,Age]", "10")
        .array_flow(
            "mixed[Region,Age]",
            "pop[Region, young] * pop[Region, Age]",
            None,
        )
        .array_stock("stock[Region,Age]", "0", &["mixed"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_mixed_occurrences() {
    let actual = dump_synthetic_vars(
        per_element_mixed_occurrences_model(),
        true,
        "link_score\u{205A}pop",
    );
    assert_golden("per_element_mixed_occurrences", &actual);
}

// ---------------------------------------------------------------------------
// Model A4 (finding 1): per-element target over positionally-MAPPED dims whose
// body carries an all-iterated occurrence of the live source alongside the
// emitting `PerElement` site.
//
// `growth[State,Age] = pop[State, young] * pop[State, Age] * 0.01`, with `State`
// positionally mapped to `Region` and `pop` declared over `Region x Age`:
//   * `pop[State, young]` is the emitting `PerElement` site (State iterated and
//     mapped to Region, Age pinned to `young`).
//   * `pop[State, Age]` is an all-iterated (`Bare`-shaped) occurrence whose
//     `State` axis is a MAPPED reference to `pop`'s declared `Region` axis.
//
// The wrap freezes the all-iterated occurrence at `PREVIOUS`. The row-pinning
// lowering must resolve its `State` index through the `State -> Region`
// correspondence, yielding `PREVIOUS(pop[region·<mapped>, age·<age>])`. The
// pre-fix inverted composition collapsed the iterated live-source subscript to
// a bare `PREVIOUS(pop)` BEFORE the lowering could see its indices, and the
// bare-`Var` pin -- which projects by the source's OWN dims -- found no row in
// the target-keyed projection, leaving an over-arity `PREVIOUS(pop)` that fails
// to compile and silently zeroes the score. The suppression of the live-source
// collapse under the `PerElement` live shape keeps the subscript alive so the
// mapped projection resolves. Byte-identical to HEAD.
// ---------------------------------------------------------------------------

fn per_element_mapped_occurrence_model() -> datamodel::Project {
    TestProject::new("per_element_mapped_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension_with_mapping("State", &["west", "east"], "Region")
        .array_aux("pop[Region,Age]", "10")
        .array_flow(
            "growth[State,Age]",
            "pop[State, young] * pop[State, Age] * 0.01",
            None,
        )
        .array_stock("stock[State,Age]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_mapped_occurrence() {
    let actual = dump_synthetic_vars(
        per_element_mapped_occurrence_model(),
        true,
        "link_score\u{205A}pop",
    );
    assert_golden("per_element_mapped_occurrence", &actual);
}

// Finding 1 materiality guard: the mapped analogue of
// `per_element_subset_dep_scores_are_live_not_silent_zero`. A DYNAMIC model
// whose per-element target reads the live source through a positionally-MAPPED
// all-iterated occurrence: `pop` grows over time (a stock fed by a
// Region-dimensioned inflow -- no mapping in the flow->stock wiring, only in
// the `growth[State,*]` reads), so the `pop[State,young] -> growth[State,Age]`
// per-element link scores are genuinely non-zero. Pre-fix, the wrap collapsed
// the mapped `pop[State,Age]` occurrence to a bare `PREVIOUS(pop)` (arity 2 over
// the scalar fragment) that failed to compile -- four `Assembly` warnings and
// all-zero score series. The constant-`pop` text golden alone could not catch
// that runtime consequence.

fn per_element_mapped_feedback_model() -> datamodel::Project {
    TestProject::new("per_element_mapped_feedback")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension_with_mapping("State", &["west", "east"], "Region")
        .array_stock("pop[Region,Age]", "10", &["popinflow"], &[], None)
        .array_flow("popinflow[Region,Age]", "1", None)
        .array_flow(
            "growth[State,Age]",
            "pop[State, young] * pop[State, Age] * 0.01",
            None,
        )
        .array_stock("stock[State,Age]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn per_element_mapped_occurrence_scores_are_live_not_silent_zero() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = per_element_mapped_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    // Discovery mode scores every causal edge, so the `pop -> growth`
    // per-element scores are emitted without needing a mapped flow->stock loop
    // (which the dimension checker would reject).
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    // No LTM synthetic fragment may fail to compile: the pre-fix collapse
    // produced arity-2 `PREVIOUS(pop)` fragments that surfaced four of these.
    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .map(|d| d.variable.clone().unwrap_or_default())
        .collect();
    assert!(
        frag_failures.is_empty(),
        "per-element mapped-occurrence link-score fragments must all compile \
         (no silent constant-0 stub); failed: {frag_failures:?}"
    );

    // ...and the per-element scores must actually be non-zero. Filter on the
    // `->growth[<elem>]` per-element targets so the Bare A2A `pop->growth` slots
    // and the `popinflow->pop` edge are excluded.
    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed");
    let score_offsets: Vec<usize> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| {
            k.as_str().contains("link_score\u{205A}pop[") && k.as_str().contains("\u{2192}growth[")
        })
        .map(|(_, v)| *v)
        .collect();
    assert_eq!(
        score_offsets.len(),
        4,
        "expected four per-element mapped pop->growth link scores in offsets"
    );
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    for off in score_offsets {
        let has_nonzero = results.iter().any(|row| row[off] != 0.0);
        assert!(
            has_nonzero,
            "mapped per-element pop->growth link score at offset {off} is all-zero \
             -- the silent-degradation regression"
        );
    }
}

// ---------------------------------------------------------------------------
// Model A5 (Track A3 stage 1, finding 1): per-element target whose body carries
// an all-`Pinned` (literal) live-source occurrence whose element name belongs to
// MULTIPLE dimensions.
//
// `growth[Region,Age] = pop[Region, young] * pop[boston, old]`, with `boston`
// declared by BOTH `Region` and `Cities`:
//   * `pop[Region, young]` is the emitting `PerElement` site.
//   * `pop[boston, old]` is an all-`Pinned` (FixedIndex) occurrence. `boston` is
//     ambiguous (Region + Cities), so the wrap's `qualify_element_index` (which
//     only qualifies a name declared by exactly ONE dimension) declines it.
//
// The row-pinning lowering must still fully qualify the frozen occurrence from
// the SOURCE's own declared dims (`pop` is `Region x Age`), yielding
// `PREVIOUS(pop[region·boston, age·old])` -- NOT the half-qualified
// `PREVIOUS(pop[boston, age·old])` (where the wrap qualified `old -> age·old`
// but left the ambiguous `boston` bare, and the lowering's per-axis classifier
// could no longer classify the half-qualified subscript to re-qualify it). The
// fix suppresses the wrap's generic index qualification for the live source's
// own frozen subscript on the `PerElement` path, so the lowering owns
// qualification via `from_dims` (the single, ambiguity-free owner). Byte-
// identical to HEAD `f057ef38`.
// ---------------------------------------------------------------------------

fn per_element_ambiguous_pin_model() -> datamodel::Project {
    TestProject::new("per_element_ambiguous_pin_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Cities", &["boston", "la"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("pop[Region,Age]", "10")
        .array_flow(
            "growth[Region,Age]",
            "pop[Region, young] * pop[boston, old]",
            None,
        )
        .array_stock("stock[Region,Age]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_ambiguous_pin() {
    let actual = dump_synthetic_vars(
        per_element_ambiguous_pin_model(),
        true,
        "link_score\u{205A}pop",
    );
    assert_golden("per_element_ambiguous_pin", &actual);
}

// Finding 1 materiality guard: a DYNAMIC feedback model whose per-element target
// body carries an all-`Pinned` live-source occurrence over an AMBIGUOUS element.
// `pop` grows over time so the `pop[Region,young] -> growth[Region,Age]` scores
// are genuinely non-zero. The pre-fix half-qualified `PREVIOUS(pop[boston, ...])`
// still happened to compile (positional axis resolution keeps bare `boston`
// resolvable), so unlike the collapse regressions this shape did NOT surface a
// compile failure -- which is exactly why the text-pin gap was material: a
// value-divergent change in this shape would have slipped through silently. This
// guard freezes the runtime equivalence: every per-element score is non-zero and
// the fragments all compile, in the same tree the text golden pins.

fn per_element_ambiguous_pin_feedback_model() -> datamodel::Project {
    TestProject::new("per_element_ambiguous_pin_feedback")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Cities", &["boston", "la"])
        .named_dimension("Age", &["young", "old"])
        .array_flow(
            "growth[Region,Age]",
            "pop[Region, young] * pop[boston, old] * 0.001",
            None,
        )
        .array_stock("pop[Region,Age]", "10", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn per_element_ambiguous_pin_scores_are_live_not_silent_zero() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = per_element_ambiguous_pin_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .map(|d| d.variable.clone().unwrap_or_default())
        .collect();
    assert!(
        frag_failures.is_empty(),
        "per-element ambiguous-pin link-score fragments must all compile; \
         failed: {frag_failures:?}"
    );

    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed");
    let score_offsets: Vec<usize> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| {
            k.as_str().contains("link_score\u{205A}pop[") && k.as_str().contains("\u{2192}growth[")
        })
        .map(|(_, v)| *v)
        .collect();
    assert!(
        !score_offsets.is_empty(),
        "expected per-element pop[.,young]->growth link scores in offsets"
    );
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    for off in score_offsets {
        let has_nonzero = results.iter().any(|row| row[off] != 0.0);
        assert!(
            has_nonzero,
            "per-element ambiguous-pin link score at offset {off} is all-zero"
        );
    }
}

// ---------------------------------------------------------------------------
// Model A6 (Track A3 stage 1, finding 2, ADJUDICATED divergence): a live-source
// `PerElement` occurrence nested inside ANOTHER dep's subscript index.
//
// `growth[Region] = pop[Region, young] * other[pop[Region, young]]`:
//   * the direct `pop[Region, young]` is the emitting `PerElement` site (held
//     live -> bare row `pop[<r>, young]`).
//   * the nested `pop[Region, young]` inside `other[...]`'s index is a SECOND
//     occurrence of the same site shape. The wrap freezes the enclosing
//     `other[...]` at `PREVIOUS` (an other-dep), so the row-pinning lowering
//     descends into it with `force_qualified` and takes its qualified arm.
//
// This pin captures the transform-first output `PREVIOUS(other[pop[region·<r>,
// age·young]])` (the nested occurrence fully qualified from `pop`'s declared
// dims). It DIVERGES from HEAD `f057ef38`, which emitted the bare-row form
// `PREVIOUS(other[pop[<r>, young]])`: HEAD's pre-transform rewrite pinned the
// nested occurrence BEFORE the wrap inserted the `PREVIOUS`, so it ran with
// `force_qualified == false` and produced the bare row. The divergence is
// semantics-preserving -- both spellings statically resolve to the identical
// `pop` element used as a dynamic index into `other`, and the
// `per_element_index_nested_scores_are_live_not_silent_zero` guard below proves
// the compiled score fragments are live (non-zero, finite, all compile). The
// transform-first lowering
// consistently qualifies EVERY frozen source occurrence from the source's own
// declared dims (the same rule that fixes finding 1's ambiguous all-`Pinned`
// occurrence); matching HEAD's incidental bare form here would require the
// lowering to distinguish a wrap-inserted `PREVIOUS` from an original-equation
// one, fragile machinery that de-simplifies the very transform this stage
// clarifies. Adjudicated as accepted per the stage-1 contract (see
// trackA3s1-report.md §3).
// ---------------------------------------------------------------------------

fn per_element_index_nested_model() -> datamodel::Project {
    TestProject::new("per_element_index_nested_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("pop[Region,Age]", "10")
        .array_aux("other[Region]", "0.5")
        .array_flow(
            "growth[Region]",
            "pop[Region, young] * other[pop[Region, young]]",
            None,
        )
        .array_stock("stock[Region]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_index_nested_occurrence() {
    let actual = dump_synthetic_vars(
        per_element_index_nested_model(),
        true,
        "link_score\u{205A}pop",
    );
    assert_golden("per_element_index_nested_occurrence", &actual);
}

// Finding 2 materiality guard: a DYNAMIC model whose per-element target reads
// the live source through an index-nested occurrence. `pop` grows over time, so
// the `pop -> growth` per-element scores (whose equations carry the qualified
// nested-index spelling this pin adjudicates) are genuinely non-zero. The index
// stays in range (small `pop`, wide `Slot`), so `other[pop[...]]` is finite and
// the score is not NaN-poisoned. This freezes the adjudicated spelling's runtime
// behavior: every per-element `pop -> growth` score is finite and non-zero and
// every LTM fragment compiles -- so a future value-divergent change to the
// index-nested lowering turns this red, closing the pin gap the review named.

fn per_element_index_nested_feedback_model() -> datamodel::Project {
    TestProject::new("per_element_index_nested_feedback")
        .with_sim_time(0.0, 2.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension("Slot", &["s0", "s1", "s2", "s3", "s4"])
        .array_aux("other[Slot]", "0.5")
        .array_stock("pop[Region,Age]", "1", &["popinflow"], &[], None)
        .array_flow("popinflow[Region,Age]", "0.5", None)
        .array_flow(
            "growth[Region]",
            "pop[Region, young] * other[pop[Region, young]]",
            None,
        )
        .array_stock("stock[Region]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn per_element_index_nested_scores_are_live_not_silent_zero() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = per_element_index_nested_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .map(|d| d.variable.clone().unwrap_or_default())
        .collect();
    assert!(
        frag_failures.is_empty(),
        "per-element index-nested link-score fragments must all compile; \
         failed: {frag_failures:?}"
    );

    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed");
    let score_offsets: Vec<usize> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| {
            k.as_str().contains("link_score\u{205A}pop[") && k.as_str().contains("\u{2192}growth[")
        })
        .map(|(_, v)| *v)
        .collect();
    assert!(
        !score_offsets.is_empty(),
        "expected per-element pop->growth link scores in offsets"
    );
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    for off in score_offsets {
        let mut saw_nonzero = false;
        for row in results.iter() {
            let v = row[off];
            assert!(
                v.is_finite(),
                "index-nested pop->growth score at offset {off} is non-finite \
                 ({v}) -- the qualified nested-index spelling must stay in range"
            );
            saw_nonzero |= v != 0.0;
        }
        assert!(
            saw_nonzero,
            "index-nested pop->growth link score at offset {off} is all-zero"
        );
    }
}

// ---------------------------------------------------------------------------
// Model A7 (Track A3 stage 1, finding 1): per-element target whose body carries
// a frozen live-source occurrence with a DYNAMIC (non-element, non-dim-name)
// index.
//
// `growth[Region] = pop[Region, young] * pop[Region, idx] * 0.001`, `idx = 1 +
// (TIME MOD 2)`:
//   * `pop[Region, young]` is the emitting `PerElement` site.
//   * `pop[Region, idx]` is a frozen occurrence whose Age axis is the DYNAMIC
//     index `idx` -- a genuine ceteris-paribus dependency, not an element
//     selector.
//
// The frozen occurrence must lower to `PREVIOUS(pop[region·<r>, PREVIOUS(idx)])`:
// the iterated `Region` axis pinned+qualified by the lowering, and the dynamic
// `idx` WRAPPED in `PREVIOUS` by the wrap's index pass (its ceteris-paribus lag).
// The finding-1 index-qualification suppression must be scoped to element
// qualification ONLY -- a blanket "keep the indices pristine" would drop the
// `PREVIOUS(idx)` lag, emitting `PREVIOUS(pop[region·<r>, idx])` and changing the
// compiled score series (HEAD: a NaN at the first live step; the un-lagged form:
// finite). Byte-identical to HEAD `f057ef38`.
// ---------------------------------------------------------------------------

fn per_element_dynamic_index_model() -> datamodel::Project {
    TestProject::new("dyn_index_char")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .scalar_aux("idx", "1 + (TIME MOD 2)")
        .array_flow(
            "growth[Region]",
            "pop[Region, young] * pop[Region, idx] * 0.001",
            None,
        )
        .array_stock("pop[Region,Age]", "10", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_per_element_dynamic_index() {
    let actual = dump_synthetic_vars(
        per_element_dynamic_index_model(),
        true,
        "link_score\u{205A}pop",
    );
    assert_golden("per_element_dynamic_index", &actual);
}

// Finding 1 materiality guard: the DYNAMIC-index sibling of the ambiguous-pin
// guard. The frozen `pop[Region, idx]` occurrence lowers to a `PREVIOUS(idx)`
// lag; the buggy blanket-skip would instead leave `idx` un-lagged, a
// value-divergent change the constant-`pop` text golden alone would still catch
// (the golden diverges) but whose RUNTIME consequence this guard freezes. HEAD's
// series carries a NaN at the first live step (a pre-existing HEAD behavior: the
// synthesized `PREVIOUS(idx)` capture-helper aux reads an uninitialized value at
// the initial step; tracked separately as a latent bug). Reproducing that NaN is
// the byte-parity contract; the un-lagged regression makes the same step FINITE,
// so `results[1]` being NaN is the exact discriminator. This guard therefore
// pins (a) every fragment compiles (the `PREVIOUS(idx)` helper is well-formed),
// (b) the first-live-step score is NaN (the dynamic-index lag is present), and
// (c) a later step is finite and non-zero (the score is materially live).
fn per_element_dynamic_index_feedback_model() -> datamodel::Project {
    // Same shape as `per_element_dynamic_index_model` (pop is already a
    // growth-fed stock there), reused directly so the guard and the text pin
    // exercise the identical model.
    per_element_dynamic_index_model()
}

#[test]
fn per_element_dynamic_index_scores_preserve_head_lag() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = per_element_dynamic_index_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let frag_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .map(|d| d.variable.clone().unwrap_or_default())
        .collect();
    assert!(
        frag_failures.is_empty(),
        "dynamic-index per-element link-score fragments must all compile; \
         failed: {frag_failures:?}"
    );

    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed");
    // The real per-element scores start with the single `$⁚ltm⁚link_score⁚`
    // prefix; the synthesized `PREVIOUS`-capture helper auxes carry a `$⁚$⁚`
    // double prefix, so `starts_with` excludes them.
    let score_offsets: Vec<usize> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| {
            k.as_str()
                .starts_with("$\u{205A}ltm\u{205A}link_score\u{205A}pop[")
                && k.as_str().contains("\u{2192}growth[")
        })
        .map(|(_, v)| *v)
        .collect();
    assert_eq!(
        score_offsets.len(),
        2,
        "expected two per-element dynamic-index pop->growth link scores"
    );
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    let rows: Vec<_> = results.iter().collect();
    for off in score_offsets {
        // Step 1 (the first live step) reproduces HEAD's NaN, the signature of
        // the `PREVIOUS(idx)` lag reading its uninitialized capture helper. An
        // un-lagged regression (`idx` not wrapped) makes this step FINITE.
        assert!(
            rows[1][off].is_nan(),
            "dynamic-index pop->growth score at offset {off} step 1 must be NaN \
             (the preserved PREVIOUS(idx) dynamic-index lag); a finite value means \
             the lag was dropped"
        );
        // A later step is finite and non-zero: the score is materially live.
        let has_finite_nonzero = rows
            .iter()
            .skip(2)
            .any(|row| row[off].is_finite() && row[off] != 0.0);
        assert!(
            has_finite_nonzero,
            "dynamic-index pop->growth score at offset {off} has no finite non-zero \
             step -- the score is not materially live"
        );
    }
}

// ---------------------------------------------------------------------------
// Model B: agg -> scalar target (synthetic agg, `generate_agg_to_scalar_target_equation`)
//
// `total = SUM(pop[*]) * 2` -- the reducer is a SUBexpression (not the whole
// RHS), so it is hoisted into a synthetic `$⁚ltm⁚agg⁚0` and the `agg -> total`
// half runs through the scalar-target generator on the agg-substituted text.
// ---------------------------------------------------------------------------

fn agg_to_scalar_model() -> datamodel::Project {
    TestProject::new("agg_to_scalar_char")
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "10")
        .scalar_aux("total", "SUM(pop[*]) * 2")
        .stock("acc", "0", &["totalflow"], &[], None)
        .aux("totalflow", "total", None)
        .build_datamodel()
}

#[test]
fn char_agg_to_scalar_target() {
    let actual = dump_synthetic_vars(agg_to_scalar_model(), true, "\u{205A}agg\u{205A}0\u{2192}");
    assert_golden("agg_to_scalar_target", &actual);
}

// ---------------------------------------------------------------------------
// Model B2 (Track A3 stage 1, finding 2): a hoisted reducer NESTED inside a
// DECLINED (non-hoisted) outer reducer.
//
// `growth[Region] = SUM(matrix[Region,*] * other[nyc,*] * SUM(pop[*])) * 0.001`:
//   * the inner `SUM(pop[*])` is a whole-extent reduce -> hoisted into
//     `$⁚ltm⁚agg⁚0`.
//   * the outer `SUM(...)` is DECLINED by the I1 acceptance (its co-source slices
//     differ: `matrix[Region,*]` is `[Iterated, Reduced]` while `other[nyc,*]` is
//     `[Pinned, Reduced]`), so it stays inline.
//
// The `agg⁚0 -> growth[e]` half wraps the target's own equation with the inner
// reducer held LIVE by `live_reducer_text`. The wrap must NOT freeze the whole
// declined outer reducer: doing so (the GH #517 arm) would drop the live agg
// entirely, yielding `PREVIOUS(SUM(matrix[..] * other[..] * SUM(pop[*])))` -- a
// clean-compiling structural zero (the frozen partial equals the frozen anchor).
// HEAD instead recurses into the outer reducer, holds the agg live, and freezes
// the co-source array slices: `sum(previous(matrix[region·<r>, *]) *
// previous(other[region·nyc, *]) * "$⁚ltm⁚agg⁚0") * 0.001` -- which fails to
// compile (no LoadPrev-of-array-view path) and surfaces as a LOUD warned zero.
// The finding-2 gate teaches the GH #517 freeze about `live_reducer_text` so the
// enclosing reducer recurses, restoring HEAD's text (and its loud degradation --
// a loud failure is strictly better than a silent structural zero). Byte-
// identical to HEAD `f057ef38`.
// ---------------------------------------------------------------------------

fn agg_nested_reducer_model() -> datamodel::Project {
    TestProject::new("nested_reducer_char")
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "10")
        .array_aux("matrix[Region,Region]", "1")
        .array_aux("other[Region,Region]", "1")
        .array_flow(
            "growth[Region]",
            "SUM(matrix[Region,*] * other[nyc,*] * SUM(pop[*])) * 0.001",
            None,
        )
        .array_stock("stock[Region]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_agg_nested_reducer() {
    let actual = dump_synthetic_vars(agg_nested_reducer_model(), true, "link_score");
    assert_golden("agg_nested_reducer", &actual);
}

// Finding 2 materiality guard: unlike every other guard in this file (which
// asserts fragments MUST compile), the byte-parity contract here is to REPRODUCE
// HEAD's LOUD failure. HEAD emits the live-agg-inside-a-frozen-array-slice
// partial that cannot compile (six `Assembly` "failed to compile" warnings: the
// two `agg⁚0->growth[e]` scores plus their four `PREVIOUS`-capture helper auxes),
// producing a warned zero. The pre-fix transform-first freeze produced a SILENT
// clean-compiling zero (zero warnings) -- the worst failure class. This guard
// pins that the loud warnings ARE present: a regression that silently zeroes the
// score would drop them. `pop` is a stock so the edge is causally live.
fn agg_nested_reducer_feedback_model() -> datamodel::Project {
    TestProject::new("nested_reducer_feedback")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("matrix[Region,Region]", "1")
        .array_aux("other[Region,Region]", "1")
        .array_flow(
            "growth[Region]",
            "SUM(matrix[Region,*] * other[nyc,*] * SUM(pop[*])) * 0.001",
            None,
        )
        .array_stock("pop[Region]", "10", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn agg_nested_reducer_preserves_loud_failure_not_silent_zero() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = agg_nested_reducer_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let frag_failures: Vec<String> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .map(|d| d.variable.clone().unwrap_or_default())
        .collect();
    // The loud degradation must be present: the two agg⁚0->growth[e] scores must
    // each fail to compile (the live-agg-inside-a-frozen-array-slice partial),
    // matching HEAD. A silent-zero regression would report NO such failure.
    let agg_growth_failures: Vec<&String> = frag_failures
        .iter()
        .filter(|v| v.contains("agg\u{205A}0\u{2192}growth"))
        .collect();
    assert!(
        agg_growth_failures.len() >= 2,
        "the nested-reducer agg->growth partial must LOUDLY fail to compile \
         (HEAD's warned zero), not silently zero; failed fragments: {frag_failures:?}"
    );
}

// ---------------------------------------------------------------------------
// Model C: agg -> arrayed target (`generate_scalar_to_element_equation`, scalar agg)
//
// `outflow[Region] = SUM(pop[*]) * frac[Region]` -- whole-extent SUM hoisted
// into a scalar synthetic agg; the `agg -> outflow[e]` half is one scalar var
// per target element via the scalar-to-element generator.
// ---------------------------------------------------------------------------

fn agg_to_arrayed_model() -> datamodel::Project {
    TestProject::new("agg_to_arrayed_char")
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "10")
        .array_aux("frac[Region]", "0.5")
        .array_flow("outflow[Region]", "SUM(pop[*]) * frac[Region]", None)
        .array_stock("stock[Region]", "0", &["outflow"], &[], None)
        .build_datamodel()
}

#[test]
fn char_agg_to_arrayed_target() {
    let actual = dump_synthetic_vars(agg_to_arrayed_model(), true, "\u{205A}agg\u{205A}0\u{2192}");
    assert_golden("agg_to_arrayed_target", &actual);
}

// ---------------------------------------------------------------------------
// Model D: arrayed agg -> arrayed target with `source_pins` (GH #528)
//
// `outflow[Region] = SUM(matrix[Region,*]) * 0.1` -- a SLICED reducer hoisted
// into an ARRAYED agg over Region; the `agg -> outflow[e]` half pins the agg
// to the target-element projection (`source_pins`), exercising the arrayed-agg
// branch of `generate_scalar_to_element_equation`.
// ---------------------------------------------------------------------------

fn arrayed_agg_model() -> datamodel::Project {
    TestProject::new("arrayed_agg_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Dim2", &["a", "b"])
        .array_aux("matrix[Region,Dim2]", "10")
        .array_flow("outflow[Region]", "SUM(matrix[Region,*]) * 0.1", None)
        .array_stock("stock[Region]", "0", &["outflow"], &[], None)
        .build_datamodel()
}

#[test]
fn char_arrayed_agg_to_target() {
    let actual = dump_synthetic_vars(arrayed_agg_model(), true, "link_score");
    assert_golden("arrayed_agg_to_target", &actual);
}

// ---------------------------------------------------------------------------
// Model E: Ast::Arrayed (per-element-equation) target (Category B,
// `build_arrayed_link_score_equation`)
//
// `mp` has per-element equations referencing `pop` by fixed element subscripts;
// each slot's link score is the guard form built on THAT slot's own text.
// ---------------------------------------------------------------------------

fn arrayed_target_model() -> datamodel::Project {
    let mut p = TestProject::new("arrayed_target_char")
        .named_dimension("Region", &["nyc", "boston"])
        .array_aux("pop[Region]", "10");
    // A per-element-equation (Ast::Arrayed) target: each element has its own eqn.
    p = p.array_with_default_and_overrides(
        "mp[Region]",
        "0",
        vec![
            ("nyc", "(pop[nyc] - pop[boston]) * 0.01"),
            ("boston", "(pop[boston] - pop[nyc]) * 0.01"),
        ],
    );
    p.array_stock("stock[Region]", "0", &["mpflow"], &[], None)
        .array_flow("mpflow[Region]", "mp", None)
        .build_datamodel()
}

#[test]
fn char_arrayed_target_slot_scores() {
    let actual = dump_synthetic_vars(arrayed_target_model(), true, "link_score\u{205A}pop");
    assert_golden("arrayed_target_slot_scores", &actual);
}

// ---------------------------------------------------------------------------
// Model F (Track A3 stage 2, review finding 2): the GH #517 whole-reducer
// freeze over an INDEX-NESTED live-source occurrence (Fig. 2 Q4).
//
// `to = SUM(w[from, *]) + from`, edge `(from -> to)`, `Bare` shape. `from`
// (the live source) occurs TWICE in `to`'s equation:
//   * bare, outside any reducer (`+ from`) -- the live occurrence;
//   * index-nested, inside the reducer's subscript (`w[from, *]`).
//
// The changed-first partial must FREEZE THE WHOLE REDUCER --
// `PREVIOUS(SUM(w[from, *])) + from` -- because `expr0_contains_live_match`'s
// Subscript arm matches only a subscript whose HEAD is the live source, never
// an index-nested occurrence (`ltm_augment.rs`). Recursing into the reducer
// instead would emit `SUM(PREVIOUS(w[from, *]))` (a PREVIOUS of an array view,
// which has no LoadPrev path -- a loud compile failure and a silently-zeroed
// score, GH #517).
//
// This pins the exact Fig. 2 Q4 selection semantics the stage-2 occurrence
// switch must reproduce via `occ.index_nested`: an index-nested occurrence is
// excluded from the reducer-containment test AND from live selection. Before
// this golden, mutating `expr0_contains_live_match` to count index-nested
// occurrences (the exact stage-2 `occ.index_nested` mishandling) passed the
// entire corpus green while changing this text -- verified: the mutation flips
// this golden's numerator from the changed-first whole-reducer freeze
// `PREVIOUS(sum(w[from, *])) + from` to the changed-LAST form
// `to - (sum(w[from, *]) + PREVIOUS(from))` (the reducer held live, the bare
// `from` frozen). Both COMPILE and are non-zero, so only this exact-text pin
// catches the drift -- a runtime compile/non-zero guard would not. The
// scalar-element form (`SUM(w[from])`) is pinned directly at the wrap level by
// `partial_freezes_whole_reducer_over_index_nested_live_source` in
// `ltm_augment_tests.rs`; the runtime guard below is a separate silent-zero
// backstop that this shape's score stays materially live.
// ---------------------------------------------------------------------------

fn reducer_index_nested_model() -> datamodel::Project {
    TestProject::new("reducer_index_nested_char")
        .named_dimension("Slot", &["s1", "s2"])
        .named_dimension("K", &["k1", "k2"])
        .array_aux("w[Slot,K]", "0.5")
        .aux("from", "1", None)
        .aux("to", "SUM(w[from, *]) + from", None)
        .stock("acc", "0", &["toflow"], &[], None)
        .flow("toflow", "to", None)
        .build_datamodel()
}

#[test]
fn char_reducer_index_nested_freeze() {
    let actual = dump_synthetic_vars(
        reducer_index_nested_model(),
        true,
        "link_score\u{205A}from\u{2192}to",
    );
    assert_golden("reducer_index_nested_freeze", &actual);
}

// Finding 2 materiality guard for the index-nested reducer-freeze shape,
// mirroring the `agg_nested_reducer_preserves_loud_failure_not_silent_zero`
// idiom: the byte-parity behavior of `to = SUM(w[from, *]) + from` is a LOUD
// failure, not a silent compile. Because `from` is a dynamic index, the frozen
// `PREVIOUS(sum(w[from, *]))` desugars to a synthesized scalar PREVIOUS-helper
// over a dynamic-index reducer, which the engine cannot compile (a pre-existing
// dynamic-index-reducer limitation) and reports as a `Warning`.
//
// NOTE on scope: the specific index-nested-exclusion drift (recursing into the
// reducer to hold the index-nested `from` live) is a TEXT-only change at
// runtime -- verified: it flips the wrap to the changed-last form
// `to - (sum(w[from, *]) + PREVIOUS(from))`, which STILL fails to compile (a
// dynamic-index reducer inline in a scalar guard equation), so both baseline
// and mutation loud-fail. That drift is caught by the `reducer_index_nested_freeze`
// text golden and the wrap unit test, not here. This guard is the complementary
// silent-vs-loud backstop: it pins that this Fig. 2 Q4 shape LOUDLY fails rather
// than silently compiling to a wrong-valued score -- the codebase's preferred
// degradation (GH #311/#661/#743). A future selector that made this shape
// silently compile (dropping the warning) turns this red.

fn reducer_index_nested_feedback_model() -> datamodel::Project {
    TestProject::new("reducer_index_nested_feedback")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Slot", &["s1", "s2"])
        .named_dimension("K", &["k1", "k2"])
        .array_aux("w[Slot,K]", "0.5")
        .aux("from", "1 + (TIME MOD 2)", None)
        .aux("to", "SUM(w[from, *]) + from", None)
        .stock("acc", "0", &["toflow"], &[], None)
        .flow("toflow", "to", None)
        .build_datamodel()
}

#[test]
fn reducer_index_nested_freeze_preserves_loud_failure_not_silent_compile() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    use salsa::Setter;

    let project = reducer_index_nested_feedback_model();
    let mut db = SimlinDb::default();
    let (source_project, source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);
    source_project.set_ltm_discovery_mode(&mut db).to(true);

    let diags = collect_model_diagnostics(&db, source_model, source_project);
    let from_to_failures: Vec<_> = diags
        .iter()
        .filter(|d| {
            d.severity == DiagnosticSeverity::Warning
                && matches!(
                    &d.error,
                    DiagnosticError::Assembly(msg) if msg.contains("failed to compile")
                )
        })
        .filter_map(|d| d.variable.clone())
        .filter(|v| v.contains("link_score\u{205A}from\u{2192}to"))
        .collect();
    assert!(
        !from_to_failures.is_empty(),
        "the changed-first whole-reducer freeze over a dynamic index-nested live \
         source must preserve HEAD's loud compile-failure warning (not silently \
         compile the changed-last form that holds the reducer live); a missing \
         warning means the index-nested occurrence was held live"
    );
}

// ---------------------------------------------------------------------------
// Model G (Track A3 stage 2, review finding 2): an already-lagged other-dep
// (Fig. 2 Q3).
//
// `to = from + PREVIOUS(g)`, edge `(from -> to)`, `Bare` shape. `g` occurs
// only inside `PREVIOUS(g)` -- it is already lagged. The changed-first partial
// holds `from` live and must LEAVE `PREVIOUS(g)` untouched
// (`from + PREVIOUS(g)`), NOT re-wrap it to `PREVIOUS(PREVIOUS(g))` (a t-2
// read). This pins the `already_lagged` selection semantics the stage-2 switch
// must reproduce via `occ.already_lagged`: it suppresses the wrap/freeze of an
// already-lagged occurrence but not its live selection.
// ---------------------------------------------------------------------------

fn already_lagged_other_dep_model() -> datamodel::Project {
    TestProject::new("already_lagged_char")
        .aux("g", "3", None)
        .aux("from", "1", None)
        .aux("to", "from + PREVIOUS(g)", None)
        .stock("acc", "0", &["toflow"], &[], None)
        .flow("toflow", "to", None)
        .build_datamodel()
}

#[test]
fn char_already_lagged_other_dep() {
    let actual = dump_synthetic_vars(
        already_lagged_other_dep_model(),
        true,
        "link_score\u{205A}from\u{2192}to",
    );
    assert_golden("already_lagged_other_dep", &actual);
}
