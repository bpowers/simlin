// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::ast::{
    self, ArrayView, BinaryOp, Expr3, Expr3LowerContext, IndexExpr3, Loc, Pass1Context,
};
use crate::common::{
    Canonical, CanonicalDimensionName, CanonicalElementName, ErrorCode, ErrorKind, Ident, Result,
    canonicalize,
};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::variable::Variable;
use crate::{Error, sim_err};

use super::dimensions::{UnaryOp, find_dimension_reordering, match_dimensions_with_mapping};
use super::expr::{BuiltinFn, Expr, SubscriptIndex};
use super::subscript::{
    IndexOp, Subscript3Config, ViewBuildConfig, ViewBuildResult, build_view_from_ops,
    normalize_subscripts3,
};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy)]
pub(crate) struct VariableMetadata<'a> {
    pub(crate) offset: usize,
    pub(crate) size: usize,
    pub(crate) var: &'a Variable,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub(crate) struct Context<'a> {
    pub(crate) core: ContextCore<'a>,
    #[allow(dead_code)]
    pub(crate) ident: &'a Ident<Canonical>,
    pub(crate) active_dimension: Option<Arc<[Dimension]>>,
    pub(crate) active_subscript: Option<Vec<CanonicalElementName>>,
    pub(crate) is_initial: bool,
    /// When true, wildcards should always be preserved for iteration (inside SUM, etc.)
    /// rather than being collapsed based on active_dimension matching.
    pub(crate) preserve_wildcards_for_iteration: bool,
    /// When true, ActiveDimRef subscripts are promoted to Wildcard so the full
    /// dimension view is preserved.  This is needed for array-producing builtins
    /// (VectorSortOrder, VectorElmMap, etc.) but NOT for array reducers (SUM,
    /// MEAN, etc.) where ActiveDimRef should resolve to a concrete offset.
    pub(crate) promote_active_dim_ref: bool,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy)]
pub(crate) struct ContextCore<'a> {
    pub(crate) dimensions: &'a [Dimension],
    #[allow(dead_code)]
    pub(crate) dimensions_ctx: &'a DimensionsContext,
    pub(crate) model_name: &'a Ident<Canonical>,
    pub(crate) metadata:
        &'a HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata<'a>>>,
    pub(crate) module_models:
        &'a HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>>,
    pub(crate) inputs: &'a BTreeSet<Ident<Canonical>>,
}

impl<'a> std::ops::Deref for Context<'a> {
    type Target = ContextCore<'a>;

    fn deref(&self) -> &Self::Target {
        &self.core
    }
}

