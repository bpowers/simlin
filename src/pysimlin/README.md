# pysimlin - Python bindings for Simlin

Python bindings for the Simlin system dynamics simulation engine.

## Features

- Load models from XMILE, Vensim MDL, and Simlin JSON and protobuf formats
- Run system dynamics simulations with full control
- Get simulation results as pandas DataFrames
- Analyze model structure and feedback loops
- Edit existing models or build new ones programmatically via Python context managers
- Full type hints for IDE support
- Loops That Matter (LTM) analysis for feedback loop importance

## Installation

```bash
pip install pysimlin
```

**Note:** Install with `pip install pysimlin` but import with `import simlin`.

## Requirements

- Python 3.11 or higher
- numpy >= 1.22.0
- pandas >= 1.5.0
- cffi >= 1.15.0

## Quick Start

```python
import simlin
from simlin import Project
from simlin.json_types import Stock, Flow, Auxiliary

# Create a simple population model programmatically
project = Project.new(
    name="population-demo",
    sim_start=0.0,
    sim_stop=100.0,
    dt=0.25,
    time_units="years"
)

model = project.get_model()
with model.edit() as (_, patch):
    # Stock: population level
    patch.upsert_stock(Stock(
        name="population",
        initial_equation="1000",
        inflows=["births"],
        outflows=["deaths"]
    ))

    # Flows: births and deaths
    patch.upsert_flow(Flow(name="births", equation="population * birth_rate"))
    patch.upsert_flow(Flow(name="deaths", equation="population * death_rate"))

    # Parameters
    patch.upsert_aux(Auxiliary(name="birth_rate", equation="0.03"))
    patch.upsert_aux(Auxiliary(name="death_rate", equation="0.02"))

# Run simulation and get results
run = model.run(analyze_loops=False)
print(run.results.head())

# Access individual variables
population_series = run.results["population"]
print(f"Population grows from {population_series.iloc[0]:.0f} to {population_series.iloc[-1]:.0f}")
```

## Examples

### Editing a flow in an existing model

<!-- pysimlin-test: reset -->
```python
"""Example showing how to edit an existing model's flow equation with pysimlin."""

from __future__ import annotations

import simlin


EXAMPLE_XMILE = b"""<?xml version='1.0' encoding='utf-8'?>
<xmile version=\"1.0\" xmlns=\"http://docs.oasis-open.org/xmile/ns/XMILE/v1.0\" xmlns:isee=\"http://iseesystems.com/XMILE\" xmlns:simlin=\"https://simlin.com/XMILE/v1.0\">
  <header>
    <name>pysimlin-edit-example</name>
    <vendor>Simlin</vendor>
    <product version=\"0.1.0\" lang=\"en\">Simlin</product>
  </header>
  <sim_specs method=\"Euler\" time_units=\"Year\">
    <start>0</start>
    <stop>80</stop>
    <dt>0.25</dt>
  </sim_specs>
  <model name=\"main\">
    <variables>
      <stock name=\"population\">
        <eqn>25</eqn>
        <inflow>net_birth_rate</inflow>
      </stock>
      <flow name=\"net_birth_rate\">
        <eqn>fractional_growth_rate * population</eqn>
      </flow>
      <aux name=\"fractional_growth_rate\">
        <eqn>maximum_growth_rate * (1 - population / carrying_capacity)</eqn>
      </aux>
      <aux name=\"maximum_growth_rate\">
        <eqn>0.10</eqn>
      </aux>
      <aux name=\"carrying_capacity\">
        <eqn>1000</eqn>
      </aux>
    </variables>
  </model>
</xmile>
"""


def run_simulation(model: simlin.Model) -> float:
    """Run the model to the configured stop time and return the ending population."""

    with model.simulate() as sim:
        sim.run_to_end()
        return float(sim.get_value("population"))


def main() -> None:
    """Demonstrate editing a flow equation and verify the change takes effect."""

    # Load model from XMILE bytes by writing to temp file first
    import tempfile
    import os

    with tempfile.NamedTemporaryFile(suffix=".stmx", delete=False) as f:
        f.write(EXAMPLE_XMILE)
        temp_path = f.name

    try:
        model = simlin.load(temp_path)
        baseline_final = run_simulation(model)

        with model.edit() as (current, patch):
            flow = current["net_birth_rate"]
            flow.equation = "fractional_growth_rate * population * 1.5"
            patch.upsert_flow(flow)

        accelerated_final = run_simulation(model)

        if not accelerated_final > baseline_final + 10:
            raise RuntimeError(
                "Edited model did not accelerate growth as expected: "
                f"baseline={baseline_final:.2f} accelerated={accelerated_final:.2f}"
            )

        print(
            "Updated growth equation increased the final population from "
            f"{baseline_final:.1f} to {accelerated_final:.1f}."
        )
    finally:
        os.unlink(temp_path)


if __name__ == "__main__":
    main()
```

