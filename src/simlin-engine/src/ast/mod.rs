// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub use crate::builtins::Loc;
use std::collections::HashMap;

use crate::builtins::{BuiltinContents, UntypedBuiltinFn, walk_builtin_expr};
use crate::common::{CanonicalElementName, EquationResult, canonicalize};
use crate::dimensions::Dimension;
use crate::model::{ModelStage0, ScopeStage0};
use crate::variable::Variable;
use unicode_xid::UnicodeXID;

mod array_view;
mod expr0;
mod expr1;
mod expr2;
mod expr3;

pub use array_view::{ArrayView, SparseInfo};
pub use expr0::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
pub use expr1::Expr1;
#[allow(unused_imports)]
pub use expr2::{ArrayBounds, Expr2, Expr2Context, IndexExpr2};
#[allow(unused_imports)]
pub use expr3::{Expr3, Expr3LowerContext, IndexExpr3, Pass1Context};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, salsa::Update)]
pub enum Ast<Expr> {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(
        Vec<Dimension>,
        HashMap<CanonicalElementName, Expr>,
        Option<Expr>,
        bool,
    ),
}

impl Ast<Expr0> {
    /// Returns the source location of the first reference to the given
    /// canonical identifier, or None if not referenced.
    pub(crate) fn get_var_loc(&self, canonical_ident: &str) -> Option<Loc> {
        match self {
            Ast::Scalar(expr) => expr.get_var_loc(canonical_ident),
            Ast::ApplyToAll(_, expr) => expr.get_var_loc(canonical_ident),
            Ast::Arrayed(_, subscripts, ..) => {
                for (_, expr) in subscripts.iter() {
                    if let Some(loc) = expr.get_var_loc(canonical_ident) {
                        return Some(loc);
                    }
                }
                None
            }
        }
    }

    /// Render the equation as LaTeX using raw identifiers (canonicalized).
    pub fn to_latex(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn_expr0(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(..) => "TODO(array)".to_owned(),
        }
    }

    /// Render the equation as LaTeX with `\htmlData{eqnloc=…}` source-range
    /// annotations on every node (see [`latex_eqn_expr0_annotated`]). Requires
    /// KaTeX's `trust` option (scoped to `\htmlData`) to render.
    pub fn to_latex_annotated(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn_expr0_annotated(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(..) => "TODO(array)".to_owned(),
        }
    }
}

impl Ast<Expr2> {
    // Called from db.rs via salsa tracked functions; clippy can't follow salsa dispatch.
    #[allow(dead_code)]
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Ast::Scalar(expr) => expr.get_var_loc(ident),
            Ast::ApplyToAll(_, expr) => expr.get_var_loc(ident),
            Ast::Arrayed(_, subscripts, default_expr, _) => {
                for (_, expr) in subscripts.iter() {
                    if let Some(loc) = expr.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                if let Some(expr) = default_expr {
                    return expr.get_var_loc(ident);
                }
                None
            }
        }
    }

    pub fn to_latex(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(_, _, _, _) => "TODO(array)".to_owned(),
        }
    }
}

/// Context for AST lowering that provides dimension information from ScopeStage0
struct ArrayContext<'a> {
    scope: &'a ScopeStage0<'a>,
    model_name: &'a str,
    next_temp_id: u32,
    is_array: bool,
    /// When true, allows union of named dimensions (cross-product).
    /// Set inside array reduction builtins like SUM.
    allow_dimension_union: bool,
}

impl<'a> ArrayContext<'a> {
    fn new(scope: &'a ScopeStage0<'a>, model_name: &'a str) -> Self {
        Self {
            scope,
            model_name,
            next_temp_id: 0,
            is_array: false,
            allow_dimension_union: false,
        }
    }

    fn with_array_context(scope: &'a ScopeStage0<'a>, model_name: &'a str) -> Self {
        Self {
            scope,
            model_name,
            next_temp_id: 0,
            is_array: true,
            allow_dimension_union: false,
        }
    }

    fn get_model(&self, model_name: &str) -> Option<&'a ModelStage0> {
        self.scope.models.get(&*canonicalize(model_name)).copied()
    }

    fn get_variable(
        &self,
        model_name: &str,
        ident: &str,
    ) -> Option<&'a Variable<crate::datamodel::ModuleReference, Expr0>> {
        // Handle dotted notation for submodel variables
        if let Some(pos) = ident.find('·') {
            let submodel_module_name = &ident[..pos];
            let submodel_var = &ident[pos + '·'.len_utf8()..];

            // Get the module variable to find the submodel name
            let module_var = self
                .get_model(model_name)?
                .variables
                .get(&*canonicalize(submodel_module_name))?;
            if let Variable::Module {
                model_name: submodel_name,
                ..
            } = module_var
            {
                return self.get_variable(submodel_name.as_str(), submodel_var);
            }
            None
        } else {
            self.get_model(model_name)?
                .variables
                .get(&*canonicalize(ident))
        }
    }
}

impl<'a> Expr2Context for ArrayContext<'a> {
    fn get_dimensions(&self, ident: &str) -> Option<Vec<crate::dimensions::Dimension>> {
        // During AST lowering, we may encounter variables that don't exist yet
        // (e.g., in tests or when processing incomplete models)
        let var = self.get_variable(self.model_name, ident)?;
        var.get_dimensions().map(|dims| dims.to_vec())
    }

    fn allocate_temp_id(&mut self) -> u32 {
        let id = self.next_temp_id;
        self.next_temp_id += 1;
        id
    }

    fn is_dimension_name(&self, ident: &str) -> bool {
        // Check if this identifier is the name of a dimension
        self.scope.dimensions.is_dimension_name(ident)
    }

    fn is_array_context(&self) -> bool {
        self.is_array
    }

    fn get_dimension_len(&self, name: &crate::common::CanonicalDimensionName) -> Option<usize> {
        self.scope.dimensions.get(name).map(|dim| dim.len())
    }

    fn is_indexed_dimension(&self, name: &str) -> bool {
        let canonical_name = crate::common::CanonicalDimensionName::from_raw(name);
        self.scope
            .dimensions
            .get(&canonical_name)
            .map(|dim| matches!(dim, crate::dimensions::Dimension::Indexed(_, _)))
            .unwrap_or(false)
    }

    fn allow_dimension_union(&self) -> bool {
        self.allow_dimension_union
    }

    fn set_allow_dimension_union(&mut self, allow: bool) -> bool {
        let prev = self.allow_dimension_union;
        self.allow_dimension_union = allow;
        prev
    }

    fn has_mapping_to(&self, dim_name: &str, target: &str) -> bool {
        let dim_canonical = crate::common::CanonicalDimensionName::from_raw(dim_name);
        let target_canonical = crate::common::CanonicalDimensionName::from_raw(target);
        self.scope
            .dimensions
            .has_mapping_to(&dim_canonical, &target_canonical)
    }
}

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr2>> {
    match ast {
        Ast::Scalar(expr) => {
            let mut ctx = ArrayContext::new(scope, scope.model_name);
            Expr1::from(expr)
                .map(|expr| expr.constify_dimensions(scope))
                .and_then(|expr| Expr2::from(expr, &mut ctx))
                .map(Ast::Scalar)
        }
        Ast::ApplyToAll(dims, expr) => {
            let mut ctx = ArrayContext::with_array_context(scope, scope.model_name);
            Expr1::from(expr)
                .map(|expr| expr.constify_dimensions(scope))
                .and_then(|expr| Expr2::from(expr, &mut ctx))
                .map(|expr| Ast::ApplyToAll(dims, expr))
        }
        Ast::Arrayed(dims, elements, default_expr, apply_default_to_missing) => {
            let mut ctx = ArrayContext::with_array_context(scope, scope.model_name);
            let elements: EquationResult<HashMap<CanonicalElementName, Expr2>> = elements
                .into_iter()
                .map(|(id, expr)| {
                    match Expr1::from(expr)
                        .map(|expr| expr.constify_dimensions(scope))
                        .and_then(|expr| Expr2::from(expr, &mut ctx))
                    {
                        Ok(expr) => Ok((id, expr)),
                        Err(err) => Err(err),
                    }
                })
                .collect();
            let default_expr = match default_expr {
                Some(expr) => Some(
                    Expr1::from(expr)
                        .map(|expr| expr.constify_dimensions(scope))
                        .and_then(|expr| Expr2::from(expr, &mut ctx))?,
                ),
                None => None,
            };
            match elements {
                Ok(elements) => Ok(Ast::Arrayed(
                    dims,
                    elements,
                    default_expr,
                    apply_default_to_missing,
                )),
                Err(err) => Err(err),
            }
        }
    }
}

/// Visitors walk Expr ASTs.
pub trait Visitor<T> {
    fn walk_index(&mut self, e: &IndexExpr0) -> T;
    fn walk(&mut self, e: &Expr0) -> T;
}

