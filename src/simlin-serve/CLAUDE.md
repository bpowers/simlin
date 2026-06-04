# simlin-serve

Local-first HTTP server + React SPA + in-process MCP server. Distributed as the `@simlin/serve` npm package; running it opens any directory containing SD models in a browser tab and an AI client.

<!-- Last reviewed: 2026-04-26 -->

## Architecture

`simlin-serve` is one binary that binds two ports on `127.0.0.1`:

1. **UI port** (default ephemeral) -- Axum HTTP router serving the routes "/healthz", "/api/projects" (list/get/save/create), "/api/updates" (WebSocket), and a SPA fallback.
2. **MCP port** (default `7878`) -- rmcp `StreamableHttpService` mounted at the route "/mcp". Stable across launches so AI client configs don't drift.

V1 is intended for **single-user workstations** (a developer running `npx` `@simlin/serve` from a terminal on their laptop). The trust boundary is the OS user account; both routers rely on the loopback bind plus the host/origin allowlist for cross-origin defense, with no bearer-token gate. See `docs/threat-model.md` for the full design and what is explicitly out of scope (multi-user shared hosts).

Both ports share one `AppState` (registry, git probe, root, event bus, port numbers, strict-origin flag). The same in-memory `LoroDoc` per project backs the UI save handler and the MCP `EditModel` tool; concurrent edits from either side merge instead of clobbering.

A long-lived watcher actor observes the directory tree, classifies events (model change, model removal, git status change), and merges external edits through the same `apply_canonical_json` primitive the HTTP and MCP paths use.

## Files

### Crate root
- `src/lib.rs` -- `build_router(state)` composes the UI router with `host_validator` -> `RequestBodyLimitLayer` (16 MiB) -> `TraceLayer`. Registration order matters: the "/api/projects/new" route must precede the "/api/projects/{*rel_path}" wildcard so the Axum matcher dispatches POST-to-create before POST-to-save against a file literally named `new`.
- `src/main.rs` -- Composes the binary: scans the root, binds both listeners up front (so port-conflict diagnoses surface early), constructs `AppState`, prints the launch URL, optionally opens a browser, spawns the watcher and MCP servers under a single Ctrl-C shutdown signal.

