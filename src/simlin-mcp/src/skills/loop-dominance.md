# Loop Dominance Analysis

Simlin uses the Loops That Matter (LTM) algorithm to identify which feedback loops drive model behavior over time. Loop dominance analysis is enabled by default when you call `model.run()`.

> **Prerequisites:** Install the Python package with `pip install pysimlin` (see the `simlin://skills/pysimlin-basics` resource for full setup details). The Python module is imported as `simlin`.

## Getting Loops from a Run

```python
import simlin

model = simlin.load("predator_prey.stmx")
run = model.base_case
```

`run.loops` is a tuple of `Loop` objects, each with behavioral data:

```python
for loop in run.loops:
    print(f"{loop.id} ({loop.polarity}): {loop.variables}")
    print(f"  average importance: {loop.average_importance()}")
    print(f"  max importance: {loop.max_importance()}")
```

### Loop Properties

Each `Loop` has:

- `id` -- Loop identifier string (e.g., `"R1"`, `"B2"`, `"U3"`)
- `variables` -- Tuple of variable names forming the loop
- `polarity` -- One of five `LoopPolarity` variants:
  - `LoopPolarity.REINFORCING` -- every loop-score sample is positive (R)
  - `LoopPolarity.BALANCING` -- every loop-score sample is negative (B)
  - `LoopPolarity.MOSTLY_REINFORCING` -- mixed signs but reinforcing dominates with confidence at or above the 0.99 threshold from Schoenberg & Eberlein (2020) (Rux)
  - `LoopPolarity.MOSTLY_BALANCING` -- mixed signs but balancing dominates above the same threshold (Bux)
  - `LoopPolarity.UNDETERMINED` -- mixed signs with neither side dominant enough to clear the threshold (U)

  `Run.loops` can return any of the five; `Model.loops` (structural-only)
  reports just `REINFORCING`/`BALANCING`/`UNDETERMINED` because the
  `MOSTLY_*` variants require simulated runtime scores.
- `behavior_time_series` -- NumPy array of signed relative loop scores in `[-1, 1]` per timestep (from `Run.loops` and `Model.analyze()`; `None` for structural loops from `Model.loops`). Each value is the loop's share of its cycle partition's total absolute loop score, sign preserved (a balancing loop reads negative), so `abs(...)` is comparable across loops

### Importance Methods

- `loop.average_importance()` -- Mean of absolute behavior_time_series values. Returns `None` if no behavioral data.
- `loop.max_importance()` -- Maximum of absolute behavior_time_series values. Returns `None` if no behavioral data.
- `loop.contains_variable(name)` -- Check if a variable participates in this loop.

## Dominant Periods

`run.dominant_periods` identifies contiguous time intervals where specific loops dominate:

```python
for period in run.dominant_periods:
    print(
        f"t=[{period.start_time}, {period.end_time}]: "
        f"loops={period.dominant_loops}"
    )
```

Each `DominantPeriod` has:

- `dominant_loops` -- Tuple of loop ID strings that dominate during this period
- `start_time` -- Period start time
- `end_time` -- Period end time
- `duration()` -- Returns `end_time - start_time`
- `contains_loop(loop_id)` -- Check if a specific loop dominates during this period

## Plotting Importance Over Time

```python
import matplotlib.pyplot as plt
import numpy as np

model = simlin.load("model.stmx")
run = model.base_case
time = run.results.index.values

fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(10, 8), sharex=True)

# Plot a key stock variable
ax1.plot(time, run.results["population"], color="black")
ax1.set_ylabel("Population")
ax1.set_title("Model Behavior and Loop Dominance")

# Plot importance time series for each loop
for loop in run.loops:
    if loop.behavior_time_series is not None:
        label = f"{loop.id} ({loop.polarity})"
        ax2.plot(time, loop.behavior_time_series, label=label)

ax2.set_ylabel("Loop Importance")
ax2.set_xlabel("Time")
ax2.legend()
plt.tight_layout()
plt.savefig("loop_dominance.png")
```

## Annotating Dominant Periods

Shade the plot background to show which loops dominate in each period:

```python
colors = {"R1": "lightcoral", "B1": "lightblue", "B2": "lightgreen"}

fig, ax = plt.subplots(figsize=(10, 4))
ax.plot(time, run.results["population"])

for period in run.dominant_periods:
    loop_ids = ", ".join(period.dominant_loops)
    color = colors.get(period.dominant_loops[0], "lightyellow")
    ax.axvspan(
        period.start_time,
        period.end_time,
        alpha=0.3,
        color=color,
        label=loop_ids,
    )

ax.set_xlabel("Time")
ax.set_ylabel("Population")
ax.legend()
plt.savefig("dominant_periods.png")
```

## Structural Loops (Without Simulation)

`model.loops` returns structural feedback loops without behavioral data:

```python
for loop in model.loops:
    print(f"{loop.id} ({loop.polarity}): {' -> '.join(loop.variables)}")
    # loop.behavior_time_series is None
    # loop.average_importance() returns None
```

## Disabling Loop Analysis

If you do not need loop dominance and want faster simulation:

```python
run = model.run(analyze_loops=False)
# run.loops will be an empty tuple
# run.dominant_periods will be an empty tuple
```

## Finding the Most Important Loop

```python
if run.loops:
    most_important = max(
        run.loops,
        key=lambda l: l.average_importance() or 0,
    )
    print(f"Most important loop: {most_important.id}")
    print(f"Variables: {most_important.variables}")
```
