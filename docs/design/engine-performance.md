# Engine performance: profile and optimization opportunities

Status: analysis + four rounds of wins landed. Round 1 2026-05-19; round 2
(constant folding + linear-run fast paths) 2026-06-03; round 3 2026-08-10 —
the salsa pipeline's own redundancy on the compile side, a superinstruction
family on the run side, and the LTM link-score arms that were being
materialized only to evaluate to zero.

This documents an empirical CPU/memory profile of **compiling and simulating the
C-LEARN hero model** (the largest model we have: ~53k MDL lines / 1.4 MB, 934
datamodel variables, 5726 root slots, 162 graphical functions, 1000 Euler
timesteps), the clear-win optimizations already implemented on top of it, and a
set of larger proposals grounded in the measured data.

## Methodology

- Harness: `src/simlin-engine/examples/clearn_profile.rs` — times each pipeline
  stage (parse → compile-via-salsa → `Vm::new` → `run_to_end`) and, with
  `CLEARN_COUNT_ALLOCS=1`, reports allocation counts / peak live bytes per stage
  via a gated counting global allocator (`CLEARN_ALLOC_HIST=1` adds a size
  histogram of the allocations and reallocs in each stage). With high `CLEARN_COMPILE_ITERS` /
  `CLEARN_RUN_ITERS` it is a focused `perf record` / `callgrind` target.
- `CompiledSimulation::bytecode_profile()` — opcode histogram + table sizes.
- CPU: `perf record -g --call-graph dwarf` and `valgrind --tool=callgrind`
  (exact call counts). Memory: the counting allocator. Machine: Ryzen 9950X.
- Numbers below are `opt-level="z"` + LTO builds unless noted; the release
  profile is `opt-level=3` on every target (see the build levers below).
  Profile builds add `CARGO_PROFILE_RELEASE_DEBUG=1
  CARGO_PROFILE_RELEASE_STRIP=false`.

### Measuring a change

Three channels, each answering a different question. **None substitutes for
another**, and a change is not established until the question you are actually
asking has been answered by the channel that can answer it.

| channel | question it answers | tool |
|---|---|---|
| exact instruction attribution | *did the intended work disappear?* | `valgrind --tool=callgrind` |
| retired instructions / branches | *how much work disappeared?* | `perf stat` |
| cycles / wall clock | *did it get faster?* | `perf stat`, interleaved A/B |

**Callgrind is deterministic** and immune to both binary layout and machine
load. It is the right first measurement for any change with a mechanism: it
says whether the work you meant to remove is gone, per function and per source
line, with no statistics. A change whose per-call cost is unchanged did not
fire, whatever the end-to-end counters say.

**Retired instructions and branches are properties of the program**; cycles are
a property of the machine executing it. That distinction sets the noise floors,
and they are three orders of magnitude apart. Measured on the C-LEARN run
across six independent build+run pairs of identical source:

| channel | sd across builds | a 2.7% effect is |
|---|---|---|
| instructions | **0.026%** | ~104 sigma |
| branches | **0.028%** | ~96 sigma |
| cycles, quiet machine | 1.65% | 1.7 sigma |
| cycles, machine under load | 9.9%–11% | 0.24 sigma |

Those floors are the C-LEARN *run* on the Ryzen test machine. The whole-process
*compile* measurement the compiler-unification ledger uses
(`CLEARN_PROFILE=compile CLEARN_COMPILE_ITERS=5 perf stat -e instructions`, a
single binary, Apple M-series) has a wider instruction-channel floor: 0.13%
across nine identical-binary runs, so a compile delta under ~0.15% is
unresolved there and needs interleaved A/B pairs with every run on one side
landing on one side of every run on the other.

So a few-percent effect is resolved by one build pair on the instruction
channel and is **not** resolvable on the cycles channel without a deliberate
protocol. Reaching for multi-build A/Bs to establish an instruction-count
reduction wastes hours the instruction channel settles in one pair; quoting a
cycles delta from one pair asserts something the measurement cannot support.

**Every cycles claim needs a null control from the same session.** Run the
identical binary as both sides of the A/B, interleaved, alongside the real
comparison. The apparent delta it produces is that session's floor. A measured
example, taken at load average 4–9:

```
identical binary, both sides, 5 interleaved rounds, medians:
  instructions   -0.003%
  branches       -0.004%
  cycles         -1.540%     <- a "win" from nothing
```

A cycles delta that does not clearly exceed the session's own null delta is
**unresolved, and must be reported as unresolved rather than as a small win**.
Use the session's null, never a floor recorded here or anywhere else: machine
conditions vary hour to hour, and taking a historical figure for the current
one is what turns noise into a reported result.

**Contention is a reason to wait, not to average harder.** Resolving 3% at the
quiet-machine sd of 1.65% needs about 5 builds per side; at a contended 9.9% it
needs about 175. The second is not a measurement plan. Check the load average
before starting, pin with `taskset`, interleave A/B/A/B so drift is shared, take
medians, and reject outliers explicitly rather than letting them widen the
spread.

**Prefer a structural check to a statistical one where the change admits it.**
When a change is confined to a function that is `#[inline(never)]` and keeps its
signature, its callers' machine code should be unchanged. Verify it by
disassembling both binaries and diffing the caller.

The claim to check is *instruction-sequence-identical modulo relocation*, not
byte-identical: adding code anywhere shifts the text section, so absolute branch
targets and every rip-relative displacement move even when nothing about the
caller changed. Normalise those, then require the same instruction count, the
same mnemonics and operands, and the same in-function branch offsets.

That is a binary answer rather than a sample, and it directly detects the
failure mode that has bitten this file's eval-loop work repeatedly: a change
leaking into `eval_bytecode` and perturbing the register allocation of a very
large function. Treat a single differing instruction as a hard stop and explain
it before quoting any number.

**Size a fast path by the work it replaces, not by how often it applies.** How
many inputs are *eligible* for a shortcut and how many *benefit* from it are
different questions, and only the second predicts the outcome. A shortcut has
its own fixed cost, so it wins only where the work it displaces exceeds that
cost -- which usually means a size threshold, and a fallback that is now paid on
every input below it. Cost both sides before predicting, and gate on the
threshold rather than on eligibility.

**Decide what would falsify the change before measuring it.** Write down the
predicted delta per channel, and the signatures that would mean it did not work:
end-to-end instructions falling while the callgrind per-call cost is unchanged
means something other than the intended mechanism moved; instructions falling
while the branch count holds means a branchy inner loop was not actually
replaced. Stating these in advance is what makes the eventual number a result
instead of a reading.

**State which channel a recorded number came from.** A verdict written as "only
~1.5%" invites the next reader to compare it against whatever floor they happen
to have in mind, and the floors differ by three orders of magnitude between
channels. Write "1.5% of retired instructions" or "1.5% of cycles"; a
percentage with no channel attached is how a cycles floor ends up being applied
to an instruction measurement.

