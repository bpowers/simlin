// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{print_eqn, Expr};
use crate::builtins::is_builtin_fn;
use crate::common::{EquationError, Ident};
use crate::{datamodel, eqn_err};

fn stdlib_args(name: &str) -> Option<&'static [&'static str]> {
    let args = match name {
        "smth1" | "smth3" | "delay1" | "delay3" | "trend" => {
            &["input", "delay_time", "initial_value"]
        }
        _ => {
            return None;
        }
    };
    Some(args)
}

pub struct BuiltinVisitor<'a> {
    #[allow(dead_code)]
    variable_name: &'a str,
    vars: HashMap<Ident, datamodel::Variable>,
    n: usize,
}

impl<'a> BuiltinVisitor<'a> {
    // FIXME: remove when done here
    #[allow(dead_code)]
    pub fn new(variable_name: &'a str) -> Self {
        Self {
            variable_name,
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
                let args = args?;
                if is_builtin_fn(&func) {
                    return Ok(App(func, args));
                }

                // TODO: make this a function call/hash lookup
                if !crate::stdlib::MODEL_NAMES.contains(&func.as_str()) {
                    return eqn_err!(UnknownBuiltin, 0);
                }

                let stdlib_model_inputs = stdlib_args(&func).unwrap();

                let ident_args: Vec<Ident> = args
                    .into_iter()
                    .enumerate()
                    .map(|(i, arg)| {
                        if let Expr::Var(id) = arg {
                            id
                        } else {
                            let id = format!("$·{}·{}·arg{}", self.variable_name, self.n, i);
                            let eqn = print_eqn(&arg);
                            let x_var = datamodel::Variable::Aux(datamodel::Aux {
                                ident: id.clone(),
                                equation: eqn,
                                documentation: "".to_string(),
                                units: None,
                                gf: None,
                            });
                            self.vars.insert(id.clone(), x_var);
                            id
                        }
                    })
                    .collect();

                let module_name = format!("$·{}·{}·{}", self.variable_name, self.n, func);
                let references: Vec<_> = ident_args
                    .into_iter()
                    .enumerate()
                    .map(|(i, src)| datamodel::ModuleReference {
                        src,
                        dst: format!("{}.{}", module_name, stdlib_model_inputs[i]),
                    })
                    .collect();
                let x_module = datamodel::Variable::Module(datamodel::Module {
                    ident: module_name.clone(),
                    model_name: format!("stdlib·{}", func),
                    documentation: "".to_string(),
                    units: None,
                    references,
                });
                let module_output_name = format!("{}.output", module_name);
                self.vars.insert(module_name, x_module);

                self.n += 1;
                Var(module_output_name)
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

pub fn instantiate_implicit_modules(
    variable_name: &str,
    ast: Expr,
) -> std::result::Result<(Expr, Vec<datamodel::Variable>), EquationError> {
    let mut builtin_visitor = BuiltinVisitor::new(variable_name);
    let ast = builtin_visitor.walk(ast)?;
    let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
    Ok((ast, vars))
}
