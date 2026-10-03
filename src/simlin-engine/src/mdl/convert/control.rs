// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The control variables: `INITIAL TIME`, `FINAL TIME`, `TIME STEP` and
//! `SAVEPER`. A Vensim model defines each with an equation; the datamodel
//! keeps them as the project's sim specs, which are numbers.
//!
//! An equation that is a constant is folded to its number: a literal, or
//! arithmetic over literals, the other control variables and the model's own
//! constants (`FINAL TIME = INITIAL TIME + 50`, `FINAL TIME = max time`).
//! One that is not (`FINAL TIME = IF THEN ELSE(Time < 5, 10, 50)`, which
//! Vensim re-evaluates as the run goes) has no number: the spec takes its
//! default and the reader reports the equation as not kept
//! ([`ConversionContext::control_losses`]), since a save then writes the
//! default in its place.

use crate::datamodel::Dt;
use crate::errors::join_quoted_names;
use crate::import_losses::ImportWarning;
use crate::mdl::ast::{BinaryOp, Equation as MdlEquation, Expr, UnaryOp};
use crate::mdl::builtins::eq_lower_space;

use super::ConversionContext;
use super::helpers::canonical_name;

/// A control variable.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Control {
    InitialTime,
    FinalTime,
    TimeStep,
    Saveper,
}

impl Control {
    pub(crate) const ALL: [Control; 4] = [
        Control::InitialTime,
        Control::FinalTime,
        Control::TimeStep,
        Control::Saveper,
    ];

    /// The variable's name as Vensim writes it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Control::InitialTime => "INITIAL TIME",
            Control::FinalTime => "FINAL TIME",
            Control::TimeStep => "TIME STEP",
            Control::Saveper => "SAVEPER",
        }
    }

    /// The symbol table's key for it.
    pub(super) fn key(self) -> &'static str {
        match self {
            Control::InitialTime => "initial time",
            Control::FinalTime => "final time",
            Control::TimeStep => "time step",
            Control::Saveper => "saveper",
        }
    }

    /// The control variable `name` names, in any case and spacing.
    pub(crate) fn named(name: &str) -> Option<Control> {
        Control::ALL
            .into_iter()
            .find(|control| eq_lower_space(name, control.key()))
    }
}

/// How deep a chain of constants a control equation may read through. A
/// model's constants reference each other a few deep at most; the bound keeps
/// a definition cycle from recursing without end.
const MAX_DEPTH: usize = 32;

impl<'input> ConversionContext<'input> {
    /// The number `expr` is, when it is a constant: a literal, or arithmetic
    /// over literals and over variables whose own equations are constants.
    /// None for anything a run could change (`Time`, a function call, a
    /// subscripted or undefined variable) and for a result that is not finite.
    pub(super) fn constant_value(&self, expr: &Expr<'_>) -> Option<f64> {
        self.fold(expr, 0).filter(|v| v.is_finite())
    }

    fn fold(&self, expr: &Expr<'_>, depth: usize) -> Option<f64> {
        if depth > MAX_DEPTH {
            return None;
        }
        match expr {
            Expr::Const(v, _) => Some(*v),
            Expr::Paren(inner, _) => self.fold(inner, depth),
            Expr::Op1(UnaryOp::Negative, inner, _) => self.fold(inner, depth).map(|v| -v),
            Expr::Op1(UnaryOp::Positive, inner, _) => self.fold(inner, depth),
            Expr::Op1(UnaryOp::Not, _, _) => None,
            Expr::Op2(op, left, right, _) => {
                let (l, r) = (self.fold(left, depth)?, self.fold(right, depth)?);
                match op {
                    BinaryOp::Add => Some(l + r),
                    BinaryOp::Sub => Some(l - r),
                    BinaryOp::Mul => Some(l * r),
                    BinaryOp::Div => Some(l / r),
                    BinaryOp::Exp => Some(l.powf(r)),
                    BinaryOp::Lt
                    | BinaryOp::Gt
                    | BinaryOp::Lte
                    | BinaryOp::Gte
                    | BinaryOp::Eq
                    | BinaryOp::Neq
                    | BinaryOp::And
                    | BinaryOp::Or => None,
                }
            }
            Expr::Var(name, subscripts, _) if subscripts.is_empty() => {
                let info = self.symbols.get(&canonical_name(name))?;
                match &self.select_equation(&info.equations)?.equation {
                    MdlEquation::Regular(lhs, definition) if lhs.subscripts.is_empty() => {
                        self.fold(definition, depth + 1)
                    }
                    _ => None,
                }
            }
            Expr::Var(_, _, _)
            | Expr::App(_, _, _, _, _, _)
            | Expr::Literal(_, _)
            | Expr::Na(_) => None,
        }
    }

