# simlin-mcp

MCP (Model Context Protocol) server exposing the Simlin simulation engine as tools for AI assistants.

## Architecture

- `src/protocol.rs` — Stdio JSON-RPC 2.0 MCP server (newline-delimited JSON)
- `src/tool.rs` — `Tool` trait, `TypedTool<I>` helper, `Registry`, and `input_schema_for<T>()` schema generator
- `src/tools/` — Tool implementations: `read_model`, `edit_model`, `create_model`

## Tool Registration Pattern

Tools are defined using `TypedTool<I>` where `I` implements `Deserialize + JsonSchema`. The JSON Schema for the tool input is automatically derived from the Rust type via `schemars`, so the full schema (including nested types like patch operations and variable definitions) is visible to MCP clients.

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

## Build / Test

```sh
cargo test -p simlin-mcp
cargo build -p simlin-mcp
```

## Dependencies

Depends on `simlin-engine` for model types, patch application, and file format parsing. Does NOT depend on `libsimlin` (the C FFI crate). Tools that call `analyze_model` create a `SimlinDb` and `sync_from_datamodel` to provide the required salsa db and source project.
