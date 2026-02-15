// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::{ArrayView, Expr3, IndexExpr3, SparseInfo};
use crate::common::{CanonicalElementName, ErrorCode, ErrorKind, Result, canonicalize};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::{Error, sim_err};

/// Represents a subscript operation after parsing but before view construction.
/// Used to normalize different subscript syntaxes into a uniform representation
/// that can be processed by build_view_from_ops.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum IndexOp {
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
pub(crate) struct ViewBuildResult {
    /// The constructed array view
    pub(crate) view: ArrayView,
    /// Mapping from output dimension index to input dimension index.
    /// dim_mapping[i] = Some(j) means output dim i comes from input dim j.
    /// dim_mapping[i] = None means output dim i was removed (single index).
    pub(crate) dim_mapping: Vec<Option<usize>>,
    /// Start offset for each input dimension (for A2A element index calculation)
    pub(crate) single_indices: Vec<usize>,
}

/// Configuration for view building.
/// Contains context needed for ActiveDimRef resolution.
pub(crate) struct ViewBuildConfig<'a> {
    /// Active A2A subscript values (if in A2A context)
    pub(crate) active_subscript: Option<&'a [CanonicalElementName]>,
    /// Dimensions of the variable being subscripted (for element name -> offset lookups)
    pub(crate) dims: &'a [Dimension],
}

/// Configuration for subscript normalization from Expr3.
pub(crate) struct Subscript3Config<'a> {
    /// Dimensions of the variable being subscripted
    pub(crate) dims: &'a [Dimension],
    /// All dimensions in the model (for checking if a name is a dimension)
    pub(crate) all_dimensions: &'a [Dimension],
    /// For subdimension relationship lookups
    pub(crate) dimensions_ctx: &'a DimensionsContext,
    /// Active A2A dimensions (if in A2A context)
    pub(crate) active_dimension: Option<&'a [Dimension]>,
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
pub(crate) fn normalize_subscripts3(
    args: &[IndexExpr3],
    config: &Subscript3Config,
) -> Option<Vec<IndexOp>> {
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
                        if *canonicalize(d.name()) == *subdim_canonical {
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

            // StaticRange - already has 0-based indices from Expr2->Expr3 lowering
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
                            // Could be a named dimension element - use O(1) hash lookup
                            if let Dimension::Named(_, named_dim) = parent_dim {
                                named_dim.get_element_index(ident.as_str())
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
                        // Use O(1) hash lookup instead of linear search
                        let element_idx = if let Dimension::Named(_, named_dim) = parent_dim {
                            named_dim.get_element_index(ident.as_str())
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
                                    .any(|d| &*canonicalize(d.name()) == ident.as_str());

                                if is_dim_name {
                                    let active_dims = config.active_dimension?;
                                    let active_idx = active_dims.iter().position(|ad| {
                                        &*canonicalize(ad.name()) == ident.as_str()
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
                                .any(|d| &*canonicalize(d.name()) == ident.as_str());

                            if is_dim_name {
                                // It's a dimension name - find matching active dimension
                                let active_dims = config.active_dimension?;
                                let active_idx = active_dims
                                    .iter()
                                    .position(|ad| &*canonicalize(ad.name()) == ident.as_str())?;
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
                // Use O(1) hash lookup instead of linear search.
                if let Dimension::Named(_, named_dim) = parent_dim
                    && let Some(idx) = named_dim.get_element_index(name.as_str())
                {
                    operations.push(IndexOp::Single(idx));
                    continue;
                }

                // A2A dimension reference - need to find matching active dimension
                let active_dims = config.active_dimension?;
                let active_idx = active_dims
                    .iter()
                    .position(|ad| &*canonicalize(ad.name()) == name.as_str())?;
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
pub(crate) fn build_view_from_ops(
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
