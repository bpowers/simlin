// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Symbolic bytecode: the layout-independent form the compiler emits, and the
//! single late pass that turns it concrete.
//!
//! Codegen emits `SymbolicOpcode`s whose variable operands are
//! [`crate::compiler::VarRef`]s -- a canonical variable name plus an element
//! offset -- so a compiled fragment says nothing about where its variables live.
//! That is what lets salsa cache one `PerVarBytecodes` per variable and reuse it
//! across variable additions, removals and renames, and what lets one cache
//! entry serve both the diagnostic pass and assembly.
//!
//! The pipeline is: lowered `Expr` (names) -> codegen -> `SymbolicByteCode` ->
//! `resolve` -> concrete bytecode. Addresses travel in exactly one direction and
//! are assigned exactly once, at assembly, against the model's final
//! `VariableLayout`.
//!
//! The peephole optimizer and the literal pool live on this side too
//! ([`SymbolicByteCodeBuilder`]): they are address-independent, and running them
//! before resolution keeps `resolve_bytecode` a strict 1:1 mapping, which is
//! what several downstream invariants rely on (the run-invariant prefix
//! boundary, the SCC segment boundaries).

// These types and functions are used by the incremental compilation pipeline.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, CompiledInitial, CompiledModule, DimId, DimListId,
    GraphicalFunctionId, LiteralId, LookupMode, ModuleDeclaration, ModuleId, ModuleInputOffset,
    Op2, Opcode, PcOffset, RuntimeSparseMapping, STACK_CAPACITY, StaticArrayView, TempId,
    VariableOffset, ViewId, ViewStorage,
};
use crate::common::{Canonical, Ident};

pub(crate) use super::expr::VarRef as SymVarRef;

// ============================================================================
// Types
// ============================================================================

/// Symbolic version of `Opcode`. Identical structure except opcodes that
/// reference model variable offsets use `SymVarRef` instead of `VariableOffset`.
///
/// Opcodes that reference global implicit variables (time, dt, etc.) keep their
/// fixed offsets since those never change.
///
/// The 3-address fused opcodes (`BinVarVar`, `AssignAddVarVarCurr`, ...) have no
/// counterpart here, and that absence is structural rather than an oversight:
/// `ByteCode::fuse_three_address` runs at `Vm::new`, on the VM's private copy of
/// already-resolved bytecode, so a fused opcode can never exist in the symbolic
/// domain.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum SymbolicOpcode {
    // === ARITHMETIC & LOGIC (unchanged) ===
    Op2 {
        op: Op2,
    },
    Not {},

    // === CONSTANTS & VARIABLES ===
    LoadConstant {
        id: LiteralId,
    },
    LoadVar {
        var: SymVarRef,
    },
    /// Symbolic counterpart of `Opcode::LoadPrev`.
    SymLoadPrev {
        var: SymVarRef,
    },
    /// Symbolic counterpart of `Opcode::LoadInitial`.
    SymLoadInitial {
        var: SymVarRef,
    },
    LoadGlobalVar {
        off: VariableOffset,
    },

    // === LEGACY SUBSCRIPT ===
    PushSubscriptIndex {
        bounds: VariableOffset,
    },
    LoadSubscript {
        var: SymVarRef,
    },

    // === CONTROL FLOW (unchanged) ===
    SetCond {},
    If {},
    Ret,

    // === MODULES (unchanged) ===
    LoadModuleInput {
        input: ModuleInputOffset,
    },
    EvalModule {
        id: ModuleId,
        n_inputs: u8,
    },

    // === ASSIGNMENT ===
    AssignCurr {
        var: SymVarRef,
    },

    // === BUILTINS & LOOKUPS (unchanged) ===
    Apply {
        func: BuiltinId,
    },
    Lookup {
        base_gf: GraphicalFunctionId,
        table_count: u16,
        mode: LookupMode,
    },
    /// `Lookup` with the element offset resolved at COMPILE time.
    ///
    /// `compiler::codegen` pushes a `LoadConstant` for a lookup's element
    /// offset before the index expression, and for a scalar table that
    /// constant is always 0 -- 429k dispatches per C-LEARN run and 5.1% of
    /// WORLD3's, spent pushing a zero the VM immediately pops and range-checks.
    /// The push is not adjacent to the `Lookup` (the index expression sits
    /// between), so no peephole can remove it; it has to not be emitted.
    ///
    /// `base_gf`/`table_count` still describe the variable's WHOLE table block,
    /// exactly as on `Lookup`, because `gf_blocks_of_fragment` reads block
    /// extents off these two fields. `elem` is the resolved offset WITHIN that
    /// block, bounds-checked at emit time (codegen only emits this form when
    /// `elem < table_count`), so the VM needs no runtime range check.
    LookupDirect {
        base_gf: GraphicalFunctionId,
        table_count: u16,
        elem: u8,
        mode: LookupMode,
    },

    // === SUPERINSTRUCTIONS ===
    AssignConstCurr {
        var: SymVarRef,
        literal_id: LiteralId,
    },
    BinOpAssignCurr {
        op: Op2,
        var: SymVarRef,
    },
    BinOpAssignNext {
        op: Op2,
        var: SymVarRef,
    },

    // === ARRAY VIEW STACK ===
    //
    // Every variant in this enum is one `Compiler` constructs. `Compiler`
    // (through its `SymbolicByteCodeBuilder`, which builds the fused
    // superinstructions) is the ONLY producer of a `SymbolicOpcode` and
    // `resolve_opcode` the only producer of an `Opcode`, so a symbolic variant
    // nothing constructs is a program the compiler cannot express -- and the
    // dead-code lint, which clippy denies, is what keeps such a variant out:
    // never `allow` it here.
    PushStaticView {
        view_id: ViewId,
    },
    PushVarViewDirect {
        var: SymVarRef,
        dim_list_id: DimListId,
    },
    ViewSubscriptDynamic {
        dim_idx: u8,
    },
    ViewRangeDynamic {
        dim_idx: u8,
    },
    PopView {},

    // === TEMP ARRAY ACCESS (unchanged) ===
    LoadTempConst {
        temp_id: TempId,
        index: u16,
    },

    // === ITERATION (unchanged) ===
    BeginIter {
        write_temp_id: TempId,
        has_write_temp: bool,
    },
    LoadIterViewAt {
        offset: u8,
    },
    StoreIterElement {},
    NextIterOrJump {
        jump_back: PcOffset,
    },
    EndIter {},

    // === ARRAY REDUCTIONS (unchanged) ===
    ArraySum {},
    ArrayMax {},
    ArrayMin {},
    ArrayMean {},
    ArrayStddev {},
    ArraySize {},

    // === VECTOR OPERATIONS (unchanged) ===
    VectorSelect {},
    VectorElmMap {
        write_temp_id: TempId,
        full_source_len: u32,
    },
    VectorSortOrder {
        write_temp_id: TempId,
    },
    Rank {
        write_temp_id: TempId,
    },
    // Per-element arrayed-GF lookup -> temp (GH #580 Bug B). All fields are
    // layout-independent (GF-table indices + temp id), so it round-trips
    // through symbolization unchanged, exactly like `Lookup`.
    LookupArray {
        base_gf: GraphicalFunctionId,
        table_count: u16,
        mode: LookupMode,
        write_temp_id: TempId,
    },
    AllocateAvailable {
        write_temp_id: TempId,
    },
    AllocateByPriority {
        write_temp_id: TempId,
    },
}

/// Require every scratch-temp lifetime in `member`'s symbolic program to be
/// contained by one per-element assignment segment.
///
/// Resolved recurrence SCCs reorder those segments while preserving the
/// opcodes *inside* each segment. A temp touched by two segments therefore has
/// no dominance guarantee after interleaving: its consumer may move before its
/// definition. Conversely, a temp whose complete set of touches is contained
/// by one segment moves as one unit, so its original definition-before-use
/// order is preserved. A "touch" includes both direct temp-id opcodes and a
/// `PushStaticView` whose backing storage is that temp; the latter is how most
/// array-producing builtin results are consumed.
///
/// The final member write terminates the final segment. Any trailing cleanup
/// opcodes belong to that same segment, matching SCC assembly's
/// `segment_member_by_element` contract. An invalid view id is a malformed
/// fragment and is rejected rather than treated as a non-temp view.
pub(crate) fn validate_segment_local_temps(
    member: &str,
    code: &[SymbolicOpcode],
    static_views: &[SymbolicStaticView],
) -> Result<(), String> {
    let write_element = |op: &SymbolicOpcode| {
        matches!(
            op,
            SymbolicOpcode::AssignCurr { var }
                | SymbolicOpcode::AssignConstCurr { var, .. }
                | SymbolicOpcode::BinOpAssignCurr { var, .. }
                if var.name.as_str() == member
        )
    };
    let Some(last_write) = code.iter().rposition(write_element) else {
        // The caller owns the separate "element sourceable" refusal. There is
        // no reorderable boundary here at which a temp could cross.
        return Ok(());
    };

    let mut segment = 0usize;
    let mut owner_by_temp: HashMap<u32, usize> = HashMap::new();
    for (pc, op) in code.iter().enumerate() {
        let direct_temp = match op {
            SymbolicOpcode::LoadTempConst { temp_id, .. } => Some(*temp_id as u32),
            SymbolicOpcode::BeginIter {
                write_temp_id,
                has_write_temp: true,
            }
            | SymbolicOpcode::VectorElmMap { write_temp_id, .. }
            | SymbolicOpcode::VectorSortOrder { write_temp_id }
            | SymbolicOpcode::Rank { write_temp_id }
            | SymbolicOpcode::LookupArray { write_temp_id, .. }
            | SymbolicOpcode::AllocateAvailable { write_temp_id }
            | SymbolicOpcode::AllocateByPriority { write_temp_id } => Some(*write_temp_id as u32),
            SymbolicOpcode::PushStaticView { view_id } => {
                let view = static_views.get(*view_id as usize).ok_or_else(|| {
                    format!(
                        "SCC member `{member}` pushes static view {view_id}, but its fragment has \
                         only {} views; keeping CircularDependency",
                        static_views.len()
                    )
                })?;
                match &view.base {
                    SymStaticViewBase::Temp(temp_id) => Some(*temp_id),
                    SymStaticViewBase::Var(_)
                    | SymStaticViewBase::PrevVar(_)
                    | SymStaticViewBase::InitialVar(_) => None,
                }
            }
            // Exhaustive by design: a new symbolic opcode cannot silently
            // acquire a temp operand without classifying its SCC lifetime.
            SymbolicOpcode::Op2 { .. }
            | SymbolicOpcode::Not { .. }
            | SymbolicOpcode::LoadConstant { .. }
            | SymbolicOpcode::LoadVar { .. }
            | SymbolicOpcode::SymLoadPrev { .. }
            | SymbolicOpcode::SymLoadInitial { .. }
            | SymbolicOpcode::LoadGlobalVar { .. }
            | SymbolicOpcode::PushSubscriptIndex { .. }
            | SymbolicOpcode::LoadSubscript { .. }
            | SymbolicOpcode::SetCond { .. }
            | SymbolicOpcode::If { .. }
            | SymbolicOpcode::Ret
            | SymbolicOpcode::LoadModuleInput { .. }
            | SymbolicOpcode::EvalModule { .. }
            | SymbolicOpcode::AssignCurr { .. }
            | SymbolicOpcode::Apply { .. }
            | SymbolicOpcode::Lookup { .. }
            | SymbolicOpcode::LookupDirect { .. }
            | SymbolicOpcode::AssignConstCurr { .. }
            | SymbolicOpcode::BinOpAssignCurr { .. }
            | SymbolicOpcode::BinOpAssignNext { .. }
            | SymbolicOpcode::PushVarViewDirect { .. }
            | SymbolicOpcode::ViewSubscriptDynamic { .. }
            | SymbolicOpcode::ViewRangeDynamic { .. }
            | SymbolicOpcode::PopView { .. }
            | SymbolicOpcode::BeginIter {
                has_write_temp: false,
                ..
            }
            | SymbolicOpcode::LoadIterViewAt { .. }
            | SymbolicOpcode::StoreIterElement { .. }
            | SymbolicOpcode::NextIterOrJump { .. }
            | SymbolicOpcode::EndIter { .. }
            | SymbolicOpcode::ArraySum { .. }
            | SymbolicOpcode::ArrayMax { .. }
            | SymbolicOpcode::ArrayMin { .. }
            | SymbolicOpcode::ArrayMean { .. }
            | SymbolicOpcode::ArrayStddev { .. }
            | SymbolicOpcode::ArraySize { .. }
            | SymbolicOpcode::VectorSelect { .. } => None,
        };

        if let Some(temp_id) = direct_temp
            && let Some(owner) = owner_by_temp.insert(temp_id, segment)
            && owner != segment
        {
            return Err(format!(
                "SCC member `{member}` uses temp {temp_id} in per-element segments \
                 {owner} and {segment}; segment reordering cannot preserve its definition \
                 dominance, so keeping CircularDependency"
            ));
        }

        // Cleanup after the final write remains part of the final segment.
        if pc < last_write && write_element(op) {
            segment += 1;
        }
    }

    Ok(())
}

/// Symbolic version of `ByteCode`. Contains the literal pool (unchanged)
/// and symbolic opcodes.
///
/// Its `f64`s are compared with the DERIVED (IEEE) `PartialEq`, not by bit
/// pattern: a NaN makes this value unequal to a bit-identical rebuild and so
/// unable to backdate, and two pools differing only in a zero's sign compare
/// equal. Both are accepted, knowingly -- see the "Float equality in this
/// crate" section on [`crate::ast::Literal`] for the position, the corrected
/// premise behind GH #642, and what would change the decision.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SymbolicByteCode {
    pub literals: Vec<f64>,
    pub code: Vec<SymbolicOpcode>,
}

/// Symbolic version of `StaticArrayView`. When the view refers to a model
/// variable (not a temp), `base_off` is replaced with a `SymVarRef`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SymbolicStaticView {
    pub base: SymStaticViewBase,
    pub dims: SmallVec<[u16; 4]>,
    pub strides: SmallVec<[i32; 4]>,
    pub offset: u32,
    pub sparse: SmallVec<[RuntimeSparseMapping; 2]>,
    pub dim_ids: SmallVec<[DimId; 4]>,
}

/// Where a symbolic static view's elements live, before layout assignment.
///
/// The three variable-backed arms differ ONLY in which of the VM's parallel
/// chunk-shaped regions they read; they share `curr`'s slot numbering, so all
/// three resolve a `SymVarRef` through the same layout lookup. Splitting them
/// into distinct variants rather than pairing one `Var` with a storage field
/// keeps `Temp` + a snapshot region -- a temp has no snapshot -- unrepresentable
/// (GH #995).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SymStaticViewBase {
    /// Model variable reference, read from `curr`
    Var(SymVarRef),
    /// Model variable reference, read from the `PREVIOUS` snapshot
    PrevVar(SymVarRef),
    /// Model variable reference, read from the `INIT` snapshot
    InitialVar(SymVarRef),
    /// Temp array ID
    Temp(u32),
}

/// Symbolic version of `ModuleDeclaration`. The `off` field (parent module
/// offset of the module variable) is replaced with a symbolic reference.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolicModuleDecl {
    pub model_name: Ident<Canonical>,
    pub input_set: BTreeSet<Ident<Canonical>>,
    pub var: SymVarRef,
}

/// Symbolic version of `CompiledInitial`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolicCompiledInitial {
    pub ident: Ident<Canonical>,
    pub bytecode: SymbolicByteCode,
}

/// Full symbolic representation of a `CompiledModule`.
///
/// There is deliberately no `n_slots`: a slot count is a property of a layout,
/// and this value has none. `resolve_module` takes the slot count from the
/// layout it resolves against, which is the only place the number is meaningful.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolicCompiledModule {
    pub ident: Ident<Canonical>,
    pub compiled_initials: Vec<SymbolicCompiledInitial>,
    pub compiled_flows: SymbolicByteCode,
    pub compiled_stocks: SymbolicByteCode,
    pub graphical_functions: Vec<Vec<(f64, f64)>>,
    pub module_decls: Vec<SymbolicModuleDecl>,
    pub static_views: Vec<SymbolicStaticView>,
    // Unchanged context fields
    pub dimensions: Vec<crate::bytecode::DimensionInfo>,
    pub names: Vec<String>,
    pub temp_offsets: Vec<usize>,
    pub temp_total_size: usize,
    pub dim_lists: Vec<(u8, [u16; 4])>,
    /// Opcode length of the run-invariant prefix of `compiled_flows.code`
    /// (GH #712). Carried symbolically so `resolve_module` (opcode-count-
    /// preserving) can copy it straight onto the resolved `CompiledModule`.
    /// `0` when no flow variable is run-invariant.
    pub flows_invariant_opcode_len: usize,
}

// ============================================================================
// Per-Variable Compiled Fragments
// ============================================================================

/// Compiled output for a single variable, with symbolic (layout-independent)
/// bytecodes. Produced by `compile_var_fragment`, consumed by `assemble_module`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CompiledVarFragment {
    pub ident: String,
    /// Symbolic bytecodes for the initial-value phase (None if var not in initials runlist)
    pub initial_bytecodes: Option<PerVarBytecodes>,
    /// Symbolic bytecodes for the flow/dt phase
    pub flow_bytecodes: Option<PerVarBytecodes>,
    /// Symbolic bytecodes for the stock-update phase
    pub stock_bytecodes: Option<PerVarBytecodes>,
}

/// The three programs of an assembled module. A variable's fragment carries
/// bytecode for whichever of them its runlist membership schedules it in, and
/// `db::assemble` emits each program from the fragments that have one for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Initials,
    Flows,
    Stocks,
}

impl CompiledVarFragment {
    /// This variable's bytecode for `phase`, if it is scheduled in it.
    pub(crate) fn phase(&self, phase: Phase) -> Option<&PerVarBytecodes> {
        match phase {
            Phase::Initials => self.initial_bytecodes.as_ref(),
            Phase::Flows => self.flow_bytecodes.as_ref(),
            Phase::Stocks => self.stock_bytecodes.as_ref(),
        }
    }
}

/// Bytecodes plus side-channel data for one variable in one phase.
///
/// This is the value of the TRACKED `db::compile_var_fragment`, read by the
/// TRACKED `db::assemble_module` -- which is why GH #642's "the only consumer is
/// non-tracked" reasoning does not hold here. Its floats (the literal pool
/// inside `symbolic`, and `graphical_functions`) keep the derived IEEE
/// `PartialEq`; see the "Float equality in this crate" section on
/// [`crate::ast::Literal`] for why that is accepted rather than open.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PerVarBytecodes {
    pub symbolic: SymbolicByteCode,
    /// Graphical functions (lookup tables) referenced by this variable's code
    pub graphical_functions: Vec<Vec<(f64, f64)>>,
    /// Module declarations for module variables
    pub module_decls: Vec<SymbolicModuleDecl>,
    /// Static array views referenced
    pub static_views: Vec<SymbolicStaticView>,
    /// Temp array sizes: (temp_id, size)
    pub temp_sizes: Vec<(u32, usize)>,
    /// Dimension list entries
    pub dim_lists: Vec<Vec<u16>>,
}

// ============================================================================
// Variable Layout
// ============================================================================

/// Entry in a variable layout: the variable's offset and size within the module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LayoutEntry {
    pub offset: usize,
    pub size: usize,
}

/// Maps variable names to their (offset, size) within a module.
/// This is the output of `compute_layout` and the input to assembly.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct VariableLayout {
    entries: HashMap<String, LayoutEntry>,
    /// Total number of slots in this module.
    pub n_slots: usize,
}

impl VariableLayout {
    pub fn new(entries: HashMap<String, LayoutEntry>, n_slots: usize) -> Self {
        VariableLayout { entries, n_slots }
    }

    pub fn get(&self, name: &str) -> Option<&LayoutEntry> {
        self.entries.get(name)
    }

    /// Every variable the layout places, with its entry. Iteration order is
    /// the map's and carries no meaning; a consumer that needs an order sorts.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &LayoutEntry)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name.as_str(), entry))
    }

    /// Produce the root-module layout from this body layout.
    ///
    /// `compute_layout` returns a role-independent *body* layout whose
    /// offsets start at 0. The root (main) module additionally reserves
    /// slots `0..IMPLICIT_VAR_COUNT` for the implicit globals
    /// `time`/`dt`/`initial_time`/`final_time` (read at absolute fixed
    /// slots by `LoadGlobalVar`, so the root data array must physically
    /// contain them). This shift relocates every body entry by
    /// `IMPLICIT_VAR_COUNT`, inserts the four implicit globals at their
    /// fixed slots, and grows `n_slots` accordingly.
    ///
    /// This is the SINGLE place the root +`IMPLICIT_VAR_COUNT` shift is
    /// computed. Both consumers of final root offsets read it:
    /// `assemble_module`'s root path (it resolves every fragment `SymVarRef`
    /// and module-decl `off` against the shifted layout) and
    /// `db::layout::flattened_offsets` (the results-offset map is this layout
    /// flattened), so the two cannot diverge on the reservation amount, the
    /// implicit-global slots, or the body offset.
    pub(crate) fn root_shifted(&self) -> VariableLayout {
        use crate::vm::{DT_OFF, FINAL_TIME_OFF, IMPLICIT_VAR_COUNT, INITIAL_TIME_OFF, TIME_OFF};

        let mut entries = HashMap::with_capacity(self.entries.len() + IMPLICIT_VAR_COUNT);
        entries.insert(
            "time".to_string(),
            LayoutEntry {
                offset: TIME_OFF,
                size: 1,
            },
        );
        entries.insert(
            "dt".to_string(),
            LayoutEntry {
                offset: DT_OFF,
                size: 1,
            },
        );
        entries.insert(
            "initial_time".to_string(),
            LayoutEntry {
                offset: INITIAL_TIME_OFF,
                size: 1,
            },
        );
        entries.insert(
            "final_time".to_string(),
            LayoutEntry {
                offset: FINAL_TIME_OFF,
                size: 1,
            },
        );
        for (name, entry) in &self.entries {
            entries.insert(
                name.clone(),
                LayoutEntry {
                    offset: entry.offset + IMPLICIT_VAR_COUNT,
                    size: entry.size,
                },
            );
        }
        VariableLayout {
            entries,
            n_slots: self.n_slots + IMPLICIT_VAR_COUNT,
        }
    }
}

// ============================================================================
// Emission: the symbolic bytecode builder
// ============================================================================

impl SymbolicOpcode {
    /// The jump offset, if this opcode is a backward jump.
    ///
    /// Centralized here because two passes must agree on which opcodes carry a
    /// PC, and a new jump opcode that failed to report itself would be
    /// silently mishandled by both: the peephole optimizer would mis-relocate
    /// it, and `db::assemble::segment_member_by_element` -- which REORDERS the
    /// segments a jump lives between -- would not notice it escaping its
    /// segment.
    pub(crate) fn jump_offset(&self) -> Option<PcOffset> {
        match self {
            SymbolicOpcode::NextIterOrJump { jump_back } => Some(*jump_back),
            _ => None,
        }
    }

    /// Mutably borrow the jump offset, if this opcode is a backward jump.
    fn jump_offset_mut(&mut self) -> Option<&mut PcOffset> {
        match self {
            SymbolicOpcode::NextIterOrJump { jump_back } => Some(jump_back),
            _ => None,
        }
    }
}