/// Determine if a child expression needs parentheses given the parent's
/// precedence.
///
/// At equal precedence the operator's ASSOCIATIVITY decides which side must be
/// grouped: it is always the side the parser would NOT have chosen on its own.
///
/// * `^` associates right-to-left (XMILE 3.3.1), so it groups its LEFT child:
///   `(a ^ b) ^ c` must not become `a ^ b ^ c`, which re-parses as `a ^ (b ^ c)`
///   -- `(4^3)^2 = 4096` printed as `4^(3^2) = 262144`.
/// * Every other binary operator associates left-to-right, so it groups its
///   RIGHT child: `a - (b - c)` must not become `a - b - c`.
///
/// The right-child rule applies to `+` and `*` too, not just the obviously
/// non-associative `-` / `/` / `mod`. Floating-point addition and multiplication
/// are NOT associative, so re-printing `a + (b + c)` as `a + b + c` silently
/// reassociates the sum. Only an AST the parser would rebuild identically may go
/// unparenthesized.
fn needs_parens_for_op(parent_op: &BinaryOp, child_op: &BinaryOp, is_right_child: bool) -> bool {
    let parent_prec = parent_op.precedence();
    let child_prec = child_op.precedence();
    if parent_prec > child_prec {
        return true;
    }
    if parent_prec == child_prec {
        return if matches!(parent_op, BinaryOp::Exp) {
            !is_right_child
        } else {
            is_right_child
        };
    }
    false
}

/// Whether `op` is a PREFIX unary operator (as opposed to the postfix `'`).
///
/// A prefix binds looser than `^` and a postfix binds tighter than everything,
/// so the two need opposite parenthesization treatment.
fn is_prefix_unary(op: &UnaryOp) -> bool {
    !matches!(op, UnaryOp::Transpose)
}

/// Parenthesize an `Expr0` child of a unary/binary operator when the operator's
/// precedence, associativity, or the grammar requires it.
///
/// The engine re-parses `print_eqn` output constantly (LTM partial-equation
/// synthesis, `patch`'s equation normalization, `builtins_visitor`'s synthesized
/// helper auxes, the MDL writer), and the invariant is `parse(print(e)) == e` --
/// text that parses to a *different* AST is a silent semantic corruption. Three
/// grammar facts drive the non-precedence cases:
///
/// * **`If` is not an atom.** `parse_expr` accepts it only at the top of an
///   expression; everywhere else it must already be delimited (a paren group, a
///   call argument, a subscript). So an `If` directly under an operator ALWAYS
///   needs parens -- `-if (a) then (1) else (0)` is text `Expr0::new` rejects.
/// * **A prefix unary binds looser than `^`.** `(-a) ^ b` printed bare would
///   re-parse as `-(a ^ b)`: a sign flip, not a parse error.
/// * **The postfix transpose binds tighter than everything.** `(a + b)'` printed
///   bare would transpose only `b`. Its operand must additionally be one of the
///   things `parse_postfix` can reach: a `Const`, `Var`, or `Subscript`. A CALL is
///   NOT among them -- `parse_app` returns an `App` before `parse_postfix` ever
///   runs, so `abs(a)'` is a parse error while `(abs(a))'` is fine.
fn paren_if_necessary(parent: &Expr0, child: &Expr0, is_right_child: bool, eqn: String) -> String {
    let needs = match parent {
        Expr0::Const(_, _, _) | Expr0::Var(_, _) => false,
        Expr0::App(_, _) | Expr0::Subscript(_, _, _) => false,
        Expr0::Op1(UnaryOp::Transpose, _, _) => !matches!(
            child,
            Expr0::Const(_, _, _) | Expr0::Var(_, _) | Expr0::Subscript(_, _, _)
        ),
        Expr0::Op1(_, _, _) => matches!(child, Expr0::Op2(_, _, _, _) | Expr0::If(_, _, _, _)),
        Expr0::Op2(parent_op, _, _, _) => match child {
            Expr0::Op2(child_op, _, _, _) => {
                needs_parens_for_op(parent_op, child_op, is_right_child)
            }
            // Only the BASE of `^` needs it: the exponent is parsed as a full
            // unary expression, so `a ^ -b` already round-trips.
            Expr0::Op1(child_op, _, _) => {
                matches!(parent_op, BinaryOp::Exp) && !is_right_child && is_prefix_unary(child_op)
            }
            Expr0::If(_, _, _, _) => true,
            _ => false,
        },
        Expr0::If(_, _, _, _) => false,
    };
    if needs { format!("({eqn})") } else { eqn }
}

/// The LaTeX-rendering sibling of [`paren_if_necessary`]. LaTeX output is never
/// re-parsed, so this is a *readability* rule rather than a correctness one, but
/// it shares `needs_parens_for_op` and therefore the same associativity facts:
/// `(a^b)^c` must not render as the ambiguous `a^{b}^{c}`, and a negated base
/// must render as `(-a)^{b}` so a reader groups it the way the engine does.
///
/// The transpose-parent arm mirrors [`paren_if_necessary`]'s: `^T` renders as a
/// superscript, which visually binds to the single token it follows, so any
/// operand other than a `Const`/`Var`/`Subscript` must be grouped -- `(-a)'`
/// rendered as `-a^T` reads as negating the transpose, not transposing the
/// negation.
///
/// Every arm mirrors its [`paren_if_necessary`] twin, `If`-under-an-operator
/// included, even though nothing here re-parses. A model rendered through the
/// `Expr2` printer and the same model rendered through the `Expr0` one must not
/// disagree about where the parentheses go, and "these two arms differ because
/// only one of them has to be correct" is a distinction no reader of the output
/// can see. Pinned by `test_latex_printers_agree_on_if_under_an_operator`.
fn paren_if_necessary1(parent: &Expr2, child: &Expr2, is_right_child: bool, eqn: String) -> String {
    let needs = match parent {
        Expr2::Const(_, _, _) | Expr2::Var(_, _, _) => false,
        Expr2::App(_, _, _) | Expr2::Subscript(_, _, _, _) => false,
        Expr2::Op1(UnaryOp::Transpose, _, _, _) => !matches!(
            child,
            Expr2::Const(_, _, _) | Expr2::Var(_, _, _) | Expr2::Subscript(_, _, _, _)
        ),
        Expr2::Op1(_, _, _, _) => {
            matches!(child, Expr2::Op2(_, _, _, _, _) | Expr2::If(_, _, _, _, _))
        }
        Expr2::Op2(parent_op, _, _, _, _) => match child {
            Expr2::Op2(child_op, _, _, _, _) => {
                needs_parens_for_op(parent_op, child_op, is_right_child)
            }
            Expr2::Op1(child_op, _, _, _) => {
                matches!(parent_op, BinaryOp::Exp) && !is_right_child && is_prefix_unary(child_op)
            }
            Expr2::If(_, _, _, _, _) => true,
            _ => false,
        },
        Expr2::If(_, _, _, _, _) => false,
    };
    if needs { format!("({eqn})") } else { eqn }
}

