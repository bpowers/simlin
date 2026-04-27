# Server Rewrite — Human Test Plan

This test plan complements the automated tests for the 8-phase server rewrite implemented under [docs/implementation-plans/2026-04-05-server-rewrite/](../implementation-plans/2026-04-05-server-rewrite/). It covers the manual verifications that cannot be automated in CI (browser auto-open, headless behavior, multi-tab live editing, external-editor coexistence, AI client integration).

## Prerequisites

- macOS arm64, Linux x64 (with `DISPLAY`), and a Windows x64 host available for AC7.2 and AC7.4 verification.
- Repository checked out at `server-rewrite` head.
- Build the binary in release: `cargo build --release -p simlin-serve` (artifact at `target/release/simlin-serve`).
- `cargo test -p simlin-serve` passes in default mode.
- `cargo test --release --test smoke -- --ignored` passes locally.
- `pnpm --filter @simlin/serve-web run test` passes.
- `git --version` returns successfully (required for AC2 manual checks).
- Write a fresh playground directory: `mkdir -p ~/simlin-playground/sub && cd ~/simlin-playground` and place at least one of each: `teacup.xmile`, `teacup.mdl`, `small.sd.json`, `sub/nested.xmile` (copy from `test/test-models/samples/teacup/` and `test/sd-ai-simple.sd.json`).

## Phase 1: Default Browser Launch (AC7.2)

| Step | Action | Expected |
|------|--------|----------|
| 1 | On macOS: `target/release/simlin-serve ~/simlin-playground` | Two lines printed: `  UI:  http://127.0.0.1:<port>/?token=<token>` and `  MCP: http://127.0.0.1:7878/mcp`; default browser opens to UI URL |
| 2 | On Linux with `DISPLAY` set: `target/release/simlin-serve ~/simlin-playground` | Same two URLs print; default browser tab opens (xdg-open) |
| 3 | On Windows (PowerShell): `.\target\release\simlin-serve.exe %USERPROFILE%\simlin-playground` | Two URLs print; default browser opens to UI URL |
| 4 | In each browser tab, verify the project list shows `small.sd.json`, `sub/nested.xmile`, `teacup.mdl`, `teacup.xmile` in alphabetical order | List renders with bare basenames where unique, full path under collisions |

## Phase 2: Headless Fallbacks (AC7.4)

| Step | Action | Expected |
|------|--------|----------|
| 1 | On Linux: `unset DISPLAY && target/release/simlin-serve ~/simlin-playground` | Both URLs print prominently; server keeps running (no exit, no panic) |
| 2 | While running, in another terminal: `curl -H "Authorization: Bearer <token>" http://127.0.0.1:<port>/api/projects` | 200 with non-empty `projects` array |
| 3 | Stop with Ctrl-C, then run `target/release/simlin-serve --no-open ~/simlin-playground` | Server starts; no browser opens; URL still printed |
| 4 | Repeat step 1 on macOS in a no-GUI ssh session | Same: URL prints, server runs, no error |
| 5 | Repeat step 1 on Windows Server Core or with `start` removed from PATH | URL prints, server runs |

## Phase 3: End-to-End Editing (AC1, AC3, AC4, AC6)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open browser tab from Phase 1 | Project list visible; selecting `teacup.xmile` opens the diagram editor |
| 2 | Drag a stock; the diagram updates | Position reflects drag |
| 3 | Add an auxiliary `manual_test_aux` with equation `42`, save | No banner; version-counter advances; no errors |
| 4 | In a separate terminal: `cat ~/simlin-playground/teacup.xmile` | XMILE contains `manual_test_aux` |
| 5 | While the tab is open, externally: `echo "<root/>" >> ~/simlin-playground/teacup.xmile`, then save again | Status toast "model was updated on disk" appears momentarily |
| 6 | Open the same browser to two tabs side-by-side, edit different stocks, save in succession | Both edits land; no overwrite |
| 7 | Click the project list's `New model` affordance, name `manual_e2e`, format `stmx`, click Create | New entry appears in list; editor switches to the new file; verify file at `~/simlin-playground/manual_e2e.stmx` |
| 8 | Open `teacup.mdl` from list | Banner mentions sidecar |
| 9 | Edit + save | `teacup.sd.json` appears in list; tab switches to sidecar |
| 10 | `diff <(cat ~/simlin-playground/teacup.mdl) <(git show HEAD:test/test-models/samples/teacup/teacup.mdl)` | Identical |

## Phase 4: Git Status (AC2)

