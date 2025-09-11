# simlin - Python bindings for Simlin

Python bindings for the Simlin system dynamics simulation engine.

## Features

- Load models from XMILE, Vensim MDL, and binary protobuf formats
- Run system dynamics simulations with full control
- Get simulation results as pandas DataFrames
- Analyze model structure and feedback loops
- Full type hints for IDE support

## Installation

```bash
pip install simlin
```

## Quick Start

```python
import simlin

# Load a model from XMILE format
with open("model.stmx", "rb") as f:
    project = simlin.Project.from_xmile(f.read())

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

## Advanced Usage

### Working with Model Structure

```python
# Get all variables in the model
var_names = model.get_var_names()

# Analyze dependencies
deps = model.get_incoming_links("population")

# Get all causal links
links = model.get_links()
for link in links:
    print(f"{link.from_var} -> {link.to_var} ({link.polarity})")
```

### Feedback Loop Analysis

```python
# Analyze feedback loops
loops = project.get_loops()
for loop in loops:
    print(f"Loop {loop.id}: {' -> '.join(loop.variables)} ({loop.polarity})")

# Enable Loop Transmission Method analysis
sim_ltm = model.new_sim(enable_ltm=True)
sim_ltm.run_to_end()

# Get link scores over time
links_with_scores = sim_ltm.get_links()
```

### Error Handling

```python
# Check for model errors
errors = project.get_errors()
if errors:
    for error in errors:
        print(f"Error in {error.model_name}/{error.variable_name}: {error.message}")
```

## Supported Platforms

- macOS (ARM64)
- Linux (ARM64, x86_64)

## License

Apache License 2.0

## Development

See the main [Simlin repository](https://github.com/bpowers/simlin) for development instructions.