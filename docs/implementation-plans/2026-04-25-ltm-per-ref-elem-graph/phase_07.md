# Phase 7 — Final Cleanup

**Goal:** Remove any temporary scaffolding (especially `#[allow(dead_code)]` attributes) introduced in Phases 1–6, verify the workspace is fully clean, and confirm the pre-commit hook passes within budget.

**Architecture:** Cleanup pass; no new functionality.

**Tech Stack:** Rust, pre-commit hook.

**Codebase verified:** 2026-04-25 (Phase 7 codebase-investigator confirmed: ~63 `#[allow(dead_code)]` annotations exist in the engine today and are mostly unrelated to this refactor; orphan candidates from earlier phases include `classify_element_dependency`, `classify_in_expr`, `expand_edge_to_elements`, `expand_same_element`, `ElementDependencyKind`, `build_partial_equation`, `wrap_deps_in_previous`, `wrap_index_deps_in_previous`).

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### ltm-per-ref-elem-graph.AC5: Performance does not regress
- **ltm-per-ref-elem-graph.AC5.2 Pre-commit budget honored:** The pre-commit hook completes successfully and `cargo test --workspace` remains under the 3-minute wall-clock cap.

> Note: AC5.2 was checked at the end of every prior phase as an interim gate. Phase 7 performs the final verification as a single-pass clean run.

---

## Implementation Tasks

<!-- START_TASK_1 -->
### Task 1: Audit for orphaned code from earlier phases

**Verifies:** infrastructure for AC5.2 (clean build)

**Files:**
- Read: `src/simlin-engine/src/db_analysis.rs`
- Read: `src/simlin-engine/src/ltm_augment.rs`
- Read: `src/simlin-engine/src/db_ltm.rs`
- Read: `src/simlin-engine/src/ltm.rs`

**Implementation:**
Scan each modified file for orphaned remnants of the refactor. Specifically search for:

1. **Functions still present that should have been deleted in Phase 2:**
   - `classify_element_dependency`
   - `classify_in_expr`
   - `expand_edge_to_elements`
   - `expand_same_element` (only if no longer referenced)
   - `ElementDependencyKind` enum
   - `mod classify_element_dependency_tests`

2. **Functions still present that should have been deleted in Phase 3:**
   - `build_partial_equation` (replaced by `build_partial_equation_shaped`)
   - `wrap_deps_in_previous`
   - `wrap_index_deps_in_previous`

3. **`#[allow(dead_code)]` annotations** added during the refactor that are now reachable. Run:
   ```bash
   git log --grep='LTM\|ltm-per-ref' -p src/simlin-engine/src/ | grep -A2 -B2 'allow(dead_code)'
   ```
   Identify any suppression attributes added by Phases 1–6 commits that should now be removed.

4. **`TODO(phase-N)` or `// temporary` comments** left by earlier phases.

