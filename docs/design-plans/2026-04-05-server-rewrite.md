# Local Server Design (`simlin-serve`)

## Summary

This design replaces the original cloud-server-rewrite vision with a local-first tool: a single binary, distributed as `npx @simlin/serve`, that the user runs in any directory containing system dynamics models. It opens a browser to a Simlin editor pointed at the user's local files (`.stmx`, `.xmile`, `.mdl`) and concurrently exposes an in-process MCP server on a stable localhost port so a desktop AI client (Claude desktop, Claude Code, etc.) can read, edit, and observe the same models the human is editing in the browser. The two surfaces share a single in-memory project state and a single Loro CRDT merge layer, so edits from the browser, the AI, and direct text-editor changes on disk all compose without conflict.

The design eliminates everything the cloud rewrite needed -- auth, sessions, deployment, sprites, per-user sandboxing, persistent databases, agent transport bridging -- in favor of a much smaller surface: filesystem discovery, git-status reporting, file watching, JSON-to-Loro diffing, and HTTP/MCP serving over loopback. The user's working directory is the entire persistent state; there is no database and no hidden state directory.

The work is structured as nine phases starting with a deployable scaffold (`npx @simlin/serve` opens an empty placeholder UI) and culminating in a V1 with full browser/AI/disk co-editing, MCP push notifications, and packaging parity with the existing `@simlin/mcp` distribution. Code is organized so the same MCP tool implementations and Loro merge primitives can later be lifted into a Rust replacement of `src/server` for the cloud case.

## Definition of Done

A design document that specifies a local-first Simlin server (`simlin-serve`) -- discoverable as `npx @simlin/serve`, runnable in any directory, browser-accessible on an ephemeral port, MCP-accessible on a stable port -- with justified rationale for each choice and clear phase boundaries.

**Success criteria:**
- Each major architectural decision is grounded in either an existing pattern in the monorepo (notably the `@simlin/mcp` distribution model and the `Editor.tsx` save semantics) or in a documented trade-off
- Architecture supports: local file discovery and editing, git-status reporting, MCP integration with desktop AI clients, and concurrent edits from browser + AI + direct disk edits
- No multi-tenancy, no auth, no database, no hidden state directory required to ship V1
- Architecture preserves a clear path to lifting the same MCP tools and merge primitives into a future cloud server (no transport-coupled tool implementations, no filesystem-coupled MCP handlers)
- Clear enough to serve as the basis for an implementation plan

**Out of scope:** Cloud deployment, multi-user collaboration over the open internet, Vensim `.mdl` writeback, real-time browser-to-browser sync, AI-side permission prompts, future cloud-server design (separate plan once V1 is shipped).

## Acceptance Criteria

### server-rewrite.AC1: Discovery and listing
- **server-rewrite.AC1.1 Success:** Running `simlin-serve` in a directory containing `.stmx`, `.xmile`, and `.mdl` files lists all of them in the browser UI
- **server-rewrite.AC1.2 Success:** Discovery is recursive across subdirectories, with each file's path shown relative to `$PWD`
- **server-rewrite.AC1.3 Success:** Files inside `node_modules/`, `.git/`, `target/`, and other conventional generated directories are excluded
- **server-rewrite.AC1.4 Edge:** Directories with no model files render an empty-state UI with a "Create new model" affordance
- **server-rewrite.AC1.5 Edge:** Symlinked directories are not followed (avoids infinite-loop risk)

### server-rewrite.AC2: Git status reporting
- **server-rewrite.AC2.1 Success:** Each listed file shows a "version controlled" indicator if it is tracked by an enclosing git repository
- **server-rewrite.AC2.2 Success:** Each listed file shows a "modified" indicator if it has uncommitted local changes
- **server-rewrite.AC2.3 Success:** Files outside any git working tree show a clear "not under version control" warning in both the list view and the editor view
- **server-rewrite.AC2.4 Edge:** Git status is recomputed when the file watcher fires for the file or for `.git/HEAD`/`.git/index`
- **server-rewrite.AC2.5 Failure:** When `git` is not on the user's PATH, the server still runs but every file is reported as "git status unavailable" with a one-time UI hint

