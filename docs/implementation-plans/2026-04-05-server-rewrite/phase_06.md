# Phase 6: In-Process MCP in simlin-serve Implementation Plan

**Goal:** Mount an rmcp Streamable HTTP MCP server inside `simlin-serve` on a stable, configurable port (default `7878`). The MCP handlers share the same `ProjectRegistry` (from Phases 1-4) as the HTTP/UI handlers, so MCP-initiated edits and browser-initiated edits flow through the same `apply_canonical_json` merge primitive and produce identical end states. The MCP server exposes the existing `read_model` / `edit_model` / `create_model` tools (delegating to `simlin-mcp-core` from Phase 5) plus two new tools — `list_projects` (mirrors `GET /api/projects`) and `simulate` (runs a simulation via `simlin-engine`).

**Architecture:** A new `RegistryAccess` struct in `simlin-serve` implements `simlin_mcp_core::access::ProjectAccess`. Its `open` method calls `ProjectRegistry::get_or_init_doc(...)` and returns the in-memory `LoroDoc`-derived state (so MCP clients always see the same in-flight state as the browser). Its `save` method calls `ProjectRegistry::check_increment_and_merge(...)` (Phase 3) — the same code path as a browser save — and then writes to disk via the same writer module from Phase 2 (XMILE in-place for `.stmx`/`.xmile`/`.sd.json`, `.sd.json` sidecar for `.mdl`). After a successful save, `EventBus::publish(WsMessage::ProjectChanged { source: ChangeSource::Agent, ... })` broadcasts the change so the browser's WebSocket subscriber re-fetches and the editor remounts. A new `SimlinServeMcpServer<A>` in `simlin-serve` is the rmcp `ServerHandler`, with five `#[tool]` methods that delegate the three reused tools to `simlin-mcp-core`'s free async fns and define `list_projects` and `simulate` locally. `StreamableHttpService::new(|| Ok(SimlinServeMcpServer::new(Arc::clone(&state))), ...)` is mounted at `/mcp` on a separate `axum::Router` bound to the stable MCP port. The two-listener pattern (`tokio::try_join!`) serves both routers concurrently. Port-conflict detection on bind reports `ErrorKind::AddrInUse` with a friendly hint.

**Tech Stack:** Reuses everything. New for simlin-serve: `rmcp = "1"` with `features = ["server", "macros", "schemars", "transport-streamable-http-server-session"]` (the HTTP transport feature in addition to what simlin-mcp-core already adds). Optional: `axum-server` is unnecessary; the standard `axum::serve` works.

**Scope:** Phase 6 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC4 (closeout): Concurrent editing via Loro
- **server-rewrite.AC4.1 Success:** Two near-simultaneous edits from the browser and the MCP server both apply without data loss *(now fully covered — Phase 3 verified browser-vs-browser; this phase verifies browser-vs-MCP via shared `apply_canonical_json` path)*

### server-rewrite.AC5: In-process MCP
- **server-rewrite.AC5.1 Success:** `simlin-serve` exposes an MCP server at `http://127.0.0.1:7878/mcp` (configurable via `--mcp-port`) when launched
- **server-rewrite.AC5.2 Success:** The MCP server advertises `file://$PWD` as a root in its initialize response *(see Notes below — this language is corrected to use the `instructions` field, since `roots` is a client capability per the MCP spec, not a server one)*
- **server-rewrite.AC5.3 Success:** A Claude desktop client configured against the URL can call `list_projects`, `get_project`, `apply_edit`, `simulate`, and `create_project` tools and observe results in the browser within one second *(tool name mapping: `get_project` → `read_model`, `apply_edit` → `edit_model`, `create_project` → `create_model` — see Notes)*
- **server-rewrite.AC5.4 Success:** MCP-initiated edits and browser-initiated edits flow through the same `apply_canonical_json` merge primitive and produce identical end states regardless of order
- **server-rewrite.AC5.5 Edge:** A second `simlin-serve` started in the same `$PWD` (or with the same `--mcp-port`) fails fast with a port-conflict message and a hint to either stop the running instance or pass `--mcp-port`

### server-rewrite.AC6 (continued partial): MCP push notifications — agent source
- **server-rewrite.AC6.3 Success:** Any change (browser, MCP, disk) emits a `projectChanged` notification with a `source` discriminator (`"user" | "agent" | "disk"`) *(`"agent"` source added; the `notifications_router` that pushes these to MCP clients arrives in Phase 7)*

---

## Notes for Executor

The Phase 6 codebase + research produced findings that change naive readings of the design. Read these before implementing:

