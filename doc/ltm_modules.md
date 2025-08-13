# LTM Module Handling Design

## Problem Statement

When feedback loops cross module boundaries in system dynamics models, the Loops That Matter (LTM) method needs a principled approach to handle the encapsulation. Users care about how inputs to a module influence outputs, not the internal link scores. For example, in a loop like "population stock → SMOOTH3 module → inflow → population stock", users want to understand the module's role in the loop dominance, not SMOOTH3's internal dynamics.

## Background

The LTM method calculates:
- **Link scores**: Measure the contribution of one variable to changes in another
- **Loop scores**: Products of all link scores in a feedback loop
- **Relative loop scores**: Normalized loop scores for dominance analysis

The 2023 improvement (Schoenberg et al.) updated flow-to-stock link scores to use second-order changes, making the method insensitive to flow aggregation. This provides a foundation for thinking about module boundaries as another form of aggregation.

## Three Candidate Approaches

### Approach 1: Black Box Module Treatment

Treat modules as atomic units that transform inputs to outputs.

**Implementation:**
- Calculate a single "module transfer score" from each input to each output
- This score represents the aggregate effect of all internal pathways
- For module M with input x and output y:
  ```
  Module_Transfer_Score(x→y) = Δy_due_to_x / Δy × sign(Δy/Δx)
  ```
- Replace all internal link scores with this single transfer score when calculating loop scores

**Advantages:**
- Simple and intuitive
- Matches user mental model of modules as functional units
- Minimal computational overhead
- No need to expose internal structure

**Disadvantages:**
- Loses information about internal dynamics
- Cannot distinguish between different internal pathways
- May miss important internal feedback effects

### Approach 2: Selective Internal Score Export

Export only the link scores that participate in loops crossing module boundaries.

**Implementation:**
- During loop detection, identify loops that cross module boundaries
- For each cross-boundary loop, trace the path through the module
- Mark internal link scores on these paths as "export-required"
- Export these scores with names like `module_instance._ltm_internal_link_x_to_y`
- Parent model imports these scores when calculating loop scores

**Example:**
```
// In parent model, for a loop through SMOOTH3:
_ltm_loop_R1 = _ltm_link_population_to_smooth3_input 
             * smooth3_instance._ltm_internal_path_score
             * _ltm_link_smooth3_output_to_inflow
             * _ltm_link_inflow_to_population
```

**Advantages:**
- Preserves critical information for loop analysis
- Allows proper loop score calculation across boundaries
- Maintains mathematical correctness of the LTM method

**Disadvantages:**
- Complex to determine which scores to export
- Requires module introspection during loop detection
- Increases coupling between modules and parent models

### Approach 3: Hierarchical Aggregation with Path Weighting

Calculate and aggregate link scores based on their participation in cross-boundary dynamics.

**Implementation:**
- For each module input-output pair, calculate all possible internal paths
- Weight each path by its contribution to the output change
- Aggregate weighted path scores into a composite transfer score
- Use importance sampling for modules with many internal paths

**Mathematical Framework:**

For module M with input x, output y, and internal paths P₁, P₂, ..., Pₙ:

```
Transfer_Score(x→y) = Σᵢ (Path_Score(Pᵢ) × Path_Weight(Pᵢ))

where:
Path_Score(Pᵢ) = ∏(link scores along path i)
Path_Weight(Pᵢ) = |Δy_via_Pᵢ| / |Δy_total|
```

**Advantages:**
- Comprehensive representation of module behavior
- Accounts for multiple internal pathways
- Theoretically sound aggregation

**Disadvantages:**
- Computationally expensive for complex modules
- Requires detailed path enumeration
- May be overkill for simple modules

## Chosen Approach: Black Box Module Treatment (Option 1)

After evaluation, we are implementing **Option 1: Black Box Module Treatment** as the initial approach. This provides a simple, intuitive foundation that can be extended later if needed. The other approaches are documented above for future reference and potential enhancement.

### Implementation Overview

With the black box approach:

1. **Module Transfer Scores**: Calculate a single transfer score for each input-output pair that represents the aggregate effect of all internal pathways.

2. **Loop Integration**: When loops cross module boundaries, use the module transfer score in place of tracing through internal structure.

3. **Mathematical Consistency**: Maintain the same mathematical framework as regular link scores to ensure proper loop score calculation.

### Implementation Steps

#### Phase 1: Module Transfer Score Calculation

For each module instance:
1. Identify all input variables (marked with `access="input"`)
2. Identify all output variables (marked with `access="output"` or the module itself for single-output modules)
3. For each input-output pair, calculate the transfer score:
   ```
   transfer_score = (Δoutput_due_to_input / Δoutput) × sign(Δoutput/Δinput)
   ```

#### Phase 2: Cross-Boundary Loop Integration

When a loop crosses module boundaries:
1. Replace the module-internal portion with the module transfer score
2. Chain the transfer score with external link scores:
   ```
   loop_score = external_links_score × module_transfer_score
   ```

#### Phase 3: Special Case Handling

Handle specific module types appropriately:
- Single-output modules where the module instance itself is the output
- Multi-output modules with explicit output variables
- Builtin modules with known transfer functions

### Special Cases

#### Builtin Modules (SMOOTH, DELAY, etc.)
- Treat as black boxes with known transfer functions
- Use analytical formulas where possible
- For SMOOTH3: transfer_score ≈ (1 - exp(-dt/tau))^3

#### Module Arrays
- Calculate transfer scores element-wise
- Aggregate using the same rules as regular array variables

#### Nested Modules
- Apply the method recursively
- Aggregate from innermost to outermost

## Example: SMOOTH3 in Population Model

Consider the loop: "population → SMOOTH3 → inflow → population"

**Without module handling:**
- Would need to trace through SMOOTH3's three internal stocks
- Complex and not meaningful to users

**With recommended approach:**
1. Calculate SMOOTH3 transfer score:
   ```
   smooth3_transfer = Δsmooth3_output / Δsmooth3_input × (1 - exp(-dt/tau))³
   ```
2. Calculate loop score:
   ```
   loop_score = link_score(population→smooth3) 
              × smooth3_transfer
              × link_score(smooth3→inflow)
              × link_score(inflow→population)
   ```

## Validation Strategy

1. **Equivalence Testing**: Verify that loop scores are consistent when:
   - A module is expanded inline vs. kept encapsulated
   - Flows are aggregated vs. disaggregated (per 2023 improvement)

2. **Benchmark Models**: Test with standard models containing:
   - Simple modules (single path)
   - Complex modules (multiple paths)
   - Nested modules
   - Module arrays

3. **Performance Testing**: Ensure computational overhead is acceptable for:
   - Models with many module instances
   - Deeply nested modules
   - Modules with many internal feedback loops

## Conclusion

The recommended adaptive hybrid approach balances simplicity, correctness, and user needs. It treats modules as black boxes by default while preserving the mathematical integrity of loop score calculations. This approach maintains the key insight from the 2023 improvement—that aggregation level shouldn't affect the analysis—and extends it to module boundaries.

The implementation should be staged, starting with black box treatment and gradually adding path aggregation and selective export capabilities based on user needs and performance constraints.