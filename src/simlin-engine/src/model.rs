// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::ast::{Ast, Expr0, Expr2, lower_ast};
use crate::common::{Canonical, EquationError, EquationResult, Ident};
use crate::dimensions::DimensionsContext;
use crate::variable::{ModuleInput, VarKind, Variable};
use crate::{datamodel, eqn_err};

#[cfg(test)]
use crate::testutils::{x_aux, x_flow, x_model, x_module, x_stock};

pub type ParsedVariable = Variable<datamodel::ModuleReference, Expr0>;

/// Canonical formal-parameter names of a macro (empty for a non-macro model).
/// Unit inference lowers a macro body's parameter-named unit declarations
/// (`~ xfrom`) to the parameters' metavariables so they resolve to the actual
/// argument units at each instantiation, rather than leaking the parameter
/// name as a literal base unit (GH #619).
pub(crate) fn macro_param_idents(
    macro_spec: Option<&datamodel::MacroSpec>,
) -> Vec<Ident<Canonical>> {
    macro_spec
        .map(|spec| spec.parameters.iter().map(|p| Ident::new(p)).collect())
        .unwrap_or_default()
}

/// A transient, dependency-granular set of parsed variables used while one
/// variable is lowered. It is never a salsa result and never owns a whole
/// source model.
pub(crate) struct LoweringModel<'a> {
    pub variables: HashMap<Ident<Canonical>, Cow<'a, ParsedVariable>>,
}

/// A stack-local unit-analysis view over memo-owned per-variable lowered data.
///
/// This type never crosses a tracked-query boundary: its map clones handles to
/// the one `Expr2` value retained for each variable, so model-wide inference
/// can walk a coherent module scope without creating another owned equation
/// tree.
pub(crate) struct UnitModel {
    pub name: Ident<Canonical>,
    pub variables: HashMap<Ident<Canonical>, std::sync::Arc<Variable>>,
    pub is_macro: bool,
    pub macro_params: Vec<Ident<Canonical>>,
}

fn resolve_relative<'a>(
    models: &'a HashMap<Ident<Canonical>, LoweringModel<'a>>,
    model_name: &str,
    ident: &str,
) -> Option<&'a ParsedVariable> {
    let ident = if model_name == "main" && ident.starts_with('·') {
        &ident['·'.len_utf8()..]
    } else {
        ident
    };
    let model = models.get(model_name)?;

    let input_prefix = format!("{model_name}·");
    // TODO: this is weird to do here and not before we call into this fn
    let ident = ident.strip_prefix(&input_prefix).unwrap_or(ident);

    // if the identifier is still dotted, its a further submodel reference
    // TODO: this will have to change when we break `module ident == model name`
    if let Some(pos) = ident.find('·') {
        let submodel_name = &ident[..pos];
        let submodel_var = &ident[pos + '·'.len_utf8()..];
        resolve_relative(models, submodel_name, submodel_var)
    } else {
        Some(model.variables.get(ident)?)
    }
}

/// Resolve one parsed variable's module inputs and dimension indexes.
///
/// Everything but `kind` carries over unchanged, so this is a map over the
/// kind: lower the ASTs a `Stock`/`Aux` holds, resolve the input wiring a
/// `Module` holds, and append whatever each raised to the variable's error
/// channel.
pub(crate) fn lower_variable(scope: &LoweringScope, var_s0: &ParsedVariable) -> Variable {
    let mut errors = var_s0.errors.clone();
    let mut lower = |ast: &Option<Ast<Expr0>>| -> Option<Ast<Expr2>> {
        ast.as_ref().and_then(|ast| match lower_ast(scope, ast) {
            Ok(ast) => Some(ast),
            Err(err) => {
                errors.push(err);
                None
            }
        })
    };

    let kind = match &var_s0.kind {
        VarKind::Stock {
            init_ast,
            inflows,
            outflows,
            non_negative,
        } => VarKind::Stock {
            init_ast: lower(init_ast),
            inflows: inflows.clone(),
            outflows: outflows.clone(),
            non_negative: *non_negative,
        },
        VarKind::Aux {
            ast,
            init_ast,
            tables,
            non_negative,
            is_flow,
            is_table_only,
        } => VarKind::Aux {
            ast: lower(ast),
            init_ast: lower(init_ast),
            tables: tables.clone(),
            non_negative: *non_negative,
            is_flow: *is_flow,
            is_table_only: *is_table_only,
        },
        VarKind::Module { model_name, inputs } => {
            let resolved = inputs.iter().map(|mi| {
                resolve_module_input(
                    scope.models,
                    scope.model_name,
                    var_s0.ident.as_str(),
                    mi.src.as_str(),
                    mi.dst.as_str(),
                )
            });

            let (inputs, input_errors): (Vec<_>, Vec<_>) =
                resolved.partition(EquationResult::is_ok);
            let inputs: Vec<ModuleInput> = inputs.into_iter().flat_map(|i| i.unwrap()).collect();
            // Wiring errors are prepended rather than appended. The order is
            // not observable in production: `parse_var`'s Module arm produces
            // errors only from its `module_input_mapper`, and every call site
            // passes the infallible `|mi| Ok(Some(mi.clone()))`, so a module
            // arrives here with no errors of its own. This is a stated
            // convention, not a load-bearing one.
            let mut module_errors: Vec<EquationError> =
                input_errors.into_iter().map(|e| e.unwrap_err()).collect();
            module_errors.append(&mut errors);
            errors = module_errors;

            VarKind::Module {
                model_name: model_name.clone(),
                inputs,
            }
        }
    };

    Variable {
        ident: var_s0.ident.clone(),
        units: var_s0.units.clone(),
        eqn: var_s0.eqn.clone(),
        errors,
        unit_errors: var_s0.unit_errors.clone(),
        kind,
    }
}

