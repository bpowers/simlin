# C-LEARN Residual — Phase 2: Stop import-time linearization of shadowed macros

**Goal:** A user-defined macro that shares a builtin's name (here `RAMP FROM TO`) is resolved by the compile-time macro path instead of being silently replaced by an import-time linear-only rewrite, so the macro's exponential branch runs when the model selects it.

**Architecture:** The MDL→datamodel expression formatter (`xmile_compat.rs`) currently has a `format_call_ctx` arm that rewrites any 4+-arg `RAMP FROM TO(...)` into a hardcoded linear `(xfrom) + RAMP(slope, tstart, tend)` string at import time, discarding the 5th arg (`islinear`) and the entire macro body, *before* compile-time macro resolution runs. We delete that arm so the call survives import as `RAMP_FROM_TO(...)` and resolves through the already-correct "macro-shadows-everything" precedence in `builtins_visitor.rs`. No new builtin, opcode, or codegen primitive is added.

**Tech Stack:** Rust (`simlin-engine` crate). Tests use the in-crate inline-MDL macro harness (`macro_expansion_tests.rs`: `mdl()` + `run_mdl_var()`) and the existing file-fixture path (`tests/simulate.rs::simulate_mdl_path`).

**Scope:** Phase 2 of 5 from `docs/design-plans/2026-05-19-clearn-residual.md`.

**Codebase verified:** 2026-05-20 (branch `clearn-residual`, off `main`@`2ed93950`).

---

## Acceptance Criteria Coverage

This phase implements and tests:

### clearn-residual.AC2: User macros sharing a builtin name resolve via the macro path
- **clearn-residual.AC2.1 Success:** A model defining `:MACRO: RAMP FROM TO(...)` invoked with the exponential selector (`islinear = 0`) produces the exponential trajectory from the macro body, not a linear ramp.
- **clearn-residual.AC2.2 Success (no regression):** The same invocation with the linear selector (`islinear = 1`) produces the linear trajectory (existing `simulates_macro_clearn_ramp_from_to_mdl` behavior preserved).
- **clearn-residual.AC2.3 Success:** After import, `RAMP FROM TO(...)` is a resolvable call (not a pre-linearized `RAMP(...)` string), so compile-time macro resolution applies.
- **clearn-residual.AC2.4 Edge:** Nonpositive endpoints exercise the macro's `linear` selector (forced linear when `xfrom>0 :AND: xto>0` is false) per the macro definition.
- **clearn-residual.AC2.5 Failure/guard:** No `xmile_compat.rs` formatter special-case rewrites a name a model defines as a macro ahead of macro resolution (verified for the audited builtin-named macros, e.g. `SSHAPE`).

---

## Verified ground truth (read before starting)

These were confirmed by investigation on 2026-05-20. Trust them over the design's line numbers.

