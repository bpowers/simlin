# simlin-mcp 0.1.0 Publish Readiness Design

## Summary

Simlin-mcp is the MCP server that exposes the Simlin simulation engine as tools for AI assistants. Today it can read, create, and edit system dynamics models, but several gaps make it unsuitable for a reliable 0.1.0 release: edited XMILE files get redirected to a JSON sidecar instead of being written back to the original file, compilation errors are silently swallowed rather than surfaced to the agent, there is no way to name feedback loops, and the server ships without onboarding instructions or reference material.

This design addresses those gaps across seven parallel-where-possible phases. At the engine layer, it adds an atomic file-write utility (temp file, fsync, rename) and moves the error-formatting code out of the C FFI crate into simlin-engine so both the MCP server and pysimlin can share it. At the MCP layer, it makes EditModel format-aware so XMILE, native JSON, and SD-AI JSON files are all written back in their original format through atomic writes, and it adds a compile-then-check gate that rejects edits introducing errors before anything touches disk. ReadModel gains an optional structured errors field so agents can see exactly what is wrong with a model. A new SetLoopName patch operation lets agents name feedback loops by specifying the variable list that forms the loop. Finally, the server gains comprehensive instructions embedded in the binary, and four skill resources served over the MCP resources protocol, giving agents on-demand reference material for pysimlin usage, scenario analysis, loop dominance, and Vensim equation syntax.

## Definition of Done

Prepare simlin-mcp for its first NPM publish as `@simlin/mcp` by making the tools reliable, informative, and well-documented for AI agent consumers (Claude, Codex, Gemini). Specifically:

1. **Atomic file writes** -- A reusable `atomic_write()` function in simlin-engine that writes via sibling temp file, fsync, and rename.
2. **XMILE write-back** -- EditModel writes edits back to the original `.stmx`/`.xmile`/`.xml` file instead of a `.simlin.json` sidecar.
3. **Structured error responses** -- ReadModel returns an optional `errors` field with structured error details when the model has compilation problems. EditModel returns an MCP-level error with structured details when an edit would introduce errors.
4. **Comprehensive instructions** -- `instructions.md` compiled into the binary with: core modeling guidance, equation syntax reference, basic loop dominance explanation, tool workflow, format support, loop naming guidance with example. A Rust test validates the referenced pysimlin version matches the latest `pysimlin-v*` tag.
5. **MCP resources (skills)** -- Extend the protocol to support `resources/list` and `resources/read`. Four skills as `.md` files compiled into the binary: pysimlin basics, scenario analysis patterns, loop dominance analysis, Vensim equation syntax.
6. **Coherent error model** -- Error codes and shapes align between MCP tool responses and pysimlin's error API. Move `FormattedError` and `format_diagnostic` from libsimlin into simlin-engine so both MCP and pysimlin share the same formatting code.
7. **Loop naming operation** -- A `SetLoopName` operation in EditModel that accepts a variable list and name/description. The engine resolves variable names to UIDs internally, matching the praxis pattern where UIDs are the durable identity (not transient loop IDs like "R1").
8. **JSON format detection** -- Content-based detection of native Simlin JSON vs SD-AI JSON format. Both formats roundtrip correctly through read and write.

Out of scope: loop auto-naming via LLM, raw simulation traces in tool responses, Windows atomic write guarantees, MCP resource templates.

## Acceptance Criteria

### mcp-publish-ready.AC1: Atomic file writes
- **AC1.1 Success:** `atomic_write` writes correct content to target path
- **AC1.2 Success:** Temp `.new` file is cleaned up after successful write
- **AC1.3 Success:** Overwrites existing file with new content atomically
- **AC1.4 Failure:** Returns error when parent directory does not exist
- **AC1.5 Edge:** Best-effort parent dir fsync (non-fatal if unsupported)

### mcp-publish-ready.AC2: XMILE write-back
- **AC2.1 Success:** EditModel on `.stmx` writes back to the original file (not `.simlin.json` sidecar)
- **AC2.2 Success:** Edited `.stmx` file is readable by ReadModel with the edit preserved
- **AC2.3 Success:** Output `projectPath` equals input path for all formats
- **AC2.4 Failure:** Dry-run on XMILE does not modify the file
- **AC2.5 Edge:** No `.simlin.json` sidecar file is created

