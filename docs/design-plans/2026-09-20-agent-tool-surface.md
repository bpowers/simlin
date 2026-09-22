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
- **Edits come back as a plan the host lands.** The engine plans and gates an edit; the host lands it once the person approves, through `simlin_tool_session_land_plan`, which lands it by construction on the project as it is. So an agent's edit is one edit of the host's, undoable wherever the host keeps undo and shown like anyone else's, and it never lands on a model it was not checked against.

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
- **Named runs**, each with the experiment that produced it, the revision it ran at, and a key of what it simulated, so a later claim can cite a run and a run can be replayed (with the LTM overlay, when loops are asked of it). A run is fresh while the model has what it simulated: the project without its diagrams, sectors, provenance, source file, and its variables' documentation and units, hashed once per revision. Units are checked, never simulated, so a layout edit, or an edit of a variable's notes or units, leaves every run fresh, and the current run is not simulated again.
- **What the agent last read**: the revision and the model's variable records at its last `read_model`, with its sim specs and what its variables rest on (the project's dimensions and unit definitions, and its other models, which a module instantiates). When the revision moves, the next read reports what changed (a variable added, removed, or changed in named fields, a case-only rename among them; the specs, the dimensions, the unit definitions; each other model whose variables changed), except what the session's own landed plans left, and an edit planned against an older read is refused, naming what changed.
- **Caches per revision**, dropped when the revision moves.

A session is cheap and owns nothing of the project's, so a host can keep one per conversation, or share one between an agent and its own views of the same model.

### The revision

The host passes the project's revision with every call; equal revisions must mean equal contents. libsimlin counts them: `ProjectContents` advances a counter on every mutable borrow of the datamodel (the same `DerefMut` that already drops the hit indexes) and on every replacement of it (`replace`, which a committed patch and `simlin_project_replace_contents` go through), so no entry point can forget it, and reads (dry-run and rejected patches, simulations, diagnostics, renders) never advance it. The counter sits beside the shared datamodel, not in it, so a copy's revision is its own. A landing trusts it: a plan whose revision is the project's lands without planning again. `simlin_project_get_revision` reads it. A host without a counter of its own, like the stateless MCP filesystem access, passes a hash of the file it read.

A revision is a statement about the whole datamodel, views included. A layout-only change advances it, but it changes no variable record, so it makes no read stale and no run stale: a read's staleness is judged by comparing variable records, and a run's by comparing what a simulation reads, never by the counter alone.

## The catalog

`tools::catalog_json()` describes every tool: its name, a description that says what it is for, the JSON Schema of its input and of its output, and its effect (`read` or `plan_edit`). The schemas are derived from the serde types with schemars (the `schema` feature) and checked in as `src/simlin-engine/src/tools/catalog.json`, which a freshness test regenerates and compares, the pattern `docs/simlin-project.schema.json` already follows. libsimlin builds the engine without `schema`, and embeds the checked-in file, so no schemars code reaches the wasm bundle.

Property order in each schema is declaration order (`preserve_order`): a host that bridges the catalog into a structured-generation framework can take a property order from it.

Inputs are strict (`deny_unknown_fields`): an agent that misnames a field is told which field and what the schema expects, rather than having it silently ignored.

## Outputs and failures

Every output is JSON with camelCase keys, and every successful output names the revision it read. Output types are serde types in the engine, so a UI consumes the same shapes an agent reads.

Every answer keeps to a byte budget (`OUTLINE_BUDGET`, 12,000 bytes of JSON, about 3,000 tokens), fitted rather than cut: what an answer leaves out is counted or named, and each tool says where the rest is read. A quote of an equation in a diagnostic's reason is a window of 240 characters around what it points at.

A number that is not finite -- a series that divided by zero or overflowed -- is left out of an answer, and its field is optional in the output schema; the behavior mode says when the series went undefined. JSON has no NaN, and a `null` where a schema says `number` is an answer the agent's framework may refuse to parse.

A domain failure is ordinary output with `is_error` set: `{"error": "...", "suggestions": [...]}`, naming the rule and the repair. An unknown variable is answered with the closest names; input that does not match the schema is answered with serde's reason. A host bridges `is_error` to its framework's tool-error channel, and must not turn it into an exception that ends the agent's turn: the agent reads the refusal and repairs its call. Only a host's own misuse (a null pointer, an unknown tool name) is an FFI error. A call that stopped for other work on the project, and kept nothing, adds `"interrupted": true`, so a host that retries by itself can tell it from a refusal of the call itself (see the libsimlin surface). A call its host cancelled adds `"cancelled": true` instead, and is not made again.

Names are matched forgivingly: canonically first, then by a similarity over the canonical name and its words, so a misspelled or misheard name finds its variable, and one owner (`tools::names`) answers both "which variable did you mean" and `find_variables`.

## The tools

