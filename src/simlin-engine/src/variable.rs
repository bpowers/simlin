// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

#[cfg(test)]
use crate::ast::Loc;
use crate::ast::{Ast, Expr0, Expr2, IndexExpr2};
use crate::builtins::{BuiltinContents, BuiltinFn, walk_builtin_expr};
use crate::builtins_visitor::instantiate_implicit_modules;
use crate::common::{
    Canonical, CanonicalElementName, DimensionName, EquationError, EquationResult, Ident,
    UnitError, canonicalize,
};
use crate::datamodel;
use crate::dimensions::{Dimension, DimensionsContext};
use crate::lexer::LexerType;
#[cfg(test)]
use crate::model::ScopeStage0;
use crate::units::parse_units;
use crate::{ErrorCode, eqn_err, units};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct Table {
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    x_range: datamodel::GraphicalFunctionScale,
    y_range: datamodel::GraphicalFunctionScale,
}

impl Table {
    /// Creates an empty placeholder table that returns NaN for any lookup.
    fn empty() -> Self {
        Table {
            x: Vec::new(),
            y: Vec::new(),
            x_range: datamodel::GraphicalFunctionScale { min: 0.0, max: 0.0 },
            y_range: datamodel::GraphicalFunctionScale { min: 0.0, max: 0.0 },
        }
    }

