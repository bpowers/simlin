# Array Support in Simlin-Engine

## Executive Summary

The simlin-engine implements comprehensive array support following the XMILE v1.0 specification. The architecture uses a multi-phase approach with strong support for static optimization of array operations through the ArrayView abstraction. The parser, type checker, compiler, and AST interpreter provide full support for array operations including complex expressions with array builtins. The bytecode VM lacks support for the advanced array features, limiting their use in production.

## General subscript notes:
* Array indexing is 1-based
* Numerical and integer ranges like `Cities.Boston:Cities.Seattle` and `1:5` are inclusive of both the start and end number, so `1:3` selects the elements 1,2,3
* Sub dimensions and '*:dim', etc

## Current Implementation Status

### Fully Implemented Features

#### Core Array Infrastructure
- **Dimension definitions**: Both indexed (1-based) and named dimensions
- **Array variable declarations**: Variables with associated dimensions
- **Flat storage model**: Row-major layout with offset calculation
- **Element-wise operations**: Full support for array arithmetic following XMILE semantics
- **Broadcasting**: Automatic scalar-to-array and compatible array-to-array operations
- **Basic subscripting**: Element access via indices or named elements
- **XMILE-compliant ranges**: Inclusive ranges (e.g., `[1:5]` includes elements 1,2,3,4,5)

#### Parser (Expr0)
- **Transpose operator** (`'`): Postfix unary operator with proper precedence
- **Dimension position** (`@n`): References to nth dimension in subscripts
- **Range subscripts**: Both numeric (`[1:3]`) and named (`[Boston:LA]`) ranges
- **Wildcard subscripts** (`[*]`): Preserves dimension in result
- **Star ranges** (`[*:End]`): Supported in AST representation

#### Type Checker (Expr2)
- **Simplified array bounds**: Clean separation between named variables and temporaries
- **Temp ID allocation**: Context-based system for tracking intermediate array results
- **Dimension tracking**: Maximum bounds computation for type checking
- **Array unification**: Proper handling of broadcasting and dimension compatibility

#### Compiler
- **ArrayView abstraction**: Efficient strided array representation without data copying
- **Static subscript optimization**: Pre-computed views for compile-time known subscripts
- **Named element resolution**: Automatic case-insensitive lookup of dimension elements
- **Range slicing**: Creates efficient views with adjusted offsets and strides (inclusive)
- **Transpose support**: Stride reversal for subscripted arrays
- **Dimension position handling**: Support for `@n` references
- **Expression rewriting**: Complex array builtin arguments automatically decomposed into temporaries
- **Temporary array management**: Full support for AssignTemp and TempArray expressions

#### AST Interpreter
- **Complete array builtin support**: SUM, MEAN, STDDEV, MIN, MAX, SIZE with complex expressions
- **StaticSubscript evaluation**: Direct offset access for optimized subscripts
- **TempArray evaluation**: Support for temporary array storage and access
- **Complex expression handling**: Nested operations like `MAX(source[3:5] * 2 - 1)`
- **Efficient array iteration**: Helper methods for clean, reusable array operations
- **Multi-dimensional support**: Proper stride-aware iteration for any dimensionality

### Partially Implemented Features

#### Bytecode VM
- **Basic array operations**: Element-wise operations work through existing infrastructure
- **Simple subscripting**: Dynamic subscript evaluation with bounds checking
- **No advanced features**: StaticSubscript, TempArray, and ArrayView operations not implemented

### Not Implemented
- **VM support for ArrayView operations**: Critical gap preventing use of optimized array features in production
- **Bare array transpose**: Results in panic in interpreter for non-subscripted arrays
- **Star ranges**: Parser support exists but no runtime evaluation
- **RANK builtin**: Full implementation pending

## Architecture

### Core Data Structures