### server-rewrite.AC3: Editing round-trip
- **server-rewrite.AC3.1 Success:** Opening a `.stmx` or `.xmile` file in the browser displays the model and allows editing
- **server-rewrite.AC3.2 Success:** Saving an edit to a `.stmx`/`.xmile` file writes the new content back to the original file in XMILE format
- **server-rewrite.AC3.3 Success:** Opening a `.mdl` file in the browser parses the file via xmutil and displays the model
- **server-rewrite.AC3.4 Success:** Saving an edit to a `.mdl` file writes a `<name>.sd.json` sidecar in the same directory; the original `.mdl` is never modified
- **server-rewrite.AC3.5 Success:** Re-opening a `.mdl` after edits prefers `<name>.sd.json` over the `.mdl` (the sidecar is the new source of truth once it exists)
- **server-rewrite.AC3.6 Failure:** Stale-version save (optimistic lock) returns 409 Conflict and the browser refetches

### server-rewrite.AC4: Concurrent editing via Loro
- **server-rewrite.AC4.1 Success:** Two near-simultaneous edits from the browser and the MCP server both apply without data loss
- **server-rewrite.AC4.2 Success:** Editing a model file in an external text editor (e.g. vim) while the browser has the model open causes the browser to update live with the disk-side changes (no reload prompt)
- **server-rewrite.AC4.3 Success:** Browser-side in-flight edits are preserved across an external disk edit (the merge layer combines both)
- **server-rewrite.AC4.4 Edge:** A disk edit that is byte-identical to the current Loro tip is a no-op (no broadcast, no churn)

### server-rewrite.AC5: In-process MCP
- **server-rewrite.AC5.1 Success:** `simlin-serve` exposes an MCP server at `http://127.0.0.1:7878/mcp` (configurable via `--mcp-port`) when launched
- **server-rewrite.AC5.2 Success:** The MCP server advertises `file://$PWD` as a root in its initialize response
- **server-rewrite.AC5.3 Success:** A Claude desktop client configured against the URL can call `list_projects`, `get_project`, `apply_edit`, `simulate`, and `create_project` tools and observe results in the browser within one second
- **server-rewrite.AC5.4 Success:** MCP-initiated edits and browser-initiated edits flow through the same `apply_canonical_json` merge primitive and produce identical end states regardless of order
- **server-rewrite.AC5.5 Edge:** A second `simlin-serve` started in the same `$PWD` (or with the same `--mcp-port`) fails fast with a port-conflict message and a hint to either stop the running instance or pass `--mcp-port`

### server-rewrite.AC6: MCP push notifications
- **server-rewrite.AC6.1 Success:** Opening or switching a project in the browser emits a `projectFocused` notification to subscribed MCP clients
- **server-rewrite.AC6.2 Success:** Selecting a variable in the browser emits a `selectionChanged` notification with the variable's canonical idents
- **server-rewrite.AC6.3 Success:** Any change (browser, MCP, disk) emits a `projectChanged` notification with a `source` discriminator (`"user" | "agent" | "disk"`)
- **server-rewrite.AC6.4 Success:** Changes in validation diagnostics emit `diagnosticsChanged` notifications

### server-rewrite.AC7: Distribution and bootstrap
- **server-rewrite.AC7.1 Success:** `npx @simlin/serve` works on macOS (x64 and arm64), Linux (x64 and arm64), and Windows (x64) by downloading the appropriate prebuilt native binary as an `optionalDependencies` install
- **server-rewrite.AC7.2 Success:** On startup, the server prints the local URLs (HTTP UI and MCP) and attempts to open the HTTP URL via `open` (macOS), `xdg-open` (Linux), or `start` (Windows)
- **server-rewrite.AC7.3 Success:** The launched URL includes a one-time `?token=...` query parameter; the SPA stores it in sessionStorage and uses it as a bearer for the WebSocket upgrade. Loopback is the primary boundary; the token is defense in depth against another local process opening tabs into our editor.
- **server-rewrite.AC7.4 Edge:** When the browser cannot be opened automatically (headless environment, missing `open`/`xdg-open`), the server prints the URL prominently and continues running

## Glossary

