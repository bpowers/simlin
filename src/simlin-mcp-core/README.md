# simlin-mcp-core

Transport-agnostic core library for the Simlin Model Context Protocol (MCP)
server.  This crate hosts the tool implementations
(`read_model`, `edit_model`, `create_model`), format-detection helpers,
output types, the rmcp `ServerHandler` implementation, and the stateless
`FileSystemAccess` implementation of [`ProjectAccess`].

`simlin-mcp`, the thin stdio entry point for the `@simlin/mcp` npm package,
mounts this library.

The library is generic over a concrete `ProjectAccess` impl rather than
trait objects so rmcp's macro-generated dispatch sees a fully concrete
handler type.
