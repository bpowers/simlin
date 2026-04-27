# simlin-mcp-core

Transport-agnostic core library shared by every Simlin MCP server.

<!-- Last reviewed: 2026-04-26 -->

## Purpose

Owns the MCP tool surface (`ReadModel`, `EditModel`, `CreateModel`) as async free functions parameterised by a [`ProjectAccess`] backing store, plus the rmcp `ServerHandler` that wires those functions into MCP. Two binaries mount this library against different transports and storage strategies:

- `simlin-mcp` -- stdio entry point for the `@simlin/mcp` npm package; uses a stateless `FileSystemAccess` impl that re-reads the file on every call.
- `simlin-serve` -- HTTP host inside the `@simlin/serve` npm package; uses a `RegistryAccess` impl backed by an in-memory `LoroDoc` plus optimistic-lock versioning, and adds its own `ListProjects` and `Simulate` tools alongside the three reused ones.

The library is generic over a concrete `A: ProjectAccess` (not `dyn`) so rmcp's `tool_router` macro sees a fully concrete handler type. Native async-fn-in-trait (AFIT) avoids the heap allocation `async-trait` would force.

## Files

- `src/lib.rs` -- Module declarations + `pub use` re-exports for the stable surface (`ProjectAccess`, `OpenedProject`, `AccessError`, `SimlinMcpServer`, `ResourceContent`, `LoopDominanceSummary`, `DominantPeriodOutput`, `ErrorOutput`, `SourceFormat`).
- `src/access.rs` -- The `ProjectAccess` trait and `OpenedProject` struct. `version` is the optimistic-lock token: stateless impls return `0`, registry-backed impls return their monotonically-increasing counter.
- `src/errors.rs` -- `AccessError` (NotFound / IoError / ParseError / VersionMismatch / WriteError / Validation). The `Validation` variant carries `Vec<ErrorOutput>` so the wire shape stays identical to the pre-refactor `simlin-mcp` server.
- `src/types.rs` -- Wire-format types preserved verbatim from the pre-rmcp binary (`SourceFormat`, `LoopDominanceSummary`, `DominantPeriodOutput`, `ErrorOutput`) plus `build_empty_project` and `build_empty_project_with_specs` shared with the new-project HTTP route in `simlin-serve` for byte-identical create output.
- `src/open.rs` -- Format-detection + parsing helpers (`open_project`, `resolve_model_name`, `ensure_variable_uids`). I/O-free: callers pass already-loaded bytes.
- `src/tools/` -- The three reused tools (`read_model.rs`, `edit_model.rs`, `create_model.rs`) as async free functions taking `&impl ProjectAccess`. Exposed input/output types use `#[serde(rename_all = "camelCase")]` and curate out engine-internal fields (`uid`, `compat`, `aiState`).
- `src/server.rs` -- `SimlinMcpServer<A: ProjectAccess>` rmcp `ServerHandler` impl with the three `#[tool]` macros plus `list_resources` and `read_resource`. `version` is plumbed in by the binary so `serverInfo.version` reflects the binary's `CARGO_PKG_VERSION`, not the library's.
- `src/test_support.rs` -- `#[doc(hidden)]` `TestFileSystemAccess` shared by integration tests so each test suite doesn't reimplement the same FS-backed `ProjectAccess`.

## Contracts

- **`ProjectAccess` trait** -- the stable surface every transport mounts against. Production callers use `&Path` keys; backends interpret that as a filesystem path or registry key. `expected_version: Option<u64>` on `save` is the optimistic-lock token (`None` = skip the check). Trait methods use AFIT (`-> impl Future + Send`) so callers always know `A` statically and avoid `async-trait`'s allocation.
- **`SimlinMcpServer<A>` is `Clone`** -- rmcp's streamable-HTTP factory expects `Self: Clone`. Internal state lives behind `Arc` so cloning is cheap.
- **Tool wire shape is byte-identical to pre-refactor `simlin-mcp`** -- existing `@simlin/mcp` clients render success and error responses verbatim. The `error` string in `AccessError::Validation`'s structured output is `"edit introduces compilation errors"`; that exact phrase must not change.
- **`build_empty_project` is the single source of truth for empty-project shape** -- both the MCP `CreateModel` tool and the equivalent HTTP create route in `simlin-serve` go through it so the parity integration test keeps passing.
- **`ErrorOutput.code` strings come from `ErrorCode`'s `Display` impl** -- a regression test in `types.rs` locks the snake_case rendering down. pysimlin derives the same codes via `SimlinErrorCode`; both surfaces stay aligned.

## Dependencies

- Depends on `simlin-engine` for parsing, diagnostics, and the `datamodel::Project` shape.
- Depends on `rmcp` (server + macros + schemars features) for the `ServerHandler` trait and `#[tool_router]` / `#[tool_handler]` macros.
- Depends on `tokio` (rt-multi-thread + macros + fs) because the trait methods are `async` and `read_model.rs` etc. await the access impl.
- Used-by: `simlin-mcp` (binary), `simlin-serve` (library + binary). No other crate consumes it.

## Build / Test

```sh
cargo test -p simlin-mcp-core
```

Tests are split between unit tests (in-source `#[cfg(test)] mod tests`) and integration tests under `tests/`:
- `create_model_e2e.rs`, `edit_model_e2e.rs`, `read_model_e2e.rs` -- per-tool E2E coverage against `TestFileSystemAccess`.
- `server.rs` -- `SimlinMcpServer` happy paths for `get_info`, `list_resources`, and `read_resource`.
- `tool_dispatch.rs` -- end-to-end rmcp tool dispatch over an in-memory transport.
