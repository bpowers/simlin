# Phase 5: Refactor simlin-mcp into Core Library + Binary Implementation Plan

**Goal:** Split `src/simlin-mcp/` into a transport-agnostic library `simlin-mcp-core` (containing all tool implementations, shared format-detection logic, output types, and the rmcp `ServerHandler` impl) and a thin `simlin-mcp` binary (CLI entry point, build-time `{PYSIMLIN_VERSION}` substitution, the embedded `instructions.md` and skill resources, a stateless `FileSystemAccess` implementation of the new `ProjectAccess` trait, and an rmcp `.serve(stdio())` call). The `@simlin/mcp` npm package and its release pipeline are **not** touched. Existing `@simlin/mcp` users see no behavioral changes at the wire level. Phase 6 then mounts the same `simlin-mcp-core` over rmcp's HTTP/SSE transport in `simlin-serve`, sharing a `ProjectRegistry`-backed `ProjectAccess` impl with the HTTP/UI handlers.

**Architecture:** A new `ProjectAccess` async trait abstracts how a tool gets a `(datamodel::Project, SourceFormat)` from a path and how it persists changes. The trait has two impls: the binary's `FileSystemAccess` (stateless, re-reads/writes the file each call — preserves existing `@simlin/mcp` semantics) and (in Phase 6) `simlin-serve`'s `RegistryAccess` (backed by the lazy-hydrated `ProjectRegistry` + `LoroDoc` cache). `simlin-mcp-core` owns the rmcp `ServerHandler` impl as a generic struct `SimlinMcpServer<A: ProjectAccess>`, with each MCP tool defined as an `async fn` decorated by rmcp's `#[tool]` macro. The binary `main` constructs `SimlinMcpServer<FileSystemAccess>::new(...)`, builds the `ServerInfo` (including the `OUT_DIR`-embedded `instructions.md` and the skill resource list), then calls `.serve(stdio()).await`. The hand-rolled `protocol.rs`/`transport.rs`/`tool.rs` are deleted.

**Tech Stack:** New: `rmcp = "1"` with `features = ["server", "macros", "transport-io", "schemars"]`. The `transport-streamable-http-server-session` feature is added by Phase 6 in `simlin-serve`'s `Cargo.toml`, not here. Existing simlin-engine, schemars, serde stay.

**Scope:** Phase 5 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

**No external acceptance criterion is closed by Phase 5** — the design's `server-rewrite.AC*` set targets `simlin-serve` user-facing behavior. Phase 5 is a structural refactor whose verification is operational:
- All `simlin-mcp` integration tests (`tests/build_npm_packages.rs`, `tests/mcp_release_workflow.rs`) pass unchanged.
- A new test suite in `simlin-mcp-core` directly exercises tool functions and the rmcp `ServerHandler` impl.
- A manual or scripted smoke test runs the rebuilt `@simlin/mcp` against an MCP host (claude-cli or `mcp-inspector`) and confirms `initialize` / `tools/list` / `tools/call` / `resources/list` / `resources/read` all behave as before.

This phase enables Phase 6 to consume `simlin-mcp-core` from `simlin-serve` without re-implementing the tool surface. That enabling capability is the structural deliverable.

---

## Notes for Executor

The Phase 5 codebase + rmcp research produced findings that change naive readings of the design. Read these before implementing:

**1. `Tool::call` is sync today; we move to native async.** The current `Tool` trait at `/home/bpowers/src/simlin/src/simlin-mcp/src/tool.rs:20-31` defines `fn call(&self, input: Value) -> anyhow::Result<Value>` — synchronous. simlin-mcp's `serve_async` calls these handlers synchronously inside its async loop, blocking the runtime during simulation and disk I/O. The refactored library uses **native async tool functions** (`async fn name(&self, params: ...) -> Result<...>`), which is what rmcp expects. This is the right shape because (a) Phase 6's HTTP/SSE serving demands async to handle concurrent clients without blocking, (b) the underlying `simlin_engine` operations are CPU-bound but interleaved with disk I/O, and (c) rmcp's macros emit async glue.

**2. `structuredContent` is a NATIVE rmcp field, not a vendor extension.** The current `simlin-mcp` advertises `structuredContent` on tool results — research confirmed this is `CallToolResult::structured_content: Option<Value>` in rmcp 1.5.0. **No workaround needed.** The migration preserves this field byte-for-byte.

**3. Custom error code `-32002` is preserved via `ErrorCode(-32002)`.** The current resource-not-found error uses `-32002`. rmcp exposes `ErrorCode` as a `pub i32` newtype with a public constructor — `ErrorCode(-32002)` works directly. The associated `ErrorCode::RESOURCE_NOT_FOUND` constant exists but the spec history is ambiguous about whether the value is `-32002` or `-32001`; using the explicit `ErrorCode(-32002)` preserves the existing behavior unambiguously.

