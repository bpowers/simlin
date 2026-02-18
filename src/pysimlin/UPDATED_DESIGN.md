# pysimlin API Design

## Overview

This document describes the redesigned pysimlin API, which provides a Python interface to the simlin system dynamics modeling engine. The primary goal is to create an intuitive, composable API that integrates seamlessly with the Python data science ecosystem (pandas, numpy, matplotlib, seaborn, plotly) while exposing the unique capabilities of system dynamics modeling, particularly feedback loop analysis.

## Background and Motivation

### Primary Use Cases

pysimlin is designed primarily for:

1. **AI agents** analyzing system dynamics models as part of model understanding, critique, and calibration workflows
2. **Expert modelers and data scientists** performing rigorous analysis, parameter estimation, and scenario comparison
3. **Domain experts** exploring model behavior and validating model structure against empirical data

The API should optimize for these use cases, not for building interactive GUIs or creating custom visualization frameworks.

### Design Philosophy

**Core Principle: Provide rich data, leverage standard tools**

We take a strong stance: **pysimlin should do what only pysimlin can do, and delegate everything else to the broader ecosystem.**

What pysimlin uniquely provides:
- Loading and compiling system dynamics models from various formats (XMILE, SDAI JSON, native JSON)
- Running efficient simulations of stock-flow models
- Computing feedback loop dominance analysis (Loops That Matter / LTM)
- Structural analysis of models (stocks, flows, feedback loops)

What pysimlin should NOT reimplement:
- Plotting (matplotlib, seaborn, plotly do this better)
- Statistical analysis (pandas, numpy, scipy do this better)
- Data manipulation (pandas does this better)
- Comparison/aggregation operations (pandas does this better)

This philosophy leads to a clean separation:
- pysimlin returns standard Python data structures (pandas DataFrames, numpy arrays, lists, dicts)
- Users apply standard tools to these structures
- AI agents can use their existing knowledge of pandas/matplotlib without learning custom APIs

### Key Design Decisions

1. **simlin.load() returns Model** - Always returns the main/default model, even for multi-model projects. Other models accessible via `model.project.get_model(name)`. This optimizes for the 95% case (single model files).

2. **model.base_case property** - Eagerly-evaluated simulation results with default parameters, computed during load(). Makes exploration trivial: `model.base_case.results['population'].plot()`

3. **Separate Stock/Flow/Aux classes** - Different variable types have different semantics and structure (e.g., stocks have inflows/outflows). Encoding this in the type system is more pythonic than a single Variable class with a `type` field.

4. **Run class for simulation results** - Bundles time series data (`results`), loop analysis (`loops`, `dominant_periods`), and metadata (`params`, `time_spec`) together. This makes provenance clear.

5. **No custom plotting/comparison APIs** - Return DataFrames and let users use pandas/matplotlib. This reduces API surface area, increases flexibility, and leverages existing knowledge.

6. **Explicit over convenient** - No `run['population']` shorthand. Just `run.results['population']`. One clear way to access data, consistent with accessing other run attributes.

7. **Sim class for gaming only** - Low-level step-by-step simulation for gaming applications (where you modify variables during simulation). For batch analysis, use `model.run()`.

## API Specification

### Top-Level Functions

```python
def load(path: str) -> Model:
    """
    Load a system dynamics model from file.

    Supports XMILE (.stmx, .xmile), SDAI JSON, and native JSON formats.
    Always returns the default/main model. For multi-model projects,
    access other models via model.project.get_model(name).

    Args:
        path: Path to model file

    Returns:
        The main/default model

    Example:
        >>> model = simlin.load("population.stmx")
        >>> print(f"Model has {len(model.stocks)} stocks")
        >>> model.base_case.results['population'].plot()
    """
```

### Model Structure Classes

```python
@dataclass(frozen=True)
class TimeSpec:
    """Time specification for simulation."""
    start: float
    stop: float
    dt: float
    units: Optional[str] = None


@dataclass(frozen=True)
class GraphicalFunctionScale:
    """Scale for graphical function axes."""
    min: float
    max: float


@dataclass(frozen=True)
class GraphicalFunction:
    """
    A graphical/table function (lookup table).

    Represents a piecewise function defined by data points.
    Used in table functions and WITH LOOKUP.
    """

    x_points: Optional[tuple[float, ...]]
    """X coordinates. If None, uses implicit x scale from 0 to len(y_points)-1"""

    y_points: tuple[float, ...]
    """Y coordinates (function values)"""

    x_scale: GraphicalFunctionScale
    """X axis scale"""

    y_scale: GraphicalFunctionScale
    """Y axis scale"""

    kind: str = "continuous"
    """Interpolation: 'continuous', 'discrete', or 'extrapolate'"""
```