    #[cfg(test)]
    pub fn new_for_test(x: Vec<f64>, y: Vec<f64>) -> Self {
        let x_min = x.first().copied().unwrap_or(0.0);
        let x_max = x.last().copied().unwrap_or(0.0);
        let y_min = y.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let y_max = y.iter().fold(f64::NEG_INFINITY, |a, &b| a.max(b));

        Table {
            x,
            y,
            x_range: datamodel::GraphicalFunctionScale {
                min: x_min,
                max: x_max,
            },
            y_range: datamodel::GraphicalFunctionScale {
                min: y_min,
                max: y_max,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, salsa::Update)]
pub struct ModuleInput {
    // the Variable identifier in the current model we will use for input
    pub src: Ident<Canonical>,
    // the Variable identifier in the module's model we will override
    pub dst: Ident<Canonical>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub enum Variable<MI = ModuleInput, E = Expr2> {
    Stock {
        ident: Ident<Canonical>,
        init_ast: Option<Ast<E>>,
        eqn: Option<datamodel::Equation>,
        units: Option<datamodel::UnitMap>,
        inflows: Vec<Ident<Canonical>>,
        outflows: Vec<Ident<Canonical>>,
        non_negative: bool,
        errors: Vec<EquationError>,
        unit_errors: Vec<UnitError>,
    },
    Var {
        ident: Ident<Canonical>,
        ast: Option<Ast<E>>,
        init_ast: Option<Ast<E>>,
        eqn: Option<datamodel::Equation>,
        units: Option<datamodel::UnitMap>,
        tables: Vec<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        errors: Vec<EquationError>,
        unit_errors: Vec<UnitError>,
    },
    Module {
        // the current spec has ident == model name
        ident: Ident<Canonical>,
        model_name: Ident<Canonical>,
        units: Option<datamodel::UnitMap>,
        inputs: Vec<MI>,
        errors: Vec<EquationError>,
        unit_errors: Vec<UnitError>,
    },
}

impl<MI, E> Variable<MI, E> {
    pub fn ident(&self) -> &str {
        match self {
            Variable::Stock { ident: name, .. }
            | Variable::Var { ident: name, .. }
            | Variable::Module { ident: name, .. } => name.as_str(),
        }
    }

    pub fn canonical_ident(&self) -> &Ident<Canonical> {
        match self {
            Variable::Stock { ident: name, .. }
            | Variable::Var { ident: name, .. }
            | Variable::Module { ident: name, .. } => name,
        }
    }

    pub fn ast(&self) -> Option<&Ast<E>> {
        match self {
            Variable::Stock {
                init_ast: Some(ast),
                ..
            }
            | Variable::Var { ast: Some(ast), .. } => Some(ast),
            _ => None,
        }
    }

    // returns the init_ast if one exists, otherwise ast()
    pub fn init_ast(&self) -> Option<&Ast<E>> {
        if let Variable::Var {
            init_ast: Some(ast),
            ..
        } = self
        {
            return Some(ast);
        }
        self.ast()
    }

    pub fn scalar_equation(&self) -> Option<&String> {
        match self {
            Variable::Stock {
                eqn: Some(datamodel::Equation::Scalar(s)),
                ..
            }
            | Variable::Var {
                eqn: Some(datamodel::Equation::Scalar(s)),
                ..
            } => Some(s),
            _ => None,
        }
    }

    pub fn get_dimensions(&self) -> Option<&[Dimension]> {
        match self {
            Variable::Stock {
                init_ast: Some(Ast::Arrayed(dims, _, _, _)),
                ..
            }
            | Variable::Var {
                ast: Some(Ast::Arrayed(dims, _, _, _)),
                ..
            } => Some(dims),
            Variable::Stock {
                init_ast: Some(Ast::ApplyToAll(dims, _)),
                ..
            }
            | Variable::Var {
                ast: Some(Ast::ApplyToAll(dims, _)),
                ..
            } => Some(dims),
            _ => None,
        }
    }

    pub fn is_stock(&self) -> bool {
        matches!(self, Variable::Stock { .. })
    }

    pub fn is_module(&self) -> bool {
        matches!(self, Variable::Module { .. })
    }

    pub fn equation_errors(&self) -> Option<Vec<EquationError>> {
        let errors = match self {
            Variable::Stock { errors, .. }
            | Variable::Var { errors, .. }
            | Variable::Module { errors, .. } => errors,
        };
        if errors.is_empty() {
            None
        } else {
            Some(errors.clone())
        }
    }

    pub fn unit_errors(&self) -> Option<Vec<UnitError>> {
        let errors = match self {
            Variable::Stock { unit_errors, .. }
            | Variable::Var { unit_errors, .. }
            | Variable::Module { unit_errors, .. } => unit_errors,
        };
        if errors.is_empty() {
            None
        } else {
            Some(errors.clone())
        }
    }

    pub fn push_error(&mut self, err: EquationError) {
        match self {
            Variable::Stock { errors, .. }
            | Variable::Var { errors, .. }
            | Variable::Module { errors, .. } => errors.push(err),
        }
    }

    pub fn push_unit_error(&mut self, err: UnitError) {
        match self {
            Variable::Stock { unit_errors, .. }
            | Variable::Var { unit_errors, .. }
            | Variable::Module { unit_errors, .. } => unit_errors.push(err),
        }
    }

    pub fn table(&self) -> Option<&Table> {
        match self {
            Variable::Stock { .. } => None,
            Variable::Var { tables, .. } => tables.first(),
            Variable::Module { .. } => None,
        }
    }

    pub fn tables(&self) -> &[Table] {
        match self {
            Variable::Stock { .. } => &[],
            Variable::Var { tables, .. } => tables,
            Variable::Module { .. } => &[],
        }
    }

    pub fn units(&self) -> Option<&datamodel::UnitMap> {
        match self {
            Variable::Stock { units, .. } => units.as_ref(),
            Variable::Var { units, .. } => units.as_ref(),
            Variable::Module { units, .. } => units.as_ref(),
        }
    }
}

#[allow(clippy::unnecessary_wraps)]
pub(crate) fn parse_table(
    gf: &Option<datamodel::GraphicalFunction>,
) -> EquationResult<Option<Table>> {
    if gf.is_none() {
        return Ok(None);
    }
    let gf = gf.as_ref().unwrap();

    let x: Vec<f64> = match &gf.x_points {
        Some(x_points) => x_points.clone(),
        None => {
            let x_min = gf.x_scale.min;
            let x_max = gf.x_scale.max;
            let size = gf.y_points.len() as f64;
            gf.y_points
                .iter()
                .enumerate()
                .map(|(i, _)| ((i as f64) / (size - 1.0)) * (x_max - x_min) + x_min)
                .collect()
        }
    };

    Ok(Some(Table {
        x,
        y: gf.y_points.clone(),
        x_range: gf.x_scale.clone(),
        y_range: gf.y_scale.clone(),
    }))
}

/// Build the tables vector from equation and variable-level gf.
/// For arrayed variables with per-element gfs, tables are built from each element.
/// For scalar variables or arrayed without per-element gfs, uses variable-level gf.
fn build_tables(
    gf: &Option<datamodel::GraphicalFunction>,
    equation: &datamodel::Equation,
) -> (Vec<Table>, Vec<EquationError>) {
    let mut tables = Vec::new();
    let mut errors = Vec::new();

    // Check for per-element gfs in arrayed equation
    if let datamodel::Equation::Arrayed(_, elements, _) = equation {
        let has_element_gfs = elements.iter().any(|(_, _, _, gf)| gf.is_some());
        if has_element_gfs {
            for (_, _, _, elem_gf) in elements {
                match parse_table(elem_gf) {
                    Ok(Some(table)) => tables.push(table),
                    Ok(None) => {
                        // Element has no gf - insert empty placeholder to maintain indexing
                        // so that table[element_offset] corresponds to the correct element.
                        // Lookups on empty tables return NaN.
                        tables.push(Table::empty());
                    }
                    Err(err) => errors.push(err),
                }
            }
            return (tables, errors);
        }
    }

    // Fall back to variable-level gf
    match parse_table(gf) {
        Ok(Some(table)) => tables.push(table),
        Ok(None) => {}
        Err(err) => errors.push(err),
    }

    (tables, errors)
}

fn get_dimensions(
    dimensions: &[datamodel::Dimension],
    names: &[DimensionName],
) -> Result<Vec<Dimension>, EquationError> {
    names
        .iter()
        .map(|name| -> Result<Dimension, EquationError> {
            for dim in dimensions {
                if dim.name() == name {
                    return Ok(Dimension::from(dim));
                }
            }
            eqn_err!(BadDimensionName, 0, 0)
        })
        .collect()
}

fn parse_equation(
    eqn: &datamodel::Equation,
    dimensions: &[datamodel::Dimension],
    is_initial: bool,
    active_initial: Option<&str>,
) -> (Option<Ast<Expr0>>, Vec<EquationError>) {
    fn should_apply_default_to_missing(
        dimension_names: &[DimensionName],
        dimensions: &[datamodel::Dimension],
        elements: &[(
            String,
            String,
            Option<String>,
            Option<datamodel::GraphicalFunction>,
        )],
        default_eq: &Option<String>,
    ) -> bool {
        let Some(default_eq) = default_eq else {
            return false;
        };

        let Ok(dims) = get_dimensions(dimensions, dimension_names) else {
            return false;
        };
        let total_slots: usize = dims.iter().map(|d| d.len()).product();
        if total_slots <= elements.len() {
            return false;
        }

        // EXCEPT conversion produces sparse arrays where ALL explicit elements
        // have the same equation as the default (the base equation).  Override-
        // style sparse arrays have at least one element that differs.
        !elements
            .iter()
            .all(|(_, eqn, _, _)| eqn.trim() == default_eq.trim())
    }

    fn parse_inner(eqn: &str) -> (Option<Expr0>, Vec<EquationError>) {
        match Expr0::new(eqn, LexerType::Equation) {
            Ok(expr) => (expr, vec![]),
            Err(errors) => (None, errors),
        }
    }
    match eqn {
        datamodel::Equation::Scalar(eqn) => {
            let (ast, errors) = if !is_initial {
                parse_inner(eqn)
            } else if let Some(init_eqn) = active_initial {
                parse_inner(init_eqn)
            } else {
                (None, vec![])
            };
            (ast.map(Ast::Scalar), errors)
        }
        datamodel::Equation::ApplyToAll(dimension_names, eqn) => {
            let (ast, mut errors) = if !is_initial {
                parse_inner(eqn)
            } else if let Some(init_eqn) = active_initial {
                parse_inner(init_eqn)
            } else {
                (None, vec![])
            };

            match get_dimensions(dimensions, dimension_names) {
                Ok(dims) => (ast.map(|ast| Ast::ApplyToAll(dims, ast)), errors),
                Err(err) => {
                    errors.push(err);
                    (None, errors)
                }
            }
        }
        // Preserve the default equation (EXCEPT semantics) so sparse array
        // definitions can apply it to omitted elements during lowering.
        datamodel::Equation::Arrayed(dimension_names, elements, default_eq) => {
            let mut errors: Vec<EquationError> = vec![];
            let apply_default_to_missing =
                should_apply_default_to_missing(dimension_names, dimensions, elements, default_eq);
            let elements: HashMap<_, _> = elements
                .iter()
                .map(|(subscript, eqn, init_eqn, _gf)| {
                    let (ast, single_errors) = if is_initial && init_eqn.is_some() {
                        parse_inner(init_eqn.as_ref().unwrap())
                    } else {
                        parse_inner(eqn)
                    };
                    errors.extend(single_errors);
                    (CanonicalElementName::from_raw(subscript), ast)
                })
                .filter(|(_, ast)| ast.is_some())
                .map(|(subscript, ast)| (subscript, ast.unwrap()))
                .collect();
            let default_expr = default_eq.as_ref().and_then(|eqn| {
                let (ast, default_errors) = parse_inner(eqn);
                errors.extend(default_errors);
                ast
            });

            match get_dimensions(dimensions, dimension_names) {
                Ok(dims) => (
                    Some(Ast::Arrayed(
                        dims,
                        elements,
                        default_expr,
                        apply_default_to_missing,
                    )),
                    errors,
                ),
                Err(err) => {
                    errors.push(err);
                    (None, errors)
                }
            }
        }
    }
}

pub fn parse_var<MI, F>(
    dimensions: &[datamodel::Dimension],
    v: &datamodel::Variable,
    implicit_vars: &mut Vec<datamodel::Variable>,
    units_ctx: &units::Context,
    module_input_mapper: F,
) -> Variable<MI, Expr0>
where
    MI: std::fmt::Debug, // TODO: not sure why unwrap_err needs this
    F: Fn(&datamodel::ModuleReference) -> EquationResult<Option<MI>>,
{
    parse_var_with_module_context(
        dimensions,
        v,
        implicit_vars,
        units_ctx,
        module_input_mapper,
        None,
    )
}

/// Like `parse_var` but accepts a set of module variable identifiers from
/// the parent model. When provided, `PREVIOUS(module_var)` in equations will
/// fall through to module expansion instead of compiling to LoadPrev.
pub fn parse_var_with_module_context<MI, F>(
    dimensions: &[datamodel::Dimension],
    v: &datamodel::Variable,
    implicit_vars: &mut Vec<datamodel::Variable>,
    units_ctx: &units::Context,
    module_input_mapper: F,
    module_idents: Option<&HashSet<Ident<Canonical>>>,
) -> Variable<MI, Expr0>
where
    MI: std::fmt::Debug, // TODO: not sure why unwrap_err needs this
    F: Fn(&datamodel::ModuleReference) -> EquationResult<Option<MI>>,
{
    // Create DimensionsContext for dimension mapping lookups in builtin expansion
    let dimensions_ctx = DimensionsContext::from(dimensions);

    let mut parse_and_lower_eqn = |ident: &str,
                                   eqn: &datamodel::Equation,
                                   is_initial: bool,
                                   active_initial: Option<&str>|
     -> (Option<Ast<Expr0>>, Vec<EquationError>) {
        let (ast, mut errors) = parse_equation(eqn, dimensions, is_initial, active_initial);
        let ast = match ast {
            Some(ast) => {
                match instantiate_implicit_modules(ident, ast, Some(&dimensions_ctx), module_idents)
                {
                    Ok((ast, mut new_vars)) => {
                        implicit_vars.append(&mut new_vars);
                        Some(ast)
                    }
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                }
            }
            None => {
                if errors.is_empty() && !is_initial && !v.can_be_module_input() {
                    errors.push(EquationError {
                        start: 0,
                        end: 0,
                        code: ErrorCode::EmptyEquation,
                    })
                }
                None
            }
        };

        (ast, errors)
    };
    match v {
        datamodel::Variable::Stock(v) => {
            let ident = v.ident.clone();

            // TODO: should is_intial be true here?
            let (ast, errors) = parse_and_lower_eqn(&ident, &v.equation, false, None);

            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };
            Variable::Stock {
                ident: Ident::new(&ident),
                init_ast: ast,
                eqn: Some(v.equation.clone()),
                units,
                inflows: v.inflows.iter().map(|i| Ident::new(i)).collect(),
                outflows: v.outflows.iter().map(|o| Ident::new(o)).collect(),
                non_negative: v.compat.non_negative,
                errors,
                unit_errors,
            }
        }
        datamodel::Variable::Flow(v) => {
            let ident = Ident::new(&v.ident);

            let (ast, mut errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, false, None);
            let (init_ast, init_errors) = parse_and_lower_eqn(
                ident.as_str(),
                &v.equation,
                true,
                v.compat.active_initial.as_deref(),
            );
            errors.extend(init_errors);

            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };
            let (tables, table_errors) = build_tables(&v.gf, &v.equation);
            errors.extend(table_errors);
            Variable::Var {
                ident,
                ast,
                init_ast,
                eqn: Some(v.equation.clone()),
                units,
                tables,
                is_flow: true,
                is_table_only: false,
                non_negative: v.compat.non_negative,
                errors,
                unit_errors,
            }
        }
        datamodel::Variable::Aux(v) => {
            let ident = Ident::new(&v.ident);

            let (ast, mut errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, false, None);
            let (init_ast, init_errors) = parse_and_lower_eqn(
                ident.as_str(),
                &v.equation,
                true,
                v.compat.active_initial.as_deref(),
            );
            errors.extend(init_errors);

            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };
            let (tables, table_errors) = build_tables(&v.gf, &v.equation);
            errors.extend(table_errors);
            Variable::Var {
                ident,
                ast,
                init_ast,
                eqn: Some(v.equation.clone()),
                units,
                tables,
                is_flow: false,
                is_table_only: false,
                non_negative: false,
                errors,
                unit_errors,
            }
        }
        datamodel::Variable::Module(v) => {
            let ident = Ident::new(&v.ident);
            let inputs = v.references.iter().map(module_input_mapper);
            let (inputs, errors): (Vec<_>, Vec<_>) = inputs.partition(EquationResult::is_ok);
            let inputs: Vec<MI> = inputs.into_iter().flat_map(|i| i.unwrap()).collect();
            let errors: Vec<EquationError> = errors.into_iter().map(|e| e.unwrap_err()).collect();
            let mut unit_errors: Vec<UnitError> = vec![];
            let units = match parse_units(units_ctx, v.units.as_deref()) {
                Ok(units) => units,
                Err(errors) => {
                    for err in errors.into_iter() {
                        unit_errors.push(err);
                    }
                    None
                }
            };

            Variable::Module {
                model_name: Ident::new(&v.model_name),
                ident,
                units,
                inputs,
                errors,
                unit_errors,
            }
        }
    }
}

