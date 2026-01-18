// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::ast::{
    self, ArrayView, Ast, BinaryOp, Expr3, Expr3LowerContext, IndexExpr3, Loc, Pass1Context,
    SparseInfo,
};
use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeBuilder, ByteCodeContext, CompiledModule, DimId, DimensionInfo,
    GraphicalFunctionId, LookupMode, ModuleDeclaration, ModuleId, ModuleInputOffset, NameId, Op2,
    Opcode, RuntimeSparseMapping, StaticArrayView, SubdimensionRelation, TempId, VariableOffset,
    ViewId,
};
use crate::common::{
    Canonical, CanonicalElementName, ErrorCode, ErrorKind, Ident, Result, canonicalize,
};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::model::ModelStage1;
use crate::project::Project;
use crate::variable::Variable;
use crate::vm::{
    DT_OFF, FINAL_TIME_OFF, IMPLICIT_VAR_COUNT, INITIAL_TIME_OFF, ModuleKey, SubscriptIterator,
    TIME_OFF,
};
use crate::{Error, sim_err};
use smallvec::SmallVec;

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

/// Represents a subscript operation after parsing but before view construction.
/// Used to normalize different subscript syntaxes into a uniform representation
/// that can be processed by build_view_from_ops.
#[derive(Clone, Debug, PartialEq)]
enum IndexOp {
    /// Range subscript with start and end (0-based, end exclusive).
    /// Example: `arr[2:5]` becomes `Range(1, 5)` (converted from 1-based)
    Range(usize, usize),
    /// Single element access (0-based index).
    /// Example: `arr[3]` becomes `Single(2)` (converted from 1-based)
    Single(usize),
    /// Wildcard that preserves the dimension.
    /// Example: `arr[*]` keeps the full dimension
    Wildcard,
    /// Dimension position reference (0-based).
    /// Example: `arr[@2]` references dimension at position 1
    DimPosition(usize),
    /// Sparse (non-contiguous) range for subdimension iteration.
    /// Contains parent offsets to iterate (e.g., [0, 2] for elements at indices 0 and 2)
    SparseRange(Vec<usize>),
    /// Reference to an active A2A dimension by index.
    /// Used when a dimension name appears as a subscript in A2A context
    ActiveDimRef(usize),
}

/// Result of building an ArrayView from IndexOp operations.
struct ViewBuildResult {
    /// The constructed array view
    view: ArrayView,
    /// Mapping from output dimension index to input dimension index.
    /// dim_mapping[i] = Some(j) means output dim i comes from input dim j.
    /// dim_mapping[i] = None means output dim i was removed (single index).
    dim_mapping: Vec<Option<usize>>,
    /// Start offset for each input dimension (for A2A element index calculation)
    single_indices: Vec<usize>,
}

/// Configuration for view building.
/// Contains context needed for ActiveDimRef resolution.
struct ViewBuildConfig<'a> {
    /// Active A2A subscript values (if in A2A context)
    active_subscript: Option<&'a [CanonicalElementName]>,
    /// Dimensions of the variable being subscripted (for element name → offset lookups)
    dims: &'a [Dimension],
}

/// Configuration for subscript normalization from Expr3.
struct Subscript3Config<'a> {
    /// Dimensions of the variable being subscripted
    dims: &'a [Dimension],
    /// All dimensions in the model (for checking if a name is a dimension)
    all_dimensions: &'a [Dimension],
    /// For subdimension relationship lookups
    dimensions_ctx: &'a DimensionsContext,
    /// Active A2A dimensions (if in A2A context)
    active_dimension: Option<&'a [Dimension]>,
}

