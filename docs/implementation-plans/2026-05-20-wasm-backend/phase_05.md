# WebAssembly Simulation Backend — Phase 5: Arrays — subscripts, iteration, reducers

**Goal:** Lower the core array machinery — the view-stack opcodes, the `BeginIter…NextIterOrJump…EndIter` iteration loop, the `Array{Sum,Max,Min,Mean,Stddev,Size}` reducers, the temp-array region, and dynamic subscripting — to wasm, matching the VM element-for-element including out-of-bounds→NaN and empty-view semantics.

**Architecture:** The VM resolves array access through a runtime `view_stack` of `RuntimeView`s. Because every view's geometry (base offset, dims, strides, offset, sparsity, is_temp) is known at compile time, the wasm emitter maintains a **compile-time view-descriptor stack** instead: `Push*View`/`ViewSubscript*`/`ViewRange*`/`ViewWildcard`/`ViewTranspose`/`PopView`/`DupView` push/transform/pop descriptors; `BeginIter…EndIter` becomes a wasm bounded loop with a loop-index local and compile-time stride arithmetic (or a precomputed flat-offset table for non-contiguous views); reducers loop over the top descriptor's elements; dynamic subscripts (`ViewSubscriptDynamic`/`ViewRangeDynamic`, legacy `PushSubscriptIndex`/`LoadSubscript`) carry a runtime offset + validity flag so OOB yields NaN exactly as the VM does. **Apply-to-all (A2A) variables are unrolled to scalar bytecode by the compiler — they need no array opcodes — so this phase targets array-producing builtins, reducer arguments, and explicit subscripting.**

**Tech Stack:** `wasm-encoder` (loops/blocks, data segments for precomputed offset tables); `StaticArrayView`/`RuntimeView`/`DimensionInfo`/`SubdimensionRelation` (`bytecode.rs`); the VM array dispatch arms + `reduce_view` + `flat_offset` + `match_dimensions_two_pass` as spec.

