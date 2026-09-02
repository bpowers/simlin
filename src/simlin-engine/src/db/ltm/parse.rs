// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Typed parsing helpers for LTM synthetic variables.
//!
//! LTM synthetic equations are stored as a typed [`LtmEquation`] (a parsed
//! `Expr0` AST plus its diagnostic text), so these helpers do NOT re-parse
//! source text: they build the flow-phase `Ast<Expr0>` from the already-parsed
//! arms (resolving the dimension names) and run the SAME implicit-module /
//! PREVIOUS-INIT-helper visitor (`instantiate_implicit_modules`) the ordinary
//! variable parse runs. This replaces the former `parse_ltm_equation` transient
//! `datamodel::Variable::Aux` round trip -- printing an equation to text and
//! parsing it back to compile it -- which ran 2-3 times per equation (GH #655
//! finding 3) and coupled LTM compilation to the printer/parser asymmetry class
//! (GH #913). The `datamodel::Equation`-shaping helpers moved onto `LtmEquation`
//! itself (`scalarize`, `retarget_dims`, `dimensions`); the thin wrappers below
//! keep the call sites' free-function spelling.

use std::collections::HashSet;

use crate::builtins_visitor::{
    SnapshotIndexFacts, empty_macro_registry, instantiate_implicit_modules,
};
use crate::common::{Canonical, Ident};
use crate::dimensions::DimensionsContext;

use crate::db::ParsedVariableResult;

use super::LtmEquation;

/// Parse an LTM synthetic variable's typed equation into a
/// `Variable<ModuleInput, Expr0>` plus any implicit helper/module variables the
/// PREVIOUS/INIT and stdlib-module expansion visitor synthesizes.
///
/// The equation is already a parsed AST (`LtmEquation`), so this only resolves
/// its dimension names to `Dimension`s (building the flow-phase `Ast<Expr0>`)
/// and runs `instantiate_implicit_modules` -- the exact visitor the ordinary
/// `variable::parse_var` runs -- so PREVIOUS/INIT capture
/// auxes and stdlib module calls expand identically.
///
/// Flow-phase only: LTM synthetic variables are scalar auxes (never stocks),
/// compiled in the flow phase, and their init phase would only re-run the
/// visitor over the same arm bodies, synthesizing the *same-named* helpers that
/// dedup away (`model_ltm_implicit_var_info` keys implicit vars by canonical
/// name), so it is skipped.
///
/// `model_var_names` is the model's whole variable-name set, the generated
/// path's rule for a bare element subscript of a `PREVIOUS`/`INIT` argument
/// (`SnapshotIndexFacts::ModelNames`); every LTM parse site passes
/// `ltm_model_var_names` so the helper set and the compiled helpers agree.
pub(super) fn parse_ltm_equation(
    var_name: &str,
    equation: &LtmEquation,
    dims: &DimensionsContext,
    model_var_names: &HashSet<Ident<Canonical>>,
) -> ParsedVariableResult {
    let (flow_ast, mut errors) = equation.to_flow_ast(dims);

    let mut implicit_vars = Vec::new();
    let ast = match flow_ast {
        Some(ast) => match instantiate_implicit_modules(
            var_name,
            ast,
            Some(dims),
            SnapshotIndexFacts::ModelNames(model_var_names),
            // LTM synthetic equations are engine-generated and never contain
            // user macro invocations -> no registry needed; and are never a
            // macro body, so no enclosing-macro context (#554).
            empty_macro_registry(),
            None,
        ) {
            Ok((ast, mut new_vars)) => {
                implicit_vars.append(&mut new_vars);
                Some(ast)
            }
            Err(err) => {
                errors.push(err);
                None
            }
        },
        None => None,
    };

    let variable = crate::variable::Variable {
        ident: Ident::new(var_name),
        units: None,
        eqn: None,
        errors,
        unit_errors: vec![],
        kind: crate::variable::VarKind::Aux {
            ast,
            init_ast: None,
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
        },
    };

    ParsedVariableResult {
        variable,
        implicit_vars,
    }
}

/// The dimension names an LTM `LtmEquation` carries (datamodel casing),
/// or `&[]` for a scalar one. Thin wrapper over [`LtmEquation::dimensions`]
/// so the call sites keep their free-function spelling.
pub(super) fn ltm_equation_dimensions(equation: &LtmEquation) -> &[String] {
    equation.dimensions()
}

/// Reduce an LTM equation to a scalar one. Thin wrapper over
/// [`LtmEquation::scalarize`].
pub(crate) fn scalarize_ltm_equation(equation: LtmEquation) -> LtmEquation {
    equation.scalarize()
}

/// Re-tag a link-score `LtmEquation`'s dimension names to `dims`. Thin
/// wrapper over [`LtmEquation::retarget_dims`].
pub(super) fn retarget_ltm_equation_dims(equation: LtmEquation, dims: &[String]) -> LtmEquation {
    equation.retarget_dims(dims)
}
