# Phase 1: MVP Read-Only Viewer Implementation Plan

**Goal:** Stand up the `simlin-serve` Rust crate, an `@simlin/serve` npm shim, and CI release plumbing so that `npx @simlin/serve` (run in any directory) opens a browser to a list of discovered system-dynamics model files with git-status indicators and lets the user view (read-only) any model in the existing `<Editor>` from `src/diagram`.

**Architecture:** A new Cargo crate `src/simlin-serve/` builds a single Rust binary. It hosts an Axum 0.8 HTTP server bound to an OS-chosen ephemeral port on `127.0.0.1`, performs recursive filesystem discovery for `*.stmx`/`*.xmile`/`*.mdl` files via the `ignore` crate, shells out to `git status --porcelain` for per-file VCS state, exposes JSON read endpoints, and serves an embedded React SPA built with Vite. The SPA hosts the existing `<Editor>` component from `@simlin/diagram` in `embedded`/`readOnlyMode` mode. Distribution mirrors the existing `@simlin/mcp` pattern: an `optionalDependencies` npm shim resolves the per-platform native binary at runtime, with binaries cross-built via `cargo-zigbuild` (Linux/Windows) and native macOS arm64 runner.

**Tech Stack:** Rust 1.95.0 / edition 2024 / `axum 0.8` / `tower-http 0.6` / `tokio 1` / `rust-embed 8` + `mime_guess 2` / `ignore 0.4` / `open 5` / `rand 0.10` + `base64 0.22` / `clap 4` / `tracing` + `tracing-subscriber 0.3`. Frontend: TypeScript / React **class components** (per project convention) / Vite (NOT Rsbuild) / `@simlin/diagram` `<Editor>` in JSON input mode / Jest + ts-jest + @testing-library/react. Distribution: npm `optionalDependencies` per-platform packages.

**Scope:** Phase 1 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC1: Discovery and listing
- **server-rewrite.AC1.1 Success:** Running `simlin-serve` in a directory containing `.stmx`, `.xmile`, and `.mdl` files lists all of them in the browser UI
- **server-rewrite.AC1.2 Success:** Discovery is recursive across subdirectories, with each file's path shown relative to `$PWD`
- **server-rewrite.AC1.3 Success:** Files inside `node_modules/`, `.git/`, `target/`, and other conventional generated directories are excluded
- **server-rewrite.AC1.4 Edge:** Directories with no model files render an empty-state UI with a "Create new model" affordance *(partial — empty-state rendered in Phase 1; the "Create new model" affordance is delivered in Phase 8)*
- **server-rewrite.AC1.5 Edge:** Symlinked directories are not followed (avoids infinite-loop risk)

### server-rewrite.AC2: Git status reporting
- **server-rewrite.AC2.1 Success:** Each listed file shows a "version controlled" indicator if it is tracked by an enclosing git repository
- **server-rewrite.AC2.2 Success:** Each listed file shows a "modified" indicator if it has uncommitted local changes
- **server-rewrite.AC2.3 Success:** Files outside any git working tree show a clear "not under version control" warning in both the list view and the editor view
- **server-rewrite.AC2.4 Edge:** Git status is recomputed when the file watcher fires for the file or for `.git/HEAD`/`.git/index` *(partial — Phase 1 implements eager recomputation on each `GET /api/projects`; the file-watcher-driven invalidation arrives in Phase 4)*
- **server-rewrite.AC2.5 Failure:** When `git` is not on the user's PATH, the server still runs but every file is reported as "git status unavailable" with a one-time UI hint

### server-rewrite.AC3 (partial): Editing round-trip — read path only
- **server-rewrite.AC3.1 Success:** Opening a `.stmx` or `.xmile` file in the browser displays the model and allows editing *(partial — Phase 1 displays the model; "allows editing" is enabled in Phase 2)*
- **server-rewrite.AC3.3 Success:** Opening a `.mdl` file in the browser parses the file via xmutil and displays the model *(see Notes below regarding the parser choice)*

