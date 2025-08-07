# High-Level Design for Array Support in Simlin-Engine

## Executive Summary

The simlin-engine has initial implementations of several XMILE array features in the compiler and AST-walking interpreter. However, these features are **not usable in practice** because they are not implemented in the bytecode VM, which is the primary execution engine.

🚧 **Partially Implemented** (Compiler + AST interpreter only, NOT in bytecode VM):
- **ArrayView abstraction** - Efficient strided array access (compiler only)
- **Wildcard subscripts** (`a[*]`) - Works in compiler and AST interpreter
- **Range subscripts** (`a[1:3]`) - Initial support in compiler, SUM works in AST interpreter
- **Dimension positions** (`a[@2,@1]`) - Basic support in compiler
- **StaticSubscript** - Compile-time optimized array access (compiler only)

⚠️ **Very Limited Implementation**:
- **Transpose operator** (`a'`) - Only works for subscripted arrays in compiler, `todo!()` for bare arrays in interpreter
- **SUM with ranges** - Works in AST interpreter for some cases only

❌ **Not Implemented**:
- Bytecode VM support for ANY of the new array features
- Star ranges (`*:DimName`)
- Named dimension ranges (`DimA.Boston:DimA.LA`)
- Additional aggregate functions (MEAN, MIN, MAX, etc.)
- Bare array transpose (fails with `todo!()` in interpreter)

The architecture uses a three-phase approach: Parser → Type Checking (Expr2) → Compiler with ArrayView. However, the bytecode VM does not yet support these new features, limiting their practical use.

## Overview

This document outlines the design for comprehensive array support in `simlin-engine`, which is needed to run large existing and important models.
It builds upon the existing foundation while addressing gaps with whats outlined in the XMILE specification, current implementation, and test requirements.


## High-level design

System dynamics models as specified in the [XMILE standard](./xmile-v1.0.html) support a rich syntax around accessing and slicing arrays, much of which (e.g., `array[time]` or `array[some_variable]`) is dynamic simulation-time behavior.
This means we cannot determine exact strides, offsets, or even which elements will be accessed during the equation parsing or compilation. 


### Multi-Phase Array Handling

1. **Parser (Expr0)**: Captures all array syntax including subscripts, transpose, dimension positions (DONE)
2. **Type Checking (Expr2)**: Focuses on: (DONE)
   - Computing **maximum** array bounds (conservative estimates)
   - Basic array-size compatibility checks
   - Calculating temporary storage requirements, e.g. uniquely numbering temporaries
3. **Compiler**: Static optimizations and efficiently preparing for runtime evaluation: (TODO)
   - Pulls to "compile time" what we can, e.g. stride calculations for static subscripts
   - View creation (transpose, slicing)
   - Identifies how much temporary storage to allocate for array temporaries
     - in the interpreter and VM we will use this to allocate one scratch buffer that we bump-allocate array temporaries out of  
4. **AST-walking Interpreter**: Runtime evaluation (TODO)
   - Dynamic subscript evaluation
   - Actual array operations and array-based builtin functions
5. **Bytecode VM**: Runtime evaluation (TODO)
   - implement the same semantics as the AST-walking Interpreter to validate our implementations produce the same behavior
   - Dynamic subscript evaluation
   - Actual array operations and array-based builtin functions
   

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

## Current State

### Working Features
The engine has significant array support already implemented that works across all execution paths:
- Dimension definitions (indexed and named)
- Array variable declarations with dimension associations
- Subscript parsing and AST representation (including wildcards and element selection)
- Flat storage with offset calculation
- Element-wise operations between arrays (ApplyToAll and Arrayed)
- Array-to-array arithmetic operations (addition, multiplication, etc.)
- Scalar-to-array operations
- Basic broadcasting support (e.g., 2D array * 1D array)
- Basic reduction operations (e.g., SUM along specific dimensions)

### Partially Implemented Features
These features have initial implementations but **do not work in the bytecode VM**:
- **Transpose operator** (`'`) - Compiler support for subscripted arrays only, bare arrays fail
- **Dimension position operator** (`@`) - Basic compiler support, not in VM
- **Wildcard subscripts** (`*`) - Works in compiler and AST interpreter, not in VM
- **Range subscripts** (e.g., `a[1:3]`) - Initial compiler support with ArrayView, not in VM
- **SUM with ranges** - Limited AST interpreter support, not in VM

### Not Implemented
- Star ranges (e.g., `*:Dimension`)
- Named dimension ranges (e.g., `DimA.Boston:DimA.LA`)
- Other aggregate functions (MEAN, STDDEV, MIN, MAX, PROD)
- Complete error handling for invalid indices

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

