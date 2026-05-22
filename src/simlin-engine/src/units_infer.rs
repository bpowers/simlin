// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr2};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{Canonical, ErrorCode, Ident, UnitError, canonicalize};
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

/// The result of unit inference for a model.
///
/// Inference is *partial*: `resolved` holds every metavariable the solver
/// could pin to a concrete unit, and `conflicts` holds every dimensional
/// contradiction it found. A conflict in one connected component of the
/// constraint graph cannot affect another (substitution only flows along
/// shared metavariables), so a single bad equation no longer discards the
/// units resolved for the rest of the model -- and the conflict set is
/// complete rather than just the first contradiction encountered (GH #614).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Default)]
pub(crate) struct InferenceResult {
    pub resolved: HashMap<Ident<Canonical>, UnitMap>,
    pub conflicts: Vec<UnitError>,
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

/// Maintains a `Vec<LocatedConstraint>` with an inverted index from metavar
/// names to constraint indices, so that `substitute` only visits constraints
/// that actually contain the target metavar.
#[derive(Default)]
struct ConstraintSet {
    constraints: Vec<LocatedConstraint>,
    /// Maps metavar name (keys starting with '@') to indices in `constraints`.
    metavar_index: HashMap<String, Vec<usize>>,
}

impl ConstraintSet {
    fn from_vec(constraints: Vec<LocatedConstraint>) -> Self {
        let mut metavar_index: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, c) in constraints.iter().enumerate() {
            for key in c.unit_map.map.keys() {
                if key.starts_with('@') {
                    metavar_index.entry(key.clone()).or_default().push(i);
                }
            }
        }
        ConstraintSet {
            constraints,
            metavar_index,
        }
    }

    fn pop(&mut self) -> Option<LocatedConstraint> {
        let c = self.constraints.pop()?;
        let idx = self.constraints.len();
        for key in c.unit_map.map.keys() {
            if key.starts_with('@')
                && let Some(indices) = self.metavar_index.get_mut(key)
            {
                // The popped element is always the last, so its index
                // is the largest value in the Vec -- just pop it.
                if indices.last() == Some(&idx) {
                    indices.pop();
                } else {
                    indices.retain(|&i| i != idx);
                }
                if indices.is_empty() {
                    self.metavar_index.remove(key);
                }
            }
        }
        Some(c)
    }

    fn push(&mut self, c: LocatedConstraint) {
        let idx = self.constraints.len();
        for key in c.unit_map.map.keys() {
            if key.starts_with('@') {
                self.metavar_index.entry(key.clone()).or_default().push(idx);
            }
        }
        self.constraints.push(c);
    }

