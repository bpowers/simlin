// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The one `Expr0 -> Expr2` lowering of a parsed variable, [`lower_variable`].
//!
//! Its callers are the per-variable memos `db::lowered_source_variable` and
//! `db::lowered_implicit_variable` (each variable lowered once, under the
//! shapes its equation references), the LTM lowering of a generated equation
//! or capture (`db::ltm::compile::lower_ltm_variable`, through
//! [`lower_variable_from_typed`] with the tree it classified), and the unit
//! check's transient conveyor parameters. No whole-model lowered copy exists:
//! a consumer that needs several variables assembles handles to the memos
//! (`db::model_lowered_variables`).

use crate::ast::{Ast, Expr0, Expr1, Expr2, LoweringScope, lower_ast, lower_typed_ast, typed_ast};
use crate::canonicalize;
use crate::common::EquationResult;
use crate::datamodel;
use crate::db::{build_module_inputs, module_input_prefix};
use crate::diagnostic::DiagnosticError;
use crate::variable::{VarKind, Variable};

/// A variable as the parse leaves it: `Expr0` equations and a module's
/// input wiring still as the datamodel's references.
pub type ParsedVariable = Variable<datamodel::ModuleReference, Expr0>;

/// Lower a parsed variable to its `Expr2` form under `scope`.
///
/// Everything but `kind` carries over unchanged, so this is a map over the
/// kind: lower the ASTs a `Stock`/`Aux` holds against the scope's dimension
/// context and dependency shapes, appending whatever `lower_ast` raised to the
/// variable's diagnostics, and resolve the input wiring a `Module` holds
/// through `db::build_module_inputs`, the one owner of that wiring. Total: a
/// variable whose equation does not lower keeps its diagnostics and loses its
/// AST, and the caller decides what that means.
pub(crate) fn lower_variable(scope: &LoweringScope, parsed: &ParsedVariable) -> Variable {
    let primary = parsed.ast().map(|ast| typed_ast(ast, scope.dimensions));
    lower_variable_from_typed(scope, parsed, primary)
}

/// [`lower_variable`] with the variable's primary equation (`parsed.ast()`: a
/// stock's initial equation, an aux's dt equation) already at the typed tier.
///
/// The one map over a variable's kind, for both entry points. A caller that
/// typed the primary equation to classify its reads before the shapes it
/// lowers under existed (`db::ltm::compile::lower_ltm_variable`) hands that
/// tree in rather than typing the equation again; `primary` is that
/// equation's `typed_ast` result, or `None` for a variable without one. An
/// aux's initial equation is typed here.
pub(crate) fn lower_variable_from_typed(
    scope: &LoweringScope,
    parsed: &ParsedVariable,
    primary: Option<EquationResult<Ast<Expr1>>>,
) -> Variable {
    let mut diagnostics = parsed.diagnostics.clone();
    let element_scoped = parsed.element_scope().is_some();
    let mut record = |lowered: EquationResult<Ast<Expr2>>| -> Option<Ast<Expr2>> {
        match lowered {
            Ok(ast) => Some(ast),
            Err(err) => {
                diagnostics.push(DiagnosticError::Equation(err));
                None
            }
        }
    };
    // The primary equation first, then an aux's initial equation, so the
    // diagnostics keep equation order.
    let primary = primary.and_then(|typed| {
        record(typed.and_then(|typed| lower_typed_ast(scope, typed, element_scoped)))
    });
    let mut lower_initial = |ast: &Option<Ast<Expr0>>| -> Option<Ast<Expr2>> {
        ast.as_ref()
            .and_then(|ast| record(lower_ast(scope, ast, element_scoped)))
    };

    let kind = match &parsed.kind {
        VarKind::Stock {
            init_ast: _,
            inflows,
            outflows,
            non_negative,
        } => VarKind::Stock {
            init_ast: primary,
            inflows: inflows.clone(),
            outflows: outflows.clone(),
            non_negative: *non_negative,
        },
        VarKind::Aux {
            ast: _,
            init_ast,
            tables,
            non_negative,
            is_flow,
            is_table_only,
            element_scope,
        } => VarKind::Aux {
            ast: primary,
            init_ast: lower_initial(init_ast),
            tables: tables.clone(),
            non_negative: *non_negative,
            is_flow: *is_flow,
            is_table_only: *is_table_only,
            element_scope: element_scope.clone(),
        },
        VarKind::Module { model_name, inputs } => VarKind::Module {
            model_name: model_name.clone(),
            inputs: build_module_inputs(
                scope.model_name,
                &module_input_prefix(parsed.ident.as_str()),
                inputs
                    .iter()
                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
            ),
        },
    };

    Variable {
        ident: parsed.ident.clone(),
        units: parsed.units.clone(),
        eqn: parsed.eqn.clone(),
        diagnostics,
        kind,
    }
}

