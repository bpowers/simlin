# Test Requirements: simlin-serve V1

This document maps every server-rewrite acceptance criterion to either an automated test (with file path + test type) or human verification (with justification + approach). The test-analyst agent uses this during execution to validate that the implemented test coverage matches the criteria.

## Conventions

- Test types: `unit` (in-crate Rust `#[cfg(test)] mod tests`), `integration` (Rust `tests/*.rs`), `frontend-unit` (Jest), `e2e` (binary smoke), `manual` (human verification).
- File paths reference the implementation tasks (Phase N Task M) that produce them. A single AC may cite multiple test files when coverage is layered (server-side merge invariant + UI surface + composition smoke).
- The end-to-end smoke test at `/home/bpowers/src/simlin/src/simlin-serve/tests/smoke.rs` (Phase 8 Task 11) covers many ACs as a composition; it appears as a secondary citation on rows where a focused test already exists.
- Per-platform CI coverage for `tests/smoke.rs` runs on `macos-latest`, `ubuntu-latest`, and `windows-latest` (Phase 8 Task 12, `smoke-test` matrix job in `ci.yaml`). A row that cites `smoke.rs` plus a focused test means: the focused test is the primary correctness check; the smoke test additionally proves cross-platform composition.
- Run-on-demand: `cargo test --release --test smoke -- --ignored` locally; the CI matrix passes the same flags. The `#[ignore]` attribute keeps it out of the default `cargo test` budget.

## Automated Tests

### server-rewrite.AC1: Discovery and listing

Coverage is layered: walker-level tests prove the recursion + exclusion semantics, HTTP-level tests prove the API surface mirrors them, and frontend tests prove the UI renders what the API returns. AC1.4 picks up additional coverage from Phase 8 once the create-new-model affordance lands.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC1.1 | `.stmx`/`.xmile`/`.mdl` files at the root are discovered with correct formats | `/home/bpowers/src/simlin/src/simlin-serve/tests/discovery_integration.rs` | integration | Phase 1 Task 5 |
| AC1.1 | HTTP `GET /api/projects` returns all three formats in the registry snapshot | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_projects.rs` | integration | Phase 1 Task 8 |
| AC1.1 | UI: `<ProjectList>` renders one row per project for mocked snapshot | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.test.tsx` | frontend-unit | Phase 1 Task 12 |
| AC1.1 | Composition: smoke test asserts a tempdir with `.xmile` + `.mdl` + `.sd.json` lists 4 entries via `GET /api/projects` | `/home/bpowers/src/simlin/src/simlin-serve/tests/smoke.rs` | e2e | Phase 8 Task 11 |
| AC1.2 | Recursive walk surfaces `sub/d.stmx` with relative path display | `/home/bpowers/src/simlin/src/simlin-serve/tests/discovery_integration.rs` | integration | Phase 1 Task 5 |
| AC1.2 | HTTP response uses forward-slash relative paths | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_projects.rs` | integration | Phase 1 Task 8 |
| AC1.3 | Files inside `node_modules/`, `.git/`, `target/` are excluded by `ignore` walker | `/home/bpowers/src/simlin/src/simlin-serve/tests/discovery_integration.rs` | integration | Phase 1 Task 5 |
| AC1.4 | Empty-directory snapshot produces empty `projects` array; `<EmptyState>` rendered | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.test.tsx` | frontend-unit | Phase 1 Task 12 |
| AC1.4 | `POST /api/projects/new` creates a new model file in an empty tempdir | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` | integration | Phase 8 Task 4 |
| AC1.4 | `<NewProjectButton>` form posts and triggers navigation on success | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/NewProjectButton.test.tsx` | frontend-unit | Phase 8 Task 5 |
| AC1.5 | Symlinked directory cycle does not loop (Unix-only via `#[cfg(unix)]`) | `/home/bpowers/src/simlin/src/simlin-serve/tests/discovery_integration.rs` | integration | Phase 1 Task 5 |

### server-rewrite.AC2: Git status reporting