### mcp-publish-ready.AC3: Structured error responses
- **AC3.1 Success:** ReadModel on a model with broken equations returns model snapshot + non-empty `errors` array
- **AC3.2 Success:** Each error has code, message, variable_name, and kind fields
- **AC3.3 Success:** ReadModel on a clean model omits or empties the `errors` field
- **AC3.4 Failure:** EditModel rejects (isError=true) when edit introduces compilation errors
- **AC3.5 Failure:** EditModel error response includes structured error details
- **AC3.6 Failure:** EditModel does not write to disk when edit introduces errors
- **AC3.7 Edge:** Error codes align with pysimlin's ErrorCode enum semantics

### mcp-publish-ready.AC4: Instructions
- **AC4.1 Success:** Initialize response includes non-empty instructions field
- **AC4.2 Success:** Instructions mention ReadModel, EditModel, CreateModel, .mdl limitation, pysimlin
- **AC4.3 Success:** Instructions include loop naming guidance with SetLoopName example
- **AC4.4 Success:** Instructions reference the current pysimlin version
- **AC4.5 Edge:** Version test fails if instructions.md references an outdated pysimlin version

### mcp-publish-ready.AC5: MCP resources (skills)
- **AC5.1 Success:** `resources/list` returns metadata for all four skills
- **AC5.2 Success:** `resources/read` with valid URI returns skill content
- **AC5.3 Failure:** `resources/read` with unknown URI returns error
- **AC5.4 Success:** Skills are compiled into the binary (no runtime file dependency)

### mcp-publish-ready.AC6: Loop naming
- **AC6.1 Success:** SetLoopName with valid variable list creates LoopMetadata with correct UIDs
- **AC6.2 Success:** SetLoopName updates existing named loop (matched by UID set)
- **AC6.3 Failure:** SetLoopName with unknown variable name returns error
- **AC6.4 Success:** Loop names appear in subsequent ReadModel/EditModel loop dominance output

### mcp-publish-ready.AC7: JSON format detection
- **AC7.1 Success:** ReadModel reads native JSON files (top-level `models` key)
- **AC7.2 Success:** ReadModel reads SD-AI JSON files (top-level `variables` key)
- **AC7.3 Success:** EditModel writes back in the same format it read (native stays native, SD-AI stays SD-AI)
- **AC7.4 Failure:** Unrecognized JSON structure returns descriptive error
- **AC7.5 Edge:** `.sd.json` extension works for both native and SD-AI formats

## Glossary

- **MCP (Model Context Protocol)**: An open protocol for connecting AI assistants to external tools and data sources. Simlin-mcp implements an MCP server over stdio JSON-RPC.
- **XMILE**: An XML-based interchange format for system dynamics models (`.xmile`, `.stmx`).
- **SD-AI JSON**: A flat JSON format for SD models with a top-level `variables` key. Contrast with native Simlin JSON which has `models`.
- **Native JSON**: Simlin's own JSON format (`.simlin.json`, `.sd.json`), with a top-level `models` key containing the full project hierarchy.
- **SourceFormat**: New enum (`Xmile | NativeJson | SdaiJson`) tracking the parsed file format for correct write-back.
- **atomic_write**: Write to a temporary sibling file, fsync, rename over target. Prevents partial writes on crash.
- **fsync**: OS call that flushes file data from kernel buffers to durable storage.
- **salsa**: Incremental computation framework used by simlin-engine. Tracked functions re-evaluate only when inputs change.
- **LTM (Loops That Matter)**: Analysis technique identifying which feedback loops dominate model behavior at each simulated time point.
- **LoopMetadata**: Engine data structure storing a named loop's identity: variable UIDs, name, and description.
- **UID**: Stable integer identifier for each model variable. Persists across renames; used as durable identity in LoopMetadata.
- **patch system**: Mechanism in `patch.rs` for applying structured edits (`ModelPatch` with `ModelOperation` variants).
- **TypedTool\<I\>**: MCP server pattern for tool registration where input schema is auto-derived from Rust types via schemars.
- **FormattedError**: Structured error type with code, message, model name, variable name, and kind. Shared by MCP and pysimlin.
- **collect_formatted_errors**: Function that gathers salsa compilation diagnostics and formats them into `FormattedError` instances.
- **include_str!**: Rust macro embedding file contents as a string literal at compile time.
- **MCP resources**: Protocol capability for serving static content that clients can list and read on demand.
- **pysimlin**: Python bindings for the Simlin engine (`pip install simlin`).
- **dry-run**: EditModel mode that validates edits without writing to disk.
- **loop dominance**: Which feedback loop most strongly influences behavior at a given time, computed via LTM.

