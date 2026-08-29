// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Regression tests for the module-reference cycle guard.
//!
//! A cyclic or self-referential module graph makes the recursive
//! `compute_layout` / `model_shape` salsa queries loop, which salsa turns
//! into an unrecoverable dependency-graph panic. The empty-`model_name` sibling
//! of this class was fixed in c1c4c954; this is the reachable cousin tracked as
//! GH #806. Every production entry point -- compile, diagnostic collection, and
//! analysis -- must surface the cycle as a clean `CircularDependency` error
//! instead of aborting (a WASM panic plus the recursive-mutex cascade).
//!
//! The gate that does that -- `db::project_module_graph` -- records only EXPLICIT
//! module edges, deliberately, so it stays parse-free. The section at the bottom
//! of this file covers the one shape that could close a cycle through an IMPLICIT
//! (macro-call) edge and so evade the gate entirely. It is fixed by REJECTION
//! rather than by a wider gate (`macros.AC5.7` /
//! `ErrorCode::MacroContainsModule`), which is why those tests assert a rejection
//! and assert that the gate is still blind.

use crate::analysis::analyze_model;
use crate::common::ErrorCode;
use crate::datamodel::{self, Equation, Variable};
use crate::db::{
    DiagnosticCategory, SimlinDb, collect_all_diagnostics, compile_project_incremental,
    sync_from_datamodel,
};
use crate::test_common::TestProject;

fn module_var(ident: &str, target_model: &str) -> Variable {
    Variable::Module(datamodel::Module {
        ident: ident.to_string(),
        model_name: target_model.to_string(),
        documentation: String::new(),
        units: None,
        references: vec![],
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

fn aux_var(ident: &str, equation: &str) -> Variable {
    Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    })
}

fn model(name: &str, variables: Vec<Variable>) -> datamodel::Model {
    datamodel::Model {
        name: name.to_string(),
        sim_specs: None,
        variables,
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }
}

fn has_circular_diagnostic(diags: &[crate::db::Diagnostic]) -> bool {
    diags
        .iter()
        .any(|d| d.is(DiagnosticCategory::Model, ErrorCode::CircularDependency))
}

/// A module that instantiates its own enclosing model: `main` contains a module
/// whose `model_name` is `main`.
#[test]
fn self_referential_module_errors_without_panicking() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models[0].variables.push(module_var("m", "main"));
    project.models[0].variables.push(aux_var("x", "1"));

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    // Compile must reject cleanly rather than panic.
    assert!(
        compile_project_incremental(&db, sp, "main").is_err(),
        "a self-referential module must not compile"
    );

    // Diagnostic collection must surface the cycle, not panic.
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        has_circular_diagnostic(&diags),
        "expected a CircularDependency diagnostic, got {diags:?}"
    );

    // Analysis (the MCP read_model path) must degrade to an analysis_error.
    let mut db = db;
    let analysis = analyze_model(&project, &mut db, sp, "main", None)
        .expect("analyze_model must not panic on a module cycle");
    assert!(
        analysis.analysis_error.is_some(),
        "expected an analysis_error for a cyclic module graph"
    );
}

/// Two models that instantiate each other: `a` contains a module targeting `b`
/// and `b` contains a module targeting `a`.
#[test]
fn mutually_recursive_modules_error_without_panicking() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("a", vec![module_var("to_b", "b"), aux_var("x", "1")]),
        // Model `b` also carries a variable-level cycle, so this pins that the
        // module-cycle gate runs BEFORE the dependency graph: the dependency
        // walk over `b` must never start (it would recurse through the module
        // map into salsa's cycle panic).
        model(
            "b",
            vec![
                module_var("to_a", "a"),
                aux_var("y", "1"),
                aux_var("p", "q + 1"),
                aux_var("q", "p + 1"),
            ],
        ),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    assert!(
        compile_project_incremental(&db, sp, "a").is_err(),
        "mutually recursive modules must not compile"
    );

    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        has_circular_diagnostic(&diags),
        "expected a CircularDependency diagnostic, got {diags:?}"
    );

    let mut db = db;
    let analysis = analyze_model(&project, &mut db, sp, "a", None)
        .expect("analyze_model must not panic on a module cycle");
    assert!(analysis.analysis_error.is_some());
}

