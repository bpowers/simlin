// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

use indexmap::IndexMap;

#[cfg(test)]
use crate::ast::Loc;
#[cfg(test)]
use crate::ast::LoweringScope;
use crate::ast::{Ast, Expr0, Expr1, Expr2, IndexExpr1, IndexExpr2};
use crate::builtins::{BuiltinContents, BuiltinFn, walk_builtin_expr};
use crate::builtins_visitor::{
    SnapshotIndexFacts, empty_macro_registry, instantiate_implicit_modules,
};
use crate::capture::{ImplicitVar, insert_implicit_var};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, DimensionName, EquationError,
    EquationResult, Ident, canonicalize,
};
use crate::datamodel;
use crate::db::SourceVariableKind;
use crate::diagnostic::DiagnosticError;
use crate::dimensions::{Dimension, DimensionsContext};
use crate::lexer::LexerType;
use crate::module_functions::MacroRegistry;
use crate::units::parse_units;
use crate::{ErrorCode, eqn_err, units};

/// A graphical function's points, as the compiler and the VM read them.
///
/// The `f64`s keep the derived (IEEE) `PartialEq`, so a lookup table holding a
/// NaN y-point makes this -- and every `db::query::ParsedVariableResult` and
/// lowered-variable memo carrying it -- unequal to a bit-identical
/// rebuild, defeating salsa backdating. The XMILE reader admits one, since
/// `f64::from_str` accepts `"NaN"` in a `<ypts>` list unvalidated. Accepted
/// knowingly, on the same terms as the bytecode types: see the "Float equality
/// in this crate" section on [`crate::ast::Literal`].
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleInput {
    // the Variable identifier in the current model we will use for input
    pub src: Ident<Canonical>,
    // the Variable identifier in the module's model we will override
    pub dst: Ident<Canonical>,
}

/// One element of an apply-to-all body, as the context a SCALAR equation is
/// resolved in.
///
/// A per-element helper -- a `PREVIOUS`/`INIT` capture or a hoisted
/// module-call argument minted while its parent's apply-to-all body was
/// expanded for one element -- holds its argument exactly as it was written
/// inside that body. Its storage is one slot, but its body is one element of
/// the parent's equation: `dims` are the parent's declared axes in order and
/// `element` the active element on each. Lowering seeds the same
/// active-element context the parent's own slot is lowered under
/// (`compiler::Var::new`), so `x[State]`, a bare arrayed name, a mapped or
/// subdimension read and a repeated axis resolve through the compiler's rules
/// (`match_axes`, `resolve_mapped_read`) and read what the parent's element
/// reads. Nothing rewrites the body to get there: a rewrite is a second
/// resolution rule, and the two drift exactly where the rules are non-trivial
/// (GH #1035).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq)]
pub struct ElementScope {
    pub dims: Vec<CanonicalDimensionName>,
    pub element: Vec<CanonicalElementName>,
}

/// A variable's per-kind payload: exactly the facts whose meaning depends on
/// what kind of variable this is. Everything a variable has regardless of kind
/// -- its name, its declared units, its source equation, and the two error
/// channels -- lives on [`Variable`] itself, so a transformation that only
/// rewrites equations (`model::lower_variable`) maps over `kind` instead of
/// re-listing every field of every variant.
///
/// `Aux` covers both auxiliaries and flows (`is_flow` says which) because they
/// lower identically: one slot per element holding the value of one equation.
/// A flow is distinguished only by where a stock's integration reads it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub enum VarKind<MI = ModuleInput, E = Expr2> {
    Stock {
        init_ast: Option<Ast<E>>,
        inflows: Vec<Ident<Canonical>>,
        outflows: Vec<Ident<Canonical>>,
        non_negative: bool,
    },
    Aux {
        ast: Option<Ast<E>>,
        init_ast: Option<Ast<E>>,
        tables: Vec<Table>,
        non_negative: bool,
        is_flow: bool,
        is_table_only: bool,
        /// `Some` for a per-element helper, whose scalar `ast` is one element
        /// of its parent's apply-to-all body ([`ElementScope`]); `None` for
        /// every variable a model declares.
        element_scope: Option<ElementScope>,
    },
    Module {
        // the current spec has ident == model name
        model_name: Ident<Canonical>,
        inputs: Vec<MI>,
    },
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct Variable<MI = ModuleInput, E = Expr2> {
    pub ident: Ident<Canonical>,
    pub units: Option<datamodel::UnitMap>,
    /// The variable's source equation, kept for the consumers that reason about
    /// what the modeller WROTE rather than what it lowered to (LTM's arrayed
    /// re-derivation, unit inference's diagnostics). `None` for a module
    /// instance, which has no equation of its own.
    pub eqn: Option<datamodel::Equation>,
    /// How parsing and lowering report a failure on this variable: the typed
    /// errors they raised, context-free (the variable knows neither its model
    /// nor a severity), in emission order. `parse_var` writes the malformed
    /// `<units>` string as a `Unit` entry and the equation's parse errors as
    /// `Equation` entries, `model::lower_variable` appends what `lower_ast`
    /// raises, and the salsa layer turns each into a `Diagnostic` at the one
    /// place it knows the model (`db::var_fragment::explicit_fragment_input`,
    /// `db::fragment_compile::implicit_fragment_input`). A `Unit` entry is
    /// non-fatal; every other entry stops the variable's compilation
    /// ([`Variable::fatal_diagnostics`]).
    pub diagnostics: Vec<DiagnosticError>,
    pub kind: VarKind<MI, E>,
}

/// A model's lowered variables by canonical name, as the unit pass and the
/// LTM describers read them: handles to the per-variable memos
/// (`db::lowered_source_variable`, `db::lowered_implicit_variable`), never a
/// second copy of a lowered tree. Built by `db::model_lowered_variables`.
pub(crate) type LoweredVariableMap = HashMap<Ident<Canonical>, std::sync::Arc<Variable>>;

impl<MI, E> Variable<MI, E> {
    pub fn ident(&self) -> &str {
        self.ident.as_str()
    }

    pub fn canonical_ident(&self) -> &Ident<Canonical> {
        &self.ident
    }

    pub fn ast(&self) -> Option<&Ast<E>> {
        match &self.kind {
            VarKind::Stock {
                init_ast: Some(ast),
                ..
            }
            | VarKind::Aux { ast: Some(ast), .. } => Some(ast),
            _ => None,
        }
    }

    // returns the init_ast if one exists, otherwise ast()
    pub fn init_ast(&self) -> Option<&Ast<E>> {
        if let VarKind::Aux {
            init_ast: Some(ast),
            ..
        } = &self.kind
        {
            return Some(ast);
        }
        self.ast()
    }

    pub fn get_dimensions(&self) -> Option<&[Dimension]> {
        match self.ast()? {
            Ast::Arrayed(dims, _, _, _) | Ast::ApplyToAll(dims, _) => Some(dims),
            Ast::Scalar(_) => None,
        }
    }

    /// The element this scalar's body is one element of, when it is a
    /// per-element helper's.
    pub fn element_scope(&self) -> Option<&ElementScope> {
        match &self.kind {
            VarKind::Aux { element_scope, .. } => element_scope.as_ref(),
            VarKind::Stock { .. } | VarKind::Module { .. } => None,
        }
    }

    pub fn is_stock(&self) -> bool {
        matches!(self.kind, VarKind::Stock { .. })
    }

    pub fn is_module(&self) -> bool {
        matches!(self.kind, VarKind::Module { .. })
    }

    /// The diagnostics on this variable that stop its compilation: every
    /// entry of [`Variable::diagnostics`] but a malformed `<units>` string,
    /// which is reported and compiled past.
    ///
    /// **`diagnostics` is a live error channel**, not a copy of the salsa
    /// diagnostics: `parse_var` and `model::lower_variable` both produce a
    /// `Variable` and have nowhere else to put a failure, and the salsa path
    /// reads the field to emit them. The read of the LOWERED variable is the
    /// one nothing else covers -- drop it and every `MismatchedDimensions`
    /// disappears (`db::diagnostic_tests::variable_error_fields_are_the_lowering_channel`
    /// is the standing gate). The read of the PARSED variable sees a strict
    /// subset, since `lower_variable` carries the parse entries forward, but
    /// it is where the conveyor/queue driven-flow `EmptyEquation` suppression
    /// applies, so dropping it turns a spec-sanctioned empty equation into a
    /// phantom error (`db::diagnostic_tests`'
    /// `test_conveyor_driven_flow_empty_equation_suppressed` and its two
    /// siblings). So `db::collect_model_diagnostics` is not an ALTERNATIVE
    /// source for these -- it is the same errors, downstream of this field.
    pub fn fatal_diagnostics(&self) -> impl Iterator<Item = &DiagnosticError> {
        self.diagnostics
            .iter()
            .filter(|d| !matches!(d, DiagnosticError::Unit(_)))
    }

    pub fn table(&self) -> Option<&Table> {
        self.tables().first()
    }

    pub fn tables(&self) -> &[Table] {
        match &self.kind {
            VarKind::Aux { tables, .. } => tables,
            VarKind::Stock { .. } | VarKind::Module { .. } => &[],
        }
    }

    pub fn units(&self) -> Option<&datamodel::UnitMap> {
        self.units.as_ref()
    }
}

