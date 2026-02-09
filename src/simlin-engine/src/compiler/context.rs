// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};

use crate::ast::{
    self, ArrayView, BinaryOp, Expr3, Expr3LowerContext, IndexExpr3, Loc, Pass1Context,
};
use crate::common::{
    Canonical, CanonicalElementName, ErrorCode, ErrorKind, Ident, Result, canonicalize,
};
use crate::dimensions::{Dimension, DimensionsContext};
use crate::variable::Variable;
use crate::float::SimFloat;
use crate::{Error, sim_err};

use super::dimensions::{UnaryOp, find_dimension_reordering, match_dimensions_two_pass_partial};
use super::expr::{BuiltinFn, Expr, SubscriptIndex};
use super::subscript::{
    IndexOp, Subscript3Config, ViewBuildConfig, ViewBuildResult, build_view_from_ops,
    normalize_subscripts3,
};

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub(crate) struct VariableMetadata {
    pub(crate) offset: usize,
    pub(crate) size: usize,
    // FIXME: this should be able to be borrowed
    pub(crate) var: Variable,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
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
    pub(super) fn get_offset(&self, ident: &Ident<Canonical>) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, false)
    }

    /// get_base_offset ignores arrays and should only be used from Var::new and Expr::Subscript
    pub(super) fn get_base_offset(&self, ident: &Ident<Canonical>) -> Result<usize> {
        self.get_submodel_offset(self.model_name, ident, true)
    }

    pub(super) fn get_metadata(&self, ident: &Ident<Canonical>) -> Result<&VariableMetadata> {
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
    ) -> Result<&VariableMetadata> {
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
    /// Applies pass 0 -> Expr3 -> pass 1 -> lower_from_expr3.
    /// Returns a Vec<Expr<F>> where the first elements are temp assignments
    /// and the last element is the main expression.
    ///
    /// When A2A context is available (active_dimension and active_subscript set),
    /// pass 1 can resolve Dimension and DimPosition references to concrete indices,
    /// enabling decomposition of expressions that would otherwise be deferred.
    pub(super) fn lower<F: SimFloat>(&self, expr: &ast::Expr2) -> Result<Vec<Expr<F>>> {
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
        let mut result: Vec<Expr<F>> = assignments
            .iter()
            .map(|a| self.lower_from_expr3(a))
            .collect::<Result<Vec<_>>>()?;

        // Lower the main expression
        let main_expr = self.lower_from_expr3(&transformed)?;
        result.push(main_expr);

        Ok(result)
    }

    pub(super) fn fold_flows<F: SimFloat>(&self, flows: &[Ident<Canonical>]) -> Result<Option<Expr<F>>> {
        if flows.is_empty() {
            return Ok(None);
        }

        let loads: Result<Vec<Expr<F>>> = flows
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
    fn apply_dimension_reordering<F: SimFloat>(
        &self,
        expr: Expr<F>,
        reordering: Vec<usize>,
        loc: Loc,
    ) -> Result<Expr<F>> {
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

    pub(super) fn build_stock_update_expr<F: SimFloat>(&self, stock_off: usize, var: &Variable) -> Result<Expr<F>> {
        if let Variable::Stock {
            inflows, outflows, ..
        } = var
        {
            let inflows = self
                .fold_flows(inflows)?
                .unwrap_or(Expr::Const(F::zero(), Loc::default()));
            let outflows = self
                .fold_flows(outflows)?
                .unwrap_or(Expr::Const(F::zero(), Loc::default()));

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
pub struct Pass1Result<F: SimFloat> {
    /// Temp assignments in order of dependency (first should be evaluated first)
    pub assignments: Vec<Expr<F>>,
    /// The main expression (references temps via TempArray)
    pub expr: Expr<F>,
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
    pub(super) fn lower_from_expr3<F: SimFloat>(&self, expr: &Expr3) -> Result<Expr<F>> {
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
            Expr3::Const(_, n, loc) => Ok(Expr::Const(F::from_f64(*n), *loc)),

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
                                    return Ok(Expr::Const(F::from_f64(index), *loc));
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
                                Box::new(Expr::Const(F::zero(), *loc)),
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

    /// Lower a BuiltinFn<Expr3> to BuiltinFn<F> (i.e., BuiltinFn<Expr<F>>).
    /// Handles array builtins that need preserve_wildcards_for_iteration.
    fn lower_builtin_expr3<F: SimFloat>(
        &self,
        builtin: &crate::builtins::BuiltinFn<Expr3>,
    ) -> Result<BuiltinFn<F>> {
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
                    .collect::<Result<Vec<Expr<F>>>>();
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
    fn lower_index_expr3<F: SimFloat>(
        &self,
        idx: &IndexExpr3,
        id: &Ident<Canonical>,
        i: usize,
        dims: &[Dimension],
        _orig_dims: &[usize],
        _loc: Loc,
    ) -> Result<SubscriptIndex<F>> {
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
                                    F::from_usize(subscript_off + 1),
                                    *star_loc,
                                )));
                            } else if let Dimension::Indexed(_, _) = dim
                                && let Ok(idx_val) = active_subscript.as_str().parse::<usize>()
                            {
                                return Ok(SubscriptIndex::Single(Expr::Const(
                                    F::from_usize(idx_val),
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
                let start_expr = Expr::Const(F::from_usize(*start_0based + 1), *loc);
                let end_expr = Expr::Const(F::from_usize(*end_0based), *loc);
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
                        F::from_usize(offset + 1),
                        *dim_loc,
                    )))
                } else if let Ok(idx_val) = subscript.as_str().parse::<usize>() {
                    Ok(SubscriptIndex::Single(Expr::Const(
                        F::from_usize(idx_val),
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
                            F::from_usize(offset + 1),
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
                                    F::from_usize(idx), *var_loc,
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
                                        F::from_usize(offset + 1),
                                        *var_loc,
                                    )));
                                } else if let Ok(idx_val) =
                                    active_subscript.as_str().parse::<usize>()
                                {
                                    return Ok(SubscriptIndex::Single(Expr::Const(
                                        F::from_usize(idx_val),
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
                        F::from_usize(offset + 1),
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
                                F::from_usize(offset + 1),
                                *dim_loc,
                            )));
                        } else if let Ok(idx_val) = active_subscript.as_str().parse::<usize>() {
                            return Ok(SubscriptIndex::Single(Expr::Const(
                                F::from_usize(idx_val),
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

    let output = context.lower::<f64>(&input);
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

    let output = context.lower::<f64>(&input);
    assert!(output.is_ok());
    let mut output_exprs = output.unwrap();
    // The last element is the main expression
    assert_eq!(expected, output_exprs.pop().unwrap());
}
