// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Import-verification of the bundled `:MACRO:` test fixtures.
//!
//! These tests verify that every `test/test-models/tests/macro_*` `.mdl`
//! fixture imports into a `datamodel::Project` whose macro-marked `Model`(s)
//! carry the expected `MacroSpec`, body variables, and synthesized formal-
//! parameter port variables, and that the `"main"` model preserves each
//! invocation as ordinary equation text (Phase 2 does not expand
//! invocations). It also pins the unterminated-macro / stray-end error cases.
//!
//! Fixtures are embedded with `include_str!` so a missing fixture is a
//! compile error rather than a silently-skipped test (the project's stated
//! "fail loudly if a required test file is missing" preference). The relative
//! path is five `../` from this file's directory
//! (`src/simlin-engine/src/mdl/convert/`) to the repository root.
//!
//! Verifies: macros.AC1.1, macros.AC1.6, macros.AC1.7.

use crate::datamodel::{Equation, Model, Project, Variable};
use crate::mdl::ReaderError;

use super::{ConvertError, convert_mdl};

// --- Embedded fixtures -----------------------------------------------------

const MACRO_EXPRESSION: &str = include_str!(
    "../../../../../test/test-models/tests/macro_expression/test_macro_expression.mdl"
);
const MACRO_MULTI_EXPRESSION: &str = include_str!(
    "../../../../../test/test-models/tests/macro_multi_expression/test_macro_multi_expression.mdl"
);
const MACRO_STOCK: &str =
    include_str!("../../../../../test/test-models/tests/macro_stock/test_macro_stock.mdl");
const MACRO_CROSS_REFERENCE: &str = include_str!(
    "../../../../../test/test-models/tests/macro_cross_reference/test_macro_cross_reference.mdl"
);
const MACRO_MULTI_MACROS: &str = include_str!(
    "../../../../../test/test-models/tests/macro_multi_macros/test_macro_multi_macros.mdl"
);
const MACRO_TRAILING_DEFINITION: &str = include_str!(
    "../../../../../test/test-models/tests/macro_trailing_definition/test_macro_trailing_definition.mdl"
);

// --- Shared assertion helpers ---------------------------------------------

/// Find the (single) macro-marked model with the given canonical name.
fn macro_model<'a>(project: &'a Project, name: &str) -> &'a Model {
    project
        .models
        .iter()
        .find(|m| m.name == name && m.macro_spec.is_some())
        .unwrap_or_else(|| {
            panic!(
                "expected macro-marked model {:?}; models present: {:?}",
                name,
                project
                    .models
                    .iter()
                    .map(|m| (m.name.clone(), m.macro_spec.is_some()))
                    .collect::<Vec<_>>()
            )
        })
}

fn main_model(project: &Project) -> &Model {
    project
        .models
        .iter()
        .find(|m| m.name == "main")
        .expect("project must contain a \"main\" model")
}

fn var<'a>(model: &'a Model, ident: &str) -> &'a Variable {
    model
        .variables
        .iter()
        .find(|v| v.get_ident() == ident)
        .unwrap_or_else(|| {
            panic!(
                "expected variable {:?} in model {:?}; variables: {:?}",
                ident,
                model.name,
                model
                    .variables
                    .iter()
                    .map(|v| v.get_ident().to_string())
                    .collect::<Vec<_>>()
            )
        })
}

fn scalar_eq(v: &Variable) -> String {
    match v.get_equation() {
        Some(Equation::Scalar(s)) => s.clone(),
        other => panic!("expected Scalar equation, got {:?}", other),
    }
}

/// Assert a model's variable idents are *exactly* `expected` (order-
/// independent). This pins the phase spec's "variables match the body
/// equations PLUS a synthesized port var per formal parameter" precisely:
/// no missing body variable, no extra/leaked synthetic, no duplicated port.
fn assert_exact_vars(model: &Model, expected: &[&str]) {
    let mut got: Vec<&str> = model.variables.iter().map(|v| v.get_ident()).collect();
    got.sort_unstable();
    let mut want: Vec<&str> = expected.to_vec();
    want.sort_unstable();
    assert_eq!(
        got, want,
        "model {:?} variable set mismatch (body equations + one synthesized \
         port per formal parameter, nothing else)",
        model.name
    );
}

