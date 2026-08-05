// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::result::Result as StdResult;

use crate::ast::{BinaryOp, Expr0, UnaryOp};
use crate::common::{EquationError, EquationResult, ErrorCode, UnitError};
use crate::datamodel::{SimSpecs, Unit, UnitMap};
use crate::float::approx_eq;
use crate::lexer::LexerType;
use crate::{canonicalize, eqn_err};

/// Units is used to distinguish between explicit units (and explicit
/// dimensionless-ness) and dimensionless-ness that comes from computing
/// on constants.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(PartialEq, Eq, Clone)]
pub enum Units {
    Explicit(UnitMap),
    Constant,
}

impl Units {
    pub fn equals(&self, rhs: &Units) -> bool {
        match (self, rhs) {
            (Units::Constant, Units::Constant)
            | (Units::Explicit(_), Units::Constant)
            | (Units::Constant, Units::Explicit(_)) => true,
            (Units::Explicit(lhs), Units::Explicit(rhs)) => *lhs == *rhs,
        }
    }

    /// The units a unit-polymorphic two-argument builtin (MAX, MIN,
    /// PREVIOUS) produces: the first *explicit* units among its arguments.
    /// A bare numeric literal is unit-polymorphic -- `MAX(0, x)` has x's
    /// units -- so returning the first argument's verdict unconditionally
    /// (as this code used to) collapsed the result to `Constant` and made
    /// e.g. WORLD3's `MAX(0, land)/time` compute as `1/time`.
    pub fn first_explicit(self, rhs: Units) -> Units {
        match self {
            Units::Explicit(_) => self,
            Units::Constant => rhs,
        }
    }

