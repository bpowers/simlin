# WebAssembly Simulation Backend — Phase 2: Full scalar builtins + numeric parity

**Goal:** Bring every scalar `BuiltinId` and `Op2` to VM parity: open-code the transcendentals wasm lacks, route equality/truthiness through a wasm `approx_eq` helper that matches `crate::float::approx_eq` exactly, and lower `Mod`/`Exp` and the composed builtins (`Step`/`Pulse`/`Ramp`/`Sshape`/`Sign`/`Quantum`/`SafeDiv`) to faithful f64 sequences.

**Architecture:** Builds on Phase 1's opcode emitter (`wasmgen/lower.rs`). Math wasm provides natively (`f64.abs`/`sqrt`/`floor`/`min`/`max`/arithmetic/compares) maps to the instruction directly; the transcendentals (`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`exp`/`ln`/`log10`/`pow`) are emitted once each as self-contained wasm helper functions (range reduction + polynomial) and called by name — the blob needs no math imports. Equality and truthiness route through a single emitted `approx_eq` helper so the backend takes the same branch the VM takes.

**Tech Stack:** Rust; `wasm-encoder` (multi-function modules, `call`); the DLR-FT interpreter oracle; `crate::float::approx_eq` (`float_cmp` 0.10) as the equality reference; the VM's `apply()` (`vm.rs:2938-3012`) and `eval_op2` (`vm.rs:94-111`) as the builtin/operator spec.

**Scope:** Phase 2 of 8 from `docs/design-plans/2026-05-20-wasm-backend.md`.

**Codebase verified:** 2026-05-21 (branch `wasm-backend-poc`).

---

## Acceptance Criteria Coverage

### wasm-backend.AC1
- **wasm-backend.AC1.1 Success:** A model within the supported feature set runs through the wasm backend and passes the same `simulate.rs` comparison the VM passes — its results clear `ensure_results` / `ensure_vdf_results` against the model's expected outputs at those tests' existing tolerances. (No separate, tighter wasm-vs-VM threshold.)
- **wasm-backend.AC1.5 Edge:** Empty-view reducers, out-of-bounds subscripts, and division-by-zero produce the same NaN / finite-`:NA:` / Inf values the VM produces. *(Phase 2 covers the finite-`:NA:`-sentinel-vs-genuine-NaN distinction via the `approx_eq` helper; the empty-reducer/OOB portions complete in Phase 5.)*

### wasm-backend.AC7
- **wasm-backend.AC7.1 Success:** Math wasm provides natively (`sqrt`, `abs`, `floor`/`ceil`/`trunc`/`nearest`, `min`/`max`, arithmetic) uses wasm instructions; the transcendentals wasm lacks (`sin`/`cos`/`tan`/`asin`/`acos`/`atan`/`exp`/`ln`/`log10`/`pow`) and the allocation `erfc` are open-coded as self-contained wasm helper functions (range reduction + polynomial). Each open-coded helper has a unit test comparing its output to Rust `f64` over a sampled range; results need not be bit-identical to the VM's libm — only close enough that the existing tests pass. *(The allocation `erfc`/`normal_cdf` helpers land in Phase 6; Phase 2 covers the scalar transcendentals.)*
- **wasm-backend.AC7.2 Success:** Equality and truthiness (`Eq`/`Neq`/`And`/`Or`/`If` condition) use ULP-based `approx_eq` matching the VM.
- **wasm-backend.AC7.3 Edge:** `Mod` matches the VM's `rem_euclid` semantics (computed via wasm `floor`). `Max`/`Min` use the wasm `f64.max`/`f64.min` instructions; if a corpus test surfaces a NaN/±0 difference from the VM's compare-based form, fall back to explicit compare-and-select for that case.

---

## Notes for the implementer (read first)

