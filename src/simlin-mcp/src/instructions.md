# Simlin MCP Server

Simlin is a system dynamics modeling tool. This server exposes tools for reading, creating, and editing stock-and-flow simulation models.

## Tools

- **ReadModel**: Read a model file and return a JSON snapshot with Loops That Matter (loop dominance) analysis. Accepts `projectPath` (file path) and optional `modelName` (defaults to "main").
- **EditModel**: Apply operations to an existing model. Operations are applied in order; the result includes a refreshed snapshot with loop analysis. Supports `dryRun: true` to preview without writing.
- **CreateModel**: Create a new empty `.sd.json` model file at a given path with optional `simSpecs`.

CRITICAL: this is new software -- `ReadModel` and `CreateModel` are safe, but ONLY use `EditModel` on models in version-controlled projects or newly created models, otherwise we risk corrupting important user files without a clear recovery mechanism.  If a user tells you to edit a non-version-controlled model please explain the risks (we may lose charts and other visual UI elements, and may not handle conveyors or other advanced Stella features correctly), and only if they are OK with the risks proceed.

### EditModel operations

- `upsertStock` -- Create or replace a stock (accumulator). Requires `name` and `initialEquation`. Optional: `units`, `documentation`, `inflows`, `outflows`.
- `upsertFlow` -- Create or replace a flow (rate). Requires `name` and `equation`. Optional: `units`, `documentation`, `graphicalFunction`.
- `upsertAuxiliary` -- Create or replace an auxiliary variable. Requires `name` and `equation`. Optional: `units`, `documentation`, `graphicalFunction`.
- `removeVariable` -- Remove a variable by `name`.
- `setLoopName` -- Assign a human-readable name to a feedback loop. Requires `variables` (list of variable names forming the loop) and `name`. Optional: `description`.

### Typical workflow

1. Use ReadModel to inspect an existing model or start from scratch with CreateModel.
2. Use EditModel with one or more operations to build up the model incrementally.
3. After each EditModel call, check the response for `errors` -- if present, fix them before proceeding.
4. Use ReadModel to review the final state including loop dominance analysis.

## File format support

| Format | Extensions                                 | Read | Edit/Create |
|--------|--------------------------------------------|------|-------------|
| XMILE | `.stmx`, `.xmile`, `.xml`                  | Yes | Yes |
| Native JSON | `.sd.json`, `.sd.json` (with `models` key) | Yes | Yes |
| SD-AI JSON | `.json` (with `variables` key)             | Yes | Yes |
| Vensim | `.mdl`                                     | Yes (import only) | No |

Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, then CreateModel to start a new `.sd.json` file you can edit.

## Equation syntax

Variables use XMILE equation syntax. Key functions and their behavior:

| Function | Description |
|----------|-------------|
| `IF cond THEN a ELSE b` | Conditional (ternary form, not a function call) |
| `SAFEDIV(a, b)` | Division returning 0 when b is 0 |
| `SAFEDIV(a, b, x)` | Division returning x when b is 0 |
| `SMTH1(input, delay_time)` | First-order exponential smooth |
| `SMTH3(input, delay_time)` | Third-order exponential smooth |
| `DELAY1(input, delay_time)` | First-order material delay |
| `DELAY3(input, delay_time)` | Third-order material delay |
| `DELAY(input, delay_time, initial)` | Fixed delay |
| `INIT(expr)` | Capture value at simulation start |
| `PREVIOUS(expr, initial)` | Value from previous timestep |
| `PULSE(volume, first_pulse, interval)` | Pulse input |
| `STEP(height, step_time)` | Step input |
| `RAMP(slope, start_time, end_time)` | Ramp input |
| `MIN(a, b)`, `MAX(a, b)` | Minimum / maximum |
| `ABS(x)`, `EXP(x)`, `LN(x)`, `LOG10(x)` | Math functions |
| `SIN(x)`, `COS(x)`, `ARCTAN(x)` | Trigonometric functions |
| `INT(x)` | Truncate to integer |
| `MODULO(a, b)` | Modulo (a MOD b) |
| `SIZE(dimension)` | Number of elements in a dimension |
| `SUM(array)`, `MEAN(array)` | Array aggregation |
| `UNIFORM(min, max, seed)` | Random uniform distribution |