Coverage relies on real git repos in tempdirs (the `git_integration.rs` and `watcher_git.rs` tests run `git init` / `git add` / `git commit` via `Command`). Tests skip locally when `git` is absent from PATH but fail loudly in CI per project policy. The UI surface tests use mocked snapshots since the chip-rendering logic is independent of how the server computed the state.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC2.1 | Tracked clean file in tempdir git repo returns `Tracked { dirty: false }` | `/home/bpowers/src/simlin/src/simlin-serve/tests/git_integration.rs` | integration | Phase 1 Task 6 |
| AC2.1 | UI chip renders the "version controlled" state for tracked entries | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.test.tsx` | frontend-unit | Phase 1 Task 12 |
| AC2.2 | Modified file after commit returns `Tracked { dirty: true }` | `/home/bpowers/src/simlin/src/simlin-serve/tests/git_integration.rs` | integration | Phase 1 Task 6 |
| AC2.2 | UI chip renders the "modified" state | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.test.tsx` | frontend-unit | Phase 1 Task 12 |
| AC2.3 | File outside any `.git` working tree returns `Untracked`; HTTP surface mirrors | `/home/bpowers/src/simlin/src/simlin-serve/tests/git_integration.rs` and `tests/api_projects.rs` | integration | Phase 1 Tasks 6, 8 |
| AC2.3 | UI surfaces "not under version control" warning state | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.test.tsx` | frontend-unit | Phase 1 Task 12 |
| AC2.4 | Watcher event on a tracked file flips git state from "modified" to "tracked clean" within 500ms | `/home/bpowers/src/simlin/src/simlin-serve/tests/watcher_git.rs` | integration | Phase 4 Task 7 |
| AC2.5 | Stubbed `GitProbe { git_available: false }` returns `Unavailable` for every entry | `/home/bpowers/src/simlin/src/simlin-serve/src/git.rs` (inline `mod tests`) | unit | Phase 1 Task 6 |
| AC2.5 | HTTP response carries `git_available: false`; UI banner renders + dismisses to sessionStorage | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_projects.rs` and `web/src/components/ProjectList.test.tsx` | integration + frontend-unit | Phase 1 Tasks 8, 12 |

### server-rewrite.AC3: Editing round-trip

Tests fall in three layers: (1) endpoint-level integration tests in `tests/api_save.rs` and `tests/api_get_project.rs` against a tempdir-backed `AppState`, (2) byte-stability and validation unit tests against the writer / registry, (3) frontend tests for the `<EditorHost>` save/conflict/redirect flows. The smoke test exercises one full round-trip per format end-to-end.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC3.1 | `GET /api/projects/{*path}` returns canonical JSON for `.stmx` and `.xmile` fixtures | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_get_project.rs` | integration | Phase 1 Task 9 |
| AC3.1 | `<EditorHost>` renders the diagram for a mocked successful fetch | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx` | frontend-unit | Phase 1 Task 12, Phase 2 Task 8 |
| AC3.2 | Save round-trip: POST writes XMILE to disk; re-read parses to mutated state | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` | integration | Phase 2 Task 5 |
| AC3.2 | XMILE writer is byte-stable for two saves of the same project | `/home/bpowers/src/simlin/src/simlin-serve/src/writer.rs` (inline `mod tests`) | unit | Phase 2 Task 5 |
| AC3.3 | `GET /api/projects/{*.mdl}` parses via `simlin_engine::open_vensim` and returns JSON | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_get_project.rs` | integration | Phase 1 Task 9 |
| AC3.4 | Save against `.mdl` writes `<basename>.sd.json` sidecar; original `.mdl` byte-unchanged | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` | integration | Phase 2 Task 6 |
| AC3.4 | UI: `<EditorHost>` follows sidecar-redirect path on save response | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx` | frontend-unit | Phase 2 Task 8 |
| AC3.5 | After a `.mdl` save creates a sidecar, GET on the original `.mdl` returns sidecar content | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` and `tests/api_get_project.rs` | integration | Phase 1 Task 9, Phase 2 Task 6 |
| AC3.6 | Stale-version POST returns 409 Conflict with the actual current version | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` | integration | Phase 2 Tasks 2, 4 |
| AC3.6 | Concurrency: two threads racing `check_and_increment` produce exactly one Ok and one VersionMismatch | `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (inline `mod tests`) | unit | Phase 2 Task 2 |
| AC3.6 | UI: 409 path triggers refetch and `onConflict` re-render | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx` | frontend-unit | Phase 2 Task 9 |
| AC3.2 | Composition: smoke test mutates `teacup.xmile` via POST then re-parses the on-disk bytes | `/home/bpowers/src/simlin/src/simlin-serve/tests/smoke.rs` | e2e | Phase 8 Task 11 |