/// Resolve only a parsed module's input wiring, preserving every field shared
/// by all variable kinds.
///
/// Module references change representation from datamodel strings to
/// canonical [`ModuleInput`] pairs. Units and both error channels are parse
/// results and must pass through exactly like they do in [`lower_variable`].
pub(crate) fn resolve_parsed_module(
    parsed: &ParsedVariable,
    inputs: Vec<ModuleInput>,
) -> Option<Variable> {
    let VarKind::Module { model_name, .. } = &parsed.kind else {
        return None;
    };
    Some(Variable {
        ident: parsed.ident.clone(),
        units: parsed.units.clone(),
        eqn: parsed.eqn.clone(),
        errors: parsed.errors.clone(),
        unit_errors: parsed.unit_errors.clone(),
        kind: VarKind::Module {
            model_name: model_name.clone(),
            inputs,
        },
    })
}

// parent_module_name is the name of the model that has the module instantiation,
// _not_ the name of the model this module instantiates
pub(crate) fn resolve_module_input<'a>(
    models: &HashMap<Ident<Canonical>, LoweringModel<'_>>,
    parent_model_name: &str,
    ident: &str,
    orig_src: &'a str,
    orig_dst: &'a str,
) -> EquationResult<Option<ModuleInput>> {
    let input_prefix = format!("{ident}·");
    let maybe_strip_leading_dot = |s: &'a str| -> &'a str {
        if parent_model_name == "main" && s.starts_with('·') {
            &s['·'.len_utf8()..] // '·' is a 2 byte long unicode character
        } else {
            s
        }
    };
    let src = Ident::new(maybe_strip_leading_dot(orig_src));
    let dst = Ident::new(maybe_strip_leading_dot(orig_dst));

    // Stella has a bug where if you have one module feeding into another,
    // it writes identical tags to both.  So skip the tag that is non-local
    // but don't report it as an error
    if src.as_str().starts_with(&input_prefix) {
        return Ok(None);
    }

    let dst_stripped = dst.as_str().strip_prefix(&input_prefix);
    if dst_stripped.is_none() {
        return eqn_err!(
            BadModuleInputDst,
            0,
            0,
            format!("'{dst}' is not an input of the module being instantiated")
        );
    }
    let dst = Ident::new(dst_stripped.unwrap());

    // TODO: reevaluate if this is really the best option here
    // if the source is a temporary created by the engine, assume it is OK
    if src.as_str().starts_with("$⁚") {
        return Ok(Some(ModuleInput { src, dst }));
    }

    match resolve_relative(models, parent_model_name, src.as_str()) {
        Some(_) => Ok(Some(ModuleInput { src, dst })),
        None => eqn_err!(
            BadModuleInputSrc,
            0,
            0,
            format!("'{src}' is not a variable of model '{parent_model_name}'")
        ),
    }
}

pub(crate) struct LoweringScope<'a> {
    pub models: &'a HashMap<Ident<Canonical>, LoweringModel<'a>>,
    pub dimensions: &'a DimensionsContext,
    pub model_name: &'a str,
}

