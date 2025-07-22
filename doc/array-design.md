# High-Level Design for Array Support in Simlin-Engine

## Overview

This document outlines the design for comprehensive array support in `simlin-engine`, which is needed to run large existing and important models.
It builds upon the existing foundation while addressing gaps with whats outlined in the XMILE specification, current implementation, and test requirements.

## High-level design

System dynamics models as specified in the [XMILE standard](./xmile-v1.0.html) support a rich syntax around accessing and slicing arrays, much of which (e.g., `array[time]` or `array[some_variable]`) is dynamic simulation-time behavior.
This means we cannot determine exact strides, offsets, or even which elements will be accessed during the equation parsing or compilation. 

### Multi-Phase Array Handling

1. **Parser (Expr0)**: Captures all array syntax including subscripts, transpose, dimension positions (DONE)
2. **Type Checking (Expr2)**: Focuses on: (IN PROGRESS)
   - Computing **maximum** array bounds (conservative estimates)
   - Basic array-size compatibility checks
   - Determining if subscripts are static or dynamic
   - Calculating temporary storage requirements, e.g. uniquely numbering temporaries
3. **Compiler**: Static optimizations and efficiently preparing for runtime evaluation: (TODO)
   - Pulls to "compile time" what we can, e.g. stride calculations for static subscripts
   - View creation (transpose, slicing)
   - Identifies how much temporary storage to allocate for array temporaries
     - in the interpreter and VM we will use this to allocate one scratch buffer that we bump-allocate array temporaries out of  
4. **Interpreter/VM**: Runtime evaluation (TODO)
   - We have both an AST-walking interpreter and bytecode VM, we implement the same semantics in both to validate our implementations produce the same behavior
   - Dynamic subscript evaluation
   - Actual array operations and array-based builtin functions
   
### Simplified Expr2 Representation

Instead of complex ArrayView with Contiguous/Strided variants, Expr2 will use:

```rust
// Simplified array information in Expr2
struct ArrayBounds {
    dims:       Vec<(bool, usize)>,  // tuple of "is_dynamic" and "max size"
}

// ArraySource now references ArrayBounds instead of ArrayView
pub enum ArraySource {
   Named(Ident, ArrayBounds),
   Temp(u32, ArrayBounds),
}
```

### Compiler Subscript Types

The compiler will distinguish between static and dynamic subscripts by having separate enum variants in its `Expr` enum:

```rust
pub enum Expr {
   // ...
   StaticSubscript(usize, ArrayView, Loc), // offset, index expression, bounds
   DynamicSubscript(usize, DynamicArrayView<Expr>, Loc), // offset, index expression, bounds
   // ...
}
```

