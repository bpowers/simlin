# simlin-mcp

MCP (Model Context Protocol) server exposing the Simlin simulation engine as tools for AI assistants.

<!-- Last reviewed: 2026-04-26 -->

## Architecture

This crate is a thin binary wrapper around `simlin-mcp-core`, which owns the entire MCP tool surface (tool implementations, output types, the rmcp `ServerHandler` impl). The binary contributes:

- **Build-time content embedding** -- `build.rs` substitutes `{PYSIMLIN_VERSION}` from `pysimlin.version` into `instructions.md` and `src/skills/pysimlin-basics.md`, writing processed files to `OUT_DIR`. Skill files without version placeholders are included verbatim from source via `include_str!`.
- **Stateless filesystem `ProjectAccess` impl** -- the `simlin-mcp-core` library is generic over `A: ProjectAccess`; this crate provides the binary-specific `FileSystemAccess` impl that re-reads/writes the file on every call (preserving the pre-rmcp wire semantics).
- **Stdio transport glue** -- `main.rs` constructs `SimlinMcpServer<FileSystemAccess>` with the embedded resources and hands it to rmcp's `ServiceExt::serve(stdio())`.

### Files

- `build.rs` -- Build script reading `pysimlin.version` and templating `instructions.md` + `pysimlin-basics.md` into `OUT_DIR`.
- `pysimlin.version` -- Single source of truth for the pysimlin version embedded in MCP content. Updated by `scripts/release-pysimlin.sh`.
- `src/main.rs` -- Binary entry point. Loads OUT_DIR-substituted content via `include_str!`, builds the `Vec<ResourceContent>` for the four skill resources, constructs `SimlinMcpServer<FileSystemAccess>`, and runs `serve(stdio()).await?.waiting().await`.
- `src/lib.rs` -- Library half (re-exports `pub mod access`). Keeps the lib + bin layout so integration tests under `tests/` can import `FileSystemAccess` without spawning the binary.
- `src/access.rs` -- `FileSystemAccess` impl of `ProjectAccess`. `open` uses `tokio::fs::read_to_string` + `simlin_mcp_core::open::open_project`; `save` serialises per `SourceFormat` and atomic-writes; `create` validates non-existence, ensures parent dir, atomic-writes. Rejects `.mdl` writes here (canonical error message preserved verbatim for backwards compat with existing `@simlin/mcp` clients). For `SourceFormat::SdaiJson`, regenerates the `relationships` field from `compute_link_polarities` + `generate_relationships` so saves match the pre-refactor behaviour.
- `src/instructions.md` -- Comprehensive instructions template embedded in the binary, covering tool usage, SD concepts, Vensim syntax, and workflow guidance. Contains `{PYSIMLIN_VERSION}` placeholder resolved at build time.
- `src/skills/` -- Four skill markdown files compiled into the binary as MCP resources:
  - `pysimlin-basics.md` -- Loading models, simulation, DataFrame access. Contains `{PYSIMLIN_VERSION}` placeholder resolved at build time.
  - `scenario-analysis.md` -- Parameter sweeps and intervention analysis.
  - `loop-dominance.md` -- Plotting behavior, annotating dominant periods.
  - `vensim-equation-syntax.md` -- MDL-to-XMILE mapping table.

### Tools

The actual tool implementations (`ReadModel`, `EditModel`, `CreateModel`) live in `../simlin-mcp-core/src/tools/` and are dispatched via rmcp's `#[tool]`/`#[tool_router]`/`#[tool_handler]` macros on `simlin_mcp_core::server::SimlinMcpServer`. See `../simlin-mcp-core/` for the per-tool input/output shape and behaviour notes.

## npm Distribution

Published to npm as `@simlin/mcp` with platform-specific binary packages following the Node optional-dependency pattern (same approach as esbuild, turbo, etc.).

- **Wrapper package**: `@simlin/mcp` -- entry point `bin/simlin-mcp.js` resolves the correct platform binary at runtime
- **Platform packages**: `@simlin/mcp-{darwin-arm64,linux-arm64,linux-x64,win32-x64}` -- each contains a single native binary in `bin/`
- **Version source of truth**: `Cargo.toml` -- `build-npm-packages.sh` reads it; CI validates tag matches
- **Release trigger**: push a `mcp-v*` tag; the `mcp-release.yml` workflow builds, publishes platform packages, then publishes the wrapper

### Supported Platforms

| npm package | Rust target | Build method |
|---|---|---|
| `@simlin/mcp-linux-x64` | `x86_64-unknown-linux-musl` | cargo-zigbuild |
| `@simlin/mcp-linux-arm64` | `aarch64-unknown-linux-musl` | cargo-zigbuild |
| `@simlin/mcp-win32-x64` | `x86_64-pc-windows-gnu` | cargo-zigbuild |
| `@simlin/mcp-darwin-arm64` | `aarch64-apple-darwin` | native (macOS runner) |

### Scripts

- `build-npm-packages.sh` -- generates platform `package.json` files in `npm/@simlin/mcp-*`
- `scripts/cross-build.sh` -- local cross-compilation via Docker + cargo-zigbuild (outputs to dist/)
- `Dockerfile.cross` -- toolchain image for cross-build.sh
- `scripts/release-mcp.sh <version>` (repo root) -- bumps Cargo.toml + npm package versions, runs tests, commits, and creates `mcp-v<version>` tag. Does not push
- `scripts/release-pysimlin.sh <version>` (repo root) -- updates `pysimlin.version`, commits, and creates `pysimlin-v<version>` tag. Does not push

## Version Management

The pysimlin version referenced in MCP instructions and skill resources is managed through build-time template substitution rather than hardcoded strings:

1. `pysimlin.version` contains the current version (e.g. `0.6.3`)
2. `build.rs` reads this file and substitutes `{PYSIMLIN_VERSION}` in `instructions.md` and `src/skills/pysimlin-basics.md`, writing processed output to `OUT_DIR`
3. `main.rs` includes the processed files from `OUT_DIR` via `include_str!`
4. Tests validate that the version in `pysimlin.version` matches the latest `pysimlin-v*` git tag and that substitution produced correct output

To update the pysimlin version reference: run `scripts/release-pysimlin.sh <version>`.

## Build / Test

```sh
cargo test -p simlin-mcp
cargo build -p simlin-mcp
```

Tests in this crate cover transport, filesystem access, and the npm release workflow:
- `tests/integration/file_system_access.rs` -- E2E coverage of `FileSystemAccess` (open/save/create + `.mdl` rejection).
- `tests/integration/stdio_smoke.rs` -- spawns the built binary, exchanges a JSON-RPC `initialize`, asserts the wire surface (server name, capabilities, instructions). The single smoke test replaces the deleted JSON-RPC dispatcher unit tests; rmcp owns those wire mechanics now.
- `tests/integration/build_npm_packages.rs`, `tests/integration/mcp_release_workflow.rs` -- CI/release validators (independent of the runtime).

Tool-level behaviour tests (success and validation-error paths for each tool) live in `../simlin-mcp-core/tests/`.

## Dependencies

Depends on `simlin-engine` for model types, file format parsing, error formatting (`simlin_engine::errors`), atomic file writes (`simlin_engine::io`), and SD-AI relationship regeneration (`compute_link_polarities` + `generate_relationships`). Depends on `simlin-mcp-core` for the tool surface and `ProjectAccess` trait. Depends on `rmcp` for the stdio transport. Does NOT depend on `libsimlin` (the C FFI crate).
