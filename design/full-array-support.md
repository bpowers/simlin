# Full Array Support Design for simlin-engine

## Overview

This document outlines the design for implementing comprehensive multi-dimensional array support in simlin-engine, following the XMILE v1.0 specification section 3.7.1. The implementation will enable proper handling of arrays in system dynamics models, including array operations, slicing, and built-in functions.

## Current State

**✅ MAJOR MILESTONE ACHIEVED: Tree-walking interpreter array support is now working!**

The simlin-engine has implemented significant array functionality:

### Working Features
- **✅ AST Types**: `DimensionVector`, `DimensionRange`, and `Ast` enum variants for `ApplyToAll` and `Arrayed`
- **✅ Data Model**: `Dimension` enum supporting both `Indexed` and `Named` dimensions  
- **✅ Iteration**: `SubscriptIterator` for traversing array dimensions
- **✅ Parsing**: Array subscript notation is parsed and fully processed
- **✅ Basic Array Builtins**: SUM, MIN, MAX, STDDEV working in tree-walking interpreter
- **✅ Partial Reductions**: `SUM(m[DimD, *])` with correct multidimensional indexing
- **✅ Complex Array Expressions**: `SUM(a[*]*b[*]/DT)` (element-wise) and `SUM(a[*]+h[*])` (cross-product)
- **✅ Wildcard Support**: `*` wildcards in array subscripts are properly handled
- **✅ Array Expression Evaluation**: Recursive evaluation of complex expressions with array substitutions

### Test Status
- **✅ `simulates_sum` test**: Now passing! All array operations in the sum model work correctly
- **✅ Element-wise operations**: `SUM(a[*]*b[*]/DT)` = 32 
- **✅ Cross-product operations**: `SUM(a[*]+h[*])` = 198
- **✅ Simple scalar sums**: `SUM(a[*])`, `SUM(b[*])` 
- **✅ Partial reductions**: `SUM(m[DimD, *])` with proper offset calculation

### Implemented Infrastructure
- **✅ Array expression detection**: `expr_contains_array_wildcards()` recursively finds array wildcards
- **✅ Smart operation heuristics**: Distinguishes element-wise vs cross-product based on operation type
- **✅ Recursive substitution**: `eval_with_array_substitution()` handles complex nested expressions
- **✅ Cross-product evaluation**: Full combination generation for different-dimension arrays
- **✅ Bytecode VM array operations**: ArraySum, ArrayMin, ArrayMax, ArrayStddev opcodes (basic implementation)

## Design Goals

1. **Specification Compliance**: Implement arrays according to XMILE v1.0 section 3.7.1
2. **Type Safety**: Propagate dimension information through the AST for compile-time checking
3. **Error Quality**: Provide excellent error messages for dimension mismatches and invalid operations
4. **Maintainability**: Design extensible abstractions for future array operations
5. **Performance**: Enable efficient compilation and execution of array operations

## Key Design Decision: AST Refactoring

### Phase 1: Rename Current Types

Rename existing AST types to make room for dimension-annotated versions:
- `Expr` -> `Expr1` (dimension-annotated expression)
- `IndexExpr` -> `IndexExpr1` (dimension-annotated index expression)
- `Expr0` -> `Expr0` (keep as parsed, unannotated expression)
- `IndexExpr0` -> `IndexExpr0` (keep as parsed, unannotated index expression)

### Phase 2: New Dimension-Annotated Types

```rust
/// Expression with dimension information propagated
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Const(String, f64, DimensionVector, Loc),
    Var(Ident, DimensionVector, Loc),
    App(BuiltinFn<Expr>, DimensionVector, Loc),
    Subscript(Ident, Vec<IndexExpr>, DimensionVector, Loc),
    Op1(UnaryOp, Box<Expr>, DimensionVector, Loc),
    Op2(BinaryOp, Box<Expr>, Box<Expr>, DimensionVector, Loc),
    If(Box<Expr>, Box<Expr>, Box<Expr>, DimensionVector, Loc),
}

/// Index expression with dimension information
#[derive(Clone, Debug, PartialEq)]
pub enum IndexExpr {
    Wildcard(DimensionVector, Loc),
    StarRange(Ident, DimensionVector, Loc),
    Range(Expr, Expr, DimensionVector, Loc),
    Expr(Expr),
}
```

