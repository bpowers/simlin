# simlin-mcp-core

Transport-agnostic core library shared by every Simlin MCP server.

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
- `src/open.rs` -- Format-detection + parsing helpers (`open_project`, `resolve_model_name`). I/O-free: callers pass already-loaded bytes. `ensure_variable_uids` is private: `open_project` is the only caller, and running it is part of what opening a project means rather than a step a caller may skip.
- `src/fs_access.rs` -- `FileSystemAccess`, the stateless filesystem `ProjectAccess` impl. Used by the `simlin-mcp` binary (re-exported there as `simlin_mcp::access::FileSystemAccess`) AND by this crate's integration suites (`test_support::TestFileSystemAccess` is a type alias for it). It lives here rather than in the binary so the tests exercise the shipping impl -- a hand-maintained near-copy drifts at exactly the points where this file is non-trivial (the `.mdl` write rejection and the SD-AI `relationships` regeneration on save), so a test saving through a copy proves something about a simpler function than the one that ships. The `.mdl` guard here is defense in depth: `tools::edit_model` rejects a `.mdl` path before `save` is reached, so the arm is unreachable through the MCP tool surface but still owed to any direct `ProjectAccess` caller.
- `src/tools/` -- The three reused tools (`read_model.rs`, `edit_model.rs`, `create_model.rs`) as async free functions taking `&impl ProjectAccess`. Exposed types use `#[serde(rename_all = "camelCase")]`; the curated *input* types deliberately exclude engine-internal fields (`uid`, `compat`, `aiState`), while both tool outputs embed the full engine `json::Model`, which serializes `uid`/`compat` when populated. `read_model`/`edit_model` both unconditionally run LTM loop analysis (`analysis::analyze_model`), so they (1) carry an `analysisError` field that surfaces the actionable compile error when a model can't be compiled for LTM -- most notably the GH #486 Euler guidance -- instead of returning a silent empty `loopDominance` (GH #660), and (2) collect their `collect_all_diagnostics` passes with LTM transiently enabled via the shared `simlin_engine::db::LtmEnabledGuard` (the same guard libsimlin's `simlin_project_get_errors` uses for GH #466), surfacing the LTM auto-flip-to-discovery advisory and synthetic-fragment compile-failure warnings in a model-scoped `warnings` field that was previously dropped entirely because the collection ran with `ltm_enabled=false` (GH #662). `edit_model` enables LTM on both its pre- and post-edit diagnostic passes so the new-error gate compares like-with-like (the LTM advisories are Warnings, and the #486 rejection rides the assemble path, so neither affects the Error-severity gate).
- `src/server.rs` -- `SimlinMcpServer<A: ProjectAccess>` rmcp `ServerHandler` impl with the three `#[tool]` macros plus `list_resources` and `read_resource`. `version` is plumbed in by the binary so `serverInfo.version` reflects the binary's `CARGO_PKG_VERSION`, not the library's.
- `src/test_support.rs` -- `#[doc(hidden)]` integration-test fixtures, gated behind the `test-support` feature so they are not compiled into shipped binaries (the crate takes a self dev-dependency enabling the feature so `tests/` still resolves them). `TestFileSystemAccess` is a type alias for the production `fs_access::FileSystemAccess`, never a second implementation (see its rustdoc for why); `chain_scc_project_json` builds the oversized-SCC model the LTM auto-flip warning tests need.

## Tool Design

The consumers of this tool surface are agents, and a tool result is context for the consuming agent's next decision -- design every result for that role. A capability is only usable when the agent can traverse the complete loop: discover the tool, recognize when it applies, invoke it correctly, interpret the result, recover from failure, and verify the effect. Concretely:

- **Quiet success, bounded results.** A successful call returns the data needed for the next decision, not a transcript of the work. Large payloads (full model JSON, long loop lists) should be curated -- the edit/create *input* types deliberately exclude engine-internal fields (`uid`, `compat`, `aiState`) so agents never have to supply them. The outputs currently embed the full engine `json::Model` (which serializes `uid`/`compat` when populated), so a new output payload does not inherit that curation for free -- apply it deliberately.
- **Errors name the violated invariant and the repair.** "edit introduces compilation errors" plus the structured `ErrorOutput` list tells the agent what rule was broken and where to fix it; a bare failure would leave the agent guessing at hidden state. New error paths should meet that bar (the GH #486 Euler guidance in `analysisError` is the model to follow: it converts a silent empty result into an actionable explanation).
- **Advisory context rides the result.** Warnings and analysis errors surface conditions the agent cannot otherwise observe (LTM auto-flip, synthetic-fragment failures). When adding a tool, ask what the agent would need to relay through a human today and put that in the result instead.
- **Tool descriptions advertise what and why.** The schema-visible name and description are how an agent selects the tool at the moment of need; detail belongs in the result, not the catalog.

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

Tests are split between unit tests (in-source `#[cfg(test)] mod tests`) and integration tests in the single consolidated `tests/integration` harness (one binary instead of one per file; see GH #706 -- add new integration tests as a `mod` in `tests/integration/main.rs`):
- `create_model_e2e.rs`, `edit_model_e2e.rs`, `read_model_e2e.rs` -- per-tool E2E coverage against `TestFileSystemAccess`.
- `server.rs` -- `SimlinMcpServer` happy paths for `get_info`, `list_resources`, and `read_resource`.
- `tool_dispatch.rs` -- end-to-end rmcp tool dispatch over an in-memory transport.
