# MCP npm Release -- Human Test Plan

Generated from test-requirements.md after automated coverage validation passed.

## Prerequisites

- Docker installed and running (for cross-build tests)
- Access to the GitHub repository with permission to push tags
- Access to npmjs.com with Trusted Publisher configured for the `@simlin` organization
- `cargo test -p simlin-mcp` passing (confirms all automated tests are green)
- Access to both an x64 Linux machine and an arm64 machine (for AC3.3)

## Phase 1: Cross-Build Script Verification

| Step | Action | Expected |
|------|--------|----------|
| 1.1 | On a Linux x64 host, run `cd src/simlin-mcp && bash scripts/cross-build.sh` | Script completes without error. Docker builds the toolchain image and produces binaries. |
| 1.2 | Verify output: run `ls -lh dist/` and check for three subdirectories | Three directories exist: `x86_64-unknown-linux-musl/`, `aarch64-unknown-linux-musl/`, `x86_64-pc-windows-gnu/` |
| 1.3 | Verify each binary exists: `file dist/x86_64-unknown-linux-musl/simlin-mcp`, `file dist/aarch64-unknown-linux-musl/simlin-mcp`, `file dist/x86_64-pc-windows-gnu/simlin-mcp.exe` | `file` reports the x64 Linux binary as ELF 64-bit x86-64 (statically linked), the arm64 binary as ELF 64-bit ARM aarch64, and the Windows binary as PE32+ executable x86-64 |
| 1.4 | Run the built-in smoke test: `echo '' \| timeout 2 dist/x86_64-unknown-linux-musl/simlin-mcp` | Binary loads and exits. Exit code is 0 or a timeout (exit 124), NOT a crash/segfault (exit 139) or permission error (exit 126). |
| 1.5 | (AC3.3) On an arm64 host (e.g., Apple Silicon Mac with Docker), run `bash scripts/cross-build.sh` | Same results as steps 1.1-1.3. The Dockerfile correctly detects `aarch64` via `uname -m` and downloads the arm64 Zig toolchain. |

## Phase 2: CI Workflow Trigger Verification

| Step | Action | Expected |
|------|--------|----------|
| 2.1 | Push a non-matching tag: `git tag test-ignore-tag && git push origin test-ignore-tag` | Navigate to the repository's Actions tab. Confirm NO "MCP npm Release" workflow run appears for this tag. |
| 2.2 | Remove the test tag: `git push origin :refs/tags/test-ignore-tag && git tag -d test-ignore-tag` | Tag is cleaned up from both local and remote. |
| 2.3 | Push a matching tag: `git tag mcp-v0.1.0 && git push origin mcp-v0.1.0` | Navigate to the repository's Actions tab. Confirm the "MCP npm Release" workflow starts. |
| 2.4 | Monitor the "Validate version" job | Job succeeds. Log shows `Cargo.toml version: 0.1.0` and `Tag version: 0.1.0` matching. |
| 2.5 | Monitor the 4 "Build" matrix jobs | All 4 jobs succeed: mcp-linux-x64, mcp-linux-arm64, mcp-win32-x64, mcp-darwin-arm64. Each uploads an artifact. |

## Phase 3: Publish Ordering and Registry Verification

| Step | Action | Expected |
|------|--------|----------|
| 3.1 | In the Actions run, observe the "Publish platform packages" job | It starts only after all 4 build jobs complete (its `needs` includes `build`). |
| 3.2 | Observe the "Publish @simlin/mcp" (wrapper) job | It starts only after "Publish platform packages" completes (its `needs` includes `publish-platform`). |
| 3.3 | After all jobs complete, run: `npm view @simlin/mcp-linux-x64@0.1.0` | Package exists on the registry with the correct version. |
| 3.4 | Run: `npm view @simlin/mcp@0.1.0 optionalDependencies` | Output lists all 4 platform packages (`@simlin/mcp-darwin-arm64`, `@simlin/mcp-linux-arm64`, `@simlin/mcp-linux-x64`, `@simlin/mcp-win32-x64`) each at version `0.1.0`. |

## Phase 4: Provenance and OIDC Verification

| Step | Action | Expected |
|------|--------|----------|
| 4.1 | Navigate to `https://www.npmjs.com/package/@simlin/mcp/v/0.1.0` | The package page displays a provenance badge (green checkmark or "Provenance" label in the sidebar). |
| 4.2 | Run: `npm audit signatures @simlin/mcp` | Audit reports valid signatures with no integrity errors. |
| 4.3 | Confirm no `NPM_TOKEN` secret is configured in the repository settings (Settings > Secrets and variables > Actions) | No secret named `NPM_TOKEN`, `NODE_AUTH_TOKEN`, or any `NPM`-prefixed secret exists. The publish succeeded purely via OIDC. |

## Phase 5: End-to-End Install and Run

