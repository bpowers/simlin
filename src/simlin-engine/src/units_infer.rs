// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr2};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{Canonical, ErrorCode, Ident, UnitError, UnitResult, canonicalize};
use crate::datamodel::UnitMap;
use crate::model::ModelStage1;
#[cfg(test)]
use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
use crate::units::{Context, UnitOp, Units, combine};
use crate::variable::Variable;

/// Source of a constraint for error reporting.
/// Tracks which variable a constraint relates to and optionally where in that variable's equation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct ConstraintSource {
    /// Variable identifier with module prefix (e.g., "module1·varname")
    var: String,
    /// Location within that variable's equation (None for structural constraints like stock/flow)
    loc: Option<Loc>,
}

/// A constraint with source tracking for error reporting.
/// Each constraint represents an equation of the form `1 == unit_map`.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
struct LocatedConstraint {
    /// The unit map representing the constraint
    unit_map: UnitMap,
    /// Where this constraint originated (may have multiple sources for cross-variable constraints)
    sources: Vec<ConstraintSource>,
}

#[allow(dead_code)]
impl LocatedConstraint {
    /// Create a new constraint with a single source
    fn new(unit_map: UnitMap, var: &str, loc: Option<Loc>) -> Self {
        LocatedConstraint {
            unit_map,
            sources: vec![ConstraintSource {
                var: var.to_string(),
                loc,
            }],
        }
    }

    /// Add an additional source to this constraint
    fn with_source(mut self, var: &str, loc: Option<Loc>) -> Self {
        self.sources.push(ConstraintSource {
            var: var.to_string(),
            loc,
        });
        self
    }

    /// Merge sources from another constraint into this one
    fn merge_sources(&mut self, other: &LocatedConstraint) {
        for source in &other.sources {
            // Avoid duplicates
            if !self
                .sources
                .iter()
                .any(|s| s.var == source.var && s.loc == source.loc)
            {
                self.sources.push(source.clone());
            }
        }
    }

    /// Get the primary variable this constraint is about
    fn primary_var(&self) -> Option<&str> {
        self.sources.first().map(|s| s.var.as_str())
    }

    /// Get the primary location for error reporting
    fn primary_loc(&self) -> Option<Loc> {
        self.sources.first().and_then(|s| s.loc)
    }

    /// Check if the unit_map is empty (dimensionless/identity)
    fn is_empty(&self) -> bool {
        self.unit_map.is_empty()
    }
}

struct UnitInferer<'a> {
    ctx: &'a Context,
    models: &'a HashMap<Ident<Canonical>, &'a ModelStage1>,
    // units for module inputs
    time: Variable,
}

