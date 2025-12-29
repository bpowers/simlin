// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use std::collections::HashMap;
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
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeSparseMapping {
    /// Which dimension (0-indexed) in the view is sparse
    pub dim_index: u8,
    /// Parent offsets to iterate (e.g., [0, 2] for elements at indices 0 and 2)
    pub parent_offsets: SmallVec<[u16; 16]>,
}

/// A runtime array view used during VM execution.
/// More dynamic than compile-time ArrayView - supports incremental building.
#[derive(Clone, Debug, PartialEq)]
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

#[derive(Copy, Clone, Debug)]
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

#[derive(Copy, Clone, Debug)]
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
#[derive(Clone, Debug)]
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
    Lookup {
        gf: GraphicalFunctionId,
    },

    // =========================================================================
    // ARRAY SUPPORT (new)
    // =========================================================================

    // === VIEW STACK: Building views dynamically ===
    /// Push a view for a variable's full array onto the view stack.
    /// Looks up dimension info to compute strides.
    PushVarView {
        base_off: VariableOffset, // Variable offset in curr[]
        n_dims: u8,               // Number of dimensions (1-4)
        dim_ids: [DimId; 4],      // Dimension IDs (padded with 0 if < 4)
    },

    /// Push a view for a temp array onto the view stack.
    PushTempView {
        temp_id: TempId,
        n_dims: u8,
        dim_ids: [DimId; 4],
    },

    /// Push a pre-computed static view onto the view stack.
    PushStaticView {
        view_id: ViewId,
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

    /// Apply range subscript [start:end) to a dimension.
    ViewRange {
        dim_idx: u8,
        start: u16,
        end: u16,
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

// ============================================================================
// Module and Array Declarations
// ============================================================================

#[derive(Clone, Debug)]
pub struct ModuleDeclaration {
    pub(crate) model_name: Ident<Canonical>,
    pub(crate) off: usize, // offset within the parent module
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ArrayDefinition {
    pub(crate) dimensions: Vec<usize>,
}

/// A static array view for compile-time known subscripts.
/// Stored in ByteCodeContext and referenced by ViewId.
#[derive(Clone, Debug, PartialEq)]
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
#[derive(Clone, Debug, Default)]
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
}

#[derive(Clone, Debug, Default)]
pub struct ByteCode {
    pub(crate) literals: Vec<f64>,
    pub(crate) code: Vec<Opcode>,
}

#[derive(Clone, Debug, Default)]
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

    pub(crate) fn finish(self) -> ByteCode {
        self.bytecode
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
    fn test_opcode_size() {
        use std::mem::size_of;
        // With array support opcodes (PushVarView has [DimId; 4] = 8 bytes),
        // the opcode size increases. We accept up to 16 bytes.
        let size = size_of::<Opcode>();
        assert!(size <= 16, "Opcode size {} exceeds 16 bytes", size);
        // Print actual size for documentation
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
}

#[derive(Clone, Debug)]
pub struct CompiledModule {
    pub(crate) ident: Ident<Canonical>,
    pub(crate) n_slots: usize,
    pub(crate) context: Arc<ByteCodeContext>,
    pub(crate) compiled_initials: Arc<ByteCode>,
    pub(crate) compiled_flows: Arc<ByteCode>,
    pub(crate) compiled_stocks: Arc<ByteCode>,
}