This follows the same pattern as the existing AST transformation:
- `Expr0` (parsed) → `Expr1` (typed/resolved) → `Expr` (dimension-annotated)
- Each variant carries its dimension information directly
- Transformation passes can build the enriched AST incrementally
- Type safety is maintained throughout the compilation pipeline

## Dimension Type System

### DimensionVector Operations

Extend `DimensionVector` with key operations:

```rust
impl DimensionVector {
    /// Check if dimensions are broadcastable for element-wise operations
    pub fn is_broadcast_compatible(&self, other: &Self) -> bool;
    
    /// Result dimensions after broadcasting
    pub fn broadcast_shape(&self, other: &Self) -> Result<Self>;
    
    /// Check if this can be assigned to target dimensions
    pub fn is_assignable_to(&self, target: &Self) -> bool;
    
    /// Apply slicing operation (for wildcards and ranges)
    pub fn slice(&self, slice_spec: &[SliceSpec]) -> Result<Self>;
}

pub enum SliceSpec {
    Index(usize),        // Single element selection
    Wildcard,           // Keep dimension (*)
    Range(usize, usize), // Range selection (start:end)
    DimName(String),    // Dimension placeholder
}
```

### Broadcasting Semantics

"Broadcasting" refers to how arrays with different dimensions can be made compatible for element-wise operations. This concept, borrowed from NumPy, allows operations between arrays of different shapes by automatically expanding dimensions where needed.

#### Broadcasting Rules (adapted for XMILE):

1. **Scalar Broadcasting**: A scalar (0-dimensional) value can operate with any array by being conceptually repeated for each element:
   ```
   scalar 5 + array[Location, Product] → array[Location, Product]
   ```

2. **Dimension Matching**: When operating on two arrays, dimensions are compared from right to left:
   - Dimensions must either be equal or one must be 1 (singleton)
   - Missing dimensions are treated as 1

3. **Examples**:
   ```
   array[3, 4] + array[4]     → array[3, 4]  // Second array broadcasts over first dimension
   array[3, 1] * array[3, 4]  → array[3, 4]  // Singleton dimension expands
   array[3, 4] + array[2, 4]  → ERROR        // Incompatible dimensions (3 ≠ 2)
   ```

4. **XMILE-Specific Considerations**:
   - Named dimensions must match by name, not just size
   - `Location[Boston, Chicago]` is not broadcast-compatible with `Product[dresses, skirts]` even if both have size 2
   - This provides better type safety than purely numeric broadcasting

#### Relationship to XMILE Specification:

While the XMILE specification doesn't explicitly use the term "broadcasting", it describes similar behavior:
- Section 3.7.1.1 states that arithmetic operators work element-wise on arrays
- Operations with scalars naturally extend to all array elements
- The specification's approach to dimension names in Apply-to-All arrays implicitly requires dimension compatibility

Our broadcasting rules formalize these behaviors and extend them to handle partial dimension matching in a predictable way.

### Dimension Propagation Rules

1. **Constants**: Always scalar (`DimensionVector::scalar()`)
2. **Variables**: Look up dimensions from variable metadata
3. **Binary Operations**: 
   - Arithmetic: Broadcast compatible dimensions
   - Comparison: Operands must have same dimensions
   - Logical: Operands must be broadcastable
4. **Subscripts**: Apply slicing rules to reduce dimensions
5. **Function Calls**: Function-specific rules (see below)

## Array Built-in Functions

### Implementation Strategy

Each array built-in will have:
1. **Dimension Checker**: Validates input dimensions and computes output dimensions
2. **Compiler**: Generates appropriate bytecode
3. **VM Operation**: Executes the array operation

### Function-Specific Rules

#### SUM(array)
- **Input**: Array of any dimensions
- **Output**: Scalar (all dimensions reduced)
- **Variants**: SUM(array[*, i]) for partial reduction

