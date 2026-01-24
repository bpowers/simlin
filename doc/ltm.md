# Loops That Matter (LTM) Implementation

## Overview

The Loops That Matter (LTM) method has been implemented in simlin to provide loop dominance analysis. This technique determines which feedback loops drive model behavior at each point in time during simulation.

## Implementation Approach

The implementation augments the `Project` with synthetic variables that calculate link scores and loop scores during normal simulation. This approach:
- Is opt-in and default off, enabled via the `with_ltm()` method
- Leverages existing simulation infrastructure
- Maintains compatibility with both interpreter and bytecode VM

## Key Concepts

### Link Score
A link score measures the contribution of one variable to the change in another variable. For standard (non-stock) links:

```
LinkScore(x → z) = |Δxz/Δz| × sign(Δxz/Δx)
```

Where:
- `Δxz` is the partial change in z with respect to x (change in z if only x changed)
- `Δz` is the total change in z
- The sign term captures link polarity

### Flow-to-Stock Link Score (Updated Formula from 2023 Paper)
For flows to stocks, we use the corrected formula that treats them consistently:

```
LinkScore(inflow → stock) = Δi / (ΔSt - ΔSt-dt)
LinkScore(outflow → stock) = -Δo / (ΔSt - ΔSt-dt)
```

Where:
- `Δi`, `Δo` are changes in inflow/outflow rates
- `ΔSt - ΔSt-dt` is the second-order change in stock (acceleration)

This corrects the original formulation's sensitivity to flow aggregation.


### Loop Score
A loop score is the product of all link scores in a feedback loop:

```
LoopScore(L) = ∏ LinkScore(link_i) for all links in L
```

The sign of a loop score indicates its structural polarity:
- **Positive loop score → Reinforcing (R) loop**: An even number of negative links yields a positive product, indicating the loop amplifies changes
- **Negative loop score → Balancing (B) loop**: An odd number of negative links yields a negative product, indicating the loop counteracts changes

### Loop Polarity Classification

Loops are classified by their polarity, which can be determined either structurally (before simulation) or at runtime (based on actual loop scores):

**Structural classification** (before simulation):
- **R (Reinforcing)**: All links have known polarity, with an even number of negative links
- **B (Balancing)**: All links have known polarity, with an odd number of negative links
- **U (Undetermined)**: Any link has unknown polarity (conservative: known × known × unknown = unknown)

**Runtime classification** (after simulation):
- **R (Reinforcing)**: Loop score is positive throughout the simulation
- **B (Balancing)**: Loop score is negative throughout the simulation
- **U (Undetermined)**: Loop score changes sign during the simulation (has both positive and negative values at different time points)

Undetermined polarity at runtime can occur in nonlinear models where link polarities change based on variable values. For example, in the yeast alcohol model, the births loop can change from reinforcing to effectively balancing when alcohol levels become high enough to reverse the birth rate.

### Relative Loop Score
For analysis, we normalize loop scores:

```
RelativeLoopScore(L) = LoopScore(L) / Σ|LoopScore(Li)| for all loops
```

## Core Implementation

### 1. Loop Detection

Loop detection is implemented in `src/simlin-engine/src/ltm.rs` using a graph-based approach:

```rust
pub struct CausalGraph {
    edges: HashMap<Ident<Canonical>, Vec<Ident<Canonical>>>,
    stocks: HashSet<Ident<Canonical>>,
    variables: HashMap<Ident<Canonical>, Variable>,
    module_graphs: HashMap<Ident<Canonical>, Box<CausalGraph>>,
}

pub struct Loop {
    pub id: String,
    pub links: Vec<Link>,
    pub stocks: Vec<Ident<Canonical>>,
    pub polarity: LoopPolarity,
}
```

The `CausalGraph::find_loops()` method uses a modified Johnson's algorithm to detect elementary circuits, including:
- Loops within single models
- Loops that cross module boundaries
- Proper polarity detection (Reinforcing vs Balancing)


### 2. Project Augmentation

The `Project::with_ltm()` method creates an LTM-instrumented project:

```rust
impl Project {
    pub fn with_ltm(self) -> Result<Self> {
        // Check for unsupported features (arrays)
        check_for_arrays(&self)?;
        
        // Generate synthetic variables
        let ltm_vars = ltm_augment::generate_ltm_variables(&self)?;
        
        // Add variables to datamodel and reconstruct project
        // ...
    }
}
```

The augmentation process:
1. Detects all loops using `detect_loops()`
2. Generates link score variables for each causal link
3. Generates loop score variables for each detected loop
4. Adds relative loop score variables for dominance analysis

### 3. Synthetic Variable Generation

The implementation in `src/simlin-engine/src/ltm_augment.rs` generates:

#### Link Score Variables
For each causal link (x → y):
- Variable name: `$⁚ltm⁚link_score⁚{x}⁚{y}`
- Equation: Implements the appropriate link score calculation based on link type:
  - Auxiliary-to-auxiliary: Standard partial derivative approximation
  - Flow-to-stock: Uses the 2023 corrected formula with second-order stock changes
  - Stock-to-flow: Considers feedback from stocks to their flows
  - Module links: Black box transfer scores