```python
@dataclass(frozen=True)
class Stock:
    """
    A stock (level, accumulation) variable.

    Immutable - modifying attributes will not change the underlying model.
    """

    name: str
    """Variable name"""

    initial_equation: str
    """Initial value expression"""

    inflows: tuple[str, ...]
    """Names of flows that increase this stock"""

    outflows: tuple[str, ...]
    """Names of flows that decrease this stock"""

    units: Optional[str] = None
    """Units (if specified)"""

    documentation: Optional[str] = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    non_negative: bool = False
    """Whether this stock is constrained to be non-negative"""
```

```python
@dataclass(frozen=True)
class Flow:
    """
    A flow (rate) variable.

    Immutable - modifying attributes will not change the underlying model.
    """

    name: str
    """Variable name"""

    equation: str
    """Flow rate expression"""

    units: Optional[str] = None
    """Units (if specified)"""

    documentation: Optional[str] = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    non_negative: bool = False
    """Whether this flow is constrained to be non-negative"""

    graphical_function: Optional[GraphicalFunction] = None
    """Graphical/table function if this uses WITH LOOKUP"""
```

```python
@dataclass(frozen=True)
class Aux:
    """
    An auxiliary (intermediate calculation) variable.

    Immutable - modifying attributes will not change the underlying model.
    """

    name: str
    """Variable name"""

    equation: str
    """Equation defining this variable"""

    active_initial: Optional[str] = None
    """Active initial equation (Vensim ACTIVE INITIAL)"""

    units: Optional[str] = None
    """Units (if specified)"""

    documentation: Optional[str] = None
    """Documentation/comments"""

    dimensions: tuple[str, ...] = ()
    """Dimension names for arrayed variables (empty if scalar)"""

    graphical_function: Optional[GraphicalFunction] = None
    """Graphical/table function if this uses WITH LOOKUP"""
```

```python
@dataclass(frozen=True)
class Loop:
    """
    Represents a feedback loop.

    When obtained from Model.loops (structural), behavior_time_series is None.
    When obtained from Run.loops (behavioral), includes time series data.
    Immutable - modifying attributes will not change the model.
    """

    id: str
    """Loop identifier (e.g., 'R1', 'B2')"""

    variables: tuple[str, ...]
    """Variables in this loop"""

    polarity: Literal["reinforcing", "balancing"]
    """Loop polarity"""

    behavior_time_series: Optional[NDArray[np.float64]] = None
    """
    Loop's contribution to model behavior over time.
    None for structural loops, populated for loops from Run objects.
    """

    def average_importance(self) -> Optional[float]:
        """
        Average importance across simulation.

        Returns None if behavior_time_series is not available.

        Example:
            >>> important_loops = [l for l in run.loops if l.average_importance() > 0.1]
        """
        if self.behavior_time_series is None or len(self.behavior_time_series) == 0:
            return None
        return float(np.mean(np.abs(self.behavior_time_series)))

    def max_importance(self) -> Optional[float]:
        """
        Maximum importance during simulation.

        Returns None if behavior_time_series is not available.
        """
        if self.behavior_time_series is None or len(self.behavior_time_series) == 0:
            return None
        return float(np.max(np.abs(self.behavior_time_series)))
```

### Model Class

