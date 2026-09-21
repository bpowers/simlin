# Agent tool surface in the engine

## Why

Simlin's mission puts agents beside people: an agent reads a model, runs experiments, finds the feedback loops that drive it, tests it, and edits it. Today every host that gives an agent tools builds them itself:

- `simlin-mcp-core` has `ReadModel`, `EditModel` and `CreateModel`. `ReadModel` returns the whole model's JSON and every reported loop's per-step importance series; `EditModel` takes wholesale variable upserts, so an agent re-reads a variable before changing one field of it.
- pysimlin has `Model.explain`, `check`, `get_links`, `run(overrides=)`, `simulate()` with `run_to` and `set_value`, `Run.loops` and `analyze()`, each a composition in Python over libsimlin primitives.
- A native host that embeds an assistant would write a third set over the C API, and libsimlin already composes some analyses privately (`analyze_links_core`).

Three implementations of "what does this model do" drift, and none of them was designed for the job an agent does. An agent's tool result is the context for its next decision, and a claim it makes about a model is only as good as the evidence behind it. This plan puts the tools' semantics in the engine, once, for every host:

- **The engine computes, the agent explains.** Unit algebra, loop enumeration, dominance, sensitivity and integration error are what language models get wrong and what the engine gets exactly right. Every analysis an agent's critique relies on is a tool the engine runs.
- **Results are bounded and quiet.** No tool returns the whole model or a raw series. A model too large to outline in a few thousand tokens is outlined by sector. Success says little; a refusal names the rule it broke and the repair.
- **Results carry ids a claim can cite.** Diagnostics, loops, runs, test results and findings have ids that are stable for the life of a session, so an agent can say "D3" or "the half-adjustment run" and a verifier can check it.
- **Experiments are data.** An experiment is a declarative spec (values, multipliers, replacement equations, a start time, run specs), so it is bounded, checkable and cheap to describe.
- **Edits come back as a plan the host lands.** The engine plans and gates an edit; the host applies the patch through `simlin_project_apply_patch`, so an agent's edit lands through the one patch path, is undoable wherever the host keeps undo, and is shown like anyone else's.

The consumers are a host's own assistant (through libsimlin), MCP clients (through `simlin-mcp-core`), and pysimlin users, including evaluation harnesses that must exercise exactly what ships.

## Where it lives

`simlin_engine::tools`, beside `editing` and `analysis`. Both libsimlin and `simlin-mcp-core` already depend on the engine, so the module needs no new crate, no change to `scripts/dep-policy.json`, and no second copy of any engine query. The editing core (`docs/design-plans/2026-09-12-editing-core-in-rust.md`) set the precedent: host-facing planning that must agree across hosts lives in the engine, and libsimlin exposes it.

A separate crate would isolate the surface's serde and schema code from the compiler, but every tool is a composition of engine queries (diagnostics, causal edges, `build_sim`, discovery), so the crate would re-export most of the engine to itself.

## The session

A tool call runs in a `tools::Session`, bound to one model of a project:

```rust
pub struct Workspace<'a> {
    pub project: &'a datamodel::Project,
    pub db: &'a mut db::SimlinDb, // synced to `project`
    pub revision: u64,
}

impl Session {
    pub fn new(model_name: &str) -> Session;
    /// `Err` only for a tool the catalog does not list: the host's mistake.
    pub fn call(&mut self, ws: Workspace<'_>, tool: &str, input: &str)
        -> Result<ToolOutput, UnknownTool>;
    /// What changed in the model's variables and sim specs since the last
    /// `read_model`, for a host to tell the agent before its next turn.
    pub fn changes_since_read(&self, project: &datamodel::Project, revision: u64)
        -> Option<Changes>;
}

pub struct ToolOutput {
    pub json: String,
    pub is_error: bool,
}
```

A session holds what one agent's work needs across calls:

- **Evidence ids.** Diagnostics (`D1`, `D2`, ...), loops (`L1`, ...), test results and findings get ids when first reported, and keep them while the thing they name exists. A diagnostic is keyed by its code, variable and reason; a loop by its canonical cycle (the rotation-invariant, direction-preserving node sequence), so the engine's own loop-id scheme, which the single-mode LTM plan changes, never reaches an agent.
- **Named runs**, each with the experiment that produced it and the revision it ran at, so a later claim can cite a run and a run can be replayed (with the LTM overlay, when loops are asked of it).
- **What the agent last read**: the revision and the model's variable records at its last `read_model`, with its sim specs and what its variables rest on (the project's dimensions and unit definitions, and its other models, which a module instantiates). When the revision moves, the next read reports what changed (a variable added, removed, or changed in named fields, a case-only rename among them; the specs, the dimensions, the unit definitions; each other model whose variables changed), and an edit planned against an older read is refused, naming what changed.
- **Caches per revision**, dropped when the revision moves.

A session is cheap and owns nothing of the project's, so a host can keep one per conversation, or share one between an agent and its own views of the same model.

### The revision

The host passes the project's revision with every call; equal revisions must mean equal contents. libsimlin counts them: `ProjectContents` advances a counter on every mutable borrow of the datamodel (the same `DerefMut` that already drops the hit indexes) and on every replacement of it (`replace`, which a committed patch and `simlin_project_replace_contents` go through), so no entry point can forget it, and reads (dry-run and rejected patches, simulations, diagnostics, renders) never advance it. The counter sits beside the shared datamodel, not in it, so a copy's revision is its own. A landing trusts it: a plan whose revision is the project's lands without planning again. `simlin_project_get_revision` reads it. A host without a counter of its own, like the stateless MCP filesystem access, passes a hash of the file it read.

A revision is a statement about the whole datamodel, views included. A layout-only change advances it, but it changes no variable record, so it makes no read stale: staleness is judged by comparing variable records, never by the counter alone.

## The catalog

`tools::catalog_json()` describes every tool: its name, a description that says what it is for, the JSON Schema of its input and of its output, and its effect (`read` or `plan_edit`). The schemas are derived from the serde types with schemars (the `schema` feature) and checked in as `src/simlin-engine/src/tools/catalog.json`, which a freshness test regenerates and compares, the pattern `docs/simlin-project.schema.json` already follows. libsimlin builds the engine without `schema`, and embeds the checked-in file, so no schemars code reaches the wasm bundle.

Property order in each schema is declaration order (`preserve_order`): a host that bridges the catalog into a structured-generation framework can take a property order from it.

Inputs are strict (`deny_unknown_fields`): an agent that misnames a field is told which field and what the schema expects, rather than having it silently ignored.

## Outputs and failures

Every output is JSON with camelCase keys, and every successful output names the revision it read. Output types are serde types in the engine, so a UI consumes the same shapes an agent reads.

Every answer keeps to a byte budget (`OUTLINE_BUDGET`, 12,000 bytes of JSON, about 3,000 tokens), fitted rather than cut: what an answer leaves out is counted or named, and each tool says where the rest is read. A quote of an equation in a diagnostic's reason is a window of 240 characters around what it points at.

A domain failure is ordinary output with `is_error` set: `{"error": "...", "suggestions": [...]}`, naming the rule and the repair. An unknown variable is answered with the closest names; input that does not match the schema is answered with serde's reason. A host bridges `is_error` to its framework's tool-error channel, and must not turn it into an exception that ends the agent's turn: the agent reads the refusal and repairs its call. Only a host's own misuse (a null pointer, an unknown tool name) is an FFI error. A call that stopped for other work on the project, and kept nothing, adds `"interrupted": true`, so a host that retries by itself can tell it from a refusal of the call itself (see the libsimlin surface).

Names are matched forgivingly: canonically first, then by a similarity over the canonical name and its words, so a misspelled or misheard name finds its variable, and one owner (`tools::names`) answers both "which variable did you mean" and `find_variables`.

## The tools

Each tool starts from the MCP tool or the pysimlin call it refines; nothing depends on those shapes, and the refinements are the point.

