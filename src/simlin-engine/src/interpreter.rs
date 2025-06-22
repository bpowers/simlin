// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::BinaryOp;
use crate::bytecode::CompiledModule;
use crate::compiler::{BuiltinFn, Expr, Module, UnaryOp};
use crate::model::enumerate_modules;
use crate::sim_err;
use crate::vm::{
    CompiledSimulation, DT_OFF, FINAL_TIME_OFF, INITIAL_TIME_OFF, Specs, StepPart, TIME_OFF,
    is_truthy, pulse, ramp, step,
};
use crate::{Ident, Project, Results, compiler, quoteize};
use float_cmp::approx_eq;
use std::borrow::BorrowMut;
use std::collections::HashMap;
use std::rc::Rc;

pub struct ModuleEvaluator<'a> {
    step_part: StepPart,
    off: usize,
    inputs: &'a [f64],
    curr: &'a mut [f64],
    next: &'a mut [f64],
    module: &'a Module,
    sim: &'a Simulation,
}

impl ModuleEvaluator<'_> {
    fn eval(&mut self, expr: &Expr) -> f64 {
        match expr {
            Expr::Const(n, _) => *n,
            Expr::Dt(_) => self.curr[DT_OFF],
            Expr::ModuleInput(off, _) => self.inputs[*off],
            Expr::EvalModule(ident, model_name, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();
                let module_offsets = &self.module.offsets[&self.module.ident];
                let off = self.off + module_offsets[ident].0;
                let module = &self.sim.modules[model_name.as_str()];

                self.sim
                    .calc(self.step_part, module, off, &args, self.curr, self.next);

                0.0
            }
            Expr::Var(off, _) => self.curr[self.off + *off],
            Expr::Subscript(off, r, bounds, _) => {
                let indices: Vec<_> = r.iter().map(|r| self.eval(r)).collect();
                let mut index = 0;
                let max_bounds = bounds.iter().product();
                let mut ok = true;
                assert_eq!(indices.len(), bounds.len());
                for (i, rhs) in indices.into_iter().enumerate() {
                    let bounds = bounds[i];
                    let one_index = rhs.floor() as usize;
                    if one_index == 0 || one_index > bounds {
                        ok = false;
                        break;
                    } else {
                        index *= bounds;
                        index += one_index - 1;
                    }
                }
                if !ok || index > max_bounds {
                    // 3.7.1 Arrays: If a subscript expression results in an invalid subscript index (i.e., it is out of range), a zero (0) MUST be returned[10]
                    // note 10: Note this can be NaN if so specified in the <uses_arrays> tag of the header options block
                    // 0 makes less sense than NaN, so lets do that until real models force us to do otherwise
                    f64::NAN
                } else {
                    self.curr[self.off + *off + index]
                }
            }
            Expr::AssignCurr(off, r) => {
                let rhs = self.eval(r);
                if self.off + *off > self.curr.len() {
                    unreachable!();
                }
                self.curr[self.off + *off] = rhs;
                0.0
            }
            Expr::AssignNext(off, r) => {
                let rhs = self.eval(r);
                self.next[self.off + *off] = rhs;
                0.0
            }
            Expr::If(cond, t, f, _) => {
                let cond: f64 = self.eval(cond);
                if is_truthy(cond) {
                    self.eval(t)
                } else {
                    self.eval(f)
                }
            }
            Expr::Op1(op, l, _) => {
                let l = self.eval(l);
                match op {
                    UnaryOp::Not => (!is_truthy(l)) as i8 as f64,
                    UnaryOp::Transpose => {
                        // For scalars, transpose is identity
                        l
                    }
                }
            }
            Expr::Op2(op, l, r, _) => {
                let l = self.eval(l);
                let r = self.eval(r);
                match op {
                    BinaryOp::Add => l + r,
                    BinaryOp::Sub => l - r,
                    BinaryOp::Exp => l.powf(r),
                    BinaryOp::Mul => l * r,
                    BinaryOp::Div => l / r,
                    BinaryOp::Mod => l.rem_euclid(r),
                    BinaryOp::Gt => (l > r) as i8 as f64,
                    BinaryOp::Gte => (l >= r) as i8 as f64,
                    BinaryOp::Lt => (l < r) as i8 as f64,
                    BinaryOp::Lte => (l <= r) as i8 as f64,
                    BinaryOp::Eq => approx_eq!(f64, l, r) as i8 as f64,
                    BinaryOp::Neq => !approx_eq!(f64, l, r) as i8 as f64,
                    BinaryOp::And => (is_truthy(l) && is_truthy(r)) as i8 as f64,
                    BinaryOp::Or => (is_truthy(l) || is_truthy(r)) as i8 as f64,
                }
            }
            Expr::App(builtin, _) => {
                match builtin {
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => {
                        let off = match builtin {
                            BuiltinFn::Time => TIME_OFF,
                            BuiltinFn::TimeStep => DT_OFF,
                            BuiltinFn::StartTime => INITIAL_TIME_OFF,
                            BuiltinFn::FinalTime => FINAL_TIME_OFF,
                            _ => unreachable!(),
                        };
                        self.curr[off]
                    }
                    BuiltinFn::Abs(a) => self.eval(a).abs(),
                    BuiltinFn::Cos(a) => self.eval(a).cos(),
                    BuiltinFn::Sin(a) => self.eval(a).sin(),
                    BuiltinFn::Tan(a) => self.eval(a).tan(),
                    BuiltinFn::Arccos(a) => self.eval(a).acos(),
                    BuiltinFn::Arcsin(a) => self.eval(a).asin(),
                    BuiltinFn::Arctan(a) => self.eval(a).atan(),
                    BuiltinFn::Exp(a) => self.eval(a).exp(),
                    BuiltinFn::Inf => f64::INFINITY,
                    BuiltinFn::Pi => std::f64::consts::PI,
                    BuiltinFn::Int(a) => self.eval(a).floor(),
                    BuiltinFn::IsModuleInput(ident, _) => {
                        self.module.inputs.contains(ident) as i8 as f64
                    }
                    BuiltinFn::Ln(a) => self.eval(a).ln(),
                    BuiltinFn::Log10(a) => self.eval(a).log10(),
                    BuiltinFn::SafeDiv(a, b, default) => {
                        let a = self.eval(a);
                        let b = self.eval(b);

                        if b != 0.0 {
                            a / b
                        } else if let Some(c) = default {
                            self.eval(c)
                        } else {
                            0.0
                        }
                    }
                    BuiltinFn::Sqrt(a) => self.eval(a).sqrt(),
                    BuiltinFn::Min(a, b) => {
                        if let Some(b) = b {
                            let a = self.eval(a);
                            let b = self.eval(b);
                            // we can't use std::cmp::min here, becuase f64 is only
                            // PartialOrd
                            if a < b { a } else { b }
                        } else {
                            // Single argument array case
                            self.eval_array_min(a)
                        }
                    }
                    BuiltinFn::Mean(args) => {
                        let count = args.len() as f64;
                        let sum: f64 = args.iter().map(|arg| self.eval(arg)).sum();
                        sum / count
                    }
                    BuiltinFn::Max(a, b) => {
                        if let Some(b) = b {
                            let a = self.eval(a);
                            let b = self.eval(b);
                            // we can't use std::cmp::min here, becuase f64 is only
                            // PartialOrd
                            if a > b { a } else { b }
                        } else {
                            // Single argument array case
                            self.eval_array_max(a)
                        }
                    }
                    BuiltinFn::Lookup(id, index, _) => {
                        if !self.module.tables.contains_key(id) {
                            eprintln!("bad lookup for {}", id);
                            unreachable!();
                        }
                        let table = &self.module.tables[id].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);
                        if index.is_nan() {
                            // things get wonky below if we try to binary search for NaN
                            return f64::NAN;
                        }

                        // check if index is below the start of the table
                        {
                            let (x, y) = table[0];
                            if index < x {
                                return y;
                            }
                        }

                        let size = table.len();
                        {
                            let (x, y) = table[size - 1];
                            if index > x {
                                return y;
                            }
                        }
                        // binary search seems to be the most appropriate choice here.
                        let mut low = 0;
                        let mut high = size;
                        while low < high {
                            let mid = low + (high - low) / 2;
                            if table[mid].0 < index {
                                low = mid + 1;
                            } else {
                                high = mid;
                            }
                        }

                        let i = low;
                        if approx_eq!(f64, table[i].0, index) {
                            table[i].1
                        } else {
                            // slope = deltaY/deltaX
                            let slope =
                                (table[i].1 - table[i - 1].1) / (table[i].0 - table[i - 1].0);
                            // y = m*x + b
                            (index - table[i - 1].0) * slope + table[i - 1].1
                        }
                    }
                    BuiltinFn::Pulse(a, b, c) => {
                        let time = self.curr[TIME_OFF];
                        let dt = self.curr[DT_OFF];
                        let volume = self.eval(a);
                        let first_pulse = self.eval(b);
                        let interval = match c.as_ref() {
                            Some(c) => self.eval(c),
                            None => 0.0,
                        };

                        pulse(time, dt, volume, first_pulse, interval)
                    }
                    BuiltinFn::Ramp(a, b, c) => {
                        let time = self.curr[TIME_OFF];
                        let slope = self.eval(a);
                        let start_time = self.eval(b);
                        let end_time = c.as_ref().map(|c| self.eval(c));

                        ramp(time, slope, start_time, end_time)
                    }
                    BuiltinFn::Step(a, b) => {
                        let time = self.curr[TIME_OFF];
                        let dt = self.curr[DT_OFF];
                        let height = self.eval(a);
                        let step_time = self.eval(b);

                        step(time, dt, height, step_time)
                    }
                    BuiltinFn::Size(_) => {
                        // SIZE always returns 1 for scalars in interpreter
                        1.0
                    }
                    BuiltinFn::Sum(expr) => {
                        // Handle array summation properly
                        self.eval_sum(expr)
                    }
                    BuiltinFn::Stddev(expr) => self.eval_array_stddev(expr),
                    BuiltinFn::Rank(_, _) => {
                        // Not implemented in interpreter yet
                        0.0
                    }
                }
            }
        }
    }

    fn eval_sum(&mut self, expr: &Expr) -> f64 {
        self.eval_array_operation(expr, "sum")
    }

    fn eval_array_min(&mut self, expr: &Expr) -> f64 {
        self.eval_array_operation(expr, "min")
    }

    fn eval_array_max(&mut self, expr: &Expr) -> f64 {
        self.eval_array_operation(expr, "max")
    }

    fn eval_array_stddev(&mut self, expr: &Expr) -> f64 {
        self.eval_array_operation(expr, "stddev")
    }

    fn eval_array_operation(&mut self, expr: &Expr, operation: &str) -> f64 {
        match expr {
            Expr::Subscript(off, indices, bounds, _) => {
                // Check if this is a wildcard subscript like a[*]
                // In that case, operate over all array elements
                let has_wildcard = indices
                    .iter()
                    .any(|idx| matches!(idx, Expr::Const(val, _) if *val == 0.0));

                if has_wildcard {
                    // For now, use the same simple approach as the bytecode VM:
                    // Calculate total size based on bounds for wildcard dimensions
                    let wildcard_positions: Vec<_> = indices
                        .iter()
                        .enumerate()
                        .filter_map(|(i, idx)| {
                            if matches!(idx, Expr::Const(val, _) if *val == 0.0) {
                                Some(i)
                            } else {
                                None
                            }
                        })
                        .collect();

                    let total_size = wildcard_positions
                        .iter()
                        .map(|&pos| bounds[pos])
                        .product::<usize>();

                    if total_size == 0 {
                        return match operation {
                            "sum" => 0.0,
                            "min" => f64::INFINITY,
                            "max" => f64::NEG_INFINITY,
                            "stddev" => 0.0,
                            _ => 0.0,
                        };
                    }

                    // Calculate offset adjustment for resolved (non-wildcard) dimensions
                    let mut offset_adjustment = 0;
                    for (i, idx) in indices.iter().enumerate() {
                        if !matches!(idx, Expr::Const(val, _) if *val == 0.0) {
                            // This is a resolved dimension, not a wildcard
                            if let Expr::Const(dim_value, _) = idx {
                                // Convert from 1-based to 0-based indexing and calculate offset
                                let dim_index = (*dim_value as usize).saturating_sub(1);
                                // Calculate stride for this dimension
                                let stride: usize = bounds[i + 1..].iter().product();
                                offset_adjustment += dim_index * stride;
                            }
                        }
                    }

                    let adjusted_base = self.off + *off + offset_adjustment;

                    match operation {
                        "sum" => {
                            let mut sum = 0.0;
                            for i in 0..total_size {
                                sum += self.curr[adjusted_base + i];
                            }
                            sum
                        }
                        "min" => {
                            let mut min_val = f64::INFINITY;
                            for i in 0..total_size {
                                let val = self.curr[adjusted_base + i];
                                if val < min_val {
                                    min_val = val;
                                }
                            }
                            min_val
                        }
                        "max" => {
                            let mut max_val = f64::NEG_INFINITY;
                            for i in 0..total_size {
                                let val = self.curr[adjusted_base + i];
                                if val > max_val {
                                    max_val = val;
                                }
                            }
                            max_val
                        }
                        "stddev" => {
                            if total_size <= 1 {
                                return 0.0;
                            }

                            // First pass: calculate mean
                            let mut sum = 0.0;
                            for i in 0..total_size {
                                sum += self.curr[adjusted_base + i];
                            }
                            let mean = sum / total_size as f64;

                            // Second pass: calculate variance
                            let mut variance_sum = 0.0;
                            for i in 0..total_size {
                                let diff = self.curr[adjusted_base + i] - mean;
                                variance_sum += diff * diff;
                            }
                            let variance = variance_sum / (total_size - 1) as f64; // Sample standard deviation
                            variance.sqrt()
                        }
                        _ => 0.0,
                    }
                } else {
                    // Regular subscript, just evaluate normally
                    self.eval(expr)
                }
            }
            _ => {
                // This might be a complex array expression like a[*]+h[*]
                // Check if it contains array wildcards and handle accordingly
                if self.expr_contains_array_wildcards(expr) {
                    self.eval_array_expression(expr, operation)
                } else {
                    // Regular scalar expression
                    self.eval(expr)
                }
            }
        }
    }

    /// Check if an expression contains array subscripts with wildcards
    fn expr_contains_array_wildcards(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Subscript(_, indices, _, _) => indices
                .iter()
                .any(|idx| matches!(idx, Expr::Const(val, _) if *val == 0.0)),
            Expr::Op1(_, sub_expr, _) => self.expr_contains_array_wildcards(sub_expr),
            Expr::Op2(_, left, right, _) => {
                self.expr_contains_array_wildcards(left)
                    || self.expr_contains_array_wildcards(right)
            }
            Expr::If(cond, left, right, _) => {
                self.expr_contains_array_wildcards(cond)
                    || self.expr_contains_array_wildcards(left)
                    || self.expr_contains_array_wildcards(right)
            }
            Expr::App(builtin, _) => {
                // Check if any arguments contain array wildcards
                match builtin {
                    BuiltinFn::Sum(arg) | BuiltinFn::Stddev(arg) | BuiltinFn::Size(arg) => {
                        self.expr_contains_array_wildcards(arg)
                    }
                    BuiltinFn::Mean(args) => args
                        .iter()
                        .any(|arg| self.expr_contains_array_wildcards(arg)),
                    BuiltinFn::Min(arg, opt_arg) | BuiltinFn::Max(arg, opt_arg) => {
                        self.expr_contains_array_wildcards(arg)
                            || opt_arg
                                .as_ref()
                                .is_some_and(|a| self.expr_contains_array_wildcards(a))
                    }
                    _ => false,
                }
            }
            _ => false,
        }
    }

    /// Evaluate an array expression and apply the specified operation
    fn eval_array_expression(&mut self, expr: &Expr, operation: &str) -> f64 {
        // For now, implement a basic version for simple cross-product cases
        // This handles expressions like SUM(a[*]+h[*]) where arrays have different dimensions

        match expr {
            Expr::Op2(_op, left, right, _) => {
                // For now, directly handle the cross-product case without needing extract_array_info
                let mut results = Vec::new();
                self.eval_cross_product_operation(expr, &mut results);

                if results.is_empty() {
                    // Fallback to regular evaluation if no results were generated
                    return self.eval(expr);
                }

                // Apply the operation to all results
                match operation {
                    "sum" => results.iter().sum(),
                    "min" => results.iter().fold(f64::INFINITY, |acc, &x| acc.min(x)),
                    "max" => results.iter().fold(f64::NEG_INFINITY, |acc, &x| acc.max(x)),
                    "stddev" => {
                        if results.len() <= 1 {
                            0.0
                        } else {
                            let mean = results.iter().sum::<f64>() / results.len() as f64;
                            let variance = results.iter().map(|&x| (x - mean).powi(2)).sum::<f64>()
                                / (results.len() - 1) as f64;
                            variance.sqrt()
                        }
                    }
                    _ => 0.0,
                }
            }
            _ => {
                // For other expression types, fall back to regular evaluation
                self.eval(expr)
            }
        }
    }

    /// Extract array information from an expression (placeholder for now)
    fn extract_array_info(&self, _expr: &Expr) -> Vec<(usize, Vec<usize>)> {
        // Returns (offset, bounds) for each array found in the expression
        // This is a simplified version - full implementation would be more complex
        Vec::new()
    }

    /// Evaluate cross-product operations (basic implementation)
    fn eval_cross_product_operation(&mut self, expr: &Expr, results: &mut Vec<f64>) {
        // Handle array expressions by finding all array subscripts with wildcards
        // and evaluating the expression for all combinations

        if let Some(array_combinations) = self.find_array_combinations(expr) {
            // We found arrays with wildcards - evaluate for all combinations
            self.eval_expression_for_combinations(expr, &array_combinations, results);
        } else {
            // No arrays found, just evaluate normally
            results.push(self.eval(expr));
        }
    }

    /// Find all array combinations in an expression
    fn find_array_combinations(&self, expr: &Expr) -> Option<Vec<(usize, usize, usize)>> {
        let mut arrays = Vec::new();
        self.collect_wildcard_arrays(expr, &mut arrays);

        if arrays.is_empty() {
            None
        } else {
            Some(arrays)
        }
    }

    /// Recursively collect all array subscripts with wildcards
    fn collect_wildcard_arrays(&self, expr: &Expr, arrays: &mut Vec<(usize, usize, usize)>) {
        match expr {
            Expr::Subscript(off, indices, bounds, _) => {
                let has_wildcard = indices
                    .iter()
                    .any(|idx| matches!(idx, Expr::Const(val, _) if *val == 0.0));

                if has_wildcard {
                    let size = bounds.iter().product::<usize>();
                    arrays.push((*off, size, arrays.len())); // (offset, size, index)
                }
            }
            Expr::Op1(_, sub_expr, _) => {
                self.collect_wildcard_arrays(sub_expr, arrays);
            }
            Expr::Op2(_, left, right, _) => {
                self.collect_wildcard_arrays(left, arrays);
                self.collect_wildcard_arrays(right, arrays);
            }
            Expr::If(cond, left, right, _) => {
                self.collect_wildcard_arrays(cond, arrays);
                self.collect_wildcard_arrays(left, arrays);
                self.collect_wildcard_arrays(right, arrays);
            }
            _ => {} // Other expression types don't contain arrays
        }
    }

    /// Evaluate an expression for all array combinations
    fn eval_expression_for_combinations(
        &mut self,
        expr: &Expr,
        arrays: &[(usize, usize, usize)],
        results: &mut Vec<f64>,
    ) {
        if arrays.is_empty() {
            results.push(self.eval(expr));
            return;
        }

        // For now, implement a simple case for single arrays or element-wise operations
        if arrays.len() == 1 {
            // Single array case - element-wise operation
            let (offset, size, _) = arrays[0];
            for i in 0..size {
                // Temporarily modify the array state to evaluate for this element
                let result = self.eval_with_array_index(expr, offset, i);
                results.push(result);
            }
        } else if arrays.len() == 2 {
            // Two arrays case - could be element-wise or cross-product
            let (offset1, size1, _) = arrays[0];
            let (offset2, size2, _) = arrays[1];

            // Simple heuristic based on operation type and array relationship
            // For multiplication/division with same-sized arrays: assume element-wise
            // For addition/subtraction: check if arrays are likely same dimension
            let use_element_wise = if size1 == size2 {
                // Same size arrays - check operation type and offset relationship
                if self.expr_contains_multiplication_or_division(expr) {
                    // Multiplication/division operations are more likely element-wise
                    true
                } else {
                    // Addition/subtraction - check offset proximity
                    let offset_diff = if offset1 > offset2 {
                        offset1 - offset2
                    } else {
                        offset2 - offset1
                    };
                    offset_diff <= size1.max(size2)
                }
            } else {
                // Different sizes - always cross-product
                false
            };

            if use_element_wise {
                // Element-wise operation: a[i] op b[i]
                for i in 0..size1 {
                    let result = self.eval_with_two_array_indices(expr, offset1, i, offset2, i);
                    results.push(result);
                }
            } else {
                // Cross-product operation: a[i] op b[j] for all i,j
                for i in 0..size1 {
                    for j in 0..size2 {
                        let result = self.eval_with_two_array_indices(expr, offset1, i, offset2, j);
                        results.push(result);
                    }
                }
            }
        } else {
            // More complex cases - fall back to regular evaluation for now
            results.push(self.eval(expr));
        }
    }

    /// Evaluate expression with a specific array index
    fn eval_with_array_index(&mut self, expr: &Expr, array_offset: usize, index: usize) -> f64 {
        // For now, handle simple cases directly
        match expr {
            Expr::Subscript(off, indices, _bounds, _) => {
                if *off == array_offset {
                    // This is the array we're substituting - return the specific element
                    self.curr[self.off + *off + index]
                } else {
                    // Different array, evaluate normally
                    self.eval(expr)
                }
            }
            _ => self.eval(expr),
        }
    }

    /// Evaluate expression with two specific array indices
    fn eval_with_two_array_indices(
        &mut self,
        expr: &Expr,
        offset1: usize,
        index1: usize,
        offset2: usize,
        index2: usize,
    ) -> f64 {
        // Handle simple binary operations between two arrays
        match expr {
            Expr::Op2(op, left, right, _) => {
                let left_val =
                    self.eval_with_array_substitution(left, offset1, index1, offset2, index2);
                let right_val =
                    self.eval_with_array_substitution(right, offset1, index1, offset2, index2);

                match op {
                    BinaryOp::Add => left_val + right_val,
                    BinaryOp::Sub => left_val - right_val,
                    BinaryOp::Mul => left_val * right_val,
                    BinaryOp::Div => left_val / right_val,
                    _ => 0.0,
                }
            }
            _ => self.eval(expr),
        }
    }

    /// Helper to evaluate an expression with array substitutions
    fn eval_with_array_substitution(
        &mut self,
        expr: &Expr,
        offset1: usize,
        index1: usize,
        offset2: usize,
        index2: usize,
    ) -> f64 {
        match expr {
            Expr::Subscript(off, indices, _bounds, _) => {
                let has_wildcard = indices
                    .iter()
                    .any(|idx| matches!(idx, Expr::Const(val, _) if *val == 0.0));

                if has_wildcard {
                    if *off == offset1 {
                        self.curr[self.off + *off + index1]
                    } else if *off == offset2 {
                        self.curr[self.off + *off + index2]
                    } else {
                        // Different array, evaluate normally (but this shouldn't happen)
                        self.eval(expr)
                    }
                } else {
                    // No wildcard, evaluate normally
                    self.eval(expr)
                }
            }
            Expr::Op2(op, left, right, _) => {
                let left_val =
                    self.eval_with_array_substitution(left, offset1, index1, offset2, index2);
                let right_val =
                    self.eval_with_array_substitution(right, offset1, index1, offset2, index2);

                match op {
                    BinaryOp::Add => left_val + right_val,
                    BinaryOp::Sub => left_val - right_val,
                    BinaryOp::Mul => left_val * right_val,
                    BinaryOp::Div => left_val / right_val,
                    _ => 0.0,
                }
            }
            Expr::Op1(op, sub_expr, _) => {
                let val =
                    self.eval_with_array_substitution(sub_expr, offset1, index1, offset2, index2);
                match op {
                    UnaryOp::Not => (!is_truthy(val)) as i8 as f64,
                    UnaryOp::Transpose => {
                        // For scalars in array context, transpose is identity
                        val
                    }
                }
            }
            Expr::If(cond, true_expr, false_expr, _) => {
                let cond_val =
                    self.eval_with_array_substitution(cond, offset1, index1, offset2, index2);
                if is_truthy(cond_val) {
                    self.eval_with_array_substitution(true_expr, offset1, index1, offset2, index2)
                } else {
                    self.eval_with_array_substitution(false_expr, offset1, index1, offset2, index2)
                }
            }
            // For other expression types (constants, variables, etc.), evaluate normally
            _ => self.eval(expr),
        }
    }

    /// Check if expression contains multiplication or division operations
    fn expr_contains_multiplication_or_division(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Op2(op, left, right, _) => {
                matches!(op, BinaryOp::Mul | BinaryOp::Div)
                    || self.expr_contains_multiplication_or_division(left)
                    || self.expr_contains_multiplication_or_division(right)
            }
            Expr::Op1(_, sub_expr, _) => self.expr_contains_multiplication_or_division(sub_expr),
            Expr::If(cond, true_expr, false_expr, _) => {
                self.expr_contains_multiplication_or_division(cond)
                    || self.expr_contains_multiplication_or_division(true_expr)
                    || self.expr_contains_multiplication_or_division(false_expr)
            }
            _ => false,
        }
    }
}