```python
class Model:
    """
    Represents a system dynamics model.

    This is the primary interface for working with models. Use simlin.load()
    to create Model instances.
    """

    # -------------------------------------------------------------------------
    # Structural properties (no simulation required)
    # -------------------------------------------------------------------------

    @property
    def name(self) -> str:
        """Model name"""

    @property
    def stocks(self) -> tuple[Stock, ...]:
        """Stock variables (immutable tuple)"""

    @property
    def flows(self) -> tuple[Flow, ...]:
        """Flow variables (immutable tuple)"""

    @property
    def auxs(self) -> tuple[Aux, ...]:
        """Auxiliary variables (immutable tuple)"""

    @property
    def variables(self) -> tuple[Union[Stock, Flow, Aux], ...]:
        """
        All variables in the model.

        Returns stocks + flows + auxs combined as an immutable tuple.
        """

    @property
    def loops(self) -> tuple[Loop, ...]:
        """
        Structural feedback loops (no behavior data).

        Returns an immutable tuple of Loop objects.
        For loops with behavior time series, use model.base_case.loops
        or run.loops from a specific simulation run.
        """

    @property
    def time_spec(self) -> TimeSpec:
        """Time bounds and step size"""

    # -------------------------------------------------------------------------
    # Simulation and analysis
    # -------------------------------------------------------------------------

    @property
    def base_case(self) -> Run:
        """
        Simulation results with default parameters.

        Computed eagerly during model loading. This represents the
        model's baseline behavior using parameters from the model file.

        Example:
            >>> model = simlin.load("model.stmx")
            >>> model.base_case.results['population'].plot()
            >>> print(model.base_case.results['population'].iloc[-1])
            >>> for period in model.base_case.dominant_periods:
            ...     print(f"{period.start_time}-{period.end_time}: {period.dominant_loops}")
        """

    def run(
        self,
        overrides: Optional[Dict[str, float]] = None,
        time_range: Optional[tuple[float, float]] = None,
        dt: Optional[float] = None,
        analyze_loops: bool = True,
    ) -> Run:
        """
        Run simulation with optional variable overrides.

        Args:
            overrides: Override values for any model variables (by name)
            time_range: (start, stop) time bounds (uses model defaults if None)
            dt: Time step (uses model default if None)
            analyze_loops: Whether to compute loop dominance analysis (LTM)

        Returns:
            Run object with results and analysis

        Example:
            >>> # Single run with overrides
            >>> run = model.run(overrides={'birth_rate': 0.03})
            >>> run.results['population'].plot()
            >>>
            >>> # Compare scenarios using pandas
            >>> baseline = model.base_case
            >>> policy = model.run(overrides={'tax_rate': 0.3})
            >>> comparison = pd.DataFrame({
            ...     'baseline': baseline.results['gdp'],
            ...     'policy': policy.results['gdp']
            ... })
            >>> comparison.plot()
            >>>
            >>> # Scenario sweep
            >>> runs = [model.run(overrides={'rate': r}) for r in np.linspace(0.01, 0.05, 20)]
            >>> final_values = [r.results['population'].iloc[-1] for r in runs]
            >>> plt.plot(np.linspace(0.01, 0.05, 20), final_values)
        """

    def simulate(
        self,
        overrides: Optional[Dict[str, float]] = None,
        enable_ltm: bool = False,
    ) -> Sim:
        """
        Create low-level simulation for step-by-step execution.

        Use this for gaming applications where you need to inspect state
        and modify variables during simulation. For batch analysis, use
        model.run() instead.

        Args:
            overrides: Variable value overrides
            enable_ltm: Enable Loops That Matter tracking

        Returns:
            Sim context manager for step-by-step execution

        Example:
            >>> with model.simulate() as sim:
            ...     sim.run_to_end()
            ...     run = sim.get_run()
            ...     # Or for interactive gaming:
            ...     # sim.run_to(50)
            ...     # if sim.get_value('inventory') < 10:
            ...     #     sim.set_value('production_rate', 1.5)
            ...     # sim.run_to_end()
        """

    # -------------------------------------------------------------------------
    # Utilities
    # -------------------------------------------------------------------------

    def check(self) -> tuple[ModelIssue, ...]:
        """
        Check model for common issues.

        Returns tuple of warnings/errors about model structure, equations, etc.

        Example:
            >>> issues = model.check()
            >>> for issue in issues:
            ...     print(f"{issue.severity}: {issue.message}")
        """

    def check_units(self) -> tuple[UnitIssue, ...]:
        """
        Check dimensional consistency of equations.

        Returns tuple of unit issues found.

        Example:
            >>> issues = model.check_units()
            >>> errors = [i for i in issues if i.expected_units != i.actual_units]
        """

    def explain(self, variable: str) -> str:
        """
        Get human-readable explanation of a variable.

        Args:
            variable: Variable name

        Returns:
            Textual description of what defines/drives this variable

        Example:
            >>> print(model.explain('population'))
            "population is a stock increased by births and decreased by deaths"
        """

    # -------------------------------------------------------------------------
    # Multi-model access
    # -------------------------------------------------------------------------

    @property
    def project(self) -> Project:
        """
        Parent project (for accessing other models in multi-model files).

        Example:
            >>> main_model = simlin.load("model.stmx")
            >>> sub_model = main_model.project.get_model("SubModule")
        """
```

### Run Class

```python
@dataclass(frozen=True)
class DominantPeriod:
    """Time period where specific loops dominate model behavior."""

    dominant_loops: tuple[str, ...]
    """Loop IDs that dominate during this period"""

    start_time: float
    """Period start time"""

    end_time: float
    """Period end time"""

    def duration(self) -> float:
        """Duration of this period"""
        return self.end_time - self.start_time

    def contains_loop(self, loop_id: str) -> bool:
        """Check if a loop dominates during this period"""
        return loop_id in self.dominant_loops
```

