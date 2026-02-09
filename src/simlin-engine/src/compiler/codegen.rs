// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
use std::sync::Arc;

use smallvec::SmallVec;

use crate::ast::{ArrayView, BinaryOp};
use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeBuilder, ByteCodeContext, CompiledInitial, CompiledModule, DimId,
    DimListId, DimensionInfo, GraphicalFunctionId, LookupMode, ModuleDeclaration, ModuleId,
    ModuleInputOffset, NameId, Op2, Opcode, RuntimeSparseMapping, StaticArrayView,
    SubdimensionRelation, TempId, VariableOffset, ViewId,
};
use crate::common::{Canonical, ErrorCode, ErrorKind, Ident, Result, canonicalize};
use crate::dimensions::Dimension;
use crate::sim_err;
use crate::vm::{DT_OFF, FINAL_TIME_OFF, INITIAL_TIME_OFF, TIME_OFF};

use super::Module;
use super::dimensions::UnaryOp;
use super::expr::{BuiltinFn, Expr, SubscriptIndex};

pub(super) struct Compiler<'module> {
    module: &'module Module,
    module_decls: Vec<ModuleDeclaration>,
    graphical_functions: Vec<Vec<(f64, f64)>>,
    /// Maps table variable names to their base index in graphical_functions.
    /// For subscripted lookups, the actual table is at base_id + element_offset.
    table_base_ids: HashMap<Ident<Canonical>, GraphicalFunctionId>,
    curr_code: ByteCodeBuilder,
    // Array support fields
    pub(super) dimensions: Vec<DimensionInfo>,
    pub(super) subdim_relations: Vec<SubdimensionRelation>,
    names: Vec<String>,
    static_views: Vec<StaticArrayView>,
    dim_lists: Vec<(u8, [u16; 4])>,
    // Iteration context - set when compiling inside AssignTemp
    in_iteration: bool,
    /// When in optimized iteration mode, maps pre-pushed views to their stack offset.
    /// Each entry is (StaticArrayView, stack_offset) where stack_offset is 1-based from top.
    /// The output view is always at offset (n_source_views + 1).
    iter_source_views: Option<Vec<(StaticArrayView, u8)>>,
}

impl<'module> Compiler<'module> {
    pub(super) fn new(module: &'module Module) -> Compiler<'module> {
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
            dim_lists: Vec::new(),
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
    pub(super) fn get_or_add_subdim_relation(
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
                let dim_list_id = self.dim_lists.len() as DimListId;
                self.dim_lists.push((n_dims, dims));
                self.push(Opcode::PushVarViewDirect {
                    base_off: *off as u16,
                    dim_list_id,
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
                                    crate::Error::new(
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
                                    crate::Error::new(
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
                                    crate::Error::new(
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
                        crate::Error::new(
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
                        crate::Error::new(
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
                if let Expr::Const(value, _) = rhs.as_ref() {
                    let id = self.curr_code.push_named_literal(*value);
                    self.push(Opcode::AssignConstCurr {
                        off: *off as VariableOffset,
                        literal_id: id,
                    });
                } else {
                    self.walk_expr(rhs)?.unwrap();
                    self.push(Opcode::AssignCurr {
                        off: *off as VariableOffset,
                    });
                }
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

    pub(super) fn compile(mut self) -> Result<CompiledModule> {
        // Compile each variable's initials separately
        let compiled_initials: Vec<CompiledInitial> = self
            .module
            .runlist_initials_by_var
            .iter()
            .map(|var_init| {
                let bytecode = self.walk(&var_init.ast)?;
                Ok(CompiledInitial {
                    ident: var_init.ident.clone(),
                    offsets: var_init.offsets.clone(),
                    bytecode,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let compiled_initials = Arc::new(compiled_initials);

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
                dimensions: self.dimensions,
                subdim_relations: self.subdim_relations,
                names: self.names,
                static_views: self.static_views,
                temp_offsets,
                temp_total_size,
                dim_lists: self.dim_lists,
            }),
            compiled_initials,
            compiled_flows,
            compiled_stocks,
        })
    }
}
