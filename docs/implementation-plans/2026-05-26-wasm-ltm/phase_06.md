# Loops That Matter on the wasm Backend (wasm-ltm) — Phase 6: Browser/worker verification + docs

**Goal:** LTM-on-wasm works through the Web Worker path with no protocol change — `getLinks` over a `WorkerBackend` matches node (and the VM) — and the docs that called LTM "VM-only" are updated to reflect support.

**Architecture:** `WorkerServer` wraps a `DirectBackend` and forwards `simGetLinks` straight to it; `simGetLinks` already traverses the worker for the VM engine. So the Phase 3 `DirectBackend` change automatically makes LTM-on-wasm work through the worker — no new message types, no protocol change. Phase 6 proves this with a `WorkerBackend` parity test (using the existing in-process worker test harness) and updates the stale "LTM (VM-only)" documentation.

**Tech Stack:** TypeScript (`@simlin/engine`), the in-process worker test pair (`createTestPair`), ts-jest. Markdown docs.

**Scope:** Phase 6 of 6.

**Codebase verified:** 2026-05-27

---

## Acceptance Criteria Coverage

This phase implements and tests:

### wasm-ltm.AC1: LTM-enabled wasm compilation produces a blob carrying the LTM series
- **wasm-ltm.AC1.4 Success:** the same `engine:'wasm'` + `enableLtm` path through `WorkerBackend` (browser) yields `getLinks` scores matching node.

### wasm-ltm.AC5: Engineering quality (cross-cutting) — completion
- **wasm-ltm.AC5.2:** new code reaches >=95% coverage via TDD; each new FFI function and each new lowering/feature group has unit tests.

> **AC5.2 is satisfied by construction, not by a separate coverage measurement.** Every phase follows the repo's TDD discipline and adds focused tests per new unit of behavior — the wasm compile flags (P1 layout on/off tests), each new FFI function (P2 `links_from_wasm` + `rel_loop_score_from_wasm` parity tests), the node read/analyze path (P3), each lowering/feature group (P4 arrayed/cross-element + the Unsupported path), discovery (P5), and the worker path (P6). The repo does not run a hard coverage-percentage gate in CI (it relies on TDD + the pre-commit suite), so no task produces a coverage report; the ">=95%" threshold is met by the per-feature test enumeration above, and a phase-end reviewer should verify that enumeration rather than look for a coverage number.

---

## Background: what exists today (verified 2026-05-27)

**The worker path needs no protocol change (`src/engine/src`):**
- `WorkerServer` wraps a `DirectBackend`: `this.backend = new DirectBackend()` (`worker-server.ts:45`); all `sim*` ops delegate to `this.backend.*`.
- `getLinks` already traverses the worker for VM: `simGetLinks` is in `WorkerRequest` (`worker-protocol.ts:95`) and `VALID_REQUEST_TYPES` (`:201`); `WorkerBackend.simGetLinks` sends it (`worker-backend.ts:607-613`); `WorkerServer` dispatches it to `this.backend.simGetLinks(handle)` (`worker-server.ts:433-437`). `simNew` already carries `request.engine` (`worker-server.ts:369`; protocol `:84` carries `engine?: SimEngine` + `enableLtm: boolean`).
- ⇒ The Phase 3 `DirectBackend.simGetLinks` wasm change is automatically exercised through the worker. **No new worker message types.**

**Worker tests run in-process (no real `worker_threads`/jsdom):**
- `createTestPair()` (`worker-backend.test.ts:35-61`) wires a real `WorkerServer` to a real `WorkerBackend` via two closures hopping `setTimeout(..., 0)` to simulate async `postMessage`. `jest.config.js` is `testEnvironment: 'node'`.
- The wasm-worker tests live in `tests/worker-wasm.test.ts` (same in-process pair). **The current rejection test to replace is `worker-wasm.test.ts:319-327`** (`'rejects enableLtm on the wasm engine across the worker boundary'`) — after Phase 3 the worker no longer rejects, so this test must flip to a parity assertion.

