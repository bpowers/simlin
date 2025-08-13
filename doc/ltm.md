# Loops That Matter (LTM) Implementation Design

## Overview

This document describes the design for implementing the Loops That Matter (LTM) method in simlin.
LTM is a loop dominance analysis technique that determines which feedback loops drive model behavior at each point in time during simulation.


## Approach

We will augment the `Project` with synthetic variables that calculate link scores and loop scores during normal simulation. This approach:
- Is opt-in and default off, but enabled with a flag
- Leverages existing simulation infrastructure
- Maintains compatibility with both interpreter and bytecode VM

We will need to add a new "SIGN" builtin as well.

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

### Relative Loop Score
For analysis, we normalize loop scores:

```
RelativeLoopScore(L) = LoopScore(L) / Σ|LoopScore(Li)| for all loops
```

## Implementation Design

### 1. Loop Detection Phase

Before augmenting the project, identify all feedback loops:

```rust
pub struct LoopDetector {
    loops: Vec<Loop>,
}

pub struct Loop {
    id: String,
    links: Vec<(Ident<Canonical>, Ident<Canonical>)>, // (from, to) pairs
    stocks: Vec<Ident<Canonical>>, // stocks in the loop
}

impl LoopDetector {
    pub fn find_loops(project: &Project) -> Vec<Loop> {
        // Use graph algorithms to find all elementary circuits
        // Group by cycle partition (connected components via feedback)
    }
}
```


### 2. Project Augmentation

Add a new method to create an LTM-instrumented project:

```rust
impl Project {
    pub fn with_ltm(mut self) -> Result<Self> {
        // 1. Detect all loops
        let loops = LoopDetector::find_loops(&self);
        
        // 2. For each model, add synthetic variables
        for model in &mut self.models {
            augment_model_with_ltm(model, &loops)?;
        }
        
        Ok(self)
    }
}
```

### 3. Synthetic Variable Generation

For each model, add:

#### Link Score Variables
For each causal link (x → y):
- Variable name: `_ltm_link_{x}_to_{y}`
- Equation: Implements link score calculation.  
  The previous value of variables is available via the `PREVIOUS` builtin.
  The partial change caclulation is different for each link score, so there is no good reason to create additional temporary/intermediate values -- the full formulation of link score will be in this equation.

#### Loop Score Variables  
For each loop L:
- Variable name: `_ltm_loop_{loop_id}`
- Equation: Product of constituent link scores

### 4. Equation Generation

#### Standard Link Score Calculation
```rust
fn generate_link_score_equation(from: &str, to: &str, target_equation: &str, deps: &[&str]) -> String {
    // For auxiliary to auxiliary
    // We need to calculate the partial change in 'to' with respect to 'from'
    // by evaluating the target equation with current 'from' but previous values for other deps
    
    // Build the partial equation: substitute PREVIOUS(dep) for all deps except 'from'
    let mut partial_eq = target_equation.to_string();
    for dep in deps {
        if *dep != from {
            partial_eq = partial_eq.replace(dep, &format!("PREVIOUS({})", dep));
        }
    }
    
    format!(
        "IF THEN ELSE(
            ({to} - PREVIOUS({to})) = 0 :OR: ({from} - PREVIOUS({from})) = 0,
            0,
            ABS((({partial_eq}) - PREVIOUS({to})) / ({to} - PREVIOUS({to}))) * 
            IF THEN ELSE(
                ({from} - PREVIOUS({from})) = 0,
                0,
                SIGN((({partial_eq}) - PREVIOUS({to})) / ({from} - PREVIOUS({from})))
            )
        )",
        to = to,
        from = from,
        partial_eq = partial_eq
    )
}
```

#### Flow-to-Stock Link Score
```rust
fn generate_flow_stock_link_equation(flow: &str, stock: &str, is_inflow: bool) -> String {
    let sign = if is_inflow { "" } else { "-" };
    format!(
        "IF THEN ELSE(
            ({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock}))) = 0,
            0,
            {sign}(({flow} - PREVIOUS({flow})) / (({stock} - PREVIOUS({stock})) - (PREVIOUS({stock}) - PREVIOUS(PREVIOUS({stock})))))
        )",
        stock = stock,
        flow = flow,
        sign = sign
    )
}
```

### 5. Module Boundaries

Loops that cross module boundaries require special handling:

#### Module Interface Variables
When a loop crosses from module A to module B:
1. Module A exports the partial link score up to the boundary.  This can happen via an annotation on the synthetic link score variable rather than a _separate_ export variable.
2. Module B imports this value and continues the calculation
3. The complete loop score is calculated in the outermost module

