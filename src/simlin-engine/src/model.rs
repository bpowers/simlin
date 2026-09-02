// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, Expr0, Expr2, LoweringScope, lower_ast};
use crate::canonicalize;
use crate::common::{Canonical, Ident, IdentMap};
use crate::compiler::fragment::DepShape;
use crate::datamodel;
use crate::db::{build_module_inputs, module_input_prefix};
use crate::dimensions::DimensionsContext;
use crate::variable::{VarKind, Variable};

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

/// A model's variables lowered to `Expr2`: a [`ModelStage0`] lowered against
/// the project's dimension context and its own variables' shapes
/// (`db::stages::model_stage1`). Unit inference and checking read it;
/// simulation compiles per variable and never builds one.
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

/// Lower a parsed variable to its `Expr2` form under `scope`.
///
/// Everything but `kind` carries over unchanged, so this is a map over the
/// kind: lower the ASTs a `Stock`/`Aux` holds against the scope's dimension
/// context and dependency shapes, appending whatever `lower_ast` raised to the
/// variable's error channel, and resolve the input wiring a `Module` holds
/// through `db::build_module_inputs`, the one owner of that wiring. Total: a
/// variable whose equation does not lower keeps its errors and loses its AST,
/// and the caller decides what that means.
pub(crate) fn lower_variable(scope: &LoweringScope, var_s0: &VariableStage0) -> Variable {
    let mut errors = var_s0.errors.clone();
    let element_scoped = var_s0.element_scope().is_some();
    let mut lower = |ast: &Option<Ast<Expr0>>| -> Option<Ast<Expr2>> {
        ast.as_ref()
            .and_then(|ast| match lower_ast(scope, ast, element_scoped) {
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
            element_scope,
        } => VarKind::Aux {
            ast: lower(ast),
            init_ast: lower(init_ast),
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
                &module_input_prefix(var_s0.ident.as_str()),
                inputs
                    .iter()
                    .map(|mr| (canonicalize(&mr.src), canonicalize(&mr.dst))),
            ),
        },
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

impl ModelStage0 {
    /// The shape of every variable of the model, by name: the `LoweringScope`
    /// its own equations lower under. A module instance has no dimensions and
    /// the `Expr2` tier asks nothing else of a shape, so instances are left
    /// out rather than given a sub-model layout this stage does not have.
    pub(crate) fn lowering_shapes(&self) -> IdentMap<Ident<Canonical>, DepShape> {
        self.variables
            .iter()
            .filter(|(_, var)| !var.is_module())
            .map(|(ident, var)| {
                let dims = var
                    .get_dimensions()
                    .map(<[crate::dimensions::Dimension]>::to_vec)
                    .unwrap_or_default();
                (ident.clone(), DepShape::var(dims))
            })
            .collect()
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
    /// `db::stages::model_stage0` -- the two share `parse_var` but derive the
    /// macro registry, the enclosing-macro fact and the duplicate-ident errors
    /// along completely different routes -- which is what makes it a real
    /// oracle for that query rather than a restatement of it.
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

        // A build error here is a test-fixture bug -- surface it loudly.
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
            // No owning model to ask: a `PREVIOUS`/`INIT` element index
            // captures here where the salsa parse reads the referenced axis,
            // so a fixture holding one is compared against production
            // values, never against this oracle.
            snapshot_index: crate::builtins_visitor::SnapshotIndexFacts::NoModel,
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
                .iter()
                .map(|iv| iv.parsed_variable(&dimensions_ctx)),
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

impl ModelStage1 {
    /// Lower every variable of `model_s0` under the model's own shapes
    /// ([`ModelStage0::lowering_shapes`]) and the project's `dimensions`.
    pub(crate) fn new(dimensions: &DimensionsContext, model_s0: &ModelStage0) -> Self {
        let shapes = model_s0.lowering_shapes();
        let scope = LoweringScope {
            dimensions,
            shapes: &shapes,
            model_name: model_s0.ident.as_str(),
        };

        ModelStage1 {
            name: model_s0.ident.clone(),
            display_name: model_s0.display_name.clone(),
            variables: model_s0
                .variables
                .iter()
                .map(|(ident, v)| (ident.clone(), lower_variable(&scope, v)))
                .collect(),
            implicit: model_s0.implicit,
            is_macro: model_s0.is_macro,
            macro_params: model_s0.macro_params.clone(),
        }
    }
}

/// `lower_variable` carries EVERY field of a Stage0 variable into its Stage1
/// twin, for every `VarKind`.
///
/// Lowering rewrites exactly two things -- a `Stock`/`Aux` AST from `Expr0` to
/// `Expr2`, and a `Module`'s input wiring from `datamodel::ModuleReference`s to
/// resolved `ModuleInput`s -- and must pass everything else through untouched.
/// A field dropped from one arm would otherwise be invisible; this pins the
/// pass-through per kind.
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
    let s0 = ModelStage0::new_in_project(&project_models, &main_model, &[], &units_ctx, false);
    let s1 = ModelStage1::new(&DimensionsContext::default(), &s0);

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
                // Inputs are RESOLVED by lowering, not passed through: the
                // Stage0 form is a `datamodel::ModuleReference` and the Stage1
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

// Salsa stores any `'static` value as a tracked-fn `returns(ref)` result and
// backdates its memo by `PartialEq`, so `ModelStage0`/`ModelStage1`/`Error`
// need no opt-in beyond the `PartialEq` they derive. A lowered
// `compiler::Expr` is equally cacheable: it references variables by NAME
// (`compiler::VarRef`), carries no offsets, and addresses are assigned exactly
// once at assembly by `symbolic::resolve_module` -- so caching one across a
// layout change is sound.