```python
class Run:
    """
    Results and analysis from a single simulation run.

    This is returned by model.run() and model.base_case. It contains
    time series data, loop analysis, and metadata about the run.

    Use standard pandas/numpy operations for analysis and visualization.
    """

    # -------------------------------------------------------------------------
    # Data access
    # -------------------------------------------------------------------------

    @property
    def results(self) -> pd.DataFrame:
        """
        Time series results as a pandas DataFrame.

        Index is simulation time. Columns are variable names.
        For arrayed variables, columns are named like "var[element]".

        Use standard pandas methods for analysis and visualization.

        Example:
            >>> # Basic plotting
            >>> run.results['population'].plot()
            >>>
            >>> # Multiple variables
            >>> run.results[['births', 'deaths']].plot()
            >>>
            >>> # Statistics
            >>> print(run.results['population'].describe())
            >>>
            >>> # Final value
            >>> final_pop = run.results['population'].iloc[-1]
            >>>
            >>> # Export
            >>> run.results.to_csv('results.csv')
            >>>
            >>> # Filtering/slicing
            >>> after_1950 = run.results[run.results.index > 1950]
        """

    # -------------------------------------------------------------------------
    # Loop analysis
    # -------------------------------------------------------------------------

    @property
    def loops(self) -> tuple[Loop, ...]:
        """
        Feedback loops with behavior time series.

        Each Loop has behavior_time_series populated showing the loop's
        contribution to model behavior at each time step.

        Returns empty tuple if analyze_loops=False was used.

        Example:
            >>> # Find most important loop
            >>> most_important = max(run.loops, key=lambda l: l.average_importance())
            >>> print(f"Most important: {most_important.id}")
            >>>
            >>> # Plot loop behavior over time
            >>> import matplotlib.pyplot as plt
            >>> for loop in run.loops:
            ...     plt.plot(run.results.index, loop.behavior_time_series, label=loop.id)
            >>> plt.legend()
            >>> plt.xlabel('Time')
            >>> plt.ylabel('Loop Importance')
            >>>
            >>> # Filter to important loops
            >>> important = [l for l in run.loops if l.average_importance() > 0.1]
        """

    @property
    def dominant_periods(self) -> tuple[DominantPeriod, ...]:
        """
        Time periods where specific loops dominate.

        Uses greedy algorithm to identify which loops explain the most
        variance in model behavior during each period.

        Returns empty tuple if analyze_loops=False was used.

        Example:
            >>> # Print regime shifts
            >>> for period in run.dominant_periods:
            ...     print(f"t=[{period.start_time}, {period.end_time}]: {period.dominant_loops}")
            >>>
            >>> # Find longest regime
            >>> longest = max(run.dominant_periods, key=lambda p: p.duration())
            >>>
            >>> # Visualize regime changes
            >>> import matplotlib.pyplot as plt
            >>> fig, ax = plt.subplots()
            >>> ax.plot(run.results.index, run.results['population'])
            >>> for period in run.dominant_periods:
            ...     ax.axvspan(period.start_time, period.end_time,
            ...                alpha=0.2, label=','.join(period.dominant_loops))
            >>> ax.legend()
        """

    # -------------------------------------------------------------------------
    # Metadata
    # -------------------------------------------------------------------------

    @property
    def overrides(self) -> Dict[str, float]:
        """
        Variable overrides used for this run.

        Empty dict if no overrides were specified.

        Example:
            >>> run = model.run(overrides={'birth_rate': 0.03})
            >>> print(f"Overrides: {run.overrides}")
        """

    @property
    def time_spec(self) -> TimeSpec:
        """
        Time specification used for this run.

        Example:
            >>> print(f"Simulated from {run.time_spec.start} to {run.time_spec.stop}")
            >>> print(f"Time step: {run.time_spec.dt}")
        """
```

### Sim Class (Low-Level)

```python
class Sim:
    """
    Low-level simulation for step-by-step execution.

    Use as a context manager for gaming applications. For batch analysis,
    use Model.run() instead.
    """

    @property
    def time(self) -> float:
        """Current simulation time"""

    def run_to_end(self) -> None:
        """Run simulation to completion"""

    def run_to(self, time: float) -> None:
        """Run simulation to specified time"""

    def reset(self) -> None:
        """Reset simulation to initial state"""

    def get_value(self, variable: str) -> float:
        """Get current value of a variable"""

    def set_value(self, variable: str, value: float) -> None:
        """Set current value of a variable (for gaming)"""

    def get_series(self, variable: str) -> NDArray[np.float64]:
        """
        Get time series for a variable (all steps up to current time).

        Example:
            >>> with model.simulate() as sim:
            ...     sim.run_to_end()
            ...     time = sim.get_series('time')
            ...     population = sim.get_series('population')
            ...     plt.plot(time, population)
        """

    def get_run(self) -> Run:
        """
        Get simulation results as a Run object.

        Loop analysis is included if the simulation was created with enable_ltm=True.
        Can be called before run_to_end() to get partial results.

        Returns:
            Run object with results and analysis

        Example:
            >>> with model.simulate(enable_ltm=True) as sim:
            ...     sim.run_to_end()
            ...     run = sim.get_run()
            ...     print(run.dominant_periods)
        """

    def __enter__(self) -> Sim:
        return self

    def __exit__(self, *args) -> None:
        pass
```

### Project Class

