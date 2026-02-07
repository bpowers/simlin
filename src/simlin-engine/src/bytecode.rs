// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use smallvec::SmallVec;

use crate::common::{Canonical, Ident};
use ordered_float::OrderedFloat;

// ============================================================================
// Type Aliases
// ============================================================================

pub type LiteralId = u16;
pub type ModuleId = u16;
pub type VariableOffset = u16;
pub type ModuleInputOffset = u16;
pub type GraphicalFunctionId = u8;

// New types for array support
pub type ViewId = u16; // Index into static_views table
pub type DimId = u16; // Index into dimensions table
pub type TempId = u8; // Temp array ID (max 256 temps per module)
pub type PcOffset = i16; // Relative PC offset for jumps (signed for backward jumps)
pub type NameId = u16; // Index into names table
pub type DimListId = u16; // Index into dim_lists table (for [DimId; 4] or [u16; 4])

/// Fixed capacity for the VM arithmetic stack.
///
/// 64 is generous for system dynamics expressions: the stack depth equals the
/// maximum nesting depth of an expression tree. Even complex equations like
/// `IF(a > b AND c < d, MAX(e, f) * g + h, MIN(i, j) / k - l)` use ~5 slots.
/// The stack resets to 0 after every assignment opcode, so depth depends only on
/// expression complexity, not on model size.
///
/// `ByteCodeBuilder::finish()` validates at compile time that no bytecode
/// sequence exceeds this capacity, making the VM's unsafe unchecked stack
/// access provably safe. The `#![deny(unsafe_code)]` crate attribute ensures
/// no other unsafe code can be added without explicit opt-in.
pub(crate) const STACK_CAPACITY: usize = 64;

/// Lookup interpolation mode for graphical function tables.
#[repr(u8)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LookupMode {
    /// Linear interpolation between points (standard LOOKUP behavior)
    Interpolate = 0,
    /// Step function: return y at first point where x >= index (LOOKUP_FORWARD)
    Forward = 1,
    /// Step function: return y at last point where x <= index (LOOKUP_BACKWARD)
    Backward = 2,
}

// ============================================================================
// Dimension Information (for runtime dimension table)
// ============================================================================

/// Runtime dimension information stored in ByteCodeContext.
/// Used for dynamic array operations like star ranges and broadcasting.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct DimensionInfo {
    /// Index into names table for this dimension's name
    pub name_id: NameId,
    /// Number of elements in this dimension
    pub size: u16,
    /// true = indexed dimension (1..n), false = named elements
    pub is_indexed: bool,
    /// For named dimensions: indices into names table for each element name
    /// For indexed dimensions: empty
    pub element_name_ids: SmallVec<[NameId; 8]>,
}

#[allow(dead_code)]
impl DimensionInfo {
    /// Create a new indexed dimension (elements are 1..size)
    pub fn indexed(name_id: NameId, size: u16) -> Self {
        DimensionInfo {
            name_id,
            size,
            is_indexed: true,
            element_name_ids: SmallVec::new(),
        }
    }

    /// Create a new named dimension with element name IDs
    pub fn named(name_id: NameId, element_name_ids: SmallVec<[NameId; 8]>) -> Self {
        let size = element_name_ids.len() as u16;
        DimensionInfo {
            name_id,
            size,
            is_indexed: false,
            element_name_ids,
        }
    }
}

/// Subdimension relationship for star ranges like `*:SubDim`.
/// Describes which parent indices a subdimension maps to.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct SubdimensionRelation {
    /// Index of parent dimension in dimensions table
    pub parent_dim_id: DimId,
    /// Index of child (sub)dimension in dimensions table
    pub child_dim_id: DimId,
    /// Which parent indices this subdimension maps to (0-based)
    /// e.g., [0, 2, 3] means subdim elements map to parent indices 0, 2, 3
    pub parent_offsets: SmallVec<[u16; 16]>,
    /// Optimization flag: if true, parent_offsets form a contiguous range
    pub is_contiguous: bool,
    /// If contiguous, the starting offset in the parent
    pub start_offset: u16,
}

#[allow(dead_code)]
impl SubdimensionRelation {
    /// Create a contiguous subdimension relation (elements form a range)
    pub fn contiguous(parent_dim_id: DimId, child_dim_id: DimId, start: u16, count: u16) -> Self {
        let parent_offsets: SmallVec<[u16; 16]> = (start..start + count).collect();
        SubdimensionRelation {
            parent_dim_id,
            child_dim_id,
            parent_offsets,
            is_contiguous: true,
            start_offset: start,
        }
    }

    /// Create a sparse subdimension relation (elements at arbitrary positions)
    pub fn sparse(parent_dim_id: DimId, child_dim_id: DimId, offsets: SmallVec<[u16; 16]>) -> Self {
        // Check if actually contiguous
        let is_contiguous = offsets.windows(2).all(|w| w[1] == w[0] + 1);
        let start_offset = offsets.first().copied().unwrap_or(0);
        SubdimensionRelation {
            parent_dim_id,
            child_dim_id,
            parent_offsets: offsets,
            is_contiguous,
            start_offset,
        }
    }
}

// ============================================================================
// Runtime View (for view stack during VM execution)
// ============================================================================

/// Sparse mapping for a single dimension in a RuntimeView.
/// Used when iterating over non-contiguous elements.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RuntimeSparseMapping {
    /// Which dimension (0-indexed) in the view is sparse
    pub dim_index: u8,
    /// Parent offsets to iterate (e.g., [0, 2] for elements at indices 0 and 2)
    pub parent_offsets: SmallVec<[u16; 16]>,
}

/// A runtime array view used during VM execution.
/// More dynamic than compile-time ArrayView - supports incremental building.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct RuntimeView {
    /// Base offset: either variable offset in curr[] or temp_id for temps
    pub base_off: u32,
    /// true = base_off is a temp_id, false = base_off is offset in curr[]
    pub is_temp: bool,
    /// Dimension sizes for this view
    pub dims: SmallVec<[u16; 4]>,
    /// Strides for each dimension (signed to support transpose)
    pub strides: SmallVec<[i32; 4]>,
    /// Starting offset within the base array
    pub offset: u32,
    /// Sparse dimension mappings (empty if fully contiguous)
    pub sparse: SmallVec<[RuntimeSparseMapping; 2]>,
    /// Dimension IDs for broadcasting (to match dimensions by name)
    pub dim_ids: SmallVec<[DimId; 4]>,
    /// Whether this view is valid (false if out-of-bounds subscript was applied)
    pub is_valid: bool,
}

impl RuntimeView {
    /// Create a new contiguous view for a variable
    pub fn for_var(base_off: u32, dims: SmallVec<[u16; 4]>, dim_ids: SmallVec<[DimId; 4]>) -> Self {
        let mut strides = SmallVec::new();
        let mut stride = 1i32;
        // Build strides from right to left for row-major order
        for &dim in dims.iter().rev() {
            strides.push(stride);
            stride *= dim as i32;
        }
        strides.reverse();

        RuntimeView {
            base_off,
            is_temp: false,
            dims,
            strides,
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids,
            is_valid: true,
        }
    }

    /// Create a new contiguous view for a temp array
    pub fn for_temp(
        temp_id: TempId,
        dims: SmallVec<[u16; 4]>,
        dim_ids: SmallVec<[DimId; 4]>,
    ) -> Self {
        let mut view = Self::for_var(temp_id as u32, dims, dim_ids);
        view.is_temp = true;
        view
    }

    /// Create an invalid view (for out-of-bounds subscripts)
    #[allow(dead_code)]
    pub fn invalid() -> Self {
        RuntimeView {
            base_off: 0,
            is_temp: false,
            dims: SmallVec::new(),
            strides: SmallVec::new(),
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::new(),
            is_valid: false,
        }
    }

    /// Mark this view as invalid (e.g., after out-of-bounds subscript)
    #[allow(dead_code)]
    pub fn mark_invalid(&mut self) {
        self.is_valid = false;
    }

    /// Apply a single-element subscript with bounds checking.
    /// If index is out of bounds (using 1-based indexing), marks view as invalid.
    /// Returns true if subscript was valid, false otherwise.
    pub fn apply_single_subscript_checked(&mut self, dim_idx: usize, index_1based: u16) -> bool {
        if dim_idx >= self.dims.len() {
            self.is_valid = false;
            return false;
        }

        // Check bounds (1-based: valid range is 1..=dim_size)
        if index_1based == 0 || index_1based > self.dims[dim_idx] {
            self.is_valid = false;
            return false;
        }

        // Convert to 0-based and apply
        let index_0based = index_1based - 1;
        self.apply_single_subscript(dim_idx, index_0based);
        true
    }

    /// Total number of elements in this view
    pub fn size(&self) -> usize {
        self.dims.iter().map(|&d| d as usize).product()
    }

    /// Check if the view is contiguous (no sparse mappings, offset 0, standard strides)
    pub fn is_contiguous(&self) -> bool {
        if self.offset != 0 || !self.sparse.is_empty() {
            return false;
        }

        let mut expected_stride = 1i32;
        for i in (0..self.dims.len()).rev() {
            if self.strides[i] != expected_stride {
                return false;
            }
            expected_stride *= self.dims[i] as i32;
        }
        true
    }

    /// Compute the flat offset for a given multi-dimensional index.
    /// Takes into account strides, offset, and sparse mappings.
    pub fn flat_offset(&self, indices: &[u16]) -> usize {
        debug_assert_eq!(indices.len(), self.dims.len());

        let mut flat = self.offset as usize;

        // Build a quick lookup for sparse dimensions
        let sparse_lookup: SmallVec<[Option<&[u16]>; 4]> = (0..self.dims.len())
            .map(|i| {
                self.sparse
                    .iter()
                    .find(|s| s.dim_index as usize == i)
                    .map(|s| s.parent_offsets.as_slice())
            })
            .collect();

        for (i, &idx) in indices.iter().enumerate() {
            let actual_idx = if let Some(offsets) = sparse_lookup[i] {
                // Sparse dimension: map through parent_offsets
                offsets[idx as usize] as usize
            } else {
                idx as usize
            };
            flat += actual_idx * self.strides[i] as usize;
        }

        flat
    }

    /// Apply a single-element subscript, removing one dimension
    pub fn apply_single_subscript(&mut self, dim_idx: usize, index: u16) {
        debug_assert!(dim_idx < self.dims.len());
        debug_assert!(index < self.dims[dim_idx]);

        // Check if this dimension is sparse
        let actual_index = if let Some(pos) = self
            .sparse
            .iter()
            .position(|s| s.dim_index as usize == dim_idx)
        {
            let mapping = &self.sparse[pos];
            let parent_idx = mapping.parent_offsets[index as usize];
            // Remove the sparse mapping since we're collapsing this dimension
            self.sparse.remove(pos);
            parent_idx
        } else {
            index
        };

        // Adjust offset
        self.offset += actual_index as u32 * self.strides[dim_idx] as u32;

        // Remove the dimension
        self.dims.remove(dim_idx);
        self.strides.remove(dim_idx);
        self.dim_ids.remove(dim_idx);

        // Adjust sparse dim_index for any mappings after this dimension
        for s in &mut self.sparse {
            if s.dim_index as usize > dim_idx {
                s.dim_index -= 1;
            }
        }
    }

    /// Apply a range subscript [start:end) to a dimension
    pub fn apply_range(&mut self, dim_idx: usize, start: u16, end: u16) {
        debug_assert!(dim_idx < self.dims.len());
        debug_assert!(start < end && end <= self.dims[dim_idx]);

        // Adjust offset for start
        self.offset += start as u32 * self.strides[dim_idx] as u32;
        // Adjust dimension size
        self.dims[dim_idx] = end - start;
    }

