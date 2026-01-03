// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[cfg(test)]
use crate::ast::ArrayView;
use crate::ast::{Ast, BinaryOp};
use crate::bytecode::CompiledModule;
use crate::common::{Canonical, Ident, canonicalize};
use crate::compiler::{BuiltinFn, Expr, Module, SubscriptIndex, UnaryOp};
use crate::model::{ModuleInputSet, enumerate_modules};
use crate::sim_err;
use crate::vm::{
    CompiledSimulation, DT_OFF, FINAL_TIME_OFF, IMPLICIT_VAR_COUNT, INITIAL_TIME_OFF, ModuleKey,
    Specs, StepPart, SubscriptIterator, TIME_OFF, is_truthy, pulse, ramp, step,
};
use crate::{Project, Results, Variable, compiler};
use float_cmp::approx_eq;
use std::borrow::BorrowMut;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

/// Maps a flat index from transposed array space to original array space
///
/// For a 2D array [rows, cols], transpose maps [r,c] -> [c,r]
/// In flat indexing: idx = r*cols + c becomes idx' = c*rows + r
///
/// # Arguments
/// * `transposed_flat_idx` - The flat index in the transposed array
/// * `transposed_dims` - The dimensions of the transposed array
///
/// # Returns
/// The corresponding flat index in the original array
pub fn transpose_flat_index(transposed_flat_idx: usize, transposed_dims: &[usize]) -> usize {
    if transposed_dims.is_empty() || transposed_dims.len() == 1 {
        // 0D or 1D arrays are unchanged by transpose
        return transposed_flat_idx;
    }

    // Get original dimensions by reversing transposed dimensions
    let mut orig_dims = transposed_dims.to_vec();
    orig_dims.reverse();

    // Convert flat index to coordinates in transposed space
    let mut coords = Vec::with_capacity(transposed_dims.len());
    let mut remaining = transposed_flat_idx;
    for &dim in transposed_dims.iter().rev() {
        coords.push(remaining % dim);
        remaining /= dim;
    }
    coords.reverse();

    // Reverse coordinates to get original space coordinates
    coords.reverse();

    // Convert to flat index in original space
    let mut orig_idx = 0;
    for (i, &coord) in coords.iter().enumerate() {
        orig_idx = orig_idx * orig_dims[i] + coord;
    }

    orig_idx
}

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
    /// Helper to find array dimensions from an expression (returns a cloned dims vector)
    fn find_array_dims(expr: &Expr) -> Option<Vec<usize>> {
        match expr {
            Expr::StaticSubscript(_, view, _) | Expr::TempArray(_, view, _) => {
                Some(view.dims.clone())
            }
            Expr::Subscript(_, indices, bounds, _) => {
                // For dynamic subscripts with ranges, compute effective dimensions
                // Only Single indices collapse a dimension; Range indices preserve it
                let mut dims = Vec::new();
                for (i, idx) in indices.iter().enumerate() {
                    match idx {
                        SubscriptIndex::Single(_) => {
                            // Single index collapses this dimension
                        }
                        SubscriptIndex::Range(_, _) => {
                            // Range preserves dimension - will need runtime evaluation
                            // For now, use the full bound as a conservative estimate
                            // The actual iteration will handle the range correctly
                            dims.push(bounds[i]);
                        }
                    }
                }
                if dims.is_empty() {
                    None // All dimensions collapsed = scalar
                } else {
                    Some(dims)
                }
            }
            Expr::Op1(UnaryOp::Transpose, inner, _) => Self::find_array_dims(inner).map(|dims| {
                let mut dims = dims;
                dims.reverse();
                dims
            }),
            Expr::Op1(_, inner, _) => Self::find_array_dims(inner),
            Expr::Op2(_, left, right, _) => {
                Self::find_array_dims(left).or_else(|| Self::find_array_dims(right))
            }
            Expr::If(_, t, f, _) => Self::find_array_dims(t).or_else(|| Self::find_array_dims(f)),
            Expr::App(builtin, _) => match builtin {
                BuiltinFn::Abs(e)
                | BuiltinFn::Arccos(e)
                | BuiltinFn::Arcsin(e)
                | BuiltinFn::Arctan(e)
                | BuiltinFn::Cos(e)
                | BuiltinFn::Exp(e)
                | BuiltinFn::Int(e)
                | BuiltinFn::Ln(e)
                | BuiltinFn::Log10(e)
                | BuiltinFn::Sin(e)
                | BuiltinFn::Sqrt(e)
                | BuiltinFn::Tan(e) => Self::find_array_dims(e),
                BuiltinFn::Max(_, None) | BuiltinFn::Min(_, None) => None,
                BuiltinFn::Max(_, Some(_)) | BuiltinFn::Min(_, Some(_)) => None,
                BuiltinFn::SafeDiv(a, b, _) => {
                    Self::find_array_dims(a).or_else(|| Self::find_array_dims(b))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Helper to evaluate an expression at a specific array index
    fn eval_at_index(&mut self, expr: &Expr, index: usize) -> f64 {
        match expr {
            Expr::StaticSubscript(off, view, _) => {
                let base_off = self.off + *off;
                let total_elements: usize = view.dims.iter().product();
                if index >= total_elements {
                    return f64::NAN;
                }

                // Build sparse lookup for O(1) access
                let sparse_map: std::collections::HashMap<usize, &[usize]> = view
                    .sparse
                    .iter()
                    .map(|s| (s.dim_index, s.parent_offsets.as_slice()))
                    .collect();

                let mut remainder = index;
                let mut idx = view.offset;
                for (dim_idx, &dim_size) in view.dims.iter().enumerate().rev() {
                    let coord = remainder % dim_size;
                    remainder /= dim_size;
                    let parent_coord = if let Some(offsets) = sparse_map.get(&dim_idx) {
                        if coord < offsets.len() {
                            offsets[coord]
                        } else {
                            coord
                        }
                    } else {
                        coord
                    };
                    idx += parent_coord * view.strides[dim_idx] as usize;
                }
                self.curr[base_off + idx]
            }
            Expr::TempArray(id, view, _) => {
                let id = *id as usize;
                if id >= self.sim.temp_offsets.len() - 1 {
                    return f64::NAN;
                }
                let start = self.sim.temp_offsets[id];
                let temps = (*self.sim.temps).borrow();
                let size: usize = view.dims.iter().product();
                debug_assert!(
                    view.offset == 0 && view.sparse.is_empty() && view.is_contiguous(),
                    "TempArray view should be contiguous and rebased"
                );
                if index < size {
                    temps[start + index]
                } else {
                    f64::NAN
                }
            }
            Expr::Op1(op, inner, _) => {
                let val = self.eval_at_index(inner, index);
                match op {
                    UnaryOp::Not => {
                        if is_truthy(val) {
                            0.0
                        } else {
                            1.0
                        }
                    }
                    UnaryOp::Transpose => {
                        if let Some(dims) = Self::find_array_dims(expr) {
                            let orig_idx = transpose_flat_index(index, &dims);
                            self.eval_at_index(inner, orig_idx)
                        } else {
                            val
                        }
                    }
                }
            }
            Expr::Op2(op, left, right, _) => {
                let lval = if Self::find_array_dims(left).is_some() {
                    self.eval_at_index(left, index)
                } else {
                    self.eval(left)
                };
                let rval = if Self::find_array_dims(right).is_some() {
                    self.eval_at_index(right, index)
                } else {
                    self.eval(right)
                };
                match op {
                    BinaryOp::Add => lval + rval,
                    BinaryOp::Sub => lval - rval,
                    BinaryOp::Mul => lval * rval,
                    BinaryOp::Div => lval / rval,
                    BinaryOp::Mod => lval % rval,
                    BinaryOp::Exp => lval.powf(rval),
                    BinaryOp::Lt => {
                        if lval < rval {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::Lte => {
                        if lval <= rval {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::Gt => {
                        if lval > rval {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::Gte => {
                        if lval >= rval {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::Eq => {
                        if (lval - rval).abs() < f64::EPSILON {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::Neq => {
                        if (lval - rval).abs() >= f64::EPSILON {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::And => {
                        if is_truthy(lval) && is_truthy(rval) {
                            1.0
                        } else {
                            0.0
                        }
                    }
                    BinaryOp::Or => {
                        if is_truthy(lval) || is_truthy(rval) {
                            1.0
                        } else {
                            0.0
                        }
                    }
                }
            }
            Expr::If(cond, t, f, _) => {
                let cond_val = if Self::find_array_dims(cond).is_some() {
                    self.eval_at_index(cond, index)
                } else {
                    self.eval(cond)
                };
                if is_truthy(cond_val) {
                    if Self::find_array_dims(t).is_some() {
                        self.eval_at_index(t, index)
                    } else {
                        self.eval(t)
                    }
                } else if Self::find_array_dims(f).is_some() {
                    self.eval_at_index(f, index)
                } else {
                    self.eval(f)
                }
            }
            Expr::App(builtin, _) => {
                // For simple functions, evaluate the argument at index
                match builtin {
                    BuiltinFn::Abs(e) => self.eval_at_index(e, index).abs(),
                    BuiltinFn::Cos(e) => self.eval_at_index(e, index).cos(),
                    BuiltinFn::Sin(e) => self.eval_at_index(e, index).sin(),
                    BuiltinFn::Tan(e) => self.eval_at_index(e, index).tan(),
                    BuiltinFn::Exp(e) => self.eval_at_index(e, index).exp(),
                    BuiltinFn::Ln(e) => self.eval_at_index(e, index).ln(),
                    BuiltinFn::Log10(e) => self.eval_at_index(e, index).log10(),
                    BuiltinFn::Sqrt(e) => self.eval_at_index(e, index).sqrt(),
                    BuiltinFn::Int(e) => self.eval_at_index(e, index).trunc(),
                    BuiltinFn::Arccos(e) => self.eval_at_index(e, index).acos(),
                    BuiltinFn::Arcsin(e) => self.eval_at_index(e, index).asin(),
                    BuiltinFn::Arctan(e) => self.eval_at_index(e, index).atan(),
                    _ => self.eval(expr), // Fall back to scalar eval for other builtins
                }
            }
            // For non-array expressions, just evaluate normally
            _ => self.eval(expr),
        }
    }

    /// Helper to iterate over all elements in an array expression
    fn iter_array_elements<F>(&mut self, expr: &Expr, mut f: F)
    where
        F: FnMut(f64),
    {
        match expr {
            Expr::StaticSubscript(off, view, _) => {
                let base_off = self.off + *off;
                let total_elements = view.dims.iter().product::<usize>();

                // Build sparse lookup for O(1) access
                let sparse_map: std::collections::HashMap<usize, &[usize]> = view
                    .sparse
                    .iter()
                    .map(|s| (s.dim_index, s.parent_offsets.as_slice()))
                    .collect();

                for i in 0..total_elements {
                    let mut remainder = i;
                    let mut idx = view.offset;
                    for (dim_idx, &dim_size) in view.dims.iter().enumerate().rev() {
                        let coord = remainder % dim_size;
                        remainder /= dim_size;
                        // If this dimension is sparse, translate coord through parent_offsets
                        let parent_coord = if let Some(offsets) = sparse_map.get(&dim_idx) {
                            debug_assert!(
                                coord < offsets.len(),
                                "SparseInfo invariant violated: coord={} offsets.len()={} dim_size={}",
                                coord,
                                offsets.len(),
                                dim_size
                            );
                            offsets[coord]
                        } else {
                            coord
                        };
                        idx += parent_coord * view.strides[dim_idx] as usize;
                    }
                    f(self.curr[base_off + idx]);
                }
            }
            Expr::TempArray(id, view, _) => {
                let id = *id as usize;
                if id >= self.sim.temp_offsets.len() - 1 {
                    panic!("Invalid temporary ID: {id}");
                }

                let start = self.sim.temp_offsets[id];
                let temps = (*self.sim.temps).borrow();
                let size = view.dims.iter().product::<usize>();

                for i in 0..size {
                    f(temps[start + i]);
                }
            }
            Expr::Subscript(off, indices, bounds, _) => {
                // Handle dynamic subscripts with potential ranges
                let base_off = self.off + *off;

                // Evaluate index bounds for each dimension
                // For Single: evaluate to get a single index (1-based)
                // For Range: evaluate start and end to get bounds (1-based)
                let mut dim_ranges: Vec<(usize, usize)> = Vec::with_capacity(indices.len());
                for (i, idx) in indices.iter().enumerate() {
                    match idx {
                        SubscriptIndex::Single(e) => {
                            let idx_1based = self.eval(e).floor() as usize;
                            let idx_0based = idx_1based.saturating_sub(1);
                            dim_ranges.push((idx_0based, idx_0based + 1));
                        }
                        SubscriptIndex::Range(start_expr, end_expr) => {
                            let start_val = self.eval(start_expr);
                            let end_val = self.eval(end_expr);
                            let start_1based = start_val.floor() as usize;
                            let end_1based = end_val.floor() as usize;
                            let start_0based = start_1based.saturating_sub(1);
                            // end is inclusive in XMILE, so add 1 for exclusive end
                            let end_0based = end_1based.min(bounds[i]);
                            dim_ranges.push((start_0based, end_0based));
                        }
                    }
                }
                // Compute strides for row-major layout
                let mut strides: Vec<usize> = vec![1; bounds.len()];
                for i in (0..bounds.len().saturating_sub(1)).rev() {
                    strides[i] = strides[i + 1] * bounds[i + 1];
                }

                // Iterate over all elements in the ranges
                fn iterate_ranges(
                    dim_ranges: &[(usize, usize)],
                    strides: &[usize],
                    dim_idx: usize,
                    current_offset: usize,
                    base_off: usize,
                    curr: &[f64],
                    f: &mut dyn FnMut(f64),
                ) {
                    if dim_idx >= dim_ranges.len() {
                        f(curr[base_off + current_offset]);
                        return;
                    }
                    let (start, end) = dim_ranges[dim_idx];
                    for i in start..end {
                        iterate_ranges(
                            dim_ranges,
                            strides,
                            dim_idx + 1,
                            current_offset + i * strides[dim_idx],
                            base_off,
                            curr,
                            f,
                        );
                    }
                }

                iterate_ranges(&dim_ranges, &strides, 0, 0, base_off, self.curr, &mut f);
            }
            // Handle composite expressions with arrays (e.g., a[*]*b[*]/DT)
            _ => {
                if let Some(dims) = Self::find_array_dims(expr) {
                    let total_elements: usize = dims.iter().product();
                    for i in 0..total_elements {
                        f(self.eval_at_index(expr, i));
                    }
                } else {
                    f(self.eval(expr));
                }
            }
        }
    }

    /// Helper to get the size of an array.
    /// For dynamic subscripts with ranges, evaluates range bounds at runtime.
    fn get_array_size(&mut self, expr: &Expr) -> usize {
        match expr {
            Expr::StaticSubscript(_, view, _) | Expr::TempArray(_, view, _) => {
                view.dims.iter().product()
            }
            Expr::TempArrayElement(_, _, _, _) => 1, // Single element
            Expr::Subscript(_, indices, bounds, _) => {
                // For dynamic subscripts, compute actual range sizes at runtime
                let mut size = 1usize;
                for (i, idx) in indices.iter().enumerate() {
                    match idx {
                        SubscriptIndex::Single(_) => {
                            // Single index collapses dimension - contributes 1
                        }
                        SubscriptIndex::Range(start_expr, end_expr) => {
                            // Evaluate range bounds at runtime
                            let start_1based = self.eval(start_expr).floor() as usize;
                            let end_1based = self.eval(end_expr).floor() as usize;
                            // Clamp and compute range size (1-based inclusive range)
                            let start_0based = start_1based.saturating_sub(1);
                            let end_0based = end_1based.min(bounds[i]);
                            let range_size = end_0based.saturating_sub(start_0based);
                            size *= range_size; // Empty ranges result in size 0
                        }
                    }
                }
                size
            }
            _ => Self::find_array_dims(expr)
                .map(|dims| dims.iter().product())
                .unwrap_or(1),
        }
    }

    /// Helper to apply a reduction operation over array elements
    fn reduce_array<F>(&mut self, expr: &Expr, init: f64, mut reducer: F) -> f64
    where
        F: FnMut(f64, f64) -> f64,
    {
        let mut acc = init;
        self.iter_array_elements(expr, |val| {
            acc = reducer(acc, val);
        });
        acc
    }

    /// Helper to calculate mean of an array
    fn array_mean(&mut self, expr: &Expr) -> f64 {
        let size = self.get_array_size(expr);
        if size == 0 {
            return 0.0;
        }

        let sum = self.reduce_array(expr, 0.0, |acc, val| acc + val);
        sum / size as f64
    }

    /// Helper to calculate standard deviation of an array
    fn array_stddev(&mut self, expr: &Expr) -> f64 {
        let size = self.get_array_size(expr);
        if size <= 1 {
            return 0.0;
        }

        // First pass: calculate mean
        let mean = self.array_mean(expr);

        // Second pass: calculate variance
        let mut variance = 0.0;
        self.iter_array_elements(expr, |val| {
            let diff = val - mean;
            variance += diff * diff;
        });

        // Sample standard deviation (n-1 divisor)
        (variance / (size - 1) as f64).sqrt()
    }

    /// Extract the table identifier and element offset from a lookup table expression.
    ///
    /// The offset lookup strategy differs by expression type:
    /// - Expr::Var: Uses range-based lookup because the offset has already been computed
    ///   (base + element), so we find the variable by checking which range contains it.
    /// - Expr::StaticSubscript/Subscript: Uses exact match because the offset is just
    ///   the base of the array, and the element offset comes from view.offset or
    ///   subscript indices computed at runtime.
    fn extract_table_info(&mut self, table_expr: &Expr) -> Option<(Ident<Canonical>, usize)> {
        match table_expr {
            compiler::Expr::Var(off, _) => {
                // Could be a simple scalar table or an element of an arrayed table
                // (when subscript was static and compiled to a direct Var reference).
                // The offset is already the final computed offset (base + element),
                // so we find the variable by checking which range contains it.
                let module_offsets = &self.module.offsets[&self.module.ident];
                let (ident, base_off) = module_offsets
                    .iter()
                    .find(|(_, (base, size))| *off >= *base && *off < *base + *size)
                    .map(|(k, (base, _))| (k.clone(), *base))?;
                let elem_off = *off - base_off;
                Some((ident, elem_off))
            }
            compiler::Expr::StaticSubscript(off, view, _) => {
                // Static subscript - offset is the base of the array, element comes from view.offset
                let module_offsets = &self.module.offsets[&self.module.ident];
                let ident = module_offsets
                    .iter()
                    .find(|(_, (o, _))| *o == *off)
                    .map(|(k, _)| k.clone())?;
                Some((ident, view.offset))
            }
            compiler::Expr::Subscript(off, subscript_indices, dim_sizes, _) => {
                // Subscripted table reference - offset is the base, compute element offset at runtime
                let module_offsets = &self.module.offsets[&self.module.ident];
                let ident = module_offsets
                    .iter()
                    .find(|(_, (o, _))| *o == *off)
                    .map(|(k, _)| k.clone())?;

                // Compute linear offset from subscript indices
                let mut offset = 0usize;
                let mut stride = 1usize;
                for (i, sub_idx) in subscript_indices.iter().enumerate().rev() {
                    let idx = match sub_idx {
                        compiler::SubscriptIndex::Single(expr) => {
                            // Evaluate expression and convert to 0-based
                            (self.eval(expr) as usize).saturating_sub(1)
                        }
                        compiler::SubscriptIndex::Range(_, _) => {
                            eprintln!("range subscripts not supported in lookup tables");
                            return None;
                        }
                    };
                    offset += idx * stride;
                    stride *= dim_sizes.get(i).copied().unwrap_or(1);
                }
                Some((ident, offset))
            }
            _ => {
                eprintln!("unsupported expression type for lookup table reference: {table_expr:?}");
                None
            }
        }
    }

    fn eval(&mut self, expr: &Expr) -> f64 {
        match expr {
            Expr::Const(n, _) => *n,
            Expr::Dt(_) => self.curr[DT_OFF],
            Expr::ModuleInput(off, _) => self.inputs[*off],
            Expr::EvalModule(ident, model_name, input_set, args) => {
                let args: Vec<f64> = args.iter().map(|arg| self.eval(arg)).collect();
                let module_offsets = &self.module.offsets[&self.module.ident];
                let off = self.off + module_offsets[ident].0;
                let module_key: ModuleKey = (model_name.clone(), input_set.clone());
                let module = &self.sim.modules[&module_key];

                self.sim
                    .calc(self.step_part, module, off, &args, self.curr, self.next);

                0.0
            }
            Expr::Var(off, _) => self.curr[self.off + *off],
            Expr::StaticSubscript(off, view, _) => {
                // Static subscripts represent a pre-computed view into an array
                // The view contains offset and strides for efficient access
                self.curr[self.off + *off + view.offset]
            }
            Expr::Subscript(off, r, bounds, _) => {
                use crate::compiler::SubscriptIndex;

                // Evaluate all subscript indices - for now, range subscripts are not supported
                // in scalar context (they should be handled via iteration)
                let indices: Vec<_> = r
                    .iter()
                    .map(|idx| match idx {
                        SubscriptIndex::Single(e) => self.eval(e),
                        SubscriptIndex::Range(_, _) => {
                            // Range subscripts in scalar context return NaN
                            f64::NAN
                        }
                    })
                    .collect();
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
                match op {
                    UnaryOp::Not => {
                        let l = self.eval(l);
                        (!is_truthy(l)) as i8 as f64
                    }
                    UnaryOp::Transpose => {
                        // Transpose should only be handled through TempArrayElement
                        // in properly compiled A2A assignments
                        panic!(
                            "Bare transpose in interpreter - should be handled via TempArrayElement"
                        )
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
                    BuiltinFn::Sign(a) => {
                        let v = self.eval(a);
                        if v > 0.0 {
                            1.0
                        } else if v < 0.0 {
                            -1.0
                        } else {
                            0.0
                        }
                    }
                    BuiltinFn::Sin(a) => self.eval(a).sin(),
                    BuiltinFn::Tan(a) => self.eval(a).tan(),
                    BuiltinFn::Arccos(a) => self.eval(a).acos(),
                    BuiltinFn::Arcsin(a) => self.eval(a).asin(),
                    BuiltinFn::Arctan(a) => self.eval(a).atan(),
                    BuiltinFn::Exp(a) => self.eval(a).exp(),
                    BuiltinFn::Inf => f64::INFINITY,
                    BuiltinFn::Pi => std::f64::consts::PI,
                    BuiltinFn::Int(a) => self.eval(a).floor(),
                    BuiltinFn::IsModuleInput(ident, _) => self.module.inputs.contains(&Ident::<
                        Canonical,
                    >::from_raw(
                        ident
                    )) as i8 as f64,
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
                        // Check if this is array min or scalar min
                        if b.is_none() {
                            // Single argument - must be an array
                            self.reduce_array(
                                a,
                                f64::INFINITY,
                                |acc, val| {
                                    if val < acc { val } else { acc }
                                },
                            )
                        } else {
                            // Two scalar arguments
                            let a = self.eval(a);
                            let b = self.eval(b.as_ref().unwrap());
                            if a < b { a } else { b }
                        }
                    }
                    BuiltinFn::Mean(args) => {
                        // Check if this is a single array argument or multiple scalar arguments
                        if args.len() == 1 {
                            // Single array argument
                            self.array_mean(&args[0])
                        } else {
                            // Multiple scalar arguments - original behavior
                            let count = args.len() as f64;
                            let sum: f64 = args.iter().map(|arg| self.eval(arg)).sum();
                            sum / count
                        }
                    }
                    BuiltinFn::Max(a, b) => {
                        // Check if this is array max or scalar max
                        if b.is_none() {
                            // Single argument - must be an array
                            self.reduce_array(a, f64::NEG_INFINITY, |acc, val| {
                                if val > acc { val } else { acc }
                            })
                        } else {
                            // Two scalar arguments
                            let a = self.eval(a);
                            let b = self.eval(b.as_ref().unwrap());
                            if a > b { a } else { b }
                        }
                    }
                    BuiltinFn::Lookup(table_expr, index, _) => {
                        let Some((canonical_id, element_offset)) =
                            self.extract_table_info(table_expr.as_ref())
                        else {
                            return f64::NAN;
                        };
                        let Some(tables) = self.module.tables.get(&canonical_id) else {
                            eprintln!("bad lookup for {canonical_id}");
                            return f64::NAN;
                        };
                        if element_offset >= tables.len() {
                            eprintln!(
                                "element_offset {element_offset} out of range for {canonical_id}"
                            );
                            return f64::NAN;
                        }
                        let table = &tables[element_offset].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);
                        if index.is_nan() {
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
                        // Binary search for the first point with x >= index
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
                    BuiltinFn::LookupForward(table_expr, index, _) => {
                        let Some((canonical_id, element_offset)) =
                            self.extract_table_info(table_expr.as_ref())
                        else {
                            return f64::NAN;
                        };
                        let Some(tables) = self.module.tables.get(&canonical_id) else {
                            return f64::NAN;
                        };
                        if element_offset >= tables.len() {
                            return f64::NAN;
                        }
                        let table = &tables[element_offset].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);
                        if index.is_nan() {
                            return f64::NAN;
                        }

                        // At or below first point - return first y
                        if index <= table[0].0 {
                            return table[0].1;
                        }

                        let size = table.len();
                        // At or above last point - return last y
                        if index >= table[size - 1].0 {
                            return table[size - 1].1;
                        }

                        // Binary search for the first point with x >= index (lower bound)
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
                        table[low].1
                    }
                    BuiltinFn::LookupBackward(table_expr, index, _) => {
                        let Some((canonical_id, element_offset)) =
                            self.extract_table_info(table_expr.as_ref())
                        else {
                            return f64::NAN;
                        };
                        let Some(tables) = self.module.tables.get(&canonical_id) else {
                            return f64::NAN;
                        };
                        if element_offset >= tables.len() {
                            return f64::NAN;
                        }
                        let table = &tables[element_offset].data;
                        if table.is_empty() {
                            return f64::NAN;
                        }

                        let index = self.eval(index);
                        if index.is_nan() {
                            return f64::NAN;
                        }

                        // At or below first point - return first y
                        if index <= table[0].0 {
                            return table[0].1;
                        }

                        let size = table.len();
                        // At or above last point - return last y
                        if index >= table[size - 1].0 {
                            return table[size - 1].1;
                        }

                        // Binary search for the first point with x > index (upper bound)
                        // This finds the insertion point after all elements <= index,
                        // which correctly handles duplicate x-values by returning the last one.
                        let mut low = 0;
                        let mut high = size;
                        while low < high {
                            let mid = low + (high - low) / 2;
                            if table[mid].0 <= index {
                                low = mid + 1;
                            } else {
                                high = mid;
                            }
                        }
                        // low now points to the first element > index
                        // We want the element just before it (the last element <= index)
                        table[low - 1].1
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
                    BuiltinFn::Sum(arg) => {
                        // Sum array elements
                        self.reduce_array(arg, 0.0, |acc, val| acc + val)
                    }
                    BuiltinFn::Stddev(arg) => self.array_stddev(arg),
                    BuiltinFn::Size(arg) => self.get_array_size(arg) as f64,
                    BuiltinFn::Rank(_, _) => {
                        unreachable!();
                    }
                }
            }
            Expr::TempArray(id, view, _) => {
                // TempArray should only be used in array contexts (like builtins)
                // For scalar evaluation in A2A contexts, TempArrayElement should be used instead
                let id = *id as usize;
                let start = self.sim.temp_offsets[id];
                let temp_data = (*self.sim.temps).borrow();

                let size = view.dims.iter().product::<usize>();
                debug_assert!(
                    view.offset == 0 && view.sparse.is_empty() && view.is_contiguous(),
                    "TempArray view should be contiguous and rebased"
                );

                // If it's a single-element array, return that element
                if size == 1 {
                    return temp_data[start];
                }

                // For multi-element arrays, TempArray cannot be evaluated as scalar
                // The compiler should have converted this to TempArrayElement for A2A contexts
                panic!(
                    "TempArray {id} cannot be evaluated as scalar - use TempArrayElement for A2A"
                );
            }
            Expr::TempArrayElement(id, _view, element_idx, _) => {
                // TempArrayElement specifies which element to access
                let id = *id as usize;
                let start = self.sim.temp_offsets[id];
                let temp_data = (*self.sim.temps).borrow();

                // The temp array has already been computed and stored
                // element_idx is the flat index into the view
                // Just return that element directly
                temp_data[start + element_idx]
            }
            Expr::AssignTemp(id, rhs, view) => {
                // Evaluate the array expression element by element and store in temporary
                let id = *id as usize;
                if id >= self.sim.temp_offsets.len() - 1 {
                    panic!("Invalid temporary ID: {id}");
                }

                let start = self.sim.temp_offsets[id];
                let total_elements = view.dims.iter().product::<usize>();

                // Helper function to evaluate an expression at a specific array index
                // dims and dim_names are the OUTPUT dimensions; the source view may have
                // fewer dimensions which requires broadcasting (dimension matching by name).
                fn eval_at_index(
                    evaluator: &mut ModuleEvaluator,
                    expr: &Expr,
                    flat_idx: usize,
                    dims: &[usize],
                    dim_names: &[String],
                ) -> f64 {
                    // First, decompose flat_idx into per-dimension coordinates
                    let mut coords: Vec<usize> = vec![0; dims.len()];
                    let mut remainder = flat_idx;
                    for (dim_idx, &dim_size) in dims.iter().enumerate().rev() {
                        coords[dim_idx] = remainder % dim_size;
                        remainder /= dim_size;
                    }

                    match expr {
                        Expr::Const(n, _) => *n,
                        Expr::StaticSubscript(off, view, _) => {
                            // Calculate position in the source array with broadcasting support
                            // Build sparse lookup for this view
                            let sparse_map: std::collections::HashMap<usize, &[usize]> = view
                                .sparse
                                .iter()
                                .map(|s| (s.dim_index, s.parent_offsets.as_slice()))
                                .collect();

                            let mut src_idx = view.offset;

                            // Check if we can use name-based matching
                            // We need both source and output to have valid dimension names
                            let has_valid_names = view.dim_names.iter().all(|n| !n.is_empty())
                                && dim_names.iter().all(|n| !n.is_empty());

                            if has_valid_names && view.dims.len() <= dims.len() {
                                // Broadcasting path: use name-based matching
                                for (src_dim_idx, src_dim_name) in view.dim_names.iter().enumerate()
                                {
                                    let output_dim_idx =
                                        dim_names.iter().position(|name| name == src_dim_name);

                                    debug_assert!(
                                        output_dim_idx.is_some(),
                                        "Source dimension '{}' not found in output dimensions {:?}. \
                                         This indicates a compiler bug.",
                                        src_dim_name,
                                        dim_names
                                    );

                                    let coord = output_dim_idx.map(|idx| coords[idx]).unwrap_or(0);

                                    let parent_coord =
                                        if let Some(offsets) = sparse_map.get(&src_dim_idx) {
                                            offsets[coord]
                                        } else {
                                            coord
                                        };
                                    src_idx += parent_coord * view.strides[src_dim_idx] as usize;
                                }
                            } else {
                                // Positional matching: use same dimension order
                                for (dim_idx, &coord) in coords.iter().enumerate() {
                                    if dim_idx < view.strides.len() {
                                        let parent_coord =
                                            if let Some(offsets) = sparse_map.get(&dim_idx) {
                                                offsets[coord]
                                            } else {
                                                coord
                                            };
                                        src_idx += parent_coord * view.strides[dim_idx] as usize;
                                    }
                                }
                            }
                            evaluator.curr[evaluator.off + *off + src_idx]
                        }
                        Expr::TempArray(id, view, _) => {
                            // Access element from temporary array with broadcasting support
                            let id = *id as usize;
                            let start = evaluator.sim.temp_offsets[id];

                            // Build sparse lookup for this view
                            let sparse_map: std::collections::HashMap<usize, &[usize]> = view
                                .sparse
                                .iter()
                                .map(|s| (s.dim_index, s.parent_offsets.as_slice()))
                                .collect();

                            let mut src_idx = view.offset;

                            // Check if we can use name-based matching
                            let has_valid_names = view.dim_names.iter().all(|n| !n.is_empty())
                                && dim_names.iter().all(|n| !n.is_empty());

                            if has_valid_names && view.dims.len() <= dims.len() {
                                // Broadcasting path: use name-based matching
                                for (src_dim_idx, src_dim_name) in view.dim_names.iter().enumerate()
                                {
                                    let output_dim_idx =
                                        dim_names.iter().position(|name| name == src_dim_name);

                                    debug_assert!(
                                        output_dim_idx.is_some(),
                                        "Source dimension '{}' not found in output dimensions {:?}. \
                                         This indicates a compiler bug.",
                                        src_dim_name,
                                        dim_names
                                    );

                                    let coord = output_dim_idx.map(|idx| coords[idx]).unwrap_or(0);

                                    let parent_coord =
                                        if let Some(offsets) = sparse_map.get(&src_dim_idx) {
                                            offsets[coord]
                                        } else {
                                            coord
                                        };
                                    src_idx += parent_coord * view.strides[src_dim_idx] as usize;
                                }
                            } else {
                                // Positional matching: use same dimension order
                                for (dim_idx, &coord) in coords.iter().enumerate() {
                                    if dim_idx < view.strides.len() {
                                        let parent_coord =
                                            if let Some(offsets) = sparse_map.get(&dim_idx) {
                                                offsets[coord]
                                            } else {
                                                coord
                                            };
                                        src_idx += parent_coord * view.strides[dim_idx] as usize;
                                    }
                                }
                            }

                            let temp_data = (*evaluator.sim.temps).borrow();
                            temp_data[start + src_idx]
                        }
                        Expr::Op2(op, l, r, _) => {
                            let l_val = eval_at_index(evaluator, l, flat_idx, dims, dim_names);
                            let r_val = eval_at_index(evaluator, r, flat_idx, dims, dim_names);
                            match op {
                                BinaryOp::Add => l_val + r_val,
                                BinaryOp::Sub => l_val - r_val,
                                BinaryOp::Mul => l_val * r_val,
                                BinaryOp::Div => l_val / r_val,
                                BinaryOp::Exp => l_val.powf(r_val),
                                BinaryOp::Mod => l_val % r_val,
                                BinaryOp::Lt => (l_val < r_val) as i8 as f64,
                                BinaryOp::Lte => (l_val <= r_val) as i8 as f64,
                                BinaryOp::Gt => (l_val > r_val) as i8 as f64,
                                BinaryOp::Gte => (l_val >= r_val) as i8 as f64,
                                BinaryOp::Eq => approx_eq!(f64, l_val, r_val) as i8 as f64,
                                BinaryOp::Neq => (!approx_eq!(f64, l_val, r_val)) as i8 as f64,
                                BinaryOp::And => {
                                    (is_truthy(l_val) && is_truthy(r_val)) as i8 as f64
                                }
                                BinaryOp::Or => (is_truthy(l_val) || is_truthy(r_val)) as i8 as f64,
                            }
                        }
                        Expr::Op1(op, e, _) => {
                            match op {
                                UnaryOp::Not => {
                                    let val =
                                        eval_at_index(evaluator, e, flat_idx, dims, dim_names);
                                    (!is_truthy(val)) as i8 as f64
                                }
                                UnaryOp::Transpose => {
                                    // For transpose in AssignTemp, we need to map the index
                                    // flat_idx is in the transposed space (dims), we need to map to original space
                                    let orig_idx = transpose_flat_index(flat_idx, dims);

                                    // Get original dimensions by reversing transposed dimensions
                                    let mut orig_dims = dims.to_vec();
                                    orig_dims.reverse();
                                    let mut orig_names = dim_names.to_vec();
                                    orig_names.reverse();

                                    eval_at_index(evaluator, e, orig_idx, &orig_dims, &orig_names)
                                }
                            }
                        }
                        Expr::If(cond, t, f, _) => {
                            // Evaluate condition (may be scalar or array)
                            let cond_val =
                                eval_at_index(evaluator, cond, flat_idx, dims, dim_names);
                            if is_truthy(cond_val) {
                                eval_at_index(evaluator, t, flat_idx, dims, dim_names)
                            } else {
                                eval_at_index(evaluator, f, flat_idx, dims, dim_names)
                            }
                        }
                        // Scalar expressions: same value for every array index
                        // Delegate to main eval method
                        Expr::Var(_, _)
                        | Expr::Dt(_)
                        | Expr::ModuleInput(_, _)
                        | Expr::Subscript(_, _, _, _)
                        | Expr::EvalModule(_, _, _, _)
                        | Expr::App(_, _) => {
                            // These are scalar expressions (or reduce to scalar)
                            // They don't depend on the array index, so evaluate once
                            evaluator.eval(expr)
                        }
                        // Assignment expressions shouldn't appear inside AssignTemp RHS
                        Expr::AssignCurr(_, _)
                        | Expr::AssignNext(_, _)
                        | Expr::AssignTemp(_, _, _)
                        | Expr::TempArrayElement(_, _, _, _) => {
                            panic!("Unexpected assignment expression in AssignTemp RHS: {expr:?}")
                        }
                    }
                }

                let mut temp_data = (*self.sim.temps).borrow_mut();

                // Evaluate element by element
                for i in 0..total_elements {
                    temp_data[start + i] =
                        eval_at_index(self, rhs.as_ref(), i, &view.dims, &view.dim_names);
                }

                // AssignTemp doesn't produce a scalar value
                0.0
            }
        }
    }
}

#[derive(Debug)]
pub struct Simulation {
    pub(crate) modules: HashMap<ModuleKey, Module>,
    specs: Specs,
    root: ModuleKey,
    offsets: HashMap<Ident<Canonical>, usize>,
    temps: std::rc::Rc<RefCell<Vec<f64>>>, // Flat storage for all temporary arrays
    temp_offsets: Vec<usize>,              // Offset of each temporary in the temps vector
}

impl Simulation {
    pub fn new(project: &Project, main_model_name: &str) -> crate::Result<Self> {
        let main_model_ident = canonicalize(main_model_name);
        if !project.models.contains_key(&main_model_ident) {
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

        let module_names: Vec<&Ident<Canonical>> = {
            let mut module_names: Vec<_> = modules.keys().collect();
            module_names.sort_unstable();

            let mut sorted_names = vec![&main_model_ident];
            sorted_names.extend(
                module_names
                    .into_iter()
                    .filter(|n| n.as_str() != main_model_name),
            );
            sorted_names
        };

        // Root module key uses empty input set (main model has no module inputs)
        let root_input_set: ModuleInputSet = BTreeSet::new();
        let root_key: ModuleKey = (main_model_ident.clone(), root_input_set.clone());

        let mut compiled_modules: HashMap<ModuleKey, Module> = HashMap::new();
        for name in module_names {
            let distinct_inputs = &modules[name];
            for inputs in distinct_inputs.iter() {
                let model = Arc::clone(&project.models[name]);
                let is_root = name.as_str() == main_model_ident.as_str();
                let module = Module::new(project, model, inputs, is_root)?;
                // Create module key from model name and input set
                let module_key: ModuleKey = (name.clone(), inputs.clone());
                compiled_modules.insert(module_key, module);
            }
        }

        let sim_specs_dm = project
            .datamodel
            .get_model(main_model_name)
            .and_then(|model| model.sim_specs.clone())
            .unwrap_or_else(|| project.datamodel.sim_specs.clone());

        let specs = Specs::from(&sim_specs_dm);

        let offsets = calc_flattened_offsets(project, main_model_name);
        let offsets: HashMap<Ident<Canonical>, usize> =
            offsets.into_iter().map(|(k, (off, _))| (k, off)).collect();

        // Calculate temporary storage requirements
        let mut max_temps = 0;
        let mut max_temp_sizes = Vec::new();
        for module in compiled_modules.values() {
            if module.n_temps > max_temps {
                max_temps = module.n_temps;
                max_temp_sizes = module.temp_sizes.clone();
            }
        }

        // Allocate temporary storage
        let mut temp_offsets = Vec::with_capacity(max_temps + 1);
        let mut total_temp_size = 0;
        for size in &max_temp_sizes {
            temp_offsets.push(total_temp_size);
            total_temp_size += size;
        }
        temp_offsets.push(total_temp_size); // Final offset for easy range calculation

        let temps = std::rc::Rc::new(RefCell::new(vec![0.0; total_temp_size]));

        Ok(Simulation {
            modules: compiled_modules,
            specs,
            root: root_key,
            offsets,
            temps,
            temp_offsets,
        })
    }

    pub fn compile(&self) -> crate::Result<CompiledSimulation> {
        let modules: crate::Result<HashMap<ModuleKey, CompiledModule>> = self
            .modules
            .iter()
            .map(|(key, module)| module.compile().map(|module| (key.clone(), module)))
            .collect();

        Ok(CompiledSimulation {
            modules: modules?,
            specs: self.specs.clone(),
            root: self.root.clone(),
            offsets: self.offsets.clone(),
        })
    }

    pub fn runlist_order(&self) -> Vec<Ident<Canonical>> {
        calc_flattened_order(self, &self.root)
    }

    pub fn debug_print_runlists(&self, _model_name: &str) {
        let mut module_keys: Vec<_> = self.modules.keys().collect();
        module_keys.sort_unstable();
        for module_key in module_keys {
            eprintln!("\n\nMODULE: {:?}", module_key);
            let module = &self.modules[module_key];
            let model_ident = &module_key.0;
            let offsets = &module.offsets[model_ident];
            let mut idents: Vec<_> = offsets.keys().collect();
            idents.sort_unstable();

            eprintln!("offsets");
            for ident in idents {
                let (off, size) = offsets[ident];
                eprintln!("\t{ident}: {off}, {size}");
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

    fn n_slots(&self, module_key: &ModuleKey) -> usize {
        self.modules[module_key].n_slots
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

/// calc_flattened_offsets generates a mapping from name to offset
/// for all individual variables and subscripts in a model, including
/// in submodels.  For example a variable named "offset" in a module
/// instantiated with name "sector" will produce the key "sector.offset".
pub fn calc_flattened_offsets(
    project: &Project,
    model_name: &str,
) -> HashMap<Ident<Canonical>, (usize, usize)> {
    let is_root = model_name == "main";

    let mut offsets: HashMap<Ident<Canonical>, (usize, usize)> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert(canonicalize("time"), (0, 1));
        offsets.insert(canonicalize("dt"), (1, 1));
        offsets.insert(canonicalize("initial_time"), (2, 1));
        offsets.insert(canonicalize("final_time"), (3, 1));
        i += IMPLICIT_VAR_COUNT;
    }

    let model = Arc::clone(&project.models[&canonicalize(model_name)]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    for ident in var_names.iter() {
        let size = if let Variable::Module { model_name, .. } =
            &model.variables[&canonicalize(ident)]
        {
            let sub_offsets = calc_flattened_offsets(project, model_name.as_str());
            let mut sub_var_names: Vec<&Ident<Canonical>> = sub_offsets.keys().collect();
            sub_var_names.sort_unstable();
            for sub_name in sub_var_names {
                let (sub_off, sub_size) = sub_offsets[sub_name];
                let ident_canonical = canonicalize(ident);
                let sub_canonical = canonicalize(sub_name.as_str());
                offsets.insert(
                    Ident::<Canonical>::from_unchecked(format!(
                        "{}.{}",
                        ident_canonical.to_source_repr(),
                        sub_canonical.to_source_repr()
                    )),
                    (i + sub_off, sub_size),
                );
            }
            let sub_size: usize = sub_offsets.iter().map(|(_, (_, size))| size).sum();
            sub_size
        } else if let Some(Ast::ApplyToAll(dims, _)) = &model.variables[&canonicalize(ident)].ast()
        {
            for (j, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let subscript = subscripts.join(",");
                let ident_canonical = canonicalize(ident);
                let subscripted_ident = Ident::<Canonical>::from_unchecked(format!(
                    "{}[{}]",
                    ident_canonical.to_source_repr(),
                    subscript
                ));
                offsets.insert(subscripted_ident, (i + j, 1));
            }
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _)) = &model.variables[&canonicalize(ident)].ast() {
            for (j, subscripts) in SubscriptIterator::new(dims).enumerate() {
                let subscript = subscripts.join(",");
                let ident_canonical = canonicalize(ident);
                let subscripted_ident = Ident::<Canonical>::from_unchecked(format!(
                    "{}[{}]",
                    ident_canonical.to_source_repr(),
                    subscript
                ));
                offsets.insert(subscripted_ident, (i + j, 1));
            }
            dims.iter().map(|dim| dim.len()).product()
        } else {
            let ident_canonical = canonicalize(ident);
            offsets.insert(
                Ident::<Canonical>::from_unchecked(ident_canonical.to_source_repr()),
                (i, 1),
            );
            1
        };
        i += size;
    }

    offsets
}

/// Find a module by model name (ignoring input_set).
/// Returns the first matching module key, if any.
fn find_module_by_model_name<'a>(
    sim: &'a Simulation,
    model_name: &Ident<Canonical>,
) -> Option<&'a ModuleKey> {
    sim.modules.keys().find(|(name, _)| name == model_name)
}

fn calc_flattened_order(sim: &Simulation, module_key: &ModuleKey) -> Vec<Ident<Canonical>> {
    let (model_name, _) = module_key;
    let is_root = model_name.as_str() == "main";

    let module = &sim.modules[module_key];

    let mut offsets: Vec<Ident<Canonical>> = Vec::with_capacity(module.runlist_order.len() + 1);

    if is_root {
        offsets.push(canonicalize("time"));
    }

    for ident in module.runlist_order.iter() {
        // FIXME: this isn't quite right (assumes no regular var has same name as module)
        if let Some(sub_module_key) = find_module_by_model_name(sim, ident) {
            let sub_var_names = calc_flattened_order(sim, sub_module_key);
            for sub_name in sub_var_names.iter() {
                offsets.push(Ident::<Canonical>::from_unchecked(format!(
                    "{}.{}",
                    ident.to_source_repr(),
                    sub_name.to_source_repr()
                )));
            }
        } else {
            offsets.push(Ident::<Canonical>::from_unchecked(ident.to_source_repr()));
        }
    }

    offsets
}

#[cfg(test)]
mod transpose_tests {
    use super::transpose_flat_index;

    #[test]
    fn test_transpose_1d_array() {
        // 1D arrays should be unchanged by transpose
        assert_eq!(transpose_flat_index(0, &[5]), 0);
        assert_eq!(transpose_flat_index(2, &[5]), 2);
        assert_eq!(transpose_flat_index(4, &[5]), 4);
    }

    #[test]
    fn test_transpose_2d_array() {
        // 2x3 matrix transposed to 3x2
        // Original: [[0,1,2], [3,4,5]]
        // Transposed: [[0,3], [1,4], [2,5]]
        let transposed_dims = &[3, 2];

        // Element at transposed[0,0] = original[0,0] = 0
        assert_eq!(transpose_flat_index(0, transposed_dims), 0);

        // Element at transposed[0,1] = original[1,0] = 3
        assert_eq!(transpose_flat_index(1, transposed_dims), 3);

        // Element at transposed[1,0] = original[0,1] = 1
        assert_eq!(transpose_flat_index(2, transposed_dims), 1);

        // Element at transposed[1,1] = original[1,1] = 4
        assert_eq!(transpose_flat_index(3, transposed_dims), 4);

        // Element at transposed[2,0] = original[0,2] = 2
        assert_eq!(transpose_flat_index(4, transposed_dims), 2);

        // Element at transposed[2,1] = original[1,2] = 5
        assert_eq!(transpose_flat_index(5, transposed_dims), 5);
    }

    #[test]
    fn test_transpose_3d_array() {
        // 2x3x4 array transposed to 4x3x2
        let transposed_dims = &[4, 3, 2];

        // Test a few key mappings
        // transposed[0,0,0] = original[0,0,0] = 0
        assert_eq!(transpose_flat_index(0, transposed_dims), 0);

        // transposed[0,0,1] = original[1,0,0] = 12 (stride=12 in original)
        assert_eq!(transpose_flat_index(1, transposed_dims), 12);

        // transposed[1,0,0] = original[0,0,1] = 1
        assert_eq!(transpose_flat_index(6, transposed_dims), 1);

        // transposed[3,2,1] = original[1,2,3] = 12+8+3 = 23
        assert_eq!(transpose_flat_index(23, transposed_dims), 23);
    }

    #[test]
    fn test_transpose_square_matrix() {
        // 3x3 matrix - transpose should swap row/col indices
        let transposed_dims = &[3, 3];

        // Diagonal elements unchanged
        assert_eq!(transpose_flat_index(0, transposed_dims), 0); // [0,0]
        assert_eq!(transpose_flat_index(4, transposed_dims), 4); // [1,1]
        assert_eq!(transpose_flat_index(8, transposed_dims), 8); // [2,2]

        // Off-diagonal elements swap
        assert_eq!(transpose_flat_index(1, transposed_dims), 3); // [0,1] -> [1,0]
        assert_eq!(transpose_flat_index(3, transposed_dims), 1); // [1,0] -> [0,1]
        assert_eq!(transpose_flat_index(2, transposed_dims), 6); // [0,2] -> [2,0]
        assert_eq!(transpose_flat_index(6, transposed_dims), 2); // [2,0] -> [0,2]
    }

    #[test]
    fn test_transpose_empty_array() {
        // Empty dimensions should return input unchanged
        assert_eq!(transpose_flat_index(0, &[]), 0);
        assert_eq!(transpose_flat_index(5, &[]), 5);
    }

    #[test]
    fn test_transpose_index_mapping_correctness() {
        // Test that transpose is its own inverse for 2D arrays
        let dims_2d = &[3, 4];
        let transposed_dims_2d = &[4, 3];

        for i in 0..12 {
            let transposed_idx = transpose_flat_index(i, dims_2d);
            let back_to_original = transpose_flat_index(transposed_idx, transposed_dims_2d);
            assert_eq!(
                back_to_original, i,
                "Transpose should be its own inverse: {i} -> {transposed_idx} -> {back_to_original}"
            );
        }
    }
}

#[test]
fn test_arrays() {
    use crate::ast::Loc;
    use crate::compiler::{Context, Var};
    use std::collections::BTreeSet;

    let project = {
        use crate::datamodel::*;
        Project {
            name: "arrays".to_owned(),
            source: None,
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 12.0,
                dt: Dt::Dt(0.25),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_owned()),
            },
            dimensions: vec![Dimension::Named(
                "letters".to_owned(),
                vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            )],
            units: vec![],
            models: vec![Model {
                name: "main".to_owned(),
                sim_specs: None,
                variables: vec![
                    Variable::Aux(Aux {
                        ident: "constants".to_owned(),
                        equation: Equation::Arrayed(
                            vec!["letters".to_owned()],
                            vec![
                                ("a".to_owned(), "9".to_owned(), None, None),
                                ("b".to_owned(), "7".to_owned(), None, None),
                                ("c".to_owned(), "5".to_owned(), None, None),
                            ],
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked".to_owned(),
                        equation: Equation::Scalar("aux[INT(TIME MOD 5) + 1]".to_owned(), None),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                    Variable::Aux(Aux {
                        ident: "aux".to_owned(),
                        equation: Equation::ApplyToAll(
                            vec!["letters".to_owned()],
                            "constants".to_owned(),
                            None,
                        ),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                    Variable::Aux(Aux {
                        ident: "picked2".to_owned(),
                        equation: Equation::Scalar("aux[b]".to_owned(), None),
                        documentation: "".to_owned(),
                        units: None,
                        gf: None,
                        can_be_module_input: false,
                        visibility: Visibility::Private,
                        ai_state: None,
                        uid: None,
                    }),
                ],
                views: vec![],
                loop_metadata: vec![],
            }],
            ai_information: None,
        }
    };

    let parsed_project = Arc::new(Project::from(project));

    {
        let actual = calc_flattened_offsets(&parsed_project, "main");
        let expected: HashMap<_, _> = vec![
            (canonicalize("time"), (0, 1)),
            (canonicalize("dt"), (1, 1)),
            (canonicalize("initial_time"), (2, 1)),
            (canonicalize("final_time"), (3, 1)),
            (canonicalize("aux[a]"), (4, 1)),
            (canonicalize("aux[b]"), (5, 1)),
            (canonicalize("aux[c]"), (6, 1)),
            (canonicalize("constants[a]"), (7, 1)),
            (canonicalize("constants[b]"), (8, 1)),
            (canonicalize("constants[c]"), (9, 1)),
            (canonicalize("picked"), (10, 1)),
            (canonicalize("picked2"), (11, 1)),
        ]
        .into_iter()
        .collect();
        assert_eq!(actual, expected);
    }

    let main_ident = canonicalize("main");
    let metadata = compiler::build_metadata(&parsed_project, &main_ident, true);
    let main_metadata = &metadata[&main_ident];
    assert_eq!(main_metadata[&canonicalize("aux")].offset, 4);
    assert_eq!(main_metadata[&canonicalize("aux")].size, 3);
    assert_eq!(main_metadata[&canonicalize("constants")].offset, 7);
    assert_eq!(main_metadata[&canonicalize("constants")].size, 3);
    assert_eq!(main_metadata[&canonicalize("picked")].offset, 10);
    assert_eq!(main_metadata[&canonicalize("picked")].size, 1);
    assert_eq!(main_metadata[&canonicalize("picked2")].offset, 11);
    assert_eq!(main_metadata[&canonicalize("picked2")].size, 1);

    let module_models = compiler::calc_module_model_map(&parsed_project, &main_ident);

    let arrayed_constants_var =
        &parsed_project.models[&main_ident].variables[&canonicalize("constants")];
    let parsed_var = Var::new(
        &Context {
            dimensions: parsed_project
                .datamodel
                .dimensions
                .iter()
                .map(|d| crate::dimensions::Dimension::from(d.clone()))
                .collect(),
            dimensions_ctx: &parsed_project.dimensions_ctx,
            model_name: &main_ident,
            ident: arrayed_constants_var.canonical_ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
            preserve_wildcards_for_iteration: false,
        },
        arrayed_constants_var,
    );

    assert!(parsed_var.is_ok());

    let expected = Var {
        ident: canonicalize(arrayed_constants_var.ident()),
        ast: vec![
            Expr::AssignCurr(7, Box::new(Expr::Const(9.0, Loc::default()))),
            Expr::AssignCurr(8, Box::new(Expr::Const(7.0, Loc::default()))),
            Expr::AssignCurr(9, Box::new(Expr::Const(5.0, Loc::default()))),
        ],
    };
    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let arrayed_aux_var = &parsed_project.models[&main_ident].variables[&canonicalize("aux")];
    let parsed_var = Var::new(
        &Context {
            dimensions: parsed_project
                .datamodel
                .dimensions
                .iter()
                .map(|d| crate::dimensions::Dimension::from(d.clone()))
                .collect(),
            dimensions_ctx: &parsed_project.dimensions_ctx,
            model_name: &main_ident,
            ident: arrayed_aux_var.canonical_ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
            preserve_wildcards_for_iteration: false,
        },
        arrayed_aux_var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: canonicalize(arrayed_aux_var.ident()),
        ast: vec![
            Expr::AssignCurr(4, Box::new(Expr::Var(7, Loc::default()))),
            Expr::AssignCurr(5, Box::new(Expr::Var(8, Loc::default()))),
            Expr::AssignCurr(6, Box::new(Expr::Var(9, Loc::default()))),
        ],
    };
    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let var = &parsed_project.models[&main_ident].variables[&canonicalize("picked2")];
    let parsed_var = Var::new(
        &Context {
            dimensions: parsed_project
                .datamodel
                .dimensions
                .iter()
                .map(|d| crate::dimensions::Dimension::from(d.clone()))
                .collect(),
            dimensions_ctx: &parsed_project.dimensions_ctx,
            model_name: &main_ident,
            ident: var.canonical_ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
            preserve_wildcards_for_iteration: false,
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: canonicalize(var.ident()),
        ast: vec![Expr::AssignCurr(
            11,
            Box::new(Expr::StaticSubscript(
                4,
                ArrayView {
                    dims: vec![],
                    strides: vec![],
                    offset: 1,
                    sparse: Vec::new(),
                    dim_names: vec![],
                },
                Loc::default(),
            )),
        )],
    };

    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let var = &parsed_project.models[&main_ident].variables[&canonicalize("picked")];
    let parsed_var = Var::new(
        &Context {
            dimensions: parsed_project
                .datamodel
                .dimensions
                .iter()
                .map(|d| crate::dimensions::Dimension::from(d.clone()))
                .collect(),
            dimensions_ctx: &parsed_project.dimensions_ctx,
            model_name: &main_ident,
            ident: var.canonical_ident(),
            active_dimension: None,
            active_subscript: None,
            metadata: &metadata,
            module_models: &module_models,
            is_initial: false,
            inputs: &BTreeSet::new(),
            preserve_wildcards_for_iteration: false,
        },
        var,
    );

    assert!(parsed_var.is_ok());
    let expected = Var {
        ident: canonicalize(var.ident()),
        ast: vec![Expr::AssignCurr(
            10,
            Box::new(Expr::Subscript(
                4,
                vec![SubscriptIndex::Single(Expr::Op2(
                    BinaryOp::Add,
                    Box::new(Expr::App(
                        BuiltinFn::Int(Box::new(Expr::Op2(
                            BinaryOp::Mod,
                            Box::new(Expr::App(BuiltinFn::Time, Loc::default())),
                            Box::new(Expr::Const(5.0, Loc::default())),
                            Loc::default(),
                        ))),
                        Loc::default(),
                    )),
                    Box::new(Expr::Const(1.0, Loc::default())),
                    Loc::default(),
                ))],
                vec![3],
                Loc::default(),
            )),
        )],
    };

    let mut parsed_var = parsed_var.unwrap();
    for expr in parsed_var.ast.iter_mut() {
        *expr = expr.clone().strip_loc();
    }
    assert_eq!(expected, parsed_var);

    let sim = Simulation::new(&parsed_project, "main");
    assert!(sim.is_ok());
}

#[test]
fn nan_is_approx_eq() {
    assert!(approx_eq!(f64, f64::NAN, f64::NAN));
}

#[test]
fn simulation_uses_model_sim_specs_when_present() {
    use crate::datamodel::{self, Aux, Equation, SimSpecs as DmSimSpecs, Variable, Visibility};

    let project_specs = DmSimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: datamodel::Dt::Dt(1.0),
        save_step: Some(datamodel::Dt::Dt(1.0)),
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Days".to_string()),
    };

    let model_specs = DmSimSpecs {
        start: 2.0,
        stop: 20.0,
        dt: datamodel::Dt::Dt(0.5),
        save_step: Some(datamodel::Dt::Dt(2.5)),
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Hours".to_string()),
    };

    let model = datamodel::Model {
        name: "main".to_string(),
        sim_specs: Some(model_specs.clone()),
        variables: vec![Variable::Aux(Aux {
            ident: "const".to_string(),
            equation: Equation::Scalar("1".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: Visibility::Private,
            ai_state: None,
            uid: None,
        })],
        views: vec![],
        loop_metadata: vec![],
    };

    let datamodel_project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: project_specs,
        dimensions: vec![],
        units: vec![],
        models: vec![model],
        source: None,
        ai_information: None,
    };

    let compiled = crate::project::Project::from(datamodel_project);
    let sim = Simulation::new(&compiled, "main").expect("simulation should build");

    assert_eq!(sim.specs.start, 2.0);
    assert_eq!(sim.specs.stop, 20.0);
    assert!(approx_eq!(f64, sim.specs.dt, 0.5));
    assert!(approx_eq!(f64, sim.specs.save_step, 2.5));
}

#[test]
fn simulation_defaults_to_project_sim_specs_without_model_override() {
    use crate::datamodel::{self, Aux, Equation, SimSpecs as DmSimSpecs, Variable, Visibility};

    let project_specs = DmSimSpecs {
        start: 1.0,
        stop: 11.0,
        dt: datamodel::Dt::Dt(0.25),
        save_step: Some(datamodel::Dt::Dt(0.5)),
        sim_method: datamodel::SimMethod::Euler,
        time_units: Some("Weeks".to_string()),
    };

    let model = datamodel::Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: vec![Variable::Aux(Aux {
            ident: "const".to_string(),
            equation: Equation::Scalar("1".to_string(), None),
            documentation: String::new(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: Visibility::Private,
            ai_state: None,
            uid: None,
        })],
        views: vec![],
        loop_metadata: vec![],
    };

    let datamodel_project = datamodel::Project {
        name: "test".to_string(),
        sim_specs: project_specs,
        dimensions: vec![],
        units: vec![],
        models: vec![model],
        source: None,
        ai_information: None,
    };

    let compiled = crate::project::Project::from(datamodel_project);
    let sim = Simulation::new(&compiled, "main").expect("simulation should build");

    assert_eq!(sim.specs.start, 1.0);
    assert_eq!(sim.specs.stop, 11.0);
    assert!(approx_eq!(f64, sim.specs.dt, 0.25));
    assert!(approx_eq!(f64, sim.specs.save_step, 0.5));
}