## Modeling conventions

- **Stocks** accumulate over time. They use `initialEquation` (not `equation`) to set the starting value. The stock's rate of change is determined by its inflows and outflows.
- **Flows** are rate variables that add to or subtract from stocks. A flow's `equation` defines the rate per unit time.
- **Auxiliary variables** hold intermediate calculations or constants. Use `equation` for the formula.
- **Units** are optional but recommended for dimensional consistency checking. Set via the `units` field.
- **Graphical functions** (table functions / lookups) define piecewise-linear relationships. Set via `graphicalFunction` on flows or auxiliaries.

## Loop dominance analysis

ReadModel returns loop analysis data from the Loops That Matter (LTM) algorithm:

- `loopDominance`: Array of feedback loops discovered in the model. Each loop has:
  - `loopId`: Unique identifier
  - `name`: Human-assigned name (null if unnamed)
  - `polarity`: "reinforcing", "balancing", or "undetermined"
  - `variables`: Ordered list of variable names around the loop
  - `importance`: Array of importance values (0 to 1) per simulation timestep, indicating how much this loop drives model behavior at each point in time

- `dominantLoopsByPeriod`: Time intervals showing which loop dominates. Each period has:
  - `startTime`, `endTime`: Time range
  - `dominantLoops`: Names of the loops that dominate during this period

### Naming loops with setLoopName

Loops are discovered automatically but start unnamed. Use `setLoopName` in EditModel to assign meaningful names:

```json
{
  "projectPath": "model.sd.json",
  "operations": [
    {
      "setLoopName": {
        "variables": ["population", "births"],
        "name": "Growth Loop",
        "description": "More population leads to more births, which increases population"
      }
    }
  ]
}
```

The `variables` field lists the variable names that form the loop (order does not matter -- the engine matches by set membership). After naming, the loop's `name` field and `dominantLoopsByPeriod` entries will use the assigned name.

## Errors

ReadModel may return an `errors` array with compilation diagnostics when the model has problems:

```json
{
  "errors": [
    {
      "code": "unknown_dependency",
      "message": "error in model 'main' variable 'flow1': unknown_dependency",
      "modelName": "main",
      "variableName": "flow1",
      "kind": "variable"
    }
  ]
}
```

Fields: `code` (machine-readable error code), `message` (human-readable description), `variableName` (which variable has the error, if applicable), `modelName`, and `kind` (one of: "project", "model", "variable", "units", "simulation").

EditModel rejects edits that introduce new compilation errors -- the response will contain an error with the diagnostics so you can fix and retry.

## Simulation and advanced analysis with pysimlin

For running simulations, parameter sweeps, scenario analysis, and detailed loop dominance analysis beyond what the MCP tools provide, use the pysimlin Python package:

```
pip install pysimlin=={PYSIMLIN_VERSION}
```

pysimlin provides a full simulation API with pandas DataFrame results, parameter overrides, and programmatic access to loop importance time series. See the skill resources (`simlin://skills/pysimlin-basics`, `simlin://skills/scenario-analysis`, `simlin://skills/loop-dominance`) for detailed usage guides.

Imported in Python as `simlin`:

```python
import simlin

model = simlin.load("population.stmx")
run = model.run()
print(run.results["population"].iloc[-1])
```

The server also exposes skill resources around how to use the Python library:

- `simlin://skills/pysimlin-basics` -- Loading models, simulation, DataFrame access
- `simlin://skills/scenario-analysis` -- Parameter sweeps and intervention analysis
- `simlin://skills/loop-dominance` -- Feedback loop analysis and visualization
- `simlin://skills/vensim-equation-syntax` -- Vensim-to-XMILE function mapping