/// The PER-MODEL diagnostic collector needs the same cycle gate the
/// whole-project one has. `collect_all_diagnostics` consults
/// `project_module_graph` before driving each model's passes;
/// `collect_model_diagnostics` -- equally `pub`, and the shape a future caller
/// would reach for to diagnose one model -- did not, so on a cyclic project it
/// drove `model_all_diagnostics` straight into the recursive `compute_layout`
/// query (through `model_shape`) and salsa raised an unrecoverable dependency-graph cycle panic. Milder
/// than a stack overflow (this one unwinds, so a non-`panic=abort` host could
/// catch it) but the same class: the caller-side gate was the only thing
/// holding it, and only on one of the two callers.
#[test]
fn per_model_diagnostics_report_a_cycle_instead_of_panicking() {
    use crate::db::collect_model_diagnostics;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("a", vec![module_var("to_b", "b"), aux_var("x", "1")]),
        model("b", vec![module_var("to_a", "a"), aux_var("y", "1")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_model_diagnostics(&db, sync.models["a"].source, sync.project);
    assert!(
        has_circular_diagnostic(&diags),
        "expected a CircularDependency diagnostic, got {diags:?}"
    );
}

/// The gate must not swallow a valid model's diagnostics just because some
/// OTHER model in the project is cyclic -- the same reachability scoping
/// `collect_all_diagnostics` uses. `main` here reaches no cycle, so its own
/// passes still run and its own equation error is still reported.
#[test]
fn per_model_diagnostics_survive_an_unrelated_draft_cycle() {
    use crate::db::collect_model_diagnostics;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("main", vec![aux_var("x", "undefined_thing")]),
        model("a", vec![module_var("to_b", "b")]),
        model("b", vec![module_var("to_a", "a")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let diags = collect_model_diagnostics(&db, sync.models["main"].source, sync.project);
    assert!(
        !has_circular_diagnostic(&diags),
        "a model that reaches no cycle must not be reported as cyclic: {diags:?}"
    );
    assert!(
        !diags.is_empty(),
        "main's own diagnostics must still be collected despite the unrelated \
         draft cycle, got: {diags:?}"
    );
}

/// The cycle diagnostic must carry the model's DISPLAY name, not its canonical
/// map key -- and that is load-bearing, not cosmetic.
///
/// `simlin-mcp-core`'s `read_model` (and `edit_model`) filter the collected
/// diagnostics with `e.model_name.as_ref().is_none_or(|name| name ==
/// model_name)`, where `model_name` comes from `resolve_model_name` and is the
/// DATAMODEL spelling (`&m.name`). So a diagnostic reporting a model as
/// `sub_a` when the project spells it `Sub A` does not match and is dropped
/// from what the MCP tool returns -- silently, since the filter has no
/// fallback. Every other diagnostic in the crate already used
/// `model.name(db)`; the module-cycle gate was the sole outlier, so a
/// `CircularDependency` on a non-canonically-named model was the one error that
/// vanished on the way out.
///
/// Every OTHER fixture in this file uses names that are already canonical
/// (`a`, `b`, `main`), where the two spellings coincide and nothing can
/// detect a regression. This one does not, so "tidying" the gate back to the
/// canonical key reds here.
#[test]
fn cycle_diagnostic_carries_the_display_name_not_the_canonical_key() {
    use crate::db::collect_model_diagnostics;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model(
            "Sub A",
            vec![module_var("to_b", "Sub B"), aux_var("x", "1")],
        ),
        model(
            "Sub B",
            vec![module_var("to_a", "Sub A"), aux_var("y", "1")],
        ),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let circular_models = |diags: &[crate::db::Diagnostic]| -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.is(DiagnosticCategory::Model, ErrorCode::CircularDependency))
            .map(|d| d.model.clone())
            .collect()
    };

    // The per-model entry point (the one that gained the gate).
    let per_model = collect_model_diagnostics(&db, sync.models["sub_a"].source, sync.project);
    assert_eq!(
        circular_models(&per_model),
        vec!["Sub A".to_string()],
        "the per-model collector must report the display spelling"
    );

    // ...and the whole-project one, which reaches the same helper by delegating.
    let all = collect_all_diagnostics(&db, sync.project);
    let mut names = circular_models(&all);
    names.sort();
    assert_eq!(
        names,
        vec!["Sub A".to_string(), "Sub B".to_string()],
        "the whole-project collector must report display spellings too"
    );
}

/// An unused draft cycle (`a <-> b`) that the requested `main` model does not
/// reach must NOT block compiling or analyzing `main` -- the recursive queries
/// only loop for modules under the requested root. The cycle is still surfaced
/// as a diagnostic for the affected models. Regression for the Codex review
/// finding on PR #807 (the project-wide guard over-rejected valid roots).
///
/// `main` carries a dimensional problem of its OWN so that the "does not
/// block" half is actually pinned. With a clean `main` this test could not
/// tell "main's passes ran and found nothing" apart from "main's passes were
/// skipped" -- the exact failure a project-wide reject would produce, and the
/// one the gate's reachability scoping exists to prevent. A unit mismatch is
/// the right canary: it is a Warning, so it flows through `check_model_units`
/// (a pass the gate would skip) without disturbing the compile assertion
/// above.
#[test]
fn unused_draft_cycle_does_not_block_valid_main() {
    use crate::testutils::x_aux;

    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model(
            "main",
            vec![
                x_aux("x", "1", Some("widget")),
                // widget + time: a genuine dimensional mismatch, reported as a
                // Warning by `check_model_units`.
                aux_var("bad", "x + TIME"),
            ],
        ),
        model("a", vec![module_var("to_b", "b")]),
        model("b", vec![module_var("to_a", "a")]),
    ];

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    // `main` cannot reach the a<->b cycle, so it compiles cleanly.
    assert!(
        compile_project_incremental(&db, sp, "main").is_ok(),
        "a valid main must compile despite an unrelated draft cycle"
    );

    // Diagnostics still surface the draft cycle (for the affected models)...
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        has_circular_diagnostic(&diags),
        "the unrelated draft cycle should still be reported: {diags:?}"
    );
    // ...and main's own passes still ran, so its own problem is still reported.
    assert!(
        diags.iter().any(|d| {
            d.model == "main" && d.is(DiagnosticCategory::UnitInference, ErrorCode::UnitMismatch)
        }),
        "a valid model's own diagnostics must not be hidden by an unrelated \
         draft cycle: {diags:?}"
    );

    // Analyzing main succeeds (no analysis_error); analyzing a cyclic model degrades.
    let main_analysis =
        analyze_model(&project, &mut db, sp, "main", None).expect("analyze_model must not panic");
    assert!(
        main_analysis.analysis_error.is_none(),
        "analyzing a valid main must not error on an unrelated draft cycle"
    );
    let a_analysis =
        analyze_model(&project, &mut db, sp, "a", None).expect("analyze_model must not panic");
    assert!(
        a_analysis.analysis_error.is_some(),
        "analyzing a model that reaches a cycle must surface an analysis_error"
    );
}

