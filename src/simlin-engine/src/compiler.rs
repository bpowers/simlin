// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use crate::ast::{self, Ast, BinaryOp, IndexExpr2, Loc};
use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeBuilder, ByteCodeContext, CompiledModule, GraphicalFunctionId,
    ModuleDeclaration, ModuleId, ModuleInputOffset, Op2, Opcode, VariableOffset,
};
use crate::common::{
    Canonical, CanonicalElementName, ErrorCode, ErrorKind, Ident, Result, canonicalize,
};
use crate::dimensions::Dimension;
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::Variable;
use crate::vm::{
    DT_OFF, FINAL_TIME_OFF, IMPLICIT_VAR_COUNT, INITIAL_TIME_OFF, SubscriptIterator, TIME_OFF,
};
use crate::{Error, sim_err};

// Type alias to reduce complexity
type VariableOffsetMap = HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, (usize, usize)>>;

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub data: Vec<(f64, f64)>,
}

impl Table {
    fn new(ident: &str, t: &crate::variable::Table) -> Result<Self> {
        if t.x.len() != t.y.len() {
            return sim_err!(BadTable, ident.to_string());
        }

        let data: Vec<(f64, f64)> = t.x.iter().copied().zip(t.y.iter().copied()).collect();

        Ok(Self { data })
    }
}

pub(crate) type BuiltinFn = crate::builtins::BuiltinFn<Expr>;

/// Represents a view into array data with support for striding and slicing
#[derive(PartialEq, Clone, Debug)]
pub struct ArrayView {
    /// Dimension sizes after slicing/viewing
    pub dims: Vec<usize>,
    /// Stride for each dimension (elements to skip to move by 1 in that dimension)
    pub strides: Vec<isize>,
    /// Starting offset in the underlying data
    pub offset: usize,
}

impl ArrayView {
    /// Create a contiguous array view (row-major order)
    #[allow(dead_code)]
    pub fn contiguous(dims: Vec<usize>) -> Self {
        let mut strides = vec![1isize; dims.len()];
        // Build strides from right to left for row-major order
        for i in (0..dims.len().saturating_sub(1)).rev() {
            strides[i] = strides[i + 1] * dims[i + 1] as isize;
        }
        ArrayView {
            dims,
            strides,
            offset: 0,
        }
    }

    /// Total number of elements in the view
    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.dims.iter().product()
    }

    /// Check if this view represents contiguous data in row-major order
    #[allow(dead_code)]
    pub fn is_contiguous(&self) -> bool {
        if self.offset != 0 {
            return false;
        }

        let mut expected_stride = 1isize;
        for i in (0..self.dims.len()).rev() {
            if self.strides[i] != expected_stride {
                return false;
            }
            expected_stride *= self.dims[i] as isize;
        }
        true
    }

    /// Apply a range subscript to create a new view
    #[allow(dead_code)]
    pub fn apply_range_subscript(
        &self,
        dim_index: usize,
        start: usize,
        end: usize,
    ) -> Result<ArrayView> {
        if dim_index >= self.dims.len() {
            return sim_err!(Generic, "dimension index out of bounds".to_string());
        }
        if start >= end || end > self.dims[dim_index] {
            return sim_err!(Generic, "invalid range bounds".to_string());
        }

        let mut new_dims = self.dims.clone();
        new_dims[dim_index] = end - start;

        let new_strides = self.strides.clone();
        let new_offset = self.offset + (start * self.strides[dim_index] as usize);

        Ok(ArrayView {
            dims: new_dims,
            strides: new_strides,
            offset: new_offset,
        })
    }
}

#[derive(PartialEq, Clone, Debug)]
#[allow(dead_code)]
pub enum Expr {
    Const(f64, Loc),
    Var(usize, Loc),                              // offset
    Subscript(usize, Vec<Expr>, Vec<usize>, Loc), // offset, index expression, bounds (for dynamic/old-style)
    StaticSubscript(usize, ArrayView, Loc),       // offset, precomputed view, location
    TempArray(u32, ArrayView, Loc),               // temp id, view into temp array, location
    TempArrayElement(u32, ArrayView, usize, Loc), // temp id, view, element index, location
    Dt(Loc),
    App(BuiltinFn, Loc),
    EvalModule(Ident<Canonical>, Ident<Canonical>, Vec<Expr>),
    ModuleInput(usize, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, Loc),
    Op1(UnaryOp, Box<Expr>, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, Loc),
    AssignCurr(usize, Box<Expr>),
    AssignNext(usize, Box<Expr>),
    AssignTemp(u32, Box<Expr>, ArrayView), // temp id, expression to evaluate, view info
}

impl Expr {
    fn get_loc(&self) -> Loc {
        match self {
            Expr::Const(_, loc) => *loc,
            Expr::Var(_, loc) => *loc,
            Expr::Subscript(_, _, _, loc) => *loc,
            Expr::StaticSubscript(_, _, loc) => *loc,
            Expr::TempArray(_, _, loc) => *loc,
            Expr::TempArrayElement(_, _, _, loc) => *loc,
            Expr::Dt(loc) => *loc,
            Expr::App(_, loc) => *loc,
            Expr::EvalModule(_, _, _) => Loc::default(),
            Expr::ModuleInput(_, loc) => *loc,
            Expr::Op2(_, _, _, loc) => *loc,
            Expr::Op1(_, _, loc) => *loc,
            Expr::If(_, _, _, loc) => *loc,
            Expr::AssignCurr(_, _) => Loc::default(),
            Expr::AssignNext(_, _) => Loc::default(),
            Expr::AssignTemp(_, _, _) => Loc::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        let loc = Loc::default();
        match self {
            Expr::Const(c, _loc) => Expr::Const(c, loc),
            Expr::Var(v, _loc) => Expr::Var(v, loc),
            Expr::Subscript(off, subscripts, bounds, _) => {
                let subscripts = subscripts
                    .into_iter()
                    .map(|expr| expr.strip_loc())
                    .collect();
                Expr::Subscript(off, subscripts, bounds, loc)
            }
            Expr::StaticSubscript(off, view, _) => Expr::StaticSubscript(off, view, loc),
            Expr::TempArray(id, view, _) => Expr::TempArray(id, view, loc),
            Expr::TempArrayElement(id, view, idx, _) => Expr::TempArrayElement(id, view, idx, loc),
            Expr::Dt(_) => Expr::Dt(loc),
            Expr::App(builtin, _loc) => {
                let builtin = match builtin {
                    // nothing to strip from these simple ones
                    BuiltinFn::Inf
                    | BuiltinFn::Pi
                    | BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => builtin,
                    BuiltinFn::IsModuleInput(id, _loc) => BuiltinFn::IsModuleInput(id, loc),
                    BuiltinFn::Lookup(id, a, _loc) => {
                        BuiltinFn::Lookup(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::Abs(a) => BuiltinFn::Abs(Box::new(a.strip_loc())),
                    BuiltinFn::Arccos(a) => BuiltinFn::Arccos(Box::new(a.strip_loc())),
                    BuiltinFn::Arcsin(a) => BuiltinFn::Arcsin(Box::new(a.strip_loc())),
                    BuiltinFn::Arctan(a) => BuiltinFn::Arctan(Box::new(a.strip_loc())),
                    BuiltinFn::Cos(a) => BuiltinFn::Cos(Box::new(a.strip_loc())),
                    BuiltinFn::Exp(a) => BuiltinFn::Exp(Box::new(a.strip_loc())),
                    BuiltinFn::Int(a) => BuiltinFn::Int(Box::new(a.strip_loc())),
                    BuiltinFn::Ln(a) => BuiltinFn::Ln(Box::new(a.strip_loc())),
                    BuiltinFn::Log10(a) => BuiltinFn::Log10(Box::new(a.strip_loc())),
                    BuiltinFn::Mean(args) => {
                        BuiltinFn::Mean(args.into_iter().map(|arg| arg.strip_loc()).collect())
                    }
                    BuiltinFn::Sign(a) => BuiltinFn::Sign(Box::new(a.strip_loc())),
                    BuiltinFn::Sin(a) => BuiltinFn::Sin(Box::new(a.strip_loc())),
                    BuiltinFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(a.strip_loc())),
                    BuiltinFn::Tan(a) => BuiltinFn::Tan(Box::new(a.strip_loc())),
                    BuiltinFn::Max(a, b) => {
                        BuiltinFn::Max(Box::new(a.strip_loc()), b.map(|b| Box::new(b.strip_loc())))
                    }
                    BuiltinFn::Min(a, b) => {
                        BuiltinFn::Min(Box::new(a.strip_loc()), b.map(|b| Box::new(b.strip_loc())))
                    }
                    BuiltinFn::Step(a, b) => {
                        BuiltinFn::Step(Box::new(a.strip_loc()), Box::new(b.strip_loc()))
                    }
                    BuiltinFn::Pulse(a, b, c) => BuiltinFn::Pulse(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::Ramp(a, b, c) => BuiltinFn::Ramp(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::SafeDiv(a, b, c) => BuiltinFn::SafeDiv(
                        Box::new(a.strip_loc()),
                        Box::new(b.strip_loc()),
                        c.map(|expr| Box::new(expr.strip_loc())),
                    ),
                    BuiltinFn::Rank(a, rest) => BuiltinFn::Rank(
                        Box::new(a.strip_loc()),
                        rest.map(|(b, c)| {
                            (Box::new(b.strip_loc()), c.map(|c| Box::new(c.strip_loc())))
                        }),
                    ),
                    BuiltinFn::Size(a) => BuiltinFn::Size(Box::new(a.strip_loc())),
                    BuiltinFn::Stddev(a) => BuiltinFn::Stddev(Box::new(a.strip_loc())),
                    BuiltinFn::Sum(a) => BuiltinFn::Sum(Box::new(a.strip_loc())),
                };
                Expr::App(builtin, loc)
            }
            Expr::EvalModule(id1, id2, args) => {
                let args = args.into_iter().map(|expr| expr.strip_loc()).collect();
                Expr::EvalModule(id1, id2, args)
            }
            Expr::ModuleInput(mi, _loc) => Expr::ModuleInput(mi, loc),
            Expr::Op2(op, l, r, _loc) => {
                Expr::Op2(op, Box::new(l.strip_loc()), Box::new(r.strip_loc()), loc)
            }
            Expr::Op1(op, r, _loc) => Expr::Op1(op, Box::new(r.strip_loc()), loc),
            Expr::If(cond, t, f, _loc) => Expr::If(
                Box::new(cond.strip_loc()),
                Box::new(t.strip_loc()),
                Box::new(f.strip_loc()),
                loc,
            ),
            Expr::AssignCurr(off, rhs) => Expr::AssignCurr(off, Box::new(rhs.strip_loc())),
            Expr::AssignNext(off, rhs) => Expr::AssignNext(off, Box::new(rhs.strip_loc())),
            Expr::AssignTemp(id, rhs, view) => {
                Expr::AssignTemp(id, Box::new(rhs.strip_loc()), view)
            }
        }
    }
}

#[allow(dead_code)]
fn decompose_array_temps(expr: Expr, next_temp_id: usize) -> Result<(Expr, Vec<Expr>, usize)> {
    Ok((expr, vec![], next_temp_id))
}

#[derive(Clone, Debug)]
pub(crate) struct VariableMetadata {
    pub(crate) offset: usize,
    pub(crate) size: usize,
    // FIXME: this should be able to be borrowed
    pub(crate) var: Variable,
}

#[derive(Clone, Debug)]
pub(crate) struct Context<'a> {
    pub(crate) dimensions: Vec<Dimension>,
    pub(crate) model_name: &'a Ident<Canonical>,
    #[allow(dead_code)]
    pub(crate) ident: &'a Ident<Canonical>,
    pub(crate) active_dimension: Option<Vec<Dimension>>,
    pub(crate) active_subscript: Option<Vec<CanonicalElementName>>,
    pub(crate) metadata: &'a HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>>,
    pub(crate) module_models:
        &'a HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>>,
    pub(crate) is_initial: bool,
    pub(crate) inputs: &'a BTreeSet<Ident<Canonical>>,
}

impl Context<'_> {
    fn get_offset(&self, ident: &Ident<Canonical>) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, false)
    }

