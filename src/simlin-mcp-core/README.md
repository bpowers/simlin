# simlin-mcp-core

Transport-agnostic core library for the Simlin Model Context Protocol (MCP)
server.  This crate hosts the shared tool implementations
(`read_model`, `edit_model`, `create_model`), format-detection helpers,
output types, and (in subsequent phases) the rmcp `ServerHandler`
implementation.

Two binaries mount this library over different transports:

- `simlin-mcp` -- thin stdio entry point for the `@simlin/mcp` npm package.
  Provides a stateless `FileSystemAccess` implementation of
  [`ProjectAccess`].
- `simlin-serve` (Phase 6) -- local-first HTTP server that mounts the same
  tool surface over rmcp's HTTP transport, backed by a
  `ProjectRegistry`-aware `ProjectAccess` implementation.

The library is generic over a concrete `ProjectAccess` impl rather than
trait objects so rmcp's macro-generated dispatch sees a fully concrete
handler type.
