# Phase 8: V1 Polish Implementation Plan

**Goal:** Close out V1 by adding the remaining user-facing affordances (new-project creation, file-rename handling, basename-collision disambiguation), the security defense-in-depth that the design plan calls for (rotated launch token + documented threat model + DNS-rebinding mitigation), the user-facing documentation, and a cross-platform smoke test that runs in CI on macOS arm64, Linux x64, and Windows x64.

**Architecture:** Phases 1-7 already provide the infrastructure (registry, file watcher, MCP, notifications, frontend shell). Phase 8 layers polish on top:
- **New-project UX**: a `<NewProjectButton>` component renders an inline form (filename + format dropdown), POSTs to a new `POST /api/projects/new` endpoint that delegates to `RegistryAccess::create` (Phase 6 Task 3, generalized for HTTP use), then navigates the editor to the new file.
- **Rename handling**: extend Phase 4's `WatcherActor::classify` with a `Renamed { from, to, format }` variant for `RenameMode::Both` events (the canonical paired form). The handler re-keys the registry entry (preserving the in-memory `LoroDoc`, version, and content hash) and broadcasts `WsMessage::ProjectRenamed { from, to }`. The frontend updates the entry in place; if the renamed file is the currently-viewed one, the editor's path state updates without remounting.
- **Disambiguation**: a pure client-side helper `disambiguatedLabels(projects)` examines basename collisions and renders the full relative path for any colliding entry, the bare basename otherwise.
- **Threat model + middleware**: a new `host_origin_validator` tower middleware checks the `Host:` header (HTTP) and `Origin:` header (WS upgrades) against an allowlist (`127.0.0.1:<port>`, `localhost:<port>`); rejects others with `421 Misdirected Request`. This is defense-in-depth against DNS rebinding (cf. CVE-2025-66414 in the official MCP TypeScript SDK).
- **Documentation**: `src/simlin-serve/README.md` (mirrors `@simlin/mcp`'s structure) + `docs/threat-model.md` (V1 threat model with the per-launch-token rotation, loopback boundary, and DNS-rebinding defense documented).
- **Smoke test**: a Rust integration test (`tests/smoke.rs`, gated by `#[ignore]`) spawns the binary, exercises the browser API path and the MCP tool path, and verifies disk state. A new GitHub Actions matrix job runs it on `ubuntu-latest`, `macos-latest`, and `windows-latest`.

**Tech Stack:** No new dependencies. Polish only.

**Scope:** Phase 8 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC1: Discovery and listing (closeout)
- **server-rewrite.AC1.4 Edge:** Directories with no model files render an empty-state UI with a "Create new model" affordance *(now fully covered — the affordance is added)*

### server-rewrite.AC7 (closeout): Distribution and bootstrap
- **server-rewrite.AC7.1 Success:** `npx @simlin/serve` works on macOS (x64 and arm64), Linux (x64 and arm64), and Windows (x64) by downloading the appropriate prebuilt native binary as an `optionalDependencies` install *(Phase 1 shipped 4 platforms matching `@simlin/mcp` parity; Phase 8 deals with the remaining `darwin-x64` (macOS Intel) gap. See Notes for whether to do this in V1 or as a follow-up.)*

This phase also makes a smoke-test pass on `[macos-latest, ubuntu-latest, windows-latest]` (the 3 platforms Phase 1 ships) — verifies AC7 end-to-end in CI.

### Phase 8 also delivers (not new ACs, but design components):
- File rename handling (Phase 4's design line 318 mentioned "renames update the path key"; Phase 4 deferred rename to here)
- Multiple files with the same basename in different subdirectories are disambiguated in the UI (full relative path shown when ambiguous)
- One-time bearer token rotated on each launch + documented threat model
- Smoke test boots `simlin-serve`, creates a project, edits via the browser path and via the MCP path, verifies disk state

---

## Notes for Executor

The Phase 8 codebase + research produced findings that change naive readings of the design. Read these before implementing:

**1. AC7.1 — 5 platforms vs. 4 ship platforms.** Phase 1 ships 4 platforms (`darwin-arm64`, `linux-x64`, `linux-arm64`, `win32-x64`) matching the existing `@simlin/mcp` posture (verified at `src/simlin-mcp/package.json:11-15` and `mcp-release.yml:53-56`). AC7.1 lists 5 (adding `darwin-x64`, macOS Intel). Two options for V1 closeout:
   - **(a) Defer to a follow-up** (recommended): file a follow-up ticket via the `track-issue` agent to add `darwin-x64` to BOTH `@simlin/mcp` and `@simlin/serve`. macOS Intel is a shrinking installed base; building it requires `cargo-zigbuild` for `x86_64-apple-darwin` (the Phase 1 cross-build chain already uses zigbuild for non-macOS targets, so adding this is mostly a matrix entry). Document AC7.1 as "partial — 4 of 5 platforms shipped, darwin-x64 deferred" in the V1 release notes.
   - **(b) Add `darwin-x64` in Phase 8.** Add one more matrix entry to `serve-release.yml` (and ideally to `mcp-release.yml` too for parity) using `cargo-zigbuild --target x86_64-apple-darwin`. Note: this is **not** Rosetta — Rosetta is an x86-on-arm64 emulator at runtime, not a cross-compiler. The build mechanism is `cargo-zigbuild`, which natively cross-compiles `x86_64-apple-darwin` from any host.
   
   Choose option (a) unless the user explicitly asks for full 5-platform coverage in V1. Document the decision in the README.

**2. Watcher renames are paired into `RenameMode::Both` events with `paths == [from, to]`.** Confirmed via `notify-debouncer-full` 0.7+ source. The debouncer's file-ID cache compensates for macOS FSEvents not natively pairing — most renames are normalized to the paired form. Edge cases (cross-mount renames, racy filesystem operations) fall back to separate `RenameMode::From`-only and `RenameMode::To`-only events; treat those as `Removed` + `Created` respectively (degraded behavior is acceptable for V1 — a stale `LoroDoc` is dropped, and the new entry hydrates fresh).

**3. DNS rebinding is a REAL threat.** CVE-2025-66414 (December 2025) bit the official MCP TypeScript SDK's localhost servers via DNS rebinding. Phase 8 adds a `Host:` header allowlist middleware as defense-in-depth. Without it: a malicious website that resolves a controlled DNS name to `127.0.0.1` could reach our HTTP/UI and MCP servers from a victim's browser. The bearer token gates `/api/*` and the WS upgrade, so the impact is bounded — but skipping the `Host:` check is sloppy and the design plan calls for a documented threat model.

**4. Token rotation is implicit but document explicitly.** Phase 1's `main()` generates a fresh launch token in-process per invocation (never persisted). Phase 8's only deliverable for "one-time bearer token rotated on each launch" is to document this property in `docs/threat-model.md` and the README. No code change needed.

**5. New-project UX delegates to existing primitives.** The `RegistryAccess::create` from Phase 6 Task 3 already does the heavy lifting (write the file via `atomic_write`, register the entry, broadcast `ProjectChanged`). Phase 8 adds:
   - A `POST /api/projects/new` endpoint that takes `{ name: String, format: "stmx" | "sd_json", parent_dir: Option<String> }`, builds an empty `datamodel::Project` (with reasonable default `SimSpecs`), and calls the same logic as `RegistryAccess::create` (factor common code into a helper function in `writer.rs`).
   - The frontend `NewProjectButton` and `EmptyState`'s "Create new model" affordance call this endpoint and navigate to the new file on success.

**6. Smoke test as a Rust integration test, not a shell script.** Cross-platform shell scripting is painful. Use Rust + `std::process::Command` + `reqwest` (add as dev-dep) to spawn the binary, hit the HTTP API, and exercise the MCP path. Gate with `#[ignore]` so it doesn't run on every `cargo test` (slow); CI runs `cargo test --release --test smoke -- --ignored`.

**7. Disambiguation is a pure helper.** No existing codebase pattern. Implement as a pure function `disambiguatedLabels(projects: ProjectMeta[]): { project, label }[]` in `web/src/utils/disambiguate.ts` (functional core). `<ProjectList>` calls it in render. Easy to unit-test.

**8. Frontend rename handling.** When `WsMessage::ProjectRenamed { from, to }` arrives:
   - `App.tsx` updates the `projects` state: replace the entry whose `path == from` with one whose `path == to`.
   - If `state.selectedPath == from`, set `state.selectedPath = to` (the editor's path prop changes; `EditorHost.componentDidUpdate` does NOT trigger a refetch because the underlying state is unchanged — only the path identifier changed). Add a guard: when the `liveVersion` for the new path equals the current state's version, skip the refetch.

**9. The threat model document is V1-final.** Include the table from the Phase 8B research findings: cover (a) other local processes, (b) DNS rebinding, (c) cross-origin browser attacks, (d) supply-chain (postinstall script absence + npm provenance), (e) token leakage via logs (mitigation: never log it). Out-of-scope items: HTTPS (loopback-only; no benefit), persistent token (no persistence), OS user namespace (trusted user assumption), token brute-force (256-bit keyspace).

**10. Smoke-test fixtures choice.** Use `test/test-models/samples/teacup/teacup.xmile` (3.9K, the canonical fixture), `test/test-models/samples/teacup/teacup.mdl` (2.4K, smallest Vensim), and `test/sd-ai-simple.sd.json` (428 B, smallest valid SD-AI JSON). Total ~7K — fast, exercises all three formats, plus a nested subdir `subdir/nested.xmile` for AC1.2 recursion verification.

**11. README mirrors `@simlin/mcp`'s structure.** Concise (~80 lines), no badges, no troubleshooting (V1). Sections: Title + value prop, Quick Start (`npx @simlin/serve`), CLI flags table, MCP setup (Claude Code + Claude Desktop snippets — reuse Phase 6 Task 10 content), Supported file formats table, Threat model link, License.

**12. `darwin-x64` macOS Intel — known limitation.** If we go with option (a) for AC7.1 (defer 5-platform coverage), we should document in the README: "macOS Intel (`darwin-x64`) is not yet supported. The shipped `darwin-arm64` binary cannot run on Intel hardware — Rosetta only translates x86_64 binaries onto Apple Silicon, never the reverse. Intel Mac users can build from source (`cargo install --git ...`) or wait for the `darwin-x64` binary in a follow-up."

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
### Subcomponent A: File-watcher rename handling

<!-- START_TASK_1 -->
### Task 1: Add `Renamed` classification + `WsMessage::ProjectRenamed` variant

**Verifies:** none directly (foundation for AC4.2 polish — rename-while-watching)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs` (add `Renamed { from: PathBuf, to: PathBuf, format: ProjectFormat }` variant to `ClassifiedEvent`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/events.rs` (add `WsMessage::ProjectRenamed { from: String, to: String }` variant)
- Test: extend inline tests in `watcher.rs`

**Implementation:**
- Update `classify(event)` to detect `EventKind::Modify(ModifyKind::Name(RenameMode::Both))` with `paths.len() == 2`:
  ```rust
  if matches!(event.kind, EventKind::Modify(ModifyKind::Name(RenameMode::Both))) && event.paths.len() == 2 {
      let from = event.paths[0].clone();
      let to = event.paths[1].clone();
      // Both paths must classify as model files (or both not — a rename to or from a non-model extension is treated as separate Removed + Created).
      match (classify_extension(&from), classify_extension(&to)) {
          (Some(_from_fmt), Some(to_fmt)) => return ClassifiedEvent::Renamed { from, to, format: to_fmt },
          (Some(from_fmt), None) => return ClassifiedEvent::Removed { path: from, format: Some(from_fmt) },
          (None, Some(to_fmt)) => return ClassifiedEvent::ModelFile { path: to, format: to_fmt, change: ChangeKind::Created },
          (None, None) => return ClassifiedEvent::Ignored,
      }
  }
  ```
- For unpaired `RenameMode::From` → treat as `Removed`. For unpaired `RenameMode::To` → treat as `Created`. (Already handled correctly if Phase 4's classify falls through to `EventKind::Modify(_)` → `Modified`; verify this edge is covered.)
- The `WsMessage::ProjectRenamed` envelope: `{"type":"projectRenamed","from":"old/path","to":"new/path"}`.

**Testing:**
- Synthetic `DebouncedEvent` with `RenameMode::Both` and two model-file paths → `Renamed { from, to, format }`.
- Same with mixed model+non-model → `Removed` (the model side) or `Created` (the non-model side).

**Verification:**
- `cargo test -p simlin-serve watcher::` passes new tests.

**Commit:** `serve: add Renamed classification and ProjectRenamed event`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `handle_model_rename` re-keys registry, preserves Loro state

**Verifies:** server-rewrite.AC4.2 polish (rename while editing)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs` (`handle_model_rename` async fn)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (`pub fn rename_entry(&self, from: &Path, to: &Path) -> Result<(), RegistryError>` — moves the entry to the new key, preserving doc + version + last_disk_hash + last_diagnostic_keys)
- Test: extend `tests/watcher_merge.rs` with a rename test

**Implementation:**
- `rename_entry`: under the registry write lock, look up the entry at `from`. If absent → `Err(NotFound)`. If present, remove it from the map, update its `path` field to the relative form of `to`, insert at the new absolute-path key. Don't touch `version`, `doc`, `last_disk_hash`, or `last_diagnostic_keys` — they're all path-independent.
- `handle_model_rename(from, to, format)`:
  1. Verify both paths are within `state.root` (path-traversal check).
  2. `state.registry.rename_entry(&from, &to)?`. On `NotFound` (the entry was never in the registry — perhaps a non-model file just got renamed to a model extension), fall through to a fresh `handle_model_change(to, format, Created)` to discover and hydrate it.
  3. `state.events.publish(WsMessage::ProjectRenamed { from: <relative from>, to: <relative to> })`.
- The `last_disk_hash` is preserved, so if the rename was a no-op content-wise (just a path change), the next watcher event for the destination won't re-merge.

**Testing:**
- Setup: tempdir with `models/a.stmx` registered (Loro doc populated, version=2). External `mv` to `models/b.stmx`. Wait for the watcher event. Assert: registry no longer has `a.stmx`, has `b.stmx` with the same doc / version / hash. WS receives `ProjectRenamed { from, to }`.
- Sidecar handling: rename `population.mdl` to `data.mdl`. The sidecar file `population.sd.json` (if present) is also renamed by the user separately — confirm both events fire and the registry updates correctly. (This is an edge case; don't try to be clever about pairing sidecars across renames in V1.)

**Verification:**
- `cargo test -p simlin-serve --test watcher_merge` passes.

**Commit:** `serve: handle_model_rename preserves LoroDoc state and emits ProjectRenamed`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Frontend handles `ProjectRenamed`

**Verifies:** server-rewrite.AC4.2 polish (UI surface)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/App.tsx`
- Test: extend `App.test.tsx`

**Implementation:**
- In the WS-event dispatcher, on `projectRenamed`:
  ```typescript
  this.setState(prev => {
    const projects = prev.projects?.map(p =>
      p.path === renamed.from ? { ...p, path: renamed.to } : p
    ) ?? null;
    const selectedPath = prev.selectedPath === renamed.from ? renamed.to : prev.selectedPath;
    return { projects, selectedPath };
  });
  ```
- The editor's `key` prop (Phase 3 Task 11 uses `key={state.version}`) doesn't change because the version is preserved across renames — Editor stays mounted, no remount. The `path` prop changes; `EditorHost.componentDidUpdate` sees the change but since `liveVersion` is unchanged, it should NOT trigger a refetch. Add a guard: `if (prevProps.path !== this.props.path && prevProps.liveVersion === this.props.liveVersion) skipRefetch();`. (Or equivalently, only refetch when `liveVersion > state.version`.)
- Update the URL bar / browser history if the app uses one (Phase 1 didn't, so skip).

**Testing:**
- Render `<App>` with a mocked WS that emits `projectRenamed { from: "a.stmx", to: "b.stmx" }`. Assert the projects state shows `b.stmx`. If `selectedPath` was `a.stmx`, it's now `b.stmx`. Editor doesn't refetch.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: frontend handles projectRenamed without remounting editor`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-6) -->
### Subcomponent B: New-project UX

<!-- START_TASK_4 -->
### Task 4: `POST /api/projects/new` endpoint

**Verifies:** server-rewrite.AC1.4 (server-side support)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (add `pub async fn create_new_project` handler + `NewProjectRequest`/`NewProjectResponse` types)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (mount `.route("/api/projects/new", post(create_new_project))`)
- Modify: `/home/bpowers/src/simlin/src/simlin-mcp-core/src/types.rs` (add `pub fn build_empty_project() -> datamodel::Project` — defined in the LIBRARY, not in simlin-serve, so both `simlin_mcp_core::tools::create_model::create_model` and the new HTTP `POST /api/projects/new` handler call the same helper. Optional `sim_specs` override applied by callers if non-default.)
- Test: extend `tests/api_save.rs` with create-new tests

**Implementation:**
- `pub struct NewProjectRequest { pub name: String, pub format: ProjectFormat, pub parent_dir: Option<String> }`. `name` is the filename without extension (e.g., `"new-model"`); `format` determines the extension (`Stmx` → `.stmx`, `SdJson` → `.sd.json`; reject `Mdl` and `Xmile` for new files — `.stmx` is the canonical native format and `.sd.json` is the canonical AI format).
- Sanitize `name`: reject if it contains `/`, `\`, `..`, leading dot, or non-alphanumeric/underscore/hyphen characters. Length cap (e.g., 64 chars).
- Resolve the absolute path: `state.root.join(parent_dir.unwrap_or_default()).join(format!("{name}.{ext}"))`. Path-traversal check (descendant of `state.root`).
- `let mut project = simlin_mcp_core::types::build_empty_project();` — produces a minimal valid `datamodel::Project` with default sim specs (`start: 0.0, stop: 100.0, dt: 0.25, save_step: 1.0, method: Euler`) and one empty model named `"main"`. The same helper is invoked from `simlin_mcp_core::tools::create_model::create_model` (the MCP path) and from this HTTP handler, so the two paths produce byte-identical files when called with default inputs (verified by Phase 8 Task 6's parity test).
- Reuse `RegistryAccess`-style logic OR a shared helper:
  - Construct the file content (XMILE via `to_xmile` or pretty JSON for `SdJson`).
  - `simlin_engine::io::atomic_write(&abs_path, &bytes)`.
  - Add a fresh `ProjectMeta` to the registry.
  - Compute initial `last_disk_hash` and `last_diagnostic_keys`.
  - `state.events.publish(WsMessage::ProjectChanged { path: <relative>, version: 0, source: ChangeSource::User })`.
- Response: `{ path: <relative path>, version: 0 }`.
- Error cases: `Err(AlreadyExists)` → 409; invalid name → 400; path-traversal attempt → 403.

**Testing:**
- Happy path: POST with `{name: "new", format: "stmx"}` against an empty tempdir. Expect 200 + path `"new.stmx"`. File exists on disk; registry has the entry.
- Conflict: POST same name twice → second returns 409.
- Sanitization: POST with `name: "../etc/passwd"` → 400.
- SdJson format: POST with `format: "sd_json"` → file `new.sd.json` created with valid native JSON.

**Verification:**
- `cargo test -p simlin-serve --test api_save` passes new tests.

**Commit:** `serve: POST /api/projects/new creates an empty model file`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Frontend `<NewProjectButton>` + EmptyState affordance

**Verifies:** server-rewrite.AC1.4 (UI surface)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/NewProjectButton.tsx`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EmptyState.tsx` (Phase 1 Task 12 placeholder; render the button)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.tsx` (render the button at the top of the list)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/api.ts` (add `createProject(name, format, parentDir?)`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/NewProjectButton.test.tsx`

**Implementation:**
- `<NewProjectButton onCreated={(path) => void}>`: a class component that renders a "+ New model" button. Clicking expands an inline form with two fields: `<input>` for the filename (no extension), `<select>` with options "XMILE (.stmx)" / "Simlin JSON (.sd.json)" (default `.stmx`). A "Create" button calls `api.createProject(...)`, then `props.onCreated(response.path)` to navigate; an "X" button cancels.
- `<EmptyState>` renders centered text "No model files in this directory" + `<NewProjectButton onCreated={path => onSelect(path)}>`.
- `<ProjectList>` renders the button as the first item (always visible, not just in empty state — per AC1.4 wording "from any state").
- `App.tsx` provides the `onCreated` handler: refetch `/api/projects` (or rely on the WS `projectChanged` event to update the list, which is cleaner — just set `selectedPath = newPath`).

**Testing:**
- Render `<NewProjectButton>`, click it → form appears. Type `"foo"`, leave format default. Click create. Mock `fetch` returns success with `path: "foo.stmx"`. Assert `onCreated("foo.stmx")` was called.
- Validation: empty name disables the create button. `name="../etc"` shows a client-side error (in addition to the server's 400).

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: NewProjectButton and EmptyState/ProjectList affordances`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: `create_model` MCP tool symmetry

**Verifies:** symmetry between HTTP and MCP create paths

**Files:**
- Confirm: Phase 6 Task 3 (`RegistryAccess::create`) + Phase 5 Task 4 (`create_model` library fn) + Phase 6 Task 4 (`create_model` rmcp tool method) already give MCP clients the equivalent capability.
- This task is verification-only: write a test exercising both HTTP `POST /api/projects/new` and MCP `create_model` against the same tempdir and assert they produce the same file shape.

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/parity_create.rs`

**Implementation:**
- Test sets up a tempdir, calls `POST /api/projects/new {name: "h", format: "stmx"}`, then calls MCP `create_model {project_path: "<abs>/m.stmx"}`. Assert both files exist, both parse to a `datamodel::Project` with one empty model named `"main"`, and the bytes are byte-identical (verifies `build_empty_project` is canonically used in both code paths).

**Verification:**
- `cargo test -p simlin-serve --test parity_create` passes.

**Commit:** `serve: parity test for HTTP create and MCP create_model`
<!-- END_TASK_6 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (task 7) -->
### Subcomponent C: Disambiguation

<!-- START_TASK_7 -->
### Task 7: `disambiguatedLabels` helper + `<ProjectList>` integration

**Verifies:** the design's "multiple files with the same basename ... disambiguated" requirement

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/utils/disambiguate.ts`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.tsx`
- Test: `/home/bpowers/src/simlin/src/simlin-serve/web/src/utils/disambiguate.test.ts`

**Implementation:**
- Pure function:
  ```typescript
  export function disambiguatedLabels<T extends { path: string }>(
    items: T[]
  ): { item: T; label: string }[] {
    const counts = new Map<string, number>();
    for (const it of items) {
      const base = basename(it.path);
      counts.set(base, (counts.get(base) ?? 0) + 1);
    }
    return items.map(it => {
      const base = basename(it.path);
      const ambiguous = (counts.get(base) ?? 0) > 1;
      return { item: it, label: ambiguous ? it.path : base };
    });
  }
  
  function basename(path: string): string {
    const idx = path.lastIndexOf('/');
    return idx === -1 ? path : path.slice(idx + 1);
  }
  ```
- `<ProjectList>` calls `disambiguatedLabels(this.props.projects)` and renders each entry's `label`.
- Visual hint: when the label is a full path (i.e., contains `/`), render the directory portion (`label.slice(0, label.lastIndexOf('/') + 1)`) in a lighter color (`opacity: 0.65`) and the basename in normal weight. CSS-only.

**Testing:**
- Unit tests: `[{path: "a/x.stmx"}, {path: "b/x.stmx"}, {path: "y.xmile"}]` → labels `["a/x.stmx", "b/x.stmx", "y.xmile"]`.
- Three-way collision: `[{path: "a/x"}, {path: "b/x"}, {path: "c/x"}]` → all three render full paths.
- Empty input: `[]` → `[]`.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: disambiguatedLabels helper handles basename collisions`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (tasks 8-9) -->
### Subcomponent D: Threat-model + Host/Origin validator

<!-- START_TASK_8 -->
### Task 8: `host_origin_validator` middleware (DNS rebinding defense)

**Verifies:** none directly (security defense-in-depth — closes the design's "documented threat model" deliverable)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/middleware.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (mount the middleware on both `build_router` and `build_mcp_router`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (the `updates_ws_handler` from Phase 3 also validates `Origin:` since the middleware is HTTP-only — WS upgrades use the request's `Origin:` header)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/tests/middleware_host.rs`

**Implementation:**
- `pub async fn host_validator_middleware<B>(State(state): State<AppState>, request: Request<B>, next: Next<B>) -> Response`:
  - Extract `Host:` header.
  - Compute the allowed values from `state.ui_port` and `state.mcp_port` (both stored on `AppState`): `["127.0.0.1:<ui_port>", "localhost:<ui_port>", "127.0.0.1:<mcp_port>", "localhost:<mcp_port>"]`.
  - If header missing or not in the set → `(StatusCode::MISDIRECTED_REQUEST, "Host header not allowed").into_response()`.
  - Otherwise → `next.run(request).await`.
- `Origin:` validator inside `updates_ws_handler` (Phase 3 Task 7):
  - Extract `Origin:` header from request. If present, must be in `["http://127.0.0.1:<ui_port>", "http://localhost:<ui_port>"]`. (Empty `Origin` is allowed for non-browser clients like `wscat` — log a tracing::info but don't reject.)
  - Add a CLI flag `--strict-origin` (default `true`) that controls whether empty `Origin` is rejected. For dev convenience, allow disabling.
- Add `ui_port: u16` and `mcp_port: u16` fields to `AppState` so the validator can compute the allowlist at request time. (`main()` populates these after binding.)

**Testing:**
- HTTP test: GET `/api/projects` with `Host: 127.0.0.1:<port>` → 200. With `Host: evil.example.com:<port>` → 421. With no `Host:` header (HTTP/1.0 omits it; `tower::ServiceExt::oneshot` allows building requests without it) → 421.
- WS test: connect with `Origin: http://127.0.0.1:<port>` → 101 Switching Protocols. With `Origin: https://evil.example.com` → 403/421. With no `Origin` (wscat-like) → succeeds (log present in tracing-test capture).

**Verification:**
- `cargo test -p simlin-serve --test middleware_host` passes.

**Commit:** `serve: Host/Origin validator middleware (DNS rebinding defense)`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: `docs/threat-model.md` documentation

**Verifies:** design plan's "documented threat model" deliverable

**Files:**
- Create: `/home/bpowers/src/simlin/docs/threat-model.md`
- Modify: `/home/bpowers/src/simlin/docs/README.md` (add a link to the new file per `docs/CLAUDE.md`'s instruction to keep the index current)

**Implementation:**
- Document V1's threat model. Cover:
  - **Trust boundary:** the OS user account. Any process running as the user can read/write files directly; the MCP server is not a privilege boundary.
  - **Loopback as primary boundary:** `127.0.0.1` bind prevents network-attached attackers; the server is reachable only by processes on the local machine.
  - **Bearer token (defense-in-depth):** 32-byte (256-bit) URL-safe random token, generated at startup, embedded in the launched URL, stored by the SPA in `sessionStorage`. Required for `/api/*` and the WS upgrade. Each `npx @simlin/serve` invocation rotates the token; killing and restarting the server invalidates all prior URLs.
  - **DNS rebinding defense:** `Host:` header allowlist (only `127.0.0.1:<port>` / `localhost:<port>` accepted) prevents a malicious website from reaching the server via a controlled DNS name. References CVE-2025-66414 (the official MCP TypeScript SDK was bitten by this).
  - **Cross-origin defense:** `Origin:` header allowlist on WS upgrades.
  - **Supply chain:** no `postinstall` scripts in the npm shim. Per-platform binaries are published with `npm publish --provenance` (OIDC). Trust boundary is npm + GitHub Actions OIDC.
- Out-of-scope (documented):
  - HTTPS: loopback only; no benefit.
  - Persistent token: not stored anywhere outside the launched URL + sessionStorage.
  - Token brute-force: 256-bit keyspace.
  - OS user namespace: trusted by assumption.
  - File-watcher events: untrusted disk content cannot smuggle code execution; the validator gate on the merge path rejects malformed projects.

**Testing:** None (documentation).

**Verification:** Manual review.

**Commit:** `serve: docs/threat-model.md documents V1 security posture`
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_D -->

<!-- START_SUBCOMPONENT_E (task 10) -->
### Subcomponent E: README

<!-- START_TASK_10 -->
### Task 10: `src/simlin-serve/README.md` — user-facing documentation

**Verifies:** design plan's "README and `npx @simlin/serve` install/use docs" deliverable

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/README.md` (Phase 1 Task 1 created a stub; Phase 8 expands it)

**Implementation:**
- Sections (mirror `@simlin/mcp`'s structure, adapted):
  1. **Title + value prop:** "Simlin Serve — view, edit, and AI-collaborate on system dynamics models from any directory." Link to simlin.com.
  2. **Quick Start:**
     ```bash
     # In any directory containing .stmx, .xmile, .mdl, or .sd.json files:
     npx @simlin/serve@latest
     ```
     Expected output:
     ```
     Simlin Serve
       UI:  http://127.0.0.1:54321/?token=<random>
       MCP: http://127.0.0.1:7878/mcp
     ```
     "Your default browser should open the UI automatically. The MCP URL is stable across launches; configure your AI client once."
  3. **CLI flags table:**
     | Flag | Default | Purpose |
     | `[ROOT]` | `$PWD` | Directory to serve |
     | `--port <N>` | `0` (ephemeral) | UI HTTP port |
     | `--mcp-port <N>` | `7878` | MCP HTTP port |
     | `--no-open` | false | Suppress browser launch |
     | `--strict-origin` | true | Reject WS upgrades with non-allowlisted `Origin` |
  4. **MCP setup:** the Claude Code CLI command from Phase 6 Task 10 + the Claude Desktop `mcp-remote` snippet.
  5. **Supported file formats** (mirror `@simlin/mcp`'s table exactly):
     | Format | Extensions | Read | Edit |
     | XMILE | .stmx, .xmile, .xml | yes | yes (in-place) |
     | Simlin JSON | .sd.json | yes | yes (in-place) |
     | Vensim MDL | .mdl | yes (via xmutil) | yes (writes .sd.json sidecar; .mdl untouched) |
  6. **MCP tool surface:**
     | Tool | Description |
     | `list_projects` | Lists discovered models with format and git status |
     | `read_model` | Reads a model and returns its JSON snapshot with loop dominance |
     | `edit_model` | Applies edits to a model |
     | `simulate` | Runs a simulation (with optional parameter overrides) |
     | `create_model` | Creates a new empty model |
  7. **Notifications:** brief summary; link to phase_07.md or to a notifications-section in this README. Document the ordering caveat ("notifications are advisory; treat them as refetch hints").
  8. **Threat model link:** link to `/docs/threat-model.md`.
  9. **Limitations (V1):**
     - Vensim `.mdl` write goes to a `.sd.json` sidecar. True `.mdl` round-trip is future work.
     - macOS Intel (`darwin-x64`) binaries are not yet published. Apple Silicon (`darwin-arm64`), Linux (x64 and arm64), and Windows x64 are all shipped.
     - Claude Desktop requires the `mcp-remote` npm proxy.
  10. **License:** Apache-2.0.

**Testing:** None (documentation).

**Verification:** Manual review for accuracy. Render in GitHub's markdown preview to spot formatting issues.

**Commit:** `serve: README provides quick start, MCP setup, and limitations`
<!-- END_TASK_10 -->
<!-- END_SUBCOMPONENT_E -->

<!-- START_SUBCOMPONENT_F (tasks 11-12) -->
### Subcomponent F: Cross-platform smoke test

<!-- START_TASK_11 -->
### Task 11: Rust integration smoke test (`tests/smoke.rs`)

**Verifies:** server-rewrite.AC1, AC2, AC3, AC5, AC7 (composition end-to-end on the local platform)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/smoke.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/Cargo.toml` (add `reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }` as a dev-dependency)

**Implementation:**
- Mark the test `#[ignore]` so it doesn't run in `cargo test` by default (it spawns the binary which is slow); `cargo test --release --test smoke -- --ignored` runs it.
- Test setup:
  1. Create a tempdir with `teacup.xmile` (copied from fixtures), `teacup.mdl` (copied), `subdir/nested.xmile` (copied as `teacup.xmile`), and `small.sd.json` (copied from `test/sd-ai-simple.sd.json`).
  2. `Command::new(env!("CARGO_BIN_EXE_simlin-serve")).args(&["--no-open", "--port", "0", "--mcp-port", "0", tempdir.path().to_str().unwrap()]).stdout(Stdio::piped()).spawn()`.
  3. Read stdout line-by-line until two URLs are printed; parse out the UI port and MCP port. (Use `BufRead::lines`.)
  4. Parse the launch token from the UI URL.
- Tests against the running binary:
  - GET `http://127.0.0.1:<ui_port>/healthz` → 200, body `"ok"`.
  - GET `http://127.0.0.1:<ui_port>/api/projects` (with `Authorization: Bearer <token>`) → 4 entries.
  - GET `http://127.0.0.1:<ui_port>/api/projects/teacup.xmile` → 200, JSON with the model.
  - POST `http://127.0.0.1:<ui_port>/api/projects/teacup.xmile` with a small mutation → 200, version 1.
  - GET again → returns the mutated content.
  - Verify on disk: `tempdir.path().join("teacup.xmile")` now reflects the mutation (parse via `simlin_engine::open_xmile` and check the variable).
  - MCP path: POST a JSON-RPC `tools/call` for `read_model` to `http://127.0.0.1:<mcp_port>/mcp`. Expect a successful response.
  - MCP `create_model` for a new file `mcp_created.stmx`, then verify it's on disk.
- Teardown: send SIGTERM (Unix) / `taskkill /T /F` (Windows) to the child. Wait up to 5s for exit. If it doesn't exit, fail the test.

**Testing meta:**
- Run locally: `cargo test --release --test smoke -- --ignored` should pass.
- Default `cargo test` (without `--ignored`) does NOT run the smoke test; verify by running `cargo test -p simlin-serve` and confirming the smoke test is reported as `ignored`.

**Verification:**
- Local: `cargo test --release --test smoke -- --ignored` passes.
- The test is robust to ephemeral-port allocation (no hardcoded ports).

**Commit:** `serve: tests/smoke.rs end-to-end binary integration test`
<!-- END_TASK_11 -->

<!-- START_TASK_12 -->
### Task 12: GitHub Actions matrix smoke job

**Verifies:** server-rewrite.AC7.1 (CI-verified on 3 platforms)

**Files:**
- Modify: `/home/bpowers/src/simlin/.github/workflows/serve-release.yml` (Phase 1 Task 21 created this; add a new `smoke` job that runs after `build`)
- ALTERNATIVELY: Modify `/home/bpowers/src/simlin/.github/workflows/ci.yaml` (add a new job; runs on every PR, not just on `serve-v*` tags)

**Implementation:**
- Recommend adding to `ci.yaml` so the smoke test runs on every PR — catches regressions earlier.
- New job `smoke-test`:
  ```yaml
  smoke-test:
    name: simlin-serve smoke (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    timeout-minutes: 10
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          toolchain: 1.95.0
      - uses: pnpm/action-setup@v3
        with:
          version: 9
      - name: Build frontend
        run: |
          cd src/simlin-serve/web
          pnpm install --frozen-lockfile
          pnpm build
      - name: Build binary
        env:
          SIMLIN_SERVE_BUILD_WEB: "1"
        run: cargo build -p simlin-serve --release
      - name: Run smoke test
        run: cargo test -p simlin-serve --release --test smoke -- --ignored --nocapture
  ```
- Document the test as gating: a PR cannot merge if the smoke test fails on any of the three platforms.

**Verification:**
- Push the change to a feature branch. CI runs the smoke job on all three platforms. Confirm passing status.

**Commit:** `serve: CI smoke-test job runs on macOS arm64, Linux x64, Windows x64`
<!-- END_TASK_12 -->
<!-- END_SUBCOMPONENT_F -->

---

## Phase Verification Checklist

Before marking Phase 8 complete:

1. `cargo test --workspace` (no regressions; new tests pass).
2. `cargo test -p simlin-serve --release --test smoke -- --ignored` passes locally.
3. CI smoke job passes on all three platforms (verify by pushing to a branch).
4. `cd src/simlin-serve/web && pnpm test && pnpm lint` passes.
5. `cargo clippy --workspace -- -D warnings` clean.
6. `cargo fmt --workspace --check` clean.
7. **Manual end-to-end:**
   - Start the server in an empty tempdir → empty state shows the "+ New model" button.
   - Click "+ New model", create `first.stmx` → editor opens with the empty model.
   - Add a stock, save → file written.
   - Externally `mv first.stmx second.stmx` → sidebar updates the entry to `second.stmx` without losing editor state.
   - Create a second `first.stmx` in `subdir/` → both entries show as `first.stmx` and `subdir/first.stmx` (disambiguated).
8. **Manual security:** with the server running, try `curl -H "Host: evil.example.com:<port>" http://127.0.0.1:<port>/api/projects` → 421 Misdirected Request.
9. **Manual cross-client:** Claude Code CLI configured per README → `list_projects` returns the same entries the browser shows; `simulate` returns time series.
10. **AC sweep:** verify every `server-rewrite.AC*` line in the design plan against the implementation (check off each one with a brief verification note).
11. **README review:** new user with no prior knowledge can install + use the server following only the README. (Have someone unfamiliar try it; budget 15 min.)

If all 11 verifications pass, Phase 8 is done — and V1 is ready to ship.