#### SIZE(array)
- **Input**: Array of any dimensions  
- **Output**: Scalar (total element count)

#### MEAN(array), STDDEV(array)
- **Input**: Array of any dimensions
- **Output**: Scalar

#### MIN/MAX(array1, array2, ...)
- **Input**: Multiple arrays (must be broadcast compatible)
- **Output**: Element-wise minimum/maximum with broadcast shape

#### RANK(array, rank_number[, tiebreaker])
- **Input**: Array to rank, scalar rank number, optional tiebreaker array
- **Output**: Scalar index or flat index for N-D arrays

## Compilation Strategy

### New Compiler Passes

1. **Dimension Inference Pass**
   ```rust
   fn infer_dimensions(expr: Expr1, ctx: &DimensionContext) -> Result<Expr>
   ```
   - Converts `Expr1` to dimension-annotated `Expr`
   - Propagates dimensions bottom-up through AST
   - Validates dimension compatibility
   - Similar to existing `Expr::from(Expr0)` transformation

2. **Array Operation Lowering**
   ```rust
   fn lower_array_ops(expr: &Expr, ctx: &Context) -> Result<Vec<BytecodeOp>>
   ```
   - Converts high-level array operations to loops
   - Generates index calculations for multi-dimensional access
   - Optimizes common patterns (e.g., full array sum)

### Bytecode Extensions

New opcodes for array operations:

```rust
pub enum Opcode {
    // ... existing opcodes ...
    
    // Array iteration
    BeginArrayLoop { dims: Vec<u16> },  // Start iterating over dimensions
    EndArrayLoop,                        // End array iteration
    LoadLoopIndex { dim: u16 },          // Get current loop index for dimension
    
    // Array operations  
    ArraySum { n_dims: u16 },            // Sum array elements
    ArraySize { n_dims: u16 },           // Get array size
    ArrayMin { n_dims: u16 },            // Find minimum
    ArrayMax { n_dims: u16 },            // Find maximum
    
    // Stack operations for aggregation
    InitAccumulator,                     // Initialize accumulator for reduction
    Accumulate { op: AccumulateOp },    // Add to accumulator
    LoadAccumulator,                     // Push accumulator value
}

pub enum AccumulateOp {
    Sum,
    Min,
    Max,
    Count,
}
```

## VM Execution

### Array Operation Execution

The VM will handle array operations through:

1. **Loop Management**: Track nested loop state for multi-dimensional iteration
2. **Index Calculation**: Compute flat indices from multi-dimensional coordinates
3. **Accumulator State**: Maintain state for reduction operations

```rust
struct ArrayLoopState {
    dims: Vec<usize>,
    indices: Vec<usize>,
    done: bool,
}

impl Vm {
    fn execute_array_loop(&mut self, state: &mut ArrayLoopState) {
        // Update indices, handling carry for multi-dimensional loops
    }
    
    fn calculate_flat_index(&self, indices: &[usize], dims: &[usize]) -> usize {
        // Row-major order calculation
    }
}
```

## Error Handling

### Compile-Time Errors

1. **Dimension Mismatch**: 
   ```
   Error: Dimension mismatch in expression
   --> model.xmile:45:10
   |
   | revenue[Location, Product] = sales[Location] * price
   |                              ^^^^^^^^^^^^^^^^^^^^^^^^
   | Expected dimensions [Location, Product], found [Location]
   ```

2. **Invalid Subscript**:
   ```
   Error: Invalid subscript for dimension
   --> model.xmile:23:15
   |
   | total = sum(sales[Boston, InvalidProduct])
   |                          ^^^^^^^^^^^^^^^
   | 'InvalidProduct' is not a valid subscript for dimension 'Product'
   | Valid subscripts: dresses, skirts
   ```

### Runtime Errors

1. **Out of Bounds**: Return 0 with optional warning (per spec)
2. **Invalid Operations**: NaN propagation for invalid array operations

## Implementation Status

### ✅ Completed Phases