**Scope:** Phase 5 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.1 Success:** A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` at those tests' existing tolerances.
- **wasm-backend.AC1.2 Success:** Arrayed/subscripted models (apply-to-all, subscripts, vector operations) match the VM element-for-element. *(Phase 5 covers A2A/subscript/reducer; vector ops complete it in Phase 6.)*
- **wasm-backend.AC1.5 Edge:** Empty-view reducers, out-of-bounds subscripts, and division-by-zero produce the same NaN / finite-`:NA:` / Inf values the VM produces. *(Phase 5 covers the empty-view-reducer (NaN-vs-0.0, and invalid-view→NaN for all reducers) and out-of-bounds-subscript portions.)*

### wasm-backend.AC7
- **wasm-backend.AC7.3 Edge:** `Mod` matches `rem_euclid`; `Max`/`Min` use `f64.max`/`f64.min` with compare-fallback. *(Reaffirmed for the array reducers `ArrayMax`/`ArrayMin`, whose empty-view→NaN semantics differ from the binary builtins.)*

---

## Notes for the implementer (read first)

- **CRITICAL — the design's "opcode" names are `Expr` IR, not bytecode.** `Subscript`, `StaticSubscript`, `TempArray`, `TempArrayElement`, `AssignTemp` are `compiler::Expr` nodes (`compiler/expr.rs:62-88`) that codegen lowers to the view/iter opcodes below — they NEVER appear in `ByteCode.code`. Lower the actual opcodes.
- **A2A is unrolled at compile time** (`compiler/mod.rs:1912-1990`): `c[D] = a[D]*b[D]` compiles to one independent scalar `LoadVar…AssignCurr(off+i)` per element — no array opcodes. So most arrayed models already pass via Phases 1-2. The array opcodes appear for: array-producing builtins (`AssignTemp` → `BeginIter` loop), reducer arguments that are elementwise array expressions, and reducers (`PushStaticView → Array<Reduce> → PopView`).
- **The actual array opcodes** (`bytecode.rs`), with operands and stack effects (`bytecode.rs:1220-1365`):
  - View construction (stack `(0,0)` unless noted): `PushVarView { base_off: u16, dim_list_id: u16 }` (full var array; dims from `ctx.dim_lists[dim_list_id]` → `(n_dims,[DimId;4])`, sizes from `ctx.dimensions[DimId].size`); `PushTempView { temp_id: u8, dim_list_id: u16 }` (is_temp); `PushStaticView { view_id: u16 }` (**the workhorse**: `ctx.static_views[view_id]` baked at compile time); `PushVarViewDirect { base_off, dim_list_id }` (raw sizes, dynamic subscript).
  - View transform (mutate top descriptor): `ViewSubscriptConst { dim_idx: u8, index: u16 }` (drop a dim, 0-based); `ViewSubscriptDynamic { dim_idx }` (stack `(1,0)`: pop 1-based index, **OOB → view invalid**); `ViewRange { dim_idx, start, end }` ([start:end)); `ViewRangeDynamic { dim_idx }` (stack `(2,0)`: pop end then start, clamp); `ViewStarRange { dim_idx, subdim_relation_id }` (sparse via `ctx.subdim_relations[id]`); `ViewWildcard { dim_idx }` (**no-op**); `ViewTranspose {}` (reverse dims/strides/dim_ids); `PopView {}`; `DupView {}`.
  - Temp element: `LoadTempConst { temp_id, index }` (stack `(0,1)`: push `temp_storage[temp_offsets[temp_id]+index]`); `LoadTempDynamic { temp_id }` (stack `(1,1)`: pop index).
  - Iteration: `BeginIter { write_temp_id: u8, has_write_temp: bool }` (captures `view_stack.last()` as the iter view); `LoadIterElement {}` (`(0,1)`, element at `current` from the captured view); `LoadIterTempElement { temp_id }`; `LoadIterViewTop {}` (`(0,1)`, from `view_stack.last()` at `current`, broadcasting); `LoadIterViewAt { offset: u8 }` (`(0,1)`, from `view_stack[len-offset]`, broadcasting; **this is what `StaticSubscript`/`TempArray` lower to inside a loop**, codegen.rs:523-571); `StoreIterElement {}` (`(1,0)`, write to `temp_storage[temp_offsets[write_temp_id]+current]`); `NextIterOrJump { jump_back: i16 }` (`current+=1`; if `<size`, `pc+=jump_back`); `EndIter {}`.
  - Reducers (operate on top view, **do not pop it**, stack `(0,1)`): `ArraySum` (empty→**0.0**); `ArrayMax`/`ArrayMin`/`ArrayMean`/`ArrayStddev` (empty→**NaN**; Stddev = population variance, divisor N, then sqrt); `ArraySize` (push `view.size()`).
  - Legacy dynamic scalar subscript: `PushSubscriptIndex { bounds: u16 }` (`(1,0)`: pop 1-based index, append `(index-1,bounds)`; OOB → invalid); `LoadSubscript { off: u16 }` (`(0,1)`: fold accumulated indices to a flat offset, push `curr[module_off+off+flat]`; invalid → NaN). VM arms `vm.rs:1341-1366`.
  - Broadcast iteration (also Phase 5): `BeginBroadcastIter { n_sources, dest_temp_id }`, `LoadBroadcastElement { source_idx }`, `StoreBroadcastElement {}`, `NextBroadcastOrJump { jump_back }`, `EndBroadcastIter {}`.
- **`StaticArrayView`** (`bytecode.rs:1522-1541`): `{ base_off: u32, is_temp: bool, dims: SmallVec<[u16;4]>, strides: SmallVec<[i32;4]>, offset: u32, sparse: SmallVec<[RuntimeSparseMapping;2]>, dim_ids: SmallVec<[DimId;4]> }`. **Dense element address** for indices `[i_0..i_{n-1}]`: `base_address + offset + Σ i_k*strides[k]`, where `base_address` = `curr[base_off..]` if `!is_temp` else `temp_storage[temp_offsets[base_off]..]`. `size() = Π dims`. Sparse: a sparse dim's real index is `parent_offsets[idx]` (precomputable at compile time). See `RuntimeView::flat_offset` (`bytecode.rs:283-323`), `offset_for_iter_index` (`bytecode.rs:433-456`).
- **`BeginIter` precompute** (`vm.rs:1876-1912`): if the iter view is `sparse.is_empty() && is_contiguous()`, per-iteration offset is `view.offset + current`; else the VM precomputes a `flat_offsets` table by walking multi-dim indices. The wasm emitter does the same at compile time: contiguous → `base+offset+i`; non-contiguous/sparse → bake a precomputed offset table (data segment) and read `offsets[i]`, **or** fully unroll for small arrays.
- **`reduce_view`** (`vm.rs:2802-2840`): `if !view.is_valid { return NaN }`; else fold over `size()` elements (via `flat_offset` + the is_temp dual addressing). **Asymmetry to match exactly:** an *invalid* view (OOB subscript) → NaN for **all** reducers including `ArraySum`; an *empty-but-valid* view → 0.0 for `ArraySum`, NaN for Max/Min/Mean/Stddev, `0` size for `ArraySize`. OOB-subscript→NaN is pinned by `array_tests.rs:1298-1340, 2449-2575`.
- **`temp_storage`**: a flat region of `temp_total_size` f64 (`vm.rs:584-586`); element `index` of temp `temp_id` lives at `temp_storage[temp_offsets[temp_id] + index]`. `temp_offsets`/`temp_total_size` are `ByteCodeContext` fields (compile-time).
- **Broadcasting** in `LoadIterViewTop`/`LoadIterViewAt` (`vm.rs:1946-2182`) uses `match_dimensions_two_pass` (`dimensions.rs:729`) when the source view's dims/dim_ids differ from the iter view's; a smaller source or invalid view → NaN. Mirror this exactly.
- **Memory layout addition:** the `temp_storage` region (`temp_total_size*8` bytes) + any precomputed iter-offset tables (data segments). Append after the Phase-4 regions; grow `pages`.
- `pub(crate)`/`pub` latitude per the repo owner. TDD, inline `#[cfg(test)] mod tests`; `cargo test -p simlin-engine --features file_io wasmgen`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
<!-- START_TASK_1 -->
### Task 1: Compile-time view-descriptor stack + static view opcodes + temp region