- **Axum**: Rust web framework on top of Tower and Tokio, used for the HTTP/WebSocket layer.
- **Tokio**: The async runtime for Rust; one event loop owns both the HTTP server and the MCP server.
- **rmcp**: The official Rust MCP crate. Separates protocol from transport so the same tool implementations can be served over HTTP/SSE today and over stdio later without rewriting.
- **MCP root**: A URI advertised by an MCP server to indicate the scope it operates over. Used by clients to render context ("Simlin is operating on /Users/bobby/sd-models") and to plan tool calls sensibly.
- **MCP push notification**: A server-initiated message sent to subscribed clients (e.g., `notifications/projectFocused`). Distinct from tool responses; the server can emit them at any time.
- **Loro**: A CRDT library for structured documents. Used in-memory only -- one Loro doc per opened project, hydrated from the canonical file on first open, discarded when the process exits.
- **`apply_canonical_json`**: The single primitive that takes the current Loro doc and an arbitrary new canonical JSON, computes the minimal Loro op-set that transitions between them, applies, and returns the resulting tip. Browser saves, file-watcher reconciliation, and MCP whole-document edits all funnel through it.
- **Canonical JSON**: A deterministic JSON serialization of a Simlin model (sorted keys, stable enum encoding) so that semantically identical documents serialize byte-identically. Produced by `simlin-engine`.
- **`ProjectRegistry`**: The single in-memory map of `path -> ProjectState`, owning Loro docs, file metadata, and git status. Shared by reference across the HTTP and MCP handlers.
- **`.sd.json` sidecar**: A canonical-Simlin-JSON file written next to a `.mdl` file (e.g., `population.mdl` -> `population.sd.json`). Used because we have no reliable Vensim `.mdl` writer; the sidecar becomes the new source of truth on first edit.
- **Ephemeral port**: An OS-chosen TCP port (the user does not configure it). Used for the HTTP/UI surface; discovered by the launched `open`/`xdg-open` URL.
- **Stable port**: A configurable, well-known port (default `7878`). Used for the MCP surface so the user can configure their AI client's mcp.json once and have it always work.
- **xmutil**: The Vensim-to-XMILE converter (existing in `src/xmutil`). We use its read path; the write path does not exist.
- **simlin-engine**: The core Rust simulation crate. Linked directly by `simlin-serve` for parsing, validation, simulation, and thumbnail rendering -- no WASM round-trips.
- **simlin-mcp-core** (new): Library crate extracted from `src/simlin-mcp/src/`. Exposes tool implementations as transport-agnostic async functions over a `&ProjectRegistry`. Consumed by both the existing standalone `@simlin/mcp` binary and the new in-process MCP in `simlin-serve`.
- **One-time launch token**: A short bearer string the server generates at startup, embedded in the launched URL. The browser stores it for WebSocket upgrades. Adds defense-in-depth on top of loopback binding.

## Architecture

### Chosen Stack

**Rust binary, distributed as a tiny npm shim, runs entirely on the user's machine.**

| Dimension | Choice | Rationale |
|-----------|--------|-----------|
| Process model | Single Rust binary, single Tokio event loop | One process owns both HTTP/UI and MCP surfaces, sharing in-memory `ProjectRegistry` and Loro docs. No IPC, no race conditions across server boundaries. |
| Persistence | The user's filesystem, period | Model files (`.stmx`/`.xmile`) are read/written in place; `.mdl` files get a `.sd.json` sidecar. No database, no hidden state directory. Survives restart trivially because there is nothing to survive. |
| Concurrency | In-memory Loro CRDT, rebuilt per session | Server-side Loro doc per opened project merges browser + MCP + file-watcher edits without conflict. Discarded on shutdown; rebuilt from canonical file on next open. |
| MCP transport | In-process HTTP/SSE on stable port via `rmcp` | One process, one shared state, no subprocess. Tool implementations are transport-agnostic so a stdio bridge can be added later without changing tool code. |
| Distribution | npm shim + per-platform native binaries (esbuild model) | `npx @simlin/serve` UX with full Rust performance. Pattern is already in use for `@simlin/mcp`; CI infrastructure can be reused/generalized. |
| Browser launch | OS-native `open`/`xdg-open`/`start` to ephemeral-port URL | Zero-configuration startup. Token in URL gates the WebSocket upgrade. |
| Frontend | Reuse `src/diagram`'s `Editor.tsx` | Already has correct save semantics (debounced via `inSave`/`saveQueued`), already designed as a host-agnostic toolkit (`onSave`/`onLoad` callbacks). Embedded into the binary via `rust-embed`. |

### System Components

