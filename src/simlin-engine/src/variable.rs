// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};

#[cfg(test)]
use crate::ast::Loc;
use crate::ast::{Ast, Expr0, Expr2, IndexExpr2};
use crate::builtins::{BuiltinContents, BuiltinFn, walk_builtin_expr};
use crate::builtins_visitor::{empty_macro_registry, instantiate_implicit_modules};
use crate::capture::ImplicitVar;
use crate::common::{
    Canonical, CanonicalElementName, DimensionName, EquationError, EquationResult, Ident,
    UnitError, canonicalize,
};
use crate::datamodel;
use crate::db::SourceVariableKind;
use crate::dimensions::{Dimension, DimensionsContext};
use crate::lexer::LexerType;
#[cfg(test)]
use crate::model::ScopeStage0;
use crate::module_functions::MacroRegistry;
use crate::units::parse_units;
use crate::{ErrorCode, eqn_err, units};

/// A graphical function's points, as the compiler and the VM read them.
///
/// The `f64`s keep the derived (IEEE) `PartialEq`, so a lookup table holding a
/// NaN y-point makes this -- and every `ModelStage0` / `ModelStage1` /
/// `db::query::ParsedVariableResult` carrying it -- unequal to a bit-identical
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
    /// How parsing and lowering report a failure on this variable; see the
    /// note on [`Variable::equation_errors`].
    pub errors: Vec<EquationError>,
    /// How parsing reports a malformed `<units>` string on this variable;
    /// see the note on [`Variable::unit_errors`].
    pub unit_errors: Vec<UnitError>,
    pub kind: VarKind<MI, E>,
}

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

    pub fn scalar_equation(&self) -> Option<&String> {
        match &self.eqn {
            Some(datamodel::Equation::Scalar(s)) => Some(s),
            _ => None,
        }
    }

    pub fn get_dimensions(&self) -> Option<&[Dimension]> {
        match self.ast()? {
            Ast::Arrayed(dims, _, _, _) | Ast::ApplyToAll(dims, _) => Some(dims),
            Ast::Scalar(_) => None,
        }
    }

    pub fn is_stock(&self) -> bool {
        matches!(self.kind, VarKind::Stock { .. })
    }

    pub fn is_module(&self) -> bool {
        matches!(self.kind, VarKind::Module { .. })
    }

    /// The equation errors parsing and lowering recorded on this variable.
    ///
    /// **This is a live error channel.** `parse_var` writes an equation's
    /// parse errors here and
    /// `model::lower_variable` appends the errors `lower_ast` raises, because
    /// both produce a `Variable` and have nowhere else to put a failure. The
    /// salsa path READS it: `db::var_fragment::explicit_fragment_input` turns each
    /// entry into a `Diagnostic`, at two sites. The read of the LOWERED
    /// variable is the one nothing else covers -- drop it and every
    /// `MismatchedDimensions` disappears
    /// (`db::diagnostic_tests::variable_error_fields_are_the_lowering_channel`
    /// is the standing gate). The read of the PARSED variable sees a strict
    /// subset, since `lower_variable` clones the parse errors forward, but it
    /// is where the conveyor/queue driven-flow `EmptyEquation` suppression
    /// applies, so dropping it turns a spec-sanctioned empty equation into a
    /// phantom error (`db::diagnostic_tests`'
    /// `test_conveyor_driven_flow_empty_equation_suppressed` and its two
    /// siblings).
    ///
    /// So `db::collect_model_diagnostics` is not an ALTERNATIVE source for
    /// these -- it is the same errors, downstream of this field.
    pub fn equation_errors(&self) -> Option<Vec<EquationError>> {
        if self.errors.is_empty() {
            None
        } else {
            Some(self.errors.clone())
        }
    }

    /// The malformed-`<units>`-string errors parsing recorded on this variable.
    ///
    /// Live for the same reason as [`Variable::equation_errors`]: `parse_var`
    /// is where a unit string is parsed, and `explicit_fragment_input` reads this
    /// field to emit the non-fatal `DiagnosticError::Unit` rows. Unit
    /// *consistency* mismatches are a different pass (`db::units`) and never
    /// land here -- nothing appends to this field after parsing.
    pub fn unit_errors(&self) -> Option<Vec<UnitError>> {
        if self.unit_errors.is_empty() {
            None
        } else {
            Some(self.unit_errors.clone())
        }
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
            errors: vec![],
            unit_errors: vec![],
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
            // check rejected it as `BadDimensionName` (the GH #541 arrayed
            // PREVIOUS/INIT helper regression on C-LEARN's capitalized
            // dimensions). Importer-produced equations already match exactly,
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
/// non-salsa paths (the `ModelStage0` oracle, and every path that parses a
/// synthesized implicit `datamodel::Variable`) come through the
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
/// contexts plus the four optional model-level facts that decide how
/// `PREVIOUS`/`INIT` and module-function calls expand.
///
/// A struct rather than a parameter list because every field is optional
/// context that most callers do not supply -- [`ParseContext::new`] is the
/// "no project context" parse the non-salsa paths use.
pub struct ParseContext<'a> {
    pub dimensions: &'a DimensionsContext,
    pub units_ctx: &'a units::Context,
    /// The parent model's module-backed variable identifiers. When provided,
    /// `PREVIOUS(module_var)` synthesizes a scalar helper aux instead of
    /// compiling `LoadPrev` directly against a multi-slot module.
    pub module_idents: Option<&'a HashSet<Ident<Canonical>>>,
    /// The model's full variable-name set. When provided, `PREVIOUS`/`INIT`
    /// accept a non-shadowed bare element name as a static subscript index
    /// instead of synthesizing a helper aux per call site (see
    /// `BuiltinVisitor::index_is_static`). The salsa per-variable parse path
    /// passes `None` to preserve incremental invalidation granularity (the
    /// parse must not depend on the model's full name set); the LTM equation
    /// parse path -- whose equations are regenerated wholesale on model
    /// changes anyway -- passes the set, which is what keeps large arrayed
    /// models' LTM helper volume bounded (GH #654).
    pub model_var_names: Option<&'a HashSet<Ident<Canonical>>>,
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
    /// A parse with no model-level context: no module-ident set, no model
    /// variable-name set, no project macros, and no enclosing macro body.
    pub fn new(dimensions: &'a DimensionsContext, units_ctx: &'a units::Context) -> Self {
        ParseContext {
            dimensions,
            units_ctx,
            module_idents: None,
            model_var_names: None,
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

    // Canonical name -> its index in `implicit_vars`, for the helpers THIS call
    // contributes. Seeded empty rather than from the caller's vector, which is
    // deliberate on both counts:
    //
    // * only helpers of the SAME parent can collide, since a synthesized name
    //   embeds its parent's ident (`$⁚{parent}⁚{n}⁚…`) and two parents sharing a
    //   canonical name is already a `DuplicateVariable` model error (GH #885);
    // * `model::ModelStage0` passes ONE vector across every variable of a model,
    //   so seeding from it would make each variable pay for every helper minted
    //   before it -- quadratic in the model, which is the shape this map exists
    //   to remove in the first place.
    let mut implicit_index: HashMap<Ident<Canonical>, usize> = HashMap::new();

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
                    ctx.module_idents,
                    ctx.model_var_names,
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
                        // `compute_layout` allocates one slot per name, so the
                        // loser used to be discarded in silence and one phase ran
                        // the other phase's helper body.
                        //
                        // The rule is `dedup_vars_by_ident`'s, applied across the
                        // phases instead of within one: a same-definition repeat
                        // collapses (the `Arrayed` arm re-parses every slot on the
                        // initial pass, so this is the common case and costs
                        // nothing), and a same-name/different-body pair is a loud
                        // error rather than a silent pick.
                        for new_var in new_vars {
                            let ident = Ident::<Canonical>::new(new_var.ident());
                            // Indexed, not scanned: an apply-to-all `SMTH1` over
                            // an N-element dimension mints ~2N helpers on one
                            // variable, and a scan here is the same O(k^2) shape
                            // `ImplicitVarMeta::index_hint` exists to remove --
                            // measured at +30% on N=800 before this map.
                            match implicit_index.get(&ident).map(|i| &implicit_vars[*i]) {
                                Some(existing) if existing.same_definition(&new_var) => {}
                                Some(_) => {
                                    // `DuplicateVariable` rather than the
                                    // `Generic` its within-one-pass twin uses:
                                    // this one is reachable from a model a user
                                    // wrote, so the code should say what went
                                    // wrong. Two helpers really do claim one
                                    // name here.
                                    errors.push(EquationError::detailed(
                                        ErrorCode::DuplicateVariable,
                                        0,
                                        0,
                                        format!(
                                            "two different synthesized helpers both claim the \
                                             name '{ident}'"
                                        ),
                                    ));
                                }
                                None => {
                                    implicit_index.insert(ident, implicit_vars.len());
                                    implicit_vars.push(new_var);
                                }
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

    let mut unit_errors: Vec<UnitError> = vec![];
    let units = match parse_units(ctx.units_ctx, v.units) {
        Ok(units) => units,
        Err(errors) => {
            unit_errors.extend(errors);
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
    Variable {
        ident,
        units,
        eqn,
        errors,
        unit_errors,
        kind,
    }
}

/// Result of classifying all dependency categories from a single AST walk.
#[derive(Default)]
pub struct DepClassification {
    /// All referenced identifiers (current + lagged + init-only).
    /// Dimension names are filtered out.
    pub all: HashSet<Ident<Canonical>>,
    /// Idents appearing as direct args to INIT() calls.
    pub init_referenced: BTreeSet<String>,
    /// Idents appearing as direct args to PREVIOUS() calls.
    pub previous_referenced: BTreeSet<String>,
    /// Idents referenced ONLY inside PREVIOUS() -- not outside it.
    pub previous_only: BTreeSet<String>,
    /// Idents referenced ONLY inside INIT() or PREVIOUS() -- not outside either.
    pub init_only: BTreeSet<String>,
    /// Standalone lookup tables referenced via `LOOKUP(table, x)`. A table
    /// reference is a *layout* reference (codegen needs the table variable's
    /// offset for the table-identity reverse-map), NOT a data-flow dependency:
    /// it is kept OUT of `all` so it never creates a runlist-ordering or
    /// causal/LTM edge, and is reunited with the dependency set only when the
    /// fragment compiler builds its metadata + tables map (issue #606).
    ///
    /// **A consumer wanting a table reference reads THIS FIELD; it is not, and
    /// must not be, in `all`.** GH #606 justified the exclusion purely in terms
    /// of runlist ordering and said nothing about the other questions a caller
    /// can ask, which is how a later consumer came to read the omission as the
    /// information being unavailable. It is not: this field rides the same
    /// struct, from the same single pass, over the same AST. Spelled as the
    /// three questions a caller might mean:
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
    /// Moving these into `all` and filtering at the consumers was measured and
    /// rejected: five consumer families see them, four must filter them out, and
    /// the first one to break is compilation itself (a lookup-only holder has no
    /// value slot, so the fragment compiler refuses -- `lookup_only_tests`).
    /// Absent-by-default fails loudly (a consumer that needs tables sees nothing
    /// and says so); present-by-default fails silently.
    pub referenced_tables: BTreeSet<String>,
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
    referenced_tables: BTreeSet<String>,
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

    /// Walk an expression, filtering out dimension names/elements
    fn walk_index_expr(&mut self, expr: &Expr2) {
        if let Expr2::Var(arg_ident, _, _) = expr {
            if !self.is_dimension_or_element(arg_ident) {
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
                // `Dimension::canonical_name` is already canonical, as is `id`.
                let is_dimension = self
                    .dimensions
                    .iter()
                    .any(|dim| id.as_str() == dim.canonical_name().as_str());
                if !is_dimension {
                    self.all.insert(id.clone());
                    if self.in_init && !self.in_previous {
                        self.init_referenced.insert(id.to_string());
                    }
                }
                self.record_ident(id.as_str());
            }
            Expr2::App(builtin, _, _) => match builtin {
                BuiltinFn::Previous(arg, fallback) => {
                    if let Expr2::Var(ident, _, _) | Expr2::Subscript(ident, _, _, _) = arg.as_ref()
                    {
                        self.previous_referenced.insert(ident.to_string());
                    }

                    let old = self.in_previous;
                    self.in_previous = true;
                    self.walk(arg);
                    self.in_previous = old;

                    let old = self.in_init;
                    self.in_init = true;
                    self.walk(fallback);
                    self.in_init = old;
                }
                BuiltinFn::Init(arg) => {
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
                        // A graphical-function table reference is a *layout*
                        // reference, not a data-flow dependency: record it in
                        // `referenced_tables` (so the fragment compiler can find
                        // the table's offset for the reverse-map) WITHOUT adding
                        // it to `all`, keeping it off the runlist-ordering and
                        // causal/LTM graphs (issue #606). A *bare* reference to
                        // such a table is a plain `Var`, not a `LookupTable`, and
                        // is rejected separately as a compile error.
                        BuiltinContents::LookupTable(table_expr) => {
                            if let Expr2::Var(id, _, _) | Expr2::Subscript(id, _, _, _) = table_expr
                            {
                                self.referenced_tables.insert(id.to_string());
                            }
                        }
                    });
                }
            },
            Expr2::Subscript(id, args, _, _) => {
                self.all.insert(id.clone());
                if self.in_init && !self.in_previous {
                    self.init_referenced.insert(id.to_string());
                }
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
        referenced_tables: visitor.referenced_tables,
    }
}

pub fn identifier_set(
    ast: &Ast<Expr2>,
    dimensions: &[Dimension],
    module_inputs: Option<&BTreeSet<Ident<Canonical>>>,
) -> HashSet<Ident<Canonical>> {
    classify_dependencies(ast, dimensions, module_inputs).all
}

/// Collect variable identifiers referenced by `PREVIOUS(x)` calls in an AST.
///
/// These identifiers are lagged dependencies (t-1), not same-step edges.
pub fn previous_referenced_idents(ast: &Ast<Expr2>) -> BTreeSet<String> {
    classify_dependencies(ast, &[], None).previous_referenced
}

/// Build an `Ast<Expr2>` from a scalar equation string via parse + lower.
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
    let scope = ScopeStage0 {
        models: &Default::default(),
        dimensions: &Default::default(),
        model_name: "test",
    };
    lower_ast(&scope, &ast.unwrap()).unwrap()
}

/// Table-driven matrix test for `classify_dependencies`.
///
/// Covers all combinations of reference form (direct, PREVIOUS, INIT, mixed,
/// both-lagged) x context (scalar, isModuleInput, ApplyToAll, subscript range),
/// plus all 7 prior bug-fix edge cases. Each case asserts all 5 fields of
/// `DepClassification`.
#[test]
fn test_classify_dependencies_matrix() {
    use crate::common::CanonicalElementName;

    struct DepTestCase {
        /// Human-readable label for assertion messages
        label: &'static str,
        /// The AST to classify
        ast: Ast<Expr2>,
        /// Dimensions for filtering (empty for most cases)
        dimensions: Vec<Dimension>,
        /// Module inputs for IsModuleInput branch selection (None for most cases)
        module_inputs: Option<BTreeSet<Ident<Canonical>>>,
        /// Expected: all referenced identifiers (as strings)
        expected_all: HashSet<&'static str>,
        /// Expected: direct INIT() argument names
        expected_init_referenced: BTreeSet<&'static str>,
        /// Expected: direct PREVIOUS() argument names
        expected_previous_referenced: BTreeSet<&'static str>,
        /// Expected: idents ONLY inside PREVIOUS (not outside)
        expected_previous_only: BTreeSet<&'static str>,
        /// Expected: idents ONLY inside INIT/PREVIOUS (not outside either)
        expected_init_only: BTreeSet<&'static str>,
    }

    let loc = Loc::new(0, 1);
    let const_one = Expr2::Const("1".to_string(), crate::ast::Literal::new(1.0), loc);
    let const_zero = Expr2::Const("0".to_string(), crate::ast::Literal::new(0.0), loc);

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
            ast: scalar_ast("a + b"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["a", "b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "direct_a2a",
            ast: Ast::ApplyToAll(vec![dim1.clone()], {
                // a + b wrapped in ApplyToAll
                let a = Expr2::Var(Ident::new("a"), None, loc);
                let b = Expr2::Var(Ident::new("b"), None, loc);
                Expr2::Op2(
                    crate::ast::BinaryOp::Add,
                    Box::new(a),
                    Box::new(b),
                    None,
                    loc,
                )
            }),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["a", "b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "direct_arrayed",
            ast: Ast::Arrayed(
                vec![dim1.clone()],
                {
                    let mut elements = HashMap::new();
                    elements.insert(
                        CanonicalElementName::from_raw("e1"),
                        Expr2::Var(Ident::new("a"), None, loc),
                    );
                    elements
                },
                Some(Expr2::Var(Ident::new("b"), None, loc)),
                false,
            ),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["a", "b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "direct_ismoduleinput",
            ast: scalar_ast("if isModuleInput(input) then a else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected_all: ["a"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "direct_range",
            ast: Ast::Scalar(Expr2::Subscript(
                Ident::new("arr"),
                vec![IndexExpr2::Range(
                    const_one.clone(),
                    Expr2::Var(Ident::new("const"), None, loc),
                    loc,
                )],
                None,
                loc,
            )),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["arr", "const"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        // -- Reference form: PREVIOUS only --
        DepTestCase {
            // Edge case 1: PREVIOUS feedback
            label: "previous_scalar",
            ast: scalar_ast("PREVIOUS(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: ["b"].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "previous_a2a",
            ast: Ast::ApplyToAll(
                vec![dim1.clone()],
                Expr2::App(
                    BuiltinFn::Previous(
                        Box::new(Expr2::Var(Ident::new("b"), None, loc)),
                        Box::new(const_zero.clone()),
                    ),
                    None,
                    loc,
                ),
            ),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: ["b"].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "previous_ismoduleinput",
            ast: scalar_ast("if isModuleInput(input) then PREVIOUS(a) else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected_all: ["a"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["a"].into(),
            expected_previous_only: ["a"].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "previous_range",
            ast: Ast::Scalar(Expr2::Subscript(
                Ident::new("arr"),
                vec![IndexExpr2::Range(
                    const_one.clone(),
                    Expr2::App(
                        BuiltinFn::Previous(
                            Box::new(Expr2::Var(Ident::new("lagged"), None, loc)),
                            Box::new(const_zero.clone()),
                        ),
                        None,
                        loc,
                    ),
                    loc,
                )],
                None,
                loc,
            )),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["arr", "lagged"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["lagged"].into(),
            expected_previous_only: ["lagged"].into(),
            expected_init_only: [].into(),
        },
        // -- Reference form: INIT only --
        DepTestCase {
            // Edge cases 4 and 5: INIT-only + fragment context (all contains b)
            label: "init_scalar",
            ast: scalar_ast("INIT(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: ["b"].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["b"].into(),
        },
        DepTestCase {
            label: "init_a2a",
            ast: Ast::ApplyToAll(
                vec![dim1.clone()],
                Expr2::App(
                    BuiltinFn::Init(Box::new(Expr2::Var(Ident::new("b"), None, loc))),
                    None,
                    loc,
                ),
            ),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: ["b"].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["b"].into(),
        },
        DepTestCase {
            label: "init_ismoduleinput",
            ast: scalar_ast("if isModuleInput(input) then INIT(a) else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected_all: ["a"].into(),
            expected_init_referenced: ["a"].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["a"].into(),
        },
        DepTestCase {
            label: "init_range",
            ast: Ast::Scalar(Expr2::Subscript(
                Ident::new("arr"),
                vec![IndexExpr2::Range(
                    const_one.clone(),
                    Expr2::App(
                        BuiltinFn::Init(Box::new(Expr2::Var(Ident::new("seed"), None, loc))),
                        None,
                        loc,
                    ),
                    loc,
                )],
                None,
                loc,
            )),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["arr", "seed"].into(),
            expected_init_referenced: ["seed"].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["seed"].into(),
        },
        // -- Reference form: mixed (current + lagged) --
        DepTestCase {
            // Edge case 2: mixed current+lagged -- b is NOT previous_only
            label: "mixed_prev_current",
            ast: scalar_ast("PREVIOUS(b) + b"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "mixed_init_current",
            ast: scalar_ast("INIT(b) + b"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: ["b"].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "mixed_prev_current_a2a",
            ast: Ast::ApplyToAll(vec![dim1.clone()], {
                let prev = Expr2::App(
                    BuiltinFn::Previous(
                        Box::new(Expr2::Var(Ident::new("b"), None, loc)),
                        Box::new(const_zero.clone()),
                    ),
                    None,
                    loc,
                );
                let direct = Expr2::Var(Ident::new("b"), None, loc);
                Expr2::Op2(
                    crate::ast::BinaryOp::Add,
                    Box::new(prev),
                    Box::new(direct),
                    None,
                    loc,
                )
            }),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "mixed_prev_current_ismoduleinput",
            ast: scalar_ast("if isModuleInput(input) then PREVIOUS(a) + a else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected_all: ["a"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["a"].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            // mixed x range: b appears as the PREVIOUS range start and as the direct
            // range end.  b is in previous_referenced but also in non_previous (the
            // direct range end occurrence), so previous_only is empty.
            label: "mixed_prev_range",
            ast: Ast::Scalar(Expr2::Subscript(
                Ident::new("arr"),
                vec![IndexExpr2::Range(
                    Expr2::App(
                        BuiltinFn::Previous(
                            Box::new(Expr2::Var(Ident::new("b"), None, loc)),
                            Box::new(const_zero.clone()),
                        ),
                        None,
                        loc,
                    ),
                    Expr2::Var(Ident::new("b"), None, loc),
                    loc,
                )],
                None,
                loc,
            )),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["arr", "b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        // -- Reference form: both-lagged (PREVIOUS + INIT) --
        DepTestCase {
            // Edge case 6: PREVIOUS + INIT combined -- b is init_only
            // (PREVIOUS context also counts as init-excluded).
            // b is NOT previous_only because INIT(b) walks b outside PREVIOUS context.
            label: "both_lagged_scalar",
            ast: scalar_ast("PREVIOUS(b) + INIT(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: ["b"].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["b"].into(),
        },
        DepTestCase {
            label: "both_lagged_different",
            ast: scalar_ast("PREVIOUS(a) + INIT(b)"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["a", "b"].into(),
            expected_init_referenced: ["b"].into(),
            expected_previous_referenced: ["a"].into(),
            expected_previous_only: ["a"].into(),
            expected_init_only: ["b"].into(),
        },
        DepTestCase {
            // Same semantics as both_lagged_scalar: INIT(b) walks b outside
            // PREVIOUS context, so b is NOT previous_only.
            label: "both_lagged_a2a",
            ast: Ast::ApplyToAll(vec![dim1.clone()], {
                let prev = Expr2::App(
                    BuiltinFn::Previous(
                        Box::new(Expr2::Var(Ident::new("b"), None, loc)),
                        Box::new(const_zero.clone()),
                    ),
                    None,
                    loc,
                );
                let init = Expr2::App(
                    BuiltinFn::Init(Box::new(Expr2::Var(Ident::new("b"), None, loc))),
                    None,
                    loc,
                );
                Expr2::Op2(
                    crate::ast::BinaryOp::Add,
                    Box::new(prev),
                    Box::new(init),
                    None,
                    loc,
                )
            }),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b"].into(),
            expected_init_referenced: ["b"].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["b"].into(),
        },
        DepTestCase {
            // both-lagged x isModuleInput: the active (then) branch is
            // PREVIOUS(a) + INIT(a).  a is in both previous_referenced and
            // init_referenced.  INIT(a) walks a outside PREVIOUS context, so
            // a ends up in non_previous, making previous_only empty.  a is
            // never walked outside any lagged context, so init_only={a}.
            label: "both_lagged_ismoduleinput",
            ast: scalar_ast("if isModuleInput(input) then PREVIOUS(a) + INIT(a) else b"),
            dimensions: vec![],
            module_inputs: Some(module_inputs_with_input.clone()),
            expected_all: ["a"].into(),
            expected_init_referenced: ["a"].into(),
            expected_previous_referenced: ["a"].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["a"].into(),
        },
        DepTestCase {
            // both-lagged x range: range start is PREVIOUS(x), range end is INIT(y).
            // x is in previous_referenced and previous_only (never seen outside PREVIOUS).
            // y is in init_referenced and init_only (never seen outside any lagged context).
            label: "both_lagged_range",
            ast: Ast::Scalar(Expr2::Subscript(
                Ident::new("arr"),
                vec![IndexExpr2::Range(
                    Expr2::App(
                        BuiltinFn::Previous(
                            Box::new(Expr2::Var(Ident::new("x"), None, loc)),
                            Box::new(const_zero.clone()),
                        ),
                        None,
                        loc,
                    ),
                    Expr2::App(
                        BuiltinFn::Init(Box::new(Expr2::Var(Ident::new("y"), None, loc))),
                        None,
                        loc,
                    ),
                    loc,
                )],
                None,
                loc,
            )),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["arr", "x", "y"].into(),
            expected_init_referenced: ["y"].into(),
            expected_previous_referenced: ["x"].into(),
            expected_previous_only: ["x"].into(),
            expected_init_only: ["y"].into(),
        },
        // -- Additional edge cases --
        DepTestCase {
            // Edge case 7: nested PREVIOUS
            label: "nested_previous",
            ast: scalar_ast("PREVIOUS(PREVIOUS(x))"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["x"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["x"].into(),
            expected_previous_only: ["x"].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            label: "init_with_dotted_ref",
            ast: scalar_ast("INIT(m.out1) + m.out2"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["m\u{00b7}out1", "m\u{00b7}out2"].into(),
            expected_init_referenced: ["m\u{00b7}out1"].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: ["m\u{00b7}out1"].into(),
        },
        DepTestCase {
            // Dimension element names in subscript positions are filtered out.
            // g[foo] with dim1={foo} -> only g appears in all.
            label: "dim_filtering",
            ast: scalar_ast("g[foo]"),
            dimensions: vec![dim1.clone()],
            module_inputs: None,
            expected_all: ["g"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        DepTestCase {
            // Without module_inputs, isModuleInput is not pruned
            label: "ismoduleinput_no_pruning",
            ast: scalar_ast("if isModuleInput(input) then a else b"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["input", "a", "b"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: [].into(),
            expected_previous_only: [].into(),
            expected_init_only: [].into(),
        },
        // -- Edge case 3: split by phase --
        // classify_dependencies is phase-agnostic. The same equation produces
        // identical classifications regardless of whether the caller considers it
        // a dt AST or init AST. The "split" behavior is in how db.rs assigns
        // results from separate classify_dependencies calls.
        DepTestCase {
            label: "split_phase",
            ast: scalar_ast("PREVIOUS(b) + c"),
            dimensions: vec![],
            module_inputs: None,
            expected_all: ["b", "c"].into(),
            expected_init_referenced: [].into(),
            expected_previous_referenced: ["b"].into(),
            expected_previous_only: ["b"].into(),
            expected_init_only: [].into(),
        },
    ];

    for case in &cases {
        let result =
            classify_dependencies(&case.ast, &case.dimensions, case.module_inputs.as_ref());

        // Convert all to HashSet<&str> for comparison
        let got_all: HashSet<&str> = result.all.iter().map(|id| id.as_str()).collect();
        assert_eq!(case.expected_all, got_all, "case '{}': all", case.label);

        let got_init_ref: BTreeSet<&str> =
            result.init_referenced.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            case.expected_init_referenced, got_init_ref,
            "case '{}': init_referenced",
            case.label
        );

        let got_prev_ref: BTreeSet<&str> = result
            .previous_referenced
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            case.expected_previous_referenced, got_prev_ref,
            "case '{}': previous_referenced",
            case.label
        );

        let got_prev_only: BTreeSet<&str> =
            result.previous_only.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            case.expected_previous_only, got_prev_only,
            "case '{}': previous_only",
            case.label
        );

        let got_init_only: BTreeSet<&str> = result.init_only.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            case.expected_init_only, got_init_only,
            "case '{}': init_only",
            case.label
        );

        // Structural invariant: `all` (as strings) must be a superset of
        // init_referenced union previous_referenced.
        // This is the fragment context invariant (edge case 5): compile_var_fragment
        // uses `all` for dt_deps, so it must include INIT/PREVIOUS args.
        let init_prev_union: HashSet<&str> = result
            .init_referenced
            .iter()
            .chain(result.previous_referenced.iter())
            .map(|s| s.as_str())
            .collect();
        assert!(
            got_all.is_superset(&init_prev_union),
            "case '{}': structural invariant violated -- `all` must be superset of \
             init_referenced union previous_referenced.\n  all: {:?}\n  union: {:?}",
            case.label,
            got_all,
            init_prev_union,
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
        errors: vec![],
        unit_errors: vec![],
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
        },
    };

    let mut implicit_vars: Vec<crate::capture::ImplicitVar> = Vec::new();
    let unit_ctx = crate::units::Context::new(&[], &Default::default()).0;
    let dims_ctx = DimensionsContext::default();
    let ctx = ParseContext::new(&dims_ctx, &unit_ctx);
    let output = parse_var(&ctx, &input, &mut implicit_vars, |mi| Ok(Some(mi.clone())));

    assert_eq!(expected, output);
}