/// How a binary operator is laid out in LaTeX. Most are a simple infix token
/// (`{l} <token> {r}`); exponentiation stacks the right operand as a
/// superscript and division uses `\frac`, neither of which has a single
/// operator glyph. Shared by every LaTeX-rendering path so the operator
/// strings can't drift between them.
enum BinaryOpLatex {
    Infix(&'static str),
    Superscript,
    Fraction,
}

/// LaTeX rendering for a binary operator. The tokens are chosen to be valid in
/// math mode and idiomatic: `mod` -> `\bmod` (`%` is the TeX comment
/// character, so the previous rendering silently ate the right operand),
/// `and`/`or` -> `\land`/`\lor` (`&` is an alignment tab outside an array
/// environment; `||` renders as bare bars), and the comparisons use the
/// proper relation symbols (`\neq`, `\geq`, `\leq`).
fn binary_op_latex(op: BinaryOp) -> BinaryOpLatex {
    use BinaryOpLatex::{Fraction, Infix, Superscript};
    match op {
        BinaryOp::Add => Infix("+"),
        BinaryOp::Sub => Infix("-"),
        BinaryOp::Mul => Infix("\\cdot"),
        BinaryOp::Exp => Superscript,
        BinaryOp::Div => Fraction,
        BinaryOp::Mod => Infix("\\bmod"),
        BinaryOp::Gt => Infix(">"),
        BinaryOp::Lt => Infix("<"),
        BinaryOp::Gte => Infix("\\geq"),
        BinaryOp::Lte => Infix("\\leq"),
        BinaryOp::Eq => Infix("="),
        BinaryOp::Neq => Infix("\\neq"),
        BinaryOp::And => Infix("\\land"),
        BinaryOp::Or => Infix("\\lor"),
    }
}

/// Check whether a canonicalized identifier needs double-quoting to be
/// re-parseable **in the equation language** (`LexerType::Equation`). Three ways
/// a name fails to be bare-spellable, one per clause below:
///
/// * a character outside XID_Start/XID_Continue (`$`, `⁚`, `/`);
/// * a FIRST character that is not `XID_Start`, even when every character is
///   alphanumeric (`1stock`, a legal quoted XMILE name: bare, the lexer reads
///   the number `1` then the identifier `stock`);
/// * a name the lexer resolves to a KEYWORD instead of an identifier
///   ([`crate::lexer::is_reserved_word`] -- `if`, `mod`, `nan`, ...).  XMILE
///   lets a modeler quote any name, so `"if"` is a legal variable and
///   canonicalization keeps it as `if`; printed bare it re-parses as the `if`
///   of a conditional (`nan` re-parses as the NaN *literal*), which is how a
///   `patch` rename silently rewrote a valid model into an unparseable one
///   (GH #976).  The predicate delegates to the lexer's own table rather than
///   restating it, so printer and lexer cannot disagree about what a keyword
///   is.
///
/// The units lexer shares that keyword table and differs only in also admitting
/// `$` inside identifiers, so this predicate is *conservative* -- never wrong --
/// if it is ever asked about a unit expression.  It is not today: every caller
/// prints equation text.
///
/// `pub(crate)` because this is the single "can this name be spelled bare"
/// predicate: `print_ident` uses it for the `print_eqn` path and
/// `ltm_augment::quote_ident` for LTM's generated guard forms. A second
/// implementation drifts -- `quote_ident` previously tested "alphanumeric or
/// `_`", which a leading digit satisfies, so an LTM equation mixed a
/// `print_eqn`-quoted `"1stock"` with a bare `1stock` and failed to parse.
pub(crate) fn needs_quoting(canonical: &str) -> bool {
    let mut chars = canonical.chars();
    match chars.next() {
        None => return true,
        Some(c) if !UnicodeXID::is_xid_start(c) && c != '_' => return true,
        _ => {}
    }
    for c in chars {
        if !UnicodeXID::is_xid_continue(c) && c != '_' {
            return true;
        }
    }
    crate::lexer::is_reserved_word(canonical)
}

/// Canonicalize an identifier for display, re-quoting if the canonical form
/// contains characters that can't appear in a bare identifier.
fn print_ident(raw: &str) -> String {
    let canonical = canonicalize(raw);
    if needs_quoting(&canonical) {
        format!("\"{}\"", canonical)
    } else {
        canonical.into_owned()
    }
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => {
                format!("*:{}", print_ident(id.as_str()))
            }
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::DimPosition(n, _) => format!("@{n}"),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => print_ident(id.as_str()),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", print_ident(id.as_str()), args.join(", "))
            }
            Expr0::Op1(op, l, _) => {
                // The operand is parenthesized through the shared rule in both
                // arms: the postfix `'` binds tighter than any operator, so a
                // non-atomic operand must be grouped (`(a + b)'`).
                let l = paren_if_necessary(expr, l, false, self.walk(l));
                match op {
                    UnaryOp::Transpose => format!("{l}'"),
                    // `not `, NOT `!`: the equation lexer has no `!` rule at all
                    // (GH #913), so the bang form was unparseable text.
                    UnaryOp::Not => format!("not {l}"),
                    UnaryOp::Positive => format!("+{l}"),
                    UnaryOp::Negative => format!("-{l}"),
                }
            }
            Expr0::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, false, self.walk(l));
                let r = paren_if_necessary(expr, r, true, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "mod",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    // `<>`, NOT `!=`: the lexer produces `Neq` only from `<>`
                    // (GH #913).
                    BinaryOp::Neq => "<>",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{l} {op} {r}")
            }
            Expr0::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("if ({cond}) then ({t}) else ({f})")
            }
        }
    }
}

pub fn print_eqn(expr: &Expr0) -> String {
    let mut visitor = PrintVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_print_eqn() {
    use crate::common::RawIdent;
    assert_eq!(
        "a + b",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a + b * c",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Mul,
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a * (b + c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Add,
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a - (b - c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Sub,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Sub,
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 11),
        ))
    );
    assert_eq!(
        "a / (b / c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Div,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Div,
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 11),
        ))
    );
    assert_eq!(
        "a mod (b mod c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Mod,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Mod,
                Box::new(Expr0::Var(RawIdent::new_from_str("b"), Loc::default())),
                Box::new(Expr0::Var(RawIdent::new_from_str("c"), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 15),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    // `not a`, not `!a`: the equation lexer has no `!` rule at all, so the
    // bang form was text `Expr0::new` could never accept (GH #913).
    assert_eq!(
        "not a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Not,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Positive,
            Box::new(Expr0::Var(RawIdent::new_from_str("a"), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        print_eqn(&Expr0::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr0::App(
            UntypedBuiltinFn(
                "lookup".to_string(),
                vec![
                    Expr0::Var(RawIdent::new_from_str("a"), Loc::new(7, 8)),
                    Expr0::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
                ]
            ),
            Loc::new(0, 14),
        ))
    );
}

/// `print_eqn`'s output is fed straight back into `Expr0::new` all over the
/// engine (LTM partial-equation synthesis, `patch`'s equation normalization,
/// `builtins_visitor`'s synthesized helper auxes, the MDL writer). The invariant
/// is therefore not merely "the text parses" but `parse(print(e)) == e`: text
/// that parses to a DIFFERENT AST is a silent semantic corruption, which is
/// strictly worse than a loud parse error.
#[cfg(test)]
fn assert_print_reparse_roundtrip(expr: &Expr0, expected_text: &str) {
    use crate::lexer::LexerType;

    let printed = print_eqn(expr);
    assert_eq!(expected_text, printed);
    let reparsed = Expr0::new(&printed, LexerType::Equation)
        .unwrap_or_else(|e| panic!("print_eqn output {printed:?} did not re-parse: {e:?}"))
        .expect("non-empty");
    assert_eq!(
        expr.clone().strip_loc(),
        reparsed.strip_loc(),
        "print_eqn output {printed:?} re-parsed to a DIFFERENT AST"
    );
}

#[cfg(test)]
fn t_var(name: &str) -> Expr0 {
    Expr0::Var(crate::common::RawIdent::new_from_str(name), Loc::default())
}

#[cfg(test)]
fn t_op2(op: BinaryOp, l: Expr0, r: Expr0) -> Expr0 {
    Expr0::Op2(op, Box::new(l), Box::new(r), Loc::default())
}

#[cfg(test)]
fn t_op1(op: UnaryOp, inner: Expr0) -> Expr0 {
    Expr0::Op1(op, Box::new(inner), Loc::default())
}

#[cfg(test)]
fn t_if() -> Expr0 {
    Expr0::If(
        Box::new(t_var("a")),
        Box::new(Expr0::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr0::Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    )
}

/// An `If` is not an atom in the equation grammar -- it is only legal at the top
/// of an expression, inside parentheses, or as a call argument -- so an `If`
/// sitting directly under a unary or binary operator must be parenthesized or
/// the printer emits text its own parser rejects (#912's defect class).
#[test]
fn test_print_eqn_parenthesizes_if_under_an_operator() {
    assert_print_reparse_roundtrip(
        &t_op1(UnaryOp::Negative, t_if()),
        "-(if (a) then (1) else (0))",
    );
    assert_print_reparse_roundtrip(
        &t_op2(
            BinaryOp::Add,
            Expr0::Const("1".to_string(), 1.0, Loc::default()),
            t_if(),
        ),
        "1 + (if (a) then (1) else (0))",
    );

    // A bare top-level `If`, and an `If` as a call argument, stay unwrapped:
    // both positions already accept it.
    assert_print_reparse_roundtrip(&t_if(), "if (a) then (1) else (0)");
    assert_print_reparse_roundtrip(
        &Expr0::App(
            UntypedBuiltinFn("abs".to_string(), vec![t_if()]),
            Loc::default(),
        ),
        "abs(if (a) then (1) else (0))",
    );
}

/// A stacked unary prefix prints unparenthesized (`--a`) and the parser accepts
/// it, so `print_eqn` needs no extra parens there (#912). Pinned because the
/// alternative -- parenthesizing -- would churn every MDL equation the writer
/// emits and move the corpus round-trip ratchets.
#[test]
fn test_print_eqn_stacked_unary_is_unparenthesized_and_reparses() {
    assert_print_reparse_roundtrip(
        &t_op1(UnaryOp::Negative, t_op1(UnaryOp::Negative, t_var("a"))),
        "--a",
    );
}

/// `^` is right-associative and binds tighter than any prefix operator, so the
/// side that needs grouping is the mirror image of the `-`/`/`/`mod` case:
///
///   * a LEFT `^` operand that is itself a `^` needs parens (`(a^b)^c`),
///     while the right one does not (`a ^ b ^ c` already means `a^(b^c)`);
///   * a LEFT operand carrying a prefix needs parens (`(-a) ^ b`), or the text
///     re-parses as `-(a^b)` -- a SIGN FLIP, `(-2)^2 = 4` printed as `-4`;
///   * a RIGHT operand carrying a prefix does not (`a ^ -b` is already correct,
///     the exponent is parsed as a full unary expression).
#[test]
fn test_print_eqn_exponent_associativity_and_unary_base() {
    let a = || t_var("a");
    let b = || t_var("b");
    let c = || t_var("c");

    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Exp, t_op2(BinaryOp::Exp, a(), b()), c()),
        "(a ^ b) ^ c",
    );
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Exp, a(), t_op2(BinaryOp::Exp, b(), c())),
        "a ^ b ^ c",
    );
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Exp, t_op1(UnaryOp::Negative, a()), b()),
        "(-a) ^ b",
    );
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Exp, a(), t_op1(UnaryOp::Negative, b())),
        "a ^ -b",
    );
    // A prefix OUTSIDE the `^` still prints bare (unary binds looser).
    assert_print_reparse_roundtrip(
        &t_op1(UnaryOp::Negative, t_op2(BinaryOp::Exp, a(), b())),
        "-(a ^ b)",
    );
    // A lower-precedence operand of `^` is grouped on either side.
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Exp, t_op2(BinaryOp::Mul, a(), b()), c()),
        "(a * b) ^ c",
    );
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Exp, a(), t_op2(BinaryOp::Mul, b(), c())),
        "a ^ (b * c)",
    );
}