/// Assert the macro `Model` carries the expected `MacroSpec` and a
/// `can_be_module_input` port variable for every formal parameter.
///
/// `params` is the header input list in order; `additional_outputs` the
/// (possibly empty) `:`-list. This is the common shape every fixture shares,
/// so a per-fixture test only adds the body-equation specifics.
fn assert_macro_shape<'a>(
    project: &'a Project,
    macro_name: &str,
    params: &[&str],
    additional_outputs: &[&str],
) -> &'a Model {
    let m = macro_model(project, macro_name);
    let spec = m
        .macro_spec
        .as_ref()
        .expect("macro model must have macro_spec: Some");

    assert_eq!(
        spec.parameters,
        params.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "MacroSpec.parameters must match the header input list in order"
    );
    assert_eq!(
        spec.primary_output, macro_name,
        "MacroSpec.primary_output must be the canonicalized macro name"
    );
    assert_eq!(
        spec.additional_outputs,
        additional_outputs
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>(),
        "MacroSpec.additional_outputs must match the `:`-output list in order"
    );

    // Every formal parameter is synthesized as a port variable with the
    // module-input flag set (so the macro Model is a self-contained,
    // compilable sub-model).
    for p in params {
        let port = var(m, p);
        assert!(
            port.can_be_module_input(),
            "port variable {:?} of macro {:?} must have can_be_module_input == true",
            p,
            macro_name
        );
    }

    m
}

/// Assert the `"main"` model is not macro-marked and preserves `invocation`
/// (a macro name) as an unexpanded call in `caller_var`'s equation text.
fn assert_invocation_preserved(project: &Project, caller_var: &str, invoked_macro: &str) {
    let main = main_model(project);
    assert!(
        main.macro_spec.is_none(),
        "the \"main\" model must have macro_spec: None"
    );
    let inv = var(main, caller_var);
    let inv_eq = scalar_eq(inv);
    // The MDL formatter keeps the call-site name's original casing
    // (`EXPRESSION_MACRO(...)`); compare case-insensitively. The trailing `(`
    // proves it stayed a call and was not expanded / materialized.
    assert!(
        inv_eq
            .to_lowercase()
            .contains(&format!("{}(", invoked_macro)),
        "invocation in {:?} ({:?}) must preserve the {:?} call verbatim",
        caller_var,
        inv_eq,
        invoked_macro
    );
}

// --- macros.AC1.1: the 6 fixtures import as macro-marked models ------------

#[test]
fn fixture_macro_expression_imports() {
    // Single single-output macro `EXPRESSION MACRO(input, parameter)` whose
    // body is one aux `EXPRESSION MACRO = input * parameter`.
    let project = convert_mdl(MACRO_EXPRESSION).expect("macro_expression must import");

    assert_eq!(
        project
            .models
            .iter()
            .filter(|m| m.macro_spec.is_some())
            .count(),
        1,
        "macro_expression defines exactly one macro"
    );

    let m = assert_macro_shape(&project, "expression_macro", &["input", "parameter"], &[]);

    // Body equations (`expression_macro`) plus one synthesized port per
    // formal parameter -- nothing else.
    assert_exact_vars(m, &["expression_macro", "input", "parameter"]);

    let body = var(m, "expression_macro");
    assert!(
        matches!(body, Variable::Aux(_)),
        "body primary output must be an Aux, got {:?}",
        body
    );
    let body_eq = scalar_eq(body);
    assert!(
        body_eq.contains("input") && body_eq.contains("parameter"),
        "body equation {:?} must reference both formal parameters \
         byte-identically to MacroSpec.parameters",
        body_eq
    );

    assert_invocation_preserved(&project, "macro_output", "expression_macro");
}

