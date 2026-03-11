// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;

use crate::ast::{Expr0, lower_ast};
use crate::common::{
    Canonical, EquationError, EquationResult, Error, ErrorCode, ErrorKind, Ident, Result,
    UnitError, canonicalize,
};
use crate::dimensions::DimensionsContext;
use crate::variable::{ModuleInput, Variable, identifier_set};
use crate::{datamodel, eqn_err, model_err};

#[cfg(any(test, feature = "testing"))]
use {
    crate::common::topo_sort,
    crate::datamodel::{Dimension, UnitMap},
    crate::db::{self, SourceModel, SourceProject},
    crate::units::Context,
    crate::units_check,
    crate::var_eqn_err,
    crate::variable::{parse_var, parse_var_with_module_context},
    crate::vm::StepPart,
    std::result::Result as StdResult,
};

#[cfg(test)]
use crate::testutils::{aux, flow, stock, x_aux, x_flow, x_model, x_module, x_stock};

pub type ModuleInputSet = BTreeSet<Ident<Canonical>>;
pub type DependencySet = BTreeSet<Ident<Canonical>>;
#[cfg(any(test, feature = "testing"))]
pub type DependencyMap = HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>>;

pub type VariableStage0 = Variable<datamodel::ModuleReference, Expr0>;

/// ModelStage0 converts a datamodel::Model to one with a map of canonicalized
/// identifiers to Variables where module dependencies haven't been resolved.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ModelStage0 {
    pub ident: Ident<Canonical>,
    pub display_name: String,
    pub variables: HashMap<Ident<Canonical>, VariableStage0>,
    pub errors: Option<Vec<Error>>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ModelStage1 {
    pub name: Ident<Canonical>,
    pub display_name: String,
    pub variables: HashMap<Ident<Canonical>, Variable>,
    /// Model-level errors are also accumulated via the salsa accumulator in
    /// `compile_var_fragment` and `check_model_units`. This field is retained
    /// because `Module::new` (interpreter path) checks it for early-exit
    /// validation and several test helpers inspect it directly.
    pub errors: Option<Vec<Error>>,
    /// Unit warnings are also accumulated via the salsa accumulator in
    /// `check_model_units`. This field is retained for the monolithic
    /// `Project::from` construction path used by tests.
    ///
    /// Contains unit-related issues that should be surfaced to users but
    /// should NOT block simulation. Unit mismatches are common in real-world
    /// models and should not prevent running simulations.
    pub unit_warnings: Option<Vec<Error>>,
    /// model_deps is the transitive set of model names referenced from modules in this model
    pub model_deps: Option<BTreeSet<Ident<Canonical>>>,
    pub instantiations: Option<HashMap<ModuleInputSet, ModuleStage2>>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct ModuleStage2 {
    pub model_ident: Ident<Canonical>,
    /// inputs is the set of variables overridden (provided as input) in this
    /// module instantiation.
    pub inputs: ModuleInputSet,
    /// initial_dependencies contains variables dependencies needed to calculate the initial values of stocks
    pub initial_dependencies: HashMap<Ident<Canonical>, DependencySet>,
    /// dt_dependencies contains the variable dependencies used during normal "dt" iterations/calculations.
    pub dt_dependencies: HashMap<Ident<Canonical>, DependencySet>,
    pub runlist_initials: Vec<Ident<Canonical>>,
    pub runlist_flows: Vec<Ident<Canonical>>,
    pub runlist_stocks: Vec<Ident<Canonical>>,
}

impl ModelStage1 {
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn dt_deps(
        &self,
        inputs: &ModuleInputSet,
    ) -> Option<&HashMap<Ident<Canonical>, DependencySet>> {
        self.instantiations
            .as_ref()
            .and_then(|instances| instances.get(inputs).map(|module| &module.dt_dependencies))
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn initial_deps(
        &self,
        inputs: &ModuleInputSet,
    ) -> Option<&HashMap<Ident<Canonical>, DependencySet>> {
        self.instantiations.as_ref().and_then(|instances| {
            instances
                .get(inputs)
                .map(|module| &module.initial_dependencies)
        })
    }

    /// Collect the set of variables referenced by INIT() calls across all
    /// equations in this model. These variables must be included in the
    /// Initials runlist so that INIT(x) can read x's initial value even
    /// when x is not a stock or module.
    ///
    /// Parallel logic exists in db.rs variable_direct_dependencies_impl for
    /// the salsa incremental path.
    #[cfg(any(test, feature = "testing"))]
    fn init_referenced_vars(&self) -> HashSet<Ident<Canonical>> {
        self.variables
            .values()
            .filter_map(|v| v.ast())
            .flat_map(crate::variable::init_referenced_idents)
            .map(|s| Ident::new(&s))
            .collect()
    }
}

#[cfg(any(test, feature = "testing"))]
fn module_deps(
    ctx: &DepContext,
    var: &Variable,
    is_stock: &dyn Fn(&Ident<Canonical>) -> bool,
) -> Vec<Ident<Canonical>> {
    if let Variable::Module {
        inputs, model_name, ..
    } = var
    {
        if ctx.is_initial {
            let model = ctx.models[model_name];
            // FIXME: do this higher up
            let module_inputs = &inputs.iter().map(|mi| mi.dst.clone()).collect();
            if let Some(initial_deps) = model.initial_deps(module_inputs) {
                let mut stock_deps = HashSet::<Ident<Canonical>>::new();

                for var in model.variables.values() {
                    if let Variable::Stock { .. } = var
                        && let Some(deps) = initial_deps.get(var.ident())
                    {
                        stock_deps.extend(deps.iter().cloned());
                    }
                }

                // During initialization, modules need their stock
                // inputs initialized first (e.g. SMOOTH3 needs its
                // input stock's initial value).  Unlike the DT phase
                // where stocks use their previous-timestep value, the
                // initial phase must respect stock dependencies.
                inputs
                    .iter()
                    .flat_map(|input| {
                        let src = &input.src;
                        if stock_deps.contains(&input.dst) {
                            let direct_dep = match src.as_str().find('.') {
                                Some(pos) => &src.as_str()[..pos],
                                None => src.as_str(),
                            };

                            Some(Ident::new(direct_dep))
                        } else {
                            None
                        }
                    })
                    .collect()
            } else {
                panic!("internal compiler error: invariant broken");
            }
        } else {
            inputs
                .iter()
                .flat_map(|r| {
                    let src = &r.src;
                    let direct_dep = match src.as_str().find('.') {
                        Some(pos) => &src.as_str()[..pos],
                        None => src.as_str(),
                    };

                    if is_stock(src) {
                        None
                    } else {
                        Some(Ident::new(direct_dep))
                    }
                })
                .collect()
        }
    } else {
        unreachable!();
    }
}

#[cfg(any(test, feature = "testing"))]
fn module_output_deps<'a>(
    ctx: &DepContext,
    model_name: &Ident<Canonical>,
    output_ident: &str,
    inputs: &'a [ModuleInput],
    module_ident: &'a str,
) -> Result<BTreeSet<&'a str>> {
    if !ctx.models.contains_key(model_name) {
        return model_err!(BadModelName, model_name.to_string());
    }
    let model = ctx.models[model_name];

    let module_inputs = &inputs.iter().map(|mi| mi.dst.clone()).collect();
    let deps = if ctx.is_initial {
        model.initial_deps(module_inputs)
    } else {
        model.dt_deps(module_inputs)
    };

    if deps.is_none() {
        return model_err!(Generic, output_ident.to_owned());
    }
    let deps = deps.unwrap();
    if !deps.contains_key(output_ident) {
        return model_err!(UnknownDependency, output_ident.to_owned());
    }

    let output_var = &model.variables[output_ident];
    let output_deps = &deps[output_ident];

    let mut final_deps: BTreeSet<&str> = BTreeSet::new();

    if ctx.is_initial || !output_var.is_stock() {
        final_deps.insert(module_ident);
    }

    for dep in output_deps.iter() {
        for module_input in inputs.iter() {
            if &module_input.dst == dep {
                final_deps.insert(module_input.src.as_str());
            }
        }
    }

    Ok(final_deps)
}