```python
class Project:
    """
    A system dynamics project, potentially with multiple models.

    Usually accessed via model.project rather than directly.
    """

    def get_model(self, name: Optional[str] = None) -> Model:
        """
        Get a model by name.

        Args:
            name: Model name (uses main model if None)

        Returns:
            The requested model
        """

    @property
    def models(self) -> tuple[Model, ...]:
        """All models in this project (immutable tuple)"""

    @property
    def main_model(self) -> Model:
        """The main/default model"""
```

### Diagnostics

```python
@dataclass
class ModelIssue:
    """An issue found during model checking."""

    severity: Literal["error", "warning", "info"]
    message: str
    variable: Optional[str] = None
    suggestion: Optional[str] = None
```

```python
@dataclass
class UnitIssue:
    """A dimensional analysis issue."""

    variable: str
    message: str
    expected_units: Optional[str] = None
    actual_units: Optional[str] = None
```

## Usage Examples

### Basic Exploration

```python
import simlin

# Load and explore structure
model = simlin.load("population.stmx")
print(f"Model: {model.name}")
print(f"Stocks: {[s.name for s in model.stocks]}")
print(f"Flows: {[f.name for f in model.flows]}")

# View base case results
model.base_case.results['population'].plot()
print(f"Final population: {model.base_case.results['population'].iloc[-1]}")
```

### Scenario Comparison

```python
import pandas as pd
import matplotlib.pyplot as plt

model = simlin.load("economy.stmx")

# Run scenarios
baseline = model.base_case
low_tax = model.run(overrides={'tax_rate': 0.1})
high_tax = model.run(overrides={'tax_rate': 0.3})

# Compare using pandas
comparison = pd.DataFrame({
    'baseline': baseline.results['gdp'],
    'low_tax': low_tax.results['gdp'],
    'high_tax': high_tax.results['gdp']
})

comparison.plot()
plt.title('GDP Under Different Tax Rates')
plt.ylabel('GDP')
plt.xlabel('Time')
plt.show()

# Statistical comparison
print(comparison.describe())
print(f"\nFinal values:")
print(comparison.iloc[-1])
```

### Sensitivity Analysis

```python
import numpy as np
import matplotlib.pyplot as plt

model = simlin.load("sir_model.stmx")

# Sweep infection rate
infection_rates = np.linspace(0.1, 0.5, 20)
runs = [model.run(overrides={'infection_rate': r}) for r in infection_rates]

# Extract peak infections
peak_infections = [r.results['infected'].max() for r in runs]

# Visualize
plt.plot(infection_rates, peak_infections)
plt.xlabel('Infection Rate')
plt.ylabel('Peak Infections')
plt.title('Sensitivity to Infection Rate')
plt.grid(True)
plt.show()
```

### Loop Dominance Analysis

```python
model = simlin.load("business_cycle.stmx")
run = model.base_case

# Find most important loops
most_important = max(run.loops, key=lambda l: l.average_importance())
print(f"Most important loop: {most_important.id} ({most_important.polarity})")
print(f"Variables: {most_important.variables}")
print(f"Average importance: {most_important.average_importance():.3f}")

# Plot loop behavior over time
import matplotlib.pyplot as plt
fig, ax = plt.subplots(figsize=(12, 6))
for loop in run.loops:
    ax.plot(run.results.index, loop.behavior_time_series, label=loop.id)
ax.legend()
ax.set_xlabel('Time')
ax.set_ylabel('Loop Importance')
ax.set_title('Feedback Loop Contributions Over Time')
plt.show()

# Identify regime shifts
print("\nDominant periods:")
for period in run.dominant_periods:
    print(f"  t=[{period.start_time:.1f}, {period.end_time:.1f}]: {period.dominant_loops}")
```

### Visualizing Regimes

```python
import matplotlib.pyplot as plt

model = simlin.load("climate_model.stmx")
run = model.base_case

# Plot key variable with regime backgrounds
fig, ax = plt.subplots(figsize=(14, 6))
ax.plot(run.results.index, run.results['temperature'], linewidth=2, color='black')

# Color background by dominant loop
colors = {'R1': 'red', 'B1': 'blue', 'R2': 'orange', 'B2': 'green'}
for period in run.dominant_periods:
    loop_label = ','.join(period.dominant_loops)
    color = colors.get(period.dominant_loops[0], 'gray')
    ax.axvspan(period.start_time, period.end_time,
               alpha=0.2, color=color, label=loop_label)

ax.set_xlabel('Time (years)')
ax.set_ylabel('Temperature (°C)')
ax.set_title('Temperature Change with Dominant Loop Regimes')
ax.legend(loc='upper left')
plt.show()
```

### Monte Carlo Analysis

