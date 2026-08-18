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
| Vensim | `.mdl`                                     | Yes | Yes |

Every format is edited in place: EditModel rewrites the file in its own format (regenerated XMILE, Vensim text including the sketch, or JSON), and CreateModel picks the format from the path's extension. Vensim cannot express a few Simlin constructs (a non-negative stock or flow, a discrete or extrapolating lookup, the ROUND builtin, ...); an EditModel on a `.mdl` still saves those in their closest Vensim form and lists each degradation in the response's `warnings` with an `MDL export:` prefix. Check `warnings` after editing a `.mdl` and tell the user when a construct was degraded.

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
| `INT(x)` | Integer part (floor: rounds toward negative infinity) |
| `ROUND(x)` | Round to nearest integer; exact .5 ties go to the even neighbor (like Python's `round()`) |
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
  - `polarity`: one of "reinforcing", "balancing", "mostly_reinforcing", "mostly_balancing", or "undetermined". "reinforcing" / "balancing" are emitted when every loop-score sample shares one sign. "mostly_reinforcing" / "mostly_balancing" (Rux / Bux in the LTM literature) are emitted when the loop expresses both polarities during simulation but one side dominates with confidence at or above the 0.99 threshold from Schoenberg & Eberlein (2020). "undetermined" covers genuinely mixed-sign loops below that threshold.
  - `polarityConfidence`: the confidence ratio in [0, 1] behind `polarity` -- 1.0 for a clean single-signed reinforcing/balancing loop, below 1.0 (but at or above 0.99) for a "mostly_*" loop, and 0.0 for "undetermined".
  - `variables`: Ordered list of variable names around the loop
  - `importance`: Array of signed relative loop scores (-1 to 1) per simulation timestep. This is the loop's share of its cycle partition's total absolute loop score, with the sign of the loop's contribution preserved (a balancing loop reads negative). `abs(importance)` is the fraction of partition activity this loop drives at that timestep. Loops arrive already ranked: loops that compete with other loops in their cycle partition come first (ordered by mean `abs(importance)`), and loops trivially alone in their partition -- whose relative score is exactly 1 by construction, e.g. an isolated stock-decay loop, carrying no information -- come last. Treat the list order as the dominance ranking; re-ranking by raw mean `abs(importance)` would re-surface the trivially-isolated loops at the top

- `enumerationComplete`: Whether the loop analysis was EXACT. `true` means the engine enumerated every loop that could ever score and `loopDominance` is the selection from that whole set (exact for cross-aggregate reducer loops too only while `aggRecoveryTruncated` is absent). `false` means a budget cut the enumeration short and a shortest-path search sampled the model's loops instead -- on a `false` result, a loop's absence from `loopDominance` is not evidence the model lacks it, so say so rather than concluding the model has no such feedback. Always present.
- `retainedLoops`: How many loops cleared the importance filter before the 200-loop report cap truncated `loopDominance`. When this exceeds the length of `loopDominance`, you are looking at a coverage-aware subset of the retained loops -- each step's dominant loop per feedback partition is guaranteed a slot (while those dominant loops fit the cap) and the rest is filled by mean importance -- presented in importance order but not a strict most-important-first prefix, so an omitted loop can outrank a reported one.
- `truncated`: Present (and `true`) only when candidate generation stopped before covering every saved step -- the shortest-path fallback hit its candidate bound -- so `loopDominance` is a sample cut short. Absent otherwise.
- `universeLoops`: How many DISTINCT loops the enumerated candidate universe held -- the population each loop's `importance` is a share of. Absent when `enumerationComplete` is false, because a sample has no universe to report (which is a different claim from a universe of zero).

- `dominantLoopsByPeriod`: Time intervals showing which loop dominates, computed per cycle partition (a loop's importance is its share WITHIN its partition, so dominance across partitions is not comparable). The list carries one period timeline per partition, most-competitive partition first. Each period has:
  - `startTime`, `endTime`: Time range
  - `dominantLoops`: Names of the loops that dominate during this period
  - `partition`: Index into `partitions` naming which cycle partition this period describes (the same index space as each loop's `partition`); absent for loops with no partition metadata

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
