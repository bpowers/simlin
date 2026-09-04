// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::ast::{ArrayView, BinaryOp};
use crate::bytecode::{
    BuiltinId, DimId, DimListId, DimensionInfo, GraphicalFunctionId, LookupMode, ModuleId,
    ModuleInputOffset, NameId, Op2, RuntimeSparseMapping, TempId, VariableOffset, ViewId,
};
use crate::common::{Canonical, ErrorCode, ErrorKind, Ident, Result, canonicalize};
use crate::dimensions::Dimension;
use crate::sim_err;
use crate::snapshot_arg::{SnapshotAccess, SnapshotArg, SnapshotIndex};
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
/// codegen behind it, with one caller:
///
/// * `db::assemble::compile_phase_to_per_var_bytecodes` -- the per-variable
///   fragment compiler, which borrows the salsa-cached project-global
///   converted dimensions plus the variable's own lowered expressions, and
///   keeps the emitted fragment
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
    /// Reference -> the extent of the variable it addresses in whole. The sole
    /// reader is [`Compiler::full_source_len`]; the sole producer is
    /// `fragment::reference_extents`, shared with lowering.
    pub(crate) var_sizes: &'a VarSizes,
    pub(crate) tables: &'a HashMap<Ident<Canonical>, Vec<Table>>,
    pub(crate) dimensions: &'a [Dimension],
}

impl<'a> ModuleCtx<'a> {
    /// Emit this unit's bytecode. The single entry point into codegen.
    pub(crate) fn compile(self) -> Result<SymbolicCompiledModule> {
        Compiler::new(self).compile()
    }
}

/// Where a `PREVIOUS`/`INIT` call sits, which decides how permissive the
/// snapshot-view route is (`Compiler::snapshot_static_view`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapshotPosition {
    /// The array operand of a builtin: `walk_expr_as_view` is emitting a view
    /// here, so a view is what the call must produce.
    ViewOperand,
    /// A per-element read inside a `BeginIter` body: a scalar position, where
    /// only an array-valued call needs the view route.
    IterationBody,
}

/// Reduce a LOWERED `PREVIOUS`/`INIT` argument to the form
/// [`SnapshotArg::access`] decides over.
///
/// The lowering has already resolved every index it can, so the classification
/// reads straight off the shape: a bare reference is whole storage, a
/// `StaticSubscript` is storage with `view.dims.len()` dimensions still
/// standing (a dynamic index never produces one -- it lowers to
/// `Expr::Subscript`), and nothing else references one variable's storage at
/// all.
///
/// `builtins_visitor::BuiltinVisitor::snapshot_arg` is the twin over the
/// SOURCE argument. Both feed the one rule, so the parse's capture decision and
/// the two codegen routes below cannot disagree about which shapes are
/// addressable (GH #568's class).
pub(crate) fn lowered_snapshot_arg(arg: &Expr) -> SnapshotArg {
    match arg {
        Expr::Var(_, _) => SnapshotArg::whole(),
        Expr::StaticSubscript(_, view, _) => {
            SnapshotArg::subscripted(view.dims.iter().map(|_| SnapshotIndex::SpansDimension))
        }
        _ => SnapshotArg::not_storage(),
    }
}

/// The single slot a `LoadPrev`/`LoadInitial` can address, if `arg` resolved to
/// one: a scalar variable (`Expr::Var`), or an array reference whose subscripts
/// collapsed the view to one element (`arr[Dim.elem]`, `arr[2]`).
///
/// This is the ARRAY route's negation as well as the scalar route's predicate
/// (`Compiler::snapshot_static_view`), so the two partition the argument
/// shapes -- and the partition is [`SnapshotAccess`] rather than a second
/// statement of it here.
fn static_slot(arg: &Expr) -> Option<VarRef> {
    if lowered_snapshot_arg(arg).access() != SnapshotAccess::Slot {
        return None;
    }
    match arg {
        Expr::Var(var, _) => Some(var.clone()),
        Expr::StaticSubscript(base, view, _) => Some(base.offset_by(view.offset)),
        // `SnapshotAccess::Slot` is produced for exactly the two shapes above;
        // this arm is the loud-safe restatement of that, not a live one.
        _ => None,
    }
}

