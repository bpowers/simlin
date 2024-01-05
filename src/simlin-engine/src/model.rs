// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::result::Result as StdResult;

use crate::ast::{lower_ast, Expr0};
use crate::common::{
    topo_sort, EquationError, EquationResult, Error, ErrorCode, ErrorKind, Ident, Result, UnitError,
};
use crate::datamodel::{Dimension, UnitMap};
use crate::dimensions::DimensionsContext;
#[cfg(test)]
use crate::testutils::{aux, flow, stock, x_aux, x_flow, x_model, x_module, x_stock};
use crate::units::Context;
use crate::variable::{identifier_set, parse_var, ModuleInput, Variable};
use crate::vm::StepPart;
use crate::{canonicalize, datamodel, eqn_err, model_err, units_check, var_eqn_err};

pub type ModuleInputSet = BTreeSet<Ident>;
pub type DependencySet = BTreeSet<Ident>;

pub type VariableStage0 = Variable<datamodel::ModuleReference, Expr0>;

/// ModelStage0 converts a datamodel::Model to one with a map of canonicalized
/// identifiers to Variables where module dependencies haven't been resolved.
#[derive(Clone, PartialEq, Debug)]
pub struct ModelStage0 {
    pub ident: Ident,
    pub display_name: String,
    pub variables: HashMap<Ident, VariableStage0>,
    pub errors: Option<Vec<Error>>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ModelStage1 {
    pub name: String,
    pub display_name: String,
    pub variables: HashMap<Ident, Variable>,
    pub errors: Option<Vec<Error>>,
    /// model_deps is the transitive set of model names referenced from modules in this model
    pub model_deps: Option<BTreeSet<Ident>>,
    pub instantiations: Option<HashMap<ModuleInputSet, ModuleStage2>>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ModuleStage2 {
    pub model_ident: Ident,
    /// inputs is the set of variables overridden (provided as input) in this
    /// module instantiation.
    pub inputs: ModuleInputSet,
    /// initial_dependencies contains variables dependencies needed to calculate the initial values of stocks
    pub initial_dependencies: HashMap<Ident, DependencySet>,
    /// dt_dependencies contains the variable dependencies used during normal "dt" iterations/calculations.
    pub dt_dependencies: HashMap<Ident, DependencySet>,
    pub runlist_initials: Vec<Ident>,
    pub runlist_flows: Vec<Ident>,
    pub runlist_stocks: Vec<Ident>,
}

impl ModelStage1 {
    pub(crate) fn dt_deps(
        &self,
        inputs: &ModuleInputSet,
    ) -> Option<&HashMap<Ident, DependencySet>> {
        self.instantiations
            .as_ref()
            .and_then(|instances| instances.get(inputs).map(|module| &module.dt_dependencies))
    }

    pub(crate) fn initial_deps(
        &self,
        inputs: &ModuleInputSet,
    ) -> Option<&HashMap<Ident, DependencySet>> {
        self.instantiations.as_ref().and_then(|instances| {
            instances
                .get(inputs)
                .map(|module| &module.initial_dependencies)
        })
    }
}

fn module_deps(ctx: &DepContext, var: &Variable, is_stock: &dyn Fn(&str) -> bool) -> Vec<Ident> {
    if let Variable::Module {
        inputs, model_name, ..
    } = var
    {
        if ctx.is_initial {
            let model = ctx.models[model_name];
            // FIXME: do this higher up
            let module_inputs = &inputs.iter().map(|mi| mi.dst.clone()).collect();
            if let Some(initial_deps) = model.initial_deps(module_inputs) {
                let mut stock_deps = HashSet::<Ident>::new();

                for var in model.variables.values() {
                    if let Variable::Stock { .. } = var {
                        if let Some(deps) = initial_deps.get(var.ident()) {
                            stock_deps.extend(deps.iter().cloned());
                        }
                    }
                }

                inputs
                    .iter()
                    .flat_map(|input| {
                        let src = &input.src;
                        if stock_deps.contains(&input.dst) {
                            let direct_dep = match src.find('.') {
                                Some(pos) => &src[..pos],
                                None => src,
                            };

                            if is_stock(src) {
                                None
                            } else {
                                Some(direct_dep.to_string())
                            }
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
                    let direct_dep = match src.find('.') {
                        Some(pos) => &src[..pos],
                        None => src,
                    };

                    if is_stock(src) {
                        None
                    } else {
                        Some(direct_dep.to_string())
                    }
                })
                .collect()
        }
    } else {
        unreachable!();
    }
}

fn module_output_deps<'a>(
    ctx: &DepContext,
    model_name: &str,
    output_ident: &str,
    inputs: &'a [ModuleInput],
    module_ident: &'a str,
) -> Result<BTreeSet<&'a str>> {
    if !ctx.models.contains_key(model_name) {
        return model_err!(BadModelName, model_name.to_owned());
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
                final_deps.insert(&module_input.src);
            }
        }
    }

