# Human Test Plan: MCP Publish Ready

## Prerequisites

- Environment: Clone the repository, run `./scripts/dev-init.sh`
- All automated tests passing: `cargo test -p simlin-engine && cargo test -p simlin-mcp`
- The `simlin-mcp` binary is built: `cargo build -p simlin-mcp`

## Phase 1: Atomic File Writes (AC1)

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | Inspect `src/simlin-engine/src/io.rs` lines 36-42: confirm `let _ = dir.sync_all()` is present in the `write_and_rename` function after the `fs::rename` call | The `let _` pattern discards any fsync error, making it best-effort. This is the structural guarantee for AC1.5 |

## Phase 2: XMILE Write-Back (AC2)

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Copy `test/logistic_growth_ltm/logistic_growth.stmx` to `/tmp/test_xmile/`. Start `simlin-mcp` via stdio. Send an `initialize` request, then an `EditModel` tools/call adding `{"upsertAuxiliary": {"name": "manual_test", "equation": "42"}}` with `projectPath` pointing to the copy. | Response `isError: false`. `projectPath` in the output equals the .stmx input path |
| 2.2 | Open the .stmx file in a text editor | The XML contains an `<aux name="manual_test">` element with equation `42` |
| 2.3 | Verify no `.simlin.json` file was created alongside the .stmx | Only the .stmx file exists in `/tmp/test_xmile/` |
| 2.4 | Send a `ReadModel` tools/call for the same .stmx file | Response contains `manual_test` in the model's auxiliaries array |
| 2.5 | Send an `EditModel` tools/call with `dryRun: true` adding another variable | Response includes the new variable in the model snapshot. The .stmx file on disk is unchanged (compare with step 2.2 content) |

## Phase 3: Structured Error Responses (AC3)

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | Create a JSON model file with a broken equation (e.g., aux referencing `nonexistent_var`). Send a `ReadModel` tools/call | Response has `isError: false`. The JSON output has a `model` object (snapshot) AND a non-empty `errors` array |
| 3.2 | Inspect each entry in the `errors` array | Each has `code` (string, e.g., "unknown_dependency"), `message` (string), `variableName` (string), and `kind` (string, e.g., "variable") |
| 3.3 | Send a `ReadModel` tools/call for `test/logistic-growth.sd.json` | The `errors` field is absent from the response (not an empty array, but completely missing) |
| 3.4 | Send an `EditModel` tools/call that introduces an error (e.g., upsert aux with `equation: "ghost_var + 1"`) | Response has `isError: true` |
| 3.5 | Parse the `structuredContent` field of the error response | Contains an `errors` array with entries having `code`, `message`, `kind` fields |
| 3.6 | After step 3.4, read the model file on disk | File bytes are unchanged from before the failed edit |

## Phase 4: Instructions (AC4)

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Start `simlin-mcp`, send an `initialize` request | Response `result.instructions` is a non-empty string |
| 4.2 | Inspect the instructions string | Contains the words: `ReadModel`, `EditModel`, `CreateModel`, `.mdl`, `pysimlin` |
| 4.3 | Search the instructions for loop naming guidance | Contains `setLoopName` and `variables` |
| 4.4 | Search the instructions for a version string | Contains `0.6.2` (the current pysimlin version) |
| 4.5 | Run `git tag --list 'pysimlin-v*' --sort=-v:refname | head -1` and strip the prefix | The version from the tag matches what appears in instructions.md and skills/pysimlin-basics.md |

## Phase 5: MCP Resources / Skills (AC5)

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | Send a `resources/list` JSON-RPC request | Response contains exactly 4 resources with URIs: `simlin://skills/pysimlin-basics`, `simlin://skills/scenario-analysis`, `simlin://skills/loop-dominance`, `simlin://skills/vensim-equation-syntax`. Each has `name`, `description`, `mimeType: "text/markdown"` |
| 5.2 | Send `resources/read` with `uri: "simlin://skills/pysimlin-basics"` | Response `contents[0].text` is non-empty markdown. `contents[0].uri` equals the requested URI |
| 5.3 | Send `resources/read` with `uri: "simlin://skills/nonexistent"` | JSON-RPC error with code `-32002` |
| 5.4 | Inspect `src/simlin-mcp/src/resource.rs` | All content fields use `include_str!("skills/...")`. All metadata fields are `&'static str`. No `std::fs::read` calls anywhere in the module |

## Phase 6: Loop Naming (AC6)