This separation allows the compiler to optimize static subscripts while properly handling dynamic ones.

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
- Transpose operator (')
- Dimension position operator (@)

Key limitations:
- Limited slicing with range operations (e.g., a[1:3])
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
1. ~~**Transpose operator** (`'`): Reverses array dimensions~~ **IMPLEMENTED** - Parser support added, AST nodes created, awaiting compiler implementation
2. ~~**Dimension position operator** (`@`): References dimensions by position~~ **IMPLEMENTED** - Parser support added, AST nodes created, awaiting compiler implementation
3. **Range subscripts**: Selecting subarrays with syntax like `array[1:3, *]`

### 2. Temporary Array Storage Management (Simplified)

The simplified design moves array view complexity from Expr2 to the compiler:

**Expr2 Phase (Simple)**:
- Each array-producing expression gets a `temp_id: Option<u32>` 
- Array bounds are tracked as maximum possible sizes
- No complex ArrayView or ArraySource types needed

**Compiler Phase (Complex)**:
Array views are computed during compilation when we have more context.

**Benefits of Simplified Approach**:
- **Cleaner Expr2**: No complex ArrayView propagation through AST
- **Better dynamic handling**: Compiler can generate different code paths for static vs dynamic cases
- **Easier optimization**: All array layout decisions happen in one place
- **Reduced AST size**: No ArraySource field on every expression

**How it works**:
1. During Expr2 transformation, assign temp IDs to intermediate array results
2. Track maximum bounds for type checking
3. In compiler, analyze subscript patterns and generate appropriate views
4. For static subscripts: compute exact offsets and strides at compile time
5. For dynamic subscripts: generate bounds checking and offset calculation code

### 3. Array Expression Types

Current support includes:
- **Element-wise operations**: Already implemented through existing expression evaluation
- **Reduction operations**: `SUM(a[*])`, `MEAN(a[DimA, *])` - already supported
- **Basic subscripting**: `a[DimA.Boston, *]` - already supported

Still needed:
- **Range slicing**: `a[1:3, *]`, `a[DimA.Boston:DimA.LA, *]`
- ~~**Transpose**: `a'` or `a[DimA, DimB]'`~~ **IMPLEMENTED** in parser
- ~~**Dimension position**: `a[@2, @1]` for reordering dimensions~~ **IMPLEMENTED** in parser

### 4. Dimension Context Enhancement

Enhance dimension handling to support:
- **Dimension arithmetic**: `Location.Boston + 1` → `Location.Chicago`
- **Dimension ranges**: `DimA.Start:DimA.End`
- **Dynamic dimension queries**: `SIZE(array, dimension_index)`
- **Dimension membership tests**: `IS_IN(index, dimension)`

## Examples of Simplified Approach

### Example 1: Static Subscript
```
array[Location.Boston, *]
```
- **Expr2**: Records max bounds = [size of second dimension], static subscript
- **Compiler**: Computes exact offset = Boston's index × stride of first dimension
- **VM**: Direct memory access with pre-computed offset

### Example 2: Dynamic Subscript
```
array[time, Product.A]
```
- **Expr2**: Records max bounds = scalar, dynamic first subscript
- **Compiler**: Generates code to evaluate `time` and compute offset at runtime
- **VM**: Bounds check, then access with computed offset

### Example 3: Transpose with Range
```
matrix[1:3, *]'
```
- **Expr2**: Records max bounds = [3, second dimension size], has transpose
- **Compiler**: 
  - Creates view with offset=0, adjusted first dimension size
  - Swaps stride order for transpose
- **VM**: Iterates with transposed strides

### Example 4: Complex Dynamic Expression
```
array[@(dim_index), time:time+5]
```
- **Expr2**: Records max bounds = [first dim size, 5], both subscripts dynamic
- **Compiler**: Generates code to:
  - Evaluate `dim_index` and validate it's a valid dimension position
  - Evaluate `time` and `time+5` for range bounds
  - Compute appropriate view at runtime
- **VM**: Dynamic bounds checking and view creation

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

## Current Implementation Status

### Successfully Implemented Features

#### 1. Simplified Expr2 Array Representation ✅
- **ArrayBounds**: Simplified enum with just `Named` and `Temp` variants (as designed)
- **Temp ID allocation**: Working context-based allocation system
- **Dimension tracking**: Properly tracks maximum bounds for type checking
- **Tests**: Comprehensive test coverage for array bounds tracking and propagation

#### 2. Transpose Operator (`'`) - Partially Complete
- **Parser Support**: ✅ Full support for parsing transpose operator as a postfix unary operator
- **AST Representation**: ✅ Added `Transpose` variant to `UnaryOp` enum
- **Expr2 Handling**: ✅ Properly reverses dimensions during type checking
- **Tests**: ✅ Comprehensive parser and Expr2 tests
- **Compiler**: ❌ Returns `ArraysNotImplemented` error - needs implementation
- **VM/Interpreter**: ❌ Not yet implemented

#### 3. Dimension Position Operator (`@`) - Partially Complete
- **Parser Support**: ✅ Full support for parsing `@n` syntax in subscript expressions
- **AST Representation**: ✅ Added `DimensionPosition(u32, Loc)` variant to all `IndexExpr{0,1,2}` enums
- **Lexer**: ✅ Added `At` token and lexer rule for `@` character
- **Expr2 Handling**: ✅ Properly tracked through type checking phase
- **Tests**: ✅ Comprehensive parser tests including `a[@1]`, `a[@3, @2, @1]`
- **Compiler**: ❌ Returns `ArraysNotImplemented` error - needs implementation
- **VM/Interpreter**: ❌ Not yet implemented

#### 4. Range Subscripts - Partially Complete
- **Parser Support**: ✅ Can parse range syntax like `a[1:3]`
- **AST Representation**: ✅ `Range` variant in `IndexExpr{0,1,2}`
- **Expr2 Handling**: ✅ Properly tracked through type checking phase
- **Compiler**: ❌ Returns `TodoRange` error - needs implementation
- **VM/Interpreter**: ❌ Not yet implemented

### Implementation Details

The implementation follows a clean architecture:
1. **Lexer** recognizes new tokens (`'` as `Apostrophe`, `@` as `At`)
2. **Parser** constructs appropriate AST nodes with proper precedence
3. **AST transformations** properly handle new variants through expr0→expr1→expr2 pipeline
4. **Visitor patterns** updated to handle new AST nodes
5. **Error handling** returns appropriate error codes when features aren't fully implemented

## Implementation Phases

### Phase 1: Simplify Expr2 Array Representation ✅ COMPLETE
1. **Remove ArrayView complexity from Expr2**: ✅
   - Replaced with simple ArrayBounds enum
   - Added temp_id tracking for intermediate results
   - Proper dimension tracking for type checking
2. **Update AST transformations**: ✅
   - Simplified expr0→expr1→expr2 array handling
   - Focus on maximum bounds computation
   - Tests verify correct array bounds propagation

### Phase 2: Enhanced Compiler Array Support 🚧 IN PROGRESS
This is the current focus area. The compiler needs to handle the array operations that are already parsed and type-checked.

1. **Split Subscript handling** (TODO):
   - Create StaticSubscript for compile-time resolution
   - Create DynamicSubscript for runtime evaluation
   - Generate efficient code for each case
2. **Implement array operations in compiler** (TODO):
   - **Transpose**: Generate stride-swapped views (currently returns `ArraysNotImplemented`)
   - **Ranges**: Handle slice bounds and view creation (currently returns `TodoRange`)
   - **Dimension positions**: Resolve @n references (currently returns `ArraysNotImplemented`)
   - **Wildcards**: Handle wildcard subscripts (currently returns `TodoWildcard`)
   - **Star ranges**: Handle *:dimension syntax (currently returns `TodoStarRange`)
3. **VM instruction updates** (TODO):
   - Add instructions for dynamic view creation
   - Implement efficient bounds checking
   - Support strided array iteration

### Phase 3: Interpreter Support (TODO)
Parallel to compiler work, the interpreter needs the same functionality:
1. **Array view operations**: Transpose, slicing, dimension reordering
2. **Dynamic subscript evaluation**: Runtime bounds checking and offset calculation
3. **Test parity**: Ensure interpreter and VM produce identical results

### Phase 4: Optimization (FUTURE)
1. **Static subscript optimization**:
   - Pre-compute all offsets at compile time
   - Eliminate runtime bounds checks where possible
   - Inline simple array accesses
2. **Dynamic subscript optimization**:
   - Cache computed offsets when possible
   - Optimize common patterns (sequential access)
   - Vectorize where applicable

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

This design has evolved from a complex ArrayView-based approach to a simplified three-phase architecture that better handles the realities of dynamic subscripts in system dynamics models.

**Key Design Decisions**:

1. **Simplified Expr2**: ✅ **IMPLEMENTED** - Successfully removed complex ArrayView types from Expr2, focusing on maximum bounds computation. This has reduced AST complexity and made the type checking phase more maintainable.

2. **Compiler-Centric Array Handling**: Moving stride calculations, view creation, and offset computation to the compiler allows us to optimize static cases while properly handling dynamic subscripts. This is the current focus area.

3. **Static vs Dynamic Distinction**: The design calls for explicitly separating static and dynamic subscript handling in the compiler to enable better optimization opportunities and clearer code paths.

**Current Status (January 2025)**:

1. **Parser**: ✅ Fully supports all XMILE array syntax (transpose, dimension positions, ranges)
2. **Expr2**: ✅ Successfully simplified with ArrayBounds replacing complex ArrayView types
3. **Compiler**: 🚧 Needs implementation of array operations (currently returns error codes)
4. **VM/Interpreter**: ⏳ Needs array view operations and dynamic subscript support

## Recommended Next Steps

Based on the current implementation status, here's the prioritized roadmap:

### Immediate Priority: Compiler Array Operations
The most pressing need is to implement array operations in the compiler. Start with the simplest cases and build up:

1. **Implement Wildcard Subscripts** (`TodoWildcard`)
   - These are the simplest to implement
   - Create a view that preserves the dimension
   - Good starting point for array view infrastructure

2. **Implement Dimension Position** (`ArraysNotImplemented` for `@n`)
   - Relatively straightforward dimension reordering
   - Build on wildcard implementation
   - Can be statically resolved at compile time

3. **Implement Transpose** (`ArraysNotImplemented` for `'`)
   - Similar to dimension position but simpler (just reverse)
   - Reuse dimension reordering logic
   - Good test case for stride manipulation

4. **Implement Range Subscripts** (`TodoRange`)
   - More complex due to dynamic bounds
   - Requires runtime bounds checking
   - Foundation for more advanced slicing

5. **Implement Star Ranges** (`TodoStarRange`)
   - Most complex subscript type
   - Builds on range implementation
   - May require dimension name resolution

### Secondary Priority: Interpreter Parity
Once compiler implementations are working, ensure the interpreter has matching functionality for testing and validation.

### Future Work
- Static optimization for known compile-time subscripts
- Efficient memory layout for common access patterns
- Advanced array functions and operations

By following this incremental approach, we can systematically build up full array support while maintaining a working system at each step.