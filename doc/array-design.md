# High-Level Design for Array Support in Simlin-Engine

## Overview

This document outlines the design for comprehensive array support in the simlin-engine, building upon the existing foundation while addressing gaps identified through analysis of the XMILE specification, current implementation, and test requirements.

## Current State Summary

The engine has substantial array support already implemented:
- Dimension definitions (indexed and named)
- Array variable declarations with dimension associations
- Subscript parsing and AST representation (including wildcards and element selection)
- Flat storage with offset calculation
- Element-wise operations between arrays (ApplyToAll and Arrayed)
- Array-to-array arithmetic operations (addition, multiplication, etc.)
- Scalar-to-array operations
- Aggregate functions (sum, mean, stddev, min, max, prod)
- Basic broadcasting support (e.g., 2D array * 1D array)
- Partial reduction operations (e.g., SUM along specific dimensions)

Key limitations:
- Limited slicing with range operations (e.g., a[1:3])
- No transpose operator (')
- Missing dimension position operator (@)
- Missing array manipulation functions
- Incomplete error handling for invalid indices
- No explicit array constructors

## Design Goals

1. **XMILE Compliance**: Full support for array features as specified in XMILE v1.0
2. **Performance**: Efficient array operations without unnecessary allocations
3. **Safety**: Proper bounds checking with configurable invalid index behavior
4. **Extensibility**: Architecture that allows adding new array operations easily
5. **Compatibility**: Maintain backward compatibility with existing models

## Core Concepts

### 1. XMILE Array Operations

Per the XMILE specification, array operations follow specific rules:
- Arithmetic operators perform element-by-element operations (not linear algebra operations)
- Operations with scalars apply to each element
- Addition and subtraction of same-sized arrays work element-wise
- Multiplication, division, and exponentiation are element-by-element (not matrix operations)

**Current Broadcasting Support:**
The engine already supports some broadcasting scenarios:
- 1D array can multiply with 2D array when dimensions match (e.g., `array[A,B] * array[A]`)
- This is implemented through the existing ApplyToAll and expression evaluation

**Required XMILE Features Not Yet Implemented:**
1. **Transpose operator** (`'`): Reverses array dimensions
2. **Dimension position operator** (`@`): References dimensions by position
3. **Range subscripts**: Selecting subarrays with syntax like `array[1:3, *]`

### 2. Temporary Array Storage Management

Rather than creating new variables and rewriting ASTs, array operations will use a temporary storage system with unique IDs:

**Key concepts:**
- **ArrayView**: Describes how to access array data
  - `Contiguous`: Simple arrays with uniform stride (most common case)
  - `Strided`: For transposed arrays, slices, and other complex views
- **ArraySource**: Identifies where array data lives
  - `Named(Ident, ArrayView)`: References a named variable
  - `Temp(u32, ArrayView)`: References temporary storage with a unique ID

**ArrayView enum variants:**
```rust
enum ArrayView {
    Contiguous {
        shape: DimensionVec,  // Shape is all we need for row-major arrays
    },
    Strided {
        shape: DimensionVec,
        strides: Vec<isize>,  // One stride per dimension
        offset: usize,        // Starting offset
    },
}
```

For `Contiguous` arrays, strides are implicit and calculated from the shape (row-major order):
- Last dimension has stride 1
- Each previous dimension's stride = next dimension's stride × next dimension's size
- Total elements = product of all dimension sizes

**How it works:**
- Each AST node that produces an array result gets an `Option<ArraySource>`
- During compilation, temporary IDs are assigned where needed
- During evaluation, space is allocated for each temporary ID
- Subscript operations modify the ArrayView without allocating new storage
- Transpose creates a `Strided` view with reordered strides (no data copy)
- Slicing adjusts offset and strides to create sub-array views

This approach avoids AST rewriting while enabling efficient array operations.

### 3. Array Expression Types

Current support includes:
- **Element-wise operations**: Already implemented through existing expression evaluation
- **Reduction operations**: `SUM(a[*])`, `MEAN(a[DimA, *])` - already supported
- **Basic subscripting**: `a[DimA.Boston, *]` - already supported

Still needed:
- **Range slicing**: `a[1:3, *]`, `a[DimA.Boston:DimA.LA, *]`
- **Transpose**: `a'` or `a[DimA, DimB]'`
- **Dimension position**: `a[@2, @1]` for reordering dimensions

### 4. Dimension Context Enhancement

Enhance dimension handling to support:
- **Dimension arithmetic**: `Location.Boston + 1` → `Location.Chicago`
- **Dimension ranges**: `DimA.Start:DimA.End`
- **Dynamic dimension queries**: `SIZE(array, dimension_index)`
- **Dimension membership tests**: `IS_IN(index, dimension)`

## Proposed Architecture

### 1. AST Extensions

Since element-wise operations are already supported through the existing expression evaluation, the main AST extensions needed are:

```rust
enum Expr {
    // Existing variants...
    
    // New array-specific variants
    Transpose {
        array: Box<Expr>,
        loc: Loc,
    },
    
    Slice {
        array: Box<Expr>,
        indices: Vec<SliceSpec>,
        loc: Loc,
    },
}

enum SliceSpec {
    Index(IndexExpr),      // Existing
    Range(Option<Expr>, Option<Expr>), // New: for a[1:3]
    All,                   // Existing: for a[*]
    DimensionPosition(u32), // New: for @1, @2, etc.
}
```

Note: The existing `Expr::Subscript` already handles basic indexing, but needs enhancement for ranges.

### 2. Compiler Enhancements

The compiler will:
1. **Analyze array shapes**: Determine result shapes for all operations
2. **Insert broadcast operations**: Add temporary variables for broadcasting
3. **Generate slice views**: Create efficient views without copying data
4. **Optimize access patterns**: Reorder operations for cache efficiency

### 3. VM Enhancements

The existing VM already handles element-wise operations through its Op2 instructions. New functionality needed:

1. **Slice operations**: Create `Strided` views with adjusted offsets and strides
2. **Transpose operations**: Create `Strided` views with reordered strides
3. **View-aware iteration**: Handle both `Contiguous` and `Strided` array access patterns

**Example: Transpose Implementation**
For a 2D array with shape [3, 4] and row-major layout:
- Original: `Contiguous { shape: [3, 4] }` (implicit strides: [4, 1])
- After transpose: `Strided { shape: [4, 3], strides: [1, 4], offset: 0 }`

This swaps how we iterate: instead of moving by 4 elements between rows (implicit), we move by 1; instead of moving by 1 between columns (implicit), we move by 4.

Most operations can be implemented through the existing bytecode infrastructure with additional support in the compiler for generating the appropriate instruction sequences.

### 4. Runtime Array Metadata

Track array metadata for efficient operations:
```rust
struct ArrayInfo {
    shape: Vec<usize>,
    strides: Vec<isize>,  // Support for non-contiguous arrays
    offset: usize,
    is_view: bool,
    base_array: Option<VarId>,
}
```

## Implementation Phases

### Phase 1: Core XMILE Features
1. **Range subscripts**: Implement `array[start:end]` syntax
   - Extend parser to handle range syntax
   - Generate temporary arrays for slice results
   - Handle edge cases and bounds checking
2. **Transpose operator**: Implement `array'` syntax
   - Add to parser with correct precedence
   - Implement dimension reversal in compiler
   - Create efficient view without copying data

### Phase 2: Advanced XMILE Features  
1. **Dimension position operator**: Implement `@n` syntax
   - Parse and resolve dimension positions
   - Support in subscript expressions
   - Handle dimension reordering
2. **Enhanced error handling**:
   - Configurable invalid index behavior (0 or NaN)
   - Clear shape mismatch error messages
   - Runtime bounds checking

### Phase 3: Optimization & Extensions
1. **Performance optimizations**:
   - Lazy evaluation for slices (views instead of copies)
   - Cache-aware memory layouts
   - Vectorized operations where possible
2. **Additional array functions** (if needed):
   - Array constructors
   - Additional statistical functions
   - Matrix operations (if required beyond XMILE)

## Error Handling

1. **Invalid indices**: Return configurable value (0 or NaN) as per XMILE spec
2. **Shape mismatches**: Clear error messages with shape information
3. **Dimension errors**: Report which dimensions don't match
4. **Performance warnings**: Alert on inefficient access patterns

## Testing Strategy

1. **Unit tests**: Each array operation in isolation
2. **Integration tests**: Complex array expressions
3. **XMILE compliance**: Test against sdeverywhere array models
4. **Performance tests**: Ensure operations scale well
5. **Error case tests**: Verify all error conditions handled

## Future Considerations

1. **GPU acceleration**: Design allows for future GPU backend
2. **Sparse arrays**: Metadata structure supports sparse storage
3. **Complex indexing**: Boolean masks, indirect indexing
4. **Array functions library**: Statistical, financial, etc.

## Conclusion

This design focuses on implementing the specific array features required by the XMILE specification that are not yet supported in simlin-engine. The engine already has robust support for basic array operations, element-wise arithmetic, and some broadcasting. The main gaps are:

1. Range-based subscripting (e.g., `array[1:3]`)
2. Transpose operator (`'`)
3. Dimension position operator (`@`)

By focusing on these specific features rather than reimplementing existing functionality, we can achieve full XMILE compliance efficiently. The phased approach prioritizes the most commonly used features first, with optimizations and extensions following as needed.