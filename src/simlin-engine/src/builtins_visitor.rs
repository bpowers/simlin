// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use crate::ast::{Ast, Expr0, IndexExpr0, print_eqn};
use crate::builtins::{UntypedBuiltinFn, is_builtin_fn};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, EquationError, Ident, RawIdent,
    canonicalize,
};
use crate::datamodel::Visibility;
use crate::dimensions::Dimension;
use crate::vm::SubscriptIterator;
use crate::{datamodel, eqn_err};

fn stdlib_args(name: &str) -> Option<&'static [&'static str]> {
    let args: &'static [&'static str] = match name {
        "smth1" | "smth3" | "delay1" | "delay3" | "trend" => {
            &["input", "delay_time", "initial_value"]
        }
        "previous" => &["input", "initial_value"],
        "init" => &["input"],
        _ => {
            return None;
        }
    };
    Some(args)
}

/// Check if the expression contains any stdlib function calls that need per-element expansion
fn contains_stdlib_call(expr: &Expr0) -> bool {
    use Expr0::*;
    match expr {
        Const(_, _, _) => false,
        Var(_, _) => false,
        App(UntypedBuiltinFn(func, args), _) => {
            if crate::stdlib::MODEL_NAMES.contains(&func.as_str()) {
                return true;
            }
            args.iter().any(contains_stdlib_call)
        }
        Subscript(_, args, _) => args.iter().any(|idx| match idx {
            IndexExpr0::Expr(e) => contains_stdlib_call(e),
            _ => false,
        }),
        Op1(_, r, _) => contains_stdlib_call(r),
        Op2(_, l, r, _) => contains_stdlib_call(l) || contains_stdlib_call(r),
        If(cond, t, f, _) => {
            contains_stdlib_call(cond) || contains_stdlib_call(t) || contains_stdlib_call(f)
        }
    }
}

/// Get dimension names from a slice of Dimensions
fn get_dimension_names(dimensions: &[Dimension]) -> Vec<CanonicalDimensionName> {
    dimensions
        .iter()
        .map(|d| match d {
            Dimension::Named(name, _) => name.clone(),
            Dimension::Indexed(name, _) => name.clone(),
        })
        .collect()
}

pub struct BuiltinVisitor<'a> {
    variable_name: &'a str,
    vars: HashMap<Ident<Canonical>, datamodel::Variable>,
    n: usize,
    self_allowed: bool,
    /// Full dimension info for A2A context (used to identify indexed vs named dimensions)
    dimensions: Vec<Dimension>,
    /// Dimension names for A2A context (derived from dimensions)
    dimension_names: Vec<CanonicalDimensionName>,
    /// Current subscript element names being processed in A2A context
    active_subscript: Option<Vec<String>>,
}

impl<'a> BuiltinVisitor<'a> {
    pub fn new(variable_name: &'a str) -> Self {
        Self {
            variable_name,
            vars: Default::default(),
            n: 0,
            self_allowed: false,
            dimensions: Vec::new(),
            dimension_names: Vec::new(),
            active_subscript: None,
        }
    }

    /// Create a visitor with A2A subscript context for per-element module creation
    pub fn new_with_subscript_context(
        variable_name: &'a str,
        dimensions: &[Dimension],
        subscript: &[String],
    ) -> Self {
        Self {
            variable_name,
            vars: Default::default(),
            n: 0,
            self_allowed: false,
            dimensions: dimensions.to_vec(),
            dimension_names: get_dimension_names(dimensions),
            active_subscript: Some(subscript.to_vec()),
        }
    }