| Tool | Starts from | Takes | Returns |
|---|---|---|---|
| `read_model` | MCP `ReadModel`; pysimlin `check`, `explain` | nothing | An outline: sim specs; each stock with its initial value, inflows and outflows; the flows, other variables and constants with their equations and units; each variable's diagnostic ids; the diagnostics with their ids, severities, codes and reasons, errors first; what changed since the session's last read. An outline over its budget keeps every entry and lists as many diagnostics as fit when the entries fit alone, and is otherwise outlined by sector: stocks and their flows, counts by kind, then as many diagnostics, then as many other variables' names, as fit. |
| `read_variables` | pysimlin `get_variable`, `get_incoming_links`, `get_links`, `explain` | up to 12 names, each a variable or one element of one (`population[north]`) | Per variable: kind, units, documentation, equation or initial value (an element's own, when one is named), arrayed equations, lookup points, a stock's flows or the stocks a flow fills and drains, its inputs and its readers with each link's polarity, its diagnostics in full, and a one-line summary of its behavior in the current run. A record lists at most 24 per-element equations, 24 inputs, 24 readers and 64 lookup points, counting the rest, and cuts documentation at 480 characters; records past the budget are named for another call. Names not found come back with suggestions. |
| `find_variables` | nothing | a phrase | Up to 10 variables, closest first, with kinds and units. |
| `run_experiment` | pysimlin `run(overrides=)`, `simulate()` with `run_to` and `set_value`; simlin-serve's `Simulate` | an experiment | A named run kept in the session: the changes as applied (a multiplier shows the value it produced), and per recorded variable a behavior summary and behavior mode, compared with the run it started from. |
| `read_behavior` | pysimlin `Run.results` | variables, runs | Per variable and run: start and end, minimum and maximum with their times, turning points, whether and when it goes negative, the behavior mode, and about a dozen samples. |
| `analyze_loops` | MCP `ReadModel`'s loop dominance; pysimlin `Run.loops`, `dominant_periods` | a run | Per cycle partition: its stocks, a dominance timeline sampled at about 12 times naming the leading loops and their shares, and up to 8 loops, each with its session id, runtime polarity, its chain closed back to its first variable with each link's sign, and its mean share. Whether the enumeration was complete, and how many loops were left out. A run at equilibrium, where every loop score is zero, reports its structural loops and says dominance is undefined there. |
| `run_tests` | nothing | tests, and optionally their targets | The validation battery (below), each result with an id. |
| `edit_model` | MCP `EditModel` and its gate; pysimlin `edit()` | a summary and a list of operations | A plan: the patch, one line per variable changed, the diagnostics it would add, whether the result simulates, and the gate's verdict. |
| `verify_findings` | nothing | findings with citations | Each citation's verdict: holds, or fails and what is true instead. |

### Experiments

```json
{
  "name": "half adjustment time",
  "from": "current",
  "set": [
    {"variable": "inventory adjustment time", "multiply": 0.5},
    {"variable": "customer orders", "equation": "10 + STEP(2, 5)"}
  ],
  "fromTime": 5,
  "specs": {"dt": "1/8", "method": "rk4", "stop": 60},
  "record": ["inventory", "production"]
}
```

- A constant takes a `value` or a `multiply`, applied as a VM override (`Vm::set_value`), with no recompile. `fromTime` applies value changes from that time on (`run_to`, then `set_value`), as a pysimlin game loop does.
- A computed variable takes a replacement `equation`, in that run only: the session stages a copy of the datamodel with the replacement on the host's db (`sync_staged`), compiles and runs it, and restores (`restore`), exactly as a dry-run patch does, so the project is never changed and unchanged variables keep their compiled fragments. The result names the loops the replacement cuts; that is how an agent knocks out a link to test an explanation.
- `specs` change the run's DT, integration method and stop time the same way.
- `from` starts from another named run's changes.

### Behavior modes

One engine function classifies a series: at rest, linear growth or decline, exponential growth, goal seeking (approach to an equilibrium), S-shaped growth, overshoot, oscillation (damped, sustained or growing), or none of these. The battery, experiments, `read_behavior` and a host comparing a person's predicted behavior with a run all call it. It works on first and second differences over the saved steps with a tolerance relative to the series' range, so noise below a percent of the range never makes a turning point.

### The validation battery

The battery is the model tests of system dynamics practice (Forrester and Senge, "Tests for building confidence in system dynamics models", 1980; Sterman, *Business Dynamics*, ch. 21), run mechanically:

| Test | What the engine runs | What it reports |
|---|---|---|
| `units` | unit checking | each unit diagnostic, by id |
| `extreme_conditions` | each targeted input at zero and at ten times its value (by default every constant that feeds a flow) | per run: stocks that go negative (a failure for a stock marked non-negative, reported for judgment otherwise), values that become NaN or infinite, values that blow up |
| `integration_error` | the run at half the DT, and under RK4 | each stock's largest relative difference, flagged above 1% |
| `sensitivity` | each targeted constant at half and double | per recorded variable: the change in its final value and its peak, and any change of behavior mode |
| `loop_knockout` | a targeted variable held at its initial value | the loops that cuts, and how behavior changes |
| `disturbance` | a step in each exogenous input from the base run | each stock's response and behavior mode, and the loops that dominate it |

- **Extremes are meaningful.** A time constant's low extreme is DT, not zero: a time constant below DT is an integration artifact, not a condition of the system, and zero divides by zero. A time constant is recognized by its role (a stock divided by it in a flow, or the delay or averaging time of a delay or smooth) as well as by its units, since models often declare one dimensionless.
- **A model at rest hides its loops.** A model that starts in equilibrium, as models are encouraged to, has zero loop scores throughout, so dominance and loop polarity from a run are undefined there. The disturbance test is how the battery sees its structure.

### Editing and the gate

`edit_model` takes operations as a tagged list (`add_stock`, `add_flow`, `add_variable`, `set_equation`, `set_units`, `set_notes`, `set_lookup`, `connect_flow`, `rename`, `delete`, `name_loop`, `set_sim_specs`) and returns a plan without applying it:

1. **Staleness.** A plan against a model whose variable records changed since the session last read them is refused, naming what changed.
2. **One patch.** Field edits keep every field they do not set. Additions get view elements from the engine's incremental layout inside the same patch, so placement undoes with the edit.
3. **The gate.** The patch is staged and its diagnostics compared with the model's: it is refused if it adds an error the model did not have, compared by code and variable. New unit warnings are listed so the agent fixes them. This is `simlin-mcp-core`'s gate, moved where every host gets it.
4. **The plan**: the patch as the JSON `simlin_project_apply_patch` takes, one line per variable changed, the diagnostics it would add, whether the result simulates, and the verdict.

The patch JSON's types move from libsimlin into the engine (`json` owns the project's JSON already), so the plan is serialized from the type the host deserializes.