#[test]
fn fixture_macro_multi_expression_imports() {
    // Same header; two-equation body: the primary output references a helper
    // `intermediate`, which the scoped conversion pipeline resolves on its
    // own (it is an ordinary body aux, not a port).
    let project = convert_mdl(MACRO_MULTI_EXPRESSION).expect("macro_multi_expression must import");

    let m = assert_macro_shape(&project, "expression_macro", &["input", "parameter"], &[]);

    // Two body equations (`expression_macro`, the `intermediate` helper)
    // plus one synthesized port per formal parameter.
    assert_exact_vars(
        m,
        &["expression_macro", "intermediate", "input", "parameter"],
    );

    let body = var(m, "expression_macro");
    assert!(matches!(body, Variable::Aux(_)));
    let body_eq = scalar_eq(body);
    assert!(
        body_eq.contains("input") && body_eq.contains("intermediate"),
        "primary-output body equation {:?} must reference `input` and the \
         `intermediate` helper",
        body_eq
    );

    // The helper is an ordinary body aux, NOT a synthesized port.
    let helper = var(m, "intermediate");
    assert!(matches!(helper, Variable::Aux(_)));
    assert!(
        !helper.can_be_module_input(),
        "the `intermediate` helper must not be a module-input port"
    );
    let helper_eq = scalar_eq(helper);
    assert!(
        helper_eq.contains("parameter"),
        "helper equation {:?} must reference `parameter`",
        helper_eq
    );

    assert_invocation_preserved(&project, "macro_output", "expression_macro");
}

#[test]
fn fixture_macro_stock_imports() {
    // Stock-bodied macro `EXPRESSION MACRO = INTEG(input, parameter)`:
    // the body var is a Stock, `input` (the INTEG rate) a Flow port, and
    // `parameter` (the INTEG initial) an Aux port -- both module-input.
    let project = convert_mdl(MACRO_STOCK).expect("macro_stock must import");

    let m = assert_macro_shape(&project, "expression_macro", &["input", "parameter"], &[]);

    // The body stock plus its two synthesized ports -- nothing else.
    assert_exact_vars(m, &["expression_macro", "input", "parameter"]);

    let body = var(m, "expression_macro");
    let stock = match body {
        Variable::Stock(s) => s,
        other => panic!("body primary output must be a Stock, got {:?}", other),
    };

    let input_port = var(m, "input");
    assert!(
        matches!(input_port, Variable::Flow(_)),
        "the INTEG rate parameter `input` must synthesize as a Flow, got {:?}",
        input_port
    );
    assert!(input_port.can_be_module_input());

    let parameter_port = var(m, "parameter");
    assert!(
        matches!(parameter_port, Variable::Aux(_)),
        "the INTEG initial parameter `parameter` must synthesize as an Aux, got {:?}",
        parameter_port
    );
    assert!(parameter_port.can_be_module_input());

    // The stock's inflow resolves to `input` directly, or via a synthetic
    // flow whose scalar equation is `input`.
    let direct = stock.inflows.iter().any(|f| f == "input");
    let via_synthetic = stock.inflows.iter().any(|fname| {
        m.variables.iter().any(|v| {
            v.get_ident() == fname
                && matches!(v.get_equation(), Some(Equation::Scalar(s)) if s == "input")
        })
    });
    assert!(
        direct || via_synthetic,
        "stock inflow must resolve to `input` (directly or via a synthetic \
         flow whose equation is `input`); inflows={:?}",
        stock.inflows
    );

    assert_invocation_preserved(&project, "macro_output", "expression_macro");
}