## Architecture

### Engine layer additions (simlin-engine)

Three new modules in `src/simlin-engine/src/`:

**`io.rs`** -- Reusable atomic file write. `atomic_write(path, contents)` writes to `{path}.new`, calls `sync_all()`, renames over target, then best-effort `sync_all()` on parent directory for durability. Cleans up temp file on error.

**`errors.rs`** -- Error formatting moved from `src/libsimlin/src/errors.rs`. Provides `FormattedError` (code, message, model_name, variable_name, kind) and `collect_formatted_errors(db, sync, project)` which composes `collect_all_diagnostics` with formatting. Both simlin-mcp and libsimlin import from here.

**`patch.rs` extension** -- New `ModelOperation::SetLoopName { variables, name, description }` variant. The `apply_patch` handler canonicalizes variable names, looks up each variable's `uid` field in the model, builds a `LoopMetadata` entry with the UID set, and either updates an existing entry with matching UIDs or appends a new one.

### MCP tool changes

**`open_project` refactor** (`tools/mod.rs`) -- Returns `(Project, SourceFormat)` where `SourceFormat` is `Xmile | NativeJson | SdaiJson`. For JSON files, peeks at top-level keys: `"models"` -> native, `"variables"` -> SD-AI. Unrecognized structure returns a descriptive error.

**EditModel** (`tools/edit_model.rs`):
- Format-aware write-back using `SourceFormat`: XMILE via `to_xmile()`, native JSON via `ejson::Project`, SD-AI via `json_sdai::SdaiModel`. All paths use `atomic_write`.
- After applying a patch: compile the model, collect diagnostics. If any diagnostic has `severity: Error`, return `isError: true` with structured error details. Do not write to disk.
- New `SetLoopName` operation exposed as `EditOperation::SetLoopName(SetLoopNameInput)`.
- Output `project_path` always equals input path (no `.simlin.json` redirect).

**ReadModel** (`tools/read_model.rs`):
- After analysis, collect diagnostics via `collect_formatted_errors`. Add optional `errors` field to `ReadModelOutput`. Tool still succeeds -- agent gets model snapshot + empty loops + the error list explaining why.

### MCP protocol extension

**Resources** in `protocol.rs`:
- `ServerCapabilities` gains a `resources: ResourcesCapability` field.
- Two new dispatch handlers: `resources/list` returns metadata for all skills, `resources/read` returns content by URI.
- Static registry: skills embedded via `include_str!` from `src/simlin-mcp/src/skills/`. URI scheme: `simlin://skills/{name}`.

**Instructions** in `main.rs`:
- `instructions.md` embedded via `include_str!("instructions.md")`. Contains tool workflow, format support, equation syntax basics, modeling conventions, loop dominance basics, loop naming guidance, pysimlin version reference.

### Skill files

Four markdown files in `src/simlin-mcp/src/skills/`:

| File | URI | Content |
|------|-----|---------|
| `pysimlin-basics.md` | `simlin://skills/pysimlin-basics` | Loading models, running simulations, DataFrame access, matplotlib basics, error handling |
| `scenario-analysis.md` | `simlin://skills/scenario-analysis` | Parameter sweeps with overrides, intervention analysis via simulate() context manager, comparing scenarios |
| `loop-dominance.md` | `simlin://skills/loop-dominance` | Plotting behavior_time_series, annotating dominant_periods on charts, interpreting importance values |
| `vensim-equation-syntax.md` | `simlin://skills/vensim-equation-syntax` | Vensim-specific names (SMOOTH/DELAY/ZIDZ/XIDZ), :AND:/:OR:/:NOT:, IF THEN ELSE() function form, subscript bang syntax, complete MDL-to-XMILE mapping table |

## Existing Patterns

**Tool registration** -- Tools use `TypedTool<I>` where `I: Deserialize + JsonSchema`. Schema is auto-derived via schemars. This pattern continues for new operations (SetLoopName input type).

**Patch system** -- `ModelOperation` enum in `patch.rs` with `apply_patch` handler. SetLoopName follows the same pattern as existing operations (UpsertStock, DeleteVariable, etc.).

**Error types** -- `common.rs` defines `ErrorCode` with 100+ variants. `db.rs` has `Diagnostic` with `DiagnosticSeverity`. `libsimlin/src/errors.rs` has `FormattedError` and `format_diagnostic`. This design moves the formatting into simlin-engine, keeping the same types and logic.

