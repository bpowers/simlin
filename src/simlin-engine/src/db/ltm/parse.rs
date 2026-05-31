// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Parsing and `datamodel::Equation`-shaping helpers for LTM synthetic
//! variables.
//!
//! These functions turn an LTM synthetic variable's equation text into a
//! parsed (and optionally lowered) `Variable`, and convert between the
//! scalar / `ApplyToAll` / `Arrayed` equation variants the rest of the LTM
//! pipeline produces and consumes.

use std::collections::{HashMap, HashSet};

use crate::canonicalize;
use crate::common::{Canonical, Ident};
use crate::datamodel;

use crate::db::{Db, ParsedVariableResult, SourceModel, SourceProject};
use crate::db::{project_datamodel_dims, project_dimensions_context, project_units_context};

use super::ltm_module_idents;

/// Parse an LTM synthetic variable's equation.
///
/// Creates a transient `datamodel::Variable::Aux` carrying `equation`
/// verbatim, runs it through `parse_var` (which invokes `BuiltinVisitor`
/// and `instantiate_implicit_modules`), and returns the parsed variable
/// plus any implicit helper/module variables generated while parsing.
///
/// `equation` already carries its own dimensionality: `Equation::Scalar`
/// for scalar LTM vars, `Equation::ApplyToAll` for A2A vars (so the
/// compiler expands the formula across all dimension elements), and
/// `Equation::Arrayed` for per-element link-score equations -- the
/// `datamodel::Equation` -> `Ast` conversion in `variable.rs` produces
/// the matching `Ast` variant for each.
pub(super) fn parse_ltm_equation(
    var_name: &str,
    equation: &datamodel::Equation,
    dims: &[datamodel::Dimension],
    units_ctx: &crate::units::Context,
    module_idents: Option<&HashSet<Ident<Canonical>>>,
) -> ParsedVariableResult {
    let dm_var = datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize(var_name).into_owned(),
        equation: equation.clone(),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let mut implicit_vars = Vec::new();
    let variable = crate::variable::parse_var_with_module_context(
        dims,
        &dm_var,
        &mut implicit_vars,
        units_ctx,
        |mi| Ok(Some(mi.clone())),
        module_idents,
        // LTM synthetic equations (link/loop scores) are engine-generated
        // and never contain user macro invocations -> no registry needed.
        None,
        // ...and are never a macro body, so no #554 enclosing-macro context.
        None,
    );

    ParsedVariableResult {
        variable,
        implicit_vars,
    }
}

pub(super) fn parse_ltm_equation_for_model_with_ids(
    db: &dyn Db,
    var_name: &str,
    equation: &datamodel::Equation,
    project: SourceProject,
    module_idents: &HashSet<Ident<Canonical>>,
) -> ParsedVariableResult {
    let dims = project_datamodel_dims(db, project);
    let units_ctx = project_units_context(db, project);
    parse_ltm_equation(var_name, equation, dims, units_ctx, Some(module_idents))
}

pub(crate) fn parse_ltm_var_with_ids(
    db: &dyn Db,
    ltm_var: &crate::db::LtmSyntheticVar,
    project: SourceProject,
    module_idents: &HashSet<Ident<Canonical>>,
) -> ParsedVariableResult {
    parse_ltm_equation_for_model_with_ids(
        db,
        &ltm_var.name,
        &ltm_var.equation,
        project,
        module_idents,
    )
}

/// Parse *and lower* an LTM equation to a `Variable<ModuleInput, Expr2>`
/// (the type `classify_reducer` and the rest of the type-checked AST machinery
/// expect). Mirrors the parse-then-lower boilerplate `compile_ltm_equation_fragment`
/// does; scoped with an empty model set and the project's datamodel dimensions.
/// Returns `None` if the equation fails to parse.
pub(super) fn reconstruct_ltm_var_lowered(
    db: &dyn Db,
    var_name: &str,
    equation: &datamodel::Equation,
    model: SourceModel,
    project: SourceProject,
) -> Option<crate::variable::Variable> {
    let dim_context = project_dimensions_context(db, project);
    let module_idents = ltm_module_idents(db, model, project);
    let parsed =
        parse_ltm_equation_for_model_with_ids(db, var_name, equation, project, &module_idents);
    if parsed
        .variable
        .equation_errors()
        .is_some_and(|e| !e.is_empty())
    {
        return None;
    }
    let models = HashMap::new();
    let scope = crate::model::ScopeStage0 {
        models: &models,
        dimensions: dim_context,
        model_name: "",
    };
    Some(crate::model::lower_variable(&scope, &parsed.variable))
}

