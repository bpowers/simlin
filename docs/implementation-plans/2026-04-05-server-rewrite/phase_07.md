# Phase 7: MCP Push Notifications Implementation Plan

**Goal:** Active MCP sessions receive four notification kinds — `simlin/projectFocused`, `simlin/selectionChanged`, `simlin/projectChanged`, `simlin/diagnosticsChanged` — driven by browser actions and internal state changes. The browser emits selection and focus events to the server over the existing WebSocket; the server fans these out (along with internal change/diagnostic events) to all subscribed MCP clients via per-session notification forwarders.

**Architecture:** The `EventBus` introduced in Phase 3 already broadcasts `WsMessage::ProjectChanged` (with `source: User` from browser saves, `source: Disk` from the file watcher in Phase 4, and `source: Agent` from MCP edits in Phase 6) and `WsMessage::ProjectRemoved` (Phase 4). Phase 7 adds three new variants — `ProjectFocused`, `SelectionChanged`, and `DiagnosticsChanged` — and wires production of each:
- **`ProjectFocused`** and **`SelectionChanged`**: The browser sends JSON frames over the existing WebSocket; the server's `handle_socket` parses them and publishes them on the EventBus.
- **`DiagnosticsChanged`**: Computed inside `RegistryAccess::save` and `RegistryAccess::create` (Phase 6) AND inside the file watcher's merge path (Phase 4). After each successful merge, simlin-engine's diagnostic check runs; if the resulting `(code, variable_name)` set differs from what was last computed for that path, publish `DiagnosticsChanged` with the formatted error list.

The fan-out to MCP clients uses a per-session pattern: each MCP session's `SimlinServeMcpServer` instance, in its `initialize` handler, captures its `Peer<RoleServer>` clone and spawns a background task that subscribes to the `EventBus` and translates each `WsMessage` into an rmcp `ServerNotification::CustomNotification` sent via the captured `Peer`. When the session ends, `peer.send_notification` returns `Err(ServiceError::TransportClosed)` and the task exits. No global peer registry is needed.

To add `onSelectionChanged` browser-side, Phase 7 introduces an optional `onSelectionChanged?: (idents: string[]) => void` prop on `<Editor>`'s `EditorPropsBase`. Existing consumers (`src/app`'s `HostedWebEditor`) are unaffected because the prop defaults to `undefined`. The `EditorHost` in `simlin-serve/web/` debounces selection changes browser-side (150ms via `setTimeout`-based pattern, matching the codebase's existing idiom) before sending over the WebSocket.

**Tech Stack:** No new server deps. New browser deps: none (debounce uses `setTimeout`).

**Scope:** Phase 7 of 8 from `/home/bpowers/src/simlin/docs/design-plans/2026-04-05-server-rewrite.md`.

**Codebase verified:** 2026-04-25

---

## Acceptance Criteria Coverage

This phase implements and tests:

### server-rewrite.AC6: MCP push notifications (full)
- **server-rewrite.AC6.1 Success:** Opening or switching a project in the browser emits a `projectFocused` notification to subscribed MCP clients
- **server-rewrite.AC6.2 Success:** Selecting a variable in the browser emits a `selectionChanged` notification with the variable's canonical idents
- **server-rewrite.AC6.3 Success:** Any change (browser, MCP, disk) emits a `projectChanged` notification with a `source` discriminator (`"user" | "agent" | "disk"`) — closing out the partial coverage from Phases 3-6
- **server-rewrite.AC6.4 Success:** Changes in validation diagnostics emit `diagnosticsChanged` notifications

This phase also addresses the **ordering** sub-requirement from the design's Phase 7 done-when ("ordering relative to tool responses is correct (no notification for an MCP-initiated edit before that edit's tool response)") — see Notes for the design decision.

---

## Notes for Executor

The Phase 7 codebase + research produced findings that change naive readings of the design. Read these before implementing:

**1. Per-session notification forwarder, not global peer registry.** Each MCP session's `SimlinServeMcpServer` instance captures its `Peer<RoleServer>` during `initialize`, spawns a `tokio::spawn` task that subscribes to the EventBus, and forwards each `WsMessage` as a `CustomNotification`. When the session ends, `peer.send_notification` returns `ServiceError::TransportClosed` and the task exits naturally. **Do NOT build a `PeerRegistry: Arc<RwLock<Vec<Peer<RoleServer>>>>` — it's the wrong abstraction.** The per-session pattern is simpler, scales naturally with sessions, and avoids manual cleanup.

