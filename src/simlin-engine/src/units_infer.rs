// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr};
use crate::builtins::BuiltinFn;
use crate::common::{canonicalize, Ident, Result, UnitResult};
use crate::datamodel::UnitMap;
use crate::model::ModelStage1;
use crate::model_err;
#[cfg(test)]
use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
use crate::units::{combine, Context, UnitOp, Units};
use crate::variable::Variable;

struct UnitInferer<'a> {
    ctx: &'a Context,
    models: &'a HashMap<Ident, &'a ModelStage1>,
    // units for module inputs
    time: Variable,
}

fn single_fv(units: &UnitMap) -> Option<&str> {
    let mut result = None;
    for (unit, _) in units.map.iter() {
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

fn solve_for(var: &str, mut lhs: UnitMap) -> UnitMap {
    // we have:
    //   `1 == $lhs`
    // where $lhs contains $var.  We want:
    //   `$var = $lhs'`
    // so if $var is in the numerator (has a value > 0) we want the
    // inverse of $lhs; otherwise (value < 0) just delete $var from $lhs

    let inverse = if let Some(exponent) = lhs.map.remove(var) {
        // TODO: we seem to be expecting this to be 1 -- what if it is > 1?
        if exponent.abs() != 1 {
            println!("oh no!  solve_for removed {} with exp {}", var, exponent);
        }
        exponent > 0
    } else {
        false
    };

    if inverse {
        lhs.reciprocal()
    } else {
        lhs
    }
}

fn substitute(var: &str, units: &UnitMap, constraints: Vec<UnitMap>) -> Vec<UnitMap> {
    constraints
        .into_iter()
        .map(|mut l| {
            if let Some(exponent) = l.map.remove(var) {
                if exponent.abs() != 1 {
                    println!("oh no!  subst removed {} with exp {}", var, exponent);
                }

                let op = if exponent > 0 {
                    UnitOp::Mul
                } else {
                    UnitOp::Div
                };
                combine(op, l, units.clone())
            } else {
                l
            }
        })
        .collect()
}

impl<'a> UnitInferer<'a> {
    /// gen_constraints generates a set of equality constraints for a given expression,
    /// storing those constraints in the mutable `constraints` argument. This is
    /// right out of Hindley-Milner type inference/Algorithm W, but because we are
    /// dealing with arithmatic expressions instead of types, instead of pairs of types
    /// we can get away with a single UnitMap -- our full constraint is `1 == UnitMap`, we just
    /// leave off the `1 ==` part.
    fn gen_constraints(
        &self,
        expr: &Expr,
        prefix: &str,
        constraints: &mut Vec<UnitMap>,
    ) -> UnitResult<Units> {
        match expr {
            Expr::Const(_, _, _) => Ok(Units::Constant),
            Expr::Var(ident, _loc) => {
                let units: UnitMap = [(format!("@{}{}", prefix, ident), 1)]
                    .iter()
                    .cloned()
                    .collect();

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
                BuiltinFn::Lookup(ident, _, _loc) => {
                    // lookups have the units specified on the table
                    let units: UnitMap = [(format!("@{}{}", prefix, ident), 1)]
                        .iter()
                        .cloned()
                        .collect();

                    Ok(Units::Explicit(units))
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
                | BuiltinFn::Tan(a) => self.gen_constraints(a, prefix, constraints),
                BuiltinFn::Mean(args) => {
                    let args = args
                        .iter()
                        .map(|arg| self.gen_constraints(arg, prefix, constraints))
                        .collect::<UnitResult<Vec<_>>>()?;

                    if args.is_empty() {
                        return Ok(Units::Constant);
                    }

                    // find the first non-constant argument
                    let arg0 = args
                        .iter()
                        .find(|arg| matches!(arg, Units::Explicit(_)))
                        .cloned();
                    match arg0 {
                        Some(Units::Explicit(arg0)) => {
                            for arg in args.iter() {
                                if let Units::Explicit(arg) = arg {
                                    constraints.push(combine(
                                        UnitOp::Div,
                                        arg0.clone(),
                                        arg.clone(),
                                    ));
                                }
                            }
                            Ok(Units::Explicit(arg0))
                        }
                        Some(Units::Constant) => Ok(Units::Constant),
                        None => Ok(Units::Constant),
                    }
                }
                BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                    let a_units = self.gen_constraints(a, prefix, constraints)?;
                    let b_units = self.gen_constraints(b, prefix, constraints)?;

                    if let Units::Explicit(ref lunits) = a_units {
                        if let Units::Explicit(runits) = b_units {
                            constraints.push(combine(UnitOp::Div, lunits.clone(), runits));
                        }
                    }
                    Ok(a_units)
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
                    let units = self.gen_constraints(&div, prefix, constraints)?;

                    // the optional argument to safediv, if specified, should match the units of a/b
                    if let Units::Explicit(ref result_units) = units {
                        if let Some(c) = c {
                            if let Units::Explicit(c_units) =
                                self.gen_constraints(c, prefix, constraints)?
                            {
                                constraints.push(combine(
                                    UnitOp::Div,
                                    c_units,
                                    result_units.clone(),
                                ));
                            }
                        }
                    }

                    Ok(units)
                }
            },
            Expr::Subscript(_, _, _) => Ok(Units::Explicit(UnitMap::new())),
            Expr::Op1(_, l, _) => self.gen_constraints(l, prefix, constraints),
            Expr::Op2(op, l, r, _) => {
                let lunits = self.gen_constraints(l, prefix, constraints)?;
                let runits = self.gen_constraints(r, prefix, constraints)?;

                match op {
                    BinaryOp::Add | BinaryOp::Sub => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Constant, Units::Explicit(units))
                        | (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            constraints.push(combine(UnitOp::Div, lunits.clone(), runits));
                            Ok(Units::Explicit(lunits))
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
                let lunits = self.gen_constraints(l, prefix, constraints)?;
                let runits = self.gen_constraints(r, prefix, constraints)?;

                if let Units::Explicit(ref lunits) = lunits {
                    if let Units::Explicit(runits) = runits {
                        constraints.push(combine(UnitOp::Div, lunits.clone(), runits));
                    }
                }

                Ok(lunits)
            }
        }
    }

    fn gen_all_constraints(
        &self,
        model: &ModelStage1,
        prefix: &str,
        constraints: &mut Vec<UnitMap>,
    ) {
        let time_units = canonicalize(self.ctx.sim_specs.time_units.as_deref().unwrap_or("time"));

        for (id, var) in model.variables.iter() {
            if let Variable::Stock {
                ident,
                inflows,
                outflows,
                ..
            } = var
            {
                let stock_ident = ident;
                let expected = [
                    (format!("@{}{}", prefix, stock_ident), 1),
                    (time_units.clone(), -1),
                ]
                .iter()
                .cloned()
                .collect::<UnitMap>()
                .push_ctx(format!("stock@{}{}", prefix, stock_ident));
                let mut check_flows = |flows: &Vec<Ident>| {
                    for ident in flows.iter() {
                        let flow_units: UnitMap = [(format!("@{}{}", prefix, ident), 1)]
                            .iter()
                            .cloned()
                            .collect();
                        constraints.push(combine(
                            UnitOp::Div,
                            flow_units.push_ctx(format!("stock-flow@{}{}", prefix, ident)),
                            expected.clone(),
                        ));
                    }
                };
                check_flows(inflows);
                check_flows(outflows);
            } else if let Variable::Module {
                ident,
                model_name,
                inputs,
                ..
            } = var
            {
                let submodel = self.models[model_name];
                let subprefix = format!("{}{}·", prefix, ident);
                for input in inputs {
                    let src = format!("@{}{}", prefix, input.src);
                    let dst = format!("@{}{}", subprefix, input.dst);
                    // src = dst === 1 = src/dst
                    let units = [(src.clone(), 1), (dst.clone(), -1)]
                        .iter()
                        .cloned()
                        .collect::<UnitMap>()
                        .push_ctx(format!("module-input{}{}", src, dst));
                    constraints.push(units);
                }
                self.gen_all_constraints(submodel, &subprefix, constraints);
            }
            // we only should be adding constraints based on the equation if
            // the variable _doesn't_ have an associated lookup table/graphical
            // function.
            if var.table().is_none() {
                let var_units = match var.ast() {
                    Some(Ast::Scalar(ast)) => self.gen_constraints(ast, prefix, constraints),
                    Some(Ast::ApplyToAll(_, ast)) => self.gen_constraints(ast, prefix, constraints),
                    Some(Ast::Arrayed(_, _asts)) => {
                        // todo!();
                        Ok(Units::Constant)
                    }
                    None => {
                        // TODO: maybe we should bail early?  If there is no equation we will fail
                        continue;
                    }
                }
                .unwrap();
                match var_units {
                    Units::Constant => {
                        // TODO: constant means ~ unconstrained I think
                    }
                    Units::Explicit(units) => {
                        let mv = [(format!("@{}{}", prefix, id), 1)]
                            .iter()
                            .cloned()
                            .collect::<UnitMap>()
                            .push_ctx(format!("computed-mv@{}{}", prefix, id));
                        constraints.push(combine(UnitOp::Div, mv, units));
                    }
                };
            }
            if let Some(units) = var.units() {
                let mv = [(format!("@{}{}", prefix, id), 1)]
                    .iter()
                    .cloned()
                    .collect::<UnitMap>()
                    .push_ctx(format!("userdef-mv@{}{}", prefix, id));
                constraints.push(combine(UnitOp::Div, mv, units.clone()));
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn unify(
        &self,
        mut constraints: Vec<UnitMap>,
    ) -> Result<(HashMap<Ident, UnitMap>, Option<Vec<UnitMap>>)> {
        let mut resolved_fvs: HashMap<Ident, UnitMap> = HashMap::new();
        let mut final_constraints: Vec<UnitMap> = Vec::with_capacity(constraints.len());

        // FIXME: I think this is O(n^3) worst case; we could do better
        //        by maintaining an index of metavar usage -> Units
        loop {
            let initial_constraint_count = constraints.len();
            while let Some(c) = constraints.pop() {
                // dimensionless/identity unit: `1 == 1`; nothing to do
                if c.is_empty() {
                    continue;
                }
                if let Some(var) = single_fv(&c) {
                    let var = var.to_owned();
                    let units = solve_for(&var, c);
                    constraints = substitute(&var, &units, constraints);
                    final_constraints = substitute(&var, &units, final_constraints);
                    if let Some(existing_units) = resolved_fvs.get(&var) {
                        if *existing_units != units {
                            return model_err!(
                                UnitMismatch,
                                format!(
                                    "units for {} don't match ({} != {}); this should be Result",
                                    var, existing_units, units,
                                )
                            );
                        }
                    } else {
                        let var = var.strip_prefix('@').unwrap().to_owned();
                        resolved_fvs.insert(var, units);
                    }
                } else {
                    final_constraints.push(c);
                }
            }
            // iterate to a fixed point
            if final_constraints.len() == initial_constraint_count {
                break;
            } else {
                constraints = std::mem::take(&mut final_constraints);
            }
        }

        let final_constraints = if final_constraints.is_empty() {
            None
        } else {
            Some(final_constraints)
        };

        Ok((resolved_fvs, final_constraints))
    }

    fn infer(&self, model: &ModelStage1) -> Result<HashMap<String, UnitMap>> {
        // use rand::seq::SliceRandom;
        // use rand::thread_rng;

        let mut constraints = vec![];
        self.gen_all_constraints(model, "", &mut constraints);
        // mostly for robustness: ensure we don't inadvertently depend on
        // test cases iterating in a specific order.
        // constraints.shuffle(&mut thread_rng());

        let (results, constraints) = self.unify(constraints)?;

        if let Some(constraints) = constraints {
            use std::fmt::Write;
            let prefix = "unit checking failed; couldn't resolve: \n";
            let mut s = prefix.to_owned();
            for c in constraints.iter() {
                let delim = if s.len() == prefix.len() { "" } else { "; " };
                write!(s, "{}\n    1 == {}", delim, c).unwrap();
            }
            model_err!(UnitMismatch, s)
        } else {
            Ok(results)
        }
    }
}

#[test]
fn test_inference() {
    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).unwrap();

    // test cases where we should be able to infer all units
    let test_cases: &[&[(crate::datamodel::Variable, &'static str)]] = &[
        &[
            (x_aux("input", "6", Some("widget")), "widget"),
            (x_flow("delay", "3", Some("parsec")), "parsec"),
            // testing the 2-input version of smth1
            (x_aux("seen", "SMTH1(input, delay)", None), "widget"),
            (x_aux("seen_dep", "seen + 1", None), "widget"),
        ],
        &[
            (
                x_stock("stock_1", "1", &["inflow"], &[], Some("usd")),
                "usd",
            ),
            (x_aux("window", "6", Some("parsec")), "parsec"),
            (x_flow("inflow", "seen/window", None), "usd/parsec"),
            (x_aux("seen", "sin(seen_dep) mod 3", None), "usd"),
            (x_aux("seen_dep", "1 + 3 * stock_1", None), "usd"),
        ],
        &[
            (x_aux("initial", "70", Some("widget")), "widget"),
            (x_aux("input", "6", Some("widget")), "widget"),
            (x_flow("delay", "3", Some("parsec")), "parsec"),
            // testing the 3-input version of smth1
            (
                x_aux("seen", "DELAY1(input, delay, initial)", None),
                "widget",
            ),
            (x_aux("seen_dep", "seen + 1", None), "widget"),
        ],
    ];

    for test_case in test_cases.iter() {
        let expected = test_case
            .iter()
            .map(|(var, units)| (var.get_ident(), *units))
            .collect::<HashMap<&str, &str>>();
        let vars = test_case
            .iter()
            .map(|(var, _unit)| var)
            .cloned()
            .collect::<Vec<_>>();
        let model = x_model("main", vars);
        let project_datamodel = x_project(sim_specs.clone(), &[model]);

        // there is non-determinism in inference; do it a few times to
        // shake out heisenbugs
        for _ in 0..64 {
            let mut results: Result<HashMap<Ident, UnitMap>> =
                model_err!(UnitMismatch, "".to_owned());
            let _project = crate::project::Project::base_from(
                project_datamodel.clone(),
                |models, units_ctx, model| {
                    results = infer(models, units_ctx, model);
                },
            );
            let results = results.unwrap();
            for (ident, expected_units) in expected.iter() {
                let expected_units: UnitMap =
                    crate::units::parse_units(&units_ctx, Some(expected_units))
                        .unwrap()
                        .unwrap();
                if let Some(computed_units) = results.get(*ident) {
                    assert_eq!(expected_units, *computed_units);
                } else {
                    panic!("inference results don't contain variable '{}'", ident);
                }
            }
        }
    }
}

#[test]
fn test_inference_negative() {
    let sim_specs = sim_specs_with_units("parsec");

    // test cases where we should expect to fail
    let test_cases: &[&[(crate::datamodel::Variable, &'static str)]] = &[
        &[
            // the "+ TIME" here causes constraints to fail
            (x_aux("input", "6 + TIME", Some("widget")), "widget"),
            (x_flow("delay", "3", Some("parsec")), "parsec"),
            // testing the 2-input version of smth1
            (x_aux("seen", "SMTH1(input, delay)", None), "widget"),
            (x_aux("seen_dep", "seen + 1", None), "widget"),
        ],
        &[
            (
                x_stock("stock_1", "1", &["inflow"], &[], Some("usd")),
                "usd",
            ),
            // need this to be defined
            (x_aux("window", "6", None), "parsec"),
            (x_flow("inflow", "seen/window", None), "usd/parsec"),
            (x_aux("seen", "sin(seen_dep) mod 3", None), "usd"),
            (x_aux("seen_dep", "1 + 3", None), "usd"),
        ],
        &[
            // initial needs to have the same units as input
            (x_aux("initial", "70", Some("wallop")), "wallop"),
            (x_aux("input", "6", Some("widget")), "widget"),
            (x_flow("delay", "3", Some("parsec")), "parsec"),
            // testing the 3-input version of smth1
            (
                x_aux("seen", "SMTH1(input, delay, initial)", None),
                "widget",
            ),
            (x_aux("seen_dep", "seen + 1", None), "widget"),
        ],
    ];

    for test_case in test_cases.iter() {
        let vars = test_case
            .iter()
            .map(|(var, _unit)| var)
            .cloned()
            .collect::<Vec<_>>();
        let model = x_model("main", vars);
        let project_datamodel = x_project(sim_specs.clone(), &[model]);

        // there is non-determinism in inference; do it a few times to
        // shake out heisenbugs
        for _ in 0..64 {
            let mut results: Result<HashMap<Ident, UnitMap>> =
                model_err!(UnitMismatch, "".to_owned());
            let _project = crate::project::Project::base_from(
                project_datamodel.clone(),
                |models, units_ctx, model| {
                    results = infer(models, units_ctx, model);
                },
            );
            assert!(results.is_err());
        }
    }
}

pub(crate) fn infer(
    models: &HashMap<Ident, &ModelStage1>,
    units_ctx: &Context,
    model: &ModelStage1,
) -> Result<HashMap<String, UnitMap>> {
    let time_units = canonicalize(units_ctx.sim_specs.time_units.as_deref().unwrap_or("time"));

    let units = UnitInferer {
        ctx: units_ctx,
        models,
        time: Variable::Var {
            ident: "time".to_string(),
            ast: None,
            init_ast: None,
            eqn: None,
            units: Some([(time_units, 1)].iter().cloned().collect()),
            table: None,
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        },
    };

    units.infer(model)
}