- **Confirmed enums.** `Op2` (`bytecode.rs:527`): `Add, Sub, Exp, Mul, Div, Mod, Gt, Gte, Lt, Lte, Eq, And, Or` (no `Neq`). `BuiltinId` (`bytecode.rs:500`): `Abs, Arccos, Arcsin, Arctan, Cos, Exp, Inf, Int, Ln, Log10, Max, Min, Pi, Pulse, Quantum, Ramp, SafeDiv, Sign, Sin, Sshape, Sqrt, Step, Tan`. **There is no `Mean` and no `IsModuleInput` `BuiltinId`** — scalar `MEAN(a,b,…)` is lowered by codegen to `(0+a+b+…)/N` using `Op2::Add`/`Op2::Div` (already handled by Phase 1/2), single-arg `MEAN(array)` becomes `ArrayMean` (Phase 5), and `IsModuleInput` is resolved to a `LoadConstant 1.0/0.0` at codegen. So the backend never sees a `Mean`/`IsModuleInput` opcode.
- **`Apply` always pops exactly 3 operands** (codegen pads: 1-arg builtins with two `LoadConstant 0.0`; 2-arg with one; `Ramp` pads its end-time with `LoadGlobalVar{FINAL_TIME_OFF}`). So lower `Apply{func}` by popping the 3 stack values into three scratch f64 locals `a`, `b`, `c` (top is `c`), reading `time = curr[TIME_OFF]`/`dt = curr[DT_OFF]` from memory when the builtin needs them, computing per `apply()` (`vm.rs:2938-3012`), and pushing the result.
- **`apply()` exact sequences** (mirror verbatim, `vm.rs:2938-3012`): `Abs=a.abs()`, `Sqrt=a.sqrt()`, `Int=a.floor()` (**floor, not trunc**), `Min={if a<b {a} else {b}}`, `Max={if a>b {a} else {b}}`, `Sign={if a>0 {1} else if a<0 {-1} else {0}}`, `Quantum={if b==0.0 {a} else {(a/b).trunc()*b}}`, `SafeDiv={if b != 0.0 {a/b} else {c}}` (**exact `!= 0.0`, not approx**), `Sshape=b + (c-b)/(1.0 + (-4.0*(2.0*a-1.0)).exp())`, `Exp=a.exp()`, `Ln=a.ln()`, `Log10=a.log10()`, `Sin/Cos/Tan/Arcsin/Arccos/Arctan` = the libm calls, `Inf=f64::INFINITY`, `Pi=PI`, `Step=step(time,dt,a,b)`, `Pulse=pulse(time,dt,a,b,c)`, `Ramp=ramp(time,a,b,Some(c))`. Helper bodies: `step` (`vm.rs:3027`): `if time + dt/2.0 > step_time {height} else {0.0}`; `ramp` (`vm.rs:3014`): `if time > start {if end.is_some() && time>=end {slope*(end-start)} else {slope*(time-start)}} else {0.0}`; `pulse` (`vm.rs:3036`): a `while` loop — emit it as a wasm helper function with a loop.
- **`eval_op2`** (`vm.rs:94-111`): `Exp=l.powf(r)`, `Mod=l.rem_euclid(r)`, `Eq=approx_eq(l,r) as f64`, `And=(is_truthy(l)&&is_truthy(r)) as f64`, `Or=(is_truthy(l)||is_truthy(r)) as f64`. The rest (`Add/Sub/Mul/Div/Gt/Gte/Lt/Lte`) are Phase 1.
- **`approx_eq` is `float_cmp::approx_eq!(f64, a, b)`** with `float-cmp` 0.10.0 defaults `epsilon = f64::EPSILON`, `ulps = 4`. Exact algorithm (must be reproduced bit-faithfully in wasm; confirmed by reading the crate):
  - `a == b` → true (handles ±inf and exact equality), OR
  - `(a-b).abs() <= f64::EPSILON` → true, OR
  - `|ulps_diff(a,b)| <= 4` → true,
  where `ulps_diff(a,b) = ordered(a).wrapping_sub(ordered(b))` as `i64` (then `saturating_abs`), and `ordered(f) = { let bits = f.to_bits() as i64; if (bits as u64) & (1<<63) != 0 { !bits ... } else { bits ^ (1<<63) } }` — i.e. map the sign-magnitude bit pattern to a monotonic ordered integer. Consequence: **`approx_eq(NaN, NaN) == true`** (identical bits → 0 ulps), and the finite `:NA:` sentinel (`crate::float::NA = -2^109`) compares unequal to ordinary values (its exponent is far from theirs). `is_truthy(n) = !approx_eq(n, 0.0)` (`vm.rs:89`).
- **`pub(crate)`/`pub` latitude** (per the repo owner): widen visibility freely. Reuse the Rust `crate::float::approx_eq` in unit tests as the oracle for the wasm helper.
- **TDD, inline `#[cfg(test)] mod tests`, < 2s per test.** Run: `cargo test -p simlin-engine --features file_io wasmgen`.

