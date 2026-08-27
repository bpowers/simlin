// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, Expr0, Expr2, lower_ast};
use crate::common::{Canonical, EquationError, EquationResult, Ident};
use crate::dimensions::DimensionsContext;
use crate::variable::{ModuleInput, VarKind, Variable};
use crate::{datamodel, eqn_err};

#[cfg(test)]
use {
    crate::datamodel::Dimension,
    crate::units::Context,
    crate::variable::{ParseContext, parse_var},
};

#[cfg(test)]
use crate::testutils::{x_aux, x_flow, x_model, x_module, x_stock};

pub type VariableStage0 = Variable<datamodel::ModuleReference, Expr0>;

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

/// ModelStage0 converts a datamodel::Model to one with a map of canonicalized
/// identifiers to Variables where module dependencies haven't been resolved.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ModelStage0 {
    pub ident: Ident<Canonical>,
    pub display_name: String,
    pub variables: HashMap<Ident<Canonical>, VariableStage0>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
    /// is_macro is true if this model is a macro definition. A macro is a
    /// polymorphic template: its body variables' declared units may name the
    /// macro's formal parameters (a Vensim idiom -- e.g. `~ xfrom` inside
    /// RAMP FROM TO), so unit inference must treat those as polymorphic rather
    /// than concrete base units.
    pub is_macro: bool,
    /// Canonical formal-parameter names when `is_macro` is true (empty
    /// otherwise). Lets unit inference recognize which identifiers in a macro
    /// body's unit declarations are parameters and lower them to the
    /// corresponding metavariables (GH #619).
    pub macro_params: Vec<Ident<Canonical>>,
}

/// A model's variables lowered to `Expr2`: a [`ModelStage0`] resolved against
/// the project's dimension context and the module inputs of the models in its
/// lowering scope (`db::stages::model_stage1`). Unit inference and checking
/// read it; simulation compiles per variable and never builds one.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ModelStage1 {
    pub name: Ident<Canonical>,
    pub display_name: String,
    pub variables: HashMap<Ident<Canonical>, Variable>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
    /// is_macro is true if this model is a macro definition; see
    /// `ModelStage0::is_macro`. Inference treats a macro body's declared units
    /// as polymorphic rather than concrete base units.
    pub is_macro: bool,
    /// Canonical formal-parameter names when `is_macro` is true (empty
    /// otherwise); see `ModelStage0::macro_params` (GH #619).
    pub macro_params: Vec<Ident<Canonical>>,
}

fn resolve_relative<'a>(
    models: &'a HashMap<Ident<Canonical>, &'a ModelStage0>,
    model_name: &str,
    ident: &str,
) -> Option<&'a VariableStage0> {
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