**4. Existing protocol tests (~27 tests in `protocol.rs`'s `#[cfg(test)]` block) are mostly testing the JSON-RPC dispatcher, not simlin-specific behavior.** They use `MockTransport` and a `roundtrip()` helper to assert wire-format mechanics (parse errors, malformed requests, missing fields, etc.). After the migration, the JSON-RPC dispatcher is rmcp's responsibility, and rmcp has its own test suite covering those cases. **The simlin-mcp protocol tests are dropped**; their replacement is a smaller set of behavior-focused tests at the library level (e.g., "ReadModel returns valid output for the teacup fixture") plus a single end-to-end smoke test that exercises rmcp's stdio transport against the new binary.
   - **This is a meaningful divergence from the design's literal text "All existing simlin-mcp tests pass unchanged".** The tests being dropped do not test simlin-mcp's actual contribution; they test JSON-RPC mechanics that rmcp now owns. The migration preserves *behavioral coverage* (every tool's input/output shape is still tested, just via direct calls instead of through `MockTransport`) while shedding *implementation-detail tests*. Surface this to the user during the Finalization review if there's any doubt.
   - The two integration tests under `src/simlin-mcp/tests/` (`build_npm_packages.rs` and `mcp_release_workflow.rs`) are CI/release validators — unrelated to protocol code. They remain unchanged.

**5. `ProjectAccess` trait — small, async, with a `version` honesty escape hatch.** The trait shape:
```rust
#[async_trait::async_trait]
pub trait ProjectAccess: Send + Sync + 'static {
    async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError>;
    async fn save(&self, abs_path: &Path, project: &datamodel::Project, expected_version: Option<u64>) -> Result<u64, AccessError>;
    async fn create(&self, abs_path: &Path, project: &datamodel::Project) -> Result<(), AccessError>;
}

pub struct OpenedProject {
    pub project: datamodel::Project,
    pub source_format: SourceFormat,
    pub version: u64,
}
```
- `expected_version: Option<u64>` lets the stateless impl pass `None` (no concurrency check) and the registry-backed impl pass `Some(v)` for optimistic locking. Both impls return the new version. Stateless impl always returns `0`.
- `AccessError` enum: `NotFound`, `IoError(io::Error)`, `ParseError(...)`, `VersionMismatch { expected: u64, actual: u64 }`, `WriteError(...)`. The rmcp tool layer maps these to MCP errors.

**6. `ProjectRegistry` from Phases 1-4 stays in `simlin-serve` for now.** The design hints at putting `ProjectRegistry` in a shared location, but it's tightly coupled to the Loro doc cache and file watcher — both of which are simlin-serve-specific. Keeping it in simlin-serve and having simlin-serve provide its own `ProjectAccess` impl in Phase 6 is the cleanest decomposition. simlin-mcp-core never needs to know `ProjectRegistry` exists; it operates against the trait.

**7. `build.rs` and `OUT_DIR`-embedded resources stay in the binary.** Three things drove this:
   - `build.rs` does `{PYSIMLIN_VERSION}` substitution into `instructions.md` and `skills/pysimlin-basics.md`. The substitution belongs to the binary's release process.
   - `include_str!(concat!(env!("OUT_DIR"), ...))` macro uses are tied to the crate that owns `build.rs`.
   - The library should not embed binary-specific content — that violates the "library is reusable" principle. Phase 6's `simlin-serve` will embed its own (different or absent) instructions.
   
   Mechanism: `simlin-mcp-core` exposes `SimlinMcpServer::new(access, server_info)` where `server_info: rmcp::model::ServerInfo` is constructed by the caller. The binary builds the `ServerInfo` with its own `instructions` field and resource list (using the OUT_DIR-included strings).

**8. Resource registry — pass content in, don't embed.** `simlin-mcp-core` exposes a `ResourceContent { uri, name, description, mime_type, body: &'static str }` struct and a `Vec<ResourceContent>` constructor parameter. The binary builds the `Vec` with its own `include_str!`-loaded skill content. The library implements `ServerHandler::list_resources` and `read_resource` against this Vec. Trade-off: the library is content-agnostic; the binary owns its own resources. Phase 6's `simlin-serve` either passes the same Vec (re-using the binary's compiled resources via cargo) or its own Vec.
   - Concretely: `simlin-mcp-core::SimlinMcpServer::new(access: A, instructions: String, resources: Vec<ResourceContent>) -> Self`.

**9. `PROTOCOL_VERSION` constant.** Today's simlin-mcp advertises `"2025-11-25"`. rmcp exposes `ProtocolVersion::V_2025_11_25` and `ProtocolVersion::LATEST` (currently equal). Use `LATEST` so the library auto-tracks rmcp's view of the spec; if a future rmcp bumps `LATEST` to a newer date and we want to pin the old one, switch to the explicit constant.

**10. Single-file `protocol.rs` deletion.** The 1,118-line `protocol.rs` becomes empty after the migration. Delete the file rather than leaving a stub. Same for `transport.rs` (the `Transport` trait + `StdioTransport` are subsumed by rmcp's `transport::stdio()`) and `tool.rs` (the `Tool` trait + `Registry` + `TypedTool` are subsumed by rmcp's macros).

**11. `serde_json` types in tool inputs.** rmcp's `Parameters<T>` extractor expects `T: JsonSchema + DeserializeOwned`. The existing input structs (`ReadModelInput`, `EditModelInput`, `CreateModelInput`) already derive `JsonSchema` and `Deserialize` via simlin-mcp's `TypedTool` infrastructure. Migration is a derive-macro swap from simlin-mcp's hand-rolled schema generation to rmcp's expected derives. Use `schemars = { version = "1", features = ["preserve_order"] }` to match the existing simlin-mcp Cargo.toml entry (`src/simlin-mcp/Cargo.toml:15`); both `simlin-mcp-core` and `simlin-mcp` use the same version so the workspace stays single-versioned.

**12. `Send + Sync + Clone` on the server type.** rmcp's `StreamableHttpService::new(|| Ok(Self::new()), ...)` (Phase 6) takes a factory that constructs a fresh handler per session. The handler must be `Clone` so rmcp can clone it as needed. `Arc<A>` for the access impl handles this — the handler holds a cloneable `Arc<A>`, not the access value directly.

**13. The async-trait crate.** rmcp 1.5 uses RPITIT (return-position-impl-trait-in-trait), so it does NOT need `async-trait` for its own traits. But our `ProjectAccess` trait needs to be `dyn`-friendly so we can store `Arc<dyn ProjectAccess>` if needed later — for that we DO need `#[async_trait::async_trait]`. Add `async-trait = "0.1"` as a dependency. Alternatively, if the trait is only ever used with concrete types (which is how rmcp wants it — generics, not trait objects), drop async-trait and use native `async fn` in trait definitions. Decision: use native `async fn` in the trait (Rust 1.95 supports this) and **avoid trait objects**. The `SimlinMcpServer<A>` is generic; the binary instantiates `SimlinMcpServer<FileSystemAccess>`; simlin-serve in Phase 6 instantiates `SimlinMcpServer<RegistryAccess>`. No `Arc<dyn ProjectAccess>` needed.

**14. EditModel's diagnostic-gating logic moves to the library and stays sync at the simlin-engine call site.** The salsa-based diagnostic check (`SimlinDb::default()` + `sync_from_datamodel` + `collect_all_diagnostics`) is sync in simlin-engine. Calling it from an `async fn` is fine — it just blocks the task during the (typically <100ms) check. For long-running models, this could become a problem; if so, wrap in `tokio::task::spawn_blocking`. For Phase 5, do not optimize prematurely.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-2) -->
### Subcomponent A: Library scaffold + ProjectAccess trait

<!-- START_TASK_1 -->
### Task 1: Create `simlin-mcp-core` crate; update workspace

**Verifies:** none directly (scaffolding)

**Files:**
- Modify: `/home/bpowers/src/simlin/Cargo.toml` (add `"src/simlin-mcp-core",` to workspace `members`, alphabetical)
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/Cargo.toml`
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/lib.rs` (empty `pub mod` declarations stub)
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/README.md`

**Implementation:**
- Cargo.toml: `name = "simlin-mcp-core"`, `edition = "2024"`, `version = "0.1.0"`, `license = "Apache-2.0"`, `authors = ["Bobby Powers <bobbypowers@gmail.com>"]`. Initial deps: `simlin-engine = { path = "../simlin-engine" }`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `schemars = { version = "1", features = ["preserve_order"] }` (matches existing simlin-mcp), `tokio = { version = "1", features = ["rt-multi-thread", "macros"] }`, `anyhow = "1"`, `tracing = "0.1"`. (rmcp added in Task 6.)
- `lib.rs`: declare modules that will land in subsequent tasks: `pub mod access; pub mod errors; pub mod open; pub mod tools; pub mod types;`. Each is an empty module stub (`pub mod access {}` placeholder).

**Verification:**
- `cargo build -p simlin-mcp-core` succeeds.
- `cargo metadata` shows the crate.

**Commit:** `mcp-core: scaffold simlin-mcp-core crate`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `ProjectAccess` trait + `OpenedProject`/`AccessError`

**Verifies:** none directly (foundation for tool refactor)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/access.rs`
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/errors.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/lib.rs` (re-export `pub use access::*; pub use errors::*;`)

**Implementation:**
- `errors.rs` defines `pub enum AccessError` with variants: `NotFound { path: PathBuf }`, `IoError(io::Error)`, `ParseError(anyhow::Error)`, `VersionMismatch { expected: u64, actual: u64 }`, `WriteError(io::Error)`, `Validation { errors: Vec<ValidationError> }`. Implements `Display` + `Error`.
- `pub struct ValidationError { pub code: String, pub message: String, pub model_name: Option<String>, pub variable_name: Option<String>, pub kind: String }` (mirror of simlin-mcp's existing `ErrorOutput` shape — preserves the wire format).
- `access.rs` defines `pub struct OpenedProject { pub project: datamodel::Project, pub source_format: SourceFormat, pub version: u64 }` and:
  ```rust
  pub trait ProjectAccess: Send + Sync + 'static {
      fn open(&self, abs_path: &Path) -> impl Future<Output = Result<OpenedProject, AccessError>> + Send;
      fn save(
          &self,
          abs_path: &Path,
          project: &datamodel::Project,
          format: SourceFormat,
          expected_version: Option<u64>,
      ) -> impl Future<Output = Result<u64, AccessError>> + Send;
      fn create(
          &self,
          abs_path: &Path,
          project: &datamodel::Project,
          format: SourceFormat,
      ) -> impl Future<Output = Result<(), AccessError>> + Send;
  }
  ```
  Native AFIT (no `#[async_trait]`). The trait is generic-only; no `dyn ProjectAccess` is supported, which is acceptable because rmcp wants concrete types.