impl Variable {
    /// A module instance in its lowered form: the instance `ident`, the model
    /// it instantiates, and its resolved input wiring. A module has no
    /// equation of its own, so its lowered form is exactly these three facts;
    /// the fragment constructors build it from the instance's `(src, dst)`
    /// references (`db::build_module_inputs`) without a parse.
    pub(crate) fn module_instance(
        ident: Ident<Canonical>,
        model_name: Ident<Canonical>,
        inputs: Vec<ModuleInput>,
    ) -> Self {
        Variable {
            ident,
            units: None,
            eqn: None,
            diagnostics: vec![],
            kind: VarKind::Module { model_name, inputs },
        }
    }
}

#[allow(clippy::unnecessary_wraps)]
pub(crate) fn parse_table(
    gf: Option<&datamodel::GraphicalFunction>,
) -> EquationResult<Option<Table>> {
    let Some(gf) = gf else {
        return Ok(None);
    };

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

/// Lay out a per-element table list so each element's table lands at the
/// element's flat **declared dimension index** (row-major across all of
/// `dims`), regardless of the order `present` was collected in.
///
/// The runtime selects a per-element graphical-function table by the flat
/// array offset (`vm.rs` `Lookup`/`LookupArray`: `graphical_functions[base_gf +
/// element_offset]`), where `element_offset` is the row-major declared-order
/// index computed from the subscript at compile time (see
/// `compiler::codegen` `extract_table_info`). The `Equation::Arrayed` `elems`
/// Vec, by contrast, can be in any order -- the MDL importer sorts it
/// alphabetically (`mdl/convert/variables.rs`). So a per-element GF table list
/// must be re-keyed by element name -> dimension index here, at lowering time,
/// or every element of a non-alphabetically-declared dimension reads the wrong
/// element's table. This is a one-time compile-time reorder; the VM hot path
/// is unchanged (the opcode carries no element name).
///
/// `dims` are the variable's resolved dimensions, iterated by `SubscriptIterator`
/// in the same row-major order the codegen flat offset assumes; `present` maps
/// each element's comma-joined canonical subscript name to its (already
/// parsed) table. Elements absent from `present` get `empty()` placeholders so
/// `tables[element_offset]` stays aligned (a lookup on an empty table is NaN).
pub(crate) fn reorder_arrayed_element_tables<T>(
    dims: &[Dimension],
    present: &HashMap<CanonicalElementName, T>,
    empty: impl Fn() -> T,
    clone_table: impl Fn(&T) -> T,
) -> Vec<T> {
    crate::dimensions::SubscriptIterator::new(dims)
        .map(|subscripts| {
            let key = CanonicalElementName::from_raw(&subscripts.join(","));
            present.get(&key).map(&clone_table).unwrap_or_else(&empty)
        })
        .collect()
}

/// Build the tables vector from equation and variable-level gf.
/// For arrayed variables with per-element gfs, tables are built from each
/// element and laid out by the element's declared dimension index (via
/// [`reorder_arrayed_element_tables`]), NOT by `elems` Vec position. For scalar
/// variables or arrayed without per-element gfs, uses the variable-level gf.
///
/// `dimensions` is the project/model dimension context; the arrayed
/// equation's dimension names are resolved against it to drive the
/// element-name -> dimension-index reorder.
fn build_tables(
    gf: Option<&datamodel::GraphicalFunction>,
    equation: &datamodel::Equation,
    dimensions: &DimensionsContext,
) -> (Vec<Table>, Vec<EquationError>) {
    let mut errors = Vec::new();

    // Check for per-element gfs in arrayed equation
    if let datamodel::Equation::Arrayed(dim_names, elements, _, _) = equation {
        let has_element_gfs = elements.iter().any(|(_, _, _, gf)| gf.is_some());
        if has_element_gfs {
            // Parse each element's table, keyed by the element's canonical
            // (comma-joined) subscript name. Elements without a GF are simply
            // absent from the map and get an empty placeholder at their slot.
            let mut present: HashMap<CanonicalElementName, Table> = HashMap::new();
            for (subscript, _, _, elem_gf) in elements {
                match parse_table(elem_gf.as_ref()) {
                    Ok(Some(table)) => {
                        present.insert(CanonicalElementName::from_raw(subscript), table);
                    }
                    Ok(None) => {}
                    Err(err) => errors.push(err),
                }
            }

            // Resolve the equation's dimensions so the reorder maps each
            // element name to its row-major declared-order flat offset. If the
            // dimensions cannot be resolved (a separate BadDimensionName error
            // the model already surfaces), fall back to the original
            // Vec-positional layout rather than dropping tables.
            let tables = match get_dimensions(dimensions, dim_names) {
                Ok(dims) => {
                    reorder_arrayed_element_tables(&dims, &present, Table::empty, |t: &Table| {
                        t.clone()
                    })
                }
                Err(_) => elements
                    .iter()
                    .map(|(subscript, _, _, _)| {
                        present
                            .get(&CanonicalElementName::from_raw(subscript))
                            .cloned()
                            .unwrap_or_else(Table::empty)
                    })
                    .collect(),
            };
            return (tables, errors);
        }
    }

    // Fall back to variable-level gf
    let mut tables = Vec::new();
    match parse_table(gf) {
        Ok(Some(table)) => tables.push(table),
        Ok(None) => {}
        Err(err) => errors.push(err),
    }

    (tables, errors)
}

/// An equation string with no functional content: empty/whitespace-only (the
/// canonical lookup-only form) or exactly the MDL lookup sentinel `"0+0"` (a
/// legacy back-compat read-shim -- older serialized models may still carry it).
/// The trimmed comparison mirrors `mdl::writer::is_lookup_only_equation`.
pub(crate) fn is_empty_or_sentinel(equation: &str) -> bool {
    let trimmed = equation.trim();
    trimmed.is_empty() || trimmed == crate::mdl::LOOKUP_SENTINEL
}

/// Whether a variable is a standalone "lookup-only" table: it carries a
/// graphical function and has no functional input expression to feed it. Such
/// a variable is a *table indexed by an explicit input* (`y = table(input)`),
/// not a runtime value-bearing variable: it produces no simulation series and
/// is excluded from every runlist (see [`VarKind::Aux::is_table_only`] and
/// `db::source_var_is_table_only`, which is this predicate over the salsa
/// inputs).
///
/// The rule per equation shape:
///
/// * **Scalar / apply-to-all** -- lookup-only iff the one equation is
///   empty-or-sentinel and a variable-level `gf` is present.
/// * **Arrayed (per-element)** -- lookup-only iff the variable holds a table
///   *somewhere* (a variable-level `gf` or any per-element one) and EVERY
///   element equation, plus the EXCEPT default when there is one, is
///   empty-or-sentinel: a pure per-element table holder.
///
/// The empty-or-sentinel rule mirrors `mdl::writer::is_lookup_only_equation`,
/// so BOTH the canonical empty-equation form (what both the XMILE and MDL
/// importers emit today) and a legacy MDL-sourced one (the `LOOKUP_SENTINEL`
/// `"0+0"` equation alongside a `gf`) are detected.
///
/// Returns `false` for WITH LOOKUP (`var = WITH LOOKUP(input, table)`: tables
/// present but a *real* input equation), for an ordinary aux (a real equation,
/// no tables), and for an empty-RHS aux with no `gf` at all.
pub(crate) fn is_lookup_only(
    eqn: &datamodel::Equation,
    gf: Option<&datamodel::GraphicalFunction>,
) -> bool {
    use crate::datamodel::Equation;
    match eqn {
        Equation::Scalar(s) | Equation::ApplyToAll(_, s) => gf.is_some() && is_empty_or_sentinel(s),
        // The per-element gf is the 4th tuple field
        // `(subscript, equation, gf_equation, gf)`.
        Equation::Arrayed(_, elements, default, _) => {
            let has_tables = gf.is_some() || elements.iter().any(|(_, _, _, gf)| gf.is_some());
            has_tables
                && !elements.is_empty()
                && elements.iter().all(|(_, e, _, _)| is_empty_or_sentinel(e))
                && default.as_deref().map(is_empty_or_sentinel).unwrap_or(true)
        }
    }
}

/// Does `equation` consist of nothing but the NaN literal?
///
/// Decided by LEXING rather than by comparing against the string `"nan"`: the
/// spelling of the literal lives in `lexer::KEYWORDS`, and restating it here
/// would be a second copy of that table to keep in step (the same reason
/// `ast::needs_quoting` reads `lexer::is_reserved_word`, GH #976). Lexing also
/// settles case and surrounding whitespace for free -- both spellings occur in
/// practice, since the MDL importer writes `NAN` for `A FUNCTION OF(...)` while
/// the MDL writer prints a `Const` NaN back as `NaN`.
///
/// "Nothing but" is exactly one token, so a NaN nested in a larger expression
/// (`nan + 0`, `IF x > 0 THEN 1 ELSE nan`) is NOT this -- that is a modeller
/// deliberately using NaN as a sentinel, a different claim with a different
/// remedy. A parenthesized `(nan)` is likewise outside the rule; nothing
/// produces it, and admitting it would mean deciding how far to unwrap.
pub(crate) fn is_nan_literal(equation: &str) -> bool {
    use crate::lexer::{Lexer, LexerType, Token};
    let mut lexer = Lexer::new(equation, LexerType::Equation);
    matches!(lexer.next(), Some(Ok((_, Token::Nan, _)))) && lexer.next().is_none()
}

/// Which of a variable's equation arms are UNFILLED -- carry the NaN literal
/// where a formula belongs (see [`is_nan_literal`]).
///
/// An "arm" is one whole equation: the single formula of a scalar or
/// apply-to-all variable, one `<element>` entry of a per-element arrayed
/// variable, or an arrayed variable's EXCEPT default (the formula for every
/// element with no entry of its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UnfilledArms {
    /// The variable has NO equation anywhere: its scalar/apply-to-all formula is
    /// the NaN literal, or every arm of its arrayed equation is (its EXCEPT
    /// default included, when it has one).
    Whole,
    /// Only part of an arrayed variable is unfilled: these element subscripts,
    /// in declaration order, plus the EXCEPT default when `default` is set.
    Partial {
        elements: Vec<String>,
        default: bool,
    },
}

