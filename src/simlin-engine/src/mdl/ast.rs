// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! AST types for the Vensim MDL parser.
//!
//! These types are produced by the recursive descent parser from normalized
//! tokens and later converted to `crate::datamodel` structures.

use std::borrow::Cow;

/// Byte span in source text for error reporting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Loc {
    pub start: u32,
    pub end: u32,
}

impl Loc {
    pub fn new(start: usize, end: usize) -> Self {
        Loc {
            start: start as u32,
            end: end as u32,
        }
    }

    pub fn merge(a: Loc, b: Loc) -> Self {
        Loc {
            start: a.start.min(b.start),
            end: a.end.max(b.end),
        }
    }
}

/// Unary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    /// `+x`
    Positive,
    /// `-x`
    Negative,
    /// `:NOT: x`
    Not,
}

/// Binary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `^`
    Exp,
    /// `<`
    Lt,
    /// `>`
    Gt,
    /// `<=`
    Lte,
    /// `>=`
    Gte,
    /// `=`
    Eq,
    /// `<>`
    Neq,
    /// `:AND:`
    And,
    /// `:OR:`
    Or,
}

/// Subscript expression (inside `[...]`).
#[derive(Clone, Debug, PartialEq)]
pub enum Subscript<'input> {
    /// Simple element or dimension name: `DimA`, `elem1`
    Element(Cow<'input, str>, Loc),
    /// Bang-marked element: `elem1!`
    BangElement(Cow<'input, str>, Loc),
}

/// Distinguishes the origin of a call expression.
///
/// The parser receives different token types for known builtin functions vs
/// symbol-based calls. This distinction is important for conversion:
/// - `Builtin`: Known function like `MAX`, `INTEG`, parsed from `Token::Function`
/// - `Symbol`: Symbol-based call parsed from `Token::Symbol`, could be:
///   - Lookup invocation (1 arg): `table(x)`
///   - Unknown function (multiple args): treated as error or macro call
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    /// Known builtin function (from `Token::Function`)
    Builtin,
    /// Symbol-based call (from `Token::Symbol`)
    ///
    /// If 1 arg: likely a lookup invocation
    /// If multiple args: unknown function (may be macro or error)
    Symbol,
}