#[test]
fn fixture_macro_cross_reference_imports() {
    // Two macros. `SECOND MACRO = input / parameter`, and
    // `EXPRESSION MACRO = SECOND MACRO(input, parameter)` -- a macro call
    // *inside* a macro body, which stays as equation text in Phase 2 (it is
    // not expanded here).
    let project = convert_mdl(MACRO_CROSS_REFERENCE).expect("macro_cross_reference must import");

    assert_eq!(
        project
            .models
            .iter()
            .filter(|m| m.macro_spec.is_some())
            .count(),
        2,
        "macro_cross_reference defines exactly two macros"
    );

    // SECOND MACRO: ordinary single-aux body.
    let second = assert_macro_shape(&project, "second_macro", &["input", "parameter"], &[]);
    assert_exact_vars(second, &["second_macro", "input", "parameter"]);
    let second_body = var(second, "second_macro");
    assert!(matches!(second_body, Variable::Aux(_)));
    let second_eq = scalar_eq(second_body);
    assert!(
        second_eq.contains("input") && second_eq.contains("parameter"),
        "SECOND MACRO body {:?} must reference both parameters",
        second_eq
    );

    // EXPRESSION MACRO: its body calls SECOND MACRO -- the call must remain
    // as unexpanded equation text (Phase 2 does not inline macro calls,
    // even body-level ones).
    let expr = assert_macro_shape(&project, "expression_macro", &["input", "parameter"], &[]);
    assert_exact_vars(expr, &["expression_macro", "input", "parameter"]);
    let expr_body = var(expr, "expression_macro");
    assert!(matches!(expr_body, Variable::Aux(_)));
    let expr_eq = scalar_eq(expr_body);
    assert!(
        expr_eq.to_lowercase().contains("second_macro("),
        "EXPRESSION MACRO body {:?} must keep the `SECOND MACRO(...)` call \
         as equation text (not expanded in Phase 2)",
        expr_eq
    );

    assert_invocation_preserved(&project, "macro_output", "expression_macro");
}

#[test]
fn fixture_macro_multi_macros_imports() {
    // Two independent macros, each invoked from `main`.
    let project = convert_mdl(MACRO_MULTI_MACROS).expect("macro_multi_macros must import");

    assert_eq!(
        project
            .models
            .iter()
            .filter(|m| m.macro_spec.is_some())
            .count(),
        2,
        "macro_multi_macros defines exactly two macros"
    );

    let expr = assert_macro_shape(&project, "expression_macro", &["input", "parameter"], &[]);
    assert_exact_vars(expr, &["expression_macro", "input", "parameter"]);
    let expr_eq = scalar_eq(var(expr, "expression_macro"));
    assert!(
        expr_eq.contains("input") && expr_eq.contains("parameter"),
        "EXPRESSION MACRO body {:?} must reference both parameters",
        expr_eq
    );

    let second = assert_macro_shape(&project, "second_macro", &["input", "parameter"], &[]);
    assert_exact_vars(second, &["second_macro", "input", "parameter"]);
    let second_eq = scalar_eq(var(second, "second_macro"));
    assert!(
        second_eq.contains("input") && second_eq.contains("parameter"),
        "SECOND MACRO body {:?} must reference both parameters",
        second_eq
    );

    // Both invocations are preserved in `main`.
    assert_invocation_preserved(&project, "macro_output", "expression_macro");
    assert_invocation_preserved(&project, "second_macro_output", "second_macro");
}

#[test]
fn fixture_macro_trailing_definition_imports() {
    // macros.AC1.7: the macro is defined *after* its call site. The reader
    // collects every MdlItem before convert() runs, so definition order is
    // irrelevant -- the macro Model is produced and `main`'s invocation is
    // preserved.
    let project =
        convert_mdl(MACRO_TRAILING_DEFINITION).expect("macro_trailing_definition must import");

    assert_eq!(
        project
            .models
            .iter()
            .filter(|m| m.macro_spec.is_some())
            .count(),
        1,
        "macro_trailing_definition defines exactly one macro"
    );

    let m = assert_macro_shape(&project, "expression_macro", &["input", "parameter"], &[]);
    assert_exact_vars(m, &["expression_macro", "input", "parameter"]);
    let body_eq = scalar_eq(var(m, "expression_macro"));
    assert!(
        body_eq.contains("input") && body_eq.contains("parameter"),
        "trailing-defined macro body {:?} must reference both parameters",
        body_eq
    );

    // The call site appears *before* the :MACRO: block in the source, yet
    // the invocation is still preserved in `main`.
    assert_invocation_preserved(&project, "macro_output", "expression_macro");
}