| Step | Action | Expected |
|------|--------|----------|
| 1 | `cd ~/simlin-playground && git init -b main && git add -A && git commit -m init && cd -` | Repo created |
| 2 | Refresh browser tab | All four project rows display "version controlled" chip |
| 3 | Edit `teacup.xmile` in the editor and save | Chip on `teacup.xmile` flips to "modified" within ~1s |
| 4 | In a terminal: `cd ~/simlin-playground && git add -A && git commit -m save` | After commit, chip flips back to clean state within ~1s |
| 5 | Move the directory: `mv ~/simlin-playground ~/no-git-dir` (after stopping the server), restart `simlin-serve ~/no-git-dir` | Same files surface as Untracked (gray) chip; Untracked warning aria-label visible |
| 6 | Verify with: `which git` then rename git out of PATH (`alias git=false` or rename binary) and restart server | Banner appears: "git unavailable"; click "Dismiss"; reload; banner stays dismissed for the session |

## Phase 5: AI / MCP Integration (AC5, AC6)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Configure your AI client (Claude Desktop or Claude Code CLI per `src/simlin-serve/README.md`) to call the MCP server with URL `http://127.0.0.1:7878/mcp`. From the AI tool surface, list tools | `CreateModel`, `EditModel`, `ListProjects`, `ReadModel`, `Simulate` advertised |
| 2 | Issue `ReadModel` for `teacup.xmile` from the AI client | Tool result includes a `model` field; not `is_error` |
| 3 | Issue `EditModel` adding aux `agent_added_aux` with equation `5` | Browser tab's editor refreshes within ~1s; toast "updated on disk" or refetch is silent |
| 4 | In the same browser, observe `agent_added_aux` in the editor | New aux visible |
| 5 | Issue `Simulate` for `teacup.xmile` | Tool returns `time` array and `variables.teacup_temperature` array |
| 6 | Issue `Simulate` with override `upsertStock { name: "Teacup Temperature", initialEquation: "10", outflows: [...] }` | First time-series value approximately 10.0 |
| 7 | Issue `CreateModel` with name `agent_created.sd.json` | File appears in browser project list |
| 8 | Verify the AI receives `simlin/projectChanged`, `simlin/projectFocused`, `simlin/selectionChanged`, `simlin/diagnosticsChanged` notifications during the session | Notifications printed by the AI client when the browser interacts with the model |

## Phase 6: Port Conflict (AC5.5)

| Step | Action | Expected |
|------|--------|----------|
| 1 | In one terminal: `target/release/simlin-serve --mcp-port 7878 ~/simlin-playground` | Starts |
| 2 | In another terminal: `target/release/simlin-serve --mcp-port 7878 ~/simlin-playground` | Exits non-zero with stderr containing `address already in use` and the hint to set `--mcp-port` |

## End-to-End: Multi-tab Live Editing (AC4.1, AC6.3)

Purpose: Validate per-variable LWW preserves both edits with concurrent browser tabs.

Steps:
1. Open the same UI URL in two browser tabs (tab A and tab B).
2. Tab A: select `teacup.xmile`, edit Stock 1's initial equation to `100`, save.
3. Tab B (without refresh): also select `teacup.xmile`, edit Stock 2's initial equation to `200`, save (expect 409 + auto-refetch + onConflict).
4. After the conflict resolution, Stock 1 should still show `100`, Stock 2 now `200`.
5. Both tabs should converge on the merged state via WS broadcast.

## End-to-End: External Editor Coexistence (AC4.2, AC4.3)

Purpose: Validate disk-edit merge preserves in-flight browser state.

Steps:
1. Open `~/simlin-playground/teacup.xmile` in the simlin-serve browser tab; pan and start editing Stock 1 but do not save.
2. In an external editor (vim, VS Code), open the same file, change Stock 2's initial equation, save.
3. Browser tab should display "model was updated on disk" toast.
4. Save Stock 1's edit. Verify both stocks retain their edits in the resulting file.

## End-to-End: SPA Bootstrap with Token (AC7.3)

Purpose: Validate launch token issuance, capture, sessionStorage, and constant-time compare.

Steps:
1. Start the binary and copy the printed UI URL.
2. Open the URL in a fresh browser private/incognito window.
3. Open DevTools - Application - Storage - Session Storage.
4. Verify a key matching `simlin-serve-launch-token` (or similar — see `web/src/launch-token.ts`) holds the token value.
5. Verify the URL bar's `?token=...` was removed.
6. Reload the page; verify the SPA continues to function without the URL token (the sessionStorage value is the source of truth).
7. Open a second browser window with a tampered URL (`?token=wrong`); verify subsequent API calls return 401 (visible in Network tab).
8. Connect a websocket using `wscat -c "ws://127.0.0.1:<port>/api/updates?token=wrong"`; expect 401 close.
9. Connect with `wscat -c "ws://127.0.0.1:<port>/api/updates"` (no token); expect 400.

## Human Verification Required

