// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;

use crate::ast::{Expr0, lower_ast};
use crate::common::{
    Canonical, EquationError, EquationResult, Error, ErrorCode, ErrorKind, Ident, Result,
    canonicalize,
};
use crate::dimensions::DimensionsContext;
use crate::variable::{ModuleInput, Variable, identifier_set};
use crate::{datamodel, eqn_err, model_err};

#[cfg(test)]
use {
    crate::datamodel::Dimension,
    crate::db,
    crate::units::Context,
    crate::variable::{parse_var, parse_var_with_module_context},
};

#[cfg(test)]
use crate::testutils::{x_aux, x_flow, x_model, x_module, x_stock};

pub type ModuleInputSet = BTreeSet<Ident<Canonical>>;

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
#[derive(Clone, PartialEq, salsa::Update)]
pub struct ModelStage0 {
    pub ident: Ident<Canonical>,
    pub display_name: String,
    pub variables: HashMap<Ident<Canonical>, VariableStage0>,
    /// Model-level errors recorded while building this stage: today only the
    /// duplicate-canonical-ident collision (GH #891), which the canonical-keyed
    /// `variables` map above would otherwise swallow last-wins.
    ///
    /// Read only by [`ModelStage1::new`], which copies it into
    /// [`ModelStage1::errors`] -- the monolithic path's simulatability gate.
    /// Production's own duplicate-ident diagnostic is a separate derivation
    /// (`db::model_duplicate_variables` -> `emit_duplicate_variable_diagnostics`)
    /// over the raw `declared_variable_idents` input, so the two are
    /// independent, and `db::stages_tests` compares this field against the
    /// salsa-free `ModelStage0::new_in_project` oracle.
    pub errors: Option<Vec<Error>>,
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

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct ModelStage1 {
    pub name: Ident<Canonical>,
    pub display_name: String,
    pub variables: HashMap<Ident<Canonical>, Variable>,
    /// The monolithic path's simulatability gate: `compiler::Module::new`
    /// refuses to build a module from a model with a non-empty list here.
    ///
    /// Filled by [`ModelStage1::set_dependencies`] from three sources -- the
    /// Stage0 duplicate-ident collision, the production dependency graph's
    /// `has_cycle` verdict, and a roll-up of the equation errors the variables
    /// themselves carry. It is NOT an alternative to the salsa diagnostics: it
    /// is deliberately coarser (a code, no location), it exists only on this
    /// test-only construction path, and everything that reports errors to a
    /// user goes through `db::collect_all_diagnostics` instead.
    pub errors: Option<Vec<Error>>,
    /// model_deps is the transitive set of model names referenced from modules in this model
    pub model_deps: Option<BTreeSet<Ident<Canonical>>>,
    pub instantiations: Option<HashMap<ModuleInputSet, ModuleStage2>>,
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

/// One module instantiation's evaluation order, as the monolithic
/// `compiler::Module` consumes it.
///
/// The three runlists are copied verbatim out of the production dependency
/// graph (`db::dep_graph::model_dependency_graph`); this type carries no
/// dependency analysis of its own. It used to also carry that graph's
/// `dt_dependencies` / `initial_dependencies` maps, whose only readers were the
/// second, now-deleted dependency walk that produced them.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct ModuleStage2 {
    pub model_ident: Ident<Canonical>,
    /// inputs is the set of variables overridden (provided as input) in this
    /// module instantiation.
    pub inputs: ModuleInputSet,
    pub runlist_initials: Vec<Ident<Canonical>>,
    pub runlist_flows: Vec<Ident<Canonical>>,
    pub runlist_stocks: Vec<Ident<Canonical>>,
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
pub(crate) fn lower_variable(scope: &ScopeStage0, var_s0: &VariableStage0) -> Variable {
    match var_s0 {
        Variable::Stock {
            ident,
            init_ast: ast,
            eqn,
            units,
            inflows,
            outflows,
            non_negative,
            errors,
            unit_errors,
        } => {
            let mut errors = errors.clone();
            let ast = ast
                .as_ref()
                .and_then(|ast| match lower_ast(scope, ast.clone()) {
                    Ok(ast) => Some(ast),
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                });
            Variable::Stock {
                ident: ident.clone(),
                init_ast: ast,
                eqn: eqn.clone(),
                units: units.clone(),
                inflows: inflows.clone(),
                outflows: outflows.clone(),
                non_negative: *non_negative,
                errors,
                unit_errors: unit_errors.clone(),
            }
        }
        Variable::Var {
            ident,
            ast,
            init_ast,
            eqn,
            units,
            tables,
            non_negative,
            is_flow,
            is_table_only,
            errors,
            unit_errors,
        } => {
            let mut errors = errors.clone();
            let ast = ast
                .as_ref()
                .and_then(|ast| match lower_ast(scope, ast.clone()) {
                    Ok(ast) => Some(ast),
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                });
            let init_ast = init_ast
                .as_ref()
                .and_then(|ast| match lower_ast(scope, ast.clone()) {
                    Ok(ast) => Some(ast),
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                });
            Variable::Var {
                ident: ident.clone(),
                ast,
                init_ast,
                eqn: eqn.clone(),
                units: units.clone(),
                tables: tables.clone(),
                non_negative: *non_negative,
                is_flow: *is_flow,
                is_table_only: *is_table_only,
                errors,
                unit_errors: unit_errors.clone(),
            }
        }
        Variable::Module {
            ident,
            model_name,
            units,
            inputs,
            errors,
            unit_errors,
        } => {
            let var_errors = errors;

            let inputs = inputs.iter().map(|mi| {
                resolve_module_input(
                    scope.models,
                    scope.model_name,
                    ident.as_str(),
                    mi.src.as_str(),
                    mi.dst.as_str(),
                )
            });

            let (inputs, errors): (Vec<_>, Vec<_>) = inputs.partition(EquationResult::is_ok);
            let inputs: Vec<ModuleInput> = inputs.into_iter().flat_map(|i| i.unwrap()).collect();
            let mut errors: Vec<EquationError> =
                errors.into_iter().map(|e| e.unwrap_err()).collect();
            errors.append(&mut var_errors.clone());

            Variable::Module {
                ident: ident.clone(),
                model_name: model_name.clone(),
                units: units.clone(),
                inputs,
                errors,
                unit_errors: unit_errors.clone(),
            }
        }
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
        return eqn_err!(BadModuleInputDst, 0, 0);
    }
    let dst = Ident::new(dst_stripped.unwrap());

    // TODO: reevaluate if this is really the best option here
    // if the source is a temporary created by the engine, assume it is OK
    if src.as_str().starts_with("$⁚") {
        return Ok(Some(ModuleInput { src, dst }));
    }

    match resolve_relative(models, parent_model_name, src.as_str()) {
        Some(_) => Ok(Some(ModuleInput { src, dst })),
        None => eqn_err!(BadModuleInputSrc, 0, 0),
    }
}

pub fn enumerate_modules<T>(
    models: &HashMap<&str, &ModelStage1>,
    main_model_name: &str,
    mapper: fn(&ModelStage1) -> T,
) -> Result<HashMap<T, BTreeSet<BTreeSet<Ident<Canonical>>>>>
where
    T: Eq + Hash,
{
    let mut modules = HashMap::new();
    // manually insert the main model (which has no dependencies)
    if let Some(main_model) = models.get(main_model_name) {
        let no_module_inputs = BTreeSet::new();
        modules.insert(
            mapper(main_model),
            [no_module_inputs].iter().cloned().collect(),
        );
    } else {
        return model_err!(BadModelName, main_model_name.to_owned());
    }

    enumerate_modules_inner(models, main_model_name, mapper, &mut modules)?;

    Ok(modules)
}

pub(crate) fn enumerate_modules_inner<T>(
    models: &HashMap<&str, &ModelStage1>,
    model_name: &str,
    mapper: fn(&ModelStage1) -> T,
    modules: &mut HashMap<T, BTreeSet<BTreeSet<Ident<Canonical>>>>,
) -> Result<()>
where
    T: Eq + Hash,
{
    let model = *models.get(model_name).ok_or_else(|| Error {
        kind: ErrorKind::Simulation,
        code: ErrorCode::NotSimulatable,
        details: Some(format!("model for module '{model_name}' not found")),
    })?;
    for (_id, v) in model.variables.iter() {
        if let Variable::Module {
            model_name, inputs, ..
        } = v
        {
            if let Some(model) = models.get(model_name.as_str()) {
                let inputs: BTreeSet<Ident<Canonical>> =
                    inputs.iter().map(|input| input.dst.clone()).collect();

                let key = mapper(model);
                let first_sighting = !modules.contains_key(&key);

                // Record this instantiation BEFORE descending into the model.
                // The `first_sighting` test is what stops the walk revisiting a
                // model, so a model that is still being walked has to count as
                // seen: otherwise two models that instantiate each other are
                // each unrecorded when the other looks, and the recursion
                // diverges into a stack overflow -- a process abort, not a
                // catchable panic. (A cycle THROUGH the main model happened to
                // terminate already, because `enumerate_modules` records main
                // up front.) Recording early cannot lose an instantiation: this
                // line runs at every module site regardless, and the values are
                // a set of input sets, so the order they arrive in is not
                // observable.
                modules.entry(key).or_default().insert(inputs);

                if first_sighting {
                    // first time we are seeing the model for this module.
                    // make sure all _its_ module instantiations are recorded
                    enumerate_modules_inner(models, model_name.as_str(), mapper, modules)?;
                }
            } else {
                return model_err!(BadModelName, model_name.as_str().to_string());
            }
        }
    }

    Ok(())
}

/// Scan a model's datamodel variables and return the set of identifiers
/// that will become module variables during compilation.
///
/// This includes:
/// - Explicit `datamodel::Variable::Module` variables
/// - `datamodel::Variable::Aux` and `datamodel::Variable::Flow` variables
///   whose equations parse to a top-level **module-function** call: a stdlib
///   function (`SMTH1`, `DELAY`, ...) or a project macro (consulted via the
///   passed `MacroRegistry`).
///
/// This set is needed so that `PREVIOUS(module_var)` rewrites through a
/// synthesized scalar helper aux instead of compiling `LoadPrev` directly
/// against a multi-slot module. A `y = MYMACRO(...)` caller must be
/// pre-classified here exactly as a `y = SMTH1(...)` caller is, so the
/// PREVIOUS/INIT rewrite sees it as module-backed.
pub(crate) fn collect_module_idents(
    variables: &[datamodel::Variable],
    macro_registry: &crate::module_functions::MacroRegistry,
) -> HashSet<Ident<Canonical>> {
    let mut module_idents = HashSet::new();
    for v in variables {
        if v.can_be_module_input() {
            module_idents.insert(Ident::new(&canonicalize(v.get_ident())));
        }
        match v {
            datamodel::Variable::Module(m) => {
                module_idents.insert(Ident::new(&canonicalize(&m.ident)));
            }
            datamodel::Variable::Aux(a) => {
                if equation_is_module_call(&a.equation, macro_registry) {
                    module_idents.insert(Ident::new(&canonicalize(&a.ident)));
                }
            }
            datamodel::Variable::Flow(f) => {
                if equation_is_module_call(&f.equation, macro_registry) {
                    module_idents.insert(Ident::new(&canonicalize(&f.ident)));
                }
            }
            datamodel::Variable::Stock(_) => {}
        }
    }
    module_idents
}

/// Check if a scalar equation's top-level expression is a **module-function**
/// call: a stdlib function (`is_stdlib_module_function`) or a project macro
/// (resolved via `macro_registry`). A project macro is recognized even when
/// its name shadows a builtin, since the macro registry is consulted
/// directly (the actual macro-shadows-builtin precedence is enforced later
/// in `BuiltinVisitor::walk`).
///
/// This intentionally re-parses the equation text rather than reusing the
/// already-parsed AST. It runs during `collect_module_idents` (called from
/// `ModelStage0::new_in_project` and the salsa `module_ident_context`
/// query), before the full per-variable parse in `parse_var`. The re-parse
/// is cheap (single equation, top-level only) and avoids threading the
/// parsed AST through an intermediate data structure just for this early
/// classification step.
///
/// Scope note: this inspects only `Equation::Scalar` and
/// `Equation::ApplyToAll`; it returns `false` for `Equation::Arrayed` (the
/// per-element-equation form). A per-element-equation macro call would not be
/// pre-classified here -- but that exactly matches the pre-existing behavior
/// for arrayed stdlib calls, so it is not a macro-specific regression.
pub(crate) fn equation_is_module_call(
    eqn: &datamodel::Equation,
    macro_registry: &crate::module_functions::MacroRegistry,
) -> bool {
    let text = match eqn {
        datamodel::Equation::Scalar(s) | datamodel::Equation::ApplyToAll(_, s) => s.as_str(),
        _ => return false,
    };
    let Ok(Some(ast)) = Expr0::new(text, crate::lexer::LexerType::Equation) else {
        return false;
    };
    match &ast {
        Expr0::App(crate::builtins::UntypedBuiltinFn(func, _args), _) => {
            let func_lower = func.to_lowercase();
            // Any resolvable project macro counts as a module call here,
            // including a genuine passthrough macro (`:MACRO: INIT(x) =
            // INITIAL(x)`) that `builtins_visitor::walk` later collapses to a
            // scalar opcode (#591-c1). Pre-classifying the passthrough caller
            // as module-backed is benign: the only downstream consumer of this
            // result is `is_module_backed_ident`, which gates whether a
            // *referencing* `PREVIOUS`/`INIT` synthesizes a scalar temp arg
            // (`builtins_visitor.rs`). Since a passthrough caller collapses to
            // a plain flat-slot variable, that temp-arg copy is value-identical
            // to reading the slot directly -- so the classification does not
            // change any observable result either way.
            let is_module_macro = macro_registry.resolve_macro(func).is_some();
            crate::builtins::is_stdlib_module_function(&func_lower) || is_module_macro
        }
        _ => false,
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
    /// `db::stages::model_stage0` -- the two share `parse_var_with_module_context`
    /// but derive the module-ident set, the macro registry and the
    /// duplicate-ident errors along completely different routes -- which is what
    /// makes it a real oracle for that query rather than a restatement of it.
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
        let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();

        // Determine which variable names should force PREVIOUS to synthesize
        // a scalar temp arg rather than reading a flat slot directly.
        //
        // For user models, only explicit Module variables and stdlib-call
        // Aux/Flow variables need temp-arg rewriting because they occupy
        // multiple slots and LoadPrev at the base offset reads the wrong
        // sub-variable.
        //
        // For implicit (stdlib) models, ALL variable names are included.
        // Inside a submodule, some variables are module inputs whose values
        // are passed from the parent via a transient array -- they have no
        // persistent slot in prev_values. PREVIOUS(module_input) must first
        // capture the current scalar into a temp helper so LoadPrev reads
        // that helper's slot on the next step.
        // A build error here is a test-fixture bug -- surface it loudly.
        let macro_registry = crate::module_functions::MacroRegistry::build(project_models)
            .expect("test fixture macro set must be valid");
        let module_idents: HashSet<Ident<Canonical>> = if implicit {
            x_model
                .variables
                .iter()
                .map(|v| Ident::new(&canonicalize(v.get_ident())))
                .collect()
        } else {
            collect_module_idents(&x_model.variables, &macro_registry)
        };

        // #554: a macro-marked model's body variables get the model name as
        // `enclosing_model` so a renamed `init`/`previous` builtin inside the
        // like-named macro resolves to the intrinsic, not the macro.
        let enclosing_model: Option<&str> =
            x_model.macro_spec.as_ref().map(|_| x_model.name.as_str());
        let mut variable_list: Vec<VariableStage0> = x_model
            .variables
            .iter()
            .map(|v| {
                parse_var_with_module_context(
                    dimensions,
                    v,
                    &mut implicit_vars,
                    units_ctx,
                    |mi| Ok(Some(mi.clone())),
                    Some(&module_idents),
                    None,
                    Some(&macro_registry),
                    enclosing_model,
                )
            })
            .collect();

        {
            // FIXME: this is an unfortunate API choice
            let mut dummy_implicit_vars: Vec<datamodel::Variable> = Vec::new();
            variable_list.extend(implicit_vars.into_iter().map(|x_var| {
                parse_var(
                    dimensions,
                    &x_var,
                    &mut dummy_implicit_vars,
                    units_ctx,
                    |mi| Ok(Some(mi.clone())),
                )
            }));
            assert_eq!(0, dummy_implicit_vars.len());
        }

        let variables: HashMap<Ident<Canonical>, _> = variable_list
            .into_iter()
            .map(|v| (Ident::new(v.ident()), v))
            .collect();

        Self {
            ident: Ident::new(&x_model.name),
            display_name: x_model.name.clone(),
            variables,
            // The canonical-keyed map above collapses same-canonical twins
            // last-wins; record the collision as a model error instead of
            // silently building a different model (GH #891). Only DECLARED
            // idents are scanned (mirroring the salsa gate's
            // `declared_variable_idents`, GH #885) -- synthesized implicit
            // vars are unique by construction.
            errors: crate::common::duplicate_variable_errors(
                &x_model.name,
                x_model.variables.iter().map(|v| v.get_ident()),
            ),
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
        let model_deps = model_s0
            .variables
            .values()
            .filter(|v| v.is_module())
            .map(|v| {
                if let Variable::Module { model_name, .. } = v {
                    model_name.to_owned()
                } else {
                    unreachable!();
                }
            })
            .collect::<BTreeSet<_>>();

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
            errors: model_s0.errors.clone(),
            model_deps: Some(model_deps),
            instantiations: None,
            implicit: model_s0.implicit,
            is_macro: model_s0.is_macro,
            macro_params: model_s0.macro_params.clone(),
        }
    }

    /// Fill `instantiations` -- the per-module-instance evaluation order the
    /// monolithic `compiler::Module` consumes -- from the production
    /// dependency-graph query.
    ///
    /// This used to be a second, independent dependency analysis: its own
    /// transitive-closure walk (`all_deps`), its own cross-model output
    /// resolution, its own `CircularDependency` gate and its own topological
    /// runlists. That is the divergence GH #568 tracked. The two gates did not
    /// merely *risk* disagreeing -- they DID: an element-acyclic recurrence SCC
    /// (`ecc[t2] = ecc[t1] + 1`) that `db::dep_graph::resolve_recurrence_sccs`
    /// resolves was still a whole-variable `CircularDependency` here, so this
    /// path rejected models production compiles and simulates. There is now one
    /// gate, and it is the one production uses.
    ///
    /// The runlists are therefore the production runlists, including everything
    /// the second walk did not have: the resolved-SCC contiguous blocks, the
    /// dt stock-submodel-output chain break, and the `INITIAL()`-backed
    /// initials seeding (GH #584).
    ///
    /// # Two model classes are refused, for two different reasons
    ///
    /// A rejected graph (`has_cycle`) records a model-level
    /// `CircularDependency`. Note it does NOT get empty runlists, despite the
    /// dependency MAP being emptied: `topo_sort_str` over an empty map still
    /// emits every allowed name, in sorted order, so the runlists come out
    /// populated and mutually incoherent (a stock's init can read a variable
    /// that is absent from the initials runlist). `ModelStage1::errors` is what
    /// stops `compiler::Module::new` from compiling them; the gate is not
    /// redundant with an empty-runlist check, because there is no such thing.
    ///
    /// A RESOLVED recurrence SCC (`resolved_sccs` non-empty) is refused too,
    /// with `NotSimulatable`. Unifying the gate did not unify the emitter:
    /// production compiles such an SCC by interleaving its members' per-element
    /// segments (`db::assemble::combine_scc_fragment`, reached only from
    /// `assemble_module`), and `compiler::Module` has no equivalent -- it would
    /// emit the members whole, in runlist order, so a member reads a co-member's
    /// element before it is assigned. That is a silent wrong answer, not a
    /// failure. Before the unification the second gate happened to refuse these
    /// models by calling them circular; refusing them deliberately keeps the
    /// monolith honest as an oracle, which is the only reason it still exists.
    /// The distinct code matters: `CircularDependency` here would contradict
    /// `project::tests::the_circular_dependency_gate_is_the_production_one`,
    /// which asserts the two gates agree that this model class is NOT circular.
    pub(crate) fn set_dependencies(
        &mut self,
        db: &dyn crate::db::Db,
        source_model: crate::db::SourceModel,
        project: crate::db::SourceProject,
        instantiations: &BTreeSet<ModuleInputSet>,
    ) {
        // Model errors: seed with any pre-existing model-level errors recorded
        // at Stage0 construction (e.g. DuplicateVariable, GH #891) so this
        // recompute extends rather than clobbers them. `set_dependencies` runs
        // once per model (Project::from_salsa), so taking the list cannot
        // double-report.
        let mut errors: Vec<Error> = self.errors.take().unwrap_or_default();
        let mut has_cycle = false;
        let mut has_resolved_scc = false;

        let to_idents = |names: &[String]| -> Vec<Ident<Canonical>> {
            // The graph's runlists are canonical names by construction, so
            // interning them needs no re-canonicalization scan.
            names.iter().map(|n| Ident::from_str_unchecked(n)).collect()
        };

        let instantiations: HashMap<ModuleInputSet, ModuleStage2> = instantiations
            .iter()
            .map(|inputs| {
                let interned = crate::db::ModuleInputSet::from_canonical_set(db, inputs);
                let graph = crate::db::model_dependency_graph(db, source_model, project, interned);
                has_cycle |= graph.has_cycle;
                has_resolved_scc |= !graph.resolved_sccs.is_empty();
                (
                    inputs.clone(),
                    ModuleStage2 {
                        model_ident: self.name.clone(),
                        inputs: inputs.clone(),
                        runlist_initials: to_idents(&graph.runlist_initials),
                        runlist_flows: to_idents(&graph.runlist_flows),
                        runlist_stocks: to_idents(&graph.runlist_stocks),
                    },
                )
            })
            .collect();

        self.instantiations = Some(instantiations);

        if has_cycle {
            errors.push(Error::new(
                ErrorKind::Model,
                ErrorCode::CircularDependency,
                None,
            ));
        }

        // The gate is unified; the EMITTER is not. See the rustdoc above: the
        // monolith cannot lower an interleaved per-element SCC, so refuse rather
        // than emit members whole and read a co-member's element early.
        if has_resolved_scc {
            errors.push(Error::new(
                ErrorKind::Model,
                ErrorCode::NotSimulatable,
                Some(format!(
                    "model '{}' contains a resolved recurrence SCC, which only \
                     the per-element interleaving fragment compiler can lower",
                    self.name
                )),
            ));
        }

        // Equation errors already ride on the variables themselves, recorded by
        // parsing and by `lower_variable`. Roll them up to the model level so
        // the `Module::new` gate still refuses a model with a broken variable.
        if self
            .variables
            .values()
            .any(|var| var.equation_errors().is_some())
        {
            errors.push(Error::new(
                ErrorKind::Model,
                ErrorCode::VariablesHaveErrors,
                None,
            ));
        }

        self.errors = if errors.is_empty() {
            None
        } else {
            Some(errors)
        };
    }

    /// The equation errors this model's variables carry, keyed by variable.
    ///
    /// A projection of [`Variable::equation_errors`] over the model, used by
    /// the roll-up above and by the tests that check it. User-facing reporting
    /// goes through `db::collect_all_diagnostics`, which reports the same
    /// errors with a source location attached.
    pub fn get_variable_errors(&self) -> HashMap<Ident<Canonical>, Vec<EquationError>> {
        self.variables
            .iter()
            .flat_map(|(ident, var)| var.equation_errors().map(|errs| (ident.clone(), errs)))
            .collect()
    }
}

/// Resolves dependencies to exclude private variables.
/// Private variables (starting with "$⁚") are internal implementation details that
/// should not be exposed through public APIs. This function transitively resolves
/// them to their non-private dependencies.
pub fn resolve_non_private_dependencies(
    model: &ModelStage1,
    deps: HashSet<Ident<Canonical>>,
) -> HashSet<Ident<Canonical>> {
    let mut resolved = HashSet::new();
    let mut visited = HashSet::new();
    let mut to_process: Vec<_> = deps.into_iter().collect();

    while let Some(dep) = to_process.pop() {
        if !visited.insert(dep.clone()) {
            continue;
        }

        if !dep.as_str().starts_with("$⁚") {
            // Public variable - include in results
            resolved.insert(dep);
            continue;
        }

        // Private variable - resolve to its dependencies
        let deps_to_add = if dep.as_str().contains('·') {
            // Module output reference: "module·output"
            // Dependencies are the module's input sources
            let module_name = dep.as_str().split('·').next().unwrap();
            match model.variables.get(module_name) {
                Some(Variable::Module { inputs, .. }) => {
                    inputs.iter().map(|input| input.src.clone()).collect()
                }
                _ => vec![],
            }
        } else {
            // Regular private variable - get its direct dependencies
            match model.variables.get(&dep) {
                Some(var) => {
                    let ast = var.ast().or_else(|| var.init_ast());
                    ast.map(|a| identifier_set(a, &[], None).into_iter().collect())
                        .unwrap_or_default()
                }
                None => vec![],
            }
        };

        // Queue dependencies for processing
        for dep in deps_to_add {
            if !visited.contains(&dep) {
                to_process.push(dep);
            }
        }
    }

    resolved
}

/// Extract the incoming links (dependencies) for a variable using its AST.
///
/// Returns `None` if the variable doesn't exist. Returns `Some(empty set)`
/// for variables with no AST (e.g. per-variable compilation errors).
/// Private/synthetic dependencies are resolved to their public sources.
pub fn get_incoming_links(
    model: &ModelStage1,
    var_ident: &Ident<Canonical>,
) -> Option<HashSet<Ident<Canonical>>> {
    let var = model.variables.get(var_ident)?;
    let raw_deps = match var {
        Variable::Stock {
            init_ast: Some(ast),
            ..
        } => identifier_set(ast, &[], None),
        Variable::Var { ast: Some(ast), .. } => identifier_set(ast, &[], None),
        Variable::Module { inputs, .. } => inputs.iter().map(|i| i.src.clone()).collect(),
        _ => return Some(HashSet::new()),
    };
    Some(resolve_non_private_dependencies(model, raw_deps))
}

#[test]
fn test_module_dependency() {
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
            x_aux("main_init", "7", None),
            x_module("lynxes", &[("main_init", "lynxes.init")], None),
            x_module("hares", &[("lynxes.lynxes", "hares.lynxes")], None),
        ],
    );

