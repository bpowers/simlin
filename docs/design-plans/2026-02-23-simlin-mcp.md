# Simlin MCP Server Design

## Summary

This design evolves the existing synchronous simlin-mcp prototype into a production-quality async MCP server that exposes Simlin's system dynamics simulation engine to LLM-based AI assistants. The core strategy has three parts: (1) add an `analyze_model()` API to simlin-engine that bundles compilation, simulation, feedback loop detection, and dominant-period calculation into a single call; (2) replace the prototype's blocking stdio loop with an async tokio architecture using three cooperating tasks (stdin reader, message processor, stdout writer) connected by MPSC channels, behind a `Transport` trait that allows a future HTTP/SSE implementation with no changes to tool logic; and (3) wrap the compiled binary in an npm package using platform-specific optional dependencies so that `npx @simlin/mcp` works on macOS, Linux, and Windows without requiring a Rust toolchain.

The three MCP tools -- CreateModel, ReadModel, and EditModel -- match the input/output shapes of the existing Go implementation so that LLM tool descriptions remain compatible. EditModel exposes a curated subset of variable fields (name, equation, units, documentation, graphical functions) while hiding engine-internal fields like uid and compat, keeping the schema focused on what an LLM should plausibly set. ReadModel and EditModel responses include loop dominance analysis alongside the model snapshot, giving the LLM both structural and dynamic feedback after every change.

## Definition of Done

Production-quality Rust MCP server (`simlin-mcp`) with async tokio transport, three tools (CreateModel, ReadModel, EditModel) matching the Go `praxis/cmd/simlin-mcp` input/output shapes, curated LLM-facing operation types, and an npm package (`@simlin/mcp`) for cross-platform distribution via `npx`.

Specifically:

1. **Async tokio MCP stdio protocol** -- separate stdin reader, message processor, and stdout writer tasks communicating via channels (codex-rs pattern), with a transport abstraction that accommodates future HTTP/SSE.

2. **Three MCP tools with Go-matching shapes:**
   - **CreateModel**: `{projectPath, simSpecs?}` returns `{projectPath, simSpecs, modelName}`
   - **ReadModel**: `{projectPath, modelName?}` returns `{model, time, loopDominance, dominantLoopsByPeriod}`
   - **EditModel**: `{projectPath, modelName?, dryRun?, simSpecs?, operations?}` returns ReadModel fields + `{dryRun}`

3. **LLM-optimized edit operations** -- MCP-facing types expose only fields an LLM should set (name, equation, units, documentation, graphicalFunction, arrayedEquation, inflows/outflows, nonNegative, canBeModuleInput, isPublic). Internal fields (uid, compat, aiState) are excluded from the schema.

4. **MCP-standard error handling** -- hard errors use `isError: true` on the MCP `CallToolResult`; success responses may include diagnostics.

5. **npm package** (`@simlin/mcp`) with platform-specific binaries for macOS (arm64/x86), Linux (x64), and Windows (x64).

**Out of scope:** LLM-based loop naming, in-memory project caching, HTTP/SSE transport implementation, CI/CD pipeline for publishing.

## Acceptance Criteria

### simlin-mcp.AC1: Async Transport
- **simlin-mcp.AC1.1 Success:** Server starts, completes MCP initialize handshake, and responds to ping over stdio
- **simlin-mcp.AC1.2 Success:** Server processes multiple sequential JSON-RPC requests without restarting
- **simlin-mcp.AC1.3 Success:** Server shuts down cleanly when stdin reaches EOF (no hanging tasks or zombie processes)

### simlin-mcp.AC2: ReadModel Tool
- **simlin-mcp.AC2.1 Success:** ReadModel returns model snapshot with loop dominance data for a model with known feedback loops
- **simlin-mcp.AC2.2 Success:** ReadModel returns time array, per-loop importance arrays, and dominant periods consistent with simulation results
- **simlin-mcp.AC2.3 Success:** ReadModel defaults modelName to "main" when omitted
- **simlin-mcp.AC2.4 Success:** ReadModel opens XMILE (.stmx/.xmile), Vensim (.mdl), and Simlin JSON formats
- **simlin-mcp.AC2.5 Success:** ReadModel omits views from the returned model snapshot
- **simlin-mcp.AC2.6 Success:** ReadModel returns model snapshot with empty loop arrays when simulation fails (equation errors)
- **simlin-mcp.AC2.7 Failure:** ReadModel returns isError when file does not exist