### server-rewrite.AC4: Concurrent editing via Loro

This is the most layered AC group. AC4.1 progresses across three phases (3 = browser↔browser, 4 = browser↔disk via merge, 6 = browser↔MCP). The watcher tests drive real `tokio::fs::write` from a sibling task and assert the disk-source `ProjectChanged` envelope arrives on a real WS subscriber within 500ms. AC4.4's hash short-circuit is verified by the negative case: an own-write should NOT cause a re-broadcast.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC4.1 | Two near-simultaneous browser saves merge without data loss; both edits present after second `ProjectChanged` | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` and `tests/e2e_live_update.rs` | integration | Phase 3 Tasks 8, 12 |
| AC4.1 | Project-shape merge invariant: per-variable LWW preserves both edits to different stocks | `/home/bpowers/src/simlin/src/simlin-serve/src/loro_doc.rs` (inline `mod tests`) | unit | Phase 3 Task 4 |
| AC4.1 | Browser save and MCP `RegistryAccess::save` running in parallel both land; bus emits one `User` and one `Agent` event | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_registry_access.rs` | integration | Phase 6 Task 2 |
| AC4.1 | End-to-end: MCP edit visible in browser within 1s of `ProjectChanged` | `/home/bpowers/src/simlin/src/simlin-serve/tests/e2e_mcp_browser.rs` | integration | Phase 6 Task 9 |
| AC4.2 | External `tokio::fs::write` triggers `ProjectChanged { source: "disk" }` on the WS within 500ms; subsequent GET returns new state | `/home/bpowers/src/simlin/src/simlin-serve/tests/watcher_merge.rs` | integration | Phase 4 Task 5 |
| AC4.3 | In-flight registry edit + external disk edit to a different stock both present after merge | `/home/bpowers/src/simlin/src/simlin-serve/tests/watcher_merge.rs` | integration | Phase 4 Task 5 |
| AC4.4 | Server's own `atomic_write` does not echo a second `ProjectChanged { source: "disk" }` (hash short-circuit) | `/home/bpowers/src/simlin/src/simlin-serve/tests/watcher_merge.rs` | integration | Phase 4 Task 5 |
| AC4.4 | `content_hash` is stable and distinguishes different inputs | `/home/bpowers/src/simlin/src/simlin-serve/src/hashing.rs` (inline `mod tests`) | unit | Phase 4 Task 1 |

### server-rewrite.AC5: In-process MCP

The MCP surface is exercised at three levels: (1) unit tests against the rmcp `ServerHandler` impl with a `MockAccess`, (2) integration tests using rmcp's CLIENT-side helpers (`tests/mcp_tool_surface.rs`) against a real bound listener mounted with `RegistryAccess`, (3) the `dual_port_smoke.rs` integration test that spawns the actual binary and confirms both listeners answer their respective routes. AC5.5's port-conflict diagnostic captures the binary's stderr after pre-binding 7878 from the test process.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC5.1 | `/mcp` is mounted on the dedicated MCP router; `tower::oneshot` returns non-404 | `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/transport.rs` (inline `mod tests`) | unit | Phase 6 Task 7 |
| AC5.1 | Spawned binary prints both UI and MCP URLs; `/healthz` answers on the UI port and `/mcp` answers on the MCP port (default 7878) | `/home/bpowers/src/simlin/src/simlin-serve/tests/dual_port_smoke.rs` | integration | Phase 6 Task 8 |
| AC5.2 | `get_info()` returns `instructions` containing the working-directory hint as `file://<root>`; capabilities declare tools/resources | `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/server.rs` (inline `mod tests`) | unit | Phase 6 Task 4 |
| AC5.3 | rmcp client successfully invokes `read_model`, `edit_model`, `create_model`, `list_projects`, `simulate` against a tempdir-backed registry | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_tool_surface.rs` | integration | Phase 6 Tasks 4, 5, 6 |
| AC5.3 | `simulate` returns time-series JSON with `time` and per-variable arrays; override and variables-filter modes both work | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_tool_surface.rs` | integration | Phase 6 Task 6 |
| AC5.3 | MCP `edit_model` propagates to the browser WebSocket within 1s | `/home/bpowers/src/simlin/src/simlin-serve/tests/e2e_mcp_browser.rs` | integration | Phase 6 Task 9 |
| AC5.4 | MCP-initiated `RegistryAccess::save` and HTTP `save_project` both flow through `apply_canonical_json`; final state contains both edits regardless of order | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_registry_access.rs` | integration | Phase 6 Task 2 |
| AC5.4 | `RegistryAccess::open` returns the same in-memory state seen by the HTTP `GET /api/projects/{*path}` handler | `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/access.rs` (inline `mod tests`) | unit | Phase 6 Task 1 |
| AC5.5 | Pre-binding 7878 from a test then spawning the binary produces a non-zero exit with stderr containing "address already in use" + `--mcp-port` hint | `/home/bpowers/src/simlin/src/simlin-serve/tests/dual_port_smoke.rs` | integration | Phase 6 Task 8 |
| AC5.3 | Composition: smoke test posts a JSON-RPC `tools/call` for `read_model` against `/mcp` and a `create_model` for a new file, then verifies the file on disk | `/home/bpowers/src/simlin/src/simlin-serve/tests/smoke.rs` | e2e | Phase 8 Task 11 |

### server-rewrite.AC6: MCP push notifications

Each notification kind is verified at three levels: (1) WS frame envelope round-trip on the EventBus (`events::tests` and `tests/ws_updates.rs`), (2) the production path that triggers the notification (browser save handler, `RegistryAccess::save`, watcher merge, `validate_save`), and (3) MCP-client-side delivery via `tests/mcp_tool_surface.rs` using rmcp's notification listener. The ordering caveat (notifications may arrive before or after a tool response — see Phase 7 Notes #3) is documentation-only and not test-asserted.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC6.1 | Inbound `projectFocused` WS frame from the browser publishes on the EventBus | `/home/bpowers/src/simlin/src/simlin-serve/tests/ws_updates.rs` | integration | Phase 7 Task 3 |
| AC6.1 | `<EditorHost>` emits `projectFocused` on mount and on path change via `UpdatesSocket.send` | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx` | frontend-unit | Phase 7 Task 4 |
| AC6.1 | Per-session forwarder delivers `simlin/projectFocused` notification to a connected rmcp client | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_tool_surface.rs` | integration | Phase 7 Task 8 |
| AC6.2 | `<Editor>`'s `onSelectionChanged` prop fires with canonical idents on selection change | `/home/bpowers/src/simlin/src/diagram/Editor.test.tsx` | frontend-unit | Phase 7 Task 2 |
| AC6.2 | `<EditorHost>` debounces selection changes to 150ms and sends `selectionChanged` once with latest idents | `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx` | frontend-unit | Phase 7 Task 5 |
| AC6.2 | Inbound `selectionChanged` WS frame publishes on the EventBus | `/home/bpowers/src/simlin/src/simlin-serve/tests/ws_updates.rs` | integration | Phase 7 Task 3 |
| AC6.2 | rmcp client receives `simlin/selectionChanged` notification with `path` + `variableIdents` | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_tool_surface.rs` | integration | Phase 7 Task 8 |
| AC6.3 | Browser save broadcasts `ProjectChanged { source: "user" }` on the WS within 1s | `/home/bpowers/src/simlin/src/simlin-serve/tests/api_save.rs` and `tests/e2e_live_update.rs` | integration | Phase 3 Tasks 8, 12 |
| AC6.3 | MCP `RegistryAccess::save` broadcasts `ProjectChanged { source: "agent" }` | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_registry_access.rs` | integration | Phase 6 Task 2 |
| AC6.3 | Watcher disk-edit broadcasts `ProjectChanged { source: "disk" }` | `/home/bpowers/src/simlin/src/simlin-serve/tests/watcher_merge.rs` | integration | Phase 4 Task 5 |
| AC6.3 | Per-session forwarder maps EventBus `ProjectChanged` to `simlin/projectChanged` notifications for all three sources | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_tool_surface.rs` | integration | Phase 7 Task 8 |
| AC6.4 | Save that introduces a new validation error emits `DiagnosticsChanged` with the formatted error list; subsequent fix emits empty list; diagnostically-equivalent save emits no `DiagnosticsChanged` | `/home/bpowers/src/simlin/src/simlin-serve/tests/diagnostics_events.rs` | integration | Phase 7 Task 7 |
| AC6.4 | rmcp client receives `simlin/diagnosticsChanged` for the same scenarios | `/home/bpowers/src/simlin/src/simlin-serve/tests/mcp_tool_surface.rs` | integration | Phase 7 Task 8 |