---

<!-- START_SUBCOMPONENT_A (tasks 1) -->
<!-- START_TASK_1 -->
### Task 1: `approx_eq` wasm helper + equality/truthiness routing

**Verifies:** wasm-backend.AC7.2, wasm-backend.AC1.5 (the finite `:NA:` sentinel vs genuine NaN — `approx_eq` keeps them distinct).

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/lower.rs` (the emitter) and the module-assembly code so the helper function is emitted once and callable.
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
1. Emit one wasm helper function `approx_eq(a: f64, b: f64) -> i32` (returns 1/0) reproducing the algorithm above using `i64.reinterpret_f64`, `i64` arithmetic (`wrapping_sub` is plain `i64.sub`; replicate `saturating_abs` and the `ordered` bit map), `f64.eq`, `f64.sub`/`f64.abs`, and `f64.const f64::EPSILON`. Reserve a function index for it (it joins the module's function table; later phases reuse it). Provide a small `pub(crate)` helper in the emitter that pushes two f64 operands and emits `call approx_eq`.
2. Replace Phase 1's placeholder truthiness everywhere it matters:
   - `Not {}`: `call approx_eq(value, 0.0)` → i32 `is_false`; logical-not (`i32.eqz`) → `is_truthy`; convert to f64 1.0/0.0. (i.e. `Not` pushes `(!is_truthy) as f64` = `is_false as f64`; mirror `vm.rs` `Not` = `(!is_truthy(pop)) as f64`.)
   - `SetCond {}`: `is_truthy(pop) = approx_eq(pop, 0.0) == 0` → store the i32 into the condition local.
   - `Op2::Eq`: `call approx_eq(l, r)` → i32 → `f64.convert_i32_u` (f64 1.0/0.0).
   - `Op2::And`: `is_truthy(l) & is_truthy(r)` → f64; `Op2::Or`: `is_truthy(l) | is_truthy(r)` → f64. (Both operands are on the stack; compute `is_truthy` of each via `approx_eq(·,0.0); i32.eqz`, combine with `i32.and`/`i32.or`, convert to f64.)
   - `If {}` condition: unchanged structurally (reads the condition local set by `SetCond`), but the local now holds the `approx_eq`-based truthiness.
   `Neq` is not an `Op2` (codegen lowers it to `Eq`+`Not`), so routing `Eq` through `approx_eq` automatically makes `Neq` correct.

**Testing:**
- A unit test that emits a tiny module exporting `eq(a,b)->i32` wired to the `approx_eq` helper, runs it under DLR-FT for a curated + randomized sample of f64 pairs, and asserts the wasm result equals `crate::float::approx_eq(a,b)` for every pair. Sample must include: exact equal, far apart, 1–4 ULP apart, `f64::EPSILON`-apart around 1.0, around-zero (subnormals), `(NaN,NaN)`, `(NaN,1.0)`, `(NA, NA)`, `(NA, 0.0)`, `(+0.0,-0.0)`, `(±inf, ±inf)`.
- Tests that `Op2::Eq`, `Op2::And`, `Op2::Or`, `Not`, and `SetCond`+`If` now match the VM's `eval_op2`/`is_truthy` for near-zero / ULP-adjacent operands where raw `==`/`!=0.0` would diverge.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen::lower`
Expected: the `approx_eq`-parity tests pass.

**Commit:** `engine: wasmgen approx_eq helper + equality/truthiness routing`
<!-- END_TASK_1 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 2-4) -->
<!-- START_TASK_2 -->
### Task 2: Open-coded transcendental helpers

**Verifies:** wasm-backend.AC7.1.

**Files:**
- Create: `src/simlin-engine/src/wasmgen/math.rs` (the transcendental helper emitters) — or add to `lower.rs`; prefer a dedicated module for clarity.
- Test: inline `#[cfg(test)] mod tests`.