    /// Substitute dimension references in the expression with concrete element names.
    /// For example, if we're processing element "A2" of dimension "SubA",
    /// transform `input[SubA]` to `input[A2]`.
    fn substitute_dimension_refs(&self, expr: Expr0) -> Expr0 {
        use Expr0::*;
        use std::mem;

        let subscript = match &self.active_subscript {
            Some(s) => s,
            None => return expr,
        };

        match expr {
            Const(_, _, _) => expr,
            Var(ref ident, loc) => {
                // Check if this var is a dimension name that should be substituted
                let canonical_name = CanonicalDimensionName::from_raw(ident.as_str());
                for (i, dim_name) in self.dimension_names.iter().enumerate() {
                    if &canonical_name == dim_name {
                        // Check if this is an indexed or named dimension
                        match &self.dimensions[i] {
                            Dimension::Indexed(_, _) => {
                                // For indexed dimensions, the subscript element is a number
                                // Use it directly as a Const
                                let val: f64 = subscript[i].parse().unwrap_or(0.0);
                                return Const(subscript[i].clone(), val, loc);
                            }
                            Dimension::Named(_, _) => {
                                // For named dimensions, use qualified element (dimension·element).
                                // During constify_dimensions, this gets looked up via
                                // DimensionsContext::lookup which returns a 1-based index
                                // (from indexed_elements). The compiler then converts this
                                // 1-based value to 0-based when processing subscript indices.
                                let qualified_name =
                                    format!("{}·{}", dim_name.as_str(), subscript[i]);
                                return Var(RawIdent::new_from_str(&qualified_name), loc);
                            }
                        }
                    }
                }
                expr
            }
            App(UntypedBuiltinFn(func, args), loc) => {
                let args = args
                    .into_iter()
                    .map(|a| self.substitute_dimension_refs(a))
                    .collect();
                App(UntypedBuiltinFn(func, args), loc)
            }
            Subscript(id, args, loc) => {
                let args = args
                    .into_iter()
                    .map(|idx| match idx {
                        IndexExpr0::Expr(e) => IndexExpr0::Expr(self.substitute_dimension_refs(e)),
                        other => other,
                    })
                    .collect();
                Subscript(id, args, loc)
            }
            Op1(op, mut r, loc) => {
                *r = self.substitute_dimension_refs(mem::take(&mut *r));
                Op1(op, r, loc)
            }
            Op2(op, mut l, mut r, loc) => {
                *l = self.substitute_dimension_refs(mem::take(&mut *l));
                *r = self.substitute_dimension_refs(mem::take(&mut *r));
                Op2(op, l, r, loc)
            }
            If(mut cond, mut t, mut f, loc) => {
                *cond = self.substitute_dimension_refs(mem::take(&mut *cond));
                *t = self.substitute_dimension_refs(mem::take(&mut *t));
                *f = self.substitute_dimension_refs(mem::take(&mut *f));
                If(cond, t, f, loc)
            }
        }
    }

    /// Get the subscript suffix for module/helper names (e.g., "a2" or "a1,b2")
    fn subscript_suffix(&self) -> String {
        match &self.active_subscript {
            Some(s) => s.join(",").to_lowercase(),
            None => String::new(),
        }
    }

    fn walk_index(&mut self, expr: IndexExpr0) -> Result<IndexExpr0, EquationError> {
        use IndexExpr0::*;
        let result: IndexExpr0 = match expr {
            Wildcard(_) => expr,
            StarRange(_, _) => expr,
            Range(_, _, _) => expr,
            DimPosition(_, _) => expr,
            Expr(expr) => Expr(self.walk(expr)?),
        };

        Ok(result)
    }