```python
import numpy as np
import pandas as pd
from scipy import stats

model = simlin.load("supply_chain.stmx")

# Define parameter distributions
n_runs = 1000
override_samples = {
    'lead_time': np.random.uniform(2, 8, n_runs),
    'order_threshold': np.random.uniform(50, 150, n_runs),
}

# Run Monte Carlo
runs = []
for i in range(n_runs):
    overrides = {k: v[i] for k, v in override_samples.items()}
    run = model.run(overrides=overrides, analyze_loops=False)  # faster without loop analysis
    runs.append(run)

# Aggregate results
final_inventories = [r.results['inventory'].iloc[-1] for r in runs]
final_costs = [r.results['total_cost'].iloc[-1] for r in runs]

# Analyze
print(f"Final inventory: {np.mean(final_inventories):.1f} ± {np.std(final_inventories):.1f}")
print(f"Final cost: {np.mean(final_costs):.1f} ± {np.std(final_costs):.1f}")

# Visualize uncertainty
import matplotlib.pyplot as plt
fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 4))

ax1.hist(final_inventories, bins=30, alpha=0.7)
ax1.set_xlabel('Final Inventory')
ax1.set_ylabel('Frequency')
ax1.set_title('Inventory Distribution')

ax2.hist(final_costs, bins=30, alpha=0.7, color='orange')
ax2.set_xlabel('Final Cost')
ax2.set_ylabel('Frequency')
ax2.set_title('Cost Distribution')

plt.tight_layout()
plt.show()
```

### Gaming Example

```python
model = simlin.load("inventory_management.stmx")

# Interactive simulation with interventions
with model.simulate() as sim:
    # Run first half
    sim.run_to(50)

    # Check state and intervene
    inventory = sim.get_value('inventory')
    if inventory < 20:
        print(f"t={sim.time:.1f}: Low inventory ({inventory:.1f}), increasing production")
        sim.set_value('production_rate', 15)
    elif inventory > 80:
        print(f"t={sim.time:.1f}: High inventory ({inventory:.1f}), decreasing production")
        sim.set_value('production_rate', 5)

    # Run to completion
    sim.run_to_end()

    # Get results for analysis
    run = sim.get_run()

# Analyze intervention effects
run.results[['inventory', 'production_rate']].plot(secondary_y='production_rate')
```

## Implementation Notes

### Immutability

All data classes representing model structure are immutable (frozen dataclasses):
- `Stock`, `Flow`, `Aux`: Cannot be modified after creation
- `GraphicalFunction`, `GraphicalFunctionScale`: Immutable data
- `TimeSpec`, `DominantPeriod`: Immutable metadata
- Use tuples instead of lists for all sequence fields to enforce immutability

**Rationale**: Modifying these objects should not affect the underlying model. Models are loaded once and remain constant. If users want to modify a model, they should create a new one (future feature) rather than mutating existing objects.

**Implementation**: Use `@dataclass(frozen=True)` and tuple types for sequences.

### Eager Evaluation of base_case

The `model.base_case` property contains a Run object computed during `simlin.load()`:
1. After loading and parsing the model file
2. Create a simulation with default parameters and `enable_ltm=True`
3. Run the simulation to completion
4. Compute loop dominance analysis
5. Create a Run object and store it in the Model
6. Return the Model to the user

This means the base case is always available immediately when accessing `model.base_case`, with no computation needed. Model objects are immutable after loading.

### Loop Dominance Algorithm

The loop dominance analysis (for `Run.dominant_periods`) should implement the greedy algorithm from the Go implementation in `engine/model_impl.go:calculateDominantPeriods`:

1. For each timestep:
   - Sort loops by absolute importance score (descending)
   - Try adding same-polarity loops until combined score ≥ threshold (default 0.5)
   - Try reinforcing loops first, then balancing loops
   - If neither reaches threshold, use the polarity with higher total score

2. Group consecutive timesteps with identical dominant loop sets into DominantPeriod objects

### DataFrame Construction

The `Run.results` DataFrame should:
- Have simulation time as the index (named 'time')
- Have one column per variable
- For arrayed variables (e.g., `population[region]` where region = {urban, rural}):
  - Create separate columns: `population[urban]`, `population[rural]`
  - This enables standard pandas operations like: `results.filter(like='population').sum(axis=1)`

### Variable Name Canonicalization

Variable names in the engine use canonical forms (lowercase, underscores). The API should preserve the original names from the model file where possible, but accept canonical forms in `run.results['var_name']` lookups.

### Type Hints

All public APIs should have complete type hints compatible with mypy strict mode. Use:
- `List`, `Dict`, `Optional`, `Union`, `Literal` from `typing`
- `NDArray` from `numpy.typing`
- `pd.DataFrame`, `pd.Series` from pandas

### Error Handling

Custom exception hierarchy:
```python
class SimlinError(Exception):
    """Base exception for simlin errors"""

class SimlinLoadError(SimlinError):
    """Error loading model file"""

class SimlinCompileError(SimlinError):
    """Error compiling model"""

class SimlinRuntimeError(SimlinError):
    """Error during simulation execution"""
```