/// Result of classifying all dependency categories from a single AST walk.
///
/// Replaces the five separate AST-walking functions that previously computed
/// these categories independently: `identifier_set`, `init_referenced_idents`,
/// `previous_referenced_idents`, `lagged_only_previous_idents_with_module_inputs`,
/// and `init_only_referenced_idents_with_module_inputs`.
pub struct DepClassification {
    /// All referenced identifiers (current + lagged + init-only).
    /// Dimension names are filtered out. Replaces `identifier_set`.
    pub all: HashSet<Ident<Canonical>>,
    /// Idents appearing as direct args to INIT() calls.
    /// Replaces `init_referenced_idents`.
    pub init_referenced: BTreeSet<String>,
    /// Idents appearing as direct args to PREVIOUS() calls.
    /// Replaces `previous_referenced_idents`.
    pub previous_referenced: BTreeSet<String>,
    /// Idents referenced ONLY inside PREVIOUS() -- not outside it.
    /// Replaces `lagged_only_previous_idents_with_module_inputs`.
    pub previous_only: BTreeSet<String>,
    /// Idents referenced ONLY inside INIT() or PREVIOUS() -- not outside either.
    /// Replaces `init_only_referenced_idents_with_module_inputs`.
    pub init_only: BTreeSet<String>,
}