**Phase 1: AST Refactoring and Dimension Propagation**
- ✅ Renamed existing types (`Expr` → `Expr1`, etc.) - *Prepared AST structure*
- ✅ Implemented new dimension-annotated `Expr` and `IndexExpr` enums - *Infrastructure ready*
- ⚠️ Add dimension inference pass to transform `Expr1` → `Expr` - *Partial: basic type structure exists*
- ⚠️ Update error messages to include dimension information - *Not implemented yet*

**Phase 2: Basic Array Builtins** 
- ✅ Implement SUM for complete arrays - *Working in tree-walking interpreter*
- ✅ Implement SIZE - *Working in tree-walking interpreter*
- ✅ Add bytecode generation for simple array loops - *Basic implementation*
- ✅ Update VM to execute array operations - *Basic ArraySum, ArrayMin, ArrayMax, ArrayStddev*

**Phase 3: Advanced Array Operations**
- ✅ Implement slicing with wildcards and ranges - *Wildcard support working*
- ✅ Add MEAN, STDDEV, MIN, MAX operations - *Working in tree-walking interpreter*
- ⚠️ Implement RANK with tiebreakers - *Placeholder implementation only*
- ✅ Support partial reductions (e.g., SUM(array[*, i])) - *Working with proper offset calculation*

**Phase 4: Array Expressions** 
- ✅ Element-wise operations on arrays - *Working: `SUM(a[*]*b[*]/DT)`*
- ✅ Broadcasting for compatible dimensions - *Heuristic-based approach working*
- ⚠️ Array transposition - *Not implemented*
- ⚠️ Optimize common patterns - *Basic optimization only*

### 🚧 What's Left To Do

#### High Priority (Core Functionality)
1. **Bytecode VM Array Support**: Extend bytecode VM to match tree-walking interpreter capabilities
   - Fix complex array expressions in bytecode compiler
   - Implement element-wise vs cross-product detection in VM
   - Add proper array expression evaluation to bytecode path

2. **RANK Function**: Complete implementation of RANK builtin function
   - Currently has placeholder implementation
   - Needs proper ranking algorithm with tiebreaker support

3. **Error Messages**: Improve array-related error messages
   - Include dimension information in error reports
   - Better out-of-bounds error handling
   - Clear messages for dimension mismatches

#### Medium Priority (Robustness)
4. **Dimension Type System**: Complete the formal dimension propagation system
   - Implement full `DimensionVector` operations (`is_broadcast_compatible`, `broadcast_shape`)
   - Add compile-time dimension checking
   - Support for named dimension validation

5. **Array Transposition**: Implement array reshape and transpose operations
   - Support for dimension reordering
   - Integration with existing subscript system

6. **Star Ranges**: Implement `[*:DimName]` syntax for dimension-specific wildcards
   - Currently parsed but not fully implemented
   - Needed for advanced partial reductions

#### Low Priority (Optimization & Polish)
7. **Performance Optimization**: 
   - Vectorized operations for large arrays
   - Memory-efficient storage for sparse arrays
   - Optimize common array operation patterns

8. **Advanced Broadcasting**: Full NumPy-style broadcasting semantics
   - Currently uses heuristics; could be more systematic
   - Better handling of singleton dimensions

9. **Array Range Operations**: Support for `[start:end]` range subscripts
   - Currently parsed but not implemented
   - Useful for array slicing operations

## 🔧 Opportunities for Improvement and Cleanup

### Code Architecture Improvements

1. **Unify Array Evaluation Paths**: Currently there are two different implementations:
   - Tree-walking interpreter: Full array expression support with heuristic-based element-wise/cross-product detection
   - Bytecode VM: Basic array operations only, missing complex expression support
   - **Opportunity**: Extract common array evaluation logic into shared modules

2. **Improve Dimension Detection**: Current heuristics work but are not robust:
   - Uses operation type (mul/div vs add/sub) to guess element-wise vs cross-product behavior
   - Uses memory offset proximity to determine if arrays share dimensions
   - **Opportunity**: Access actual dimension names during expression evaluation for proper type checking