/// Classify a variable's PARSED equation by which of its arms are unfilled, or
/// `None` when none are.
///
/// Arms rather than whole variables, deliberately. An arrayed variable's
/// elements are separate simulated series, so an unfilled `x[b]` stops `x[b]`'s
/// line on the graph exactly the way an unfilled scalar stops its own, and costs
/// the same backward hunt (`crate::float`). Reporting only the all-arms case
/// would go silent precisely where the model is most confusing -- some lines
/// fine, one stopping -- so a partially-unfilled variable is reported too, and
/// [`UnfilledArms::Partial`] names the arms so the message can point at them.
/// Either way it is at most ONE finding per variable, never one per element.
///
/// # Why this takes the parsed `Ast`, not the `datamodel::Equation`
///
/// Four review findings on this diagnostic were the same mistake, each caught
/// one at a time: an arm shadowed because the others cover the dimension, an arm
/// whose subscript names nothing, an arm a later duplicate overrides, and an arm
/// whose equation is EMPTY. Every one is a gap between the arms AS WRITTEN and
/// the arms the compiler EVALUATES, and the first three were each fixed by
/// re-deriving one more stage of that pipeline by hand. The fourth proved the
/// approach wrong: the hand-derived selection was missing a stage, and missing
/// one silently -- it reported nothing where a slot really was NaN.
///
/// So this no longer re-derives anything. [`parse_equation`] already performs
/// the pipeline, and its `Ast` IS the result: empty and unparseable arms
/// dropped, duplicate canonical subscripts collapsed last-wins, dimensions
/// resolved. The one stage that is not in the `Ast` -- which declared slot takes
/// which arm -- is the `SubscriptIterator` walk below, and it is the compiler's
/// own (`compiler::expand_per_element` looks each combination's key up
/// in this same map and falls to the EXCEPT default only on a miss). Nothing
/// here mirrors a stage that exists elsewhere.
///
/// The consequence for arrayed reporting: element names come out CANONICAL and
/// in row-major declared order, because that is how the map is keyed and how the
/// slots are walked. The as-written spelling is not recoverable from the `Ast`,
/// and asking the datamodel for it would mean re-deriving the last-wins rule to
/// know which spelling survived -- exactly the re-derivation this avoids.
pub(crate) fn unfilled_arms(ast: &Ast<Expr0>) -> Option<UnfilledArms> {
    match ast {
        // One arm covering the whole variable: a scalar formula, or one formula
        // applied to every element.
        Ast::Scalar(expr) | Ast::ApplyToAll(_, expr) => {
            is_nan_constant(expr).then_some(UnfilledArms::Whole)
        }
        Ast::Arrayed(dims, elements, default, apply_default_to_missing) => {
            let mut unfilled: Vec<String> = vec![];
            let mut slots_with_an_arm = 0usize;
            let mut default_is_selected = false;
            for combination in crate::dimensions::SubscriptIterator::new(dims) {
                let key = CanonicalElementName::from_raw(&combination.join(","));
                match elements.get(&key) {
                    Some(expr) => {
                        slots_with_an_arm += 1;
                        if is_nan_constant(expr) {
                            unfilled.push(key.as_str().to_string());
                        }
                    }
                    // No arm names this slot, so its value comes from the EXCEPT
                    // default when one is live and from the compiler's silent
                    // `0` otherwise.
                    None => default_is_selected = true,
                }
            }
            let default_unfilled = default_is_selected
                && *apply_default_to_missing
                && default.as_ref().is_some_and(is_nan_constant);
            // The silent `0` is finite, so a slot that falls to it is NOT an
            // unfilled equation. (It is its own reportable shape, and a
            // deliberately separate one: GH #905.)
            let slots_past_the_arms_are_nan = !default_is_selected || default_unfilled;

            if unfilled.is_empty() && !default_unfilled {
                None
            } else if unfilled.len() == slots_with_an_arm && slots_past_the_arms_are_nan {
                // Every slot the variable has evaluates to NaN.
                Some(UnfilledArms::Whole)
            } else {
                Some(UnfilledArms::Partial {
                    elements: unfilled,
                    default: default_unfilled,
                })
            }
        }
    }
}

/// Is `expr` exactly a NaN constant -- the whole formula, not a NaN inside one?
///
/// A root-level `Expr0::Const` IS the whole equation: the parser builds no node
/// for parentheses, so this is the parsed twin of [`is_nan_literal`]'s
/// single-token rule and agrees with it on every text that reaches here.
fn is_nan_constant(expr: &Expr0) -> bool {
    matches!(expr, Expr0::Const(_, literal, _) if literal.value().is_nan())
}

/// [`is_nan_constant`] for the decision table, which must compute a fixture's
/// cell by the same rule the classifier uses.
#[cfg(test)]
pub(crate) fn is_nan_constant_for_test(expr: &Expr0) -> bool {
    is_nan_constant(expr)
}

/// Could `equation` possibly produce an unfilled-equation finding?
///
/// A cheap superset test, and the ONLY thing an ordinary variable pays. Every
/// finding names either an arm or the EXCEPT default, so if no arm text and no
/// default text is a lone NaN there is nothing to report and the caller can skip
/// the slot walk entirely -- which for an arrayed variable means skipping a
/// Cartesian product over its declared elements.
///
/// That matters because `db::diagnostic::model_all_diagnostics` runs on the
/// interactive path: without this gate every arrayed variable in a model paid an
/// O(slot-count) allocation on every keystroke, for a warning almost none of
/// them will ever emit.
///
/// It must stay a SUPERSET of what [`unfilled_arms`] reports, which it is by
/// construction: that function only ever looks at these same texts.
///
/// # `nan_names_a_variable`: declining to make an undecidable claim
///
/// When the model declares a variable actually NAMED `nan`, the stored text
/// `NAN` has two readings and nothing here can tell them apart, so this returns
/// `false` and the variable is not reported at all.
///
/// The ambiguity is one we chose. The MDL importer quotes every keyword-shaped
/// variable reference EXCEPT `nan` (`mdl::xmile_compat`'s `quote_reference`),
/// because quoting it would bind Vensim's `A FUNCTION OF(...)` placeholder --
/// which we store as the text `NAN` -- to any like-named variable, and a
/// round-tripped model would then compute a value for a variable that has none.
/// That trade is deliberate and is pinned by
/// `keyword_ident_tests::a_bare_nan_reference_in_mdl_is_still_the_literal`,
/// which records the residual it leaves: `b = nan` referring to a declared `nan`
/// still reads as the literal. So a model containing both shapes stores them
/// identically, and `b = nan` -- a formula the modeller really did write --
/// looked exactly like a placeholder they never filled in.
///
/// Silence is the right answer rather than a guess in either direction. This
/// warning's whole value is that a practitioner can trust it and skip the
/// backward hunt (`crate::float`), so a warning that MIGHT be false is worse
/// than one that is absent. Note this is not the declared-name resolution rule
/// batch 1 rejected: that one resolved the ambiguity in favour of one reading
/// and would have shipped a wrong VALUE. This ships no claim.
///
/// Scoped to the ambiguity, not to the model: the flag only ever suppresses
/// equations whose text IS the bare literal, which is the only text with two
/// readings. Every other diagnostic in the pass is untouched, and in a model
/// with no variable named `nan` -- every ordinary model -- nothing changes.
pub(crate) fn may_have_unfilled_arms(
    equation: &datamodel::Equation,
    nan_names_a_variable: bool,
) -> bool {
    if nan_names_a_variable {
        return false;
    }
    use crate::datamodel::Equation;
    match equation {
        Equation::Scalar(s) | Equation::ApplyToAll(_, s) => is_nan_literal(s),
        Equation::Arrayed(_, elements, default, _) => {
            elements.iter().any(|(_, eqn, _, _)| is_nan_literal(eqn))
                || default.as_deref().is_some_and(is_nan_literal)
        }
    }
}

#[cfg(test)]
mod is_lookup_only_tests {
    use super::*;