### server-rewrite.AC7: Distribution and bootstrap

AC7.1 splits across three test artifacts: package shape (npm package generator output), workflow shape (`serve-release.yml` parsing), and runtime correctness (the `smoke.rs` test running on three platforms in CI). AC7.2 and AC7.4 are necessarily manual — see the Human Verification section. AC7.3 has a server-side enforcement test (Phase 3 Task 7) that is the primary correctness check; the Phase 1 Task 15/16 tests cover token issuance and SPA storage.

| AC | Test | File | Type | Phase/Task |
|----|------|------|------|------------|
| AC7.1 | Per-platform npm package generator produces correctly-shaped `package.json` files for all four shipped platforms | `/home/bpowers/src/simlin/src/simlin-serve/tests/build_npm_packages.rs` | integration | Phase 1 Tasks 18, 19 |
| AC7.1 | `serve-release.yml` workflow has the documented trigger pattern, matrix entries, and publish order | `/home/bpowers/src/simlin/src/simlin-serve/tests/serve_release_workflow.rs` | integration | Phase 1 Task 21 |
| AC7.1 | Cross-platform smoke test boots the binary, exercises HTTP and MCP paths, verifies disk state on macOS arm64, Linux x64, Windows x64 | `/home/bpowers/src/simlin/src/simlin-serve/tests/smoke.rs` (CI matrix `smoke-test` job) | e2e | Phase 8 Tasks 11, 12 |
| AC7.3 | One-time launch token is 43 URL-safe base64 characters with 256 bits of entropy and is unique across calls | `/home/bpowers/src/simlin/src/simlin-serve/src/token.rs` (inline `mod tests`) | unit | Phase 1 Task 16 |
| AC7.3 | SPA reads `?token=` from the URL, stores it in sessionStorage, and removes it from the visible URL | `/home/bpowers/src/simlin/src/simlin-serve/web/src/main.tsx` (Jest test in same dir) and `web/src/api.ts` (Jest) | frontend-unit | Phase 1 Task 15 |
| AC7.3 | Server-side bearer enforcement: WS upgrade with correct token succeeds; bad/missing token returns 401/400; constant-time compare | `/home/bpowers/src/simlin/src/simlin-serve/tests/ws_updates.rs` | integration | Phase 3 Task 7 |

