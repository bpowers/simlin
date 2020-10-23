// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::Expr;
use crate::common::{EquationError, Ident};
use crate::xmile;

pub struct BuiltinVisitor<'a> {
    variable_name: &'a str,
    models: &'a HashMap<String, HashMap<Ident, &'a xmile::Var>>,
    vars: HashMap<Ident, xmile::Var>,
    n: usize,
}

impl<'a> BuiltinVisitor<'a> {
    // FIXME: remove when done here
    #[allow(dead_code)]
    pub fn new(
        variable_name: &'a str,
        models: &'a HashMap<String, HashMap<Ident, &'a xmile::Var>>,
    ) -> Self {
        Self {
            variable_name,
            models,
            vars: Default::default(),
            n: 0,
        }
    }

    fn walk(&mut self, expr: Expr) -> std::result::Result<Expr, EquationError> {
        use crate::ast::Expr::*;
        use std::mem;
        let result: Expr = match expr {
            Const(_, _) => expr,
            Var(_) => expr,
            App(func, args) => {
                let args: std::result::Result<Vec<Expr>, EquationError> =
                    args.into_iter().map(|e| self.walk(e)).collect();

                if self.models.contains_key(&func) {
                    self.n += 1;
                }
                App(func, args?)
            }
            Op1(op, mut r) => {
                *r = self.walk(mem::take(&mut *r))?;
                Op1(op, r)
            }
            Op2(op, mut l, mut r) => {
                *l = self.walk(mem::take(&mut *l))?;
                *r = self.walk(mem::take(&mut *r))?;
                Op2(op, l, r)
            }
            If(mut cond, mut t, mut f) => {
                *cond = self.walk(mem::take(&mut *cond))?;
                *t = self.walk(mem::take(&mut *t))?;
                *f = self.walk(mem::take(&mut *f))?;
                If(cond, t, f)
            }
        };

        Ok(result)
    }
}

#[test]
fn test_builtin_visitor() {}

pub fn instantiate_implicit_modules<'a>(
    variable_name: &'a str,
    ast: Expr,
    models: &'a HashMap<String, HashMap<Ident, &'a xmile::Var>>,
) -> std::result::Result<(Expr, Vec<xmile::Var>), EquationError> {
    let mut builtin_visitor = BuiltinVisitor::new(variable_name, models);
    let ast = builtin_visitor.walk(ast)?;
    let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
    Ok((ast, vars))
}
