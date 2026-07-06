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
authoritative source is the per-stock `<conveyor>` block.

The reader accepts **three** encodings of the header option: the
`<uses_conveyor/>` element, the `<uses_conveyors/>` spelling, and the attribute
form Stella actually writes on the root element — `uses_conveyor=""` on
`<smile>`/`<xmile>` (both vendored `.stmx` fixtures use the attribute form).
simlin emits the element form `<uses_conveyor/>` whenever any stock in the
project is a conveyor, setting `arrest`/`leak` to reflect whether any conveyor
uses those features.

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

The **primary** (belt-end) outflow is the first `<outflow>` that is *not*
marked as a leakage; every leak-marked outflow is a leakage regardless of list
position. This implements both halves of XMILE §4.2.1: the first-is-primary
*convention* (when nothing is marked, the first outflow is primary and the rest
are leakages) and its explicit-override provision ("this behavior MAY be
explicitly overridden by tagging each leakage flow with the `<leak/>`
property") — so a model whose first listed outflow carries `<leak/>` gets a
later primary. Compile **errors**: a conveyor with no outflows, or whose
outflows are all leak-marked, cannot simulate (XMILE: at least one outflow MUST
be the normal outflow).

XMILE says a conveyor outflow MUST NOT carry a normal equation — the conveyor
drives it — but real Stella exports put a **placeholder** `<eqn>0</eqn>` on
primary conveyor outflows anyway (both vendored `.stmx` fixtures do, e.g.
`recovering` in `sir_social_distancing_mixnot.stmx`). The reader therefore
**preserves but ignores** any `<eqn>` on a primary conveyor outflow: it is kept
for round-trip fidelity and plays no role in simulation — never an error. The
writer emits the spec-strict form (no `<eqn>` on a primary outflow), consistent
with the `<non_negative/>` writer rule in [§3.4](#34-non-negativity). On a
*leak-marked* flow the `<eqn>` is meaningful — it carries the leak fraction.
Real Stella models put the leak **fraction** in the `<eqn>` of a
`<leak/>`-tagged flow:

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
(the spec's example). If a flow carries **both** a value-bearing `<leak>` and an
`<eqn>`, the `<leak>` content wins (it is the more specific, spec-blessed
carrier); the `<eqn>` is preserved for round-trip but ignored for simulation. A
`<leak/>` with no fraction and no `<eqn>` is a valid "leakage, fraction TBD"
marker (used mid-edit); it parses and represents but contributes zero leakage
until a fraction is supplied.

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
it there without error and ignores it on the primary inflow/outflow. The writer
MUST NOT emit `<non_negative/>` on a primary conveyor outflow (XMILE §4.3: "this
property MUST NOT appear for them"); it MAY emit it on leak flows, matching
Stella.

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
    leak_carry: Vec<f64>,   // per leak-flow accumulator for <leak_integers/> (never resets)
    in_carry:  f64,         // per-time-unit in_limit budget spent (discrete only; resets at
                            // integer time boundaries -- §6.3)
    quant_carry: Vec<f64>,  // discrete admission fractional-unit accumulators, one PER
                            // equation-driven inflow (§6.4 rule 1 -- per-inflow so every
                            // inserted whole unit is attributable to the upstream flow
                            // whose clearance accrued it; never resets -- distinct state
                            // from in_carry)
}
Slat {
    content: f64,           // material in this slat
    leak_alloc:  Vec<f64>,  // per leak-flow: fixed per-DT linear-leak amount (set at insertion)
    leak_budget: Vec<f64>,  // per leak-flow: remaining linear-leak total (set at insertion)
}
```

`leak_alloc`/`leak_budget` carry each cohort's **linear** leak schedule, fixed
when the cohort enters ([§5.1](#51-linear-leakage)); exponential leakage needs no
per-cohort state (it reads current content). When cohorts merge into one slat
(a shortened transit time — [§6.2](#62-belt-growth-merging-and-non-fifo-exit)),
`content`, `leak_alloc`, and `leak_budget` are all **summed**, which is exact
because linear leak is linear in the entering amount. For a constant transit
time the deque has a fixed length `N`; a variable transit time grows/shrinks it.

The conveyor variable's **reported scalar value** is the sum of all slat
contents (total material on the belt). This is simlin's defined semantics and is
consistent with the steady-state initialization ([§7](#7-initialization)).

### 4.3 Per-DT update

Conveyors integrate under Euler only ([§9.4](#94-integration-method)). Within
each Euler step, after ordinary flow equations are evaluated and before stocks
are updated, the conveyor pass runs in **two phases over all conveyors**. Phase A
reads only each conveyor's own start-of-step state; Phase B consumes Phase A's
outputs. Because no phase reads another conveyor's same-phase results, the
iteration order **within** each phase is irrelevant — conveyor chains and even
conveyor cycles (A feeds B feeds A) are deterministic with no topological sort.

The key structural rule that makes this possible (and matches isee: "It is not
possible to connect a conveyor outflow to something that would constrain the
flow value — the conveyor itself determines the value of the outflow"):
**a conveyor-driven flow (a primary outflow or a leak flow) is never blocked by
its destination**, with the single exception of an arrested destination (below).
A downstream conveyor admits conveyor-driven inflow unconditionally — its
`capacity` may be transiently exceeded (sanctioned by the XMILE capacity prose,
which allows exceedance until drainage) and its `in_limit` does not apply
(isee: the inflow limit "is not available if the inflow comes from another
conveyor"). A modeler who wants blocking between conveyors inserts a queue,
which is exactly isee's guidance.

**Phase A — leak and exit (each conveyor, from its own start-of-step state):**

0. **Arrest.** Evaluate `<arrest>` **first — before the latch**, because an
   arrested conveyor's time is suspended and it must not re-latch (otherwise a
   step where arrest, sample, and a `<len>` change coincide would freeze the
   material yet still change the post-release entry depth). If nonzero, this
   conveyor is *arrested* this step: every inflow and outflow of this conveyor
   reports 0, `slats` is left untouched (material frozen, not lost), and all
   remaining steps — including the latch — are skipped for it. Arrest flags
   come from ordinary expressions already evaluated this step, so all flags
   are known before any conveyor mutates. Clock-based state is unaffected by
   arrest: the `in_carry` integer-time-boundary reset
   ([§6.3](#63-capacity-and-inflow-limit)) still fires, and
   `leak_carry`/`quant_carry` persist untouched.

1. **Latch.** Evaluate `<sample>`; when it is nonzero, update `latched_transit`
   from `<len>` (with the runtime hygiene of
   [§4.4](#44-runtime-expression-hygiene)). This happens before any belt
   mutation, so this step's insert (step 6) uses the newly latched depth.

2. **Leak.** For each leak flow `k` in outflow-list order, for each slat `i` in
   `k`'s zone ([§5.3](#53-leak-zones)): compute `leak_{k,i}` per
   [§5.1](#51-linear-leakage)/[§5.2](#52-exponential-leakage), clamp it to
   `slats[i].content` (earlier flows have priority), subtract it from the slat
   (and from `leak_budget[k]` for linear). Flow `k`'s reported **rate** =
   `(Σ_i leak_{k,i}) / DT`, quantized per [§5.4](#54-integer-leakage) when
   `<leak_integers/>` is set. Exception: if leak flow `k`'s destination is an
   arrested conveyor, skip it entirely this step (rate 0, content stays).

3. **Exit.** If the primary outflow's destination is an arrested conveyor, the
   exit is *held*: outflow rate 0, and the exit slat's contents stay in place
   (they merge with the slat arriving behind them in Phase B, accumulating at
   the exit until the destination un-arrests). Otherwise: exit **volume**
   `out_vol = slats[0].content` (post-leak); primary outflow **rate** =
   `out_vol / DT`.

**Phase B — admit, shift, insert (each conveyor):**

4. **Admit inflow.** Partition this conveyor's inflows into *conveyor-driven*
   (the value is an upstream conveyor's Phase-A outflow or leak) and
   *equation-driven* (everything else; already evaluated). Then:
   - `conv_vol` = Σ conveyor-driven inflow volumes — admitted unconditionally.
   - `contents_after = contents₀ − (Σ leak this step) − out_vol`.
   - `cap_room = capacity == INF ? INF : max(0, capacity − contents_after − conv_vol)`.
   - `limit_vol` = the inflow-budget cap per [§6.3](#63-capacity-and-inflow-limit)
     (applies to equation-driven inflows only).
   - `eq_admitted = min(Σ equation-driven request volumes, cap_room, limit_vol)`,
     apportioned across the equation-driven inflows **in listed order** (fill the
     first inflow's request fully, then the second, …). Each inflow's **reported
     rate** = its apportioned share / DT. Un-apportioned material is never
     removed from the upstream stock (the flow's value *is* the admitted rate),
     so blocked material accumulates upstream automatically.
   - `admitted = conv_vol + eq_admitted`.

5. **Shift.** If the exit was held (step 3), leave slat 0 in place and merge the
   next slat into it (summing `content`/`leak_alloc`/`leak_budget`; with a
   single-slat belt there is nothing to merge and slat 0 simply stays);
   otherwise pop slat 0 (it left as outflow). Every remaining slat advances one
   position toward the exit. Drop trailing empty slats.

6. **Insert.** This step **always executes, even when `admitted = 0`** — so
   after step 6 the belt always has at least `d` slats (this matters: belt
   length feeds zone membership, [§5.3](#53-leak-zones)). Place `admitted` into
   the belt at **entry depth** `d = round(latched_transit / DT)` from the exit
   ([§6](#6-variable-transit-time-sample-and-len)) using the placement method of
   [§8](#8-inflow-placement-spread-inputs) (default: all at depth `d`). Extend
   the belt with empty slats if `d` exceeds its current length. Compute the
   cohort's linear-leak `leak_alloc`/`leak_budget` per
   [§5.1](#51-linear-leakage) and **add** all three fields into the target
   slat(s) (summing with any cohort already there).

**Transit-time check.** A cohort inserted at depth `d` is popped as outflow after
exactly `d` steps (it occupies each of `d` slats for one DT), so its transit is
`d × DT = effective transit time`. Leakage applies on every DT it is on the belt,
including the DT it exits.

**Conservation.** Every step, for every conveyor:
`admitted − out_vol − Σ leak = Δ(Σ slats.content)` exactly (up to f64
arithmetic). The reference prototype asserts this on every scenario step.

**Visibility to other equations.** The values the pass produces follow the VM's
normal flow/stock timing ([§9.3](#93-runtime-vm-design)):

- The conveyor's driven flows (primary outflow, leaks) and the *admitted*
  values of its inflows are **this step's flow values**: any ordinary equation
  that reads one of them (e.g. `x = graduating * 2`) is dependency-ordered
  **after** the conveyor pass and sees the same value the CSV records — a
  reader never sees a pre-clamp requested rate.
- The conveyor's own stock value follows stock semantics: equations evaluated
  during step `t` read the **start-of-step** contents; the post-update
  `Σ slats.content` becomes the value at `t + DT`. Reading the stock therefore
  never creates an ordering dependency on the pass.
- The pass's *inputs* (`arrest`/`sample`/`len`/`capacity`/`in_limit`, leak
  fractions, requested inflow rates) must be computable **before** the pass. A
  same-step cycle from a pass output back into a pass input (e.g.
  `capacity = f(graduating)`) is a compile-time `CircularDependency` error —
  unless the path runs through the conveyor's stock value, which (as a
  start-of-step read) breaks the cycle exactly like any stock.

### 4.4 Runtime expression hygiene

`<len>`, `<capacity>`, `<in_limit>`, leak fractions, and inflow equations are
arbitrary expressions, so invalid values are reachable at runtime even when the
compile-time checks pass. The rules (all simlin-defined, chosen for
determinism):

- **Transit time.** At each latch (step 1): a finite value is clamped to
  `max(DT, value)`; a non-finite (NaN/±INF) value leaves `latched_transit`
  unchanged. The *initial* latch (during initialization) with a non-finite or
  `≤ 0` value is a runtime initialization error.
- **Leak fractions.** Linear fractions clamp to `[0, 1]`; exponential rates
  clamp to `[0, ∞)`; NaN is treated as 0 (no leak). A linear fraction is
  sampled **once, at the cohort's entry DT** (it is baked into
  `leak_alloc`/`leak_budget`, [§5.1](#51-linear-leakage)); an exponential rate
  is re-read every DT ([§5.2](#52-exponential-leakage)).
- **Inflow requests.** An equation-driven inflow request clamps to
  `max(0, rate)` (conveyor inflows are uniflows — [§3.4](#34-non-negativity));
  a NaN request admits 0.
- **Capacity / inflow limit.** A negative value is treated as 0 (fully
  blocking); NaN or +INF is treated as INF (unconstrained — INF is already
  their documented "no constraint" value).
- **Arrest / sample.** "Nonzero" means `value != 0` with NaN treated as 0 (not
  arrested, not sampled).

## 5. Leakage

The conveyor-level `exponential_leak` flag selects the model for **all** its leak
flows. The per-flow number `f` from `<leak>`/`<eqn>` is interpreted differently
by model (this dual meaning is isee's actual behavior and is specified here
explicitly):

### 5.1 Linear leakage

`f ∈ [0, 1]` is the fraction of an entering cohort that leaks out by the time it
exits. It is removed as a **constant absolute amount per DT** while the cohort is
in the zone, and that amount is **fixed at the moment the cohort enters** —
matching isee's "the amount that is leaked is based on the transit time that was
used when the material was put on the conveyor" (`f` itself is likewise sampled
once, at the cohort's entry DT — [§4.4](#44-runtime-expression-hygiene)).

Terminology (this resolves an ambiguity in the word "entry"): the **entry
depth** `d = round(latched_transit / DT)` is where default-placement material is
inserted (step 6); a cohort's **insertion depth** `d_c` is where it actually
lands (`d_c = d` for default placement; `d_c < d` for most spread-input shares;
only a `dest` share can land beyond `d`, on a stale-tail slat —
[§8](#8-inflow-placement-spread-inputs)). After a transit shrink the *physical*
belt can be longer than `d` (a stale tail of older material) — a new cohort's
journey is its own `d_c` slats, **not** the physical belt length.

When a cohort of volume `A` is inserted, for each linear leak flow `k`
(one formula covers default and spread placement):

```
M_k(p)   = count of in-zone slats among indices 0..p-1     // §5.3, per the belt after step-6 extension
alloc_k  = f_k × A / M_k(d)                 // per-DT leak amount, fixed for the cohort's life
budget_k = alloc_k × min(M_k(d_c), M_k(d))  // lifetime total this cohort may leak to flow k
```

(The `min` with `M_k(d)` matters only for a `dest` share landing on a
stale-tail slat beyond `d`: it caps that share's lifetime leak at the
documented `f_k × A` instead of letting the longer path over-schedule it. For
every other placement `d_c ≤ d` and the `min` is a no-op.)

For default placement (`d_c = d`) the budget is exactly `f_k × A` — the full
documented fraction, leaked evenly over the cohort's own `d`-slat journey. This
holds **regardless of the physical belt length**: after a transit shrink a new
cohort still leaks `f_k × A` in total (the denominator is its own path, never
the stale-tail length). For a spread-input share inserted mid-belt (`d_c < d`)
the per-DT amount matches an entry cohort of the same size and the budget is
prorated to the zone slats it will actually traverse — matching isee's "linear
leak fractions will be applied only for the duration the material is in the
conveyor." If `M_k(d) = 0` (the zone is narrower than one slat at this DT
resolution), the cohort leaks nothing to flow `k`.

Each DT, an in-zone slat leaks `min(leak_alloc[k], leak_budget[k], content)` to
flow `k` (step 2), decrementing the budget by the amount actually removed. Under
a constant transit time the budget never binds and the totals are exact
(`f_k × A` per entry cohort — scenario S3); the budget exists solely to bound a
cohort's lifetime leak when zone membership shifts under it (a partial zone
combined with a belt-length change, the one corner where in-zone DT counts can
drift). In that corner a cohort under-leaks deterministically, never over-leaks;
the residual is simlin-defined behavior (vendor docs are silent at this level of
detail).

Constraint: `Σ_k f_k ≤ 1` across a conveyor's linear leak flows; at exactly 1
the primary outflow is 0. The check is enforced at runtime by the content clamp
(step-2 priority ordering), and the compiler warns when constant leak fractions
sum above 1.

### 5.2 Exponential leakage

`f` is a per-time-unit **rate**; each in-zone slat loses the same fraction of its
**current** content per DT:

```
leak_{k,i} = slats[i].content × f_k × DT              (slat i in flow k's zone)
```

Overlapping zones from multiple exponential flows compound (each applied in
step-2 order to the running content). Exponential leakage carries no per-cohort
state — it depends only on current content — so it is unaffected by transit-time
changes and by cohort merging. This matches the isee steady-state factor
`(1 − f×DT)` per slat.

### 5.3 Leak zones

`leak_start = a` and `leak_end = b` (`0 ≤ a ≤ b ≤ 1`) measure fractional belt
position **from the inflow (entry) side**: position 0 = entry, position 1 = exit
(XMILE's "fraction of conveyor length" — the zone is a region of the *belt*, not
of a cohort's journey). With `L` the current belt length in slats, slat `i`
(where `i = 0` is the exit) has center position `p_i = (i + 0.5) / L` measured
from the exit, i.e. `1 − p_i` from the entry. Slat `i` is **in zone** when
`a ≤ (1 − p_i) ≤ b`. In-zone membership is evaluated against the belt as it
exists at that moment: at step 2 for leaking, and at step 6 (for the `M_k(d)` /
`M_k(d_c)` counts of [§5.1](#51-linear-leakage)) for a cohort's fixed schedule.
Defaults `a = 0, b = 1` put the whole belt in zone. A shorter zone leaks the
same total (linear) or the same per-DT fraction (exponential) concentrated over
fewer slats.

### 5.4 Integer leakage

With `<leak_integers/>`, flow `k` accumulates its computed real leak into
`leak_carry[k]` each DT; it actually removes `floor(leak_carry[k])` whole units
(distributed from the in-zone slats, exit-most first) and retains the fractional
remainder in `leak_carry[k]`. Reported rate = whole units removed / DT. Linear
`leak_budget` decrements by each slat's *computed* (pre-quantization) leak, so
the carry redistributes timing without changing a cohort's lifetime total.

## 6. Variable transit time, `<sample>`, and `<len>`

### 6.1 Latching

`latched_transit` starts at the initial value of `<len>`. Each DT, `<sample>` is
evaluated; when it is nonzero, `latched_transit` is updated to the current value
of `<len>` (default `<sample> = 1`, so it re-latches every DT). Newly entering
material is placed at depth `round(latched_transit / DT)` (step 6). Material
already on the belt is **never** repositioned — it keeps advancing one slat/DT.

### 6.2 Belt growth, merging, and non-FIFO exit

If `latched_transit` increases, the entry depth `d` grows; the belt extends with
empty slats behind existing material (which continues shifting forward on
schedule). If `latched_transit` decreases, newly entering material is placed
shallower and can therefore exit **before** older, deeper material — the
documented non-FIFO behavior. The belt is not truncated; it shrinks naturally as
empty tail slats fall off during shifts.

When a new cohort's insertion depth lands on a slat that already holds material
(a shortened transit), the cohorts **merge**: `content`, `leak_alloc`, and
`leak_budget` are summed field-wise. Summation is exact for linear leakage
(both the per-DT amount and the lifetime budget are linear in the entering
volume) and irrelevant for exponential leakage (stateless). Each cohort's leak
schedule was fixed at its own entry ([§5.1](#51-linear-leakage)), so a later
transit change never retroactively alters it.

### 6.3 Capacity and inflow limit

Both default to INF. Per-DT (`vol = rate × DT`):

Both apply to **equation-driven** inflows only; conveyor-driven inflows bypass
both (the never-blocked rule of [§4.3](#43-per-dt-update): capacity may be
transiently exceeded, and isee documents that the inflow limit "is not
available if the inflow comes from another conveyor").

- **Capacity** bounds instantaneous contents: `cap_room = capacity −
  contents_after − conv_vol` (step 4), where `contents_after` credits the room
  freed by this DT's outflow **and leak**, and `conv_vol` is the
  unconditionally-admitted conveyor-driven inflow. This *extends* isee's
  documented `(Capacity − Conveyor)/DT + outflow` formula, which credits only
  the outflow: simlin also credits leak (the room genuinely exists by the end
  of the step). Models combining capacity limits with leakage may therefore
  admit slightly more per DT than Stella — a known, deliberate delta to check
  in the cross-engine confirmation ([§14](#14-validation-and-logistics)).
- **Inflow limit** bounds equation-driven inflow per **time unit**:
  - *Continuous conveyor:* `limit_vol = in_limit × DT` (the per-time-unit limit
    prorated to this DT).
  - *Discrete conveyor:* the full per-time-unit budget may enter within a single
    DT. `in_carry` tracks volume admitted since the last integer time boundary
    and resets to 0 when the simulation clock crosses an integer time unit;
    `limit_vol = in_limit − in_carry`.

Equation-driven admitted inflow is `min(req_vol, cap_room, limit_vol)`,
apportioned in inflow order (step 4). Blocked material stays upstream.

### 6.4 Discrete conveyors

`discrete="true"` makes the conveyor move whole units ("batches") instead of a
continuous stream. Its complete semantics are three rules, all already
integrated into the algorithm above:

1. **Quantized admission, tracked per inflow.** Step 4's *equation-driven*
   clearance accumulates rather than inserting directly, and the accumulator
   is **per inflow** (`quant_carry[j]` for the `j`-th equation-driven inflow —
   [§4.2](#42-conveyor-runtime-state); never resets, distinct from the
   boundary-resetting `in_carry`) so that every whole unit that eventually
   inserts is attributable to the specific upstream flow whose clearance
   accrued it — even on a later step where that inflow's request has dropped
   to 0, and even with several inflows. Per step:
   - Step 4 apportions the clearance in listed order as usual; each inflow's
     share accrues to its own carry: `quant_carry[j] += cleared_j`.
   - Insertion walks the inflows **in listed order** with a shared capacity
     budget `B = floor(cap_room)` (`B = ∞` when capacity is INF):
     `units_j = min(floor(quant_carry[j]), B)`; `B -= units_j`;
     `quant_carry[j] -= units_j`. Inflow `j`'s **reported rate** =
     `units_j / DT` — the reported value debits exactly the upstream that
     owns the material, so no flow misattributes or invents material.
   - The capacity re-check on whole units is load-bearing: carry accumulated
     in earlier steps was never capacity-cleared *as a unit*, so without it a
     unit could materialize into room step 4 cleared only fractionally (e.g.
     carry `0.9`, `cap_room = 0.2`: the 0.2 clearance raises the carry to 1.1,
     but no unit fits until `cap_room ≥ 1`). The `in_limit` window
     (`in_carry`) accounts at clearance time and is *not* re-checked at
     insertion.
   Un-inserted carry is never taken from upstream — an inflow's reported rate
   reflects only its inserted units, so conservation and the capacity bound
   both hold exactly. Slat contents therefore stay integral, and exits arrive
   as integral lumps. Conveyor-driven inflow bypasses quantization entirely
   (never blocked — [§4.3](#43-per-dt-update); it is already integral when the
   upstream is discrete, which the queue-fed case mandates).
2. **Per-time-unit inflow-limit window** ([§6.3](#63-capacity-and-inflow-limit)):
   the full `in_limit` budget may be consumed within a single DT, resetting at
   integer time boundaries — rather than being prorated `× DT`.
3. **Start-of-time-unit initialization** ([§7](#7-initialization)): the belt's
   `N` slats partition into **time-unit blocks** by simulated travel time —
   slat `i` belongs to block `u = floor(i × DT)` (well-defined for any DT,
   integer `1/DT` or not), giving `U = floor((N − 1) × DT) + 1` blocks (the
   number of blocks that actually own a slat; equal to `N × DT` when `1/DT` is
   integral, and never producing an empty block when it is not). A scalar
   initial value is divided across the `U` blocks, the whole per-block share
   placed in that block's deepest slat rather than spread evenly; an explicit
   init list places each entry the same way
   ([§7.2](#72-explicit-per-slat-list)).

A conveyor with a queue directly upstream MUST be discrete (XMILE §3.7.2;
enforced as a compile error — [§11](#11-queues-and-the-conveyor-side-of-queueconveyor-coupling)).

## 7. Initialization

The stock `<eqn>` gives the initial value `V`. Two forms:

### 7.1 Scalar initial value, steady-state fill

`V` is the total initial contents, distributed so the belt is at the equilibrium
implied by its leak profile. (For a *discrete* conveyor the distribution is
placed at time-unit starts instead — [§6.4](#64-discrete-conveyors) rule 3; the
cohort scale below still applies.) General algorithm (works for any leak
configuration, linear or exponential, any zones, any number of flows):

1. Simulate a single unit cohort (volume 1, entered at the entry slat) forward
   through the belt, applying exactly the [§5](#5-leakage) per-DT leak rules, to
   obtain the **retained profile** `c[i]` = the content a steady cohort holds
   upon arriving at slat `i` at the start of a step:
   `c[N-1] = 1` at the entry slat, and
   `c[i-1] = max(0, c[i] −` (the leak slat `i` sheds in one DT)`)` — the
   `max(0, ·)` clamp matters exactly when leak fractions sum above 1, which
   compiles with only a warning ([§5.1](#51-linear-leakage)).
2. Let `S = Σ_i c[i]`. Set the cohort scale `E = V / S` (or `E = 0` if `S = 0`).
3. Initialize each slat `i` as a cohort of entering volume `E` that has already
   traveled to position `i`: `content = E × c[i]`,
   `leak_alloc[k] = f_k × E / M_k`, and `leak_budget[k] = leak_alloc[k] ×`
   (the number of flow-`k` in-zone slats from `i` to the exit, inclusive) —
   i.e. the unspent remainder of its schedule ([§5.1](#51-linear-leakage)).

Closed forms for the common cases (illustration; the algorithm above is
authoritative):

- **No leak:** `c[i] = 1` for all `i`, so each slat = `V / N` (even spread).
- **Linear, full zone, fraction `f`:** `c[i] = 1 − f·(N−1−i)/N`; equivalently
  each slat = `E·c[i]` with `E = V / (N − f·(N−1)/2)`.
- **Exponential, full zone, rate `f`:** `c[i] = (1 − f·DT)^(N−1−i)`, giving
  `E = V·f·DT / (1 − (1 − f·DT)^N)` — the isee exponential steady-state form.

### 7.2 Explicit per-slat list

A comma-separated `<eqn>` list initializes the belt directly. Two
interpretations exist in isee's documentation (its "Initializing Discrete
Stocks" help page — an isee source, not the OASIS spec: "a number of values
equal to the transit time, or the transit time divided by DT"), disambiguated
by list length:

- **Length `N` (one entry per slat):** entry `j` (1-based, front first) fills
  slat `j − 1` directly. This is the only interpretation available for
  non-integer transit times, per the isee rule.
- **Any other length (one entry per time unit):** using the time-unit blocks of
  [§6.4](#64-discrete-conveyors) rule 3 (slat `i` in block `floor(i × DT)`,
  `U = floor((N − 1) × DT) + 1` blocks), entry `v_u` fills block `u`: split evenly across
  the block's slats for a **continuous** conveyor (so the outflow during unit
  `u` totals `v_u`), or placed whole in the block's deepest slat for a
  **discrete** conveyor (isee "start of each time unit" semantics). The list is
  normalized to `U` entries first: extra entries are truncated, a short list
  repeats its last entry.
- When `N` equals `U` (DT = 1) the two interpretations coincide.

Each filled slat gets the linear-leak schedule of a cohort that entered at the
belt entry and traveled to its position, as in [§7.1](#71-scalar-initial-value-steady-state-fill)
step 3.

## 8. Inflow placement (spread inputs)

The default XMILE conveyor places all admitted inflow at the entry (depth `d`).
isee models may select another placement via `isee:spreadflow` on the inflow.
simlin implements all five; each distributes the admitted volume `A` (from step
4) across slats at insert time (step 6):

Definitions used below: `d` = the entry depth (step 6); target slats are indexed
`i ∈ 0..d−1` (0 = exit) for every method except `dest`, which targets the whole
physical belt `i ∈ 0..L−1` (see its row); a slat's **fractional position from
the entry side** over the entry path is `x_i = 1 − (i + 0.5)/d` (so `x ≈ 0` at
the entry slat `i = d−1`, `x ≈ 1` at the exit slat `i = 0`, consistent with the
[§5.3](#53-leak-zones) orientation). Every placement distributes the admitted
volume `A` as per-slat shares `A_i ≥ 0` with `Σ A_i = A` **exactly** — no
placement may lose admitted material:

| `isee:spreadflow` | Per-slat share `A_i` |
|---|---|
| `beginning` (default) | `A_i = A` for `i = d−1` (the entry slat), 0 elsewhere: one cohort at depth `d`. |
| `even` | `A_i = A / d` for every `i ∈ 0..d−1` — including the exit slat, whose share exits after one DT (isee: "equal amount every position"). |
| `dest` | `A_i = A × content_i / Σ_j content_j` for `i ∈ 0..L−1` — every slat of the current physical belt receives its content-proportional share (an empty slat's share is 0 by construction, and `Σ A_i = A` exactly since numerators and denominator range over the same slats). A share landing on a stale-tail slat beyond `d` has `d_c = i + 1 > d`; its leak budget is capped by the [§5.1](#51-linear-leakage) `min(M_k(d_c), M_k(d))` rule. If total content is 0, fall back to `beginning`. |
| `dist` | `A_i = A × w_i / Σ_j w_j` over `i ∈ 0..d−1`, where `w_i = max(0, g(x_i))` and `g` is `<isee:distrib_eq>`: a graphical function evaluated at `x_i` with its own x-axis scale and extrapolation rules (isee normalizes the table "so that it is effectively a probability density function" — the `Σ w` division here is that normalization), or a 1-D array of length `m` where `w_i` = the element `floor(x_i × m)` (clamped to `m−1`). If `Σ w = 0`, fall back to `beginning`. |
| `source` | Applies only when this inflow is **conveyor-driven from an upstream leak flow** (the "coupling" is flow identity: the inflow *is* that leak). For each upstream slat `j` that leaked `q_j` this step (Phase A), with `y_j` = that slat's fractional position from the entry side of the *upstream* belt, place `q_j` at the target slat `i ∈ 0..d−1` whose `x_i` is nearest `y_j` (ties toward the exit) — positions mirror proportionally when the two belts differ in length. If the inflow is not an upstream leak, fall back to `beginning`. |

Each per-slat share `A_i` is a cohort in its own right: its linear-leak
`leak_alloc`/`leak_budget` are computed from its own volume `A_i` and its own
insertion depth `d_c = i + 1` per [§5.1](#51-linear-leakage) (so a mid-belt
share's budget is prorated to the zone slats it will actually traverse), then
summed into the target slat like any merge
([§6.2](#62-belt-growth-merging-and-non-fifo-exit)).

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
- **Update hook.** Add the two-phase conveyor pass
  ([§4.3](#43-per-dt-update)) inside the Euler loop's `eval_step`, ordered
  **after** the equations the pass depends on (its inputs: requested inflow
  rates and the `arrest`/`sample`/`len`/`capacity`/`in_limit`/leak-fraction
  expressions) and **before** both stock integration and any equation that
  *reads* a pass output — the §4.3 "Visibility to other equations" rules are
  the normative ordering; equations reading a driven flow are dependency-
  ordered after the pass, not lumped with "ordinary flows" generally. Phase A
  writes each conveyor's outflow and leak rates; Phase B writes admitted-inflow
  rates and advances the belts. Within each phase the iteration order over
  conveyors is arbitrary (no topological sort — [§4.3](#43-per-dt-update)).
  The conveyor's own value slot receives `Σ slats.content`.
- **Initialization** ([§7](#7-initialization)) runs in the initials pass, filling
  `slats` from the stock `<eqn>` value before the first step.
- **Compiled representation.** Conveyor-driven flows (the primary outflow and
  leak flows) compile to a "driven by conveyor" marker, not an equation — the
  primary outflow's `<eqn>` is absent or an ignored Stella placeholder
  ([§3.3](#33-leakage-flows)), and a leak flow's fraction expression feeds the
  cohort schedule rather than the flow slot. The `empty_equation` error no
  longer fires; instead the compiler wires the flow's value slot to the owning
  conveyor's update output.

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

### 9.8 Errors, units, and lifecycle

**New diagnostics** (added to `ErrorCode` in `src/simlin-engine/src/common.rs`):

| Diagnostic | Severity | Trigger |
|---|---|---|
| `ConveyorWithoutOutflow` | Error | conveyor with no outflows, or all outflows leak-marked ([§3.3](#33-leakage-flows)) |
| `ConveyorNonEulerMethod` | Error | any conveyor present under RK2/RK4 ([§9.4](#94-integration-method)) |
| `ConveyorQueueUpstreamNotDiscrete` | Error | queue directly upstream of a non-discrete conveyor ([§11](#11-queues-and-the-conveyor-side-of-queueconveyor-coupling)) |
| `ConveyorTransitNotPositive` | Error | constant `<len>` ≤ 0 at compile time; non-finite/`≤ 0` initial latch at runtime ([§4.4](#44-runtime-expression-hygiene)) |
| `ConveyorTransitNotDtMultiple` | Warning | `T/DT` not integral; message reports the effective transit `N × DT` ([§4.1](#41-slat-count-and-non-integer-transit-times)) |
| `ConveyorLeakFractionsExceedOne` | Warning | constant linear leak fractions summing above 1 ([§5.1](#51-linear-leakage)) |
| `ConveyorLtmDegraded` | Warning | LTM analysis requested on a model containing conveyors ([§9.6](#96-ltm)) |

**Unit checking** (`src/simlin-engine/src/units_check.rs`): with the conveyor
stock's units `S` and the model's time unit `t` — `<len>`: `t`; `<capacity>`:
`S`; `<in_limit>`: `S/t`; a linear leak fraction: dimensionless; an exponential
leak rate: `1/t`; `<sample>` and `<arrest>`: dimensionless (both are
*conditions* evaluated for nonzero — a predicate like `TIME > 10` type-checks
dimensionless, so requiring `t` would reject valid sample expressions). Driven
flows (primary outflow, leaks, admitted inflows) carry `S/t` like any flow.

**VM lifecycle**: `Vm::reset` re-initializes the conveyor side table exactly as
it re-runs initials (the belt is derived state — same pattern as the
`prev_values`/`initial_values` buffers it sits beside). Each module *instance*
containing a conveyor owns its own `ConveyorState` (the side table is per
instance, like module state generally). `PREVIOUS(conv, …)` reads the previous
step's `Σ slats.content` (the ordinary `prev_values` snapshot of the stock
slot); `INIT(conv)` reads the initial value `V` (the ordinary `initial_values`
snapshot).

**Double-clamp composition**: a flow can be simultaneously an equation-driven
conveyor inflow and the outflow of a non-negative upstream stock. Clamps
compose upstream-first: the non-negative-stock clamp bounds what the upstream
can supply, then conveyor admission (step 4) clips further; the flow's single
reported value is the final admitted rate. (Today simlin does not enforce
non-negative stocks at runtime — the composition rule is normative for when it
does.)

## 10. Arrayed conveyors and container access

An arrayed conveyor is `N_elem` **independent** conveyors, one per array element,
each with its own `ConveyorState`, transit time, leak flows, capacity, and inflow
limit. Non-apply-to-all arrays (`<element>` blocks) may give each element its own
`<len>` and other per-element attributes; shared properties (units, the
conveyor/leak markers) apply to all elements (XMILE §4.5.2). Each element's belt
updates by [§4.3](#43-per-dt-update) independently.

**Container access** (XMILE §3.7.1: conveyors are containers, so `[]` and the
array builtins MUST work over their contents when arrays are supported):

- For a **scalar** conveyor, `conv[j]` (`j` 1-based from the front/exit) reads
  `slats[j−1].content` of the current belt. `j` outside `[1, L]` (where `L` is
  the current physical belt length, which can exceed `N` after a transit
  shrink) yields NaN — the same rule as an out-of-range dynamic array
  subscript in simlin.
- For an **arrayed** conveyor, two subscript groups apply:
  `conv[array_subscript][slat]` reads the slat of that element's belt, same
  rules.
- The array builtins (`SUM`, `MIN`, `MAX`, `MEAN`, `STDDEV`) over a conveyor
  operate on the current slat-content vector (length `L`); `SIZE(conv)` returns
  `L`. All values are read from start-of-step state (like the stock value —
  [§4.3](#43-per-dt-update) visibility rules).
- The isee **cycle-time builtin family** (`CTMEAN`, `CTSTDDEV`, `CTMAX`,
  `CTMIN`, `CTFLOW`, `CYCLETIME`, `THROUGHPUT`) requires per-material age
  tracking the slat model does not carry. These are isee extensions, not
  XMILE-mandated: they are **explicitly out of scope** and fail loudly as
  unknown builtins, exactly as today.

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
   methods, [§10](#10-arrayed-conveyors-and-container-access) arrayed conveyors, `[]` slat access,
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
  ([§10](#10-arrayed-conveyors-and-container-access)).

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

The spec is complete, and its core — the per-DT algorithm (including arrest and
held exits), both leakage models, capacity/inflow limits, variable transit with
merging, and steady-state initialization — is machine-verified by the reference
prototype ([§15](#15-worked-examples-verified-reference-trajectories)), whose
trajectories are the concrete acceptance oracles for step 2/3. §15 states
exactly which rules the prototype does and does not execute; the unexecuted
rules (integer/zoned leakage, discrete quantization, spread placements,
explicit-list init) are specified in prose and get fixtures with the Rust
implementation. Two items are logistics, not spec gaps:

- **Cross-engine confirmation.** The prototype pins simlin's own numerics; a
  Stella (or other conforming-engine) run of the vendored fixtures would confirm
  the derived formulas in §5/§7 match the vendor bit-for-bit. Not required to
  implement — the §15 trajectories are sufficient to build and test against —
  but a good confidence check when Stella access is available.
- **Diagram/editor authoring.** Rendering and authoring conveyor stocks and leak
  flows in the diagram editor is scoped with the TypeScript surface work in
  step 1/4 of [§12](#12-build-sequence); it does not affect the engine spec.

## 15. Worked examples (verified reference trajectories)

The core of the spec was transcribed into a standalone reference prototype
(`test/conveyors/reference_prototype.py`) and run on the scenarios below; its
trajectories are the acceptance oracles a Rust implementation must reproduce.
Every check below **passes**. Run it with
`python3 test/conveyors/reference_prototype.py` (exits nonzero on any failure).

**Prototype coverage — what these scenarios do and do not verify.** Executed:
the two-phase update including arrest and the held-exit merge (§4.3),
linear/exponential leakage with fixed per-cohort schedules (§5.1–§5.3),
capacity and inflow limits (§6.3), discrete quantized admission against a tight
capacity with per-inflow attribution (§6.4 rule 1), transit latching/shrink/merging (§6.1–§6.2),
steady-state initialization (§7.1), conveyor chains, and half-away rounding
(§4.1). **Not** executed (specified in prose only; they get fixtures with the
Rust implementation): leak zones narrower than the belt, `<leak_integers/>`,
spread-input placements, explicit-list initialization, and leak-fed chains
(`source` placement). Treat only the executed set as prototype-verified.

**Reporting convention.** Each trajectory row shows the stock's
**start-of-step** contents at time `t` together with the flow rates during
`[t, t + DT)` — the same convention as simlin's CSV output, so rows diff
directly against simulation results with no off-by-one.

All scenarios use `DT = 0.25`, transit time `T = 4` (so `N = 16` slats), inflow
rate 250/time unit unless noted.

| # | Scenario | Steady contents | Steady primary outflow | Invariant checked |
|---|---|---|---|---|
| S1 | `minimal_conveyor` steady state (init `V = 250·4 = 1000`) | 1000 (constant) | 250 (constant) | contents and outflow constant at the equilibrium `inflow·T` |
| S2 | fill from empty (`V = 0`) | 0 at `t = 0`, rises linearly, first reaches 1000 at `t = 4.0` | 0 until `t = 4`, then 250 | first nonzero outflow at exactly `t = T = 4.0` (transit delay); start-of-step convention pinned |
| S3 | linear leak `f = 0.2`, full zone | 906.25 | 200, leak 50 | steady `outflow / inflow = 1 − f = 0.8`; total leaked over a cohort = `f · entry` |
| S4 | exponential leak `f = 0.1`/time, full zone | 832.699579 | 166.730042, leak 83.269958 | steady outflow = `250·(1 − f·DT)^N = 166.730042` (matches closed form) |
| S5 | `capacity = 600`, req inflow 250 | plateaus at 600 | throttled | contents never exceed capacity; blocked inflow (rate 0 once full) stays upstream |
| S6 | `in_limit = 150`/time (continuous), req inflow 250 | plateaus at 600 (`150·4`) | 150 | admitted inflow never exceeds 150; equilibrium contents = `in_limit·T` |
| S7 | non-integer transit `T = 4.1` and half-case `T = 4.125` | — | — | `N(16.4) = 16` and `N(16.5) = 17` — half rounds **away from zero**, never banker's rounding; effective transit `N·DT`; compile Warning |
| S8 | chain: conveyor A (`T = 2`, draining 500) feeds conveyor B (`T = 4`, `capacity = 100`) | — | — | conveyor-driven inflow is never blocked ([§4.3](#43-per-dt-update)): B's capacity is transiently exceeded, and total material across A + B + B's cumulative outflow is conserved exactly |
| S9 | transit shrink `4 → 2` at `t = 2` with linear leak `f = 0.2` | — | — | cohorts merging into one slat sum their `content`/`leak_alloc`/`leak_budget` ([§6.2](#62-belt-growth-merging-and-non-fifo-exit)); whole-run conservation holds; lifetime leak never exceeds the `f`-budget; **post-shrink steady outflow returns to `(1 − f)·inflow = 200`** — new entry cohorts leak the full fraction over their own path even while the stale tail keeps the physical belt longer than the entry depth ([§5.1](#51-linear-leakage)) |
| S10 | chain A (`T = 2`, draining 500) feeds B; B arrested for `t ∈ [1, 2)` | — | — | held exit ([§4.3](#43-per-dt-update) steps 3/5): during the hold A's outflow reports 0 and material accumulates in A's exit slat while B is frozen; on release the accumulated 250 exits A as one lump (rate 1000); chain conservation exact throughout |
| S11 | discrete conveyor (`T = 2`, `capacity = 3`), fractional requests (0.6/DT) | — | — | quantized admission ([§6.4](#64-discrete-conveyors) rule 1): slat contents stay integral, a whole unit inserts only when `floor(cap_room)` fits it — so contents never exceed capacity even as the carry crosses 1.0 against fractional room — and material still flows (no deadlock) |
| S12 | discrete conveyor, **two** equation-driven inflows (1.6 and 0.8/time; the second shuts off at `t = 8`) | — | — | per-inflow attribution ([§6.4](#64-discrete-conveyors) rule 1): the bookkeeping identity `cum_cleared_j = cum_reported_j + quant_carry[j]` holds for each inflow at every step, so every inserted unit debits exactly the upstream flow that cleared it; after shutoff an inflow reports at most its residual carry; both totals integral |

In addition to the per-scenario invariants, the harness asserts the
[§4.3](#43-per-dt-update) **conservation identity**
(`admitted − out − leak = Δcontents`) for every conveyor on every step of every
scenario.

Reading S1–S2 together confirms the transit-time semantics: a step of inflow
into an empty conveyor produces zero outflow for exactly `T` time units, then the
outflow equals the inflow — a pure `T`-unit delay, which is the defining
behavior of a conveyor. S3/S4 confirm the two leakage models produce the
documented conservation (`1 − f` of a cohort survives for linear; the geometric
`(1 − f·DT)^N` survival for exponential). S5/S6 confirm capacity and inflow limit
throttle the admitted inflow and push unadmitted material back upstream, settling
at the `inflow·T` / `in_limit·T` equilibria. S8–S10 confirm the structural
rules added in review: conveyor-driven flows are never blocked (so conveyor
chains and cycles need no topological ordering), cohort merging under a
shortened transit is exact summation with the full leak fraction preserved,
and an arrested destination holds material at the upstream exit without loss.