    fn substitute(&mut self, var: &str, units: &UnitMap, subst_sources: &[ConstraintSource]) {
        let affected = match self.metavar_index.remove(var) {
            Some(indices) => indices,
            None => return,
        };
        for idx in &affected {
            let c = &mut self.constraints[*idx];
            let exponent = match c.unit_map.map.remove(var) {
                Some(e) => e,
                None => continue,
            };

            let scaled_units = if exponent.abs() == 1 {
                units.clone()
            } else {
                units.clone().exp(exponent.abs())
            };

            let op = if exponent > 0 {
                UnitOp::Mul
            } else {
                UnitOp::Div
            };
            let taken = std::mem::take(&mut c.unit_map);
            c.unit_map = combine(op, taken, scaled_units);

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
    }

    fn len(&self) -> usize {
        self.constraints.len()
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }

    fn into_vec(self) -> Vec<LocatedConstraint> {
        self.constraints
    }
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
/// Find every dimensional contradiction among the residual (post-solve)
/// constraints, rather than just the first. Collecting them all -- instead of
/// short-circuiting on the first -- gives a complete diagnostic set in one pass
/// and makes the reported set independent of which contradiction the solver
/// happens to reach first (GH #614, and mitigates the order-dependence in
/// GH #474).
///
/// Two kinds of contradiction:
///
/// 1. A constraint with only concrete units (no metavariables) that isn't
///    dimensionless -- e.g. `meters == seconds`, which is impossible.
/// 2. Two constraints with the same metavariable "signature" but different
///    concrete "residuals" -- e.g. `@a/@b * meters == 1` and
///    `@a/@b * seconds == 1` imply `meters == seconds`.
///
/// Grouping by signature keeps this O(n) rather than O(n^2) pairwise.
fn find_constraint_mismatches(constraints: &[LocatedConstraint]) -> Vec<UnitError> {
    use std::collections::HashMap;
    use std::fmt::Write;

    let mut mismatches: Vec<UnitError> = Vec::new();

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
            mismatches.push(UnitError::InferenceError {
                code: ErrorCode::UnitMismatch,
                sources: constraint
                    .sources
                    .iter()
                    .map(|s| (s.var.clone(), s.loc))
                    .collect(),
                details: Some(s),
            });
            continue;
        }

        // Create a canonical string key for the signature (sorted for consistency)
        let sig_key = format!("{signature}");

        if let Some((first_constraint, first_residual)) = signature_groups.get(&sig_key) {
            // Case 2: Same signature but different residual means contradiction.
            // We compare every member against the group's first residual, so a
            // signature with k distinct residuals yields k-1 mismatches; exact
            // duplicates are deduped by the caller.
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

                mismatches.push(UnitError::InferenceError {
                    code: ErrorCode::UnitMismatch,
                    sources: all_sources,
                    details: Some(s),
                });
            }
        } else {
            signature_groups.insert(sig_key, (constraint, residual));
        }
    }

    mismatches
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
    ) -> Units {
        // Constraint generation is total: every well-formed expression yields
        // a `Units` value (and pushes zero or more `1 == UnitMap` constraints).
        // Dimensional *inconsistency* is detected later, during solving
        // (`unify`/`find_constraint_mismatch`), never here -- so this function
        // does not return a `Result` and cannot fail.
        match expr {
            Expr2::Const(_, _, _) => Units::Constant,
            Expr2::Var(ident, _, _loc) => {
                let units: UnitMap = [(format!("@{prefix}{ident}"), 1)].iter().cloned().collect();

                Units::Explicit(units)
            }
            Expr2::App(builtin, _, _) => match builtin {
                BuiltinFn::Inf | BuiltinFn::Pi => Units::Constant,
                BuiltinFn::Time
                | BuiltinFn::TimeStep
                | BuiltinFn::StartTime
                | BuiltinFn::FinalTime => {
                    Units::Explicit(self.time.units().cloned().unwrap_or_default())
                }
                BuiltinFn::IsModuleInput(_, _) => {
                    // returns a bool, which is unitless
                    Units::Explicit(UnitMap::new())
                }
                BuiltinFn::Lookup(table_expr, _, _loc)
                | BuiltinFn::LookupForward(table_expr, _, _loc)
                | BuiltinFn::LookupBackward(table_expr, _, _loc) => {
                    // lookups have the units specified on the table
                    let table_name = match table_expr.as_ref() {
                        Expr2::Var(name, _, _) => name.as_str(),
                        Expr2::Subscript(name, _, _, _) => name.as_str(),
                        _ => return Units::Constant,
                    };
                    let units: UnitMap = [(format!("@{prefix}{table_name}"), 1)]
                        .iter()
                        .cloned()
                        .collect();

                    Units::Explicit(units)
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
                        .collect::<Vec<_>>();

                    if args.is_empty() {
                        return Units::Constant;
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
                            Units::Explicit(arg0)
                        }
                        Some(Units::Constant) => Units::Constant,
                        None => Units::Constant,
                    }
                }
                BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                    let a_units = self.gen_constraints(a, prefix, current_var, constraints);
                    if let Some(b) = b {
                        let b_units = self.gen_constraints(b, prefix, current_var, constraints);

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
                    a_units
                }
                BuiltinFn::Quantum(a, _) => {
                    self.gen_constraints(a, prefix, current_var, constraints)
                }
                BuiltinFn::Sshape(_, bottom, _) => {
                    self.gen_constraints(bottom, prefix, current_var, constraints)
                }
                BuiltinFn::Pulse(_, _, _) | BuiltinFn::Ramp(_, _, _) | BuiltinFn::Step(_, _) => {
                    Units::Constant
                }
                BuiltinFn::SafeDiv(a, b, c) => {
                    let div = Expr2::Op2(
                        BinaryOp::Div,
                        a.clone(),
                        b.clone(),
                        None,
                        a.get_loc().union(&b.get_loc()),
                    );
                    let units = self.gen_constraints(&div, prefix, current_var, constraints);

                    // the optional argument to safediv, if specified, should match the units of a/b
                    if let Units::Explicit(ref result_units) = units
                        && let Some(c) = c
                        && let Units::Explicit(c_units) =
                            self.gen_constraints(c, prefix, current_var, constraints)
                    {
                        constraints.push(LocatedConstraint::new(
                            combine(UnitOp::Div, c_units, result_units.clone()),
                            current_var,
                            Some(c.get_loc()),
                        ));
                    }

                    units
                }
                BuiltinFn::Rank(a, _) => {
                    // Walk the ranked array so any constraints inside `a` are
                    // generated, but discard its units: a RANK result is a
                    // dimensionless position/index (like a comparison result),
                    // not the units of the array being ranked. The direction
                    // argument is a unitless control input.
                    self.gen_constraints(a, prefix, current_var, constraints);
                    Units::Explicit(UnitMap::new())
                }
                BuiltinFn::VectorSelect(_, expr_array, _, _, _) => {
                    self.gen_constraints(expr_array, prefix, current_var, constraints)
                }
                BuiltinFn::VectorElmMap(source, _) => {
                    self.gen_constraints(source, prefix, current_var, constraints)
                }
                BuiltinFn::VectorSortOrder(_, _) => Units::Constant,
                BuiltinFn::AllocateAvailable(req, _, _)
                | BuiltinFn::AllocateByPriority(req, _, _, _, _) => {
                    self.gen_constraints(req, prefix, current_var, constraints)
                }
                // Previous(x, fallback) and Init(x) preserve the units of the
                // lagged/current argument; the fallback must be compatible.
                BuiltinFn::Previous(a, b) => {
                    let a_units = self.gen_constraints(a, prefix, current_var, constraints);
                    let b_units = self.gen_constraints(b, prefix, current_var, constraints);
                    // Constrain fallback to match the lagged argument's units,
                    // analogous to Max/Min handling.
                    if let Units::Explicit(ref a_map) = a_units
                        && let Units::Explicit(b_map) = b_units
                    {
                        let loc = a.get_loc().union(&b.get_loc());
                        constraints.push(LocatedConstraint::new(
                            combine(UnitOp::Div, a_map.clone(), b_map),
                            current_var,
                            Some(loc),
                        ));
                    }
                    a_units
                }
                BuiltinFn::Init(a) => self.gen_constraints(a, prefix, current_var, constraints),
            },
            Expr2::Subscript(base_name, _, _, _) => {
                // A subscripted expression has the same units as the base array
                let units: UnitMap = [(format!("@{prefix}{base_name}"), 1)]
                    .iter()
                    .cloned()
                    .collect();
                Units::Explicit(units)
            }
            Expr2::Op1(_, l, _, _) => self.gen_constraints(l, prefix, current_var, constraints),
            Expr2::Op2(op, l, r, _, _) => {
                let lunits = self.gen_constraints(l, prefix, current_var, constraints);
                let runits = self.gen_constraints(r, prefix, current_var, constraints);

                match op {
                    BinaryOp::Add | BinaryOp::Sub => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Units::Constant,
                        (Units::Constant, Units::Explicit(units))
                        | (Units::Explicit(units), Units::Constant) => Units::Explicit(units),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            let loc = l.get_loc().union(&r.get_loc());
                            constraints.push(LocatedConstraint::new(
                                combine(UnitOp::Div, lunits.clone(), runits),
                                current_var,
                                Some(loc),
                            ));
                            Units::Explicit(lunits)
                        }
                    },
                    BinaryOp::Exp | BinaryOp::Mod => lunits,
                    BinaryOp::Mul => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Units::Constant,
                        (Units::Explicit(units), Units::Constant)
                        | (Units::Constant, Units::Explicit(units)) => Units::Explicit(units),
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            Units::Explicit(combine(UnitOp::Mul, lunits, runits))
                        }
                    },
                    BinaryOp::Div => match (lunits, runits) {
                        (Units::Constant, Units::Constant) => Units::Constant,
                        (Units::Explicit(units), Units::Constant) => Units::Explicit(units),
                        (Units::Constant, Units::Explicit(units)) => {
                            Units::Explicit(combine(UnitOp::Div, UnitMap::new(), units))
                        }
                        (Units::Explicit(lunits), Units::Explicit(runits)) => {
                            Units::Explicit(combine(UnitOp::Div, lunits, runits))
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
                        Units::Explicit(UnitMap::new())
                    }
                }
            }
            Expr2::If(_, l, r, _, _) => {
                let lunits = self.gen_constraints(l, prefix, current_var, constraints);
                let runits = self.gen_constraints(r, prefix, current_var, constraints);

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

                lunits
            }
        }
    }

    fn gen_all_constraints(
        &self,
        model: &ModelStage1,
        prefix: &str,
        constraints: &mut Vec<LocatedConstraint>,
    ) {
        let time_units_name =
            canonicalize(self.ctx.sim_specs.time_units.as_deref().unwrap_or("time")).into_owned();
        // Resolve the time unit through the units Context's alias map so that
        // inference uses the same canonical time unit as `units_check::check`.
        // Without this, a model that declares some units with an aliased time
        // name (e.g. `yr`) while `time_units` names the primary (`year`)
        // produces a spurious `year` vs `yr` mismatch on every stock/flow
        // constraint.
        let time_units: UnitMap = self
            .ctx
            .lookup(&time_units_name)
            .cloned()
            .unwrap_or_else(|| [(time_units_name.clone(), 1)].iter().cloned().collect());

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
                // expected = @stock / time_units (the units a flow must carry).
                let expected = combine(
                    UnitOp::Div,
                    [(format!("@{prefix}{stock_ident}"), 1)]
                        .iter()
                        .cloned()
                        .collect::<UnitMap>(),
                    time_units.clone(),
                );
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
                    Some(Ast::Arrayed(_, asts, default_expr, _)) => {
                        // For arrayed variables, each element may have a different expression,
                        // but all elements must have the same units. Process each expression
                        // and add a constraint tying each element's units to the array variable.
                        // If elements have conflicting units, this will be detected as a mismatch
                        // in the unify phase.
                        let array_var: UnitMap =
                            [(format!("@{prefix}{id}"), 1)].iter().cloned().collect();

                        for (_element, expr) in asts.iter() {
                            let expr_units =
                                self.gen_constraints(expr, prefix, &current_var, constraints);
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
                        if let Some(default_expr) = default_expr {
                            let expr_units = self.gen_constraints(
                                default_expr,
                                prefix,
                                &current_var,
                                constraints,
                            );
                            if let Units::Explicit(units) = expr_units {
                                constraints.push(LocatedConstraint::new(
                                    combine(UnitOp::Div, array_var.clone(), units),
                                    &format!("{current_var}[default]"),
                                    Some(default_expr.get_loc()),
                                ));
                            }
                        }
                        // We added the per-element constraints directly above, so the
                        // array variable itself contributes no further equation
                        // constraint here (the `Units::Explicit` branch below would be
                        // redundant).
                        Units::Constant
                    }
                    None => {
                        // No parsed equation -- e.g. an empty/not-yet-written equation
                        // or a module-input placeholder. There is no equation-derived
                        // constraint to add, but we must NOT skip the variable: we fall
                        // through to the `var.units()` constraint below so a variable
                        // with declared units but no equation still informs inference of
                        // its dependents.
                        Units::Constant
                    }
                };
                // Constants don't generate constraints - they adopt units from context
                // (e.g., in "x + 1", the 1 has the same units as x)
                if let Units::Explicit(units) = var_units {
                    let mv: UnitMap = [(format!("@{prefix}{id}"), 1)].iter().cloned().collect();
                    // Get the location from the AST for equation-based constraints
                    let loc = var.ast().map(|ast| match ast {
                        Ast::Scalar(expr) => expr.get_loc(),
                        Ast::ApplyToAll(_, expr) => expr.get_loc(),
                        Ast::Arrayed(_, asts, default_expr, _) => {
                            // Use the first element's location if available
                            asts.values().next().map_or_else(
                                || {
                                    default_expr
                                        .as_ref()
                                        .map_or(Loc::default(), |e| e.get_loc())
                                },
                                |e| e.get_loc(),
                            )
                        }
                    });
                    constraints.push(LocatedConstraint::new(
                        combine(UnitOp::Div, mv, units),
                        &current_var,
                        loc,
                    ));
                }
            }
            // A macro is a polymorphic template: its body variables' declared
            // units may name the macro's formal parameters (a Vensim idiom,
            // e.g. `~ xfrom` inside RAMP FROM TO), which would otherwise leak
            // the parameter name as a literal base unit into every
            // instantiation and conflict with the real argument units. So we
            // skip declared-units constraints for macro bodies and let those
            // units be inferred polymorphically from the equation and the
            // cross-module parameter bindings instead. (This mirrors
            // `check_model_units`, which skips unit-checking macro models
            // entirely.)
            if !model.is_macro
                && let Some(units) = var.units()
            {
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
    /// Solve the constraint system by Gaussian-elimination-style substitution,
    /// returning the resolved metavariable units, the residual (still
    /// metavariable-bearing) constraints, and any dimensional conflicts found
    /// while solving.
    ///
    /// A conflict is recorded and solving continues with the *first* binding
    /// kept, rather than aborting (GH #614). Because substitution only ever
    /// flows along shared metavariables, a contradiction is confined to its own
    /// connected component of the constraint graph: keeping going can never
    /// corrupt the units resolved for an independent component.
    #[allow(clippy::type_complexity)]
    fn unify(
        &self,
        constraints: Vec<LocatedConstraint>,
    ) -> (
        HashMap<Ident<Canonical>, UnitMap>,
        Vec<LocatedConstraint>,
        Vec<UnitError>,
    ) {
        let mut resolved_fvs: HashMap<Ident<Canonical>, UnitMap> = HashMap::new();
        // Track sources for each resolved variable in case of conflict
        let mut resolved_sources: HashMap<Ident<Canonical>, Vec<ConstraintSource>> = HashMap::new();
        let mut conflicts: Vec<UnitError> = Vec::new();
        let mut pending = ConstraintSet::from_vec(constraints);
        let mut finalized = ConstraintSet::default();

        loop {
            let initial_constraint_count = pending.len();
            while let Some(c) = pending.pop() {
                if c.is_empty() {
                    continue;
                }
                if let Some(var) = single_fv(&c.unit_map) {
                    let var = var.to_owned();
                    let units = solve_for(&var, c.unit_map.clone());
                    let sources = c.sources.clone();
                    let var_key = var.strip_prefix('@').unwrap();
                    let var_ident = Ident::<Canonical>::from_str_unchecked(var_key);
                    // Decide whether to accept this binding BEFORE substituting it,
                    // so a rejected (conflicting) re-derivation can never overwrite
                    // the kept binding in the remaining constraints -- substitution
                    // only ever propagates the binding we actually accept.
                    //
                    // The conflict arm is currently unreachable: `substitute`
                    // removes the metavariable from every remaining constraint, so a
                    // metavariable is solved at most once and `resolved_fvs` never
                    // already contains it here. A genuine over-constraint (the same
                    // metavariable forced to two different units) instead surfaces as
                    // a residual concrete contradiction (e.g. `meter == second`)
                    // reported by `find_constraint_mismatches`. The arm is kept, and
                    // ordered so it never substitutes the rejected units, so "keep
                    // the first binding" stays correct if that invariant ever changes.
                    if let Some(existing_units) = resolved_fvs.get(&var_ident) {
                        if *existing_units != units {
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
                            conflicts.push(UnitError::InferenceError {
                                code: ErrorCode::UnitMismatch,
                                sources: all_sources,
                                details: Some(format!(
                                    "conflicting units for {}: {} vs {}",
                                    var, existing_units, units
                                )),
                            });
                        }
                        // Keep the first binding either way: do NOT substitute the
                        // re-derived units into the remaining constraints.
                    } else {
                        pending.substitute(&var, &units, &sources);
                        finalized.substitute(&var, &units, &sources);
                        resolved_fvs.insert(var_ident.clone(), units);
                        resolved_sources.insert(var_ident, sources);
                    }
                } else {
                    finalized.push(c);
                }
            }
            if finalized.len() == initial_constraint_count {
                break;
            } else {
                pending = std::mem::take(&mut finalized);
            }
        }

        (resolved_fvs, finalized.into_vec(), conflicts)
    }

    fn infer(&self, model: &ModelStage1) -> InferenceResult {
        let mut constraints = vec![];
        self.gen_all_constraints(model, "", &mut constraints);

        let (resolved, leftover, mut conflicts) = self.unify(constraints);

        // Leftover constraints that still contain metavariables just mean the
        // model is under-constrained (e.g. undeclared units) -- not an error.
        // Only a concrete contradiction among them is a real mismatch.
        conflicts.extend(find_constraint_mismatches(&leftover));

        // The same contradiction can be reached both while solving and as a
        // leftover constraint; drop exact duplicates so each is reported once.
        let mut deduped: Vec<UnitError> = Vec::with_capacity(conflicts.len());
        for conflict in conflicts {
            if !deduped.contains(&conflict) {
                deduped.push(conflict);
            }
        }

        InferenceResult {
            resolved,
            conflicts: deduped,
        }
    }
}