    let _models: HashMap<String, &datamodel::Model> = vec![
        ("main".to_string(), &main_model),
        ("lynxes".to_string(), &lynxes_model),
        ("hares".to_string(), &hares_model),
    ]
    .into_iter()
    .collect();
}

#[test]
fn test_get_incoming_links_basic() {
    let dm_model = x_model(
        "test",
        vec![
            x_aux("rate", "0.1", None),
            x_stock("population", "100", &["births"], &[], None),
            x_flow("births", "population * rate", None),
        ],
    );
    let project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![dm_model],
        source: None,
        ai_information: None,
    };
    let db = db::SimlinDb::default();
    let sync = db::sync_from_datamodel(&db, &project);
    let source_model = sync.models["test"].source;
    let edges_result = db::model_causal_edges(&db, source_model, sync.project);

    // "births" depends on "population" and "rate": the causal edges map
    // records dep -> {dependents}, so "population" and "rate" should each
    // list "births" as a dependent.
    assert!(
        edges_result
            .edges
            .get("population")
            .is_some_and(|s| s.contains("births")),
        "births should depend on population"
    );
    assert!(
        edges_result
            .edges
            .get("rate")
            .is_some_and(|s| s.contains("births")),
        "births should depend on rate"
    );

    // "rate" has no dependencies (constant) -- "rate" should not appear
    // as a value in any edge set (nothing depends on rate except births,
    // which we already checked). Verify rate has no outgoing edges of its own.
    let rate_has_deps = edges_result.edges.values().any(|s| s.contains("rate"));
    // "rate" appears as a dep key (things depend on rate), but rate itself
    // should not appear as a dependent of anything.
    assert!(!rate_has_deps, "rate should have no incoming dependencies");
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
    let expected = Variable::Module {
        model_name: Ident::new("hares"),
        ident: Ident::new("hares"),
        units: None,
        inputs,
        errors: vec![],
        unit_errors: vec![],
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

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
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

    let actual = parse_var(&[], hares_var, &mut implicit_vars, &units_ctx, |mi| {
        resolve_module_input(&models, "main", hares_var.get_ident(), &mi.src, &mi.dst)
    });
    assert!(actual.equation_errors().is_none());
    assert!(implicit_vars.is_empty());
    assert_eq!(expected, actual);
}

