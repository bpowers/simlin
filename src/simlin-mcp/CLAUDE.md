# simlin-mcp

MCP (Model Context Protocol) server exposing the Simlin simulation engine as tools for AI assistants.

<!-- Last reviewed: 2026-03-31 -->

## Architecture

- `build.rs` -- Build script that reads `pysimlin.version` and performs `{PYSIMLIN_VERSION}` template substitution on `instructions.md` and `skills/pysimlin-basics.md`, writing processed files to `OUT_DIR`
- `pysimlin.version` -- Single source of truth for the pysimlin version embedded in MCP content. Updated by `scripts/release-pysimlin.sh`
- `src/main.rs` -- Binary entry point. Loads build-processed `instructions.md` from `OUT_DIR` via `include_str!`, registers tools, and runs the async server loop
- `src/protocol.rs` -- Stdio JSON-RPC 2.0 MCP server (newline-delimited JSON). Handles six methods: initialize, ping, tools list, tools call, resources list, resources read. Advertises both tools and resources capabilities. `ServerConfig.instructions` is included in the initialize response
- `src/tool.rs` -- `Tool` trait, `TypedTool<I>` helper, `Registry`, and `input_schema_for<T>()` schema generator
- `src/resource.rs` -- Static resource registry for MCP resource listing and reading. Resources are compiled into the binary via `include_str!` (versioned files from `OUT_DIR`, others from source). `list()` returns all entries; `get(uri)` looks up by URI. Each `ResourceEntry` has metadata (uri, name, description, mime_type) and content
- `src/tools/` -- Tool implementations (see Tools section below)
- `src/tools/types.rs` -- Shared output types: `LoopDominanceSummary` (with 3-sig-fig importance rounding), `DominantPeriodOutput`, `ErrorOutput` (structured error detail with code/message/modelName/variableName/kind). `ErrorOutput` converts from `simlin_engine::errors::FormattedError`
- `src/instructions.md` -- Comprehensive instructions template embedded in the binary, covering tool usage, SD concepts, Vensim syntax, and workflow guidance. Contains `{PYSIMLIN_VERSION}` placeholder resolved at build time
- `src/skills/` -- Four skill markdown files compiled into the binary as MCP resources:
  - `pysimlin-basics.md` -- Loading models, simulation, DataFrame access. Contains `{PYSIMLIN_VERSION}` placeholder resolved at build time
  - `scenario-analysis.md` -- Parameter sweeps and intervention analysis
  - `loop-dominance.md` -- Plotting behavior, annotating dominant periods
  - `vensim-equation-syntax.md` -- MDL-to-XMILE mapping table

## Tools

### ReadModel

Reads a model file and returns a JSON snapshot enriched with loop dominance analysis. Supports XMILE (.stmx, .xmile), Vensim (.mdl), and Simlin JSON (.sd.json) formats.

Output shape: `{ model, time, loopDominance, dominantLoopsByPeriod, errors? }`. The `errors` field is omitted (not empty array) when the model has no errors. Each error has `code`, `message`, `modelName?`, `variableName?`, `kind` fields. Error codes are snake_case strings matching `ErrorCode::Display`, aligned with pysimlin.

### EditModel

Applies operations to an existing model file. Operations: `UpsertStock`, `UpsertFlow`, `UpsertAuxiliary`, `RemoveVariable`, `SetLoopName`. Upsert replaces the full variable definition (omitted optional fields default to empty).

Key behaviors:
- **Format-aware write-back**: Detects source format on open (`SourceFormat::Xmile`, `NativeJson`, `SdaiJson`) and writes back in the same format. `.mdl` files are parsed as XMILE internally but are read-only; EditModel rejects them with a clear error message
- **Atomic writes**: Uses `simlin_engine::io::atomic_write` for crash-safe file output
- **Error gate**: After patch application, runs compilation diagnostics. If errors are detected, returns a structured error response with `ErrorOutput` details instead of writing to disk
- **Diagram sync**: After successful variable operations (non-dry-run), regenerates layout via incremental or full layout depending on existing view state
- **SetLoopName**: Maps variable names to UIDs and writes `LoopMetadata` entries on the model. Loop names then surface in ReadModel's `loopDominance` output

Output shape: `{ projectPath, model, time, loopDominance, dominantLoopsByPeriod, dryRun }`

### CreateModel

Creates a new empty model file at the specified path.

## Format Detection

`open_project(path, contents)` in `src/tools/mod.rs` handles format detection:
- `.stmx`/`.xmile`/`.xml` -> XMILE parser
- `.mdl` -> Vensim parser (treated as XMILE format for write-back, but EditModel rejects .mdl)
- Other extensions (`.sd.json`, `.json`) -> content-based JSON detection: top-level `models` key = native Simlin JSON, `variables` key = SD-AI JSON

`resolve_model_name()` handles the "main" default: falls back to first model when no model is literally named "main".

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
- `scripts/release-mcp.sh <version>` (repo root) -- bumps Cargo.toml + npm package versions, runs tests, commits, and creates `mcp-v<version>` tag. Does not push
- `scripts/release-pysimlin.sh <version>` (repo root) -- updates `pysimlin.version`, commits, and creates `pysimlin-v<version>` tag. Does not push

## Version Management

The pysimlin version referenced in MCP instructions and skill resources is managed through build-time template substitution rather than hardcoded strings:

1. `pysimlin.version` contains the current version (e.g. `0.6.3`)
2. `build.rs` reads this file and substitutes `{PYSIMLIN_VERSION}` in `instructions.md` and `skills/pysimlin-basics.md`, writing processed output to `OUT_DIR`
3. `main.rs` and `resource.rs` include the processed files from `OUT_DIR`
4. Tests validate that the version in `pysimlin.version` matches the latest `pysimlin-v*` git tag and that substitution produced correct output

To update the pysimlin version reference: run `scripts/release-pysimlin.sh <version>`.

## Build / Test

```sh
cargo test -p simlin-mcp
cargo build -p simlin-mcp
```

## Dependencies

Depends on `simlin-engine` for model types, patch application, file format parsing, error formatting (`simlin_engine::errors`), and atomic file writes (`simlin_engine::io`). Does NOT depend on `libsimlin` (the C FFI crate). Tools that call `analyze_model` create a `SimlinDb` and `sync_from_datamodel` to provide the required salsa db and source project.