    /// A minimal graphical function -- the only thing the predicate asks of a
    /// `gf` is whether one is there.
    fn gf() -> datamodel::GraphicalFunction {
        datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: None,
            x_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
            y_points: vec![0.0, 1.0],
        }
    }

    fn arrayed(elements: &[(&str, &str, bool)], default: Option<&str>) -> datamodel::Equation {
        datamodel::Equation::Arrayed(
            vec!["d".to_owned()],
            elements
                .iter()
                .map(|(sub, eqn, has_gf)| {
                    ((*sub).to_owned(), (*eqn).to_owned(), None, has_gf.then(gf))
                })
                .collect(),
            default.map(str::to_owned),
            default.is_some(),
        )
    }

    /// Every combination of (equation shape) x (equation content) x (where the
    /// table lives), derived from `datamodel::Equation`'s three variants and
    /// the two places a graphical function can sit (the variable, an element).
    /// One row per cell; the expectation is the documented rule, not a
    /// restatement of the code.
    #[test]
    fn is_lookup_only_covers_every_equation_shape() {
        use datamodel::Equation::{ApplyToAll, Scalar};
        let sentinel = crate::mdl::LOOKUP_SENTINEL;
        let cases: Vec<(&str, datamodel::Equation, bool, bool)> = vec![
            // -- scalar --
            ("scalar empty + gf", Scalar(String::new()), true, true),
            (
                "scalar whitespace + gf",
                Scalar("   ".to_owned()),
                true,
                true,
            ),
            (
                "scalar sentinel + gf (legacy MDL)",
                Scalar(sentinel.to_owned()),
                true,
                true,
            ),
            // WITH LOOKUP: a real input expression beside the table.
            (
                "scalar real equation + gf",
                Scalar("some_input".to_owned()),
                true,
                false,
            ),
            // An ordinary aux, and an empty-RHS aux: no table, so never this.
            (
                "scalar real equation, no gf",
                Scalar("3 * x + 1".to_owned()),
                false,
                false,
            ),
            ("scalar empty, no gf", Scalar(String::new()), false, false),
            (
                "scalar sentinel, no gf",
                Scalar(sentinel.to_owned()),
                false,
                false,
            ),
            // -- apply-to-all: one equation, same rule as scalar --
            (
                "a2a empty + gf",
                ApplyToAll(vec!["d".to_owned()], String::new()),
                true,
                true,
            ),
            (
                "a2a real equation + gf",
                ApplyToAll(vec!["d".to_owned()], "x".to_owned()),
                true,
                false,
            ),
            (
                "a2a empty, no gf",
                ApplyToAll(vec!["d".to_owned()], String::new()),
                false,
                false,
            ),
            // -- arrayed: every arm must be empty, and a table must exist --
            (
                "arrayed all empty, per-element gfs",
                arrayed(&[("a", "", true), ("b", "", true)], None),
                false,
                true,
            ),
            (
                "arrayed all empty, variable-level gf",
                arrayed(&[("a", "", false), ("b", "", false)], None),
                true,
                true,
            ),
            (
                "arrayed all sentinel, per-element gfs",
                arrayed(&[("a", sentinel, true)], None),
                false,
                true,
            ),
            (
                "arrayed one arm has an equation",
                arrayed(&[("a", "", true), ("b", "x", true)], None),
                false,
                false,
            ),
            (
                "arrayed all empty, no table anywhere",
                arrayed(&[("a", "", false)], None),
                false,
                false,
            ),
            (
                "arrayed all empty, EXCEPT default empty",
                arrayed(&[("a", "", true)], Some("")),
                false,
                true,
            ),
            (
                "arrayed all empty, EXCEPT default has an equation",
                arrayed(&[("a", "", true)], Some("x")),
                false,
                false,
            ),
            (
                "arrayed with no elements at all",
                arrayed(&[], None),
                true,
                false,
            ),
        ];
        for (label, eqn, has_var_gf, expected) in cases {
            let table = gf();
            let var_gf = has_var_gf.then_some(&table);
            assert_eq!(
                expected,
                is_lookup_only(&eqn, var_gf),
                "is_lookup_only disagreed on '{label}'"
            );
        }
    }
}

pub(crate) fn get_dimensions(
    dimensions: &DimensionsContext,
    names: &[DimensionName],
) -> Result<Vec<Dimension>, EquationError> {
    names
        .iter()
        .map(|name| -> Result<Dimension, EquationError> {
            // Match by canonical name, not raw string equality: a dimension's
            // identity is its canonical name (the context is keyed by it, so
            // two distinct dimensions can never canonicalize to the same
            // string). A synthesized `Equation::ApplyToAll` whose dimension
            // names came from `print_eqn` carries CANONICAL names (`hfc_type`),
            // which must still resolve against a dimension declared with
            // original casing/spacing (`HFC type` -> `HFC_type`); a raw `==`
            // check rejected it as `BadDimensionName` (a synthesized apply-to-all
            // capture over C-LEARN's capitalized dimensions, GH #541).
            // Importer-produced equations already match exactly,
            // so canonical matching is a strict superset.
            //
            // Taking the already-built `DimensionsContext` rather than the raw
            // `&[datamodel::Dimension]` turns this from a linear scan that
            // re-canonicalized every declared dimension name per lookup (and
            // then rebuilt the matched `Dimension` from scratch, re-interning
            // its every element name) into one canonicalize plus a hash probe.
            match dimensions.get_by_raw_name(name) {
                Some(dim) => Ok(dim.clone()),
                None => eqn_err!(
                    BadDimensionName,
                    0,
                    0,
                    format!("'{name}' is not a declared dimension")
                ),
            }
        })
        .collect()
}

fn parse_equation(
    eqn: &datamodel::Equation,
    dimensions: &DimensionsContext,
    is_initial: bool,
    active_initial: Option<&str>,
) -> (Option<Ast<Expr0>>, Vec<EquationError>) {
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
        datamodel::Equation::Arrayed(dimension_names, elements, default_eq, has_except_default) => {
            let mut errors: Vec<EquationError> = vec![];
            let apply_default_to_missing = *has_except_default;
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

/// The fields the parser reads from a variable, borrowed from whichever
/// representation the caller holds.
///
/// Two producers, one consumer. The salsa path builds this directly over
/// `SourceVariable`'s split input fields (`db::input::variable_source`), which
/// is why nothing between the salsa inputs and the parse has to re-assemble --
/// and deep-clone -- a kind-tagged `datamodel::Variable` per parse. The
/// non-salsa paths (the unit check's transient conveyor parameters, and every
/// path that parses a synthesized `datamodel::Variable`) come through the
/// `From<&datamodel::Variable>` impl below.
///
/// `equation` is a `Cow` for one producer-specific rewrite: a conveyor stock's
/// `<eqn>` may be a §7.2 explicit init list (`"100, 200, 300"`), which is not a
/// scalar expression, so the salsa producer substitutes the constant raw-sum
/// placeholder `conveyor_compile::explicit_init_list` computes. That
/// substitution and the empty stand-in for a variable with no equation at all
/// (a module instance) are the only owned values either producer builds; every
/// other field, on both producers, is a plain borrow.
pub struct VariableSource<'a> {
    pub ident: &'a str,
    pub equation: std::borrow::Cow<'a, datamodel::Equation>,
    pub kind: SourceVariableKind,
    pub units: Option<&'a str>,
    pub gf: Option<&'a datamodel::GraphicalFunction>,
    pub inflows: &'a [String],
    pub outflows: &'a [String],
    pub module_refs: &'a [datamodel::ModuleReference],
    /// The model a `Module` instance instantiates; empty for every other kind.
    pub model_name: &'a str,
    pub non_negative: bool,
    pub can_be_module_input: bool,
    /// `compat.active_initial`: an importer-supplied separate initial-phase
    /// equation (Vensim's `ACTIVE INITIAL`), read only on the initial pass.
    pub active_initial: Option<&'a str>,
}

impl VariableSource<'_> {
    /// Whether this is a standalone lookup-only table -- see [`is_lookup_only`],
    /// the one owner of the rule.
    pub fn is_lookup_only(&self) -> bool {
        is_lookup_only(&self.equation, self.gf)
    }
}

impl<'a> From<&'a datamodel::Variable> for VariableSource<'a> {
    fn from(v: &'a datamodel::Variable) -> Self {
        const NO_NAMES: &[String] = &[];
        const NO_REFS: &[datamodel::ModuleReference] = &[];
        let (inflows, outflows) = match v {
            datamodel::Variable::Stock(s) => (s.inflows.as_slice(), s.outflows.as_slice()),
            _ => (NO_NAMES, NO_NAMES),
        };
        let (module_refs, model_name) = match v {
            datamodel::Variable::Module(m) => (m.references.as_slice(), m.model_name.as_str()),
            _ => (NO_REFS, ""),
        };
        let gf = match v {
            datamodel::Variable::Flow(f) => f.gf.as_ref(),
            datamodel::Variable::Aux(a) => a.gf.as_ref(),
            _ => None,
        };
        // An aux's `non_negative` is deliberately dropped: only a stock and a
        // flow have the flag, and `db::sync` stores the same `false` for an aux
        // on the salsa input, so both producers agree.
        let non_negative = match v {
            datamodel::Variable::Stock(s) => s.compat.non_negative,
            datamodel::Variable::Flow(f) => f.compat.non_negative,
            _ => false,
        };
        let compat = match v {
            datamodel::Variable::Stock(s) => &s.compat,
            datamodel::Variable::Flow(f) => &f.compat,
            datamodel::Variable::Aux(a) => &a.compat,
            datamodel::Variable::Module(m) => &m.compat,
        };
        VariableSource {
            ident: v.get_ident(),
            // Borrowed, like every other field: only the salsa producer's
            // conveyor rewrite owns. A module has no equation at all, and the
            // empty scalar stand-in is the one value with nothing to borrow
            // from.
            equation: v
                .get_equation()
                .map(std::borrow::Cow::Borrowed)
                .unwrap_or_else(|| {
                    std::borrow::Cow::Owned(datamodel::Equation::Scalar(String::new()))
                }),
            kind: SourceVariableKind::from_datamodel_variable(v),
            units: v.get_units().map(String::as_str),
            gf,
            inflows,
            outflows,
            module_refs,
            model_name,
            non_negative,
            can_be_module_input: v.can_be_module_input(),
            active_initial: compat.active_initial.as_deref(),
        }
    }
}