/// Normalize IndexExpr3 subscripts to IndexOp operations.
///
/// Returns Some(ops) if all subscripts can be converted statically,
/// None if any subscript requires dynamic evaluation.
///
/// Key features:
/// - Handles IndexExpr3::StarRange where the name might be the full dimension (not just subdimension)
/// - Handles IndexExpr3::Dimension for A2A dimension references
/// - No Wildcard variant (wildcards are converted to StarRange in pass 0)
fn normalize_subscripts3(args: &[IndexExpr3], config: &Subscript3Config) -> Option<Vec<IndexOp>> {
    use crate::common::CanonicalDimensionName;

    let mut operations = Vec::with_capacity(args.len());

    for (i, arg) in args.iter().enumerate() {
        if i >= config.dims.len() {
            return None;
        }

        let parent_dim = &config.dims[i];
        let parent_name = CanonicalDimensionName::from_raw(parent_dim.name());

        let op = match arg {
            IndexExpr3::StarRange(subdim_name, _) => {
                // Check if this is the full dimension (from wildcard conversion in pass 0)
                if subdim_name.as_str() == parent_name.as_str() {
                    // Full dimension - treat as Wildcard
                    IndexOp::Wildcard
                } else {
                    // Check if subdim_name refers to an indexed dimension.
                    // For indexed dimensions, *:IndexedDim desugars to [1:SIZE(IndexedDim)],
                    // which is Range(0, size) in 0-based internal representation.
                    let subdim_canonical = canonicalize(subdim_name.as_str());
                    let indexed_dim_size = config.all_dimensions.iter().find_map(|d| {
                        if canonicalize(d.name()).as_str() == subdim_canonical.as_str() {
                            if let Dimension::Indexed(_, size) = d {
                                Some(*size as usize)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    });

                    if let Some(size) = indexed_dim_size {
                        // Indexed subdimension - desugar to range [0, size) (0-based)
                        IndexOp::Range(0, size)
                    } else {
                        // Named subdimension - look up relationship
                        let relation = config
                            .dimensions_ctx
                            .get_subdimension_relation(subdim_name, &parent_name)?;

                        if relation.is_contiguous() {
                            let start = relation.start_offset();
                            let end = start + relation.parent_offsets.len();
                            IndexOp::Range(start, end)
                        } else {
                            IndexOp::SparseRange(relation.parent_offsets.clone())
                        }
                    }
                }
            }

            // StaticRange - already has 0-based indices from Expr2→Expr3 lowering
            IndexExpr3::StaticRange(start_0based, end_0based, _) => {
                IndexOp::Range(*start_0based, *end_0based)
            }

            IndexExpr3::Range(start_expr, end_expr, _) => {
                // Dynamic range - try to resolve both bounds to constants
                // If either can't be resolved, normalization fails and we fall back to dynamic handling
                let resolve_to_index = |expr: &Expr3| -> Option<usize> {
                    match expr {
                        Expr3::Const(_, val, _) => {
                            // Numeric constant - convert from 1-based to 0-based
                            Some((*val as isize - 1).max(0) as usize)
                        }
                        Expr3::Var(ident, _, _) => {
                            // Could be a named dimension element
                            if let Dimension::Named(_, named_dim) = parent_dim {
                                named_dim
                                    .elements
                                    .iter()
                                    .position(|elem| elem.as_str() == ident.as_str())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                };

                let start_idx = resolve_to_index(start_expr)?;
                let end_idx = resolve_to_index(end_expr)?;
                // end_idx is inclusive in the source, but we need exclusive for the range
                IndexOp::Range(start_idx, end_idx + 1)
            }

            IndexExpr3::DimPosition(pos, _) => {
                // @1 is position 0, @2 is position 1, etc.
                let dim_idx = (*pos as usize).saturating_sub(1);
                IndexOp::DimPosition(dim_idx)
            }

            IndexExpr3::Expr(expr) => {
                match expr {
                    Expr3::Const(_, val, _) => {
                        let idx = (*val as isize - 1).max(0) as usize;
                        IndexOp::Single(idx)
                    }
                    Expr3::Var(ident, _, _) => {
                        // First check if it's a named dimension element (takes priority)
                        let element_idx = if let Dimension::Named(_, named_dim) = parent_dim {
                            named_dim
                                .elements
                                .iter()
                                .position(|elem| elem.as_str() == ident.as_str())
                        } else {
                            None
                        };

                        if let Some(idx) = element_idx {
                            IndexOp::Single(idx)
                        } else if let Dimension::Indexed(dim_name, size) = parent_dim {
                            // For indexed dimensions, check if ident is "DimName.Index" format
                            let expected_prefix = format!("{}.", dim_name.as_str());
                            if ident.as_str().starts_with(&expected_prefix) {
                                if let Ok(idx) =
                                    ident.as_str()[expected_prefix.len()..].parse::<usize>()
                                {
                                    // Validate the index is within bounds (1-based)
                                    let size_usize = *size as usize;
                                    if idx >= 1 && idx <= size_usize {
                                        IndexOp::Single(idx - 1) // Convert to 0-based
                                    } else {
                                        return None;
                                    }
                                } else {
                                    return None;
                                }
                            } else {
                                // Check for dimension name (A2A reference)
                                let is_dim_name = config
                                    .all_dimensions
                                    .iter()
                                    .any(|d| canonicalize(d.name()).as_str() == ident.as_str());

                                if is_dim_name {
                                    let active_dims = config.active_dimension?;
                                    let active_idx = active_dims.iter().position(|ad| {
                                        canonicalize(ad.name()).as_str() == ident.as_str()
                                    })?;
                                    IndexOp::ActiveDimRef(active_idx)
                                } else {
                                    return None;
                                }
                            }
                        } else {
                            // Not an element - check if it's a dimension name (A2A reference)
                            let is_dim_name = config
                                .all_dimensions
                                .iter()
                                .any(|d| canonicalize(d.name()).as_str() == ident.as_str());

                            if is_dim_name {
                                // It's a dimension name - find matching active dimension
                                let active_dims = config.active_dimension?;
                                let active_idx = active_dims.iter().position(|ad| {
                                    canonicalize(ad.name()).as_str() == ident.as_str()
                                })?;
                                IndexOp::ActiveDimRef(active_idx)
                            } else {
                                // Not a known element or dimension - need dynamic handling
                                return None;
                            }
                        }
                    }
                    _ => return None,
                }
            }

            IndexExpr3::Dimension(name, _) => {
                // First check if the name matches an element of the parent dimension.
                // An element name that happens to match a dimension name should be
                // resolved as an element, not as an A2A dimension reference.
                if let Dimension::Named(_, named_dim) = parent_dim
                    && let Some(idx) = named_dim
                        .elements
                        .iter()
                        .position(|elem| elem.as_str() == name.as_str())
                {
                    operations.push(IndexOp::Single(idx));
                    continue;
                }

                // A2A dimension reference - need to find matching active dimension
                let active_dims = config.active_dimension?;
                let active_idx = active_dims
                    .iter()
                    .position(|ad| canonicalize(ad.name()).as_str() == name.as_str())?;
                IndexOp::ActiveDimRef(active_idx)
            }
        };

        operations.push(op);
    }

    Some(operations)
}

/// Build an ArrayView from normalized IndexOp operations.
///
/// Returns the view, dimension mapping, and single_indices needed for
/// A2A element index computation and range/sparse semantics.
fn build_view_from_ops(
    operations: &[IndexOp],
    orig_dims: &[usize],
    orig_strides: &[isize],
    config: &ViewBuildConfig,
) -> Result<ViewBuildResult> {
    let mut dim_mapping: Vec<Option<usize>> = Vec::new();
    let mut single_indices: Vec<usize> = Vec::new();
    let mut offset_adjustment = 0usize;

    // First pass: determine dimension mapping and validate
    for (i, op) in operations.iter().enumerate() {
        match op {
            IndexOp::Single(idx) => {
                // Validate bounds
                if *idx >= orig_dims[i] {
                    return sim_err!(Generic, format!("Index out of bounds for dimension {}", i));
                }
                single_indices.push(*idx);
                offset_adjustment += idx * orig_strides[i] as usize;
            }
            IndexOp::Range(start, end) => {
                // Validate bounds
                if *end > orig_dims[i] || *start >= *end {
                    return sim_err!(Generic, format!("Invalid range bounds for dimension {}", i));
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
            IndexOp::SparseRange(parent_offsets) => {
                // Validate all parent offsets are in bounds
                for &off in parent_offsets {
                    if off >= orig_dims[i] {
                        return sim_err!(
                            Generic,
                            format!("Sparse range offset out of bounds for dimension {}", i)
                        );
                    }
                }
                dim_mapping.push(Some(i));
                single_indices.push(0); // No static offset for sparse dimensions
            }
            IndexOp::ActiveDimRef(active_idx) => {
                // Reference to active A2A dimension - resolve to concrete offset
                let active_subscripts = config.active_subscript.ok_or_else(|| {
                    Error::new(
                        ErrorKind::Model,
                        ErrorCode::Generic,
                        Some("ActiveDimRef without active subscript context".to_string()),
                    )
                })?;
                let subscript = &active_subscripts[*active_idx];
                let dim = &config.dims[i];

                if let Some(offset) = dim.get_offset(subscript) {
                    single_indices.push(offset);
                    offset_adjustment += offset * orig_strides[i] as usize;
                } else {
                    return sim_err!(
                        Generic,
                        format!(
                            "Invalid active subscript '{}' for dimension {}",
                            subscript.as_str(),
                            i
                        )
                    );
                }
            }
        }
    }

    // Second pass: build the resulting view
    let mut new_dims = Vec::new();
    let mut new_strides = Vec::new();
    let mut new_dim_names = Vec::new();
    let mut sparse_info = Vec::new();
    let mut output_dim_idx = 0usize;

    for (i, op) in operations.iter().enumerate() {
        match op {
            IndexOp::Single(_) => {
                // Dimension is removed, don't add to output
            }
            IndexOp::Range(start, end) => {
                new_dims.push(end - start);
                new_strides.push(orig_strides[i]);
                // Preserve dimension name from input dimension
                if i < config.dims.len() {
                    new_dim_names.push(config.dims[i].name().to_string());
                } else {
                    new_dim_names.push(String::new());
                }
                output_dim_idx += 1;
            }
            IndexOp::Wildcard => {
                new_dims.push(orig_dims[i]);
                new_strides.push(orig_strides[i]);
                // Preserve dimension name from input dimension
                if i < config.dims.len() {
                    new_dim_names.push(config.dims[i].name().to_string());
                } else {
                    new_dim_names.push(String::new());
                }
                output_dim_idx += 1;
            }
            IndexOp::DimPosition(pos) => {
                // Use the dimension size and stride from the referenced position
                new_dims.push(orig_dims[*pos]);
                new_strides.push(orig_strides[*pos]);
                // Use dimension name from the referenced position
                if *pos < config.dims.len() {
                    new_dim_names.push(config.dims[*pos].name().to_string());
                } else {
                    new_dim_names.push(String::new());
                }
                output_dim_idx += 1;
            }
            IndexOp::SparseRange(parent_offsets) => {
                // Dimension size is the number of sparse elements
                new_dims.push(parent_offsets.len());
                new_strides.push(orig_strides[i]);
                sparse_info.push(SparseInfo {
                    dim_index: output_dim_idx,
                    parent_offsets: parent_offsets.clone(),
                });
                // For sparse ranges (subdimensions), use the subdimension name
                // TODO: This should ideally use the subdimension name, not parent
                if i < config.dims.len() {
                    new_dim_names.push(config.dims[i].name().to_string());
                } else {
                    new_dim_names.push(String::new());
                }
                output_dim_idx += 1;
            }
            IndexOp::ActiveDimRef(_) => {
                // Dimension is consumed (resolved to active subscript), don't add to output
            }
        }
    }

    Ok(ViewBuildResult {
        view: ArrayView {
            dims: new_dims,
            strides: new_strides,
            offset: offset_adjustment,
            sparse: sparse_info,
            dim_names: new_dim_names,
        },
        dim_mapping,
        single_indices,
    })
}

/// Represents a single subscript index in a dynamic Subscript expression.
/// This enum distinguishes between single-element access and range access,
/// enabling proper bytecode generation for dynamic ranges.
#[derive(PartialEq, Clone, Debug)]
pub enum SubscriptIndex {
    /// Single element access - evaluates to a 1-based index
    Single(Expr),
    /// Range access - start and end expressions (1-based, inclusive)
    /// Used for dynamic ranges like arr[start:end] where bounds are variables
    Range(Expr, Expr),
}

impl SubscriptIndex {
    #[cfg(test)]
    pub(crate) fn strip_loc(self) -> Self {
        match self {
            SubscriptIndex::Single(expr) => SubscriptIndex::Single(expr.strip_loc()),
            SubscriptIndex::Range(start, end) => {
                SubscriptIndex::Range(start.strip_loc(), end.strip_loc())
            }
        }
    }
}

#[derive(PartialEq, Clone, Debug)]
#[allow(dead_code)]
pub enum Expr {
    Const(f64, Loc),
    Var(usize, Loc), // offset
    /// Dynamic subscript with possible range indices
    /// (offset, subscript indices, dimension sizes, location)
    Subscript(usize, Vec<SubscriptIndex>, Vec<usize>, Loc),
    StaticSubscript(usize, ArrayView, Loc), // offset, precomputed view, location
    TempArray(u32, ArrayView, Loc),         // temp id, view into temp array, location
    TempArrayElement(u32, ArrayView, usize, Loc), // temp id, view, element index, location
    Dt(Loc),
    App(BuiltinFn, Loc),
    /// EvalModule(module_ident, model_name, input_set, args)
    /// input_set is needed to look up the correct compiled module when a model has multiple instantiations
    EvalModule(
        Ident<Canonical>,
        Ident<Canonical>,
        BTreeSet<Ident<Canonical>>,
        Vec<Expr>,
    ),
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
            Expr::EvalModule(_, _, _, _) => Loc::default(),
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
                    BuiltinFn::LookupForward(id, a, _loc) => {
                        BuiltinFn::LookupForward(id, Box::new(a.strip_loc()), loc)
                    }
                    BuiltinFn::LookupBackward(id, a, _loc) => {
                        BuiltinFn::LookupBackward(id, Box::new(a.strip_loc()), loc)
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
            Expr::EvalModule(id1, id2, input_set, args) => {
                let args = args.into_iter().map(|expr| expr.strip_loc()).collect();
                Expr::EvalModule(id1, id2, input_set, args)
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
    #[allow(dead_code)]
    pub(crate) dimensions_ctx: &'a DimensionsContext,
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
    /// When true, wildcards should always be preserved for iteration (inside SUM, etc.)
    /// rather than being collapsed based on active_dimension matching.
    pub(crate) preserve_wildcards_for_iteration: bool,
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

        // Track which active dimensions have been used
        let mut used: Vec<bool> = vec![false; active_dims.len()];

        for dim in dims.iter() {
            // FIRST PASS: Try to find an exact name match anywhere in unused active dims.
            // This prevents size-based fallback from grabbing the wrong dimension when
            // the correct name match exists later in the list.
            let name_match_idx = active_dims.iter().enumerate().find_map(|(i, candidate)| {
                if !used[i] && candidate.name() == dim.name() {
                    Some(i)
                } else {
                    None
                }
            });

            if let Some(idx) = name_match_idx {
                subscripts.push(active_subscripts[idx].as_str());
                used[idx] = true;
                continue;
            }

            // SECOND PASS: Check for dimension mapping matches.
            // If dim.maps_to matches an active dimension (or the active dimension is
            // a subdimension of maps_to), we can match them.
            let maps_to = self.dimensions_ctx.get_maps_to(dim.canonical_name());
            let mapping_match_idx = if let Some(maps_to_dim) = maps_to {
                active_dims.iter().enumerate().find_map(|(i, candidate)| {
                    if used[i] {
                        return None;
                    }
                    let candidate_name = candidate.canonical_name();
                    // Direct mapping: dim maps to this active dimension
                    if candidate_name == maps_to_dim {
                        return Some(i);
                    }
                    // Subdimension mapping: active_dim is a subdimension of maps_to
                    if self
                        .dimensions_ctx
                        .is_subdimension_of(candidate_name, maps_to_dim)
                    {
                        return Some(i);
                    }
                    None
                })
            } else {
                None
            };

            if let Some(idx) = mapping_match_idx {
                subscripts.push(active_subscripts[idx].as_str());
                used[idx] = true;
                continue;
            }

            // THIRD PASS: Only if no name or mapping match exists, try size-based matching
            // for indexed dimensions. Find the first unused indexed dimension with
            // the same size.
            //
            // IMPORTANT: Size-based fallback only applies when BOTH dimensions are
            // indexed. Named dimensions must match by name (or subdimension relationship)
            // because their elements have semantic meaning. For example, Cities=[Boston,
            // Seattle] and Products=[Widgets,Gadgets] shouldn't match just because both
            // have size 2 - that would be semantically incorrect.
            //
            // NOTE: The two-pass (name → size) matching logic is shared with the VM via
            // dimensions::match_dimensions_two_pass. This compiler version adds a mapping
            // pass between name and size matching.
            let size_match_idx = if let Dimension::Indexed(_, dim_size) = dim {
                active_dims.iter().enumerate().find_map(|(i, candidate)| {
                    if !used[i]
                        && let Dimension::Indexed(_, candidate_size) = candidate
                        && dim_size == candidate_size
                    {
                        return Some(i);
                    }
                    None
                })
            } else {
                None
            };

            if let Some(idx) = size_match_idx {
                subscripts.push(active_subscripts[idx].as_str());
                used[idx] = true;
                continue;
            }

            // No match found
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

    /// Convert a dimension + subscript to its 1-based index value.
    /// For indexed dimensions (Dim(5)), the subscript is a numeric string like "3".
    /// For named dimensions (Cities{A,B,C}), the subscript is an element name like "B",
    /// and we return its position + 1.
    fn subscript_to_index(dim: &Dimension, subscript: &CanonicalElementName) -> f64 {
        match dim {
            Dimension::Indexed(_, _) => {
                // For indexed dimensions, the subscript is already a 1-based index
                // stored as a string (e.g., "3" means the third element).
                subscript.as_str().parse::<f64>().unwrap_or(1.0)
            }
            Dimension::Named(_, named_dim) => {
                // For named dimensions, find the element's position (0-based) and add 1
                named_dim
                    .elements
                    .iter()
                    .position(|elem| elem.as_str() == subscript.as_str())
                    .map(|off| (off + 1) as f64)
                    .unwrap_or(1.0)
            }
        }
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

    /// Pass 0: Structural lowering - expands bare array variable references.
    ///
    /// Transforms `Expr2::Var` with ArrayBounds into `Expr2::Subscript` with
    /// dimension name subscripts. This ensures:
    /// 1. Subsequent phases can treat all Var nodes as scalars
    /// 2. Dimension bindings are explicit for A2A processing
    /// 3. Dimension reordering works correctly
    fn lower_pass0(&self, expr: &ast::Expr2) -> ast::Expr2 {
        match expr {
            ast::Expr2::Var(id, Some(bounds), loc) => {
                // Expand bare array variable to Subscript with dimension name subscripts
                let subscripts = self.make_dimension_subscripts(id, bounds, *loc);
                let subscript_bounds = self.make_subscript_bounds(id, bounds, &subscripts);
                ast::Expr2::Subscript(id.clone(), subscripts, subscript_bounds, *loc)
            }
            ast::Expr2::Var(_, None, _) => expr.clone(), // Scalar - unchanged
            ast::Expr2::Const(_, _, _) => expr.clone(),
            ast::Expr2::Subscript(id, args, bounds, loc) => {
                // Recursively process expressions inside subscripts
                let new_args: Vec<ast::IndexExpr2> = args
                    .iter()
                    .map(|arg| self.lower_pass0_index_expr(arg))
                    .collect();
                ast::Expr2::Subscript(id.clone(), new_args, bounds.clone(), *loc)
            }
            ast::Expr2::Op1(op, inner, bounds, loc) => {
                ast::Expr2::Op1(*op, Box::new(self.lower_pass0(inner)), bounds.clone(), *loc)
            }
            ast::Expr2::Op2(op, left, right, bounds, loc) => ast::Expr2::Op2(
                *op,
                Box::new(self.lower_pass0(left)),
                Box::new(self.lower_pass0(right)),
                bounds.clone(),
                *loc,
            ),
            ast::Expr2::If(cond, then_branch, else_branch, bounds, loc) => ast::Expr2::If(
                Box::new(self.lower_pass0(cond)),
                Box::new(self.lower_pass0(then_branch)),
                Box::new(self.lower_pass0(else_branch)),
                bounds.clone(),
                *loc,
            ),
            ast::Expr2::App(builtin, bounds, loc) => {
                let new_builtin = self.lower_pass0_builtin(builtin);
                ast::Expr2::App(new_builtin, bounds.clone(), *loc)
            }
        }
    }

    /// Create dimension name subscripts from ArrayBounds.
    ///
    /// For each dimension in bounds:
    /// - If the dimension is in the active set, use a dimension name subscript
    ///   (creates proper A2A binding via ActiveDimRef)
    /// - If the dimension is NOT in the active set, use a wildcard
    ///   (needed for reductions like SUM where we iterate over non-active dims)
    ///
    /// This handles:
    /// - Full A2A: result[A,B] = source where source is [A,B] -> source[A,B]
    /// - Partial reduction: result[A] = SUM(source) where source is [A,B] -> SUM(source[A,*])
    /// - Full reduction: total = SUM(source) where source is [A,B] -> SUM(source[*,*])
    fn make_dimension_subscripts(
        &self,
        ident: &Ident<Canonical>,
        bounds: &ast::ArrayBounds,
        loc: Loc,
    ) -> Vec<ast::IndexExpr2> {
        // Get the source dimensions (from metadata or bounds)
        let source_dims: Option<Vec<Dimension>> = self
            .get_metadata(ident)
            .ok()
            .and_then(|metadata| metadata.var.get_dimensions())
            .map(|dims| dims.to_vec());

        let Some(source_dims) = source_dims else {
            return bounds
                .dims()
                .iter()
                .map(|_| ast::IndexExpr2::Wildcard(loc))
                .collect();
        };

        // If we have active dimensions, use the unified dimension matching algorithm
        let Some(active_dims) = self.active_dimension.as_ref() else {
            // No active dimensions (not in A2A context) - use wildcards
            return source_dims
                .iter()
                .map(|_| ast::IndexExpr2::Wildcard(loc))
                .collect();
        };

        // Use two-pass matching to ensure name matches are reserved before size matches.
        // This is critical for correct dimension reordering when same-sized indexed dims exist.
        //
        // Pass 1: Assign all exact name matches (reserve them)
        // Pass 2: For remaining sources, try size-based matching (indexed dims only)
        //
        // Use partial matching (not all-or-nothing) to support reductions like SUM(source[A,B])
        // in context [A] where B doesn't match anything.
        let source_to_target = match_dimensions_two_pass_partial(
            &source_dims,
            active_dims,
            &vec![false; active_dims.len()],
        );

        source_dims
            .iter()
            .enumerate()
            .map(|(source_idx, _source_dim)| {
                if let Some(target_idx) = source_to_target[source_idx] {
                    let active_dim = &active_dims[target_idx];
                    // Create a dimension reference to the matched active dimension
                    ast::IndexExpr2::Expr(ast::Expr2::Var(
                        canonicalize(active_dim.name()),
                        None,
                        loc,
                    ))
                } else {
                    // Source dimension didn't match any active dimension - use wildcard
                    // (needed for reductions like SUM where we iterate over non-matched dims)
                    ast::IndexExpr2::Wildcard(loc)
                }
            })
            .collect()
    }

    fn make_subscript_bounds(
        &self,
        ident: &Ident<Canonical>,
        bounds: &ast::ArrayBounds,
        subscripts: &[ast::IndexExpr2],
    ) -> Option<ast::ArrayBounds> {
        let dims = self
            .get_metadata(ident)
            .ok()
            .and_then(|metadata| metadata.var.get_dimensions())?;

        let mut result_dims = Vec::new();
        let mut result_dim_names = Vec::new();

        for (i, subscript) in subscripts.iter().enumerate() {
            match subscript {
                ast::IndexExpr2::Wildcard(_) | ast::IndexExpr2::Range(_, _, _) => {
                    result_dims.push(dims[i].len());
                    result_dim_names.push(dims[i].name().to_string());
                }
                ast::IndexExpr2::StarRange(subdim_name, _) => {
                    let len = self
                        .dimensions_ctx
                        .get(subdim_name)
                        .map(|dim| dim.len())
                        .unwrap_or_else(|| dims[i].len());
                    result_dims.push(len);
                    result_dim_names.push(subdim_name.as_str().to_string());
                }
                ast::IndexExpr2::Expr(_) | ast::IndexExpr2::DimPosition(_, _) => {}
            }
        }

        if result_dims.is_empty() {
            return None;
        }

        let dim_names = Some(result_dim_names);
        match bounds {
            ast::ArrayBounds::Named { name, .. } => Some(ast::ArrayBounds::Named {
                name: name.clone(),
                dims: result_dims,
                dim_names,
            }),
            ast::ArrayBounds::Temp { id, .. } => Some(ast::ArrayBounds::Temp {
                id: *id,
                dims: result_dims,
                dim_names,
            }),
        }
    }

    /// Recursively process index expressions
    fn lower_pass0_index_expr(&self, expr: &ast::IndexExpr2) -> ast::IndexExpr2 {
        match expr {
            ast::IndexExpr2::Expr(inner) => ast::IndexExpr2::Expr(self.lower_pass0(inner)),
            ast::IndexExpr2::Range(start, end, loc) => {
                ast::IndexExpr2::Range(self.lower_pass0(start), self.lower_pass0(end), *loc)
            }
            // Wildcard, StarRange, DimPosition remain unchanged
            ast::IndexExpr2::Wildcard(_)
            | ast::IndexExpr2::StarRange(_, _)
            | ast::IndexExpr2::DimPosition(_, _) => expr.clone(),
        }
    }

    /// Recursively process builtin function arguments
    fn lower_pass0_builtin(
        &self,
        builtin: &crate::builtins::BuiltinFn<ast::Expr2>,
    ) -> crate::builtins::BuiltinFn<ast::Expr2> {
        use crate::builtins::BuiltinFn::*;
        match builtin {
            // Single expression argument
            Abs(e) => Abs(Box::new(self.lower_pass0(e))),
            Arccos(e) => Arccos(Box::new(self.lower_pass0(e))),
            Arcsin(e) => Arcsin(Box::new(self.lower_pass0(e))),
            Arctan(e) => Arctan(Box::new(self.lower_pass0(e))),
            Cos(e) => Cos(Box::new(self.lower_pass0(e))),
            Exp(e) => Exp(Box::new(self.lower_pass0(e))),
            Int(e) => Int(Box::new(self.lower_pass0(e))),
            Ln(e) => Ln(Box::new(self.lower_pass0(e))),
            Log10(e) => Log10(Box::new(self.lower_pass0(e))),
            Sign(e) => Sign(Box::new(self.lower_pass0(e))),
            Sin(e) => Sin(Box::new(self.lower_pass0(e))),
            Sqrt(e) => Sqrt(Box::new(self.lower_pass0(e))),
            Tan(e) => Tan(Box::new(self.lower_pass0(e))),

            // Array builtins with single expression
            Size(e) => Size(Box::new(self.lower_pass0(e))),
            Stddev(e) => Stddev(Box::new(self.lower_pass0(e))),
            Sum(e) => Sum(Box::new(self.lower_pass0(e))),

            // Two expression arguments with optional second
            Max(a, b) => Max(
                Box::new(self.lower_pass0(a)),
                b.as_ref().map(|e| Box::new(self.lower_pass0(e))),
            ),
            Min(a, b) => Min(
                Box::new(self.lower_pass0(a)),
                b.as_ref().map(|e| Box::new(self.lower_pass0(e))),
            ),

            // Two required expression arguments
            Step(a, b) => Step(Box::new(self.lower_pass0(a)), Box::new(self.lower_pass0(b))),

            // Three expression arguments (last optional)
            Pulse(a, b, c) => Pulse(
                Box::new(self.lower_pass0(a)),
                Box::new(self.lower_pass0(b)),
                c.as_ref().map(|e| Box::new(self.lower_pass0(e))),
            ),
            Ramp(a, b, c) => Ramp(
                Box::new(self.lower_pass0(a)),
                Box::new(self.lower_pass0(b)),
                c.as_ref().map(|e| Box::new(self.lower_pass0(e))),
            ),
            SafeDiv(a, b, c) => SafeDiv(
                Box::new(self.lower_pass0(a)),
                Box::new(self.lower_pass0(b)),
                c.as_ref().map(|e| Box::new(self.lower_pass0(e))),
            ),

            // Vec of expressions
            Mean(exprs) => Mean(exprs.iter().map(|e| self.lower_pass0(e)).collect()),

            // Lookup with string table name + expression
            Lookup(name, e, loc) => Lookup(name.clone(), Box::new(self.lower_pass0(e)), *loc),
            LookupForward(name, e, loc) => {
                LookupForward(name.clone(), Box::new(self.lower_pass0(e)), *loc)
            }
            LookupBackward(name, e, loc) => {
                LookupBackward(name.clone(), Box::new(self.lower_pass0(e)), *loc)
            }

            // Rank with complex signature
            Rank(e, maybe_tuple) => Rank(
                Box::new(self.lower_pass0(e)),
                maybe_tuple.as_ref().map(|(a, b)| {
                    (
                        Box::new(self.lower_pass0(a)),
                        b.as_ref().map(|e| Box::new(self.lower_pass0(e))),
                    )
                }),
            ),

            // 0-arity builtins (no expressions to transform)
            Inf => Inf,
            Pi => Pi,
            Time => Time,
            TimeStep => TimeStep,
            StartTime => StartTime,
            FinalTime => FinalTime,

            // IsModuleInput has string + loc, no Expr
            IsModuleInput(name, loc) => IsModuleInput(name.clone(), *loc),
        }
    }

    /// Entry point for lowering Expr2 to compiler's Expr representation.
    /// Applies pass 0 → Expr3 → pass 1 → lower_from_expr3.
    /// Returns a Vec<Expr> where the first elements are temp assignments
    /// and the last element is the main expression.
    ///
    /// When A2A context is available (active_dimension and active_subscript set),
    /// pass 1 can resolve Dimension and DimPosition references to concrete indices,
    /// enabling decomposition of expressions that would otherwise be deferred.
    fn lower(&self, expr: &ast::Expr2) -> Result<Vec<Expr>> {
        // Pass 0: normalize bare arrays, subscripts
        let normalized = self.lower_pass0(expr);

        // Convert to Expr3 (wildcard resolution, dimension detection)
        let expr3 = Expr3::from_expr2(&normalized, self).map_err(|e| Error {
            kind: ErrorKind::Model,
            code: e.code,
            details: Some(format!("Error at {}:{}", e.start, e.end)),
        })?;

        // Pass 1: temp decomposition for complex array expressions
        // Use A2A context when available to resolve dimension references
        let mut pass1_ctx = match (&self.active_dimension, &self.active_subscript) {
            (Some(dims), Some(subs)) => Pass1Context::with_a2a_context(dims, subs),
            _ => Pass1Context::new(),
        };
        let transformed = pass1_ctx.transform(expr3);
        let assignments = pass1_ctx.take_assignments();

        // Lower the assignments
        let mut result: Vec<Expr> = assignments
            .iter()
            .map(|a| self.lower_from_expr3(a))
            .collect::<Result<Vec<_>>>()?;

        // Lower the main expression
        let main_expr = self.lower_from_expr3(&transformed)?;
        result.push(main_expr);

        Ok(result)
    }

    fn fold_flows(&self, flows: &[Ident<Canonical>]) -> Result<Option<Expr>> {
        if flows.is_empty() {
            return Ok(None);
        }

        let loads: Result<Vec<Expr>> = flows
            .iter()
            .map(|flow| {
                self.get_offset(flow)
                    .map(|off| Expr::Var(off, Loc::default()))
            })
            .collect();
        let mut loads = loads?.into_iter();

        let first = loads.next().unwrap();
        Ok(Some(loads.fold(first, |acc, flow| {
            Expr::Op2(BinaryOp::Add, Box::new(acc), Box::new(flow), Loc::default())
        })))
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
                    let orig_dim_names: Vec<String> =
                        dims.iter().map(|d| d.name().to_string()).collect();

                    // Create a contiguous view with names and apply reordering
                    let view = ArrayView::contiguous_with_names(orig_dims, orig_dim_names);
                    return Ok(Expr::StaticSubscript(
                        *off,
                        view.reorder_dimensions(&reordering),
                        loc,
                    ));
                }
            }
            Expr::StaticSubscript(off, view, _) => {
                // Apply reordering to existing view
                return Ok(Expr::StaticSubscript(
                    *off,
                    view.reorder_dimensions(&reordering),
                    loc,
                ));
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

    fn build_stock_update_expr(&self, stock_off: usize, var: &Variable) -> Result<Expr> {
        if let Variable::Stock {
            inflows, outflows, ..
        } = var
        {
            let inflows = self
                .fold_flows(inflows)?
                .unwrap_or(Expr::Const(0.0, Loc::default()));
            let outflows = self
                .fold_flows(outflows)?
                .unwrap_or(Expr::Const(0.0, Loc::default()));

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

            Ok(Expr::Op2(
                BinaryOp::Add,
                Box::new(Expr::Var(stock_off, Loc::default())),
                Box::new(dt_update),
                Loc::default(),
            ))
        } else {
            unreachable!(
                "build_stock_update_expr called with non-stock {}",
                var.ident()
            );
        }
    }
}

// Implement Expr3LowerContext for Context to enable Expr2 -> Expr3 conversion
impl Expr3LowerContext for Context<'_> {
    fn get_dimensions(&self, ident: &str) -> Option<Vec<Dimension>> {
        let metadata = self.metadata.get(self.model_name)?;
        let var_metadata = metadata.get(&canonicalize(ident))?;
        var_metadata.var.get_dimensions().map(|dims| dims.to_vec())
    }

    fn is_dimension_name(&self, ident: &str) -> bool {
        let canonical = canonicalize(ident);
        self.dimensions
            .iter()
            .any(|dim| canonicalize(dim.name()).as_str() == canonical.as_str())
    }
}

/// Result of applying pass 1 to an expression.
/// Contains the transformed expression and any temp assignments that must be
/// evaluated before the main expression.
#[allow(dead_code)]
pub struct Pass1Result {
    /// Temp assignments in order of dependency (first should be evaluated first)
    pub assignments: Vec<Expr>,
    /// The main expression (references temps via TempArray)
    pub expr: Expr,
}

impl Context<'_> {
    /// Create a context with transposed active dimensions for transpose operations.
    /// Used when processing expressions under a Transpose operator in A2A context.
    fn with_transposed_active_context(&self) -> Self {
        let mut ctx = self.clone();
        if let Some(ref active_dims) = ctx.active_dimension {
            let mut reversed = active_dims.clone();
            reversed.reverse();
            ctx.active_dimension = Some(reversed);
        }
        if let Some(ref active_subs) = ctx.active_subscript {
            let mut reversed = active_subs.clone();
            reversed.reverse();
            ctx.active_subscript = Some(reversed);
        }
        ctx
    }

    /// Create a context that preserves wildcards for array iteration.
    /// Used for array reduction builtins (SUM, MAX, MIN, MEAN, STDDEV, SIZE).
    fn with_preserved_wildcards(&self) -> Self {
        let mut ctx = self.clone();
        ctx.preserve_wildcards_for_iteration = true;
        ctx
    }

    /// Lower an Expr3 to compiler's Expr representation.
    /// Handles all Expr3 variants directly, including pass-1 specific variants
    /// (TempArray, AssignTemp, etc.) and common expression types.
    fn lower_from_expr3(&self, expr: &Expr3) -> Result<Expr> {
        match expr {
            // Handle Expr3-specific variants directly
            Expr3::StaticSubscript(id, view, _, loc) => {
                let off = self.get_base_offset(id)?;
                Ok(Expr::StaticSubscript(off, view.clone(), *loc))
            }

            Expr3::TempArray(id, view, loc) => Ok(Expr::TempArray(*id, view.clone(), *loc)),

            Expr3::TempArrayElement(id, view, idx, loc) => {
                Ok(Expr::TempArrayElement(*id, view.clone(), *idx, *loc))
            }

            Expr3::AssignTemp(id, inner, view) => {
                let lowered_inner = self.lower_from_expr3(inner)?;
                Ok(Expr::AssignTemp(*id, Box::new(lowered_inner), view.clone()))
            }

            // Handle common variants directly (no longer converting to Expr2)
            Expr3::Const(_, n, loc) => Ok(Expr::Const(*n, *loc)),

            Expr3::Var(id, _, loc) => {
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
                                    let index = Self::subscript_to_index(dim, subscript);
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
                    return Ok(Expr::ModuleInput(off, *loc));
                }

                // Check if it's a regular variable
                match self.get_offset(id) {
                    Ok(off) => Ok(Expr::Var(off, *loc)),
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
                                if let Some(reordering) =
                                    find_dimension_reordering(&source_dim_names, &target_dim_names)
                                {
                                    // Check if reordering is needed (not identity)
                                    let needs_reordering =
                                        reordering.iter().enumerate().any(|(i, &idx)| i != idx);

                                    if needs_reordering {
                                        // Create a transposed view
                                        let orig_dims: Vec<usize> =
                                            source_dims.iter().map(|d| d.len()).collect();

                                        // Reorder the dimensions
                                        let reordered_dims: Vec<usize> = target_dims
                                            .iter()
                                            .map(|target_dim| {
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
                                            strides[i] = strides[i + 1] * orig_dims[i + 1] as isize;
                                        }

                                        // Reorder the strides according to the dimension reordering
                                        let reordered_strides: Vec<isize> =
                                            reordering.iter().map(|&idx| strides[idx]).collect();

                                        let view = ArrayView {
                                            dims: reordered_dims,
                                            strides: reordered_strides,
                                            offset: 0,
                                            sparse: Vec::new(),
                                            dim_names: target_dim_names.clone(),
                                        };

                                        return Ok(Expr::StaticSubscript(off, view, *loc));
                                    }
                                }
                            }

                            // No reordering needed or not in A2A context
                            let orig_dims: Vec<usize> =
                                source_dims.iter().map(|d| d.len()).collect();
                            let dim_names: Vec<String> =
                                source_dims.iter().map(|d| d.name().to_string()).collect();
                            let view = ArrayView::contiguous_with_names(orig_dims, dim_names);
                            return Ok(Expr::StaticSubscript(off, view, *loc));
                        }
                        Err(err)
                    }
                }
            }

            Expr3::Subscript(id, indices, _bounds, loc) => {
                // Handle subscript directly without converting to Expr2
                let off = self.get_base_offset(id)?;
                let metadata = self.get_metadata(id)?;
                let dims = metadata.var.get_dimensions().ok_or_else(|| {
                    Error::new(
                        ErrorKind::Model,
                        ErrorCode::Generic,
                        Some(format!(
                            "expected array variable '{}' to have dimensions",
                            id.as_str()
                        )),
                    )
                })?;

                if indices.len() != dims.len() {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }

                // Validate no array-valued subscript expressions
                for idx in indices {
                    if let IndexExpr3::Expr(expr) = idx
                        && expr.get_array_bounds().is_some()
                    {
                        return sim_err!(
                            Generic,
                            format!("array-valued subscript expression for '{}'", id.as_str())
                        );
                    }
                }

                // Try to normalize subscripts to static operations
                let config = Subscript3Config {
                    dims,
                    all_dimensions: &self.dimensions,
                    dimensions_ctx: self.dimensions_ctx,
                    active_dimension: self.active_dimension.as_deref(),
                };

                if let Some(operations) = normalize_subscripts3(indices, &config) {
                    // Build a unified view for any combination of static operations
                    let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();

                    // Calculate original strides (row-major)
                    let mut orig_strides = vec![1isize; orig_dims.len()];
                    for i in (0..orig_dims.len().saturating_sub(1)).rev() {
                        orig_strides[i] = orig_strides[i + 1] * orig_dims[i + 1] as isize;
                    }

                    // Build the view using the helper
                    let view_config = ViewBuildConfig {
                        active_subscript: self.active_subscript.as_deref(),
                        dims,
                    };
                    let ViewBuildResult {
                        view,
                        dim_mapping,
                        single_indices,
                    } = build_view_from_ops(&operations, &orig_dims, &orig_strides, &view_config)?;

                    // Check if we're in an array iteration context
                    if let Some(active_subscripts) = &self.active_subscript
                        && let Some(active_dims) = &self.active_dimension
                    {
                        // Check if we have any dimension positions
                        let has_dim_positions = operations
                            .iter()
                            .any(|op| matches!(op, IndexOp::DimPosition(_)));

                        // Check for operations that preserve dimensions for iteration
                        let has_iteration_preserving_ops = operations.iter().any(|op| {
                            matches!(
                                op,
                                IndexOp::Wildcard | IndexOp::SparseRange(_) | IndexOp::Range(_, _)
                            )
                        });

                        let preserve_for_iteration =
                            self.preserve_wildcards_for_iteration && has_iteration_preserving_ops;

                        if has_dim_positions {
                            // Fall through to dynamic handling at the end
                        } else if preserve_for_iteration {
                            return Ok(Expr::StaticSubscript(off, view, *loc));
                        } else {
                            if view.dims.is_empty() {
                                return Ok(Expr::Var(off + view.offset, *loc));
                            }

                            // For broadcasting: source array may have fewer dimensions than output.
                            // Try to match dimensions by name. If name-based matching fails or isn't
                            // applicable, fall back to positional matching.
                            //
                            // Build a map from dimension name to (active_idx, subscript)
                            let active_dim_map: std::collections::HashMap<
                                &str,
                                (usize, &CanonicalElementName),
                            > = active_dims
                                .iter()
                                .zip(active_subscripts.iter())
                                .enumerate()
                                .map(|(idx, (dim, sub))| (dim.name(), (idx, sub)))
                                .collect();

                            // Determine matching mode for each view dimension:
                            // - Name-based: view dim name matches an active dim name (broadcasting)
                            // - Mapping-based: view dim maps_to matches active dim or its parent
                            // - Positional: view dim name is empty or doesn't match any active dim
                            //
                            // Broadcasting is allowed when source has fewer dimensions than output,
                            // and all source dimensions match some output dimension by name/mapping.
                            // Positional matching requires equal dimension counts.
                            let use_name_matching: Vec<bool> = view
                                .dim_names
                                .iter()
                                .map(|name| {
                                    if name.is_empty() {
                                        return false;
                                    }
                                    // Direct name match
                                    if active_dim_map.contains_key(name.as_str()) {
                                        return true;
                                    }
                                    // Check for dimension mapping match
                                    use crate::common::CanonicalDimensionName;
                                    let source_dim_name =
                                        CanonicalDimensionName::from_raw(name.as_str());
                                    if let Some(maps_to) =
                                        self.dimensions_ctx.get_maps_to(&source_dim_name)
                                    {
                                        // Direct mapping: source.maps_to == active_dim
                                        if active_dim_map.contains_key(maps_to.as_str()) {
                                            return true;
                                        }
                                        // Subdimension mapping: active_dim is subdimension of maps_to
                                        for active_dim_name in active_dim_map.keys() {
                                            let active_canonical =
                                                CanonicalDimensionName::from_raw(active_dim_name);
                                            if self
                                                .dimensions_ctx
                                                .is_subdimension_of(&active_canonical, maps_to)
                                            {
                                                return true;
                                            }
                                        }
                                    }
                                    false
                                })
                                .collect();

                            let all_name_matching = use_name_matching.iter().all(|&b| b);

                            // If all dimensions use name matching, allow broadcasting (fewer dims)
                            // Otherwise, dimension counts must match for positional matching
                            if !all_name_matching && view.dims.len() != active_dims.len() {
                                return sim_err!(MismatchedDimensions, id.as_str().to_string());
                            }

                            // For positional matching, verify sizes match
                            for (view_idx, &view_dim) in view.dims.iter().enumerate() {
                                if !use_name_matching[view_idx] && view_idx < active_dims.len() {
                                    // Positional matching - sizes must match
                                    if view_dim != active_dims[view_idx].len() {
                                        return sim_err!(
                                            MismatchedDimensions,
                                            id.as_str().to_string()
                                        );
                                    }
                                }
                            }

                            // Calculate the linear index in the result array based on the view
                            let mut result_index = 0;

                            // Build map of dim_index -> sparse parent_offsets for quick lookup
                            let sparse_map: std::collections::HashMap<usize, &[usize]> = view
                                .sparse
                                .iter()
                                .map(|s| (s.dim_index, s.parent_offsets.as_slice()))
                                .collect();

                            // For each dimension in the view, find its value from active subscripts
                            for (view_idx, stride) in view.strides.iter().enumerate() {
                                // Find the active dimension and subscript for this view dimension
                                let (active_idx, subscript) = if use_name_matching[view_idx] {
                                    // Name-based matching - could be direct name match or via mapping
                                    let view_dim_name = &view.dim_names[view_idx];

                                    // First try direct name match
                                    if let Some(&(active_idx, subscript)) =
                                        active_dim_map.get(view_dim_name.as_str())
                                    {
                                        (active_idx, subscript)
                                    } else {
                                        // Try mapping-based match: find the active dimension that
                                        // matches via the source dimension's maps_to
                                        use crate::common::CanonicalDimensionName;
                                        let source_dim_name = CanonicalDimensionName::from_raw(
                                            view_dim_name.as_str(),
                                        );
                                        let maps_to =
                                            self.dimensions_ctx.get_maps_to(&source_dim_name);

                                        let mut found = None;
                                        if let Some(maps_to_dim) = maps_to {
                                            // Direct mapping match
                                            if let Some(&(active_idx, subscript)) =
                                                active_dim_map.get(maps_to_dim.as_str())
                                            {
                                                found = Some((active_idx, subscript));
                                            } else {
                                                // Subdimension match: find active dim that is a
                                                // subdimension of maps_to
                                                for (active_dim_name, &(active_idx, subscript)) in
                                                    &active_dim_map
                                                {
                                                    let active_canonical =
                                                        CanonicalDimensionName::from_raw(
                                                            active_dim_name,
                                                        );
                                                    if self.dimensions_ctx.is_subdimension_of(
                                                        &active_canonical,
                                                        maps_to_dim,
                                                    ) {
                                                        found = Some((active_idx, subscript));
                                                        break;
                                                    }
                                                }
                                            }
                                        }

                                        if let Some((active_idx, subscript)) = found {
                                            (active_idx, subscript)
                                        } else {
                                            return sim_err!(
                                                MismatchedDimensions,
                                                id.as_str().to_string()
                                            );
                                        }
                                    }
                                } else {
                                    // Positional matching
                                    (view_idx, &active_subscripts[view_idx])
                                };

                                let dim_idx = if let Some(dim_idx) =
                                    dim_mapping.get(view_idx).and_then(|idx| *idx)
                                {
                                    dim_idx
                                } else {
                                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                                };
                                if dim_idx >= dims.len() {
                                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                                }

                                let source_dim = &dims[dim_idx];
                                let target_dim = &active_dims[active_idx];

                                let is_sparse = sparse_map.contains_key(&view_idx);

                                let prefer_source = source_dim.name() == target_dim.name()
                                    || matches!(source_dim, Dimension::Named(_, _));

                                let mut source_offset = if prefer_source {
                                    source_dim.get_offset(subscript)
                                } else {
                                    None
                                };

                                // If source_offset failed, try dimension mapping.
                                // If source_dim maps to target_dim (or a parent of target_dim),
                                // translate the subscript from target context to source_dim's
                                // corresponding element.
                                let mut mapping_failed = false;
                                if source_offset.is_none() {
                                    let source_dim_name = source_dim.canonical_name();
                                    let target_dim_name = target_dim.canonical_name();

                                    // Check if a mapping exists between these dimensions.
                                    // First try direct mapping: source_dim.maps_to == target_dim
                                    // If that fails, check if target_dim is a subdimension of
                                    // the maps_to dimension (e.g., SubB is a subdimension of DimB,
                                    // and source_dim maps to DimB).
                                    let maps_to = self.dimensions_ctx.get_maps_to(source_dim_name);
                                    let effective_target = if maps_to == Some(target_dim_name) {
                                        // Direct mapping: source_dim maps directly to target_dim
                                        Some(target_dim_name.clone())
                                    } else if let Some(maps_to_dim) = maps_to {
                                        // Check if target_dim is a subdimension of maps_to.
                                        // If so, use maps_to as the effective target for translation.
                                        // The subscript element (e.g., "B2") is valid in both
                                        // target_dim (SubB) and the parent dimension (DimB).
                                        let is_subdim = self
                                            .dimensions_ctx
                                            .is_subdimension_of(target_dim_name, maps_to_dim);
                                        if is_subdim {
                                            Some(maps_to_dim.clone())
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    };

                                    if let Some(effective_target_dim) = effective_target {
                                        if let Some(translated) =
                                            self.dimensions_ctx.translate_to_source_via_mapping(
                                                source_dim_name,
                                                &effective_target_dim,
                                                subscript,
                                            )
                                        {
                                            source_offset = source_dim.get_offset(&translated);
                                        } else {
                                            // Mapping exists but translation failed - this is a
                                            // configuration error (e.g., size mismatch or invalid subscript)
                                            mapping_failed = true;
                                        }
                                    }
                                }

                                // Only try target_dim.get_offset as a fallback if:
                                // 1. source_offset is still None (no direct or mapped resolution)
                                // 2. mapping did NOT fail (mapping_failed is false)
                                //
                                // If a dimension mapping exists but translation failed, we must NOT
                                // fall back to target_dim.get_offset. The mapping is authoritative -
                                // falling back would hide configuration errors (like dimension size
                                // mismatches) and could lead to subtle, hard-to-debug incorrect
                                // array indexing behavior.
                                let target_offset = if source_offset.is_none() && !mapping_failed {
                                    target_dim.get_offset(subscript)
                                } else {
                                    None
                                };

                                let (abs_offset, offset_from_source) = if let Some(abs_offset) =
                                    source_offset
                                {
                                    (abs_offset, true)
                                } else if let Some(abs_offset) = target_offset {
                                    (abs_offset, false)
                                } else if mapping_failed {
                                    // Provide a more specific error when mapping exists but failed
                                    return sim_err!(
                                        MismatchedDimensions,
                                        format!(
                                            "{}: dimension mapping from {} to {} failed for subscript '{}' \
                                             (check that both dimensions have the same number of elements)",
                                            id.as_str(),
                                            source_dim.name(),
                                            target_dim.name(),
                                            subscript.as_str()
                                        )
                                    );
                                } else {
                                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                                };

                                let rel_offset = if is_sparse {
                                    if !offset_from_source {
                                        return sim_err!(
                                            MismatchedDimensions,
                                            id.as_str().to_string()
                                        );
                                    }
                                    abs_offset
                                } else if offset_from_source {
                                    let start_offset = single_indices[dim_idx];
                                    if let Some(rel_offset) = abs_offset.checked_sub(start_offset) {
                                        rel_offset
                                    } else {
                                        return sim_err!(
                                            MismatchedDimensions,
                                            id.as_str().to_string()
                                        );
                                    }
                                } else {
                                    abs_offset
                                };

                                result_index += rel_offset * (*stride as usize);
                            }

                            return Ok(Expr::Var(off + view.offset + result_index, *loc));
                        }

                        if !has_dim_positions {
                            return Ok(Expr::StaticSubscript(off, view, *loc));
                        }
                        // has_dim_positions is true - fall through to dynamic handling
                    } else {
                        // Not in A2A context - return StaticSubscript for the full view
                        return Ok(Expr::StaticSubscript(off, view, *loc));
                    }
                }

                // Fall back to dynamic subscript handling for Expr3
                // This handles cases where normalize_subscripts3 returned None
                let orig_dims: Vec<usize> = dims.iter().map(|d| d.len()).collect();
                let args: Result<Vec<_>> = indices
                    .iter()
                    .enumerate()
                    .map(|(i, arg)| self.lower_index_expr3(arg, id, i, dims, &orig_dims, *loc))
                    .collect();
                Ok(Expr::Subscript(off, args?, orig_dims, *loc))
            }

            Expr3::App(builtin, _bounds, loc) => {
                // Lower builtin directly without converting to Expr2
                let lowered_builtin = self.lower_builtin_expr3(builtin)?;
                Ok(Expr::App(lowered_builtin, *loc))
            }

            Expr3::Op1(op, inner, _bounds, loc) => {
                match op {
                    ast::UnaryOp::Transpose => {
                        // Special handling for transpose of bare array variables
                        if let Expr3::Var(id, _, var_loc) = &**inner {
                            // Get the variable's metadata to check if it's an array
                            if let Ok(metadata) = self.get_metadata(id)
                                && let Some(dims) = metadata.var.get_dimensions()
                            {
                                if self.active_dimension.is_some() {
                                    // We're in an A2A context - need to handle bare array transpose specially
                                    // Process the variable with reversed active dimensions
                                    let result = self
                                        .with_transposed_active_context()
                                        .lower_from_expr3(inner)?;
                                    return Ok(result);
                                } else {
                                    // Not in A2A context - create a wildcard subscript to get the full array
                                    // then apply transpose
                                    let off = self.get_base_offset(id)?;
                                    let orig_dims: Vec<usize> =
                                        dims.iter().map(|d| d.len()).collect();
                                    let orig_dim_names: Vec<String> =
                                        dims.iter().map(|d| d.name().to_string()).collect();
                                    let orig_strides =
                                        ArrayView::contiguous(orig_dims.clone()).strides;

                                    // Create a view for the full array and transpose it
                                    let view = ArrayView {
                                        dims: orig_dims.clone(),
                                        strides: orig_strides,
                                        offset: 0,
                                        sparse: Vec::new(),
                                        dim_names: orig_dim_names,
                                    };

                                    return Ok(Expr::StaticSubscript(
                                        off,
                                        view.transpose(),
                                        *var_loc,
                                    ));
                                }
                            }
                        }

                        // Default transpose handling
                        if self.active_dimension.is_some() {
                            // In A2A context, transpose swaps the active indices
                            self.with_transposed_active_context()
                                .lower_from_expr3(inner)
                        } else {
                            let lowered = self.lower_from_expr3(inner)?;
                            // Transpose reverses the dimensions of an array
                            match lowered {
                                Expr::StaticSubscript(off, view, expr_loc) => {
                                    Ok(Expr::StaticSubscript(off, view.transpose(), expr_loc))
                                }
                                _ => {
                                    // For other expressions, wrap in a transpose operation
                                    Ok(Expr::Op1(UnaryOp::Transpose, Box::new(lowered), *loc))
                                }
                            }
                        }
                    }
                    _ => {
                        // Process the inner expression first for other operators
                        let lowered = self.lower_from_expr3(inner)?;
                        let result = match op {
                            ast::UnaryOp::Negative => Expr::Op2(
                                BinaryOp::Sub,
                                Box::new(Expr::Const(0.0, *loc)),
                                Box::new(lowered),
                                *loc,
                            ),
                            ast::UnaryOp::Positive => lowered,
                            ast::UnaryOp::Not => Expr::Op1(UnaryOp::Not, Box::new(lowered), *loc),
                            ast::UnaryOp::Transpose => unreachable!("Transpose handled above"),
                        };
                        Ok(result)
                    }
                }
            }

            Expr3::Op2(op, left, right, array_bounds, loc) => {
                // Lower both operands
                let mut l_expr = self.lower_from_expr3(left)?;
                let mut r_expr = self.lower_from_expr3(right)?;

                // Only apply dimension reordering if we're NOT in an A2A context.
                // In A2A context, the implicit subscripts already handle dimension reordering.
                if self.active_dimension.is_none() {
                    // If the result is an array, check if operand dimension reordering is needed.
                    if let Some(bounds) = array_bounds
                        && bounds.dim_names().is_some()
                    {
                        let l_dim_names: Option<Vec<String>> =
                            match left.get_array_bounds().and_then(|b| b.dim_names()) {
                                Some(names) => Some(names.iter().map(|s| s.to_string()).collect()),
                                None => self.get_expr3_dimension_names(left),
                            };
                        let r_dim_names: Option<Vec<String>> =
                            match right.get_array_bounds().and_then(|b| b.dim_names()) {
                                Some(names) => Some(names.iter().map(|s| s.to_string()).collect()),
                                None => self.get_expr3_dimension_names(right),
                            };

                        // Check if right needs reordering to match left's dimension order
                        if let (Some(l_names), Some(r_names)) = (&l_dim_names, &r_dim_names)
                            && l_names != r_names
                        {
                            // Check if r can be reordered to match l
                            if let Some(reordering) = find_dimension_reordering(r_names, l_names) {
                                r_expr =
                                    self.apply_dimension_reordering(r_expr, reordering, *loc)?;
                            }
                            // Otherwise check if l can be reordered to match r
                            else if let Some(reordering) =
                                find_dimension_reordering(l_names, r_names)
                            {
                                l_expr =
                                    self.apply_dimension_reordering(l_expr, reordering, *loc)?;
                            }
                        }
                    }
                }

                let bin_op = match op {
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

                Ok(Expr::Op2(bin_op, Box::new(l_expr), Box::new(r_expr), *loc))
            }

            Expr3::If(cond, then_expr, else_expr, _bounds, loc) => {
                let cond = self.lower_from_expr3(cond)?;
                let t = self.lower_from_expr3(then_expr)?;
                let f = self.lower_from_expr3(else_expr)?;
                Ok(Expr::If(Box::new(cond), Box::new(t), Box::new(f), *loc))
            }
        }
    }

    /// Get dimension names from an Expr3 if it's an array variable
    fn get_expr3_dimension_names(&self, expr: &Expr3) -> Option<Vec<String>> {
        match expr {
            Expr3::Var(ident, _, _) => {
                let metadata = self.get_metadata(ident).ok()?;
                let dims = metadata.var.get_dimensions()?;
                Some(dims.iter().map(|d| d.name().to_string()).collect())
            }
            Expr3::Subscript(ident, _, _, _) => {
                let metadata = self.get_metadata(ident).ok()?;
                let dims = metadata.var.get_dimensions()?;
                Some(dims.iter().map(|d| d.name().to_string()).collect())
            }
            _ => None,
        }
    }

    /// Lower a BuiltinFn<Expr3> to BuiltinFn (i.e., BuiltinFn<Expr>).
    /// Handles array builtins that need preserve_wildcards_for_iteration.
    fn lower_builtin_expr3(
        &self,
        builtin: &crate::builtins::BuiltinFn<Expr3>,
    ) -> Result<BuiltinFn> {
        use crate::builtins::BuiltinFn as BFn;
        Ok(match builtin {
            BFn::Lookup(table_expr, index_expr, loc) => BuiltinFn::Lookup(
                Box::new(self.lower_from_expr3(table_expr)?),
                Box::new(self.lower_from_expr3(index_expr)?),
                *loc,
            ),
            BFn::LookupForward(table_expr, index_expr, loc) => BuiltinFn::LookupForward(
                Box::new(self.lower_from_expr3(table_expr)?),
                Box::new(self.lower_from_expr3(index_expr)?),
                *loc,
            ),
            BFn::LookupBackward(table_expr, index_expr, loc) => BuiltinFn::LookupBackward(
                Box::new(self.lower_from_expr3(table_expr)?),
                Box::new(self.lower_from_expr3(index_expr)?),
                *loc,
            ),
            BFn::Abs(a) => BuiltinFn::Abs(Box::new(self.lower_from_expr3(a)?)),
            BFn::Arccos(a) => BuiltinFn::Arccos(Box::new(self.lower_from_expr3(a)?)),
            BFn::Arcsin(a) => BuiltinFn::Arcsin(Box::new(self.lower_from_expr3(a)?)),
            BFn::Arctan(a) => BuiltinFn::Arctan(Box::new(self.lower_from_expr3(a)?)),
            BFn::Cos(a) => BuiltinFn::Cos(Box::new(self.lower_from_expr3(a)?)),
            BFn::Exp(a) => BuiltinFn::Exp(Box::new(self.lower_from_expr3(a)?)),
            BFn::Inf => BuiltinFn::Inf,
            BFn::Int(a) => BuiltinFn::Int(Box::new(self.lower_from_expr3(a)?)),
            BFn::IsModuleInput(id, loc) => BuiltinFn::IsModuleInput(id.clone(), *loc),
            BFn::Ln(a) => BuiltinFn::Ln(Box::new(self.lower_from_expr3(a)?)),
            BFn::Log10(a) => BuiltinFn::Log10(Box::new(self.lower_from_expr3(a)?)),
            BFn::Max(a, b) => {
                if b.is_none() {
                    // Single-arg array Max: preserve wildcards for iteration
                    let a = self.with_preserved_wildcards().lower_from_expr3(a)?;
                    BuiltinFn::Max(Box::new(a), None)
                } else {
                    // Two-arg scalar Max
                    let a = Box::new(self.lower_from_expr3(a)?);
                    let b = Some(Box::new(self.lower_from_expr3(b.as_ref().unwrap())?));
                    BuiltinFn::Max(a, b)
                }
            }
            BFn::Mean(args) => {
                // Mean can be used with arrays - preserve wildcards
                let ctx = self.with_preserved_wildcards();
                let args = args
                    .iter()
                    .map(|arg| ctx.lower_from_expr3(arg))
                    .collect::<Result<Vec<Expr>>>();
                BuiltinFn::Mean(args?)
            }
            BFn::Min(a, b) => {
                if b.is_none() {
                    // Single-arg array Min: preserve wildcards for iteration
                    let a = self.with_preserved_wildcards().lower_from_expr3(a)?;
                    BuiltinFn::Min(Box::new(a), None)
                } else {
                    // Two-arg scalar Min
                    let a = Box::new(self.lower_from_expr3(a)?);
                    let b = Some(Box::new(self.lower_from_expr3(b.as_ref().unwrap())?));
                    BuiltinFn::Min(a, b)
                }
            }
            BFn::Pi => BuiltinFn::Pi,
            BFn::Pulse(a, b, c) => {
                let c = match c {
                    Some(c) => Some(Box::new(self.lower_from_expr3(c)?)),
                    None => None,
                };
                BuiltinFn::Pulse(
                    Box::new(self.lower_from_expr3(a)?),
                    Box::new(self.lower_from_expr3(b)?),
                    c,
                )
            }
            BFn::Ramp(a, b, c) => {
                let c = match c {
                    Some(c) => Some(Box::new(self.lower_from_expr3(c)?)),
                    None => None,
                };
                BuiltinFn::Ramp(
                    Box::new(self.lower_from_expr3(a)?),
                    Box::new(self.lower_from_expr3(b)?),
                    c,
                )
            }
            BFn::SafeDiv(a, b, c) => {
                let c = match c {
                    Some(c) => Some(Box::new(self.lower_from_expr3(c)?)),
                    None => None,
                };
                BuiltinFn::SafeDiv(
                    Box::new(self.lower_from_expr3(a)?),
                    Box::new(self.lower_from_expr3(b)?),
                    c,
                )
            }
            BFn::Sign(a) => BuiltinFn::Sign(Box::new(self.lower_from_expr3(a)?)),
            BFn::Sin(a) => BuiltinFn::Sin(Box::new(self.lower_from_expr3(a)?)),
            BFn::Sqrt(a) => BuiltinFn::Sqrt(Box::new(self.lower_from_expr3(a)?)),
            BFn::Step(a, b) => BuiltinFn::Step(
                Box::new(self.lower_from_expr3(a)?),
                Box::new(self.lower_from_expr3(b)?),
            ),
            BFn::Tan(a) => BuiltinFn::Tan(Box::new(self.lower_from_expr3(a)?)),
            BFn::Time => BuiltinFn::Time,
            BFn::TimeStep => BuiltinFn::TimeStep,
            BFn::StartTime => BuiltinFn::StartTime,
            BFn::FinalTime => BuiltinFn::FinalTime,
            BFn::Rank(_, _) => {
                return sim_err!(TodoArrayBuiltin, self.ident.to_string());
            }
            BFn::Size(a) => {
                // Preserve wildcards for array iteration
                let a = self.with_preserved_wildcards().lower_from_expr3(a)?;
                BuiltinFn::Size(Box::new(a))
            }
            BFn::Stddev(a) => {
                let a = self.with_preserved_wildcards().lower_from_expr3(a)?;
                BuiltinFn::Stddev(Box::new(a))
            }
            BFn::Sum(a) => {
                let a = self.with_preserved_wildcards().lower_from_expr3(a)?;
                BuiltinFn::Sum(Box::new(a))
            }
        })
    }

    /// Lower an IndexExpr3 to SubscriptIndex for dynamic subscript handling.
    /// This is used when normalize_subscripts3 returns None.
    /// Returns SubscriptIndex::Single for single-element access or
    /// SubscriptIndex::Range for range access.
    #[allow(clippy::too_many_arguments)]
    fn lower_index_expr3(
        &self,
        idx: &IndexExpr3,
        id: &Ident<Canonical>,
        i: usize,
        dims: &[Dimension],
        _orig_dims: &[usize],
        _loc: Loc,
    ) -> Result<SubscriptIndex> {
        match idx {
            IndexExpr3::StarRange(subdim_name, star_loc) => {
                // StarRange in dynamic context - need to resolve the current element
                if self.active_dimension.is_none() {
                    return sim_err!(
                        ArrayReferenceNeedsExplicitSubscripts,
                        id.as_str().to_string()
                    );
                }
                let active_dims = self.active_dimension.as_ref().unwrap();
                let active_subscripts = self.active_subscript.as_ref().unwrap();
                let dim = &dims[i];

                // Check if this is the full dimension or a subdimension
                let parent_name = crate::common::CanonicalDimensionName::from_raw(dim.name());

                if subdim_name.as_str() == parent_name.as_str() {
                    // Full dimension - find matching active dimension
                    for (active_dim, active_subscript) in active_dims.iter().zip(active_subscripts)
                    {
                        if active_dim.name() == dim.name() {
                            if let Dimension::Named(_, _) = dim
                                && let Some(subscript_off) = dim.get_offset(active_subscript)
                            {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    (subscript_off + 1) as f64,
                                    *star_loc,
                                )));
                            } else if let Dimension::Indexed(_, _) = dim
                                && let Ok(idx_val) = active_subscript.as_str().parse::<usize>()
                            {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    idx_val as f64,
                                    *star_loc,
                                )));
                            }
                        }
                    }
                }

                // Subdimension case - not yet supported in dynamic context
                sim_err!(TodoStarRange, id.as_str().to_string())
            }

            // StaticRange - should have been handled by normalize_subscripts3,
            // but handle here as a fallback by creating a Range with constants
            IndexExpr3::StaticRange(start_0based, end_0based, loc) => {
                // Convert back to 1-based for the Expr (XMILE uses 1-based indices)
                let start_expr = Expr::Const((*start_0based + 1) as f64, *loc);
                let end_expr = Expr::Const(*end_0based as f64, *loc);
                Ok(SubscriptIndex::Range(start_expr, end_expr))
            }

            IndexExpr3::Range(start, end, _range_loc) => {
                // Dynamic range - lower both bound expressions
                let start_expr = self.lower_from_expr3(start)?;
                let end_expr = self.lower_from_expr3(end)?;
                Ok(SubscriptIndex::Range(start_expr, end_expr))
            }

            IndexExpr3::DimPosition(pos, dim_loc) => {
                // @1, @2, etc. in dynamic context
                if self.active_dimension.is_none() {
                    return sim_err!(
                        ArrayReferenceNeedsExplicitSubscripts,
                        id.as_str().to_string()
                    );
                }
                let active_subscripts = self.active_subscript.as_ref().unwrap();
                let pos = (*pos as usize).saturating_sub(1);
                if pos >= active_subscripts.len() {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }

                let subscript = &active_subscripts[pos];
                let dim = &dims[i];

                if let Some(offset) = dim.get_offset(subscript) {
                    Ok(SubscriptIndex::Single(Expr::Const(
                        (offset + 1) as f64,
                        *dim_loc,
                    )))
                } else if let Ok(idx_val) = subscript.as_str().parse::<usize>() {
                    Ok(SubscriptIndex::Single(Expr::Const(
                        idx_val as f64,
                        *dim_loc,
                    )))
                } else {
                    sim_err!(MismatchedDimensions, id.as_str().to_string())
                }
            }

            IndexExpr3::Expr(e) => {
                // Handle Var expressions that might be dimension elements or DimName.Index syntax
                if let Expr3::Var(ident, _, var_loc) = e {
                    let dim = &dims[i];

                    // First check if it's a named dimension element
                    if let Some(offset) = dim.get_offset(
                        &crate::common::CanonicalElementName::from_raw(ident.as_str()),
                    ) {
                        return Ok(SubscriptIndex::Single(Expr::Const(
                            (offset + 1) as f64,
                            *var_loc,
                        )));
                    }

                    // Check for DimName.Index syntax (e.g., "Dim.3" for indexed dimensions)
                    if let Dimension::Indexed(dim_name, size) = dim {
                        let expected_prefix = format!("{}.", dim_name.as_str());
                        if ident.as_str().starts_with(&expected_prefix)
                            && let Ok(idx) =
                                ident.as_str()[expected_prefix.len()..].parse::<usize>()
                        {
                            // Validate the index is within bounds (1-based)
                            let size_usize = *size as usize;
                            if idx >= 1 && idx <= size_usize {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    idx as f64, *var_loc,
                                )));
                            }
                        }
                    }

                    // Check if it's a dimension name (A2A reference)
                    let is_dim_name = self
                        .dimensions
                        .iter()
                        .any(|d| canonicalize(d.name()).as_str() == ident.as_str());

                    if is_dim_name {
                        if self.active_dimension.is_none() {
                            return sim_err!(
                                ArrayReferenceNeedsExplicitSubscripts,
                                id.as_str().to_string()
                            );
                        }
                        let active_dims = self.active_dimension.as_ref().unwrap();
                        let active_subscripts = self.active_subscript.as_ref().unwrap();

                        for (active_dim, active_subscript) in
                            active_dims.iter().zip(active_subscripts)
                        {
                            if canonicalize(active_dim.name()).as_str() == ident.as_str() {
                                if let Some(offset) = dim.get_offset(active_subscript) {
                                    return Ok(SubscriptIndex::Single(Expr::Const(
                                        (offset + 1) as f64,
                                        *var_loc,
                                    )));
                                } else if let Ok(idx_val) =
                                    active_subscript.as_str().parse::<usize>()
                                {
                                    return Ok(SubscriptIndex::Single(Expr::Const(
                                        idx_val as f64,
                                        *var_loc,
                                    )));
                                }
                            }
                        }
                    }
                }

                // Fall back to lowering the expression directly
                Ok(SubscriptIndex::Single(self.lower_from_expr3(e)?))
            }

            IndexExpr3::Dimension(name, dim_loc) => {
                let dim = &dims[i];

                // First check if the name matches an element of the parent dimension.
                // An element name that happens to match a dimension name should be
                // resolved as an element, not as an A2A dimension reference.
                if let Some(offset) = dim.get_offset(
                    &crate::common::CanonicalElementName::from_raw(name.as_str()),
                ) {
                    return Ok(SubscriptIndex::Single(Expr::Const(
                        (offset + 1) as f64,
                        *dim_loc,
                    )));
                }

                // A2A dimension reference in dynamic context
                if self.active_dimension.is_none() {
                    return sim_err!(
                        ArrayReferenceNeedsExplicitSubscripts,
                        id.as_str().to_string()
                    );
                }
                let active_dims = self.active_dimension.as_ref().unwrap();
                let active_subscripts = self.active_subscript.as_ref().unwrap();

                // Find the matching active dimension
                for (active_dim, active_subscript) in active_dims.iter().zip(active_subscripts) {
                    if canonicalize(active_dim.name()).as_str() == name.as_str() {
                        // Found the matching dimension
                        if let Some(offset) = dim.get_offset(active_subscript) {
                            return Ok(SubscriptIndex::Single(Expr::Const(
                                (offset + 1) as f64,
                                *dim_loc,
                            )));
                        } else if let Ok(idx_val) = active_subscript.as_str().parse::<usize>() {
                            return Ok(SubscriptIndex::Single(Expr::Const(
                                idx_val as f64,
                                *dim_loc,
                            )));
                        }
                    }
                }

                sim_err!(MismatchedDimensions, id.as_str().to_string())
            }
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
                tables: vec![],
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
                tables: vec![],
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
    let dims_ctx = DimensionsContext::default();
    let context = Context {
        dimensions: vec![],
        dimensions_ctx: &dims_ctx,
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
        preserve_wildcards_for_iteration: false,
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
    let mut output_exprs = output.unwrap();
    // The last element is the main expression
    assert_eq!(expected, output_exprs.pop().unwrap());

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
                tables: vec![],
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
                tables: vec![],
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
    let dims_ctx = DimensionsContext::default();
    let context = Context {
        dimensions: vec![],
        dimensions_ctx: &dims_ctx,
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
        preserve_wildcards_for_iteration: false,
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
    let mut output_exprs = output.unwrap();
    // The last element is the main expression
    assert_eq!(expected, output_exprs.pop().unwrap());
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
                tables: vec![],
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
                tables: vec![],
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
                tables: vec![],
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
                tables: vec![],
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
    let dims_ctx = DimensionsContext::default();
    let ctx = Context {
        dimensions: vec![],
        dimensions_ctx: &dims_ctx,
        model_name: &main_ident,
        ident: &test_ident,
        active_dimension: None,
        active_subscript: None,
        metadata: &metadata2,
        module_models: &module_models,
        is_initial: false,
        inputs,
        preserve_wildcards_for_iteration: false,
    };

    assert_eq!(Ok(None), ctx.fold_flows(&[]));
    assert_eq!(
        Ok(Some(Expr::Var(1, Loc::default()))),
        ctx.fold_flows(&[canonicalize("a")])
    );
    assert_eq!(
        Ok(Some(Expr::Op2(
            BinaryOp::Add,
            Box::new(Expr::Var(1, Loc::default())),
            Box::new(Expr::Var(4, Loc::default())),
            Loc::default(),
        ))),
        ctx.fold_flows(&[canonicalize("a"), canonicalize("d")])
    );

    // Test that fold_flows returns an error for non-existent flows
    let result = ctx.fold_flows(&[canonicalize("nonexistent")]);
    assert!(result.is_err(), "Expected error for non-existent flow");
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
                    // Create input set for module lookup key
                    let input_set: BTreeSet<Ident<Canonical>> =
                        inputs.iter().map(|mi| mi.dst.clone()).collect();
                    let inputs: Vec<Expr> = inputs
                        .into_iter()
                        .map(|mi| Expr::Var(ctx.get_offset(&mi.src).unwrap(), Loc::default()))
                        .collect();
                    vec![Expr::EvalModule(
                        ident.clone(),
                        model_name.clone(),
                        input_set,
                        inputs,
                    )]
                }
                Variable::Stock { init_ast: ast, .. } => {
                    let off = ctx.get_base_offset(&canonicalize(var.ident()))?;
                    if ctx.is_initial {
                        if ast.is_none() {
                            return sim_err!(EmptyEquation, var.ident().to_string());
                        }
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(ast) => {
                                let mut exprs = ctx.lower(ast)?;
                                let main_expr = exprs.pop().unwrap();
                                exprs.push(Expr::AssignCurr(off, Box::new(main_expr)));
                                exprs
                            }
                            Ast::ApplyToAll(dims, ast) => {
                                let exprs: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
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
                                        ctx.lower(ast).map(|mut exprs| {
                                            let main_expr = exprs.pop().unwrap();
                                            exprs.push(Expr::AssignCurr(
                                                off + i,
                                                Box::new(main_expr),
                                            ));
                                            exprs
                                        })
                                    })
                                    .collect();
                                exprs?.into_iter().flatten().collect()
                            }
                            Ast::Arrayed(dims, elements) => {
                                let exprs: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
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
                                        ctx.lower(ast).map(|mut exprs| {
                                            let main_expr = exprs.pop().unwrap();
                                            exprs.push(Expr::AssignCurr(
                                                off + i,
                                                Box::new(main_expr),
                                            ));
                                            exprs
                                        })
                                    })
                                    .collect();
                                exprs?.into_iter().flatten().collect()
                            }
                        }
                    } else {
                        match ast.as_ref().unwrap() {
                            Ast::Scalar(_) => vec![Expr::AssignNext(
                                off,
                                Box::new(ctx.build_stock_update_expr(off, var)?),
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
                                        let update_expr = ctx.build_stock_update_expr(
                                            ctx.get_offset(&canonicalize(var.ident()))?,
                                            var,
                                        )?;
                                        Ok(Expr::AssignNext(off + i, Box::new(update_expr)))
                                    })
                                    .collect();
                                exprs?
                            }
                        }
                    }
                }
                Variable::Var { tables, .. } => {
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
                            let mut exprs = ctx.lower(ast)?;
                            let main_expr = exprs.pop().unwrap();
                            let main_expr = if !tables.is_empty() {
                                let loc = main_expr.get_loc();
                                Expr::App(
                                    BuiltinFn::Lookup(
                                        Box::new(Expr::Var(off, loc)),
                                        Box::new(main_expr),
                                        loc,
                                    ),
                                    loc,
                                )
                            } else {
                                main_expr
                            };
                            exprs.push(Expr::AssignCurr(off, Box::new(main_expr)));
                            exprs
                        }
                        Ast::ApplyToAll(dims, ast) => {
                            let exprs: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
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
                                    ctx.lower(ast).map(|mut exprs| {
                                        let main_expr = exprs.pop().unwrap();
                                        exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                                        exprs
                                    })
                                })
                                .collect();
                            exprs?.into_iter().flatten().collect()
                        }
                        Ast::Arrayed(dims, elements) => {
                            let exprs: Result<Vec<Vec<Expr>>> = SubscriptIterator::new(dims)
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
                                    ctx.lower(ast).map(|mut exprs| {
                                        let main_expr = exprs.pop().unwrap();
                                        exprs.push(Expr::AssignCurr(off + i, Box::new(main_expr)));
                                        exprs
                                    })
                                })
                                .collect();
                            exprs?.into_iter().flatten().collect()
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

