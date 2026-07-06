# Conveyor support: specification and requirements

Status: proposed. This document specifies XMILE conveyor support for the simlin
engine. It is written to be implemented by a fresh engineer or agent with no
prior context on conveyors. It separates what the specs make **unambiguous**
from what **needs a decision** (flagged `DECISION:`) or **experimentation
against Stella** (flagged `EXPERIMENT:`). Items requiring the maintainer's input
are collected in [§12](#12-decisions-needed-from-the-maintainer).

## 1. Motivation

Conveyors are a first-class stock type in Stella / isee systems models, used
heavily for aging chains, disease-progression stages, and material-transport
structures. XMILE 1.0 marks them an OPTIONAL feature (§3.7.2, §4.2.1, §4.3).
Many real Stella `.stmx` models in the wild use them; without support, simlin
cannot faithfully import, round-trip, or simulate those models.

### Current behavior (verified against HEAD, 2026-07-05)

Conveyors are not merely unsupported — they fail **silently and confusingly**:

1. **Import drops the block.** The XMILE reader struct `xmile::Stock`
   (`src/simlin-engine/src/xmile/variables.rs`) has no field for `<conveyor>`,
   and quick-xml ignores unknown child elements, so the entire conveyor
   specification is discarded on import.
2. **Compilation then fails on the outflow.** A conveyor's outflow MUST NOT have
   an equation (the conveyor drives it — XMILE §4.3). With the conveyor block
   gone, that equation-less flow reaches the compiler as an ordinary flow and
   errors with `empty_equation` naming the outflow — a message that gives the
   modeler no hint the real problem is an unsupported conveyor.
3. **Export loses the block.** The `<uses_conveyor/>` header option round-trips
   (the `Feature::UsesConveyor` enum exists in `xmile/mod.rs`), but the
   `<conveyor>` block does not, so import-then-export **corrupts** a Stella
   model: the header advertises conveyors while the stocks have become plain
   stocks.

Reproduce with `test/conveyors/minimal_conveyor.xmile`:

```
$ simlin-cli simulate test/conveyors/minimal_conveyor.xmile
error in model 'main' variable 'graduating': empty_equation
```

Even the immediate Phase 0 goal (below) is a strict improvement: represent the
conveyor and replace this misleading error with a clear "conveyors are not yet
simulatable" diagnostic.

## 2. Concepts and vocabulary

A conveyor is a stock whose contents move along a fixed-length belt. Material
enters at the back, advances one **slat** per DT, and falls off the front after
the **transit time** has elapsed. Conceptually (isee "traditional conveyor"
model):

- The belt is a row of slats, one slat per DT. The number of slats is
  `transit_time / DT`. Each slat holds a quantity of material; slat 1 is the
  exit end.
- Each DT, every slat's contents shift one position toward the exit; slat 1's
  contents leave as the **outflow**; the inflow is deposited into the slat
  `transit_time/DT` positions from the exit.
- **Leakage** flows let material fall off partway along the belt (attrition,
  mortality). Leakage is linear (constant amount per DT) or exponential
  (constant fraction of remaining material per DT), optionally confined to a
  **leak zone** (a fractional span of the belt).
- Conveyors are **not FIFO by default**: if the transit time shrinks, material
  added later can exit earlier. Material already on the belt keeps advancing one
  slat/DT regardless of later transit-time changes.

Related XMILE stock modes, both OPTIONAL and both **out of scope for the first
milestones** (see phasing): **queues** (FIFO batch-tracking stocks) and isee
**ovens** (batch-process stocks, not an XMILE standard construct). Queues matter
here only because a conveyor can sit downstream of a queue and constrain it; see
[§9](#9-queues-and-conveyor-queue-coupling-later-phase).

## 3. XMILE syntax (unambiguous — from the spec)

Reference: OASIS XMILE v1.0, §2.2.1, §3.7.2, §4.2, §4.2.1, §4.3. Local copy:
`docs/reference/xmile-v1.0.html` (windows-1252 encoded; use `grep -a`).

### 3.1 Options header

A model using conveyors MUST declare it in the `<options>` block:

```xml
<uses_conveyor/>                       <!-- also spelled <uses_conveyors/> in an example; accept both -->
```

Two OPTIONAL boolean attributes advertise sub-features: `arrest="true|false"`
(default false) and `leak="true|false"` (default false). These are advisory;
the authoritative source of truth is the per-stock `<conveyor>` block.

### 3.2 The conveyor block (on a stock)

`<conveyor>` is one of three mutually exclusive stock options (`<conveyor>`,
`<queue/>`, `<non_negative/>`). The stock's `<eqn>` is the conveyor's initial
value (see [§5](#5-initialization)); its `<inflow>`/`<outflow>` lists name the
flows.

```xml
<stock name="Students">
  <eqn>1000</eqn>                      <!-- initial contents -->
  <inflow>matriculating</inflow>
  <outflow>graduating</outflow>        <!-- primary (belt-end) outflow -->
  <outflow>attriting</outflow>         <!-- leakage outflow (see 3.3) -->
  <conveyor discrete="false" batch_integrity="false"
            one_at_a_time="true" exponential_leak="false">
    <len>4</len>                        <!-- REQUIRED: transit time, in time units -->
    <capacity>1200</capacity>           <!-- OPTIONAL: max contents (default INF) -->
    <in_limit>500</in_limit>            <!-- OPTIONAL: max inflow per time unit (default INF) -->
    <sample>1</sample>                  <!-- OPTIONAL: when to re-latch transit time (default: every DT) -->
    <arrest>0</arrest>                  <!-- OPTIONAL: when nonzero, conveyor stops (default: never) -->
  </conveyor>
</stock>
```

Element/attribute reference:

| Tag / attr | Kind | Type | Default | Meaning |
|---|---|---|---|---|
| `<len>` | element | XMILE expr | REQUIRED | Transit time in time units. |
| `<capacity>` | element | XMILE expr | INF | Max material on the belt at any instant. |
| `<in_limit>` | element | XMILE expr | INF | Max material entering per **time unit**. |
| `<sample>` | element | XMILE cond expr | 1 (every DT) | When nonzero, re-latch the transit time from `<len>` for newly entering material. |
| `<arrest>` | element | XMILE cond expr | 0 (never) | When nonzero, all flows forced to zero and belt frozen. |
| `discrete` | attr | bool | false | Discrete (integer batches) vs continuous stream. |
| `batch_integrity` | attr | bool | false | Only whole upstream-queue batches may be taken (queue-upstream only). |
| `one_at_a_time` | attr | bool | true | Take only the front queue batch per DT (queue-upstream only). |
| `exponential_leak` | attr | bool | false | Exponential (vs linear) leakage, for all leak flows. |

### 3.3 Leakage flows

The first `<outflow>` is the primary belt-end outflow; the rest are leakages
(XMILE §3.7.2, §4.2.1). A leakage flow SHOULD be explicitly tagged. A conveyor
outflow MUST NOT carry a normal `<eqn>` — the conveyor drives it. Real Stella
models put the leak **fraction** in the `<eqn>` of a `<leak/>`-tagged flow:

```xml
<flow name="attriting" leak_start="0" leak_end="0.25">
  <eqn>0.1</eqn>                        <!-- leak fraction: 10% by exit time -->
  <non_negative/>
  <leak/>                               <!-- marks this outflow as a leakage -->
  <leak_integers/>                      <!-- OPTIONAL: leak only whole units -->
</flow>
```

Note the encoding wrinkle observed in real models (both peterhovmand corpus and
the moose model): the leak fraction lives in `<eqn>`, and `<leak/>` /
`<leak_integers/>` are empty sibling marker tags. The spec also shows a
`<leak>0.1</leak>` element form. **The reader must accept both** the
marker-`<leak/>`-plus-`<eqn>` form and the value-bearing `<leak>expr</leak>`
form. `<leak/>` with no fraction yet is a valid "leakage, fraction TBD" marker
(used mid-edit) that does not simulate.

Leakage flow options:

| Tag / attr | Type | Default | Meaning |
|---|---|---|---|
| `<leak>` / `<leak/>` | expr / marker | — | Leak fraction (fraction of inflowing material leaked by exit), or bare marker. |
| `<leak_integers/>` | marker | off | Leak only whole units; accumulate the fraction until ≥ 1, then leak one unit. |
| `leak_start` | attr in [0,1] | 0 | Fractional belt position where the leak zone starts (from the inflow side). |
| `leak_end` | attr in [0,1] | 1 | Fractional belt position where the leak zone ends. |

### 3.4 Non-negativity

Conveyor and queue **inflows** MUST be non-negative (uniflow); the primary
conveyor outflow is non-negative by definition. `<non_negative/>` MUST NOT appear
on those flows. (XMILE §4.3.) The reader should tolerate its presence in
real-world files rather than hard-erroring, since Stella emits `<non_negative/>`
on leak flows.

## 4. Simulation semantics

This is the substance. Sources are the OASIS spec (behavioral prose) plus isee's
"Computational Details" help pages, which are the only place the per-DT math is
documented. Key isee pages (verify against these when implementing; the
"traditional conveyor" page is the richest and is the model simlin most likely
needs to match):

- Traditional (non-FIFO) conveyors:
  `iseesystems.com/resources/help/v2/Content/08-Reference/05-Computational_Details/TraditionalConveyors.htm`
- FIFO conveyors:
  `.../v3/.../FIFOConveyors.htm`
- Spreading conveyor inputs:
  `.../v3/.../SpreadingConveyorInputs.htm`
- Initializing discrete stocks:
  `.../v3/.../InitializingDiscreteStocks.htm`
- Equation tab (capacity/in_limit/sample/arrest/discrete/split/FIFO):
  `.../v3/.../07-SharedProperties/Equation_tab.htm`

### 4.1 The slat model (clear)

- Slat count `N = transit_time / DT`. Slat 1 is the exit; slat N is the entry.
- Per DT: outflow = contents of slat 1; all slats shift toward the exit; inflow
  is placed into the entry slat.
- The conveyor's scalar/reported value = the sum of all slat contents (total
  material on the belt). *(Inferred, consistent with the steady-state formulas;
  not stated verbatim — `EXPERIMENT` to confirm.)*

### 4.2 Per-DT update order (clear)

Each DT, for each conveyor:

1. If `<arrest>` evaluates nonzero: force all inflows and outflows to zero, do
   not advance the belt, skip to next conveyor.
2. Compute the admitted inflow, clipped by capacity and inflow limit
   ([§4.5](#45-capacity-and-inflow-limit-pushback)). Un-admitted material stays
   upstream.
3. Advance: outflow = slat 1; shift every slat one position toward the exit.
4. Apply leakage to the (now shifted) belt in leak-flow priority order
   ([§4.3](#43-leakage)).
5. Deposit admitted inflow into the entry slat (position `N` from the exit,
   using the transit time latched per [§4.4](#44-transit-time-changes)).

`DECISION:` the exact interleave of steps 3–5 (does inflow land before or after
the shift; is leakage before or after the outflow is taken) determines
first-DT numerics. Pin it against a Stella reference run on
`minimal_conveyor.xmile` before finalizing. The ordering above is the best
reading of the isee prose but is not guaranteed bit-exact.

`EXPERIMENT:` **non-integer `N = transit_time/DT`.** The docs give no rounding
rule, and the traditional (integer-slat) and FIFO (fractional-edge-slat) pages
imply different handling. Options: round to nearest integer slat count; or carry
a fractional head/tail slat. Must be resolved experimentally. Recommend the
engine **reject a non-integer ratio with a clear error** in Phase 1 and only add
fractional-slat support if a reference model needs it.

### 4.3 Leakage (mostly clear; formulas verify-against-isee)

Leak fraction `f` = the fraction of inflowing material that leaks out by the
time it reaches the exit.

- **Linear** (`exponential_leak=false`): "the same amount is taken away every
  DT." Total leaked over the belt equals `f`. Distributed across the slats the
  material occupies, so per-slat leak ≈ `f × (amount that entered) / N`. isee
  example: 8% over 8 DTs = 1%/DT. Constraint: total linear leak fraction across
  all leak flows ≤ 1; a fraction of 1 leaves no outflow.
  - Under a leak zone `[leak_start, leak_end]`: the total leaked is still `f`
    regardless of zone length — a shorter zone leaks *more per slat*.
  - Under a variable transit time: the amount leaked is based on the transit
    time in force **when the material entered**.
- **Exponential** (`exponential_leak=true`): "the same fraction of remaining
  material is removed every DT." Per-slat per-DT leak = `material_in_slat × f ×
  DT`. Overlapping leak zones add their fractions. Bases leakage on current belt
  contents, independent of entry transit time.
- **Multiple leak flows**: applied in the order the outflows are listed (first =
  highest priority).
- **`<leak_integers/>`**: accumulate the fractional leak until it exceeds 1,
  then leak exactly one unit; carry the remainder.

`EXPERIMENT:` the precise per-slat apportionment (especially linear leak with a
partial zone, and the exact denominator) is prose-only in the isee docs. Derive
the exact recurrence and validate against `covid19_severity.stmx` (which uses
`exponential_leak="true"` leak flows) and a hand-authored linear-leak fixture.

`DEFER:` isee also documents an "ignore losses from earlier leak zones" flag and
five "spreading conveyor inputs" placement methods (at-beginning is the default
XMILE behavior; evenly / destination-profile / distribution / source-based are
isee extensions). These are not XMILE-standard `<conveyor>` options and are out
of scope; do not implement until a real model requires them.

### 4.4 Transit-time changes (clear-ish)

- Default conveyors are **non-FIFO**. Material already on the belt keeps
  advancing one slat/DT; a transit-time change only affects where **newly
  entering** material is placed (slat `N_new = new_transit_time / DT` from the
  exit). If the transit time drops, later material can exit before earlier
  material.
- `<sample>`: the transit time is re-read from `<len>` only on DTs when the
  sample expression is nonzero; between samples the last latched value is used
  for placement. Default is every DT.
- FIFO mode (isee `<isee:...>` extension / Equation-tab checkbox) preserves
  content order across transit-time changes and, on a shortening, doubles the
  outflow until the old-transit material is exhausted. `DEFER:` treat FIFO as a
  later enhancement; the XMILE-standard behavior is non-FIFO.

### 4.5 Capacity and inflow limit (pushback) (clear formulas, subtle plumbing)

Both default to INF. Per the isee Equation-tab page:

- **Capacity**: `admitted_inflow_volume = MIN((capacity - current_contents)/DT +
  total_outflow_volume, requested_inflow_volume)` per DT (volumes = rate×DT).
- **Inflow limit** (per **time unit**, not per DT):
  - Continuous: `admitted = MIN(in_limit, requested)` after multiplying the
    per-time-unit limit into a per-DT budget (`DT × in_limit`).
  - Discrete: the full per-time-unit budget may enter within a single DT; the
    running total resets at each integer time unit.
  - Inflow limit is unavailable when the inflow comes from another conveyor.
- **Blocked material stays upstream.** If the upstream is an ordinary stock, the
  un-admitted inflow simply is not removed from that stock. This is the subtle
  part for simlin's architecture: **a conveyor's admitted inflow depends on the
  conveyor's state, so the inflow's effective value is not a pure function of
  its own equation** — it is clipped by the downstream conveyor. See
  [§6.4](#64-the-inflow-clipping-problem).

`DECISION:` apportioning a capacity/limit clip among multiple simultaneous
inflows to one conveyor is only partly documented (creation order). Pick a rule
(proportional, or priority by inflow order) and document it.

### 4.6 Arrest (clear)

When `<arrest>` is nonzero: all inflows and outflows to the conveyor are forced
to zero and the belt freezes in place (material is not lost, time suspends for
it). It resumes when the expression returns to zero.

### 4.7 Discrete conveyors (partly clear)

`discrete=true` moves material as integer chunks that exit as discrete lumps
rather than a smooth stream; the initial value seeds the start of each time unit
rather than being spread evenly ([§5](#5-initialization)); the inflow-limit
budget is per-time-unit (fillable in one DT) rather than per-DT. `EXPERIMENT:`
exact chunk quantization and remainder handling are qualitative in the docs.
`discrete=true` is REQUIRED when there is a queue directly upstream.

## 5. Initialization

The stock `<eqn>` gives the initial contents. Two modes:

- **Scalar initial value** → steady-state distribution. Stella distributes the
  value so the belt is in equilibrium. Documented closed forms (valid only when
  `transit_time` is a multiple of DT and leak zones span 0→100%):
  - No leak: contents `= inflow × transit_time`, i.e. the scalar value is spread
    evenly, `value/N` per slat.
  - Linear leak fraction `f`:
    `inflow × (transit_time × (1 − f/2) + f × DT / transit_time)`.
  - Exponential leak fraction `f`:
    `inflow × (1 − (1 − f×DT)^(transit_time/DT)) / f`.
  These relate the scalar initial value, the equilibrium inflow, and the
  per-slat fill. `EXPERIMENT:` confirm which quantity the `<eqn>` value denotes
  (total contents vs equilibrium inflow) — the isee steady-state blog gives an
  alternate outflow-first formulation; do not guess.
- **Explicit per-slat list** (comma-separated) → each entry is the quantity for
  one **time unit** (not one DT); with fractional DT the value is repeated across
  that unit's DTs. List length must equal `transit_time` or `transit_time/DT`
  (the latter is required for non-integer transit times); too many → truncate,
  too few → repeat the last entry.

Phase 1 can implement only the scalar even-spread no-leak case and defer the
leak/explicit-list cases.

## 6. Engine architecture and integration points

This section maps the feature onto simlin's actual code. Read
`src/simlin-engine/CLAUDE.md` for the compilation pipeline.

### 6.1 Data model (`src/simlin-engine/src/datamodel.rs`)

`datamodel::Stock` (line ~306) needs to carry an optional conveyor spec, and
`datamodel::Flow` needs optional leakage metadata. Suggested shape:

```rust
pub struct Conveyor {
    pub transit_time: String,          // <len> expression (required)
    pub capacity: Option<String>,      // <capacity>
    pub inflow_limit: Option<String>,  // <in_limit>
    pub sample: Option<String>,        // <sample>
    pub arrest: Option<String>,        // <arrest>
    pub discrete: bool,
    pub batch_integrity: bool,
    pub one_at_a_time: bool,           // default true
    pub exponential_leak: bool,
}
// Stock gains: pub conveyor: Option<Conveyor>

pub struct Leakage {
    pub fraction: Option<String>,      // <leak> expr, or None for bare <leak/>
    pub integers: bool,                // <leak_integers/>
    pub zone_start: Option<String>,    // leak_start (default 0)
    pub zone_end: Option<String>,      // leak_end (default 1)
}
// Flow gains: pub leakage: Option<Leakage>
```

### 6.2 Protobuf (`src/simlin-engine/src/project_io.proto`) — compatibility-critical

Protobuf is the one place backward compatibility is REQUIRED (there is a DB of
serialized instances). `Variable.Stock` uses field numbers up to 12 and
`Variable.Flow` up to 12. **Add new fields with fresh, never-reused field
numbers** (e.g. `optional Conveyor conveyor = 13;` on Stock, `optional Leakage
leakage = 13;` on Flow) and define new `Conveyor`/`Leakage` messages. Absent
fields decode to `None`, so old serialized stocks remain valid plain stocks.
Regenerate with `pnpm build:gen-protobufs`. Never renumber or repurpose an
existing field.

### 6.3 Readers/writers

- XMILE reader/writer (`src/simlin-engine/src/xmile/variables.rs`): add
  `conveyor: Option<Conveyor>` and the `<element>`-level plumbing to `Stock`,
  and `leak`/`leak_integers`/`leak_start`/`leak_end` to `Flow`. Accept both
  leak encodings ([§3.3](#33-leakage-flows)). Emit the `<conveyor>` block and
  the `<uses_conveyor/>` option (already modeled as `Feature::UsesConveyor`).
  This alone fixes the round-trip corruption, independent of simulation.
- MDL: Vensim has no conveyor primitive. `delay conveyor` is recognized as a
  builtin name in `src/simlin-engine/src/mdl/builtins.rs` but is a distinct
  (unimplemented) function — **out of scope**; do not conflate.
- JSON (`src/json.rs`), TypeScript datamodel (`src/core`), diagram: extend once
  the engine representation is settled.

### 6.4 The inflow-clipping problem (the hard architectural question)

simlin compiles each variable to a bytecode fragment evaluated in dependency
order; a flow's value is a pure function of its equation. A conveyor breaks two
assumptions:

1. **A conveyor is stateful beyond a single f64.** A stock today is one slot in
   the value arrays. A conveyor needs a per-instance ring buffer of `N` slats
   (plus latched transit time, leak accumulators). Precedents for side-state to
   study: graphical-function storage, `prev_values`/`initial_values` snapshot
   buffers in `vm.rs`, and module instance state.
2. **A conveyor outflow has no equation, and its inflow may be clipped by the
   conveyor's own capacity/limit.** So the outflow value and the *effective*
   inflow value must be produced by conveyor-update logic that runs at a
   well-defined point in the step, not by per-variable equation evaluation. This
   resembles how non-negative stocks would clip outflows, and how queues drive
   their outflows.

`DECISION:` the core implementation strategy. Two broad options:

- **(A) New opcode / VM-native conveyor object.** Represent the conveyor as a
  dedicated runtime object with its own update step, wired into the flows/stocks
  phases. Cleanest semantics, most engine work (VM + wasmgen + compiler +
  layout offsets).
- **(B) Desugar to existing primitives at compile time.** Lower a conveyor to an
  array of `N` aux/stock slats plus generated flow equations (a fixed-length
  aging chain). Reuses the existing array + stock machinery, but `N` depends on
  `transit_time/DT` (so it must be a compile-time constant — rules out a
  variable `<len>` unless re-lowered) and capacity/limit pushback on the inflow
  is awkward to express as equations.

Recommend prototyping (A) for a single continuous constant-transit no-leak
conveyor first, since it generalizes to leakage/variable-transit; but the
maintainer should choose (see [§12](#12-decisions-needed-from-the-maintainer)).

### 6.5 wasmgen

The WebAssembly backend mirrors the VM opcode-for-opcode and must either support
the conveyor path or return `WasmGenError::Unsupported` — **there is no silent
VM fallback** (established rule, see `src/simlin-engine/src/wasmgen`). Phase 1
may legitimately return `Unsupported` for conveyor models from wasmgen while the
VM path works, as long as it is loud.

### 6.6 Integration method (Euler)

isee documents that Runge-Kutta "doesn't deal well with queues, conveyors, or
ovens" and recommends Euler, but does **not** state that Stella forces Euler or
errors. simlin has precedent for an Euler-only hard rejection: GH #486 rejects
non-Euler integration for LTM link scores (see `src/simlin-engine/src/db/
assemble.rs` around the "GH #486" comments). `DECISION:` for conveyors, either
(a) hard-error when a conveyor is present under RK2/RK4, or (b) silently force
Euler for the whole model. `EXPERIMENT:` check what Stella actually does. The
slat model is inherently a per-DT (Euler-like) construct; RK sub-steps have no
meaning for it. Recommend a hard, clear error in Phase 1.

### 6.7 LTM (Loops That Matter)

A conveyor is a stock with special dynamics; the flow-to-stock link-score
formula assumes plain INTEG (and Euler — GH #486). Conveyor internal dynamics
are not INTEG. Minimum bar: LTM analysis over a model containing a conveyor must
**degrade loudly** (a clear warning / documented limitation), never crash or
emit a silently-wrong score. Full LTM-through-conveyor semantics are out of
scope for the initial milestones.

## 7. Container access and builtins (later phase)

XMILE §3.7.1.3 requires that array builtins (MIN/MAX/MEAN/STDDEV/SUM/SIZE) and
`[]` indexing work over a conveyor's contents: `conv[3]` reads the third slat
from the front; `SIZE`/`MEAN` etc. operate on the slat vector; an arrayed
conveyor uses two subscript groups `conv[arr_subscript][slat]`. isee also
exposes cycle-time builtins on time-stamped material (`CTMEAN`, `CTSTDDEV`,
`CTMAX`, `CTMIN`, `CTFLOW`, `CYCLETIME`, `THROUGHPUT`). All of this is
**deferred** — it depends on the conveyor holding an inspectable slat vector,
which the core simulation must exist first.

## 8. Arrayed conveyors (later phase)

`covid19_severity.stmx` has conveyors dimensioned over a `Severity` dimension.
The XMILE non-apply-to-all array rules (§4.5.2) allow per-element transit times.
The isee docs document only the indexing syntax, not distinct per-element
computational semantics — they appear to behave as independent per-element
conveyors. Treat as a Phase-3 concern; the core scalar conveyor must work first.

## 9. Queues and conveyor–queue coupling (later phase)

Queues (`<queue/>`) are a separate XMILE OPTIONAL feature (§3.7.3) with their own
`<uses_queue>` option and `<overflow/>` flow property. They are FIFO
batch-tracking stocks. They intersect conveyors because a conveyor downstream of
a queue constrains the queue's outflow (capacity/inflow-limit/arrest), and the
conveyor's `batch_integrity` / `one_at_a_time` / discrete-required flags only
apply in that configuration. **Queues are out of scope for the initial conveyor
milestones** and should be specced separately; a conveyor with an ordinary stock
or cloud upstream is fully implementable without them.

## 10. Phasing

- **Phase 0 — represent & round-trip (no simulation).** Datamodel + proto +
  XMILE reader/writer. Import preserves the conveyor block; export re-emits it;
  round-trip no longer corrupts. Replace the misleading `empty_equation` error
  with a clear "conveyors are not yet simulatable" diagnostic on the conveyor
  outflow. Vendored models parse without dropping data.
- **Phase 1 — simulate the simple continuous conveyor.** Constant transit time
  (integer `transit_time/DT`), scalar even-spread initial value, single primary
  outflow, no leakage, Euler only. Capacity + inflow limit with upstream-stock
  pushback. wasmgen may return `Unsupported`. Oracle: `minimal_conveyor.xmile`
  vs a Stella reference run.
- **Phase 2 — leakage & variable transit.** Linear + exponential leakage, leak
  zones, `leak_integers`, variable `<len>` with `<sample>`, arrest, discrete
  conveyors, explicit-list initialization. Oracles from the peterhovmand corpus.
- **Phase 3 — arrays, container access, builtins.** Arrayed conveyors, `[]`
  slat access, array/cycle-time builtins over contents. wasmgen parity.
- **Phase 4 — queues** (separate spec) and queue–conveyor coupling
  (batch_integrity, one_at_a_time, overflow).

## 11. Test oracles

Vendored under `test/conveyors/` (see its README for full provenance, licenses,
and attribution — the CC BY 4.0 attribution requirement is satisfied there):

- `minimal_conveyor.xmile` — hand-authored Phase-1 target (transit time +
  capacity, single outflow, no leak).
- `sir_social_distancing_mixnot.stmx` — peterhovmand corpus, CC BY 4.0,
  transit-time-only, deterministic; simplest real oracle.
- `covid19_severity.stmx` — peterhovmand corpus, CC BY 4.0, leakage + arrayed
  conveyors, deterministic.

**None ship expected-output CSVs**, and the real models additionally use
non-conveyor features simlin doesn't yet support (the isee builtin `LOOKUPMEAN`,
some unit-consistency issues, `PREVIOUS`, `isee:spreadflow`). So they are
**reference fixtures, not wired into `tests/integration/main.rs` yet**. Turning
them into executable oracles requires (a) conveyor support and (b) reference
output generated from Stella. Simlin's test convention is
`test/<name>/model.xmile` + `output.csv`, wired as a `mod` in
`tests/integration/main.rs`; follow it when oracles exist.

No open-source model was found that exercises `<capacity>`, `<in_limit>`,
`<discrete>`, `<arrest>`, or an upstream queue — those need hand-authored
fixtures plus Stella reference output when the corresponding phase is built.

## 12. Decisions needed from the maintainer

Collected `DECISION:` / `EXPERIMENT:` items and scoping questions, in rough
priority order:

1. **Core implementation strategy** ([§6.4](#64-the-inflow-clipping-problem)):
   VM-native conveyor object (A) vs compile-time desugar to a slat aging chain
   (B). This shapes everything downstream. Recommendation: (A).
2. **Oracle generation.** Do you have Stella access to produce reference-output
   CSVs? Without it, Phase 1+ can only be validated by hand-reasoned expected
   values on tiny models. This gates how much we can trust the numerics.
3. **Non-integer `transit_time/DT`** ([§4.2](#42-per-dt-update-order-clear)):
   reject with an error, round to an integer slat count, or carry fractional
   slats? Recommendation: reject in Phase 1.
4. **Non-Euler integration** ([§6.6](#66-integration-method-euler)): hard-error
   vs silently force Euler when a conveyor is present? Recommendation: hard
   error, following the GH #486 pattern.
5. **Queue scope.** Confirm queues (and thus batch_integrity / one_at_a_time /
   discrete-required coupling) are out of scope for now, to be specced
   separately.
6. **Per-DT update interleave & leakage apportionment**
   ([§4.2](#42-per-dt-update-order-clear),
   [§4.3](#43-leakage-mostly-clear-formulas-verify-against-isee)): these are
   `EXPERIMENT` items that need a Stella reference to pin bit-exactly; acceptable
   to defer until oracle generation is sorted.
7. **UI / diagram treatment.** When (if in this effort) should the diagram
   editor render and let users author conveyor stocks and leak flows? Currently
   unscoped.