#[cfg(any(test, feature = "testing"))]
fn direct_deps(ctx: &DepContext, var: &Variable) -> Vec<Ident<Canonical>> {
    let is_stock = |ident: &Ident<Canonical>| -> bool {
        matches!(
            resolve_relative2(ctx, ident.as_str()),
            Some(Variable::Stock { .. })
        )
    };
    if var.is_module() {
        module_deps(ctx, var, &is_stock)
    } else {
        let ast = if ctx.is_initial {
            var.init_ast()
        } else {
            var.ast()
        };
        match ast {
            Some(ast) => {
                let converted_dims: Vec<crate::dimensions::Dimension> = ctx
                    .dimensions
                    .iter()
                    .map(crate::dimensions::Dimension::from)
                    .collect();
                let mut deps = identifier_set(ast, &converted_dims, ctx.module_inputs);
                if !ctx.is_initial {
                    let init_only = crate::variable::init_only_referenced_idents_with_module_inputs(
                        ast,
                        ctx.module_inputs,
                    );
                    deps.retain(|dep| !init_only.contains(dep.as_str()));
                }
                let lagged_only = crate::variable::lagged_only_previous_idents_with_module_inputs(
                    ast,
                    ctx.module_inputs,
                );
                deps.retain(|dep| !lagged_only.contains(dep.as_str()));
                deps
            }
            .into_iter()
            .collect(),
            None => vec![],
        }
    }
}

#[cfg(any(test, feature = "testing"))]
struct DepContext<'a> {
    is_initial: bool,
    model_name: &'a str, // this needs to be a str, not an Ident<Canonical> for lifetime reasons when recursing
    models: &'a HashMap<Ident<Canonical>, &'a ModelStage1>,
    sibling_vars: &'a HashMap<Ident<Canonical>, Variable>,
    module_inputs: Option<&'a ModuleInputSet>,
    dimensions: &'a [Dimension],
}

