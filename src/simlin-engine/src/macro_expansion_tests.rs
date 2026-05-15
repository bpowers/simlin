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

    let (transformed, vars) =
        crate::builtins_visitor::instantiate_implicit_modules("y", ast, None, None, &registry)
            .expect("a macro call must expand");

    let modules: Vec<&crate::datamodel::Module> = vars
        .iter()
        .filter_map(|v| match v {
            crate::datamodel::Variable::Module(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(
        modules.len(),
        1,
        "a single-output macro call synthesizes exactly one Module, got {vars:?}",
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

/// `contains_module_call` macro-awareness (item 4 of Task 3): with a
/// registry containing macro `MYMACRO`, it returns `true` for an *arrayed*
/// macro `App` (`MYMACRO(x[Dim], k)`), `true` for a stdlib call
/// (`SMTH1(x, 5)`), and `false` for a plain arithmetic expression
/// (`a + b`).
#[test]
fn contains_module_call_is_macro_aware() {
    use crate::ast::Expr0;

    let registry = mymacro_registry();
    let parse = |s: &str| {
        Expr0::new(s, crate::lexer::LexerType::Equation)
            .expect("parse")
            .expect("non-empty")
    };

    assert!(
        crate::builtins_visitor::contains_module_call(&parse("MYMACRO(x[Dim], k)"), &registry),
        "an arrayed macro App must be recognized by the apply-to-all gate",
    );
    assert!(
        crate::builtins_visitor::contains_module_call(&parse("SMTH1(x, 5)"), &registry),
        "a stdlib call must still be recognized by the apply-to-all gate",
    );
    assert!(
        !crate::builtins_visitor::contains_module_call(&parse("a + b"), &registry),
        "a plain arithmetic expression is not a module call",
    );
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