## Human Verification

These criteria cannot be fully automated and require manual verification:

| AC | Verification approach | Justification |
|----|----------------------|---------------|
| AC7.2 | After `cargo run -p simlin-serve`, confirm the default browser opens to the printed UI URL on macOS, Linux (with DISPLAY), and Windows. Confirm both the HTTP UI URL and the MCP URL are printed to stdout once Phase 6 is in place. | The `open` crate hands off to `open` (macOS), `xdg-open` (Linux), or `start` (Windows), all of which depend on the user's OS-level default-browser registration and a live windowing system. CI runners and headless test environments do not register default browsers; emulating an actual browser launch in CI would require virtual displays (`Xvfb`), and would still not exercise the OS-specific launcher binaries. The URL-printing portion is testable in `tests/smoke.rs`, but the actual browser-window appearance must be eyeballed. The Phase 1 Task 17 verification block flags this as manual. |
| AC7.4 | Manually run `unset DISPLAY && cargo run -p simlin-serve` on Linux (or rename `xdg-open` out of PATH) and confirm the URL is printed prominently and the server keeps running. Repeat with `--no-open` to confirm the explicit headless mode. | The fallback path triggers when `open::that(url)` returns an error. Reproducing a "no opener available" environment in CI is fragile (each platform fails differently — Linux without `DISPLAY`, macOS in a no-GUI session, Windows Server Core); the URL-printing path is testable via the smoke test but the actual "no-opener present, server keeps running" behavior is best confirmed by a human with environmental control. The Phase 1 Task 17 verification block flags this as manual. |