impl Context<'_> {
    pub(crate) fn new<'a>(
        core: ContextCore<'a>,
        ident: &'a Ident<Canonical>,
        is_initial: bool,
    ) -> Context<'a> {
        Context {
            core,
            ident,
            active_dimension: None,
            active_subscript: None,
            is_initial,
            preserve_wildcards_for_iteration: false,
            promote_active_dim_ref: false,
        }
    }

    fn with_active_context(
        &self,
        active_dimension: Option<Arc<[Dimension]>>,
        active_subscript: Option<Vec<CanonicalElementName>>,
    ) -> Self {
        Context {
            core: self.core,
            ident: self.ident,
            active_dimension,
            active_subscript,
            is_initial: self.is_initial,
            preserve_wildcards_for_iteration: self.preserve_wildcards_for_iteration,
            promote_active_dim_ref: self.promote_active_dim_ref,
        }
    }

    pub(crate) fn with_active_subscripts<S: AsRef<str>>(
        &self,
        active_dimension: Arc<[Dimension]>,
        subscripts: &[S],
    ) -> Self {
        self.with_active_context(
            Some(active_dimension),
            Some(
                subscripts
                    .iter()
                    .map(|s| CanonicalElementName::from_raw(s.as_ref()))
                    .collect(),
            ),
        )
    }

    pub(super) fn get_offset(&self, ident: &Ident<Canonical>) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, false)
    }

    /// get_base_offset ignores arrays and should only be used from Var::new and Expr::Subscript
    pub(super) fn get_base_offset(&self, ident: &Ident<Canonical>) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, true)
    }

    pub(super) fn get_metadata(&self, ident: &Ident<Canonical>) -> Result<&VariableMetadata<'_>> {
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

            // SECOND PASS: Check for dimension mapping matches in both directions.
            // Forward: dim has any mapping to an active dimension
            // Reverse: active_dim has any mapping to dim
            let mapping_match_idx = {
                // Forward: dim has mapping to active dim (or active is subdim of mapping target)
                let mut found = active_dims.iter().enumerate().find_map(|(i, candidate)| {
                    if used[i] {
                        return None;
                    }
                    let candidate_name = candidate.canonical_name();
                    if self
                        .dimensions_ctx
                        .has_mapping_to(dim.canonical_name(), candidate_name)
                    {
                        return Some(i);
                    }
                    if self
                        .dimensions_ctx
                        .has_mapping_to_parent_of(dim.canonical_name(), candidate_name)
                    {
                        return Some(i);
                    }
                    None
                });
                // Reverse: active_dim has mapping to dim
                if found.is_none() {
                    found = active_dims.iter().enumerate().find_map(|(i, candidate)| {
                        if used[i] {
                            return None;
                        }
                        if self
                            .dimensions_ctx
                            .has_mapping_to(candidate.canonical_name(), dim.canonical_name())
                        {
                            return Some(i);
                        }
                        None
                    });
                }
                found
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
            // NOTE: The two-pass (name -> size) matching logic is shared with the VM via
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
        let active_dims = self.active_dimension.as_ref().unwrap();

        let mut off = 0_usize;
        for (dim, subscript) in dims.iter().zip(subscripts) {
            let element = CanonicalElementName::from_raw(subscript);
            let element_off = dim.get_offset(&element).or_else(|| {
                // The subscript comes from the active dimension but the source dimension
                // uses different element names. Use dimension mapping to translate.
                for active_dim in active_dims.iter() {
                    if active_dim.get_offset(&element).is_some()
                        && let Some(translated) = self.dimensions_ctx.translate_via_mapping(
                            dim.canonical_name(),
                            active_dim.canonical_name(),
                            &element,
                        )
                    {
                        return dim.get_offset(&translated);
                    }

                    // If dim maps to a parent of the active subdimension, translate through
                    // that mapped parent (active subdimension elements are a subset of parent).
                    if active_dim.get_offset(&element).is_some()
                        && let Some(parent_dim) = self.dimensions_ctx.find_mapping_parent_of(
                            dim.canonical_name(),
                            active_dim.canonical_name(),
                        )
                        && let Some(translated) =
                            self.dimensions_ctx.translate_to_source_via_mapping(
                                dim.canonical_name(),
                                parent_dim,
                                &element,
                            )
                    {
                        return dim.get_offset(&translated);
                    }
                }
                None
            });
            let element_off = element_off.ok_or_else(|| {
                crate::Error::new(
                    ErrorKind::Model,
                    ErrorCode::MismatchedDimensions,
                    Some(format!(
                        "cannot resolve subscript '{}' for dimension '{}' on variable '{}'",
                        subscript,
                        dim.name(),
                        ident
                    )),
                )
            })?;
            off = off * dim.len() + element_off;
        }

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
                // For named dimensions, find the element's position using O(1) hash lookup
                // get_element_index returns 0-based, so add 1 for 1-based subscript offset
                named_dim
                    .get_element_index(subscript.as_str())
                    .map(|off| (off + 1) as f64)
                    .unwrap_or(1.0)
            }
        }
    }

    fn get_submodel_metadata(
        &self,
        model: &Ident<Canonical>,
        ident: &Ident<Canonical>,
    ) -> Result<&VariableMetadata<'_>> {
        let metadata = &self.metadata[model];
        if let Some(pos) = ident.as_str().find('\u{00B7}') {
            let submodel_module_name = &ident.as_str()[..pos];
            let submodel_name = &self.module_models[model]
                [&Ident::<Canonical>::from_str_unchecked(submodel_module_name)];
            let submodel_var = &ident.as_str()[pos + '\u{00B7}'.len_utf8()..];
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
        if let Some(pos) = ident_str.find('\u{00B7}') {
            let submodel_module_name = &ident_str[..pos];
            let submodel_name = &self.module_models[model]
                [&Ident::<Canonical>::from_str_unchecked(submodel_module_name)];
            let submodel_var = &ident_str[pos + '\u{00B7}'.len_utf8()..];
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
    pub(super) fn lower_pass0(&self, expr: &ast::Expr2) -> ast::Expr2 {
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

        // Use three-pass matching (name -> mapping -> size) to correctly handle:
        // 1. Exact name matches (highest priority, reserved first)
        // 2. Dimension mappings (source.maps_to == target or vice versa)
        // 3. Size-based matching for indexed dims (lowest priority)
        //
        // Partial matching supports reductions like SUM(source[A,B])
        // in context [A] where B doesn't match anything.
        let source_to_target = match_dimensions_with_mapping(
            &source_dims,
            active_dims,
            &vec![false; active_dims.len()],
            self.dimensions_ctx,
        );

        source_dims
            .iter()
            .enumerate()
            .map(|(source_idx, _source_dim)| {
                if let Some(target_idx) = source_to_target[source_idx] {
                    let active_dim = &active_dims[target_idx];
                    // Create a dimension reference to the matched active dimension
                    ast::IndexExpr2::Expr(ast::Expr2::Var(Ident::new(active_dim.name()), None, loc))
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
            Quantum(a, b) => Quantum(Box::new(self.lower_pass0(a)), Box::new(self.lower_pass0(b))),
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
            Sshape(a, b, c) => Sshape(
                Box::new(self.lower_pass0(a)),
                Box::new(self.lower_pass0(b)),
                Box::new(self.lower_pass0(c)),
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

            VectorSelect(a, b, c, d, e) => VectorSelect(
                Box::new(self.lower_pass0(a)),
                Box::new(self.lower_pass0(b)),
                Box::new(self.lower_pass0(c)),
                Box::new(self.lower_pass0(d)),
                Box::new(self.lower_pass0(e)),
            ),
            VectorElmMap(a, b) => {
                VectorElmMap(Box::new(self.lower_pass0(a)), Box::new(self.lower_pass0(b)))
            }
            VectorSortOrder(a, b) => {
                VectorSortOrder(Box::new(self.lower_pass0(a)), Box::new(self.lower_pass0(b)))
            }
            AllocateAvailable(a, b, c) => AllocateAvailable(
                Box::new(self.lower_pass0(a)),
                Box::new(self.lower_pass0(b)),
                Box::new(self.lower_pass0(c)),
            ),
            // Single expression builtins replacing stdlib modules
            Previous(a, b) => {
                Previous(Box::new(self.lower_pass0(a)), Box::new(self.lower_pass0(b)))
            }
            Init(e) => Init(Box::new(self.lower_pass0(e))),
        }
    }

    /// Entry point for lowering Expr2 to compiler's Expr representation.
    /// Applies pass 0 -> Expr3 -> pass 1 -> lower_from_expr3.
    /// Returns a Vec<Expr> where the first elements are temp assignments
    /// and the last element is the main expression.
    ///
    /// When A2A context is available (active_dimension and active_subscript set),
    /// pass 1 can resolve Dimension and DimPosition references to concrete indices,
    /// enabling decomposition of expressions that would otherwise be deferred.
    pub(super) fn lower(&self, expr: &ast::Expr2) -> Result<Vec<Expr>> {
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

    /// Lower an expression like `lower()`, but skip resolving Dimension
    /// references in Pass 1.  This keeps `IndexExpr3::Dimension` intact so
    /// that `normalize_subscripts3` can turn them into `ActiveDimRef`, which
    /// `preserve_wildcards_for_iteration` then converts to `Wildcard` inside
    /// array-producing builtins.
    ///
    /// Used when lowering equations from `Ast::Arrayed` that contain
    /// array-producing builtins (VectorElmMap, VectorSortOrder, etc.).
    pub(super) fn lower_preserving_dimensions(&self, expr: &ast::Expr2) -> Result<Vec<Expr>> {
        let normalized = self.lower_pass0(expr);

        let expr3 = Expr3::from_expr2(&normalized, self).map_err(|e| Error {
            kind: ErrorKind::Model,
            code: e.code,
            details: Some(format!("Error at {}:{}", e.start, e.end)),
        })?;

        // Use Pass1Context WITHOUT A2A context so Dimension references
        // are preserved (not resolved to concrete element indices).
        let mut pass1_ctx = Pass1Context::new();
        let transformed = pass1_ctx.transform(expr3);
        let assignments = pass1_ctx.take_assignments();

        let mut result: Vec<Expr> = assignments
            .iter()
            .map(|a| self.lower_from_expr3(a))
            .collect::<Result<Vec<_>>>()?;

        let main_expr = self.lower_from_expr3(&transformed)?;
        result.push(main_expr);

        Ok(result)
    }

    pub(super) fn fold_flows(&self, flows: &[Ident<Canonical>]) -> Result<Option<Expr>> {
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
    fn get_variable_metadata_by_offset(&self, offset: usize) -> Result<&VariableMetadata<'_>> {
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

    pub(super) fn build_stock_update_expr(&self, stock_off: usize, var: &Variable) -> Result<Expr> {
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
        let var_metadata = metadata.get(&*canonicalize(ident))?;
        var_metadata.var.get_dimensions().map(|dims| dims.to_vec())
    }

    fn is_dimension_name(&self, ident: &str) -> bool {
        let canonical = canonicalize(ident);
        self.dimensions
            .iter()
            .any(|dim| *canonicalize(dim.name()) == *canonical)
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
        let reversed_dims = self.active_dimension.as_ref().map(|active_dims| {
            let mut reversed: Vec<Dimension> = active_dims.iter().cloned().collect();
            reversed.reverse();
            Arc::<[Dimension]>::from(reversed)
        });
        let reversed_subscripts = self.active_subscript.as_ref().map(|active_subs| {
            let mut reversed = active_subs.clone();
            reversed.reverse();
            reversed
        });
        self.with_active_context(reversed_dims, reversed_subscripts)
    }

    /// Create a context that preserves wildcards for array iteration.
    /// Used for array reducer builtins (SUM, MAX, MIN, MEAN, STDDEV, SIZE).
    /// ActiveDimRef subscripts are NOT promoted -- they resolve to a concrete
    /// element offset, so `SUM(matrix[DimA, DimB])` sums over one dimension
    /// while the other iterates.
    fn with_preserved_wildcards(&self) -> Self {
        Context {
            core: self.core,
            ident: self.ident,
            active_dimension: self.active_dimension.clone(),
            active_subscript: self.active_subscript.clone(),
            is_initial: self.is_initial,
            preserve_wildcards_for_iteration: true,
            promote_active_dim_ref: false,
        }
    }

    /// Create a context for array-producing vector builtins (VectorSortOrder,
    /// VectorElmMap, VectorSelect, AllocateAvailable).  Like
    /// `with_preserved_wildcards`, but also promotes ActiveDimRef to Wildcard so
    /// references like `vals[DimA]` inside these builtins keep their full array
    /// view.
    fn with_vector_builtin_wildcards(&self) -> Self {
        Context {
            core: self.core,
            ident: self.ident,
            active_dimension: self.active_dimension.clone(),
            active_subscript: self.active_subscript.clone(),
            is_initial: self.is_initial,
            preserve_wildcards_for_iteration: true,
            promote_active_dim_ref: true,
        }
    }

    /// Lower an Expr3 to compiler's Expr representation.
    /// Handles all Expr3 variants directly, including pass-1 specific variants
    /// (TempArray, AssignTemp, etc.) and common expression types.
    pub(super) fn lower_from_expr3(&self, expr: &Expr3) -> Result<Expr> {
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
                // AssignTemp content was hoisted out of an array reducer
                // (SUM, MEAN, etc.) by Pass 1.  It may contain
                // cross-dimension wildcards (e.g. c[*] with DimA in a
                // DimB context) that must be preserved, so lower in a
                // wildcard-preserving context.
                let lowered_inner = self.with_preserved_wildcards().lower_from_expr3(inner)?;
                Ok(Expr::AssignTemp(*id, Box::new(lowered_inner), view.clone()))
            }

            // Handle common variants directly (no longer converting to Expr2)
            Expr3::Const(_, n, loc) => Ok(Expr::Const(*n, *loc)),

            Expr3::Var(id, _, loc) => {
                // Check if this identifier is a dimension name
                let is_dimension = self
                    .dimensions
                    .iter()
                    .any(|dim| id.as_str() == &*canonicalize(dim.name()));

                if is_dimension {
                    // This is a dimension name
                    if let Some(active_dims) = &self.active_dimension {
                        if let Some(active_subscripts) = &self.active_subscript {
                            // We're in an array context - find the matching dimension
                            for (dim, subscript) in active_dims.iter().zip(active_subscripts.iter())
                            {
                                if id.as_str() == &*canonicalize(dim.name()) {
                                    let index = Self::subscript_to_index(dim, subscript);
                                    return Ok(Expr::Const(index, *loc));
                                }
                            }
                            // Not a direct match -- check dimension mappings.
                            // e.g. s[DimA] = DimB where DimB -> DimA
                            let id_dim_name = CanonicalDimensionName::from_raw(id.as_str());
                            for (dim, subscript) in active_dims.iter().zip(active_subscripts.iter())
                            {
                                let active_name =
                                    CanonicalDimensionName::from_raw(&canonicalize(dim.name()));
                                if self
                                    .dimensions_ctx
                                    .has_mapping_to(&id_dim_name, &active_name)
                                    || self
                                        .dimensions_ctx
                                        .has_mapping_to(&active_name, &id_dim_name)
                                {
                                    // Translate through mapping to find the position in the
                                    // referenced dimension, not the active dimension. This
                                    // matters for reordered element-level mappings.
                                    if let Some(translated) =
                                        self.dimensions_ctx.translate_via_mapping(
                                            &id_dim_name,
                                            &active_name,
                                            subscript,
                                        )
                                        && let Some(id_dim) = self.dimensions_ctx.get(&id_dim_name)
                                    {
                                        let index = Self::subscript_to_index(id_dim, &translated);
                                        return Ok(Expr::Const(index, *loc));
                                    }
                                    return Err(Error::new(
                                        ErrorKind::Model,
                                        ErrorCode::Generic,
                                        Some(format!(
                                            "dimension mapping between '{}' and '{}' exists but could not translate subscript '{}'",
                                            id_dim_name, active_name, subscript
                                        )),
                                    ));
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
                    all_dimensions: self.dimensions,
                    dimensions_ctx: self.dimensions_ctx,
                    active_dimension: self.active_dimension.as_deref(),
                };

                if let Some(mut operations) = normalize_subscripts3(indices, &config) {
                    // In scalar context (no active A2A dimension and not
                    // inside an array-reducing builtin like SUM), resolve
                    // DimPosition(@N) to a concrete Single element offset.
                    // DimPosition normally preserves the dimension for A2A
                    // iteration, but in scalar context @N selects element N.
                    if self.active_dimension.is_none() && !self.preserve_wildcards_for_iteration {
                        for (i, op) in operations.iter_mut().enumerate() {
                            if let IndexOp::DimPosition(pos) = op {
                                let pos_1based = *pos + 1;
                                // pos_1based == 0 is defensive: normalize_subscripts3
                                // already rejects @0, but we guard here too.
                                if pos_1based == 0 || pos_1based > dims[i].len() {
                                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                                }
                                // Convert to 0-based Single index
                                *op = IndexOp::Single(pos_1based - 1);
                            }
                        }
                    }

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
                        active_dimension: self.active_dimension.as_deref(),
                        dimensions_ctx: Some(self.dimensions_ctx),
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

                        // Wildcard, SparseRange, and Range always preserve
                        // dimensions for iteration inside any array builtin.
                        let has_wildcard_ops = operations.iter().any(|op| {
                            matches!(
                                op,
                                IndexOp::Wildcard | IndexOp::SparseRange(_) | IndexOp::Range(_, _)
                            )
                        });
                        // ActiveDimRef should only be promoted to Wildcard inside
                        // array-producing builtins (VectorSortOrder, VectorElmMap,
                        // etc.).  For reducers (SUM, MEAN, etc.) ActiveDimRef
                        // resolves to a concrete offset via build_view_from_ops.
                        let has_active_dim_ref = operations
                            .iter()
                            .any(|op| matches!(op, IndexOp::ActiveDimRef(_)));

                        let preserve_for_iteration = self.preserve_wildcards_for_iteration
                            && (has_wildcard_ops
                                || (self.promote_active_dim_ref && has_active_dim_ref));

                        if has_dim_positions {
                            // Fall through to dynamic handling at the end
                        } else if preserve_for_iteration {
                            // Rebuild view, only promoting ActiveDimRef to Wildcard
                            // when inside an array-producing builtin context
                            let preserved_ops: Vec<IndexOp> = operations
                                .iter()
                                .map(|op| match op {
                                    IndexOp::ActiveDimRef(_) if self.promote_active_dim_ref => {
                                        IndexOp::Wildcard
                                    }
                                    other => other.clone(),
                                })
                                .collect();
                            let preserved_result = build_view_from_ops(
                                &preserved_ops,
                                &orig_dims,
                                &orig_strides,
                                &view_config,
                            )?;
                            return Ok(Expr::StaticSubscript(off, preserved_result.view, *loc));
                        } else {
                            if view.dims.is_empty() {
                                // Inside array-producing builtins, a fully-collapsed
                                // subscript like b[B1] should be promoted back to the
                                // full source array. The Single ops came from named
                                // element subscripts (not ActiveDimRef resolution), so
                                // promoting them to Wildcard restores the array view
                                // that VectorElmMap/VectorSortOrder expect.
                                let has_single_ops = self.promote_active_dim_ref
                                    && operations.iter().any(|op| matches!(op, IndexOp::Single(_)));
                                if has_single_ops {
                                    let promoted_ops: Vec<IndexOp> = operations
                                        .iter()
                                        .map(|op| match op {
                                            IndexOp::Single(_) => IndexOp::Wildcard,
                                            other => other.clone(),
                                        })
                                        .collect();
                                    let promoted_result = build_view_from_ops(
                                        &promoted_ops,
                                        &orig_dims,
                                        &orig_strides,
                                        &view_config,
                                    )?;
                                    return Ok(Expr::StaticSubscript(
                                        off,
                                        promoted_result.view,
                                        *loc,
                                    ));
                                }
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
                                    let source_dim_name =
                                        CanonicalDimensionName::from_raw(name.as_str());

                                    // Check forward mapping: source has any mapping to an active dim
                                    for active_dim_name in active_dim_map.keys() {
                                        let active_canonical =
                                            CanonicalDimensionName::from_raw(active_dim_name);
                                        if self
                                            .dimensions_ctx
                                            .has_mapping_to(&source_dim_name, &active_canonical)
                                        {
                                            return true;
                                        }
                                        if self.dimensions_ctx.has_mapping_to_parent_of(
                                            &source_dim_name,
                                            &active_canonical,
                                        ) {
                                            return true;
                                        }
                                    }

                                    // Check reverse mapping: active_dim has mapping to source
                                    for active_dim_name in active_dim_map.keys() {
                                        let active_canonical =
                                            CanonicalDimensionName::from_raw(active_dim_name);
                                        if self
                                            .dimensions_ctx
                                            .has_mapping_to(&active_canonical, &source_dim_name)
                                        {
                                            return true;
                                        }
                                    }

                                    false
                                })
                                .collect();

                            let all_name_matching = use_name_matching.iter().all(|&b| b);

                            // If all dimensions use name matching, allow broadcasting (fewer dims).
                            // Inside array-producing builtins (promote_active_dim_ref), dimension
                            // mismatches are expected: the source array lives in a different
                            // dimension space than the output (e.g. d[DimA,B1] partially
                            // collapses to DimA-only, which differs from a DimA x DimB output).
                            if !all_name_matching
                                && !self.promote_active_dim_ref
                                && view.dims.len() != active_dims.len()
                            {
                                return sim_err!(MismatchedDimensions, id.as_str().to_string());
                            }

                            // For positional matching, verify sizes match.
                            // Skip when preserving wildcards for iteration (SUM,
                            // MEAN, etc.): the view describes what the reduction
                            // iterates over and is independent of the active
                            // (output) dimensions.  Cross-dimension wildcards
                            // like SUM(c[*]) in a DimB context are valid -- the
                            // reduction iterates over c's DimA regardless of the
                            // output's DimB.
                            if !self.preserve_wildcards_for_iteration {
                                for (view_idx, &view_dim) in view.dims.iter().enumerate() {
                                    if !use_name_matching[view_idx] && view_idx < active_dims.len()
                                    {
                                        // Positional matching - sizes must match
                                        if view_dim != active_dims[view_idx].len() {
                                            return sim_err!(
                                                MismatchedDimensions,
                                                id.as_str().to_string()
                                            );
                                        }
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
                                        // matches via dimension mapping (forward or reverse)
                                        let source_dim_name = CanonicalDimensionName::from_raw(
                                            view_dim_name.as_str(),
                                        );

                                        let mut found = None;
                                        // Forward: source has mapping to active_dim
                                        for (active_dim_name, &(active_idx, subscript)) in
                                            &active_dim_map
                                        {
                                            let active_canonical =
                                                CanonicalDimensionName::from_raw(active_dim_name);
                                            if self
                                                .dimensions_ctx
                                                .has_mapping_to(&source_dim_name, &active_canonical)
                                            {
                                                found = Some((active_idx, subscript));
                                                break;
                                            }
                                            if self.dimensions_ctx.has_mapping_to_parent_of(
                                                &source_dim_name,
                                                &active_canonical,
                                            ) {
                                                found = Some((active_idx, subscript));
                                                break;
                                            }
                                        }
                                        // Reverse: active_dim has mapping to source
                                        if found.is_none() {
                                            for (active_dim_name, &(active_idx, subscript)) in
                                                &active_dim_map
                                            {
                                                let active_canonical =
                                                    CanonicalDimensionName::from_raw(
                                                        active_dim_name,
                                                    );
                                                if self.dimensions_ctx.has_mapping_to(
                                                    &active_canonical,
                                                    &source_dim_name,
                                                ) {
                                                    found = Some((active_idx, subscript));
                                                    break;
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

                                // If source_offset failed, try dimension mapping in
                                // both directions (forward and reverse).
                                let mut mapping_failed = false;
                                if source_offset.is_none() {
                                    let source_dim_name = source_dim.canonical_name();
                                    let target_dim_name = target_dim.canonical_name();

                                    // Use bidirectional translation which handles:
                                    // - Forward: source_dim maps to target_dim
                                    // - Reverse: target_dim maps to source_dim
                                    // - Subdimension: source_dim maps to parent of target_dim
                                    let has_direct_or_reverse_mapping = self
                                        .dimensions_ctx
                                        .has_mapping_to(source_dim_name, target_dim_name)
                                        || self
                                            .dimensions_ctx
                                            .has_mapping_to(target_dim_name, source_dim_name);
                                    let has_parent_mapping = self
                                        .dimensions_ctx
                                        .has_mapping_to_parent_of(source_dim_name, target_dim_name);

                                    if let Some(translated) =
                                        self.dimensions_ctx.translate_via_mapping(
                                            source_dim_name,
                                            target_dim_name,
                                            subscript,
                                        )
                                    {
                                        source_offset = source_dim.get_offset(&translated);
                                    } else if has_parent_mapping {
                                        // Source maps to a parent of target -- find the specific
                                        // parent (not just the first mapping target) and translate
                                        // through it.
                                        let parent_target =
                                            self.dimensions_ctx.find_mapping_parent_of(
                                                source_dim_name,
                                                target_dim_name,
                                            );
                                        if let Some(parent) = parent_target
                                            && let Some(translated) =
                                                self.dimensions_ctx.translate_to_source_via_mapping(
                                                    source_dim_name,
                                                    parent,
                                                    subscript,
                                                )
                                        {
                                            source_offset = source_dim.get_offset(&translated);
                                        } else {
                                            mapping_failed = true;
                                        }
                                    } else if has_direct_or_reverse_mapping {
                                        mapping_failed = true;
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
            BFn::Quantum(a, b) => BuiltinFn::Quantum(
                Box::new(self.lower_from_expr3(a)?),
                Box::new(self.lower_from_expr3(b)?),
            ),
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
            BFn::Sshape(a, b, c) => BuiltinFn::Sshape(
                Box::new(self.lower_from_expr3(a)?),
                Box::new(self.lower_from_expr3(b)?),
                Box::new(self.lower_from_expr3(c)?),
            ),
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
            BFn::Rank(arr, rest) => {
                let ctx = self.with_vector_builtin_wildcards();
                let lowered_arr = Box::new(ctx.lower_from_expr3(arr)?);
                let lowered_rest = match rest {
                    Some((dir, tiebreak)) => {
                        let lowered_dir = Box::new(self.lower_from_expr3(dir)?);
                        let lowered_tiebreak = match tiebreak {
                            Some(tb) => Some(Box::new(self.lower_from_expr3(tb)?)),
                            None => None,
                        };
                        Some((lowered_dir, lowered_tiebreak))
                    }
                    None => None,
                };
                BuiltinFn::Rank(lowered_arr, lowered_rest)
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
            BFn::VectorSelect(sel, expr, max_val, action, err) => {
                let ctx = self.with_vector_builtin_wildcards();
                BuiltinFn::VectorSelect(
                    Box::new(ctx.lower_from_expr3(sel)?),
                    Box::new(ctx.lower_from_expr3(expr)?),
                    Box::new(self.lower_from_expr3(max_val)?),
                    Box::new(self.lower_from_expr3(action)?),
                    Box::new(self.lower_from_expr3(err)?),
                )
            }
            BFn::VectorElmMap(src, offs) => {
                let ctx = self.with_vector_builtin_wildcards();
                BuiltinFn::VectorElmMap(
                    Box::new(ctx.lower_from_expr3(src)?),
                    Box::new(ctx.lower_from_expr3(offs)?),
                )
            }
            BFn::VectorSortOrder(arr, dir) => {
                let ctx = self.with_vector_builtin_wildcards();
                BuiltinFn::VectorSortOrder(
                    Box::new(ctx.lower_from_expr3(arr)?),
                    Box::new(self.lower_from_expr3(dir)?),
                )
            }
            BFn::AllocateAvailable(req, pp, avail) => {
                let ctx = self.with_vector_builtin_wildcards();
                let lowered_req = ctx.lower_from_expr3(req)?;
                // The pp argument needs the full 2D array (requester_dim x XPriority).
                // In Vensim, pp[region, ptype] means "priority_vector starting from ptype",
                // but ALLOCATE AVAILABLE reads all 4 XPriority columns for each requester.
                // Lower pp normally, then expand any single-element-collapsed dimensions
                // back to full wildcards by replacing the view with a full-array view.
                let lowered_pp = ctx.lower_from_expr3(pp)?;
                let lowered_pp = self.expand_pp_view_for_allocate(pp, lowered_pp)?;
                BuiltinFn::AllocateAvailable(
                    Box::new(lowered_req),
                    Box::new(lowered_pp),
                    Box::new(self.lower_from_expr3(avail)?),
                )
            }
            BFn::Previous(a, b) => BuiltinFn::Previous(
                Box::new(self.lower_from_expr3(a)?),
                Box::new(self.lower_from_expr3(b)?),
            ),
            BFn::Init(a) => BuiltinFn::Init(Box::new(self.lower_from_expr3(a)?)),
        })
    }

    /// For ALLOCATE AVAILABLE's pp argument, ensure the full variable array
    /// is accessible.  The Vensim convention pp[requester_dim, ptype] means
    /// "the priority vector starting at ptype", but ALLOCATE AVAILABLE reads
    /// all XPriority columns for each requester.  If lowering produced a
    /// StaticSubscript that collapsed some dimensions (e.g. only region but
    /// not XPriority), replace it with a full-variable view.
    fn expand_pp_view_for_allocate(&self, expr3: &Expr3, lowered: Expr) -> Result<Expr> {
        // Only expand if the lowered expression is a subscripted variable
        // with fewer dimensions than the full variable.
        let (current_view, loc) = match &lowered {
            Expr::StaticSubscript(_, view, loc) => (Some(view), *loc),
            Expr::Var(_, loc) => (None, *loc),
            _ => return Ok(lowered),
        };

        // Find the variable identifier from the Expr3 to look up full dimensions
        let var_ident = match expr3 {
            Expr3::Subscript(id, _, _, _) => id,
            Expr3::Var(id, _, _) => id,
            _ => return Ok(lowered),
        };

        let metadata = match self.get_metadata(var_ident) {
            Ok(m) => m,
            Err(_) => return Ok(lowered),
        };

        let full_dims = match metadata.var.get_dimensions() {
            Some(d) => d,
            None => return Ok(lowered),
        };

        let full_ndims = full_dims.len();
        let current_ndims = current_view.map(|v| v.dims.len()).unwrap_or(0);

        if current_ndims >= full_ndims {
            return Ok(lowered);
        }

        // The view has fewer dimensions than the full variable -- some were
        // collapsed by per-element subscript evaluation.  Rebuild with a
        // full contiguous view because ALLOCATE AVAILABLE needs the complete
        // priority profile array (all requesters' profiles) to perform
        // simultaneous allocation.  Any explicit subscripts that restricted
        // dimensions are intentionally overridden: the allocator requires
        // the full array regardless of the calling element's context.
        let base = self.get_base_offset(var_ident)?;
        let dim_sizes: Vec<usize> = full_dims.iter().map(|d| d.len()).collect();
        let dim_names: Vec<String> = full_dims.iter().map(|d| d.name().to_string()).collect();
        let view = ArrayView::contiguous_with_names(dim_sizes, dim_names);
        Ok(Expr::StaticSubscript(base, view, loc))
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
                let pos_val = *pos as usize;

                // Scalar context: no active A2A dimension, resolve @N directly
                // to a concrete 1-based element offset in the target dimension.
                if self.active_dimension.is_none() {
                    if pos_val == 0 || pos_val > dims[i].len() {
                        return sim_err!(MismatchedDimensions, id.as_str().to_string());
                    }
                    return Ok(SubscriptIndex::Single(Expr::Const(
                        pos_val as f64,
                        *dim_loc,
                    )));
                }

                // A2A context: try to resolve @N via the active subscript at
                // this position (dimension-reordering path, e.g. matrix[@2, @1]).
                // For named dimensions, element names are unique across dimensions,
                // so get_offset reliably distinguishes elements — this also handles
                // subdimension cases (e.g. selected[SubRegion] = data[@1]).
                // For indexed dimensions, numeric element names overlap across
                // unrelated dimensions (e.g. "2" is valid in both X and Y), so
                // get_offset alone can't discriminate the mixed-wildcard case
                // (row[Y] = matrix[@1, *]); we require an exact dimension match.
                let active_subscripts = self.active_subscript.as_ref().unwrap();
                let active_dims = self.active_dimension.as_ref().unwrap();
                let dim = &dims[i];
                let pos_0 = pos_val.saturating_sub(1);
                if pos_0 < active_subscripts.len() {
                    let subscript = &active_subscripts[pos_0];
                    let allow_binding = match dim {
                        Dimension::Named(..) => true,
                        Dimension::Indexed(..) => active_dims.iter().any(|ad| ad == dim),
                    };
                    if allow_binding && let Some(offset) = dim.get_offset(subscript) {
                        return Ok(SubscriptIndex::Single(Expr::Const(
                            (offset + 1) as f64,
                            *dim_loc,
                        )));
                    }
                }

                // A2A fallback for mixed cases (e.g. cube[@1, *, @3]) where
                // the active subscript doesn't match the target dimension.
                // Resolve to a concrete 1-based offset, same as scalar context.
                if pos_val == 0 || pos_val > dims[i].len() {
                    return sim_err!(MismatchedDimensions, id.as_str().to_string());
                }
                Ok(SubscriptIndex::Single(Expr::Const(
                    pos_val as f64,
                    *dim_loc,
                )))
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
                        .any(|d| &*canonicalize(d.name()) == ident.as_str());

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
                            if &*canonicalize(active_dim.name()) == ident.as_str() {
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

                // Find the matching active dimension (direct name match)
                for (active_dim, active_subscript) in active_dims.iter().zip(active_subscripts) {
                    if &*canonicalize(active_dim.name()) == name.as_str() {
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

                // No direct match -- check dimension mappings.
                // The subscript dimension (name) maps to an active dimension, or vice versa.
                let sub_dim_name = CanonicalDimensionName::from_raw(name.as_str());
                for (active_dim, active_subscript) in active_dims.iter().zip(active_subscripts) {
                    let active_dim_name =
                        CanonicalDimensionName::from_raw(&canonicalize(active_dim.name()));
                    let has_forward = self
                        .dimensions_ctx
                        .has_mapping_to(&sub_dim_name, &active_dim_name);
                    let has_reverse = self
                        .dimensions_ctx
                        .has_mapping_to(&active_dim_name, &sub_dim_name);
                    if (has_forward || has_reverse)
                        && let Some(translated) = self.dimensions_ctx.translate_via_mapping(
                            dim.canonical_name(),
                            active_dim.canonical_name(),
                            active_subscript,
                        )
                        && let Some(offset) = dim.get_offset(&translated)
                    {
                        return Ok(SubscriptIndex::Single(Expr::Const(
                            (offset + 1) as f64,
                            *dim_loc,
                        )));
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
                Box::new(Var(Ident::new("true_input"), None, Loc::default())),
                Box::new(Var(Ident::new("false_input"), None, Loc::default())),
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
    let true_var = Variable::Var {
        ident: Ident::new(""),
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
    };
    let false_var = Variable::Var {
        ident: Ident::new(""),
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    metadata.insert(
        Ident::new("true_input"),
        VariableMetadata {
            offset: 7,
            size: 1,
            var: &true_var,
        },
    );
    metadata.insert(
        Ident::new("false_input"),
        VariableMetadata {
            offset: 8,
            size: 1,
            var: &false_var,
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let context = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );
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
                Box::new(Var(Ident::new("true_input"), None, Loc::default())),
                Box::new(Var(Ident::new("false_input"), None, Loc::default())),
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
    let true_var = Variable::Var {
        ident: Ident::new(""),
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
    };
    let false_var = Variable::Var {
        ident: Ident::new(""),
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
    };
    let mut metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    metadata.insert(
        Ident::new("true_input"),
        VariableMetadata {
            offset: 7,
            size: 1,
            var: &true_var,
        },
    );
    metadata.insert(
        Ident::new("false_input"),
        VariableMetadata {
            offset: 8,
            size: 1,
            var: &false_var,
        },
    );
    let mut metadata2 = HashMap::new();
    let main_ident = Ident::new("main");
    let test_ident = Ident::new("test");
    metadata2.insert(main_ident.clone(), metadata);
    let dims_ctx = DimensionsContext::default();
    let context = Context::new(
        ContextCore {
            dimensions: &[],
            dimensions_ctx: &dims_ctx,
            model_name: &main_ident,
            metadata: &metadata2,
            module_models: &module_models,
            inputs,
        },
        &test_ident,
        false,
    );
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

#[test]
fn test_with_active_subscripts_reuses_dimension_storage() {
    use crate::common::CanonicalDimensionName;

    let model_name = Ident::new("main");
    let ident = Ident::new("aux");
    let dims_ctx = DimensionsContext::default();
    let dims = vec![Dimension::Indexed(
        CanonicalDimensionName::from_raw("letters"),
        3,
    )];
    let metadata: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata<'_>>> =
        HashMap::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let inputs = BTreeSet::new();

    let base = Context::new(
        ContextCore {
            dimensions: &dims,
            dimensions_ctx: &dims_ctx,
            model_name: &model_name,
            metadata: &metadata,
            module_models: &module_models,
            inputs: &inputs,
        },
        &ident,
        false,
    );

    let active_dims = Arc::<[Dimension]>::from(dims.clone());
    let ctx_a = base.with_active_subscripts(active_dims.clone(), &["1"]);
    let ctx_b = base.with_active_subscripts(active_dims.clone(), &["2"]);

    assert!(Arc::ptr_eq(
        ctx_a.active_dimension.as_ref().unwrap(),
        ctx_b.active_dimension.as_ref().unwrap()
    ));
    assert_eq!(ctx_a.active_subscript.as_ref().unwrap()[0].as_str(), "1");
    assert_eq!(ctx_b.active_subscript.as_ref().unwrap()[0].as_str(), "2");
}

#[test]
fn test_get_implicit_subscript_off_translates_through_mapping_parent() {
    let dim_a = crate::datamodel::Dimension::named(
        "dima".to_string(),
        vec!["a1".to_string(), "a2".to_string(), "a3".to_string()],
    );
    let sub_a = crate::datamodel::Dimension::named(
        "suba".to_string(),
        vec!["a2".to_string(), "a3".to_string()],
    );
    let mut dim_b = crate::datamodel::Dimension::named(
        "dimb".to_string(),
        vec!["b1".to_string(), "b2".to_string(), "b3".to_string()],
    );
    dim_b.set_maps_to("dima".to_string());

    let dims_ctx = DimensionsContext::from(&[dim_a.clone(), sub_a.clone(), dim_b.clone()]);
    let all_dims = vec![
        Dimension::from(&dim_a),
        Dimension::from(&sub_a),
        Dimension::from(&dim_b),
    ];
    let metadata: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata<'_>>> =
        HashMap::new();
    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let inputs = BTreeSet::new();
    let model_name = Ident::new("main");
    let ident = Ident::new("test_var");

    let base = Context::new(
        ContextCore {
            dimensions: &all_dims,
            dimensions_ctx: &dims_ctx,
            model_name: &model_name,
            metadata: &metadata,
            module_models: &module_models,
            inputs: &inputs,
        },
        &ident,
        false,
    );

    let active_dims = Arc::<[Dimension]>::from(vec![Dimension::from(&sub_a)]);
    let ctx = base.with_active_subscripts(active_dims, &["a2"]);
    let source_dims = vec![Dimension::from(&dim_b)];

    let off = ctx
        .get_implicit_subscript_off(&source_dims, "src")
        .expect("implicit offset should map suba[a2] -> dimb[b2]");
    assert_eq!(off, 1);
}

#[test]
fn test_positional_fallback_ignores_unrelated_mapping() {
    let mut source = crate::datamodel::Dimension::named(
        "source".to_string(),
        vec!["s1".to_string(), "s2".to_string()],
    );
    let target = crate::datamodel::Dimension::named(
        "target".to_string(),
        vec!["t1".to_string(), "t2".to_string()],
    );
    let other = crate::datamodel::Dimension::named(
        "other".to_string(),
        vec!["o1".to_string(), "o2".to_string()],
    );
    // Mapping to an unrelated dimension should not block positional fallback.
    source.set_maps_to("other".to_string());

    let dims_ctx = DimensionsContext::from(&[source.clone(), target.clone(), other.clone()]);
    let all_dims = vec![
        Dimension::from(&source),
        Dimension::from(&target),
        Dimension::from(&other),
    ];

    let source_var = Variable::Var {
        ident: Ident::new("source_var"),
        ast: Some(ast::Ast::ApplyToAll(
            vec![Dimension::from(&source)],
            ast::Expr2::Const("0".to_string(), 0.0, Loc::default()),
        )),
        init_ast: None,
        eqn: None,
        units: None,
        tables: vec![],
        non_negative: false,
        is_flow: false,
        is_table_only: false,
        errors: vec![],
        unit_errors: vec![],
    };

    let mut model_metadata: HashMap<Ident<Canonical>, VariableMetadata<'_>> = HashMap::new();
    model_metadata.insert(
        Ident::new("source_var"),
        VariableMetadata {
            offset: 10,
            size: 2,
            var: &source_var,
        },
    );

    let mut metadata: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, VariableMetadata<'_>>> =
        HashMap::new();
    let model_name = Ident::new("main");
    metadata.insert(model_name.clone(), model_metadata);

    let module_models: HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, Ident<Canonical>>> =
        HashMap::new();
    let inputs = BTreeSet::new();
    let ident = Ident::new("test_var");
    let base = Context::new(
        ContextCore {
            dimensions: &all_dims,
            dimensions_ctx: &dims_ctx,
            model_name: &model_name,
            metadata: &metadata,
            module_models: &module_models,
            inputs: &inputs,
        },
        &ident,
        false,
    );

    let active_dims = Arc::<[Dimension]>::from(vec![Dimension::from(&target)]);
    let ctx = base.with_active_subscripts(active_dims, &["t2"]);
    let expr = Expr3::Subscript(
        Ident::new("source_var"),
        vec![IndexExpr3::StarRange(
            CanonicalDimensionName::from_raw("source"),
            Loc::default(),
        )],
        None,
        Loc::default(),
    );

    let lowered = ctx
        .lower_from_expr3(&expr)
        .expect("positional fallback should resolve source[*] in target context");
    assert_eq!(
        lowered,
        Expr::Var(11, Loc::default()),
        "target element t2 should select the second source element"
    );
}
