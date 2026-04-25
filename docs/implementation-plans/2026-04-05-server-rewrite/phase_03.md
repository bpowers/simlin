# Phase 3: Loro Merge Primitive Implementation Plan

**Goal:** Introduce a per-project in-memory `LoroDoc` and a single `apply_canonical_json` primitive through which **all** writes to a project flow. Add an internal broadcast channel for `ProjectChanged` events and a WebSocket endpoint `/api/updates` (token-gated) that fans events out to subscribed browser tabs. After this phase, two near-simultaneous browser saves merge without data loss in tests, and the in-memory state is the source of truth for what gets serialized to disk.

**Architecture:** Each project entry in `ProjectRegistry` lazily creates a `LoroDoc` on first access. The doc's structure mirrors `simlin_engine::json::Project`: a root `LoroMap("project")` containing scalar fields plus nested `LoroMap`s for `models` (keyed by model name), each model containing `LoroMap`s for `stocks`/`flows`/`auxiliaries`/`modules` (keyed by canonical variable name) plus a `LoroList` for `views`. The `apply_canonical_json(doc, new_json)` primitive walks the incoming `serde_json::Value` against the current Loro state and emits the minimal op-set: scalar changes → `insert`, new keys → `insert_container` + populate, removed keys → `delete`, lists → replace-and-repush (acceptable trade-off for V1; named-key maps absorb the CRDT benefit). After every successful merge the registry's `version` increments, the `LoroDoc` exports its tip via `get_deep_value()` for serialization back to disk, and a `ProjectChanged { path, version, source }` envelope is broadcast on a `tokio::sync::broadcast` channel. The WebSocket handler subscribes to that channel and sends each event as a JSON frame to the connected browser. The frontend remounts `<Editor>` (using `key={projectVersion}`) on receipt, refetching the latest state.

**Tech Stack:** New: `loro = "1.11"`, `axum` `ws` feature (already in Phase 1's `Cargo.toml` if mentioned; otherwise add now), `tokio-tungstenite = "0.29"` + `futures-util = "0.3"` (dev-deps only). All other primitives reused from Phases 1-2.

**Scope:** Phase 3 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC4 (partial): Concurrent editing via Loro
- **server-rewrite.AC4.1 Success:** Two near-simultaneous edits from the browser and the MCP server both apply without data loss *(partial — Phase 3 verifies this for two concurrent **browser** edits via the merge primitive; the MCP-side participation arrives in Phase 6 once the in-process MCP exists)*

### server-rewrite.AC6 (partial, source="user"): MCP push notifications — internal broadcast surface
- **server-rewrite.AC6.3 Success:** Any change (browser, MCP, disk) emits a `projectChanged` notification with a `source` discriminator (`"user" | "agent" | "disk"`) *(partial — Phase 3 emits `source: "user"` for browser saves over the WebSocket. The `"agent"` arrives in Phase 6/7 via the MCP-side notification router; the `"disk"` arrives in Phase 4 with the file watcher.)*

This phase does NOT close any AC fully; it sets the structural foundation that Phases 4, 6, and 7 build on. Phase 3's "Done when" is operational: browser saves go through the merge primitive, the WebSocket reports changes, and a concurrency test demonstrates merge-without-loss for two browser saves.

---

## Notes for Executor

The Phase 3 codebase + external research produced findings that change naive readings of the design. Read these before implementing:

**1. Loro 1.11 — stable but no built-in JSON differ.** `LoroDoc` does not have an `apply_json` or "diff against this target value" method. The diff has to be hand-rolled: walk the incoming `serde_json::Value` alongside the existing `LoroMap`/`LoroList` state via `for_each` / `get`, and emit `insert` / `insert_container` / `delete` ops. Phase 3 implements this (Tasks 2-4).

**2. Map-by-name vs list-by-position trade-off.** The JSON shape from `simlin_engine::json::Project` (verified in `/home/bpowers/src/simlin/src/simlin-engine/src/json.rs`) uses `Vec<Stock>`, `Vec<Flow>`, etc. In Loro we project these into `LoroMap`s keyed by canonical variable name. **Why:** map-by-name gives last-writer-wins **per variable**, which is the CRDT property we want for "two browsers edit different variables simultaneously, both edits land". A `LoroList` would force position semantics (insertions/deletions at indices) and fight the natural "edit a stock" workflow. The conversion happens in `apply_canonical_json`: incoming Vec is keyed by `variable.name` (canonical form, matched against `simlin_engine::common::Ident<Canonical>` to handle case/whitespace consistently); existing LoroMap is walked and reconciled.

**3. Views and view elements stay as `LoroList`.** They are positionally meaningful (view 0 is the primary diagram). For Phase 3 we use replace-the-list semantics (delete all elements, push all new). This loses the CRDT benefit for views, which is acceptable because (a) view edits are infrequent compared to variable edits, (b) preserving uid-based identity through a `LoroMovableList` is significant additional complexity, and (c) view-level conflicts are visible (the user just sees their layout snap back). Document this trade-off in code; revisit if view conflicts become painful in real use.

**4. Opaque blobs.** `source.content` (raw MDL text), `graphical_function.points` (lookup tables), `arrayed_equation` (subscripted variable internals) are stored as `LoroValue::String` containing the JSON-stringified blob. They are too coarse for Loro to merge meaningfully, and storing them as opaque strings keeps the Loro tree shape predictable.

**5. `LoroDoc` is single-owner within a process.** Per Loro's semantics, CRDT merge applies to **separate replicas** (e.g., across machines). Within one process with a single `LoroDoc`, mutations must be serialized — there is no internal lock. We achieve serialization by holding the registry's write lock while we call `apply_canonical_json` against that project's `LoroDoc` (the registry already gates check-and-increment under the write lock; Phase 3 extends the locked section to cover the merge call). Trade-off: the lock is held longer (now spans the JSON parse + Loro merge + serialization). Mitigated by the lock being per-process (single user); the cost is acceptable for V1.

**6. `subscribe_root` for change events.** `LoroDoc::subscribe_root(callback)` fires after every `commit()` with a `DiffEvent`. Phase 3 uses this hook to push events onto the broadcast channel when external callers haven't already done so. **Wrinkle:** Our save handler already knows it just modified the doc — it can broadcast directly without going through `subscribe_root`. We use `subscribe_root` only for cases where the doc is mutated through other paths (currently none; Phase 4's file watcher is one such case). For Phase 3 the save handler broadcasts directly; we do not wire `subscribe_root` until Phase 4 needs it. Document this so the executor doesn't add subscription plumbing prematurely.