#[test]
fn test_module_parse() {
    use crate::variable::ModuleInput;
    let inputs: Vec<ModuleInput> = vec![
        ModuleInput {
            src: Ident::new("area"),
            dst: Ident::new("area"),
        },
        ModuleInput {
            src: Ident::new("lynxes·lynxes_stock"),
            dst: Ident::new("lynxes"),
        },
    ];
    let expected = Variable {
        ident: Ident::new("hares"),
        units: None,
        eqn: None,
        errors: vec![],
        unit_errors: vec![],
        kind: VarKind::Module {
            model_name: Ident::new("hares"),
            inputs,
        },
    };

    let lynxes_model = x_model(
        "lynxes",
        vec![
            x_aux("init", "5", None),
            x_stock("lynxes_stock", "100 * init", &["inflow"], &[], None),
            x_flow("inflow", "1", None),
        ],
    );
    let hares_model = x_model(
        "hares",
        vec![
            x_aux("lynxes", "0", None),
            x_stock("hares_stock", "100", &[], &["outflow"], None),
            x_flow("outflow", ".1 * hares_stock", None),
        ],
    );
    let main_model = x_model(
        "main",
        vec![
            x_aux("area", "time", None),
            x_module("lynxes", &[], None),
            x_module(
                "hares",
                &[
                    ("area", "hares.area"),
                    ("lynxes.lynxes_stock", "hares.lynxes"),
                ],
                None,
            ),
        ],
    );

    let project = datamodel::Project {
        name: "module_parse".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![main_model, lynxes_model, hares_model],
        source: None,
        ai_information: None,
    };
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project);
    let source = sync.models["main"].variables["hares"].source;
    let actual =
        crate::db::lowered_source_variable(&db, source, sync.models["main"].source, sync.project);
    assert!(actual.equation_errors().is_none());
    assert_eq!(&expected, actual.as_ref());
}