/// Floating-point `+` and `*` are NOT associative, so a right-nested sum is a
/// different computation from a left-nested one. Re-printing `a + (b + c)` as
/// the bare `a + b + c` silently reassociates it -- exactly the corruption the
/// AST-equality property (`writer_proptest.rs`) exists to catch.
#[test]
fn test_print_eqn_preserves_right_nested_associative_operators() {
    let a = || t_var("a");
    let b = || t_var("b");
    let c = || t_var("c");

    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Add, a(), t_op2(BinaryOp::Add, b(), c())),
        "a + (b + c)",
    );
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Mul, a(), t_op2(BinaryOp::Mul, b(), c())),
        "a * (b * c)",
    );
    // The left-nested (parser-natural) shape still prints bare.
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Add, t_op2(BinaryOp::Add, a(), b()), c()),
        "a + b + c",
    );
    assert_print_reparse_roundtrip(
        &t_op2(BinaryOp::Mul, t_op2(BinaryOp::Mul, a(), b()), c()),
        "a * b * c",
    );
}

/// GH #913: the printer must spell operators the way the lexer reads them.
/// The equation lexer has no `!` rule at all (only the `not` keyword, and only
/// `<>` for inequality), so `!a` / `a != b` were unconditionally unparseable.
#[test]
fn test_print_eqn_not_and_neq_use_lexable_spellings() {
    assert_print_reparse_roundtrip(&t_op1(UnaryOp::Not, t_var("a")), "not a");
    assert_print_reparse_roundtrip(
        &t_op1(UnaryOp::Not, t_op1(UnaryOp::Not, t_var("a"))),
        "not not a",
    );
    assert_print_reparse_roundtrip(
        &t_op1(UnaryOp::Not, t_op2(BinaryOp::Gt, t_var("a"), t_var("b"))),
        "not (a > b)",
    );
    assert_print_reparse_roundtrip(&t_op2(BinaryOp::Neq, t_var("a"), t_var("b")), "a <> b");
}

/// GH #913: the postfix transpose arm bypassed `paren_if_necessary` entirely, so
/// `Op1(Transpose, a + b)` printed `a + b'` -- transposing only `b`. Transpose
/// binds tighter than every infix and prefix operator, so any non-atomic operand
/// must be grouped.
#[test]
fn test_print_eqn_parenthesizes_transpose_operand() {
    assert_print_reparse_roundtrip(
        &t_op1(
            UnaryOp::Transpose,
            t_op2(BinaryOp::Add, t_var("a"), t_var("b")),
        ),
        "(a + b)'",
    );
    assert_print_reparse_roundtrip(
        &t_op1(UnaryOp::Transpose, t_op1(UnaryOp::Negative, t_var("a"))),
        "(-a)'",
    );
    // A CALL operand needs parens for a different reason than precedence:
    // `parse_app` returns the `App` before `parse_postfix` runs, so `abs(a)'` is
    // a hard parse error (pinned by `test_illegal_transpose_on_function_result`)
    // while `(abs(a))'` re-parses to the same AST.
    assert_print_reparse_roundtrip(
        &t_op1(
            UnaryOp::Transpose,
            Expr0::App(
                UntypedBuiltinFn("abs".to_string(), vec![t_var("a")]),
                Loc::default(),
            ),
        ),
        "(abs(a))'",
    );
    // A bare identifier operand still prints without parens.
    assert_print_reparse_roundtrip(&t_op1(UnaryOp::Transpose, t_var("a")), "a'");
}

#[test]
fn test_print_eqn_quotes_special_identifiers() {
    use crate::common::RawIdent;

    // Identifiers with characters that aren't valid in bare identifiers
    // must be wrapped in double quotes so the output is re-parseable.
    assert_eq!(
        "\"$⁚ltm⁚link_score⁚x→y\"",
        print_eqn(&Expr0::Var(
            RawIdent::new_from_str("\"$⁚ltm⁚link_score⁚x→y\""),
            Loc::default(),
        ))
    );

    // Normal identifiers should NOT be quoted
    assert_eq!(
        "population",
        print_eqn(&Expr0::Var(
            RawIdent::new_from_str("population"),
            Loc::default(),
        ))
    );

    // Identifiers with slashes (from quoted Vensim names) need re-quoting
    assert_eq!(
        "\"a/d\"",
        print_eqn(&Expr0::Var(
            RawIdent::new_from_str("\"a/d\""),
            Loc::default(),
        ))
    );
}

struct LatexVisitor {}

impl LatexVisitor {
    fn walk_index(&mut self, expr: &IndexExpr2) -> String {
        match expr {
            IndexExpr2::Wildcard(_) => "*".to_string(),
            IndexExpr2::StarRange(id, _) => format!("*:{id}"),
            IndexExpr2::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr2::DimPosition(n, _) => format!("@{n}"),
            IndexExpr2::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr2) -> String {
        match expr {
            Expr2::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr2::Var(id, _, _) => {
                let id = str::replace(id.as_str(), "_", "\\_");
                format!("\\mathrm{{{id}}}")
            }
            Expr2::App(builtin, _, _) => {
                let mut args: Vec<String> = vec![];
                walk_builtin_expr(builtin, |contents| {
                    let arg = match contents {
                        BuiltinContents::Ident(id, _loc) => format!("\\mathrm{{{id}}}"),
                        // The lookup table identity is a printed argument too.
                        BuiltinContents::Expr(expr) | BuiltinContents::LookupTable(expr) => {
                            self.walk(expr)
                        }
                    };
                    args.push(arg);
                });
                let func = builtin.name();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr2::Subscript(id, args, _, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id.as_str(), args.join(", "))
            }
            Expr2::Op1(op, l, _, _) => {
                // The operand is grouped through the shared rule in both arms:
                // the postfix `^T` binds tighter than any infix, so `(a + b)^T`
                // must keep its parentheses or it reads as transposing only `b`.
                let l = paren_if_necessary1(expr, l, false, self.walk(l));
                match op {
                    UnaryOp::Transpose => format!("{l}^T"),
                    UnaryOp::Positive => format!("+{l}"),
                    UnaryOp::Negative => format!("-{l}"),
                    UnaryOp::Not => format!("\\neg {l}"),
                }
            }
            Expr2::Op2(op, l, r, _, _) => {
                let l = paren_if_necessary1(expr, l, false, self.walk(l));
                let r = paren_if_necessary1(expr, r, true, self.walk(r));
                match binary_op_latex(*op) {
                    BinaryOpLatex::Infix(token) => format!("{l} {token} {r}"),
                    BinaryOpLatex::Superscript => format!("{l}^{{{r}}}"),
                    BinaryOpLatex::Fraction => format!("\\frac{{{l}}}{{{r}}}"),
                }
            }
            Expr2::If(cond, t, f, _, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);

                format!(
                    "\\begin{{cases}}
                     {t} & \\text{{if }} {cond} \\\\
                     {f} & \\text{{else}}
                 \\end{{cases}}"
                )
            }
        }
    }
}

pub fn latex_eqn(expr: &Expr2) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