5. **Imports** that were added for now-removed types (e.g., a `use` line for `ElementDependencyKind` that should follow the type's deletion).

For each orphan found, decide:
- If the function/enum is genuinely unused now, delete it.
- If it's still used (e.g., `expand_same_element` may still be called by the new walker for partial-collapse), leave it but verify it's reachable.

**Verification:**
Run: `cargo build -p simlin-engine 2>&1 | tee /tmp/phase7-build.log`. Expected: clean build with no warnings.
Run: `cargo clippy -p simlin-engine --all-targets --all-features -- -D warnings 2>&1 | tee /tmp/phase7-clippy.log`. Expected: no warnings, no errors.

**Commit:** `engine: remove orphan helpers and dead-code suppressions from LTM refactor` (only if anything was actually removed; otherwise no commit)
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Verify all Phase 1 ignored tests are activated

**Verifies:** AC5.2 (test coverage live)

**Files:**
- Read: `src/simlin-engine/src/db_element_graph_tests.rs`
- Read: `src/simlin-engine/src/db_element_graph_proptest.rs`
- Read: `src/simlin-engine/src/ltm_augment.rs` (`#[cfg(test)] mod tests` block)
- Read: `src/simlin-engine/tests/simulate_ltm.rs`

**Implementation:**
Phase 1 added several tests with `#[ignore = "Phase N: ..."]` annotations. Phases 2 and 3 removed those annotations as the underlying APIs landed. Verify no stragglers remain by grepping:

```bash
grep -rn 'ignore.*Phase' src/simlin-engine/src/ src/simlin-engine/tests/
```

If any `#[ignore = "Phase ..."]` annotation remains, investigate:
- Is the annotation pointing to a phase that has not been completed? If so, Phases 2–4 missed an activation step. Investigate and fix.
- Is the test for behavior that's still genuinely deferred (e.g., a follow-up tech-debt item)? If so, replace the phase-pointer comment with a tech-debt-pointer comment.

**Verification:**
Run: `cargo test -p simlin-engine -- --ignored 2>&1 | grep 'test result'`. Expected: most or all ignored tests are now obsolete or active. The remaining ignored tests should map to deliberately-deferred work, not Phase-1 placeholders.

**Commit:** `engine: activate any straggler Phase-1 ignored tests` (only if any were found; otherwise no commit)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Final pre-commit hook end-to-end

**Verifies:** AC5.2

**Files:**
- None modified

**Implementation:**
Trigger the pre-commit hook on a fresh commit (or via `git commit --amend --no-edit`). Watch all stages:

```bash
git commit --amend --no-edit
```

Expected hook output:
```
Checking dependency policy... ✓
Checking documentation links... ✓
Running project lint rules... ✓
Running Rust checks (fmt, cbindgen, clippy, test)... ✓
Running TypeScript checks (lint, build, tsc, test)... ✓
Running pysimlin tests (Python 3.11+)... ✓
All pre-commit checks passed!
```

**Wall-clock budget check:**
Time the Rust test stage. If `cargo test --workspace` is approaching 180s, identify the slowest tests:

```bash
cargo test -p simlin-engine -- --report-time 2>&1 | sort -k2 -n | tail -20
```

If a Phase 1–6 test is in the top-20 slowest, consider gating it under `#[ignore]` for on-demand runs, with documentation pointing to its purpose.

**Verification:**
Pre-commit prints "All pre-commit checks passed!" within the 180s test budget.

**Commit:** No new commit (verification gate). If anything was adjusted (e.g., a slow test moved to `#[ignore]`), a separate commit captures it.
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Stretch — verify auto-memory entries

**Verifies:** none directly (operational hygiene)

**Files:**
- Read: `/home/bpowers/.claude/projects/-home-bpowers-src-simlin/memory/MEMORY.md` (if it exists)

**Implementation:**
The Phase 6 codebase-investigator confirmed no MEMORY.md entries reference `ElementDependencyKind` or the old element-graph behavior, so no cleanup is needed there. Phase 7 just verifies this is still true:

```bash
grep -i 'ElementDependencyKind\|classify_element_dependency\|expand_edge_to_elements' \
    ~/.claude/projects/-home-bpowers-src-simlin/memory/*.md \
    2>/dev/null
```

Expected: no matches. If matches appear (e.g., from a memory file added during this work), update those memory entries to reflect the post-refactor state.

**Verification:**
The grep returns no matches, OR matches found are documented as updated.

**Commit:** N/A (memory file is per-user, not in repo).
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Branch wrap-up — verify clean state, no uncommitted changes

**Verifies:** AC5.2 (final gate)

**Files:**
- None modified

**Implementation:**
Verify the branch is clean and all phases' commits are present:

```bash
git status                              # clean working tree
git log --oneline main..HEAD            # list of commits on this branch
git diff main...HEAD --stat             # summary of files changed
```

Expected: clean working tree; the branch contains a coherent series of commits, one per task in each phase.

**Verification:**
- `git status` shows "nothing to commit, working tree clean".
- `git log --oneline main..HEAD` shows commits for all 7 phases.
- `git diff main...HEAD --stat` shows changes in `db_analysis.rs`, `db_ltm.rs`, `ltm_augment.rs`, `ltm.rs`, `ltm_finding.rs`, integration tests, design doc, CLAUDE.md, tech-debt.md, and the implementation-plan documents.

**Commit:** N/A (final verification).
<!-- END_TASK_5 -->

---

## Phase Done When

- All 5 cleanup tasks executed; no orphan code or dead-code suppressions remain from this refactor.
- All Phase 1 `#[ignore]` placeholders are activated; surviving `#[ignore]` annotations point to deliberately-deferred work.
- Pre-commit hook passes cleanly within the 180s budget.
- Working tree is clean; the branch contains all expected commits.
- No memory entries reference removed types.
- The branch is ready for merge or further validation (e.g., a final code review).
