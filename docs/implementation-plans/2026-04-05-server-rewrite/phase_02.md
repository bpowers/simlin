# Phase 2: Save and Write-Back Implementation Plan

**Goal:** Add the write path so a user can edit a model in the browser and have the result land on disk in the appropriate format. `.stmx`/`.xmile` files are overwritten atomically with regenerated XMILE; `.mdl` files are never modified — edits land in a sibling `<basename>.sd.json` sidecar that becomes the new source of truth. Optimistic version checks prevent stale overwrites; invalid edits are rejected without touching disk.

**Architecture:** A new `POST /api/projects/{*path}` endpoint takes `{ json: string, version: u64 }`. The handler holds the `ProjectRegistry` write lock across the version check + version increment so that concurrent saves cannot both win. Inside the locked critical section it parses the incoming JSON via `serde_json::from_str::<json::Project>`, converts to `datamodel::Project`, runs simlin-engine's salsa-based diagnostic check (using a pre-edit baseline so saves that *fix* existing errors are not blocked), and only then writes via `simlin_engine::io::atomic_write`. The frontend `<EditorHost>` drops `embedded` / `readOnlyMode` from Phase 1 and wires the real `onSave` to the new endpoint, handling `409 Conflict` by refetching and presenting the latest state.

**Tech Stack:** Reuses everything from Phase 1 — no new dependencies. New surface: `simlin_engine::to_xmile`, `simlin_engine::io::atomic_write`, `simlin_engine::db::SimlinDb` + `sync_from_datamodel` + `collect_all_diagnostics`, `simlin_engine::errors::collect_formatted_errors`. Frontend: existing `<Editor>` `onSave` callback semantics (`inSave`/`saveQueued` debouncing already implemented inside Editor.tsx).

**Scope:** Phase 2 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC3: Editing round-trip
- **server-rewrite.AC3.2 Success:** Saving an edit to a `.stmx`/`.xmile` file writes the new content back to the original file in XMILE format
- **server-rewrite.AC3.4 Success:** Saving an edit to a `.mdl` file writes a `<name>.sd.json` sidecar in the same directory; the original `.mdl` is never modified
- **server-rewrite.AC3.5 Success:** Re-opening a `.mdl` after edits prefers `<name>.sd.json` over the `.mdl` (the sidecar is the new source of truth once it exists) *(Phase 1 implemented the read-side preference; Phase 2 verifies it end-to-end after a save)*
- **server-rewrite.AC3.6 Failure:** Stale-version save (optimistic lock) returns 409 Conflict and the browser refetches

This phase also closes out the Phase 1 partial coverage of:
- **server-rewrite.AC3.1 Success:** Opening a `.stmx` or `.xmile` file in the browser displays the model and **allows editing** *(now fully covered)*
- **server-rewrite.AC3.3 Success:** Opening a `.mdl` file in the browser parses the file via xmutil and displays the model *(remains delivered; the editor now also accepts edits, which land in the sidecar)*

---

## Notes for Executor

The Phase 2 codebase investigation produced several findings that change naive readings of the design. Read these before implementing:

**1. `simlin_engine::io::atomic_write` is the right primitive.** Signature: `pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()>` at `/home/bpowers/src/simlin/src/simlin-engine/src/io.rs:21`. Already used by `simlin-mcp::EditModel` (`/home/bpowers/src/simlin/src/simlin-mcp/src/tools/edit_model.rs:341`). Writes to `{path}.new`, fsyncs, renames over the target, fsyncs the parent directory. **Windows caveat (documented in source at lines 45-50):** because `std::fs::rename` does not atomically replace an existing file on Windows, the implementation calls `fs::remove_file(target)` first, leaving a tiny window where neither file exists. Truly atomic Windows replacement requires `MoveFileExW`, which std does not expose. This is a known gap and acceptable for a single-user local tool.

