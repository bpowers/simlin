# MDL Full Compatibility -- Phase 5: Missing Builtins -- VM and Stdlib

**Goal:** Implement all missing Vensim builtin functions needed for the SDEverywhere test suite: QUANTUM, SSHAPE, RAMP FROM TO as VM builtins; NPV as a stdlib model; GET DATA BETWEEN TIMES as compiler/DataProvider integration. Also investigate and fix root causes for delayfixed, sample, and quantum test model failures.

**Architecture:** New pure-math builtins become `BuiltinFn` variants with `BuiltinId` opcodes dispatched in the VM. NPV follows the existing stdlib model pattern (stock-flow composition compiled into `stdlib.gen.rs`). GET DATA BETWEEN TIMES requires DataProvider infrastructure from Phase 4. Several test models (quantum, sample, delayfixed) may already work after Phase 1-4 fixes since xmutil desugars them to existing primitives; these need investigation first.

**Tech Stack:** Rust (simlin-engine crate)

**Scope:** 7 phases from original design (phase 5 of 7)

**Codebase verified:** 2026-02-26

---

## Acceptance Criteria Coverage

This phase implements and tests:

### mdl-full-compat.AC5: Missing builtins
- **mdl-full-compat.AC5.1 Success:** QUANTUM, SSHAPE, RAMP_FROM_TO produce correct values for standard test inputs
- **mdl-full-compat.AC5.2 Success:** DELAY FIXED, SAMPLE IF TRUE, NPV simulate correctly as stdlib model expansions
- **mdl-full-compat.AC5.3 Success:** GET DATA BETWEEN TIMES retrieves correct values from data-backed lookups

Note: The design's "Definition of Done" item 5 lists "SAMPLE IF TRUE / SAMPLE UNTIL". SAMPLE UNTIL is NOT a Vensim builtin -- it is a user-defined `:MACRO:` in the C-LEARN model (defined at lines 47-52 of the C-LEARN .mdl file). It uses INTEG internally and would be handled by macro expansion support in the MDL parser, which is out of scope for this phase. SAMPLE IF TRUE is the actual builtin addressed here.

---

## Reference Files

Builtin infrastructure:
- `/home/bpowers/src/simlin/src/simlin-engine/src/builtins.rs` -- `BuiltinFn` enum (line 57), `is_builtin_fn()` (line 288)
- `/home/bpowers/src/simlin/src/simlin-engine/src/bytecode.rs` -- `BuiltinId` enum (line 483)
- `/home/bpowers/src/simlin/src/simlin-engine/src/vm.rs` -- `apply()` function (line 1972), `Opcode::Apply` dispatch (line 1142)
- `/home/bpowers/src/simlin/src/simlin-engine/src/interpreter.rs` -- interpreter builtin handling
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/codegen.rs` -- `BuiltinFn` to `BuiltinId` mapping (line 862)
- `/home/bpowers/src/simlin/src/simlin-engine/src/compiler/expr.rs` -- `strip_loc()` (line 147)

MDL recognition:
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/builtins.rs` -- `BUILTINS` set (line 248), `classify_symbol()` (line 360)
- `/home/bpowers/src/simlin/src/simlin-engine/src/mdl/xmile_compat.rs` -- inline expansion of QUANTUM (line 287), SAMPLE IF TRUE (line 321), DELAY FIXED (line 433)

Stdlib:
- `/home/bpowers/src/simlin/src/simlin-engine/src/stdlib.gen.rs` -- generated stdlib models (delay1, delay3, init, previous, smth1, smth3, trend)
- `/home/bpowers/src/simlin/src/simlin-engine/src/builtins_visitor.rs` -- stdlib module expansion (line 285+), `stdlib_args()` (line 15)

Adding a new VM builtin requires touching:
1. `builtins.rs`: add `BuiltinFn` variant + update `name()`, `is_builtin_fn()`, `try_map()`, `for_each_expr_ref()`, `walk_builtin_expr()`
2. `bytecode.rs`: add `BuiltinId` variant
3. `vm.rs`: add arm in `apply()`
4. `interpreter.rs`: add arm
5. `compiler/codegen.rs`: add `BuiltinFn` -> `BuiltinId` mapping + argument push setup
6. `compiler/expr.rs`: add arm in `strip_loc()`

---

## Important Investigation Note

The codebase investigation revealed that several test models may ALREADY work after Phase 1-4 changes:

- **quantum**: xmutil desugars `QUANTUM(x, q)` to `(q)*INT((x)/(q))`. `INT` is implemented. The MDL native path inline-expands via xmile_compat.rs line 287.
- **sample**: xmutil desugars `SAMPLE IF TRUE` to `PREVIOUS(SELF, ...)`. `previous` is in stdlib.
- **delayfixed/delayfixed2**: xmutil maps DELAY FIXED to `DELAY(...)` which maps to `delay1` stdlib.

Task 1 investigates which models actually fail after Phase 1-4 work, avoiding unnecessary implementation.

---

<!-- START_TASK_1 -->
### Task 1: Investigate which builtin test models actually fail

**Verifies:** mdl-full-compat.AC5.1, mdl-full-compat.AC5.2

**Files:**
- Read: `src/simlin-engine/tests/simulate.rs` (lines 526-583)
- Read: `test/sdeverywhere/models/quantum/quantum.xmile`
- Read: `test/sdeverywhere/models/sample/sample.xmile`
- Read: `test/sdeverywhere/models/delayfixed/delayfixed.xmile`
- Read: `test/sdeverywhere/models/npv/npv.xmile`

**Implementation:**

Temporarily uncomment the six test models in `TEST_SDEVERYWHERE_MODELS` and run:

```bash
cargo test --features file_io --test simulate -- quantum sample delayfixed npv getdata
```

Capture which models actually fail and their error messages. Categorize:
1. Models that now pass (no work needed)
2. Models that fail with `EmptyEquation` or `UnknownBuiltin` (function missing)
3. Models that fail with other errors (different root cause)

Based on findings, adjust the remaining tasks. If quantum/sample/delayfixed already pass, skip their implementation tasks and focus on what's actually broken.

**Verification:** Diagnostic task -- output determines scope of remaining work.

No commit (investigation only).
<!-- END_TASK_1 -->

<!-- START_SUBCOMPONENT_A (tasks 2-4) -->
## Subcomponent A: VM Builtins -- SSHAPE and RAMP FROM TO

<!-- START_TASK_2 -->
### Task 2: Add SSHAPE VM builtin

**Verifies:** mdl-full-compat.AC5.1

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs` (BuiltinFn enum, is_builtin_fn, name, map/walk methods)
- Modify: `src/simlin-engine/src/bytecode.rs` (BuiltinId enum)
- Modify: `src/simlin-engine/src/vm.rs` (apply function)
- Modify: `src/simlin-engine/src/interpreter.rs`
- Modify: `src/simlin-engine/src/compiler/codegen.rs`
- Modify: `src/simlin-engine/src/compiler/expr.rs`
- Modify: `src/simlin-engine/src/mdl/builtins.rs` (add to BUILTINS set)
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs` (add function name mapping)

**Implementation:**

SSHAPE(x, bottom, top) returns an S-shaped growth curve between bottom and top. The Vensim formula is:

```
SSHAPE(x, bottom, top) = bottom + (top - bottom) / (1 + EXP(-4 * (2*x - 1)))
```

Where x is expected in [0, 1] range. When x=0, result approaches bottom; when x=1, approaches top; x=0.5 gives midpoint.

Add across all six files following the existing pattern for 3-arg builtins (like SafeDiv):

1. `builtins.rs`: `Sshape(Box<Expr>, Box<Expr>, Box<Expr>)` -- args are (x, bottom, top)
2. `bytecode.rs`: `BuiltinId::Sshape`
3. `vm.rs`: `BuiltinId::Sshape => a_bottom + (a_top - a_bottom) / (1.0 + (-4.0 * (2.0 * a_x - 1.0)).exp())`
4. All other files: follow existing 3-arg pattern

If `SSHAPE` is not in the MDL `BUILTINS` set, add `"sshape"` to it. Add the function name to `format_function_name()` in xmile_compat.rs if needed.

**Testing:**

Add unit tests verifying:
- `SSHAPE(0, 0, 100)` approaches 0
- `SSHAPE(0.5, 0, 100)` equals 50
- `SSHAPE(1, 0, 100)` approaches 100

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: add SSHAPE VM builtin`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Add RAMP FROM TO recognition

**Verifies:** mdl-full-compat.AC5.1

**Files:**
- Modify: `src/simlin-engine/src/mdl/builtins.rs`
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs`

**Implementation:**

The existing `Ramp` builtin already supports 3 arguments: `Ramp(slope, start_time, Option<end_time>)`. RAMP FROM TO is functionally equivalent to RAMP with a required end_time.