/// Recursively extract temporary array sizes from an expression.
/// Populates the temp_sizes_map with (temp_id, max_size) entries.
/// Since temp IDs restart at 0 for each lower() call, the same ID may be
/// reused across different expressions with different sizes. We track the
/// maximum size per ID to ensure the temp buffer is large enough for all uses.
fn extract_temp_sizes(expr: &Expr, temp_sizes_map: &mut HashMap<u32, usize>) {
    match expr {
        Expr::AssignTemp(id, inner, view) => {
            let size = view.dims.iter().product::<usize>();
            // Preserve the maximum size for this temp ID across all expressions
            temp_sizes_map
                .entry(*id)
                .and_modify(|existing| *existing = (*existing).max(size))
                .or_insert(size);
            extract_temp_sizes(inner, temp_sizes_map);
        }
        Expr::TempArray(_, _, _) | Expr::TempArrayElement(_, _, _, _) => {
            // These reference temps, but don't define sizes - do nothing
        }
        Expr::Const(_, _) | Expr::Var(_, _) | Expr::Dt(_) => {}
        Expr::Subscript(_, indices, _, _) => {
            for idx in indices {
                match idx {
                    SubscriptIndex::Single(e) => extract_temp_sizes(e, temp_sizes_map),
                    SubscriptIndex::Range(start, end) => {
                        extract_temp_sizes(start, temp_sizes_map);
                        extract_temp_sizes(end, temp_sizes_map);
                    }
                }
            }
        }
        Expr::StaticSubscript(_, _, _) => {}
        Expr::App(builtin, _) => {
            extract_temp_sizes_from_builtin(builtin, temp_sizes_map);
        }
        Expr::EvalModule(_, _, _, args) => {
            for arg in args {
                extract_temp_sizes(arg, temp_sizes_map);
            }
        }
        Expr::ModuleInput(_, _) => {}
        Expr::Op2(_, left, right, _) => {
            extract_temp_sizes(left, temp_sizes_map);
            extract_temp_sizes(right, temp_sizes_map);
        }
        Expr::Op1(_, inner, _) => {
            extract_temp_sizes(inner, temp_sizes_map);
        }
        Expr::If(cond, t, f, _) => {
            extract_temp_sizes(cond, temp_sizes_map);
            extract_temp_sizes(t, temp_sizes_map);
            extract_temp_sizes(f, temp_sizes_map);
        }
        Expr::AssignCurr(_, inner) | Expr::AssignNext(_, inner) => {
            extract_temp_sizes(inner, temp_sizes_map);
        }
    }
}

