# Array Support in Simlin-Engine

## Executive Summary

The simlin-engine implements comprehensive array support following the XMILE v1.0 specification. The architecture uses a multi-phase approach with strong support for static optimization of array operations through the ArrayView abstraction. While the parser, type checker, and compiler have mature implementations, the bytecode VM lacks support for the advanced array features, limiting their practical use.

## Current Implementation Status

### Fully Implemented Features

#### Core Array Infrastructure
- **Dimension definitions**: Both indexed (1-based) and named dimensions
- **Array variable declarations**: Variables with associated dimensions
- **Flat storage model**: Row-major layout with offset calculation
- **Element-wise operations**: Full support for array arithmetic following XMILE semantics
- **Broadcasting**: Automatic scalar-to-array and compatible array-to-array operations
- **Basic subscripting**: Element access via indices or named elements

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
- **Range slicing**: Creates efficient views with adjusted offsets and strides
- **Transpose support**: Stride reversal for subscripted arrays
- **Dimension position handling**: Basic support for `@n` references

### Partially Implemented Features

#### AST Interpreter
- **StaticSubscript evaluation**: Direct offset access for optimized subscripts
- **Limited transpose**: Works for subscripted arrays, fails for bare arrays
- **SUM with ranges**: Basic support for some array view patterns
- **Array context tracking**: Infrastructure for A2A (apply-to-all) operations

#### Bytecode VM
- **Basic array operations**: Element-wise operations work through existing infrastructure
- **Simple subscripting**: Dynamic subscript evaluation with bounds checking
- **No advanced features**: StaticSubscript, TempArray, and ArrayView operations not implemented

### Not Implemented
- **VM support for ArrayView operations**: Critical gap preventing use of optimized array features
- **Bare array transpose**: Results in `todo!()` panic in interpreter
- **Star ranges**: Parser support exists but no evaluation implementation
- **Additional aggregate functions**: MEAN, STDDEV, MIN, MAX, PROD
- **Temporary array management**: TempArray allocation and access

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
3. **Compiler**: Optimizes static subscripts, creates ArrayView instances, generates Expr tree
4. **Interpreter/VM**: Evaluates expressions with array operations

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
- **Range selection**: Combines offset adjustment with dimension reduction

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

### Priority 2: Complete Interpreter Support

1. **Fix bare array transpose**: Implement proper index mapping without ArrayView
2. **Complete aggregate functions**: Extend SUM pattern to other reductions
3. **Star range evaluation**: Implement runtime dimension queries

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
- Compiler: Static subscript optimization and view creation
- Integration: XMILE model compatibility tests

### Needed Tests
- VM array operations with various view configurations
- Stress tests for large arrays
- Performance benchmarks for array operations
- Edge cases in dimension position and star ranges

## Future Enhancements

### Near Term
- Complete VM implementation for production use
- Add remaining aggregate functions
- Implement star ranges fully

### Long Term
- GPU acceleration for large array operations
- Sparse array support
- Advanced indexing (boolean masks, indirect)
- Parallel array operations
- Memory pooling for temporary arrays