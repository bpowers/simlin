// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Imperative Shell
//
// End-to-end tests for Phase 3 macro compilation: a `.mdl` string is parsed
// (`open_vensim`, the public equivalent of the in-crate `convert_mdl`),
// synced into a salsa DB, and compiled/run via the production incremental
// path. These exercise the full registry-build -> classification ->
// BuiltinVisitor-expansion -> module-instantiation -> VM pipeline, so they
// are an Imperative Shell (real compile + VM I/O), not a pure core. The
// arithmetic each macro performs is kept trivial so expected values are
// hand-verifiable and documented inline.
//
// MDL note: a `NAME(arg)` call with *exactly one* argument is rewritten by
// the MDL converter to `LOOKUP(NAME, arg)` (the lookup-invocation
// heuristic, `mdl/xmile_compat.rs`), so a single-arg call never reaches the
// macro resolver as a function call. Every macro here therefore takes *two*
// parameters, matching the bundled `macro_*` fixtures (all `(input,
// parameter)`); this is a pre-existing MDL behavior, not a Phase 3 concern.

use crate::common::ErrorCode;
use crate::compat::open_vensim;
use crate::db::{
    DiagnosticError, DiagnosticSeverity, SimlinDb, collect_all_diagnostics,
    compile_project_incremental, sync_from_datamodel_incremental,
};
use crate::vm::Vm;

/// The fixed Vensim control + sketch tail every test `.mdl` shares. Keeps the
/// per-test source focused on the macro definitions and invocations.
const CONTROL_TAIL: &str = r#"
********************************************************
	.Control
********************************************************~
		Simulation Control Parameters
	|

FINAL TIME  = 2
	~	Month
	~	The final time for the simulation.
	|

INITIAL TIME  = 0
	~	Month
	~	The initial time for the simulation.
	|

SAVEPER  =
        TIME STEP
	~	Month [0,?]
	~	The frequency with which output is stored.
	|

TIME STEP  = 1
	~	Month [0,?]
	~	The time step for the simulation.
	|

\\\---/// Sketch information - do not modify anything except names
V300  Do not put anything below this section - it will be ignored
*View 1
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|72,72,100,0
///---\\\
:L<%^E!@
1:Current.vdf
"#;

/// Build the full `.mdl` source: the UTF-8 header, the test-specific body,
/// then the shared control/sketch tail.
fn mdl(body: &str) -> String {
    format!("{{UTF-8}}\n{body}\n{CONTROL_TAIL}")
}

/// Compile a `.mdl` source through the production incremental path and
/// return the compile `Result`.
fn compile_mdl(source: &str) -> crate::Result<crate::vm::CompiledSimulation> {
    let project = open_vensim(source).expect("MDL must parse into a datamodel project");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    compile_project_incremental(&db, sync.project, "main")
}

/// Run a `.mdl` source end-to-end and return the named variable's series.
#[allow(dead_code)] // consumed by the Task 3 simulation tests appended later
fn run_mdl_var(source: &str, var: &str) -> Vec<f64> {
    let compiled = compile_mdl(source).unwrap_or_else(|e| {
        panic!("incremental compilation should succeed: {e:?}");
    });
    let mut vm = Vm::new(compiled).expect("VM creation should succeed");
    vm.run_to_end().expect("VM run should succeed");
    let results = vm.into_results();
    let collected = crate::test_common::collect_results(&results);
    collected
        .get(var)
        .unwrap_or_else(|| panic!("variable {var:?} not in results: {:?}", collected.keys()))
        .clone()
}

/// Collect all diagnostics for a `.mdl` source via the salsa diagnostic path.
fn diagnostics_for(source: &str) -> Vec<crate::db::Diagnostic> {
    let project = open_vensim(source).expect("MDL must parse into a datamodel project");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    collect_all_diagnostics(&db, &sync.to_sync_result())
}

/// True iff some Error-severity diagnostic carries a `Model` error with the
/// given code (registry-build errors are project-level `Model` errors).
fn has_model_error(diags: &[crate::db::Diagnostic], code: ErrorCode) -> bool {
    diags.iter().any(|d| {
        d.severity == DiagnosticSeverity::Error
            && matches!(&d.error, DiagnosticError::Model(e) if e.code == code)
    })
}