**2. `to_xmile` is byte-stable for round-trips.** Signature: `pub fn to_xmile(project: &Project) -> Result<String>` at `/home/bpowers/src/simlin/src/simlin-engine/src/compat.rs:32`. The XMILE writer (`xmile::project_to_xmile` at `src/simlin-engine/src/xmile/mod.rs:802`) uses fixed 4-space indentation, sorts variables by canonical identifier on the read path, and has no `HashMap` iteration in the write path. **Existing test asserts byte-equality:** `tests/simulate.rs:299-300` runs `XMILE → datamodel → XMILE` and checks `assert_eq!(&serialized_xmile, &serialized_xmile2)` against the entire `TEST_MODELS` list. This phase relies on that property and adds a similar assertion for the JSON→XMILE side specifically.

**3. Validation: do NOT use `analyze_model` — it includes LTM analysis we don't need.** The validation-only path that simlin-mcp uses is:
```rust
let db = simlin_engine::db::SimlinDb::default();
let sync = simlin_engine::db::sync_from_datamodel(&db, &project);
let diagnostics = simlin_engine::db::collect_all_diagnostics(&db, &sync);
let formatted = simlin_engine::errors::collect_formatted_errors(
    diagnostics.iter().filter(|d| matches!(d.severity, DiagnosticSeverity::Error)),
    &project,
);
```
Phase 2 mirrors this. `collect_formatted_errors` lives at `/home/bpowers/src/simlin/src/simlin-engine/src/errors.rs:288`.

**4. Pre-edit baseline matters.** `simlin-mcp::EditModel` (`edit_model.rs:240-305`) builds a baseline of pre-edit error keys `(error_code, variable_name)` and only rejects post-edit errors that are *new* (not in the baseline). This means a save that fixes some errors but introduces no new ones is accepted. Phase 2 must replicate this — without it, any project that opens with errors (and there are many real-world models that do) becomes uneditable.

**5. `<Editor>`'s `onSave` semantics in detail** (verified in `src/diagram/Editor.tsx:208-489`):
- `JsonProjectData` shape (lines 208-211): `{ format: 'json'; data: string }` where `data` is the result of `engine.serializeJson()`.
- `onSave` returns `Promise<number | undefined>`. Returning a number updates `state.projectVersion`; returning `undefined` is treated as "no version update" but not an error; throwing is an explicit error and is appended to `modelErrors`.
- The `inSave`/`saveQueued` pattern (lines 452-489) coalesces concurrent edits — saves are never lost, but the POST body always reflects the latest model state (re-serialized at flush time, not at queue time).
- `currVersion` is `toInt(projectVersion)` — the floored integer part. The Editor uses `0.01`/`0.001` increments to force React re-renders for view-only updates without bumping the server-known version.

**6. Drop both `embedded` and `readOnlyMode` for Phase 2.** Phase 1's `<EditorHost>` passed both. Phase 2 unblocks editing by removing them entirely (the no-op `onSave` is replaced with the real one).

**7. JSON pipeline pitfalls.**
- `From<json::Project> for datamodel::Project` at `/home/bpowers/src/simlin/src/simlin-engine/src/json.rs:1300` — consuming conversion.
- The reverse `From<&datamodel::Project> for json::Project` at line 1972 is what produces the JSON for the GET path (Phase 1).
- `ai_information` is silently dropped: not present in `json::Project` at all, so any AI-signing metadata in the original `.stmx` is lost on the first round-trip through the editor. This already happens in `@simlin/mcp::EditModel` and is acceptable.
- `json::Project`'s schema is camelCase (`#[serde(rename_all = "camelCase")]`); `engine.serializeJson()` produces it; `serde_json::from_str::<json::Project>` consumes it. No naming mismatch.

**8. Optimistic locking primitive.** No prior pattern exists in the workspace. Use `std::sync::RwLock` (the Phase 1 plan settled on this since `parking_lot` is only a transitive dep). Hold the **write lock** for the entire `(read existing version → check incoming version → on match, increment + write metadata)` sequence; this is brief enough that it won't starve readers. The actual file I/O happens **after** the locked section: parse, validate, then atomic_write outside the lock. Reads of the in-memory state during write (e.g., another GET arriving) return the pre-write metadata and pre-write JSON, which is fine — once the write commits, a subsequent GET sees the new state.

