# WebAssembly Simulation Backend — Phase 1: Bytecode-to-wasm scalar core + parity harness

**Goal:** Restructure the `wasmgen` proof-of-concept so it consumes the salsa-compiled bytecode (`CompiledSimulation`) instead of the monolithic `compiler::Module`/`Expr` IR, lower the scalar-core opcode set + the Euler integration loop to a self-contained wasm module, and stand up the dual VM-vs-wasm parity gate in `tests/simulate.rs`.

**Architecture:** The bytecode VM (`src/simlin-engine/src/vm.rs`) is a stack machine over a flat f64 "slab" in linear memory; wasm is also a stack machine over linear memory, so each `Opcode` lowers to a short, mostly 1:1 wasm instruction sequence operating on the wasm operand stack. The backend walks the un-fused opcode programs of each `CompiledModule` (`compiled_initials`/`compiled_flows`/`compiled_stocks`) and emits three wasm functions, then a `run` function that seeds the reserved globals + initials and drives the Euler loop, writing step-major snapshots into a results region. The module exports `memory`, `run`, and three i32 geometry globals (`n_slots`, `n_chunks`, `results_offset`); a `WasmLayout` (variable-name→slot-offset map) is returned alongside the bytes for host-side by-name reads.

**Tech Stack:** Rust; `wasm-encoder` 0.244 (module emission); the DLR-FT `wasm-interpreter` (`wasm::validate`) + `checked::Store` (host run) as the in-test execution oracle; the existing `compile_project_incremental` salsa pipeline; the `tests/simulate.rs` corpus harness.

**Scope:** Phase 1 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-backend.AC1: The wasm backend reproduces the VM's simulation results
- **wasm-backend.AC1.1 Success:** A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` against the model's expected outputs at those tests' existing tolerances. (No separate, tighter wasm-vs-VM threshold.) *(Phase 1 covers scalar, Euler models; later phases widen the supported set.)*
- **wasm-backend.AC1.4 Failure:** A model using a not-yet-supported construct returns `WasmGenError::Unsupported` — a clean error, never a panic or a silently wrong result.
- **wasm-backend.AC1.5 Edge:** Empty-view reducers, out-of-bounds subscripts, and division-by-zero produce the same NaN / finite-`:NA:` / Inf values the VM produces. *(Phase 1 covers the division-by-zero portion — raw `Op2::Div`; the empty-reducer/OOB and finite-`:NA:`-vs-NaN portions complete in Phases 5 and 2.)*

### wasm-backend.AC2: The backend consumes the salsa compiled bytecode
- **wasm-backend.AC2.1 Success:** The wasm module is produced from `compile_project_incremental(...) -> CompiledSimulation`, not from the `Expr` IR or the monolithic `compiler::Module`.
- **wasm-backend.AC2.2 Success:** The POC's `#[cfg(test)]` un-gating of the monolithic builder is reverted; the crate builds with `Module::new`/`build_metadata`/`calc_n_slots`/`calc_module_model_map` test-only again.

### wasm-backend.AC3: simulate.rs runs the corpus through both backends
- **wasm-backend.AC3.1 Success:** During rollout, each corpus model runs through the VM and (when supported) the wasm backend, comparing wasm-vs-VM; unsupported models are skipped (not failed) and counted against a monotonically rising floor.

### wasm-backend.AC4: Self-describing results + efficient by-name retrieval
- **wasm-backend.AC4.1 Success:** The blob exports `n_slots`/`n_chunks`/`results_offset` and writes step-major snapshots; a host locates and strides the results with no external metadata.

### wasm-backend.AC7: Numeric-parity specifics
- **wasm-backend.AC7.4 Success:** Euler, RK2, and RK4 each match the VM's saved samples (cadence and values); `PREVIOUS`/`INIT` match via the snapshot regions. *(Phase 1 establishes the Euler cadence/values portion only; RK2/RK4 + PREVIOUS/INIT complete this AC in Phase 4.)*

### wasm-backend.AC8: Engineering quality (cross-cutting)
- **wasm-backend.AC8.1 / AC8.2** are satisfied cross-cuttingly across every phase rather than headered per-phase: each functionality task is TDD with inline `#[cfg(test)]` unit tests that execute emitted wasm under the DLR-FT interpreter, each opcode/feature group is individually tested toward ≥95% coverage, and every phase ends with passing tests for the ACs it claims (its "Done When").

