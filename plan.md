# Array Compiler Support Implementation Plan

## Executive Summary

The VM now has comprehensive bytecode infrastructure for array operations (commit 5d28234), but the compiler doesn't yet emit these opcodes. This plan outlines the implementation of compiler support to emit array-related bytecode, enabling the VM to execute models with array builtins (SUM, MEAN, etc.).

## Current State Analysis

### What's Already Implemented

**VM Infrastructure (bytecode.rs, vm.rs):**
- `RuntimeView` - dynamic view representation with sparse dimension support
- View stack operations: `PushVarView`, `PushTempView`, `PushStaticView`
- View manipulation: `ViewSubscriptConst`, `ViewSubscriptDynamic`, `ViewRange`, `ViewStarRange`, `ViewWildcard`, `ViewTranspose`
- Iteration: `BeginIter`, `LoadIterElement`, `StoreIterElement`, `NextIterOrJump`, `EndIter`
- Reductions: `ArraySum`, `ArrayMax`, `ArrayMin`, `ArrayMean`, `ArrayStddev`, `ArraySize`
- Broadcasting: `BeginBroadcastIter`, `LoadBroadcastElement`, `StoreBroadcastElement`, etc.
- Temp access: `LoadTempConst`, `LoadTempDynamic`

**Compiler Infrastructure (compiler.rs):**
- Expression types: `StaticSubscript`, `TempArray`, `TempArrayElement`, `AssignTemp`
- Temp size extraction: `extract_temp_sizes()` populates `temp_offsets` and `temp_total_size`
- ByteCodeContext has fields for `dimensions`, `subdim_relations`, `names`, `static_views` (currently empty)

### What's Missing

1. **Compiler doesn't populate ByteCodeContext dimension/view tables**
2. **walk_expr() has TODO placeholders for array expressions:**
   - `StaticSubscript`: Only handles scalar case (assumes view.size() == 1)
   - `TempArray`: Returns error "TempArray not yet implemented"
   - `TempArrayElement`: Returns error "TempArrayElement not yet implemented"
   - `AssignTemp`: Returns error "AssignTemp not yet implemented"
   - Array builtins (Sum, Size, Stddev, etc.): Return `TodoArrayBuiltin` error

## Implementation Plan

### Phase 1: Test Infrastructure Setup

**Goal:** Enable TDD by running existing tests and observing failures.

1. Create a new test that specifically exercises array bytecode compilation
2. Remove `#[ignore]` from `simulates_sum()` temporarily to observe failure mode
3. Add debug output to understand what expressions are being generated

### Phase 2: Populate ByteCodeContext Tables

**Goal:** Ensure dimension info flows from compilation to runtime.

The compiler needs to populate:
- `ByteCodeContext.dimensions` - DimensionInfo for each dimension
- `ByteCodeContext.subdim_relations` - SubdimensionRelation for star ranges
- `ByteCodeContext.names` - interned dimension/element names
- `ByteCodeContext.static_views` - pre-computed StaticArrayView instances

**Implementation:**
1. Add `dimensions: Vec<DimensionInfo>` to `Compiler` struct
2. In `Compiler::new()`, iterate model dimensions and populate:
   ```rust
   for dim in &model.dimensions {
       let name_id = self.intern_name(dim.name());
       let dim_info = if dim.is_indexed() {
           DimensionInfo::indexed(name_id, dim.size())
       } else {
           let elem_ids = dim.elements().iter()
               .map(|e| self.intern_name(e))
               .collect();
           DimensionInfo::named(name_id, elem_ids)
       };
       self.dimensions.push(dim_info);
   }
   ```
3. Add helper `intern_name(&mut self, name: &str) -> NameId` that populates `names` vector
4. Add helper `add_static_view(&mut self, view: StaticArrayView) -> ViewId`

### Phase 3: Implement StaticSubscript Bytecode

**Goal:** Handle `Expr::StaticSubscript(off, view, _)` for both scalar and array cases.

**Current behavior:** Only emits `LoadVar { off: off + view.offset }` assuming scalar result.

**Required behavior:**
- If `view.size() == 1`: Current behavior is correct (load single element)
- If `view.size() > 1`: Push view onto view stack for subsequent operations

**Implementation:**
```rust
Expr::StaticSubscript(off, view, _) => {
    if view.dims.iter().product::<usize>() == 1 {
        // Scalar result - compute final offset and load
        let final_off = (*off + view.offset) as VariableOffset;
        self.push(Opcode::LoadVar { off: final_off });
    } else {
        // Array result - push static view for iteration
        let static_view = self.create_static_view(*off, view);
        let view_id = self.add_static_view(static_view);
        self.push(Opcode::PushStaticView { view_id });
    }
    Some(())
}
```

### Phase 4: Implement AssignTemp Bytecode

**Goal:** Handle `Expr::AssignTemp(id, rhs, view)` to populate temp arrays.

This is the most complex case - it needs to:
1. Create iteration context for the temp array's dimensions
2. Emit loop that evaluates RHS for each position
3. Store results into temp storage