## Measured baseline (before this work)

| Phase | Wall (per iter) | Allocations | Dominant costs |
|---|---|---|---|
| parse (`open_vensim`) | ~69 ms | 0.82M | MDL lexer/parser/convert |
| **compile (salsa)** | **~3574 ms** | **73M (8.9 GiB churned, 3.3 MiB retained)** | ~30% raw `malloc`/`free`; `reconstruct_variable` 6.4%; `canonicalize`+`to_lowercase` ~3.8% (6.1M `to_lowercase` calls); parse front-end ~4% (3.86M `parse_app`) |
| `Vm::new` | ~0.6 ms | 7.8k | buffer allocation |
| **run (`run_to_end`)** | **~342 ms** | **2.9M (~2944/timestep)** | `eval_bytecode` 35%; **~15% `make_module_key` clone + `HashMap<ModuleKey>` SipHash inside `EvalModule`**; `RuntimeView` machinery ~9% |

Two structural facts dominate:

1. **Compile is ~10× the run and is allocation-bound.** ~30% of compile
   instructions are in glibc `malloc`/`free`, churning millions of tiny,
   short-lived allocations (AST `Box` nodes, `canonicalize` `String`s, repeated
   `datamodel::Variable` reconstruction). The front-end node count is amplified
   because arrayed equations are parsed per declared element.
2. **The run's entire per-timestep allocation churn was one thing:** the
   `EvalModule` opcode rebuilt a `(String, BTreeSet<String>)` module key and
   SipHashed it for a `HashMap` lookup on every module evaluation, every step
   (~1344 `EvalModule` × 1000 steps ≈ 1.34M key constructions, each ≥2 heap
   allocations).

Bytecode shape (unchanged by this work): 64420 opcodes (8 B each = 503 KiB);
34673 are flow (the hot per-step program = 277 KiB). Flow histogram: `LoadVar`
32.8%, `Op2` 18.9%, `LoadConstant` 12.1%, `AssignCurr` 6.8%, `If`/`SetCond` 4.7%
each. So ~70% of executed opcodes are load / store / binary-op.

## Clear wins implemented

