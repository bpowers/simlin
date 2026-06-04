# Engine performance: profile and optimization opportunities

Status: analysis + two rounds of wins landed. Round 1 2026-05-19; round 2
(constant folding + linear-run fast paths, below) 2026-06-03.

This documents an empirical CPU/memory profile of **compiling and simulating the
C-LEARN hero model** (the largest model we have: ~53k MDL lines / 1.4 MB, 934
datamodel variables, 5726 root slots, 162 graphical functions, 1000 Euler
timesteps), the clear-win optimizations already implemented on top of it, and a
set of larger proposals grounded in the measured data.

## Methodology

- Harness: `src/simlin-engine/examples/clearn_profile.rs` — times each pipeline
  stage (parse → compile-via-salsa → `Vm::new` → `run_to_end`) and, with
  `CLEARN_COUNT_ALLOCS=1`, reports allocation counts / peak live bytes per stage
  via a gated counting global allocator. With high `CLEARN_COMPILE_ITERS` /
  `CLEARN_RUN_ITERS` it is a focused `perf record` / `callgrind` target.
- `CompiledSimulation::bytecode_profile()` — opcode histogram + table sizes.
- CPU: `perf record -g --call-graph dwarf` and `valgrind --tool=callgrind`
  (exact call counts). Memory: the counting allocator. Machine: Ryzen 9950X.
- Numbers below are the shipped `[profile.release]` (`opt-level="z"`, LTO)
  unless noted. Profile builds add `CARGO_PROFILE_RELEASE_DEBUG=1
  CARGO_PROFILE_RELEASE_STRIP=false`.

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

These need no engine-code changes and dwarf the code-level compile work. Both are
**native-only**: the WASM bundle (built via `cargo build --target
wasm32-unknown-unknown --release`) keeps `opt-level="z"` for download size and
never links mimalloc.

### A. `opt-level = 3` for native (compile −30%, run −41%)

`[profile.release]` is now `opt-level = 3`. The WASM bundle is forced back to
`opt-level=z` by `.cargo/config.toml` (`[target.wasm32-unknown-unknown] rustflags
= ["-C", "opt-level=z"]`) — keyed on the target, so every wasm build path stays
size-optimized regardless of invocation (verified: wasm bundle 7.19 MB at z vs
9.75 MB at 3). Measured on C-LEARN (with the code wins in):

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
flow/stock execution bytecode at `Vm::new`, reusing `peephole_optimize`'s
jump-target guard + old→new PC remap and preserving `max_stack_depth`. It runs at
`Vm::new` rather than compile time deliberately: the `CompiledSimulation` stays a
pure, *symbolizable*, salsa-cached artifact (the symbolic roundtrip tests
symbolize it; the fused opcodes have no symbolic form), and the `Vm`'s execution
copy is where the optimization lives. Per-`Vm` fusion is a linear scan, negligible
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
Portable options:

- **More superinstructions** for the top opcode bigrams/trigrams (e.g.
  `LoadVar; LoadVar; Op2`, `LoadConstant; Op2`). Each fused opcode removes a
  dispatch; incremental and low-risk. This is the portable lever today.
- Revisit explicit tail-call dispatch if/when `become` stabilizes.
- R2 (register VM) reduces dispatch count more than any dispatch-mechanism change.

### Round 2 wins (2026-06-03, measured on Apple M-series / Asahi)

Baseline on this machine: C-LEARN `run_to_end` 151 ms (1000 Euler steps),
WORLD3-03 1.3 ms. Note the machine difference from the round-1 numbers: on
this core the run is throughput-bound (IPC ~4.5, branch-miss rate ~1.0%), not
mispredict-bound like the Ryzen profile above -- but the lever is the same
(less executed work per step).

**Constant folding (`compiler::fold`, run −2%, bytecode −5%).** The flow
program re-evaluated every `literal op literal` subtree per step -- 792
`BinConstConst` sites on C-LEARN, including one per negative literal (unary
minus lowers to `0 - x`). A fold pass in `Var::new` (the chokepoint both the
monolithic and salsa lowering paths funnel through) collapses constant-only
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
SmallVec `PartialEq` in `LoadIterViewTop`/`LoadIterViewAt` (an out-of-line
memcmp per element per site, ~2% of the run) with a branchless ≤4-wide
compare.

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