**2. `Peer<RoleServer>` is `Clone + Send + Sync` in rmcp 1.5.x.** Confirmed via direct inspection of the rmcp source. The send signature is `pub async fn send_notification(&self, notification: ServerNotification) -> Result<(), ServiceError>`. Disconnect detection is `peer.is_transport_closed() -> bool` (cheap; reads an mpsc sender's `is_closed()`).

**3. ORDERING IS NOT GUARANTEED between tool responses and notifications in rmcp 1.5.x.** The service layer uses two internal channels: `sink_proxy_tx` for responses, `Peer.tx` for notifications. The Streamable HTTP transport delivers them on parallel paths — the notification can arrive at the client BEFORE the response to the tool call that triggered it. The design's "no notification for an MCP-initiated edit before that edit's tool response" requirement is, strictly speaking, unenforceable from the server side without significant additional plumbing.
   
   **Mitigation strategy: design the notification as an idempotent re-fetch hint, not as authoritative state.** The notification carries the new version number and source. AI clients should use it as a signal to (a) optionally re-read the project if they care about latest state, or (b) ignore it because they already have the state from the tool response. Document this in the README's "Notifications semantics" section. This is the same pattern the browser uses (Phase 3's `liveVersion`-driven remount).
   
   **A stronger guarantee would require:** delaying the EventBus publish until after the rmcp service has flushed the tool response — but rmcp doesn't expose hooks for that. Acceptable trade-off for V1: notifications are advisory, not authoritative.

**4. Add `onSelectionChanged` prop to `<Editor>`.** The `Editor` component in `src/diagram/Editor.tsx` has no public selection callback today. The internal `handleSelection` method (line 574-583) sets state but doesn't notify the host. Phase 7 adds:
   ```typescript
   interface EditorPropsBase {
       initialProjectVersion: number;
       name: string;
       embedded?: boolean;
       readOnlyMode?: boolean;
       onSelectionChanged?: (idents: string[]) => void;  // NEW
   }
   ```
   Then in `handleSelection`: after `setState({ selection })`, call `this.props.onSelectionChanged?.(this.getSelectionIdents())`. The existing `getSelectionIdents()` helper at line 1577-1592 returns `string[]` of canonical names. Make the prop optional so `src/app`'s `HostedWebEditor` (which doesn't pass it) keeps working unchanged.
   - **Note for simlin-mcp users**: this change is in the `@simlin/diagram` package, not the `@simlin/mcp` server. It only affects the React component library. simlin-mcp is unaffected.

**5. `embedded={true}` mode disables `onSelectionChanged`.** In `embedded` mode, the canvas's `onSetSelection` is a no-op (`Editor.tsx:1500`), so `handleSelection` never fires, so the new callback never invokes. This is correct: `simlin-serve` uses `embedded={false}` (Phase 2 onward), and `src/app`'s embedded editor wouldn't want to fire selection events anyway.

**6. Browser-side debouncing with raw `setTimeout`.** No `lodash`/`use-debounce` library is in the codebase. The existing pattern is raw `setTimeout` (e.g., `Editor.tsx:279, 336, 447`). The `EditorHost`'s selection forwarder cancels any pending timer on each `onSelectionChanged` call and sets a new one for 150ms. Only fire the WS frame after the timer expires.

**7. `selectionChanged` payload must include `path`.** The Editor only knows idents, not which project they belong to. The host (`EditorHost`) combines `props.path` with the idents before sending. The wire frame:
   ```json
   {"type":"selectionChanged","path":"models/teacup.xmile","variableIdents":["teacup_temperature","ambient_temperature"]}
   ```

**8. `projectFocused` is host-side only.** Same logic: the Editor doesn't know "which project is focused" — that's a host concept. Fire `projectFocused` from `EditorHost.componentDidMount` (initial focus) and from `componentDidUpdate` when `props.path` changes. The `App.tsx` shell can ALSO emit a `projectFocused` when it switches `selectedPath` — either is fine; pick the one that's simpler. Recommended: `EditorHost.componentDidMount`/`componentDidUpdate`. The wire frame:
   ```json
   {"type":"projectFocused","path":"models/teacup.xmile"}
   ```