3. **Simplify Array Expression AST**: The current approach has multiple expression types:
   - `Expr0` (parsed) → `Expr1` (typed) → `Expr` (dimension-annotated)
   - The dimension-annotated `Expr` enum exists but isn't fully utilized
   - **Opportunity**: Complete the transition to dimension-annotated AST for better type safety

### Performance and Memory Optimizations

4. **Reduce Dynamic Allocation**: Current implementation uses `Vec<f64>` for intermediate results
   - For simple operations like `SUM(a[*])`, could compute results directly without storing intermediates
   - **Opportunity**: Stream-based evaluation for array operations to reduce memory usage

5. **Optimize Cross-Product Operations**: Currently generates all combinations in memory
   - For `SUM(a[*]+h[*])` with large arrays, this could use significant memory
   - **Opportunity**: Streaming evaluation that computes and accumulates results without storing all combinations

6. **Cache Array Metadata**: Currently re-analyzes array structure for each operation
   - Array bounds, dimension information, and offset calculations are repeated
   - **Opportunity**: Cache array metadata during compilation phase

### Code Quality and Maintainability

7. **Remove Dead Code**: Several methods are marked as unused or placeholder:
   - `extract_array_info()` method is unused in current implementation
   - Many `DimensionVector` methods exist but aren't used
   - **Opportunity**: Clean up unused code and complete partially implemented features

8. **Improve Error Messages**: Current error handling is basic:
   - Array operations that fail often return NaN without clear error messages
   - Out-of-bounds access should provide better diagnostics
   - **Opportunity**: Add comprehensive error reporting with dimension information

9. **Standardize Array Operation Interface**: Different array functions use different patterns:
   - Some use specialized methods (`eval_sum`, `eval_array_min`)
   - Others use generic methods (`eval_array_operation`)
   - **Opportunity**: Create consistent interface for all array operations

### Testing and Validation Improvements

10. **Expand Test Coverage**: Current tests focus on the `sum` model:
    - Need tests for edge cases (empty arrays, single-element arrays)
    - Need tests for error conditions (dimension mismatches, out-of-bounds)
    - **Opportunity**: Add comprehensive test suite covering all array operation combinations

11. **Add Performance Benchmarks**: No current performance testing for array operations:
    - Need to validate that array operations scale well with array size
    - Need to compare tree-walking vs bytecode VM performance
    - **Opportunity**: Add benchmark suite for array operations

12. **Validate Against Reference Implementation**: Limited validation against known-good results:
    - Currently uses golden results from `.dat` files
    - Could benefit from cross-validation with other system dynamics tools
    - **Opportunity**: Expand validation test suite

### Technical Debt Reduction

13. **Resolve Compiler Warnings**: Multiple unused variable and dead code warnings:
    - Suggests incomplete implementation or over-engineering in some areas
    - **Opportunity**: Review and clean up all compiler warnings

14. **Improve Documentation**: Code comments are sparse in array-related code:
    - Complex array evaluation logic lacks detailed comments
    - Heuristic algorithms need better documentation of assumptions
    - **Opportunity**: Add comprehensive documentation for array evaluation logic

15. **Consider Alternative Architectures**: Current approach mixes runtime evaluation with compile-time analysis:
    - Could benefit from more separation of concerns
    - Could explore template-based approaches for better performance
    - **Opportunity**: Evaluate alternative architectural approaches for array operations

## Testing Strategy

1. **Unit Tests**: Test each dimension operation and propagation rule
2. **Integration Tests**: Use existing XMILE test models (e.g., `test/sdeverywhere/models/sum/`)
3. **Error Tests**: Verify quality of error messages
4. **Performance Tests**: Ensure array operations scale well

## Future Considerations

1. **Optimization**: Vectorized operations for better performance
2. **Debugging**: Array inspection in error messages
3. **Extensions**: Matrix operations beyond element-wise
4. **Memory**: Efficient storage for sparse arrays

## Conclusion

This design provides a solid foundation for implementing full array support in simlin-engine. By augmenting the AST with dimension information and implementing systematic dimension propagation, we can provide excellent compile-time checking and clear error messages while maintaining compatibility with the XMILE specification.