**Bytecode pattern:**
```
// For: temp[0] <- source[*] + 1  (where source has dims [5])
PushVarView { base_off: source_off, n_dims: 1, dim_ids: [dim_id, 0, 0, 0] }
BeginIter { write_temp_id: 0, has_write_temp: true }
  LoadIterElement {}       // load source[current_idx]
  LoadConstant { id: 1.0 }
  Op2 { op: Add }
  StoreIterElement {}      // store to temp[0][current_idx]
NextIterOrJump { jump_back: -N }  // where N = distance to LoadIterElement
EndIter {}
PopView {}
```

**Implementation approach:**
1. Emit code to push source view onto view stack
2. Emit `BeginIter { write_temp_id: id, has_write_temp: true }`
3. Recursively walk RHS expression (which may use LoadIterElement)
4. Emit `StoreIterElement {}`
5. Calculate jump offset and emit `NextIterOrJump { jump_back }`
6. Emit `EndIter {}` and `PopView {}`

**Challenge:** The RHS expression may contain `StaticSubscript` or `TempArray` references that need special handling during iteration context.

### Phase 5: Implement TempArray and TempArrayElement Bytecode

**Goal:** Handle loading from temporary arrays.

For `Expr::TempArray(id, view, _)`:
```rust
// Push temp view onto view stack
let static_view = self.create_static_temp_view(*id, view);
let view_id = self.add_static_view(static_view);
self.push(Opcode::PushStaticView { view_id });
```

For `Expr::TempArrayElement(id, view, idx, _)`:
```rust
// Load single element from temp
let offset = view.offset + idx;
self.push(Opcode::LoadTempConst { temp_id: *id as TempId, index: offset as u16 });
```

### Phase 6: Implement Array Builtins

**Goal:** Handle SUM, SIZE, MEAN, STDDEV, MIN, MAX array operations.

**Pattern for SUM(arg):**
```rust
BuiltinFn::Sum(arg) => {
    // Emit code to evaluate arg and push view onto view stack
    self.walk_expr_as_view(arg)?;
    // Emit reduction
    self.push(Opcode::ArraySum {});
    // Clean up view stack
    self.push(Opcode::PopView {});
    return Ok(Some(()));
}
```

Need helper `walk_expr_as_view(&mut self, expr: &Expr)` that ensures expression result is on view stack.

### Phase 7: Handle Expression Context

**Challenge:** Some expressions need different bytecode depending on context:
- In scalar context: need final value on arithmetic stack
- In array context: need view on view stack

**Solution:** Add context parameter to walk_expr or use separate methods:
- `walk_expr_scalar(expr)` - result on arithmetic stack
- `walk_expr_view(expr)` - result on view stack

### Phase 8: Testing and Validation

1. Enable `simulates_sum` test (remove `#[ignore]`)
2. Run `cargo test -p simlin-compat simulates_sum` to verify
3. Enable other array tests progressively
4. Compare interpreter and VM results for all array models

## Implementation Order

1. **Phase 2** - Populate ByteCodeContext (foundation for everything)
2. **Phase 3** - StaticSubscript (enables basic array access)
3. **Phase 5** - TempArray/TempArrayElement (needed before AssignTemp can be useful)
4. **Phase 4** - AssignTemp (enables temp array population)
5. **Phase 6** - Array builtins (final user-visible feature)
6. **Phase 7** - Context handling (if needed for edge cases)
7. **Phase 8** - Testing and validation

## Test Models

These models exercise array functionality:
- `test/arrays1/arrays.stmx` - basic array access (currently passing)
- `test/sdeverywhere/models/sum/sum.xmile` - SUM builtin (currently `#[ignore]`)
- `test/test-models/tests/subscript_1d_arrays/` - 1D array operations
- `test/test-models/tests/subscript_2d_arrays/` - 2D array operations
- `test/test-models/samples/arrays/a2a/` - apply-to-all arrays
- `test/test-models/samples/arrays/non-a2a/` - non-A2A arrays

## Success Criteria

1. All existing array tests pass with both interpreter AND VM
2. `simulates_sum` test passes (currently `#[ignore]`)
3. No regression in existing tests
4. VM and interpreter produce identical results for all array models

## Risks and Mitigations

1. **Complexity of iteration context**: Start with simple cases (single dimension) and progressively add multi-dimension support
2. **Broadcasting complexity**: May need to defer full broadcasting support; start with simpler cases
3. **Performance**: Initial implementation prioritizes correctness over performance; optimize later

## Notes from Design Document

From `doc/array-design.md` (Bobby notes section):
- Temp IDs restart at 0 for each `lower()` call - handled by `extract_temp_sizes` using max
- Pass 0 expands bare array refs to explicit subscripts
- Pass 1 generates AssignTemp for complex expressions
- Pass 2 handles A2A-specific decomposition

The compiler infrastructure already handles the AST transformation; we just need to emit the corresponding bytecode.