**Verifies:** wasm-backend.AC1.2 (prerequisite).

**Files:**
- Create: `src/simlin-engine/src/wasmgen/views.rs` (the compile-time `ViewDesc` model + address-computation helpers) — or add to `lower.rs`.
- Modify: `wasmgen/module.rs` (temp region in the layout), `wasmgen/lower.rs` (view-stack opcode arms).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
1. Add the `temp_storage` region to the memory layout (base + `temp_total_size*8`); thread its base into `EmitCtx`.
2. Define a compile-time `ViewDesc` mirroring the static parts of `RuntimeView`: `{ base_off, is_temp, dims, strides, offset, sparse, dim_ids, runtime_off_local: Option<u32>, valid_local: Option<u32> }`. The last two are wasm locals introduced only by dynamic subscripts (Task 4); static views leave them `None`. Maintain a `Vec<ViewDesc>` in the emitter as the compile-time view stack.
3. Lower the static view opcodes: `PushStaticView{view_id}` (clone `ctx.static_views[view_id]` into a `ViewDesc`), `PushVarView`/`PushTempView`/`PushVarViewDirect` (build from `dim_list_id`/`base_off`), `ViewSubscriptConst`/`ViewRange`/`ViewStarRange`/`ViewWildcard`(no-op)/`ViewTranspose` (static transforms of the top `ViewDesc` mirroring `RuntimeView::apply_*`), `PopView`/`DupView`. Provide a `view_element_addr(desc, flat_index)` emitter that produces the byte address for a flat element index (contiguous fast path `base+offset+i`; strided/sparse via precomputed table or arithmetic).
4. Lower `LoadTempConst{temp_id,index}` (push `f64.load[temp_offsets[temp_id]*8 + index*8]`) and `LoadTempDynamic{temp_id}` (pop index → compute address → load).