#[test]
fn test_inference() {
    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

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
            let mut results = InferenceResult::default();
            let db = crate::db::SimlinDb::default();
            let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
            let _project = crate::project::Project::from_salsa(
                project_datamodel.clone(),
                &db,
                sync.project,
                |models, units_ctx, model| {
                    results = infer(models, units_ctx, model);
                },
            );
            assert!(
                results.conflicts.is_empty(),
                "expected no conflicts for a fully-inferrable model, got: {:?}",
                results.conflicts
            );
            let results = results.resolved;
            for (ident, expected_units) in expected.iter() {
                let expected_units: UnitMap =
                    crate::units::parse_units(&units_ctx, Some(expected_units))
                        .unwrap()
                        .unwrap();
                if let Some(computed_units) = results.get(&*canonicalize(ident)) {
                    assert_eq!(expected_units, *computed_units);
                } else {
                    panic!("inference results don't contain variable '{ident}'");
                }
            }
        }
    }
}

/// A variable can have declared units but no parsed equation -- e.g. in the
/// editor when units are entered before the equation is written (the same
/// half-built state that powers unit fill-in). Such a variable must still
/// contribute its declared units to inference so that dependents can be
/// inferred. Regression test for the `None => continue` gap in
/// `gen_all_constraints`, which skipped the `var.units()` constraint entirely
/// for equation-less variables.
#[test]
fn test_declared_units_without_equation_propagate() {
    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    // `base` has declared units but an empty equation (so `ast()` is None).
    // `derived = base` has no declared units; inference should propagate
    // `widget` to it through the reference.
    let vars = vec![
        x_aux("base", "", Some("widget")),
        x_aux("derived", "base", None),
    ];
    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let mut results = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );
    let results = results.resolved;

    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        results.get(&*canonicalize("derived")),
        Some(&widget),
        "derived should inherit base's declared units via inference even though base has no equation"
    );
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
            let mut results = InferenceResult::default();
            let db = crate::db::SimlinDb::default();
            let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
            let _project = crate::project::Project::from_salsa(
                project_datamodel.clone(),
                &db,
                sync.project,
                |models, units_ctx, model| {
                    results = infer(models, units_ctx, model);
                },
            );
            assert!(
                !results.conflicts.is_empty(),
                "expected a dimensional conflict to be reported"
            );
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

    let mut results = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    // Verify that at least one reported conflict carries source + location info.
    assert!(
        !results.conflicts.is_empty(),
        "expected at least one conflict to be reported"
    );
    let found = results.conflicts.iter().any(|conflict| {
        if let UnitError::InferenceError {
            code,
            sources,
            details,
        } = conflict
        {
            *code == ErrorCode::UnitMismatch
                // at least one source references "bad" (the mismatched variable)
                && sources.iter().any(|(var, _)| var == "bad")
                // at least one source carries an equation location (some sources,
                // e.g. bare unit declarations, legitimately have None)
                && sources.iter().any(|(_, loc)| loc.is_some())
                && details.is_some()
        } else {
            false
        }
    });
    assert!(
        found,
        "expected an InferenceError mentioning 'bad' with a location and details, got: {:?}",
        results.conflicts
    );
}

