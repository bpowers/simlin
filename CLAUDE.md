# Agent Guidance

## Overview

Simlin is a system dynamics (SD) modeling tool for building and simulating stock-and-flow models, including models from Vensim and Stella. The XMILE specification (`doc/xmile-v1.0.html`) is a crucial reference for simulation concepts, array/subscript notation, and equation syntax.

The engine is in Rust, the interactive editor in TypeScript/React, the server and app in TypeScript. This is a monorepo without external users -- breaking changes are OK if tests pass. Exception: protobuf files must follow standard versioning (we have a DB with serialized instances).

For detailed architecture and the dependency graph, see [doc/architecture.md](/doc/architecture.md).
For documentation index, see [doc/README.md](/doc/README.md).

## Components

| Component | Language | Description | Docs |
|-----------|----------|-------------|------|
| `src/simlin-engine` | Rust | Compiles, type-checks, and simulates SD models | [CLAUDE.md](/src/simlin-engine/CLAUDE.md) |
| `src/libsimlin` | Rust | Flat C FFI to simlin-engine (WASM, CGo, C/C++) | [CLAUDE.md](/src/libsimlin/CLAUDE.md) |
| `src/engine` | TypeScript | Promise-based TypeScript API for WASM engine | [CLAUDE.md](/src/engine/CLAUDE.md) |
| `src/core` | TypeScript | Shared data models and common utilities | [CLAUDE.md](/src/core/CLAUDE.md) |
| `src/diagram` | TypeScript | React model editor and visualization toolkit | [CLAUDE.md](/src/diagram/CLAUDE.md) |
| `src/app` | TypeScript | Full-featured SD application | [CLAUDE.md](/src/app/CLAUDE.md) |
| `src/server` | TypeScript | Express.js backend (Firebase Auth, Firestore) | [CLAUDE.md](/src/server/CLAUDE.md) |
| `src/xmutil` | C++/Rust | Vensim-to-XMILE converter (test-only) | -- |
| `src/simlin-cli` | Rust | CLI for simulation/conversion (testing/debugging) | [CLAUDE.md](/src/simlin-cli/CLAUDE.md) |
| `src/pysimlin` | Python/Rust | Python bindings for the simulation engine | [CLAUDE.md](/src/pysimlin/CLAUDE.md) |
| `website` | TypeScript | Rspress-based documentation site | [CLAUDE.md](/website/CLAUDE.md) |

## Environment Setup

**Always run at the start of every session:**

```bash
./scripts/dev-init.sh
```

Idempotent and fast -- short-circuits work already done.

## Build / Test / Lint

See [doc/dev/commands.md](/doc/dev/commands.md) for the full command reference.

Quick reference: `pnpm build`, `cargo test`, `pnpm test`, `pnpm lint`, `pnpm format`.

For benchmarks and profiling, see [doc/dev/benchmarks.md](/doc/dev/benchmarks.md).

## Pre-commit Hooks

The pre-commit hook (`scripts/pre-commit`) runs automatically and performs:
1. Rust formatting check
2. Rust linting (clippy)
3. Rust tests
4. TypeScript/JavaScript linting
5. TypeScript type checking
6. WASM build
7. TypeScript tests
8. Python bindings tests

**Important**: Never use `--no-verify` to skip hooks.

IMPORTANT: Lean on the pre-commit hook -- just run `git commit ...` and fix reported problems rather than running tests yourself to try to get a clean commit on the first try.

## Commit Message Style

- First line: `component: lowercase description` (no period, under 60 chars)
- Component prefix: module/directory name with "simlin-" prefix removed (e.g., `engine`, `diagram`, `core`, `doc`, `build`)
- Body: 1-2 paragraphs explaining "why", highlighting assumptions and non-obvious decisions
- DO NOT use "fixes"/"resolves" or emoji in commit messages

## Hard Rules

It is CRITICAL that you NEVER use `--no-verify` with `git commit`.

IMPORTANT: Simple, general, testable, maintainable code is better than preserving an interface. There are NO places where VM bytecode is serialized to disk; compatibility is only needed around protobufs.

**CRITICAL**: ALL work must follow test-driven development targeting 95%+ code coverage. For TypeScript, follow the functional core / imperative shell pattern.

IMPORTANT: If feedback seems non-actionable, it means you need comments explaining why the code looks that way.

## Development Standards

- Rust: [doc/dev/rust.md](/doc/dev/rust.md)
- TypeScript/React: [doc/dev/typescript.md](/doc/dev/typescript.md)
- Python (pysimlin): [doc/dev/python.md](/doc/dev/python.md)
- Workflow and problem-solving: [doc/dev/workflow.md](/doc/dev/workflow.md)

## Development Workflow for LLM Agents

### Understanding Requirements
- Read relevant code and documentation before making changes.
- If there are important/ambiguous architecture decisions, stop and ask.
- Start by adding tests to validate assumptions.
- Build the simplest interfaces possible while fully addressing the task.

### libsimlin API Design
Keep the FFI surface small and orthogonal. Prefer composable primitives over bulk endpoints. Do NOT add bulk/batch variants to paper over caller-side concurrency issues.

## Generated/Noise Paths

Treat these as generated output unless the task explicitly targets them:
- `src/*/lib/**`, `src/*/lib.browser/**`, `src/*/lib.module/**`
- `src/app/build/**`, `website/build/**`
- `node_modules/**`, `target/**`, `playwright-report/**`, `test-results/**`

## Test Models

The `test/` directory contains model files (XMILE, Vensim `.mdl`) with expected simulation outputs. These integration tests ensure engine behavior matches known-good results from other SD software.

## Protobuf Generation

`pnpm build:gen-protobufs` -- regenerate TypeScript and Rust protobuf bindings.