#[derive(Debug)]
pub struct Simulation {
    pub(crate) modules: HashMap<Ident, Module>,
    specs: Specs,
    root: String,
    offsets: HashMap<Ident, usize>,
}

impl Simulation {
    pub fn new(project: &Project, main_model_name: &str) -> crate::Result<Self> {
        if !project.models.contains_key(main_model_name) {
            return sim_err!(
                NotSimulatable,
                format!("no model named '{}' to simulate", main_model_name)
            );
        }

        let modules = {
            let project_models: HashMap<_, _> = project
                .models
                .iter()
                .map(|(name, model)| (name.as_str(), model.as_ref()))
                .collect();
            // then pull in all the module instantiations the main model depends on
            enumerate_modules(&project_models, main_model_name, |model| model.name.clone())?
        };

        let module_names: Vec<&str> = {
            let mut module_names: Vec<&str> = modules.keys().map(|id| id.as_str()).collect();
            module_names.sort_unstable();

            let mut sorted_names = vec![main_model_name];
            sorted_names.extend(module_names.into_iter().filter(|n| *n != main_model_name));
            sorted_names
        };

        let mut compiled_modules: HashMap<Ident, Module> = HashMap::new();
        for name in module_names {
            let distinct_inputs = &modules[name];
            for inputs in distinct_inputs.iter() {
                let model = Rc::clone(&project.models[name]);
                let is_root = name == main_model_name;
                let module = Module::new(project, model, inputs, is_root)?;
                compiled_modules.insert(name.to_string(), module);
            }
        }

        let specs = Specs::from(&project.datamodel.sim_specs);

        let offsets = compiler::calc_flattened_offsets(project, main_model_name);
        let offsets: HashMap<Ident, usize> =
            offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

        Ok(Simulation {
            modules: compiled_modules,
            specs,
            root: main_model_name.to_string(),
            offsets,
        })
    }

