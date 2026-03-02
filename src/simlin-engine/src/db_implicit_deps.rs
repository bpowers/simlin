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

            // Two calls to classify_dependencies replace 5 separate walker calls.
            let dt_classification = match lowered.ast() {
                Some(ast) => {
                    crate::variable::classify_dependencies(ast, &converted_dims, module_inputs)
                }
                None => crate::variable::DepClassification::default(),
            };
            let init_classification = match lowered.init_ast() {
                Some(ast) => {
                    crate::variable::classify_dependencies(ast, &converted_dims, module_inputs)
                }
                None => crate::variable::DepClassification::default(),
            };

            ImplicitVarDeps {
                name: implicit_name,
                is_stock: parsed_implicit.is_stock(),
                is_module,
                model_name,
                dt_deps: dt_classification
                    .all
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                initial_deps: init_classification
                    .all
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect(),
                dt_init_only_referenced_vars: dt_classification.init_only,
                dt_previous_referenced_vars: dt_classification.previous_only,
                initial_previous_referenced_vars: init_classification.previous_only,
            }
        })
        .collect()
}
