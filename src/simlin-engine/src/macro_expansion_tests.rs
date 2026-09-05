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
fn compile_mdl(source: &str) -> crate::Result<std::sync::Arc<crate::vm::CompiledSimulation>> {
    let project = open_vensim(source).expect("MDL must parse into a datamodel project");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    compile_project_incremental(&db, sync.project, "main")
}

/// Run a `.mdl` source end-to-end and return the named variable's series.
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
    collect_all_diagnostics(&db, sync.project)
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

// ── Task 3: BuiltinVisitor macro expansion ─────────────────────────────────

/// A non-macro scalar `datamodel::Aux` body helper for the structural tests.
fn mk_aux(ident: &str, equation: &str) -> crate::datamodel::Variable {
    crate::datamodel::Variable::Aux(crate::datamodel::Aux {
        ident: ident.to_string(),
        equation: crate::datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: crate::datamodel::Compat::default(),
    })
}

/// Build a single-macro registry for `MYMACRO(p1, p2)` with primary output
/// `mymacro` (a `p1 + p2` body).
fn mymacro_registry() -> crate::module_functions::MacroRegistry {
    let macro_model = crate::datamodel::Model {
        name: "mymacro".to_string(),
        sim_specs: None,
        variables: vec![
            mk_aux("mymacro", "p1 + p2"),
            mk_aux("p1", "0"),
            mk_aux("p2", "0"),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(crate::datamodel::MacroSpec {
            parameters: vec!["p1".to_string(), "p2".to_string()],
            primary_output: "mymacro".to_string(),
            additional_outputs: vec![],
        }),
    };
    crate::module_functions::MacroRegistry::build(&[macro_model])
        .expect("valid single-macro registry")
}

/// Structural: `y = MYMACRO(a, b)` expands into a synthetic
/// `Variable::Module` whose `model_name` is the macro's model, with one
/// `ModuleReference` per parameter (the `dst` ports are the macro's
/// `MacroSpec.parameters`), and the caller equation is replaced by a
/// reference to `<module>·<primary_output>`.
#[test]
fn macro_call_expands_to_synthetic_module_structurally() {
    use crate::ast::{Ast, Expr0};

    let registry = mymacro_registry();
    let ast = Ast::Scalar(
        Expr0::new("MYMACRO(a, b)", crate::lexer::LexerType::Equation)
            .expect("parse")
            .expect("non-empty"),
    );

    let (transformed, vars) = crate::builtins_visitor::instantiate_implicit_modules(
        "y",
        ast,
        None,
        crate::builtins_visitor::SnapshotIndexFacts::NoModel,
        &registry,
        None,
    )
    .expect("a macro call must expand");

    let modules: Vec<&crate::capture::ImplicitModule> =
        vars.iter().filter_map(|v| v.module()).collect();
    assert_eq!(
        modules.len(),
        1,
        "a single-output macro call synthesizes exactly one Module, got {} helpers",
        vars.len(),
    );
    let module = modules[0];
    assert_eq!(
        module.model_name, "mymacro",
        "the synthetic module must target the macro's model",
    );
    assert_eq!(
        module.references.len(),
        2,
        "one ModuleReference per macro parameter",
    );
    let dst_ports: Vec<String> = module
        .references
        .iter()
        .map(|r| r.dst.rsplit('.').next().unwrap().to_string())
        .collect();
    assert_eq!(
        dst_ports,
        vec!["p1".to_string(), "p2".to_string()],
        "the ModuleReference dst ports are the macro's parameter ports, in order",
    );

    let Ast::Scalar(Expr0::Var(replacement, _)) = &transformed else {
        panic!("the call expression must be replaced by a single Var, got {transformed:?}");
    };
    let expected = format!("{}\u{b7}mymacro", module.ident);
    assert_eq!(
        replacement.as_str(),
        expected,
        "the call must be replaced by <module>·<primary_output>",
    );
}

/// What an apply-to-all body asks of the expansion
/// (`builtins_visitor::per_element_requirements`), one row per arm of
/// `MacroRegistry::resolve_call` crossed with what the call lowers as, plus
/// the positions a call can sit in.
///
/// The rows ARE the routing enumeration: `Expand` (a macro instance),
/// `Passthrough` (a macro that lowers as the builtin it names, here the
/// snapshot intrinsic `INIT`), `RenamedBuiltinSelfCall` (the enclosing
/// macro's own renamed builtin, here the stdlib alias `DELAYN`) and
/// `Unresolved` (a stdlib call, a snapshot intrinsic, an ordinary builtin, no
/// call). A body's requirement is the maximum over its calls wherever they
/// sit -- an argument, a subscript index, a range bound -- so the last rows
/// nest one inside another.
#[test]
fn apply_to_all_requirements_follow_every_macro_call_resolution_arm() {
    use crate::ast::Expr0;
    use crate::builtins_visitor::{PerElement, per_element_requirements};

    // `mymacro` expands; `init` is a genuine passthrough of the renamed
    // builtin; `delayn` is a macro whose body calls the like-named stdlib
    // alias, which inside its own body is the builtin (GH #554).
    let macro_model = |name: &str, params: &[&str], body: &str| crate::datamodel::Model {
        name: name.to_string(),
        sim_specs: None,
        variables: std::iter::once(mk_aux(name, body))
            .chain(params.iter().map(|p| mk_aux(p, "0")))
            .collect(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(crate::datamodel::MacroSpec {
            parameters: params.iter().map(|p| p.to_string()).collect(),
            primary_output: name.to_string(),
            additional_outputs: vec![],
        }),
    };
    let registry = crate::module_functions::MacroRegistry::build(&[
        macro_model("mymacro", &["p1", "p2"], "p1 + p2"),
        macro_model("init", &["x"], "init(x)"),
        macro_model("delayn", &["x", "t", "n"], "delayn(x, t, n)"),
    ])
    .expect("valid registry");
    let parse = |s: &str| {
        Expr0::new(s, crate::lexer::LexerType::Equation)
            .expect("parse")
            .expect("non-empty")
    };

    // `(body, enclosing macro model, routing arm, requirement)`.
    let rows: &[(&str, Option<&str>, &str, PerElement)] = &[
        (
            "MYMACRO(x[Dim], k)",
            None,
            "Expand",
            PerElement::ModuleInstance,
        ),
        (
            "INIT(x)",
            None,
            "Passthrough: lowers as the snapshot intrinsic",
            PerElement::SnapshotOnly,
        ),
        (
            "DELAYN(x, 2, 3)",
            Some("delayn"),
            "RenamedBuiltinSelfCall: lowers as the stdlib alias",
            PerElement::ModuleInstance,
        ),
        (
            "SMTH1(x, 5)",
            None,
            "Unresolved: a stdlib call",
            PerElement::ModuleInstance,
        ),
        (
            "DELAY(x, 5)",
            None,
            "Unresolved: the DELAY alias",
            PerElement::ModuleInstance,
        ),
        (
            "PREVIOUS(x, 0)",
            None,
            "Unresolved: PREVIOUS",
            PerElement::SnapshotOnly,
        ),
        (
            "ABS(x) + MAX(a, b)",
            None,
            "Unresolved: ordinary builtins",
            PerElement::None,
        ),
        ("a + b", None, "no call", PerElement::None),
        (
            "ABS(PREVIOUS(x, 0)) + 1",
            None,
            "a snapshot nested in a builtin argument",
            PerElement::SnapshotOnly,
        ),
        (
            "PREVIOUS(SMTH1(x, 1), 0)",
            None,
            "the maximum: a module call nested in a snapshot argument",
            PerElement::ModuleInstance,
        ),
        (
            "vals[PREVIOUS(i, 1)]",
            None,
            "a snapshot in a subscript index",
            PerElement::SnapshotOnly,
        ),
        (
            "vals[1:SMTH1(n, 1)]",
            None,
            "a module call in a range bound",
            PerElement::ModuleInstance,
        ),
        (
            "IF a > 0 THEN INIT(x) ELSE SMTH1(x, 1)",
            None,
            "the maximum over the branches of an IF",
            PerElement::ModuleInstance,
        ),
    ];
    for (body, enclosing, arm, expected) in rows {
        assert_eq!(
            per_element_requirements(&parse(body), &registry, *enclosing),
            *expected,
            "{arm}: `{body}`"
        );
    }
}

/// macros.AC2.1 smoke: a trivial single-output macro `M(a, b) = a * b`
/// invoked `y = M(5, 1.1)` compiles, runs, and yields `y == 5.5` at every
/// step -- the full expand -> register -> compile -> VM path for macros.
#[test]
fn ac2_1_single_output_macro_smoke() {
    let source = mdl(r#":MACRO: M(a, b)
M = a * b
	~	a
	~	trivial product macro
	|

:END OF MACRO:
y=
	M(5, 1.1)
	~
	~		|
"#);

    let y = run_mdl_var(&source, "y");
    assert!(!y.is_empty(), "expected at least one output step");
    for (i, v) in y.iter().enumerate() {
        assert!(
            (v - 5.5).abs() < 1e-9,
            "y at step {i} expected 5.5 (= 5 * 1.1), got {v}",
        );
    }
}

/// macros.AC5.4: a macro shadowing the `SSHAPE` builtin resolves to the
/// macro (not the builtin). `:MACRO: SSHAPE(x, p)` / `SSHAPE = x + p`,
/// invoked `y = SSHAPE(3, 4)` => `y == 7` (the macro's definition; the
/// `SSHAPE` builtin would compute something else entirely).
#[test]
fn ac5_4_macro_shadows_sshape_builtin() {
    let source = mdl(r#":MACRO: SSHAPE(x, p)
SSHAPE = x + p
	~	x
	~	shadows the SSHAPE builtin
	|

:END OF MACRO:
y=
	SSHAPE(3, 4)
	~
	~		|
"#);

    let y = run_mdl_var(&source, "y");
    for (i, v) in y.iter().enumerate() {
        assert!(
            (v - 7.0).abs() < 1e-9,
            "y at step {i} expected 7 (macro SSHAPE = 3 + 4), got {v} -- \
             the SSHAPE *builtin* must not have been invoked",
        );
    }
}

/// macros.AC5.4 (second builtin-shadow case): a macro named `RAMP FROM TO`
/// shadows the same-named builtin. `RAMP FROM TO(a, b) = a + b`, invoked
/// `y = RAMP FROM TO(2, 9)` => `y == 11`.
#[test]
fn ac5_4_macro_shadows_ramp_from_to_builtin() {
    let source = mdl(r#":MACRO: RAMP FROM TO(a, b)
RAMP FROM TO = a + b
	~	a
	~	shadows the RAMP FROM TO builtin
	|

:END OF MACRO:
y=
	RAMP FROM TO(2, 9)
	~
	~		|
"#);

    let y = run_mdl_var(&source, "y");
    for (i, v) in y.iter().enumerate() {
        assert!(
            (v - 11.0).abs() < 1e-9,
            "y at step {i} expected 11 (macro RAMP FROM TO = 2 + 9), got {v}",
        );
    }
}

/// clearn-residual.AC2.1/AC2.2/AC2.4: a 5-parameter `RAMP FROM TO` macro that
/// branches on its `islinear` selector must run the branch the caller selects.
/// This is the discriminating test the existing fixtures lack: the macro's
/// `exp` branch (here `xfrom + 2*RAMP(...)`, deliberately *double* slope so it
/// is provably distinct from the linear branch mid-ramp) cannot be reproduced
/// by the import-time linear rewrite, so with the buggy formatter all three
/// invocations collapse to the linear value and `y_exp` fails.
///
/// Harness control (`CONTROL_TAIL`): INITIAL TIME = 0, FINAL TIME = 2,
/// TIME STEP = SAVEPER = 1, so the saved steps are Time = {0, 1, 2}. The ramp
/// window `tstart = 0 .. tend = 2` puts the single interior saved step at
/// Time = 1, where `RAMP(slope, 0, 2) == slope * (1 - 0) == slope`.
///
/// Expected values at Time = 1 (series index 1):
/// - `y_exp = RAMP FROM TO(10, 110, 0, 2, 0)`: slope = (110-10)/(2-0) = 50;
///   both endpoints positive so `linear = islinear = 0` -> exp branch:
///   `10 + 2*50 = 110` (linear branch would be `10 + 50 = 60`).
/// - `y_lin = RAMP FROM TO(10, 110, 0, 2, 1)`: `linear = 1` -> linear branch:
///   `10 + 50 = 60`.
/// - `y_force = RAMP FROM TO(-10, 110, 0, 2, 0)`: slope = (110-(-10))/(2-0) = 60;
///   `xfrom <= 0` forces `linear = 1` despite `islinear = 0` -> linear branch:
///   `-10 + 60 = 50` (exp branch would be `-10 + 2*60 = 110`).
#[test]
fn macro_ramp_from_to_runs_selected_branch() {
    let source = mdl(r#":MACRO: RAMP FROM TO(xfrom, xto, tstart, tend, islinear)
RAMP FROM TO = IF THEN ELSE(linear = 1, linear ramp, exp ramp)
	~	dmnl
	~	|
linear = IF THEN ELSE(xfrom > 0 :AND: xto > 0, islinear, 1)
	~	dmnl
	~	|
slope = (xto - xfrom) / (tend - tstart)
	~	dmnl
	~	|
linear ramp = xfrom + RAMP(slope, tstart, tend)
	~	dmnl
	~	|
exp ramp = xfrom + 2 * RAMP(slope, tstart, tend)
	~	dmnl
	~	|
:END OF MACRO:
y_exp=
	RAMP FROM TO(10, 110, 0, 2, 0)
	~
	~		|

y_lin=
	RAMP FROM TO(10, 110, 0, 2, 1)
	~
	~		|

y_force=
	RAMP FROM TO(-10, 110, 0, 2, 0)
	~
	~		|
"#);

    // Time = 1 is series index 1 (saved steps Time = 0, 1, 2).
    const MID: usize = 1;

    let y_exp = run_mdl_var(&source, "y_exp");
    let y_lin = run_mdl_var(&source, "y_lin");
    let y_force = run_mdl_var(&source, "y_force");

    // AC2.1: islinear = 0 with positive endpoints runs the exp branch (110),
    // which is provably distinct from the linear branch (60). The import-time
    // linearizer cannot produce 110, so this assertion is the true RED.
    assert!(
        (y_exp[MID] - 110.0).abs() < 1e-9,
        "y_exp at Time=1 expected 110 (exp branch: xfrom + 2*slope), got {}",
        y_exp[MID]
    );
    assert!(
        (y_exp[MID] - 60.0).abs() > 1e-6,
        "y_exp at Time=1 must NOT equal the linear value 60; got {} -- the \
         macro's exp branch did not run (formatter linearized the call)",
        y_exp[MID]
    );

    // AC2.2: islinear = 1 runs the linear branch (60), the no-regression value.
    assert!(
        (y_lin[MID] - 60.0).abs() < 1e-9,
        "y_lin at Time=1 expected 60 (linear branch: xfrom + slope), got {}",
        y_lin[MID]
    );

    // AC2.4: nonpositive endpoint forces linear = 1 despite islinear = 0, so
    // the linear branch (50) runs, not the exp branch (110).
    assert!(
        (y_force[MID] - 50.0).abs() < 1e-9,
        "y_force at Time=1 expected 50 (forced-linear branch: xfrom + slope), got {}",
        y_force[MID]
    );
    assert!(
        (y_force[MID] - 110.0).abs() > 1e-6,
        "y_force at Time=1 must NOT equal the exp value 110; got {} -- the \
         forced-linear selector did not take effect",
        y_force[MID]
    );
}

/// macros.AC5.6: a call to a name that is neither a macro, a stdlib
/// function, nor a builtin must fail the compile with `UnknownBuiltin`.
#[test]
fn ac5_6_unknown_function_name_is_unknown_builtin() {
    let source = mdl(r#"x=
	1
	~
	~		|

y=
	NOTAFUNCTION(x, 2)
	~
	~		|
"#);

    let err = compile_mdl(&source).expect_err("an unknown function name must fail the compile");
    assert_eq!(err.code, ErrorCode::NotSimulatable);

    let diags = diagnostics_for(&source);
    assert!(
        diags.iter().any(|d| {
            d.severity == DiagnosticSeverity::Error
                && matches!(
                    &d.error,
                    DiagnosticError::Equation(e) if e.code == ErrorCode::UnknownBuiltin
                )
        }),
        "an unknown call name must produce an UnknownBuiltin diagnostic, got: {diags:?}",
    );
}

/// macros.AC5.1: a 2-parameter macro invoked with too many arguments (3,
/// then 4) fails the compile with `BadBuiltinArgs`, and the diagnostic's
/// equation span covers the macro call so the macro is identifiable in
/// context. (A 1-arg under-supply cannot be expressed: `M(1)` is rewritten
/// to `LOOKUP(M, 1)` by the MDL converter before reaching the resolver --
/// the under-supply arity path is covered by the focused descriptor unit
/// test in `builtins_visitor.rs`.)
#[test]
fn ac5_1_macro_arity_mismatch_is_bad_builtin_args() {
    for call_args in ["1, 2, 3", "1, 2, 3, 4"] {
        let source = mdl(&format!(
            r#":MACRO: M(a, b)
M = a + b
	~	a
	~	two-parameter macro
	|

:END OF MACRO:
y=
	M({call_args})
	~
	~		|
"#
        ));

        let err = compile_mdl(&source).expect_err(&format!(
            "M called with `{call_args}` must fail (wrong arity)"
        ));
        assert_eq!(err.code, ErrorCode::NotSimulatable);

        let diags = diagnostics_for(&source);
        let arity_diag = diags
            .iter()
            .find(|d| {
                d.severity == DiagnosticSeverity::Error
                    && matches!(
                        &d.error,
                        DiagnosticError::Equation(e) if e.code == ErrorCode::BadBuiltinArgs
                    )
            })
            .unwrap_or_else(|| {
                panic!(
                    "M called with `{call_args}` must produce a BadBuiltinArgs diagnostic, got: {diags:?}"
                )
            });
        if let DiagnosticError::Equation(e) = &arity_diag.error {
            assert!(
                e.end > e.start,
                "the arity error span must cover the macro call (start={}, end={})",
                e.start,
                e.end,
            );
        }
    }
}

// ── #554: macro wrapping a same-canonical-name opcode intrinsic ────────────
//
// The MDL importer MUST rename the Vensim `INITIAL` builtin to `INIT`
// (`mdl/xmile_compat.rs`; the engine's `Expr1` lowering recognizes only the
// opcode name `init`, not `initial`). C-LEARN's uninvoked
// `:MACRO: INIT(x) ... INIT = INITIAL(x)` therefore stores the datamodel
// body `init = init(x)`. Pre-fix, the macro recursion check mistook that
// renamed-intrinsic call for a recursive `init -> init` macro edge and failed
// the WHOLE `MacroRegistry::build`; the empty registry then un-shadowed the
// project's OTHER macros, so their (correct) calls fell through to the
// builtins and failed with `BadBuiltinArgs`/`UnknownBuiltin` -- a single
// false positive blocking all macro expansion.
//
// These end-to-end tests reproduce the pattern on a tiny inline `.mdl`
// (C-LEARN itself is 1.4 MB; its full-corpus guard is the `#[ignore]`d
// `corpus_clearn_macros_import` in `tests/integration/simulate.rs`). The macros take two
// params so the invocation is not rewritten to `LOOKUP` (the unrelated #553
// 1-arg-call heuristic) -- the `INITIAL(x)`->`INIT(x)` rename inside the body
// (the #554 trigger) is independent of the invocation's arity.

/// GH #554's model, through the production MDL import: C-LEARN's uninvoked
/// `:MACRO: INIT(x) ... INIT = INITIAL(x)`, whose body the importer's necessary
/// `INITIAL -> INIT` rename turns into the self-call `init = init(x)`.
///
/// The registry must build with no `init -> init` cycle (the false recursion
/// that emptied the registry and un-shadowed every other macro), the sibling
/// macro must still expand, and the two routes a call to `init` can take must
/// both land on the builtin: the body's own call is the enclosing macro's
/// renamed builtin (`MacroCallResolution::RenamedBuiltinSelfCall`, a direct
/// slot read of the port, so no helper at all), and `INITIAL(k * 2)` in `main`
/// is the passthrough at an external call site
/// (`MacroCallResolution::Passthrough`), which captures its computed argument
/// exactly as the bare builtin does.
#[test]
fn issue_554_model_imports_registers_and_routes_both_init_calls_to_the_builtin() {
    use crate::capture::{CaptureKind, ImplicitVar};
    use crate::db::sync_from_datamodel;
    use crate::test_common::implicit_vars_of;

    let source = mdl(r#":MACRO: INIT(x)
INIT = INITIAL(x)
	~	dmnl
	~	C-LEARN's uninvoked macro: the body's INITIAL is renamed to INIT
	|

:END OF MACRO:
:MACRO: SSHAPE(a, b)
SSHAPE = a * b
	~	dmnl
	~	sibling macro, shadowing the 3-arg SSHAPE builtin
	|

:END OF MACRO:
k = 3
	~	dmnl
	~	|
sibling = SSHAPE(4, 5)
	~	dmnl
	~	|
frozen = INITIAL(k * 2)
	~	dmnl
	~	|
"#);

    let diags = diagnostics_for(&source);
    assert!(
        !has_model_error(&diags, ErrorCode::CircularDependency),
        "the body's renamed INITIAL is not a recursive macro call; diagnostics: {diags:?}"
    );
    let registry = crate::module_functions::MacroRegistry::build(
        &open_vensim(&source)
            .expect("the issue's model imports")
            .models,
    )
    .expect("the registry builds: no false init -> init cycle");
    assert!(
        registry
            .resolve_macro("init")
            .expect("the uninvoked macro is registered")
            .passthrough,
        "the importer's `init = init(x)` body is a genuine passthrough"
    );

    let sibling = run_mdl_var(&source, "sibling");
    assert!(
        sibling.iter().all(|&v| (v - 20.0).abs() < 1e-9),
        "SSHAPE(4, 5) = 20: the sibling macro still shadows the builtin: {sibling:?}"
    );
    let frozen = run_mdl_var(&source, "frozen");
    assert!(
        frozen.iter().all(|&v| (v - 6.0).abs() < 1e-9),
        "INITIAL(k * 2) = 6 at every step: {frozen:?}"
    );

    let project = open_vensim(&source).expect("the issue's model imports");
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let frozen_helpers = implicit_vars_of(&db, &sync, "main", "frozen");
    assert!(
        matches!(
            frozen_helpers.as_slice(),
            [ImplicitVar::Capture(c)]
                if c.ident() == "$⁚frozen⁚0⁚arg0" && c.kind() == CaptureKind::Init
        ),
        "the external call lowers as the INIT builtin and captures its computed argument"
    );
    let body_helpers = implicit_vars_of(&db, &sync, "init", "init");
    assert!(
        body_helpers.is_empty(),
        "the body's INIT(x) lowers as the builtin reading the port `x`'s own slot -- \
         never as an instance of itself, and with no capture of the port; got {:?}",
        body_helpers
            .iter()
            .map(|v| v.ident().to_string())
            .collect::<Vec<_>>()
    );
}

/// A passthrough macro keeps the arity the model declared, even though a
/// valid call lowers as the builtin. `PREVIOUS(x)` declares one parameter;
/// the builtin it lowers to also accepts a fallback, so without the check
/// `PREVIOUS(input, 0)` would compile as the builtin behind a macro that says
/// otherwise.
///
/// XMILE rather than MDL: unary `PREVIOUS` is engine/XMILE syntax, not a
/// Vensim builtin, so the MDL converter's one-argument heuristic would import
/// `PREVIOUS(x)` as `LOOKUP(previous, x)`.
#[test]
fn a_passthrough_macro_keeps_its_declared_arity_at_an_external_call_site() {
    let project_for = |call_args: &str| {
        let source = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><vendor>Simlin</vendor><product>arity</product><name>main</name></header>
  <sim_specs><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <model name="main"><variables>
    <aux name="input"><eqn>10</eqn></aux>
    <aux name="out"><eqn>PREVIOUS({call_args})</eqn></aux>
  </variables></model>
  <macro name="PREVIOUS">
    <parm>x</parm>
    <eqn>PREVIOUS(x)</eqn>
  </macro>
</xmile>"#
        );
        crate::compat::open_xmile(&mut source.as_bytes()).expect("the XMILE imports")
    };
    let compile = |project: &crate::datamodel::Project| {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, project, None);
        (
            compile_project_incremental(&db, sync.project, "main"),
            collect_all_diagnostics(&db, sync.project),
        )
    };

    let valid = project_for("input");
    assert!(
        crate::module_functions::MacroRegistry::build(&valid.models)
            .expect("the registry builds")
            .resolve_macro("previous")
            .expect("the macro is registered")
            .passthrough,
        "the fixture must be a genuine passthrough, or the arm under test is not reached"
    );
    let (compiled, diags) = compile(&valid);
    assert!(
        compiled.is_ok(),
        "the declared one-argument call lowers as the builtin; diagnostics: {diags:?}"
    );

    let (compiled, diags) = compile(&project_for("input, 0"));
    assert_eq!(
        compiled.map(|_| ()).unwrap_err().code,
        ErrorCode::NotSimulatable,
        "a call that violates the macro's declared arity is refused"
    );
    let arity = diags.iter().find_map(|d| match &d.error {
        DiagnosticError::Equation(e)
            if e.code == ErrorCode::BadBuiltinArgs && d.variable.as_deref() == Some("out") =>
        {
            Some(e.details.clone().unwrap_or_default())
        }
        _ => None,
    });
    assert_eq!(
        arity.as_deref(),
        Some("macro previous takes exactly 1 argument(s), but 2 were given"),
        "the refusal names the macro's contract, not the builtin's; diagnostics: {diags:?}"
    );
}

/// Two helpers of one call claiming one name is refused, not silently
/// overwritten. A macro named `ARG1` invoked as `ARG1(k, k * 2)` mints its
/// instance as `$⁚out⁚0⁚arg1` -- the call name is the instance's part -- and
/// its second argument's helper under the same name. With a last-wins map the
/// instance replaced the helper and wired its second port to itself.
#[test]
fn a_macro_named_arg1_cannot_alias_its_own_hoisted_argument() {
    let source = mdl(r#":MACRO: ARG1(a, b)
ARG1 = a + b
	~	dmnl
	~	named so that its instance and its argument 1 helper derive one name
	|

:END OF MACRO:
k = 3
	~	dmnl
	~	|
out = ARG1(k, k * 2)
	~	dmnl
	~	|
"#);

    let err = compile_mdl(&source).expect_err("two helpers claiming one name must refuse");
    assert_eq!(err.code, ErrorCode::NotSimulatable);
    let diags = diagnostics_for(&source);
    let collision = diags.iter().find_map(|d| match &d.error {
        DiagnosticError::Equation(e)
            if e.code == ErrorCode::DuplicateVariable && d.variable.as_deref() == Some("out") =>
        {
            Some(e.details.clone().unwrap_or_default())
        }
        _ => None,
    });
    assert_eq!(
        collision.as_deref(),
        Some("two different synthesized helpers both claim the name '$⁚out⁚0⁚arg1'"),
        "diagnostics: {diags:?}"
    );
}

/// Part A + B together: a macro whose body wraps its own same-named `INIT`
/// intrinsic, INVOKED alongside a sibling macro, must (1) build the registry
/// with no false `init -> init` recursion, (2) NOT infinite-loop on the
/// invoked wrap-own-intrinsic macro (it resolves to the `LoadInitial`
/// intrinsic, terminating), and (3) leave the sibling macro's call expanding
/// correctly (the #554 cascade is gone). `INITIAL(x)` freezes x's t=0 value;
/// with constant `x` it equals `x`, so the arithmetic is hand-verifiable.
#[test]
fn issue_554_invoked_macro_wrapping_own_init_intrinsic_compiles_and_runs() {
    let source = mdl(r#":MACRO: INIT(x, k)
INIT = INITIAL(x) + k
	~	a
	~	#554: body wraps the same-canonical-name INITIAL builtin, which the
		importer renames to INIT -- NOT recursion
	|

:END OF MACRO:
:MACRO: SSHAPE(a, b)
SSHAPE = a * b
	~	a
	~	sibling macro; its name shadows the 3-arg SSHAPE builtin, so a
		registry-build failure (the #554 cascade) would make this 2-arg
		call a BadBuiltinArgs
	|

:END OF MACRO:
wrapped=
	INIT(7, 3)
	~
	~		|

sibling=
	SSHAPE(4, 5)
	~
	~		|
"#);

    // (1) No macro-registry CircularDependency cascade.
    assert!(
        !has_model_error(&diagnostics_for(&source), ErrorCode::CircularDependency),
        "the #554 false `init -> init` recursion must be gone (no \
         macro-registry CircularDependency); diagnostics: {:?}",
        diagnostics_for(&source),
    );

    // (2)+(3) Compiles and runs -- the invoked wrap-own-intrinsic macro
    // terminates (resolves to the LoadInitial opcode, NOT recursively to the
    // macro) and the sibling macro expands (no cascade).
    let wrapped = run_mdl_var(&source, "wrapped");
    let sibling = run_mdl_var(&source, "sibling");

    // INITIAL(7) = 7 (frozen at t=0), + k=3 => 10, constant over time.
    assert!(
        wrapped.iter().all(|&v| (v - 10.0).abs() < 1e-9),
        "INIT(7,3) = INITIAL(7)+3 = 10 at every step (the body's INIT(x) is \
         the renamed INITIAL intrinsic, not a recursive macro call): {wrapped:?}",
    );
    // SSHAPE(4,5) = 4*5 = 20 -- proves the sibling macro still shadows the
    // builtin and expands (the #554 cascade no longer blocks it).
    assert!(
        sibling.iter().all(|&v| (v - 20.0).abs() < 1e-9),
        "SSHAPE(4,5) = 4*5 = 20 -- the sibling macro must still expand \
         despite the wrap-own-intrinsic macro: {sibling:?}",
    );
}

/// #584 (AC7.3 Cluster A): a macro whose primary output is *exactly*
/// `INITIAL(x)` (compiles to a bare `LoadInitial`) must have that output
/// written during the parent's initials phase, so a parent reading the
/// output via `INITIAL()` sees its true t=0 value -- NOT the uninitialized
/// slot (0 in a clean VM, `inf`/NaN in C-LEARN's reused buffer).
///
/// The macro's compiled INITIALS runlist used to OMIT its own `INITIAL()`
/// primary output (`myinit = INITIAL(x)`): the output was compiled only into
/// the flows phase, so during the parent's initials the module-output slot
/// was never written. `y = MYINIT(xin, 0)` reads correctly in the *flows*
/// phase, but `z = INITIAL(y)` snapshots `y`'s never-written initials slot --
/// reading 0 (the zeroed data buffer) instead of `xin`'s t=0 value of 5. This
/// is the clean-room manifestation of the ~177-climate-var C-LEARN inf cascade
/// (`volumetric_heat_capacity = INITIAL(...)`): same structural defect, the
/// garbage value differs only because C-LEARN's slot is reused.
///
/// `MYINIT(x, k)` takes two params so the invocation is not rewritten to
/// `LOOKUP` (the unrelated 1-arg-call heuristic); `k` is unused by the body.
#[test]
fn issue_584_initial_backed_macro_output_is_written_during_initials() {
    let source = mdl(r#":MACRO: MYINIT(x, k)
MYINIT = INITIAL(x)
	~	a
	~	#584: primary output is a bare INITIAL -- must be in the macro's
		INITIALS runlist so a parent reading it during initials sees its
		true t=0 value, not the never-written (garbage) slot
	|

:END OF MACRO:
xin = 5
	~
	~		|

y=
	MYINIT(xin, 0)
	~
	~	the module output, read in the flows phase (already correct pre-fix)
	|

z=
	INITIAL(y)
	~
	~	reads the module output DURING the parent's initials phase -- this
		is what surfaces the never-written slot
	|
"#);

    // y reads the module output in the flows phase, which was always correct.
    let y = run_mdl_var(&source, "y");
    assert!(
        y.iter().all(|&v| (v - 5.0).abs() < 1e-9),
        "y = MYINIT(xin=5, 0) = INITIAL(5) = 5 at every step: {y:?}",
    );

    // z reads the module output DURING initials. Pre-fix the macro output's
    // initials slot was never written, so INITIAL(y) snapshotted garbage (0).
    // Post-fix the INITIAL-backed output is in the macro's initials runlist,
    // so z = INITIAL(y) = 5.
    let z = run_mdl_var(&source, "z");
    assert!(
        z.iter().all(|&v| (v - 5.0).abs() < 1e-9),
        "z = INITIAL(y) must read y's true t=0 value 5 (the INITIAL-backed \
         macro output must be evaluated during initials, not left as the \
         never-written/garbage slot): {z:?}",
    );
}

/// Part A + B together, the `previous` analogue (coverage symmetry with the
/// `init` test above): a macro whose canonical name is `previous`, whose body
/// wraps its own same-named `PREVIOUS` intrinsic, INVOKED alongside a sibling
/// macro, must (1) build the registry with no false `previous -> previous`
/// recursion, (2) NOT infinite-loop on the invoked wrap-own-intrinsic macro
/// (it resolves to the `LoadPrev` intrinsic, terminating), and (3) leave the
/// sibling macro's call expanding correctly (the #554 cascade is gone).
///
/// This is the faithful `previous` mirror of the `init` test: the MDL
/// importer desugars Vensim `SAMPLE IF TRUE(cond,input,init)` to
/// `... PREVIOUS(SELF, init) ...` (`mdl/xmile_compat.rs`), and the engine's
/// `Expr1` lowering recognizes only the opcode name `previous`. A user macro
/// canonically named `PREVIOUS` whose body calls `PREVIOUS(...)` is therefore
/// the same importer-rename collision as C-LEARN's `INIT = INITIAL(x)`, just
/// for the other opcode-backed intrinsic in `is_renamed_opcode_intrinsic`.
/// Before #554's Part B the invoked macro's body `PREVIOUS(x, k)` would
/// re-resolve to the `previous` macro forever (the registry-build-only
/// `issue_554_macro_wrapping_same_named_previous_intrinsic_builds_ok` in
/// `module_functions.rs` exercises Part A; nothing exercised Part B for
/// `previous` end-to-end until this test).
///
/// PREVIOUS's verified signature is `PREVIOUS(input, initial)`: at the first
/// step the `prev_values` snapshot is not yet valid so it returns `initial`;
/// thereafter it returns `input`'s previous-timestep value (`vm.rs`'s
/// `LoadPrev` + `use_prev_fallback`; cross-checked against
/// `test/previous/output.tab`, e.g. `PREVIOUS(based_on_time, 66.6)` =
/// `66.6, then the prior TIME`). Here `input` is the constant macro port
/// `x = 9` and `initial` is `k = 4`, so over the t=0,1,2 run
/// (INITIAL TIME 0, FINAL TIME 2, TIME STEP 1):
///   t=0: fallback        => k       = 4
///   t=1: prev value of x => 9 (const) = 9
///   t=2: prev value of x => 9 (const) = 9
/// i.e. `wrapped == [4, 9, 9]`. (`x` is a plain port aux, not module-backed,
/// so `PREVIOUS(x, k)` compiles straight to `LoadPrev` -- the same intrinsic
/// path C-LEARN's renamed `SAMPLE IF TRUE` desugar takes.)
#[test]
fn issue_554_invoked_macro_wrapping_own_previous_intrinsic_compiles_and_runs() {
    let source = mdl(r#":MACRO: PREVIOUS(x, k)
PREVIOUS = PREVIOUS(x, k)
	~	a
	~	#554: body wraps the same-canonical-name PREVIOUS intrinsic (the
		importer's SAMPLE IF TRUE -> PREVIOUS(SELF, init) rename target) --
		NOT recursion
	|

:END OF MACRO:
:MACRO: SSHAPE(a, b)
SSHAPE = a * b
	~	a
	~	sibling macro; its name shadows the 3-arg SSHAPE builtin, so a
		registry-build failure (the #554 cascade) would make this 2-arg
		call a BadBuiltinArgs
	|

:END OF MACRO:
wrapped=
	PREVIOUS(9, 4)
	~
	~		|

sibling=
	SSHAPE(4, 5)
	~
	~		|
"#);

    // (1) No macro-registry CircularDependency cascade.
    assert!(
        !has_model_error(&diagnostics_for(&source), ErrorCode::CircularDependency),
        "the #554 false `previous -> previous` recursion must be gone (no \
         macro-registry CircularDependency); diagnostics: {:?}",
        diagnostics_for(&source),
    );

    // (2)+(3) Compiles and runs -- the invoked wrap-own-intrinsic macro
    // terminates (resolves to the LoadPrev opcode, NOT recursively to the
    // macro) and the sibling macro expands (no cascade).
    let wrapped = run_mdl_var(&source, "wrapped");
    let sibling = run_mdl_var(&source, "sibling");

    // PREVIOUS(x=9, k=4): t=0 => k=4 (fallback, prev_values not yet valid);
    // t>=1 => x's previous-timestep value = 9 (x is the constant port 9).
    let expected_wrapped = [4.0, 9.0, 9.0];
    assert_eq!(
        wrapped.len(),
        expected_wrapped.len(),
        "expected one value per step over the t=0,1,2 run: {wrapped:?}",
    );
    for (i, (&got, &want)) in wrapped.iter().zip(expected_wrapped.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1e-9,
            "PREVIOUS(9,4) at step {i} expected {want} (the body's PREVIOUS(x,k) \
             is the renamed PREVIOUS intrinsic, not a recursive macro call): \
             got {got}, full series {wrapped:?}",
        );
    }
    // SSHAPE(4,5) = 4*5 = 20 -- proves the sibling macro still shadows the
    // builtin and expands (the #554 cascade no longer blocks it).
    assert!(
        sibling.iter().all(|&v| (v - 20.0).abs() < 1e-9),
        "SSHAPE(4,5) = 4*5 = 20 -- the sibling macro must still expand \
         despite the wrap-own-intrinsic macro: {sibling:?}",
    );
}

/// macros.AC5.2 end-to-end guard adjacent to the #554 fix: a GENUINELY
/// self-recursive macro (`FOO = FOO(...)`, `FOO` is NOT an opcode intrinsic)
/// invoked from `main` must STILL fail to compile with a recursion cycle.
/// The #554 exception is scoped to the same-named-opcode-intrinsic case only;
/// real recursion stays rejected (mirrors
/// `ac5_2_directly_recursive_macro_fails_compile_with_cycle`, kept here so a
/// regression that over-broadens the #554 carve-out is caught next to it).
#[test]
fn issue_554_does_not_weaken_ac5_2_genuine_recursion_end_to_end() {
    let source = mdl(r#":MACRO: SELFCALL(a, b)
SELFCALL = SELFCALL(a, b) + 1
	~	a
	~	genuine self-recursion (SELFCALL is not an opcode intrinsic)
	|

:END OF MACRO:
y=
	SELFCALL(3, 4)
	~
	~		|
"#);

    let err = compile_mdl(&source).expect_err(
        "genuine self-recursion of a non-intrinsic macro must STILL fail to \
         compile -- the #554 exception must not weaken macros.AC5.2",
    );
    assert_eq!(err.code, ErrorCode::NotSimulatable);
    let details = err.get_details().unwrap_or_default();
    assert!(
        details.contains("recursive macro") && details.to_lowercase().contains("selfcall"),
        "the surfaced cycle message must name the recursive macro: {details:?}",
    );
    assert!(
        has_model_error(&diagnostics_for(&source), ErrorCode::CircularDependency),
        "genuine recursion must still accumulate a CircularDependency diagnostic",
    );
}

// ── #554 follow-up: macro wrapping a same-canonical-name STDLIB-MODULE-backed
//    renamed builtin (`DELAY N`; the metasd thyroid-2008-d.mdl case) ──────────
//
// The MDL importer rewrites Vensim `DELAY N(input,dt,init,n)` to the
// single-token XMILE `DELAYN(input,dt,n,init)` (`mdl/xmile_compat.rs`). So
// thyroid-2008-d.mdl's `:MACRO: DELAYN(Input,DelayTime,Init,Order) ... DELAYN
// = DELAY N(Input,DelayTime,Init,Order)` stores the datamodel macro body
// `delayn = delayn(input, delaytime, order, init)`. Pre-fix, the macro
// recursion check mistook that renamed-builtin call for a recursive `delayn ->
// delayn` macro edge and failed the WHOLE `MacroRegistry::build` -- the same
// #554 cascade, but for a *stdlib-module-backed* builtin that #554 was
// deliberately scoped to exclude (its termination argument -- fall through to
// the LoadInitial/LoadPrev opcode -- did not cover the stdlib-module case).
//
// The follow-up extends the shared self-edge suppression to the
// stdlib-module-backed renamed-builtin set: skipping the macro resolve makes
// the body's `delayn(...)` fall through to
// `rewrite_alias_module_call`/`stdlib_descriptor`, resolving to a DISTINCT
// `stdlib⁚delay1`/`stdlib⁚delay3` module (never the user `delayn` macro
// model), so it terminates and computes the stdlib delay behavior.

/// Part A + B together (the precise termination e2e, the stdlib-module
/// analogue of the `init`/`previous` #554 e2e tests): an INVOKED macro
/// `DELAYN` whose body wraps its own same-named `DELAY N` with a *literal*
/// order, alongside a sibling SSHAPE macro, must (1) build the registry with
/// no false `delayn -> delayn` recursion, (2) NOT infinite-loop / form a
/// salsa module-map cycle on the invoked wrap-own-builtin macro (the body's
/// `delayn` resolves to the distinct `stdlib⁚delay1` MODULE, not recursively
/// to the macro), and (3) leave the sibling macro's call expanding correctly
/// (the #554-class cascade is gone).
///
/// Why the order is a literal in the macro *body*: `DELAY N`'s stdlib
/// expansion picks `delay1` vs `delay3` from the *value* of the order arg
/// (`builtins_visitor::rewrite_alias_module_call` requires a compile-time
/// constant). The faithful thyroid shape passes the order as a macro *port*
/// (`DELAYN = DELAY N(Input,DelayTime,Init,Order)`), which the macro
/// *template* cannot resolve to a literal -- an orthogonal, pre-existing
/// stdlib limitation that surfaces as a non-macro-attributable
/// `UnknownBuiltin` *inside the macro body* (exactly the "unrelated blocker
/// in a macro body" the metasd harness tolerates; pinned structurally by the
/// sibling test below, and tracked separately). To isolate the property
/// under test here -- the #554-follow-up *termination* (the self-named
/// `delayn` call resolving to the distinct stdlib module rather than
/// recursing) -- the body fixes the order to `1`, so the importer-rewritten
/// body is `delayn(input, delaytime, 1, init)` and
/// `rewrite_alias_module_call` resolves it to the stdlib `delay1` model. The
/// macro stays the #554-collision shape (canonical name `delayn`, body calls
/// `delayn`).
///
/// `:MACRO: DELAYN(Input, DelayTime, Init) ... DELAYN = DELAY N(Input,
/// DelayTime, Init, 1)`; the importer rewrites the body's
/// `DELAY N(in,dt,init,1)` to `DELAYN(in,dt,1,init)`. Invoked as
/// `DELAYN(10, 5, 0)` the body is `delayn(10, 5, 1, 0)` -> stdlib `delay1`
/// as `DELAY1(10, 5, 0)`.
///
/// DELAY N is an Nth-order material (Erlang) delay; order 1 is the stdlib
/// `delay1` model (`stdlib.gen.rs`): a one-stock material delay with
/// `stock(0) = init*delay_time`, `output = stock/delay_time`,
/// `stock' = input - output`, integrated by Euler with DT. With input=10,
/// delay_time=5, init=0, DT=1 over t=0,1,2 (INITIAL TIME 0, FINAL TIME 2,
/// TIME STEP 1) -- identical arithmetic to the verified
/// `builtins_visitor::tests::test_arrayed_delay1_numerical_values`:
///   t=0: stock=0,                output=0/5   = 0
///   t=1: stock=0 +1*(10-0) =10,  output=10/5  = 2
///   t=2: stock=10+1*(10-2) =18,  output=18/5  = 3.6
/// i.e. `wrapped == [0, 2, 3.6]` -- a concrete closed-form expected series
/// (not merely a structural assertion), proving the body's `delayn(...)`
/// resolved to the stdlib delay module and computed DELAY N's defined
/// behavior rather than recursing. Non-vacuity: with the #554-follow-up
/// extension removed, `compile_mdl` RED-fails here with the
/// `recursive macro: delayn -> delayn` cascade (Part A) / a salsa
/// module-map dependency cycle (Part B), exactly as the
/// `module_functions.rs` RED proof showed.
#[test]
fn issue_554_followup_invoked_macro_wrapping_own_delayn_builtin_compiles_and_runs() {
    let source = mdl(r#":MACRO: DELAYN(Input, DelayTime, Init)
DELAYN = DELAY N(Input, DelayTime, Init, 1)
	~	a
	~	#554 follow-up: body wraps the same-canonical-name DELAY N builtin,
		which the importer renames to the single-token DELAYN -- NOT recursion
	|

:END OF MACRO:
:MACRO: SSHAPE(a, b)
SSHAPE = a * b
	~	a
	~	sibling macro; its name shadows the 3-arg SSHAPE builtin, so a
		registry-build failure (the #554 cascade) would make this 2-arg
		call a BadBuiltinArgs
	|

:END OF MACRO:
wrapped=
	DELAYN(10, 5, 0)
	~
	~		|

sibling=
	SSHAPE(4, 5)
	~
	~		|
"#);

    // (1) No macro-registry CircularDependency cascade (Part A: the false
    // `delayn -> delayn` self-edge is suppressed for the renamed stdlib
    // builtin, exactly as for `init`/`previous`).
    assert!(
        !has_model_error(&diagnostics_for(&source), ErrorCode::CircularDependency),
        "the #554-class false `delayn -> delayn` recursion must be gone (no \
         macro-registry CircularDependency); diagnostics: {:?}",
        diagnostics_for(&source),
    );

    // (2)+(3) Compiles and runs -- the invoked wrap-own-builtin macro
    // terminates (the body's `delayn(...)` resolves to the stdlib⁚delay1
    // MODULE via rewrite_alias_module_call, NOT recursively to the macro) and
    // the sibling macro expands (no cascade).
    let wrapped = run_mdl_var(&source, "wrapped");
    let sibling = run_mdl_var(&source, "sibling");

    // DELAYN(10,5,0) (body order literal 1) == DELAY1(10,5,0): [0, 2, 3.6]
    // over t=0,1,2 (the body's `delayn` is the renamed DELAY N builtin
    // resolving to the stdlib delay module, not a recursive macro call).
    let expected_wrapped = [0.0, 2.0, 3.6];
    assert_eq!(
        wrapped.len(),
        expected_wrapped.len(),
        "expected one value per step over the t=0,1,2 run: {wrapped:?}",
    );
    for (i, (&got, &want)) in wrapped.iter().zip(expected_wrapped.iter()).enumerate() {
        assert!(
            (got - want).abs() < 1e-9,
            "DELAYN(10,5,0) (body order 1 -> stdlib delay1) at step {i} \
             expected {want} (the body's DELAY N is the renamed builtin \
             resolving to the stdlib delay module, not a recursive macro \
             call): got {got}, full series {wrapped:?}",
        );
    }
    // SSHAPE(4,5) = 4*5 = 20 -- proves the sibling macro still shadows the
    // builtin and expands (the #554-class cascade no longer blocks it).
    assert!(
        sibling.iter().all(|&v| (v - 20.0).abs() < 1e-9),
        "SSHAPE(4,5) = 4*5 = 20 -- the sibling macro must still expand \
         despite the wrap-own-builtin macro: {sibling:?}",
    );
}

/// Part A + B, the *faithful thyroid shape* (the metasd
/// `thyroid-2008-d.mdl` `:MACRO: DELAYN(Input,DelayTime,Init,Order) ...
/// DELAYN = DELAY N(Input,DelayTime,Init,Order)` with the order as a macro
/// *port*), asserted structurally per the task's allowance for when an exact
/// closed-form run is impractical.
///
/// What this pins (the #554-follow-up deliverable for thyroid): the
/// macro-registry builds with NO false `delayn -> delayn`
/// `CircularDependency` (the #554-class cascade), the sibling macro still
/// resolves, and there is NO macro-attributable diagnostic (a registry-build
/// error or a macro/model name collision), matching the metasd corpus
/// harness's AC6.4 "macro-attributable" definition. The macro template
/// body's `DELAY N(...,Order)` -- with the order an unresolved port -- still
/// surfaces an `UnknownBuiltin` *inside the macro body*
/// (`rewrite_alias_module_call` needs a compile-time-constant order; a macro
/// port is not one). That is an orthogonal, pre-existing stdlib limitation,
/// NOT a macro-handling failure: it is the same gap `DELAY N(x,dt,init,v)`
/// with a non-constant `v` hits in a plain `main` equation, and it is
/// exactly the "unrelated blocker in a macro body" class the metasd
/// expansion tier tolerates (so thyroid PASSES the expansion tier). This
/// test asserts the macro-attributable set is empty (the property the
/// follow-up fixes) and explicitly tolerates the orthogonal in-body
/// `UnknownBuiltin` (tracked separately, surfaced for a tracking issue).
#[test]
fn issue_554_followup_thyroid_shape_builds_with_no_macro_attributable_diag() {
    // The exact thyroid macro shape: order is a macro PORT, not a literal.
    let source = mdl(r#":MACRO: DELAYN(Input, DelayTime, Init, Order)
DELAYN = DELAY N(Input, DelayTime, Init, Order)
	~	a
	~	faithful thyroid shape: DELAY N order is the macro port `Order`
	|

:END OF MACRO:
:MACRO: SSHAPE(a, b)
SSHAPE = a * b
	~	a
	~	sibling macro (would BadBuiltinArgs under the #554 cascade)
	|

:END OF MACRO:
wrapped=
	DELAYN(10, 5, 0, 1)
	~
	~		|

sibling=
	SSHAPE(4, 5)
	~
	~		|
"#);

    let project = open_vensim(&source).expect("MDL must parse");
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, &project, None);
    let diags = collect_all_diagnostics(&db, sync.project);

    let macro_models: std::collections::BTreeSet<&str> = project
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.as_str())
        .collect();

    // The metasd-harness "macro-attributable" classifier (kept in lockstep
    // with `tests/integration/metasd_macros.rs`): a project-level macro-registry build
    // error (the #554 cascade class) or a macro/model name collision.
    let macro_attributable: Vec<&crate::db::Diagnostic> = diags
        .iter()
        .filter(|d| {
            let code = match &d.error {
                DiagnosticError::Equation(e) => Some(e.code),
                DiagnosticError::Model(e) => Some(e.code),
                _ => None,
            };
            let is_project_level = d.model.is_empty() && d.variable.is_none();
            let in_macro_model = macro_models.contains(d.model.as_str());
            let registry_build_error = is_project_level
                && matches!(&d.error, DiagnosticError::Model(_))
                && matches!(
                    code,
                    Some(ErrorCode::CircularDependency) | Some(ErrorCode::DuplicateMacroName)
                );
            let name_collision = matches!(
                code,
                Some(ErrorCode::BadModelName) | Some(ErrorCode::DuplicateMacroName)
            ) && (in_macro_model || is_project_level);
            registry_build_error || name_collision
        })
        .collect();

    assert!(
        macro_attributable.is_empty(),
        "the faithful thyroid shape must produce ZERO macro-attributable \
         diagnostics after the #554 follow-up (no false `delayn -> delayn` \
         registry CircularDependency, no macro/model name collision); got: \
         {macro_attributable:#?}",
    );

    // Specifically: the #554-class false-positive recursion is gone.
    assert!(
        !has_model_error(&diags, ErrorCode::CircularDependency),
        "no project-level `recursive macro: delayn -> delayn` \
         CircularDependency; diags: {diags:?}",
    );

    // Structural: the registry resolves BOTH the wrap-own-builtin macro and
    // the sibling (proving the cascade that un-shadowed siblings is gone).
    let registry = crate::module_functions::MacroRegistry::build(&project.models)
        .expect("registry must build (no false delayn -> delayn recursion)");
    assert!(
        registry.resolve_macro("delayn").is_some(),
        "the `delayn` macro must still be registered"
    );
    assert!(
        registry.resolve_macro("sshape").is_some(),
        "the sibling `sshape` macro must resolve -- no #554-class cascade"
    );

    // The ONLY remaining error is the orthogonal, non-macro-attributable
    // in-body `UnknownBuiltin` (DELAY N with a non-constant/port order):
    // assert it is confined to the macro body and is NOT one of the
    // macro-attributable codes (documents the tolerated unrelated blocker).
    for d in &diags {
        if d.severity != DiagnosticSeverity::Error {
            continue;
        }
        let code = match &d.error {
            DiagnosticError::Equation(e) => Some(e.code),
            DiagnosticError::Model(e) => Some(e.code),
            _ => None,
        };
        assert_eq!(
            code,
            Some(ErrorCode::UnknownBuiltin),
            "the only tolerated Error here is the orthogonal in-body \
             UnknownBuiltin (DELAY N needs a constant order; the macro port \
             is not one) -- any other Error means a real regression: {d:?}",
        );
        assert!(
            macro_models.contains(d.model.as_str()),
            "the tolerated UnknownBuiltin must be inside a macro body \
             (model={:?}), not project-level/main: {d:?}",
            d.model,
        );
    }
}

// ── unit checking: macro-marked models are templates, like stdlib ──────────

/// F4: a macro-marked model is a generic template -- its formal parameters are
/// unitless, so unit checking it in isolation produces only spurious errors.
/// Like a stdlib model, it must be SKIPPED by unit checking. Even a blatant
/// dimensional inconsistency in the macro body (here `meters + seconds`) must
/// not surface a diagnostic attributed to the macro model. (Macro correctness
/// is validated at instantiation via cross-module unit constraints; this is
/// the source of C-LEARN's spurious `ramp_from_to`/`sshape` unit warnings.)
#[test]
fn macro_body_units_are_not_checked() {
    let source = mdl(r#":MACRO: BADUNITS(a, b)
BADUNITS = lhs + rhs
	~	widgets
	~	|
lhs = a
	~	meters
	~	|
rhs = b
	~	seconds
	~	|
:END OF MACRO:
y =
	BADUNITS(3, 4)
	~	widgets
	~	|
"#);

    let diags = diagnostics_for(&source);
    let macro_diags: Vec<_> = diags.iter().filter(|d| d.model == "badunits").collect();
    assert!(
        macro_diags.is_empty(),
        "unit checking must skip macro-marked models; diagnostics attributed \
         to the `badunits` macro model:\n{macro_diags:#?}",
    );
}

/// A macro body's `INITIAL(input)` of its formal parameter reads the bound
/// port's own slot: the invoked value is the parameter's t=0 value, and no
/// capture is synthesized for it.
///
/// `EXPRESSION MACRO(input, parameter) = INITIAL(input) * parameter +
/// SMOOTH(input, 2)` over `macro input = Time + 5` and `macro parameter =
/// 1.1` over the control tail's `TIME = 0..2` at dt 1: `INITIAL(input) *
/// parameter = 5 * 1.1 = 5.5`, and the `SMOOTH` starts at its input and
/// follows it with delay 2 (`5, 5, 5.5`), so the output is `10.5, 10.5, 11`.
///
/// The body's helpers are the `SMOOTH` call's alone, numbered from 0: a
/// capture of the port would have taken `⁚0⁚` and pushed the instance to
/// `⁚1⁚smth1`. Helper names are external keys (the results offset map, the
/// LTM causal graph libsimlin surfaces), so which name the instance carries
/// is pinned rather than left to drift.
#[test]
fn a_macro_body_snapshot_of_its_formal_parameter_reads_the_bound_port() {
    use crate::db::sync_from_datamodel;
    use crate::test_common::implicit_vars_of;

    let source = mdl(r#":MACRO: EXPRESSION MACRO(input, parameter)
EXPRESSION MACRO = INITIAL(input) * parameter + SMOOTH(input, 2)
	~	input
	~	a macro whose body snapshots its formal parameter through INITIAL
	|

:END OF MACRO:
macro input=
	Time + 5
	~
	~		|

macro output=
	EXPRESSION MACRO(macro input,macro parameter)
	~
	~		|

macro parameter=
	1.1
	~
	~		|
"#);
    let output = run_mdl_var(&source, "macro_output");
    assert_eq!(output, vec![10.5, 10.5, 11.0]);

    let project = open_vensim(&source).expect("the macro source imports");
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let names: Vec<String> = implicit_vars_of(&db, &sync, "expression_macro", "expression_macro")
        .iter()
        .map(|v| v.ident().to_string())
        .collect();
    assert_eq!(
        names,
        ["$⁚expression_macro⁚0⁚arg1", "$⁚expression_macro⁚0⁚smth1"],
        "INITIAL of the bound port captures nothing, so the SMOOTH call's helpers \
         are the body's only ones and take walk index 0"
    );
}