**9. `diagnosticsChanged` set comparison.** The simlin-mcp `EditModel` uses `HashSet<(error_code, variable_name)>` (`edit_model.rs:244-258`). Phase 7 stores the last-computed set per registry entry (in `ProjectMeta.last_diagnostic_keys: BTreeSet<(String, Option<String>)>`) and compares after each merge. **Cheap comparison: BTreeSet equality.** Only emit `DiagnosticsChanged` when the set differs.
   - First-ever computation for a path: compare against an empty set; if any errors exist, emit `DiagnosticsChanged` with the full list.
   - When the set becomes empty (all errors fixed), emit `DiagnosticsChanged` with `errors: []`.

**10. WebSocket message envelope discipline.** Phase 3 used `{"type":"projectChanged",...}` with `#[serde(tag = "type", rename_all = "camelCase")]`. Phase 4 added `projectRemoved`. Phase 7 adds `projectFocused`, `selectionChanged`, `diagnosticsChanged`. **All inbound (browser→server) frames use the same shape with `type` discriminant for symmetry.** The server's `handle_socket` parses incoming JSON via `serde_json::from_str::<ClientWsMessage>` (a separate enum from `WsMessage` because the server handles different inbound vs outbound types).

**11. Authentication of inbound WS frames is via the existing token gate.** Phase 3's WS upgrade already validates the launch token. Once the connection is up, all inbound frames are trusted (loopback + token already verified at upgrade time).

**12. `notifications_router` per-session task — error handling.**
   - `event_bus.subscribe()` returns a `broadcast::Receiver<WsMessage>` whose `recv().await` yields `Result<WsMessage, broadcast::error::RecvError>`. On `Lagged(n)` → log `tracing::warn!`, continue. On `Closed` → exit task.
   - `peer.send_notification(...).await` returns `Result<(), ServiceError>`. On `TransportClosed` → exit task. On other errors → log + continue.

**13. The MCP notification method name.** Use `simlin/projectChanged`, `simlin/projectFocused`, `simlin/selectionChanged`, `simlin/diagnosticsChanged`. The `simlin/` prefix is a custom namespace; per the rmcp research it appears as the literal `method` field in the JSON-RPC notification frame (no `notifications/` prefix added by rmcp). This avoids collision with future MCP standard notifications and signals to AI clients that these are simlin-specific.

**14. `mcp-remote` (Claude Desktop proxy) passes notifications through.** Confirmed from the `geelen/mcp-remote` source: the proxy forwards all incoming server messages unconditionally to the stdio client. So Claude Desktop users receive these notifications too — assuming the Desktop client surfaces them somewhere (currently it does so only for standard MCP notification types; custom ones may be silently ignored at the UI layer but they DO arrive on the wire).

---

## Implementation Tasks

<!-- START_SUBCOMPONENT_A (tasks 1-3) -->
### Subcomponent A: WsMessage variants + Editor onSelectionChanged prop

<!-- START_TASK_1 -->
### Task 1: Add three new `WsMessage` variants

**Verifies:** none directly (data types for AC6.1, AC6.2, AC6.4)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/events.rs`
- Test: extend `events::tests`

**Implementation:**
- Add to the `WsMessage` enum:
  ```rust
  ProjectFocused { path: String },
  SelectionChanged { path: String, variable_idents: Vec<String> },
  DiagnosticsChanged { path: String, errors: Vec<ValidationError> },
  ```
  All using the existing `#[serde(rename_all = "camelCase")]` so wire format is `variableIdents` etc.
- `ValidationError` reuses `simlin_mcp_core::errors::ValidationError` (Phase 5) — re-export from `events.rs` for ergonomics.

**Testing:**
- Round-trip serialize each new variant via `serde_json::to_string` and back; assert the wire shape.
- Publish each on a test EventBus; subscribers receive them.

**Verification:**
- `cargo test -p simlin-serve events::` passes.

**Commit:** `serve: add ProjectFocused/SelectionChanged/DiagnosticsChanged WsMessage variants`
<!-- END_TASK_1 -->

<!-- START_TASK_2 -->
### Task 2: Add `onSelectionChanged` prop to `<Editor>`

