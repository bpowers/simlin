# Queue support: specification

This is the "companion queue document" referenced by
[docs/design/conveyors.md §11](/docs/design/conveyors.md). It specifies the
XMILE **queue** stock type and, together with conveyors.md §11, the full
queue-conveyor coupling. It follows the same sourcing and precedence rules as
the conveyor spec.

**Sources.** OASIS XMILE v1.0 (`docs/reference/xmile-v1.0.html`, §3.7.3, §4.2,
§4.2.1, §4.3) for syntax and prose semantics, and isee systems' behavior for
the per-DT math where the OASIS prose is silent.

**Precedence rule: Stella wins.** Where the OASIS XMILE prose and documented or
observed Stella/isee behavior conflict, simlin follows **Stella**. Where
Stella's behavior is unknown, this document makes an explicit, conservative
choice and flags it as a Stella-probe point (the same convention conveyors.md
uses); such a choice is a decision to verify, not a spec to defend.

## 1. Motivation

Queues are a first-class stock type in Stella / isee systems models: a
first-in-first-out waiting line that tracks individual batches. They matter
whenever material must **wait** because something downstream is constrained --
canonically, a queue feeding a `discrete` conveyor whose capacity or inflow
limit throttles admission. XMILE 1.0 marks queues OPTIONAL (§3.7.3, "Optional
Queue Conformance").

### Current behavior (verified against HEAD, 2026-07-07)

`<queue/>` is not represented: the XMILE reader does not recognize the stock
option, so a queue stock imports as a plain INTEG stock and the `<overflow/>`
flow property and `<uses_queue>` header are dropped. Import-then-export
**corrupts** a Stella queue model exactly as it did conveyors before their
support landed. A conveyor with a queue directly upstream cannot be expressed at
all, so conveyors.md §11 (the coupling) is unreachable.

## 2. Concepts and vocabulary

- **Batch.** The material that entered the queue during one DT: a single scalar
  **volume**. Batches are the queue's unit of tracking (XMILE §3.7.3: "track
  individual batches that enter them, otherwise they'd just be stocks"). A batch
  carries only its volume -- unlike a conveyor slat, a queue batch has no age or
  transit schedule, because nothing in the queue or coupling semantics depends
  on how long a batch has waited. The queue value is `Σ batch volumes`.
- **FIFO.** The first batch to enter is the first to leave. Outflows pull from
  the **front** (oldest); the inflow appends at the **back**.
- **Driven outflow.** A queue outflow has no user equation: its value is
  determined by the queue's inflow, its contents, and what is downstream (XMILE
  §3.7.3). The engine computes it, exactly like a conveyor's driven outflow
  (conveyors.md §9.3). Any `<eqn>` on a queue outflow is preserved but ignored,
  as with conveyor driven flows.
- **Unconstrained vs constrained downstream.** A queue outflow to a **cloud or
  regular stock** is unconstrained: it always empties the queue. A queue outflow
  to a **conveyor** is constrained: the conveyor limits it by its capacity and
  inflow-limit (and arrest). Constraint is what makes batches wait.
- **Overflow.** A secondary outflow marked `<overflow/>` that becomes active
  only when a higher-priority outflow is **blocked** by a capacity restriction,
  an inflow-limit restriction, or an arrested downstream conveyor -- the only
  three conditions that redirect queue contents to an overflow (XMILE §3.7.3).

## 3. XMILE syntax

### 3.1 Options header

Real Stella exports put `<uses_queue>` in the model's `<isee:options>` /
`<options>` header when any queue is present. It has one OPTIONAL attribute:

```xml
<uses_queue overflow="true"/>
```

`overflow="true"` announces that some queue has an overflow outflow. simlin's
reader accepts the header (with or without `overflow`) and ignores it for
semantics; the writer re-emits `<uses_queue/>` whenever the project has a queue,
adding `overflow="true"` when any queue outflow is an overflow. Round-trip
fidelity only; the presence of a `<queue/>` block is the authoritative signal.