// to ensure we sort the list of variables in O(n*log(n)) time, we
// need to iterate over the set of variables we have and compute
// their recursive dependencies.  (assuming this function runs
// in <= O(n*log(n)))
#[cfg(any(test, feature = "testing"))]
fn all_deps<'a, Iter>(
    ctx: &DepContext,
    vars: Iter,
) -> StdResult<DependencyMap, (Ident<Canonical>, EquationError)>
where
    Iter: Iterator<Item = &'a Variable>,
{
    // we need to use vars multiple times, so collect it into a Vec once
    let vars = vars.collect::<Vec<_>>();
    let mut processing: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let mut all_vars: HashMap<&'a str, &'a Variable> =
        vars.iter().map(|v| (v.ident(), *v)).collect();
    let mut all_var_deps: HashMap<Ident<Canonical>, Option<BTreeSet<Ident<Canonical>>>> = vars
        .iter()
        .map(|v| (Ident::from_str_unchecked(v.ident()), None))
        .collect();

    fn all_deps_inner<'a>(
        ctx: &DepContext,
        id: &str,
        processing: &mut BTreeSet<Ident<Canonical>>,
        all_vars: &mut HashMap<&'a str, &'a Variable>,
        all_var_deps: &mut HashMap<Ident<Canonical>, Option<BTreeSet<Ident<Canonical>>>>,
    ) -> StdResult<(), (Ident<Canonical>, EquationError)> {
        let var = all_vars[id];

        // short circuit if we've already figured this out
        let canonical_id = Ident::from_str_unchecked(id);
        if all_var_deps[&canonical_id].is_some() {
            return Ok(());
        }

        // dependency chains break at stocks, as we use their value from the
        // last dt timestep.  BUT if we are calculating dependencies in the
        // initial dt, then we need to treat stocks as ordinary variables.
        if var.is_stock() && !ctx.is_initial {
            all_var_deps.insert(canonical_id.clone(), Some(BTreeSet::new()));
            return Ok(());
        }

        processing.insert(canonical_id.clone());

        // all deps start out as the direct deps
        let mut all_deps: BTreeSet<Ident<Canonical>> = BTreeSet::new();

        for dep in direct_deps(ctx, var).into_iter() {
            // TODO: we could potentially handle this by passing around some context
            //   variable, but its just terrible.
            if dep.as_str().starts_with("\\·") {
                let loc = var
                    .ast()
                    .unwrap()
                    .get_var_loc(dep.as_str())
                    .unwrap_or_default();
                return var_eqn_err!(
                    Ident::from_str_unchecked(var.ident()),
                    NoAbsoluteReferences,
                    loc.start,
                    loc.end
                );
            }

            // in the case of a dependency on a module output, this one dep may
            // turn into several: we'll need to depend on the inputs to that module
            let filtered_deps: Vec<Ident<Canonical>> = if dep.as_str().contains('·') {
                // if the dependency was e.g. "submodel.output", do a dataflow analysis to
                // figure out which of the set of (inputs + module) we depend on
                let parts = dep.as_str().splitn(2, '·').collect::<Vec<_>>();
                let module_ident = parts[0];
                let output_ident = parts[1];

                if !all_vars.contains_key(module_ident) {
                    let loc = var
                        .ast()
                        .unwrap()
                        .get_var_loc(dep.as_str())
                        .unwrap_or_default();
                    return var_eqn_err!(
                        Ident::from_str_unchecked(var.ident()),
                        UnknownDependency,
                        loc.start,
                        loc.end
                    );
                }

                if let Variable::Module {
                    model_name, inputs, ..
                } = all_vars[module_ident]
                {
                    // XXX: I don't remember why we do this differently here
                    //      and then special case modules below (end of this
                    //      for loop)
                    match module_output_deps(ctx, model_name, output_ident, inputs, module_ident) {
                        Ok(deps) => deps.into_iter().map(Ident::from_str_unchecked).collect(),
                        Err(err) => {
                            return Err((Ident::from_str_unchecked(var.ident()), err.into()));
                        }
                    }
                } else {
                    let loc = var
                        .ast()
                        .unwrap()
                        .get_var_loc(dep.as_str())
                        .unwrap_or_default();
                    return var_eqn_err!(
                        Ident::from_str_unchecked(var.ident()),
                        ExpectedModule,
                        loc.start,
                        loc.end
                    );
                }
            } else {
                vec![dep]
            };

            for dep in filtered_deps {
                if !all_vars.contains_key(dep.as_str()) {
                    let loc = var
                        .ast()
                        .unwrap()
                        .get_var_loc(dep.as_str())
                        .unwrap_or_default();
                    return var_eqn_err!(
                        Ident::from_str_unchecked(var.ident()),
                        UnknownDependency,
                        loc.start,
                        loc.end
                    );
                }

                if ctx.is_initial || !all_vars[dep.as_str()].is_stock() {
                    all_deps.insert(dep.clone());

                    // ensure we don't blow the stack
                    if processing.contains(&dep) {
                        let loc = match var.ast() {
                            Some(ast) => ast.get_var_loc(dep.as_str()).unwrap_or_default(),
                            None => Default::default(),
                        };
                        return var_eqn_err!(
                            Ident::from_str_unchecked(var.ident()),
                            CircularDependency,
                            loc.start,
                            loc.end
                        );
                    }

                    if all_var_deps[&dep].is_none() {
                        all_deps_inner(ctx, dep.as_str(), processing, all_vars, all_var_deps)?;
                    }

                    // we actually don't want the module's dependencies here;
                    // we handled that above in module_output_deps()
                    if !all_vars[dep.as_str()].is_module() {
                        let dep_deps = all_var_deps[&dep].as_ref().unwrap();
                        all_deps.extend(dep_deps.iter().cloned());
                    }
                }
            }
        }

        processing.remove(&canonical_id);

        all_var_deps.insert(canonical_id, Some(all_deps));

        Ok(())
    }

    for var in vars {
        all_deps_inner(
            ctx,
            var.ident(),
            &mut processing,
            &mut all_vars,
            &mut all_var_deps,
        )?;
    }

    // this unwrap is safe, because of the full iteration over vars directly above
    let var_deps: HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>> = all_var_deps
        .into_iter()
        .map(|(k, v)| (k, v.unwrap()))
        .collect();

    Ok(var_deps)
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

