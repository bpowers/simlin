# Scenario Analysis

Use `model.run(overrides={...})` to explore how parameter changes affect model behavior.

## Parameter Sweeps

Vary a single parameter across a range and collect results:

```python
import simlin
import pandas as pd
import matplotlib.pyplot as plt

model = simlin.load("population.stmx")

birth_rates = [0.01, 0.02, 0.03, 0.04, 0.05]
results = {}

for rate in birth_rates:
    run = model.run(overrides={"birth_rate": rate})
    results[f"birth_rate={rate}"] = run.results["population"]

comparison = pd.DataFrame(results)
comparison.plot()
plt.xlabel("Time")
plt.ylabel("Population")
plt.title("Sensitivity to Birth Rate")
plt.savefig("birth_rate_sweep.png")
```

## Comparing Scenarios

Run a baseline and one or more policy scenarios, then compare:

```python
baseline = model.run()
policy_a = model.run(overrides={"tax_rate": 0.25})
policy_b = model.run(overrides={"tax_rate": 0.25, "subsidy": 100})

fig, ax = plt.subplots()
ax.plot(baseline.results.index, baseline.results["gdp"], label="Baseline")
ax.plot(policy_a.results.index, policy_a.results["gdp"], label="Tax only")
ax.plot(policy_b.results.index, policy_b.results["gdp"], label="Tax + Subsidy")
ax.legend()
ax.set_xlabel("Time")
ax.set_ylabel("GDP")
plt.savefig("scenario_comparison.png")
```

## Time Range Access

Each run carries the time specification used:

```python
run = model.run()
ts = run.time_spec
print(f"Simulated from {ts.start} to {ts.stop}, dt={ts.dt}")
```

The results DataFrame index is simulation time, so you can slice by time:

```python
# Get results from time 10 to 50
subset = run.results.loc[10:50]
```

## Multi-Variable Sensitivity Analysis

Vary two parameters simultaneously:

```python
import itertools

birth_rates = [0.02, 0.03, 0.04]
death_rates = [0.01, 0.015, 0.02]

results = {}
for br, dr in itertools.product(birth_rates, death_rates):
    run = model.run(overrides={"birth_rate": br, "death_rate": dr})
    final_pop = run.results["population"].iloc[-1]
    results[(br, dr)] = final_pop

for (br, dr), pop in sorted(results.items()):
    print(f"birth_rate={br}, death_rate={dr} -> final population={pop:.0f}")
```

## Extracting Key Metrics

Compute summary statistics across scenarios:

```python
scenarios = {
    "low_growth": {"birth_rate": 0.01},
    "medium_growth": {"birth_rate": 0.03},
    "high_growth": {"birth_rate": 0.05},
}

metrics = []
for name, overrides in scenarios.items():
    run = model.run(overrides=overrides)
    pop = run.results["population"]
    metrics.append({
        "scenario": name,
        "final_value": pop.iloc[-1],
        "peak_value": pop.max(),
        "mean_value": pop.mean(),
    })

summary = pd.DataFrame(metrics)
print(summary.to_string(index=False))
```

## Checking Overrides

The overrides used for a run are available on the `Run` object:

```python
run = model.run(overrides={"birth_rate": 0.05})
print(run.overrides)  # {"birth_rate": 0.05}

baseline = model.base_case
print(baseline.overrides)  # {}
```
