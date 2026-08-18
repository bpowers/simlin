# pysimlin

Python bindings for [Simlin](https://simlin.com), a system dynamics simulation engine.

## Features

- Load XMILE (`.stmx`, `.xmile`), Vensim (`.mdl`), sd-ai JSON, and Simlin JSON models
- Simulate with per-run parameter overrides; results as pandas DataFrames
- Loop dominance analysis ("Loops that Matter"): measure each feedback
  loop's contribution to behavior at every timestep, and find where
  dominance shifts
- Inspect model structure: variables, equations, causal links, feedback loops
- Edit models, or build them from scratch, through a transactional context manager
- Import Vensim `.vdf` binary output as DataFrames
- Generate SVG and PNG diagrams of the model's structure
- Full type hints

## Installation

```bash
pip install pysimlin
```

The distribution is named `pysimlin`; the importable package is `simlin`:

```python
import simlin
```

Requires Python 3.11+ on macOS (ARM64) or Linux (ARM64, x86_64). Depends on
numpy, pandas, and cffi.

## Quick Start

Build a logistic growth model, simulate it, and see which feedback loop
dominates when:

```python
import simlin
from simlin import Aux, Flow, Stock

project = simlin.Project.new(
    name="logistic-growth", sim_start=0, sim_stop=100, dt=0.25, time_units="years"
)

model = project.get_model()
with model.edit() as (_, patch):
    patch.upsert(Stock(name="population", initial_equation="50", inflows=["net_growth"]))
    patch.upsert(Flow(name="net_growth", equation="population * fractional_growth"))
    patch.upsert(Aux(
        name="fractional_growth",
        equation="max_growth_rate * (1 - population / carrying_capacity)",
    ))
    patch.upsert(Aux(name="max_growth_rate", equation="0.08"))
    patch.upsert(Aux(name="carrying_capacity", equation="10000"))

run = model.run()
print(f"final population: {run.results['population'].iloc[-1]:.0f}")

for loop in run.loops:
    print(f"{loop.id} ({loop.polarity}): average importance {loop.average_importance():.2f}")

for period in run.dominant_periods:
    print(f"t=[{period.start_time:.0f}, {period.end_time:.0f}] dominated by {period.dominant_loops}")
```

Output:

```
final population: 9359
b1 (B): average importance 0.34
r1 (R): average importance 0.66
t=[0, 67] dominated by ('r1',)
t=[67, 100] dominated by ('b1',)
```

The model has two feedback loops: the reinforcing compounding loop `r1`
(`population -> net_growth -> population`) and the balancing crowding loop
`b1` through `fractional_growth`. `run()` simulates with loop analysis
enabled, scoring every loop's contribution to behavior at every timestep.
The reinforcing loop dominates the first two-thirds of the run; as
population approaches the carrying capacity, the balancing loop takes over
-- the handoff at t=67 is the inflection point of the S-curve.

Loop ids (`r1`, `b1`) are stable identifiers assigned from the model's
structure; the polarity in parentheses is classified from the run's actual
behavior. The two usually agree, but they are independent claims: a loop
whose sign genuinely depends on parameter values (initialize `population`
above `carrying_capacity` and the compounding loop acts balancing) keeps
its structural id while the runtime polarity reports what the run really
did.

Every model is also a diagram. `render_svg()` draws the stock-and-flow
structure, computing a layout automatically for a model that doesn't
already carry one (`render_png()` is the bitmap sibling):

```python
from pathlib import Path

Path("logistic-growth.svg").write_bytes(project.render_svg())
```

<img src="https://raw.githubusercontent.com/bpowers/simlin/main/src/pysimlin/docs/logistic-growth.svg"
     width="431" alt="Stock-and-flow diagram of the logistic growth model">


## Examples

Complete, runnable programs live in
[`examples/`](https://github.com/bpowers/simlin/tree/main/src/pysimlin/examples):

- [`edit_existing_model.py`](https://github.com/bpowers/simlin/blob/main/src/pysimlin/examples/edit_existing_model.py)
  loads an XMILE model, changes a flow's equation, and verifies the change
  alters behavior.
- [`population_model.py`](https://github.com/bpowers/simlin/blob/main/src/pysimlin/examples/population_model.py)
  builds a model from scratch and validates the shape of its output.

Both run in CI on every commit.

## API Reference

Method-level documentation lives in the docstrings (`help(simlin.Model)`,
etc.); this section is a guided tour. Its examples continue with the
`model` built in Quick Start.

### Loading Models

`simlin.load()` reads a model file, auto-detecting the format from its
extension:

<!-- pysimlin-test: skip -->
```python
model = simlin.load("model.stmx")   # XMILE (.stmx, .xmile, .xml)
model = simlin.load("model.mdl")    # Vensim
model = simlin.load("model.json")   # Simlin JSON
project = model.project             # the containing Project
```

To start from nothing, create a project with `simlin.Project.new()` and add
variables with `model.edit()`, as in Quick Start.

### Working with Models

```python
from simlin import VARTYPE_STOCK, VARTYPE_FLOW, VARTYPE_AUX

names = model.get_var_names()                          # all variables
stocks = model.get_var_names(type_mask=VARTYPE_STOCK)  # just stocks

var = model.get_variable("population")   # Stock | Flow | Aux | Module | None
spec = model.time_spec                   # start, stop, dt, units

deps = model.get_incoming_links("net_growth")  # direct inputs of one variable
for link in model.get_links():                 # every causal link, with polarity
    print(link)                                # e.g. "fractional_growth --+--> net_growth"

print(model.explain("population"))
# population is a stock with initial value 50, increased by net_growth, ...

# Audit a model you loaded but didn't write: issues, or an empty tuple
for issue in model.check():
    print(f"{issue.severity}: {issue.message}")
```

### Model Editing

`model.edit()` opens a transaction: `current` maps variable names to their
definitions, and `patch` collects changes. Edits are validated and applied
together when the `with` block exits. An edit that would introduce a
circular dependency, an invalid equation, or a new unit error raises and
the model is unchanged, with the message naming the offending variable. A
completed `edit()` block needs no follow-up `check()` -- an accepted edit
is a valid model:

<!-- pysimlin-test: expect-error -->
```python
with model.edit() as (_, patch):
    patch.upsert(Aux(name="broken", equation="no_such_var * 2"))
```

Variables are frozen dataclasses -- the same `Stock`/`Flow`/`Aux` objects
everywhere, whether you read them or write them. To change one, derive an
updated copy with `dataclasses.replace` and `upsert` it:

```python
from dataclasses import replace

# Change an equation
with model.edit() as (current, patch):
    patch.upsert(replace(current["carrying_capacity"], equation="12000"))

# Add a harvest outflow: create the flow, then attach it to the stock
with model.edit() as (current, patch):
    patch.upsert(Flow(name="harvest", equation="population * harvest_fraction"))
    patch.upsert(Aux(name="harvest_fraction", equation="0.01"))
    stock = current["population"]
    patch.upsert(replace(stock, outflows=[*stock.outflows, "harvest"]))
```

### Running Simulations

```python
# Simulate to the end of the configured time range
run = model.run()

# Override constants for a single run (parameters only, not computed variables)
run = model.run(overrides={"max_growth_rate": 0.12})

# The no-overrides run, computed once and cached
base = model.base_case
```

`run()` performs loop analysis by default; pass `analyze_loops=False` to
skip it when you only need the time series. Simulations are deterministic,
so runs are reproducible and diffable. To change the time range or `dt`,
update the project's sim specs first (`model.project.set_sim_specs()`).

For step-by-step control -- inspecting state mid-run, or intervening at a
specific time -- use `model.simulate()`:

```python
with model.simulate() as sim:
    sim.run_to(50.0)
    sim.set_value("max_growth_rate", 0.12)  # intervene at t=50
    sim.run_to_end()
    run = sim.get_run()
```

### Accessing Results

`Run.results` is a pandas DataFrame: the index is simulation time, with one
column per variable. Arrayed variables appear as one column per element
(`"stock[element]"`).

```python
df = run.results
print(df["population"].describe())
print(df.tail())

spec = run.time_spec       # start/stop/dt of this run
changed = run.overrides    # overrides used for this run
```

### Importing Vensim Data Files (VDF)

Vensim saves simulation output in a binary `.vdf` format. `simlin.load_vdf`
reads one directly (no model file needed) and returns a DataFrame shaped
exactly like `Run.results`. Simulation runs, sensitivity runs, and imported
dataset files are auto-detected from the file magic.

<!-- pysimlin-test: skip -->
```python
import pandas as pd

df = simlin.load_vdf("Current.vdf")
print(df["water_level"].iloc[-1])

# Compare a Vensim run against a Simlin re-simulation. The two frames
# have independent time indexes, so align on the shared time points.
run = simlin.load("model.mdl").run(analyze_loops=False)
comparison = pd.DataFrame(
    {"vensim": df["water_level"], "simlin": run.results["water_level"]}
).dropna()
```

### Model Interventions

Compare scenarios by running with different overrides:

```python
import pandas as pd

scenarios = {
    f"growth={rate}": model.run(overrides={"max_growth_rate": rate}).results["population"]
    for rate in [0.04, 0.08, 0.12]
}
print(pd.DataFrame(scenarios).tail())
```

### Feedback Loop Analysis

Every `run()` scores each feedback loop's contribution to model behavior at
every timestep, using the Loops that Matter method (Schoenberg, Eberlein &
Rahmandad, 2020):

- `run.loops` -- every feedback loop, with runtime polarity and a
  `behavior_time_series`: the loop's signed share, in [-1, 1], of the total
  loop activity in its part of the model at each timestep.
- `loop.average_importance()` / `loop.max_importance()` -- reductions of
  the absolute value of that series, in [0, 1]. Comparable across loops in
  the same cycle partition (see below), so they rank loops by dominance.
- `run.dominant_periods` -- contiguous intervals in which one set of
  same-polarity loops explains the majority of behavior.
- `run.ltm_mode` -- `"exhaustive"`, `"discovery"`, or `"disabled"` (see the
  next section).

```python
run = model.run()

most = max(run.loops, key=lambda loop: loop.average_importance() or 0)
print(f"most influential: {most.id}: {' -> '.join(most.variables)}")

for period in run.dominant_periods:
    print(f"t=[{period.start_time:g}, {period.end_time:g}]: {period.dominant_loops}")
```

Importance is normalized within a loop's *cycle partition* -- a group of
stocks connected by feedback. Models with several independent feedback
subsystems have several partitions, and scores are only comparable between
loops in the same one (`loop.partition` indexes `model.loop_partitions`); a
loop alone in its partition scores exactly 1 by construction.

For loop structure without simulating, use `model.loops`: the same loops
with structural polarity and no behavior series.

#### Loop Polarity

- **R (reinforcing)** -- amplifies change: every loop score positive.
- **B (balancing)** -- counteracts change: every loop score negative.
- **Rux / Bux (mostly reinforcing / mostly balancing)** -- mixed-sign
  scores with one side dominating; `loop.polarity_confidence` carries the
  ratio `|r - |b|| / (r + |b|)` (Schoenberg & Eberlein, 2020), and
  classification requires confidence >= 0.99.
- **U (undetermined)** -- mixed-sign scores with no dominant side.

```python
from simlin import LoopPolarity

reinforcing = [l for l in run.loops if l.polarity == LoopPolarity.REINFORCING]
balancing = [l for l in run.loops if l.polarity == LoopPolarity.BALANCING]
```

### Loop Discovery for Large Models

The surfaces above enumerate every feedback loop, which is only tractable
for smaller models. Past a size gate the engine switches to *discovery*
mode: `run()` emits a `RuntimeWarning`, `run.ltm_mode` reports
`"discovery"`, and `run.loops` is empty. For these models, call
`Model.analyze()` -- an explicit, timeout-guarded search for the loops that
drive behavior:

<!-- pysimlin-test: skip -->
```python
model = simlin.load("wrld3-03.mdl")   # World3: 311 variables
analysis = model.analyze(timeout=30.0)

if analysis.truncated:
    print("timeout elapsed; results are partial")

if not analysis.enumeration_complete:
    print("loops below are a sample, not every loop the model has")

for loop in analysis.loops[:3]:
    chain = " -> ".join(loop.variables[:3])
    print(f"{loop.id} ({loop.polarity}) importance {loop.average_importance():.3f}: {chain} ...")
```

Output for World3, the *Limits to Growth* model
([`test/metasd/WRLD3-03/wrld3-03.mdl`](https://github.com/bpowers/simlin/tree/main/test/metasd/WRLD3-03)
in the Simlin repository), where discovery finds 200 loops across 15
feedback-coupled stocks in under a second:

```
b51 (B) importance 0.052: population_0_to_14 -> maturation_14_to_15 -> population_15_to_44 ...
r55 (R) importance 0.033: persistent_pollution -> persistent_pollution_index -> land_fertility_degredation_rate ...
b24 (B) importance 0.031: persistent_pollution -> persistent_pollution_index -> land_fertility_degredation_rate ...
```

`analysis.loops` is ranked most-important-first, except that loops with no
competition in their cycle partition sort last (their relative score is 1
by construction and says nothing). `analysis.partitions` describes the
partitions; on models with more than one, compare loops within a partition,
as above. `analysis.dominant_periods` is also available, but with hundreds
of loops the per-timestep dominant sets are fine-grained -- ranking by
`average_importance()` and reading the top loops' variable chains is
usually the more legible summary.

Three fields say how complete that list is.
`analysis.enumeration_complete` distinguishes an EXACT analysis -- the engine
enumerated every loop that could ever score and picked from that whole set --
from a SAMPLED one, where a budget cut the enumeration short and a
shortest-path search stood in for it; a loop missing from a sampled analysis
is not evidence the model lacks it. `analysis.universe_loops` is how large
that enumerated universe was (`None` when sampled, which is a different claim
from `0`), and `analysis.retained_loops` is how many loops cleared the
importance filter before the report cap. On a model like World3 both run to
thousands against the 200 loops reported, which is what tells you the list is
a ranked prefix rather than the whole story.

Two details worth knowing: the `timeout` bounds only the discovery sweep --
the model is first compiled and simulated with loop instrumentation, and
that time is not counted against it; and if there are specific loops you
always want scored, pin them by name with `patch.set_loop_name()` inside
`model.edit()`.

### Link Scores

Loop scores are built from link scores: with loop analysis enabled, every
causal link is scored at every timestep. `average_relative_score()` gives
the fraction of the target's change attributable to that input, in
[-1, 1] -- use it to rank the inputs of one variable:

```python
with model.simulate(enable_ltm=True) as sim:
    sim.run_to_end()
    links = sim.get_links()

inputs = [ln for ln in links if ln.to_var == "net_growth"]
inputs.sort(key=lambda ln: abs(ln.average_relative_score() or 0), reverse=True)
for ln in inputs:
    print(f"{ln}: {ln.average_relative_score():.2f}")
```

The normalization is per target, so relative scores answer "which of this
variable's inputs matters most" -- not "which link matters most globally".
A link into a target with a single scored input reads 1 by construction.
(The raw `link.score` series is normalized differently for each target and
is not comparable across targets at all.)

### Model Export

```python
xmile_bytes = project.to_xmile()        # XMILE XML
json_bytes = project.serialize_json()   # Simlin JSON
```

Write the bytes to a file to save the model (`.stmx` for XMILE).

### Error Handling

`project.get_errors()` reports compilation problems for the whole project;
`model.check()` presents the same information per model.

```python
from simlin import SimlinImportError, ErrorCode

for error in project.get_errors():
    print(f"{error.code.name} in {error.model_name}/{error.variable_name}: {error.message}")
```

Loading a malformed file raises `SimlinImportError`:

<!-- pysimlin-test: skip -->
```python
try:
    model = simlin.load("model.stmx")
except SimlinImportError as e:
    print(f"import failed: {e}")
    if e.code == ErrorCode.XML_DESERIALIZATION:
        print("invalid XML")
```

## Complete Example

Load a model from a file and compare a policy against the baseline:

<!-- pysimlin-test: skip -->
```python
import matplotlib.pyplot as plt
import simlin

model = simlin.load("population_model.stmx")
baseline = model.base_case
policy = model.run(overrides={"birth_rate": 0.03})

fig, ax = plt.subplots()
ax.plot(baseline.results.index, baseline.results["population"], label="baseline")
ax.plot(policy.results.index, policy.results["population"], label="policy")
ax.set_xlabel("time")
ax.set_ylabel("population")
ax.legend()
plt.show()
```

## License

Apache License 2.0

## Development

pysimlin is developed in the
[Simlin monorepo](https://github.com/bpowers/simlin) (`src/pysimlin`).