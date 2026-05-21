# WebAssembly Simulation Backend — Phase 6: Arrays — vector operations and allocation

**Goal:** Lower the helper-heavy array builtins — `VectorSelect`, `VectorElmMap`, `VectorSortOrder`, `Rank`, `LookupArray`, and the `AllocateAvailable`/`AllocateByPriority` market-clearing allocators — to wasm helpers that match the VM (and its sibling modules `vm_vector_elm_map.rs`/`vm_vector_sort_order.rs`/`alloc.rs`) element-for-element.

**Architecture:** Each opcode reads its inputs from the compile-time view stack (Phase 5) and the operand stack and writes its result array to its `write_temp_id` region of `temp_storage` (except `VectorSelect`, which reduces to one scalar). Each is emitted as a self-contained wasm helper mirroring the VM. Sorting (`VectorSortOrder`/`Rank`) uses a stable comparison sort (NaN-as-Equal to preserve stability). Allocation reuses Phase 2's `exp` helper for the open-coded `erfc`/`normal_cdf` and runs the VM's bisection over the per-requester allocation curves.

**Tech Stack:** Phase 5 view/temp infrastructure; Phase 3 `lookup_*` helpers (for `LookupArray`); Phase 2 `approx_eq`/`is_truthy`/`exp`; the VM dispatch arms + sibling modules + `alloc.rs` as spec.

**Scope:** Phase 6 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.2 Success:** Arrayed/subscripted models (apply-to-all, subscripts, vector operations) match the VM element-for-element.

### wasm-backend.AC7
- **wasm-backend.AC7.1 Success:** … the allocation `erfc` [is] open-coded as [a] self-contained wasm helper function (range reduction + polynomial). Each open-coded helper has a unit test comparing its output to Rust `f64` over a sampled range.

---

## Notes for the implementer (read first)

- **Opcodes** (`bytecode.rs`), inputs from the view stack (top = last) and operand stack; outputs to `temp_storage[temp_offsets[write_temp_id]+i]`:
  - `VectorSelect {}` (`vm.rs:2444-2502`): pop `action` (`.round() as i32`), pop `max_value`; views `expr_view=top`, `sel_view=top-1`. `size = min(sel.size, expr.size)`, independent index odometers. For each i: if `is_truthy(sel_val)` collect `expr_val`. Empty selection → `max_value`. Else by `action`: `1`=min, `2`=mean(sum/len), `3`=max, `4`=product, `_`=sum. Push the single scalar. (Invalid view → push one NaN.)
  - `VectorElmMap { write_temp_id, full_source_len }` (`vm_vector_elm_map.rs:33-116`): `source_view=top-1`, `offset_view=top`. For each i in `offset_view.size()`: `base_i` = 0 if source is the full contiguous array else projected from carried axes; `flat_i = base_i + round(offset_val)`; result = `NaN` if `offset_val.is_nan()` or `flat_i<0 || flat_i>=full_source_len`, else `source[flat_i]` over full row-major storage. **No modulo.** Write `temp[i]`.
  - `VectorSortOrder { write_temp_id }` (`vm_vector_sort_order.rs:49-101`): `input_view=top`; pop `direction` (`.round() as i32`). Innermost dim is the sorted axis (`inner = dims[n_dims-1]`, or whole view if scalar). Per row of `inner` elements: build `(value, local_idx 0..inner)`, **stable** sort (asc if `direction==1` else desc), write `temp[row_base + rank] = local_idx as f64` (**0-based in-row source index** at the sorted position).
  - `Rank { write_temp_id }` (`vm.rs:2540-2584`): `input_view=top`; pop `direction`. Over the **whole view** collect `(value, orig_idx 0..size)` (orig_idx = sequential iteration index), **stable** sort, write `temp[orig_idx] = (rank_0based + 1) as f64` (**1-based**, indexed by original position).
  - `LookupArray { base_gf, table_count, mode, write_temp_id }` (`vm.rs:2586-2629`): pop `index`; `input_view=top`. For each i in `view.size()`: `elem_off = view.flat_offset(indices)`; if `elem_off >= table_count` → NaN, else dispatch `mode` on `graphical_functions[base_gf+elem_off]` at `index` (reuse Phase 3 `lookup_interp/forward/backward`); write `temp[i]` (sequential index). 
  - `AllocateAvailable { write_temp_id }` (`vm.rs:2631-2721`): pop `avail`; `profile_view=top`, `requests_view=top-1`. Collect `requests` (n), `pp_values`; `pp_cols = if !pp_values.is_empty() && n>0 && pp_size%n==0 { pp_size/n } else { 4 }`; build per-requester `profiles[(ptype,ppriority,pwidth,pextra)]` reading `pp_values[i*pp_cols + {0,1,2,3}]` with defaults `(0.0, 0.0, 1.0, 0.0)` when out of range; `allocate_available(&requests,&profiles,avail)` → write temp.
  - `AllocateByPriority { write_temp_id }` (`vm.rs:2723-2794`): pop `supply` then `width`; `priority_view=top`, `requests_view=top-1`. Build rectangular `profiles[(1.0, priorities[i] or 0.0, width, 0.0)]`; `allocate_available(&requests,&profiles,supply)` → write temp.