### simlin-mcp.AC3: CreateModel and EditModel Tools
- **simlin-mcp.AC3.1 Success:** CreateModel creates a `.simlin.json` file with "main" model and default sim specs (start=0, end=100, dt=1)
- **simlin-mcp.AC3.2 Success:** CreateModel accepts custom simSpecs and uses them
- **simlin-mcp.AC3.3 Failure:** CreateModel returns isError when target file already exists
- **simlin-mcp.AC3.4 Success:** EditModel applies upsertStock with name, initialEquation, and optional fields (units, documentation, inflows, outflows)
- **simlin-mcp.AC3.5 Success:** EditModel applies upsertFlow and upsertAuxiliary with name, equation, and optional graphicalFunction
- **simlin-mcp.AC3.6 Success:** EditModel applies removeVariable by name
- **simlin-mcp.AC3.7 Success:** EditModel applies simSpecs changes before variable operations in the same request
- **simlin-mcp.AC3.8 Success:** EditModel returns refreshed model snapshot and loop dominance data after applying changes
- **simlin-mcp.AC3.9 Success:** EditModel dry-run validates changes without writing to disk
- **simlin-mcp.AC3.10 Edge:** EditModel JSON schema does not contain uid, compat, or aiState fields
- **simlin-mcp.AC3.11 Failure:** EditModel returns isError when target file does not exist

### simlin-mcp.AC4: Error Handling
- **simlin-mcp.AC4.1 Success:** Tool execution errors return CallToolResult with isError=true and descriptive text content
- **simlin-mcp.AC4.2 Success:** Malformed JSON-RPC requests return standard JSON-RPC error codes (-32700, -32600)
- **simlin-mcp.AC4.3 Success:** Unknown method calls return JSON-RPC method-not-found error (-32601)

### simlin-mcp.AC5: npm Package
- **simlin-mcp.AC5.1 Success:** JS entry point detects platform and resolves the correct platform-specific binary
- **simlin-mcp.AC5.2 Success:** Platform packages have correct `os` and `cpu` fields in package.json
- **simlin-mcp.AC5.3 Success:** `node bin/simlin-mcp.js` spawns the native binary with stdin/stdout/stderr forwarding and exit code propagation

## Glossary