```
                    ┌──────────────────────────────────────┐
                    │       Browser (single user)            │
                    │   src/diagram Editor inside SPA        │
                    └────────────┬─────────────────────────┘
                                 │ HTTP (ephemeral port)
                                 │ WebSocket (live updates)
                    ┌────────────▼─────────────────────────┐
                    │   simlin-serve (single Rust process)  │
                    │  ┌─────────────────────────────────┐  │
                    │  │  Axum HTTP/WebSocket handlers    │  │
                    │  │  rmcp HTTP/SSE MCP server        │  │
                    │  └────────────┬────────────────────┘  │
                    │  ┌────────────▼────────────────────┐  │
                    │  │     ProjectRegistry              │  │
                    │  │  path -> { Loro doc, meta, git }  │  │
                    │  └────────────┬────────────────────┘  │
                    │  ┌────────────▼────────────────────┐  │
                    │  │  apply_canonical_json primitive  │  │
                    │  │  (single Loro merge entry point) │  │
                    │  └─────────────────────────────────┘  │
                    │  ┌─────────────────────────────────┐  │
                    │  │  notify-rs file watcher          │  │
                    │  │  shell-out to git for status     │  │
                    │  │  simlin-engine for parse/sim/png │  │
                    │  └─────────────────────────────────┘  │
                    └────────────┬─────────────────────────┘
                                 │ HTTP (stable :7878)
                                 │
                    ┌────────────▼─────────────────────────┐
                    │   AI client (Claude desktop / Code)    │
                    │   mcp.json points at http://127.0.0.1   │
                    └────────────────────────────────────────┘

                    ┌────────────────────────────────────────┐
                    │   User's filesystem ($PWD recursive)    │
                    │   *.stmx, *.xmile, *.mdl + sidecars     │
                    │   git for version-control status        │
                    └────────────────────────────────────────┘
```

### Data Flow

**Initial load:**
1. Browser fetches `GET /api/projects` -> `ProjectRegistry` snapshot (path, format, git-status).
2. User clicks a project. Browser fetches `GET /api/projects/<url-encoded-path>`.
3. Server reads file from disk (or `.sd.json` sidecar for `.mdl` if it exists), parses to canonical JSON via `simlin-engine`, hydrates a Loro doc, caches in `ProjectRegistry`, returns canonical JSON + version.
4. Browser hands the JSON to `Editor.tsx` as `initialProject`; opens a WebSocket for live updates.

**Browser save (the existing `Editor.tsx` save flow):**
1. Editor's `onSave({format: 'json', data}, currentVersion)` callback POSTs canonical JSON to `/api/projects/<path>`.
2. Server checks optimistic version. If stale, returns 409 and the browser refetches.
3. Server runs `apply_canonical_json(loro_doc, new_json)` -- diffs against current Loro tip and applies the resulting ops.
4. Server materializes the new canonical JSON, writes to disk:
   - `.stmx`/`.xmile`: serialize via `simlin-engine`'s XMILE writer, write atomically (write-temp + rename) to original path.
   - `.mdl` source: write/overwrite `.sd.json` sidecar; original `.mdl` untouched.
5. Server broadcasts `ProjectChanged{path, version, source: "user"}` on the WebSocket and emits matching MCP `projectChanged` notifications.

**MCP edit (Claude calls `apply_edit`):**
1. Tool handler resolves `path` through `ProjectRegistry`.
2. Handler computes the new canonical JSON (via the existing simlin-mcp tool semantics) and calls the same `apply_canonical_json` primitive.
3. Same disk write + broadcast path as a browser save, with `source: "agent"`.

**External disk change (user edits `.stmx` in vim):**
1. `notify-rs` fires for `<file>`. Server debounces, then re-reads and re-parses the file.
2. Server runs `apply_canonical_json(loro_doc, new_json_from_disk)` -- the same primitive. In-flight browser edits are preserved through the merge.
3. Broadcast `ProjectChanged{..., source: "disk"}` on WebSocket; emit MCP `projectChanged` notification.
4. Browser receives the update and reconciles via the standard live-update path (no modal reload prompt).

**Thumbnail:**
- Generated on demand by `GET /api/preview/<path>`, keyed by `(path, version)`. Renders via `simlin-engine` (linked as a crate, no WASM). Cached in memory; invalidated by `ProjectChanged`.