/// Expression AST.
///
/// The lifetime `'input` ties string references to the source text for zero-copy parsing.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr<'input> {
    /// Numeric literal: `1.5`, `1e-6`
    Const(f64, Loc),
    /// Variable reference: `x`, `x[DimA]` (possibly subscripted)
    Var(Cow<'input, str>, Vec<Subscript<'input>>, Loc),
    /// Call expression: `MAX(a, b)`, `table(x)`, `macro(args)`
    ///
    /// Fields: name, subscripts (for `table[DimA](x)`), args, call kind, loc
    ///
    /// The `CallKind` indicates whether this came from a known builtin function
    /// or a symbol-based call. For symbol calls:
    /// - 1 arg: likely a lookup invocation
    /// - multiple args: unknown function (could be macro or error)
    App(
        Cow<'input, str>,
        Vec<Subscript<'input>>,
        Vec<Expr<'input>>,
        CallKind,
        Loc,
    ),
    /// Unary operator: `-x`, `+x`, `:NOT: x`
    Op1(UnaryOp, Box<Expr<'input>>, Loc),
    /// Binary operator: `a + b`, `a :AND: b`
    Op2(BinaryOp, Box<Expr<'input>>, Box<Expr<'input>>, Loc),
    /// Parenthesized expression: `(x + y)`
    ///
    /// Retained for accurate roundtripping even though semantically equivalent to inner.
    Paren(Box<Expr<'input>>, Loc),
    /// Literal string: `'A FUNCTION OF'` (single-quoted)
    ///
    /// Also used for the `?` placeholder produced by trailing-comma function
    /// calls. For example, `FUNC(a, b,)` produces args `[a, b, Literal("?")]`.
    /// This matches xmutil's `vpyy_literal_expression("?")` behavior.
    Literal(Cow<'input, str>, Loc),
    /// `:NA:` constant (-1e38)
    Na(Loc),
}

/// Format of lookup table data.
///
/// Vensim supports two table formats that require different handling:
/// - Modern pairs format: `(x1,y1), (x2,y2), ...`
/// - Legacy XY vector format: `x1, x2, ..., xN, y1, y2, ..., yN` (flat vector split in half)
///
/// The legacy format must be transformed during conversion via `transform_legacy()`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TableFormat {
    /// Modern pairs format: `(x1,y1), (x2,y2), ...`
    #[default]
    Pairs,
    /// Legacy XY vector format: `x1, x2, ..., xN, y1, y2, ..., yN`
    ///
    /// The raw values form a flat vector that must be split in half during
    /// conversion: the first half becomes X values, the second half becomes
    /// Y values. This matches xmutil's `TransformLegacy()` behavior.
    ///
    /// Note: The raw values are stored in `x_vals` during parsing. During
    /// conversion, call `transform_legacy()` to split them into x/y pairs.
    LegacyXY,
}

/// Inline lookup table definition: `[(xmin,ymin)-(xmax,ymax)], (x1,y1), ...`
#[derive(Clone, Debug, PartialEq)]
pub struct LookupTable {
    pub x_vals: Vec<f64>,
    pub y_vals: Vec<f64>,
    pub x_range: Option<(f64, f64)>,
    pub y_range: Option<(f64, f64)>,
    /// Format of the table data (pairs vs legacy XY vector)
    pub format: TableFormat,
    /// Whether this table should extrapolate beyond its bounds.
    ///
    /// This is set during conversion when `LOOKUP EXTRAPOLATE` or `TABXL`
    /// functions reference this table. It affects XMILE output (emits
    /// `type="extrapolate"` on the `<gf>` element).
    pub extrapolate: bool,
    pub loc: Loc,
}

impl LookupTable {
    /// Create an empty lookup table with pairs format.
    pub fn new(loc: Loc) -> Self {
        LookupTable {
            x_vals: Vec::new(),
            y_vals: Vec::new(),
            x_range: None,
            y_range: None,
            format: TableFormat::Pairs,
            extrapolate: false,
            loc,
        }
    }

    /// Create an empty lookup table with legacy XY format.
    pub fn new_legacy(loc: Loc) -> Self {
        LookupTable {
            x_vals: Vec::new(),
            y_vals: Vec::new(),
            x_range: None,
            y_range: None,
            format: TableFormat::LegacyXY,
            extrapolate: false,
            loc,
        }
    }

    /// Add a (x, y) pair to the table.
    pub fn add_pair(&mut self, x: f64, y: f64) {
        self.x_vals.push(x);
        self.y_vals.push(y);
    }

    /// Add a raw value to x_vals (used for legacy format during parsing).
    pub fn add_raw(&mut self, val: f64) {
        self.x_vals.push(val);
    }

    /// Set the x/y range bounds from a `[(xmin,ymin)-(xmax,ymax)]` clause.
    pub fn set_range(&mut self, x_min: f64, y_min: f64, x_max: f64, y_max: f64) {
        self.x_range = Some((x_min, x_max));
        self.y_range = Some((y_min, y_max));
    }

    /// Transform a legacy XY format table to pairs format.
    ///
    /// Legacy format stores values as a flat vector `x1, x2, ..., xN, y1, y2, ..., yN`.
    /// This method splits the vector in half: first half becomes x_vals, second half
    /// becomes y_vals. This matches xmutil's `TransformLegacy()` behavior.
    ///
    /// Returns `Err` if the format is not LegacyXY or if the count is odd.
    pub fn transform_legacy(&mut self) -> Result<(), &'static str> {
        if self.format != TableFormat::LegacyXY {
            return Err("transform_legacy called on non-legacy table");
        }
        if !self.x_vals.len().is_multiple_of(2) {
            return Err("legacy table must have even number of values");
        }

        let n = self.x_vals.len() / 2;
        // Split: first half stays as x_vals, second half becomes y_vals
        self.y_vals = self.x_vals.split_off(n);
        self.format = TableFormat::Pairs;
        Ok(())
    }
}

/// Interpolation mode for data equations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpMode {
    Interpolate,
    Raw,
    HoldBackward,
    LookForward,
}

/// Exception clause: `:EXCEPT: [elem1, elem2], [elem3]`
#[derive(Clone, Debug, PartialEq)]
pub struct ExceptList<'input> {
    pub subscripts: Vec<Vec<Subscript<'input>>>,
    pub loc: Loc,
}

