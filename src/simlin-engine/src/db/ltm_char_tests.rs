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
/// string: `Scalar`/`ApplyToAll` are their diagnostic text; `Arrayed` lists
/// each `element => eqn` slot (element-sorted, as the generator emits) plus the
/// optional `default => eqn`. Dimensions are prefixed so a shape change is
/// visible in the pin.
///
/// Renders the arm's diagnostic `text` (the generator's exact source-form
/// spelling), NOT `print_eqn` of the parsed AST, so the golden pins the same
/// bytes the augmentation layer produced -- the AST is the compiled form, the
/// text the diagnostic serialization (see [`crate::db::LtmEquation`]).
fn render_equation(eq: &crate::db::LtmEquation) -> String {
    use crate::db::LtmEquation;
    match eq {
        LtmEquation::Scalar(arm) => format!("scalar: {}", arm.text),
        LtmEquation::ApplyToAll(dims, arm) => {
            format!("a2a[{}]: {}", dims.join(","), arm.text)
        }
        LtmEquation::Arrayed {
            dims,
            elements,
            default,
            has_except_default,
        } => {
            let mut out = format!(
                "arrayed[{}] (apply_default={has_except_default}):",
                dims.join(",")
            );
            for (elem, arm) in elements {
                out.push_str(&format!("\n    {elem} => {}", arm.text));
            }
            if let Some(default_arm) = default {
                out.push_str(&format!("\n    <default> => {}", default_arm.text));
            }
            out
        }
    }
}

/// What a characterization fixture expects from the LTM fragment-compile
/// diagnostic pass (`model_ltm_fragment_diagnostics`).
///
/// Every fixture must state this explicitly -- it is a required argument of
/// [`assert_char_fixture`], so a new fixture cannot forget to declare it and
/// quietly contribute no runtime coverage.
///
/// Why the char goldens need it at all: a golden pins generated equation TEXT,
/// so it is structurally blind to a generated equation that is *stably*
/// unparseable. Such an equation compiles to no bytecode, the variable reads a
/// constant 0, and the golden stays green forever. Two independent
/// unquotable-generation bugs were found in one day (the `1stock` leading digit,
/// and the bare-keyword class of GH #976), neither of which any text golden would
/// have caught -- so the corpus asserts the runtime consequence too.
enum FragmentExpectation {
    /// No LTM synthetic fragment (or implicit helper) may fail to compile.
    AllCompile,
    /// This fixture deliberately exercises loud warned-skip degradation.
    /// `vars` is the EXACT set of variables expected to fail -- an extra
    /// failure fails the test, and so does a failure that disappears (which
    /// means the annotation is now stale and should be tightened).
    ExpectedFailures {
        /// Why these specifically cannot compile. A carve-out without a reason
        /// is indistinguishable from a bug that was annotated away.
        why: &'static str,
        vars: &'static [&'static str],
    },
}