/// Accumulates one emission unit's symbolic opcodes and its literal pool.
///
/// This is the compiler's only bytecode builder. It runs entirely in the
/// layout-independent domain: the peephole fusions it performs
/// (`LoadConstant; AssignCurr` -> `AssignConstCurr`, `Op2; AssignCurr` ->
/// `BinOpAssignCurr`, and the emit-time `Op2` -> `BinOpAssignNext`) key on
/// opcode shape, never on an address, so fusing before resolution rather than
/// after is not an approximation -- it is the same decision made one step
/// earlier. Keeping it here is what makes `resolve_bytecode` a strict 1:1
/// mapping, which the run-invariant flow prefix
/// (`SymbolicCompiledModule::flows_invariant_opcode_len`) and the SCC
/// per-element segmentation both depend on.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub(crate) struct SymbolicByteCodeBuilder {
    bytecode: SymbolicByteCode,
    // keyed on the literal's bit pattern: interning only needs Eq + Hash, and
    // bit-exact deduplication is the right semantic for codegen (it never
    // conflates distinct values; at worst -0.0 and 0.0 get separate slots)
    interned_literals: HashMap<u64, LiteralId>,
}

impl SymbolicByteCodeBuilder {
    pub(crate) fn intern_literal(&mut self, lit: f64) -> LiteralId {
        let key = lit.to_bits();
        if self.interned_literals.contains_key(&key) {
            return self.interned_literals[&key];
        }
        self.bytecode.literals.push(lit);
        let literal_id = (self.bytecode.literals.len() - 1) as u16;
        self.interned_literals.insert(key, literal_id);
        literal_id
    }

    /// Allocate a new literal slot without deduplication.
    /// Used for named constants so each variable gets its own slot,
    /// preventing shared-literal corruption when overriding via set_value.
    pub(crate) fn push_named_literal(&mut self, lit: f64) -> LiteralId {
        self.bytecode.literals.push(lit);
        (self.bytecode.literals.len() - 1) as u16
    }

    pub(crate) fn push_opcode(&mut self, op: SymbolicOpcode) {
        self.bytecode.code.push(op)
    }

    /// Replace a just-emitted trailing `Op2` with the fused
    /// `BinOpAssignNext { op, var }` that both computes it and stores the
    /// result into `next[var]`. Returns `false` -- emitting nothing -- when the
    /// stream does not end in an `Op2`.
    ///
    /// There is no un-fused `SymbolicOpcode::AssignNext`: a stock update is the
    /// only thing that ever writes `next[]`, and `Context::build_stock_update_expr`
    /// always returns `Op2(Add, curr_value, net * dt)`, so the operand walk
    /// always ends in an `Op2` and the fused form is always emittable. Codegen
    /// therefore fuses here, at emit time, and reports a typed error if a stock
    /// update ever arrives in a shape that would need the un-fused opcode --
    /// most plausibly an eventual `non_negative` implementation (GH #545)
    /// wrapping the update in a `MAX`. A comment could not fail; this can.
    ///
    /// Fusing at emit time is strictly inside `peephole_optimize`'s
    /// jump-target safety envelope rather than an exception to it: the `Op2`
    /// being replaced is the LAST opcode in the stream, so no jump emitted so
    /// far can target it (every jump is a backward `NextIterOrJump` emitted
    /// after its target), and the pair it replaces has no successor whose PC
    /// could shift.
    pub(crate) fn fuse_trailing_op2_into_assign_next(&mut self, var: &SymVarRef) -> bool {
        match self.bytecode.code.last() {
            Some(SymbolicOpcode::Op2 { op }) => {
                let op = *op;
                let last = self.bytecode.code.len() - 1;
                self.bytecode.code[last] = SymbolicOpcode::BinOpAssignNext {
                    op,
                    var: var.clone(),
                };
                true
            }
            _ => false,
        }
    }

    /// Returns the current number of opcodes in the bytecode
    pub(crate) fn len(&self) -> usize {
        self.bytecode.code.len()
    }

    pub(crate) fn finish(self) -> SymbolicByteCode {
        let mut bc = self.bytecode;
        bc.peephole_optimize();
        bc
    }
}

impl SymbolicByteCode {
    /// Peephole optimization pass: fuse common opcode sequences into
    /// superinstructions to reduce dispatch overhead.
    ///
    /// Only fuses adjacent instructions when neither is a jump target.
    /// Jump offsets are recalculated after fusion using an old->new PC map.
    fn peephole_optimize(&mut self) {
        if self.code.is_empty() {
            return;
        }

        // 1. Build set of PCs that are jump targets
        let mut jump_targets = vec![false; self.code.len()];
        for (pc, op) in self.code.iter().enumerate() {
            if let Some(offset) = op.jump_offset() {
                let target = (pc as isize + offset as isize) as usize;
                assert!(
                    target < jump_targets.len(),
                    "jump at pc {pc} targets {target}, which is out of bounds (code length: {})",
                    self.code.len()
                );
                jump_targets[target] = true;
            }
        }

        // 2. Build old_pc -> new_pc mapping and fused output.
        // pc_map has one entry per original instruction so that jump fixup
        // can index by the original PC directly.
        let mut optimized: Vec<SymbolicOpcode> = Vec::with_capacity(self.code.len());
        let mut pc_map: Vec<usize> = Vec::with_capacity(self.code.len() + 1);
        let mut i = 0;
        while i < self.code.len() {
            let new_pc = optimized.len();
            pc_map.push(new_pc);

            // Only try fusion if the next instruction is not a jump target.
            // We intentionally don't check whether instruction i itself is a
            // jump target: the fused instruction replaces both i and i+1 at the
            // same PC, so jumps to i still land on the correct (fused) opcode.
            let can_fuse = i + 1 < self.code.len() && !jump_targets[i + 1];

            if can_fuse {
                let fused = match (&self.code[i], &self.code[i + 1]) {
                    // Pattern: LoadConstant + AssignCurr -> AssignConstCurr
                    (SymbolicOpcode::LoadConstant { id }, SymbolicOpcode::AssignCurr { var }) => {
                        Some(SymbolicOpcode::AssignConstCurr {
                            var: var.clone(),
                            literal_id: *id,
                        })
                    }
                    // Pattern: Op2 + AssignCurr -> BinOpAssignCurr
                    (SymbolicOpcode::Op2 { op }, SymbolicOpcode::AssignCurr { var }) => {
                        Some(SymbolicOpcode::BinOpAssignCurr {
                            op: *op,
                            var: var.clone(),
                        })
                    }
                    _ => None,
                };

                if let Some(op) = fused {
                    optimized.push(op);
                    // Both old PCs map to the same new PC
                    pc_map.push(new_pc);
                    i += 2;
                    continue;
                }
            }

            // No pattern matched - copy opcode as-is
            optimized.push(self.code[i].clone());
            i += 1;
        }
        // Sentinel for instructions past the end
        pc_map.push(optimized.len());

        // 3. Fix up jump offsets.  Iterate original code to find jumps,
        // then use pc_map (indexed by old_pc) for O(1) translation.
        for (old_pc, op) in self.code.iter().enumerate() {
            let Some(jump_back) = op.jump_offset() else {
                continue;
            };
            let new_pc = pc_map[old_pc];
            let old_target = (old_pc as isize + jump_back as isize) as usize;
            let new_target = pc_map[old_target];
            let new_jump_back = (new_target as isize - new_pc as isize) as PcOffset;
            *optimized[new_pc].jump_offset_mut().unwrap() = new_jump_back;
        }

        self.code = optimized;
    }
}

// ============================================================================
// Layout Validation
// ============================================================================

/// Collect all SymVarRef names referenced in a SymbolicByteCode.
fn sym_var_refs_in_bytecode(sbc: &SymbolicByteCode) -> impl Iterator<Item = &str> {
    sbc.code.iter().filter_map(|op| match op {
        SymbolicOpcode::LoadVar { var }
        | SymbolicOpcode::SymLoadPrev { var }
        | SymbolicOpcode::SymLoadInitial { var }
        | SymbolicOpcode::LoadSubscript { var }
        | SymbolicOpcode::AssignCurr { var }
        | SymbolicOpcode::AssignConstCurr { var, .. }
        | SymbolicOpcode::BinOpAssignCurr { var, .. }
        | SymbolicOpcode::BinOpAssignNext { var, .. }
        | SymbolicOpcode::PushVarViewDirect { var, .. } => Some(var.name.as_str()),
        _ => None,
    })
}

/// Returns true if all SymVarRef names in `fragment` are present in `layout`.
///
/// LTM synthetic fragments compiled for a sub-model may reference variable
/// names that exist only in the root model's namespace (e.g. implicit stdlib
/// module instance names like "smth1" instead of "$:var_name:0:smth1").
/// Calling this before inserting an LTM fragment into `all_fragments` lets the
/// assembler silently drop unresolvable fragments rather than failing the
/// entire compilation.
pub(crate) fn fragment_vars_in_layout(
    fragment: &CompiledVarFragment,
    layout: &VariableLayout,
) -> bool {
    let phases = [
        fragment.initial_bytecodes.as_ref().map(|p| &p.symbolic),
        fragment.flow_bytecodes.as_ref().map(|p| &p.symbolic),
        fragment.stock_bytecodes.as_ref().map(|p| &p.symbolic),
    ];
    for maybe_bc in &phases {
        let Some(bc) = maybe_bc else { continue };
        if sym_var_refs_in_bytecode(bc).any(|name| layout.get(name).is_none()) {
            return false;
        }
    }
    // Also check SymbolicModuleDecl var references in each phase
    let phase_decls = [
        fragment.initial_bytecodes.as_ref().map(|p| &p.module_decls),
        fragment.flow_bytecodes.as_ref().map(|p| &p.module_decls),
        fragment.stock_bytecodes.as_ref().map(|p| &p.module_decls),
    ];
    for maybe_decls in &phase_decls {
        let Some(decls) = maybe_decls else { continue };
        if decls
            .iter()
            .any(|d| layout.get(d.var.name.as_str()).is_none())
        {
            return false;
        }
    }
    true
}

// ============================================================================
// Resolve: Symbolic -> Concrete (Assembly)
// ============================================================================

/// Check that a module layout of `n_slots` slots is addressable by the
/// bytecode's `VariableOffset` (u16) offsets.
///
/// A module with `n_slots` slots uses offsets `0..n_slots`, so the largest
/// offset is `n_slots - 1`; anything past `u16::MAX + 1` slots has at least
/// one unaddressable slot. Without this check a silent `as u16` cast wraps
/// such offsets back into the low slots -- the variable at offset 65,536
/// overwrites slot 0 (`time`), freezing simulated time and corrupting every
/// result. Very large LTM-instrumented models (C-LEARN in discovery mode
/// needs ~171k slots) are the practical way to hit this.
pub(crate) fn check_layout_addressable(n_slots: usize, model_name: &str) -> Result<(), String> {
    const MAX_SLOTS: usize = (VariableOffset::MAX as usize) + 1;
    if n_slots > MAX_SLOTS {
        return Err(format!(
            "model '{model_name}' requires {n_slots} result slots, which exceeds the \
             bytecode VM's addressable limit of {MAX_SLOTS} (variable offsets are 16-bit). \
             This typically happens when LTM is enabled on a very large model: run the \
             simulation without LTM (analyze_loops=False / enable_ltm=false), or reduce \
             the model's LTM instrumentation."
        ));
    }
    Ok(())
}

pub(crate) fn resolve_var_ref(
    var: &SymVarRef,
    layout: &VariableLayout,
) -> Result<VariableOffset, String> {
    let entry = layout.get(var.name.as_str()).ok_or_else(|| {
        format!(
            "variable '{}' not found in layout during resolution",
            var.name
        )
    })?;
    if var.element_offset >= entry.size {
        return Err(format!(
            "element_offset {} out of bounds for variable '{}' (size {})",
            var.element_offset, var.name, entry.size
        ));
    }
    let off = entry.offset + var.element_offset;
    // Checked narrowing: a silent `as` cast here wraps offsets past u16::MAX
    // into the low slots, overwriting time/dt/initial_time/final_time and
    // corrupting the whole simulation. `check_layout_addressable` catches this
    // before assembly starts; this is the defense-in-depth backstop for any
    // path that reaches resolution with an oversized layout.
    VariableOffset::try_from(off).map_err(|_| {
        format!(
            "variable '{}' resolves to result slot {off}, beyond the bytecode VM's \
             u16 offset limit of {}",
            var.name,
            VariableOffset::MAX
        )
    })
}

