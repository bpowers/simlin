// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

pub use crate::builtins::Loc;
use std::collections::HashMap;

use crate::builtins::{BuiltinContents, UntypedBuiltinFn, walk_builtin_expr};
use crate::common::{ElementName, EquationResult};
use crate::datamodel::Dimension;
use crate::model::{ModelStage0, ScopeStage0};
use crate::variable::Variable;

mod expr0;
mod expr1;
mod expr2;

pub use expr0::{BinaryOp, Expr0, IndexExpr0, UnaryOp};
pub use expr1::Expr1;
#[allow(unused_imports)]
pub use expr2::{ArraySource, ArrayView, Expr2, Expr2Context, IndexExpr2, StridedDimension};

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ast<Expr> {
    Scalar(Expr),
    ApplyToAll(Vec<Dimension>, Expr),
    Arrayed(Vec<Dimension>, HashMap<ElementName, Expr>),
}

impl Ast<Expr2> {
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

/// Context for AST lowering that provides dimension information from ScopeStage0
struct ArrayContext<'a> {
    scope: &'a ScopeStage0<'a>,
    model_name: &'a str,
    next_temp_id: u32,
}

impl<'a> ArrayContext<'a> {
    fn new(scope: &'a ScopeStage0<'a>, model_name: &'a str) -> Self {
        Self {
            scope,
            model_name,
            next_temp_id: 0,
        }
    }

    fn get_model(&self, model_name: &str) -> Option<&'a ModelStage0> {
        self.scope.models.get(model_name)
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
                .get(submodel_module_name)?;
            if let Variable::Module {
                model_name: submodel_name,
                ..
            } = module_var
            {
                return self.get_variable(submodel_name, submodel_var);
            }
            None
        } else {
            self.get_model(model_name)?.variables.get(ident)
        }
    }
}

impl<'a> Expr2Context for ArrayContext<'a> {
    fn get_dimensions(&self, ident: &str) -> Option<Vec<crate::dimensions::Dimension>> {
        // During AST lowering, we may encounter variables that don't exist yet
        // (e.g., in tests or when processing incomplete models)
        let var = self.get_variable(self.model_name, ident)?;
        var.get_dimensions().map(|dims| {
            dims.iter()
                .map(|d| crate::dimensions::Dimension::from(d.clone()))
                .collect()
        })
    }

    fn allocate_temp_id(&mut self) -> u32 {
        let id = self.next_temp_id;
        self.next_temp_id += 1;
        id
    }
}