**Verifies:** server-rewrite.AC6.2 (signal source)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/diagram/Editor.tsx`
- Test: add or extend `/home/bpowers/src/simlin/src/diagram/Editor.test.tsx` (or create one if absent — the `src/diagram` package already uses Jest per Phase 1's investigation)

**Implementation:**
- Add to `EditorPropsBase` (around `Editor.tsx:229-235`):
  ```typescript
  onSelectionChanged?: (idents: string[]) => void;
  ```
- In `handleSelection` (around `Editor.tsx:574-583`), after `this.setState({ selection })`:
  ```typescript
  if (this.props.onSelectionChanged) {
    const idents = this.getSelectionIdents();
    // Defer one tick so the new state is committed before the host reads it back.
    setTimeout(() => this.props.onSelectionChanged?.(idents), 0);
  }
  ```
- The deferred call ensures React's setState commit happens before the host receives the callback (avoids a stale-state race if the host calls back into the editor synchronously).

**Testing:**
- Render `<Editor>` with `onSelectionChanged={mock}`. Programmatically trigger `handleSelection` (or simulate a click on a stock). Assert `mock` is called with the canonical ident array.
- Render without `onSelectionChanged` — no error (the optional callback is safely skipped).

**Verification:**
- `cd src/diagram && pnpm test` passes the new tests.
- `cargo test --workspace` and `pnpm test` overall pass — confirming `src/app`'s `HostedWebEditor` is unaffected.

**Commit:** `diagram: optional onSelectionChanged prop on Editor`
<!-- END_TASK_2 -->

<!-- START_TASK_3 -->
### Task 3: Inbound `ClientWsMessage` enum + parser in `handle_socket`

**Verifies:** AC6.1, AC6.2 (server-side ingestion)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/events.rs` (add `ClientWsMessage` enum)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (replace the Phase 3 "ignore inbound" branch with a parse-and-publish path)

**Implementation:**
- `pub enum ClientWsMessage` with `#[serde(tag = "type", rename_all = "camelCase")]`:
  ```rust
  ProjectFocused { path: String },
  SelectionChanged { path: String, variable_idents: Vec<String> },
  ```
  (No `DiagnosticsChanged` from the client — diagnostics are server-computed.)
- In `handle_socket`'s `Some(Ok(Message::Text(text)))` branch:
  ```rust
  match serde_json::from_str::<ClientWsMessage>(&text) {
      Ok(ClientWsMessage::ProjectFocused { path }) => {
          state.events.publish(WsMessage::ProjectFocused { path });
      }
      Ok(ClientWsMessage::SelectionChanged { path, variable_idents }) => {
          state.events.publish(WsMessage::SelectionChanged { path, variable_idents });
      }
      Err(e) => {
          tracing::warn!("ws: malformed inbound frame: {e}");
          // Continue serving — don't break the connection on bad client input.
      }
  }
  ```
- Document inline that the WS channel is one-way for `DiagnosticsChanged` and `ProjectChanged` events (those are server-internal); only `ProjectFocused` and `SelectionChanged` are accepted from the client.

**Testing:**
- Existing `tests/ws_updates.rs` from Phase 3: extend with a "send selectionChanged from client; subscriber receives it" test. Use `tokio_tungstenite` to send a `Message::Text` with the JSON frame; spawn a second `subscribe()` to verify the EventBus saw it.
- Malformed-input test: send `Message::Text("not json")`. Connection stays open; tracing warning emitted (assertable via `tracing-subscriber`'s `tracing-test` crate, or just verify the connection didn't close).

**Verification:**
- `cargo test -p simlin-serve --test ws_updates` passes new tests.

**Commit:** `serve: WS handler parses inbound projectFocused/selectionChanged`
<!-- END_TASK_3 -->
<!-- END_SUBCOMPONENT_A -->

<!-- START_SUBCOMPONENT_B (tasks 4-5) -->
### Subcomponent B: Frontend — emit projectFocused / selectionChanged

<!-- START_TASK_4 -->
### Task 4: `EditorHost` emits `projectFocused` on path change

**Verifies:** server-rewrite.AC6.1 (browser side)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/ws.ts` (add `send` method to `UpdatesSocket` for client→server frames)
- Test: extend `EditorHost.test.tsx`

**Implementation:**
- `UpdatesSocket` (Phase 3 Task 10): add `public send(msg: ClientWsMessage): void` that does `this.socket.send(JSON.stringify(msg))` if the connection is open; otherwise enqueues for after reconnect.
- `EditorHost` accepts `props.socket: UpdatesSocket` (passed down from `App`):
  - In `componentDidMount`: `props.socket.send({ type: "projectFocused", path: props.path })`.
  - In `componentDidUpdate(prevProps)`: if `prevProps.path !== this.props.path`, send `projectFocused` with the new path.

**Testing:**
- Mock `UpdatesSocket`. Render `<EditorHost path="a.stmx">`. Verify `socket.send` was called with `{ type: "projectFocused", path: "a.stmx" }`.
- Re-render with `path="b.stmx"`. Verify another `projectFocused` for `"b.stmx"`.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: EditorHost emits projectFocused on mount and path change`
<!-- END_TASK_4 -->