// --- macros.AC1.6: unterminated-macro / stray-end error cases --------------

#[test]
fn unterminated_macro_is_eof_inside_macro_error() {
    // A `:MACRO:` block with no `:END OF MACRO:` must report a clear parse
    // error: ConvertError::Reader(ReaderError::EofInsideMacro), Display
    // exactly "reader error: unexpected end of file inside macro".
    let mdl = ":MACRO: BAD(x)
BAD = x * 2
~ ~|
y = 1
~ ~|
\\\\\\---///
";
    let err = convert_mdl(mdl).expect_err("an unterminated :MACRO: must fail to convert");
    assert!(
        matches!(err, ConvertError::Reader(ReaderError::EofInsideMacro)),
        "expected ConvertError::Reader(ReaderError::EofInsideMacro), got {:?}",
        err
    );
    assert_eq!(
        err.to_string(),
        "reader error: unexpected end of file inside macro",
        "ConvertError Display must be the clear unterminated-macro message"
    );

    // Through the production entry point the message is prefixed.
    let prod_err = crate::compat::open_vensim(mdl)
        .expect_err("open_vensim must surface the unterminated-macro error");
    assert_eq!(
        prod_err.get_details().unwrap_or_default(),
        "Failed to parse MDL: reader error: unexpected end of file inside macro",
        "open_vensim must wrap the reader error with the \"Failed to parse MDL\" prefix"
    );
}

#[test]
fn stray_end_of_macro_is_unmatched_macro_end_error() {
    // A `:END OF MACRO:` with no opening `:MACRO:` must report
    // ConvertError::Reader(ReaderError::UnmatchedMacroEnd).
    let mdl = "x = 1
~ ~|
:END OF MACRO:
y = 2
~ ~|
\\\\\\---///
";
    let err = convert_mdl(mdl).expect_err("a stray :END OF MACRO: must fail to convert");
    assert!(
        matches!(err, ConvertError::Reader(ReaderError::UnmatchedMacroEnd)),
        "expected ConvertError::Reader(ReaderError::UnmatchedMacroEnd), got {:?}",
        err
    );
    assert_eq!(
        err.to_string(),
        "reader error: macro end without matching start",
        "ConvertError Display must be the clear stray-end message"
    );
}

// --- macros.AC3.1: multi-output invocation materialization -----------------
//
// A multi-output invocation `total = ADD3(in1, in2, in3 : the min, the max)`
// has no plain-text equivalent (a call returns several named values at once),
// so Phase 4 materializes it at MDL import as an explicit `Variable::Module`
// (input ports wired to the call arguments) plus one binding `Variable::Aux`
// per output: the LHS aux reads `<module>.<primary_output>`, and each
// `:`-list aux reads `<module>.<additional_output>`. The datamodel layer uses
// an ASCII period as the module-output separator (it canonicalizes to U+00B7
// only later, at compile-time parse -- see the authoritative Separator note
// in the phase plan).

const MACRO_MULTI_OUTPUT: &str = include_str!(
    "../../../../../test/test-models/tests/macro_multi_output/test_macro_multi_output.mdl"
);