**Implementation:**
Emit one self-contained wasm helper function per transcendental, each `(f64) -> f64` (or `(f64,f64)->f64` for `pow`), using range reduction + a polynomial/rational approximation. The blob imports no host math. There is no external library to integrate — this is standard numerical method work, validated against Rust `f64`. Recommended kernels (refine only if a corpus model needs more accuracy — the bar is the `simulate.rs` tolerances, abs `2e-3` / rel `5e-6` / VDF 1%):
- `exp(x)`: reduce `x = k·ln2 + r`, `|r| <= ln2/2`; `exp(x) = 2^k · exp(r)` (poly in `r`); assemble `2^k` by composing the exponent bits (`i64`→`f64` via `f64.reinterpret_i64`). Handle overflow→`+inf`, underflow→`0`, `NaN`→`NaN`.
- `ln(x)`: split `x = m · 2^e` with `m ∈ [1,2)` (decompose the f64 bits); `ln(x) = e·ln2 + ln(m)` (poly/`atanh` series in `(m-1)/(m+1)`). `x<0`→`NaN`, `x==0`→`-inf`.
- `sin(x)`/`cos(x)`: reduce modulo `π/2` (Cody–Waite or a simple `k = round(x/(π/2))` with extended-precision subtraction), choose the kernel poly by `k mod 4`.
- `atan(x)`: reduce using `atan(x) = π/2 - atan(1/x)` for `|x|>1` and a small-argument poly; sign symmetry.
- Composed: `tan = sin/cos`; `pow(x,y) = exp(y·ln x)` (matches `powf` for `x>0`; **negative-base integer powers diverge** — note this as a known limitation, refine only if a corpus model uses it); `log10(x) = ln(x)·(1/ln10)`; `asin(x) = atan(x / sqrt(1-x²))` (with domain clamping at `|x|=1`); `acos(x) = π/2 - asin(x)`.

Wire each `BuiltinId` transcendental in the `Apply` lowering (Task 4) to `call` the matching helper. Emit each helper at most once per module (lazily, recording its function index).

**Testing:**
Per AC7.1, **each helper gets a unit test comparing the emitted wasm output to Rust `f64` over a sampled range**: emit a module exporting the helper, run it under DLR-FT for a dense sample across the function's domain (and edge cases: 0, ±large, near asymptotes, the `asin`/`acos` endpoints, negative args for `ln`/`sqrt`/even roots), and assert `|wasm(x) - rust_f64(x)| <= tol` with a tol comfortably inside the `simulate.rs` tolerances (e.g. rel `1e-9`..`1e-6` depending on the function; document the chosen tol per helper and why it suffices). Include NaN/inf propagation assertions.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen::math`
Expected: every transcendental helper's accuracy test passes.

**Commit:** `engine: open-coded wasm transcendental helpers (exp/ln/sin/cos/atan + composed)`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: `Op2::Exp` and `Op2::Mod`

**Verifies:** wasm-backend.AC7.3 (Mod), wasm-backend.AC1.1.

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/lower.rs` (extend the `Op2` arm).

**Implementation:**
- `Op2::Exp`: operands `[l, r]` on stack → `call pow` (the Task 2 helper). Matches `l.powf(r)` for positive base.
- `Op2::Mod`: compute `rem_euclid(l, r)` faithfully (do **not** use a plain truncated remainder). `r0 = l - r * (l / r).trunc()` (the `%` result, via `f64.div`, `f64.trunc`, `f64.mul`, `f64.sub`); then `if r0 < 0.0 { r0 + r.abs() } else { r0 }` (via `f64.lt`, `f64.abs`, `f64.add`, `select`). This reproduces Rust's `f64::rem_euclid` exactly (a result in `[0, |r|)`). (The design's "via floor" phrasing is approximate; the trunc-then-adjust form matches `rem_euclid` for negative divisors too.)

**Testing:**
- `Op2::Exp`: assert wasm matches `l.powf(r)` (via the VM) for a sample of positive bases and assorted exponents (integer, fractional, negative).
- `Op2::Mod`: assert wasm matches `l.rem_euclid(r)` for the four sign combinations of `(l, r)` and non-integer operands; assert the result is always in `[0, |r|)`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen::lower`
Expected: Exp/Mod parity tests pass.

**Commit:** `engine: wasmgen Op2 Exp (pow) and Mod (rem_euclid)`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: `Apply` lowering for the full `BuiltinId` set

**Verifies:** wasm-backend.AC1.1, wasm-backend.AC7.1, wasm-backend.AC7.3 (Min/Max).

**Files:**
- Modify: `src/simlin-engine/src/wasmgen/lower.rs` (add the `Apply { func }` arm).

