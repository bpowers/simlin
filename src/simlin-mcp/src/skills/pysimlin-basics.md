# pysimlin Basics

Install the Python bindings for the Simlin simulation engine:

```
pip install pysimlin=={PYSIMLIN_VERSION}
```

## Loading Models

Load a model file and get back the main `Model` object:

```python
import simlin

model = simlin.load("population.stmx")
```

Supported file formats: XMILE (`.stmx`, `.xmile`, `.xml`), Vensim MDL (`.mdl`), native JSON (`.json`), protobuf (`.pb`, `.bin`).

For multi-model projects, access other models through the `Project`:

```python
project = model.project
names = project.get_model_names()
other = project.get_model("submodel_name")
```

You can also open a project directly:

```python
project = simlin.Project.new(name="my project", sim_start=0, sim_stop=100, dt=0.25)
main = project.get_model()
```

## Running Simulations

Run with default parameters:

```python
run = model.run()
```

Run with parameter overrides:

```python
run = model.run(overrides={"birth_rate": 0.03, "death_rate": 0.01})
```

The cached `base_case` property runs once with defaults and caches the result:

```python
run = model.base_case
```

## Accessing Results

`run.results` is a pandas `DataFrame` indexed by simulation time:

```python
df = run.results

# Single variable
pop = df["population"]
final_value = df["population"].iloc[-1]

# Summary statistics
print(df["population"].describe())

# All column names (variable names)
print(df.columns.tolist())
```

## Time Specification

```python
ts = run.time_spec
print(f"From {ts.start} to {ts.stop}, dt={ts.dt}")
```

## Inspecting Variables

```python
# All variable names
names = model.get_var_names()

# Filter by type (bitmask)
stocks = model.get_var_names(type_mask=simlin.VARTYPE_STOCK)
flows = model.get_var_names(type_mask=simlin.VARTYPE_FLOW)
auxs = model.get_var_names(type_mask=simlin.VARTYPE_AUX)

# Get variable details
var = model.get_variable("population")
# var is a Stock, Flow, or Aux dataclass

# All variables as a tuple
for v in model.variables:
    print(v.name)
```

Stock objects have `initial_equation`, `inflows`, `outflows`, `units`, `documentation`.
Flow objects have `equation`, `units`, `documentation`, `graphical_function`.
Aux objects have `equation`, `active_initial`, `units`, `documentation`, `graphical_function`.

## Plotting with matplotlib

```python
import matplotlib.pyplot as plt

model = simlin.load("sir.stmx")
run = model.base_case

fig, ax = plt.subplots()
run.results[["susceptible", "infected", "recovered"]].plot(ax=ax)
ax.set_xlabel("Time")
ax.set_ylabel("Population")
ax.set_title("SIR Model")
plt.savefig("sir_results.png")
```

## Error Handling

```python
from simlin import SimlinError, SimlinCompilationError

try:
    model = simlin.load("broken_model.stmx")
    run = model.run()
except SimlinCompilationError as e:
    print(f"Compilation failed: {e}")
except SimlinError as e:
    print(f"Error: {e}")
```

Check for errors without running:

```python
issues = model.check()
for issue in issues:
    print(f"{issue.severity}: {issue.message} (variable: {issue.variable})")
```

Project-level errors:

```python
errors = model.project.get_errors()
for err in errors:
    print(f"[{err.code}] {err.message} (variable: {err.variable_name})")
```

## Context Managers

Both `Project` and `Model` support context managers for deterministic cleanup:

```python
with simlin.load("model.stmx") as model:
    run = model.run()
    print(run.results["population"].iloc[-1])
```