/// Per-variable lowering carries every kind-independent field into the
/// compiled projection, for every `VarKind`.
///
/// Lowering rewrites exactly two things -- a `Stock`/`Aux` AST from `Expr0` to
/// `Expr2`, and a `Module`'s input wiring from `datamodel::ModuleReference`s to
/// resolved `ModuleInput`s -- and must pass everything else through untouched.
/// The old shape restated nine to eleven field names per variant, and a field
/// dropped from one arm would have been invisible; this pins the pass-through
/// per kind.
///
/// The rows are the `VarKind` enumeration: one variable of each kind, plus both
/// `Aux` sub-shapes (flow and non-flow), driven through the production
/// per-variable parse and lowering queries. Both destructurings below are
/// exhaustive, so a new field fails to compile here until it is asserted.
#[test]
fn lower_variable_preserves_every_field_of_every_kind() {
    use crate::variable::VarKind;

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
            crate::testutils::x_module_named(
                "sub_with_units",
                "sub",
                &[("rate", "sub_with_units.port")],
                Some("widgets"),
            ),
            crate::testutils::x_module_named(
                "sub_bad_units",
                "sub",
                &[("rate", "sub_bad_units.port")],
                Some("bad units here!!!"),
            ),
        ],
    );

    let project = datamodel::Project {
        name: "lowering_field_preservation".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![main_model.clone(), sub_model],
        source: None,
        ai_information: None,
    };
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;

    // Every kind of the enumeration must actually be exercised; a fixture that
    // silently stopped producing one would otherwise make this test vacuous.
    let mut seen_stock = false;
    let mut seen_flow = false;
    let mut seen_aux = false;
    let mut seen_module = false;

    for source_variable in &main_model.variables {
        let ident = source_variable.get_ident();
        let source = sync.models["main"].variables[ident].source;
        let parsed_result = crate::db::parse_source_variable(&db, source, sync.project);
        let parsed = &parsed_result.variable;
        let lowered = crate::db::lowered_source_variable(&db, source, source_model, sync.project);

        // The five kind-independent fields pass through verbatim.
        assert_eq!(parsed.ident, lowered.ident, "{ident}: ident");
        assert_eq!(parsed.units, lowered.units, "{ident}: units");
        assert_eq!(parsed.eqn, lowered.eqn, "{ident}: eqn");
        assert_eq!(
            parsed.unit_errors, lowered.unit_errors,
            "{ident}: unit_errors"
        );
        assert_eq!(
            parsed.errors, lowered.errors,
            "{ident}: errors (this fixture raises none, so lowering must add none)"
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
                },
                VarKind::Aux {
                    ast: l_ast,
                    init_ast: l_init,
                    tables: l_tables,
                    non_negative: l_nn,
                    is_flow: l_flow,
                    is_table_only: l_table_only,
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
                // Inputs are RESOLVED by lowering, not passed through: the
                // Parsing retains `datamodel::ModuleReference`; lowering
                // resolves each one to a canonical `(src, dst)` pair.
                assert_eq!(p_inputs.len(), l_inputs.len(), "{ident}: input count");
                let wiring: Vec<(&str, &str)> = l_inputs
                    .iter()
                    .map(|mi| (mi.src.as_str(), mi.dst.as_str()))
                    .collect();
                // `resolve_module_input` strips the instance prefix from the
                // destination, so `sub.port` binds the sub-model's own `port`.
                assert_eq!(wiring, vec![("rate", "port")], "{ident}: wiring");
            }
            _ => panic!("{ident}: lowering changed the variable's kind"),
        }
    }

    assert!(
        seen_stock && seen_flow && seen_aux && seen_module,
        "the fixture must exercise every VarKind (stock {seen_stock}, flow {seen_flow}, \
         aux {seen_aux}, module {seen_module})"
    );
    let valid = sync.models["main"].variables["sub_with_units"].source;
    let valid = crate::db::lowered_source_variable(&db, valid, source_model, sync.project);
    assert!(valid.units.is_some(), "declared module units must survive");
    assert!(valid.unit_errors.is_empty());
    let malformed = sync.models["main"].variables["sub_bad_units"].source;
    let malformed = crate::db::lowered_source_variable(&db, malformed, source_model, sync.project);
    assert!(malformed.units.is_none());
    assert_eq!(
        malformed.unit_errors.len(),
        1,
        "the module's malformed units must survive lowering"
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

#[test]
fn test_previous_module_input_declaration_does_not_change_parse_shape() {
    let module_input = datamodel::Variable::Aux(datamodel::Aux {
        ident: "input".to_string(),
        equation: datamodel::Equation::Scalar("0".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat {
            can_be_module_input: true,
            ..datamodel::Compat::default()
        },
    });
    let main = x_model(
        "main",
        vec![module_input, x_aux("lagged", "PREVIOUS(input)", None)],
    );
    let project = datamodel::Project {
        name: "module_input_parse_shape".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![main],
        source: None,
        ai_information: None,
    };
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project);
    let parsed = crate::db::parse_source_variable(
        &db,
        sync.models["main"].variables["lagged"].source,
        sync.project,
    );
    assert!(
        !parsed
            .implicit_vars
            .iter()
            .any(|implicit| implicit.ident().starts_with("$⁚lagged⁚0⁚arg0")),
        "source parsing cannot depend on whether another model binds `input` as a module port"
    );
}

#[test]
fn test_model_implicit_var_info_does_not_capture_a_scalar_module_call_aux() {
    let project = datamodel::Project {
        name: "implicit_info_module_context".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_aux("x", "TIME", None),
                x_aux("delayed", "SMTH1(x, 99)", None),
                x_aux("prev_delayed", "PREVIOUS(delayed, 123)", None),
            ],
        )],
        source: None,
        ai_information: None,
    };
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project);
    let source_model = sync.models["main"].source;
    let implicit_info = crate::db::model_implicit_var_info(&db, source_model, sync.project);
    assert!(
        !implicit_info
            .keys()
            .any(|name| name.starts_with("$⁚prev_delayed⁚0⁚arg0")),
        "a scalar module-call aux has its own slot, so PREVIOUS can read it directly"
    );
}

#[test]
fn test_incremental_compile_previous_of_module_backed_var() {
    let project = datamodel::Project {
        name: "incremental_prev_module_backed".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![x_model(
            "main",
            vec![
                x_aux("x", "TIME", None),
                x_aux("delayed", "SMTH1(x, 99)", None),
                x_aux("prev_delayed", "PREVIOUS(delayed, 123)", None),
            ],
        )],
        source: None,
        ai_information: None,
    };
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project);
    let compiled = crate::db::compile_project_incremental(&db, sync.project, "main");
    assert!(
        compiled.is_ok(),
        "incremental compile should support PREVIOUS(module-backed var): {:?}",
        compiled.err()
    );
}

// A lowered `compiler::Expr` is cacheable across layout changes: it references
// variables by name (`compiler::VarRef`) and carries no offsets. Assembly
// assigns addresses exactly once through `symbolic::resolve_module`.
