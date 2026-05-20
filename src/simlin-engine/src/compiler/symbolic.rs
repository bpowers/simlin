// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Symbolic bytecode layer for layout-independent compilation.
//!
//! The existing compiler produces bytecodes with integer offsets that depend on
//! the model's variable layout. This module introduces a symbolic representation
//! where opcodes reference variables by name instead of offset. This decouples
//! per-variable compilation from the global layout, enabling salsa to cache
//! compiled fragments that remain valid even when variables are added or removed.
//!
//! The pipeline is: concrete bytecodes -> symbolize -> SymbolicByteCode -> resolve -> concrete bytecodes.

// These types and functions are used by the incremental compilation pipeline.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::bytecode::{
    BuiltinId, ByteCode, ByteCodeContext, CompiledInitial, CompiledModule, DimId, DimListId,
    GraphicalFunctionId, LiteralId, LookupMode, ModuleDeclaration, ModuleId, ModuleInputOffset,
    Op2, Opcode, PcOffset, RuntimeSparseMapping, StaticArrayView, TempId, VariableOffset, ViewId,
};
use crate::common::{Canonical, Ident};

// ============================================================================
// Types
// ============================================================================

/// Symbolic reference to a variable location within a model.
/// Replaces raw integer offsets with a variable name and element offset,
/// making the reference independent of the model's variable layout.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct SymVarRef {
    /// Canonical variable name
    pub name: String,
    /// Offset within the variable (0 for scalars, 0..size for array elements)
    pub element_offset: usize,
}

/// Symbolic version of `Opcode`. Identical structure except opcodes that
/// reference model variable offsets use `SymVarRef` instead of `VariableOffset`.
///
/// Opcodes that reference global implicit variables (time, dt, etc.) keep their
/// fixed offsets since those never change.
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
    AssignNext {
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
    PushVarView {
        var: SymVarRef,
        dim_list_id: DimListId,
    },
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
    ViewSubscriptConst {
        dim_idx: u8,
        index: u16,
    },
    ViewSubscriptDynamic {
        dim_idx: u8,
    },
    ViewRange {
        dim_idx: u8,
        start: u16,
        end: u16,
    },
    ViewRangeDynamic {
        dim_idx: u8,
    },
    ViewStarRange {
        dim_idx: u8,
        subdim_relation_id: u16,
    },
    ViewWildcard {
        dim_idx: u8,
    },
    ViewTranspose {},
    PopView {},
    DupView {},

    // === TEMP ARRAY ACCESS (unchanged) ===
    LoadTempConst {
        temp_id: TempId,
        index: u16,
    },
    LoadTempDynamic {
        temp_id: TempId,
    },

    // === ITERATION (unchanged) ===
    BeginIter {
        write_temp_id: TempId,
        has_write_temp: bool,
    },
    LoadIterElement {},
    LoadIterTempElement {
        temp_id: TempId,
    },
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
    BeginBroadcastIter {
        n_sources: u8,
        dest_temp_id: TempId,
    },
    LoadBroadcastElement {
        source_idx: u8,
    },
    StoreBroadcastElement {},
    NextBroadcastOrJump {
        jump_back: PcOffset,
    },
    EndBroadcastIter {},
}

/// Symbolic version of `ByteCode`. Contains the literal pool (unchanged)
/// and symbolic opcodes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolicByteCode {
    pub literals: Vec<f64>,
    pub code: Vec<SymbolicOpcode>,
}