/// Inference is partial: a dimensional conflict in one part of a model must
/// not discard the units inference resolved elsewhere, and every independent
/// conflict must be reported -- not just whichever one happens to be found
/// first (see GH #614).
#[test]
fn test_inference_partial_results_survive_conflict() {
    let sim_specs = sim_specs_with_units("year");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    let vars = vec![
        // A clean, independently-inferrable chain.
        x_aux("clean_src", "6", Some("widget")),
        x_aux("clean_dst", "clean_src", None), // inferred: widget
        // Conflict A: `a` is forced to two incompatible units.
        x_aux("a", "10", None),
        x_aux("ay", "a", Some("meter")),
        x_aux("az", "a", Some("second")),
        // Conflict B: independent of A -- `b` forced to two incompatible units.
        x_aux("b", "10", None),
        x_aux("bp", "b", Some("gram")),
        x_aux("bq", "b", Some("ampere")),
    ];
    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let mut result = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            result = infer(models, units_ctx, model);
        },
    );

    // The clean chain is resolved despite conflicts elsewhere in the model.
    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.resolved.get(&*canonicalize("clean_dst")),
        Some(&widget),
        "an unrelated dimensional conflict must not discard resolved units"
    );

    // Both independent conflicts are reported, not just the first one found.
    assert!(
        result.conflicts.len() >= 2,
        "expected at least two independent conflicts, got {}: {:?}",
        result.conflicts.len(),
        result.conflicts
    );
}

