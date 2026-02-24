# simlin-mcp

MCP (Model Context Protocol) server exposing the Simlin simulation engine as tools for AI assistants.

## Architecture

- `src/protocol.rs` — Stdio JSON-RPC 2.0 MCP server (newline-delimited JSON)
- `src/tool.rs` — `Tool` trait, `TypedTool<I>` helper, `Registry`, and `input_schema_for<T>()` schema generator
- `src/tools/` — Tool implementations: `read_model`, `edit_model`, `create_model`

## Tool Registration Pattern

Tools are defined using `TypedTool<I>` where `I` implements `Deserialize + JsonSchema`. The JSON Schema for the tool input is automatically derived from the Rust type via `schemars`, so the full schema (including nested types like patch operations and variable definitions) is visible to MCP clients.

## Build / Test

```sh
cargo test -p simlin-mcp
cargo build -p simlin-mcp
```

## Dependencies

Depends on `simlin-engine` for model types, patch application, and file format parsing. Does NOT depend on `libsimlin` (the C FFI crate).