fn single_fv(units: &UnitMap) -> Option<&str> {
    let mut result = None;
    for (unit, exp) in units.map.iter() {
        if unit.starts_with('@') {
            // Only consider metavariables with exponent ±1.
            // If |exponent| > 1, we can't solve for this variable because it would
            // require fractional exponents (e.g., @x^2 = meters => @x = meters^(1/2)).
            if exp.abs() != 1 {
                return None;
            }
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
    // We have:
    //   `1 == $lhs`
    // where $lhs contains $var with exponent ±1 (ensured by single_fv check).
    // We want:
    //   `$var = $lhs'`
    // So if $var is in the numerator (exponent > 0) we want the
    // inverse of $lhs; otherwise (exponent < 0) just delete $var from $lhs.

    let inverse = if let Some(exponent) = lhs.map.remove(var) {
        // single_fv ensures we only get here with exponent ±1.
        // Use a regular assert since violating this invariant would produce
        // incorrect results (not just a performance issue).
        assert!(
            exponent.abs() == 1,
            "solve_for called with |exponent| != 1; single_fv should prevent this"
        );
        exponent > 0
    } else {
        false
    };

    if inverse { lhs.reciprocal() } else { lhs }
}

fn substitute(
    var: &str,
    units: &UnitMap,
    subst_sources: &[ConstraintSource],
    constraints: Vec<LocatedConstraint>,
) -> Vec<LocatedConstraint> {
    constraints
        .into_iter()
        .map(|mut c| {
            if let Some(exponent) = c.unit_map.map.remove(var) {
                // Scale the units by the exponent magnitude.
                // For example, if @x = seconds and we're substituting into @x^2,
                // we need seconds^2, not just seconds.
                let scaled_units = units.clone().exp(exponent.abs());

                let op = if exponent > 0 {
                    UnitOp::Mul
                } else {
                    UnitOp::Div
                };
                c.unit_map = combine(op, c.unit_map, scaled_units);

                // Merge sources from the substitution
                for source in subst_sources {
                    if !c
                        .sources
                        .iter()
                        .any(|s| s.var == source.var && s.loc == source.loc)
                    {
                        c.sources.push(source.clone());
                    }
                }
            }
            c
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
fn find_constraint_mismatch(constraints: &[LocatedConstraint]) -> Option<UnitError> {
    use std::collections::HashMap;
    use std::fmt::Write;

    // Group constraints by their metavariable signature.
    // Key: sorted string representation of metavar signature (for HashMap key)
    // Value: reference to the first LocatedConstraint with this signature, plus its residual
    let mut signature_groups: HashMap<String, (&LocatedConstraint, UnitMap)> = HashMap::new();

    for constraint in constraints {
        let (signature, residual) = split_constraint(&constraint.unit_map);

        // Case 1: No metavariables means this is a direct concrete mismatch
        if signature.map.is_empty() && !residual.map.is_empty() {
            let mut s = "unit checking failed; conflicting constraint:\n".to_owned();
            write!(s, "    1 == {}", constraint.unit_map).unwrap();
            return Some(UnitError::InferenceError {
                code: ErrorCode::UnitMismatch,
                sources: constraint
                    .sources
                    .iter()
                    .map(|s| (s.var.clone(), s.loc))
                    .collect(),
                details: Some(s),
            });
        }

        // Create a canonical string key for the signature (sorted for consistency)
        let sig_key = format!("{signature}");

        if let Some((first_constraint, first_residual)) = signature_groups.get(&sig_key) {
            // Case 2: Same signature but different residual means contradiction
            if residual != *first_residual {
                let mut s = "unit checking failed; inconsistent constraints:\n".to_owned();
                writeln!(s, "    1 == {}", first_constraint.unit_map).unwrap();
                writeln!(s, "    1 == {}", constraint.unit_map).unwrap();
                // The ratio of residuals shows the implied contradiction
                let implied = first_residual.clone() / residual;
                write!(s, "  These imply: 1 == {implied}").unwrap();

                // Combine sources from both constraints
                let mut all_sources: Vec<(String, Option<Loc>)> = first_constraint
                    .sources
                    .iter()
                    .map(|s| (s.var.clone(), s.loc))
                    .collect();
                for source in &constraint.sources {
                    if !all_sources
                        .iter()
                        .any(|(v, l)| v == &source.var && *l == source.loc)
                    {
                        all_sources.push((source.var.clone(), source.loc));
                    }
                }

                return Some(UnitError::InferenceError {
                    code: ErrorCode::UnitMismatch,
                    sources: all_sources,
                    details: Some(s),
                });
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
        current_var: &str,
        constraints: &mut Vec<LocatedConstraint>,
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
                | BuiltinFn::Sum(a) => self.gen_constraints(a, prefix, current_var, constraints),
                BuiltinFn::Mean(args) => {
                    let args = args
                        .iter()
                        .map(|arg| self.gen_constraints(arg, prefix, current_var, constraints))
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
                                    // Mean arguments must have same units
                                    constraints.push(LocatedConstraint::new(
                                        combine(UnitOp::Div, arg0.clone(), arg.clone()),
                                        current_var,
                                        Some(expr.get_loc()),
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
                    let a_units = self.gen_constraints(a, prefix, current_var, constraints)?;
                    if let Some(b) = b {
                        let b_units = self.gen_constraints(b, prefix, current_var, constraints)?;

                        if let Units::Explicit(ref lunits) = a_units
                            && let Units::Explicit(runits) = b_units
                        {
                            let loc = a.get_loc().union(&b.get_loc());
                            constraints.push(LocatedConstraint::new(
                                combine(UnitOp::Div, lunits.clone(), runits),
                                current_var,
                                Some(loc),
                            ));
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
                    let units = self.gen_constraints(&div, prefix, current_var, constraints)?;

                    // the optional argument to safediv, if specified, should match the units of a/b
                    if let Units::Explicit(ref result_units) = units
                        && let Some(c) = c
                        && let Units::Explicit(c_units) =
                            self.gen_constraints(c, prefix, current_var, constraints)?
                    {
                        constraints.push(LocatedConstraint::new(
                            combine(UnitOp::Div, c_units, result_units.clone()),
                            current_var,
                            Some(c.get_loc()),
                        ));
                    }

                    Ok(units)
                }
                BuiltinFn::Rank(a, _rest) => {
                    let a_units = self.gen_constraints(a, prefix, current_var, constraints)?;

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
            Expr2::Op1(_, l, _, _) => self.gen_constraints(l, prefix, current_var, constraints),
            Expr2::Op2(op, l, r, _, _) => {
                let lunits = self.gen_constraints(l, prefix, current_var, constraints)?;
                let runits = self.gen_constraints(r, prefix, current_var, constraints)?;

                match op {
                    BinaryOp::Add | BinaryOp::Sub => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Ok(Units::Constant),
                        (Units::Constant, Units::Explicit(units))
                        | (Units::Explicit(units), Units::Constant) => Ok(Units::Explicit(units)),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            let loc = l.get_loc().union(&r.get_loc());
                            constraints.push(LocatedConstraint::new(
                                combine(UnitOp::Div, lunits.clone(), runits),
                                current_var,
                                Some(loc),
                            ));
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
                let lunits = self.gen_constraints(l, prefix, current_var, constraints)?;
                let runits = self.gen_constraints(r, prefix, current_var, constraints)?;

                if let Units::Explicit(ref lunits) = lunits
                    && let Units::Explicit(runits) = runits
                {
                    let loc = l.get_loc().union(&r.get_loc());
                    constraints.push(LocatedConstraint::new(
                        combine(UnitOp::Div, lunits.clone(), runits),
                        current_var,
                        Some(loc),
                    ));
                }

                Ok(lunits)
            }
        }
    }

    fn gen_all_constraints(
        &self,
        model: &ModelStage1,
        prefix: &str,
        constraints: &mut Vec<LocatedConstraint>,
    ) {
        let time_units = canonicalize(self.ctx.sim_specs.time_units.as_deref().unwrap_or("time"))
            .as_str()
            .to_string();

        for (id, var) in model.variables.iter() {
            let current_var = format!("{prefix}{id}");

            if let Variable::Stock {
                ident,
                inflows,
                outflows,
                ..
            } = var
            {
                let stock_ident = ident;
                let stock_var = format!("{prefix}{stock_ident}");
                let expected = [
                    (format!("@{prefix}{stock_ident}"), 1),
                    (time_units.clone(), -1),
                ]
                .iter()
                .cloned()
                .collect::<UnitMap>();
                let mut check_flows = |flows: &Vec<Ident<Canonical>>| {
                    for flow_ident in flows.iter() {
                        let flow_var = format!("{prefix}{flow_ident}");
                        let flow_units: UnitMap = [(format!("@{prefix}{flow_ident}"), 1)]
                            .iter()
                            .cloned()
                            .collect();
                        // Stock/flow constraint: both stock and flow are sources, no equation location
                        constraints.push(
                            LocatedConstraint::new(
                                combine(UnitOp::Div, flow_units, expected.clone()),
                                &flow_var,
                                None,
                            )
                            .with_source(&stock_var, None),
                        );
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
                    let src_var = format!("{}{}", prefix, input.src);
                    let dst_var = format!("{}{}", subprefix, input.dst);
                    let src = format!("@{src_var}");
                    let dst = format!("@{dst_var}");
                    // src = dst === 1 = src/dst
                    let units: UnitMap = [(src, 1), (dst, -1)].iter().cloned().collect();
                    // Module input constraint: both caller and callee are sources
                    constraints.push(
                        LocatedConstraint::new(units, &src_var, None).with_source(&dst_var, None),
                    );
                }
                self.gen_all_constraints(submodel, &subprefix, constraints);
            }
            // we only should be adding constraints based on the equation if
            // the variable _doesn't_ have an associated lookup table/graphical
            // function.
            if var.table().is_none() {
                let var_units = match var.ast() {
                    Some(Ast::Scalar(ast)) => {
                        self.gen_constraints(ast, prefix, &current_var, constraints)
                    }
                    Some(Ast::ApplyToAll(_, ast)) => {
                        self.gen_constraints(ast, prefix, &current_var, constraints)
                    }
                    Some(Ast::Arrayed(_, asts)) => {
                        // For arrayed variables, each element may have a different expression,
                        // but all elements must have the same units. Process each expression
                        // and add a constraint tying each element's units to the array variable.
                        // If elements have conflicting units, this will be detected as a mismatch
                        // in the unify phase.
                        let array_var: UnitMap =
                            [(format!("@{prefix}{id}"), 1)].iter().cloned().collect();

                        let mut result: UnitResult<Units> = Ok(Units::Constant);
                        for (_element, expr) in asts.iter() {
                            match self.gen_constraints(expr, prefix, &current_var, constraints) {
                                Ok(expr_units) => {
                                    // Add a constraint tying this element's units to the array variable
                                    if let Units::Explicit(units) = expr_units {
                                        let element_var = format!("{current_var}[element]");
                                        constraints.push(LocatedConstraint::new(
                                            combine(UnitOp::Div, array_var.clone(), units),
                                            &element_var,
                                            Some(expr.get_loc()),
                                        ));
                                    }
                                }
                                Err(e) => {
                                    result = Err(e);
                                    break;
                                }
                            }
                        }
                        // Return Constant since we've added constraints directly above
                        // (the constraint from var_units below would be redundant)
                        result
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
                    let mv: UnitMap = [(format!("@{prefix}{id}"), 1)].iter().cloned().collect();
                    // Get the location from the AST for equation-based constraints
                    let loc = var.ast().map(|ast| match ast {
                        Ast::Scalar(expr) => expr.get_loc(),
                        Ast::ApplyToAll(_, expr) => expr.get_loc(),
                        Ast::Arrayed(_, asts) => {
                            // Use the first element's location if available
                            asts.values().next().map_or(Loc::default(), |e| e.get_loc())
                        }
                    });
                    constraints.push(LocatedConstraint::new(
                        combine(UnitOp::Div, mv, units),
                        &current_var,
                        loc,
                    ));
                }
            }
            if let Some(units) = var.units() {
                let mv: UnitMap = [(format!("@{prefix}{id}"), 1)].iter().cloned().collect();
                // User-defined unit declarations don't have equation locations
                constraints.push(LocatedConstraint::new(
                    combine(UnitOp::Div, mv, units.clone()),
                    &current_var,
                    None,
                ));
            }
        }
    }

    #[allow(clippy::type_complexity)]
    fn unify(
        &self,
        mut constraints: Vec<LocatedConstraint>,
    ) -> std::result::Result<
        (
            HashMap<Ident<Canonical>, UnitMap>,
            Option<Vec<LocatedConstraint>>,
        ),
        UnitError,
    > {
        let mut resolved_fvs: HashMap<Ident<Canonical>, UnitMap> = HashMap::new();
        // Track sources for each resolved variable in case of conflict
        let mut resolved_sources: HashMap<Ident<Canonical>, Vec<ConstraintSource>> = HashMap::new();
        let mut final_constraints: Vec<LocatedConstraint> = Vec::with_capacity(constraints.len());

        // FIXME: I think this is O(n^3) worst case; we could do better
        //        by maintaining an index of metavar usage -> Units
        loop {
            let initial_constraint_count = constraints.len();
            while let Some(c) = constraints.pop() {
                // dimensionless/identity unit: `1 == 1`; nothing to do
                if c.is_empty() {
                    continue;
                }
                if let Some(var) = single_fv(&c.unit_map) {
                    let var = var.to_owned();
                    let units = solve_for(&var, c.unit_map.clone());
                    let sources = c.sources.clone();
                    constraints = substitute(&var, &units, &sources, constraints);
                    final_constraints = substitute(&var, &units, &sources, final_constraints);
                    let var_key = var.strip_prefix('@').unwrap();
                    let var_ident = Ident::<Canonical>::from_str_unchecked(var_key);
                    if let Some(existing_units) = resolved_fvs.get(&var_ident) {
                        if *existing_units != units {
                            // Combine sources from both the new constraint and the existing one
                            let mut all_sources: Vec<(String, Option<Loc>)> =
                                c.sources.iter().map(|s| (s.var.clone(), s.loc)).collect();
                            if let Some(existing_sources) = resolved_sources.get(&var_ident) {
                                for s in existing_sources {
                                    if !all_sources.iter().any(|(v, l)| v == &s.var && *l == s.loc)
                                    {
                                        all_sources.push((s.var.clone(), s.loc));
                                    }
                                }
                            }
                            return Err(UnitError::InferenceError {
                                code: ErrorCode::UnitMismatch,
                                sources: all_sources,
                                details: Some(format!(
                                    "conflicting units for {}: {} vs {}",
                                    var, existing_units, units
                                )),
                            });
                        }
                    } else {
                        resolved_fvs.insert(var_ident.clone(), units);
                        resolved_sources.insert(var_ident, sources);
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

    fn infer(&self, model: &ModelStage1) -> UnitResult<HashMap<Ident<Canonical>, UnitMap>> {
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

            if let Some(err) = mismatch {
                Err(err)
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
            let mut results: UnitResult<HashMap<Ident<Canonical>, UnitMap>> =
                Ok(Default::default());
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
            let mut results: UnitResult<HashMap<Ident<Canonical>, UnitMap>> =
                Err(UnitError::InferenceError {
                    code: ErrorCode::UnitMismatch,
                    sources: vec![],
                    details: None,
                });
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

#[test]
fn test_inference_error_has_location() {
    let sim_specs = sim_specs_with_units("parsec");

    // Create a model with a known unit mismatch: input + TIME where input has widget units
    let vars = vec![
        x_aux("input", "6", Some("widget")),
        x_aux("bad", "input + TIME", None), // widget + parsec = mismatch
    ];
    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let mut results: UnitResult<HashMap<Ident<Canonical>, UnitMap>> = Ok(Default::default());
    let _project = crate::project::Project::base_from(
        project_datamodel.clone(),
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    // Verify that we got an error with location information
    match results {
        Err(UnitError::InferenceError {
            code,
            sources,
            details,
        }) => {
            assert_eq!(code, ErrorCode::UnitMismatch);
            // Should have at least one source with the variable name
            assert!(
                !sources.is_empty(),
                "inference error should have at least one source"
            );
            // At least one source should reference "bad" (the variable with the mismatch)
            let has_bad = sources.iter().any(|(var, _)| var == "bad");
            assert!(
                has_bad,
                "sources should contain 'bad' variable, got: {:?}",
                sources
            );
            // The source should have a location (non-None) for the equation-based constraint
            // Note: some sources may have None loc (e.g., from declarations without equations)
            let has_loc = sources.iter().any(|(_, loc)| loc.is_some());
            assert!(
                has_loc,
                "at least one source should have a location, got: {:?}",
                sources
            );
            // Verify there's a meaningful details message
            assert!(details.is_some(), "error should have details");
        }
        Ok(_) => panic!("expected inference error, got Ok"),
        Err(e) => panic!("expected InferenceError, got {:?}", e),
    }
}

pub(crate) fn infer(
    models: &HashMap<Ident<Canonical>, &ModelStage1>,
    units_ctx: &Context,
    model: &ModelStage1,
) -> UnitResult<HashMap<Ident<Canonical>, UnitMap>> {
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

    let mut results: UnitResult<HashMap<Ident<Canonical>, UnitMap>> =
        Err(UnitError::InferenceError {
            code: ErrorCode::UnitMismatch,
            sources: vec![],
            details: None,
        });
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

#[cfg(test)]
/// Helper to create a LocatedConstraint from a UnitMap for testing
fn test_constraint(unit_map: UnitMap) -> LocatedConstraint {
    LocatedConstraint::new(unit_map, "test", None)
}

#[test]
fn test_find_constraint_mismatch_direct() {
    // Test the find_constraint_mismatch function directly
    use crate::datamodel::UnitMap;

    // Case 1: Direct concrete-only mismatch
    let constraints = vec![test_constraint(
        [("meter".to_owned(), 1), ("second".to_owned(), -1)]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
    )];
    let result = find_constraint_mismatch(&constraints);
    assert!(result.is_some(), "Should detect direct concrete mismatch");

    // Case 2: Pairwise mismatch with shared metavariables
    let constraints = vec![
        test_constraint(
            [
                ("@a".to_owned(), 1),
                ("@b".to_owned(), -1),
                ("meter".to_owned(), 1),
            ]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
        ),
        test_constraint(
            [
                ("@a".to_owned(), 1),
                ("@b".to_owned(), -1),
                ("second".to_owned(), 1),
            ]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
        ),
    ];
    let result = find_constraint_mismatch(&constraints);
    assert!(
        result.is_some(),
        "Should detect pairwise constraint mismatch"
    );

    // Case 3: No mismatch - same concrete units
    let constraints = vec![
        test_constraint(
            [
                ("@a".to_owned(), 1),
                ("@b".to_owned(), -1),
                ("meter".to_owned(), 1),
            ]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
        ),
        test_constraint(
            [
                ("@c".to_owned(), 1),
                ("@d".to_owned(), -1),
                ("meter".to_owned(), 1),
            ]
            .iter()
            .cloned()
            .collect::<UnitMap>(),
        ),
    ];
    let result = find_constraint_mismatch(&constraints);
    // The ratio of these two would be @a/@b * @d/@c which still has metavariables
    assert!(
        result.is_none(),
        "Should not detect mismatch for different metavar structures"
    );

    // Case 4: No mismatch - under-constrained but not contradictory
    let constraints = vec![
        test_constraint(
            [("@a".to_owned(), 1), ("@b".to_owned(), -1)]
                .iter()
                .cloned()
                .collect::<UnitMap>(),
        ),
        test_constraint(
            [("@c".to_owned(), 1), ("@d".to_owned(), -1)]
                .iter()
                .cloned()
                .collect::<UnitMap>(),
        ),
    ];
    let result = find_constraint_mismatch(&constraints);
    assert!(
        result.is_none(),
        "Should not detect mismatch for purely under-constrained case"
    );
}

#[test]
fn test_located_constraint_merge_sources() {
    use crate::builtins::Loc;

    // Test merge_sources deduplication
    let mut constraint1 = LocatedConstraint::new(UnitMap::new(), "var_a", Some(Loc::new(0, 10)));
    let constraint2 = LocatedConstraint::new(UnitMap::new(), "var_b", Some(Loc::new(5, 15)));

    // Merge sources from constraint2 into constraint1
    constraint1.merge_sources(&constraint2);

    assert_eq!(constraint1.sources.len(), 2, "Should have both sources");
    assert_eq!(constraint1.sources[0].var, "var_a");
    assert_eq!(constraint1.sources[1].var, "var_b");

    // Merging again should not add duplicate
    constraint1.merge_sources(&constraint2);
    assert_eq!(
        constraint1.sources.len(),
        2,
        "Should not add duplicate source"
    );

    // But a different location for the same variable should be added
    let constraint3 = LocatedConstraint::new(UnitMap::new(), "var_b", Some(Loc::new(20, 30)));
    constraint1.merge_sources(&constraint3);
    assert_eq!(
        constraint1.sources.len(),
        3,
        "Should add source with different location"
    );

    // Test merging with None location
    let constraint4 = LocatedConstraint::new(UnitMap::new(), "var_c", None);
    constraint1.merge_sources(&constraint4);
    assert_eq!(constraint1.sources.len(), 4);

    // Merging same var with None location again should not duplicate
    constraint1.merge_sources(&constraint4);
    assert_eq!(
        constraint1.sources.len(),
        4,
        "Should not add duplicate None location"
    );
}

#[test]
fn test_located_constraint_primary_accessors() {
    use crate::builtins::Loc;

    // Test primary_var and primary_loc with sources
    let constraint = LocatedConstraint::new(UnitMap::new(), "primary_var", Some(Loc::new(5, 15)));

    assert_eq!(
        constraint.primary_var(),
        Some("primary_var"),
        "primary_var should return first source's variable"
    );
    assert_eq!(
        constraint.primary_loc(),
        Some(Loc::new(5, 15)),
        "primary_loc should return first source's location"
    );

    // Test with None location
    let constraint_no_loc = LocatedConstraint::new(UnitMap::new(), "another_var", None);
    assert_eq!(constraint_no_loc.primary_var(), Some("another_var"));
    assert_eq!(
        constraint_no_loc.primary_loc(),
        None,
        "primary_loc should be None when source has no location"
    );

    // Test with_source chaining
    let constraint_multi = LocatedConstraint::new(UnitMap::new(), "first", Some(Loc::new(1, 2)))
        .with_source("second", Some(Loc::new(3, 4)));
    assert_eq!(constraint_multi.sources.len(), 2);
    assert_eq!(
        constraint_multi.primary_var(),
        Some("first"),
        "primary_var should still be first source"
    );
}

#[test]
fn test_located_constraint_is_empty() {
    // Test is_empty on LocatedConstraint
    let empty_constraint = LocatedConstraint::new(UnitMap::new(), "test", None);
    assert!(
        empty_constraint.is_empty(),
        "Empty UnitMap should make constraint empty"
    );

    let non_empty: UnitMap = [("meter".to_owned(), 1)].iter().cloned().collect();
    let non_empty_constraint = LocatedConstraint::new(non_empty, "test", None);
    assert!(
        !non_empty_constraint.is_empty(),
        "Non-empty UnitMap should not be empty"
    );
}

#[test]
fn test_rank_builtin_unit_inference() {
    // Test that RANK builtin is handled correctly in unit inference
    // RANK returns the same units as its first argument (the array being ranked)
    let sim_specs = sim_specs_with_units("year");

    let vars = [
        (x_aux("values", "10", Some("dollar")), "dollar"),
        (x_aux("ranking", "RANK(values, 1)", None), "dollar"),
    ];

    let expected: HashMap<&str, &str> = vars
        .iter()
        .map(|(var, units)| (var.get_ident(), *units))
        .collect();
    let model_vars: Vec<_> = vars.iter().map(|(var, _)| var.clone()).collect();
    let model = x_model("main", model_vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let units_ctx = Context::new_with_builtins(&[], &sim_specs).unwrap();
    let mut results: UnitResult<HashMap<Ident<Canonical>, UnitMap>> = Ok(Default::default());
    let _project = crate::project::Project::base_from(
        project_datamodel.clone(),
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    let results = results.expect("RANK inference should succeed");
    for (ident, expected_units) in expected.iter() {
        let expected_units: UnitMap = crate::units::parse_units(&units_ctx, Some(expected_units))
            .unwrap()
            .unwrap();
        if let Some(computed_units) = results.get(&canonicalize(ident)) {
            assert_eq!(
                expected_units, *computed_units,
                "Units for {} should match",
                ident
            );
        }
    }
}

#[test]
fn test_unify_conflict_detection() {
    // Test that unify() detects when the same variable is resolved to different units
    // This exercises the code path at lines 656-680 in unify()
    let sim_specs = sim_specs_with_units("year");

    // Create a model where the same undeclared variable gets constrained to two different units
    // x = a (no units on x or a)
    // y = a * 1 {meter} (forces a to be meters through y's declared units)
    // z = a * 1 {second} (forces a to be seconds through z's declared units)
    // This creates a conflict: a can't be both meters and seconds
    let vars = vec![
        x_aux("a", "10", None),          // undeclared
        x_aux("x", "a", None),           // uses a
        x_aux("y", "a", Some("meter")),  // declares y as meters, constrains a
        x_aux("z", "a", Some("second")), // declares z as seconds, constrains a
    ];

    let model_vars: Vec<_> = vars.into_iter().collect();
    let model = x_model("main", model_vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let mut results: UnitResult<HashMap<Ident<Canonical>, UnitMap>> = Ok(Default::default());
    let _project = crate::project::Project::base_from(
        project_datamodel.clone(),
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    // This should fail because 'a' can't be both meters and seconds
    assert!(
        results.is_err(),
        "Should detect conflict when same variable has different unit constraints"
    );

    // Verify we get an InferenceError with the right code
    match results {
        Err(UnitError::InferenceError { code, sources, .. }) => {
            assert_eq!(code, ErrorCode::UnitMismatch);
            // Should have sources indicating which variables are involved
            assert!(
                !sources.is_empty(),
                "Should have source information for conflict"
            );
        }
        Err(e) => panic!("Expected InferenceError, got {:?}", e),
        Ok(_) => panic!("Expected error, got success"),
    }
}

#[test]
fn test_substitute_handles_higher_exponents() {
    // Test that substitute correctly handles exponents > 1
    // If @x = seconds and we substitute into @x^2 * meters, we should get seconds^2 * meters

    let var = "@x";
    let units: UnitMap = [("seconds".to_owned(), 1)].iter().cloned().collect();
    let sources = vec![];

    // Constraint: 1 == @x^2 * meters  (i.e., @x^2 = 1/meters)
    let constraint: UnitMap = [("@x".to_owned(), 2), ("meters".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();

    let constraints = vec![test_constraint(constraint)];
    let result = substitute(var, &units, &sources, constraints);

    // After substitution: 1 == seconds^2 * meters
    assert_eq!(result.len(), 1);
    let result_map = &result[0].unit_map;

    // Should have seconds^2 (exponent 2) and meters^1
    assert_eq!(
        result_map.map.get("seconds"),
        Some(&2),
        "seconds should have exponent 2 after substitution"
    );
    assert_eq!(
        result_map.map.get("meters"),
        Some(&1),
        "meters should have exponent 1"
    );
    assert!(
        !result_map.map.contains_key("@x"),
        "@x should be removed after substitution"
    );
}

#[test]
fn test_substitute_handles_negative_higher_exponents() {
    // Test that substitute correctly handles exponents < -1
    // If @x = seconds and we substitute into @x^-2 * meters, we should get seconds^-2 * meters

    let var = "@x";
    let units: UnitMap = [("seconds".to_owned(), 1)].iter().cloned().collect();
    let sources = vec![];

    // Constraint: 1 == @x^-2 * meters  (i.e., meters/@x^2 = 1)
    let constraint: UnitMap = [("@x".to_owned(), -2), ("meters".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();

    let constraints = vec![test_constraint(constraint)];
    let result = substitute(var, &units, &sources, constraints);

    assert_eq!(result.len(), 1);
    let result_map = &result[0].unit_map;

    // Should have seconds^-2 and meters^1
    assert_eq!(
        result_map.map.get("seconds"),
        Some(&-2),
        "seconds should have exponent -2 after substitution"
    );
    assert_eq!(
        result_map.map.get("meters"),
        Some(&1),
        "meters should have exponent 1"
    );
}

#[test]
fn test_solve_for_skips_higher_exponents() {
    // Test that solve_for returns None for constraints with |exponent| > 1
    // because we can't represent fractional exponents (e.g., sqrt(meters))

    // Constraint: 1 == @x^2 * meters  =>  @x = meters^(-1/2), which we can't represent
    let constraint: UnitMap = [("@x".to_owned(), 2), ("meters".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();

    // single_fv should return None because @x has exponent 2, not ±1
    let fv = single_fv(&constraint);
    assert!(
        fv.is_none(),
        "single_fv should return None for metavariables with |exponent| > 1"
    );
}

#[test]
fn test_single_fv_with_exponent_1() {
    // Test that single_fv works correctly for exponent ±1

    // @x^1 * meters => should return Some("@x")
    let constraint1: UnitMap = [("@x".to_owned(), 1), ("meters".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();
    assert_eq!(single_fv(&constraint1), Some("@x"));

    // @x^-1 * meters => should return Some("@x")
    let constraint2: UnitMap = [("@x".to_owned(), -1), ("meters".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();
    assert_eq!(single_fv(&constraint2), Some("@x"));

    // @x^1 * @y^1 => should return None (multiple metavariables)
    let constraint3: UnitMap = [("@x".to_owned(), 1), ("@y".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();
    assert_eq!(single_fv(&constraint3), None);
}