All three are behavior-preserving: the 3530 engine lib tests, 91 `simulate`
integration tests, and the `clearn_residual_exactness` guard (C-LEARN matches
Vensim's `Ref.vdf` byte-for-byte) all pass, and the compiled bytecode is
byte-identical (64420 opcodes).

### 1. `EvalModule` index dispatch (run −17%, run allocations → 0)

`make_module_key` cloned a `String` + `BTreeSet<String>` and the `EvalModule`
opcode SipHashed it for a `HashMap<ModuleKey, _>` lookup, every module-eval every
timestep. Replaced the three keyed maps (`flow_modules` / `stock_modules` /
`initial_modules`) with a single `Vec<ResolvedModule>` indexed by integer, plus a
`child_targets: Vec<u32>` per module resolving each `EvalModule` declaration to
its child's index **once** at `Vm::new`. The eval recursion threads a
`module_idx` and array-indexes; the `ModuleKey` map survives only for the cold
`set_value` / `clear_values` literal-override paths.

- **run 342 → 283 ms (−17%)**; `run_to_end` allocations **2.94M → 0**.
- Post-change profile: `eval_bytecode` 35% → 46% (now the real work), the ~15%
  SipHash cost gone entirely.

### 2. Allocation-free 0-arity-builtin check (compile −3%, −1.45M allocs)

`Expr0::reify_0_arity_builtins` called `id.as_str().to_lowercase()` (a heap
allocation) on **every** variable reference just to test membership in a
9-element ASCII set. Added `builtins::is_0_arity_builtin_fn_ci` (ASCII
case-insensitive, allocation-free) and only materialize the lowercased name in
the rare case a genuine `pi`/`time`/etc. reference is reified.

- **compile 3574 → 3458 ms (−3.2%)**, −1.45M allocations.

### 3. Cached project dims in `compile_var_fragment` (−130k allocs)

`compile_var_fragment` (salsa-tracked, once per variable) rebuilt the full
datamodel dimension `Vec` via `source_dims_to_datamodel(project.dimensions(db))`
per variable; switched to the already-cached `project_datamodel_dims` query
(`returns(ref)`). Provably equivalent (the cached query is defined as exactly
that call). Marginal on C-LEARN (only 18 dims) but strictly correct and removes a
redundant per-variable rebuild.

## Build-level levers (measured, near-free, the biggest wins) — IMPLEMENTED

These need no engine-code changes and dwarf the code-level compile work. Lever A
applies to every target; the WASM bundle additionally builds with fat LTO
(`src/engine/build.sh` builds it as a cdylib alone, which is what makes cargo
pass `-C lto`) and `codegen-units=1` (`.cargo/config.toml`, which carries the
measured trade-offs). Lever B is **native-only**: the WASM bundle never links
mimalloc.

### A. `opt-level = 3` for native (compile −30%, run −41%)

`[profile.release]` is `opt-level = 3`, and the WASM bundle takes it too: on
C-LEARN v77 through `src/engine/bench/clearn-alloc.mjs` the browser bundle's
open-compile-run pipeline is 0.59x to 0.62x the `opt-level="z"` time (compile
0.50x to 0.57x, run 0.60x to 0.76x on V8 12.4 and 13.6) for a bundle 1.8x
larger raw (5.38 MB to 9.52 MB after wasm-opt) and 1.5x larger compressed
(brotli 1.27 MB to 1.87 MB). Native, measured on C-LEARN (with the code wins in):

| | opt="z" | opt=3 | delta |
|---|---|---|---|
| compile | 3485 ms | 2450 ms | **−30%** |
| run | 283 ms | 168 ms | **−41%** |

Caveat documented in `.cargo/config.toml`: a `RUSTFLAGS` *env var* replaces the
target rustflags, so don't set `RUSTFLAGS` during a wasm release build.

### B. mimalloc for native (compile −40% on top of opt=3)

Compile is allocation-bound, so a faster allocator pays off directly:

| | system malloc | mimalloc | delta |
|---|---|---|---|
| compile | 2450 ms | 1459 ms | **−40%** |
| run | 168 ms | 167 ms | none (run is allocation-free post-win #1) |

Wiring: the binaries (`simlin-cli`, `simlin-serve`, `simlin-mcp`) set
`#[global_allocator] mimalloc::MiMalloc` in their `main.rs` (native binaries,
never wasm) and depend on the `mimalloc` crate directly. `libsimlin` (the cdylib
used by pysimlin via cffi and by C/C++ FFI, *and* the wasm crate) gates it behind
an opt-in `mimalloc` feature that is additionally `cfg(not(target_arch =
"wasm32"))`; pysimlin's build (`Makefile`, `scripts/build_wheels.py`) enables
`--features mimalloc`. The feature is off by default. None of the three binaries
depends on `libsimlin`: the CLI deliberately does not, so its dependency closure
holds no cdylib/staticlib crate. libsimlin's fixed-name (unhashed) rlib cannot
coexist with the workspace's feature-unified variant of itself, so depending on
it relinked the CLI on every `cargo build` <-> `cargo build -p simlin-cli`
switch.

**Cumulative compile: 3574 → 1459 ms (−59%)** via code wins + opt=3 + mimalloc.
**Cumulative run: 342 → 168 ms (−51%)** via code win + opt=3.

## Run-side proposals (post-win hot path: `eval_bytecode` 46%, `RuntimeView` ~20%)

### R1. Bounds-check elimination on `curr`/`next` indexing — INVESTIGATED, not worth it

The hot opcodes index `curr[module_off + off]`, `next[...]`,
`bytecode.literals[id]`, and `context.graphical_functions[gf]`. Disassembly
confirms `eval_bytecode` carries 127 `panic_bounds_check` sites, so LLVM is not
eliding them. An earlier draft of this doc proposed `get_unchecked` here as "the
biggest code-level run win" — direct measurement disproves that.

**Measured ceiling: ~0.** Replacing the bounds checks on the hottest scalar arms
(`LoadVar`, `LoadConstant`, `LoadGlobalVar`, `AssignCurr`/`Next`,
`AssignConstCurr`, `BinOpAssignCurr`/`Next`) *and* the dispatch `code[pc]` access
with `get_unchecked` moved the C-LEARN run by less than run-to-run noise (165–172
ms across runs, vs ~167 ms checked). On a modern out-of-order core at
`opt-level=3` an always-in-bounds check is a perfectly-predicted, never-taken
branch with an out-of-line cold panic path — effectively free. (The ~10% in
`RuntimeView::flat_offset` is a per-element `SmallVec` rebuild + linear sparse
search, *not* a bounds check — see R4.)

**Can safe code eliminate them (the optimizer-coaxing question)?**
- The dispatch index is *already* check-free in safe code: `while pc <
  code.len() { match &code[pc] }` — the loop guard dominates the access with the
  identical bound, so LLVM proves it in range. This is the canonical safe-BCE
  pattern (the Go equivalent is the elision after `for i := 0; i < len(s);
  i++`). Confirmed: `get_unchecked` on `code[pc]` made no difference.
- The data-driven indices cannot be made check-free in safe code. `off` is `u16`
  opcode data and `module_off` is a runtime module base; the in-range invariant
  is established by a separate validation pass and is not re-derivable at the
  access site from types or local control flow. The safe idioms that *do* elide
  don't fit: sequential iteration / `chunks`/`windows` (this is random access);
  fixed-size `[T; N]` (n_slots is runtime); power-of-two masking `i & (len-1)`
  (needs a compile-time-constant power-of-two length); a hoisted `assert!(i <
  len)` (that *is* the check, relocated — `i` is per-opcode so it can't hoist out
  of the loop). Removing them would require `unsafe` `get_unchecked` + a static
  validation pass (the `Stack` pattern), verifiable under miri — and miri detects
  OOB at runtime, it does not remove checks.

**Decision: do not implement.** `unsafe` in a `#![deny(unsafe_code)]` crate, plus
a validation pass and a miri burden, is not justified for a sub-noise gain. The
run's *instruction count*, not its bounds checks, is the lever — that is R2. The
"bytecode density / dcache" intuition is also a non-issue: the program streams
linearly (prefetcher-friendly) and is already 8 B/opcode.

### R2. 3-address binop fusion — IMPLEMENTED (run −6.8% on C-LEARN)

~70% of executed opcodes are load/store/binop. A stack VM evaluates `a op b` as
`LoadX; LoadY; Op2` (3 dispatches); folding the leaf operand loads into the op
makes it 1. Crucially **the `curr[]` slot array is already the register file** —
variables live at fixed offsets — so the fused ops read operands straight from
`curr[]`/`literals` (or pop one from the stack), and the stack carries only
nested subexpression results.

**Opcode budget forced a 2-operand design.** A full 3-operand `dst = a op b`
(3×u16 + Op2 = 7-byte payload → 10-byte enum) blows the asserted 8-byte `Opcode`.
So the fused ops are 2-operand *pushing* forms (≤6 bytes): `BinVarVar`,
`BinVarConst`, `BinConstVar` (both operands are leaves; fuse `Load; Load; Op2`,
3→1) and `BinStackVar`, `BinStackConst` (lhs already on the stack; fuse `Load;
Op2`, 2→1). A leaf *assignment* `dst = a op b` keeps the existing
`BinOpAssignCurr` for the store (so it stays 3 ops, not 1) — those are a minority
(`BinOpAssignCurr` ≪ `Op2`).

**Where it runs.** A late `ByteCode::fuse_three_address` pass applied to the Vm's
flow/stock execution bytecode at `Vm::new`, reusing the symbolic peephole's
jump-target guard + old→new PC remap and preserving `max_stack_depth`. It runs at
`Vm::new` rather than compile time deliberately: the fused opcodes have no
symbolic form (`SymbolicOpcode` deliberately has no 3-address variants), so
running the pass earlier would produce bytecode the salsa-cached artifact cannot
represent -- and the `CompiledSimulation` must stay the pure resolution of the
cached symbolic fragments. The `Vm`'s private execution copy is where the
optimization lives. Per-`Vm` fusion is a linear scan, negligible
vs a run. Initials are left unfused (run once; `extract_assign_curr_offsets` reads
their `AssignCurr` targets).

**Result.** Flow opcodes 34673 → 26539 on C-LEARN (−23.5%); run 166.8 → 155.4 ms
(−6.8%). The opcode reduction outweighs the runtime gain because the f64
arithmetic, stock phase, save/copy, and array machinery (`flat_offset`, R4) are
untouched — only the scalar *dispatch* shrinks. Scalar-heavy models benefit more
than array-heavy C-LEARN. Behavior-preserving: full suite + `clearn_residual_
exactness` pass, with dedicated fusion-pass and operand-order unit tests.

A true register VM with a scratch-register file and a 3-operand instruction set
(register allocation over each expression DAG) would cut more, but is a large
codegen rewrite touching the symbolic/incremental layer; the 2-operand fusion
captures most of the dispatch win at a fraction of the risk.

### R3. Faster dispatch

The dispatch is `while pc < len { match &code[pc] { … } }`, which LLVM lowers to a
jump table (one indirect branch whose target is data-dependent → BTB-unfriendly).
Classic threaded dispatch (computed-goto / guaranteed tail calls) would spread the
indirect branch across handlers for better prediction, but **stable Rust offers
neither computed-goto nor guaranteed TCO** (the `become` keyword is unstable).
Superinstructions are the portable lever, and the family below is implemented.
Each removes a dispatch **and the operand work behind it**, which is why a
removed dispatch costs ~25.9 instructions rather than the ~10 a bare dispatch
costs — size proposals in this family against 25.9 or they read ~3x cheaper
than they are. Both figures were measured by injecting an empty `ProbeNop`
opcode at controlled rates and taking the instruction slope, validated by
bit-identical results at every rate and an exactly linear dispatch count.

Landed, all created by `ByteCode::fuse_three_address` on the Vm's private
execution copy unless noted:

| form | fuses |
|---|---|
| `SelectIf` / `SelectIfAssignCurr` | `SetCond; If[; AssignCurr]` |
| `AssignVarCurr` / `AssignInitialCurr` / `AssignModInputCurr` | a leaf load + its store |
| `BinStackModInput` / `AssignStackModInputCurr` | module inputs as a fusible leaf |
| `LoadPrevConst` | `LoadConstant; LoadPrev` (the `PREVIOUS` fallback) |
| `ApplyTerConst` | a 3-arity builtin's literal trailing operand |
| `SubVarPrev` / `BinStackPrev` | the `v - PREVIOUS(v)` delta, 4->1 and 3->1 |
| `LookupDirect` (codegen, so it reaches wasmgen) | a lookup's constant element offset |

`SetCond; If` is safe to fuse because codegen is the sole producer of both and
emits them together, so the pair is adjacent by construction rather than by
luck.

Two rules this family established. **A fusion may live in the symbolic layer
iff the fused opcode has a `SymbolicOpcode` form**, because `CompiledSimulation`
must stay the pure resolution of the cached symbolic fragments; the rest are
Vm-local and never reach wasmgen. And **score a helper-variable idea against
the post-fusion stream**: hoisting a repeated subexpression into a shared aux
replaces each use with a `LoadVar` — one dispatch, exactly what a fused opcode
costs — so the hoist is worth zero wherever a superinstruction can match the
pattern, while still paying for a store and a slot.

What this family cannot reach: **mispredicts**. The dispatches superinstructions
remove best are the perfectly-predicted ones — `SetCond` always jumps to `If`'s
arm — so fusing them removes instructions and branches but not branch misses.
Measured: branches −6.3% against branch-misses −2.6%. The mispredict cost lives
in the genuinely-unpredictable dispatches, which is where #604's hypothesis
would have to be tested if anyone retries it.

Remaining: revisit explicit tail-call dispatch if/when `become` stabilizes; a
register VM reduces dispatch count more than any dispatch-mechanism change.

### Round 2 wins (2026-06-03, measured on Apple M-series / Asahi)

Baseline on this machine: C-LEARN `run_to_end` 151 ms (1000 Euler steps),
WORLD3-03 1.3 ms. Note the machine difference from the round-1 numbers: on
this core the run is throughput-bound (IPC ~4.5, branch-miss rate ~1.0%), not
mispredict-bound like the Ryzen profile above -- but the lever is the same
(less executed work per step).

**Constant folding (`compiler::fold`, run −2%, bytecode −5%).** The flow
program re-evaluated every `literal op literal` subtree per step -- 792
`BinConstConst` sites on C-LEARN, including one per negative literal (unary
minus lowers to `0 - x`). A fold pass in `Var::new` (the chokepoint every
fragment lowering funnels through) collapses constant-only
subtrees at compile time, computing results with the VM's own
`eval_op2`/`is_truthy` so folds are bit-identical by construction. Only
IEEE-exact ops fold; `^` (libm `powf`) and transcendental builtins stay
runtime so compiled artifacts (and the wasm blob) remain platform-
deterministic. Folding also cascades into deeper 3-address fusion
(`BinVarConst` 726 -> 1034). WORLD3 has no foldable sites (unchanged).

**Linear-run fast paths (run −7%).** `RuntimeView::dense_linear_start()` --
"no sparse mappings, strides are row-major for the current dims", i.e.
`is_contiguous` minus the `offset == 0` requirement -- keys three fast paths:
`offset_for_iter_index` (direct `start + k`), the `BeginIter` precompute
decision (offset slices no longer precompute a `Vec` of offsets), and a
slice-fold fast path in `reduce_view` (same row-major order, bit-identical
reductions). `vector_elm_map` (168 sites on C-LEARN, the largest
`flat_offset` caller at ~4% of the run) hoists the offset view's addressing
out of its per-element loop. `RuntimeView::same_shape()` replaces the
SmallVec `PartialEq` in `LoadIterViewAt` (an out-of-line memcmp per element
per site, ~2% of the run) with a branchless ≤4-wide compare.

**Cumulative round 2: C-LEARN run 151 -> ~137 ms (−9%).** Both rounds
together (vs. the 342 ms pre-round-1 baseline, different machine): the
per-step program shrank from 34,673 to ~23,000 dispatched opcodes.

**Negative result (reinforces #604).** Rewriting `vector_elm_map`'s
strict-slice base as a precomputed affine dot product (provably equivalent,
structurally less work per element) measured a consistent ~5 ms *regression*
-- enlarging the function perturbed the codegen of the giant inlined
`eval_bytecode`. Treat every eval-loop-adjacent "improvement" as
unproven until measured; structural arguments do not survive contact with
the inliner.

**Negative result #2: the #602 lookup last-segment memo (implemented,
reverted).** A per-(module, GF) hint that validates the previous binary-search
lower bound in O(1) measured only ~0.7 ms gross (137.4 -> 136.7; the ~8-deep
search over C-LEARN's 229-point tables is already well-predicted at a
slowly-advancing index). Adversarial review then found the hint diverges from
`lookup` on unsorted-x tables -- which ARE reachable, no import path validates
x ordering (GH #715) -- so soundness required gating the hint on a per-table
sortedness check. Every sound formulation (sentinel checked inside
`lookup_with_hint`, or hoisted to the dispatch arm) measured a consistent
+9..15 ms regression: one extra branch + call edge in `eval_bytecode`'s
`Lookup` arm perturbed the giant function's codegen (instructions +1.5%,
cycles +6.4%, branches *down*, IPC 4.34 -> 4.13). Net: gross win 0.5%,
soundness cost ~7% -- reverted wholesale. Lesson on top of #604: the
interleaved-A/B-worktree protocol caught a sub-agent's "machine variation"
rationalization; never accept a perf delta explanation without an
interleaved A/B on freshly built binaries.

**Negative result #3: #712 B2 invariant-prefix execution (implemented,
preserved on `experiment-712-b2-execution`, not merged).** Stage B1 (the
classifier + runlist partition, KEPT -- behavior- and perf-neutral, +0.56%
compile) showed 45.4% of C-LEARN's root flow opcodes write run-invariant
slots. The B2 execution stage (split at `flows_invariant_opcode_len`,
invariant prefix evaluated once per `run_to`, per-step scatter of 868 slots
from a snapshot) is complete and gate-green -- VDF byte-exactness, wasm
parity, zero-alloc all hold -- but did not clear the keep bar:

- **The static-opcode share was a bad proxy.** Removing ~45% of the root
  flow stream's static opcodes from per-step execution removed only **−1.3%
  of executed instructions**: C-LEARN's per-step work is dominated by module
  evaluations and per-element array/iteration loops (which re-execute their
  opcodes many times), not the root scalar stream the prefix lives in.
- **The wall-clock effect is below the build-layout noise floor.** Two
  independent build pairs measured opposite signs (+3.5% vs −1.0% on
  C-LEARN): two builds of the *same base source* differed by ~6 ms (135.9
  vs 142.2). Interleaved A/B controls machine conditions, but each *build*
  samples a ±2-4% binary-layout lottery; an effect of ~1% cannot be
  resolved by one build pair. World3 trended slightly negative both times.
- Branch-misses fell 8.4%, so a mispredict-bound core (the round-1 Ryzen)
  might see a real win -- that is the retry condition recorded on GH #712.

**Negative result #4: the uniform-grid lookup index (implemented, not
landed).** Graphical-function x-axes are overwhelmingly uniform -- 86.6% of
corpus tables exactly, another 4.7% to within an ulp -- so `lookup`'s binary
search can be replaced by an O(1) position computed from the table's endpoints
and then verified, falling back to the search when the check fails. It is exact
on any sorted axis (the check `x[k-1] < index <= x[k]` identifies the same
position the search returns) and needs no stored metadata, so nothing is
threaded through the dispatch arm -- the property whose absence sank #602.