- **The load-bearing bug** is the `"ramp from to"` arm in `XmileFormatter::format_call_ctx` at `src/simlin-engine/src/mdl/xmile_compat.rs:422-434`. It fires for `args.len() >= 4` (so it catches the C-LEARN 5-arg call), returns the linear string, and never reads `args[4]` (`islinear`). Deleting this arm is the entire fix.
- **The name mapping `"ramp from to" => "RAMP_FROM_TO"` at `xmile_compat.rs:555`** is *redundant* with the catch-all `_ => canonical.to_uppercase().replace(' ', "_")` (same file, ~line 569): both produce `RAMP_FROM_TO`. Keep line 555 (it documents intent and is the active formatting path once the arm is gone); changing it alone changes nothing. Do **not** remove it as part of the fix — its presence is harmless and self-documenting.
- **`RAMP FROM TO` is NOT a native builtin** (no `BuiltinFn::RampFromTo`; confirmed absent from `builtins.rs`/`builtins_visitor.rs`). After the arm is removed, the *only* resolution is the user macro. **No test model uses `RAMP FROM TO` without also defining the macro** (verified by grep over `test/`; every occurrence — the `macro_clearn_ramp_from_to` fixture, C-LEARN itself at line 67, and the inline `macro_expansion_tests.rs` test — co-defines the macro). So removing the arm orphans nothing.
- **The macro-shadows-everything precedence already handles `RAMP FROM TO`**: `builtins_visitor.rs:626-638` resolves an in-model macro named like a builtin before alias/modulo/builtin handling. Proven by the passing `macro_expansion_tests.rs` test `ac5_4_macro_shadows_ramp_from_to_builtin` (the 2-arg case, which today escapes the formatter arm because `args.len() < 4`).
- **The #554 self-call exception is irrelevant here**: `is_enclosing_macro_renamed_builtin_self_call` (`builtins_visitor.rs:254-261`) only fires for `init`/`previous`/stdlib-module names; `is_renamed_builtin_macro_collision("ramp_from_to")` is `false`.
- **SSHAPE is NOT a formatter-rewrite hazard.** There is *no* `sshape` arm in `format_call_ctx`. `sshape` appears only as a name-preserving rename in `format_function_name` (`xmile_compat.rs:556`), and `SSHAPE` is a genuine 3-arg builtin (`BuiltinFn::Sshape`). A same-named macro shadows it cleanly via the normal path (proven by the existing `macro_expansion_tests.rs` test `ac5_4_macro_shadows_sshape_builtin`). The design's "audit SSHAPE" instruction is based on an incorrect assumption; the audit (Task 4) records this.
- **The existing `simulates_macro_clearn_ramp_from_to_mdl` (`tests/simulate.rs:3181`) does NOT discriminate the bug from the fix.** Its fixture uses `is linear = 1` with positive endpoints, so the linear rewrite and the correct macro resolution produce the identical series `[2,2,4,6,8,10,10]`. It passes today *with the macro bypassed* and will still pass after the fix. It is a no-regression guard only — **not** proof the macro path runs. Task 1 adds the discriminating test.
- **The unit test `test_ramp_from_to_transforms_args` (`xmile_compat.rs:2031-2054`) encodes the buggy linear output**: it asserts the formatter output `contains("RAMP")` and `!starts_with("RAMP_FROM_TO")`. Task 2 inverts it.
- **Inline MDL test harness** (in-crate, fast, NOT `file_io`-gated): `src/simlin-engine/src/macro_expansion_tests.rs` provides `fn mdl(body: &str) -> String` (wraps body with `{UTF-8}` header + a `CONTROL_TAIL` control section) and `fn run_mdl_var(source: &str, var: &str) -> Vec<f64>` (compiles via salsa, runs the VM, returns the variable's saved series). Read `CONTROL_TAIL` (top of that file) to learn the `INITIAL TIME`/`FINAL TIME`/`TIME STEP`/`SAVEPER` of the harness so you can align the ramp window and the asserted step. The 2-arg shadow test at `macro_expansion_tests.rs:463` is the closest existing template.

---

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->

<!-- START_TASK_1 -->
### Task 1: Add a discriminating exponential-branch regression test (RED)

**Verifies:** clearn-residual.AC2.1, clearn-residual.AC2.2, clearn-residual.AC2.4

**Files:**
- Test: `src/simlin-engine/src/macro_expansion_tests.rs` (add a new `#[test]` near the existing `ac5_4_macro_shadows_ramp_from_to_builtin` at line 463) — in-crate unit test, no `file_io`.

**Implementation:**
Write a test that defines a 5-parameter `RAMP FROM TO` macro whose body genuinely branches on `islinear`, with an exponential (here: clearly non-linear) branch that the import-time linear rewrite cannot reproduce. Use `mdl()` + `run_mdl_var()`. First read `CONTROL_TAIL` and pick `tstart`/`tend`/the asserted step so the asserted step lies strictly inside the ramp window.

Fixture body (the macro mirrors C-LEARN's `linear` selector so AC2.4 is exercised; the exp branch uses double slope so it is provably distinct from the linear branch mid-ramp):

```
:MACRO: RAMP FROM TO(xfrom, xto, tstart, tend, islinear)
RAMP FROM TO = IF THEN ELSE(linear = 1, linear ramp, exp ramp)
	~	dmnl
	~	|
linear = IF THEN ELSE(xfrom > 0 :AND: xto > 0, islinear, 1)
	~	dmnl
	~	|
slope = (xto - xfrom) / (tend - tstart)
	~	dmnl
	~	|
linear ramp = xfrom + RAMP(slope, tstart, tend)
	~	dmnl
	~	|
exp ramp = xfrom + 2 * RAMP(slope, tstart, tend)
	~	dmnl
	~	|
:END OF MACRO:
```

Calling models (three separate `run_mdl_var` invocations or three vars in one model):
- `y_exp = RAMP FROM TO(10, 110, <tstart>, <tend>, 0)` — `islinear = 0`, positive endpoints. Expected: the **exp** branch (`xfrom + 2*slope*(t-tstart)`), NOT the linear branch.
- `y_lin = RAMP FROM TO(10, 110, <tstart>, <tend>, 1)` — `islinear = 1`, positive endpoints. Expected: the **linear** branch.
- `y_force = RAMP FROM TO(-10, 110, <tstart>, <tend>, 0)` — `islinear = 0` but `xfrom <= 0`, so `linear` is forced to `1`. Expected: the **linear** branch despite `islinear = 0`.

**Testing:** assertions (compute exact values from the chosen window; example with `tstart=1, tend=11, slope=10`, asserted at a saved step where `Time=6` → `RAMP(10,1,11)=50`):
- AC2.1: `y_exp` at the mid step `== 10 + 2*50 == 110` (exp), and is NOT equal to the linear value `10 + 50 == 60` (a margin assertion `>= 50` apart, or exact `== 110`).
- AC2.2: `y_lin` at the mid step `== 60` (linear).
- AC2.4: `y_force` at the mid step `== (-10) + 50 == 40` (linear branch, because `linear` forced to 1), and is NOT the exp value `-10 + 100 == 90`.

Note: with the current (buggy) formatter, all three reduce to the linear rewrite, so `y_exp` would be `60` and `y_force` would be `40` — the `y_exp` assertion FAILS, giving a true RED.

**Verification:**
Run: `cargo test -p simlin-engine --lib <new_test_name>`
Expected: **FAILS** (RED) — `y_exp` is the linear value `60`, not the exp value `110`, because the formatter linearized the call before macro resolution.

**Commit:** `engine: add failing exponential-branch test for shadowed RAMP FROM TO macro`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Invert the formatter unit test to expect macro survival (RED)

**Verifies:** clearn-residual.AC2.3

**Files:**
- Test: `src/simlin-engine/src/mdl/xmile_compat.rs:2031-2054` (rewrite `test_ramp_from_to_transforms_args`).

**Implementation:**
The existing test asserts the formatter linearizes (`result.contains("RAMP")` and `!result.starts_with("RAMP_FROM_TO")`). Invert it so it asserts the call SURVIVES as a resolvable macro call: format the same 4-arg `ramp from to` call and assert the result IS the `RAMP_FROM_TO(...)` form (so compile-time macro resolution can apply) and is NOT a pre-linearized `RAMP(...)` rewrite. Rename the test to reflect the new intent (e.g. `test_ramp_from_to_survives_as_macro_call`). Keep the arg-count and arg-content checks meaningful (the four args appear in order in the emitted call).

**Testing:**
- AC2.3: assert the formatted output starts with `RAMP_FROM_TO(` (or canonicalizes to `ramp_from_to`), contains the four argument expressions, and does NOT contain a top-level `RAMP(` linear rewrite.

**Verification:**
Run: `cargo test -p simlin-engine --lib test_ramp_from_to`
Expected: **FAILS** (RED) — the current formatter still emits the linear `RAMP(...)`, so the inverted assertion fails.

**Commit:** `engine: rewrite RAMP FROM TO formatter test to expect macro survival`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Remove the import-time linearization arm (GREEN)

**Verifies:** clearn-residual.AC2.1, clearn-residual.AC2.2, clearn-residual.AC2.3, clearn-residual.AC2.4

**Files:**
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs:422-434` (delete the entire `"ramp from to" => { ... }` arm from `format_call_ctx`).

**Implementation:**
Delete the `"ramp from to"` arm at `xmile_compat.rs:422-434`. After deletion, a `ramp from to` call falls through to the default call-formatting path, which calls `format_function_name("ramp from to")` → `"RAMP_FROM_TO"` (line 555, or equivalently the catch-all), producing `RAMP_FROM_TO(arg0, arg1, arg2, arg3, arg4)`. At compile time, `builtins_visitor.rs` canonicalizes this to `ramp_from_to`, `resolve_macro` finds the in-model macro, and `expand_module_function` expands it (the macro-shadows-everything path at `builtins_visitor.rs:626-638`). Do NOT touch line 555. Do NOT add any new builtin/opcode.

**Testing:**
- Tasks 1 and 2 tests now pass (GREEN).
- No regression: `simulates_macro_clearn_ramp_from_to_mdl` (`tests/simulate.rs:3181`) still passes (its `islinear=1` fixture yields the same linear series via the macro path).

**Verification:**
Run: `cargo test -p simlin-engine --lib test_ramp_from_to <task1_test_name>`
Expected: both pass (GREEN).
Run: `cargo test -p simlin-engine --features file_io --test simulate simulates_macro_clearn_ramp_from_to_mdl`
Expected: passes (no regression).
Run: `cargo test -p simlin-engine --lib macro`
Expected: all macro-expansion tests pass (e.g. `ac5_4_macro_shadows_ramp_from_to_builtin`, `ac5_4_macro_shadows_sshape_builtin`).

**Commit:** `engine: stop linearizing shadowed RAMP FROM TO macro at import`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Record the formatter special-case audit and verify the shadow guard

**Verifies:** clearn-residual.AC2.5

**Files:**
- Modify: `src/simlin-engine/src/mdl/xmile_compat.rs` (add a doc comment above `format_call_ctx`'s `match` documenting the audit and the shadowing hazard).
- Test (verify, add only if a gap exists): `src/simlin-engine/src/macro_expansion_tests.rs`.

**Implementation:**
Add a concise doc comment above the `match canonical.as_str()` in `format_call_ctx` recording the audit conclusion: arms that *restructure* a builtin-named call into a different expression at import time pre-empt any same-named user `:MACRO:`. The `ramp from to` arm was the one C-LEARN tripped (now removed). Document that other restructuring arms (notably the multi-word `sample if true`, plus `pulse`/`pulse train`/`modulo`/`zidz`/`xidz`/`get data between times`/etc.) carry the *same* latent hazard but are not currently shadowed by any in-repo model; if a future model defines a macro with one of those names, the arm must be guarded the same way. Explicitly note that **`SSHAPE` is name-preserving** (handled only by `format_function_name`, not restructured here), so it shadows correctly via `builtins_visitor.rs:626-638` with no formatter change needed.

Then confirm the guard already exists: `ac5_4_macro_shadows_sshape_builtin` and `ac5_4_macro_shadows_ramp_from_to_builtin` in `macro_expansion_tests.rs` prove a same-named macro shadows the builtin. If (and only if) one is missing, add a minimal equivalent. Do not duplicate existing coverage.

This task changes no runtime behavior; it is documentation + a coverage check (the design's AC2.5 is a guard, not new functionality).

**Testing:**
- AC2.5: the existing `ac5_4_macro_shadows_*` tests pass, demonstrating macro-shadows precedence for both a multi-word name (`RAMP FROM TO`) and a single-word builtin (`SSHAPE`). The audit doc is present.

**Verification:**
Run: `cargo test -p simlin-engine --lib ac5_4_macro_shadows`
Expected: both shadow tests pass.
Run: `cargo build -p simlin-engine` (doc comment compiles).

**Commit:** `engine: document formatter macro-shadowing audit (RAMP FROM TO, SSHAPE)`
<!-- END_TASK_4 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase completion criteria

- Tasks 1-4 committed; the discriminating exponential-branch test passes (was RED before Task 3, GREEN after).
- `cargo test -p simlin-engine` (default, non-ignored) is green, including the inverted formatter test and all macro-expansion tests.
- `simulates_macro_clearn_ramp_from_to_mdl` still passes (no regression).
- The formatter audit is recorded as a doc comment.
- **Ignored C-LEARN gate note:** This phase is expected to make the `im_3_emissions`, `im_3_emissions_vs_rs`, `im_3_ff_co2`, `relative_emissions_to_equity`, and `relative_emissions_to_equity_target` bases reconcile against `Ref.vdf`. Do NOT prune `EXPECTED_VDF_RESIDUAL` here — Phase 4 re-measures after Phases 1-3 and reconciles the gate. Running `cargo test -p simlin-engine --features file_io --release --test simulate -- --ignored clearn_residual_exactness` after this phase will report those bases as `shrank`; that is expected and is closed in Phase 4. Optionally record the observed `shrank` set in the commit body as evidence the fix landed.

## No special-casing (hard constraint)

No change in this phase keys on a C-LEARN variable name, the C-LEARN `.mdl`/`.vdf` path, or the residual list. The fix is the deletion of one general formatter arm; the generality test uses a small inline macro independent of C-LEARN.