/// A variable carrying an equation error rolls up to a model-level
/// `VariablesHaveErrors`, which is what stops `compiler::Module::new` from
/// compiling a model with a broken variable.
///
/// The errors rolled up are the ones parsing and `lower_variable` recorded on
/// the variables themselves (`Variable::errors`). `set_dependencies` used to
/// contribute a second source -- its own dependency walk's
/// `UnknownDependency` / `CircularDependency` / `ExpectedModule` -- which was
/// the GH #568 divergence; that walk is gone and its diagnostics come from the
/// production gate instead (see
/// `unknown_dependency_reaches_the_one_remaining_gate`).
#[test]
fn variable_equation_errors_roll_up_to_the_model() {
    let units_ctx = Context::new(&[], &Default::default()).0;
    let main_model = x_model("main", vec![x_aux("aux_3", "1 +", None)]);
    let direct = ModelStage0::new(&main_model, &[], &units_ctx, false);
    let models: HashMap<Ident<Canonical>, &ModelStage0> =
        std::iter::once((Ident::new("main"), &direct)).collect();

    let db = db::SimlinDb::default();
    let project_datamodel = datamodel::Project {
        name: "errors".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![main_model.clone()],
        source: None,
        ai_information: None,
    };
    let sync = db::sync_from_datamodel(&db, &project_datamodel);

    let scope = ScopeStage0 {
        models: &models,
        dimensions: &Default::default(),
        model_name: "main",
    };
    let mut model = ModelStage1::new(&scope, &direct);
    let no_module_inputs: ModuleInputSet = BTreeSet::new();
    let default_instantiation = [no_module_inputs].iter().cloned().collect();
    model.set_dependencies(
        &db,
        sync.models["main"].source,
        sync.project,
        &default_instantiation,
    );

    assert_eq!(
        Some(&Error::new(
            ErrorKind::Model,
            ErrorCode::VariablesHaveErrors,
            None
        )),
        model.errors.as_ref().and_then(|errs| errs.first()),
    );

    let var_errors = model.get_variable_errors();
    let aux_3_key = Ident::new("aux_3");
    assert_eq!(
        1,
        var_errors.len(),
        "exactly the broken variable carries errors, got: {var_errors:?}"
    );
    assert!(var_errors.contains_key(&aux_3_key));
}