### Domain modules (under `src/`)
- `cli.rs` -- `clap` `Args` (`root`, `--port`, `--mcp-port`, `--no-open`, `--strict-origin`).
- `discovery.rs`, `scan.rs` -- Recursive walk via `ignore` (respects `.gitignore`, excludes `node_modules` and `target`); `classify_extension` decides which paths register as projects.
- `parse.rs` -- Format-detection + parse to `datamodel::Project`. Distinguishes `.stmx`/`.xmile`/`.xml`, `.mdl`, `.sd.json` by extension first, then by content.
- `path_resolution.rs` -- Shared primitives every consumer (HTTP handlers, MCP `RegistryAccess`, watcher) must call instead of reimplementing inline. Exports `resolve_existing_within_root` (canonicalize a leaf and confirm it's inside the registry root); `resolve_canonical_path` (walk up to deepest existing ancestor and re-attach the lexical remainder, used both for write targets whose leaf doesn't yet exist *and* for watcher events whose leaf has just disappeared — it also masks the macOS FSEvents quirk of reporting paths through an unresolved symlink alias of the canonicalized root); `resolve_create_target` (write-side alias of `resolve_canonical_path` documenting pre-write intent); `apply_sidecar_preference` (return the canonical registry key for a `.mdl` whose sibling `.sd.json` exists), plus the trivial helpers `sidecar_for_mdl`, `is_mdl_extension`, `to_forward_slash`. **New code that resolves user-supplied paths MUST use these helpers** -- the recurring class of bug "consumer X forgot to apply the rule consumer Y enforces" is closed only by funneling every consumer through the same primitive.
- `git.rs` -- `GitProbe` shells out to `git ls-files` and `git status --porcelain` to classify each project as `Tracked { dirty }`, `Untracked`, or `Unavailable`. Per-repo cache keyed by index mtime to avoid re-shelling on every list.
- `registry.rs` -- `ProjectRegistry` and `ProjectMeta`. Stores absolute canonicalized paths as keys, with version counter, `last_disk_hash` (for watcher echo suppression), `last_diagnostic_keys` (for diagnostics-deduplication), and a lazily-hydrated `Arc<RwLock<Option<Arc<ProjectDoc>>>>`. The `check_increment_and_merge` primitive is the single mutator used by HTTP saves, MCP edits, and watcher disk reloads.
- `loro_doc.rs` -- `ProjectDoc` wraps a `LoroDoc`. The `apply_canonical_json` merge primitive walks incoming JSON against the live `LoroMap` and `LoroList` state and emits the minimal op set; `export_canonical_json` is the inverse. Variable arrays inside each model are reshaped to canonical-name keys so per-variable last-writer-wins works on concurrent edits; `views` stays an array (positions matter).
- `events.rs` -- `EventBus` wraps a `tokio::sync::broadcast::Sender<WsMessage>` (capacity 64). `WsMessage` variants: `ProjectChanged { source: User|Agent|Disk }`, `ProjectRemoved`, `ProjectRenamed`, `ProjectFocused`, `SelectionChanged`, `DiagnosticsChanged`. Re-exports `ValidationError = simlin_mcp_core::ErrorOutput` so the WebSocket and MCP error shapes are byte-identical.
- `validation.rs` -- Pure save-validation pipeline. `compute_baseline` captures the on-disk error set; `validate_save_project` rejects only *new* errors (so saves that fix some errors without introducing more are accepted).
- `writer.rs` -- `resolve_save_target` (pure dispatch) + `commit_write` (atomic file I/O). `.stmx`/`.xmile` go in-place; `.mdl` writes to a sibling `.sd.json` sidecar (the `.mdl` itself stays untouched -- the GET handler prefers the sidecar when both exist); `.sd.json` overwrites in place.
- `watcher.rs` -- `WatcherActor` consumes `notify-debouncer-full` batches via a tokio mpsc channel. Per-event handlers: `handle_model_change` (read, hash-compare for echo suppression, parse, validate, merge, broadcast), `handle_model_removal` (drop registry entry, broadcast `ProjectRemoved`), `handle_model_rename` (preserves the `LoroDoc`, broadcasts `ProjectRenamed`), `handle_git_change` (invalidate `GitProbe` cache, re-evaluate every entry in the affected repo).
- `handlers.rs` -- The Axum HTTP handlers (`list_projects`, `get_project`, `save_project`, `create_new_project`, `updates_ws_handler`) plus the `AppState`. The save handler runs validation, calls `check_increment_and_merge`, calls `commit_write`, refreshes registry mtime/size, and publishes `WsMessage::ProjectChanged { source: User }`.
- `middleware.rs` -- `host_validator_middleware` enforces the per-launch `Host:` allowlist (`127.0.0.1:<ui_port>`, `localhost:<ui_port>`) on every HTTP route. Layered innermost so the inexpensive trace and body-limit layers wrap it.
- `serving.rs` -- `bind_or_die` binds a `TcpListener` and prints a hint mentioning the relevant CLI flag on `EADDRINUSE`.
- `static_assets.rs` -- `rust-embed`-baked SPA assets with SPA-fallback (unknown routes serve `index.html`).
- `launcher.rs` -- `build_launch_url`, `open_browser`. Optional headless launch via `--no-open`.
- `hashing.rs` -- XXH3-64 of file bytes for watcher echo suppression.
- `diagnostics.rs` -- `compute_diagnostic_set` runs the engine pipeline once and returns the canonical key set used by `DiagnosticsChanged` deduplication.

### MCP integration (`src/mcp/`)
- `mod.rs` -- Re-exports `RegistryAccess`, `SimlinServeMcpServer`, `build_mcp_router`.
- `access.rs` -- `RegistryAccess` impl of `simlin_mcp_core::ProjectAccess`. Path-traverses out of the canonicalized root (returns `NotFound` to avoid leaking layout); routes through the same `ProjectRegistry` mutators the HTTP save handler uses; broadcasts `ProjectChanged { source: Agent }` after a successful merge.
- `server.rs` -- `SimlinServeMcpServer<A>` rmcp `ServerHandler`. Mounts the three reused tools by delegating to `simlin_mcp_core::tools` + adds `ListProjects` (mirrors GET on the projects route) and `Simulate` (runs a sim with optional `overrides` and `simSpecsOverride`). On `initialize`, spawns a per-session forwarder that bridges the `EventBus` to MCP notifications.
- `notifications.rs` -- `forward_events_to_peer` translates each `WsMessage` to a JSON-RPC notification under the "simlin/" method namespace (`projectChanged`, `projectRemoved`, `projectFocused`, `selectionChanged`, `diagnosticsChanged`). Exits naturally on `ServiceError::TransportClosed` when the session ends.
- `transport.rs` -- `build_mcp_router(state)` returns the Axum router that mounts rmcp's `StreamableHttpService` at the MCP route. Each session gets a fresh `SimlinServeMcpServer` via the factory closure, but they share the same `Arc<AppState>`.
- `list_projects.rs`, `simulate.rs` -- Tool implementations specific to `simlin-serve` (not reused from `simlin-mcp-core`).

### SPA (`web/`)
React 19 + Vite + Jest. Built into a `dist` directory and embedded by `rust-embed` at compile time. Key files (under `web/src/`):
- `App.tsx` -- Top-level shell, project list, route to `EditorHost`.
- `EditorHost.tsx` (in the `components` subdirectory) -- Hosts `@simlin/diagram`'s `<Editor>`. Wires real `onSave` (with sidecar-redirect on 409 conflict), debounced `onSelectionChanged` (150ms), and `projectFocused` on mount/path change.
- `api.ts` -- HTTP client (list/get/save/create).
- `ws.ts` -- WebSocket client; reconnects on transient failure, parses `WsMessage` discriminants, ignores unknown `type` values for forward compat.