// ── the macro-module cycle hole (macros.AC5.7) ──────────────────────────────

/// The minimal project that used to abort: a macro-marked model holding an
/// explicit module whose target CALLS that macro.
///
/// The cycle is `mac ->(explicit module `u_hop`)-> u ->(macro call `mac(input)`)->
/// mac`. Its closing edge is the macro call, which `project_module_graph` does
/// not record (it reads variable KINDS off the salsa inputs, never a parse
/// result), so `cycle_error_from` reported no cycle from any root and every
/// entry point drove the recursive `compute_layout` straight into salsa's
/// dependency-graph cycle panic -- an abort under `panic=abort`.
///
/// Every part of this fixture was ablated; only these three switches carry the
/// bug, and all three are necessary:
///
///   - `mac` carrying a `macro_spec` (as a plain model, `mac(input)` is just an
///     `UnknownBuiltin` and no implicit edge exists);
///   - `mac` holding the explicit module (without it there is no `mac -> u`
///     edge);
///   - `u`'s equation calling `mac(...)` (without it there is no back edge).
///
/// Three things the original reproducer carried are DECORATION, measured, and
/// dropped here so the regression pins the bug rather than the fixture: a
/// conventional `main` model instantiating `u`; a `+ u_hop·out` term in
/// `scaled` (added to keep the module from being "dead" -- the module VARIABLE
/// is the edge, so reading its output is not needed); and even the `p1` port
/// aux, which the panic does not need. The port aux is nonetheless KEPT: a
/// macro whose declared parameter has no body variable is a malformed fixture,
/// and what this pins is that a WELL-FORMED macro holding a module is rejected.
fn macro_holding_a_module_project() -> datamodel::Project {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_module_named, x_project};

    let mac = datamodel::Model {
        name: "mac".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("scaled", "p1 * 2", None),
            x_aux("p1", "0", None),
            x_module_named("u_hop", "u", &[("p1", "u_hop.input")], None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(datamodel::MacroSpec {
            parameters: vec!["p1".to_string()],
            primary_output: "scaled".to_string(),
            additional_outputs: vec![],
        }),
    };
    let u = x_model(
        "u",
        vec![x_aux("input", "0", None), x_aux("out", "mac(input)", None)],
    );
    x_project(sim_specs_with_units("month"), &[u, mac])
}