    /// get_base_offset ignores arrays and should only be used from Var::new and Expr::Subscript
    fn get_base_offset(&self, ident: &Ident<Canonical>) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, true)
    }

    fn get_metadata(&self, ident: &Ident<Canonical>) -> Result<&VariableMetadata> {
        self.get_submodel_metadata(self.model_name, ident)
    }

    fn get_implicit_subscripts(&self, dims: &[Dimension], ident: &str) -> Result<Vec<&str>> {
        if self.active_dimension.is_none() {
            return sim_err!(ArrayReferenceNeedsExplicitSubscripts, ident.to_owned());
        }
        let active_dims = self.active_dimension.as_ref().unwrap();
        let active_subscripts = self.active_subscript.as_ref().unwrap();
        assert_eq!(active_dims.len(), active_subscripts.len());

        // Check if dimensions can be reordered to match
        if dims.len() == active_dims.len() {
            // Get dimension names (all canonical at this point)
            let source_dim_names: Vec<String> = dims.iter().map(|d| d.name().to_string()).collect();
            let target_dim_names: Vec<String> =
                active_dims.iter().map(|d| d.name().to_string()).collect();

            // Check if dimensions can be reordered
            // Note: we're asking "how to reorder target to match source"
            if let Some(_reordering) =
                find_dimension_reordering(&target_dim_names, &source_dim_names)
            {
                // Build subscripts in the order needed by the source dims
                // reordering[i] tells us which target dimension to use for source position i
                let mut subscripts: Vec<&str> = Vec::with_capacity(dims.len());
                for source_dim in dims {
                    // Find which active dimension matches this source dimension
                    for (j, active_dim) in active_dims.iter().enumerate() {
                        if active_dim.name() == source_dim.name() {
                            subscripts.push(active_subscripts[j].as_str());
                            break;
                        }
                    }
                }
                return Ok(subscripts);
            }
        }

        // Fall back to original logic for partial dimension matching
        // if we need more dimensions than are implicit, that's an error
        if dims.len() > active_dims.len() {
            return sim_err!(MismatchedDimensions, ident.to_owned());
        }

        // goal: if this is a valid equation, dims will be a subset of active_dims (order preserving)

        let mut subscripts: Vec<&str> = Vec::with_capacity(dims.len());

        let mut active_off = 0;
        for dim in dims.iter() {
            while active_off < active_dims.len() {
                let off = active_off;
                active_off += 1;
                let candidate = &active_dims[off];
                if candidate.name() == dim.name() {
                    subscripts.push(active_subscripts[off].as_str());
                    break;
                }
            }
        }

        if subscripts.len() != dims.len() {
            return sim_err!(MismatchedDimensions, ident.to_owned());
        }

        Ok(subscripts)
    }

    fn get_implicit_subscript_off(&self, dims: &[Dimension], ident: &str) -> Result<usize> {
        let subscripts = self.get_implicit_subscripts(dims, ident)?;

        let off = dims
            .iter()
            .zip(subscripts)
            .fold(0_usize, |acc, (dim, subscript)| {
                acc * dim.len()
                    + dim
                        .get_offset(&CanonicalElementName::from_raw(subscript))
                        .unwrap()
            });

        Ok(off)
    }

    fn get_dimension_name_subscript(&self, dim_name: &str) -> Option<usize> {
        let active_dims = self.active_dimension.as_ref()?;
        let active_subscripts = self.active_subscript.as_ref().unwrap();

        for (dim, subscript) in active_dims.iter().zip(active_subscripts) {
            if dim.name() == dim_name {
                return dim.get_offset(subscript);
            }
        }

        None
    }

    fn get_submodel_metadata(
        &self,
        model: &Ident<Canonical>,
        ident: &Ident<Canonical>,
    ) -> Result<&VariableMetadata> {
        let metadata = &self.metadata[model];
        if let Some(pos) = ident.as_str().find('·') {
            let submodel_module_name = &ident.as_str()[..pos];
            let submodel_name = &self.module_models[model]
                [&Ident::<Canonical>::from_str_unchecked(submodel_module_name)];
            let submodel_var = &ident.as_str()[pos + '·'.len_utf8()..];
            self.get_submodel_metadata(
                submodel_name,
                &Ident::<Canonical>::from_str_unchecked(submodel_var),
            )
        } else {
            Ok(&metadata[ident])
        }
    }

    fn get_submodel_offset(
        &self,
        model: &Ident<Canonical>,
        ident: &Ident<Canonical>,
        ignore_arrays: bool,
    ) -> Result<usize> {
        let metadata = &self.metadata[model];
        let ident_str = ident.as_str();
        if let Some(pos) = ident_str.find('·') {
            let submodel_module_name = &ident_str[..pos];
            let submodel_name = &self.module_models[model]
                [&Ident::<Canonical>::from_str_unchecked(submodel_module_name)];
            let submodel_var = &ident_str[pos + '·'.len_utf8()..];
            let submodel_off =
                metadata[&Ident::<Canonical>::from_str_unchecked(submodel_module_name)].offset;
            Ok(submodel_off
                + self.get_submodel_offset(
                    submodel_name,
                    &Ident::<Canonical>::from_str_unchecked(submodel_var),
                    ignore_arrays,
                )?)
        } else if !ignore_arrays {
            if !metadata.contains_key(ident) {
                return sim_err!(DoesNotExist);
            }
            if let Some(dims) = metadata[ident].var.get_dimensions() {
                let off = self.get_implicit_subscript_off(dims, ident.as_str())?;
                Ok(metadata[ident].offset + off)
            } else {
                Ok(metadata[ident].offset)
            }
        } else {
            Ok(metadata[ident].offset)
        }
    }

    fn lower(&self, expr: &ast::Expr2) -> Result<Expr> {
        let expr = match expr {
            ast::Expr2::Const(_, n, loc) => Expr::Const(*n, *loc),
            ast::Expr2::Var(id, _, loc) => {
                // Check if this identifier is a dimension name
                let is_dimension = self
                    .dimensions
                    .iter()
                    .any(|dim| id.as_str() == canonicalize(dim.name()).as_str());

                if is_dimension {
                    // This is a dimension name
                    if let Some(active_dims) = &self.active_dimension {
                        if let Some(active_subscripts) = &self.active_subscript {
                            // We're in an array context - find the matching dimension
                            for (dim, subscript) in active_dims.iter().zip(active_subscripts.iter())
                            {
                                if id.as_str() == canonicalize(dim.name()).as_str() {
                                    // Convert to the subscript index (0-based)
                                    let index = match dim {
                                        Dimension::Indexed(_, _) => {
                                            // Subscript is already a 1-based index as a string
                                            subscript.as_str().parse::<f64>().unwrap()
                                        }
                                        Dimension::Named(_, named_dim) => {
                                            let off = named_dim
                                                .elements
                                                .iter()
                                                .position(|elem| {
                                                    elem.as_str() == subscript.as_str()
                                                })
                                                .unwrap();

                                            (off + 1) as f64
                                        }
                                    };
                                    return Ok(Expr::Const(index, *loc));
                                }
                            }
                        }
                    } else {
                        // We're in a scalar context but trying to use a dimension name
                        return Err(Error {
                            kind: ErrorKind::Model,
                            code: ErrorCode::DimensionInScalarContext,
                            details: Some(format!(
                                "Dimension '{id}' cannot be used in a scalar equation"
                            )),
                        });
                    }
                }

                // Not a dimension, check if it's a module input
                if let Some((off, _)) = self
                    .inputs
                    .iter()
                    .enumerate()
                    .find(|(_, input)| id.as_str() == input.as_str())
                {
                    Expr::ModuleInput(off, *loc)
                } else {
                    match self.get_offset(id) {
                        Ok(off) => Expr::Var(off, *loc),
                        Err(err) => {
                            // If get_offset fails because it's an array without implicit subscripts,
                            // try to create a full array view
                            if matches!(err.code, ErrorCode::ArrayReferenceNeedsExplicitSubscripts)
                                && let Ok(metadata) = self.get_metadata(id)
                                && let Some(source_dims) = metadata.var.get_dimensions()
                            {
                                // This is an array variable - check if we need dimension reordering
                                let off = self.get_base_offset(id)?;

                                // Check if we're in an A2A context and need to reorder dimensions
                                if let Some(target_dims) = &self.active_dimension {
                                    // Get dimension names
                                    let source_dim_names: Vec<String> =
                                        source_dims.iter().map(|d| d.name().to_string()).collect();
                                    let target_dim_names: Vec<String> =
                                        target_dims.iter().map(|d| d.name().to_string()).collect();

                                    // Check if dimensions can be reordered
                                    if let Some(reordering) = find_dimension_reordering(
                                        &source_dim_names,
                                        &target_dim_names,
                                    ) {
                                        // Check if reordering is needed (not identity)
                                        let needs_reordering =
                                            reordering.iter().enumerate().any(|(i, &idx)| i != idx);

                                        if needs_reordering {
                                            // Create a transposed view
                                            // We need to apply the dimension reordering
                                            let orig_dims: Vec<usize> =
                                                source_dims.iter().map(|d| d.len()).collect();

                                            // Reorder the dimensions
                                            let reordered_dims: Vec<usize> = target_dims
                                                .iter()
                                                .map(|target_dim| {
                                                    // Find the matching source dimension
                                                    source_dims
                                                        .iter()
                                                        .find(|source_dim| {
                                                            canonicalize(source_dim.name())
                                                                == canonicalize(target_dim.name())
                                                        })
                                                        .unwrap()
                                                        .len()
                                                })
                                                .collect();

                                            // Create strides for the reordered view
                                            let mut strides = vec![1isize; orig_dims.len()];
                                            for i in (0..orig_dims.len() - 1).rev() {
                                                strides[i] =
                                                    strides[i + 1] * orig_dims[i + 1] as isize;
                                            }

                                            // Reorder the strides according to the dimension reordering
                                            let reordered_strides: Vec<isize> = reordering
                                                .iter()
                                                .map(|&idx| strides[idx])
                                                .collect();

                                            let view = ArrayView {
                                                dims: reordered_dims,
                                                strides: reordered_strides,
                                                offset: 0,
                                            };

                                            return Ok(Expr::StaticSubscript(off, view, *loc));
                                        }
                                    }
                                }

                                // No reordering needed or not in A2A context
                                let orig_dims: Vec<usize> =
                                    source_dims.iter().map(|d| d.len()).collect();
                                let view = ArrayView::contiguous(orig_dims);
                                return Ok(Expr::StaticSubscript(off, view, *loc));
                            }
                            return Err(err);
                        }
                    }
                }
            }
            ast::Expr2::App(builtin, _, loc) => {
                use crate::builtins::BuiltinFn as BFn;
                let builtin: BuiltinFn = match builtin {
                    BFn::Lookup(id, expr, loc) => {
                        BuiltinFn::Lookup(id.clone(), Box::new(self.lower(expr)?), *loc)
                    }
                    BFn::Abs(a) => BuiltinFn::Abs(Box::new(self.lower(a)?)),
                    BFn::Arccos(a) => BuiltinFn::Arccos(Box::new(self.lower(a)?)),
                    BFn::Arcsin(a) => BuiltinFn::Arcsin(Box::new(self.lower(a)?)),
                    BFn::Arctan(a) => BuiltinFn::Arctan(Box::new(self.lower(a)?)),
                    BFn::Cos(a) => BuiltinFn::Cos(Box::new(self.lower(a)?)),
                    BFn::Exp(a) => BuiltinFn::Exp(Box::new(self.lower(a)?)),
                    BFn::Inf => BuiltinFn::Inf,
                    BFn::Int(a) => BuiltinFn::Int(Box::new(self.lower(a)?)),
                    BFn::IsModuleInput(id, loc) => BuiltinFn::IsModuleInput(id.clone(), *loc),
                    BFn::Ln(a) => BuiltinFn::Ln(Box::new(self.lower(a)?)),
                    BFn::Log10(a) => BuiltinFn::Log10(Box::new(self.lower(a)?)),
                    BFn::Max(a, b) => {
                        let b = if let Some(b) = b {
                            Some(Box::new(self.lower(b)?))
                        } else {
                            None
                        };
                        BuiltinFn::Max(Box::new(self.lower(a)?), b)
                    }
                    BFn::Mean(args) => {
                        let args = args
                            .iter()
                            .map(|arg| self.lower(arg))
                            .collect::<Result<Vec<Expr>>>();
                        BuiltinFn::Mean(args?)
                    }
                    BFn::Min(a, b) => {
                        let b = if let Some(b) = b {
                            Some(Box::new(self.lower(b)?))
                        } else {
                            None
                        };
                        BuiltinFn::Min(Box::new(self.lower(a)?), b)
                    }
                    BFn::Pi => BuiltinFn::Pi,
                    BFn::Pulse(a, b, c) => {
                        let c = match c {
                            Some(c) => Some(Box::new(self.lower(c)?)),
                            None => None,
                        };
                        BuiltinFn::Pulse(Box::new(self.lower(a)?), Box::new(self.lower(b)?), c)
                    }
                    BFn::Ramp(a, b, c) => {
                        let c = match c {
                            Some(c) => Some(Box::new(self.lower(c)?)),
                            None => None,
                        };
                        BuiltinFn::Ramp(Box::new(self.lower(a)?), Box::new(self.lower(b)?), c)
                    }
                    BFn::SafeDiv(a, b, c) => {
                        let c = match c {
                            Some(c) => Some(Box::new(self.lower(c)?)),
                            None => None,
                        };
                        BuiltinFn::SafeDiv(Box::new(self.lower(a)?), Box::new(self.lower(b)?), c)
                    }
                    BFn::Sign(a) => BuiltinFn::Sign(Box::new(self.lower(a)?)),
                    BFn::Sin(a) => BuiltinFn::Sin(Box::new(self.lower(a)?)),
                    BFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(self.lower(a)?)),
                    BFn::Step(a, b) => {
                        BuiltinFn::Step(Box::new(self.lower(a)?), Box::new(self.lower(b)?))
                    }
                    BFn::Tan(a) => BuiltinFn::Tan(Box::new(self.lower(a)?)),
                    BFn::Time => BuiltinFn::Time,
                    BFn::TimeStep => BuiltinFn::TimeStep,
                    BFn::StartTime => BuiltinFn::StartTime,
                    BFn::FinalTime => BuiltinFn::FinalTime,
                    BFn::Rank(_, _) => {
                        return sim_err!(TodoArrayBuiltin, self.ident.to_string());
                    }
                    BFn::Size(a) => {
                        let arg = self.lower(a)?;
                        BuiltinFn::Size(Box::new(arg))
                    }
                    BFn::Stddev(a) => {
                        let arg = self.lower(a)?;
                        BuiltinFn::Stddev(Box::new(arg))
                    }
                    BFn::Sum(a) => {
                        let arg = self.lower(a)?;
                        BuiltinFn::Sum(Box::new(arg))
                    }
                };
                Expr::App(builtin, *loc)
            }
            ast::Expr2::Subscript(id, args, _, loc) => {
                let off = self.get_base_offset(id)?;
                let metadata = self.get_metadata(id)?;
                let dims = metadata.var.get_dimensions().unwrap();
                if args.len() != dims.len() {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }

                // First, check if this is a static subscript that we can optimize
                let mut is_static = true;

                // Build a list of operations to apply to the view
                enum IndexOp {
                    Range(usize, usize), // start, end (0-based, end exclusive)
                    Single(usize),       // single index (0-based)
                    Wildcard,            // keep dimension
                    DimPosition(usize),  // dimension position (0-based)
                }

                let mut operations = Vec::new();

                for (i, arg) in args.iter().enumerate() {
                    match arg {
                        IndexExpr2::Range(start_expr, end_expr, _) => {
                            // Helper to resolve a dimension element to an index
                            let resolve_to_index = |expr: &ast::Expr2| -> Option<usize> {
                                match expr {
                                    ast::Expr2::Const(_, val, _) => {
                                        // Numeric constant - convert from 1-based to 0-based
                                        Some((*val as isize - 1).max(0) as usize)
                                    }
                                    ast::Expr2::Var(ident, _, _) => {
                                        // Could be a named dimension element
                                        if i < dims.len() {
                                            if let Dimension::Named(_, named_dim) = &dims[i] {
                                                named_dim.elements.iter().position(|elem| {
                                                    elem.as_str() == ident.as_str()
                                                })
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        }
                                    }
                                    _ => None,
                                }
                            };

                            if let (Some(start_idx), Some(end_idx)) =
                                (resolve_to_index(start_expr), resolve_to_index(end_expr))
                            {
                                // end_idx is inclusive in the source, but we need exclusive for the range
                                operations.push(IndexOp::Range(start_idx, end_idx + 1));
                            } else {
                                is_static = false;
                                break;
                            }
                        }
                        IndexExpr2::Wildcard(_) => {
                            operations.push(IndexOp::Wildcard);
                        }
                        IndexExpr2::DimPosition(pos, _) => {
                            // @1 is position 0, @2 is position 1, etc.
                            let dim_idx = (*pos as usize).saturating_sub(1);
                            operations.push(IndexOp::DimPosition(dim_idx));
                        }
                        IndexExpr2::Expr(expr) => {
                            match expr {
                                ast::Expr2::Const(_, val, _) => {
                                    let idx = (*val as isize - 1).max(0) as usize;
                                    operations.push(IndexOp::Single(idx));
                                }
                                ast::Expr2::Var(ident, _, _) => {
                                    // Check if it's a named dimension element
                                    if i < dims.len() {
                                        if let Dimension::Named(_, named_dim) = &dims[i] {
                                            if let Some(idx) = named_dim
                                                .elements
                                                .iter()
                                                .position(|elem| elem.as_str() == ident.as_str())
                                            {
                                                operations.push(IndexOp::Single(idx));
                                            } else {
                                                is_static = false;
                                                break;
                                            }
                                        } else {
                                            is_static = false;
                                            break;
                                        }
                                    } else {
                                        is_static = false;
                                        break;
                                    }
                                }
                                _ => {
                                    is_static = false;
                                    break;
                                }
                            }
                        }
                        _ => {
                            is_static = false;
                            break;
                        }
                    }
                }

                if is_static {
                    // Build a unified view for any combination of static operations
                    let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();

                    // Calculate original strides (row-major)
                    let mut orig_strides = vec![1isize; orig_dims.len()];
                    for i in (0..orig_dims.len().saturating_sub(1)).rev() {
                        orig_strides[i] = orig_strides[i + 1] * orig_dims[i + 1] as isize;
                    }

                    // First pass: determine dimension mapping and validate
                    // dim_mapping[i] = Some(j) means output dim i comes from input dim j
                    // dim_mapping[i] = None means output dim i is removed (single index)
                    let mut dim_mapping: Vec<Option<usize>> = Vec::new();
                    let mut single_indices: Vec<usize> = Vec::new();
                    let mut offset_adjustment = 0usize;

                    for (i, op) in operations.iter().enumerate() {
                        match op {
                            IndexOp::Single(idx) => {
                                // Validate bounds
                                if *idx >= orig_dims[i] {
                                    return sim_err!(
                                        Generic,
                                        format!("Index out of bounds for dimension {}", i)
                                    );
                                }
                                single_indices.push(*idx);
                                offset_adjustment += idx * orig_strides[i] as usize;
                            }
                            IndexOp::Range(start, end) => {
                                // Validate bounds
                                if *end > orig_dims[i] || *start >= *end {
                                    return sim_err!(
                                        Generic,
                                        format!("Invalid range bounds for dimension {}", i)
                                    );
                                }
                                dim_mapping.push(Some(i));
                                single_indices.push(*start); // Track start offset
                                offset_adjustment += start * orig_strides[i] as usize;
                            }
                            IndexOp::Wildcard => {
                                dim_mapping.push(Some(i));
                                single_indices.push(0); // No offset for wildcard
                            }
                            IndexOp::DimPosition(pos) => {
                                if *pos >= orig_dims.len() {
                                    return sim_err!(
                                        Generic,
                                        format!("Dimension position @{} out of bounds", pos + 1)
                                    );
                                }
                                dim_mapping.push(Some(*pos));
                                single_indices.push(0); // Will be resolved at runtime in A2A context
                            }
                        }
                    }

                    // Build the resulting view
                    let mut new_dims = Vec::new();
                    let mut new_strides = Vec::new();

                    for (i, op) in operations.iter().enumerate() {
                        match op {
                            IndexOp::Single(_) => {
                                // Dimension is removed, don't add to output
                            }
                            IndexOp::Range(start, end) => {
                                new_dims.push(end - start);
                                new_strides.push(orig_strides[i]);
                            }
                            IndexOp::Wildcard => {
                                new_dims.push(orig_dims[i]);
                                new_strides.push(orig_strides[i]);
                            }
                            IndexOp::DimPosition(pos) => {
                                // Use the dimension size and stride from the referenced position
                                new_dims.push(orig_dims[*pos]);
                                new_strides.push(orig_strides[*pos]);
                            }
                        }
                    }

                    let view = ArrayView {
                        dims: new_dims,
                        strides: new_strides,
                        offset: offset_adjustment,
                    };

                    // Check if we're in an array iteration context
                    // If we have dimension positions, we need special handling in A2A context
                    if let Some(active_subscripts) = &self.active_subscript
                        && let Some(_active_dims) = &self.active_dimension
                    {
                        // Check if we have any dimension positions
                        let has_dim_positions = operations
                            .iter()
                            .any(|op| matches!(op, IndexOp::DimPosition(_)));

                        if has_dim_positions {
                            // For dimension positions in A2A context, we need to fall back to dynamic evaluation
                            // because @n refers to the nth dimension of the target variable's current iteration
                            is_static = false;
                        } else {
                            // Calculate the linear index in the result array based on the view
                            let mut result_index = 0;

                            // For each dimension in the view, find its value from active subscripts
                            // The active subscripts correspond to the OUTPUT dimensions, not the input
                            for (view_idx, stride) in view.strides.iter().enumerate() {
                                if view_idx < active_subscripts.len() {
                                    // Get the dimension for this view index
                                    let dim_idx = dim_mapping[view_idx].unwrap_or(view_idx);
                                    if dim_idx < dims.len() {
                                        let dim = &dims[dim_idx];
                                        let subscript = &active_subscripts[view_idx];

                                        // Get the offset for this subscript in the dimension
                                        if let Some(offset) = dim.get_offset(subscript) {
                                            result_index += offset * (*stride as usize);
                                        } else if let Ok(idx) = subscript.as_str().parse::<usize>()
                                        {
                                            // For indexed dimensions with numeric subscripts
                                            let idx_0based = idx - 1;
                                            result_index += idx_0based * (*stride as usize);
                                        }
                                    }
                                }
                            }

                            return Ok(Expr::Var(off + view.offset + result_index, *loc));
                        }
                    }

                    if !is_static {
                        // Fall through to dynamic handling
                    } else {
                        // Not in iteration context or no dimension positions - use StaticSubscript for the full view
                        return Ok(Expr::StaticSubscript(off, view, *loc));
                    }
                }

                // Fall back to dynamic subscript handling
                let args: Result<Vec<_>> = args
                    .iter()
                    .enumerate()
                    .map(|(i, arg)| {
                        match arg {
                            IndexExpr2::Wildcard(loc) => {
                                // Wildcard means use the implicit subscript for this dimension
                                if self.active_dimension.is_none() {
                                    return sim_err!(
                                        ArrayReferenceNeedsExplicitSubscripts,
                                        id.as_str().to_string()
                                    );
                                }
                                let active_dims = self.active_dimension.as_ref().unwrap();
                                let active_subscripts = self.active_subscript.as_ref().unwrap();
                                let dim = &dims[i];

                                // Find the matching dimension in the active context
                                for (active_dim, active_subscript) in
                                    active_dims.iter().zip(active_subscripts)
                                {
                                    if active_dim.name() == dim.name() {
                                        // Found the matching dimension, use its subscript
                                        if let Dimension::Named(_, _) = dim {
                                            if let Some(subscript_off) =
                                                dim.get_offset(active_subscript)
                                            {
                                                return Ok(Expr::Const(
                                                    (subscript_off + 1) as f64,
                                                    *loc,
                                                ));
                                            }
                                        } else if let Dimension::Indexed(_name, _size) = dim {
                                            // For indexed dimensions, the subscript is now just a numeric string
                                            // like "1", "2", etc. (1-based)
                                            if let Ok(idx) =
                                                active_subscript.as_str().parse::<usize>()
                                            {
                                                // The index is already 1-based, so we can use it directly
                                                return Ok(Expr::Const(idx as f64, *loc));
                                            }
                                        }
                                    }
                                }

                                // If we didn't find a matching dimension, that's an error
                                sim_err!(MismatchedDimensions, id.as_str().to_string())
                            }
                            IndexExpr2::StarRange(_id, _loc) => {
                                sim_err!(TodoStarRange, id.as_str().to_string())
                            }
                            IndexExpr2::Range(_start_expr, _end_expr, _loc) => {
                                // Dynamic range - not supported yet in old-style subscript
                                sim_err!(TodoRange, id.as_str().to_string())
                            }
                            IndexExpr2::DimPosition(pos, loc) => {
                                // @1 refers to the first dimension, @2 to the second, etc.
                                // In dynamic context, we need the active subscript for that dimension position
                                if self.active_dimension.is_none() {
                                    return sim_err!(
                                        ArrayReferenceNeedsExplicitSubscripts,
                                        id.as_str().to_string()
                                    );
                                }
                                let active_dims = self.active_dimension.as_ref().unwrap();
                                let active_subscripts = self.active_subscript.as_ref().unwrap();

                                // Convert 1-based position to 0-based index
                                let dim_idx = (*pos as usize).saturating_sub(1);

                                // Check if the dimension position is valid
                                if dim_idx >= active_dims.len() {
                                    return sim_err!(
                                        Generic,
                                        format!("Dimension position @{} out of bounds", pos)
                                    );
                                }

                                // Get the subscript for the specified dimension position
                                let subscript = &active_subscripts[dim_idx];

                                // Parse it as a numeric index (1-based)
                                if let Ok(idx) = subscript.as_str().parse::<usize>() {
                                    Ok(Expr::Const(idx as f64, *loc))
                                } else {
                                    // If it's a named subscript, we need to resolve it
                                    // This would require looking up the dimension at that position
                                    // For now, return an error
                                    sim_err!(ArraysNotImplemented, id.as_str().to_string())
                                }
                            }
                            IndexExpr2::Expr(arg) => {
                                let expr = if let ast::Expr2::Var(ident, _, loc) = arg {
                                    let dim = &dims[i];
                                    // we need to check to make sure that any explicit subscript names are
                                    // converted to offsets here and not passed to self.lower

                                    // First check for named dimension subscripts
                                    // Need to do case-insensitive matching since identifiers are canonicalized
                                    let subscript_off =
                                        if let Dimension::Named(_, named_dim) = dim {
                                            let canonicalized_ident = ident.as_str();
                                            named_dim.elements.iter().position(|elem| {
                                                elem.as_str() == canonicalized_ident
                                            })
                                        } else {
                                            None
                                        };

                                    if let Some(offset) = subscript_off {
                                        Expr::Const((offset + 1) as f64, *loc)
                                    } else if let Dimension::Indexed(name, _size) = dim {
                                        // For indexed dimensions, check if ident is of format "DimName.Index"
                                        let expected_prefix = format!("{}.", name.as_str());
                                        if ident.as_str().starts_with(&expected_prefix) {
                                            if let Ok(idx) = ident.as_str()[expected_prefix.len()..]
                                                .parse::<usize>()
                                            {
                                                // Validate the index is within bounds (1-based)
                                                if idx >= 1 && idx <= dim.len() {
                                                    Expr::Const(idx as f64, *loc)
                                                } else {
                                                    return sim_err!(
                                                        BadDimensionName,
                                                        id.as_str().to_string()
                                                    );
                                                }
                                            } else {
                                                self.lower(arg)?
                                            }
                                        } else {
                                            self.lower(arg)?
                                        }
                                    } else if let Some(subscript_off) =
                                        self.get_dimension_name_subscript(ident.as_str())
                                    {
                                        // some modelers do `Variable[SubscriptName]` in their A2A equations
                                        Expr::Const((subscript_off + 1) as f64, *loc)
                                    } else {
                                        self.lower(arg)?
                                    }
                                } else {
                                    self.lower(arg)?
                                };
                                Ok(expr)
                            }
                        }
                    })
                    .collect();
                let bounds = dims.iter().map(|dim| dim.len()).collect();
                Expr::Subscript(off, args?, bounds, *loc)
            }
            ast::Expr2::Op1(op, l, _, loc) => {
                match op {
                    ast::UnaryOp::Transpose => {
                        // Special handling for transpose of bare array variables
                        if let ast::Expr2::Var(id, _, var_loc) = &**l {
                            // Get the variable's metadata to check if it's an array
                            if let Ok(metadata) = self.get_metadata(id)
                                && let Some(dims) = metadata.var.get_dimensions()
                            {
                                if self.active_dimension.is_some() {
                                    // We're in an A2A context - need to handle bare array transpose specially
                                    // We need to reverse the active dimensions before processing the variable
                                    let mut ctx = self.clone();
                                    if let Some(ref active_dims) = ctx.active_dimension {
                                        let mut reversed_dims = active_dims.clone();
                                        reversed_dims.reverse();
                                        ctx.active_dimension = Some(reversed_dims);
                                    }
                                    if let Some(ref active_subs) = ctx.active_subscript {
                                        let mut reversed_subs = active_subs.clone();
                                        reversed_subs.reverse();
                                        ctx.active_subscript = Some(reversed_subs);
                                    }
                                    // Process the variable with reversed dimensions
                                    let inner = ctx.lower(l)?;
                                    // The result already has the correct transposed access pattern
                                    return Ok(inner);
                                } else {
                                    // Not in A2A context - create a wildcard subscript to get the full array
                                    // then apply transpose
                                    let off = self.get_base_offset(id)?;
                                    let orig_dims: Vec<usize> =
                                        dims.iter().map(|d| d.len()).collect();
                                    let orig_strides =
                                        ArrayView::contiguous(orig_dims.clone()).strides;

                                    // Create a view for the full array
                                    let view = ArrayView {
                                        dims: orig_dims.clone(),
                                        strides: orig_strides,
                                        offset: 0,
                                    };

                                    // Now transpose it
                                    let mut transposed_dims = view.dims.clone();
                                    transposed_dims.reverse();
                                    let mut transposed_strides = view.strides.clone();
                                    transposed_strides.reverse();
                                    let transposed_view = ArrayView {
                                        dims: transposed_dims,
                                        strides: transposed_strides,
                                        offset: view.offset,
                                    };

                                    return Ok(Expr::StaticSubscript(
                                        off,
                                        transposed_view,
                                        *var_loc,
                                    ));
                                }
                            }
                        }

                        // Default transpose handling
                        // If we're in an A2A context and the inner expression might contain bare arrays,
                        // we need to handle it specially by creating a temporary
                        if self.active_dimension.is_some() {
                            // In A2A context - the inner expression needs to be processed without A2A
                            // to get the full array, then we transpose and apply the A2A subscript
                            // For now, just wrap in transpose and let expression rewriting handle it
                            let mut ctx = self.clone();
                            ctx.active_dimension = None;
                            ctx.active_subscript = None;
                            let l = ctx.lower(l)?;
                            Expr::Op1(UnaryOp::Transpose, Box::new(l), *loc)
                        } else {
                            let l = self.lower(l)?;
                            // Transpose reverses the dimensions of an array
                            match l {
                                Expr::StaticSubscript(off, view, loc) => {
                                    // Transpose a view by reversing its dimensions and strides
                                    let mut transposed_dims = view.dims.clone();
                                    transposed_dims.reverse();
                                    let mut transposed_strides = view.strides.clone();
                                    transposed_strides.reverse();

                                    let transposed_view = ArrayView {
                                        dims: transposed_dims,
                                        strides: transposed_strides,
                                        offset: view.offset,
                                    };

                                    Expr::StaticSubscript(off, transposed_view, loc)
                                }
                                _ => {
                                    // For other expressions (including bare variables),
                                    // wrap in a transpose operation to be handled at runtime
                                    Expr::Op1(UnaryOp::Transpose, Box::new(l), *loc)
                                }
                            }
                        }
                    }
                    _ => {
                        // Process the inner expression first for other operators
                        let l = self.lower(l)?;
                        match op {
                            ast::UnaryOp::Negative => Expr::Op2(
                                BinaryOp::Sub,
                                Box::new(Expr::Const(0.0, *loc)),
                                Box::new(l),
                                *loc,
                            ),
                            ast::UnaryOp::Positive => l,
                            ast::UnaryOp::Not => Expr::Op1(UnaryOp::Not, Box::new(l), *loc),
                            ast::UnaryOp::Transpose => unreachable!("Transpose handled above"),
                        }
                    }
                }
            }
            ast::Expr2::Op2(op, l, r, array_bounds, loc) => {
                // Check if we need dimension reordering for binary operations
                let mut l_expr = self.lower(l)?;
                let mut r_expr = self.lower(r)?;

                // Only apply dimension reordering if we're NOT in an A2A context
                // In A2A context, the implicit subscripts already handle dimension reordering
                if self.active_dimension.is_none() {
                    // If we have array bounds, check if dimension reordering is needed
                    if let Some(bounds) = array_bounds {
                        // Get dimension names from the array bounds
                        if let Some(_target_dim_names) = bounds.dim_names() {
                            // Try to get dimension names from left and right expressions
                            let l_dim_names = self.get_expr_dimension_names(l);
                            let r_dim_names = self.get_expr_dimension_names(r);

                            // Check if right needs reordering to match left's dimension order
                            if let (Some(l_names), Some(r_names)) = (&l_dim_names, &r_dim_names)
                                && l_names != r_names
                            {
                                // Check if r can be reordered to match l
                                if let Some(reordering) =
                                    find_dimension_reordering(r_names, l_names)
                                {
                                    // Apply reordering to r_expr
                                    r_expr =
                                        self.apply_dimension_reordering(r_expr, reordering, *loc)?;
                                }
                                // Otherwise check if l can be reordered to match r
                                else if let Some(reordering) =
                                    find_dimension_reordering(l_names, r_names)
                                {
                                    // Apply reordering to l_expr
                                    l_expr =
                                        self.apply_dimension_reordering(l_expr, reordering, *loc)?;
                                }
                            }
                        }
                    }
                }

                let op = match op {
                    ast::BinaryOp::Add => BinaryOp::Add,
                    ast::BinaryOp::Sub => BinaryOp::Sub,
                    ast::BinaryOp::Exp => BinaryOp::Exp,
                    ast::BinaryOp::Mul => BinaryOp::Mul,
                    ast::BinaryOp::Div => BinaryOp::Div,
                    ast::BinaryOp::Mod => BinaryOp::Mod,
                    ast::BinaryOp::Gt => BinaryOp::Gt,
                    ast::BinaryOp::Gte => BinaryOp::Gte,
                    ast::BinaryOp::Lt => BinaryOp::Lt,
                    ast::BinaryOp::Lte => BinaryOp::Lte,
                    ast::BinaryOp::Eq => BinaryOp::Eq,
                    ast::BinaryOp::Neq => BinaryOp::Neq,
                    ast::BinaryOp::And => BinaryOp::And,
                    ast::BinaryOp::Or => BinaryOp::Or,
                };

                // For now, just create the Op2 expression
                // The rewriting to use temporaries will happen in a separate pass
                Expr::Op2(op, Box::new(l_expr), Box::new(r_expr), *loc)
            }
            ast::Expr2::If(cond, t, f, _, loc) => {
                let cond = self.lower(cond)?;
                let t = self.lower(t)?;
                let f = self.lower(f)?;
                Expr::If(Box::new(cond), Box::new(t), Box::new(f), *loc)
            }
        };

        Ok(expr)
    }

    fn fold_flows(&self, flows: &[Ident<Canonical>]) -> Option<Expr> {
        if flows.is_empty() {
            return None;
        }

        let mut loads = flows
            .iter()
            .map(|flow| Expr::Var(self.get_offset(flow).unwrap(), Loc::default()));

        let first = loads.next().unwrap();
        Some(loads.fold(first, |acc, flow| {
            Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow), Loc::default())
        }))
    }

    /// Get dimension names from an Expr2 if it's an array variable
    fn get_expr_dimension_names(&self, expr: &ast::Expr2) -> Option<Vec<String>> {
        match expr {
            ast::Expr2::Var(id, _, _) => {
                // Get the variable's dimensions
                let metadata = self.metadata.get(self.model_name)?;
                let var_metadata = metadata.get(id)?;
                let dims = var_metadata.var.get_dimensions()?;
                Some(dims.iter().map(|d| d.name().to_string()).collect())
            }
            ast::Expr2::Subscript(id, _, _, _) => {
                // For subscripted arrays, get the base variable's dimensions
                let metadata = self.metadata.get(self.model_name)?;
                let var_metadata = metadata.get(id)?;
                let dims = var_metadata.var.get_dimensions()?;
                Some(dims.iter().map(|d| d.name().to_string()).collect())
            }
            ast::Expr2::Op1(ast::UnaryOp::Transpose, inner, _, _) => {
                // For transpose, get the inner dimensions and reverse them
                let mut dims = self.get_expr_dimension_names(inner)?;
                dims.reverse();
                Some(dims)
            }
            _ => None,
        }
    }

    /// Apply dimension reordering to an expression
    fn apply_dimension_reordering(
        &self,
        expr: Expr,
        reordering: Vec<usize>,
        loc: Loc,
    ) -> Result<Expr> {
        // The reordering vector contains 0-based indices indicating the new position of each dimension
        // For example, [1, 0] means swap dimensions (transpose for 2D)
        // [1, 2, 0] means the first output dim is the second input dim, etc.

        // Check if this is a simple variable or static subscript that we can reorder directly
        match &expr {
            Expr::Var(off, _) => {
                // This is a bare array variable - create a StaticSubscript with reordered view
                // First, get the variable metadata to get dimensions
                if let Ok(metadata) = self.get_variable_metadata_by_offset(*off)
                    && let Some(dims) = metadata.var.get_dimensions()
                {
                    let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();

                    // Create a contiguous view first
                    let view = ArrayView::contiguous(orig_dims.clone());

                    // Apply the reordering
                    let reordered_dims: Vec<usize> =
                        reordering.iter().map(|&idx| orig_dims[idx]).collect();
                    let reordered_strides: Vec<isize> =
                        reordering.iter().map(|&idx| view.strides[idx]).collect();

                    let reordered_view = ArrayView {
                        dims: reordered_dims,
                        strides: reordered_strides,
                        offset: 0,
                    };

                    return Ok(Expr::StaticSubscript(*off, reordered_view, loc));
                }
            }
            Expr::StaticSubscript(off, view, _) => {
                // Apply reordering to existing view
                let reordered_dims: Vec<usize> =
                    reordering.iter().map(|&idx| view.dims[idx]).collect();
                let reordered_strides: Vec<isize> =
                    reordering.iter().map(|&idx| view.strides[idx]).collect();

                let reordered_view = ArrayView {
                    dims: reordered_dims,
                    strides: reordered_strides,
                    offset: view.offset,
                };

                return Ok(Expr::StaticSubscript(*off, reordered_view, loc));
            }
            _ => {}
        }

        // For other expressions, fall back to transpose for 2D
        if reordering.len() == 2 && reordering == vec![1, 0] {
            // This is a simple transpose
            Ok(Expr::Op1(UnaryOp::Transpose, Box::new(expr), loc))
        } else {
            // For more complex reordering, we'd need to create a view with reordered strides
            // For now, just return the expression unchanged
            // TODO: Implement general dimension reordering
            Ok(expr)
        }
    }

    /// Helper to get variable metadata by offset
    fn get_variable_metadata_by_offset(&self, offset: usize) -> Result<&VariableMetadata> {
        let metadata = self.metadata.get(self.model_name).ok_or_else(|| {
            use crate::common::{Error, ErrorCode, ErrorKind};
            Error {
                kind: ErrorKind::Simulation,
                code: ErrorCode::BadModelName,
                details: Some("Model not found".to_string()),
            }
        })?;

        // Find the variable with the matching offset
        for (_, var_metadata) in metadata.iter() {
            if var_metadata.offset == offset {
                return Ok(var_metadata);
            }
        }

        sim_err!(DoesNotExist, "Variable not found by offset".to_string())
    }

    fn build_stock_update_expr(&self, stock_off: usize, var: &Variable) -> Expr {
        if let Variable::Stock {
            inflows, outflows, ..
        } = var
        {
            // TODO: simplify the expressions we generate
            let inflows = match self.fold_flows(inflows) {
                None => Expr::Const(0.0, Loc::default()),
                Some(flows) => flows,
            };
            let outflows = match self.fold_flows(outflows) {
                None => Expr::Const(0.0, Loc::default()),
                Some(flows) => flows,
            };

            let dt_update = Expr::Op2(
                BinaryOp::Mul,
                Box::new(Expr::Op2(
                    BinaryOp::Sub,
                    Box::new(inflows),
                    Box::new(outflows),
                    Loc::default(),
                )),
                Box::new(Expr::Dt(Loc::default())),
                Loc::default(),
            );

            Expr::Op2(
                BinaryOp::Add,
                Box::new(Expr::Var(stock_off, Loc::default())),
                Box::new(dt_update),
                Loc::default(),
            )
        } else {
            panic!(
                "build_stock_update_expr called with non-stock {}",
                var.ident()
            );
        }
    }
}

