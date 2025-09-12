# pysimlin - Python bindings for Simlin

Python bindings for the Simlin system dynamics simulation engine.

## Features

- Load models from XMILE, Vensim MDL, and binary protobuf formats
- Run system dynamics simulations with full control
- Get simulation results as pandas DataFrames
- Analyze model structure and feedback loops
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
from pathlib import Path

# Load a model from file (auto-detects format)
project = simlin.Project.from_file("model.stmx")

# Get the default model
model = project.get_model()

# Create and run a simulation
sim = model.new_sim()
sim.run_to_end()

# Get results as a pandas DataFrame
results = sim.get_results()
print(results.head())

# Access individual variables
population = sim.get_series("population")
```

## API Reference

### Loading Models

```python
# Load from different formats
project = simlin.Project.from_xmile(xmile_bytes)  # XMILE/STMX format
project = simlin.Project.from_mdl(mdl_bytes)      # Vensim MDL format
project = simlin.Project.from_protobin(pb_bytes)  # Binary protobuf format

# Auto-detect format from file extension
project = simlin.Project.from_file("model.stmx")  # .stmx, .mdl, .pb, etc.

# Context manager for automatic cleanup
with simlin.Project.from_file("model.stmx") as project:
    model = project.get_model()
    # Project is automatically cleaned up when exiting the context
```

### Working with Projects

```python
# Get model information
model_names = project.get_model_names()

# Access models
model = project.get_model()           # Get default/main model
model = project.get_model("submodel") # Get specific model by name

# Check for compilation errors
errors = project.get_errors()
if errors:
    for error in errors:
        print(f"{error.code.name}: {error.message}")
```

### Model Analysis

```python
# Get model structure
var_names = model.get_var_names()
var_count = model.get_var_count()

# Analyze variable dependencies
incoming_deps = model.get_incoming_links("population")  # Variables that affect "population"

# Get causal links
links = model.get_links()
for link in links:
    print(f"{link.from_var} --{link.polarity}--> {link.to_var}")
```

### Running Simulations

```python
# Create simulation
sim = model.new_sim()                # Standard simulation
sim = model.new_sim(enable_ltm=True) # Enable Loops That Matter

# Run simulation
sim.run_to_end()        # Run to final time
sim.run_to(50.0)        # Run to specific time

# Reset to run again
sim.reset()
sim.run_to_end()

# Context manager for automatic cleanup
with model.new_sim() as sim:
    sim.run_to_end()
    results = sim.get_results()
```

### Accessing Results

```python
# Get results as pandas DataFrame
df = sim.get_results()                    # All variables
df = sim.get_results(variables=["x", "y"]) # Specific variables

# Get individual time series as numpy arrays
values = sim.get_series("population")

# Get current value (at current simulation time)
current_val = sim.get_value("population")

# Get metadata
step_count = sim.get_step_count()
```

### Model Interventions

```python
# Set initial values before running
sim.set_value("initial_population", 1000)
sim.run_to_end()

# Mid-simulation interventions
sim.run_to(10)
sim.set_value("growth_rate", 0.05)
sim.run_to_end()
```

### Feedback Loop Analysis

```python
# Get all feedback loops
loops = project.get_loops()
for loop in loops:
    print(f"Loop {loop.id} ({loop.polarity}): {' -> '.join(loop.variables)}")
    
# Check if variable is in a loop
for loop in loops:
    if loop.contains_variable("population"):
        print(f"Population is in loop {loop.id}")
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