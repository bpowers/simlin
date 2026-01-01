# Plan: Different-Named Indexed Dimension Broadcasting

## Executive Summary

This document outlines the implementation plan for supporting different-named indexed dimension broadcasting in the Simlin engine compiler. This feature allows operations like `sales[Products] + costs[Regions]` where Products and Regions are different indexed dimensions with the same size.

## Problem Statement

Currently, the compiler only allows dimension matching by name. Two test categories are failing:

### Category 1: Same-Output-Dimension Broadcasting (Simpler)
```
indexed_dimension("Products", 3)
indexed_dimension("Regions", 3)
array_aux("sales[Products]", "Products")        // [1, 2, 3]
array_aux("costs[Regions]", "Regions * 10")     // [10, 20, 30]
array_aux("combined[Products]", "sales + costs") // Should be [11, 22, 33]
```

Here, `costs[Regions]` needs to match `Products` via positional/size-based matching because both are indexed dimensions of size 3.

### Category 2: 2D Broadcasting (Complex)
```
indexed_dimension("First", 2)
indexed_dimension("Second", 2)
array_aux("a[First]", "First * 10")   // [10, 20]
array_aux("b[Second]", "Second")      // [1, 2]
array_aux("combined[First, Second]", "a + b")
// Should be: [11, 12, 21, 22] (row-major)
// combined[i,j] = a[i] + b[j]
```

This requires true broadcasting: `a[First]` repeats over the Second dimension, and `b[Second]` repeats over the First dimension.

## Current Architecture Analysis

### Key Code Paths

There are **four main code paths** where dimension matching occurs:

1. **`find_dimension_reordering`** (compiler.rs:4559)
   - Purpose: Match source dimensions to target dimensions for reordering
   - Current behavior: Exact name matching only
   - Status: Needs positional fallback for indexed dimensions

2. **`get_implicit_subscripts`** (compiler.rs:716)
   - Purpose: Determine which active subscripts to use for bare array references
   - Current behavior: Has size-based fallback (lines 783-813)
   - Status: Logic exists but not applied consistently elsewhere

3. **Subscript handling in `lower_from_expr3`** (compiler.rs:1649-1800)
   - Purpose: Resolve array subscripts to concrete offsets in A2A context
   - Current behavior: Name-matching with incomplete positional fallback
   - Status: Needs coordinated updates

4. **Bare Var handling** (compiler.rs:1480-1560)
   - Purpose: Handle unsubscripted array variable references in A2A context
   - Current behavior: Uses `find_dimension_reordering` which is name-only
   - Status: Needs positional fallback

### Current Size-Based Matching Logic (Reference)

The size-based fallback in `get_implicit_subscripts` (lines 783-813) provides a reference implementation:

```rust
// SECOND PASS: Only if no name match exists, try size-based matching
// for indexed dimensions. Find the first unused indexed dimension with
// the same size.
//
// IMPORTANT: Size-based fallback only applies when BOTH dimensions are
// indexed. Named dimensions must match by name (or subdimension relationship)
// because their elements have semantic meaning.
let size_match_idx = if let Dimension::Indexed(_, dim_size) = dim {
    active_dims.iter().enumerate().find_map(|(i, candidate)| {
        if !used[i]
            && let Dimension::Indexed(_, candidate_size) = candidate
            && dim_size == candidate_size
        {
            return Some(i);
        }
        None
    })
} else {
    None
};
```

Key principles:
- **Indexed dimensions only**: Named dimensions must match by name
- **Size equality required**: Dimensions must have the same size
- **First unused match**: Preserves positional semantics

## Implementation Plan

### Phase 1: Create Unified Dimension Matching Helper

Create a new helper function that encapsulates the matching logic used in `get_implicit_subscripts` so it can be reused consistently across all code paths.

**New function: `find_dimension_matching`**