**7. Browser WebSocket bearer goes via `?token=...` query param.** Browser native `WebSocket` API cannot set custom request headers on the upgrade handshake. The frontend constructs `new WebSocket("ws://127.0.0.1:<port>/api/updates?token=<value>")`; the server extracts via `Query<TokenParams>` alongside `WebSocketUpgrade`.

**8. Editor cannot consume mid-life prop updates** (verified in `src/diagram/Editor.tsx`). The `<Editor>` is a `React.PureComponent` with no `componentDidUpdate`. `initialProjectJson` is consumed only at mount inside `openInitialProject()`. To present new state after a `ProjectChanged` event, the host must **remount** the Editor. The cleanest way is `<Editor key={projectVersion} ... />` — when `projectVersion` changes, React unmounts and remounts. Phase 2's 409 conflict handler already established this pattern; Phase 3 reuses it for live updates.

**9. tokio broadcast capacity = 64.** Plenty of headroom for a handful of WebSocket clients receiving infrequent project-change events. `Lagged(n)` errors are logged + ignored; the receiver auto-advances and resumes from the oldest retained message.

**10. Existing test patterns absent.** No existing first-party Rust test in this workspace exercises a real WebSocket upgrade. Phase 3 introduces the pattern: bind a real `TcpListener` on `127.0.0.1:0`, spawn `axum::serve` in `tokio::spawn`, connect via `tokio_tungstenite::connect_async`. Add `tokio-tungstenite = "0.29"` and `futures-util = "0.3"` to `[dev-dependencies]` in `simlin-serve/Cargo.toml`.

**11. Locking discipline.** The registry's `RwLock<HashMap>` write-lock now wraps four operations: (a) check-and-increment version, (b) call `apply_canonical_json` on the project's `LoroDoc`, (c) export the doc state, (d) update `mtime` after the disk write completes. The disk write itself happens **outside** the lock (since `simlin_engine::io::atomic_write` may block on fsync). Sequence: lock → check+increment+apply+export → unlock → atomic_write → re-acquire lock briefly to refresh `mtime`/`size` → broadcast. This keeps the lock scope tight while still preserving the version-correctness invariant.

**12. Preserve Phase 2's validation gate.** Phase 3's `apply_canonical_json` does not validate — it only performs structural diff/merge. Validation (the `simlin-engine` diagnostic check from Phase 2) still happens **before** the merge: the incoming JSON is parsed to `datamodel::Project`, validated with the baseline-comparison logic, then on pass converted back to JSON and merged into the LoroDoc. This way invalid JSON never enters the doc.

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-5) -->
### Subcomponent A: Loro doc + apply_canonical_json merge primitive

