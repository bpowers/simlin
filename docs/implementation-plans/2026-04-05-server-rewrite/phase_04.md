# Phase 4: File Watcher and On-Disk Diff/Merge Implementation Plan

**Goal:** Add a recursive filesystem watcher rooted at the project root. When an external editor modifies a model file, the watcher feeds the new disk content through the **same** `apply_canonical_json` primitive used by browser saves (introduced in Phase 3). Browser-side in-flight edits are preserved because the primitive merges rather than replaces. Byte-identical disk writes are short-circuited via a content-hash cache so the watcher does not echo our own atomic writes back into the system.

**Architecture:** A long-lived `WatcherActor` owns a `notify-debouncer-full` watcher rooted at `$PWD` with a 100ms debounce window. The debouncer's `tokio::sync::mpsc::UnboundedSender` (via the `tokio` feature) feeds debounced batches into the actor's `tokio::select!` loop. For each batch, the actor classifies events into three buckets: model-file mutations (`.stmx`/`.xmile`/`.mdl`/`.sd.json`), `.git/HEAD` and `.git/index` mutations (git-status invalidation), and everything else (ignored). Model-file mutations are short-circuited if the new file's XXH3-64 hash matches the cached hash of what we last wrote (the no-op case for our own atomic writes). Otherwise the actor parses the file, validates via simlin-engine's diagnostic check (using the current Loro tip as the baseline so disk edits that fix existing errors are accepted), then calls `apply_canonical_json(&doc, &new_json)` under the registry's per-entry write lock — exactly the same call browser saves make. After the merge, the registry's version increments, the new content is broadcast on the existing `EventBus` with `source: Disk`, and the registry's `mtime`/`size`/`hash` cache is refreshed.

**Tech Stack:** New: `notify-debouncer-full = "0.7"` with `features = ["tokio"]`, `twox-hash = "2"` with `xxhash3_64` feature. All other primitives from Phases 1-3.

**Scope:** Phase 4 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC4: Concurrent editing via Loro
- **server-rewrite.AC4.2 Success:** Editing a model file in an external text editor (e.g. vim) while the browser has the model open causes the browser to update live with the disk-side changes (no reload prompt)
- **server-rewrite.AC4.3 Success:** Browser-side in-flight edits are preserved across an external disk edit (the merge layer combines both)
- **server-rewrite.AC4.4 Edge:** A disk edit that is byte-identical to the current Loro tip is a no-op (no broadcast, no churn)

### server-rewrite.AC2 (closeout): Git status reporting — file-watcher invalidation
- **server-rewrite.AC2.4 Edge:** Git status is recomputed when the file watcher fires for the file or for `.git/HEAD`/`.git/index` *(now fully covered)*

### server-rewrite.AC6 (continued partial): MCP push notifications — disk source
- **server-rewrite.AC6.3 Success:** Any change (browser, MCP, disk) emits a `projectChanged` notification with a `source` discriminator (`"user" | "agent" | "disk"`) *(`"disk"` source added; `"agent"` still pending Phase 6/7)*

---

## Notes for Executor

The Phase 4 research produced several findings that change naive readings of the design. Read these before implementing:

**1. Use `notify-debouncer-full`, not bare `notify`.** `notify-debouncer-full = "0.7"` with `features = ["tokio"]` gives us a `tokio::sync::mpsc::UnboundedSender` blanket impl for `DebounceEventHandler`. The debouncer runs its ticker on a dedicated OS thread (uses `std::thread::spawn` internally) and feeds batched, rename-coalesced `Vec<DebouncedEvent>` straight into the tokio runtime via non-blocking `send`. Do **not** use `tokio::task::spawn_blocking` to wrap the watcher — the debouncer is already on its own thread.

**2. Watch the directory, not files.** On all three platforms, `simlin_engine::io::atomic_write` (`<file>.new` + `rename` over the target) replaces the inode. A file-level watch dies after the rename. Watch the `$PWD` (via `RecursiveMode::Recursive`) so the watch is anchored on the parent and survives renames.