| Step | Action | Expected |
|------|--------|----------|
| 6.1 | Create a model with a feedback loop (stock "population", flow "births" where births = population * rate). Send `EditModel` with `SetLoopName` operation listing the loop variables, name: "Growth Loop", description: "reinforcing growth" | Response `isError: false` |
| 6.2 | Send `ReadModel` on the same file. Inspect the `loopDominance` array | At least one entry has `"name": "Growth Loop"` |
| 6.3 | Send `EditModel` with `SetLoopName` using the same variables but a new name "Updated Growth" | Response `isError: false`. Subsequent `ReadModel` shows "Updated Growth" (not both names) |
| 6.4 | Send `EditModel` with `SetLoopName` listing a nonexistent variable name | Response `isError: true` with error mentioning the unknown variable |

## Phase 7: JSON Format Detection (AC7)

| Step | Action | Expected |
|------|--------|----------|
| 7.1 | Send `ReadModel` with `test/logistic-growth.sd.json` (native format, has top-level `models` key) | Returns a valid model object |
| 7.2 | Send `ReadModel` with `test/sd-ai-simple.sd.json` (SD-AI format, has top-level `variables` key) | Returns a valid model object containing the expected variables (e.g., "Population") |
| 7.3a | Copy `test/sd-ai-simple.sd.json` to a temp directory. Send `EditModel` to add a variable. Read the file from disk | File still has top-level `"variables"` key, not `"models"` |
| 7.3b | Create a native JSON model file. Send `EditModel` to add a variable. Read the file from disk | File still has top-level `"models"` key, not `"variables"` |
| 7.4 | Create a file with content `{"unrelated": true}` and `.sd.json` extension. Send `ReadModel` | Error response mentioning both "models" and "variables" expected formats |
| 7.5 | Create two `.sd.json` files: one with native content, one with SD-AI content. Send `ReadModel` on each | Both succeed: first detected as native, second as SD-AI |

## End-to-End: Full MCP Client Lifecycle

**Purpose:** Validates that a realistic MCP client session works from initialize through model creation, editing, error handling, and resource access.

1. Start `simlin-mcp` via stdio (pipe stdin/stdout).
2. Send `initialize` with valid client info. Verify response has `protocolVersion`, `serverInfo`, `capabilities.tools`, `capabilities.resources`, and non-empty `instructions`.
3. Send `notifications/initialized` (fire-and-forget). No response expected.
4. Send `tools/list`. Verify 3 tools returned: ReadModel, EditModel, CreateModel.
5. Send `tools/call` for `CreateModel` with a new file path. Verify the file is created.
6. Send `tools/call` for `EditModel` to add a stock, flow, and auxiliary to the new model. Verify response has model snapshot, time array, loopDominance array.
7. Send `tools/call` for `ReadModel` on the same file. Verify all added variables appear.
8. Send `tools/call` for `EditModel` with a broken equation. Verify `isError: true` and file is unchanged.
9. Send `resources/list`. Verify 4 resources returned.
10. Send `resources/read` for each of the 4 skill URIs. Verify non-empty content.
11. Close stdin (EOF). Verify the server exits cleanly.

## End-to-End: XMILE Round-Trip Fidelity

**Purpose:** Validates that editing an XMILE file does not corrupt existing model structure.

1. Copy `test/logistic_growth_ltm/logistic_growth.stmx` to a temp directory.
2. Send `ReadModel` on the copy. Record the number of stocks, flows, auxiliaries, and loop dominance entries.
3. Send `EditModel` to add one auxiliary variable.
4. Send `ReadModel` again. Verify all original variables are still present plus the new one. Loop dominance analysis should still produce results.
5. Open the .stmx file in a text editor or another XMILE-compatible tool. Verify the XML is well-formed and the original model structure is intact.

## End-to-End: SD-AI Format Preservation

**Purpose:** Validates that the SD-AI JSON format survives an edit round-trip without format corruption.

1. Copy `test/sd-ai-simple.sd.json` to a temp directory.
2. Read the file, confirm top-level key is `"variables"`.
3. Send `EditModel` to add an auxiliary. Verify response success.
4. Read the file from disk. Confirm top-level key is still `"variables"` (not `"models"`).
5. Send `ReadModel` on the file. Confirm the new variable appears in the model.