/// The `UnknownDependency` the deleted second dependency walk used to raise is
/// still raised -- by the production gate, which is now the only one.
#[test]
fn unknown_dependency_reaches_the_one_remaining_gate() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("main").aux("aux_3", "unknown_variable * 3.14", None);
    let errs = match tp.compile() {
        Ok(_) => panic!("a model referencing an undefined variable must not compile"),
        Err(errs) => errs,
    };
    assert!(
        errs.iter()
            .any(|(loc, code)| loc == "main.aux_3" && *code == ErrorCode::UnknownDependency),
        "expected a main.aux_3 UnknownDependency, got: {errs:?}"
    );
}

/// `PREVIOUS(module_var)` must rewrite through a synthesized scalar helper aux
/// on the salsa-cached parse path exactly as it does on the direct one.
///
/// A module occupies several flattened slots, so a `LoadPrev` at its base
/// offset would read the wrong sub-variable; the parser therefore captures the
/// current value into a helper first. That rewrite is driven by the
/// module-ident set, which the two paths derive along different routes
/// (`collect_module_idents` over the `datamodel::Model` here, an interned
/// `ModuleIdentContext` off the salsa inputs there) -- so the agreement is a
/// real cross-check.
///
/// Previously written against the test-only `ModelStage0::new_cached` twin;
/// now against `db::stages::model_stage0`, which IS the crate's salsa-cached
/// Stage0 constructor.
#[test]
fn test_cached_stage0_preserves_previous_helper_rewrite() {
    let units_ctx = Context::new(&[], &Default::default()).0;
    let main_model = x_model(
        "main",
        vec![
            x_module("sub", &[], None),
            x_aux("prev_sub", "PREVIOUS(sub)", None),
        ],
    );
    // Multiple vars so `sub` is clearly multi-slot when flattened.
    let sub_model = x_model(
        "sub",
        vec![x_aux("internal", "42", None), x_aux("output", "TIME", None)],
    );
    let project_datamodel = datamodel::Project {
        name: "cached_prev_module".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![main_model.clone(), sub_model],
        source: None,
        ai_information: None,
    };

    let direct = ModelStage0::new(&main_model, &[], &units_ctx, false);

    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let cached = db::model_stage0(&db, sync.models["main"].source, sync.project);

    let has_previous_helper = |model: &ModelStage0| {
        model
            .variables
            .keys()
            .any(|ident| ident.as_str().starts_with("$⁚prev_sub⁚0⁚arg0"))
    };

    assert!(
        has_previous_helper(&direct),
        "direct parse should synthesize a scalar helper for PREVIOUS(sub)"
    );
    assert_eq!(
        has_previous_helper(&direct),
        has_previous_helper(cached),
        "cached parse should preserve PREVIOUS(module_var) helper rewriting"
    );
}