    Ok(final_deps)
}

fn direct_deps(ctx: &DepContext, var: &Variable) -> Vec<Ident> {
    let is_stock = |ident: &str| -> bool {
        matches!(resolve_relative2(ctx, ident), Some(Variable::Stock { .. }))
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
            Some(ast) => identifier_set(ast, ctx.dimensions, ctx.module_inputs)
                .into_iter()
                .collect(),
            None => vec![],
        }
    }
}

struct DepContext<'a> {
    is_initial: bool,
    model_name: &'a str,
    models: &'a HashMap<Ident, &'a ModelStage1>,
    sibling_vars: &'a HashMap<Ident, Variable>,
    module_inputs: Option<&'a ModuleInputSet>,
    dimensions: &'a [Dimension],
}

// to ensure we sort the list of variables in O(n*log(n)) time, we
// need to iterate over the set of variables we have and compute
// their recursive dependencies.  (assuming this function runs
// in <= O(n*log(n)))
fn all_deps<'a, Iter>(
    ctx: &DepContext,
    vars: Iter,
) -> StdResult<HashMap<Ident, BTreeSet<Ident>>, (Ident, EquationError)>
where
    Iter: Iterator<Item = &'a Variable>,
{
    // we need to use vars multiple times, so collect it into a Vec once
    let vars = vars.collect::<Vec<_>>();
    let mut processing: BTreeSet<Ident> = BTreeSet::new();
    let mut all_vars: HashMap<&'a str, &'a Variable> =
        vars.iter().map(|v| (v.ident(), *v)).collect();
    let mut all_var_deps: HashMap<Ident, Option<BTreeSet<Ident>>> =
        vars.iter().map(|v| (v.ident().to_owned(), None)).collect();

    fn all_deps_inner<'a>(
        ctx: &DepContext,
        id: &str,
        processing: &mut BTreeSet<Ident>,
        all_vars: &mut HashMap<&'a str, &'a Variable>,
        all_var_deps: &mut HashMap<Ident, Option<BTreeSet<Ident>>>,
    ) -> StdResult<(), (Ident, EquationError)> {
        let var = all_vars[id];

        // short circuit if we've already figured this out
        if all_var_deps[id].is_some() {
            return Ok(());
        }

        // dependency chains break at stocks, as we use their value from the
        // last dt timestep.  BUT if we are calculating dependencies in the
        // initial dt, then we need to treat stocks as ordinary variables.
        if var.is_stock() && !ctx.is_initial {
            all_var_deps.insert(id.to_owned(), Some(BTreeSet::new()));
            return Ok(());
        }

        processing.insert(id.to_owned());

        // all deps start out as the direct deps
        let mut all_deps: BTreeSet<Ident> = BTreeSet::new();

        for dep in direct_deps(ctx, var).into_iter() {
            // TODO: we could potentially handle this by passing around some context
            //   variable, but its just terrible.
            if dep.starts_with("\\·") {
                let loc = var.ast().unwrap().get_var_loc(&dep).unwrap_or_default();
                return var_eqn_err!(
                    var.ident().to_owned(),
                    NoAbsoluteReferences,
                    loc.start,
                    loc.end
                );
            }

            // in the case of a dependency on a module output, this one dep may
            // turn into several: we'll need to depend on the inputs to that module
            let filtered_deps: Vec<Ident> = if dep.contains('·') {
                // if the dependency was e.g. "submodel.output", do a dataflow analysis to
                // figure out which of the set of (inputs + module) we depend on
                let parts = dep.splitn(2, '·').collect::<Vec<_>>();
                let module_ident = parts[0];
                let output_ident = parts[1];

                if !all_vars.contains_key(module_ident) {
                    let loc = var.ast().unwrap().get_var_loc(&dep).unwrap_or_default();
                    return var_eqn_err!(
                        var.ident().to_owned(),
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
                        Ok(deps) => deps.into_iter().map(|s| s.to_string()).collect(),
                        Err(err) => {
                            return Err((var.ident().to_owned(), err.into()));
                        }
                    }
                } else {
                    let loc = var.ast().unwrap().get_var_loc(&dep).unwrap_or_default();
                    return var_eqn_err!(
                        var.ident().to_owned(),
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
                    let loc = var.ast().unwrap().get_var_loc(&dep).unwrap_or_default();
                    return var_eqn_err!(
                        var.ident().to_owned(),
                        UnknownDependency,
                        loc.start,
                        loc.end
                    );
                }

                if ctx.is_initial || !all_vars[dep.as_str()].is_stock() {
                    all_deps.insert(dep.to_string());

                    // ensure we don't blow the stack
                    if processing.contains(dep.as_str()) {
                        let loc = match var.ast() {
                            Some(ast) => ast.get_var_loc(&dep).unwrap_or_default(),
                            None => Default::default(),
                        };
                        return var_eqn_err!(
                            var.ident().to_owned(),
                            CircularDependency,
                            loc.start,
                            loc.end
                        );
                    }

                    if all_var_deps[dep.as_str()].is_none() {
                        all_deps_inner(ctx, &dep, processing, all_vars, all_var_deps)?;
                    }

                    // we actually don't want the module's dependencies here;
                    // we handled that above in module_output_deps()
                    if !all_vars[dep.as_str()].is_module() {
                        let dep_deps = all_var_deps[dep.as_str()].as_ref().unwrap();
                        all_deps.extend(dep_deps.iter().cloned());
                    }
                }
            }
        }

        processing.remove(id);

        all_var_deps.insert(id.to_owned(), Some(all_deps));

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
    let var_deps: HashMap<Ident, BTreeSet<Ident>> = all_var_deps
        .into_iter()
        .map(|(k, v)| (k, v.unwrap()))
        .collect();

    Ok(var_deps)
}

fn resolve_relative<'a>(
    models: &'a HashMap<Ident, ModelStage0>,
    model_name: &str,
    ident: &str,
) -> Option<&'a VariableStage0> {
    let ident = if model_name == "main" && ident.starts_with('·') {
        &ident['·'.len_utf8()..]
    } else {
        ident
    };
    let model = models.get(model_name)?;

    let input_prefix = format!("{}·", model_name);
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