<!-- START_TASK_1 -->
### Task 1: Add `loro` dep, define `ProjectDoc` wrapping `LoroDoc`

**Verifies:** none directly (scaffolding for AC4.1)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/Cargo.toml` (add `loro = "1.11"`)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/loro_doc.rs`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (re-export `pub mod loro_doc;`)
- Test: inline `#[cfg(test)] mod tests` in `loro_doc.rs`

**Implementation:**
- `pub struct ProjectDoc { doc: LoroDoc }` (newtype so we can add behavior without exposing Loro's surface). `impl ProjectDoc`:
  - `pub fn new() -> Self` → constructs `LoroDoc::new()` and returns the wrapper. The doc starts empty; first `apply_canonical_json` populates it.
  - `pub fn apply_canonical_json(&self, new_json: &serde_json::Value) -> Result<(), MergeError>` → stub for now; Tasks 2-4 implement it.
  - `pub fn export_canonical_json(&self) -> Result<serde_json::Value, MergeError>` → reads `self.doc.get_deep_value()`, converts the resulting `LoroValue` tree to `serde_json::Value`. This is the inverse direction of the merge — what gets serialized to disk.
  - `pub fn current_state_as_json_string(&self) -> Result<String, MergeError>` → convenience: `serde_json::to_string(&self.export_canonical_json()?)`.
- `pub enum MergeError { LoroError(loro::LoroError), JsonError(serde_json::Error), ShapeError { path: String, expected: &'static str, actual: &'static str } }` with `Display` and `std::error::Error` impls.

**Testing:**
- Construct a `ProjectDoc`, immediately call `export_canonical_json` → expect an empty JSON object (or specific empty shape — verify what Loro returns for an empty doc).
- Construct, manually insert a single string key into the root map via the Loro API, export, assert the JSON contains that key. (This proves the wrapper compiles and the export round-trip works before we tackle the diff logic.)

**Verification:**
- `cargo test -p simlin-serve loro_doc::` passes.

**Commit:** `serve: add loro dep and ProjectDoc wrapper around LoroDoc`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: `apply_canonical_json` — root + scalar fields + nested maps (recursive)

**Verifies:** none directly (building block for AC4.1)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/loro_doc.rs`
- Test: inline tests in `loro_doc.rs`

**Implementation:**
- Implement the recursive object-map diff. The entry point gets the root LoroMap via `let root = self.doc.get_map("project");` then calls a helper `merge_map(&root, json_object)?`.
- `fn merge_map(map: &LoroMap, json: &serde_json::Map<String, Value>) -> Result<(), MergeError>`:
  1. Build a `HashSet<String>` of keys present in the incoming JSON.
  2. For each existing key in the LoroMap (use `map.for_each(|k, _| ...)` to collect keys first, since we can't mutate during iteration), if the key is not in the incoming set → `map.delete(&key)?`.
  3. For each (key, value) in the incoming JSON:
     - If `value.is_null()` → `map.delete(&key)?` (treat null as deletion).
     - If `value` is a scalar (`Bool`, `Number`, `String`):
       - If the existing entry equals the new value → no-op (avoids unnecessary ops).
       - Otherwise → `map.insert(&key, LoroValue::from(value.clone()))?`.
     - If `value` is an object: get-or-create child LoroMap (if existing entry is a LoroMap, reuse it; otherwise delete + insert_container); recurse.
     - If `value` is an array: delegate to `merge_list(...)` (Task 3).
- `commit()` is called once at the end of `apply_canonical_json` — Loro batches all the inserts/deletes between commits, and the commit is what fires `subscribe_root` callbacks. Single commit per merge call.
- Equality check for scalars: compare `LoroValue::from(json.clone())` to the existing `LoroValue` from `map.get(&key)`. This is straightforward since `LoroValue` derives `PartialEq`.

**Testing:**
- Apply a JSON `{ "name": "foo", "version": 1 }` to an empty doc → export equals the input.
- Apply `{ "name": "bar" }` to the same doc → export equals `{ "name": "bar" }` (the `version` key is removed).
- Apply `{ "name": "foo", "meta": { "author": "alice" } }` → export contains the nested `meta` map.
- Apply `{ "name": "foo", "meta": { "author": "bob" } }` → export shows the updated `author`.
- Apply twice with the same JSON → idempotent (export unchanged; no spurious ops). For the "no spurious ops" check, use the `LoroDoc` API to compare doc state before vs after — the exact method name in Loro 1.11 may be `state_frontiers()`, `state_frontier()` (singular), or `frontiers()`; verify against [docs.rs/loro/1.11/loro/struct.LoroDoc.html](https://docs.rs/loro/1.11/loro/struct.LoroDoc.html) before implementing. If Loro records ops on a no-op apply (some CRDTs do), document the limitation and use a content-comparison assertion (`export_canonical_json()` byte-equal) instead.

**Verification:**
- `cargo test -p simlin-serve loro_doc::` passes.

**Commit:** `serve: apply_canonical_json scalar + nested-map recursion`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: `apply_canonical_json` — list handling (replace-and-repush)

**Verifies:** none directly (building block for AC4.1)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/loro_doc.rs`
- Test: extend inline tests

**Implementation:**
- `fn merge_list(list: &LoroList, json: &Vec<Value>) -> Result<(), MergeError>`:
  1. Truncate the existing list: `let len = list.len(); if len > 0 { list.delete(0, len)?; }`. (Loro `LoroList::delete(pos, len)` removes a range.)
  2. For each element in the incoming JSON array:
     - Scalar → `list.push(LoroValue::from(value.clone()))?`
     - Object → `list.push_container(LoroMap::new())?` then recurse `merge_map(&pushed_map, ...)`. (`push_container` returns the inserted container.)
     - Array → `list.push_container(LoroList::new())?` then recurse `merge_list(...)`.
- The list semantics are intentionally lossy for CRDT purposes: we replace the entire list on every merge. This is correct (no data loss) but wasteful for big lists. For Phase 3 it's acceptable because:
  - Variable-keyed maps absorb the per-variable edits (the common case).
  - Views (~1-3 per project) are small enough that re-pushing is cheap.
  - View elements (~5-50 per view) are still small.
  Document this in a code comment as "Phase 3 trade-off: lists use replace-semantics; reorderings show as full-list rewrites".

**Testing:**
- Apply `{ "tags": ["a", "b"] }` → export has `tags: ["a", "b"]`.
- Apply `{ "tags": ["a", "c", "b"] }` → export has `tags: ["a", "c", "b"]`.
- Apply `{ "tags": [] }` → export has `tags: []`.
- Apply `{ "items": [{ "id": 1 }, { "id": 2 }] }` → export matches structurally.

**Verification:**
- `cargo test -p simlin-serve loro_doc::` passes.

**Commit:** `serve: apply_canonical_json list handling via replace-and-repush`
<!-- END_TASK_3 -->

<!-- START_TASK_4 -->
### Task 4: Project-shape-aware merge — variables-as-maps projection

**Verifies:** none directly (concurrency benefit for AC4.1)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/loro_doc.rs`
- Test: extend inline tests

**Implementation:**
- The naive `merge_map` from Task 2 + `merge_list` from Task 3 handles arbitrary JSON correctly but stores `models[].stocks` as a `LoroList` — losing the per-variable LWW property we want for concurrency.
- Add a JSON pre-processing step that re-shapes specific Vec fields into Map-keyed shapes before merging. The pre-processing happens **before** calling `merge_map` and is reversed in `export_canonical_json` so the on-disk shape is unchanged.
- `fn project_json_to_loro_shape(json: &Value) -> Result<Value, MergeError>`:
  - Walks the incoming `json::Project`-shaped value.
  - Replaces `models: [<Model>, ...]` with `models: { "<model_name>": <Model>, ... }`.
  - Within each model, replaces `stocks`/`flows`/`auxiliaries`/`modules` (all `Vec<Variable>`) with `<type>: { "<var_canonical_name>": <Variable>, ... }`.
  - Leaves `views`, `dimensions`, `units`, `groups`, `loop_metadata` as arrays (no key-projection benefit; views are positionally meaningful).
  - Variable name canonicalization: use `simlin_engine::common::Ident::<Canonical>::new(name).as_str().to_owned()` so map keys match the canonical form simlin-engine uses.
- `fn loro_shape_to_canonical_json(value: Value) -> Result<Value, MergeError>`:
  - Inverse: walks the exported value, converts the maps back to arrays, sorts by `name` (for determinism — matches `xmile`'s sort order).
- `apply_canonical_json` now calls `project_json_to_loro_shape` first, then `merge_map`. `export_canonical_json` exports via `get_deep_value`, converts to serde_json, then `loro_shape_to_canonical_json`.

**Testing:**
- Apply a small full `json::Project` (synthesized from `test/test-models/samples/teacup/teacup.xmile` parsed through the engine). Export → assert structural equality with the input (the inner Vec ordering is sorted by canonical name, which matches what the XMILE writer already does).
- **Concurrency invariant test (the AC4.1 partial verification):**
  - Apply project A (full state).
  - Snapshot version A_v.
  - In two parallel tasks: task 1 modifies stock "S1"'s equation, task 2 modifies stock "S2"'s equation. Each task: clone the JSON, mutate, call `apply_canonical_json`. Run both serially under the registry write lock (which is the actual production execution model — Loro is single-doc in our process).
  - After both, export. Assert both edits are present.
  - This test demonstrates the per-variable-key merge property. The test runs serially on purpose; concurrency at the API layer is what introduces the race, but inside the lock the operations sequence cleanly. (Phase 6 will exercise true cross-source concurrency once MCP and HTTP both hit the same registry.)

**Verification:**
- `cargo test -p simlin-serve loro_doc::` passes.

**Commit:** `serve: project-shape-aware variables-as-maps projection`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: Lazy `ProjectDoc` hydration in `ProjectRegistry`

**Verifies:** none directly (registry plumbing for AC4.1)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add `doc: Option<Arc<ProjectDoc>>` field on `ProjectMeta`; getter that lazily hydrates)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (`get_project` GET handler now uses the hydrated doc as the source of truth instead of re-reading the file each time)
- Test: extend `tests/api_get_project.rs`

**Implementation:**
- `ProjectMeta` gains `doc: Arc<RwLock<Option<Arc<ProjectDoc>>>>`. The outer Arc lets us share across clones; the inner RwLock+Option lets us lazily initialize.
- `pub fn get_or_init_doc(&self, abs_path: &Path) -> Result<Arc<ProjectDoc>, RegistryError>` on `ProjectRegistry`:
  - Acquires the registry write lock briefly to look up the entry.
  - Acquires the entry's `doc` write lock; if `Some`, return the cloned `Arc`.
  - If `None`: read the file from disk, parse to `datamodel::Project`, convert to `json::Project`, serialize to `Value`, construct a fresh `ProjectDoc::new()`, call `apply_canonical_json` to populate, store in the Option, return.
  - The hydration may panic if the file changed between discovery and now — wrap in `Result` and return a `HydrationFailed` error.
- `get_project` HTTP handler (Phase 1) now: gets the doc via `get_or_init_doc`, calls `current_state_as_json_string()`, returns that. The on-disk file is no longer read on every GET — once hydrated, the in-memory state is canonical.
- `version` reporting: still comes from `ProjectMeta.version`, not from the doc itself. The doc is always at the version stored in the registry — they advance in lockstep.

**Testing:**
- After GET on a project, the registry's `ProjectMeta.doc` is `Some`. A second GET returns the same content without re-reading the file (verifiable by deleting the file between the two GETs and confirming the second still succeeds — though that's hacky; a cleaner test counts file reads via a wrapping I/O recorder).
- Extend the existing `api_get_project` tests: response shape is unchanged (still `{ json, version, source_format }`).

**Verification:**
- `cargo test -p simlin-serve --test api_get_project` passes (existing assertions plus the lazy-hydration check).

**Commit:** `serve: lazy ProjectDoc hydration in ProjectRegistry`
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 6-9) -->
### Subcomponent B: Broadcast channel + WebSocket endpoint

<!-- START_TASK_6 -->
### Task 6: Internal broadcast channel for `ProjectChanged`

**Verifies:** none directly (plumbing for AC6.3 partial)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/events.rs`
- Modify: `lib.rs` (re-export)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (`AppState` gains an `events: Arc<EventBus>` field; `build_router` constructs and stores it)
- Test: inline tests in `events.rs`

**Implementation:**
- `pub struct EventBus { tx: broadcast::Sender<WsMessage> }` with capacity 64 (`broadcast::channel(64)`).
- `pub fn new() -> Self` constructs the channel; the receiver from `channel()` is dropped — subscribers create their own via `subscribe()`.
- `pub fn subscribe(&self) -> broadcast::Receiver<WsMessage>`.
- `pub fn publish(&self, msg: WsMessage)` calls `self.tx.send(msg)` and ignores the `Result` (an Err means no subscribers, which is fine).
- `pub enum WsMessage` (with `serde::Serialize`, `Clone`, `Debug`) — initial variant `ProjectChanged { path: String, version: u64, source: ChangeSource }` using `#[serde(tag = "type", rename_all = "camelCase")]`. Future variants (`ProjectFocused`, `SelectionChanged`, `DiagnosticsChanged`) added in Phase 7.
- `pub enum ChangeSource { User, Agent, Disk }` with `#[serde(rename_all = "lowercase")]`. Phase 3 only emits `User`.

**Testing:**
- Construct an `EventBus`, two subscribers, `publish` one message, both subscribers receive it. (`assert_eq!` after each `recv().await`.)
- Lagged-receiver test: capacity-64 bus, drop one subscriber's reads while publishing 100 messages, verify subsequent `recv()` returns `Err(Lagged(_))` then resumes successfully on the next call.

**Verification:**
- `cargo test -p simlin-serve events::` passes.

**Commit:** `serve: EventBus broadcast channel for ProjectChanged`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: WebSocket endpoint `/api/updates` with token gate

**Verifies:** server-rewrite.AC7.3 (server-side bearer enforcement)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (add `pub async fn updates_ws_handler(State(state), Query(params), ws: WebSocketUpgrade) -> impl IntoResponse`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/lib.rs` (`AppState` gains `launch_token: Arc<String>`; `build_router` mounts `.route("/api/updates", get(updates_ws_handler))`; ensure `axum` `Cargo.toml` has `features = ["ws"]`)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/tests/ws_updates.rs`
- Modify: `Cargo.toml` (add `[dev-dependencies] tokio-tungstenite = "0.29"`, `futures-util = "0.3"`)

**Implementation:**
- `WsParams { token: String }` — `serde::Deserialize`.
- Handler:
  1. Compare `params.token` against `state.launch_token` using **constant-time comparison** (`subtle::ConstantTimeEq` or a hand-rolled byte-loop). On mismatch, return `(StatusCode::UNAUTHORIZED, "invalid token").into_response()`. (Yes, this is local-loopback, but constant-time comparison is a 5-line safety belt and we prefer it.)
  2. On match, call `ws.on_upgrade(move |socket| handle_socket(socket, state.events.subscribe()))`.
- `async fn handle_socket(mut socket: WebSocket, mut rx: broadcast::Receiver<WsMessage>)`:
  - Loop with `tokio::select!`:
    - `result = rx.recv()`:
      - `Ok(msg)` → serialize via `serde_json::to_string(&msg)`, send via `socket.send(Message::Text(json.into()))`. On send error → break.
      - `Err(Lagged(n))` → `tracing::warn!("ws lagged by {n}; client may have missed events")`, continue.
      - `Err(Closed)` → break (server shutting down).
    - `msg = socket.recv()`:
      - `Some(Ok(Message::Close(_)))` or `None` → break.
      - `Some(Err(_))` → break.
      - `Some(Ok(_))` → ignore (Phase 3 has no client→server messages; Phase 7 will add `selectionChanged` from the browser).
- The token is generated in Phase 1's `main.rs` and stored in `AppState.launch_token` (added in this task).

**Testing:**
Verifies AC7.3 server-side enforcement. Tests in `tests/ws_updates.rs`:
- **Bind real listener + spawn server** pattern (use the canonical `axum::serve` + `tokio::spawn` approach).
- Test 1 — happy path: connect with `?token=<correct>`, assert handshake succeeds. Publish a `ProjectChanged` via `state.events.publish(...)` (use a back-channel — expose `publish` via a test helper). Receive on the WS, parse the JSON, assert structure.
- Test 2 — bad token: connect with `?token=wrong`, assert handshake fails (401).
- Test 3 — missing token: connect with no `?token=` query param, assert handshake fails (400 due to missing required `Query` field).
- Test 4 — lagged client: connect, do not read for a while, publish 100 messages, then read — expect to receive a partial set with no panic. (Lagged warning in tracing output is acceptable.)
- Test 5 — graceful close: connect, send `Message::Close(None)` from client, assert the server's `tokio::spawn`'d task exits cleanly (probe via channel-sender drop or by asserting the connection closes from the server side too).

**Verification:**
- `cargo test -p simlin-serve --test ws_updates` passes.

**Commit:** `serve: token-gated WebSocket endpoint /api/updates`
<!-- END_TASK_7 -->

<!-- START_TASK_8 -->
### Task 8: Save handler routes through `apply_canonical_json` + emits `ProjectChanged`

**Verifies:** server-rewrite.AC4.1 (full for browser-vs-browser)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (`save_project` switches from "validate → write directly" to "validate → merge into LoroDoc → export → write")
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add `pub fn check_increment_and_merge(&self, abs_path, expected_version, new_json) -> Result<(u64, Arc<ProjectDoc>), RegistryError>`)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/writer.rs` (the writer now serializes from the `ProjectDoc`'s exported JSON, not from the raw incoming JSON)
- Test: extend `tests/api_save.rs`

**Implementation:**
- `check_increment_and_merge` is a single method that holds the registry write lock for the entire (a) version check (b) version increment (c) `apply_canonical_json` call. It returns the new version and an `Arc<ProjectDoc>` so the caller can serialize/write **outside** the lock.
- `save_project` flow becomes:
  1. Path canonicalization + traversal check (unchanged from Phase 2).
  2. **Pre-fetch baseline** (still under read lock so we can re-parse the **current** doc state — not the disk file): `let current_doc = registry.get_or_init_doc(&abs_path)?; let current_json = current_doc.export_canonical_json()?; let current_project = json_to_datamodel(&current_json)?; let baseline = validation::compute_baseline(&current_project);`. (The doc-derived current state replaces the file-read from Phase 2.)
  3. Validate the incoming JSON: `validation::validate_save(&body.json, &baseline)?`. As before, on validation failure return 422.
  4. Acquire registry write lock via `check_increment_and_merge(&abs_path, body.version, &body.json_value)`. On version mismatch return 409. On apply error return 500. On success → `(new_version, project_doc)`.
  5. Outside the lock: `writer::save_to_disk(&project_doc.export_canonical_json()?, &target)?` (use the validated `datamodel::Project` from step 3 — but note we want to serialize from the **merged** state, not the raw incoming, in case the merge changed anything; rebuild `datamodel::Project` from the doc's exported JSON).
  6. Refresh registry meta (mtime, size) under a brief write lock.
  7. **Publish:** `state.events.publish(WsMessage::ProjectChanged { path: rel_path.clone(), version: new_version, source: ChangeSource::User })`.
  8. Return `200` with the `SaveResponse`.

**Testing:**
- All existing Phase 2 tests in `tests/api_save.rs` still pass.
- Extended test: connect a WebSocket client (using the test helper from Task 7), POST a save, assert the WS receives `ProjectChanged { path, version: 1, source: "user" }` within a reasonable timeout (e.g., 1 second via `tokio::time::timeout`).
- **AC4.1 partial verification:** in a single test, connect a WS client, POST save 1 (modifies stock S1), POST save 2 (modifies stock S2 against version 1). Both should succeed (versions 1 → 2). Final GET shows both modifications present. The `ProjectChanged` events for both saves arrive on the WS in order.

**Verification:**
- `cargo test -p simlin-serve --test api_save` passes (existing + new).

**Commit:** `serve: save routes through Loro merge primitive and emits ProjectChanged`
<!-- END_TASK_8 -->

<!-- START_TASK_9 -->
### Task 9: Healthcheck for the WS path; document the wire protocol

**Verifies:** none directly (operational observability)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/README.md` (add a "WebSocket protocol" section documenting the `WsMessage` envelope and the auth flow)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (add `tracing::info!` for each WS connection accept, `tracing::debug!` for each message sent)

**Implementation:**
- Document the wire shape: `{"type":"projectChanged","path":"...","version":N,"source":"user"|"agent"|"disk"}`. Show a minimal client snippet (browser JS) that opens the socket and prints incoming events.
- Trace logging gives ops visibility without changing behavior.

**Testing:**
- None (docs + tracing).

**Verification:**
- Manual `tail -f` of the binary's stderr while running confirms the trace lines appear on connection and event emit.

**Commit:** `serve: document WebSocket protocol; add tracing for WS lifecycle`
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 10-12) -->
### Subcomponent C: Frontend WebSocket client + Editor remount