### 3.2 The queue block (on a stock)

```xml
<stock name="waiting">
  <eqn>0</eqn>
  <inflow>arrivals</inflow>
  <outflow>into_service</outflow>
  <queue/>
</stock>
```

`<queue/>` is a bare marker with **no options** (XMILE §4.2: "Queues do not have
any options"). It causes the stock to behave as a queue. As with conveyors,
Stella writes a placeholder `<eqn>` (typically `0`, or the initial batch total)
that the engine preserves-but-ignores for the outflow wiring; the reader
tolerates any `<eqn>` and the writer re-emits the preserved placeholder.

An initial value `V > 0` seeds the queue with a single batch of volume `V` at
time 0 (§7). Initialization from an explicit batch list is not part of XMILE and
is out of scope.

### 3.3 Outflow priority and overflow

A queue MAY have multiple outflows. Their order in the stock's `<outflow>` tags
is their **priority order**: the first is the primary (highest priority). Every
outflow after the first MAY carry the `<overflow/>` flow property:

```xml
<flow name="into_service"><eqn>0</eqn></flow>          <!-- primary, driven -->
<flow name="balk"><eqn>0</eqn><overflow/></flow>       <!-- overflow, driven -->
```

`<overflow/>` may appear ONLY on a queue outflow, and never on the first one
(XMILE §4.3). Both the primary and overflow outflows are driven (no user
equation). A non-overflow second outflow (a queue with two ordinary competing
outflows) is legal in XMILE but rare; §5.4 specifies it.

### 3.4 Non-negativity

Queue inflows MUST be non-negative and queue outflows are non-negative by
definition, so `<non_negative/>` MUST NOT appear on them (XMILE §4.3). The
engine treats a queue inflow as clamped at zero (a negative inflow contributes
no batch) and never produces a negative driven outflow.

### 3.5 Container access

`queue[k]` reads the volume of the `k`-th batch from the FRONT (1-based;
`k` outside `[1, batch_count]` yields NaN). `SUM/MEAN/MIN/MAX/STDDEV(queue)`
reduce over the current batch-volume vector and `SIZE(queue)` is the batch count
(XMILE §3.7.3 / §2.2.1: these MUST work on queues as containers). Arrayed:
`A[i,j][k]` accesses the `k`-th front batch of queue element `A[i,j]`. This
reuses the conveyor container-access machinery (conveyors.md §10) verbatim --
see §8.

## 4. Runtime model and the per-DT algorithm

### 4.1 Queue runtime state

Each queue owns a `QueueState` side table, parallel to a conveyor's belt and to
`prev_values`/`initial_values`: it is derived state, re-initialized on reset and
per module instance. The state is a FIFO of batch volumes:

```
batches: VecDeque<f64>   // front = index 0 = oldest; back = newest
```

Invariant: `Σ batches == the queue stock's value`. The queue stock slot is
integrated by the ordinary Stocks phase from the driven inflow/outflow rates
(the conservation identity `Δqueue = inflow − Σ outflows`, exactly as a conveyor
stock integrates from its driven flows -- conveyors.md §9.3); the side table
tracks the same total at batch granularity so container access and the coupling
can see individual batches.

### 4.2 Per-DT update

The queue pass runs in the Euler loop between the Flows phase (which computes
the inflow rate and every downstream conveyor's capacity/inflow-limit/arrest
inputs) and the Stocks phase (which integrates the stock). For each queue, in a
single pass:

1. **Admit inflow.** Read the inflow flow's value `f_in` (already computed this
   step). Append one batch of volume `in_vol = max(f_in, 0) · DT` at the back.
   (Multiple inflows sum; each is clamped at zero independently, then one batch
   of the summed volume is appended. A queue with no inflow simply appends
   nothing.)