### Building a logistic population model programmatically

<!-- pysimlin-test: reset -->
```python
"""Create a new Simlin project and build a simple population model using pysimlin's edit API."""

from __future__ import annotations

import simlin
from simlin.json_types import Stock, Flow, Auxiliary


def build_population_project() -> simlin.Project:
    """Return a project containing a logistic population model created via model.edit()."""

    project = simlin.Project.new(
        name="pysimlin-population-example",
        sim_start=0.0,
        sim_stop=80.0,
        dt=0.25,
        time_units="years",
    )

    model = project.get_model()
    with model.edit() as (_, patch):
        population = Stock(
            name="population",
            initial_equation="50",
            inflows=["births"],
            outflows=["deaths"],
        )
        patch.upsert_stock(population)

        births = Flow(
            name="births",
            equation="population * birth_rate",
        )
        patch.upsert_flow(births)

        deaths = Flow(
            name="deaths",
            equation="population * birth_rate * (population / 1000)",
        )
        patch.upsert_flow(deaths)

        birth_rate = Auxiliary(
            name="birth_rate",
            equation="0.08",
        )
        patch.upsert_aux(birth_rate)

    return project


def validate_population_curve(values: list[float]) -> None:
    """Ensure the generated population series shows logistic (S-shaped) growth."""

    if len(values) < 3:
        raise RuntimeError("Population series is unexpectedly short")

    if any(b < a for a, b in zip(values, values[1:])):
        raise RuntimeError("Population should not decline in this model")

    initial = values[0]
    mid = values[len(values) // 2]
    last = values[-1]
    growth_first_half = mid - initial
    growth_second_half = last - mid

    if not growth_first_half > 0:
        raise RuntimeError("Population failed to grow early in the simulation")

    if not growth_second_half > 0:
        raise RuntimeError("Population failed to grow late in the simulation")

    if not growth_second_half < growth_first_half:
        raise RuntimeError("Logistic growth should slow over time")

    if not 950 <= last <= 1025:
        raise RuntimeError(
            "Population should approach the carrying capacity (~1000), "
            f"but ended at {last:.2f}"
        )


def main() -> None:
    """Build, simulate, and validate the population model."""

    project = build_population_project()
    errors = project.get_errors()
    if errors:
        raise RuntimeError(f"Generated project contains validation errors: {errors}")

    model = project.get_model()
    with model.simulate() as sim:
        sim.run_to_end()
        population_series = [float(value) for value in sim.get_series("population")]

    validate_population_curve(population_series)

    print(
        "Population grows from "
        f"{population_series[0]:.1f} to {population_series[-1]:.1f}, forming an S-shaped trajectory."
    )


if __name__ == "__main__":
    main()
```

Both examples live under `src/pysimlin/examples/` and are executed by `scripts/pysimlin-tests.sh`.

## API Reference

### Loading Models

```python
import simlin
from simlin import Project
from simlin.json_types import Stock, Flow, Auxiliary

# Create a model programmatically (used by all API examples below)
project = Project.new(
    name="api-demo",
    sim_start=0.0,
    sim_stop=100.0,
    dt=0.25,
    time_units="years"
)

model = project.get_model()
with model.edit() as (_, patch):
    patch.upsert_stock(Stock(
        name="population",
        initial_equation="1000",
        inflows=["births"],
        outflows=["deaths"]
    ))
    patch.upsert_flow(Flow(name="births", equation="population * birth_rate"))
    patch.upsert_flow(Flow(name="deaths", equation="population * death_rate"))
    patch.upsert_aux(Auxiliary(name="birth_rate", equation="0.03"))
    patch.upsert_aux(Auxiliary(name="death_rate", equation="0.02"))

print(f"Created model with {len(model.get_var_names())} variables")
```