/// Find the (single) `Variable::Module` in a model.
fn the_module(model: &Model) -> &crate::datamodel::Module {
    let modules: Vec<&crate::datamodel::Module> = model
        .variables
        .iter()
        .filter_map(|v| match v {
            Variable::Module(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(
        modules.len(),
        1,
        "expected exactly one Variable::Module in model {:?}; variables: {:?}",
        model.name,
        model
            .variables
            .iter()
            .map(|v| v.get_ident().to_string())
            .collect::<Vec<_>>()
    );
    modules[0]
}

#[test]
fn multi_output_invocation_materializes_module_and_binding_auxes() {
    // macros.AC3.1: the multi-output invocation materializes as a module
    // instance; the LHS (`total`) receives the primary output, and the
    // `:`-list names (`the min` / `the max`) become model variables holding
    // the additional outputs.
    let project = convert_mdl(MACRO_MULTI_OUTPUT).expect("macro_multi_output must import");

    // The ADD3 macro still imports as a macro-marked model with the
    // 2-additional-output `:`-list spec.
    let _m = assert_macro_shape(&project, "add3", &["a", "b", "c"], &["minval", "maxval"]);

    let main = main_model(&project);
    assert!(main.macro_spec.is_none(), "\"main\" is not macro-marked");

    // Exactly one Variable::Module, pointing at the ADD3 macro's model.
    let module = the_module(main);
    assert_eq!(
        module.model_name, "add3",
        "the materialized module must target the ADD3 macro's model"
    );

    // Its ident is the deterministic, serialization-stable `{lhs}_macro`
    // form (NOT the `$⁚` compile-time-synthetic prefix).
    assert_eq!(
        module.ident, "total_macro",
        "module ident must be the stable, human-readable `{{lhs}}_macro` form"
    );

    // Exactly three INPUT ModuleReferences, wiring in1/in2/in3 to the
    // a/b/c ports (dst is `<module>.<port>`, src is the argument's
    // canonical name). Outputs are NOT references -- they are realized as
    // separate binding auxes (compile time strips non-self-prefixed dst).
    let mut refs: Vec<(String, String)> = module
        .references
        .iter()
        .map(|r| (r.src.clone(), r.dst.clone()))
        .collect();
    refs.sort();
    assert_eq!(
        refs,
        vec![
            ("in1".to_string(), "total_macro.a".to_string()),
            ("in2".to_string(), "total_macro.b".to_string()),
            ("in3".to_string(), "total_macro.c".to_string()),
        ],
        "exactly the three input ports must be wired (no output references)"
    );

    // `total` is an Aux reading the module's PRIMARY output (ASCII period
    // -- the datamodel form).
    let total = var(main, "total");
    assert!(
        matches!(total, Variable::Aux(_)),
        "the LHS binding must be an Aux, got {:?}",
        total
    );
    assert_eq!(
        scalar_eq(total),
        "total_macro.add3",
        "`total` must read the module's primary output `<module>.add3`"
    );

    // `the min` / `the max` are Auxes reading the module's ADDITIONAL
    // outputs. The call-site name becomes the variable ident; the macro's
    // internal output name (minval/maxval) is what it reads.
    let the_min = var(main, "the_min");
    assert!(matches!(the_min, Variable::Aux(_)));
    assert_eq!(
        scalar_eq(the_min),
        "total_macro.minval",
        "`the min` must read `<module>.minval` (the macro-internal name)"
    );
    let the_max = var(main, "the_max");
    assert!(matches!(the_max, Variable::Aux(_)));
    assert_eq!(
        scalar_eq(the_max),
        "total_macro.maxval",
        "`the max` must read `<module>.maxval` (the macro-internal name)"
    );

    // The downstream equation that references the additional outputs is
    // untouched (proves the `:`-list names are ordinary referenceable
    // model variables).
    let spread = var(main, "spread");
    let spread_eq = scalar_eq(spread).to_lowercase();
    assert!(
        spread_eq.contains("the_max") && spread_eq.contains("the_min"),
        "`spread` must reference the bound additional-output variables, got {:?}",
        scalar_eq(spread)
    );

    // No multi-output `Expr::App` survived as plain text: the `total`
    // equation is the binding reference, not `add3(...)`.
    assert!(
        !scalar_eq(total).to_lowercase().contains("add3("),
        "the multi-output call must NOT survive as plain `add3(...)` text"
    );
}

/// Assert `convert_mdl(mdl)` fails with a `ConvertError` whose message names
/// the macro (`needle`).
fn assert_multi_output_convert_error_names(mdl: &str, needle: &str) {
    let err = convert_mdl(mdl).expect_err("a bad multi-output invocation must fail to convert");
    let msg = err.to_string();
    assert!(
        msg.to_lowercase().contains(&needle.to_lowercase()),
        "the ConvertError must name {:?}; got: {:?}",
        needle,
        msg
    );
}

#[test]
fn multi_output_call_to_unknown_macro_is_an_error_naming_it() {
    // A `:`-list call to a name that is not a known macro must fail with a
    // ConvertError naming the called name.
    let mdl = "total = NOPE(a, b : x, y)
~ ~|
a = 1
~ ~|
b = 2
~ ~|
\\\\\\---///
";
    assert_multi_output_convert_error_names(mdl, "nope");
}

#[test]
fn multi_output_call_with_wrong_argument_count_is_an_error() {
    // ADD3 has 3 parameters; calling it with 2 args (but the right number
    // of `:`-outputs) is an arity error naming ADD3.
    let mdl = ":MACRO: ADD3(a, b, c : minval, maxval)
ADD3 = a + b + c
~ ~|
minval = MIN(a, MIN(b, c))
~ ~|
maxval = MAX(a, MAX(b, c))
~ ~|
:END OF MACRO:
total = ADD3(p, q : lo, hi)
~ ~|
p = 1
~ ~|
q = 2
~ ~|
\\\\\\---///
";
    assert_multi_output_convert_error_names(mdl, "add3");
}

#[test]
fn multi_output_call_with_wrong_output_count_is_an_error() {
    // ADD3 declares 2 additional outputs; binding only 1 `:`-output (with
    // the right number of args) is an arity error naming ADD3.
    let mdl = ":MACRO: ADD3(a, b, c : minval, maxval)
ADD3 = a + b + c
~ ~|
minval = MIN(a, MIN(b, c))
~ ~|
maxval = MAX(a, MAX(b, c))
~ ~|
:END OF MACRO:
total = ADD3(p, q, r : lo)
~ ~|
p = 1
~ ~|
q = 2
~ ~|
r = 3
~ ~|
\\\\\\---///
";
    assert_multi_output_convert_error_names(mdl, "add3");
}

#[test]
fn nested_multi_output_call_is_an_error_naming_the_macro() {
    // The parser ACCEPTS the invalid nested form (`y = 1 + ADD3(p, q, r :
    // lo, hi)`) -- the inner `Expr::App` keeps its `output_bindings`. A
    // multi-output call may ONLY be the entire RHS of an equation: only the
    // whole-RHS form has a well-defined materialization (one Module + the
    // primary/additional binding auxes). The nested form has no such
    // materialization. Before the multi-output guard was added, this slipped
    // past `detect_multi_output_call` (which matches only a whole-RHS App)
    // and reached the XMILE formatter, where it PANICKED on the Phase-2
    // `debug_assert!(output_bindings.is_empty())` in a debug/test build, and
    // SILENTLY dropped the `:`-list outputs (`lo`/`hi`) in a release build.
    // The converter must instead reject it with a clean `ConvertError`
    // naming the macro and conveying the whole-RHS-only rule -- and must NOT
    // panic.
    let mdl = ":MACRO: ADD3(a, b, c : minval, maxval)
ADD3 = a + b + c
~ ~|
minval = MIN(a, MIN(b, c))
~ ~|
maxval = MAX(a, MAX(b, c))
~ ~|
:END OF MACRO:
y = 1 + ADD3(p, q, r : lo, hi)
~ ~|
p = 1
~ ~|
q = 2
~ ~|
r = 3
~ ~|
\\\\\\---///
";
    // `expect_err` (inside the helper) is itself the no-panic assertion: if
    // the nested form panicked (the old debug/test behavior), the test would
    // abort here instead of returning a clean Err.
    assert_multi_output_convert_error_names(mdl, "add3");

    // The diagnostic must convey the whole-RHS-only rule so the modeler
    // knows *why* it was rejected (not merely that something failed).
    let err = convert_mdl(mdl).expect_err("a nested multi-output call must fail to convert");
    let msg = err.to_string().to_lowercase();
    assert!(
        msg.contains("right-hand side") || msg.contains("right hand side"),
        "the ConvertError must explain the whole-right-hand-side-only rule; got: {:?}",
        err.to_string()
    );
}
