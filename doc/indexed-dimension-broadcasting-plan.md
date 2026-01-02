# Plan: Different-Named Indexed Dimension Broadcasting

## Executive Summary

This document outlines the implementation plan for supporting different-named indexed dimension broadcasting in the Simlin engine compiler. This feature allows operations like `sales[Products] + costs[Regions]` where Products and Regions are different indexed dimensions with the same size.

**Key Principles:**
1. **N-dimensional generalization**: All algorithms work for any number of dimensions, no special-casing for 1D/2D
2. **Test-Driven Development**: Write/enable tests first, then implement to make them pass
3. **No compatibility shims**: Replace old code paths entirely, delete superseded code

## Problem Statement

Currently, the compiler only allows dimension matching by name. The failing tests demonstrate two patterns that should work:

### Pattern 1: Same-Rank Positional Matching
```
indexed_dimension("Products", 3)
indexed_dimension("Regions", 3)
array_aux("sales[Products]", "Products")        // [1, 2, 3]
array_aux("costs[Regions]", "Regions * 10")     // [10, 20, 30]
array_aux("combined[Products]", "sales + costs") // Should be [11, 22, 33]
```

Here, `costs[Regions]` needs to match `Products` via size-based matching because both are indexed dimensions of size 3.

### Pattern 2: Cross-Dimension Broadcasting (N-dimensional)
```
// 2D example
indexed_dimension("First", 2)
indexed_dimension("Second", 2)
array_aux("a[First]", "First * 10")   // [10, 20]
array_aux("b[Second]", "Second")      // [1, 2]
array_aux("combined[First, Second]", "a + b")
// combined[i,j] = a[i] + b[j] → [11, 12, 21, 22]

// 3D example
indexed_dimension("X", 2)
indexed_dimension("Y", 3)
indexed_dimension("Z", 4)
array_aux("plane[X, Y]", "X * 10 + Y")     // 2x3 array
array_aux("line[Z]", "Z")                   // length-4 array
array_aux("cube[X, Y, Z]", "plane + line")  // 2x3x4 array
// cube[i,j,k] = plane[i,j] + line[k]
```

The algorithm must work for ANY number of dimensions, not just 1D/2D.

## Current Architecture Analysis

### Problem: Scattered Dimension Matching Logic

There are **four separate code paths** where dimension matching occurs, each with slightly different logic:

1. **`find_dimension_reordering`** (compiler.rs:4559) - Name-only matching
2. **`get_implicit_subscripts`** (compiler.rs:716) - Has size-based fallback
3. **Subscript handling in `lower_from_expr3`** (compiler.rs:1649-1800) - Partial size matching
4. **Bare Var handling** (compiler.rs:1480-1560) - Uses name-only `find_dimension_reordering`

**This scattered logic is the root cause of the bug.** The fix requires consolidating into ONE unified approach.

### Key Semantic Rules

These rules must be preserved:
1. **Named dimensions**: Match by name ONLY (semantic meaning matters)
2. **Indexed dimensions**: Match by name first, then by size as fallback
3. **Broadcasting**: When input has fewer dimensions than output, unmatched output dimensions use stride 0

## Implementation Plan

### Phase 1: Test Infrastructure (TDD)

**Goal:** Define expected behavior through tests BEFORE implementation.

#### 1.1 Enable existing ignored tests
Remove `#[ignore]` from the tests in `indexed_dimension_broadcasting_tests`:
- `different_indexed_dims_same_size_broadcast`
- `different_indexed_dims_with_wildcard`
- `indexed_dims_2d_positional_matching`
- `add_1d_different_dim_arrays_in_2d_context`
- `name_match_before_size_match_for_indexed_dims`
- `name_match_same_size_dims`