pub(crate) fn resolve_opcode(
    op: &SymbolicOpcode,
    layout: &VariableLayout,
) -> Result<Opcode, String> {
    match op {
        // Opcodes with symbolic variable references
        SymbolicOpcode::LoadVar { var } => Ok(Opcode::LoadVar {
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::SymLoadPrev { var } => Ok(Opcode::LoadPrev {
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::SymLoadInitial { var } => Ok(Opcode::LoadInitial {
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::LoadSubscript { var } => Ok(Opcode::LoadSubscript {
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::AssignCurr { var } => Ok(Opcode::AssignCurr {
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::AssignConstCurr { var, literal_id } => Ok(Opcode::AssignConstCurr {
            off: resolve_var_ref(var, layout)?,
            literal_id: *literal_id,
        }),
        SymbolicOpcode::BinOpAssignCurr { op, var } => Ok(Opcode::BinOpAssignCurr {
            op: *op,
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::BinOpAssignNext { op, var } => Ok(Opcode::BinOpAssignNext {
            op: *op,
            off: resolve_var_ref(var, layout)?,
        }),
        SymbolicOpcode::PushVarViewDirect { var, dim_list_id } => Ok(Opcode::PushVarViewDirect {
            base_off: resolve_var_ref(var, layout)?,
            dim_list_id: *dim_list_id,
        }),

        // Opcodes that pass through unchanged
        SymbolicOpcode::Op2 { op } => Ok(Opcode::Op2 { op: *op }),
        SymbolicOpcode::Not {} => Ok(Opcode::Not {}),
        SymbolicOpcode::LoadConstant { id } => Ok(Opcode::LoadConstant { id: *id }),
        SymbolicOpcode::LoadGlobalVar { off } => Ok(Opcode::LoadGlobalVar { off: *off }),
        SymbolicOpcode::PushSubscriptIndex { bounds } => {
            Ok(Opcode::PushSubscriptIndex { bounds: *bounds })
        }
        SymbolicOpcode::SetCond {} => Ok(Opcode::SetCond {}),
        SymbolicOpcode::If {} => Ok(Opcode::If {}),
        SymbolicOpcode::Ret => Ok(Opcode::Ret),
        SymbolicOpcode::LoadModuleInput { input } => Ok(Opcode::LoadModuleInput { input: *input }),
        SymbolicOpcode::EvalModule { id, n_inputs } => Ok(Opcode::EvalModule {
            id: *id,
            n_inputs: *n_inputs,
        }),
        SymbolicOpcode::Apply { func } => Ok(Opcode::Apply { func: *func }),
        SymbolicOpcode::Lookup {
            base_gf,
            table_count,
            mode,
        } => Ok(Opcode::Lookup {
            base_gf: *base_gf,
            table_count: *table_count,
            mode: *mode,
        }),
        SymbolicOpcode::LookupDirect {
            base_gf,
            elem,
            mode,
            ..
        } => Ok(Opcode::LookupDirect {
            base_gf: *base_gf,
            elem: *elem,
            mode: *mode,
        }),
        SymbolicOpcode::PushStaticView { view_id } => {
            Ok(Opcode::PushStaticView { view_id: *view_id })
        }
        SymbolicOpcode::ViewSubscriptDynamic { dim_idx } => {
            Ok(Opcode::ViewSubscriptDynamic { dim_idx: *dim_idx })
        }
        SymbolicOpcode::ViewRangeDynamic { dim_idx } => {
            Ok(Opcode::ViewRangeDynamic { dim_idx: *dim_idx })
        }
        SymbolicOpcode::PopView {} => Ok(Opcode::PopView {}),
        SymbolicOpcode::LoadTempConst { temp_id, index } => Ok(Opcode::LoadTempConst {
            temp_id: *temp_id,
            index: *index,
        }),
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => Ok(Opcode::BeginIter {
            write_temp_id: *write_temp_id,
            has_write_temp: *has_write_temp,
        }),
        SymbolicOpcode::LoadIterViewAt { offset } => Ok(Opcode::LoadIterViewAt { offset: *offset }),
        SymbolicOpcode::StoreIterElement {} => Ok(Opcode::StoreIterElement {}),
        SymbolicOpcode::NextIterOrJump { jump_back } => Ok(Opcode::NextIterOrJump {
            jump_back: *jump_back,
        }),
        SymbolicOpcode::EndIter {} => Ok(Opcode::EndIter {}),
        SymbolicOpcode::ArraySum {} => Ok(Opcode::ArraySum {}),
        SymbolicOpcode::ArrayMax {} => Ok(Opcode::ArrayMax {}),
        SymbolicOpcode::ArrayMin {} => Ok(Opcode::ArrayMin {}),
        SymbolicOpcode::ArrayMean {} => Ok(Opcode::ArrayMean {}),
        SymbolicOpcode::ArrayStddev {} => Ok(Opcode::ArrayStddev {}),
        SymbolicOpcode::ArraySize {} => Ok(Opcode::ArraySize {}),
        SymbolicOpcode::VectorSelect {} => Ok(Opcode::VectorSelect {}),
        SymbolicOpcode::VectorElmMap {
            write_temp_id,
            full_source_len,
        } => Ok(Opcode::VectorElmMap {
            write_temp_id: *write_temp_id,
            // `full_source_len` is the source variable's ABSOLUTE element
            // count (`vm_vector_elm_map`'s full-array-vs-strict-slice
            // threshold and the out-of-range `[0, full_source_len)` -> NaN
            // guard). It is NOT a renumber-able resource id like
            // temp/lit/gf/view/dim_list/module: it is invariant under
            // `renumber_opcode` and copied through unchanged on fragment
            // concatenation (see the matching arm in `renumber_opcode`).
            //
            // The genuine-Vensim `.dat` simulate corpus (`vector_simple.dat` /
            // `vector.dat`) deliberately has no out-of-range offset and no
            // shape that flips the full-array branch, so a wrong
            // `full_source_len` is invisible through `simulates_vector_simple_mdl`
            // / `simulates_vector_xmile_genuine` alone. The NUMERIC end-to-end
            // coverage (GH #579) therefore lives in `array_tests`: the
            // full-array-source `out_of_bounds_element_returns_nan_vm`
            // (base 0, `source_is_full_array == true`) and the
            // strict-slice-source `strict_slice_source_oob_returns_nan_vm`
            // (base != 0, the other branch) both feed an
            // out-of-range offset, so a `full_source_len` corrupted in EITHER
            // the codegen computation (`codegen::full_source_len`) OR this
            // `resolve`/`renumber_opcode` path stops yielding the expected NaN
            // and the assertions fail loudly (verified by hard-forcing a wrong
            // constant in both sites). The structural symbolic round-trip --
            // `test_renumber_vector_builtin_temp_ids` (isolated `renumber_opcode`)
            // and `test_vector_elm_map_full_source_len_survives_fragment_roundtrip`
            // (the full `concatenate_fragments` -> `resolve_bytecode`
            // merge path) -- complements them by pinning that this field is
            // invariant under renumbering (it is NOT a renumber-able resource id
            // like temp/lit/gf/view/dim_list/module).
            full_source_len: *full_source_len,
        }),
        SymbolicOpcode::VectorSortOrder { write_temp_id } => Ok(Opcode::VectorSortOrder {
            write_temp_id: *write_temp_id,
        }),
        SymbolicOpcode::Rank { write_temp_id } => Ok(Opcode::Rank {
            write_temp_id: *write_temp_id,
        }),
        SymbolicOpcode::LookupArray {
            base_gf,
            table_count,
            mode,
            write_temp_id,
        } => Ok(Opcode::LookupArray {
            base_gf: *base_gf,
            table_count: *table_count,
            mode: *mode,
            write_temp_id: *write_temp_id,
        }),
        SymbolicOpcode::AllocateAvailable { write_temp_id } => Ok(Opcode::AllocateAvailable {
            write_temp_id: *write_temp_id,
        }),
        SymbolicOpcode::AllocateByPriority { write_temp_id } => Ok(Opcode::AllocateByPriority {
            write_temp_id: *write_temp_id,
        }),
    }
}

/// Resolve a symbolic bytecode stream against `layout`, producing the concrete
/// bytecode the VM executes.
///
/// **This is the only place concrete bytecode is born**, and therefore the place
/// the VM's stack-safety proof is discharged: the emitted program must not be
/// able to overflow the VM's fixed-size arithmetic stack, which is what makes
/// `vm::Stack`'s unchecked accesses sound.
///
/// The check used to live in the per-fragment `ByteCodeBuilder::finish()`. Moving
/// it here made it strictly STRONGER, not merely equivalent: a fragment starts
/// and ends at depth 0 today, so per-fragment maxima and the concatenation's
/// maximum agree -- but `concatenate_fragments` strips each fragment's trailing
/// `Ret` and appends, so a fragment that did NOT balance would accumulate depth
/// across fragment boundaries in a way the per-fragment check could not see.
/// It also runs once per assembled phase instead of once per fragment.
///
/// Both failure modes are reported, not asserted: an over-deep program and an
/// `Opcode::stack_effect` underflow (a compiler-metadata bug) both come back as
/// `Err`, so neither can abort a `libsimlin` host process from inside a
/// `Result`-returning function.
pub(crate) fn resolve_bytecode(
    sbc: &SymbolicByteCode,
    layout: &VariableLayout,
) -> Result<ByteCode, String> {
    let code = sbc
        .code
        .iter()
        .map(|op| resolve_opcode(op, layout))
        .collect::<Result<Vec<_>, _>>()?;

    let bc = ByteCode {
        literals: sbc.literals.clone(),
        code,
    };

    let depth = bc.max_stack_depth()?;
    if depth >= STACK_CAPACITY {
        return Err(format!(
            "compiled bytecode requires stack depth {depth}, exceeding VM capacity \
             {STACK_CAPACITY}"
        ));
    }

    Ok(bc)
}

pub(crate) fn resolve_static_view(
    sv: &SymbolicStaticView,
    layout: &VariableLayout,
) -> Result<StaticArrayView, String> {
    // The three chunk-shaped regions share `curr`'s slot numbering (each is an
    // `n_slots` snapshot of it), so one layout lookup serves all three and only
    // the region tag differs.
    let resolve_var = |var_ref: &SymVarRef| -> Result<u32, String> {
        let entry = layout.get(var_ref.name.as_str()).ok_or_else(|| {
            format!(
                "variable '{}' not found in layout during static view resolution",
                var_ref.name
            )
        })?;
        Ok((entry.offset + var_ref.element_offset) as u32)
    };
    let (base_off, storage) = match &sv.base {
        SymStaticViewBase::Var(var_ref) => (resolve_var(var_ref)?, ViewStorage::Curr),
        SymStaticViewBase::PrevVar(var_ref) => (resolve_var(var_ref)?, ViewStorage::Prev),
        SymStaticViewBase::InitialVar(var_ref) => (resolve_var(var_ref)?, ViewStorage::Initial),
        // A view base is the ONE place a temp id is carried as a `u32`. Every
        // OTHER opcode that names a temp -- `BeginIter` and the
        // array-producing opcodes' `write_temp_id`, `LoadTempConst`'s
        // `temp_id` -- carries it as `TempId` (= `u8`), narrowed at emit time
        // with a plain `as`. So a view over a temp above 255 reads storage
        // nothing ever wrote: the writer's `as TempId` lands on `id % 256`
        // while this read lands on `id`, and the program is well-formed either
        // way -- wrong numbers with no diagnostic. Reject it in the resolution
        // layer, where the concrete program is produced. Per-element
        // materialization reuses one bounded id range; this guard covers a
        // fragment that genuinely needs more than 256 concurrent temps. The
        // temp bullet under "Standing invariants of the compiler" in the
        // crate's CLAUDE.md states the rule.
        SymStaticViewBase::Temp(id) => {
            if *id > TempId::MAX as u32 {
                return Err(format!(
                    "a view over temp {} exceeds TempId capacity (u8::MAX = {}); \
                     every writer of a temp narrows its id to u8, so this view \
                     would read storage no opcode writes",
                    id,
                    TempId::MAX
                ));
            }
            (*id, ViewStorage::Temp)
        }
    };

    Ok(StaticArrayView {
        base_off,
        storage,
        dims: sv.dims.clone(),
        strides: sv.strides.clone(),
        offset: sv.offset,
        sparse: sv.sparse.clone(),
        dim_ids: sv.dim_ids.clone(),
    })
}

pub(crate) fn resolve_module_decl(
    sd: &SymbolicModuleDecl,
    layout: &VariableLayout,
) -> Result<ModuleDeclaration, String> {
    let entry = layout.get(sd.var.name.as_str()).ok_or_else(|| {
        format!(
            "module variable '{}' not found in layout during resolution",
            sd.var.name
        )
    })?;

    Ok(ModuleDeclaration {
        model_name: sd.model_name.clone(),
        input_set: sd.input_set.clone(),
        off: entry.offset + sd.var.element_offset,
    })
}

/// Convert a `SymbolicCompiledModule` back to a `CompiledModule` by
/// resolving all symbolic variable references using the given layout.
pub(crate) fn resolve_module(
    sym: &SymbolicCompiledModule,
    layout: &VariableLayout,
) -> Result<CompiledModule, String> {
    let compiled_initials: Vec<CompiledInitial> = sym
        .compiled_initials
        .iter()
        .map(|sci| {
            let bytecode = resolve_bytecode(&sci.bytecode, layout)?;
            // Re-derive offsets from the resolved bytecode
            let offsets = extract_assign_curr_offsets(&bytecode);
            Ok(CompiledInitial {
                ident: sci.ident.clone(),
                offsets,
                bytecode,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    // `resolve_module` is the single symbolic -> concrete primitive, so the
    // 3-address fusion (R2) is NOT applied here -- `Vm::new` applies it to its
    // own private copy of the bytecode, after the salsa-cached artifact has
    // been produced.
    let compiled_flows = resolve_bytecode(&sym.compiled_flows, layout)?;
    let compiled_stocks = resolve_bytecode(&sym.compiled_stocks, layout)?;

    let static_views = sym
        .static_views
        .iter()
        .map(|sv| resolve_static_view(sv, layout))
        .collect::<Result<Vec<_>, _>>()?;

    let module_decls = sym
        .module_decls
        .iter()
        .map(|md| resolve_module_decl(md, layout))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(CompiledModule {
        ident: sym.ident.clone(),
        n_slots: layout.n_slots,
        context: Arc::new(ByteCodeContext {
            graphical_functions: sym.graphical_functions.clone(),
            modules: module_decls,
            dimensions: sym.dimensions.clone(),
            names: sym.names.clone(),
            static_views,
            temp_offsets: sym.temp_offsets.clone(),
            temp_total_size: sym.temp_total_size,
            dim_lists: sym.dim_lists.clone(),
        }),
        compiled_initials: Arc::new(compiled_initials),
        compiled_flows: Arc::new(compiled_flows),
        compiled_stocks: Arc::new(compiled_stocks),
        // `resolve_bytecode` is opcode-count-preserving and `resolve_module`
        // does no fusion, so the symbolic prefix boundary is the concrete
        // prefix boundary (GH #712).
        flows_invariant_opcode_len: sym.flows_invariant_opcode_len,
    })
}

/// Extract sorted, deduplicated AssignCurr target offsets from a ByteCode.
pub(crate) fn extract_assign_curr_offsets(bc: &ByteCode) -> Vec<usize> {
    let mut offsets: Vec<usize> = bc
        .code
        .iter()
        .filter_map(|op| match op {
            Opcode::AssignCurr { off } | Opcode::AssignConstCurr { off, .. } => Some(*off as usize),
            Opcode::BinOpAssignCurr { off, .. } => Some(*off as usize),
            _ => None,
        })
        .collect();
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

// ============================================================================
// Fragment Concatenation
// ============================================================================

/// How many entries a `u16`-addressed merged resource table can hold: ids run
/// `0..=u16::MAX`.
const U16_ID_CAPACITY: usize = u16::MAX as usize + 1;

#[cfg(test)]
thread_local! {
    static ID_CAPACITY_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The capacity [`resource_base`] bounds against: `U16_ID_CAPACITY`, except
/// under an [`IdCapacityGuard`] in a test.
///
/// The override exists so a test can reach the bound with a tiny fixture
/// instead of one large enough to genuinely fill a 16-bit id space -- a
/// 65,537-entry model would blow the per-test time budget many times over
/// (`docs/dev/rust.md#test-time-budgets`). It does not add a second statement
/// of the bound: the comparison still happens once, in `resource_base`; only
/// the constant it compares against moves.
#[inline]
fn id_capacity() -> usize {
    #[cfg(test)]
    if let Some(cap) = ID_CAPACITY_OVERRIDE.with(|c| c.get()) {
        return cap;
    }
    U16_ID_CAPACITY
}

/// Shrink the resource-id capacity for the current thread, restoring it on
/// drop. Thread-local because tests run in parallel.
#[cfg(test)]
pub(crate) struct IdCapacityGuard;

#[cfg(test)]
impl IdCapacityGuard {
    pub(crate) fn new(capacity: usize) -> Self {
        ID_CAPACITY_OVERRIDE.with(|c| c.set(Some(capacity)));
        IdCapacityGuard
    }
}

#[cfg(test)]
impl Drop for IdCapacityGuard {
    fn drop(&mut self) {
        ID_CAPACITY_OVERRIDE.with(|c| c.set(None));
    }
}

/// The base id for a fragment's `frag_len` entries appended to a merged table
/// that already holds `merged_len` (M2 + M3).
///
/// **This is the only place the `u16` capacity bound is stated.** Every base a
/// `u16`-addressed resource is renumbered against comes from here -- the
/// merger's four -- so the boundary cannot be one value in one place and a
/// different one in another. Everything upstream counts in `usize`.
///
/// The ids this fragment will use are `base .. base + frag_len`, so the check
/// is that the LAST of them is still representable: `base + frag_len` must not
/// exceed the table capacity. Computed in `usize` and narrowed once, so neither
/// the base nor the end can wrap on the way to the comparison.
///
/// **`end > U16_ID_CAPACITY` is the load-bearing line, and the `>` is exact.**
/// `end` is one past the last id, so `end == U16_ID_CAPACITY` means the last id
/// is `u16::MAX` -- addressable. Tightening it to `>=` rejects a table that
/// fills the id space exactly, which is a legal program; a reader "simplifying"
/// it that way is the live risk, and `literal_pool_past_u16_capacity_fails_loud`
/// / `a_full_table_bounds_only_the_programs_that_name_it` /
/// `a_standalone_program_shares_the_tables_bound` all red if they do.
///
/// A fragment carrying NONE of this resource is exempt: it names no id, so
/// there is nothing to represent, and the base it is handed is never used. Note
/// what that exemption does and does not buy. It does NOT move the boundary --
/// deleting it leaves every test green, because the check above already admits
/// a full table. What it buys is that `base as u16` cannot WRAP when `base` is
/// exactly `U16_ID_CAPACITY`: the value is dead (no caller reads a base for a
/// fragment with no entries), but returning a saturated 65,535 rather than a
/// wrapped 0 keeps the dead value from ever being mistaken for a live one.
pub(crate) fn resource_base(
    merged_len: usize,
    frag_len: usize,
    label: &str,
) -> Result<u16, String> {
    let base = merged_len;
    let end = base + frag_len;
    if frag_len == 0 {
        // No id to assign; hand back a base the caller will not use, saturated
        // so the narrowing itself cannot wrap.
        return Ok(base.min(u16::MAX as usize) as u16);
    }
    let capacity = id_capacity();
    if end > capacity {
        return Err(format!(
            "merged {label} count {end} exceeds the bytecode id capacity of \
             {capacity} (ids are 16-bit)"
        ));
    }
    Ok(base as u16)
}

/// Add a fragment-local resource id to its fragment's base, reporting M3
/// rather than wrapping. The `u16` twin of `checked_add_u8`.
pub(crate) fn checked_add_u16(base: u16, off: u16, label: &str) -> Result<u16, String> {
    base.checked_add(off).ok_or_else(|| {
        format!(
            "{label} overflow: {base} + {off} exceeds u16::MAX ({})",
            u16::MAX
        )
    })
}

/// The five flat resource-ID base offsets a single fragment's non-GF
/// opcodes are renumbered by (the result of absorbing that fragment into a
/// `FragmentMerger`). Graphical-function IDs are NOT a flat offset -- they
/// are content-de-duplicated (#582), so they are remapped per local slot
/// via the companion `GfRemap` rather than a single base. Pass both to
/// `renumber_opcode` / `renumber_fragment_code`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FragmentResourceOffsets {
    pub lit_offset: u16,
    pub mod_offset: u16,
    pub view_offset: u16,
    pub temp_offset: u32,
    pub dl_offset: u16,
}

/// The four bases for the SHARED CONTEXT resources alone -- what
/// [`FragmentMerger::absorb_context`] can honestly report.
///
/// Separate from [`FragmentResourceOffsets`] so that "this absorb assigned no
/// literal id" is a property of the type rather than a placeholder value: the
/// context tables span every program of a module while a literal pool belongs
/// to one, and a `lit_offset: 0` sitting in a context-only result would be
/// indistinguishable from a real base of zero.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ContextResourceOffsets {
    pub mod_offset: u16,
    pub view_offset: u16,
    pub temp_offset: u32,
    pub dl_offset: u16,
}

impl ContextResourceOffsets {
    /// Pair these context bases with the literal base of the same absorb.
    fn with_literals(self, lit_offset: u16) -> FragmentResourceOffsets {
        FragmentResourceOffsets {
            lit_offset,
            mod_offset: self.mod_offset,
            view_offset: self.view_offset,
            temp_offset: self.temp_offset,
            dl_offset: self.dl_offset,
        }
    }
}

/// Per-fragment local-GF-slot -> global-(deduped)-GF-slot map. Index `i`
/// holds the `merged_gf` index that this fragment's local
/// `graphical_functions[i]` was de-duplicated to. Total over `[0, gf_len)`,
/// so a `Lookup`/`LookupArray` `base_gf` is remapped by a single
/// `gf_remap[base_gf]` lookup; the whole-list shift in
/// `FragmentMerger::absorb_gf` guarantees `gf_remap[base + k] ==
/// gf_remap[base] + k`, so the array-lookup `[base .. base + table_count]`
/// contract survives the remap.
pub(crate) type GfRemap = SmallVec<[GraphicalFunctionId; 8]>;

/// How a `FragmentMerger` lays out fragment *temps* (#583).
///
/// Temps are per-variable scratch arrays (the result storage of array-
/// producing builtins like `VectorSortOrder`/`VectorElmMap`): a fragment is
/// one variable's bytecode, its temps are 0-based, and they are written and
/// read entirely within that variable's expression evaluation -- dead once
/// the variable's runlist segment completes.
///
/// Temps are therefore the ONE merged resource that may legitimately be
/// shared between fragments, and obligation M5 is what bounds the sharing:
/// **in the merged opcode stream, the uses of one merged temp slot must not
/// interleave between two fragments.** Two fragments may share a slot only if
/// the emitter lays their opcodes out as disjoint contiguous runs; if it
/// interleaves them, a shared slot means one fragment reads storage the other
/// has already overwritten. The strategy is how the caller declares which
/// emission shape it is about to produce:
///
/// - `Recycle` -- the caller emits each fragment's opcodes as one contiguous
///   run, in fragment order (`FragmentMerger::concatenate`; the fragments are
///   sequential, non-overlapping runlist segments), or as a program of its own
///   (`FragmentMerger::standalone_program`). Temps collapse by
///   IDENTITY into one shared pool: fragment A's temp 0 and fragment B's
///   temp 0 both become slot `base + 0`, and that slot's size is the MAX over
///   its users, so it is large enough for each of them in turn. Sharing is
///   safe because no two users are live at the same time, and it is necessary
///   because summing 0-based per-fragment temp counts across a whole model
///   overflows the `TempId` (= `u8`) namespace -- the GH #583 failure on
///   C-LEARN, whose flows phase legitimately needs 347 summed slots but far
///   fewer recycled ones. The `u8` width is why recycling is the fix rather
///   than an optimization; widening the id type was the rejected alternative
///   (see #583).
///
/// - `Sum` -- the caller INTERLEAVES fragments' opcodes (`combine_scc_fragment`
///   emits a resolved recurrence SCC's members' per-element segments in
///   `element_order`, so `ce[0], ecc[0], ce[1], ecc[1]`). Members' temp live
///   ranges overlap, so identity recycling would alias two simultaneously-live
///   temps and silently miscompile the SCC. Each fragment gets a DISJOINT id
///   range instead, which trivially satisfies M5 because no slot has two users
///   at all. An SCC is a handful of members, so the summed count stays far
///   inside `u8`.
///
/// Both arms are pinned as the live-range property itself rather than as a
/// layout comparison: see `merged_temp_slot_uses_never_interleave` and
/// `recycle_shares_by_identity_and_sizes_by_max` in
/// `symbolic_merge_proptest.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TempStrategy {
    /// Collapse fragment temps by identity into one shared pool, each slot
    /// sized to the max of its users. For an emitter that lays fragments out
    /// as disjoint contiguous runs.
    Recycle,
    /// Give each fragment a disjoint temp id range. For an emitter that
    /// interleaves fragments' opcodes.
    Sum,
}

/// Running merge state for combining `PerVarBytecodes` into a single
/// resource namespace.
///
/// One merger assembles one module. `assemble_module` drives it through the
/// module's programs in order -- every initial as a [`standalone_program`],
/// then the flows and the stocks as one [`concatenate`] each -- and finishes
/// it with [`into_side_channels`]: the module's one module-decl / static-view /
/// temp / dim-list / graphical-function table set, which every program's ids
/// index. `combine_scc_fragment` drives a second, isolated merger per resolved
/// recurrence SCC, absorbing each member once ([`absorb`]) and renumbering the
/// member's per-element segments itself before interleaving them; the combined
/// fragment is itself a fragment the module's merger later absorbs. Both
/// consumers go through the same absorption, so they cannot drift.
///
/// [`standalone_program`]: FragmentMerger::standalone_program
/// [`concatenate`]: FragmentMerger::concatenate
/// [`into_side_channels`]: FragmentMerger::into_side_channels
/// [`absorb`]: FragmentMerger::absorb
///
/// # What the merged fragment owes
///
/// A fragment is one variable's one phase, compiled against nothing but its
/// own resources: its opcodes carry ids into its OWN `literals`,
/// `graphical_functions`, `module_decls`, `static_views`, temp slots and
/// `dim_lists`. Merging relocates those private id spaces into one shared
/// space. These are the obligations that makes sound, stated as properties of
/// the merged fragment -- not as a comparison with any other compiler. Each
/// names the test that pins it. Unqualified test names are in the sibling
/// `symbolic_merge_proptest.rs`; the interleaving consumer's half lives in
/// `db/combined_fragment_proptest.rs`.
///
/// **M1 -- referential integrity.** For every absorbed fragment `F` and every
/// opcode of `F`, the renumbered opcode names the same resource VALUE in the
/// merged tables that the original named in `F`'s own tables:
/// `merged.literals[id + lit_off] == F.literals[id]`, and likewise for module
/// decls, static views (up to M5's temp-base shift), dim lists (up to the
/// 4-element truncation), and every table of a GF run. This is the whole
/// point of the merge; M2-M4 are the means. Pinned by
/// `merged_ids_dereference_to_their_own_fragments_resources`, with
/// `forced_rich_fragments_exercise_every_resource_kind` as its non-vacuity
/// guard.
///
/// **M2 -- flat resources are disjoint.** Literals, module decls, static views
/// and dim lists are APPENDED: fragment `i`'s entries occupy
/// `[base_i, base_i + |R_i|)`, those ranges are pairwise disjoint, and
/// together they tile the merged table in fragment order. No fragment can
/// reach another's entry, so M1 is never satisfied by accidental sharing.
/// Pinned by `flat_resource_ranges_are_disjoint_and_tile_the_merged_table`.
///
/// **M3 -- ids fit their bytecode id type.** Every ASSIGNED id is
/// representable in the type the opcode field carries (`LiteralId`/`ModuleId`/
/// `ViewId`/`DimListId` are `u16`, `TempId` and `GraphicalFunctionId` are
/// `u8`). A merged table that outgrows its id type is a loud `Err`, never a
/// wrapped id silently naming a different resource. Note what the obligation
/// does NOT say: a table of exactly `U16_ID_CAPACITY` entries satisfies it, and
/// so does any number of later programs that add nothing to one -- so the
/// tables are counted in `usize` and the bound is discharged in exactly one
/// function, [`resource_base`], the only point that sees both a table's length
/// and the length of the fragment about to extend it. The bound is per table
/// the module KEEPS: each program's literal pool is its own, so a module whose
/// pools each fit assembles even when they would not fit as one (roughly 33k
/// scalar stocks, inside the size class this compiler targets). Pinned by
/// `literal_pool_past_u16_capacity_fails_loud` (within a program),
/// `a_full_table_bounds_only_the_programs_that_name_it` (across programs),
/// `a_standalone_program_shares_the_tables_bound`,
/// `literal_pools_are_per_program` and
/// `assembly_bounds_the_retained_pools_and_not_the_aggregate` (per-program
/// pools), `renumber_opcode_u16_addition_overflow_is_loud`,
/// `test_renumber_opcode_u8_addition_overflow` and
/// `test_concatenate_genuinely_distinct_gf_over_capacity_fails_loud`.
///
/// **M4 -- graphical functions share by content, and only by content.** GFs
/// are the one resource that deliberately does NOT get a disjoint range: N
/// consumers of one dependency's arrayed GF each re-extract the same tables,
/// and appending all N is how the `u8` `GraphicalFunctionId` overflowed
/// (#582). Two local slots may map to one merged id only when their content
/// is bit-identical; a `Lookup`/`LookupArray` run `[b, b + table_count)` stays
/// contiguous and in order; and the remap is TOTAL over `[0, gf_len)`, so even
/// a slot no opcode reads keeps its own content. Pinned by
/// `gf_dedup_preserves_runs_and_never_merges_distinct_content`.
///
/// **M5 -- temps never alias between simultaneously-live fragments.** The one
/// shareable resource; see [`TempStrategy`] for the live-range argument and
/// which strategy a given emission shape needs. `temp_offsets` is the prefix
/// sum of the merged sizes, so distinct slots' storage never overlaps either.
/// Pinned by `merged_temp_slot_uses_never_interleave` and
/// `recycle_shares_by_identity_and_sizes_by_max` for the sequential emitter,
/// and by `db::combined_fragment_proptest`'s
/// `interleaved_members_never_share_a_temp_slot` for the interleaving one.
///
/// **M6 -- absorption is 1:1 on opcodes.** A concatenated program is each
/// fragment's Ret-stripped opcodes, contiguous and in fragment order, plus one
/// terminal `Ret`; a standalone program is its fragment's opcodes verbatim,
/// `Ret` included. Two downstream boundaries are COUNTS into the concatenated
/// stream and would move silently if it were not 1:1: `flows_invariant_opcode_len`
/// (the run-invariant flow prefix, GH #712, computed in `db::assemble` as a sum
/// of Ret-stripped fragment lengths) and the SCC per-element segmentation.
/// Pinned by `merge_is_one_to_one_on_opcodes_and_prefix_lengths_are_boundaries`
/// -- which asserts the stronger form the boundary actually needs, that a
/// fragment's renumbering never depends on what FOLLOWS it -- and, for the
/// interleaving emitter, by `db::combined_fragment_proptest`'s
/// `interleave_conserves_opcodes_and_follows_element_order`.
///
/// **M7 -- variable references are not the merger's business.** Renumbering
/// touches resource ids only: every `SymVarRef` comes through byte-identical
/// and every opcode keeps its variant. Addresses are assigned exactly once,
/// later, by `resolve_module`. Pinned by the `skeleton` half of
/// `merged_ids_dereference_to_their_own_fragments_resources`: the comparison
/// blanks only the resource ids, so every `SymVarRef` and every jump offset
/// must survive byte-identical.
///
/// **M8 -- every program of a module indexes one table set.** A module keeps
/// three kinds of program -- each initial, which `eval_initials` runs on its
/// own; the flows; the stocks -- and ONE set of context tables. The merger
/// assigns a fragment's module / view / temp / dim-list ids from its running
/// tables whichever program the fragment lands in, so an id any program carries
/// indexes the tables the merger finishes with. Literals are per program: a
/// program's bytecode carries its own pool, so the pool is taken with the
/// program and the next program's literal ids start at 0 again. Pinned by
/// `module_programs_index_one_table_set`, with
/// `forced_rich_module_programs_use_non_zero_bases` as its non-vacuity guard --
/// a module whose initials carry none of a resource leaves the flows' bases at
/// zero, where the property would pass without testing anything.
pub(crate) struct FragmentMerger {
    temp_strategy: TempStrategy,
    /// The literal pool of the program under construction. Each program keeps
    /// its own pool (M8), so the method that finishes a program takes this and
    /// the next program's literal ids start at 0.
    merged_literals: Vec<f64>,
    merged_gf: Vec<Vec<(f64, f64)>>,
    /// Cross-fragment graphical-function de-duplication index (#582, M4).
    /// Maps a GF *block* -- a maximal contiguous run of one or more lookup
    /// tables, the granularity `Compiler::new` lays a fragment's tables out in
    /// (one run per table-bearing variable, `codegen.rs`) -- keyed by its
    /// bit-exact content, to the global `merged_gf` offset its first
    /// occurrence was appended at. A dependency arrayed GF referenced by N
    /// consumer fragments produces N fragments each carrying the *same* block
    /// (every consumer re-extracts the dependency's `Vec<Table>` -- see
    /// `db/var_fragment.rs`); de-duplicating the block appends it once and
    /// remaps every consumer's `base_gf` by the single shared offset, which is
    /// what keeps the merged count proportional to the DISTINCT tables a model
    /// has rather than to how many fragments mention them.
    ///
    /// A fragment's blocks are the *maximal* contiguous intervals its
    /// `Lookup`/`LookupArray` opcodes reference (overlapping/nested ranges
    /// merged -- a fragment can reference a per-element arrayed GF both as
    /// the whole array `g[D!](x)` => `LookupArray { base, |D| }` and at one
    /// element `g[e](x)` => `Lookup { base + e, 1 }`, which nest), plus one
    /// block per maximal *un-referenced* gap (over-collected dependency
    /// tables -- see `gf_blocks_of_fragment`). The returned per-slot remap
    /// shifts each maximal block as a unit, so an interior/overlapping
    /// `base_gf` lands at `block_new_base + (base_gf - block_old_base)` and
    /// the `[base .. base + table_count]` array-lookup span is preserved.
    /// Value-exact: a block key is its full content, so two genuinely-
    /// different blocks NEVER share an offset (which would silently make a
    /// lookup read the wrong table).
    gf_block_index: HashMap<GfBlockKey, u16>,
    merged_modules: Vec<SymbolicModuleDecl>,
    merged_views: Vec<SymbolicStaticView>,
    merged_temp_sizes: Vec<usize>,
    merged_dim_lists: Vec<(u8, [u16; 4])>,
}

/// De-duplication key for one GF *block*: the bit-exact content of every
/// `(x, y)` point of every table in the block, in order, with a table-
/// boundary marker between tables so that `[[a],[b,c]]` and `[[a,b],[c]]`
/// (same flattened points, different table split) never collide. `f64` is
/// not `Hash`/`Eq`, so points are keyed by `to_bits()`; `-0.0` / `+0.0`
/// hash distinctly, which is the conservative direction (it can only keep
/// two blocks apart, never merge genuinely-distinct ones).
type GfBlockKey = SmallVec<[u64; 16]>;

/// Compute the de-duplication key for one GF block (a table slice).
fn gf_block_key(tables: &[Vec<(f64, f64)>]) -> GfBlockKey {
    let mut key: GfBlockKey = SmallVec::new();
    for table in tables {
        // Boundary marker: the table's point count packed into a NaN bit
        // pattern (sign set + exponent all-ones). Genuine GF points are
        // finite, so a finite point's `to_bits()` never equals this marker --
        // only a NaN point value could collide, and GF data never contains
        // NaN. Worst case if one ever did: a spurious block *distinction*,
        // never an over-merge (which is the only unsound direction).
        key.push(0xFFFF_FFFF_0000_0000 | table.len() as u64);
        for (x, y) in table {
            key.push(x.to_bits());
            key.push(y.to_bits());
        }
    }
    key
}

impl SymbolicOpcode {
    /// The graphical-function BLOCK this opcode references, as
    /// `(base_gf, table_count)` -- i.e. the run `[base_gf, base_gf + table_count)`
    /// in the fragment's own `graphical_functions`.
    ///
    /// This is the SINGLE place that decides whether an opcode carries a
    /// graphical function, and the match is exhaustive with no `_` arm on
    /// purpose: a new variant cannot be added without answering the question
    /// here, which is a compile error rather than a silent omission.
    ///
    /// That matters because the consumer, `gf_blocks_of_fragment`, reconstructs
    /// a fragment's GF block layout by scanning for these runs, and a lookup
    /// opcode it does not recognise is not an error -- the block simply stops
    /// being seen as referenced, collapses into a maximal un-referenced GAP,
    /// and the de-duplicated table layout comes out wrong with no diagnostic
    /// anywhere. Wrong numbers, not a failure. A test cannot close that hole
    /// either: a fixture exercising one lookup opcode passes unchanged when a
    /// second is added and ignored, which is the "a test that pins one arm of
    /// an N-way decision reads exactly like a test that pins the decision"
    /// hazard. Only the compiler covers every arm.
    ///
    /// The same reasoning, and the same shape, as `BuiltinId::arity`.
    pub(crate) fn gf_run(&self) -> Option<(usize, usize)> {
        match self {
            SymbolicOpcode::Lookup {
                base_gf,
                table_count,
                ..
            }
            | SymbolicOpcode::LookupDirect {
                base_gf,
                table_count,
                ..
            }
            | SymbolicOpcode::LookupArray {
                base_gf,
                table_count,
                ..
            } => Some((*base_gf as usize, *table_count as usize)),
            // Every remaining variant, spelled out rather than wildcarded --
            // that is what makes a new one a compile error here.
            SymbolicOpcode::Op2 { .. }
            | SymbolicOpcode::Not { .. }
            | SymbolicOpcode::LoadConstant { .. }
            | SymbolicOpcode::LoadVar { .. }
            | SymbolicOpcode::SymLoadPrev { .. }
            | SymbolicOpcode::SymLoadInitial { .. }
            | SymbolicOpcode::LoadGlobalVar { .. }
            | SymbolicOpcode::PushSubscriptIndex { .. }
            | SymbolicOpcode::LoadSubscript { .. }
            | SymbolicOpcode::SetCond { .. }
            | SymbolicOpcode::If { .. }
            | SymbolicOpcode::Ret
            | SymbolicOpcode::LoadModuleInput { .. }
            | SymbolicOpcode::EvalModule { .. }
            | SymbolicOpcode::AssignCurr { .. }
            | SymbolicOpcode::Apply { .. }
            | SymbolicOpcode::AssignConstCurr { .. }
            | SymbolicOpcode::BinOpAssignCurr { .. }
            | SymbolicOpcode::BinOpAssignNext { .. }
            | SymbolicOpcode::PushStaticView { .. }
            | SymbolicOpcode::PushVarViewDirect { .. }
            | SymbolicOpcode::ViewSubscriptDynamic { .. }
            | SymbolicOpcode::ViewRangeDynamic { .. }
            | SymbolicOpcode::PopView { .. }
            | SymbolicOpcode::LoadTempConst { .. }
            | SymbolicOpcode::BeginIter { .. }
            | SymbolicOpcode::LoadIterViewAt { .. }
            | SymbolicOpcode::StoreIterElement { .. }
            | SymbolicOpcode::NextIterOrJump { .. }
            | SymbolicOpcode::EndIter { .. }
            | SymbolicOpcode::ArraySum { .. }
            | SymbolicOpcode::ArrayMax { .. }
            | SymbolicOpcode::ArrayMin { .. }
            | SymbolicOpcode::ArrayMean { .. }
            | SymbolicOpcode::ArrayStddev { .. }
            | SymbolicOpcode::ArraySize { .. }
            | SymbolicOpcode::VectorSelect { .. }
            | SymbolicOpcode::VectorElmMap { .. }
            | SymbolicOpcode::VectorSortOrder { .. }
            | SymbolicOpcode::Rank { .. }
            | SymbolicOpcode::AllocateAvailable { .. }
            | SymbolicOpcode::AllocateByPriority { .. } => None,
        }
    }
}

/// Reconstruct the GF *block* layout of a single fragment as a list of
/// `(start, len)` blocks covering `[0, gf_len)` exactly, sorted by `start`
/// (#582).
///
/// This is a property of the FRAGMENT, read off its own opcodes, and it needs
/// exactly two things from the emitter that produced it (`Compiler::new` --
/// the one codegen both the fragment path and the test-only whole-model path
/// share): each `base_gf` an opcode carries is the start of some table-bearing
/// variable's run, and distinct variables' runs are disjoint. Both hold
/// because `Compiler::new` assigns every `base_gf` out of one
/// `table_base_ids` map built by laying each variable's tables down
/// contiguously from a monotonically advancing cursor.
///
/// Given that, a `Lookup`/`LookupArray` run `[base_gf, base_gf + table_count)`
/// can be *nested* inside another run but never partially overlap it: a
/// per-element arrayed GF `g` is read both as the whole array
/// (`LookupArray { base, |D| }`) and at one element (`Lookup { base + e, 1 }`,
/// fully inside the array's range). The whole-array run is the real block; the
/// nested element run is a sub-reference, NOT a separate block (splitting the
/// block at the element boundary could scatter the array across the deduped
/// table and miscompile the `[base .. base + table_count]` array lookup). The
/// blocks are therefore the *maximal-by-inclusion* opcode runs (nested runs
/// dropped), plus one block per maximal *un-referenced* gap (over-collected
/// dependency tables `db/var_fragment.rs` gathered but no opcode reads --
/// never read, so an imperfect gap boundary cannot miscompile, only mildly
/// affect the deduped count).
///
/// `Err` only if a run extends past `gf_len` or two runs partially overlap
/// (a corrupt fragment the engine never produces); loud-safe.
fn gf_blocks_of_fragment(frag: &PerVarBytecodes) -> Result<Vec<(usize, usize)>, String> {
    let gf_len = frag.graphical_functions.len();
    if gf_len == 0 {
        return Ok(Vec::new());
    }
    // Collect the distinct opcode runs.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for op in &frag.symbolic.code {
        let Some((base, count)) = op.gf_run() else {
            continue;
        };
        if count == 0 {
            continue;
        }
        let end = base
            .checked_add(count)
            .filter(|&e| e <= gf_len)
            .ok_or_else(|| {
                format!(
                    "GF run [{base}, {base}+{count}) extends past \
                     graphical_functions length {gf_len}"
                )
            })?;
        if !runs.contains(&(base, end)) {
            runs.push((base, end));
        }
    }
    // Keep only the maximal-by-inclusion runs (drop a run strictly
    // contained in another), and verify the survivors are pairwise disjoint
    // (partial overlap is corrupt). Sorting by (start, Reverse(end)) puts a
    // container immediately before the runs it contains.
    runs.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    let mut maximal: Vec<(usize, usize)> = Vec::new();
    for (start, end) in runs {
        match maximal.last() {
            Some(&(_, prev_end)) if end <= prev_end => {
                // Nested inside the previous (wider, same-or-earlier start)
                // run -- a sub-reference, not a separate block.
                continue;
            }
            Some(&(_, prev_end)) if start < prev_end => {
                return Err(format!(
                    "GF runs partially overlap (.. {prev_end}) vs ({start} ..) \
                     in one fragment"
                ));
            }
            _ => maximal.push((start, end)),
        }
    }
    // Emit the maximal runs as blocks, filling each un-referenced gap
    // (including before the first / after the last run) with its own block.
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    let mut cursor = 0usize;
    for (start, end) in maximal {
        if start > cursor {
            blocks.push((cursor, start - cursor));
        }
        blocks.push((start, end - start));
        cursor = end;
    }
    if cursor < gf_len {
        blocks.push((cursor, gf_len - cursor));
    }
    Ok(blocks)
}

impl FragmentMerger {
    /// A merger over empty tables. `temp_strategy` declares the emission shape
    /// the caller will produce (M5): `Recycle` for a module's programs, `Sum`
    /// for an interleaved SCC. See [`TempStrategy`].
    pub(crate) fn new(temp_strategy: TempStrategy) -> Self {
        FragmentMerger {
            temp_strategy,
            merged_literals: Vec::new(),
            merged_gf: Vec::new(),
            gf_block_index: HashMap::new(),
            merged_modules: Vec::new(),
            merged_views: Vec::new(),
            merged_temp_sizes: Vec::new(),
            merged_dim_lists: Vec::new(),
        }
    }

    /// Absorb one fragment's side-channels into the running merge state and
    /// return the five flat non-GF resource base offsets plus the per-slot GF
    /// remap this fragment's opcodes must be renumbered by. This is
    /// `absorb_non_gf` followed by `absorb_gf` (see those for the contract).
    /// The program-building methods call it once per fragment and renumber on
    /// the caller's behalf; `combine_scc_fragment` calls it directly, because
    /// it renumbers a member's per-element segments itself before interleaving
    /// them.
    pub(crate) fn absorb(
        &mut self,
        frag: &PerVarBytecodes,
    ) -> Result<(FragmentResourceOffsets, GfRemap), String> {
        let off = self.absorb_non_gf(frag)?;
        let gf_remap = self.absorb_gf(frag)?;
        Ok((off, gf_remap))
    }

    /// Absorb one fragment's flat (non-GF) side-channels -- literals,
    /// modules, views, temp sizes, dim lists -- into the running merge
    /// state and return the five flat resource base offsets.
    ///
    /// M2: the literal / module / view / dim-list offsets are the *pre-merge*
    /// lengths, and the fragment's entries are then appended, so this
    /// fragment's entries occupy exactly `[base, base + len)` and no earlier
    /// fragment's range can overlap it. Two things are rewritten on the way
    /// in, and both are M1 (a merged entry must still denote what the
    /// fragment's entry denoted): a `Temp`-based static view is shifted by
    /// this fragment's `temp_offset` so it still points at the same temp, and
    /// a dim-list is truncated to the 4 elements the `(u8, [u16; 4])` merged
    /// representation holds.
    ///
    /// The literal base is the length of the pool the CURRENT program has
    /// built so far (M8): the pool is per program, so it restarts at zero
    /// with each one. Every other flat table spans the module and keeps
    /// growing -- that half is `absorb_context`. The *temp* offset follows
    /// `temp_strategy` (#583, M5): `Sum` advances per fragment so each gets a
    /// disjoint range; `Recycle` places every fragment's temp `t` on slot `t`,
    /// max-merged in `absorb_context`. Graphical functions are handled
    /// separately by `absorb_gf` (content-de-duplicated, #582, M4).
    ///
    /// `Err` on M3: a merged table that outgrows its `u16` id type. The base
    /// offsets are checked here rather than only at `renumber_opcode` because
    /// a fragment may carry entries no opcode reads (over-collected dependency
    /// resources), and those still consume ids for the fragments after it.
    fn absorb_non_gf(&mut self, frag: &PerVarBytecodes) -> Result<FragmentResourceOffsets, String> {
        let lit_offset = resource_base(
            self.merged_literals.len(),
            frag.symbolic.literals.len(),
            "literal",
        )?;
        let ctx = self.absorb_context(frag)?;
        self.merged_literals
            .extend_from_slice(&frag.symbolic.literals);
        Ok(ctx.with_literals(lit_offset))
    }

    /// Absorb one fragment's SHARED CONTEXT side-channels -- module decls,
    /// static views, temp sizes, dim lists -- and return their base offsets.
    /// The literal pool is deliberately not touched: these tables span every
    /// program of the module (M8), while the pool belongs to one program.
    fn absorb_context(&mut self, frag: &PerVarBytecodes) -> Result<ContextResourceOffsets, String> {
        // Modules, views, and dim-lists are appended, so a fragment's base is
        // the table's length before it.
        let mod_offset = resource_base(
            self.merged_modules.len(),
            frag.module_decls.len(),
            "module declaration",
        )?;
        let view_offset = resource_base(
            self.merged_views.len(),
            frag.static_views.len(),
            "static view",
        )?;
        // #583: temps recycle (a module's programs) or sum (an interleaved SCC).
        //
        // `Recycle`: every fragment's temp `t` lands on slot `t` -- one identity
        //   pool shared by every program of the module, sized per slot to the
        //   largest user by the max-merge below.
        // `Sum`: advance by the running pool length so each fragment gets a
        //   disjoint range (interleaved segments need non-overlapping live
        //   ranges).
        let temp_offset = match self.temp_strategy {
            TempStrategy::Recycle => 0,
            TempStrategy::Sum => self.merged_temp_sizes.len() as u32,
        };
        let dl_offset = resource_base(
            self.merged_dim_lists.len(),
            frag.dim_lists.len(),
            "dimension list",
        )?;

        self.merged_modules.extend_from_slice(&frag.module_decls);
        self.merged_views.extend(frag.static_views.iter().map(|sv| {
            let base = match &sv.base {
                SymStaticViewBase::Temp(id) => SymStaticViewBase::Temp(*id + temp_offset),
                // Variable-backed bases -- `curr` and both snapshot regions --
                // name a variable, not a merged resource, so they carry across
                // untouched. Written out rather than caught by `_` so a future
                // base variant has to state which side of M1 it falls on.
                base @ (SymStaticViewBase::Var(_)
                | SymStaticViewBase::PrevVar(_)
                | SymStaticViewBase::InitialVar(_)) => base.clone(),
            };
            SymbolicStaticView { base, ..sv.clone() }
        }));
        self.merged_dim_lists
            .extend(frag.dim_lists.iter().map(|dl| {
                let n = dl.len().min(4) as u8;
                let mut arr = [0u16; 4];
                for (i, &v) in dl.iter().take(4).enumerate() {
                    arr[i] = v;
                }
                (n, arr)
            }));

        for (id, size) in &frag.temp_sizes {
            let new_id = *id + temp_offset;
            if new_id as usize >= self.merged_temp_sizes.len() {
                self.merged_temp_sizes.resize(new_id as usize + 1, 0);
            }
            self.merged_temp_sizes[new_id as usize] =
                self.merged_temp_sizes[new_id as usize].max(*size);
        }

        Ok(ContextResourceOffsets {
            mod_offset,
            view_offset,
            temp_offset,
            dl_offset,
        })
    }

    /// Content-de-duplicate one fragment's graphical-function *blocks* into
    /// the running `merged_gf` and return the per-slot local->global remap
    /// (#582).
    ///
    /// Each block (`gf_blocks_of_fragment`) is keyed by its bit-exact
    /// content; a block already present (from a prior fragment -- the common
    /// case: every consumer of a dependency arrayed GF re-extracts the same
    /// `Vec<Table>`) reuses its existing global start, otherwise the block
    /// is appended. The returned `GfRemap` shifts each block as a unit, so a
    /// `Lookup`/`LookupArray` `base_gf` -- whether the block start or an
    /// interior element reference -- maps to `block_new_base + (base_gf -
    /// block_old_base)`, preserving the `[base .. base + table_count]`
    /// array-lookup span.
    ///
    /// Returns `Err` if the *distinct* GF count exceeds
    /// `GraphicalFunctionId` capacity (`u8::MAX`) -- the genuine-capacity
    /// case the dedup cannot help; escalate, do not widen the ID width here.
    fn absorb_gf(&mut self, frag: &PerVarBytecodes) -> Result<GfRemap, String> {
        let gf_len = frag.graphical_functions.len();
        if gf_len == 0 {
            return Ok(GfRemap::new());
        }
        let mut gf_remap: GfRemap = smallvec::smallvec![0; gf_len];
        for (block_start, block_len) in gf_blocks_of_fragment(frag)? {
            let block = &frag.graphical_functions[block_start..block_start + block_len];
            let key = gf_block_key(block);
            let global_start = match self.gf_block_index.get(&key) {
                Some(&existing) => existing,
                None => {
                    let start = self.merged_gf.len();
                    self.merged_gf.extend_from_slice(block);
                    // The deduped (distinct) GF count must fit the
                    // GraphicalFunctionId capacity; if it does not, the
                    // dedup cannot help (these are genuinely-distinct
                    // tables) -- fail loud rather than wrap a `base_gf` to a
                    // wrong table.
                    if self.merged_gf.len() > u8::MAX as usize + 1 {
                        return Err(format!(
                            "distinct graphical function count {} exceeds \
                             GraphicalFunctionId capacity (u8::MAX = {})",
                            self.merged_gf.len(),
                            u8::MAX
                        ));
                    }
                    let start_u16 = start as u16;
                    self.gf_block_index.insert(key, start_u16);
                    start_u16
                }
            };
            // Shift the whole block by the same delta, so an interior /
            // nested `base_gf` lands at `global_start + (local - block_start)`.
            for k in 0..block_len {
                gf_remap[block_start + k] = (global_start as usize + k) as GraphicalFunctionId;
            }
        }
        Ok(gf_remap)
    }

    /// Renumber `frag` as a program of its own, its trailing `Ret` kept -- the
    /// initials shape, which `eval_initials` runs one at a time. The fragment's
    /// context resources join the module tables exactly as `concatenate`'s do
    /// (M8); its literal pool is the program's.
    pub(crate) fn standalone_program(
        &mut self,
        frag: &PerVarBytecodes,
    ) -> Result<SymbolicByteCode, String> {
        debug_assert!(
            self.merged_literals.is_empty(),
            "a program's literal pool starts empty"
        );
        let (off, gf_remap) = self.absorb(frag)?;
        let code = frag
            .symbolic
            .code
            .iter()
            .map(|op| {
                renumber_opcode(
                    op,
                    off.lit_offset,
                    &gf_remap,
                    off.mod_offset,
                    off.view_offset,
                    off.temp_offset,
                    off.dl_offset,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(self.finish_program(code))
    }

    /// Merge `fragments` into one program: each fragment's Ret-stripped opcodes
    /// appended as one contiguous run, in order, under a single terminal `Ret`
    /// when there is any code (M6), with one literal pool (M8). The contiguous
    /// runs are what make `TempStrategy::Recycle` sound for this shape (M5),
    /// and what make a prefix of the fragment list a prefix of the program --
    /// the boundary `assemble_module` counts for the run-invariant flow prefix.
    pub(crate) fn concatenate(
        &mut self,
        fragments: &[&PerVarBytecodes],
    ) -> Result<SymbolicByteCode, String> {
        debug_assert!(
            self.merged_literals.is_empty(),
            "a program's literal pool starts empty"
        );
        let mut code: Vec<SymbolicOpcode> = Vec::new();
        for frag in fragments {
            let (off, gf_remap) = self.absorb(frag)?;
            renumber_fragment_code(&frag.symbolic.code, &off, &gf_remap, &mut code)?;
        }
        if !code.is_empty() {
            code.push(SymbolicOpcode::Ret);
        }
        Ok(self.finish_program(code))
    }

    /// Pair a program's renumbered code with the literal pool built for it,
    /// leaving the pool empty for the next program (M8).
    fn finish_program(&mut self, code: Vec<SymbolicOpcode>) -> SymbolicByteCode {
        SymbolicByteCode {
            literals: std::mem::take(&mut self.merged_literals),
            code,
        }
    }

    /// Consume the merger and finalize the module's shared tables -- the ones
    /// every program built through it indexes (M8). `temp_offsets` is the
    /// prefix sum of the merged temp sizes, computed here and nowhere else.
    pub(crate) fn into_side_channels(self) -> ContextSideChannels {
        let mut temp_offsets = Vec::with_capacity(self.merged_temp_sizes.len());
        let mut offset = 0usize;
        for &size in &self.merged_temp_sizes {
            temp_offsets.push(offset);
            offset += size;
        }

        ContextSideChannels {
            graphical_functions: self.merged_gf,
            module_decls: self.merged_modules,
            static_views: self.merged_views,
            temp_offsets,
            temp_total_size: offset,
            dim_lists: self.merged_dim_lists,
        }
    }

    /// Consume the merger and finalize into a `PerVarBytecodes` (the shape
    /// `combine_scc_fragment` returns -- a combined fragment is itself a
    /// fragment, absorbed by the module's merger at assembly). `code` is
    /// the already-renumbered opcode stream of the interleaved segments;
    /// a single trailing `Ret` is appended iff `code` is non-empty.
    ///
    /// `temp_sizes`/`dim_lists` are converted back to the `PerVarBytecodes`
    /// representations: `merged_temp_sizes[i]` becomes `(i, size)` for
    /// every slot (including zero-size ones, so the combined fragment's temp
    /// count is preserved), and each truncated dim-list `(n, arr)` becomes
    /// `arr[..n].to_vec()`. The truncation is idempotent on the <=4-element
    /// dimension tuples dim-lists hold, so the module merger's later pass is
    /// unaffected.
    pub(crate) fn into_per_var_bytecodes(self, mut code: Vec<SymbolicOpcode>) -> PerVarBytecodes {
        if !code.is_empty() {
            code.push(SymbolicOpcode::Ret);
        }

        let temp_sizes: Vec<(u32, usize)> = self
            .merged_temp_sizes
            .iter()
            .enumerate()
            .map(|(i, &size)| (i as u32, size))
            .collect();
        let dim_lists: Vec<Vec<u16>> = self
            .merged_dim_lists
            .iter()
            .map(|(n, arr)| arr[..(*n as usize)].to_vec())
            .collect();

        PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: self.merged_literals,
                code,
            },
            graphical_functions: self.merged_gf,
            module_decls: self.merged_modules,
            static_views: self.merged_views,
            temp_sizes,
            dim_lists,
        }
    }
}

/// Renumber a single fragment's (Ret-stripped) opcodes by the offsets
/// returned from `FragmentMerger::absorb`. Shared by both consumers so the
/// trailing-`Ret` strip and the renumber call site are defined once.
pub(crate) fn renumber_fragment_code(
    code: &[SymbolicOpcode],
    off: &FragmentResourceOffsets,
    gf_remap: &[GraphicalFunctionId],
    out: &mut Vec<SymbolicOpcode>,
) -> Result<(), String> {
    // Strip a trailing Ret -- the merger appends a single Ret at the end.
    let end = if code.last() == Some(&SymbolicOpcode::Ret) {
        code.len() - 1
    } else {
        code.len()
    };
    for op in &code[..end] {
        out.push(renumber_opcode(
            op,
            off.lit_offset,
            gf_remap,
            off.mod_offset,
            off.view_offset,
            off.temp_offset,
            off.dl_offset,
        )?);
    }
    Ok(())
}

/// The tables every program of an assembled module shares (M8), finished once
/// by [`FragmentMerger::into_side_channels`]. The bytecode and literal pool of
/// each program are handed back by the method that builds it.
pub(crate) struct ContextSideChannels {
    pub graphical_functions: Vec<Vec<(f64, f64)>>,
    pub module_decls: Vec<SymbolicModuleDecl>,
    pub static_views: Vec<SymbolicStaticView>,
    pub temp_offsets: Vec<usize>,
    pub temp_total_size: usize,
    pub dim_lists: Vec<(u8, [u16; 4])>,
}

/// A program and the tables it indexes: the shape a unit test that merges one
/// fragment list in isolation reads. Production never pairs the two -- a
/// module's programs share one table set that `assemble_module` takes once,
/// after the last program.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ConcatenatedBytecodes {
    pub bytecode: SymbolicByteCode,
    pub graphical_functions: Vec<Vec<(f64, f64)>>,
    pub module_decls: Vec<SymbolicModuleDecl>,
    pub static_views: Vec<SymbolicStaticView>,
    pub temp_offsets: Vec<usize>,
    pub temp_total_size: usize,
    pub dim_lists: Vec<(u8, [u16; 4])>,
}

/// Merge `fragments` as one program on a fresh `Recycle` merger and return it
/// with the tables it indexes: the focused-unit-test surface for the merge and
/// GF-dedup behavior.
#[cfg(test)]
pub(crate) fn concatenate_fragments(
    fragments: &[&PerVarBytecodes],
) -> Result<ConcatenatedBytecodes, String> {
    let mut merger = FragmentMerger::new(TempStrategy::Recycle);
    let bytecode = merger.concatenate(fragments)?;
    let side = merger.into_side_channels();
    Ok(ConcatenatedBytecodes {
        bytecode,
        graphical_functions: side.graphical_functions,
        module_decls: side.module_decls,
        static_views: side.static_views,
        temp_offsets: side.temp_offsets,
        temp_total_size: side.temp_total_size,
        dim_lists: side.dim_lists,
    })
}

fn checked_add_u8(base: u8, off: u8, label: &str) -> Result<u8, String> {
    base.checked_add(off).ok_or_else(|| {
        format!(
            "{} overflow: {} + {} exceeds u8::MAX ({})",
            label,
            base,
            off,
            u8::MAX
        )
    })
}

/// Remap a `Lookup`/`LookupArray` `base_gf` through a fragment's per-slot
/// GF remap (#582). The whole-list shift in `FragmentMerger::absorb_gf`
/// guarantees `gf_remap[base + k] == gf_remap[base] + k`, so a single
/// lookup of `base_gf` suffices and the `table_count` span stays valid. An
/// out-of-range `base_gf` is a corrupt fragment (loud-safe `Err`, never a
/// silent wrong-table read).
fn remap_gf(
    base_gf: GraphicalFunctionId,
    gf_remap: &[GraphicalFunctionId],
) -> Result<GraphicalFunctionId, String> {
    gf_remap.get(base_gf as usize).copied().ok_or_else(|| {
        format!(
            "GF base {} out of range for fragment GF remap of length {}",
            base_gf,
            gf_remap.len()
        )
    })
}

/// Renumber resource IDs within a single opcode.
///
/// Flat resources (`LiteralId`, `ModuleId`, `ViewId`, `TempId`,
/// `DimListId`) are offset by the fragment's flat base; `GraphicalFunctionId`
/// is *content-de-duplicated* (#582), so a `Lookup`/`LookupArray` `base_gf`
/// is translated through `gf_remap` (the fragment's per-slot local->global
/// map from `FragmentMerger::absorb_gf`) rather than a flat add.
///
/// Every offsetting add is checked (M3): a wrapped id is a well-formed
/// program that names a different resource, so the failure mode of getting
/// this wrong is wrong numbers with no diagnostic. `Err` on a temp id past
/// `TempId` (= `u8`), on any `u16` id past its type, or on a `base_gf` out of
/// range for `gf_remap` (a corrupt fragment).
///
/// There is no separate `temp_off > u8::MAX` precheck (#583): a module's
/// programs recycle temps into one identity pool whose `temp_off` is 0, and
/// `combine_scc_fragment` sums into a per-SCC range bounded by the members'
/// (small) temp counts. A genuine
/// per-opcode overflow -- a single variable bearing more than 255 temps, or
/// an SCC summing past 255 -- is still caught loud by `checked_add_u8`,
/// which adds the actual `temp_id` to the offset (the precheck only saw the
/// offset, so it could not have been the real bound anyway).
///
/// The `u16` adds are belt-and-braces alongside `absorb_non_gf`'s capacity
/// check: that one bounds the merged TABLE, this one bounds the id an
/// individual opcode carries. Every production base comes from the merger; the
/// add stays checked so a caller with a hand-computed base could not wrap one.
pub(crate) fn renumber_opcode(
    op: &SymbolicOpcode,
    lit_off: u16,
    gf_remap: &[GraphicalFunctionId],
    mod_off: u16,
    view_off: u16,
    temp_off: u32,
    dl_off: u16,
) -> Result<SymbolicOpcode, String> {
    // A `temp_off` that itself exceeds u8 can only arise from the `Sum` path
    // (interleaved SCC) summing past 255 temps; `checked_add_u8` below
    // surfaces it loud when the first temp opcode is renumbered. The
    // recycle path's `temp_off` is always 0.
    let temp_off_u8 = u8::try_from(temp_off).map_err(|_| {
        format!(
            "temp offset {} exceeds TempId capacity (u8::MAX = {})",
            temp_off,
            u8::MAX
        )
    })?;
    Ok(match op {
        SymbolicOpcode::LoadConstant { id } => SymbolicOpcode::LoadConstant {
            id: checked_add_u16(*id, lit_off, "LiteralId")?,
        },
        SymbolicOpcode::AssignConstCurr { var, literal_id } => SymbolicOpcode::AssignConstCurr {
            var: var.clone(),
            literal_id: checked_add_u16(*literal_id, lit_off, "LiteralId")?,
        },
        SymbolicOpcode::Lookup {
            base_gf,
            table_count,
            mode,
        } => SymbolicOpcode::Lookup {
            base_gf: remap_gf(*base_gf, gf_remap)?,
            table_count: *table_count,
            mode: *mode,
        },
        SymbolicOpcode::LookupDirect {
            base_gf,
            table_count,
            elem,
            mode,
        } => SymbolicOpcode::LookupDirect {
            base_gf: remap_gf(*base_gf, gf_remap)?,
            table_count: *table_count,
            elem: *elem,
            mode: *mode,
        },
        SymbolicOpcode::EvalModule { id, n_inputs } => SymbolicOpcode::EvalModule {
            id: checked_add_u16(*id, mod_off, "ModuleId")?,
            n_inputs: *n_inputs,
        },
        SymbolicOpcode::PushStaticView { view_id } => SymbolicOpcode::PushStaticView {
            view_id: checked_add_u16(*view_id, view_off, "ViewId")?,
        },
        SymbolicOpcode::PushVarViewDirect { var, dim_list_id } => {
            SymbolicOpcode::PushVarViewDirect {
                var: var.clone(),
                dim_list_id: checked_add_u16(*dim_list_id, dl_off, "DimListId")?,
            }
        }
        SymbolicOpcode::LoadTempConst { temp_id, index } => SymbolicOpcode::LoadTempConst {
            temp_id: checked_add_u8(*temp_id, temp_off_u8, "TempId")?,
            index: *index,
        },
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => SymbolicOpcode::BeginIter {
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
            has_write_temp: *has_write_temp,
        },
        SymbolicOpcode::VectorElmMap {
            write_temp_id,
            full_source_len,
        } => SymbolicOpcode::VectorElmMap {
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
            // full_source_len is the source variable's absolute element count,
            // not a temp id -- it is not renumbered on fragment concatenation.
            full_source_len: *full_source_len,
        },
        SymbolicOpcode::VectorSortOrder { write_temp_id } => SymbolicOpcode::VectorSortOrder {
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
        },
        SymbolicOpcode::Rank { write_temp_id } => SymbolicOpcode::Rank {
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
        },
        // LookupArray carries BOTH a GF-table base (like `Lookup`) and a
        // result temp id (like the other vector ops). The GF base is
        // content-remapped (the `[base .. base + table_count]` block stays
        // contiguous after dedup); the temp id is flat-offset.
        SymbolicOpcode::LookupArray {
            base_gf,
            table_count,
            mode,
            write_temp_id,
        } => SymbolicOpcode::LookupArray {
            base_gf: remap_gf(*base_gf, gf_remap)?,
            table_count: *table_count,
            mode: *mode,
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
        },
        SymbolicOpcode::AllocateAvailable { write_temp_id } => SymbolicOpcode::AllocateAvailable {
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
        },
        SymbolicOpcode::AllocateByPriority { write_temp_id } => {
            SymbolicOpcode::AllocateByPriority {
                write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
            }
        }
        // All other opcodes have no resource IDs to renumber
        other => other.clone(),
    })
}

#[cfg(test)]
#[path = "symbolic_builder_tests.rs"]
mod builder_tests;

#[cfg(test)]
#[path = "symbolic_merge_proptest.rs"]
mod merge_proptest;

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Op2;

    /// A symbolic reference to `name`'s element `elem`.
    fn sref(name: &str, elem: usize) -> SymVarRef {
        SymVarRef::new(Ident::new(name), elem)
    }

    /// Every opcode variant that carries a temp id directly. The view-backed
    /// channel is separate below because its id lives in `static_views` rather
    /// than in the opcode. Keeping this table next to the exhaustive production
    /// match makes it clear which temp-bearing variants the behavioral rows
    /// exercise.
    fn direct_temp_touches() -> Vec<SymbolicOpcode> {
        vec![
            SymbolicOpcode::LoadTempConst {
                temp_id: 0,
                index: 0,
            },
            SymbolicOpcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: true,
            },
            SymbolicOpcode::VectorElmMap {
                write_temp_id: 0,
                full_source_len: 2,
            },
            SymbolicOpcode::VectorSortOrder { write_temp_id: 0 },
            SymbolicOpcode::Rank { write_temp_id: 0 },
            SymbolicOpcode::LookupArray {
                base_gf: 0,
                table_count: 1,
                mode: LookupMode::Interpolate,
                write_temp_id: 0,
            },
            SymbolicOpcode::AllocateAvailable { write_temp_id: 0 },
            SymbolicOpcode::AllocateByPriority { write_temp_id: 0 },
        ]
    }

    #[test]
    fn every_direct_temp_touch_crossing_an_element_segment_is_rejected() {
        for touch in direct_temp_touches() {
            let name = format!("{touch:?}");
            let code = vec![
                touch,
                SymbolicOpcode::AssignCurr { var: sref("a", 0) },
                SymbolicOpcode::LoadTempConst {
                    temp_id: 0,
                    index: 0,
                },
                SymbolicOpcode::AssignCurr { var: sref("a", 1) },
            ];
            assert!(
                validate_segment_local_temps("a", &code, &[]).is_err(),
                "{name} must be classified as a temp touch"
            );
        }
    }

    #[test]
    fn view_backed_temp_touches_obey_the_same_segment_boundary() {
        let views = [SymbolicStaticView {
            base: SymStaticViewBase::Temp(0),
            dims: SmallVec::from_slice(&[2]),
            strides: SmallVec::from_slice(&[1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0]),
        }];
        let code = vec![
            SymbolicOpcode::VectorSortOrder { write_temp_id: 0 },
            SymbolicOpcode::AssignCurr { var: sref("a", 0) },
            SymbolicOpcode::PushStaticView { view_id: 0 },
            SymbolicOpcode::AssignCurr { var: sref("a", 1) },
        ];
        assert!(validate_segment_local_temps("a", &code, &views).is_err());

        let malformed = vec![
            SymbolicOpcode::PushStaticView { view_id: 1 },
            SymbolicOpcode::AssignCurr { var: sref("a", 0) },
        ];
        assert!(
            validate_segment_local_temps("a", &malformed, &views).is_err(),
            "an invalid view id must fail loud rather than disappear from the temp scan"
        );
    }

    #[test]
    fn element_local_temp_lifetimes_preserve_dominance() {
        for touch in direct_temp_touches() {
            let name = format!("{touch:?}");
            let code = vec![
                touch,
                SymbolicOpcode::LoadTempConst {
                    temp_id: 0,
                    index: 0,
                },
                SymbolicOpcode::AssignCurr { var: sref("a", 0) },
                SymbolicOpcode::AssignCurr { var: sref("a", 1) },
            ];
            assert!(
                validate_segment_local_temps("a", &code, &[]).is_ok(),
                "{name} and its consumer remain ordered inside segment zero"
            );
        }

        // Cleanup after the final write belongs to that final segment, just as
        // `segment_member_by_element` emits it.
        let trailing = vec![
            SymbolicOpcode::AssignCurr { var: sref("a", 0) },
            SymbolicOpcode::VectorSortOrder { write_temp_id: 0 },
            SymbolicOpcode::AssignCurr { var: sref("a", 1) },
            SymbolicOpcode::LoadTempConst {
                temp_id: 0,
                index: 0,
            },
        ];
        assert!(validate_segment_local_temps("a", &trailing, &[]).is_ok());

        // A read-only iteration carries an irrelevant placeholder temp id.
        let read_only = vec![
            SymbolicOpcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: false,
            },
            SymbolicOpcode::AssignCurr { var: sref("a", 0) },
            SymbolicOpcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: false,
            },
            SymbolicOpcode::AssignCurr { var: sref("a", 1) },
        ];
        assert!(validate_segment_local_temps("a", &read_only, &[]).is_ok());
    }

    fn simple_layout() -> VariableLayout {
        let mut entries = HashMap::new();
        // Root model: implicit vars at 0-3, then user vars alphabetically
        entries.insert("time".to_string(), LayoutEntry { offset: 0, size: 1 });
        entries.insert("dt".to_string(), LayoutEntry { offset: 1, size: 1 });
        entries.insert(
            "initial_time".to_string(),
            LayoutEntry { offset: 2, size: 1 },
        );
        entries.insert("final_time".to_string(), LayoutEntry { offset: 3, size: 1 });
        entries.insert("births".to_string(), LayoutEntry { offset: 4, size: 1 });
        entries.insert("population".to_string(), LayoutEntry { offset: 5, size: 1 });
        VariableLayout::new(entries, 6)
    }

    /// The two reference-bearing opcode families every model uses.
    #[test]
    fn test_resolve_load_var_and_assign_curr() {
        assert_resolves(
            SymbolicOpcode::LoadVar {
                var: sref("population", 0),
            },
            Opcode::LoadVar { off: 5 },
        );
        assert_resolves(
            SymbolicOpcode::AssignCurr {
                var: sref("births", 0),
            },
            Opcode::AssignCurr { off: 4 },
        );
    }

    /// Opcodes that carry no variable reference pass through resolution with
    /// their operands untouched.
    #[test]
    fn test_resolve_passthrough_opcodes() {
        assert_resolves(
            SymbolicOpcode::LoadGlobalVar { off: 1 },
            Opcode::LoadGlobalVar { off: 1 },
        );
        assert_resolves(
            SymbolicOpcode::Op2 { op: Op2::Add },
            Opcode::Op2 { op: Op2::Add },
        );
        assert_resolves(SymbolicOpcode::Ret, Opcode::Ret);
    }

    #[test]
    fn test_resolve_var_ref() {
        let layout = simple_layout();

        let var = sref("population", 0);
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 5);

        let var = sref("births", 0);
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 4);
    }

    #[test]
    fn test_resolve_var_ref_array_element() {
        let mut entries = HashMap::new();
        entries.insert(
            "arr".to_string(),
            LayoutEntry {
                offset: 10,
                size: 3,
            },
        );
        let layout = VariableLayout::new(entries, 13);

        let var = sref("arr", 2);
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 12);
    }

    #[test]
    fn test_resolve_var_ref_element_offset_out_of_bounds() {
        let mut entries = HashMap::new();
        entries.insert("arr".to_string(), LayoutEntry { offset: 4, size: 3 });
        let layout = VariableLayout::new(entries, 7);

        // element_offset == size (out of bounds)
        let var = sref("arr", 3);
        assert!(
            resolve_var_ref(&var, &layout).is_err(),
            "element_offset >= size should fail"
        );

        // element_offset well beyond size
        let var = sref("arr", 100);
        assert!(resolve_var_ref(&var, &layout).is_err());

        // element_offset at max valid index should succeed
        let var = sref("arr", 2);
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 6);
    }

    #[test]
    fn test_resolve_missing_variable() {
        let layout = simple_layout();
        let var = sref("nonexistent", 0);
        assert!(resolve_var_ref(&var, &layout).is_err());
    }

    #[test]
    fn test_resolve_var_ref_rejects_offset_beyond_u16() {
        // VariableOffset is u16. A layout entry beyond its range (which a very
        // large LTM-instrumented model can produce -- C-LEARN in discovery mode
        // needs 171k slots) must be a loud resolution error, NOT a silent `as`
        // truncation: a wrapped offset writes into the low slots, overwriting
        // time/dt and corrupting every result.
        let mut entries = HashMap::new();
        entries.insert(
            "huge".to_string(),
            LayoutEntry {
                offset: (VariableOffset::MAX as usize) + 1,
                size: 1,
            },
        );
        let layout = VariableLayout::new(entries, (VariableOffset::MAX as usize) + 2);

        let var = sref("huge", 0);
        let err = resolve_var_ref(&var, &layout).expect_err("offset beyond u16 must error");
        assert!(
            err.contains("huge") && err.contains("65535"),
            "error should name the variable and the limit, got: {err}"
        );

        // An array whose BASE fits but whose element offset crosses the limit
        // must also error.
        let mut entries = HashMap::new();
        entries.insert(
            "arr".to_string(),
            LayoutEntry {
                offset: (VariableOffset::MAX as usize) - 1,
                size: 4,
            },
        );
        let layout = VariableLayout::new(entries, (VariableOffset::MAX as usize) + 4);
        let var = sref("arr", 3);
        assert!(
            resolve_var_ref(&var, &layout).is_err(),
            "element offset crossing the u16 limit must error"
        );
    }

    #[test]
    fn test_resolve_var_ref_accepts_offset_at_u16_max() {
        // Boundary: the largest addressable offset (u16::MAX itself) is valid.
        let mut entries = HashMap::new();
        entries.insert(
            "edge".to_string(),
            LayoutEntry {
                offset: VariableOffset::MAX as usize,
                size: 1,
            },
        );
        let layout = VariableLayout::new(entries, (VariableOffset::MAX as usize) + 1);
        let var = sref("edge", 0);
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), VariableOffset::MAX);
    }

    #[test]
    fn test_check_layout_addressable() {
        // Pure functional-core check used by assembly to fail fast (before
        // compiling thousands of fragments) when a module's layout cannot be
        // addressed by the bytecode's u16 offsets.
        let limit = (VariableOffset::MAX as usize) + 1; // 65,536 slots = offsets 0..=65,535

        // At the limit: every offset fits.
        assert!(check_layout_addressable(limit, "main").is_ok());
        assert!(check_layout_addressable(0, "main").is_ok());
        assert!(check_layout_addressable(1, "main").is_ok());

        // One past the limit: slot 65,536 exists but cannot be addressed.
        let err = check_layout_addressable(limit + 1, "main").expect_err("must reject");
        assert!(
            err.contains("main") && err.contains("65536") && err.contains("65,536")
                || err.contains("main"),
            "error should name the model, got: {err}"
        );

        // C-LEARN-with-LTM scale.
        assert!(check_layout_addressable(171_597, "main").is_err());
    }

    /// A whole bytecode stream resolves opcode-for-opcode, literals intact.
    #[test]
    fn test_bytecode_resolution() {
        let layout = simple_layout();

        let sym = SymbolicByteCode {
            literals: vec![1.0, 0.5],
            code: vec![
                SymbolicOpcode::LoadVar {
                    var: sref("population", 0),
                },
                SymbolicOpcode::LoadConstant { id: 1 },
                SymbolicOpcode::Op2 { op: Op2::Mul },
                SymbolicOpcode::AssignCurr {
                    var: sref("births", 0),
                },
                SymbolicOpcode::Ret,
            ],
        };

        let resolved = resolve_bytecode(&sym, &layout).unwrap();
        assert_eq!(resolved.literals, vec![1.0, 0.5]);
        assert_eq!(
            resolved.code,
            vec![
                Opcode::LoadVar { off: 5 },
                Opcode::LoadConstant { id: 1 },
                Opcode::Op2 { op: Op2::Mul },
                Opcode::AssignCurr { off: 4 },
                Opcode::Ret,
            ]
        );
    }

    /// The three superinstructions the peephole and the emit-time stock fusion
    /// produce all carry a reference and all resolve.
    #[test]
    fn test_bytecode_resolution_superinstructions() {
        let layout = simple_layout();

        // The two `BinOpAssign*` forms pop their operands, so the stream has
        // to be stack-balanced: `resolve_bytecode` validates depth (that is
        // where the VM's unchecked stack access is proven sound).
        let sym = SymbolicByteCode {
            literals: vec![100.0, 0.0],
            code: vec![
                SymbolicOpcode::AssignConstCurr {
                    var: sref("population", 0),
                    literal_id: 0,
                },
                SymbolicOpcode::LoadConstant { id: 0 },
                SymbolicOpcode::LoadConstant { id: 1 },
                SymbolicOpcode::BinOpAssignCurr {
                    op: Op2::Add,
                    var: sref("births", 0),
                },
                SymbolicOpcode::LoadConstant { id: 0 },
                SymbolicOpcode::LoadConstant { id: 1 },
                SymbolicOpcode::BinOpAssignNext {
                    op: Op2::Mul,
                    var: sref("population", 0),
                },
                SymbolicOpcode::Ret,
            ],
        };

        let resolved = resolve_bytecode(&sym, &layout).unwrap();
        assert_eq!(
            resolved.code,
            vec![
                Opcode::AssignConstCurr {
                    off: 5,
                    literal_id: 0,
                },
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Add,
                    off: 4,
                },
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                Opcode::BinOpAssignNext {
                    op: Op2::Mul,
                    off: 5,
                },
                Opcode::Ret,
            ]
        );
    }

    /// The implicit globals keep their fixed absolute slots: they are read by
    /// `LoadGlobalVar`, which never goes through a layout at all.
    #[test]
    fn test_bytecode_resolution_global_vars() {
        let layout = simple_layout();

        let sym = SymbolicByteCode {
            literals: vec![],
            code: vec![
                SymbolicOpcode::LoadGlobalVar { off: 0 },
                SymbolicOpcode::LoadGlobalVar { off: 1 },
                SymbolicOpcode::Op2 { op: Op2::Add },
                SymbolicOpcode::AssignCurr {
                    var: sref("births", 0),
                },
                SymbolicOpcode::Ret,
            ],
        };

        let resolved = resolve_bytecode(&sym, &layout).unwrap();
        assert_eq!(
            resolved.code,
            vec![
                Opcode::LoadGlobalVar { off: 0 },
                Opcode::LoadGlobalVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 4 },
                Opcode::Ret,
            ]
        );
    }

    /// A bytecode stream deeper than the VM's fixed arithmetic stack is a
    /// compile error, not an abort: `resolve_bytecode` is the single place
    /// concrete bytecode is born, so it is where the VM's unchecked stack
    /// access is proven sound.
    #[test]
    fn test_resolution_rejects_stack_overflow() {
        let layout = simple_layout();

        let mut code: Vec<SymbolicOpcode> =
            vec![SymbolicOpcode::LoadConstant { id: 0 }; STACK_CAPACITY];
        code.push(SymbolicOpcode::Ret);
        let sym = SymbolicByteCode {
            literals: vec![1.0],
            code,
        };

        let err = resolve_bytecode(&sym, &layout).unwrap_err();
        assert!(
            err.contains("exceeding VM capacity"),
            "expected a stack-capacity error, got: {err}"
        );
    }

    #[test]
    fn test_static_view_resolution_var() {
        let layout = simple_layout();

        let sym = SymbolicStaticView {
            base: SymStaticViewBase::Var(sref("population", 0)),
            dims: SmallVec::from_slice(&[3]),
            strides: SmallVec::from_slice(&[1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0]),
        };

        let resolved = resolve_static_view(&sym, &layout).unwrap();
        assert_eq!(resolved.base_off, 5);
        assert_eq!(resolved.storage, ViewStorage::Curr);
        assert_eq!(resolved.dims, sym.dims);
        assert_eq!(resolved.offset, 0);
    }

    #[test]
    fn test_static_view_resolution_temp() {
        let layout = simple_layout();

        let sym = SymbolicStaticView {
            base: SymStaticViewBase::Temp(7),
            dims: SmallVec::from_slice(&[2, 3]),
            strides: SmallVec::from_slice(&[3, 1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0, 1]),
        };

        let resolved = resolve_static_view(&sym, &layout).unwrap();
        assert_eq!(resolved.base_off, 7);
        assert_eq!(resolved.storage, ViewStorage::Temp);
    }

    #[test]
    fn test_module_decl_resolution() {
        let layout = simple_layout();

        let sym = SymbolicModuleDecl {
            model_name: Ident::new("sub_model"),
            input_set: BTreeSet::new(),
            var: sref("births", 0),
        };

        let resolved = resolve_module_decl(&sym, &layout).unwrap();
        assert_eq!(resolved.off, 4);
        assert_eq!(resolved.model_name, Ident::new("sub_model"));
    }

    /// The property the whole symbolic layer exists for: one fragment, two
    /// layouts. Adding a variable ahead of `population` moves its slot, and the
    /// SAME symbolic bytecode resolves to the new slot with no recompilation.
    #[test]
    fn test_layout_independence() {
        let sym = SymbolicByteCode {
            literals: vec![0.1],
            code: vec![
                SymbolicOpcode::LoadVar {
                    var: sref("population", 0),
                },
                SymbolicOpcode::LoadConstant { id: 0 },
                SymbolicOpcode::Op2 { op: Op2::Mul },
                SymbolicOpcode::AssignCurr {
                    var: sref("births", 0),
                },
                SymbolicOpcode::Ret,
            ],
        };

        // layout1: births=4, population=5
        let resolved1 = resolve_bytecode(&sym, &simple_layout()).unwrap();
        assert_eq!(resolved1.code[0], Opcode::LoadVar { off: 5 });
        assert_eq!(resolved1.code[3], Opcode::AssignCurr { off: 4 });

        // layout2 inserts `growth_rate` alphabetically between births and
        // population, so population moves to 6 and births stays at 4.
        let mut entries2 = HashMap::new();
        entries2.insert("time".to_string(), LayoutEntry { offset: 0, size: 1 });
        entries2.insert("dt".to_string(), LayoutEntry { offset: 1, size: 1 });
        entries2.insert(
            "initial_time".to_string(),
            LayoutEntry { offset: 2, size: 1 },
        );
        entries2.insert("final_time".to_string(), LayoutEntry { offset: 3, size: 1 });
        entries2.insert("births".to_string(), LayoutEntry { offset: 4, size: 1 });
        entries2.insert(
            "growth_rate".to_string(),
            LayoutEntry { offset: 5, size: 1 },
        );
        entries2.insert("population".to_string(), LayoutEntry { offset: 6, size: 1 });
        let layout2 = VariableLayout::new(entries2, 7);

        let resolved2 = resolve_bytecode(&sym, &layout2).unwrap();
        assert_eq!(resolved2.code[0], Opcode::LoadVar { off: 6 });
        assert_eq!(resolved2.code[3], Opcode::AssignCurr { off: 4 });
    }

    #[test]
    fn test_extract_assign_curr_offsets() {
        let bc = ByteCode {
            literals: vec![1.0, 2.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 7 },
                Opcode::LoadConstant { id: 1 },
                Opcode::AssignCurr { off: 5 },
                Opcode::AssignConstCurr {
                    off: 6,
                    literal_id: 0,
                },
                Opcode::Ret,
            ],
        };

        let offsets = extract_assign_curr_offsets(&bc);
        assert_eq!(offsets, vec![5, 6, 7]);
    }

    /// Assert that `sym` resolves against `simple_layout()` to `expected`.
    ///
    /// This is the shape every opcode-family test takes now that resolution is
    /// the only direction of travel: build the symbolic opcode the compiler
    /// emits, resolve it, and pin the concrete opcode the VM will execute.
    #[track_caller]
    fn assert_resolves(sym: SymbolicOpcode, expected: Opcode) {
        let layout = simple_layout();
        let resolved = resolve_opcode(&sym, &layout)
            .unwrap_or_else(|e| panic!("resolve failed for {sym:?}: {e}"));
        assert!(
            resolved == expected,
            "resolve produced the wrong opcode for {sym:?}"
        );
    }

    // ====================================================================
    // Integration tests: compile real models and roundtrip through symbolic
    // ====================================================================

    use crate::testutils::{x_aux, x_flow, x_model, x_module, x_project, x_stock};

    fn default_sim_specs() -> crate::datamodel::SimSpecs {
        crate::datamodel::SimSpecs {
            start: 0.0,
            stop: 12.0,
            dt: crate::datamodel::Dt::Dt(1.0),
            save_step: None,
            sim_method: Default::default(),
            time_units: None,
        }
    }

    /// Compile a real model through the production incremental path and check
    /// that every address in the result is one the model's layout actually
    /// assigns.
    ///
    /// This replaced a `symbolize(compiled) -> resolve -> compare` roundtrip,
    /// which tested that two functions inverted each other; one of them no
    /// longer exists, because production never travels concrete-to-symbolic.
    /// What is worth asserting now is the forward direction end to end: the
    /// resolved module is addressed against the same layout the assembler
    /// used, each `CompiledInitial` carries exactly the write targets its own
    /// bytecode has, and each module declaration points at a variable's base.
    fn compile_and_check_resolution(dm_project: &crate::datamodel::Project, model_name: &str) {
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, dm_project, None);
        let sim = crate::db::compile_project_incremental(&db, sync.project, model_name)
            .expect("incremental compile should succeed");

        let compiled = &sim.modules[&sim.root];

        let source_model = sync.models[model_name].source_model;
        // The root module is assembled against the root-shifted layout
        // (implicit globals at fixed slots 0..3, body at +IMPLICIT_VAR_COUNT).
        let layout = crate::db::compute_layout(&db, source_model, sync.project).root_shifted();

        assert_eq!(compiled.n_slots, layout.n_slots);

        // Every slot a layout entry owns, so an emitted offset can be checked
        // for being a real address rather than a stray integer.
        let mut owned: Vec<bool> = vec![false; layout.n_slots];
        for entry in layout.entries.values() {
            owned[entry.offset..entry.offset + entry.size].fill(true);
        }
        let check = |bc: &ByteCode, what: &str| {
            for op in &bc.code {
                if let Some(off) = referenced_offset(op) {
                    assert!(
                        (off as usize) < layout.n_slots && owned[off as usize],
                        "{what}: opcode references slot {off}, which no layout entry owns"
                    );
                }
            }
        };

        for init in compiled.compiled_initials.iter() {
            check(&init.bytecode, "initials");
            assert_eq!(
                init.offsets,
                extract_assign_curr_offsets(&init.bytecode),
                "initial `{}` carries offsets its bytecode does not write",
                init.ident
            );
        }
        check(&compiled.compiled_flows, "flows");
        check(&compiled.compiled_stocks, "stocks");

        for md in compiled.context.modules.iter() {
            assert!(
                md.off < layout.n_slots && owned[md.off],
                "module declaration for '{}' points at slot {}, which no layout entry owns",
                md.model_name,
                md.off
            );
        }
        for sv in compiled.context.static_views.iter() {
            // The three chunk-shaped regions are all `n_slots` wide and share
            // `curr`'s numbering, so the owned-slot check applies to each; only
            // a temp base indexes something else.
            let addresses_a_slot = match sv.storage {
                ViewStorage::Curr | ViewStorage::Prev | ViewStorage::Initial => true,
                ViewStorage::Temp => false,
            };
            if addresses_a_slot {
                assert!(
                    (sv.base_off as usize) < layout.n_slots && owned[sv.base_off as usize],
                    "static view base {} is not an owned slot",
                    sv.base_off
                );
            }
        }
    }

    /// The model slot an opcode reads or writes, if it has one. Mirrors the
    /// nine `SymVarRef`-carrying symbolic opcode families.
    fn referenced_offset(op: &Opcode) -> Option<VariableOffset> {
        match op {
            Opcode::LoadVar { off }
            | Opcode::LoadPrev { off }
            | Opcode::LoadInitial { off }
            | Opcode::LoadSubscript { off }
            | Opcode::AssignCurr { off }
            | Opcode::AssignConstCurr { off, .. }
            | Opcode::BinOpAssignCurr { off, .. }
            | Opcode::BinOpAssignNext { off, .. } => Some(*off),
            Opcode::PushVarViewDirect { base_off, .. } => Some(*base_off),
            _ => None,
        }
    }

    #[test]
    fn test_roundtrip_sir_model() {
        let dm_project = x_project(
            default_sim_specs(),
            &[x_model(
                "main",
                vec![
                    x_stock("susceptible", "999", &[], &["succumbing"], None),
                    x_flow("succumbing", "susceptible * infectious * 0.003", None),
                    x_stock("infectious", "1", &["succumbing"], &["recovering"], None),
                    x_flow("recovering", "infectious / 5", None),
                    x_stock("recovered", "0", &["recovering"], &[], None),
                ],
            )],
        );
        compile_and_check_resolution(&dm_project, "main");
    }

    #[test]
    fn test_roundtrip_simple_aux_chain() {
        let dm_project = x_project(
            default_sim_specs(),
            &[x_model(
                "main",
                vec![
                    x_aux("a", "1", None),
                    x_aux("b", "a * 2", None),
                    x_aux("c", "a + b", None),
                ],
            )],
        );
        compile_and_check_resolution(&dm_project, "main");
    }

    #[test]
    fn test_roundtrip_stock_with_lookup() {
        let dm_project = x_project(
            default_sim_specs(),
            &[x_model(
                "main",
                vec![
                    x_stock("population", "100", &["births"], &[], None),
                    x_flow("births", "population * birth_rate", None),
                    x_aux("birth_rate", "0.05", None),
                ],
            )],
        );
        compile_and_check_resolution(&dm_project, "main");
    }

    /// The resolved module's slot count comes from the layout, never from the
    /// symbolic value. `SymbolicCompiledModule` carries no slot count at all
    /// now, so this pins the remaining half: a bigger layout produces a bigger
    /// module from the same fragments.
    #[test]
    fn test_resolve_uses_layout_n_slots() {
        let layout = simple_layout();
        let sym = SymbolicCompiledModule {
            ident: Ident::new("main"),
            compiled_initials: vec![],
            compiled_flows: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::LoadVar {
                        var: sref("population", 0),
                    },
                    SymbolicOpcode::AssignCurr {
                        var: sref("births", 0),
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            compiled_stocks: SymbolicByteCode::default(),
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            dimensions: vec![],
            names: vec![],
            temp_offsets: vec![],
            temp_total_size: 0,
            dim_lists: vec![],
            flows_invariant_opcode_len: 0,
        };

        assert_eq!(
            resolve_module(&sym, &layout).unwrap().n_slots,
            layout.n_slots
        );

        // A variable added past the end grows the layout; the same fragments
        // resolve into the bigger module.
        let mut bigger_entries = layout.entries.clone();
        bigger_entries.insert(
            "new_var".to_string(),
            LayoutEntry {
                offset: layout.n_slots,
                size: 1,
            },
        );
        let bigger_layout = VariableLayout::new(bigger_entries, layout.n_slots + 1);
        assert_eq!(
            resolve_module(&sym, &bigger_layout).unwrap().n_slots,
            bigger_layout.n_slots
        );
    }

    // ====================================================================
    // u16 truncation boundary tests (issue #291)
    // ====================================================================

    /// A model bigger than u16 can address is rejected before assembly, but the
    /// resolution primitives must handle large *usize* offsets correctly up to
    /// that point rather than truncating.
    #[test]
    fn test_large_offset_static_view() {
        let large_off: usize = 70_000;
        let mut entries = HashMap::new();
        entries.insert(
            "big_var".to_string(),
            LayoutEntry {
                offset: large_off,
                size: 3,
            },
        );
        let layout = VariableLayout::new(entries, large_off + 3);

        let sym = SymbolicStaticView {
            base: SymStaticViewBase::Var(sref("big_var", 0)),
            dims: SmallVec::from_slice(&[3]),
            strides: SmallVec::from_slice(&[1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0]),
        };

        let resolved = resolve_static_view(&sym, &layout).unwrap();
        assert_eq!(resolved.base_off, large_off as u32);
        assert_eq!(resolved.storage, ViewStorage::Curr);
    }

    #[test]
    fn test_large_offset_module_decl() {
        let large_off: usize = 70_000;
        let mut entries = HashMap::new();
        entries.insert(
            "big_module".to_string(),
            LayoutEntry {
                offset: large_off,
                size: 5,
            },
        );
        let layout = VariableLayout::new(entries, large_off + 5);

        let sym = SymbolicModuleDecl {
            model_name: Ident::new("sub"),
            input_set: BTreeSet::new(),
            var: sref("big_module", 0),
        };

        let resolved = resolve_module_decl(&sym, &layout).unwrap();
        assert_eq!(resolved.off, large_off);
    }

    // ====================================================================
    // Resolution coverage: one case per opcode family
    // ====================================================================
    //
    // `resolve_opcode` is an exhaustive match over `SymbolicOpcode`, so a new
    // variant is a compile error there; these pin that each existing arm maps
    // to the right concrete opcode with its operands intact.

    #[test]
    fn test_resolve_control_flow_and_builtin_opcodes() {
        for (sym, concrete) in [
            (SymbolicOpcode::Not {}, Opcode::Not {}),
            (SymbolicOpcode::SetCond {}, Opcode::SetCond {}),
            (SymbolicOpcode::If {}, Opcode::If {}),
            (
                SymbolicOpcode::LoadModuleInput { input: 3 },
                Opcode::LoadModuleInput { input: 3 },
            ),
            (
                SymbolicOpcode::EvalModule { id: 0, n_inputs: 2 },
                Opcode::EvalModule { id: 0, n_inputs: 2 },
            ),
            (
                SymbolicOpcode::Apply {
                    func: BuiltinId::Abs,
                },
                Opcode::Apply {
                    func: BuiltinId::Abs,
                },
            ),
            (
                SymbolicOpcode::Lookup {
                    base_gf: 0,
                    table_count: 4,
                    mode: LookupMode::Interpolate,
                },
                Opcode::Lookup {
                    base_gf: 0,
                    table_count: 4,
                    mode: LookupMode::Interpolate,
                },
            ),
            (
                SymbolicOpcode::Lookup {
                    base_gf: 1,
                    table_count: 1,
                    mode: LookupMode::Forward,
                },
                Opcode::Lookup {
                    base_gf: 1,
                    table_count: 1,
                    mode: LookupMode::Forward,
                },
            ),
            (SymbolicOpcode::Ret, Opcode::Ret),
        ] {
            assert_resolves(sym, concrete);
        }
    }

    #[test]
    fn test_resolve_view_stack_opcodes() {
        for (sym, concrete) in [
            (
                SymbolicOpcode::PushStaticView { view_id: 3 },
                Opcode::PushStaticView { view_id: 3 },
            ),
            (
                SymbolicOpcode::PushVarViewDirect {
                    var: sref("population", 0),
                    dim_list_id: 1,
                },
                Opcode::PushVarViewDirect {
                    base_off: 5,
                    dim_list_id: 1,
                },
            ),
            (
                SymbolicOpcode::ViewSubscriptDynamic { dim_idx: 1 },
                Opcode::ViewSubscriptDynamic { dim_idx: 1 },
            ),
            (
                SymbolicOpcode::ViewRangeDynamic { dim_idx: 2 },
                Opcode::ViewRangeDynamic { dim_idx: 2 },
            ),
            (SymbolicOpcode::PopView {}, Opcode::PopView {}),
        ] {
            assert_resolves(sym, concrete);
        }
    }

    #[test]
    fn test_resolve_temp_and_subscript_opcodes() {
        for (sym, concrete) in [
            (
                SymbolicOpcode::LoadTempConst {
                    temp_id: 0,
                    index: 3,
                },
                Opcode::LoadTempConst {
                    temp_id: 0,
                    index: 3,
                },
            ),
            (
                SymbolicOpcode::PushSubscriptIndex { bounds: 4 },
                Opcode::PushSubscriptIndex { bounds: 4 },
            ),
            (
                SymbolicOpcode::LoadSubscript {
                    var: sref("population", 0),
                },
                Opcode::LoadSubscript { off: 5 },
            ),
            (
                SymbolicOpcode::BinOpAssignNext {
                    op: Op2::Add,
                    var: sref("births", 0),
                },
                Opcode::BinOpAssignNext {
                    op: Op2::Add,
                    off: 4,
                },
            ),
            (
                SymbolicOpcode::SymLoadPrev {
                    var: sref("population", 0),
                },
                Opcode::LoadPrev { off: 5 },
            ),
            (
                SymbolicOpcode::SymLoadInitial {
                    var: sref("births", 0),
                },
                Opcode::LoadInitial { off: 4 },
            ),
        ] {
            assert_resolves(sym, concrete);
        }
    }

    #[test]
    fn test_resolve_iteration_opcodes() {
        for (sym, concrete) in [
            (
                SymbolicOpcode::BeginIter {
                    write_temp_id: 0,
                    has_write_temp: true,
                },
                Opcode::BeginIter {
                    write_temp_id: 0,
                    has_write_temp: true,
                },
            ),
            (
                SymbolicOpcode::BeginIter {
                    write_temp_id: 0,
                    has_write_temp: false,
                },
                Opcode::BeginIter {
                    write_temp_id: 0,
                    has_write_temp: false,
                },
            ),
            (
                SymbolicOpcode::LoadIterViewAt { offset: 2 },
                Opcode::LoadIterViewAt { offset: 2 },
            ),
            (
                SymbolicOpcode::StoreIterElement {},
                Opcode::StoreIterElement {},
            ),
            (
                SymbolicOpcode::NextIterOrJump { jump_back: -5 },
                Opcode::NextIterOrJump { jump_back: -5 },
            ),
            (SymbolicOpcode::EndIter {}, Opcode::EndIter {}),
        ] {
            assert_resolves(sym, concrete);
        }
    }

    #[test]
    fn test_resolve_reduction_and_vector_opcodes() {
        for (sym, concrete) in [
            (SymbolicOpcode::ArraySum {}, Opcode::ArraySum {}),
            (SymbolicOpcode::ArrayMax {}, Opcode::ArrayMax {}),
            (SymbolicOpcode::ArrayMin {}, Opcode::ArrayMin {}),
            (SymbolicOpcode::ArrayMean {}, Opcode::ArrayMean {}),
            (SymbolicOpcode::ArrayStddev {}, Opcode::ArrayStddev {}),
            (SymbolicOpcode::ArraySize {}, Opcode::ArraySize {}),
            (SymbolicOpcode::VectorSelect {}, Opcode::VectorSelect {}),
            (
                SymbolicOpcode::VectorElmMap {
                    write_temp_id: 1,
                    full_source_len: 12,
                },
                Opcode::VectorElmMap {
                    write_temp_id: 1,
                    full_source_len: 12,
                },
            ),
            (
                SymbolicOpcode::VectorSortOrder { write_temp_id: 2 },
                Opcode::VectorSortOrder { write_temp_id: 2 },
            ),
            (
                SymbolicOpcode::Rank { write_temp_id: 3 },
                Opcode::Rank { write_temp_id: 3 },
            ),
            (
                SymbolicOpcode::LookupArray {
                    base_gf: 2,
                    table_count: 3,
                    mode: LookupMode::Backward,
                    write_temp_id: 4,
                },
                Opcode::LookupArray {
                    base_gf: 2,
                    table_count: 3,
                    mode: LookupMode::Backward,
                    write_temp_id: 4,
                },
            ),
            (
                SymbolicOpcode::AllocateAvailable { write_temp_id: 5 },
                Opcode::AllocateAvailable { write_temp_id: 5 },
            ),
            (
                SymbolicOpcode::AllocateByPriority { write_temp_id: 6 },
                Opcode::AllocateByPriority { write_temp_id: 6 },
            ),
        ] {
            assert_resolves(sym, concrete);
        }
    }

    // ====================================================================
    // Error path coverage
    // ====================================================================

    #[test]
    fn test_resolve_opcode_unknown_variable() {
        let layout = simple_layout();
        let err = resolve_opcode(
            &SymbolicOpcode::LoadVar {
                var: sref("nonexistent", 0),
            },
            &layout,
        )
        .unwrap_err();
        assert!(err.contains("not found in layout"), "got: {err}");
    }

    #[test]
    fn test_resolve_static_view_missing_variable() {
        let layout = simple_layout();
        let sym_view = SymbolicStaticView {
            base: SymStaticViewBase::Var(sref("nonexistent", 0)),
            dims: SmallVec::from_slice(&[3]),
            strides: SmallVec::from_slice(&[1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0]),
        };

        let err = resolve_static_view(&sym_view, &layout).unwrap_err();
        assert!(err.contains("not found in layout"));
    }

    #[test]
    fn test_resolve_module_decl_missing_variable() {
        let layout = simple_layout();
        let sym_decl = SymbolicModuleDecl {
            model_name: Ident::new("sub"),
            input_set: BTreeSet::new(),
            var: sref("nonexistent", 0),
        };

        let err = resolve_module_decl(&sym_decl, &layout).unwrap_err();
        assert!(err.contains("not found in layout"));
    }

    // ====================================================================
    // Integration: module with submodules
    // ====================================================================

    #[test]
    fn test_roundtrip_module_with_submodel() {
        let dm_project = x_project(
            default_sim_specs(),
            &[
                x_model(
                    "main",
                    vec![
                        x_aux("input_val", "42", None),
                        x_module("inner", &[("input_val", "x")], None),
                    ],
                ),
                x_model(
                    "inner",
                    vec![x_aux("x", "0", None), x_aux("y", "x * 2", None)],
                ),
            ],
        );
        compile_and_check_resolution(&dm_project, "main");
    }

    // ====================================================================
    // renumber_opcode bounds checking (fix #5)
    // ====================================================================

    #[test]
    fn test_renumber_opcode_temp_offset_overflow() {
        let op = SymbolicOpcode::LoadTempConst {
            temp_id: 0,
            index: 0,
        };
        let err = renumber_opcode(&op, 0, &[], 0, 0, 300, 0).unwrap_err();
        assert!(
            err.contains("TempId capacity"),
            "expected TempId overflow error, got: {}",
            err
        );
    }

    #[test]
    fn test_renumber_opcode_gf_base_out_of_range_is_loud() {
        // #582: the GF base is now content-remapped through a per-fragment
        // remap (not a flat add). A `base_gf` outside the remap's range is
        // a corrupt fragment -- it must fail loud rather than silently read
        // a wrong (or out-of-bounds) table.
        let op = SymbolicOpcode::Lookup {
            base_gf: 3,
            table_count: 1,
            mode: LookupMode::Interpolate,
        };
        // Remap only covers slots 0..2; base_gf 3 is out of range.
        let err = renumber_opcode(&op, 0, &[0, 1], 0, 0, 0, 0).unwrap_err();
        assert!(
            err.contains("out of range for fragment GF remap"),
            "expected out-of-range GF remap error, got: {}",
            err
        );
    }

    #[test]
    fn test_renumber_opcode_gf_remap_translates_base() {
        // The remap relocates `base_gf` to its deduped global slot; the
        // happy path must apply it (and leave `table_count` intact).
        let op = SymbolicOpcode::Lookup {
            base_gf: 1,
            table_count: 1,
            mode: LookupMode::Interpolate,
        };
        match renumber_opcode(&op, 0, &[5, 9, 13], 0, 0, 0, 0).unwrap() {
            SymbolicOpcode::Lookup {
                base_gf,
                table_count,
                ..
            } => {
                assert_eq!(base_gf, 9, "base_gf must be remapped via gf_remap[1]");
                assert_eq!(table_count, 1);
            }
            other => panic!("expected Lookup, got {:?}", other),
        }
    }

    #[test]
    fn test_renumber_opcode_at_boundary() {
        // u8::MAX = 255, so temp_off=255 should succeed
        let op = SymbolicOpcode::LoadTempConst {
            temp_id: 0,
            index: 0,
        };
        assert!(renumber_opcode(&op, 0, &[], 0, 0, 255, 0).is_ok());

        // A GF base remapped to 255 (the last valid GraphicalFunctionId)
        // should succeed.
        let op = SymbolicOpcode::Lookup {
            base_gf: 0,
            table_count: 1,
            mode: LookupMode::Interpolate,
        };
        assert!(renumber_opcode(&op, 0, &[255], 0, 0, 0, 0).is_ok());
    }

    /// A fragment carrying `n` module declarations and nothing else.
    fn module_decls_frag(n: usize) -> PerVarBytecodes {
        PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: (0..n)
                    .map(|i| SymbolicOpcode::EvalModule {
                        id: i as ModuleId,
                        n_inputs: 0,
                    })
                    .chain(std::iter::once(SymbolicOpcode::Ret))
                    .collect(),
            },
            graphical_functions: vec![],
            module_decls: (0..n)
                .map(|i| SymbolicModuleDecl {
                    model_name: Ident::new(&format!("sub{i}")),
                    input_set: BTreeSet::new(),
                    var: SymVarRef::new(Ident::new("m"), i),
                })
                .collect(),
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        }
    }

    #[test]
    fn test_gf_ids_are_independent_of_the_flat_tables() {
        // Fragment A has 1 GF and its opcode references GF 0
        let frag_a = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![1.0],
                code: vec![
                    SymbolicOpcode::Lookup {
                        base_gf: 0,
                        table_count: 1,
                        mode: LookupMode::Interpolate,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![vec![(0.0, 0.0), (1.0, 1.0)]],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        };

        // Fragment B has 1 GF and its opcode references GF 0
        let frag_b = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![2.0],
                code: vec![
                    SymbolicOpcode::Lookup {
                        base_gf: 0,
                        table_count: 1,
                        mode: LookupMode::Interpolate,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![vec![(0.0, 0.0), (2.0, 2.0)]],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        };

        // The two fragments carry DIFFERENT GF content, so they stay
        // distinct: frag_a's GF at 0, frag_b's at 1 (#582 dedup is
        // value-exact -- different content never collides).
        let merged = concatenate_fragments(&[&frag_a, &frag_b]).unwrap();
        assert_eq!(merged.graphical_functions.len(), 2);
        match &merged.bytecode.code[0] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 0),
            other => panic!("expected Lookup, got {:?}", other),
        }
        match &merged.bytecode.code[1] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 1),
            other => panic!("expected Lookup, got {:?}", other),
        }

        // GF numbering is INDEPENDENT of the flat tables already merged --
        // graphical functions are content-de-duplicated and remapped, not
        // flat-offset by a preceding program's counts (#582). Five module
        // declarations absorbed by an earlier program of the same module must
        // NOT shift the next program's GF indices.
        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        merger.concatenate(&[&module_decls_frag(5)]).unwrap();
        let later = merger.concatenate(&[&frag_a, &frag_b]).unwrap();
        match &later.code[0] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 0),
            other => panic!("expected Lookup, got {:?}", other),
        }
        match &later.code[1] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 1),
            other => panic!("expected Lookup, got {:?}", other),
        }
    }

    #[test]
    fn test_concatenate_renumbers_static_view_temp_base() {
        // A static view whose base is a temp must be renumbered by the SAME
        // temp offset the merger assigns the temp it points at. #583: a
        // module's programs RECYCLE temps into one identity pool, so two
        // fragments' id-0 temps share slot 0 -- a `Temp(0)` static view base
        // stays `Temp(0)` (it tracks the recycled slot, NOT a per-fragment
        // sum). Under `Sum` the base moves with the fragment's temps, which is
        // exercised below.
        let frag_a = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![SymbolicOpcode::Ret],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 4)],
            dim_lists: vec![],
        };
        let frag_b = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![SymbolicOpcode::Ret],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![SymbolicStaticView {
                base: SymStaticViewBase::Temp(0),
                dims: SmallVec::new(),
                strides: SmallVec::new(),
                offset: 0,
                sparse: SmallVec::new(),
                dim_ids: SmallVec::new(),
            }],
            temp_sizes: vec![(0, 8)],
            dim_lists: vec![],
        };

        // Under `Recycle`, frag_b's Temp(0) recycles to slot 0 -- the same slot
        // frag_a's temp 0 occupies (max size 8). The view base must stay
        // Temp(0).
        let merged = concatenate_fragments(&[&frag_a, &frag_b]).unwrap();
        assert_eq!(merged.static_views.len(), 1);
        match &merged.static_views[0].base {
            SymStaticViewBase::Temp(id) => assert_eq!(
                *id, 0,
                "recycle: frag_b's Temp(0) view base recycles to shared slot 0"
            ),
            other => panic!("expected Temp base, got {:?}", other),
        }
        assert_eq!(
            merged.temp_offsets.len(),
            1,
            "both fragments' id-0 temps recycle to one slot"
        );

        // Under `Sum` -- the interleaving emitter's strategy -- frag_b's temp
        // follows frag_a's one slot, and its view's Temp base moves with it:
        // the base is renumbered by the same per-fragment offset as the
        // fragment's temp opcodes, whichever strategy sets that offset.
        let mut merger = FragmentMerger::new(TempStrategy::Sum);
        merger.concatenate(&[&frag_a, &frag_b]).unwrap();
        let summed = merger.into_side_channels();
        assert_eq!(
            summed.temp_offsets.len(),
            2,
            "Sum gives the two fragments disjoint slots"
        );
        match &summed.static_views[0].base {
            SymStaticViewBase::Temp(id) => assert_eq!(
                *id, 1,
                "Temp(0) view base follows frag_a's one temp under Sum"
            ),
            other => panic!("expected Temp base, got {:?}", other),
        }
    }

    #[test]
    fn test_renumber_vector_builtin_temp_ids() {
        // VectorElmMap, VectorSortOrder, and AllocateAvailable each carry
        // a write_temp_id that must be renumbered during fragment
        // concatenation, just like LoadTempConst and BeginIter.
        let temp_off: u32 = 5;

        let elm_map = SymbolicOpcode::VectorElmMap {
            write_temp_id: 0,
            full_source_len: 6,
        };
        match renumber_opcode(&elm_map, 0, &[], 0, 0, temp_off, 0).unwrap() {
            SymbolicOpcode::VectorElmMap {
                write_temp_id,
                full_source_len,
            } => {
                assert_eq!(write_temp_id, 5);
                // full_source_len passes through unchanged (absolute, not a temp id)
                assert_eq!(full_source_len, 6);
            }
            other => panic!("expected VectorElmMap, got {:?}", other),
        }

        let sort_order = SymbolicOpcode::VectorSortOrder { write_temp_id: 2 };
        match renumber_opcode(&sort_order, 0, &[], 0, 0, temp_off, 0).unwrap() {
            SymbolicOpcode::VectorSortOrder { write_temp_id } => {
                assert_eq!(write_temp_id, 7);
            }
            other => panic!("expected VectorSortOrder, got {:?}", other),
        }

        let alloc = SymbolicOpcode::AllocateAvailable { write_temp_id: 1 };
        match renumber_opcode(&alloc, 0, &[], 0, 0, temp_off, 0).unwrap() {
            SymbolicOpcode::AllocateAvailable { write_temp_id } => {
                assert_eq!(write_temp_id, 6);
            }
            other => panic!("expected AllocateAvailable, got {:?}", other),
        }
    }

    #[test]
    fn test_vector_elm_map_full_source_len_survives_fragment_roundtrip() {
        // End-to-end belt-and-suspenders for the Phase 5 `full_source_len`
        // opcode field. `test_renumber_vector_builtin_temp_ids` covers the
        // isolated `renumber_opcode` call; this exercises the *real merge
        // path* a compiled model takes -- codegen's symbolic opcode ->
        // `concatenate_fragments` (absorb + `renumber_fragment_code`) ->
        // `resolve_bytecode` -- with the VECTOR ELM MAP opcode merged AFTER a
        // temp-contributing fragment so its `write_temp_id` gets a *non-zero*
        // renumber offset. The invariant under test: `full_source_len` is an
        // absolute element count, NOT a renumber-able resource id, so it must
        // come out of concatenate/renumber -> resolve byte-identical even
        // though `write_temp_id` is offset. If `full_source_len` were ever
        // (mistakenly) treated like a temp id, it would be shifted by
        // `frag_a`'s temp count here and this test would fail; the existing
        // renumber unit test would not catch that regression because it never
        // drives the fragment merger.
        const GENUINE_FULL_SOURCE_LEN: u32 = 6; // e.g. d[DimA,DimB] = 3 x 2
        let symbolic_elm_map = SymbolicOpcode::VectorElmMap {
            write_temp_id: 0,
            full_source_len: GENUINE_FULL_SOURCE_LEN,
        };

        // frag_a is a temp-bearing fragment; frag_b carries the VectorElmMap.
        // #583: a module's programs RECYCLE temps into one identity pool, so
        // under `Recycle` frag_b's id-0 write_temp_id would stay 0 and the
        // renumber would be the identity. To keep this test's renumber
        // NON-trivial -- so the `full_source_len` survival assertion is
        // load-bearing -- the merge runs under `Sum`, the interleaving
        // emitter's strategy, which places frag_b's temp past frag_a's two.
        let frag_a = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![SymbolicOpcode::Ret],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 4), (1, 8)],
            dim_lists: vec![],
        };
        let frag_b = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![symbolic_elm_map],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 4)],
            dim_lists: vec![],
        };

        let mut merger = FragmentMerger::new(TempStrategy::Sum);
        let merged = merger.concatenate(&[&frag_a, &frag_b]).unwrap();

        // Resolve to concrete bytecode against an empty layout: the
        // VectorElmMap opcode carries no variable reference.
        let empty_layout = VariableLayout::new(HashMap::new(), 0);
        let resolved = resolve_bytecode(&merged, &empty_layout).unwrap();

        let elm_map = resolved
            .code
            .iter()
            .find_map(|op| match op {
                Opcode::VectorElmMap {
                    write_temp_id,
                    full_source_len,
                } => Some((*write_temp_id, *full_source_len)),
                _ => None,
            })
            .expect("merged+resolved bytecode should contain a VectorElmMap opcode");

        // The merge path actually ran a non-trivial renumber on this opcode:
        // `Sum` places frag_b's write_temp_id 0 past frag_a's two temps. (If
        // this were 0, the merger never renumbered the opcode and the
        // full_source_len assertion below would prove nothing.)
        assert_eq!(
            elm_map.0, 2,
            "write_temp_id must be offset past frag_a's temps, proving the \
             fragment merger renumbered this opcode"
        );
        // The invariant: full_source_len is absolute, not renumbered. It must
        // survive concatenate/renumber -> resolve unchanged even
        // though write_temp_id was offset.
        assert_eq!(
            elm_map.1, GENUINE_FULL_SOURCE_LEN,
            "full_source_len must survive the fragment-merge -> resolve path \
             byte-identical (it is an absolute element \
             count, not a renumber-able resource id); a corrupted value would \
             feed the VM a wrong full-source extent and break genuine VECTOR \
             ELM MAP results"
        );
    }

    // ====================================================================
    // M3: a resource id past its bytecode type is reported, never wrapped.
    //
    // `GraphicalFunctionId` (#582) and `TempId` (#583) both overflowed on
    // C-LEARN and both fail loud. The four `u16` resources -- literals, module
    // decls, static views, dim lists -- were the unguarded members of the same
    // family: `absorb_non_gf` narrowed their bases with `as u16` and
    // `renumber_opcode` added them unchecked, so a merged table past 65,536
    // entries produced a wrapped id that names a real, in-range resource. The
    // program runs and returns different numbers, with no diagnostic anywhere.
    //
    // The bound is EXACT in both directions -- a table of 65,536 entries is
    // addressable, 65,537 is not -- and the tests below say so at each surface
    // that reaches it: within one program, across the programs of a module, and
    // through a standalone program. Stating it at every surface is what keeps a
    // surface from carrying its own, different boundary.
    // ====================================================================

    /// A fragment carrying `n` literals and nothing else.
    fn literal_pool_frag(n: usize) -> PerVarBytecodes {
        PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![1.0; n],
                code: vec![SymbolicOpcode::LoadConstant { id: 0 }, SymbolicOpcode::Ret],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        }
    }

    /// M3 for the literal pool, the cheapest `u16` resource to overflow and the
    /// one whose failure is worst: a wrapped `LiteralId` names a real, in-range
    /// literal, so the program runs and returns different numbers.
    #[test]
    fn literal_pool_past_u16_capacity_fails_loud() {
        let cap = u16::MAX as usize + 1;
        let a = literal_pool_frag(cap - 1);
        let b = literal_pool_frag(4);
        let err = concatenate_fragments(&[&a, &b])
            .expect_err("a literal pool past u16 capacity must be reported, not wrapped");
        assert!(
            err.contains("literal") && err.contains("16-bit"),
            "expected a loud literal-capacity error, got: {err}"
        );

        // ...and one entry less is still fine, so the bound is exact rather than
        // conservative.
        let c = literal_pool_frag(1);
        concatenate_fragments(&[&a, &c])
            .expect("a pool of exactly u16::MAX + 1 literals is addressable");

        // A fragment carrying NONE of the resource is exempt even at a full
        // table: it names no id, so there is nothing that has to be
        // representable. Without the exemption the base -- which this fragment
        // never uses -- would be rejected for being one past `u16::MAX`.
        let empty = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![SymbolicOpcode::Ret],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        };
        concatenate_fragments(&[&a, &c, &empty])
            .expect("a fragment with no literals must not be bounded by a full pool");
    }

    /// A fragment carrying `n` dim lists and nothing else.
    fn dim_list_frag(n: usize) -> PerVarBytecodes {
        PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::PushVarViewDirect {
                        var: SymVarRef::base(Ident::new("x")),
                        dim_list_id: 0,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![vec![1u16]; n],
        }
    }

    /// The same bound, one level up: across the PROGRAMS of a module, where a
    /// later program's base is the table length the earlier programs left.
    ///
    /// The literal pool cannot reach this path -- it is per program, so it
    /// always restarts at 0 -- which is why the exactness
    /// `literal_pool_past_u16_capacity_fails_loud` pins is restated here for a
    /// table that DOES span programs: filling it exactly is legal, a following
    /// program that names none of the resource is exempt, and one that names a
    /// single entry past the end is reported.
    #[test]
    fn a_full_table_bounds_only_the_programs_that_name_it() {
        let cap = U16_ID_CAPACITY;
        let full = dim_list_frag(cap);
        let one = dim_list_frag(1);
        let none = dim_list_frag(0);

        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        merger
            .concatenate(&[&full])
            .expect("a program filling the dim-list table exactly is addressable");
        merger
            .concatenate(&[&none])
            .expect("a program naming no dim list is not bounded by a full table");
        let err = merger
            .concatenate(&[&one])
            .expect_err("a dim list id past u16 capacity must be reported, not wrapped");
        assert!(
            err.contains("dimension list") && err.contains("16-bit"),
            "expected a loud dim-list-capacity error, got: {err}"
        );

        // ...and the bound is exact from below: one entry short of capacity, the
        // following program's single entry is id 65,535 and merges.
        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        merger
            .concatenate(&[&dim_list_frag(cap - 1)])
            .expect("one short of capacity");
        merger
            .concatenate(&[&one])
            .expect("the last addressable dim list id (u16::MAX) must merge");

        // The same exactness within one program's merged table, so the two
        // halves of the bound are stated against the same resource.
        concatenate_fragments(&[&dim_list_frag(cap - 1), &one])
            .expect("a merged table of exactly u16::MAX + 1 dim lists is addressable");
        concatenate_fragments(&[&full, &one])
            .expect_err("one dim list past a full merged table must be reported");
        concatenate_fragments(&[&full, &none])
            .expect("a fragment with no dim lists must not be bounded by a full table");
    }

    /// A standalone program extends the module tables through the same
    /// absorption a concatenation does, so it shares the bound exactly: an
    /// initial that fills the dim-list table is addressable, a following one
    /// naming no dim list is exempt, and one needing id 65,536 is reported.
    #[test]
    fn a_standalone_program_shares_the_tables_bound() {
        let cap = U16_ID_CAPACITY;
        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        merger
            .standalone_program(&dim_list_frag(cap))
            .expect("initials that fill the dim-list table exactly are addressable");
        merger
            .standalone_program(&dim_list_frag(0))
            .expect("an initial naming no dim list is not bounded by a full table");
        let err = merger
            .standalone_program(&dim_list_frag(1))
            .expect_err("an initial needing dim list id 65,536 must be reported");
        assert!(
            err.contains("dimension list") && err.contains("16-bit"),
            "expected a loud dim-list-capacity error, got: {err}"
        );
    }

    /// Literal pools are per program. Each initial keeps its own pool and the
    /// flows and stocks keep one each, so only a single program's pool is
    /// bounded: a module whose pools each fit assembles even when they would
    /// not fit as one.
    #[test]
    fn literal_pools_are_per_program() {
        let cap = U16_ID_CAPACITY;
        let full = literal_pool_frag(cap);
        let one = literal_pool_frag(1);

        // Merged into ONE program they assign literal id 65,536 -- that pool
        // would be retained, so the bound is right.
        let err = concatenate_fragments(&[&full, &one])
            .expect_err("a retained pool past capacity must be reported");
        assert!(
            err.contains("literal") && err.contains("16-bit"),
            "expected a loud literal-capacity error, got: {err}"
        );

        // As two programs of one module each pool starts at 0 and both fit.
        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        let first = merger
            .concatenate(&[&full])
            .expect("a pool of exactly u16::MAX + 1 literals is addressable");
        let second = merger
            .concatenate(&[&one])
            .expect("the next program's pool starts at literal id 0");
        assert_eq!(first.literals.len(), cap);
        assert_eq!(second.literals.len(), 1);
        assert!(
            matches!(second.code[0], SymbolicOpcode::LoadConstant { id: 0 }),
            "the second program's literal ids start at 0: {:?}",
            second.code[0]
        );

        // Standalone programs likewise.
        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        merger
            .standalone_program(&full)
            .expect("an initial's own pool may fill the table");
        let next = merger
            .standalone_program(&one)
            .expect("the next initial's pool starts at literal id 0");
        assert_eq!(next.literals.len(), 1);
    }

    /// The same property end-to-end, through `assemble_module` on a real model,
    /// so a future refactor that points the all-phases aggregation back at a
    /// full merge fails here rather than only at the unit level.
    ///
    /// Reaching the bound honestly would need a model with 65,537 literals,
    /// which is orders of magnitude outside the per-test time budget, so the
    /// capacity is shrunk instead (`docs/dev/rust.md#test-time-budgets`, the
    /// repo's stated preference over building a fixture large enough to trip a
    /// production threshold). Five scalar stocks give one literal per initial
    /// (five pools of one), five in the flows pool and five in the stocks pool
    /// -- fifteen in aggregate. At a capacity of 8 every RETAINED pool fits and
    /// only the discarded aggregate does not.
    ///
    /// The second half is what stops this from being a test that merely proves
    /// a check was deleted: at a capacity of 4 the flows pool genuinely does not
    /// fit, and assembly must still fail.
    #[test]
    fn assembly_bounds_the_retained_pools_and_not_the_aggregate() {
        use crate::test_common::TestProject;

        let model = || {
            let mut p = TestProject::new("literal_capacity");
            for i in 0..5 {
                p = p
                    .stock(
                        &format!("s{i}"),
                        &format!("{}", i + 1),
                        &[&format!("f{i}")],
                        &[],
                        None,
                    )
                    .flow(&format!("f{i}"), &format!("{}", 10 + i), None);
            }
            p
        };

        {
            let _cap = IdCapacityGuard::new(8);
            model()
                .compile_incremental()
                .expect("every retained literal pool fits; only the discarded aggregate does not");
        }

        {
            let _cap = IdCapacityGuard::new(4);
            let err = model()
                .compile_incremental()
                .expect_err("the flows phase's own five-literal pool does not fit in four ids");
            assert!(
                format!("{err:?}").contains("literal"),
                "expected the failure to name the literal pool, got: {err:?}"
            );
        }
    }

    /// M3 for the per-opcode add: `renumber_opcode` checks it itself rather than
    /// relying on the merger's table bound alone, so a base the merger did not
    /// compute could not wrap an id.
    #[test]
    fn renumber_opcode_u16_addition_overflow_is_loud() {
        let cases: Vec<(SymbolicOpcode, &str, [u16; 4])> = vec![
            (
                SymbolicOpcode::LoadConstant { id: u16::MAX },
                "LiteralId",
                [1, 0, 0, 0],
            ),
            (
                SymbolicOpcode::EvalModule {
                    id: u16::MAX,
                    n_inputs: 0,
                },
                "ModuleId",
                [0, 1, 0, 0],
            ),
            (
                SymbolicOpcode::PushStaticView { view_id: u16::MAX },
                "ViewId",
                [0, 0, 1, 0],
            ),
            (
                SymbolicOpcode::PushVarViewDirect {
                    var: SymVarRef::base(Ident::new("x")),
                    dim_list_id: u16::MAX,
                },
                "DimListId",
                [0, 0, 0, 1],
            ),
        ];
        for (op, label, [lit, md, vw, dl]) in cases {
            let err = renumber_opcode(&op, lit, &[], md, vw, 0, dl)
                .expect_err("an id at u16::MAX plus a non-zero base must be reported");
            assert!(
                err.contains(label) && err.contains("overflow"),
                "expected a loud {label} overflow, got: {err}"
            );
        }
    }

    #[test]
    fn test_renumber_opcode_u8_addition_overflow() {
        // temp_off=200 fits in u8, but base temp_id=100 + 200 = 300 overflows u8
        let op = SymbolicOpcode::LoadTempConst {
            temp_id: 100,
            index: 0,
        };
        let err = renumber_opcode(&op, 0, &[], 0, 0, 200, 0).unwrap_err();
        assert!(
            err.contains("overflow"),
            "expected overflow error, got: {}",
            err
        );
    }

    // ====================================================================
    // #582, M4: cross-fragment graphical-function de-duplication.
    //
    // `concatenate_fragments` previously appended every fragment's
    // `graphical_functions` with no de-duplication (the flat running
    // `gf_offset = merged_gf.len()`), so a dependency arrayed GF referenced
    // by N consumer fragments duplicated N times and `renumber_opcode`'s
    // `gf_off > u8::MAX` guard tripped once the duplicated count crossed
    // 255 -- even though the *distinct* count is small. Codegen lays each
    // table-bearing variable's tables down exactly once per fragment (its
    // `tables` map is keyed by ident); the duplication was entirely ACROSS
    // fragments, which is why de-duplicating on block content is a complete
    // fix rather than a capacity workaround.
    // ====================================================================

    /// Build a single-`Lookup` fragment carrying one scalar GF table whose
    /// content is `data`, with its opcode referencing GF 0.
    fn gf_lookup_frag(data: Vec<(f64, f64)>) -> PerVarBytecodes {
        PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::Lookup {
                        base_gf: 0,
                        table_count: 1,
                        mode: LookupMode::Interpolate,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![data],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        }
    }

    /// Every opcode that carries a `base_gf`, with the run it reports.
    ///
    /// Derived from the enum rather than sampled: `gf_run`'s match is
    /// exhaustive with no `_`, so the compiler is what guarantees a new variant
    /// answers the question, and this pins the answer for the three that do
    /// carry one. A representative non-carrier of each shape (unit and struct)
    /// is included so the `None` side is exercised too.
    #[test]
    fn gf_run_reports_every_lookup_family_opcode() {
        let rows: Vec<(SymbolicOpcode, Option<(usize, usize)>)> = vec![
            (
                SymbolicOpcode::Lookup {
                    base_gf: 3,
                    table_count: 2,
                    mode: LookupMode::Interpolate,
                },
                Some((3, 2)),
            ),
            (
                SymbolicOpcode::LookupDirect {
                    base_gf: 5,
                    table_count: 4,
                    elem: 1,
                    mode: LookupMode::Interpolate,
                },
                Some((5, 4)),
            ),
            (
                SymbolicOpcode::LookupArray {
                    base_gf: 7,
                    table_count: 6,
                    mode: LookupMode::Interpolate,
                    write_temp_id: 0,
                },
                Some((7, 6)),
            ),
            (SymbolicOpcode::Ret, None),
            (SymbolicOpcode::SetCond {}, None),
        ];
        for (op, want) in rows {
            assert_eq!(op.gf_run(), want, "gf_run of {op:?}");
        }
    }

    /// Two SEPARATE single-table GF blocks in one fragment, each read by a
    /// `LookupDirect`, merged with a fragment holding only the second table's
    /// content.
    ///
    /// This is the pin on `gf_blocks_of_fragment`'s opcode scan, and it is
    /// built to FAIL if a lookup-family opcode is added without teaching that
    /// scan about it. The scan ends in a `_ => continue`, so an unknown
    /// lookup opcode is skipped SILENTLY -- the two referenced runs stop being
    /// seen as runs and collapse into one maximal un-referenced GAP block
    /// `[0, 2)`. A gap block is keyed for de-duplication by its whole content,
    /// so the shared second table no longer matches the other fragment's copy
    /// and the merge yields three tables instead of two, with the interior
    /// `base_gf` remapped off the wrong block base.
    ///
    /// Asserting the deduped COUNT is what makes the test discriminating: a
    /// fixture with a single block would dedup identically whether or not the
    /// opcode were known, and would pin nothing.
    #[test]
    fn test_gf_block_scan_sees_lookup_direct_runs() {
        let table_a = vec![(0.0, 1.0), (1.0, 2.0)];
        let table_b = vec![(0.0, 5.0), (1.0, 6.0)];

        // One fragment, two distinct single-table blocks, both read through
        // the constant-element-offset form.
        let two_blocks = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::LookupDirect {
                        base_gf: 0,
                        table_count: 1,
                        elem: 0,
                        mode: LookupMode::Interpolate,
                    },
                    SymbolicOpcode::LookupDirect {
                        base_gf: 1,
                        table_count: 1,
                        elem: 0,
                        mode: LookupMode::Interpolate,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![table_a.clone(), table_b.clone()],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![],
            dim_lists: vec![],
        };
        // A second fragment holding ONLY table_b, so a correct scan lets the
        // two copies of table_b dedup to one slot.
        let shares_b = gf_lookup_frag(table_b.clone());

        let merged = concatenate_fragments(&[&two_blocks, &shares_b]).unwrap();

        assert_eq!(
            merged.graphical_functions.len(),
            2,
            "the two LookupDirect runs must be seen as separate blocks so the \
             shared table de-duplicates; 3 means `gf_blocks_of_fragment` did \
             not recognise LookupDirect and collapsed them into one gap block"
        );
        assert!(merged.graphical_functions.contains(&table_a));
        assert!(merged.graphical_functions.contains(&table_b));

        // And every emitted lookup must still address a real table.
        for op in &merged.bytecode.code {
            let base = match op {
                SymbolicOpcode::LookupDirect { base_gf, .. }
                | SymbolicOpcode::Lookup { base_gf, .. } => *base_gf as usize,
                _ => continue,
            };
            assert!(
                base < merged.graphical_functions.len(),
                "remapped base_gf {base} is past the merged table list"
            );
        }
    }

    #[test]
    fn test_concatenate_dedups_identical_gf_tables_under_u8_capacity() {
        // 300 consumer fragments, each referencing the SAME dependency GF
        // table by content. The pre-fix flat append would push 300 tables
        // into `merged_gf` and the 256th `base_gf` renumber would overflow
        // u8 -- exactly the C-LEARN `... exceeds GraphicalFunctionId
        // capacity` failure. With content de-duplication there is ONE
        // distinct table, so every `base_gf` resolves to 0 and the result
        // is well under u8::MAX.
        let shared = vec![(0.0, 0.0), (1.0, 10.0), (2.0, 20.0)];
        let frags: Vec<PerVarBytecodes> =
            (0..300).map(|_| gf_lookup_frag(shared.clone())).collect();
        let refs: Vec<&PerVarBytecodes> = frags.iter().collect();

        let merged = concatenate_fragments(&refs)
            .expect("identical GF tables must de-duplicate, not overflow u8");

        assert_eq!(
            merged.graphical_functions.len(),
            1,
            "300 fragments sharing one GF table must collapse to a single \
             distinct table"
        );
        assert_eq!(merged.graphical_functions[0], shared);
        // Every fragment's Lookup must resolve to the single deduped table.
        let lookups: Vec<u8> = merged
            .bytecode
            .code
            .iter()
            .filter_map(|op| match op {
                SymbolicOpcode::Lookup { base_gf, .. } => Some(*base_gf),
                _ => None,
            })
            .collect();
        assert_eq!(lookups.len(), 300);
        assert!(
            lookups.iter().all(|&b| b == 0),
            "all 300 Lookups must point at the single deduped table index 0"
        );
    }

    #[test]
    fn test_concatenate_keeps_distinct_gf_tables_distinct() {
        // Value-exactness guard: two tables with DIFFERENT content must
        // NEVER merge to one index (that would silently make a Lookup read
        // the wrong table). Three fragments: A and C share content, B is
        // distinct -> exactly two deduped tables, and A/C point at one, B
        // at the other.
        let content_ac = vec![(0.0, 1.0), (1.0, 2.0)];
        let content_b = vec![(0.0, 1.0), (1.0, 99.0)]; // same x, different y
        let frag_a = gf_lookup_frag(content_ac.clone());
        let frag_b = gf_lookup_frag(content_b.clone());
        let frag_c = gf_lookup_frag(content_ac.clone());

        let merged = concatenate_fragments(&[&frag_a, &frag_b, &frag_c]).unwrap();

        assert_eq!(
            merged.graphical_functions.len(),
            2,
            "distinct-content tables must stay distinct"
        );
        let lookups: Vec<u8> = merged
            .bytecode
            .code
            .iter()
            .filter_map(|op| match op {
                SymbolicOpcode::Lookup { base_gf, .. } => Some(*base_gf),
                _ => None,
            })
            .collect();
        assert_eq!(lookups.len(), 3);
        // A and C must resolve to the SAME index; B to a DIFFERENT one.
        assert_eq!(lookups[0], lookups[2], "A and C share content");
        assert_ne!(lookups[0], lookups[1], "B is distinct content");
        // And each resolved index must actually hold that fragment's content.
        assert_eq!(
            merged.graphical_functions[lookups[0] as usize], content_ac,
            "A's resolved table must be A's content"
        );
        assert_eq!(
            merged.graphical_functions[lookups[1] as usize], content_b,
            "B's resolved table must be B's content (NOT A's)"
        );
    }

    #[test]
    fn test_concatenate_dedups_arrayed_gf_lists_preserving_contiguity() {
        // A `LookupArray` reads `graphical_functions[base_gf .. base_gf +
        // table_count]`, so an arrayed GF is a CONTIGUOUS run of tables.
        // Whole-list de-duplication must keep that run contiguous and in
        // order: two fragments carrying the same 3-table list collapse to
        // one shared run; a third fragment carrying a DIFFERENT list gets
        // its own contiguous run.
        let list_xy = vec![vec![(0.0, 0.0)], vec![(0.0, 10.0)], vec![(0.0, 20.0)]];
        let list_z = vec![
            vec![(0.0, 0.0)],  // shares element 0 content with list_xy
            vec![(0.0, 99.0)], // diverges at element 1
            vec![(0.0, 20.0)],
        ];
        let arrayed_frag = |list: Vec<Vec<(f64, f64)>>| PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::LookupArray {
                        base_gf: 0,
                        table_count: 3,
                        mode: LookupMode::Interpolate,
                        write_temp_id: 0,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: list,
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 3)],
            dim_lists: vec![],
        };
        let fa = arrayed_frag(list_xy.clone());
        let fb = arrayed_frag(list_xy.clone());
        let fc = arrayed_frag(list_z.clone());

        let merged = concatenate_fragments(&[&fa, &fb, &fc]).unwrap();

        // list_xy (shared by fa, fb) + list_z (fc) = 2 distinct 3-table
        // runs = 6 tables, NOT the pre-fix 9.
        assert_eq!(
            merged.graphical_functions.len(),
            6,
            "two distinct 3-table lists must dedup to 6 tables, contiguity \
             preserved"
        );
        let bases: Vec<u8> = merged
            .bytecode
            .code
            .iter()
            .filter_map(|op| match op {
                SymbolicOpcode::LookupArray { base_gf, .. } => Some(*base_gf),
                _ => None,
            })
            .collect();
        assert_eq!(bases.len(), 3);
        assert_eq!(bases[0], bases[1], "fa and fb share the list");
        assert_ne!(bases[0], bases[2], "fc's list diverges");
        // Each resolved run must be exactly that fragment's list, in order
        // (the contiguity contract `LookupArray` depends on).
        let read_run = |base: u8| -> Vec<Vec<(f64, f64)>> {
            (0..3)
                .map(|k| merged.graphical_functions[base as usize + k].clone())
                .collect()
        };
        assert_eq!(read_run(bases[0]), list_xy);
        assert_eq!(read_run(bases[2]), list_z);
    }

    #[test]
    fn test_concatenate_dedups_overlapping_element_and_whole_array_refs() {
        // A single fragment can reference a per-element arrayed GF BOTH as
        // the whole array (`LookupArray { base_gf: 0, table_count: 3 }`) and
        // at a specific element (`Lookup { base_gf: 1, table_count: 1 }`) --
        // the `base_gf` ranges overlap/nest. The whole-list shift must
        // relocate BOTH refs by the same offset so the element ref still
        // lands inside the relocated array. (This is the
        // `lookup/lookup.xmile`-shape that a naive disjoint-block dedup
        // mis-rejected as "overlapping blocks".)
        let arrayed_list = vec![vec![(0.0, 0.0)], vec![(0.0, 10.0)], vec![(0.0, 20.0)]];
        let overlap_frag = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::LookupArray {
                        base_gf: 0,
                        table_count: 3,
                        mode: LookupMode::Interpolate,
                        write_temp_id: 0,
                    },
                    SymbolicOpcode::Lookup {
                        base_gf: 1, // element 1 of the SAME arrayed GF
                        table_count: 1,
                        mode: LookupMode::Interpolate,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: arrayed_list.clone(),
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 3)],
            dim_lists: vec![],
        };
        // A preceding fragment with one distinct table forces a non-zero
        // shift, so the relocation is actually exercised.
        let prefix = gf_lookup_frag(vec![(5.0, 5.0)]);

        let merged = concatenate_fragments(&[&prefix, &overlap_frag])
            .expect("overlapping element/whole-array refs must not be rejected");

        // prefix (1 table) + the 3-table arrayed list = 4 tables, none
        // dropped, none mis-detected as overlapping.
        assert_eq!(merged.graphical_functions.len(), 4);
        let (array_base, elem_base) = {
            let mut array_base = None;
            let mut elem_base = None;
            for op in &merged.bytecode.code {
                match op {
                    SymbolicOpcode::LookupArray { base_gf, .. } => array_base = Some(*base_gf),
                    SymbolicOpcode::Lookup { base_gf, .. } => elem_base = Some(*base_gf),
                    _ => {}
                }
            }
            (array_base.unwrap(), elem_base.unwrap())
        };
        // The array was shifted to start at 1 (after the prefix table); the
        // element ref must remain its +1 interior offset (now 2).
        assert_eq!(array_base, 1, "arrayed GF shifted past the prefix table");
        assert_eq!(
            elem_base, 2,
            "the element ref must stay at array_base + 1 after the whole-list shift"
        );
        // The relocated run must hold the arrayed list verbatim, and the
        // element ref must index its element 1.
        assert_eq!(
            merged.graphical_functions[array_base as usize..array_base as usize + 3].to_vec(),
            arrayed_list
        );
        assert_eq!(
            merged.graphical_functions[elem_base as usize],
            arrayed_list[1]
        );
    }

    // ====================================================================
    // #583, M5: the sequential concat shares temp slots by IDENTITY.
    //
    // A fragment's temps are 0-based scratch that dies at the end of that
    // fragment's runlist segment, and `FragmentMerger::concatenate` emits
    // each fragment as one contiguous run, so no two fragments' temp uses
    // interleave and fragment A's temp 0 may occupy the same slot as fragment
    // B's temp 0 -- sized to the MAX of its users, since each of them holds it
    // alone for the length of its own segment. That sharing is not an
    // optimization: `TempId` is `u8`, and summing 0-based per-fragment counts
    // across a model overflows it (the C-LEARN
    // `temp offset ... exceeds TempId capacity` failure, #583).
    // `combine_scc_fragment` interleaves per-element segments, so its members'
    // live ranges DO overlap and it stays on the disjoint `Sum` path -- see
    // `db/combined_fragment_tests`.
    //
    // These fixtures pin the rule on hand-built shapes; the general statement
    // (over arbitrary fragment sets, including the non-interleaving property
    // that is the actual justification) is
    // `symbolic_merge_proptest::recycle_shares_by_identity_and_sizes_by_max`
    // and `merged_temp_slot_uses_never_interleave`.
    // ====================================================================

    /// Build a single-variable-shaped fragment carrying a `VectorSortOrder`
    /// whose `write_temp_id` is `local_tid`, plus a `temp_sizes` entry for it.
    /// Models a per-variable fragment whose temps start at 0 (the plain-phase
    /// concat input shape `compile_phase_to_per_var_bytecodes` produces).
    fn sort_order_temp_frag(local_tid: TempId, size: usize) -> PerVarBytecodes {
        PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::VectorSortOrder {
                        write_temp_id: local_tid,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(local_tid as u32, size)],
            dim_lists: vec![],
        }
    }

    #[test]
    fn test_concatenate_recycles_three_id_zero_temps_onto_one_slot() {
        // Three per-variable fragments, each with its own 0-based temp 0.
        // They are emitted as three disjoint contiguous runs, so all three may
        // hold ONE slot in turn: the pool is one slot wide, sized to the
        // largest of them.
        let frag_a = sort_order_temp_frag(0, 4);
        let frag_b = sort_order_temp_frag(0, 8);
        let frag_c = sort_order_temp_frag(0, 2);
        let refs: Vec<&PerVarBytecodes> = vec![&frag_a, &frag_b, &frag_c];

        let merged = concatenate_fragments(&refs).unwrap();

        assert_eq!(
            merged.temp_offsets.len(),
            1,
            "three fragments' id-0 temps must recycle onto one shared slot, \
             not sum to three"
        );
        assert_eq!(
            merged.temp_total_size, 8,
            "a shared slot must be sized to the LARGEST of its users, or the \
             fragment needing 8 elements would write past its storage"
        );

        // Every renumbered temp opcode must resolve in-range (index < pool).
        for op in &merged.bytecode.code {
            if let SymbolicOpcode::VectorSortOrder { write_temp_id } = op {
                assert!(
                    (*write_temp_id as usize) < merged.temp_offsets.len(),
                    "renumbered write_temp_id {write_temp_id} out of range for \
                     temp pool of size {}",
                    merged.temp_offsets.len()
                );
            }
        }
    }

    #[test]
    fn test_concatenate_temp_recycle_distinct_ids_max_merge() {
        // A fragment using temp ids {0, 1} and another using {0}: the pool is
        // the UNION of the ids (2 slots), each sized to the max of the
        // fragments using that id -- not 3 summed slots.
        let frag_a = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![],
                code: vec![
                    SymbolicOpcode::VectorSortOrder { write_temp_id: 0 },
                    SymbolicOpcode::VectorElmMap {
                        write_temp_id: 1,
                        full_source_len: 4,
                    },
                    SymbolicOpcode::Ret,
                ],
            },
            graphical_functions: vec![],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 4), (1, 6)],
            dim_lists: vec![],
        };
        let frag_b = sort_order_temp_frag(0, 8);
        let refs: Vec<&PerVarBytecodes> = vec![&frag_a, &frag_b];

        let merged = concatenate_fragments(&refs).unwrap();
        assert_eq!(
            merged.temp_offsets.len(),
            2,
            "ids {{0,1}} unioned with {{0}} -> 2 slots"
        );
        // Slot 0 size = max(4, 8) = 8; slot 1 size = 6.
        assert_eq!(merged.temp_total_size, 8 + 6);

        // frag_b's write_temp_id 0 must stay 0 (identity recycle), NOT be
        // pushed to 2 by frag_a's two temps.
        let sort_writes: Vec<TempId> = merged
            .bytecode
            .code
            .iter()
            .filter_map(|op| match op {
                SymbolicOpcode::VectorSortOrder { write_temp_id } => Some(*write_temp_id),
                _ => None,
            })
            .collect();
        assert_eq!(
            sort_writes,
            vec![0, 0],
            "both fragments' id-0 sort writes recycle to slot 0"
        );
    }

    #[test]
    fn test_temps_recycle_to_one_pool_across_a_modules_programs() {
        // Every program of a module shares ONE identity temp pool: the same
        // fragment temp gets the same slot whether it is merged into the first
        // program or into a later one whose flat tables already hold entries,
        // and the module's pool is one slot wide. (A per-program partition of
        // the pool would drive the renumbered `temp_id` past `u8::MAX` on a
        // large model and make a later program index temp storage the tables
        // do not describe -- #583.)
        let frag = sort_order_temp_frag(0, 4);
        let mut merger = FragmentMerger::new(TempStrategy::Recycle);
        let first = merger
            .concatenate(&[&frag, &dim_list_frag(2), &module_decls_frag(3)])
            .unwrap();
        let later = merger.concatenate(&[&frag]).unwrap();
        let tables = merger.into_side_channels();

        let temp_write = |bc: &SymbolicByteCode| -> TempId {
            bc.code
                .iter()
                .find_map(|op| match op {
                    SymbolicOpcode::VectorSortOrder { write_temp_id } => Some(*write_temp_id),
                    _ => None,
                })
                .expect("a VectorSortOrder opcode")
        };
        assert_eq!(
            temp_write(&first),
            temp_write(&later),
            "the same fragment temp must get the same identity id in every program of the module"
        );
        assert_eq!(temp_write(&later), 0, "identity recycle keeps id 0");
        assert_eq!(
            tables.temp_offsets.len(),
            1,
            "the module's temp pool is one slot wide, not one slot per program"
        );
    }

    #[test]
    fn test_concatenate_genuinely_distinct_gf_over_capacity_fails_loud() {
        // If a model genuinely has MORE than `GraphicalFunctionId::MAX + 1`
        // (256) *distinct* GF tables, de-duplication cannot help -- the ID
        // width truly cannot address them. That must fail with a clear
        // capacity error (the escalation case), NEVER silently wrap a
        // `base_gf` to a wrong table.
        let frags: Vec<PerVarBytecodes> = (0..300)
            .map(|i| gf_lookup_frag(vec![(0.0, i as f64), (1.0, (i + 1) as f64)]))
            .collect();
        let refs: Vec<&PerVarBytecodes> = frags.iter().collect();
        let err = concatenate_fragments(&refs)
            .expect_err("300 genuinely-distinct GF tables exceed u8 capacity");
        assert!(
            err.contains("distinct graphical function count")
                && err.contains("GraphicalFunctionId capacity"),
            "expected a loud distinct-GF-capacity error, got: {err}"
        );
    }
}