**9. Sidecar write is unprecedented in the workspace.** Phase 2 introduces this. Decision rules:
- For a `.mdl`-backed entry without a sidecar yet: save creates `<basename>.sd.json` next to the `.mdl`; the registry entry's `path` is updated from the `.mdl` to the `.sd.json` (so subsequent GETs return the sidecar; the `.mdl` is dropped from the registry). This matches the design's "sidecar becomes the new source of truth once it exists."
- For an entry already pointing at a `.sd.json` (whether discovered as a standalone or created by an earlier save): save writes back to that `.sd.json` directly, no `.mdl`-related logic.
- Atomicity: use `simlin_engine::io::atomic_write` for both the sidecar and any in-place `.stmx`/`.xmile` overwrite.

**10. `simlin-mcp` rejects `.mdl` writes — do NOT copy that behavior.** The exact rejection text (from `src/simlin-mcp/src/tools/edit_model.rs:219-222`) is "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, then CreateModel to start a new .sd.json file you can edit." Our path is different: we transparently write the sidecar.

**11. Test fixture choice.** Use `test/test-models/samples/teacup/teacup.xmile` (3.9 KB) for round-trip tests — already used by `simlin-engine`'s existing `json_roundtrip.rs` tests. For `.mdl` tests, use `test/sdeverywhere/models/comments/comments.mdl` (960 bytes, smallest); construct the expected sidecar JSON inline by parsing through the engine.

**12. Body limit bump.** Phase 1 set `RequestBodyLimitLayer::new(4 * 1024 * 1024)` (4 MiB) — fine for reads but POST bodies will be larger (full canonical JSON of a model). Bump to 16 MiB for Phase 2; revisit if real models exceed it.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
### Subcomponent A: POST endpoint with version check + validation gate

<!-- START_TASK_1 -->
### Task 1: Save request/response types and route registration

**Verifies:** none directly (scaffolding for AC3.2, AC3.4, AC3.6)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (add `SaveRequest`, `SaveResponse`, `SaveError` types; register the new route in `build_router` (in `lib.rs`))
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (add `.route("/api/projects/{*path}", post(save_project))` alongside the existing `get(get_project)` mapping using `axum::routing::get_post` or two separate `.route` calls — Axum 0.8 routes by method, so the same path string accepts both HTTP methods on the same handler entry)

**Implementation:**
- `pub struct SaveRequest { pub json: String, pub version: u64 }` with `#[derive(Deserialize)]`. The `json` field is the raw stringified JSON the Editor produced from `engine.serializeJson()`.
- `pub struct SaveResponse { pub version: u64, pub path: String }` with `#[derive(Serialize)]`. The `path` is the relative path to the file we actually wrote (relevant for `.mdl` saves, where the response path differs from the request path because we redirect to the sidecar).
- `SaveError` enum: `VersionMismatch { expected: u64, actual: u64 }` → `409 Conflict`, `BadRequest(String)` → `400`, `Forbidden` → `403`, `Validation { errors: Vec<ValidationError> }` → `422 Unprocessable Entity`, `Internal(anyhow::Error)` → `500`. Implement `IntoResponse` so each variant carries a JSON body with `{ error: "...", details: ... }`.
- `ValidationError` carries `{ code: String, message: String, model_name: Option<String>, variable_name: Option<String>, kind: String }` (mirror `simlin-mcp::tools::types::ErrorOutput` field-for-field; do NOT import that crate — copy the structure to keep crate boundaries clean).
- Handler signature: `pub async fn save_project(State(state): State<AppState>, Path(rel_path): Path<String>, Json(body): Json<SaveRequest>) -> Result<Json<SaveResponse>, SaveError>`. Implementation in subsequent tasks; this task lands a stub returning `SaveError::Internal(anyhow::anyhow!("not yet implemented"))` and the route registration.

