# Conveyor support: specification

Status: proposed. This is a complete, implementable specification of XMILE
conveyor support for the simlin engine, written for an engineer or agent with no
prior conveyor context. It commits to concrete algorithms and formulas: every
per-DT rule, leakage formula, initialization rule, and edge case is specified
here, not left open. Where a rule is derived rather than quoted verbatim from a
vendor spec, it says so and defines simlin's behavior authoritatively; the
vendored fixtures ([§13](#13-test-oracles)) are the regression oracles that pin
the numerics.

Sources: the OASIS XMILE 1.0 spec (`docs/reference/xmile-v1.0.html`; windows-1252,
use `grep -a`) for syntax and prose semantics, and isee systems' "Computational
Details" help pages for the per-DT math. The isee "traditional conveyor" model
is the reference behavior simlin implements.

## 1. Motivation

Conveyors are a first-class stock type in Stella / isee systems models, used for
aging chains, disease-progression stages, and material-transport structures.
XMILE 1.0 marks them OPTIONAL (§3.7.2, §4.2.1, §4.3). Many real `.stmx` models
use them; without support simlin cannot faithfully import, round-trip, or
simulate those models.

### Current behavior (verified against HEAD, 2026-07-05)

Conveyors fail **silently and confusingly** today:

1. **Import drops the block.** The XMILE reader struct `xmile::Stock`
   (`src/simlin-engine/src/xmile/variables.rs`) has no field for `<conveyor>`;
   quick-xml ignores unknown child elements, so the conveyor spec is discarded.
2. **Compilation then fails on the outflow.** A conveyor outflow MUST NOT have an
   equation (the conveyor drives it — XMILE §4.3). With the block gone, that
   equation-less flow errors with `empty_equation`, giving no hint that the real
   problem is an unsupported conveyor.
3. **Export loses the block.** The `<uses_conveyor/>` header option round-trips
   (`Feature::UsesConveyor` in `xmile/mod.rs`) but the `<conveyor>` block does
   not, so import-then-export **corrupts** a Stella model.

Reproduce with `test/conveyors/minimal_conveyor.xmile`:

```
$ simlin-cli simulate test/conveyors/minimal_conveyor.xmile
error in model 'main' variable 'graduating': empty_equation
```

## 2. Concepts and vocabulary

A conveyor is a stock whose contents ride a belt of fixed length. Material enters
at the back, advances one **slat** per DT, and falls off the front after the
**transit time** elapses.

- The belt is an ordered list of slats, one slat = one DT of travel. The slat
  count is `N = transit_time / DT`. Slat 1 is the exit (front); the highest-index
  slat is the entry (back). Each slat holds a real quantity of material.
- Each DT: leakage is removed, the exit slat's contents leave as the primary
  **outflow**, every slat shifts one position toward the exit, and the admitted
  inflow is deposited at the entry.
- **Leakage** flows let material fall off partway (attrition, mortality), linear
  or exponential, optionally confined to a **leak zone** (a fractional span of
  the belt).
- Conveyors are **not FIFO**: if the transit time shrinks, material added later
  can exit earlier. Material already on the belt keeps advancing one slat/DT
  regardless of later transit-time changes.

Two related XMILE stock modes are distinct objects, not conveyors: **queues**
(`<queue/>`, FIFO batch-tracking stocks — [§11](#11-queues-and-the-conveyor-side-of-queueconveyor-coupling))
and isee **ovens** (batch processors, not XMILE-standard — not specified here).
They intersect conveyors only when a queue sits directly upstream of a conveyor.

## 3. XMILE syntax

Reference: OASIS XMILE v1.0 §2.2.1, §3.7.2, §4.2, §4.2.1, §4.3.

### 3.1 Options header

```xml
<uses_conveyor/>          <!-- an example in the spec also spells it <uses_conveyors/>; accept both -->
```

Two OPTIONAL boolean attributes advertise sub-features: `arrest="true|false"`
(default false) and `leak="true|false"` (default false). They are advisory; the
authoritative source is the per-stock `<conveyor>` block. simlin emits
`<uses_conveyor/>` whenever any stock in the project is a conveyor, and sets
`arrest`/`leak` to reflect whether any conveyor uses those features.

### 3.2 The conveyor block (on a stock)

`<conveyor>` is one of three mutually exclusive stock options (`<conveyor>`,
`<queue/>`, `<non_negative/>`). The stock's `<eqn>` is the conveyor's initial
value ([§7](#7-initialization)); `<inflow>`/`<outflow>` name the flows.

```xml
<stock name="Students">
  <eqn>1000</eqn>
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
(XMILE §3.7.2, §4.2.1). A conveyor outflow MUST NOT carry a normal `<eqn>`. Real
Stella models put the leak **fraction** in the `<eqn>` of a `<leak/>`-tagged
flow:

```xml
<flow name="attriting" leak_start="0" leak_end="0.25">
  <eqn>0.1</eqn>                        <!-- leak fraction -->
  <non_negative/>
  <leak/>                               <!-- marks this outflow as a leakage -->
  <leak_integers/>                      <!-- OPTIONAL: leak only whole units -->
</flow>
```

The reader MUST accept both encodings: the marker-`<leak/>`-plus-`<eqn>` form
(what the vendored fixtures use) and the value-bearing `<leak>expr</leak>` form
(the spec's example). A `<leak/>` with no fraction and no `<eqn>` is a valid
"leakage, fraction TBD" marker (used mid-edit); it parses and represents but
contributes zero leakage until a fraction is supplied.

| Tag / attr | Type | Default | Meaning |
|---|---|---|---|
| `<leak>` / `<leak/>` | expr / marker | 0 | Leak fraction (interpretation depends on linear vs exponential — [§5](#5-leakage)). |
| `<leak_integers/>` | marker | off | Leak only whole units. |
| `leak_start` | attr in [0,1] | 0 | Fractional belt position where the leak zone starts (from the inflow side). |
| `leak_end` | attr in [0,1] | 1 | Fractional belt position where the leak zone ends. |

### 3.4 Non-negativity

Conveyor and queue **inflows** are non-negative by requirement (uniflow); the
primary conveyor outflow is non-negative by definition. `<non_negative/>` is
redundant on those flows but Stella emits it on leak flows, so the reader accepts
it there without error and ignores it on the primary inflow/outflow.

### 3.5 isee spread-input attributes

Real Stella conveyors may carry isee vendor attributes controlling where inflow
lands on the belt: `isee:spreadflow="beginning|even|dest|dist|source"` on an
inflow, with `<isee:distrib_eq>` naming a distribution for the `dist` method.
These are specified in [§8](#8-inflow-placement-spread-inputs); the reader MUST
parse and preserve them.

## 4. Runtime model and the per-DT algorithm

This section is the core. It defines the exact state and update; §5–§8 fill in
leakage, capacity, transit-time changes, and placement.

### 4.1 Slat count and non-integer transit times

For a conveyor with transit time `T` (the latched value — [§6](#6-variable-transit-time-sample-and-len)) and time step `DT`:

- `N = round(T / DT)`, rounding half away from zero, clamped to `N ≥ 1`.
- The **effective transit time** is `N × DT`. When `T` is not an integer multiple
  of `DT` (`|T/DT − round(T/DT)| > 1e-9`), the compiler emits a **Warning**
  naming the conveyor and reporting the effective transit time. simlin does not
  model fractional slats: the belt is DT-quantized, matching how isee's slat
  model discretizes. This is a deliberate, documented divergence from a
  fractional-belt reading of the FIFO help page; it is deterministic and keeps
  the belt an integer array.

`T ≤ 0` is a compile **error** (a conveyor needs a positive transit time).

### 4.2 Conveyor runtime state

Each conveyor instance owns side state, held in a table parallel to the VM's
existing `graphical_functions` / `prev_values` buffers (see
[§9](#9-engine-integration)):

```
ConveyorState {
    slats:   Deque<Slat>,   // index 0 = exit (front); back = entry
    latched_transit: f64,   // transit time last sampled (see §6)
    leak_carry: Vec<f64>,   // per leak-flow accumulator for <leak_integers/>
    in_carry:  f64,         // per-time-unit inflow-budget accumulator (discrete / in_limit)
}
Slat { content: f64, entry_amount: f64 }   // entry_amount = the cohort's admitted volume at entry
```

`entry_amount` is carried so **linear** leakage (an absolute amount tied to the
entering cohort — [§5](#5-leakage)) can be computed as material decays. For a
constant transit time the deque has a fixed length `N`; a variable transit time
grows/shrinks it ([§6](#6-variable-transit-time-sample-and-len)).

The conveyor variable's **reported scalar value** is the sum of all slat
contents (total material on the belt). This is simlin's defined semantics and is
consistent with the steady-state initialization ([§7](#7-initialization)).

### 4.3 Per-DT update

Conveyors integrate under Euler only ([§9.4](#94-integration-method)). Within
each Euler step, after ordinary flow equations are evaluated and before stocks
are updated, run this sequence for each conveyor, in topological order of the
conveyor chain (upstream conveyors before downstream, so a downstream conveyor's
pull is visible to its upstream source this same step):

Let `contents₀ = Σ slats.content` at step start.

1. **Arrest.** Evaluate `<arrest>`. If nonzero: set every inflow and every
   outflow of this conveyor to 0 for this step, leave `slats` unchanged, and skip
   steps 2–6. (Material is frozen, not lost.)

2. **Leak.** For each leak flow `k` in outflow-list order, for each slat `i`
   within `k`'s zone ([§5.3](#53-leak-zones)), compute `leak_{k,i}` per
   [§5.1](#51-linear-leakage)/[§5.2](#52-exponential-leakage), clamped so the
   running total removed from slat `i` this step never exceeds `slats[i].content`
   (earlier leak flows have priority). Subtract from `slats[i].content`. Leak
   flow `k`'s reported **rate** = `(Σ_i leak_{k,i}) / DT`. Apply
   `<leak_integers/>` quantization per [§5.4](#54-integer-leakage).

3. **Outflow.** Primary outflow **volume** = `slats[0].content` (post-leak).
   Primary outflow **rate** = `slats[0].content / DT`.

4. **Admit inflow.** Evaluate each inflow equation to a requested rate; requested
   **volume** = `rate × DT`, summed over inflows in listed order as
   `req_vol`. Compute:
   - `contents_after = contents₀ − (Σ leak this step) − slats[0].content`
     (contents once leak and outflow are removed).
   - `cap_room = capacity == INF ? INF : max(0, capacity − contents_after)`.
   - `limit_vol` = the inflow-budget cap per [§6.3](#63-capacity-and-inflow-limit).
   - `admitted = min(req_vol, cap_room, limit_vol)`.
   - Apportion `admitted` across the inflows **in listed order** (fill the first
     inflow's request fully, then the second, …). Each inflow's **reported rate**
     = its apportioned share / DT. Un-apportioned material is not removed from the
     upstream stock (its outflow-to-conveyor is exactly the admitted rate), so
     blocked material accumulates upstream automatically.

5. **Shift.** Pop the exit slat (index 0; it left as outflow in step 3). Every
   remaining slat's index decreases by one (advance toward the exit). Drop
   trailing empty slats produced by a shortened belt.

6. **Insert.** Place the admitted inflow into the belt at **entry depth**
   `d = round(latched_transit / DT)` measured from the exit
   ([§6](#6-variable-transit-time-sample-and-len)) using the placement method of
   [§8](#8-inflow-placement-spread-inputs) (default = all at depth `d`). Extend
   the belt with empty slats if `d` exceeds the current length. The inserted
   cohort's `entry_amount` = the admitted volume it received.

**Transit-time check.** A cohort inserted at depth `d` is popped as outflow after
exactly `d` steps (it occupies each of `d` slats for one DT), so its transit is
`d × DT = effective transit time`. Leakage is applied on every DT it is on the
belt, including the DT it exits.

## 5. Leakage

The conveyor-level `exponential_leak` flag selects the model for **all** its leak
flows. The per-flow number `f` from `<leak>`/`<eqn>` is interpreted differently
by model (this dual meaning is isee's actual behavior and is specified here
explicitly):

### 5.1 Linear leakage

`f ∈ [0, 1]` is the fraction of an entering cohort that leaks out by the time it
exits. It is removed as a **constant absolute amount per DT** while the cohort is
in the zone. With `M` in-zone slats ([§5.3](#53-leak-zones)):

```
leak_{k,i} = f_k × slats[i].entry_amount / M_k        (slat i in flow k's zone)
```

Because `entry_amount` is fixed for a cohort, the total leaked over its `M_k`
in-zone DTs is `f_k × entry_amount`. Constraint: the sum of linear leak fractions
across all leak flows over any slat must be `≤ 1`; if it reaches 1 the primary
outflow is 0. A flow's leak is clamped to the slat's remaining content (step-2
priority ordering).

### 5.2 Exponential leakage

`f` is a per-time-unit **rate**; each in-zone slat loses the same fraction of its
**current** content per DT:

```
leak_{k,i} = slats[i].content × f_k × DT              (slat i in flow k's zone)
```

Overlapping zones from multiple exponential flows compound (each applied in
step-2 order to the running content). Exponential leakage ignores `entry_amount`
and depends only on current content, so it is unaffected by transit-time changes.
This matches the isee steady-state factor `(1 − f×DT)` per slat.

### 5.3 Leak zones

`leak_start = a` and `leak_end = b` (`0 ≤ a ≤ b ≤ 1`) measure fractional belt
position **from the inflow (entry) side**: position 0 = entry, position 1 = exit.
Slat `i` (with `i = 0` the exit) has center position `p_i = (i + 0.5) / N`
measured from the exit, i.e. `1 − p_i` from the entry. Slat `i` is **in zone**
when `a ≤ (1 − p_i) ≤ b`. `M_k` = the count of in-zone slats for flow `k`
(recomputed when `N` changes). Defaults `a = 0, b = 1` put the whole belt in
zone. A shorter zone leaks the same total (linear) or the same per-DT fraction
(exponential) concentrated over fewer slats.

### 5.4 Integer leakage

With `<leak_integers/>`, flow `k` accumulates its computed real leak into
`leak_carry[k]` each DT; it actually removes `floor(leak_carry[k])` whole units
(distributed from the in-zone slats, exit-most first) and retains the fractional
remainder in `leak_carry[k]`. Reported rate = whole units removed / DT.

## 6. Variable transit time, `<sample>`, and `<len>`

### 6.1 Latching

`latched_transit` starts at the initial value of `<len>`. Each DT, `<sample>` is
evaluated; when it is nonzero, `latched_transit` is updated to the current value
of `<len>` (default `<sample> = 1`, so it re-latches every DT). Newly entering
material is placed at depth `round(latched_transit / DT)` (step 6). Material
already on the belt is **never** repositioned — it keeps advancing one slat/DT.

### 6.2 Belt growth and non-FIFO exit

If `latched_transit` increases, the entry depth `d` grows; the belt extends with
empty slats behind existing material (which continues shifting forward on
schedule). If `latched_transit` decreases, newly entering material is placed
shallower and can therefore exit **before** older, deeper material — the
documented non-FIFO behavior. The belt is not truncated; it shrinks naturally as
empty tail slats fall off during shifts. Linear leakage uses the cohort's own
`entry_amount` (fixed at its entry transit), so a later transit change does not
retroactively alter an existing cohort's leak schedule.

### 6.3 Capacity and inflow limit

Both default to INF. Per-DT (`vol = rate × DT`):

- **Capacity** bounds instantaneous contents: `cap_room = capacity −
  contents_after` (step 4), where `contents_after` already credits the room freed
  by this DT's leak and outflow (matching isee's `(Capacity − Conveyor)/DT +
  outflow` formula).
- **Inflow limit** bounds inflow per **time unit**:
  - *Continuous conveyor:* `limit_vol = in_limit × DT` (the per-time-unit limit
    prorated to this DT).
  - *Discrete conveyor:* the full per-time-unit budget may enter within a single
    DT. `in_carry` tracks volume admitted since the last integer time boundary
    and resets to 0 when the simulation clock crosses an integer time unit;
    `limit_vol = in_limit − in_carry`.
  - The inflow limit is ignored when the inflow's upstream source is itself a
    conveyor (the upstream conveyor already governs the rate).

Admitted inflow is `min(req_vol, cap_room, limit_vol)`, apportioned in inflow
order (step 4). Blocked material stays upstream.

## 7. Initialization

The stock `<eqn>` gives the initial value `V`. Two forms:

### 7.1 Scalar initial value, steady-state fill

`V` is the total initial contents, distributed so the belt is at the equilibrium
implied by its leak profile. General algorithm (works for any leak
configuration, linear or exponential, any zones, any number of flows):

1. Simulate a single unit cohort (`entry_amount = 1`) forward through the belt,
   applying exactly the [§5](#5-leakage) per-DT leak rules, to obtain the
   **retained profile** `c[i]` = the content a steady cohort holds upon arriving
   at slat `i` at the start of a step: `c[N-1] = 1` at the entry slat, and
   `c[i-1] = c[i] −` (the leak slat `i` sheds in one DT).
2. Let `S = Σ_i c[i]`. Set the cohort scale `E = V / S` (or `E = 0` if `S = 0`).
3. Initialize `slats[i].content = E × c[i]` and `slats[i].entry_amount = E` for
   all `i`.

Closed forms for the common cases (illustration; the algorithm above is
authoritative):

- **No leak:** `c[i] = 1` for all `i`, so each slat = `V / N` (even spread).
- **Linear, full zone, fraction `f`:** `c[i] = 1 − f·(N−1−i)/N`; equivalently
  each slat = `E·c[i]` with `E = V / (N − f·(N−1)/2)`.
- **Exponential, full zone, rate `f`:** `c[i] = (1 − f·DT)^(N−1−i)`, giving
  `E = V·f·DT / (1 − (1 − f·DT)^N)` — the isee exponential steady-state form.

### 7.2 Explicit per-slat list

A comma-separated `<eqn>` list initializes the belt directly. Each entry is the
quantity for **one time unit** (not one DT). With `k = 1/DT` slats per time unit,
list entry `v_u` for time-unit `u` fills that unit's `k` slats each with
`v_u × DT` for a **continuous** conveyor (so the outflow during unit `u` totals
`v_u`), or places the whole `v_u` in the first slat of the unit's block and
zeroes the rest for a **discrete** conveyor (isee "start of each time unit"
semantics). List length must equal `N` or the number of time units; too many
entries are truncated, too few repeat the last entry (XMILE
"InitializingDiscreteStocks" rules).

## 8. Inflow placement (spread inputs)

The default XMILE conveyor places all admitted inflow at the entry (depth `d`).
isee models may select another placement via `isee:spreadflow` on the inflow.
simlin implements all five; each distributes the admitted volume `A` (from step
4) across slats at insert time (step 6):

| `isee:spreadflow` | Placement of admitted volume `A` |
|---|---|
| `beginning` (default) | All of `A` at entry depth `d` (one cohort, `entry_amount = A`). |
| `even` | `A / (d)` into each of the `d` slats from exit+1 … entry; each becomes its own cohort with `entry_amount` = its share. |
| `dest` | Distribute `A` across the occupied slats **proportional to current content**; empty belt falls back to `beginning`. |
| `dist` | Distribute `A` across the `d` slats **proportional to a normalized distribution** from `<isee:distrib_eq>` (a graphical function or 1-D array), treated as a PDF over belt position and auto-normalized to sum 1. |
| `source` | Leakage-mirror: `A` is placed to mirror the position profile of the material pulled from a coupled upstream conveyor's leak. Requires an upstream conveyor; absent one, falls back to `beginning`. |

For linear leakage, a spread cohort's `entry_amount` is its own share, so each
sub-cohort leaks in proportion to what it carries. `dist`/`even` create multiple
cohorts in one DT; `content` and `entry_amount` are tracked per slat as usual.

## 9. Engine integration

Read `src/simlin-engine/CLAUDE.md` for the compilation pipeline.

### 9.1 Data model

`datamodel::Stock` gains `conveyor: Option<Conveyor>`; `datamodel::Flow` gains
`leakage: Option<Leakage>`; an inflow gains an optional placement:

```rust
pub struct Conveyor {
    pub transit_time: String,          // <len> (required)
    pub capacity: Option<String>,      // <capacity>
    pub inflow_limit: Option<String>,  // <in_limit>
    pub sample: Option<String>,        // <sample>
    pub arrest: Option<String>,        // <arrest>
    pub discrete: bool,
    pub batch_integrity: bool,
    pub one_at_a_time: bool,           // default true
    pub exponential_leak: bool,
}
pub struct Leakage {
    pub fraction: Option<String>,      // <leak> expr, None for a bare <leak/> marker
    pub integers: bool,                // <leak_integers/>
    pub zone_start: Option<String>,    // leak_start (default 0)
    pub zone_end: Option<String>,      // leak_end (default 1)
}
pub enum SpreadFlow { Beginning, Even, Dest, Dist(String), Source }  // on a Flow inflow
```

### 9.2 Protobuf, compatibility-critical

Protobuf is the one place backward compatibility is REQUIRED (a DB holds
serialized instances). `Variable.Stock` and `Variable.Flow` use field numbers up
through 12. Add new fields with **fresh, never-reused numbers** (e.g. `optional
Conveyor conveyor = 13;` on Stock, `optional Leakage leakage = 13;` and `optional
SpreadFlow spreadflow = 14;` on Flow) plus new `Conveyor`/`Leakage`/`SpreadFlow`
messages. Absent fields decode to `None`, so old serialized stocks remain valid
plain stocks. Regenerate with `pnpm build:gen-protobufs`. Never renumber or
repurpose an existing field.

### 9.3 Runtime (VM) design

A conveyor's outflow, leak, and admitted-inflow values are produced by the per-DT
algorithm ([§4.3](#43-per-dt-update)), not by per-variable equation evaluation.
Implement as a VM-native conveyor object (chosen over compile-time desugaring to
an `N`-slat aging chain because `N` can vary at runtime and capacity/limit
pushback on the inflow cannot be expressed as fixed equations):

- **State.** `Vm` gains a `conveyors: Box<[ConveyorState]>` side table
  ([§4.2](#42-conveyor-runtime-state)), parallel to `graphical_functions` /
  `prev_values` (`src/simlin-engine/src/vm.rs`). Each conveyor variable and its
  driven outflow/leak/inflow slots map to entries in this table via the layout.
- **Update hook.** Add a conveyor-update pass inside the Euler loop's
  `eval_step`, ordered **after** ordinary flow equations are evaluated (so
  requested inflow rates and `arrest`/`sample`/`len`/`capacity`/`in_limit`
  auxiliaries are current) and **before** stock integration (so the driven flow
  values feed the stock update). The pass writes each conveyor's outflow, leak,
  and admitted-inflow rates into their value slots, then advances the belt. The
  conveyor's own value slot receives `Σ slats.content`.
- **Initialization** ([§7](#7-initialization)) runs in the initials pass, filling
  `slats` from the stock `<eqn>` value before the first step.
- **Compiled representation.** The equation-less conveyor outflow/leak flows
  compile to a "driven by conveyor" marker (not an equation), so the
  `empty_equation` error no longer fires; instead the compiler wires the flow's
  value slot to the owning conveyor's update output.

### 9.4 Integration method

Conveyors require **Euler**. The slat model is defined per-DT; RK2/RK4 substeps
(`src/simlin-engine/src/vm.rs`, the `RungeKutta4` arm evaluates the derivative at
fractional-DT points) have no meaning for a belt that advances one slat per full
DT. If `sim_specs.method` is RK2 or RK4 and any conveyor is present, compilation
**fails with a clear error** naming a conveyor and stating that conveyors require
Euler integration. This follows the GH #486 precedent (LTM's non-Euler
rejection, `src/simlin-engine/src/db/assemble.rs`).

### 9.5 wasmgen

The WebAssembly backend mirrors the VM opcode-for-opcode with **no silent VM
fallback** (established rule, `src/simlin-engine/src/wasmgen`). The conveyor
side table and update pass are lowered to wasm the same way the GF/snapshot
regions are; until that lowering exists, a conveyor model returns
`WasmGenError::Unsupported` (loud), never a silent fallback.

### 9.6 LTM

A conveyor is a stock with non-INTEG dynamics; the flow-to-stock link-score
formula assumes plain INTEG under Euler (GH #486). LTM treats a conveyor's
primary outflow → downstream as an ordinary link, but the internal slat dynamics
are not scored as INTEG. LTM analysis over a model containing a conveyor
degrades **loudly** (a `Warning` naming the conveyor, emitted through the same
diagnostic path as the auto-flip warnings), never a silently-wrong score. Full
LTM-through-conveyor attribution is a separate enhancement.

### 9.7 Readers, writers, and other surfaces

- XMILE reader/writer (`src/simlin-engine/src/xmile/variables.rs`): add the
  `<conveyor>` block and `<element>`-level plumbing to `Stock`, the leak
  properties and `isee:spreadflow`/`<isee:distrib_eq>` to `Flow`; accept both
  leak encodings ([§3.3](#33-leakage-flows)); emit the `<conveyor>` block and
  `<uses_conveyor/>`. This alone fixes the round-trip corruption.
- MDL: Vensim has no conveyor primitive; the `delay conveyor` name recognized in
  `src/simlin-engine/src/mdl/builtins.rs` is a distinct, separate function — not
  a conveyor, not covered here.
- JSON (`src/json.rs`), TypeScript datamodel (`src/core`), and the diagram
  editor extend once the engine representation lands; the diagram renders a
  conveyor stock with its belt affordance and leak outflows.

## 10. Arrayed conveyors

An arrayed conveyor is `N_elem` **independent** conveyors, one per array element,
each with its own `ConveyorState`, transit time, leak flows, capacity, and inflow
limit. Non-apply-to-all arrays (`<element>` blocks) may give each element its own
`<len>` and other per-element attributes; shared properties (units, the
conveyor/leak markers) apply to all elements (XMILE §4.5.2). Element access uses
two subscript groups: `conv[array_subscript][slat]` reads slat `[slat]` (1-based
from the front) of element `[array_subscript]`. Each element's belt updates by
[§4.3](#43-per-dt-update) independently.

## 11. Queues and the conveyor side of queue–conveyor coupling

Queues (`<queue/>`) are a separate FIFO batch-tracking stock type with their own
`<uses_queue>` option and `<overflow/>` flow property (XMILE §3.7.3). Their
internal batch management is specified in a companion queue document. This
section fully specifies the **conveyor side** of a queue feeding a conveyor,
which is where the `discrete` / `batch_integrity` / `one_at_a_time` attributes
take effect:

- A conveyor with a queue directly upstream MUST be `discrete` (XMILE §3.7.2);
  the compiler errors if it is not.
- Each DT the conveyor requests up to `min(cap_room, limit_vol)`
  ([§6.3](#63-capacity-and-inflow-limit)) from the front of the queue:
  - `one_at_a_time = true` (default): take at most the single front batch this
    DT (even if more would fit).
  - `one_at_a_time = false`: take as many whole front batches as fit within the
    caps.
  - `batch_integrity = true`: never split a batch — if the front batch does not
    fully fit within the remaining cap, take nothing (it waits; a queue
    `<overflow/>` outflow may drain it — see the queue doc).
  - `batch_integrity = false`: split the front batch, taking exactly the volume
    that fits.
- Admitted batches enter the belt at depth `d` (step 6) like any inflow.

A conveyor fed by an ordinary stock or cloud ignores all three attributes and
uses the [§4.3](#43-per-dt-update) inflow admission directly; it needs no queue
support and is fully implementable without it.

## 12. Build sequence

All semantics above are fully specified regardless of build order; this is a
suggested implementation sequence, each step independently shippable:

1. **Represent & round-trip.** Datamodel + proto + XMILE reader/writer
   ([§9.1](#91-data-model)–[§9.2](#92-protobuf-compatibility-critical),
   [§9.7](#97-readers-writers-and-other-surfaces)). Import preserves the block;
   export re-emits it; the `empty_equation` error is replaced by the conveyor
   wiring ([§9.3](#93-runtime-vm-design)). Fixtures parse without data loss.
2. **Core continuous conveyor.** [§4](#4-runtime-model-and-the-per-dt-algorithm)
   update, [§7.1](#71-scalar-initial-value-steady-state-fill) scalar init,
   constant transit, single primary outflow, capacity + inflow limit, Euler-only
   enforcement ([§9.4](#94-integration-method)). Oracle: `minimal_conveyor.xmile`.
3. **Leakage & variable transit.** [§5](#5-leakage) linear + exponential + zones
   + integer leakage, [§6](#6-variable-transit-time-sample-and-len) variable
   `<len>`/`<sample>`, arrest, discrete conveyors,
   [§7.2](#72-explicit-per-slat-list) explicit-list init.
4. **Spread inputs & arrays.** [§8](#8-inflow-placement-spread-inputs) placement
   methods, [§10](#10-arrayed-conveyors) arrayed conveyors, `[]` slat access,
   array/cycle-time builtins over contents, wasmgen parity.
5. **Queues.** The companion queue spec plus
   [§11](#11-queues-and-the-conveyor-side-of-queueconveyor-coupling) coupling.

## 13. Test oracles

Vendored under `test/conveyors/` (see its README for full provenance, licenses,
and the CC BY 4.0 attribution):

- `minimal_conveyor.xmile` — hand-authored, transit time + capacity, single
  outflow, no leak. The clean core-conveyor oracle.
- `sir_social_distancing_mixnot.stmx` — peterhovmand corpus, CC BY 4.0. The belt
  is transit-time-only, but its `Not_Mixing` submodel feeds the conveyor via an
  inflow marked `isee:spreadflow="dist"` with `<isee:distrib_eq>profile`, so it
  exercises the distribution placement method ([§8](#8-inflow-placement-spread-inputs));
  it also uses the isee builtin `LOOKUPMEAN` for the transit time (a separate
  builtin gap).
- `covid19_severity.stmx` — peterhovmand corpus, CC BY 4.0. Leakage
  (`exponential_leak="true"` `<leak/>` flows) + arrayed conveyors
  ([§10](#10-arrayed-conveyors)).

None ship expected-output CSVs, and the real models also use non-conveyor
features simlin does not yet implement (`LOOKUPMEAN`, some unit-consistency
issues, `PREVIOUS`, other `isee:` builtins), so they are **reference fixtures,
not yet wired into `tests/integration/main.rs`**. They become executable oracles
once conveyor support lands and reference output is generated from Stella (or
another conforming engine); simlin's convention is `test/<name>/model.xmile` +
`output.csv`, wired as a `mod` in `tests/integration/main.rs`.

No open-source model was found exercising conveyor `<capacity>`, `<in_limit>`,
`<discrete>`, `<arrest>`, or an upstream queue; those need hand-authored
fixtures (and reference output) as their build step is reached.

## 14. Validation and logistics

The spec is complete and self-consistent — the per-DT algorithm, leakage
formulas, and initialization were validated by a reference prototype
([§15](#15-worked-examples-verified-reference-trajectories)) whose trajectories
are the concrete acceptance oracles for step 2/3. Two items are logistics, not
spec gaps:

- **Cross-engine confirmation.** The prototype pins simlin's own numerics; a
  Stella (or other conforming-engine) run of the vendored fixtures would confirm
  the derived formulas in §5/§7 match the vendor bit-for-bit. Not required to
  implement — the §15 trajectories are sufficient to build and test against —
  but a good confidence check when Stella access is available.
- **Diagram/editor authoring.** Rendering and authoring conveyor stocks and leak
  flows in the diagram editor is scoped with the TypeScript surface work in
  step 1/4 of [§12](#12-build-sequence); it does not affect the engine spec.

## 15. Worked examples (verified reference trajectories)

The per-DT algorithm (§4), leakage (§5), capacity/inflow-limit (§6.3), and
initialization (§7) were transcribed into a standalone reference prototype and
run on the scenarios below. The prototype (`test/conveyors/reference_prototype.py`)
is a faithful, executable statement of this spec; its trajectories are the
acceptance oracles a Rust implementation must reproduce. Every check below
**passes**, which is what "the spec is self-consistent" means concretely. Run it
with `python3 test/conveyors/reference_prototype.py`.

All scenarios use `DT = 0.25`, transit time `T = 4` (so `N = 16` slats), inflow
rate 250/time unit unless noted.

| # | Scenario | Steady contents | Steady primary outflow | Invariant checked |
|---|---|---|---|---|
| S1 | `minimal_conveyor` steady state (init `V = 250·4 = 1000`) | 1000 (constant) | 250 (constant) | contents and outflow constant at the equilibrium `inflow·T` |
| S2 | fill from empty (`V = 0`) | rises to 1000 by `t = 4` | 0 until `t = 4`, then 250 | first nonzero outflow at exactly `t = T = 4.0` (transit delay) |
| S3 | linear leak `f = 0.2`, full zone | 906.25 | 200, leak 50 | steady `outflow / inflow = 1 − f = 0.8`; total leaked over a cohort = `f · entry` |
| S4 | exponential leak `f = 0.1`/time, full zone | 832.699579 | 166.730042, leak 83.269958 | steady outflow = `250·(1 − f·DT)^N = 166.730042` (matches closed form) |
| S5 | `capacity = 600`, req inflow 250 | plateaus at 600 | throttled | contents never exceed capacity; blocked inflow (rate 0 once full) stays upstream |
| S6 | `in_limit = 150`/time (continuous), req inflow 250 | plateaus at 600 (`150·4`) | 150 | admitted inflow never exceeds 150; equilibrium contents = `in_limit·T` |
| S7 | non-integer transit `T = 4.1` | — | — | `N = round(4.1/0.25) = round(16.4) = 16`; effective transit `16·0.25 = 4.0`; compile Warning |

Reading S1–S2 together confirms the transit-time semantics: a step of inflow
into an empty conveyor produces zero outflow for exactly `T` time units, then the
outflow equals the inflow — a pure `T`-unit delay, which is the defining
behavior of a conveyor. S3/S4 confirm the two leakage models produce the
documented conservation (`1 − f` of a cohort survives for linear; the geometric
`(1 − f·DT)^N` survival for exponential). S5/S6 confirm capacity and inflow limit
throttle the admitted inflow and push unadmitted material back upstream, settling
at the `inflow·T` / `in_limit·T` equilibria.