## Contracts

- **Path resolution goes through `path_resolution`** -- the sidecar-preference rule, the canonicalize-within-root check, and the resolve-create-target check are implemented once in `src/path_resolution.rs` and called by every consumer (HTTP handlers, MCP `RegistryAccess`, watcher). Inlined reimplementations are a maintenance bug-source we have repeatedly paid for; new consumers must call the existing helpers or extend the module rather than open-coding the rule.
- **One Loro document per project, shared by all editors** -- the HTTP save handler, the MCP `EditModel` tool, and the watcher's `merge_disk_change` all funnel through `ProjectRegistry::check_increment_and_merge`. New write paths must use this primitive or they bypass the CRDT.
- **Source provenance is observable** -- every `ProjectChanged` broadcast carries `ChangeSource: User | Agent | Disk` so subscribers can distinguish "I made this" from "the AI made this" from "an external edit landed".
- **Optimistic-locking is mandatory** -- save handlers and the MCP `EditModel` impl pass `expected_version`. A mismatch returns 409 / `AccessError::VersionMismatch`; the SPA refetches and re-renders.
- **MCP notifications are advisory** -- the transport delivers tool responses and notifications on parallel paths; clients may see a notification before the response that produced it. Treat each as a hint, not authoritative state.
- **No bearer-token auth** -- V1 trusts the OS user-account boundary. The host- and origin-allowlist on the WS upgrade is the only cross-origin defense. Multi-user shared hosts are out of scope; see `docs/threat-model.md`.
- **Empty-`Origin` policy** -- `--strict-origin` defaults to `true` (production-correct: SPA always sends `Origin`). Setting to `false` allows non-browser clients like `wscat` for local development. A *present* `Origin` must always match the loopback allowlist regardless.
- **`.mdl` writes are sidecar-only** -- the `.mdl` source is never modified; structural edits land in a sibling `.sd.json` that becomes source-of-truth on subsequent reads.
- **No external deps unless launched** -- the binary is self-contained: SPA assets are embedded, the MCP server is in-process, and there is no database.

## Dependencies

- `simlin-engine` -- model parsing, diagnostics, atomic writes (`simlin_engine::io::atomic_write`), XMILE serialization (`to_xmile`).
- `simlin-mcp-core` -- the `ProjectAccess` trait, the three reused tools, output types, error shape.
- `rmcp` -- the streamable-HTTP server transport and `ServerHandler` machinery.
- `loro` -- the per-project CRDT.
- `notify-debouncer-full` -- filesystem watcher (recommended platform watcher under the hood).
- `axum` (with `ws` feature), `tower-http`, `tokio` (`full`), `tracing`, `clap`, `rust-embed`, `twox-hash` (XXH3-64), `ignore` (gitignore-aware walk).

Used-by: nothing else in the workspace links against `simlin-serve` -- it is a leaf binary + library.

## npm Distribution

Mirrors `@simlin/mcp`'s pattern:
- Wrapper package `@simlin/serve` with a `bin/simlin-serve.js` runtime shim that resolves the platform-specific binary via npm `optionalDependencies`.
- Per-platform packages `@simlin/serve-{darwin-arm64,linux-arm64,linux-x64,win32-x64}` each containing one binary.
- `darwin-x64` is not yet published (Rosetta cannot run an arm64 binary on Intel; tracked in [docs/tech-debt.md](/docs/tech-debt.md)).
- `build-npm-packages.sh` generates per-platform `package.json` files.
- `Dockerfile.cross` and `scripts/cross-build.sh` orchestrate Linux/Windows cross-compilation via `cargo-zigbuild`; macOS arm64 builds natively on the macOS GitHub runner.
- Release trigger: push a `serve-v*` tag; the `serve-release.yml` workflow builds, publishes platform packages, then publishes the wrapper.
- The internal SPA package is `@simlin/serve-web` (TypeScript workspace member; not published — its build output is embedded in the binary).

## Build / Test

```sh
cargo test -p simlin-serve              # Rust integration tests
pnpm --filter @simlin/serve-web run test # SPA unit tests
pnpm --filter @simlin/serve-web run lint
```

Integration tests live in the single consolidated `tests/integration` harness (one binary instead of one per file; see GH #706 -- add new integration tests as a `mod` in `tests/integration/main.rs`; shared fixtures stay in `tests/fixtures/`) and cover discovery, registry, watcher behaviour, save validation, MCP tool dispatch, MCP notifications, dual-port smoke, end-to-end MCP-to-browser propagation, and the npm release workflow.

## Security and Threat Model

See [/docs/threat-model.md](/docs/threat-model.md) for the V1 threat model: loopback bind, per-launch bearer token, DNS-rebinding mitigation via Host and Origin allowlists, and what is explicitly out of scope.