**MCP push notifications** ride the same internal broadcast channel as the WebSocket. A `notifications_router` translates internal events to MCP notification frames per subscribed session.

### MCP tool surface (V1)

| Tool | Purpose |
|------|---------|
| `list_projects` | Returns the `ProjectRegistry` snapshot the same way `GET /api/projects` does. |
| `get_project(path)` | Returns canonical JSON; reads through the registry so the AI sees the same in-memory state as the browser, including in-flight edits not yet on disk. |
| `apply_edit(path, ops)` | Applies an edit batch. Reuses the existing `simlin-mcp` tool semantics, then funnels through `apply_canonical_json`. |
| `simulate(path, overrides?)` | Runs a simulation via `simlin-engine`. |
| `create_project(path, format)` | Creates an empty model file at `path` (`.stmx` for native, `.sd.json` sidecar if `path` ends in `.mdl`). |

### MCP push notifications (V1)

| Notification | When | Payload |
|--------------|------|---------|
| `projectFocused` | Browser opens or switches a project tab | `{path}` |
| `selectionChanged` | User selects a variable | `{path, variable_idents: [...]}` |
| `projectChanged` | Any model mutation, regardless of source | `{path, version, source: "user"|"agent"|"disk"}` |
| `diagnosticsChanged` | Validation results change | `{path, errors: [...]}` |

### Server structure

**Framework:** Axum 0.8 on Tokio, with `rmcp` for MCP serving.

**Middleware stack** (outer to inner) on the HTTP/UI route:
1. Tracing (`tracing` + `tower-http::TraceLayer`)
2. Body size limit (`tower-http::RequestBodyLimit`, 10 MB)
3. WebSocket upgrade requires the one-time launch token

**Route structure:**
```
GET    /                                    -- SPA entry (rust-embed)
GET    /assets/*                            -- SPA assets
GET    /api/projects                        -- list registry
GET    /api/projects/:path                  -- read canonical JSON
POST   /api/projects/:path                  -- save (whole-document)
GET    /api/preview/:path                   -- PNG thumbnail
WS     /api/updates                         -- broadcast channel for live updates (token-gated)
GET    /healthz                             -- health check
GET    /mcp/*                               -- rmcp HTTP/SSE MCP (on --mcp-port; can be same port via subpath, default separate stable port)
```

**Static file serving:** Embedded via `rust-embed` so the binary is self-contained. Dev mode proxies to Vite dev server.

**File watching:** `notify-rs` with a debounce of ~100ms. Watch only directories that contain a discovered model file (or any of its enclosing dirs back to `$PWD`); avoid watching the entire tree. Also watch `.git/HEAD` and `.git/index` of each enclosing git root for cheap "did git status change?" detection.

**Git status:** Shell out to `git status --porcelain` per directory tree, cache results, invalidate on file-watcher events. If `git` is missing from PATH, set every entry to "git status unavailable" without failing the server.

### Refactor: simlin-mcp into core library + binary

The existing `src/simlin-mcp/src/` is split into:

- **`simlin-mcp-core`** (new library crate): exposes each MCP tool as `async fn(registry: &ProjectRegistry, params: Params) -> Result<Output>`. No transport, no Axum, no HTTP context, no filesystem assumptions beyond what `ProjectRegistry` provides. Reuses simlin-mcp's existing edit op surface.
- **`simlin-mcp`** (existing binary): becomes a thin wrapper that constructs a `ProjectRegistry` from a CLI directory arg and serves `simlin-mcp-core` over stdio via `rmcp`. Existing `@simlin/mcp` users see no behavior change.
- **`simlin-serve`** (new binary): mounts `simlin-mcp-core` directly over `rmcp::HttpServer`, sharing its `ProjectRegistry` instance with the Axum HTTP/UI handlers.

This refactor is doable as a no-op-on-behavior change to `simlin-mcp` first, with `simlin-serve` consuming the new library second. The two consumers share the entire MCP code path going forward.

## Existing Patterns