/// Extract temp sizes from builtin function arguments.
fn extract_temp_sizes_from_builtin(builtin: &BuiltinFn, temp_sizes_map: &mut HashMap<u32, usize>) {
    match builtin {
        BuiltinFn::Lookup(_, expr, _)
        | BuiltinFn::LookupForward(_, expr, _)
        | BuiltinFn::LookupBackward(_, expr, _)
        | BuiltinFn::Abs(expr)
        | BuiltinFn::Arccos(expr)
        | BuiltinFn::Arcsin(expr)
        | BuiltinFn::Arctan(expr)
        | BuiltinFn::Cos(expr)
        | BuiltinFn::Exp(expr)
        | BuiltinFn::Int(expr)
        | BuiltinFn::Ln(expr)
        | BuiltinFn::Log10(expr)
        | BuiltinFn::Sign(expr)
        | BuiltinFn::Sin(expr)
        | BuiltinFn::Size(expr)
        | BuiltinFn::Sqrt(expr)
        | BuiltinFn::Stddev(expr)
        | BuiltinFn::Sum(expr)
        | BuiltinFn::Tan(expr) => {
            extract_temp_sizes(expr, temp_sizes_map);
        }
        BuiltinFn::Max(a, b) | BuiltinFn::Min(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            if let Some(b) = b {
                extract_temp_sizes(b, temp_sizes_map);
            }
        }
        BuiltinFn::Mean(args) => {
            for arg in args {
                extract_temp_sizes(arg, temp_sizes_map);
            }
        }
        BuiltinFn::Pulse(a, b, c) | BuiltinFn::Ramp(a, b, c) | BuiltinFn::SafeDiv(a, b, c) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
            if let Some(c) = c {
                extract_temp_sizes(c, temp_sizes_map);
            }
        }
        BuiltinFn::Rank(a, opt) => {
            extract_temp_sizes(a, temp_sizes_map);
            if let Some((b, c)) = opt {
                extract_temp_sizes(b, temp_sizes_map);
                if let Some(c) = c {
                    extract_temp_sizes(c, temp_sizes_map);
                }
            }
        }
        BuiltinFn::Step(a, b) => {
            extract_temp_sizes(a, temp_sizes_map);
            extract_temp_sizes(b, temp_sizes_map);
        }
        BuiltinFn::Inf
        | BuiltinFn::Pi
        | BuiltinFn::Time
        | BuiltinFn::TimeStep
        | BuiltinFn::StartTime
        | BuiltinFn::FinalTime
        | BuiltinFn::IsModuleInput(_, _) => {}
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
    pub(crate) tables: HashMap<Ident<Canonical>, Vec<Table>>,
    /// All dimensions from the project, for bytecode compilation
    pub(crate) dimensions: Vec<Dimension>,
    /// DimensionsContext for subdimension relationship lookups
    pub(crate) dimensions_ctx: DimensionsContext,
    /// Maps module variable idents to their full ModuleKey (model_name, input_set).
    /// Used to correctly expand nested modules in runlist_order.
    pub(crate) module_refs: HashMap<Ident<Canonical>, ModuleKey>,
}