```rust
// In the model definition, mark link score variables that need to be exported:
// This would be done during augment_model_with_ltm()

// For a link score that crosses module boundary:
let link_var = Variable {
    ident: "_ltm_link_output_x_to_module_B_input_y".into(),
    equation: generate_link_score_equation(...),
    is_exported: true,  // Mark for export
    ..Default::default()
};

// In module B, create an import reference:
let import_var = Variable {
    ident: "_ltm_import_input_y_from_module_A".into(),
    equation: "module_A._ltm_link_output_x_to_module_B_input_y".into(),
    ..Default::default()
};

// Loop score in outermost module uses the imported value:
let loop_score = Variable {
    ident: "_ltm_loop_cross_module_loop_1".into(),
    equation: "_ltm_import_input_y_from_module_A * _ltm_link_y_to_z * _ltm_link_z_to_module_A_x".into(),
    ..Default::default()
};
```

### 6. Variable Ordering

LTM variables must be calculated in proper dependency order:
1. Link scores (`_ltm_link_{x}_to_{y}`)
2. Imports of link scores from module instances
3. Loop scores (`_ltm_loop_{id}`)


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
let loop_score = results.get_series("_ltm_loop_R1")?;
let link_score = results.get_series("_ltm_link_population_to_births")?;
```


## Testing Strategy

1. Unit tests for loop detection algorithm
2. Verify link score calculations against hand-calculated examples
3. Compare loop dominance results with published models from LTM papers:
   - Bass diffusion model
   - Population models
   - Inventory/workforce model


## References

- Schoenberg et al. (2020): "Understanding model behavior using the Loops that Matter method"
- Schoenberg et al. (2023): "Improving Loops that Matter" (corrected flow-to-stock formula)


## TODO

The following high-level chunks are necessary to implement LTM, in order:

### Phase 1: Prerequisites
1. **Add SIGN builtin function** - Required for link score polarity calculation. Should return -1, 0, or 1 based on input sign.
2. **Verify PREVIOUS builtin** - Ensure PREVIOUS(PREVIOUS(x)) works correctly for second-order stock changes needed in flow-to-stock calculations.

### Phase 2: Loop Detection
3. **Implement basic graph representation** - Create graph structure from model variables and their dependencies to enable loop detection.
4. **Implement elementary circuit detection** - Use Johnson's algorithm or similar to find all elementary feedback loops in the dependency graph.
5. **Add loop classification** - Distinguish between reinforcing (R) and balancing (B) loops based on product of link polarities.

### Phase 3: Core Link Score Calculations
6. **Implement auxiliary-to-auxiliary link scores** - Generate equations for standard variable-to-variable causal links using partial derivative approximation.
7. **Implement flow-to-stock link scores** - Generate specialized equations for inflows/outflows using the 2023 corrected formula with second-order stock changes.
8. **Implement stock-to-flow link scores** - Handle feedback from stocks to their own flows (e.g., population affecting death rate).

### Phase 4: Loop Score Aggregation
9. **Generate loop score variables** - Create synthetic variables that multiply constituent link scores for each detected loop.
10. **Add relative loop score calculation** - Normalize loop scores for dominance analysis (percentage contribution).

### Phase 5: Project Augmentation Infrastructure
11. **Create Project::with_ltm() method** - Main entry point that orchestrates loop detection and variable augmentation.
12. **Implement variable name generation** - Ensure synthetic variable names (_ltm_*) don't conflict with user variables.
13. **Handle variable ordering** - Ensure LTM variables are calculated in proper dependency order during simulation.

### Phase 6: Module Boundaries
14. **Design module export mechanism** - Mark link score variables that need to cross module boundaries for export.
15. **Implement cross-module loop tracking** - Handle loops that span multiple module instances with proper import/export of partial scores.

### Phase 7: Error Handling and Limitations
16. **Add array detection and error reporting** - Initially error out when with_ltm() is called on models with array variables.
17. **Add validation for unsupported features** - Check for other limitations (e.g., delays, lookups) and provide clear error messages.

### Phase 8: Testing and Validation
18. **Create unit tests for loop detection** - Test with known graph structures including self-loops, parallel paths, and nested loops.
19. **Add integration tests with simple models** - Verify link and loop scores against hand-calculated examples (e.g., exponential growth, goal-seeking).
20. **Validate against published LTM examples** - Implement and test Bass diffusion, population, and inventory models from the papers.

### Phase 9: API and Results Access
21. **Add LTM results to simulation output** - Ensure _ltm_* variables are accessible through standard results API.
22. **Create helper methods for LTM analysis** - Add convenience functions to extract dominant loops at specific time points.

### Phase 10: Future Enhancements (Post-MVP)
23. **Array support** - Extend to handle element-wise link scores for array variables.
24. **Performance optimization** - Cache partial calculations, optimize equation generation.
25. **Visualization support** - Add metadata for UI to highlight dominant loops during simulation.