/// Unified AST walker that computes all dependency categories in a single pass.
///
/// Maintains two boolean flags (`in_previous`, `in_init`) to track whether the
/// current position is inside a PREVIOUS() or INIT() call. Accumulates identifiers
/// into multiple sets:
///
/// - `all`: every referenced identifier, with dimension names filtered (same as
///   `IdentifierSetVisitor`)
/// - `init_referenced` / `previous_referenced`: direct Var/Subscript args of
///   INIT() / PREVIOUS() calls
/// - `non_previous`: idents seen outside any PREVIOUS() context
/// - `non_init`: idents seen outside both INIT() and PREVIOUS() context
///
/// After walking, derived sets are computed:
/// - `previous_only = previous_referenced - non_previous`
/// - `init_only = init_referenced - non_init`
///
/// The walker preserves `IdentifierSetVisitor`'s behaviors: dimension-name
/// filtering from index expressions, `IsModuleInput` branch selection via
/// `module_inputs`, and `IndexExpr2::Range` endpoint walking.
struct ClassifyVisitor<'a> {
    all: HashSet<Ident<Canonical>>,
    init_referenced: BTreeSet<String>,
    previous_referenced: BTreeSet<String>,
    non_previous: BTreeSet<String>,
    non_init: BTreeSet<String>,
    dimensions: &'a [Dimension],
    module_inputs: Option<&'a BTreeSet<Ident<Canonical>>>,
    in_previous: bool,
    in_init: bool,
}