### R4. `RuntimeView` allocation + `flat_offset` (~20% of post-win run)

`PushVarView`/`PushTempView` rebuild `SmallVec`s (dims, strides, dim_ids) on every
execution; `flat_offset` (10.3%) recomputes row-major offsets per element. For
arrayed models this is now the #2 run cost.

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

### C1. Arena-allocate the transient parse AST

The equation parser builds `Expr0` with `Box` children + `Vec` args — 3.86M+
transient heap allocations, all lowered to `VariableStage0` and dropped.
`bumpalo` is already a dependency. Allocating the AST in a per-parse arena turns
these into pointer bumps. The constraint: the salsa-cached result
(`ParsedVariableResult`) must be owned/`'static`, so the arena can only back the
transient parse→lower step, with the cached value being the owned lowered form.
Much of this benefit is captured more cheaply by mimalloc (B); pursue the arena
only if profiling after B still shows the parser as a hotspot.

- Effort: large (thread an arena through the parser; verify nothing cached
  retains an arena reference). Risk: medium.

### C2. Halve `reconstruct_variable` (6.4% of compile)

`reconstruct_variable` rebuilds a full `datamodel::Variable` (ident/equation/
inflows/outflows/compat clones) and is called ~2× per variable: once in the
per-variable parse, and once in `module_ident_context_for_model` →
`collect_module_idents`. The latter only needs each variable's `(ident, kind,
is-module-call)` — a lighter projection straight from `SourceVariable` would
avoid ~half the full reconstructions (and their clones).

- Effort: medium. Risk: low–medium (changes the `collect_module_idents` input
  type; behavior must stay identical).

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

## Suggested ordering

1. ~~**Build levers A (opt=3 native) + B (mimalloc native)**~~ — DONE. Measured
   −59% compile / −41% run for ~no engine code and near-zero risk
   (`[profile.release] opt-level=3` + `.cargo/config.toml` wasm override;
   `mimalloc` global allocator on the native binaries + libsimlin's opt-in
   feature). WASM stays on `z` and links no mimalloc.
2. ~~**R1 (bounds-check elimination)**~~ — INVESTIGATED, dropped: measured
   sub-noise (~0) ceiling; bounds checks are effectively free at opt-level=3.
3. ~~**R2 (3-address binop fusion)**~~ — DONE. Flow opcodes −23.5%, run −6.8% on
   C-LEARN; a late `fuse_three_address` pass at Vm::new (the `CompiledSimulation`
   stays symbolizable). A full register VM would cut more but is a large rewrite.
4. ~~**R4 (RuntimeView)**~~ — largely DONE via round 2's `dense_linear_start`
   fast paths (`flat_offset` 8.2% -> ~4% of a smaller run); the residual is
   the strict-slice `vector_elm_map` base and `offset_for_iter_index`'s
   decompose path for shape-equal non-linear views (a per-loop access-plan
   cache is the next idea there — and see the round-2 negative result before
   attempting it).
5. **R3 superinstructions** — incremental dispatch wins, low risk.
6. **C2 / C3** — only if incremental-compile latency still bites after A+B.

Larger run-side swings identified during round 2, unprioritized and
unmeasured (file/see issues):

- **Lazy `If`** — `SetCond`/`If` evaluate BOTH branches every step (3,046
  `If` sites on C-LEARN, ~10% of opcodes counting the condition chains).
  Skipping the untaken branch needs forward jumps (codegen + stack-depth
  validation + peephole/fusion jump maps + wasmgen parity): a real design
  effort.
- **Time-invariant hoisting** — constants are re-assigned and
  constant-derived auxes re-computed every step; a "constant phase" computed
  once per `run_to` (re-run after `set_value`) could skip them. Needs a
  measurement of what fraction of the per-step program is time-invariant,
  and care with results presentation (each saved chunk must still carry the
  values).
- **Lookup last-segment memo** (#602) — C-LEARN's year-indexed tables are
  evaluated at slowly-advancing TIME; remembering the last segment per GF
  would skip most binary searches.