#[test]
fn test_lower() {
    use crate::common::{Canonical, Ident};
    let input = {
        use ast::BinaryOp::*;
        use ast::Expr2::*;
        Box::new(If(
            Box::new(Op2(
                And,
                Box::new(Var(canonicalize("true_input"), None, Loc::default())),
                Box::new(Var(canonicalize("false_input"), None, Loc::default())),
                None,
                Loc::default(),
            )),
            Box::new(Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Const("0".to_string(), 0.0, Loc::default())),
            None,
            Loc::default(),
        ))
    };

    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata> = HashMap::new();
    metadata.insert(
        canonicalize("true_input"),
        VariableMetadata {
            offset: 7,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        canonicalize("false_input"),
        VariableMetadata {
            offset: 8,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = canonicalize("main");
    let test_ident = canonicalize("test");
    metadata2.insert(main_ident.clone(), metadata);
    let context = Context {
        dimensions: vec![],
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
    };
    let expected = Expr::If(
        Box::new(Expr::Op2(
            BinaryOp::And,
            Box::new(Expr::Var(7, Loc::default())),
            Box::new(Expr::Var(8, Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr::Const(1.0, Loc::default())),
        Box::new(Expr::Const(0.0, Loc::default())),
        Loc::default(),
    );

    let output = context.lower(&input);
    assert!(output.is_ok());
    assert_eq!(expected, output.unwrap());

    let input = {
        use ast::BinaryOp::*;
        use ast::Expr2::*;
        Box::new(If(
            Box::new(Op2(
                Or,
                Box::new(Var(canonicalize("true_input"), None, Loc::default())),
                Box::new(Var(canonicalize("false_input"), None, Loc::default())),
                None,
                Loc::default(),
            )),
            Box::new(Const("1".to_string(), 1.0, Loc::default())),
            Box::new(Const("0".to_string(), 0.0, Loc::default())),
            None,
            Loc::default(),
        ))
    };

    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata> = HashMap::new();
    metadata.insert(
        canonicalize("true_input"),
        VariableMetadata {
            offset: 7,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        canonicalize("false_input"),
        VariableMetadata {
            offset: 8,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = canonicalize("main");
    let test_ident = canonicalize("test");
    metadata2.insert(main_ident.clone(), metadata);
    let context = Context {
        dimensions: vec![],
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
    };
    let expected = Expr::If(
        Box::new(Expr::Op2(
            BinaryOp::Or,
            Box::new(Expr::Var(7, Loc::default())),
            Box::new(Expr::Var(8, Loc::default())),
            Loc::default(),
        )),
        Box::new(Expr::Const(1.0, Loc::default())),
        Box::new(Expr::Const(0.0, Loc::default())),
        Loc::default(),
    );

    let output = context.lower(&input);
    assert!(output.is_ok());
    assert_eq!(expected, output.unwrap());
}

#[derive(Clone, Debug, PartialEq)]
pub struct Var {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) ast: Vec<Expr>,
}

#[test]
fn test_fold_flows() {
    let inputs = &BTreeSet::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata> = HashMap::new();
    metadata.insert(
        canonicalize("a"),
        VariableMetadata {
            offset: 1,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        canonicalize("b"),
        VariableMetadata {
            offset: 2,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        canonicalize("c"),
        VariableMetadata {
            offset: 3,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    metadata.insert(
        canonicalize("d"),
        VariableMetadata {
            offset: 4,
            size: 1,
            var: Variable::Var {
                ident: canonicalize(""),
                ast: None,
                init_ast: None,
                eqn: None,
                units: None,
                table: None,
                non_negative: false,
                is_flow: false,
                is_table_only: false,
                errors: vec![],
                unit_errors: vec![],
            },
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = canonicalize("main");
    let test_ident = canonicalize("test");
    metadata2.insert(main_ident.clone(), metadata);
    let ctx = Context {
        dimensions: vec![],
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
    };

    assert_eq!(None, ctx.fold_flows(&[]));
    assert_eq!(
        Some(Expr::Var(1, Loc::default())),
        ctx.fold_flows(&[canonicalize("a")])
    );
    assert_eq!(
        Some(Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var(1, Loc::default())),
            Box::new(Expr::Var(4, Loc::default())),
            Loc::default(),
        )),
        ctx.fold_flows(&[canonicalize("a"), canonicalize("d")])
    );
}

impl Var {
    pub(crate) fn new(ctx: &Context, var: &Variable) -> Result<Self> {
        // if this variable is overriden by a module input, our expression is easy
        let ast: Vec<Expr> = if let Some((off, _ident)) = ctx
            .inputs
            .iter()
            .enumerate()
            .find(|(_i, n)| n.as_str() == var.ident())
        {
            vec![Expr::AssignCurr(
                ctx.get_offset(&canonicalize(var.ident()))?,
                Box::new(Expr::ModuleInput(off, Loc::default())),
            )]
        } else {
            match var {
                Variable::Module {
                    ident,
                    model_name,
                    inputs,
                    ..
                } => {
                    let mut inputs = inputs.clone();
                    inputs.sort_unstable_by(|a, b| a.dst.partial_cmp(&b.dst).unwrap());
                    let inputs: Vec<Expr> = inputs
                        .into_iter()
                        .map(|mi| Expr::Var(ctx.get_offset(&mi.src).unwrap(), Loc::default()))
                        .collect();
                    vec![Expr::EvalModule(ident.clone(), model_name.clone(), inputs)]
                }
                Variable::Stock { init_ast: ast, .. } => {
                    let off = ctx.get_base_offset(&canonicalize(var.ident()))?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => {
                                vec![Expr::AssignCurr(off, Box::new(ctx.lower(ast)?))]
                            }
                            Ast::ApplyToAll(dims, ast) => {
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(
                                            subscripts
                                                .iter()
                                                .map(|s| CanonicalElementName::from_raw(s))
                                                .collect(),
                                        );
                                        ctx.lower(ast)
                                            .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                            Ast::Arrayed(dims, elements) => {
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let subscript_str = subscripts.join(",");
                                        let canonical_key =
                                            CanonicalElementName::from_raw(&subscript_str);
                                        let ast = &elements[&canonical_key];
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(
                                            subscripts
                                                .iter()
                                                .map(|s| CanonicalElementName::from_raw(s))
                                                .collect(),
                                        );
                                        ctx.lower(ast)
                                            .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    } else {
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(_) => vec![Expr::AssignNext(
                                off,
                                Box::new(ctx.build_stock_update_expr(off, var)),
                            )],
                            Ast::ApplyToAll(dims, _) | Ast::Arrayed(dims, _) => {
                                let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                    .enumerate()
                                    .map(|(i, subscripts)| {
                                        let mut ctx = ctx.clone();
                                        ctx.active_dimension = Some(dims.clone());
                                        ctx.active_subscript = Some(
                                            subscripts
                                                .iter()
                                                .map(|s| CanonicalElementName::from_raw(s))
                                                .collect(),
                                        );
                                        // when building the stock update expression, we need
                                        // the specific index of this subscript, not the base offset
                                        let update_expr = ctx.build_stock_update_expr(
                                            ctx.get_offset(&canonicalize(var.ident()))?,
                                            var,
                                        );
                                        Ok(Expr::AssignNext(off + i, Box::new(update_expr)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    }
                }
                Variable::Var { ident, table, .. } => {
                    let off = ctx.get_base_offset(&canonicalize(var.ident()))?;
                    let ast = if ctx.is_initial {
                        var.init_ast()
                    } else {
                        var.ast()
                    };
                    if ast.is_none() {
                        return sim_err!(EmptyEquation, var.ident().to_string());
                    }
                    match ast.as_ref().unwrap() {
                        Ast::Scalar(ast) => {
                            let expr = ctx.lower(ast)?;
                            let expr = if table.is_some() {
                                let loc = expr.get_loc();
                                Expr::App(
                                    BuiltinFn::Lookup(
                                        ident.as_str().to_string(),
                                        Box::new(expr),
                                        loc,
                                    ),
                                    loc,
                                )
                            } else {
                                expr
                            };
                            vec![Expr::AssignCurr(off, Box::new(expr))]
                        }
                        Ast::ApplyToAll(dims, ast) => {
                            let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                .enumerate()
                                .map(|(i, subscripts)| {
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims.clone());
                                    ctx.active_subscript = Some(
                                        subscripts
                                            .iter()
                                            .map(|s| CanonicalElementName::from_raw(s))
                                            .collect(),
                                    );
                                    ctx.lower(ast)
                                        .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                })
                                .collect();
                            exprs?
                        }
                        Ast::Arrayed(dims, elements) => {
                            let exprs: Result<Vec<Expr>> = SubscriptIterator::new(dims)
                                .enumerate()
                                .map(|(i, subscripts)| {
                                    let subscript_str = subscripts.join(",");
                                    let canonical_key =
                                        CanonicalElementName::from_raw(&subscript_str);
                                    let ast = &elements[&canonical_key];
                                    let mut ctx = ctx.clone();
                                    ctx.active_dimension = Some(dims.clone());
                                    ctx.active_subscript = Some(
                                        subscripts
                                            .iter()
                                            .map(|s| CanonicalElementName::from_raw(s))
                                            .collect(),
                                    );
                                    ctx.lower(ast)
                                        .map(|ast| Expr::AssignCurr(off + i, Box::new(ast)))
                                })
                                .collect();
                            exprs?
                        }
                    }
                }
            }
        };
        Ok(Var {
            ident: canonicalize(var.ident()),
            ast,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Module {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) inputs: HashSet<Ident<Canonical>>,
    pub(crate) n_slots: usize,         // number of f64s we need storage for
    pub(crate) n_temps: usize,         // number of temporary arrays
    pub(crate) temp_sizes: Vec<usize>, // size of each temporary array
    pub(crate) runlist_initials: Vec<Expr>,
    pub(crate) runlist_flows: Vec<Expr>,
    pub(crate) runlist_stocks: Vec<Expr>,
    pub(crate) offsets: VariableOffsetMap,
    pub(crate) runlist_order: Vec<Ident<Canonical>>,
    pub(crate) tables: HashMap<Ident<Canonical>, Table>,
}

/// Create a temporary for an array expression
#[allow(dead_code)]
fn create_temp_for_array_expr(
    expr: Expr,
    next_temp_id: &mut u32,
    temp_sizes: &mut Vec<usize>,
) -> (Expr, Vec<Expr>) {
    // Get the array dimensions from the expression
    let view = get_array_view(&expr);
    if view.is_none() {
        return (expr, vec![]);
    }
    let view = view.unwrap();

    let temp_id = *next_temp_id;
    *next_temp_id += 1;

    // Add the size of this temporary
    let size = view.dims.iter().product();
    temp_sizes.push(size);

    // Create the assignment to populate the temporary
    let assign = Expr::AssignTemp(temp_id, Box::new(expr), view.clone());

    // Return a reference to the temporary
    let temp_ref = Expr::TempArray(temp_id, view, Loc::default());

    (temp_ref, vec![assign])
}

/// Extract the array view from an expression
#[allow(dead_code)]
fn get_array_view(expr: &Expr) -> Option<ArrayView> {
    match expr {
        Expr::StaticSubscript(_, view, _) => Some(view.clone()),
        Expr::Op2(_, l, r, _) => {
            // For binary operations, get the view from the array operand
            get_array_view(l).or_else(|| get_array_view(r))
        }
        Expr::Op1(UnaryOp::Transpose, e, _) => {
            // For transpose, get the view and reverse its dimensions
            get_array_view(e).map(|view| {
                let mut transposed_dims = view.dims.clone();
                transposed_dims.reverse();
                let mut transposed_strides = view.strides.clone();
                transposed_strides.reverse();
                ArrayView {
                    dims: transposed_dims,
                    strides: transposed_strides,
                    offset: view.offset,
                }
            })
        }
        Expr::Op1(_, e, _) => get_array_view(e),
        _ => None,
    }
}

// calculate a mapping of module variable name -> module model name
pub(crate) fn calc_module_model_map(
    project: &Project,
    model_name: &Ident<Canonical>,
) -> HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> {
    let mut all_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&str> = {
        let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
        var_names.sort_unstable();
        var_names
    };

    let mut current_mapping: HashMap<Ident<Canonical>, Ident<Canonical>> = HashMap::new();

    for ident in var_names.iter() {
        let canonical_ident = canonicalize(ident);
        if let Variable::Module {
            model_name: module_model_name,
            ..
        } = &model.variables[&canonical_ident]
        {
            current_mapping.insert(canonical_ident.clone(), module_model_name.clone());
            let all_sub_models = calc_module_model_map(project, module_model_name);
            all_models.extend(all_sub_models);
        };
    }

    all_models.insert(model_name.clone(), current_mapping);

    all_models
}

// TODO: this should memoize
pub(crate) fn build_metadata(
    project: &Project,
    model_name: &Ident<Canonical>,
    is_root: bool,
) -> HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>> {
    let mut all_offsets: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>> =
        HashMap::new();

    let mut offsets: HashMap<Ident<Canonical>, VariableMetadata> = HashMap::new();
    let mut i = 0;
    if is_root {
        offsets.insert(
            canonicalize("time"),
            VariableMetadata {
                offset: 0,
                size: 1,
                var: Variable::Var {
                    ident: canonicalize("time"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        offsets.insert(
            canonicalize("dt"),
            VariableMetadata {
                offset: 1,
                size: 1,
                var: Variable::Var {
                    ident: canonicalize("dt"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        offsets.insert(
            canonicalize("initial_time"),
            VariableMetadata {
                offset: 2,
                size: 1,
                var: Variable::Var {
                    ident: canonicalize("initial_time"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        offsets.insert(
            canonicalize("final_time"),
            VariableMetadata {
                offset: 3,
                size: 1,
                var: Variable::Var {
                    ident: canonicalize("final_time"),
                    ast: None,
                    init_ast: None,
                    eqn: None,
                    units: None,
                    table: None,
                    non_negative: false,
                    is_flow: false,
                    is_table_only: false,
                    errors: vec![],
                    unit_errors: vec![],
                },
            },
        );
        i += IMPLICIT_VAR_COUNT;
    }

    let model = Rc::clone(&project.models[model_name]);
    let var_names: Vec<&Ident<Canonical>> = {
        let mut var_names: Vec<_> = model.variables.keys().collect();
        var_names.sort_unstable();
        var_names
    };

    for canonical_ident in var_names {
        let size = if let Variable::Module { model_name, .. } = &model.variables[canonical_ident] {
            let all_sub_offsets = build_metadata(project, model_name, false);
            let sub_offsets = &all_sub_offsets[model_name];
            let sub_size: usize = sub_offsets.values().map(|metadata| metadata.size).sum();
            all_offsets.extend(all_sub_offsets);
            sub_size
        } else if let Some(Ast::ApplyToAll(dims, _)) = model.variables[canonical_ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else if let Some(Ast::Arrayed(dims, _)) = model.variables[canonical_ident].ast() {
            dims.iter().map(|dim| dim.len()).product()
        } else {
            1
        };
        offsets.insert(
            canonical_ident.clone(),
            VariableMetadata {
                offset: i,
                size,
                var: model.variables[canonical_ident].clone(),
            },
        );
        i += size;
    }

    all_offsets.insert(model_name.clone(), offsets);

    all_offsets
}

fn calc_n_slots(
    all_metadata: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata>>,
    model_name: &Ident<Canonical>,
) -> usize {
    let metadata = &all_metadata[model_name];

    metadata.values().map(|v| v.size).sum()
}

impl Module {
    pub(crate) fn new(
        project: &Project,
        model: Rc<ModelStage1>,
        inputs: &BTreeSet<Ident<Canonical>>,
        is_root: bool,
    ) -> Result<Self> {
        let instantiation = model
            .instantiations
            .as_ref()
            .and_then(|instantiations| instantiations.get(inputs))
            .ok_or(Error {
                kind: ErrorKind::Simulation,
                code: ErrorCode::NotSimulatable,
                details: Some(model.name.to_string()),
            })?;

        // TODO: eventually we should try to simulate subsets of the model in the face of errors
        if model.errors.is_some() && !model.errors.as_ref().unwrap().is_empty() {
            return sim_err!(NotSimulatable, model.name.to_string());
        }

        let model_name: &Ident<Canonical> = &model.name;
        let metadata = build_metadata(project, model_name, is_root);

        let n_slots = calc_n_slots(&metadata, model_name);
        let var_names: Vec<&str> = {
            let mut var_names: Vec<_> = model.variables.keys().map(|s| s.as_str()).collect();
            var_names.sort_unstable();
            var_names
        };
        let module_models = calc_module_model_map(project, model_name);

        let converted_dims: Vec<Dimension> = project
            .datamodel
            .dimensions
            .iter()
            .map(|d| Dimension::from(d.clone()))
            .collect();

        let build_var = |ident: &Ident<Canonical>, is_initial| {
            Var::new(
                &Context {
                    dimensions: converted_dims.clone(),
                    model_name,
                    ident,
                    active_dimension: None,
                    active_subscript: None,
                    metadata: &metadata,
                    module_models: &module_models,
                    is_initial,
                    inputs,
                },
                &model.variables[ident],
            )
        };

        let runlist_initials = instantiation
            .runlist_initials
            .iter()
            .map(|ident| build_var(ident, true))
            .collect::<Result<Vec<Var>>>()?;

        let runlist_flows = instantiation
            .runlist_flows
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var>>>()?;

        let runlist_stocks = instantiation
            .runlist_stocks
            .iter()
            .map(|ident| build_var(ident, false))
            .collect::<Result<Vec<Var>>>()?;

        let mut runlist_order = Vec::with_capacity(runlist_flows.len() + runlist_stocks.len());
        runlist_order.extend(runlist_flows.iter().map(|v| v.ident.clone()));
        runlist_order.extend(runlist_stocks.iter().map(|v| v.ident.clone()));

        // flatten out the variables so that we're just dealing with lists of expressions
        let runlist_initials = runlist_initials.into_iter().flat_map(|v| v.ast).collect();
        let runlist_flows: Vec<Expr> = runlist_flows.into_iter().flat_map(|v| v.ast).collect();
        let runlist_stocks = runlist_stocks.into_iter().flat_map(|v| v.ast).collect();

        let tables: Result<HashMap<Ident<Canonical>, Table>> = var_names
            .iter()
            .map(|id| {
                let canonical_id = canonicalize(id);
                (id, &model.variables[&canonical_id])
            })
            .filter(|(_, v)| v.table().is_some())
            .map(|(id, v)| (id, Table::new(id, v.table().unwrap())))
            .map(|(id, t)| match t {
                Ok(table) => Ok((canonicalize(id), table)),
                Err(err) => Err(err),
            })
            .collect();
        let tables = tables?;

        let offsets = metadata
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    v.iter()
                        .map(|(k, v)| (k.clone(), (v.offset, v.size)))
                        .collect(),
                )
            })
            .collect();

        let n_temps = 0;
        let temp_sizes = vec![];

        Ok(Module {
            ident: model_name.clone(),
            inputs: inputs.iter().cloned().collect(),
            n_slots,
            n_temps,
            temp_sizes,
            runlist_initials,
            runlist_flows,
            runlist_stocks,
            offsets,
            runlist_order,
            tables,
        })
    }

    pub fn compile(&self) -> Result<CompiledModule> {
        Compiler::new(self).compile()
    }
}

struct Compiler<'module> {
    module: &'module Module,
    module_decls: Vec<ModuleDeclaration>,
    graphical_functions: Vec<Vec<(f64, f64)>>,
    curr_code: ByteCodeBuilder,
}

impl<'module> Compiler<'module> {
    fn new(module: &'module Module) -> Compiler<'module> {
        Compiler {
            module,
            module_decls: vec![],
            graphical_functions: vec![],
            curr_code: ByteCodeBuilder::default(),
        }
    }

    fn walk(&mut self, exprs: &[Expr]) -> Result<ByteCode> {
        for expr in exprs.iter() {
            self.walk_expr(expr)?;
        }
        self.push(Opcode::Ret);

        let curr = std::mem::take(&mut self.curr_code);

        Ok(curr.finish())
    }

    fn walk_expr(&mut self, expr: &Expr) -> Result<Option<()>> {
        let result = match expr {
            Expr::Const(value, _) => {
                let id = self.curr_code.intern_literal(*value);
                self.push(Opcode::LoadConstant { id });
                Some(())
            }
            Expr::Var(off, _) => {
                self.push(Opcode::LoadVar {
                    off: *off as VariableOffset,
                });
                Some(())
            }
            Expr::Subscript(off, indices, bounds, _) => {
                for (i, expr) in indices.iter().enumerate() {
                    self.walk_expr(expr).unwrap().unwrap();
                    let bounds = bounds[i] as VariableOffset;
                    self.push(Opcode::PushSubscriptIndex { bounds });
                }
                assert!(indices.len() == bounds.len());
                self.push(Opcode::LoadSubscript {
                    off: *off as VariableOffset,
                });
                Some(())
            }
            Expr::StaticSubscript(off, view, _) => {
                // For static subscripts, we can directly compute the final offset
                // For now, just load from the offset + view offset
                // TODO: This needs proper iteration support for non-scalar views
                let final_off = (*off + view.offset) as VariableOffset;
                self.push(Opcode::LoadVar { off: final_off });
                Some(())
            }
            Expr::TempArray(_id, _view, _) => {
                // TODO: Implement loading from temporary arrays
                // For now, just return an error
                return sim_err!(
                    Generic,
                    "TempArray not yet implemented in bytecode compiler".to_string()
                );
            }
            Expr::TempArrayElement(_id, _view, _idx, _) => {
                // TODO: Implement loading from temporary array elements
                // For now, just return an error
                return sim_err!(
                    Generic,
                    "TempArrayElement not yet implemented in bytecode compiler".to_string()
                );
            }
            Expr::Dt(_) => {
                self.push(Opcode::LoadGlobalVar {
                    off: DT_OFF as VariableOffset,
                });
                Some(())
            }
            Expr::App(builtin, _) => {
                // lookups are special
                if let BuiltinFn::Lookup(ident, index, _loc) = builtin {
                    let table = &self.module.tables[&canonicalize(ident)];
                    self.graphical_functions.push(table.data.clone());
                    let gf = (self.graphical_functions.len() - 1) as GraphicalFunctionId;
                    self.walk_expr(index)?.unwrap();
                    self.push(Opcode::Lookup { gf });
                    return Ok(Some(()));
                };

                // so are module builtins
                if let BuiltinFn::IsModuleInput(ident, _loc) = builtin {
                    let id = if self.module.inputs.contains(&canonicalize(ident)) {
                        self.curr_code.intern_literal(1.0)
                    } else {
                        self.curr_code.intern_literal(0.0)
                    };
                    self.push(Opcode::LoadConstant { id });
                    return Ok(Some(()));
                };

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
                        } as u16;
                        self.push(Opcode::LoadGlobalVar { off });
                        return Ok(Some(()));
                    }
                    BuiltinFn::Lookup(_, _, _) | BuiltinFn::IsModuleInput(_, _) => unreachable!(),
                    BuiltinFn::Inf | BuiltinFn::Pi => {
                        let lit = match builtin {
                            BuiltinFn::Inf => f64::INFINITY,
                            BuiltinFn::Pi => std::f64::consts::PI,
                            _ => unreachable!(),
                        };
                        let id = self.curr_code.intern_literal(lit);
                        self.push(Opcode::LoadConstant { id });
                        return Ok(Some(()));
                    }
                    BuiltinFn::Abs(a)
                    | BuiltinFn::Arccos(a)
                    | BuiltinFn::Arcsin(a)
                    | BuiltinFn::Arctan(a)
                    | BuiltinFn::Cos(a)
                    | BuiltinFn::Exp(a)
                    | BuiltinFn::Int(a)
                    | BuiltinFn::Ln(a)
                    | BuiltinFn::Log10(a)
                    | BuiltinFn::Sign(a)
                    | BuiltinFn::Sin(a)
                    | BuiltinFn::Sqrt(a)
                    | BuiltinFn::Tan(a) => {
                        self.walk_expr(a)?.unwrap();
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(Opcode::LoadConstant { id });
                        self.push(Opcode::LoadConstant { id });
                    }
                    BuiltinFn::Step(a, b) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(Opcode::LoadConstant { id });
                    }
                    BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
                        if let Some(b) = b {
                            self.walk_expr(a)?.unwrap();
                            self.walk_expr(b)?.unwrap();
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        } else {
                            return sim_err!(BadBuiltinArgs, "".to_owned());
                        }
                    }
                    BuiltinFn::Pulse(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        if c.is_some() {
                            self.walk_expr(c.as_ref().unwrap())?.unwrap()
                        } else {
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        };
                    }
                    BuiltinFn::Ramp(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        if c.is_some() {
                            self.walk_expr(c.as_ref().unwrap())?.unwrap()
                        } else {
                            self.push(Opcode::LoadVar {
                                off: FINAL_TIME_OFF as u16,
                            });
                        };
                    }
                    BuiltinFn::SafeDiv(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        let c = c.as_ref().map(|c| self.walk_expr(c).unwrap().unwrap());
                        if c.is_none() {
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        }
                    }
                    BuiltinFn::Mean(args) => {
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(Opcode::LoadConstant { id });

                        for arg in args.iter() {
                            self.walk_expr(arg)?.unwrap();
                            self.push(Opcode::Op2 { op: Op2::Add });
                        }

                        let id = self.curr_code.intern_literal(args.len() as f64);
                        self.push(Opcode::LoadConstant { id });
                        self.push(Opcode::Op2 { op: Op2::Div });
                        return Ok(Some(()));
                    }
                    BuiltinFn::Rank(_, _) => {
                        return sim_err!(TodoArrayBuiltin, "".to_owned());
                    }
                    BuiltinFn::Size(_) => {
                        return sim_err!(TodoArrayBuiltin, "".to_owned());
                    }
                    BuiltinFn::Stddev(_) => {
                        return sim_err!(TodoArrayBuiltin, "".to_owned());
                    }
                    BuiltinFn::Sum(_) => {
                        return sim_err!(TodoArrayBuiltin, "".to_owned());
                    }
                };
                let func = match builtin {
                    BuiltinFn::Lookup(_, _, _) => unreachable!(),
                    BuiltinFn::Abs(_) => BuiltinId::Abs,
                    BuiltinFn::Arccos(_) => BuiltinId::Arccos,
                    BuiltinFn::Arcsin(_) => BuiltinId::Arcsin,
                    BuiltinFn::Arctan(_) => BuiltinId::Arctan,
                    BuiltinFn::Cos(_) => BuiltinId::Cos,
                    BuiltinFn::Exp(_) => BuiltinId::Exp,
                    BuiltinFn::Inf => BuiltinId::Inf,
                    BuiltinFn::Int(_) => BuiltinId::Int,
                    BuiltinFn::IsModuleInput(_, _) => unreachable!(),
                    BuiltinFn::Ln(_) => BuiltinId::Ln,
                    BuiltinFn::Log10(_) => BuiltinId::Log10,
                    BuiltinFn::Max(_, _) => BuiltinId::Max,
                    BuiltinFn::Mean(_) => unreachable!(),
                    BuiltinFn::Min(_, _) => BuiltinId::Min,
                    BuiltinFn::Pi => BuiltinId::Pi,
                    BuiltinFn::Pulse(_, _, _) => BuiltinId::Pulse,
                    BuiltinFn::Ramp(_, _, _) => BuiltinId::Ramp,
                    BuiltinFn::SafeDiv(_, _, _) => BuiltinId::SafeDiv,
                    BuiltinFn::Sign(_) => BuiltinId::Sign,
                    BuiltinFn::Sin(_) => BuiltinId::Sin,
                    BuiltinFn::Sqrt(_) => BuiltinId::Sqrt,
                    BuiltinFn::Step(_, _) => BuiltinId::Step,
                    BuiltinFn::Tan(_) => BuiltinId::Tan,
                    // handled above; we exit early
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => unreachable!(),
                    BuiltinFn::Rank(_, _)
                    | BuiltinFn::Size(_)
                    | BuiltinFn::Stddev(_)
                    | BuiltinFn::Sum(_) => {
                        return sim_err!(TodoArrayBuiltin, "".to_owned());
                    }
                };

                self.push(Opcode::Apply { func });
                Some(())
            }
            Expr::EvalModule(ident, model_name, args) => {
                for arg in args.iter() {
                    self.walk_expr(arg).unwrap().unwrap()
                }
                let module_offsets = &self.module.offsets[&self.module.ident];
                self.module_decls.push(ModuleDeclaration {
                    model_name: model_name.clone(),
                    off: module_offsets[ident].0,
                });
                let id = (self.module_decls.len() - 1) as ModuleId;

                self.push(Opcode::EvalModule {
                    id,
                    n_inputs: args.len() as u8,
                });
                None
            }
            Expr::ModuleInput(off, _) => {
                self.push(Opcode::LoadModuleInput {
                    input: *off as ModuleInputOffset,
                });
                Some(())
            }
            Expr::Op2(op, lhs, rhs, _) => {
                self.walk_expr(lhs)?.unwrap();
                self.walk_expr(rhs)?.unwrap();
                let opcode = match op {
                    BinaryOp::Add => Opcode::Op2 { op: Op2::Add },
                    BinaryOp::Sub => Opcode::Op2 { op: Op2::Sub },
                    BinaryOp::Exp => Opcode::Op2 { op: Op2::Exp },
                    BinaryOp::Mul => Opcode::Op2 { op: Op2::Mul },
                    BinaryOp::Div => Opcode::Op2 { op: Op2::Div },
                    BinaryOp::Mod => Opcode::Op2 { op: Op2::Mod },
                    BinaryOp::Gt => Opcode::Op2 { op: Op2::Gt },
                    BinaryOp::Gte => Opcode::Op2 { op: Op2::Gte },
                    BinaryOp::Lt => Opcode::Op2 { op: Op2::Lt },
                    BinaryOp::Lte => Opcode::Op2 { op: Op2::Lte },
                    BinaryOp::Eq => Opcode::Op2 { op: Op2::Eq },
                    BinaryOp::Neq => {
                        self.push(Opcode::Op2 { op: Op2::Eq });
                        Opcode::Not {}
                    }
                    BinaryOp::And => Opcode::Op2 { op: Op2::And },
                    BinaryOp::Or => Opcode::Op2 { op: Op2::Or },
                };
                self.push(opcode);
                Some(())
            }
            Expr::Op1(op, rhs, _) => {
                self.walk_expr(rhs)?.unwrap();
                match op {
                    UnaryOp::Not => self.push(Opcode::Not {}),
                    UnaryOp::Transpose => {
                        unreachable!("Transpose should be handled at compile time in lower()");
                    }
                };
                Some(())
            }
            Expr::If(cond, t, f, _) => {
                self.walk_expr(t)?.unwrap();
                self.walk_expr(f)?.unwrap();
                self.walk_expr(cond)?.unwrap();
                self.push(Opcode::SetCond {});
                self.push(Opcode::If {});
                Some(())
            }
            Expr::AssignCurr(off, rhs) => {
                self.walk_expr(rhs)?.unwrap();
                self.push(Opcode::AssignCurr {
                    off: *off as VariableOffset,
                });
                None
            }
            Expr::AssignNext(off, rhs) => {
                self.walk_expr(rhs)?.unwrap();
                self.push(Opcode::AssignNext {
                    off: *off as VariableOffset,
                });
                None
            }
            Expr::AssignTemp(_id, _rhs, _view) => {
                // TODO: Implement AssignTemp in bytecode compiler
                return sim_err!(
                    Generic,
                    "AssignTemp not yet implemented in bytecode compiler".to_string()
                );
            }
        };
        Ok(result)
    }

    fn push(&mut self, op: Opcode) {
        self.curr_code.push_opcode(op)
    }

    fn compile(mut self) -> Result<CompiledModule> {
        let compiled_initials = Rc::new(self.walk(&self.module.runlist_initials)?);
        let compiled_flows = Rc::new(self.walk(&self.module.runlist_flows)?);
        let compiled_stocks = Rc::new(self.walk(&self.module.runlist_stocks)?);

        Ok(CompiledModule {
            ident: self.module.ident.clone(),
            n_slots: self.module.n_slots,
            context: Rc::new(ByteCodeContext {
                graphical_functions: self.graphical_functions,
                modules: self.module_decls,
                arrays: vec![],
            }),
            compiled_initials,
            compiled_flows,
            compiled_stocks,
        })
    }
}

fn child_needs_parens(parent: &Expr, child: &Expr) -> bool {
    match parent {
        // no children so doesn't matter
        Expr::Const(_, _) | Expr::Var(_, _) => false,
        // children are comma separated, so no ambiguity possible
        Expr::App(_, _)
        | Expr::Subscript(_, _, _, _)
        | Expr::StaticSubscript(_, _, _)
        | Expr::TempArray(_, _, _)
        | Expr::TempArrayElement(_, _, _, _) => false,
        // these don't need it
        Expr::Dt(_)
        | Expr::EvalModule(_, _, _)
        | Expr::ModuleInput(_, _)
        | Expr::AssignCurr(_, _)
        | Expr::AssignNext(_, _)
        | Expr::AssignTemp(_, _, _) => false,
        Expr::Op1(_, _, _) => matches!(child, Expr::Op2(_, _, _, _)),
        Expr::Op2(parent_op, _, _, _) => match child {
            Expr::Const(_, _)
            | Expr::Var(_, _)
            | Expr::App(_, _)
            | Expr::Subscript(_, _, _, _)
            | Expr::StaticSubscript(_, _, _)
            | Expr::TempArray(_, _, _)
            | Expr::TempArrayElement(_, _, _, _)
            | Expr::If(_, _, _, _)
            | Expr::Dt(_)
            | Expr::EvalModule(_, _, _)
            | Expr::ModuleInput(_, _)
            | Expr::AssignCurr(_, _)
            | Expr::AssignNext(_, _)
            | Expr::AssignTemp(_, _, _)
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
        format!("({eqn})")
    } else {
        eqn
    }
}

#[allow(dead_code)]
pub fn pretty(expr: &Expr) -> String {
    match expr {
        Expr::Const(n, _) => format!("{n}"),
        Expr::Var(off, _) => format!("curr[{off}]"),
        Expr::StaticSubscript(off, view, _) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            let strides: Vec<_> = view.strides.iter().map(|s| format!("{s}")).collect();
            format!(
                "curr[{off} + view(dims: [{}], strides: [{}], offset: {})]",
                dims.join(", "),
                strides.join(", "),
                view.offset
            )
        }
        Expr::TempArray(id, view, _) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            let strides: Vec<_> = view.strides.iter().map(|s| format!("{s}")).collect();
            format!(
                "temp[{id}] + view(dims: [{}], strides: [{}], offset: {})",
                dims.join(", "),
                strides.join(", "),
                view.offset
            )
        }
        Expr::TempArrayElement(id, view, idx, _) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            format!("temp[{id}][{idx}] (dims: [{}])", dims.join(", "))
        }
        Expr::Subscript(off, args, bounds, _) => {
            let args: Vec<_> = args.iter().map(pretty).collect();
            let string_args = args.join(", ");
            let bounds: Vec<_> = bounds.iter().map(|bounds| format!("{bounds}")).collect();
            let string_bounds = bounds.join(", ");
            format!("curr[{off} + (({string_args}) - 1); bounds: {string_bounds}]")
        }
        Expr::Dt(_) => "dt".to_string(),
        Expr::App(builtin, _) => match builtin {
            BuiltinFn::Time => "time".to_string(),
            BuiltinFn::TimeStep => "time_step".to_string(),
            BuiltinFn::StartTime => "initial_time".to_string(),
            BuiltinFn::FinalTime => "final_time".to_string(),
            BuiltinFn::Lookup(table, idx, _loc) => format!("lookup({}, {})", table, pretty(idx)),
            BuiltinFn::Abs(l) => format!("abs({})", pretty(l)),
            BuiltinFn::Arccos(l) => format!("arccos({})", pretty(l)),
            BuiltinFn::Arcsin(l) => format!("arcsin({})", pretty(l)),
            BuiltinFn::Arctan(l) => format!("arctan({})", pretty(l)),
            BuiltinFn::Cos(l) => format!("cos({})", pretty(l)),
            BuiltinFn::Exp(l) => format!("exp({})", pretty(l)),
            BuiltinFn::Inf => "∞".to_string(),
            BuiltinFn::Int(l) => format!("int({})", pretty(l)),
            BuiltinFn::IsModuleInput(ident, _loc) => format!("isModuleInput({ident})"),
            BuiltinFn::Ln(l) => format!("ln({})", pretty(l)),
            BuiltinFn::Log10(l) => format!("log10({})", pretty(l)),
            BuiltinFn::Max(l, r) => {
                if let Some(r) = r {
                    format!("max({}, {})", pretty(l), pretty(r))
                } else {
                    format!("max({})", pretty(l))
                }
            }
            BuiltinFn::Mean(args) => {
                let args: Vec<_> = args.iter().map(pretty).collect();
                let string_args = args.join(", ");
                format!("mean({string_args})")
            }
            BuiltinFn::Min(l, r) => {
                if let Some(r) = r {
                    format!("min({}, {})", pretty(l), pretty(r))
                } else {
                    format!("min({})", pretty(l))
                }
            }
            BuiltinFn::Pi => "𝜋".to_string(),
            BuiltinFn::Pulse(a, b, c) => {
                let c = match c.as_ref() {
                    Some(c) => pretty(c),
                    None => "0<default>".to_owned(),
                };
                format!("pulse({}, {}, {})", pretty(a), pretty(b), c)
            }
            BuiltinFn::Ramp(a, b, c) => {
                let c = match c.as_ref() {
                    Some(c) => pretty(c),
                    None => "0<default>".to_owned(),
                };
                format!("ramp({}, {}, {})", pretty(a), pretty(b), c)
            }
            BuiltinFn::SafeDiv(a, b, c) => format!(
                "safediv({}, {}, {})",
                pretty(a),
                pretty(b),
                c.as_ref()
                    .map(|expr| pretty(expr))
                    .unwrap_or_else(|| "<None>".to_string())
            ),
            BuiltinFn::Sign(l) => format!("sign({})", pretty(l)),
            BuiltinFn::Sin(l) => format!("sin({})", pretty(l)),
            BuiltinFn::Sqrt(l) => format!("sqrt({})", pretty(l)),
            BuiltinFn::Step(a, b) => {
                format!("step({}, {})", pretty(a), pretty(b))
            }
            BuiltinFn::Tan(l) => format!("tan({})", pretty(l)),
            BuiltinFn::Rank(a, b) => {
                if let Some((b, c)) = b {
                    if let Some(c) = c {
                        format!("rank({}, {}, {})", pretty(a), pretty(b), pretty(c))
                    } else {
                        format!("rank({}, {})", pretty(a), pretty(b))
                    }
                } else {
                    format!("rank({})", pretty(a))
                }
            }
            BuiltinFn::Size(a) => format!("size({})", pretty(a)),
            BuiltinFn::Stddev(a) => format!("stddev({})", pretty(a)),
            BuiltinFn::Sum(a) => format!("sum({})", pretty(a)),
        },
        Expr::EvalModule(module, model_name, args) => {
            let args: Vec<_> = args.iter().map(pretty).collect();
            let string_args = args.join(", ");
            format!("eval<{module}::{model_name}>({string_args})")
        }
        Expr::ModuleInput(a, _) => format!("mi<{a}>"),
        Expr::Op2(op, l, r, _) => {
            let op: &str = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Exp => "^",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::Mod => "%",
                BinaryOp::Gt => ">",
                BinaryOp::Gte => ">=",
                BinaryOp::Lt => "<",
                BinaryOp::Lte => "<=",
                BinaryOp::Eq => "==",
                BinaryOp::Neq => "!=",
                BinaryOp::And => "&&",
                BinaryOp::Or => "||",
            };

            format!(
                "{} {} {}",
                paren_if_necessary(expr, l, pretty(l)),
                op,
                paren_if_necessary(expr, r, pretty(r))
            )
        }
        Expr::Op1(op, l, _) => {
            let op: &str = match op {
                UnaryOp::Not => "!",
                UnaryOp::Transpose => "'",
            };
            format!("{}{}", op, paren_if_necessary(expr, l, pretty(l)))
        }
        Expr::If(cond, l, r, _) => {
            format!("if {} then {} else {}", pretty(cond), pretty(l), pretty(r))
        }
        Expr::AssignCurr(off, rhs) => format!("curr[{}] := {}", off, pretty(rhs)),
        Expr::AssignNext(off, rhs) => format!("next[{}] := {}", off, pretty(rhs)),
        Expr::AssignTemp(id, expr, view) => {
            let dims: Vec<_> = view.dims.iter().map(|d| format!("{d}")).collect();
            format!("temp[{id}][{}] <- {}", dims.join(", "), pretty(expr))
        }
    }
}

/// Determines if dimensions can be reordered to match target dimensions and returns the reordering
///
/// Given source dimensions and target dimensions, determines if the source can be
/// reordered to match the target. If so, returns a vector of indices indicating
/// how to reorder the source dimensions (suitable for use as @N subscripts).
///
/// # Arguments
/// * `source_dims` - The dimension names of the source array
/// * `target_dims` - The dimension names of the target array
///
/// # Returns
/// * `Some(reordering)` - A vector where reordering[i] is the source dimension index
///   that should go in position i of the target
/// * `None` - If the dimensions cannot be reordered to match (different sets of dimensions)
///
/// # Examples
/// ```
/// // source: [A, B, C], target: [B, C, A]
/// // returns: Some([1, 2, 0]) meaning [@2, @3, @1] in XMILE notation (1-indexed)
/// ```
pub fn find_dimension_reordering(
    source_dims: &[String],
    target_dims: &[String],
) -> Option<Vec<usize>> {
    if source_dims.len() != target_dims.len() {
        return None;
    }

    // Build a map of dimension name to index in source
    let mut source_map: HashMap<&str, usize> = HashMap::new();
    for (i, dim) in source_dims.iter().enumerate() {
        source_map.insert(dim.as_str(), i);
    }

    // Check if all target dimensions exist in source and build reordering
    let mut reordering = Vec::with_capacity(target_dims.len());
    for target_dim in target_dims {
        match source_map.get(target_dim.as_str()) {
            Some(&source_idx) => reordering.push(source_idx),
            None => return None, // Target dimension not found in source
        }
    }

    // Verify we've used all source dimensions (no duplicates in target)
    let mut used = vec![false; source_dims.len()];
    for &idx in &reordering {
        if used[idx] {
            return None; // Duplicate dimension in target
        }
        used[idx] = true;
    }

    Some(reordering)
}

// simplified/lowered from ast::UnaryOp version
#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
pub enum UnaryOp {
    Not,
    Transpose,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_dimension_reordering() {
        // Test identical dimensions
        let source = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let target = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![0, 1, 2])
        );

        // Test simple transpose (2D)
        let source = vec!["Row".to_string(), "Col".to_string()];
        let target = vec!["Col".to_string(), "Row".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![1, 0])
        );

        // Test 3D reordering: [A, B, C] -> [B, C, A]
        let source = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let target = vec!["B".to_string(), "C".to_string(), "A".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![1, 2, 0])
        );

        // Test 3D reordering: [A, B, C] -> [C, A, B]
        let source = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let target = vec!["C".to_string(), "A".to_string(), "B".to_string()];
        assert_eq!(
            find_dimension_reordering(&source, &target),
            Some(vec![2, 0, 1])
        );

        // Test different dimensions - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["C".to_string(), "D".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test missing dimension - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["A".to_string(), "C".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test different lengths - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test duplicate dimensions in target - should return None
        let source = vec!["A".to_string(), "B".to_string()];
        let target = vec!["A".to_string(), "A".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), None);

        // Test single dimension
        let source = vec!["X".to_string()];
        let target = vec!["X".to_string()];
        assert_eq!(find_dimension_reordering(&source, &target), Some(vec![0]));

        // Test empty dimensions
        let source: Vec<String> = vec![];
        let target: Vec<String> = vec![];
        assert_eq!(find_dimension_reordering(&source, &target), Some(vec![]));
    }

    #[test]
    fn test_array_view_contiguous() {
        // Test creating a contiguous 2D array view
        let view = ArrayView::contiguous(vec![3, 4]);

        assert_eq!(view.dims, vec![3, 4]);
        assert_eq!(view.strides, vec![4, 1]); // Row-major order
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 12);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_contiguous_1d() {
        // Test creating a contiguous 1D array view
        let view = ArrayView::contiguous(vec![5]);

        assert_eq!(view.dims, vec![5]);
        assert_eq!(view.strides, vec![1]);
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 5);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_contiguous_3d() {
        // Test creating a contiguous 3D array view
        let view = ArrayView::contiguous(vec![2, 3, 4]);

        assert_eq!(view.dims, vec![2, 3, 4]);
        assert_eq!(view.strides, vec![12, 4, 1]); // Row-major: 3*4, 4, 1
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 24);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_apply_range_first_dim() {
        // Test applying a range to the first dimension
        let view = ArrayView::contiguous(vec![5, 3]);
        let sliced = view.apply_range_subscript(0, 2, 5).unwrap();

        assert_eq!(sliced.dims, vec![3, 3]); // [2:5] gives 3 elements
        assert_eq!(sliced.strides, vec![3, 1]); // Same strides
        assert_eq!(sliced.offset, 6); // Skip first 2 rows (2 * 3 = 6)
        assert_eq!(sliced.size(), 9);
        assert!(!sliced.is_contiguous()); // No longer contiguous due to offset
    }

    #[test]
    fn test_array_view_apply_range_second_dim() {
        // Test applying a range to the second dimension
        let view = ArrayView::contiguous(vec![3, 5]);
        let sliced = view.apply_range_subscript(1, 1, 3).unwrap();

        assert_eq!(sliced.dims, vec![3, 2]); // [1:3] gives 2 elements
        assert_eq!(sliced.strides, vec![5, 1]); // Row stride unchanged
        assert_eq!(sliced.offset, 1); // Skip first column
        assert_eq!(sliced.size(), 6);
        assert!(!sliced.is_contiguous());
    }

    #[test]
    fn test_array_view_apply_range_1d() {
        // Test applying a range to a 1D array (like source[3:5])
        let view = ArrayView::contiguous(vec![5]);
        let sliced = view.apply_range_subscript(0, 2, 5).unwrap(); // 0-based: [2:5)

        assert_eq!(sliced.dims, vec![3]); // Elements at indices 2, 3, 4
        assert_eq!(sliced.strides, vec![1]);
        assert_eq!(sliced.offset, 2);
        assert_eq!(sliced.size(), 3);
        assert!(!sliced.is_contiguous()); // Has non-zero offset
    }

    #[test]
    fn test_array_view_range_bounds_checking() {
        let view = ArrayView::contiguous(vec![5, 3]);

        // Test out of bounds dimension index
        assert!(view.apply_range_subscript(2, 0, 1).is_err());

        // Test invalid range (start >= end)
        assert!(view.apply_range_subscript(0, 3, 3).is_err());
        assert!(view.apply_range_subscript(0, 4, 2).is_err());

        // Test range exceeding dimension size
        assert!(view.apply_range_subscript(0, 0, 6).is_err());
        assert!(view.apply_range_subscript(0, 4, 6).is_err());
    }

    #[test]
    fn test_array_view_empty_array() {
        // Test edge case of empty array
        let view = ArrayView::contiguous(vec![]);

        assert_eq!(view.dims, vec![]);
        assert_eq!(view.strides, vec![]);
        assert_eq!(view.offset, 0);
        assert_eq!(view.size(), 1); // Empty product is 1
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_array_view_is_contiguous() {
        // Test various cases for is_contiguous

        // Contiguous: fresh array
        let view1 = ArrayView::contiguous(vec![3, 4]);
        assert!(view1.is_contiguous());

        // Not contiguous: has offset
        let view2 = ArrayView {
            dims: vec![3, 4],
            strides: vec![4, 1],
            offset: 5,
        };
        assert!(!view2.is_contiguous());

        // Not contiguous: wrong strides for row-major
        let view3 = ArrayView {
            dims: vec![3, 4],
            strides: vec![1, 3], // Column-major strides
            offset: 0,
        };
        assert!(!view3.is_contiguous());

        // Contiguous: manually constructed but correct
        let view4 = ArrayView {
            dims: vec![2, 3, 4],
            strides: vec![12, 4, 1],
            offset: 0,
        };
        assert!(view4.is_contiguous());
    }
}