fn resolve_relative2<'a>(ctx: &DepContext<'a>, ident: &'a str) -> Option<&'a Variable> {
    let model_name = ctx.model_name;
    let ident = if model_name == "main" && ident.starts_with('·') {
        &ident['·'.len_utf8()..]
    } else {
        ident
    };

    let input_prefix = format!("{}·", model_name);
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
            sibling_vars: &ctx.models.get(submodel_name)?.variables,
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
pub(crate) fn lower_variable(
    scope: &ScopeStage0,
    parent_module_name: &str,
    var_s0: &VariableStage0,
) -> Variable {
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
            table,
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
                table: table.clone(),
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
                resolve_module_input(scope.models, parent_module_name, ident, &mi.src, &mi.dst)
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
    models: &HashMap<String, ModelStage0>,
    parent_model_name: &str,
    ident: &str,
    orig_src: &'a str,
    orig_dst: &'a str,
) -> EquationResult<Option<ModuleInput>> {
    let input_prefix = format!("{}·", ident);
    let maybe_strip_leading_dot = |s: &'a str| -> &'a str {
        if parent_model_name == "main" && s.starts_with('·') {
            &s['·'.len_utf8()..] // '·' is a 2 byte long unicode character
        } else {
            s
        }
    };
    let src: Ident = canonicalize(maybe_strip_leading_dot(orig_src));
    let dst: Ident = canonicalize(maybe_strip_leading_dot(orig_dst));

    // Stella has a bug where if you have one module feeding into another,
    // it writes identical tags to both.  So skip the tag that is non-local
    // but don't report it as an error
    if src.starts_with(&input_prefix) {
        return Ok(None);
    }

    let dst = dst.strip_prefix(&input_prefix);
    if dst.is_none() {
        return eqn_err!(BadModuleInputDst, 0, 0);
    }
    let dst = dst.unwrap().to_string();

    // TODO: reevaluate if this is really the best option here
    // if the source is a temporary created by the engine, assume it is OK
    if src.starts_with("$⁚") {
        return Ok(Some(ModuleInput { src, dst }));
    }

    match resolve_relative(models, parent_model_name, &src) {
        Some(_) => Ok(Some(ModuleInput { src, dst })),
        None => eqn_err!(BadModuleInputSrc, 0, 0),
    }
}

pub fn enumerate_modules<T>(
    models: &HashMap<&str, &ModelStage1>,
    main_model_name: &str,
    mapper: fn(&ModelStage1) -> T,
) -> Result<HashMap<T, BTreeSet<BTreeSet<Ident>>>>
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
    modules: &mut HashMap<T, BTreeSet<BTreeSet<Ident>>>,
) -> Result<()>
where
    T: Eq + Hash,
{
    let model = *models.get(model_name).ok_or_else(|| Error {
        kind: ErrorKind::Simulation,
        code: ErrorCode::NotSimulatable,
        details: Some(format!("model for module '{}' not found", model_name)),
    })?;
    for (_id, v) in model.variables.iter() {
        if let Variable::Module {
            model_name, inputs, ..
        } = v
        {
            if let Some(model) = models.get(model_name.as_str()) {
                let inputs: BTreeSet<Ident> =
                    inputs.iter().map(|input| input.dst.clone()).collect();

                let key = mapper(model);

                if !modules.contains_key(&key) {
                    // first time we are seeing the model for this module.
                    // make sure all _its_ module instantiations are recorded
                    enumerate_modules_inner(models, model_name, mapper, modules)?;
                }

                modules.entry(key).or_default().insert(inputs);
            } else {
                return model_err!(BadModelName, model_name.clone());
            }
        }
    }

    Ok(())
}