/// `lower_variable` carries EVERY field of a parsed variable into its lowered
/// twin, for every `VarKind`.
///
/// Lowering rewrites exactly two things -- a `Stock`/`Aux` AST from `Expr0` to
/// `Expr2`, and a `Module`'s input wiring from `datamodel::ModuleReference`s to
/// resolved `ModuleInput`s -- and must pass everything else through untouched.
/// A field dropped from one arm would otherwise be invisible; this pins the
/// pass-through per kind.
///
/// The rows ARE the `VarKind` enumeration: one variable of each kind, plus both
/// `Aux` sub-shapes (flow and non-flow), read through the production parse
/// memo and the production lowering memo. Both destructurings below are
/// exhaustive -- no `..` -- so a new field on any variant fails to compile here
/// until it is either asserted or explicitly excused.
#[test]
fn lower_variable_preserves_every_field_of_every_kind() {
    use crate::db::{
        SimlinDb, lowered_source_variable, parse_source_variable, sync_from_datamodel,
    };
    use crate::testutils::{
        sim_specs_with_units, x_aux, x_flow, x_model, x_module, x_project, x_stock,
    };

    let sub_model = x_model(
        "sub",
        vec![x_aux("port", "1", None), x_aux("out", "2", None)],
    );
    let main_model = x_model(
        "main",
        vec![
            x_stock("level", "7", &["fill"], &["drain"], Some("widgets")),
            x_flow("fill", "1", Some("widgets/time")),
            x_flow("drain", "level * 0.1", None),
            x_aux("rate", "level / 2", Some("widgets")),
            x_module("sub", &[("rate", "sub.port")], Some("widgets")),
        ],
    );
    let project = x_project(sim_specs_with_units("month"), &[main_model, sub_model]);
    let db = SimlinDb::default();
    let sync = sync_from_datamodel(&db, &project);
    let main = sync.models["main"].source;

    // Every kind of the enumeration must actually be exercised; a fixture that
    // silently stopped producing one would otherwise make this test vacuous.
    let mut seen_stock = false;
    let mut seen_flow = false;
    let mut seen_aux = false;
    let mut seen_module = false;

    for (ident, synced) in &sync.models["main"].variables {
        let parsed = &parse_source_variable(&db, synced.source, sync.project).variable;
        let lowered = &lowered_source_variable(&db, synced.source, main, sync.project).variable;

        // The four kind-independent fields pass through verbatim.
        assert_eq!(parsed.ident, lowered.ident, "{ident}: ident");
        assert_eq!(parsed.units, lowered.units, "{ident}: units");
        assert_eq!(parsed.eqn, lowered.eqn, "{ident}: eqn");
        assert_eq!(
            parsed.diagnostics, lowered.diagnostics,
            "{ident}: diagnostics (this fixture raises none, so lowering must add none)"
        );

        match (&parsed.kind, &lowered.kind) {
            (
                VarKind::Stock {
                    init_ast: p_init,
                    inflows: p_in,
                    outflows: p_out,
                    non_negative: p_nn,
                },
                VarKind::Stock {
                    init_ast: l_init,
                    inflows: l_in,
                    outflows: l_out,
                    non_negative: l_nn,
                },
            ) => {
                seen_stock = true;
                assert_eq!(p_in, l_in, "{ident}: inflows");
                assert_eq!(p_out, l_out, "{ident}: outflows");
                assert_eq!(p_nn, l_nn, "{ident}: non_negative");
                // The AST changes tier, so only its presence is comparable.
                assert_eq!(p_init.is_some(), l_init.is_some(), "{ident}: init_ast");
                assert!(l_init.is_some(), "{ident}: the stock has an equation");
            }
            (
                VarKind::Aux {
                    ast: p_ast,
                    init_ast: p_init,
                    tables: p_tables,
                    non_negative: p_nn,
                    is_flow: p_flow,
                    is_table_only: p_table_only,
                    element_scope: p_scope,
                },
                VarKind::Aux {
                    ast: l_ast,
                    init_ast: l_init,
                    tables: l_tables,
                    non_negative: l_nn,
                    is_flow: l_flow,
                    is_table_only: l_table_only,
                    element_scope: l_scope,
                },
            ) => {
                if *l_flow {
                    seen_flow = true;
                } else {
                    seen_aux = true;
                }
                assert_eq!(p_tables, l_tables, "{ident}: tables");
                assert_eq!(p_nn, l_nn, "{ident}: non_negative");
                assert_eq!(p_flow, l_flow, "{ident}: is_flow");
                assert_eq!(p_table_only, l_table_only, "{ident}: is_table_only");
                assert_eq!(p_scope, l_scope, "{ident}: element_scope");
                assert_eq!(p_ast.is_some(), l_ast.is_some(), "{ident}: ast");
                assert_eq!(p_init.is_some(), l_init.is_some(), "{ident}: init_ast");
                assert!(l_ast.is_some(), "{ident}: the aux/flow has an equation");
            }
            (
                VarKind::Module {
                    model_name: p_model,
                    inputs: p_inputs,
                },
                VarKind::Module {
                    model_name: l_model,
                    inputs: l_inputs,
                },
            ) => {
                seen_module = true;
                assert_eq!(p_model, l_model, "{ident}: model_name");
                assert!(
                    lowered.units.is_some(),
                    "{ident}: a module's declared units survive lowering"
                );
                // Inputs are RESOLVED by lowering, not passed through: the
                // parsed form is a `datamodel::ModuleReference` and the lowered
                // form a `(src, dst)` pair of canonical idents. What must not
                // change is that every reference produces exactly one input.
                assert_eq!(p_inputs.len(), l_inputs.len(), "{ident}: input count");
                let wiring: Vec<(&str, &str)> = l_inputs
                    .iter()
                    .map(|mi| (mi.src.as_str(), mi.dst.as_str()))
                    .collect();
                // `build_module_inputs` strips the instance prefix from the
                // destination, so `sub.port` binds the sub-model's own `port`.
                assert_eq!(
                    wiring,
                    vec![("rate", "port")],
                    "{ident}: resolved module wiring"
                );
            }
            _ => panic!("{ident}: lowering changed the variable's kind"),
        }
    }

    assert!(
        seen_stock && seen_flow && seen_aux && seen_module,
        "the fixture must exercise every VarKind (stock {seen_stock}, flow {seen_flow}, \
         aux {seen_aux}, module {seen_module})"
    );
}