impl ClassifyVisitor<'_> {
    /// Check if an identifier is a dimension name or element (and should be skipped)
    fn is_dimension_or_element(&self, ident: &str) -> bool {
        for dim in self.dimensions.iter() {
            if ident == &*canonicalize(dim.name()) {
                return true;
            }
            if let Dimension::Named(_, named_dim) = dim
                && named_dim.get_element_index(ident).is_some()
            {
                return true;
            }
        }
        false
    }

    /// Walk an expression, filtering out dimension names/elements
    fn walk_index_expr(&mut self, expr: &Expr2) {
        if let Expr2::Var(arg_ident, _, _) = expr {
            if !self.is_dimension_or_element(arg_ident.as_str()) {
                self.walk(expr);
            }
        } else {
            self.walk(expr)
        }
    }

    fn walk_index(&mut self, e: &IndexExpr2) {
        match e {
            IndexExpr2::Wildcard(_) => {}
            IndexExpr2::StarRange(_, _) => {}
            IndexExpr2::Range(start, end, _) => {
                self.walk_index_expr(start);
                self.walk_index_expr(end);
            }
            IndexExpr2::DimPosition(_, _) => {}
            IndexExpr2::Expr(expr) => {
                self.walk_index_expr(expr);
            }
        }
    }

    /// Record an identifier string into the flag-dependent sets.
    fn record_ident(&mut self, ident_str: &str) {
        if !self.in_previous {
            self.non_previous.insert(ident_str.to_owned());
        }
        // PREVIOUS() context also excludes from non_init, matching the existing
        // behavior of init_only_referenced_idents_with_module_inputs where
        // BuiltinFn::Previous sets in_init=true.
        if !self.in_init && !self.in_previous {
            self.non_init.insert(ident_str.to_owned());
        }
    }

    fn walk(&mut self, e: &Expr2) {
        match e {
            Expr2::Const(_, _, _) => (),
            Expr2::Var(id, _, _) => {
                let is_dimension = self.dimensions.iter().any(|dim| {
                    let canonicalized_dim = canonicalize(dim.name());
                    id.as_str() == &*canonicalized_dim
                });
                if !is_dimension {
                    self.all.insert(id.clone());
                }
                self.record_ident(id.as_str());
            }
            Expr2::App(builtin, _, _) => match builtin {
                BuiltinFn::Previous(arg) => {
                    if let Expr2::Var(ident, _, _) | Expr2::Subscript(ident, _, _, _) = arg.as_ref()
                    {
                        self.previous_referenced.insert(ident.to_string());
                    }

                    let old = self.in_previous;
                    self.in_previous = true;
                    self.walk(arg);
                    self.in_previous = old;
                }
                BuiltinFn::Init(arg) => {
                    if let Expr2::Var(ident, _, _) | Expr2::Subscript(ident, _, _, _) = arg.as_ref()
                    {
                        self.init_referenced.insert(ident.to_string());
                    }
                    let old = self.in_init;
                    self.in_init = true;
                    self.walk(arg);
                    self.in_init = old;
                }
                _ => {
                    walk_builtin_expr(builtin, |contents| match contents {
                        BuiltinContents::Ident(id, _loc) => {
                            self.all.insert(Ident::new(id));
                        }
                        BuiltinContents::Expr(expr) => self.walk(expr),
                    });
                }
            },
            Expr2::Subscript(id, args, _, _) => {
                self.all.insert(id.clone());
                self.record_ident(id.as_str());
                args.iter().for_each(|arg| self.walk_index(arg));
            }
            Expr2::Op2(_, l, r, _, _) => {
                self.walk(l);
                self.walk(r);
            }
            Expr2::Op1(_, l, _, _) => {
                self.walk(l);
            }
            Expr2::If(cond, t, f, _, _) => {
                if let Some(module_inputs) = self.module_inputs
                    && let Expr2::App(BuiltinFn::IsModuleInput(ident, _), _, _) = cond.as_ref()
                {
                    if module_inputs.contains(&*canonicalize(ident)) {
                        self.walk(t);
                    } else {
                        self.walk(f);
                    }
                    return;
                }

                self.walk(cond);
                self.walk(t);
                self.walk(f);
            }
        }
    }
}

