// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::capture::CaptureKind;
use crate::db::query::DepScope;
use std::collections::BTreeSet;

/// The reads of one helper a variable's parse synthesized, beside the
/// helper's identity; the dependency representation is [`DepRefs`], the
/// same one the parent carries.
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
    /// Every read of the helper: its body's, or a module instance's input
    /// sources.
    pub deps: DepRefs,
    /// Lookup tables referenced via `LOOKUP(table, x)` -- layout references, not
    /// data-flow deps. Kept out of `deps` (no ordering/causal edge) but needed
    /// by the implicit-var fragment compiler's metadata + tables map (issue
    /// #606). Mirrors `VariableDeps::referenced_tables`.
    pub referenced_tables: BTreeSet<Ident<Canonical>>,
}

/// `dim_context` and `converted_dims` are two views of the *same* project
/// dimension list (context form and converted form); the caller sources both
/// from the per-project salsa caches, so they are consistent by construction.
/// Passing inconsistent views would silently misclassify dependencies.
pub(super) fn extract_implicit_var_deps(
    scope: &DepScope<'_>,
    parsed: &ParsedVariableResult,
    dim_context: &crate::dimensions::DimensionsContext,
    converted_dims: &[crate::dimensions::Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> Vec<ImplicitVarDeps> {
    parsed
        .implicit_vars
        .iter()
        .map(|implicit_var| {
            let implicit_name = canonicalize(implicit_var.ident()).into_owned();

            // A module instance has no AST: its reads are its input sources.
            if let Some(m) = implicit_var.module() {
                return ImplicitVarDeps {
                    name: implicit_name,
                    is_module: true,
                    model_name: Some(m.model_name.clone()),
                    capture_kind: None,
                    deps: scope
                        .module_input_deps(m.references.iter().map(|mr| Ident::new(&mr.src))),
                    // A module never references a lookup table via LOOKUP(...).
                    referenced_tables: BTreeSet::new(),
                };
            }

            let helper = implicit_var.parsed_variable(dim_context);
            let (deps, referenced_tables) =
                scope.parsed_variable_deps(&helper, dim_context, converted_dims, module_inputs);
            ImplicitVarDeps {
                name: implicit_name,
                // The module arm returned above, so nothing here is one.
                is_module: false,
                model_name: None,
                capture_kind: implicit_var.capture().map(|c| c.kind()),
                deps,
                referenced_tables,
            }
        })
        .collect()
}