/// lower_variable takes a stage 0 variable and turns it into a stage 1 variable.
/// This involves resolving both module inputs and dimension indexes.
///
/// Everything but `kind` carries over unchanged, so this is a map over the
/// kind: lower the ASTs a `Stock`/`Aux` holds, resolve the input wiring a
/// `Module` holds, and append whatever each raised to the variable's error
/// channel.
pub(crate) fn lower_variable(scope: &ScopeStage0, var_s0: &VariableStage0) -> Variable {
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

// parent_module_name is the name of the model that has the module instantiation,
// _not_ the name of the model this module instantiates
pub(crate) fn resolve_module_input<'a>(
    models: &HashMap<Ident<Canonical>, &ModelStage0>,
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

#[cfg(test)]
impl ModelStage0 {
    /// Stage a model that stands alone, resolving module-function calls against
    /// its OWN macro definitions only.
    ///
    /// Correct for the many single-model test fixtures, and for a macro body
    /// staged in isolation (its own `macro_spec` is in the registry, so a
    /// self-call resolves). NOT correct for a model that CALLS a macro defined
    /// in a sibling model -- use [`ModelStage0::new_in_project`] there.
    pub fn new(
        x_model: &datamodel::Model,
        dimensions: &[Dimension],
        units_ctx: &Context,
        implicit: bool,
    ) -> Self {
        Self::new_in_project(
            std::slice::from_ref(x_model),
            x_model,
            dimensions,
            units_ctx,
            implicit,
        )
    }

    /// The datamodel-driven Stage0 constructor: no salsa database, everything
    /// derived from `x_model` plus the project's macro definitions.
    ///
    /// This is the independent twin of the salsa-native
    /// `db::stages::model_stage0`: both share `parse_var`, while this path
    /// derives the macro registry and duplicate-ident errors without salsa.
    ///
    /// `project_models` is the whole project's model list, only so that the
    /// `MacroRegistry` matches the project-wide one `db::macro_registry`'s query
    /// builds. Passing just `x_model` (what the [`ModelStage0::new`] wrapper
    /// does) leaves a caller of a sibling-defined macro unclassified and its
    /// call unexpanded, which silently makes the oracle disagree with the query
    /// for reasons that have nothing to do with the code under test.
    pub fn new_in_project(
        project_models: &[datamodel::Model],
        x_model: &datamodel::Model,
        dimensions: &[Dimension],
        units_ctx: &Context,
        implicit: bool,
    ) -> Self {
        let mut implicit_vars: Vec<crate::capture::ImplicitVar> = Vec::new();

        let macro_registry = crate::module_functions::MacroRegistry::build(project_models)
            .expect("test fixture macro set must be valid");

        // #554: a macro-marked model's body variables get the model name as
        // `enclosing_model` so a renamed `init`/`previous` builtin inside the
        // like-named macro resolves to the intrinsic, not the macro.
        let enclosing_model: Option<&str> =
            x_model.macro_spec.as_ref().map(|_| x_model.name.as_str());
        // One context for the whole model: `parse_var*` needs the canonical
        // form, and building it per variable is what the salsa path stopped
        // doing (it reads the cached `project_dimensions_context` instead).
        let dimensions_ctx = DimensionsContext::from(dimensions);
        let ctx = ParseContext {
            dimensions: &dimensions_ctx,
            units_ctx,
            macro_registry: Some(&macro_registry),
            enclosing_model,
        };
        let mut variable_list: Vec<VariableStage0> = x_model
            .variables
            .iter()
            .map(|v| parse_var(&ctx, v, &mut implicit_vars, |mi| Ok(Some(mi.clone()))))
            .collect();

        variable_list.extend(
            implicit_vars
                .into_iter()
                .map(|iv| iv.variable_stage0(&dimensions_ctx)),
        );

        let variables: HashMap<Ident<Canonical>, _> = variable_list
            .into_iter()
            .map(|v| (Ident::new(v.ident()), v))
            .collect();

        Self {
            ident: Ident::new(&x_model.name),
            display_name: x_model.name.clone(),
            variables,
            implicit,
            is_macro: x_model.macro_spec.is_some(),
            macro_params: macro_param_idents(x_model.macro_spec.as_ref()),
        }
    }
}

pub(crate) struct ScopeStage0<'a> {
    pub models: &'a HashMap<Ident<Canonical>, &'a ModelStage0>,
    pub dimensions: &'a DimensionsContext,
    pub model_name: &'a str,
}

impl ModelStage1 {
    pub(crate) fn new(scope: &ScopeStage0, model_s0: &ModelStage0) -> Self {
        // Create a new scope with the model name for this specific model
        let model_scope = ScopeStage0 {
            models: scope.models,
            dimensions: scope.dimensions,
            model_name: model_s0.ident.as_str(),
        };

        ModelStage1 {
            name: model_s0.ident.clone(),
            display_name: model_s0.display_name.clone(),
            variables: model_s0
                .variables
                .iter()
                .map(|(ident, v)| (ident.clone(), lower_variable(&model_scope, v)))
                .collect(),
            implicit: model_s0.implicit,
            is_macro: model_s0.is_macro,
            macro_params: model_s0.macro_params.clone(),
        }
    }
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

    let mut implicit_vars: Vec<crate::capture::ImplicitVar> = Vec::new();
    let units_ctx = crate::units::Context::new(&[], &Default::default()).0;

    let owned_models: HashMap<Ident<Canonical>, ModelStage0> = vec![
        ("main".to_string(), &main_model),
        ("lynxes".to_string(), &lynxes_model),
        ("hares".to_string(), &hares_model),
    ]
    .into_iter()
    .map(|(name, m)| {
        (
            Ident::new(&name),
            ModelStage0::new(m, &[], &units_ctx, false),
        )
    })
    .collect();
    let models: HashMap<Ident<Canonical>, &ModelStage0> =
        owned_models.iter().map(|(k, v)| (k.clone(), v)).collect();

    let hares_var = &main_model.variables[2];
    assert_eq!("hares", hares_var.get_ident());