    /// Apply a range subscript with bounds checking using 1-based indices.
    ///
    /// Handles invalid ranges gracefully:
    /// - If dim_idx is out of bounds, marks view as invalid
    /// - If start_1based is 0 or > dim size, clamps to valid range
    /// - If end_1based > dim size, clamps to dim size
    /// - If range is empty or reversed (start >= end), marks view as invalid
    ///
    /// Returns true if a valid range was applied, false otherwise.
    pub fn apply_range_checked(
        &mut self,
        dim_idx: usize,
        start_1based: u16,
        end_1based: u16,
    ) -> bool {
        // Check dimension index is valid
        if dim_idx >= self.dims.len() {
            self.is_valid = false;
            return false;
        }

        let dim_size = self.dims[dim_idx];

        // Convert 1-based inclusive range to 0-based exclusive range
        // XMILE: arr[3:7] means elements 3,4,5,6,7 (1-based inclusive)
        // Internal: [2, 7) means indices 2,3,4,5,6 (0-based exclusive)

        // Handle invalid start (0 is invalid in 1-based indexing)
        let start_0based = if start_1based == 0 {
            // Invalid start index, treat as 0 but mark for potential issues
            0
        } else {
            start_1based.saturating_sub(1)
        };

        // Clamp end to dimension size (end_1based is inclusive, so use as-is for exclusive bound)
        let end_0based = end_1based.min(dim_size);

        // Check for empty or reversed range.
        // This also catches out-of-bounds start: if start_0based >= dim_size,
        // then start_0based >= end_0based (since end_0based <= dim_size).
        if start_0based >= end_0based {
            self.dims[dim_idx] = 0;
            self.is_valid = false;
            return false;
        }

        // At this point we have the invariant:
        // 0 <= start_0based < end_0based <= dim_size
        // So start_0based is a valid index into this dimension.
        self.offset += start_0based as u32 * self.strides[dim_idx] as u32;
        self.dims[dim_idx] = end_0based - start_0based;
        true
    }

    /// Compute the flat memory offset for a given iteration index.
    /// This converts a flat iteration index (0..size) to multi-dimensional indices,
    /// then uses flat_offset to compute the memory location.
    pub fn offset_for_iter_index(&self, iter_idx: usize) -> usize {
        if self.dims.is_empty() {
            // Scalar view
            return self.offset as usize;
        }

        // For contiguous views with no sparse mappings, we can compute directly
        if self.sparse.is_empty() && self.is_contiguous() {
            return self.offset as usize + iter_idx;
        }

        // Convert flat index to multi-dimensional indices
        let mut indices: SmallVec<[u16; 4]> = SmallVec::new();
        let mut remaining = iter_idx;

        // Compute indices from last dimension to first
        for &dim in self.dims.iter().rev() {
            indices.push((remaining % dim as usize) as u16);
            remaining /= dim as usize;
        }
        indices.reverse();

        self.flat_offset(&indices)
    }

    /// Apply a sparse subscript (star range) to a dimension
    pub fn apply_sparse(&mut self, dim_idx: usize, parent_offsets: SmallVec<[u16; 16]>) {
        debug_assert!(dim_idx < self.dims.len());

        // Update dimension size
        self.dims[dim_idx] = parent_offsets.len() as u16;

        // Add sparse mapping
        self.sparse.push(RuntimeSparseMapping {
            dim_index: dim_idx as u8,
            parent_offsets,
        });
    }

    /// Apply a sparse subscript (star range) and update the dimension ID.
    /// Used for star ranges like `*:SubDim` where the view's dimension should
    /// be relabeled with the subdimension ID for proper broadcast matching.
    pub fn apply_sparse_with_dim_id(
        &mut self,
        dim_idx: usize,
        parent_offsets: SmallVec<[u16; 16]>,
        new_dim_id: DimId,
    ) {
        self.apply_sparse(dim_idx, parent_offsets);
        // Update the dimension ID to the subdimension
        self.dim_ids[dim_idx] = new_dim_id;
    }

    /// Transpose the view (reverse dimensions and strides)
    pub fn transpose(&mut self) {
        self.dims.reverse();
        self.strides.reverse();
        self.dim_ids.reverse();
        // Also need to renumber sparse dim_indices
        let n = self.dims.len();
        for s in &mut self.sparse {
            s.dim_index = (n - 1 - s.dim_index as usize) as u8;
        }
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone)]
pub(crate) enum BuiltinId {
    Abs,
    Arccos,
    Arcsin,
    Arctan,
    Cos,
    Exp,
    Inf,
    Int,
    Ln,
    Log10,
    Max,
    Min,
    Pi,
    Pulse,
    Ramp,
    SafeDiv,
    Sign,
    Sin,
    Sqrt,
    Step,
    Tan,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Copy, Clone)]
pub(crate) enum Op2 {
    Add,
    Sub,
    Exp,
    Mul,
    Div,
    Mod,
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
    And,
    Or,
}

// ============================================================================
// Opcodes
// ============================================================================

/// Bytecode opcodes for the VM.
///
/// The opcodes are organized into categories:
/// - Arithmetic and logic (Op2, Not, etc.)
/// - Variable access (LoadVar, LoadGlobalVar, etc.)
/// - Control flow (SetCond, If, Ret)
/// - Module operations (EvalModule, LoadModuleInput)
/// - Assignment (AssignCurr, AssignNext)
/// - Builtins and lookups (Apply, Lookup)
/// - Array view stack operations (PushVarView, ViewSubscript*, etc.)
/// - Array iteration (BeginIter, LoadIterElement, etc.)
/// - Array reductions (ArraySum, ArrayMax, etc.)
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy)]
#[allow(dead_code)] // Array opcodes not yet emitted by compiler
pub(crate) enum Opcode {
    // === ARITHMETIC & LOGIC ===
    Op2 {
        op: Op2,
    },
    Not {},

    // === CONSTANTS & VARIABLES ===
    LoadConstant {
        id: LiteralId,
    },
    LoadVar {
        off: VariableOffset,
    },
    LoadGlobalVar {
        off: VariableOffset,
    },

    // === LEGACY SUBSCRIPT (dynamic, for backward compatibility) ===
    PushSubscriptIndex {
        bounds: VariableOffset,
    },
    LoadSubscript {
        off: VariableOffset,
    },

    // === CONTROL FLOW ===
    SetCond {},
    If {},
    Ret,

    // === MODULES ===
    LoadModuleInput {
        input: ModuleInputOffset,
    },
    EvalModule {
        id: ModuleId,
        n_inputs: u8,
    },

    // === ASSIGNMENT ===
    AssignCurr {
        off: VariableOffset,
    },
    AssignNext {
        off: VariableOffset,
    },

    // === BUILTINS & LOOKUPS ===
    Apply {
        func: BuiltinId,
    },
    /// Lookup a value in a graphical function table.
    /// Stack: [..., element_offset, lookup_index] -> [..., result]
    /// The actual table used is graphical_functions[base_gf + element_offset].
    /// For scalar tables, element_offset is always 0.
    /// If element_offset >= table_count, returns NaN.
    Lookup {
        base_gf: GraphicalFunctionId,
        /// Number of tables for this variable (1 for scalars, n for arrayed).
        /// Used for bounds checking at runtime.
        table_count: u16,
        /// Interpolation mode: Interpolate (linear), Forward (step up), Backward (step down)
        mode: LookupMode,
    },

    // === SUPERINSTRUCTIONS (fused opcodes for common patterns) ===
    /// Fused LoadConstant + AssignCurr.
    /// curr[module_off + off] = literals[literal_id]; stack unchanged.
    AssignConstCurr {
        off: VariableOffset,
        literal_id: LiteralId,
    },

    /// Fused Op2 + AssignCurr.
    /// Pops two values, applies binary op, assigns result to curr[module_off + off].
    BinOpAssignCurr {
        op: Op2,
        off: VariableOffset,
    },

    /// Fused Op2 + AssignNext.
    /// Pops two values, applies binary op, assigns result to next[module_off + off].
    BinOpAssignNext {
        op: Op2,
        off: VariableOffset,
    },

    // =========================================================================
    // ARRAY SUPPORT (new)
    // =========================================================================

    // === VIEW STACK: Building views dynamically ===
    /// Push a view for a variable's full array onto the view stack.
    /// Looks up dimension info to compute strides.
    /// The dim_list_id references a (n_dims, [DimId; 4]) entry in ByteCodeContext.dim_lists.
    PushVarView {
        base_off: VariableOffset,
        dim_list_id: DimListId,
    },

    /// Push a view for a temp array onto the view stack.
    /// The dim_list_id references a (n_dims, [DimId; 4]) entry in ByteCodeContext.dim_lists.
    PushTempView {
        temp_id: TempId,
        dim_list_id: DimListId,
    },

    /// Push a pre-computed static view onto the view stack.
    PushStaticView {
        view_id: ViewId,
    },

    /// Push a view for a variable with explicit dimension sizes.
    /// Used when we have bounds but not dim_ids (e.g., dynamic subscripts).
    /// The dim_list_id references a (n_dims, [u16; 4]) entry in ByteCodeContext.dim_lists.
    PushVarViewDirect {
        base_off: VariableOffset,
        dim_list_id: DimListId,
    },

    /// Apply single-element subscript with constant index to top view.
    /// Removes one dimension from the view.
    ViewSubscriptConst {
        dim_idx: u8,
        index: u16,
    },

    /// Apply single-element subscript with dynamic index (from arithmetic stack).
    /// Removes one dimension from the view.
    ViewSubscriptDynamic {
        dim_idx: u8,
    },

    /// Apply range subscript [start:end) to a dimension with static bounds.
    ViewRange {
        dim_idx: u8,
        start: u16,
        end: u16,
    },

    /// Apply range subscript with dynamic bounds (from stack).
    /// Pops end then start from arithmetic stack (1-based indices).
    /// Applies range to the specified dimension of the top view.
    ViewRangeDynamic {
        dim_idx: u8,
    },

    /// Apply star range (*:subdim) using subdimension relation.
    /// Converts dimension to sparse iteration.
    ViewStarRange {
        dim_idx: u8,
        subdim_relation_id: u16,
    },

    /// Apply wildcard [*] - keeps dimension as-is (explicit no-op for clarity).
    ViewWildcard {
        dim_idx: u8,
    },

    /// Transpose the top view (reverse dims and strides).
    ViewTranspose {},

    /// Pop and discard the top view from view stack.
    PopView {},

    /// Duplicate the top view on the view stack.
    DupView {},

    // === TEMP ARRAY ACCESS ===
    /// Load single element from temp array with constant index.
    LoadTempConst {
        temp_id: TempId,
        index: u16,
    },

    /// Load single element from temp array with dynamic index (from stack).
    LoadTempDynamic {
        temp_id: TempId,
    },

    // === ITERATION ===
    /// Begin iteration over top view, optionally writing results to a temp.
    /// If write_temp is Some, iteration writes to that temp array.
    /// View remains on stack during iteration.
    BeginIter {
        write_temp_id: TempId,
        has_write_temp: bool,
    },

    /// Load element at current iteration position from the iteration source.
    /// Pushes value onto arithmetic stack.
    LoadIterElement {},

    /// Load element at current iteration position from a temp array.
    LoadIterTempElement {
        temp_id: TempId,
    },

    /// Load element from top view at current iteration index.
    /// Unlike LoadIterElement, this uses the view currently on top of view_stack,
    /// not the view captured when BeginIter was called.
    /// This allows loading from multiple different source arrays in a single iteration loop.
    LoadIterViewTop {},

