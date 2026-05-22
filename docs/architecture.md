# Simlin Architecture

## Overview

Simlin is a system dynamics (SD) modeling tool for building and simulating stock-and-flow models, including models created in Vensim and Stella. The engine is in Rust; the interactive editor is in TypeScript/React.

This is a monorepo without external users -- breaking changes are OK as long as tests pass. The one exception: changes to protobuf files must follow standard best practices, as we have a DB with serialized protobuf instances.

## Component Descriptions

### `src/simlin-engine` (Rust)
Core simulation engine. Compiles, type-checks, unit-checks, and simulates SD models.
- Projects consist of 1 or more models, compiled to bytecode (`compiler/`)
- Primary compilation path is `db::compile_project_incremental()` using salsa tracked functions for fine-grained incrementality (`db.rs`, `db_analysis.rs`, `db_ltm.rs`, `db_ltm_ir.rs`)
- Equation text is parsed via recursive descent parser (`parser/mod.rs`)
- Simulations run on a stack-based bytecode VM (`vm.rs`) with `PREVIOUS`/`INIT` intrinsic opcodes
- An alternative WebAssembly code-generation backend (`wasmgen/`) lowers a compiled model to one self-contained wasm module (no host imports) for fast repeated re-simulation; the VM stays the correctness oracle (every emitted module is checked against it). Surfaced through libsimlin `simlin_model_compile_to_wasm`
- `builtins.rs` defines builtin functions (including `PREVIOUS`, `INIT`); stateful module functions (TREND, SMOOTH3) are model definitions in `stdlib/*.stmx`, generated into `stdlib.gen.rs`
- Native Vensim MDL parser in `mdl/` (replaces C++ xmutil); see [docs/design/mdl-parser.md](/docs/design/mdl-parser.md)

### `src/libsimlin` (Rust)
Flat "C" FFI wrapper around simlin-engine. Used from TypeScript (WASM), Go (CGo), and C/C++ (`simlin.h`).
- **API design**: keep the FFI surface small and orthogonal. Prefer composable primitives over bulk endpoints. Each FFI function is individually thread-safe.

### `src/engine` (TypeScript)
TypeScript API for WASM libsimlin. Promise-based; in browser, WASM runs in a Web Worker.

### `src/core` (TypeScript)
Shared data models and utilities. Protobuf-based `datamodel.ts`, `canonicalize.ts` for variable name handling.

### `src/diagram` (TypeScript)
React components for model visualization and editing. General-purpose SD model editor toolkit without Simlin app dependencies. `Editor.tsx` handles user interaction; `drawing/` contains rendering components.

### `src/app` (TypeScript)
Full-featured SD application. Browse, create, import models; login/logout.

### `src/server` (TypeScript)
Express.js backend. Firebase Auth (`authn.ts`), Firestore persistence (`models/db-firestore.ts`) in protobuf form.

### `src/xmutil` (C++ and Rust)
Rust wrapper around Bob Eberlein's xmutil C++ tool for converting Vensim models to XMILE. Only used for testing -- `src/simlin-engine/src/mdl` now fully implements this in Rust.

### `src/simlin-mcp-core` (Rust)
Transport-agnostic core library shared by every Simlin MCP server. Owns the `ProjectAccess` trait (the storage abstraction), the three reused tools (`read_model`, `edit_model`, `create_model`) as async free functions, the rmcp `ServerHandler` impl as `SimlinMcpServer<A: ProjectAccess>`, and the wire-format types (`ErrorOutput`, `LoopDominanceSummary`, `DominantPeriodOutput`).

### `src/simlin-mcp` (Rust)
Stdio MCP server for AI assistants. Thin binary built on `simlin-mcp-core` plus a stateless `FileSystemAccess` impl that re-reads/writes the file on every call. Distributed as `@simlin/mcp` on npm via per-platform optional dependencies.

### `src/simlin-serve` (Rust + TypeScript SPA)
Local HTTP viewer/editor and in-process MCP server. Binds two `127.0.0.1` ports (UI + MCP) and serves a React SPA from any directory containing SD models. Saves merge through a per-project Loro CRDT shared by the HTTP, MCP, and watcher paths so concurrent edits converge. Distributed as `@simlin/serve` on npm using the same per-platform pattern as `@simlin/mcp`. SPA workspace member is `@simlin/serve-web` (not published).

### `src/simlin-cli` (Rust)
CLI for simulating and converting models, mostly for testing/debugging.

### `src/pysimlin` (Python/Rust)
Python bindings for the simulation engine via CFFI. Thread-safe wrapper classes for free-threaded Python (PEP 703). Tooling: `ruff`, `mypy` strict, `pytest` + `hypothesis`, `uv`.

### `website` (TypeScript)
Rspress-based documentation and website package.

## Dependency Graph

The allowed dependency graph is enforced by `scripts/check-deps.py` reading from `scripts/dep-policy.json`.

### Rust

```
xmutil (standalone)
  ^
  | (optional, feature-gated)
simlin-engine
  ^       ^             ^
  |       |             |
simlin    simlin-mcp-core
  ^         ^      ^
  |         |      |
simlin-cli  simlin-mcp  simlin-serve
            (also depends on simlin-engine directly)
```

- `simlin-engine` -> `xmutil` (optional, feature-gated via `dep:xmutil`)
- `simlin` (libsimlin) -> `simlin-engine`
- `simlin-mcp-core` -> `simlin-engine`
- `simlin-mcp` -> `simlin-mcp-core`, `simlin-engine`
- `simlin-serve` -> `simlin-mcp-core`, `simlin-engine`
- `simlin-cli` -> `simlin-engine`, `simlin`
- `xmutil` -> (none)

### TypeScript

```
@simlin/engine (leaf)
  ^
  |
@simlin/core
  ^
  |
@simlin/diagram
  ^       ^         ^
  |       |         |
@simlin/app   @simlin/server   @simlin/serve-web

simlin-site (standalone)
```

- `@simlin/engine` -> (none)
- `@simlin/core` -> `@simlin/engine`
- `@simlin/diagram` -> `@simlin/core`, `@simlin/engine`
- `@simlin/app` -> `@simlin/core`, `@simlin/diagram`, `@simlin/engine`
- `@simlin/server` -> `@simlin/core`, `@simlin/diagram`, `@simlin/engine`
- `@simlin/serve-web` -> `@simlin/core`, `@simlin/diagram`, `@simlin/engine`
- `simlin-site` -> (none)

## Test Models

The `test/` directory contains model files (XMILE, Vensim `.mdl`) with expected simulation outputs. These integration tests ensure engine behavior matches known-good results from other SD software.

## Generated/Noise Paths

Treat these as generated output unless the task explicitly targets them:
- `src/*/lib/**`, `src/*/lib.browser/**`, `src/*/lib.module/**`
- `src/app/build/**`, `website/build/**`
- `node_modules/**`, `target/**`, `playwright-report/**`, `test-results/**`

## XMILE Specification

The XMILE interchange format spec is at `docs/xmile-v1.0.html`. It covers simulation concepts, array/subscript notation, XML structure, and equation syntax. Consult it when adding engine functionality.