impl ModelStage0 {
    pub fn new(
        x_model: &datamodel::Model,
        dimensions: &[Dimension],
        units_ctx: &Context,
        implicit: bool,
    ) -> Self {
        let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();

        let mut variable_list: Vec<VariableStage0> = x_model
            .variables
            .iter()
            .map(|v| {
                parse_var(dimensions, v, &mut implicit_vars, units_ctx, |mi| {
                    Ok(Some(mi.clone()))
                })
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

        let variables: HashMap<Ident, _> = variable_list
            .into_iter()
            .map(|v| (v.ident().to_string(), v))
            .collect();

        Self {
            ident: canonicalize(&x_model.name),
            display_name: x_model.name.clone(),
            variables,
            errors: None,
            implicit,
        }
    }
}

pub(crate) struct ScopeStage0<'a> {
    pub models: &'a HashMap<Ident, ModelStage0>,
    pub dimensions: &'a DimensionsContext,
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

        ModelStage1 {
            name: model_s0.ident.clone(),
            display_name: model_s0.display_name.clone(),
            variables: model_s0
                .variables
                .iter()
                .map(|(ident, v)| (ident.clone(), lower_variable(scope, &model_s0.ident, v)))
                .collect(),
            errors: model_s0.errors.clone(),
            model_deps: Some(model_deps),
            instantiations: None,
            implicit: model_s0.implicit,
        }
    }

