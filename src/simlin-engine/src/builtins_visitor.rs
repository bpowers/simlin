// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{print_eqn, Ast, Expr};
use crate::builtins::is_builtin_fn;
use crate::common::{EquationError, Ident};
use crate::datamodel::Visibility;
use crate::{datamodel, eqn_err};

fn stdlib_args(name: &str) -> Option<&'static [&'static str]> {
    let args: &'static [&'static str] = match name {
        "smth1" | "smth3" | "delay1" | "delay3" | "trend" => {
            &["input", "delay_time", "initial_value"]
        }
        "init" => &["input"],
        _ => {
            return None;
        }
    };
    Some(args)
}

pub struct BuiltinVisitor<'a> {
    variable_name: &'a str,
    vars: HashMap<Ident, datamodel::Variable>,
    n: usize,
}

impl<'a> BuiltinVisitor<'a> {
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
            Const(_, _, _) => expr,
            Var(_, _) => expr,
            App(func, args, loc) => {
                let args: std::result::Result<Vec<Expr>, EquationError> =
                    args.into_iter().map(|e| self.walk(e)).collect();
                let args = args?;
                if is_builtin_fn(&func) {
                    return Ok(App(func, args, loc));
                }

                // TODO: make this a function call/hash lookup
                if !crate::stdlib::MODEL_NAMES.contains(&func.as_str()) {
                    return eqn_err!(UnknownBuiltin, loc.start, loc.end);
                }

                let stdlib_model_inputs = stdlib_args(&func).unwrap();

                let ident_args: Vec<Ident> = args
                    .into_iter()
                    .enumerate()
                    .map(|(i, arg)| {
                        if let Expr::Var(id, _loc) = arg {
                            id
                        } else {
                            let id = format!("$⁚{}⁚{}⁚arg{}", self.variable_name, self.n, i);
                            let eqn = print_eqn(&arg);
                            let x_var = datamodel::Variable::Aux(datamodel::Aux {
                                ident: id.clone(),
                                equation: datamodel::Equation::Scalar(eqn),
                                documentation: "".to_string(),
                                units: None,
                                gf: None,
                                can_be_module_input: false,
                                visibility: datamodel::Visibility::Private,
                            });
                            self.vars.insert(id.clone(), x_var);
                            id
                        }
                    })
                    .collect();

                let module_name = format!("$⁚{}⁚{}⁚{}", self.variable_name, self.n, func);
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
                    model_name: format!("stdlib⁚{}", func),
                    documentation: "".to_string(),
                    units: None,
                    references,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                });
                let module_output_name = format!("{}·output", module_name);
                self.vars.insert(module_name, x_module);

                self.n += 1;
                Var(module_output_name, loc)
            }
            Subscript(id, args, loc) => {
                let args: std::result::Result<Vec<Expr>, EquationError> =
                    args.into_iter().map(|e| self.walk(e)).collect();
                let args = args?;
                Subscript(id, args, loc)
            }
            Op1(op, mut r, loc) => {
                *r = self.walk(mem::take(&mut *r))?;
                Op1(op, r, loc)
            }
            Op2(op, mut l, mut r, loc) => {
                *l = self.walk(mem::take(&mut *l))?;
                *r = self.walk(mem::take(&mut *r))?;
                Op2(op, l, r, loc)
            }
            If(mut cond, mut t, mut f, loc) => {
                *cond = self.walk(mem::take(&mut *cond))?;
                *t = self.walk(mem::take(&mut *t))?;
                *f = self.walk(mem::take(&mut *f))?;
                If(cond, t, f, loc)
            }
        };

        Ok(result)
    }
}

#[test]
fn test_builtin_visitor() {}

pub fn instantiate_implicit_modules(
    variable_name: &str,
    ast: Ast,
) -> std::result::Result<(Ast, Vec<datamodel::Variable>), EquationError> {
    let mut builtin_visitor = BuiltinVisitor::new(variable_name);
    let ast = match ast {
        Ast::Scalar(ast) => Ast::Scalar(builtin_visitor.walk(ast)?),
        Ast::ApplyToAll(dimensions, ast) => Ast::ApplyToAll(dimensions, builtin_visitor.walk(ast)?),
        Ast::Arrayed(dimensions, elements) => {
            let elements: std::result::Result<HashMap<_, _>, EquationError> = elements
                .into_iter()
                .map(|(subscript, equation)| {
                    builtin_visitor.walk(equation).map(|ast| (subscript, ast))
                })
                .collect();
            Ast::Arrayed(dimensions, elements?)
        }
    };
    let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
    Ok((ast, vars))
}