/// The loud refusal for a `PREVIOUS`/`INIT` argument
/// [`SnapshotArg::access`] says addresses no storage.
///
/// Kept as one function because [`Compiler::snapshot_static_view`] needs it in
/// two positions, and because both messages must stay refusals rather than
/// approximations: reading one element's snapshot and broadcasting it where an
/// array was written is a plausible array of wrong numbers.
fn refuse_unaddressable_snapshot(arg: &Expr) -> crate::Error {
    match arg {
        // A temp has no snapshot: nothing copies `temp_storage` into
        // `prev_values`, so a computed array's previous value is simply not
        // recorded anywhere.
        Expr::TempArray(_, _, _) => crate::Error::new(
            ErrorKind::Simulation,
            ErrorCode::NotSimulatable,
            Some(
                "PREVIOUS/INIT of a computed array has no snapshot to read: only \
                 a stored variable's values are captured each step"
                    .to_string(),
            ),
        ),
        // Everything else is an expression that `find_expr_array_view` gave a
        // shape but that did not lower to a view over storage -- there is no
        // snapshot of an expression either.
        other => crate::Error::new(
            ErrorKind::Simulation,
            ErrorCode::NotSimulatable,
            Some(format!(
                "PREVIOUS/INIT used where an array is required needs a \
                 statically resolvable array reference, got {:?}",
                std::mem::discriminant(other)
            )),
        ),
    }
}

/// `ALLOCATE AVAILABLE`'s priority-profile position refuses a `PREVIOUS`/`INIT`
/// (GH #995 phase C3), for the same reason it refuses a computed profile.
///
/// The allocator reads ALL of a requester's XPriority columns, but the Vensim
/// convention writes the argument collapsed (`pp[D,1]` -- "the priority vector
/// starting at column 1"). `context::expand_pp_view_for_allocate` is what
/// re-expands that back to the variable's full requester x XPriority array, and
/// it only understands a direct variable reference: everything else falls
/// through its `_ => Ok(lowered)` arm untouched. Before C3 that was harmless,
/// because a `PREVIOUS` of an array did not compile at all and the position's
/// only reachable non-reference shape (a computed profile) was rejected by
/// `walk_expr_as_view`. Once `PREVIOUS(pp[D,1])` became a legal view, it started
/// compiling to a ONE-COLUMN-per-requester profile and the allocator bisected
/// over it -- a silently wrong allocation where HEAD had a loud failure, which
/// is strictly worse. Rejected here instead.
///
/// Declining rather than fixing is the proportionate move: it restores exactly
/// what HEAD did for this position. Making it CORRECT is option (b) -- teach
/// `expand_pp_view_for_allocate` to look through a `PREVIOUS`/`INIT`, rebuild
/// the full-variable view underneath it, and re-wrap -- which is a lowering
/// change with its own allocator-semantics question (whether a frozen profile
/// should freeze the whole array or only the referenced column) and belongs
/// with a fixture that can tell the two apart. The workaround needs no engine
/// change at all: capture the profile into a variable of its own
/// (`frozen[D,XP] = PREVIOUS(pp[D,XP])`) and pass `frozen[D,1]`, which is a
/// direct reference the expander does understand.
fn reject_snapshot_priority_profile(profile: &Expr) -> Result<()> {
    let is_snapshot = matches!(
        profile,
        Expr::App(BuiltinFn::Previous(_, _) | BuiltinFn::Init(_), _)
    );
    if is_snapshot {
        return sim_err!(
            NotSimulatable,
            "ALLOCATE AVAILABLE reads every priority column, and its profile \
             argument is re-expanded to the whole array from a direct variable \
             reference -- a PREVIOUS/INIT there would allocate over one column. \
             Capture the frozen profile in a variable of its own and pass that."
                .to_string()
        );
    }
    Ok(())
}

/// Is this `PREVIOUS` fallback the default the unary spelling desugars to?
///
/// `builtins_visitor` rewrites `PREVIOUS(x)` to `PREVIOUS(x, 0)`, and the array
/// route can only reproduce that one value (see
/// `Compiler::snapshot_static_view`).
///
/// Compared by BIT PATTERN, which is not a formality here: `1 / PREVIOUS(x, 0)`
/// and `1 / PREVIOUS(x, -0)` differ in the sign of the infinity they yield on
/// the first step, and a `-0.0` fallback IS reachable -- not as the literal
/// `-0`, which is a negation of `0` that constant folding turns back into
/// `+0.0`, but as `0 * -1`, the shape `compiler::fold` is documented to produce.
/// See the float-equality position on [`crate::ast::Literal`]; both spellings
/// are pinned by
/// `array_operand_materialization_tests::a_non_default_array_previous_fallback_declines_loudly`.
fn is_default_previous_fallback(fallback: &Expr) -> bool {
    matches!(fallback, Expr::Const(value, _) if value.to_bits() == 0.0f64.to_bits())
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
    names: Vec<String>,
    /// Hash index over `names` so interning is O(1) amortized. The compiler
    /// runs once per per-variable fragment (tens of thousands of times on
    /// large LTM builds), and `Compiler::new` interns every dimension and
    /// element name up front -- with a linear-scan intern that was O(D^2)
    /// string comparisons per fragment (GH #655).
    name_ids: crate::common::IdentMap<String, NameId>,
    static_views: Vec<SymbolicStaticView>,
    dim_lists: Vec<(u8, [u16; 4])>,
    // Iteration context - set when compiling inside AssignTemp
    in_iteration: bool,
    /// When in optimized iteration mode, maps pre-pushed views to their stack offset.
    /// Each entry is (SymbolicStaticView, stack_offset) where stack_offset is 1-based from top.
    /// The output view is always at offset (n_source_views + 1).
    iter_source_views: Option<Vec<(SymbolicStaticView, u8)>>,
}