/// Everything a parse reads BESIDE the variable itself: the project-global
/// contexts (dimensions, units, the macro registry, the enclosing macro body)
/// plus the one model-level question the parse answers itself.
///
/// Nothing here says which variables of the owning model are module
/// instances, module-call auxes or bound input ports. Whether a
/// `PREVIOUS`/`INIT` argument addresses snapshot storage is decided by the
/// argument's own spelling here and by its dependency shape at lowering
/// (`compiler::context`); the one fact the parse asks of the model is whether
/// an identifier subscript of such an argument pins a declared element
/// ([`SnapshotIndexFacts`]), asked per name, so a parse is a function of
/// `(variable, project)` and no edit to a sibling variable re-keys it.
///
/// A struct rather than a parameter list because every field is optional
/// context that most callers do not supply -- [`ParseContext::new`] is the
/// "no project context" parse the non-salsa paths use.
pub struct ParseContext<'a> {
    pub dimensions: &'a DimensionsContext,
    pub units_ctx: &'a units::Context,
    /// What a `PREVIOUS`/`INIT` subscript may ask the owning model about an
    /// identifier index (see `BuiltinVisitor::index_is_static`).
    pub snapshot_index: SnapshotIndexFacts<'a>,
    /// The per-project macro registry. When provided, a call resolving to a
    /// project macro expands into a synthetic module variable (and shadows an
    /// identically named builtin/stdlib func). `None` means "no project
    /// macros", an empty registry.
    pub macro_registry: Option<&'a MacroRegistry>,
    /// The owning model's name when the variable is a macro-marked model's
    /// body variable; `None` for ordinary variables. Threaded to
    /// `instantiate_implicit_modules` for the #554 same-named-opcode-intrinsic
    /// exception (a macro body's renamed `init`/`previous` builtin must
    /// resolve to the intrinsic, not recurse into the like-named macro).
    pub enclosing_model: Option<&'a str>,
}

impl<'a> ParseContext<'a> {
    /// A parse with no model-level context: no owning model to ask about a
    /// subscript index, no project macros, and no enclosing macro body.
    pub fn new(dimensions: &'a DimensionsContext, units_ctx: &'a units::Context) -> Self {
        ParseContext {
            dimensions,
            units_ctx,
            snapshot_index: SnapshotIndexFacts::NoModel,
            macro_registry: None,
            enclosing_model: None,
        }
    }
}

/// Parse one variable's equations into a `Variable<MI, Expr0>`, appending any
/// implicit helper variables the `PREVIOUS`/`INIT`/stdlib-call expansion
/// synthesizes to `implicit_vars`.
pub fn parse_var<'a, MI, F>(
    ctx: &ParseContext<'_>,
    v: impl Into<VariableSource<'a>>,
    implicit_vars: &mut Vec<ImplicitVar>,
    module_input_mapper: F,
) -> Variable<MI, Expr0>
where
    MI: std::fmt::Debug, // TODO: not sure why unwrap_err needs this
    F: Fn(&datamodel::ModuleReference) -> EquationResult<Option<MI>>,
{
    let v: VariableSource<'a> = v.into();
    let dimensions = ctx.dimensions;

    // The helpers THIS call contributes, filed by name. Seeded empty rather
    // than from the caller's vector, which is deliberate on both counts:
    //
    // * only helpers of the SAME parent can collide, since a synthesized name
    //   embeds its parent's ident (`$⁚{parent}⁚{n}⁚…`) and two parents sharing a
    //   canonical name is already a `DuplicateVariable` model error (GH #885);
    // * a caller that parses a whole model through one vector would otherwise
    //   make each variable pay for every helper minted before it -- quadratic
    //   in the model.
    let mut helpers: IndexMap<Ident<Canonical>, ImplicitVar> = IndexMap::new();

    // Resolve the default at use (an empty `'static` registry) rather than
    // rebinding here -- unifying a borrowed `Some(&'a _)` with the
    // `&'static` empty default before the parse closure captures it would
    // force the closure (and hence `'a`) to `'static`.
    let mut parse_and_lower_eqn = |ident: &str,
                                   eqn: &datamodel::Equation,
                                   is_initial: bool,
                                   active_initial: Option<&str>|
     -> (Option<Ast<Expr0>>, Vec<EquationError>) {
        let (ast, mut errors) = parse_equation(eqn, dimensions, is_initial, active_initial);
        let ast = match ast {
            Some(ast) => {
                // The closure (not the bare `fn` pointer) lets the
                // `&'static` empty default coerce to the parameter's
                // borrow lifetime instead of forcing it to `'static`.
                let registry = ctx.macro_registry.unwrap_or_else(|| empty_macro_registry());
                match instantiate_implicit_modules(
                    ident,
                    ast,
                    Some(dimensions),
                    ctx.snapshot_index,
                    registry,
                    ctx.enclosing_model,
                ) {
                    Ok((ast, new_vars)) => {
                        // MERGE rather than append. This closure runs twice per
                        // variable -- once for the dt phase, once for the
                        // initial phase -- and both passes name their helpers
                        // from a counter that restarts at zero, so the two can
                        // mint the SAME name for different bodies whenever the
                        // initial pass reads a different equation (an `Arrayed`
                        // element's own init equation, or `compat.active_initial`).
                        // Downstream, `model_implicit_var_info` is name-keyed and
                        // `compute_layout` allocates one slot per name, so a
                        // silent last-wins would run one phase's helper body in
                        // the other. `insert_implicit_var` collapses a
                        // same-definition repeat (the `Arrayed` arm re-parses
                        // every slot on the initial pass, so this is the common
                        // case) and refuses a different body loudly.
                        for new_var in new_vars {
                            if let Err(err) = insert_implicit_var(&mut helpers, new_var) {
                                errors.push(err);
                            }
                        }
                        Some(ast)
                    }
                    Err(err) => {
                        errors.push(err);
                        None
                    }
                }
            }
            None => {
                // An empty equation is only an error when the variable has no
                // graphical function. A standalone lookup-only table (empty
                // equation + a `<gf>`) is a valid static table, not an error
                // (issue #606): it produces no series and is excluded from the
                // runlist downstream. (WITH LOOKUP has a real input equation, so
                // it never reaches this empty-equation branch.) Only a flow and
                // an aux can carry a `gf`, so its presence IS the kind test.
                if errors.is_empty() && !is_initial && !v.can_be_module_input && v.gf.is_none() {
                    errors.push(EquationError::new(ErrorCode::EmptyEquation, 0, 0))
                }
                None
            }
        };

        (ast, errors)
    };

    // The unit string's diagnostics come first: a malformed `<units>` is
    // reported before, and compiled past, whatever the equation raises.
    let mut diagnostics: Vec<DiagnosticError> = vec![];
    let units = match parse_units(ctx.units_ctx, v.units) {
        Ok(units) => units,
        Err(errors) => {
            diagnostics.extend(errors.into_iter().map(DiagnosticError::Unit));
            None
        }
    };

    let ident = Ident::<Canonical>::new(v.ident);
    let (eqn, errors, kind) = match v.kind {
        SourceVariableKind::Stock => {
            // TODO: should is_intial be true here?
            let (ast, errors) = parse_and_lower_eqn(v.ident, &v.equation, false, None);
            (
                Some(v.equation.as_ref().clone()),
                errors,
                VarKind::Stock {
                    init_ast: ast,
                    inflows: v.inflows.iter().map(|i| Ident::new(i)).collect(),
                    outflows: v.outflows.iter().map(|o| Ident::new(o)).collect(),
                    non_negative: v.non_negative,
                },
            )
        }
        SourceVariableKind::Flow | SourceVariableKind::Aux => {
            let (ast, mut errors) = parse_and_lower_eqn(ident.as_str(), &v.equation, false, None);
            let (init_ast, init_errors) =
                parse_and_lower_eqn(ident.as_str(), &v.equation, true, v.active_initial);
            errors.extend(init_errors);

            let (tables, table_errors) = build_tables(v.gf, &v.equation, dimensions);
            errors.extend(table_errors);
            // A standalone graphical-function holder (a `<gf>`/MDL lookup with an
            // empty-or-sentinel equation) is a static table, not a value-bearing
            // variable: it is excluded from the runlist and produces no series.
            let is_table_only = v.is_lookup_only();
            (
                Some(v.equation.as_ref().clone()),
                errors,
                VarKind::Aux {
                    ast,
                    init_ast,
                    tables,
                    non_negative: v.non_negative,
                    is_flow: matches!(v.kind, SourceVariableKind::Flow),
                    is_table_only,
                    element_scope: None,
                },
            )
        }
        SourceVariableKind::Module => {
            let inputs = v.module_refs.iter().map(module_input_mapper);
            let (inputs, errors): (Vec<_>, Vec<_>) = inputs.partition(EquationResult::is_ok);
            let inputs: Vec<MI> = inputs.into_iter().flat_map(|i| i.unwrap()).collect();
            let errors: Vec<EquationError> = errors.into_iter().map(|e| e.unwrap_err()).collect();
            (
                None,
                errors,
                VarKind::Module {
                    model_name: Ident::new(v.model_name),
                    inputs,
                },
            )
        }
    };
    implicit_vars.extend(helpers.into_values());
    diagnostics.extend(errors.into_iter().map(DiagnosticError::Equation));
    Variable {
        ident,
        units,
        eqn,
        diagnostics,
        kind,
    }
}

