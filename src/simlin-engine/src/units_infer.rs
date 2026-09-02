// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, BinaryOp, Expr2};
use crate::builtins::{BuiltinFn, Loc};
use crate::common::{Canonical, ErrorCode, Ident, UnitError, canonicalize};
use crate::datamodel::UnitMap;
#[cfg(test)]
use crate::testutils::{sim_specs_with_units, x_aux, x_flow, x_model, x_project, x_stock};
use crate::units::{Context, UnitOp, Units, combine};
use crate::units_check::UnitModel;
use crate::variable::{VarKind, Variable};

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
    models: &'a HashMap<Ident<Canonical>, UnitModel>,
    // units for module inputs
    time: Variable,
}

/// The models whose bodies are being walked on the CURRENT instantiation path,
/// as a cons list threaded down the recursion in `gen_all_constraints`.
///
/// A module graph is a graph, not a tree: it may contain cycles (`a`
/// instantiates `b`, `b` instantiates `a`), which without a guard make the walk
/// diverge and overflow the stack -- an immediate process abort under
/// `panic=abort`, not a catchable panic. The guard must distinguish a back edge
/// from a legal repeated visit, so it tracks the models on the current PATH
/// rather than every model visited anywhere: in a diamond (`a` instantiates `b`
/// and `c`, both of which instantiate `d`) `d` is genuinely instantiated twice,
/// under two different prefixes, and both instantiations need their own
/// constraints. A visited-anywhere set would silently drop one of them.
///
/// A cons list rather than a `Vec` push/pop pair because the entry's lifetime
/// IS the stack frame's: there is no pop to forget and no way to leave a stale
/// model on the path. Membership is linear in the path's LENGTH, which is at
/// most one entry per model in the project.
///
/// That depth bound is not a bound on total work, and this is the wrong place
/// to read one: the number of instantiation PREFIXES walked is not bounded by
/// the model count. `k` models each instantiating the next twice reaches the
/// last one `2^k` times, and every one of those walks is legal -- they are
/// distinct instantiations that must each be constrained (see
/// `diamond_module_graph_is_walked_on_every_path` for the k=2 case). The
/// exponential is inherent to per-instantiation constraint generation, not
/// introduced by this guard; the guard only makes the walk FINITE.
struct InstantiationPath<'a> {
    model: &'a Ident<Canonical>,
    parent: Option<&'a InstantiationPath<'a>>,
}

impl InstantiationPath<'_> {
    /// The path consisting of just `model` -- the root of a walk.
    fn root(model: &Ident<Canonical>) -> InstantiationPath<'_> {
        InstantiationPath {
            model,
            parent: None,
        }
    }

    /// Is `model` already being walked further up this path? Such an edge
    /// closes a cycle and must not be followed.
    fn contains(&self, model: &Ident<Canonical>) -> bool {
        let mut node = Some(self);
        while let Some(entry) = node {
            if entry.model == model {
                return true;
            }
            node = entry.parent;
        }
        false
    }
}