- `SourceFormat` enum: `Xmile`, `NativeJson`, `SdaiJson`. Identical to `simlin-mcp`'s existing enum (which moves here in Task 3).

**Testing:**
- Inline test: a stub `MockAccess` impl, call `open(Path::new("nonexistent"))` returns `Err(NotFound)`. (Just verifies the trait compiles and the error path works.)

**Verification:**
- `cargo test -p simlin-mcp-core access::` passes.

**Commit:** `mcp-core: ProjectAccess trait with OpenedProject and AccessError`
<!-- END_TASK_2 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 3-6) -->
### Subcomponent B: Move tool implementations into the library

<!-- START_TASK_3 -->
### Task 3: Move shared logic — `open_project`, `resolve_model_name`, `ensure_variable_uids`, output types

**Verifies:** none directly (refactor scaffolding)

**Files:**
- Move: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/mod.rs` (the shared functions only, NOT the tool registration) → `/home/bpowers/src/simlin/src/simlin-mcp-core/src/open.rs` and `/home/bpowers/src/simlin/src/simlin-mcp-core/src/types.rs`
- Move: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/types.rs` → `/home/bpowers/src/simlin/src/simlin-mcp-core/src/types.rs` (merge with the destination's existing content)
- Modify: simlin-mcp's remaining `tools/mod.rs` to re-export from `simlin_mcp_core`

**Implementation:**
- `open.rs` exposes `pub fn open_project(path: &Path, contents: &str) -> Result<(datamodel::Project, SourceFormat), AccessError>` (same semantics as today, error type swapped to `AccessError`), `pub fn resolve_model_name(project: &datamodel::Project, requested: &str) -> Result<String, AccessError>`, `pub fn ensure_variable_uids(project: &mut datamodel::Project)`.
- `types.rs` exposes `LoopDominanceSummary`, `DominantPeriodOutput`, `ErrorOutput` (with `From<&FormattedError>`), `EditOperation` enum, etc. — the full set from simlin-mcp's current `tools/types.rs`. Preserve all `#[serde(rename_all = "camelCase")]` attributes so wire format is identical.
- simlin-mcp's `tools/mod.rs` becomes a re-export shim: `pub use simlin_mcp_core::open::*; pub use simlin_mcp_core::types::*;`. Tool files (`read_model.rs`, etc.) continue to compile against these re-exports for now. (Task 4 moves them too.)
- Modify simlin-mcp's `Cargo.toml` to add `simlin-mcp-core = { path = "../simlin-mcp-core" }`.

**Testing:**
- Move (do not duplicate) any unit tests inside the moved files.
- Run `cargo test -p simlin-mcp` — existing tests must still pass since the re-exports preserve the call sites.

**Verification:**
- `cargo test --workspace` passes (no behavior change yet).

**Commit:** `mcp-core: move open_project, resolve_model_name, types; mcp re-exports`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Move ReadModel + CreateModel as async library functions

**Verifies:** none directly (refactor)

**Files:**
- Move: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/read_model.rs` → `/home/bpowers/src/simlin/src/simlin-mcp-core/src/tools/read_model.rs`
- Move: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/create_model.rs` → `/home/bpowers/src/simlin/src/simlin-mcp-core/src/tools/create_model.rs`
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/tools/mod.rs` (declare submodules)
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/lib.rs` (re-export `pub mod tools;`)

**Implementation:**
- Both tools become async fns that take `&A: ProjectAccess`:
  ```rust
  pub async fn read_model<A: ProjectAccess>(
      access: &A,
      input: ReadModelInput,
  ) -> Result<ReadModelOutput, AccessError> { ... }

  pub async fn create_model<A: ProjectAccess>(
      access: &A,
      input: CreateModelInput,
  ) -> Result<CreateModelOutput, AccessError> { ... }
  ```
- Internally: `let opened = access.open(Path::new(&input.project_path)).await?;` replaces today's `std::fs::read_to_string(&input.project_path)?`. The rest of the logic (resolve_model_name, analyze_model, etc.) is unchanged.
- For CreateModel: the function builds an empty `datamodel::Project` from `CreateModelInput`'s `sim_specs`, then calls `access.create(...)`.
- Add `derive(JsonSchema)` to `ReadModelInput` and `CreateModelInput` if not already present.

**Sequencing — keep the binary green at every commit, do the cutover in Task 8.** The library's tools are async (the new shape). The existing simlin-mcp binary uses sync `Tool::call`. Bridging sync→async via `Handle::current().block_on()` panics inside a tokio runtime; bridging via `tokio::task::block_in_place(|| Handle::current().block_on(...))` works **only** on the multi-thread runtime (simlin-mcp's `#[tokio::main]` defaults to multi-thread, so it would work — but it's a footgun). To avoid both pitfalls and avoid landing in a half-broken state:

- **Tasks 4-5 do NOT move the tool wrappers from `src/simlin-mcp/src/tools/`.** They CREATE the new library async fns alongside, leaving the binary's existing sync tool wrappers in place. The new library async fns and the existing sync wrappers will share the underlying simlin-engine calls (open_project, apply_patch, etc.) which are themselves sync. The library's async fn is a thin async-flavored adapter that does `let opened = access.open(...).await?;` and then calls the same sync engine code. The binary's sync tool wrapper continues to call `std::fs::read_to_string(...)` directly. Both code paths coexist; the binary is unchanged in behavior.
- **Task 6** adds `rmcp` as a dependency on the library (not the binary) and defines `SimlinMcpServer<A>` calling the async fns. Still doesn't touch the binary's transport.
- **Task 7** adds `FileSystemAccess` (a new file in the binary) implementing `ProjectAccess`. Still doesn't change the binary's transport.
- **Task 8** is the atomic cutover commit: delete `src/simlin-mcp/src/tools/` (now redundant — the library has the canonical async versions), delete `protocol.rs` / `transport.rs` / `tool.rs` / `resource.rs`, replace `main.rs` with the rmcp `serve(stdio())` pattern using `SimlinMcpServer<FileSystemAccess>`. After Task 8, the binary uses the library's async fns natively via rmcp's async tool dispatch.

The pre-commit hook stays green because each intermediate commit (Tasks 4-7) leaves the binary unchanged in behavior. The behavioral switch happens in Task 8, which is one commit, and is followed immediately by Task 9's smoke test against `mcp-inspector`.

**Testing:**
- New library test: `tokio::test`-driven, calls `read_model(&FileSystemAccess::new(), ReadModelInput { project_path: "../../test/test-models/samples/teacup/teacup.xmile".into(), model_name: None }).await`, asserts the output's `model.name == "main"` and `errors` is None.
- Same for `create_model`: tempdir, call create, assert file exists with parseable content.

**Verification:**
- `cargo test -p simlin-mcp-core tools::read_model::` and `tools::create_model::` pass.
- `cargo test -p simlin-mcp` (existing tests) still pass (they go through the bridge code).

**Commit:** `mcp-core: move ReadModel and CreateModel as async lib functions`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Move EditModel as async library function (with diagnostic gating)

**Verifies:** none directly (refactor)

**Files:**
- Move: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/edit_model.rs` → `/home/bpowers/src/simlin/src/simlin-mcp-core/src/tools/edit_model.rs`
- Test: `/home/bpowers/src/simlin/src/simlin-mcp-core/tests/edit_model_e2e.rs`

**Implementation:**
- `pub async fn edit_model<A: ProjectAccess>(access: &A, input: EditModelInput) -> Result<EditModelOutput, AccessError>`.
- Replace `std::fs::read_to_string`/`atomic_write` calls with `access.open(...)` and `access.save(...)`. The diagnostic-gating logic (pre-edit baseline + post-edit check) is preserved — it runs against the parsed `datamodel::Project` from `access.open`, and surfaces validation failures via `AccessError::Validation { errors }`.
- The current implementation calls `simlin-engine`'s salsa-based diagnostic API synchronously inside what is now an async fn — that's fine for Phase 5 (each call is fast enough). If profiling later shows blocking issues, wrap the salsa call in `tokio::task::spawn_blocking`.
- The `.mdl` rejection (existing behavior at `edit_model.rs:219-222`) stays as-is — `simlin-mcp` still rejects edits to `.mdl` paths. Phase 6's `simlin-serve` uses `RegistryAccess` whose `save` for `.mdl`-format projects writes the sidecar; the simlin-mcp binary's `FileSystemAccess` would need to do the same to be consistent. Leave the rejection in place for Phase 5 (matches today's semantics); Task 8 in `simlin-serve`'s plan can revisit.

**Testing:**
- E2E test: load fixture, apply an `UpsertStock` op via `edit_model(&FileSystemAccess::new(), input).await`, assert the file on disk reflects the change.
- Negative test: apply an op that would introduce a new error → `Err(AccessError::Validation { errors: [...] })`, file unchanged.

**Verification:**
- `cargo test -p simlin-mcp-core --test edit_model_e2e` passes.

**Commit:** `mcp-core: move EditModel as async lib function with validation gate`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Add `rmcp` dep; define `SimlinMcpServer<A: ProjectAccess>` with `#[tool]` macros

**Verifies:** none directly (rmcp surface)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp-core/Cargo.toml` (add `rmcp = { version = "1", features = ["server", "macros", "schemars"] }`)
- Create: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/server.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/lib.rs` (re-export `pub mod server; pub use server::SimlinMcpServer;`)

**Implementation:**
- `pub struct ResourceContent { pub uri: String, pub name: String, pub description: String, pub mime_type: String, pub body: String }`.
- `#[derive(Clone)]` on `SimlinMcpServer<A: ProjectAccess>` with fields `access: Arc<A>`, `instructions: Arc<String>`, `resources: Arc<Vec<ResourceContent>>`, `tool_router: ToolRouter<Self>`.
- Constructor `pub fn new(access: A, instructions: String, resources: Vec<ResourceContent>) -> Self`:
  - Wraps each input in `Arc`.
  - Calls `Self::tool_router()` (generated by `#[tool_router]`).
- `#[tool_router] impl<A: ProjectAccess> SimlinMcpServer<A> { ... }` block defines `read_model`, `edit_model`, `create_model` methods, each with `#[tool(description = "...")]` and a `Parameters<InputType>` extractor. Each method:
  ```rust
  #[tool(description = "Read a model and return its JSON snapshot with loop dominance analysis.")]
  async fn read_model(
      &self,
      Parameters(input): Parameters<ReadModelInput>,
  ) -> Result<CallToolResult, McpError> {
      match crate::tools::read_model::read_model(&*self.access, input).await {
          Ok(output) => {
              let value = serde_json::to_value(&output).map_err(|e| McpError::internal_error(e.to_string()))?;
              let text = serde_json::to_string(&value).unwrap();
              Ok(CallToolResult::success(vec![Content::text(text)])
                  .with_structured_content(value))  // preserves the structuredContent field
          }
          Err(AccessError::Validation { errors }) => {
              let value = serde_json::to_value(&errors).map_err(|e| McpError::internal_error(e.to_string()))?;
              let text = serde_json::to_string(&value).unwrap();
              Ok(CallToolResult::error(vec![Content::text(text)])
                  .with_structured_content(value))
          }
          Err(e) => Err(McpError::internal_error(e.to_string())),
      }
  }
  ```
  (CallToolResult helper methods may be slightly different — verify against rmcp 1.5.0 docs at `https://docs.rs/rmcp/latest/rmcp/model/struct.CallToolResult.html`. The intent is: success path includes both `text` content and `structured_content`; error path same with `is_error: true`.)
- Repeat for `edit_model` and `create_model`.
- `#[tool_handler] impl<A: ProjectAccess> ServerHandler for SimlinMcpServer<A> { ... }`:
  - `fn get_info(&self) -> ServerInfo`: returns `ServerInfo::new(ServerCapabilities::builder().enable_tools().enable_resources().build()).with_server_info(Implementation::new("simlin-mcp", env!("CARGO_PKG_VERSION"))).with_protocol_version(ProtocolVersion::LATEST).with_instructions(self.instructions.as_str())`.
  - `async fn list_resources(&self, _: Option<PaginatedRequestParams>, _: RequestContext<RoleServer>) -> Result<ListResourcesResult, McpError>`: maps `self.resources` to `Resource` entries (uri, name, mime_type, description; no `content` field — that's read on demand).
  - `async fn read_resource(&self, params: ReadResourceRequestParams, _: RequestContext<RoleServer>) -> Result<ReadResourceResult, McpError>`: linear scan over `self.resources` for the URI, returns `ReadResourceResult { contents: vec![ResourceContents::text(uri, body.clone())], meta: None }`. On miss returns `Err(McpError::resource_not_found(...))` with `ErrorCode(-32002)` (use `McpError { code: ErrorCode(-32002), message: ..., data: ... }` if the `resource_not_found` constructor doesn't allow forcing the code).

**Testing:**
- Unit test: construct `SimlinMcpServer::new(MockAccess::new(), "test instructions".into(), vec![])`, call `get_info()`, assert protocol version and capabilities.
- Test resource list/read with one synthetic `ResourceContent`.

**Verification:**
- `cargo test -p simlin-mcp-core server::` passes.
- `cargo build -p simlin-mcp-core --release` succeeds (validates rmcp macro expansion).

**Commit:** `mcp-core: SimlinMcpServer rmcp ServerHandler impl with tool macros`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 7-9) -->
### Subcomponent C: Migrate `simlin-mcp` binary to thin wrapper

<!-- START_TASK_7 -->
### Task 7: `FileSystemAccess` impl in the binary

**Verifies:** none directly (preserves @simlin/mcp behavior)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-mcp/src/access.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp/Cargo.toml` (add `rmcp = { version = "1", features = ["server", "macros", "transport-io"] }`; remove `tokio` features that are no longer needed)
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp/src/lib.rs` (or create if not present; re-export the `FileSystemAccess`)

**Implementation:**
- `pub struct FileSystemAccess;` with `impl FileSystemAccess { pub fn new() -> Self { Self } }`.
- `impl ProjectAccess for FileSystemAccess`:
  - `open`: `let contents = tokio::fs::read_to_string(abs_path).await?; let (project, format) = simlin_mcp_core::open::open_project(abs_path, &contents)?; Ok(OpenedProject { project, source_format: format, version: 0 })`
  - `save`: serialize `project` to bytes per format (XMILE for `Xmile`, JSON for `NativeJson`/`SdaiJson` — use the same logic that's in the current `edit_model.rs:330-340`, lifted into a small helper). Then `simlin_engine::io::atomic_write(abs_path, &bytes)`. Return `Ok(0)` (stateless = no version).
  - `create`: validate the path doesn't exist, ensure parent dir, serialize, `atomic_write`. Return `Ok(())`.
- Reject `.mdl` writes here (preserve the simlin-mcp behavior): if `format == Xmile` AND the path's extension is `mdl`, return `Err(AccessError::WriteError(io::Error::new(io::ErrorKind::Unsupported, "Vensim .mdl files are read-only. Use ReadModel to inspect a .mdl file, then CreateModel to start a new .sd.json file you can edit.")))`. Use the same exact error string for backward compat.

**Testing:**
- E2E test in `src/simlin-mcp/tests/file_system_access.rs`: open a fixture, save it back, re-read, assert byte stability.
- `.mdl` rejection test: try to save a project with format `Xmile` to a `.mdl` path, expect `Err(WriteError)` with the canonical message.

**Verification:**
- `cargo test -p simlin-mcp --test file_system_access` passes.

**Commit:** `mcp: FileSystemAccess impl preserves stateless file semantics`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Replace `protocol.rs`/`transport.rs`/`tool.rs` with rmcp `serve(stdio())`

**Verifies:** none directly (binary migration)

**Files:**
- Delete: `/home/bpowers/src/simlin/src/simlin-mcp/src/protocol.rs`
- Delete: `/home/bpowers/src/simlin/src/simlin-mcp/src/transport.rs`
- Delete: `/home/bpowers/src/simlin/src/simlin-mcp/src/tool.rs`
- Delete: `/home/bpowers/src/simlin/src/simlin-mcp/src/resource.rs` (resources are now constructed in `main.rs` and passed to `SimlinMcpServer::new`)
- Delete: `/home/bpowers/src/simlin/src/simlin-mcp/src/tools/` (the entire directory — all moved to library, plus the bridge stubs added in Task 4 are no longer needed)
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp/src/main.rs` (rewrite as a thin entry point)
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp/Cargo.toml` (drop deps no longer needed: `serde_json` direct dep may still be needed for `Value` types in tests; keep on a need-by-need basis)

**Implementation:**
- New `main.rs` (~30 lines):
  ```rust
  use simlin_mcp::access::FileSystemAccess;
  use simlin_mcp_core::{server::SimlinMcpServer, server::ResourceContent};
  use rmcp::{ServiceExt, transport::stdio};

  #[tokio::main]
  async fn main() -> anyhow::Result<()> {
      let args: Vec<String> = std::env::args().collect();
      if args.iter().any(|a| a == "--version" || a == "-V") {
          println!("simlin-mcp {}", env!("CARGO_PKG_VERSION"));
          return Ok(());
      }

      let instructions = include_str!(concat!(env!("OUT_DIR"), "/instructions.md")).to_string();

      // Two skills go through build.rs's {PYSIMLIN_VERSION} substitution (see src/simlin-mcp/build.rs:19-31):
      // - instructions.md (used as the ServerInfo.instructions)
      // - skills/pysimlin-basics.md (the only skill that needs version substitution)
      // The other three skill files are included verbatim from source.
      let resources = vec![
          ResourceContent {
              uri: "simlin://skills/pysimlin-basics".into(),
              name: "pysimlin basics".into(),
              description: "Loading models, simulation, DataFrame access.".into(),
              mime_type: "text/markdown".into(),
              body: include_str!(concat!(env!("OUT_DIR"), "/pysimlin-basics.md")).into(),
          },
          ResourceContent {
              uri: "simlin://skills/scenario-analysis".into(),
              name: "scenario analysis".into(),
              description: "Parameter sweeps and intervention analysis.".into(),
              mime_type: "text/markdown".into(),
              body: include_str!("skills/scenario-analysis.md").into(),
          },
          ResourceContent {
              uri: "simlin://skills/loop-dominance".into(),
              name: "loop dominance".into(),
              description: "Plotting behavior, annotating dominant periods.".into(),
              mime_type: "text/markdown".into(),
              body: include_str!("skills/loop-dominance.md").into(),
          },
          ResourceContent {
              uri: "simlin://skills/vensim-equation-syntax".into(),
              name: "vensim equation syntax".into(),
              description: "MDL-to-XMILE mapping table.".into(),
              mime_type: "text/markdown".into(),
              body: include_str!("skills/vensim-equation-syntax.md").into(),
          },
      ];

      let server = SimlinMcpServer::new(FileSystemAccess::new(), instructions, resources);
      let service = server.serve(stdio()).await?;
      service.waiting().await?;
      Ok(())
  }
  ```
- The `skills/` directory MOVES with the binary (it's binary-specific content). The `build.rs` keeps doing `{PYSIMLIN_VERSION}` substitution into `OUT_DIR/pysimlin-basics.md`.
- Cargo.toml dep additions: `rmcp` already added in Task 7. Remove direct deps that were only used by the deleted files (`tracing` if unused, etc.).
- Delete the entire `src/simlin-mcp/src/tools/` directory — there are no longer any tool files there.

**Testing:**
- Integration tests in `src/simlin-mcp/tests/` (`build_npm_packages.rs`, `mcp_release_workflow.rs`) must still pass — they don't depend on protocol.rs.
- New smoke test `src/simlin-mcp/tests/stdio_smoke.rs`: spawn the binary as a subprocess (`std::process::Command::new(env!("CARGO_BIN_EXE_simlin-mcp"))` — Cargo provides this env var for integration tests), pipe a JSON-RPC `initialize` request to stdin, read the response from stdout, assert `protocolVersion`, `serverInfo.name == "simlin-mcp"`, `capabilities.tools` and `capabilities.resources` present.

**Verification:**
- `cargo test -p simlin-mcp` passes (existing integration tests + the new smoke test).
- Manual: `echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}' | cargo run -p simlin-mcp` returns a sensible response.

**Commit:** `mcp: replace protocol/transport/tool with rmcp serve(stdio())`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Drop the old hand-rolled tests; add focused library + smoke tests

**Verifies:** none directly (test reorganization)

**Files:**
- The `protocol.rs` `#[cfg(test)]` block was already deleted in Task 8 (when we deleted `protocol.rs`). This task ensures the surviving test surface is sufficient.
- Verify: `src/simlin-mcp-core/tests/` contains:
  - `read_model_e2e.rs` (Task 4) — tool-level test against fixtures
  - `create_model_e2e.rs` (Task 4)
  - `edit_model_e2e.rs` (Task 5) — including the validation-error path
- Verify: `src/simlin-mcp/tests/` contains:
  - `file_system_access.rs` (Task 7) — `ProjectAccess` impl test
  - `stdio_smoke.rs` (Task 8) — end-to-end against the running binary
  - `build_npm_packages.rs` (untouched) — CI artifact validation
  - `mcp_release_workflow.rs` (untouched) — release workflow validation
- Add coverage for any tool behaviors that the old protocol tests verified but that aren't covered by the library-level tests:
  - "Unknown tool name" → covered by rmcp; no test needed in our crates (delete if added).
  - "Tool error returns is_error: true" → add a test in `simlin-mcp-core/tests/` that calls a tool with an input that causes a validation error and asserts the response uses `is_error: true` and includes the structured content. Walk through `SimlinMcpServer::read_model` (or the equivalent rmcp dispatch path) directly.

**Verification:**
- `cargo test -p simlin-mcp-core` passes — every tool's success and error path is covered.
- `cargo test -p simlin-mcp` passes — the binary's smoke test and infrastructure tests pass.
- `cargo test --workspace` passes — no regressions anywhere.

**Commit:** `mcp: tests reorganized — library covers behavior, binary covers transport`
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (task 10) -->
### Subcomponent D: End-to-end verification against an MCP host

<!-- START_TASK_10 -->
### Task 10: Manual end-to-end smoke against `mcp-inspector` or claude-cli

**Verifies:** server-rewrite.AC5 partial (the existing simlin-mcp surface is preserved; AC5 is fully verified in Phase 6)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp/README.md` (add a "Verifying the build" section with instructions for `mcp-inspector` or `claude mcp`)

**Implementation:**
- Document the manual smoke procedure:
  1. `cargo build -p simlin-mcp --release`.
  2. Run `npx @modelcontextprotocol/inspector ./target/release/simlin-mcp`.
  3. In the inspector UI, verify the `initialize` exchange shows `protocolVersion: "2025-11-25"`, `serverInfo.name: "simlin-mcp"`, `capabilities.tools` and `capabilities.resources` present.
  4. Click "List Tools" — verify `read_model`, `edit_model`, `create_model` appear.
  5. Click "Call Tool" → `read_model` with `{ "project_path": "<absolute path to teacup.xmile>" }`. Verify the response carries `structuredContent` with the expected shape.
  6. List resources → verify the four `simlin://skills/...` URIs appear; read each → verify content non-empty.

**Verification:**
- Manual procedure documented; executor performs once and confirms.

**Commit:** `mcp: README documents end-to-end verification procedure`
<!-- END_TASK_10 -->
<!-- END_SUBCOMPONENT_D -->

---

## Phase Verification Checklist

Before marking Phase 5 complete:

1. `cargo test --workspace` passes — every crate's tests succeed.
2. `cargo clippy --workspace -- -D warnings` clean.
3. `cargo fmt --workspace --check` clean.
4. `cargo build -p simlin-mcp --release` produces a working binary.
5. **Manual smoke per Task 10:** `mcp-inspector` against the new binary shows the same tool/resource surface as the old one, and a sample `read_model` call returns the expected output with `structuredContent` present.
6. **`@simlin/mcp` distribution sanity check:** `bash src/simlin-mcp/build-npm-packages.sh` still works (the script doesn't touch Rust code; its output package.json files unchanged). The CI `mcp-release.yml` workflow YAML is untouched.
7. **Library smoke from `simlin-serve` (preview for Phase 6):** add `simlin-mcp-core = { path = "../simlin-mcp-core" }` to `simlin-serve/Cargo.toml`, write a one-line check in `simlin-serve/src/lib.rs` that imports something from the library (`use simlin_mcp_core::access::ProjectAccess;`), `cargo build -p simlin-serve` succeeds. This verifies the library is consumable by both binaries — no need to wire it up further; that's Phase 6's job.

If all 7 verifications pass, Phase 5 is done.