/// How one occurrence reads its dependency's value.
///
/// A property of the occurrence, not of the name: one equation can read the
/// same variable currently and through `PREVIOUS`, and the scheduling
/// questions -- does this read order the reader after the dependency, does it
/// seed the initial snapshot, is the edge lagged -- are each answered by
/// selecting the lags that matter rather than by subtracting name sets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DepLag {
    /// The value the dependency holds in the phase being evaluated.
    Current,
    /// The prior step's committed value: a `PREVIOUS` argument.
    Previous,
    /// The frozen initial snapshot: an `INIT` argument, or a `PREVIOUS`
    /// fallback, which the initials phase populates so the reader's first
    /// step finds it (`db::dep_graph`'s `all_init_referenced` seeding).
    Initial,
}

/// One name an equation reads, and how.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DepOccurrence {
    /// The name as the equation spells it after canonicalization: a bare
    /// variable, or a `·`-qualified module read whose hops
    /// `db::variable_direct_dependencies` proves against the model.
    pub ident: Ident<Canonical>,
    pub lag: DepLag,
}

/// Every read of one AST, from a single walk ([`classify_dependencies`]).
#[derive(Default)]
pub struct DepClassification {
    /// Every data read, each name once per lag. Dimension names, and element
    /// names in subscript position, are syntax rather than reads and are
    /// filtered out.
    pub occurrences: BTreeSet<DepOccurrence>,
    /// Standalone lookup tables referenced via `LOOKUP(table, x)`. A table
    /// reference is a *layout* reference (codegen needs the table variable's
    /// offset for the table-identity reverse-map), NOT a data-flow dependency:
    /// it is kept OUT of `occurrences` so it never creates a runlist-ordering
    /// or causal/LTM edge, and is reunited with the dependency set only when
    /// the fragment compiler builds its metadata + tables map (issue #606).
    ///
    /// **A consumer wanting a table reference reads THIS FIELD; it is not, and
    /// must not be, in `occurrences`.** GH #606 justified the exclusion purely
    /// in terms of runlist ordering and said nothing about the other questions
    /// a caller can ask, which is how a later consumer came to read the
    /// omission as the information being unavailable. It is not: this field
    /// rides the same struct, from the same single pass, over the same AST.
    /// Spelled as the three questions a caller might mean:
    ///
    /// * **ordering** -- NOT a dependency. Static data imposes no
    ///   evaluation-order constraint, and a lookup-only holder is excluded from
    ///   every runlist (`Var::is_table_only`), so an edge to it would order
    ///   against something that is never evaluated.
    /// * **attribution** -- NOT a dependency. A table cannot vary, so it has no
    ///   delta and can carry no causal edge or link score. An edge here is a
    ///   wrong NUMBER, not a crash.
    /// * **dimension resolution** -- IT IS needed. A table argument's subscript
    ///   has to be resolved and element-pinned like any other arrayed
    ///   reference, and that needs the holder's declared dimensions.
    ///   `db::ltm::link_scores::pinnable_arrayed_deps` is the worked example: it
    ///   widens the per-element PIN candidates with this field while leaving the
    ///   ceteris-paribus wrap's dep set alone.
    ///
    /// Moving these into `occurrences` and filtering at the consumers was
    /// measured and rejected: five consumer families see them, four must filter
    /// them out, and the first one to break is compilation itself (a
    /// lookup-only holder has no value slot, so the fragment compiler refuses
    /// -- `lookup_only_tests`). Absent-by-default fails loudly (a consumer that
    /// needs tables sees nothing and says so); present-by-default fails
    /// silently.
    pub referenced_tables: BTreeSet<Ident<Canonical>>,
}

impl DepClassification {
    /// Every name read, whatever the lag.
    pub fn names(&self) -> HashSet<Ident<Canonical>> {
        self.occurrences
            .iter()
            .map(|occurrence| occurrence.ident.clone())
            .collect()
    }
}

/// What the dependency walk reads off one node, at either typed tier.
///
/// The walk runs over `Expr1` for the per-variable dependency query (the
/// typed tree, before any array bound exists) and over the retained `Expr2`
/// for LTM's per-slot readers, which classify one element's subtree of a
/// lowered target. Both tiers project onto this view, so there is one walk.
pub(crate) enum DepNode<'a, E: DepExpr> {
    Const,
    Var(&'a Ident<Canonical>),
    App(&'a BuiltinFn<E>),
    Subscript(&'a Ident<Canonical>, &'a [E::Index]),
    Op1(&'a E),
    Op2(&'a E, &'a E),
    If(&'a E, &'a E, &'a E),
}

/// A subscript position as the walk reads it: the expressions it holds.
pub(crate) enum DepIndex<'a, E> {
    Range(&'a E, &'a E),
    Expr(&'a E),
    /// A wildcard, a `*:dimension` star range or a dimension position:
    /// nothing is read.
    Positional,
}

/// An expression tier the dependency walk can read ([`DepNode`]).
pub(crate) trait DepExpr: Sized {
    type Index;
    fn dep_node(&self) -> DepNode<'_, Self>;
    fn dep_index(index: &Self::Index) -> DepIndex<'_, Self>;
}

impl DepExpr for Expr1 {
    type Index = IndexExpr1;

    fn dep_node(&self) -> DepNode<'_, Self> {
        match self {
            Expr1::Const(..) => DepNode::Const,
            Expr1::Var(id, _) => DepNode::Var(id),
            Expr1::App(builtin, _) => DepNode::App(builtin),
            Expr1::Subscript(id, args, _) => DepNode::Subscript(id, args),
            Expr1::Op1(_, l, _) => DepNode::Op1(l),
            Expr1::Op2(_, l, r, _) => DepNode::Op2(l, r),
            Expr1::If(c, t, f, _) => DepNode::If(c, t, f),
        }
    }

    fn dep_index(index: &IndexExpr1) -> DepIndex<'_, Self> {
        match index {
            IndexExpr1::Range(start, end, _) => DepIndex::Range(start, end),
            IndexExpr1::Expr(expr) => DepIndex::Expr(expr),
            IndexExpr1::Wildcard(_)
            | IndexExpr1::StarRange(_, _)
            | IndexExpr1::DimPosition(_, _) => DepIndex::Positional,
        }
    }
}

impl DepExpr for Expr2 {
    type Index = IndexExpr2;

    fn dep_node(&self) -> DepNode<'_, Self> {
        match self {
            Expr2::Const(..) => DepNode::Const,
            Expr2::Var(id, _, _) => DepNode::Var(id),
            Expr2::App(builtin, _, _) => DepNode::App(builtin),
            Expr2::Subscript(id, args, _, _) => DepNode::Subscript(id, args),
            Expr2::Op1(_, l, _, _) => DepNode::Op1(l),
            Expr2::Op2(_, l, r, _, _) => DepNode::Op2(l, r),
            Expr2::If(c, t, f, _, _) => DepNode::If(c, t, f),
        }
    }

    fn dep_index(index: &IndexExpr2) -> DepIndex<'_, Self> {
        match index {
            IndexExpr2::Range(start, end, _) => DepIndex::Range(start, end),
            IndexExpr2::Expr(expr) => DepIndex::Expr(expr),
            IndexExpr2::Wildcard(_)
            | IndexExpr2::StarRange(_, _)
            | IndexExpr2::DimPosition(_, _) => DepIndex::Positional,
        }
    }
}

/// The one AST walk that records each read with its lag.
///
/// `in_previous` / `in_init` say whether the walk is inside a `PREVIOUS` or
/// `INIT` argument; a read inside `PREVIOUS` is `Previous` whatever encloses
/// it, a read inside `INIT` (or a `PREVIOUS` fallback) is `Initial`, and any
/// other read is `Current`. A dimension name is never a read, an element name
/// in subscript position is not either, and an `isModuleInput(...)`
/// conditional walks only the live branch when the instance's inputs are
/// known.
struct ClassifyVisitor<'a> {
    occurrences: BTreeSet<DepOccurrence>,
    referenced_tables: BTreeSet<Ident<Canonical>>,
    dimensions: &'a [Dimension],
    module_inputs: Option<&'a BTreeSet<Ident<Canonical>>>,
    in_previous: bool,
    in_init: bool,
}

