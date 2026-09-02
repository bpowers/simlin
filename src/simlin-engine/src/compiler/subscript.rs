// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::ast::{ArrayView, Expr3, IndexExpr3, SparseInfo};
use crate::common::{CanonicalDimensionName, CanonicalElementName, ErrorCode, ErrorKind, Result};
use crate::dimensions::{
    Axis, Dimension, DimensionsContext, DirectMappingsOnly, axes_of, match_axes_partial,
};
use crate::{Error, sim_err};

/// Represents a subscript operation after parsing but before view construction.
/// Used to normalize different subscript syntaxes into a uniform representation
/// that can be processed by build_view_from_ops.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum IndexOp {
    /// Range subscript with start and end (0-based, end exclusive).
    /// Example: `arr[2:5]` becomes `Range { start: 1, end: 5, .. }`.
    Range {
        start: usize,
        end: usize,
        /// The dimension the resulting axis ranges over, when the range came
        /// from a star range over a CONTIGUOUS subdimension (`arr[*:Sub]`) or
        /// over an indexed one. `None` for literal bounds (`arr[2:5]`), whose
        /// axis is a slice of the parent and keeps the parent's name.
        axis: Option<CanonicalDimensionName>,
    },
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
    SparseRange {
        /// Parent offsets to iterate (e.g. `[0, 2]` for elements at indices 0
        /// and 2).
        parent_offsets: Vec<usize>,
        /// The subdimension the resulting axis ranges over.
        axis: CanonicalDimensionName,
    },
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
    /// Active A2A dimensions (for dimension mapping resolution)
    pub(crate) active_dimension: Option<&'a [Dimension]>,
    /// For dimension mapping lookups
    pub(crate) dimensions_ctx: Option<&'a DimensionsContext>,
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