**Patterns followed:**
- **`Editor.tsx` save semantics.** The web Editor already coalesces edits via `inSave`/`saveQueued` and uses a host-injected `onSave` callback. `simlin-serve` provides the `onSave` that POSTs to `/api/projects/:path`. No new save policy designed; we inherit the right one.
- **`@simlin/mcp` distribution.** The npm-shim + per-platform `optionalDependencies` model already used for `@simlin/mcp` (see `src/simlin-mcp/package.json`, `cross-build.sh`, `mcp-release.yml`) is replicated for `@simlin/serve`. CI scripts can be parameterized to share infrastructure.
- **Praxis local-first patterns.** Praxis's `service/watcher`, `service/gitproj_middleware.go`, and `service/websocket` packages solve the same shape of problems (filesystem source-of-truth, git-aware project context, live updates to clients) and are useful reference implementations.
- **`simlin-engine` as a direct crate dependency.** Same approach the original cloud rewrite called for. No WASM round-trips for parse/sim/render in the server.

**Patterns diverged from:**
- **The original cloud-rewrite design** (the previous version of this document). Auth, sessions, fly.io deployment, sprites, Litestream, Tigris, claude-agent-rs, sprites-rs, seshcookie-rs are all out of scope. The MCP server is hosted by us, not consumed by us. Persistence is the user's filesystem. The `@simlin/mcp` package is preserved (refactored) rather than retired; it gains a sibling rather than a replacement.
- **`src/server`'s content-addressable file storage.** `simlin-serve` operates directly on user files at their existing paths. There is no project ID space; the path is the identity. Optimistic locking via a server-side version counter still applies.
- **`src/app`'s Firebase coupling.** `simlin-serve`'s frontend reuses the `src/diagram` Editor toolkit (which is host-agnostic by design) rather than starting from `src/app`. The hosting React shell is small and lives inside `src/simlin-serve/web/`.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: MVP read-only viewer

**Goal:** `npx @simlin/serve` installs a per-platform native binary, runs in any directory, opens a browser to a project list with git-status indicators, and lets the user view (read-only) any discovered model in the existing diagram editor.

