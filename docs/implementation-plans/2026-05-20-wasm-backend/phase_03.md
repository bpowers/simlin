# WebAssembly Simulation Backend — Phase 3: Graphical functions (lookups)

**Goal:** Bring the scalar `Lookup` opcode (Interpolate / Forward / Backward modes) to VM parity by laying the graphical-function tables into the blob's linear memory and emitting a shared lookup helper that mirrors the VM's three lookup functions exactly.

**Architecture:** The `ByteCodeContext.graphical_functions` (a `Vec<Vec<(f64,f64)>>`) is serialized into a read-only region of the module's linear memory via an active wasm data segment, alongside a per-table directory (byte offset + point count). Three wasm helper functions — `lookup_interp`, `lookup_forward`, `lookup_backward` — reproduce `vm.rs`'s `lookup`/`lookup_forward`/`lookup_backward` (`vm.rs:3055-3186`) over a `(data_offset, count, index)` interface. The `Lookup { base_gf, table_count, mode }` opcode lowers to a runtime element-offset bounds check + a directory lookup + a `call` to the mode's helper. The interpolate kernel reuses Phase 2's `approx_eq` helper for the at-knot exact-hit test.

**Tech Stack:** `wasm-encoder` `DataSection` (active data); the Phase 2 `approx_eq` helper; the VM lookup functions as spec.

**Scope:** Phase 3 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.1 Success:** A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` against the model's expected outputs at those tests' existing tolerances. (No separate, tighter wasm-vs-VM threshold.)

### wasm-backend.AC7
- **wasm-backend.AC7.1 Success:** Math wasm provides natively uses wasm instructions; the transcendentals … are open-coded as self-contained wasm helper functions … Each open-coded helper has a unit test comparing its output to Rust `f64` over a sampled range. *(For Phase 3 the relevant helpers are the lookup kernels; tested against the VM's `lookup`/`lookup_forward`/`lookup_backward`.)*

---

## Notes for the implementer (read first)

- **Opcode** (`bytecode.rs:626-638`): `Lookup { base_gf: GraphicalFunctionId, table_count: u16, mode: LookupMode }`. `GraphicalFunctionId = u8` (`bytecode.rs:21`, so ≤256 tables/module). `LookupMode` (`bytecode.rs:45-55`): `Interpolate = 0`, `Forward = 1`, `Backward = 2`. Stack effect `(2,1)`.
- **Stack discipline** (`vm.rs:1710-1731`): the opcode pops `lookup_index` first, then `element_offset` (so the producing opcodes pushed `element_offset` then `lookup_index`). Bounds check: `if element_offset < 0.0 || element_offset >= table_count as f64 { push NaN } else { gf_idx = base_gf + element_offset; dispatch mode }`. For the common scalar case codegen emits `LoadConstant 0.0` for `element_offset` (so it is 0), but **the lowering must handle a runtime element_offset** (arrayed scalar-`Lookup` selects a per-element table).
- **Tables** (`bytecode.rs:1588`): `graphical_functions: Vec<Vec<(f64,f64)>>`; the table used is `graphical_functions[base_gf + element_offset]`, a list of `(x,y)` knots in x-ascending order.
- **The three VM lookup functions are NOT one function — they differ in three ways** (confirmed; this is the key parity risk):
  - `lookup` (Interpolate, `vm.rs:3055-3102`): empty→NaN; NaN index→NaN; `index < x[0]` (**strict**)→`y[0]`; `index > x[n-1]` (**strict**)→`y[n-1]`; lower-bound binary search (`while low<high { mid; if x[mid] < index {low=mid+1} else {high=mid} }`); at `i=low`: **if `approx_eq(x[i], index)`** → `y[i]`, else linear interp `slope=(y[i]-y[i-1])/(x[i]-x[i-1]); (index-x[i-1])*slope + y[i-1]`.
  - `lookup_forward` (`vm.rs:3104-3142`): empty/NaN→NaN; `index <= x[0]` (**inclusive**)→`y[0]`; `index >= x[n-1]`→`y[n-1]`; **same lower-bound** search; return `y[low]`. **No approx_eq, no interpolation.**
  - `lookup_backward` (`vm.rs:3144-3186`): empty/NaN→NaN; `index <= x[0]`→`y[0]`; `index >= x[n-1]`→`y[n-1]`; **upper-bound** search (`if x[mid] <= index {low=mid+1} else {high=mid}`); return `y[low-1]` (last knot with `x <= index`; for duplicate x, the LAST). **No approx_eq, no interpolation.**
- The `context.graphical_functions[gf_idx]` access is a safe bounds-checked index in the VM; the element_offset/table_count check guarantees it's in range.
- **Memory-layout convention (extended each phase).** Phase 1 used `[curr][next][results]`. Phase 3 appends two regions after the results region: a **GF directory** (per global table index: byte offset of its data + point count) and the **GF data** (all tables' `(x,y)` pairs as f64). Compute these region bases in `compile_simulation`, grow `pages` accordingly, and initialize them with an active `DataSection`. `results_offset` (exported) is unchanged. (Phases 4/5 append RK-scratch / temp regions similarly.)
- `pub(crate)`/`pub` latitude per the repo owner. TDD, inline `#[cfg(test)] mod tests`, `cargo test -p simlin-engine --features file_io wasmgen`.