    let dims_ctx = DimensionsContext::default();
    let ctx = ParseContext::new(&dims_ctx, &units_ctx);
    let actual = parse_var(&ctx, hares_var, &mut implicit_vars, |mi| {
        resolve_module_input(&models, "main", hares_var.get_ident(), &mi.src, &mi.dst)
    });
    assert!(actual.equation_errors().is_none());
    assert!(implicit_vars.is_empty());
    assert_eq!(expected, actual);
}

/// `lower_variable` carries EVERY field of a Stage0 variable into its Stage1
/// twin, for every `VarKind`.
///
/// Lowering rewrites exactly two things -- a `Stock`/`Aux` AST from `Expr0` to
/// `Expr2`, and a `Module`'s input wiring from `datamodel::ModuleReference`s to
/// resolved `ModuleInput`s -- and must pass everything else through untouched.
/// The old shape restated nine to eleven field names per variant, and a field
/// dropped from one arm would have been invisible; this pins the pass-through
/// per kind.
///
/// The rows ARE the `VarKind` enumeration: one variable of each kind, plus both
/// `Aux` sub-shapes (flow and non-flow), driven through the production parse
/// (`ModelStage0::new_in_project`) and the production lowering
/// (`ModelStage1::new`). Both destructurings below are exhaustive -- no `..` --
/// so a new field on any variant fails to compile here until it is either
/// asserted or explicitly excused.
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
            x_module("sub", &[("rate", "sub.port")], None),
        ],
    );

    let units_ctx = Context::new(&[], &Default::default()).0;
    let project_models = vec![main_model.clone(), sub_model.clone()];
    let owned_s0: HashMap<Ident<Canonical>, ModelStage0> = project_models
        .iter()
        .map(|m| {
            (
                Ident::new(&m.name),
                ModelStage0::new_in_project(&project_models, m, &[], &units_ctx, false),
            )
        })
        .collect();
    let models: HashMap<Ident<Canonical>, &ModelStage0> =
        owned_s0.iter().map(|(k, v)| (k.clone(), v)).collect();
    let dimensions_ctx = DimensionsContext::default();
    let scope = ScopeStage0 {
        models: &models,
        dimensions: &dimensions_ctx,
        model_name: "main",
    };
    let s0 = &owned_s0[&Ident::<Canonical>::new("main")];
    let s1 = ModelStage1::new(&scope, s0);

    // Every kind of the enumeration must actually be exercised; a fixture that
    // silently stopped producing one would otherwise make this test vacuous.
    let mut seen_stock = false;
    let mut seen_flow = false;
    let mut seen_aux = false;
    let mut seen_module = false;

    for (ident, parsed) in s0.variables.iter() {
        let lowered = s1
            .variables
            .get(ident)
            .unwrap_or_else(|| panic!("{ident} was dropped by lowering"));

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
                // Stage0 form is a `datamodel::ModuleReference` and the Stage1
                // form a `(src, dst)` pair of canonical idents. What must not
                // change is that every reference produces exactly one input.
                assert_eq!(p_inputs.len(), l_inputs.len(), "{ident}: input count");
                let wiring: Vec<(&str, &str)> = l_inputs
                    .iter()
                    .map(|mi| (mi.src.as_str(), mi.dst.as_str()))
                    .collect();
                // `resolve_module_input` strips the instance prefix from the
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

#[test]
fn test_previous_module_input_declaration_does_not_change_parse_shape() {
    let units_ctx = Context::new(&[], &Default::default()).0;
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
    let model = x_model(
        "main",
        vec![module_input, x_aux("lagged", "PREVIOUS(input)", None)],
    );
    let parsed = ModelStage0::new(&model, &[], &units_ctx, false);
    assert!(
        !parsed
            .variables
            .keys()
            .any(|ident| ident.as_str().starts_with("$⁚lagged⁚0⁚arg0")),
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

// Salsa stores any `'static` value as a tracked-fn `returns(ref)` result and
// backdates its memo by `PartialEq`, so `ModelStage0`/`ModelStage1`/`Error`
// need no opt-in beyond the `PartialEq` they derive. A lowered
// `compiler::Expr` is equally cacheable: it references variables by NAME
// (`compiler::VarRef`), carries no offsets, and addresses are assigned exactly
// once at assembly by `symbolic::resolve_module` -- so caching one across a
// layout change is sound.
