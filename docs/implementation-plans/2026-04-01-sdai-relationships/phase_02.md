# SD-AI Relationships Array Generation - Implementation Plan

**Goal:** Generate the SD-AI JSON `relationships` array from equation dependency graphs and computed polarities, replacing the re-attach-from-original-file approach.

**Architecture:** Pure function `generate_relationships()` in `json_sdai.rs` takes pre-computed polarity map and datamodel, filters stock-flow structural edges, maps remaining equation-derived edges to `Relationship` structs with computed polarity. Called from `edit_model.rs` during SD-AI JSON serialization.

**Tech Stack:** Rust (simlin-engine crate, simlin-mcp crate), serde, std collections

**Scope:** 2 phases from original design (phases 1-2)

**Codebase verified:** 2026-04-01

---

## Acceptance Criteria Coverage

This phase implements and tests:

### sdai-relationships.AC4: Integration with SD-AI JSON serialization
- **sdai-relationships.AC4.1 Success:** Editing an SD-AI JSON model via `handle_edit_model` produces output containing generated `relationships` array
- **sdai-relationships.AC4.2 Success:** No `reasoning` or `polarityReasoning` fields appear in output

---

## Reference Files

The implementor should read the following CLAUDE.md files for project conventions:

- `/home/bpowers/src/simlin/CLAUDE.md` -- root project guidelines (TDD mandate, commit style, comment standards)
- `/home/bpowers/src/simlin/src/simlin-engine/CLAUDE.md` -- engine module map
- `/home/bpowers/src/simlin/src/simlin-mcp/CLAUDE.md` -- MCP tool architecture, test patterns

Key source files for this phase:

- `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/edit_model.rs` -- `SdaiJson` serialization arm (lines 322-334), integration tests (line 536+)
- `/home/bpowers/src/simlin/src/simlin-engine/src/json_sdai.rs` -- `generate_relationships()` (added in Phase 1), `filter_stale_relationships()` (line 422, to be removed), test module (line 684)
- `/home/bpowers/src/simlin/src/simlin-engine/src/db_analysis.rs` -- `compute_link_polarities()` (line 388), re-exported from `simlin_engine::db`
- `/home/bpowers/src/simlin/src/simlin-engine/src/analysis.rs` -- `analyze_model()` (line 72); shows pattern for obtaining `SourceModel` from `source_project.models(db)` (line 142)
- `/home/bpowers/src/simlin/src/simlin-engine/src/common.rs` -- `canonicalize()` (line 323) for model name lookup
- `/home/bpowers/src/simlin/src/simlin-engine/src/datamodel.rs` -- `Project::get_model()` (line 1053)

---

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->

<!-- START_TASK_1 -->
### Task 1: Wire generate_relationships into SdaiJson serialization path and update tests