- **Invalid input view → `fill_temp_nan`** (`vm.rs:2866-2881`): fill the whole destination temp region with NaN (VectorSelect instead pushes one NaN). The NaN here is IEEE NaN, never `crate::float::NA`.
- **`alloc.rs` (verbatim, port bit-faithfully):**
  - `erfc_approx(z)` (`alloc.rs:8-21`): for `z<0` return `2.0 - erfc_approx(-z)`; else `t=1/(1+0.3275911*z)`; `(((((1.061405429*t + -1.453152027)*t) + 1.421413741)*t + -0.284496736)*t + 0.254829592) * t * (-z*z).exp()`. (Abramowitz-Stegun 26.2.17; uses Phase 2 `exp`.)
  - `normal_cdf(x)` (`alloc.rs:25-30`): `if x.is_nan() {NaN} else 0.5 * erfc_approx(-x / SQRT_2)`.
  - `alloc_curve(p, request, ptype, ppriority, pwidth, pextra)` (`alloc.rs:40-129`): `if request<=0 {0.0}`; `fraction` by `ptype % 10`: 0 fixed (`p<=ppriority?1:0`), 1 rectangular, 2 triangular, 3 normal (`normal_cdf((ppriority-p)/pwidth)`), 4 exponential, 5 CES, `_` fixed (exact formulas in the investigator report / `alloc.rs:48-126`). Then `alloc = request*fraction; if ptype>=10 { alloc.floor() } else alloc`.
  - `allocate_available(requests, profiles, avail)` (`alloc.rs:136-199`): `n=len`; if 0 → empty. `total_demand = Σ requests where r>0`; if `avail>=total_demand` → `requests.map(|r| r.max(0))`; if `avail<=0` → zeros. Else compute search range `[p_min,p_max]` from profiles (per-type `spread`), then **bisection up to 100 iterations**: `mid=(lo+hi)/2; total=Σ alloc_curve(mid, ...); if total<avail {hi=mid} else {lo=mid}; break when |hi-lo| < 1e-14*(1+|hi|)`. Return `alloc_curve(p_star=(lo+hi)/2, ...)` per requester.