#### 1.2 Add N-dimensional tests (3D, 4D)
```rust
#[test]
fn broadcast_to_3d() {
    // plane[X,Y] + line[Z] → cube[X,Y,Z]
    let project = TestProject::new("broadcast_3d")
        .indexed_dimension("X", 2)
        .indexed_dimension("Y", 3)
        .indexed_dimension("Z", 4)
        .array_aux("plane[X, Y]", "X * 10 + Y")  // 2x3
        .array_aux("line[Z]", "Z * 100")         // 4
        .array_aux("cube[X, Y, Z]", "plane + line");
    // cube[x,y,z] = plane[x,y] + line[z]
    // Expected: 24 values in row-major order
    project.assert_compiles();
    project.assert_sim_builds();
    // Verify specific values...
}

#[test]
fn broadcast_scalar_to_4d() {
    // scalar + 4D array
    let project = TestProject::new("broadcast_4d")
        .indexed_dimension("A", 2)
        .indexed_dimension("B", 2)
        .indexed_dimension("C", 2)
        .indexed_dimension("D", 2)
        .scalar_const("offset", 100.0)
        .array_aux("arr[A, B, C, D]", "A + B*2 + C*4 + D*8")
        .array_aux("result[A, B, C, D]", "arr + offset");
    project.assert_compiles();
    // ...
}

#[test]
fn broadcast_multiple_inputs_3d() {
    // a[X] + b[Y] + c[Z] → result[X,Y,Z]
    // Three 1D arrays broadcasting to 3D
    let project = TestProject::new("triple_broadcast")
        .indexed_dimension("X", 2)
        .indexed_dimension("Y", 3)
        .indexed_dimension("Z", 4)
        .array_aux("a[X]", "X * 1000")
        .array_aux("b[Y]", "Y * 100")
        .array_aux("c[Z]", "Z")
        .array_aux("result[X, Y, Z]", "a + b + c");
    // result[x,y,z] = a[x] + b[y] + c[z]
    project.assert_compiles();
    // ...
}
```

#### 1.3 Run tests to confirm they fail
```bash
cargo test -p simlin-engine indexed_dimension_broadcasting
```
Document the specific error messages to understand what needs fixing.

### Phase 2: Core Algorithm - Unified Dimension Matching

**Goal:** Create ONE function that handles ALL dimension matching, replacing scattered logic.

#### 2.1 The N-Dimensional Matching Algorithm

```rust
/// Result of matching source dimensions to target dimensions.
///
/// For each target dimension, provides either:
/// - Some(source_idx): which source dimension maps here
/// - None: no source dimension (broadcast with stride 0)
pub struct DimensionMapping {
    /// mapping[target_idx] = Some(source_idx) or None
    pub mapping: Vec<Option<usize>>,
    /// For each source dimension, which target dimension it matched
    pub source_to_target: Vec<usize>,
}

/// Match source dimensions to target dimensions.
///
/// Algorithm (dimension-agnostic, works for any N):
/// 1. For each source dimension, find a matching target dimension
///    - Named dims: match by name only
///    - Indexed dims: match by name first, then by size
/// 2. Verify all source dimensions found a match
/// 3. Build the reverse mapping (target → source)
///
/// Returns None if any source dimension cannot be matched.
pub fn match_dimensions(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<DimensionMapping> {
    let mut target_used = vec![false; target_dims.len()];
    let mut source_to_target = Vec::with_capacity(source_dims.len());

    // For each source dimension, find its target
    for source_dim in source_dims {
        let target_idx = find_target_for_source(source_dim, target_dims, &target_used)?;
        target_used[target_idx] = true;
        source_to_target.push(target_idx);
    }

    // Build reverse mapping
    let mut mapping = vec![None; target_dims.len()];
    for (source_idx, &target_idx) in source_to_target.iter().enumerate() {
        mapping[target_idx] = Some(source_idx);
    }

    Some(DimensionMapping { mapping, source_to_target })
}

/// Find target dimension for a source dimension.
fn find_target_for_source(
    source_dim: &Dimension,
    target_dims: &[Dimension],
    used: &[bool],
) -> Option<usize> {
    // First pass: exact name match (works for both named and indexed)
    for (i, target) in target_dims.iter().enumerate() {
        if !used[i] && target.name() == source_dim.name() {
            return Some(i);
        }
    }

    // Second pass: size-based match (indexed dimensions only)
    if let Dimension::Indexed(_, source_size) = source_dim {
        for (i, target) in target_dims.iter().enumerate() {
            if !used[i] {
                if let Dimension::Indexed(_, target_size) = target {
                    if source_size == target_size {
                        return Some(i);
                    }
                }
            }
        }
    }

    None
}
```