**Verifies:** sdai-relationships.AC4.1, sdai-relationships.AC4.2

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/edit_model.rs` (lines 322-334 for SdaiJson arm, lines 2083-2114 and 2334-2383 for tests)

**Implementation:**

Replace the `SdaiJson` match arm at lines 322-334 in `edit_model.rs`. The current code re-parses the original file to recover `relationships` and calls `filter_stale_relationships()`. Replace it with `compute_link_polarities` + `generate_relationships`:

Current code to replace (lines 322-334):
```rust
super::SourceFormat::SdaiJson => {
    let mut sdai_model = simlin_engine::json_sdai::SdaiModel::from(&project);
    // The relationships field is not captured in datamodel::Project,
    // so re-parse the original content to retrieve it and avoid
    // silently dropping it on write-back.
    if let Ok(original) =
        serde_json::from_str::<simlin_engine::json_sdai::SdaiModel>(&contents)
    {
        sdai_model.relationships = original.relationships;
        sdai_model.filter_stale_relationships();
    }
    serde_json::to_string_pretty(&sdai_model)?.into_bytes()
}
```

Replace with:
```rust
super::SourceFormat::SdaiJson => {
    let mut sdai_model = simlin_engine::json_sdai::SdaiModel::from(&project);
    let canonical_name = simlin_engine::canonicalize(&model_name).into_owned();
    if let Some(source_model) =
        source_project.models(&db).get(&canonical_name).copied()
    {
        let polarities = simlin_engine::db::compute_link_polarities(
            &db,
            source_model,
            source_project,
        );
        if let Some(dm_model) = project.get_model(&model_name) {
            sdai_model.relationships = Some(
                simlin_engine::json_sdai::generate_relationships(
                    &polarities, dm_model,
                ),
            );
        }
    }
    serde_json::to_string_pretty(&sdai_model)?.into_bytes()
}
```

Key context for the replacement: `source_project` is a `SourceProject` (which is `Copy`) obtained from `sync.project` at line 308. `db` is a `SimlinDb` created at line 272. `project` is the post-patch `datamodel::Project`. `model_name` is a `String` obtained at line 230. `source_project.models(&db)` returns a salsa-tracked `HashMap<String, SourceModel>` keyed by canonical name -- the same pattern used in `analysis.rs` line 142. No new `use` imports are needed in `edit_model.rs` -- the code uses fully qualified paths (`simlin_engine::canonicalize`, `simlin_engine::db::compute_link_polarities`, `simlin_engine::json_sdai::generate_relationships`).

**Testing:**

Update existing integration tests in `edit_model.rs` to reflect the new behavior (relationships are generated from equations, not preserved from the input file):

- **sdai-relationships.AC4.1:** Rewrite `sdai_relationships_preserved_after_edit` (line 2083) as `sdai_relationships_generated_after_edit`. The fixture has stock `population` (eq="1000") and flow `births` (eq="population * 0.03"). The fixture's stock has NO `inflows`/`outflows` fields in the SD-AI JSON, which means after conversion to datamodel, the stock has empty inflows/outflows -- so there are no stock-flow structural edges to filter. After adding `birth_rate` aux, the only equation-derived dependency is `births` referencing `population`, giving exactly one generated relationship: `{from: "population", to: "births", polarity: "+"}`. Note the direction is reversed from the original fixture's input relationship `{from: "births", to: "population"}` -- this is correct because the old relationship was a hand-written input, while the new one is derived from the equation `births = population * 0.03`. Assert exactly one relationship, verify `from`, `to`, and `polarity` fields.

- **sdai-relationships.AC4.2:** In the same updated test, verify that the output relationships have NO `reasoning` or `polarityReasoning` fields. Parse the saved JSON and check that `rels[0].get("reasoning")` is `None` and `rels[0].get("polarityReasoning")` is `None`.

- **Rename and update** `remove_variable_filters_stale_sdai_relationships` (line 2334) to `sdai_relationships_reflect_current_equations_after_remove`. The model has auxes A (eq=10), B (eq=A*2), C (eq=A+1). After removing B, generated relationships should be `{from: "A", to: "C", polarity: "+"}`. The existing assertions (len==1, from=="A", to=="C") already match the generated behavior, but the test name and comments should reflect that this now tests generation rather than stale filtering. Also verify polarity is `"+"`.

- The `sdai_relationships_preserved_on_dry_run` test (line 2117) needs NO changes -- dry-run skips the serialization block entirely, so the file on disk remains unchanged. The test correctly asserts the file is unmodified.

- The `sdai_fixture_with_relationships()` helper (line 2053) is still used by the dry-run test and can remain as-is.

**Verification:**

```bash
cargo test -p simlin-mcp
```

Expected: all tests pass, including updated relationship tests.

**Commit:** `mcp: generate SD-AI relationships from equation polarities instead of re-attaching from file`

<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Remove filter_stale_relationships and its unit tests

**Verifies:** None (dead code removal)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-engine/src/json_sdai.rs` (remove `filter_stale_relationships` method and its two unit tests)

Note: the line numbers below reference the pre-Phase-1 state. Phase 1 adds code to this file, shifting line numbers. Search by function/test name rather than line number.

**Implementation:**

Remove `filter_stale_relationships()` method (lines 416-437) from `SdaiModel` in `json_sdai.rs`. Also remove these two unit tests from the `#[cfg(test)] mod tests` block:

- `filter_stale_relationships_removes_references_to_absent_vars` (lines 1199-1254)
- `filter_stale_relationships_noop_when_none` (lines 1256-1268)

After this removal, the only caller was the `SdaiJson` arm in `edit_model.rs` (already replaced in Task 1). No other code references `filter_stale_relationships`.

Also remove the `/// Remove relationships that reference variables not present in the model.` doc comment block (lines 416-421) above the function.

**Verification:**

```bash
cargo test -p simlin-engine json_sdai && cargo test -p simlin-mcp
```

Expected: all tests pass. No compilation warnings about unused code.

**Commit:** `engine: remove filter_stale_relationships (replaced by generate_relationships)`

<!-- END_TASK_2 -->

<!-- END_SUBCOMPONENT_A -->