impl ClassifyVisitor<'_> {
    /// Whether `ident` names one of the active dimensions or one of their
    /// elements (and so is a subscript, not a variable reference).
    ///
    /// Both sides are already canonical -- `Dimension::canonical_name` is a
    /// `CanonicalDimensionName` and the AST's identifiers are
    /// `Ident<Canonical>` -- so this is two string comparisons per axis and no
    /// case folding.
    fn is_dimension_or_element(&self, ident: &Ident<Canonical>) -> bool {
        for dim in self.dimensions.iter() {
            if ident.as_str() == dim.canonical_name().as_str() {
                return true;
            }
            if let Dimension::Named(_, named_dim) = dim
                && named_dim.index_of(ident).is_some()
            {
                return true;
            }
        }
        false
    }

    fn is_dimension(&self, ident: &Ident<Canonical>) -> bool {
        self.dimensions
            .iter()
            .any(|dim| ident.as_str() == dim.canonical_name().as_str())
    }

    fn record(&mut self, ident: &Ident<Canonical>, lag: DepLag) {
        self.occurrences.insert(DepOccurrence {
            ident: ident.clone(),
            lag,
        });
    }

    /// The lag of a value read at the current position.
    fn lag(&self) -> DepLag {
        if self.in_previous {
            DepLag::Previous
        } else if self.in_init {
            DepLag::Initial
        } else {
            DepLag::Current
        }
    }

    /// Walk an index expression, filtering out dimension names/elements.
    fn walk_index_expr<E: DepExpr>(&mut self, expr: &E) {
        if let DepNode::Var(ident) = expr.dep_node()
            && self.is_dimension_or_element(ident)
        {
            return;
        }
        self.walk(expr);
    }

    fn walk_index<E: DepExpr>(&mut self, index: &E::Index) {
        match E::dep_index(index) {
            DepIndex::Range(start, end) => {
                self.walk_index_expr(start);
                self.walk_index_expr(end);
            }
            DepIndex::Expr(expr) => self.walk_index_expr(expr),
            DepIndex::Positional => {}
        }
    }

    fn walk<E: DepExpr>(&mut self, e: &E) {
        match e.dep_node() {
            DepNode::Const => {}
            DepNode::Var(id) => {
                if !self.is_dimension(id) {
                    self.record(id, self.lag());
                }
            }
            DepNode::App(builtin) => match builtin {
                BuiltinFn::Previous(arg, fallback) => {
                    let old = self.in_previous;
                    self.in_previous = true;
                    self.walk(arg.as_ref());
                    self.in_previous = old;

                    let old = self.in_init;
                    self.in_init = true;
                    self.walk(fallback.as_ref());
                    self.in_init = old;
                }
                BuiltinFn::Init(arg) => {
                    let old = self.in_init;
                    self.in_init = true;
                    self.walk(arg.as_ref());
                    self.in_init = old;
                }
                _ => {
                    walk_builtin_expr(builtin, |contents| match contents {
                        // The port an `isModuleInput` names is a structural
                        // fact about the instance, resolved at lowering; it is
                        // recorded as a current read wherever it appears, so a
                        // snapshot around it neither lags nor seeds it.
                        BuiltinContents::Ident(id, _loc) => {
                            self.record(&Ident::new(id), DepLag::Current);
                        }
                        BuiltinContents::Expr(expr) => self.walk(expr),
                        // A graphical-function table reference is a *layout*
                        // reference, not a data-flow dependency: record it in
                        // `referenced_tables` (so the fragment compiler can find
                        // the table's offset for the reverse-map) WITHOUT
                        // recording a read, keeping it off the runlist-ordering
                        // and causal/LTM graphs (issue #606). A *bare* reference
                        // to such a table is a plain `Var`, not a `LookupTable`,
                        // and is rejected separately as a compile error.
                        BuiltinContents::LookupTable(table_expr) => {
                            if let DepNode::Var(id) | DepNode::Subscript(id, _) =
                                table_expr.dep_node()
                            {
                                self.referenced_tables.insert(id.clone());
                            }
                        }
                    });
                }
            },
            DepNode::Subscript(id, args) => {
                self.record(id, self.lag());
                for arg in args {
                    self.walk_index::<E>(arg);
                }
            }
            DepNode::Op2(l, r) => {
                self.walk(l);
                self.walk(r);
            }
            DepNode::Op1(l) => {
                self.walk(l);
            }
            DepNode::If(cond, t, f) => {
                if let Some(module_inputs) = self.module_inputs
                    && let DepNode::App(BuiltinFn::IsModuleInput(ident, _)) = cond.dep_node()
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

/// Classify every read of an AST, with its lag, in one walk.
///
/// The one dependency walk: `db::variable_direct_dependencies` runs it over
/// a variable's typed `Expr1` and attaches the phase and the module path,
/// and LTM's readers run it over a retained `Expr2` subtree. `dimensions`
/// are the axes whose names and elements are syntax here; `module_inputs`
/// selects the live branch of an `isModuleInput(...)` conditional and walks
/// every branch when `None`.
pub(crate) fn classify_dependencies<E: DepExpr>(
    ast: &Ast<E>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> DepClassification {
    let mut visitor = ClassifyVisitor {
        occurrences: BTreeSet::new(),
        referenced_tables: BTreeSet::new(),
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
    DepClassification {
        occurrences: visitor.occurrences,
        referenced_tables: visitor.referenced_tables,
    }
}

/// Every name an AST reads, whatever the lag: the projection of
/// [`classify_dependencies`] the LTM rewriters consume.
pub(crate) fn identifier_set<E: DepExpr>(
    ast: &Ast<E>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> HashSet<Ident<Canonical>> {
    classify_dependencies(ast, dimensions, module_inputs).names()
}

/// Build an `Ast<Expr2>` from a scalar equation string via parse + lower, as
/// LTM's per-slot readers see a retained lowered tree.
///
/// The scope carries no shapes (a bounds-free lowering), which is inert for a
/// dependency row: `classify_dependencies` walks references, never bounds.
/// Production lowers under the project's dimension context where this uses an
/// empty one; the difference is inert for these rows because
/// `DimensionsContext::lookup` constifies only a qualified `dim·elem` spelling
/// and no row spells one, and a dimension name inside a `[..]` is filtered by
/// `classify_dependencies` from the `dimensions` it is handed, never by the
/// lowering.
///
/// Panics on parse or lowering errors -- intended for test use only.
#[cfg(test)]
pub(crate) fn scalar_ast(eqn: &str) -> Ast<Expr2> {
    use crate::ast::lower_ast;

    let (ast, err) = parse_equation(
        &datamodel::Equation::Scalar(eqn.to_owned()),
        &DimensionsContext::default(),
        false,
        None,
    );
    assert!(err.is_empty(), "parse error in test equation: {eqn}");
    let scope = LoweringScope {
        dimensions: &Default::default(),
        shapes: &Default::default(),
        model_name: "test",
    };
    lower_ast(&scope, &ast.unwrap(), false).unwrap()
}

/// The same equation at the typed tier, as `db::variable_direct_dependencies`
/// classifies it.
#[cfg(test)]
fn scalar_typed(eqn: &str) -> Ast<Expr1> {
    let (ast, err) = parse_equation(
        &datamodel::Equation::Scalar(eqn.to_owned()),
        &DimensionsContext::default(),
        false,
        None,
    );
    assert!(err.is_empty(), "parse error in test equation: {eqn}");
    crate::ast::typed_ast(&ast.unwrap(), &DimensionsContext::default()).unwrap()
}

/// Table-driven matrix test for `classify_dependencies`.
///
/// Rows cover every reference form (direct, `PREVIOUS`, `INIT`, mixed,
/// both-lagged, a `PREVIOUS` fallback) x context (scalar, `isModuleInput`,
/// apply-to-all, arrayed, every `IndexExpr` arm), the dimension-name and
/// element-name filters, the `isModuleInput` port rule and the table channel.
/// Each row asserts the complete occurrence relation and the table set; a row
/// given as an equation is classified at BOTH tiers -- the typed `Expr1` the
/// dependency query walks and the lowered `Expr2` LTM walks -- and the two
/// must agree, which is what pins the two `DepExpr` projections to one walk.
#[test]
fn test_classify_dependencies_matrix() {
    use crate::common::CanonicalElementName;

    enum Source {
        Eqn(&'static str),
        Ast(Ast<Expr2>),
    }

    struct DepTestCase {
        label: &'static str,
        source: Source,
        /// Dimensions for filtering (empty for most cases)
        dimensions: Vec<Dimension>,
        /// Module inputs for IsModuleInput branch selection (None for most cases)
        module_inputs: Option<BTreeSet<Ident<Canonical>>>,
        /// Expected: every `(name, lag)` occurrence
        expected: &'static [(&'static str, DepLag)],
        /// Expected: the `LOOKUP` table holders
        expected_tables: &'static [&'static str],
    }

    use DepLag::{Current, Initial, Previous};

    let loc = Loc::new(0, 1);
    let const_one = Expr2::Const("1".to_string(), crate::ast::Literal::new(1.0), loc);
    let const_zero = Expr2::Const("0".to_string(), crate::ast::Literal::new(0.0), loc);
    let var = |name: &str| Expr2::Var(Ident::new(name), None, loc);
    let previous = |name: &str| {
        Expr2::App(
            BuiltinFn::Previous(Box::new(var(name)), Box::new(const_zero.clone())),
            None,
            loc,
        )
    };
    let init = |name: &str| Expr2::App(BuiltinFn::Init(Box::new(var(name))), None, loc);
    let add = |l: Expr2, r: Expr2| {
        Expr2::Op2(
            crate::ast::BinaryOp::Add,
            Box::new(l),
            Box::new(r),
            None,
            loc,
        )
    };
    let subscript = |name: &str, index: IndexExpr2| {
        Ast::Scalar(Expr2::Subscript(Ident::new(name), vec![index], None, loc))
    };

    let module_inputs_with_input: BTreeSet<Ident<Canonical>> =
        [Ident::new("input")].into_iter().collect();

    // Dimension used for filtering tests
    let dim1 = Dimension::from(datamodel::Dimension::named(
        "dim1".to_string(),
        vec!["foo".to_owned()],
    ));

    let cases = vec![
        // -- Reference form: direct (no PREVIOUS/INIT) --
        DepTestCase {
            label: "direct_scalar",
            source: Source::Eqn("a + b"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("a", Current), ("b", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_a2a",
            source: Source::Ast(Ast::ApplyToAll(vec![dim1.clone()], add(var("a"), var("b")))),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("a", Current), ("b", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_arrayed",
            source: Source::Ast(Ast::Arrayed(
                vec![dim1.clone()],
                {
                    let mut elements = HashMap::new();
                    elements.insert(CanonicalElementName::from_raw("e1"), var("a"));
                    elements
                },
                Some(var("b")),
                false,
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("a", Current), ("b", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_ismoduleinput",
            source: Source::Eqn("if isModuleInput(input) then a else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected: &[("a", Current)],
            expected_tables: &[],
        },
        // -- Every `IndexExpr` arm --
        DepTestCase {
            label: "direct_range",
            source: Source::Ast(subscript(
                "arr",
                IndexExpr2::Range(const_one.clone(), var("const"), loc),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current), ("const", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_index_expr",
            source: Source::Ast(subscript("arr", IndexExpr2::Expr(var("index")))),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current), ("index", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_wildcard",
            source: Source::Ast(subscript("arr", IndexExpr2::Wildcard(loc))),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_star_range",
            source: Source::Ast(subscript(
                "arr",
                IndexExpr2::StarRange(crate::common::CanonicalDimensionName::from_raw("dim1"), loc),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "direct_dimension_position",
            source: Source::Ast(subscript("arr", IndexExpr2::DimPosition(1, loc))),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current)],
            expected_tables: &[],
        },
        // -- Reference form: PREVIOUS only --
        DepTestCase {
            label: "previous_scalar",
            source: Source::Eqn("PREVIOUS(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "previous_a2a",
            source: Source::Ast(Ast::ApplyToAll(vec![dim1.clone()], previous("b"))),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "previous_ismoduleinput",
            source: Source::Eqn("if isModuleInput(input) then PREVIOUS(a) else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected: &[("a", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "previous_range",
            source: Source::Ast(subscript(
                "arr",
                IndexExpr2::Range(const_one.clone(), previous("lagged"), loc),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current), ("lagged", Previous)],
            expected_tables: &[],
        },
        // -- Reference form: INIT only --
        DepTestCase {
            label: "init_scalar",
            source: Source::Eqn("INIT(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "init_a2a",
            source: Source::Ast(Ast::ApplyToAll(vec![dim1.clone()], init("b"))),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "init_ismoduleinput",
            source: Source::Eqn("if isModuleInput(input) then INIT(a) else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected: &[("a", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "init_range",
            source: Source::Ast(subscript(
                "arr",
                IndexExpr2::Range(const_one.clone(), init("seed"), loc),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current), ("seed", Initial)],
            expected_tables: &[],
        },
        // -- Reference form: mixed (current + lagged): one name, two lags --
        DepTestCase {
            label: "mixed_prev_current",
            source: Source::Eqn("PREVIOUS(b) + b"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Current), ("b", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "mixed_init_current",
            source: Source::Eqn("INIT(b) + b"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Current), ("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "mixed_prev_current_a2a",
            source: Source::Ast(Ast::ApplyToAll(
                vec![dim1.clone()],
                add(previous("b"), var("b")),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Current), ("b", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "mixed_prev_current_ismoduleinput",
            source: Source::Eqn("if isModuleInput(input) then PREVIOUS(a) + a else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected: &[("a", Current), ("a", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "mixed_prev_range",
            source: Source::Ast(subscript(
                "arr",
                IndexExpr2::Range(previous("b"), var("b"), loc),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current), ("b", Current), ("b", Previous)],
            expected_tables: &[],
        },
        // -- Reference form: both-lagged (PREVIOUS + INIT). Neither read is
        // current, so the name is read only through snapshots: the dt phase
        // orders nothing after it, and the `Previous` read stays a lagged
        // edge ("Phase 8.5 semantic divergences") --
        DepTestCase {
            label: "both_lagged_scalar",
            source: Source::Eqn("PREVIOUS(b) + INIT(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous), ("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "previous_fallback_same_target",
            source: Source::Eqn("PREVIOUS(b, b)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous), ("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "previous_fallback_other_target",
            source: Source::Eqn("PREVIOUS(b, c)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous), ("c", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "both_lagged_different",
            source: Source::Eqn("PREVIOUS(a) + INIT(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("a", Previous), ("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "both_lagged_a2a",
            source: Source::Ast(Ast::ApplyToAll(
                vec![dim1.clone()],
                add(previous("b"), init("b")),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous), ("b", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "both_lagged_ismoduleinput",
            source: Source::Eqn("if isModuleInput(input) then PREVIOUS(a) + INIT(a) else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected: &[("a", Previous), ("a", Initial)],
            expected_tables: &[],
        },
        DepTestCase {
            label: "both_lagged_range",
            source: Source::Ast(subscript(
                "arr",
                IndexExpr2::Range(previous("x"), init("y"), loc),
            )),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("arr", Current), ("x", Previous), ("y", Initial)],
            expected_tables: &[],
        },
        // -- Additional edge cases --
        DepTestCase {
            label: "nested_previous",
            source: Source::Eqn("PREVIOUS(PREVIOUS(x))"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("x", Previous)],
            expected_tables: &[],
        },
        DepTestCase {
            // A module read is one name here; the dependency query proves
            // its hops.
            label: "init_with_dotted_ref",
            source: Source::Eqn("INIT(m.out1) + m.out2"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("m\u{00b7}out1", Initial), ("m\u{00b7}out2", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            // An element name in subscript position is syntax.
            label: "element_in_subscript_is_not_a_read",
            source: Source::Eqn("g[foo]"),
            dimensions: vec![dim1.clone()],
            module_inputs: None,
            expected: &[("g", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            // A dimension name is syntax in any position.
            label: "dimension_name_is_not_a_read",
            source: Source::Eqn("g[dim1] + dim1"),
            dimensions: vec![dim1.clone()],
            module_inputs: None,
            expected: &[("g", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            // Without module_inputs, isModuleInput is not pruned and the port
            // it names is a read.
            label: "ismoduleinput_no_pruning",
            source: Source::Eqn("if isModuleInput(input) then a else b"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("a", Current), ("b", Current), ("input", Current)],
            expected_tables: &[],
        },
        DepTestCase {
            // The port is a structural fact: a snapshot around the
            // conditional lags the branches, never the port.
            label: "ismoduleinput_inside_init",
            source: Source::Eqn("INIT(if isModuleInput(input) then a else b)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("a", Initial), ("b", Initial), ("input", Current)],
            expected_tables: &[],
        },
        // -- The table channel --
        DepTestCase {
            label: "lookup_table_is_a_layout_reference",
            source: Source::Eqn("LOOKUP(tbl, x)"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("x", Current)],
            expected_tables: &["tbl"],
        },
        DepTestCase {
            label: "subscripted_lookup_table",
            source: Source::Eqn("LOOKUP(tbl[foo], PREVIOUS(x))"),
            dimensions: vec![dim1.clone()],
            module_inputs: None,
            expected: &[("x", Previous)],
            expected_tables: &["tbl"],
        },
        // -- Split by phase: the walk is phase-agnostic; the phase is what
        // `db::variable_direct_dependencies` attaches --
        DepTestCase {
            label: "split_phase",
            source: Source::Eqn("PREVIOUS(b) + c"),
            dimensions: vec![],
            module_inputs: None,
            expected: &[("b", Previous), ("c", Current)],
            expected_tables: &[],
        },
    ];

    let rows = |classified: &DepClassification| -> BTreeSet<(String, DepLag)> {
        classified
            .occurrences
            .iter()
            .map(|occurrence| (occurrence.ident.as_str().to_string(), occurrence.lag))
            .collect()
    };
    let tables = |classified: &DepClassification| -> BTreeSet<String> {
        classified
            .referenced_tables
            .iter()
            .map(|table| table.as_str().to_string())
            .collect()
    };

    for case in &cases {
        let expected: BTreeSet<(String, DepLag)> = case
            .expected
            .iter()
            .map(|(name, lag)| (name.to_string(), *lag))
            .collect();
        let expected_tables: BTreeSet<String> =
            case.expected_tables.iter().map(|t| t.to_string()).collect();

        let lowered = match &case.source {
            Source::Eqn(eqn) => {
                let typed = classify_dependencies(
                    &scalar_typed(eqn),
                    &case.dimensions,
                    case.module_inputs.as_ref(),
                );
                assert_eq!(expected, rows(&typed), "case '{}': typed tier", case.label);
                assert_eq!(
                    expected_tables,
                    tables(&typed),
                    "case '{}': typed tier tables",
                    case.label
                );
                scalar_ast(eqn)
            }
            Source::Ast(ast) => ast.clone(),
        };
        let result = classify_dependencies(&lowered, &case.dimensions, case.module_inputs.as_ref());
        assert_eq!(
            expected,
            rows(&result),
            "case '{}': lowered tier",
            case.label
        );
        assert_eq!(
            expected_tables,
            tables(&result),
            "case '{}': lowered tier tables",
            case.label
        );
    }
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
        true,
    );

    let (ast, errors) = parse_equation(
        &equation,
        &DimensionsContext::from(&dimensions),
        false,
        None,
    );
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

    let expected = Variable {
        ident: Ident::new("lookup_function_table"),
        units: None,
        eqn: Some(datamodel::Equation::Scalar("0".to_string())),
        diagnostics: vec![],
        kind: VarKind::Aux {
            ast: Some(Ast::Scalar(Expr0::Const(
                "0".to_string(),
                crate::ast::Literal::new(0.0),
                Loc::new(0, 1),
            ))),
            init_ast: None,
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
            element_scope: None,
        },
    };

    let mut implicit_vars: Vec<crate::capture::ImplicitVar> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let dims_ctx = DimensionsContext::default();
    let ctx = ParseContext::new(&dims_ctx, &unit_ctx);
    let output = parse_var(&ctx, &input, &mut implicit_vars, |mi| Ok(Some(mi.clone())));

    assert_eq!(expected, output);
}