You can also load models from files:

<!-- pysimlin-test: skip -->
```python
# Load from file (auto-detects format from extension)
model = simlin.load("model.stmx")  # .stmx, .mdl, .json, etc.
project = model.project
```

### Working with Models

```python
from simlin import VARTYPE_STOCK, VARTYPE_FLOW, VARTYPE_AUX

# Get variable names, optionally filtered by type
all_names = model.get_var_names()                          # All variable names
stock_names = model.get_var_names(type_mask=VARTYPE_STOCK) # Stock names only
flow_names = model.get_var_names(type_mask=VARTYPE_FLOW)   # Flow names only
aux_names = model.get_var_names(type_mask=VARTYPE_AUX)     # Aux names only

# Get detailed variable information
for name in model.get_var_names():
    var = model.get_variable(name)
    if var is not None:
        print(f"{var.name} ({type(var).__name__})")

# Get time configuration
time_spec = model.time_spec
print(f"Simulation: t={time_spec.start} to {time_spec.stop}, dt={time_spec.dt}")

# Analyze variable dependencies
incoming_deps = model.get_incoming_links("population")

# Get causal links
links = model.get_links()
for link in links:
    print(f"{link.from_var} --{link.polarity}--> {link.to_var}")

# Check for model issues
issues = model.check()
for issue in issues:
    print(f"{issue.severity}: {issue.message}")

# Get explanation for a variable
explanation = model.explain("population")
print(explanation)
```

### Model Editing

```python
from dataclasses import replace
from simlin.json_types import Stock, Flow, Auxiliary

# Edit existing model variables using context manager
with model.edit() as (current, patch):
    # Access current variables by name (returns Stock, Flow, Auxiliary, or Module)
    stock_var = current["population"]

    # Modify the variable using dataclasses.replace()
    updated_stock = replace(stock_var, initial_equation="100")

    # Apply the change
    patch.upsert_stock(updated_stock)

# Create new variables programmatically
with model.edit() as (current, patch):
    # Create a new auxiliary variable
    new_aux = Auxiliary(
        name="growth_rate",
        equation="0.05",
    )
    patch.upsert_aux(new_aux)

    # Create a new flow variable
    new_flow = Flow(
        name="births",
        equation="population * growth_rate",
    )
    patch.upsert_flow(new_flow)
```

### Running Simulations

```python
# High-level API: run and get results immediately
run = model.run(analyze_loops=False)
print(run.results.head())

# Run with variable overrides
run = model.run(overrides={"birth_rate": 0.05}, analyze_loops=False)

# Use the cached base case
base_case = model.base_case  # Automatically cached
print(base_case.results["population"].tail())

# Low-level API: create simulation for step-by-step control
with model.simulate() as sim:
    sim.run_to(50.0)        # Run to specific time
    sim.set_value("growth_rate", 0.10)  # Intervention
    sim.run_to_end()        # Continue to end
    run = sim.get_run()     # Get results as Run object

# Enable Loops That Matter analysis
with model.simulate(enable_ltm=True) as sim:
    sim.run_to_end()
    run = sim.get_run()
    print(run.dominant_periods)
```

### Accessing Results

```python
# Results are pandas DataFrames
run = model.run(analyze_loops=False)
df = run.results  # Time series for all variables

# Access specific variables
population = df["population"]
births = df["births"]

# Standard pandas operations
print(df.describe())
print(df.tail())

# Get metadata
time_spec = run.time_spec
overrides = run.overrides  # Dict of variable overrides used
```

### Model Interventions

```python
# Run with different parameter values
scenarios = {}
for rate in [0.02, 0.03, 0.04]:
    run = model.run(
        overrides={"birth_rate": rate},
        analyze_loops=False
    )
    scenarios[f"rate_{rate}"] = run.results["population"]

# Compare scenarios
import pandas as pd
comparison = pd.DataFrame(scenarios)
print(comparison.tail())
```

### Feedback Loop Analysis

```python
run = model.run()

# Access feedback loops with polarity and behavioral importance
for loop in run.loops:
    print(f"Loop {loop.id} ({loop.polarity}): {' -> '.join(loop.variables)}")
    if loop.behavior_time_series is not None:
        avg_importance = loop.average_importance()
        print(f"  avg importance = {avg_importance:.3f}")

# Analyze dominant periods
for period in run.dominant_periods:
    print(f"t=[{period.start_time}, {period.end_time}]: {period.dominant_loops}")
```