/// GH #891: every `ModelStage0` constructor collapses variables into a
/// canonical-keyed map (last-in-declaration-order wins), so two variables whose
/// names canonicalize identically would silently produce a DIFFERENT model than
/// the one written. Both the datamodel-driven constructor and the cached query
/// must record a `DuplicateVariable` model-level error naming the colliding
/// spellings, and that error must survive `set_dependencies` (which recomputes
/// model-level errors).
///
/// The two derive the error from genuinely different inputs -- the raw
/// `datamodel::Variable` list here, the memoized `db::model_duplicate_variables`
/// groups off `SourceModel::declared_variable_idents` there -- so comparing them
/// is a cross-check, not a restatement. (Before this commit the salsa side of
/// this test was `ModelStage0::new_cached`, which computed the errors with the
/// SAME raw-list helper as the direct constructor and so compared nothing.)
#[test]
fn test_stage0_records_duplicate_variable_error() {
    let units_ctx = Context::new(&[], &Default::default()).0;
    let main_model = x_model(
        "main",
        vec![x_aux("net flow", "1", None), x_aux("net_flow", "2", None)],
    );

    let direct = ModelStage0::new(&main_model, &[], &units_ctx, false);
    let errors = direct
        .errors
        .as_ref()
        .expect("duplicate canonical idents must record a model-level error");
    assert_eq!(1, errors.len());
    assert_eq!(ErrorCode::DuplicateVariable, errors[0].code);
    let msg = errors[0].details.as_deref().unwrap_or("");
    assert!(
        msg.contains("'net flow'") && msg.contains("'net_flow'"),
        "message should name both colliding spellings, got: {msg}"
    );

    // The salsa-cached query must agree with the direct constructor.
    let project_datamodel = datamodel::Project {
        name: "dup".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: vec![],
        units: vec![],
        models: vec![main_model.clone()],
        source: None,
        ai_information: None,
    };
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let cached = db::model_stage0(&db, sync.models["main"].source, sync.project);
    assert_eq!(direct.errors, cached.errors);

    // `set_dependencies` rebuilds the model-level error list; the Stage0
    // duplicate error must be extended, not clobbered.
    let models: HashMap<Ident<Canonical>, &ModelStage0> =
        std::iter::once((Ident::new("main"), &direct)).collect();
    let scope = ScopeStage0 {
        models: &models,
        dimensions: &Default::default(),
        model_name: "main",
    };
    let mut model = ModelStage1::new(&scope, &direct);
    let no_module_inputs: ModuleInputSet = BTreeSet::new();
    let default_instantiation = [no_module_inputs].iter().cloned().collect();
    model.set_dependencies(
        &db,
        sync.models["main"].source,
        sync.project,
        &default_instantiation,
    );
    assert!(
        model
            .errors
            .as_ref()
            .is_some_and(|errs| errs.iter().any(|e| e.code == ErrorCode::DuplicateVariable)),
        "DuplicateVariable must survive set_dependencies, got: {:?}",
        model.errors
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
fn test_previous_module_input_var_uses_helper_rewrite() {
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
        parsed
            .variables
            .keys()
            .any(|ident| ident.as_str().starts_with("$⁚lagged⁚0⁚arg0")),
        "PREVIOUS(module_input) should synthesize a scalar helper aux"
    );
}

#[test]
fn test_model_implicit_var_info_uses_module_context() {
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
        implicit_info
            .keys()
            .any(|name| name.starts_with("$⁚prev_delayed⁚0⁚arg0")),
        "model_implicit_var_info should include helper auxes for PREVIOUS(module-backed var)"
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

#[test]
fn test_collect_module_idents_skips_intrinsic_previous() {
    let vars = vec![
        x_aux("x", "TIME", None),
        x_aux("prev_x", "PREVIOUS(x)", None),
        x_aux("prev_x_init", "PREVIOUS(x, 42)", None),
    ];
    let registry = crate::module_functions::MacroRegistry::default();
    let ids = collect_module_idents(&vars, &registry);
    assert!(
        !ids.contains(&Ident::new("prev_x")),
        "1-arg PREVIOUS should stay on the intrinsic opcode path",
    );
    assert!(
        !ids.contains(&Ident::new("prev_x_init")),
        "2-arg PREVIOUS should also stay intrinsic",
    );
}

/// `equation_is_module_call` (Phase 3 Task 2 signature) returns `true` for
/// both a macro call and a stdlib call, and `false` for a plain arithmetic
/// expression. This is the pre-classification predicate that decides whether
/// a caller variable's ident lands in `module_idents` (so `PREVIOUS(y)`
/// rewrites correctly when `y = MYMACRO(...)`).
#[test]
fn test_equation_is_module_call_recognizes_macros_and_stdlib() {
    use crate::module_functions::MacroRegistry;

    // A registry containing a single macro `MYMACRO(a, b)`.
    let macro_model = datamodel::Model {
        name: "mymacro".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("mymacro", "a * b", None),
            x_aux("a", "0", None),
            x_aux("b", "0", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(datamodel::MacroSpec {
            parameters: vec!["a".to_string(), "b".to_string()],
            primary_output: "mymacro".to_string(),
            additional_outputs: vec![],
        }),
    };
    let registry = MacroRegistry::build(&[macro_model]).expect("valid macro project builds");

    let macro_call = datamodel::Equation::Scalar("MYMACRO(a, b)".to_string());
    assert!(
        equation_is_module_call(&macro_call, &registry),
        "a top-level macro call must be recognized as a module call",
    );

    let stdlib_call = datamodel::Equation::Scalar("SMTH1(x, 5)".to_string());
    assert!(
        equation_is_module_call(&stdlib_call, &registry),
        "a top-level stdlib call must still be recognized as a module call",
    );

    let arithmetic = datamodel::Equation::Scalar("a + b".to_string());
    assert!(
        !equation_is_module_call(&arithmetic, &registry),
        "a plain arithmetic expression is not a module call",
    );
}

#[test]
fn test_collect_module_idents_skips_apply_to_all_previous() {
    let vars = vec![
        x_aux("x", "TIME", None),
        datamodel::Variable::Aux(datamodel::Aux {
            ident: "prev_x_init".to_string(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["DimA".to_string()],
                "PREVIOUS(x, 42)".to_string(),
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }),
    ];
    let registry = crate::module_functions::MacroRegistry::default();
    let ids = collect_module_idents(&vars, &registry);
    assert!(
        !ids.contains(&Ident::new("prev_x_init")),
        "ApplyToAll equations that invoke PREVIOUS should stay intrinsic",
    );
}

/// Compile-time proof that the two name-keyed, pre-layout model-compilation
/// stages and the plain diagnostic `Error` implement `salsa::Update`.
///
/// This is the enabling derive for GH #966: the per-project Stage0/Stage1 maps
/// are about to be cached as `#[salsa::tracked]` `returns(ref)` queries (so
/// whole-project unit diagnostics stop being quadratic in the model count).
///
/// NOTE: salsa 0.26 does NOT require the `Update` derive to store these as a
/// tracked function's `returns(ref)` value. All three are `'static + PartialEq`,
/// and salsa's derive/dispatch resolves an update through the method-dispatch
/// hack: with no `Update` impl it falls back to the `UpdateFallback` blanket impl
/// for `'static + PartialEq` (compare-and-replace), so a `returns(ref)` query
/// over any of them compiles unchanged even with the derive removed. At runtime a
/// tracked function's memo is backdated purely by `PartialEq` (`values_equal`) and
/// never calls `maybe_update`. So the box-2 cached queries would NOT catch a
/// dropped derive -- this test is the SOLE guard pinning all three derives (and,
/// through the reasoning below, the `compiler::Expr` prohibition).
///
/// What the derive actually buys: it opts each type into salsa's recursive
/// in-place `maybe_update` path instead of the compare-and-replace fallback, and
/// makes it eligible for every salsa position -- including a future tracked-STRUCT
/// field, which IS updated through `maybe_update` at runtime (unlike a tracked-fn
/// return). Keeping the derives is cheap and forward-compatible; the point of the
/// test is that nothing else forces them to stay.
///
/// This used to carry a prohibition: `compiler::Expr` and "everything
/// downstream of variable layout" must never derive `salsa::Update`, because
/// those values were keyed to ONE model-global slot layout and caching one
/// across a layout change would silently return a stale address. **That premise
/// no longer holds.** Since GH #964's symbolic emission, a lowered `Expr`
/// references variables by NAME (`compiler::VarRef`) and carries no offsets at
/// all; addresses are assigned exactly once, at assembly, by
/// `symbolic::resolve_module`. `Expr` is now as layout-independent as the two
/// stages above it, and caching one across a layout change is sound -- which is
/// the whole reason a per-variable fragment survives its neighbours moving.
///
/// The derive is still absent from `Expr`, and this test still does not assert
/// it, because nothing needs it yet: making the *lowering* itself a tracked
/// query is a separate change. The point is that the obstacle is gone, so
/// whoever wants it does not have to relitigate a soundness argument.
/// `ModelStage0` and `ModelStage1` are keyed by canonical name and built before
/// layout, and `Error` is layout-independent diagnostic data, so the derive is
/// sound for all three.
#[test]
fn stage_types_and_error_implement_salsa_update() {
    fn assert_update<T: salsa::Update>() {}
    assert_update::<ModelStage0>();
    assert_update::<ModelStage1>();
    assert_update::<crate::common::Error>();
}
