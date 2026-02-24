# Human Test Plan: simlin-mcp

## Prerequisites

- Rust toolchain installed (nightly, per project config)
- Node.js 18+ installed
- Run `./scripts/dev-init.sh` from project root
- All automated tests pass: `cargo test -p simlin-mcp`

## Phase 1: Platform Detection and Binary Resolution (AC5.1)

| Step | Action | Expected |
|------|--------|----------|
| 1 | Open `src/simlin-mcp/bin/simlin-mcp.js` and inspect the `PLATFORM_MAP` object (line 18-35). | Map contains exactly 4 entries: `darwin-arm64`, `darwin-x64`, `linux-x64`, `win32-x64`. Each has `package` and `triple` fields. |
| 2 | Verify the unsupported-platform error path (lines 40-48). | When `platformKey` is not in `PLATFORM_MAP`, the script prints a descriptive error listing supported platforms and exits with code 1. |
| 3 | Run `node src/simlin-mcp/bin/simlin-mcp.js` without any native binary or platform package installed. | Script prints an error message including the current platform name and instructions for installing the platform package or building from source. Exits with code 1. |
| 4 | Build the native binary: `cargo build -p simlin-mcp`. Create the vendor directory and symlink: `mkdir -p src/simlin-mcp/vendor/$(rustc -vV | grep host | awk '{print $2}')` and `cp target/debug/simlin-mcp src/simlin-mcp/vendor/$(rustc -vV | grep host | awk '{print $2}')/simlin-mcp`. | Binary is placed at the expected vendor path. |
| 5 | Run `node src/simlin-mcp/bin/simlin-mcp.js` with the vendor binary in place. | The process starts and blocks waiting for stdin input (MCP server started). No error output. |

## Phase 2: Binary Spawning, stdio Forwarding, and Exit Code Propagation (AC5.3)

| Step | Action | Expected |
|------|--------|----------|
| 1 | With vendor binary set up from Phase 1 Step 4, pipe an initialize message to the wrapper: `echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}' | node src/simlin-mcp/bin/simlin-mcp.js` | A JSON-RPC response appears on stdout containing `"protocolVersion":"2025-11-25"` and `"serverInfo"` with `"name":"simlin-mcp"`. The process exits cleanly after stdin EOF. |
| 2 | Check the exit code of the previous command: `echo $?` | Exit code is 0. |
| 3 | Pipe a sequence of messages (initialize + ping) then close stdin: `printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"test","version":"1.0"},"capabilities":{}}}\n{"jsonrpc":"2.0","id":2,"method":"ping"}\n' | node src/simlin-mcp/bin/simlin-mcp.js` | Two JSON-RPC responses appear on stdout: (a) initialize response with id=1 and serverInfo, (b) ping response with id=2 and `"result":{}`. Process exits cleanly. |
| 4 | Start the wrapper in background: `node src/simlin-mcp/bin/simlin-mcp.js &`. Note the PID. Send it SIGTERM: `kill <PID>`. | The process terminates. No error messages on stderr. The wrapper forwards SIGTERM to the child binary. |
| 5 | Start the wrapper, send it SIGINT (Ctrl+C in an interactive terminal): `node src/simlin-mcp/bin/simlin-mcp.js` then press Ctrl+C. | Process terminates cleanly. The signal is forwarded to the child and both processes exit. |

## End-to-End: Full Model Lifecycle via MCP Protocol

**Purpose:** Validate the complete workflow of creating a model, editing it with variables forming a feedback loop, reading the analysis, and verifying loop dominance data -- all through the stdio JSON-RPC interface.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Build simlin-mcp: `cargo build -p simlin-mcp`. Start the server: `./target/debug/simlin-mcp`. Send the initialize message on stdin. | Response contains `protocolVersion` and `serverInfo`. |
| 2 | Send `tools/list` request: `{"jsonrpc":"2.0","id":2,"method":"tools/list"}` | Response lists 3 tools: ReadModel, EditModel, CreateModel. Each has `name`, `description`, and `inputSchema`. |
| 3 | Send a `tools/call` to CreateModel with a temp path (e.g., `/tmp/mcp-e2e-test.simlin.json`): `{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"CreateModel","arguments":{"projectPath":"/tmp/mcp-e2e-test.simlin.json"}}}` | Response has `isError: false`. Structured content contains `projectPath`, `modelName: "main"`, and default `simSpecs` (start=0, end=100, dt="1"). File exists at `/tmp/mcp-e2e-test.simlin.json`. |
| 4 | Send a `tools/call` to EditModel adding a stock, flow, and auxiliary forming a feedback loop: population stock with inflow "births", births flow with equation "population * birth_rate", birth_rate auxiliary with equation "0.03". | Response has `isError: false`. Structured content contains `model` with the three variables. `time` array has 101 entries (0 to 100 inclusive with dt=1). `loopDominance` and `dominantLoopsByPeriod` are present (may be empty if this simple loop does not trigger dominance detection). |
| 5 | Send a `tools/call` to ReadModel on the same path. | Response matches the model state: 1 stock (population), 1 flow (births), 1 auxiliary (birth_rate). Views are absent or empty. |
| 6 | Send a `tools/call` to EditModel with `dryRun: true` adding a second auxiliary. | Response shows the new auxiliary in the model snapshot. Reading the file from disk shows it does not contain the new auxiliary. |
| 7 | Close stdin (EOF). | Server exits cleanly. No error output. |
| 8 | Clean up: `rm /tmp/mcp-e2e-test.simlin.json` | File removed. |