<!-- START_TASK_5 -->
### Task 5: `EditorHost` emits debounced `selectionChanged`

**Verifies:** server-rewrite.AC6.2 (browser side)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/web/src/components/EditorHost.tsx`
- Test: extend `EditorHost.test.tsx`

**Implementation:**
- `EditorHost` keeps a `private selectionDebounceTimer: number | null` field.
- Pass `onSelectionChanged={(idents) => this.handleSelectionChanged(idents)}` to `<Editor>`.
- `handleSelectionChanged(idents: string[])`:
  ```typescript
  if (this.selectionDebounceTimer !== null) {
    clearTimeout(this.selectionDebounceTimer);
  }
  this.selectionDebounceTimer = window.setTimeout(() => {
    this.props.socket.send({
      type: "selectionChanged",
      path: this.props.path,
      variableIdents: idents,
    });
    this.selectionDebounceTimer = null;
  }, 150);
  ```
- In `componentWillUnmount`: clear the pending timer.

**Testing:**
- Use jest fake timers. Trigger `handleSelectionChanged(["a"])`, then `handleSelectionChanged(["a","b"])` 50ms later, then advance 200ms. Assert `socket.send` was called exactly once with the latest idents `["a","b"]`.
- Cleanup test: trigger, unmount before 150ms, advance time. Assert `socket.send` was NOT called.

**Verification:**
- `cd src/simlin-serve/web && pnpm test` passes.

**Commit:** `serve: EditorHost emits debounced selectionChanged via setTimeout`
<!-- END_TASK_5 -->
<!-- END_SUBCOMPONENT_B -->

<!-- START_SUBCOMPONENT_C (tasks 6-7) -->
### Subcomponent C: Server-side `diagnosticsChanged` production

<!-- START_TASK_6 -->
### Task 6: Cache last-known diagnostics per registry entry

**Verifies:** none directly (foundation for AC6.4)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/registry.rs` (add `last_diagnostic_keys: BTreeSet<(String, Option<String>)>` field on `ProjectMeta`; helper to update it under the registry write lock)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/diagnostics.rs` (compute the current diagnostic set from a `datamodel::Project` — same simlin-engine pattern as Phase 2's validation)
- Modify: `lib.rs` (re-export)

**Implementation:**
- `pub fn compute_diagnostic_set(project: &datamodel::Project) -> (BTreeSet<(String, Option<String>)>, Vec<ValidationError>)`:
  - Build `SimlinDb::default()` + `sync_from_datamodel` + `collect_all_diagnostics` (filter to errors) + `collect_formatted_errors`.
  - Convert to `BTreeSet<(code, variable_name)>` for the cheap comparison key, plus a `Vec<ValidationError>` for the wire payload.
- `pub fn diagnostics_set_changed(meta: &ProjectMeta, new_keys: &BTreeSet<...>) -> bool`: just `meta.last_diagnostic_keys != *new_keys`.
- Initial value of `last_diagnostic_keys` is the empty set.

**Testing:**
- Unit: load fixture, compute → empty set (assuming the fixture is clean). Mutate the fixture to introduce an error, recompute → set has the new entry, `diagnostics_set_changed` returns true.

**Verification:**
- `cargo test -p simlin-serve diagnostics::` passes.

**Commit:** `serve: cache last_diagnostic_keys on ProjectMeta; compute helper`
<!-- END_TASK_6 -->

<!-- START_TASK_7 -->
### Task 7: Emit `DiagnosticsChanged` from all merge paths

**Verifies:** server-rewrite.AC6.4

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/handlers.rs` (`save_project` after a successful merge, recompute diagnostics, compare, emit if changed)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/access.rs` (`RegistryAccess::save` and `::create` — same recompute + emit)
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/watcher.rs` (`handle_model_change` — same recompute + emit after the disk-source merge)
- Test: new `tests/diagnostics_events.rs`