**Protocol dispatch** -- `protocol.rs` matches on method strings and dispatches to handler functions. Resources handlers follow the same pattern as existing `tools/list` and `tools/call`.

**File embedding** -- No existing pattern in simlin-mcp, but `include_str!` is used elsewhere in the engine (e.g., `stdlib.gen.rs`). Skills follow this approach.

**Test helpers** -- Tests use `tempfile::tempdir()` for isolation and a `call_tool` helper. New tests follow this pattern.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Engine infrastructure

**Goal:** Add atomic write utility and move error formatting into simlin-engine.

**Components:**
- `src/simlin-engine/src/io.rs` -- new module with `atomic_write(path, contents)` function
- `src/simlin-engine/src/errors.rs` -- new module with `FormattedError`, `FormattedErrorKind`, `format_diagnostic`, `collect_formatted_errors`
- `src/simlin-engine/src/lib.rs` -- add `pub mod io;` and `pub mod errors;`
- `src/libsimlin/src/errors.rs` -- refactor to import from simlin-engine instead of defining locally

**Dependencies:** None (first phase)

**Done when:** Engine tests pass for atomic write (correct content, temp cleanup, error on missing parent, overwrites). Error formatting produces same output as before. libsimlin tests still pass after refactor.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: SetLoopName engine patch

**Goal:** Add a patch operation for naming feedback loops by their variable list.

**Components:**
- `src/simlin-engine/src/patch.rs` -- new `ModelOperation::SetLoopName { variables, name, description }` variant with apply logic (canonicalize names, resolve UIDs, match/update LoopMetadata)
- `src/simlin-engine/src/datamodel.rs` -- no changes needed (LoopMetadata already exists)

**Dependencies:** None (parallel with Phase 1)

**Done when:** Patch tests verify: naming a loop by variable list creates correct LoopMetadata with resolved UIDs, updating an existing named loop replaces name/description, variables with missing UIDs are handled gracefully.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: JSON format detection and SD-AI roundtrip

**Goal:** Content-based detection of native vs SD-AI JSON format with correct roundtrip for both.

**Components:**
- `src/simlin-mcp/src/tools/mod.rs` -- refactor `open_project` to return `(Project, SourceFormat)`, add content-based JSON detection (peek at `"models"` vs `"variables"` keys)
- `src/simlin-mcp/src/tools/edit_model.rs` -- thread `SourceFormat` through to the write path
- `src/simlin-mcp/src/tools/read_model.rs` -- update to use new `open_project` signature (ignore SourceFormat)

**Dependencies:** None (parallel with Phases 1-2)

**Done when:** ReadModel can read SD-AI format files. EditModel can read and write back SD-AI format (file retains `"variables"` structure after edit). Unrecognized JSON returns descriptive error. Existing native JSON tests still pass.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: EditModel overhaul

**Goal:** Format-aware write-back (XMILE, native JSON, SD-AI) with atomic writes and structured error checking.

**Components:**
- `src/simlin-mcp/src/tools/edit_model.rs`:
  - Remove `write_path` redirect logic, deduplicate `ext` computation
  - Format-aware serialization: XMILE via `to_xmile()`, NativeJson via `ejson::Project`, SdaiJson via `json_sdai::SdaiModel`
  - All write paths use `simlin_engine::io::atomic_write`
  - Post-patch diagnostic collection via `collect_formatted_errors`. If errors exist, return `isError: true` with structured details, don't write.
  - New `EditOperation::SetLoopName(SetLoopNameInput)` variant with `variables: Vec<String>`, `name: String`, `description: Option<String>`
  - Update `EditModelOutput.project_path` doc comment
- `src/simlin-mcp/src/tools/types.rs` -- add `ErrorOutput` serialization type

**Dependencies:** Phases 1 (atomic write, error formatting), 2 (SetLoopName patch), 3 (SourceFormat)

**Done when:** XMILE files are written back to original path (roundtrip test: edit then re-read). Atomic write leaves no `.new` temp files. EditModel rejects edits that introduce compilation errors with structured error details. SetLoopName operation works end-to-end. Existing tests updated (no `.simlin.json` redirect).
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: ReadModel structured errors

**Goal:** Surface compilation diagnostics in ReadModel responses.