**Required XMILE Features Implementation Status:**
1. **Transpose operator** (`'`): 🚧 **PARTIAL** - Works for subscripted arrays in compiler only, bare arrays hit `todo!()` in interpreter, not in VM
2. **Dimension position operator** (`@`): 🚧 **PARTIAL** - Basic compiler support with dimension reordering, not in VM
3. **Range subscripts**: 🚧 **PARTIAL** - Initial compiler support via ArrayView slicing, limited interpreter support, not in VM

### 2. Temporary Array Storage Management

The simplified design moves array view complexity from Expr2 to the compiler:

**Expr2 Phase**:
- Each array-producing expression gets a `temp_id: Option<u32>` 
- Array bounds are tracked as maximum possible sizes
- No complex ArrayView or ArraySource types needed

**Compiler Phase (Complex)**:
Array views are computed during compilation when we have more context.

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

Partially implemented features - **Compiler and AST interpreter only, not in VM**:
- **Range slicing**: 🚧 `a[1:3, *]` - Initial ArrayView support in compiler
- **Transpose**: ⚠️ `a'` or `a[DimA, DimB]'` - Only subscripted arrays in compiler, bare arrays fail with `todo!()`
- **Dimension position**: 🚧 `a[@2, @1]` for reordering dimensions - Basic compiler support
- **Wildcard subscripts**: 🚧 `a[*]` - Works in compiler and AST interpreter
- **SUM with ranges**: 🚧 `SUM(a[1:3])` - Limited support in AST interpreter only

Still needed:
- **Named dimension ranges**: `a[DimA.Boston:DimA.LA, *]` 
- **Star ranges**: `*:DimA.End` syntax

### 4. Dimension Context Enhancement

Enhance dimension handling to support:
- **Dimension arithmetic**: `Location.Boston + 1` → `Location.Chicago`
- **Dimension ranges**: `DimA.Start:DimA.End`
- **Dynamic dimension queries**: `SIZE(array, dimension_index)`

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

#### 2. Transpose Operator (`'`) 🚧 PARTIAL
- **Parser Support**: ✅ Full support for parsing transpose operator as a postfix unary operator
- **AST Representation**: ✅ Added `Transpose` variant to `UnaryOp` enum
- **Expr2 Handling**: ✅ Properly reverses dimensions during type checking
- **Compiler**: 🚧 Works ONLY for subscripted arrays with ArrayView stride reversal
- **AST Interpreter**: ❌ Has `todo!()` for bare array transpose - not implemented
- **Bytecode VM**: ❌ Not implemented
- **Tests**: 🚧 Some tests pass for subscripted arrays, bare array tests are commented out/ignored

#### 3. Dimension Position Operator (`@`) 🚧 PARTIAL
- **Parser Support**: ✅ Full support for parsing `@n` syntax in subscript expressions
- **AST Representation**: ✅ Added `DimensionPosition(u32, Loc)` variant to all `IndexExpr{0,1,2}` enums
- **Lexer**: ✅ Added `At` token and lexer rule for `@` character
- **Expr2 Handling**: ✅ Properly tracked through type checking phase
- **Compiler**: 🚧 Basic implementation with dimension reordering in ArrayView
- **AST Interpreter**: 🚧 Limited support
- **Bytecode VM**: ❌ Not implemented
- **Tests**: 🚧 Some tests pass in limited scenarios

#### 4. Range Subscripts 🚧 PARTIAL
- **Parser Support**: ✅ Can parse range syntax like `a[1:3]`
- **AST Representation**: ✅ `Range` variant in `IndexExpr{0,1,2}`
- **Expr2 Handling**: ✅ Properly tracked through type checking phase
- **Compiler**: 🚧 Initial implementation with ArrayView slicing using `apply_range_subscript`
- **AST Interpreter**: 🚧 SUM function has limited support for range subscripts
- **Bytecode VM**: ❌ Not implemented
- **Tests**: 🚧 Many tests exist but several are ignored/not passing

#### 5. Wildcard Subscripts 🚧 PARTIAL
- **Parser Support**: ✅ Parsing `*` in subscripts
- **Compiler**: 🚧 Implementation with dimension preservation in ArrayView
- **AST Interpreter**: 🚧 Basic support for wildcard subscripts
- **Bytecode VM**: ❌ Not implemented
- **Tests**: 🚧 Tests pass for AST interpreter, not for VM

### Implementation Details

The implementation follows a clean architecture:
1. **Lexer** recognizes new tokens (`'` as `Apostrophe`, `@` as `At`)
2. **Parser** constructs appropriate AST nodes with proper precedence
3. **AST transformations** properly handle new variants through expr0→expr1→expr2 pipeline
4. **Visitor patterns** updated to handle new AST nodes
5. **Error handling** returns appropriate error codes when features aren't fully implemented

## Implementation Status

### Completed: Expr2 Array Representation
1. **ArrayView complexity removed from Expr2**: ✅
   - Replaced with simple ArrayBounds enum
   - Added temp_id tracking for intermediate results
   - Proper dimension tracking for type checking
