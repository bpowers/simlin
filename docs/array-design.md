# Array Support in Simlin-Engine

## Overview

The simlin-engine implements comprehensive array support following the XMILE v1.0 specification. The implementation is complete for the core array functionality, with a few edge cases remaining as documented below.

## Subscript Notation Quick Reference

* Array indexing is 1-based in source equations, converted to 0-based internally
* Numeric ranges like `1:5` are inclusive of both start and end (selects elements 1,2,3,4,5)
* Named ranges like `Cities.Boston:Cities.Seattle` are inclusive
* Wildcards (`*`) preserve the dimension in the result
* Star ranges (`*:SubDim`) select elements matching a subdimension
* Dimension position (`@n`) references the nth dimension in subscripts
* Transpose (`'`) reverses dimension order

## Architecture

### Processing Pipeline

Array expressions go through a multi-phase compilation pipeline:

1. **Parser (Expr0 -> Expr1)**: Captures all XMILE array syntax
2. **Type Checker (Expr1 -> Expr2)**: Computes array bounds, validates dimensions
3. **Pass 0 (Expr2 -> Expr2)**: Normalizes array expressions
   - Expands bare array references to explicit subscripts
   - Normalizes wildcards (e.g., `*` to `dim.*`)
   - Ensures all Var references can be treated as scalars in later phases
4. **Pass 1 (Expr2 -> Expr3)**: Generates temp array assignments
   - Creates `AssignTemp` expressions for complex array builtin arguments
   - Handles expressions that don't need A2A-element-specific behavior
5. **Compiler (Expr3 -> Expr)**: Creates optimized expressions
   - Resolves static subscripts into `ArrayView` instances
   - Generates `StaticSubscript`, `TempArray`, `TempArrayElement` expressions
6. **Bytecode Generation**: Emits VM opcodes
   - View stack operations for array access
   - Iteration loops for element-wise operations
   - Reduction opcodes for array builtins

### Core Data Structures

```rust
// Efficient array view representation (no data copying)
pub struct ArrayView {
    pub dims: Vec<usize>,      // Dimension sizes after slicing
    pub strides: Vec<isize>,   // Elements to skip per dimension
    pub offset: usize,         // Starting offset in underlying data
}

// Runtime view with dimension identity tracking
pub struct RuntimeView {
    pub base_off: usize,       // Base variable offset
    pub dims: Vec<ViewDim>,    // Dimension info with dim_ids
    pub is_valid: bool,        // For out-of-bounds detection
    pub sparse: Vec<(usize, Vec<usize>)>, // Sparse iteration support
}

// Compiler expression types
pub enum Expr {
    Var(usize, Loc),                              // Scalar variable
    Subscript(usize, Vec<Expr>, Vec<usize>, Loc), // Dynamic subscript
    StaticSubscript(usize, ArrayView, Loc),       // Optimized static subscript
    TempArray(u32, ArrayView, Loc),               // Reference to temp array
    TempArrayElement(u32, ArrayView, usize, Loc), // Single temp element
    AssignTemp(u32, Box<Expr>, ArrayView, Loc),   // Populate temp array
    // ... other variants
}
```

### Dimension Matching Algorithm

When combining arrays with different dimensions (broadcasting), the VM uses a two-pass algorithm:

1. **Pass 1 - Name Matching**: Match dimensions by exact `dim_id` (semantic identity)
2. **Pass 2 - Positional Matching**: For **indexed dimensions only**, fall back to size-based matching

Named dimensions (e.g., `Cities=[Boston,Seattle]`) must match by name because their elements have semantic meaning. Two different named dimensions of the same size will NOT match.

Indexed dimensions (e.g., numeric dimensions like `Periods(5)`) can use positional matching when names don't match but sizes do.

## Implementation Status

### Fully Implemented

#### Parser
- Transpose operator (`'`)
- Dimension position (`@n`)
- Range subscripts: numeric (`[1:3]`) and named (`[Boston:LA]`)
- Wildcard subscripts (`[*]`)
- Star ranges (`[*:SubDim]`)

#### Type Checker
- Array bounds propagation
- Dimension compatibility validation
- Temp ID allocation for intermediate results

#### Compiler
- `ArrayView` abstraction for zero-copy array operations
- Static subscript optimization
- Named element resolution (case-insensitive)
- Range slicing with proper inclusive semantics
- Transpose support
- Dimension position handling
- Expression rewriting for array builtins
- All expression types: `StaticSubscript`, `TempArray`, `TempArrayElement`, `AssignTemp`

#### Bytecode VM
- View stack operations: `PushVarView`, `PushTempView`, `PushStaticView`
- View manipulation: `ViewSubscriptConst/Dynamic`, `ViewRange/Dynamic`, `ViewStarRange`, `ViewWildcard`, `ViewTranspose`
- Iteration: `BeginIter`, `LoadIterElement`, `StoreIterElement`, `NextIterOrJump`, `EndIter`
- Reductions: `ArraySum`, `ArrayMax`, `ArrayMin`, `ArrayMean`, `ArrayStddev`, `ArraySize`
- Broadcasting: `BeginBroadcastIter`, `LoadBroadcastElement`, `StoreBroadcastElement`
- Sparse iteration support via pre-computed flat offsets