- **System dynamics (SD)**: A simulation methodology for modeling systems with feedback loops, stocks (accumulators), flows (rates of change), and auxiliary variables, typically using differential equations integrated over time.
- **Stock-and-flow model**: The core representation in SD: stocks accumulate quantities over time; flows control rates of accumulation; auxiliaries compute intermediate values. Together they form causal loop structures.
- **MCP (Model Context Protocol)**: An open protocol by Anthropic for connecting AI assistants to external tools and data sources. It defines a standard way to advertise tool schemas, accept tool calls, and return structured results.
- **JSON-RPC 2.0**: A stateless remote procedure call protocol encoded in JSON, used as the wire format for MCP. Each request has a method, params, and id; responses carry a result or error.
- **XMILE**: An XML-based interchange format for system dynamics models (IEEE/OASIS standard). Simlin reads `.xmile` and `.stmx` (Stella's variant) files.
- **Vensim / `.mdl`**: A commercial SD modeling tool. Simlin's engine includes a native Rust parser for Vensim's `.mdl` text format.
- **Loop dominance**: An analytical technique (Loops That Matter / LTM) that determines which feedback loops are most influential at each point in a simulation. Importance scores are derived from synthetic instrumentation variables injected before simulation.
- **Dominant period**: A contiguous time interval during which the same set of feedback loops dominates system behavior. Calculated by grouping consecutive timesteps with identical dominant loop sets.
- **LTM (Loops That Matter)**: The specific algorithm implemented in simlin-engine for loop detection, polarity analysis, and importance scoring. It augments the model with synthetic variables, simulates, then scores each loop's contribution.
- **tokio**: An async runtime for Rust providing event-driven I/O, task scheduling, timers, and channel primitives. Used here for non-blocking stdio and future HTTP transport.
- **MPSC channel**: A multi-producer, single-consumer message queue from tokio. The design uses a bounded channel (stdin to processor) for backpressure and an unbounded channel (processor to stdout) to avoid blocking the processor on slow output.
- **`spawn_blocking`**: A tokio function that runs synchronous, CPU-bound work on a dedicated thread pool without blocking the async runtime. Relevant for future HTTP/SSE transport where simulation must not stall request handling.
- **schemars**: A Rust crate that derives JSON Schema definitions from Rust types. Used here to auto-generate MCP tool input schemas from the `Deserialize` structs.
- **`CallToolResult`**: The MCP response type for tool invocations. Contains content blocks (text, images, etc.) and an `isError` boolean flag distinguishing tool-level failures from protocol errors.
- **SimSpecs**: Simulation specification parameters -- start time, end time, and dt (timestep size) -- that control how the simulation integrates over time.
- **Graphical function**: A lookup table (piecewise-linear function) that maps one variable to another via (x, y) point pairs, used when a relationship is empirical rather than algebraic.
- **`optionalDependencies`**: An npm `package.json` field listing packages that are installed if available but whose absence is not an error. Used here so that only the platform-matching binary package is installed while others are silently skipped.
- **codex-rs pattern**: The npm distribution pattern used by OpenAI's Codex CLI: a main JS package with platform-specific binary packages as optional dependencies, and a JS entry point that detects `process.platform`/`process.arch` to resolve the correct binary at runtime.
- **Dry run**: An EditModel mode (`dryRun: true`) that validates and simulates proposed changes without writing them to disk, allowing an LLM to preview the effect of edits before committing.
- **`TypedTool<I>`**: A generic helper in the existing prototype that wraps a handler function with automatic JSON Schema derivation and input deserialization, reducing per-tool boilerplate.

## Architecture

### Approach

Engine-enriched async architecture: simlin-engine provides a high-level `analyze_model()` API that bundles compilation, simulation, loop detection, and dominant period calculation. simlin-mcp is an async tokio MCP server with thin tool handlers that orchestrate file I/O and call the engine. An npm package wraps the binary for cross-platform distribution.

### Layers

```
@simlin/mcp (npm)                -- JS wrapper resolving platform binary
  simlin-mcp (Rust binary)      -- async transport + MCP protocol + tool handlers
    simlin-engine                -- compilation, simulation, loop analysis
```

### Async Transport

Three tokio tasks communicate via MPSC channels:

```
stdin task --(bounded channel)--> processor task --(unbounded channel)--> stdout task
```

- **stdin task**: reads lines from `tokio::io::stdin()`, pushes to bounded MPSC (capacity ~128). Exits on EOF, propagating shutdown via channel close.
- **stdout task**: receives from unbounded MPSC, writes newline-delimited JSON to `tokio::io::stdout()`. Unbounded because stdout backpressure should not block the processor.
- **processor task**: receives messages from stdin channel, dispatches via protocol handler, sends responses to stdout channel. Tool calls execute synchronously within this task (simulation is CPU-bound).

A `Transport` trait abstracts the message source/sink:

```rust
trait Transport {
    async fn recv(&mut self) -> Option<String>;
    async fn send(&mut self, message: String) -> Result<()>;
}
```

`StdioTransport` implements this trait. A future `HttpTransport` would implement the same trait, keeping the processor and all tool logic identical.

**Shutdown**: stdin EOF closes the incoming channel. Processor sees `None` from `recv()`, finishes in-flight work, drops the outgoing sender. stdout task drains remaining messages and exits.

### MCP Protocol

JSON-RPC 2.0 over newline-delimited JSON (same as current prototype). Protocol dispatch routes:
- `initialize` -- server capabilities and version handshake
- `ping` -- health check
- `tools/list` -- returns tool definitions with auto-derived JSON schemas
- `tools/call` -- dispatches to tool handler by name

Tool execution errors return `CallToolResult { isError: true }` with a descriptive text content block. JSON-RPC errors are reserved for protocol failures (malformed JSON, unknown method).

### Tool Framework

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn call(&self, input: Value) -> Result<Value>;
}
```

`TypedTool<I>` generic helper auto-derives JSON schemas via schemars from input type `I: JsonSchema + DeserializeOwned`. Registry holds `Vec<Box<dyn Tool>>` for dispatch.

### Tool Contracts

**CreateModel**:
- Input: `{ projectPath: string, simSpecs?: SimSpecs }`
- Output: `{ projectPath: string, simSpecs: SimSpecs, modelName: string }`
- Creates a new `.simlin.json` file with a single "main" model. Errors if file exists. Defaults: start=0, end=100, dt=1.

**ReadModel**:
- Input: `{ projectPath: string, modelName?: string }`
- Output: `{ model: Model, time: number[], loopDominance: LoopDominanceSummary[], dominantLoopsByPeriod: DominantPeriod[] }`
- Opens file (XMILE, Vensim, or Simlin JSON), calls `analyze_model()`, returns model snapshot with loop analysis. `modelName` defaults to "main". Views omitted from model output.

**EditModel**:
- Input: `{ projectPath: string, modelName?: string, dryRun?: bool, simSpecs?: SimSpecs, operations?: EditOperation[] }`
- Output: ReadModel fields + `{ dryRun: bool }`
- Applies sim specs first, then variable operations. Non-dry-run edits save to disk. Returns refreshed model snapshot and loop analysis.

**EditOperation** (union -- each object has exactly one non-null field):

```
{ upsertStock: { name, initialEquation, units?, documentation?, inflows?, outflows? } }
{ upsertFlow: { name, equation, units?, documentation?, graphicalFunction? } }
{ upsertAuxiliary: { name, equation, units?, documentation?, graphicalFunction? } }
{ removeVariable: { name } }
```

**Supporting output types**:

```
LoopDominanceSummary: { loopId, name?, polarity, variables?, importance? }
DominantPeriod: { dominantLoops, startTime, endTime }
```

### Engine Analysis API

New `analysis` module in simlin-engine:

```rust
pub struct ModelAnalysis {
    pub model: json::Model,
    pub time: Vec<f64>,
    pub loop_dominance: Vec<LoopSummary>,
    pub dominant_loops_by_period: Vec<DominantPeriod>,
}
```

`analyze_model(project: &Project, model_name: &str) -> Result<ModelAnalysis>` pipeline:
1. Augment project with LTM synthetic variables (`generate_ltm_variables_all_links`)
2. Compile augmented project (`compile_project`)
3. Simulate (`run_to_end`)
4. Discover loops from results (`discover_loops`)
5. Extract time array from simulation specs
6. Compute dominant periods (`calculate_dominant_periods` -- ported from Go)
7. Build model snapshot as `json::Model` with views stripped
8. Cross-reference `LoopMetadata` for existing loop names

If simulation fails (equation errors), returns model snapshot with empty loop data rather than failing entirely.

`calculate_dominant_periods()` ported from Go `engine/model_impl.go`:
- For each timestep: collect loop scores, sort by absolute score, greedily accumulate loops of dominant polarity until cumulative importance >= 0.5
- Group consecutive timesteps with identical dominant loop sets into periods

### npm Package Distribution

Main package `@simlin/mcp` with `optionalDependencies` pointing to platform packages:
- `@simlin/mcp-darwin-arm64` (macOS Apple Silicon)
- `@simlin/mcp-darwin-x64` (macOS Intel)
- `@simlin/mcp-linux-x64` (Linux x86_64)
- `@simlin/mcp-win32-x64` (Windows x86_64)

Each platform package contains the pre-built `simlin-mcp` binary with `os` and `cpu` fields in `package.json` so npm auto-selects the correct one.

A JS `bin/simlin-mcp.js` entry point detects `process.platform`/`process.arch`, resolves the installed platform package, and spawns the native binary with signal forwarding. Follows the pattern used by `@openai/codex`.

A `build-npm-packages.sh` script generates platform `package.json` files with correct `os`/`cpu`/version fields.

## Existing Patterns

**Current simlin-mcp prototype** (`src/simlin-mcp/`): The prototype on the current branch provides the foundation. Tool trait, `TypedTool<I>`, Registry, JSON-RPC protocol handling, and all three tool handlers exist. The design evolves this by replacing the synchronous `serve()` loop with async tokio tasks and adding loop dominance analysis.

**simlin-engine JSON types**: All engine JSON types (`Stock`, `Flow`, `Auxiliary`, `Module`, `SimSpecs`, `View`, `Model`, etc.) already have conditional `#[derive(JsonSchema)]` behind the `schema` feature flag. simlin-mcp enables this feature and uses these types directly for output serialization. MCP-specific input types (edit operations) are separate, curated structs.

