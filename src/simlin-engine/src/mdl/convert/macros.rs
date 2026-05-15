// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Macro-definition conversion: turn each parsed `MacroDef` into a
//! macro-marked `datamodel::Model`.
//!
//! A macro body is a little model with its own variable namespace, so it is
//! converted with a *scoped* [`ConversionContext`] (Task 4's `new_from_items`,
//! sharing the parent's already-built dimensions / data provider / formatter)
//! rather than the global symbol table. The format-agnostic port-synthesis +
//! `MacroSpec` construction lives in `datamodel::Model::new_macro`, which the
//! XMILE reader reuses; only the step that builds the body variable list from
//! `MacroDef.equations` is MDL-specific and lives here.

use std::borrow::Cow;

use crate::datamodel::Model;

use crate::mdl::ast::{Equation as MdlEquation, Expr, FullEquation, Lhs, Loc, MdlItem};

use super::ConversionContext;
use super::helpers::{macro_param_ident, rewrite_dollar_time, variable_ident};
use super::types::ConvertError;

impl<'input> ConversionContext<'input> {
    /// Build a macro-marked [`Model`] for every `MdlItem::Macro` in `self`.
    ///
    /// `collect_symbols` deliberately leaves the `MdlItem::Macro` entries in
    /// `self.items` (it only declines to register them as *main-model*
    /// symbols), so this pass consumes them here. Single-output macro
    /// *invocations* are not materialized -- they stay as ordinary equation
    /// text in their `Aux`/`Stock`/`Flow` and are expanded in a later phase.
    /// Definition order is irrelevant: every `MdlItem` was collected before
    /// `convert()` ran, so a macro defined after its call site still produces
    /// a model.
    pub(super) fn build_macro_models(&self) -> Result<Vec<Model>, ConvertError> {
        let mut models = Vec::new();

        for item in &self.items {
            let MdlItem::Macro(macro_def) = item else {
                continue;
            };

            // The macro name and its formal parameter / additional-output
            // idents are canonicalized to the variable-ident form, so they
            // are byte-identical to the body variables they name (the macro
            // name names the body's primary-output equation) and to the
            // synthesized port-variable idents.
            let macro_name = variable_ident(&macro_def.name);

            let mut parameters = Vec::with_capacity(macro_def.args.len());
            for arg in &macro_def.args {
                let ident = macro_param_ident(arg).ok_or_else(|| {
                    ConvertError::Other(format!(
                        "macro {:?} has a non-variable formal parameter",
                        macro_def.name
                    ))
                })?;
                parameters.push(ident);
            }

            let mut additional_outputs = Vec::with_capacity(macro_def.outputs.len());
            for out in &macro_def.outputs {
                let ident = macro_param_ident(out).ok_or_else(|| {
                    ConvertError::Other(format!(
                        "macro {:?} has a non-variable additional output",
                        macro_def.name
                    ))
                })?;
                additional_outputs.push(ident);
            }

            let body_variables =
                self.build_macro_body_variables(&macro_name, &parameters, &macro_def.equations)?;

            models.push(Model::new_macro(
                &macro_name,
                &parameters,
                &additional_outputs,
                body_variables,
            ));
        }

        Ok(models)
    }

    /// Convert a macro body's equations into a `datamodel::Variable` list via
    /// a scoped [`ConversionContext`] that reuses the parent's dimensions,
    /// data provider, and formatter.
    ///
    /// A synthetic `<param> = 0` equation is prepended for every formal
    /// parameter so the existing pipeline assigns each port the correct
    /// stock/flow/aux kind itself: e.g. for `EXPRESSION MACRO = INTEG(input,
    /// parameter)`, `link_stocks_and_flows`'s rate decomposition only treats
    /// `input` as a flow if `input` is a known symbol (an undefined name
    /// fails decomposition and a net flow is synthesized instead). The
    /// placeholder equations make `input`/`parameter` known, so `input`
    /// becomes a `Flow` (the INTEG rate) and `parameter` an `Aux` (the INTEG
    /// initial). `Model::new_macro` then flips `can_be_module_input` on these
    /// pipeline-built ports without disturbing their kind.
    fn build_macro_body_variables(
        &self,
        macro_name: &str,
        parameters: &[String],
        equations: &[FullEquation<'input>],
    ) -> Result<Vec<crate::datamodel::Variable>, ConvertError> {
        let mut items: Vec<MdlItem<'input>> =
            Vec::with_capacity(parameters.len() + equations.len());

        for param in parameters {
            items.push(synthetic_param_equation(param));
        }
        for eq in equations {
            let mut eq = eq.clone();
            // Translate `$`-suffixed time references (`Time$`, `TIME STEP$`,
            // ...) to canonical engine time idents *before* the scoped
            // conversion formats the equation. Scoped to macro bodies only.
            rewrite_equation_dollar_time(&mut eq.equation);
            items.push(MdlItem::Equation(Box::new(eq)));
        }

        // The sub-context shares the parent's already-built dimensions,
        // data provider, and formatter (the macro body defines no dimensions
        // of its own, and the formatter's subrange-name state must match the
        // parent's so body-equation formatting is identical). The remaining
        // settings-derived inputs are irrelevant to a macro body.
        let mut ctx = ConversionContext::new_from_items(
            items,
            self.dimensions.clone(),
            self.formatter.clone(),
            self.data_provider,
            super::types::SimSpecsBuilder::default(),
            crate::datamodel::SimMethod::default(),
            Vec::new(),
            Vec::new(),
            std::collections::HashMap::new(),
        );

        ctx.collect_symbols();
        ctx.mark_variable_types();
        ctx.scan_for_extrapolate_lookups();
        ctx.link_stocks_and_flows();
        // A macro body cannot contain a multi-output macro invocation, so the
        // scoped sub-context builds with an empty materialization.
        let model = ctx.build_model(macro_name, &Default::default())?;

        Ok(model.variables)
    }
}

/// Apply [`rewrite_dollar_time`] to every `Expr` carried by a macro body
/// `MdlEquation` (scoped to macro bodies only -- the global formatter and
/// non-macro equations are untouched).
fn rewrite_equation_dollar_time(eq: &mut MdlEquation<'_>) {
    match eq {
        MdlEquation::Regular(_, expr) => rewrite_dollar_time(expr),
        MdlEquation::WithLookup(_, input, _) => rewrite_dollar_time(input),
        MdlEquation::Data(_, Some(expr)) => rewrite_dollar_time(expr),
        MdlEquation::EmptyRhs(_, _)
        | MdlEquation::Implicit(_)
        | MdlEquation::Lookup(_, _)
        | MdlEquation::Data(_, None)
        | MdlEquation::TabbedArray(_, _)
        | MdlEquation::NumberList(_, _)
        | MdlEquation::SubscriptDef(_, _)
        | MdlEquation::Equivalence(_, _, _) => {}
    }
}

/// Build a synthetic `<name> = 0` equation item used as a placeholder for a
/// macro formal parameter (overridden by the call-site argument at every
/// invocation; only ever evaluated if the port is left unwired).
fn synthetic_param_equation<'input>(name: &str) -> MdlItem<'input> {
    let loc = Loc::new(0, 0);
    let lhs = Lhs {
        name: Cow::Owned(name.to_string()),
        subscripts: vec![],
        except: None,
        interp_mode: None,
        loc,
    };
    let eq = MdlEquation::Regular(lhs, Expr::Const(0.0, loc));
    MdlItem::Equation(Box::new(FullEquation {
        equation: eq,
        units: None,
        comment: None,
        supplementary: false,
        loc,
    }))
}
