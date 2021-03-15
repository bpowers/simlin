// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::result::Result as StdResult;

use crate::common::{
    len_utf8, EquationError, EquationResult, Error, ErrorCode, ErrorKind, Ident, Result,
};
use crate::datamodel::Dimension;
use crate::variable::{identifier_set, parse_var, ModuleInput, Variable};
use crate::{canonicalize, datamodel, eqn_err, model_err, var_eqn_err};

pub type ModuleInputSet = BTreeSet<Ident>;
pub type DependencySet = BTreeSet<Ident>;

#[derive(Clone, PartialEq, Debug)]
pub struct Model {
    pub name: String,
    pub variables: HashMap<String, Variable>,
    pub errors: Option<Vec<Error>>,
    /// model_deps is the transitive set of model names referenced from modules in this model
    pub model_deps: Option<BTreeSet<Ident>>,
    // the dep maps have an extra layer of indirection: the key is the
    // set of module inputs
    dt_dep_map: Option<HashMap<ModuleInputSet, HashMap<Ident, DependencySet>>>,
    initial_dep_map: Option<HashMap<ModuleInputSet, HashMap<Ident, DependencySet>>>,
    /// implicit is true if this model was implicitly added to the project
    /// by virtue of it being in the stdlib (or some similar reason)
    pub implicit: bool,
}

impl Model {
    pub(crate) fn initial_deps(&self) -> Option<&HashMap<Ident, DependencySet>> {
        match &self.initial_dep_map {
            Some(deps) => {
                #[allow(clippy::never_loop)]
                for v in deps.values() {
                    return Some(v);
                }
                None
            }
            None => None,
        }
    }

    pub(crate) fn dt_deps(&self) -> Option<&HashMap<Ident, DependencySet>> {
        match &self.dt_dep_map {
            Some(deps) => {
                #[allow(clippy::never_loop)]
                for v in deps.values() {
                    return Some(v);
                }
                None
            }
            None => None,
        }
    }
}