<!-- START_TASK_10 -->
### Task 10: Frontend WebSocket client

**Verifies:** server-rewrite.AC7.3 (browser-side bearer wiring)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/web/src/ws.ts`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/App.tsx` (subscribe to WS in `componentDidMount`, store latest `ProjectChanged` per path in state)
- Test: `/home/bpowers/src/simlin/src/simlin-serve/web/src/ws.test.ts`

**Implementation:**
- `ws.ts` exports a `class UpdatesSocket`:
  - Constructor takes `(token: string, onMessage: (msg: WsMessage) => void)`.
  - On construction: opens `ws://${location.host}/api/updates?token=${encodeURIComponent(token)}`. (Use `location.host` so the port from Phase 1 matches.)
  - On `message` event: `JSON.parse(event.data)`, dispatch to `onMessage`.
  - On `close` or `error`: log and attempt reconnect after a 1s/2s/5s exponential backoff (cap 5s).
  - Public `close()` method for cleanup.
- `App.tsx` constructs an `UpdatesSocket` in `componentDidMount` (using the token from `sessionStorage`). On `ProjectChanged` for the currently-viewed path, sets state `{ liveVersion: version }`. Closes the socket in `componentWillUnmount`.
- `<EditorHost>` receives `liveVersion` as a prop. When `liveVersion > currentlyDisplayedVersion`, it triggers a refetch via `componentDidUpdate` (this is appropriate here because EditorHost is our own class component, not the third-party Editor).

