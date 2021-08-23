// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[cfg(test)]
use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr};
use crate::builtins::BuiltinFn;
use crate::common::{canonicalize, ErrorCode, Ident, Result, UnitError, UnitResult};
use crate::datamodel::UnitMap;
#[cfg(test)]
use crate::model::ModelStage0;
use crate::model::ModelStage1;
#[cfg(test)]
use crate::testutils::{x_aux, x_flow, x_model, x_stock};
use crate::units::pretty_print_unit;
use crate::units::{combine, Context, UnitOp, Units};
use crate::variable::Variable;

struct UnitInferer<'a> {
    ctx: &'a Context,
    model: &'a ModelStage1,
    // units for module inputs
    time: Variable,
}

fn single_fv(units: &UnitMap) -> Option<&str> {
    let mut result = None;
    for (unit, _) in units.iter() {
        if unit.starts_with('@') {
            if result.is_none() {
                result = Some(unit.as_str())
            } else {
                return None;
            }
        }
    }
    result
}

fn solve_for(var: &str, lhs: &UnitMap, rhs: &UnitMap) -> UnitMap {
    let orig_lhs = lhs;
    let orig_rhs = rhs;
    // after this, "lhs" is always the UnitMap that contains var
    let (lhs, rhs) = if lhs.contains_key(var) {
        (lhs, rhs)
    } else if rhs.contains_key(var) {
        (rhs, lhs)
    } else {
        unreachable!();
    };
    // lhs / var = rhs === lhs / rhs = var
    let (lhs, rhs) = if lhs[var] < 0 { (rhs, lhs) } else { (lhs, rhs) };
    // lhs * var = rhs === rhs / lhs
    // if 'var' is on the left, solve for it by dividing rhs by (lhs \ var)
    let num: UnitMap = rhs
        .iter()
        .filter(|(s, _)| var != *s)
        .map(|(k, v)| (k.clone(), *v))
        .collect();
    let div: UnitMap = lhs
        .iter()
        .filter(|(s, _)| var != *s)
        .map(|(k, v)| (k.clone(), *v))
        .collect();

    let result = combine(UnitOp::Div, num, div);

    eprintln!(
        "SOLVED: ({} == {}) -> ({} == {})",
        pretty_print_unit(orig_lhs),
        pretty_print_unit(orig_rhs),
        var,
        pretty_print_unit(&result)
    );

    result
}

fn substitute(
    var: &str,
    units: &UnitMap,
    constraints: Vec<(UnitMap, UnitMap)>,
) -> Vec<(UnitMap, UnitMap)> {
    constraints
        .into_iter()
        .map(|(mut l, mut r)| {
            if l.contains_key(var) {
                let op = if l[var] > 0 { UnitOp::Mul } else { UnitOp::Div };
                let _ = l.remove(var);
                l = combine(op, l, units.clone());
            }
            if r.contains_key(var) {
                let op = if r[var] > 0 { UnitOp::Mul } else { UnitOp::Div };
                let _ = r.remove(var);
                r = combine(op, r, units.clone());
            }
            (l, r)
        })
        .collect()
}