---

## Notes for the implementer (read first)

- **The VM is the executable spec.** Every opcode's wasm lowering must reproduce the matching arm of `vm.rs`. Cite-and-mirror, do not invent. Key references confirmed during planning:
  - The `Opcode` enum: `src/simlin-engine/src/bytecode.rs:561`. The scalar-core variants are `Op2 { op: Op2 }`, `Not {}`, `LoadConstant { id: LiteralId }`, `LoadVar { off: VariableOffset }`, `LoadGlobalVar { off: VariableOffset }`, `SetCond {}`, `If {}`, `AssignCurr { off }`, `AssignNext { off }`, `Ret`. (`LiteralId`/`VariableOffset` are `u16`.)
  - `Op2` enum: `bytecode.rs:526` — `Add, Sub, Exp, Mul, Div, Mod, Gt, Gte, Lt, Lte, Eq, And, Or`. **There is no `Neq`** (the AST `Neq` lowers to `Eq` then `Not`). The VM's `eval_op2` is `vm.rs:94-111`.
  - `is_truthy(n) = !crate::float::approx_eq(n, 0.0)` — `vm.rs:89`.
  - The Euler loop and the `save_advance!` macro — `vm.rs:631-711` (Euler arm `vm.rs:698-711`; `save_advance!` `vm.rs:675-695`).
  - Reserved global slots `TIME_OFF=0`, `DT_OFF=1`, `INITIAL_TIME_OFF=2`, `FINAL_TIME_OFF=3`, `IMPLICIT_VAR_COUNT=4` — `vm.rs:83-87`.
- **`CompiledSimulation` shape** (`vm.rs:132-140`), all fields `pub(crate)` (the in-crate `wasmgen` module reads them directly):
  - `modules: HashMap<ModuleKey, CompiledModule>`, `specs: Specs`, `root: ModuleKey`, `offsets: HashMap<Ident<Canonical>, usize>` (the global var-name→slot map — this becomes `WasmLayout.var_offsets`), plus a private `cached_constant_info` (ignore until Phase 7).
  - `ModuleKey = (Ident<Canonical>, BTreeSet<Ident<Canonical>>)` (`vm.rs:24`).
  - `CompiledModule` (`bytecode.rs:4616`): `ident`, `n_slots: usize`, `context: Arc<ByteCodeContext>`, `compiled_initials: Arc<Vec<CompiledInitial>>`, `compiled_flows: Arc<ByteCode>`, `compiled_stocks: Arc<ByteCode>`.
  - `ByteCode { literals: Vec<f64>, code: Vec<Opcode> }` (`bytecode.rs:1702`). **`literals` live inside each `ByteCode`**, not on `CompiledModule`. `CompiledInitial { ident, offsets: Vec<usize>, bytecode: ByteCode }` (`bytecode.rs:4603`) — initials are a **vector of per-variable programs**, each its own `ByteCode`.
  - `Specs` (`results.rs:22`): `start`, `stop`, `dt`, `save_step`, `method: Method`, `n_chunks: usize`. `Method` is `Euler | RungeKutta2 | RungeKutta4`.
- **The opcode programs are un-fused.** `fuse_three_address` runs inside `Vm::new` (`vm.rs:397`), *after* `CompiledSimulation` is produced, on the VM's private execution copy. A `CompiledSimulation` consumer only ever sees the plain opcode set above — never `BinVarVar`, `AssignConstCurr`, etc. The emitter does not need to handle the fused/superinstruction opcodes; if one is ever encountered, return `WasmGenError::Unsupported`.
- **DLR-FT oracle pattern** (used by every wasm-executing test), confirmed verbatim at `wasmgen/module.rs:392-422`:
  ```rust
  use checked::Store;
  use wasm::validate;
  let info = validate(&wasm_bytes).expect("module must validate");
  let mut store = Store::new(());
  let inst = store.module_instantiate(&info, Vec::new(), None).expect("instantiate").module_addr;
  let run = store.instance_export(inst, "run").unwrap().as_func().unwrap();
  store.invoke_simple_typed::<(), ()>(run, ()).expect("run wasm");
  let mem = store.instance_export(inst, "memory").unwrap().as_mem().unwrap();
  let data: Vec<f64> = store.mem_access_mut_slice(mem, |bytes| { /* read f64 LE at byte offsets */ });
  ```