/// A Vensim macro can annotate a body variable's units with the macro's formal
/// parameter *names* (e.g. `~ xfrom` inside C-LEARN's `RAMP FROM TO`) -- a
/// symbolic, polymorphic unit, NOT a concrete base unit. Inference must treat
/// such a macro-body unit as polymorphic; otherwise the parameter name leaks as
/// a literal unit into every instantiation and conflicts with the real argument
/// units (the source of C-LEARN's `xfrom`/`xto` unit-error storm once #614 stops
/// the all-or-nothing behavior from masking it).
#[test]
fn test_macro_body_units_naming_parameters_are_polymorphic() {
    let sim_specs = sim_specs_with_units("year");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    // A macro `scaleit(amount)` whose output is the parameter `amount`, with the
    // output's units declared as the parameter name `amount` (the polymorphic
    // idiom). Instantiated with a `widget` argument, the result must infer to
    // `widget`, not conflict against a bogus `amount` base unit.
    let macro_model = crate::datamodel::Model {
        name: "scaleit".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("scaleit", "amount", Some("amount")),
            x_aux("amount", "0", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(crate::datamodel::MacroSpec {
            parameters: vec!["amount".to_string()],
            primary_output: "scaleit".to_string(),
            additional_outputs: vec![],
        }),
    };
    let root = x_model(
        "main",
        vec![
            x_aux("source", "10", Some("widget")),
            x_aux("scaled", "scaleit(source)", None),
        ],
    );
    let project_datamodel = x_project(sim_specs.clone(), &[root, macro_model]);

    let mut result = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            if model.name.as_str() == "main" {
                result = infer(models, units_ctx, model);
            }
        },
    );

    assert!(
        result.conflicts.is_empty(),
        "a macro body unit naming a parameter must be polymorphic, not leak as a literal unit; got conflicts: {:?}",
        result.conflicts
    );
    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.resolved.get(&*canonicalize("scaled")),
        Some(&widget),
        "the macro result should infer to the argument's units"
    );
}