// ── macros.AC5.2: recursion cycle (end-to-end) ─────────────────────────────

/// A directly-recursive macro (its body calls itself) plus a `main`
/// invocation must fail the compile with a `CircularDependency` error whose
/// message names the macro -- the registry's cycle detection surfaced as a
/// compile failure, not silently expanded without termination.
#[test]
fn ac5_2_directly_recursive_macro_fails_compile_with_cycle() {
    let source = mdl(r#":MACRO: RECUR(a, b)
RECUR = RECUR(a, b) + 1
	~	a
	~	directly recursive
	|

:END OF MACRO:
y=
	RECUR(3, 4)
	~
	~		|
"#);

    let err = compile_mdl(&source).expect_err("a directly recursive macro must fail to compile");
    assert_eq!(
        err.code,
        ErrorCode::NotSimulatable,
        "the compile entry maps a project-level error to NotSimulatable",
    );
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("recursive macro") && details.to_lowercase().contains("recur"),
        "the surfaced cycle message must name the macro: {details:?}",
    );

    assert!(
        has_model_error(&diagnostics_for(&source), ErrorCode::CircularDependency),
        "a recursive macro must accumulate a CircularDependency diagnostic",
    );
}

/// A mutually-recursive macro pair (`A` calls `B`, `B` calls `A`) plus a
/// `main` invocation must also fail with a cycle-detection error.
#[test]
fn ac5_2_mutually_recursive_macros_fail_compile_with_cycle() {
    let source = mdl(r#":MACRO: A MACRO(a, b)
A MACRO = B MACRO(a, b) + 1
	~	a
	~	mutually recursive: A -> B
	|

:END OF MACRO:
:MACRO: B MACRO(a, b)
B MACRO = A MACRO(a, b) * 2
	~	a
	~	mutually recursive: B -> A
	|

:END OF MACRO:
y=
	A MACRO(5, 6)
	~
	~		|
"#);

    let err =
        compile_mdl(&source).expect_err("a mutually recursive macro pair must fail to compile");
    assert_eq!(err.code, ErrorCode::NotSimulatable);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("recursive macro"),
        "the surfaced cycle message must identify the recursion: {details:?}",
    );

    assert!(
        has_model_error(&diagnostics_for(&source), ErrorCode::CircularDependency),
        "a mutually recursive macro pair must accumulate a CircularDependency diagnostic",
    );
}

// ── macros.AC5.3: duplicate macro name / macro-model collision ─────────────

/// Two `:MACRO:` blocks with the same name must fail the compile with a
/// duplicate-name error that names the macro.
#[test]
fn ac5_3_duplicate_macro_name_fails_compile() {
    let source = mdl(r#":MACRO: DUP(a, b)
DUP = a + b
	~	a
	~	first definition
	|

:END OF MACRO:
:MACRO: DUP(a, b)
DUP = a * b
	~	a
	~	duplicate definition
	|

:END OF MACRO:
out=
	DUP(4, 5)
	~
	~		|
"#);

    let err = compile_mdl(&source).expect_err("two macros of the same name must fail to compile");
    assert_eq!(err.code, ErrorCode::NotSimulatable);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.to_lowercase().contains("dup"),
        "the duplicate-macro error must name the macro: {details:?}",
    );

    assert!(
        has_model_error(&diagnostics_for(&source), ErrorCode::DuplicateMacroName),
        "a duplicate macro name must accumulate a DuplicateMacroName diagnostic",
    );
}

/// A macro named `main` collides with the implicit `main` model and must
/// fail the compile with a collision error naming `main`.
#[test]
fn ac5_3_macro_named_main_collides_with_main_model() {
    let source = mdl(r#":MACRO: MAIN(a, b)
MAIN = a + b
	~	a
	~	collides with the main model name
	|

:END OF MACRO:
out=
	MAIN(7, 8)
	~
	~		|
"#);

    let err = compile_mdl(&source)
        .expect_err("a macro named `main` must collide with the main model and fail");
    assert_eq!(err.code, ErrorCode::NotSimulatable);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.to_lowercase().contains("main"),
        "the collision error must name the collision: {details:?}",
    );

    assert!(
        has_model_error(&diagnostics_for(&source), ErrorCode::DuplicateMacroName),
        "a macro/model name collision must accumulate a DuplicateMacroName diagnostic",
    );
}
