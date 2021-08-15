// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::result::Result as StdResult;

use lalrpop_util::ParseError;

use crate::common::{ElementName, EquationError, Ident};
use crate::datamodel::Dimension;
use crate::token::LexerType;

// equations are strings typed by humans for a single
// variable -- u16 is long enough
#[derive(PartialEq, Clone, Copy, Debug, Default)]
pub struct Loc {
    pub start: u16,
    pub end: u16,
}

impl Loc {
    pub fn new(start: usize, end: usize) -> Self {
        Loc {
            start: start as u16,
            end: end as u16,
        }
    }

    pub fn union(&self, rhs: &Self) -> Self {
        Loc {
            start: self.start.min(rhs.start),
            end: self.end.max(rhs.end),
        }
    }
}

#[test]
fn test_loc_basics() {
    let a = Loc { start: 3, end: 7 };
    assert_eq!(a, Loc::new(3, 7));

    let b = Loc { start: 4, end: 11 };
    assert_eq!(Loc::new(3, 11), a.union(&b));

    let c = Loc { start: 1, end: 5 };
    assert_eq!(Loc::new(1, 7), a.union(&c));
}

// we use Boxs here because we may walk and update ASTs a number of times,
// and we want to avoid copying and reallocating subexpressions all over
// the place.
#[derive(PartialEq, Clone, Debug)]
pub enum Expr {
    Const(String, f64, Loc),
    Var(Ident, Loc),
    App(Ident, Vec<Expr>, Loc),
    Subscript(Ident, Vec<Expr>, Loc),
    Op1(UnaryOp, Box<Expr>, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Loc),
}

