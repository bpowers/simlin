// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::capture::CaptureKind;
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImplicitVarDeps {
    pub name: String,
    pub is_module: bool,
    pub model_name: Option<String>,
    /// The phase demand of a `PREVIOUS`/`INIT` capture, which decides its
    /// runlists (`db::dep_graph::model_dependency_graph`); `None` for a
    /// hoisted argument or a module instance, which are per-step definitions
    /// like any explicit aux.
    pub capture_kind: Option<CaptureKind>,
    pub dt_deps: BTreeSet<String>,
    pub initial_deps: BTreeSet<String>,
    /// Names read through `INIT` in the helper's own equation. Mirrors
    /// `VariableDeps::init_referenced_vars`: they seed the initials runlist,
    /// so the frozen snapshot holds them whether or not the helper itself
    /// runs in initials (a `PREVIOUS` capture whose body reads `INIT(x)`).
    pub init_referenced_vars: BTreeSet<String>,
    pub dt_init_only_referenced_vars: BTreeSet<String>,
    pub dt_previous_referenced_vars: BTreeSet<String>,
    pub initial_previous_referenced_vars: BTreeSet<String>,
    /// Lookup tables referenced via `LOOKUP(table, x)` -- layout references, not
    /// data-flow deps. Kept out of `dt_deps`/`initial_deps` (no ordering/causal
    /// edge) but needed by the implicit-var fragment compiler's metadata +
    /// tables map (issue #606). Mirrors `VariableDeps::referenced_tables`.
    pub referenced_tables: BTreeSet<String>,
}

/// `dim_context` and `converted_dims` are two views of the *same* project
/// dimension list (context form and converted form); the caller sources both
/// from the per-project salsa caches, so they are consistent by construction.
/// Passing inconsistent views would silently misclassify dependencies.
pub(super) fn extract_implicit_var_deps(
    parsed: &ParsedVariableResult,
    dim_context: &crate::dimensions::DimensionsContext,
    converted_dims: &[crate::dimensions::Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> Vec<ImplicitVarDeps> {
    if parsed.implicit_vars.is_empty() {
        return Vec::new();
    }

    parsed
        .implicit_vars
        .iter()
        .map(|implicit_var| {
            let implicit_name = canonicalize(implicit_var.ident()).into_owned();

            // Module-type implicit vars have no AST -- extract deps from
            // their module reference src fields instead.
            if let Some(m) = implicit_var.module() {
                let refs: BTreeSet<String> = m
                    .references
                    .iter()
                    .map(|mr| canonicalize(&mr.src).into_owned())
                    .collect();
                return ImplicitVarDeps {
                    name: implicit_name,
                    is_module: true,
                    model_name: Some(m.model_name.clone()),
                    capture_kind: None,
                    dt_deps: refs.clone(),
                    initial_deps: refs,
                    init_referenced_vars: BTreeSet::new(),
                    dt_init_only_referenced_vars: BTreeSet::new(),
                    dt_previous_referenced_vars: BTreeSet::new(),
                    initial_previous_referenced_vars: BTreeSet::new(),
                    // A module never references a lookup table via LOOKUP(...).
                    referenced_tables: BTreeSet::new(),
                };
            }

            let parsed_implicit = implicit_var.parsed_variable(dim_context);

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
                    crate::variable::classify_dependencies(ast, converted_dims, module_inputs)
                }
                None => crate::variable::DepClassification::default(),
            };
            let init_classification = match lowered.init_ast() {
                Some(ast) => {
                    crate::variable::classify_dependencies(ast, converted_dims, module_inputs)
                }
                None => crate::variable::DepClassification::default(),
            };

            ImplicitVarDeps {
                name: implicit_name,
                // The module arm returned above, so nothing here is one.
                is_module: false,
                model_name: None,
                capture_kind: implicit_var.capture().map(|c| c.kind()),
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
                init_referenced_vars: dt_classification.init_referenced,
                dt_init_only_referenced_vars: dt_classification.init_only,
                dt_previous_referenced_vars: dt_classification.previous_only,
                initial_previous_referenced_vars: init_classification.previous_only,
                referenced_tables: dt_classification
                    .referenced_tables
                    .into_iter()
                    .chain(init_classification.referenced_tables)
                    .collect(),
            }
        })
        .collect()
}