**1. The design's "advertise `file://$PWD` as a root" is incorrect — `roots` is a CLIENT capability, not a server one.** Per the MCP spec (2025-06-18 onward), `roots` is what clients declare so servers know workspace boundaries; clients call `roots/list` on themselves at the server's request. Servers cannot advertise roots. AC5.2's literal language is preserved above for traceability, but the implementation puts the working-directory hint in the `instructions` field of `get_info()` instead. Example: `instructions: format!("Operating on file://{}. Use list_projects to enumerate models, then apply_edit to modify them.", root_dir.display())`. This is the spec-conformant way to communicate the working directory to LLM clients.

**2. Tool name mapping.** AC5.3 lists `list_projects, get_project, apply_edit, simulate, create_project`. We **keep the existing simlin-mcp tool names** (`read_model`, `edit_model`, `create_model`) for the three reused tools so AI clients see a consistent surface across `@simlin/mcp` (stdio) and `simlin-serve` (HTTP). The new tools added by simlin-serve are `list_projects` and `simulate`. So the actual tool surface is:
   - `read_model` (was `get_project` in design) — already exists in simlin-mcp-core
   - `edit_model` (was `apply_edit` in design) — already exists in simlin-mcp-core
   - `create_model` (was `create_project` in design) — already exists in simlin-mcp-core
   - `list_projects` — new in this phase
   - `simulate` — new in this phase

