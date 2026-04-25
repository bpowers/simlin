# Phase 5 — Auto-Flip Threshold Measurement

**Goal:** Re-measure element-graph SCC sizes on representative arrayed fixtures after Phases 2/3 land. Decide whether `MAX_LTM_SCC_NODES = 50` should change. Append a measurement postscript to the design plan. No threshold change is expected; this phase is informational.

**Architecture:** Extend `src/simlin-engine/examples/ltm_full_bench.rs` to report element-level largest-SCC size. Run on four fixtures (cross_element, arrayed_population, hero_culture, WRLD3) before and after Phases 2/3. Document deltas.

**Tech Stack:** Rust example binary; no production code changes other than possibly a comment update on `MAX_LTM_SCC_NODES`.

**Codebase verified:** 2026-04-25 (Phase 5 codebase-investigator confirmed: `ltm_full_bench` exists; `largest_scc_size()` at `ltm.rs:1058`; `causal_graph_from_element_edges(...).largest_scc_size()` is the call used by the live gate at `db_ltm.rs:2128`; fixture paths exist; `hero_culture.sd.json` is JSON format and the bench can't load it as-is).

---

## Acceptance Criteria Coverage

This phase implements and verifies:

### ltm-per-ref-elem-graph.AC5: Performance does not regress
- **ltm-per-ref-elem-graph.AC5.1 SCC sizes shrink or stay equal:** On the fixtures `test/cross_element_ltm`, `test/arrayed_population_ltm`, `test/hero_culture_ltm`, and `test/metasd/WRLD3-03/wrld3-03.mdl`, the largest element-graph SCC after the fix is less-than-or-equal to the size before.
- **ltm-per-ref-elem-graph.AC5.2 Pre-commit budget honored:** The pre-commit hook completes successfully and `cargo test --workspace` remains under the 3-minute wall-clock cap.

---

## Implementation Tasks

<!-- START_TASK_1 -->
### Task 1: Add element-level SCC size reporting to `ltm_full_bench`

**Verifies:** infrastructure for AC5.1

**Files:**
- Modify: `src/simlin-engine/examples/ltm_full_bench.rs`

**Implementation:**
After the `element_edges` stage's existing computation, call `causal_graph_from_element_edges(elem_edges).largest_scc_size()` and append the value to the stage note. Example diff (conceptual):

```rust
// In the element_edges stage:
let elem_largest_scc = crate::db::causal_graph_from_element_edges(elem_edges).largest_scc_size();
let note = format!(
    "src_nodes={} total_edges={} stocks={} largest_scc={}",
    src_nodes, total_edges, stocks_count, elem_largest_scc
);
```

(Use the actual API — `causal_graph_from_element_edges` is in `db_analysis.rs` re-exported from `db.rs:30`.)

Also: in the `loop_circuits` stage, optionally report the variable-level largest SCC for comparison. Many bench runs report only one of the two; for Phase 5 we want both side-by-side.

**Testing:**
Run the bench on a small fixture (cross_element_ltm) to verify output:

```bash
cargo run --release --example ltm_full_bench -- test/cross_element_ltm/cross_element.stmx
```

Expected: stderr output contains a `largest_scc=N` value in the `element_edges` stage note.

**Commit:** `engine: report element-level SCC size in ltm_full_bench`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Optional — extend bench to load `.sd.json` fixtures

**Verifies:** infrastructure for AC5.1 (hero_culture)

**Files:**
- Modify: `src/simlin-engine/examples/ltm_full_bench.rs`

**Implementation:**
The hero_culture fixture is `.sd.json`, not `.mdl` or `.stmx`. Today's bench only handles `.mdl` (via `open_vensim`) and XMILE (`open_xmile`). To include hero_culture in measurements, add a JSON branch:

```rust
let datamodel_project = if path.ends_with(".sd.json") {
    let json = std::fs::read_to_string(&path)?;
    let project: simlin_engine::datamodel::Project = serde_json::from_str(&json)?;
    project
} else if path.ends_with(".stmx") || path.ends_with(".xmile") {
    open_xmile(&path)?
} else {
    open_vensim(&path)?
};
```

(Adapt to actual function signatures and the bench's existing loader pattern.)

If this proves more invasive than expected, **skip hero_culture** for the measurement table and document its absence with a one-line note. The other three fixtures cover the relevant cases.

**Testing:**
Run on hero_culture if extended:
```bash
cargo run --release --example ltm_full_bench -- test/hero_culture_ltm/hero_culture.sd.json
```
Expected: completes; output includes element_edges and largest_scc stats.

**Commit:** `engine: ltm_full_bench supports .sd.json fixtures` (or skip if not extended)
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Capture before/after measurements

**Verifies:** AC5.1

**Files:**
- None modified yet (measurement only)

**Implementation:**
By the time Phase 5 runs, Phases 2/3 commits are landed on the branch. Capture **after** numbers first (current HEAD), then check out the pre-Phase-2 commit to capture **before** numbers, then return to the branch HEAD.

Step 1 — capture "after" numbers on current HEAD:

```bash
cargo run --release --example ltm_full_bench -- test/cross_element_ltm/cross_element.stmx | tee /tmp/scc-cross-after.log
cargo run --release --example ltm_full_bench -- test/arrayed_population_ltm/arrayed_population.stmx | tee /tmp/scc-arrpop-after.log
cargo run --release --example ltm_full_bench -- test/metasd/WRLD3-03/wrld3-03.mdl | tee /tmp/scc-wrld3-after.log
```

Step 2 — identify the pre-Phase-2 commit SHA. Run `git log --oneline | head -20` and pick the last commit before Phase 2's first task ("engine: AST walker collect_reference_sites with shape classification" or whatever the first Phase 2 commit is named). Record this SHA in the postscript table for traceability.

Step 3 — checkout that SHA detached and run the bench:

```bash
git checkout <pre-phase-2-sha>
cargo run --release --example ltm_full_bench -- test/cross_element_ltm/cross_element.stmx | tee /tmp/scc-cross-before.log
# ... repeat for the other fixtures ...
git checkout ltm-per-ref-elem-graph
```

Step 4 — capture for each fixture (from the log pairs):
- Variable-level edge count (from `causal_edges` stage note)
- Variable-level largest SCC (from `loop_circuits` stage if reported, else compute manually)
- Element-level node count (from `element_edges` stage note)
- Element-level edge count (from `element_edges` stage note)
- Element-level largest SCC (from new bench output)
- Whether `auto_flipped` fires (compare element-largest-SCC to 50)

**Verification:**
Three log pairs in `/tmp/scc-*-{before,after}.log`. Eyeball each pair for correctness; no automated test.

**Commit:** No new commit (logs are local).
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Append measurement postscript to design plan

**Verifies:** AC5.1, AC6.3 (interim — final tech-debt update lands in Phase 6)

**Files:**
- Modify: `docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md` (append a "## Measurement Postscript" section at the end)

**Implementation:**
Append a section like:

```markdown
## Measurement Postscript

Captured 2026-MM-DD on commit `<sha>` (pre-Phase-2) and commit `<sha>` (post-Phase-4).

| Fixture | Pre var-edges | Pre elem-edges | Pre elem-SCC | Post elem-edges | Post elem-SCC | Auto-flip pre / post |
|---------|--------------:|---------------:|-------------:|----------------:|--------------:|---------------------|
| cross_element_ltm | ... | ... | ... | ... | ... | no / no |
| arrayed_population_ltm | ... | ... | ... | ... | ... | no / no |
| hero_culture_ltm | ... | ... | ... | ... | ... | (skipped if bench couldn't load) |
| WRLD3-03 | ... | ... | ... | ... | ... | yes / yes |

Notes:
- (Per fixture: explanation of what changed and why; e.g., "cross_element_ltm: edge count dropped from 21 to 13 because the four FixedIndex broadcasts no longer expand to 2x2 each. Largest SCC unchanged at 4 (already small).")
- Threshold decision: `MAX_LTM_SCC_NODES = 50` retained. Justification: WRLD3 still trips the gate (its SCCs come from variable-level cycles, not element-graph artifacts). Smaller models continue to fit comfortably under the threshold.
```

If the data justifies a threshold change (e.g., WRLD3 element-SCC dropped below 50), discuss with the user before changing.

**Testing:**
N/A — documentation only.

**Verification:**
Read the postscript and confirm it tells a coherent story.

**Commit:** `doc: measurement postscript for ltm per-ref element graph design`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Comment update on `MAX_LTM_SCC_NODES` (if warranted)

**Verifies:** AC5.1

**Files:**
- Possibly: `src/simlin-engine/src/ltm.rs:38-46` (`MAX_LTM_SCC_NODES` doc comment)

**Implementation:**
If the postscript shows that element-graph SCCs shrink materially on FixedIndex models post-refactor, add a one-paragraph note to the existing docstring saying so. For example:

```rust
/// ...existing docstring...
///
/// Note (2026-04-25): After the per-reference element-graph refactor
/// (docs/design-plans/2026-04-25-ltm-per-ref-elem-graph.md), FixedIndex
/// references no longer expand to NxN edges. This shrinks element-graph
/// SCCs on models that use literal-index references (e.g., the cross_element
/// fixture) but does not change WRLD3's behavior — its SCCs come from
/// variable-level feedback cycles rather than spurious cross-element edges.
/// The threshold remains conservative at 50 to keep existing test models
/// on the exhaustive path.
pub const MAX_LTM_SCC_NODES: usize = 50;
```

If the measurement data does NOT show meaningful SCC shrinkage, skip this task.

**Verification:**
Run: `cargo build -p simlin-engine`. Expected: clean build (comment changes only).

**Commit:** `engine: note FixedIndex refactor's effect on element-graph SCCs` (or skip)
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Pre-commit budget verification

**Verifies:** AC5.2

**Files:**
- None modified

**Implementation:**
Run the pre-commit hook script and observe `cargo test --workspace` wall-clock time. Confirm <180s.

**Verification:**
Run: `bash scripts/pre-commit`. Expected: prints "All pre-commit checks passed!" within budget.

If `cargo test` is close to the cap, identify the slowest tests via `cargo test -p simlin-engine -- --report-time` and consider gating any expensive new test under `#[ignore]` for on-demand runs.

**Commit:** No new commit (verification gate).
<!-- END_TASK_6 -->

---

## Phase Done When

- All 6 implementation tasks committed (Tasks 1, 2, 4 produce commits; Tasks 3, 5, 6 may be no-op based on measurements).
- Design plan has a "Measurement Postscript" section with concrete before/after numbers.
- `MAX_LTM_SCC_NODES = 50` either retained with a documentation note OR adjusted with explicit user discussion.
- Pre-commit hook passes within 180s.