    /// The expression a control variable is defined by, if the model defines
    /// it with one.
    fn control_expr(&self, control: Control) -> Option<&Expr<'input>> {
        let info = self.symbols.get(control.key())?;
        match &self.select_equation(&info.equations)?.equation {
            MdlEquation::Regular(_, expr) => Some(expr),
            _ => None,
        }
    }

    /// Reads the control variables into the sim specs, recording the ones
    /// whose equations are not constants (`unread_controls`).
    pub(super) fn read_controls(&mut self) {
        for control in Control::ALL {
            let Some(expr) = self.control_expr(control) else {
                continue;
            };
            let value = self.constant_value(expr);
            let step = || reciprocal(expr).or(value.map(Dt::Dt));
            let read = match control {
                Control::InitialTime => {
                    self.sim_specs.start = value;
                    value.is_some()
                }
                Control::FinalTime => {
                    self.sim_specs.stop = value;
                    value.is_some()
                }
                Control::TimeStep => {
                    self.sim_specs.dt = step();
                    self.sim_specs.dt.is_some()
                }
                // A save step that is the time step follows it, which the
                // datamodel says with no save step: folding it to the time
                // step's number would pin it there.
                Control::Saveper if names_time_step(expr) => true,
                Control::Saveper => {
                    self.sim_specs.save_step = step();
                    self.sim_specs.save_step.is_some()
                }
            };
            if !read {
                self.unread_controls.push(control);
            }
        }
    }

    /// The control equations the project does not keep, as one warning; None
    /// when every one was a constant.
    pub(super) fn control_losses(&self) -> Option<ImportWarning> {
        let names: Vec<&str> = self.unread_controls.iter().map(|c| c.name()).collect();
        if names.is_empty() {
            return None;
        }
        let n = names.len();
        let (noun, verb, each) = if n == 1 {
            (
                "control equation",
                "is",
                "it is not a constant, so the model runs with its default",
            )
        } else {
            (
                "control equations",
                "are",
                "they are not constants, so the model runs with their defaults",
            )
        };
        Some(ImportWarning {
            message: format!(
                "{n} {noun} in the model {verb} not kept: {}; {each}",
                join_quoted_names(&names)
            ),
            count: n,
            one: "control equation".to_owned(),
            many: "control equations".to_owned(),
        })
    }
}