```rust
/// Result of dimension matching for a single source dimension.
enum DimensionMatch {
    /// Found an exact name match at the given index
    NameMatch(usize),
    /// Found a positional match (indexed dimensions with same size)
    PositionalMatch(usize),
    /// No match found
    NoMatch,
}

/// Find a matching active dimension for the given source dimension.
///
/// Matching priority:
/// 1. Exact name match (highest priority)
/// 2. Positional match for indexed dimensions of same size
///
/// Named dimensions ONLY match by name - never by size.
fn find_dimension_match(
    source_dim: &Dimension,
    active_dims: &[Dimension],
    used: &[bool],
) -> DimensionMatch {
    // First pass: exact name match
    for (i, candidate) in active_dims.iter().enumerate() {
        if !used[i] && candidate.name() == source_dim.name() {
            return DimensionMatch::NameMatch(i);
        }
    }

    // Second pass: positional match for indexed dimensions only
    if let Dimension::Indexed(_, dim_size) = source_dim {
        for (i, candidate) in active_dims.iter().enumerate() {
            if !used[i] {
                if let Dimension::Indexed(_, candidate_size) = candidate {
                    if dim_size == candidate_size {
                        return DimensionMatch::PositionalMatch(i);
                    }
                }
            }
        }
    }

    DimensionMatch::NoMatch
}
```

### Phase 2: Update `find_dimension_reordering`

Extend to support positional matching as fallback for indexed dimensions.

**Current signature:**
```rust
pub fn find_dimension_reordering(
    source_dims: &[String],
    target_dims: &[String],
) -> Option<Vec<usize>>
```

**New signature:**
```rust
pub fn find_dimension_reordering_with_dims(
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<Vec<usize>>
```

This allows the function to check dimension types and apply size-based matching for indexed dimensions.

### Phase 3: Update Bare Var Handling

Modify the code at lines 1480-1560 to use the new matching logic.

The key change is replacing:
```rust
if let Some(reordering) = find_dimension_reordering(&source_dim_names, &target_dim_names)
```

With:
```rust
if let Some(reordering) = find_dimension_reordering_with_dims(source_dims, target_dims)
```

### Phase 4: Update Subscript Handling in A2A Context

The most complex changes are in the subscript handling (lines 1649-1800).

**Current logic flow:**
1. Build `active_dim_map` from dimension name → (index, subscript)
2. For each view dimension, try name-based matching first
3. Fall back to positional matching only if name matching fails and counts match

**Required changes:**
1. Add size-based matching as a middle tier:
   - If name matching fails
   - AND source dim is indexed
   - AND there's an unused active dim with same size (also indexed)
   - Then use that active dim's subscript

**Key modification around line 1717:**
```rust
let (active_idx, subscript) = if use_name_matching[view_idx] {
    // Name-based matching (existing code)
    ...
} else {
    // Enhanced positional matching
    // Check if we can use size-based matching for indexed dimensions
    let view_dim_name = &view.dim_names[view_idx];
    let view_dim_size = view.dims[view_idx];

    // Try to find matching indexed dimension by size
    let size_match = if is_indexed_dimension(view_dim_name, &self.dimensions) {
        active_dims.iter().enumerate()
            .filter(|(i, _)| !used_active[*i])
            .find(|(_, dim)| {
                matches!(dim, Dimension::Indexed(_, size) if *size as usize == view_dim_size)
            })
            .map(|(i, _)| (i, &active_subscripts[i]))
    } else {
        None
    };

    if let Some((idx, sub)) = size_match {
        used_active[idx] = true;
        (idx, sub)
    } else {
        // Fall back to strict positional matching
        (view_idx, &active_subscripts[view_idx])
    }
};
```

### Phase 5: Handle 2D Broadcasting (Category 2)

This is the most complex case and requires additional infrastructure.

**Problem:** When we have:
- Output: `combined[First, Second]`
- Input: `a[First]` (1D), `b[Second]` (1D)
- Operation: `a + b`

We need to recognize that:
- `a[First]` should broadcast to `[First, Second]` by repeating over Second
- `b[Second]` should broadcast to `[First, Second]` by repeating over First

**Approach:** The key insight is that broadcasting is already partially handled by the dimension matching logic. When we have fewer source dimensions than target dimensions:

1. **Dimension Identification**: For each source dimension, find which output dimension it matches (by name for named dims, by size/position for indexed dims)

2. **Stride Adjustment**: Create a view where:
   - Matched dimension: use normal stride
   - Unmatched dimensions: use stride of 0 (value repeats)

**Example for `a[First]` in `combined[First, Second]`:**
- Source view: dims=[2], strides=[1], offset=0
- After broadcasting: dims=[2, 2], strides=[1, 0], offset=0
  - The stride of 0 for Second means a[i] is used for all j values

**Implementation:**

Add a `broadcast_view_to_target` function:

```rust
/// Broadcast a view to match target dimensions.
///
/// For each target dimension:
/// - If source has a matching dimension: use its stride
/// - If source lacks this dimension: use stride 0 (broadcast/repeat)
///
/// Returns None if dimensions are incompatible.
fn broadcast_view_to_target(
    source_view: &ArrayView,
    source_dims: &[Dimension],
    target_dims: &[Dimension],
) -> Option<ArrayView> {
    let mut new_dims = Vec::with_capacity(target_dims.len());
    let mut new_strides = Vec::with_capacity(target_dims.len());
    let mut new_dim_names = Vec::with_capacity(target_dims.len());

    for target_dim in target_dims {
        // Find matching source dimension
        let source_match = find_source_dimension_for_target(
            target_dim, source_dims, source_view
        );

        match source_match {
            Some((source_idx, stride)) => {
                new_dims.push(target_dim.len());
                new_strides.push(stride);
                new_dim_names.push(target_dim.name().to_string());
            }
            None => {
                // No match - broadcast (use stride 0)
                new_dims.push(target_dim.len());
                new_strides.push(0);
                new_dim_names.push(target_dim.name().to_string());
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

### Phase 6: Integration into Binary Operations

Modify the Op2 handling in `lower_from_expr3` (around line 1973) to apply broadcasting when operand dimensions don't match output dimensions.

```rust
Expr3::Op2(op, left, right, array_bounds, loc) => {
    let mut l_expr = self.lower_from_expr3(left)?;
    let mut r_expr = self.lower_from_expr3(right)?;

    if let Some(bounds) = array_bounds {
        let output_dims = bounds.dims();
        let output_dim_names = bounds.dim_names();

        // Check if broadcasting is needed
        if let Some(output_names) = output_dim_names {
            // Get dimensions of each operand
            let l_dims = get_expr_dims(&l_expr);
            let r_dims = get_expr_dims(&r_expr);

            // Apply broadcasting if needed
            if l_dims.map(|d| d.len()).unwrap_or(0) < output_dims.len() {
                l_expr = apply_broadcast(l_expr, l_dims, output_names)?;
            }
            if r_dims.map(|d| d.len()).unwrap_or(0) < output_dims.len() {
                r_expr = apply_broadcast(r_expr, r_dims, output_names)?;
            }
        }
    }

    Ok(Expr::Op2(lower_op(*op), Box::new(l_expr), Box::new(r_expr), *loc))
}
```

## Testing Strategy

### Test Order

1. **Phase 1-4 tests** (same-dimension-count broadcasting):
   - `different_indexed_dims_same_size_broadcast`
   - `different_indexed_dims_with_wildcard`
   - `indexed_dims_2d_positional_matching`

2. **Phase 5-6 tests** (cross-dimension broadcasting):
   - `add_1d_different_dim_arrays_in_2d_context`
   - `name_match_before_size_match_for_indexed_dims`
   - `name_match_same_size_dims`

### Regression Prevention

- All existing tests must continue to pass
- Named dimensions must NOT use size-based matching
- Only indexed dimensions with matching sizes can be positionally matched

## Risks and Mitigation

### Risk 1: Breaking existing dimension matching
**Mitigation:** Size-based matching is ONLY applied to indexed dimensions. Named dimensions always require exact name or subdimension matching.

### Risk 2: Ambiguous matching
**Mitigation:** When multiple indexed dimensions have the same size, use the first unused one (preserving positional semantics). This matches Stella/Vensim behavior.

### Risk 3: Complexity in VM
**Mitigation:** The bytecode VM already has infrastructure for stride-based iteration. Broadcasting with stride 0 is a natural extension.

## Implementation Order

1. Create the unified `find_dimension_match` helper
2. Update `find_dimension_reordering` to support dimension types
3. Update bare Var handling (Phase 3)
4. Update Subscript handling (Phase 4)
5. Implement `broadcast_view_to_target` (Phase 5)
6. Integrate broadcasting into Op2 (Phase 6)
7. Enable ignored tests and verify

## Estimated Complexity

- Phase 1: Low (new helper function)
- Phase 2: Low (extend existing function)
- Phase 3: Low (use new helper)
- Phase 4: Medium (careful integration with existing logic)
- Phase 5: Medium (new broadcasting logic)
- Phase 6: Medium (integration with expression lowering)

Total: ~400-600 lines of new/modified code across compiler.rs.