/// All three production entry points must REJECT [`macro_holding_a_module_project`]
/// rather than abort on it.
///
/// Before the `MacroRegistry::build` rejection, all three panicked with salsa's
/// `dependency graph cycle` -- but NOT all in the same query, and the difference
/// is worth recording because it is what someone debugging a reopening would
/// need. Measured by disabling Pass 4 and driving each entry point separately:
///
///   - `collect_all_diagnostics` -> **`compute_layout`** (the fragment path
///     reaches it through `model_shape`: `model_all_diagnostics ->
///     compile_var_fragment -> model_shape -> compute_layout -> ...`);
///   - `compile_project_incremental` -> **`compute_layout`**
///     (`assemble_simulation -> assemble_module -> compute_layout ->
///     compute_layout -> variable_size -> variable_dimensions -> ...`);
///   - `analyze_model` -> **`compute_layout`** (same prefix, reaching
///     `variable_size` via `model_ltm_mode`).
///
/// So every entry point reaches the same recursive cross-model query
/// (`compute_layout`, directly or through `model_shape`); naming the query is
/// what sends a reader debugging a reopening to the right place.
///
/// Because the panic aborted the test, the return values were never observed --
/// so this test asserts what the entry points do NOW and records the panic as
/// the pre-fix behavior, rather than pretending the old return values were known.
///
/// The fix is a rejection, NOT a widening of the module graph, so the gate is
/// deliberately still blind here: the first assertion pins that. Widening
/// `project_module_graph` to parse-derived edges would put every variable's
/// parse on every compile's dependency list, which is why the shape is deleted
/// instead (see that query's rustdoc). A future change that intentionally
/// widens the gate has to come here and say so.
#[test]
fn macro_holding_a_module_is_rejected_instead_of_aborting() {
    let project = macro_holding_a_module_project();

    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    let graph = crate::db::project_module_graph(&db, sp);
    for name in ["mac", "u"] {
        assert_eq!(
            graph.cycle_error_from(name),
            None,
            "the module graph is still structural and does not see the macro-call \
             back edge; the rejection -- not this gate -- is what makes {name} safe",
        );
    }

    // Diagnostics: returns, and names the offending macro actionably.
    let diags = collect_all_diagnostics(&db, sp);
    let rejection = diags
        .iter()
        .find(|d| d.is(DiagnosticCategory::Model, ErrorCode::MacroContainsModule))
        .unwrap_or_else(|| panic!("expected a MacroContainsModule diagnostic, got {diags:?}"));
    let message = rejection.reason().unwrap_or_default();
    assert!(
        message.contains("mac") && message.contains("u_hop"),
        "the rejection must name the offending macro and its module variable so the \
         modeller can find it: {message:?}",
    );

    // Compile: a clean `Err`, not an abort.
    let err = compile_project_incremental(&db, sp, "u")
        .expect_err("a macro holding a module must not compile");
    assert!(
        err.get_details().unwrap_or_default().contains("mac"),
        "the compile error must name the offending macro: {err:?}",
    );

    // Analysis: degrades to an `analysis_error` carrying the SAME actionable
    // message. `analyze_model` never reads `build_error` itself -- the text
    // arrives through `run_ltm_pipeline`'s compile failure -- so this pins that
    // the rejection is not flattened into an opaque downstream error on the one
    // entry point that has no direct view of it.
    let analysis = analyze_model(&project, &mut db, sp, "u", None)
        .expect("analyze_model must not panic on a macro-module cycle");
    let analysis_error = analysis
        .analysis_error
        .expect("analyzing a model whose macro is rejected must surface an analysis_error");
    assert!(
        analysis_error.contains("mac") && analysis_error.contains("u_hop"),
        "the analysis error must carry the actionable rejection, not an opaque \
         downstream failure: {analysis_error:?}",
    );
}