    fn walk(&mut self, expr: Expr0) -> Result<Expr0, EquationError> {
        use Expr0::*;
        use std::mem;
        let result: Expr0 = match expr {
            Const(_, _, _) => expr,
            Var(ref ident, loc) => {
                if ident.as_str().eq_ignore_ascii_case("self") && self.self_allowed {
                    Var(RawIdent::new_from_str(self.variable_name), loc)
                } else {
                    expr
                }
            }
            App(UntypedBuiltinFn(func, args), loc) => {
                let orig_self_allowed = self.self_allowed;
                self.self_allowed |= func == "previous" || func == "size";
                let args: Result<Vec<Expr0>, EquationError> =
                    args.into_iter().map(|e| self.walk(e)).collect();
                self.self_allowed = orig_self_allowed;
                let args = args?;
                if is_builtin_fn(&func) {
                    return Ok(App(UntypedBuiltinFn(func, args), loc));
                }

                // TODO: make this a function call/hash lookup
                if !crate::stdlib::MODEL_NAMES.contains(&func.as_str()) {
                    return eqn_err!(UnknownBuiltin, loc.start, loc.end);
                }

                let stdlib_model_inputs = stdlib_args(&func).unwrap();

                // In A2A context, add subscript suffix to module name for uniqueness
                let subscript_suffix = self.subscript_suffix();
                let module_name = if subscript_suffix.is_empty() {
                    format!("$⁚{}⁚{}⁚{}", self.variable_name, self.n, func)
                } else {
                    format!(
                        "$⁚{}⁚{}⁚{}⁚{}",
                        self.variable_name, self.n, func, subscript_suffix
                    )
                };

                let ident_args = args.into_iter().enumerate().map(|(i, arg)| {
                    if let Var(id, _loc) = arg {
                        // In A2A context, substitute dimension refs in simple var references too
                        if self.active_subscript.is_some() {
                            let substituted = self.substitute_dimension_refs(Var(id.clone(), _loc));
                            if let Var(new_id, _) = substituted {
                                return new_id.as_str().to_string();
                            }
                        }
                        id.as_str().to_string()
                    } else {
                        // In A2A context, substitute dimension refs and add subscript suffix
                        let transformed_arg = if self.active_subscript.is_some() {
                            self.substitute_dimension_refs(arg)
                        } else {
                            arg
                        };

                        let id = if subscript_suffix.is_empty() {
                            format!("$⁚{}⁚{}⁚arg{}", self.variable_name, self.n, i)
                        } else {
                            format!(
                                "$⁚{}⁚{}⁚arg{}⁚{}",
                                self.variable_name, self.n, i, subscript_suffix
                            )
                        };
                        let eqn = print_eqn(&transformed_arg);
                        let x_var = datamodel::Variable::Aux(datamodel::Aux {
                            ident: id.clone(),
                            equation: datamodel::Equation::Scalar(eqn, None),
                            documentation: "".to_string(),
                            units: None,
                            gf: None,
                            can_be_module_input: false,
                            visibility: datamodel::Visibility::Private,
                            ai_state: None,
                            uid: None,
                        });
                        self.vars.insert(canonicalize(&id), x_var);
                        id
                    }
                });

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
                    model_name: format!("stdlib⁚{func}"),
                    documentation: "".to_string(),
                    units: None,
                    references,
                    can_be_module_input: false,
                    visibility: Visibility::Private,
                    ai_state: None,
                    uid: None,
                });
                let module_output_name = format!("{module_name}·output");
                self.vars.insert(canonicalize(&module_name), x_module);

                self.n += 1;
                Var(RawIdent::new_from_str(&module_output_name), loc)
            }
            Subscript(id, args, loc) => {
                let args: Result<Vec<IndexExpr0>, EquationError> =
                    args.into_iter().map(|e| self.walk_index(e)).collect();
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

pub fn instantiate_implicit_modules(
    variable_name: &str,
    ast: Ast<Expr0>,
) -> std::result::Result<(Ast<Expr0>, Vec<datamodel::Variable>), EquationError> {
    match ast {
        Ast::Scalar(ast) => {
            let mut builtin_visitor = BuiltinVisitor::new(variable_name);
            let transformed = builtin_visitor.walk(ast)?;
            let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
            Ok((Ast::Scalar(transformed), vars))
        }
        Ast::ApplyToAll(dimensions, ast) => {
            // Check if expression contains stdlib calls - if so, expand to per-element
            if contains_stdlib_call(&ast) && !dimensions.is_empty() {
                // Expand to per-element modules
                let mut all_vars = Vec::new();
                let mut elements = HashMap::new();

                for subscript in SubscriptIterator::new(&dimensions) {
                    let subscript_key = CanonicalElementName::from_raw(&subscript.join(","));
                    let ast_clone = ast.clone();

                    let mut visitor = BuiltinVisitor::new_with_subscript_context(
                        variable_name,
                        &dimensions,
                        &subscript,
                    );
                    let transformed_ast = visitor.walk(ast_clone)?;

                    elements.insert(subscript_key, transformed_ast);
                    all_vars.extend(visitor.vars.values().cloned());
                }

                Ok((Ast::Arrayed(dimensions, elements), all_vars))
            } else {
                // No stdlib calls - original behavior
                let mut builtin_visitor = BuiltinVisitor::new(variable_name);
                let transformed = builtin_visitor.walk(ast)?;
                let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
                Ok((Ast::ApplyToAll(dimensions, transformed), vars))
            }
        }
        Ast::Arrayed(dimensions, elements) => {
            let mut builtin_visitor = BuiltinVisitor::new(variable_name);
            let elements: std::result::Result<HashMap<_, _>, EquationError> = elements
                .into_iter()
                .map(|(subscript, equation)| {
                    builtin_visitor.walk(equation).map(|ast| (subscript, ast))
                })
                .collect();
            let vars: Vec<_> = builtin_visitor.vars.values().cloned().collect();
            Ok((Ast::Arrayed(dimensions, elements?), vars))
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_common::TestProject;

    /// Test that arrayed DELAY1 compiles and simulates
    /// d[SubA] = DELAY1(input[SubA], delay_time, init)
    #[test]
    fn test_arrayed_delay1_basic() {
        let project = TestProject::new("arrayed_delay")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .named_dimension("SubA", &["A2", "A3"])
            .array_const("input[SubA]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[SubA]", "DELAY1(input[SubA], delay_time, init)");

        project.assert_compiles();
        project.assert_sim_builds();
    }

    /// Test arrayed DELAY1 with mixed scalar and arrayed arguments
    /// d[DimA] = DELAY1(input_a[DimA], delay, init_scalar)
    #[test]
    fn test_arrayed_delay1_mixed_args() {
        let project = TestProject::new("arrayed_delay_mixed")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay", "5", None)
            .aux("init_scalar", "0", None)
            .array_aux("d[DimA]", "DELAY1(input_a[DimA], delay, init_scalar)");

        project.assert_compiles();
        project.assert_sim_builds();
    }

    /// Test that arrayed DELAY1 produces correct numerical output
    /// With input=10, delay_time=5, init=0:
    /// - At t=0: stock=0, output=0
    /// - At t=1: stock=10, output=10/5=2
    #[test]
    fn test_arrayed_delay1_numerical_values() {
        // Using dt=1, which gives us time steps at 0, 1, 2, ...
        // DELAY1 with input=10, delay=5, init=0:
        // stock(0) = 0 (init * delay)
        // output(0) = 0 (stock/delay)
        // stock(1) = 0 + 1*(10 - 0) = 10
        // output(1) = 10/5 = 2
        let project = TestProject::new("delay_numerical")
            .named_dimension("DimA", &["A1", "A2"])
            .array_const("input_a[DimA]", 10.0)
            .aux("delay", "5", None)
            .aux("init", "0", None)
            .array_aux("d[DimA]", "DELAY1(input_a[DimA], delay, init)");

        project.assert_compiles();
        project.assert_sim_builds();

        // Get results for 2 timesteps (0 and 1)
        // Each element should have independent delay state
        // At step 1, output should be input/delay = 10/5 = 2
        project.assert_interpreter_result("d", &[2.0, 2.0]);
    }

    /// Test arrayed DELAY1 with all arrayed arguments
    /// d[DimA] = DELAY1(input_a[DimA], delay_a[DimA], init_a[DimA])
    #[test]
    fn test_arrayed_delay1_all_arrayed() {
        let project = TestProject::new("arrayed_delay_all")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .array_const("input_a[DimA]", 10.0)
            .array_const("delay_a[DimA]", 1.0)
            .array_const("init_a[DimA]", 0.0)
            .array_aux(
                "d[DimA]",
                "DELAY1(input_a[DimA], delay_a[DimA], init_a[DimA])",
            );

        project.assert_compiles();
        project.assert_sim_builds();
    }

    /// Test arrayed DELAY1 with per-element different values (like d5 model)
    /// Verifies that each element gets its own module with correct inputs
    #[test]
    fn test_arrayed_delay1_different_element_values() {
        // Mirrors d5 in the delay model:
        // input_a[A1]=10, input_a[A2]=20
        // delay_a[A1]=2, delay_a[A2]=2
        // For DELAY1 with init=0:
        // At step 1: output = stock/delay = input/delay = 10/2=5, 20/2=10
        let project = TestProject::new("arrayed_delay_diff_values")
            .named_dimension("DimA", &["A1", "A2"])
            .array_with_ranges("input_a[DimA]", vec![("A1", "10"), ("A2", "20")])
            .array_const("delay_a[DimA]", 2.0)
            .array_const("init_a[DimA]", 0.0)
            .array_aux(
                "d[DimA]",
                "DELAY1(input_a[DimA], delay_a[DimA], init_a[DimA])",
            );

        project.assert_compiles();
        project.assert_sim_builds();

        // At step 1: output = stock/delay
        // For A1: input=10, delay=2, init=0 -> stock(1)=10, output(1)=10/2=5
        // For A2: input=20, delay=2, init=0 -> stock(1)=20, output(1)=20/2=10
        project.assert_interpreter_result("d", &[5.0, 10.0]);
    }

    /// Test arrayed DELAY3 with arrayed delay time
    /// d[DimA] = DELAY3(input, delay_a[DimA])
    #[test]
    fn test_arrayed_delay3() {
        let project = TestProject::new("arrayed_delay3")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .aux("input", "10", None)
            .array_const("delay_a[DimA]", 1.0)
            .array_aux("d[DimA]", "DELAY3(input, delay_a[DimA])");

        project.assert_compiles();
        project.assert_sim_builds();
    }

    /// Test arrayed SMOOTH1/SMTH1
    #[test]
    fn test_arrayed_smooth1() {
        let project = TestProject::new("arrayed_smooth1")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .array_const("input_a[DimA]", 10.0)
            .aux("smooth_time", "1", None)
            .array_aux("s[DimA]", "SMTH1(input_a[DimA], smooth_time)");

        project.assert_compiles();
        project.assert_sim_builds();
    }

    /// Test with indexed dimensions (numeric 1,2,3...)
    #[test]
    fn test_arrayed_delay1_indexed_dimension() {
        let project = TestProject::new("arrayed_delay_indexed")
            .indexed_dimension("Idx", 3)
            .array_const("input[Idx]", 10.0)
            .aux("delay_time", "1", None)
            .aux("init", "0", None)
            .array_aux("d[Idx]", "DELAY1(input[Idx], delay_time, init)");

        project.assert_compiles();
        project.assert_sim_builds();
    }

    /// Test DELAY in expression context (k * DELAY3(...))
    #[test]
    fn test_arrayed_delay_in_expression() {
        let project = TestProject::new("arrayed_delay_expr")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .aux("k", "42", None)
            .aux("input", "10", None)
            .array_const("delay_a[DimA]", 1.0)
            .array_aux("d[DimA]", "k * DELAY3(input, delay_a[DimA])");

        project.assert_compiles();
        project.assert_sim_builds();
    }
}