### server-rewrite.AC7: Distribution and bootstrap
- **server-rewrite.AC7.1 Success:** `npx @simlin/serve` works on macOS (x64 and arm64), Linux (x64 and arm64), and Windows (x64) by downloading the appropriate prebuilt native binary as an `optionalDependencies` install *(partial — Phase 1 ships the same 4 platforms `@simlin/mcp` ships today: macOS arm64, Linux x64, Linux arm64, Windows x64. Adding `darwin-x64` (macOS Intel) is deferred to Phase 8 polish; see Notes)*
- **server-rewrite.AC7.2 Success:** On startup, the server prints the local URLs (HTTP UI and MCP) and attempts to open the HTTP URL via `open` (macOS), `xdg-open` (Linux), or `start` (Windows) *(partial — Phase 1 prints only the HTTP URL, since the MCP URL doesn't exist until Phase 6)*
- **server-rewrite.AC7.3 Success:** The launched URL includes a one-time `?token=...` query parameter; the SPA stores it in sessionStorage and uses it as a bearer for the WebSocket upgrade. Loopback is the primary boundary; the token is defense in depth against another local process opening tabs into our editor. *(partial — Phase 1 generates the token, embeds it in the URL, and stores it in sessionStorage. The WebSocket bearer enforcement arrives with the WebSocket itself in Phase 3)*
- **server-rewrite.AC7.4 Edge:** When the browser cannot be opened automatically (headless environment, missing `open`/`xdg-open`), the server prints the URL prominently and continues running

---

## Notes for Executor

The codebase investigation produced several findings that change naive readings of the design. Read these before implementing:

**1. `<Editor>` read-only semantics.** The `readOnlyMode` prop on `<Editor>` (`src/diagram/Editor.tsx:233`) only adds a warning toast; it does **not** disable canvas editing. The prop that disables interaction is `embedded={true}` (it gates the toolbar, save flow, and most interaction handlers). The Phase 1 viewer must pass **both** `embedded={true}` (to make the canvas inert) and `readOnlyMode={true}` (to surface the user-facing "view-only" message). Additionally, the `onSave` prop is **required** by the TypeScript types (`ProtobufInputProps` and `JsonInputProps`); pass an `async () => undefined` no-op so the type-checker is satisfied without enabling save.

**2. `<Editor>` has no `onLoad` prop.** The host shell is responsible for fetching the project JSON via HTTP and passing it as `initialProjectJson`. The design's mention of "`onLoad` plumbed to our HTTP API" describes the host shell's `fetch()` call, not an Editor callback.

**3. Use `inputFormat: 'json'`.** The Editor accepts either a protobuf binary or a Simlin JSON string. For `simlin-serve` we use JSON throughout (matches what Phase 3's canonical-JSON pipeline will produce).

**4. Vensim `.mdl` parsing — `open_vensim` (native), not `open_vensim_xmutil`.** The design AC3.3 says "parses the file via xmutil". The codebase exposes both:
- `simlin_engine::open_vensim(contents)` at `src/simlin-engine/src/compat.rs:63` — native Rust parser, what `simlin-mcp` uses today
- `simlin_engine::open_vensim_xmutil(contents)` at `src/simlin-engine/src/compat.rs:45` — xmutil-feature-gated path that pulls in C++ FFI

The native path is preferable for `simlin-serve` because (a) it matches `simlin-mcp`'s posture and (b) it avoids adding a C++ toolchain to the cross-build container. The native parser is mature enough that `simlin-mcp`'s `ReadModel` tool ships with it. **Use `open_vensim`.** The design's AC3.3 wording is preserved literally above for traceability; if the user wants strict design fidelity, switch to `open_vensim_xmutil` and add the `xmutil` feature to `simlin-engine` in `Cargo.toml` plus update the cross-build Dockerfile to include a C++ toolchain.

**5. Format detection lives in `simlin-mcp`'s `open_project`.** `src/simlin-mcp/src/tools/mod.rs::open_project(path, contents)` already implements extension-driven format detection (and content-based JSON-shape detection for non-extension cases). Phase 1 should mirror its logic; do **not** copy the function (that's a Phase 5 refactor target). For Phase 1, write a small extension-driven dispatcher in `simlin-serve` and note that Phase 5 will consolidate.

**6. Discovery — prefer `ignore`, with a small universal exclusion list.** The `ignore` crate honors the user's `.gitignore` automatically, which already covers project-specific build artifacts. Add explicit hardcoded exclusions only for the universal ones: `node_modules`, `.git`, `target`, `playwright-report`, `test-results`. The JS-monorepo-specific entries (`lib`, `lib.browser`, `lib.module`, `build`, `dist`) are covered by `.gitignore` in this monorepo and will be in any sane JS project; we don't hardcode them. Set `follow_links(false)` (the default) per AC1.5.

**7. `ProjectRegistry` is genuinely new.** Nothing similarly named exists in the workspace. Place it in `src/simlin-serve/src/registry.rs`. (Note: `simlin-mcp/src/tool.rs::Registry` is the MCP tool-registry — different concept; the name overlap is a known confusion risk. Use the `Project` prefix to distinguish.)

**8. `rmcp` is NOT in the workspace today.** `simlin-mcp` uses a hand-rolled JSON-RPC 2.0 stdio implementation in `src/simlin-mcp/src/protocol.rs`. Phase 6 will introduce `rmcp` as a new dependency. Do **not** add `rmcp` in Phase 1.

**9. Cargo workspace conventions.** Edition 2024, license `Apache-2.0`, author `"Bobby Powers <bobbypowers@gmail.com>"`. Toolchain pinned at `1.95.0` (`rust-toolchain.toml`). There is no `[workspace.lints]` section, so the new crate sets its own lint attributes (apply `#![deny(unsafe_code)]` to match `simlin-engine`).

**10. Platform matrix discrepancy.** AC7.1 lists 5 platforms; `@simlin/mcp` already ships 4 of them (`darwin-arm64`, `linux-x64`, `linux-arm64`, `win32-x64`) — verified at `src/simlin-mcp/package.json:11-15` and `bin/simlin-mcp.js:24` (PLATFORM_MAP), and `.github/workflows/mcp-release.yml:53-56` (build matrix includes `aarch64-unknown-linux-musl`). Phase 1 ships these same 4 platforms. The remaining gap is `darwin-x64` (macOS Intel), deferred to Phase 8 polish. The Phase 1 "Done when" wording in the design plan that says "macOS arm64, Linux x64, Windows x64" is older than the current mcp matrix; matching mcp parity is the right Phase 1 target.

**11. Frontend bundler — Vite, not Rsbuild.** `src/app` uses Rsbuild with a non-trivial config; `src/simlin-serve/web/` is a much smaller shell. Vite gives us a leaner setup, faster dev iteration, and is the most-cited React+TS bundler for new apps in 2025. The build output goes to `src/simlin-serve/web/dist/`, which `rust-embed` slurps into the Rust binary at compile time.

**12. Testing posture.** Per `/home/bpowers/src/simlin/docs/dev/rust.md`: per-test budget under 2-5s, whole `cargo test --workspace` under 3 min. No mocking framework convention — Rust tests prefer real data + tempdirs (`tempfile` crate). For Phase 1, use `axum::Router::oneshot` (or the in-process `tower::ServiceExt::oneshot`) against routers constructed from a `ProjectRegistry` populated from a tempdir. There is no existing live-server (network-bound) test pattern in this workspace; do not introduce one. The frontend uses Jest with `jest-environment-jsdom` and `@testing-library/react` (see `src/diagram/jest.config.js` for a working example).

**13. TS style.** `/home/bpowers/src/simlin/docs/dev/typescript.md` and `/home/bpowers/src/simlin/CLAUDE.md` mandate TypeScript strict mode, **class components by default** (functional components only when wrapping hook-only libraries), and Functional Core / Imperative Shell separation.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
### Subcomponent A: Workspace + crate scaffold + healthcheck

<!-- START_TASK_1 -->
### Task 1: Add `simlin-serve` to the Cargo workspace

**Files:**
- Modify: `/home/bpowers/src/simlin/Cargo.toml:4-10` (workspace `members` list)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/Cargo.toml`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs` (placeholder `fn main() {}`)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/README.md` (one-paragraph description; Phase 8 expands it)

**Implementation:**
- Append `"src/simlin-serve",` to the workspace `members` array (alphabetical order between `simlin-mcp` and `xmutil`).
- New crate `Cargo.toml` declares `name = "simlin-serve"`, `edition = "2024"`, `version = "0.1.0"`, `license = "Apache-2.0"`, `authors = ["Bobby Powers <bobbypowers@gmail.com>"]`. Initial dependencies: only `tokio = { version = "1", features = ["full"] }` and `axum = "0.8"`. Other deps are added by later tasks as they're needed (keeps each commit small).
- `main.rs` is a 1-line placeholder for now (`fn main() {}`); Task 2 fills it in.

**Verification:**
- `cargo metadata --format-version=1 --no-deps | grep simlin-serve` returns the new crate.
- `cargo build -p simlin-serve` succeeds.

**Commit:** `serve: add empty simlin-serve crate to workspace`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Minimal Axum server with `/healthz`, ephemeral-port bind

**Verifies:** none directly (operational scaffold for AC7.2/AC7.4)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (the crate becomes lib + bin so we can integration-test the router)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/Cargo.toml` (add `tower-http = { version = "0.6", features = ["trace"] }`, `tracing = "0.1"`, `tracing-subscriber = { version = "0.3", features = ["env-filter"] }`, `clap = { version = "4", features = ["derive"] }`)

**Implementation:**
- Move setup logic into `lib.rs` so tests can construct the router without spawning a process. Export `pub fn build_router() -> axum::Router` (or with a state argument once `ProjectRegistry` exists in Task 4).
- Implement `GET /healthz` returning `StatusCode::OK` with body `"ok"`.
- In `main.rs`: use `clap` for CLI args (`--port` defaulting to `0` for ephemeral; `--mcp-port` accepted but unused this phase; `--no-open` to suppress browser launch). Bind via `tokio::net::TcpListener::bind(("127.0.0.1", port)).await?`. Read back the actual port via `listener.local_addr()?.port()`. Print the URL to stdout. Initialize `tracing_subscriber::fmt()` with `EnvFilter::from_default_env().add_directive("simlin_serve=info".parse()?)`.
- Wrap router with `TraceLayer::new_for_http()`.

**Testing:**
Verifies: none (infrastructure). Add an integration test at `tests/healthz.rs` that asserts `GET /healthz` returns `200` and body `"ok"` using `tower::ServiceExt::oneshot`. Add `tower = "0.5"` as a dev-dependency.

**Verification:**
- `cargo test -p simlin-serve` passes the healthz test.
- Manual: `cargo run -p simlin-serve` prints a URL like `http://127.0.0.1:<some-port>` and `curl http://127.0.0.1:<port>/healthz` returns `ok`.

**Commit:** `serve: minimal axum server with /healthz on ephemeral port`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: CLI surface — `--port`, `--mcp-port`, `--no-open`, `--root`

**Verifies:** none directly (operational scaffold for AC1.1, AC7.1)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/cli.rs` (clap-derived args struct + `parse_args() -> Args`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/src/cli.rs` (inline `#[cfg(test)] mod tests`)

**Implementation:**
- `Args` struct with: `root: PathBuf` (positional, defaulting to `std::env::current_dir()?`), `port: u16` (default `0`), `mcp_port: u16` (default `7878`; this phase parses but does not use), `no_open: bool` (default `false`).
- `--root` lets users point at a different directory without `cd`. Useful for testing.
- `Args::parse_from(...)` for tests.

**Testing:**
- Inline tests assert default values, --root override, and that `--port 8080` is parsed as `8080`.

**Verification:**
- `cargo test -p simlin-serve cli::` passes.
- Manual: `cargo run -p simlin-serve -- --help` shows the arg surface.

**Commit:** `serve: clap CLI with root, port, mcp-port, no-open`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-7) -->
### Subcomponent B: Filesystem discovery + git status + ProjectRegistry