impl Subscript3Config<'_> {
    /// The declared dimension `name` denotes, if the model has one.
    ///
    /// A `dimensions::Dimension`'s name is canonical by construction and so is
    /// every identifier reaching this module, so the comparison is between two
    /// canonical strings and neither side needs re-canonicalizing.
    fn dimension_named(&self, name: &str) -> Option<&Dimension> {
        self.all_dimensions.iter().find(|d| d.name() == name)
    }

    /// The position in the active apply-to-all dimensions that a subscript
    /// naming the dimension `name` reads, or `None` outside an apply-to-all
    /// body or when no active axis corresponds.
    ///
    /// The correspondence is [`crate::dimensions::match_axes_partial`], the
    /// engine's one axis-matching precedence, over the single named axis.
    /// Only the DIRECT declared mappings are admitted, because the element
    /// this resolves to is `build_view_from_ops`'s `ActiveDimRef` arm reading
    /// the active element off the source axis.
    ///
    /// `claimed` holds the active positions the reference's EARLIER subscripts
    /// already read, and an unclaimed one is preferred: `square[D,D]` in an
    /// equation over `[D, D]` reads `square[d_i, d_j]`, the cell, not
    /// `square[d_i, d_i]`, the diagonal. A subscript that names a dimension the
    /// equation iterates FEWER times than the reference spells it -- `m[D,D]`
    /// under a `[D]` target -- has no unclaimed position left and falls back to
    /// the claimed one, which is the only element it can mean.
    fn active_dim_ref(&self, name: &str, claimed: &[usize]) -> Option<usize> {
        let active_dims = self.active_dimension?;
        // The overwhelmingly common subscript names an active dimension by its
        // exact name, and exact name is the precedence's first rung, so the
        // first unclaimed active axis of that name is the answer the matcher
        // would give -- without building its axis lists.
        if let Some(idx) = active_dims
            .iter()
            .enumerate()
            .position(|(i, dim)| dim.name() == name && !claimed.contains(&i))
        {
            return Some(idx);
        }
        let pick = |targets: &[Axis<'_>]| -> Option<usize> {
            match_axes_partial(
                &[Axis::named(name, 0)],
                targets,
                &DirectMappingsOnly(self.dimensions_ctx),
            )
            .into_iter()
            .next()
            .flatten()
            .map(|(idx, _)| idx)
        };
        let all = axes_of(active_dims);
        // With nothing claimed the pool is every active axis.
        if claimed.is_empty() {
            return pick(&all);
        }
        let pool: Vec<usize> = (0..all.len()).filter(|i| !claimed.contains(i)).collect();
        let unclaimed: Vec<Axis<'_>> = pool.iter().map(|&i| all[i]).collect();
        pick(&unclaimed).map(|idx| pool[idx]).or_else(|| pick(&all))
    }
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
    let mut operations = Vec::with_capacity(args.len());
    // The active positions this reference's earlier subscripts already read, so
    // a repeated dimension name takes its own axis (see `active_dim_ref`).
    let mut claimed: Vec<usize> = Vec::new();

    for (i, arg) in args.iter().enumerate() {
        if i >= config.dims.len() {
            return None;
        }

        let parent_dim = &config.dims[i];
        let parent_name = parent_dim.canonical_name();

        let op = match arg {
            IndexExpr3::StarRange(subdim_name, _) => {
                // Pass 0 converts a bare `*` into a star range over the axis's
                // own dimension, which is the whole axis.
                if subdim_name == parent_name {
                    IndexOp::Wildcard
                } else if let Some(Dimension::Indexed(_, size)) =
                    config.dimension_named(subdim_name.as_str())
                {
                    // `*:IndexedDim` desugars to `[1:SIZE(IndexedDim)]`.
                    IndexOp::Range {
                        start: 0,
                        end: *size as usize,
                        axis: Some(subdim_name.clone()),
                    }
                } else {
                    let relation = config
                        .dimensions_ctx
                        .get_subdimension_relation(subdim_name, parent_name)?;

                    // The resulting axis ranges over the SUBDIMENSION, not the
                    // parent, and is named for it. `ast::Expr2`'s bounds for
                    // the same reference say the same thing, and a temp the
                    // reference is materialized into is matched to its source
                    // view by dimension id -- so naming this axis for the
                    // parent leaves the two disagreeing and every element of
                    // the temp reads NaN.
                    if relation.is_contiguous() {
                        let start = relation.start_offset();
                        IndexOp::Range {
                            start,
                            end: start + relation.parent_offsets.len(),
                            axis: Some(subdim_name.clone()),
                        }
                    } else {
                        IndexOp::SparseRange {
                            parent_offsets: relation.parent_offsets.clone(),
                            axis: subdim_name.clone(),
                        }
                    }
                }
            }

            // StaticRange - already has 0-based indices from Expr2->Expr3 lowering
            IndexExpr3::StaticRange(start_0based, end_0based, _) => IndexOp::Range {
                start: *start_0based,
                end: *end_0based,
                axis: None,
            },

            IndexExpr3::Range(start_expr, end_expr, _) => {
                // Dynamic range - try to resolve both bounds to constants
                // If either can't be resolved, normalization fails and we fall back to dynamic handling
                let resolve_to_index = |expr: &Expr3| -> Option<usize> {
                    match expr {
                        Expr3::Const(_, val, _) => {
                            // Numeric constant - convert from 1-based to 0-based
                            Some((val.value() as isize - 1).max(0) as usize)
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
                IndexOp::Range {
                    start: start_idx,
                    end: end_idx + 1,
                    axis: None,
                }
            }

            IndexExpr3::DimPosition(pos, _) => {
                // @N is 1-based: @1 -> position 0, @2 -> position 1, etc.
                // @0 is invalid; bail out so the caller reports an error.
                if *pos == 0 {
                    return None;
                }
                IndexOp::DimPosition(*pos as usize - 1)
            }

            IndexExpr3::Expr(expr) => {
                match expr {
                    Expr3::Const(_, val, _) => {
                        let idx = (val.value() as isize - 1).max(0) as usize;
                        IndexOp::Single(idx)
                    }
                    Expr3::Var(ident, _, _) => {
                        // An ELEMENT of this axis takes priority over a
                        // like-named dimension (`dimensions::resolve_axis_index_name`).
                        if let Dimension::Named(_, named_dim) = parent_dim
                            && let Some(idx) = named_dim.get_element_index(ident.as_str())
                        {
                            IndexOp::Single(idx)
                        } else if let Some(idx) = indexed_element_index(parent_dim, ident.as_str())?
                        {
                            IndexOp::Single(idx)
                        } else if config.dimension_named(ident.as_str()).is_some() {
                            let active = config.active_dim_ref(ident.as_str(), &claimed)?;
                            claimed.push(active);
                            IndexOp::ActiveDimRef(active)
                        } else {
                            // Not a known element or dimension - need dynamic handling
                            return None;
                        }
                    }
                    _ => return None,
                }
            }

            IndexExpr3::Dimension(name, _) => {
                // An ELEMENT of this axis takes priority over a like-named
                // dimension, exactly as in the `Expr` arm above.
                if let Dimension::Named(_, named_dim) = parent_dim
                    && let Some(idx) = named_dim.get_element_index(name.as_str())
                {
                    IndexOp::Single(idx)
                } else {
                    let active = config.active_dim_ref(name.as_str(), &claimed)?;
                    claimed.push(active);
                    IndexOp::ActiveDimRef(active)
                }
            }
        };

        operations.push(op);
    }

    Some(operations)
}

/// The 0-based element index `ident` names on an INDEXED axis, whose elements
/// are spelled `DimName.Index` with a 1-based index.
///
/// `Ok(None)` -- spelled here as `Some(None)` so the caller's `?` reports the
/// unrecoverable case -- means the axis is not indexed or the identifier is not
/// in that form; `None` means it is, but names no element of the axis, which no
/// static view can express.
fn indexed_element_index(parent_dim: &Dimension, ident: &str) -> Option<Option<usize>> {
    let Dimension::Indexed(dim_name, size) = parent_dim else {
        return Some(None);
    };
    let Some(index_text) = ident.strip_prefix(&format!("{}.", dim_name.as_str())) else {
        return Some(None);
    };
    let idx = index_text.parse::<usize>().ok()?;
    if idx >= 1 && idx <= *size as usize {
        Some(Some(idx - 1))
    } else {
        None
    }
}

/// The name of the view axis an operation over input dimension `i` produces:
/// the dimension the operation ranges over when it names one, otherwise the
/// input axis's own dimension.
fn axis_name(axis: Option<&CanonicalDimensionName>, config: &ViewBuildConfig, i: usize) -> String {
    if let Some(axis) = axis {
        return axis.as_str().to_string();
    }
    config
        .dims
        .get(i)
        .map(|d| d.name().to_string())
        .unwrap_or_default()
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
            IndexOp::Range { start, end, .. } => {
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
            IndexOp::SparseRange { parent_offsets, .. } => {
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

                let offset = dim.get_offset(subscript).or_else(|| {
                    // The active element's own name is not declared on this
                    // source axis, so the reference resolves through the
                    // shared executed rule (GH #997): the declared mapping,
                    // then a mapped parent of the active subdimension.
                    // `normalize_subscripts3` already picked the active
                    // dimension, so this is one call rather than a search.
                    //
                    // The mapped-parent step is new here (it was already in
                    // `get_implicit_subscript_off`, the other executed site).
                    // Unifying can only resolve a reference that previously
                    // failed to compile: the shared rule tries name and then
                    // mapping first, which is exactly what this arm did, and
                    // reaches the parent step only where both missed.
                    let dims_ctx = config.dimensions_ctx?;
                    let active_dims = config.active_dimension?;
                    let active_dim = &active_dims[*active_idx];
                    if let Some(resolved) = dims_ctx.resolve_mapped_read(dim, active_dim, subscript)
                    {
                        // A declared correspondence is authoritative: an element
                        // it names that this axis does not declare is an error,
                        // not a reason to read some other element.
                        return dim.get_offset(&resolved);
                    }
                    // Nothing is declared between the two dimensions, so the
                    // reference is POSITIONAL: the active element's ordinal
                    // within its own dimension, indexing this axis. That is
                    // `Context::resolve_iteration_element`'s last resort for an
                    // axis it could not pair by name or mapping, and this is the
                    // same question one axis-collapse earlier; a declared
                    // correspondence that fails to translate stops short of it
                    // there too. Out of range is an error, exactly as it is for
                    // the `IndexOp::Single` an explicit element produces.
                    let source_name = dim.canonical_name();
                    let active_name = active_dim.canonical_name();
                    let declared = dims_ctx.has_mapping_to(source_name, active_name)
                        || dims_ctx.has_mapping_to(active_name, source_name)
                        || dims_ctx.has_mapping_to_parent_of(source_name, active_name);
                    if declared {
                        return None;
                    }
                    active_dim
                        .get_offset(subscript)
                        .filter(|offset| *offset < dim.len())
                });

                if let Some(offset) = offset {
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
            IndexOp::Range { start, end, axis } => {
                new_dims.push(end - start);
                new_strides.push(orig_strides[i]);
                new_dim_names.push(axis_name(axis.as_ref(), config, i));
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
            IndexOp::SparseRange {
                parent_offsets,
                axis,
            } => {
                // Dimension size is the number of sparse elements
                new_dims.push(parent_offsets.len());
                new_strides.push(orig_strides[i]);
                sparse_info.push(SparseInfo {
                    dim_index: output_dim_idx,
                    parent_offsets: parent_offsets.clone(),
                });
                new_dim_names.push(axis_name(Some(axis), config, i));
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