impl Expr {
    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr::Const(s, n, _loc) => Expr::Const(s, n, loc),
            Expr::Var(v, _loc) => Expr::Var(v, loc),
            Expr::App(builtin, args, _loc) => Expr::App(
                builtin,
                args.into_iter().map(|arg| arg.strip_loc()).collect(),
                loc,
            ),
            Expr::Subscript(off, subscripts, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr::Subscript(off, subscripts, loc)
            }
            Expr::Op1(op, r, _loc) => Expr::Op1(op, Box::new(r.strip_loc()), loc),
            Expr::Op2(op, l, r, _loc) => {
                Expr::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr::If(cond, t, f, _loc) => Expr::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
        }
    }

    pub(crate) fn get_loc(&self) -> Loc {
        match self {
            Expr::Const(_, _, loc) => *loc,
            Expr::Var(_, loc) => *loc,
            Expr::App(_, _, loc) => *loc,
            Expr::Subscript(_, _, loc) => *loc,
            Expr::Op1(_, _, loc) => *loc,
            Expr::Op2(_, _, _, loc) => *loc,
            Expr::If(_, _, _, loc) => *loc,
        }
    }

    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Expr::Const(_s, _n, _loc) => None,
            Expr::Var(v, loc) if v == ident => Some(*loc),
            Expr::Var(_v, _loc) => None,
            Expr::App(_builtin, args, _loc) => {
                for arg in args.iter() {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr::Subscript(id, subscripts, loc) => {
                if id == ident {
                    let start = loc.start as usize;
                    return Some(Loc::new(start, start + id.len()));
                }
                for arg in subscripts.iter() {
                    if let Some(loc) = arg.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
            Expr::Op1(_op, r, _loc) => r.get_var_loc(ident),
            Expr::Op2(_op, l, r, _loc) => l.get_var_loc(ident).or_else(|| r.get_var_loc(ident)),
            Expr::If(cond, t, f, _loc) => cond
                .get_var_loc(ident)
                .or_else(|| t.get_var_loc(ident))
                .or_else(|| f.get_var_loc(ident)),
        }
    }
}

impl Default for Expr {
    fn default() -> Self {
        Expr::Const("0.0".to_string(), 0.0, Loc::default())
    }
}

pub fn parse_equation(
    eqn: &str,
    lexer_type: LexerType,
) -> StdResult<Option<Expr>, Vec<EquationError>> {
    let mut errs = Vec::new();

    let lexer = crate::token::Lexer::new(eqn, lexer_type);
    match crate::equation::EquationParser::new().parse(eqn, lexer) {
        Ok(ast) => Ok(Some(ast)),
        Err(err) => {
            use crate::common::ErrorCode::*;
            let err = match err {
                ParseError::InvalidToken { location: l } => EquationError {
                    start: l as u16,
                    end: (l + 1) as u16,
                    code: InvalidToken,
                },
                ParseError::UnrecognizedEOF {
                    location: l,
                    expected: _e,
                } => {
                    // if we get an EOF at position 0, that simply means
                    // we have an empty (or comment-only) equation.  Its not
                    // an _error_, but we also don't have an AST
                    if l == 0 {
                        return Ok(None);
                    }
                    // TODO: we can give a more precise error message here, including what
                    //   types of tokens would be ok
                    EquationError {
                        start: l as u16,
                        end: (l + 1) as u16,
                        code: UnrecognizedEof,
                    }
                }
                ParseError::UnrecognizedToken {
                    token: (l, _t, r), ..
                } => EquationError {
                    start: l as u16,
                    end: r as u16,
                    code: UnrecognizedToken,
                },
                ParseError::ExtraToken {
                    token: (l, _t, r), ..
                } => EquationError {
                    start: l as u16,
                    end: r as u16,
                    code: ExtraToken,
                },
                ParseError::User { error: e } => e,
            };

            errs.push(err);

            Err(errs)
        }
    }
}

#[test]
fn test_parse() {
    use crate::ast::BinaryOp::*;
    use crate::ast::Expr::*;

    let if1 = Box::new(If(
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Const("3".to_string(), 3.0, Loc::default())),
        Loc::default(),
    ));

    let if2 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("blerg".to_string(), Loc::default())),
            Box::new(Var("foo".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("2".to_string(), 2.0, Loc::default())),
        Box::new(Const("3".to_string(), 3.0, Loc::default())),
        Loc::default(),
    ));

    let if3 = Box::new(If(
        Box::new(Op2(
            Eq,
            Box::new(Var("quotient".to_string(), Loc::default())),
            Box::new(Var("quotient_target".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    ));

    let if4 = Box::new(If(
        Box::new(Op2(
            And,
            Box::new(Var("true_input".to_string(), Loc::default())),
            Box::new(Var("false_input".to_string(), Loc::default())),
            Loc::default(),
        )),
        Box::new(Const("1".to_string(), 1.0, Loc::default())),
        Box::new(Const("0".to_string(), 0.0, Loc::default())),
        Loc::default(),
    ));

    let quoting_eq = Box::new(Op2(
        Eq,
        Box::new(Var("oh_dear".to_string(), Loc::default())),
        Box::new(Var("oh_dear".to_string(), Loc::default())),
        Loc::default(),
    ));

    let subscript1 = Box::new(Subscript(
        "a".to_owned(),
        vec![Const("1".to_owned(), 1.0, Loc::default())],
        Loc::default(),
    ));
    let subscript2 = Box::new(Subscript(
        "a".to_owned(),
        vec![
            Const("2".to_owned(), 2.0, Loc::default()),
            App(
                "int".to_owned(),
                vec![Var("b".to_owned(), Loc::default())],
                Loc::default(),
            ),
        ],
        Loc::default(),
    ));

    use crate::ast::print_eqn;

    let cases = [
        ("if 1 then 2 else 3", if1, "if (1) then (2) else (3)"),
        (
            "if blerg = foo then 2 else 3",
            if2,
            "if ((blerg = foo)) then (2) else (3)",
        ),
        (
            "IF quotient = quotient_target THEN 1 ELSE 0",
            if3.clone(),
            "if ((quotient = quotient_target)) then (1) else (0)",
        ),
        (
            "(IF quotient = quotient_target THEN 1 ELSE 0)",
            if3.clone(),
            "if ((quotient = quotient_target)) then (1) else (0)",
        ),
        (
            "( IF true_input and false_input THEN 1 ELSE 0 )",
            if4.clone(),
            "if ((true_input && false_input)) then (1) else (0)",
        ),
        (
            "( IF true_input && false_input THEN 1 ELSE 0 )",
            if4.clone(),
            "if ((true_input && false_input)) then (1) else (0)",
        ),
        (
            "\"oh dear\" = oh_dear",
            quoting_eq.clone(),
            "(oh_dear = oh_dear)",
        ),
        ("a[1]", subscript1.clone(), "a[1]"),
        ("a[2, INT(b)]", subscript2.clone(), "a[2, int(b)]"),
    ];

    for case in cases.iter() {
        let eqn = case.0;
        let ast = parse_equation(eqn, LexerType::Equation).unwrap();
        assert!(ast.is_some());
        let ast = ast.unwrap().strip_loc();
        assert_eq!(&*case.1, &ast);
        let printed = print_eqn(&ast);
        assert_eq!(case.2, &printed);
    }

    let ast = parse_equation("NAN", LexerType::Equation).unwrap();
    assert!(ast.is_some());
    let ast = ast.unwrap();
    assert!(matches!(&ast, Expr::Const(_, _, _)));
    if let Expr::Const(id, n, _) = &ast {
        assert_eq!("NaN", id);
        assert!(n.is_nan());
    }
    let printed = print_eqn(&ast);
    assert_eq!("NaN", &printed);
}

#[test]
fn test_parse_failures() {
    let failures = &[
        "(",
        "(3",
        "3 +",
        "3 *",
        "(3 +)",
        "call(a,",
        "call(a,1+",
        "if if",
        "if 1 then",
        "if then",
        "if 1 then 2 else",
    ];

    for case in failures {
        let err = parse_equation(case, LexerType::Equation).unwrap_err();
        assert!(err.len() > 0);
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Ast {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(Vec<Dimension>, HashMap<ElementName, Expr>),
}

impl Ast {
    pub(crate) fn get_var_loc(&self, ident: &str) -> Option<Loc> {
        match self {
            Ast::Scalar(expr) => expr.get_var_loc(ident),
            Ast::ApplyToAll(_, expr) => expr.get_var_loc(ident),
            Ast::Arrayed(_, subscripts) => {
                for (_, expr) in subscripts.iter() {
                    if let Some(loc) = expr.get_var_loc(ident) {
                        return Some(loc);
                    }
                }
                None
            }
        }
    }

    pub fn to_latex(&self) -> String {
        match self {
            Ast::Scalar(expr) => latex_eqn(expr),
            Ast::ApplyToAll(_, _expr) => "TODO(array)".to_owned(),
            Ast::Arrayed(_, _) => "TODO(array)".to_owned(),
        }
    }
}

pub trait Visitor<T> {
    fn walk(&mut self, e: &Expr) -> T;
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum BinaryOp {
    Add,
    Sub,
    Exp,
    Mul,
    Div,
    Mod,
    Gt,
    Lt,
    Gte,
    Lte,
    Eq,
    Neq,
    And,
    Or,
}

impl BinaryOp {
    // higher the precedence, the tighter the binding.
    // e.g. Mul.precedence() > Add.precedence()
    pub(crate) fn precedence(&self) -> u8 {
        // matches equation.lalrpop
        match self {
            BinaryOp::Add => 4,
            BinaryOp::Sub => 4,
            BinaryOp::Exp => 6,
            BinaryOp::Mul => 5,
            BinaryOp::Div => 5,
            BinaryOp::Mod => 5,
            BinaryOp::Gt => 3,
            BinaryOp::Lt => 3,
            BinaryOp::Gte => 3,
            BinaryOp::Lte => 3,
            BinaryOp::Eq => 2,
            BinaryOp::Neq => 2,
            BinaryOp::And => 1,
            BinaryOp::Or => 1,
        }
    }
}

fn child_needs_parens(parent: &Expr, child: &Expr) -> bool {
    match parent {
        // no children so doesn't matter
        Expr::Const(_, _, _) | Expr::Var(_, _) => false,
        // children are comma separated, so no ambiguity possible
        Expr::App(_, _, _) | Expr::Subscript(_, _, _) => false,
        Expr::Op1(_, _, _) => matches!(child, Expr::Op2(_, _, _, _)),
        Expr::Op2(parent_op, _, _, _) => match child {
            Expr::Const(_, _, _)
            | Expr::Var(_, _)
            | Expr::App(_, _, _)
            | Expr::Subscript(_, _, _)
            | Expr::If(_, _, _, _)
            | Expr::Op1(_, _, _) => false,
            // 3 * 2 + 1
            Expr::Op2(child_op, _, _, _) => {
                // if we have `3 * (2 + 3)`, the parent's precedence
                // is higher than the child and we need enclosing parens
                parent_op.precedence() > child_op.precedence()
            }
        },
        Expr::If(_, _, _, _) => false,
    }
}

fn paren_if_necessary(parent: &Expr, child: &Expr, eqn: String) -> String {
    if child_needs_parens(parent, child) {
        format!("({})", eqn)
    } else {
        eqn
    }
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Positive,
    Negative,
    Not,
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Const(s, _, _) => s.clone(),
            Expr::Var(id, _) => id.clone(),
            Expr::App(func, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr::Op1(op, l, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "!",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => "^",
                    BinaryOp::Mul => "*",
                    BinaryOp::Div => "/",
                    BinaryOp::Mod => "%",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("({} {} {})", l, op, r)
            }
            Expr::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);
                format!("if ({}) then ({}) else ({})", cond, t, f)
            }
        }
    }
}

pub fn print_eqn(expr: &Expr) -> String {
    let mut visitor = PrintVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_print_eqn() {
    assert_eq!(
        "(a + b)",
        print_eqn(&Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr::Op1(
            UnaryOp::Not,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr::Op1(
            UnaryOp::Positive,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        print_eqn(&Expr::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "lookup(a, 1.0)",
        print_eqn(&Expr::App(
            "lookup".to_string(),
            vec![
                Expr::Var("a".to_string(), Loc::new(7, 8)),
                Expr::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
            ],
            Loc::new(0, 14),
        ))
    );
}

struct LatexVisitor {}

impl Visitor<String> for LatexVisitor {
    fn walk(&mut self, expr: &Expr) -> String {
        match expr {
            Expr::Const(s, n, _) => {
                if n.is_nan() {
                    "\\mathrm{{NaN}}".to_owned()
                } else {
                    s.clone()
                }
            }
            Expr::Var(id, _) => {
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{}}}", id)
            }
            Expr::App(func, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr::Op1(op, l, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let op: &str = match op {
                    UnaryOp::Positive => "+",
                    UnaryOp::Negative => "-",
                    UnaryOp::Not => "\\neg ",
                };
                format!("{}{}", op, l)
            }
            Expr::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => {
                        return format!("{}^{{{}}}", l, r);
                    }
                    BinaryOp::Mul => "\\cdot",
                    BinaryOp::Div => {
                        return format!("\\frac{{{}}}{{{}}}", l, r);
                    }
                    BinaryOp::Mod => "%",
                    BinaryOp::Gt => ">",
                    BinaryOp::Lt => "<",
                    BinaryOp::Gte => ">=",
                    BinaryOp::Lte => "<=",
                    BinaryOp::Eq => "=",
                    BinaryOp::Neq => "!=",
                    BinaryOp::And => "&&",
                    BinaryOp::Or => "||",
                };
                format!("{} {} {}", l, op, r)
            }
            Expr::If(cond, t, f, _) => {
                let cond = self.walk(cond);
                let t = self.walk(t);
                let f = self.walk(f);

                format!(
                    "\\begin{{cases}}
                     {} & \\text{{if }} {} \\\\
                     {} & \\text{{else}}
                 \\end{{cases}}",
                    t, cond, f
                )
            }
        }
    }
}

pub fn latex_eqn(expr: &Expr) -> String {
    let mut visitor = LatexVisitor {};
    visitor.walk(expr)
}

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Var("a_c".to_string(), Loc::new(1, 2))),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "(\\mathrm{a\\_c} - 1) \\cdot \\mathrm{b}",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Op2(
                BinaryOp::Sub,
                Box::new(Expr::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr::Op2(
            BinaryOp::Mul,
            Box::new(Expr::Var("b".to_string(), Loc::new(5, 6))),
            Box::new(Expr::Op2(
                BinaryOp::Sub,
                Box::new(Expr::Var("a_c".to_string(), Loc::new(0, 0))),
                Box::new(Expr::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                Loc::new(0, 0),
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-\\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Negative,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Not,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr::Op1(
            UnaryOp::Positive,
            Box::new(Expr::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "4.7",
        latex_eqn(&Expr::Const("4.7".to_string(), 4.7, Loc::new(0, 3)))
    );
    assert_eq!(
        "\\operatorname{lookup}(\\mathrm{a}, 1.0)",
        latex_eqn(&Expr::App(
            "lookup".to_string(),
            vec![
                Expr::Var("a".to_string(), Loc::new(7, 8)),
                Expr::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
            ],
            Loc::new(0, 14),
        ))
    );
}