**Testing:**
- Unit-test the `ViewDesc` transforms by compiling tiny models whose bytecode contains each view op (a reducer over a subscripted/transposed/sparse view) and asserting the emitted reads hit the addresses the VM's `flat_offset` computes (compare a reducer's result to the VM). Test `LoadTempConst`/`LoadTempDynamic` reads.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen compile-time view-descriptor stack + static view ops`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Array reducers

**Verifies:** wasm-backend.AC1.2, wasm-backend.AC7.3, wasm-backend.AC1.5 (empty-view reducers: `ArraySum`→0.0, Max/Min/Mean/Stddev→NaN; invalid view→NaN for all).

**Files:**
- Modify: `wasmgen/lower.rs` (reducer arms).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Lower `ArraySum`/`ArrayMax`/`ArrayMin`/`ArrayMean`/`ArrayStddev`/`ArraySize` over the top `ViewDesc` (do not pop it). Emit a bounded loop (or unrolled sum for small static sizes) over the view's `size()` elements, reading each via `view_element_addr`. Match `reduce_view` (`vm.rs:2802-2840`) and the per-reducer arms (`vm.rs:2216-2309`) exactly:
- Invalid view (the `valid_local`, when present, is 0) → push NaN for **all** reducers.
- `ArraySum`: fold with init `0.0` (empty valid view → 0.0).
- `ArrayMax`/`ArrayMin`: if `size()==0` → NaN, else fold with `NEG_INFINITY`/`INFINITY` and the VM's compare form (use compare-and-select to match the VM's `if a>b`/`if a<b`, not `f64.max`/`f64.min`, since these are the *reduce* path; AC7.3 fallback applies if a NaN difference surfaces).
- `ArrayMean`: `size()==0` → NaN, else `sum/size`.
- `ArrayStddev`: `size()==0` → NaN, else two-pass population variance (divisor `size`), then `sqrt`.
- `ArraySize`: push `size() as f64` (always defined; 0 for empty).

**Testing:**
- Reducer parity tests vs the VM: non-empty arrays for each reducer; an empty-but-valid view (`ArraySum`→0, others→NaN, `ArraySize`→0); an invalid (OOB-subscripted) view (all→NaN). Stddev population-variance value check.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen array reducers (Sum/Max/Min/Mean/Stddev/Size)`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-4) -->
<!-- START_TASK_3 -->
### Task 3: Iteration loops (BeginIter…EndIter) + broadcast

**Verifies:** wasm-backend.AC1.2.

**Files:**
- Modify: `wasmgen/lower.rs` (iteration arms).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Lower the iteration opcodes to a wasm bounded loop. On `BeginIter{write_temp_id,has_write_temp}`: capture the top `ViewDesc` as the iter view, compute `size()` at compile time, and open a wasm `block`/`loop` with an i32 iteration-index local (`current`) initialized to 0; record the iter context (the captured view, the write temp, the loop label depth) on an emitter-side iter stack. Within the body:
- `LoadIterElement` → read the captured iter view at `current` (contiguous: `base+offset+current`; else precomputed offsets[current]).
- `LoadIterTempElement{temp_id}` → `temp_storage[temp_offsets[temp_id]+current]`.
- `LoadIterViewTop`/`LoadIterViewAt{offset}` → read `view_stack[len-1]` / `view_stack[len-offset]` at `current`, reproducing the VM's dim-matching/broadcast (`match_dimensions_two_pass`, `dimensions.rs:729`) and the "smaller source / invalid view → NaN" rules (`vm.rs:1946-2182`). When the source view's dims/dim_ids equal the iter view's, it's the simple `offset_for_iter_index(current)` read.
- `StoreIterElement` → pop value, store to `temp_storage[temp_offsets[write_temp_id]+current]`.
On `NextIterOrJump{jump_back}`: `current+=1`; `br_if loop` when `current<size`. `EndIter`: close the loop/block, pop the iter context.
Also lower the `BeginBroadcastIter`/`LoadBroadcastElement`/`StoreBroadcastElement`/`NextBroadcastOrJump`/`EndBroadcastIter` family the same way, mirroring their VM arms.