#### Loop Score Variables  
For each loop L:
- Absolute score: `$⁚ltm⁚abs_loop_score⁚{loop_id}` - Product of constituent link scores
- Relative score: `$⁚ltm⁚rel_loop_score⁚{loop_id}` - Normalized for dominance analysis

### 4. Equation Generation

The implementation generates equations for different link types:

#### Auxiliary-to-Auxiliary Links
For standard causal links between non-stock variables, the equation calculates the partial derivative approximation by substituting PREVIOUS values for all dependencies except the source variable. This isolates the contribution of the source to the target's change.

#### Flow-to-Stock Links
Uses the 2023 corrected formula based on second-order stock changes:
- For inflows: `Δflow / (ΔStock_t - ΔStock_t-dt)`
- For outflows: `-Δflow / (ΔStock_t - ΔStock_t-dt)`

This formulation makes the method insensitive to flow aggregation.

#### Stock-to-Flow Links
Handles feedback from stocks to their own flows, calculating how stock levels influence flow rates.

#### Module Links (Black Box Treatment)
Module transfer scores represent the aggregate effect of all internal pathways:
- Module inputs and outputs are treated as transfer functions
- Single transfer score replaces internal link tracing
- Maintains mathematical consistency while preserving encapsulation

### 5. Module Boundaries

The implementation handles loops that cross module boundaries using the black box approach described in `doc/ltm_modules.md`:

#### Black Box Module Treatment
- Modules are treated as atomic units that transform inputs to outputs
- A single "module transfer score" represents the aggregate effect of all internal pathways
- Module transfer score formula: `Δoutput_due_to_input / Δoutput × sign(Δoutput/Δinput)`

#### Implementation Details
The `CausalGraph` structure includes:
- `module_graphs`: Nested graphs for module instances
- Cross-module loop detection in `find_cross_module_loops()`
- Module-aware link score generation in `generate_module_link_score_equation()`

When loops cross module boundaries:
- The module instance itself represents the output value in the parent model
- Link scores for module connections use black box transfer scores
- Loop scores multiply the external and module transfer scores

### 6. Variable Ordering

LTM variables must be calculated in proper dependency order:
1. Link scores (`$⁚ltm⁚link_score⁚{x}⁚{y}`)
2. Imports of link scores from module instances
3. Loop scores (`$⁚ltm⁚abs_loop_score⁚{id}`)


### 7. Special Cases

#### Arrays and Subscripts
- Initially: leave as TODO and error out if with_ltm() is called on a model with arrays
- Calculate link scores element-wise for array variables
- Figure out how to report an aggergate link score if desired

#### Simultaneous Equations
- We don't have to worry about this, because models that have simultaneous equations are in error and unable to be simulated.

#### Stock Initialization
- Link scores are 0 at t=0 (no change yet)
- Begin calculating from first timestep

## API Usage

```rust
// Create project with LTM instrumentation
let project = Project::from(datamodel);
let instrumented_project = project.with_ltm()?;

// Run simulation normally
let sim = Simulation::new(&instrumented_project, "main")?;
let results = sim.run_to_end()?;

// Access LTM results through standard variable lookup
let loop_score = results.get_series("$⁚ltm⁚abs_loop_score⁚R1")?;
let link_score = results.get_series("$⁚ltm⁚link_score⁚population⁚births")?;
let relative_score = results.get_series("$⁚ltm⁚rel_loop_score⁚R1")?;
```


## Testing Strategy

Current test coverage includes:
- Unit tests for loop detection in `ltm.rs` (simple reinforcing loops, balancing loops, no-loop models)
- Module loop detection tests (single modules, multi-module loops)
- Link polarity detection tests
- Project augmentation tests in `project.rs` (with_ltm method, array detection)

Planned validation:
- Integration tests with published LTM models (Bass diffusion, population, inventory)
- Verification of link score calculations against hand-calculated examples
- End-to-end simulation tests once PREVIOUS builtin is implemented


## References

- Schoenberg et al. (2020): "Understanding model behavior using the Loops that Matter method"
- Schoenberg et al. (2023): "Improving Loops that Matter" (corrected flow-to-stock formula)


## Remaining Work

### Critical Dependencies
1. **Implement PREVIOUS builtin** - Required for link score equations to access previous timestep values. The equations currently reference PREVIOUS but it hasn't been implemented yet in the interpreter or compiler.

### Testing and Validation
2. **Add integration tests with published models** - Validate against Bass diffusion, population, and inventory models from the LTM papers to ensure correctness.
3. **Verify equation execution** - Once PREVIOUS is implemented, ensure link and loop score calculations execute correctly during simulation.

### Future Enhancements
4. **Array support** - Currently errors out when arrays are detected. Need to extend to handle element-wise link scores for array variables.
5. **Performance optimization** - Consider caching partial calculations and optimizing equation generation for large models.
6. **Visualization support** - Add metadata for UI to highlight dominant loops during simulation.
7. **Helper methods for analysis** - Add convenience functions to extract dominant loops at specific time points.