Each tool starts from the MCP tool or the pysimlin call it refines; nothing depends on those shapes, and the refinements are the point.

| Tool | Starts from | Takes | Returns |
|---|---|---|---|
| `read_model` | MCP `ReadModel`; pysimlin `check`, `explain` | nothing | An outline: sim specs; each stock with its initial value, inflows and outflows; the flows, other variables and constants with their equations and units; each variable's diagnostic ids; the diagnostics with their ids, severities, codes and reasons, errors first; what changed since the session's last read. An outline over its budget keeps every entry and lists as many diagnostics as fit when the entries fit alone, and is otherwise outlined by sector: stocks and their flows, counts by kind, then as many diagnostics, then as many other variables' names, as fit. |
| `read_variables` | pysimlin `get_variable`, `get_incoming_links`, `get_links`, `explain` | up to 12 names, each a variable or one element of one (`population[north]`) | Per variable: kind, units, documentation, equation or initial value (an element's own, when one is named), arrayed equations, lookup points, a stock's flows or the stocks a flow fills and drains, its inputs and its readers with each link's polarity, its diagnostics in full, and a one-line summary of its behavior in the current run. A record lists at most 24 per-element equations, 24 inputs, 24 readers and 64 lookup points, counting the rest, and cuts documentation at 480 characters; records past the budget are named for another call. Names not found come back with suggestions. |
| `find_variables` | nothing | a phrase | Up to 10 variables, closest first, with kinds and units. |
| `run_experiment` | pysimlin `run(overrides=)`, `simulate()` with `run_to` and `set_value`; simlin-serve's `Simulate` | an experiment | A named run kept in the session: the changes as applied (a multiplier shows the value it produced), and per recorded variable a behavior summary and behavior mode, compared with the run it started from. |
| `list_runs` | nothing | nothing | The named runs, oldest first, each with its revision, whether it is stale or gone, the run it started from, and everything it changed from the model: each value (from a time on, or from the start), each replacement equation, the specs, as they ran. Over its budget it quotes long equations around their start, then leaves out the oldest runs, named. A host lists the same runs through `simlin_tool_session_list_runs`. |
| `read_behavior` | pysimlin `Run.results` | variables (or one element of one, `population[north]`), runs | Per variable and run: start and end, minimum and maximum with their times, turning points, whether and when it goes negative, the behavior mode, and about a dozen samples. Over its budget it leaves out the samples, then the turning points, then elements, then whole variables, and says so. |
| `analyze_loops` | MCP `ReadModel`'s loop dominance; pysimlin `Run.loops`, `dominant_periods` | a run; optionally a variable the loops go through (or one element of one), or loop ids to report whole | Per cycle partition, largest first: its stocks, a dominance timeline of at most 12 spans naming the strongest loop in each and its rivals with their shares, and the loops the timeline names plus the most important others, each with its session id, the polarity the run shows, its chain from a stock with each link's sign (or, for a long loop, its stocks), and its mean share. Whether the loops were drawn from every loop, and how many were left out. A run in which no loop is active reports the model's loops from structure and how to disturb it; a run that replaced equations names the links and loops they cut. |
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
  "specs": {"dt": 0.125, "method": "rk4", "stop": 60},
  "record": ["inventory", "production"]
}
```

- A constant (or a flow whose equation is a number) takes a `value` or a `multiply`, applied as a VM override (`Vm::set_value`) with no recompile. A multiplier is resolved when the experiment is made, against the constant's value in the starting run, so a plan holds values and equations only and replaying it reproduces the run.
- `fromTime` makes value changes take effect at the first step at or after that time, the step `IF TIME >= t` turns on at: the time is snapped up to the step grid, the VM runs to half a step before that step (`run_to` evaluates the step at its end time before it stops), then the values are set, as a game loop sets them. A change at the start time is a change from the start, which initial values read.
- A variable takes a replacement `equation`, in that run only (for a stock, its initial value): the session stages a copy of the datamodel with the replacement on the host's db (`sync_staged`), compiles and runs it, and restores (`restore`, from a guard, so a panic restores too), exactly as a dry-run patch does, so the project is never changed and unchanged variables keep their compiled fragments. An equation change holds from the start; to change a variable at a time, the time goes into its equation. A replacement equation replaces the variable's value: a table its value was read from is dropped (`applied` says so), so holding an "effect of" at 1 holds it at 1. Replacing an equation cuts the links from what it read: that is how an agent knocks out a link to test an explanation, and the loop analysis names the loops it cut. A standalone table other equations call has no value to replace.
- A model that does not simulate can be tried in a copy: an experiment from "current" that replaces equations runs, compared with nothing, so an agent can test a fix to a learner's broken model before proposing it. A value change has nothing to run there, and is refused with that repair.
- `specs` change the run's start, stop, DT and integration method the same way.
- `from` starts from another fresh run, keeping its changes except to the variables this experiment changes; a run made before the model changed is not a starting point.
- A session keeps 32 named runs and forgets the oldest past that; a name used again replaces its run. It keeps at most 64 MB of their results: past that the oldest keep only their plans, in their places in the listing, a fresh one is run again from its plan when asked for, and a stale one whose results went is gone.
- One run holds at most 2,000,000 numbers (its saved rows times the model's slots, 16 MB) and computes at most 200,000,000 (its steps times the slots), or what the model's own specs cost when that is more, so the model as it stands always runs. An experiment over either is refused before its results are allocated, with its numbers and a DT or stop time that fits. A DT of 1e-4 on a small model would otherwise ask for tens of megabytes, and the store would keep them.
- A call that runs several simulations -- a starting run, runs kept only as plans, the experiment's own -- stops before the next when other work waits for the project, and keeps nothing. A simulation is itself taken in sixteen slices of its horizon, and stops between two of them, so the work that waits waits at most a slice of a run.

### Loops

A run is made without the LTM overlay, which costs a compile the other tools never need. `analyze_loops` replays the run's plan under the overlay with the project in discovery mode (every causal edge scored; a guard sets the mode back, a panic included), runs discovery over the replay's results (`analysis::discover_run_loops`, the step `analyze_model` also takes), and keeps the analysis with the run, so a second call answers from it.

- **Partitions.** Loops compete only within a cycle partition (stocks connected by feedback), so the answer is per partition, largest first.
- **The timeline.** The run is cut into 12 windows. In each, a loop's share is its mean share of the partition's loop activity (the partition-relative loop score), and the strongest loop leads, with its rivals: loops holding at least half its share, at most three in all. Adjacent windows with the same strongest loop merge into one span, and the span's leaders are recomputed over it. Between two spans each led by a loop, the boundary moves to the step from which the new leader stays at least as strong as the old, so a change of lead reads where it happened rather than at a window's edge. A span in which no loop holds a thousandth of the activity has no leader. Shares sum to one at each time over the partition's loops, so in a partition with thousands of loops (World3's 2,602) the strongest holds a few percent; the answer then says that no one loop dominates those spans, rather than letting a small leader read as dominance.
- **Loops listed.** Every loop the timeline names, then the most important others by mean share, at most 8 besides the leaders.
- **Chains.** A chain starts at a stock and leaves out a builtin's or macro's internal nodes, composing the signs of the links across them. Discovery leaves out an aggregate node (the synthetic node an array reducer such as `SUM(pop[*])` is routed through), and a link across one is signed by the path score through it, the product of its link scores.
- **Link signs.** A link the run scored has the run's sign, by the rule a loop's polarity is read by: the sign its scores hold, where mixed signs count as one when they net to at least 99% of their magnitude, and `?` when its sign changed more than that. So a link through a non-monotone table, whose equation cannot sign it, is signed by what the run did, and a loop the run leaves undetermined shows which of its links changed sign. A link the run never scored has its equation's sign.
- **Long loops.** A loop of more than 12 variables is given by its length and its stocks; asked for by id, it comes whole.
- **Ids.** A loop's session id is keyed by its cycle's canonical rotation with the internals left out, so the same loop has one id in every run and revision, from a run or from structure.
- **A run at rest.** At equilibrium every loop score is zero, or arithmetic noise, so dominance and a loop's runtime polarity are undefined. A run whose stocks are at rest by the behavior classifier (to a summary's precision), or in which no loop is active, is answered from structure, saying which: the answer lists the model's loops from structure, with their equations' signs, when the structure alone enumerates them (`model_detected_loops` out of discovery mode, when `model_ltm_mode` says the model is small enough), and names the repair: an experiment that disturbs the model.
- **Conveyors and queues.** A model with one builds through a path that scores no loop, and its causal graph has no link through one, so the answer says loops through them are not analyzed and lists only the structural loops that avoid them.
- **Knockouts.** A plan that replaces an equation reports the links the replaced variable read and no longer reads, and the model's loops through them: its structural loops, or its current run's when its structure is too large to enumerate.
- **The budget.** The answer keeps to `OUTLINE_BUDGET`, before any id is given out: past a partition's leaders and first three others, its lesser loops go first, then the loops a cut names past its first, then the smallest partitions, then the first partition's other loops, and last the timeline's rivals. Loops asked for by id come whole, as many as fit in the order asked, the rest named in `leftOut` for another call. World3 answers in 8 KB (one partition, 30 loops listed, the long ones by their stocks) and C-LEARN in 8 KB (2 of its 15 partitions, one per scenario), in 0.5 s and 2.7 s the first time and under a millisecond after, on a release build.

### Behavior modes

One engine function classifies a series: at rest, linear, exponential, goal seeking (approach to an equilibrium, or decay), S-shaped, overshoot, rise and fall (overshoot and collapse), fall and rise, oscillation (damped, sustained or growing), undefined (a non-finite value), or other. The battery, experiments, `read_behavior` and a host comparing a person's predicted behavior with a run all call it. It works on first differences over the saved steps with one tolerance, 1% of the series' range, so noise below it never makes a turning point, after trimming a still start and a settled end. A series whose range is within 1e-5 of its magnitude, the precision a summary reports numbers to, is at rest. What it names is the run's behavior over the run's horizon, not the structure's.

### The validation battery

The battery is the model tests of system dynamics practice (Forrester and Senge, "Tests for building confidence in system dynamics models", 1980; Sterman, *Business Dynamics*, ch. 21), run mechanically by `run_tests`. Each test is a set of checks, each check one run with one change compared with the model's current run:

| Test | What the engine runs | What it reports |
|---|---|---|
| `units` | unit checking | each unit diagnostic, by its `read_model` id; fails when there is one |
| `extreme_conditions` | each targeted constant at its low extreme and at its high one | values that become NaN or infinite when they are not in the model's own run (a failure); stocks, and flows the model marks non-negative, that go negative when the model's run does not (flagged for judgment, saying when the model's marking is one the engine does not enforce) |
| `integration_error` | the run at half the DT (saved at the model's own times), and under RK4 unless it already runs under RK4 | the stocks (every element of an arrayed one) that differ most, as a fraction of their scale; fails above 1% |
| `sensitivity` | each targeted constant at half and double (a share at most the whole) | per recorded variable, the change in its final value, its largest change over the run, and any change of behavior mode; flagged when the behavior changes family materially |
| `loop_knockout` | a named variable held at its initial value, each element of an arrayed one at its own | the links and loops that cuts, and how the recorded variables respond |
| `disturbance` | each targeted constant stepped up a tenth, a tenth of the way into the run | how the recorded variables respond, and the loops that lead after the step |

- **Targets.** By default, the constants that feed the model's flows, those that reach the most stocks first, less the model's unit conversions; a disturbance, a loop analysis each, steps the first three. A knockout runs only on variables the call names. Recorded variables are the model's stocks by default.
- **Unit conversions are not inputs.** A constant whose value is one of its units changes the units a quantity is counted in, not the quantity, so zero or ten times it is a parameter made nonsensical, not an input at an extreme. It is one when its units are a ratio of one unit at two scales and its value is their ratio (`1e6 tons/Mton`, `1e9 people/billion_people`, `1000 ppt/ppb`: SI prefixes, number words and pure-number units like percent), when its units are a ratio of two units of time and its value their known ratio, within 2% for the calendars' differences (`hours_per_week = 168 hour/week`, `days_per_year = 365` or `360 day/year`, `months_per_year = 12`), or when its units are one unit and its name states its value in them (`one_year = 1 year`, `100_percent`). A ratio that is not theirs is a quantity (`0.3 Mton/Gton`, `40 hour/week` of work), and a constant its name does not state is a parameter (`adjustment_time = 1 year`). The defaults leave conversions out and name them; a call that names one tests it. A physical constant (a molar mass ratio) is tested like any other: nothing in its units says so. Vensim's `==`, the modeler's own marking of an unchangeable constant, would, but the MDL reader does not keep it yet.
- **Extremes are meaningful.** A time constant's low extreme is DT, not zero: a time constant below DT is an integration artifact, not a condition of the system, and zero divides by it. A third-order delay's or smooth's time is at least three DTs, since each stage takes a third of it (World3's persistent pollution transmission delay at one DT explodes to 10^129, an artifact). A share or fraction's high extreme is the whole, not ten times it, and doubling one stops at the whole.
- **Time constants by units where there are units, by structure otherwise.** The time of a delay, smooth or trend is one. A constant with units is one when they are a unit of time (any the engine knows, or the model's own), and is not otherwise, whatever its role: a divisor in the units of what it divides is a scale. A constant declared dimensionless (`1`, `dmnl`) has no units to go by where the rate it divides carries a unit warning: a stock over it there cannot balance, so the declaration is the modeler's slip, which that warning reports, and its role decides (workforce's `average_tenure`, mark2's `insolvency_adjustment_time`). Where the rate's units balance, the declaration is right, and the constant is not one by its role (`pipeline / delay_time / number_of_stages`). A constant without units is one by its role in a rate (a flow, or a term of what a flow adds up): the divisor in `x / c`, `x` depending on a stock (`(goal - level) / adjustment_time`, `workforce / average_tenure * effect`, `population / (lifetime * 1)`), unless `c` is itself a term of a sum in `x` (`(capacity - population) / capacity`, a scale); or a reciprocal `1 / c` beside a factor depending on a stock, directly or through a fractional rate the term multiplies (`population * (1 / lifetime)`, `population * death_fraction` with `death_fraction = 1 / lifetime`), but not a variable over `c` (`-gravity / length * angle`). A role is judged on the equation with the algebra that only spells it differently undone: a power of -1 is a division and a factor or divisor of one is nothing, so `population * lifetime ^ -1`, `population / lifetime / 1` and `population * (1 / lifetime)` all read as `population / lifetime`. A divisor elsewhere is a scale, whose low extreme is zero: `population / capacity` inside `r * p * (1 - p / capacity)`, a lookup's input. At zero, a carrying capacity divides by zero, which the check reports. A check at DT says which rule took its constant for a time constant (`timeConstant`), so an agent can tell a finding from a misclassification.
- **Shares.** A constant is a share or fraction when it is within its whole and its units say so (`fraction`, `percent`), or, without units or dimensionless, its name does (share, fraction, proportion, percent) or an equation takes its complement (`1 - c`).
- **What the model's own run already has is not a finding.** A series that is not a number somewhere in the model's run (an unfilled equation, a delay of zero) is not judged by any check, and the test says how many were left out; a series already negative is not judged for going negative.
- **Non-negative markings.** The engine does not enforce a stock's or flow's non-negative marking, so a marked one that goes negative is flagged, saying so, rather than failed: a tool that enforces the marking would hold it at zero.
- **Growth is not judged.** Ten times a growth rate grows ten times as fast; no threshold separates that from a blow-up without judgment, so only non-finite values fail.
- **A change of behavior family is material.** A classifier label flips at its boundaries, and which of linear, exponential, goal seeking or S-shaped growth a series reads as depends on the horizon as much as on the structure, so a sensitivity check is flagged for a change of behavior only when a recorded variable changes family (still, rising, falling, one turn, oscillating; not to or from "other" or "undefined") and its series moves at least 5% of its scale somewhere. A check whose run makes a recorded series undefined, where the model's run has it a number throughout, has found something in any test: a sensitivity check, a knockout or a disturbance with such a response is flagged and listed, and the response's change and largest change, which are not numbers, are left out. Goal seeking is one behavior on either side of its goal: a goal seeker whose goal moved past its start, so that it falls where it rose, still seeks it. Growth that turns to decline has changed. World3 flags 44 of 118 such checks, qualitative changes a nonlinear model makes under halving and doubling; the strongest are listed first.
- **A model at rest hides its loops.** A model that starts in equilibrium, as models are encouraged to, has zero loop scores throughout, so dominance and loop polarity from a run are undefined there. The disturbance test is how the battery sees its structure.
- **What is listed.** A check that passed is counted, not listed, except sensitivity's strongest five; the others are listed failures first, then checks that could not run, flagged, observed, at most 20 and within the answer budget, the rest counted. Each listed check has an id (`T1`, ...) keyed by its test, target and condition, so running it again after an edit gives the same id, which is what a finding cites; a check left out gets no id.
- **Cost.** Value-only checks share one compile and run in parallel VMs where the platform has threads, each run summarized as it finishes and its results dropped, so a battery holds one run per thread. World3's checks take 1.5 s and C-LEARN's 12.6 s on a release build, most of it the disturbances' loop analyses, each answer within the outline budget. The two batteries run in one process peak at about 520 MB, most of it the loop analyses' LTM compile of C-LEARN. Once other work waits for the project, no further run starts and a run under way stops between its slices, and the battery stops and keeps nothing, the ids it gave out on the way included.

### Editing and the gate

`edit_model` takes a summary for the person approving the edit and operations as a tagged list (`add_stock`, `add_flow`, `add_variable`, `set_equation`, `set_units`, `set_notes`, `set_lookup`, `connect_flow`, `rename`, `delete`, `name_loop`, `set_sim_specs`), and returns a plan without applying it:

1. **Fresh.** Everything the edit writes -- each variable (the stocks a flow is connected to or away from, and those a deleted flow filled and drained, among them), the sim specs, each loop name -- must be as the session last read it with `read_model`, or as one of the session's own plans left it, whether the host landed that plan or the person declined it; an edit of what the person changed since is refused, naming it, and an edit before any read is refused. The agent's own landed edits do not make it re-read. Provenance is no change: who made a variable is not compared.
2. **One patch.** The operations are applied in order to a copy of the project, so each sees the ones before it; a field edit keeps every field it does not set, and a flow's connections keep its stocks' lists whole. A loop is named by its session id. An edit that can change the diagram places what it adds with the engine's incremental layout (a whole layout for a model with no diagram), inside the same patch, so placement undoes with the edit. The layout travels as an edit of the view's elements when that implies no model operation of its own (an edit of equations, which adds and removes connectors), which keeps what the view holds beyond its elements; an edit that adds, deletes or renames variables replaces the view, since an element edit would create again a variable the patch has created.
3. **The gate.** The patch is staged on the host's database, under a guard that restores the database however the gate ends, its diagnostics compared with the model's by code and variable (the model's mapped through the edit's renames, so renaming a variable that has an error is not refused for it), and its run with the model's run. It is refused if it adds an error the model did not have, asks more of its run than a run may (specs that cost more than the model's own and more than an experiment's limit), stops a model that simulated from simulating, makes a value not a number that the model's run had a number throughout, or gives a model with no unit warnings its first, the rule `simlin_project_apply_patch` holds a person's patch to. The staged diagnostics and the staged run are units of the call's work: a plan stops before each when other work waits for the project, and keeps nothing. Every warning it adds is listed so the agent fixes it. It tolerates, by design:
   - errors the model had already, so a broken model can be repaired a step at a time (a second error of a code a variable has already goes with the first);
   - errors in another model of the project, a module's, which count through whether the model simulates;
   - values not a number in a model that did not simulate before, which has no run to compare with.

   This is `simlin-mcp-core`'s gate, moved where every host gets it and made stricter.
4. **The plan**: one line per variable changed (a rename's rewritten readers included, a loop named, the sim specs), the diagnostics it would add, whether the result simulates, and the verdict, fitted to the answer budget (a line's detail cut at 240 characters, the lines past the budget counted). A plan that passes is kept in the session under an id (`P1`, ...). The patch never reaches the agent: a view's JSON is nothing it reads, and the person approves the plan's lines.
5. **Landing.** The host lands a plan by id once the person approves it (`Session::land_plan`, `simlin_tool_session_land_plan`), and a plan lands by construction, under the project's lock: at the revision it was planned at, its patch, gated against those very contents; at another, its operations planned again on the contents as they are, landed only when everything it writes is as it was when the plan was made, the gate passes again, and its lines are the ones the person approved. A person's diagram edits meanwhile stay, since the placement is made again around them. A refusal says why, for the agent to plan again. No host passes `allow_errors`, and the patch never travels as JSON, so a replaced view keeps its font and the metadata an MDL writer keeps with it.
6. **Provenance.** In a project that records who made its variables (ISEE's AI information: a letter per variable, `ai_state`), what an edit adds is marked made by AI (C) and what it edits marked edited by AI: a person's variable becomes D, or H once a person has edited it; an AI's stays C, or G once a person has edited it. A project that records no provenance gets none. The JSON forms carry `aiState`, so an edit through them keeps it too.

The patch JSON's types moved from libsimlin into the engine (`json::ProjectPatch`, `json` owning the project's JSON already), and convert to and from the engine's own `ProjectPatch`.

### Findings and verification

A finding is a flaw, a strength or an observation: a claim a person will read, and one to eight citations, each naming something a tool reported:

| Citation | Holds when |
|---|---|
| `variable` | the model has the variable |
| `equation` | the variable's equation (a stock's initial value; an element's, named by its subscript) parses the same as the cited one: spacing, case and the spelling of names aside. A variable with a table is its table at its equation's value, `LOOKUP(effect, input)`, and the input alone is not its equation |
| `value` | the variable (or an element) is within 5% of the cited value at the cited time in the run (of the larger of the two), or, for values near zero, within 1e-5 of the series' largest magnitude, the summary's precision, so a value that reads as zero beside the series is zero; the floor decides only when both values are under 0.02% of that magnitude, so on a series that spans orders of magnitude one small value never stands for another; at the run's start when no time is cited, and a time outside the run is refused |
| `readers` | the variables the variable links to (those whose equations read it, and a flow's stocks) are exactly the cited ones; none, for a variable nothing reads |
| `reads` | the variable reads the other, through a link of the cited sign by its equation when a sign is cited |
| `diagnostic` | the model has the diagnostic of that id now |
| `no_diagnostics` | the model has no diagnostic (of the cited category) |
| `loop` | the run has the loop of that id, of the cited polarity (a mostly reinforcing loop is reinforcing) |
| `leads` | the loop was its partition's strongest for at least half the steps of the span in which a loop was active |
| `no_loop_through` | no loop of the run goes through the variable, or the one element cited; unshowable when the run's loops are a sample |
| `goes_negative` | the variable, or an element of it, goes below zero in the run |
| `peaks_at` | the variable is at its largest within 5% of the run of the cited time; a series at rest has no peak, and a time outside the run is refused |
| `ends_near` | the variable ends at the cited value, as `value` judges one |
| `behavior_mode` | the classifier gives the variable the cited mode in the run |
| `compares` | the variable ends higher (or lower) in one run than another, by more than `value` would call the same number: runs that end a hair apart end alike |
| `test` | the battery check of that id comes out as cited, run again when the model has changed since |

`verify_findings` checks each citation against the session at the current revision, deterministically, so a host can refuse to show a claim whose evidence does not hold, and an evaluation can check claims without judgment. A citation that fails says what is true instead ("population ends at 100 in run 'current'"). A finding every citation of which holds gets an id (`F1`, ...), keyed by its kind and claim, which the host shows it under. A run made before the model changed (its diagrams aside) is not evidence about the model: a citation of one fails and names the repair. The equation, value and readers citations exist because claims kept outrunning the others: "the capacity is now 1500" verified while citing only the variable, and "nothing uses the harvest fraction" while citing only that no loop goes through it. A variable of a run may be an element (`Population[north]`); a fact about one series of an arrayed variable names the element. Whether a claim follows from its citations is a judgment the surface does not make.

## libsimlin surface

- `simlin_project_get_revision(project, out_revision, out_error)`.
- `simlin_tools_describe(out_buf, out_len, out_error)`: the catalog JSON in a `simlin_malloc` buffer.
- `SimlinToolSession`, refcounted, holding a reference to its model: `simlin_tool_session_new(model, out_error)`, `simlin_tool_session_ref`, `simlin_tool_session_unref`.
- `simlin_tool_session_call(session, name, input, input_len, out_buf, out_len, out_is_error, out_error)`: JSON in, JSON out. `out_is_error` carries a domain refusal; `out_error` only the host's misuse.
- `simlin_tool_session_get_changes(session, out_buf, out_len, out_error)`: the change report since the session's last `read_model`, as JSON, or `null`. What the session's own plans left once a host landed them is left out, as the agent's own work, so the report is the person's.
- `simlin_tool_session_get_run(session, name, out_revision, out_stale, out_error) -> *mut SimlinResults`: a named run's series for a host to chart, through the results handle `simlin_results_open_vdf` already returns, with the revision it was made at and whether it is stale.
- `simlin_tool_session_list_runs(session, out_buf, out_len, out_error)`: the named runs as JSON, oldest first, each with its revision, whether it is stale or gone, the run it started from, and everything it changed from the model, exactly as it ran: what a host's run list, chart picker and "run again" read.
- `simlin_tool_session_forget_run(session, name, out_forgotten, out_error)`: forget a run the person discards, its series and its plan, so no tool reads it again; a run made from it keeps what it changed.
- `simlin_tool_session_cancel(session)`: cancel the session's calls under way, the one answering and any waiting for the session, as a host does when the window they were for closes. A call made after it runs as usual.
- `simlin_tool_session_land_plan(session, id, out_buf, out_len, out_error)`: land a plan the person approved, on the project as it is, writing `{"landed": true}` or `{"landed": false, "reason"}`.

A call holds its session for the call, and the project's datamodel only while it takes the contents it answers from and their revision. The contents are shared, not copied (`ProjectContents::shared`); an edit that lands meanwhile copies them first, as any edit of shared contents does. The call then answers under the db lock alone, and the db stays synced to those contents while the call holds it, since a sync needs the db lock. A call that finds the db held -- by another session's call, or by a host's query that holds only the db -- waits for it with the datamodel released, then takes the contents again. So a host's hit tests, planners and revision reads, which lock only the datamodel, never wait behind an analysis, which on the largest models takes seconds.

Work that holds the datamodel while it waits for the db -- an edit landing or an undo, a simulation, a read of the diagnostics, loop discovery, a wasm compile, a render that lays out a model with no view -- keeps those readers waiting with it, so a call yields to it. libsimlin counts such waiters: `lock_db_with` and `built_db` count themselves while they wait, and a tool call's own lock does not, so two calls never stop for each other. A call checks the count between units of its work -- a slice of a simulation, or a stage of an analysis -- through `Workspace::waiting`, and stops there, answering a refusal marked `interrupted` that kept nothing. The waiter waits at most one unit, and the agent, or a host that retries by itself, calls again once the model is as the waiter left it: after an edit, at the next revision, and never in a loop against a project that stays busy, which would only take the database back from the person's work. Salsa's own cancellation would unwind the call, and a release build aborts on a panic; a second synced database for calls would double memory on exactly the models where calls are long.

A host cancels a call at the same checkpoints. Each call takes a ticket as it begins, before it waits for the session, and `simlin_tool_session_cancel` marks every ticket taken so far, with atomics alone, so any thread can cancel, and a later call is not cancelled. A call cancelled while it waits for its session answers as soon as it has it, without waiting for the db, which another session's call may hold. The engine's entry points fold `Workspace::cancelled` into what their checkpoints ask, so no checkpoint knows the difference, and a call a cancel stopped answers a refusal marked `cancelled`, not `interrupted`: nothing waits for it to be made again. Only `simlin_tool_session_call` is cancelled. The session is shared by everything a host shows of one model, and a host's own read of a run, or a landing, the person's own act, is not the agent's work. A loop analysis stops no sooner than its LTM compile ends: the compile is one query, and is not sliced. Planning an edit is a read: it stages the patch on the db and restores it, and leaves the revision. Landing a plan is a write, and holds the datamodel for the call as `simlin_project_apply_patch` does, so nothing lands between its check and its edit. It counts itself among the waiters from before it waits for its session, so a call on that session, which holds the session for the whole call, stops for it at its next checkpoint, as a call on another session stops for it at the database.

The entry points are behind libsimlin's `agent_tools` feature (the engine's feature of the same name gates `tools`), on by default and off in the browser wasm bundle, as `png_render` is.

## MCP and pysimlin

- **pysimlin** binds the catalog (`simlin.tools.catalog()`) and a `simlin.ToolSession(model)` for its users and for evaluation harnesses, which then exercise the same functions a native host ships. `call(tool, input)` answers a `ToolOutput(data, is_error)`: a refusal is output the harness scores like any other, and only a tool the catalog lacks raises. `run(name)` reads a session run as the DataFrame `Run.results` gives, `runs()` lists the session's runs with their revisions and staleness, and `land(id)` lands a plan through `simlin_tool_session_land_plan`, as a host does once the person approves, and commits it as the project commits any edit; a plan that cannot land says why.
- **simlin-mcp-core** mounts the catalog as MCP tools, each input schema extended with `projectPath` and `modelName`, holding one session per project path; `FileSystemAccess` supplies a content hash as the revision. The MCP host lands a plan with `Session::land_plan` and writes the project to its file. The catalog's tools replace `ReadModel` and `EditModel`; `CreateModel`, which makes a file, stays a tool of the host's own.

## Relationship to other plans

- **Single-mode LTM** (`docs/design-plans/2026-09-07-ltm-single-mode.md`). `analyze_loops` reads the post-simulation discovery pipeline that plan keeps, and reports runtime polarity. Its session loop ids isolate agents from the engine's id change. Its structural fallback for a run at rest reads `model_detected_loops` out of discovery mode, which that plan replaces with `model_structural_loops`; the fallback moves to it, and gains World3 and C-LEARN, whose structure today is too large to enumerate. It needs one thing that plan's D4 would delete: a link's static sign. At equilibrium every link score is zero, so a model at rest has no runtime sign for any link; `read_variables` reports the static polarity, and a chain signs a link by its equation when the run never scored it. The replacement this surface needs is "the runtime sign when the run scored the link, the static sign otherwise".
- **Diagnostics' reasons.** Every diagnostic the engine formats carries a reason, the raising site's or, failing that, what its code means. An agent needs both what went wrong and where, so a report gives the site's reason and quotes the span whenever the diagnostic has one; a parse error, whose site wrote no reason, is reported by its span alone, and the code's meaning is the reason only when there is neither.
- **The editing core.** `edit_model`'s additions are placed by the same incremental layout, and its patch is the one the editing core's patches go through.

## Testing

- Every tool is tested through `Session::call` over models built with `TestProject`, `open_xmile` or `open_vensim`, and read back through production queries; no tool has a second path for tests.
- Decision tables derive their rows from their enums: every tool in the catalog, every kind a variable can be, every field a change report names, every behavior mode, every battery test, every citation kind, every edit operation.
- The catalog freshness test; a test that every catalog schema accepts the inputs the tool's tests send and rejects an unknown field.
- Output bounds: an outline and a loop analysis of every corpus model the suite already compiles stay within their budget (World3 and C-LEARN, with the other heavy gates, under `#[ignore]`); the outline budget itself is tested with a test-only override and a small model, and the loop analysis's order of leaving things out with a constructed analysis.
- libsimlin's integration harness drives each entry point: the revision advances exactly on mutations (a row per mutating entry point and per read, as `contents_tests.rs` rows the hit index), a session call from a model handle to JSON, refusals, and NULL safety.

## Phases

1. **The session and the read tools.** The contents revision and `simlin_project_get_revision`; `tools::Session`, the evidence ids for diagnostics, the change report, names; the catalog and its freshness test; `read_model`, `read_variables` (without behavior), `find_variables`; libsimlin's describe, session and call entry points; the links core moved into the engine as the one owner of a model's causal links with their polarities.
2. **Experiments and behavior.** `run_experiment`, `read_behavior`, behavior modes, named runs and `simlin_tool_session_get_run`; `read_variables` gains its behavior line.
3. **Loops.** `analyze_loops` over a named run, with session loop ids, the equilibrium case and knockouts; `analysis::discover_run_loops` split out of `analyze_model`.
4. **The battery.** `run_tests`.
5. **Editing.** `edit_model`, the gate and plans; `simlin_tool_session_land_plan`; the patch JSON types in the engine.
6. **Verification.** `verify_findings`.
7. **pysimlin.** `simlin.ToolSession` and the catalog.
8. **The MCP mount.** The catalog as MCP tools, as above.