2. **AST transformations updated**: ✅
   - Simplified expr0→expr1→expr2 array handling
   - Focus on maximum bounds computation
   - Tests verify correct array bounds propagation

### In Progress: Compiler Array Support
Initial array operations have been implemented in the compiler using the ArrayView abstraction, but are not yet integrated with the bytecode VM.

1. **Split Subscript handling** 🚧 PARTIAL:
   - StaticSubscript created for compile-time resolution with precomputed ArrayView
   - Works in compiler but not translated to VM bytecode
   - Dynamic subscripts partially handled
2. **Implement array operations in compiler** 🚧 PARTIAL:
   - **Transpose**: 🚧 Only for subscripted arrays, bare arrays fail
   - **Ranges**: 🚧 Initial implementation with `apply_range_subscript`
   - **Dimension positions**: 🚧 Basic @n reference resolution
   - **Wildcards**: 🚧 Initial implementation with dimension preservation
   - **Star ranges**: ❌ Not implemented (returns `TodoStarRange`)
3. **VM instruction updates** ❌ NOT DONE:
   - No VM bytecode support for new array features
   - StaticSubscript not translated to bytecode
   - ArrayView operations not available in VM

### Partial: AST Interpreter Support
The AST-walking interpreter has limited support for new array operations:
1. **Array view operations**: 🚧 Basic StaticSubscript support
2. **SUM with ranges**: 🚧 Limited implementation for some array views
3. **Transpose**: ❌ `todo!()` for bare arrays
4. **Test coverage**: 🚧 Many tests ignored or failing

Critical gaps:
- Bare array transpose hits `todo!()` and fails
- Limited coverage of edge cases
- No bytecode VM support at all

### Not Started: VM Bytecode Support
**This is the most critical gap** - none of the new array features have VM bytecode implementations.

### Future: Optimization
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

**Current Status**:

1. **Parser**: ✅ Fully supports all XMILE array syntax (transpose, dimension positions, ranges)
2. **Expr2**: ✅ Successfully simplified with ArrayBounds replacing complex ArrayView types
3. **Compiler**: 🚧 Initial array operations with ArrayView, but not integrated with VM
4. **AST Interpreter**: 🚧 Limited support, bare array transpose fails with `todo!()`
5. **Bytecode VM**: ❌ NO support for any new array features - this is the critical gap

## Recommended Next Steps

Based on the current implementation status, here's the prioritized roadmap:

### CRITICAL Priority: Bytecode VM Support

**The most critical gap is that NONE of the new array features work in the bytecode VM.** This must be addressed before these features can be considered usable.

1. **Implement VM bytecode for StaticSubscript**
   - Translate ArrayView operations to bytecode
   - Support strided iteration in VM
   - Handle view offset and stride calculations

2. **Fix bare array transpose in AST interpreter**
   - Remove `todo!()` and implement proper handling
   - Required for basic testing before VM implementation

### High Priority: Complete Partial Implementations

1. **Finish range subscript support**
   - Complete compiler implementation
   - Full AST interpreter support
   - Then implement in VM

2. **Complete wildcard implementation**
   - Ensure all edge cases work
   - Full test coverage
   - VM bytecode support

3. **Finish dimension position operator**
   - Complete compiler support
   - Full AST interpreter coverage
   - VM implementation

### Secondary Priority: Optimization and Polish

1. **Dynamic Subscript Optimization**
   - Cache computed offsets where possible
   - Optimize sequential access patterns
   - Consider SIMD for array operations

2. **Better Error Messages**
   - More descriptive array bounds errors
   - Clear dimension mismatch messages
   - Helpful suggestions for common mistakes

3. **Performance Testing**
   - Benchmark array operations at scale
   - Profile memory access patterns
   - Optimize hot paths

### Future Enhancements

1. **Star Ranges** (`*:DimName`)
   - Not yet started
   - Requires dimension name resolution

2. **Named Dimension Ranges**
   - Support `DimA.Boston:DimA.LA` syntax
   - Requires dimension element name lookup

3. **Additional Aggregate Functions**
   - MEAN, STDDEV, MIN, MAX, PROD
   - Build on SUM pattern once VM support exists

4. **Advanced Array Functions**
   - Matrix operations (though not required by XMILE)
   - Statistical functions
   - Financial array functions

## Summary

The array support infrastructure exists (ArrayView, parsing, initial compiler support), but **the features are not usable in production** because:
1. The bytecode VM has no support for any of the new array features
2. The AST interpreter has critical gaps (bare array transpose fails with `todo!()`)
3. Many test cases are ignored or commented out
4. The implementations are incomplete and only work in limited scenarios

The ArrayView abstraction provides a solid foundation, but substantial work remains to make these features production-ready. The critical next step is implementing VM bytecode support for these operations.