/// Symbolic version of `StaticArrayView`. When the view refers to a model
/// variable (not a temp), `base_off` is replaced with a `SymVarRef`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolicStaticView {
    pub base: SymStaticViewBase,
    pub dims: SmallVec<[u16; 4]>,
    pub strides: SmallVec<[i32; 4]>,
    pub offset: u32,
    pub sparse: SmallVec<[RuntimeSparseMapping; 2]>,
    pub dim_ids: SmallVec<[DimId; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SymbolicCompiledModule {
    pub ident: Ident<Canonical>,
    pub n_slots: usize,
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

    /// Build from a Module's offset map for a specific model.
    #[allow(dead_code)]
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
}

// ============================================================================
// Reverse Offset Map (for symbolization)
// ============================================================================

/// Maps absolute variable offsets back to (variable_name, element_within_variable).
/// Used during symbolization to convert integer offsets to symbolic references.
pub(crate) struct ReverseOffsetMap {
    /// Indexed by offset. `entries[off] = Some((name, element_offset))`.
    entries: Vec<Option<(String, usize)>>,
}

impl ReverseOffsetMap {
    /// Build from a VariableLayout.
    pub(crate) fn from_layout(layout: &VariableLayout) -> Self {
        let mut entries: Vec<Option<(String, usize)>> = vec![None; layout.n_slots];
        for (name, entry) in &layout.entries {
            for elem in 0..entry.size {
                let off = entry.offset + elem;
                if off < entries.len() {
                    entries[off] = Some((name.clone(), elem));
                }
            }
        }
        ReverseOffsetMap { entries }
    }

    /// Look up a variable offset.
    fn lookup(&self, off: u32) -> Result<SymVarRef, String> {
        let idx = off as usize;
        if idx >= self.entries.len() {
            return Err(format!(
                "offset {} out of range (max {})",
                off,
                self.entries.len()
            ));
        }
        match &self.entries[idx] {
            Some((name, elem)) => Ok(SymVarRef {
                name: name.clone(),
                element_offset: *elem,
            }),
            None => Err(format!("no variable mapped at offset {}", off)),
        }
    }
}

// ============================================================================
// Layout Construction
// ============================================================================

/// Build a `VariableLayout` from the metadata produced by `build_metadata()`.
///
/// The metadata map is `model_name -> (variable_name -> VariableMetadata)`.
/// This extracts the layout for a single model.
pub(crate) fn layout_from_metadata(
    metadata: &HashMap<Ident<Canonical>, HashMap<Ident<Canonical>, super::VariableMetadata<'_>>>,
    model_name: &Ident<Canonical>,
) -> Result<VariableLayout, String> {
    let model_metadata = metadata.get(model_name).ok_or_else(|| {
        format!(
            "model '{}' not found in metadata during layout construction",
            model_name.as_str()
        )
    })?;
    let mut entries = HashMap::with_capacity(model_metadata.len());
    let mut n_slots = 0;

    for (name, meta) in model_metadata {
        entries.insert(
            name.to_string(),
            LayoutEntry {
                offset: meta.offset,
                size: meta.size,
            },
        );
        n_slots = n_slots.max(meta.offset + meta.size);
    }

    Ok(VariableLayout::new(entries, n_slots))
}

// ============================================================================
// Symbolize: Concrete -> Symbolic
// ============================================================================

pub(crate) fn symbolize_opcode(
    op: &Opcode,
    rmap: &ReverseOffsetMap,
) -> Result<SymbolicOpcode, String> {
    match op {
        // Opcodes with variable offsets that need symbolization
        Opcode::LoadVar { off } => Ok(SymbolicOpcode::LoadVar {
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::LoadPrev { off } => Ok(SymbolicOpcode::SymLoadPrev {
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::LoadInitial { off } => Ok(SymbolicOpcode::SymLoadInitial {
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::LoadSubscript { off } => Ok(SymbolicOpcode::LoadSubscript {
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::AssignCurr { off } => Ok(SymbolicOpcode::AssignCurr {
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::AssignNext { off } => Ok(SymbolicOpcode::AssignNext {
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::AssignConstCurr { off, literal_id } => Ok(SymbolicOpcode::AssignConstCurr {
            var: rmap.lookup(u32::from(*off))?,
            literal_id: *literal_id,
        }),
        Opcode::BinOpAssignCurr { op, off } => Ok(SymbolicOpcode::BinOpAssignCurr {
            op: *op,
            var: rmap.lookup(u32::from(*off))?,
        }),
        Opcode::BinOpAssignNext { op, off } => Ok(SymbolicOpcode::BinOpAssignNext {
            op: *op,
            var: rmap.lookup(u32::from(*off))?,
        }),
        // The 3-address fused binops AND the fused leaf assignments are created
        // by `ByteCode::fuse_three_address`, which runs only on FINAL concrete
        // bytecode (after `resolve`), strictly after symbolization, and only on
        // the Vm's private execution copy (never the salsa-cached
        // CompiledSimulation). They therefore never reach this function; seeing
        // one means the fusion ran before symbolize, which is a compiler bug.
        // The exhaustive match here is the guarantee no fused opcode can silently
        // leak into the symbolic/incremental layer.
        Opcode::BinVarVar { .. }
        | Opcode::BinVarConst { .. }
        | Opcode::BinConstVar { .. }
        | Opcode::BinStackVar { .. }
        | Opcode::BinStackConst { .. }
        | Opcode::AssignAddVarVarCurr { .. }
        | Opcode::AssignSubVarVarCurr { .. }
        | Opcode::AssignMulVarVarCurr { .. }
        | Opcode::AssignDivVarVarCurr { .. }
        | Opcode::AssignAddVarVarNext { .. }
        | Opcode::AssignSubVarVarNext { .. }
        | Opcode::AssignMulVarVarNext { .. }
        | Opcode::AssignDivVarVarNext { .. }
        | Opcode::AssignAddVarConstCurr { .. }
        | Opcode::AssignSubVarConstCurr { .. }
        | Opcode::AssignMulVarConstCurr { .. }
        | Opcode::AssignDivVarConstCurr { .. }
        | Opcode::AssignAddVarConstNext { .. }
        | Opcode::AssignSubVarConstNext { .. }
        | Opcode::AssignMulVarConstNext { .. }
        | Opcode::AssignDivVarConstNext { .. }
        | Opcode::AssignAddConstVarCurr { .. }
        | Opcode::AssignSubConstVarCurr { .. }
        | Opcode::AssignMulConstVarCurr { .. }
        | Opcode::AssignDivConstVarCurr { .. }
        | Opcode::AssignAddConstVarNext { .. }
        | Opcode::AssignSubConstVarNext { .. }
        | Opcode::AssignMulConstVarNext { .. }
        | Opcode::AssignDivConstVarNext { .. }
        | Opcode::AssignStackVarCurr { .. }
        | Opcode::AssignStackVarNext { .. }
        | Opcode::AssignStackConstCurr { .. }
        | Opcode::AssignStackConstNext { .. } => {
            unreachable!("3-address fused opcode reached symbolize_opcode")
        }
        Opcode::PushVarView {
            base_off,
            dim_list_id,
        } => Ok(SymbolicOpcode::PushVarView {
            var: rmap.lookup(u32::from(*base_off))?,
            dim_list_id: *dim_list_id,
        }),
        Opcode::PushVarViewDirect {
            base_off,
            dim_list_id,
        } => Ok(SymbolicOpcode::PushVarViewDirect {
            var: rmap.lookup(u32::from(*base_off))?,
            dim_list_id: *dim_list_id,
        }),

        // Opcodes that are identical in symbolic form
        Opcode::Op2 { op } => Ok(SymbolicOpcode::Op2 { op: *op }),
        Opcode::Not {} => Ok(SymbolicOpcode::Not {}),
        Opcode::LoadConstant { id } => Ok(SymbolicOpcode::LoadConstant { id: *id }),
        Opcode::LoadGlobalVar { off } => Ok(SymbolicOpcode::LoadGlobalVar { off: *off }),
        Opcode::PushSubscriptIndex { bounds } => {
            Ok(SymbolicOpcode::PushSubscriptIndex { bounds: *bounds })
        }
        Opcode::SetCond {} => Ok(SymbolicOpcode::SetCond {}),
        Opcode::If {} => Ok(SymbolicOpcode::If {}),
        Opcode::Ret => Ok(SymbolicOpcode::Ret),
        Opcode::LoadModuleInput { input } => Ok(SymbolicOpcode::LoadModuleInput { input: *input }),
        Opcode::EvalModule { id, n_inputs } => Ok(SymbolicOpcode::EvalModule {
            id: *id,
            n_inputs: *n_inputs,
        }),
        Opcode::Apply { func } => Ok(SymbolicOpcode::Apply { func: *func }),
        Opcode::Lookup {
            base_gf,
            table_count,
            mode,
        } => Ok(SymbolicOpcode::Lookup {
            base_gf: *base_gf,
            table_count: *table_count,
            mode: *mode,
        }),
        Opcode::PushTempView {
            temp_id,
            dim_list_id,
        } => Ok(SymbolicOpcode::PushTempView {
            temp_id: *temp_id,
            dim_list_id: *dim_list_id,
        }),
        Opcode::PushStaticView { view_id } => {
            Ok(SymbolicOpcode::PushStaticView { view_id: *view_id })
        }
        Opcode::ViewSubscriptConst { dim_idx, index } => Ok(SymbolicOpcode::ViewSubscriptConst {
            dim_idx: *dim_idx,
            index: *index,
        }),
        Opcode::ViewSubscriptDynamic { dim_idx } => {
            Ok(SymbolicOpcode::ViewSubscriptDynamic { dim_idx: *dim_idx })
        }
        Opcode::ViewRange {
            dim_idx,
            start,
            end,
        } => Ok(SymbolicOpcode::ViewRange {
            dim_idx: *dim_idx,
            start: *start,
            end: *end,
        }),
        Opcode::ViewRangeDynamic { dim_idx } => {
            Ok(SymbolicOpcode::ViewRangeDynamic { dim_idx: *dim_idx })
        }
        Opcode::ViewStarRange {
            dim_idx,
            subdim_relation_id,
        } => Ok(SymbolicOpcode::ViewStarRange {
            dim_idx: *dim_idx,
            subdim_relation_id: *subdim_relation_id,
        }),
        Opcode::ViewWildcard { dim_idx } => Ok(SymbolicOpcode::ViewWildcard { dim_idx: *dim_idx }),
        Opcode::ViewTranspose {} => Ok(SymbolicOpcode::ViewTranspose {}),
        Opcode::PopView {} => Ok(SymbolicOpcode::PopView {}),
        Opcode::DupView {} => Ok(SymbolicOpcode::DupView {}),
        Opcode::LoadTempConst { temp_id, index } => Ok(SymbolicOpcode::LoadTempConst {
            temp_id: *temp_id,
            index: *index,
        }),
        Opcode::LoadTempDynamic { temp_id } => {
            Ok(SymbolicOpcode::LoadTempDynamic { temp_id: *temp_id })
        }
        Opcode::BeginIter {
            write_temp_id,
            has_write_temp,
        } => Ok(SymbolicOpcode::BeginIter {
            write_temp_id: *write_temp_id,
            has_write_temp: *has_write_temp,
        }),
        Opcode::LoadIterElement {} => Ok(SymbolicOpcode::LoadIterElement {}),
        Opcode::LoadIterTempElement { temp_id } => {
            Ok(SymbolicOpcode::LoadIterTempElement { temp_id: *temp_id })
        }
        Opcode::LoadIterViewTop {} => Ok(SymbolicOpcode::LoadIterViewTop {}),
        Opcode::LoadIterViewAt { offset } => Ok(SymbolicOpcode::LoadIterViewAt { offset: *offset }),
        Opcode::StoreIterElement {} => Ok(SymbolicOpcode::StoreIterElement {}),
        Opcode::NextIterOrJump { jump_back } => Ok(SymbolicOpcode::NextIterOrJump {
            jump_back: *jump_back,
        }),
        Opcode::EndIter {} => Ok(SymbolicOpcode::EndIter {}),
        Opcode::ArraySum {} => Ok(SymbolicOpcode::ArraySum {}),
        Opcode::ArrayMax {} => Ok(SymbolicOpcode::ArrayMax {}),
        Opcode::ArrayMin {} => Ok(SymbolicOpcode::ArrayMin {}),
        Opcode::ArrayMean {} => Ok(SymbolicOpcode::ArrayMean {}),
        Opcode::ArrayStddev {} => Ok(SymbolicOpcode::ArrayStddev {}),
        Opcode::ArraySize {} => Ok(SymbolicOpcode::ArraySize {}),
        Opcode::VectorSelect {} => Ok(SymbolicOpcode::VectorSelect {}),
        Opcode::VectorElmMap {
            write_temp_id,
            full_source_len,
        } => Ok(SymbolicOpcode::VectorElmMap {
            write_temp_id: *write_temp_id,
            full_source_len: *full_source_len,
        }),
        Opcode::VectorSortOrder { write_temp_id } => Ok(SymbolicOpcode::VectorSortOrder {
            write_temp_id: *write_temp_id,
        }),
        Opcode::Rank { write_temp_id } => Ok(SymbolicOpcode::Rank {
            write_temp_id: *write_temp_id,
        }),
        Opcode::LookupArray {
            base_gf,
            table_count,
            mode,
            write_temp_id,
        } => Ok(SymbolicOpcode::LookupArray {
            base_gf: *base_gf,
            table_count: *table_count,
            mode: *mode,
            write_temp_id: *write_temp_id,
        }),
        Opcode::AllocateAvailable { write_temp_id } => Ok(SymbolicOpcode::AllocateAvailable {
            write_temp_id: *write_temp_id,
        }),
        Opcode::AllocateByPriority { write_temp_id } => Ok(SymbolicOpcode::AllocateByPriority {
            write_temp_id: *write_temp_id,
        }),
        Opcode::BeginBroadcastIter {
            n_sources,
            dest_temp_id,
        } => Ok(SymbolicOpcode::BeginBroadcastIter {
            n_sources: *n_sources,
            dest_temp_id: *dest_temp_id,
        }),
        Opcode::LoadBroadcastElement { source_idx } => Ok(SymbolicOpcode::LoadBroadcastElement {
            source_idx: *source_idx,
        }),
        Opcode::StoreBroadcastElement {} => Ok(SymbolicOpcode::StoreBroadcastElement {}),
        Opcode::NextBroadcastOrJump { jump_back } => Ok(SymbolicOpcode::NextBroadcastOrJump {
            jump_back: *jump_back,
        }),
        Opcode::EndBroadcastIter {} => Ok(SymbolicOpcode::EndBroadcastIter {}),
    }
}

pub(crate) fn symbolize_static_view(
    view: &StaticArrayView,
    rmap: &ReverseOffsetMap,
) -> Result<SymbolicStaticView, String> {
    let base = if view.is_temp {
        SymStaticViewBase::Temp(view.base_off)
    } else {
        SymStaticViewBase::Var(rmap.lookup(view.base_off)?)
    };

    Ok(SymbolicStaticView {
        base,
        dims: view.dims.clone(),
        strides: view.strides.clone(),
        offset: view.offset,
        sparse: view.sparse.clone(),
        dim_ids: view.dim_ids.clone(),
    })
}

pub(crate) fn symbolize_module_decl(
    decl: &ModuleDeclaration,
    rmap: &ReverseOffsetMap,
) -> Result<SymbolicModuleDecl, String> {
    let off = u32::try_from(decl.off)
        .map_err(|_| format!("module declaration offset {} does not fit in u32", decl.off))?;
    Ok(SymbolicModuleDecl {
        model_name: decl.model_name.clone(),
        input_set: decl.input_set.clone(),
        var: rmap.lookup(off)?,
    })
}

pub(crate) fn symbolize_bytecode(
    bc: &ByteCode,
    rmap: &ReverseOffsetMap,
) -> Result<SymbolicByteCode, String> {
    let code = bc
        .code
        .iter()
        .map(|op| symbolize_opcode(op, rmap))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SymbolicByteCode {
        literals: bc.literals.clone(),
        code,
    })
}

/// Convert a `CompiledModule` to its symbolic representation.
/// All variable offsets are replaced with symbolic references using the layout.
#[allow(dead_code)]
pub(crate) fn symbolize_module(
    module: &CompiledModule,
    layout: &VariableLayout,
) -> Result<SymbolicCompiledModule, String> {
    let rmap = ReverseOffsetMap::from_layout(layout);

    let compiled_initials = module
        .compiled_initials
        .iter()
        .map(|ci| {
            Ok(SymbolicCompiledInitial {
                ident: ci.ident.clone(),
                bytecode: symbolize_bytecode(&ci.bytecode, &rmap)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let compiled_flows = symbolize_bytecode(&module.compiled_flows, &rmap)?;
    let compiled_stocks = symbolize_bytecode(&module.compiled_stocks, &rmap)?;

    let ctx = &*module.context;

    let static_views = ctx
        .static_views
        .iter()
        .map(|sv| symbolize_static_view(sv, &rmap))
        .collect::<Result<Vec<_>, _>>()?;

    let module_decls = ctx
        .modules
        .iter()
        .map(|md| symbolize_module_decl(md, &rmap))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(SymbolicCompiledModule {
        ident: module.ident.clone(),
        n_slots: module.n_slots,
        compiled_initials,
        compiled_flows,
        compiled_stocks,
        graphical_functions: ctx.graphical_functions.clone(),
        module_decls,
        static_views,
        arrays: ctx.arrays.clone(),
        dimensions: ctx.dimensions.clone(),
        subdim_relations: ctx.subdim_relations.clone(),
        names: ctx.names.clone(),
        temp_offsets: ctx.temp_offsets.clone(),
        temp_total_size: ctx.temp_total_size,
        dim_lists: ctx.dim_lists.clone(),
    })
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
        | SymbolicOpcode::AssignNext { var }
        | SymbolicOpcode::AssignConstCurr { var, .. }
        | SymbolicOpcode::BinOpAssignCurr { var, .. }
        | SymbolicOpcode::BinOpAssignNext { var, .. }
        | SymbolicOpcode::PushVarView { var, .. }
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
        if decls.iter().any(|d| layout.get(&d.var.name).is_none()) {
            return false;
        }
    }
    true
}

// ============================================================================
// Resolve: Symbolic -> Concrete (Assembly)
// ============================================================================

pub(crate) fn resolve_var_ref(
    var: &SymVarRef,
    layout: &VariableLayout,
) -> Result<VariableOffset, String> {
    let entry = layout.get(&var.name).ok_or_else(|| {
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
    Ok(off as VariableOffset)
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
        SymbolicOpcode::AssignNext { var } => Ok(Opcode::AssignNext {
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
        SymbolicOpcode::PushVarView { var, dim_list_id } => Ok(Opcode::PushVarView {
            base_off: resolve_var_ref(var, layout)?,
            dim_list_id: *dim_list_id,
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
            // Roundtrip coverage of this invariant lives in the unit tests,
            // NOT the end-to-end simulate gates, and intentionally so:
            // `vm_vector_elm_map` only consumes `full_source_len` for those
            // two purposes, and the genuine-Vensim corpus
            // (`vector_simple.dat` / `vector.dat`) deliberately has no
            // out-of-range offset and no shape that flips the full-array
            // branch, so an inflated `full_source_len` is behaviorally
            // invisible through `simulates_vector_simple_mdl` /
            // `simulates_vector_xmile_genuine` (verified: those gates pass
            // even with `full_source_len` hard-forced to a wrong constant in
            // `renumber_opcode`). The authoritative regression coverage is
            // therefore the symbolic path itself:
            // `test_renumber_vector_builtin_temp_ids` (isolated
            // `renumber_opcode`) and
            // `test_vector_elm_map_full_source_len_survives_fragment_roundtrip`
            // (the full `symbolize` -> `concatenate_fragments` (renumber at a
            // non-zero temp offset) -> `resolve_bytecode` merge path), which
            // fail loudly if this field is ever shifted.
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

pub(crate) fn resolve_bytecode(
    sbc: &SymbolicByteCode,
    layout: &VariableLayout,
) -> Result<ByteCode, String> {
    let code = sbc
        .code
        .iter()
        .map(|op| resolve_opcode(op, layout))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ByteCode {
        literals: sbc.literals.clone(),
        code,
    })
}

pub(crate) fn resolve_static_view(
    sv: &SymbolicStaticView,
    layout: &VariableLayout,
) -> Result<StaticArrayView, String> {
    let (base_off, is_temp) = match &sv.base {
        SymStaticViewBase::Var(var_ref) => {
            let entry = layout.get(&var_ref.name).ok_or_else(|| {
                format!(
                    "variable '{}' not found in layout during static view resolution",
                    var_ref.name
                )
            })?;
            ((entry.offset + var_ref.element_offset) as u32, false)
        }
        SymStaticViewBase::Temp(id) => (*id, true),
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
    let entry = layout.get(&sd.var.name).ok_or_else(|| {
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

    // `resolve_module` is a pure symbolic<->concrete primitive (the roundtrip
    // tests symbolize its output again), so the 3-address fusion (R2) is NOT
    // applied here -- the production assembler `assemble_module` applies it to
    // this function's output instead, where the result is never re-symbolized.
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
#[derive(Clone, Debug, Default)]
pub(crate) struct ContextResourceCounts {
    pub modules: u16,
    pub views: u16,
    pub temps: u32,
    pub dim_lists: u16,
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
    pub fn from_fragments(fragments: &[&PerVarBytecodes]) -> Self {
        let mut counts = ContextResourceCounts::default();
        for frag in fragments {
            counts.modules += frag.module_decls.len() as u16;
            counts.views += frag.static_views.len() as u16;
            // Each fragment's temps start at 0, so the disjoint-layout total
            // is the sum of each fragment's (max_id + 1), not the global max.
            let frag_temp_count = frag
                .temp_sizes
                .iter()
                .map(|(id, _)| *id + 1)
                .max()
                .unwrap_or(0);
            counts.temps += frag_temp_count;
            counts.dim_lists += frag.dim_lists.len() as u16;
        }
        counts
    }
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
/// the variable's runlist segment completes. The two consumers differ in
/// whether their fragments' temp live ranges can overlap:
///
/// - `Recycle` (plain-phase `concatenate_fragments`): fragments are emitted
///   as sequential, non-overlapping runlist segments, so two fragments'
///   temps are never simultaneously live. They are max-merged into ONE
///   shared identity pool keyed by temp id -- variable A's temp 0 and
///   variable B's temp 0 collapse to global slot 0, the slot's size the max
///   of the two. This exactly matches the monolithic `Module::compile` keyed
///   max-merge (`compiler/mod.rs`), so the incremental temp count equals the
///   monolithic `n_temps` instead of summing to a count that overflows the
///   `TempId` (= `u8`) namespace.
///
/// - `Sum` (`combine_scc_fragment`): the combined SCC fragment INTERLEAVES
///   its members' per-element segments per `element_order`, so members' temp
///   live ranges OVERLAP. Each member's temps must get a DISJOINT id range
///   (advancing per member) -- recycling them onto a shared slot would make
///   two simultaneously-live temps alias and silently miscompile the SCC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TempStrategy {
    /// Max-merge fragment temps into one identity-keyed pool (plain-phase
    /// concat; matches monolithic recycling).
    Recycle,
    /// Advance a disjoint temp id range per fragment (combined SCC fragment;
    /// interleaved segments need non-overlapping live ranges).
    Sum,
}

/// Running merge state for combining `PerVarBytecodes` into a single
/// resource namespace.
///
/// This is the shared core of `concatenate_fragments` and the
/// per-element-granular `combine_scc_fragment` (a multi-member recurrence
/// SCC's combined fragment). The accounting that must hold across both --
/// every fragment's literals/GFs/modules/views/dim-lists land in a
/// disjoint, non-colliding ID range -- is implemented exactly once here so
/// the two consumers cannot drift. Temps are the one resource whose layout
/// differs: see `TempStrategy`. `concatenate_fragments` absorbs each
/// fragment and immediately renumbers its whole (Ret-stripped) code;
/// `combine_scc_fragment` absorbs each *member* once and renumbers that
/// member's per-element segments with the member's offsets, emitting the
/// segments in the SCC's interleaved `element_order`.
///
/// `ctx_base` provides context resource ID offsets inherited from
/// preceding phases. Literal IDs are always phase-local (each phase's
/// bytecode has its own literal pool) so they are not affected by
/// `ctx_base`. Temps recycle into ONE global identity pool, so their
/// `ctx_base.temps` is 0 for every phase (the pool is not partitioned by
/// phase). When assembling a single phase in isolation, pass
/// `ContextResourceCounts::default()`.
pub(crate) struct FragmentMerger {
    ctx_base: ContextResourceCounts,
    temp_strategy: TempStrategy,
    merged_literals: Vec<f64>,
    merged_gf: Vec<Vec<(f64, f64)>>,
    /// Cross-fragment graphical-function de-duplication index (#582). Maps a
    /// GF *block* -- a maximal contiguous run of one or more lookup tables,
    /// the granularity the monolithic `Compiler::new` lays out one-per-
    /// variable -- keyed by its bit-exact content, to the global `merged_gf`
    /// offset its first occurrence was appended at. A dependency arrayed GF
    /// referenced by N consumer fragments produces N fragments each carrying
    /// the *same* block (every consumer re-extracts the dependency's
    /// `Vec<Table>` -- see `db_var_fragment.rs`); de-duplicating the block
    /// appends it once and remaps every consumer's `base_gf` by the single
    /// shared offset, matching the monolithic layout.
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
/// Each `Lookup`/`LookupArray` `base_gf` addresses a run of `table_count`
/// tables starting at `base_gf`, and `base_gf` is always an originating
/// variable's block start (the monolithic `Compiler::new` only ever emits a
/// `base_gf` from its one-per-variable `table_base_ids` map). Within a
/// fragment one such run can be *nested* inside another: a per-element
/// arrayed GF `g` is read both as the whole array (`LookupArray { base, |D|
/// }`) and at one element (`Lookup { base + e, 1 }` -- fully inside the
/// array's range). The whole-array run is the real block; the nested
/// element run is a sub-reference, NOT a separate block (splitting the
/// block at the element boundary could scatter the array across the deduped
/// table and miscompile the `[base .. base + table_count]` array lookup).
/// Distinct variables' blocks are laid out *disjointly* by `Compiler::new`,
/// so opcode runs are only ever disjoint or nested -- never partially
/// overlapping. The blocks are therefore the *maximal-by-inclusion* opcode
/// runs (nested runs dropped), plus one block per maximal *un-referenced*
/// gap (over-collected dependency tables `db_var_fragment.rs` gathered but
/// no opcode reads -- never read, so an imperfect gap boundary cannot
/// miscompile, only mildly affect the deduped count).
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

    /// New merger with an explicit temp strategy. `concatenate_fragments`
    /// (plain-phase, sequential segments) uses `TempStrategy::Recycle` to
    /// match the monolithic keyed max-merge; the SCC path uses `Sum`.
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
        let off = self.absorb_non_gf(frag);
        let gf_remap = self.absorb_gf(frag)?;
        Ok((off, gf_remap))
    }

    /// Absorb one fragment's flat (non-GF) side-channels -- literals,
    /// modules, views, temp sizes, dim lists -- into the running merge
    /// state and return the five flat resource base offsets. The literal /
    /// module / view / dim-list offsets are computed from the *pre-merge*
    /// lengths (each is a distinct resource, laid out disjointly), then those
    /// side-channels are appended (`Temp`-based static views shifted by this
    /// fragment's `temp_offset`, dim-lists truncated to 4). The *temp* offset
    /// instead follows `temp_strategy` (#583): `Sum` advances per fragment
    /// (disjoint ranges, for `combine_scc_fragment`'s interleaved segments);
    /// `Recycle` uses the fixed `ctx_base.temps` so every fragment's temps
    /// max-merge into one identity pool (plain-phase concat, matching the
    /// monolithic keyed max-merge). Graphical functions are handled
    /// separately by `absorb_gf` (content-de-duplicated, #582).
    pub(crate) fn absorb_non_gf(&mut self, frag: &PerVarBytecodes) -> FragmentResourceOffsets {
        // Literals are phase-local; no ctx_base offset needed. Modules,
        // views, and dim-lists are appended unshifted, so their offset is
        // `ctx_base + cumulative_appended` (no double-count: the appended
        // entries do NOT carry the ctx_base, so `merged_X.len()` excludes
        // it). Temps are different: see below.
        let lit_offset = self.merged_literals.len() as u16;
        let mod_offset = self.merged_modules.len() as u16 + self.ctx_base.modules;
        let view_offset = self.merged_views.len() as u16 + self.ctx_base.views;
        // #583: temps recycle (plain-phase) or sum (interleaved SCC).
        //
        // `Recycle`: a FIXED base (`ctx_base.temps`, which is 0 for every
        //   plain phase since temps share ONE global identity pool). The
        //   per-fragment max-merge below places fragment temp id `t` at slot
        //   `t + base`, so every fragment's id 0 collapses to the same slot
        //   -- the monolithic keyed max-merge.
        // `Sum`: advance by the running pool length so each fragment gets a
        //   disjoint range (interleaved SCC segments need non-overlapping
        //   live ranges). NOTE the previous unconditional
        //   `merged_temp_sizes.len() + ctx_base.temps` double-counted
        //   `ctx_base.temps`: temps are stored at `id + temp_offset` (which
        //   already includes the base), so `merged_temp_sizes.len()` absorbs
        //   the base -- adding it again diverged `flows_concat` from the
        //   all-phases `merged` table. The recycle path's fixed base removes
        //   that divergence; the Sum path runs only with `ctx_base.temps == 0`
        //   (`combine_scc_fragment` passes a default ctx_base).
        let temp_offset = match self.temp_strategy {
            TempStrategy::Recycle => self.ctx_base.temps,
            TempStrategy::Sum => self.merged_temp_sizes.len() as u32 + self.ctx_base.temps,
        };
        let dl_offset = self.merged_dim_lists.len() as u16 + self.ctx_base.dim_lists;

        self.merged_literals
            .extend_from_slice(&frag.symbolic.literals);
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

        FragmentResourceOffsets {
            lit_offset,
            mod_offset,
            view_offset,
            temp_offset,
            dl_offset,
        }
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
    fn into_concatenated(self, mut code: Vec<SymbolicOpcode>) -> ConcatenatedBytecodes {
        if !code.is_empty() {
            code.push(SymbolicOpcode::Ret);
        }

        let mut temp_offsets = Vec::with_capacity(self.merged_temp_sizes.len());
        let mut offset = 0usize;
        for &size in &self.merged_temp_sizes {
            temp_offsets.push(offset);
            offset += size;
        }

        ConcatenatedBytecodes {
            bytecode: SymbolicByteCode {
                literals: self.merged_literals,
                code,
            },
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
    /// De-duplicate the GF table lists of `fragments` (in order) by
    /// bit-exact content, matching the monolithic `Compiler::new`'s
    /// one-list-per-variable layout. Value-exact: genuinely-different lists
    /// never share an offset. `Err` if the *distinct* GF count exceeds
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
/// The non-GF resource accounting is byte-for-byte the original
/// `concatenate_fragments` loop -- only the GF base now comes from the
/// shared deduped remap instead of a flat `gf_off`, so the output is
/// identical to before for any model whose GF table lists were already
/// distinct.
pub(crate) fn concatenate_fragments_with_gf(
    fragments: &[&PerVarBytecodes],
    ctx_base: &ContextResourceCounts,
    dedup: &GfDedup,
    gf_index_base: usize,
) -> Result<ConcatenatedBytecodes, String> {
    // Plain-phase concat: temps RECYCLE into one identity pool (matching the
    // monolithic keyed max-merge), since fragments are sequential, non-
    // overlapping runlist segments. `combine_scc_fragment` (interleaved
    // segments) uses the disjoint `Sum` path instead.
    let mut merger = FragmentMerger::new_with_temp_strategy(ctx_base, TempStrategy::Recycle);
    let mut merged_code: Vec<SymbolicOpcode> = Vec::new();

    for (i, frag) in fragments.iter().enumerate() {
        // Only the flat resources are merged here; GF numbering comes from
        // the shared `dedup` so it is coherent across phases.
        let off = merger.absorb_non_gf(frag);
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
/// Returns `Err` if a per-opcode temp id would overflow `TempId` (= `u8`)
/// after offsetting (the `checked_add_u8` below) or if a `base_gf` is out of
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
        SymbolicOpcode::LoadConstant { id } => SymbolicOpcode::LoadConstant { id: *id + lit_off },
        SymbolicOpcode::AssignConstCurr { var, literal_id } => SymbolicOpcode::AssignConstCurr {
            var: var.clone(),
            literal_id: *literal_id + lit_off,
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
            id: *id + mod_off,
            n_inputs: *n_inputs,
        },
        SymbolicOpcode::PushStaticView { view_id } => SymbolicOpcode::PushStaticView {
            view_id: *view_id + view_off,
        },
        SymbolicOpcode::PushTempView {
            temp_id,
            dim_list_id,
        } => SymbolicOpcode::PushTempView {
            temp_id: checked_add_u8(*temp_id, temp_off_u8, "TempId")?,
            dim_list_id: *dim_list_id + dl_off,
        },
        SymbolicOpcode::PushVarView { var, dim_list_id } => SymbolicOpcode::PushVarView {
            var: var.clone(),
            dim_list_id: *dim_list_id + dl_off,
        },
        SymbolicOpcode::PushVarViewDirect { var, dim_list_id } => {
            SymbolicOpcode::PushVarViewDirect {
                var: var.clone(),
                dim_list_id: *dim_list_id + dl_off,
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::Op2;

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

    #[test]
    fn test_reverse_offset_map_basic() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let var = rmap.lookup(4).unwrap();
        assert_eq!(var.name, "births");
        assert_eq!(var.element_offset, 0);

        let var = rmap.lookup(5).unwrap();
        assert_eq!(var.name, "population");
        assert_eq!(var.element_offset, 0);
    }

    #[test]
    fn test_reverse_offset_map_array() {
        let mut entries = HashMap::new();
        entries.insert("arr".to_string(), LayoutEntry { offset: 4, size: 3 });
        let layout = VariableLayout::new(entries, 7);
        let rmap = ReverseOffsetMap::from_layout(&layout);

        assert_eq!(rmap.lookup(4).unwrap().element_offset, 0);
        assert_eq!(rmap.lookup(5).unwrap().element_offset, 1);
        assert_eq!(rmap.lookup(6).unwrap().element_offset, 2);
        assert_eq!(rmap.lookup(4).unwrap().name, "arr");
    }

    #[test]
    fn test_reverse_offset_map_out_of_range() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);
        assert!(rmap.lookup(99).is_err());
    }

    #[test]
    fn test_symbolize_load_var() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let op = Opcode::LoadVar { off: 5 };
        let sym = symbolize_opcode(&op, &rmap).unwrap();
        assert_eq!(
            sym,
            SymbolicOpcode::LoadVar {
                var: SymVarRef {
                    name: "population".to_string(),
                    element_offset: 0
                }
            }
        );
    }

    #[test]
    fn test_symbolize_assign_curr() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let op = Opcode::AssignCurr { off: 4 };
        let sym = symbolize_opcode(&op, &rmap).unwrap();
        assert_eq!(
            sym,
            SymbolicOpcode::AssignCurr {
                var: SymVarRef {
                    name: "births".to_string(),
                    element_offset: 0
                }
            }
        );
    }

    #[test]
    fn test_symbolize_passthrough_opcodes() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        // These opcodes should pass through unchanged
        let op = Opcode::LoadGlobalVar { off: 1 };
        let sym = symbolize_opcode(&op, &rmap).unwrap();
        assert_eq!(sym, SymbolicOpcode::LoadGlobalVar { off: 1 });

        let op = Opcode::Op2 { op: Op2::Add };
        let sym = symbolize_opcode(&op, &rmap).unwrap();
        assert_eq!(sym, SymbolicOpcode::Op2 { op: Op2::Add });

        let op = Opcode::Ret;
        let sym = symbolize_opcode(&op, &rmap).unwrap();
        assert_eq!(sym, SymbolicOpcode::Ret);
    }

    #[test]
    fn test_resolve_var_ref() {
        let layout = simple_layout();

        let var = SymVarRef {
            name: "population".to_string(),
            element_offset: 0,
        };
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 5);

        let var = SymVarRef {
            name: "births".to_string(),
            element_offset: 0,
        };
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

        let var = SymVarRef {
            name: "arr".to_string(),
            element_offset: 2,
        };
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 12);
    }

    #[test]
    fn test_resolve_var_ref_element_offset_out_of_bounds() {
        let mut entries = HashMap::new();
        entries.insert("arr".to_string(), LayoutEntry { offset: 4, size: 3 });
        let layout = VariableLayout::new(entries, 7);

        // element_offset == size (out of bounds)
        let var = SymVarRef {
            name: "arr".to_string(),
            element_offset: 3,
        };
        assert!(
            resolve_var_ref(&var, &layout).is_err(),
            "element_offset >= size should fail"
        );

        // element_offset well beyond size
        let var = SymVarRef {
            name: "arr".to_string(),
            element_offset: 100,
        };
        assert!(resolve_var_ref(&var, &layout).is_err());

        // element_offset at max valid index should succeed
        let var = SymVarRef {
            name: "arr".to_string(),
            element_offset: 2,
        };
        assert_eq!(resolve_var_ref(&var, &layout).unwrap(), 6);
    }

    #[test]
    fn test_resolve_missing_variable() {
        let layout = simple_layout();
        let var = SymVarRef {
            name: "nonexistent".to_string(),
            element_offset: 0,
        };
        assert!(resolve_var_ref(&var, &layout).is_err());
    }

    #[test]
    fn test_bytecode_roundtrip() {
        let layout = simple_layout();

        let bc = ByteCode {
            literals: vec![1.0, 0.5],
            code: vec![
                Opcode::LoadVar { off: 5 },     // population
                Opcode::LoadConstant { id: 1 }, // 0.5
                Opcode::Op2 { op: Op2::Mul },
                Opcode::AssignCurr { off: 4 }, // births
                Opcode::Ret,
            ],
        };

        let sym = symbolize_bytecode(&bc, &ReverseOffsetMap::from_layout(&layout)).unwrap();
        let resolved = resolve_bytecode(&sym, &layout).unwrap();

        assert_eq!(bc.literals, resolved.literals);
        assert_eq!(bc.code.len(), resolved.code.len());
        for (i, (orig, res)) in bc.code.iter().zip(resolved.code.iter()).enumerate() {
            assert!(
                opcode_eq(orig, res),
                "opcode mismatch at index {}: {:?} vs {:?}",
                i,
                orig,
                res
            );
        }
    }

    #[test]
    fn test_bytecode_roundtrip_superinstructions() {
        let layout = simple_layout();

        let bc = ByteCode {
            literals: vec![100.0, 0.0],
            code: vec![
                Opcode::AssignConstCurr {
                    off: 5,
                    literal_id: 0,
                },
                Opcode::BinOpAssignCurr {
                    op: Op2::Add,
                    off: 4,
                },
                Opcode::BinOpAssignNext {
                    op: Op2::Mul,
                    off: 5,
                },
                Opcode::Ret,
            ],
        };

        let sym = symbolize_bytecode(&bc, &ReverseOffsetMap::from_layout(&layout)).unwrap();
        let resolved = resolve_bytecode(&sym, &layout).unwrap();

        assert_eq!(bc.code.len(), resolved.code.len());
        for (i, (orig, res)) in bc.code.iter().zip(resolved.code.iter()).enumerate() {
            assert!(
                opcode_eq(orig, res),
                "opcode mismatch at index {}: {:?} vs {:?}",
                i,
                orig,
                res
            );
        }
    }

    #[test]
    fn test_bytecode_roundtrip_global_vars() {
        let layout = simple_layout();

        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadGlobalVar { off: 0 }, // time
                Opcode::LoadGlobalVar { off: 1 }, // dt
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 4 },
                Opcode::Ret,
            ],
        };

        let sym = symbolize_bytecode(&bc, &ReverseOffsetMap::from_layout(&layout)).unwrap();
        let resolved = resolve_bytecode(&sym, &layout).unwrap();

        for (i, (orig, res)) in bc.code.iter().zip(resolved.code.iter()).enumerate() {
            assert!(
                opcode_eq(orig, res),
                "opcode mismatch at index {}: {:?} vs {:?}",
                i,
                orig,
                res
            );
        }
    }

    #[test]
    fn test_static_view_roundtrip_var() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let view = StaticArrayView {
            base_off: 5,
            is_temp: false,
            dims: SmallVec::from_slice(&[3]),
            strides: SmallVec::from_slice(&[1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0]),
        };

        let sym = symbolize_static_view(&view, &rmap).unwrap();
        assert!(matches!(sym.base, SymStaticViewBase::Var(_)));

        let resolved = resolve_static_view(&sym, &layout).unwrap();
        assert_eq!(view.base_off, resolved.base_off);
        assert_eq!(view.is_temp, resolved.is_temp);
        assert_eq!(view.dims, resolved.dims);
        assert_eq!(view.offset, resolved.offset);
    }

    #[test]
    fn test_static_view_roundtrip_temp() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let view = StaticArrayView {
            base_off: 7,
            is_temp: true,
            dims: SmallVec::from_slice(&[2, 3]),
            strides: SmallVec::from_slice(&[3, 1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0, 1]),
        };

        let sym = symbolize_static_view(&view, &rmap).unwrap();
        assert!(matches!(sym.base, SymStaticViewBase::Temp(7)));

        let resolved = resolve_static_view(&sym, &layout).unwrap();
        assert_eq!(view.base_off, resolved.base_off);
        assert_eq!(view.is_temp, resolved.is_temp);
    }

    #[test]
    fn test_module_decl_roundtrip() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let decl = ModuleDeclaration {
            model_name: Ident::new("sub_model"),
            input_set: BTreeSet::new(),
            off: 4,
        };

        let sym = symbolize_module_decl(&decl, &rmap).unwrap();
        assert_eq!(sym.var.name, "births");

        let resolved = resolve_module_decl(&sym, &layout).unwrap();
        assert_eq!(decl.off, resolved.off);
        assert_eq!(decl.model_name, resolved.model_name);
    }

    #[test]
    fn test_layout_independence() {
        // Symbolize with one layout, resolve with a different layout.
        // The symbolic bytecodes should produce correct concrete offsets
        // for the new layout.

        let layout1 = simple_layout(); // births=4, population=5

        let bc = ByteCode {
            literals: vec![0.1],
            code: vec![
                Opcode::LoadVar { off: 5 }, // population in layout1
                Opcode::LoadConstant { id: 0 },
                Opcode::Op2 { op: Op2::Mul },
                Opcode::AssignCurr { off: 4 }, // births in layout1
                Opcode::Ret,
            ],
        };

        // Symbolize using layout1
        let sym = symbolize_bytecode(&bc, &ReverseOffsetMap::from_layout(&layout1)).unwrap();

        // Verify symbolic opcodes reference variable names, not offsets
        assert_eq!(
            sym.code[0],
            SymbolicOpcode::LoadVar {
                var: SymVarRef {
                    name: "population".to_string(),
                    element_offset: 0
                }
            }
        );

        // Create layout2 with different offsets (swapped positions + new variable)
        let mut entries2 = HashMap::new();
        entries2.insert("time".to_string(), LayoutEntry { offset: 0, size: 1 });
        entries2.insert("dt".to_string(), LayoutEntry { offset: 1, size: 1 });
        entries2.insert(
            "initial_time".to_string(),
            LayoutEntry { offset: 2, size: 1 },
        );
        entries2.insert("final_time".to_string(), LayoutEntry { offset: 3, size: 1 });
        // New variable inserted alphabetically between births and population
        entries2.insert("births".to_string(), LayoutEntry { offset: 4, size: 1 });
        entries2.insert(
            "growth_rate".to_string(),
            LayoutEntry { offset: 5, size: 1 },
        );
        entries2.insert("population".to_string(), LayoutEntry { offset: 6, size: 1 });
        let layout2 = VariableLayout::new(entries2, 7);

        // Resolve using layout2
        let resolved = resolve_bytecode(&sym, &layout2).unwrap();

        // population is now at offset 6 (was 5)
        assert!(opcode_eq(&resolved.code[0], &Opcode::LoadVar { off: 6 }));
        // births is still at offset 4
        assert!(opcode_eq(&resolved.code[3], &Opcode::AssignCurr { off: 4 }));
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

    // Helper: compare opcodes for equality.
    // Opcode doesn't derive PartialEq, so we compare via Debug representation.
    #[cfg(feature = "debug-derive")]
    fn opcode_eq(a: &Opcode, b: &Opcode) -> bool {
        format!("{:?}", a) == format!("{:?}", b)
    }

    // When debug-derive is not enabled, compare by encoding a known
    // discriminant + payload check for the opcodes we use in tests.
    #[cfg(not(feature = "debug-derive"))]
    fn opcode_eq(a: &Opcode, b: &Opcode) -> bool {
        // Use the symbolize/resolve roundtrip property: if both opcodes
        // symbolize to the same SymbolicOpcode, they are equal.
        // We need a layout that covers all offsets used in tests.
        let mut entries = HashMap::new();
        for off in 0..20 {
            entries.insert(
                format!("__test_var_{}", off),
                LayoutEntry {
                    offset: off,
                    size: 1,
                },
            );
        }
        let layout = VariableLayout::new(entries, 20);
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let sym_a = symbolize_opcode(a, &rmap);
        let sym_b = symbolize_opcode(b, &rmap);
        match (sym_a, sym_b) {
            (Ok(sa), Ok(sb)) => sa == sb,
            _ => false,
        }
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

    fn compile_and_roundtrip(dm_project: &crate::datamodel::Project, model_name: &str) {
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, dm_project, None);
        let sim = crate::db::compile_project_incremental(&db, sync.project, model_name)
            .expect("incremental compile should succeed");

        let compiled = &sim.modules[&sim.root];

        let source_model = sync.models[model_name].source_model;
        let layout = crate::db::compute_layout(&db, source_model, sync.project, true);

        let sym = symbolize_module(compiled, layout)
            .unwrap_or_else(|e| panic!("symbolize_module failed: {e}"));

        let resolved =
            resolve_module(&sym, layout).unwrap_or_else(|e| panic!("resolve_module failed: {e}"));

        // Verify structural equivalence
        assert_eq!(compiled.ident, resolved.ident);
        assert_eq!(compiled.n_slots, resolved.n_slots);
        assert_eq!(
            compiled.compiled_initials.len(),
            resolved.compiled_initials.len()
        );

        // Compare initials
        for (orig, res) in compiled
            .compiled_initials
            .iter()
            .zip(resolved.compiled_initials.iter())
        {
            assert_eq!(orig.ident, res.ident);
            assert_eq!(
                orig.offsets, res.offsets,
                "initial offsets mismatch for {}",
                orig.ident
            );
            assert_eq!(orig.bytecode.literals, res.bytecode.literals);
            assert_eq!(
                orig.bytecode.code.len(),
                res.bytecode.code.len(),
                "initial code length mismatch for {}",
                orig.ident
            );
            for (i, (o, r)) in orig
                .bytecode
                .code
                .iter()
                .zip(res.bytecode.code.iter())
                .enumerate()
            {
                assert!(
                    opcode_eq(o, r),
                    "initial opcode mismatch at index {} for {}: {:?} vs {:?}",
                    i,
                    orig.ident,
                    o,
                    r
                );
            }
        }

        // Compare flows bytecode
        assert_eq!(
            compiled.compiled_flows.literals,
            resolved.compiled_flows.literals
        );
        assert_eq!(
            compiled.compiled_flows.code.len(),
            resolved.compiled_flows.code.len(),
            "flows code length mismatch"
        );
        for (i, (o, r)) in compiled
            .compiled_flows
            .code
            .iter()
            .zip(resolved.compiled_flows.code.iter())
            .enumerate()
        {
            assert!(
                opcode_eq(o, r),
                "flows opcode mismatch at index {}: {:?} vs {:?}",
                i,
                o,
                r
            );
        }

        // Compare stocks bytecode
        assert_eq!(
            compiled.compiled_stocks.literals,
            resolved.compiled_stocks.literals
        );
        assert_eq!(
            compiled.compiled_stocks.code.len(),
            resolved.compiled_stocks.code.len(),
            "stocks code length mismatch"
        );
        for (i, (o, r)) in compiled
            .compiled_stocks
            .code
            .iter()
            .zip(resolved.compiled_stocks.code.iter())
            .enumerate()
        {
            assert!(
                opcode_eq(o, r),
                "stocks opcode mismatch at index {}: {:?} vs {:?}",
                i,
                o,
                r
            );
        }

        // Compare context fields
        assert_eq!(
            compiled.context.graphical_functions,
            resolved.context.graphical_functions
        );
        assert_eq!(
            compiled.context.modules.len(),
            resolved.context.modules.len()
        );
        for (orig_md, res_md) in compiled
            .context
            .modules
            .iter()
            .zip(resolved.context.modules.iter())
        {
            assert_eq!(orig_md.model_name, res_md.model_name);
            assert_eq!(orig_md.off, res_md.off);
            assert_eq!(orig_md.input_set, res_md.input_set);
        }

        assert_eq!(
            compiled.context.static_views.len(),
            resolved.context.static_views.len()
        );
        for (orig_sv, res_sv) in compiled
            .context
            .static_views
            .iter()
            .zip(resolved.context.static_views.iter())
        {
            assert_eq!(orig_sv.base_off, res_sv.base_off);
            assert_eq!(orig_sv.is_temp, res_sv.is_temp);
            assert_eq!(orig_sv.dims, res_sv.dims);
            assert_eq!(orig_sv.strides, res_sv.strides);
            assert_eq!(orig_sv.offset, res_sv.offset);
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
        compile_and_roundtrip(&dm_project, "main");
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
        compile_and_roundtrip(&dm_project, "main");
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
        compile_and_roundtrip(&dm_project, "main");
    }

    #[test]
    fn test_resolve_uses_layout_n_slots_not_symbolic() {
        let dm_project = x_project(
            default_sim_specs(),
            &[x_model(
                "main",
                vec![x_aux("a", "1", None), x_aux("b", "a + 1", None)],
            )],
        );
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &dm_project, None);
        let sim = crate::db::compile_project_incremental(&db, sync.project, "main")
            .expect("incremental compile should succeed");

        let compiled = &sim.modules[&sim.root];

        let source_model = sync.models["main"].source_model;
        let layout = crate::db::compute_layout(&db, source_model, sync.project, true);
        let sym = symbolize_module(compiled, layout).unwrap();

        // Create a layout with more slots (simulating a variable addition)
        let mut bigger_entries = layout.entries.clone();
        bigger_entries.insert(
            "new_var".to_string(),
            LayoutEntry {
                offset: layout.n_slots,
                size: 1,
            },
        );
        let bigger_layout = VariableLayout::new(bigger_entries, layout.n_slots + 1);

        let resolved = resolve_module(&sym, &bigger_layout).unwrap();
        assert_eq!(
            resolved.n_slots, bigger_layout.n_slots,
            "resolved module should use layout's n_slots, not the stale symbolic value"
        );
        assert_ne!(
            resolved.n_slots, sym.n_slots,
            "resolved n_slots should differ from symbolic n_slots when layout changed"
        );
    }

    // ====================================================================
    // u16 truncation boundary tests (issue #291)
    // ====================================================================

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
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let view = StaticArrayView {
            base_off: large_off as u32,
            is_temp: false,
            dims: SmallVec::from_slice(&[3]),
            strides: SmallVec::from_slice(&[1]),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::from_slice(&[0]),
        };

        let sym = symbolize_static_view(&view, &rmap).unwrap();
        match &sym.base {
            SymStaticViewBase::Var(var_ref) => {
                assert_eq!(var_ref.name, "big_var");
                assert_eq!(var_ref.element_offset, 0);
            }
            SymStaticViewBase::Temp(_) => panic!("expected Var, got Temp"),
        }

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
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let decl = ModuleDeclaration {
            model_name: Ident::new("sub"),
            input_set: BTreeSet::new(),
            off: large_off,
        };

        let sym = symbolize_module_decl(&decl, &rmap).unwrap();
        assert_eq!(sym.var.name, "big_module");

        let resolved = resolve_module_decl(&sym, &layout).unwrap();
        assert_eq!(resolved.off, large_off);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_module_decl_offset_overflow_is_rejected() {
        let mut entries = HashMap::new();
        entries.insert("wrapped".to_string(), LayoutEntry { offset: 4, size: 1 });
        let layout = VariableLayout::new(entries, 5);
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let overflowing_off = (u32::MAX as usize) + 5;
        let decl = ModuleDeclaration {
            model_name: Ident::new("sub"),
            input_set: BTreeSet::new(),
            off: overflowing_off,
        };

        let err = symbolize_module_decl(&decl, &rmap).unwrap_err();
        assert!(
            err.contains("does not fit in u32"),
            "expected explicit overflow error, got: {err}"
        );
    }

    #[test]
    fn test_unmapped_offset() {
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), LayoutEntry { offset: 0, size: 1 });
        // Offset 1 is allocated but not mapped to any variable
        let layout = VariableLayout::new(entries, 3);
        let rmap = ReverseOffsetMap::from_layout(&layout);

        assert!(rmap.lookup(0).is_ok());
        let err = rmap.lookup(1).unwrap_err();
        assert!(err.contains("no variable mapped at offset"));
    }

    // ====================================================================
    // Opcode roundtrip coverage: passthrough opcodes
    // ====================================================================

    #[test]
    fn test_roundtrip_control_flow_and_builtin_opcodes() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let opcodes = vec![
            Opcode::Not {},
            Opcode::SetCond {},
            Opcode::If {},
            Opcode::LoadModuleInput { input: 3 },
            Opcode::EvalModule { id: 0, n_inputs: 2 },
            Opcode::Apply {
                func: BuiltinId::Abs,
            },
            Opcode::Lookup {
                base_gf: 0,
                table_count: 4,
                mode: LookupMode::Interpolate,
            },
            Opcode::Lookup {
                base_gf: 1,
                table_count: 1,
                mode: LookupMode::Forward,
            },
            Opcode::Ret,
        ];

        for op in &opcodes {
            let sym = symbolize_opcode(op, &rmap).unwrap();
            let resolved = resolve_opcode(&sym, &layout).unwrap();
            assert!(opcode_eq(op, &resolved), "roundtrip failed for {:?}", sym);
        }
    }

    #[test]
    fn test_roundtrip_view_stack_opcodes() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let opcodes = vec![
            Opcode::PushVarView {
                base_off: 4,
                dim_list_id: 0,
            },
            Opcode::PushTempView {
                temp_id: 1,
                dim_list_id: 2,
            },
            Opcode::PushStaticView { view_id: 3 },
            Opcode::PushVarViewDirect {
                base_off: 5,
                dim_list_id: 1,
            },
            Opcode::ViewSubscriptConst {
                dim_idx: 0,
                index: 2,
            },
            Opcode::ViewSubscriptDynamic { dim_idx: 1 },
            Opcode::ViewRange {
                dim_idx: 0,
                start: 1,
                end: 5,
            },
            Opcode::ViewRangeDynamic { dim_idx: 2 },
            Opcode::ViewStarRange {
                dim_idx: 0,
                subdim_relation_id: 7,
            },
            Opcode::ViewWildcard { dim_idx: 1 },
            Opcode::ViewTranspose {},
            Opcode::PopView {},
            Opcode::DupView {},
        ];

        for op in &opcodes {
            let sym = symbolize_opcode(op, &rmap).unwrap();
            let resolved = resolve_opcode(&sym, &layout).unwrap();
            assert!(opcode_eq(op, &resolved), "roundtrip failed for {:?}", sym);
        }
    }

    #[test]
    fn test_roundtrip_temp_and_subscript_opcodes() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let opcodes = vec![
            Opcode::LoadTempConst {
                temp_id: 0,
                index: 3,
            },
            Opcode::LoadTempDynamic { temp_id: 2 },
            Opcode::PushSubscriptIndex { bounds: 4 },
            Opcode::LoadSubscript { off: 5 },
            Opcode::AssignNext { off: 4 },
        ];

        for op in &opcodes {
            let sym = symbolize_opcode(op, &rmap).unwrap();
            let resolved = resolve_opcode(&sym, &layout).unwrap();
            assert!(opcode_eq(op, &resolved), "roundtrip failed for {:?}", sym);
        }
    }

    #[test]
    fn test_roundtrip_iteration_opcodes() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let opcodes = vec![
            Opcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: true,
            },
            Opcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: false,
            },
            Opcode::LoadIterElement {},
            Opcode::LoadIterTempElement { temp_id: 1 },
            Opcode::LoadIterViewTop {},
            Opcode::LoadIterViewAt { offset: 2 },
            Opcode::StoreIterElement {},
            Opcode::NextIterOrJump { jump_back: -5 },
            Opcode::EndIter {},
        ];

        for op in &opcodes {
            let sym = symbolize_opcode(op, &rmap).unwrap();
            let resolved = resolve_opcode(&sym, &layout).unwrap();
            assert!(opcode_eq(op, &resolved), "roundtrip failed for {:?}", sym);
        }
    }

    #[test]
    fn test_roundtrip_broadcast_and_reduction_opcodes() {
        let layout = simple_layout();
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let opcodes = vec![
            Opcode::ArraySum {},
            Opcode::ArrayMax {},
            Opcode::ArrayMin {},
            Opcode::ArrayMean {},
            Opcode::ArrayStddev {},
            Opcode::ArraySize {},
            Opcode::BeginBroadcastIter {
                n_sources: 2,
                dest_temp_id: 0,
            },
            Opcode::LoadBroadcastElement { source_idx: 0 },
            Opcode::LoadBroadcastElement { source_idx: 1 },
            Opcode::StoreBroadcastElement {},
            Opcode::NextBroadcastOrJump { jump_back: -4 },
            Opcode::EndBroadcastIter {},
        ];

        for op in &opcodes {
            let sym = symbolize_opcode(op, &rmap).unwrap();
            let resolved = resolve_opcode(&sym, &layout).unwrap();
            assert!(opcode_eq(op, &resolved), "roundtrip failed for {:?}", sym);
        }
    }

    // ====================================================================
    // Error path coverage
    // ====================================================================

    #[test]
    fn test_symbolize_opcode_out_of_range_offset() {
        let mut entries = HashMap::new();
        entries.insert("a".to_string(), LayoutEntry { offset: 0, size: 1 });
        let layout = VariableLayout::new(entries, 1);
        let rmap = ReverseOffsetMap::from_layout(&layout);

        let op = Opcode::LoadVar { off: 50 };
        let err = symbolize_opcode(&op, &rmap).unwrap_err();
        assert!(err.contains("out of range"));
    }

    #[test]
    fn test_resolve_static_view_missing_variable() {
        let layout = simple_layout();
        let sym_view = SymbolicStaticView {
            base: SymStaticViewBase::Var(SymVarRef {
                name: "nonexistent".to_string(),
                element_offset: 0,
            }),
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
            var: SymVarRef {
                name: "nonexistent".to_string(),
                element_offset: 0,
            },
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
        compile_and_roundtrip(&dm_project, "main");
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
        // path* a compiled model takes -- `symbolize_opcode` ->
        // `concatenate_fragments` (absorb + `renumber_fragment_code`) ->
        // `resolve_bytecode` -- with the VECTOR ELM MAP opcode merged AFTER a
        // temp-contributing fragment so its `write_temp_id` gets a *non-zero*
        // renumber offset. The invariant under test: `full_source_len` is an
        // absolute element count, NOT a renumber-able resource id, so it must
        // come out of symbolize -> concatenate/renumber -> resolve
        // byte-identical even though `write_temp_id` is offset. If
        // `full_source_len` were ever (mistakenly) treated like a temp id, it
        // would be shifted by `frag_a`'s temp count here and this test would
        // fail; the existing renumber unit test would not catch that
        // regression because it never drives the fragment merger.
        const GENUINE_FULL_SOURCE_LEN: u32 = 6; // e.g. d[DimA,DimB] = 3 x 2
        let original = Opcode::VectorElmMap {
            write_temp_id: 0,
            full_source_len: GENUINE_FULL_SOURCE_LEN,
        };

        // The VectorElmMap opcode carries no variable offset, so an empty
        // layout (=> empty ReverseOffsetMap) is sufficient for symbolization.
        let empty_layout = VariableLayout::new(HashMap::new(), 0);
        let rmap = ReverseOffsetMap::from_layout(&empty_layout);
        let symbolic_elm_map = symbolize_opcode(&original, &rmap).unwrap();

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

        // Resolve back to concrete bytecode (same empty layout: the
        // VectorElmMap opcode carries no SymVarRef).
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
        // survive symbolize -> concatenate/renumber -> resolve unchanged even
        // though write_temp_id was offset.
        assert_eq!(
            elm_map.1, GENUINE_FULL_SOURCE_LEN,
            "full_source_len must survive the symbolize -> fragment-merge -> \
             resolve round-trip byte-identical (it is an absolute element \
             count, not a renumber-able resource id); a corrupted value would \
             feed the VM a wrong full-source extent and break genuine VECTOR \
             ELM MAP results"
        );
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
    // #582: cross-fragment graphical-function de-duplication.
    //
    // `concatenate_fragments` previously appended every fragment's
    // `graphical_functions` with no de-duplication (the flat running
    // `gf_offset = merged_gf.len()`), so a dependency arrayed GF referenced
    // by N consumer fragments duplicated N times and `renumber_opcode`'s
    // `gf_off > u8::MAX` guard tripped once the duplicated count crossed
    // 255 -- even though the *distinct* count is small. The monolithic
    // `Compiler::new` (codegen.rs) carries each variable's GF list exactly
    // once (its `module.tables` map is keyed by ident), so the incremental
    // path was incorrect-by-omission, not merely capacity-limited.
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
    // #583: match the monolithic temp recycling in the plain-phase concat.
    //
    // The monolithic `Module::compile` flattens every variable's exprs into
    // one runlist and max-merges their temps via a `HashMap<temp_id, size>`
    // (`compiler/mod.rs`): since each variable's temps are 0-based scratch
    // that die at that variable's runlist-segment end, variable A's temp 0
    // and variable B's temp 0 collapse to ONE global slot 0. The plain-phase
    // incremental concat must produce the SAME identity-recycled pool (one
    // shared pool across initials/flows/stocks), not a per-fragment SUM --
    // summing both wastes slots AND, with a non-zero phase ctx_base, drives
    // the renumbered `temp_id` past `u8::MAX` (the C-LEARN
    // `temp offset ... exceeds TempId capacity` failure). `combine_scc_fragment`
    // stays on the disjoint (sum) path because its per-element segments
    // interleave (overlapping live ranges) -- see `db_combined_fragment_tests`.
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

    /// The monolithic `Module::compile` temp count: the keyed max-merge of
    /// every fragment's `(temp_id, size)` pairs (`compiler/mod.rs` flattens
    /// every variable's exprs and merges into one `HashMap<temp_id, size>`).
    /// `n_temps` is the number of distinct ids (== `max_id + 1` for the
    /// dense 0-based ids real per-variable fragments produce).
    fn monolithic_n_temps(frags: &[&PerVarBytecodes]) -> usize {
        let mut map: HashMap<u32, usize> = HashMap::new();
        for frag in frags {
            for (id, size) in &frag.temp_sizes {
                let e = map.entry(*id).or_insert(0);
                *e = (*e).max(*size);
            }
        }
        map.len()
    }

    #[test]
    fn test_concatenate_recycles_temps_to_match_monolithic() {
        // Three per-variable fragments, each with its own 0-based temp 0.
        // Monolithic recycles them to ONE slot (n_temps == 1). The plain-
        // phase concat must do the same: a single shared identity pool, not
        // three summed slots.
        let frag_a = sort_order_temp_frag(0, 4);
        let frag_b = sort_order_temp_frag(0, 8);
        let frag_c = sort_order_temp_frag(0, 2);
        let refs: Vec<&PerVarBytecodes> = vec![&frag_a, &frag_b, &frag_c];

        let expected = monolithic_n_temps(&refs);
        assert_eq!(expected, 1, "monolithic recycles three id-0 temps to one");

        let no_base = ContextResourceCounts::default();
        let merged = concatenate_fragments(&refs, &no_base).unwrap();

        // The recycled pool has exactly `expected` slots, and the surviving
        // slot's size is the MAX over the fragments that used it (8), since
        // they share the storage (the monolithic keyed max-merge).
        assert_eq!(
            merged.temp_offsets.len(),
            expected,
            "incremental plain-phase temp count must EQUAL the monolithic \
             Module::compile n_temps (recycle, not sum)"
        );
        assert_eq!(
            merged.temp_total_size, 8,
            "the recycled slot's size is the max over the fragments using it"
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
        // A fragment using temp ids {0, 1} and another using {0}. Monolithic
        // merges to ids {0, 1} (2 slots, sizes max-merged per id). The
        // plain-phase concat must match: 2 slots, not 3 summed.
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

        let expected = monolithic_n_temps(&refs);
        assert_eq!(expected, 2, "ids {{0,1}} merged with {{0}} -> 2 distinct");

        let merged = concatenate_fragments(&refs, &ContextResourceCounts::default()).unwrap();
        assert_eq!(merged.temp_offsets.len(), expected);
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