/// Build the fixture's db in the configuration the whole char suite uses:
/// discovery mode ON (every fixture exercises the discovery emitters) and
/// `ltm_enabled` ON so `model_ltm_fragment_diagnostics` actually runs.
///
/// `ltm_enabled` does not affect the dumped text -- `model_ltm_variables` is
/// called directly and does not read the flag -- it only un-gates the diagnostic
/// pass inside `model_all_diagnostics`.
fn char_fixture_db(project: &datamodel::Project) -> (SimlinDb, SourceModel, SourceProject) {
    use salsa::Setter;
    let mut db = SimlinDb::default();
    let (source_project, model) = {
        let sync = sync_from_datamodel(&db, project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_discovery_mode(&mut db).to(true);
    source_project.set_ltm_enabled(&mut db).to(true);
    (db, model, source_project)
}

/// The LTM synthetic variables / implicit helpers whose fragments failed to
/// compile, sorted. This is exactly the condition
/// `model_ltm_fragment_diagnostics` reports: the variable keeps a layout slot
/// but has no bytecode, so it evaluates to a constant 0 and every score derived
/// from it is silently degraded.
fn fragment_compile_failures(
    db: &SimlinDb,
    model: SourceModel,
    project: SourceProject,
) -> Vec<String> {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};
    let mut failures: Vec<String> = collect_model_diagnostics(db, model, project)
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
    failures.sort();
    failures
}

/// The characterization entry point: pin the generated equation text against
/// `golden` AND assert the fixture's declared fragment-compile expectation.
///
/// Both halves matter and neither subsumes the other. The golden catches a
/// change in generated text that still compiles; the expectation catches a
/// generated equation that stops compiling (a silent per-variable zero) without
/// changing any other fixture's text.
#[track_caller]
fn assert_char_fixture(
    golden: &str,
    project: datamodel::Project,
    filter: &str,
    expect: FragmentExpectation,
) {
    let (db, model, source_project) = char_fixture_db(&project);
    let actual = render_synthetic_vars(&db, model, source_project, filter);
    assert_golden(golden, &actual);

    let failures = fragment_compile_failures(&db, model, source_project);
    match expect {
        FragmentExpectation::AllCompile => assert!(
            failures.is_empty(),
            "fixture `{golden}` declares AllCompile, but these LTM fragments failed to \
             compile (each keeps a layout slot with no bytecode, so it reads a constant 0 \
             and every score through it is silently degraded): {failures:?}"
        ),
        FragmentExpectation::ExpectedFailures { why, vars } => {
            let mut expected: Vec<String> = vars.iter().map(|v| v.to_string()).collect();
            expected.sort();
            assert_eq!(
                failures, expected,
                "fixture `{golden}`'s fragment-compile failures do not match its \
                 declared expectation.\n  declared reason: {why}\n  If a NEW failure \
                 appeared, that is a silent-zero regression. If a declared failure \
                 DISAPPEARED, the annotation is stale -- tighten it (ideally to \
                 AllCompile)."
            );
        }
    }
}

/// Deterministic dump of every synthetic variable whose name matches
/// `filter`, sorted by name, one `name` line followed by its rendered
/// equation. Used as the characterization surface: the whole string is
/// pinned byte-for-byte.
fn render_synthetic_vars(
    db: &SimlinDb,
    model: SourceModel,
    source_project: SourceProject,
    filter: &str,
) -> String {
    let ltm = model_ltm_variables(db, model, source_project);
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
    assert_char_fixture(
        "per_element_link_scores",
        per_element_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "per_element_other_deps",
        per_element_other_deps_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
// GH #974: pinning a BARE arrayed reference over the DEP's declared dimensions.
//
// The two guards below cover the issue's two sibling arms, which share one
// projection (`post_transform::dep_element_pins`, enumerated by
// `dep_element_pins_projection_enumeration`) but reach it through different
// call paths: an other-dep's bare reference goes through
// `subscript_idents_in_expr0`, and a bare reference to the LIVE SOURCE goes
// through the wrap's own `pin_bare_source_ref`.
//
// Both models below compile with ZERO diagnostics before the fix -- the defects
// are entirely inside LTM generation.
// ---------------------------------------------------------------------------

/// The equation text of the one link-score variable whose name contains every
/// fragment in `name_parts`, for a model built with [`char_fixture_db`].
fn link_score_text(
    db: &SimlinDb,
    model: SourceModel,
    project: SourceProject,
    name_parts: &[&str],
) -> String {
    let ltm = model_ltm_variables(db, model, project);
    let matching: Vec<&LtmSyntheticVar> = ltm
        .vars
        .iter()
        .filter(|v| name_parts.iter().all(|p| v.name.contains(p)))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one link score matching {name_parts:?}, got {:?}",
        matching.iter().map(|v| &v.name).collect::<Vec<_>>()
    );
    matching[0].equation.source_text().to_string()
}

fn bare_dep_own_dims_model() -> datamodel::Project {
    TestProject::new("bare_dep_own_dims")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_with_ranges_direct(
            "regw",
            vec!["Region".to_string()],
            vec![("nyc", "10"), ("boston", "20")],
            None,
        )
        .array_with_ranges_direct(
            "agew",
            vec!["Age".to_string()],
            vec![("young", "1"), ("old", "2")],
            None,
        )
        // Three deps referenced BARE in `growth`'s body, differing only in how
        // their declared dimensions relate to the target's: identical, the same
        // two REORDERED, and a strict SUBSET.
        .array_aux_direct(
            "same",
            vec!["Region".to_string(), "Age".to_string()],
            "regw[Region] + agew[Age]",
            None,
        )
        .array_aux_direct(
            "flip",
            vec!["Age".to_string(), "Region".to_string()],
            "agew[Age] * 100 + regw[Region]",
            None,
        )
        .array_aux_direct("w", vec!["Age".to_string()], "agew[Age] * 1000", None)
        .array_flow(
            "growth[Region,Age]",
            "pop[Region, young] * (same + flip + w) * 0.0001",
            None,
        )
        .array_stock("pop[Region,Age]", "10", &["growth"], &[], None)
        .build_datamodel()
}

/// A BARE arrayed dep in a per-element link-score partial is pinned over the
/// dimensions IT declares, matched by name -- not over the target's element
/// tuple (GH #974, arm 1).
///
/// The test states the executed semantics first and the generated text second,
/// because the first is what makes the second right. The numeric oracle reads
/// the deps' own series out of a real run and asserts
/// `growth[nyc,old] == pop[nyc,young] * (same[nyc,old] + flip[old,nyc] +
/// w[old]) * 1e-4`: a bare reference reads each of its OWN axes' coordinates by
/// dimension name, so the reordered `flip[Age,Region]` reads `[old,nyc]` and the
/// subset `w[Age]` reads `[old]`. Every element carries a distinct value, so a
/// wrong element changes the sum.
///
/// The pre-fix pin used the target's tuple `[region·nyc, age·old]` for all
/// three, which failed in two DIFFERENT ways -- and the difference is why both
/// rows are here. `w` got an arity-2 subscript over a 1-D variable, so its
/// fragment failed to compile and the score read a constant 0 (loud, via the
/// four `Assembly` warnings the issue reports). `flip` got an arity-2 subscript
/// that RESOLVED -- both indices are valid element references for the axes they
/// landed on -- so it compiled and silently read `flip[young,boston]`. That
/// second row had no diagnostic of any kind.
#[test]
fn bare_arrayed_dep_is_pinned_over_its_own_declared_dims() {
    let project = bare_dep_own_dims_model();

    // 1. What the SIMULATION reads. This is the claim the pin has to match, and
    //    it is checked against the engine rather than assumed.
    let plain_db = SimlinDb::default();
    let plain_project = sync_from_datamodel(&plain_db, &project).project;
    let compiled = compile_project_incremental(&plain_db, plain_project, "main")
        .expect("the model compiles with no LTM");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();
    let at = |row: &[f64], name: &str| -> f64 {
        let off = compiled
            .offsets
            .iter()
            .find(|(k, _)| k.as_str() == name)
            .unwrap_or_else(|| panic!("no slot named {name}"))
            .1;
        row[*off]
    };
    let last = results.iter().next_back().expect("at least one saved step");
    let expected = at(last, "pop[nyc,young]")
        * (at(last, "same[nyc,old]") + at(last, "flip[old,nyc]") + at(last, "w[old]"))
        * 0.0001;
    assert!(
        (at(last, "growth[nyc,old]") - expected).abs() < 1e-9,
        "a bare arrayed reference must read each of its OWN axes' coordinates by \
         dimension name: growth[nyc,old] = {}, expected {expected}",
        at(last, "growth[nyc,old]")
    );
    // Non-vacuity: the discriminating elements must differ, or a transposed or
    // over-arity read would produce the same number.
    assert_ne!(at(last, "flip[old,nyc]"), at(last, "flip[young,boston]"));
    assert_ne!(at(last, "same[nyc,old]"), at(last, "same[boston,old]"));

    // 2. What the LTM partial spells, which must be the same reads.
    let (db, model, source_project) = char_fixture_db(&project);
    assert!(
        fragment_compile_failures(&db, model, source_project).is_empty(),
        "every per-element link-score fragment must compile: the subset-dims `w` \
         pin was arity-2 over a 1-D variable before the fix"
    );
    let text = link_score_text(
        &db,
        model,
        source_project,
        &[
            "link_score\u{205A}pop[nyc,young]",
            "\u{2192}growth[nyc,old]",
        ],
    );
    for expected in [
        "PREVIOUS(same[region\u{B7}nyc, age\u{B7}old])",
        "PREVIOUS(flip[age\u{B7}old, region\u{B7}nyc])",
        "PREVIOUS(w[age\u{B7}old])",
    ] {
        assert!(
            text.contains(expected),
            "expected {expected:?} in the partial; got: {text}"
        );
    }
    assert!(
        !text.contains("flip[region\u{B7}nyc, age\u{B7}old]"),
        "the reordered dep must not be pinned with the TARGET's tuple -- that \
         spelling compiles and reads flip[young,boston]; got: {text}"
    );
}

/// A BARE reference to the LIVE SOURCE inside a positionally-MAPPED per-element
/// target is pinned through the correspondence (GH #974, arm 2).
///
/// `growth[State,Age] = pop[State, young] * pop * 0.01` over a `pop[Region,Age]`
/// source: the emitting `PerElement` site is `pop[State, young]`, and the bare
/// `pop` beside it is a second occurrence the wrap freezes. `pin_bare_source_ref`
/// matched dimension NAMES only, found no `Region` coordinate in a `State`-keyed
/// projection, and left the reference bare -- so the freeze became a bare
/// multi-slot `PREVIOUS(pop)`, which cannot compile in a scalar fragment. All
/// four per-element scores on the edge read a constant 0 behind four `Assembly`
/// warnings.
///
/// The correspondence is positional, so `state·west` (State's first element)
/// reads `region·nyc` (Region's first) -- the same element the executed A2A
/// lowering reads for that slot.
#[test]
fn mapped_bare_live_source_ref_is_pinned_through_the_correspondence() {
    let project = TestProject::new("mapped_bare_live_source")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .named_dimension_with_mapping("State", &["west", "east"], "Region")
        .array_stock("pop[Region,Age]", "10", &["popinflow"], &[], None)
        .array_flow("popinflow[Region,Age]", "1", None)
        .array_flow("growth[State,Age]", "pop[State, young] * pop * 0.01", None)
        .array_stock("stock[State,Age]", "0", &["growth"], &[], None)
        .build_datamodel();

    let (db, model, source_project) = char_fixture_db(&project);
    assert!(
        fragment_compile_failures(&db, model, source_project).is_empty(),
        "the bare mapped live-source reference must be pinned, not left as a bare \
         multi-slot PREVIOUS(pop) that fails to compile"
    );
    let text = link_score_text(
        &db,
        model,
        source_project,
        &[
            "link_score\u{205A}pop[nyc,young]",
            "\u{2192}growth[west,young]",
        ],
    );
    assert!(
        text.contains("PREVIOUS(pop[region\u{B7}nyc, age\u{B7}young])"),
        "the bare live-source occurrence must be pinned to the mapped row for \
         this target element; got: {text}"
    );
    assert!(
        !text.contains("PREVIOUS(pop) "),
        "the bare multi-slot freeze must be gone; got: {text}"
    );

    // ...and the scores must be materially live, not a compiled constant 0.
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
        "expected four per-element mapped pop->growth link scores"
    );
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();
    for off in score_offsets {
        assert!(
            results.iter().any(|row| row[off] != 0.0),
            "mapped per-element pop->growth link score at offset {off} is all-zero"
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
    assert_char_fixture(
        "per_element_mixed_occurrences",
        per_element_mixed_occurrences_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "per_element_mapped_occurrence",
        per_element_mapped_occurrence_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "per_element_ambiguous_pin",
        per_element_ambiguous_pin_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "per_element_index_nested_occurrence",
        per_element_index_nested_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "per_element_dynamic_index",
        per_element_dynamic_index_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
}

// Finding 1 materiality guard: the DYNAMIC-index sibling of the ambiguous-pin
// guard. The frozen `pop[Region, idx]` occurrence lowers to a `PREVIOUS(idx, idx)`
// lag; the buggy blanket-skip would instead leave `idx` un-lagged, a
// value-divergent change the constant-`pop` text golden alone would still catch
// (the golden diverges) but whose RUNTIME consequence this guard freezes.
//
// GH #975 changed what the runtime discriminator can be. Before it, the lag's
// signature was a NaN at the first live step: the wrap emitted a UNARY
// `PREVIOUS(idx)`, which desugars to a `0` first-DT value, and `0` is out of
// range for a 1-based subscript -- so the synthesized capture-helper aux
// evaluated to NaN at t=0 and the outer `PREVIOUS` served that NaN as the first
// live step. The freeze now names the un-lagged index as its own first-DT value,
// so every step is finite and "finite at step 1" no longer discriminates
// anything.
//
// The model is therefore this guard's OWN rather than `per_element_dynamic_index_model`'s
// (which it used to share): there `pop[.,young]` and `pop[.,old]` hold the same
// value at every step, so the lagged and un-lagged index reads are numerically
// indistinguishable and the NaN was the only available signal. Here `skew` adds
// material to `old` and not to `young`, so the two Age rows diverge and the
// capture helper's series -- `pop[nyc, idx_{t-1}]` -- is different from the
// un-lagged `pop[nyc, idx_t]` at every step after the first.
//
// The guard pins (a) every fragment compiles (the `PREVIOUS(idx, idx)` helper is
// well-formed), (b) the capture helper is FINITE at t=0 and equals the LAGGED
// element read at every step -- checked against the model's own simulated `pop`
// series, with the un-lagged read asserted to differ so the check has teeth --
// and (c) the score itself is finite at the first live step and materially live
// after it.
fn per_element_dynamic_index_feedback_model() -> datamodel::Project {
    TestProject::new("dyn_index_feedback")
        .with_sim_time(0.0, 3.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .scalar_aux("idx", "1 + (TIME MOD 2)")
        // Per-Age drift with no dependency on `pop`: it separates the two Age
        // rows without adding a feedback loop that would change the shape under
        // test.
        .array_with_ranges("agedrift[Age]", vec![("young", "0"), ("old", "5")])
        .array_flow("skew[Region,Age]", "agedrift[Age]", None)
        .array_flow(
            "growth[Region]",
            "pop[Region, young] * pop[Region, idx] * 0.001",
            None,
        )
        .array_stock("pop[Region,Age]", "10", &["growth", "skew"], &[], None)
        .build_datamodel()
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
    let series = |name: &str| -> Vec<f64> {
        let off = compiled.offsets[&Ident::<Canonical>::new(name)];
        rows.iter().map(|r| r[off]).collect()
    };

    // (b) The capture helper: `pop[nyc, PREVIOUS(idx, idx)]`, hoisted out of the
    // frozen occurrence by `builtins_visitor::hoist_capture`. It is the slot
    // GH #975 was about -- the outer `PREVIOUS` serves its t=0 value as the
    // score's first live step -- so it is asserted directly rather than through
    // the score.
    let helper = "$\u{205A}$\u{205A}ltm\u{205A}link_score\u{205A}pop[nyc,young]\u{2192}growth[nyc]\u{205A}0\u{205A}arg0";
    let helper_series = series(helper);
    assert!(
        helper_series[0].is_finite(),
        "the synthesized PREVIOUS(idx) capture helper must have a well-defined \
         value at the initial step (GH #975); got {:?}",
        helper_series
    );
    // The lag itself, checked elementwise against the model's own `pop` series:
    // `idx` is `1 + (TIME MOD 2)`, so the LAGGED read selects `young` at t=0/1
    // and alternates thereafter, while the UN-lagged read selects the other row.
    // `young`/`old` diverge (see the fixture), so the two are distinguishable.
    let idx_series = series("idx");
    let young = series("pop[nyc,young]");
    let old = series("pop[nyc,old]");
    let row_at = |i: usize, index: f64| if index == 1.0 { young[i] } else { old[i] };
    let mut lag_is_observable = false;
    for t in 0..helper_series.len() {
        // At t=0 the freeze's own first-DT initial value applies: the un-lagged
        // index. Afterwards it is the index one step back.
        let lagged_index = if t == 0 {
            idx_series[0]
        } else {
            idx_series[t - 1]
        };
        assert_eq!(
            helper_series[t],
            row_at(t, lagged_index),
            "the capture helper must read pop at the LAGGED index at step {t}; \
             helper={helper_series:?} idx={idx_series:?} young={young:?} old={old:?}"
        );
        lag_is_observable |= row_at(t, lagged_index) != row_at(t, idx_series[t]);
    }
    assert!(
        lag_is_observable,
        "the fixture must distinguish the lagged read from the un-lagged one, or \
         the assertion above cannot detect a dropped lag; young={young:?} old={old:?}"
    );

    // (c) The scores themselves: finite from the first live step on (the NaN
    // GH #975 removed), and materially live afterwards.
    for off in score_offsets {
        assert!(
            rows[1][off].is_finite(),
            "dynamic-index pop->growth score at offset {off} step 1 must be finite: \
             the synthesized PREVIOUS(idx) capture helper is initialized (GH #975)"
        );
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
    assert_char_fixture(
        "agg_to_scalar_target",
        agg_to_scalar_model(),
        "\u{205A}agg\u{205A}0\u{2192}",
        FragmentExpectation::AllCompile,
    );
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
// previous(other[region·nyc, *]) * "$⁚ltm⁚agg⁚0") * 0.001`. The finding-2 gate
// teaches the GH #517 freeze about `live_reducer_text` so the enclosing reducer
// recurses, restoring HEAD's text; the golden below is byte-identical to HEAD
// `f057ef38`.
//
// That text used to have no codegen path (an array-valued `PREVIOUS`) and
// surfaced as a LOUD warned zero, which `b7898692` deliberately preserved over
// a silent structural zero. GH #995 phase C3 gave it one, so the equation is
// unchanged and now COMPILES -- see
// `agg_nested_reducer_partial_scores_full_attribution` for the number.
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
    assert_char_fixture(
        "agg_nested_reducer",
        agg_nested_reducer_model(),
        "link_score",
        // GH #995 Phase C3 closed the degradation this fixture was written to
        // pin. The hoisted `SUM(pop[*])` held live sits inside a DECLINED outer
        // reducer, so the `agg -> growth[e]` partial embeds `PREVIOUS` of a
        // wildcard slice (visible in the golden). That used to have no codegen
        // path and surfaced as a warned zero -- six Assembly warnings: the two
        // scores plus the four `PREVIOUS`-capture helper auxes their partials
        // synthesized. An array-valued `PREVIOUS` is now a view over
        // `prev_values`, so the partial compiles and the capture helpers are no
        // longer synthesized at all (the argument is array-shaped, so
        // `builtins_visitor` passes it through). The GOLDEN TEXT is byte-
        // identical across that change: what moved is compilability, not the
        // emitted equation. `agg_nested_reducer_partial_scores_full_attribution`
        // pins the resulting NUMBER.
        FragmentExpectation::AllCompile,
    );
}

// Finding 2 materiality guard, in its post-GH-#995 form. The pre-fix
// transform-first freeze produced a SILENT clean-compiling zero (zero warnings)
// -- the worst failure class -- and the guard originally pinned the LOUD
// warned zero that replaced it (six `Assembly` "failed to compile" warnings: the
// two `agg⁚0->growth[e]` scores plus their four `PREVIOUS`-capture helper auxes).
// Phase C3 gave an array-valued `PREVIOUS` a snapshot-buffer view, so that
// partial now compiles and the guard pins the NUMBER instead: a warned zero and
// a silent zero are both ruled out by asserting the score's hand-derived value.
// `pop` is a stock so the edge is causally live.
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
fn agg_nested_reducer_partial_scores_full_attribution() {
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
    assert!(
        frag_failures.is_empty(),
        "every fragment must compile now that an array-valued PREVIOUS has a \
         view; a warned zero would show up here: {frag_failures:?}"
    );

    // The number, derived by hand from the fixture rather than recorded from a
    // run. `matrix` and `other` are constant 1 and Region has two elements, so
    //
    //   growth[e] = SUM(matrix[e,*] * other[nyc,*] * agg) * 0.001
    //             = (1*1*agg + 1*1*agg) * 0.001 = 0.002 * agg
    //
    // and the ceteris-paribus partial for `agg -> growth[e]` -- which freezes
    // the two co-source slices at their PREVIOUS values and holds `agg` live --
    // is `SUM(PREV(matrix[e,*]) * PREV(other[nyc,*]) * agg) * 0.001`. The frozen
    // slices are the same constant 1, so the partial equals `growth[e]`
    // EXACTLY. The score is then
    //
    //   SAFEDIV(partial - PREV(growth[e]), ABS(growth[e] - PREV(growth[e])))
    //     * SIGN(agg - PREV(agg))
    //   = SIGN(growth[e] - PREV(growth[e])) * SIGN(agg - PREV(agg))
    //   = 1
    //
    // because `pop` is a stock fed by `growth > 0`, so both `agg` and `growth`
    // increase every step. 1.0 is full attribution, which is the right answer:
    // `agg` is the only changing driver of `growth`. The first saved step is 0
    // by the score's own `TIME = INITIAL_TIME` guard.
    //
    // Every wrong reading lands somewhere else: a failed or stubbed fragment
    // reads a constant 0, and freezing the whole declined outer reducer (the
    // GH #517 arm) makes the partial equal the frozen anchor, i.e. also 0.
    //
    // TWO readings this fixture CANNOT tell apart, disclosed rather than left to
    // be discovered, both because `matrix` and `other` are the constant 1:
    // reading them at their CURRENT rather than their PREVIOUS values, and
    // reading the WRONG ROW of the snapshot for a `region·<r>` pin. Neither is
    // uncovered -- `array_operand_materialization_tests`'
    // `previous_operands_are_views_over_the_prev_snapshot` carries the lag over
    // time-varying arrays and `a_prev_view_of_a_row_slice_reads_that_row_of_the_snapshot`
    // carries the row over rows two orders of magnitude apart -- but they are
    // covered THERE, over the view arithmetic, not here. What this fixture is
    // for is the attribution value, which is the thing the LTM wrap decides.
    let compiled = crate::db::compile_project_incremental(&db, source_project, "main")
        .expect("the LTM-enabled fixture should compile");
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    let results = vm.into_results();
    for elem in ["nyc", "boston"] {
        let score = format!(
            "$\u{205A}ltm\u{205A}link_score\u{205A}$\u{205A}ltm\u{205A}agg\u{205A}0\u{2192}growth[{elem}]"
        );
        let offset = *compiled
            .offsets
            .get(score.as_str())
            .unwrap_or_else(|| panic!("{score} has no results offset"));
        let series: Vec<f64> = (0..results.step_count)
            .map(|step| results.data[step * results.step_size + offset])
            .collect();
        assert_eq!(
            series[0], 0.0,
            "{score}: the first step is guarded to 0 by the score equation"
        );
        for (step, value) in series.iter().enumerate().skip(1) {
            assert!(
                (value - 1.0).abs() < 1e-12,
                "{score}: step {step} must be full attribution (1.0), got {value}; \
                 whole series {series:?}"
            );
        }
    }
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
    assert_char_fixture(
        "agg_to_arrayed_target",
        agg_to_arrayed_model(),
        "\u{205A}agg\u{205A}0\u{2192}",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "arrayed_agg_to_target",
        arrayed_agg_model(),
        "link_score",
        FragmentExpectation::AllCompile,
    );
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
    assert_char_fixture(
        "arrayed_target_slot_scores",
        arrayed_target_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
}

// ---------------------------------------------------------------------------
// Model E2: the same `Ast::Arrayed` target WITHOUT an EXCEPT default -- the GH
// #977 omission path.
//
// Model E's `mp` carries an EXCEPT default, and a target with one is pinned to
// `ZeroSlotPolicy::Materialize`: an absent slot there takes the DEFAULT
// equation, not zero, so no arm may be dropped. Model E is the only arrayed
// target in this file, which is why the omission reached ZERO characterization
// coverage when it landed -- this fixture is that coverage.
//
// `mp[la]` reads no `pop` at all, so for either `pop[e] -> mp` edge every
// occurrence in the `la` arm is frozen by the ceteris-paribus wrap and the arm
// is provably `PREVIOUS(mp)`. It is omitted from the element map, which
// `compiler::expand_per_element` lowers to a single constant-zero
// assign. What the golden shows is the slot being ABSENT -- deliberately
// distinct from an arm that is present holding a `"0"` partial, which is what a
// generator that gave up would emit.
// ---------------------------------------------------------------------------

fn arrayed_target_no_default_model() -> datamodel::Project {
    let mut p = TestProject::new("arrayed_target_no_default_char")
        .named_dimension("Region", &["nyc", "boston", "la"])
        .aux("drift", "1", None)
        .array_aux("pop[Region]", "10");
    // `array_with_ranges` builds an `Equation::Arrayed` with no default, so
    // `apply_default_to_missing` is false and omission is sound.
    p = p.array_with_ranges(
        "mp[Region]",
        vec![
            ("nyc", "(pop[nyc] - pop[boston]) * 0.01"),
            ("boston", "(pop[boston] - pop[nyc]) * 0.01"),
            ("la", "drift * 0.01"),
        ],
    );
    p.array_stock("stock[Region]", "0", &["mpflow"], &[], None)
        .array_flow("mpflow[Region]", "mp", None)
        .build_datamodel()
}

#[test]
fn char_arrayed_target_no_default_slot_scores() {
    assert_char_fixture(
        "arrayed_target_no_default_slot_scores",
        arrayed_target_no_default_model(),
        "link_score\u{205A}pop",
        FragmentExpectation::AllCompile,
    );
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
// instead would emit `SUM(PREVIOUS(w[from, *]))`, which was a loud compile
// failure and a silently-zeroed score when written (GH #517: a PREVIOUS of an
// array view had no codegen path). GH #995 phase C3 gave it one, so that form
// compiles now -- but the SELECTION rule this golden pins is unchanged and is
// not about compilability: the changed-first partial freezes the whole reducer
// because the live occurrence is index-nested, and recursing would change which
// occurrence is held live.
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
    assert_char_fixture(
        "reducer_index_nested_freeze",
        reducer_index_nested_model(),
        "link_score\u{205A}from\u{2192}to",
        FragmentExpectation::ExpectedFailures {
            // This fixture's OWN model does not compile: `to = SUM(w[from, *]) +
            // from` uses a scalar variable as a subscript index, which the engine
            // rejects with `ArrayReferenceNeedsExplicitSubscripts` (an
            // Error-severity diagnostic on `to`). The fixture is deliberately
            // that shape -- it pins the Fig. 2 Q4 index-nested SELECTION
            // semantics at the text level -- so its `PREVIOUS`-capture helper
            // inherits the un-compilable subscript. Zero failures is therefore
            // not reachable here without changing what the fixture pins.
            why: "the fixture model itself is rejected \
                  (ArrayReferenceNeedsExplicitSubscripts: a scalar used as a \
                  subscript index), so its PREVIOUS-capture helper cannot compile",
            vars: &[
                "$\u{205A}$\u{205A}ltm\u{205A}link_score\u{205A}from\u{2192}to\u{205A}0\u{205A}arg0",
            ],
        },
    );
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
// read). This pins the already-lagged selection semantics the wrap reproduces by
// recognizing a `PREVIOUS`/`INIT` node structurally: it suppresses the wrap of an
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
    assert_char_fixture(
        "already_lagged_other_dep",
        already_lagged_other_dep_model(),
        "link_score\u{205A}from\u{2192}to",
        FragmentExpectation::AllCompile,
    );
}

// ---------------------------------------------------------------------------
// Model H (Track A3 stage 2b, finding 1): a SCALAR feeder read BARE both
// OUTSIDE and INSIDE a HOISTED reducer of a SCALAR target.
//
// `total = scale + SUM(arr[*] * scale)`: `scale` appears bare (outside the
// reducer) AND as the reducer's scalar feeder inside `SUM(arr[*] * scale)`. The
// `scale -> total` Bare link score is the finding-1 probe. Historically the
// LEGACY `(from, to)`-keyed `link_score_equation_text` query (which assembly
// compiled) passed an empty occurrence stream and froze the whole `SUM(...)`
// reducer -- changed-FIRST -- while the SHAPED emitter (which
// `model_ltm_variables` reports/serializes) threaded the real stream and
// recursed into it -- changed-LAST. Stage 2b re-aligned the legacy query onto
// the shaped derivation; Track A3 stage 3a then DELETED the legacy query
// outright: assembly's sub-case (a) now sources from
// `shaped_link_score(.., Bare)` directly, so the compiled and
// reported equations are one value and cannot drift. This golden pins the
// emitted (changed-LAST) text -- the numerator subtracts
// `PREVIOUS(scale) + sum(arr[*] * PREVIOUS(scale))` from the live `total`, i.e.
// `scale` is held live in BOTH occurrences. GH #517/#743: the changed-LAST
// convention is the correct one (freeze the target's OTHER inputs, hold the
// scored source live everywhere it appears).
// ---------------------------------------------------------------------------

fn scalar_feeder_bare_in_hoisted_reducer_model() -> datamodel::Project {
    TestProject::new("scalar_bare_in_reducer_char")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .array_aux("arr[D1]", "1")
        .aux("scale", "pop * 0.01", None)
        .aux("total", "scale + SUM(arr[*] * scale)", None)
        .flow("growth", "total * 0.001", None)
        .stock("pop", "1", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_scalar_feeder_bare_in_hoisted_reducer() {
    // Every `scale -> X` score: the site-1 `scale -> total` Bare score (both
    // occurrences held live, changed-LAST) plus the `scale -> $⁚ltm⁚agg⁚0`
    // scalar-feeder score, so the whole feeder attribution is frozen.
    assert_char_fixture(
        "scalar_feeder_bare_in_hoisted_reducer",
        scalar_feeder_bare_in_hoisted_reducer_model(),
        "link_score\u{205A}scale",
        FragmentExpectation::AllCompile,
    );
}

// ---------------------------------------------------------------------------
// Model H2 (Track A3 stage 3a, finding 1): the ARRAYED-target sibling of
// Model H -- a SCALAR feeder read BARE alongside a HOISTED reducer, but now
// inside an A2A (arrayed) target rather than a scalar one.
//
// `share[D1] = SUM(arr[*] * scale) + scale`: the `+ scale` is a Bare read of
// the scalar `scale` in an A2A-over-`D1` body, so `scale -> share` has a Bare
// site whose ceteris-paribus partial the shaped query builds as an
// `ApplyToAll(["D1"], ..)` (the target is arrayed). But `link_score_dimensions`
// returns `[]` for a scalar-source -> arrayed-target edge (the feeder carries
// no dimensions to inherit), so `emit_per_shape_link_scores` REPORTS the score
// as a dims-empty SCALAR var (`retarget_ltm_equation_dims(.., [])` collapses the
// A2A equation to `Scalar`) laid out with one slot, and the assembly fragment
// compiler routes it through sub-case (a) -> the salsa-cached
// `compile_ltm_var_fragment`.
//
// The regression this pins: when `compile_ltm_var_fragment` compiled the shaped
// Bare query's RAW `ApplyToAll` equation (writing element offsets 0 AND 1 into
// a 1-slot var) while the emission loop scalarized the same score, the compiled
// fragment disagreed with the reported var and `compile_project_incremental`
// hard-failed with `NotSimulatable` ("element_offset 1 out of bounds"). Sourcing
// the fragment from `scalarize_ltm_equation(shaped_raw)` -- identical to the
// reported `retarget_ltm_equation_dims(shaped_raw, [])` for the dims-empty
// sub-case (a) var, and a no-op for a scalar target -- keeps compiled == reported
// by construction, so the score degrades gracefully (the scalarized A2A text
// references `arr[*]` in scalar context, so it warn-stubs to constant 0, exactly
// as the reported var does) instead of aborting the whole model's compilation.
// ---------------------------------------------------------------------------

fn scalar_feeder_bare_in_arrayed_reducer_model() -> datamodel::Project {
    TestProject::new("scalar_bare_in_arrayed_reducer_char")
        .with_sim_time(0.0, 10.0, 1.0)
        .named_dimension("D1", &["a", "b"])
        .array_aux("arr[D1]", "1")
        .aux("scale", "pop * 0.01", None)
        .array_aux("share[D1]", "SUM(arr[*] * scale) + scale")
        .flow("growth", "SUM(share[*]) * 0.001", None)
        .stock("pop", "1", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn scalar_feeder_bare_in_arrayed_reducer_compiles_and_simulates() {
    use salsa::Setter;

    let project = scalar_feeder_bare_in_arrayed_reducer_model();
    let mut db = SimlinDb::default();
    let (source_project, _source_model) = {
        let sync = sync_from_datamodel(&db, &project);
        (sync.project, sync.models["main"].source)
    };
    source_project.set_ltm_enabled(&mut db).to(true);

    // The core regression: a scalar feeder read bare beside a hoisted reducer in
    // an arrayed A2A target must NOT turn LTM compilation into a hard failure.
    // Pre-fix this `.expect` panicked on `NotSimulatable` ("element_offset 1 out
    // of bounds for variable $⁚ltm⁚link_score⁚scale→share (size 1)").
    let compiled = compile_project_incremental(&db, source_project, "main")
        .expect("LTM incremental compilation should succeed for an arrayed target");

    // The dims-empty scalar `scale -> share` score exists in the layout with a
    // single slot -- the shape the emission loop reports.
    let share_score_offsets: Vec<usize> = compiled
        .offsets
        .iter()
        .filter(|(k, _)| k.as_str() == "$\u{205A}ltm\u{205A}link_score\u{205A}scale\u{2192}share")
        .map(|(_, v)| *v)
        .collect();
    assert_eq!(
        share_score_offsets.len(),
        1,
        "expected exactly one scalar scale->share link score slot"
    );

    // Simulation runs to completion and the model's OWN variables are
    // unperturbed by the degraded score.
    let mut vm = crate::vm::Vm::new(compiled.clone()).expect("VM creation should succeed");
    vm.run_to_end()
        .expect("simulation should run to completion");
    let results = vm.into_results();
    for (key, off) in &compiled.offsets {
        if key.as_str().starts_with("$\u{205A}ltm") {
            continue;
        }
        for row in results.iter() {
            assert!(
                row[*off].is_finite(),
                "model variable {key} must stay finite; the LTM overlay must not \
                 perturb the base simulation"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Model P: a source whose canonical name CANNOT be spelled as a bare
// identifier.
//
// A clone of Model A (the `PerElement` mixed iterated+literal shape) with the
// source renamed `pop` -> `1pop`. XMILE lets a modeler quote any name, so
// `"1pop"` is legal and canonicalizes to `1pop`; the equation lexer, though,
// only starts an identifier on `XID_Start`, so bare `1pop` lexes as the number
// `1` followed by the identifier `pop`.
//
// This fixture exists to make the CORPUS sensitive to the unquotable-generation
// class, which two independent bugs hit in one day (the `1pop`-style leading
// digit fixed in `17d4e7c0`, and the bare-keyword class of GH #976). The other
// fifteen fixtures are structurally blind to it: they pin generated TEXT, and a
// generated equation that is stably unparseable keeps its golden green forever
// while its fragment compiles to no bytecode and the score reads a constant 0.
// Verified: re-introducing the `quote_ident` regression leaves every other
// fixture green and fails THIS one, via the `AllCompile` expectation.
//
// A leading DIGIT rather than a keyword, and one fixture rather than two: since
// GH #976 both classes are decided by the SAME `ast::needs_quoting` delegation,
// so a second golden would re-test the same branch of `quote_ident` with a
// different input. The keyword class is pinned where the distinction actually
// lives -- `ltm_tests::link_score_quotes_every_keyword_named_source`, which
// ranges over the whole keyword table and asserts the generated arm PARSES.
// ---------------------------------------------------------------------------

fn unquotable_source_name_model() -> datamodel::Project {
    TestProject::new("unquotable_source_name_char")
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_aux("1pop[Region,Age]", "10")
        .array_flow("growth[Region]", "\"1pop\"[Region, young] * 0.1", None)
        .array_stock("stock[Region]", "0", &["growth"], &[], None)
        .build_datamodel()
}

#[test]
fn char_unquotable_source_name() {
    assert_char_fixture(
        "unquotable_source_name",
        unquotable_source_name_model(),
        "link_score\u{205A}1pop",
        FragmentExpectation::AllCompile,
    );
}

// ---------------------------------------------------------------------------
// GH #984, the REACHABLE half, end to end: a `LOOKUP` inside a scored target's
// equation. No other char fixture has one, so nothing else covers the arm at
// the model level.
//
// The runtime-index half of #984 has NO model fixture on purpose. A `LOOKUP`
// table argument carrying a runtime index does not compile on this engine at
// all: the TARGET's own fragment fails with an `EquationError` `DoesNotExist`
// and `compile_project_incremental` reports `NotSimulatable`, so the silent
// wrong score the issue describes cannot be reached from a compiling model
// today. Three independent constructions were tried and all three fail the
// same way -- an arrayed lookup-only table (`LOOKUP(gtab[idx], src)`), an
// arrayed graphical-function aux, and Vensim's own per-element arrayed-GF
// application (`g[idx](src)` via the MDL reader) -- while every static-index
// twin compiles. The wrap-level behaviour is therefore pinned where it is
// decided, by `ltm_augment::tests::test_partial_equation_lookup_table_index_is_frozen`
// (the general path) and `per_element_pin_freezes_a_runtime_table_arg_index`
// (the per-element path); fabricating a model fixture here would mean shipping
// one that does not exercise the engine.
// ---------------------------------------------------------------------------

/// The MDL source for the arrayed-GF `LOOKUP` fixtures: `y = g[<index>](src)`,
/// Vensim's per-element arrayed-GF application, which lowers to
/// `LOOKUP(g[<index>], src)`. That is how a `LOOKUP` gets into a target equation
/// without the LTM layer synthesizing it.
fn arrayed_gf_lookup_mdl(index: &str) -> String {
    format!(
        "\
{{UTF-8}}
D: A1, A2 ~~|
g[A1]( (0,0),(1,10),(2,20) ) ~~|
g[A2]( (0,0),(1,100),(2,200) ) ~~|
idx = 1 + MODULO(Time, 2) ~~|
inflow = 1 ~~|
src= INTEG(inflow, 1) ~~|
y = g[{index}](src) ~~|
INITIAL TIME = 0 ~~|
FINAL TIME = 3 ~~|
SAVEPER = 1 ~~|
TIME STEP = 1 ~~|
"
    )
}

/// GH #984 through the PRODUCTION path: a runtime table index is frozen when the
/// dep set is the one production derives, not one a test hands in.
///
/// This is the test the first version of the fix needed and did not have. The
/// wrap freezes an ident only if it is in `other_deps`, and `other_deps` comes
/// from `variable::identifier_set`, whose `BuiltinContents::LookupTable` arm
/// never walks the table expression -- so `idx`, referenced only as a table
/// index, is not a dependency of `y` and the freeze could not fire. The unit
/// tests supplied `idx` by hand and passed on an input production cannot
/// produce. Here nothing is supplied: `model_ltm_variables` builds the dep set
/// itself, so the assertion below is only satisfiable by
/// `freeze_lookup_table_indices` widening the set from the index's own idents.
///
/// The target does NOT compile -- a `LOOKUP` table argument with a runtime index
/// is refused upstream, which is the documented reachability limitation above --
/// so this asserts the generated TEXT rather than a score series, and asserts the
/// upstream refusal too so the reason is recorded rather than hidden.
#[test]
fn lookup_table_runtime_index_is_frozen_through_the_production_path() {
    use crate::db::{DiagnosticError, collect_model_diagnostics};

    let project = crate::open_vensim(&arrayed_gf_lookup_mdl("idx")).expect("the MDL parses");

    // The upstream limitation, asserted rather than assumed: `y` itself has no
    // bytecode, which is why there is no numeric oracle here.
    let plain_db = SimlinDb::default();
    let plain = sync_from_datamodel(&plain_db, &project);
    assert!(
        collect_model_diagnostics(&plain_db, plain.models["main"].source, plain.project)
            .iter()
            .any(|d| d.variable.as_deref() == Some("y")
                && matches!(&d.error, DiagnosticError::Equation(_))),
        "the fixture's whole point is that a runtime table index is refused \
         upstream; if `y` now compiles, replace this text assertion with a \
         numeric one"
    );

    let (db, model, source_project) = char_fixture_db(&project);
    let text = link_score_text(
        &db,
        model,
        source_project,
        &["link_score\u{205A}src\u{2192}y"],
    );
    assert!(
        text.contains("lookup(g[PREVIOUS(idx, idx)], src)"),
        "the table index must be frozen even though production does not classify \
         it as a dependency, and (GH #975) its freeze must name the un-lagged \
         index as its first-DT initial value rather than the desugared 0, which \
         is out of range for a 1-based subscript; got: {text}"
    );
    assert!(
        !text.contains("lookup(PREVIOUS("),
        "the table HEAD must stay bare; got: {text}"
    );
}

/// A static arrayed-GF table index survives the ceteris-paribus wrap untouched,
/// and the score compiles.
///
/// Both halves of the `LOOKUP` arm are visible in one partial: the table HEAD
/// `g[a1]` is NOT wrapped in `PREVIOUS` (a graphical-function table has no value
/// slot, so `lookup(PREVIOUS(g[a1]), ...)` would fail to compile and zero the
/// score -- the WRLD3 failure mode), and its STATIC element index is not frozen
/// either (there is no runtime read to hold). The live source stays live in the
/// second argument.
///
/// This is the reachable half of the arm, and the control for its sibling
/// [`lookup_table_runtime_index_is_frozen_through_the_production_path`]: same
/// model, same production path, index static instead of runtime.
#[test]
fn lookup_table_head_and_static_index_survive_the_wrap() {
    let project = crate::open_vensim(&arrayed_gf_lookup_mdl("A1")).expect("the MDL parses");
    let (db, model, source_project) = char_fixture_db(&project);
    assert!(
        fragment_compile_failures(&db, model, source_project).is_empty(),
        "the table-mediated link score must compile"
    );

    let text = link_score_text(
        &db,
        model,
        source_project,
        &["link_score\u{205A}src\u{2192}y"],
    );
    assert!(
        text.contains("lookup(g[a1], src)"),
        "the table head must stay bare and its static index untouched; got: {text}"
    );
    assert!(
        !text.contains("lookup(PREVIOUS("),
        "a PREVIOUS of a table has no value slot; got: {text}"
    );
}

// ---------------------------------------------------------------------------
// P2-1 / P2-2 of the whole-branch review: the per-element pin projection keyed
// its axis lookup by dimension NAME, which is not an axis identity. Both shapes
// below COMPILE AND RUN, so each was a silent wrong row rather than a latent
// one, and each is measured against the VM before its pin is asserted.
//
// This is the second time in this area a name-keyed table produced a wrong row
// (GH #986 was the first), which is why the projection now asks
// `compiler::dimensions::allocate_implicit_axes_partial` instead of restating
// the rule.
// ---------------------------------------------------------------------------

/// Read one variable's final-step value out of a compiled+run model.
fn final_value(project: &datamodel::Project, name: &str) -> f64 {
    let db = SimlinDb::default();
    let sp = sync_from_datamodel(&db, project).project;
    let compiled = compile_project_incremental(&db, sp, "main").expect("the fixture compiles");
    let off = *compiled
        .offsets
        .iter()
        .find(|(k, _)| k.as_str() == name)
        .unwrap_or_else(|| panic!("no slot named {name}"))
        .1;
    let mut vm = crate::vm::Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("simulation should run");
    vm.into_results().iter().next_back().expect("a saved step")[off]
}

fn repeated_dimension_model() -> datamodel::Project {
    TestProject::new("repeated_target_dim")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .array_with_ranges_direct(
            "w",
            vec!["Region".to_string()],
            vec![("nyc", "1"), ("boston", "2")],
            None,
        )
        // A stock, so the model is stateful and LTM scores it at all.
        .flow("inflow", "1", None)
        .stock("driver", "3", &["inflow"], &[], None)
        // `target` repeats `Region`: two axes, one name.
        .array_aux_direct(
            "target",
            vec!["Region".to_string(), "Region".to_string()],
            "driver * w",
            None,
        )
        .build_datamodel()
}

/// A target that REPEATS a dimension gives a bare dep the FIRST axis, and the
/// pin must say so (P2-1).
///
/// `target[Region,Region] = driver * w` with `w[Region]`: the simulation reads
/// `w` at the FIRST `Region` coordinate, so `target[nyc,boston]` reads `w[nyc]`.
/// The pin projection keyed its lookup by dimension name, so the second axis
/// overwrote the first and it emitted `PREVIOUS(w[region·boston])` -- a fragment
/// that compiles and reads the other row.
#[test]
fn repeated_target_dimension_reads_the_first_axis() {
    let project = repeated_dimension_model();

    // What the simulation reads, measured. `w[nyc]` and `w[boston]` differ, so
    // the two candidate reads are distinguishable.
    assert_ne!(
        final_value(&project, "w[nyc]"),
        final_value(&project, "w[boston]")
    );
    assert_eq!(
        final_value(&project, "target[nyc,boston]"),
        final_value(&project, "driver") * final_value(&project, "w[nyc]"),
        "a bare dep under a repeated-dimension target reads the FIRST axis"
    );

    // ...and the pin spells that row.
    let (db, model, source_project) = char_fixture_db(&project);
    let text = link_score_text(
        &db,
        model,
        source_project,
        &["link_score\u{205A}driver\u{2192}target[nyc,boston]"],
    );
    assert!(
        text.to_lowercase().contains("previous(w[region\u{B7}nyc])"),
        "the pin must read the FIRST Region axis, as the simulation does; got: {text}"
    );
    assert!(
        !text.contains("previous(w[region\u{B7}boston])"),
        "a name-keyed lookup keeps only the LAST axis and spells the wrong row; \
         got: {text}"
    );
}

fn doubly_mapped_model() -> datamodel::Project {
    let mut project = TestProject::new("doubly_mapped")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("T", &["t1", "t2"])
        .named_dimension("U", &["u1", "u2"])
        .array_with_ranges_direct(
            "aw",
            vec!["A".to_string()],
            vec![("a1", "1"), ("a2", "2")],
            None,
        )
        .array_with_ranges_direct(
            "bw",
            vec!["B".to_string()],
            vec![("b1", "10"), ("b2", "20")],
            None,
        )
        .flow("inflow", "1", None)
        .stock("driver", "1", &["inflow"], &[], None)
        .array_aux_direct(
            "dep",
            vec!["A".to_string(), "B".to_string()],
            "aw[A] + bw[B]",
            None,
        )
        .array_aux_direct(
            "target",
            vec!["T".to_string(), "U".to_string()],
            "driver * dep",
            None,
        )
        .build_datamodel();
    // `A` and `B` each map to BOTH target dimensions, so an independent per-axis
    // search hands both of them `T`.
    let both = || {
        vec![
            datamodel::DimensionMapping {
                target: "T".to_string(),
                element_map: vec![],
            },
            datamodel::DimensionMapping {
                target: "U".to_string(),
                element_map: vec![],
            },
        ]
    };
    let mut a =
        datamodel::Dimension::named("A".to_string(), vec!["a1".to_string(), "a2".to_string()]);
    a.mappings = both();
    let mut b =
        datamodel::Dimension::named("B".to_string(), vec!["b1".to_string(), "b2".to_string()]);
    b.mappings = both();
    project.dimensions.push(a);
    project.dimensions.push(b);
    project
}

/// Two dependency axes that can each map to either target axis are allocated
/// ONE-TO-ONE, in declaration order (P2-2).
///
/// `target[T,U] = driver * dep` with `dep[A,B]`, where `A` and `B` both carry
/// positional mappings to both `T` and `U`. The simulation consumes each target
/// axis once, so `target[t1,u2]` reads `dep[a1,b2]`. The pin projection searched
/// per dependency axis independently with no `used` set, so both claimed `T` and
/// it emitted `PREVIOUS(dep[a1,b1])` -- again compilable and again the wrong row.
#[test]
fn doubly_mapped_dep_axes_are_allocated_one_to_one() {
    let project = doubly_mapped_model();

    // What the simulation reads, measured; the two candidate rows differ.
    assert_ne!(
        final_value(&project, "dep[a1,b1]"),
        final_value(&project, "dep[a1,b2]")
    );
    assert_eq!(
        final_value(&project, "target[t1,u2]"),
        final_value(&project, "driver") * final_value(&project, "dep[a1,b2]"),
        "each target axis is consumed once: A takes T, B takes U"
    );

    // ...and the pin spells that row.
    let (db, model, source_project) = char_fixture_db(&project);
    let text = link_score_text(
        &db,
        model,
        source_project,
        &["link_score\u{205A}driver\u{2192}target[t1,u2]"],
    );
    assert!(
        text.to_lowercase()
            .contains("previous(dep[a\u{B7}a1, b\u{B7}b2])"),
        "the pin must allocate one-to-one, as the simulation does; got: {text}"
    );
    assert!(
        !text.contains("previous(dep[a\u{B7}a1, b\u{B7}b1])"),
        "an independent per-axis search gives both dep axes the FIRST target \
         axis and spells the wrong row; got: {text}"
    );
}

/// A `PerElement` edge whose target REPEATS a dimension is declined loudly.
///
/// The scalar-source and agg emitters project positionally and handle a repeated
/// dimension correctly (`repeated_target_dimension_reads_the_first_axis`), but the
/// per-element emitter's row derivations address a target axis by its dimension
/// NAME -- that is what an `AxisRead::Iterated` carries -- and two axes with one
/// name are indistinguishable to them. Emitting anyway produces a partial that
/// compiles and reads whichever axis the lookup resolved to, so the edge is
/// skipped with a `Warning` instead: loud beats a plausible wrong number, which
/// is the trade this area has taken every time.
///
/// The fixture's own equation compiles and runs, so the decline is LTM's and not
/// a consequence of an unsupported model.
#[test]
fn per_element_edge_declines_a_repeated_dimension_target() {
    use crate::db::{DiagnosticError, DiagnosticSeverity, collect_model_diagnostics};

    let project = TestProject::new("per_element_repeated_dim")
        .with_sim_time(0.0, 1.0, 1.0)
        .named_dimension("Region", &["nyc", "boston"])
        .named_dimension("Age", &["young", "old"])
        .array_flow("growth[Region,Age]", "1", None)
        .array_stock("pop[Region,Age]", "10", &["growth"], &[], None)
        // `pop[Region, young]` is a PerElement site; the target repeats `Region`.
        .array_aux_direct(
            "target",
            vec!["Region".to_string(), "Region".to_string()],
            "pop[Region, young]",
            None,
        )
        .build_datamodel();

    // The model itself is fine -- the decline below is LTM's.
    let plain_db = SimlinDb::default();
    let plain = sync_from_datamodel(&plain_db, &project).project;
    compile_project_incremental(&plain_db, plain, "main")
        .expect("the repeated-dimension model compiles");

    let (db, model, source_project) = char_fixture_db(&project);
    let diags = collect_model_diagnostics(&db, model, source_project);
    assert!(
        diags
            .iter()
            .any(|d| d.severity == DiagnosticSeverity::Warning
                && matches!(&d.error, DiagnosticError::Assembly(m)
                if m.contains("repeats a dimension") && m.contains("pop") && m.contains("target"))),
        "the edge must be declined with a Warning naming it; got {:?}",
        diags.iter().map(|d| &d.error).collect::<Vec<_>>()
    );

    // ...and no per-element score is emitted for it, rather than a wrong one.
    let ltm = model_ltm_variables(&db, model, source_project);
    let per_element: Vec<&String> = ltm
        .vars
        .iter()
        .map(|v| &v.name)
        .filter(|n| n.contains("link_score\u{205A}pop[") && n.contains("\u{2192}target["))
        .collect();
    assert!(
        per_element.is_empty(),
        "a declined edge must emit no per-element score; got {per_element:?}"
    );
}

// ---------------------------------------------------------------------------
// P2-8: an index on an INDEXED axis has no `dim·elem` spelling.
//
// The reported mechanism does not hold, and the measurements are in the test
// below rather than only in the report. `q["1"]` -- a legal quoted
// numeric-named variable used as an index -- is NOT a runtime read: the
// compiler's dynamic-subscript lowering resolves it through
// `Dimension::get_offset`, which for an indexed dimension parses the
// identifier's text, so `q["1"]` and `q[1]` are the same static position. The
// defect at that line was the QUALIFICATION, not the resolution.
// ---------------------------------------------------------------------------

/// The `q["1"]` fixture: an indexed dimension, a variable legally named `1`,
/// and a target that indexes `q` with it. `index_expr` is the spelling under
/// test.
fn indexed_axis_index_model(index_expr: &str, one_value: &str) -> datamodel::Project {
    let mut project = TestProject::new("indexed_axis_index")
        .with_sim_time(0.0, 2.0, 1.0)
        .aux("\"1\"", one_value, None)
        .flow("inflow", "1", None)
        .stock("source", "1", &["inflow"], &[], None)
        .aux("target", &format!("source + q[{index_expr}]"), None)
        .build_datamodel();
    project
        .dimensions
        .push(datamodel::Dimension::indexed("D".to_string(), 3));
    project
        .models
        .get_mut(0)
        .expect("main model")
        .variables
        .push(crate::datamodel::Variable::Aux(datamodel::Aux {
            ident: "q".to_string(),
            equation: crate::datamodel::Equation::Arrayed(
                vec!["D".to_string()],
                vec![
                    ("1".to_string(), "100".to_string(), None, None),
                    ("2".to_string(), "200".to_string(), None, None),
                    ("3".to_string(), "300".to_string(), None, None),
                ],
                None,
                false,
            ),
            documentation: String::new(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    project
}

/// A quoted numeric-named variable used as an index into an INDEXED axis is a
/// STATIC position, and the ceteris-paribus wrap must leave it spellable.
///
/// Batch 1 made `needs_quoting` leading-digit-aware so a variable named `1`
/// round-trips as `"1"`, which is what makes this shape appear in generated
/// text at all; the two changes meet here.
///
/// The first half is the measurement that settles what the reference means. A
/// review pass read `compiler::subscript::normalize_subscripts3` -- which does
/// decline a bare identifier on an indexed axis -- and concluded the compiler
/// treats `q["1"]` as a runtime read of the variable. It does not: declining
/// there routes to the DYNAMIC lowering (`compiler::context`), whose first move
/// is `Dimension::get_offset(ident)`, and that parses an indexed axis's
/// identifier text into a position. So `q["1"]` is position 1 no matter what
/// the variable holds -- asserted here by running the model with two different
/// values for it.
///
/// The second half is the defect that WAS there: the wrap qualified the
/// resolved element as `d·1`, a spelling `DimensionsContext::lookup` resolves
/// only for a NAMED dimension. The capture helper that froze it could not
/// compile, so both link scores read a constant 0 behind `Assembly` warnings.
/// A loud degradation rather than a wrong row -- and the reported fix, refusing
/// to resolve the identifier at all, would have turned it INTO a wrong row by
/// freezing an index the compiler resolves statically (`q[PREVIOUS("1")]` reads
/// whatever the variable held last step; the equation reads `q[1]`).
#[test]
fn quoted_numeric_index_on_an_indexed_axis_is_a_static_position() {
    // 1. What the reference MEANS, measured: independent of the variable's value.
    for (one_value, label) in [("2", "variable 1 = 2"), ("3", "variable 1 = 3")] {
        let project = indexed_axis_index_model("\"1\"", one_value);
        assert_eq!(
            final_value(&project, "target"),
            final_value(&project, "source") + final_value(&project, "q[1]"),
            "{label}: `q[\"1\"]` is the static position 1, not a read of the \
             variable named 1"
        );
        // Non-vacuity: the candidate rows differ, so a runtime read would show.
        assert_ne!(final_value(&project, "q[1]"), final_value(&project, "q[2]"));
        assert_ne!(final_value(&project, "q[1]"), final_value(&project, "q[3]"));
    }

    // 2. ...so the wrap must spell it the way the equation does, and the
    //    fragments must compile. `q["1"]` and `q[1]` are the same position, so
    //    both spellings are checked.
    for (index_expr, expected) in [("\"1\"", "PREVIOUS(q[\"1\"])"), ("1", "PREVIOUS(q[1])")] {
        let project = indexed_axis_index_model(index_expr, "2");
        let (db, model, source_project) = char_fixture_db(&project);
        assert!(
            fragment_compile_failures(&db, model, source_project).is_empty(),
            "q[{index_expr}]: every link-score fragment must compile; a `d·1` \
             qualification does not resolve on an indexed axis and zeroes the score"
        );
        let text = link_score_text(
            &db,
            model,
            source_project,
            &["link_score\u{205A}source\u{2192}target"],
        );
        assert!(
            text.contains(expected),
            "q[{index_expr}]: expected {expected:?} in the partial; got: {text}"
        );
        assert!(
            !text.contains("d\u{B7}1"),
            "q[{index_expr}]: an indexed axis has no `dim·elem` spelling; got: {text}"
        );
    }
}

/// `DimName.N` on an indexed axis is unreachable, pinned rather than handled.
///
/// `compiler::subscript` and `compiler::context` both accept `D.2` as a static
/// position, and `dimensions::resolve_axis_index_name` deliberately does not --
/// a disclosed conservative gap. It stays a gap because the shape does not
/// compile in the first place: the TARGET's own fragment fails, so no link
/// score for it is ever asked for. If this test starts failing, the gap has
/// become reachable and the resolver needs the `DimName.N` form.
#[test]
fn dim_name_dot_index_does_not_compile_so_the_resolver_gap_is_inert() {
    use crate::db::compile_project_incremental;
    let project = indexed_axis_index_model("D.2", "2");
    let db = SimlinDb::default();
    let sp = sync_from_datamodel(&db, &project).project;
    assert!(
        compile_project_incremental(&db, sp, "main").is_err(),
        "`q[D.2]` is expected NOT to compile; if it now does, \
         `resolve_axis_index_name` must learn the `DimName.N` static position \
         or the wrap will freeze it and read the wrong row"
    );
}