/// The ONE refusal for an array value in a position that consumes a single
/// value.
///
/// Two arms of `Compiler::walk_expr` reach it -- a `StaticSubscript` (a view
/// over a variable's storage) and a `TempArray` (a view over a temp) that still
/// has axes outside an iteration body -- and they are the same refusal: an
/// array where one number is required. Every legitimate array consumer reads
/// its operand through `walk_expr_as_view`, so nothing that reaches either arm
/// could use a pushed view, and the operand sites propagate the `Err` through
/// `?` rather than each guarding against a missing stack value (an `unwrap` on
/// a missing stack value is a process abort under `panic = abort`).
fn array_in_scalar_position<T>(dims: &[usize]) -> Result<T> {
    sim_err!(
        NotSimulatable,
        format!("an array of shape {dims:?} is used where a single value is required")
    )
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
            names: vec![],
            name_ids: Default::default(),
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
    /// This is a direct [`VarSizes`] lookup keyed by the reference itself,
    /// because the reference does not always name the variable being asked
    /// about: a cross-module `m·x` names the module INSTANCE `m` with `x`'s slot
    /// inside it. `VarSizes` holds one entry per variable a reference can
    /// address in whole -- a sub-model's variables among them, at their slots
    /// within the instance -- so both shapes are one lookup.
    ///
    /// A reference the table does not hold starts mid-variable: it names one
    /// element of a bigger array, and the array's extent is not that element's.
    /// Those fall back to the lowered view's element count, which is the correct
    /// full extent for the non-sliced shapes.
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
            // An array-valued `PREVIOUS`/`INIT` source (GH #995) is a view over
            // the SAME variable's storage in a snapshot buffer, so its full
            // extent is its argument's -- the snapshot regions are `n_slots`
            // copies of a chunk. Without this arm the source fell to the `_`
            // case and reported an extent of 1, and every mapped offset but 0
            // was reported out of range (`:NA:`).
            Expr::App(BuiltinFn::Previous(arg, _), _) | Expr::App(BuiltinFn::Init(arg), _) => {
                return self.full_source_len(arg);
            }
            _ => (None, 1usize),
        };

        if let Some(base) = base
            && let Some(size) = self.module.var_sizes.get(base)
        {
            return *size as u32;
        }
        view_len as u32
    }

    /// Convert an ArrayView to a SymbolicStaticView reading a variable out of
    /// `curr`.
    fn array_view_to_static(&mut self, base: &VarRef, view: &ArrayView) -> SymbolicStaticView {
        self.array_view_to_static_in(SymStaticViewBase::Var(base.clone()), view)
    }

    /// The same view geometry over one of the snapshot regions (GH #995): an
    /// array-valued `PREVIOUS`/`INIT` is the argument's view read out of
    /// `prev_values` / `initial_values`, which share `curr`'s slot numbering,
    /// so only the base tag changes.
    fn array_view_to_snapshot_static(
        &mut self,
        base: &VarRef,
        view: &ArrayView,
        storage: super::SnapshotRegion,
    ) -> SymbolicStaticView {
        let base = match storage {
            super::SnapshotRegion::Prev => SymStaticViewBase::PrevVar(base.clone()),
            super::SnapshotRegion::Initial => SymStaticViewBase::InitialVar(base.clone()),
        };
        self.array_view_to_static_in(base, view)
    }

    fn array_view_to_static_in(
        &mut self,
        base: SymStaticViewBase,
        view: &ArrayView,
    ) -> SymbolicStaticView {
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
            base,
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

    /// Build the snapshot-region static view for an **array-valued**
    /// `PREVIOUS`/`INIT` (GH #995), or `Ok(None)` when `builtin` is not one, or
    /// is the ordinary scalar form that compiles to `LoadPrev`/`LoadInitial`
    /// against a single slot.
    ///
    /// This is the single place the array route is decided, so the three
    /// consumers -- `walk_expr_as_view` (bare operand), `walk_expr` (inside a
    /// `BeginIter` body) and `collect_iter_source_views_impl` (which must
    /// pre-push exactly the views the body reads) -- cannot disagree about
    /// which shapes take it.
    ///
    /// A shape the region view cannot express is a loud `Err`, never a silent
    /// fall-through to the scalar route: reading one element's snapshot and
    /// broadcasting it where an array was written is a plausible array of wrong
    /// numbers, which is strictly worse than not compiling.
    fn snapshot_static_view(
        &mut self,
        builtin: &BuiltinFn,
        position: SnapshotPosition,
    ) -> Result<Option<SymbolicStaticView>> {
        let (arg, region) = match position {
            // The caller has already said an array is required here, so every
            // argument that lowered to a view takes the snapshot route --
            // including one that collapsed to a SINGLE element. That is what
            // makes `VECTOR ELM MAP(PREVIOUS(vals[1]), offs)` behave exactly as
            // `VECTOR ELM MAP(vals[1], offs)` does: the element establishes the
            // base and the mapping ranges over the whole source variable. A
            // `PREVIOUS` that behaved differently from its own argument in the
            // same position would be the anomaly, degenerate answers included --
            // and degenerate is what a one-element view means in a RANK-LIKE
            // position: `VECTOR SORT ORDER(PREVIOUS(vals[1]), 1)` is a constant
            // 0 sort order and `RANK(...)` a constant 1, exactly what
            // `VECTOR SORT ORDER(vals[1], 1)` already produced at HEAD. GH #995
            // warns against making that shape compile, and the warning is about
            // the LTM wrap pinning a rank-like builtin's ARGUMENT to one element
            // (a loud drop becoming a plausible constant-0 score); that half is
            // untouched -- `ltm_agg`'s rank-like decline does not key on
            // compilability, and C-LEARN's five `rank-like-partial` declines are
            // byte-identical across this change. Pinned by
            // `array_operand_materialization_tests::an_element_collapsed_snapshot_in_a_rank_like_position_matches_its_curr_twin`.
            //
            // The NUMERIC index is what the sentence above is about. The same
            // element spelled with its bare NAME (`vals[e1]`) never reaches here:
            // `builtins_visitor::index_is_static` will not accept an unqualified
            // element name on the user-equation parse path, so `PREVIOUS` reads a
            // scalar capture helper whose extent is ONE and the mapping is
            // confined to it -- measured, and pinned by
            // `array_operand_materialization_tests::every_row_of_the_issue_995_table_compiles`.
            SnapshotPosition::ViewOperand => match builtin {
                BuiltinFn::Previous(arg, _) => (arg.as_ref(), super::SnapshotRegion::Prev),
                BuiltinFn::Init(arg) => (arg.as_ref(), super::SnapshotRegion::Initial),
                _ => return Ok(None),
            },
            // A SCALAR position (a `BeginIter` body element read): only an
            // array-valued call takes the view route. A single-element argument
            // is a slot and keeps compiling to `LoadPrev`/`LoadInitial`, which
            // is what lets `PREVIOUS(matrix[E,1])` go on broadcasting.
            SnapshotPosition::IterationBody => match super::snapshot_view_arg(builtin) {
                Some(pair) => pair,
                None => return Ok(None),
            },
        };
        let fallback = match builtin {
            BuiltinFn::Previous(_, fallback) => Some(fallback.as_ref()),
            _ => None,
        };

        // A `PREVIOUS` fallback is per-call-site scalar state and a view carries
        // none. Before the first snapshot exists, a snapshot view yields 0 for
        // every element (`vm::ChunkRegions::backing`, and the wasm backend's
        // `select` on the same flag) -- which IS the default fallback the unary
        // spelling desugars to, so that case needs nothing. Any other fallback
        // would be silently ignored on the first step of every run, so reject it
        // rather than approximate it.
        if let Some(fallback) = fallback
            && !is_default_previous_fallback(fallback)
        {
            return sim_err!(
                NotSimulatable,
                "an array-valued PREVIOUS has nowhere to carry a fallback, so it \
                 can only take the default of 0; give the fallback to a scalar \
                 PREVIOUS, or capture the array in a variable of its own first"
                    .to_string()
            );
        }

        // Which shapes address storage at all is `SnapshotArg::access`, shared
        // with the scalar route above and with the parse's capture decision.
        // The refusals BELOW this gate are the other kind: an argument that does
        // address storage but whose reading here would be wrong anyway.
        if lowered_snapshot_arg(arg).access() == SnapshotAccess::Capture {
            return Err(refuse_unaddressable_snapshot(arg));
        }

        match arg {
            Expr::StaticSubscript(base, view, _) if super::view_repeats_a_dimension(view) => {
                // A source naming one dimension twice (`matrix[d,d]`) has no
                // usable projection between the array and the consumer of this
                // view -- every layer that does it matches by dimension NAME and
                // takes the first hit, so `out[i,j]` reads element `[i,i]` (see
                // `compiler::view_repeats_a_dimension`). The array route is new
                // with GH #995: `VECTOR SORT ORDER(PREVIOUS(matrix[d,d]), 1)`
                // did not compile at the merge base, and letting it compile here
                // buys a plausible array of wrong numbers. Refuse it, exactly
                // as the gate above refuses a shape that addresses no storage.
                // The DIRECT spelling
                // (`VECTOR SORT ORDER(matrix[d,d], 1)`) is untouched: it
                // compiles at the merge base, to those same wrong numbers, and
                // fixing that is a pre-existing defect in the projection rather
                // than something to bolt onto this route.
                sim_err!(
                    NotSimulatable,
                    "PREVIOUS/INIT of an array that names one dimension twice \
                     cannot be read as an array: the element-to-temp projection \
                     matches dimensions by name and cannot tell the two apart"
                        .to_string()
                )
            }
            Expr::StaticSubscript(base, view, _) => {
                Ok(Some(self.array_view_to_snapshot_static(base, view, region)))
            }
            // A bare variable reference in a view position, mirroring
            // `walk_expr_as_view`'s own `Expr::Var` arm: a one-element view.
            // Unreachable from `IterationBody`, whose classifier calls it scalar.
            Expr::Var(base, _) => {
                let view = ArrayView::contiguous(vec![1]);
                Ok(Some(
                    self.array_view_to_snapshot_static(base, &view, region),
                ))
            }
            // The gate above returned every shape that addresses no storage, so
            // this arm is the loud-safe restatement of that partition rather
            // than a live one.
            other => Err(refuse_unaddressable_snapshot(other)),
        }
    }

    /// Emit bytecode to push an expression's view onto the view stack.
    /// This is used for array operations that need to iterate over arrays.
    fn walk_expr_as_view(&mut self, expr: &Expr) -> Result<()> {
        // An array-valued `PREVIOUS`/`INIT` is its argument's view read out of
        // a snapshot region (GH #995), so it is a view position like any other
        // rather than a computed array that must be materialized first.
        if let Expr::App(builtin, _) = expr
            && let Some(static_view) =
                self.snapshot_static_view(builtin, SnapshotPosition::ViewOperand)?
        {
            let view_id = self.add_static_view(static_view);
            self.push(SymbolicOpcode::PushStaticView { view_id });
            return Ok(());
        }
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
                            // only a statement node, see `walk_expr`) is a
                            // genuine compiler invariant violation, never
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
                // Every view over storage codegen can push is an arm above;
                // anything else is a value `compiler/array_operand.rs` left
                // in place (deliberately, for `ALLOCATE AVAILABLE`'s priority
                // profile), named by what it is so the refusal can be acted on.
                let kind = match expr {
                    Expr::Const(..) => "a constant",
                    Expr::TempArrayElement(..) => "one element of an array temp",
                    Expr::Dt(..) => "DT",
                    Expr::App(..) => "a builtin call",
                    Expr::EvalModule(..) => "a module evaluation",
                    Expr::ModuleInput(..) => "a module input",
                    Expr::Op2(..) => "an arithmetic or comparison expression",
                    Expr::Op1(..) => "a negation or NOT",
                    Expr::If(..) => "a conditional",
                    Expr::AssignCurr(..) | Expr::AssignNext(..) | Expr::AssignTemp(..) => {
                        "an assignment"
                    }
                    Expr::Var(..)
                    | Expr::Subscript(..)
                    | Expr::StaticSubscript(..)
                    | Expr::TempArray(..) => unreachable!("handled above"),
                };
                sim_err!(
                    Generic,
                    format!(
                        "an array operand here must be a variable, a subscripted array or an \
                         array temp, but it is {kind}"
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

    /// Emit the bytecode for one expression.
    ///
    /// `Ok(Some(()))` means the expression left one value on the stack, which
    /// is what every operand position consumes (and unwraps). `Ok(None)` is
    /// the statement nodes -- `EvalModule`, `AssignCurr`, `AssignNext`,
    /// `AssignTemp` -- which leave nothing and appear only at the top level
    /// of a variable's lowered expression list, never as an operand. An array
    /// value in a single-value position is an `Err`, not an `Ok(None)`: the
    /// `TempArray` arm refuses it outside an iteration body, so the operand
    /// sites inherit the refusal through `?` and none of them decides it.
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
                    // A view over a variable's storage that still has axes, in
                    // a position that consumes one value: `s = arr[*] + 1`, or
                    // a per-element capture helper holding `vals[*]` in a
                    // scalar equation.
                    return array_in_scalar_position(&view.dims);
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
                    // A temp array outside an iteration body is an array value
                    // in a position that consumes one value: the whole
                    // right-hand side of a scalar (`s = LOOKUP(g, t)` with a
                    // per-element `g`), an operand (`ABS(LOOKUP(g, t))`,
                    // `LOOKUP(g, t) + 1`), or an element's right-hand side in
                    // an apply-to-all equation that leaves an axis of the
                    // table free.
                    return array_in_scalar_position(&view.dims);
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

                    // A constant, in-range element offset is resolved here so
                    // no `LoadConstant` push is emitted for it (every scalar
                    // table takes this path, its offset being a literal 0).
                    if let Some(elem) = const_element_offset(&element_offset_expr, table_count) {
                        self.walk_expr(index)?.unwrap();
                        self.push(SymbolicOpcode::LookupDirect {
                            base_gf,
                            table_count,
                            elem,
                            mode: LookupMode::Interpolate,
                        });
                        return Ok(Some(()));
                    }
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

                    if let Some(elem) = const_element_offset(&element_offset_expr, table_count) {
                        self.walk_expr(index)?.unwrap();
                        self.push(SymbolicOpcode::LookupDirect {
                            base_gf,
                            table_count,
                            elem,
                            mode,
                        });
                        return Ok(Some(()));
                    }
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
                // compile-time constant (`static_slot`).
                //
                // An ARRAY-valued argument takes the other route (GH #995):
                // `snapshot_static_view` turns it into a view over the same
                // snapshot buffer the opcodes read, which inside a `BeginIter`
                // body is one of the pre-pushed source views and is loaded per
                // element like any other array operand. Outside an iteration
                // there is no per-element context to load into, so that shape is
                // reachable only through `walk_expr_as_view`.
                if let Some(static_view) =
                    self.snapshot_static_view(builtin, SnapshotPosition::IterationBody)?
                {
                    if !self.in_iteration {
                        return sim_err!(
                            NotSimulatable,
                            "an array-valued PREVIOUS/INIT is only meaningful where an \
                             array is expected, not as a scalar operand"
                                .to_string()
                        );
                    }
                    let offset = self.find_iter_view_offset(&static_view).unwrap_or_else(|| {
                        unreachable!(
                            "snapshot view not found in pre-pushed set - \
                             collect_iter_source_views_impl and walk_expr should visit same nodes"
                        )
                    });
                    self.push(SymbolicOpcode::LoadIterViewAt { offset });
                    return Ok(Some(()));
                }
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

                // What each builtin compiles TO is per-variant semantics and
                // stays enumerated here: the fixed-slot globals and constants,
                // the array reducers (one opcode each), VECTOR SELECT, the
                // array-producing builtins (an `AssignTemp`-only shape), and
                // the `Apply` family with the VM function implementing each.
                // How many operands an `Apply` takes is not: its arguments are
                // pushed in call order below, and the three builtins whose
                // trailing operand the source may omit get the default the VM
                // reads in that position.
                let func = match builtin {
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
                    // Emitted by the early returns above.
                    BuiltinFn::Lookup(_, _, _)
                    | BuiltinFn::LookupForward(_, _, _)
                    | BuiltinFn::LookupBackward(_, _, _)
                    | BuiltinFn::IsModuleInput(_, _)
                    | BuiltinFn::Previous(_, _)
                    | BuiltinFn::Init(_) => unreachable!(),
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
                    BuiltinFn::Max(a, None) => {
                        return self.emit_array_reduce(a, SymbolicOpcode::ArrayMax {});
                    }
                    BuiltinFn::Min(a, None) => {
                        return self.emit_array_reduce(a, SymbolicOpcode::ArrayMin {});
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
                    BuiltinFn::Mean(args) => {
                        if args.len() == 1 {
                            // MEAN is variadic (Vec<Expr>), unlike other array-reduce
                            // builtins which take Box<Expr>. Single-arg MEAN can receive
                            // scalar expressions (Op2, etc.) that walk_expr_as_view
                            // can't handle, so we match on expression type first.
                            // The five shapes `walk_expr_as_view` accepts: the
                            // four storage views, plus an array-valued
                            // `PREVIOUS`/`INIT` (a view over a snapshot buffer,
                            // GH #995). Anything else is a genuine scalar
                            // expression and averages as one.
                            let is_view = matches!(
                                &args[0],
                                Expr::StaticSubscript(..)
                                    | Expr::TempArray(..)
                                    | Expr::Var(..)
                                    | Expr::Subscript(..)
                            ) || matches!(&args[0], Expr::App(b, _) if super::snapshot_view_arg(b).is_some());
                            if is_view {
                                return self
                                    .emit_array_reduce(&args[0], SymbolicOpcode::ArrayMean {});
                            }
                            self.walk_expr(&args[0])?.unwrap();
                            return Ok(Some(()));
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
                    // Array-producing builtins write a temp through an opcode
                    // of their own and are emitted only from an `AssignTemp`
                    // (below). Reached here when the equation wasn't hoisted
                    // (e.g., mixed builtin types in an Arrayed equation).
                    BuiltinFn::Rank(_, _)
                    | BuiltinFn::VectorElmMap(_, _)
                    | BuiltinFn::VectorSortOrder(_, _)
                    | BuiltinFn::AllocateAvailable(_, _, _)
                    | BuiltinFn::AllocateByPriority(_, _, _, _, _) => {
                        return sim_err!(
                            TodoArrayBuiltin,
                            "array-producing builtin outside AssignTemp context".to_owned()
                        );
                    }
                    BuiltinFn::Abs(_) => BuiltinId::Abs,
                    BuiltinFn::Arccos(_) => BuiltinId::Arccos,
                    BuiltinFn::Arcsin(_) => BuiltinId::Arcsin,
                    BuiltinFn::Arctan(_) => BuiltinId::Arctan,
                    BuiltinFn::Cos(_) => BuiltinId::Cos,
                    BuiltinFn::Exp(_) => BuiltinId::Exp,
                    BuiltinFn::Int(_) => BuiltinId::Int,
                    BuiltinFn::Round(_) => BuiltinId::Round,
                    BuiltinFn::Ln(_) => BuiltinId::Ln,
                    BuiltinFn::Log10(_) => BuiltinId::Log10,
                    BuiltinFn::Max(_, Some(_)) => BuiltinId::Max,
                    BuiltinFn::Min(_, Some(_)) => BuiltinId::Min,
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
                };

                // Operands in call order. `Apply` pops exactly
                // `BuiltinId::arity()` of them, so nothing is padded. An
                // operand is an arbitrary user expression (e.g. a
                // `PREVIOUS(...)` the LTM partial path emits), so its lowering
                // can fail recoverably; propagate via `?` rather than
                // unwrapping the `Result`.
                for arg in builtin.args() {
                    self.walk_expr(arg)?.unwrap();
                }
                // An omitted trailing operand takes the value the VM reads in
                // that position: `PULSE`'s width and `SAFEDIV`'s
                // divide-by-zero result default to 0, and a 2-arg `RAMP` ends
                // at `final_time`, which lives at the fixed absolute slot
                // FINAL_TIME_OFF (an implicit global, not a body variable). It
                // must be read with LoadGlobalVar -- an absolute-slot load with
                // no `module_off` relocation -- exactly like
                // BuiltinFn::FinalTime. A module-relative LoadVar happens to
                // alias `final_time` only at the root model (where slot 3 IS
                // final_time); inside a submodule it reads an unrelated body
                // slot (or drops the fragment when that slot has no symbolic
                // mapping).
                match builtin {
                    BuiltinFn::Pulse(_, _, None) | BuiltinFn::SafeDiv(_, _, None) => {
                        let id = self.curr_code.intern_literal(0.0);
                        self.push(SymbolicOpcode::LoadConstant { id });
                    }
                    BuiltinFn::Ramp(_, _, None) => {
                        self.push(SymbolicOpcode::LoadGlobalVar {
                            off: FINAL_TIME_OFF as u16,
                        });
                    }
                    _ => {}
                }

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
                    // An array-valued right-hand side (a `TempArray` outside an
                    // iteration, which `s = LOOKUP(g, TIME)` with a per-element
                    // `g` lowers to) is refused by the `TempArray` arm, so the
                    // `Err` propagates here and the fragment's compile reports
                    // it against this variable. The Option is then a compiler
                    // invariant: only a statement node leaves no value, and a
                    // statement is never a right-hand side.
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
                            reject_snapshot_priority_profile(profile)?;
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
                // An array-valued `PREVIOUS`/`INIT` reads its argument's view
                // out of a snapshot region rather than out of `curr`, so it
                // contributes THAT view and its argument is not walked -- the
                // `curr` view of the same reference is a different source and
                // would be pushed for nothing. A shape the region view cannot
                // express contributes nothing here and surfaces as the
                // propagated `Err` when `walk_expr` reaches the same node.
                match self.snapshot_static_view(builtin, SnapshotPosition::IterationBody) {
                    Ok(Some(static_view)) => {
                        if seen.insert(static_view.clone()) {
                            views.push(static_view);
                        }
                    }
                    Ok(None) => {
                        for arg in builtin.args() {
                            self.collect_iter_source_views_impl(arg, views, seen);
                        }
                    }
                    Err(_) => {}
                }
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
            dimensions: self.dimensions,
            names: self.names,
            temp_offsets,
            temp_total_size,
            dim_lists: self.dim_lists,
            // The flow-runlist invariant/dynamic partition is decided at module
            // assembly (the salsa `assemble_module` path, where the whole
            // root-model runlist is available), NOT here: `Compiler::compile`
            // runs per single-variable fragment, where the split is trivially
            // 0. So this is always 0; the split is installed by `assemble_module`
            // on the `SymbolicCompiledModule` it builds from the fragments
            // (GH #712).
            flows_invariant_opcode_len: 0,
        })
    }
}

/// Resolve a lookup's element-offset expression to a constant slot within the
/// variable's table block, or `None` if it must stay a runtime push.
///
/// Accepts only a non-negative integral constant strictly inside
/// `[0, table_count)` that also fits `u8`. Each condition is load-bearing:
///
/// - INTEGRAL and NON-NEGATIVE, because the VM's runtime path truncates with
///   `element_offset as usize` after rejecting negatives, and a fractional or
///   negative constant would fold to a different table than the runtime rule
///   picks. Those spellings keep the general `Lookup`.
/// - IN RANGE, because `LookupDirect` carries no `table_count` and performs no
///   runtime check; an out-of-range constant must keep the general form so the
///   VM still yields its documented NaN.
/// - FITS `u8`, because that is the field width the 8-byte `Opcode` budget
///   leaves. An arrayed GF with 256+ elements simply keeps the runtime push.
fn const_element_offset(expr: &Expr, table_count: u16) -> Option<u8> {
    let Expr::Const(value, _) = expr else {
        return None;
    };
    let value = *value;
    // `is_finite` rejects NaN and the infinities explicitly rather than leaning
    // on the `floor` comparison to catch them incidentally.
    if !value.is_finite() || value < 0.0 || value.floor() != value {
        return None;
    }
    let elem = value as usize;
    if elem >= table_count as usize {
        return None;
    }
    u8::try_from(elem).ok()
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
    }

    impl EmptyCtxOwner {
        fn new() -> Self {
            EmptyCtxOwner {
                ident: Ident::new("test"),
                inputs: Default::default(),
                var_sizes: HashMap::new(),
                tables: HashMap::new(),
            }
        }

        /// The extent of a whole reference to `name`, as
        /// `fragment::reference_extents` records an ordinary variable.
        fn with_var_size(mut self, name: &str, size: usize) -> Self {
            self.var_sizes.insert(VarRef::base(Ident::new(name)), size);
            self
        }

        /// The extent of a SUB-MODEL variable living at slot `slot` of module
        /// instance `instance`, as `reference_extents` records one.
        fn with_submodel_var_size(mut self, instance: &str, slot: usize, size: usize) -> Self {
            self.var_sizes
                .insert(VarRef::new(Ident::new(instance), slot), size);
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
    /// [`VarSizes`] entry the reference itself keys, and falls back to the view
    /// when the reference addresses no variable in whole.
    ///
    /// A collapsed element source inside an array-producing builtin keeps the
    /// variable's base and carries the element in `view.offset` (that is why
    /// `Context::lower_from_expr3` returns a `StaticSubscript` rather than a
    /// `Var` there -- GH #578), so it recovers the FULL extent. A reference that
    /// starts mid-array names one element of a bigger array and falls back.
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

        // The same view, but the reference starts 4 slots into `d`. It names one
        // element of a bigger array, whose extent is not the array's, so the
        // view's extent is all this can honestly report.
        assert_eq!(
            compiler.full_source_len(&Expr::StaticSubscript(
                VarRef::new(Ident::new("d"), 4),
                scalar_view,
                crate::ast::Loc::default(),
            )),
            1
        );
    }

    /// A CROSS-MODULE source reports the SUB-MODEL variable's extent, not the
    /// module instance's slot count.
    ///
    /// `m·x` lowers to `VarRef { name: m, element_offset: x's slot inside the
    /// instance }`, so a name-keyed lookup answered with the size of the whole
    /// instance -- and when `x` happened to sit at slot 0 of its sub-model the
    /// reference also passed a `element_offset == 0` whole-variable test, so the
    /// wrong answer was returned rather than declined. Reads past `x`'s end then
    /// landed on the NEXT sub-model variable instead of yielding `:NA:`. The
    /// end-to-end shape is
    /// `array_tests::…::elm_map_cross_module_source_uses_the_submodel_variable_extent`;
    /// this is the unit-level statement of the same rule.
    #[test]
    fn full_source_len_of_a_cross_module_source_is_the_submodel_variables_extent() {
        // Instance `m` spans 8 slots: `avals[4]` at 0, another 4-slot variable
        // at 4. Both are recorded; the instance itself is not.
        let owner = EmptyCtxOwner::new()
            .with_submodel_var_size("m", 0, 4)
            .with_submodel_var_size("m", 4, 4);
        let compiler = Compiler::new(owner.ctx());

        // A COLLAPSED element source: the view carries no dimensions, so the
        // table is the only thing that can report an extent and the answer is
        // not confounded by a fallback that happens to coincide with it.
        let collapsed = |slot: usize| {
            let mut view = ArrayView::contiguous(vec![]);
            view.offset = 1;
            compiler.full_source_len(&Expr::StaticSubscript(
                VarRef::new(Ident::new("m"), slot),
                view,
                Loc::default(),
            ))
        };

        assert_eq!(
            collapsed(0),
            4,
            "the sub-model variable at the instance's base -- its extent, not m's 8 slots"
        );
        assert_eq!(
            collapsed(4),
            4,
            "the second sub-model variable: a reference below the instance's \
             base still addresses a variable in whole"
        );
        assert_eq!(
            collapsed(2),
            1,
            "mid-array: no whole-variable entry, so the collapsed view stands"
        );

        // The dynamic-subscript shape resolves through the same table; its
        // lowered bounds agree with it for an in-model source, and only the
        // table is right for a cross-module one.
        assert_eq!(
            compiler.full_source_len(&Expr::Subscript(
                VarRef::new(Ident::new("m"), 4),
                vec![SubscriptIndex::Single(Expr::Const(0.0, Loc::default()))],
                vec![4],
                Loc::default(),
            )),
            4
        );
    }
}