/// `MacroRegistry::build` returning an EMPTY registry on failure is load-bearing
/// for CYCLE SAFETY, not merely for error quality -- and this is the observable
/// that proves it.
///
/// `collect_all_diagnostics` and `analyze_model` do NOT stop at the macro
/// build error the way `compile_project_incremental` does: they emit / ignore it
/// and then run each model's passes anyway. What keeps those two from walking
/// the cycle is that with an empty registry `u`'s `mac(input)` no longer resolves
/// as a macro call, so builtin expansion synthesizes no implicit module and the
/// `u -> mac` edge does not exist. The `UnknownBuiltin` on `u·out` below is that
/// non-existence, made visible.
///
/// A future "keep a partial registry alongside the error" refactor would resolve
/// the call again, restore the invisible edge, and reopen the abort -- on a
/// project with no explicit module inside a macro anywhere, since the rejected
/// macro would still be registered. This test reds if that happens, which is why
/// it asserts the cascade rather than filtering it out as noise.
#[test]
fn rejecting_a_macro_empties_the_registry_so_no_implicit_edge_is_synthesized() {
    let project = macro_holding_a_module_project();

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    let registry = &crate::db::macro_registry::project_macro_registry(&db, sync.project).registry;
    assert!(
        registry.resolve_macro("mac").is_none(),
        "a rejected macro set must yield an EMPTY registry, so no call is classified \
         module-backed and the implicit edge never exists",
    );

    let diags = collect_all_diagnostics(&db, sync.project);
    assert!(
        diags.iter().any(|d| {
            d.model == "u"
                && d.variable.as_deref() == Some("out")
                && d.is(DiagnosticCategory::Equation, ErrorCode::UnknownBuiltin)
        }),
        "with the registry emptied, `mac(input)` must fall through to UnknownBuiltin \
         -- the visible proof that no implicit module edge was synthesized: {diags:?}",
    );
}

/// The rejection must not catch a LEGITIMATE macro. A macro-marked model with no
/// module variable still builds, still resolves, and still expands into a
/// working module instance at its call site.
///
/// This is the negative control for the new pass, driven through the same salsa
/// pipeline as the rejection test above (rather than only `MacroRegistry::build`
/// in isolation) so it covers the whole registry -> expansion -> compile path.
#[test]
fn a_macro_without_a_module_still_builds_and_expands() {
    use crate::testutils::{sim_specs_with_units, x_aux, x_model, x_project};

    let mac = datamodel::Model {
        name: "mac".to_string(),
        sim_specs: None,
        variables: vec![x_aux("scaled", "p1 * 2", None), x_aux("p1", "0", None)],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(datamodel::MacroSpec {
            parameters: vec!["p1".to_string()],
            primary_output: "scaled".to_string(),
            additional_outputs: vec![],
        }),
    };
    let main = x_model(
        "main",
        vec![x_aux("input", "3", None), x_aux("out", "mac(input)", None)],
    );
    let project = x_project(sim_specs_with_units("month"), &[main, mac]);

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);

    assert!(
        crate::db::macro_registry::project_macro_registry(&db, sync.project)
            .build_error
            .is_none(),
        "a macro with no module variable must still build",
    );
    let diags = collect_all_diagnostics(&db, sync.project);
    assert!(
        diags.is_empty(),
        "a legitimate macro must produce no diagnostics: {diags:?}"
    );

    // ...and it actually expands: `out` reads the macro instance's `scaled`.
    let compiled = compile_project_incremental(&db, sync.project, "main")
        .expect("a legitimate macro must compile");
    let mut vm = crate::vm::Vm::new(compiled).expect("VM creation must succeed");
    vm.run_to_end().expect("VM run must succeed");
    let results = vm.into_results();
    let collected = crate::test_common::collect_results(&results);
    let out = collected
        .get("out")
        .unwrap_or_else(|| panic!("`out` missing from results: {:?}", collected.keys()));
    // The non-empty check is not redundant: `all()` over an empty series is
    // vacuously true, so without it a regression that produced no saved steps
    // would pass silently.
    assert!(!out.is_empty(), "`out` must have at least one saved step");
    assert!(
        out.iter().all(|v| *v == 6.0),
        "the expanded macro must compute 3 * 2 = 6 at every step, got {out:?}",
    );
}