/// Render an Expr0 (pre-lowering) AST node as LaTeX.
///
/// Variable names are canonicalized (lowercased with underscores) to match
/// the Expr2-based `latex_eqn`. This avoids needing the full lowering
/// pipeline just for LaTeX display.
pub fn latex_eqn_expr0(expr: &Expr0) -> String {
    match expr {
        Expr0::Const(s, n, _) => {
            if n.is_nan() {
                "\\mathrm{{NaN}}".to_owned()
            } else {
                s.clone()
            }
        }
        Expr0::Var(raw, _) => {
            let id = canonicalize(raw.as_str());
            let id = str::replace(&id, "_", "\\_");
            format!("\\mathrm{{{id}}}")
        }
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            let rendered_args: Vec<String> = args.iter().map(latex_eqn_expr0).collect();
            format!("\\operatorname{{{}}}({})", name, rendered_args.join(", "))
        }
        Expr0::Subscript(raw, indices, _) => {
            let id = canonicalize(raw.as_str());
            let rendered: Vec<String> = indices
                .iter()
                .map(|idx| match idx {
                    IndexExpr0::Wildcard(_) => "*".to_string(),
                    IndexExpr0::StarRange(id, _) => {
                        format!("*:{}", canonicalize(id.as_str()))
                    }
                    IndexExpr0::Range(l, r, _) => {
                        format!("{}:{}", latex_eqn_expr0(l), latex_eqn_expr0(r))
                    }
                    IndexExpr0::DimPosition(n, _) => format!("@{n}"),
                    IndexExpr0::Expr(e) => latex_eqn_expr0(e),
                })
                .collect();
            format!("{id}[{}]", rendered.join(", "))
        }
        Expr0::Op1(op, inner_expr, _) => {
            // Every arm groups through the shared rule, transpose included: the
            // `^T` superscript binds to the token it follows, so `(a + b)'` left
            // ungrouped renders as `a + b^T` -- transposing `b` alone.
            let inner = paren_if_necessary(expr, inner_expr, false, latex_eqn_expr0(inner_expr));
            match op {
                UnaryOp::Transpose => format!("{inner}^T"),
                UnaryOp::Positive => format!("+{inner}"),
                UnaryOp::Negative => format!("-{inner}"),
                UnaryOp::Not => format!("\\neg {inner}"),
            }
        }
        Expr0::Op2(op, l, r, _) => {
            let l = paren_if_necessary(expr, l, false, latex_eqn_expr0(l));
            let r = paren_if_necessary(expr, r, true, latex_eqn_expr0(r));
            match binary_op_latex(*op) {
                BinaryOpLatex::Infix(token) => format!("{l} {token} {r}"),
                BinaryOpLatex::Superscript => format!("{l}^{{{r}}}"),
                BinaryOpLatex::Fraction => format!("\\frac{{{l}}}{{{r}}}"),
            }
        }
        Expr0::If(cond, t, f, _) => {
            let cond = latex_eqn_expr0(cond);
            let t = latex_eqn_expr0(t);
            let f = latex_eqn_expr0(f);
            format!(
                "\\begin{{cases}}
                     {t} & \\text{{if }} {cond} \\\\
                     {f} & \\text{{else}}
                 \\end{{cases}}"
            )
        }
    }
}

/// Wrap `inner` in a KaTeX `\htmlData{<attr>=START_END}` annotation. KaTeX
/// renders this as a span carrying `data-<attr>="START_END"`, giving the
/// half-open byte range `[START, END)` of the source equation text that the
/// span covers. `attr` is `eqnloc` for a syntax node (identifier, call,
/// sub-expression …) or `oploc` for the gap around an operator token -- the
/// distinction tells the equation-preview click handler whether the range may
/// have grouping parentheses at its edges to trim past. See
/// [`HtmlDataAttr`].
enum HtmlDataAttr {
    /// `data-eqnloc`: the byte range of a syntax node.
    Node,
    /// `data-oploc`: the byte range *between two operands*, holding an operator
    /// token plus any surrounding whitespace and grouping parentheses.
    Op,
}

fn latex_html_data(attr: HtmlDataAttr, start: u16, end: u16, inner: &str) -> String {
    let name = match attr {
        HtmlDataAttr::Node => "eqnloc",
        HtmlDataAttr::Op => "oploc",
    };
    format!("\\htmlData{{{name}={start}_{end}}}{{{inner}}}")
}

/// Like [`latex_eqn_expr0`], but every rendered node is wrapped in a
/// `\htmlData{eqnloc=START_END}` annotation giving the byte range it covers
/// in the source equation, and each binary/unary operator additionally gets
/// an annotation spanning the gap between its operands -- the operator token
/// plus any surrounding whitespace; the consumer trims that range to the
/// operator itself. Exponentiation (superscript) and division (`\frac`) have
/// no operator glyph, so they get only the whole-expression wrapper.
///
/// This is what the FFI's `simlin_model_get_latex_equation` returns; rendering
/// the result requires enabling KaTeX's `trust` option, scoped to `\htmlData`.
pub fn latex_eqn_expr0_annotated(expr: &Expr0) -> String {
    let loc = expr.get_loc();
    let inner = match expr {
        Expr0::Const(s, n, _) => {
            if n.is_nan() {
                "\\mathrm{{NaN}}".to_owned()
            } else {
                s.clone()
            }
        }
        Expr0::Var(raw, _) => {
            let id = canonicalize(raw.as_str());
            let id = str::replace(&id, "_", "\\_");
            format!("\\mathrm{{{id}}}")
        }
        Expr0::App(UntypedBuiltinFn(name, args), _) => {
            let rendered_args: Vec<String> = args.iter().map(latex_eqn_expr0_annotated).collect();
            format!("\\operatorname{{{}}}({})", name, rendered_args.join(", "))
        }
        Expr0::Subscript(raw, indices, _) => {
            let id = canonicalize(raw.as_str());
            let rendered: Vec<String> = indices
                .iter()
                .map(|idx| match idx {
                    IndexExpr0::Wildcard(_) => "*".to_string(),
                    IndexExpr0::StarRange(id, _) => {
                        format!("*:{}", canonicalize(id.as_str()))
                    }
                    IndexExpr0::Range(l, r, _) => {
                        format!(
                            "{}:{}",
                            latex_eqn_expr0_annotated(l),
                            latex_eqn_expr0_annotated(r)
                        )
                    }
                    IndexExpr0::DimPosition(n, _) => format!("@{n}"),
                    IndexExpr0::Expr(e) => latex_eqn_expr0_annotated(e),
                })
                .collect();
            format!("{id}[{}]", rendered.join(", "))
        }
        Expr0::Op1(op, inner_expr, _) => {
            // The operand groups through the shared rule in every arm -- see the
            // twin in `latex_eqn_expr0`. This function backs the FFI
            // `simlin_model_get_latex_equation`, so an ungrouped transpose operand
            // is what a user sees rendered.
            let inner_rendered = paren_if_necessary(
                expr,
                inner_expr,
                false,
                latex_eqn_expr0_annotated(inner_expr),
            );
            match op {
                // Postfix: there is no operator token BEFORE the operand to
                // annotate, so `^T` is emitted plain (as it always has been).
                UnaryOp::Transpose => format!("{inner_rendered}^T"),
                _ => {
                    let op_str: &str = match op {
                        UnaryOp::Positive => "+",
                        UnaryOp::Negative => "-",
                        UnaryOp::Not => "\\neg ",
                        UnaryOp::Transpose => unreachable!(), // handled above
                    };
                    // the operator token sits somewhere in [Op1.start, operand.start)
                    let op_anno = latex_html_data(
                        HtmlDataAttr::Op,
                        loc.start,
                        inner_expr.get_loc().start,
                        op_str,
                    );
                    format!("{op_anno}{inner_rendered}")
                }
            }
        }
        Expr0::Op2(op, l, r, _) => {
            let l_rendered = paren_if_necessary(expr, l, false, latex_eqn_expr0_annotated(l));
            let r_rendered = paren_if_necessary(expr, r, true, latex_eqn_expr0_annotated(r));
            match binary_op_latex(*op) {
                BinaryOpLatex::Superscript => format!("{l_rendered}^{{{r_rendered}}}"),
                BinaryOpLatex::Fraction => format!("\\frac{{{l_rendered}}}{{{r_rendered}}}"),
                BinaryOpLatex::Infix(token) => {
                    // the operator token sits somewhere in [L.end, R.start)
                    let op_anno = latex_html_data(
                        HtmlDataAttr::Op,
                        l.get_loc().end,
                        r.get_loc().start,
                        token,
                    );
                    format!("{l_rendered} {op_anno} {r_rendered}")
                }
            }
        }
        Expr0::If(cond, t, f, _) => {
            let cond_r = latex_eqn_expr0_annotated(cond);
            let t_r = latex_eqn_expr0_annotated(t);
            let f_r = latex_eqn_expr0_annotated(f);
            format!(
                "\\begin{{cases}}
                     {t_r} & \\text{{if }} {cond_r} \\\\
                     {f_r} & \\text{{else}}
                 \\end{{cases}}"
            )
        }
    };
    latex_html_data(HtmlDataAttr::Node, loc.start, loc.end, &inner)
}