(Note: `jump_back` is a bytecode PC delta; the wasm structured loop does not need it — the emitter detects the loop body span between `BeginIter` and `NextIterOrJump` and emits a structured `loop`. Confirm the codegen always emits well-nested `BeginIter…NextIterOrJump…EndIter` so structured lowering is valid; the example at codegen.rs:1183-1378 shows the canonical shape.)

**Testing:**
- `SUM(a[*]*b[*])`-style models (elementwise product hoisted into an `AssignTemp` `BeginIter` loop then reduced): assert wasm matches the VM element-for-element. A broadcast case (source dims ≠ iter dims). A case where the source is smaller than the iter view (→NaN elements).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen BeginIter/broadcast iteration loops`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Dynamic subscripts + OOB→NaN

**Verifies:** wasm-backend.AC1.2, wasm-backend.AC1.5 (out-of-bounds subscripts → NaN, matching the VM).

**Files:**
- Modify: `wasmgen/lower.rs` (dynamic-subscript arms; extend `ViewDesc` with runtime offset/validity).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
- `ViewSubscriptDynamic{dim_idx}`: pop the 1-based runtime index; bounds-check against `dims[dim_idx]`; on OOB set the descriptor's `valid_local` (a wasm i32 local) to 0; otherwise fold `(index-1)*strides[dim_idx]` into the descriptor's `runtime_off_local`. Subsequent reads add `runtime_off_local` to the element address and, if `valid_local==0`, yield NaN. `ViewRangeDynamic{dim_idx}`: pop end then start, clamp to `[0,dims)` (empty range → 0-size dim, stays valid) per `apply_range_checked`.
- Legacy `PushSubscriptIndex{bounds}` / `LoadSubscript{off}` (`vm.rs:1341-1366`): maintain an emitter-side accumulator of `(runtime_index, bounds)` + a validity local; `PushSubscriptIndex` pops a 1-based index, range-checks against `bounds` (OOB → invalid), and accumulates; `LoadSubscript` folds the accumulated indices into a flat offset, and pushes `curr[module_off+off+flat]` unless invalid → NaN.

**Testing:**
- Models with a runtime/dynamic subscript `arr[i]` (i from an expression) in-range and out-of-range (→NaN); a dynamic range; assert wasm matches the VM (including the OOB→NaN cases pinned by `array_tests.rs`).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen dynamic subscripts with OOB->NaN`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Raise floor; arrayed corpus parity

**Verifies:** wasm-backend.AC1.1, wasm-backend.AC1.2.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (raise `WASM_SUPPORTED_FLOOR`).

**Implementation:**
Arrayed (A2A/subscript/reducer) corpus models now run through wasm. Re-observe the `Ran` count and raise `WASM_SUPPORTED_FLOOR`. (Models using vector ops/allocation remain `Skipped` until Phase 6; module-bearing models until Phase 7.)

**Testing:** the raised floor gate; note which arrayed models flipped to `Ran`.

**Verification:** `cargo test -p simlin-engine --features file_io --test simulate`

**Commit:** `engine: raise wasm parity floor after array core`
<!-- END_TASK_5 -->

---

## Phase 5 Done When
- Arrayed (A2A/subscript/reducer) corpus models match the VM element-for-element.
- Unit tests cover subscript OOB→NaN, broadcast, each reducer (incl. empty-valid vs invalid-view asymmetry), and the iteration loop.
- The floor gate is raised.