---

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
<!-- START_TASK_1 -->
### Task 1: Emit GF tables + directory into linear memory

**Verifies:** wasm-backend.AC1.1 (prerequisite for lookups).

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/module.rs` (layout + `DataSection` emission), `src/simlin-engine/src/wasmgen/lower.rs` (carry the GF region bases in `EmitCtx`).
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
In `compile_simulation`, after computing the results region, lay out:
- **GF data region:** concatenate every table in `root.context.graphical_functions` in order; each table's knots as consecutive f64 LE pairs `x0,y0,x1,y1,…`. Record each table's byte offset and point count.
- **GF directory region:** an array indexed by global table index `t` (0..`graphical_functions.len()`), each entry `(data_byte_offset: i32, n_points: i32)` — so the runtime can map `base_gf + element_offset` → its table. Store as two i32 per entry (or i32 pairs).
Emit both regions with an active `DataSection` (a data segment whose `ConstExpr` offset is the region base) so they're initialized at instantiation. Grow `pages` to cover them. Thread the directory base + data base into `EmitCtx`.

(Modules in Phase 7 each have their own `ByteCodeContext.graphical_functions`; for Phase 3 only the root's tables exist. Phase 7 generalizes the directory to cover all instances' tables.)

**Testing:**
- A test that builds a model with one graphical function, compiles it, and verifies (by reading the blob's GF data region from memory after instantiation) that the table's `(x,y)` pairs are present at the directory-indicated offset with the right count.

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen::module`

**Commit:** `engine: emit graphical-function tables + directory into wasm memory`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: The three lookup helper functions

**Verifies:** wasm-backend.AC1.1, wasm-backend.AC7.1.

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/lower.rs` (or a `wasmgen/lookup.rs`) — emit `lookup_interp`, `lookup_forward`, `lookup_backward`.
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Emit three wasm helper functions, each `(data_off: i32, count: i32, index: f64) -> f64`, reading `x = f64.load[data_off + 16*k]`, `y = f64.load[data_off + 16*k + 8]` for knot `k`. Reproduce the VM functions exactly:
- `lookup_interp`: the empty/NaN guards, **strict** edge clamps, lower-bound binary search, then at `i=low` `call approx_eq(x[i], index)` (Phase 2 helper) → if true return `y[i]`, else the linear-interp formula.
- `lookup_forward`: NaN/empty guards, **inclusive** edge clamps, lower-bound search, return `y[low]`.
- `lookup_backward`: NaN/empty guards, inclusive edge clamps, **upper-bound** search, return `y[low-1]`.
Implement the binary search with i32 locals (`low`, `high`, `mid`) and `f64.load` of `x[mid]`. (`count == 0` → return NaN; `index` NaN via `f64.ne(index,index)` → NaN.)

**Testing:**
- Emit each helper over hand-placed tables in memory and assert, under DLR-FT, that it matches the VM's `lookup`/`lookup_forward`/`lookup_backward` for: below-range, above-range, exact-knot hits, between-knots, a single-point table, duplicate-x tables (Backward's last-duplicate rule), and a NaN index. Compare directly against calling the VM functions (expose them `pub(crate)` if needed).

**Verification:** `cargo test -p simlin-engine --features file_io wasmgen`

**Commit:** `engine: wasm lookup_interp/forward/backward helpers matching the VM`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: `Lookup` opcode lowering + corpus parity

**Verifies:** wasm-backend.AC1.1.

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/lower.rs` (add the `Lookup` arm).
- Modify: `src/simlin-engine/tests/simulate.rs` (raise `WASM_SUPPORTED_FLOOR`).

**Implementation:**
Add the `Lookup { base_gf, table_count, mode }` arm. Stack has `[element_offset, index]` (top = index). Emit: pop `index` and `element_offset` into f64 locals; bounds-check `element_offset < 0.0 || element_offset >= table_count as f64` → push NaN; else compute `table_idx = base_gf + (element_offset as i32)`, load `(data_off, count)` from the GF directory at `directory_base + table_idx*8`, and `call` the mode-specific helper (`mode` is compile-time, so emit a static `call` to `lookup_interp`/`lookup_forward`/`lookup_backward`). Push the result. Match the VM's `as usize`/`as f64` cast chain for the bounds compare.

Then raise the floor: corpus models using graphical functions now run through wasm. Re-observe and raise `WASM_SUPPORTED_FLOOR`.

**Testing:**
- Unit: a model with a `LOOKUP`/graphical-function variable in Interpolate, Forward, and Backward modes; assert wasm matches the VM across the table's domain (below/above/at-knot/between) and for an out-of-range `element_offset` (→NaN).
- Corpus: at least one `simulate.rs` model that uses a graphical function now `Ran` and clears `ensure_results`.

**Verification:** `cargo test -p simlin-engine --features file_io --test simulate`

**Commit:** `engine: wasmgen Lookup opcode lowering + GF corpus parity`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

---

## Phase 3 Done When
- Corpus models using graphical functions match the VM through wasm.
- Unit tests cover interpolate / forward / backward, edge clamping, exact-knot hits, duplicate-x (Backward), and out-of-range element_offset → NaN.
- The floor gate is raised.