// the ident arg must be from a CanonicalIdent, but is a &str here for lifetime reasons around recursion.
#[cfg(any(test, feature = "testing"))]
fn resolve_relative2<'a>(ctx: &DepContext<'a>, ident: &'a str) -> Option<&'a Variable> {
    let model_name = ctx.model_name;
    let ident = if model_name == "main" && ident.starts_with('·') {
        &ident['·'.len_utf8()..]
    } else {
        ident
    };

    let input_prefix = format!("{model_name}·");
    // TODO: this is weird to do here and not before we call into this fn
    let ident = ident.strip_prefix(&input_prefix).unwrap_or(ident);

    // if the identifier is still dotted, its a further submodel reference
    // TODO: this will have to change when we break `module ident == model name`
    if let Some(pos) = ident.find('·') {
        let submodel_name = &ident[..pos];
        let submodel_var = &ident[pos + '·'.len_utf8()..];
        let ctx = DepContext {
            is_initial: ctx.is_initial,
            model_name: submodel_name,
            models: ctx.models,
            sibling_vars: &ctx
                .models
                .get(&Ident::<Canonical>::from_str_unchecked(submodel_name))?
                .variables,
            module_inputs: None,
            dimensions: ctx.dimensions,
        };
        resolve_relative2(&ctx, submodel_var)
    } else {
        Some(ctx.sibling_vars.get(ident)?)
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

                if !modules.contains_key(&key) {
                    // first time we are seeing the model for this module.
                    // make sure all _its_ module instantiations are recorded
                    enumerate_modules_inner(models, model_name.as_str(), mapper, modules)?;
                }

                modules.entry(key).or_default().insert(inputs);
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
///   whose equations parse to a top-level stdlib function call (e.g. SMTH1,
///   DELAY, etc.)
///
/// This set is needed so that `PREVIOUS(module_var)` rewrites through a
/// synthesized scalar helper aux instead of compiling `LoadPrev` directly
/// against a multi-slot module.
pub(crate) fn collect_module_idents(
    variables: &[datamodel::Variable],
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
                if equation_is_stdlib_call(&a.equation) {
                    module_idents.insert(Ident::new(&canonicalize(&a.ident)));
                }
            }
            datamodel::Variable::Flow(f) => {
                if equation_is_stdlib_call(&f.equation) {
                    module_idents.insert(Ident::new(&canonicalize(&f.ident)));
                }
            }
            datamodel::Variable::Stock(_) => {}
        }
    }
    module_idents
}

/// Check if a scalar equation's top-level expression is a stdlib function call.
///
/// Uses `is_stdlib_module_function` as the underlying predicate for name
/// matching.
///
/// This intentionally re-parses the equation text rather than reusing the
/// already-parsed AST. It runs during `collect_module_idents` (called from
/// `ModelStage0::new`), before the full per-variable parse in `parse_var`.
/// The re-parse is cheap (single equation, top-level only) and avoids
/// threading the parsed AST through an intermediate data structure just
/// for this early classification step.
pub(crate) fn equation_is_stdlib_call(eqn: &datamodel::Equation) -> bool {
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
            crate::builtins::is_stdlib_module_function(&func_lower)
        }
        _ => false,
    }
}