    /// Load element at current iteration position from view at stack offset.
    /// Like LoadIterViewTop but accesses a specific view on the stack by offset.
    /// offset=1 means top of stack, offset=2 means second from top, etc.
    /// This allows views to be pushed before the loop and accessed inside without
    /// repeated push/pop operations per iteration.
    LoadIterViewAt {
        offset: u8,
    },

    /// Store top of arithmetic stack to current iteration position in dest temp.
    /// Pops value from arithmetic stack.
    StoreIterElement {},

    /// Advance iterator. If not done, jump backward by offset; else continue.
    /// jump_back is negative offset from current PC.
    NextIterOrJump {
        jump_back: PcOffset,
    },

    /// End iteration and clean up (pops iteration context, view stays on stack).
    EndIter {},

    // === ARRAY REDUCTIONS ===
    // These operate on the top view and push result to arithmetic stack.
    // View is NOT popped (caller should PopView after if needed).
    /// Sum all elements in top view.
    ArraySum {},

    /// Maximum of all elements in top view.
    ArrayMax {},

    /// Minimum of all elements in top view.
    ArrayMin {},

    /// Mean of all elements in top view.
    ArrayMean {},

    /// Standard deviation of all elements in top view.
    ArrayStddev {},

    /// Size (element count) of top view.
    ArraySize {},

    // === BROADCASTING ITERATION ===
    // For operations like A[DimA, DimB] * B[DimA] where dims must match by name.
    /// Begin broadcast iteration over multiple source views.
    /// Expects n_sources views on the view stack.
    /// Result dimensions computed from dimension name matching.
    BeginBroadcastIter {
        n_sources: u8,
        dest_temp_id: TempId,
    },

    /// Load element from source view at broadcast-computed index.
    /// source_idx is 0-based index into the source views (0 = deepest on stack).
    LoadBroadcastElement {
        source_idx: u8,
    },

    /// Store result to current broadcast position and advance.
    StoreBroadcastElement {},

    /// Check if broadcast done; if not, jump backward.
    NextBroadcastOrJump {
        jump_back: PcOffset,
    },

    /// End broadcast iteration, clean up.
    EndBroadcastIter {},
}

impl Opcode {
    /// Returns the jump offset if this opcode is a backward jump instruction.
    /// Centralizes jump handling so new jump opcodes can't be silently missed
    /// by the peephole optimizer or other passes.
    fn jump_offset(&self) -> Option<PcOffset> {
        match self {
            Opcode::NextIterOrJump { jump_back } | Opcode::NextBroadcastOrJump { jump_back } => {
                Some(*jump_back)
            }
            _ => None,
        }
    }

    /// Mutably borrow the jump offset, if this opcode is a backward jump.
    fn jump_offset_mut(&mut self) -> Option<&mut PcOffset> {
        match self {
            Opcode::NextIterOrJump { jump_back } | Opcode::NextBroadcastOrJump { jump_back } => {
                Some(jump_back)
            }
            _ => None,
        }
    }

    /// Returns (pops, pushes) describing this opcode's effect on the arithmetic stack.
    /// Used by `ByteCode::max_stack_depth` to statically validate that compiled
    /// bytecode cannot overflow the fixed-size VM stack.
    ///
    /// Opcodes that only affect the view stack, iter stack, or broadcast stack
    /// return (0, 0) since they don't touch the arithmetic stack.
    fn stack_effect(&self) -> (u8, u8) {
        match self {
            // Arithmetic: pop 2, push 1
            Opcode::Op2 { .. } => (2, 1),
            // Logic: pop 1, push 1
            Opcode::Not {} => (1, 1),

            // Constants/variables: push 1
            Opcode::LoadConstant { .. }
            | Opcode::LoadVar { .. }
            | Opcode::LoadGlobalVar { .. }
            | Opcode::LoadModuleInput { .. } => (0, 1),

            // Legacy subscript: PushSubscriptIndex pops the index value
            Opcode::PushSubscriptIndex { .. } => (1, 0),
            // LoadSubscript pushes the looked-up value
            Opcode::LoadSubscript { .. } => (0, 1),

            // Control flow
            Opcode::SetCond {} => (1, 0),       // pops condition
            Opcode::If {} => (2, 1),             // pops true+false branches, pushes result
            Opcode::Ret => (0, 0),

            // Module eval: pops n_inputs, pushes 0
            Opcode::EvalModule { n_inputs, .. } => (*n_inputs, 0),

            // Assignment: pops 1 (the value to assign)
            Opcode::AssignCurr { .. } | Opcode::AssignNext { .. } => (1, 0),

            // Builtins always take 3 args (actual + padding), push 1 result
            Opcode::Apply { .. } => (3, 1),
            // Lookup pops element_offset and lookup_index, pushes result
            Opcode::Lookup { .. } => (2, 1),

            // Superinstructions
            Opcode::AssignConstCurr { .. } => (0, 0),   // reads literal directly
            Opcode::BinOpAssignCurr { .. } => (2, 0),    // pops 2, assigns directly
            Opcode::BinOpAssignNext { .. } => (2, 0),    // pops 2, assigns directly

            // View stack ops don't touch arithmetic stack
            Opcode::PushVarView { .. }
            | Opcode::PushTempView { .. }
            | Opcode::PushStaticView { .. }
            | Opcode::PushVarViewDirect { .. }
            | Opcode::ViewSubscriptConst { .. }
            | Opcode::ViewRange { .. }
            | Opcode::ViewStarRange { .. }
            | Opcode::ViewWildcard { .. }
            | Opcode::ViewTranspose {}
            | Opcode::PopView {}
            | Opcode::DupView {} => (0, 0),

            // Dynamic subscript/range ops pop from arithmetic stack
            Opcode::ViewSubscriptDynamic { .. } => (1, 0),
            Opcode::ViewRangeDynamic { .. } => (2, 0),

            // Temp array access
            Opcode::LoadTempConst { .. } => (0, 1),
            Opcode::LoadTempDynamic { .. } => (1, 1),  // pops index, pushes value

            // Iteration: BeginIter/EndIter don't touch arithmetic stack
            Opcode::BeginIter { .. } | Opcode::EndIter {} => (0, 0),
            // LoadIter* push 1 element
            Opcode::LoadIterElement {}
            | Opcode::LoadIterTempElement { .. }
            | Opcode::LoadIterViewTop {}
            | Opcode::LoadIterViewAt { .. } => (0, 1),
            // StoreIterElement pops 1 value
            Opcode::StoreIterElement {} => (1, 0),
            // NextIter doesn't touch arithmetic stack
            Opcode::NextIterOrJump { .. } => (0, 0),

            // Array reductions push 1 result
            Opcode::ArraySum {}
            | Opcode::ArrayMax {}
            | Opcode::ArrayMin {}
            | Opcode::ArrayMean {}
            | Opcode::ArrayStddev {}
            | Opcode::ArraySize {} => (0, 1),

            // Broadcasting
            Opcode::BeginBroadcastIter { .. } | Opcode::EndBroadcastIter {} => (0, 0),
            Opcode::LoadBroadcastElement { .. } => (0, 1),
            Opcode::StoreBroadcastElement {} => (1, 0),
            Opcode::NextBroadcastOrJump { .. } => (0, 0),
        }
    }
}

// ============================================================================
// Module and Array Declarations
// ============================================================================

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct ModuleDeclaration {
    pub(crate) model_name: Ident<Canonical>,
    /// The set of input names for this module instantiation.
    /// Different instantiations of the same model with different input sets
    /// need separate compiled modules (the ModuleInput offsets differ).
    pub(crate) input_set: BTreeSet<Ident<Canonical>>,
    pub(crate) off: usize, // offset within the parent module
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
#[allow(dead_code)]
pub struct ArrayDefinition {
    pub(crate) dimensions: Vec<usize>,
}

/// A static array view for compile-time known subscripts.
/// Stored in ByteCodeContext and referenced by ViewId.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct StaticArrayView {
    /// Base variable offset in curr[]
    pub base_off: u32,
    /// true = base_off is a temp_id, false = base_off is offset in curr[]
    pub is_temp: bool,
    /// Dimension sizes
    pub dims: SmallVec<[u16; 4]>,
    /// Strides for each dimension
    pub strides: SmallVec<[i32; 4]>,
    /// Starting offset within the base array
    pub offset: u32,
    /// Sparse dimension mappings
    pub sparse: SmallVec<[RuntimeSparseMapping; 2]>,
    /// Dimension IDs for broadcasting
    pub dim_ids: SmallVec<[DimId; 4]>,
}

impl StaticArrayView {
    /// Convert to a RuntimeView for use on the view stack
    pub fn to_runtime_view(&self) -> RuntimeView {
        RuntimeView {
            base_off: self.base_off,
            is_temp: self.is_temp,
            dims: self.dims.clone(),
            strides: self.strides.clone(),
            offset: self.offset,
            sparse: self.sparse.clone(),
            dim_ids: self.dim_ids.clone(),
            is_valid: true,
        }
    }
}

// ============================================================================
// ByteCode Context (shared across runlists)
// ============================================================================

/// Context data shared across all bytecode runlists in a module.
/// Contains tables that opcodes reference by index.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct ByteCodeContext {
    // === Existing fields ===
    /// Graphical function lookup tables
    pub(crate) graphical_functions: Vec<Vec<(f64, f64)>>,
    /// Module declarations for nested modules
    pub(crate) modules: Vec<ModuleDeclaration>,
    /// Legacy array definitions (deprecated, use dimensions instead)
    #[allow(dead_code)]
    pub(crate) arrays: Vec<ArrayDefinition>,

    // === New array support fields ===
    /// Dimension information table (indexed by DimId)
    pub(crate) dimensions: Vec<DimensionInfo>,
    /// Subdimension relationships for star ranges
    pub(crate) subdim_relations: Vec<SubdimensionRelation>,
    /// Interned names table (dimension names, element names)
    #[allow(dead_code)] // Used by array bytecode not yet emitted
    pub(crate) names: Vec<String>,
    /// Pre-computed static views (indexed by ViewId)
    pub(crate) static_views: Vec<StaticArrayView>,

    // === Temp array info ===
    /// Offset of each temp array in temp_storage (indexed by TempId)
    pub(crate) temp_offsets: Vec<usize>,
    /// Total size needed for temp_storage
    pub(crate) temp_total_size: usize,

    // === Dim list side table ===
    /// Packed (n_dims, [DimId or u16; 4]) entries referenced by DimListId.
    /// Each entry stores the dimension count and up to 4 IDs.
    pub(crate) dim_lists: Vec<(u8, [u16; 4])>,
}

#[allow(dead_code)] // Methods used by array bytecode not yet emitted
impl ByteCodeContext {
    /// Intern a name (dimension or element name) and return its NameId.
    /// If the name already exists, returns the existing ID.
    pub fn intern_name(&mut self, name: &str) -> NameId {
        if let Some(pos) = self.names.iter().position(|n| n == name) {
            return pos as NameId;
        }
        self.names.push(name.to_string());
        (self.names.len() - 1) as NameId
    }

    /// Get a name by its ID.
    pub fn get_name(&self, id: NameId) -> Option<&str> {
        self.names.get(id as usize).map(|s| s.as_str())
    }

    /// Add a dimension and return its DimId.
    pub fn add_dimension(&mut self, info: DimensionInfo) -> DimId {
        self.dimensions.push(info);
        (self.dimensions.len() - 1) as DimId
    }

    /// Get dimension info by ID.
    pub fn get_dimension(&self, id: DimId) -> Option<&DimensionInfo> {
        self.dimensions.get(id as usize)
    }