```rust
// Efficient array view representation
pub struct ArrayView {
    pub dims: Vec<usize>,      // Dimension sizes after slicing/viewing
    pub strides: Vec<isize>,   // Elements to skip for each dimension
    pub offset: usize,         // Starting offset in underlying data
}

// Compiler expression types with array support
pub enum Expr {
    Const(f64, Loc),
    Var(usize, Loc),                              // Simple variable access
    Subscript(usize, Vec<Expr>, Vec<usize>, Loc), // Dynamic subscript
    StaticSubscript(usize, ArrayView, Loc),       // Optimized static subscript
    TempArray(u32, ArrayView, Loc),               // Temporary array reference
    // ... other variants
}

// Type checker array bounds
pub enum ArrayBounds {
    Named { name: String, dims: Vec<usize> },     // Named variable array
    Temp { id: u32, dims: Vec<usize> },          // Temporary array
}
```

### Processing Pipeline

1. **Parser**: Captures all XMILE array syntax into Expr0 AST
2. **Type Checker**: Computes maximum bounds, allocates temp IDs, validates dimensions
3. **Compiler**: 
   - Optimizes static subscripts into ArrayView instances
   - Rewrites complex array builtin arguments into temporary arrays
   - Generates optimized Expr tree with AssignTemp/TempArray nodes
4. **Interpreter**: Evaluates expressions with full array operation support
5. **VM**: Limited evaluation (basic operations only)

### Static Subscript Optimization

The compiler identifies subscripts that can be resolved at compile time and creates optimized `StaticSubscript` nodes:

```rust
// Example: sales[Boston:LA, *]
// Compiler resolves "Boston" -> index 0, "LA" -> index 2
// Creates ArrayView with:
//   - dims: [3, original_dim2_size]
//   - strides: [original_stride1, original_stride2]
//   - offset: 0
```

### ArrayView Operations

ArrayView enables efficient array operations without copying data:

- **Slicing**: Adjusts offset and dimensions while preserving strides
- **Transpose**: Reverses stride order to change iteration pattern
- **Dimension reordering**: Reorders strides to match new dimension order
- **Range selection**: Combines offset adjustment with dimension reduction (inclusive ranges)

### Interpreter Array Helpers

The interpreter provides clean abstractions for array operations:

```rust
// Iterate over all elements in an array (handles both StaticSubscript and TempArray)
fn iter_array_elements<F>(&mut self, expr: &Expr, f: F) where F: FnMut(f64)

// Get the total size of an array
fn get_array_size(&self, expr: &Expr) -> usize

// Apply a reduction operation over array elements
fn reduce_array<F>(&mut self, expr: &Expr, init: f64, reducer: F) -> f64

// Calculate mean of an array
fn array_mean(&mut self, expr: &Expr) -> f64

// Calculate standard deviation of an array
fn array_stddev(&mut self, expr: &Expr) -> f64
```

These helpers eliminate code duplication and ensure consistent multi-dimensional array handling across all builtins.

## Proposed Next Steps

### Priority 1: Complete VM Support

Implement bytecode operations for advanced array features:

1. **Add ArrayView-aware opcodes**:
   - `LoadStaticSubscript`: Load using precomputed view
   - `LoadTempArray`: Access temporary array storage
   - `CreateView`: Dynamic view creation for runtime subscripts

2. **Extend VM state**:
   - Temporary array storage pool
   - View iteration support
   - Stride-aware memory access

3. **Implement view operations**:
   - Efficient iteration over strided arrays
   - Proper bounds checking for views
   - Support for non-contiguous access patterns

### Priority 2: Complete Remaining Features

1. **Fix bare array transpose**: Implement proper index mapping for non-subscripted arrays
2. **Star range evaluation**: Implement runtime dimension queries
3. **RANK builtin**: Complete implementation for dimension queries

### Priority 3: Optimization

1. **View composition**: Combine multiple view operations into single view
2. **Contiguous detection**: Fast path for contiguous array access
3. **SIMD operations**: Vectorize array operations where possible
4. **Cache-friendly iteration**: Optimize access patterns for cache locality

## Design Principles

### Efficiency Through Views
ArrayView provides zero-copy array operations by adjusting how we iterate over existing data rather than creating new arrays. This is crucial for performance with large arrays.

### Static Optimization
The compiler aggressively optimizes array operations that can be resolved at compile time, reducing runtime overhead for common patterns.