**Implementation:**
- Common helper: `fn maybe_emit_diagnostics_changed(state: &AppState, abs_path: &Path, project: &datamodel::Project)`:
  1. `let (new_keys, formatted) = compute_diagnostic_set(project);`
  2. Acquire registry write lock; look up the meta; if `meta.last_diagnostic_keys == new_keys` → return.
  3. Update `meta.last_diagnostic_keys = new_keys.clone()` under the lock.
  4. Drop the lock; publish `WsMessage::DiagnosticsChanged { path: <relative>, errors: formatted }`.
- Call this helper from each merge path.
- Important: `DiagnosticsChanged` always emits AFTER `ProjectChanged` for the same path, so subscribers see the model change first. The invariant we rely on: this helper publishes both messages **sequentially in the same async task**, so within a single `EventBus::publish` call sequence the order is preserved. (Tokio's broadcast channel preserves FIFO within a single sender's call sequence; cross-task interleaving is possible but not relevant here because both publishes happen in this one task.) Document the invariant in a code comment so a future maintainer doesn't break it by parallelizing the publishes.

**Testing:**
- Setup: tempdir with a clean fixture. Subscribe to EventBus. POST a save that introduces a syntax error in a single equation. Expect: receive `ProjectChanged` then `DiagnosticsChanged` with one error entry.
- Then save a second time fixing the error. Expect: receive `ProjectChanged` then `DiagnosticsChanged` with `errors: []`.
- Save a third time changing nothing diagnostically (e.g., add a new clean variable). Expect: receive `ProjectChanged` ONLY (no `DiagnosticsChanged` because the set didn't change).

**Verification:**
- `cargo test -p simlin-serve --test diagnostics_events` passes.

**Commit:** `serve: emit DiagnosticsChanged when validation set differs`
<!-- END_TASK_7 -->
<!-- END_SUBCOMPONENT_C -->

<!-- START_SUBCOMPONENT_D (task 8) -->
### Subcomponent D: MCP per-session notifications router

<!-- START_TASK_8 -->
### Task 8: Spawn per-session forwarder in `initialize`

**Verifies:** server-rewrite.AC6.1, AC6.2, AC6.3, AC6.4 (MCP-side delivery)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/server.rs` (override `ServerHandler::initialize` to capture the peer + spawn the forwarder task)
- Create: `/home/bpowers/src/simlin/src/simlin-serve/src/mcp/notifications.rs` (the `forward_events_to_peer` async fn)
- Test: extend `tests/mcp_tool_surface.rs` with notification-receipt assertions

**Implementation:**
- Override `initialize` on `SimlinServeMcpServer`:
  ```rust
  async fn initialize(
      &self,
      _request: InitializeRequestParams,
      ctx: RequestContext<RoleServer>,
  ) -> Result<InitializeResult, McpError> {
      // Default response from get_info (would normally be auto-generated by #[tool_handler])
      let info = self.get_info();
      
      // Capture the peer and spawn the forwarder.
      let peer = ctx.peer.clone();
      let bus_rx = self.state.events.subscribe();
      tokio::spawn(notifications::forward_events_to_peer(peer, bus_rx));
      
      Ok(InitializeResult {
          protocol_version: info.protocol_version,
          capabilities: info.capabilities,
          server_info: info.server_info,
          instructions: info.instructions,
      })
  }
  ```
- `forward_events_to_peer(peer, mut rx)`:
  ```rust
  loop {
      match rx.recv().await {
          Ok(msg) => {
              let (method, params) = wire_pair(msg);
              let notif = CustomNotification::new(method, Some(params));
              if let Err(ServiceError::TransportClosed) = peer
                  .send_notification(ServerNotification::CustomNotification(notif))
                  .await
              {
                  break;
              }
              // Other errors logged but don't terminate
          }
          Err(broadcast::error::RecvError::Lagged(n)) => {
              tracing::warn!("notifications forwarder lagged by {n} messages");
          }
          Err(broadcast::error::RecvError::Closed) => break,
      }
  }
  ```
- `fn wire_pair(msg: WsMessage) -> (&'static str, serde_json::Value)`: maps each `WsMessage` variant to (method-string, JSON-value) pair. E.g., `WsMessage::ProjectChanged { path, version, source }` → `("simlin/projectChanged", json!({"path": path, "version": version, "source": source}))`. The method strings match the design's notification names: `simlin/projectChanged`, `simlin/projectFocused`, `simlin/selectionChanged`, `simlin/diagnosticsChanged`. (Note: `WsMessage::ProjectRemoved` from Phase 4 also maps — `simlin/projectRemoved` — for completeness.)
- The `#[tool_handler]` macro generates a default `initialize` impl; we override by NOT using the macro for that method (or by manually wiring). Verify against rmcp 1.5.x: the typical pattern is to define `initialize` ourselves and let `#[tool_handler]` generate the rest. If `#[tool_handler]` overwrites our `initialize`, we may need to drop the macro and manually impl `ServerHandler` (a few extra dozen lines).

**Testing:**
- Setup: test server with EventBus. Connect rmcp HTTP client. Publish a `WsMessage::ProjectChanged` on the bus. Within 500ms, the rmcp client should receive a notification with method `"simlin/projectChanged"` and the expected payload. Use rmcp's client API to listen for notifications.
- Test all four notification types similarly.
- Disconnect test: connect, then drop the client. Wait 1s. The forwarder task should have exited (verifiable by spawning it via a sentinel-returning wrapper that signals on completion).

**Verification:**
- `cargo test -p simlin-serve --test mcp_tool_surface` passes new notification tests.

**Commit:** `serve: per-session forwarder routes EventBus to MCP notifications`
<!-- END_TASK_8 -->
<!-- END_SUBCOMPONENT_D -->

<!-- START_SUBCOMPONENT_E (task 9) -->
### Subcomponent E: Documentation

<!-- START_TASK_9 -->
### Task 9: Document notifications semantics in README

**Verifies:** none directly (operational documentation)

**Files:**
- Modify: `/home/bpowers/src/simlin/src/simlin-serve/README.md`

**Implementation:**
- Add a "Notifications" section listing the four notification methods (`simlin/projectChanged`, `simlin/projectFocused`, `simlin/selectionChanged`, `simlin/diagnosticsChanged`), their payload shapes, and when each fires.
- **Document the ordering caveat:** "Notifications are advisory and may arrive before or after a tool response that triggered them. AI clients should treat each notification as a hint to optionally re-fetch latest state, not as authoritative delivery of the new state."
- Document the `mcp-remote` proxy pass-through for Claude Desktop users (notifications do flow through, but Claude Desktop's UI may not visibly surface custom-method notifications).
- Example notification frame in the README:
  ```json
  {"jsonrpc":"2.0","method":"simlin/projectChanged","params":{"path":"models/teacup.xmile","version":3,"source":"user"}}
  ```

**Testing:** None (documentation).

**Verification:** Manual review.

**Commit:** `serve: document MCP push notification surface in README`
<!-- END_TASK_9 -->
<!-- END_SUBCOMPONENT_E -->

---

## Phase Verification Checklist

Before marking Phase 7 complete:

1. `cargo test --workspace` passes (no regressions).
2. `cd src/simlin-serve/web && pnpm test` and `cd src/diagram && pnpm test` pass (Editor + EditorHost tests).
3. `cargo clippy --workspace -- -D warnings` clean.
4. `cargo fmt --workspace --check` clean.
5. **Manual `projectFocused` test:** start `simlin-serve`. Open the browser, configure Claude Code CLI per Phase 6's README. Open a model in the browser. Run `claude mcp call simlin-serve list_resources` (or use the inspector). The AI session should have received a `simlin/projectFocused` notification (visible in `claude --debug` logs).
6. **Manual `selectionChanged` test:** with the same setup, click a stock in the browser editor. The AI session receives `simlin/selectionChanged` with the canonical name.
7. **Manual `projectChanged`:** save in the browser → `simlin/projectChanged` with `source: "user"`. Edit via MCP `edit_model` → `source: "agent"` (and ALSO sent to other MCP clients if any). Externally `vim` the file → `source: "disk"`.
8. **Manual `diagnosticsChanged`:** with the AI session live, edit a variable in the browser to introduce a syntax error. The AI receives `simlin/diagnosticsChanged` with the error list. Fix the error. The AI receives `simlin/diagnosticsChanged` with `errors: []`.
9. **Disconnect cleanup:** kill the AI client. Watch the server's tracing output — the forwarder task should log a clean exit (no panics, no leaked resources).

If all 9 verifications pass, Phase 7 is done.