Provide helpful error messages:
- "Cannot get loop scores without enabling LTM. Use model.run(analyze_loops=True) or model.simulate(enable_ltm=True)"
- "Variable 'populaton' not found. Did you mean 'population'?" (suggest close matches)

### Performance Considerations

1. **Disable loop analysis for sweeps**: Document that `analyze_loops=False` significantly speeds up batch runs when loop analysis isn't needed

2. **Lazy structural queries**: Properties like `model.stocks` should cache results after first access

### Testing Strategy

1. **Unit tests**: Test each class/method in isolation
2. **Integration tests**: Test full workflows (load → run → analyze)
3. **Example models**: Use existing test models from `test/` directory
4. **Comparison tests**: Verify parity with Go implementation for loop dominance
5. **Documentation tests**: All docstring examples should be valid doctests

### Migration Path

The current pysimlin API should continue to work. Consider:
1. Mark old APIs as deprecated with warnings
2. Provide migration guide showing old → new equivalents
3. Support both APIs for one major version
4. Remove deprecated APIs in next major version

Key mappings:
```python
# Old → New
Project.from_file(path) → simlin.load(path).project
model.new_sim() → model.simulate()
sim.get_results() → sim.get_run().results
```

## Implementation Decisions

### Multi-dimensional Arrays
For multi-dimensional arrayed variables, use comma-separated subscripts in column names:
- `population[region,age_group]` creates columns like `population[urban,young]`, `population[urban,old]`, etc.
- This matches standard mathematical notation and is parseable

### Unit Checking
`check_units()` should be comprehensive - check all equations for dimensional consistency. Return all issues found, users can filter by severity if needed.

### Model.explain()
Use template-based explanations:
- Stocks: "{name} is a stock with initial value {equation}, increased by {inflows}, decreased by {outflows}"
- Flows: "{name} is a flow computed as {equation}"
- Auxs: "{name} is an auxiliary variable computed as {equation}"

Keep it simple and deterministic. Advanced explanation can be added later.

### Sim Specs Precedence
When getting time bounds and dt for a simulation:
1. If the Model has sim_specs defined: use those
2. Otherwise: use the Project-level sim_specs
3. Both `model.run()` and base_case follow this rule

### Project.get_model() Scope
`project.get_model(name)` returns any model defined in the project file, whether it's used as a module or not. This allows inspection of the full model hierarchy.

### Sim.get_run() Behavior
Allow calling `get_run()` before `run_to_end()` - return results for the partial simulation. Useful for debugging and interrupted simulations. Loop analysis is included if the sim was created with `enable_ltm=True`.

## Rationale for Key Decisions

### Why no custom plotting?

**Decision**: Do not provide `run.plot()`, `run.plot_loop_dominance()`, or similar visualization methods.

**Rationale**:
- Users already know matplotlib/seaborn/plotly
- Any custom plotting API will be less flexible than the standard tools
- Reduces API surface area and maintenance burden
- Makes it clear that pysimlin's value is in simulation and analysis, not visualization
- AI agents have deep knowledge of matplotlib; custom APIs require learning and are less reliable

**Alternative considered**: Provide minimal plotting as convenience methods.
**Rejected because**: Convenience quickly becomes limitation. Once we provide basic plotting, users will request customization options, and we'll end up reimplementing matplotlib poorly.

### Why separate Stock/Flow/Aux classes?

**Decision**: Use three separate classes rather than one `Variable` class with a `type` field.