    pub fn compile(&self) -> crate::Result<CompiledSimulation> {
        let modules: crate::Result<HashMap<String, CompiledModule>> = self
            .modules
            .iter()
            .map(|(name, module)| module.compile().map(|module| (name.clone(), module)))
            .collect();

        Ok(CompiledSimulation {
            modules: modules?,
            specs: self.specs.clone(),
            root: self.root.clone(),
            offsets: self.offsets.clone(),
        })
    }

    pub fn runlist_order(&self) -> Vec<Ident> {
        calc_flattened_order(self, "main")
    }

    pub fn debug_print_runlists(&self, _model_name: &str) {
        let mut model_names: Vec<_> = self.modules.keys().collect();
        model_names.sort_unstable();
        for model_name in model_names {
            eprintln!("\n\nMODEL: {}", model_name);
            let module = &self.modules[model_name];
            let offsets = &module.offsets[model_name];
            let mut idents: Vec<_> = offsets.keys().collect();
            idents.sort_unstable();

            eprintln!("offsets");
            for ident in idents {
                let (off, size) = offsets[ident];
                eprintln!("\t{}: {}, {}", ident, off, size);
            }

            eprintln!("\ninital runlist:");
            for expr in module.runlist_initials.iter() {
                eprintln!("\t{}", compiler::pretty(expr));
            }

            eprintln!("\nflows runlist:");
            for expr in module.runlist_flows.iter() {
                eprintln!("\t{}", compiler::pretty(expr));
            }

            eprintln!("\nstocks runlist:");
            for expr in module.runlist_stocks.iter() {
                eprintln!("\t{}", compiler::pretty(expr));
            }
        }
    }