/// Left-hand side of an equation.
#[derive(Clone, Debug, PartialEq)]
pub struct Lhs<'input> {
    /// Variable name
    pub name: Cow<'input, str>,
    /// Optional subscripts: `[DimA, DimB]`
    pub subscripts: Vec<Subscript<'input>>,
    /// Optional exception list: `:EXCEPT: [...]`
    pub except: Option<ExceptList<'input>>,
    /// Optional interpolation mode for data equations
    pub interp_mode: Option<InterpMode>,
    pub loc: Loc,
}

impl<'input> Lhs<'input> {
    /// Create an empty LHS (used for synthetic entries like EqEnd or GroupStar).
    pub fn empty(loc: Loc) -> Self {
        Lhs {
            name: Cow::Borrowed(""),
            subscripts: vec![],
            except: None,
            interp_mode: None,
            loc,
        }
    }
}

/// Subscript/dimension definition element.
#[derive(Clone, Debug, PartialEq)]
pub enum SubscriptElement<'input> {
    /// Single element name
    Element(Cow<'input, str>, Loc),
    /// Numeric range: `(A1-A10)` expands to A1, A2, ..., A10
    Range(Cow<'input, str>, Cow<'input, str>, Loc),
}

/// Entry in a mapping list (`mapsymlist` in C++ grammar).
///
/// A mapping list can contain:
/// - Simple symbol names: `DimB`
/// - Dimension with explicit element list: `(DimB: b1, b2, b3)`
/// - Nested symbol lists (represented as List variant)
#[derive(Clone, Debug, PartialEq)]
pub enum MappingEntry<'input> {
    /// Simple symbol name
    Name(Cow<'input, str>, Loc),
    /// Bang-marked symbol name
    BangName(Cow<'input, str>, Loc),
    /// Dimension with explicit element mapping: `(DimB: b1, b2, b3)`
    DimensionMapping {
        dimension: Cow<'input, str>,
        elements: Vec<Subscript<'input>>,
        loc: Loc,
    },
    /// Nested list (for complex recursive mappings)
    List(Vec<MappingEntry<'input>>, Loc),
}

/// Mapping clause: `-> mapsymlist`
///
/// The mapping list can contain multiple entries, each of which may be
/// a simple symbol, a bang-marked symbol, a dimension mapping with elements,
/// or a nested list.
#[derive(Clone, Debug, PartialEq)]
pub struct SubscriptMapping<'input> {
    /// Mapping entries
    pub entries: Vec<MappingEntry<'input>>,
    pub loc: Loc,
}

/// Subscript/dimension definition: `DimA: elem1, elem2, ...`
#[derive(Clone, Debug, PartialEq)]
pub struct SubscriptDef<'input> {
    /// Elements: `elem1, elem2, (A1-A10)`
    pub elements: Vec<SubscriptElement<'input>>,
    /// Optional mapping: `-> DimB` or `-> (DimB: b1, b2)`
    pub mapping: Option<SubscriptMapping<'input>>,
    pub loc: Loc,
}

