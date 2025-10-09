# pysimlin - Python bindings for Simlin

Python bindings for the Simlin system dynamics simulation engine.

## Features

- Load models from XMILE, Vensim MDL, and binary protobuf formats
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

# Load a model (auto-detects format)
model = simlin.load("model.stmx")

# Run simulation and get results
run = model.run()
print(run.results.head())

# Access individual variables
population = run.results["population"]

# Or use low-level simulation for gaming/interactive use
with model.simulate() as sim:
    sim.run_to_end()
    run = sim.get_run()
    print(run.results.head())
```

## Examples

### Editing a flow in an existing model

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

    with model.new_sim() as sim:
        sim.run_to_end()
        return float(sim.get_value("population"))


def main() -> None:
    """Demonstrate editing a flow equation and verify the change takes effect."""

    project = simlin.Project.from_xmile(EXAMPLE_XMILE)
    model = project.get_model()

    baseline_final = run_simulation(model)

    with model.edit() as (current, patch):
        flow_var = current["net_birth_rate"]
        flow_var.flow.equation.scalar.equation = (
            "fractional_growth_rate * population * 1.5"
        )
        patch.upsert_flow(flow_var.flow)

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


if __name__ == "__main__":
    main()
```

### Building a logistic population model programmatically

```python
"""Create a new Simlin project and build a simple population model using pysimlin's edit API."""

from __future__ import annotations

import simlin
import simlin.pb as pb


def build_population_project() -> simlin.Project:
    """Return a project containing a logistic population model created via model.edit()."""

    project = simlin.Project.new(
        name="pysimlin-population-example",
        sim_start=0.0,
        sim_stop=80.0,
        dt=0.25,
        time_units="years",
    )

    project.set_sim_specs(
        start=0.0,
        stop=80.0,
        dt={"value": 0.25},
        time_units="years",
    )

    model = project.get_model()
    with model.edit() as (_, patch):
        population = pb.Variable.Stock()
        population.ident = "population"
        population.equation.scalar.equation = "50"
        population.inflows.extend(["births"])
        population.outflows.extend(["deaths"])
        patch.upsert_stock(population)

        births = pb.Variable.Flow()
        births.ident = "births"
        births.equation.scalar.equation = "population * birth_rate"
        patch.upsert_flow(births)

        deaths = pb.Variable.Flow()
        deaths.ident = "deaths"
        deaths.equation.scalar.equation = "population * birth_rate * (population / 1000)"
        patch.upsert_flow(deaths)

        birth_rate = pb.Variable.Aux()
        birth_rate.ident = "birth_rate"
        birth_rate.equation.scalar.equation = "0.08"
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
    with model.new_sim() as sim:
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

# Load a model (auto-detects format from extension)
model = simlin.load("model.stmx")  # .stmx, .mdl, .json, etc.

# Access the underlying project if needed
project = model.project

# Create a new blank project/model programmatically
from simlin import Project
project = Project.new(name="my-project", sim_start=0, sim_stop=100, dt=0.25)
model = project.get_model()
```

### Working with Models

```python
# Access model structure via properties
stocks = model.stocks        # Tuple of Stock objects
flows = model.flows          # Tuple of Flow objects
auxs = model.auxs            # Tuple of Aux objects
variables = model.variables  # All variables (stocks + flows + auxs)

# Access individual variable properties
for stock in model.stocks:
    print(f"{stock.name}: initial = {stock.initial_equation}")

for flow in model.flows:
    print(f"{flow.name}: {flow.equation}")

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
import simlin.pb as pb

# Edit existing model variables using context manager
with model.edit() as (current, patch):
    # Access current variables by name
    stock_var = current["population"]

    # Modify the variable's properties
    stock_var.stock.equation.scalar.equation = "100"  # Change initial value

    # Apply the change
    patch.upsert_stock(stock_var.stock)

# Create new variables programmatically
with model.edit() as (current, patch):
    # Create a new auxiliary variable
    new_aux = pb.Variable.Aux()
    new_aux.ident = "growth_rate"
    new_aux.equation.scalar.equation = "0.05"
    patch.upsert_aux(new_aux)

    # Create a new flow variable
    new_flow = pb.Variable.Flow()
    new_flow.ident = "births"
    new_flow.equation.scalar.equation = "population * growth_rate"
    patch.upsert_flow(new_flow)
```

### Running Simulations

```python
# High-level API: run and get results immediately
run = model.run(analyze_loops=False)
print(run.results.head())

# Run with variable overrides
run = model.run(overrides={"initial_population": 1000}, analyze_loops=False)

# Use the cached base case
base_case = model.base_case  # Automatically cached
print(base_case.results["population"].plot())

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
gdp = df["gdp"]

# Standard pandas operations
print(df.describe())
print(df.tail())
df["population"].plot()

# Get metadata
time_spec = run.time_spec
overrides = run.overrides  # Dict of variable overrides used
```

### Model Interventions

```python
# Run with different initial conditions
scenarios = {}
for initial_pop in [100, 500, 1000]:
    run = model.run(
        overrides={"initial_population": initial_pop},
        analyze_loops=False
    )
    scenarios[f"pop_{initial_pop}"] = run.results["population"]

# Compare scenarios
import pandas as pd
comparison = pd.DataFrame(scenarios)
comparison.plot()
```

### Feedback Loop Analysis

```python
# Get structural feedback loops
loops = model.loops
for loop in loops:
    print(f"Loop {loop.id} ({loop.polarity}): {' -> '.join(loop.variables)}")

# Run with loop behavior analysis
run = model.run(analyze_loops=True)

# Access loops with behavioral importance
for loop in run.loops:
    if loop.behavior_time_series is not None:
        avg_importance = loop.average_importance()
        print(f"Loop {loop.id}: avg importance = {avg_importance:.3f}")

# Analyze dominant periods
for period in run.dominant_periods:
    print(f"t=[{period.start_time}, {period.end_time}]: {period.dominant_loops}")
```

### Loops That Matter (LTM)

```python
# Run simulation with LTM enabled
sim = model.new_sim(enable_ltm=True)
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
# Export to different formats
xmile_bytes = project.to_xmile()    # Export as XMILE
pb_bytes = project.serialize()      # Export as protobuf

# Save to file
Path("exported.stmx").write_bytes(xmile_bytes)
Path("model.pb").write_bytes(pb_bytes)
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

try:
    project = simlin.Project.from_file("model.stmx")
except SimlinImportError as e:
    print(f"Import failed: {e}")
    if e.code == ErrorCode.XML_DESERIALIZATION:
        print("Invalid XML format")

# Check for compilation errors
errors = project.get_errors()
for error in errors:
    print(f"{error.code.name} in {error.model_name}/{error.variable_name}")
    print(f"  {error.message}")
```

## Complete Example

```python
import simlin
import pandas as pd
import matplotlib.pyplot as plt

# Load and run a population model
with simlin.Project.from_file("population_model.stmx") as project:
    model = project.get_model()
    
    # Run baseline simulation
    with model.new_sim() as sim:
        sim.run_to_end()
        baseline = sim.get_results()
    
    # Run intervention scenario
    with model.new_sim() as sim:
        sim.set_value("birth_rate", 0.03)
        sim.run_to_end()
        intervention = sim.get_results()
    
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