**Testing:**
- Inline test in `handlers.rs`: `SaveError::VersionMismatch { expected: 1, actual: 0 }.into_response().status() == StatusCode::CONFLICT`. Same pattern for each variant.
- Round-trip JSON: `serde_json::to_string(&SaveRequest { json: "{}".into(), version: 1 })` and parse it back.

**Verification:**
- `cargo test -p simlin-serve handlers::` passes.

**Commit:** `serve: save request/response types and POST route stub`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Optimistic version check with `RwLock` write hold

**Verifies:** server-rewrite.AC3.6

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add `pub fn check_and_increment(&self, abs_path: &Path, expected_version: u64) -> Result<u64, RegistryError>`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (use `check_and_increment` before any write)

**Implementation:**
- `RegistryError` enum: `NotFound`, `VersionMismatch { expected: u64, actual: u64 }`.
- `check_and_increment` acquires `self.0.write()` (a `std::sync::RwLockWriteGuard`), looks up the entry, compares `expected_version` to `entry.version`, returns `Err(VersionMismatch)` on miss, otherwise increments `entry.version += 1` and returns the new version.
- The handler calls `check_and_increment` early, before any file I/O. On error, maps to `SaveError::VersionMismatch` (returning the actual version so the frontend knows what to refetch against).
- Caveat: We hold the write lock through the increment but **release it before the actual file I/O**. This means another concurrent save will find the new version (incremented) and either match it (rare, both sides racing toward the same target) or 409. The file write that happens after the lock release is sequenced by the lock's increment, so even if two writes hit disk in opposite order, the final-version-wins property holds in the registry. Document this in a comment in `check_and_increment`.

**Testing:**
- Unit test: insert one ProjectMeta with `version = 5`. Call `check_and_increment(path, 5)` → returns `Ok(6)`, registry now reflects `version = 6`. Second call with `version = 5` → returns `Err(VersionMismatch { expected: 5, actual: 6 })`.
- Concurrency test: spawn two threads both calling `check_and_increment(path, 5)`. Exactly one must return `Ok`, the other `Err(VersionMismatch)`. Use `std::thread::spawn` and a `Barrier` to synchronize starts.

**Verification:**
- `cargo test -p simlin-serve registry::` passes including the new tests.

**Commit:** `serve: ProjectRegistry check_and_increment for optimistic locking`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: JSON parse + validation gate (no disk write yet)

**Verifies:** none directly (validation pre-write)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/validation.rs`
- Modify: `lib.rs` (re-export `pub mod validation;`)
- Modify: `Cargo.toml` (`simlin-engine` is already a dep from Phase 1; no new deps)
- Test: inline `#[cfg(test)] mod tests` in `validation.rs`

**Implementation:**
- `pub struct ValidationOutcome { pub project: simlin_engine::datamodel::Project, pub new_errors: Vec<ValidationError> }`.
- `pub fn validate_save(json: &str, baseline: &BaselineErrors) -> Result<ValidationOutcome, ValidationFailure>`:
  1. `let json_project: simlin_engine::json::Project = serde_json::from_str(json).map_err(ValidationFailure::JsonParse)?`
  2. `let project: datamodel::Project = json_project.into()`
  3. Build a `SimlinDb::default()`, call `sync_from_datamodel(&db, &project)`, collect diagnostics via `collect_all_diagnostics(&db, &sync)`, filter to `severity == Error`, format via `collect_formatted_errors(...)`.
  4. Compute `(code, variable_name)` keys for each formatted error; subtract `baseline.keys` to get *new* errors.
  5. If new errors empty, return `Ok(ValidationOutcome { project, new_errors: vec![] })`.
  6. Otherwise return `Ok(ValidationOutcome { project, new_errors: <new> })`. The handler decides whether to gate on this — saves with new errors are rejected; saves with no new errors proceed.
