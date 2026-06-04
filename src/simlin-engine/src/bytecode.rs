// Copyright 2026 The Simlin Authors. All rights reserved.
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
/// Module-relative result-slot offset baked into opcodes.
///
/// u16 keeps opcode payloads compact (the VM dispatch loop is sensitive to
/// opcode size), which caps a single module's layout at 65,536 slots. Layouts
/// beyond that -- in practice only very large LTM-instrumented models, e.g.
/// C-LEARN in discovery mode at ~171k slots -- are rejected with a clear
/// error by `compiler::symbolic::check_layout_addressable` /
/// `resolve_var_ref`; a silent overflow here would wrap writes into the
/// implicit-global slots (time/dt) and corrupt every result.
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
        self.offset == 0 && self.dense_linear_start().is_some()
    }

    /// If this view's elements occupy a single dense row-major run -- i.e.
    /// the k-th element in iteration order lives at flat offset `start + k`
    /// -- return that starting offset.
    ///
    /// This is `is_contiguous` minus the `offset == 0` requirement: an
    /// offset slice like `arr[2, *]` (or a leading-dimension range
    /// `arr[1:3, *]`) is one dense run beginning mid-array. The predicate is
    /// "no sparse mappings, and strides are exactly the row-major strides of
    /// the *current* dims": any subscript that breaks the run (an inner-dim
    /// range, a transpose) leaves a stride that no longer matches.
    ///
    /// Hot-path significance: the per-element `flat_offset` /
    /// `offset_for_iter_index` arithmetic (index decompose + stride dot
    /// product) collapses to `start + k` for these views, so iteration and
    /// reduction loops over them can be plain linear scans (GH #603).
    pub fn dense_linear_start(&self) -> Option<usize> {
        if !self.sparse.is_empty() {
            return None;
        }
        let mut expected_stride = 1i32;
        for i in (0..self.dims.len()).rev() {
            if self.strides[i] != expected_stride {
                return None;
            }
            expected_stride *= self.dims[i] as i32;
        }
        Some(self.offset as usize)
    }

    /// Shape equality (dims + dim_ids) for the per-element iteration fast
    /// path in `LoadIterViewTop` / `LoadIterViewAt`.
    ///
    /// Semantically identical to `self.dims == other.dims && self.dim_ids ==
    /// other.dim_ids`, but hand-rolled: SmallVec's slice PartialEq lowers to
    /// an out-of-line memcmp call, which measured ~2% of a C-LEARN run when
    /// executed once per element per load site. Views carry at most 4 dims,
    /// so a branchless accumulate compiles to a few inline compares (the
    /// early-exit-free loop shape also keeps LLVM's loop-idiom recognition
    /// from converting it back into a bcmp call).
    #[inline]
    pub fn same_shape(&self, other: &RuntimeView) -> bool {
        let n = self.dims.len();
        if n != other.dims.len() || self.dim_ids.len() != other.dim_ids.len() {
            return false;
        }
        let mut eq = true;
        for i in 0..n {
            eq &= self.dims[i] == other.dims[i];
        }
        for i in 0..self.dim_ids.len() {
            eq &= self.dim_ids[i] == other.dim_ids[i];
        }
        eq
    }

    /// Compute the flat offset for a given multi-dimensional index.
    /// Takes into account strides, offset, and sparse mappings.
    ///
    /// `#[inline]`: called per element from the vector-op / iteration loops
    /// (`vector_elm_map`, `vector_sort_order`, the broadcast path); as an
    /// out-of-line call the loop pays call overhead plus un-hoisted
    /// `SmallVec` spill checks on every element, which showed up as several
    /// percent of a C-LEARN run.
    #[inline]
    pub fn flat_offset(&self, indices: &[u16]) -> usize {
        debug_assert_eq!(indices.len(), self.dims.len());

        let mut flat = self.offset as usize;

        // Dense fast-path: the overwhelmingly common case is no sparse mappings.
        // This function is a per-element hot spot (called from the vector-op /
        // reducer dispatch sites). When `sparse` is empty the sparse_lookup
        // build below is pure overhead -- every entry would be `None`, so each
        // `actual_idx` equals `idx` -- making this branch numerically identical
        // to the general path while skipping the SmallVec allocation/scan and
        // the per-index Option check.
        if self.sparse.is_empty() {
            for (i, &idx) in indices.iter().enumerate() {
                flat += idx as usize * self.strides[i] as usize;
            }
            return flat;
        }

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
    /// - If range is empty or reversed (start >= end after clamping),
    ///   sets dimension size to 0 but keeps the view valid. This produces
    ///   a zero-element view so reduction operations return their identity
    ///   (0 for SUM, NaN for MIN/MAX).
    ///
    /// Returns true if a non-empty range was applied, false if the range
    /// was empty (but the view remains valid).
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

        // Empty range (reversed bounds or start past end after clamping).
        // The view stays valid with zero elements rather than being marked
        // invalid, so reduction operations return their identity values.
        if start_0based >= end_0based {
            self.dims[dim_idx] = 0;
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
    /// `#[inline]` for the same per-element-call reason as [`Self::flat_offset`].
    #[inline]
    pub fn offset_for_iter_index(&self, iter_idx: usize) -> usize {
        if self.dims.is_empty() {
            // Scalar view
            return self.offset as usize;
        }

        // A single dense row-major run (contiguous, or an offset slice like
        // `arr[2, *]`) resolves directly: element k lives at start + k.
        if let Some(start) = self.dense_linear_start() {
            return start + iter_idx;
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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
    Quantum,
    Ramp,
    SafeDiv,
    Sign,
    Sin,
    Sshape,
    Sqrt,
    Step,
    Tan,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
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
#[derive(Clone, Copy, PartialEq)]
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

    /// Resolve PREVIOUS(var, fallback) for a direct scalar variable.
    /// Pops the already-evaluated fallback from the stack, then pushes either
    /// `prev_values[module_off + off]` or that fallback when `TIME ==
    /// INITIAL_TIME`.
    LoadPrev {
        off: VariableOffset,
    },
    /// Load the initial (t=0) value of a variable from the initial-value buffer.
    /// Pushes `initial_values[module_off + off]` onto the stack.
    LoadInitial {
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

    // === 3-ADDRESS BINARY OPS (R2) ===
    // Fold the leaf operand load(s) of a binary op into the op itself, so a
    // subexpression `a op b` dispatches once instead of 3 (two loads + Op2) or
    // twice instead of 2 (one load + Op2). Each pushes its result. `curr[]` is
    // effectively the register file: these read operands straight from it (or
    // from `literals`) with no intervening stack push/pop. Created only by the
    // late `fuse_three_address` pass on final concrete bytecode -- they never
    // enter the symbolic/incremental layer. A 3-operand `dst = a op b` would
    // exceed the 8-byte Opcode budget, so the assign stays a separate op.
    /// Push `curr[module_off + l] op curr[module_off + r]`.
    BinVarVar {
        l: VariableOffset,
        r: VariableOffset,
        op: Op2,
    },
    /// Push `curr[module_off + l] op literals[r]`.
    BinVarConst {
        l: VariableOffset,
        r: LiteralId,
        op: Op2,
    },
    /// Push `literals[l] op curr[module_off + r]`.
    BinConstVar {
        l: LiteralId,
        r: VariableOffset,
        op: Op2,
    },
    /// Pop `lhs`; push `lhs op curr[module_off + r]`.
    BinStackVar {
        r: VariableOffset,
        op: Op2,
    },
    /// Pop `lhs`; push `lhs op literals[r]`.
    BinStackConst {
        r: LiteralId,
        op: Op2,
    },

    // === 3-ADDRESS BINARY OPS WITH GLOBAL OPERANDS (R2 extension) ===
    // Mirror the `Bin*` pushing forms above, but for the leaf operands that are
    // GLOBALS (TIME / DT / INITIAL_TIME / FINAL_TIME, loaded by `LoadGlobalVar`).
    // The original `Bin*` forms missed these because no binop fused a global
    // operand, so a `LoadGlobalVar` operand stayed an unfused load.
    //
    // CRITICAL: a `_global` field indexes `curr[g]` directly (an absolute global
    // slot, NO `module_off` -- exactly like `LoadGlobalVar`), while a plain
    // var field indexes `curr[module_off + v]` (module-relative, like `LoadVar`).
    // Inside a submodule (`module_off > 0`) those are different slots; conflating
    // them is a silent miscompile. The operand order is `l op r` matching the
    // original load order, load-bearing for the non-commutative Sub/Div.
    /// Push `curr[l_global] op curr[module_off + r]`.
    BinGlobalVar {
        l_global: VariableOffset,
        r: VariableOffset,
        op: Op2,
    },
    /// Push `curr[module_off + l] op curr[r_global]`.
    BinVarGlobal {
        l: VariableOffset,
        r_global: VariableOffset,
        op: Op2,
    },
    /// Push `curr[l_global] op literals[r]`.
    BinGlobalConst {
        l_global: VariableOffset,
        r: LiteralId,
        op: Op2,
    },
    /// Push `literals[l] op curr[r_global]`.
    BinConstGlobal {
        l: LiteralId,
        r_global: VariableOffset,
        op: Op2,
    },
    /// Push `curr[l_global] op curr[r_global]`.
    BinGlobalGlobal {
        l_global: VariableOffset,
        r_global: VariableOffset,
        op: Op2,
    },
    /// Pop `lhs`; push `lhs op curr[r_global]`.
    BinStackGlobal {
        r_global: VariableOffset,
        op: Op2,
    },

    // === 3-ADDRESS BINARY OP WITH TWO CONSTANT OPERANDS (R2 extension) ===
    // The greedy 3-window missed `LoadConstant; LoadConstant; Op2` (both operands
    // are separate literals) because no `Bin*` form took two constant leaves.
    // This is NOT compile-time constant folding of the result: the two operands
    // are distinct interned literals, so we fuse the two loads + the op into one
    // dispatch that still computes `literals[l] op literals[r]` at run time.
    /// Push `literals[l] op literals[r]`.
    BinConstConst {
        l: LiteralId,
        r: LiteralId,
        op: Op2,
    },

    // === 3-ADDRESS FUSED LEAF ASSIGNMENTS (R2 extension) ===
    // A leaf assignment `dst = a op b` is, post-`peephole_optimize`,
    // `LoadX a; LoadX b; BinOpAssign{Curr|Next}(op, dst)` (3 dispatches). These
    // fold all three into one register-style op that reads its operands straight
    // from `curr[]`/`literals` and writes the result straight to `curr[]`/`next[]`
    // -- no stack push or pop at all. Like the 2-operand pushing forms above they
    // are created only by the late `fuse_three_address` pass on final concrete
    // bytecode and never enter the symbolic/incremental layer.
    //
    // The operator is encoded in the variant tag (one variant per {Add,Sub,Mul,
    // Div}) rather than an `Op2` payload field. That keeps the payload at 3xu16 =
    // 6 bytes so `size_of::<Opcode>()` stays at 8; a shared `Op2` field would make
    // the payload 7 bytes and blow the budget. The four operators cover ~100% of
    // measured leaf-assign candidates; any other operator is left in its existing
    // `BinOpAssign{Curr|Next}` form. Encoding the operator in the tag also removes
    // the per-dispatch `eval_op2` operator branch: each arm is straight-line f64
    // arithmetic. The operand order is `l op r` matching the original load order,
    // which is load-bearing for the non-commutative Sub and Div.

    // -- VarVar: `c[dst] = c[l] OP c[r]` (Curr) / `n[dst] = c[l] OP c[r]` (Next) --
    AssignAddVarVarCurr {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignSubVarVarCurr {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignMulVarVarCurr {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignDivVarVarCurr {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignAddVarVarNext {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignSubVarVarNext {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignMulVarVarNext {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignDivVarVarNext {
        l: VariableOffset,
        r: VariableOffset,
        dst: VariableOffset,
    },

    // -- VarConst: `c[dst] = c[l] OP literals[r]` (l is a var, r is a literal) --
    AssignAddVarConstCurr {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignSubVarConstCurr {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignMulVarConstCurr {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignDivVarConstCurr {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignAddVarConstNext {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignSubVarConstNext {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignMulVarConstNext {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },
    AssignDivVarConstNext {
        l: VariableOffset,
        r: LiteralId,
        dst: VariableOffset,
    },

    // -- ConstVar: `c[dst] = literals[l] OP c[r]` (l is a literal, r is a var) --
    AssignAddConstVarCurr {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignSubConstVarCurr {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignMulConstVarCurr {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignDivConstVarCurr {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignAddConstVarNext {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignSubConstVarNext {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignMulConstVarNext {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },
    AssignDivConstVarNext {
        l: LiteralId,
        r: VariableOffset,
        dst: VariableOffset,
    },

    // === 2-ADDRESS STACK-LEAF FUSED ASSIGNMENTS (R2 extension) ===
    // `lhs` is already on the arithmetic stack (a nested subexpression result);
    // the rhs is a leaf load. Post-peephole this is `LoadX b; BinOpAssign(op, dst)`
    // (2 dispatches): pop the lhs, combine with the leaf rhs, store. Folds 2->1.
    // Here the operator stays in the payload (the `{dst, b}` + `op` form is 5
    // bytes, still within budget) so all operators -- not just {Add,Sub,Mul,Div}
    // -- are handled by one variant per (Var/Const, Curr/Next) combo.
    /// Pop `lhs`; `curr[module_off + dst] = lhs op curr[module_off + b]`.
    AssignStackVarCurr {
        dst: VariableOffset,
        b: VariableOffset,
        op: Op2,
    },
    /// Pop `lhs`; `next[module_off + dst] = lhs op curr[module_off + b]`.
    AssignStackVarNext {
        dst: VariableOffset,
        b: VariableOffset,
        op: Op2,
    },
    /// Pop `lhs`; `curr[module_off + dst] = lhs op literals[b]`.
    AssignStackConstCurr {
        dst: VariableOffset,
        b: LiteralId,
        op: Op2,
    },
    /// Pop `lhs`; `next[module_off + dst] = lhs op literals[b]`.
    AssignStackConstNext {
        dst: VariableOffset,
        b: LiteralId,
        op: Op2,
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

    // === VECTOR OPERATIONS ===
    // These implement Vensim's array-producing builtins that cannot be
    // expressed as simple element-wise iteration or scalar reduction.
    /// Reduces selected array elements to one scalar.
    /// Reads 2 views (selection mask, expression values) from the view stack
    /// and 2 scalars (max_value, action) from the arithmetic stack.
    /// The result is a single scalar pushed onto the arithmetic stack.
    VectorSelect {},

    /// Genuine-Vensim VECTOR ELM MAP; writes the full result array to
    /// temp_storage. `full_source_len` is the source *variable's* total
    /// element count (product of its full declared dimensions) -- the
    /// out-of-range bound for the genuine `:NA:` rule. The source variable's
    /// storage in `curr[]` is contiguous row-major, so the source view's
    /// `base_off` plus a directly-computed flat index addresses it, and the
    /// offset steps the innermost (last declared) dimension whose contiguous
    /// stride is 1.
    VectorElmMap {
        write_temp_id: TempId,
        full_source_len: u32,
    },

    /// Produces an array of sort-order indices; writes to temp_storage.
    VectorSortOrder {
        write_temp_id: TempId,
    },

    /// Produces an array of ranks (ordinal positions in sorted order); writes to temp_storage.
    /// Pops 1 scalar (direction: 1=ascending, 0=descending) from the arithmetic stack.
    /// Reads 1 view from the view stack.
    Rank {
        write_temp_id: TempId,
    },

    /// Per-element graphical-function lookup over an arrayed GF (GH #580 Bug B):
    /// `g[D!](index)` where each element of `g` carries its own lookup table.
    /// Pops 1 scalar (the shared lookup `index`) from the arithmetic stack and
    /// reads 1 view (the arrayed GF's full storage) from the view stack; for
    /// each of the view's `size()` elements `i` it evaluates the table
    /// `graphical_functions[base_gf + i]` at `index` (the per-element-table
    /// layout `Compiler::table_base_ids` records -- one table per element, in
    /// declared order) and writes the result to `temp_storage[write_temp + i]`.
    /// `table_count` bounds `base_gf + i` (an out-of-range element yields NaN,
    /// matching the scalar `Lookup` opcode). The result temp is then consumed
    /// as an array view by the reducer / vector op that wrapped the apply.
    LookupArray {
        base_gf: GraphicalFunctionId,
        table_count: u16,
        mode: LookupMode,
        write_temp_id: TempId,
    },

    /// Priority-based allocation; writes result array to temp_storage.
    AllocateAvailable {
        write_temp_id: TempId,
    },

    /// ALLOCATE BY PRIORITY desugaring: constructs rectangular priority
    /// profiles from (request, priority, width, supply) and delegates to
    /// allocate_available. Pops 2 scalars (width, supply) from the stack,
    /// reads request and priority from the top two views.
    AllocateByPriority {
        write_temp_id: TempId,
    },

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
            | Opcode::LoadInitial { .. }
            | Opcode::LoadGlobalVar { .. }
            | Opcode::LoadModuleInput { .. } => (0, 1),

            // LoadPrev pops the caller-provided fallback, then pushes
            // either the fallback (at t=INITIAL_TIME) or prev_values[off].
            Opcode::LoadPrev { .. } => (1, 1),

            // Legacy subscript: PushSubscriptIndex pops an index from the
            // arithmetic stack and appends it to a separate subscript_index
            // SmallVec (not the arithmetic stack). Multiple PushSubscriptIndex
            // ops may precede a single LoadSubscript for multi-dimensional
            // access, but each only pops 1 from the arithmetic stack.
            Opcode::PushSubscriptIndex { .. } => (1, 0),
            // LoadSubscript consumes the accumulated subscript_index entries
            // and pushes the looked-up value onto the arithmetic stack.
            Opcode::LoadSubscript { .. } => (0, 1),

            // Control flow
            Opcode::SetCond {} => (1, 0), // pops condition
            Opcode::If {} => (2, 1),      // pops true+false branches, pushes result
            Opcode::Ret => (0, 0),

            // Module eval: pops n_inputs from the caller's arithmetic stack.
            // The child module executes with its own stack context (via EvalState)
            // and writes results directly to curr/next, not back to the caller's
            // arithmetic stack, so pushes = 0 from the caller's perspective.
            Opcode::EvalModule { n_inputs, .. } => (*n_inputs, 0),

            // Assignment: pops 1 (the value to assign)
            Opcode::AssignCurr { .. } | Opcode::AssignNext { .. } => (1, 0),

            // Builtins always take 3 args (actual + padding), push 1 result
            Opcode::Apply { .. } => (3, 1),
            // Lookup pops element_offset and lookup_index, pushes result
            Opcode::Lookup { .. } => (2, 1),

            // Superinstructions
            Opcode::AssignConstCurr { .. } => (0, 0), // reads literal directly
            Opcode::BinOpAssignCurr { .. } => (2, 0), // pops 2, assigns directly
            Opcode::BinOpAssignNext { .. } => (2, 0), // pops 2, assigns directly

            // 3-address binops: the *Var/*Const forms read both operands from
            // curr/literals and push (0 pops, 1 push); the Stack* forms pop the
            // lhs and push the result (1 pop, 1 push).
            Opcode::BinVarVar { .. } | Opcode::BinVarConst { .. } | Opcode::BinConstVar { .. } => {
                (0, 1)
            }
            Opcode::BinStackVar { .. } | Opcode::BinStackConst { .. } => (1, 1),

            // Global-operand pushing forms mirror the var/const forms above: the
            // two-leaf forms read both operands from curr/literals and push
            // (0 pops, 1 push); BinStackGlobal pops the lhs and pushes (1, 1).
            // BinConstConst reads both literals and pushes (0, 1).
            Opcode::BinGlobalVar { .. }
            | Opcode::BinVarGlobal { .. }
            | Opcode::BinGlobalConst { .. }
            | Opcode::BinConstGlobal { .. }
            | Opcode::BinGlobalGlobal { .. }
            | Opcode::BinConstConst { .. } => (0, 1),
            Opcode::BinStackGlobal { .. } => (1, 1),

            // 3-address fused leaf assignments read operands from curr/literals
            // and write straight to curr/next: no arithmetic-stack traffic.
            Opcode::AssignAddVarVarCurr { .. }
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
            | Opcode::AssignDivConstVarNext { .. } => (0, 0),

            // Stack-leaf fused assignments pop the pre-existing lhs (1 pop) and
            // write the combined result to curr/next (no push).
            Opcode::AssignStackVarCurr { .. }
            | Opcode::AssignStackVarNext { .. }
            | Opcode::AssignStackConstCurr { .. }
            | Opcode::AssignStackConstNext { .. } => (1, 0),

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
            Opcode::LoadTempDynamic { .. } => (1, 1), // pops index, pushes value

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

            // VectorSelect pops 2 scalars (max_value, action), pushes 1 result
            Opcode::VectorSelect {} => (2, 1),
            // VectorElmMap writes to temp_storage without touching the arithmetic stack.
            // VectorSortOrder/Rank/AllocateAvailable pop 1 scalar each (direction/avail)
            // and write their result arrays to temp_storage.
            Opcode::VectorElmMap { .. } => (0, 0),
            Opcode::VectorSortOrder { .. } => (1, 0),
            Opcode::Rank { .. } => (1, 0),
            // LookupArray pops the scalar lookup index, reads the GF view, and
            // writes the per-element-lookup array to temp_storage.
            Opcode::LookupArray { .. } => (1, 0),
            Opcode::AllocateAvailable { .. } => (1, 0),
            // AllocateByPriority pops 2 scalars (width, supply) from the stack
            Opcode::AllocateByPriority { .. } => (2, 0),

            // Broadcasting
            Opcode::BeginBroadcastIter { .. } | Opcode::EndBroadcastIter {} => (0, 0),
            Opcode::LoadBroadcastElement { .. } => (0, 1),
            Opcode::StoreBroadcastElement {} => (1, 0),
            Opcode::NextBroadcastOrJump { .. } => (0, 0),
        }
    }

    /// Static variant name, independent of payload. Used for bytecode-composition
    /// profiling (opcode histograms) and human-readable diagnostics without
    /// depending on the optional `debug-derive` Debug impl.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Opcode::Op2 { .. } => "Op2",
            Opcode::Not {} => "Not",
            Opcode::LoadConstant { .. } => "LoadConstant",
            Opcode::LoadVar { .. } => "LoadVar",
            Opcode::LoadGlobalVar { .. } => "LoadGlobalVar",
            Opcode::LoadPrev { .. } => "LoadPrev",
            Opcode::LoadInitial { .. } => "LoadInitial",
            Opcode::PushSubscriptIndex { .. } => "PushSubscriptIndex",
            Opcode::LoadSubscript { .. } => "LoadSubscript",
            Opcode::SetCond {} => "SetCond",
            Opcode::If {} => "If",
            Opcode::Ret => "Ret",
            Opcode::LoadModuleInput { .. } => "LoadModuleInput",
            Opcode::EvalModule { .. } => "EvalModule",
            Opcode::AssignCurr { .. } => "AssignCurr",
            Opcode::AssignNext { .. } => "AssignNext",
            Opcode::Apply { .. } => "Apply",
            Opcode::Lookup { .. } => "Lookup",
            Opcode::AssignConstCurr { .. } => "AssignConstCurr",
            Opcode::BinVarVar { .. } => "BinVarVar",
            Opcode::BinVarConst { .. } => "BinVarConst",
            Opcode::BinConstVar { .. } => "BinConstVar",
            Opcode::BinStackVar { .. } => "BinStackVar",
            Opcode::BinStackConst { .. } => "BinStackConst",
            Opcode::BinGlobalVar { .. } => "BinGlobalVar",
            Opcode::BinVarGlobal { .. } => "BinVarGlobal",
            Opcode::BinGlobalConst { .. } => "BinGlobalConst",
            Opcode::BinConstGlobal { .. } => "BinConstGlobal",
            Opcode::BinGlobalGlobal { .. } => "BinGlobalGlobal",
            Opcode::BinStackGlobal { .. } => "BinStackGlobal",
            Opcode::BinConstConst { .. } => "BinConstConst",
            Opcode::BinOpAssignCurr { .. } => "BinOpAssignCurr",
            Opcode::BinOpAssignNext { .. } => "BinOpAssignNext",
            Opcode::AssignAddVarVarCurr { .. } => "AssignAddVarVarCurr",
            Opcode::AssignSubVarVarCurr { .. } => "AssignSubVarVarCurr",
            Opcode::AssignMulVarVarCurr { .. } => "AssignMulVarVarCurr",
            Opcode::AssignDivVarVarCurr { .. } => "AssignDivVarVarCurr",
            Opcode::AssignAddVarVarNext { .. } => "AssignAddVarVarNext",
            Opcode::AssignSubVarVarNext { .. } => "AssignSubVarVarNext",
            Opcode::AssignMulVarVarNext { .. } => "AssignMulVarVarNext",
            Opcode::AssignDivVarVarNext { .. } => "AssignDivVarVarNext",
            Opcode::AssignAddVarConstCurr { .. } => "AssignAddVarConstCurr",
            Opcode::AssignSubVarConstCurr { .. } => "AssignSubVarConstCurr",
            Opcode::AssignMulVarConstCurr { .. } => "AssignMulVarConstCurr",
            Opcode::AssignDivVarConstCurr { .. } => "AssignDivVarConstCurr",
            Opcode::AssignAddVarConstNext { .. } => "AssignAddVarConstNext",
            Opcode::AssignSubVarConstNext { .. } => "AssignSubVarConstNext",
            Opcode::AssignMulVarConstNext { .. } => "AssignMulVarConstNext",
            Opcode::AssignDivVarConstNext { .. } => "AssignDivVarConstNext",
            Opcode::AssignAddConstVarCurr { .. } => "AssignAddConstVarCurr",
            Opcode::AssignSubConstVarCurr { .. } => "AssignSubConstVarCurr",
            Opcode::AssignMulConstVarCurr { .. } => "AssignMulConstVarCurr",
            Opcode::AssignDivConstVarCurr { .. } => "AssignDivConstVarCurr",
            Opcode::AssignAddConstVarNext { .. } => "AssignAddConstVarNext",
            Opcode::AssignSubConstVarNext { .. } => "AssignSubConstVarNext",
            Opcode::AssignMulConstVarNext { .. } => "AssignMulConstVarNext",
            Opcode::AssignDivConstVarNext { .. } => "AssignDivConstVarNext",
            Opcode::AssignStackVarCurr { .. } => "AssignStackVarCurr",
            Opcode::AssignStackVarNext { .. } => "AssignStackVarNext",
            Opcode::AssignStackConstCurr { .. } => "AssignStackConstCurr",
            Opcode::AssignStackConstNext { .. } => "AssignStackConstNext",
            Opcode::PushVarView { .. } => "PushVarView",
            Opcode::PushTempView { .. } => "PushTempView",
            Opcode::PushStaticView { .. } => "PushStaticView",
            Opcode::PushVarViewDirect { .. } => "PushVarViewDirect",
            Opcode::ViewSubscriptConst { .. } => "ViewSubscriptConst",
            Opcode::ViewSubscriptDynamic { .. } => "ViewSubscriptDynamic",
            Opcode::ViewRange { .. } => "ViewRange",
            Opcode::ViewRangeDynamic { .. } => "ViewRangeDynamic",
            Opcode::ViewStarRange { .. } => "ViewStarRange",
            Opcode::ViewWildcard { .. } => "ViewWildcard",
            Opcode::ViewTranspose {} => "ViewTranspose",
            Opcode::PopView {} => "PopView",
            Opcode::DupView {} => "DupView",
            Opcode::LoadTempConst { .. } => "LoadTempConst",
            Opcode::LoadTempDynamic { .. } => "LoadTempDynamic",
            Opcode::BeginIter { .. } => "BeginIter",
            Opcode::LoadIterElement {} => "LoadIterElement",
            Opcode::LoadIterTempElement { .. } => "LoadIterTempElement",
            Opcode::LoadIterViewTop {} => "LoadIterViewTop",
            Opcode::LoadIterViewAt { .. } => "LoadIterViewAt",
            Opcode::StoreIterElement {} => "StoreIterElement",
            Opcode::NextIterOrJump { .. } => "NextIterOrJump",
            Opcode::EndIter {} => "EndIter",
            Opcode::ArraySum {} => "ArraySum",
            Opcode::ArrayMax {} => "ArrayMax",
            Opcode::ArrayMin {} => "ArrayMin",
            Opcode::ArrayMean {} => "ArrayMean",
            Opcode::ArrayStddev {} => "ArrayStddev",
            Opcode::ArraySize {} => "ArraySize",
            Opcode::VectorSelect {} => "VectorSelect",
            Opcode::VectorElmMap { .. } => "VectorElmMap",
            Opcode::VectorSortOrder { .. } => "VectorSortOrder",
            Opcode::Rank { .. } => "Rank",
            Opcode::LookupArray { .. } => "LookupArray",
            Opcode::AllocateAvailable { .. } => "AllocateAvailable",
            Opcode::AllocateByPriority { .. } => "AllocateByPriority",
            Opcode::BeginBroadcastIter { .. } => "BeginBroadcastIter",
            Opcode::LoadBroadcastElement { .. } => "LoadBroadcastElement",
            Opcode::StoreBroadcastElement {} => "StoreBroadcastElement",
            Opcode::NextBroadcastOrJump { .. } => "NextBroadcastOrJump",
            Opcode::EndBroadcastIter {} => "EndBroadcastIter",
        }
    }
}

// ============================================================================
// Module and Array Declarations
// ============================================================================

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct ModuleDeclaration {
    pub(crate) model_name: Ident<Canonical>,
    /// The set of input names for this module instantiation.
    /// Different instantiations of the same model with different input sets
    /// need separate compiled modules (the ModuleInput offsets differ).
    pub(crate) input_set: BTreeSet<Ident<Canonical>>,
    pub(crate) off: usize, // offset within the parent module
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    /// Convert to a RuntimeView for use on the view stack.
    ///
    /// This runs on every `PushStaticView` (~1M times on a C-LEARN run), so the
    /// per-field copies are deliberately the cheapest correct construction.
    ///
    /// `SmallVec`'s `Clone` (with the `specialization` feature off, as here)
    /// lowers to `self.as_slice().iter().cloned().collect()` -- an element-wise
    /// `Extend`, NOT a memcpy -- even for `Copy` elements. For the three
    /// `Copy`-element vectors we instead call `from_slice`, which copies the
    /// inline buffer in one `ptr::copy_nonoverlapping`. `sparse`'s element type
    /// (`RuntimeSparseMapping`) is not `Copy`, but it is empty for every dense
    /// (non-star-range) view -- the overwhelmingly common case -- so we take a
    /// free fresh empty `SmallVec` then and only fall back to a real clone for a
    /// genuinely sparse view.
    pub fn to_runtime_view(&self) -> RuntimeView {
        let sparse = if self.sparse.is_empty() {
            SmallVec::new()
        } else {
            self.sparse.clone()
        };
        RuntimeView {
            base_off: self.base_off,
            is_temp: self.is_temp,
            dims: SmallVec::from_slice(&self.dims),
            strides: SmallVec::from_slice(&self.strides),
            offset: self.offset,
            sparse,
            dim_ids: SmallVec::from_slice(&self.dim_ids),
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
#[derive(Clone, Default, PartialEq)]
pub struct ByteCodeContext {
    // === Existing fields ===
    /// Graphical function lookup tables
    pub(crate) graphical_functions: Vec<Vec<(f64, f64)>>,
    /// Module declarations for nested modules
    pub(crate) modules: Vec<ModuleDeclaration>,
    /// Legacy array definitions (deprecated, use dimensions instead)
    pub(crate) arrays: Vec<ArrayDefinition>,

    // === New array support fields ===
    /// Dimension information table (indexed by DimId)
    pub(crate) dimensions: Vec<DimensionInfo>,
    /// Subdimension relationships for star ranges
    pub(crate) subdim_relations: Vec<SubdimensionRelation>,
    /// Interned names table (dimension names, element names)
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

// ByteCodeCompiler (codegen.rs) builds these tables directly and transfers
// them into ByteCodeContext, so these convenience methods are unused in
// production but useful for testing.
#[allow(dead_code)]
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
#[derive(Clone, Default, PartialEq)]
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
                panic!("stack_effect underflow at pc {pc}: {pops} pops but depth is {depth}")
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
        let key = OrderedFloat(lit);
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
        let mut optimized: Vec<Opcode> = Vec::with_capacity(self.code.len());
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

    /// Late 3-address fusion pass (R2): fold the leaf operand load(s) of a
    /// binary op into the op itself.
    ///
    /// Two families of pattern, both longest-match-first within their window:
    /// - Pushing subexpressions: `LoadX; LoadY; Op2` -> one `Bin*` (3->1) and
    ///   `LoadX; Op2` (lhs already on the stack) -> one `BinStack*` (2->1).
    /// - Leaf assignments: post-`peephole_optimize` a leaf assign is `LoadX;
    ///   LoadY; BinOpAssign{Curr|Next}` (the peephole already folded the trailing
    ///   `Op2; AssignCurr` into `BinOpAssignCurr`). This pass folds the whole
    ///   thing into one register-style `Assign{Add|Sub|Mul|Div}{combo}{phase}`
    ///   (3->1) that reads operands from curr/literals and writes straight to
    ///   curr/next with no stack traffic. The 2-window analogue `LoadX;
    ///   BinOpAssign` (lhs on the stack) folds into `AssignStack{Var|Const}{phase}`
    ///   (2->1). The 3-operand forms exist only for {Add,Sub,Mul,Div} (the
    ///   operator is in the variant tag to keep the opcode within 8 bytes);
    ///   other operators keep their `BinOpAssign` form. The 2-operand stack-leaf
    ///   forms keep the operator in the payload so they cover every operator.
    ///
    /// MUST run only on FINAL concrete bytecode -- after `peephole_optimize`
    /// and, for the incremental path, after `resolve` -- because the fused
    /// opcodes deliberately do not exist in the symbolic/incremental layer.
    /// Greedy, longest-match-first. Reuses the same jump-target guard and
    /// old->new PC remap as `peephole_optimize`: a run is only fused when the
    /// instructions it *absorbs* (the second, and for a triple the third) are
    /// not jump targets, so no jump can land mid-fusion; a jump to the first
    /// instruction still lands on the fused opcode at the same new PC.
    /// Stack-effect-preserving (the fused ops carry the net effect of the
    /// sequence they replace), so the `max_stack_depth` safety proof is
    /// unchanged.
    pub(crate) fn fuse_three_address(&mut self) {
        if self.code.is_empty() {
            return;
        }

        // The four operators that have dedicated leaf-assign opcodes. Any other
        // operator leaves the leaf assign in its `BinOpAssign{Curr|Next}` form.
        // `is_phase_next` selects the Curr vs Next family. `l`/`r` keep the
        // original load order (load-bearing for the non-commutative Sub/Div).
        fn fused_leaf_var_var(op: Op2, is_next: bool, l: u16, r: u16, dst: u16) -> Option<Opcode> {
            Some(match (op, is_next) {
                (Op2::Add, false) => Opcode::AssignAddVarVarCurr { l, r, dst },
                (Op2::Sub, false) => Opcode::AssignSubVarVarCurr { l, r, dst },
                (Op2::Mul, false) => Opcode::AssignMulVarVarCurr { l, r, dst },
                (Op2::Div, false) => Opcode::AssignDivVarVarCurr { l, r, dst },
                (Op2::Add, true) => Opcode::AssignAddVarVarNext { l, r, dst },
                (Op2::Sub, true) => Opcode::AssignSubVarVarNext { l, r, dst },
                (Op2::Mul, true) => Opcode::AssignMulVarVarNext { l, r, dst },
                (Op2::Div, true) => Opcode::AssignDivVarVarNext { l, r, dst },
                _ => return None,
            })
        }
        fn fused_leaf_var_const(
            op: Op2,
            is_next: bool,
            l: u16,
            r: u16,
            dst: u16,
        ) -> Option<Opcode> {
            Some(match (op, is_next) {
                (Op2::Add, false) => Opcode::AssignAddVarConstCurr { l, r, dst },
                (Op2::Sub, false) => Opcode::AssignSubVarConstCurr { l, r, dst },
                (Op2::Mul, false) => Opcode::AssignMulVarConstCurr { l, r, dst },
                (Op2::Div, false) => Opcode::AssignDivVarConstCurr { l, r, dst },
                (Op2::Add, true) => Opcode::AssignAddVarConstNext { l, r, dst },
                (Op2::Sub, true) => Opcode::AssignSubVarConstNext { l, r, dst },
                (Op2::Mul, true) => Opcode::AssignMulVarConstNext { l, r, dst },
                (Op2::Div, true) => Opcode::AssignDivVarConstNext { l, r, dst },
                _ => return None,
            })
        }
        fn fused_leaf_const_var(
            op: Op2,
            is_next: bool,
            l: u16,
            r: u16,
            dst: u16,
        ) -> Option<Opcode> {
            Some(match (op, is_next) {
                (Op2::Add, false) => Opcode::AssignAddConstVarCurr { l, r, dst },
                (Op2::Sub, false) => Opcode::AssignSubConstVarCurr { l, r, dst },
                (Op2::Mul, false) => Opcode::AssignMulConstVarCurr { l, r, dst },
                (Op2::Div, false) => Opcode::AssignDivConstVarCurr { l, r, dst },
                (Op2::Add, true) => Opcode::AssignAddConstVarNext { l, r, dst },
                (Op2::Sub, true) => Opcode::AssignSubConstVarNext { l, r, dst },
                (Op2::Mul, true) => Opcode::AssignMulConstVarNext { l, r, dst },
                (Op2::Div, true) => Opcode::AssignDivConstVarNext { l, r, dst },
                _ => return None,
            })
        }

        // 1. Build set of PCs that are jump targets.
        let mut jump_targets = vec![false; self.code.len()];
        for (pc, op) in self.code.iter().enumerate() {
            if let Some(offset) = op.jump_offset() {
                let target = (pc as isize + offset as isize) as usize;
                assert!(
                    target < jump_targets.len(),
                    "jump at pc {pc} targets {target}, out of bounds (len {})",
                    self.code.len()
                );
                jump_targets[target] = true;
            }
        }

        // 2. Greedy fuse, building an old_pc -> new_pc map (one entry per
        //    original instruction) for jump fixup.
        let mut optimized: Vec<Opcode> = Vec::with_capacity(self.code.len());
        let mut pc_map: Vec<usize> = Vec::with_capacity(self.code.len() + 1);
        let mut i = 0;
        while i < self.code.len() {
            let new_pc = optimized.len();

            // 3-window: [leaf load, leaf load, <combiner>] where the combiner is
            // either an `Op2` (a pushing subexpression) or a `BinOpAssign{Curr|
            // Next}` (a leaf assignment, post-peephole). Both absorbed
            // instructions (i+1, i+2) must not be jump targets.
            //
            // The leaf-assign forms are tried first: a `BinOpAssign` third op
            // means the whole `dst = a op b` collapses into one register-style
            // op (3->1) rather than `Bin*` (3->1 pushing) + a separate store.
            // Only {Add,Sub,Mul,Div} have dedicated leaf-assign opcodes; any
            // other operator falls through and keeps the existing form.
            let three = i + 2 < self.code.len() && !jump_targets[i + 1] && !jump_targets[i + 2];
            let fused3 = if three {
                // Decode the combiner once into two mutually-exclusive options:
                // `assign3 = (op, dst, is_next)` for a leaf-assign, or
                // `push3 = op` for a pushing Op2.
                let (assign3, push3) = match &self.code[i + 2] {
                    Opcode::BinOpAssignCurr { op, off } => (Some((*op, *off, false)), None),
                    Opcode::BinOpAssignNext { op, off } => (Some((*op, *off, true)), None),
                    Opcode::Op2 { op } => (None, Some(*op)),
                    _ => (None, None),
                };
                match (&self.code[i], &self.code[i + 1]) {
                    // Leaf assignment `dst = a op b` -> one fused op (3->1).
                    (Opcode::LoadVar { off: l }, Opcode::LoadVar { off: r }) => assign3
                        .and_then(|(op, dst, n)| fused_leaf_var_var(op, n, *l, *r, dst))
                        .or_else(|| push3.map(|op| Opcode::BinVarVar { l: *l, r: *r, op })),
                    (Opcode::LoadVar { off: l }, Opcode::LoadConstant { id: r }) => assign3
                        .and_then(|(op, dst, n)| fused_leaf_var_const(op, n, *l, *r, dst))
                        .or_else(|| push3.map(|op| Opcode::BinVarConst { l: *l, r: *r, op })),
                    (Opcode::LoadConstant { id: l }, Opcode::LoadVar { off: r }) => assign3
                        .and_then(|(op, dst, n)| fused_leaf_const_var(op, n, *l, *r, dst))
                        .or_else(|| push3.map(|op| Opcode::BinConstVar { l: *l, r: *r, op })),
                    // Two constant leaves: no leaf-assign form, so this only fuses
                    // a pushing `Op2`. Computes `literals[l] op literals[r]` at run
                    // time (NOT compile-time folding -- the operands are two
                    // distinct interned literals).
                    (Opcode::LoadConstant { id: l }, Opcode::LoadConstant { id: r }) => {
                        push3.map(|op| Opcode::BinConstConst { l: *l, r: *r, op })
                    }
                    // Global-operand leaf pairs. A global has no dedicated
                    // leaf-assign opcode, so these fuse only a pushing `Op2`; a
                    // `BinOpAssign` combiner (push3 == None) falls through to the
                    // 2-window, which folds the rhs+store and leaves the global
                    // load as a standalone push. `l_global`/`r_global` index
                    // `curr[g]` (absolute), the var operand `curr[module_off + v]`.
                    (Opcode::LoadGlobalVar { off: l }, Opcode::LoadVar { off: r }) => {
                        push3.map(|op| Opcode::BinGlobalVar {
                            l_global: *l,
                            r: *r,
                            op,
                        })
                    }
                    (Opcode::LoadVar { off: l }, Opcode::LoadGlobalVar { off: r }) => {
                        push3.map(|op| Opcode::BinVarGlobal {
                            l: *l,
                            r_global: *r,
                            op,
                        })
                    }
                    (Opcode::LoadGlobalVar { off: l }, Opcode::LoadConstant { id: r }) => push3
                        .map(|op| Opcode::BinGlobalConst {
                            l_global: *l,
                            r: *r,
                            op,
                        }),
                    (Opcode::LoadConstant { id: l }, Opcode::LoadGlobalVar { off: r }) => push3
                        .map(|op| Opcode::BinConstGlobal {
                            l: *l,
                            r_global: *r,
                            op,
                        }),
                    (Opcode::LoadGlobalVar { off: l }, Opcode::LoadGlobalVar { off: r }) => push3
                        .map(|op| Opcode::BinGlobalGlobal {
                            l_global: *l,
                            r_global: *r,
                            op,
                        }),
                    _ => None,
                }
            } else {
                None
            };
            if let Some(op) = fused3 {
                optimized.push(op);
                pc_map.push(new_pc); // old i
                pc_map.push(new_pc); // old i+1
                pc_map.push(new_pc); // old i+2
                i += 3;
                continue;
            }

            // 2-window: [leaf load, <combiner>] with the lhs already on the
            // stack. The combiner is either `Op2` (pushing) or `BinOpAssign`
            // (the stack-leaf assignment `dst = lhs op b`). The stack-leaf form
            // keeps the operator in its payload, so every operator is handled.
            let two = i + 1 < self.code.len() && !jump_targets[i + 1];
            let fused2 = if two {
                // As above: a stack-leaf assign (operator kept in payload) or a
                // pushing Op2. The stack-leaf form handles every operator.
                let (assign2, push2) = match &self.code[i + 1] {
                    Opcode::BinOpAssignCurr { op, off } => (Some((*op, *off, false)), None),
                    Opcode::BinOpAssignNext { op, off } => (Some((*op, *off, true)), None),
                    Opcode::Op2 { op } => (None, Some(*op)),
                    _ => (None, None),
                };
                match &self.code[i] {
                    Opcode::LoadVar { off: b } => assign2
                        .map(|(op, dst, n)| {
                            if n {
                                Opcode::AssignStackVarNext { dst, b: *b, op }
                            } else {
                                Opcode::AssignStackVarCurr { dst, b: *b, op }
                            }
                        })
                        .or_else(|| push2.map(|op| Opcode::BinStackVar { r: *b, op })),
                    Opcode::LoadConstant { id: b } => assign2
                        .map(|(op, dst, n)| {
                            if n {
                                Opcode::AssignStackConstNext { dst, b: *b, op }
                            } else {
                                Opcode::AssignStackConstCurr { dst, b: *b, op }
                            }
                        })
                        .or_else(|| push2.map(|op| Opcode::BinStackConst { r: *b, op })),
                    // `(lhs on stack) op global`. No global stack-leaf-assign
                    // opcode exists (the global ops are pushing-only), so a
                    // `BinOpAssign` combiner (push2 == None) is left unfused: the
                    // standalone `LoadGlobalVar` then the `BinOpAssign` pops both.
                    Opcode::LoadGlobalVar { off: b } => {
                        push2.map(|op| Opcode::BinStackGlobal { r_global: *b, op })
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(op) = fused2 {
                optimized.push(op);
                pc_map.push(new_pc); // old i
                pc_map.push(new_pc); // old i+1
                i += 2;
                continue;
            }

            // No fusion: copy as-is.
            pc_map.push(new_pc);
            optimized.push(self.code[i]);
            i += 1;
        }
        pc_map.push(optimized.len());

        // 3. Fix up jump offsets via the old_pc -> new_pc map.
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

    #[test]
    fn test_push_named_literal_no_dedup() {
        let mut builder = ByteCodeBuilder::default();
        let a = builder.push_named_literal(0.1);
        let b = builder.push_named_literal(0.1);
        let c = builder.push_named_literal(0.1);

        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
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
        assert_eq!(
            (Opcode::LoadModuleInput { input: 0 }).stack_effect(),
            (0, 1)
        );
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
    fn test_stack_effect_load_prev_pops_fallback() {
        // LoadPrev pops the fallback value from the stack, then
        // pushes the result (either the fallback or prev_values[off]).
        assert_eq!((Opcode::LoadPrev { off: 0 }).stack_effect(), (1, 1));
    }

    #[test]
    fn test_stack_effect_builtins() {
        assert_eq!(
            (Opcode::Apply {
                func: BuiltinId::Abs
            })
            .stack_effect(),
            (3, 1)
        );
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
            (Opcode::EvalModule { id: 0, n_inputs: 3 }).stack_effect(),
            (3, 0)
        );
        assert_eq!(
            (Opcode::EvalModule { id: 0, n_inputs: 0 }).stack_effect(),
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

    #[test]
    fn test_stack_effect_vector_ops() {
        // VectorSelect: pops max_value + action scalars, pushes 1 result
        assert_eq!((Opcode::VectorSelect {}).stack_effect(), (2, 1));
        // VectorElmMap: reads views, writes to temp_storage, no arithmetic stack effect
        assert_eq!(
            (Opcode::VectorElmMap {
                write_temp_id: 0,
                full_source_len: 0
            })
            .stack_effect(),
            (0, 0)
        );
        // VectorSortOrder: pops 1 scalar (direction), writes to temp_storage
        assert_eq!(
            (Opcode::VectorSortOrder { write_temp_id: 0 }).stack_effect(),
            (1, 0)
        );
        // Rank: pops 1 scalar (direction), writes to temp_storage
        assert_eq!((Opcode::Rank { write_temp_id: 0 }).stack_effect(), (1, 0));
        // AllocateAvailable: pops 1 scalar (avail), writes to temp_storage
        assert_eq!(
            (Opcode::AllocateAvailable { write_temp_id: 0 }).stack_effect(),
            (1, 0)
        );
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
                Opcode::LoadVar { off: 0 },    // depth: 1
                Opcode::LoadVar { off: 1 },    // depth: 2
                Opcode::Op2 { op: Op2::Add },  // depth: 1
                Opcode::LoadVar { off: 2 },    // depth: 2
                Opcode::LoadVar { off: 3 },    // depth: 3 (peak)
                Opcode::Op2 { op: Op2::Add },  // depth: 2
                Opcode::Op2 { op: Op2::Mul },  // depth: 1
                Opcode::AssignCurr { off: 4 }, // depth: 0
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
                Opcode::Apply {
                    func: BuiltinId::Abs,
                },
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
                Opcode::LoadVar { off: 0 },    // depth: 1
                Opcode::SetCond {},            // depth: 0
                Opcode::LoadVar { off: 1 },    // depth: 1
                Opcode::LoadVar { off: 2 },    // depth: 2
                Opcode::If {},                 // depth: 1
                Opcode::AssignCurr { off: 3 }, // depth: 0
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
    fn test_max_stack_depth_multidimensional_subscript() {
        // a[i, j]: two PushSubscriptIndex (each pops 1 index from the arithmetic
        // stack, writing to a separate subscript_index SmallVec), then LoadSubscript
        // pushes the result. The indices must be loaded before being popped.
        // LoadVar(i), PushSubscriptIndex, LoadVar(j), PushSubscriptIndex, LoadSubscript, Assign
        let bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },               // depth: 1 (load index i)
                Opcode::PushSubscriptIndex { bounds: 3 }, // depth: 0 (pop i)
                Opcode::LoadVar { off: 1 },               // depth: 1 (load index j)
                Opcode::PushSubscriptIndex { bounds: 4 }, // depth: 0 (pop j)
                Opcode::LoadSubscript { off: 10 },        // depth: 1 (push result)
                Opcode::AssignCurr { off: 20 },           // depth: 0
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

    #[test]
    #[should_panic(expected = "jump at pc 0 targets")]
    fn test_peephole_panics_on_out_of_bounds_jump_target() {
        // A jump that targets beyond the code length indicates a compiler bug
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![Opcode::NextIterOrJump { jump_back: 10 }],
        };
        bc.peephole_optimize();
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

        // Reversed range [7:3] produces an empty but valid view so that
        // reduction operations return their identity (0 for SUM).
        let result = view.apply_range_checked(0, 7, 3);

        assert!(!result, "reversed range should return false");
        assert!(
            view.is_valid,
            "view should remain valid (empty, not invalid)"
        );
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
        assert!(
            view.is_valid,
            "view should remain valid (empty, not invalid)"
        );
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

    /// Oracle for the dense-linear-run property: decompose a flat iteration
    /// index into row-major multi-dim indices and route through the fully
    /// general `flat_offset`, exactly like `offset_for_iter_index`'s slow path.
    fn oracle_offset(view: &RuntimeView, iter_idx: usize) -> usize {
        let mut indices: SmallVec<[u16; 4]> = SmallVec::new();
        let mut remaining = iter_idx;
        for &dim in view.dims.iter().rev() {
            indices.push((remaining % dim as usize) as u16);
            remaining /= dim as usize;
        }
        indices.reverse();
        view.flat_offset(&indices)
    }

    /// When `dense_linear_start` claims a view is one dense run, every
    /// iteration index must resolve to `start + k` -- under both the general
    /// `flat_offset` decomposition and `offset_for_iter_index`.
    fn assert_dense_linear(view: &RuntimeView, expected_start: usize) {
        let start = view
            .dense_linear_start()
            .expect("view should be a dense linear run");
        assert_eq!(start, expected_start);
        for k in 0..view.size() {
            assert_eq!(oracle_offset(view, k), start + k, "oracle at k={k}");
            assert_eq!(
                view.offset_for_iter_index(k),
                start + k,
                "offset_for_iter_index at k={k}"
            );
        }
    }

    #[test]
    fn test_same_shape_matches_smallvec_eq() {
        let mk = |dims: &[u16], dim_ids: &[u16]| -> RuntimeView {
            RuntimeView::for_var(0, SmallVec::from_slice(dims), SmallVec::from_slice(dim_ids))
        };
        let cases = [
            (mk(&[3, 4], &[0, 1]), mk(&[3, 4], &[0, 1])),
            (mk(&[3, 4], &[0, 1]), mk(&[3, 4], &[0, 2])),
            (mk(&[3, 4], &[0, 1]), mk(&[4, 3], &[0, 1])),
            (mk(&[3, 4], &[0, 1]), mk(&[3], &[0])),
            (mk(&[], &[]), mk(&[], &[])),
            (mk(&[2], &[5]), mk(&[2], &[5])),
        ];
        for (a, b) in &cases {
            let expected = a.dims == b.dims && a.dim_ids == b.dim_ids;
            assert_eq!(a.same_shape(b), expected, "{:?} vs {:?}", a.dims, b.dims);
            assert_eq!(b.same_shape(a), expected);
        }
    }

    #[test]
    fn test_dense_linear_start_contiguous() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let view = RuntimeView::for_var(0, dims, dim_ids);
        assert_dense_linear(&view, 0);
    }

    #[test]
    fn test_dense_linear_start_scalar_after_collapse() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        view.apply_single_subscript(0, 2);
        view.apply_single_subscript(0, 1);
        // arr[2, 1] -> single element at flat 9; empty dims is a 1-element run.
        assert_eq!(view.dense_linear_start(), Some(9));
        assert_eq!(view.offset_for_iter_index(0), 9);
    }

    #[test]
    fn test_dense_linear_start_row_slice_has_offset() {
        // arr[1, *] of a 3x4: offset 4, one dense run of 4 -- linear even
        // though `is_contiguous()` is false (offset != 0).
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        view.apply_single_subscript(0, 1);
        assert!(!view.is_contiguous());
        assert_dense_linear(&view, 4);
    }

    #[test]
    fn test_dense_linear_start_leading_range_slice() {
        // arr[1:3, *] of a 5x4: rows 1..3 are adjacent, one dense run of 8
        // starting at flat 4.
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        view.apply_range(0, 1, 3);
        assert!(!view.is_contiguous());
        assert_dense_linear(&view, 4);
    }

    #[test]
    fn test_dense_linear_start_inner_range_slice_is_not_linear() {
        // arr[*, 1:3] of a 3x4: each row contributes 2 adjacent elements but
        // the rows are 4 apart -- NOT one dense run.
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        view.apply_range(1, 1, 3);
        assert_eq!(view.dense_linear_start(), None);
    }

    #[test]
    fn test_dense_linear_start_transposed_is_not_linear() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![3, 4];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0, 1];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        view.transpose();
        assert_eq!(view.dense_linear_start(), None);
    }

    #[test]
    fn test_dense_linear_start_sparse_is_not_linear() {
        let dims: SmallVec<[u16; 4]> = smallvec::smallvec![5];
        let dim_ids: SmallVec<[DimId; 4]> = smallvec::smallvec![0];
        let mut view = RuntimeView::for_var(0, dims, dim_ids);
        let offsets: SmallVec<[u16; 16]> = smallvec::smallvec![0, 2, 4];
        view.apply_sparse(0, offsets);
        assert_eq!(view.dense_linear_start(), None);
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
        let lit_id = builder.intern_literal(3.125);
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

    // === 3-address fusion (R2) ===

    #[test]
    fn test_fuse_var_var() {
        // a + b -> BinVarVar; the trailing assign is left to the existing
        // BinOpAssignCurr fusion (a 3-operand op would exceed the 8-byte budget).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 2 },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(
            bc.code[0],
            Opcode::BinVarVar {
                l: 0,
                r: 1,
                op: Op2::Add
            }
        ));
        assert!(matches!(bc.code[1], Opcode::AssignCurr { off: 2 }));
    }

    #[test]
    fn test_fuse_var_const_preserves_operand_order() {
        // `a - 5`: the var is the lhs, the const the rhs. Sub is non-commutative,
        // so a swapped encoding would be a silent miscompile.
        let mut bc = ByteCode {
            literals: vec![5.0],
            code: vec![
                Opcode::LoadVar { off: 7 },
                Opcode::LoadConstant { id: 0 },
                Opcode::Op2 { op: Op2::Sub },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinVarConst {
                l: 7,
                r: 0,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_const_var_preserves_operand_order() {
        // `5 - a`: the const is the lhs, the var the rhs.
        let mut bc = ByteCode {
            literals: vec![5.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadVar { off: 7 },
                Opcode::Op2 { op: Op2::Sub },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinConstVar {
                l: 0,
                r: 7,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_greedy_triple_then_stack_var() {
        // ((a + b) + c): the leaf triple fuses to BinVarVar (greedy prefers the
        // 3-window), then the outer `+ c` -- whose lhs is on the stack -- fuses
        // the load of c into a BinStackVar.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::LoadVar { off: 2 },
                Opcode::Op2 { op: Op2::Add },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(
            bc.code[0],
            Opcode::BinVarVar {
                l: 0,
                r: 1,
                op: Op2::Add
            }
        ));
        assert!(matches!(
            bc.code[1],
            Opcode::BinStackVar { r: 2, op: Op2::Add }
        ));
    }

    #[test]
    fn test_fuse_stack_const() {
        // (a + b) * 2: leaf triple -> BinVarVar; the outer `* 2` (lhs on stack)
        // -> BinStackConst.
        let mut bc = ByteCode {
            literals: vec![2.0],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::LoadConstant { id: 0 },
                Opcode::Op2 { op: Op2::Mul },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::BinVarVar { .. }));
        assert!(matches!(
            bc.code[1],
            Opcode::BinStackConst { r: 0, op: Op2::Mul }
        ));
    }

    // === 3-address fused leaf assignments (R2 extension) ===
    //
    // Input here is *post-peephole*: the trailing `Op2; AssignCurr` has already
    // been folded to `BinOpAssignCurr` (and `...Next`). The leaf-assign fusion
    // collapses the whole `dst = a op b` to one register-style op.

    #[test]
    fn test_fuse_leaf_assign_var_var_curr() {
        // `dst = a - b`: LoadVar; LoadVar; BinOpAssignCurr(Sub) -> one op.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 3 },
                Opcode::LoadVar { off: 4 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Sub,
                    off: 9,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(
            matches!(
                bc.code[0],
                Opcode::AssignSubVarVarCurr { l: 3, r: 4, dst: 9 }
            ),
            "got {:?}",
            bc.code[0].name()
        );
    }

    #[test]
    fn test_fuse_leaf_assign_var_var_next_div_order() {
        // `dst_next = a / b`: division is non-commutative, so a swapped encoding
        // is a loud failure. Verify l=a (numerator), r=b (denominator).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 1 },
                Opcode::LoadVar { off: 2 },
                Opcode::BinOpAssignNext {
                    op: Op2::Div,
                    off: 5,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::AssignDivVarVarNext { l: 1, r: 2, dst: 5 }
        ));
    }

    #[test]
    fn test_fuse_leaf_assign_var_const_order() {
        // `dst = a - 5`: var is lhs, const is rhs.
        let mut bc = ByteCode {
            literals: vec![5.0],
            code: vec![
                Opcode::LoadVar { off: 7 },
                Opcode::LoadConstant { id: 0 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Sub,
                    off: 2,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::AssignSubVarConstCurr { l: 7, r: 0, dst: 2 }
        ));
    }

    #[test]
    fn test_fuse_leaf_assign_const_var_order() {
        // `dst = 10 / a`: const is lhs (numerator literal), var is rhs.
        let mut bc = ByteCode {
            literals: vec![10.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadVar { off: 7 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Div,
                    off: 2,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::AssignDivConstVarCurr { l: 0, r: 7, dst: 2 }
        ));
    }

    #[test]
    fn test_fuse_leaf_assign_non_specialized_op_uses_stack_leaf() {
        // `dst = a > b`: Gt has no dedicated 3-operand leaf-assign opcode, so the
        // 3-window can't collapse it to a single op. But the operator-agnostic
        // 2-window still folds the second load: `LoadVar a` stands alone, then
        // `LoadVar b; BinOpAssignCurr(Gt)` becomes `AssignStackVarCurr{Gt}` with
        // the lhs (a) taken from the stack. Net 3->2 (still a dispatch saved).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Gt,
                    off: 2,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::LoadVar { off: 0 }));
        assert!(matches!(
            bc.code[1],
            Opcode::AssignStackVarCurr {
                dst: 2,
                b: 1,
                op: Op2::Gt
            }
        ));
    }

    #[test]
    fn test_fuse_stack_leaf_assign_order() {
        // `dst = (a - b) - c`: post-peephole, the inner `a - b` already fused to
        // BinOpAssign? No -- the inner is a pushing subexpr (`Op2`), the outer is
        // the assign. So the stream is LoadVar a; LoadVar b; Op2(Sub); LoadVar c;
        // BinOpAssignCurr(Sub). The triple folds to AssignSub..? No: the triple's
        // 3rd op is Op2, so it becomes BinVarVar; then [LoadVar c;
        // BinOpAssignCurr(Sub)] is the stack-leaf 2-window -> AssignStackVarCurr
        // with lhs = (a-b) on the stack, b = c. Sub is non-commutative: verify
        // lhs (stack) - rhs (c), not c - (a-b).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 }, // a
                Opcode::LoadVar { off: 1 }, // b
                Opcode::Op2 { op: Op2::Sub },
                Opcode::LoadVar { off: 2 }, // c
                Opcode::BinOpAssignCurr {
                    op: Op2::Sub,
                    off: 9,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(
            bc.code[0],
            Opcode::BinVarVar {
                l: 0,
                r: 1,
                op: Op2::Sub
            }
        ));
        assert!(matches!(
            bc.code[1],
            Opcode::AssignStackVarCurr {
                dst: 9,
                b: 2,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_stack_leaf_assign_const_next() {
        // `dst_next = (a - b) / 4`: BinVarVar then AssignStackConstNext.
        let mut bc = ByteCode {
            literals: vec![4.0],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Sub },
                Opcode::LoadConstant { id: 0 },
                Opcode::BinOpAssignNext {
                    op: Op2::Div,
                    off: 9,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::BinVarVar { .. }));
        assert!(matches!(
            bc.code[1],
            Opcode::AssignStackConstNext {
                dst: 9,
                b: 0,
                op: Op2::Div
            }
        ));
    }

    #[test]
    fn test_fuse_leaf_assign_preserves_max_stack_depth() {
        // Post-peephole leaf assigns must not change the (zero) net stack effect:
        // each `LoadX; LoadX; BinOpAssign` is +2 then -2. After fusion the single
        // op is (0,0). Build a sequence of leaf assigns and a nested one.
        let mut bc = ByteCode {
            literals: vec![2.0],
            code: vec![
                // x = a - b
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Sub,
                    off: 4,
                },
                // y = (c - d) * 2  (peak depth 2 mid-expression)
                Opcode::LoadVar { off: 2 },
                Opcode::LoadVar { off: 3 },
                Opcode::Op2 { op: Op2::Sub },
                Opcode::LoadConstant { id: 0 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Mul,
                    off: 5,
                },
            ],
        };
        let before = bc.max_stack_depth();
        bc.fuse_three_address();
        // Fusion folds loads into ops, so depth can only stay equal or shrink.
        assert!(bc.max_stack_depth() <= before);
        // And the leaf-assign collapsed to a (0,0) op, the stack-leaf assign to
        // (1,0): the whole stream's peak is now 1 (the BinVarVar push).
        assert_eq!(bc.max_stack_depth(), 1);
    }

    #[test]
    fn test_fuse_leaf_assign_blocked_by_jump_target() {
        // A jump targets the BinOpAssignCurr (the instruction the triple would
        // absorb). Fusing would make the jump land mid-fusion, so leave it alone.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Sub,
                    off: 2,
                }, // <- jump target
                Opcode::NextIterOrJump { jump_back: -1 },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[2], Opcode::BinOpAssignCurr { .. }));
    }

    #[test]
    fn test_fuse_noop_without_op2_and_empty() {
        // Nothing to fold into (no Op2): unchanged.
        let mut bc = ByteCode {
            literals: vec![1.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::AssignCurr { off: 0 },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::LoadConstant { id: 0 }));

        let mut empty = ByteCode::default();
        empty.fuse_three_address();
        assert!(empty.code.is_empty());
    }

    #[test]
    fn test_fuse_preserves_max_stack_depth() {
        // x = (a + b) * (c + d): peak depth 3. Fusion folds loads into ops, so
        // depth can only stay the same or shrink -- never grow (the Stack-safety
        // proof must survive fusion).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::LoadVar { off: 2 },
                Opcode::LoadVar { off: 3 },
                Opcode::Op2 { op: Op2::Add },
                Opcode::Op2 { op: Op2::Mul },
                Opcode::AssignCurr { off: 4 },
            ],
        };
        let before = bc.max_stack_depth();
        bc.fuse_three_address();
        assert!(bc.max_stack_depth() <= before);
    }

    #[test]
    fn test_fuse_triple_with_jump_target_at_first_instruction() {
        // A backward jump targets the first instruction of a fusable triple. The
        // triple still fuses (the fused op replaces the first instruction at the
        // same PC) and the jump offset is rewritten to land on it.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },               // [0] <- jump target
                Opcode::LoadVar { off: 1 },               // [1]
                Opcode::Op2 { op: Op2::Add },             // [2]
                Opcode::NextIterOrJump { jump_back: -3 }, // [3] -> [0]
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::BinVarVar { .. }));
        assert!(matches!(
            bc.code[1],
            Opcode::NextIterOrJump { jump_back: -1 }
        ));
    }

    #[test]
    fn test_fuse_blocked_when_absorbed_instruction_is_jump_target() {
        // A jump targets the Op2 (the instruction a triple would absorb). Fusing
        // would make the jump land mid-fusion, so the pass must leave it alone.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },               // [0]
                Opcode::LoadVar { off: 1 },               // [1]
                Opcode::Op2 { op: Op2::Add },             // [2] <- jump target
                Opcode::NextIterOrJump { jump_back: -1 }, // [3] -> [2]
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 4);
        assert!(matches!(bc.code[0], Opcode::LoadVar { off: 0 }));
        assert!(matches!(bc.code[1], Opcode::LoadVar { off: 1 }));
        assert!(matches!(bc.code[2], Opcode::Op2 { op: Op2::Add }));
        assert!(matches!(
            bc.code[3],
            Opcode::NextIterOrJump { jump_back: -1 }
        ));
    }

    // === 3-address fusion with GLOBAL operands and two-constant operands ===
    //
    // Globals (TIME/DT/...) load via `LoadGlobalVar`; the fusion now folds them
    // in operand position. Sub and Div are exercised because they are
    // non-commutative, so a swapped operand encoding is a loud failure rather
    // than a silent miscompile.

    #[test]
    fn test_fuse_global_var_pushing() {
        // `(g - v)` as a pushing subexpression (third op is Op2). The global is
        // the lhs leaf, the var the rhs leaf.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadGlobalVar { off: 0 },
                Opcode::LoadVar { off: 7 },
                Opcode::Op2 { op: Op2::Sub },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(
            matches!(
                bc.code[0],
                Opcode::BinGlobalVar {
                    l_global: 0,
                    r: 7,
                    op: Op2::Sub
                }
            ),
            "got {}",
            bc.code[0].name()
        );
    }

    #[test]
    fn test_fuse_var_global_div_order() {
        // `v / g`: var is the lhs (numerator), global the rhs (denominator).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 4 },
                Opcode::LoadGlobalVar { off: 1 },
                Opcode::Op2 { op: Op2::Div },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinVarGlobal {
                l: 4,
                r_global: 1,
                op: Op2::Div
            }
        ));
    }

    #[test]
    fn test_fuse_global_const_sub_order() {
        // `g - 5`: global lhs, const rhs.
        let mut bc = ByteCode {
            literals: vec![5.0],
            code: vec![
                Opcode::LoadGlobalVar { off: 0 },
                Opcode::LoadConstant { id: 0 },
                Opcode::Op2 { op: Op2::Sub },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinGlobalConst {
                l_global: 0,
                r: 0,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_const_global_div_order() {
        // `10 / g`: const lhs (numerator literal), global rhs (denominator).
        let mut bc = ByteCode {
            literals: vec![10.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadGlobalVar { off: 1 },
                Opcode::Op2 { op: Op2::Div },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinConstGlobal {
                l: 0,
                r_global: 1,
                op: Op2::Div
            }
        ));
    }

    #[test]
    fn test_fuse_global_global_sub_order() {
        // `g0 - g1`: both leaves are globals. Distinct offsets verify l/r order.
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadGlobalVar { off: 0 },
                Opcode::LoadGlobalVar { off: 1 },
                Opcode::Op2 { op: Op2::Sub },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinGlobalGlobal {
                l_global: 0,
                r_global: 1,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_stack_global() {
        // `(a - b) / g`: leaf triple -> BinVarVar; the outer `/ g` (lhs on stack)
        // -> BinStackGlobal. Div is non-commutative: lhs (stack) / rhs (g).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Sub },
                Opcode::LoadGlobalVar { off: 1 },
                Opcode::Op2 { op: Op2::Div },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(
            bc.code[0],
            Opcode::BinVarVar {
                l: 0,
                r: 1,
                op: Op2::Sub
            }
        ));
        assert!(matches!(
            bc.code[1],
            Opcode::BinStackGlobal {
                r_global: 1,
                op: Op2::Div
            }
        ));
    }

    #[test]
    fn test_fuse_const_const_sub_order() {
        // `5 - 2` from two distinct literals: fuses the two loads + Op2 into one
        // BinConstConst that still computes literals[0] - literals[1] at run time
        // (NOT compile-time folding -- the operands stay two separate literals).
        let mut bc = ByteCode {
            literals: vec![5.0, 2.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                Opcode::Op2 { op: Op2::Sub },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinConstConst {
                l: 0,
                r: 1,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_const_const_div_order() {
        // `10 / 4` from two distinct literals -> BinConstConst with l/r order.
        let mut bc = ByteCode {
            literals: vec![10.0, 4.0],
            code: vec![
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                Opcode::Op2 { op: Op2::Div },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 1);
        assert!(matches!(
            bc.code[0],
            Opcode::BinConstConst {
                l: 0,
                r: 1,
                op: Op2::Div
            }
        ));
    }

    #[test]
    fn test_fuse_global_leaf_assign_falls_through_to_stack_leaf() {
        // `dst = g - v`: post-peephole this is LoadGlobalVar; LoadVar;
        // BinOpAssignCurr. There is deliberately NO global *leaf-assign* opcode
        // (the global forms are pushing-only), so the 3-window does not collapse
        // it to a single op. The standalone LoadGlobalVar remains and the 2-window
        // folds the rhs+store into AssignStackVarCurr with the global as the
        // stack lhs (3->2, still a dispatch saved, and correct).
        let mut bc = ByteCode {
            literals: vec![],
            code: vec![
                Opcode::LoadGlobalVar { off: 0 },
                Opcode::LoadVar { off: 7 },
                Opcode::BinOpAssignCurr {
                    op: Op2::Sub,
                    off: 9,
                },
            ],
        };
        bc.fuse_three_address();
        assert_eq!(bc.code.len(), 2);
        assert!(matches!(bc.code[0], Opcode::LoadGlobalVar { off: 0 }));
        assert!(matches!(
            bc.code[1],
            Opcode::AssignStackVarCurr {
                dst: 9,
                b: 7,
                op: Op2::Sub
            }
        ));
    }

    #[test]
    fn test_fuse_global_const_const_preserve_max_stack_depth() {
        // Each new pushing form has the same net stack effect as the Load;Load;Op2
        // it replaces (+1), so fusion can only keep or shrink the peak depth --
        // the Stack-safety proof must survive. Mix global, const-const, and
        // stack-global forms.
        let mut bc = ByteCode {
            literals: vec![2.0, 3.0],
            code: vec![
                // (g - v) - here a pushing subexpr
                Opcode::LoadGlobalVar { off: 0 },
                Opcode::LoadVar { off: 1 },
                Opcode::Op2 { op: Op2::Sub },
                // ... / g   (lhs on stack -> BinStackGlobal)
                Opcode::LoadGlobalVar { off: 1 },
                Opcode::Op2 { op: Op2::Div },
                // + (2 - 3) (const-const pushing -> BinConstConst, peak +1)
                Opcode::LoadConstant { id: 0 },
                Opcode::LoadConstant { id: 1 },
                Opcode::Op2 { op: Op2::Sub },
                Opcode::Op2 { op: Op2::Add },
                Opcode::AssignCurr { off: 5 },
            ],
        };
        let before = bc.max_stack_depth();
        bc.fuse_three_address();
        assert!(bc.max_stack_depth() <= before);
    }
}

/// A single variable's compiled initial-value bytecode, along with the
/// data-buffer offsets it writes to (from AssignCurr nodes).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct CompiledInitial {
    // Used for diagnostics in debug_print_bytecode and set_value error messages
    #[allow(dead_code)]
    pub(crate) ident: Ident<Canonical>,
    /// Sorted, deduplicated offsets of all AssignCurr targets in this variable's
    /// initials bytecode.  Used in tests and debug printing.
    #[allow(dead_code)]
    pub(crate) offsets: Vec<usize>,
    pub(crate) bytecode: ByteCode,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, salsa::Update)]
pub struct CompiledModule {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) n_slots: usize,
    pub(crate) context: Arc<ByteCodeContext>,
    pub(crate) compiled_initials: Arc<Vec<CompiledInitial>>,
    pub(crate) compiled_flows: Arc<ByteCode>,
    pub(crate) compiled_stocks: Arc<ByteCode>,
    /// Opcode length of the run-invariant prefix of `compiled_flows.code`
    /// (time-invariant variable hoisting, GH #712). The flow runlist is
    /// partitioned so every run-invariant variable's bytecode precedes every
    /// dynamic variable's, and this is the boundary opcode index. `0` when no
    /// flow variable is run-invariant (every submodule, and any root model with
    /// no invariant flow var). Recorded on the **pre-fusion** resolved bytecode;
    /// the boundary is fusion-proof (no `fuse_three_address` window crosses a
    /// fragment boundary -- see the design note), so it stays a clean index. B1
    /// only records it; B2 uses it to run the invariant prefix once per
    /// `run_to`.
    pub(crate) flows_invariant_opcode_len: usize,
}