**3. `RegistryAccess` shares state with the HTTP/UI via `Arc<AppState>`.** The `Arc<ProjectRegistry>` (via `AppState`) is captured in the `StreamableHttpService::new(|| Ok(handler::new(Arc::clone(&state))), ...)` factory closure. Each MCP session gets a fresh `SimlinServeMcpServer` instance, but all instances share the same `Arc<ProjectRegistry>` — and therefore the same `LoroDoc` cache, version counter, and event bus. The `with_state(...)` mechanism on the Axum router does **not** apply here (rmcp's tower-service mounts don't see Axum's state); explicit `Arc` capture is required.

**4. Two separate listeners, single `tokio::try_join!`.** The HTTP/UI router (Phases 1-3) binds on the ephemeral port (default `0`); the MCP router binds on the stable port (default `7878`). Both run via the standard `axum::serve(listener, router).await` pattern, joined via `tokio::try_join!` in `main`. The architectural separation makes port-conflict messages localized: only an MCP-port conflict mentions `--mcp-port`.

**5. `RegistryAccess::save` for `.mdl` paths writes a sidecar (does NOT reject).** This is the agent equivalent of Phase 2's browser sidecar logic. simlin-mcp's `FileSystemAccess` rejects `.mdl` writes (preserving the existing `@simlin/mcp` behavior); simlin-serve's `RegistryAccess` instead writes `<basename>.sd.json` and updates the registry to point at the sidecar. The reused `simlin_mcp_core::tools::edit_model` function therefore behaves differently depending on the `ProjectAccess` impl — not because the tool branches on format, but because the access trait abstracts the persistence policy.

**6. `simulate` tool — minimal, fresh `SimlinDb` per call.** `simlin-engine` does not expose a stateful "session" — every simulation call is self-contained from a `datamodel::Project`. The pattern:
```rust
let mut db = SimlinDb::default();
let sync = sync_from_datamodel_incremental(&mut db, &project, None);
let compiled = compile_project_incremental(&db, sync.project, &model_name)?;
let mut vm = Vm::new(compiled)?;
vm.run_to_end()?;
let results = vm.into_results();
```
The `simulate` tool's input accepts an optional `overrides: Vec<ModelOperation>` (reusing simlin-engine's existing `ProjectPatch`/`ModelOperation` types) and an optional `sim_specs_override: Option<SimSpecs>`. If overrides are provided, apply them via `apply_patch(&mut project_clone, &patch)?` before the sync — leaving the in-memory `LoroDoc` untouched (overrides don't persist).

**7. `Results` has no built-in JSON serializer.** The simulate tool authors its own serialization shape:
```json
{
  "time": [0.0, 1.0, 2.0, ...],
  "variables": {
    "<canonical_name>": [v0, v1, v2, ...],
    ...
  }
}
```
The output struct is small (~30 LoC) and lives in `simlin-serve/src/mcp/simulate.rs` — not in `simlin-engine`, since it's an MCP-specific shape. Iterate `results.offsets` for `(name, offset)` pairs, then for each variable extract the column from `results.iter()` (yielding row slices). The time column is at `TIME_OFF = 0` per `src/simlin-engine/src/results.rs:10`.

**8. Path canonicalization for MCP-supplied paths.** MCP clients pass paths as strings. We must reject path-traversal attempts (the same logic Phase 1 uses for HTTP path parameters). Specifically: paths must canonicalize to a descendant of `state.root`. If not, the tool returns `Err(McpError::invalid_params("path is outside the working directory"))`.

**9. `notifications_router` is Phase 7.** Phase 6 emits `WsMessage::ProjectChanged { source: ChangeSource::Agent, ... }` on the broadcast channel after every successful MCP edit. Phase 7 will add the actual MCP-notifications-router that translates these into `notifications/...` frames sent to subscribed MCP clients. For Phase 6, the broadcast already reaches the browser's WebSocket clients (the existing path from Phase 3); MCP clients see edits via tool-call return values until Phase 7.

**10. Capabilities declaration and the `notifications` capability myth.** There is no top-level `notifications` capability in MCP. Notifications are sub-capabilities under `tools` (`tool_list_changed`), `resources` (`resource_list_changed`, `resource_subscribe`), and `prompts`. The Phase 6 `get_info()` should declare `enable_tools()`, `enable_resources()`, `enable_logging()` — `tool_list_changed` and `resource_list_changed` are added in Phase 7 when those notifications actually fire.

**11. Claude Code vs Claude Desktop config.** Claude Code CLI has native `--transport http` support. Claude Desktop (as of April 2026) does NOT — it requires the `mcp-remote` npm proxy package. The README in this phase documents both forms; users on Desktop need a separate `npm install -g mcp-remote` first.

**12. `RegistryAccess::create` — full new-project path.** The `create_model` tool with the registry-backed access creates a new file on disk via `simlin_engine::io::atomic_write`, adds a fresh `ProjectMeta` to the registry, hydrates a `LoroDoc`, and broadcasts `ProjectChanged { source: Agent, version: 0 }`. This is also what Phase 8 polishes for the user-facing "Create new model" affordance — the underlying primitive is the same.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
### Subcomponent A: `RegistryAccess` — simlin-serve's `ProjectAccess` impl

<!-- START_TASK_1 -->
### Task 1: `RegistryAccess` struct and `open` method

**Verifies:** server-rewrite.AC5.4 (read-side state sharing)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/mod.rs`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/access.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (re-export `pub mod mcp;`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/Cargo.toml` (add `simlin-mcp-core = { path = "../simlin-mcp-core" }`, add `rmcp = { version = "1", features = ["server", "macros", "schemars", "transport-streamable-http-server-session"] }`)
- Test: inline `#[cfg(test)] mod tests` in `access.rs`

**Implementation:**
- `pub struct RegistryAccess { state: Arc<AppState> }` — holds the shared simlin-serve `AppState` (already contains `Arc<ProjectRegistry>` and `Arc<EventBus>`).
- `pub fn new(state: Arc<AppState>) -> Self`.
- `impl ProjectAccess for RegistryAccess`:
  - `async fn open(&self, abs_path: &Path) -> Result<OpenedProject, AccessError>`:
    1. Verify the path is a descendant of `state.root` (path-traversal check, mirroring the HTTP get_project handler's logic). Return `Err(AccessError::NotFound { path })` (or `IoError(ErrorKind::PermissionDenied)`) on failure.
    2. Call `state.registry.get_or_init_doc(abs_path)?` (Phase 3) → `Arc<ProjectDoc>`.
    3. Export the doc state: `let json_value = project_doc.export_canonical_json()?`.
    4. Convert to `datamodel::Project`: parse the JSON via `serde_json::from_value::<simlin_engine::json::Project>(...)?`, then `.into()`.
    5. Look up the registry's metadata for format and version: `let meta = state.registry.get(abs_path).ok_or(...)?;`.
    6. Return `OpenedProject { project, source_format: meta.format.into_source_format(), version: meta.version }`. (`ProjectFormat::Mdl` maps to `SourceFormat::Xmile` here — the design treats `.mdl` reads as XMILE-shaped content; if a sidecar exists, the registry's format is `SdJson` already because Phase 4 redirects sidecars on save.)

**Testing:**
- Unit test: setup tempdir with a fixture `.xmile`, build an `AppState` with a `ProjectRegistry` populated by `scan_into_registry`, call `RegistryAccess::open(absolute_path)`, assert returns `Ok(OpenedProject { project, .., version: 0 })`.
- Path-traversal test: pass a path with `..` segments → `Err(AccessError::NotFound)`.

**Verification:**
- `cargo test -p simlin-serve mcp::access::` passes.

**Commit:** `serve: RegistryAccess::open shares LoroDoc state with HTTP/UI`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `RegistryAccess::save` — Loro merge + sidecar logic + agent broadcast

**Verifies:** server-rewrite.AC4.1, server-rewrite.AC5.4, server-rewrite.AC6.3 (agent source)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/access.rs`
- Test: extend `tests/api_save.rs` with an "MCP save through RegistryAccess" test, OR new `tests/mcp_registry_access.rs`

**Implementation:**
- `async fn save(&self, abs_path: &Path, project: &datamodel::Project, format: SourceFormat, expected_version: Option<u64>) -> Result<u64, AccessError>`:
  1. Path-traversal check (same as `open`).
  2. Convert `project` → `json::Project` → `serde_json::Value` (the canonical JSON the Loro merge expects).
  3. Validate via simlin-engine diagnostics with baseline (reuse `validation::validate_save` from Phase 2). On new errors → `Err(AccessError::Validation { errors })`.
  4. Determine version expectation: `let version_check = expected_version.unwrap_or_else(|| state.registry.get(abs_path).map(|m| m.version).unwrap_or(0));`. (When the caller does not pass a version, we just fetch the current. This means MCP-initiated saves don't have intrinsic optimistic-locking from the AI's perspective; they read the latest in-memory state and write against it. Browser-initiated saves continue to pass `expected_version` from the form's last GET.)
  5. Call `state.registry.check_increment_and_merge(abs_path, version_check, &json_value)` → `(new_version, project_doc)`.
  6. **Resolve save target** (reuse Phase 2's `writer::resolve_save_target`):
     - `Xmile` (or .stmx/.xmile path) → in-place atomic write of `to_xmile(project)`.
     - `.mdl` path with `Xmile` format (the project content format is XMILE-shaped but the file extension is .mdl) → write `<basename>.sd.json` sidecar; update the registry to point at the sidecar (using `redirect_to_sidecar` from Phase 2).
     - `SdJson` (path is already a `.sd.json`) → in-place atomic write of pretty-printed JSON.
     - `NativeJson` is treated like `SdJson`.
  7. After the disk write succeeds: refresh registry meta + hash (Phase 4 helper).
  8. **Broadcast:** `state.events.publish(WsMessage::ProjectChanged { path: <relative>, version: new_version, source: ChangeSource::Agent })`.
  9. Return `Ok(new_version)`.

**Testing:**
Verifies AC4.1, AC5.4, AC6.3 (agent source).

- **AC5.4 round-trip:** in a single test:
  1. Initialize the registry with a fixture `.xmile`.
  2. Run two parallel "saves": one via the existing browser save handler (Phase 2/3), one via `RegistryAccess::save` directly. Both edit different stocks. After both complete, fetch the Loro doc state and assert both edits are present.
  3. Assert that the broadcast channel saw exactly two `ProjectChanged` events: one with `source: User`, one with `source: Agent`. Order is not guaranteed; both must be present.
- **AC6.3 (agent source):** Confirm the WebSocket subscriber from Phase 3 receives `source: "agent"` for MCP-initiated saves.
- **`.mdl` sidecar via MCP:** seed the registry with a `.mdl`-backed entry. Call `RegistryAccess::save` with format `Xmile` and an updated project. Assert: (a) the original `.mdl` is byte-unchanged, (b) `<basename>.sd.json` is created with the new state, (c) the registry now has the sidecar entry and not the `.mdl` entry.

**Verification:**
- `cargo test -p simlin-serve --test mcp_registry_access` passes.

**Commit:** `serve: RegistryAccess::save merges via Loro and broadcasts source=agent`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: `RegistryAccess::create` — new-model path

**Verifies:** none directly (foundation for AC5.3 create_model)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/access.rs`
- Test: extend `tests/mcp_registry_access.rs`

**Implementation:**
- `async fn create(&self, abs_path: &Path, project: &datamodel::Project, format: SourceFormat) -> Result<(), AccessError>`:
  1. Path-traversal check.
  2. If the file already exists → `Err(AccessError::IoError(io::Error::new(ErrorKind::AlreadyExists, ...)))`.
  3. Ensure parent dir exists: `tokio::fs::create_dir_all(parent).await?`.
  4. Serialize per format: `Xmile` → `to_xmile(project)` (caveat: extension on `abs_path` should be `.stmx` for native; `.xmile` is also accepted; `.mdl` is rejected here per simlin-mcp's existing semantics — actually for simlin-serve we could allow `.mdl` and write a sidecar, but Phase 1's discovery-driven model is "users supply .stmx files for new models" — keep `.mdl` rejection in `create` for simplicity; document the edge case).
  5. Serialize for `SdJson` / `NativeJson`: `serde_json::to_string_pretty(&json_project)`.
  6. `simlin_engine::io::atomic_write(abs_path, &bytes)`.
  7. Add a fresh `ProjectMeta` to the registry: format, mtime/size from `fs::metadata`, version `0`, hash `content_hash(&bytes)`. (No need to hydrate the `ProjectDoc` immediately — lazy hydration on the next `open` will populate.)
  8. Broadcast `ProjectChanged { source: Agent, version: 0 }` so the browser sidebar sees the new entry.

**Testing:**
- Unit test: call `RegistryAccess::create` with a tempdir-relative path and a hand-built `datamodel::Project` (use `TestProject` from simlin-engine if accessible, otherwise inline construction). Assert the file exists with parseable content and the registry has the new entry.
- Conflict test: try `create` on a path that already exists → `Err(AlreadyExists)`.

**Verification:**
- `cargo test -p simlin-serve --test mcp_registry_access` passes.

**Commit:** `serve: RegistryAccess::create writes file and registers entry`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
### Subcomponent B: `SimlinServeMcpServer` with all five tools

<!-- START_TASK_4 -->
### Task 4: `SimlinServeMcpServer<A>` skeleton + delegated tools

**Verifies:** server-rewrite.AC5.3 (tool surface)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/server.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/mod.rs` (re-export)
- Test: inline tests; new `tests/mcp_tool_surface.rs` for end-to-end via rmcp

**Implementation:**
- `#[derive(Clone)] pub struct SimlinServeMcpServer<A: ProjectAccess> { access: Arc<A>, state: Arc<AppState>, root: Arc<PathBuf>, tool_router: ToolRouter<Self> }`.
- Constructor `pub fn new(state: Arc<AppState>) -> Self where A = RegistryAccess`. Internally builds `RegistryAccess::new(state.clone())` and stores it as `Arc<A>`.
  - To keep generic (so unit tests can use `MockAccess`), define the constructor on the generic type and provide a `pub fn with_access(access: A, state: Arc<AppState>) -> Self`. The binary path uses the `RegistryAccess`-specific constructor.
- `#[tool_router] impl<A: ProjectAccess> SimlinServeMcpServer<A>`:
  - `read_model` — delegates to `simlin_mcp_core::tools::read_model::read_model(&*self.access, params).await`. Same `CallToolResult` envelope shape from Phase 5 (success + structured_content; error + is_error: true).
  - `edit_model` — delegates to `simlin_mcp_core::tools::edit_model::edit_model(&*self.access, params).await`.
  - `create_model` — delegates to `simlin_mcp_core::tools::create_model::create_model(&*self.access, params).await`.
- The `list_projects` and `simulate` tools are added in Tasks 5-6.
- `#[tool_handler] impl<A: ProjectAccess> ServerHandler for SimlinServeMcpServer<A>`:
  - `fn get_info(&self) -> ServerInfo`:
    ```rust
    ServerInfo {
        protocol_version: ProtocolVersion::LATEST,
        capabilities: ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()      // resources are sparse in simlin-serve; document the few we expose
            .enable_logging()
            .build(),
        server_info: Implementation {
            name: "simlin-serve".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        instructions: Some(format!(
            "Simlin model server. Operating on file://{}. Use list_projects to enumerate available models, then read_model / edit_model / simulate to interact with them.",
            self.root.display()
        )),
    }
    ```
  - `list_resources` / `read_resource`: simlin-serve does not expose its own MCP resources for V1 — return empty list. (If a use case for an MCP-exposed resource emerges post-V1 — e.g., the threat model document — file as a follow-up.)

**Testing:**
- Unit test: construct `SimlinServeMcpServer::with_access(MockAccess::new(), Arc::new(AppState::test()))`, call `get_info()`, assert protocol version, capabilities, instructions includes the root path.
- End-to-end via rmcp: in `tests/mcp_tool_surface.rs`, bind a real listener, mount `StreamableHttpService::new(...)`, spawn the server, then use rmcp's CLIENT side (`rmcp` has a `client` feature) to connect and call `read_model` against a tempdir-backed registry. Assert the response shape.

**Verification:**
- `cargo test -p simlin-serve mcp::server::` passes.
- `cargo test -p simlin-serve --test mcp_tool_surface` passes.

**Commit:** `serve: SimlinServeMcpServer rmcp ServerHandler with three delegated tools`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: `list_projects` tool — mirror `GET /api/projects`

**Verifies:** server-rewrite.AC5.3 (list_projects)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/list_projects.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/server.rs` (add `#[tool] async fn list_projects`)
- Test: extend `tests/mcp_tool_surface.rs`

**Implementation:**
- `pub struct ListProjectsInput {}` (no input fields). Derive `Deserialize`, `JsonSchema`. Use `Parameters<ListProjectsInput>` extractor for consistency even though there's no input.
- `pub struct ListProjectsOutput { pub projects: Vec<ProjectSummary>, pub git_available: bool, pub root: String }`.
- `pub struct ProjectSummary { pub path: String, pub format: String, pub git: GitState, pub version: u64 }` (drop `mtime`/`size` from the AI-facing surface — they're noise).
- `#[tool(description = "List all system-dynamics model files in the working directory tree, with their format and git status.")]` async fn calls `state.registry.snapshot()` (Phase 1) and converts each `ProjectMeta` to a `ProjectSummary`.
- The output's `root` field is the absolute path so the AI knows where it's operating (in addition to the `instructions` hint).

**Testing:**
- E2E via rmcp client: connect, call `list_projects`, assert the response carries the right number of entries with correct formats.

**Verification:**
- `cargo test -p simlin-serve --test mcp_tool_surface` passes the new test.

**Commit:** `serve: list_projects MCP tool mirrors GET /api/projects`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: `simulate` tool — runs simulation, returns time series

**Verifies:** server-rewrite.AC5.3 (simulate)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/simulate.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/server.rs` (add `#[tool] async fn simulate`)
- Test: extend `tests/mcp_tool_surface.rs`

**Implementation:**
- `pub struct SimulateInput { pub project_path: String, pub model_name: Option<String>, pub overrides: Option<Vec<simlin_mcp_core::types::EditOperation>>, pub sim_specs_override: Option<simlin_engine::datamodel::SimSpecs>, pub variables: Option<Vec<String>> }`. Derive `Deserialize`, `JsonSchema`.
   - `model_name` defaults to `"main"`.
   - `overrides` reuses simlin-mcp-core's `EditOperation` enum so the schema is consistent across `edit_model` and `simulate`.
   - `sim_specs_override` lets the AI run with different time bounds without persisting the change.
   - `variables` is an optional whitelist of variable names to include in the output (to keep responses small for large models). If `None`, return all variables.
- `pub struct SimulateOutput { pub time: Vec<f64>, pub variables: serde_json::Map<String, serde_json::Value> }` where each variable maps to a `serde_json::Value::Array(Vec<f64>)`.
- Implementation steps inside the async fn:
  1. `let opened = self.access.open(Path::new(&input.project_path)).await?;`
  2. Clone `opened.project` into a mutable `datamodel::Project`.
  3. If `input.overrides` provided: build a `ProjectPatch` from the operations (reuse the same logic as `edit_model`'s patch construction, factored into `simlin_mcp_core::tools::patch_builder` if not already there), then `simlin_engine::patch::apply_patch(&mut project, &patch)?`. Failures → `Err(McpError::invalid_params(...))`.
  4. If `sim_specs_override` provided: assign to `project.sim_specs`.
  5. Run the simulation. The pipeline (verified entry-point line numbers in `src/simlin-engine/`):
     - `simlin_engine::db::SimlinDb::default()` (no construction args)
     - `simlin_engine::db::sync_from_datamodel_incremental` (`db.rs:2636`) — takes `&mut db, &project, prev: Option<...>`
     - `simlin_engine::db::compile_project_incremental` (`db.rs:5902`) — takes `&db, source_project, model_name: &str`
     - `simlin_engine::vm::Vm::new(compiled)` (`vm.rs:506`)
     - `vm.run_to_end()` (`vm.rs:596`) returns `Result<()>`
     - `vm.into_results()` (`vm.rs:864`) consumes the VM and returns `Results`
     ```rust
     let mut db = SimlinDb::default();
     let sync = sync_from_datamodel_incremental(&mut db, &project, None);
     let model_name = input.model_name.as_deref().unwrap_or("main");
     let compiled = compile_project_incremental(&db, sync.project, model_name)
         .map_err(|e| McpError::internal_error(format!("compile error: {e}")))?;
     let mut vm = Vm::new(compiled).map_err(|e| McpError::internal_error(format!("vm error: {e}")))?;
     vm.run_to_end().map_err(|e| McpError::internal_error(format!("sim error: {e}")))?;
     let results = vm.into_results();
     ```
     The exact `Result` error types are `simlin_engine::common::Error`; map them to `McpError` via the format strings above.
  6. Build the output: extract the time column (`offset = 0`), then iterate through `results.offsets` extracting each variable's column, filtered by `input.variables` if provided.
  7. Wrap in `CallToolResult::structured(serde_json::to_value(&output)?)`.
- Long-running simulations: if a real model takes >10s, this blocks the rmcp executor. For Phase 6, accept this — wrap in `tokio::task::spawn_blocking` only if profiling shows it's needed. Document the trade-off.

**Testing:**
- E2E test: setup tempdir with the `teacup.xmile` fixture, call `simulate` with `project_path`, no overrides, no variables filter. Assert: `time` has the expected number of steps, `variables` includes `teacup_temperature` (or whatever the canonical name is in the fixture) with a sensible time series.
- Override test: same fixture, override the initial temperature stock, assert the resulting time series differs from the baseline.
- Variables filter test: pass `variables: Some(vec!["time".into(), "teacup_temperature".into()])`, assert only those two appear in the output.

**Verification:**
- `cargo test -p simlin-serve --test mcp_tool_surface` passes the new tests.

**Commit:** `serve: simulate MCP tool runs sim and returns time-series JSON`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 7-9) -->
### Subcomponent C: HTTP transport mounting + dual-port serving + port-conflict handling

<!-- START_TASK_7 -->
### Task 7: Build the MCP `axum::Router` with `StreamableHttpService`

**Verifies:** server-rewrite.AC5.1 (server exists at /mcp)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/transport.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/mod.rs` (re-export)

**Implementation:**
- `pub fn build_mcp_router(state: Arc<AppState>) -> axum::Router`:
  ```rust
  let factory = move || Ok(SimlinServeMcpServer::<RegistryAccess>::new(Arc::clone(&state)));
  let mcp_service = StreamableHttpService::new(
      factory,
      Arc::new(LocalSessionManager::default()),
      StreamableHttpServerConfig::default(),
  );
  axum::Router::new().nest_service("/mcp", mcp_service)
  ```
- The factory closure constructs a fresh `SimlinServeMcpServer` per session — but all sessions share the same `Arc<AppState>` (and hence the same `ProjectRegistry`).
- Optionally apply `tower_http::trace::TraceLayer::new_for_http()` so MCP requests show in `tracing` output.

**Testing:**
- Unit test: call `build_mcp_router(test_state)`, hit `/mcp` with a basic HTTP request via `tower::ServiceExt::oneshot`, expect a non-404 response (the precise rmcp upgrade negotiation is opaque; just assert the route is mounted).

**Verification:**
- `cargo test -p simlin-serve mcp::transport::` passes.

**Commit:** `serve: build_mcp_router with rmcp StreamableHttpService at /mcp`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Dual-port serving with `tokio::try_join!` and port-conflict diagnostics

**Verifies:** server-rewrite.AC5.1 (binds on 7878 by default), server-rewrite.AC5.5 (port-conflict)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs` (`main()` now binds two listeners and joins them)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/serving.rs` (helper `bind_or_die`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (re-export `pub mod serving;`)

**Implementation:**
- `pub async fn bind_or_die<A: ToSocketAddrs>(addr: A, label: &str, port_hint: Option<&str>) -> anyhow::Result<TcpListener>`:
  ```rust
  TcpListener::bind(addr).await.map_err(|e| {
      if e.kind() == ErrorKind::AddrInUse {
          let hint = port_hint.map(|h| format!(" Pass {h} to use a different port.")).unwrap_or_default();
          anyhow::anyhow!("Cannot start {label}: address already in use.{hint}")
      } else {
          anyhow::anyhow!("Cannot start {label}: {e}")
      }
  })
  ```
- `main()` post-Phase-1 plumbing:
  ```rust
  let ui_listener = bind_or_die((127.0.0.1, args.port), "HTTP/UI server", None).await?;
  let mcp_listener = bind_or_die((127.0.0.1, args.mcp_port), "MCP server", Some("--mcp-port")).await?;
  let ui_port = ui_listener.local_addr()?.port();
  let mcp_port = mcp_listener.local_addr()?.port();

  // Print URLs:
  println!("Simlin Serve");
  println!("  UI:  http://127.0.0.1:{ui_port}/?token=<token>");
  println!("  MCP: http://127.0.0.1:{mcp_port}/mcp");

  // Build routers:
  let ui_router = build_router(state.clone()); // existing from Phases 1-3
  let mcp_router = build_mcp_router(state.clone());

  // Open browser, etc., as before...

  // Serve both:
  let ui_serve = axum::serve(ui_listener, ui_router).with_graceful_shutdown(shutdown_signal());
  let mcp_serve = axum::serve(mcp_listener, mcp_router).with_graceful_shutdown(shutdown_signal());
  tokio::try_join!(ui_serve, mcp_serve)?;
  ```
- Both shutdowns are gated by the same Ctrl-C signal from Phase 4.

**Testing:**
- Integration test (`tests/dual_port_smoke.rs`): start the binary in a subprocess (via `Command::new(env!("CARGO_BIN_EXE_simlin-serve"))`), wait for stdout to print both URLs (parse the printed lines), then `curl` `/healthz` on the UI port and `/mcp` on the MCP port. Both should respond.
- Port-conflict test: bind a `TcpListener` on `127.0.0.1:7878` from the test, then try to start the binary with the default `--mcp-port`. Expect non-zero exit status and stderr containing "address already in use" + "--mcp-port" hint. Use `Command::output()` to capture.

**Verification:**
- `cargo test -p simlin-serve --test dual_port_smoke` passes.

**Commit:** `serve: dual-port serving with port-conflict diagnostics`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: End-to-end test — MCP edit triggers browser update within 1s

**Verifies:** server-rewrite.AC5.3 (round-trip), server-rewrite.AC4.1 (closeout)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/e2e_mcp_browser.rs`

**Implementation:**
- The end-to-end scenario:
  1. Start the binary in a tempdir with one `.xmile` fixture.
  2. Connect a WebSocket client to the UI port (the simulated browser).
  3. Connect an rmcp HTTP client to the MCP port (the simulated AI).
  4. Via MCP, call `edit_model` with an upsert op for an aux variable.
  5. Within 1 second, the WebSocket client should receive `ProjectChanged { source: "agent", version: 1 }`.
  6. Via MCP, call `read_model` — assert the response includes the new variable.
- Time budget is the AC5.3 "within one second" requirement. Use `tokio::time::timeout(Duration::from_secs(2), ws.next())` to give some headroom; assert it didn't fire.

**Verification:**
- `cargo test -p simlin-serve --test e2e_mcp_browser` passes.

**Commit:** `serve: e2e test verifies MCP edit propagates to browser in <1s`
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (task 10) -->
### Subcomponent D: Documentation and verification

<!-- START_TASK_10 -->
### Task 10: README — `mcp.json` snippets for Claude Code and Claude Desktop

**Verifies:** server-rewrite.AC5.3 (documented)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/README.md`

**Implementation:**
- Add a "Configuring AI clients" section. Two subsections:
  
  **Claude Code CLI (recommended):**
  ```bash
  # Add the simlin-serve MCP server in the user scope:
  claude mcp add --transport http --scope user simlin-serve http://127.0.0.1:7878/mcp
  
  # Or, in the current project's .mcp.json (project scope, checks into git):
  cat > .mcp.json <<'EOF'
  {
    "mcpServers": {
      "simlin-serve": {
        "type": "http",
        "url": "http://127.0.0.1:7878/mcp"
      }
    }
  }
  EOF
  ```
  
  **Claude Desktop (requires `mcp-remote` proxy):**
  ```bash
  # Install mcp-remote globally:
  npm install -g mcp-remote
  
  # Edit Claude Desktop's config file:
  #   macOS:   ~/Library/Application Support/Claude/claude_desktop_config.json
  #   Windows: %APPDATA%\Claude\claude_desktop_config.json
  ```
  ```json
  {
    "mcpServers": {
      "simlin-serve": {
        "command": "npx",
        "args": ["mcp-remote", "http://127.0.0.1:7878/mcp"]
      }
    }
  }
  ```
- Document the prerequisite that `simlin-serve` must be running for the AI client to connect.
- Note that the URL is stable across launches (port `7878` by default), so the configuration is one-time.

**Testing:** None (documentation).

**Verification:**
- README parses as valid markdown (manual or `markdownlint`).
- Manually exercise both client paths: Claude Code CLI's `claude mcp add` succeeds; Claude Desktop's config + `mcp-remote` chain works (out-of-band by the executor; document the steps and any gotchas they hit).

**Commit:** `serve: README documents mcp.json snippets for Claude Code and Desktop`
<!-- END_TASK_10 -->
<!-- END_SUBCOMPONENT_D -->

---

## Phase Verification Checklist

Before marking Phase 6 complete:

1. `cargo test --workspace` passes.
2. `cargo clippy --workspace -- -D warnings` clean.
3. `cargo fmt --workspace --check` clean.
4. **Manual two-client test:** start `simlin-serve` against a directory with one model. Open the browser (UI on ephemeral port). Configure Claude Code CLI per the README. From the terminal: `claude` (in REPL), ask "List the models in this directory". Verify the AI calls `list_projects`, sees the model. Ask it to "set the initial value of the X stock to 100". Verify the AI calls `edit_model`, the browser editor remounts within ~1s with the new value.
5. **Concurrent edit test:** with the AI session live, edit a different variable in the browser. Save in the browser. The AI's next read should see both changes (the AI's edit and the browser's edit). No data loss.
6. **Port-conflict test:** start a second `simlin-serve` instance with `--mcp-port 7878`. Expect immediate exit with the friendly error.
7. **`@simlin/mcp` regression:** `cargo test -p simlin-mcp` still passes (Phase 5's library hasn't been broken).

If all 7 verifications pass, Phase 6 is done.