    /// Convert to a UnitMap, treating Constant as dimensionless (empty map)
    pub fn to_unit_map(&self) -> UnitMap {
        match self {
            Units::Explicit(units) => units.clone(),
            Units::Constant => UnitMap::new(),
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum UnitOp {
    Mul,
    Div,
}

/// The square root of a unit map: halves every exponent. Returns `None`
/// when any exponent is odd -- such units are not a perfect square and have
/// no representable root (Vensim's fn_sqrt: "SQRT(units*units) --> units";
/// "if argument has units that are a perfect square the result will be the
/// square root of the units"). A dimensionless (empty) map roots to itself.
pub(crate) fn try_sqrt(units: &UnitMap) -> Option<UnitMap> {
    if units.map.values().any(|exp| exp % 2 != 0) {
        return None;
    }
    let mut result = units.clone();
    for exp in result.map.values_mut() {
        *exp /= 2;
    }
    Some(result)
}

/// The units of `base^n` for a literal exponent `n`.
///
/// The single decision shared by `units_check` and `units_infer`, so the two
/// cannot drift (their `^` arms previously copy-pasted this tree). Integer
/// exponents multiply the unit exponents (matching repeated multiplication);
/// half-integer exponents (0.5, -0.5, 1.5, ...) are `sqrt(base^2n)`, which
/// requires the doubled map to be a perfect square. Any other exponent
/// (including non-half-integer literals like `x^0.3`) has no representable
/// units and degrades to `Polymorphic` -- the caller treats it as a
/// unit-constant rather than (wrongly) keeping the base's units. What Vensim
/// does for such exponents is unverified, so no warning is emitted for them.
pub(crate) enum PowerUnits {
    Explicit(UnitMap),
    /// A half-integer exponent whose doubled base is not a perfect square:
    /// `units_check` reports this, `units_infer` degrades to Constant.
    NonSquareRoot,
    Polymorphic,
}

pub(crate) fn power_units(base: &UnitMap, n: f64) -> PowerUnits {
    let doubled = 2.0 * n;
    if n.fract() == 0.0 && n.abs() <= i32::MAX as f64 {
        PowerUnits::Explicit(base.clone().exp(n as i32))
    } else if doubled.fract() == 0.0 && doubled.abs() <= i32::MAX as f64 {
        match try_sqrt(&base.clone().exp(doubled as i32)) {
            Some(root) => PowerUnits::Explicit(root),
            None => PowerUnits::NonSquareRoot,
        }
    } else {
        PowerUnits::Polymorphic
    }
}

/// Join whitespace-separated identifier runs in a unit equation into single
/// underscore-joined unit names: `Degrees Fahrenheit/Minute` becomes
/// `Degrees_Fahrenheit/Minute`.
///
/// XMILE 3.3.6: unit names follow identifier rules and are "stored with
/// underscores (_) but generally presented to users with spaces" -- and
/// Stella writes the presentation form into `<units>` tags (e.g. the
/// canonical teacup model's `Degrees Fahrenheit`). Multiplication in a unit
/// equation is always an explicit `*`, so adjacent bare words can only be
/// one multi-word name. Whitespace next to an operator or parenthesis is
/// left untouched.
///
/// The transformation is BYTE-LENGTH-PRESERVING: each joining space/tab
/// becomes one `_`, and nothing is collapsed or trimmed. Parse-error byte
/// offsets computed against the joined string are rendered against the
/// original by `errors::format_snippet` and carried to GUI consumers as
/// `FormattedError.start_offset`/`end_offset`, so a length change here
/// would shift every underline after the first whitespace run. Only ASCII
/// space and tab participate in joins (1 byte -> 1 byte); other whitespace
/// is left verbatim for the lexer to reject at its true offset.
pub(crate) fn join_multiword_unit_names(eqn: &str) -> std::borrow::Cow<'_, str> {
    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_' || c == '$'
    }
    fn is_joinable_ws(c: char) -> bool {
        c == ' ' || c == '\t'
    }

    let chars: Vec<char> = eqn.chars().collect();
    let mut out = String::with_capacity(eqn.len());
    let mut changed = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if is_joinable_ws(c) {
            // A whitespace run joins two words only if BOTH neighbors are
            // word characters; each joining char maps 1:1 to `_`.
            let prev_is_word = out.chars().next_back().is_some_and(is_word_char);
            let mut j = i;
            while j < chars.len() && is_joinable_ws(chars[j]) {
                j += 1;
            }
            let next_is_word = j < chars.len() && is_word_char(chars[j]);
            for run_char in chars.iter().take(j).skip(i) {
                if prev_is_word && next_is_word {
                    out.push('_');
                    changed = true;
                } else {
                    out.push(*run_char);
                }
            }
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    if changed {
        std::borrow::Cow::Owned(out)
    } else {
        std::borrow::Cow::Borrowed(eqn)
    }
}

pub(crate) fn combine(op: UnitOp, l: UnitMap, r: UnitMap) -> UnitMap {
    let mut l = match op {
        UnitOp::Mul => l * r,
        UnitOp::Div => l / r,
    };

    if l.map.contains_key("dmnl") {
        l.map.remove("dmnl");
    }

    l
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default, PartialEq)]
pub struct Context {
    pub sim_specs: SimSpecs,
    aliases: HashMap<String, String>,
    units: HashMap<String, UnitMap>,
}

impl Context {
    pub fn new_with_builtins(
        units: &[Unit],
        sim_specs: &SimSpecs,
    ) -> (Self, Vec<(String, Vec<EquationError>)>) {
        // Built-in units: the XMILE 3.3.6 built-in units table (time units
        // with their abbreviation/singular aliases, and the `per_X = 1/X`
        // derived units), plus Vensim's default synonym list ($/dollar,
        // person/people, unit/units -- the `22:` groups Vensim writes into
        // every mdl). The third tuple field is the unit's equation (only the
        // per_* units have one). We only add a built-in if the model doesn't
        // already define a unit with that name.
        type BuiltinUnit = (&'static str, &'static [&'static str], Option<&'static str>);
        let builtin_units: &[BuiltinUnit] = &[
            ("$", &["dollar", "dollars", "$s"], None),
            ("person", &["people", "persons"], None),
            ("unit", &["units"], None),
            ("nanosecond", &["nanoseconds", "ns"], None),
            ("microsecond", &["microseconds", "us"], None),
            ("millisecond", &["milliseconds", "ms"], None),
            ("second", &["seconds", "s"], None),
            ("minute", &["minutes", "min"], None),
            ("hour", &["hours", "hr"], None),
            ("day", &["days"], None),
            ("week", &["weeks", "wk"], None),
            ("month", &["months", "mo"], None),
            ("quarter", &["quarters", "qtr"], None),
            ("year", &["years", "yr", "yrs"], None),
            ("per_second", &[], Some("1/second")),
            ("per_minute", &[], Some("1/minute")),
            ("per_hour", &[], Some("1/hour")),
            ("per_day", &[], Some("1/day")),
            ("per_week", &[], Some("1/week")),
            ("per_month", &[], Some("1/month")),
            ("per_quarter", &[], Some("1/quarter")),
            ("per_year", &[], Some("1/year")),
        ];

        // Collect ALL model-defined unit identifiers (primary names AND aliases) into one set.
        // This allows O(1) lookups when filtering built-ins.
        let model_unit_identifiers: std::collections::HashSet<String> = units
            .iter()
            .flat_map(|u| {
                std::iter::once(canonicalize(&u.name).into_owned())
                    .chain(u.aliases.iter().map(|a| canonicalize(a).into_owned()))
            })
            .collect();

        // Model-defined units come first, builtins after. `Self::new`
        // resolves equation-bearing units dependency-aware, so list order no
        // longer decides what an equation's referents bind to (a model
        // `week = day` and the builtin `per_week = 1/week` resolve correctly
        // in either order, as does a model `hazard = per_year`); order still
        // decides which declaration wins an alias-registration race, and
        // there the model's should.
        //
        // A builtin whose name or alias the model declares itself is not
        // added verbatim -- the model's definition wins (XMILE 3.3.6: on a
        // name collision "the implementation SHOULD respect the unit
        // definitions for the model"). But DROPPING the whole group severs
        // the equivalences: a model declaring the baseline primary `weeks`
        // would leave `week`/`wk` unregistered, so the surviving builtin
        // `per_week = 1/week` minted a phantom `week` base and no longer
        // equaled `1/weeks`. Instead the group's non-colliding spellings are
        // re-tied to the model's unit as a chained alias (`week = weeks`,
        // aliases `wk`) -- the alias-by-equation mechanism 3.3.6 describes
        // for exactly this purpose.
        let mut combined_units: Vec<Unit> = units.to_vec();
        for (name, aliases, equation) in builtin_units {
            let collided: Option<String> = std::iter::once(*name)
                .chain(aliases.iter().copied())
                .map(|n| canonicalize(n).into_owned())
                .find(|n| model_unit_identifiers.contains(n));
            match collided {
                None => combined_units.push(Unit {
                    name: name.to_string(),
                    equation: equation.map(|s| s.to_string()),
                    disabled: false,
                    aliases: aliases.iter().map(|s| s.to_string()).collect(),
                }),
                Some(model_spelling) => {
                    let mut leftovers = std::iter::once(*name)
                        .chain(aliases.iter().copied())
                        .filter(|n| !model_unit_identifiers.contains(&*canonicalize(n)));
                    if let Some(primary) = leftovers.next() {
                        combined_units.push(Unit {
                            name: primary.to_string(),
                            equation: Some(model_spelling),
                            disabled: false,
                            aliases: leftovers.map(|s| s.to_string()).collect(),
                        });
                    }
                }
            }
        }

        Self::new(&combined_units, sim_specs)
    }
    pub fn new(units: &[Unit], sim_specs: &SimSpecs) -> (Self, Vec<(String, Vec<EquationError>)>) {
        let mut unit_errors: Vec<(String, Vec<EquationError>)> = Vec::new();

        // Vensim's MDL files routinely repeat the same `22:` unit-equivalence
        // line in the settings footer (e.g. wrld3-03.mdl declares each
        // equivalence twice).  Re-declaring an identical mapping is harmless,
        // so we only flag a duplicate as an error when the new declaration
        // contradicts an existing one.  A helper to keep the logic uniform
        // across the alias and primary-name checks below.
        let dup_err = |name: &str| {
            (
                name.to_owned(),
                vec![EquationError {
                    start: 0,
                    end: 0,
                    code: ErrorCode::DuplicateUnit,
                }],
            )
        };

        // step 1: build our base context consisting of all prime units
        let mut aliases: HashMap<String, String> = HashMap::new();
        let mut parsed_units: HashMap<String, UnitMap> = HashMap::new();
        for unit in units.iter().filter(|unit| unit.equation.is_none()) {
            let unit_name = canonicalize(&unit.name).into_owned();
            for alias in unit.aliases.iter() {
                let alias = canonicalize(alias).into_owned();
                match aliases.entry(alias) {
                    Entry::Vacant(e) => {
                        e.insert(unit_name.clone());
                    }
                    Entry::Occupied(e) => {
                        if e.get() != &unit_name {
                            unit_errors.push(dup_err(&unit_name));
                        }
                    }
                }
            }
            // A primary name that is already an alias of *another* unit is a
            // genuine conflict; a primary name that's already itself a prime
            // unit is a benign re-declaration as long as the inferred unit
            // map matches.  A name that appears among its OWN aliases (a
            // self-alias -- e.g. `22:Yr,...,yr,...` where both `Yr` and `yr`
            // canonicalize to `yr`) is also benign: we must still register the
            // prime unit so that the name and its aliases resolve via lookup.
            if matches!(aliases.get(&unit_name), Some(target) if target != &unit_name) {
                unit_errors.push(dup_err(&unit_name));
            } else {
                let new_map: UnitMap = [(unit_name.clone(), 1)].iter().cloned().collect();
                match parsed_units.entry(unit_name.clone()) {
                    Entry::Vacant(e) => {
                        e.insert(new_map);
                    }
                    Entry::Occupied(e) => {
                        if e.get() != &new_map {
                            unit_errors.push(dup_err(&unit_name));
                        }
                    }
                }
            }
        }

        let mut ctx = Context {
            sim_specs: sim_specs.clone(),
            aliases,
            units: parsed_units,
        };

        // step 2: use this base context to parse our units with equations.
        //
        // Resolution is DEPENDENCY-AWARE, not in-list-order: `build_unit_
        // components` silently mints a fresh base unit for any name it cannot
        // look up, so an equation resolved before its referent's equation
        // binds to a phantom. One-way ordering cannot fix this -- a model
        // `week = day` must resolve before the builtin `per_week = 1/week`,
        // while a model `hazard = per_year` must resolve after the builtin
        // `per_year = 1/year` (XMILE 3.3.6 explicitly supports user-defined
        // aliases of built-in units). So each pass resolves every unit whose
        // equation references no still-unresolved equation-bearing unit, and
        // repeats until done. If a pass makes no progress the definitions are
        // circular (which XMILE 3.3.6 forbids); we then degrade to the old
        // in-order behavior -- unresolved references mint base units -- so a
        // malformed model still yields a usable partial context.

        /// Resolve one equation-bearing unit against the context built so
        /// far, registering its unit map (or recording errors).
        fn resolve_equation_unit(
            ctx: &mut Context,
            unit_errors: &mut Vec<(String, Vec<EquationError>)>,
            dup_err: &dyn Fn(&str) -> (String, Vec<EquationError>),
            unit_name: &str,
            ast: &Option<Expr0>,
        ) {
            let unit_components: UnitMap = match ast {
                Some(ast) => match build_unit_components(ctx, ast) {
                    Ok(unit_components) => unit_components,
                    Err(err) => {
                        unit_errors.push((
                            unit_name.to_owned(),
                            vec![EquationError {
                                start: 0,
                                end: 0,
                                code: err.code,
                            }],
                        ));
                        return;
                    }
                },
                None => [(unit_name.to_owned(), 1)].iter().cloned().collect(),
            };

            // As in step 1: only an alias of *another* unit is a conflict; a
            // self-alias is benign and the prime unit must still be registered.
            if matches!(ctx.aliases.get(unit_name), Some(target) if target != unit_name) {
                unit_errors.push(dup_err(unit_name));
            } else {
                match ctx.units.entry(unit_name.to_owned()) {
                    Entry::Vacant(e) => {
                        e.insert(unit_components);
                    }
                    Entry::Occupied(e) => {
                        if e.get() != &unit_components {
                            unit_errors.push(dup_err(unit_name));
                        }
                    }
                }
            }
        }

        /// The canonical unit names an equation AST references. Only the
        /// shapes `build_unit_components` accepts are walked; the shapes it
        /// rejects (App/Subscript/If) contribute no dependencies -- they
        /// error during resolution regardless of order.
        fn referenced_unit_names(ast: &Expr0, out: &mut Vec<String>) {
            match ast {
                Expr0::Var(id, _) => out.push(canonicalize(id.as_str()).into_owned()),
                Expr0::Op1(_, inner, _) => referenced_unit_names(inner, out),
                Expr0::Op2(_, l, r, _) => {
                    referenced_unit_names(l, out);
                    referenced_unit_names(r, out);
                }
                Expr0::Const(_, _, _)
                | Expr0::App(_, _)
                | Expr0::Subscript(_, _, _)
                | Expr0::If(_, _, _, _) => {}
            }
        }

        // Phase A: register every equation-bearing unit's aliases and lex its
        // equation. A unit whose equation fails to lex is reported and drops
        // out (references to it mint a base unit, as for any unknown name).
        // Registering ALL aliases before resolving ANY equation is part of
        // the dependency-awareness: an equation may reference an alias
        // declared later in the list.
        struct PendingUnit {
            name: String,
            ast: Option<Expr0>,
        }
        let mut pending: Vec<PendingUnit> = Vec::new();
        for unit in units.iter().filter(|unit| unit.equation.is_some()) {
            let unit_name = canonicalize(&unit.name).into_owned();
            for alias in unit.aliases.iter() {
                let alias = canonicalize(alias).into_owned();
                match ctx.aliases.entry(alias) {
                    Entry::Vacant(e) => {
                        e.insert(unit_name.clone());
                    }
                    Entry::Occupied(e) => {
                        if e.get() != &unit_name {
                            unit_errors.push(dup_err(&unit_name));
                        }
                    }
                }
            }

            let eqn = &join_multiword_unit_names(unit.equation.as_ref().unwrap());
            match Expr0::new(eqn, LexerType::Units) {
                Ok(ast) => pending.push(PendingUnit {
                    name: unit_name,
                    ast,
                }),
                Err(errors) => {
                    unit_errors.push((unit_name.clone(), errors));
                }
            }
        }

        // Phase B: resolve, deferring any unit blocked on a still-unresolved
        // equation-bearing unit. Passes are bounded by the dependency-chain
        // depth (at most the unit count).
        let mut unresolved: HashSet<String> = pending.iter().map(|p| p.name.clone()).collect();
        loop {
            let mut deferred: Vec<PendingUnit> = Vec::new();
            let mut progressed = false;
            for p in pending {
                let blocked = p.ast.as_ref().is_some_and(|ast| {
                    let mut deps = Vec::new();
                    referenced_unit_names(ast, &mut deps);
                    deps.iter().any(|dep| {
                        let dep = ctx.aliases.get(dep).map(|s| s.as_str()).unwrap_or(dep);
                        dep != p.name && unresolved.contains(dep)
                    })
                });
                if blocked {
                    deferred.push(p);
                    continue;
                }
                resolve_equation_unit(&mut ctx, &mut unit_errors, &dup_err, &p.name, &p.ast);
                unresolved.remove(&p.name);
                progressed = true;
            }
            if deferred.is_empty() {
                break;
            }
            if !progressed {
                // Circular definitions: degrade to in-order resolution.
                for p in deferred {
                    resolve_equation_unit(&mut ctx, &mut unit_errors, &dup_err, &p.name, &p.ast);
                }
                break;
            }
            pending = deferred;
        }

        // Construction is partial: always return the context we built, with any
        // conflicting/duplicate declarations reported alongside it. A single bad
        // unit declaration must not discard every other (valid) unit -- callers
        // surface the errors as diagnostics and keep using the context. This is
        // the context-layer parallel of the inference partial-results fix
        // (GH #614): an empty context would lose all alias normalization
        // (yr/year, person/people) and re-create a spurious mismatch flood.
        (ctx, unit_errors)
    }

    /// Alias-resolving map lookup WITHOUT `resolve_name`'s dimensionless
    /// special-case or unknown-name minting. Production resolution goes
    /// through `resolve_name`; this stays as test introspection (asserting
    /// that a name is registered/unregistered needs the `Option`).
    #[cfg(test)]
    pub(crate) fn lookup(&self, ident: &str) -> Option<&UnitMap> {
        // first, see if this identifier is an alias of a better-known unit
        let normalized = self.aliases.get(ident).map(|s| s.as_str()).unwrap_or(ident);
        self.units.get(normalized)
    }

    /// Resolve a single (already-canonicalized) unit NAME exactly the way a
    /// unit-equation reference does: resolve aliases, treat the
    /// dimensionless spellings as the empty map, look up known units, and
    /// mint a fresh base unit for anything unknown.
    ///
    /// Shared by `build_unit_components`' identifier arm and
    /// `units_check::model_time_units` -- the synthetic time variable MUST
    /// resolve `sim_specs.time_units` identically to a variable's `<units>`
    /// string, or a dimensionless clock (`time_units="Unitless"`) makes
    /// `x = TIME ~ Unitless` mismatch against a fictitious `{unitless: 1}`
    /// base unit.
    pub(crate) fn resolve_name(&self, canonical_name: &str) -> UnitMap {
        let name = self
            .aliases
            .get(canonical_name)
            .map(|s| s.as_str())
            .unwrap_or(canonical_name);
        if is_dimensionless_unit_name(name) {
            return UnitMap::new();
        }
        self.units
            .get(name)
            .cloned()
            .unwrap_or_else(|| [(name.to_owned(), 1)].iter().cloned().collect())
    }
}

/// The spellings XMILE 3.3.6 (and Vensim convention) treat as the identity
/// element for units. Kept in one place so unit-equation resolution and the
/// synthetic time variable cannot disagree about them.
pub(crate) fn is_dimensionless_unit_name(name: &str) -> bool {
    matches!(
        name,
        "dmnl" | "nil" | "dimensionless" | "unitless" | "fraction"
    )
}

#[allow(dead_code)]
fn const_int_eval(ast: &Expr0) -> EquationResult<i32> {
    match ast {
        Expr0::Const(_, n, loc) => {
            let n = n.value();
            if approx_eq(n, n.round()) {
                Ok(n.round() as i32)
            } else {
                eqn_err!(ExpectedInteger, loc.start, loc.end)
            }
        }
        Expr0::Var(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr0::App(_, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr0::Subscript(_, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
        Expr0::Op1(op, expr, loc) => {
            let expr = const_int_eval(expr)?;
            let result = match op {
                UnaryOp::Positive => expr,
                UnaryOp::Negative => -expr,
                UnaryOp::Not => i32::from(expr == 0),
                UnaryOp::Transpose => {
                    // Transpose doesn't make sense for integer evaluation
                    return eqn_err!(ExpectedInteger, loc.start, loc.end);
                }
            };
            Ok(result)
        }
        Expr0::Op2(op, l, r, _) => {
            let l = const_int_eval(l)?;
            let r = const_int_eval(r)?;
            let result = match op {
                BinaryOp::Add => l + r,
                BinaryOp::Sub => l - r,
                BinaryOp::Exp => l.pow(r as u32),
                BinaryOp::Mul => l * r,
                BinaryOp::Div => {
                    if r == 0 {
                        0
                    } else {
                        l / r
                    }
                }
                BinaryOp::Mod => l % r,
                BinaryOp::Gt => (l > r) as i32,
                BinaryOp::Lt => (l < r) as i32,
                BinaryOp::Gte => (l >= r) as i32,
                BinaryOp::Lte => (l <= r) as i32,
                BinaryOp::Eq => (l == r) as i32,
                BinaryOp::Neq => (l != r) as i32,
                BinaryOp::And => ((l != 0) && (r != 0)) as i32,
                BinaryOp::Or => ((l != 0) || (r != 0)) as i32,
            };
            Ok(result)
        }
        Expr0::If(_, _, _, loc) => {
            eqn_err!(ExpectedInteger, loc.start, loc.end)
        }
    }
}

fn build_unit_components(ctx: &Context, ast: &Expr0) -> EquationResult<UnitMap> {
    let unit_map: UnitMap = match ast {
        Expr0::Const(_, _, loc) => {
            // dimensionless is special
            if let Ok(1) = const_int_eval(ast) {
                UnitMap::new()
            } else {
                // nothing to do here (handled below in Op2)
                return eqn_err!(NoConstInUnits, loc.start, loc.end);
            }
        }
        Expr0::Var(id, _) => {
            // Canonicalize the unit name (lowercase) for consistent lookup;
            // alias resolution, the dimensionless spellings, and the
            // unknown-name base-unit fallback all live in `resolve_name`
            // (shared with the synthetic time variable).
            ctx.resolve_name(&crate::common::canonicalize(id.as_str()))
        }
        Expr0::App(_, loc) => {
            return eqn_err!(NoAppInUnits, loc.start, loc.end);
        }
        Expr0::Subscript(_, _, loc) => {
            return eqn_err!(NoSubscriptInUnits, loc.start, loc.end);
        }
        Expr0::Op1(_, _, loc) => {
            return eqn_err!(NoUnaryOpInUnits, loc.start, loc.end);
        }
        Expr0::Op2(op, l, r, loc) => match op {
            BinaryOp::Exp => {
                let exp = const_int_eval(r)?;
                build_unit_components(ctx, l)?.exp(exp)
            }
            BinaryOp::Mul => build_unit_components(ctx, l)? * build_unit_components(ctx, r)?,
            BinaryOp::Div => {
                // check first for the reciprocal case -- 1/blah
                if let Ok(i) = const_int_eval(l) {
                    if i != 1 {
                        let loc = l.get_loc();
                        return eqn_err!(ExpectedIntegerOne, loc.start, loc.end);
                    }
                    build_unit_components(ctx, r)?.reciprocal()
                } else {
                    build_unit_components(ctx, l)? / build_unit_components(ctx, r)?
                }
            }
            _ => {
                return eqn_err!(BadBinaryOpInUnits, loc.start, loc.end);
            }
        },
        Expr0::If(_, _, _, loc) => {
            return eqn_err!(NoIfInUnits, loc.start, loc.end);
        }
    };

    Ok(unit_map)
}

pub fn parse_units(
    ctx: &Context,
    unit_eqn: Option<&str>,
) -> StdResult<Option<UnitMap>, Vec<UnitError>> {
    if let Some(unit_eqn) = unit_eqn {
        // Carry the offending source text on every definition error: a bare
        // token-error code ("extra_token") with no context is not actionable
        // for the modeler.
        let context = || format!("in units '{unit_eqn}'");
        let unit_eqn_joined = join_multiword_unit_names(unit_eqn);
        if let Some(expr) = Expr0::new(&unit_eqn_joined, LexerType::Units).map_err(|errors| {
            errors
                .into_iter()
                .map(|err| UnitError::DefinitionError(err, Some(context())))
                .collect::<Vec<UnitError>>()
        })? {
            let result = build_unit_components(ctx, &expr)
                .map_err(|err| vec![UnitError::DefinitionError(err, Some(context()))])?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    } else {
        Ok(None)
    }
}

#[test]
fn join_multiword_unit_names_preserves_byte_length() {
    // Parse-error byte offsets are computed against the JOINED string but
    // rendered against the ORIGINAL (errors::format_snippet and the
    // FormattedError start/end offsets carried to GUI consumers), so the
    // join must be byte-length-preserving or every underline after a
    // whitespace run shifts.
    let cases = [
        ("Degrees Fahrenheit/Minute", "Degrees_Fahrenheit/Minute"),
        ("widgets  //  year", "widgets  //  year"),
        ("a  b", "a__b"),
        ("people * meter", "people * meter"),
        (" leading and trailing ", " leading_and_trailing "),
        ("a\tb", "a_b"),
    ];
    for (input, expected) in cases {
        let joined = join_multiword_unit_names(input);
        assert_eq!(joined.as_ref(), expected, "join of {input:?}");
        assert_eq!(
            joined.len(),
            input.len(),
            "byte length must be preserved for {input:?}"
        );
    }
}

#[test]
fn self_aliased_prime_unit_is_registered() {
    // A Vensim `22:` equivalence group can list the canonical form of its own
    // primary name among its aliases.  C-LEARN declares
    // `22:Yr,year,years,yr,Year,Years`: the primary `Yr` canonicalizes to `yr`,
    // and `yr` ALSO appears in the alias list, so `yr` becomes an alias of
    // itself.  The prime unit must still be registered so that the primary
    // name and every alias resolve to the same UnitMap.  Before the fix the
    // self-alias caused `Context::new` to skip registering the prime unit, so
    // `lookup` returned None and callers fell back to whatever (un-normalized)
    // name they queried with -- the source of the C-LEARN `year` vs `yr`
    // mismatch flood.
    let (ctx, errors) = Context::new(
        &[Unit {
            name: "Yr".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec![
                "year".to_owned(),
                "years".to_owned(),
                "yr".to_owned(),
                "Year".to_owned(),
                "Years".to_owned(),
            ],
        }],
        &Default::default(),
    );
    assert!(
        errors.is_empty(),
        "a self-aliased unit group must not produce a DuplicateUnit error"
    );

    let yr = ctx.lookup("yr");
    let year = ctx.lookup("year");
    assert!(
        yr.is_some(),
        "lookup(\"yr\") must resolve the registered prime unit, got None"
    );
    assert_eq!(
        yr, year,
        "\"yr\" and \"year\" must resolve to the same UnitMap"
    );
}

/// Context construction is partial: one conflicting unit declaration must not
/// discard the whole context. The unrelated valid units (and their alias
/// normalization) still resolve, and the conflict is reported alongside them
/// rather than throwing every definition away. This is the context-layer
/// parallel of the inference all-or-nothing fix (GH #614); it closes the
/// long-standing "we shouldn't discard the whole context if there are errors"
/// TODO.
#[test]
fn context_construction_keeps_valid_units_despite_a_conflict() {
    let units = [
        // A valid alias group: `yr` normalizes to `year`.
        Unit {
            name: "year".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["yr".to_owned()],
        },
        // A genuine conflict: `m` is claimed as an alias of two different units.
        Unit {
            name: "meter".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["m".to_owned()],
        },
        Unit {
            name: "second".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["m".to_owned()],
        },
    ];
    let (ctx, errors) = Context::new(&units, &Default::default());

    assert!(
        !errors.is_empty(),
        "the conflicting `m` alias must be reported"
    );
    // ...but the unrelated valid alias group still resolves.
    assert!(
        ctx.lookup("yr").is_some(),
        "valid units must survive a conflict elsewhere in the unit list"
    );
    assert_eq!(
        ctx.lookup("yr"),
        ctx.lookup("year"),
        "`yr` must still normalize to `year` despite the conflict"
    );
}

#[test]
fn test_pretty_print_unit() {
    let context = Context::new(
        &[
            Unit {
                name: "time".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec![],
            },
            Unit {
                name: "people".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["person".to_owned(), "persons".to_owned()],
            },
            Unit {
                name: "meter".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["m".to_owned(), "meters".to_owned()],
            },
            Unit {
                name: "second".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["s".to_owned(), "seconds".to_owned()],
            },
        ],
        &Default::default(),
    )
    .0;

    let positive_cases: &[(&str, &str); 9] = &[
        ("m^2/s", "meter^2/second"),
        ("person * people * persons", "people^3"),
        ("m^2/meters", "meter"),
        ("m*people/time", "meter*people/time"),
        ("time * people / time", "people"),
        ("1", "dmnl"),
        ("1/dmnl", "dmnl"),
        ("1/s", "1/second"),
        ("1/s/m", "1/meter/second"),
    ];

    for (input, output) in positive_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap();
        let pretty = result.pretty_print();
        assert_eq!(*output, pretty);
    }
}

// we have 3 problems here: the first (and simpler) is evaluating unit equations and turning them in to UnitMaps (done)
// the second is: given a context of unitmaps, can we _check_ the types of variables.  This won't work if there are builtins in use.
// the third is: if we only have _some_ units filled in, can we _infer_ the rest? This will also enable units for builtins

#[test]
fn test_context_creation() {
    let simple_units = &[
        Unit {
            name: "time".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec![],
        },
        Unit {
            name: "people".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec!["person".to_owned(), "persons".to_owned()],
        },
    ];

    let expected = Context {
        sim_specs: Default::default(),
        aliases: [
            ("person".to_owned(), "people".to_owned()),
            ("persons".to_owned(), "people".to_owned()),
        ]
        .iter()
        .cloned()
        .collect(),
        units: [
            (
                "time".to_owned(),
                [("time".to_owned(), 1)].iter().cloned().collect(),
            ),
            (
                "people".to_owned(),
                [("people".to_owned(), 1)].iter().cloned().collect(),
            ),
        ]
        .iter()
        .cloned()
        .collect(),
    };

    assert_eq!(expected, Context::new(simple_units, &Default::default()).0);

    let more_units = &[
        Unit {
            name: "time".to_owned(),
            equation: None,
            disabled: false,
            aliases: vec![],
        },
        Unit {
            name: "invtime".to_owned(),
            equation: Some("1/time".to_owned()),
            disabled: false,
            aliases: vec!["itime".to_owned()],
        },
    ];

    let expected2 = Context {
        sim_specs: Default::default(),
        aliases: [("itime".to_owned(), "invtime".to_owned())]
            .iter()
            .cloned()
            .collect(),
        units: [
            (
                "time".to_owned(),
                [("time".to_owned(), 1)].iter().cloned().collect(),
            ),
            (
                "invtime".to_owned(),
                [("time".to_owned(), -1)].iter().cloned().collect(),
            ),
        ]
        .iter()
        .cloned()
        .collect(),
    };

    assert_eq!(expected2, Context::new(more_units, &Default::default()).0);
}

#[test]
fn test_basic_unit_parsing() {
    let context = Context::new(
        &[
            Unit {
                name: "time".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec![],
            },
            Unit {
                name: "people".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["person".to_owned(), "persons".to_owned()],
            },
            Unit {
                name: "meter".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["m".to_owned(), "meters".to_owned()],
            },
            Unit {
                name: "second".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["s".to_owned(), "seconds".to_owned()],
            },
        ],
        &Default::default(),
    )
    .0;

    let positive_cases: &[(&str, UnitMap); 6] = &[
        (
            "m^2/s",
            [("meter".to_owned(), 2), ("second".to_owned(), -1)]
                .iter()
                .cloned()
                .collect(),
        ),
        (
            "person * people * persons",
            [("people".to_owned(), 3)].iter().cloned().collect(),
        ),
        (
            "m^2/meters",
            [("meter".to_owned(), 1)].iter().cloned().collect(),
        ),
        (
            "time * people / time",
            [("people".to_owned(), 1)].iter().cloned().collect(),
        ),
        ("1", UnitMap::new()),
        ("dmnl", UnitMap::new()),
    ];

    for (input, output) in positive_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap();
        assert_eq!(*output, result);
    }

    use crate::common::ErrorCode;

    let negative_cases = &[
        ("2 / time", ErrorCode::ExpectedIntegerOne),
        ("2 * time", ErrorCode::NoConstInUnits),
        ("foo(time)", ErrorCode::NoAppInUnits),
        ("bar[time]", ErrorCode::NoSubscriptInUnits),
        ("-time", ErrorCode::NoUnaryOpInUnits),
        ("if 1 then time else people", ErrorCode::NoIfInUnits),
        ("time + people", ErrorCode::BadBinaryOpInUnits),
    ];

    for (input, output) in negative_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap_err();
        assert_eq!(*output, result.code);
    }
}

#[test]
fn test_basic_unit_checks() {
    let _context = Context::new(
        &[
            Unit {
                name: "time".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec![],
            },
            Unit {
                name: "people".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["person".to_owned(), "persons".to_owned()],
            },
            Unit {
                name: "USD".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["dollar".to_owned(), "dollars".to_owned(), "$".to_owned()],
            },
        ],
        &Default::default(),
    )
    .0;
    // from a set of datamodel::Units build a Context

    // with a context, check if a set of variables unit checks
}

#[test]
fn test_const_int_eval() {
    let positive_cases = &[
        ("0", 0),
        ("1", 1),
        ("-1", -1),
        ("1 * 1", 1),
        ("2 / 3", 0),
        ("7 / 0", 0),
        ("4 - 1", 3),
        ("15 mod 7", 1),
        ("3^(1+2)", 27),
        ("4 > 2", 1),
        ("4 < 2", 0),
        ("5 >= 5", 1),
        ("7 <= 6", 0),
        ("3 and 2", 1),
        ("0 or 3", 1),
        ("3 = 3", 1),
        ("3 <> 3", 0),
        ("not 7", 0),
        ("not 0", 1),
    ];

    for (input, output) in positive_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        assert_eq!(*output, const_int_eval(&expr).unwrap());
    }

    use crate::common::ErrorCode;

    let negative_cases = &["3.5", "foo", "if 1 then 2 else 3", "bar[2]", "foo(1, 2)"];

    for input in negative_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        assert_eq!(
            ErrorCode::ExpectedInteger,
            const_int_eval(&expr).unwrap_err().code
        );
    }
}

#[test]
fn test_unit_canonicalization() {
    // Verify that unit names and aliases are properly canonicalized
    let context = Context::new(
        &[
            Unit {
                name: "Meter".to_owned(), // non-canonical case
                equation: None,
                disabled: false,
                aliases: vec!["M".to_owned(), "METERS".to_owned()],
            },
            Unit {
                name: "Second".to_owned(),
                equation: None,
                disabled: false,
                aliases: vec!["S".to_owned(), "SECONDS".to_owned()],
            },
        ],
        &Default::default(),
    )
    .0;

    // All of these should resolve to the same canonical unit
    let test_cases = &["meter", "Meter", "METER", "m", "M", "meters", "METERS"];
    for input in test_cases {
        let expr = Expr0::new(input, LexerType::Units).unwrap().unwrap();
        let result = build_unit_components(&context, &expr).unwrap();
        assert_eq!(
            result,
            [("meter".to_owned(), 1)]
                .iter()
                .cloned()
                .collect::<UnitMap>(),
            "Expected '{input}' to canonicalize to 'meter'"
        );
    }

    // Verify compound units with mixed case also work
    let expr = Expr0::new("METER/SECOND", LexerType::Units)
        .unwrap()
        .unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    let expected: UnitMap = [("meter".to_owned(), 1), ("second".to_owned(), -1)]
        .iter()
        .cloned()
        .collect();
    assert_eq!(result, expected, "Compound units should be canonicalized");

    // Verify that aliases resolve through case-insensitive matching
    let expr = Expr0::new("M*S", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    let expected: UnitMap = [("meter".to_owned(), 1), ("second".to_owned(), 1)]
        .iter()
        .cloned()
        .collect();
    assert_eq!(result, expected, "Aliases should be resolved correctly");
}

#[test]
fn test_year_years_builtin_alias() {
    // Test that the year/years builtin alias works correctly
    // This tests the `new_with_builtins` path which adds built-in aliases
    let context = Context::new_with_builtins(&[], &Default::default()).0;

    // Test singular form
    let expr = Expr0::new("year", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    let expected: UnitMap = [("year".to_owned(), 1)].iter().cloned().collect();
    assert_eq!(result, expected, "year should parse correctly");

    // Test plural form - should resolve to singular
    let expr = Expr0::new("years", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "years should resolve to year");

    // Test "yr" abbreviation - common in system dynamics models
    let expr = Expr0::new("yr", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "yr should resolve to year");

    // Test "yrs" abbreviation
    let expr = Expr0::new("yrs", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "yrs should resolve to year");

    // Test that year and years are treated as the same unit in expressions
    let expr = Expr0::new("year/years", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    let expected_dmnl: UnitMap = UnitMap::new();
    assert_eq!(
        result, expected_dmnl,
        "year/years should be dimensionless (cancel out)"
    );

    // Test yr/years - all aliases should be interchangeable
    let expr = Expr0::new("yr/years", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(
        result, expected_dmnl,
        "yr/years should be dimensionless (cancel out)"
    );

    // Test compound expressions
    let expr = Expr0::new("1/years", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    let expected_inverse: UnitMap = [("year".to_owned(), -1)].iter().cloned().collect();
    assert_eq!(result, expected_inverse, "1/years should be 1/year");

    // Test 1/yr
    let expr = Expr0::new("1/yr", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected_inverse, "1/yr should be 1/year");
}

#[test]
fn test_builtin_dollar_equivalences() {
    let context = Context::new_with_builtins(&[], &Default::default()).0;

    let expected: UnitMap = [("$".to_owned(), 1)].iter().cloned().collect();

    let expr = Expr0::new("$", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "$ should parse correctly");

    let expr = Expr0::new("Dollar", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "Dollar should resolve to $");

    let expr = Expr0::new("Dollars", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "Dollars should resolve to $");

    let expr = Expr0::new("$s", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "$s should resolve to $");

    // Dollar/Dollars should cancel out
    let expr = Expr0::new("Dollar/Dollars", LexerType::Units)
        .unwrap()
        .unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    let expected_dmnl: UnitMap = UnitMap::new();
    assert_eq!(result, expected_dmnl, "Dollar/Dollars should cancel out");
}

#[test]
fn test_builtin_unit_equivalences() {
    let context = Context::new_with_builtins(&[], &Default::default()).0;

    let expected: UnitMap = [("unit".to_owned(), 1)].iter().cloned().collect();

    let expr = Expr0::new("Unit", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "Unit should parse correctly");

    let expr = Expr0::new("Units", LexerType::Units).unwrap().unwrap();
    let result = build_unit_components(&context, &expr).unwrap();
    assert_eq!(result, expected, "Units should resolve to unit");
}

#[test]
fn test_redundant_duplicate_unit_declarations_are_benign() {
    // Vensim MDL files routinely repeat the same `22:` unit-equivalence
    // line.  Identical re-declarations should not poison the units context;
    // only contradictory mappings should produce a DuplicateUnit error.
    let units = vec![
        Unit {
            name: "Resource unit".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["Resource units".to_string()],
        },
        // Verbatim duplicate -- harmless.
        Unit {
            name: "Resource unit".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["Resource units".to_string()],
        },
    ];
    let (context, errors) = Context::new_with_builtins(&units, &Default::default());
    assert!(
        errors.is_empty(),
        "duplicate identical unit declarations must be tolerated; got an error"
    );

    // Both spellings must still resolve through the alias to the same canonical unit.
    let expr = Expr0::new("Resource_unit", LexerType::Units)
        .unwrap()
        .unwrap();
    let r1 = build_unit_components(&context, &expr).unwrap();
    let expr = Expr0::new("Resource_units", LexerType::Units)
        .unwrap()
        .unwrap();
    let r2 = build_unit_components(&context, &expr).unwrap();
    assert_eq!(
        r1, r2,
        "alias resolution must survive duplicate declarations"
    );
}

#[test]
fn test_conflicting_unit_declarations_are_still_errors() {
    // If two declarations disagree about what an alias points to, that is a
    // real conflict and must still produce a DuplicateUnit error.
    let units = vec![
        Unit {
            name: "Foo".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["FB".to_string()],
        },
        Unit {
            name: "Bar".to_string(),
            equation: None,
            disabled: false,
            aliases: vec!["FB".to_string()], // same alias mapped to a different primary
        },
    ];
    let (_ctx, errors) = Context::new(&units, &Default::default());
    assert!(
        !errors.is_empty(),
        "conflicting alias declarations must still produce an error"
    );
}

#[test]
fn debug_user_alias_with_underscore_identifiers() {
    // Reproduce the WRLD3 situation: a user declares `Resource unit` with
    // alias `Resource units`, and variables write `Resource_unit` and
    // `Resource_units` (spaces → underscores) as their declared units.
    // Both parses should produce the same UnitMap via alias resolution.
    let units = vec![Unit {
        name: "Resource unit".to_string(),
        equation: None,
        disabled: false,
        aliases: vec!["Resource units".to_string()],
    }];
    let context = Context::new_with_builtins(&units, &Default::default()).0;

    let expr = Expr0::new("Resource_unit", LexerType::Units)
        .unwrap()
        .unwrap();
    let result1 = build_unit_components(&context, &expr).unwrap();

    let expr = Expr0::new("Resource_units", LexerType::Units)
        .unwrap()
        .unwrap();
    let result2 = build_unit_components(&context, &expr).unwrap();

    assert_eq!(
        result1, result2,
        "Resource_unit and Resource_units should resolve to the same unit map. \
         Got result1={result1:?} result2={result2:?}"
    );
}