    /// Add a subdimension relation and return its index.
    pub fn add_subdim_relation(&mut self, relation: SubdimensionRelation) -> u16 {
        self.subdim_relations.push(relation);
        (self.subdim_relations.len() - 1) as u16
    }

    /// Add a static view and return its ViewId.
    pub fn add_static_view(&mut self, view: StaticArrayView) -> ViewId {
        self.static_views.push(view);
        (self.static_views.len() - 1) as ViewId
    }

    /// Get a static view by ID.
    pub fn get_static_view(&self, id: ViewId) -> Option<&StaticArrayView> {
        self.static_views.get(id as usize)
    }

    /// Set up temp array info.
    pub fn set_temp_info(&mut self, offsets: Vec<usize>, total_size: usize) {
        self.temp_offsets = offsets;
        self.temp_total_size = total_size;
    }

    /// Find dimension ID by name.
    pub fn find_dimension_by_name(&self, name: &str) -> Option<DimId> {
        for (i, dim) in self.dimensions.iter().enumerate() {
            if let Some(dim_name) = self.get_name(dim.name_id)
                && dim_name == name
            {
                return Some(i as DimId);
            }
        }
        None
    }

    /// Add a dim list entry (n_dims + up to 4 IDs) and return its DimListId.
    pub fn add_dim_list(&mut self, n_dims: u8, ids: [u16; 4]) -> DimListId {
        self.dim_lists.push((n_dims, ids));
        (self.dim_lists.len() - 1) as DimListId
    }

    /// Get a dim list entry by ID.
    ///
    /// Panics on out-of-bounds ID, which is intentional: IDs are only produced
    /// by `add_dim_list` during compilation, so an invalid ID indicates a
    /// compiler bug that should surface immediately rather than be silently
    /// converted to a default value.
    pub fn get_dim_list(&self, id: DimListId) -> (u8, &[u16; 4]) {
        let (n, ref ids) = self.dim_lists[id as usize];
        (n, ids)
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct ByteCode {
    pub(crate) literals: Vec<f64>,
    pub(crate) code: Vec<Opcode>,
}

impl ByteCode {
    /// Statically compute the maximum arithmetic stack depth reached by this bytecode.
    ///
    /// Walks the opcode stream applying each instruction's stack effect. Because
    /// SD expressions are straight-line (no conditional jumps that could create
    /// divergent stack depths -- backward jumps from iteration opcodes always
    /// return to the same stack depth), a single linear pass is sufficient.
    pub(crate) fn max_stack_depth(&self) -> usize {
        let mut depth: usize = 0;
        let mut max_depth: usize = 0;
        for (pc, op) in self.code.iter().enumerate() {
            let (pops, pushes) = op.stack_effect();
            // Use checked_sub rather than saturating_sub: an underflow here
            // means stack_effect() metadata is wrong for some opcode, which
            // would silently invalidate our safety proof. Panicking surfaces
            // the bug immediately in tests.
            depth = depth.checked_sub(pops as usize).unwrap_or_else(|| {
                panic!(
                    "stack_effect underflow at pc {pc}: {pops} pops but depth is {depth}"
                )
            });
            depth += pushes as usize;
            max_depth = max_depth.max(depth);
        }
        max_depth
    }
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Default)]
pub struct ByteCodeBuilder {
    bytecode: ByteCode,
    interned_literals: HashMap<OrderedFloat<f64>, LiteralId>,
}

impl ByteCodeBuilder {
    pub(crate) fn intern_literal(&mut self, lit: f64) -> LiteralId {
        let key: OrderedFloat<f64> = lit.into();
        if self.interned_literals.contains_key(&key) {
            return self.interned_literals[&key];
        }
        self.bytecode.literals.push(lit);
        let literal_id = (self.bytecode.literals.len() - 1) as u16;
        self.interned_literals.insert(key, literal_id);
        literal_id
    }

    pub(crate) fn push_opcode(&mut self, op: Opcode) {
        self.bytecode.code.push(op)
    }

    /// Returns the current number of opcodes in the bytecode
    pub(crate) fn len(&self) -> usize {
        self.bytecode.code.len()
    }