Measured, against predictions registered before implementing:

| | predicted | measured |
|---|---|---|
| C-LEARN instructions | -2.7% | **-0.63%** |
| WORLD3 instructions | -3.1% | **+0.59%** |
| C-LEARN `vm::lookup` Ir | -60% | **-34.8%** |
| WORLD3 `vm::lookup` Ir | -35% | **+7.7%** |

The guess costs ~50 instructions (two divisions, a saturating float-to-int
cast, two bounds-checked loads for the check) against ~12 per search probe, so
it pays only above about four probes. C-LEARN's tables have a median of 251
points -- an eight-probe search -- and win; WORLD3's median is 7, a three-probe
search, and lose. Gating on a 32-point minimum recovered C-LEARN and left
WORLD3 still 7.7% worse in `lookup`, because the restructured fallback is paid
by every table below the gate, which is most of the corpus. Forcing the helper
inline changed nothing (it was already inlined).

**Why this one is worth reading before designing an experiment**: unlike the
three above, where the effect was merely small, here the aggregate and the
mechanism DISAGREED. End-to-end C-LEARN alone reads as a -0.63% win and a
plausible cycles figure could have been quoted to match it. Only the per-call
mechanism channel showed WORLD3's `lookup` getting 7.7% worse underneath that
aggregate, and only a pre-registered per-model prediction made the sign flip
impossible to read as "smaller than hoped". A single-model, single-channel
measurement ships this change.