pub(crate) fn infer(
    models: &HashMap<Ident<Canonical>, &ModelStage1>,
    units_ctx: &Context,
    model: &ModelStage1,
) -> InferenceResult {
    let time_units_name =
        canonicalize(units_ctx.sim_specs.time_units.as_deref().unwrap_or("time")).into_owned();
    // Resolve through the alias map so the synthetic `time` variable's units
    // match what `units_check::check` uses (see the same resolution in
    // `gen_all_constraints`).
    let time_units: UnitMap = units_ctx
        .lookup(&time_units_name)
        .cloned()
        .unwrap_or_else(|| [(time_units_name.clone(), 1)].iter().cloned().collect());

    let units = UnitInferer {
        ctx: units_ctx,
        models,
        time: Variable::Var {
            ident: Ident::new("time"),
            ast: None,
            init_ast: None,
            eqn: None,
            units: Some(time_units),
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

/// PREVIOUS(x) desugars to PREVIOUS(x, 0). The inferred units should
/// come from x, not the fallback 0 constant.
#[test]
fn test_previous_infers_units_from_lagged_arg() {
    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    // position has explicit units "widget". prev_pos has no declared
    // units; inference should propagate "widget" from position
    // through PREVIOUS(position, 0).
    let test_case: &[(crate::datamodel::Variable, &str)] = &[
        (x_aux("position", "10", Some("widget")), "widget"),
        (x_aux("prev_pos", "PREVIOUS(position)", None), "widget"),
    ];

    let expected = test_case
        .iter()
        .map(|(var, units)| (var.get_ident(), *units))
        .collect::<HashMap<&str, &str>>();
    let vars = test_case
        .iter()
        .map(|(var, _)| var)
        .cloned()
        .collect::<Vec<_>>();
    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    for _ in 0..64 {
        let mut results = InferenceResult::default();
        let db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
        let _project = crate::project::Project::from_salsa(
            project_datamodel.clone(),
            &db,
            sync.project,
            |models, units_ctx, model| {
                results = infer(models, units_ctx, model);
            },
        );
        let results = results.resolved;
        for (ident, expected_units) in expected.iter() {
            let expected_units: UnitMap =
                crate::units::parse_units(&units_ctx, Some(expected_units))
                    .unwrap()
                    .unwrap();
            if let Some(computed_units) = results.get(&*canonicalize(ident)) {
                assert_eq!(
                    expected_units, *computed_units,
                    "variable '{ident}': expected {expected_units:?} but got {computed_units:?}"
                );
            } else {
                panic!("inference results don't contain variable '{ident}'");
            }
        }
    }
}

/// PREVIOUS(x, fallback) should propagate units from x to fallback
/// during inference, so a fallback with incompatible declared units
/// is detected as a mismatch.
#[test]
fn test_previous_constrains_fallback_units() {
    let sim_specs = sim_specs_with_units("parsec");

    // "seed" has wrong units ("wallop" vs "widget"). PREVIOUS(position, seed)
    // should fail inference because the fallback is constrained to match
    // the lagged argument.
    let test_case: &[(crate::datamodel::Variable, &str)] = &[
        (x_aux("position", "10", Some("widget")), "widget"),
        (x_aux("seed", "0", Some("wallop")), "wallop"),
        (
            x_aux("prev_pos", "PREVIOUS(position, seed)", None),
            "widget",
        ),
    ];

    let vars = test_case
        .iter()
        .map(|(var, _unit)| var)
        .cloned()
        .collect::<Vec<_>>();
    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs, &[model]);

    for _ in 0..64 {
        let mut results = InferenceResult::default();
        let db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
        let _project = crate::project::Project::from_salsa(
            project_datamodel.clone(),
            &db,
            sync.project,
            |models, units_ctx, model| {
                results = infer(models, units_ctx, model);
            },
        );
        assert!(
            !results.conflicts.is_empty(),
            "PREVIOUS(widget, wallop) should fail unit inference"
        );
    }
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
    assert_eq!(&*map_key, "output", "Map key should be lowercase");

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

    let mut results = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    // The inference should report a conflict because x and y have inconsistent
    // unit declarations.
    assert!(
        !results.conflicts.is_empty(),
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
    let result = find_constraint_mismatches(&constraints);
    assert!(!result.is_empty(), "Should detect direct concrete mismatch");

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
    let result = find_constraint_mismatches(&constraints);
    assert!(
        !result.is_empty(),
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
    let result = find_constraint_mismatches(&constraints);
    // The ratio of these two would be @a/@b * @d/@c which still has metavariables
    assert!(
        result.is_empty(),
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
    let result = find_constraint_mismatches(&constraints);
    assert!(
        result.is_empty(),
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
    // RANK returns a dimensionless position/index, NOT the units of the
    // ranked array. So `ranking` below must infer to dimensionless (an empty
    // unit map) even though the ranked `values` is in dollars.
    let sim_specs = sim_specs_with_units("year");

    let vars = vec![
        x_aux("values", "10", Some("dollar")),
        x_aux("ranking", "RANK(values, 1)", None),
    ];

    let model = x_model("main", vars);
    let project_datamodel = x_project(sim_specs.clone(), &[model]);

    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;
    let mut results = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    let results = results.resolved;

    // `values` keeps its declared dollar units...
    let dollar: UnitMap = crate::units::parse_units(&units_ctx, Some("dollar"))
        .unwrap()
        .unwrap();
    assert_eq!(results.get(&*canonicalize("values")), Some(&dollar));

    // ...but `ranking` is dimensionless, not dollars.
    assert_eq!(
        results.get(&*canonicalize("ranking")),
        Some(&UnitMap::new()),
        "RANK result should be dimensionless, not inherit the ranked array's units"
    );
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

    let mut results = InferenceResult::default();
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, &project_datamodel);
    let _project = crate::project::Project::from_salsa(
        project_datamodel.clone(),
        &db,
        sync.project,
        |models, units_ctx, model| {
            results = infer(models, units_ctx, model);
        },
    );

    // This should report a conflict because 'a' can't be both meters and seconds.
    assert!(
        !results.conflicts.is_empty(),
        "Should detect conflict when same variable has different unit constraints"
    );

    // Verify we get an InferenceError with the right code and source info.
    let found = results.conflicts.iter().any(|conflict| {
        matches!(
            conflict,
            UnitError::InferenceError { code, sources, .. }
                if *code == ErrorCode::UnitMismatch && !sources.is_empty()
        )
    });
    assert!(
        found,
        "expected an InferenceError with source information, got: {:?}",
        results.conflicts
    );
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

    let mut set = ConstraintSet::from_vec(vec![test_constraint(constraint)]);
    set.substitute(var, &units, &sources);
    let result = set.into_vec();

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

    let mut set = ConstraintSet::from_vec(vec![test_constraint(constraint)]);
    set.substitute(var, &units, &sources);
    let result = set.into_vec();

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