**simlin-engine loop analysis**: `ltm.rs` (loop detection, polarity), `ltm_finding.rs` (LTM importance scoring), and `ltm_augment.rs` (synthetic variable generation) provide all primitives. The new `analysis.rs` module composes these into a single `analyze_model()` call and adds `calculate_dominant_periods()` ported from Go.

**pysimlin release workflow** (`.github/workflows/release.yml`): Provides reference for multi-platform Rust binary builds via GitHub Actions with QEMU for ARM64 Linux cross-compilation.

**codex-rs npm pattern** (`third_party/codex/`): The `optionalDependencies` + platform-specific packages + JS resolver pattern is directly adopted for simlin-mcp npm distribution.

**Divergences**: The async transport architecture is new -- no existing code in simlin uses tokio. This is justified by the need for non-blocking I/O and future HTTP/SSE support.

## Implementation Phases

<!-- START_PHASE_1 -->
### Phase 1: Engine Analysis API
**Goal**: Add `analyze_model()` and `calculate_dominant_periods()` to simlin-engine as a reusable public API.

**Components**:
- `src/simlin-engine/src/analysis.rs` -- `ModelAnalysis`, `LoopSummary`, `DominantPeriod` types, `analyze_model()` function composing existing primitives, `calculate_dominant_periods()` ported from Go
- `src/simlin-engine/src/lib.rs` -- `pub mod analysis` export

