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

    let (transformed, vars) = crate::builtins_visitor::instantiate_implicit_modules(
        "y", ast, None, None, &registry, None,
    )
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
// `corpus_clearn_macros_import` in `tests/simulate.rs`). The macros take two
// params so the invocation is not rewritten to `LOOKUP` (the unrelated #553
// 1-arg-call heuristic) -- the `INITIAL(x)`->`INIT(x)` rename inside the body
// (the #554 trigger) is independent of the invocation's arity.

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
    let diags = collect_all_diagnostics(&db, &sync.to_sync_result());

    let macro_models: std::collections::BTreeSet<&str> = project
        .models
        .iter()
        .filter(|m| m.macro_spec.is_some())
        .map(|m| m.name.as_str())
        .collect();

    // The metasd-harness "macro-attributable" classifier (kept in lockstep
    // with `tests/metasd_macros.rs`): a project-level macro-registry build
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