**Testing:**
- Jest test: instantiate `UpdatesSocket` with a mock `WebSocket` (use `jest-websocket-mock` or hand-roll a class), send a fake message, assert `onMessage` is called with the parsed object.
- Reconnect test: simulate a close, assert reconnect after the backoff delay (use jest fake timers).

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: frontend UpdatesSocket with reconnect`
<!-- END_TASK_10 -->

<!-- START_TASK_11 -->
### Task 11: Editor remount on `liveVersion` advance via `key` prop

**Verifies:** server-rewrite.AC4.1 (UI surface — live updates render)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Test: extend `EditorHost.test.tsx`

**Implementation:**
- `<EditorHost>` already manages `state = { json, version, sourceFormat }`. When `props.liveVersion > state.version` (set by `App` from the WS event), `componentDidUpdate` triggers a refetch (`api.getProject(path)`), updates state with new `{ json, version, sourceFormat }`, and renders `<Editor key={state.version} ... />`. The `key={state.version}` causes React to unmount the old Editor and mount a fresh one with the new `initialProjectJson`. Document this in a comment: "Editor cannot consume mid-life prop updates; we remount on every external version advance, accepting the loss of undo history. Trade-off documented in Phase 3."

**Testing:**
- Jest test: render `<EditorHost path="x.stmx" liveVersion={0}>`, mock GET → returns version 0. Re-render with `liveVersion={1}`, mock the second GET → returns `{ version: 1, json: <updated> }`. Assert the Editor re-renders with the new initial JSON. Use `screen.getByTestId('editor-root')` (add a test-id to Editor's root or wrap with a recognizable shell).
- Loop-prevention check: when an own save bumps `state.version` to 1, the WS will eventually echo `liveVersion=1`. The handler must skip the refetch when `liveVersion <= state.version`. Test: render with version 1, set `liveVersion={1}`, assert no refetch.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: EditorHost remounts on WebSocket-driven version advance`
<!-- END_TASK_11 -->