<!-- START_TASK_4 -->
### Task 4: `ProjectMeta` and `ProjectRegistry` types

**Verifies:** none directly (data structures for AC1, AC2)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (re-export `pub mod registry;`)
- Modify: `Cargo.toml` (add `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`)

**Implementation:**
- `pub enum ProjectFormat { Stmx, Xmile, Mdl, SdJson }` with `Display` / `Serialize` (lowercase strings).
- `pub enum GitState { Tracked { dirty: bool }, Untracked, Unavailable }` with `Serialize`.
- `pub struct ProjectMeta { path: PathBuf, format: ProjectFormat, mtime: SystemTime, size: u64, git: GitState, version: u64 }` with `Serialize`. The `version` field is the optimistic-lock counter introduced in Phase 2; in Phase 1 it's always `0`.
- `pub struct ProjectRegistry(Arc<RwLock<HashMap<PathBuf, ProjectMeta>>>)` with constructor `pub fn new() -> Self`, `pub fn snapshot(&self) -> Vec<ProjectMeta>` (returns a sorted-by-path Vec for deterministic ordering), `pub fn get(&self, path: &Path) -> Option<ProjectMeta>`, `pub fn upsert(&self, meta: ProjectMeta)`, `pub fn remove(&self, path: &Path)`.
- Use `parking_lot::RwLock` if it's already in the workspace; otherwise `std::sync::RwLock`.
- Path keys are stored as **absolute, canonicalized** paths but the `path` field in `ProjectMeta` is the **relative-to-root** display path. The Registry takes a `root: PathBuf` constructor argument to handle the relativization.

**Testing:**
- Unit tests in `registry.rs`: insert two ProjectMeta entries, `snapshot()` returns them sorted by path; `get(absolute_path)` finds by absolute key; `remove()` works.

**Verification:**
- `cargo test -p simlin-serve registry::` passes.

**Commit:** `serve: ProjectRegistry with ProjectMeta, GitState, ProjectFormat`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Filesystem discovery via `ignore`