- **Visibility latitude (per the repo owner):** widen any engine item to `pub(crate)` — or `pub` where the `tests/` parity harness (a crate-external target) needs it — wherever it produces a cleaner backend. The repo has no external API consumers; breaking changes are fine if tests pass. Do not contort the design to avoid touching visibility. (`compile_project_incremental`, `db::sync_from_datamodel_incremental`, `SimlinDb`, `Results`, and the new `compile_simulation`/`WasmArtifact`/`WasmLayout` must be reachable from `tests/`; make them `pub`.)
- **TDD, 95%+ coverage, inline `#[cfg(test)] mod tests`.** Each unit test that executes wasm builds a tiny module, runs it under the DLR-FT interpreter, and asserts on memory/return values. Keep each test < 2s (the suite runs under a 3-minute wall-clock cap; `docs/dev/rust.md:13-17`). Run the engine tests with `cargo test -p simlin-engine --features file_io` (the corpus tests are gated on `file_io`; bare `cargo test`/`cargo test --workspace` also activate it via workspace feature unification).
- **Addressing scheme (uniform across all phases, module-ready).** The per-program wasm functions take a single `i32` parameter `module_off` (slot base of this module instance within a chunk; `0` for the root in Phase 1). A module-relative slot `off` resolves to byte address `chunk_base + (module_off + off) * 8`, emitted as: push the dynamic part `local.get module_off; i32.const 8; i32.mul`, then `f64.load`/`f64.store` with `memarg.offset = chunk_base + off*8` (a compile-time constant) and `memarg.align = 3`. An **absolute global** slot (`LoadGlobalVar`, slots 0..4) ignores `module_off`: `i32.const 0; f64.load memarg{offset: chunk_base + off*8}`. Using `module_off` from Phase 1 (always 0 for the root) avoids a Phase 7 rewrite. `chunk_base` is `curr_base` for `LoadVar`/`LoadGlobalVar`/`AssignCurr`, `next_base` for `AssignNext`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->

<!-- START_TASK_1 -->
### Task 1: Scalar-core opcode emitter (`wasmgen/lower.rs`)

**Verifies:** wasm-backend.AC2.1 (consumes bytecode opcodes, not `Expr`); wasm-backend.AC1.4 (unsupported opcodes return a clean `WasmGenError::Unsupported`); wasm-backend.AC1.5 (raw `Op2::Div` by zero).

**Files:**
- Create: `src/simlin-engine/src/wasmgen/lower.rs`
- Modify: `src/simlin-engine/src/wasmgen/mod.rs` (add `mod lower;`)
- Test: inline `#[cfg(test)] mod tests` in `wasmgen/lower.rs`

**Implementation:**
Create the per-opcode emitter that walks a `&crate::bytecode::ByteCode` and appends wasm instructions to a `wasm_encoder::Function`, mirroring `eval_bytecode` (`vm.rs:1257+`). Reuse the POC's `EmitCtx`/`memarg`/`f64_const` helpers (currently in `wasmgen/expr.rs`) but generalize `EmitCtx` to carry `module_off` handling per the addressing scheme above.

Define:
```rust
pub(crate) struct EmitCtx {
    pub curr_base: u32,   // byte offset of slot 0 of the curr chunk
    pub next_base: u32,   // byte offset of slot 0 of the next chunk
    pub dt: f64,
    pub start_time: f64,
    pub final_time: f64,
    pub module_off_local: u32, // wasm local index holding this instance's module_off (i32)
}
```

`pub(crate) fn emit_bytecode(bc: &ByteCode, ctx: &EmitCtx, f: &mut Function) -> Result<(), WasmGenError>`:
walk `bc.code` in order; for each `Opcode` emit wasm. A scratch f64 local (reserved by the caller; pass its index in `EmitCtx` or as an arg) is needed for `AssignCurr`/`AssignNext` (the value is already on the wasm stack and the store address must be pushed under it).