**Components:**
- New crate `simlin-serve` in `src/simlin-serve/` added to the workspace
- Axum app binding to ephemeral TCP port on `127.0.0.1`, with `/healthz`
- One-time launch-token generation; token included in the URL passed to `open`/`xdg-open`/`start`
- Browser launcher with graceful fallback to printing the URL when no opener is available
- npm package `@simlin/serve` with per-platform `optionalDependencies` shim, modeled on `@simlin/mcp`
- `cross-build.sh` and `serve-release.yml` GitHub Actions workflow modeled on `mcp-release.yml`. File a follow-up ticket to factor shared cross-build infrastructure once two consumers exist.
- Recursive directory walk for `*.stmx`, `*.xmile`, `*.mdl`, excluding conventional generated dirs (`node_modules`, `.git`, `target`, `playwright-report`, `test-results`); skip symlinked dirs
- Git-root detection (walk up from each file's directory; cache by directory) and `git status --porcelain` per file with graceful fallback when `git` is unavailable
- `ProjectRegistry`: `Arc<RwLock<HashMap<PathBuf, ProjectMeta>>>` where `ProjectMeta` includes `format`, `mtime`, `size`, `git_tracked`, `git_dirty`
- `GET /api/projects` returning the registry snapshot as JSON
- `GET /api/projects/:path`: read file (preferring `.sd.json` sidecar for `.mdl` if present), parse to canonical JSON via `simlin-engine`, return `{json, version}`
- React shell at `src/simlin-serve/web/` that hosts `<Editor>` from `src/diagram` with `readOnlyMode={true}`, the project list as a sidebar, and `onLoad` plumbed to our HTTP API. No `onSave` wired -- editing is disabled this phase.
- `src/diagram` Editor and dependencies bundled and embedded via `rust-embed`

**Dependencies:** None (first phase)

**Done when:** `npx @simlin/serve` works on macOS arm64, Linux x64, and Windows x64; the browser opens to a list of all model files in the directory tree with correct git-status; clicking a file renders its diagram in the existing Editor in read-only mode; AC1, AC2, and AC7 pass; AC3.1 and AC3.3 pass (open `.stmx`/`.xmile`/`.mdl` for viewing).
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Save and write-back

**Goal:** A user can edit a model in the browser and the changes are written to disk in the appropriate format, with optimistic locking preventing stale overwrites.

**Components:**
- `POST /api/projects/:path`: optimistic version check; on miss, return 409. On match, write canonical JSON back to disk:
  - `.stmx`/`.xmile`: serialize to XMILE via `simlin-engine`'s writer, atomic write (temp file + rename)
  - `.mdl`: write `<name>.sd.json` sidecar; original file untouched
- React shell flips `readOnlyMode` off and wires `onSave` to POST canonical JSON to our endpoint
- XMILE write determinism: serializer output should be byte-stable across runs for semantically identical input. Track and harden as part of this phase; treat regressions as bugs.
- Reopening a `.mdl` after edits prefers `<name>.sd.json` (the sidecar becomes the new source of truth once it exists)

**Dependencies:** Phase 1 (read endpoints, Editor integration, registry)

**Done when:** A user can edit a model file in the browser and the changes are reflected on disk in the appropriate format; reopening the file shows the edited state; concurrent saves are rejected with 409; AC3.2, AC3.4, AC3.5, AC3.6 pass.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Loro merge primitive

**Goal:** All edits flow through a single `apply_canonical_json` primitive that diffs canonical JSON against a Loro doc and applies the minimal op-set.

**Components:**
- Per-project Loro doc, hydrated lazily on first read of the project, cached in `ProjectRegistry`
- `apply_canonical_json(loro_doc: &mut LoroDoc, new_json: &CanonicalJson) -> Result<Vec<LoroOp>>` -- the single primitive
- Refactor `POST /api/projects/:path` to flow through this primitive instead of writing JSON directly to Loro
- Internal broadcast channel (`tokio::sync::broadcast`) for `ProjectChanged` events
- WebSocket endpoint `/api/updates` (token-gated) that subscribes to the broadcast channel and forwards changes to the browser
- Browser-side handler: on receipt of `ProjectChanged`, refetch the project JSON and feed it back into Editor

**Dependencies:** Phase 2 (save endpoint exists)

**Done when:** Browser saves go through the merge primitive; the WebSocket reports changes back to all open browser tabs; two near-simultaneous saves merge without data loss in tests.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: File watcher and on-disk diff/merge

**Goal:** Editing a model file in an external editor while the browser has it open causes the browser to update live; in-flight browser edits are preserved through the merge.

**Components:**
- `notify-rs` recursive watcher rooted at `$PWD`, with the same exclusion rules as discovery
- Debounce (~100 ms) to coalesce bursts of fs events
- On a relevant file event: re-parse the file, run `apply_canonical_json` against the current Loro doc, broadcast `ProjectChanged{source: "disk"}`
- Watch for `.git/HEAD` and `.git/index` mutations to refresh git status without a full re-walk
- Tests: external script edits a `.stmx` while a Loro doc has in-flight ops; merged output contains both

**Dependencies:** Phase 3 (merge primitive exists)

**Done when:** Editing a model file in vim while the browser has it open causes the diagram to update without a reload prompt; browser-side in-flight edits are preserved; byte-identical disk writes are no-ops.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Refactor simlin-mcp into core library + binary

**Goal:** `src/simlin-mcp/src/` is split into `simlin-mcp-core` (library) and `simlin-mcp` (thin stdio wrapper) without behavior change for existing `@simlin/mcp` users.

**Components:**
- New crate `simlin-mcp-core`: each existing tool becomes `async fn(registry: &ProjectRegistry, params: Params) -> Result<Output>`. Tool surface remains exactly what `simlin-mcp` exposes today.
- `simlin-mcp` binary becomes a thin wrapper: parses CLI args (working directory), constructs a `ProjectRegistry`, serves `simlin-mcp-core` over stdio via `rmcp`
- All existing `simlin-mcp` tests pass unchanged
- The `@simlin/mcp` npm package and its release pipeline are not touched

**Dependencies:** Phase 1 (`ProjectRegistry` exists in the workspace)

**Done when:** The existing `simlin-mcp` test suite passes; `@simlin/mcp` continues to function identically for current users; `simlin-mcp-core` can be depended on from `simlin-serve` (verified by a smoke test).
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: In-process MCP in simlin-serve

**Goal:** `simlin-serve` mounts `simlin-mcp-core` over HTTP/SSE on a stable port, sharing the same `ProjectRegistry` as the HTTP/UI handlers.

**Components:**
- `rmcp` HTTP/SSE server bound to `127.0.0.1` on `--mcp-port` (default `7878`)
- MCP `initialize` advertises `file://$PWD` as a root, plus declared notification capabilities
- All MCP tool handlers borrow the same `Arc<ProjectRegistry>` as the HTTP handlers; edits flow through `apply_canonical_json`
- Server prints both the HTTP UI URL and the MCP URL on startup; documents the canonical mcp.json snippet for Claude desktop
- Port-conflict handling: fail fast with a hint to pass `--mcp-port`

**Dependencies:** Phase 5 (`simlin-mcp-core` exists), Phase 3 (merge primitive routes MCP edits through Loro)

**Done when:** A Claude desktop client configured against the printed MCP URL can list projects, read a project, edit it, and observe the changes appear in the browser within ~1 s; AC5 round-trip equivalence tests pass.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: MCP push notifications

**Goal:** Active MCP sessions receive `projectFocused`, `selectionChanged`, `projectChanged`, and `diagnosticsChanged` notifications driven by browser actions and internal state changes.

**Components:**
- Notification capability declaration in MCP `initialize` response
- A `notifications_router` that subscribes to the existing internal broadcast channel and emits MCP notifications on each registered MCP session
- Browser-side: emit `selectionChanged` and `projectFocused` events to the server over the existing WebSocket
- `diagnosticsChanged` derived from `simlin-engine` validation results, recomputed after each edit

**Dependencies:** Phase 6 (in-process MCP exists)

**Done when:** A connected MCP client receives all four notification kinds in response to corresponding actions; ordering relative to tool responses is correct (no notification for an MCP-initiated edit before that edit's tool response).
<!-- END_PHASE_7 -->

<!-- START_PHASE_8 -->
### Phase 8: V1 polish

**Goal:** A user can open `simlin-serve` in a directory, create new models, handle file deletion/rename gracefully, and trust the loopback boundary.

**Components:**
- New-project creation UX: empty-directory state and "New model" affordance from any state; user picks filename and format (`.stmx` default)
- File watcher handles delete and rename events: deleted files drop from the registry; renames update the path key; open browser tabs on a deleted file get a "this model was deleted on disk" state
- Multiple files with the same basename in different subdirectories are disambiguated in the UI (full relative path shown when ambiguous)
- README and `npx @simlin/serve` install/use docs
- One-time bearer token rotated on each launch; documented threat model
- A small smoke-test integration suite that boots `simlin-serve`, creates a project, edits via the browser path and via the MCP path, and verifies disk state

**Dependencies:** Phase 7 (push notifications exist)

**Done when:** All AC1-AC7 criteria pass; smoke test passes on macOS arm64, Linux x64, and Windows x64 in CI; README is sufficient for a new user to install and use.
<!-- END_PHASE_8 -->

## Additional Considerations

**Future cloud server (separate plan):** The original version of this document specified a full cloud rewrite. That plan is deferred. Several pieces of `simlin-serve` are intentionally structured to lift cleanly into a future cloud server: `simlin-mcp-core` is transport-agnostic, the `apply_canonical_json` primitive is stateless, the `ProjectRegistry` abstraction is the only filesystem-coupled piece (it can be backed by SQLite or another store in the cloud case). When the cloud rewrite is revisited, it should consume this work, not duplicate it.

**XMILE serialization stability:** When `simlin-engine` serializes a Loro tip back to XMILE that is semantically identical to the file on disk, the resulting bytes should also be identical (or at least stable across runs). This keeps git diffs meaningful. Track and harden this property as part of Phase 3; treat any whitespace/attribute-order regressions as bugs.

**Vensim `.mdl` round-trip:** Out of scope for V1. We read `.mdl` via `xmutil` and write to a `.sd.json` sidecar. A future Vensim writer (whether in `xmutil` or elsewhere) could enable true round-trip; until then, edits land in the sidecar and `git diff` will show changes there rather than in the original.

**Headless / CI use:** `simlin-serve` is primarily an interactive tool, but it can be useful in headless contexts (e.g., a CI job calling MCP tools). The browser launcher must fail soft (print URL, do not exit) so headless use is not blocked.

**Concurrent `simlin-serve` instances:** Two instances in the same directory will collide on the MCP port and on file-watcher debouncing semantics. Phase 7 fails fast on port conflict; we do not attempt cross-process coordination of file ownership in V1.

**Telemetry:** None in V1. The tool runs entirely on the user's machine; no usage data is collected.