### XMILE Compliance
All array operations follow XMILE v1.0 semantics:
- Element-wise arithmetic (not linear algebra)
- 1-based indexing in source, 0-based internally
- Configurable invalid index behavior (0 or NaN)

### Separation of Concerns
- Parser: Syntax only
- Type checker: Bounds and compatibility only
- Compiler: Optimization and view creation
- Runtime: Execution with minimal overhead

## Testing Strategy

### Current Test Coverage
- Parser: Comprehensive coverage of all array syntax forms
- Type checker: Array bounds propagation and unification
- Compiler: Static subscript optimization, view creation, and expression rewriting
- Interpreter: Full coverage of array builtins with complex expressions
- Integration: XMILE model compatibility tests including inclusive ranges

### Needed Tests
- VM array operations with various view configurations
- Stress tests for large arrays
- Performance benchmarks for array operations
- Edge cases in star ranges
- Bare array transpose operations

## Future Enhancements

### Near Term
- Complete VM implementation for production use
- Implement star ranges fully
- Fix bare array transpose


Bobby notes:

* we need to refactor the array support to do more work in individual passes
* proposed:
  * rejigger how we represent Dimensions: for named dimensions they need to be in a hashmap for quick lookup, _and_ we need to easily determine whether one dim is a subdimension or superdimension of another.  it probably makes sense to have some interior mutability so that after we make that determination the first time it is O(1) to answer in the future
  * ArrayView also needs an update so that we can propoerly handle '*:dim' (or 'dim.*') subdimension splats which might result in _sparse_ iteration.  I think the insight to leverage here is that dimensions are typically pretty small in length: p50 is probably < 10 items, and max is likely several hundred (having 100 items for a "population" array aging chain is normal).  We should be able to use a bitmap for sparse iteration: 2 64-bit numbers covers 128 items
  * prep: rename compiler.rs's Context.lower to Context.lower_late (or maybe lower_old)
  * pass 0 (call this Context.lower): expand bare array var references to explicit subscripts, and normalize e.g. '*' to 'dim.*'.  I think this is useful for subsequent phases to always treat encountered Vars as scalars.  We _ALSO_ need to either generate more detailed ArrayViews or use the array info from Expr2 on Expr
    - `revenue – sales` becomes `revenue[Location, Product] – sales[Location, Product]`  e.g. Op2(Subscript(), Subscript())
  * pass 1: generate AssignTempArray Exprs while recursively re-writing as much of the original expression as we can that doesn't need a2a-element specific behavior.  For example SUM(revenue[*, *]) can be rewritten here, but SUM(revenue[*, Product]) _can't_ because Product is going to be replaced by a specific integer depending on which a2a element it is evaluated for
    * as we're doing this, if there is a dimension size mismatch we need to report an error
  * pass 2: generate AssignTempArray Exprs in the context of a specific a2a element (active_element and active_dim are non-None)
    * also need to report an error if there is a dim size mismatch here
* pass 0 and pass 1 need to happen for all equations.  pass 2 has to happen for A2A equations only.  and we need a check for non-A2A equations that after pass 0 and pass 1 we're not left with.  pass 1 and pass 2 I think are the _same_ pass, but behaving differently if there is active_dim




concrete examples:

if we have:
b[dimA] = c      (no decompose, use c array view directly)
b[dimA] = c[1:3] (no decompose, use array view directly
b[dimA] = sum(c) (no decompose, use array view directly
b[dimA] = sum(c[1:3] + 1)
 -> tmp(0, [indexed(3)]) := c[1:3] + 1
    b[dimA] = sum(tmp(0, [indexed(3)]))

b[dimA] = if (rand() % 2 == 0) then sum(c[1:3] + 1) * dimA else 2*c
 -> tmp(0, [indexed(3)]) := c[1:3] + 1
    tmp(1, [dimA]) := sum(tmp(0, [indexed(3)])) * dimA
    tmp(2, [dimA]) := 2*c
    tmp(3, [dimA]) := if (rand() % 2 == 0) then tmp(1, [dimA]) else tmp(2, [dimB])
    b[dimA] = tmp(3, [dimA])