**Dependencies**: None (builds on existing engine primitives)

**Covers**: simlin-mcp.AC2.1, simlin-mcp.AC2.2, simlin-mcp.AC2.5, simlin-mcp.AC2.6

**Done when**: `analyze_model()` compiles a model, simulates, returns loop dominance data with correct dominant periods. Tests verify the full pipeline on a test model with known feedback loops.
<!-- END_PHASE_1 -->

<!-- START_PHASE_2 -->
### Phase 2: Async Transport
**Goal**: Replace synchronous stdio loop with tokio-based async transport.

**Components**:
- `src/simlin-mcp/Cargo.toml` -- add tokio dependency
- `src/simlin-mcp/src/transport.rs` -- `Transport` trait, `StdioTransport` with three tokio tasks (stdin reader, processor, stdout writer) and MPSC channels
- `src/simlin-mcp/src/main.rs` -- `#[tokio::main]`, wire transport to protocol handler

**Dependencies**: None (can proceed in parallel with Phase 1)

**Covers**: simlin-mcp.AC1.1, simlin-mcp.AC1.2, simlin-mcp.AC1.3

**Done when**: MCP server starts, accepts JSON-RPC messages via stdin, returns responses via stdout, shuts down cleanly on EOF. Existing protocol tests adapted for async.
<!-- END_PHASE_2 -->

<!-- START_PHASE_3 -->
### Phase 3: Protocol and Error Handling Updates
**Goal**: Adapt protocol layer for async dispatch and MCP-standard error handling.

**Components**:
- `src/simlin-mcp/src/protocol.rs` -- async `dispatch()`, `CallToolResult` with `isError` field, error handling convention change

**Dependencies**: Phase 2 (async transport)

**Covers**: simlin-mcp.AC4.1, simlin-mcp.AC4.2, simlin-mcp.AC4.3

**Done when**: Protocol dispatch is async. Tool errors return `isError: true` with descriptive text. JSON-RPC errors used only for protocol failures. Tests verify both error paths.
<!-- END_PHASE_3 -->

<!-- START_PHASE_4 -->
### Phase 4: Tool Handlers -- CreateModel
**Goal**: Implement CreateModel matching Go input/output contract.

**Components**:
- `src/simlin-mcp/src/tools/create_model.rs` -- `CreateModelInput` (`projectPath`, optional `simSpecs`), `CreateModelOutput` (`projectPath`, `simSpecs`, `modelName`), handler logic