impl<'a> UnitInferer<'a> {
    #[allow(dead_code)]
    fn gen_constraints(
        &self,
        expr: &Expr,
        constraints: &mut Vec<(UnitMap, UnitMap)>,
    ) -> UnitResult<Units> {
        use UnitError::ConsistencyError;
        match expr {
            Expr::Const(_, _, _) => Ok(Units::Constant),
            Expr::Var(ident, _loc) => {
                let units: UnitMap = [(format!("@{}", ident), 1)].iter().cloned().collect();

                Ok(Units::Explicit(units))
            }
            Expr::App(builtin, _) => match builtin {
                BuiltinFn::Inf | BuiltinFn::Pi => Ok(Units::Constant),
                BuiltinFn::Time
                | BuiltinFn::TimeStep
                | BuiltinFn::StartTime
                | BuiltinFn::FinalTime => Ok(Units::Explicit(
                    self.time.units().cloned().unwrap_or_default(),
                )),
                BuiltinFn::IsModuleInput(_, _) => {
                    // returns a bool, which is unitless
                    Ok(Units::Explicit(UnitMap::new()))
                }
                BuiltinFn::Lookup(ident, _, loc) => {
                    // lookups have the units specified on the table
                    if let Some(var) = self.model.variables.get(ident) {
                        var.units()
                            .ok_or_else(|| {
                                ConsistencyError(
                                    ErrorCode::UnitMismatch,
                                    *loc,
                                    Some(format!(
                                        "no units specified for lookup dependency '{}'",
                                        ident
                                    )),
                                )
                            })
                            .map(|units| Units::Explicit(units.clone()))
                    } else {
                        Err(ConsistencyError(
                            ErrorCode::DoesNotExist,
                            *loc,
                            Some(format!(
                                "can't find dependency '{}' for unit checking",
                                ident,
                            )),
                        ))
                    }
                }
                BuiltinFn::Abs(a)
                | BuiltinFn::Arccos(a)
                | BuiltinFn::Arcsin(a)
                | BuiltinFn::Arctan(a)
                | BuiltinFn::Cos(a)
                | BuiltinFn::Exp(a)
                | BuiltinFn::Int(a)
                | BuiltinFn::Ln(a)
                | BuiltinFn::Log10(a)
                | BuiltinFn::Sin(a)
                | BuiltinFn::Sqrt(a)
                | BuiltinFn::Tan(a) => self.gen_constraints(a, constraints),
                BuiltinFn::Mean(args) => {
                    let args = args
                        .iter()
                        .map(|arg| self.gen_constraints(arg, constraints))
                        .collect::<UnitResult<Vec<_>>>()?;

                    if args.is_empty() {
                        return Ok(Units::Constant);
                    }

                    // find the first non-constant argument
                    let arg0 = args
                        .iter()
                        .filter(|arg| matches!(arg, Units::Explicit(_)))
                        .cloned()
                        .next();
                    match arg0 {
                        Some(arg0) => {
                            if args.iter().all(|arg| arg0.equals(arg)) {
                                Ok(arg0)
                            } else {
                                let expected = match arg0 {
                                    Units::Explicit(units) => units,
                                    Units::Constant => Default::default(),
                                };
                                Err(ConsistencyError(
                                    ErrorCode::UnitDefinitionErrors,
                                    expr.get_loc(),
                                    Some(format!(
                                        "expected all arguments to mean() to have the same units '{}'",
                                        pretty_print_unit(&expected),
                                    )),
                                ))
                            }
                        }
                        // all args were constants, so we're good
                        None => Ok(Units::Constant),
                    }
                }
                BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                    let a_units = self.gen_constraints(a, constraints)?;
                    let b_units = self.gen_constraints(b, constraints)?;
                    if !a_units.equals(&b_units) {
                        let a_units = match a_units {
                            Units::Explicit(units) => units,
                            Units::Constant => Default::default(),
                        };
                        let b_units = match b_units {
                            Units::Explicit(units) => units,
                            Units::Constant => Default::default(),
                        };
                        let loc = a.get_loc().union(&b.get_loc());
                        Err(ConsistencyError (
                            ErrorCode::UnitDefinitionErrors,
                            loc,
                            Some(format!(
                                "expected left and right argument units to match, but '{}' and '{}' don't",
                                pretty_print_unit(&a_units),
                                pretty_print_unit(&b_units),
                            )),
                        ))
                    } else {
                        Ok(a_units)
                    }
                }
                BuiltinFn::Pulse(_, _, _) | BuiltinFn::Ramp(_, _, _) | BuiltinFn::Step(_, _) => {
                    Ok(Units::Constant)
                }
                BuiltinFn::SafeDiv(a, b, c) => {
                    let div = Expr::Op2(
                        BinaryOp::Div,
                        a.clone(),
                        b.clone(),
                        a.get_loc().union(&b.get_loc()),
                    );
                    let units = self.gen_constraints(&div, constraints)?;

                    if let Some(c) = c {
                        let c_units = self.gen_constraints(c, constraints)?;
                        if c_units != units {
                            // TODO: return an error here
                        }
                    }

                    Ok(units)
                }
            },
            Expr::Subscript(_, _, _) => Ok(Units::Explicit(UnitMap::new())),
            Expr::Op1(_, l, _) => self.gen_constraints(l, constraints),
            Expr::Op2(op, l, r, _) => {
                let lunits = self.gen_constraints(l, constraints)?;
                let runits = self.gen_constraints(r, constraints)?;

                match op {
                    BinaryOp::Add | BinaryOp::Sub => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Constant, Units::Explicit(units))
                        | (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            if lunits != runits {
                                let details = Some(format!(
                                    "expected left and right argument units to match, but '{}' and '{}' don't",
                                    pretty_print_unit(&lunits),
                                    pretty_print_unit(&runits),
                                ));
                                let loc = l.get_loc().union(&r.get_loc());
                                Err(ConsistencyError(ErrorCode::UnitMismatch, loc, details))
                            } else {
                                Ok(Units::Explicit(lunits))
                            }
                        }
                    },
                    BinaryOp::Exp | BinaryOp::Mod => Ok(lunits),
                    BinaryOp::Mul => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Explicit(units), Units::Constant)
                        | (Units::Constant, Units::Explicit(units)) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            Ok(Units::Explicit(combine(UnitOp::Mul, lunits, runits)))
                        }
                    },
                    BinaryOp::Div => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Constant, Units::Explicit(units)) => {
                            Ok(Units::Explicit(combine(UnitOp::Div, UnitMap::new(), units)))
                        }
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            Ok(Units::Explicit(combine(UnitOp::Div, lunits, runits)))
                        }
                    },
                    BinaryOp::Gt
                    | BinaryOp::Lt
                    | BinaryOp::Gte
                    | BinaryOp::Lte
                    | BinaryOp::Eq
                    | BinaryOp::Neq
                    | BinaryOp::And
                    | BinaryOp::Or => {
                        // binary comparisons result in unitless quantities
                        Ok(Units::Explicit(UnitMap::new()))
                    }
                }
            }
            Expr::If(_, l, r, _) => {
                let lunits = self.gen_constraints(l, constraints)?;
                let runits = self.gen_constraints(r, constraints)?;

                if !lunits.equals(&runits) {
                    eprintln!("TODO: if error, left and right units don't match");
                }

                Ok(lunits)
            }
        }
    }

    #[allow(dead_code)]
    fn gen_all_constraints(&self, model: &ModelStage1, constraints: &mut Vec<(UnitMap, UnitMap)>) {
        let time_units = canonicalize(self.ctx.sim_specs.time_units.as_deref().unwrap_or("time"));

        for (id, var) in model.variables.iter() {
            eprintln!("generating constraints for {}", id);
            if let Variable::Stock {
                ident,
                inflows,
                outflows,
                ..
            } = var
            {
                let stock_ident = ident;
                let expected: UnitMap =
                    [(format!("@{}", stock_ident), 1), (time_units.clone(), -1)]
                        .iter()
                        .cloned()
                        .collect();
                let mut check_flows = |flows: &Vec<Ident>| {
                    for ident in flows.iter() {
                        let flow_units: UnitMap =
                            [(format!("@{}", ident), 1)].iter().cloned().collect();
                        constraints.push((flow_units, expected.clone()));
                    }
                };
                check_flows(inflows);
                check_flows(outflows);
            }
            let var_units = match var.ast() {
                Some(Ast::Scalar(ast)) => self.gen_constraints(ast, constraints),
                Some(Ast::ApplyToAll(_, ast)) => self.gen_constraints(ast, constraints),
                Some(Ast::Arrayed(_, _asts)) => {
                    todo!();
                }
                None => {
                    eprintln!("no equation for {}", id);
                    continue;
                }
            }
            .unwrap();
            match var_units {
                Units::Constant => {
                    // TODO: constant means ~ unconstrained I think
                }
                Units::Explicit(units) => {
                    let mv = [(format!("@{}", id), 1)].iter().cloned().collect();
                    constraints.push((mv, units));
                }
            };
            if let Some(units) = var.units() {
                let mv = [(format!("@{}", id), 1)].iter().cloned().collect();
                constraints.push((mv, units.clone()));
            }
        }
    }

    fn unify(&self, mut constraints: Vec<(UnitMap, UnitMap)>) -> Vec<(UnitMap, UnitMap)> {
        // a good guess at capacity
        let mut final_constraints: Vec<(UnitMap, UnitMap)> = Vec::with_capacity(constraints.len());
        while let Some((l, r)) = constraints.pop() {
            if l == r {
                continue;
            }
            let lfv = single_fv(&l);
            let rfv = single_fv(&r);
            if lfv.is_some() && !r.contains_key(lfv.unwrap_or_default()) {
                if let Some(var) = lfv {
                    let units = solve_for(var, &l, &r);
                    constraints = substitute(var, &units, constraints);
                    final_constraints = substitute(var, &units, final_constraints);
                    final_constraints
                        .push(([(var.to_owned(), 1)].iter().cloned().collect(), units));
                }
            } else if rfv.is_some() && !l.contains_key(rfv.unwrap_or_default()) {
                if let Some(var) = rfv {
                    let units = solve_for(var, &l, &r);
                    constraints = substitute(var, &units, constraints);
                    final_constraints = substitute(var, &units, final_constraints);
                    final_constraints
                        .push(([(var.to_owned(), 1)].iter().cloned().collect(), units));
                }
            } else {
                // TODO: is this safe, or do we need some check
                final_constraints.push((l, r));
            }
        }

        final_constraints
    }

    #[allow(dead_code)]
    fn infer(&self, model: &ModelStage1) -> Result<()> {
        use rand::seq::SliceRandom;
        use rand::thread_rng;

        let mut constraints = vec![];
        self.gen_all_constraints(model, &mut constraints);
        constraints.shuffle(&mut thread_rng());

        eprintln!("constraints:");

        for (l, r) in constraints.iter() {
            eprintln!("  {} == {}", pretty_print_unit(l), pretty_print_unit(r));
        }

        let constraints = self.unify(constraints);

        eprintln!("after unification:");

        for (l, r) in constraints.iter() {
            eprintln!("  {} == {}", pretty_print_unit(l), pretty_print_unit(r));
        }

        // todo!();

        Ok(())
    }
}