The fix is in MDL function name recognition:
1. Add `"ramp from to"` to the MDL `BUILTINS` set if not present
2. In `format_function_name()` in xmile_compat.rs, map `"ramp from to"` to `"RAMP"` (the existing builtin name)
3. Ensure the argument order matches: RAMP FROM TO(slope, start, end) maps to RAMP(slope, start, end)

If RAMP FROM TO has different semantics than RAMP (check Vensim docs), implement as a new builtin. Otherwise, it's just a name alias.

**Testing:**

Add a unit test verifying RAMP FROM TO(2, 5, 10) at various time points produces correct output matching RAMP behavior.

**Verification:**
Run: `cargo test -p simlin-engine`
Expected: All tests pass

**Commit:** `engine: recognize RAMP FROM TO as alias for RAMP builtin`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Verify QUANTUM works (already inline-expanded)

**Verifies:** mdl-full-compat.AC5.1

**Files:**
- Read: `src/simlin-engine/src/mdl/xmile_compat.rs:287` (QUANTUM expansion)

**Implementation:**

QUANTUM is already expanded inline in xmile_compat.rs line 287:
```
"quantum" => { return format!("({})*INT(({})/({}))") }
```

This uses the existing `INT` builtin. Verify the expansion is correct:
- `QUANTUM(x, q) = q * INT(x / q)` -- rounds x down to nearest multiple of q

If the quantum test model passes after Phase 1 fixes (Task 1 investigation), no additional work is needed. If it fails, investigate the specific error.

**Testing:**

If quantum test model passes in Task 1 investigation, just verify existing test coverage. If not, add targeted test for the QUANTUM expansion.

**Verification:**
Run: `cargo test --features file_io --test simulate -- quantum`
Expected: quantum test passes

**Commit:** `engine: verify QUANTUM inline expansion works` (or no commit if already passing)
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 5-7) -->
## Subcomponent B: Stdlib Models -- NPV, DELAY FIXED, SAMPLE IF TRUE

<!-- START_TASK_5 -->
### Task 5: Implement NPV as stdlib model or VM builtin

**Verifies:** mdl-full-compat.AC5.2

**Files:**
- Potentially create: `src/simlin-engine/stdlib/npv.stmx`
- Modify: `src/simlin-engine/src/builtins_visitor.rs` (stdlib_args, expansion)
- Modify: `src/simlin-engine/src/builtins.rs` (is_builtin_fn or stdlib registration)
- Modify: `src/simlin-engine/src/mdl/builtins.rs` (already has "npv")

**Implementation:**

NPV(stream, discount_rate, initial_val, factor) computes the net present value of a stream. The Vensim formula is:

```
NPV = INTEG(stream * factor * (1 + discount_rate * TIME STEP) ^ -(TIME - INITIAL TIME) / TIME STEP, initial_val)
```

This is a stateful accumulation that requires a stock -- fits the stdlib model pattern.

**Approach A (stdlib model):** Create an `npv.stmx` file similar to existing stdlib models (delay1, smth1). The model has:
- Input: `stream`, `discount_rate`, `initial_val`, `factor`
- Stock: NPV accumulator
- Flow: discounted stream contribution
- Output: accumulated NPV

Add to `builtins_visitor.rs`:
- Add `"npv"` to `stdlib_args()` with `["stream", "discount_rate", "initial_val", "factor"]`
- Register in stdlib model names

Then regenerate `stdlib.gen.rs` using the gen_stdlib tool:
```bash
cargo run -p simlin-cli -- gen-stdlib
```

**Approach B (inline expansion in xmile_compat.rs):** Like QUANTUM, expand NPV inline. This avoids the stdlib model complexity but produces a complex equation.

Choose the approach based on complexity. If the accumulation pattern fits neatly into a single equation (no stock needed), use approach B. Otherwise use approach A.

The `npv` entry is already in the MDL `BUILTINS` set. Check if `format_function_name()` in xmile_compat.rs maps it to anything.

**Testing:**

Enable the NPV test model and verify simulation matches expected output.

**Verification:**
Run: `cargo test --features file_io --test simulate -- npv`
Expected: NPV test passes

**Commit:** `engine: implement NPV builtin`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Verify DELAY FIXED and SAMPLE IF TRUE

**Verifies:** mdl-full-compat.AC5.2