| Step | Action | Expected |
|------|--------|----------|
| 5.1 | On a Linux x64 machine: `npm install -g @simlin/mcp@0.1.0` | Installation succeeds. npm downloads the wrapper package and the `@simlin/mcp-linux-x64` optional dependency. |
| 5.2 | Run: `which simlin-mcp` | Prints a path (e.g., `/usr/local/bin/simlin-mcp`). |
| 5.3 | Run: `echo '{"jsonrpc":"2.0","method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}},"id":1}' \| simlin-mcp` | The binary starts, reads the JSON-RPC message from stdin, and prints an `initialize` response to stdout containing `serverInfo` and `capabilities`. |
| 5.4 | On a macOS arm64 machine: repeat steps 5.1-5.3, replacing the expected platform package with `@simlin/mcp-darwin-arm64` | Same results: installation succeeds, binary is found, and responds to the initialize message. |
| 5.5 | On a Windows x64 machine: `npm install -g @simlin/mcp@0.1.0` then send the same JSON-RPC initialize message | Installation succeeds with `@simlin/mcp-win32-x64`. Binary runs as `simlin-mcp.exe`. |

## Full Release Cycle (End-to-End)

1. Ensure `cargo test -p simlin-mcp` passes locally (all automated tests green).
2. Push tag `mcp-v0.1.0` to the remote.
3. Observe the GitHub Actions workflow triggers (AC1.1), builds all 4 platforms (AC1.2-AC1.5), publishes platform packages first (AC2.1), then publishes the wrapper (AC2.2).
4. Verify provenance badge on npmjs.com (AC2.3) and confirm no NPM_TOKEN secret was used (AC2.4, AC5.3).
5. On a clean machine, `npm install -g @simlin/mcp@0.1.0` and send a JSON-RPC initialize message to the binary (AC2.5).
6. Confirm `npm view @simlin/mcp@0.1.0` shows all 4 optional dependencies at the correct version (AC2.2).

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 Tag triggers workflow | `mcp_release_workflow.rs::ac1_1_tag_trigger_is_mcp_v_star` | Phase 2, step 2.3 |
| AC1.2 Linux x64 musl | `mcp_release_workflow.rs::ac1_2_to_1_5_build_matrix_has_all_four_targets` | -- |
| AC1.3 Linux arm64 musl | `mcp_release_workflow.rs::ac1_2_to_1_5_build_matrix_has_all_four_targets` | -- |
| AC1.4 Windows x64 PE | `mcp_release_workflow.rs::ac1_2_to_1_5_build_matrix_has_all_four_targets` | -- |
| AC1.5 macOS arm64 | `mcp_release_workflow.rs::ac1_2_to_1_5_build_matrix_has_all_four_targets` | -- |
| AC1.6 Non-matching tags ignored | `mcp_release_workflow.rs::ac1_1_tag_trigger_is_mcp_v_star` | Phase 2, steps 2.1-2.2 |
| AC2.1 Platform before wrapper | `mcp_release_workflow.rs::ac2_1_publish_wrapper_needs_publish_platform` | Phase 3, steps 3.1-3.2 |
| AC2.2 Correct optionalDeps | -- | Phase 3, step 3.4 |
| AC2.3 Provenance | `mcp_release_workflow.rs::ac2_3_all_npm_publish_commands_have_provenance` | Phase 4, step 4.1 |
| AC2.4 OIDC (no stored token) | `mcp_release_workflow.rs::ac5_3_no_npm_token_in_workflow` | Phase 4, step 4.3 |
| AC2.5 Execute permission | `mcp_release_workflow.rs::ac2_5_chmod_for_non_windows_binaries` | Phase 5, step 5.3 |
| AC3.1 cross-build produces 3 binaries | -- | Phase 1, steps 1.1-1.3 |
| AC3.2 Linux x64 binary runs | -- | Phase 1, step 1.4 |
| AC3.3 Works on x64 and arm64 | -- | Phase 1, steps 1.1 + 1.5 |
| AC4.1 darwin-x64 removed | `build_npm_packages.rs::ac4_platform_packages_have_correct_fields` | -- |
| AC4.2 Windows triple is gnu | `build_npm_packages.rs::ac4_2_js_launcher_windows_triple` | -- |
| AC4.3 Exactly 4 platform packages | `build_npm_packages.rs::ac4_platform_packages_have_correct_fields` | -- |
| AC4.4 publishConfig and repository | `build_npm_packages.rs::ac4_platform_packages_have_correct_fields` + `ac4_4_wrapper_package_json_has_publish_config` | -- |
| AC5.1 Build jobs contents:read | `mcp_release_workflow.rs::ac5_1_top_level_permissions_contents_read` | -- |
| AC5.2 Only publish jobs get id-token | `mcp_release_workflow.rs::ac5_2_only_publish_jobs_have_id_token_write` | -- |
| AC5.3 No stored npm tokens | `mcp_release_workflow.rs::ac5_3_no_npm_token_in_workflow` | -- |