<!-- START_TASK_12 -->
### Task 12: End-to-end live-update smoke test

**Verifies:** server-rewrite.AC4.1 (composition end-to-end)

**Files:**
- Create: `/home/bpowers/src/simlin/src/simlin-serve/tests/e2e_live_update.rs`

**Implementation:**
- Bind a real listener, spawn the server with a tempdir-backed registry containing a fixture `.xmile`.
- Connect a WS client.
- POST a save (mutates a variable). Assert the WS receives `ProjectChanged { version: 1, source: "user" }`.
- POST another save (mutates a different variable). Assert the WS receives `ProjectChanged { version: 2, source: "user" }`.
- GET the project — assert both mutations are present in the response JSON.

**Verification:**
- `cargo test -p simlin-serve --test e2e_live_update` passes.

**Commit:** `serve: end-to-end test for save → WS event → live state`
<!-- END_TASK_12 -->
<!-- END_SUBCOMPONENT_C -->

---

## Phase Verification Checklist

Before marking Phase 3 complete:

1. `cargo test --workspace` (no regressions; new tests pass)
2. `cd src/simlin-serve/web && pnpm test` and `pnpm lint` (frontend clean)
3. `cargo clippy -p simlin-serve -- -D warnings` (clippy clean)
4. `cargo fmt -p simlin-serve --check` (formatted)
5. **Manual two-tab test:** start the server against a directory with one model. Open two browser tabs. Edit a stock in tab 1, save. Within 1-2 seconds, tab 2 should re-render with the updated stock visible (the active editor remounts). Edit a flow in tab 2, save. Tab 1 re-renders.
6. **Manual concurrency test:** with two tabs open, almost-simultaneously edit two different variables (one in each tab), saving both. Both edits should land (verify by GET or reload); no data loss.
7. **Manual WS auth test:** with `wscat` (or browser DevTools), try connecting to `ws://127.0.0.1:<port>/api/updates` without a token → 400. With a wrong token → 401. With the correct token from the URL → handshake succeeds.

If all 7 verifications pass, Phase 3 is done.