## End-to-End: Error Handling Through the Wire

**Purpose:** Validate that protocol-level and tool-level errors are properly surfaced through the full stdio transport.

| Step | Action | Expected |
|------|--------|----------|
| 1 | Start the server. Send malformed JSON: `this is not json` | Response has `error.code == -32700` and `error.message` contains "parse error". |
| 2 | Send a request missing the jsonrpc field: `{"id":1,"method":"ping"}` | Response has `error.code == -32600`. |
| 3 | Send a request with an unknown method: `{"jsonrpc":"2.0","id":1,"method":"does_not_exist"}` | Response has `error.code == -32601`. |
| 4 | Send a `tools/call` for ReadModel with a nonexistent path: `{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"ReadModel","arguments":{"projectPath":"/does/not/exist.stmx"}}}` | Response has `result.isError == true`. Content text contains an error message about failing to read the file. |
| 5 | Send a `tools/call` for CreateModel at a path that already exists (reuse the e2e test file). | Response has `result.isError == true`. Content mentions "already exists". |

## Traceability

| Acceptance Criterion | Automated Test | Manual Step |
|----------------------|----------------|-------------|
| AC1.1 Initialize + Ping | `protocol.rs::test_async_initialize_and_ping` | E2E Lifecycle Step 1 |
| AC1.2 Sequential Requests | `protocol.rs::test_async_sequential_requests` | E2E Lifecycle Steps 1-6 |
| AC1.3 EOF Clean Shutdown | `protocol.rs::test_async_eof_clean_shutdown` | E2E Lifecycle Step 7 |
| AC2.1 Loop Dominance Non-empty | `analysis.rs::ac2_1_*`, `read_model.rs::ac2_1_*` | E2E Lifecycle Step 4 |
| AC2.2 Time/Importance Consistency | `analysis.rs::ac2_2_*`, `read_model.rs::ac2_2_*` | E2E Lifecycle Step 4 |
| AC2.3 Default modelName | `read_model.rs::ac2_3_default_model_name` | E2E Lifecycle Step 5 |
| AC2.4 Multi-format Support | `read_model.rs::ac2_4_xmile_format`, `ac2_4_simlin_json_format`, `ac2_4_vensim_mdl_format` | -- |
| AC2.5 Views Omitted | `analysis.rs::ac2_5_views_are_empty`, `read_model.rs::ac2_5_*` | E2E Lifecycle Step 5 |
| AC2.6 Broken Equations Graceful | `analysis.rs::ac2_6_*`, `read_model.rs::ac2_6_*` | -- |
| AC2.7 Missing File Error | `read_model.rs::ac2_7_missing_file_returns_error` | E2E Error Step 4 |
| AC3.1 CreateModel Defaults | `create_model.rs::test_create_model_success_default_specs` | E2E Lifecycle Step 3 |
| AC3.2 CreateModel Custom SimSpecs | `create_model.rs::test_create_model_custom_sim_specs` | -- |
| AC3.3 CreateModel Already Exists | `create_model.rs::test_create_model_already_exists` | E2E Error Step 5 |
| AC3.4 UpsertStock | `edit_model.rs::ac3_4_upsert_stock` | E2E Lifecycle Step 4 |
| AC3.5 UpsertFlow/Aux + GraphicalFunction | `edit_model.rs::ac3_5_upsert_flow_and_auxiliary` | -- |
| AC3.6 RemoveVariable | `edit_model.rs::ac3_6_remove_variable` | -- |
| AC3.7 SimSpecs Before Variables | `edit_model.rs::ac3_7_sim_specs_applied_before_variables` | -- |
| AC3.8 Response Shape | `edit_model.rs::ac3_8_response_shape` | E2E Lifecycle Step 4 |
| AC3.9 Dry-Run | `edit_model.rs::ac3_9_dry_run_does_not_write` | E2E Lifecycle Step 6 |
| AC3.10 Schema Exclusions | `edit_model.rs::ac3_10_schema_excludes_internal_fields` | -- |
| AC3.11 EditModel Missing File | `edit_model.rs::ac3_11_missing_file_returns_error` | E2E Error Step 4 |
| AC4.1 Tool Error isError:true | `protocol.rs::test_async_tool_error_returns_is_error_true` | E2E Error Steps 4-5 |
| AC4.2 Malformed JSON / Invalid Request | `protocol.rs::test_async_malformed_json_returns_parse_error`, `test_async_missing_jsonrpc_returns_invalid_request` | E2E Error Steps 1-2 |
| AC4.3 Unknown Method | `protocol.rs::test_async_unknown_method_returns_method_not_found` | E2E Error Step 3 |
| AC5.1 JS Platform Detection | -- | Phase 1 Steps 1-5 |
| AC5.2 Platform Package Fields | `tests/build_npm_packages.rs::ac5_2_platform_packages_have_correct_fields` | -- |
| AC5.3 Binary Spawn + Stdio | -- | Phase 2 Steps 1-5 |