2. **Serve outflows in priority order.** For each outflow `o_1, o_2, …` in
   declaration order, compute how much it removes from the front this DT
   (§4.3–§4.4) and pop that volume off the front (splitting the boundary batch
   if a partial volume is taken). Accumulate the removed volume as the outflow's
   driven rate `f_out = removed_vol / DT`.
3. **Publish container values** (§8): the batch-volume vector, count, and
   reductions are published at step-start from the batch list as it was BEFORE
   this step's admit/serve, so an equation reading `SUM(queue)` sees start-of-step
   state -- identical timing and mechanism to conveyor container access.

The Stocks phase then integrates `queue += (Σ f_in − Σ f_out) · DT`, which
matches the side table because step 1 added exactly `Σ f_in · DT` and step 2
removed exactly `Σ f_out · DT`.

**Ordering note (append-then-serve).** The inflow batch is appended BEFORE
outflows are served, so a batch can enter and leave in the same DT when the
downstream is unconstrained. This is the literal reading of "their outflows will
always empty the queue" (§4.3 below) and keeps a queue with an unconstrained
outflow a faithful pass-through. It is a Stella-probe point: if Stella imposes a
one-DT minimum residence (serve from start-of-DT contents only, new batch waits),
step 1 and step 2 swap order and the new batch is excluded from this DT's
service. The two differ only when the same DT both admits and fully drains.

### 4.3 Outflow to an unconstrained downstream (cloud or regular stock)

A queue outflow whose target is a cloud or a regular (non-conveyor) stock is
unconstrained and **empties the queue**: it removes every batch currently at the
front, i.e. the entire queue (XMILE §3.7.3). `removed_vol = Σ batches` at the
point this outflow is served (after higher-priority outflows already took their
share), `f_out = removed_vol / DT`, and the batch list is left empty.

If a queue has two ordinary (non-overflow) outflows both to unconstrained
targets, the first empties the queue and the second removes nothing (there is
nothing left). This degenerate case is legal; §5.4 covers competing outflows.

### 4.4 Outflow to a conveyor (constrained)

A queue outflow whose target is a `discrete` conveyor is limited by that
conveyor's admission capacity this DT. The conveyor requests up to
`req = min(cap_room, limit_vol)` from the front (conveyors.md §6.3: `cap_room`
is the remaining belt capacity, `limit_vol` the discrete per-time-unit inflow
budget). The queue supplies from the front under the `one_at_a_time` /
`batch_integrity` rules (conveyors.md §11, restated in §9 here), pops the
supplied batches, and the supplied volume is both the queue outflow rate and the
conveyor's admitted inflow for this DT. The conveyor MUST be `discrete` when a
queue is directly upstream (§9); a non-discrete one is a compile error
(`ConveyorQueueUpstreamNotDiscrete`).

The pass order therefore couples the queue and its downstream conveyor: the
queue outflow value is not known until the conveyor's `req` is known, and the
conveyor's admission is not known until the queue supplies batches. §9 specifies
the combined pass.

### 4.5 Overflow

An `<overflow/>` outflow (§3.3) is served AFTER its higher-priority sibling(s)
and only removes material that a higher-priority outflow left behind **because
that sibling was blocked** by one of the three redirecting conditions (XMILE
§3.7.3): the downstream conveyor was at capacity, at its inflow limit, or
arrested. The volume the overflow may take is exactly the front material the
blocked sibling could not accept for one of those reasons. An overflow to a
cloud or regular stock then drains that material entirely (like any unconstrained
outflow); an overflow to another conveyor is itself constrained by that
conveyor.