    pub(crate) fn finish(self) -> ByteCode {
        let mut bc = self.bytecode;
        bc.peephole_optimize();

        // Validate that the compiled bytecode cannot overflow the VM's
        // fixed-size stack. This makes the unsafe unchecked stack access
        // in the VM provably safe for this bytecode.
        let depth = bc.max_stack_depth();
        assert!(
            depth < STACK_CAPACITY,
            "compiled bytecode requires stack depth {depth}, exceeding VM capacity {STACK_CAPACITY}"
        );

        bc
    }
}

impl ByteCode {
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
                if target < jump_targets.len() {
                    jump_targets[target] = true;
                }
            }
        }

        // 2. Build old_pc -> new_pc mapping and fused output.
        // pc_map has one entry per original instruction so that jump fixup
        // can index by the original PC directly.
        let mut optimized: Vec<Opcode> = Vec::with_capacity(self.code.len());
        let mut pc_map: Vec<usize> = Vec::with_capacity(self.code.len() + 1);
        let mut i = 0;
        while i < self.code.len() {
            let new_pc = optimized.len();
            pc_map.push(new_pc);

            // Only try fusion if next instruction is not a jump target
            let can_fuse = i + 1 < self.code.len() && !jump_targets[i + 1];

            if can_fuse {
                let fused = match (&self.code[i], &self.code[i + 1]) {
                    // Pattern: LoadConstant + AssignCurr -> AssignConstCurr
                    (Opcode::LoadConstant { id }, Opcode::AssignCurr { off }) => {
                        Some(Opcode::AssignConstCurr {
                            off: *off,
                            literal_id: *id,
                        })
                    }
                    // Pattern: Op2 + AssignCurr -> BinOpAssignCurr
                    (Opcode::Op2 { op }, Opcode::AssignCurr { off }) => {
                        Some(Opcode::BinOpAssignCurr { op: *op, off: *off })
                    }
                    // Pattern: Op2 + AssignNext -> BinOpAssignNext
                    (Opcode::Op2 { op }, Opcode::AssignNext { off }) => {
                        Some(Opcode::BinOpAssignNext { op: *op, off: *off })
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
            optimized.push(self.code[i]);
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

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // ByteCode Builder Tests
    // =========================================================================

    #[test]
    fn test_memoizing_interning() {
        let mut bytecode = ByteCodeBuilder::default();
        let a1 = bytecode.intern_literal(1.0);
        let b1 = bytecode.intern_literal(1.01);
        let b2 = bytecode.intern_literal(1.01);
        let b3 = bytecode.intern_literal(1.01);
        let a2 = bytecode.intern_literal(1.0);
        let b4 = bytecode.intern_literal(1.01);

        assert_eq!(a1, a2);
        assert_eq!(b1, b2);
        assert_eq!(b1, b3);
        assert_eq!(b1, b4);
        assert_ne!(a1, b1);

        let bytecode = bytecode.finish();
        assert_eq!(2, bytecode.literals.len());
    }

    // =========================================================================
    // Stack Effect Tests
    // =========================================================================

    #[test]
    fn test_stack_effect_arithmetic_ops() {
        // Binary ops: pop 2, push 1
        assert_eq!((Opcode::Op2 { op: Op2::Add }).stack_effect(), (2, 1));
        assert_eq!((Opcode::Op2 { op: Op2::Mul }).stack_effect(), (2, 1));
        assert_eq!((Opcode::Op2 { op: Op2::Gt }).stack_effect(), (2, 1));

        // Unary not: pop 1, push 1
        assert_eq!((Opcode::Not {}).stack_effect(), (1, 1));
    }

    #[test]
    fn test_stack_effect_loads() {
        assert_eq!((Opcode::LoadConstant { id: 0 }).stack_effect(), (0, 1));
        assert_eq!((Opcode::LoadVar { off: 0 }).stack_effect(), (0, 1));
        assert_eq!((Opcode::LoadGlobalVar { off: 0 }).stack_effect(), (0, 1));
        assert_eq!((Opcode::LoadModuleInput { input: 0 }).stack_effect(), (0, 1));
    }

    #[test]
    fn test_stack_effect_assignments() {
        assert_eq!((Opcode::AssignCurr { off: 0 }).stack_effect(), (1, 0));
        assert_eq!((Opcode::AssignNext { off: 0 }).stack_effect(), (1, 0));
    }

    #[test]
    fn test_stack_effect_superinstructions() {
        assert_eq!(
            (Opcode::AssignConstCurr {
                off: 0,
                literal_id: 0
            })
            .stack_effect(),
            (0, 0)
        );
        assert_eq!(
            (Opcode::BinOpAssignCurr {
                op: Op2::Add,
                off: 0
            })
            .stack_effect(),
            (2, 0)
        );
        assert_eq!(
            (Opcode::BinOpAssignNext {
                op: Op2::Add,
                off: 0
            })
            .stack_effect(),
            (2, 0)
        );
    }

    #[test]
    fn test_stack_effect_builtins() {
        assert_eq!((Opcode::Apply { func: BuiltinId::Abs }).stack_effect(), (3, 1));
        assert_eq!(
            (Opcode::Lookup {
                base_gf: 0,
                table_count: 1,
                mode: LookupMode::Interpolate,
            })
            .stack_effect(),
            (2, 1)
        );
    }

    #[test]
    fn test_stack_effect_control_flow() {
        assert_eq!((Opcode::SetCond {}).stack_effect(), (1, 0));
        assert_eq!((Opcode::If {}).stack_effect(), (2, 1));
        assert_eq!(Opcode::Ret.stack_effect(), (0, 0));
    }

    #[test]
    fn test_stack_effect_eval_module() {
        assert_eq!(
            (Opcode::EvalModule {
                id: 0,
                n_inputs: 3,
            })
            .stack_effect(),
            (3, 0)
        );
        assert_eq!(
            (Opcode::EvalModule {
                id: 0,
                n_inputs: 0,
            })
            .stack_effect(),
            (0, 0)
        );
    }

    #[test]
    fn test_stack_effect_view_ops_dont_affect_arithmetic_stack() {
        assert_eq!(
            (Opcode::PushVarView {
                base_off: 0,
                dim_list_id: 0,
            })
            .stack_effect(),
            (0, 0)
        );
        assert_eq!((Opcode::PopView {}).stack_effect(), (0, 0));
        assert_eq!((Opcode::DupView {}).stack_effect(), (0, 0));
        assert_eq!(
            (Opcode::ViewSubscriptConst {
                dim_idx: 0,
                index: 0,
            })
            .stack_effect(),
            (0, 0)
        );
    }

    #[test]
    fn test_stack_effect_dynamic_view_ops_pop_from_arithmetic_stack() {
        assert_eq!(
            (Opcode::ViewSubscriptDynamic { dim_idx: 0 }).stack_effect(),
            (1, 0)
        );
        assert_eq!(
            (Opcode::ViewRangeDynamic { dim_idx: 0 }).stack_effect(),
            (2, 0)
        );
    }

    #[test]
    fn test_stack_effect_iteration() {
        assert_eq!(
            (Opcode::BeginIter {
                write_temp_id: 0,
                has_write_temp: false,
            })
            .stack_effect(),
            (0, 0)
        );
        assert_eq!((Opcode::LoadIterElement {}).stack_effect(), (0, 1));
        assert_eq!((Opcode::StoreIterElement {}).stack_effect(), (1, 0));
        assert_eq!((Opcode::EndIter {}).stack_effect(), (0, 0));
    }

    #[test]
    fn test_stack_effect_array_reductions() {
        assert_eq!((Opcode::ArraySum {}).stack_effect(), (0, 1));
        assert_eq!((Opcode::ArrayMax {}).stack_effect(), (0, 1));
        assert_eq!((Opcode::ArrayMin {}).stack_effect(), (0, 1));
        assert_eq!((Opcode::ArrayMean {}).stack_effect(), (0, 1));
        assert_eq!((Opcode::ArrayStddev {}).stack_effect(), (0, 1));
        assert_eq!((Opcode::ArraySize {}).stack_effect(), (0, 1));
    }

    // =========================================================================
    // Max Stack Depth Tests
    // =========================================================================

    #[test]
    fn test_max_stack_depth_empty() {
        let bc = ByteCode::default();
        assert_eq!(bc.max_stack_depth(), 0);
    }

    #[test]
    fn test_max_stack_depth_simple_assignment() {
        // x = 42.0: LoadConstant(42.0), AssignCurr(x)
        let bc = ByteCode {
            literals: vec![42.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 0 },
            ],
        };
        assert_eq!(bc.max_stack_depth(), 1);
    }

    #[test]
    fn test_max_stack_depth_binary_expression() {
        // x = a + b: LoadVar(a), LoadVar(b), Op2(Add), AssignCurr(x)
        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 2 },
            ],
        };
        assert_eq!(bc.max_stack_depth(), 2);
    }

    #[test]
    fn test_max_stack_depth_nested_expression() {
        // x = (a + b) * (c + d):
        // LoadVar(a), LoadVar(b), Op2(Add), LoadVar(c), LoadVar(d), Op2(Add), Op2(Mul), AssignCurr
        // Peak depth is 3: after loading c while (a+b) result is still on stack
        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },     // depth: 1
                Opcode::LoadVar { off: 1 },     // depth: 2
                Opcode::Op2 { op: Op2::Add },   // depth: 1
                Opcode::LoadVar { off: 2 },     // depth: 2
                Opcode::LoadVar { off: 3 },     // depth: 3 (peak)
                Opcode::Op2 { op: Op2::Add },   // depth: 2
                Opcode::Op2 { op: Op2::Mul },   // depth: 1
                Opcode::AssignCurr { off: 4 },  // depth: 0
            ],
        };
        assert_eq!(bc.max_stack_depth(), 3);
    }

    #[test]
    fn test_max_stack_depth_builtin_function() {
        // x = ABS(a): LoadVar(a), LoadConstant(0), LoadConstant(0), Apply(Abs), AssignCurr
        let bc = ByteCode {
            literals: vec![0.0],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 0 },
                Opcode::Apply { func: BuiltinId::Abs },
                Opcode::AssignCurr { off: 1 },
            ],
        };
        assert_eq!(bc.max_stack_depth(), 3);
    }

    #[test]
    fn test_max_stack_depth_if_expression() {
        // IF(cond, a, b): LoadVar(cond), SetCond, LoadVar(a), LoadVar(b), If, AssignCurr
        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },     // depth: 1
                Opcode::SetCond {},              // depth: 0
                Opcode::LoadVar { off: 1 },     // depth: 1
                Opcode::LoadVar { off: 2 },     // depth: 2
                Opcode::If {},                   // depth: 1
                Opcode::AssignCurr { off: 3 },  // depth: 0
            ],
        };
        assert_eq!(bc.max_stack_depth(), 2);
    }

    #[test]
    fn test_max_stack_depth_superinstruction_const_assign() {
        // AssignConstCurr doesn't use the stack at all
        let bc = ByteCode {
            literals: vec![42.0],
            code: vec![Opcode::AssignConstCurr {
                off: 0,
                literal_id: 0,
            }],
        };
        assert_eq!(bc.max_stack_depth(), 0);
    }

    #[test]
    fn test_max_stack_depth_multiple_assignments() {
        // x = a; y = b + c
        // Stack resets to 0 after each assignment, so peak is max of individual expressions
        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::AssignCurr { off: 3 },
                Opcode::LoadVar { off: 1 },
                Opcode::LoadVar { off: 2 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 4 },
            ],
        };
        assert_eq!(bc.max_stack_depth(), 2);
    }

    #[test]
    fn test_max_stack_depth_with_iteration() {
        // Iteration body: LoadIterElement, StoreIterElement -- each iteration
        // pushes 1 and pops 1, so peak depth within loop is 1
        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::BeginIter {
                    write_temp_id: 0,
                    has_write_temp: true,
                },
                Opcode::LoadIterElement {},
                Opcode::StoreIterElement {},
                Opcode::NextIterOrJump { jump_back: -2 },
                Opcode::EndIter {},
            ],
        };
        assert_eq!(bc.max_stack_depth(), 1);
    }

    #[test]
    fn test_finish_validates_stack_depth() {
        // Build bytecode that fits within STACK_CAPACITY -- should succeed
        let mut builder = ByteCodeBuilder::default();
        let id = builder.intern_literal(1.0);
        builder.push_opcode(Opcode::LoadConstant { id });
        builder.push_opcode(Opcode::AssignCurr { off: 0 });
        let _bc = builder.finish(); // should not panic
    }

    #[test]
    #[should_panic(expected = "stack_effect underflow at pc 0")]
    fn test_max_stack_depth_catches_underflow() {
        // An Op2 at the start with nothing on the stack should panic,
        // catching bugs in stack_effect metadata
        let bc = ByteCode {
            literals: vec![],
            code: vec![Opcode::Op2 { op: Op2::Add }],
        };
        bc.max_stack_depth();
    }

    // =========================================================================
    // Jump Offset Tests
    // =========================================================================

    #[test]
    fn test_jump_offset_returns_offset_for_jump_opcodes() {
        let iter_jump = Opcode::NextIterOrJump { jump_back: -5 };
        assert_eq!(iter_jump.jump_offset(), Some(-5));

        let broadcast_jump = Opcode::NextBroadcastOrJump { jump_back: -3 };
        assert_eq!(broadcast_jump.jump_offset(), Some(-3));

        assert_eq!(Opcode::Ret.jump_offset(), None);
        assert_eq!((Opcode::Op2 { op: Op2::Add }).jump_offset(), None);
        assert_eq!((Opcode::LoadVar { off: 0 }).jump_offset(), None);
    }

    #[test]
    fn test_jump_offset_mut_modifies_jump() {
        let mut op = Opcode::NextIterOrJump { jump_back: -5 };
        if let Some(offset) = op.jump_offset_mut() {
            *offset = -2;
        }
        assert_eq!(op.jump_offset(), Some(-2));
    }

    #[test]
    fn test_opcode_size() {
        use std::mem::size_of;
        // Large inline arrays ([DimId; 4]) moved to a side table, so
        // the largest variant payload is now ViewRange (u8 + u16 + u16 = 5 bytes)
        // or Lookup (u8 + u16 + u8 = 4 bytes). With discriminant, expect 8 bytes.
        let size = size_of::<Opcode>();
        assert!(size <= 8, "Opcode size {} exceeds 8 bytes", size);
        eprintln!("Opcode size: {} bytes", size);
    }

    // =========================================================================
    // DimensionInfo Tests
    // =========================================================================

    #[test]
    fn test_dimension_info_indexed() {
        let dim = DimensionInfo::indexed(0, 5);
        assert_eq!(dim.name_id, 0);
        assert_eq!(dim.size, 5);
        assert!(dim.is_indexed);
        assert!(dim.element_name_ids.is_empty());
    }

    #[test]
    fn test_dimension_info_named() {
        let element_ids: SmallVec<[NameId; 8]> = smallvec::smallvec![1, 2, 3];
        let dim = DimensionInfo::named(0, element_ids.clone());
        assert_eq!(dim.name_id, 0);
        assert_eq!(dim.size, 3);
        assert!(!dim.is_indexed);
        assert_eq!(dim.element_name_ids, element_ids);
    }

    // =========================================================================
    // SubdimensionRelation Tests
    // =========================================================================

    #[test]
    fn test_subdim_relation_contiguous() {
        let rel = SubdimensionRelation::contiguous(0, 1, 2, 3);
        assert_eq!(rel.parent_dim_id, 0);
        assert_eq!(rel.child_dim_id, 1);
        assert_eq!(rel.parent_offsets.as_slice(), &[2, 3, 4]);
        assert!(rel.is_contiguous);
        assert_eq!(rel.start_offset, 2);
    }

    #[test]
    fn test_subdim_relation_sparse() {
        let offsets: SmallVec<[u16; 16]> = smallvec::smallvec![0, 2, 5];
        let rel = SubdimensionRelation::sparse(0, 1, offsets);
        assert_eq!(rel.parent_dim_id, 0);
        assert_eq!(rel.child_dim_id, 1);
        assert_eq!(rel.parent_offsets.as_slice(), &[0, 2, 5]);
        assert!(!rel.is_contiguous);
        assert_eq!(rel.start_offset, 0);
    }

    #[test]
    fn test_subdim_relation_sparse_actually_contiguous() {
        // [1, 2, 3] is contiguous
        let offsets: SmallVec<[u16; 16]> = smallvec::smallvec![1, 2, 3];
        let rel = SubdimensionRelation::sparse(0, 1, offsets);
        assert!(rel.is_contiguous);
        assert_eq!(rel.start_offset, 1);
    }

    // =========================================================================
    // RuntimeView Tests
    // =========================================================================

    #[test]
    fn test_runtime_view_for_var_1d() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let view = RuntimeView::for_var(100, dims, dim_ids);

        assert_eq!(view.base_off, 100);
        assert!(!view.is_temp);
        assert_eq!(view.dims.as_slice(), &[5]);
        assert_eq!(view.strides.as_slice(), &[1]);
        assert_eq!(view.offset, 0);
        assert!(view.sparse.is_empty());
        assert_eq!(view.size(), 5);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_runtime_view_for_var_2d() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let view = RuntimeView::for_var(0, dims, dim_ids);

        assert_eq!(view.dims.as_slice(), &[3, 4]);
        assert_eq!(view.strides.as_slice(), &[4, 1]); // Row-major
        assert_eq!(view.size(), 12);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_runtime_view_for_var_3d() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![2, 3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1, 2];
        let view = RuntimeView::for_var(0, dims, dim_ids);

        assert_eq!(view.dims.as_slice(), &[2, 3, 4]);
        assert_eq!(view.strides.as_slice(), &[12, 4, 1]); // Row-major: 3*4=12, 4, 1
        assert_eq!(view.size(), 24);
        assert!(view.is_contiguous());
    }

    #[test]
    fn test_runtime_view_for_temp() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let view = RuntimeView::for_temp(3, dims, dim_ids);

        assert_eq!(view.base_off, 3);
        assert!(view.is_temp);
    }

    #[test]
    fn test_runtime_view_flat_offset_1d() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let view = RuntimeView::for_var(0, dims, dim_ids);

        assert_eq!(view.flat_offset(&[0]), 0);
        assert_eq!(view.flat_offset(&[2]), 2);
        assert_eq!(view.flat_offset(&[4]), 4);
    }

    #[test]
    fn test_runtime_view_flat_offset_2d() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let view = RuntimeView::for_var(0, dims, dim_ids);

        // [row, col] -> row * 4 + col
        assert_eq!(view.flat_offset(&[0, 0]), 0);
        assert_eq!(view.flat_offset(&[0, 3]), 3);
        assert_eq!(view.flat_offset(&[1, 0]), 4);
        assert_eq!(view.flat_offset(&[2, 3]), 11);
    }

    #[test]
    fn test_runtime_view_apply_single_subscript() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Apply subscript to first dimension: arr[1, *]
        view.apply_single_subscript(0, 1);

        assert_eq!(view.dims.as_slice(), &[4]); // Only second dimension remains
        assert_eq!(view.strides.as_slice(), &[1]);
        assert_eq!(view.offset, 4); // 1 * stride[0] = 1 * 4 = 4
        assert_eq!(view.dim_ids.as_slice(), &[1]); // Only dim 1 remains
    }

    #[test]
    fn test_runtime_view_apply_range() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Apply range to first dimension: arr[1:3, *]
        view.apply_range(0, 1, 3);

        assert_eq!(view.dims.as_slice(), &[2, 4]); // First dim now size 2
        assert_eq!(view.offset, 4); // 1 * stride[0] = 1 * 4 = 4
        assert!(!view.is_contiguous()); // offset != 0
    }

    #[test]
    fn test_runtime_view_apply_range_checked_valid() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![10];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Valid 1-based range [3:7] -> 0-based [2:7) -> elements 2,3,4,5,6 (5 elements)
        let result = view.apply_range_checked(0, 3, 7);

        assert!(result, "valid range should return true");
        assert!(view.is_valid, "view should be valid");
        assert_eq!(view.dims.as_slice(), &[5]); // 5 elements (7-2)
        assert_eq!(view.offset, 2); // start at index 2
    }

    #[test]
    fn test_runtime_view_apply_range_checked_single_element() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Single element range [3:3] -> 0-based [2:3) -> element 2 (1 element)
        let result = view.apply_range_checked(0, 3, 3);

        assert!(result, "single element range should be valid");
        assert!(view.is_valid);
        assert_eq!(view.dims.as_slice(), &[1]); // 1 element
        assert_eq!(view.offset, 2);
    }

    #[test]
    fn test_runtime_view_apply_range_checked_reversed() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![10];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Reversed range [7:3] should be invalid (start > end)
        let result = view.apply_range_checked(0, 7, 3);

        assert!(!result, "reversed range should return false");
        assert!(!view.is_valid, "view should be marked invalid");
        assert_eq!(view.dims.as_slice(), &[0]); // Empty dimension
    }

    #[test]
    fn test_runtime_view_apply_range_checked_out_of_bounds_end() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Range with end beyond bounds [3:100] should clamp to [3:5]
        // 1-based [3:100] -> 0-based [2:5) -> elements 2,3,4 (3 elements)
        let result = view.apply_range_checked(0, 3, 100);

        assert!(result, "out-of-bounds end should clamp and succeed");
        assert!(view.is_valid);
        assert_eq!(view.dims.as_slice(), &[3]); // 3 elements (5-2)
        assert_eq!(view.offset, 2);
    }

    #[test]
    fn test_runtime_view_apply_range_checked_zero_start() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Zero start [0:3] - invalid in 1-based, treated as [0:3) in 0-based
        // 0 is treated as start_0based=0, end_0based=3 -> elements 0,1,2 (3 elements)
        let result = view.apply_range_checked(0, 0, 3);

        assert!(
            result,
            "zero start should succeed (treated as 0-based start)"
        );
        assert!(view.is_valid);
        assert_eq!(view.dims.as_slice(), &[3]); // 3 elements
        assert_eq!(view.offset, 0);
    }

    #[test]
    fn test_runtime_view_apply_range_checked_invalid_dim() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Invalid dimension index
        let result = view.apply_range_checked(5, 1, 3);

        assert!(!result, "invalid dim_idx should return false");
        assert!(!view.is_valid, "view should be marked invalid");
    }

    #[test]
    fn test_runtime_view_apply_range_checked_empty_after_clamp() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Range entirely beyond bounds [10:15] should be empty
        // start_0based = 9, end_0based = min(15, 5) = 5
        // 9 >= 5, so empty
        let result = view.apply_range_checked(0, 10, 15);

        assert!(!result, "range beyond bounds should return false");
        assert!(!view.is_valid, "view should be marked invalid");
        assert_eq!(view.dims.as_slice(), &[0]); // Empty dimension
    }

    #[test]
    fn test_runtime_view_apply_sparse() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Apply sparse subscript: elements at indices 0, 2, 4
        let offsets: SmallVec<[u16; 16]> = smallvec::smallvec![0, 2, 4];
        view.apply_sparse(0, offsets);

        assert_eq!(view.dims.as_slice(), &[3]); // 3 sparse elements
        assert_eq!(view.sparse.len(), 1);
        assert_eq!(view.sparse[0].dim_index, 0);
        assert_eq!(view.sparse[0].parent_offsets.as_slice(), &[0, 2, 4]);
        assert!(!view.is_contiguous());
    }

    #[test]
    fn test_runtime_view_flat_offset_sparse() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Apply sparse subscript: elements at indices 0, 2, 4
        let offsets: SmallVec<[u16; 16]> = smallvec::smallvec![0, 2, 4];
        view.apply_sparse(0, offsets);

        // Now indices 0, 1, 2 in the sparse view map to 0, 2, 4 in parent
        assert_eq!(view.flat_offset(&[0]), 0);
        assert_eq!(view.flat_offset(&[1]), 2);
        assert_eq!(view.flat_offset(&[2]), 4);
    }

    #[test]
    fn test_runtime_view_transpose() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        view.transpose();

        assert_eq!(view.dims.as_slice(), &[4, 3]);
        assert_eq!(view.strides.as_slice(), &[1, 4]); // Reversed
        assert_eq!(view.dim_ids.as_slice(), &[1, 0]); // Reversed
    }

    #[test]
    fn test_runtime_view_transpose_flat_offset() {
        // Original 2x3 matrix:
        // [0, 1, 2]
        // [3, 4, 5]
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![2, 3];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Before transpose: [1, 2] -> 1*3 + 2 = 5
        assert_eq!(view.flat_offset(&[1, 2]), 5);

        view.transpose();

        // After transpose (now 3x2):
        // Transposed matrix is:
        // [0, 3]
        // [1, 4]
        // [2, 5]
        // Position [2, 1] should give us 5 (bottom right)
        // With reversed strides [1, 3]: 2*1 + 1*3 = 5
        assert_eq!(view.flat_offset(&[2, 1]), 5);
    }

    // =========================================================================
    // ByteCodeContext Tests
    // =========================================================================

    #[test]
    fn test_context_intern_name() {
        let mut ctx = ByteCodeContext::default();

        let id1 = ctx.intern_name("DimA");
        let id2 = ctx.intern_name("DimB");
        let id3 = ctx.intern_name("DimA"); // Should return same ID

        assert_eq!(id1, 0);
        assert_eq!(id2, 1);
        assert_eq!(id3, 0); // Same as id1

        assert_eq!(ctx.get_name(id1), Some("DimA"));
        assert_eq!(ctx.get_name(id2), Some("DimB"));
    }

    #[test]
    fn test_context_add_dimension() {
        let mut ctx = ByteCodeContext::default();

        let name_id = ctx.intern_name("DimA");
        let dim = DimensionInfo::indexed(name_id, 5);
        let dim_id = ctx.add_dimension(dim);

        assert_eq!(dim_id, 0);
        let retrieved = ctx.get_dimension(dim_id).unwrap();
        assert_eq!(retrieved.size, 5);
    }

    #[test]
    fn test_context_find_dimension_by_name() {
        let mut ctx = ByteCodeContext::default();

        let name_a = ctx.intern_name("DimA");
        let name_b = ctx.intern_name("DimB");

        ctx.add_dimension(DimensionInfo::indexed(name_a, 3));
        ctx.add_dimension(DimensionInfo::indexed(name_b, 5));

        assert_eq!(ctx.find_dimension_by_name("DimA"), Some(0));
        assert_eq!(ctx.find_dimension_by_name("DimB"), Some(1));
        assert_eq!(ctx.find_dimension_by_name("DimC"), None);
    }

    #[test]
    fn test_context_add_static_view() {
        let mut ctx = ByteCodeContext::default();

        let view = StaticArrayView {
            base_off: 100,
            is_temp: false,
            dims: smallvec::smallvec![3, 4],
            strides: smallvec::smallvec![4, 1],
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: smallvec::smallvec![0, 1],
        };

        let view_id = ctx.add_static_view(view);
        assert_eq!(view_id, 0);

        let retrieved = ctx.get_static_view(view_id).unwrap();
        assert_eq!(retrieved.base_off, 100);
        assert_eq!(retrieved.dims.as_slice(), &[3, 4]);
    }

    #[test]
    fn test_static_view_to_runtime() {
        let static_view = StaticArrayView {
            base_off: 100,
            is_temp: false,
            dims: smallvec::smallvec![3, 4],
            strides: smallvec::smallvec![4, 1],
            offset: 8,
            sparse: SmallVec::new(),
            dim_ids: smallvec::smallvec![0, 1],
        };

        let runtime = static_view.to_runtime_view();

        assert_eq!(runtime.base_off, 100);
        assert!(!runtime.is_temp);
        assert_eq!(runtime.dims.as_slice(), &[3, 4]);
        assert_eq!(runtime.strides.as_slice(), &[4, 1]);
        assert_eq!(runtime.offset, 8);
    }

    #[test]
    fn test_context_set_temp_info() {
        let mut ctx = ByteCodeContext::default();

        ctx.set_temp_info(vec![0, 10, 25], 50);

        assert_eq!(ctx.temp_offsets, vec![0, 10, 25]);
        assert_eq!(ctx.temp_total_size, 50);
    }

    #[test]
    fn test_context_subdim_relations() {
        let mut ctx = ByteCodeContext::default();

        let rel = SubdimensionRelation::contiguous(0, 1, 2, 3);
        let rel_id = ctx.add_subdim_relation(rel);

        assert_eq!(rel_id, 0);
        assert_eq!(
            ctx.subdim_relations[0].parent_offsets.as_slice(),
            &[2, 3, 4]
        );
    }

    #[test]
    fn test_runtime_view_is_valid() {
        // Views created with constructors should be valid
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let view = RuntimeView::for_var(0, dims, dim_ids);
        assert!(view.is_valid);

        // Invalid view should have is_valid = false
        let invalid_view = RuntimeView::invalid();
        assert!(!invalid_view.is_valid);
    }

    #[test]
    fn test_runtime_view_subscript_checked_valid() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Valid 1-based index (1..=5 is valid range)
        let result = view.apply_single_subscript_checked(0, 3);
        assert!(result);
        assert!(view.is_valid);
        // After subscript, view becomes scalar
        assert!(view.dims.is_empty());
    }

    #[test]
    fn test_runtime_view_subscript_checked_out_of_bounds_zero() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Index 0 is out of bounds (1-based indexing)
        let result = view.apply_single_subscript_checked(0, 0);
        assert!(!result);
        assert!(!view.is_valid);
    }

    #[test]
    fn test_runtime_view_subscript_checked_out_of_bounds_high() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Index 6 is out of bounds (max is 5)
        let result = view.apply_single_subscript_checked(0, 6);
        assert!(!result);
        assert!(!view.is_valid);
    }

    #[test]
    fn test_runtime_view_apply_sparse_with_dim_id() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0]; // Parent dim id = 0
        let mut view = RuntimeView::for_var(0, dims, dim_ids);

        // Apply sparse with child dim id = 1
        let parent_offsets: SmallVec<[u16; 16]> = smallvec::smallvec![1, 3, 4];
        view.apply_sparse_with_dim_id(0, parent_offsets, 1);

        // Dimension ID should be updated to child dim id
        assert_eq!(view.dim_ids[0], 1);
        // Size should be reduced to sparse count
        assert_eq!(view.dims[0], 3);
        // Should have sparse mapping
        assert_eq!(view.sparse.len(), 1);
        assert_eq!(view.sparse[0].parent_offsets.as_slice(), &[1, 3, 4]);
    }

    // =========================================================================
    // offset_for_iter_index Tests
    // =========================================================================

    #[test]
    fn test_offset_for_iter_index_contiguous_1d() {
        // Simple 1D contiguous array [5 elements]
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let view = RuntimeView::for_var(10, dims, dim_ids);

        // For contiguous array, offset_for_iter_index should return offset + index
        assert_eq!(view.offset_for_iter_index(0), 0);
        assert_eq!(view.offset_for_iter_index(1), 1);
        assert_eq!(view.offset_for_iter_index(4), 4);
    }

    #[test]
    fn test_offset_for_iter_index_contiguous_2d() {
        // 2D contiguous array [3][4] = 12 elements
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let view = RuntimeView::for_var(0, dims, dim_ids);

        // Row-major: flat index = row * 4 + col
        // Index 0 -> [0,0] -> 0
        // Index 1 -> [0,1] -> 1
        // Index 4 -> [1,0] -> 4
        // Index 5 -> [1,1] -> 5
        // Index 11 -> [2,3] -> 11
        assert_eq!(view.offset_for_iter_index(0), 0);
        assert_eq!(view.offset_for_iter_index(1), 1);
        assert_eq!(view.offset_for_iter_index(4), 4);
        assert_eq!(view.offset_for_iter_index(5), 5);
        assert_eq!(view.offset_for_iter_index(11), 11);
    }

    #[test]
    fn test_offset_for_iter_index_with_offset() {
        // 1D array with non-zero offset (sliced view)
        let view = RuntimeView {
            base_off: 0,
            is_temp: false,
            dims: smallvec::smallvec![3],
            strides: smallvec::smallvec![1],
            offset: 5, // Start at element 5
            sparse: SmallVec::new(),
            dim_ids: smallvec::smallvec![0],
            is_valid: true,
        };

        // Not contiguous due to offset != 0, so we compute via multi-dim indices
        // Index 0 -> element at offset 5
        // Index 1 -> element at offset 6
        // Index 2 -> element at offset 7
        assert_eq!(view.offset_for_iter_index(0), 5);
        assert_eq!(view.offset_for_iter_index(1), 6);
        assert_eq!(view.offset_for_iter_index(2), 7);
    }

    #[test]
    fn test_offset_for_iter_index_non_standard_strides() {
        // 2D array with column-major strides (not contiguous)
        let view = RuntimeView {
            base_off: 0,
            is_temp: false,
            dims: smallvec::smallvec![3, 4],    // 3 rows, 4 cols
            strides: smallvec::smallvec![1, 3], // Column-major: stride[0]=1, stride[1]=3
            offset: 0,
            sparse: SmallVec::new(),
            dim_ids: smallvec::smallvec![0, 1],
            is_valid: true,
        };

        // Not contiguous (strides are column-major, not row-major)
        // Flat index 0 -> [0,0] -> 0*1 + 0*3 = 0
        // Flat index 1 -> [0,1] -> 0*1 + 1*3 = 3
        // Flat index 4 -> [1,0] -> 1*1 + 0*3 = 1
        // Flat index 5 -> [1,1] -> 1*1 + 1*3 = 4
        assert_eq!(view.offset_for_iter_index(0), 0);
        assert_eq!(view.offset_for_iter_index(1), 3);
        assert_eq!(view.offset_for_iter_index(4), 1);
        assert_eq!(view.offset_for_iter_index(5), 4);
    }

    #[test]
    fn test_offset_for_iter_index_sparse() {
        // 1D sparse array: elements at indices [1, 3, 7] of parent
        let view = RuntimeView {
            base_off: 0,
            is_temp: false,
            dims: smallvec::smallvec![3], // 3 sparse elements
            strides: smallvec::smallvec![1],
            offset: 0,
            sparse: smallvec::smallvec![RuntimeSparseMapping {
                dim_index: 0,
                parent_offsets: smallvec::smallvec![1, 3, 7],
            }],
            dim_ids: smallvec::smallvec![0],
            is_valid: true,
        };

        // Index 0 -> parent offset 1
        // Index 1 -> parent offset 3
        // Index 2 -> parent offset 7
        assert_eq!(view.offset_for_iter_index(0), 1);
        assert_eq!(view.offset_for_iter_index(1), 3);
        assert_eq!(view.offset_for_iter_index(2), 7);
    }

    #[test]
    fn test_offset_for_iter_index_scalar() {
        // Scalar view (0 dimensions)
        let view = RuntimeView {
            base_off: 10,
            is_temp: false,
            dims: SmallVec::new(),
            strides: SmallVec::new(),
            offset: 5,
            sparse: SmallVec::new(),
            dim_ids: SmallVec::new(),
            is_valid: true,
        };

        // For scalar, always return offset
        assert_eq!(view.offset_for_iter_index(0), 5);
    }

    // =========================================================================
    // Peephole Optimizer Tests
    // =========================================================================

    #[test]
    fn test_peephole_empty_bytecode() {
        let mut bc = ByteCode {
            code: vec![],
            literals: vec![],
        };
        bc.peephole_optimize();
        assert!(bc.code.is_empty());
    }

    #[test]
    fn test_peephole_single_instruction() {
        let mut bc = ByteCode {
            code: vec![Opcode::Ret],
            literals: vec![],
        };
        bc.peephole_optimize();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(bc.code[0], Opcode::Ret));
    }

    #[test]
    fn test_peephole_no_fusible_patterns() {
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Not {},
                Opcode::Ret,
            ],
            literals: vec![],
        };
        bc.peephole_optimize();
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[0], Opcode::LoadVar { off: 0 }));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 1 }));
        assert!(matches!(bc.code[2], Opcode::Not {}));
        assert!(matches!(bc.code[3], Opcode::Ret));
    }

    #[test]
    fn test_peephole_load_constant_assign_curr_fusion() {
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 5 },
            ],
            literals: vec![42.0],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 1);
        match &bc.code[0] {
            Opcode::AssignConstCurr { off, literal_id } => {
                assert_eq!(*off, 5);
                assert_eq!(*literal_id, 0);
            }
            _ => panic!("expected AssignConstCurr"),
        }
    }

    #[test]
    fn test_peephole_op2_assign_curr_fusion() {
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 2 },
            ],
            literals: vec![],
        };
        bc.peephole_optimize();

        // LoadVar, LoadVar stay; Op2+AssignCurr fuse into BinOpAssignCurr
        assert_eq!(bc.code.len(), 3);
        assert!(matches!(bc.code[0], Opcode::LoadVar { off: 0 }));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 1 }));
        match &bc.code[2] {
            Opcode::BinOpAssignCurr { op, off } => {
                assert!(matches!(op, Op2::Add));
                assert_eq!(*off, 2);
            }
            _ => panic!("expected BinOpAssignCurr"),
        }
    }

    #[test]
    fn test_peephole_op2_assign_next_fusion() {
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Mul },
                Opcode::AssignNext { off: 3 },
            ],
            literals: vec![],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 3);
        match &bc.code[2] {
            Opcode::BinOpAssignNext { op, off } => {
                assert!(matches!(op, Op2::Mul));
                assert_eq!(*off, 3);
            }
            _ => panic!("expected BinOpAssignNext"),
        }
    }

    #[test]
    fn test_peephole_all_op2_variants_fuse() {
        // Verify every Op2 variant can be fused with AssignCurr
        let ops = [
            Op2::Add,
            Op2::Sub,
            Op2::Mul,
            Op2::Div,
            Op2::Exp,
            Op2::Mod,
            Op2::Gt,
            Op2::Gte,
            Op2::Lt,
            Op2::Lte,
            Op2::Eq,
            Op2::And,
            Op2::Or,
        ];
        for op in ops {
            let mut bc = ByteCode {
                code: vec![Opcode::Op2 { op }, Opcode::AssignCurr { off: 10 }],
                literals: vec![],
            };
            bc.peephole_optimize();
            assert_eq!(bc.code.len(), 1, "failed for op variant");
            assert!(matches!(bc.code[0], Opcode::BinOpAssignCurr { .. }));
        }
    }

    #[test]
    fn test_peephole_multiple_fusions() {
        // Two independent fusion opportunities in sequence
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::LoadVar { off: 2 },
                Opcode::Op2 { op: Op2::Sub },
                Opcode::AssignCurr { off: 3 },
            ],
            literals: vec![1.0],
        };
        bc.peephole_optimize();

        // LoadConstant+AssignCurr -> AssignConstCurr
        // LoadVar, LoadVar stay
        // Op2+AssignCurr -> BinOpAssignCurr
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[0], Opcode::AssignConstCurr { .. }));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 1 }));
        assert!(matches!(bc.code[2], Opcode::LoadVar { off: 2 }));
        assert!(matches!(bc.code[3], Opcode::BinOpAssignCurr { .. }));
    }

    #[test]
    fn test_peephole_mixed_fusible_and_nonfusible() {
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::Not {},
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 1 },
                Opcode::LoadVar { off: 2 },
                Opcode::Ret,
            ],
            literals: vec![0.0],
        };
        bc.peephole_optimize();

        // LoadVar, Not stay; LoadConstant+AssignCurr fuse; LoadVar, Ret stay
        assert_eq!(bc.code.len(), 5);
        assert!(matches!(bc.code[0], Opcode::LoadVar { off: 0 }));
        assert!(matches!(bc.code[1], Opcode::Not {}));
        assert!(matches!(bc.code[2], Opcode::AssignConstCurr { .. }));
        assert!(matches!(bc.code[3], Opcode::LoadVar { off: 2 }));
        assert!(matches!(bc.code[4], Opcode::Ret));
    }

    #[test]
    fn test_peephole_jump_target_prevents_fusion() {
        // If instruction i+1 is a jump target, don't fuse i with i+1.
        // Layout (before optimization):
        //   0: LoadConstant { id: 0 }       <- loop body start (jump target)
        //   1: AssignCurr { off: 0 }
        //   2: NextIterOrJump { jump_back: -2 }  (target = 2 + (-2) = 0)
        //   3: Ret
        //
        // Instruction 0 is a jump target, so even though 0 is LoadConstant
        // and 1 is AssignCurr, we should NOT fuse them because instruction 0
        // is a jump target. Wait -- actually the check is whether i+1 is a
        // jump target. Here instruction 0 IS a jump target. The optimizer checks
        // `!jump_targets[i + 1]` to decide whether to fuse i with i+1.
        //
        // For i=0: jump_targets[1] is false, so fusion IS allowed.
        // The jump target protection matters when the SECOND instruction of a
        // potential pair is a jump target. Let's build that scenario:
        //
        //   0: Ret                            <- something before the loop
        //   1: LoadVar { off: 5 }             <- jump target (loop body start)
        //   2: NextIterOrJump { jump_back: -1 }  (target = 2 + (-1) = 1)
        //   3: Ret
        //
        // For i=0 (Ret): can_fuse checks jump_targets[1] = true -> no fusion.
        // This prevents fusing Ret with LoadVar, which is correct.
        //
        // A more realistic scenario: Op2 followed by AssignCurr where the
        // AssignCurr is a jump target.
        let mut bc = ByteCode {
            code: vec![
                Opcode::Op2 { op: Op2::Add },             // 0
                Opcode::AssignCurr { off: 0 },            // 1 -- jump target
                Opcode::NextIterOrJump { jump_back: -1 }, // 2 -> target = 2-1 = 1
                Opcode::Ret,                              // 3
            ],
            literals: vec![],
        };
        bc.peephole_optimize();

        // Fusion of 0+1 should be prevented because instruction 1 is a jump target
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[0], Opcode::Op2 { op: Op2::Add }));
        assert!(matches!(bc.code[1], Opcode::AssignCurr { off: 0 }));
        assert!(matches!(bc.code[2], Opcode::NextIterOrJump { .. }));
        assert!(matches!(bc.code[3], Opcode::Ret));
    }

    #[test]
    fn test_peephole_jump_target_only_blocks_specific_pair() {
        // Verify that a jump target only blocks fusion of the pair where
        // the second instruction is the target, not other pairs.
        //
        //   0: LoadConstant { id: 0 }
        //   1: AssignCurr { off: 0 }         <- NOT a jump target, so 0+1 CAN fuse
        //   2: LoadVar { off: 5 }            <- jump target
        //   3: NextIterOrJump { jump_back: -1 }  (target = 3-1 = 2)
        //   4: Ret
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 0 },
                Opcode::LoadVar { off: 5 },
                Opcode::NextIterOrJump { jump_back: -1 },
                Opcode::Ret,
            ],
            literals: vec![1.0],
        };
        bc.peephole_optimize();

        // 0+1 should fuse (neither target), 2 stays (it's a jump target, but
        // the previous instruction was AssignCurr which doesn't match any pattern
        // anyway), 3 stays, 4 stays
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(
            bc.code[0],
            Opcode::AssignConstCurr {
                off: 0,
                literal_id: 0
            }
        ));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 5 }));
        assert!(matches!(bc.code[2], Opcode::NextIterOrJump { .. }));
        assert!(matches!(bc.code[3], Opcode::Ret));
    }

    #[test]
    fn test_peephole_jump_offset_recalculation_next_iter() {
        // When fusion shrinks the code, jump offsets must be recalculated.
        // This test places a fusion BEFORE the loop (outside the jump target
        // to jump instruction range) so the fixup works correctly.
        //
        // Before optimization:
        //   0: LoadConstant { id: 0 }    \
        //   1: AssignCurr { off: 0 }     / -> fuse
        //   2: LoadVar { off: 1 }        <- jump target
        //   3: AssignCurr { off: 2 }
        //   4: NextIterOrJump { jump_back: -2 }  target = 4+(-2) = 2
        //   5: Ret
        //
        // After optimization:
        //   0: AssignConstCurr            (fused 0+1)
        //   1: LoadVar { off: 1 }         (jump target)
        //   2: AssignCurr { off: 2 }
        //   3: NextIterOrJump { jump_back: -2 }  (loop body unchanged)
        //   4: Ret
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadConstant { id: 0 },           // 0
                Opcode::AssignCurr { off: 0 },            // 1
                Opcode::LoadVar { off: 1 },               // 2 (jump target)
                Opcode::AssignCurr { off: 2 },            // 3
                Opcode::NextIterOrJump { jump_back: -2 }, // 4, target=2
                Opcode::Ret,                              // 5
            ],
            literals: vec![1.0],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 5);
        assert!(matches!(bc.code[0], Opcode::AssignConstCurr { .. }));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 1 }));
        assert!(matches!(bc.code[2], Opcode::AssignCurr { off: 2 }));
        match &bc.code[3] {
            Opcode::NextIterOrJump { jump_back } => {
                assert_eq!(*jump_back, -2, "jump_back should remain -2");
            }
            _ => panic!("expected NextIterOrJump"),
        }
        assert!(matches!(bc.code[4], Opcode::Ret));
    }

    #[test]
    fn test_peephole_fusion_inside_loop_body() {
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadVar { off: 0 },               // 0 (jump target)
                Opcode::Op2 { op: Op2::Add },             // 1 \
                Opcode::AssignCurr { off: 1 },            // 2 / fuse
                Opcode::NextIterOrJump { jump_back: -3 }, // 3, target=0
                Opcode::Ret,                              // 4
            ],
            literals: vec![],
        };
        bc.peephole_optimize();

        // 1+2 fuse -> BinOpAssignCurr
        // Result: [LoadVar, BinOpAssignCurr, NextIterOrJump, Ret]
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[0], Opcode::LoadVar { off: 0 }));
        assert!(matches!(
            bc.code[1],
            Opcode::BinOpAssignCurr {
                op: Op2::Add,
                off: 1
            }
        ));
        match &bc.code[2] {
            Opcode::NextIterOrJump { jump_back } => {
                // new PC 2, target should be new PC 0 -> jump_back = -2
                assert_eq!(*jump_back, -2);
            }
            other => panic!(
                "expected NextIterOrJump, got {:?}",
                std::mem::discriminant(other)
            ),
        }
        assert!(matches!(bc.code[3], Opcode::Ret));
    }

    #[test]
    fn test_peephole_jump_offset_recalculation_next_broadcast() {
        // Same as above but with NextBroadcastOrJump
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadConstant { id: 0 },                // 0
                Opcode::AssignCurr { off: 0 },                 // 1
                Opcode::LoadVar { off: 1 },                    // 2 (jump target)
                Opcode::NextBroadcastOrJump { jump_back: -1 }, // 3, target=2
                Opcode::Ret,                                   // 4
            ],
            literals: vec![1.0],
        };
        bc.peephole_optimize();

        // 0+1 fuse -> AssignConstCurr at new PC 0
        // 2 -> new PC 1 (jump target)
        // 3 -> new PC 2
        // 4 -> new PC 3
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[0], Opcode::AssignConstCurr { .. }));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 1 }));
        match &bc.code[2] {
            Opcode::NextBroadcastOrJump { jump_back } => {
                // new PC 2, target should be new PC 1
                assert_eq!(*jump_back, -1, "jump_back should be -1");
            }
            _ => panic!("expected NextBroadcastOrJump"),
        }
        assert!(matches!(bc.code[3], Opcode::Ret));
    }

    #[test]
    fn test_peephole_no_fusion_when_patterns_dont_match() {
        // Op2 followed by something other than AssignCurr/AssignNext
        let mut bc = ByteCode {
            code: vec![Opcode::Op2 { op: Op2::Add }, Opcode::Not {}, Opcode::Ret],
            literals: vec![],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 3);
        assert!(matches!(bc.code[0], Opcode::Op2 { op: Op2::Add }));
        assert!(matches!(bc.code[1], Opcode::Not {}));
    }

    #[test]
    fn test_peephole_load_constant_not_followed_by_assign_curr() {
        // LoadConstant not followed by AssignCurr should not fuse
        let mut bc = ByteCode {
            code: vec![Opcode::LoadConstant { id: 0 }, Opcode::Not {}, Opcode::Ret],
            literals: vec![1.0],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 3);
        assert!(matches!(bc.code[0], Opcode::LoadConstant { id: 0 }));
    }

    #[test]
    fn test_peephole_via_builder() {
        // Verify that ByteCodeBuilder::finish() runs peephole_optimize
        let mut builder = ByteCodeBuilder::default();
        let lit_id = builder.intern_literal(3.14);
        builder.push_opcode(Opcode::LoadConstant { id: lit_id });
        builder.push_opcode(Opcode::AssignCurr { off: 7 });
        builder.push_opcode(Opcode::Ret);

        let bc = builder.finish();
        assert_eq!(bc.code.len(), 2);
        match &bc.code[0] {
            Opcode::AssignConstCurr { off, literal_id } => {
                assert_eq!(*off, 7);
                assert_eq!(*literal_id, lit_id);
            }
            _ => panic!("expected AssignConstCurr after builder finish"),
        }
        assert!(matches!(bc.code[1], Opcode::Ret));
    }

    #[test]
    fn test_peephole_consecutive_fusions_chain() {
        // Three consecutive fusible pairs
        let mut bc = ByteCode {
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 0 },
                Opcode::LoadConstant { id: 1 },
                Opcode::AssignCurr { off: 1 },
                Opcode::Op2 { op: Op2::Div },
                Opcode::AssignNext { off: 2 },
            ],
            literals: vec![1.0, 2.0],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 3);
        assert!(matches!(
            bc.code[0],
            Opcode::AssignConstCurr {
                off: 0,
                literal_id: 0
            }
        ));
        assert!(matches!(
            bc.code[1],
            Opcode::AssignConstCurr {
                off: 1,
                literal_id: 1
            }
        ));
        match &bc.code[2] {
            Opcode::BinOpAssignNext { op, off } => {
                assert!(matches!(op, Op2::Div));
                assert_eq!(*off, 2);
            }
            _ => panic!("expected BinOpAssignNext"),
        }
    }

    #[test]
    fn test_peephole_last_instruction_not_fused_alone() {
        // If the fusible first instruction is the very last one, no fusion happens
        let mut bc = ByteCode {
            code: vec![Opcode::Ret, Opcode::LoadConstant { id: 0 }],
            literals: vec![1.0],
        };
        bc.peephole_optimize();

        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::Ret));
        assert!(matches!(bc.code[1], Opcode::LoadConstant { id: 0 }));
    }

    // =========================================================================
    // DimList Side Table Tests
    // =========================================================================

    #[test]
    fn test_dim_list_add_and_get() {
        let mut ctx = ByteCodeContext::default();

        let id = ctx.add_dim_list(2, [10, 20, 0, 0]);
        assert_eq!(id, 0);

        let (n_dims, ids) = ctx.get_dim_list(id);
        assert_eq!(n_dims, 2);
        assert_eq!(ids[0], 10);
        assert_eq!(ids[1], 20);
    }

    #[test]
    fn test_dim_list_multiple_entries() {
        let mut ctx = ByteCodeContext::default();

        let id0 = ctx.add_dim_list(1, [5, 0, 0, 0]);
        let id1 = ctx.add_dim_list(3, [1, 2, 3, 0]);
        let id2 = ctx.add_dim_list(4, [10, 20, 30, 40]);

        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        let (n, ids) = ctx.get_dim_list(id0);
        assert_eq!(n, 1);
        assert_eq!(ids[0], 5);

        let (n, ids) = ctx.get_dim_list(id1);
        assert_eq!(n, 3);
        assert_eq!(&ids[..3], &[1, 2, 3]);

        let (n, ids) = ctx.get_dim_list(id2);
        assert_eq!(n, 4);
        assert_eq!(ids, &[10, 20, 30, 40]);
    }

    #[test]
    fn test_dim_list_zero_dims() {
        let mut ctx = ByteCodeContext::default();

        let id = ctx.add_dim_list(0, [0, 0, 0, 0]);
        let (n_dims, _ids) = ctx.get_dim_list(id);
        assert_eq!(n_dims, 0);
    }

    #[test]
    fn test_dim_list_incremental_ids() {
        let mut ctx = ByteCodeContext::default();

        // Add several entries and verify IDs are sequential
        for i in 0..10u16 {
            let id = ctx.add_dim_list(1, [i, 0, 0, 0]);
            assert_eq!(id, i, "dim list IDs should be assigned sequentially");
        }

        // Verify all entries are still retrievable
        for i in 0..10u16 {
            let (n, ids) = ctx.get_dim_list(i);
            assert_eq!(n, 1);
            assert_eq!(ids[0], i);
        }
    }
}

/// A single variable's compiled initial-value bytecode, along with the
/// data-buffer offsets it writes to (from AssignCurr nodes).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct CompiledInitial {
    // Used for diagnostics in debug_print_bytecode and set_override error messages
    #[allow(dead_code)]
    pub(crate) ident: Ident<Canonical>,
    /// Sorted, deduplicated offsets of all AssignCurr targets in this variable's
    /// initials bytecode.
    pub(crate) offsets: Vec<usize>,
    pub(crate) bytecode: ByteCode,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct CompiledModule {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) n_slots: usize,
    pub(crate) context: Arc<ByteCodeContext>,
    pub(crate) compiled_initials: Arc<Vec<CompiledInitial>>,
    pub(crate) compiled_flows: Arc<ByteCode>,
    pub(crate) compiled_stocks: Arc<ByteCode>,
}