- Shared primitives (Phase 5): `increment_indices`, `flat_offset`, `read_view_element`, `temp_offsets`; `is_truthy`/`approx_eq` (Phase 2). **Sorting:** emit a **stable** comparison sort (e.g. insertion sort over `(value, idx)` pairs in a scratch region) treating NaN comparisons as `Equal` to preserve input order (matching the VM's `partial_cmp(..).unwrap_or(Equal)` on a stable `sort_by`).
- **Memory layout additions:** scratch regions for sorting (`(value,idx)` pairs, sized to the largest view) and for allocation (`requests`, `profiles`). Append after Phase-5 regions; grow `pages`.
- `pub(crate)`/`pub` latitude per the repo owner. TDD, inline `#[cfg(test)] mod tests`; `cargo test -p simlin-engine --features file_io wasmgen`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: VectorSelect + VectorElmMap

**Verifies:** wasm-backend.AC1.2.

**Files:** Modify `wasmgen/lower.rs` (+ a `wasmgen/vector.rs` helper module if preferred). Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
- `VectorSelect`: pop `action`/`max_value`; iterate `min(sel.size, expr.size)` with two index odometers; accumulate selected `expr` values where `is_truthy(sel)` (the Phase 2 helper); emit the empty→`max_value` and the action-dispatch (min/mean/max/product/sum) reductions; push one scalar. Invalid view → push NaN.
- `VectorElmMap`: emit the per-element `source[base_i + round(offset[i])]` computation with the `full_source_len` bound (OOB/NaN→NaN, no modulo), reproducing `vm_vector_elm_map.rs` (including the `source_is_full_array` base_i=0 fast path vs the carried-axis projection). Write the result temp; `fill_temp_nan` on invalid input.

**Testing:** parity vs the VM for VectorSelect (each action, empty selection→max_value, NaN-in-mask) and VectorElmMap (in-range, OOB→NaN, NaN offset→NaN, sliced source base_i).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen VectorSelect + VectorElmMap`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: VectorSortOrder + Rank (stable sort)

**Verifies:** wasm-backend.AC1.2.

**Files:** Modify `wasmgen/lower.rs`/`wasmgen/vector.rs`; add a sort scratch region. Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Emit a stable sort helper over `(value, idx)` pairs in a scratch region, NaN-as-Equal. `VectorSortOrder`: per innermost-dim row, sort the row's `(value, local_idx)` pairs (asc/desc by `direction`), write `temp[row_base+rank] = local_idx` (0-based). `Rank`: over the whole view, sort `(value, orig_idx)`, write `temp[orig_idx] = rank+1` (1-based). Match `vm_vector_sort_order.rs` and `vm.rs:2540-2584` exactly, including the `direction` semantics and the indexing (sorted-position vs original-position).

**Testing:** parity vs the VM for ascending/descending; tie stability (equal values keep input order); multi-row VectorSortOrder; whole-view Rank; a NaN element (compares Equal → stable).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen VectorSortOrder + Rank with stable sort`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: LookupArray (per-element arrayed GF)

**Verifies:** wasm-backend.AC1.2.

**Files:** Modify `wasmgen/lower.rs`. Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Lower `LookupArray { base_gf, table_count, mode, write_temp_id }`: pop the shared `index`; for each element i of the input view, compute `elem_off = flat_offset(indices)`; if `elem_off >= table_count` → NaN, else look up the GF directory at `base_gf+elem_off` and `call` the Phase 3 `lookup_interp/forward/backward` (per `mode`) at `index`; write `temp[i]` (sequential index). `fill_temp_nan` on invalid view.

**Testing:** parity vs the VM for an arrayed graphical function across its domain, including an out-of-range element_offset element (→NaN) and all three modes.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen LookupArray (per-element arrayed GF)`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4) -->
<!-- START_TASK_4 -->
### Task 4: Allocation — erfc/normal_cdf/alloc_curve/allocate_available + the two opcodes

**Verifies:** wasm-backend.AC1.2, wasm-backend.AC7.1.

**Files:** Create `src/simlin-engine/src/wasmgen/alloc.rs` (the allocation helper emitters); modify `wasmgen/lower.rs` (the two opcode arms); add allocation scratch regions. Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Emit wasm helpers mirroring `alloc.rs` verbatim:
- `erfc_approx(z)` (using the Phase 2 `exp` helper) and `normal_cdf(x)` with the exact constants/Horner order above.
- `alloc_curve(p, request, ptype, ppriority, pwidth, pextra)` with all six `ptype % 10` branches and the `ptype >= 10` floor flag.
- `allocate_available(requests_ptr, n, profiles_ptr, avail, out_ptr)` (operating over scratch memory arrays): the `total_demand` short-circuits, the search-range computation, the 100-iteration bisection with the `1e-14*(1+|hi|)` relative convergence break, and the final per-requester `alloc_curve(p_star, ...)`.
Lower `AllocateAvailable`/`AllocateByPriority`: collect `requests`/`profiles` from the views into scratch arrays (with the `pp_cols`/default logic for AllocateAvailable, the rectangular-profile synthesis for AllocateByPriority), pop the scalars, call `allocate_available`, write results to the `write_temp_id` region. `fill_temp_nan` on invalid input views.

**Testing:**
- AC7.1: unit-test the emitted `erfc_approx`/`normal_cdf` against Rust `alloc::erfc_approx`/`normal_cdf` over a sampled range (expose them `pub(crate)` if needed); document the tolerance.
- `alloc_curve` parity for each of the 6 profile types + the `>=10` floor.
- `AllocateAvailable`/`AllocateByPriority` end-to-end parity vs the VM: `avail >= total_demand` (full grant), `avail <= 0` (zeros), and the partial-allocation bisection case across profile types.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasmgen allocation (erfc/normal_cdf/alloc_curve/allocate_available)`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Raise floor; vector-op/allocation corpus parity

**Verifies:** wasm-backend.AC1.2.

**Files:** Modify `src/simlin-engine/tests/simulate.rs` (raise `WASM_SUPPORTED_FLOOR`).

**Implementation:** Corpus models using vector ops/allocation now run through wasm. Re-observe the `Ran` count and raise `WASM_SUPPORTED_FLOOR`. (Module-bearing models remain `Skipped` until Phase 7.)

**Testing:** the raised floor gate.

**Verification:** `cargo test -p simlin-engine --features file_io --test simulate`

**Commit:** `engine: raise wasm parity floor after vector ops + allocation`
<!-- END_TASK_5 -->

---

## Phase 6 Done When
- Corpus models using vector ops/allocation match the VM element-for-element.
- Unit tests cover each op including the allocation bisection and the `erfc`/`normal_cdf` accuracy vs Rust `f64`.
- The floor gate is raised.