**The stale doc to fix:** `src/simlin-engine/CLAUDE.md:29` (the wasmgen bullet):
> Out of scope: LTM (VM-only); a true-runtime-range subscript (`ViewRangeDynamic`, GH #612) returns `WasmGenError::Unsupported`; array unrolling is bounded by `MAX_UNROLL_UNITS` (65,536 elements/function), above which a model cleanly returns `Unsupported` and the caller falls back to the VM.

There is **no** `src/simlin-engine/src/wasmgen/CLAUDE.md` (the design's reference is wrong; the note is in `simlin-engine/CLAUDE.md`). Related stale assertions: `src/engine/CLAUDE.md` (LTM-on-wasm behavior / no-VM-fallback wording) and any `src/libsimlin/CLAUDE.md` mention of LTM-on-wasm rejection; the prior `docs/design-plans/2026-05-22-engine-wasm-sim.md` out-of-scope note and its `docs/implementation-plans/2026-05-22-engine-wasm-sim/test-requirements.md` AC6.1/AC6.2 (which asserted the now-removed rejection).

**Divergence from the design doc:** the design says update "`src/simlin-engine/src/wasmgen/ CLAUDE.md`" — that file does not exist; the line is in `src/simlin-engine/CLAUDE.md:29`.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: WorkerBackend LTM-on-wasm parity test

**Verifies:** wasm-ltm.AC1.4

**Files:**
- Modify: `src/engine/tests/worker-wasm.test.ts` (replace the rejection test `:319-327`; add the parity test)

**Implementation / Testing:**
1. Replace `'rejects enableLtm on the wasm engine across the worker boundary'` (`:319-327`): the worker no longer rejects, so delete the `rejects`/`toThrow` assertion.
2. Add `'worker wasm getLinks matches node + VM'` (AC1.4): using `createTestPair()` and the scalar LTM fixture loader from Phase 3 (`test/logistic_growth_ltm/logistic_growth.stmx`):
   - Through the `WorkerBackend`: create a wasm sim with `enableLtm: true`, `runToEnd`, `simGetLinks` → `workerLinks`.
   - Through a direct node `DirectBackend` (or the Phase 3 test's result): same model/engine/options → `nodeLinks`.
   - Assert `workerLinks` equal `nodeLinks` (same link set, polarities, and `score` series within `1e-6`). Optionally also compare to a VM run for a three-way match.

**Verification:**
Run: `cd src/engine && npx jest tests/worker-wasm.test.ts`
Expected: the old rejection assertion is gone; the parity test passes.

**Commit:** `engine: verify LTM-on-wasm getLinks through the worker matches node`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Update the "LTM (VM-only)" docs

**Verifies:** (none — documentation; the design's "Done when: docs updated")

**Files:**
- Modify: `src/simlin-engine/CLAUDE.md:29` (the wasmgen bullet)
- Modify: `src/simlin-engine/src/wasmgen/lower.rs` — every "falls back to the VM" / "falling back to the VM" site: the `charge_unroll` rustdoc (`:1110`) and `WasmGenError::Unsupported` message (`:1120`), plus the related comment at `:1076`. (The verification grep below is repo-wide, so any other instance must be fixed too.)
- Modify: `src/engine/CLAUDE.md` and `src/libsimlin/CLAUDE.md` (only where they assert LTM-on-wasm is rejected / VM-only)
- Modify: `docs/design-plans/2026-05-22-engine-wasm-sim.md` (its out-of-scope note that listed LTM as VM-only)
- Modify: `docs/README.md` (only if the doc index needs a pointer update)

**Implementation:**
1. `simlin-engine/CLAUDE.md:29`: remove "LTM (VM-only)" from the out-of-scope list. Keep the `ViewRangeDynamic`/`MAX_UNROLL_UNITS` clauses, but **reconcile the "the caller falls back to the VM" wording** with the actual engine-wasm-sim contract (per `src/engine/CLAUDE.md`, the wasm path surfaces `Unsupported` as an error with **no silent VM fallback**). State that LTM now lowers to wasm, and that genuinely-unlowerable LTM models (exceeding `MAX_UNROLL_UNITS` or using an unlowerable opcode) return `WasmGenError::Unsupported` (a clean error), exactly like non-LTM models.
2. `wasmgen/lower.rs`: three sites literally say the model "falls back to the VM" / "falling back to the VM" — the comment at `:1076`, the `charge_unroll` rustdoc (`:1110`), and the `WasmGenError::Unsupported(...)` message it constructs (`:1120`). All directly contradict the no-silent-fallback contract that AC3 establishes and that an LTM caller now hits as a first-class outcome. Update all three to drop the VM-fallback claim, e.g. message: `"wasmgen: array unrolling exceeds the per-function budget of {MAX_UNROLL_UNITS} elements (a large arrayed model); the caller receives an explicit Unsupported error (no silent VM fallback)"`, and align the rustdoc/comment. Keep behavior identical (still returns `Unsupported`); only the wording changes. The repo-wide verification grep below guarantees none are missed.
3. `src/engine/CLAUDE.md` / `src/libsimlin/CLAUDE.md`: where they say LTM is rejected on the wasm engine or `getLinks` throws on wasm, update to: LTM is supported on the wasm engine; `getLinks` reads the blob slab and runs the shared analytic core via `simlin_analyze_links_from_wasm_results`.
4. `docs/design-plans/2026-05-22-engine-wasm-sim.md`: update its out-of-scope note so it no longer lists LTM as a permanent wasm gap (reference this wasm-ltm plan).
5. Do **not** edit `docs/implementation-plans/2026-05-22-engine-wasm-sim/*` (historical record); if its `test-requirements.md` AC6.1/AC6.2 (the removed rejection) are confusing, leave them as the historical record of that phase — the new behavior is documented in *this* plan's test-requirements.
6. No "Last updated" lines (repo rule).

**Verification:**
Run: `git grep -n "VM-only" src/ docs/`, `git grep -n "not supported on the wasm engine" src/`, and `git grep -nE "fall(s|ing) back to the VM" src/`
Expected: no remaining stale "LTM is VM-only / not supported on the wasm engine" claims, and no "falls back to the VM" / "falling back to the VM" wording in `src/` (the no-silent-fallback contract holds) — except in the historical 2026-05-22 implementation-plan record. `cargo build -p simlin-engine` still succeeds (the `lower.rs` change is wording-only), and `pnpm lint` / markdown checks pass.

**Commit:** `doc: wasmgen now supports LTM on the wasm backend`
<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->

---

## Phase 6 Done When

- A `WorkerBackend` `engine:'wasm'` + `enableLtm` `getLinks` matches node (and the VM) within `1e-6`, with **no** new worker message types / protocol change (**wasm-ltm.AC1.4**).
- The `worker-wasm.test.ts` rejection test is replaced by the parity test.
- `src/simlin-engine/CLAUDE.md:29` and other CLAUDE.md / design-plan notes no longer call LTM "VM-only"; the no-silent-fallback wording is reconciled everywhere, including the `wasmgen/lower.rs:1110-1124` rustdoc + `Unsupported` message string (verified by `git grep -E "fall(s|ing) back to the VM" src/` returning nothing).
- `pnpm --filter @simlin/engine test`, `pnpm lint`, and `cargo test --workspace` are green.
- **wasm-ltm.AC5.2** is complete by construction: every new FFI function, lowering/feature group, and wiring change across Phases 1-6 has focused TDD tests (enumerated in the AC Coverage note above); no separate coverage-percentage report is produced, consistent with repo practice.