**Verifies:** server-rewrite.AC1.1, server-rewrite.AC1.2, server-rewrite.AC1.3, server-rewrite.AC1.5

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/discovery.rs`
- Modify: `lib.rs` (re-export `pub mod discovery;`)
- Modify: `Cargo.toml` (add `ignore = "0.4"`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/src/discovery.rs` (inline `#[cfg(test)] mod tests`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/tests/discovery_integration.rs`
- Modify: `Cargo.toml` (add `tempfile = "3"` as a dev-dependency)

**Implementation:**
- `pub fn discover_models(root: &Path) -> Result<Vec<DiscoveredFile>, DiscoveryError>` returns one `DiscoveredFile { absolute_path: PathBuf, format: ProjectFormat }` per match.
- Use `ignore::WalkBuilder::new(root).follow_links(false).git_ignore(true).git_global(true).filter_entry(|entry| { ... })`. The `filter_entry` predicate excludes any directory whose `file_name()` matches a small **universal** hardcoded set: `["node_modules", ".git", "target", "playwright-report", "test-results"]`. (Files inside an excluded dir are never visited.) The `ignore` crate already honors any `.gitignore` the user has, which covers project-specific build artifacts (`lib/`, `build/`, `dist/`, etc.) without us hardcoding them. If real-world usage shows a need for a richer hardcoded list, file as a follow-up.
- Format detection by extension only (no content sniffing in Phase 1):
  - `.stmx` → `ProjectFormat::Stmx`
  - `.xmile` or `.xml` → `ProjectFormat::Xmile`
  - `.mdl` → `ProjectFormat::Mdl`
  - `.sd.json` → `ProjectFormat::SdJson`
  - other → skipped
- `.sd.json` is detected by the literal suffix `.sd.json` (use `path.to_str().map_or(false, |s| s.ends_with(".sd.json"))`).

**Testing:**
Verifies the listed AC cases. Tests must:
- AC1.1: tempdir contains `a.stmx`, `b.xmile`, `c.mdl` at root → all three returned with correct formats.
- AC1.2: `sub/d.stmx` at depth 2 → returned with absolute path containing `sub/d.stmx`.
- AC1.3: place a model file under `tempdir/node_modules/x.stmx`, `tempdir/.git/x.stmx`, `tempdir/target/x.stmx` → none returned.
- AC1.5: create a symlinked dir `tempdir/loop -> tempdir` (use `std::os::unix::fs::symlink` cfg'd for unix) → walk does not loop and does not return symlink-shadowed entries. Skip the test on Windows (`#[cfg(unix)]`).

**Verification:**
- `cargo test -p simlin-serve discovery::` and `cargo test -p simlin-serve --test discovery_integration` pass.

**Commit:** `serve: recursive model discovery with ignore-crate exclusions`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: Git status detection (shell-out + graceful PATH fallback)

**Verifies:** server-rewrite.AC2.1, server-rewrite.AC2.2, server-rewrite.AC2.3, server-rewrite.AC2.5

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/git.rs`
- Modify: `lib.rs` (re-export `pub mod git;`)
- Test: inline `#[cfg(test)] mod tests` in `git.rs`
- Test: `/home/bpowers/src/simlin/src/simlin-serve/tests/git_integration.rs`

**Implementation:**
- `pub struct GitProbe { git_available: bool }` constructed via `pub fn detect() -> Self` which runs `Command::new("git").arg("--version").output()`. If the command fails to spawn (PATH miss) or returns non-zero, set `git_available = false`. Cache the result for the process lifetime.
- `pub fn enclosing_git_root(file: &Path) -> Option<PathBuf>`: walks from `file.parent()` upward looking for a `.git` directory or file (worktrees use a regular file). Returns the working-tree root or `None`.
- `pub fn status_for(&self, path: &Path) -> GitState`:
  - If `!self.git_available`, return `GitState::Unavailable`.
  - Find the enclosing git root; if `None`, return `GitState::Untracked`.
  - Cache `(repo_root, mtime_of_index)` → parsed status map keyed by absolute path. On cache hit, return the cached entry. On miss or stale cache, run `git status --porcelain --untracked-files=all` with `current_dir(repo_root)` and parse the output:
    - Lines starting with `??` are untracked-but-known to git (this is a quirk of `--untracked-files=all`; we treat them as `Tracked { dirty: true }` because they are within the tree but not yet committed).
    - All other lines are `Tracked { dirty: true }` for the listed path.
    - Anything in the repo not appearing in the porcelain output is `Tracked { dirty: false }`.
  - To distinguish "tracked clean" from "outside index", run `git ls-files --error-unmatch <path>` (cached per repo via `git ls-files` once). If the path is in the file list, it's `Tracked`; otherwise it's `Untracked` (a file inside the working tree but not in the index, e.g. an `*.stmx` matching `.gitignore`).
- The cache key includes the mtime of `<repo_root>/.git/index` so cache invalidation is automatic; Phase 4 will add explicit invalidation via the file watcher (per AC2.4).

**Testing:**
Verifies the listed AC cases. Use `tempfile::TempDir`, `git init`, file writes, and `git add`/`git commit` via `Command`. Tests:
- AC2.1: file added + committed in a tempdir git repo → `Tracked { dirty: false }`.
- AC2.2: file modified after commit → `Tracked { dirty: true }`.
- AC2.3: file in a tempdir with no `.git` → `Untracked`.
- AC2.5: stub `GitProbe { git_available: false }` (constructed manually for this test, bypassing `detect()`) → all calls return `Unavailable`.
- Skip the integration tests if `git` is not on PATH (`use_test_skip!` pattern: read `which git`; if absent, `eprintln!` and `return`). Per project policy, the suite should fail loudly if a required helper is missing — `git` is universal enough that we treat absence as a hard fail in CI but a soft skip locally is acceptable; document the choice with a comment.

**Verification:**
- `cargo test -p simlin-serve git::` and `cargo test -p simlin-serve --test git_integration` pass.

**Commit:** `serve: per-file git status with ls-files + porcelain caching`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Discovery + git → registry population

**Verifies:** server-rewrite.AC1, server-rewrite.AC2 (composition)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/scan.rs`
- Modify: `lib.rs` (re-export `pub mod scan;`)
- Test: inline `#[cfg(test)] mod tests` in `scan.rs`

**Implementation:**
- `pub fn scan_into_registry(root: &Path, registry: &ProjectRegistry, git: &GitProbe) -> Result<usize, ScanError>` walks via `discovery::discover_models`, stats each file (`fs::metadata` for size + mtime), runs `git.status_for(&path)`, and `registry.upsert(...)`s the resulting `ProjectMeta`. Returns the number of files inserted.
- `ScanError` wraps `io::Error` with the path that caused it; per-file errors are logged via `tracing::warn!` and skipped, not propagated (one bad file shouldn't kill discovery).

**Testing:**
- Unit test populates a tempdir, calls `scan_into_registry`, asserts the registry has the right number of entries with the right formats and git states (mostly `Untracked` since we don't `git init` the tempdir in unit tests).

**Verification:**
- `cargo test -p simlin-serve scan::` passes.

**Commit:** `serve: glue discovery + git into ProjectRegistry`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 8-10) -->
### Subcomponent C: HTTP read endpoints

<!-- START_TASK_8 -->
### Task 8: `GET /api/projects` — list projects

**Verifies:** server-rewrite.AC1.1, server-rewrite.AC1.2 (HTTP surface), server-rewrite.AC2.1-2.3 (HTTP surface), server-rewrite.AC2.5

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs`
- Modify: `lib.rs` (re-export `pub mod handlers;`; update `build_router` to mount the new endpoint with `State<AppState>`)
- Modify: `Cargo.toml` (add `serde_qs = "0.13"` only if needed for query strings; otherwise rely on `axum::extract::Query`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/tests/api_projects.rs`

**Implementation:**
- `pub struct AppState { pub registry: Arc<ProjectRegistry>, pub git: Arc<GitProbe>, pub root: Arc<PathBuf> }` (cheaply cloned via `Arc`).
- Handler `pub async fn list_projects(State(state): State<AppState>) -> Json<ListProjectsResponse>` returns `{ projects: [{path, format, git, mtime, size, version}, ...], git_available: bool }`. Re-runs `scan_into_registry` on each call **for now** — Phase 4 will replace this with file-watcher-driven population. (Trade-off: Phase 1 is simple and correct but does I/O on every request. That's fine for a single-user local tool.)
- `path` in the response is the relative path from `state.root`, with forward slashes (call `path.to_string_lossy().replace(MAIN_SEPARATOR, '/')`). Keys in URL routes will use this same form.
- `git_available` mirrors `state.git.git_available` so the SPA can show the AC2.5 one-time hint.

**Testing:**
Tests build a router with a tempdir-backed `AppState`, populate it with a few model files, hit the route via `tower::ServiceExt::oneshot`, parse the JSON response, and assert:
- AC1.1: all three formats (`.stmx`, `.xmile`, `.mdl`) are listed.
- AC1.2: relative paths (e.g., `sub/d.stmx`) are present and use forward slashes.
- AC2.5: with a stubbed `GitProbe { git_available: false }`, every entry's `git` is `"unavailable"` and the response has `git_available: false`.

**Verification:**
- `cargo test -p simlin-serve --test api_projects` passes.

**Commit:** `serve: GET /api/projects returns registry snapshot`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: `GET /api/projects/{*path}` — read a project as JSON

**Verifies:** server-rewrite.AC3.1 (read), server-rewrite.AC3.3 (read)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/parse.rs` (extension-driven dispatcher; the Phase 5 refactor will consolidate this with `simlin-mcp`'s `open_project`)
- Modify: `Cargo.toml` (add `simlin-engine = { path = "../simlin-engine" }`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/tests/api_get_project.rs`
- Test fixtures: copy minimal `*.stmx`, `*.xmile`, `*.mdl`, `*.sd.json` files into `src/simlin-serve/tests/fixtures/` (use the smallest existing fixtures from `test/` directories — e.g., `test/xmile/teacup/teacup.stmx`)

**Implementation:**
- `pub fn parse_to_datamodel(path: &Path, format: ProjectFormat, contents: &str) -> Result<datamodel::Project, ParseError>`:
  - `Stmx` or `Xmile` → `simlin_engine::open_xmile(&mut Cursor::new(contents))`
  - `Mdl` → `simlin_engine::open_vensim(contents)` *(see Note 4 above re: xmutil)*
  - `SdJson` → parse via `serde_json::from_str::<simlin_engine::json::Project>(contents)`, then `.into()` for `datamodel::Project`. (The exact import path for the JSON shape is `simlin_engine::json::Project`; the conversion is `From<json::Project> for datamodel::Project` at `src/simlin-engine/src/json.rs:1300`.)
- `pub fn datamodel_to_canonical_json(project: &datamodel::Project) -> Result<String, serde_json::Error>`: round-trip via `simlin_engine::json::Project::from(project)` → `serde_json::to_string(&json_project)`. Note: this is **not** byte-stable canonical JSON yet — Phase 3 hardens that property. For Phase 1 we only need consistency-within-a-single-process.
- Handler `pub async fn get_project(State(state): State<AppState>, Path(rel_path): Path<String>) -> Result<Json<GetProjectResponse>, ApiError>`:
  - Use the Axum 0.8 wildcard route syntax: `/api/projects/{*rel_path}`.
  - Sanitize `rel_path`: reject if it contains `..` segments, absolute path indicators, or null bytes. Return `400 Bad Request` for any rejection.
  - Build absolute path = `state.root.join(&rel_path).canonicalize()?`. If the canonical absolute path is not a descendant of `state.root.canonicalize()?`, return `403 Forbidden` (defense-in-depth path traversal check).
  - **`.mdl` sidecar preference:** If the request is for a `.mdl` and a sibling `<basename>.sd.json` exists, return the sidecar's JSON instead of parsing the `.mdl`. (Per design: the sidecar becomes source-of-truth once it exists. Phase 1 implements the read side; Phase 2 implements the write side that creates the sidecar.) Implement as: if `rel_path` ends in `.mdl`, check for a sibling `.sd.json` first, swap to that path + format if found.
  - Read the file via `fs::read_to_string` (4 MiB cap via the body-limit layer). Parse via `parse_to_datamodel`. Serialize via `datamodel_to_canonical_json`.
  - Look up `version` from the registry (always `0` in Phase 1; Phase 2 increments on save).
  - Return `200 OK` with `{ json: <stringified canonical JSON>, version: <u64>, source_format: "stmx"|"xmile"|"mdl"|"sd_json" }`.
- `ApiError` is an enum (`NotFound`, `BadRequest(String)`, `Forbidden`, `Internal(anyhow::Error)`) implementing `IntoResponse` per Axum's pattern. Add `anyhow = "1"` if needed; otherwise hand-roll the error type.

**Testing:**
Verifies the listed AC cases. Tests:
- AC3.1: `GET /api/projects/teacup.stmx` against a fixtures-backed registry returns `200`, body has non-empty `json` field, parses back as JSON, `source_format == "stmx"`.
- AC3.1: same for an `.xmile` fixture.
- AC3.3: `GET /api/projects/<vensim>.mdl` returns `200`, `source_format == "mdl"`.
- Sidecar preference: write a `population.mdl` and a sibling `population.sd.json`. Request `/api/projects/population.mdl` returns the sidecar's content (`source_format == "sd_json"`).
- Path traversal: `GET /api/projects/../../etc/passwd` returns `400` or `403`.
- Not found: `GET /api/projects/missing.stmx` returns `404`.

**Verification:**
- `cargo test -p simlin-serve --test api_get_project` passes.

**Commit:** `serve: GET /api/projects/{*path} returns canonical JSON`
<!-- END_TASK_9 -->

<!-- START_TASK_10 -->
### Task 10: Wire `AppState` into `build_router`, mount tracing + body limit

**Verifies:** none directly (composition)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (`build_router(state: AppState) -> Router` returns the wired-up router)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs` (constructs `AppState` from `Args`, calls `build_router`, serves)
- Modify: `Cargo.toml` (add `tower-http` feature `"limit"` to the existing tower-http entry)

**Implementation:**
- `build_router` mounts `/healthz`, `/api/projects`, `/api/projects/{*path}` and applies layers in this order: `RequestBodyLimitLayer::new(4 * 1024 * 1024)` (4 MiB; saves are bigger but Phase 1 is read-only — Phase 2 may bump this), `TraceLayer::new_for_http()`. Apply via `Router::layer(...)`.
- `main` constructs `AppState { registry: Arc::new(ProjectRegistry::new(args.root.clone())), git: Arc::new(GitProbe::detect()), root: Arc::new(args.root.canonicalize()?) }`. Calls `scan_into_registry` once at startup so the first request is fast.

**Verification:**
- Existing tests still pass.
- Manual: `cargo run -p simlin-serve -- /path/to/some/sd-models/dir` then `curl 127.0.0.1:<port>/api/projects` returns the populated list.

**Commit:** `serve: compose router with state, tracing, and body limit`
<!-- END_TASK_10 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (tasks 11-15) -->
### Subcomponent D: Frontend SPA shell + rust-embed

<!-- START_TASK_11 -->
### Task 11: Vite + React TypeScript scaffold under `web/`

**Verifies:** none directly (frontend infrastructure)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/package.json`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/tsconfig.json`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/vite.config.ts`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/index.html`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/main.tsx`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/App.tsx` (placeholder; Task 13 fills it in)
- Modify: `/home/bpowers/src/simlin/.gitignore` (add `src/simlin-serve/web/dist/` and `src/simlin-serve/web/node_modules/`; if those paths are already covered by existing globs verify and skip)

**Implementation:**
- `web/package.json` declares dependencies: `react`, `react-dom`, `@simlin/diagram` (workspace ref `"workspace:*"` if pnpm workspaces are in use; otherwise `"*"`), `@simlin/engine`, `@simlin/core`. Dev: `typescript`, `vite`, `@vitejs/plugin-react`, `@types/react`, `@types/react-dom`. Scripts: `"dev"`, `"build"` (outputs `dist/`), `"test"` (jest), `"lint"`.
- Verify whether the monorepo uses pnpm workspaces by checking `/home/bpowers/src/simlin/pnpm-workspace.yaml` (if present) and `/home/bpowers/src/simlin/package.json` `workspaces` field. Match whatever convention `src/diagram/package.json` uses for its workspace deps.
- `vite.config.ts` configures the React plugin and a `base: './'` for relative asset URLs (so `rust-embed` can serve them from any subpath).
- `index.html` mounts a `<div id="root"></div>` and `<script type="module" src="/src/main.tsx"></script>`.
- `main.tsx` creates the React root and renders `<App />`.
- `App.tsx` placeholder: `class App extends React.Component { render() { return <h1>Simlin Serve</h1>; } }`.

**Verification:**
- `cd src/simlin-serve/web && pnpm install && pnpm build` succeeds (or `npm` equivalent matching the monorepo's package manager).
- `web/dist/index.html` exists.

**Commit:** `serve: Vite + React TypeScript scaffold under web/`
<!-- END_TASK_11 -->

<!-- START_TASK_12 -->
### Task 12: Project list sidebar + read-only Editor host

**Verifies:** server-rewrite.AC1.1 (UI surface), server-rewrite.AC2.1-2.3 (UI surface), server-rewrite.AC2.5 (UI hint), server-rewrite.AC3.1 (UI render), server-rewrite.AC3.3 (UI render), server-rewrite.AC1.4 (empty state, partial)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/App.tsx` (the `App` class becomes the shell)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/api.ts` (typed fetchers for `/api/projects` and `/api/projects/{*path}`)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.tsx`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EmptyState.tsx`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/styles.css` (basic layout)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.test.tsx`
- Test: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.test.tsx`

**Implementation:**
- `App` (class) holds `state = { projects: ProjectMeta[] | null, gitAvailable: boolean, selectedPath: string | null, gitHintDismissed: boolean }`. `componentDidMount` fetches `/api/projects` and updates state.
- Layout: two-column. Left = `<ProjectList>` showing each file's relative path and a chip rendering its git state (green="tracked clean", amber="modified", grey="untracked", red-warning="not in any repo"). Selecting an entry sets `selectedPath`. Right = `<EditorHost path={selectedPath} />` or `<EmptyState />` when no projects exist (per AC1.4 — Phase 1 renders the empty state but does not yet render the "Create new model" button; that's Phase 8).
- AC2.5: when `gitAvailable === false` and `!gitHintDismissed`, render a one-time dismissable banner ("git not on PATH — version-control state will not be shown") above the list. Persist dismissal to `sessionStorage`.
- `<EditorHost path={path}>`:
  - On mount / when `path` changes: fetch `GET /api/projects/<encoded path>` (URL-encode each path segment with `encodeURIComponent`; preserve `/` separators). Sets state `{ json: string, version: number, sourceFormat: string } | null`.
  - Renders `<Editor inputFormat="json" initialProjectJson={json} initialProjectVersion={version} name={path} embedded={true} readOnlyMode={true} onSave={async () => undefined} />`. The `embedded` flag makes the canvas non-interactive; `readOnlyMode` adds the user-facing warning toast; the no-op `onSave` satisfies the type system (Phase 2 wires real save).
  - When `sourceFormat === "mdl"` and the file is not yet sidecar-backed, render a small banner near the toolbar ("Vensim MDL — saves will be written to a `.sd.json` sidecar"). Phase 2 actually performs the sidecar write; Phase 1's banner is informational.
- API typings in `api.ts` mirror the Rust response structs.

**Testing:**
Tests use Jest + jsdom + @testing-library/react.
- AC1.1: render `ProjectList` with three mock projects → all three rows visible.
- AC2.1, AC2.2, AC2.3: render with mixed git states → correct chip color/text per row.
- AC2.5: render `App` with mocked `gitAvailable: false` → banner present; click dismiss → banner removed; sessionStorage updated.
- AC3.1: render `EditorHost` with a mocked successful fetch returning the teacup model JSON → Editor renders without error (use `screen.findByRole(...)` to wait for async fetch).
- AC1.4: render `App` with `projects: []` → `EmptyState` renders, no "Create new model" button.
- Mock `fetch` via `jest.spyOn(global, 'fetch')`.

**Verification:**
- `cd src/simlin-serve/web && pnpm jest src/components/ProjectList.test.tsx src/components/EditorHost.test.tsx` (run both newly-introduced test files explicitly to verify they pass).
- `cd src/simlin-serve/web && pnpm test` (full frontend suite) passes.
- Manual: `cargo run -p simlin-serve -- ./test` (against a directory with model files) opens the browser to a list of files; clicking a `.stmx` renders its diagram in non-editable mode.

**Commit:** `serve: project list + read-only Editor host SPA`
<!-- END_TASK_12 -->

<!-- START_TASK_13 -->
### Task 13: Embed `web/dist/` into the binary via `rust-embed`

**Verifies:** server-rewrite.AC7.1 (binary self-contained), server-rewrite.AC7.4 (manual launch)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/static_assets.rs`
- Modify: `lib.rs` (re-export; mount static routes in `build_router`)
- Modify: `Cargo.toml` (add `rust-embed = "8"`, `mime_guess = "2"`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/build.rs` (new) — runs `pnpm install --frozen-lockfile` and `pnpm build` in `web/` before the Rust build, **only if** `web/dist/index.html` is older than `web/src/` files. To keep the dev loop fast, gate the auto-build behind an env var (`SIMLIN_SERVE_BUILD_WEB=1`); CI sets this. Locally, developers run `pnpm build` in `web/` manually before `cargo build`.
- Modify: `Cargo.toml` (add `[build-dependencies] anyhow = "1"`)

**Implementation:**
- `#[derive(rust_embed::Embed)] #[folder = "web/dist/"] struct Assets;` in `static_assets.rs`.
- Handler `pub async fn static_handler(uri: Uri) -> Response`:
  - Strip the leading `/` from `uri.path()` and treat empty path as `index.html`.
  - `Assets::get(&path)` → if `Some`, build a response with `Content-Type` from `mime_guess::from_path(&path).first_or_octet_stream()` and the embedded bytes.
  - If `None` and the path looks like an asset (contains `.`), return `404`. Otherwise, **fall through to `index.html`** (SPA catch-all).
- Mount routes: `Router::new().route("/", get(serve_index)).fallback(static_handler)`. The API routes from earlier tasks (mounted via `.route("/api/...", ...)`) take precedence over the fallback.
- `build.rs` shells out to `pnpm` only when `SIMLIN_SERVE_BUILD_WEB=1`; emits `cargo:rerun-if-changed=web/src` and `cargo:rerun-if-changed=web/index.html` so cargo recomputes when frontend sources change. Cross-build (Task 19) sets the env var.

**Testing:**
- Unit test (in `static_assets.rs`): with a stub `Assets` shim or a small fixtures embed (use a `web/dist-test/` directory checked into the repo or use the rust-embed `#[folder]` re-pointing trick — easiest is to write a small e2e that requires `web/dist/` to exist and skips otherwise).
- Integration test (`tests/static_assets.rs`): if `web/dist/index.html` does not exist (i.e., the test environment skipped the JS build), `eprintln!("web/dist not built; skipping static_assets test")` and return. Otherwise `GET /` returns 200 with `text/html` and the body contains `<div id="root">`. `GET /missing-route` (no `.` in path) returns 200 + index.html (SPA fallback). `GET /assets/missing.js` (has `.` and no entry) returns 404.

**Verification:**
- `cd src/simlin-serve/web && pnpm build` then `cargo build -p simlin-serve` produces a binary that serves the SPA on `/`.
- `cargo test -p simlin-serve --test static_assets` either passes or skips with the documented message.

**Commit:** `serve: embed web/dist via rust-embed with SPA fallback`
<!-- END_TASK_13 -->

<!-- START_TASK_14 -->
### Task 14: End-to-end smoke test — Rust binary serves an Editor for a fixture

**Verifies:** server-rewrite.AC1.1 + AC3.1 (composition end-to-end)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/e2e_smoke.rs`

**Implementation:**
- Build the router via `build_router(test_state)` where `test_state` points at a tempdir containing one fixture `.stmx`.
- `oneshot` `GET /api/projects` → assert one entry.
- `oneshot` `GET /api/projects/teacup.stmx` → assert `200`, JSON body parses, has the expected variable count from the fixture.
- `oneshot` `GET /` → expect 200 with HTML body. Skip with documented message if `web/dist/` is not present.
- `oneshot` `GET /healthz` → 200 + `"ok"`.

**Verification:**
- `cargo test -p simlin-serve --test e2e_smoke` passes (or skips static-assets portion if web/dist missing).

**Commit:** `serve: end-to-end smoke test for read-only viewer`
<!-- END_TASK_14 -->

<!-- START_TASK_15 -->
### Task 15: Wire frontend HTTP fetches to use the launch token

**Verifies:** server-rewrite.AC7.3 (token storage; bearer enforcement deferred to Phase 3)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/main.tsx`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/api.ts`

**Implementation:**
- On startup, `main.tsx` parses `window.location.search` for a `token` query param. If present, store under `sessionStorage.setItem('simlin-serve-token', token)`. Then call `window.history.replaceState(null, '', window.location.pathname)` to remove the token from the visible URL.
- `api.ts` reads `sessionStorage.getItem('simlin-serve-token')` and includes `Authorization: Bearer <token>` on all `/api/*` fetches. (Phase 1 doesn't enforce the bearer server-side; the wiring is added now so Phase 3's enforcement work is purely server-side.)

**Testing:**
- Jest test: bootstrap `main.tsx`-equivalent in jsdom, set `window.location.search = '?token=abc'`, assert `sessionStorage` is populated and the URL is rewritten.
- Jest test for `api.ts`: with `sessionStorage` populated, mocked `fetch` is called with the `Authorization` header.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes new tests.

**Commit:** `serve: SPA stores launch token from URL into sessionStorage`
<!-- END_TASK_15 -->
<!-- END_SUBCOMPONENT_D -->

<!-- START_SUBCOMPONENT_E (tasks 16-17) -->
### Subcomponent E: Token + browser launcher

<!-- START_TASK_16 -->
### Task 16: One-time launch-token generation

**Verifies:** server-rewrite.AC7.3 (token issuance)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/token.rs`
- Modify: `lib.rs` (re-export)
- Modify: `Cargo.toml` (add `rand = "0.10"`, `base64 = "0.22"`)
- Test: inline `#[cfg(test)] mod tests` in `token.rs`

**Implementation:**
- `pub fn generate_launch_token() -> String`: 32 bytes from `rand::rng().fill_bytes(...)`, encoded with `base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(...)`. Result is a 43-character URL-safe string with 256 bits of entropy.

**Testing:**
- Repeated calls produce different tokens.
- All output characters are in `[A-Za-z0-9_-]`.
- Length is exactly 43.

**Verification:**
- `cargo test -p simlin-serve token::` passes.

**Commit:** `serve: launch-token generation via rand + base64url`
<!-- END_TASK_16 -->

<!-- START_TASK_17 -->
### Task 17: Browser launcher via `open` crate, headless fallback

**Verifies:** server-rewrite.AC7.2 (HTTP URL only — MCP URL is Phase 6), server-rewrite.AC7.4

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/launcher.rs`
- Modify: `lib.rs` (re-export)
- Modify: `Cargo.toml` (add `open = "5"`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs` (after binding the listener and learning the port: print URLs, then call `launcher::open_browser(&url)` unless `args.no_open` is set)
- Test: inline tests in `launcher.rs`

**Implementation:**
- `pub fn open_browser(url: &str) -> bool`: returns `true` if `open::that(url)` succeeded, `false` otherwise. On failure, prints `"could not open browser automatically; visit: <url>"` to stderr.
- In `main.rs`: after binding, build URL `format!("http://127.0.0.1:{}/?token={}", port, token)`, print to stdout (always, regardless of `--no-open` — the user always needs to see the URL), then conditionally launch.

**Testing:**
- `open_browser` is hard to unit-test (it actually launches the browser). Test only the URL-building logic via a separate pure helper if useful.
- Compile-time only: ensure `cargo build -p simlin-serve` succeeds with the new dep.

**Verification:**
- Manual: `cargo run -p simlin-serve` opens a browser to the right URL with the token query param.
- Manual: `cargo run -p simlin-serve -- --no-open` does not open a browser but prints the URL.
- Manual headless test (Linux, no DISPLAY): `unset DISPLAY && cargo run -p simlin-serve` should print the URL and continue running, not crash.

**Commit:** `serve: browser launcher with --no-open and headless fallback`
<!-- END_TASK_17 -->
<!-- END_SUBCOMPONENT_E -->

<!-- START_SUBCOMPONENT_F (tasks 18-21) -->
### Subcomponent F: npm shim + cross-build + release plumbing

<!-- START_TASK_18 -->
### Task 18: `@simlin/serve` npm wrapper package

**Verifies:** server-rewrite.AC7.1 (npm wrapper)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/package.json`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/bin/simlin-serve.js`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/.npmignore`

**Implementation:**
- Mirror `src/simlin-mcp/package.json` exactly, substituting `mcp` → `serve`:
  - `"name": "@simlin/serve"`, `"version": "0.1.0"`, `"description": "Local-first system dynamics model server"`, `"bin": { "simlin-serve": "bin/simlin-serve.js" }`, `"files": ["bin"]`, `"license": "Apache-2.0"`, `"type": "module"`, `"publishConfig": {"access": "public"}`, `"repository": {...}`.
  - `"optionalDependencies": { "@simlin/serve-darwin-arm64": "0.1.0", "@simlin/serve-linux-arm64": "0.1.0", "@simlin/serve-linux-x64": "0.1.0", "@simlin/serve-win32-x64": "0.1.0" }`. (Four platforms in Phase 1, matching `@simlin/mcp`. AC7.1's `darwin-x64` arrives in Phase 8.)
  - `"os": ["darwin", "linux", "win32"]`, `"cpu": ["arm64", "x64"]`.
- `bin/simlin-serve.js`: copy from `src/simlin-mcp/bin/simlin-mcp.js` with `simlin-mcp` → `simlin-serve` and `@simlin/mcp-` → `@simlin/serve-`. The shim:
  - Builds `platformKey = "${process.platform}-${process.arch}"`, looks up in `PLATFORM_MAP`, `require.resolve('${pkg}/package.json')`, executes `pkgDir/bin/simlin-serve[.exe]` with `spawn(...)`, signal forwarding, exit code mirroring.
  - PLATFORM_MAP entries: `darwin-arm64 → @simlin/serve-darwin-arm64 / aarch64-apple-darwin`, `linux-arm64 → @simlin/serve-linux-arm64 / aarch64-unknown-linux-musl`, `linux-x64 → @simlin/serve-linux-x64 / x86_64-unknown-linux-musl`, `win32-x64 → @simlin/serve-win32-x64 / x86_64-pc-windows-gnu`.
  - Falls back to `vendor/<triple>/simlin-serve` for local dev.
- `.npmignore`: only ship `bin/` and `package.json`.

**Testing:**
- Reuse the `simlin-mcp/tests/build_npm_packages.rs` pattern as a model: create `src/simlin-serve/tests/build_npm_packages.rs` that runs the build script (Task 19) and validates the produced platform packages have the right shape. Implementation comes in Task 19.

**Verification:**
- `node src/simlin-serve/bin/simlin-serve.js --help` either runs the binary (if vendored) or fails with the documented "platform binary not found" error message including remediation hints.

**Commit:** `serve: npm @simlin/serve wrapper package and runtime shim`
<!-- END_TASK_18 -->

<!-- START_TASK_19 -->
### Task 19: Per-platform package generator (`build-npm-packages.sh`)

**Verifies:** server-rewrite.AC7.1 (per-platform packages)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/build-npm-packages.sh` (mirror `src/simlin-mcp/build-npm-packages.sh`)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/build_npm_packages.rs`

**Implementation:**
- Mirror the `simlin-mcp` script: read version from `Cargo.toml`, generate `npm/@simlin/serve-{darwin-arm64,linux-arm64,linux-x64,win32-x64}/package.json` files with matching `os`/`cpu`/`version`/`bin` fields, no source files copied (binaries are dropped in via the cross-build step).
- Test mirrors `simlin-mcp/tests/build_npm_packages.rs`: invokes the script, asserts the generated files exist with the correct `version` and `os`/`cpu` constraints.

**Verification:**
- `bash src/simlin-serve/build-npm-packages.sh` (run from the repo root) creates the three platform package directories under `src/simlin-serve/npm/@simlin/`.
- `cargo test -p simlin-serve --test build_npm_packages` passes.

**Commit:** `serve: per-platform npm package generator`
<!-- END_TASK_19 -->

<!-- START_TASK_20 -->
### Task 20: Cross-build via Docker + cargo-zigbuild

**Verifies:** server-rewrite.AC7.1 (binary cross-build)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/scripts/cross-build.sh`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/Dockerfile.cross`

**Implementation:**
- Mirror `src/simlin-mcp/scripts/cross-build.sh` and `src/simlin-mcp/Dockerfile.cross`. The Dockerfile installs the rust toolchain pinned by `rust-toolchain.toml`, `cargo-zigbuild`, and zig. (The design plan line 267 mentions a workspace-level `cross-build.sh`; we intentionally land the per-crate location here to mirror `@simlin/mcp` and minimize churn. The shared infra factor-out is the follow-up filed via `track-issue` in Task 21.)
- The script builds four targets: `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, `x86_64-pc-windows-gnu`, and (non-Docker, run only on a macOS runner) `aarch64-apple-darwin`. The three non-macOS targets are produced inside the Docker container via cargo-zigbuild.
- Output binaries land in `src/simlin-serve/dist/<triple>/simlin-serve[.exe]`.
- The build sets `SIMLIN_SERVE_BUILD_WEB=1` so `build.rs` (Task 13) builds `web/dist/` automatically inside the container.
- The Dockerfile must include `pnpm` and `node` so the frontend build can run inside the container. (Inspect `mcp/Dockerfile.cross` first — if it already has node/pnpm, mirror; if not, add them and document the divergence.)

**Verification:**
- `bash src/simlin-serve/scripts/cross-build.sh linux-x64` builds the Linux binary inside Docker (requires Docker daemon).
- `file src/simlin-serve/dist/x86_64-unknown-linux-musl/simlin-serve` reports an ELF musl binary.

**Commit:** `serve: docker-based cross-compile via cargo-zigbuild`
<!-- END_TASK_20 -->

<!-- START_TASK_21 -->
### Task 21: GitHub Actions release workflow `serve-release.yml`

**Verifies:** server-rewrite.AC7.1 (CI release pipeline)

**Files:**
- Create: `/home/bpowers/src/simlin/.github/workflows/serve-release.yml`
- Create: `/home/bpowers/src/simlin/scripts/release-serve.sh`
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/serve_release_workflow.rs`

**Implementation:**
- Mirror `.github/workflows/mcp-release.yml`. Trigger: `serve-v*` tag push. Four build matrix entries (linux-x64, linux-arm64, win32-x64 via cargo-zigbuild on `ubuntu-latest`; darwin-arm64 native on `macos-latest`). Publish sequence: platform packages first, then wrapper. NPM OIDC provenance (`--provenance`).
- `scripts/release-serve.sh <version>`: bumps `Cargo.toml` version (in `src/simlin-serve/`), runs `cargo test -p simlin-serve`, runs `bash src/simlin-serve/build-npm-packages.sh`, commits, creates tag `serve-v<version>`. Does **not** push (mirrors `release-mcp.sh`).
- Test in `tests/serve_release_workflow.rs` (mirroring `simlin-mcp/tests/mcp_release_workflow.rs`) parses the YAML and asserts: the trigger pattern, the matrix entries, the publish order, the secret refs.
- File a follow-up via `track-issue` agent: factor the shared cross-build infrastructure between `simlin-mcp` and `simlin-serve` into `scripts/lib/` once both consumers exist (per design line 267 — "File a follow-up ticket to factor shared cross-build infrastructure once two consumers exist").

**Verification:**
- `cargo test -p simlin-serve --test serve_release_workflow` passes.
- `bash scripts/release-serve.sh 0.1.0-test --dry-run` (if the script supports `--dry-run`; otherwise verify by reading) does the right transformations.
- The workflow YAML lints as valid GitHub Actions (use `gh workflow view` after pushing).

**Commit:** `serve: GitHub Actions release workflow and release-serve.sh`
<!-- END_TASK_21 -->
<!-- END_SUBCOMPONENT_F -->

---

## Phase Verification Checklist

Before marking Phase 1 complete, run all of the following from the repo root:

1. `pnpm install` (idempotent monorepo bootstrap)
2. `cd src/simlin-serve/web && pnpm build && cd -` (builds the SPA)
3. `cargo build -p simlin-serve --release` (builds the Rust binary; with `SIMLIN_SERVE_BUILD_WEB=1` for hands-off mode)
4. `cargo test -p simlin-serve` (all unit + integration tests pass)
5. `cargo test --workspace` (no regressions elsewhere)
6. `pnpm --filter ./src/simlin-serve/web test` (frontend tests pass)
7. `pnpm --filter ./src/simlin-serve/web lint` (frontend lint passes)
8. `cargo clippy -p simlin-serve -- -D warnings` (clippy clean)
9. `cargo fmt -p simlin-serve --check` (formatted)
10. Manual: `cargo run -p simlin-serve -- ./test` opens a browser, lists model files with correct git status, and clicking a `.stmx`/`.xmile`/`.mdl` renders its diagram in non-editable mode.
11. Manual cross-build (run from a Linux dev machine with Docker): `bash src/simlin-serve/scripts/cross-build.sh linux-x64` produces a working `dist/x86_64-unknown-linux-musl/simlin-serve` binary.

If all 11 verifications pass, Phase 1 is done. The remaining ACs are deferred to later phases as documented in the AC Coverage section above.