// calculate a mapping of module variable name -> module model name
pub(crate) fn calc_module_model_map(
    project: &Project,
    model_name: &Ident<Canonical>,
) -> HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> {
    let mut all_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();

    let model = Arc::clone(&project.models[model_name]);
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
                    tables: vec![],
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
                    tables: vec![],
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
                    tables: vec![],
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
                    tables: vec![],
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

    let model = Arc::clone(&project.models[model_name]);
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
        model: Arc<ModelStage1>,
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

        // Build module_refs: map from module variable ident to (model_name, input_set)
        let module_refs: HashMap<Ident<Canonical>, ModuleKey> = model
            .variables
            .iter()
            .filter_map(|(ident, var)| {
                if let Variable::Module {
                    model_name: module_model_name,
                    inputs,
                    ..
                } = var
                {
                    let input_set: BTreeSet<Ident<Canonical>> =
                        inputs.iter().map(|mi| mi.dst.clone()).collect();
                    Some((ident.clone(), (module_model_name.clone(), input_set)))
                } else {
                    None
                }
            })
            .collect();

        let converted_dims: Vec<Dimension> = project
            .datamodel
            .dimensions
            .iter()
            .map(Dimension::from)
            .collect();

        let build_var = |ident: &Ident<Canonical>, is_initial| {
            Var::new(
                &Context {
                    dimensions: converted_dims.clone(),
                    dimensions_ctx: &project.dimensions_ctx,
                    model_name,
                    ident,
                    active_dimension: None,
                    active_subscript: None,
                    metadata: &metadata,
                    module_models: &module_models,
                    is_initial,
                    inputs,
                    preserve_wildcards_for_iteration: false,
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
        let runlist_initials: Vec<Expr> =
            runlist_initials.into_iter().flat_map(|v| v.ast).collect();
        let runlist_flows: Vec<Expr> = runlist_flows.into_iter().flat_map(|v| v.ast).collect();
        let runlist_stocks: Vec<Expr> = runlist_stocks.into_iter().flat_map(|v| v.ast).collect();

        // Extract temp array information from all runlists
        let mut temp_sizes_map: HashMap<u32, usize> = HashMap::new();
        for expr in runlist_initials
            .iter()
            .chain(runlist_flows.iter())
            .chain(runlist_stocks.iter())
        {
            extract_temp_sizes(expr, &mut temp_sizes_map);
        }

        // Build temp_sizes vector, ordered by temp ID
        let n_temps = temp_sizes_map.len();
        let mut temp_sizes: Vec<usize> = vec![0; n_temps];
        for (id, size) in temp_sizes_map {
            temp_sizes[id as usize] = size;
        }

        let tables: Result<HashMap<Ident<Canonical>, Vec<Table>>> = var_names
            .iter()
            .map(|id| {
                let canonical_id = canonicalize(id);
                (id, &model.variables[&canonical_id])
            })
            .filter(|(_, v)| !v.tables().is_empty())
            .map(|(id, v)| {
                let tables_result: Result<Vec<Table>> =
                    v.tables().iter().map(|t| Table::new(id, t)).collect();
                (id, tables_result)
            })
            .map(|(id, tables_result)| match tables_result {
                Ok(tables) => Ok((canonicalize(id), tables)),
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
            dimensions: converted_dims,
            dimensions_ctx: project.dimensions_ctx.clone(),
            module_refs,
        })
    }

    pub fn compile(&self) -> Result<CompiledModule> {
        Compiler::new(self).compile()
    }
}

#[cfg(test)]
impl Module {
    /// Get flow expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions that target offsets within this variable's range.
    pub fn get_flow_exprs(&self, var_name: &str) -> Vec<&Expr> {
        use crate::common::canonicalize;
        let canonical_name = canonicalize(var_name);

        // Look up the variable's offset range
        let Some(model_offsets) = self.offsets.get(&self.ident) else {
            return vec![];
        };
        let Some(&(base_offset, size)) = model_offsets.get(&canonical_name) else {
            return vec![];
        };
        let offset_range = base_offset..base_offset + size;

        // Find all AssignCurr expressions that target offsets in this range
        self.runlist_flows
            .iter()
            .filter(|expr| {
                if let Expr::AssignCurr(off, _) = expr {
                    offset_range.contains(off)
                } else {
                    false
                }
            })
            .collect()
    }

    /// Get initial expressions for a variable (may be multiple for A2A arrays).
    /// Returns all AssignCurr expressions in the initials runlist for this variable.
    pub fn get_initial_exprs(&self, var_name: &str) -> Vec<&Expr> {
        use crate::common::canonicalize;
        let canonical_name = canonicalize(var_name);

        // Look up the variable's offset range
        let Some(model_offsets) = self.offsets.get(&self.ident) else {
            return vec![];
        };
        let Some(&(base_offset, size)) = model_offsets.get(&canonical_name) else {
            return vec![];
        };
        let offset_range = base_offset..base_offset + size;

        // Find all AssignCurr expressions that target offsets in this range
        self.runlist_initials
            .iter()
            .filter(|expr| {
                if let Expr::AssignCurr(off, _) = expr {
                    offset_range.contains(off)
                } else {
                    false
                }
            })
            .collect()
    }
}

struct Compiler<'module> {
    module: &'module Module,
    module_decls: Vec<ModuleDeclaration>,
    graphical_functions: Vec<Vec<(f64, f64)>>,
    /// Maps table variable names to their base index in graphical_functions.
    /// For subscripted lookups, the actual table is at base_id + element_offset.
    table_base_ids: HashMap<Ident<Canonical>, GraphicalFunctionId>,
    curr_code: ByteCodeBuilder,
    // Array support fields
    dimensions: Vec<DimensionInfo>,
    subdim_relations: Vec<SubdimensionRelation>,
    names: Vec<String>,
    static_views: Vec<StaticArrayView>,
    // Iteration context - set when compiling inside AssignTemp
    in_iteration: bool,
    /// When in optimized iteration mode, maps pre-pushed views to their stack offset.
    /// Each entry is (StaticArrayView, stack_offset) where stack_offset is 1-based from top.
    /// The output view is always at offset (n_source_views + 1).
    iter_source_views: Option<Vec<(StaticArrayView, u8)>>,
}

impl<'module> Compiler<'module> {
    fn new(module: &'module Module) -> Compiler<'module> {
        // Pre-populate graphical_functions with all tables and record base IDs
        let mut graphical_functions = Vec::new();
        let mut table_base_ids = HashMap::new();

        for (ident, tables) in &module.tables {
            let base_gf = graphical_functions.len() as GraphicalFunctionId;
            table_base_ids.insert(ident.clone(), base_gf);
            for table in tables {
                graphical_functions.push(table.data.clone());
            }
        }

        let mut compiler = Compiler {
            module,
            module_decls: vec![],
            graphical_functions,
            table_base_ids,
            curr_code: ByteCodeBuilder::default(),
            dimensions: vec![],
            subdim_relations: vec![],
            names: vec![],
            static_views: vec![],
            in_iteration: false,
            iter_source_views: None,
        };
        compiler.populate_dimension_metadata();
        compiler
    }

    /// Populate dimension metadata tables from the module's dimensions.
    /// This populates:
    /// - `names`: interned dimension and element names
    /// - `dimensions`: DimensionInfo for each dimension
    ///
    /// Note: Subdimension relations are populated lazily via `get_or_add_subdim_relation`
    /// when ViewStarRange bytecode is emitted, rather than pre-computing all pairs.
    fn populate_dimension_metadata(&mut self) {
        for dim in &self.module.dimensions {
            let dim_name = dim.name();
            let name_id = self.intern_name(dim_name);

            let dim_info = match dim {
                Dimension::Indexed(_, size) => DimensionInfo::indexed(name_id, *size as u16),
                Dimension::Named(_, named_dim) => {
                    let element_name_ids: SmallVec<[NameId; 8]> = named_dim
                        .elements
                        .iter()
                        .map(|elem| self.intern_name(elem.as_str()))
                        .collect();
                    DimensionInfo::named(name_id, element_name_ids)
                }
            };
            self.dimensions.push(dim_info);
        }
    }

    /// Intern a string name and return its NameId.
    /// If the name already exists, returns the existing NameId.
    fn intern_name(&mut self, name: &str) -> NameId {
        // Look for existing name
        if let Some(idx) = self.names.iter().position(|n| n == name) {
            return idx as NameId;
        }
        // Add new name
        let id = self.names.len() as NameId;
        self.names.push(name.to_string());
        id
    }

    /// Get or create a DimId for a dimension with the given name and size.
    /// If a dimension with the same name exists, returns its DimId (assumes same size).
    fn get_or_add_dim_id(&mut self, dim_name: &str, size: u16) -> DimId {
        // Look for existing dimension with the same name
        let name_id_to_find = self.names.iter().position(|n| n == dim_name);
        if let Some(name_id) = name_id_to_find
            && let Some(dim_idx) = self
                .dimensions
                .iter()
                .position(|d| d.name_id == name_id as NameId)
        {
            return dim_idx as DimId;
        }
        // Create new dimension
        let name_id = self.intern_name(dim_name);
        let dim_id = self.dimensions.len() as DimId;
        self.dimensions.push(DimensionInfo {
            name_id,
            size,
            is_indexed: false, // Assume named elements for now
            element_name_ids: SmallVec::new(),
        });
        dim_id
    }

    /// Look up or add a subdimension relation between child and parent dimensions.
    /// Returns Some(subdim_relation_id) if child is a subdimension of parent,
    /// or None if no relationship exists.
    ///
    /// This method is called lazily when ViewStarRange bytecode is emitted,
    /// rather than pre-computing all possible relations.
    #[allow(dead_code)]
    fn get_or_add_subdim_relation(
        &mut self,
        child_dim_name: &crate::common::CanonicalDimensionName,
        parent_dim_name: &crate::common::CanonicalDimensionName,
    ) -> Option<u16> {
        // First, find the DimIds for child and parent
        let child_dim_id = self.find_dim_id_by_name(child_dim_name.as_str())?;
        let parent_dim_id = self.find_dim_id_by_name(parent_dim_name.as_str())?;

        // Check if this relation already exists
        for (idx, rel) in self.subdim_relations.iter().enumerate() {
            if rel.child_dim_id == child_dim_id && rel.parent_dim_id == parent_dim_id {
                return Some(idx as u16);
            }
        }

        // Look up the relation from DimensionsContext
        let relation = self
            .module
            .dimensions_ctx
            .get_subdimension_relation(child_dim_name, parent_dim_name)?;

        // Convert and add to subdim_relations
        let parent_offsets: SmallVec<[u16; 16]> =
            relation.parent_offsets.iter().map(|&x| x as u16).collect();
        let is_contiguous = relation.is_contiguous();
        let start_offset = relation.start_offset() as u16;

        let rel_id = self.subdim_relations.len() as u16;
        self.subdim_relations.push(SubdimensionRelation {
            parent_dim_id,
            child_dim_id,
            parent_offsets,
            is_contiguous,
            start_offset,
        });

        Some(rel_id)
    }

    /// Find a DimId by dimension name, returns None if not found.
    #[allow(dead_code)]
    fn find_dim_id_by_name(&self, dim_name: &str) -> Option<DimId> {
        let name_id = self.names.iter().position(|n| n == dim_name)? as NameId;
        let dim_idx = self.dimensions.iter().position(|d| d.name_id == name_id)?;
        Some(dim_idx as DimId)
    }

    /// Add a static view and return its ViewId
    fn add_static_view(&mut self, view: StaticArrayView) -> ViewId {
        self.static_views.push(view);
        (self.static_views.len() - 1) as ViewId
    }

    /// Convert an ArrayView to a StaticArrayView for a variable
    fn array_view_to_static(&mut self, base_off: usize, view: &ArrayView) -> StaticArrayView {
        // Convert sparse info
        let sparse: SmallVec<[RuntimeSparseMapping; 2]> = view
            .sparse
            .iter()
            .map(|s| RuntimeSparseMapping {
                dim_index: s.dim_index as u8,
                parent_offsets: s.parent_offsets.iter().map(|&x| x as u16).collect(),
            })
            .collect();

        // Look up or create DimIds for each dimension using the dim_names
        let dim_ids: SmallVec<[DimId; 4]> = view
            .dim_names
            .iter()
            .zip(view.dims.iter())
            .map(|(name, &size)| {
                if name.is_empty() {
                    // No dimension name available - use placeholder
                    0 as DimId
                } else {
                    self.get_or_add_dim_id(name, size as u16)
                }
            })
            .collect();

        StaticArrayView {
            base_off: base_off as u32,
            is_temp: false,
            dims: view.dims.iter().map(|&d| d as u16).collect(),
            strides: view.strides.iter().map(|&s| s as i32).collect(),
            offset: view.offset as u32,
            sparse,
            dim_ids,
        }
    }

    /// Convert an ArrayView to a StaticArrayView for a temp array
    fn array_view_to_static_temp(&mut self, temp_id: u32, view: &ArrayView) -> StaticArrayView {
        let sparse: SmallVec<[RuntimeSparseMapping; 2]> = view
            .sparse
            .iter()
            .map(|s| RuntimeSparseMapping {
                dim_index: s.dim_index as u8,
                parent_offsets: s.parent_offsets.iter().map(|&x| x as u16).collect(),
            })
            .collect();

        // Look up or create DimIds for each dimension using the dim_names
        let dim_ids: SmallVec<[DimId; 4]> = view
            .dim_names
            .iter()
            .zip(view.dims.iter())
            .map(|(name, &size)| {
                if name.is_empty() {
                    // No dimension name available - use placeholder
                    0 as DimId
                } else {
                    self.get_or_add_dim_id(name, size as u16)
                }
            })
            .collect();

        StaticArrayView {
            base_off: temp_id,
            is_temp: true,
            dims: view.dims.iter().map(|&d| d as u16).collect(),
            strides: view.strides.iter().map(|&s| s as i32).collect(),
            offset: view.offset as u32,
            sparse,
            dim_ids,
        }
    }

    /// Emit bytecode to push an expression's view onto the view stack.
    /// This is used for array operations that need to iterate over arrays.
    fn walk_expr_as_view(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::StaticSubscript(off, view, _) => {
                // Create a static view and push it
                let static_view = self.array_view_to_static(*off, view);
                let view_id = self.add_static_view(static_view);
                self.push(Opcode::PushStaticView { view_id });
                Ok(())
            }
            Expr::TempArray(id, view, _) => {
                // Create a static view for the temp array and push it
                let static_view = self.array_view_to_static_temp(*id, view);
                let view_id = self.add_static_view(static_view);
                self.push(Opcode::PushStaticView { view_id });
                Ok(())
            }
            Expr::Var(off, _) => {
                // A bare variable reference used as an array - create a scalar view
                // This shouldn't normally happen for array operations, but handle it
                let view = ArrayView::contiguous(vec![1]);
                let static_view = self.array_view_to_static(*off, &view);
                let view_id = self.add_static_view(static_view);
                self.push(Opcode::PushStaticView { view_id });
                Ok(())
            }
            Expr::Subscript(off, indices, bounds, _) => {
                // Dynamic subscript with potential range indices
                // First, push a full view for the base array using explicit bounds
                let n_dims = bounds.len().min(4) as u8;
                let mut dims = [0u16; 4];
                for (i, &bound) in bounds.iter().take(4).enumerate() {
                    dims[i] = bound as u16;
                }
                self.push(Opcode::PushVarViewDirect {
                    base_off: *off as u16,
                    n_dims,
                    dims,
                });

                // Apply each subscript index to the view.
                // Single subscripts collapse dimensions, so we track how many have been
                // processed to compute effective_dim for subsequent ops.
                let mut singles_processed = 0usize;
                for (i, idx) in indices.iter().enumerate() {
                    let effective_dim = (i - singles_processed) as u8;

                    match idx {
                        SubscriptIndex::Single(expr) => {
                            // Evaluate the index expression and apply single subscript
                            self.walk_expr(expr).unwrap().unwrap();
                            self.push(Opcode::ViewSubscriptDynamic {
                                dim_idx: effective_dim,
                            });
                            singles_processed += 1; // Track collapse for subsequent indices
                        }
                        SubscriptIndex::Range(start, end) => {
                            // Evaluate start and end, then apply dynamic range
                            self.walk_expr(start).unwrap().unwrap();
                            self.walk_expr(end).unwrap().unwrap();
                            self.push(Opcode::ViewRangeDynamic {
                                dim_idx: effective_dim,
                            });
                        }
                    }
                }
                Ok(())
            }
            _ => {
                sim_err!(
                    Generic,
                    format!(
                        "Cannot push view for expression type {:?} - expected array expression",
                        std::mem::discriminant(expr)
                    )
                )
            }
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
                // For scalar access (old-style Subscript), all indices must be Single
                for (i, idx) in indices.iter().enumerate() {
                    match idx {
                        SubscriptIndex::Single(expr) => {
                            self.walk_expr(expr).unwrap().unwrap();
                            let bounds = bounds[i] as VariableOffset;
                            self.push(Opcode::PushSubscriptIndex { bounds });
                        }
                        SubscriptIndex::Range(_, _) => {
                            // Range subscripts should be handled via walk_expr_as_view
                            // in reduction context, not through scalar walk_expr
                            return sim_err!(
                                Generic,
                                "Range subscript in scalar context - use walk_expr_as_view"
                                    .to_string()
                            );
                        }
                    }
                }
                assert!(indices.len() == bounds.len());
                self.push(Opcode::LoadSubscript {
                    off: *off as VariableOffset,
                });
                Some(())
            }
            Expr::StaticSubscript(off, view, _) => {
                if self.in_iteration {
                    // In iteration context with optimized view hoisting
                    let static_view = self.array_view_to_static(*off, view);

                    let offset = self.find_iter_view_offset(&static_view).unwrap_or_else(|| {
                        unreachable!(
                            "StaticSubscript view not found in pre-pushed set - \
                             collect_iter_source_views_impl and walk_expr should visit same nodes"
                        )
                    });
                    self.push(Opcode::LoadIterViewAt { offset });
                    Some(())
                } else if view.dims.iter().product::<usize>() == 1 {
                    // Scalar result - compute final offset and load
                    let final_off = (*off + view.offset) as VariableOffset;
                    self.push(Opcode::LoadVar { off: final_off });
                    Some(())
                } else {
                    // Non-scalar array outside iteration context - this shouldn't happen
                    // for well-formed expressions after pass 1 decomposition
                    return sim_err!(
                        Generic,
                        "Non-scalar StaticSubscript outside iteration context".to_string()
                    );
                }
            }
            Expr::TempArray(id, view, _) => {
                if self.in_iteration {
                    // In iteration context with optimized view hoisting
                    let static_view = self.array_view_to_static_temp(*id, view);

                    let offset = self.find_iter_view_offset(&static_view).unwrap_or_else(|| {
                        unreachable!(
                            "TempArray view not found in pre-pushed set - \
                             collect_iter_source_views_impl and walk_expr should visit same nodes"
                        )
                    });
                    self.push(Opcode::LoadIterViewAt { offset });
                    Some(())
                } else {
                    // Outside iteration - push temp view for subsequent operations (like SUM)
                    let static_view = self.array_view_to_static_temp(*id, view);
                    let view_id = self.add_static_view(static_view);
                    self.push(Opcode::PushStaticView { view_id });
                    // Note: caller (like array builtin) will use and pop this view
                    None
                }
            }
            Expr::TempArrayElement(id, _view, idx, _) => {
                // Load a specific element from a temp array
                self.push(Opcode::LoadTempConst {
                    temp_id: *id as TempId,
                    index: *idx as u16,
                });
                Some(())
            }
            Expr::Dt(_) => {
                self.push(Opcode::LoadGlobalVar {
                    off: DT_OFF as VariableOffset,
                });
                Some(())
            }
            Expr::App(builtin, _) => {
                // Helper to extract table info from table expression
                fn extract_table_info(
                    table_expr: &Expr,
                    module_offsets: &HashMap<Ident<Canonical>, (usize, usize)>,
                ) -> Result<(Ident<Canonical>, Expr)> {
                    match table_expr {
                        Expr::Var(off, loc) => {
                            // Could be a simple scalar table or an element of an arrayed table
                            // (when subscript was static and compiled to a direct Var reference).
                            // Find the variable whose range contains this offset.
                            let (table_ident, base_off) = module_offsets
                                .iter()
                                .find(|(_, (base, size))| *off >= *base && *off < *base + *size)
                                .map(|(k, (base, _))| (k.clone(), *base))
                                .ok_or_else(|| {
                                    Error::new(
                                        ErrorKind::Simulation,
                                        ErrorCode::BadTable,
                                        Some("could not find table variable".to_string()),
                                    )
                                })?;
                            let elem_off = *off - base_off;
                            Ok((table_ident, Expr::Const(elem_off as f64, *loc)))
                        }
                        Expr::StaticSubscript(off, view, loc) => {
                            // Static subscript - element offset is precomputed in the ArrayView
                            // Reject ranges/wildcards - only single element selection is valid
                            if view.size() > 1 {
                                return sim_err!(
                                    BadTable,
                                    "range subscripts not supported in lookup tables".to_string()
                                );
                            }
                            let table_ident = module_offsets
                                .iter()
                                .find(|(_, (base, _))| *off == *base)
                                .map(|(k, _)| k.clone())
                                .ok_or_else(|| {
                                    Error::new(
                                        ErrorKind::Simulation,
                                        ErrorCode::BadTable,
                                        Some("could not find table variable".to_string()),
                                    )
                                })?;
                            Ok((table_ident, Expr::Const(view.offset as f64, *loc)))
                        }
                        Expr::Subscript(off, subscript_indices, dim_sizes, _loc) => {
                            // Subscripted table reference - compute element_offset
                            // For a multi-dimensional subscript, compute linear offset
                            // offset = sum(index_i * stride_i) where stride_i = product of sizes[i+1..]
                            let mut offset_expr: Option<Expr> = None;
                            let mut stride = 1usize;

                            // Process indices in reverse order to compute strides correctly
                            for (i, sub_idx) in subscript_indices.iter().enumerate().rev() {
                                let idx_expr = match sub_idx {
                                    SubscriptIndex::Single(expr) => {
                                        // Convert to 0-based index by subtracting 1
                                        let one = Expr::Const(1.0, expr.get_loc());
                                        Expr::Op2(
                                            BinaryOp::Sub,
                                            Box::new(expr.clone()),
                                            Box::new(one),
                                            expr.get_loc(),
                                        )
                                    }
                                    SubscriptIndex::Range(_, _) => {
                                        return sim_err!(
                                            BadTable,
                                            "range subscripts not supported in lookup tables"
                                                .to_string()
                                        );
                                    }
                                };

                                // Multiply by stride if not innermost dimension
                                let term = if stride == 1 {
                                    idx_expr
                                } else {
                                    let stride_const =
                                        Expr::Const(stride as f64, idx_expr.get_loc());
                                    Expr::Op2(
                                        BinaryOp::Mul,
                                        Box::new(idx_expr),
                                        Box::new(stride_const),
                                        *_loc,
                                    )
                                };

                                // Add to running offset
                                offset_expr = Some(match offset_expr {
                                    None => term,
                                    Some(prev) => Expr::Op2(
                                        BinaryOp::Add,
                                        Box::new(prev),
                                        Box::new(term),
                                        *_loc,
                                    ),
                                });

                                // Update stride for next dimension
                                stride *= dim_sizes.get(i).copied().unwrap_or(1);
                            }

                            let table_ident = module_offsets
                                .iter()
                                .find(|(_, (base, _))| *off == *base)
                                .map(|(k, _)| k.clone())
                                .ok_or_else(|| {
                                    Error::new(
                                        ErrorKind::Simulation,
                                        ErrorCode::BadTable,
                                        Some("could not find table variable".to_string()),
                                    )
                                })?;
                            Ok((table_ident, offset_expr.unwrap_or(Expr::Const(0.0, *_loc))))
                        }
                        _ => {
                            sim_err!(
                                BadTable,
                                "unsupported expression type for lookup table reference"
                                    .to_string()
                            )
                        }
                    }
                }

                // lookups are special
                if let BuiltinFn::Lookup(table_expr, index, _loc) = builtin {
                    let module_offsets = &self.module.offsets[&self.module.ident];
                    let (table_ident, element_offset_expr) =
                        extract_table_info(table_expr, module_offsets)?;

                    // Look up the base_gf for this table variable
                    let base_gf = *self.table_base_ids.get(&table_ident).ok_or_else(|| {
                        Error::new(
                            ErrorKind::Simulation,
                            ErrorCode::BadTable,
                            Some(format!("no graphical function found for '{table_ident}'")),
                        )
                    })?;

                    // Get the table count for bounds checking
                    let table_count = self
                        .module
                        .tables
                        .get(&table_ident)
                        .map(|tables| tables.len() as u16)
                        .unwrap_or(1);

                    // Emit: push element_offset, push lookup_index, Lookup { base_gf, table_count, mode }
                    self.walk_expr(&element_offset_expr)?.unwrap();
                    self.walk_expr(index)?.unwrap();
                    self.push(Opcode::Lookup {
                        base_gf,
                        table_count,
                        mode: LookupMode::Interpolate,
                    });
                    return Ok(Some(()));
                };

                // LookupForward and LookupBackward use the same Lookup opcode with different modes
                if let BuiltinFn::LookupForward(table_expr, index, _loc)
                | BuiltinFn::LookupBackward(table_expr, index, _loc) = builtin
                {
                    let mode = if matches!(builtin, BuiltinFn::LookupForward(_, _, _)) {
                        LookupMode::Forward
                    } else {
                        LookupMode::Backward
                    };
                    let module_offsets = &self.module.offsets[&self.module.ident];
                    let (table_ident, element_offset_expr) =
                        extract_table_info(table_expr, module_offsets)?;

                    let base_gf = *self.table_base_ids.get(&table_ident).ok_or_else(|| {
                        Error::new(
                            ErrorKind::Simulation,
                            ErrorCode::BadTable,
                            Some(format!("no graphical function found for '{table_ident}'")),
                        )
                    })?;

                    let table_count = self
                        .module
                        .tables
                        .get(&table_ident)
                        .map(|tables| tables.len() as u16)
                        .unwrap_or(1);

                    self.walk_expr(&element_offset_expr)?.unwrap();
                    self.walk_expr(index)?.unwrap();
                    self.push(Opcode::Lookup {
                        base_gf,
                        table_count,
                        mode,
                    });
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
                    BuiltinFn::Lookup(_, _, _)
                    | BuiltinFn::LookupForward(_, _, _)
                    | BuiltinFn::LookupBackward(_, _, _)
                    | BuiltinFn::IsModuleInput(_, _) => unreachable!(),
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
                    BuiltinFn::Max(a, b) => {
                        if let Some(b) = b {
                            // Two-argument scalar max
                            self.walk_expr(a)?.unwrap();
                            self.walk_expr(b)?.unwrap();
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        } else {
                            // Single-argument array max
                            self.walk_expr_as_view(a)?;
                            self.push(Opcode::ArrayMax {});
                            self.push(Opcode::PopView {});
                            return Ok(Some(()));
                        }
                    }
                    BuiltinFn::Min(a, b) => {
                        if let Some(b) = b {
                            // Two-argument scalar min
                            self.walk_expr(a)?.unwrap();
                            self.walk_expr(b)?.unwrap();
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(Opcode::LoadConstant { id });
                        } else {
                            // Single-argument array min
                            self.walk_expr_as_view(a)?;
                            self.push(Opcode::ArrayMin {});
                            self.push(Opcode::PopView {});
                            return Ok(Some(()));
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
                        // Check if this is a single array argument (array mean)
                        // vs multiple scalar arguments (variadic mean)
                        if args.len() == 1 {
                            // Check if the argument is an array expression
                            let arg = &args[0];
                            let is_array = matches!(
                                arg,
                                Expr::StaticSubscript(_, _, _) | Expr::TempArray(_, _, _)
                            );
                            if is_array {
                                // Array mean - use ArrayMean opcode
                                self.walk_expr_as_view(arg)?;
                                self.push(Opcode::ArrayMean {});
                                self.push(Opcode::PopView {});
                                return Ok(Some(()));
                            }
                        }

                        // Multi-argument scalar mean: (arg1 + arg2 + ... + argN) / N
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
                        return sim_err!(TodoArrayBuiltin, "RANK not yet supported".to_owned());
                    }
                    BuiltinFn::Size(arg) => {
                        // SIZE returns the number of elements in an array
                        self.walk_expr_as_view(arg)?;
                        self.push(Opcode::ArraySize {});
                        self.push(Opcode::PopView {});
                        return Ok(Some(()));
                    }
                    BuiltinFn::Stddev(arg) => {
                        // STDDEV computes standard deviation of array elements
                        self.walk_expr_as_view(arg)?;
                        self.push(Opcode::ArrayStddev {});
                        self.push(Opcode::PopView {});
                        return Ok(Some(()));
                    }
                    BuiltinFn::Sum(arg) => {
                        // SUM computes the sum of array elements
                        self.walk_expr_as_view(arg)?;
                        self.push(Opcode::ArraySum {});
                        self.push(Opcode::PopView {});
                        return Ok(Some(()));
                    }
                };
                let func = match builtin {
                    BuiltinFn::Lookup(_, _, _)
                    | BuiltinFn::LookupForward(_, _, _)
                    | BuiltinFn::LookupBackward(_, _, _) => unreachable!(),
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
            Expr::EvalModule(ident, model_name, input_set, args) => {
                for arg in args.iter() {
                    self.walk_expr(arg).unwrap().unwrap()
                }
                let module_offsets = &self.module.offsets[&self.module.ident];
                self.module_decls.push(ModuleDeclaration {
                    model_name: model_name.clone(),
                    input_set: input_set.clone(),
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
            Expr::AssignTemp(id, rhs, view) => {
                // AssignTemp evaluates an array expression element-by-element and stores to temp
                //
                // OPTIMIZED Bytecode pattern (hoisted view pushes):
                // 1. PushStaticView (OUTPUT temp's view - determines iteration size)
                // 2. BeginIter { write_temp_id, has_write_temp: true }
                //    - This captures view_stack.last() as the iteration view
                // 3. PushStaticView for each source view (a, b, etc.) - pushed ONCE
                // 4. [Loop body start]
                //    - Compile RHS in iteration context
                //      (each StaticSubscript/TempArray emits LoadIterViewAt with offset)
                //    - StoreIterElement
                // 5. NextIterOrJump { jump_back }
                // 6. EndIter
                // 7. PopView for each source view
                // 8. PopView (output view)
                //
                // IMPORTANT: Source views must be pushed AFTER BeginIter because BeginIter
                // uses view_stack.last() to determine iteration bounds. If source views
                // were pushed before BeginIter, it would use the wrong view for iteration.

                // 1. Collect all source views referenced in RHS (deduplicated)
                let source_views = self.collect_iter_source_views(rhs);
                let n_source_views = source_views.len();

                // Guard: LoadIterViewAt uses u8 for stack offset, limiting to 255 source views
                if n_source_views > u8::MAX as usize {
                    return sim_err!(
                        Generic,
                        format!(
                            "Expression references {} distinct array views, exceeding the maximum of 255",
                            n_source_views
                        )
                    );
                }

                // 2. Push the OUTPUT temp's view for iteration size
                let output_static_view = self.array_view_to_static_temp(*id, view);
                let output_view_id = self.add_static_view(output_static_view);
                self.push(Opcode::PushStaticView {
                    view_id: output_view_id,
                });

                // 3. Begin iteration - MUST be before source views are pushed
                // BeginIter captures view_stack.last() as the iteration view
                self.push(Opcode::BeginIter {
                    write_temp_id: *id as TempId,
                    has_write_temp: true,
                });

                // 4. Push all source views AFTER BeginIter and record their stack offsets
                // After this, view_stack looks like: [output_view, src1, src2, ...]
                // So src1 is at offset n_source_views, src2 at n_source_views-1, etc.
                let mut iter_views_with_offsets: Vec<(StaticArrayView, u8)> =
                    Vec::with_capacity(n_source_views);

                for (i, src_view) in source_views.into_iter().enumerate() {
                    let view_id = self.add_static_view(src_view.clone());
                    self.push(Opcode::PushStaticView { view_id });
                    // Offset is counted from top: last pushed is at offset 1
                    // First pushed source view will be at offset n_source_views after all are pushed
                    let offset = (n_source_views - i) as u8;
                    iter_views_with_offsets.push((src_view, offset));
                }

                // Record loop body start position
                let loop_start = self.curr_code.len();

                // 5. Compile RHS in iteration context with pre-pushed views
                self.in_iteration = true;
                self.iter_source_views = Some(iter_views_with_offsets);
                self.walk_expr(rhs)?.unwrap();
                self.iter_source_views = None;
                self.in_iteration = false;

                // Store the result to temp
                self.push(Opcode::StoreIterElement {});

                // Calculate jump offset (negative, back to loop start)
                let next_iter_pos = self.curr_code.len();
                let jump_back = (loop_start as isize - next_iter_pos as isize) as i16;

                self.push(Opcode::NextIterOrJump { jump_back });
                self.push(Opcode::EndIter {});

                // 6. Pop all source views (in reverse order of push)
                for _ in 0..n_source_views {
                    self.push(Opcode::PopView {});
                }

                // 7. Pop output view
                self.push(Opcode::PopView {});

                // AssignTemp doesn't produce a value on the stack
                None
            }
        };
        Ok(result)
    }

    fn push(&mut self, op: Opcode) {
        self.curr_code.push_opcode(op)
    }

    /// Collect all source views referenced in an expression.
    /// This traverses the expression and collects StaticArrayView data for each
    /// StaticSubscript and TempArray node, deduplicating identical views.
    fn collect_iter_source_views(&mut self, expr: &Expr) -> Vec<StaticArrayView> {
        let mut views = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_iter_source_views_impl(expr, &mut views, &mut seen);
        views
    }

    fn collect_iter_source_views_impl(
        &mut self,
        expr: &Expr,
        views: &mut Vec<StaticArrayView>,
        seen: &mut std::collections::HashSet<StaticArrayView>,
    ) {
        match expr {
            Expr::StaticSubscript(off, view, _) => {
                let static_view = self.array_view_to_static(*off, view);
                // O(1) deduplication using HashSet
                if seen.insert(static_view.clone()) {
                    views.push(static_view);
                }
            }
            Expr::TempArray(id, view, _) => {
                let static_view = self.array_view_to_static_temp(*id, view);
                if seen.insert(static_view.clone()) {
                    views.push(static_view);
                }
            }
            // Recurse into compound expressions
            Expr::Op2(_, lhs, rhs, _) => {
                self.collect_iter_source_views_impl(lhs, views, seen);
                self.collect_iter_source_views_impl(rhs, views, seen);
            }
            Expr::Op1(_, inner, _) => {
                self.collect_iter_source_views_impl(inner, views, seen);
            }
            Expr::If(cond, then_expr, else_expr, _) => {
                self.collect_iter_source_views_impl(cond, views, seen);
                self.collect_iter_source_views_impl(then_expr, views, seen);
                self.collect_iter_source_views_impl(else_expr, views, seen);
            }
            Expr::App(builtin, _) => {
                // Recurse into all arguments of the builtin function
                self.collect_builtin_views(builtin, views, seen);
            }
            // Leaf expressions that don't contain views
            Expr::Const(_, _)
            | Expr::Var(_, _)
            | Expr::Dt(_)
            | Expr::ModuleInput(_, _)
            | Expr::TempArrayElement(_, _, _, _) => {}
            // These shouldn't appear in iteration body expressions, but handle gracefully
            Expr::Subscript(_, _, _, _)
            | Expr::AssignCurr(_, _)
            | Expr::AssignNext(_, _)
            | Expr::AssignTemp(_, _, _)
            | Expr::EvalModule(_, _, _, _) => {}
        }
    }

    fn collect_builtin_views(
        &mut self,
        builtin: &BuiltinFn,
        views: &mut Vec<StaticArrayView>,
        seen: &mut std::collections::HashSet<StaticArrayView>,
    ) {
        use crate::builtins::BuiltinFn::*;
        match builtin {
            Lookup(a, b, _) | LookupForward(a, b, _) | LookupBackward(a, b, _) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
            }
            Abs(a) | Arccos(a) | Arcsin(a) | Arctan(a) | Cos(a) | Exp(a) | Int(a) | Ln(a)
            | Log10(a) | Sign(a) | Sin(a) | Sqrt(a) | Tan(a) => {
                self.collect_iter_source_views_impl(a, views, seen);
            }
            Max(a, opt_b) | Min(a, opt_b) => {
                self.collect_iter_source_views_impl(a, views, seen);
                if let Some(b) = opt_b {
                    self.collect_iter_source_views_impl(b, views, seen);
                }
            }
            Mean(exprs) => {
                for e in exprs {
                    self.collect_iter_source_views_impl(e, views, seen);
                }
            }
            Pulse(a, b, opt_c) | Ramp(a, b, opt_c) | SafeDiv(a, b, opt_c) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
                if let Some(c) = opt_c {
                    self.collect_iter_source_views_impl(c, views, seen);
                }
            }
            Step(a, b) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
            }
            // Array builtins with single argument
            Sum(a) | Stddev(a) | Size(a) => {
                self.collect_iter_source_views_impl(a, views, seen);
            }
            // Rank has a complex optional argument structure
            Rank(a, opt_args) => {
                self.collect_iter_source_views_impl(a, views, seen);
                if let Some((b, opt_c)) = opt_args {
                    self.collect_iter_source_views_impl(b, views, seen);
                    if let Some(c) = opt_c {
                        self.collect_iter_source_views_impl(c, views, seen);
                    }
                }
            }
            // Constants/no-arg builtins
            Inf | Pi | Time | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => {}
        }
    }

    /// Find the stack offset for a view that was pre-pushed.
    /// Returns Some(offset) if found, where offset is 1-based from stack top.
    fn find_iter_view_offset(&self, view: &StaticArrayView) -> Option<u8> {
        self.iter_source_views.as_ref().and_then(|views| {
            views
                .iter()
                .find(|(v, _)| v == view)
                .map(|(_, offset)| *offset)
        })
    }

    fn compile(mut self) -> Result<CompiledModule> {
        let compiled_initials = Arc::new(self.walk(&self.module.runlist_initials)?);
        let compiled_flows = Arc::new(self.walk(&self.module.runlist_flows)?);
        let compiled_stocks = Arc::new(self.walk(&self.module.runlist_stocks)?);

        // Build temp info from module
        let mut temp_offsets = Vec::with_capacity(self.module.n_temps);
        let mut offset = 0usize;
        for &size in &self.module.temp_sizes {
            temp_offsets.push(offset);
            offset += size;
        }
        let temp_total_size = offset;

        Ok(CompiledModule {
            ident: self.module.ident.clone(),
            n_slots: self.module.n_slots,
            context: Arc::new(ByteCodeContext {
                graphical_functions: self.graphical_functions,
                modules: self.module_decls,
                arrays: vec![],
                // Array support fields populated during compilation
                dimensions: self.dimensions,
                subdim_relations: self.subdim_relations,
                names: self.names,
                static_views: self.static_views,
                temp_offsets,
                temp_total_size,
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
        | Expr::EvalModule(_, _, _, _)
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
            | Expr::EvalModule(_, _, _, _)
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

fn pretty_subscript_index(idx: &SubscriptIndex) -> String {
    match idx {
        SubscriptIndex::Single(e) => pretty(e),
        SubscriptIndex::Range(start, end) => format!("{}:{}", pretty(start), pretty(end)),
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
            let args: Vec<_> = args.iter().map(pretty_subscript_index).collect();
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
            BuiltinFn::Lookup(table, idx, _loc) => {
                format!("lookup({}, {})", pretty(table), pretty(idx))
            }
            BuiltinFn::LookupForward(table, idx, _loc) => {
                format!("lookup_forward({}, {})", pretty(table), pretty(idx))
            }
            BuiltinFn::LookupBackward(table, idx, _loc) => {
                format!("lookup_backward({}, {})", pretty(table), pretty(idx))
            }
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
        Expr::EvalModule(module, model_name, _input_set, args) => {
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

/// Result of matching source dimensions to target dimensions.
///
/// For each target dimension, provides either:
/// - Some(source_idx): which source dimension maps here
/// - None: no source dimension (broadcast with stride 0)
#[derive(Clone, Debug, PartialEq)]
#[allow(dead_code)] // Scaffolding for future broadcast_view usage
pub struct DimensionMapping {
    /// mapping[target_idx] = Some(source_idx) or None
    /// For each target dimension, which source dimension maps to it (or None for broadcasting)
    pub mapping: Vec<Option<usize>>,
    /// For each source dimension, which target dimension it matched
    pub source_to_target: Vec<usize>,
}

/// Match source dimensions to target dimensions.
///
/// Algorithm (dimension-agnostic, works for any N):
/// 1. FIRST PASS: Assign all exact name matches (reserve them)
/// 2. SECOND PASS: For remaining sources, do size-based matching (indexed dims only)
/// 3. Build the reverse mapping (target → source)
///
/// This two-pass approach ensures that name matches take priority over size matches.
/// Without it, a greedy single-pass approach could let a size match "steal" a target
/// that a later source dimension would have matched by name.
///
/// Returns None if any source dimension cannot be matched.
#[allow(dead_code)] // Scaffolding for future broadcast_view usage
pub fn match_dimensions(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<DimensionMapping> {
    let source_to_target =
        match_dimensions_two_pass(source_dims, target_dims, &vec![false; target_dims.len()])?;

    // Build reverse mapping
    let mut mapping = vec![None; target_dims.len()];
    for (source_idx, &target_idx) in source_to_target.iter().enumerate() {
        mapping[target_idx] = Some(source_idx);
    }

    Some(DimensionMapping {
        mapping,
        source_to_target,
    })
}

/// Two-pass dimension matching that reserves name matches before size matches.
///
/// Pass 1: Find and assign all exact name matches
/// Pass 2: For remaining unmatched sources, try size-based matching (indexed dims only)
///
/// Returns source_to_target mapping, or None if matching fails.
fn match_dimensions_two_pass(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
    initially_used: &[bool],
) -> Option<Vec<usize>> {
    let partial = match_dimensions_two_pass_partial(source_dims, target_dims, initially_used);

    // Verify all sources were matched
    partial.into_iter().collect()
}

/// Two-pass dimension matching that allows partial matches (some sources unmatched).
///
/// This is used for cases like SUM(arr[A,B]) in context [A] where B won't match.
/// Returns a vector where each element is Some(target_idx) or None.
fn match_dimensions_two_pass_partial(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
    initially_used: &[bool],
) -> Vec<Option<usize>> {
    let mut target_used = initially_used.to_vec();
    let mut source_to_target: Vec<Option<usize>> = vec![None; source_dims.len()];

    // PASS 1: Exact name matches (highest priority)
    for (source_idx, source_dim) in source_dims.iter().enumerate() {
        for (target_idx, target) in target_dims.iter().enumerate() {
            if !target_used[target_idx] && target.name() == source_dim.name() {
                target_used[target_idx] = true;
                source_to_target[source_idx] = Some(target_idx);
                break;
            }
        }
    }

    // PASS 2: Size-based matches for remaining sources (indexed dimensions only)
    for (source_idx, source_dim) in source_dims.iter().enumerate() {
        if source_to_target[source_idx].is_some() {
            continue; // Already matched by name
        }

        if let Dimension::Indexed(_, source_size) = source_dim {
            for (target_idx, target) in target_dims.iter().enumerate() {
                if !target_used[target_idx]
                    && let Dimension::Indexed(_, target_size) = target
                    && source_size == target_size
                {
                    target_used[target_idx] = true;
                    source_to_target[source_idx] = Some(target_idx);
                    break;
                }
            }
        }
    }

    source_to_target
}

/// Find target dimension for a source dimension (single dimension lookup).
///
/// NOTE: For matching multiple source dimensions, prefer `match_dimensions_two_pass`
/// which correctly reserves name matches before allowing size-based matches.
/// This function is kept for cases where we need to match a single dimension
/// and the caller manages the used array properly.
#[allow(dead_code)] // Kept for potential single-dimension matching use cases
fn find_target_for_source(
    source_dim: &Dimension,
    target_dims: &[Dimension],
    used: &[bool],
) -> Option<usize> {
    // First pass: exact name match (works for both named and indexed)
    for (i, target) in target_dims.iter().enumerate() {
        if !used[i] && target.name() == source_dim.name() {
            return Some(i);
        }
    }

    // Second pass: size-based match (indexed dimensions only)
    // IMPORTANT: This should only be called when there's no name match pending
    // for any other source dimension. See match_dimensions_two_pass for proper handling.
    if let Dimension::Indexed(_, source_size) = source_dim {
        for (i, target) in target_dims.iter().enumerate() {
            if !used[i]
                && let Dimension::Indexed(_, target_size) = target
                && source_size == target_size
            {
                return Some(i);
            }
        }
    }

    None
}

/// Broadcast a source view to match target dimensions.
///
/// For each target dimension:
/// - If source has a matching dimension: use its stride
/// - If no match: use stride 0 (broadcast/repeat)
///
/// This is dimension-agnostic: works for any N.
///
/// NOTE: This function does not preserve sparse array information from the source view.
/// The resulting view always has an empty sparse vector. If sparse data preservation
/// is needed in the future, this would require transforming sparse indices to account
/// for the new dimension order and any broadcast dimensions.
#[allow(dead_code)] // Scaffolding for future optimization
pub fn broadcast_view(
    source_view: &ArrayView,
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<ArrayView> {
    let mapping = match_dimensions(source_dims, target_dims)?;

    let mut new_dims = Vec::with_capacity(target_dims.len());
    let mut new_strides = Vec::with_capacity(target_dims.len());
    let mut new_dim_names = Vec::with_capacity(target_dims.len());

    for (target_idx, target_dim) in target_dims.iter().enumerate() {
        new_dims.push(target_dim.len());
        new_dim_names.push(target_dim.name().to_string());

        match mapping.mapping[target_idx] {
            Some(source_idx) => {
                // Source dimension maps here - use its stride
                new_strides.push(source_view.strides[source_idx]);
            }
            None => {
                // No source dimension - broadcast (stride 0)
                new_strides.push(0);
            }
        }
    }

    Some(ArrayView {
        dims: new_dims,
        strides: new_strides,
        offset: source_view.offset,
        // Sparse info not preserved - see doc comment for rationale
        sparse: Vec::new(),
        dim_names: new_dim_names,
    })
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

        assert_eq!(view.dims, Vec::<usize>::new());
        assert_eq!(view.strides, Vec::<isize>::new());
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
            sparse: Vec::new(),
            dim_names: vec![String::new(), String::new()],
        };
        assert!(!view2.is_contiguous());

        // Not contiguous: wrong strides for row-major
        let view3 = ArrayView {
            dims: vec![3, 4],
            strides: vec![1, 3], // Column-major strides
            offset: 0,
            sparse: Vec::new(),
            dim_names: vec![String::new(), String::new()],
        };
        assert!(!view3.is_contiguous());

        // Contiguous: manually constructed but correct
        let view4 = ArrayView {
            dims: vec![2, 3, 4],
            strides: vec![12, 4, 1],
            offset: 0,
            sparse: Vec::new(),
            dim_names: vec![String::new(), String::new(), String::new()],
        };
        assert!(view4.is_contiguous());
    }

    #[test]
    fn test_dimension_metadata_population() {
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::project::Project;

        // Create a datamodel project with a named dimension
        let datamodel_project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_string()),
            },
            dimensions: vec![datamodel::Dimension::named(
                "letters".to_string(),
                vec![
                    "a".to_string(),
                    "b".to_string(),
                    "c".to_string(),
                    "d".to_string(),
                    "e".to_string(),
                ],
            )],
            units: vec![],
            models: vec![DatamodelModel {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![DatamodelVariable::Aux(DatamodelAux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Public,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
            }],
            source: None,
            ai_information: None,
        };

        // Convert to engine project
        let project: Project = datamodel_project.into();

        // Create a Module and compile it
        let model = project
            .models
            .get(&canonicalize("main"))
            .expect("main model should exist");
        let module = Module::new(
            &project,
            model.clone(),
            &std::collections::BTreeSet::new(),
            true,
        )
        .expect("Module creation should succeed");

        // Compile the module
        let compiled = module.compile().expect("Compilation should succeed");

        // Verify dimension metadata is populated
        let context = &compiled.context;

        // Should have one dimension: "letters" with 5 elements
        assert!(
            !context.dimensions.is_empty(),
            "Dimensions should be populated"
        );
        assert!(!context.names.is_empty(), "Names should be populated");

        // Find the "letters" dimension
        let letters_dim = context.dimensions.iter().find(|dim| {
            context
                .names
                .get(dim.name_id as usize)
                .is_some_and(|n| n == "letters")
        });

        assert!(
            letters_dim.is_some(),
            "Should have a 'letters' dimension. Names: {:?}, Dimensions: {:?}",
            context.names,
            context.dimensions
        );

        let letters_dim = letters_dim.unwrap();
        assert_eq!(
            letters_dim.size, 5,
            "letters dimension should have 5 elements"
        );
        assert!(
            !letters_dim.is_indexed,
            "letters should be a named dimension, not indexed"
        );
        assert_eq!(
            letters_dim.element_name_ids.len(),
            5,
            "Should have 5 element name IDs"
        );

        // Verify element names are interned
        let element_names: Vec<&str> = letters_dim
            .element_name_ids
            .iter()
            .filter_map(|&id| context.names.get(id as usize).map(|s| s.as_str()))
            .collect();
        assert_eq!(element_names.len(), 5);
        // Element names should be canonicalized (lowercase)
        assert!(element_names.contains(&"a"));
        assert!(element_names.contains(&"b"));
        assert!(element_names.contains(&"c"));
        assert!(element_names.contains(&"d"));
        assert!(element_names.contains(&"e"));
    }

    #[test]
    fn test_indexed_dimension_metadata() {
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::project::Project;

        // Create a datamodel project with an indexed dimension
        let datamodel_project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_string()),
            },
            dimensions: vec![datamodel::Dimension::indexed("Size".to_string(), 10)],
            units: vec![],
            models: vec![DatamodelModel {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![DatamodelVariable::Aux(DatamodelAux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Public,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let project: Project = datamodel_project.into();

        let model = project
            .models
            .get(&canonicalize("main"))
            .expect("main model should exist");
        let module = Module::new(
            &project,
            model.clone(),
            &std::collections::BTreeSet::new(),
            true,
        )
        .expect("Module creation should succeed");

        let compiled = module.compile().expect("Compilation should succeed");
        let context = &compiled.context;

        // Find the "size" dimension (name is canonicalized)
        let size_dim = context.dimensions.iter().find(|dim| {
            context
                .names
                .get(dim.name_id as usize)
                .is_some_and(|n| n == "size")
        });

        assert!(size_dim.is_some(), "Should have a 'size' dimension");
        let size_dim = size_dim.unwrap();
        assert_eq!(size_dim.size, 10, "Size dimension should have 10 elements");
        assert!(size_dim.is_indexed, "Size should be an indexed dimension");
        assert!(
            size_dim.element_name_ids.is_empty(),
            "Indexed dimensions should not have element names"
        );
    }

    #[test]
    fn test_lazy_subdimension_relation() {
        use crate::common::CanonicalDimensionName;
        use crate::datamodel::{
            self, Aux as DatamodelAux, Model as DatamodelModel, SimMethod, SimSpecs,
            Variable as DatamodelVariable, Visibility,
        };
        use crate::project::Project;

        // Create a datamodel project with a parent dimension and subdimension
        let datamodel_project = datamodel::Project {
            name: "test".to_string(),
            sim_specs: SimSpecs {
                start: 0.0,
                stop: 10.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: SimMethod::Euler,
                time_units: Some("time".to_string()),
            },
            dimensions: vec![
                datamodel::Dimension::named(
                    "Parent".to_string(),
                    vec![
                        "A".to_string(),
                        "B".to_string(),
                        "C".to_string(),
                        "D".to_string(),
                    ],
                ),
                datamodel::Dimension::named(
                    "Child".to_string(),
                    vec!["B".to_string(), "C".to_string()],
                ),
            ],
            units: vec![],
            models: vec![DatamodelModel {
                name: "main".to_string(),
                sim_specs: None,
                variables: vec![DatamodelVariable::Aux(DatamodelAux {
                    ident: "x".to_string(),
                    equation: datamodel::Equation::Scalar("1".to_string(), None),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    can_be_module_input: false,
                    visibility: Visibility::Public,
                    ai_state: None,
                    uid: None,
                })],
                views: vec![],
                loop_metadata: vec![],
            }],
            source: None,
            ai_information: None,
        };

        let project: Project = datamodel_project.into();

        let model = project
            .models
            .get(&canonicalize("main"))
            .expect("main model should exist");
        let module = Module::new(
            &project,
            model.clone(),
            &std::collections::BTreeSet::new(),
            true,
        )
        .expect("Module creation should succeed");

        // Create a Compiler directly to test lazy subdim_relation population
        let mut compiler = Compiler::new(&module);

        // Initially, subdim_relations should be empty (lazy population)
        assert!(
            compiler.subdim_relations.is_empty(),
            "subdim_relations should be empty before lazy lookup"
        );

        // Dimensions should be populated
        assert_eq!(
            compiler.dimensions.len(),
            2,
            "Should have 2 dimensions populated"
        );

        // Now call get_or_add_subdim_relation to lazily add the relation
        let child_name = CanonicalDimensionName::from_raw("Child");
        let parent_name = CanonicalDimensionName::from_raw("Parent");

        let rel_id = compiler.get_or_add_subdim_relation(&child_name, &parent_name);
        assert!(rel_id.is_some(), "Child should be subdimension of Parent");
        assert_eq!(rel_id, Some(0), "First relation should have id 0");

        // Now subdim_relations should have one entry
        assert_eq!(
            compiler.subdim_relations.len(),
            1,
            "Should have 1 subdim_relation after lazy lookup"
        );

        let relation = &compiler.subdim_relations[0];
        // B is at index 1 in Parent, C is at index 2
        assert_eq!(
            relation.parent_offsets.as_slice(),
            &[1, 2],
            "Child elements should map to parent indices 1, 2"
        );
        assert!(relation.is_contiguous, "B, C are contiguous in parent");
        assert_eq!(relation.start_offset, 1);

        // Calling again should return the same id (cached)
        let rel_id_again = compiler.get_or_add_subdim_relation(&child_name, &parent_name);
        assert_eq!(
            rel_id_again,
            Some(0),
            "Should return same id for same lookup"
        );
        assert_eq!(
            compiler.subdim_relations.len(),
            1,
            "Should still have only 1 relation (no duplicate)"
        );

        // Looking up a non-existent relation should return None
        let unrelated_name = CanonicalDimensionName::from_raw("Nonexistent");
        let no_rel = compiler.get_or_add_subdim_relation(&unrelated_name, &parent_name);
        assert!(
            no_rel.is_none(),
            "Non-existent dimension should return None"
        );

        // Parent is not a subdimension of Child
        let reverse_rel = compiler.get_or_add_subdim_relation(&parent_name, &child_name);
        assert!(
            reverse_rel.is_none(),
            "Parent is not a subdimension of Child"
        );
    }

    use crate::common::CanonicalDimensionName;

    fn indexed_dim(name: &str, size: u32) -> Dimension {
        Dimension::Indexed(CanonicalDimensionName::from_raw(name), size)
    }

    fn named_dim(name: &str, elements: &[&str]) -> Dimension {
        use crate::dimensions::NamedDimension;
        let canonical_elements: Vec<crate::common::CanonicalElementName> = elements
            .iter()
            .map(|e| crate::common::CanonicalElementName::from_raw(e))
            .collect();
        let indexed_elements: std::collections::HashMap<
            crate::common::CanonicalElementName,
            usize,
        > = canonical_elements
            .iter()
            .enumerate()
            .map(|(i, elem)| (elem.clone(), i + 1))
            .collect();
        Dimension::Named(
            CanonicalDimensionName::from_raw(name),
            NamedDimension {
                indexed_elements,
                elements: canonical_elements,
                maps_to: None,
            },
        )
    }

    #[test]
    fn test_find_target_for_source_name_match() {
        // Test name matching for indexed dimensions
        let source = indexed_dim("products", 3);
        let targets = vec![indexed_dim("products", 3)];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(result, Some(0), "Should match by name");
    }

    #[test]
    fn test_find_target_for_source_size_match() {
        // Test size-based matching for indexed dimensions with different names
        let source = indexed_dim("regions", 3);
        let targets = vec![indexed_dim("products", 3)];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(
            result,
            Some(0),
            "Should match by size for different-named indexed dims"
        );
    }

    #[test]
    fn test_find_target_for_source_size_mismatch() {
        // Test that size must match for indexed dimensions
        let source = indexed_dim("regions", 3);
        let targets = vec![indexed_dim("products", 5)];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(result, None, "Should not match when sizes differ");
    }

    #[test]
    fn test_find_target_for_source_named_no_size_match() {
        // Named dimensions should NOT match by size, only by name
        let source = named_dim("cities", &["boston", "seattle"]);
        let targets = vec![named_dim("products", &["widgets", "gadgets"])];
        let used = vec![false];

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(result, None, "Named dims should not match by size");
    }

    #[test]
    fn test_find_target_for_source_respects_used() {
        // Test that already-used targets are skipped
        let source = indexed_dim("regions", 3);
        let targets = vec![indexed_dim("products", 3), indexed_dim("categories", 3)];
        let used = vec![true, false]; // products already used

        let result = find_target_for_source(&source, &targets, &used);
        assert_eq!(
            result,
            Some(1),
            "Should match second target when first is used"
        );
    }

    #[test]
    fn test_match_dimensions_same_name() {
        // Test matching dimensions with same names
        let source = vec![indexed_dim("x", 2), indexed_dim("y", 3)];
        let target = vec![indexed_dim("x", 2), indexed_dim("y", 3)];

        let result = match_dimensions(&source, &target);
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.mapping, vec![Some(0), Some(1)]);
        assert_eq!(mapping.source_to_target, vec![0, 1]);
    }

    #[test]
    fn test_match_dimensions_different_names_same_size() {
        // Test matching indexed dimensions with different names but same sizes
        let source = vec![indexed_dim("a", 3)];
        let target = vec![indexed_dim("b", 3)];

        let result = match_dimensions(&source, &target);
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.mapping, vec![Some(0)]);
        assert_eq!(mapping.source_to_target, vec![0]);
    }

    #[test]
    fn test_match_dimensions_broadcasting() {
        // Test broadcasting: 1D source to 2D target
        let source = vec![indexed_dim("x", 2)];
        let target = vec![indexed_dim("x", 2), indexed_dim("y", 3)];

        let result = match_dimensions(&source, &target);
        assert!(result.is_some());
        let mapping = result.unwrap();
        assert_eq!(mapping.mapping, vec![Some(0), None]); // x matched, y is broadcast
        assert_eq!(mapping.source_to_target, vec![0]);
    }

    #[test]
    fn test_broadcast_view() {
        // Test broadcast_view creates correct strides
        let source_dims = vec![indexed_dim("x", 2)];
        let target_dims = vec![indexed_dim("x", 2), indexed_dim("y", 3)];

        // Source view: 1D contiguous [2], strides [1]
        let source_view = ArrayView::contiguous_with_names(vec![2], vec!["x".to_string()]);

        let result = broadcast_view(&source_view, &source_dims, &target_dims);
        assert!(result.is_some());
        let broadcast = result.unwrap();

        assert_eq!(broadcast.dims, vec![2, 3]);
        assert_eq!(broadcast.strides, vec![1, 0]); // x uses stride 1, y uses stride 0 (broadcast)
        assert_eq!(broadcast.offset, 0);
    }

    #[test]
    fn test_stock_with_nonexistent_flow() {
        // Regression test for crash when a stock references a flow that doesn't exist.
        // This should return a proper error, not panic.
        use crate::test_common::TestProject;

        let project = TestProject::new("stock_missing_flow").stock(
            "inventory",
            "100",
            &["nonexistent_inflow"],
            &[],
            None,
        );

        // Trying to build a simulation should fail gracefully, not panic.
        // The stock references "nonexistent_inflow" which doesn't exist.
        let result = project.build_sim();
        assert!(
            result.is_err(),
            "Expected an error for missing flow reference, but got Ok"
        );
    }
}