The standing lesson is the sizing rule under "Measuring a change": a census
established that ~100% of both hero models' tables were ELIGIBLE, which is not
the same as benefiting, and the prediction costed the search being removed
without costing the guess replacing it. The patch is recoverable from the
round's scratch artifacts (`p9_option_c.patch`) if a cheaper guess ever makes
the break-even worth revisiting.

Methodology consequence for future rounds: the ~4% figure above bounds a
WALL-CLOCK/CYCLES claim from a single build pair, and nothing else. Retired
instructions and branches have an sd of ~0.026% across builds, so the same
effect is resolved there by one pair; see "Measuring a change" above for the
per-channel floors and the null-control rule.

### R4. `RuntimeView` allocation + `flat_offset` (~20% of post-win run)

`PushVarViewDirect` rebuilds `SmallVec`s (dims, strides, dim_ids) on every
execution; `flat_offset` (10.3%) recomputes row-major offsets per element. For
arrayed models this is now the #2 run cost. (`PushVarViewDirect` is the base
of a dynamic subscript, the one view shape that cannot be precomputed; every
whole-array and constant-subscript view already takes the `PushStaticView`
path, which is part (a) of the proposal below realized for those shapes.)

Proposal: (a) push more views through the compile-time `PushStaticView` path
(precomputed `StaticArrayView`) and store dynamic view descriptors in the
`ByteCodeContext` referenced by id (as `dim_lists` already does for dim ids),
eliminating per-op `SmallVec` construction; (b) ensure the `is_contiguous` fast
path in iteration/reduction is taken for the common dense case so `flat_offset`'s
general strided arithmetic only runs for transposed/sparse views.

- Effort: medium. Risk: low–medium (array semantics are well-tested by
  `array_tests`).

## Compile-side proposals (the bigger pie — but build levers A+B capture most of it)

After opt=3 + mimalloc the compile is ~1.46 s (from 3.57 s) with **no code
changes**. The following are second-order and worth it only if compile latency
remains a UX problem after the build levers (it matters for the salsa
*incremental* edit loop more than cold compile).

### C1. Arena-allocate the transient parse AST — NOT the dominant allocator

Re-measured after compile round 3: the parser is no longer where the
allocations are. Per cold C-LEARN compile, `Expr0::clone` accounts for 212,184
allocations (3.4% of compile instructions) and the `Expr0`/`Expr2`/`Expr3` drop
glue for ~7% — so an arena is worth ~10% for a large, medium-risk change, and
the top allocation site is not the parser at all but `Compiler::intern_name`
(320,650 allocations per compile, ~10% of all 3.24M; see C5). The original
figure below (3.86M transient allocations) predates the salsa pipeline and no
longer describes the code.

### C1. Arena-allocate the transient parse AST

The equation parser builds `Expr0` with `Box` children + `Vec` args — 3.86M+
transient heap allocations, all lowered to `VariableStage0` and dropped.
Allocating the AST in a per-parse arena (a `bumpalo`-style bump allocator; the
engine carries no such dependency) would turn these into pointer bumps. The constraint: the salsa-cached result
(`ParsedVariableResult`) must be owned/`'static`, so the arena can only back the
transient parse→lower step, with the cached value being the owned lowered form.
Much of this benefit is captured more cheaply by mimalloc (B); pursue the arena
only if profiling after B still shows the parser as a hotspot.

- Effort: large (thread an arena through the parser; verify nothing cached
  retains an arena reference). Risk: medium.

### C2. Halve `reconstruct_variable` — MOOT

`reconstruct_variable` is now the salsa-cached `reconstruct_model_variables`,
and every caller is on the LTM / analysis / patch path; it does not appear in
an ordinary compile profile at all. The 2x duplication that WAS real, and is
fixed, was a different function: `variable_dimensions` demanded the per-variable
parse under an empty `ModuleIdentContext`, a cache key nothing else used, so
every variable was parsed twice per compile.