/// `1 / n` as the reciprocal step it spells, the way the datamodel holds a
/// step given as a count per time unit (and the way a save writes one back).
fn reciprocal(expr: &Expr<'_>) -> Option<Dt> {
    match expr {
        Expr::Paren(inner, _) => reciprocal(inner),
        Expr::Op2(BinaryOp::Div, one, n, _) => match (one.as_ref(), n.as_ref()) {
            (Expr::Const(one, _), Expr::Const(n, _)) if *one == 1.0 && *n > 0.0 => {
                Some(Dt::Reciprocal(*n))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Whether `expr` is the time step itself.
fn names_time_step(expr: &Expr<'_>) -> bool {
    match expr {
        Expr::Paren(inner, _) => names_time_step(inner),
        Expr::Var(name, subscripts, _) => {
            subscripts.is_empty() && Control::named(name) == Some(Control::TimeStep)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{convert_mdl, convert_mdl_reporting};
    use super::*;
    use crate::datamodel::SimSpecs;

    /// The specs of a model with these control equations (and one variable).
    fn specs(controls: &str) -> (SimSpecs, Vec<ImportWarning>) {
        let source =
            format!("x = 1 ~~|\nlimit = 40 ~~|\nhalf = limit / 2 ~~|\n{controls}\\\\\\---///\n");
        let (project, warnings) = convert_mdl_reporting(&source, None).expect("converts");
        assert_eq!(
            convert_mdl(&source).expect("converts").sim_specs,
            project.sim_specs
        );
        (project.sim_specs, warnings)
    }

    /// The equation each control variable is given in a row, the others
    /// taking plain numbers.
    fn with(control: Control, equation: &str) -> String {
        Control::ALL
            .into_iter()
            .map(|c| {
                let rhs = if c == control {
                    equation
                } else {
                    match c {
                        Control::InitialTime => "10",
                        Control::FinalTime => "30",
                        Control::TimeStep => "0.5",
                        Control::Saveper => "2",
                    }
                };
                format!("{} = {rhs} ~~|\n", c.name())
            })
            .collect()
    }

    /// The value the specs hold for a control variable.
    fn held(specs: &SimSpecs, control: Control) -> Option<Dt> {
        match control {
            Control::InitialTime => Some(Dt::Dt(specs.start)),
            Control::FinalTime => Some(Dt::Dt(specs.stop)),
            Control::TimeStep => Some(specs.dt.clone()),
            Control::Saveper => specs.save_step.clone(),
        }
    }

    #[test]
    fn a_constant_control_equation_is_its_number() {
        // Every control variable, over every kind of constant: a literal,
        // arithmetic, another control variable, and the model's constants
        // (`half` reads `limit`).
        let rows: &[(&str, f64)] = &[
            ("8", 8.0),
            ("(2 + 6) * 1", 8.0),
            ("2 ^ 3", 8.0),
            ("-(-8)", 8.0),
            ("limit / 5", 8.0),
            ("half - 12", 8.0),
        ];
        for control in Control::ALL {
            for (equation, value) in rows {
                let (specs, warnings) = specs(&with(control, equation));
                assert_eq!(
                    held(&specs, control),
                    Some(Dt::Dt(*value)),
                    "{} = {equation}",
                    control.name()
                );
                assert!(warnings.is_empty(), "{warnings:?}");
            }
        }
        // One control variable over another.
        let (specs, warnings) = specs(&with(Control::FinalTime, "INITIAL TIME + 50"));
        assert_eq!(specs.stop, 60.0);
        assert!(warnings.is_empty());
        let (specs, _) = self::specs(&with(Control::Saveper, "2 * TIME STEP"));
        assert_eq!(specs.save_step, Some(Dt::Dt(1.0)));
    }

    #[test]
    fn a_control_equation_that_is_not_a_constant_takes_its_default_and_is_reported() {
        let defaults = SimSpecs::default();
        let rows = [
            "IF THEN ELSE(Time < 5, 10, 50)",
            "Time",
            "MAX(1, 2)",
            "undefined name",
            "1 / 0",
        ];
        for control in Control::ALL {
            for equation in rows {
                let (specs, warnings) = specs(&with(control, equation));
                let default = match control {
                    Control::InitialTime => Some(Dt::Dt(0.0)),
                    // The reader's own default, xmutil's.
                    Control::FinalTime => Some(Dt::Dt(200.0)),
                    Control::TimeStep => Some(defaults.dt.clone()),
                    Control::Saveper => None,
                };
                assert_eq!(
                    held(&specs, control),
                    default,
                    "{} = {equation}",
                    control.name()
                );
                assert_eq!(warnings.len(), 1, "{} = {equation}", control.name());
                assert_eq!(
                    warnings[0].message,
                    format!(
                        "1 control equation in the model is not kept: '{}'; it is not a \
                         constant, so the model runs with its default",
                        control.name()
                    )
                );
            }
        }
    }

    #[test]
    fn several_unread_control_equations_are_one_warning() {
        let (_, warnings) = specs(
            "INITIAL TIME = 0 ~~|\nFINAL TIME = Time + 1 ~~|\nTIME STEP = Time ~~|\nSAVEPER = TIME STEP ~~|\n",
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            warnings[0].message,
            "2 control equations in the model are not kept: 'FINAL TIME' and 'TIME STEP'; \
             they are not constants, so the model runs with their defaults"
        );
        assert_eq!(warnings[0].count, 2);
    }

    #[test]
    fn constants_defined_by_each_other_are_not_a_constant() {
        let (specs, warnings) = specs(
            "a = b ~~|\nb = a ~~|\nINITIAL TIME = 0 ~~|\nFINAL TIME = a ~~|\nTIME STEP = 1 ~~|\n",
        );
        assert_eq!(specs.stop, 200.0);
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn a_step_given_as_one_over_a_number_is_a_reciprocal() {
        let (specs, warnings) = specs(&with(Control::TimeStep, "1/4"));
        assert_eq!(specs.dt, Dt::Reciprocal(4.0));
        assert!(warnings.is_empty());
        let (specs, _) = self::specs(&with(Control::Saveper, "(1 / 2)"));
        assert_eq!(specs.save_step, Some(Dt::Reciprocal(2.0)));
        // Only that spelling: any other quotient is its number.
        let (specs, _) = self::specs(&with(Control::TimeStep, "2/4"));
        assert_eq!(specs.dt, Dt::Dt(0.5));
        // The times are numbers.
        let (specs, _) = self::specs(&with(Control::FinalTime, "1/4"));
        assert_eq!(specs.stop, 0.25);
    }

    #[test]
    fn a_save_step_that_is_the_time_step_follows_it() {
        for equation in ["TIME STEP", "(TIME STEP)", "time step"] {
            let (specs, warnings) = specs(&with(Control::Saveper, equation));
            assert_eq!(specs.save_step, None, "SAVEPER = {equation}");
            assert!(warnings.is_empty());
        }
    }

    #[test]
    fn a_model_without_a_control_variable_takes_its_default_silently() {
        let (specs, warnings) = specs("");
        assert_eq!((specs.start, specs.stop), (0.0, 200.0));
        assert!(warnings.is_empty());
    }

    #[test]
    fn every_control_variable_is_named_in_any_case_and_spacing() {
        for control in Control::ALL {
            assert_eq!(Control::named(control.name()), Some(control));
            assert_eq!(Control::named(control.key()), Some(control));
            assert_eq!(
                Control::named(&control.name().replace(' ', "_").to_lowercase()),
                Some(control)
            );
        }
        assert_eq!(Control::named("Time"), None);
    }
}