    pub(crate) fn check_units(
        &mut self,
        units_ctx: &Context,
        inferred_units: &HashMap<Ident, UnitMap>,
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

    pub(crate) fn set_dependencies(
        &mut self,
        models: &HashMap<Ident, &ModelStage1>,
        dimensions: &[Dimension],
        instantiations: &BTreeSet<ModuleInputSet>,
    ) {
        // used when building runlists - give us a stable order to start with
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = self.variables.keys().map(|s| s.as_str()).collect();
            var_names.sort_unstable();
            var_names
        };

        // use a Set to deduplicate problems we see in dt_deps and initial_deps
        let mut var_errors: HashMap<Ident, HashSet<EquationError>> = HashMap::new();
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

                let build_runlist = |deps: &HashMap<Ident, BTreeSet<Ident>>,
                                     part: StepPart,
                                     predicate: &dyn Fn(&&str) -> bool|
                 -> Vec<Ident> {
                    let runlist: Vec<&str> = var_names.iter().cloned().filter(predicate).collect();
                    let runlist = match part {
                        StepPart::Initials => {
                            let needed: HashSet<&str> = runlist
                                .iter()
                                .cloned()
                                .filter(|id| {
                                    let v = &self.variables[*id];
                                    v.is_stock() || v.is_module()
                                })
                                .collect();
                            let mut runlist: HashSet<&str> = needed
                                .iter()
                                .flat_map(|id| &deps[*id])
                                .map(|id| id.as_str())
                                .collect();
                            runlist.extend(needed);
                            let runlist = runlist.into_iter().collect();
                            topo_sort(runlist, deps)
                        }
                        StepPart::Flows => topo_sort(runlist, deps),
                        StepPart::Stocks => runlist,
                    };
                    // eprintln!("runlist {}", model_name);
                    // for (i, name) in runlist.iter().enumerate() {
                    //     eprintln!("  {}: {}", i, name);
                    // }
                    let runlist: Vec<Ident> =
                        runlist.into_iter().map(|ident| ident.to_owned()).collect();
                    // for v in runlist.clone().unwrap().iter() {
                    //     eprintln!("{}", pretty(&v.ast));
                    // }
                    // eprintln!("");

                    runlist
                };

                let runlist_initials = if let Some(deps) = initial_deps.as_ref() {
                    build_runlist(deps, StepPart::Initials, &|_| true)
                } else {
                    vec![]
                };

                let runlist_flows = if let Some(deps) = dt_deps.as_ref() {
                    build_runlist(deps, StepPart::Flows, &|id| {
                        instantiation.contains(*id) || !self.variables[*id].is_stock()
                    })
                } else {
                    vec![]
                };

                let runlist_stocks = if let Some(deps) = dt_deps.as_ref() {
                    build_runlist(deps, StepPart::Stocks, &|id| {
                        let v = &self.variables[*id];
                        // modules need to be called _both_ during Flows and Stocks, as
                        // they may contain _both_ flows and Stocks
                        !instantiation.contains(*id) && (v.is_stock() || v.is_module())
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

    pub fn get_unit_errors(&self) -> HashMap<Ident, Vec<UnitError>> {
        self.variables
            .iter()
            .flat_map(|(ident, var)| var.unit_errors().map(|errs| (ident.clone(), errs)))
            .collect()
    }

    pub fn get_variable_errors(&self) -> HashMap<Ident, Vec<EquationError>> {
        self.variables
            .iter()
            .flat_map(|(ident, var)| var.equation_errors().map(|errs| (ident.clone(), errs)))
            .collect()
    }
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
fn test_module_parse() {
    use crate::variable::ModuleInput;
    let inputs: Vec<ModuleInput> = vec![
        ModuleInput {
            src: "area".to_string(),
            dst: "area".to_string(),
        },
        ModuleInput {
            src: "lynxes·lynxes_stock".to_string(),
            dst: "lynxes".to_string(),
        },
    ];
    let expected = Variable::Module {
        model_name: "hares".to_string(),
        ident: "hares".to_string(),
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

    let models: HashMap<Ident, ModelStage0> = vec![
        ("main".to_string(), &main_model),
        ("lynxes".to_string(), &lynxes_model),
        ("hares".to_string(), &hares_model),
    ]
    .into_iter()
    .map(|(name, m)| (name, ModelStage0::new(m, &[], &units_ctx, false)))
    .collect();

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
    let models: HashMap<String, ModelStage0> = vec![("main".to_string(), &main_model)]
        .into_iter()
        .map(|(name, m)| (name, ModelStage0::new(m, &[], &units_ctx, false)))
        .collect();

    let model = {
        let no_module_inputs: ModuleInputSet = BTreeSet::new();
        let default_instantiation = [no_module_inputs].iter().cloned().collect();
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &Default::default(),
        };
        let mut model = ModelStage1::new(&scope, &models["main"]);
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
    assert!(var_errors.contains_key("aux_3"));
    assert_eq!(1, var_errors["aux_3"].len());
    let err = &var_errors["aux_3"][0];
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
fn test_all_deps() {
    use rand::seq::SliceRandom;
    use rand::thread_rng;

    fn verify_all_deps(
        expected_deps_list: &[(&Variable, &[&str])],
        is_initial: bool,
        models: &HashMap<Ident, &ModelStage1>,
        module_inputs: Option<&BTreeSet<Ident>>,
    ) {
        let default_inputs = BTreeSet::<Ident>::new();
        let expected_deps: HashMap<Ident, BTreeSet<Ident>> = expected_deps_list
            .iter()
            .map(|(v, deps)| {
                (
                    v.ident().to_string(),
                    deps.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect();

        let mut all_vars: Vec<Variable> = expected_deps_list
            .iter()
            .map(|(v, _)| (*v).clone())
            .collect();
        let ctx = DepContext {
            is_initial,
            model_name: "main",
            models,
            sibling_vars: &HashMap::new(),
            module_inputs: Some(module_inputs.unwrap_or(&default_inputs)),
            dimensions: &[],
        };
        let deps = all_deps(&ctx, all_vars.iter()).unwrap();

        if expected_deps != deps {
            let failed_dep_order: Vec<_> = all_vars.iter().map(|v| v.ident()).collect();
            eprintln!("failed order: {:?}", failed_dep_order);
            for (v, expected) in expected_deps_list.iter() {
                eprintln!("{}", v.ident());
                let mut expected: Vec<_> = expected.to_vec();
                expected.sort();
                eprintln!("  expected: {:?}", expected);
                let mut actual: Vec<_> = deps[v.ident()].iter().collect();
                actual.sort();
                eprintln!("  actual  : {:?}", actual);
            }
        };
        assert_eq!(expected_deps, deps);

        let mut rng = thread_rng();
        // no matter the order of variables in the list, we should get the same all_deps
        // (even though the order of recursion might change)
        for _ in 0..16 {
            all_vars.shuffle(&mut rng);
            let ctx = DepContext {
                is_initial,
                model_name: "main",
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
    let x_models: HashMap<String, ModelStage0> = vec![
        ("mod_1".to_owned(), &mod_1_model),
        ("main".to_owned(), &main_model),
    ]
    .into_iter()
    .map(|(name, m)| (name, ModelStage0::new(m, &[], &units_ctx, false)))
    .collect();

    let mut model_list = vec!["mod_1", "main"]
        .into_iter()
        .map(|name| {
            let model_s0 = &x_models[name];
            let scope = ScopeStage0 {
                models: &x_models,
                dimensions: &Default::default(),
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
        let mut models: HashMap<Ident, &ModelStage1> = HashMap::new();
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
    };
    let mod_1 = lower_variable(&scope, "main", &mod_1);
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
    let all_vars = vec![aux_a, aux_b];
    let ctx = DepContext {
        is_initial: false,
        model_name: "main",
        models: &models,
        sibling_vars: &HashMap::new(),
        module_inputs: None,
        dimensions: &[],
    };
    let deps_result = all_deps(&ctx, all_vars.iter());
    assert!(deps_result.is_err());

    // also self-references should return an error and not blow stock
    let aux_a = aux("aux_a", "aux_a");
    let all_vars = vec![aux_a];
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

    let module_inputs = ["aux_true".to_string()].iter().cloned().collect();
    verify_all_deps(&expected_deps_list, true, &models, Some(&module_inputs));

    // test non-existant variables
}
