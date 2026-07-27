// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::ast::{ArrayView, BinaryOp};
use crate::bytecode::{
    BuiltinId, DimId, DimListId, DimensionInfo, GraphicalFunctionId, LookupMode, ModuleId,
    ModuleInputOffset, NameId, Op2, RuntimeSparseMapping, SubdimensionRelation, TempId,
    VariableOffset, ViewId,
};
use crate::common::{Canonical, ErrorCode, ErrorKind, Ident, Result, canonicalize};
use crate::dimensions::Dimension;
use crate::sim_err;
use crate::vm::{DT_OFF, FINAL_TIME_OFF, INITIAL_TIME_OFF, TIME_OFF};

use super::dimensions::UnaryOp;
use super::expr::{BuiltinFn, Expr, SubscriptIndex, Table, VarRef};
use super::symbolic::{
    SymStaticViewBase, SymbolicByteCode, SymbolicByteCodeBuilder, SymbolicCompiledInitial,
    SymbolicCompiledModule, SymbolicModuleDecl, SymbolicOpcode, SymbolicStaticView,
};
use super::{Var, VarSizes};

/// Everything `Compiler` reads, borrowed for the duration of one emission.
///
/// This is the compiler's *whole* input contract, and there is exactly one
/// codegen behind it. Two very different callers build one:
///
/// * [`super::Module::compile`] -- the test-only monolithic whole-model path,
///   which borrows every field straight off its owned `Module` and then
///   resolves the emitted symbolic module against its own layout;
/// * `db::assemble::compile_phase_to_per_var_bytecodes` -- the production
///   per-variable fragment compiler, which borrows the salsa-cached
///   project-global dimension context and converted dimensions plus the
///   variable's own lowered expressions, and keeps the emitted fragment
///   symbolic until assembly.
///
/// Borrowing rather than owning is the point (GH #964 / #655): the fragment
/// compiler runs once per variable *per phase* -- tens of thousands of times
/// on an LTM-heavy model -- and the stand-in one-variable `Module` it used to
/// build by struct literal deep-cloned `dimensions_ctx`, `dimensions`,
/// `tables`, `offsets` and the phase's whole lowered `Vec<Expr>` on every one
/// of them. Nothing in codegen mutates any of it, so a reference is the
/// honest type.
///
/// The field set is deliberately minimal: it is what codegen actually reads.
/// In particular there is no offset map and no slot count. Codegen emits
/// `VarRef`s straight through from the lowered expressions, so the only thing
/// it still needs from the symbol table is `var_sizes` -- the *extent* of a
/// VECTOR ELM MAP source variable, which is a property of the variable and not
/// of where it lives.
#[derive(Clone, Copy)]
pub(crate) struct ModuleCtx<'a> {
    pub(crate) ident: &'a Ident<Canonical>,
    /// The module-instance input set, consulted only by `isModuleInput(x)`.
    pub(crate) inputs: &'a std::collections::BTreeSet<Ident<Canonical>>,
    /// Per-temp element counts, indexed by `TempId`. Its length is the temp
    /// count; there is no separate `n_temps`, which could only ever disagree.
    pub(crate) temp_sizes: &'a [usize],
    pub(crate) runlist_initials_by_var: &'a [Var],
    pub(crate) runlist_flows: &'a [Expr],
    pub(crate) runlist_stocks: &'a [Expr],
    /// Canonical variable name -> total slot count, for this model. The sole
    /// reader is [`Compiler::full_source_len`].
    pub(crate) var_sizes: &'a VarSizes,
    pub(crate) tables: &'a HashMap<Ident<Canonical>, Vec<Table>>,
    pub(crate) dimensions: &'a [Dimension],
    pub(crate) dimensions_ctx: &'a crate::dimensions::DimensionsContext,
}

impl<'a> ModuleCtx<'a> {
    /// Emit this unit's bytecode. The single entry point into codegen.
    pub(crate) fn compile(self) -> Result<SymbolicCompiledModule> {
        Compiler::new(self).compile()
    }
}

pub(super) struct Compiler<'module> {
    module: ModuleCtx<'module>,
    module_decls: Vec<SymbolicModuleDecl>,
    graphical_functions: Vec<Vec<(f64, f64)>>,
    /// Maps table variable names to their base index in graphical_functions.
    /// For subscripted lookups, the actual table is at base_id + element_offset.
    table_base_ids: HashMap<Ident<Canonical>, GraphicalFunctionId>,
    curr_code: SymbolicByteCodeBuilder,
    // Array support fields
    pub(super) dimensions: Vec<DimensionInfo>,
    pub(super) subdim_relations: Vec<SubdimensionRelation>,
    names: Vec<String>,
    /// Hash index over `names` so interning is O(1) amortized. The compiler
    /// runs once per per-variable fragment (tens of thousands of times on
    /// large LTM builds), and `Compiler::new` interns every dimension and
    /// element name up front -- with a linear-scan intern that was O(D^2)
    /// string comparisons per fragment (GH #655).
    name_ids: HashMap<String, NameId>,
    static_views: Vec<SymbolicStaticView>,
    dim_lists: Vec<(u8, [u16; 4])>,
    // Iteration context - set when compiling inside AssignTemp
    in_iteration: bool,
    /// When in optimized iteration mode, maps pre-pushed views to their stack offset.
    /// Each entry is (SymbolicStaticView, stack_offset) where stack_offset is 1-based from top.
    /// The output view is always at offset (n_source_views + 1).
    iter_source_views: Option<Vec<(SymbolicStaticView, u8)>>,
}

