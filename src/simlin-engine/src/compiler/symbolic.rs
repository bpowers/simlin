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

// These types and functions are exercised by tests now and will be used by later
// phases of the incremental compilation pipeline.
#![allow(dead_code)]

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
struct ReverseOffsetMap {
    /// Indexed by offset. `entries[off] = Some((name, element_offset))`.
    entries: Vec<Option<(String, usize)>>,
}

impl ReverseOffsetMap {
    /// Build from a VariableLayout.
    fn from_layout(layout: &VariableLayout) -> Self {
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
) -> VariableLayout {
    let model_metadata = &metadata[model_name];
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

    VariableLayout::new(entries, n_slots)
}

// ============================================================================
// Symbolize: Concrete -> Symbolic
// ============================================================================

fn symbolize_opcode(op: &Opcode, rmap: &ReverseOffsetMap) -> Result<SymbolicOpcode, String> {
    match op {
        // Opcodes with variable offsets that need symbolization
        Opcode::LoadVar { off } => Ok(SymbolicOpcode::LoadVar {
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

fn symbolize_static_view(
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

fn symbolize_module_decl(
    decl: &ModuleDeclaration,
    rmap: &ReverseOffsetMap,
) -> Result<SymbolicModuleDecl, String> {
    Ok(SymbolicModuleDecl {
        model_name: decl.model_name.clone(),
        input_set: decl.input_set.clone(),
        var: rmap.lookup(decl.off as u32)?,
    })
}

fn symbolize_bytecode(bc: &ByteCode, rmap: &ReverseOffsetMap) -> Result<SymbolicByteCode, String> {
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
// Resolve: Symbolic -> Concrete (Assembly)
// ============================================================================

fn resolve_var_ref(var: &SymVarRef, layout: &VariableLayout) -> Result<VariableOffset, String> {
    let entry = layout.get(&var.name).ok_or_else(|| {
        format!(
            "variable '{}' not found in layout during resolution",
            var.name
        )
    })?;
    let off = entry.offset + var.element_offset;
    Ok(off as VariableOffset)
}

fn resolve_opcode(op: &SymbolicOpcode, layout: &VariableLayout) -> Result<Opcode, String> {
    match op {
        // Opcodes with symbolic variable references
        SymbolicOpcode::LoadVar { var } => Ok(Opcode::LoadVar {
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

fn resolve_bytecode(sbc: &SymbolicByteCode, layout: &VariableLayout) -> Result<ByteCode, String> {
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

fn resolve_static_view(
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

fn resolve_module_decl(
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
fn extract_assign_curr_offsets(bc: &ByteCode) -> Vec<usize> {
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
        use std::collections::BTreeSet;
        use std::sync::Arc;

        let project = crate::project::Project::from(dm_project.clone());
        let main_ident = crate::common::Ident::new(model_name);
        let model = Arc::clone(&project.models[&main_ident]);
        let inputs: BTreeSet<crate::common::Ident<crate::common::Canonical>> = BTreeSet::new();
        let module = crate::compiler::Module::new(&project, model, &inputs, true).unwrap();
        let compiled = module.compile().unwrap();

        // Build layout from Module's offset map
        let model_offsets = &module.offsets[&main_ident];
        let layout = VariableLayout::from_offset_map(model_offsets, module.n_slots);

        // Symbolize
        let sym = symbolize_module(&compiled, &layout)
            .unwrap_or_else(|e| panic!("symbolize_module failed: {}", e));

        // Resolve back with the same layout
        let resolved = resolve_module(&sym, &layout)
            .unwrap_or_else(|e| panic!("resolve_module failed: {}", e));

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
        use std::collections::BTreeSet;
        use std::sync::Arc;

        let dm_project = x_project(
            default_sim_specs(),
            &[x_model(
                "main",
                vec![x_aux("a", "1", None), x_aux("b", "a + 1", None)],
            )],
        );
        let project = crate::project::Project::from(dm_project.clone());
        let main_ident = crate::common::Ident::new("main");
        let model = Arc::clone(&project.models[&main_ident]);
        let inputs: BTreeSet<crate::common::Ident<crate::common::Canonical>> = BTreeSet::new();
        let module = crate::compiler::Module::new(&project, model, &inputs, true).unwrap();
        let compiled = module.compile().unwrap();

        let model_offsets = &module.offsets[&main_ident];
        let layout = VariableLayout::from_offset_map(model_offsets, module.n_slots);
        let sym = symbolize_module(&compiled, &layout).unwrap();

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
}