### Findings and verification

A finding is a claim with typed citations: a variable (or one of its fields, or that it reads another), a diagnostic, a loop with its polarity, a loop's dominance over a span of a named run, a run fact (goes negative, peaks at a time, ends near a value, shows a behavior mode), a comparison between two runs, an absence (no diagnostics of a kind; no loop through a variable), or a test result. `verify_findings` checks each against the session at the current revision within a tolerance, deterministically, so a host can refuse to show a claim whose evidence does not hold, and an evaluation can check claims without judgment. Whether a claim follows from its citations is a judgment the surface does not make.

## libsimlin surface

- `simlin_project_get_revision(project, out_revision, out_error)`.
- `simlin_tools_describe(out_buf, out_len, out_error)`: the catalog JSON in a `simlin_malloc` buffer.
- `SimlinToolSession`, refcounted, holding a reference to its model: `simlin_tool_session_new(model, out_error)`, `simlin_tool_session_ref`, `simlin_tool_session_unref`.
- `simlin_tool_session_call(session, name, input, input_len, out_buf, out_len, out_is_error, out_error)`: JSON in, JSON out. `out_is_error` carries a domain refusal; `out_error` only the host's misuse.
- `simlin_tool_session_get_changes(session, out_buf, out_len, out_error)`: the change report since the session's last `read_model`, as JSON, or `null`.
- `simlin_tool_session_get_run(session, name, out_error) -> *mut SimlinResults`: a named run's series for a host to chart, through the results handle `simlin_results_open_vdf` already returns.

A call holds its session for the call, and the project's datamodel only while it takes the contents it answers from and their revision. The contents are shared, not copied (`ProjectContents::shared`); an edit that lands meanwhile copies them first, as any edit of shared contents does. The call then answers under the db lock alone, and the db stays synced to those contents while the call holds it, since a sync needs the db lock. A call that finds the db held -- by another session's call, or by a host's query that holds only the db -- waits for it with the datamodel released, then takes the contents again. So a host's hit tests, planners and revision reads, which lock only the datamodel, never wait behind an analysis, which on the largest models takes seconds.