**Dependencies**: Phase 3 (async protocol)

**Covers**: simlin-mcp.AC3.1, simlin-mcp.AC3.2, simlin-mcp.AC3.3

**Done when**: CreateModel creates `.simlin.json` files with correct defaults, returns matching output shape, rejects existing files. Tests verify creation, defaults, and error cases.
<!-- END_PHASE_4 -->

<!-- START_PHASE_5 -->
### Phase 5: Tool Handlers -- ReadModel
**Goal**: Implement ReadModel with loop dominance analysis matching Go output contract.

**Components**:
- `src/simlin-mcp/src/tools/read_model.rs` -- `ReadModelInput` (`projectPath`, optional `modelName`), output with model snapshot + loop dominance data, calls `analyze_model()`

**Dependencies**: Phase 1 (engine analysis API), Phase 3 (async protocol)

**Covers**: simlin-mcp.AC2.1, simlin-mcp.AC2.2, simlin-mcp.AC2.3, simlin-mcp.AC2.4, simlin-mcp.AC2.5, simlin-mcp.AC2.6

**Done when**: ReadModel opens files in all supported formats, returns model snapshot (views omitted) with loop dominance data and dominant periods. Tests verify output shape, format detection, and model name defaulting.
<!-- END_PHASE_5 -->

<!-- START_PHASE_6 -->
### Phase 6: Tool Handlers -- EditModel
**Goal**: Implement EditModel with curated operation types and loop dominance in response.

**Components**:
- `src/simlin-mcp/src/tools/edit_model.rs` -- `EditModelInput` with `operations` (upsertStock, upsertFlow, upsertAuxiliary, removeVariable), `simSpecs`, `dryRun`, `modelName`; LLM-curated field sets; conversion to engine patch types; calls `apply_patch()` then `analyze_model()`

**Dependencies**: Phase 1 (engine analysis API), Phase 3 (async protocol)

**Covers**: simlin-mcp.AC3.4, simlin-mcp.AC3.5, simlin-mcp.AC3.6, simlin-mcp.AC3.7, simlin-mcp.AC3.8, simlin-mcp.AC3.9, simlin-mcp.AC3.10, simlin-mcp.AC3.11

**Done when**: EditModel applies patches (sim specs and variable operations), returns refreshed model + loop dominance, respects dry-run flag, excludes internal fields from schema. Tests verify all operation types, dry-run behavior, field filtering, and error cases.
<!-- END_PHASE_6 -->

<!-- START_PHASE_7 -->
### Phase 7: npm Package Structure
**Goal**: Create npm packaging for cross-platform binary distribution.

**Components**:
- `src/simlin-mcp/package.json` -- `@simlin/mcp` with `optionalDependencies` for platform packages, `bin` entry
- `src/simlin-mcp/bin/simlin-mcp.js` -- platform detection, binary resolution, spawn with signal forwarding
- `src/simlin-mcp/build-npm-packages.sh` -- generates platform `package.json` files with `os`/`cpu` fields

**Dependencies**: Phase 2 (working binary)

**Covers**: simlin-mcp.AC5.1, simlin-mcp.AC5.2, simlin-mcp.AC5.3

**Done when**: `node bin/simlin-mcp.js` resolves and spawns the correct platform binary. Platform package.json files have correct `os`/`cpu` fields. Build script generates all platform variants.
<!-- END_PHASE_7 -->

## Additional Considerations

**Graceful degradation**: If loop analysis fails (e.g., model has equation errors that prevent simulation), ReadModel and EditModel return the model snapshot with empty `loopDominance` and `dominantLoopsByPeriod` arrays. The LLM can still inspect and fix the model structure.

**File format conversion**: EditModel writes back as `.simlin.json` regardless of input format. If an XMILE or Vensim file is edited, the output is saved as JSON. This matches the current prototype behavior.

**Future HTTP/SSE transport**: The `Transport` trait is the extension point. Adding HTTP/SSE requires implementing the trait with an HTTP server (e.g., axum), spawning tool calls via `spawn_blocking` for CPU-bound work, and handling multiple concurrent clients. No changes to protocol or tool layers needed.