If the higher-priority outflow was NOT blocked (it served everything it wanted,
or was zero because the queue was empty, or was blocked for some other reason
that does not apply in simlin -- e.g. an isee oven cooking), the overflow removes
nothing (XMILE §3.7.3: "these are the only conditions that redirect queue
contents to an overflow"). Because the only blocking conditions simlin produces
ARE capacity / inflow-limit / arrest from a downstream conveyor, in practice an
overflow drains precisely the volume the upstream conveyor rejected this DT.

## 5. Multiple outflows and priority

### 5.1 Priority order

Outflows are served in `<outflow>` declaration order. Each pops from the same
front, so a higher-priority outflow that empties the queue leaves nothing for
lower-priority ones. The primary (first) outflow is never an overflow.

### 5.2 Blocked-volume accounting

Serving records, per outflow, the volume it WANTED versus the volume it TOOK.
The shortfall attributable to a capacity / inflow-limit / arrest block on the
primary is the "redirectable" volume an overflow may claim (§4.5). A shortfall
from an empty queue is not redirectable.

### 5.3 Overflow chains

XMILE allows multiple overflows; a blocked overflow lets the next-lower overflow
become active (§3.3). Serving proceeds in priority order, each overflow claiming
the still-redirectable front volume its higher-priority siblings left.

### 5.4 Two ordinary competing outflows

Two non-overflow outflows on one queue are legal but unusual. They are served in
priority order against the same front; the first takes what its downstream
accepts (draining fully if unconstrained), the second takes from what remains.
This falls out of §4.2 step 2 with no special case.

## 6. Arrayed queues

An arrayed queue is `N` independent FIFO queues, one `QueueState` per element,
exactly as an arrayed conveyor is `N` independent belts (conveyors.md §10). The
element offsets follow the compiler's canonical `name[elem…]` row-major keys.
Container access `A[i,j][k]` selects element `A[i,j]`'s `k`-th front batch.

## 7. Initialization

A scalar initial value `V`:
- `V <= 0`: the queue starts empty (no batches).
- `V > 0`: the queue starts with a single batch of volume `V` at the front.

There is no notion of steady-state fill (a queue has no transit time) and no
per-batch initial list in XMILE.

## 8. Container access

Reading a queue's batches from equations (`queue[k]`, `SUM/MEAN/MIN/MAX/STDDEV`,
`SIZE`) reuses the conveyor container-access mechanism (conveyors.md §10)
unchanged: the queue pass computes each container result natively from the
`QueueState` and publishes it into a synthesized hidden stock's slot at
step-start (top of the Euler loop and in `run_initials`), so the value survives
the Flows phase and reflects start-of-step state. `queue[k]` for a
compile-time-constant `k` reads the `k`-th front batch (`k` outside
`[1, count]` -> NaN); reducers operate on the batch-volume vector; `SIZE` is the
batch count. Residual forms (a reducer over an expression of the batches, a
dynamic index, ranges/wildcards over batches) keep the same loud
`ContainerAccessUnsupported` rejection conveyors use.

## 9. Queue-conveyor coupling (conveyors.md §11, queue side)

This section completes conveyors.md §11 with the queue side now that batches
exist.

- **Discrete requirement.** A conveyor with a queue directly upstream MUST be
  `discrete`; a non-discrete one is `ConveyorQueueUpstreamNotDiscrete` (Error).
  "Directly upstream" means the conveyor's single equation-driven inflow flow is
  a queue outflow.
- **Combined pass.** For a queue feeding a discrete conveyor, one combined
  native pass runs (in place of the separate queue and conveyor passes for that
  pair): the conveyor computes `req = min(cap_room, limit_vol)` from its own
  belt + caps (conveyors.md §6.3), the queue supplies from the front under the
  batch rules below, the supplied batches are popped from the queue and admitted
  to the belt at depth `d` (conveyors.md §8 placement, step 6), and the supplied
  volume is written as BOTH the queue's driven outflow rate and the conveyor's
  admitted inflow.
- **`one_at_a_time = true` (default).** Take at most the single front batch this
  DT, even if more would fit within `req`.
- **`one_at_a_time = false`.** Take as many whole front batches as fit within
  `req`.