#### 2.2 Broadcasting Function (N-dimensional)

```rust
/// Broadcast a source view to match target dimensions.
///
/// For each target dimension:
/// - If source has a matching dimension: use its stride
/// - If no match: use stride 0 (broadcast/repeat)
///
/// This is dimension-agnostic: works for any N.
pub fn broadcast_view(
    source_view: &ArrayView,
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<ArrayView> {
    let mapping = match_dimensions(source_dims, target_dims)?;

    let mut new_dims = Vec::with_capacity(target_dims.len());
    let mut new_strides = Vec::with_capacity(target_dims.len());
    let mut new_dim_names = Vec::with_capacity(target_dims.len());

    for (target_idx, target_dim) in target_dims.iter().enumerate() {
        new_dims.push(target_dim.len());
        new_dim_names.push(target_dim.name().to_string());

        match mapping.mapping[target_idx] {
            Some(source_idx) => {
                // Source dimension maps here - use its stride
                new_strides.push(source_view.strides[source_idx]);
            }
            None => {
                // No source dimension - broadcast (stride 0)
                new_strides.push(0);
            }
        }
    }

    Some(ArrayView {
        dims: new_dims,
        strides: new_strides,
        offset: source_view.offset,
        sparse: Vec::new(),
        dim_names: new_dim_names,
    })
}
```

### Phase 3: Replace Scattered Matching Logic

**Goal:** Delete old code paths and replace with unified functions.

#### 3.1 Delete `find_dimension_reordering` (or reimplement using `match_dimensions`)

The old function only supported name matching. Either:
- Delete it entirely and use `match_dimensions` everywhere
- Reimplement it as a thin wrapper around `match_dimensions`

**Recommended:** Delete and replace all call sites.

#### 3.2 Simplify `get_implicit_subscripts`

Replace the scattered matching logic (lines 765-820) with:
```rust
fn get_implicit_subscripts(&self, dims: &[Dimension], ident: &str) -> Result<Vec<&str>> {
    let active_dims = self.active_dimension.as_ref()
        .ok_or_else(|| sim_err!(ArrayReferenceNeedsExplicitSubscripts, ident))?;
    let active_subscripts = self.active_subscript.as_ref().unwrap();

    let mapping = match_dimensions(dims, active_dims)
        .ok_or_else(|| sim_err!(MismatchedDimensions, ident))?;

    // Build subscripts in source dimension order
    Ok(mapping.source_to_target.iter()
        .map(|&target_idx| active_subscripts[target_idx].as_str())
        .collect())
}
```

#### 3.3 Simplify Subscript handling in `lower_from_expr3`

The complex logic at lines 1649-1800 should be replaced with calls to `match_dimensions` and `broadcast_view`.

#### 3.4 Simplify bare Var handling

Replace the reordering logic at lines 1480-1560 with the unified approach.

### Phase 4: Integration and Cleanup

#### 4.1 Update Op2 handling for broadcasting

When processing binary operations, check if either operand needs broadcasting:

```rust
// Pseudocode
fn lower_op2(&self, op, left, right, output_bounds) -> Result<Expr> {
    let l_expr = self.lower_from_expr3(left)?;
    let r_expr = self.lower_from_expr3(right)?;

    if let Some(output_dims) = self.get_output_dimensions(output_bounds) {
        let l_dims = self.get_expr_dimensions(&l_expr);
        let r_dims = self.get_expr_dimensions(&r_expr);

        // Apply broadcasting if dimensions don't match output
        let l_expr = self.maybe_broadcast(l_expr, l_dims, &output_dims)?;
        let r_expr = self.maybe_broadcast(r_expr, r_dims, &output_dims)?;
    }

    Ok(Expr::Op2(op, Box::new(l_expr), Box::new(r_expr), loc))
}
```

#### 4.2 Delete dead code

After replacing all call sites:
- Delete old `find_dimension_reordering` if not reused
- Delete any fallback paths that were superseded
- Delete helper functions that are no longer called
- Run `cargo clippy` to find unused code

#### 4.3 Final test verification

```bash
# All tests should pass
cargo test -p simlin-engine

# Verify no warnings about unused code
cargo clippy -p simlin-engine
```

## Testing Strategy

### TDD Approach

1. **Enable all ignored tests FIRST** - they define expected behavior
2. **Add N-dimensional tests** - verify generalization works
3. **Run tests** - they should fail initially
4. **Implement** - until all tests pass
5. **Cleanup** - remove dead code, verify no regressions

### Test Categories

| Category | Example | Tests |
|----------|---------|-------|
| Same-rank positional | `a[DimA] + b[DimB]` → `result[DimA]` | `different_indexed_dims_same_size_broadcast`, `different_indexed_dims_with_wildcard` |
| Cross-dimension 2D | `a[X] + b[Y]` → `result[X,Y]` | `add_1d_different_dim_arrays_in_2d_context`, `name_match_*` |
| Cross-dimension 3D | `plane[X,Y] + line[Z]` → `cube[X,Y,Z]` | `broadcast_to_3d` (new) |
| Cross-dimension 4D | `scalar + arr[A,B,C,D]` | `broadcast_scalar_to_4d` (new) |
| Multiple broadcasts | `a[X] + b[Y] + c[Z]` → 3D | `broadcast_multiple_inputs_3d` (new) |

### Regression Tests

All existing passing tests must continue to pass:
- Named dimension matching (by name only)
- Subdimension relationships
- Existing array operations

## Key Design Decisions

### 1. Named vs Indexed Semantics
- **Named dimensions** (Cities, Products): Match by name ONLY. `Cities[Boston,Seattle]` ≠ `Products[Widget,Gadget]` even if both size 2.
- **Indexed dimensions** (Dim(5)): Match by name first, then by size. This enables `a[DimA(3)] + b[DimB(3)]`.

### 2. Matching Algorithm
- **Greedy**: First match wins. When multiple same-size indexed dims exist, first unused one matches.
- **All-or-nothing**: If any source dimension can't match, the operation fails.

### 3. Broadcasting via Stride 0
- Unmatched target dimensions get stride 0 in the view
- This means the value repeats (broadcasts) over that dimension
- No data copying required - pure view manipulation

### 4. No Compatibility Shims
- Old `find_dimension_reordering` will be deleted or reimplemented
- All four code paths will be unified into ONE approach
- No fallback to old behavior

## Risks and Mitigation

| Risk | Mitigation |
|------|------------|
| Breaking named dimension semantics | Size-based matching ONLY for indexed dimensions |
| Ambiguous multi-match | Greedy algorithm with first-unused selection |
| N-dimensional edge cases | Comprehensive test suite including 3D, 4D cases |
| Performance regression | Algorithm is O(n²) on dimension count, which is always small (<10) |

## Summary

The implementation consolidates scattered dimension matching logic into TWO key functions:
1. `match_dimensions()` - unified N-dimensional matching algorithm
2. `broadcast_view()` - creates views with stride 0 for broadcasting

These replace the four separate code paths with ONE consistent approach, enabling both same-rank positional matching and cross-dimension broadcasting for any number of dimensions.