- `pub struct BaselineErrors { pub keys: HashSet<(String, Option<String>)> }`.
- `pub fn compute_baseline(project: &datamodel::Project) -> BaselineErrors`: same pipeline as steps 3-4 but starting from a known-current project. Used to capture the pre-edit error set.

**Where to call `compute_baseline`:** The save handler captures the baseline by reading the **current on-disk state** at the start of the request, parsing it, and computing errors. This is necessary because the registry only stores metadata, not the parsed project. Trade-off: we re-parse on every save. For Phase 2 this is fine; Phase 3's Loro doc cache will eliminate the re-parse.

**Testing:**
Verifies the validation pipeline works as expected.
- Construct a tiny `json::Project` via the public API (or load `test/test-models/samples/teacup/teacup.xmile`, parse to `datamodel::Project`, convert to `json::Project`, serialize). Validate it — expect `new_errors: []`.
- Mutate the JSON to introduce an error (e.g., reference an undefined identifier in an equation) — validate against an empty baseline → expect `new_errors` non-empty.
- Same mutated JSON, but baseline contains the same error → `new_errors` empty (the save would be accepted because it didn't introduce a new error).

**Verification:**
- `cargo test -p simlin-serve validation::` passes.

**Commit:** `serve: validation gate using simlin-engine diagnostics + baseline`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Wire validation into the save handler (still no disk write)

**Verifies:** none directly (composition for AC3.2, AC3.4)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (`save_project` now: version-check → read current file → compute baseline → validate incoming → if OK, return a stub success without writing yet (Task 5/6 wire the actual write))

**Implementation:**
- After version check passes, the handler:
  1. Resolves the absolute path from the relative path (reuse the path-canonicalization + traversal-safety from Phase 1's GET handler — extract into a helper if needed).
  2. Reads the current file (as in the GET handler), parses to `datamodel::Project`, calls `validation::compute_baseline(&current_project)`.
  3. Calls `validation::validate_save(&body.json, &baseline)`.
  4. If `validate_save` returns `JsonParse` failure → `SaveError::BadRequest`.
  5. If `new_errors` non-empty → `SaveError::Validation { errors: new_errors }`. Return `422 Unprocessable Entity`.
  6. If clean → return a stub `SaveResponse { version: <new from registry>, path: <relative path> }`. The actual file write is added in Task 5/6.

**Testing:**
- New integration test in `tests/api_save.rs`:
  - Setup: tempdir with one `.stmx` fixture, registry seeded with `version: 0`.
  - POST a valid edit (parse fixture, mutate trivially, re-serialize) with `version: 0` → expect `200` and response `version: 1`.
  - POST same body with `version: 0` again → expect `409 Conflict` (the registry incremented to 1).
  - POST a body with `version: 0` (after re-reading version 1 from a GET) but with intentionally invalid JSON (e.g., reference to a nonexistent variable) → expect `422` with `errors[]` populated.
  - Note: the file on disk is **not yet written** at this stage; tests verify the file's mtime/contents are unchanged.

**Verification:**
- `cargo test -p simlin-serve --test api_save` passes the version-check + validation portion.

**Commit:** `serve: save handler wiring for version check and validation`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 5-7) -->
### Subcomponent B: Format-aware write paths (XMILE in-place, .sd.json sidecar)

<!-- START_TASK_5 -->
### Task 5: XMILE write-back for `.stmx`/`.xmile`

**Verifies:** server-rewrite.AC3.2

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/writer.rs`
- Modify: `lib.rs` (re-export)
- Modify: `handlers.rs` (call `writer::save_to_disk` from inside `save_project`)
- Test: inline `#[cfg(test)] mod tests` in `writer.rs`
- Test: extend `tests/api_save.rs` with an end-to-end XMILE-write test

**Implementation:**
- `pub enum SaveTarget { InPlaceXmile(PathBuf), SidecarJson { mdl_path: PathBuf, sidecar_path: PathBuf } }`.
- `pub fn resolve_save_target(absolute_path: &Path, source_format: ProjectFormat) -> SaveTarget`:
  - `Stmx` or `Xmile` → `InPlaceXmile(absolute_path.to_path_buf())`
  - `Mdl` → `SidecarJson { mdl_path, sidecar_path: <sibling .sd.json> }` (Task 6)
  - `SdJson` → `InPlaceXmile`-like (write to the same path) but as JSON bytes — handled in Task 6
- `pub fn save_to_disk(project: &datamodel::Project, target: &SaveTarget) -> Result<PathBuf, SaveDiskError>`:
  - For `InPlaceXmile`: serialize via `simlin_engine::to_xmile(project)?`, write via `simlin_engine::io::atomic_write(path, xmile_string.as_bytes())?`. Return the path.
- The `SaveDiskError` wraps `simlin_engine`'s error type and `io::Error` with the path that failed.
- The handler's `save_project`, after validation, calls `writer::resolve_save_target(...)` then `writer::save_to_disk(&validated.project, &target)`. On success, updates the registry's `mtime` for the entry to `fs::metadata(...).modified()?`.

**Testing:**
Verifies AC3.2.
- Unit test: round-trip a fixture through `save_to_disk` to a tempdir path, re-read, parse, assert structural equality with the input project (compare the serialized JSON of both for simplicity).
- Unit test (byte-stability): save the same project twice to two tempdir paths; assert the bytes are identical (this exercises the design's "byte-stable for semantically identical input" hardening goal — fail loudly if a regression slips in).
- Integration test in `tests/api_save.rs`: POST a valid edit to an `.stmx` fixture; assert the file on disk has been overwritten and parses back to the new project.

**Verification:**
- `cargo test -p simlin-serve writer::` and `--test api_save` pass.

**Commit:** `serve: XMILE in-place write via to_xmile + atomic_write`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: `.sd.json` sidecar write for `.mdl`-backed entries

**Verifies:** server-rewrite.AC3.4, server-rewrite.AC3.5

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/writer.rs` (add the `SidecarJson` arm and the `SdJson` write path)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add `pub fn redirect_to_sidecar(&self, mdl_path: &Path, sidecar_path: PathBuf) -> Result<(), RegistryError>`)
- Modify: `handlers.rs` (after a sidecar write, call `redirect_to_sidecar` so the registry now points at the `.sd.json` and the original `.mdl` entry is dropped)
- Test: extend `writer.rs` and `tests/api_save.rs`

**Implementation:**
- `save_to_disk`'s `SidecarJson` arm:
  1. Serialize `&datamodel::Project` to a `json::Project` via `simlin_engine::json::Project::from(project)`.
  2. `serde_json::to_string_pretty(&json_project)?` (pretty-printed for git-friendliness; an option to switch to compact later if file size matters).
  3. `simlin_engine::io::atomic_write(&sidecar_path, json_str.as_bytes())?`.
  4. The `.mdl` file is NOT touched.
  5. Return `sidecar_path`.
- `save_to_disk`'s `SdJson` arm: same as `SidecarJson` but the path is the existing `.sd.json` itself (no `.mdl` involved).
- `redirect_to_sidecar`: under the registry write lock, look up the `.mdl` entry, create a new `ProjectMeta` for the sidecar path (format = `SdJson`, `version` carried over from the `.mdl` entry), insert at the sidecar path key, remove the `.mdl` entry.
- The `path` field returned in `SaveResponse` is the relative path to the **sidecar** (so the frontend updates its URL/state to point at the new location).

**Testing:**
Verifies AC3.4, AC3.5.
- Unit test: save to a `.mdl`-backed `SaveTarget`. Verify (a) `.sd.json` exists with the JSON content, (b) the original `.mdl` is byte-identical to before (use `fs::read` before and after, assert equality).
- Integration test in `tests/api_save.rs`: setup tempdir with a `.mdl` fixture, GET it (returns version 0, format `mdl`), POST an edit (mutate trivially, send back). Expect `200` with `path: <sidecar relative path>` and `source_format: "sd_json"` if reused.
- AC3.5 check: after the POST in the previous step, GET the original `.mdl` path → expect 200 with content matching the sidecar (Phase 1's read handler already implements the sidecar-preference rule, so this should "just work"). Equivalently, GET the sidecar's path directly → same content.
- Idempotence: after a sidecar exists, a second POST against either the `.mdl` path or the sidecar path writes only to the sidecar.

**Verification:**
- `cargo test -p simlin-serve --test api_save` passes the new sidecar tests.

**Commit:** `serve: .sd.json sidecar write for .mdl-backed entries`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Body limit bump and metadata refresh after save

**Verifies:** none directly (operational tightening)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (bump `RequestBodyLimitLayer::new(16 * 1024 * 1024)`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (`save_project` updates `mtime` and `size` on the registry entry after a successful disk write)
- Test: extend `tests/api_save.rs` with a "metadata is refreshed" check

**Implementation:**
- After `save_to_disk` returns, fetch `fs::metadata(&path)?.modified()?` and `len()`, then call a new `ProjectRegistry::refresh_meta(path, mtime, size)` helper (under the write lock).
- The body-limit bump is a one-liner.

**Testing:**
- After a successful POST, the registry's snapshot for that path shows updated `mtime` (newer than the pre-save mtime) and updated `size` (matches `fs::metadata(...).len()`).

**Verification:**
- `cargo test -p simlin-serve --test api_save` passes the new check.

**Commit:** `serve: bump body limit to 16 MiB; refresh registry meta on save`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 8-10) -->
### Subcomponent C: Frontend save wiring + 409 handling

<!-- START_TASK_8 -->
### Task 8: Drop `embedded`/`readOnlyMode`, wire real `onSave`

**Verifies:** server-rewrite.AC3.1 (full), server-rewrite.AC3.2 (UI surface), server-rewrite.AC3.3 (full), server-rewrite.AC3.4 (UI surface)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/api.ts` (add `saveProject(path: string, json: string, version: number): Promise<SaveResponse>`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx`

**Implementation:**
- `<EditorHost>` no longer passes `embedded={true}` or `readOnlyMode={true}` (the props default to false/undefined). The no-op `onSave` is replaced with:
  ```typescript
  onSave={async (project, currVersion) => {
    if (project.format !== 'json') return undefined;
    const result = await api.saveProject(this.props.path, project.data, currVersion);
    if (result.path !== this.props.path) {
      // Sidecar redirect: update our path state to the new location
      this.props.onPathRedirect?.(result.path);
    }
    return result.version;
  }}
  ```
- The optional `onPathRedirect` callback bubbles up to `App` so the active `selectedPath` state and the UI list are updated when a `.mdl` save creates a sidecar. (Without this, the UI keeps thinking it's editing the `.mdl` path even though saves are landing in the sidecar — works but confuses the user.)
- `api.ts::saveProject(path, json, version)` POSTs to `/api/projects/<encoded path>` with `{ json, version }` body and `Authorization: Bearer <token>`. On 409, throws a `VersionConflictError` with `actualVersion`. On 422, throws a `ValidationError` with the error list. On 200, returns `{ version, path }`.

**Testing:**
- Jest test: render `<EditorHost path="teacup.stmx">`, mock `fetch` for the GET (returning a project), mock `fetch` for the POST (returning `{ version: 1, path: 'teacup.stmx' }`). Trigger a save (call the captured `onSave` callback with a fake `JsonProjectData` and `currVersion: 0`) → assert the POST was called with the right body, returned promise resolves to `1`.
- Jest test for `.mdl` sidecar redirect: GET returns format `"mdl"`, save POST returns `{ version: 1, path: 'population.sd.json' }` → assert `onPathRedirect` was called with `'population.sd.json'`.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes new tests.
- Manual: run the binary against a directory with an `.stmx`, edit a variable in the browser, observe the file on disk updates within 1-2 seconds.

**Commit:** `serve: EditorHost wires real onSave with sidecar redirect`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: 409 Conflict handling — refetch + present-current-state

**Verifies:** server-rewrite.AC3.6 (UI surface)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/api.ts` (`saveProject` already throws `VersionConflictError` per Task 8)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Test: extend `EditorHost.test.tsx`

**Implementation:**
- `<EditorHost>`'s `onSave` callback catches `VersionConflictError`, refetches `GET /api/projects/<path>` to obtain the latest `{ json, version }`, then calls a parent-supplied `onConflict(latestJson, latestVersion)` which is responsible for re-rendering the Editor with the latest server state. Throws a friendly error to the Editor so the user sees a "your edit conflicted with another save; the latest version has been loaded — please re-apply your changes" toast (added to `modelErrors` automatically by the Editor's `onSave` error path).
- This implementation does not yet auto-merge — the user re-applies their edit. Phase 3's Loro merge primitive will replace this with proper merging.

**Testing:**
- Jest test: first POST returns 409. Second fetch (GET) returns the latest `{ json, version: 5 }`. Verify the parent's `onConflict` callback was called with that data and that the EditorHost re-renders with the new initial state.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: 409 conflict handling refetches and renders latest state`
<!-- END_TASK_9 -->

<!-- START_TASK_10 -->
### Task 10: Validation-error UI surface (422 path)

**Verifies:** server-rewrite.AC3.2 / AC3.4 negative paths (validation errors don't write to disk)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Test: extend `EditorHost.test.tsx`

**Implementation:**
- When `saveProject` throws a `ValidationError`, surface the error list to the user via the Editor's existing `modelErrors` mechanism: throw an `Error` with a formatted message ("Save failed: <error code>: <variable>: <message>" for each new error). The Editor's `onSave` error catch (`Editor.tsx:471-476`) appends the message to `modelErrors`, which surfaces it as a toast.
- For Phase 2 we don't try to highlight the offending variables in-canvas — that's a future polish item.

**Testing:**
- Jest test: POST returns 422 with `{ errors: [{ code: 'unknown_ident', message: 'undefined: foo', variable_name: 'bar', model_name: 'main', kind: 'equation' }] }`. Verify the Editor receives an error message containing those fields and the file on disk is unchanged.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: validation errors surface in Editor toasts`
<!-- END_TASK_10 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase Verification Checklist

Before marking Phase 2 complete:

1. `cargo test --workspace` (no regressions; new save tests pass)
2. `cd src/simlin-serve/web && pnpm test` (frontend tests pass)
3. `cd src/simlin-serve/web && pnpm lint` (frontend lint clean)
4. `cargo clippy -p simlin-serve -- -D warnings` (clippy clean)
5. `cargo fmt -p simlin-serve --check` (formatted)
6. **Manual `.stmx` round-trip:** `cargo run -p simlin-serve -- ./test/test-models/samples/teacup` → open browser, edit a variable, watch the `.xmile` file update on disk. `git diff teacup.xmile` shows only the edited variable's lines (verifies XMILE byte-stability holds in practice).
7. **Manual `.mdl` sidecar:** Run against a directory with a `population.mdl`. Edit in browser. Verify `population.mdl` is unchanged (`md5sum` before vs after) and `population.sd.json` was created with the new state. Refresh the browser; verify the same model loads with the new state.
8. **Manual 409:** Open the same project in two browser tabs. Edit + save in tab 1. Edit + save in tab 2 → expect a "conflict" toast and the editor in tab 2 reloads to show tab 1's state.
9. **Manual 422:** Edit a variable to reference a nonexistent identifier (something the `simlin-engine` validator rejects). Save → expect the validation toast; verify the file is unchanged.

If all 9 verifications pass, Phase 2 is done.