Per-opcode lowering (Phase 1 supported set; everything else → `WasmGenError::Unsupported(format!(...))`):

| Opcode | wasm emitted |
|---|---|
| `LoadConstant { id }` | `f64.const bc.literals[id as usize]` |
| `LoadVar { off }` | address(`curr_base`, `off`, dynamic `module_off`); `f64.load` |
| `LoadGlobalVar { off }` | `i32.const 0; f64.load memarg{curr_base + off*8}` (absolute, no `module_off`) |
| `Op2 { op }` | operands already on stack. `Add/Sub/Mul/Div` → `f64.add/sub/mul/div`. `Gt/Gte/Lt/Lte` → `f64.gt/ge/lt/le` then convert the i32 0/1 to f64 (`f64.convert_i32_u`) so booleans stay f64 1.0/0.0 like the VM. `Eq/And/Or/Mod/Exp` → `Unsupported` (Phase 2). |
| `Not {}` | operand on stack; truthiness-negate. Phase 1 uses simple `value == 0.0` (`f64.const 0.0; f64.eq; f64.convert_i32_u`), matching the POC; Phase 2 routes through the `approx_eq` helper. |
| `SetCond {}` | pop the f64 condition; reduce to i32 truthiness (Phase 1: `f64.const 0.0; f64.ne` → i32) and `local.set` into a reserved i32 "condition" local. |
| `If {}` | the two arm values (`t` then `f`) are already on the wasm stack from preceding opcodes; emit `local.get <cond_local>; select`. wasm `select` pops `[t, f, cond_i32]` and yields `t` if `cond != 0` else `f` — exactly the VM's `If` (`push(if condition { t } else { f })`). |
| `AssignCurr { off }` | pop value into the scratch f64 local; emit address(`curr_base`, `off`, `module_off`); `local.get scratch`; `f64.store`. |
| `AssignNext { off }` | same as `AssignCurr` but `next_base`. |
| `Ret` | emit nothing (the wasm function's `End` is emitted by the caller). |

**Critical correctness notes** (all confirmed against the VM):
- `SetCond` is a *separate opcode* that sets a condition register read by `If`; they are always emitted adjacently by codegen but the emitter must reserve a dedicated i32 local for the condition. Nesting: an inner `If` can occur between an outer `SetCond` and its `If`, so use a **stack of condition locals** (push on `SetCond`, pop on `If`) rather than a single local, to be safe — confirm against `compiler/codegen.rs:1153-1159` that emission is well-nested; if codegen guarantees `SetCond` immediately precedes its `If` with no intervening `SetCond`, a single local suffices. Default to the local-stack to be robust.
- `Op2` operand order: the VM pops `r` then `l` and computes `l op r`; wasm leaves them in push order `[l, r]` on the stack, so `f64.sub`/`f64.div` (non-commutative) are already correct.
- Comparisons must yield f64 `1.0`/`0.0` (not raw i32), because downstream opcodes consume them as f64.

**Testing:**
Hand-build small `ByteCode` values (`ByteCode { literals, code }` — fields are `pub(crate)`, reachable in-crate) wrapping each opcode/sequence, wrap in a one-function test module that exports `eval`/`mem` (mirror the harness in the current `wasmgen/expr.rs:300-396`), execute under the DLR-FT interpreter, and assert. Cover:
- wasm-backend.AC2.1: each scalar-core opcode (`LoadConstant`, `LoadVar`, `LoadGlobalVar`, every supported `Op2`, `Not`, `SetCond`+`If` true/false, `AssignCurr`, `AssignNext`) lowers and produces the value/store the VM's `eval_op2`/handler produces.
- `If` selecting the correct arm for truthy and zero conditions; nested `If`.
- wasm-backend.AC1.5: raw `Op2::Div` by zero matches the VM (`x/0` → ±Inf, `0/0` → NaN — IEEE-identical, since wasm `f64.div` matches Rust `f64`).
- wasm-backend.AC1.4: unsupported opcodes (`Op2::Eq`, `Op2::Mod`, `Apply`, `Lookup`, an array opcode) return `WasmGenError::Unsupported` (a clean error, never a panic).

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen::lower`
Expected: all new tests pass.

**Commit:** `engine: wasmgen scalar-core opcode emitter over bytecode`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `compile_simulation` — whole-model assembly (root, Euler)

**Verifies:** wasm-backend.AC2.1, wasm-backend.AC4.1, wasm-backend.AC7.4 (Euler portion).

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs` (add the new `compile_simulation` path + `WasmArtifact`/`WasmLayout`; the old `compile_module(&Module, &Specs)` is removed in Task 3)
- Modify: `src/simlin-engine/src/wasmgen/mod.rs` (export the new types/fn)
- Test: inline `#[cfg(test)] mod tests` in `wasmgen/module.rs`

**Implementation:**
Add the public contract types and entry point. Place the types in `mod.rs` (or `module.rs` and re-export); make them `pub`:
```rust
pub struct WasmArtifact {
    pub wasm: Vec<u8>,
    pub layout: WasmLayout,
}

pub struct WasmLayout {
    pub n_slots: usize,
    pub n_chunks: usize,
    pub results_offset: usize,             // byte offset of the results region
    pub var_offsets: Vec<(String, usize)>, // canonical variable name -> slot offset
}

pub fn compile_simulation(sim: &CompiledSimulation) -> Result<WasmArtifact, WasmGenError>;
```

`compile_simulation` (Phase 1 supports the root module only, Euler only):
1. Look up the root `CompiledModule` via `sim.modules.get(&sim.root)`. Return `Unsupported` if `sim.specs.method != Method::Euler`. Return `Unsupported` if the root has any nested modules (`root.context.modules` non-empty) — modules land in Phase 7.
2. Compute layout: `n_slots = root.n_slots`, `n_chunks = sim.specs.n_chunks`, `stride = n_slots*8`, `curr_base = 0`, `next_base = stride`, `results_base = 2*stride`, `pages = ceil((results_base + n_chunks*stride)/65536)`. (Mirror the POC's `compile_module`, `module.rs:72-85`.) `save_every = max(1, round(save_step/dt))`.
3. Emit three wasm functions over the shared linear memory, each `(module_off: i32) -> ()`:
   - **initials**: for each `CompiledInitial` in `root.compiled_initials`, `emit_bytecode(&ci.bytecode, ...)` in order.
   - **flows**: `emit_bytecode(&root.compiled_flows, ...)`.
   - **stocks**: `emit_bytecode(&root.compiled_stocks, ...)`.
   Each function reserves the scratch f64 local + condition i32 local(s) the emitter needs.
4. Emit the **`run`** function (`() -> ()`): seed `curr[TIME_OFF]=start`, `curr[DT_OFF]=dt`, `curr[INITIAL_TIME_OFF]=start`, `curr[FINAL_TIME_OFF]=stop`; `call initials(0)`; then the Euler loop mirroring `vm.rs:698-711` + `save_advance!` (`vm.rs:675-695`): each step call `flows(0)` then `stocks(0)`, force-save the t=start sample then every `save_every` steps, write the full `curr` row (all `n_slots`) into `results[saved]`, advance stocks `next→curr` and `time += dt`, stop after `n_chunks` saves or when `time > stop`. The POC's `emit_run` (`module.rs:172-286`) is a correct reference for this control-flow shape — adapt it to call the three opcode-emitted functions instead of inlining `Expr` lowering, and to derive the stock copy-back offsets from the `AssignNext` opcodes in `root.compiled_stocks` (collect their `off`, analogous to the POC's `collect_assign_next_offsets`, `module.rs:139-147`).
5. Assemble the module (Type/Function/Memory/Global/Export/Code sections per the POC's `assemble`, `module.rs:293-338`): export `memory`, `run`, and three immutable i32 globals `n_slots`/`n_chunks`/`results_offset` (= `results_base`). With multiple functions, emit a type section entry for `(i32)->()` and `()->()`, a function section indexing them, and export `run` by its function index.
6. Build `WasmLayout`: `var_offsets = sim.offsets.iter().map(|(k,v)| (k.as_str().to_string(), *v)).collect()`; `n_slots`, `n_chunks`, `results_offset = results_base`.

**Testing:**
- wasm-backend.AC2.1 + AC7.4(Euler): build a `CompiledSimulation` for a small scalar Euler model via `compile_project_incremental` (mirror `wasmgen/module.rs:367-373`) — e.g. the `default_projects/population/model.xmile` already used by the POC test, and 1-2 hand-built scalar models via `TestProject` (`src/simlin-engine/src/test_common.rs`). Run the blob under DLR-FT, read the step-major slab, and assert every shared variable's full series matches `Vm::new(sim).run_to_end().into_results()` (reuse the comparison shape from `module.rs:425-457`). Assert `step_count == n_chunks` and the saved cadence matches.
- wasm-backend.AC4.1: a dedicated test reads the three exported i32 globals from the instantiated module (via the `checked` crate's `instance_export(inst, "n_slots").as_global()` accessor) and verifies they equal the `WasmLayout` values; then uses `results_offset`/`n_slots`/`n_chunks` (read from the module, no external metadata) to stride to one variable's series and confirm it matches the VM.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen::module`
Expected: all new tests pass.

**Commit:** `engine: wasmgen compile_simulation (root, Euler) over CompiledSimulation`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Reroute the datamodel entry point; remove the `Expr`-based path

**Verifies:** wasm-backend.AC2.1.

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs` (replace `compile_datamodel_to_wasm` body; remove `compile_module(&Module, &Specs)` and the `collect_assign_next_offsets(&[Expr])`/`store_curr_const`/`emit_run` helpers that consumed `Expr`/`compiler::Module`)
- Delete: `src/simlin-engine/src/wasmgen/expr.rs`
- Modify: `src/simlin-engine/src/wasmgen/mod.rs` (remove `mod expr;`, update `pub use`)

**Implementation:**
Rewrite `compile_datamodel_to_wasm(datamodel, model_name) -> Result<Vec<u8>, WasmGenError>` to go through the salsa pipeline and the new entry point (this is what makes AC2.1 true end-to-end and removes the only production use of `compiler::Module`):
```rust
pub fn compile_datamodel_to_wasm(datamodel: &crate::datamodel::Project, model_name: &str)
    -> Result<Vec<u8>, WasmGenError>
{
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, datamodel, None);
    let sim = crate::db::compile_project_incremental(&db, sync.project, model_name)
        .map_err(|e| WasmGenError::Unsupported(format!("wasmgen: incremental compile failed: {e:?}")))?;
    Ok(compile_simulation(&sim)?.wasm)
}
```
(The `WasmLayout` is dropped here; Phase 7 changes the FFI to surface it. Keep this function's signature stable so `libsimlin` and the `wasm-backend-poc.mjs` exploratory script keep building.)

Delete `wasmgen/expr.rs` entirely (its `Expr`-tree lowering is replaced by `lower.rs`'s opcode emitter). Move the still-needed shared helpers (`memarg`, `f64_const`) into `lower.rs` if not already there. Replace the old `population_wasm_matches_vm` test so it builds the wasm via `compile_simulation(&compiled)` (the same `compiled` it already produces for the VM golden at `module.rs:369-373`) rather than `compile_module(&module, &specs)`; drop the monolithic `compiler::Module::new` usage from the test.

**Testing:**
- The rerouted `population_wasm_matches_vm` (now compiling via `compile_simulation`) passes.
- Add a test that `compile_datamodel_to_wasm` returns a non-empty blob for the population model and that the blob validates under `wasm::validate`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen`
Expected: all wasmgen tests pass; `wasmgen/expr.rs` no longer exists; no references to `crate::compiler::Module` remain in `wasmgen/`.

**Commit:** `engine: route wasmgen through compile_simulation; drop Expr path`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Revert the monolithic-compiler `#[cfg(test)]` un-gating

**Verifies:** wasm-backend.AC2.2.

**Files:**
- Modify: `src/simlin-engine/src/compiler/mod.rs`

**Implementation:**
The POC removed `#[cfg(test)]` from the monolithic builder so the `Expr`-based wasmgen could use it in production. Now that wasmgen consumes `CompiledSimulation`, re-gate it (restoring `main`'s state). Re-add `#[cfg(test)]` to:
- the four imports the POC un-gated at `compiler/mod.rs:16-29` (`use crate::common::{Error, ErrorCode, ErrorKind};`, `use crate::model::ModelStage1;`, `use crate::project::Project;`, `use crate::vm::IMPLICIT_VAR_COUNT;` — confirm exact set against `git diff main -- src/simlin-engine/src/compiler/mod.rs`),
- `calc_module_model_map` (`mod.rs:2660`, currently `pub(crate) fn`),
- `build_metadata` (`mod.rs:2694`, currently `pub(crate) fn`),
- `calc_n_slots` (`mod.rs:2830`, currently bare-private `fn`),
- the `impl Module { fn new }` block (`mod.rs:2849`, `pub(crate) fn new`).

Use `git diff main -- src/simlin-engine/src/compiler/mod.rs` to see precisely what the POC changed and invert exactly that diff (do **not** touch the separate pre-existing `#[cfg(test)] impl Module` test-helper block at `mod.rs:3046`, nor the non-test `impl Module { pub fn compile() }` at `mod.rs:2839`).

**Testing:**
This is a visibility/gating revert verified operationally (no new behavior; **Verifies: AC2.2** via build state). The existing `#[cfg(test)]` users of `Module::new` (and the test suite) continue to compile.

**Verification:**
Run: `cargo build -p simlin-engine` — builds with the four items test-only again (a non-test build no longer references them).
Run: `cargo test -p simlin-engine --features file_io` — compiles and passes (test code still reaches the now-`#[cfg(test)]` builder).
Run: `git diff main -- src/simlin-engine/src/compiler/mod.rs` — shows only the re-gating (the POC's un-gating is fully inverted).

**Commit:** `engine: re-gate monolithic compiler builder to test-only`
<!-- END_SUBCOMPONENT_A -->
<!-- END_TASK_4 -->

<!-- START_SUBCOMPONENT_B (tasks 5-6) -->

<!-- START_TASK_5 -->
### Task 5: `ensure_wasm_matches` parity helper

**Verifies:** wasm-backend.AC1.1, wasm-backend.AC3.1.

**Files:**
- Modify: `src/simlin-engine/tests/test_helpers.rs` (add the helper + a `WasmRunOutcome` type; add the `checked`/`wasm` imports)
- (If `compile_simulation`/`WasmArtifact`/`WasmLayout`/`sync_from_datamodel_incremental`/`compile_project_incremental`/`SimlinDb` are not `pub`, widen them to `pub` so this `tests/` target can call them.)

**Implementation:**
Add a helper that compiles a model to wasm, runs it under the DLR-FT interpreter, builds a `Results` from the step-major slab, and compares it to the model's expected outputs with the **existing** comparator (`ensure_results_excluding`, `test_helpers.rs:62`) — the same check the VM passes. There is no separate wasm-vs-VM threshold (per the design's validation bar); "wasm-vs-VM parity" is achieved because both clear the same comparator against the same expected outputs.

```rust
pub enum WasmRunOutcome { Ran, Skipped(String) }   // Skipped carries the Unsupported message

pub fn ensure_wasm_matches(
    datamodel: &simlin_engine::datamodel::Project,
    model_name: &str,
    expected: &simlin_engine::Results,
    excluded: &[&str],
) -> WasmRunOutcome
```
Steps:
1. Build `CompiledSimulation` exactly as the VM corpus path does (`simulate.rs:105-111` `compile_vm`): `SimlinDb::default()` → `sync_from_datamodel_incremental` → `compile_project_incremental(&db, sync.project, model_name)`. (If the incremental compile itself errors, that is a VM-side issue already covered elsewhere — return `Skipped` with the message rather than failing here.)
2. `let artifact = match simlin_engine::wasmgen::compile_simulation(&sim) { Ok(a) => a, Err(WasmGenError::Unsupported(m)) => return WasmRunOutcome::Skipped(m) };`
3. Instantiate `artifact.wasm` under `checked::Store`, invoke `run`, and read the results region. Read geometry from `artifact.layout` (`n_slots`, `n_chunks`, `results_offset`) — copy `n_chunks * n_slots` f64 from `results_offset`.
4. Build a `simlin_engine::Results`: `offsets` from `artifact.layout.var_offsets` (map each `String` back to `Ident<Canonical>` via the canonicalizing constructor), `data` = the slab (boxed), `step_size = n_slots`, `step_count = n_chunks`, `specs = sim.specs.clone()`, `is_vensim = false`.
5. `ensure_results_excluding(expected, &wasm_results, excluded);` (panics on mismatch — a supported model producing wrong wasm fails loudly). Return `WasmRunOutcome::Ran`.

**Testing:**
This helper is exercised by Task 6's corpus wiring and by a focused unit test here: call `ensure_wasm_matches` on one tiny scalar model (build its `expected` from the VM) and assert it returns `Ran`; call it on a model using an unsupported construct (e.g. a builtin/`Apply`) and assert it returns `Skipped`. (AC1.1: a supported model clears `ensure_results`; AC3.1: an unsupported model is skipped, not failed.)

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate ensure_wasm_matches`
Expected: helper unit tests pass.

**Commit:** `engine: add ensure_wasm_matches parity helper`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Wire the corpus through both backends + the rising floor gate

**Verifies:** wasm-backend.AC1.1, wasm-backend.AC3.1, wasm-backend.AC4.1.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs`

**Implementation:**
1. **Inline hook:** in `simulate_path_with_excluding` (`simulate.rs:843-915`), after the existing VM `ensure_results_excluding` comparisons pass, call `ensure_wasm_matches(&datamodel, "main", &expected, excluded)` once per model. A `Ran` outcome means the wasm output already cleared `ensure_results` inside the helper (a supported-but-wrong model panics there); a `Skipped` outcome is recorded, not failed. Do the same in the `.mdl` path (`simulate_mdl_path*`). Do **not** add the hook to `run_clearn_vs_vdf`/`simulates_clearn` or other `#[ignore]` heavy-model paths — those get `#[ignore]`d wasm twins in Phase 8 (the DLR-FT interpreter is slow; keep the default suite under the 3-minute cap).
2. **Floor gate:** add `const WASM_SUPPORTED_FLOOR: usize = <observed>;` and a `#[test] fn wasm_parity_floor()` that iterates the small/medium corpus list (`TEST_MODELS`, `simulate.rs:22-101`, skipping any entry that is itself `#[ignore]`-class/heavy), runs each through `ensure_wasm_matches` (building `expected` from the VM via the existing parse+`compile_vm`+run path), counts `Ran`, and asserts `ran >= WASM_SUPPORTED_FLOOR`. Set `WASM_SUPPORTED_FLOOR` to the count Phase 1 actually achieves (run the test once, observe, pin it). Document with a comment that each subsequent phase raises this floor and that dropping below it is a regression (AC3.1 / AC3.3). Keep the gate's total runtime within budget — if iterating all of `TEST_MODELS` under the interpreter is too slow, restrict the gate to a representative scalar subset and note it; the per-model inline hook still covers the rest functionally.

**Testing:**
The gate test *is* the test. Also confirm (manually, noted in the commit) that at least one scalar model reports `Ran` and that introducing a deliberate `Unsupported` (temporarily) lowers the count — i.e. the floor would catch a regression.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate wasm_parity_floor`
Expected: passes with `ran >= WASM_SUPPORTED_FLOOR`.
Run: `cargo test -p simlin-engine --features file_io --test simulate`
Expected: the full corpus passes (VM unchanged; supported models also clear wasm; unsupported models skip).

**Commit:** `engine: run corpus through wasm backend with rising floor gate`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

---

## Phase 1 Done When
- Scalar Euler corpus models match the VM through wasm (clearing the existing `ensure_results` comparator); unsupported models skip cleanly via `WasmGenError::Unsupported`.
- The floor gate (`wasm_parity_floor`) is active and pinned.
- The monolithic builder is re-gated to `#[cfg(test)]`; `cargo build -p simlin-engine` and `cargo test -p simlin-engine --features file_io` both pass.
- The blob is self-describing (exports `n_slots`/`n_chunks`/`results_offset`, step-major results) and a test reads geometry from the module to stride results (AC4.1).