**Implementation:**
Add the `Apply { func }` arm: pop the 3 operands into scratch f64 locals `a`/`b`/`c` (top is `c`), then emit per `func`, reading `time`/`dt` from `curr[TIME_OFF]`/`curr[DT_OFF]` where needed (mirror `apply()` exactly):
- Native f64 instr: `Abs`→`f64.abs(a)`, `Sqrt`→`f64.sqrt(a)`, `Int`→`f64.floor(a)`, `Max`→`f64.max(a,b)`, `Min`→`f64.min(a,b)`.
  - **AC7.3 Min/Max note:** `f64.min`/`f64.max` differ from the VM's compare form (`if a>b {a} else {b}`) on NaN and ±0. Use the wasm instructions first; if a corpus test surfaces a NaN/±0 divergence, switch *that* op to the compare-and-select form `(a>b)?a:b` / `(a<b)?a:b` matching `apply()`.
- Compare/arith composed: `Sign` (`a>0`→1, `a<0`→-1, else 0 via compares+selects), `Quantum` (`b==0.0`→`a` else `(a/b).trunc()*b` — exact `==`), `SafeDiv` (`b != 0.0`→`a/b` else `c` — exact `!=`), `Sshape` (`b + (c-b)/(1.0 + exp(-4.0*(2.0*a-1.0)))`, calling the `exp` helper).
- Transcendental: `Exp/Ln/Log10/Sin/Cos/Tan/Arcsin/Arccos/Arctan` → `call` the Task 2 helpers on `a`.
- Time-driven helpers: `Step` (`time + dt/2 > b ? a : 0`), `Ramp` (the `ramp(time, a, b, Some(c))` branch logic), `Pulse` (emit/`call` a `pulse(time, dt, volume, first, interval)` wasm helper containing the VM's `while` loop, `vm.rs:3036-3053`).
- Constants: `Inf`→`f64.const INFINITY`, `Pi`→`f64.const PI`. (Codegen usually emits these as `LoadConstant`, but handle the `Apply` form too.)

**Testing:**
- Per-builtin unit tests: emit each `Apply{func}` over hand-built operand sequences, run under DLR-FT, assert equality with the VM's `apply(func, time, dt, a, b, c)` over representative inputs (including the edge values: `Int` of negatives (floor vs trunc), `Quantum` with `b==0`, `SafeDiv` with `b==0` and `b`=subnormal, `Sign(0)`, `Step`/`Ramp` across their breakpoints, `Pulse` across multiple intervals, `Sshape` across `[0,1]`).
- AC7.1: the transcendental `Apply` arms produce values within the documented tolerance of Rust `f64`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io wasmgen::lower`
Expected: all builtin parity tests pass.

**Commit:** `engine: wasmgen Apply lowering for full scalar builtin set`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_5 -->
### Task 5: Raise the floor; scalar-only corpus parity

**Verifies:** wasm-backend.AC1.1.

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs` (raise `WASM_SUPPORTED_FLOOR`).

**Implementation:**
With all scalar builtins/operators supported, more corpus models now run through wasm. Re-run the `wasm_parity_floor` gate, observe the new `Ran` count, and raise `WASM_SUPPORTED_FLOOR` to it. Any model that is purely scalar (no arrays, lookups, modules, RK, PREVIOUS/INIT) should now `Ran` and clear `ensure_results`. Models still using unsupported constructs (graphical functions, arrays, modules, RK2/RK4, PREVIOUS/INIT) remain `Skipped` until their phases land.

**Testing:**
The raised floor gate is the test. Confirm (note in the commit) that scalar models which were `Skipped` in Phase 1 due to `Eq`/builtins now `Ran`.

**Verification:**
Run: `cargo test -p simlin-engine --features file_io --test simulate`
Expected: full corpus passes; `wasm_parity_floor` passes at the raised floor.

**Commit:** `engine: raise wasm parity floor after full scalar builtins`
<!-- END_TASK_5 -->

---

## Phase 2 Done When
- All scalar-only corpus models match the VM through wasm (clearing `ensure_results`).
- Unit tests cover each builtin, each transcendental helper (vs Rust `f64`), and the `approx_eq`/NaN/`:NA:` edge cases.
- `Mod`=`rem_euclid`, `Exp`=`pow`, equality/truthiness via `approx_eq`; `Min`/`Max` via `f64.min`/`f64.max` (compare-fallback noted).
- The floor gate is raised to the new supported count.
