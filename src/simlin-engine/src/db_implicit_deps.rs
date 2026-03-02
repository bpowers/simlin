// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq, salsa::Update)]
pub struct ImplicitVarDeps {
    pub name: String,
    pub is_stock: bool,
    pub is_module: bool,
    pub model_name: Option<String>,
    pub dt_deps: BTreeSet<String>,
    pub initial_deps: BTreeSet<String>,
    pub dt_init_only_referenced_vars: BTreeSet<String>,
    pub dt_previous_referenced_vars: BTreeSet<String>,
    pub initial_previous_referenced_vars: BTreeSet<String>,
}

pub(super) fn extract_implicit_var_deps(
    parsed: &ParsedVariableResult,
    dims: &[datamodel::Dimension],
    dim_context: &crate::dimensions::DimensionsContext,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> Vec<ImplicitVarDeps> {
    if parsed.implicit_vars.is_empty() {
        return Vec::new();
    }

    let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap_or_default();
    let converted_dims: Vec<crate::dimensions::Dimension> = dims
        .iter()
        .map(crate::dimensions::Dimension::from)
        .collect();

    parsed
        .implicit_vars
        .iter()
        .map(|implicit_var| {
            let implicit_name = canonicalize(implicit_var.get_ident()).into_owned();
            let is_module = matches!(implicit_var, datamodel::Variable::Module(_));
            let model_name = match implicit_var {
                datamodel::Variable::Module(m) => Some(m.model_name.clone()),
                _ => None,
            };

            // Module-type implicit vars have no AST -- extract deps from
            // their module reference src fields instead.
            if let datamodel::Variable::Module(m) = implicit_var {
                let refs: BTreeSet<String> = m
                    .references
                    .iter()
                    .map(|mr| canonicalize(&mr.src).into_owned())
                    .collect();
                return ImplicitVarDeps {
                    name: implicit_name,
                    is_stock: false,
                    is_module: true,
                    model_name: Some(m.model_name.clone()),
                    dt_deps: refs.clone(),
                    initial_deps: refs,
                    dt_init_only_referenced_vars: BTreeSet::new(),
                    dt_previous_referenced_vars: BTreeSet::new(),
                    initial_previous_referenced_vars: BTreeSet::new(),
                };
            }

            let mut dummy_implicits = Vec::new();
            let parsed_implicit = crate::variable::parse_var(
                dims,
                implicit_var,
                &mut dummy_implicits,
                &units_ctx,
                |mi| Ok(Some(mi.clone())),
            );

            let models = HashMap::new();
            let scope = crate::model::ScopeStage0 {
                models: &models,
                dimensions: dim_context,
                model_name: "",
            };
            let lowered = crate::model::lower_variable(&scope, &parsed_implicit);

            let dt = match lowered.ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, module_inputs)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };
            let initial = match lowered.init_ast() {
                Some(ast) => crate::variable::identifier_set(ast, &converted_dims, module_inputs)
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                None => BTreeSet::new(),
            };
            let dt_init_only_referenced_vars = match lowered.ast() {
                Some(ast) => crate::variable::init_only_referenced_idents_with_module_inputs(
                    ast,
                    module_inputs,
                ),
                None => BTreeSet::new(),
            };
            let dt_previous_referenced_vars = match lowered.ast() {
                Some(ast) => crate::variable::lagged_only_previous_idents_with_module_inputs(
                    ast,
                    module_inputs,
                ),
                None => BTreeSet::new(),
            };
            let initial_previous_referenced_vars = match lowered.init_ast() {
                Some(ast) => crate::variable::lagged_only_previous_idents_with_module_inputs(
                    ast,
                    module_inputs,
                ),
                None => BTreeSet::new(),
            };

            ImplicitVarDeps {
                name: implicit_name,
                is_stock: parsed_implicit.is_stock(),
                is_module,
                model_name,
                dt_deps: dt,
                initial_deps: initial,
                dt_init_only_referenced_vars,
                dt_previous_referenced_vars,
                initial_previous_referenced_vars,
            }
        })
        .collect()
}