/// Different equation types in Vensim MDL.
///
/// Each variant includes enough location info for error reporting.
#[derive(Clone, Debug, PartialEq)]
pub enum Equation<'input> {
    /// Regular equation: `lhs = expr`
    Regular(Lhs<'input>, Expr<'input>),
    /// Empty RHS: `lhs =` with nothing after equals.
    ///
    /// Vensim treats this as "A FUNCTION OF" placeholder.
    /// Includes `eq_loc` for the position of the `=` sign.
    EmptyRhs(Lhs<'input>, Loc),
    /// Implicit lookup: `lhs` alone with no `=` or table data.
    ///
    /// This is an exogenous data entry. During conversion, this must be
    /// transformed to a lookup on TIME with a default table of `(0,1),(1,1)`.
    /// This matches xmutil's `AddTable()` behavior when `tbl` is NULL.
    ///
    /// Example conversion pseudocode:
    /// ```text
    /// // Create default table
    /// let mut table = LookupTable::new(loc);
    /// table.add_pair(0.0, 1.0);
    /// table.add_pair(1.0, 1.0);
    /// // Create TIME reference as input
    /// let input = Expr::Var("TIME", ...);
    /// // Result is: lhs = LOOKUP(TIME, table)
    /// ```
    Implicit(Lhs<'input>),
    /// Lookup definition: `lhs(table_pairs)`
    Lookup(Lhs<'input>, LookupTable),
    /// WITH LOOKUP: `lhs = WITH LOOKUP(input, (table))`
    WithLookup(Lhs<'input>, Box<Expr<'input>>, LookupTable),
    /// Data equation: `lhs := expr` or `lhs :DATA: expr`
    Data(Lhs<'input>, Option<Expr<'input>>),
    /// Tabbed array: `lhs = TABBED ARRAY(values)`
    ///
    /// Values are stored as a flat vector, matching xmutil behavior which
    /// discards row boundaries. The parser flattens the 2D input.
    TabbedArray(Lhs<'input>, Vec<f64>),
    /// Number list: `lhs = num1, num2, num3`
    ///
    /// When the RHS is a comma-separated list of numbers (exprlist with
    /// multiple items that are all numbers), it becomes a NumberList.
    /// This is different from TABBED ARRAY which uses the explicit keyword.
    NumberList(Lhs<'input>, Vec<f64>),
    /// Subscript definition: `DimA: elem1, elem2, ...`
    SubscriptDef(Cow<'input, str>, SubscriptDef<'input>),
    /// Equivalence: `DimA <-> DimB`
    Equivalence(Cow<'input, str>, Cow<'input, str>, Loc),
}

/// Unit expression in units section.
#[derive(Clone, Debug, PartialEq)]
pub enum UnitExpr<'input> {
    /// Simple unit name: `Year`, `Widgets`
    Unit(Cow<'input, str>, Loc),
    /// Unit multiplication: `A * B`
    Mul(Box<UnitExpr<'input>>, Box<UnitExpr<'input>>, Loc),
    /// Unit division: `A / B`
    Div(Box<UnitExpr<'input>>, Box<UnitExpr<'input>>, Loc),
}

/// Optional range on units: `[min, max]` or `[min, max, step]`
///
/// The `?` character in Vensim maps to `None` for the corresponding field.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitRange {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub step: Option<f64>,
}

/// Units with optional range.
#[derive(Clone, Debug, PartialEq)]
pub struct Units<'input> {
    pub expr: Option<UnitExpr<'input>>,
    pub range: Option<UnitRange>,
    pub loc: Loc,
}

/// Complete equation with units and comment.
#[derive(Clone, Debug, PartialEq)]
pub struct FullEquation<'input> {
    pub equation: Equation<'input>,
    pub units: Option<Units<'input>>,
    pub comment: Option<Cow<'input, str>>,
    /// True if marked with `:SUP` or `:SUPPLEMENTARY` flag.
    /// Indicates the variable is intentionally not connected to feedback loops.
    pub supplementary: bool,
    pub loc: Loc,
}

/// Group marker: `{**GroupName**}` or `*** GroupName ***`
///
/// The full group name is preserved, including any numeric prefix that
/// indicates hierarchy (e.g., "1 Control", "2 Data"). Hierarchy reconstruction
/// is performed during conversion by examining these prefixes.
#[derive(Clone, Debug, PartialEq)]
pub struct Group<'input> {
    pub name: Cow<'input, str>,
    pub loc: Loc,
}

/// Macro definition: `:MACRO: name(args) ... :END OF MACRO:`
///
/// Note: The grammar parses macro arguments as `exprlist`, which allows
/// arbitrary expressions. In valid macros, these should be simple variable
/// references (parameter names). Validation happens during conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct MacroDef<'input> {
    pub name: Cow<'input, str>,
    /// Macro parameters as parsed expressions.
    ///
    /// The parser accepts an exprlist for macro arguments. In valid macros,
    /// each should be a simple `Expr::Var` (variable name). During conversion,
    /// these are validated and the names extracted.
    pub args: Vec<Expr<'input>>,
    pub equations: Vec<FullEquation<'input>>,
    pub loc: Loc,
}