/// A valid nested-module project (no cycle) must still compile and produce no
/// spurious cycle diagnostic -- the guard must not false-positive on legitimate
/// acyclic nesting.
#[test]
fn acyclic_nested_modules_compile_clean() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models = vec![
        model("main", vec![module_var("mid", "middle"), aux_var("x", "1")]),
        model(
            "middle",
            vec![module_var("leaf_mod", "leaf"), aux_var("y", "2")],
        ),
        model("leaf", vec![aux_var("z", "3")]),
    ];

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    assert!(
        compile_project_incremental(&db, sp, "main").is_ok(),
        "an acyclic nested-module project must compile"
    );
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        !has_circular_diagnostic(&diags),
        "acyclic nesting must not report a module cycle: {diags:?}"
    );
}

/// A module whose `model_name` is not in the project -- a reference to a
/// deleted model -- is a project a user can produce at any moment. Every
/// production entry point must refuse it cleanly rather than index a missing
/// model and abort, and the diagnostic pass must say WHY: assembly is the only
/// other place that notices, and it never runs on that pass, so without the
/// wiring check's `BadModelName` the modeller sees "does not compile" and no
/// explanation.
#[test]
fn module_targeting_a_missing_model_errors_without_panicking() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models[0].variables.push(aux_var("x", "1"));
    project.models[0]
        .variables
        .push(module_var("m", "nonexistent"));

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    assert!(
        compile_project_incremental(&db, sp, "main").is_err(),
        "a module targeting a missing model must not compile"
    );

    let diags = collect_all_diagnostics(&db, sp);
    let dangling: Vec<&crate::db::Diagnostic> = diags
        .iter()
        .filter(|d| d.is(DiagnosticCategory::Model, ErrorCode::BadModelName))
        .collect();
    assert_eq!(
        dangling.len(),
        1,
        "expected exactly one BadModelName diagnostic, got {diags:?}"
    );
    assert_eq!(dangling[0].severity, crate::db::DiagnosticSeverity::Error);
    assert_eq!(dangling[0].model, "main");
    assert_eq!(
        dangling[0].variable.as_deref(),
        Some("m"),
        "the diagnostic must name the module variable"
    );

    let mut db = db;
    let analysis = analyze_model(&project, &mut db, sp, "main", None)
        .expect("analyze_model must not panic on a missing module target");
    assert!(
        analysis.analysis_error.is_some(),
        "expected an analysis_error for a module targeting a missing model"
    );
}

/// The other arm of the dangling-target check: an EMPTY `model_name` is the
/// normal freshly-drawn state of a module, so it is not reported -- a modeller
/// who has not yet pointed the module at a model gets no Error on every
/// keystroke. The project still does not compile, exactly as for a dangling
/// name; only the diagnostic differs.
#[test]
fn module_with_an_empty_model_name_is_not_reported_as_dangling() {
    let mut project = TestProject::new("test").build_datamodel();
    project.models[0].variables.push(aux_var("x", "1"));
    project.models[0].variables.push(module_var("m", ""));

    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let sp = sync.project;

    assert!(
        compile_project_incremental(&db, sp, "main").is_err(),
        "a module with no target model must not compile"
    );
    let diags = collect_all_diagnostics(&db, sp);
    assert!(
        !diags
            .iter()
            .any(|d| d.is(DiagnosticCategory::Model, ErrorCode::BadModelName)),
        "an empty model_name is the freshly-drawn state, not a dangling reference: {diags:?}"
    );
}