#### Loop Polarity

Loops are classified by polarity, which indicates how they affect the system:

- **R (Reinforcing)**: Loop amplifies changes (positive loop scores)
- **B (Balancing)**: Loop counteracts changes (negative loop scores)
- **U (Undetermined)**: Loop polarity cannot be reliably determined

Loop IDs use the polarity as a prefix (e.g., "R1", "B2", "U3").

When you run a simulation, pysimlin computes actual loop scores at each timestep. The polarity is classified based on these runtime values:
- If loop scores are consistently positive throughout: Reinforcing
- If loop scores are consistently negative throughout: Balancing
- If loop scores change sign during simulation: Undetermined (occurs in nonlinear models where link effects depend on variable values)

```python
from simlin import LoopPolarity

run = model.run()

# Filter by polarity
reinforcing = [l for l in run.loops if l.polarity == LoopPolarity.REINFORCING]
balancing = [l for l in run.loops if l.polarity == LoopPolarity.BALANCING]
undetermined = [l for l in run.loops if l.polarity == LoopPolarity.UNDETERMINED]
```

### Loops That Matter (LTM)

```python
# Run simulation with LTM enabled
sim = model.simulate(enable_ltm=True)
sim.run_to_end()

# Get links with importance scores over time
links = sim.get_links()
for link in links:
    if link.has_score():
        print(f"{link.from_var} -> {link.to_var}")
        print(f"  Average score: {link.average_score():.4f}")
        print(f"  Max score: {link.max_score():.4f}")

# Get relative loop scores
loops = project.get_loops()
if loops:
    loop_scores = sim.get_relative_loop_score(loops[0].id)
```

### Model Export

```python
from pathlib import Path

# Export to different formats
xmile_bytes = project.to_xmile()           # Export as XMILE XML
json_bytes = project.serialize_json()      # Export as JSON

print(f"XMILE export: {len(xmile_bytes)} bytes")
print(f"JSON export: {len(json_bytes)} bytes")

# Save to file (example - commented out to avoid creating files)
# Path("exported.stmx").write_bytes(xmile_bytes)
# Path("model.json").write_bytes(json_bytes)
```

### Error Handling

```python
from simlin import (
    SimlinError,
    SimlinImportError,
    SimlinRuntimeError,
    SimlinCompilationError,
    ErrorCode
)

# Check for compilation errors on the existing project
errors = project.get_errors()
if errors:
    for error in errors:
        print(f"{error.code.name} in {error.model_name}/{error.variable_name}")
        print(f"  {error.message}")
else:
    print("No errors in project")
```

When loading models from files, you can catch import errors:

<!-- pysimlin-test: skip -->
```python
try:
    model = simlin.load("model.stmx")
except SimlinImportError as e:
    print(f"Import failed: {e}")
    if e.code == ErrorCode.XML_DESERIALIZATION:
        print("Invalid XML format")
```

## Complete Example

This example demonstrates loading a model from file and comparing scenarios with matplotlib:

<!-- pysimlin-test: skip -->
```python
import simlin
import pandas as pd
import matplotlib.pyplot as plt

# Load and run a population model
model = simlin.load("population_model.stmx")

# Run baseline simulation
with model.simulate() as sim:
    sim.run_to_end()
    baseline = sim.get_run().results

# Run intervention scenario
with model.simulate() as sim:
    sim.set_value("birth_rate", 0.03)
    sim.run_to_end()
    intervention = sim.get_run().results

# Compare results
fig, ax = plt.subplots()
ax.plot(baseline.index, baseline["population"], label="Baseline")
ax.plot(intervention.index, intervention["population"], label="Intervention")
ax.set_xlabel("Time")
ax.set_ylabel("Population")
ax.legend()
plt.show()
```

## Supported Platforms

- macOS (ARM64)
- Linux (ARM64, x86_64)

## License

Apache License 2.0

## Development

For development setup and contribution guidelines, see the main [Simlin repository](https://github.com/bpowers/simlin).

### Running Tests

```bash
cd src/pysimlin
pip install -e ".[dev]"
pytest
```

### Building from Source

```bash
cd src/pysimlin
python -m build
```