- **`batch_integrity = true`.** Never split a batch: if the front batch does not
  fully fit within `req`, take nothing (it waits; a queue `<overflow/>` may
  drain it -- §4.5).
- **`batch_integrity = false`.** Split the front batch, taking exactly the
  volume that fits within `req`.
- **Blocked volume -> overflow.** Whatever `req` (capacity / inflow-limit) or an
  arrested conveyor prevents the primary outflow from taking is the redirectable
  volume for a queue overflow (§4.5).

## 10. Engine integration

### 10.1 Data model

`datamodel::Compat` gains, alongside `conveyor`/`leakage`/`spreadflow`:
- `queue: Option<Queue>` on a stock (present iff `<queue/>`).
- `overflow: bool` on a flow (present iff `<overflow/>`).

`Queue` is a bare marker struct (no options) mirroring `Conveyor`'s placement
and derive set (`Clone, PartialEq, Eq, salsa::Update`, plus an UNCONDITIONAL
`Debug` -- a `debug-derive`-gated Debug breaks the no-default-features WASM
build). Keeping it a struct (rather than a bool) leaves room for any future
vendor attribute without churning construction sites, matching `Conveyor`.

### 10.2 Protobuf, compatibility-critical

Add `Queue` and the flow `overflow` bool to `project_io.proto` following the
existing Compat field conventions; new fields append (never renumber) since a DB
holds serialized instances. Round-trips through protobuf, JSON, and XMILE.

### 10.3 Runtime (VM) design

Mirror `conveyor_compile`: a datamodel pre-pass expands each queue into an
ordinary INTEG stock plus placeholder-`0` driven outflows and (for a queue-fed
conveyor) wires the combined pass, clearing the `<queue/>` marker so the
ordinary compile path integrates the stock normally. A `QueueNotExpanded` guard
in `compile_project_incremental` rejects a surviving `<queue/>` marker loudly,
identical to `ConveyorNotExpanded`, so no ordinary or wasmgen path silently
mis-simulates. The queue pass and (where coupled) the conveyor pass run in the
Euler arm between Flows and Stocks. Queues are Euler-only for the same reason
conveyors are (`QueueNonEulerMethod`, mirroring `ConveyorNonEulerMethod`).

### 10.4 wasmgen

A queue model IS lowered by the wasm backend (`wasmgen/passes.rs`). The wasmgen
datamodel entry point routes through the same `queue_compile::compile_sim`
dispatch the VM takes, so it compiles the identical expanded project and resolves
the identical plans; the pass is then emitted as unrolled, plan-specialized wasm
(no runtime interpretation of a plan structure). Both hook points are preserved:
containers publish at step start, and admit-then-serve runs between Flows and
Stocks. The `QueueNotExpanded` guard still fires for any path that reaches
`compile_project_incremental` with a live `<queue/>` marker, so a future caller
that bypasses the dispatch cannot silently mis-simulate.

Conveyors are still `Unsupported` from the wasm backend (loud, no silent VM
fallback; conveyors.md §9.5), and a conveyor-bearing model is rejected up front
before the dispatch. Conveyor-to-queue coupling therefore does not arise on the
wasm path yet; a `QueueOutflowKind::Coupled` outflow is rejected explicitly
rather than mis-lowered.

### 10.5 LTM