fn single_fv(units: &UnitMap) -> Option<&str> {
    let mut result = None;
    for (unit, exp) in units.map.iter() {
        if unit.starts_with('@') {
            // Only consider metavariables with exponent ±1.
            // If |exponent| > 1, we can't solve for this variable because it would
            // require fractional exponents (e.g., @x^2 = meters => @x = meters^(1/2)).
            // unsigned_abs: |i32::MIN| overflows, and a saturated exponent
            // (reachable from a literal like `(y*y)^-2^30`) must not panic.
            if exp.unsigned_abs() != 1 {
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
            exponent.unsigned_abs() == 1,
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

            // saturating/unsigned: |i32::MIN| overflows, and an absurd
            // saturated exponent must scale absurdly rather than panic.
            let scaled_units = if exponent.unsigned_abs() == 1 {
                units.clone()
            } else {
                units.clone().exp(exponent.saturating_abs())
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

    fn into_vec(self) -> Vec<LocatedConstraint> {
        self.constraints
    }
}

/// Lower a macro body's declared units, replacing each unit identifier that
/// names a formal parameter with that parameter's metavariable
/// (`@{prefix}{param}`). A genuine base unit -- any name that is not a
/// parameter, e.g. `dmnl` -- is kept verbatim. This is what makes a macro's
/// unit signature polymorphic: a `~ xfrom` declaration becomes a constraint
/// tying the body variable to the metavariable the module's input binding
/// already ties to the actual argument, so it resolves to the argument's units
/// at this instantiation instead of leaking a bogus literal base unit `xfrom`
/// (GH #619). `prefix` is the instantiation prefix of the macro body (e.g.
/// `$⁚ramped⁚0⁚ramp_from_to·`), so the metavariable is per-instantiation.
fn lower_macro_unit_to_metavars(
    units: &UnitMap,
    params: &[Ident<Canonical>],
    prefix: &str,
) -> UnitMap {
    let mut lowered = UnitMap::new();
    for (name, exp) in units.map.iter() {
        let canonical = canonicalize(name);
        if params.iter().any(|p| p.as_str() == &*canonical) {
            lowered.map.insert(format!("@{prefix}{canonical}"), *exp);
        } else {
            lowered.map.insert(name.clone(), *exp);
        }
    }
    lowered
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

/// Parse a (possibly synthetic) inference source-variable name into the owning
/// user variable and the macro/stdlib function it expands. A module-function
/// call is rewritten by `builtins_visitor::expand_module_function` into a
/// synthetic module named `$⁚{var}⁚{n}⁚{func}` (optionally `⁚{subscript}` in
/// A2A context), and a body-variable reference appends `·{body}` path
/// segments. So `$⁚ramped⁚0⁚ramp_from_to·slope` means: variable `ramped`'s
/// equation calls the macro/function `ramp_from_to`. Returns `(var, func)` for
/// such a synthetic name, or `None` for an ordinary user variable. The two
/// separators are U+205A (`⁚`, the synthetic-name field separator) and U+00B7
/// (`·`, the compile-time module-path separator).
fn synthetic_owner_and_func(name: &str) -> Option<(String, String)> {
    for segment in name.split('\u{b7}') {
        if let Some(rest) = segment.strip_prefix("$\u{205A}") {
            let parts: Vec<&str> = rest.split('\u{205A}').collect();
            // parts == [var, n, func, (subscript?)]
            if parts.len() >= 3 {
                return Some((parts[0].to_string(), parts[2].to_string()));
            }
        }
    }
    None
}

/// Rewrite an inference conflict that involves a synthetic module/macro
/// instantiation into a clear, user-facing diagnostic. The synthetic
/// instantiation variable names (`$⁚…·…`) and the `@`-metavariable constraint
/// text are meaningless to a modeler, so we collapse the synthetic sources to
/// the owning user variable and rebuild the message to name the function and
/// the variable using it (GH #619). A conflict with no synthetic source is an
/// ordinary user-level diagnostic and is left untouched. Distinct raw
/// conflicts that clarify to the same message dedupe in the caller, so a single
/// inconsistent macro yields one clear warning rather than one per internal
/// contradiction.
fn clarify_macro_conflict(error: UnitError) -> UnitError {
    let UnitError::InferenceError {
        code,
        sources,
        details,
    } = error
    else {
        return error;
    };

    let owner_func = sources
        .iter()
        .find_map(|(var, _)| synthetic_owner_and_func(var));
    let Some((owner, func)) = owner_func else {
        // No synthetic instantiation involved: already a user-level diagnostic.
        return UnitError::InferenceError {
            code,
            sources,
            details,
        };
    };

    // Collapse synthetic sources to the owning user variable, keeping any
    // genuine (non-synthetic) user variables that were also involved so the
    // modeler still sees which of their own variables participated.
    let mut clean_sources: Vec<(String, Option<Loc>)> = vec![(owner.clone(), None)];
    for (var, loc) in &sources {
        if synthetic_owner_and_func(var).is_none()
            && !clean_sources.iter().any(|(existing, _)| existing == var)
        {
            clean_sources.push((var.clone(), *loc));
        }
    }

    UnitError::InferenceError {
        code,
        sources: clean_sources,
        details: Some(format!(
            "units in '{func}' (used by variable '{owner}') are inconsistent"
        )),
    }
}

impl UnitInferer<'_> {
    /// The units of a square root whose argument map has no representable
    /// root (it carries metavariables, or odd concrete exponents): a FRESH
    /// metavariable `R` plus the residual constraint `R^2 == square`.
    ///
    /// `single_fv` refuses to solve for a metavariable with |exp| != 1, so
    /// the constraint itself never mis-binds `R`; but once `R` and the
    /// square's metavariables are bound through OTHER constraints,
    /// `substitute` scales them in and a wrong binding surfaces as a
    /// concrete contradiction in `find_constraint_mismatches`. Returning a
    /// free `Constant` here instead severed the only relationship between a
    /// SQRT result and its source. The metavariable name embeds the current
    /// variable and source location so distinct call sites stay distinct;
    /// the reserved `$⁚` prefix keeps it disjoint from real identifiers.
    fn sqrt_result_metavar(
        &self,
        prefix: &str,
        current_var: &str,
        loc: Loc,
        square: UnitMap,
        constraints: &mut Vec<LocatedConstraint>,
    ) -> Units {
        let fresh_name = format!(
            "@{prefix}$\u{205a}sqrt\u{205a}{current_var}\u{205a}{}",
            loc.start
        );
        let fresh: UnitMap = [(fresh_name, 1)].iter().cloned().collect();
        constraints.push(LocatedConstraint::new(
            combine(UnitOp::Div, fresh.clone().exp(2), square),
            current_var,
            Some(loc),
        ));
        Units::Explicit(fresh)
    }

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
            // Per-variant semantics: each builtin's unit rule -- which argument
            // carries the result's units, which constraints its arguments share.
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
                | BuiltinFn::Round(a)
                | BuiltinFn::Sign(a)
                | BuiltinFn::Sin(a)
                | BuiltinFn::Tan(a)
                | BuiltinFn::Size(a)
                | BuiltinFn::Stddev(a)
                | BuiltinFn::Sum(a) => self.gen_constraints(a, prefix, current_var, constraints),
                // SQRT halves unit exponents (mirroring `units_check`). The
                // argument's symbolic map usually carries metavariables with
                // odd exponents (`@x^1`), which have no representable root in
                // integer-exponent maps; the result is then a FRESH
                // metavariable R with the residual constraint R^2 == arg
                // (see `sqrt_result_metavar`) rather than a free Constant --
                // degrading severed the only relationship between a SQRT
                // result and its source, so a consumer could bind the
                // (undeclared) result to arbitrary units with no complaint.
                BuiltinFn::Sqrt(a) => {
                    match self.gen_constraints(a, prefix, current_var, constraints) {
                        Units::Constant => Units::Constant,
                        Units::Explicit(units) => match crate::units::try_sqrt(&units) {
                            Some(root) => Units::Explicit(root),
                            None => self.sqrt_result_metavar(
                                prefix,
                                current_var,
                                expr.get_loc(),
                                units,
                                constraints,
                            ),
                        },
                    }
                }
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
                            && let Units::Explicit(ref runits) = b_units
                        {
                            let loc = a.get_loc().union(&b.get_loc());
                            constraints.push(LocatedConstraint::new(
                                combine(UnitOp::Div, lunits.clone(), runits.clone()),
                                current_var,
                                Some(loc),
                            ));
                        }
                        // A literal argument is unit-polymorphic: `MAX(0, x)`
                        // has x's units (mirroring `units_check`).
                        return a_units.first_explicit(b_units);
                    }
                    a_units
                }
                BuiltinFn::Quantum(a, _) => {
                    self.gen_constraints(a, prefix, current_var, constraints)
                }
                // SSHAPE(x, bottom, top): bottom and top carry the result
                // units and must agree; x is visited for its own constraints
                // but its units are unconstrained (mirroring `units_check`).
                BuiltinFn::Sshape(x, bottom, top) => {
                    self.gen_constraints(x, prefix, current_var, constraints);
                    let bottom_units =
                        self.gen_constraints(bottom, prefix, current_var, constraints);
                    let top_units = self.gen_constraints(top, prefix, current_var, constraints);
                    if let Units::Explicit(ref b_map) = bottom_units
                        && let Units::Explicit(ref t_map) = top_units
                    {
                        let loc = bottom.get_loc().union(&top.get_loc());
                        constraints.push(LocatedConstraint::new(
                            combine(UnitOp::Div, b_map.clone(), t_map.clone()),
                            current_var,
                            Some(loc),
                        ));
                    }
                    bottom_units.first_explicit(top_units)
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

                    // The optional fallback, if specified, must match the
                    // units of a/b -- and when the quotient is a bare
                    // literal ratio, the explicit fallback's units carry
                    // (the MAX/MIN literal-polymorphism rule, mirroring
                    // `units_check`).
                    if let Some(c) = c {
                        let c_units = self.gen_constraints(c, prefix, current_var, constraints);
                        if let Units::Explicit(ref result_units) = units
                            && let Units::Explicit(ref c_map) = c_units
                        {
                            constraints.push(LocatedConstraint::new(
                                combine(UnitOp::Div, c_map.clone(), result_units.clone()),
                                current_var,
                                Some(c.get_loc()),
                            ));
                        }
                        return units.first_explicit(c_units);
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
                        && let Units::Explicit(ref b_map) = b_units
                    {
                        let loc = a.get_loc().union(&b.get_loc());
                        constraints.push(LocatedConstraint::new(
                            combine(UnitOp::Div, a_map.clone(), b_map.clone()),
                            current_var,
                            Some(loc),
                        ));
                    }
                    // A literal in either position is unit-polymorphic
                    // (mirroring `units_check`).
                    a_units.first_explicit(b_units)
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
                    // `x^n` routes through the shared `units::power_units`
                    // (see units_check's `^` arm): integer-literal exponents
                    // are valid symbolically too (`@x^1` becomes `@x^n`); a
                    // half-integer exponent whose doubled map has no
                    // representable root gets the same fresh-metavariable
                    // residual constraint as SQRT (`x^n = sqrt(x^2n)`); only
                    // a non-half-integer or non-literal exponent degrades to
                    // Constant (its units are genuinely undetermined).
                    BinaryOp::Exp => match (lunits, crate::units_check::literal_exponent(r)) {
                        (Units::Constant, _) => Units::Constant,
                        (Units::Explicit(base), Some(n)) => {
                            match crate::units::power_units(&base, n) {
                                crate::units::PowerUnits::Explicit(units) => Units::Explicit(units),
                                crate::units::PowerUnits::NonSquareRoot => {
                                    let doubled = base.exp((2.0 * n) as i32);
                                    self.sqrt_result_metavar(
                                        prefix,
                                        current_var,
                                        expr.get_loc(),
                                        doubled,
                                        constraints,
                                    )
                                }
                                crate::units::PowerUnits::Polymorphic => Units::Constant,
                            }
                        }
                        (Units::Explicit(_), None) => Units::Constant,
                    },
                    BinaryOp::Mod => lunits,
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
                    && let Units::Explicit(ref runits) = runits
                {
                    let loc = l.get_loc().union(&r.get_loc());
                    constraints.push(LocatedConstraint::new(
                        combine(UnitOp::Div, lunits.clone(), runits.clone()),
                        current_var,
                        Some(loc),
                    ));
                }

                // A literal branch (`IF c THEN 0 ELSE flow`) is
                // unit-polymorphic (mirroring `units_check`).
                lunits.first_explicit(runits)
            }
        }
    }

    /// Generate every constraint for `model` at instantiation `prefix`,
    /// recursing through its module instantiations.
    ///
    /// `active` is the set of models already being walked on the path that led
    /// here; a module targeting one of them closes a cycle and is skipped. See
    /// [`InstantiationPath`].
    fn gen_all_constraints(
        &self,
        model: &UnitModel,
        prefix: &str,
        constraints: &mut Vec<LocatedConstraint>,
        active: &InstantiationPath<'_>,
    ) {
        let time_units_name =
            canonicalize(self.ctx.sim_specs.time_units.as_deref().unwrap_or("time")).into_owned();
        // Resolve the time unit exactly the way a variable's `<units>` string
        // is resolved (`Context::resolve_name`: aliases, the dimensionless
        // spellings, unknown-name fallback) so inference uses the same
        // canonical time unit as `units_check::check`. Without the alias
        // step, a model declaring units with an aliased time name (e.g. `yr`)
        // while `time_units` names the primary (`year`) produced a spurious
        // `year` vs `yr` mismatch on every stock/flow constraint; without the
        // dimensionless step, `time_units="Unitless"` minted a fictitious
        // `{unitless: 1}` base unit.
        let time_units: UnitMap = self.ctx.resolve_name(&time_units_name);

        // Deterministic constraint order: `variables` is a HashMap, and its
        // per-instance iteration order used to reach TWO observables --
        // which constraint anchors each signature group in
        // `find_constraint_mismatches` (hence which VARIABLE a conflict
        // names, and how many pairs a k-way conflict yields), and the
        // solver's binding choices. Sorting is the GH #595/#633 recipe:
        // an unordered collection must not reach an observable (GH #999).
        let mut sorted_vars: Vec<(&Ident<Canonical>, &Variable)> = model
            .variables
            .iter()
            .map(|(id, var)| (id, var.as_ref()))
            .collect();
        sorted_vars.sort_unstable_by_key(|(id, _)| id.as_str());
        for (id, var) in sorted_vars {
            let current_var = format!("{prefix}{id}");

            if let VarKind::Stock {
                inflows, outflows, ..
            } = &var.kind
            {
                let stock_ident = &var.ident;
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
            } else if let VarKind::Module { model_name, inputs } = &var.kind {
                // Two reasons to decline a module's submodel constraints, both
                // of which DEGRADE rather than fail -- the variable still falls
                // through to its declared-units constraint below, and inference
                // is partial-result, so the rest of the model resolves.
                //
                // 1. The target model is not in the map: a freshly drawn module
                //    in the editor carries an empty model_name until its target
                //    is assigned (and a dangling reference can outlive a deleted
                //    model). Skip rather than panic on the missing key.
                // 2. The target model is already being walked on this path, so
                //    this edge closes a module cycle. Following it diverges;
                //    see `InstantiationPath`. The cycle itself gets no unit
                //    diagnostic: `project_module_graph` already reports it as a
                //    `CircularDependency`, and a second, unit-flavoured message
                //    for the same structural fact would be noise.
                //
                // The input constraints go with the body, and that has a KNOWN
                // COST -- it is a conservative choice, not a free one. The
                // callee-side metavariable `@{subprefix}{dst}` is NOT
                // necessarily dead just because we skip the body: a parent
                // equation reading `{ident}·{var}` emits that same
                // metavariable (`gen_constraints`' `Expr2::Var` arm renders an
                // ident verbatim under the active prefix), so the binding we
                // decline to make can contradict one the parent already made.
                // Keeping the constraint would therefore report a genuine
                // cross-module dimensional conflict that we now stay silent
                // about. We accept that: the project is already rejected as
                // `CircularDependency`, so a unit conflict on a model that
                // cannot compile is noise on top of the real error.
                // `back_edge_declines_a_real_cross_module_conflict` builds
                // exactly that shape and pins the silence, so this trade stays
                // visible instead of looking like an accident.
                if let Some(submodel) = self.models.get(model_name)
                    && !active.contains(model_name)
                {
                    let subprefix = format!("{prefix}{}·", var.ident);
                    for input in inputs {
                        let src_var = format!("{}{}", prefix, input.src);
                        let dst_var = format!("{}{}", subprefix, input.dst);
                        let src = format!("@{src_var}");
                        let dst = format!("@{dst_var}");
                        // src = dst === 1 = src/dst
                        let units: UnitMap = [(src, 1), (dst, -1)].iter().cloned().collect();
                        // Module input constraint: both caller and callee are sources
                        constraints.push(
                            LocatedConstraint::new(units, &src_var, None)
                                .with_source(&dst_var, None),
                        );
                    }
                    let subpath = InstantiationPath {
                        model: model_name,
                        parent: Some(active),
                    };
                    self.gen_all_constraints(submodel, &subprefix, constraints, &subpath);
                }
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

                        // Sorted (GH #999): element order decides which
                        // element's units the solver binds the array's
                        // metavariable to first, i.e. the RESOLVED units of a
                        // mixed-units array -- and with them every downstream
                        // consistency row's content.
                        let mut sorted_elements: Vec<_> = asts.iter().collect();
                        sorted_elements.sort_unstable_by_key(|(element, _)| element.as_str());
                        for (_element, expr) in sorted_elements {
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
                            // The lexicographically-first element's location.
                            // Deterministic-pick hygiene only: this arm is
                            // UNREACHABLE today (the `var_units` match above
                            // returns `Units::Constant` for `Ast::Arrayed`,
                            // so the enclosing `Units::Explicit` gate never
                            // admits one) -- kept ordered so a future
                            // reachability change cannot resurrect the
                            // GH #999 class here.
                            asts.iter()
                                .min_by_key(|(element, _)| element.as_str())
                                .map(|(_, e)| e)
                                .map_or_else(
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
            // Declared-units constraint. A macro is a polymorphic template:
            // its body variables' declared units may name the macro's formal
            // parameters (a Vensim idiom, e.g. `~ xfrom` inside RAMP FROM TO).
            // Treating such a name as a literal base unit would leak the
            // parameter name into every instantiation and collide with the
            // real argument units, so for a macro body we lower each
            // parameter-named unit to that parameter's metavariable -- the
            // declared units then resolve to the actual argument units at this
            // instantiation, AND a genuine declared-vs-equation inconsistency
            // is still caught (genuine base units like `dmnl` are kept and
            // checked). A non-macro model contributes its declared units
            // verbatim. (GH #619; the earlier GH #618 fix skipped macro
            // declarations entirely -- containing the leak but neither
            // resolving nor checking them.)
            if let Some(units) = var.units() {
                let mv: UnitMap = [(format!("@{prefix}{id}"), 1)].iter().cloned().collect();
                let declared = if model.is_macro {
                    lower_macro_unit_to_metavars(units, &model.macro_params, prefix)
                } else {
                    units.clone()
                };
                // User-defined unit declarations don't have equation locations
                constraints.push(LocatedConstraint::new(
                    combine(UnitOp::Div, mv, declared),
                    &current_var,
                    None,
                ));
            }
        }
    }

    /// Solve the constraint system by Gaussian-elimination-style substitution,
    /// returning the resolved metavariable units and the residual (still
    /// metavariable-bearing) constraints.
    ///
    /// A metavariable is solved at most once: `substitute` removes it from every
    /// remaining constraint (and the metavar index), so it can never reappear as
    /// a single free variable. `unify` therefore does not detect conflicts
    /// itself -- a genuine over-constraint (the same metavariable forced to two
    /// different units) reduces to a residual concrete contradiction (e.g.
    /// `meter == second`) that `find_constraint_mismatches` reports. The
    /// vacant-entry guard keeps the first binding -- and never propagates a
    /// rejected re-derivation -- if that solved-at-most-once invariant is ever
    /// weakened (GH #614).
    #[allow(clippy::type_complexity)]
    fn unify(
        &self,
        constraints: Vec<LocatedConstraint>,
    ) -> (HashMap<Ident<Canonical>, UnitMap>, Vec<LocatedConstraint>) {
        let mut resolved_fvs: HashMap<Ident<Canonical>, UnitMap> = HashMap::new();
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
                    let var_key = var.strip_prefix('@').unwrap();
                    let var_ident = Ident::<Canonical>::from_str_unchecked(var_key);
                    // Record the first (and, by the invariant above, only) binding
                    // for this metavariable and propagate it. The vacant-entry
                    // guard keeps the first binding -- and never substitutes a
                    // rejected re-derivation into the remaining constraints -- even
                    // if the solved-at-most-once invariant is ever weakened.
                    if let std::collections::hash_map::Entry::Vacant(e) =
                        resolved_fvs.entry(var_ident)
                    {
                        pending.substitute(&var, &units, &c.sources);
                        finalized.substitute(&var, &units, &c.sources);
                        e.insert(units);
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

        (resolved_fvs, finalized.into_vec())
    }

    fn infer(&self, model: &UnitModel) -> InferenceResult {
        let mut constraints = vec![];
        // The root model is seeded onto the path, so a model that instantiates
        // ITSELF is declined at the first edge rather than unrolled once.
        self.gen_all_constraints(
            model,
            "",
            &mut constraints,
            &InstantiationPath::root(&model.name),
        );

        let (resolved, leftover) = self.unify(constraints);

        // Leftover constraints that still contain metavariables just mean the
        // model is under-constrained (e.g. undeclared units) -- not an error.
        // Only a concrete contradiction among them is a real mismatch; this is
        // now the single source of conflicts, so we just dedup identical
        // findings (the same contradiction can be reported from more than one
        // residual constraint) to report each once.
        let mut conflicts: Vec<UnitError> = Vec::new();
        for conflict in find_constraint_mismatches(&leftover) {
            let conflict = clarify_macro_conflict(conflict);
            if !conflicts.contains(&conflict) {
                conflicts.push(conflict);
            }
        }

        InferenceResult {
            resolved,
            conflicts,
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
            let results = infer_project(&project_datamodel, "main");
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

    let results = infer_project(&project_datamodel, "main");
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
            let results = infer_project(&project_datamodel, "main");
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

    let results = infer_project(&project_datamodel, "main");

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

    let result = infer_project(&project_datamodel, "main");

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

    let result = infer_project(&project_datamodel, "main");

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

/// A macro-body variable whose units are pinned ONLY by a parameter-named
/// declaration (`~ amount`) -- not derivable from its own (constant) equation
/// -- must RESOLVE to the actual argument's units at each instantiation. This
/// is the "resolve, don't merely contain" half of GH #619: the GH #618
/// containment skipped the declaration entirely, so the body variable (and the
/// macro result that reads it) were left unresolved. Lowering the parameter
/// name to the parameter's metavariable ties it to the actual argument.
#[test]
fn test_macro_param_named_units_resolve_to_actual_arg() {
    let sim_specs = sim_specs_with_units("year");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    // macro carryunits(amount):
    //   carryunits = held            (no declared units; equation only)
    //   held       = 5     ~ amount  (CONSTANT equation -> units come ONLY
    //                                 from the parameter-named declaration)
    //   amount     = 0               (parameter placeholder)
    let macro_model = crate::datamodel::Model {
        name: "carryunits".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("carryunits", "held", None),
            x_aux("held", "5", Some("amount")),
            x_aux("amount", "0", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(crate::datamodel::MacroSpec {
            parameters: vec!["amount".to_string()],
            primary_output: "carryunits".to_string(),
            additional_outputs: vec![],
        }),
    };
    let root = x_model(
        "main",
        vec![
            x_aux("source", "10", Some("widget")),
            x_aux("scaled", "carryunits(source)", None),
        ],
    );
    let project_datamodel = x_project(sim_specs.clone(), &[root, macro_model]);

    let result = infer_project(&project_datamodel, "main");

    assert!(
        result.conflicts.is_empty(),
        "no conflict expected for a consistent macro; got: {:?}",
        result.conflicts
    );
    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.resolved.get(&*canonicalize("scaled")),
        Some(&widget),
        "a parameter-named macro-body unit must resolve to the actual argument's units"
    );
}

/// When a macro's declared units are internally inconsistent with its
/// equations, the conflict must be reported with a CLEAR, user-facing message
/// that names the macro and the variable using it -- NOT the synthetic
/// instantiation names (`$⁚...`) or raw `@` unit metavariables (GH #619). End
/// users are modelers, not software developers, so the diagnostic has to be
/// comprehensible.
#[test]
fn test_inconsistent_macro_reports_clear_user_facing_conflict() {
    let sim_specs = sim_specs_with_units("year");

    // macro squareit(amount):
    //   squareit = amount * amount  ~ amount  (equation units = amount^2, but
    //                                          the signature declares `amount`)
    //   amount   = 0
    let macro_model = crate::datamodel::Model {
        name: "squareit".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("squareit", "amount * amount", Some("amount")),
            x_aux("amount", "0", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(crate::datamodel::MacroSpec {
            parameters: vec!["amount".to_string()],
            primary_output: "squareit".to_string(),
            additional_outputs: vec![],
        }),
    };
    let root = x_model(
        "main",
        vec![
            x_aux("source", "10", Some("widget")),
            x_aux("result", "squareit(source)", None),
        ],
    );
    let project_datamodel = x_project(sim_specs.clone(), &[root, macro_model]);

    let result = infer_project(&project_datamodel, "main");

    assert!(
        !result.conflicts.is_empty(),
        "an internally-inconsistent macro must produce a conflict"
    );
    let rendered = result
        .conflicts
        .iter()
        .map(|c| format!("{c}"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains('\u{205A}'),
        "diagnostic must not expose synthetic instantiation names (got: {rendered})"
    );
    assert!(
        !rendered.contains('@'),
        "diagnostic must not expose raw unit metavariables (got: {rendered})"
    );
    assert!(
        rendered.contains("squareit"),
        "diagnostic should name the macro (got: {rendered})"
    );
    assert!(
        rendered.contains("result"),
        "diagnostic should name the variable that uses the macro (got: {rendered})"
    );
}

/// A macro that mixes parameter-named units (`~ xfrom`, `~ xfrom/tstart`) with
/// genuine base units (`~ dmnl`), instantiated with concrete arguments, must
/// infer cleanly with NO conflicts: the parameter names lower to the argument
/// metavariables (so there is no `xfrom`-as-a-literal-base-unit storm) while
/// genuine base units are still honored. This mirrors C-LEARN's RAMP FROM TO
/// and guards against re-introducing the leak GH #618 contained.
#[test]
fn test_macro_mixed_param_and_base_units_infer_cleanly() {
    let sim_specs = sim_specs_with_units("year");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    // macro rampish(xfrom, tstart):
    //   rampish = xfrom + slope * tstart  ~ xfrom         (output)
    //   slope   = xfrom / tstart          ~ xfrom/tstart  (two params)
    //   flag    = 1                       ~ dmnl          (genuine base unit)
    //   xfrom   = 0
    //   tstart  = 0
    let macro_model = crate::datamodel::Model {
        name: "rampish".to_string(),
        sim_specs: None,
        variables: vec![
            x_aux("rampish", "xfrom + slope * tstart", Some("xfrom")),
            x_aux("slope", "xfrom / tstart", Some("xfrom/tstart")),
            x_aux("flag", "1", Some("dmnl")),
            x_aux("xfrom", "0", None),
            x_aux("tstart", "0", None),
        ],
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: Some(crate::datamodel::MacroSpec {
            parameters: vec!["xfrom".to_string(), "tstart".to_string()],
            primary_output: "rampish".to_string(),
            additional_outputs: vec![],
        }),
    };
    // `dur` carries the time units (year); `amt` is an arbitrary base unit.
    let root = x_model(
        "main",
        vec![
            x_aux("amt", "10", Some("widget")),
            x_aux("dur", "3", Some("year")),
            x_aux("ramped", "rampish(amt, dur)", None),
        ],
    );
    let project_datamodel = x_project(sim_specs.clone(), &[root, macro_model]);

    let result = infer_project(&project_datamodel, "main");

    assert!(
        result.conflicts.is_empty(),
        "a macro mixing parameter-named and genuine base units must infer cleanly; got: {:?}",
        result.conflicts
    );
    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        result.resolved.get(&*canonicalize("ramped")),
        Some(&widget),
        "the macro result (declared ~ xfrom) should resolve to the first argument's units"
    );
}

pub(crate) fn infer(
    models: &HashMap<Ident<Canonical>, UnitModel>,
    units_ctx: &Context,
    model: &UnitModel,
) -> InferenceResult {
    let time_units_name =
        canonicalize(units_ctx.sim_specs.time_units.as_deref().unwrap_or("time")).into_owned();
    // Resolve through `Context::resolve_name` so the synthetic `time`
    // variable's units match what `units_check::check` uses (see the same
    // resolution in `gen_all_constraints`).
    let time_units: UnitMap = units_ctx.resolve_name(&time_units_name);

    let units = UnitInferer {
        ctx: units_ctx,
        models,
        time: Variable {
            ident: Ident::new("time"),
            units: Some(time_units),
            eqn: None,
            errors: vec![],
            unit_errors: vec![],
            kind: VarKind::Aux {
                ast: None,
                init_ast: None,
                tables: vec![],
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                element_scope: None,
            },
        },
    };

    units.infer(model)
}

// ── Module-graph shape: cycles degrade, diamonds are fully walked ────────────
//
// `gen_all_constraints` recurses through every module instantiation, so the
// shape of the module graph -- not just its contents -- decides whether it
// terminates and whether it generates the constraints a legal model needs.
// These tests drive `infer` the way `db::units::check_model_units` does, over
// each project model's `UnitModel` view of the per-variable lowering memos.

/// Run inference over `model_name` with every project model's unit view in
/// the map, the way `db::units::check_model_units` drives `infer` (its map is
/// the model's module-reachable scope, a subset of this one; inference only
/// ever looks up module targets, so the two agree).
#[cfg(test)]
fn infer_project(
    project_datamodel: &crate::datamodel::Project,
    model_name: &str,
) -> InferenceResult {
    let db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel(&db, project_datamodel);
    let models: HashMap<Ident<Canonical>, UnitModel> = sync
        .project
        .models(&db)
        .values()
        .map(|src_model| {
            let view = crate::db::units::unit_model(&db, *src_model, sync.project);
            (view.name.clone(), view)
        })
        .collect();
    let target = &models[&Ident::<Canonical>::new(model_name)];
    infer(
        &models,
        crate::db::project_units_context(&db, sync.project),
        target,
    )
}

/// The metavariable a prefixed variable resolved to, if any. Metavariable keys
/// carry the instantiation prefix (`b·d·out`), which is exactly what
/// distinguishes one instantiation of a model from another.
#[cfg(test)]
fn resolved_units<'a>(result: &'a InferenceResult, prefixed_ident: &str) -> Option<&'a UnitMap> {
    result
        .resolved
        .get(&Ident::<Canonical>::from_str_unchecked(prefixed_ident))
}

/// A DIAMOND module graph -- `main` instantiates `sub_b` and `sub_c`, each of
/// which instantiates `leaf` -- must generate `leaf`'s constraints under BOTH
/// instantiation prefixes.
///
/// This is the test that a careless cycle fix breaks. Guarding the recursion
/// with a set of models visited ANYWHERE (rather than models on the current
/// path) prunes `leaf`'s second visit, and a perfectly legal model silently
/// loses every constraint from one of its two instantiations. `leaf·out` is a
/// BODY variable of `leaf`, so its metavariable exists only if the body was
/// walked -- a guard that keeps generating module-input constraints while
/// skipping the body is caught here too.
#[test]
fn diamond_module_graph_is_walked_on_every_path() {
    use crate::testutils::x_module_named;

    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    let main = x_model(
        "main",
        vec![
            x_aux("driver", "1", Some("widget")),
            x_module_named("b", "sub_b", &[("driver", "b.input")], None),
            x_module_named("c", "sub_c", &[("driver", "c.input")], None),
        ],
    );
    let sub_b = x_model(
        "sub_b",
        vec![
            x_aux("input", "0", None),
            x_module_named("d", "leaf", &[("input", "d.input")], None),
        ],
    );
    let sub_c = x_model(
        "sub_c",
        vec![
            x_aux("input", "0", None),
            x_module_named("d", "leaf", &[("input", "d.input")], None),
        ],
    );
    let leaf = x_model(
        "leaf",
        vec![x_aux("input", "0", None), x_aux("out", "input", None)],
    );
    let project = x_project(sim_specs.clone(), &[main, sub_b, sub_c, leaf]);

    let result = infer_project(&project, "main");
    assert!(
        result.conflicts.is_empty(),
        "a consistent diamond must not conflict, got: {:?}",
        result.conflicts
    );

    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    for path in ["b\u{b7}d\u{b7}out", "c\u{b7}d\u{b7}out"] {
        assert_eq!(
            resolved_units(&result, path),
            Some(&widget),
            "'{path}' must be inferred: a model reached twice on different paths \
             is instantiated twice and must be constrained twice"
        );
    }
}

/// The user-visible half of the diamond: a unit conflict inside a model
/// instantiated twice must be reported for BOTH instantiations. The two
/// callers feed `leaf` incompatible units, so `leaf`'s `widget`-declared
/// output contradicts each of them differently -- and each contradiction names
/// its own instantiation path.
#[test]
fn diamond_module_graph_reports_a_conflict_on_each_path() {
    use crate::testutils::x_module_named;

    let sim_specs = sim_specs_with_units("parsec");

    let main = x_model(
        "main",
        vec![
            x_aux("driver_b", "1", Some("meter")),
            x_aux("driver_c", "1", Some("gram")),
            x_module_named("b", "sub_b", &[("driver_b", "b.input")], None),
            x_module_named("c", "sub_c", &[("driver_c", "c.input")], None),
        ],
    );
    let sub_b = x_model(
        "sub_b",
        vec![
            x_aux("input", "0", None),
            x_module_named("d", "leaf", &[("input", "d.input")], None),
        ],
    );
    let sub_c = x_model(
        "sub_c",
        vec![
            x_aux("input", "0", None),
            x_module_named("d", "leaf", &[("input", "d.input")], None),
        ],
    );
    let leaf = x_model(
        "leaf",
        vec![
            x_aux("input", "0", None),
            x_aux("out", "input", Some("widget")),
        ],
    );
    let project = x_project(sim_specs, &[main, sub_b, sub_c, leaf]);

    let result = infer_project(&project, "main");

    let mentions = |prefix: &str| {
        result.conflicts.iter().any(|conflict| match conflict {
            UnitError::InferenceError { sources, .. } => {
                sources.iter().any(|(var, _)| var.starts_with(prefix))
            }
            _ => false,
        })
    };
    for prefix in ["b\u{b7}", "c\u{b7}"] {
        assert!(
            mentions(prefix),
            "expected a conflict naming the '{prefix}' instantiation, got: {:?}",
            result.conflicts
        );
    }
}

/// A module CYCLE (`main` instantiates `sub`, `sub` instantiates `main`) must
/// degrade, not diverge. Before the recursion guard this overflowed the stack,
/// which is an immediate process abort rather than a panic -- fatal for a
/// `panic=abort` host like a WASM tab or an MCP server.
///
/// Degrading means declining to generate constraints across the BACK EDGE only:
/// everything on the acyclic part of the walk is still inferred, and no new
/// hard error appears (`project_module_graph` already reports the cycle
/// structurally, so a second unit-flavoured message for it would be noise).
#[test]
fn module_cycle_degrades_instead_of_diverging() {
    use crate::testutils::x_module_named;

    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    let main = x_model(
        "main",
        vec![
            x_aux("x", "1", Some("widget")),
            x_module_named("to_sub", "sub", &[("x", "to_sub.input")], None),
        ],
    );
    let sub = x_model(
        "sub",
        vec![
            x_aux("input", "0", None),
            x_aux("echo", "input", None),
            x_module_named("back", "main", &[("input", "back.x")], None),
        ],
    );
    let project = x_project(sim_specs, &[main, sub]);

    let result = infer_project(&project, "main");

    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved_units(&result, "to_sub\u{b7}echo"),
        Some(&widget),
        "the acyclic part of the walk must still be inferred"
    );
    assert!(
        result.conflicts.is_empty(),
        "a module cycle is a structural problem, not a dimensional one; \
         unit inference must not invent a conflict for it, got: {:?}",
        result.conflicts
    );
}

/// THE KNOWN COST of declining a back edge, pinned so it stays visible.
///
/// Declining the back edge takes the module-INPUT constraint with it, and that
/// constraint is not inert: the callee-side metavariable it would have bound
/// can be constrained by the PARENT's own equations, because a parent equation
/// reading `{module}·{var}` emits that very metavariable (`gen_constraints`'
/// `Expr2::Var` arm renders an ident verbatim under the active prefix). Here
/// `b`'s `peek = to_a.x ~ gram` pins `@to_b·to_a·x` to `gram` while `a` feeds
/// its `widget`-declared `x` into the same slot -- a genuine cross-module
/// dimensional contradiction that IS NOT REPORTED, because the edge carrying
/// the `widget` half is the one we decline.
///
/// That is a deliberate conservative choice, not an oversight: the project is
/// already rejected as `CircularDependency`, and resurrecting a unit conflict
/// on a model that cannot compile is noise. This test exists so the cost is a
/// documented degradation rather than a silent one -- if you re-enable the
/// input constraint across a back edge, this test goes red and tells you what
/// you are trading for.
#[test]
fn back_edge_declines_a_real_cross_module_conflict() {
    use crate::testutils::x_module_named;

    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    let model_a = x_model(
        "a",
        vec![
            x_aux("x", "1", Some("widget")),
            x_module_named("to_b", "b", &[("x", "to_b.input")], None),
        ],
    );
    let model_b = x_model(
        "b",
        vec![
            x_aux("input", "0", None),
            // Reads back across the cycle, and declares the units of what it
            // reads -- this is what makes the declined metavariable live.
            x_aux("peek", "to_a.x", Some("gram")),
            x_module_named("to_a", "a", &[("input", "to_a.x")], None),
        ],
    );
    let project = x_project(sim_specs, &[model_a, model_b]);

    let result = infer_project(&project, "a");

    // The metavariable the declined constraint would have bound is genuinely
    // constrained elsewhere. This is the fact that makes the omission a real
    // loss rather than a no-op.
    let gram: UnitMap = crate::units::parse_units(&units_ctx, Some("gram"))
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved_units(&result, "to_b\u{b7}to_a\u{b7}x"),
        Some(&gram),
        "the parent's own equation constrains the back edge's callee-side \
         metavariable, so declining the input constraint drops information"
    );

    // ...and the contradiction against `a`'s `widget` goes unreported.
    assert!(
        result.conflicts.is_empty(),
        "declining the back edge is a deliberate conservative choice whose \
         known cost is exactly this unreported conflict; got: {:?}",
        result.conflicts
    );
}

/// A model that instantiates ITSELF is the degenerate cycle. Its own body is
/// walked once, at the root, and the self-edge is declined.
#[test]
fn self_instantiating_model_degrades_instead_of_diverging() {
    use crate::testutils::x_module_named;

    let sim_specs = sim_specs_with_units("parsec");
    let units_ctx = Context::new_with_builtins(&[], &sim_specs).0;

    let main = x_model(
        "main",
        vec![
            x_aux("x", "1", Some("widget")),
            x_aux("y", "x", None),
            x_module_named("me", "main", &[("x", "me.x")], None),
        ],
    );
    let project = x_project(sim_specs, &[main]);

    let result = infer_project(&project, "main");

    let widget: UnitMap = crate::units::parse_units(&units_ctx, Some("widget"))
        .unwrap()
        .unwrap();
    assert_eq!(
        resolved_units(&result, "y"),
        Some(&widget),
        "the root model's own variables must still be inferred"
    );
    // The other half of the claim, and the one the `y` assertion does NOT
    // make: the self-edge is declined at the FIRST edge rather than unrolled
    // one level. Seeding the root path with anything that does not match the
    // model's own ident still resolves `y`, but would also produce `me·y`.
    assert_eq!(
        resolved_units(&result, "me\u{b7}y"),
        None,
        "the self-edge must be declined immediately, not unrolled once"
    );
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
        let results = infer_project(&project_datamodel, "main");
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
        let results = infer_project(&project_datamodel, "main");
        assert!(
            !results.conflicts.is_empty(),
            "PREVIOUS(widget, wallop) should fail unit inference"
        );
    }
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

    let results = infer_project(&project_datamodel, "main");

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
    let results = infer_project(&project_datamodel, "main");

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

    let results = infer_project(&project_datamodel, "main");

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
