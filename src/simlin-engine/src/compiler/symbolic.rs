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
    VariableOffset, ViewId,
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
    // ── Opcodes codegen does not emit ────────────────────────────────────
    //
    // The sixteen variants carrying `#[allow(dead_code)]` in this enum are ones
    // `Compiler` never constructs, which -- now that codegen emits symbolic
    // opcodes directly -- the dead-code lint proves rather than merely
    // suggests: `Compiler` is the ONLY producer of a `SymbolicOpcode`, and
    // `resolve_opcode` is the only producer of an `Opcode`, so a symbolic
    // variant nothing constructs is a program the compiler cannot express.
    // They are the superseded halves of two schemes: incremental view-stack
    // construction (`ViewSubscriptConst`/`ViewRange`/`ViewStarRange`/
    // `ViewWildcard`/`ViewTranspose`/`DupView`/`PushTempView`/
    // `LoadTempDynamic`/`LoadIter{Element,TempElement,ViewTop}`), which
    // precomputed `PushStaticView` views replaced, and broadcast iteration
    // (`*BroadcastIter*`/`*BroadcastElement`/`NextBroadcastOrJump`), which
    // `BeginIter` replaced.
    //
    // They are NOT deleted here, deliberately. Each also has an `Opcode`
    // twin with a VM execution arm and a wasm-backend lowering arm (~149
    // sites plus their tests), so removing them is a change to the VM
    // instruction set and the wasm parity harness -- a different subsystem,
    // with a different risk profile, that would swamp this change's review
    // surface. The broadcast family in particular reads as a half-landed
    // feature, so retiring it is a product call rather than a cleanup.
    // Sequenced as its own change; the evidence it needs is exactly this
    // lint, so it does not need the empirical probe GH #964's earlier
    // opcode deletions did.
    #[allow(dead_code)]
    PushTempView {
        temp_id: TempId,
        dim_list_id: DimListId,
    },
    PushStaticView {
        view_id: ViewId,
    },
    PushVarViewDirect {
        var: SymVarRef,
        dim_list_id: DimListId,
    },
    #[allow(dead_code)]
    ViewSubscriptConst {
        dim_idx: u8,
        index: u16,
    },
    ViewSubscriptDynamic {
        dim_idx: u8,
    },
    #[allow(dead_code)]
    ViewRange {
        dim_idx: u8,
        start: u16,
        end: u16,
    },
    ViewRangeDynamic {
        dim_idx: u8,
    },
    #[allow(dead_code)]
    ViewStarRange {
        dim_idx: u8,
        subdim_relation_id: u16,
    },
    #[allow(dead_code)]
    ViewWildcard {
        dim_idx: u8,
    },
    #[allow(dead_code)]
    ViewTranspose {},
    PopView {},
    #[allow(dead_code)]
    DupView {},

    // === TEMP ARRAY ACCESS (unchanged) ===
    LoadTempConst {
        temp_id: TempId,
        index: u16,
    },
    #[allow(dead_code)]
    LoadTempDynamic {
        temp_id: TempId,
    },

    // === ITERATION (unchanged) ===
    BeginIter {
        write_temp_id: TempId,
        has_write_temp: bool,
    },
    #[allow(dead_code)]
    LoadIterElement {},
    #[allow(dead_code)]
    LoadIterTempElement {
        temp_id: TempId,
    },
    #[allow(dead_code)]
    LoadIterViewTop {},
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

    // === BROADCASTING ITERATION (unchanged) ===
    #[allow(dead_code)]
    BeginBroadcastIter {
        n_sources: u8,
        dest_temp_id: TempId,
    },
    #[allow(dead_code)]
    LoadBroadcastElement {
        source_idx: u8,
    },
    #[allow(dead_code)]
    StoreBroadcastElement {},
    #[allow(dead_code)]
    NextBroadcastOrJump {
        jump_back: PcOffset,
    },
    #[allow(dead_code)]
    EndBroadcastIter {},
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SymStaticViewBase {
    /// Model variable reference (replaces base_off when is_temp=false)
    Var(SymVarRef),
    /// Temp array ID (kept as-is when is_temp=true)
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
    pub arrays: Vec<crate::bytecode::ArrayDefinition>,
    pub dimensions: Vec<crate::bytecode::DimensionInfo>,
    pub subdim_relations: Vec<crate::bytecode::SubdimensionRelation>,
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

    /// Build from a model's `name -> (offset, size)` map.
    ///
    /// `#[cfg(test)]` because its only caller is: the test-only monolithic
    /// `compiler::Module` builds its whole-model layout this way and resolves
    /// its emitted symbolic module against it. Production gets the same shape
    /// from the salsa `compute_layout` query instead.
    #[cfg(test)]
    pub fn from_offset_map(
        offsets: &HashMap<crate::common::Ident<crate::common::Canonical>, (usize, usize)>,
        n_slots: usize,
    ) -> Self {
        let entries = offsets
            .iter()
            .map(|(name, (offset, size))| {
                (
                    name.to_string(),
                    LayoutEntry {
                        offset: *offset,
                        size: *size,
                    },
                )
            })
            .collect();
        VariableLayout { entries, n_slots }
    }

    pub fn get(&self, name: &str) -> Option<&LayoutEntry> {
        self.entries.get(name)
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
    /// computed. Both machineries that produce final root offsets consume
    /// it: `assemble_module`'s root path (it resolves every fragment
    /// `SymVarRef` and module-decl `off` against the shifted layout) and
    /// `calc_flattened_offsets_incremental`'s LTM section (it reads the
    /// shifted entry offsets so the results map agrees with the assembled
    /// module entry-for-entry). Centralizing it keeps the two in lockstep:
    /// they cannot diverge on the reservation amount, the implicit-global
    /// slots, or the body offset.
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
            SymbolicOpcode::NextIterOrJump { jump_back }
            | SymbolicOpcode::NextBroadcastOrJump { jump_back } => Some(*jump_back),
            _ => None,
        }
    }

    /// Mutably borrow the jump offset, if this opcode is a backward jump.
    fn jump_offset_mut(&mut self) -> Option<&mut PcOffset> {
        match self {
            SymbolicOpcode::NextIterOrJump { jump_back }
            | SymbolicOpcode::NextBroadcastOrJump { jump_back } => Some(jump_back),
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
        SymbolicOpcode::PushTempView {
            temp_id,
            dim_list_id,
        } => Ok(Opcode::PushTempView {
            temp_id: *temp_id,
            dim_list_id: *dim_list_id,
        }),
        SymbolicOpcode::PushStaticView { view_id } => {
            Ok(Opcode::PushStaticView { view_id: *view_id })
        }
        SymbolicOpcode::ViewSubscriptConst { dim_idx, index } => Ok(Opcode::ViewSubscriptConst {
            dim_idx: *dim_idx,
            index: *index,
        }),
        SymbolicOpcode::ViewSubscriptDynamic { dim_idx } => {
            Ok(Opcode::ViewSubscriptDynamic { dim_idx: *dim_idx })
        }
        SymbolicOpcode::ViewRange {
            dim_idx,
            start,
            end,
        } => Ok(Opcode::ViewRange {
            dim_idx: *dim_idx,
            start: *start,
            end: *end,
        }),
        SymbolicOpcode::ViewRangeDynamic { dim_idx } => {
            Ok(Opcode::ViewRangeDynamic { dim_idx: *dim_idx })
        }
        SymbolicOpcode::ViewStarRange {
            dim_idx,
            subdim_relation_id,
        } => Ok(Opcode::ViewStarRange {
            dim_idx: *dim_idx,
            subdim_relation_id: *subdim_relation_id,
        }),
        SymbolicOpcode::ViewWildcard { dim_idx } => Ok(Opcode::ViewWildcard { dim_idx: *dim_idx }),
        SymbolicOpcode::ViewTranspose {} => Ok(Opcode::ViewTranspose {}),
        SymbolicOpcode::PopView {} => Ok(Opcode::PopView {}),
        SymbolicOpcode::DupView {} => Ok(Opcode::DupView {}),
        SymbolicOpcode::LoadTempConst { temp_id, index } => Ok(Opcode::LoadTempConst {
            temp_id: *temp_id,
            index: *index,
        }),
        SymbolicOpcode::LoadTempDynamic { temp_id } => {
            Ok(Opcode::LoadTempDynamic { temp_id: *temp_id })
        }
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => Ok(Opcode::BeginIter {
            write_temp_id: *write_temp_id,
            has_write_temp: *has_write_temp,
        }),
        SymbolicOpcode::LoadIterElement {} => Ok(Opcode::LoadIterElement {}),
        SymbolicOpcode::LoadIterTempElement { temp_id } => {
            Ok(Opcode::LoadIterTempElement { temp_id: *temp_id })
        }
        SymbolicOpcode::LoadIterViewTop {} => Ok(Opcode::LoadIterViewTop {}),
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
            // full-array-source `out_of_bounds_element_returns_nan_{vm,
            // monolithic}` (base 0, `source_is_full_array == true`) and the
            // strict-slice-source `strict_slice_source_oob_returns_nan_{vm,
            // monolithic}` (base != 0, the other branch) both feed an
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
        SymbolicOpcode::BeginBroadcastIter {
            n_sources,
            dest_temp_id,
        } => Ok(Opcode::BeginBroadcastIter {
            n_sources: *n_sources,
            dest_temp_id: *dest_temp_id,
        }),
        SymbolicOpcode::LoadBroadcastElement { source_idx } => Ok(Opcode::LoadBroadcastElement {
            source_idx: *source_idx,
        }),
        SymbolicOpcode::StoreBroadcastElement {} => Ok(Opcode::StoreBroadcastElement {}),
        SymbolicOpcode::NextBroadcastOrJump { jump_back } => Ok(Opcode::NextBroadcastOrJump {
            jump_back: *jump_back,
        }),
        SymbolicOpcode::EndBroadcastIter {} => Ok(Opcode::EndBroadcastIter {}),
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
    let (base_off, is_temp) = match &sv.base {
        SymStaticViewBase::Var(var_ref) => {
            let entry = layout.get(var_ref.name.as_str()).ok_or_else(|| {
                format!(
                    "variable '{}' not found in layout during static view resolution",
                    var_ref.name
                )
            })?;
            ((entry.offset + var_ref.element_offset) as u32, false)
        }
        // A view base is the ONE place a temp id is carried as a `u32`. Every
        // OTHER opcode that names a temp -- `BeginIter` and the
        // array-producing opcodes' `write_temp_id`, `LoadTempConst`'s
        // `temp_id` -- carries it as `TempId` (= `u8`), narrowed at emit time
        // with a plain `as`. So a view over a temp above 255 reads storage
        // nothing ever wrote: the writer's `as TempId` lands on `id % 256`
        // while this read lands on `id`, and the program is well-formed either
        // way -- wrong numbers with no diagnostic. Reject it in the resolution
        // layer, where the concrete program is produced (#583 is the real fix:
        // the id namespace is too small for a per-element hoist over a few
        // hundred elements). The write-side narrowing is deliberately left
        // unguarded; see the module note on this in the crate's CLAUDE.md.
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
            (*id, true)
        }
    };

    Ok(StaticArrayView {
        base_off,
        is_temp,
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
            arrays: sym.arrays.clone(),
            dimensions: sym.dimensions.clone(),
            subdim_relations: sym.subdim_relations.clone(),
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

/// Merged result of concatenating per-variable symbolic bytecodes.
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

/// Flat base offsets for the *non-GF* shared context resources when
/// concatenating multiple phases into a single resource namespace.
/// Literals are excluded because each phase's bytecode has its own literal
/// pool. Graphical functions are excluded because they are content-de-
/// duplicated (#582) rather than flat-counted -- their per-fragment base
/// comes from a shared `GfDedup` remap, not a running sum.
///
/// The three `u16`-addressed counts are held as `usize` on purpose. They are
/// COUNTS, not ids: a count of exactly `U16_ID_CAPACITY` describes a full table
/// whose every id (`0..=u16::MAX`) is representable, and a phase that follows it
/// with none of that resource assigns no id at all. Narrowing them here would
/// reject that program even though M3 -- *every assigned id is representable* --
/// holds for it; the one place that can tell is [`resource_base`], which sees
/// the following fragment's length and is where the bound therefore lives.
#[derive(Clone, Debug, Default)]
pub(crate) struct ContextResourceCounts {
    pub modules: usize,
    pub views: usize,
    pub temps: u32,
    pub dim_lists: usize,
}

impl ContextResourceCounts {
    /// Sum the flat (non-GF) context resource counts from a set of
    /// per-variable fragments. Used to derive a later phase's `ctx_base`
    /// from the preceding phases' module / view / dim-list counts (those
    /// resources are laid out disjointly per phase).
    ///
    /// The `temps` sum is a count utility only: temps RECYCLE into one
    /// global identity pool (#583), so `assemble_module` passes a
    /// `ctx_base.temps` of 0 for every plain phase (the recycle's fixed base)
    /// rather than this per-phase sum. The field is summed here for the
    /// benefit of any caller that genuinely wants the disjoint per-phase temp
    /// count (e.g. the `Sum` strategy / `combine_scc_fragment` accounting).
    ///
    /// Infallible: summing counts cannot violate M3 by itself, because a base
    /// only has to be representable once a fragment uses it to name something.
    /// The check lives in [`resource_base`], which is handed both the base and
    /// the length of the fragment about to consume it.
    pub fn from_fragments(fragments: &[&PerVarBytecodes]) -> Self {
        let mut modules: usize = 0;
        let mut views: usize = 0;
        let mut temps: u32 = 0;
        let mut dim_lists: usize = 0;
        for frag in fragments {
            modules += frag.module_decls.len();
            views += frag.static_views.len();
            // Each fragment's temps start at 0, so the disjoint-layout total
            // is the sum of each fragment's (max_id + 1), not the global max.
            let frag_temp_count = frag
                .temp_sizes
                .iter()
                .map(|(id, _)| *id + 1)
                .max()
                .unwrap_or(0);
            temps += frag_temp_count;
            dim_lists += frag.dim_lists.len();
        }
        ContextResourceCounts {
            modules,
            views,
            temps,
            dim_lists,
        }
    }
}

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
/// that already holds `merged_len`, itself offset by `ctx_base` (M2 + M3).
///
/// **This is the only place the `u16` capacity bound is stated.** Every base a
/// `u16`-addressed resource is renumbered against comes from here -- the
/// merger's four, and the running offsets `db::assemble::renumber_initials_phase`
/// tracks by hand -- so the boundary cannot be one value in one place and a
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
/// / `a_full_ctx_base_bounds_only_the_phases_that_use_it` /
/// `the_initials_phase_shares_the_mergers_capacity_bound` all red if they do.
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
    ctx_base: usize,
    merged_len: usize,
    frag_len: usize,
    label: &str,
) -> Result<u16, String> {
    let base = ctx_base + merged_len;
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
/// literal id" is a property of the type rather than a placeholder value. The
/// all-phases aggregation keeps no literal pool, and a `lit_offset: 0` sitting
/// in its result would be indistinguishable from a real base of zero.
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
///   run, in fragment order (`concatenate_fragments_with_gf`; the fragments
///   are sequential, non-overlapping runlist segments). Temps collapse by
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
/// This is the shared core of `concatenate_fragments_with_gf` and the
/// per-element-granular `combine_scc_fragment` (a multi-member recurrence
/// SCC's combined fragment). Both consumers absorb fragments through this one
/// implementation so they cannot drift: `concatenate_fragments_with_gf`
/// absorbs each fragment and immediately renumbers its whole (Ret-stripped)
/// code, while `combine_scc_fragment` absorbs each *member* once and
/// renumbers that member's per-element segments with the member's offsets,
/// emitting the segments in the SCC's interleaved `element_order`.
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
/// so does any number of later phases that add nothing to one -- so the counts
/// that become a later phase's base are carried in `usize` and the bound is
/// discharged in exactly one function, [`resource_base`], which is the only
/// point that sees both a base and the length of the fragment about to consume
/// it. "Assigned" also means assigned to something the module KEEPS: a merge
/// whose pool is discarded names no id, so bounding it rejects programs that
/// are entirely valid. That is not hypothetical either -- it is why the
/// all-phases aggregation is [`merge_context_side_channels`] and not a full
/// merge. Pinned by `literal_pool_past_u16_capacity_fails_loud` (within a
/// phase), `a_full_ctx_base_bounds_only_the_phases_that_use_it` (across
/// phases), `the_initials_phase_shares_the_mergers_capacity_bound` (the
/// initials phase's own running offsets),
/// `the_all_phases_aggregation_does_not_bound_the_literal_pool` and
/// `assembly_bounds_the_retained_pools_and_not_the_aggregate` (retained vs
/// discarded), `renumber_opcode_u16_addition_overflow_is_loud`,
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
/// **M6 -- absorption is 1:1 on opcodes.** The merged code is each fragment's
/// Ret-stripped opcodes, contiguous and in fragment order, plus one terminal
/// `Ret`. Two downstream boundaries are COUNTS into this stream and would move
/// silently if it were not 1:1: `flows_invariant_opcode_len` (the
/// run-invariant flow prefix, GH #712, computed in `db::assemble` as a sum of
/// Ret-stripped fragment lengths) and the SCC per-element segmentation.
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
/// **M8 -- the phase split agrees with the whole-model merge.** `assemble_module`
/// renumbers initials, flows and stocks SEPARATELY, but takes the module's
/// `module_decls` / `static_views` / `temp_offsets` / `dim_lists` tables from a
/// single all-phases merge. So merging one phase with `ctx_base` set to the
/// summed counts of the preceding phases must assign each fragment exactly the
/// ids the all-phases merge assigns it -- otherwise a flows opcode indexes the
/// module's table with an id computed against a different table. Literals are
/// the deliberate exception: each phase's bytecode carries its own pool and
/// the all-phases pool is discarded, so `ctx_base` does not apply to them.
/// Pinned by `phase_split_assigns_the_same_ids_as_the_all_phases_merge`, with
/// `forced_rich_phase_split_uses_non_zero_bases` as its non-vacuity guard --
/// with an all-zero `ctx_base` the split IS the all-phases merge, so the
/// property would pass without testing anything.
///
/// `ctx_base` provides the context resource id offsets inherited from
/// preceding phases (M8). Temps recycle into ONE pool shared by every phase,
/// so `ctx_base.temps` is 0 for every phase -- the pool is not partitioned by
/// phase. When merging a single phase in isolation, pass
/// `ContextResourceCounts::default()`.
pub(crate) struct FragmentMerger {
    ctx_base: ContextResourceCounts,
    temp_strategy: TempStrategy,
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
        let (base, count) = match op {
            SymbolicOpcode::Lookup {
                base_gf,
                table_count,
                ..
            }
            | SymbolicOpcode::LookupArray {
                base_gf,
                table_count,
                ..
            } => (*base_gf as usize, *table_count as usize),
            _ => continue,
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
    /// New merger with the disjoint-range (`Sum`) temp strategy -- the form
    /// `combine_scc_fragment` (interleaved segments) and the GF-only
    /// `GfDedup::build` (never touches temps) use.
    pub(crate) fn new(ctx_base: &ContextResourceCounts) -> Self {
        Self::new_with_temp_strategy(ctx_base, TempStrategy::Sum)
    }

    /// New merger with an explicit temp strategy (M5). A caller that emits
    /// each fragment's opcodes as one contiguous run passes
    /// `TempStrategy::Recycle`; one that interleaves them passes `Sum`. See
    /// [`TempStrategy`].
    pub(crate) fn new_with_temp_strategy(
        ctx_base: &ContextResourceCounts,
        temp_strategy: TempStrategy,
    ) -> Self {
        FragmentMerger {
            ctx_base: ctx_base.clone(),
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
    /// return the five flat non-GF resource base offsets plus the per-slot
    /// GF remap this fragment's opcodes must be renumbered by. This is
    /// `absorb_non_gf` followed by `absorb_gf` (see those for the contract);
    /// it is the form `combine_scc_fragment` and `GfDedup::build` use, where
    /// the GF dedup and the flat accounting must be driven by the same
    /// merger.
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
    /// The *temp* offset instead follows `temp_strategy` (#583, M5): `Sum`
    /// advances per fragment so each gets a disjoint range;  `Recycle` uses
    /// the fixed `ctx_base.temps` so every fragment's temp `t` lands on the
    /// same slot `t + base`, max-merged below. See [`TempStrategy`] for which
    /// emission shape needs which. Graphical functions are handled separately
    /// by `absorb_gf` (content-de-duplicated, #582, M4).
    ///
    /// `Err` on M3: a merged table that outgrows its `u16` id type. The base
    /// offsets are checked here rather than only at `renumber_opcode` because
    /// a fragment may carry entries no opcode reads (over-collected dependency
    /// resources), and those still consume ids for the fragments after it.
    pub(crate) fn absorb_non_gf(
        &mut self,
        frag: &PerVarBytecodes,
    ) -> Result<FragmentResourceOffsets, String> {
        // Literals are phase-local; no ctx_base offset needed (M8). They are
        // also the ONE resource an aggregation may legitimately not want, which
        // is why they are the half that lives here rather than in
        // `absorb_context` -- see that method's rustdoc.
        let lit_offset = resource_base(
            0,
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
    /// The literal pool is deliberately not touched.
    ///
    /// This is the whole of `absorb_non_gf` except the literal pool, and it is
    /// separate because the two have different lifetimes in the assembled
    /// module. The context tables are shared by every phase and reported ONCE,
    /// by the all-phases aggregation ([`merge_context_side_channels`]); the
    /// literal pools are per-phase and each is retained by the phase that built
    /// it. An aggregation that wants the former must not be bounded by the
    /// latter, because it assigns no merged literal id to anything.
    fn absorb_context(&mut self, frag: &PerVarBytecodes) -> Result<ContextResourceOffsets, String> {
        // Modules, views, and dim-lists are appended unshifted, so their offset
        // is `ctx_base + cumulative_appended` (no double-count: the appended
        // entries do NOT carry the ctx_base, so `merged_X.len()` excludes it).
        // Temps are different: see below.
        let mod_offset = resource_base(
            self.ctx_base.modules,
            self.merged_modules.len(),
            frag.module_decls.len(),
            "module declaration",
        )?;
        let view_offset = resource_base(
            self.ctx_base.views,
            self.merged_views.len(),
            frag.static_views.len(),
            "static view",
        )?;
        // #583: temps recycle (plain-phase) or sum (interleaved SCC).
        //
        // `Recycle`: a FIXED base (`ctx_base.temps`, which is 0 for every
        //   plain phase since temps share ONE global identity pool). The
        //   per-fragment max-merge below places fragment temp id `t` at slot
        //   `t + base`, so every fragment's id 0 collapses to the same slot.
        // `Sum`: advance by the running pool length so each fragment gets a
        //   disjoint range (interleaved segments need non-overlapping live
        //   ranges). NOTE the previous unconditional
        //   `merged_temp_sizes.len() + ctx_base.temps` double-counted
        //   `ctx_base.temps`: temps are stored at `id + temp_offset` (which
        //   already includes the base), so `merged_temp_sizes.len()` absorbs
        //   the base -- adding it again diverged `flows_concat` from the
        //   all-phases `merged` table (an M8 violation). The recycle path's
        //   fixed base removes that divergence; the Sum path runs only with
        //   `ctx_base.temps == 0` (`combine_scc_fragment` passes a default
        //   ctx_base).
        let temp_offset = match self.temp_strategy {
            TempStrategy::Recycle => self.ctx_base.temps,
            TempStrategy::Sum => self.merged_temp_sizes.len() as u32 + self.ctx_base.temps,
        };
        let dl_offset = resource_base(
            self.ctx_base.dim_lists,
            self.merged_dim_lists.len(),
            frag.dim_lists.len(),
            "dimension list",
        )?;

        self.merged_modules.extend_from_slice(&frag.module_decls);
        self.merged_views.extend(frag.static_views.iter().map(|sv| {
            let base = match &sv.base {
                SymStaticViewBase::Temp(id) => SymStaticViewBase::Temp(*id + temp_offset),
                other => other.clone(),
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
    pub(crate) fn absorb_gf(&mut self, frag: &PerVarBytecodes) -> Result<GfRemap, String> {
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

    /// Consume the merger and finalize into a `ConcatenatedBytecodes`,
    /// computing per-temp byte offsets from the max-merged temp sizes.
    /// `code` is the already-renumbered, Ret-stripped opcode stream;
    /// a single trailing `Ret` is appended iff `code` is non-empty
    /// (preserving the original `concatenate_fragments` behavior).
    fn into_concatenated(mut self, mut code: Vec<SymbolicOpcode>) -> ConcatenatedBytecodes {
        if !code.is_empty() {
            code.push(SymbolicOpcode::Ret);
        }
        let literals = std::mem::take(&mut self.merged_literals);
        let side = self.into_side_channels();
        ConcatenatedBytecodes {
            bytecode: SymbolicByteCode { literals, code },
            graphical_functions: side.graphical_functions,
            module_decls: side.module_decls,
            static_views: side.static_views,
            temp_offsets: side.temp_offsets,
            temp_total_size: side.temp_total_size,
            dim_lists: side.dim_lists,
        }
    }

    /// Consume the merger and finalize just the SHARED CONTEXT side-channels,
    /// discarding any literal pool. `temp_offsets` is the prefix sum of the
    /// merged temp sizes, computed here and nowhere else so the two finishers
    /// cannot disagree about the temp layout.
    fn into_side_channels(self) -> ContextSideChannels {
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
    /// fragment, re-fed to `concatenate_fragments` at assembly). `code` is
    /// the already-renumbered opcode stream of the interleaved segments;
    /// a single trailing `Ret` is appended iff `code` is non-empty.
    ///
    /// `temp_sizes`/`dim_lists` are converted back to the `PerVarBytecodes`
    /// representations: `merged_temp_sizes[i]` becomes `(i, size)` for
    /// every slot (including zero-size ones, so `from_fragments`'
    /// `max(id+1)` temp count is preserved), and each truncated dim-list
    /// `(n, arr)` becomes `arr[..n].to_vec()`. The truncation is
    /// idempotent on the <=4-element dimension tuples dim-lists hold, so a
    /// later `concatenate_fragments` pass is unaffected.
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

/// The context resources every phase of an assembled module SHARES, reported
/// once. These are the only things the all-phases aggregation exists to
/// produce; the bytecode and literal pool a full merge would also build are
/// per-phase and are retained by the phases that build them.
pub(crate) struct ContextSideChannels {
    pub graphical_functions: Vec<Vec<(f64, f64)>>,
    pub module_decls: Vec<SymbolicModuleDecl>,
    pub static_views: Vec<SymbolicStaticView>,
    pub temp_offsets: Vec<usize>,
    pub temp_total_size: usize,
    pub dim_lists: Vec<(u8, [u16; 4])>,
}

/// Aggregate the SHARED CONTEXT resources of every fragment in a module, in the
/// all-phases order (initials, flows, stocks), so the module reports one
/// module-decl / static-view / temp / dim-list table that each phase's
/// separately-renumbered ids index.
///
/// This is deliberately NOT a merge. The phases that keep bytecode
/// (`concatenate_fragments_with_gf` for flows and stocks,
/// `db::assemble::renumber_initials_phase` for initials) each keep their own
/// literal pool, so an aggregate pool over all three corresponds to no id the
/// module retains. Building one was not merely wasted work: it put the whole
/// model's literal count under `resource_base`'s `u16` bound, so a model whose
/// every RETAINED pool was comfortably addressable failed to assemble once the
/// three phases' pools summed past 65,536 -- roughly 33k scalar stocks, which
/// is inside the size class this compiler targets. Assembly used to survive it
/// by wrapping the offsets of a stream nobody read; the capacity check turned
/// that into a hard error.
///
/// M8 is unaffected: it obliges the per-phase renumber and the all-phases
/// aggregation to assign the same *context* ids, and literals were never part
/// of it (`absorb_non_gf` bases them at 0 in every phase precisely because they
/// are phase-local). Graphical functions come from the shared `dedup`, exactly
/// as they do for a phase merge.
pub(crate) fn merge_context_side_channels(
    fragments: &[&PerVarBytecodes],
    ctx_base: &ContextResourceCounts,
    dedup: &GfDedup,
) -> Result<ContextSideChannels, String> {
    // `Recycle` to match the phase merges: temp slots collapse by identity into
    // the one pool every phase shares, and this aggregation is what sizes it.
    let mut merger = FragmentMerger::new_with_temp_strategy(ctx_base, TempStrategy::Recycle);
    for frag in fragments {
        merger.absorb_context(frag)?;
    }
    let mut side = merger.into_side_channels();
    // The merger never touched GF (`absorb_context`), so install the shared
    // deduped table; every phase reports the same `graphical_functions`.
    side.graphical_functions = dedup.tables.clone();
    Ok(side)
}

/// Merge a single phase's `PerVarBytecodes` into one stream, renumbering
/// `LiteralId`, `GraphicalFunctionId`, `ModuleId`, `ViewId`, `TempId`, and
/// `DimListId` to avoid collisions across fragments, with the graphical
/// functions content-de-duplicated within the call (#582).
///
/// Assembly's multi-phase path uses `GfDedup::build` +
/// `concatenate_fragments_with_gf` directly (one shared GF dedup across all
/// phases); this single-call convenience wrapper -- a `GfDedup::build` over
/// exactly `fragments` followed by `concatenate_fragments_with_gf` -- is the
/// focused-unit-test surface for the merge + dedup behavior, so it is
/// `#[cfg(test)]`.
#[cfg(test)]
pub(crate) fn concatenate_fragments(
    fragments: &[&PerVarBytecodes],
    ctx_base: &ContextResourceCounts,
) -> Result<ConcatenatedBytecodes, String> {
    let dedup = GfDedup::build(fragments)?;
    concatenate_fragments_with_gf(fragments, ctx_base, &dedup, 0)
}

/// Cross-fragment graphical-function de-duplication result (#582): the one
/// de-duplicated GF table plus a per-fragment local-slot -> global-slot
/// remap, one entry per input fragment (by input index). Built once over
/// the union of all fragments that share a resource namespace, then handed
/// to every phase's renumber so each phase's `base_gf`s index the same
/// deduped table -- the only way the dedup stays coherent when the
/// initials / flows / stocks phases are renumbered separately.
pub(crate) struct GfDedup {
    /// The de-duplicated GF tables (the module's final `graphical_functions`).
    pub tables: Vec<Vec<(f64, f64)>>,
    /// Per-fragment (by input index) local-slot -> deduped-global-slot map.
    remaps: Vec<GfRemap>,
}

impl GfDedup {
    /// De-duplicate the GF blocks of `fragments` (in order) by bit-exact
    /// content (M4). Value-exact: two blocks share an offset only when their
    /// content is identical, so a `Lookup` can never be redirected to a
    /// different table. `Err` if the *distinct* GF count exceeds
    /// `GraphicalFunctionId` capacity (the genuine-capacity case the dedup
    /// cannot help -- escalate, do not widen the ID width here).
    pub fn build(fragments: &[&PerVarBytecodes]) -> Result<Self, String> {
        // Reuse `FragmentMerger`'s GF-dedup machinery on an otherwise-unused
        // merger so the de-dup logic lives in exactly one place. Only the
        // GF side-channel is touched (`absorb_gf`); the flat resources are
        // the per-phase callers' concern.
        let mut merger = FragmentMerger::new(&ContextResourceCounts::default());
        let mut remaps = Vec::with_capacity(fragments.len());
        for frag in fragments {
            remaps.push(merger.absorb_gf(frag)?);
        }
        Ok(GfDedup {
            tables: merger.merged_gf,
            remaps,
        })
    }

    pub(crate) fn remap(&self, frag_index: usize) -> &[GraphicalFunctionId] {
        &self.remaps[frag_index]
    }
}

/// Renumber `fragments` into one stream, using `dedup` for the (already
/// computed, possibly cross-phase) GF de-duplication and `ctx_base` +
/// flat running counts for the other resources. `gf_index_base` is the
/// position of `fragments[0]` within the fragment slice `dedup` was built
/// over (0 when `dedup` covers exactly `fragments`; the running phase
/// offset when one `GfDedup` spans initials + flows + stocks).
///
/// This is the sequential-emission consumer of `FragmentMerger`: each
/// fragment's Ret-stripped opcodes are appended as ONE contiguous run, in
/// fragment order (M6), which is what makes `TempStrategy::Recycle` sound
/// here (M5) and what makes a prefix of the fragment list correspond to a
/// prefix of the merged opcode stream -- the boundary `assemble_module`
/// counts for the run-invariant flow prefix.
pub(crate) fn concatenate_fragments_with_gf(
    fragments: &[&PerVarBytecodes],
    ctx_base: &ContextResourceCounts,
    dedup: &GfDedup,
    gf_index_base: usize,
) -> Result<ConcatenatedBytecodes, String> {
    // Fragments are appended whole and in order below, so no two fragments'
    // temp uses interleave and they may share slots by identity (M5).
    // `combine_scc_fragment`, which interleaves per-element segments, uses the
    // disjoint `Sum` path instead.
    let mut merger = FragmentMerger::new_with_temp_strategy(ctx_base, TempStrategy::Recycle);
    let mut merged_code: Vec<SymbolicOpcode> = Vec::new();

    for (i, frag) in fragments.iter().enumerate() {
        // Only the flat resources are merged here; GF numbering comes from
        // the shared `dedup` so it is coherent across phases.
        let off = merger.absorb_non_gf(frag)?;
        renumber_fragment_code(
            &frag.symbolic.code,
            &off,
            dedup.remap(gf_index_base + i),
            &mut merged_code,
        )?;
    }

    let mut concatenated = merger.into_concatenated(merged_code);
    // The merger never touched GF (`absorb_non_gf`), so install the shared
    // deduped table; every phase reports the same `graphical_functions`.
    concatenated.graphical_functions = dedup.tables.clone();
    Ok(concatenated)
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
/// There is no separate `temp_off > u8::MAX` precheck (#583): the plain-
/// phase concat recycles temps into one identity pool whose `temp_off` is 0
/// (or a small fixed `ctx_base.temps`), and `combine_scc_fragment` sums into
/// a per-SCC range bounded by the members' (small) temp counts. A genuine
/// per-opcode overflow -- a single variable bearing more than 255 temps, or
/// an SCC summing past 255 -- is still caught loud by `checked_add_u8`,
/// which adds the actual `temp_id` to the offset (the precheck only saw the
/// offset, so it could not have been the real bound anyway).
///
/// The `u16` adds are belt-and-braces alongside `absorb_non_gf`'s capacity
/// check: that one bounds the merged TABLE, this one bounds the id an
/// individual opcode carries, and a caller can reach this function with a base
/// the merger did not compute (`assemble_module`'s per-initial renumber loop
/// tracks its own running offsets).
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
    // recycle path's `temp_off` is always a small fixed base.
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
        SymbolicOpcode::EvalModule { id, n_inputs } => SymbolicOpcode::EvalModule {
            id: checked_add_u16(*id, mod_off, "ModuleId")?,
            n_inputs: *n_inputs,
        },
        SymbolicOpcode::PushStaticView { view_id } => SymbolicOpcode::PushStaticView {
            view_id: checked_add_u16(*view_id, view_off, "ViewId")?,
        },
        SymbolicOpcode::PushTempView {
            temp_id,
            dim_list_id,
        } => SymbolicOpcode::PushTempView {
            temp_id: checked_add_u8(*temp_id, temp_off_u8, "TempId")?,
            dim_list_id: checked_add_u16(*dim_list_id, dl_off, "DimListId")?,
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
        SymbolicOpcode::LoadTempDynamic { temp_id } => SymbolicOpcode::LoadTempDynamic {
            temp_id: checked_add_u8(*temp_id, temp_off_u8, "TempId")?,
        },
        SymbolicOpcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => SymbolicOpcode::BeginIter {
            write_temp_id: checked_add_u8(*write_temp_id, temp_off_u8, "TempId")?,
            has_write_temp: *has_write_temp,
        },
        SymbolicOpcode::LoadIterTempElement { temp_id } => SymbolicOpcode::LoadIterTempElement {
            temp_id: checked_add_u8(*temp_id, temp_off_u8, "TempId")?,
        },
        SymbolicOpcode::BeginBroadcastIter {
            n_sources,
            dest_temp_id,
        } => SymbolicOpcode::BeginBroadcastIter {
            n_sources: *n_sources,
            dest_temp_id: checked_add_u8(*dest_temp_id, temp_off_u8, "TempId")?,
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
        assert!(!resolved.is_temp);
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
        assert!(resolved.is_temp);
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
            if !sv.is_temp {
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
            arrays: vec![],
            dimensions: vec![],
            subdim_relations: vec![],
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
        assert!(!resolved.is_temp);
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
    // VariableLayout::from_offset_map coverage
    // ====================================================================

    #[test]
    fn test_layout_from_offset_map() {
        let mut offsets: HashMap<Ident<Canonical>, (usize, usize)> = HashMap::new();
        offsets.insert(Ident::new("alpha"), (0, 1));
        offsets.insert(Ident::new("beta"), (1, 3));

        let layout = VariableLayout::from_offset_map(&offsets, 4);
        assert_eq!(layout.n_slots, 4);

        let alpha = layout.get("alpha").unwrap();
        assert_eq!(alpha.offset, 0);
        assert_eq!(alpha.size, 1);

        let beta = layout.get("beta").unwrap();
        assert_eq!(beta.offset, 1);
        assert_eq!(beta.size, 3);

        assert!(layout.get("gamma").is_none());
    }

    // ====================================================================
    // renumber_opcode bounds checking (fix #5)
    // ====================================================================

    #[test]
    fn test_renumber_opcode_temp_offset_overflow() {
        let op = SymbolicOpcode::LoadTempDynamic { temp_id: 0 };
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
        let op = SymbolicOpcode::LoadTempDynamic { temp_id: 0 };
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

    // ====================================================================
    // concatenate_fragments with base offsets (fix #1)
    // ====================================================================

    #[test]
    fn test_concatenate_with_base_offsets() {
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
        let no_base = ContextResourceCounts::default();
        let merged_no_base = concatenate_fragments(&[&frag_a, &frag_b], &no_base).unwrap();
        assert_eq!(merged_no_base.graphical_functions.len(), 2);
        match &merged_no_base.bytecode.code[0] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 0),
            other => panic!("expected Lookup, got {:?}", other),
        }
        match &merged_no_base.bytecode.code[1] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 1),
            other => panic!("expected Lookup, got {:?}", other),
        }

        // GF numbering is INDEPENDENT of the (now GF-free) non-GF
        // `ctx_base` -- graphical functions are content-de-duplicated and
        // globally remapped, not flat-offset by a preceding-phase count
        // (#582). A non-default non-GF base (e.g. 5 preceding modules) must
        // NOT shift the GF indices.
        let base = ContextResourceCounts {
            modules: 5,
            ..ContextResourceCounts::default()
        };
        let merged_with_base = concatenate_fragments(&[&frag_a, &frag_b], &base).unwrap();
        match &merged_with_base.bytecode.code[0] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 0),
            other => panic!("expected Lookup, got {:?}", other),
        }
        match &merged_with_base.bytecode.code[1] {
            SymbolicOpcode::Lookup { base_gf, .. } => assert_eq!(*base_gf, 1),
            other => panic!("expected Lookup, got {:?}", other),
        }
    }

    #[test]
    fn test_resource_counts_from_fragments() {
        let frag = PerVarBytecodes {
            symbolic: SymbolicByteCode {
                literals: vec![1.0, 2.0, 3.0],
                code: vec![SymbolicOpcode::Ret],
            },
            // GF count is NOT a `ContextResourceCounts` field anymore (#582
            // dedup), so a GF here must not affect the flat counts below.
            graphical_functions: vec![vec![(0.0, 1.0)], vec![(1.0, 2.0)]],
            module_decls: vec![],
            static_views: vec![],
            temp_sizes: vec![(0, 4), (1, 8)],
            dim_lists: vec![vec![1, 2]],
        };

        let counts = ContextResourceCounts::from_fragments(&[&frag]);
        assert_eq!(counts.modules, 0);
        assert_eq!(counts.views, 0);
        assert_eq!(counts.temps, 2);
        assert_eq!(counts.dim_lists, 1);
    }

    #[test]
    fn test_resource_counts_sums_temps_across_fragments() {
        // Each fragment starts temps at 0; the total should be the sum,
        // not the max. Two fragments with temp_sizes [(0, 4)] each should
        // produce temps=2 (one slot per fragment), not temps=1 (max(0+1, 0+1)).
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
            static_views: vec![],
            temp_sizes: vec![(0, 4)],
            dim_lists: vec![],
        };

        let counts = ContextResourceCounts::from_fragments(&[&frag_a, &frag_b]);
        assert_eq!(
            counts.temps, 2,
            "temps should be sum of per-fragment counts, not max"
        );
    }

    #[test]
    fn test_concatenate_renumbers_static_view_temp_base() {
        // A static view whose base is a temp must be renumbered by the SAME
        // temp offset the recycle assigns the temp it points at. #583: the
        // plain-phase concat RECYCLES temps into one identity pool, so two
        // fragments' id-0 temps share slot 0 -- a `Temp(0)` static view base
        // stays `Temp(0)` (it tracks the recycled slot, NOT a per-fragment
        // sum). The view base shifts only by the fixed `ctx_base.temps`
        // recycle base, which is exercised below.
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

        // With the production plain-phase base (temps == 0), frag_b's Temp(0)
        // recycles to slot 0 -- the same slot frag_a's temp 0 occupies (max
        // size 8). The view base must stay Temp(0).
        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&[&frag_a, &frag_b], &no_base).unwrap();
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

        // A non-zero fixed recycle base (the `ctx_base.temps`) shifts every
        // fragment's temp ids -- including a static view's Temp base -- by
        // that base uniformly, proving the Temp-base renumber tracks the
        // recycle base (not a per-fragment running sum).
        let based = ContextResourceCounts {
            temps: 3,
            ..ContextResourceCounts::default()
        };
        let merged_based = concatenate_fragments(&[&frag_a, &frag_b], &based).unwrap();
        match &merged_based.static_views[0].base {
            SymStaticViewBase::Temp(id) => assert_eq!(
                *id, 3,
                "Temp(0) view base shifts by the fixed ctx_base.temps recycle base"
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
        // #583: the plain-phase concat RECYCLES temps into one identity pool,
        // so frag_b's id-0 write_temp_id recycles to slot 0 (not summed past
        // frag_a's temps). To keep this test's renumber NON-trivial -- so the
        // `full_source_len` survival assertion is load-bearing -- we drive
        // the concat with a fixed non-zero `ctx_base.temps` recycle base
        // (TEMP_BASE), which shifts every fragment's temp ids uniformly: a
        // legitimate exercise of the recycle renumber arithmetic.
        const TEMP_BASE: u32 = 2;
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

        let based = ContextResourceCounts {
            temps: TEMP_BASE,
            ..ContextResourceCounts::default()
        };
        let merged = concatenate_fragments(&[&frag_a, &frag_b], &based).unwrap();

        // Resolve to concrete bytecode against an empty layout: the
        // VectorElmMap opcode carries no variable reference.
        let empty_layout = VariableLayout::new(HashMap::new(), 0);
        let resolved = resolve_bytecode(&merged.bytecode, &empty_layout).unwrap();

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
        // the fixed recycle base TEMP_BASE shifts frag_b's write_temp_id 0 to
        // TEMP_BASE. (If this were 0, the merger never renumbered the opcode
        // and the full_source_len assertion below would prove nothing.)
        assert_eq!(
            elm_map.0, TEMP_BASE as u8,
            "write_temp_id must be offset by the fixed recycle base, proving \
             the fragment merger renumbered this opcode"
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
    // addressable, 65,537 is not -- and the tests below say so at each of the
    // three places a base is computed, because the first fix stated it once per
    // place and the three disagreed: the cross-phase and initials-phase
    // narrowings rejected a count of exactly 65,536 that the merger accepts.
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
        let err = concatenate_fragments(&[&a, &b], &ContextResourceCounts::default())
            .expect_err("a literal pool past u16 capacity must be reported, not wrapped");
        assert!(
            err.contains("literal") && err.contains("16-bit"),
            "expected a loud literal-capacity error, got: {err}"
        );

        // ...and one entry less is still fine, so the bound is exact rather than
        // conservative.
        let c = literal_pool_frag(1);
        concatenate_fragments(&[&a, &c], &ContextResourceCounts::default())
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
        concatenate_fragments(&[&a, &c, &empty], &ContextResourceCounts::default())
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

    /// The same bound, one level up: across PHASES, where the base is a
    /// preceding phase's count rather than a running merged length.
    ///
    /// The literal pool cannot reach this path -- literals are phase-local, so
    /// their `ctx_base` is always 0 -- which is why the exactness the test above
    /// pins did not extend to the three resources that DO carry a `ctx_base`.
    /// Those went through a separate narrowing (`ContextResourceCounts` held
    /// `u16` fields and `assemble_module` summed them with a checked add), and
    /// that narrowing rejected a count of exactly `U16_ID_CAPACITY` outright --
    /// disagreeing with `resource_base`, which accepts a table of exactly that
    /// size because every id in it is `<= u16::MAX`. A model whose initials
    /// filled the table and whose flows and stocks then named no dim list at all
    /// was rejected with every one of its ids valid.
    #[test]
    fn a_full_ctx_base_bounds_only_the_phases_that_use_it() {
        let cap = U16_ID_CAPACITY;
        let full = dim_list_frag(cap);
        let one = dim_list_frag(1);
        let none = dim_list_frag(0);

        // A phase count is just a count: filling the table exactly is legal, and
        // saying so is not the same as handing out an id past the end.
        let at_capacity = ContextResourceCounts::from_fragments(&[&full]);
        assert_eq!(at_capacity.dim_lists, cap);

        // A following phase that names no dim list assigns no id, so it merges.
        concatenate_fragments(&[&none], &at_capacity)
            .expect("a phase carrying no dim lists is not bounded by a full table");

        // One that names even a single dim list would need id 65,536.
        let err = concatenate_fragments(&[&one], &at_capacity)
            .expect_err("a dim list id past u16 capacity must be reported, not wrapped");
        assert!(
            err.contains("dimension list") && err.contains("16-bit"),
            "expected a loud dim-list-capacity error, got: {err}"
        );

        // ...and the bound is exact from below: one entry short of capacity, the
        // following phase's single entry is id 65,535 and merges.
        let nearly = ContextResourceCounts::from_fragments(&[&dim_list_frag(cap - 1)]);
        concatenate_fragments(&[&one], &nearly)
            .expect("the last addressable dim list id (u16::MAX) must merge");

        // The same exactness within one phase's merged table, so the two halves
        // of the bound are stated against the same resource.
        concatenate_fragments(
            &[&dim_list_frag(cap - 1), &one],
            &ContextResourceCounts::default(),
        )
        .expect("a merged table of exactly u16::MAX + 1 dim lists is addressable");
        concatenate_fragments(&[&full, &one], &ContextResourceCounts::default())
            .expect_err("one dim list past a full merged table must be reported");
        concatenate_fragments(&[&full, &none], &ContextResourceCounts::default())
            .expect("a fragment with no dim lists must not be bounded by a full table");
    }

    /// The initials phase tracks its own running offsets rather than going
    /// through the merger (each initial keeps its own bytecode, so each is
    /// renumbered at literal offset 0), and it has to agree with the merger
    /// about where the table ends.
    ///
    /// It did not: it narrowed each initial's count and its running total to
    /// `u16` eagerly, so an initials list that filled the table exactly was
    /// rejected the moment the LAST initial's count was folded in -- with
    /// nothing left to assign an id to.
    #[test]
    fn the_initials_phase_shares_the_mergers_capacity_bound() {
        let cap = U16_ID_CAPACITY;
        let full = dim_list_frag(cap);
        let one = dim_list_frag(1);
        let none = dim_list_frag(0);

        let run = |frags: &[&PerVarBytecodes]| -> Result<(), String> {
            let dedup = GfDedup::build(frags)?;
            let named: Vec<(String, &PerVarBytecodes)> = frags
                .iter()
                .enumerate()
                .map(|(i, f)| (format!("init{i}"), *f))
                .collect();
            crate::db::renumber_initials_phase(&named, &dedup).map(|_| ())
        };

        run(&[&full]).expect("initials that fill the dim-list table exactly are addressable");
        run(&[&full, &none]).expect("an initial naming no dim list is not bounded by a full table");
        let err = run(&[&full, &one])
            .expect_err("an initial needing dim list id 65,536 must be reported");
        assert!(
            err.contains("dimension list") && err.contains("16-bit"),
            "expected a loud dim-list-capacity error, got: {err}"
        );
    }

    /// The all-phases aggregation of the shared context tables must NOT bound
    /// the model's literal count, because it retains no literal pool.
    ///
    /// Each compiled initial keeps its own pool and the flows and stocks phases
    /// keep one each; the aggregation exists only for the module-decl /
    /// static-view / temp / dim-list tables those three phases' ids index. A
    /// full merge over every fragment additionally built a literal pool spanning
    /// the whole model and threw it away -- but not before `resource_base`
    /// bounded it, so a model whose every RETAINED pool was comfortably
    /// addressable stopped assembling once the three summed past capacity.
    #[test]
    fn the_all_phases_aggregation_does_not_bound_the_literal_pool() {
        let cap = U16_ID_CAPACITY;
        // Two fragments that together overrun the literal id space.
        let full = literal_pool_frag(cap);
        let one = literal_pool_frag(1);
        let frags = [&full, &one];
        let dedup = GfDedup::build(&frags).expect("no GFs to dedup");

        // Merging them into ONE stream assigns literal id 65,536 and is an
        // error -- that pool would be retained, so the bound is right.
        let err =
            concatenate_fragments_with_gf(&frags, &ContextResourceCounts::default(), &dedup, 0)
                .expect_err("a retained pool past capacity must be reported");
        assert!(
            err.contains("literal") && err.contains("16-bit"),
            "expected a loud literal-capacity error, got: {err}"
        );

        // Aggregating their context side-channels assigns no literal id at all,
        // so the same fragments are fine.
        let side = merge_context_side_channels(&frags, &ContextResourceCounts::default(), &dedup)
            .expect("the side-channel aggregation must not be bounded by a literal pool");
        assert!(
            side.module_decls.is_empty() && side.static_views.is_empty(),
            "these fragments carry no context resources; only literals"
        );
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

    /// M3 for the per-opcode add. `renumber_opcode` can be reached with a base the
    /// merger did not compute -- `assemble_module`'s per-initial loop tracks its
    /// own running module / view / dim-list offsets -- so the add is checked there
    /// too rather than relying on the merger's table bound alone.
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
        let op = SymbolicOpcode::LoadTempDynamic { temp_id: 100 };
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

        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&refs, &no_base)
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

        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&[&frag_a, &frag_b, &frag_c], &no_base).unwrap();

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

        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&[&fa, &fb, &fc], &no_base).unwrap();

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

        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&[&prefix, &overlap_frag], &no_base)
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
    // fragment's runlist segment, and `concatenate_fragments_with_gf` emits
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

        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&refs, &no_base).unwrap();

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

        let merged = concatenate_fragments(&refs, &ContextResourceCounts::default()).unwrap();
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
    fn test_concatenate_temp_recycle_agrees_across_phase_bases() {
        // The all-phases `merged` (no_base) and a later phase's concat (with
        // a non-zero non-temp ctx_base, as `flow_base`/`stock_base` carry)
        // must assign the SAME identity temp ids to the same fragment temps,
        // because temps recycle into ONE global identity pool whose ctx_base
        // temps offset is 0 for every phase. (Before #583 the per-phase
        // ctx_base.temps was re-added per fragment, so `flows_concat` and
        // `merged` disagreed -- the runtime OOB.)
        let frag = sort_order_temp_frag(0, 4);
        let refs: Vec<&PerVarBytecodes> = vec![&frag];

        let merged = concatenate_fragments(&refs, &ContextResourceCounts::default()).unwrap();
        // A phase base with preceding modules/views/dim_lists but temps left
        // to recycle globally (temps: 0).
        let phase_base = ContextResourceCounts {
            modules: 5,
            views: 3,
            temps: 0,
            dim_lists: 2,
        };
        let phase = concatenate_fragments(&refs, &phase_base).unwrap();

        let temp_write = |bc: &ConcatenatedBytecodes| -> TempId {
            bc.bytecode
                .code
                .iter()
                .find_map(|op| match op {
                    SymbolicOpcode::VectorSortOrder { write_temp_id } => Some(*write_temp_id),
                    _ => None,
                })
                .expect("a VectorSortOrder opcode")
        };
        assert_eq!(
            temp_write(&merged),
            temp_write(&phase),
            "the same fragment temp must get the same identity id in the \
             all-phases merge and a phase concat (temps recycle globally)"
        );
        assert_eq!(temp_write(&merged), 0, "identity recycle keeps id 0");
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
        let err = concatenate_fragments(&refs, &ContextResourceCounts::default())
            .expect_err("300 genuinely-distinct GF tables exceed u8 capacity");
        assert!(
            err.contains("distinct graphical function count")
                && err.contains("GraphicalFunctionId capacity"),
            "expected a loud distinct-GF-capacity error, got: {err}"
        );
    }
}