#[test]
fn test_latex_eqn_binary_op_strings() {
    use crate::common::Ident;
    // Every binary operator that lacks a clean source-syntax-equals-TeX token
    // must render as something valid in math mode. Regression: `mod` used to
    // render as `%` (the TeX comment character, which ate the right operand);
    // `and`/`or` as `&&`/`||` (an alignment tab / bare bars).
    let bin = |op| {
        let l = Box::new(Expr2::Var(Ident::new("a"), None, Loc::new(0, 1)));
        let r = Box::new(Expr2::Var(Ident::new("b"), None, Loc::new(2, 3)));
        latex_eqn(&Expr2::Op2(op, l, r, None, Loc::new(0, 3)))
    };
    assert_eq!("\\mathrm{a} + \\mathrm{b}", bin(BinaryOp::Add));
    assert_eq!("\\mathrm{a} - \\mathrm{b}", bin(BinaryOp::Sub));
    assert_eq!("\\mathrm{a} \\cdot \\mathrm{b}", bin(BinaryOp::Mul));
    assert_eq!("\\mathrm{a} \\bmod \\mathrm{b}", bin(BinaryOp::Mod));
    assert_eq!("\\mathrm{a}^{\\mathrm{b}}", bin(BinaryOp::Exp));
    assert_eq!("\\frac{\\mathrm{a}}{\\mathrm{b}}", bin(BinaryOp::Div));
    assert_eq!("\\mathrm{a} > \\mathrm{b}", bin(BinaryOp::Gt));
    assert_eq!("\\mathrm{a} < \\mathrm{b}", bin(BinaryOp::Lt));
    assert_eq!("\\mathrm{a} \\geq \\mathrm{b}", bin(BinaryOp::Gte));
    assert_eq!("\\mathrm{a} \\leq \\mathrm{b}", bin(BinaryOp::Lte));
    assert_eq!("\\mathrm{a} = \\mathrm{b}", bin(BinaryOp::Eq));
    assert_eq!("\\mathrm{a} \\neq \\mathrm{b}", bin(BinaryOp::Neq));
    assert_eq!("\\mathrm{a} \\land \\mathrm{b}", bin(BinaryOp::And));
    assert_eq!("\\mathrm{a} \\lor \\mathrm{b}", bin(BinaryOp::Or));
}

#[test]
fn test_latex_eqn_expr0() {
    use crate::lexer::LexerType;
    let render =
        |eqn: &str| latex_eqn_expr0(&Expr0::new(eqn, LexerType::Equation).unwrap().unwrap());

    // the fixed operator tokens (same set as test_latex_eqn_binary_op_strings,
    // but via the Expr0 path)
    assert_eq!("\\mathrm{a} \\bmod \\mathrm{b}", render("a mod b"));
    assert_eq!("\\mathrm{a} \\land \\mathrm{b}", render("a and b"));
    assert_eq!("\\mathrm{a} \\lor \\mathrm{b}", render("a or b"));
    assert_eq!("\\mathrm{a} \\neq \\mathrm{b}", render("a <> b"));
    assert_eq!("\\mathrm{a} \\geq \\mathrm{b}", render("a >= b"));
    assert_eq!("\\mathrm{a} \\leq \\mathrm{b}", render("a <= b"));

    // precedence-preserving parentheses: a lower-precedence operand of a
    // higher-precedence operator (or of a unary operator) must be parenthesized
    // so the rendering means the same thing as the source.
    assert_eq!(
        "(\\mathrm{a} + \\mathrm{b}) \\cdot \\mathrm{c}",
        render("(a + b) * c")
    );
    assert_eq!("-(\\mathrm{a} + \\mathrm{b})", render("-(a + b)"));
    assert_eq!(
        "\\mathrm{a} - (\\mathrm{b} - \\mathrm{c})",
        render("a - (b - c)")
    );
}