/// Top-level parsed item from MDL file.
#[derive(Clone, Debug, PartialEq)]
pub enum MdlItem<'input> {
    Equation(Box<FullEquation<'input>>),
    Group(Group<'input>),
    Macro(Box<MacroDef<'input>>),
    /// End of equations marker (`\\\\---///` or `///---\\\`)
    ///
    /// Loc is needed for error ranges and view-section boundary tracking.
    EqEnd(Loc),
}

/// Terminal type indicating what follows an equation section.
///
/// This is returned by the parser to indicate what terminal was seen,
/// allowing the reader to determine whether to capture a comment.
#[derive(Clone, Debug, PartialEq)]
pub enum SectionEnd<'input> {
    /// Second `~` seen - comment section follows
    Tilde,
    /// `|` seen - no comment, next equation
    Pipe,
    /// End of equations marker
    EqEnd(Loc),
    /// Group marker with name
    GroupStar(Cow<'input, str>, Loc),
    /// Macro definition start with name and arguments
    MacroStart(Cow<'input, str>, Vec<Expr<'input>>, Loc),
    /// Macro definition end
    MacroEnd(Loc),
}

/// Expression list result - tracks whether we have a single expr or multiple.
///
/// Used by the parser to determine whether to create a Regular equation
/// (single expression) or NumberList equation (multiple numeric literals).
#[derive(Clone, Debug, PartialEq)]
pub enum ExprListResult<'input> {
    Single(Expr<'input>),
    Multiple(Vec<Expr<'input>>),
}

impl<'input> ExprListResult<'input> {
    /// Append an expression to the list.
    pub fn append(self, e: Expr<'input>) -> Self {
        match self {
            ExprListResult::Single(first) => ExprListResult::Multiple(vec![first, e]),
            ExprListResult::Multiple(mut v) => {
                v.push(e);
                ExprListResult::Multiple(v)
            }
        }
    }

    /// Convert to a vector of expressions.
    pub fn into_exprs(self) -> Vec<Expr<'input>> {
        match self {
            ExprListResult::Single(e) => vec![e],
            ExprListResult::Multiple(v) => v,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_loc_new() {
        let loc = Loc::new(10, 20);
        assert_eq!(loc.start, 10);
        assert_eq!(loc.end, 20);
    }

    #[test]
    fn test_loc_merge() {
        let a = Loc::new(5, 15);
        let b = Loc::new(10, 25);
        let merged = Loc::merge(a, b);
        assert_eq!(merged.start, 5);
        assert_eq!(merged.end, 25);
    }

    #[test]
    fn test_loc_merge_disjoint() {
        let a = Loc::new(100, 200);
        let b = Loc::new(10, 20);
        let merged = Loc::merge(a, b);
        assert_eq!(merged.start, 10);
        assert_eq!(merged.end, 200);
    }

    #[test]
    fn test_loc_default() {
        let loc = Loc::default();
        assert_eq!(loc.start, 0);
        assert_eq!(loc.end, 0);
    }

    #[test]
    fn test_lookup_table_new() {
        let loc = Loc::new(0, 10);
        let table = LookupTable::new(loc);
        assert!(table.x_vals.is_empty());
        assert!(table.y_vals.is_empty());
        assert!(table.x_range.is_none());
        assert!(table.y_range.is_none());
        assert_eq!(table.format, TableFormat::Pairs);
        assert!(!table.extrapolate);
    }

    #[test]
    fn test_lookup_table_new_legacy() {
        let loc = Loc::new(0, 10);
        let table = LookupTable::new_legacy(loc);
        assert_eq!(table.format, TableFormat::LegacyXY);
        assert!(!table.extrapolate);
    }

    #[test]
    fn test_lookup_table_add_pair() {
        let mut table = LookupTable::new(Loc::default());
        table.add_pair(1.0, 2.0);
        table.add_pair(3.0, 4.0);
        assert_eq!(table.x_vals, vec![1.0, 3.0]);
        assert_eq!(table.y_vals, vec![2.0, 4.0]);
    }

    #[test]
    fn test_lookup_table_set_range() {
        let mut table = LookupTable::new(Loc::default());
        table.set_range(0.0, 0.0, 100.0, 50.0);
        assert_eq!(table.x_range, Some((0.0, 100.0)));
        assert_eq!(table.y_range, Some((0.0, 50.0)));
    }

    #[test]
    fn test_lookup_table_add_raw() {
        let mut table = LookupTable::new_legacy(Loc::default());
        table.add_raw(1.0);
        table.add_raw(2.0);
        table.add_raw(10.0);
        table.add_raw(20.0);
        assert_eq!(table.x_vals, vec![1.0, 2.0, 10.0, 20.0]);
        assert!(table.y_vals.is_empty());
    }

    #[test]
    fn test_lookup_table_transform_legacy() {
        let mut table = LookupTable::new_legacy(Loc::default());
        // Legacy format: x1, x2, y1, y2 (first half = x, second half = y)
        table.add_raw(1.0);
        table.add_raw(2.0);
        table.add_raw(10.0);
        table.add_raw(20.0);
        assert!(table.transform_legacy().is_ok());
        assert_eq!(table.x_vals, vec![1.0, 2.0]);
        assert_eq!(table.y_vals, vec![10.0, 20.0]);
        assert_eq!(table.format, TableFormat::Pairs);
    }

    #[test]
    fn test_lookup_table_transform_legacy_odd_count() {
        let mut table = LookupTable::new_legacy(Loc::default());
        table.add_raw(1.0);
        table.add_raw(2.0);
        table.add_raw(3.0); // Odd count - should fail
        assert!(table.transform_legacy().is_err());
    }

    #[test]
    fn test_lookup_table_transform_legacy_wrong_format() {
        let mut table = LookupTable::new(Loc::default()); // Pairs format
        table.add_pair(1.0, 10.0);
        assert!(table.transform_legacy().is_err());
    }

    #[test]
    fn test_table_format_default() {
        let format = TableFormat::default();
        assert_eq!(format, TableFormat::Pairs);
    }

    #[test]
    fn test_unary_op_variants() {
        let ops = [UnaryOp::Positive, UnaryOp::Negative, UnaryOp::Not];
        assert_eq!(ops.len(), 3);
    }

    #[test]
    fn test_binary_op_variants() {
        let ops = [
            BinaryOp::Add,
            BinaryOp::Sub,
            BinaryOp::Mul,
            BinaryOp::Div,
            BinaryOp::Exp,
            BinaryOp::Lt,
            BinaryOp::Gt,
            BinaryOp::Lte,
            BinaryOp::Gte,
            BinaryOp::Eq,
            BinaryOp::Neq,
            BinaryOp::And,
            BinaryOp::Or,
        ];
        assert_eq!(ops.len(), 13);
    }

    #[test]
    fn test_interp_mode_variants() {
        let modes = [
            InterpMode::Interpolate,
            InterpMode::Raw,
            InterpMode::HoldBackward,
            InterpMode::LookForward,
        ];
        assert_eq!(modes.len(), 4);
    }

    #[test]
    fn test_call_kind_variants() {
        let kinds = [CallKind::Builtin, CallKind::Symbol];
        assert_eq!(kinds.len(), 2);
    }

    #[test]
    fn test_expr_const() {
        let expr = Expr::Const(42.5, Loc::new(0, 4));
        if let Expr::Const(val, loc) = expr {
            assert!((val - 42.5).abs() < 1e-10);
            assert_eq!(loc.start, 0);
            assert_eq!(loc.end, 4);
        } else {
            panic!("Expected Expr::Const");
        }
    }

    #[test]
    fn test_expr_var() {
        let expr = Expr::Var(Cow::Borrowed("x"), vec![], Loc::new(0, 1));
        if let Expr::Var(name, subscripts, _) = expr {
            assert_eq!(name.as_ref(), "x");
            assert!(subscripts.is_empty());
        } else {
            panic!("Expected Expr::Var");
        }
    }

    #[test]
    fn test_expr_app_builtin() {
        let expr = Expr::App(
            Cow::Borrowed("MAX"),
            vec![],
            vec![
                Expr::Var(Cow::Borrowed("a"), vec![], Loc::new(4, 5)),
                Expr::Var(Cow::Borrowed("b"), vec![], Loc::new(7, 8)),
            ],
            CallKind::Builtin,
            Loc::new(0, 9),
        );
        if let Expr::App(name, _, args, kind, _) = expr {
            assert_eq!(name.as_ref(), "MAX");
            assert_eq!(args.len(), 2);
            assert_eq!(kind, CallKind::Builtin);
        } else {
            panic!("Expected Expr::App");
        }
    }

    #[test]
    fn test_expr_app_symbol_lookup() {
        // Single arg symbol call = likely lookup invocation
        let expr = Expr::App(
            Cow::Borrowed("my_table"),
            vec![],
            vec![Expr::Var(Cow::Borrowed("x"), vec![], Loc::new(10, 11))],
            CallKind::Symbol,
            Loc::new(0, 12),
        );
        if let Expr::App(name, _, args, kind, _) = expr {
            assert_eq!(name.as_ref(), "my_table");
            assert_eq!(args.len(), 1);
            assert_eq!(kind, CallKind::Symbol);
        } else {
            panic!("Expected Expr::App");
        }
    }

    #[test]
    fn test_subscript_element() {
        let sub = Subscript::Element(Cow::Borrowed("DimA"), Loc::new(0, 4));
        if let Subscript::Element(name, _) = sub {
            assert_eq!(name.as_ref(), "DimA");
        } else {
            panic!("Expected Subscript::Element");
        }
    }

    #[test]
    fn test_subscript_bang_element() {
        let sub = Subscript::BangElement(Cow::Borrowed("elem1"), Loc::new(0, 6));
        if let Subscript::BangElement(name, _) = sub {
            assert_eq!(name.as_ref(), "elem1");
        } else {
            panic!("Expected Subscript::BangElement");
        }
    }

    #[test]
    fn test_lhs_simple() {
        let lhs = Lhs {
            name: Cow::Borrowed("my_var"),
            subscripts: vec![],
            except: None,
            interp_mode: None,
            loc: Loc::new(0, 6),
        };
        assert_eq!(lhs.name.as_ref(), "my_var");
        assert!(lhs.subscripts.is_empty());
        assert!(lhs.except.is_none());
        assert!(lhs.interp_mode.is_none());
    }

    #[test]
    fn test_lhs_with_subscripts() {
        let lhs = Lhs {
            name: Cow::Borrowed("arr"),
            subscripts: vec![
                Subscript::Element(Cow::Borrowed("DimA"), Loc::new(4, 8)),
                Subscript::Element(Cow::Borrowed("DimB"), Loc::new(10, 14)),
            ],
            except: None,
            interp_mode: None,
            loc: Loc::new(0, 15),
        };
        assert_eq!(lhs.subscripts.len(), 2);
    }

    #[test]
    fn test_equation_regular() {
        let lhs = Lhs {
            name: Cow::Borrowed("x"),
            subscripts: vec![],
            except: None,
            interp_mode: None,
            loc: Loc::new(0, 1),
        };
        let expr = Expr::Const(5.0, Loc::new(4, 5));
        let eq = Equation::Regular(lhs, expr);
        if let Equation::Regular(l, e) = eq {
            assert_eq!(l.name.as_ref(), "x");
            if let Expr::Const(val, _) = e {
                assert!((val - 5.0).abs() < 1e-10);
            } else {
                panic!("Expected Expr::Const");
            }
        } else {
            panic!("Expected Equation::Regular");
        }
    }

    #[test]
    fn test_equation_number_list() {
        let lhs = Lhs {
            name: Cow::Borrowed("arr"),
            subscripts: vec![Subscript::Element(Cow::Borrowed("DimA"), Loc::new(4, 8))],
            except: None,
            interp_mode: None,
            loc: Loc::new(0, 9),
        };
        let eq = Equation::NumberList(lhs, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        if let Equation::NumberList(l, nums) = eq {
            assert_eq!(l.name.as_ref(), "arr");
            assert_eq!(nums.len(), 5);
            assert!((nums[0] - 1.0).abs() < 1e-10);
            assert!((nums[4] - 5.0).abs() < 1e-10);
        } else {
            panic!("Expected Equation::NumberList");
        }
    }

    #[test]
    fn test_mapping_entry_name() {
        let entry = MappingEntry::Name(Cow::Borrowed("DimB"), Loc::new(0, 4));
        if let MappingEntry::Name(name, _) = entry {
            assert_eq!(name.as_ref(), "DimB");
        } else {
            panic!("Expected MappingEntry::Name");
        }
    }

    #[test]
    fn test_mapping_entry_dimension_mapping() {
        let entry = MappingEntry::DimensionMapping {
            dimension: Cow::Borrowed("DimB"),
            elements: vec![
                Subscript::Element(Cow::Borrowed("b1"), Loc::new(7, 9)),
                Subscript::Element(Cow::Borrowed("b2"), Loc::new(11, 13)),
            ],
            loc: Loc::new(0, 14),
        };
        if let MappingEntry::DimensionMapping {
            dimension,
            elements,
            ..
        } = entry
        {
            assert_eq!(dimension.as_ref(), "DimB");
            assert_eq!(elements.len(), 2);
        } else {
            panic!("Expected MappingEntry::DimensionMapping");
        }
    }

    #[test]
    fn test_subscript_mapping() {
        let mapping = SubscriptMapping {
            entries: vec![
                MappingEntry::Name(Cow::Borrowed("DimB"), Loc::new(3, 7)),
                MappingEntry::DimensionMapping {
                    dimension: Cow::Borrowed("DimC"),
                    elements: vec![
                        Subscript::Element(Cow::Borrowed("c1"), Loc::new(15, 17)),
                        Subscript::Element(Cow::Borrowed("c2"), Loc::new(19, 21)),
                    ],
                    loc: Loc::new(9, 22),
                },
            ],
            loc: Loc::new(0, 22),
        };
        assert_eq!(mapping.entries.len(), 2);
    }

    #[test]
    fn test_subscript_def() {
        let def = SubscriptDef {
            elements: vec![
                SubscriptElement::Element(Cow::Borrowed("a"), Loc::new(0, 1)),
                SubscriptElement::Element(Cow::Borrowed("b"), Loc::new(3, 4)),
            ],
            mapping: None,
            loc: Loc::new(0, 4),
        };
        assert_eq!(def.elements.len(), 2);
        assert!(def.mapping.is_none());
    }

    #[test]
    fn test_subscript_element_range() {
        let elem =
            SubscriptElement::Range(Cow::Borrowed("A1"), Cow::Borrowed("A10"), Loc::new(0, 8));
        if let SubscriptElement::Range(start, end, _) = elem {
            assert_eq!(start.as_ref(), "A1");
            assert_eq!(end.as_ref(), "A10");
        } else {
            panic!("Expected SubscriptElement::Range");
        }
    }

    #[test]
    fn test_unit_expr() {
        let unit = UnitExpr::Unit(Cow::Borrowed("Widgets"), Loc::new(0, 7));
        if let UnitExpr::Unit(name, _) = unit {
            assert_eq!(name.as_ref(), "Widgets");
        } else {
            panic!("Expected UnitExpr::Unit");
        }
    }

    #[test]
    fn test_unit_range() {
        let range = UnitRange {
            min: Some(0.0),
            max: Some(100.0),
            step: None,
        };
        assert_eq!(range.min, Some(0.0));
        assert_eq!(range.max, Some(100.0));
        assert!(range.step.is_none());
    }

    #[test]
    fn test_units() {
        let units = Units {
            expr: Some(UnitExpr::Unit(Cow::Borrowed("Year"), Loc::new(0, 4))),
            range: Some(UnitRange {
                min: Some(0.0),
                max: None,
                step: None,
            }),
            loc: Loc::new(0, 10),
        };
        assert!(units.expr.is_some());
        assert!(units.range.is_some());
    }

    #[test]
    fn test_full_equation() {
        let lhs = Lhs {
            name: Cow::Borrowed("x"),
            subscripts: vec![],
            except: None,
            interp_mode: None,
            loc: Loc::new(0, 1),
        };
        let eq = FullEquation {
            equation: Equation::EmptyRhs(lhs, Loc::new(2, 3)),
            units: None,
            comment: Some(Cow::Borrowed("A placeholder")),
            supplementary: false,
            loc: Loc::new(0, 20),
        };
        assert!(eq.units.is_none());
        assert_eq!(eq.comment.as_ref().unwrap().as_ref(), "A placeholder");
    }

    #[test]
    fn test_group() {
        let group = Group {
            name: Cow::Borrowed("1 Control"),
            loc: Loc::new(0, 15),
        };
        // Name includes numeric prefix for hierarchy
        assert_eq!(group.name.as_ref(), "1 Control");
    }

    #[test]
    fn test_macro_def() {
        let mac = MacroDef {
            name: Cow::Borrowed("MYMACRO"),
            args: vec![
                Expr::Var(Cow::Borrowed("input"), vec![], Loc::new(8, 13)),
                Expr::Var(Cow::Borrowed("dt"), vec![], Loc::new(15, 17)),
            ],
            equations: vec![],
            loc: Loc::new(0, 50),
        };
        assert_eq!(mac.name.as_ref(), "MYMACRO");
        assert_eq!(mac.args.len(), 2);
        // Verify args are Expr::Var (valid parameter names)
        for arg in &mac.args {
            assert!(matches!(arg, Expr::Var(_, _, _)));
        }
        assert!(mac.equations.is_empty());
    }

    #[test]
    fn test_mdl_item_variants() {
        let eq_end = MdlItem::EqEnd(Loc::new(0, 9));
        if let MdlItem::EqEnd(loc) = eq_end {
            assert_eq!(loc.end, 9);
        } else {
            panic!("Expected MdlItem::EqEnd");
        }

        let group = MdlItem::Group(Group {
            name: Cow::Borrowed("Test"),
            loc: Loc::default(),
        });
        assert!(matches!(group, MdlItem::Group(_)));
    }
}