#### Array Builtins (in both interpreter and VM)
- `SUM(array)` - Sum all elements
- `MEAN(array)` - Arithmetic mean
- `STDDEV(array)` - Standard deviation
- `MIN(array)` - Minimum value
- `MAX(array)` - Maximum value
- `SIZE(array)` - Element count

#### Dimension Features
- Named dimensions with element lookup
- Indexed dimensions with numeric access
- Subdimension detection and star range support
- Broadcasting between compatible dimensions
- Different-named indexed dimensions can broadcast by position

### Remaining Work

#### 1. Cross-Dimension Broadcasting in Array Builtins

**Issue**: When array builtins like `SUM(a[*]+h[*])` combine arrays with different dimensions (e.g., `a[DimA]` and `h[DimC]`), the expected XMILE behavior is a cross-product (3x3 = 9 elements summed), but the current implementation treats this as element-wise.

**Test**: `simulates_sum` in `src/simlin-compat/tests/simulate.rs` (ignored)

**Example**:
```
a[DimA] = [1, 2, 3]  // DimA = {A1, A2, A3}
h[DimC] = [10, 20, 30]  // DimC = {C1, C2, C3}
result = SUM(a[*] + h[*])
// Expected: 198 (sum of 3x3 cross-product: 11+21+31+12+22+32+13+23+33)
// Current: 66 (sum of 3 elements: 11+22+33)
```

#### 2. Slice Assignment Size Mismatch Behavior

**Issue**: When assigning a smaller array slice to a larger array (e.g., `array[5] = source[1:3]`), elements beyond the slice should be NaN or zero, not a repeat of the last value.

**Tests**: `range_basic`, `range_with_expressions` in `src/simlin-engine/src/array_tests.rs` (ignored)

**Example**:
```
source[Periods(5)] = [1, 2, 3, 4, 5]
slice[Periods(5)] = source[1:3]
// Expected: [1.0, 2.0, 3.0, NaN, NaN] or [1.0, 2.0, 3.0, 0.0, 0.0]
// Current: [1.0, 2.0, 3.0, 3.0, 3.0] (extends last element)
```

#### 3. Out-of-Bounds Iteration

**Issue**: When iterating over mismatched-size arrays in A2A context, out-of-bounds accesses should return NaN.

**Tests**: `out_of_bounds_iteration_returns_nan`, `bounds_check_in_fast_path` in `src/simlin-engine/src/array_tests.rs` (ignored)

**Notes**: VM changes are in place (`is_valid` flag on `RuntimeView`), but compiler needs to generate code that properly creates mismatched-size views.

#### 4. Complex Combined Operations

**Issue**: Some edge cases with transpose combined with slicing may not work correctly.

**Test**: `transpose_and_slice` in `src/simlin-engine/src/array_tests.rs` (ignored)

#### 5. EXCEPT Builtin

**Issue**: The EXCEPT builtin for Vensim compatibility requires subscript mapping information that isn't preserved in XMILE conversion.

**Test**: `simulates_except` in `src/simlin-compat/tests/simulate.rs` (ignored)

**Note**: This is a Vensim compatibility issue, not core array support.

## Testing

### Passing Test Coverage

The following test categories exercise array functionality and pass:

- `simulates_arrays` - Basic array model
- `simulates_array_sum_simple` - SUM with simple expressions
- `simulates_array_sum_expr` - SUM with complex expressions
- `simulates_array_multi_source` - Multiple arrays in expressions
- `simulates_array_broadcast` - Cross-dimension broadcasting
- `different_indexed_dims_same_size_broadcast` - Indexed dimension broadcasting
- `star_range_indexed_subdimension` - Star ranges with indexed subdimensions
- `named_dims_same_size_no_fallback` - Named dimensions require name matching

### Test Files

- `test/arrays1/arrays.stmx` - Basic array access
- `test/test-models/tests/subscript_1d_arrays/` - 1D array operations
- `test/test-models/tests/subscript_2d_arrays/` - 2D array operations
- `test/test-models/samples/arrays/a2a/` - Apply-to-all arrays
- `test/test-models/samples/arrays/non-a2a/` - Non-A2A arrays

## Design Principles

### Zero-Copy Operations
`ArrayView` enables efficient array operations by adjusting iteration patterns rather than copying data. This is crucial for large array performance.

### Static Optimization
The compiler aggressively optimizes array operations resolvable at compile time, reducing runtime overhead for common patterns.

### XMILE Compliance
All array operations follow XMILE v1.0 semantics:
- Element-wise arithmetic (not linear algebra)
- 1-based indexing in source, 0-based internally
- Inclusive ranges

### Semantic Dimension Matching
Named dimensions preserve semantic meaning - `Cities` will never match `Products` even if both have the same size. This prevents subtle bugs from accidental dimension mismatches.

## Future Enhancements

If additional array features are needed:

1. **RANK builtin**: Query number of dimensions
2. **View composition optimization**: Combine multiple view operations into single view
3. **Contiguous detection fast path**: Optimize for contiguous array access
4. **SIMD operations**: Vectorize array operations where possible