**Files:**
- Read: `test/sdeverywhere/models/delayfixed/delayfixed.xmile`
- Read: `test/sdeverywhere/models/sample/sample.xmile`

**Implementation:**

Based on the investigation:
- DELAY FIXED is mapped to `DELAY(...)` -> `delay1` stdlib by xmutil
- SAMPLE IF TRUE is mapped to `PREVIOUS(SELF, ...)` -> `previous` stdlib by xmutil

If these models pass in the Task 1 investigation, no work is needed. If they fail:

**DELAY FIXED** -- DELAY FIXED has FIFO pipeline semantics (input enters a queue and exits after exactly `delay_time`), which is fundamentally different from DELAY1's exponential smoothing. The xmutil mapping of DELAY FIXED to `DELAY(...)` (which becomes `delay1` stdlib) is semantically incorrect -- it will produce exponential decay instead of a fixed time offset. The delayfixed test model compares against Vensim's FIFO output, so this mapping will likely produce wrong results even if the model compiles. If both SDEverywhere paths (XMILE and MDL) map to the same wrong DELAY1, the test may appear to pass against itself but not match the `.dat` reference data.

Check both paths: (1) does the `.dat` comparison pass? If yes, the test model may not exercise the semantic difference enough for Euler integration to diverge. (2) If it fails, implement DELAY FIXED as a proper stdlib model with pipeline/conveyor semantics. The design notes: "the internal array must be sized to `ceil(delay_time / time_step)` elements, which depends on simulation specs known at compile time."

**SAMPLE IF TRUE** -- If PREVIOUS(SELF, ...) doesn't produce correct output, the issue may be in the `self` reference handling or the `previous` stdlib model's semantics. Investigate the specific test output differences.

**Testing:**

Enable test models and compare output.

**Verification:**
Run: `cargo test --features file_io --test simulate -- delayfixed sample`
Expected: Tests pass

**Commit:** Fix commits as needed based on investigation findings
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_TASK_7 -->
### Task 7: Implement GET DATA BETWEEN TIMES

**Verifies:** mdl-full-compat.AC5.3

**Files:**
- Modify: `src/simlin-engine/src/builtins.rs` (add GetDataBetweenTimes builtin)
- Modify: `src/simlin-engine/src/mdl/builtins.rs` (recognition)
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs` (function mapping)
- Potentially modify: `src/simlin-engine/src/compiler/expr.rs`

**Implementation:**

GET DATA BETWEEN TIMES(variable, time, mode) retrieves the value of a data-backed variable at a specific time point. The `mode` parameter controls interpolation:
- mode = 0: interpolate
- mode = -1: raw (no interpolation, use previous value)
- mode = 1: forward (use next value)

After Phase 4 DataProvider work, data variables become lookup-backed variables. GET DATA BETWEEN TIMES is then equivalent to a lookup operation on the data variable's table at the specified time.

The implementation can be:
1. A lookup builtin variant: `LookupAtTime(data_var, time_expr, mode)` that performs the lookup with the specified interpolation mode
2. Or, during conversion, transform `GET DATA BETWEEN TIMES(var, time, mode)` into a lookup expression on `var` at `time`

The existing `Lookup`, `LookupForward`, `LookupBackward` builtins already support different interpolation modes. Map GET DATA BETWEEN TIMES to the appropriate lookup variant based on the mode parameter.

**Testing:**

Enable the getdata test model and verify simulation matches expected output.

**Verification:**
Run: `cargo test --features file_io,ext_data --test simulate -- getdata`
Expected: getdata test passes

**Commit:** `engine: implement GET DATA BETWEEN TIMES via lookup translation`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Enable all builtin test models and final verification

**Verifies:** mdl-full-compat.AC5.1, mdl-full-compat.AC5.2, mdl-full-compat.AC5.3

**Files:**
- Modify: `src/simlin-engine/tests/simulate.rs`

**Implementation:**

Uncomment all builtin test models from `TEST_SDEVERYWHERE_MODELS`:
- `quantum/quantum.xmile`
- `npv/npv.xmile`
- `sample/sample.xmile`
- `delayfixed/delayfixed.xmile`
- `delayfixed2/delayfixed2.xmile`
- `getdata/getdata.xmile`

Run the full test suite:

```bash
cargo test -p simlin-engine
cargo test --features file_io,ext_data --test simulate
```

Expected: All tests pass including the newly enabled builtin models.

**Commit:** `engine: enable all builtin test models`
<!-- END_TASK_8 -->