#[test]
fn test_inference() {
    let sim_specs = crate::datamodel::SimSpecs {
        start: 0.0,
        stop: 0.0,
        dt: Default::default(),
        save_step: None,
        sim_method: Default::default(),
        // if star wars says its a time unit, that's good enough for me
        time_units: Some("parsec".to_owned()),
    };

    // we should be able to fully infer the units here:
    // - window needs to be "parsec"
    // - seen needs to be "usd"
    // - and inflow needs to be "usd/parsec"
    let model = x_model(
        "main",
        vec![
            x_stock("stock_1", "1", &["inflow"], &[], Some("usd")),
            x_aux("window", "6", None),
            x_flow("inflow", "seen/window", None),
            x_aux("seen", "3 * stock_1", None),
        ],
    );

    let units_ctx = Context::new_with_builtins(&[], &sim_specs).unwrap();

    let model = ModelStage0::new(&model, &[], &units_ctx, false);
    let model = ModelStage1::new(&units_ctx, &HashMap::new(), &model);

    let time_units = canonicalize(units_ctx.sim_specs.time_units.as_deref().unwrap_or("time"));

    let units = UnitInferer {
        ctx: &units_ctx,
        model: &model,
        time: Variable::Var {
            ident: "time".to_string(),
            ast: None,
            eqn: None,
            units: Some([(time_units.clone(), 1)].iter().cloned().collect()),
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        },
    };

    units.infer(&model).unwrap();
}