impl<'module> Compiler<'module> {
    pub(super) fn new(module: ModuleCtx<'module>) -> Compiler<'module> {
        // Pre-populate graphical_functions with all tables and record base IDs.
        //
        // Iterated in sorted ident order, NOT `HashMap` order, and that is
        // load-bearing rather than cosmetic. This loop assigns both the layout
        // of `graphical_functions` and every `base_gf` operand the emitted
        // `Lookup`/`LookupArray` opcodes carry, so with `HashMap` order a model
        // whose fragment holds two or more table-bearing variables compiled to
        // a *different* (still self-consistent, still numerically correct)
        // bytecode on every run. `PerVarBytecodes` is a salsa-cached value with
        // a derived `PartialEq`, so that defeats backdating exactly as an
        // unordered `temp_sizes` did (`db::assemble::temp_sizes_by_id`), and the
        // assembled `CompiledModule` was not reproducible: measured differing on
        // 18 of 23 fresh-database repeats. It reaches shipped models --
        // `test/metasd/theil-statistics/Theil_2011.mdl` compiles a fragment
        // holding `["dummy_data", "dummy_simulation"]`.
        //
        // Nothing downstream depends on WHICH order is chosen, only that a
        // fragment's `base_gf` operands agree with its own
        // `graphical_functions` vector and that distinct variables' blocks stay
        // disjoint -- both of which sorting preserves. Checked: the VM reads
        // `graphical_functions[base_gf + element_offset]` self-relatively;
        // `resolve` passes `base_gf`/`table_count` through untouched (they
        // are table indices, not layout offsets); and
        // `symbolic::gf_blocks_of_fragment` derives its blocks from the
        // fragment's own opcode runs (sorting them itself) while
        // `FragmentMerger::absorb_gf` dedups on block CONTENT, so #582's
        // cross-fragment GF dedup is order-insensitive by construction.
        let mut graphical_functions = Vec::new();
        let mut table_base_ids = HashMap::new();

        let mut table_idents: Vec<&Ident<Canonical>> = module.tables.keys().collect();
        table_idents.sort_unstable();
        for ident in table_idents {
            let base_gf = graphical_functions.len() as GraphicalFunctionId;
            table_base_ids.insert(ident.clone(), base_gf);
            for table in &module.tables[ident] {
                graphical_functions.push(table.data.clone());
            }
        }

        let mut compiler = Compiler {
            module,
            module_decls: vec![],
            graphical_functions,
            table_base_ids,
            curr_code: SymbolicByteCodeBuilder::default(),
            dimensions: vec![],
            subdim_relations: vec![],
            names: vec![],
            name_ids: HashMap::new(),
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
        for dim in self.module.dimensions {
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
        if let Some(&id) = self.name_ids.get(name) {
            return id;
        }
        let id = self.names.len() as NameId;
        self.names.push(name.to_string());
        self.name_ids.insert(name.to_string(), id);
        id
    }

    /// Get or create a DimId for a dimension with the given name and size.
    /// If a dimension with the same name exists, returns its DimId (assumes same size).
    fn get_or_add_dim_id(&mut self, dim_name: &str, size: u16) -> DimId {
        // Look for existing dimension with the same name
        if let Some(&name_id) = self.name_ids.get(dim_name)
            && let Some(dim_idx) = self.dimensions.iter().position(|d| d.name_id == name_id)
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
        let name_id = *self.name_ids.get(dim_name)?;
        let dim_idx = self.dimensions.iter().position(|d| d.name_id == name_id)?;
        Some(dim_idx as DimId)
    }

    /// Add a static view and return its ViewId
    fn add_static_view(&mut self, view: SymbolicStaticView) -> ViewId {
        self.static_views.push(view);
        (self.static_views.len() - 1) as ViewId
    }

    /// Total element count of the source *variable* referenced by an
    /// expression, i.e. the product of its full declared dimensions. This is
    /// the genuine-Vensim VECTOR ELM MAP out-of-range bound (`:NA:` is
    /// returned for an offset that would map outside the source variable's
    /// full storage).
    ///
    /// The reference carries the owning variable's name, so this is a direct
    /// `var_sizes` lookup. It applies only to a reference that addresses the
    /// variable *in whole*: a reference starting mid-variable names one element
    /// of a bigger array, or one slot inside a module instance, and the full
    /// extent of the thing it names is not knowable from the enclosing model's
    /// symbol table. Those fall back to the lowered view's element count, which
    /// is the correct full extent for the non-sliced shapes -- exactly what the
    /// previous "does any variable's storage begin at this offset?" scan did,
    /// since a variable's slot range contains no other variable's base.
    fn full_source_len(&self, source: &Expr) -> u32 {
        let (base, view_len) = match source {
            Expr::StaticSubscript(base, view, _) => {
                (Some(base), view.dims.iter().product::<usize>().max(1))
            }
            // A *dynamic* subscript (`arr[i]`, `i` a variable) is the same
            // shape as `StaticSubscript` for this purpose -- the source is
            // still the whole `arr` variable, only the base element is chosen
            // at runtime. Without this arm it fell to the `_` case and reported
            // a full extent of 1, so `VECTOR ELM MAP(arr[i], offsets)` returned
            // NaN for every element whenever `i` selected anything but `arr`'s
            // first element, and for any non-zero offset at all.
            //
            // No wasm-backend parity test accompanies the VM regression test
            // (`array_tests::…::elm_map_dynamic_source_subscript_uses_full_variable_extent_vm`)
            // because the backends cannot diverge here: `wasmgen::vector`
            // rejects a dynamically-subscripted ELM MAP source outright
            // (`source_view.runtime_off_local.is_some()` =>
            // `WasmGenError::Unsupported`), so this shape never reaches wasm
            // lowering at all.
            Expr::Subscript(base, _, bounds, _) => {
                (Some(base), bounds.iter().product::<usize>().max(1))
            }
            Expr::Var(base, _) => (Some(base), 1usize),
            Expr::TempArray(_, view, _) => (None, view.dims.iter().product::<usize>().max(1)),
            _ => (None, 1usize),
        };

        if let Some(base) = base
            && base.is_whole_var()
            && let Some(size) = self.module.var_sizes.get(&base.name)
        {
            return *size as u32;
        }
        view_len as u32
    }

    /// Convert an ArrayView to a SymbolicStaticView for a variable
    fn array_view_to_static(&mut self, base: &VarRef, view: &ArrayView) -> SymbolicStaticView {
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

        SymbolicStaticView {
            base: SymStaticViewBase::Var(base.clone()),
            dims: view.dims.iter().map(|&d| d as u16).collect(),
            strides: view.strides.iter().map(|&s| s as i32).collect(),
            offset: view.offset as u32,
            sparse,
            dim_ids,
        }
    }

    /// Convert an ArrayView to a SymbolicStaticView for a temp array
    fn array_view_to_static_temp(&mut self, temp_id: u32, view: &ArrayView) -> SymbolicStaticView {
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

        // Temp arrays are always written into compact scratch storage:
        // `AssignTemp` writes `temp_off + current`, and array-producing
        // builtins write `temp_off + orig_idx`. Preserve the shape and
        // dimension IDs for broadcasting, but do not preserve source-view
        // offset/strides/sparse metadata from the expression that produced
        // the temp.
        let mut strides: SmallVec<[i32; 4]> = view.dims.iter().map(|_| 1).collect();
        let mut stride = 1i32;
        for i in (0..view.dims.len()).rev() {
            strides[i] = stride;
            stride *= view.dims[i] as i32;
        }

        SymbolicStaticView {
            base: SymStaticViewBase::Temp(temp_id),
            dims: view.dims.iter().map(|&d| d as u16).collect(),
            strides,
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids,
        }
    }

    /// Resolve `(base_gf, table_count)` for a *per-element arrayed graphical
    /// function* whose full array is referenced by `table_expr` (a bare `Var`
    /// or a whole-array `StaticSubscript`). This is the array counterpart of
    /// the per-element resolution the scalar `Lookup` codegen does via
    /// `extract_table_info`, but here the base is intentionally the *whole*
    /// array (`g[D!]`, `view.size() > 1`) -- the very shape `extract_table_info`
    /// rejects -- because `LookupArray` evaluates every element's table. The
    /// table base id and per-element table count come from the same
    /// `table_base_ids` / `module.tables` maps the scalar lookup uses, so the
    /// per-element table layout is identical. A `table_expr` that is neither a
    /// recognised array base nor a GF-bearing variable yields a precise
    /// `BadTable` (loud-safe: an un-reconstructable arrayed-GF dependency must
    /// never become a silent stub -- GH #580 / AC7.5).
    fn arrayed_lookup_table_info(&self, table_expr: &Expr) -> Result<(GraphicalFunctionId, u16)> {
        let base = match table_expr {
            // Whole-array reference: the view spans the full array, so the
            // reference sits at the variable's base.
            Expr::StaticSubscript(base, _, _) | Expr::Var(base, _) => base,
            other => {
                return sim_err!(
                    BadTable,
                    format!(
                        "arrayed graphical-function apply expected a whole-array \
                         base, got {:?}",
                        std::mem::discriminant(other)
                    )
                );
            }
        };
        // A reference into the middle of a variable is not a whole-array base,
        // and never was: the previous exact-base offset scan rejected it too,
        // because a variable's slot range contains no other variable's base.
        if !base.is_whole_var() {
            return sim_err!(
                BadTable,
                "arrayed graphical-function apply expected a whole-array base, got an \
                 element reference"
                    .to_string()
            );
        }
        let table_ident = base.name.clone();
        let base_gf = *self.table_base_ids.get(&table_ident).ok_or_else(|| {
            crate::Error::new(
                ErrorKind::Simulation,
                ErrorCode::BadTable,
                Some(format!(
                    "no graphical function found for arrayed lookup '{table_ident}'"
                )),
            )
        })?;
        let table_count = self
            .module
            .tables
            .get(&table_ident)
            .map(|tables| tables.len() as u16)
            .unwrap_or(1);
        Ok((base_gf, table_count))
    }

    /// Emit bytecode to push an expression's view onto the view stack.
    /// This is used for array operations that need to iterate over arrays.
    fn walk_expr_as_view(&mut self, expr: &Expr) -> Result<()> {
        match expr {
            Expr::StaticSubscript(base, view, _) => {
                // Create a static view and push it
                let static_view = self.array_view_to_static(base, view);
                let view_id = self.add_static_view(static_view);
                self.push(SymbolicOpcode::PushStaticView { view_id });
                Ok(())
            }
            Expr::TempArray(id, view, _) => {
                // Create a static view for the temp array and push it
                let static_view = self.array_view_to_static_temp(*id, view);
                let view_id = self.add_static_view(static_view);
                self.push(SymbolicOpcode::PushStaticView { view_id });
                Ok(())
            }
            Expr::Var(base, _) => {
                // A bare variable reference used as an array - create a scalar view
                // This shouldn't normally happen for array operations, but handle it
                let view = ArrayView::contiguous(vec![1]);
                let static_view = self.array_view_to_static(base, &view);
                let view_id = self.add_static_view(static_view);
                self.push(SymbolicOpcode::PushStaticView { view_id });
                Ok(())
            }
            Expr::Subscript(base, indices, bounds, _) => {
                // Dynamic subscript with potential range indices
                // First, push a full view for the base array using explicit bounds
                let n_dims = bounds.len().min(4) as u8;
                let mut dims = [0u16; 4];
                for (i, &bound) in bounds.iter().take(4).enumerate() {
                    dims[i] = bound as u16;
                }
                let dim_list_id = self.dim_lists.len() as DimListId;
                self.dim_lists.push((n_dims, dims));
                self.push(SymbolicOpcode::PushVarViewDirect {
                    var: base.clone(),
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
                            // Propagate a recoverable codegen Err via `?` (matching
                            // the scalar `Subscript` arm of `walk_expr`): an index
                            // expression can fail to lower -- e.g. a
                            // `PREVIOUS(SUM(...))` partial the LTM ceteris-paribus
                            // path emits, whose inner reference survives helper
                            // rewriting as a non-variable expression
                            // (`NotSimulatable`). That Err must flow back to the
                            // caller (`db/ltm/compile.rs`'s `module.compile()` ->
                            // `Err(_) => None` gracefully drops the un-compilable
                            // LTM synthetic fragment), never escalate to a
                            // process-killing panic (#363, GH #541/#525). The
                            // Option unwrap stays a hard unwrap: an index that
                            // lowered to no value-producing opcode (`Ok(None)` --
                            // only an `EvalModule` or a non-iteration `TempArray`)
                            // is a genuine compiler invariant violation, never
                            // reachable from a real subscript index.
                            self.walk_expr(expr)?.unwrap();
                            self.push(SymbolicOpcode::ViewSubscriptDynamic {
                                dim_idx: effective_dim,
                            });
                            singles_processed += 1; // Track collapse for subsequent indices
                        }
                        SubscriptIndex::Range(start, end) => {
                            // Same propagation contract as the Single arm above:
                            // a range bound can carry a recoverable lowering Err.
                            self.walk_expr(start)?.unwrap();
                            self.walk_expr(end)?.unwrap();
                            self.push(SymbolicOpcode::ViewRangeDynamic {
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

    /// Emit the array-reduce pattern: push view, emit reduction opcode, pop view.
    /// Used by SUM, SIZE, STDDEV, MIN (1-arg), MAX (1-arg), and MEAN (1-arg).
    fn emit_array_reduce(&mut self, arg: &Expr, opcode: SymbolicOpcode) -> Result<Option<()>> {
        self.walk_expr_as_view(arg)?;
        self.push(opcode);
        self.push(SymbolicOpcode::PopView {});
        Ok(Some(()))
    }

    fn walk(&mut self, exprs: &[Expr]) -> Result<SymbolicByteCode> {
        for expr in exprs.iter() {
            self.walk_expr(expr)?;
        }
        self.push(SymbolicOpcode::Ret);

        let curr = std::mem::take(&mut self.curr_code);

        Ok(curr.finish())
    }

    fn walk_expr(&mut self, expr: &Expr) -> Result<Option<()>> {
        let result = match expr {
            Expr::Const(value, _) => {
                let id = self.curr_code.intern_literal(*value);
                self.push(SymbolicOpcode::LoadConstant { id });
                Some(())
            }
            Expr::Var(var, _) => {
                self.push(SymbolicOpcode::LoadVar { var: var.clone() });
                Some(())
            }
            Expr::Subscript(base, indices, bounds, _) => {
                // For scalar access (old-style Subscript), all indices must be Single
                for (i, idx) in indices.iter().enumerate() {
                    match idx {
                        SubscriptIndex::Single(expr) => {
                            // Propagate via `?` (matching the sibling walk_expr
                            // call sites at the LookupForward/Backward and
                            // PREVIOUS/INIT arms): a recoverable codegen Err here
                            // -- e.g. a PREVIOUS whose arg survived helper
                            // rewriting as a non-variable expression
                            // (NotSimulatable) -- must flow back to the caller
                            // (db/ltm/compile.rs gracefully drops the un-compilable
                            // LTM synthetic fragment), never escalate to a panic (#363).
                            self.walk_expr(expr)?.unwrap();
                            let bounds = bounds[i] as VariableOffset;
                            self.push(SymbolicOpcode::PushSubscriptIndex { bounds });
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
                self.push(SymbolicOpcode::LoadSubscript { var: base.clone() });
                Some(())
            }
            Expr::StaticSubscript(base, view, _) => {
                if self.in_iteration {
                    // In iteration context with optimized view hoisting
                    let static_view = self.array_view_to_static(base, view);

                    let offset = self.find_iter_view_offset(&static_view).unwrap_or_else(|| {
                        unreachable!(
                            "StaticSubscript view not found in pre-pushed set - \
                             collect_iter_source_views_impl and walk_expr should visit same nodes"
                        )
                    });
                    self.push(SymbolicOpcode::LoadIterViewAt { offset });
                    Some(())
                } else if view.dims.iter().product::<usize>() == 1 {
                    // Scalar result - the view has collapsed to one element, so
                    // read it directly.
                    self.push(SymbolicOpcode::LoadVar {
                        var: base.offset_by(view.offset),
                    });
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
                    self.push(SymbolicOpcode::LoadIterViewAt { offset });
                    Some(())
                } else {
                    // Outside iteration - push temp view for subsequent operations (like SUM)
                    let static_view = self.array_view_to_static_temp(*id, view);
                    let view_id = self.add_static_view(static_view);
                    self.push(SymbolicOpcode::PushStaticView { view_id });
                    // Note: caller (like array builtin) will use and pop this view
                    None
                }
            }
            Expr::TempArrayElement(id, _view, idx, _) => {
                // Load a specific element from a temp array
                self.push(SymbolicOpcode::LoadTempConst {
                    temp_id: *id as TempId,
                    index: *idx as u16,
                });
                Some(())
            }
            Expr::Dt(_) => {
                self.push(SymbolicOpcode::LoadGlobalVar {
                    off: DT_OFF as VariableOffset,
                });
                Some(())
            }
            Expr::App(builtin, _) => {
                // Helper to extract table info from table expression.
                //
                // The table's identity and the element within it both come
                // straight off the reference: `name` is the variable that owns
                // the slot and `element_offset` is the element within it --
                // which is what the offset-range scan this used to do
                // reconstructed. A reference whose base is not the variable's
                // own (a cross-module `m·x`, whose owner in this model is the
                // module variable) can only fail the `table_base_ids` lookup
                // below, exactly as the scan's exact-base match used to fail.
                fn extract_table_info(table_expr: &Expr) -> Result<(Ident<Canonical>, Expr)> {
                    match table_expr {
                        Expr::Var(var, loc) => {
                            // Either a scalar table or an element of an arrayed
                            // table (a static subscript that compiled down to a
                            // direct element reference).
                            Ok((
                                var.name.clone(),
                                Expr::Const(var.element_offset as f64, *loc),
                            ))
                        }
                        Expr::StaticSubscript(base, view, loc) => {
                            // Static subscript - element offset is precomputed in the ArrayView
                            // Reject ranges/wildcards - only single element selection is valid
                            if view.size() > 1 {
                                return sim_err!(
                                    BadTable,
                                    "range subscripts not supported in lookup tables".to_string()
                                );
                            }
                            Ok((
                                base.name.clone(),
                                Expr::Const((base.element_offset + view.offset) as f64, *loc),
                            ))
                        }
                        Expr::Subscript(base, subscript_indices, dim_sizes, _loc) => {
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

                            // A dynamically-subscripted table reference is always
                            // to the whole table variable: the index expression
                            // selects the element. `base.element_offset` is
                            // therefore 0, and a non-zero one (a cross-module
                            // `m·x`) has no representable table identity here --
                            // the previous exact-base offset scan rejected it too.
                            if !base.is_whole_var() {
                                return sim_err!(
                                    BadTable,
                                    "subscripted lookup table reference must name a \
                                     variable of this model"
                                        .to_string()
                                );
                            }
                            Ok((
                                base.name.clone(),
                                offset_expr.unwrap_or(Expr::Const(0.0, *_loc)),
                            ))
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
                    let (table_ident, element_offset_expr) = extract_table_info(table_expr)?;

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
                    self.push(SymbolicOpcode::Lookup {
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
                    let (table_ident, element_offset_expr) = extract_table_info(table_expr)?;

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
                    self.push(SymbolicOpcode::Lookup {
                        base_gf,
                        table_count,
                        mode,
                    });
                    return Ok(Some(()));
                };

                // so are module builtins
                if let BuiltinFn::IsModuleInput(ident, _loc) = builtin {
                    let id = if self.module.inputs.contains(&*canonicalize(ident)) {
                        self.curr_code.intern_literal(1.0)
                    } else {
                        self.curr_code.intern_literal(0.0)
                    };
                    self.push(SymbolicOpcode::LoadConstant { id });
                    return Ok(Some(()));
                };

                // PREVIOUS(x, init) and INIT(x) compile to dedicated opcodes that
                // read from curr[] (previous timestep) or the initial-value
                // buffer, respectively.  Handle them before the general
                // builtin dispatch because they do not use CallBuiltin.
                //
                // Both opcodes read a fixed slot, so the argument must have
                // resolved to a static location: either a scalar variable
                // (Expr::Var) or a statically-resolved single-element array
                // reference (Expr::StaticSubscript whose view collapsed to a
                // scalar -- e.g. `arr[Dim.elem]` or `arr[2]`). The latter is
                // what the builtins-visitor lets through instead of
                // synthesizing a helper aux when every subscript index is a
                // compile-time constant. Anything else (dynamic indices,
                // expressions) was rewritten through a helper aux at parse
                // time, so reaching here with one is a compiler bug.
                let static_slot = |arg: &Expr| -> Option<VarRef> {
                    match arg {
                        Expr::Var(var, _) => Some(var.clone()),
                        Expr::StaticSubscript(base, view, _) if view.dims.is_empty() => {
                            Some(base.offset_by(view.offset))
                        }
                        _ => None,
                    }
                };
                match builtin {
                    BuiltinFn::Previous(arg, fallback) => {
                        self.walk_expr(fallback)?.unwrap();
                        match static_slot(arg.as_ref()) {
                            Some(var) => {
                                self.push(SymbolicOpcode::SymLoadPrev { var });
                            }
                            None => {
                                return sim_err!(
                                    NotSimulatable,
                                    "PREVIOUS requires a variable reference after helper rewriting"
                                        .to_string()
                                );
                            }
                        }
                        return Ok(Some(()));
                    }
                    BuiltinFn::Init(arg) => {
                        let var = match static_slot(arg.as_ref()) {
                            Some(var) => var,
                            None => {
                                return sim_err!(
                                    NotSimulatable,
                                    "INIT requires a variable reference argument".to_string()
                                );
                            }
                        };
                        self.push(SymbolicOpcode::SymLoadInitial { var });
                        return Ok(Some(()));
                    }
                    _ => {}
                }

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
                        self.push(SymbolicOpcode::LoadGlobalVar { off });
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
                        self.push(SymbolicOpcode::LoadConstant { id });
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
                        self.push(SymbolicOpcode::LoadConstant { id });
                        self.push(SymbolicOpcode::LoadConstant { id });
                    }
                    BuiltinFn::Step(a, b) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(SymbolicOpcode::LoadConstant { id });
                    }
                    BuiltinFn::Max(a, b) => {
                        if let Some(b) = b {
                            self.walk_expr(a)?.unwrap();
                            self.walk_expr(b)?.unwrap();
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(SymbolicOpcode::LoadConstant { id });
                        } else {
                            return self.emit_array_reduce(a, SymbolicOpcode::ArrayMax {});
                        }
                    }
                    BuiltinFn::Min(a, b) => {
                        if let Some(b) = b {
                            self.walk_expr(a)?.unwrap();
                            self.walk_expr(b)?.unwrap();
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(SymbolicOpcode::LoadConstant { id });
                        } else {
                            return self.emit_array_reduce(a, SymbolicOpcode::ArrayMin {});
                        }
                    }
                    BuiltinFn::Quantum(a, b) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(SymbolicOpcode::LoadConstant { id });
                    }
                    BuiltinFn::Pulse(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        if c.is_some() {
                            self.walk_expr(c.as_ref().unwrap())?.unwrap()
                        } else {
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(SymbolicOpcode::LoadConstant { id });
                        };
                    }
                    BuiltinFn::Ramp(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        if c.is_some() {
                            self.walk_expr(c.as_ref().unwrap())?.unwrap()
                        } else {
                            // A 2-arg RAMP defaults its end time to `final_time`,
                            // which lives at the fixed absolute slot
                            // FINAL_TIME_OFF (an implicit global, not a body
                            // variable). It must be read with LoadGlobalVar -- an
                            // absolute-slot load with no `module_off` relocation --
                            // exactly like BuiltinFn::FinalTime. A module-relative
                            // LoadVar happens to alias `final_time` only at the
                            // root model (where slot 3 IS final_time); inside a
                            // submodule it reads an unrelated body slot (or drops
                            // the fragment when that slot has no symbolic mapping).
                            self.push(SymbolicOpcode::LoadGlobalVar {
                                off: FINAL_TIME_OFF as u16,
                            });
                        };
                    }
                    BuiltinFn::SafeDiv(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        // The optional third arg is an arbitrary user expression
                        // (e.g. a `PREVIOUS(...)` the LTM partial path emits), so
                        // its lowering can fail recoverably; propagate via `?`
                        // rather than unwrapping (matching `a`/`b` above). A `.map`
                        // closure can't carry `?`, so walk it before the match.
                        let c = match c {
                            Some(c) => {
                                self.walk_expr(c)?.unwrap();
                                Some(())
                            }
                            None => None,
                        };
                        if c.is_none() {
                            let id = self.curr_code.intern_literal(0.0);
                            self.push(SymbolicOpcode::LoadConstant { id });
                        }
                    }
                    BuiltinFn::Sshape(a, b, c) => {
                        self.walk_expr(a)?.unwrap();
                        self.walk_expr(b)?.unwrap();
                        self.walk_expr(c)?.unwrap();
                    }
                    BuiltinFn::Mean(args) => {
                        if args.len() == 1 {
                            // MEAN is variadic (Vec<Expr>), unlike other array-reduce
                            // builtins which take Box<Expr>. Single-arg MEAN can receive
                            // scalar expressions (Op2, etc.) that walk_expr_as_view
                            // can't handle, so we match on expression type first.
                            match &args[0] {
                                Expr::StaticSubscript(..)
                                | Expr::TempArray(..)
                                | Expr::Var(..)
                                | Expr::Subscript(..) => {
                                    return self
                                        .emit_array_reduce(&args[0], SymbolicOpcode::ArrayMean {});
                                }
                                _ => {
                                    self.walk_expr(&args[0])?.unwrap();
                                    return Ok(Some(()));
                                }
                            }
                        }

                        // Multi-argument scalar mean: (arg1 + arg2 + ... + argN) / N
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(SymbolicOpcode::LoadConstant { id });

                        for arg in args.iter() {
                            self.walk_expr(arg)?.unwrap();
                            self.push(SymbolicOpcode::Op2 { op: Op2::Add });
                        }

                        let id = self.curr_code.intern_literal(args.len() as f64);
                        self.push(SymbolicOpcode::LoadConstant { id });
                        self.push(SymbolicOpcode::Op2 { op: Op2::Div });
                        return Ok(Some(()));
                    }
                    BuiltinFn::Rank(_, _) => {
                        return sim_err!(
                            TodoArrayBuiltin,
                            "array-producing builtin outside AssignTemp context".to_owned()
                        );
                    }
                    BuiltinFn::Size(arg) => {
                        return self.emit_array_reduce(arg, SymbolicOpcode::ArraySize {});
                    }
                    BuiltinFn::Stddev(arg) => {
                        return self.emit_array_reduce(arg, SymbolicOpcode::ArrayStddev {});
                    }
                    BuiltinFn::Sum(arg) => {
                        return self.emit_array_reduce(arg, SymbolicOpcode::ArraySum {});
                    }
                    BuiltinFn::VectorSelect(sel, expr, max_val, action, _err) => {
                        self.walk_expr_as_view(sel)?;
                        self.walk_expr_as_view(expr)?;
                        self.walk_expr(max_val)?.unwrap();
                        self.walk_expr(action)?.unwrap();
                        self.push(SymbolicOpcode::VectorSelect {});
                        self.push(SymbolicOpcode::PopView {});
                        self.push(SymbolicOpcode::PopView {});
                        return Ok(Some(()));
                    }
                    BuiltinFn::Previous(_, _) | BuiltinFn::Init(_) => {
                        unreachable!(
                            "Previous/Init builtins should be handled before reaching BuiltinId dispatch"
                        );
                    }
                    // Normally routed through AssignTemp by A2A hoisting.
                    // Reached here when the equation wasn't hoisted (e.g.,
                    // mixed builtin types in an Arrayed equation).
                    BuiltinFn::VectorElmMap(_, _)
                    | BuiltinFn::VectorSortOrder(_, _)
                    | BuiltinFn::AllocateAvailable(_, _, _)
                    | BuiltinFn::AllocateByPriority(_, _, _, _, _) => {
                        return sim_err!(
                            TodoArrayBuiltin,
                            "array-producing builtin outside AssignTemp context".to_owned()
                        );
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
                    BuiltinFn::Quantum(_, _) => BuiltinId::Quantum,
                    BuiltinFn::Ramp(_, _, _) => BuiltinId::Ramp,
                    BuiltinFn::SafeDiv(_, _, _) => BuiltinId::SafeDiv,
                    BuiltinFn::Sign(_) => BuiltinId::Sign,
                    BuiltinFn::Sin(_) => BuiltinId::Sin,
                    BuiltinFn::Sshape(_, _, _) => BuiltinId::Sshape,
                    BuiltinFn::Sqrt(_) => BuiltinId::Sqrt,
                    BuiltinFn::Step(_, _) => BuiltinId::Step,
                    BuiltinFn::Tan(_) => BuiltinId::Tan,
                    // handled above; we exit early
                    BuiltinFn::Time
                    | BuiltinFn::TimeStep
                    | BuiltinFn::StartTime
                    | BuiltinFn::FinalTime => unreachable!(),
                    BuiltinFn::Rank(_, _) => {
                        return sim_err!(
                            TodoArrayBuiltin,
                            "array-producing builtin outside AssignTemp context".to_owned()
                        );
                    }
                    // Previous/Init are handled by the early-return path at the top
                    // of walk_builtin (LoadPrev/LoadInitial opcodes). Reaching here
                    // would be a logic error.
                    BuiltinFn::Previous(_, _) | BuiltinFn::Init(_) => {
                        unreachable!(
                            "Previous/Init builtins should be handled before reaching BuiltinId dispatch"
                        );
                    }
                    // handled above; we exit early
                    BuiltinFn::Size(_)
                    | BuiltinFn::Stddev(_)
                    | BuiltinFn::Sum(_)
                    | BuiltinFn::VectorSelect(_, _, _, _, _) => unreachable!(),
                    BuiltinFn::VectorElmMap(_, _)
                    | BuiltinFn::VectorSortOrder(_, _)
                    | BuiltinFn::AllocateAvailable(_, _, _)
                    | BuiltinFn::AllocateByPriority(_, _, _, _, _) => {
                        return sim_err!(
                            TodoArrayBuiltin,
                            "array-producing builtin outside AssignTemp context".to_owned()
                        );
                    }
                };

                self.push(SymbolicOpcode::Apply { func });
                Some(())
            }
            Expr::EvalModule(ident, model_name, input_set, args) => {
                for arg in args.iter() {
                    // Module input args are user expressions; propagate a
                    // recoverable lowering Err via `?` rather than panicking
                    // (consistent with every other operand walk in this fn).
                    self.walk_expr(arg)?.unwrap();
                }
                // The instance's base slot is the module variable's own first
                // slot; naming it is all the declaration needs, and assembly
                // resolves it against the final layout like any other reference.
                self.module_decls.push(SymbolicModuleDecl {
                    model_name: model_name.clone(),
                    input_set: input_set.clone(),
                    var: VarRef::base(ident.clone()),
                });
                let id = (self.module_decls.len() - 1) as ModuleId;

                self.push(SymbolicOpcode::EvalModule {
                    id,
                    n_inputs: args.len() as u8,
                });
                None
            }
            Expr::ModuleInput(off, _) => {
                self.push(SymbolicOpcode::LoadModuleInput {
                    input: *off as ModuleInputOffset,
                });
                Some(())
            }
            Expr::Op2(op, lhs, rhs, _) => {
                self.walk_expr(lhs)?.unwrap();
                self.walk_expr(rhs)?.unwrap();
                let opcode = match op {
                    BinaryOp::Add => SymbolicOpcode::Op2 { op: Op2::Add },
                    BinaryOp::Sub => SymbolicOpcode::Op2 { op: Op2::Sub },
                    BinaryOp::Exp => SymbolicOpcode::Op2 { op: Op2::Exp },
                    BinaryOp::Mul => SymbolicOpcode::Op2 { op: Op2::Mul },
                    BinaryOp::Div => SymbolicOpcode::Op2 { op: Op2::Div },
                    BinaryOp::Mod => SymbolicOpcode::Op2 { op: Op2::Mod },
                    BinaryOp::Gt => SymbolicOpcode::Op2 { op: Op2::Gt },
                    BinaryOp::Gte => SymbolicOpcode::Op2 { op: Op2::Gte },
                    BinaryOp::Lt => SymbolicOpcode::Op2 { op: Op2::Lt },
                    BinaryOp::Lte => SymbolicOpcode::Op2 { op: Op2::Lte },
                    BinaryOp::Eq => SymbolicOpcode::Op2 { op: Op2::Eq },
                    BinaryOp::Neq => {
                        self.push(SymbolicOpcode::Op2 { op: Op2::Eq });
                        SymbolicOpcode::Not {}
                    }
                    BinaryOp::And => SymbolicOpcode::Op2 { op: Op2::And },
                    BinaryOp::Or => SymbolicOpcode::Op2 { op: Op2::Or },
                };
                self.push(opcode);
                Some(())
            }
            Expr::Op1(op, rhs, _) => {
                self.walk_expr(rhs)?.unwrap();
                match op {
                    UnaryOp::Not => self.push(SymbolicOpcode::Not {}),
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
                self.push(SymbolicOpcode::SetCond {});
                self.push(SymbolicOpcode::If {});
                Some(())
            }
            Expr::AssignCurr(dst, rhs) => {
                if let Expr::Const(value, _) = rhs.as_ref() {
                    let id = self.curr_code.push_named_literal(*value);
                    self.push(SymbolicOpcode::AssignConstCurr {
                        var: dst.clone(),
                        literal_id: id,
                    });
                } else {
                    self.walk_expr(rhs)?.unwrap();
                    self.push(SymbolicOpcode::AssignCurr { var: dst.clone() });
                }
                None
            }
            // A stock update -- the only thing that writes `next[]`. It is
            // emitted as the fused `BinOpAssignNext` directly, because
            // `build_stock_update_expr` always produces `Op2(Add, curr, net*dt)`
            // and so the operand walk always ends in an `Op2`. There is no
            // un-fused `Opcode::AssignNext` to fall back to; a stock update
            // arriving in any other shape is a compile error here rather than a
            // silently different program (see
            // `SymbolicByteCodeBuilder::fuse_trailing_op2_into_assign_next`).
            Expr::AssignNext(dst, rhs) => {
                self.walk_expr(rhs)?.unwrap();
                if !self.curr_code.fuse_trailing_op2_into_assign_next(dst) {
                    return sim_err!(
                        NotSimulatable,
                        format!(
                            "stock update for '{}' does not end in a binary \
                             operation, so it cannot be emitted as a next-value \
                             assignment",
                            dst.name
                        )
                    );
                }
                None
            }
            Expr::AssignTemp(id, rhs, view) => {
                // Array-producing builtins bypass the BeginIter loop entirely
                if let Expr::App(builtin, _) = rhs.as_ref() {
                    match builtin {
                        BuiltinFn::VectorElmMap(source, offset) => {
                            // Genuine Vensim resolves the mapping over the
                            // source *variable's* full storage; capture its
                            // total element count before the (possibly
                            // sliced) source view is pushed so the VM can
                            // apply the out-of-range -> :NA: bound and the
                            // per-element base correctly.
                            let full_source_len = self.full_source_len(source);
                            self.walk_expr_as_view(source)?;
                            self.walk_expr_as_view(offset)?;
                            self.push(SymbolicOpcode::VectorElmMap {
                                write_temp_id: *id as TempId,
                                full_source_len,
                            });
                            self.push(SymbolicOpcode::PopView {});
                            self.push(SymbolicOpcode::PopView {});
                            return Ok(None);
                        }
                        BuiltinFn::VectorSortOrder(array, direction) => {
                            self.walk_expr_as_view(array)?;
                            self.walk_expr(direction)?.unwrap();
                            self.push(SymbolicOpcode::VectorSortOrder {
                                write_temp_id: *id as TempId,
                            });
                            self.push(SymbolicOpcode::PopView {});
                            return Ok(None);
                        }
                        BuiltinFn::Rank(array, direction) => {
                            self.walk_expr_as_view(array)?;
                            self.walk_expr(direction)?.unwrap();
                            self.push(SymbolicOpcode::Rank {
                                write_temp_id: *id as TempId,
                            });
                            self.push(SymbolicOpcode::PopView {});
                            return Ok(None);
                        }
                        // Per-element arrayed-GF lookup (GH #580 Bug B):
                        // `g[D!](index)` where each element of `g` carries its
                        // own table. The hoisting pass (`mod.rs`) wraps this in
                        // an `AssignTemp` whose view is the GF array's view, so
                        // here we push that array as a view (for the element
                        // count + per-element flat offsets), evaluate the
                        // shared scalar `index`, and emit `LookupArray` to fill
                        // the temp -- the array analogue of the scalar `Lookup`
                        // arm below. The result temp is then consumed as a view
                        // by the wrapping reducer / vector op.
                        BuiltinFn::Lookup(table_expr, index, _loc)
                        | BuiltinFn::LookupForward(table_expr, index, _loc)
                        | BuiltinFn::LookupBackward(table_expr, index, _loc) => {
                            let mode = match builtin {
                                BuiltinFn::LookupForward(_, _, _) => LookupMode::Forward,
                                BuiltinFn::LookupBackward(_, _, _) => LookupMode::Backward,
                                _ => LookupMode::Interpolate,
                            };
                            let (base_gf, table_count) =
                                self.arrayed_lookup_table_info(table_expr)?;
                            self.walk_expr_as_view(table_expr)?;
                            self.walk_expr(index)?.unwrap();
                            self.push(SymbolicOpcode::LookupArray {
                                base_gf,
                                table_count,
                                mode,
                                write_temp_id: *id as TempId,
                            });
                            self.push(SymbolicOpcode::PopView {});
                            return Ok(None);
                        }
                        BuiltinFn::AllocateAvailable(requests, profile, avail) => {
                            self.walk_expr_as_view(requests)?;
                            self.walk_expr_as_view(profile)?;
                            self.walk_expr(avail)?.unwrap();
                            self.push(SymbolicOpcode::AllocateAvailable {
                                write_temp_id: *id as TempId,
                            });
                            self.push(SymbolicOpcode::PopView {});
                            self.push(SymbolicOpcode::PopView {});
                            return Ok(None);
                        }
                        BuiltinFn::AllocateByPriority(requests, priority, _size, width, supply) => {
                            // _size is a Vensim compatibility parameter that is
                            // always ignored -- the array size is determined from
                            // the view dimensions of the requests/priority arrays.
                            self.walk_expr_as_view(requests)?;
                            self.walk_expr_as_view(priority)?;
                            self.walk_expr(width)?.unwrap();
                            self.walk_expr(supply)?.unwrap();
                            self.push(SymbolicOpcode::AllocateByPriority {
                                write_temp_id: *id as TempId,
                            });
                            self.push(SymbolicOpcode::PopView {});
                            self.push(SymbolicOpcode::PopView {});
                            return Ok(None);
                        }
                        _ => {} // fall through to existing BeginIter logic
                    }
                }

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
                self.push(SymbolicOpcode::PushStaticView {
                    view_id: output_view_id,
                });

                // 3. Begin iteration - MUST be before source views are pushed
                // BeginIter captures view_stack.last() as the iteration view
                self.push(SymbolicOpcode::BeginIter {
                    write_temp_id: *id as TempId,
                    has_write_temp: true,
                });

                // 4. Push all source views AFTER BeginIter and record their stack offsets
                // After this, view_stack looks like: [output_view, src1, src2, ...]
                // So src1 is at offset n_source_views, src2 at n_source_views-1, etc.
                let mut iter_views_with_offsets: Vec<(SymbolicStaticView, u8)> =
                    Vec::with_capacity(n_source_views);

                for (i, src_view) in source_views.into_iter().enumerate() {
                    let view_id = self.add_static_view(src_view.clone());
                    self.push(SymbolicOpcode::PushStaticView { view_id });
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
                self.push(SymbolicOpcode::StoreIterElement {});

                // Calculate jump offset (negative, back to loop start)
                let next_iter_pos = self.curr_code.len();
                let jump_back = (loop_start as isize - next_iter_pos as isize) as i16;

                self.push(SymbolicOpcode::NextIterOrJump { jump_back });
                self.push(SymbolicOpcode::EndIter {});

                // 6. Pop all source views (in reverse order of push)
                for _ in 0..n_source_views {
                    self.push(SymbolicOpcode::PopView {});
                }

                // 7. Pop output view
                self.push(SymbolicOpcode::PopView {});

                // AssignTemp doesn't produce a value on the stack
                None
            }
        };
        Ok(result)
    }

    fn push(&mut self, op: SymbolicOpcode) {
        self.curr_code.push_opcode(op)
    }

    /// Collect all source views referenced in an expression.
    /// This traverses the expression and collects StaticArrayView data for each
    /// StaticSubscript and TempArray node, deduplicating identical views.
    fn collect_iter_source_views(&mut self, expr: &Expr) -> Vec<SymbolicStaticView> {
        let mut views = Vec::new();
        let mut seen = std::collections::HashSet::new();
        self.collect_iter_source_views_impl(expr, &mut views, &mut seen);
        views
    }

    fn collect_iter_source_views_impl(
        &mut self,
        expr: &Expr,
        views: &mut Vec<SymbolicStaticView>,
        seen: &mut std::collections::HashSet<SymbolicStaticView>,
    ) {
        match expr {
            Expr::StaticSubscript(base, view, _) => {
                let static_view = self.array_view_to_static(base, view);
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
        views: &mut Vec<SymbolicStaticView>,
        seen: &mut std::collections::HashSet<SymbolicStaticView>,
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
            Quantum(a, b) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
            }
            Pulse(a, b, opt_c) | Ramp(a, b, opt_c) | SafeDiv(a, b, opt_c) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
                if let Some(c) = opt_c {
                    self.collect_iter_source_views_impl(c, views, seen);
                }
            }
            Sshape(a, b, c) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
                self.collect_iter_source_views_impl(c, views, seen);
            }
            Step(a, b) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
            }
            // Array builtins with single argument
            Sum(a) | Stddev(a) | Size(a) => {
                self.collect_iter_source_views_impl(a, views, seen);
            }
            Rank(a, direction) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(direction, views, seen);
            }
            VectorSelect(a, b, c, d, e) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
                self.collect_iter_source_views_impl(c, views, seen);
                self.collect_iter_source_views_impl(d, views, seen);
                self.collect_iter_source_views_impl(e, views, seen);
            }
            VectorElmMap(a, b) | VectorSortOrder(a, b) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
            }
            AllocateAvailable(a, b, c) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
                self.collect_iter_source_views_impl(c, views, seen);
            }
            AllocateByPriority(a, b, c, d, e) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
                self.collect_iter_source_views_impl(c, views, seen);
                self.collect_iter_source_views_impl(d, views, seen);
                self.collect_iter_source_views_impl(e, views, seen);
            }
            // Scalar lag/initial builtins
            Previous(a, b) => {
                self.collect_iter_source_views_impl(a, views, seen);
                self.collect_iter_source_views_impl(b, views, seen);
            }
            Init(a) => {
                self.collect_iter_source_views_impl(a, views, seen);
            }
            // Constants/no-arg builtins
            Inf | Pi | Time | TimeStep | StartTime | FinalTime | IsModuleInput(_, _) => {}
        }
    }

    /// Find the stack offset for a view that was pre-pushed.
    /// Returns Some(offset) if found, where offset is 1-based from stack top.
    fn find_iter_view_offset(&self, view: &SymbolicStaticView) -> Option<u8> {
        self.iter_source_views.as_ref().and_then(|views| {
            views
                .iter()
                .find(|(v, _)| v == view)
                .map(|(_, offset)| *offset)
        })
    }

    pub(super) fn compile(mut self) -> Result<SymbolicCompiledModule> {
        // The runlists live for `'module`, independent of `&mut self`, so bind
        // them out of the (Copy) context before the mutable walks.
        let initials_by_var = self.module.runlist_initials_by_var;
        let flows = self.module.runlist_flows;
        let stocks = self.module.runlist_stocks;
        let temp_sizes = self.module.temp_sizes;

        // Compile each variable's initials separately
        let compiled_initials: Vec<SymbolicCompiledInitial> = initials_by_var
            .iter()
            .map(|var_init| {
                let bytecode = self.walk(&var_init.ast)?;
                Ok(SymbolicCompiledInitial {
                    ident: var_init.ident.clone(),
                    bytecode,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let compiled_flows = self.walk(flows)?;
        let compiled_stocks = self.walk(stocks)?;

        // Build temp info from module
        let mut temp_offsets = Vec::with_capacity(temp_sizes.len());
        let mut offset = 0usize;
        for &size in temp_sizes {
            temp_offsets.push(offset);
            offset += size;
        }
        let temp_total_size = offset;

        Ok(SymbolicCompiledModule {
            ident: self.module.ident.clone(),
            compiled_initials,
            compiled_flows,
            compiled_stocks,
            graphical_functions: self.graphical_functions,
            module_decls: self.module_decls,
            static_views: self.static_views,
            arrays: vec![],
            dimensions: self.dimensions,
            subdim_relations: self.subdim_relations,
            names: self.names,
            temp_offsets,
            temp_total_size,
            dim_lists: self.dim_lists,
            // The flow-runlist invariant/dynamic partition is decided at module
            // assembly (the salsa `assemble_module` path, where the whole
            // root-model runlist is available), NOT here: `Compiler::compile`
            // runs both per single-variable fragment (split is trivially 0) and
            // on the test-only monolithic whole-model `Module`. So this is
            // always 0; the production split is installed by `assemble_module`
            // on the `SymbolicCompiledModule` it builds from the fragments
            // (GH #712).
            flows_invariant_opcode_len: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::Loc;
    use std::collections::HashMap;

    /// Owner for the borrows a minimal, dimension-free [`ModuleCtx`] needs.
    /// The runlists are empty -- the tests call `walk_expr` directly, so the
    /// only requirement is a well-formed context `Compiler::new` can populate
    /// metadata from.
    struct EmptyCtxOwner {
        ident: Ident<Canonical>,
        inputs: std::collections::BTreeSet<Ident<Canonical>>,
        var_sizes: VarSizes,
        tables: HashMap<Ident<Canonical>, Vec<Table>>,
        dimensions_ctx: crate::dimensions::DimensionsContext,
    }

    impl EmptyCtxOwner {
        fn new() -> Self {
            EmptyCtxOwner {
                ident: Ident::new("test"),
                inputs: Default::default(),
                var_sizes: HashMap::new(),
                tables: HashMap::new(),
                dimensions_ctx: Default::default(),
            }
        }

        fn with_var_size(mut self, name: &str, size: usize) -> Self {
            self.var_sizes.insert(Ident::new(name), size);
            self
        }

        fn ctx(&self) -> ModuleCtx<'_> {
            ModuleCtx {
                ident: &self.ident,
                inputs: &self.inputs,
                temp_sizes: &[],
                runlist_initials_by_var: &[],
                runlist_flows: &[],
                runlist_stocks: &[],
                var_sizes: &self.var_sizes,
                tables: &self.tables,
                dimensions: &[],
                dimensions_ctx: &self.dimensions_ctx,
            }
        }
    }

    /// A `PREVIOUS(...)` whose argument is not a bare `Expr::Var` (it survived
    /// helper rewriting as a non-variable expression) is `NotSimulatable`. When
    /// such a `PREVIOUS` sits inside a `Subscript` index expression, the scalar
    /// `Subscript` arm of `walk_expr` must *propagate* that recoverable `Err`
    /// (so a caller like `db/ltm/compile.rs`'s LTM-synthetic-fragment compile can
    /// gracefully drop the un-compilable fragment), not escalate it to a
    /// process-killing panic. This pins the converted condition behind #363
    /// (codegen.rs line 494 was a double-`unwrap` on this `Result`).
    #[test]
    fn previous_of_non_var_inside_subscript_index_is_err_not_panic() {
        let owner = EmptyCtxOwner::new();
        let mut compiler = Compiler::new(owner.ctx());

        // arr[ PREVIOUS(1, 0) ] -- the index is a PREVIOUS of a constant, which
        // is not a bare variable reference, so the PREVIOUS arm returns
        // NotSimulatable. Before the fix the enclosing Subscript arm panicked
        // by unwrapping that Err; after the fix it propagates via `?`.
        let expr = Expr::Subscript(
            VarRef::base(Ident::new("arr")),
            vec![SubscriptIndex::Single(Expr::App(
                BuiltinFn::Previous(
                    Box::new(Expr::Const(1.0, Loc::default())),
                    Box::new(Expr::Const(0.0, Loc::default())),
                ),
                Loc::default(),
            ))],
            vec![3],
            Loc::default(),
        );

        let result = compiler.walk_expr(&expr);

        let err = result.expect_err(
            "PREVIOUS-of-non-var inside a subscript index must return a typed Err, not Ok",
        );
        assert_eq!(
            err.code,
            ErrorCode::NotSimulatable,
            "expected NotSimulatable, got {err:?}"
        );
    }

    /// The array-VIEW twin of the test above. The dynamic-subscript arm of
    /// `walk_expr_as_view` (the path a wrapping reducer like `SUM(arr[i])`
    /// takes) walks each index expression too. A `PREVIOUS`-of-non-var index
    /// there is the exact shape the GH #525 LTM partial emits
    /// (`PREVIOUS(SUM(pop[region,young]))`). Before the fix this arm did a
    /// double-`unwrap` on the `Result` and PANICKED on that `Err`, defeating
    /// the LTM assemble path's `Err(_) => None` graceful-stub handler; after
    /// the fix it propagates the typed `NotSimulatable` via `?`.
    #[test]
    fn previous_of_non_var_inside_view_subscript_index_is_err_not_panic() {
        let owner = EmptyCtxOwner::new();
        let mut compiler = Compiler::new(owner.ctx());

        // arr[ PREVIOUS(1, 0) ] driven through the array-view path: the index
        // is a PREVIOUS of a constant (NotSimulatable). The `Subscript` arm of
        // `walk_expr_as_view` must return that typed Err, not panic.
        let expr = Expr::Subscript(
            VarRef::base(Ident::new("arr")),
            vec![SubscriptIndex::Single(Expr::App(
                BuiltinFn::Previous(
                    Box::new(Expr::Const(1.0, Loc::default())),
                    Box::new(Expr::Const(0.0, Loc::default())),
                ),
                Loc::default(),
            ))],
            vec![3],
            Loc::default(),
        );

        let err = compiler.walk_expr_as_view(&expr).expect_err(
            "PREVIOUS-of-non-var inside a view subscript index must return a typed Err, not panic",
        );
        assert_eq!(
            err.code,
            ErrorCode::NotSimulatable,
            "expected NotSimulatable, got {err:?}"
        );
    }

    /// `full_source_len` reports a VECTOR ELM MAP source's extent from the
    /// variable's own size, and only when the reference addresses that
    /// variable in whole.
    ///
    /// The whole-variable rule is the exact translation of the offset scan this
    /// replaced ("does any variable's storage BEGIN at this offset?"), and it is
    /// what makes the two shapes below differ. A collapsed element source inside
    /// an array-producing builtin keeps the variable's base and carries the
    /// element in `view.offset` (that is why `Context::lower_from_expr3` returns
    /// a `StaticSubscript` rather than a `Var` there -- GH #578), so it takes
    /// the first arm and recovers the FULL extent. A reference that genuinely
    /// starts mid-variable -- a cross-module `m·x`, whose owner in this model is
    /// the module variable `m` -- names something whose extent this model's
    /// symbol table does not know, so it falls back to the view.
    #[test]
    fn full_source_len_uses_the_whole_variable_only_at_its_base() {
        let owner = EmptyCtxOwner::new().with_var_size("d", 6);
        let compiler = Compiler::new(owner.ctx());

        // `d[DimA, DimB]` collapsed to one element: base is `d`, the element
        // rides in the view. The genuine-Vensim bound is d's FULL 6 elements.
        let scalar_view = {
            let mut v = ArrayView::contiguous(vec![]);
            v.offset = 4;
            v
        };
        assert_eq!(
            compiler.full_source_len(&Expr::StaticSubscript(
                VarRef::base(Ident::new("d")),
                scalar_view.clone(),
                crate::ast::Loc::default(),
            )),
            6
        );

        // The same view, but the reference starts 4 slots into its owner (the
        // shape a cross-module reference produces). The owner is not `d` as a
        // whole, so the view's extent is all this can honestly report.
        assert_eq!(
            compiler.full_source_len(&Expr::StaticSubscript(
                VarRef::new(Ident::new("d"), 4),
                scalar_view,
                crate::ast::Loc::default(),
            )),
            1
        );
    }
}