#[cfg(any(test, feature = "testing"))]
#[allow(dead_code)]
impl ModelStage0 {
    pub fn new(
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
        let module_idents: HashSet<Ident<Canonical>> = if implicit {
            x_model
                .variables
                .iter()
                .map(|v| Ident::new(&canonicalize(v.get_ident())))
                .collect()
        } else {
            collect_module_idents(&x_model.variables)
        };

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
            errors: None,
            implicit,
        }
    }

    /// Construct a ModelStage0 using salsa-cached per-variable parsing.
    /// Each variable's parse result is individually memoized — editing one
    /// variable's equation only re-parses that variable.
    pub fn new_cached(
        salsa_db: &dyn db::Db,
        source_model: SourceModel,
        source_project: SourceProject,
        x_model: &datamodel::Model,
        dimensions: &[Dimension],
        units_ctx: &Context,
        implicit: bool,
    ) -> Self {
        // For implicit (stdlib) models, bypass the salsa cache and use
        // the direct path with module_idents awareness. This ensures
        // PREVIOUS calls inside submodules rewrite through scalar helper
        // auxes instead of compiling LoadPrev/LoadModuleInput directly
        // against transient module-input slots. The performance impact is
        // negligible since stdlib models have very few variables.
        if implicit {
            return Self::new(x_model, dimensions, units_ctx, implicit);
        }

        let source_vars = source_model.variables(salsa_db);

        let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
        let mut variable_list: Vec<VariableStage0> = Vec::new();

        // Collect module identifiers for the PREVIOUS/INIT helper rewrite.
        // For user models, only explicit Module variables and stdlib-call
        // Aux/Flow variables are multi-slot/module-backed.
        let module_idents: HashSet<Ident<Canonical>> = collect_module_idents(&x_model.variables);
        let mut module_ident_list: Vec<String> = module_idents
            .iter()
            .map(|ident| ident.as_str().to_owned())
            .collect();
        module_ident_list.sort();
        let module_ident_context = db::ModuleIdentContext::new(salsa_db, module_ident_list);

        for dm_var in &x_model.variables {
            let canonical_name = canonicalize(dm_var.get_ident());
            if let Some(source_var) = source_vars.get(canonical_name.as_ref()) {
                let result = db::parse_source_variable_with_module_context(
                    salsa_db,
                    *source_var,
                    source_project,
                    module_ident_context,
                );
                variable_list.push(result.variable.clone());
                implicit_vars.extend(result.implicit_vars.iter().cloned());
            } else {
                variable_list.push(parse_var_with_module_context(
                    dimensions,
                    dm_var,
                    &mut implicit_vars,
                    units_ctx,
                    |mi| Ok(Some(mi.clone())),
                    Some(&module_idents),
                ));
            }
        }

        // Implicit vars (from builtin module expansion) are always parsed
        // directly since they don't have SourceVariable entries.
        {
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
            errors: None,
            implicit,
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
            unit_warnings: None,
            model_deps: Some(model_deps),
            instantiations: None,
            implicit: model_s0.implicit,
        }
    }

    /// Only called from the test-gated `run_default_model_checks`; the
    /// production path runs unit checking via salsa tracked functions.
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn check_units(
        &mut self,
        units_ctx: &Context,
        inferred_units: &HashMap<Ident<Canonical>, UnitMap>,
    ) {
        match units_check::check(units_ctx, inferred_units, self) {
            Ok(Ok(())) => {}
            Ok(Err(errors)) => {
                for (ident, err) in errors.into_iter() {
                    if let Some(var) = self.variables.get_mut(&ident) {
                        var.push_unit_error(err);
                    }
                }
            }
            Err(err) => {
                let mut errors = self.errors.take().unwrap_or_default();
                errors.push(err);
                self.errors = Some(errors);
            }
        };
    }

    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn set_dependencies(
        &mut self,
        models: &HashMap<Ident<Canonical>, &ModelStage1>,
        dimensions: &[Dimension],
        instantiations: &BTreeSet<ModuleInputSet>,
    ) {
        // used when building runlists - give us a stable order to start with
        let mut var_names: Vec<&Ident<Canonical>> = self.variables.keys().collect();
        var_names.sort_unstable();

        // use a Set to deduplicate problems we see in dt_deps and initial_deps
        let mut var_errors: HashMap<Ident<Canonical>, HashSet<EquationError>> = HashMap::new();
        // model errors
        let mut errors: Vec<Error> = Vec::new();

        let instantiations = instantiations
            .iter()
            .map(|instantiation| {
                let mut ctx = DepContext {
                    is_initial: false,
                    model_name: self.name.as_str(),
                    sibling_vars: &self.variables,
                    models,
                    module_inputs: Some(instantiation),
                    dimensions,
                };

                let dt_deps = match all_deps(&ctx, self.variables.values()) {
                    Ok(deps) => Some(deps),
                    Err((ident, err)) => {
                        var_errors.entry(ident).or_default().insert(err);
                        None
                    }
                };

                ctx.is_initial = true;

                let initial_deps = match all_deps(&ctx, self.variables.values()) {
                    Ok(deps) => Some(deps),
                    Err((ident, err)) => {
                        var_errors.entry(ident).or_default().insert(err);
                        None
                    }
                };

                let init_referenced = self.init_referenced_vars();

                let build_runlist = |deps: &HashMap<
                    Ident<Canonical>,
                    BTreeSet<Ident<Canonical>>,
                >,
                                     part: StepPart,
                                     predicate: &dyn Fn(&Ident<Canonical>) -> bool|
                 -> Vec<Ident<Canonical>> {
                    let canonical_var_names: Vec<Ident<Canonical>> = var_names
                        .iter()
                        .filter(|id| predicate(id))
                        .map(|id| (*id).clone())
                        .collect();
                    let runlist: Vec<&Ident<Canonical>> = canonical_var_names.iter().collect();
                    let runlist = match part {
                        StepPart::Initials => {
                            let needed: HashSet<&Ident<Canonical>> = runlist
                                .iter()
                                .cloned()
                                .filter(|id| {
                                    let v = &self.variables[*id];
                                    v.is_stock() || v.is_module() || init_referenced.contains(*id)
                                })
                                .collect();
                            let mut runlist: HashSet<&Ident<Canonical>> =
                                needed.iter().flat_map(|id| &deps[*id]).collect();
                            runlist.extend(needed);
                            let runlist = runlist.into_iter().collect();
                            topo_sort(runlist, deps)
                        }
                        StepPart::Flows => topo_sort(runlist, deps),
                        StepPart::Stocks => runlist,
                    };

                    let runlist: Vec<Ident<Canonical>> = runlist.into_iter().cloned().collect();

                    runlist
                };

                let runlist_initials = if let Some(deps) = initial_deps.as_ref() {
                    build_runlist(deps, StepPart::Initials, &|_| true)
                } else {
                    vec![]
                };

                let runlist_flows = if let Some(deps) = dt_deps.as_ref() {
                    build_runlist(deps, StepPart::Flows, &|id| {
                        instantiation.contains(id) || !self.variables[id].is_stock()
                    })
                } else {
                    vec![]
                };

                let runlist_stocks = if let Some(deps) = dt_deps.as_ref() {
                    build_runlist(deps, StepPart::Stocks, &|id| {
                        let v = &self.variables[id];
                        // modules need to be called _both_ during Flows and Stocks, as
                        // they may contain _both_ flows and Stocks
                        !instantiation.contains(id) && (v.is_stock() || v.is_module())
                    })
                } else {
                    vec![]
                };

                (
                    instantiation.clone(),
                    ModuleStage2 {
                        model_ident: self.name.clone(),
                        inputs: instantiation.clone(),
                        dt_dependencies: dt_deps.unwrap_or_default(),
                        initial_dependencies: initial_deps.unwrap_or_default(),
                        runlist_initials,
                        runlist_flows,
                        runlist_stocks,
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        self.instantiations = Some(instantiations);

        let mut variables_have_errors = false;
        for (ident, var) in self.variables.iter_mut() {
            if var_errors.contains_key(ident) {
                let errors = std::mem::take(var_errors.get_mut(ident).unwrap());
                for error in errors.into_iter() {
                    var.push_error(error);
                }
                variables_have_errors = true;
            }
        }

        if variables_have_errors {
            errors.push(Error::new(
                ErrorKind::Model,
                ErrorCode::VariablesHaveErrors,
                None,
            ));
        }

        let maybe_errors = match errors.len() {
            0 => None,
            _ => Some(errors),
        };

        self.errors = maybe_errors;
    }

    /// Returns unit errors collected via the legacy monolithic compilation path.
    /// The salsa incremental path emits unit errors through `CompilationDiagnostic`
    /// accumulators; prefer `db::collect_model_diagnostics` for new code. This method
    /// is retained for the monolithic test path and for cross-validation.
    pub fn get_unit_errors(&self) -> HashMap<Ident<Canonical>, Vec<UnitError>> {
        self.variables
            .iter()
            .flat_map(|(ident, var)| var.unit_errors().map(|errs| (ident.clone(), errs)))
            .collect()
    }

    /// Returns equation errors collected via the legacy monolithic compilation path.
    /// The salsa incremental path emits equation errors through `CompilationDiagnostic`
    /// accumulators; prefer `db::collect_model_diagnostics` for new code. This method
    /// is retained for the monolithic test path and for cross-validation.
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
    let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();

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

#[test]
fn test_errors() {
    let units_ctx = Context::new(&[], &Default::default()).unwrap();
    let main_model = x_model(
        "main",
        vec![x_aux("aux_3", "unknown_variable * 3.14", None)],
    );
    let owned_models: HashMap<Ident<Canonical>, ModelStage0> =
        vec![("main".to_string(), &main_model)]
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

    let model = {
        let no_module_inputs: ModuleInputSet = BTreeSet::new();
        let default_instantiation = [no_module_inputs].iter().cloned().collect();
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &Default::default(),
            model_name: "main",
        };
        let mut model = ModelStage1::new(&scope, models[&*canonicalize("main")]);
        model.set_dependencies(&HashMap::new(), &[], &default_instantiation);
        model
    };

    assert!(model.errors.is_some());
    assert_eq!(
        &Error::new(ErrorKind::Model, ErrorCode::VariablesHaveErrors, None),
        &model.errors.as_ref().unwrap()[0]
    );

    let var_errors = model.get_variable_errors();
    assert_eq!(1, var_errors.len());
    let aux_3_key = Ident::new("aux_3");
    assert!(var_errors.contains_key(&aux_3_key));
    assert_eq!(1, var_errors[&aux_3_key].len());
    let err = &var_errors[&aux_3_key][0];
    assert_eq!(
        &EquationError {
            start: 0,
            end: 16,
            code: ErrorCode::UnknownDependency
        },
        err
    );
}

#[test]
fn test_new_cached_preserves_previous_helper_rewrite() {
    let units_ctx = Context::new(&[], &Default::default()).unwrap();
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
    let source_model = sync.models["main"].source;
    let cached = ModelStage0::new_cached(
        &db,
        source_model,
        sync.project,
        &main_model,
        &[],
        &units_ctx,
        false,
    );

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
        has_previous_helper(&cached),
        "cached parse should preserve PREVIOUS(module_var) helper rewriting"
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
fn test_init_expression_interpreter_vm_parity() {
    use crate::test_common::TestProject;

    let tp = TestProject::new("init_expr_parity")
        .with_sim_time(1.0, 5.0, 1.0)
        .aux("growing", "TIME * 2", None)
        .aux("frozen_expr", "INIT(growing + 1)", None);

    let interp = tp
        .run_interpreter()
        .expect("interpreter should run successfully");
    let vm = tp.run_vm().expect("VM should run successfully");

    let interp_vals = interp
        .get("frozen_expr")
        .expect("frozen_expr not in interpreter results");
    let vm_vals = vm
        .get("frozen_expr")
        .expect("frozen_expr not in VM results");

    assert_eq!(
        interp_vals.len(),
        vm_vals.len(),
        "step count mismatch between interpreter and VM"
    );

    for (step, (iv, vv)) in interp_vals.iter().zip(vm_vals.iter()).enumerate() {
        assert!(
            (iv - vv).abs() < 1e-10,
            "frozen_expr mismatch at step {step}: interpreter={iv}, vm={vv}"
        );
    }

    // TIME starts at 1.0, so growing+1 starts at 3.0 and INIT should
    // preserve that value for all timesteps.
    for (step, val) in interp_vals.iter().enumerate() {
        assert!(
            (val - 3.0).abs() < 1e-10,
            "frozen_expr should be 3.0 at every step, got {val} at step {step}"
        );
    }
}

#[test]
fn test_previous_module_input_var_uses_helper_rewrite() {
    let units_ctx = Context::new(&[], &Default::default()).unwrap();
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
    let ids = collect_module_idents(&vars);
    assert!(
        !ids.contains(&Ident::new("prev_x")),
        "1-arg PREVIOUS should stay on the intrinsic opcode path",
    );
    assert!(
        !ids.contains(&Ident::new("prev_x_init")),
        "2-arg PREVIOUS should also stay intrinsic",
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
    let ids = collect_module_idents(&vars);
    assert!(
        !ids.contains(&Ident::new("prev_x_init")),
        "ApplyToAll equations that invoke PREVIOUS should stay intrinsic",
    );
}

#[test]
fn test_all_deps() {
    use rand::rng;
    use rand::seq::SliceRandom;

    fn verify_all_deps(
        expected_deps_list: &[(&Variable, &[&str])],
        is_initial: bool,
        models: &HashMap<Ident<Canonical>, &ModelStage1>,
        module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
    ) {
        let default_inputs = BTreeSet::<Ident<Canonical>>::new();
        let expected_deps: HashMap<Ident<Canonical>, BTreeSet<Ident<Canonical>>> =
            expected_deps_list
                .iter()
                .map(|(v, deps)| {
                    (
                        Ident::new(v.ident()),
                        deps.iter().map(|s| Ident::new(s)).collect(),
                    )
                })
                .collect();

        let mut all_vars: Vec<Variable> = expected_deps_list
            .iter()
            .map(|(v, _)| (*v).clone())
            .collect();
        let ctx = DepContext {
            is_initial,
            model_name: "test",
            models,
            sibling_vars: &HashMap::new(),
            module_inputs: Some(module_inputs.unwrap_or(&default_inputs)),
            dimensions: &[],
        };
        let deps = all_deps(&ctx, all_vars.iter()).unwrap();

        if expected_deps != deps {
            let failed_dep_order: Vec<_> = all_vars.iter().map(|v| v.ident()).collect();
            eprintln!("failed order: {failed_dep_order:?}");
            for (v, expected) in expected_deps_list.iter() {
                eprintln!("{}", v.ident());
                let mut expected: Vec<_> = expected.to_vec();
                expected.sort();
                eprintln!("  expected: {expected:?}");
                let mut actual: Vec<_> = deps[&*canonicalize(v.ident())].iter().collect();
                actual.sort();
                eprintln!("  actual  : {actual:?}");
            }
        };
        assert_eq!(expected_deps, deps);

        let mut rng = rng();
        // no matter the order of variables in the list, we should get the same all_deps
        // (even though the order of recursion might change)
        for _ in 0..16 {
            all_vars.shuffle(&mut rng);
            let ctx = DepContext {
                is_initial,
                model_name: "test",
                models,
                sibling_vars: &HashMap::new(),
                module_inputs: Some(module_inputs.unwrap_or(&default_inputs)),
                dimensions: &[],
            };
            let deps = all_deps(&ctx, all_vars.iter()).unwrap();
            assert_eq!(expected_deps, deps);
        }
    }

    let mod_1_model = x_model(
        "mod_1",
        vec![
            x_aux("input", "{expects to be set with module input}", None),
            x_aux("output", "3 * TIME", None),
            x_aux("flow", "2 * input", None),
            x_stock("output_2", "input", &["flow"], &[], None),
        ],
    );

    let main_model = x_model(
        "main",
        vec![
            x_module("mod_1", &[("aux_3", "mod_1.input")], None),
            x_aux("aux_3", "6", None),
            x_flow("inflow", "mod_1.flow", None),
            x_aux("aux_4", "mod_1.output", None),
        ],
    );
    let units_ctx = Context::new(&[], &Default::default()).unwrap();
    let owned_x_models: HashMap<Ident<Canonical>, ModelStage0> = vec![
        ("mod_1".to_owned(), &mod_1_model),
        ("main".to_owned(), &main_model),
    ]
    .into_iter()
    .map(|(name, m)| {
        (
            Ident::new(&name),
            ModelStage0::new(m, &[], &units_ctx, false),
        )
    })
    .collect();
    let x_models: HashMap<Ident<Canonical>, &ModelStage0> =
        owned_x_models.iter().map(|(k, v)| (k.clone(), v)).collect();

    let mut model_list = vec!["mod_1", "main"]
        .into_iter()
        .map(|name| {
            let model_s0 = x_models[&*canonicalize(name)];
            let scope = ScopeStage0 {
                models: &x_models,
                dimensions: &Default::default(),
                model_name: name,
            };
            ModelStage1::new(&scope, model_s0)
        })
        .collect::<Vec<_>>();

    let module_instantiations = {
        let models = model_list.iter().map(|m| (m.name.as_str(), m)).collect();
        // FIXME: ignoring the result here because if we have errors, it doesn't really matter
        enumerate_modules(&models, "main", |model| model.name.clone()).unwrap()
    };

    let models = {
        let no_instantiations = BTreeSet::new();
        let mut models: HashMap<Ident<Canonical>, &ModelStage1> = HashMap::new();
        for model in model_list.iter_mut() {
            let instantiations = module_instantiations
                .get(&model.name)
                .unwrap_or(&no_instantiations);
            model.set_dependencies(&models, &[], instantiations);
            models.insert(model.name.clone(), model);
        }
        models
    };

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let mod_1_orig = &main_model.variables[0];
    assert_eq!("mod_1", mod_1_orig.get_ident());
    let mod_1 = parse_var(&[], mod_1_orig, &mut implicit_vars, &unit_ctx, |mi| {
        Ok(Some(mi.clone()))
    });
    let scope = ScopeStage0 {
        models: &x_models,
        dimensions: &Default::default(),
        model_name: "main",
    };
    let mod_1 = lower_variable(&scope, &mod_1);
    assert!(implicit_vars.is_empty());
    let aux_3 = aux("aux_3", "6");
    let aux_4 = aux("aux_4", "mod_1.output");
    let inflow = flow("inflow", "mod_1.flow");
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&inflow, &["mod_1", "aux_3"]),
        (&mod_1, &["aux_3"]),
        (&aux_3, &[]),
        (&aux_4, &["mod_1"]),
    ];

    verify_all_deps(&expected_deps_list, false, &models, None);

    let aux_used_in_initial = aux("aux_used_in_initial", "7");
    let aux_2 = aux("aux_2", "aux_used_in_initial");
    let aux_3 = aux("aux_3", "aux_2");
    let aux_4 = aux("aux_4", "aux_2");
    let inflow = flow("inflow", "aux_3 + aux_4");
    let outflow = flow("outflow", "stock_1");
    let stock_1 = stock("stock_1", "aux_used_in_initial", &["inflow"], &["outflow"]);
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&aux_used_in_initial, &[]),
        (&aux_2, &["aux_used_in_initial"]),
        (&aux_3, &["aux_used_in_initial", "aux_2"]),
        (&aux_4, &["aux_used_in_initial", "aux_2"]),
        (&inflow, &["aux_used_in_initial", "aux_2", "aux_3", "aux_4"]),
        (&outflow, &[]),
        (&stock_1, &[]),
    ];

    verify_all_deps(&expected_deps_list, false, &models, None);

    // test circular references return an error and don't do something like infinitely
    // recurse
    let aux_a = aux("aux_a", "aux_b");
    let aux_b = aux("aux_b", "aux_a");
    let all_vars = [aux_a, aux_b];
    let ctx = DepContext {
        is_initial: false,
        model_name: "test",
        models: &models,
        sibling_vars: &HashMap::new(),
        module_inputs: None,
        dimensions: &[],
    };
    let deps_result = all_deps(&ctx, all_vars.iter());
    assert!(deps_result.is_err());

    // also self-references should return an error and not blow stock
    let aux_a = aux("aux_a", "aux_a");
    let all_vars = [aux_a];
    let deps_result = all_deps(&ctx, all_vars.iter());
    assert!(deps_result.is_err());

    // test initials
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&aux_used_in_initial, &[]),
        (&aux_2, &["aux_used_in_initial"]),
        (&aux_3, &["aux_used_in_initial", "aux_2"]),
        (&aux_4, &["aux_used_in_initial", "aux_2"]),
        (&inflow, &["aux_used_in_initial", "aux_2", "aux_3", "aux_4"]),
        (&outflow, &["stock_1", "aux_used_in_initial"]),
        (&stock_1, &["aux_used_in_initial"]),
    ];

    verify_all_deps(&expected_deps_list, true, &models, None);

    let aux_if = aux(
        "aux_if",
        "if isModuleInput(aux_true) THEN aux_true ELSE aux_false",
    );
    let aux_true = aux("aux_true", "TIME * 3");
    let aux_false = aux("aux_false", "TIME * 4");
    let expected_deps_list: Vec<(&Variable, &[&str])> = vec![
        (&aux_if, &["aux_true"]),
        (&aux_true, &[]),
        (&aux_false, &[]),
    ];

    let module_inputs = [Ident::new("aux_true")].iter().cloned().collect();
    verify_all_deps(&expected_deps_list, true, &models, Some(&module_inputs));

    // test non-existant variables
}