## Human Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC1.5 (fsync best-effort) | Cannot portably force an fsync failure in a test on Linux. The structural guarantee (`let _ = dir.sync_all()`) must be verified by code inspection | Inspect `src/simlin-engine/src/io.rs` lines 37-40. Confirm the `let _ =` pattern on `dir.sync_all()` |
| AC5.4 (compiled into binary) | Compile-time guarantee via `include_str!` cannot be tested at runtime beyond verifying content loads | Inspect `src/simlin-mcp/src/resource.rs`. Confirm all content fields use `include_str!` and all types are `&'static str`. Confirm no `std::fs::read` calls |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 | `io.rs::writes_correct_content` | -- |
| AC1.2 | `io.rs::temp_file_cleaned_up_after_success` | -- |
| AC1.3 | `io.rs::overwrites_existing_file` | -- |
| AC1.4 | `io.rs::returns_error_when_parent_dir_missing` | -- |
| AC1.5 | `io.rs::succeeds_on_normal_directory` | Phase 1, Step 1.1 |
| AC2.1 | `edit_model.rs::ac2_1_stmx_edit_writes_back_to_original_file` | Phase 2, Steps 2.1-2.2 |
| AC2.2 | `edit_model.rs::ac2_2_xmile_roundtrip_edit_then_read` | Phase 2, Step 2.4 |
| AC2.3 | `edit_model.rs::ac2_3_project_path_equals_input_for_{stmx,native_json,sdai_json}` | Phase 2, Step 2.1 |
| AC2.4 | `edit_model.rs::ac2_4_xmile_dry_run_does_not_modify_file` | Phase 2, Step 2.5 |
| AC2.5 | `edit_model.rs::ac2_5_no_sidecar_created_for_stmx_edit` | Phase 2, Step 2.3 |
| AC3.1 | `read_model.rs::ac3_1_broken_equations_return_errors` | Phase 3, Step 3.1 |
| AC3.2 | `read_model.rs::ac3_2_error_fields_present` | Phase 3, Step 3.2 |
| AC3.3 | `read_model.rs::ac3_3_clean_model_omits_errors` | Phase 3, Step 3.3 |
| AC3.4 | `edit_model.rs::ac3_4_edit_with_compilation_error_returns_is_error` | Phase 3, Step 3.4 |
| AC3.5 | `edit_model.rs::ac3_5_structured_error_includes_error_details` | Phase 3, Step 3.5 |
| AC3.6 | `edit_model.rs::ac3_6_file_not_modified_when_edit_has_errors` | Phase 3, Step 3.6 |
| AC3.7 | `types.rs::error_code_strings_align_with_pysimlin` | -- |
| AC4.1 | `protocol.rs::test_initialize_instructions_present_when_configured` | Phase 4, Step 4.1 |
| AC4.2 | `main.rs::instructions_mention_core_tools` | Phase 4, Step 4.2 |
| AC4.3 | `main.rs::instructions_include_set_loop_name` | Phase 4, Step 4.3 |
| AC4.4 | `main.rs::instructions_reference_pysimlin_version` | Phase 4, Step 4.4 |
| AC4.5 | `main.rs::instructions_reference_current_pysimlin_version` | Phase 4, Step 4.5 |
| AC5.1 | `protocol.rs::test_resources_list` | Phase 5, Step 5.1 |
| AC5.2 | `protocol.rs::test_resources_read_valid_uri` | Phase 5, Step 5.2 |
| AC5.3 | `protocol.rs::test_resources_read_unknown_uri` | Phase 5, Step 5.3 |
| AC5.4 | `resource.rs` (structural) + `protocol.rs::test_resources_read_valid_uri` | Phase 5, Step 5.4 |
| AC6.1 | `patch.rs::set_loop_name_creates_loop_metadata` | Phase 6, Step 6.1 |
| AC6.2 | `patch.rs::set_loop_name_updates_existing_loop` | Phase 6, Step 6.3 |
| AC6.3 | `patch.rs::set_loop_name_unknown_variable_returns_error` | Phase 6, Step 6.4 |
| AC6.4 | `read_model.rs::ac6_4_loop_names_surface_in_read_model` | Phase 6, Step 6.2 |
| AC7.1 | `mod.rs::ac7_1_open_project_detects_native_json` | Phase 7, Step 7.1 |
| AC7.2 | `mod.rs::ac7_2_open_project_detects_sdai_json` | Phase 7, Step 7.2 |
| AC7.3 | `edit_model.rs::ac7_3_sdai_json_format_preserved_after_edit`, `ac7_3_native_json_format_preserved_after_edit` | Phase 7, Steps 7.3a-7.3b |
| AC7.4 | `mod.rs::ac7_4_unrecognized_json_returns_error` | Phase 7, Step 7.4 |
| AC7.5 | `mod.rs::ac7_5_sd_json_extension_works_for_both_formats` | Phase 7, Step 7.5 |