### C2. Halve `reconstruct_variable` (6.4% of compile)

`reconstruct_variable` rebuilds a full `datamodel::Variable` (ident/equation/
inflows/outflows/compat clones) and is called ~2× per variable: once in the
per-variable parse, and once in `module_ident_context_for_model` →
`collect_module_idents`. The latter only needs each variable's `(ident, kind,
is-module-call)` — a lighter projection straight from `SourceVariable` would
avoid ~half the full reconstructions (and their clones).

- Effort: medium. Risk: low–medium (changes the `collect_module_idents` input
  type; behavior must stay identical).

### C3. `canonicalize` — the lever is call elimination, not a faster slow path

`canonicalize` is still the largest non-allocator cost of a cold compile, but
neither half of the proposal below is the way to reduce it, and the reason is
worth keeping because the profile invites the wrong conclusion.

**(b) interning is done.** `Ident<Canonical>` is a 64-shard-interned `Arc`
(`common.rs`): `Clone` is a refcount bump and `PartialEq` is pointer equality.

**(a) an ASCII fast path exists and already carries almost all traffic.**
`is_canonical_needing_no_trim` is a single-pass byte-table scan returning
`Cow::Borrowed`, measured at ~90 instructions per call at the hottest site.
Only **4.6% of calls allocate** (99,322 of 2,146,745 per compile), so making
the slow path cheaper cannot reach the other 95.4% — while rewriting it puts
the GH #559 idempotence proptests, which guard the Unicode arm (titlecase,
U+00A0, quoted sections, backslash unescaping), at risk for that 4.6%.

**What works is not calling it.** Half of all calls came from one predicate
re-canonicalizing names that are canonical by construction; deleting that inner
call, which touches `canonicalize` not at all, measured −5.0% of a cold compile.
The residual worth having is narrower still: `changes_when_lowercased` is asked
about the separators the engine itself mints (`·` in every `submodel·var`
ident), which is answered from a three-character list rather than the Unicode
case tables.

### C3. `canonicalize` ASCII fast-path + ident interning

6.1M `to_lowercase` calls; ~4.6M are the `canonicalize` slow path (Vensim names
have spaces/capitals so they don't hit the alloc-free fast path). Two levers:
(a) lowercase ASCII in place into the output buffer instead of allocating a
per-part intermediate `String` (careful: keep Unicode correctness — the function
has extensive idempotence tests, #559); (b) **intern** canonical idents so
repeated canonicalization of the same name is a hashmap hit rather than a
re-derivation. (b) is broader but touches many call sites.

- Effort: (a) small/careful, (b) medium–large. Risk: (a) medium (correctness-
  critical function), (b) medium.

### Compile round 3 (2026-08-10): the salsa pipeline's own redundancy

Cold C-LEARN compile 2.119G -> 1.602G retired instructions, **−24.4%**, and a
warm single-equation edit **−47%** (median wall 38 ms -> 4.3 ms). Every change
is artifact-identical: 5215 slots, 58291 opcodes (31525 flow + 1477 stock +
25289 initial), same literal / GF / temp / dimension / view / name / module
counts. Measured as retired instructions throughout, because the machine was
contended and the cycles channel cannot resolve effects this size there.

What the round found, stated as the standing shape of the problem rather than
as five fixes: **the cold compile's redundancy was in the salsa layer's own
keying, not in the compiler.** Four of the five were a query being asked a
question it had already answered, under a key that did not say so.

| what | mechanism | share of cold compile |
|---|---|---|
| `is_dimension_name` | re-canonicalized every declared dimension name per call | −5.0% |
| `variable_dimensions` | demanded the parse under an empty `ModuleIdentContext` -> every variable parsed twice | −3.5% |
| `compile_implicit_var_fragment` | not tracked: every SMOOTH/DELAY/TREND helper recompiled per assembly | −12% cold, **−28% of a warm edit** |
| `var_phase_symbolic_fragment_prod` | not tracked: cycle gate built 135 fragments per compile for 57 distinct keys | −14.3% |
| topo-sort probe maps, `changes_when_lowercased` | SipHash and Unicode tables on the engine's own idents | −1.8%, −2.4% |

Two constraints follow, and both are cheap to violate:

- **A per-variable helper needs a per-variable key.** The two biggest wins were
  functions whose comment said salsa already cached them, because their *parse*
  was cached. Lowering and codegen are the expensive half and were not. When
  adding a per-variable compiler, the question is not "is something upstream
  memoized" but "does this function have a key of its own".
- **A projection is what keeps a per-variable query per-variable.** Both new
  queries read a three-bit `RunlistMembership` rather than the whole
  `ModelDepGraphResult`; taking the whole result would re-execute every
  fragment whenever any variable's dependencies moved, silently restoring the
  coarseness the key was introduced to remove.

### C4. Parallel fan-out of per-variable fragment compilation — designed and measured, NOT implemented

The compile is **exactly serial** (`task-clock` / `elapsed` = 1.000 over two
independent measurements). A prototype fan-out was built and measured on
C-LEARN before round 3 landed; it is not in the tree, and these are the facts
whoever implements it needs so they are not rediscovered.

**Achievable, and bounded well below the core count.** Staged prewarm (parse +
dependency memos, then per-variable fragments) reached **2.23x achieved
parallelism but only 1.34x wall speedup** (132.7 -> 99.3 ms), at +12% retired
instructions. The ceiling is a property of the query decomposition: at the time
of measurement `model_dependency_graph` (35.5% of compile) was one query per
`(model, input-set)` and could not be split by variable, `compile_implicit_var_fragment`
(12.2%) had no key to prewarm, and symbolic->concrete resolution (~20%) is
inherently sequential. Amdahl over that ~68% serial floor predicts 1.44x; the
measurement was 1.34x. Round 3 has since moved the middle two rows into keyed
queries, so the floor is lower and the ceiling correspondingly higher — but it
is still a decomposition question, not a thread-count one.

**The fan-out cannot live inside the salsa query graph.** `salsa::Database` is
`Send` but **not `Sync`**, so `&dyn Db` cannot cross a rayon boundary and
neither `assemble_module` nor `assemble_simulation` can fan out from within.
It has to run from `compile_project_incremental`, which holds a concrete
`&SimlinDb`. `Storage<Db>: Clone` clones the shared `Arc<Zalsa>` and mints a
fresh per-thread `ZalsaLocal`, so each worker takes its own **moved** handle
(`SimlinDb` is `Send`, not `Sync` — a handle may be given to a thread, never
shared with one). Every handle must drop before the next `db.sync`: `zalsa_mut`
cancels and blocks on outstanding handles, so a leaked one deadlocks the next
edit.

**Two hazards found by measurement, not by reasoning.** Both are silent.

1. **The prewarm must run AFTER the module-cycle gate, never before.**
   `compile_var_fragment` demands the recursive `compute_layout` (through
   `model_shape`), which salsa
   turns into a dependency-graph cycle panic — a process abort under
   `panic = abort` (GH #806). A prewarm placed ahead of
   `assemble_simulation`'s `project_module_graph(..).cycle_error_from(..)`
   check reopened exactly that hole: the lib suite went from its baseline to
   two extra failures, both module-cycle regression tests, and repeating the
   gate ahead of the prewarm restored the baseline exactly.
2. **The fan-out must be gated on cold-ness.** Unconditionally prewarming
   regressed the fully-cached recompile from 0.85–1.32 ms to 3.29–3.42 ms — a
   2.5–4x regression on the path that matters most for interactive editing —
   because it builds a work list over every variable and spins up workers to
   re-verify memos that are already valid.

Determinism is **not** a hazard here, and that is a measured result rather than
an assumption: the 12-repeat byte-identical determinism suites
(`fragment_determinism_tests`, `diagnostic_determinism_tests`) pass with the
prewarm active. Salsa's accumulator drain is a dependency DFS, not an execution
order.

### C6. Warm-edit latency: what a single-equation edit costs, and what still does not scale

Interactive edit latency, not cold compile, is what a modeller experiences, and
it is measured with an out-of-tree probe that drives `SimlinDb::sync` +
`compile_project_incremental` over real edits to a real model. Two facts about
the measurement itself come first, because both were got wrong on the way to
the numbers and either one silently misreports the result by an order of
magnitude.

**An "equation edit" is not one workload.** Appending a term to an equation can
change the DEPENDENCY STRUCTURE rather than just the text -- turning a bare
`INITIAL(x)` into an expression containing an `INITIAL(x)` is the case that bit
here, and C-LEARN has 177 `INITIAL(` equations. A probe that edits each variable
once measures the structural cost for every one of them. Pre-applying one edit
so that later edits only change digits is what separates the two, and it moves
the reported p90 by a factor of twelve.

**Consumer count does not predict cost.** The obvious explanation for an
expensive edit -- a constant read by many variables, each recompiling under the
one-hop rule -- is false and was measured false: the slowest variable
(`2x CO2 forcing`) has 3 references in the model and a fast one (`c uptake`) has
47. Do not spend a day on fan-out.

**Structure-preserving single-equation edit, C-LEARN** (40 edits, paired over
the same variables, before = the first two round-3 commits, after = all six):

| | before | after |
|---|---:|---:|
| median | 2.8 ms | 2.8 ms |
| **p90** | **36.0 ms** | **3.0 ms** |
| max | 73.9 ms | **6.7 ms** |
| retired instructions | 11.03G | **5.27G** |

The median was already fine; **the tail was the problem and the tail is gone.**
That tail was the per-assembly recompile of every implicit helper and the cycle
gate's un-memoized fragment probe -- the two changes keyed in round 3. A cheap
edit now costs 40.6M instructions and its profile is almost entirely salsa's
own `maybe_changed_after` verification plus the lexer re-reading the one edited
equation, which is what proportional looks like.

**What still costs a full recompile: an edit that changes the dependency
structure.** Measured at 1.798G instructions -- 85% of a cold compile -- and it
decomposes as:

| | calls | Ir | share |
|---|---:|---:|---:|
| `compile_var_fragment` | **911** of ~955 | 402M | 22% |
| `model_dependency_graph` | 1 | 565M | 31% |
| ...of which `resolve_recurrence_sccs` | 2 | 245M | 14% |
| `compile_implicit_var_fragment` | **651** (all) | 233M | 13% |

The table above was measured under a parse keyed on an interned module-ident
set of the owning model: a structural edit that grew the set minted a new key,
and a new key cannot backdate, so every variable's parse and every fragment
behind it re-ran. Under the current `(variable, project)` key
(`parse_source_variable` reads nothing of the owning model) a
module-instantiating add recompiles the added variable, the template's first
instance and the input source whose initials membership changed, and nothing
else (`db::fragment_char_tests::implicit_helper_add_is_tight_for_plain_and_module_helpers`,
`module_helper_add_reparses_only_the_added_variable`, both measured over every
tracked query with `db::exec_probe`); the structural edit's instruction cost
under that key is not yet profiled.

### C5. `Compiler::intern_name` — the top allocation site, blocked on artifact identity

320,650 allocations per cold C-LEARN compile, ~10% of all 3.24M, from two
independent causes: `intern_name` calls `name.to_string()` twice per new name
(once for `names`, once for the `name_ids` key), and `Compiler::new` re-interns
every project dimension and element name for each of ~1,600 per-variable
fragments.

The second is the real cost and cannot be hoisted naively: `NameId` assignment
order is baked into the compiled artifact (`base_gf`, `DimId`, and every
`name_id` operand), and the ids are assigned per fragment from 0 and merged by
`FragmentMerger`. Sharing a project-global prefix changes those ids. Any
attempt here must either preserve the assignment exactly or accept an artifact
change and re-baseline the goldens deliberately — which is why round 3 stopped
short of it rather than taking a ~2-3%.

## Suggested ordering

1. ~~**Build levers A (opt=3 native) + B (mimalloc native)**~~ — DONE. Measured
   −59% compile / −41% run for ~no engine code and near-zero risk
   (`[profile.release] opt-level=3` on every target, plus LTO and
   `codegen-units=1` for wasm via `src/engine/build.sh` and
   `.cargo/config.toml`; `mimalloc` global allocator on the native binaries +
   libsimlin's opt-in feature). WASM links no mimalloc.
2. ~~**R1 (bounds-check elimination)**~~ — INVESTIGATED, dropped: measured
   sub-noise (~0) ceiling; bounds checks are effectively free at opt-level=3.
3. ~~**R2 (3-address binop fusion)**~~ — DONE. Flow opcodes −23.5%, run −6.8% on
   C-LEARN; a late `fuse_three_address` pass at Vm::new (the fused opcodes have no
   symbolic form, so they must not exist before assembly). A full register VM would
   cut more but is a large rewrite.
4. ~~**R4 (RuntimeView)**~~ — largely DONE via round 2's `dense_linear_start`
   fast paths (`flat_offset` 8.2% -> ~4% of a smaller run); the residual is
   the strict-slice `vector_elm_map` base and `offset_for_iter_index`'s
   decompose path for shape-equal non-linear views (a per-loop access-plan
   cache is the next idea there — and see the round-2 negative result before
   attempting it).
5. ~~**R3 superinstructions**~~ — DONE; the family and its two rules are in the
   R3 section above. Cumulative on the LTM-augmented run, which is where the
   `PREVIOUS`-heavy forms pay most: post-fusion flow opcodes −44.3%, retired
   instructions −28.3% on C-LEARN and −33.6% on WORLD3-03. An instruction/branch
   win, not a predictor win.
6. ~~**C2 / C3**~~ — answered, and not as proposed: C2 is moot (the function
   is salsa-cached and off the ordinary compile path) and C3's two halves are
   already done or the wrong lever. The compile round 3 section above records
   what the profile actually pointed at, and what it cost.
7. **C4 (parallel fan-out)** — the largest remaining compile lever and the only
   one that needs a design rather than a fix. Read its two hazards before
   starting; both are silent, and one is a process abort.
8. **C5 (`Compiler::intern_name`)** — the top allocation site, blocked on
   `NameId` assignment order being part of the compiled artifact.
9. **C6's residual** — a profile of the structural edit under the
   `(variable, project)` parse key. The table in C6 is the profile under the
   older model-keyed parse; the execution-count tests say what recompiles
   now, and no instruction count does.
10. **LTM link-score arms** — the dominant cost of an LTM-enabled run on an
    arrayed model, and mostly a generation question rather than a VM one. An
    arm whose ceteris-paribus partial is *provably* `PREVIOUS(target)` is
    omitted and lowers to a single zero-store; on C-LEARN that is 4,335 arms
    and −19.2% of the flow program. The residual is gated on a semantics
    question, not on engineering: ~5,000 further arms are blocked solely by a
    live `time()`, because TIME is excluded from the freeze (GH #1016), and
    resolving that would roughly double the win. Do **not** substitute the
    cheaper negative test ("the link's source stayed frozen") — it asks a
    different question and silently rewrites 187 result slots. GH #977 carries
    the decomposition and the standing constraints.

    "Provably" carries a LAG-ALIGNMENT requirement that a walk stopping at the
    first `PREVIOUS` will miss: the partial equals `PREVIOUS(target)` only if
    every read is lagged by exactly one step. An ORIGINAL `PREVIOUS(z)` in the
    target's equation (which the wrap deliberately leaves untouched, so the
    partial reads `z(t-1)` where the anchor read `z(t-2)`) and a synthesized
    `PREVIOUS` nested inside another (which the subscript-index freeze produces)
    both look entirely frozen and are not aligned. Either one omits an arm worth
    close to the canonical ±1 attribution. Both are rejected, each is pinned by
    its own row in `db::ltm_value_gate_tests`, and rejecting them costs zero arms
    on C-LEARN — the win above is measured with both checks in place.

    One disclosed **value** change remains, on a model that produces non-finite
    values: a materialized arm over a `NaN` (or infinite) target computes
    `NaN - NaN` and evaluates to `NaN`, where an omitted slot is `+0.0`. It is
    reproduced both ways by
    `db::ltm_value_gate_tests::a_nonfinite_target_arm_is_omitted_to_zero_not_nan`.
    Whether `0` is the better answer is **open** and tracked as #1022:
    `src/float.rs` argues an
    engine-manufactured NaN is noise, while GH #542 built the `denom_summand`
    exclusion specifically to preserve a `NaN` score as a per-loop "undefined
    here" signal. The signal survives on the target's own series and on every
    live arm, so what changes is confined to arms with no causal dependence on
    their source.

Larger run-side swings identified during round 2 — all three were taken to
a data verdict in round 3 (2026-06-04):

- **Lazy `If` (#711) — measured NO-GO.** `SetCond`/`If` evaluate BOTH
  branches every evaluation; skipping the untaken branch needs forward jumps
  (codegen + stack-depth join validation + peephole/fusion jump maps +
  symbolic layer + wasmgen). A census (temporary VM dispatch counters + a
  stack-effect branch-span reconstruction over the fused stream) measured
  C-LEARN at exactly **30,524 executed dispatches/step**, of which lazy-If
  would skip 4,859 (**15.9%**) — but 93% of the skipped opcodes are cheap
  scalar loads/binops, so the share of RETIRED INSTRUCTIONS is only **~1.5%**
  (~35k of ~2.4M instr/step). That is measurable (the instruction channel's sd
  is ~0.026%; see "Measuring a change"), so the verdict does not rest on it
  being unresolvable — it rests on ~1.5% of instructions being a small return
  for the highest design cost of the three candidates. WORLD3: 3.25% dispatch
  share.
  The cheap part of the win has since been taken WITHOUT that machinery:
  fusing `SetCond;If[;AssignCurr]` into conditional-select opcodes removes
  **12.0% of executed dispatches** against this item's projected 15.9%, resting
  on the pair being adjacent by construction (`compiler::codegen`'s `Expr::If`
  arm is the sole producer of both and emits them together; executed counts are
  exactly equal at 1,874,169 each). What remains here is the residual after
  that fusion, against the full forward-jump cost.
  Notably **69.8% of the skippable dispatches sit behind constant
  conditions** (1,300 of 1,679 flow `If` sites take the same branch for the
  whole run) — a compile-time / #712-family observation, not a runtime-jump
  one. Revisit only on a mispredict-bound core or alongside a
  threaded-dispatch rewrite (#601).
- **Time-invariant hoisting** (#712) — constants are re-assigned and
  constant-derived auxes re-computed every step; a "constant phase" computed
  once per `run_to` (re-run after `set_value`) could skip them. **Stage B1
  landed** (classification + flow-runlist partition + split metadata,
  behavior-neutral; see
  [the design note](/docs/design-plans/2026-06-04-time-invariant-hoisting.md)):
  the run-invariant flow vars are classified (C-LEARN: 868 invariant slots
  of the root flow phase, oracle-verified bit-constant with 0 violations;
  WORLD3: 78) and reordered into a contiguous flow-runlist prefix, with the
  prefix opcode length recorded on `CompiledModule`. B1 still runs the whole
  program every step, so it is **perf-neutral** as expected -- two independent
  interleaved A/Bs (manager + reviewer, the reviewer's with
  `CLEARN_RUN_ITERS=200` over 5 rounds) measured ~135.5 ms vs ~136.3 ms,
  delta ≤0.8 ms, within noise. An earlier single-run comparison (3 rounds)
  appeared to show a ~9.5 ms gain (145.0 -> 135.5 ms), but that delta came
  from an anomalously slow base binary, not the source change: build-to-build
  binary-layout and cold-start variance can reach several ms for identical
  source, and whichever binary runs cold typically looks slower. **Methodology
  lesson**: interleaved A/B controls machine conditions but not binary-layout
  luck — warm both binaries before timing and require the delta to reproduce
  across multiple interleaved rounds before believing it. Stage B2 (run the
  invariant prefix once per `run_to`; snapshot + copy-forward into each saved
  chunk; re-run after `set_value`; wasmgen keeps the single reordered program)
  was implemented, gate-green — and did NOT clear the keep bar: see negative
  result #3 above (preserved on `experiment-712-b2-execution`).
- **Lookup last-segment memo** (#602) — implemented, then reverted: see
  negative result #2 above (gross win 0.5%, soundness cost ~7%).