fn module_deps(ctx: &DepContext, var: &Variable, is_stock: &dyn Fn(&str) -> bool) -> Vec<Ident> {
    if let Variable::Module {
        inputs, model_name, ..
    } = var
    {
        if ctx.is_initial {
            let model = ctx.models[model_name];
            if model.initial_deps().is_some() {
                let model_ctx = DepContext {
                    is_initial: ctx.is_initial,
                    model_name: &model.name,
                    models: ctx.models,
                    sibling_vars: &model.variables,
                    module_inputs: Some(inputs),
                    dimensions: ctx.dimensions,
                };
                // this unwrap should be safe because we _already_ successfully computed
                // this (the if initial_deps.is_some() we are in).  We have to recompute it to narrow down
                // if statements containing `isModuleInput()` conditions, now that we have
                // the context to know what our module inputs are.
                let initial_deps = all_deps(&model_ctx, model.variables.values()).unwrap();
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
                    .map(|input| {
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
                    .filter(|d| d.is_some())
                    .map(|d| d.unwrap())
                    .collect()
            } else {
                vec![]
            }
        } else {
            inputs
                .iter()
                .map(|r| {
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
                .filter(|d| d.is_some())
                .map(|d| d.unwrap())
                .collect()
        }
    } else {
        unreachable!();
    }
}

fn module_output_deps<'a>(
    ctx: &DepContext,
    output_ident: &str,
    inputs: &'a [ModuleInput],
    module_ident: &'a str,
) -> Result<BTreeSet<&'a str>> {
    if !ctx.models.contains_key(ctx.model_name) {
        return model_err!(BadModelName, ctx.model_name.to_owned());
    }
    let model = ctx.models[ctx.model_name];

    let deps = all_deps(&ctx, model.variables.values())?;
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
        match var.ast() {
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
    models: &'a HashMap<Ident, &'a Model>,
    sibling_vars: &'a HashMap<Ident, Variable>,
    module_inputs: Option<&'a [ModuleInput]>,
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

            // in the case of module output dependencies, this one dep may
            // turn into several.
            let filtered_deps: Vec<Ident> = if dep.contains('·') {
                // if the dependency was e.g. "submodel.output", do a dataflow analysis to
                // figure out which of the set of (inputs + module) we depend on
                let parts = (&dep).splitn(2, '·').collect::<Vec<_>>();
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
                    let module_ctx = DepContext {
                        is_initial: ctx.is_initial,
                        model_name: model_name.as_str(),
                        models: ctx.models,
                        sibling_vars: &ctx.models.get(model_name.as_str()).unwrap().variables,
                        module_inputs: Some(inputs),
                        dimensions: ctx.dimensions,
                    };
                    match module_output_deps(&module_ctx, output_ident, &inputs, module_ident) {
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
    models: &HashMap<String, HashMap<Ident, &'a datamodel::Variable>>,
    model_name: &str,
    ident: &str,
) -> Option<&'a datamodel::Variable> {
    let ident = if model_name == "main" && ident.starts_with('·') {
        &ident[len_utf8('·')..]
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
        let submodel_var = &ident[pos + len_utf8('·')..];
        resolve_relative(models, submodel_name, submodel_var)
    } else {
        Some(model.get(ident)?)
    }
}

fn resolve_relative2<'a>(ctx: &DepContext<'a>, ident: &'a str) -> Option<&'a Variable> {
    let model_name = ctx.model_name;
    let ident = if model_name == "main" && ident.starts_with('·') {
        &ident[len_utf8('·')..]
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
        let submodel_var = &ident[pos + len_utf8('·')..];
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

// parent_module_name is the name of the model that has the module instantiation,
// _not_ the name of the model this module instantiates
pub fn resolve_module_input<'a>(
    models: &HashMap<String, HashMap<Ident, &datamodel::Variable>>,
    parent_model_name: &str,
    ident: &str,
    orig_src: &'a str,
    orig_dst: &'a str,
) -> EquationResult<Option<ModuleInput>> {
    let input_prefix = format!("{}·", ident);
    let maybe_strip_leading_dot = |s: &'a str| -> &'a str {
        if parent_model_name == "main" && s.starts_with('·') {
            &s[len_utf8('·')..] // '·' is a 2 byte long unicode character
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
    if (&src).starts_with("$⁚") {
        return Ok(Some(ModuleInput { src, dst }));
    }

    match resolve_relative(models, parent_model_name, &src) {
        Some(_) => Ok(Some(ModuleInput { src, dst })),
        None => eqn_err!(BadModuleInputSrc, 0, 0),
    }
}

pub(crate) fn enumerate_modules(
    models: &HashMap<Ident, &Model>,
    model_name: &str,
    modules: &mut HashMap<Ident, BTreeSet<BTreeSet<Ident>>>,
) -> Result<()> {
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
            let inputs: BTreeSet<String> = inputs.iter().map(|input| input.dst.clone()).collect();

            if !modules.contains_key(model_name) {
                // first time we are seeing the model for this module.
                // make sure all _its_ module instantiations are recorded
                enumerate_modules(models, model_name, modules)?;
            }

            modules
                .entry(model_name.clone())
                .or_insert_with(BTreeSet::new)
                .insert(inputs);
        }
    }

    Ok(())
}

impl Model {
    pub fn new(
        models: &HashMap<String, HashMap<Ident, &datamodel::Variable>>,
        x_model: &datamodel::Model,
        dimensions: &[Dimension],
        implicit: bool,
    ) -> Self {
        let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();

        let mut variable_list: Vec<Variable> = x_model
            .variables
            .iter()
            .map(|v| parse_var(models, &x_model.name, dimensions, v, &mut implicit_vars))
            .collect();

        {
            // FIXME: this is an unfortunate API choice
            let mut dummy_implicit_vars: Vec<datamodel::Variable> = Vec::new();
            variable_list.extend(implicit_vars.into_iter().map(|x_var| {
                parse_var(
                    models,
                    &x_model.name,
                    dimensions,
                    &x_var,
                    &mut dummy_implicit_vars,
                )
            }));
            assert_eq!(0, dummy_implicit_vars.len());
        }

        let variables: HashMap<String, Variable> = variable_list
            .into_iter()
            .map(|v| (v.ident().to_string(), v))
            .collect();

        let model_deps = variables
            .values()
            .filter(|v| v.is_module())
            .map(|v| {
                if let Variable::Module { model_name, .. } = v {
                    model_name.to_owned()
                } else {
                    unreachable!();
                }
            })
            .collect();

        Model {
            name: x_model.name.clone(),
            variables,
            errors: None,
            model_deps: Some(model_deps),
            dt_dep_map: None,
            initial_dep_map: None,
            implicit,
        }
    }

    pub(crate) fn set_dependencies(
        &mut self,
        models: &HashMap<Ident, &Model>,
        dimensions: &[Dimension],
        instantiations: &BTreeSet<BTreeSet<Ident>>,
    ) {
        // use a Set to deduplicate problems we see in dt_deps and initial_deps
        let mut var_errors: HashMap<Ident, HashSet<EquationError>> = HashMap::new();

        let mut ctx = DepContext {
            is_initial: false,
            model_name: self.name.as_str(),
            sibling_vars: &self.variables,
            models,
            module_inputs: None,
            dimensions,
        };

        let empty_instantiation = BTreeSet::<Ident>::new();

        let mut dt_dep_map = HashMap::with_capacity(instantiations.len());
        let dt_deps = match all_deps(&ctx, self.variables.values()) {
            Ok(deps) => Some(deps),
            Err((ident, err)) => {
                var_errors
                    .entry(ident)
                    .or_insert_with(HashSet::new)
                    .insert(err);
                None
            }
        };

        if let Some(deps) = dt_deps {
            dt_dep_map.insert(empty_instantiation.clone(), deps);
            self.dt_dep_map = Some(dt_dep_map);
        }

        ctx.is_initial = true;

        let mut initial_dep_map = HashMap::with_capacity(instantiations.len());
        let initial_deps = match all_deps(&ctx, self.variables.values()) {
            Ok(deps) => Some(deps),
            Err((ident, err)) => {
                var_errors
                    .entry(ident)
                    .or_insert_with(HashSet::new)
                    .insert(err);
                None
            }
        };

        if let Some(deps) = initial_deps {
            initial_dep_map.insert(empty_instantiation, deps);
            self.initial_dep_map = Some(initial_dep_map);
        }

        let mut errors: Vec<Error> = Vec::new();
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

    pub fn get_variable_errors(&self) -> HashMap<Ident, Vec<EquationError>> {
        self.variables
            .iter()
            .filter(|(_, var)| var.errors().is_some())
            .map(|(ident, var)| {
                let errors = var.errors().unwrap();
                (ident.clone(), errors.clone())
            })
            .collect()
    }
}

#[cfg(test)]
fn optional_vec(slice: &[&str]) -> Vec<String> {
    slice.iter().map(|id| id.to_string()).collect()
}

#[cfg(test)]
fn x_module(ident: &str, refs: &[(&str, &str)]) -> datamodel::Variable {
    use datamodel::{Module, ModuleReference, Variable, Visibility};
    let references: Vec<ModuleReference> = refs
        .iter()
        .map(|(src, dst)| ModuleReference {
            src: src.to_string(),
            dst: dst.to_string(),
        })
        .collect();

    Variable::Module(Module {
        ident: ident.to_string(),
        model_name: ident.to_string(),
        documentation: "".to_string(),
        units: None,
        references,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
fn x_flow(ident: &str, eqn: &str) -> datamodel::Variable {
    use datamodel::{Equation, Flow, Variable, Visibility};
    Variable::Flow(Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: None,
        gf: None,
        non_negative: false,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
fn flow(ident: &str, eqn: &str) -> Variable {
    let var = x_flow(ident, eqn);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let var = parse_var(&HashMap::new(), "main", &[], &var, &mut implicit_vars);
    assert!(var.errors().is_none());
    assert!(implicit_vars.is_empty());
    var
}

#[cfg(test)]
fn x_aux(ident: &str, eqn: &str) -> datamodel::Variable {
    use datamodel::{Aux, Equation, Variable, Visibility};
    Variable::Aux(Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: None,
        gf: None,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
fn aux(ident: &str, eqn: &str) -> Variable {
    let var = x_aux(ident, eqn);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let var = parse_var(&HashMap::new(), "main", &[], &var, &mut implicit_vars);
    assert!(var.errors().is_none());
    assert!(implicit_vars.is_empty());
    var
}

#[cfg(test)]
fn x_stock(ident: &str, eqn: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    use datamodel::{Equation, Stock, Variable, Visibility};
    Variable::Stock(Stock {
        ident: ident.to_string(),
        equation: Equation::Scalar(eqn.to_string()),
        documentation: "".to_string(),
        units: None,
        inflows: optional_vec(inflows),
        outflows: optional_vec(outflows),
        non_negative: false,
        can_be_module_input: false,
        visibility: Visibility::Private,
    })
}

#[cfg(test)]
fn stock(ident: &str, eqn: &str, inflows: &[&str], outflows: &[&str]) -> Variable {
    let var = x_stock(ident, eqn, inflows, outflows);
    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let var = parse_var(&HashMap::new(), "main", &[], &var, &mut implicit_vars);
    assert!(var.errors().is_none());
    assert!(implicit_vars.is_empty());
    var
}

#[cfg(test)]
fn x_model(ident: &str, variables: Vec<datamodel::Variable>) -> datamodel::Model {
    datamodel::Model {
        name: ident.to_string(),
        variables,
        views: vec![],
    }
}

#[test]
fn test_module_dependency() {
    let lynxes_model = x_model(
        "lynxes",
        vec![
            x_aux("init", "5"),
            x_stock("lynxes_stock", "100 * init", &["inflow"], &[]),
            x_flow("inflow", "1"),
        ],
    );
    let hares_model = x_model(
        "hares",
        vec![
            x_aux("lynxes", "0"),
            x_stock("hares_stock", "100", &[], &["outflow"]),
            x_flow("outflow", ".1 * hares_stock"),
        ],
    );
    let main_model = x_model(
        "main",
        vec![
            x_aux("main_init", "7"),
            x_module("lynxes", &[("main_init", "lynxes.init")]),
            x_module("hares", &[("lynxes.lynxes", "hares.lynxes")]),
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
    };

    let lynxes_model = x_model(
        "lynxes",
        vec![
            x_aux("init", "5"),
            x_stock("lynxes_stock", "100 * init", &["inflow"], &[]),
            x_flow("inflow", "1"),
        ],
    );
    let hares_model = x_model(
        "hares",
        vec![
            x_aux("lynxes", "0"),
            x_stock("hares_stock", "100", &[], &["outflow"]),
            x_flow("outflow", ".1 * hares_stock"),
        ],
    );
    let main_model = x_model(
        "main",
        vec![
            x_aux("area", "time"),
            x_module("lynxes", &[]),
            x_module(
                "hares",
                &[
                    ("area", "hares.area"),
                    ("lynxes.lynxes_stock", "hares.lynxes"),
                ],
            ),
        ],
    );

    let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = vec![
        ("main".to_string(), &main_model),
        ("lynxes".to_string(), &lynxes_model),
        ("hares".to_string(), &hares_model),
    ]
    .into_iter()
    .map(|(name, m)| build_xvars_map(name, m))
    .collect();

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let actual = parse_var(
        &models,
        "main",
        &[],
        models["main"]["hares"],
        &mut implicit_vars,
    );
    assert!(actual.errors().is_none());
    assert!(implicit_vars.is_empty());
    assert_eq!(expected, actual);
}

pub fn build_xvars_map(
    name: Ident,
    m: &datamodel::Model,
) -> (Ident, HashMap<Ident, &datamodel::Variable>) {
    (
        canonicalize(&name),
        m.variables
            .iter()
            .map(|v| (v.get_ident().to_string(), v))
            .collect(),
    )
}

#[test]
fn test_errors() {
    let main_model = x_model("main", vec![x_aux("aux_3", "unknown_variable * 3.14")]);
    let models: HashMap<String, HashMap<Ident, &datamodel::Variable>> =
        vec![("main".to_string(), &main_model)]
            .into_iter()
            .map(|(name, m)| build_xvars_map(name, m))
            .collect();

    let model = {
        let mut model = Model::new(&models, &main_model, &[], false);
        model.set_dependencies(&HashMap::new(), &[], &BTreeSet::new());
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
        models: &HashMap<Ident, &Model>,
        module_inputs: Option<&[ModuleInput]>,
    ) {
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
            module_inputs,
            dimensions: &[],
        };
        let deps = all_deps(&ctx, all_vars.iter()).unwrap();

        if expected_deps != deps {
            let failed_dep_order: Vec<_> = all_vars.iter().map(|v| v.ident()).collect();
            eprintln!("failed order: {:?}", failed_dep_order);
            for (v, expected) in expected_deps_list.iter() {
                eprintln!("{}", v.ident());
                let mut expected: Vec<_> = expected.iter().cloned().collect();
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
                module_inputs,
                dimensions: &[],
            };
            let deps = all_deps(&ctx, all_vars.iter()).unwrap();
            assert_eq!(expected_deps, deps);
        }
    }

    let mod_1_model = x_model(
        "mod_1",
        vec![
            x_aux("input", "{expects to be set with module input}"),
            x_aux("output", "3 * TIME"),
            x_aux("flow", "2 * input"),
            x_stock("output_2", "input", &["flow"], &[]),
        ],
    );

    let main_model = x_model(
        "main",
        vec![
            x_module("mod_1", &[("aux_3", "mod_1.input")]),
            x_aux("aux_3", "6"),
            x_flow("inflow", "mod_1.flow"),
            x_aux("aux_4", "mod_1.output"),
        ],
    );
    let x_models: HashMap<String, HashMap<Ident, &datamodel::Variable>> = vec![
        ("mod_1".to_owned(), &mod_1_model),
        ("main".to_owned(), &main_model),
    ]
    .into_iter()
    .map(|(name, m)| build_xvars_map(name, m))
    .collect();

    let mut model_list = vec!["mod_1", "main"]
        .into_iter()
        .map(|name| {
            let vars = &x_models[name];
            let x_model = datamodel::Model {
                name: name.to_owned(),
                variables: vars.values().cloned().cloned().collect(),
                views: vec![],
            };
            Model::new(&x_models, &x_model, &[], false)
        })
        .collect::<Vec<_>>();

    let models = {
        let mut models: HashMap<Ident, &Model> = HashMap::new();
        for model in model_list.iter_mut() {
            model.set_dependencies(&models, &[], &BTreeSet::new());
            models.insert(model.name.clone(), model);
        }
        models
    };

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let mod_1 = parse_var(
        &x_models,
        "main",
        &[],
        x_models["main"]["mod_1"],
        &mut implicit_vars,
    );
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

    verify_all_deps(
        &expected_deps_list,
        true,
        &models,
        Some(&[ModuleInput {
            src: "doesnt_matter".to_string(),
            dst: "aux_true".to_string(),
        }]),
    );

    // test non-existant variables
}