| Criterion | Why Manual | Steps |
|-----------|------------|-------|
| AC7.2 — default browser opens | Hands off to OS-level `open`/`xdg-open`/`start`, depends on default-browser registration; CI runners are headless so cannot exercise this | Phase 1 above: visually confirm default browser opens on macOS, Linux (with DISPLAY), Windows |
| AC7.4 — graceful headless behavior | Reproducing "no opener available" in CI is fragile across platforms; the URL-printing path is testable but the actual "no-opener present, server keeps running" must be eyeballed | Phase 2 above: `unset DISPLAY` (Linux), no-GUI ssh (macOS), Windows Server Core |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 | discovery_integration.rs::ac1_1, api_projects.rs::ac1_1, ProjectList.test.tsx, smoke.rs | Phase 3 step 1 (project list visible) |
| AC1.2 | discovery_integration.rs::ac1_2, api_projects.rs::ac1_2 | Phase 3 step 1 (sub/nested.xmile in list) |
| AC1.3 | discovery_integration.rs::ac1_3 | (none — purely server-side) |
| AC1.4 | api_save.rs::create_new_*, NewProjectButton.test.tsx, App.test.tsx | Phase 3 step 7 |
| AC1.5 | discovery_integration.rs::ac1_5 (cfg(unix)) | (none — Unix-only edge case) |
| AC2.1 | git_integration.rs::ac2_1, ProjectList.test.tsx | Phase 4 step 2 |
| AC2.2 | git_integration.rs::ac2_2, ProjectList.test.tsx | Phase 4 step 3 |
| AC2.3 | git_integration.rs::ac2_3, api_projects.rs::ac2_5, ProjectList.test.tsx | Phase 4 step 5 |
| AC2.4 | watcher_git.rs::git_commit_flips_registry_entry | Phase 4 steps 3-4 |
| AC2.5 | git.rs inline tests, api_projects.rs::ac2_5, App.test.tsx (banner+dismiss) | Phase 4 step 6 |
| AC3.1 | api_get_project.rs::ac3_1*, EditorHost.test.tsx | Phase 3 step 1 |
| AC3.2 | api_save.rs::save_xmile_*, writer.rs inline tests | Phase 3 steps 3-4 |
| AC3.3 | api_get_project.rs::ac3_3 | Phase 3 step 8 |
| AC3.4 | api_save.rs::save_mdl_creates_sidecar*, EditorHost.test.tsx | Phase 3 steps 8-10 |
| AC3.5 | api_save.rs + api_get_project.rs sidecar tests | Phase 3 step 9 |
| AC3.6 | api_save.rs::stale_version*, registry.rs concurrent test, EditorHost.test.tsx | Multi-tab step 3 |
| AC4.1 | api_save.rs::two_saves*, loro_doc.rs::concurrent_serial, mcp_registry_access.rs::*, e2e_mcp_browser.rs | Multi-tab + Phase 5 step 3 |
| AC4.2 | watcher_merge.rs::external_disk_edit | External-editor coexistence + Phase 3 step 5 |
| AC4.3 | watcher_merge.rs::browser_and_disk_edits | External-editor coexistence |
| AC4.4 | watcher_merge.rs::echo_suppression*, hashing.rs inline tests | (none — purely internal optimization) |
| AC5.1 | mcp/transport.rs inline + dual_port_smoke.rs | Phase 5 step 1 + Phase 1 step 1 |
| AC5.2 | mcp/server.rs inline tests + mcp_tool_surface.rs | Phase 5 step 1 |
| AC5.3 | mcp_tool_surface.rs (multiple), e2e_mcp_browser.rs, smoke.rs | Phase 5 steps 1-7 |
| AC5.4 | mcp_registry_access.rs (multiple), mcp/access.rs inline tests | Phase 5 step 4 |
| AC5.5 | dual_port_smoke.rs::port_conflict_* | Phase 6 |
| AC6.1 | ws_updates.rs::inbound_project_focused, EditorHost.test.tsx, mcp_tool_surface.rs | Phase 5 step 8 |
| AC6.2 | editor-selection-changed.test.ts, EditorHost.test.tsx, ws_updates.rs, mcp_tool_surface.rs | Phase 5 step 8 |
| AC6.3 | api_save.rs (User), mcp_registry_access.rs (Agent), watcher_merge.rs (Disk), mcp_tool_surface.rs (forwarder) | Phase 3 step 6 + Phase 5 step 3 |
| AC6.4 | diagnostics_events.rs (multiple), mcp_tool_surface.rs::diagnostics_changed | (covered by automated; visible if you create an invalid model) |
| AC7.1 | build_npm_packages.rs, serve_release_workflow.rs, smoke.rs (CI matrix) | (none — release-time concerns) |
| AC7.2 | (URL printing in smoke.rs) | Phase 1 (browser opens) |
| AC7.3 | token.rs inline tests, launch-token.test.ts, api.test.ts, ws_updates.rs (multi-test) | SPA Bootstrap |
| AC7.4 | (URL printing in smoke.rs) | Phase 2 |
