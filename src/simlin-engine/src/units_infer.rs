// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr2};
use crate::builtins::BuiltinFn;
use crate::common::{Canonical, Ident, Result, UnitResult, canonicalize};
use crate::datamodel::UnitMap;
use crate::model::ModelStage1;
use crate::model_err;
#[cfg(test)]
use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
use crate::units::{Context, UnitOp, Units, combine};
use crate::variable::Variable;

struct UnitInferer<'a> {
    ctx: &'a Context,
    models: &'a HashMap<Ident<Canonical>, &'a ModelStage1>,
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
            println!("oh no!  solve_for removed {var} with exp {exponent}");
        }
        exponent > 0
    } else {
        false
    };

    if inverse { lhs.reciprocal() } else { lhs }
}

fn substitute(var: &str, units: &UnitMap, constraints: Vec<UnitMap>) -> Vec<UnitMap> {
    constraints
        .into_iter()
        .map(|mut l| {
            if let Some(exponent) = l.map.remove(var) {
                if exponent.abs() != 1 {
                    println!("oh no!  subst removed {var} with exp {exponent}");
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

/// Splits a UnitMap into its metavariable part (signature) and concrete part (residual).
/// This enables O(n) mismatch detection by grouping constraints with the same signature.
fn split_constraint(u: &UnitMap) -> (UnitMap, UnitMap) {
    let mut signature = UnitMap::new();
    let mut residual = UnitMap::new();

    for (name, exp) in u.map.iter() {
        if name.starts_with('@') {
            signature.map.insert(name.clone(), *exp);
        } else {
            residual.map.insert(name.clone(), *exp);
        }
    }

    (signature, residual)
}

/// Finds mismatches in the remaining constraints after unification.
///
/// There are two types of mismatches:
///
/// 1. A constraint with only concrete units (no metavariables) that isn't dimensionless.
///    This means we have an equation like `meters = seconds` which is impossible.
///
/// 2. Two constraints with the same metavariable "signature" but different concrete "residuals".
///    For example, `@a/@b * meters = 1` and `@a/@b * seconds = 1` both have signature `@a/@b`
///    but residuals `meters` vs `seconds`. This implies `meters = seconds`, a contradiction.
///
/// This is O(n) by grouping constraints by their metavariable signature using a HashMap,
/// rather than O(n²) pairwise comparison.
fn find_constraint_mismatch(constraints: &[UnitMap]) -> Option<String> {
    use std::collections::HashMap;
    use std::fmt::Write;

    // Group constraints by their metavariable signature.
    // Key: sorted string representation of metavar signature (for HashMap key)
    // Value: (first constraint with this signature, its residual)
    let mut signature_groups: HashMap<String, (&UnitMap, UnitMap)> = HashMap::new();

    for constraint in constraints {
        let (signature, residual) = split_constraint(constraint);

        // Case 1: No metavariables means this is a direct concrete mismatch
        if signature.map.is_empty() && !residual.map.is_empty() {
            let mut s = "unit checking failed; conflicting constraint:\n".to_owned();
            write!(s, "    1 == {constraint}").unwrap();
            return Some(s);
        }

        // Create a canonical string key for the signature (sorted for consistency)
        let sig_key = format!("{signature}");

        if let Some((first_constraint, first_residual)) = signature_groups.get(&sig_key) {
            // Case 2: Same signature but different residual means contradiction
            if residual != *first_residual {
                let mut s = "unit checking failed; inconsistent constraints:\n".to_owned();
                writeln!(s, "    1 == {}", first_constraint).unwrap();
                writeln!(s, "    1 == {}", constraint).unwrap();
                // The ratio of residuals shows the implied contradiction
                let implied = first_residual.clone() / residual;
                write!(s, "  These imply: 1 == {implied}").unwrap();
                return Some(s);
            }
        } else {
            signature_groups.insert(sig_key, (constraint, residual));
        }
    }

    None
}

impl UnitInferer<'_> {
    /// gen_constraints generates a set of equality constraints for a given expression,
    /// storing those constraints in the mutable `constraints` argument. This is
    /// right out of Hindley-Milner type inference/Algorithm W, but because we are
    /// dealing with arithmatic expressions instead of types, instead of pairs of types
    /// we can get away with a single UnitMap -- our full constraint is `1 == UnitMap`, we just
    /// leave off the `1 ==` part.
    fn gen_constraints(
        &self,
        expr: &Expr2,
        prefix: &str,
        constraints: &mut Vec<UnitMap>,
    ) -> UnitResult<Units> {
        match expr {
            Expr2::Const(_, _, _) => Ok(Units::Constant),
            Expr2::Var(ident, _, _loc) => {
                let units: UnitMap = [(format!("@{prefix}{ident}"), 1)].iter().cloned().collect();

                Ok(Units::Explicit(units))
            }
            Expr2::App(builtin, _, _) => match builtin {
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
                BuiltinFn::Lookup(table_expr, _, _loc)
                | BuiltinFn::LookupForward(table_expr, _, _loc)
                | BuiltinFn::LookupBackward(table_expr, _, _loc) => {
                    // lookups have the units specified on the table
                    let table_name = match table_expr.as_ref() {
                        Expr2::Var(name, _, _) => name.as_str(),
                        Expr2::Subscript(name, _, _, _) => name.as_str(),
                        _ => return Ok(Units::Constant),
                    };
                    let units: UnitMap = [(format!("@{prefix}{table_name}"), 1)]
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
                | BuiltinFn::Sign(a)
                | BuiltinFn::Sin(a)
                | BuiltinFn::Sqrt(a)
                | BuiltinFn::Tan(a)
                | BuiltinFn::Size(a)
                | BuiltinFn::Stddev(a)
                | BuiltinFn::Sum(a) => self.gen_constraints(a, prefix, constraints),
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
                    if let Some(b) = b {
                        let b_units = self.gen_constraints(b, prefix, constraints)?;

                        if let Units::Explicit(ref lunits) = a_units
                            && let Units::Explicit(runits) = b_units
                        {
                            constraints.push(combine(UnitOp::Div, lunits.clone(), runits));
                        }
                    }
                    Ok(a_units)
                }
                BuiltinFn::Pulse(_, _, _) | BuiltinFn::Ramp(_, _, _) | BuiltinFn::Step(_, _) => {
                    Ok(Units::Constant)
                }
                BuiltinFn::SafeDiv(a, b, c) => {
                    let div = Expr2::Op2(
                        BinaryOp::Div,
                        a.clone(),
                        b.clone(),
                        None,
                        a.get_loc().union(&b.get_loc()),
                    );
                    let units = self.gen_constraints(&div, prefix, constraints)?;

                    // the optional argument to safediv, if specified, should match the units of a/b
                    if let Units::Explicit(ref result_units) = units
                        && let Some(c) = c
                        && let Units::Explicit(c_units) =
                            self.gen_constraints(c, prefix, constraints)?
                    {
                        constraints.push(combine(UnitOp::Div, c_units, result_units.clone()));
                    }

                    Ok(units)
                }
                BuiltinFn::Rank(a, _rest) => {
                    let a_units = self.gen_constraints(a, prefix, constraints)?;

                    // from the spec, I don't think there are any constraints on the optional args:
                    // RANK(A, SIZE) gives index of MAX value in array A (i.e., final ranked, ascending order)
                    // RANK(A, 3, B) gives index of third smallest value in array A, breaking any ties between same-valued elements in A by comparing the corresponding elements in array B

                    Ok(a_units)
                }
            },
            Expr2::Subscript(base_name, _, _, _) => {
                // A subscripted expression has the same units as the base array
                let units: UnitMap = [(format!("@{prefix}{base_name}"), 1)]
                    .iter()
                    .cloned()
                    .collect();
                Ok(Units::Explicit(units))
            }
            Expr2::Op1(_, l, _, _) => self.gen_constraints(l, prefix, constraints),
            Expr2::Op2(op, l, r, _, _) => {
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
            Expr2::If(_, l, r, _, _) => {
                let lunits = self.gen_constraints(l, prefix, constraints)?;
                let runits = self.gen_constraints(r, prefix, constraints)?;

                if let Units::Explicit(ref lunits) = lunits
                    && let Units::Explicit(runits) = runits
                {
                    constraints.push(combine(UnitOp::Div, lunits.clone(), runits));
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
        let time_units = canonicalize(self.ctx.sim_specs.time_units.as_deref().unwrap_or("time"))
            .as_str()
            .to_string();

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
                    (format!("@{prefix}{stock_ident}"), 1),
                    (time_units.clone(), -1),
                ]
                .iter()
                .cloned()
                .collect::<UnitMap>()
                .push_ctx(format!("stock@{prefix}{stock_ident}"));
                let mut check_flows = |flows: &Vec<Ident<Canonical>>| {
                    for ident in flows.iter() {
                        let flow_units: UnitMap =
                            [(format!("@{prefix}{ident}"), 1)].iter().cloned().collect();
                        constraints.push(combine(
                            UnitOp::Div,
                            flow_units.push_ctx(format!("stock-flow@{prefix}{ident}")),
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
                let subprefix = format!("{prefix}{ident}·");
                for input in inputs {
                    let src = format!("@{}{}", prefix, input.src);
                    let dst = format!("@{}{}", subprefix, input.dst);
                    // src = dst === 1 = src/dst
                    let units = [(src.clone(), 1), (dst.clone(), -1)]
                        .iter()
                        .cloned()
                        .collect::<UnitMap>()
                        .push_ctx(format!("module-input{src}{dst}"));
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
                // Constants don't generate constraints - they adopt units from context
                // (e.g., in "x + 1", the 1 has the same units as x)
                if let Units::Explicit(units) = var_units {
                    let mv = [(format!("@{prefix}{id}"), 1)]
                        .iter()
                        .cloned()
                        .collect::<UnitMap>()
                        .push_ctx(format!("computed-mv@{prefix}{id}"));
                    constraints.push(combine(UnitOp::Div, mv, units));
                }
            }
            if let Some(units) = var.units() {
                let mv = [(format!("@{prefix}{id}"), 1)]
                    .iter()
                    .cloned()
                    .collect::<UnitMap>()
                    .push_ctx(format!("userdef-mv@{prefix}{id}"));
                constraints.push(combine(UnitOp::Div, mv, units.clone()));
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn unify(
        &self,
        mut constraints: Vec<UnitMap>,
    ) -> Result<(HashMap<Ident<Canonical>, UnitMap>, Option<Vec<UnitMap>>)> {
        let mut resolved_fvs: HashMap<Ident<Canonical>, UnitMap> = HashMap::new();
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
                    let var_key = var.strip_prefix('@').unwrap();
                    if let Some(existing_units) =
                        resolved_fvs.get(&Ident::<Canonical>::from_str_unchecked(var_key))
                    {
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
                        resolved_fvs.insert(Ident::<Canonical>::from_str_unchecked(var_key), units);
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

    fn infer(&self, model: &ModelStage1) -> Result<HashMap<Ident<Canonical>, UnitMap>> {
        // use rand::seq::SliceRandom;
        // use rand::thread_rng;

        let mut constraints = vec![];
        self.gen_all_constraints(model, "", &mut constraints);
        // mostly for robustness: ensure we don't inadvertently depend on
        // test cases iterating in a specific order.
        // constraints.shuffle(&mut thread_rng());

        let (results, constraints) = self.unify(constraints)?;

        if let Some(constraints) = constraints {
            // Check if any unresolved constraint represents an actual mismatch
            let mismatch = find_constraint_mismatch(&constraints);

            if let Some(mismatch_info) = mismatch {
                model_err!(UnitMismatch, mismatch_info)
            } else {
                // Unresolved constraints with metavariables just mean the model
                // is under-constrained (e.g., no units declared). Return partial results.
                Ok(results)
            }
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
        // Test that a constant without declared units is properly constrained through
        // module/builtin usage. Here delay_const has no declared units but should be
        // inferred as "parsec" (time units) because it's used as the delay parameter in SMTH1.
        &[
            (x_aux("input", "6", Some("widget")), "widget"),
            // delay_const is a constant (no units declared), but should be inferred as time units
            (x_aux("delay_const", "3", None), "parsec"),
            (x_aux("seen", "SMTH1(input, delay_const)", None), "widget"),
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
            let mut results: Result<HashMap<Ident<Canonical>, UnitMap>> =
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
                if let Some(computed_units) = results.get(&canonicalize(ident)) {
                    assert_eq!(expected_units, *computed_units);
                } else {
                    panic!("inference results don't contain variable '{ident}'");
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
            // window has wrong units (usd instead of parsec/time)
            // This creates a mismatch: inflow = seen/window should be usd/parsec
            // but with window in usd, it would be usd/usd = dimensionless
            (x_aux("window", "6", Some("usd")), "usd"),
            (
                x_flow("inflow", "seen/window", Some("usd/parsec")),
                "usd/parsec",
            ),
            (x_aux("seen", "sin(seen_dep) mod 3", Some("usd")), "usd"),
            (x_aux("seen_dep", "1 + 3 * stock_1", None), "usd"),
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
            let mut results: Result<HashMap<Ident<Canonical>, UnitMap>> =
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
    models: &HashMap<Ident<Canonical>, &ModelStage1>,
    units_ctx: &Context,
    model: &ModelStage1,
) -> Result<HashMap<Ident<Canonical>, UnitMap>> {
    let time_units = canonicalize(units_ctx.sim_specs.time_units.as_deref().unwrap_or("time"))
        .as_str()
        .to_string();

    let units = UnitInferer {
        ctx: units_ctx,
        models,
        time: Variable::Var {
            ident: canonicalize("time"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: Some([(time_units, 1)].iter().cloned().collect()),
            tables: vec![],
            non_negative: false,
            is_flow: false,
            is_table_only: false,
            errors: vec![],
            unit_errors: vec![],
        },
    };

    units.infer(model)
}

#[test]
fn test_constraint_generation_consistency() {
    use crate::common::canonicalize;

    // Test that constraint generation produces consistent variable names
    // This simulates what happens when stdlib models are processed

    // In the stdlib XMILE file, the variable might be "Output" (capitalized)
    let xmile_var_name = "Output";

    // When it becomes an Ident<Canonical> key in the HashMap
    let map_key = canonicalize(xmile_var_name);
    assert_eq!(map_key.as_str(), "output", "Map key should be lowercase");

    // When used in constraint generation in line 366/376
    let constraint_var = format!("@{map_key}");
    assert_eq!(constraint_var, "@output");

    // But if the AST still references the canonical form...
    let ast_reference = canonicalize("output");
    let ast_constraint = format!("@{ast_reference}");
    assert_eq!(ast_constraint, "@output");

    // They should match!
    assert_eq!(constraint_var, ast_constraint);
}

#[test]
fn test_multi_metavar_constraint_mismatch() {
    // Test that we detect mismatches in constraints that contain multiple metavariables.
    // This is the P2 badge case: two derived variables with declared units m and s
    // both defined as a/b when neither a nor b has explicit units.
    //
    // This creates constraints:
    //   @x = @a/@b  (from x = a/b)
    //   @x = m      (from declared units of x)
    //   @y = @a/@b  (from y = a/b)
    //   @y = s      (from declared units of y)
    //
    // After unification, we get:
    //   m = @a/@b
    //   s = @a/@b
    //
    // These are contradictory: if @a/@b = m and @a/@b = s, then m = s.
    // But m != s, so we should detect this as a mismatch.

    let sim_specs = sim_specs_with_units("parsec");

    let test_case: &[(crate::datamodel::Variable, &'static str)] = &[
        (x_aux("a", "10", None), ""),                    // no units declared
        (x_aux("b", "2", None), ""),                     // no units declared
        (x_aux("x", "a / b", Some("meter")), "meter"),   // declared as meters
        (x_aux("y", "a / b", Some("second")), "second"), // declared as seconds
    ];

    let vars = test_case
        .iter()
        .map(|(var, _unit)| var)
        .cloned()
        .collect::<Vec<_>>();
    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let mut results: Result<HashMap<Ident<Canonical>, UnitMap>> =
        model_err!(UnitMismatch, "".to_owned());
    let _project = crate::project::Project::base_from(
        project_datamodel.clone(),
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    // The inference should fail because x and y have inconsistent unit declarations
    assert!(
        results.is_err(),
        "Should detect multi-metavar constraint mismatch"
    );
}

#[test]
fn test_find_constraint_mismatch_direct() {
    // Test the find_constraint_mismatch function directly
    use crate::datamodel::UnitMap;

    // Case 1: Direct concrete-only mismatch
    let constraints = vec![
        [("meter".to_owned(), 1), ("second".to_owned(), -1)]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
    ];
    let result = find_constraint_mismatch(&constraints);
    assert!(result.is_some(), "Should detect direct concrete mismatch");

    // Case 2: Pairwise mismatch with shared metavariables
    let constraints = vec![
        [
            ("@a".to_owned(), 1),
            ("@b".to_owned(), -1),
            ("meter".to_owned(), 1),
        ]
        .iter()
        .cloned()
        .collect::<UnitMap>(),
        [
            ("@a".to_owned(), 1),
            ("@b".to_owned(), -1),
            ("second".to_owned(), 1),
        ]
        .iter()
        .cloned()
        .collect::<UnitMap>(),
    ];
    let result = find_constraint_mismatch(&constraints);
    assert!(
        result.is_some(),
        "Should detect pairwise constraint mismatch"
    );

    // Case 3: No mismatch - same concrete units
    let constraints = vec![
        [
            ("@a".to_owned(), 1),
            ("@b".to_owned(), -1),
            ("meter".to_owned(), 1),
        ]
        .iter()
        .cloned()
        .collect::<UnitMap>(),
        [
            ("@c".to_owned(), 1),
            ("@d".to_owned(), -1),
            ("meter".to_owned(), 1),
        ]
        .iter()
        .cloned()
        .collect::<UnitMap>(),
    ];
    let result = find_constraint_mismatch(&constraints);
    // The ratio of these two would be @a/@b * @d/@c which still has metavariables
    assert!(
        result.is_none(),
        "Should not detect mismatch for different metavar structures"
    );

    // Case 4: No mismatch - under-constrained but not contradictory
    let constraints = vec![
        [("@a".to_owned(), 1), ("@b".to_owned(), -1)]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
        [("@c".to_owned(), 1), ("@d".to_owned(), -1)]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
    ];
    let result = find_constraint_mismatch(&constraints);
    assert!(
        result.is_none(),
        "Should not detect mismatch for purely under-constrained case"
    );
}