/// Classify all dependency categories of an AST in a single walk.
///
/// Returns a `DepClassification` with five sets:
/// - `all`: every referenced identifier (dimension names filtered)
/// - `init_referenced` / `previous_referenced`: direct args of INIT/PREVIOUS calls
/// - `previous_only`: idents referenced ONLY inside PREVIOUS (not outside)
/// - `init_only`: idents referenced ONLY inside INIT or PREVIOUS (not outside either)
///
/// This replaces five separate functions that previously required up to 10 calls
/// per variable. The walker applies `IsModuleInput` branch selection when
/// `module_inputs` is provided, and filters dimension/element names from index
/// expressions.
pub fn classify_dependencies(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> DepClassification {
    let mut visitor = ClassifyVisitor {
        all: HashSet::new(),
        init_referenced: BTreeSet::new(),
        previous_referenced: BTreeSet::new(),
        non_previous: BTreeSet::new(),
        non_init: BTreeSet::new(),
        dimensions,
        module_inputs,
        in_previous: false,
        in_init: false,
    };
    match ast {
        Ast::Scalar(expr) => visitor.walk(expr),
        Ast::ApplyToAll(_, expr) => visitor.walk(expr),
        Ast::Arrayed(_, elements, default_expr, _) => {
            for expr in elements.values() {
                visitor.walk(expr);
            }
            if let Some(default_expr) = default_expr {
                visitor.walk(default_expr);
            }
        }
    }
    let previous_only = visitor
        .previous_referenced
        .difference(&visitor.non_previous)
        .cloned()
        .collect();
    let init_only = visitor
        .init_referenced
        .difference(&visitor.non_init)
        .cloned()
        .collect();
    DepClassification {
        all: visitor.all,
        init_referenced: visitor.init_referenced,
        previous_referenced: visitor.previous_referenced,
        previous_only,
        init_only,
    }
}

pub fn identifier_set(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> HashSet<Ident<Canonical>> {
    classify_dependencies(ast, dimensions, module_inputs).all
}

/// Collect variable identifiers referenced by `INIT(x)` calls in an AST.
///
/// These are not same-step dependencies, but they must be included in the
/// initials runlist so INIT can read their captured t=0 values.
pub fn init_referenced_idents(ast: &Ast<Expr2>) -> BTreeSet<String> {
    classify_dependencies(ast, &[], None).init_referenced
}

/// Collect variable identifiers referenced by `PREVIOUS(x)` calls in an AST.
///
/// These identifiers are lagged dependencies (t-1), not same-step edges.
pub fn previous_referenced_idents(ast: &Ast<Expr2>) -> BTreeSet<String> {
    classify_dependencies(ast, &[], None).previous_referenced
}

/// Collect identifiers referenced *only* through PREVIOUS(...) in an AST.
///
/// If an identifier appears both inside and outside PREVIOUS, it is excluded.
pub fn lagged_only_previous_idents_with_module_inputs(
    ast: &Ast<Expr2>,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> BTreeSet<String> {
    classify_dependencies(ast, &[], module_inputs).previous_only
}

/// Collect identifiers referenced *only* through INIT(...) in an AST.
///
/// If an identifier appears both inside and outside INIT, it is excluded.
pub fn init_only_referenced_idents_with_module_inputs(
    ast: &Ast<Expr2>,
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> BTreeSet<String> {
    classify_dependencies(ast, &[], module_inputs).init_only
}

#[test]
fn test_identifier_sets() {
    let cases: &[(&str, &[&str])] = &[
        ("if isModuleInput(input) then b else c", &["b"]),
        ("if a then b else c", &["a", "b", "c"]),
        ("lookup(b, c)", &["b", "c"]),
        ("-(a)", &["a"]),
        ("if a = 1 then -c else lookup(c,b)", &["a", "b", "c"]),
        ("if a.d then b else c", &["a·d", "b", "c"]),
        ("if \"a.d\" then b else c", &["a.d", "b", "c"]),
        ("g[foo]", &["g"]),
    ];

    let dimensions: Vec<Dimension> = vec![Dimension::from(datamodel::Dimension::named(
        "dim1".to_string(),
        vec!["foo".to_owned()],
    ))];

    let module_inputs: &[ModuleInput] = &[ModuleInput {
        src: Ident::new("whatever"),
        dst: Ident::new("input"),
    }];

    use crate::ast::lower_ast;

    for (eqn, id_list) in cases.iter() {
        let (ast, err) = parse_equation(
            &datamodel::Equation::Scalar((*eqn).to_owned()),
            &[],
            false,
            None,
        );
        assert_eq!(err.len(), 0);
        assert!(ast.is_some());
        let scope = ScopeStage0 {
            models: &Default::default(),
            dimensions: &Default::default(),
            model_name: "test_model",
        };
        let ast = lower_ast(&scope, ast.unwrap()).unwrap();
        let id_set_expected: HashSet<Ident<Canonical>> = id_list
            .iter()
            .map(|s| {
                // If the test expectation already contains a middle dot, use it directly
                // Otherwise canonicalize it
                if s.contains('·') {
                    Ident::<Canonical>::from_unchecked(s.to_string())
                } else {
                    // For test expectations like "a.d", we treat them as already canonical
                    // (as they would be after parsing a quoted identifier)
                    Ident::<Canonical>::from_unchecked(s.to_string())
                }
            })
            .collect();
        let module_input_names = module_inputs.iter().map(|mi| mi.dst.clone()).collect();
        let id_set_test = identifier_set(&ast, &dimensions, Some(&module_input_names));
        if id_set_expected != id_set_test {
            eprintln!("Test case failed: {eqn}");
            eprintln!("Expected: {id_set_expected:?}");
            eprintln!("Got: {id_set_test:?}");
        }
        assert_eq!(id_set_expected, id_set_test);
    }
}

#[test]
fn test_init_only_referenced_idents() {
    use crate::ast::lower_ast;

    let cases: &[(&str, &[&str])] = &[
        ("INIT(b)", &["b"]),
        ("INIT(b) + b", &[]),
        ("PREVIOUS(b) + INIT(b)", &["b"]),
        ("INIT(m.out1) + m.out2", &["m·out1"]),
    ];

    for (eqn, expected) in cases {
        let (ast, err) = parse_equation(
            &datamodel::Equation::Scalar((*eqn).to_owned()),
            &[],
            false,
            None,
        );
        assert!(err.is_empty());
        let scope = ScopeStage0 {
            models: &Default::default(),
            dimensions: &Default::default(),
            model_name: "test_model",
        };
        let lowered = lower_ast(&scope, ast.expect("failed to parse equation")).unwrap();
        let got = init_only_referenced_idents_with_module_inputs(&lowered, None);
        let expected: BTreeSet<String> = expected.iter().map(|s| s.to_string()).collect();
        assert_eq!(expected, got, "eqn={eqn}");
    }
}

#[test]
fn test_range_end_expressions_are_walked_in_init_previous_helpers() {
    let loc = Loc::new(0, 1);
    let const_one = Expr2::Const("1".to_string(), 1.0, loc);

    let prev_range_ast = Ast::Scalar(Expr2::Subscript(
        Ident::new("arr"),
        vec![IndexExpr2::Range(
            const_one.clone(),
            Expr2::App(
                BuiltinFn::Previous(Box::new(Expr2::Var(Ident::new("lagged"), None, loc))),
                None,
                loc,
            ),
            loc,
        )],
        None,
        loc,
    ));

    let previous_refs = previous_referenced_idents(&prev_range_ast);
    assert!(
        previous_refs.contains("lagged"),
        "range end should contribute PREVIOUS references"
    );

    let lagged_only = lagged_only_previous_idents_with_module_inputs(&prev_range_ast, None);
    assert!(
        lagged_only.contains("lagged"),
        "range end should contribute PREVIOUS-only references"
    );

    let init_range_ast = Ast::Scalar(Expr2::Subscript(
        Ident::new("arr"),
        vec![IndexExpr2::Range(
            const_one,
            Expr2::App(
                BuiltinFn::Init(Box::new(Expr2::Var(Ident::new("seed"), None, loc))),
                None,
                loc,
            ),
            loc,
        )],
        None,
        loc,
    ));

    let init_refs = init_referenced_idents(&init_range_ast);
    assert!(
        init_refs.contains("seed"),
        "range end should contribute INIT references"
    );

    let init_only = init_only_referenced_idents_with_module_inputs(&init_range_ast, None);
    assert!(
        init_only.contains("seed"),
        "range end should contribute INIT-only references"
    );
}

#[test]
fn test_parse_equation_arrayed_preserves_default_expression() {
    let dimensions = vec![datamodel::Dimension::named(
        "dim".to_string(),
        vec!["a".to_string(), "b".to_string()],
    )];
    let equation = datamodel::Equation::Arrayed(
        vec!["dim".to_string()],
        vec![("a".to_string(), "1".to_string(), None, None)],
        Some("2 + 3".to_string()),
    );

    let (ast, errors) = parse_equation(&equation, &dimensions, false, None);
    assert!(errors.is_empty(), "arrayed parse should not emit errors");

    let Some(Ast::Arrayed(_, _, default_expr, apply_default_to_missing)) = ast else {
        panic!("expected arrayed AST");
    };
    assert!(
        default_expr.is_some(),
        "arrayed default equation should be preserved in AST lowering"
    );
    assert!(apply_default_to_missing);
}

#[test]
fn test_parse_equation_arrayed_applies_default_when_element_matches_default() {
    // Sparse array like {a=7, b=10, default=7}: element "a" matches the default,
    // but missing element "c" should still get the default 7, not 0.
    let dimensions = vec![datamodel::Dimension::named(
        "dim".to_string(),
        vec!["a".to_string(), "b".to_string(), "c".to_string()],
    )];
    let equation = datamodel::Equation::Arrayed(
        vec!["dim".to_string()],
        vec![
            ("a".to_string(), "7".to_string(), None, None),
            ("b".to_string(), "10".to_string(), None, None),
        ],
        Some("7".to_string()),
    );

    let (ast, errors) = parse_equation(&equation, &dimensions, false, None);
    assert!(errors.is_empty(), "arrayed parse should not emit errors");

    let Some(Ast::Arrayed(_, _, default_expr, apply_default_to_missing)) = ast else {
        panic!("expected arrayed AST");
    };
    assert!(default_expr.is_some());
    assert!(
        apply_default_to_missing,
        "defaults must apply to missing elements even when an explicit element matches the default"
    );
}

#[test]
fn test_tables() {
    use crate::common::canonicalize;
    let input = datamodel::Variable::Aux(datamodel::Aux {
        ident: canonicalize("lookup function table").into_owned(),
        equation: datamodel::Equation::Scalar("0".to_string()),
        documentation: "".to_string(),
        units: None,
        gf: Some(datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_scale: datamodel::GraphicalFunctionScale {
                min: 0.0,
                max: 45.0,
            },
            y_scale: datamodel::GraphicalFunctionScale {
                min: -1.0,
                max: 1.0,
            },
            x_points: None,
            y_points: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
        }),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    });

    let expected = Variable::Var {
        ident: Ident::new("lookup_function_table"),
        ast: Some(Ast::Scalar(Expr0::Const(
            "0".to_string(),
            0.0,
            Loc::new(0, 1),
        ))),
        init_ast: None,
        eqn: Some(datamodel::Equation::Scalar("0".to_string())),
        units: None,
        tables: vec![Table {
            x: vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0],
            y: vec![0.0, 0.0, 1.0, 1.0, 0.0, 0.0, -1.0, -1.0, 0.0, 0.0],
            x_range: datamodel::GraphicalFunctionScale {
                min: 0.0,
                max: 45.0,
            },
            y_range: datamodel::GraphicalFunctionScale {
                min: -1.0,
                max: 1.0,
            },
        }],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    };

    if let Variable::Var { tables, .. } = &expected {
        assert!(!tables.is_empty());
        assert_eq!(tables[0].x.len(), tables[0].y.len());
    } else {
        panic!("expected Var");
    }

    let mut implicit_vars: Vec<datamodel::Variable> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
    let output = parse_var(&[], &input, &mut implicit_vars, &unit_ctx, |mi| {
        Ok(Some(mi.clone()))
    });

    assert_eq!(expected, output);
}