A queue, like a conveyor, has non-INTEG dynamics; LTM over a model containing a
queue degrades loudly with a `Warning` naming the queue (`QueueLtmDegraded`,
mirroring `ConveyorLtmDegraded` and emitted from the same
`model_all_diagnostics` site to avoid the cross-module double-drain of #866).

### 10.6 Readers, writers, and other surfaces

- XMILE reader/writer (`src/simlin-engine/src/xmile/`): recognize `<queue/>` on
  a stock, `<overflow/>` on a flow, and the `<uses_queue>` header; emit them.
  This alone fixes the round-trip corruption.
- MDL: Vensim has no queue primitive; not covered.
- JSON, TypeScript datamodel (`src/core`), and the diagram editor extend once the
  engine representation lands.

### 10.7 Errors, units, and lifecycle

New / reused diagnostics (`ErrorCode`):

| Diagnostic | Severity | Trigger |
|---|---|---|
| `QueueNotExpanded` | Error | a `<queue/>` marker reaches the ordinary compile path un-expanded |
| `QueueNonEulerMethod` | Error | any queue present under RK2/RK4 |
| `ConveyorQueueUpstreamNotDiscrete` | Error | queue directly upstream of a non-discrete conveyor (already defined; now reachable) |
| `QueueOverflowNotOnQueue` | Error | `<overflow/>` on a non-queue-outflow, or on a queue's first outflow |
| `QueueSecondaryOutflowToConveyor` | Error | a queue outflow other than the first feeds a conveyor (constrained secondary/overflow service is deferred; only the primary may couple) |
| `StockBothConveyorAndQueue` | Error | one stock carries BOTH a `<conveyor>` block and a `<queue/>` marker -- a stock has exactly one type (also applies to conveyors.md §9.8). The reader preserves both markers faithfully; the conflict is rejected before expansion in the unified build path, since each expansion clears only its own marker and the two passes would otherwise silently double-drive the shared outflow slot |
| `QueueLtmDegraded` | Warning | LTM requested on a model containing a queue |

**Unit checking.** A queue outflow carries `S/t` like any flow; the queue stock
and its inflow are checked by the ordinary path. There are no queue block
parameter expressions to unit-check (a queue has no options), so §9.8-style
special checking is unnecessary.

**Lifecycle.** `Vm::reset` re-initializes the queue side table exactly as it
re-runs initials (derived state). Each module instance owns its own
`QueueState`. `PREVIOUS(queue)` reads the previous step's `Σ batches`;
`INIT(queue)` reads the initial value `V`.

## 11. Build sequence

Each step is independently shippable:

1. **Represent & round-trip.** Datamodel + proto + XMILE reader/writer for
   `<queue/>`, `<overflow/>`, `<uses_queue>` (§10.1-10.2, §10.6). Import
   preserves the block; export re-emits it. Fixtures parse without data loss.
2. **Core queue runtime.** `queue.rs` FIFO batch state, per-DT admit + serve for
   the unconstrained (cloud/stock) case, scalar init (§4-§4.3, §7). Oracle:
   a hand-authored `queue_drain.xmile`.
3. **VM/compile integration.** `queue_compile` expansion, driven outflows,
   `QueueNotExpanded`/`QueueNonEulerMethod` guards, Euler-only, lifecycle (§10.3,
   §10.7).
4. **Container access & arrayed queues.** Reuse the conveyor mechanism (§6, §8).
5. **Multiple outflows, priority, overflow** (§4.5, §5).
6. **Queue-conveyor coupling.** The combined pass + batch rules (§9); flip
   conveyors.md §11 from loud-reject to supported.

## 12. Test oracles

Hand-authored fixtures under `test/queues/` (none of the vendored conveyor
corpus exercises an upstream queue). At minimum:
- `queue_drain.xmile` -- queue -> cloud, unconstrained drain (§4.3).
- `queue_wait.xmile` -- queue -> discrete conveyor at capacity, batches wait and
  the belt fills to capacity (§9), plus a `one_at_a_time` vs all-at-once twin.
- `queue_overflow.xmile` -- queue -> capacity-limited conveyor with an
  `<overflow/>` to a cloud draining the rejected volume (§4.5).
- `queue_container.xmile` -- `SIZE`/`SUM`/`queue[k]` over batches (§8).

Reference output is generated from Stella (the ground truth per the precedence
rule) or, for the pure batch arithmetic, hand-computed and pinned by a reference
prototype in the style of `test/conveyors/reference_prototype.py`.