/// The dimension names an LTM `Equation` carries (datamodel casing),
/// or `&[]` for a scalar one. These are the names whose product gives
/// the variable's layout slot count, and the same names `compute_layout`
/// reads from `LtmSyntheticVar::dimensions` -- the two are kept in sync
/// at every `LtmSyntheticVar` construction site.
pub(super) fn ltm_equation_dimensions(equation: &datamodel::Equation) -> &[String] {
    match equation {
        datamodel::Equation::Scalar(_) => &[],
        datamodel::Equation::ApplyToAll(dims, _) | datamodel::Equation::Arrayed(dims, _, _, _) => {
            dims
        }
    }
}

/// Build the `Equation` variant an `LtmSyntheticVar` should carry given
/// its synthetic equation *text* and dimension list: empty `dimensions`
/// ⇒ `Equation::Scalar`, non-empty ⇒ `Equation::ApplyToAll` over exactly
/// those names. (`Equation::Arrayed` link-score equations are built
/// directly by the augmentation layer, not via this helper.)
pub(super) fn ltm_synthetic_equation(text: String, dimensions: &[String]) -> datamodel::Equation {
    if dimensions.is_empty() {
        datamodel::Equation::Scalar(text)
    } else {
        datamodel::Equation::ApplyToAll(dimensions.to_vec(), text)
    }
}

/// Reduce an LTM equation to a scalar one, keeping the equation text.
/// Used by the legacy `(from, to)`-keyed link-score path
/// (`link_score_equation_text`), which always emits a scalar variable
/// regardless of the target's dimensionality, and by
/// [`retarget_ltm_equation_dims`] to collapse a degenerate zero-dimension
/// `Arrayed` (the empty-`dims` case) -- a per-element link score with no
/// dimension to index is meaningless, so it falls back to scalar. For an
/// `Equation::Arrayed`, the first per-element slot's text is used (the
/// legacy path predates per-element link scores and is only ever compiled
/// for scalar targets in practice).
pub(crate) fn scalarize_ltm_equation(equation: datamodel::Equation) -> datamodel::Equation {
    match equation {
        datamodel::Equation::Scalar(_) => equation,
        datamodel::Equation::ApplyToAll(_, text) => datamodel::Equation::Scalar(text),
        datamodel::Equation::Arrayed(_, elements, default, _) => {
            let text = elements
                .into_iter()
                .next()
                .map(|(_, eqn, _, _)| eqn)
                .or(default)
                .unwrap_or_else(|| "0".to_string());
            datamodel::Equation::Scalar(text)
        }
    }
}

/// Re-tag a link-score `Equation` so its dimension names match `dims`
/// (the link-score-dimensions policy result the emission loop assigned to
/// `LtmSyntheticVar::dimensions`). Empty `dims` collapses the equation to
/// `Scalar` (via [`scalarize_ltm_equation`] for an `Arrayed` input, since
/// a zero-dimension `Arrayed` is degenerate); non-empty `dims` widens a
/// scalar to `ApplyToAll` or re-targets the dimension names of an existing
/// `ApplyToAll`/`Arrayed`, preserving the equation text / per-element
/// formulas verbatim.
pub(super) fn retarget_ltm_equation_dims(
    equation: datamodel::Equation,
    dims: &[String],
) -> datamodel::Equation {
    use datamodel::Equation::{ApplyToAll, Arrayed, Scalar};
    match equation {
        Scalar(text) | ApplyToAll(_, text) => {
            if dims.is_empty() {
                Scalar(text)
            } else {
                ApplyToAll(dims.to_vec(), text)
            }
        }
        Arrayed(orig_dims, elements, default, apply_default) => {
            if dims.is_empty() {
                // The link-score-dimensions policy assigned no dimensions:
                // the (rare) arrayed-target edge whose source dimensions
                // are incompatible with the target's (so the per-shape A2A
                // path was skipped and the cross-dimensional path declined
                // it for having a non-scalar target). A zero-dimension
                // `Arrayed` is degenerate -- its per-element partials are
                // meaningless without a target dimension to index -- so
                // collapse to a scalar link score, matching the contract
                // in this function's doc comment and the pre-existing
                // behavior where such edges produced a scalar link score.
                scalarize_ltm_equation(Arrayed(orig_dims, elements, default, apply_default))
            } else {
                Arrayed(dims.to_vec(), elements, default, apply_default)
            }
        }
    }
}