#[test]
fn test_latex_eqn() {
    use crate::common::Ident;
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Var(Ident::new("a_c"), None, Loc::new(1, 2))),
            Box::new(Expr2::Var(Ident::new("b"), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(Ident::new("a_c"), None, Loc::new(1, 2))),
            Box::new(Expr2::Var(Ident::new("b"), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Op2(
                BinaryOp::Sub,
                Box::new(Expr2::Var(Ident::new("a_c"), None, Loc::new(0, 0))),
                Box::new(Expr2::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                None,
                Loc::new(0, 0),
            )),
            Box::new(Expr2::Var(Ident::new("b"), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var(Ident::new("b"), None, Loc::new(5, 6))),
            Box::new(Expr2::Op2(
                BinaryOp::Sub,
                Box::new(Expr2::Var(Ident::new("a_c"), None, Loc::new(0, 0))),
                Box::new(Expr2::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                None,
                Loc::new(0, 0),
            )),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Negative,
            Box::new(Expr2::Var(Ident::new("a"), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Not,
            Box::new(Expr2::Var(Ident::new("a"), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Positive,
            Box::new(Expr2::Var(Ident::new("a"), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr2::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr2::App(
            crate::builtins::BuiltinFn::Lookup(
                Box::new(Expr2::Var(Ident::new("a"), None, Default::default())),
                Box::new(Expr2::Const("1.0".to_owned(), 1.0, Default::default())),
                Default::default(),
            ),
            None,
            Loc::new(0, 14),
        ))
    );
}

#[test]
fn test_latex_eqn_expr0_annotated() {
    use crate::lexer::LexerType;

    let parse = |eqn: &str| Expr0::new(eqn, LexerType::Equation).unwrap().unwrap();

    // `incidents * avg` -- identifiers get `eqnloc`; the `*` operator gap " * "
    // (`[9,12)`) gets `oploc`, which the consumer trims to the `\cdot` itself.
    assert_eq!(
        "\\htmlData{eqnloc=0_15}{\\htmlData{eqnloc=0_9}{\\mathrm{incidents}} \\htmlData{oploc=9_12}{\\cdot} \\htmlData{eqnloc=12_15}{\\mathrm{avg}}}",
        latex_eqn_expr0_annotated(&parse("incidents * avg"))
    );

    // `not running` -- the `\neg` glyph's `oploc` spans `[0,4)` ("not "); the
    // consumer trims that to "not" for caret placement.
    assert_eq!(
        "\\htmlData{eqnloc=0_11}{\\htmlData{oploc=0_4}{\\neg }\\htmlData{eqnloc=4_11}{\\mathrm{running}}}",
        latex_eqn_expr0_annotated(&parse("not running"))
    );

    // identifier underscores are escaped as `\_` inside `\mathrm`; the `+`
    // operator gap " + " (`[3,6)`) gets `oploc`.
    assert_eq!(
        "\\htmlData{eqnloc=0_7}{\\htmlData{eqnloc=0_3}{\\mathrm{a\\_b}} \\htmlData{oploc=3_6}{+} \\htmlData{eqnloc=6_7}{\\mathrm{c}}}",
        latex_eqn_expr0_annotated(&parse("a_b + c"))
    );

    // function call -- each argument is itself annotated; the call's range
    // (`eqnloc`, not `oploc`) includes the closing paren, which must not be
    // trimmed.
    assert_eq!(
        "\\htmlData{eqnloc=0_9}{\\operatorname{min}(\\htmlData{eqnloc=4_5}{\\mathrm{a}}, \\htmlData{eqnloc=7_8}{\\mathrm{b}})}",
        latex_eqn_expr0_annotated(&parse("min(a, b)"))
    );

    // a fixed operator (`mod` -> `\bmod`); its `oploc` spans the gap " mod "
    assert_eq!(
        "\\htmlData{eqnloc=0_7}{\\htmlData{eqnloc=0_1}{\\mathrm{a}} \\htmlData{oploc=1_6}{\\bmod} \\htmlData{eqnloc=6_7}{\\mathrm{b}}}",
        latex_eqn_expr0_annotated(&parse("a mod b"))
    );

    // precedence parentheses are added outside the inner node's annotation;
    // the outer `*`'s `oploc` (`[6,10)` = ") * ") will be trimmed past the `)`.
    assert_eq!(
        "\\htmlData{eqnloc=1_11}{(\\htmlData{eqnloc=1_6}{\\htmlData{eqnloc=1_2}{\\mathrm{a}} \\htmlData{oploc=2_5}{+} \\htmlData{eqnloc=5_6}{\\mathrm{b}}}) \\htmlData{oploc=6_10}{\\cdot} \\htmlData{eqnloc=10_11}{\\mathrm{c}}}",
        latex_eqn_expr0_annotated(&parse("(a + b) * c"))
    );
}

/// All THREE LaTeX printers must group a transpose operand, not just the `Expr2`
/// one. `latex_eqn_expr0_annotated` backs the FFI `simlin_model_get_latex_equation`,
/// so an ungrouped operand is what a user sees rendered: `(a + b)'` displayed as
/// `a + b^T` claims the model transposes `b` alone.
#[test]
fn test_latex_printers_group_the_transpose_operand() {
    use crate::lexer::LexerType;

    let parse = |eqn: &str| Expr0::new(eqn, LexerType::Equation).unwrap().unwrap();

    let sum = parse("(a + b)'");
    assert_eq!(
        "(\\mathrm{a} + \\mathrm{b})^T",
        latex_eqn_expr0(&sum),
        "latex_eqn_expr0 must parenthesize a non-atomic transpose operand"
    );
    assert!(
        latex_eqn_expr0_annotated(&sum).contains("})^T"),
        "latex_eqn_expr0_annotated must parenthesize too, got {}",
        latex_eqn_expr0_annotated(&sum)
    );

    // A transpose of a PREFIX unary is the same hazard one level down: `(-a)'`
    // rendered as `-a^T` reads as negating the transpose.
    let negated = t_op1(UnaryOp::Transpose, t_op1(UnaryOp::Negative, t_var("a")));
    assert_eq!("(-\\mathrm{a})^T", latex_eqn_expr0(&negated));

    // An atom needs no parens.
    assert_eq!("\\mathrm{a}^T", latex_eqn_expr0(&parse("a'")));
}

/// The `Expr2` LaTeX printer's transpose arm must group a prefix-unary operand,
/// not only an `Op2` one -- its `Expr0` twin already emits `(-a)'` here.
#[test]
fn test_latex_eqn_groups_transpose_of_a_prefix_unary() {
    let a = Expr2::Var(crate::common::Ident::new("a"), None, Loc::default());
    let negated = Expr2::Op1(UnaryOp::Negative, Box::new(a), None, Loc::default());
    let transposed = Expr2::Op1(UnaryOp::Transpose, Box::new(negated), None, Loc::default());
    assert_eq!("(-\\mathrm{a})^T", latex_eqn(&transposed));
}

/// The `Expr0` and `Expr2` LaTeX printers must agree on where parentheses go.
/// `paren_if_necessary`'s `If`-under-an-operator rule exists for CORRECTNESS on
/// the `print_eqn` path it shares, but `paren_if_necessary1` has no such
/// obligation -- and dropping the rule there made the same model render two
/// different ways depending on which lowering stage it happened to be in.
#[test]
fn test_latex_printers_agree_on_if_under_an_operator() {
    use crate::common::Ident;
    use crate::lexer::LexerType;

    let cases2 = Expr2::If(
        Box::new(Expr2::Var(Ident::new("a"), None, Loc::default())),
        Box::new(Expr2::Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Expr2::Const("0".to_string(), 0.0, Loc::default())),
        None,
        Loc::default(),
    );
    let sum2 = Expr2::Op2(
        BinaryOp::Add,
        Box::new(Expr2::Var(Ident::new("b"), None, Loc::default())),
        Box::new(cases2),
        None,
        Loc::default(),
    );

    let expr0 = Expr0::new("b + (if a then 1 else 0)", LexerType::Equation)
        .unwrap()
        .unwrap();

    assert_eq!(
        latex_eqn_expr0(&expr0),
        latex_eqn(&sum2),
        "the two LaTeX printers must parenthesize an `If` operand identically"
    );
    assert!(
        latex_eqn(&sum2).contains("(\\begin{cases}"),
        "the cases block must be grouped, got {}",
        latex_eqn(&sum2)
    );
}

/// `parse(print_eqn(e)) == e` over the FULL operator set and over a NAME POOL
/// that reaches every clause of [`needs_quoting`].
///
/// The MDL writer's fixpoint proptest (`mdl::writer_proptest`) re-reads with
/// `mdl::parser`, whose binary precedence table is inverted (GH #914), so its
/// generator is restricted to arithmetic. That leaves `Not`, `Neq`, `Transpose`,
/// `Mod`, the comparisons, and `and`/`or` -- most of `print_eqn`'s surface, and
/// three of the four #913 shapes -- unexercised by any property. This module
/// re-parses with the XMILE grammar, which `print_eqn` targets, so it can cover
/// everything.
#[cfg(test)]
mod print_eqn_proptest {
    use super::*;
    use crate::common::RawIdent;
    use crate::lexer::{LexerType, Token};
    use proptest::prelude::*;

    /// Identifiers reaching every clause of [`needs_quoting`], so the round-trip
    /// property is sensitive to the quoting decision and not only to operator
    /// placement: bare-legal names, all eight equation-language KEYWORDS
    /// (GH #976), a leading-digit name (`1stock`, the case `17d4e7c0` fixed), a
    /// name carrying the synthetic `⁚`/`→` characters LTM mints, and a `·`
    /// module-qualified name (`XID_Continue`, so it stays bare).
    ///
    /// Spelled out rather than read from `lexer::KEYWORDS`: a fixture derived
    /// from the table under test could not notice that table losing an entry.
    const NAME_POOL: [&str; 13] = [
        "a", "b", "_c", "if", "then", "else", "not", "mod", "and", "or", "nan", "1stock", "m·out",
    ];

    /// Rewrite every identifier to its canonical form.
    ///
    /// `print_ident` canonicalizes as it prints, and a quoted name comes back
    /// from the parser with its quotes still attached to the `RawIdent`, so RAW
    /// ident equality is not the property `print_eqn` promises -- CANONICAL
    /// ident equality is. On the bare names this is the identity, so the
    /// operator coverage is unaffected. The match is exhaustive with no `_` arm,
    /// so a new `Expr0` variant is a compile error here.
    fn canonicalize_idents(expr: Expr0) -> Expr0 {
        fn canon(raw: &RawIdent) -> RawIdent {
            RawIdent::new(canonicalize(raw.as_str()).into_owned())
        }
        match expr {
            Expr0::Const(text, value, loc) => Expr0::Const(text, value, loc),
            Expr0::Var(id, loc) => Expr0::Var(canon(&id), loc),
            Expr0::App(UntypedBuiltinFn(func, args), loc) => Expr0::App(
                UntypedBuiltinFn(func, args.into_iter().map(canonicalize_idents).collect()),
                loc,
            ),
            Expr0::Subscript(id, args, loc) => Expr0::Subscript(
                canon(&id),
                args.into_iter().map(canonicalize_index_idents).collect(),
                loc,
            ),
            Expr0::Op1(op, l, loc) => Expr0::Op1(op, Box::new(canonicalize_idents(*l)), loc),
            Expr0::Op2(op, l, r, loc) => Expr0::Op2(
                op,
                Box::new(canonicalize_idents(*l)),
                Box::new(canonicalize_idents(*r)),
                loc,
            ),
            Expr0::If(c, t, f, loc) => Expr0::If(
                Box::new(canonicalize_idents(*c)),
                Box::new(canonicalize_idents(*t)),
                Box::new(canonicalize_idents(*f)),
                loc,
            ),
        }
    }

    fn canonicalize_index_idents(index: IndexExpr0) -> IndexExpr0 {
        match index {
            IndexExpr0::Wildcard(loc) => IndexExpr0::Wildcard(loc),
            IndexExpr0::StarRange(dim, loc) => {
                IndexExpr0::StarRange(RawIdent::new(canonicalize(dim.as_str()).into_owned()), loc)
            }
            IndexExpr0::Range(l, r, loc) => {
                IndexExpr0::Range(canonicalize_idents(l), canonicalize_idents(r), loc)
            }
            IndexExpr0::DimPosition(n, loc) => IndexExpr0::DimPosition(n, loc),
            IndexExpr0::Expr(e) => IndexExpr0::Expr(canonicalize_idents(e)),
        }
    }

    /// Every `BinaryOp`, so a new variant cannot be silently skipped: the match is
    /// exhaustive and adding a variant is a compile error here.
    fn all_binary_ops() -> Vec<BinaryOp> {
        use BinaryOp::*;
        let all = [
            Add, Sub, Mul, Div, Mod, Exp, Gt, Lt, Gte, Lte, Eq, Neq, And, Or,
        ];
        // Exhaustiveness guard: destructuring forces an update when a variant lands.
        for op in all {
            match op {
                Add | Sub | Mul | Div | Mod | Exp | Gt | Lt | Gte | Lte | Eq | Neq | And | Or => {}
            }
        }
        all.to_vec()
    }

    fn expr_strategy() -> impl Strategy<Value = Expr0> {
        let leaf = prop_oneof![
            prop::sample::select(&NAME_POOL[..])
                .prop_map(|n| Expr0::Var(RawIdent::new_from_str(n), Loc::default())),
            prop::sample::select(&[0.0f64, 1.0, 2.5][..]).prop_map(|v| Expr0::Const(
                format!("{v}"),
                v,
                Loc::default()
            )),
        ];

        leaf.prop_recursive(5, 64, 4, |inner| {
            let bin_op = prop::sample::select(all_binary_ops());
            let un_op = prop_oneof![
                Just(UnaryOp::Negative),
                Just(UnaryOp::Positive),
                Just(UnaryOp::Not),
                Just(UnaryOp::Transpose),
            ];
            prop_oneof![
                (bin_op, inner.clone(), inner.clone()).prop_map(|(op, l, r)| Expr0::Op2(
                    op,
                    Box::new(l),
                    Box::new(r),
                    Loc::default()
                )),
                (un_op, inner.clone()).prop_map(|(op, l)| Expr0::Op1(
                    op,
                    Box::new(l),
                    Loc::default()
                )),
                (
                    prop::sample::select(&["abs", "exp", "ln"][..]),
                    inner.clone()
                )
                    .prop_map(|(f, e)| Expr0::App(
                        UntypedBuiltinFn(f.to_owned(), vec![e]),
                        Loc::default()
                    )),
                (inner.clone(), inner.clone(), inner.clone()).prop_map(|(c, t, f)| Expr0::If(
                    Box::new(c),
                    Box::new(t),
                    Box::new(f),
                    Loc::default()
                )),
            ]
        })
    }

    /// Does `text` lex as ONE identifier covering the whole input?
    ///
    /// This is the lexer-side statement of "bare-spellable", derived by running
    /// the lexer rather than by restating its character classes -- which is the
    /// point: `needs_quoting` restates them, and the restatement was incomplete
    /// (GH #976).
    fn lexes_as_one_whole_ident(text: &str) -> bool {
        let mut lexer = crate::lexer::Lexer::new(text, LexerType::Equation);
        match (lexer.next(), lexer.next()) {
            (Some(Ok((start, Token::Ident(word), end))), None) => {
                start == 0 && end == text.len() && word == text
            }
            _ => false,
        }
    }

    /// Names in the shapes canonicalization can produce, plus arbitrary short
    /// strings over the alphabet those shapes draw from. `"` is excluded, and
    /// [`a_canonical_name_containing_a_quote_is_unspellable`] says why.
    fn name_strategy() -> impl Strategy<Value = String> {
        prop_oneof![
            prop::sample::select(&NAME_POOL[..]).prop_map(str::to_string),
            "[a-zA-Z0-9_·⁚$→]{1,6}",
        ]
    }

    /// The one name shape `needs_quoting` cannot rescue, pinned rather than
    /// quietly excluded from the property above.
    ///
    /// `canonicalize` preserves an embedded `"` (an XMILE `name="a&quot;b"`
    /// reaches the compiler as the canonical `a"b`), and `Lexer::quoted_ident`
    /// terminates on the FIRST `"` with no escape sequence of any kind -- so
    /// `a"b` has no bare spelling AND no quoted spelling. `needs_quoting`
    /// correctly says "quote it"; there is simply nothing to say.
    ///
    /// Such a name IS reachable -- the XMILE reader admits
    /// `<aux name="a&quot;b">` with zero diagnostics -- and a rename TO one
    /// used to persist a corrupted model through the same `patch.rs` path as
    /// GH #976: the rename reprints every dependent equation, so `c = a + 1`
    /// became `c = "x"y" + 1` and the previously-valid model stopped compiling
    /// with `UnclosedQuotedIdent`. That direction is now refused at the front
    /// door by `patch::apply_rename_variable`, which is where the loudness
    /// belongs; giving the name a spelling instead would mean an escape in the
    /// lexer's quoted-identifier rule, a language change and not a printer one.
    ///
    /// So what remains is exactly this: a name that can be DEFINED (by either
    /// reader) but never REFERENCED. This test is the characterization pin for
    /// that state, and it reds if the lexer ever grows an escape -- which is
    /// when `print_ident` needs revisiting.
    #[test]
    fn a_canonical_name_containing_a_quote_is_unspellable() {
        let canonical = canonicalize("a\"b");
        assert_eq!(
            "a\"b",
            canonical.as_ref(),
            "the quote survives canonicalization"
        );
        assert!(needs_quoting(&canonical));
        assert!(!lexes_as_one_whole_ident(&canonical), "no bare spelling");

        let printed = print_ident(&canonical);
        assert_eq!("\"a\"b\"", printed);
        assert!(
            !lexes_as_one_whole_ident(&printed),
            "no quoted spelling either: the lexer has no escape inside a quoted ident"
        );
    }

    proptest! {
        #[test]
        fn print_eqn_roundtrips_over_the_full_operator_set(expr in expr_strategy()) {
            let printed = print_eqn(&expr);
            let reparsed = Expr0::new(&printed, LexerType::Equation);
            prop_assert!(
                matches!(reparsed, Ok(Some(_))),
                "print_eqn output did not re-parse: {printed:?} ({reparsed:?})"
            );
            prop_assert_eq!(
                canonicalize_idents(expr.clone()).strip_loc(),
                canonicalize_idents(reparsed.unwrap().unwrap()).strip_loc(),
                "print_eqn output re-parsed to a DIFFERENT AST: {}",
                printed
            );
        }

        /// The completeness guard for [`needs_quoting`]: a name it calls
        /// bare-spellable must ACTUALLY lex as one identifier. Stating it against
        /// the lexer (rather than against a second copy of the character rules)
        /// is what makes the predicate checkable: every past hole here --
        /// leading digit, keyword -- was a clause the printer never knew about.
        ///
        /// The converse is deliberately not asserted. Over-quoting is always
        /// safe, and `ltm_augment::quote_ident` relies on that to keep quoting
        /// `·`-qualified names the lexer would happily read bare.
        #[test]
        fn a_bare_spellable_name_lexes_as_one_identifier(name in name_strategy()) {
            let canonical = canonicalize(&name);
            prop_assume!(!canonical.is_empty());
            if !needs_quoting(&canonical) {
                prop_assert!(
                    lexes_as_one_whole_ident(&canonical),
                    "needs_quoting says `{}` can be spelled bare, but the lexer does not \
                     read it as a single identifier",
                    canonical
                );
            }
        }
    }
}

#[cfg(test)]
mod ast_tests {
    use super::*;
    use crate::common::{Ident, canonicalize};
    use crate::datamodel;
    use crate::model::ModelStage0;
    use std::collections::HashMap;

    #[test]
    fn test_simple_expr2_context() {
        // Create a simple model with an array variable
        let dim = datamodel::Dimension::named(
            "region".to_string(),
            vec!["north".to_string(), "south".to_string()],
        );
        let array_var = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("population").into_owned(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["region".to_string()],
                "100".to_string(),
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            sim_specs: None,
            variables: vec![array_var],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
        let model_s0 = ModelStage0::new(
            &model_datamodel,
            std::slice::from_ref(&dim),
            &units_ctx,
            false,
        );

        let mut models = HashMap::new();
        models.insert(Ident::new("test_model"), &model_s0);

        let dims_ctx = crate::dimensions::DimensionsContext::from(&[dim]);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
            model_name: "test_model",
        };

        let mut ctx = ArrayContext::new(&scope, "test_model");

        // Test that we can get dimensions for the array variable
        let dims = ctx.get_dimensions("population");
        assert!(dims.is_some());
        let dims = dims.unwrap();
        assert_eq!(dims.len(), 1);
        assert_eq!(dims[0].name(), "region");
        assert_eq!(dims[0].len(), 2);

        // Test that scalar variables return None
        assert!(ctx.get_dimensions("nonexistent").is_none());

        // Test temp ID allocation
        assert_eq!(ctx.allocate_temp_id(), 0);
        assert_eq!(ctx.allocate_temp_id(), 1);
        assert_eq!(ctx.allocate_temp_id(), 2);
    }

    #[test]
    fn test_expr2_dimension_mismatch_errors() {
        use crate::ast::BinaryOp;
        use crate::ast::expr1::Expr1;
        use crate::common::{ErrorCode, Ident, canonicalize};

        // Create a model with array variables of different dimensions
        let dim1 = datamodel::Dimension::named(
            "region".to_string(),
            vec!["north".to_string(), "south".to_string()],
        );
        let dim2 = datamodel::Dimension::named(
            "product".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );

        let array_var1 = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("regional_data").into_owned(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["region".to_string()],
                "100".to_string(),
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });
        let array_var2 = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("product_data").into_owned(),
            equation: datamodel::Equation::ApplyToAll(
                vec!["product".to_string()],
                "50".to_string(),
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            sim_specs: None,
            variables: vec![array_var1, array_var2],
            views: vec![],
            loop_metadata: vec![],
            groups: vec![],
            macro_spec: None,
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).0;
        let model_s0 = ModelStage0::new(
            &model_datamodel,
            &[dim1.clone(), dim2.clone()],
            &units_ctx,
            false,
        );

        let mut models = HashMap::new();
        models.insert(Ident::new("test_model"), &model_s0);

        let dims_ctx = crate::dimensions::DimensionsContext::from(&[dim1, dim2]);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
            model_name: "test_model",
        };

        let mut ctx = ArrayContext::new(&scope, "test_model");

        // Test binary op with mismatched dimensions
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var(Ident::new("regional_data"), Loc::default())),
            Box::new(Expr1::Var(Ident::new("product_data"), Loc::default())),
            Loc::new(0, 10),
        );
        let result = Expr2::from(add_expr, &mut ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::MismatchedDimensions);

        // Test if expression with mismatched dimensions
        let if_expr = Expr1::If(
            Box::new(Expr1::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr1::Var(Ident::new("regional_data"), Loc::default())),
            Box::new(Expr1::Var(Ident::new("product_data"), Loc::default())),
            Loc::new(0, 20),
        );
        let result = Expr2::from(if_expr, &mut ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::MismatchedDimensions);
    }
}