Work that holds the datamodel while it waits for the db -- an edit landing or an undo, a simulation, a read of the diagnostics, loop discovery, a wasm compile, a render that lays out a model with no view -- keeps those readers waiting with it, so a call yields to it. libsimlin counts such waiters: `lock_db_with` and `built_db` count themselves while they wait, and a tool call's own lock does not, so two calls never stop for each other. A call checks the count between units of its work -- one simulation, or one stage of an analysis -- through `Workspace::waiting`, and stops there, answering a refusal marked `interrupted` that kept nothing. The waiter waits at most one unit, and the agent, or a host that retries by itself, calls again once the model is as the waiter left it: after an edit, at the next revision, and never in a loop against a project that stays busy, which would only take the database back from the person's work. Salsa's own cancellation would unwind the call, and a release build aborts on a panic; a second synced database for calls would double memory on exactly the models where calls are long. A plan's patch lands through `simlin_project_apply_patch` as any edit does.

The entry points are behind libsimlin's `agent_tools` feature (the engine's feature of the same name gates `tools`), on by default and off in the browser wasm bundle, as `png_render` is.

## MCP and pysimlin

- **pysimlin** binds the catalog and a `ToolSession` (`Model.tools()`, `call(name, **input) -> dict`), for its users and for evaluation harnesses, which then exercise the same functions a native host ships.
- **simlin-mcp-core** mounts the catalog as MCP tools, each input schema extended with `projectPath` and `modelName`, holding one session per project path; `FileSystemAccess` supplies a content hash as the revision. A plan is landed by the MCP host in its file. Whether these tools join `ReadModel`/`EditModel`/`CreateModel` or replace them is a wire decision for the MCP surface's owner.

## Relationship to other plans

- **Single-mode LTM** (`docs/design-plans/2026-09-07-ltm-single-mode.md`). `analyze_loops` reads the post-simulation discovery pipeline that plan keeps, and reports runtime polarity. Its session loop ids isolate agents from the engine's id change. It needs one thing that plan's D4 would delete: a link's static sign. At equilibrium every link score is zero, so a model at rest has no runtime sign for any link, and `read_variables` reports the static polarity there. The replacement this surface needs is "the runtime sign when the run moves, the static sign otherwise".
- **Diagnostics' reasons.** Every diagnostic the engine formats carries a reason, the raising site's or, failing that, what its code means. An agent needs both what went wrong and where, so a report gives the site's reason and quotes the span whenever the diagnostic has one; a parse error, whose site wrote no reason, is reported by its span alone, and the code's meaning is the reason only when there is neither.
- **The editing core.** `edit_model`'s additions are placed by the same incremental layout, and its patch is the one the editing core's patches go through.

## Testing

- Every tool is tested through `Session::call` over models built with `TestProject`, `open_xmile` or `open_vensim`, and read back through production queries; no tool has a second path for tests.
- Decision tables derive their rows from their enums: every tool in the catalog, every kind a variable can be, every field a change report names, every behavior mode, every battery test, every citation kind, every edit operation.
- The catalog freshness test; a test that every catalog schema accepts the inputs the tool's tests send and rejects an unknown field.
- Output bounds: an outline of every corpus model the suite already compiles stays within its budget (World3 and C-LEARN, with the other heavy gates, under `#[ignore]`); the budget itself is tested with a test-only override and a small model.
- libsimlin's integration harness drives each entry point: the revision advances exactly on mutations (a row per mutating entry point and per read, as `contents_tests.rs` rows the hit index), a session call from a model handle to JSON, refusals, and NULL safety.

## Phases

1. **The session and the read tools.** The contents revision and `simlin_project_get_revision`; `tools::Session`, the evidence ids for diagnostics, the change report, names; the catalog and its freshness test; `read_model`, `read_variables` (without behavior), `find_variables`; libsimlin's describe, session and call entry points; the links core moved into the engine as the one owner of a model's causal links with their polarities.
2. **Experiments and behavior.** `run_experiment`, `read_behavior`, behavior modes, named runs and `simlin_tool_session_get_run`; `read_variables` gains its behavior line.
3. **Loops.** `analyze_loops` over a named run, with session loop ids and the equilibrium case.
4. **The battery.** `run_tests`.
5. **Editing.** `edit_model` and the gate; the patch JSON types in the engine.
6. **Verification.** `verify_findings`.
7. **Hosts.** The pysimlin binding and the MCP mount.