/// A reference to an undefined variable is refused by the production dependency
/// gate, as an `UnknownDependency` attributed to the referencing variable.
#[test]
fn unknown_dependency_is_attributed_to_the_referencing_variable() {
    use crate::common::ErrorCode;
    use crate::test_common::TestProject;

    let errs = TestProject::new("main")
        .aux("aux_3", "unknown_variable * 3.14", None)
        .error_diagnostics();
    assert!(
        errs.iter()
            .any(|(loc, code)| loc == "main.aux_3" && *code == ErrorCode::UnknownDependency),
        "expected a main.aux_3 UnknownDependency, got: {errs:?}"
    );
}

#[test]
fn test_init_aux_only_array_subscript() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("init_aux_only_array_subscript")
        .with_sim_time(1.0, 5.0, 1.0)
        .named_dimension("DimA", &["a1", "a2"])
        .array_with_ranges(
            "growing[DimA]",
            vec![("a1", "TIME * 2"), ("a2", "TIME * 3")],
        )
        .array_aux("frozen[DimA]", "INIT(growing[DimA])");

    let vm = tp.run_vm().expect("VM should run");
    let frozen_a1 = vm.get("frozen[a1]").expect("frozen[a1] not in results");
    let frozen_a2 = vm.get("frozen[a2]").expect("frozen[a2] not in results");

    for (step, val) in frozen_a1.iter().enumerate() {
        assert!(
            (val - 2.0).abs() < 1e-10,
            "frozen[a1] should be 2.0 at every step, got {val} at step {step}"
        );
    }
    for (step, val) in frozen_a2.iter().enumerate() {
        assert!(
            (val - 3.0).abs() < 1e-10,
            "frozen[a2] should be 3.0 at every step, got {val} at step {step}"
        );
    }
}

#[test]
fn test_init_expression_vm() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("init_expr_parity")
        .with_sim_time(1.0, 5.0, 1.0)
        .aux("growing", "TIME * 2", None)
        .aux("frozen_expr", "INIT(growing + 1)", None);

    let vm = tp.run_vm().expect("VM should run successfully");

    let vm_vals = vm
        .get("frozen_expr")
        .expect("frozen_expr not in VM results");

    // TIME starts at 1.0, so growing+1 starts at 3.0 and INIT should
    // preserve that value for all timesteps.
    for (step, val) in vm_vals.iter().enumerate() {
        assert!(
            (val - 3.0).abs() < 1e-10,
            "frozen_expr should be 3.0 at every step, got {val} at step {step}"
        );
    }
}