**Components:**
- `src/simlin-mcp/src/tools/read_model.rs` -- add optional `errors: Vec<ErrorOutput>` field to `ReadModelOutput`, collect diagnostics via `collect_formatted_errors` after analysis
- `src/simlin-mcp/src/tools/types.rs` -- reuse `ErrorOutput` from Phase 4

**Dependencies:** Phase 1 (error formatting), Phase 3 (open_project refactor)

**Done when:** ReadModel on a model with broken equations returns model snapshot + non-empty `errors` array with correct error codes and variable names. ReadModel on a clean model returns no `errors` field (or empty). Existing ReadModel tests still pass.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: MCP resources protocol

**Goal:** Extend the MCP server to serve static resources via `resources/list` and `resources/read`.

**Components:**
- `src/simlin-mcp/src/protocol.rs`:
  - `ResourcesCapability` struct added to `ServerCapabilities`
  - `Resource` metadata type (uri, name, description, mimeType)
  - `ListResourcesResult` and `ReadResourceResult` response types
  - `handle_resources_list` and `handle_resources_read` dispatch handlers
- `src/simlin-mcp/src/resource.rs` -- new module with static resource registry mapping URIs to embedded content via `include_str!`
- `src/simlin-mcp/src/main.rs` -- register resource module

**Dependencies:** None (parallel with other phases)

**Done when:** MCP `resources/list` returns metadata for all registered resources. `resources/read` with a valid URI returns the skill content. `resources/read` with an unknown URI returns an error. Protocol tests verify JSON-RPC roundtrip.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: Instructions and skills content

**Goal:** Write the instructions and four skill files, embed them in the binary.

**Components:**
- `src/simlin-mcp/src/instructions.md` -- tool workflow, format support, equation syntax basics, modeling conventions, loop dominance basics, loop naming guidance with example, error field explanation, pysimlin version reference
- `src/simlin-mcp/src/skills/pysimlin-basics.md` -- loading models, running simulations, DataFrame access, matplotlib, error handling
- `src/simlin-mcp/src/skills/scenario-analysis.md` -- parameter sweeps, interventions, sensitivity analysis
- `src/simlin-mcp/src/skills/loop-dominance.md` -- plotting importance over time, annotating dominant periods
- `src/simlin-mcp/src/skills/vensim-equation-syntax.md` -- complete Vensim-to-XMILE mapping, grounded in the MDL parser/writer code
- `src/simlin-mcp/src/main.rs` -- replace placeholder instructions with `include_str!("instructions.md")`
- `src/simlin-mcp/src/resource.rs` -- register four skills with URIs and metadata
- Version test: shells out to `git tag --list 'pysimlin-v*' --sort=-v:refname` to find latest tag, asserts `instructions.md` contains that version

**Dependencies:** Phase 6 (resources protocol for serving skills)

**Done when:** Initialize response includes comprehensive instructions. `resources/list` returns four skills. `resources/read` returns each skill's content. Version test passes. Instructions content is grounded in the codebase (equation syntax verified against builtins.rs/mdl/builtins.rs).
<!-- END_PHASE_7 -->

## Additional Considerations

**EditModel error semantics:** The compile-then-check approach means the patch is applied in memory before diagnostics are collected. If the patch introduces errors, the tool returns `isError: true` and the file is not written. This matches praxis behavior where edits are atomic -- either the whole edit succeeds cleanly or nothing changes on disk.

**SourceFormat tracking:** `SourceFormat` is determined at parse time and threaded through to the write path. For dry-run calls, `SourceFormat` is still computed (for the output `projectPath`) but no write occurs. CreateModel always writes native JSON (`.simlin.json`) and does not need format detection.

**SD-AI format limitations:** The SD-AI format is single-model (flat variable list). Converting a multi-model native project to SD-AI drops all models except the first. Module variables are silently dropped. The instructions should note that `.sd.json` files support both formats.

**Vensim equation syntax skill accuracy:** The syntax reference must be grounded in the actual parser and writer code (`src/simlin-engine/src/mdl/builtins.rs`, `mdl/writer.rs`, `mdl/xmile_compat.rs`), not in external documentation or LLM knowledge. The mapping table between Vensim and XMILE function names is derived from `xmile_to_mdl_function_name()` and `format_function_name()`.

**Audience breadth:** Instructions and skills must be model-agnostic (not Claude-specific). Avoid assumptions about specific tool-calling syntax or capabilities. The content works for Claude (Code + Cowork), Codex, and Gemini.