    fn calc(
        &self,
        step_part: StepPart,
        module: &Module,
        module_off: usize,
        module_inputs: &[f64],
        curr: &mut [f64],
        next: &mut [f64],
    ) {
        let runlist = match step_part {
            StepPart::Initials => &module.runlist_initials,
            StepPart::Flows => &module.runlist_flows,
            StepPart::Stocks => &module.runlist_stocks,
        };

        let mut step = ModuleEvaluator {
            step_part,
            off: module_off,
            curr,
            next,
            module,
            inputs: module_inputs,
            sim: self,
        };

        for expr in runlist.iter() {
            step.eval(expr);
        }
    }

    fn n_slots(&self, module_name: &str) -> usize {
        self.modules[module_name].n_slots
    }

    pub fn run_to_end(&self) -> crate::Result<Results> {
        let spec = &self.specs;
        if spec.stop < spec.start {
            return sim_err!(BadSimSpecs, "".to_string());
        }
        let save_step = if spec.save_step > spec.dt {
            spec.save_step
        } else {
            spec.dt
        };
        let n_chunks: usize = ((spec.stop - spec.start) / save_step + 1.0) as usize;
        let save_every = std::cmp::max(1, (spec.save_step / spec.dt + 0.5).floor() as usize);

        let dt = spec.dt;
        let stop = spec.stop;

        let n_slots = self.n_slots(&self.root);

        let module = &self.modules[&self.root];

        let slab: Vec<f64> = vec![0.0; n_slots * (n_chunks + 1)];
        let mut boxed_slab = slab.into_boxed_slice();
        {
            let mut slabs = boxed_slab.chunks_mut(n_slots);

            // let mut results: Vec<&[f64]> = Vec::with_capacity(n_chunks + 1);

            let module_inputs: &[f64] = &[];

            let mut curr = slabs.next().unwrap();
            let mut next = slabs.next().unwrap();
            curr[TIME_OFF] = self.specs.start;
            curr[DT_OFF] = dt;
            curr[INITIAL_TIME_OFF] = self.specs.start;
            curr[FINAL_TIME_OFF] = self.specs.stop;
            self.calc(StepPart::Initials, module, 0, module_inputs, curr, next);
            let mut is_initial_timestep = true;
            let mut step = 0;
            loop {
                self.calc(StepPart::Flows, module, 0, module_inputs, curr, next);
                self.calc(StepPart::Stocks, module, 0, module_inputs, curr, next);
                next[TIME_OFF] = curr[TIME_OFF] + dt;
                next[DT_OFF] = dt;
                curr[INITIAL_TIME_OFF] = self.specs.start;
                curr[FINAL_TIME_OFF] = self.specs.stop;
                step += 1;
                if step != save_every && !is_initial_timestep {
                    let curr = curr.borrow_mut();
                    curr.copy_from_slice(next);
                } else {
                    curr = next;
                    let maybe_next = slabs.next();
                    if maybe_next.is_none() {
                        break;
                    }
                    next = maybe_next.unwrap();
                    step = 0;
                    is_initial_timestep = false;
                }
            }
            // ensure we've calculated stock + flow values for the dt <= end_time
            assert!(curr[TIME_OFF] > stop);
        }

        Ok(Results {
            offsets: self.offsets.clone(),
            data: boxed_slab,
            step_size: n_slots,
            step_count: n_chunks,
            specs: spec.clone(),
            is_vensim: false,
        })
    }
}

pub(crate) fn calc_flattened_order(sim: &Simulation, model_name: &str) -> Vec<Ident> {
    let is_root = model_name == "main";

    let module = &sim.modules[model_name];

    let mut offsets: Vec<Ident> = Vec::with_capacity(module.runlist_order.len() + 1);

    if is_root {
        offsets.push("time".to_owned());
    }

    for ident in module.runlist_order.iter() {
        // FIXME: this isnt' quite right (assumes no regular var has same name as module)
        if sim.modules.contains_key(ident) {
            let sub_var_names = calc_flattened_order(sim, ident);
            for sub_name in sub_var_names.iter() {
                offsets.push(format!("{}.{}", quoteize(ident), quoteize(sub_name)));
            }
        } else {
            offsets.push(quoteize(ident));
        }
    }

    offsets
}

#[test]
fn nan_is_approx_eq() {
    assert!(approx_eq!(f64, f64::NAN, f64::NAN));
}