**3. macOS FSEvents does not emit paired rename events.** Filter by **path**, not by `EventKind`. The debouncer's file-ID cache normalizes the final path; ingest based on which paths changed.

**4. Echo suppression — XXH3-64 of last-written bytes.** When *we* atomic-write a file (browser save in Phase 2/3), we'll see a watcher event for that same file moments later. Skip ingestion when the new file's content hash equals the cached hash from our last write. Use `twox-hash = "2"` with `features = ["xxhash3_64", "std"]` (`default-features = false` to drop the random/xxhash32 surface). One-shot: `XxHash3_64::oneshot(0, &bytes)`.

**5. Reuse Phase 1's discovery exclusions for incoming watch events.** The watcher subscribes to the entire root tree; we filter incoming events through the same `discovery::is_excluded_dir` predicate so that events under `node_modules`, `target`, etc., are dropped before any processing. Add the helper to `discovery.rs` if it isn't already exposed; otherwise reuse it directly.

**6. Reuse Phase 3's `apply_canonical_json` primitive.** This phase does not introduce any new merge logic. The watcher's path is identical to the save handler's path, except (a) the source is `Disk`, (b) the version increment happens because of the disk change rather than a POST, and (c) there is no version check from the client (the watcher is authoritative for "what happened on disk").

**7. Validation gate matters here too.** A user can `vim` a file into an invalid state (e.g., introducing a syntax error). We do **not** want that to clobber the in-memory Loro doc with garbage. Validation is performed exactly the same way: parse, run diagnostics, compare against baseline (the current Loro tip's diagnostics). On validation failure, the watcher logs a warning but does **not** apply the merge — the in-memory state stays with the last-known-good content. The browser shows that last-known-good state, the user fixes the file in vim, and a subsequent valid disk write goes through. Document this so users understand the behavior.

**8. `.git/HEAD` and `.git/index` watching.** The watcher already covers these because the recursive watch includes `.git/`. The discovery-exclusion filter must be **bypassed** for these specific paths (we want events for `.git/HEAD` and `.git/index` even though `discovery` excludes the rest of `.git/`). Add a special-case branch in the event filter: if the path matches `*/.git/HEAD` or `*/.git/index`, accept it; otherwise drop `.git`-internal paths.

**9. Sidecar handling on disk changes.** If a user edits a `.mdl` file directly:
   - The watcher sees the `.mdl` change.
   - Phase 1's read logic prefers a sibling `.sd.json` if present.
   - **Decision:** if a sidecar exists, the `.mdl` change is **ignored** (sidecar is canonical). Document this. If no sidecar exists yet, re-parse the `.mdl` and merge as usual.
If the user edits the `.sd.json` sidecar directly: standard merge path.
If the user creates a new model file (e.g., copies an `.stmx` into the directory): the watcher sees the create event, runs discovery on it, adds a new `ProjectMeta` entry, hydrates a fresh `LoroDoc`, broadcasts `ProjectChanged` (so any browser tabs show the new entry in the sidebar). This is a Phase 4 deliverable, not Phase 8.
If the user deletes a model file: the watcher sees the remove event, drops the registry entry, broadcasts a new event variant `ProjectRemoved` (added to `WsMessage` in this phase).

**10. `WsMessage` gains `ProjectRemoved` variant.** Add `WsMessage::ProjectRemoved { path: String }`. The frontend already handles `ProjectChanged`; in Phase 4 the frontend also handles `ProjectRemoved` by removing the entry from the sidebar (and showing a "this model was deleted on disk" state if it's the currently-viewed entry, per the Phase 8 spec — but Phase 4 lays the wiring; the polish UX comes in Phase 8).

**11. `tokio::select!` shape inside the actor.** Two arms: (a) `Some(batch) = rx.recv()` for watcher batches, (b) `_ = shutdown.notified()` for graceful shutdown when the binary exits. Shutdown signal is `Arc<tokio::sync::Notify>` — std-tokio surface, no extra dep. `main()` calls `shutdown.notify_waiters()` on Ctrl-C.

**12. Don't introduce thrashing.** When the same project file changes 10 times in a 100ms window (e.g., an editor that does many small writes), the debouncer coalesces these into one event. We process one merge per debounced event per file. Keep this property — do not add additional per-event work that would defeat the debounce.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-4) -->
### Subcomponent A: Watcher infrastructure

<!-- START_TASK_1 -->
### Task 1: Add `notify-debouncer-full` and `twox-hash` deps; define `ContentHash` cache field

**Verifies:** none directly (scaffolding for AC4.4)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/Cargo.toml` (add `notify-debouncer-full = { version = "0.7", features = ["tokio"] }`, `twox-hash = { version = "2", default-features = false, features = ["xxhash3_64", "std"] }`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add `pub last_disk_hash: u64` field to `ProjectMeta`; default to `0`)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/hashing.rs` (`pub fn content_hash(bytes: &[u8]) -> u64` thin wrapper over `XxHash3_64::oneshot(0, bytes)`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (re-export `pub mod hashing;`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/writer.rs` (after a successful `atomic_write`, compute `content_hash(&bytes)` and store it on the `ProjectMeta` via a new `RegistryRegistry::refresh_after_write(path, mtime, size, hash)` helper)

**Implementation:**
- Two-line `hashing.rs`: import + `pub fn content_hash(bytes: &[u8]) -> u64 { twox_hash::XxHash3_64::oneshot(0, bytes) }`. Document why XXH3-64: fast, non-cryptographic (no need for crypto here), pure Rust, 5M+ downloads/month.
- `ProjectMeta.last_disk_hash` is the hash of bytes most recently written by the server (Phase 2 save path). The watcher uses this as the echo-suppression key.
- `refresh_after_write` is called from `save_to_disk`'s success path (replacing the existing `refresh_meta`).

**Testing:**
- Inline test: hash of `b"hello"` is stable across runs (assert against a captured constant).
- Hashing two different byte slices produces different values.

**Verification:**
- `cargo test -p simlin-serve hashing::` passes.
- `cargo build -p simlin-serve` succeeds with the new deps.

**Commit:** `serve: add notify-debouncer-full + twox-hash deps with content_hash helper`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `WatcherActor` skeleton — debouncer, channel, tokio::select! loop

**Verifies:** none directly (plumbing for AC4.2)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs`
- Modify: `lib.rs` (re-export `pub mod watcher;`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs` (after `AppState` is built, spawn the `WatcherActor` and store its shutdown handle so we can stop it gracefully on Ctrl-C)

**Implementation:**
- `pub struct WatcherActor { state: AppState, debouncer: Debouncer<RecommendedWatcher, RecommendedCache>, rx: UnboundedReceiver<DebounceEventResult>, shutdown: ShutdownSignal }` (field types per `notify-debouncer-full`'s public API).
- `pub fn spawn(state: AppState, shutdown: ShutdownSignal) -> Result<JoinHandle<()>, WatcherError>`:
  1. Create the unbounded mpsc channel.
  2. Build the debouncer: `let mut debouncer = new_debouncer(Duration::from_millis(100), None, tx)?;`. Watch `state.root` recursively: `debouncer.watcher().watch(&state.root, RecursiveMode::Recursive)?;`.
  3. Spawn an async task running `actor.run().await`.
  4. Return the join handle.
- `async fn run(self)`:
  ```rust
  loop {
      tokio::select! {
          Some(result) = self.rx.recv() => self.handle_batch(result).await,
          _ = self.shutdown.notified() => break,
      }
  }
  ```
- `handle_batch(result: DebounceEventResult)` is a stub that just `tracing::info!`s the batch shape for now. Tasks 3-7 build out the real handling.
- `ShutdownSignal = Arc<tokio::sync::Notify>`. Use `tokio::sync::Notify` (already in tokio's stdlib-shaped surface; no extra dep). The actor's loop calls `shutdown.notified().await` in the `select!` arm; `main()` calls `shutdown.notify_waiters()` on Ctrl-C.

**Testing:**
- Integration test (`tests/watcher_smoke.rs`): spawn the actor against a tempdir, write a `.stmx` file, wait for a debounced event, assert at least one event arrived. (Pure smoke test; no merging yet — Tasks 3-7 add the real ingestion.)

**Verification:**
- `cargo test -p simlin-serve --test watcher_smoke` passes.
- Manual: `cargo run -p simlin-serve -- /tmp/some-dir` and `touch /tmp/some-dir/foo.stmx` produces a tracing line within 100ms.

**Commit:** `serve: WatcherActor with notify-debouncer-full + tokio mpsc bridge`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Event classification — model files, git internals, ignored

**Verifies:** server-rewrite.AC2.4 (event detection), AC4.2 (event detection)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/discovery.rs` (expose `pub fn is_excluded_dir(name: &str) -> bool` and `pub fn classify_extension(path: &Path) -> Option<ProjectFormat>` so the watcher reuses Phase 1's logic)
- Test: inline tests in `watcher.rs`

**Implementation:**
- `enum ClassifiedEvent { ModelFile { path: PathBuf, format: ProjectFormat, change: ChangeKind }, GitInternal { repo_root: PathBuf }, Removed { path: PathBuf, format: Option<ProjectFormat> }, Ignored }` where `ChangeKind` is `Created | Modified` (renames are coalesced by the debouncer to a single final-path event).
- `fn classify(event: &DebouncedEvent) -> ClassifiedEvent`:
  - For each path in `event.paths` (the debouncer may include multiple for a rename-pair):
    1. If the path's components contain any `is_excluded_dir(component_name)` → check if it's a `.git/HEAD` or `.git/index` → if yes, `GitInternal { repo_root: <walk up to git root> }`; otherwise `Ignored`.
    2. If `classify_extension(&path)` returns `Some(format)` → check `event.kind`:
       - `EventKind::Create(_)` or `EventKind::Modify(_)` → `ModelFile { path, format, change: Created/Modified }`.
       - `EventKind::Remove(_)` → `Removed { path, format: Some(format) }`.
       - Other → `Ignored`.
    3. Otherwise → `Ignored`.
  - On macOS, `EventKind::Modify(Name(Any))` for renames may appear without a content-modify event — treat as `Modified` (the file's content is what gets re-read on the next read).
- `is_excluded_dir` already lives in `discovery.rs` from Phase 1; export it. `classify_extension` is the same dispatch logic from Phase 1's `discover_models` — extract to a `pub fn`.

**Testing:**
- Unit: feed synthetic `DebouncedEvent`s with various paths and kinds; assert classification.
  - `node_modules/foo.stmx` Modify → `Ignored` (excluded dir).
  - `models/x.stmx` Modify → `ModelFile { path, format: Stmx, change: Modified }`.
  - `models/x.stmx` Create → `ModelFile { ..., change: Created }`.
  - `models/x.stmx` Remove → `Removed { ..., format: Some(Stmx) }`.
  - `repo/.git/HEAD` Modify → `GitInternal { repo_root: "repo" }`.
  - `repo/.git/objects/abc` Modify → `Ignored`.

**Verification:**
- `cargo test -p simlin-serve watcher::` passes new tests.

**Commit:** `serve: classify watcher events into model/git/removed/ignored`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: `handle_batch` dispatch (still no merge yet)

**Verifies:** none directly (composition for AC4.2)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs`

**Implementation:**
- `async fn handle_batch(&self, result: DebounceEventResult)`:
  - On `Err(errors)`: log each error via `tracing::warn!`; do not propagate.
  - On `Ok(events)`: for each event, classify, then dispatch:
    - `ModelFile` → call `self.handle_model_change(...)` (Task 5)
    - `Removed` → call `self.handle_model_removal(...)` (Task 6)
    - `GitInternal` → call `self.handle_git_change(...)` (Task 7)
    - `Ignored` → no-op
- For now, all of these are `tracing::debug!` stubs.

**Verification:**
- `cargo test -p simlin-serve --test watcher_smoke` still passes (the dispatch is wired but does nothing observable yet).

**Commit:** `serve: WatcherActor handle_batch dispatch by event class`
<!-- END_TASK_4 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 5-8) -->
### Subcomponent B: Disk → Loro merge with echo suppression

<!-- START_TASK_5 -->
### Task 5: `handle_model_change` — read, hash-compare, parse, validate, merge

**Verifies:** server-rewrite.AC4.2, server-rewrite.AC4.3, server-rewrite.AC4.4

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add a public `pub fn merge_disk_change(&self, abs_path: &Path, new_json: &Value) -> Result<u64, RegistryError>` that holds the write lock for the version increment + Loro merge — analogous to `check_increment_and_merge` from Phase 3 but **without** the version comparison since the watcher is authoritative)
- Test: extend `tests/watcher_smoke.rs` and add `tests/watcher_merge.rs`

**Implementation:**
- `async fn handle_model_change(&self, path: PathBuf, format: ProjectFormat, change: ChangeKind)`:
  1. **Sidecar override:** if `format == Mdl`, check for a sibling `.sd.json`. If present, ignore this `.mdl` event entirely (the sidecar is the source of truth; an event on the `.mdl` is stale).
  2. Read the file: `let bytes = tokio::fs::read(&path).await?;` (use the async fs API since we're in an async task). On error (file vanished between event + read) → log + return.
  3. **Echo suppression:** compute `let new_hash = hashing::content_hash(&bytes);`. Look up the current `ProjectMeta` for this path; if `meta.last_disk_hash == new_hash`, return — this is our own echo. Document with a comment.
  4. Parse: dispatch by format (reuse `parse::parse_to_datamodel` from Phase 1). On parse error → `tracing::warn!("watcher: parse failed for {path}: {err}; ignoring change")` and return (preserve last-known-good in-memory state).
  5. Validate: `let baseline = registry.get_or_init_doc(&abs_path)?.export_canonical_json()? → datamodel::Project → compute_baseline(...)`. Then `validation::validate_save(&new_json_string, &baseline)`. On new errors → `tracing::warn!("watcher: validation failed for {path}: {errors:?}; ignoring change")` and return (same preserve-LKG strategy).
  6. **Merge:** `let new_version = registry.merge_disk_change(&abs_path, &new_json_value)?;`. The registry's write-locked critical section calls `apply_canonical_json` against the per-project doc.
  7. **Refresh meta + hash:** under a brief write lock, update `mtime`, `size`, and `last_disk_hash` to the new values.
  8. **Broadcast:** `state.events.publish(WsMessage::ProjectChanged { path: <relative path>, version: new_version, source: ChangeSource::Disk });`.
- `change == Created`: if the path is not in the registry yet, run `discovery::classify_extension` (already done) and add a fresh `ProjectMeta` before doing the read+merge. The `merge` populates the freshly-created `ProjectDoc`.

**Testing:**
Verifies AC4.2, AC4.3, AC4.4. Tests in `tests/watcher_merge.rs`:

- **AC4.2:** spawn server + watcher against a tempdir with one `.xmile` fixture. Connect a WS client. Externally rewrite the file (via `tokio::fs::write` from the test) with a mutated version. Expect to receive a `ProjectChanged { source: "disk", version: 1 }` on the WS within 500ms. Subsequent GET returns the new state.

- **AC4.3:** more involved. Setup: registry with one fixture, current version 0, in-memory Loro doc populated. From the test: (a) simulate a "browser save" by directly calling `registry.check_increment_and_merge(...)` with an edit to stock S1 (this becomes version 1). (b) Then externally rewrite the file with an edit to stock S2 (independent of S1). Expect: after the watcher processes the disk event, the merge keeps both edits (the doc has S1 from the in-memory edit and S2 from the disk edit). Verify by exporting the doc and asserting both stocks have the new equations. The version is now 2.

- **AC4.4:** setup as above. Trigger an atomic_write via the save handler (browser save path). Wait long enough for the watcher to see the event. Assert: no `ProjectChanged { source: "disk" }` is emitted (only the original `source: "user"` from the save). The hash-compare short-circuits the watcher's merge attempt.

- Negative test: external write that introduces a syntax error → no merge happens, no `ProjectChanged` is emitted, the in-memory doc is unchanged.

**Verification:**
- `cargo test -p simlin-serve --test watcher_merge` passes.

**Commit:** `serve: handle_model_change with hash echo-suppression and Loro merge`
<!-- END_TASK_5 -->

<!-- START_TASK_6 -->
### Task 6: `handle_model_removal` — drop registry entry + emit `ProjectRemoved`

**Verifies:** none directly (foundation for Phase 8 polish)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/events.rs` (add `ProjectRemoved { path: String }` variant to `WsMessage`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (existing `remove(path)` is enough; just call from the watcher)
- Test: extend `tests/watcher_merge.rs`

**Implementation:**
- `handle_model_removal(path)`: `registry.remove(&path)`; `state.events.publish(WsMessage::ProjectRemoved { path: <relative> })`.
- Sidecar pairing: if a `.mdl` was discovered with a `.sd.json` sidecar and the sidecar is removed, the `.mdl` becomes the source of truth again — re-add it to the registry by running discovery on its path. (This is an edge case; for Phase 4, document it but do not implement — file as a follow-up. Phase 8 polish revisits.)
- A removal of a non-tracked path is a no-op.

**Testing:**
- Test: with one `.stmx` in the registry, externally `rm` the file. Watcher event fires. Registry no longer has the entry. WS receives `ProjectRemoved { path }`.

**Verification:**
- `cargo test -p simlin-serve --test watcher_merge` passes the new removal test.

**Commit:** `serve: handle_model_removal emits ProjectRemoved over WebSocket`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: `handle_git_change` — invalidate git status cache for the affected repo

**Verifies:** server-rewrite.AC2.4

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/git.rs` (add `pub fn invalidate_repo_cache(&self, repo_root: &Path)`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs`

**Implementation:**
- `GitProbe`'s status cache (introduced in Phase 1 Task 6) is keyed by `(repo_root, mtime_of_.git/index)`. The watcher already invalidates implicitly via the mtime check, but explicit invalidation makes the contract clearer and guards against clock-skew weirdness.
- `invalidate_repo_cache(repo_root)`: removes any cached entries for that repo from the GitProbe's internal cache.
- `handle_git_change(repo_root)`: calls `state.git.invalidate_repo_cache(&repo_root)`. Then iterates over the registry: for each entry whose path is inside `repo_root`, recompute `git_state` via `state.git.status_for(&path)` and update the registry entry. For each updated entry, broadcast `ProjectChanged { source: Disk, version: meta.version }` (the model itself didn't change but its git state did — the frontend uses the same broadcast to refresh; alternatively add a `GitStatusChanged` variant if the cost of re-rendering becomes apparent. For Phase 4, reuse `ProjectChanged`).

**Testing:**
- Integration test in `tests/watcher_git.rs`: tempdir with `git init`, one tracked `.stmx`, watcher running. From the test, run `git commit -am 'change'` against the tracked file. Assert: within 500ms, the registry's git state for that file flips from "modified" to "tracked clean" (the commit moved the change from working-tree to history). WS receives a notification.

**Verification:**
- `cargo test -p simlin-serve --test watcher_git` passes.

**Commit:** `serve: handle_git_change invalidates GitProbe cache and refreshes affected entries`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Graceful shutdown wiring

**Verifies:** none directly (operational hygiene)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/main.rs` (capture Ctrl-C via `tokio::signal::ctrl_c().await`; trigger the shutdown signal; await both the Axum server's shutdown and the `WatcherActor`'s join)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs` (ensure the `run` loop exits cleanly on shutdown signal; drop the debouncer to release the OS-level watch)

**Implementation:**
- Use `tokio::select!` in `main` between the server task and the Ctrl-C signal. On Ctrl-C, broadcast shutdown to both the server (via `axum::serve(...).with_graceful_shutdown(...)`) and the watcher actor.
- Document the order of teardown: server first (stops accepting new connections), then watcher (avoids spurious events during teardown).

**Verification:**
- Manual: `cargo run -p simlin-serve` then Ctrl-C; verify the binary exits within ~1s without a "still listening on port" message on the next start.

**Commit:** `serve: graceful Ctrl-C shutdown for server and watcher`
<!-- END_TASK_8 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 9-10) -->
### Subcomponent C: Frontend — handle `ProjectRemoved` + disk-source updates

<!-- START_TASK_9 -->
### Task 9: Frontend handles `ProjectRemoved`

**Verifies:** server-rewrite.AC4.2 (UI surface for delete)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/App.tsx`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/ProjectList.tsx`
- Test: extend `App.test.tsx` (or create one if Phase 1 didn't)

**Implementation:**
- `App` already subscribes to `WsMessage` events (Phase 3 Task 10). Extend the dispatcher: on `projectRemoved`, remove the entry from the projects state. If `state.selectedPath === removed.path`, set `selectedPath = null` and render the `<EmptyState>` (or a more specific "this model was deleted on disk" message — Phase 8 polishes the message, Phase 4 just needs the sane fallback).

**Testing:**
- Jest test: render `<App>` with a mocked WS that emits `projectRemoved` for the currently-selected path. Assert the editor area falls back to the empty state and the entry is gone from the list.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: frontend handles projectRemoved by clearing selection`
<!-- END_TASK_9 -->

<!-- START_TASK_10 -->
### Task 10: Frontend distinguishes `source` in toasts (optional polish)

**Verifies:** none directly (UX clarity for AC4.2 / AC6.3)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`

**Implementation:**
- When a `ProjectChanged` event arrives with `source: "disk"`, the editor remounts (Phase 3 logic) but additionally a small toast is shown: "This model was updated on disk." This is a nice-to-have; if pressed for time it can move to Phase 8.

**Testing:**
- Jest test: send a `projectChanged` event with `source: "disk"`, assert the toast appears.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: surface disk-source updates with a small toast`
<!-- END_TASK_10 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase Verification Checklist

Before marking Phase 4 complete:

1. `cargo test --workspace` (no regressions; new watcher tests pass)
2. `cd src/simlin-serve/web && pnpm test && pnpm lint` (frontend clean)
3. `cargo clippy -p simlin-serve -- -D warnings` (clippy clean)
4. `cargo fmt -p simlin-serve --check` (formatted)
5. **Manual external-edit test:** start the server against a directory with an `.stmx`. Open the browser. In a terminal, `vim` the same file, change a stock's equation, write+quit. Within 1-2 seconds, the browser editor remounts with the new equation visible. No "reload" prompt.
6. **Manual concurrent edit test:** with the browser open, edit a flow in the editor (auto-save fires). In parallel, externally edit a stock via vim. The final state has both changes (verify via reload + GET).
7. **Manual byte-identical test:** save in the browser. Watch the tracing output — the watcher event for our own atomic write does NOT result in a re-merge or a second `ProjectChanged` broadcast. Confirms AC4.4.
8. **Manual git status test:** with a tracked file open in the browser, externally `git commit -am 'whatever'`. The git-status indicator in the sidebar flips from "modified" to "tracked clean".
9. **Manual delete test:** externally `rm` a model file. The browser sidebar drops the entry. If it was the active one, the editor falls back to the empty state.
10. **Graceful shutdown:** Ctrl-C exits cleanly within 1s; restart on the same port works.

If all 10 verifications pass, Phase 4 is done.