**Rationale**:
- Different variable types have different structure (stocks have inflows/outflows)
- Type system can enforce correctness (can't ask for inflows of a Flow)
- More pythonic - use types to express semantics
- Easier for type checkers and IDEs to provide good support

**Alternative considered**: Single `Variable` class with `type: Literal["stock", "flow", "aux"]` and optional fields.
**Rejected because**: Leads to runtime errors ("AttributeError: 'Variable' object has no attribute 'inflows'") instead of type errors.

### Why no Run.compare()?

**Decision**: Do not provide comparison methods on Run objects.

**Rationale**:
- Comparison is just: `pd.DataFrame({'base': base.results['x'], 'policy': policy.results['x']})`
- pandas provides richer comparison operations than we could build
- Reduces API surface, increases flexibility
- Users/agents already know pandas

**Alternative considered**: Provide `run1.compare(run2)` returning a `Comparison` object with difference metrics.
**Rejected because**: Adds complexity without adding value. Pandas does this better.

### Why base_case instead of base_run or baseline?

**Decision**: Use `model.base_case` as the property name.

**Rationale**:
- "Base case" is standard SD terminology
- Clear that this is the default/reference scenario
- Natural to say "compare policy to base case"
- "baseline" often implies empirical data rather than model output
- "base_run" is more generic, less evocative

**Alternative considered**: `model.baseline`, `model.default_run`
**Selected base_case**: Most natural for SD practitioners and domain experts.

### Why is analyze_loops True by default?

**Decision**: `model.run(analyze_loops=True)` by default, must explicitly disable.

**Rationale**:
- Loop analysis is pysimlin's unique value proposition
- Most users want loop analysis (otherwise why use simlin vs basic ODE solver?)
- Making it opt-in would lead to confusion ("why don't I have dominant_periods?")
- Performance cost is acceptable for most models
- Advanced users can disable for batch runs where performance matters

**Alternative considered**: `analyze_loops=False` by default, opt-in for analysis.
**Rejected because**: Makes the common case (want loop analysis) require extra code.

### Why Run instead of Result/Simulation/Execution?

**Decision**: Name the class returned by `model.run()` as `Run`.

**Rationale**:
- Natural: "I ran the model and got a run"
- Short and simple
- Common term in both SD and data science
- Works well grammatically: "the run shows...", "compare runs"

**Alternatives considered**:
- `Result`: Too generic, doesn't convey simulation aspect
- `Simulation`: Conflicts with `Sim` class for low-level gaming
- `Execution`: Too formal/computer-sciency
- `ModelRun`: Too verbose
- `Outcome`: Good but less familiar to SD community

**Selected Run**: Best balance of clarity, brevity, and domain fit.

## Composability with Data Science Ecosystem

The key insight of this design is that **pysimlin should be a data source, not a data analysis framework**.

### Integration with pandas

```python
import pandas as pd

# Multiple runs → DataFrame
runs = [model.run(params={'rate': r}) for r in range(10)]
df = pd.DataFrame({
    f'run_{i}': run.results['population']
    for i, run in enumerate(runs)
})

# Standard pandas operations
print(df.describe())
print(df.corr())
df.plot()

# Join with empirical data
empirical = pd.read_csv('data.csv', index_col='year')
comparison = pd.DataFrame({
    'modeled': model.base_case.results['gdp'],
    'empirical': empirical['gdp']
})
comparison.plot()
```

### Integration with numpy

```python
import numpy as np

# Extract to numpy for numerical analysis
results = np.array([run.results['population'].values for run in runs])
mean_trajectory = np.mean(results, axis=0)
std_trajectory = np.std(results, axis=0)

# Loop importance as numpy array
loop = model.base_case.loops[0]
importance = loop.behavior_time_series  # Already numpy array
fft = np.fft.fft(importance)  # Frequency analysis
```

### Integration with matplotlib

```python
import matplotlib.pyplot as plt

# Standard matplotlib
fig, ax = plt.subplots()
for run in runs:
    ax.plot(run.results.index, run.results['population'], alpha=0.3)
ax.set_xlabel('Time')
ax.set_ylabel('Population')
plt.show()

# Subplots
fig, axes = plt.subplots(2, 2, figsize=(12, 10))
model.base_case.results[['births', 'deaths']].plot(ax=axes[0, 0])
model.base_case.results['population'].plot(ax=axes[0, 1])
# etc.
```

### Integration with seaborn

```python
import seaborn as sns

# Prepare data for seaborn
data = []
for i, run in enumerate(runs):
    df = run.results[['population', 'gdp']].copy()
    df['run'] = i
    df['time'] = df.index
    data.append(df)
combined = pd.concat(data)

# Seaborn visualizations
sns.lineplot(data=combined, x='time', y='population', hue='run', alpha=0.5)
sns.scatterplot(data=combined, x='population', y='gdp', hue='run', alpha=0.3)
sns.heatmap(combined.pivot(index='time', columns='run', values='population'))
```

### Integration with plotly

```python
import plotly.express as px
import plotly.graph_objects as go

# Interactive plots
fig = px.line(run.results, y=['population', 'births', 'deaths'])
fig.show()

# Loop dominance with plotly
fig = go.Figure()
for loop in run.loops:
    fig.add_trace(go.Scatter(
        x=run.results.index,
        y=loop.behavior_time_series,
        name=loop.id,
        stackgroup='one'  # Stacked area chart
    ))
fig.update_layout(title='Loop Dominance Over Time')
fig.show()
```

### Integration with scipy/scikit-learn

```python
from scipy.optimize import minimize
from sklearn.metrics import mean_squared_error

# Calibration with scipy
empirical_data = load_empirical_data()

def objective(values_array):
    overrides = dict(zip(var_names, values_array))
    run = model.run(overrides=overrides, analyze_loops=False)
    return mean_squared_error(
        empirical_data['population'],
        run.results['population']
    )

result = minimize(objective, x0=initial_values, method='Nelder-Mead')
best_overrides = dict(zip(var_names, result.x))
calibrated_run = model.run(overrides=best_overrides)
```

The pattern is clear: pysimlin provides rich data structures that integrate seamlessly with existing tools. No custom plotting, no custom statistics, no custom comparison operators - just clean data that works with the entire ecosystem.