pub(crate) fn lower_ast(scope: &ScopeStage0, ast: Ast<Expr0>) -> EquationResult<Ast<Expr2>> {
    let mut ctx = ArrayContext::new(scope, scope.model_name);

    match ast {
        Ast::Scalar(expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .and_then(|expr| Expr2::from(expr, &mut ctx))
            .map(Ast::Scalar),
        Ast::ApplyToAll(dims, expr) => Expr1::from(expr)
            .map(|expr| expr.constify_dimensions(scope))
            .and_then(|expr| Expr2::from(expr, &mut ctx))
            .map(|expr| Ast::ApplyToAll(dims, expr)),
        Ast::Arrayed(dims, elements) => {
            let elements: EquationResult<HashMap<ElementName, Expr2>> = elements
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
            match elements {
                Ok(elements) => Ok(Ast::Arrayed(dims, elements)),
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

macro_rules! child_needs_parens(
    ($expr:tt, $parent:expr, $child:expr, $eqn:expr) => {{
        match $parent {
            // no children so doesn't matter
            $expr::Const(_, _, _) | $expr::Var(_, _) => false,
            // children are comma separated, so no ambiguity possible
            $expr::App(_, _) | $expr::Subscript(_, _, _) => false,
            $expr::Op1(_, _, _) => matches!($child, $expr::Op2(_, _, _, _)),
            $expr::Op2(parent_op, _, _, _) => match $child {
                $expr::Const(_, _, _)
                | $expr::Var(_, _)
                | $expr::App(_, _)
                | $expr::Subscript(_, _, _)
                | $expr::If(_, _, _, _)
                | $expr::Op1(_, _, _) => false,
                // 3 * 2 + 1
                $expr::Op2(child_op, _, _, _) => {
                    // if we have `3 * (2 + 3)`, the parent's precedence
                    // is higher than the child and we need enclosing parens
                    parent_op.precedence() > child_op.precedence()
                }
            },
            $expr::If(_, _, _, _) => false,
        }
    }}
);

fn paren_if_necessary(parent: &Expr0, child: &Expr0, eqn: String) -> String {
    if child_needs_parens!(Expr0, parent, child, eqn) {
        format!("({eqn})")
    } else {
        eqn
    }
}

macro_rules! child_needs_parens2(
    ($expr:tt, $parent:expr, $child:expr, $eqn:expr) => {{
        match $parent {
            // no children so doesn't matter
            $expr::Const(_, _, _) | $expr::Var(_, _, _) => false,
            // children are comma separated, so no ambiguity possible
            $expr::App(_, _, _) | $expr::Subscript(_, _, _, _) => false,
            $expr::Op1(_, _, _, _) => matches!($child, $expr::Op2(_, _, _, _, _)),
            $expr::Op2(parent_op, _, _, _, _) => match $child {
                $expr::Const(_, _, _)
                | $expr::Var(_, _, _)
                | $expr::App(_, _, _)
                | $expr::Subscript(_, _, _, _)
                | $expr::If(_, _, _, _, _)
                | $expr::Op1(_, _, _, _) => false,
                // 3 * 2 + 1
                $expr::Op2(child_op, _, _, _, _) => {
                    // if we have `3 * (2 + 3)`, the parent's precedence
                    // is higher than the child and we need enclosing parens
                    parent_op.precedence() > child_op.precedence()
                }
            },
            $expr::If(_, _, _, _, _) => false,
        }
    }}
);

fn paren_if_necessary1(parent: &Expr2, child: &Expr2, eqn: String) -> String {
    if child_needs_parens2!(Expr2, parent, child, eqn) {
        format!("({eqn})")
    } else {
        eqn
    }
}

struct PrintVisitor {}

impl Visitor<String> for PrintVisitor {
    fn walk_index(&mut self, expr: &IndexExpr0) -> String {
        match expr {
            IndexExpr0::Wildcard(_) => "*".to_string(),
            IndexExpr0::StarRange(id, _) => format!("*:{id}"),
            IndexExpr0::Range(l, r, _) => format!("{}:{}", self.walk(l), self.walk(r)),
            IndexExpr0::DimPosition(n, _) => format!("@{n}"),
            IndexExpr0::Expr(e) => self.walk(e),
        }
    }

    fn walk(&mut self, expr: &Expr0) -> String {
        match expr {
            Expr0::Const(s, _, _) => s.clone(),
            Expr0::Var(id, _) => id.clone(),
            Expr0::App(UntypedBuiltinFn(func, args), _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk(e)).collect();
                format!("{}({})", func, args.join(", "))
            }
            Expr0::Subscript(id, args, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr0::Op1(op, l, _) => {
                match op {
                    UnaryOp::Transpose => {
                        let l = self.walk(l);
                        format!("{l}'")
                    }
                    _ => {
                        let l = paren_if_necessary(expr, l, self.walk(l));
                        let op: &str = match op {
                            UnaryOp::Positive => "+",
                            UnaryOp::Negative => "-",
                            UnaryOp::Not => "!",
                            UnaryOp::Transpose => unreachable!(), // handled above
                        };
                        format!("{op}{l}")
                    }
                }
            }
            Expr0::Op2(op, l, r, _) => {
                let l = paren_if_necessary(expr, l, self.walk(l));
                let r = paren_if_necessary(expr, r, self.walk(r));
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
                    BinaryOp::Neq => "!=",
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
    assert_eq!(
        "a + b",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Var("b".to_string(), Loc::new(5, 6))),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a + b * c",
        print_eqn(&Expr0::Op2(
            BinaryOp::Add,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Mul,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "a * (b + c)",
        print_eqn(&Expr0::Op2(
            BinaryOp::Mul,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Box::new(Expr0::Op2(
                BinaryOp::Add,
                Box::new(Expr0::Var("b".to_string(), Loc::default())),
                Box::new(Expr0::Var("c".to_owned(), Loc::default())),
                Loc::default()
            )),
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "-a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Negative,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "!a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Not,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+a",
        print_eqn(&Expr0::Op1(
            UnaryOp::Positive,
            Box::new(Expr0::Var("a".to_string(), Loc::new(1, 2))),
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
                    Expr0::Var("a".to_string(), Loc::new(7, 8)),
                    Expr0::Const("1.0".to_string(), 1.0, Loc::new(10, 13))
                ]
            ),
            Loc::new(0, 14),
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
                let id = str::replace(id, "_", "\\_");
                format!("\\mathrm{{{id}}}")
            }
            Expr2::App(builtin, _, _) => {
                let mut args: Vec<String> = vec![];
                walk_builtin_expr(builtin, |contents| {
                    let arg = match contents {
                        BuiltinContents::Ident(id, _loc) => format!("\\mathrm{{{id}}}"),
                        BuiltinContents::Expr(expr) => self.walk(expr),
                    };
                    args.push(arg);
                });
                let func = builtin.name();
                format!("\\operatorname{{{}}}({})", func, args.join(", "))
            }
            Expr2::Subscript(id, args, _, _) => {
                let args: Vec<String> = args.iter().map(|e| self.walk_index(e)).collect();
                format!("{}[{}]", id, args.join(", "))
            }
            Expr2::Op1(op, l, _, _) => {
                match op {
                    UnaryOp::Transpose => {
                        let l = self.walk(l);
                        format!("{l}^T")
                    }
                    _ => {
                        let l = paren_if_necessary1(expr, l, self.walk(l));
                        let op: &str = match op {
                            UnaryOp::Positive => "+",
                            UnaryOp::Negative => "-",
                            UnaryOp::Not => "\\neg ",
                            UnaryOp::Transpose => unreachable!(), // handled above
                        };
                        format!("{op}{l}")
                    }
                }
            }
            Expr2::Op2(op, l, r, _, _) => {
                let l = paren_if_necessary1(expr, l, self.walk(l));
                let r = paren_if_necessary1(expr, r, self.walk(r));
                let op: &str = match op {
                    BinaryOp::Add => "+",
                    BinaryOp::Sub => "-",
                    BinaryOp::Exp => {
                        return format!("{l}^{{{r}}}");
                    }
                    BinaryOp::Mul => "\\cdot",
                    BinaryOp::Div => {
                        return format!("\\frac{{{l}}}{{{r}}}");
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
                format!("{l} {op} {r}")
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

#[test]
fn test_latex_eqn() {
    assert_eq!(
        "\\mathrm{a\\_c} + \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Add,
            Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(1, 2))),
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{a\\_c} \\cdot \\mathrm{b}",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(1, 2))),
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
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
                Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(0, 0))),
                Box::new(Expr2::Const("1".to_string(), 1.0, Loc::new(0, 0))),
                None,
                Loc::new(0, 0),
            )),
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            None,
            Loc::new(0, 7),
        ))
    );
    assert_eq!(
        "\\mathrm{b} \\cdot (\\mathrm{a\\_c} - 1)",
        latex_eqn(&Expr2::Op2(
            BinaryOp::Mul,
            Box::new(Expr2::Var("b".to_string(), None, Loc::new(5, 6))),
            Box::new(Expr2::Op2(
                BinaryOp::Sub,
                Box::new(Expr2::Var("a_c".to_string(), None, Loc::new(0, 0))),
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
            Box::new(Expr2::Var("a".to_string(), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "\\neg \\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Not,
            Box::new(Expr2::Var("a".to_string(), None, Loc::new(1, 2))),
            None,
            Loc::new(0, 2),
        ))
    );
    assert_eq!(
        "+\\mathrm{a}",
        latex_eqn(&Expr2::Op1(
            UnaryOp::Positive,
            Box::new(Expr2::Var("a".to_string(), None, Loc::new(1, 2))),
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
                "a".to_string(),
                Box::new(Expr2::Const("1.0".to_owned(), 1.0, Default::default())),
                Default::default(),
            ),
            None,
            Loc::new(0, 14),
        ))
    );
}

#[cfg(test)]
mod ast_tests {
    use super::*;
    use crate::common::canonicalize;
    use crate::datamodel;
    use crate::model::ModelStage0;
    use std::collections::HashMap;

    #[test]
    fn test_simple_expr2_context() {
        // Create a simple model with an array variable
        let dim = datamodel::Dimension::Named(
            "region".to_string(),
            vec!["north".to_string(), "south".to_string()],
        );
        let array_var = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("population"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["region".to_string()],
                "100".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            variables: vec![array_var],
            views: vec![],
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let model_s0 = ModelStage0::new(&model_datamodel, &[dim.clone()], &units_ctx, false);

        let mut models = HashMap::new();
        models.insert("test_model".to_string(), model_s0);

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
    fn test_expr2_from_array_source() {
        use crate::ast::expr1::Expr1;
        use crate::ast::expr2::{ArraySource, ArrayView};

        // Create a model with array variables
        let dim = datamodel::Dimension::Named(
            "product".to_string(),
            vec!["A".to_string(), "B".to_string()],
        );
        let array_var = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("sales"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["product".to_string()],
                "50".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            variables: vec![array_var],
            views: vec![],
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let model_s0 = ModelStage0::new(&model_datamodel, &[dim.clone()], &units_ctx, false);

        let mut models = HashMap::new();
        models.insert("test_model".to_string(), model_s0);

        let dims_ctx = crate::dimensions::DimensionsContext::from(&[dim]);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
            model_name: "test_model",
        };

        let mut ctx = ArrayContext::new(&scope, "test_model");

        // Test Var expression for array variable
        let var_expr = Expr1::Var("sales".to_string(), Loc::default());
        let expr2 = Expr2::from(var_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_source, _) => {
                assert_eq!(id, "sales");
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Named(name, view) => {
                        assert_eq!(name, "sales");
                        match view {
                            ArrayView::Contiguous { dims } => {
                                assert_eq!(dims.len(), 1);
                                assert_eq!(dims[0].name(), "product");
                            }
                            _ => panic!("Expected contiguous array view"),
                        }
                    }
                    _ => panic!("Expected named array source"),
                }
            }
            _ => panic!("Expected Var expression"),
        }

        // Test Var expression for scalar (non-existent) variable
        let scalar_expr = Expr1::Var("scalar_var".to_string(), Loc::default());
        let expr2 = Expr2::from(scalar_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Var(id, array_source, _) => {
                assert_eq!(id, "scalar_var");
                assert!(array_source.is_none());
            }
            _ => panic!("Expected Var expression"),
        }

        // Test Subscript expression for array variable
        let subscript_expr = Expr1::Subscript(
            "sales".to_string(),
            vec![crate::ast::expr1::IndexExpr1::Expr(Expr1::Const(
                "0".to_string(),
                0.0,
                Loc::default(),
            ))],
            Loc::default(),
        );
        let expr2 = Expr2::from(subscript_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Subscript(id, args, array_source, _) => {
                assert_eq!(id, "sales");
                assert_eq!(args.len(), 1);
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_id, view) => {
                        // Temp IDs should be allocated starting from 0
                        // The view should be the result of subscripting
                        match view {
                            ArrayView::Contiguous { dims } => {
                                // After subscripting with one index, we should have a scalar
                                assert_eq!(dims.len(), 0);
                            }
                            _ => panic!("Expected contiguous array view"),
                        }
                    }
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected Subscript expression"),
        }
    }

    #[test]
    fn test_expr2_op1_array_source() {
        use crate::ast::expr1::Expr1;
        use crate::ast::expr2::{ArraySource, ArrayView};
        use crate::ast::{Expr2, UnaryOp};

        // Create a model with array variables
        let dim =
            datamodel::Dimension::Named("dim1".to_string(), vec!["A".to_string(), "B".to_string()]);
        let array_var = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("array_var"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["dim1".to_string()],
                "10".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            variables: vec![array_var],
            views: vec![],
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let model_s0 = ModelStage0::new(&model_datamodel, &[dim.clone()], &units_ctx, false);

        let mut models = HashMap::new();
        models.insert("test_model".to_string(), model_s0);

        let dims_ctx = crate::dimensions::DimensionsContext::from(&[dim]);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
            model_name: "test_model",
        };

        let mut ctx = ArrayContext::new(&scope, "test_model");

        // Test unary negative on array
        let neg_expr = Expr1::Op1(
            UnaryOp::Negative,
            Box::new(Expr1::Var("array_var".to_string(), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(neg_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op1(UnaryOp::Negative, _, array_source, _) => {
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_, view) => match view {
                        ArrayView::Contiguous { dims } => {
                            assert_eq!(dims.len(), 1);
                            assert_eq!(dims[0].name(), "dim1");
                        }
                        _ => panic!("Expected contiguous array view"),
                    },
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected Op1 expression"),
        }

        // Test transpose on array
        let transpose_expr = Expr1::Op1(
            UnaryOp::Transpose,
            Box::new(Expr1::Var("array_var".to_string(), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(transpose_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op1(UnaryOp::Transpose, _, array_source, _) => {
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_, view) => {
                        // With the new allocate_temp_array helper, we allocate contiguous
                        // storage for the transposed result
                        match view {
                            ArrayView::Contiguous { dims } => {
                                assert_eq!(dims.len(), 1);
                                assert_eq!(dims[0].name(), "dim1");
                            }
                            _ => panic!("Expected contiguous array view for allocated temp"),
                        }
                    }
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected Op1 expression"),
        }
    }

    #[test]
    fn test_expr2_op2_array_source() {
        use crate::ast::expr1::Expr1;
        use crate::ast::expr2::{ArraySource, ArrayView};
        use crate::ast::{BinaryOp, Expr2};

        // Create a model with array variables
        let dim = datamodel::Dimension::Named(
            "region".to_string(),
            vec!["north".to_string(), "south".to_string()],
        );
        let array_var1 = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("sales"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["region".to_string()],
                "100".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });
        let array_var2 = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("costs"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["region".to_string()],
                "50".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            variables: vec![array_var1, array_var2],
            views: vec![],
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let model_s0 = ModelStage0::new(&model_datamodel, &[dim.clone()], &units_ctx, false);

        let mut models = HashMap::new();
        models.insert("test_model".to_string(), model_s0);

        let dims_ctx = crate::dimensions::DimensionsContext::from(&[dim]);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
            model_name: "test_model",
        };

        let mut ctx = ArrayContext::new(&scope, "test_model");

        // Test array + array (matching dimensions)
        let add_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var("sales".to_string(), Loc::default())),
            Box::new(Expr1::Var("costs".to_string(), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(add_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op2(BinaryOp::Add, _, _, array_source, _) => {
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_, view) => match view {
                        ArrayView::Contiguous { dims } => {
                            assert_eq!(dims.len(), 1);
                            assert_eq!(dims[0].name(), "region");
                        }
                        _ => panic!("Expected contiguous array view"),
                    },
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected Op2 expression"),
        }

        // Test array + scalar (broadcasting)
        let add_scalar_expr = Expr1::Op2(
            BinaryOp::Add,
            Box::new(Expr1::Var("sales".to_string(), Loc::default())),
            Box::new(Expr1::Const("10".to_string(), 10.0, Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(add_scalar_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::Op2(BinaryOp::Add, _, _, array_source, _) => {
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_, view) => match view {
                        ArrayView::Contiguous { dims } => {
                            assert_eq!(dims.len(), 1);
                            assert_eq!(dims[0].name(), "region");
                        }
                        _ => panic!("Expected contiguous array view"),
                    },
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected Op2 expression"),
        }
    }

    #[test]
    fn test_expr2_if_array_source() {
        use crate::ast::Expr2;
        use crate::ast::expr1::Expr1;
        use crate::ast::expr2::{ArraySource, ArrayView};

        // Create a model with array variables
        let dim = datamodel::Dimension::Named(
            "time_period".to_string(),
            vec![
                "Q1".to_string(),
                "Q2".to_string(),
                "Q3".to_string(),
                "Q4".to_string(),
            ],
        );
        let array_var = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("quarterly_data"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["time_period".to_string()],
                "25".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            variables: vec![array_var],
            views: vec![],
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let model_s0 = ModelStage0::new(&model_datamodel, &[dim.clone()], &units_ctx, false);

        let mut models = HashMap::new();
        models.insert("test_model".to_string(), model_s0);

        let dims_ctx = crate::dimensions::DimensionsContext::from(&[dim]);
        let scope = ScopeStage0 {
            models: &models,
            dimensions: &dims_ctx,
            model_name: "test_model",
        };

        let mut ctx = ArrayContext::new(&scope, "test_model");

        // Test if expression with array in both branches
        let if_expr = Expr1::If(
            Box::new(Expr1::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr1::Var("quarterly_data".to_string(), Loc::default())),
            Box::new(Expr1::Var("quarterly_data".to_string(), Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(if_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::If(_, _, _, array_source, _) => {
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_, view) => match view {
                        ArrayView::Contiguous { dims } => {
                            assert_eq!(dims.len(), 1);
                            assert_eq!(dims[0].name(), "time_period");
                            assert_eq!(dims[0].len(), 4);
                        }
                        _ => panic!("Expected contiguous array view"),
                    },
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected If expression"),
        }

        // Test if expression with array in one branch, scalar in other
        let if_mixed_expr = Expr1::If(
            Box::new(Expr1::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr1::Var("quarterly_data".to_string(), Loc::default())),
            Box::new(Expr1::Const("0".to_string(), 0.0, Loc::default())),
            Loc::default(),
        );
        let expr2 = Expr2::from(if_mixed_expr, &mut ctx).unwrap();

        match expr2 {
            Expr2::If(_, _, _, array_source, _) => {
                assert!(array_source.is_some());
                match array_source.unwrap() {
                    ArraySource::Temp(_, view) => match view {
                        ArrayView::Contiguous { dims } => {
                            assert_eq!(dims.len(), 1);
                            assert_eq!(dims[0].name(), "time_period");
                        }
                        _ => panic!("Expected contiguous array view"),
                    },
                    _ => panic!("Expected temp array source"),
                }
            }
            _ => panic!("Expected If expression"),
        }
    }

    #[test]
    fn test_expr2_dimension_mismatch_errors() {
        use crate::ast::BinaryOp;
        use crate::ast::expr1::Expr1;
        use crate::common::ErrorCode;

        // Create a model with array variables of different dimensions
        let dim1 = datamodel::Dimension::Named(
            "region".to_string(),
            vec!["north".to_string(), "south".to_string()],
        );
        let dim2 = datamodel::Dimension::Named(
            "product".to_string(),
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
        );

        let array_var1 = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("regional_data"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["region".to_string()],
                "100".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });
        let array_var2 = datamodel::Variable::Aux(datamodel::Aux {
            ident: canonicalize("product_data"),
            equation: datamodel::Equation::ApplyToAll(
                vec!["product".to_string()],
                "50".to_string(),
                None,
            ),
            documentation: "".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: datamodel::Visibility::Private,
        });

        let model_datamodel = datamodel::Model {
            name: "test_model".to_string(),
            variables: vec![array_var1, array_var2],
            views: vec![],
        };

        let units_ctx = crate::units::Context::new(&[], &Default::default()).unwrap();
        let model_s0 = ModelStage0::new(
            &model_datamodel,
            &[dim1.clone(), dim2.clone()],
            &units_ctx,
            false,
        );

        let mut models = HashMap::new();
        models.insert("test_model".to_string(), model_s0);

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
            Box::new(Expr1::Var("regional_data".to_string(), Loc::default())),
            Box::new(Expr1::Var("product_data".to_string(), Loc::default())),
            Loc::new(0, 10),
        );
        let result = Expr2::from(add_expr, &mut ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::MismatchedDimensions);

        // Test if expression with mismatched dimensions
        let if_expr = Expr1::If(
            Box::new(Expr1::Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Expr1::Var("regional_data".to_string(), Loc::default())),
            Box::new(Expr1::Var("product_data".to_string(), Loc::default())),
            Loc::new(0, 20),
        );
        let result = Expr2::from(if_expr, &mut ctx);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, ErrorCode::MismatchedDimensions);
    }
}
