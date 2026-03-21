# Simlin Development Guide

Simlin is a set of tools for building, editing, simulating, and analyzing system dynamics (SD) models.

## Simlin's Mission

Enable AI agents and humans to debug their intuition through simulation modeling, leveling-up their ability to learn.
With Simlin you can iterate on strategy and policy faster than you can in the real world, with fewer costs and the freedom to fail.

## Components

This is a monorepo without external users -- breaking changes are OK if tests pass. Exception: protobuf files must follow standard versioning (we have a DB with serialized instances).

| Component           | Language    | Description                                       | Docs                                      |
|---------------------|-------------|---------------------------------------------------|-------------------------------------------|
| `src/simlin-engine` | Rust        | Compiles, type-checks, and simulates SD models    | [CLAUDE.md](/src/simlin-engine/CLAUDE.md) |
| `src/libsimlin`     | Rust        | Flat C FFI to simlin-engine (WASM, CGo, C/C++)    | [CLAUDE.md](/src/libsimlin/CLAUDE.md)     |
| `src/simlin-mcp`    | Rust/JS     | MCP server for AI assistants (`@simlin/mcp` npm)  | [CLAUDE.md](/src/simlin-mcp/CLAUDE.md)    |
| `src/engine`        | TypeScript  | Promise-based TypeScript API for WASM engine      | [CLAUDE.md](/src/engine/CLAUDE.md)        |
| `src/core`          | TypeScript  | Shared data models and common utilities           | [CLAUDE.md](/src/core/CLAUDE.md)          |
| `src/diagram`       | TypeScript  | React model editor and visualization toolkit      | [CLAUDE.md](/src/diagram/CLAUDE.md)       |
| `src/app`           | TypeScript  | Full-featured SD application                      | [CLAUDE.md](/src/app/CLAUDE.md)           |
| `src/server`        | TypeScript  | Express.js backend (Firebase Auth, Firestore)     | [CLAUDE.md](/src/server/CLAUDE.md)        |
| `src/xmutil`        | C++/Rust    | Vensim-to-XMILE converter (test-only)             | --                                        |
| `src/simlin-cli`    | Rust        | CLI for simulation/conversion (testing/debugging) | [CLAUDE.md](/src/simlin-cli/CLAUDE.md)    |
| `src/pysimlin`      | Python/Rust | Python bindings for the simulation engine         | [CLAUDE.md](/src/pysimlin/CLAUDE.md)      |
| `website`           | TypeScript  | Rspress-based documentation site                  | [CLAUDE.md](/website/CLAUDE.md)           |

The XMILE specification (`docs/reference/xmile-v1.0.html`) is a crucial reference for simulation concepts, array/subscript notation, and equation syntax.

For detailed architecture and the dependency graph, see [docs/architecture.md](/docs/architecture.md).
For documentation index, see [docs/README.md](/docs/README.md).

## Environment Setup

**Always run at the start of every session:**

```bash
./scripts/dev-init.sh
```

(Idempotent and fast: short-circuits work already done)

## Build / Test / Lint

See [docs/dev/commands.md](/docs/dev/commands.md) for the full command reference.

Quick reference: `pnpm build`, `cargo test`, `pnpm test`, `pnpm lint`, `pnpm format`.

For benchmarks and profiling, see [docs/dev/benchmarks.md](/docs/dev/benchmarks.md).

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

**Important**: NEVER use `--no-verify` with `git commit` to skip hooks.

Lean on the pre-commit hook: run `git commit ...` and fix reported problems rather than running tests yourself to try to get a clean commit on the first try.

## Commit Message Style

- First line: `component: lowercase description` (no period, under 60 chars)
- Component prefix: module/directory name with "simlin-" prefix removed (e.g., `engine`, `diagram`, `core`, `doc`, `build`)
- Body: 1-2 paragraphs explaining "why", highlighting assumptions and non-obvious decisions
- DO NOT use "fixes"/"resolves" or emoji in commit messages

## Hard Rules

IMPORTANT: Simple, general, testable, maintainable code is better than preserving an interface. There are NO places where VM bytecode is serialized to disk; backwards compatibility is ONLY needed for protobufs.

**CRITICAL**: ALL work must follow test-driven development targeting 95%+ code coverage. For TypeScript, follow the functional core / imperative shell pattern.

IMPORTANT: If feedback seems non-actionable, it means you need comments explaining why the code looks that way.

## Comment and Rustdoc Standards

- Preserve useful comments/docstrings when refactoring. Do not delete comments unless they are stale, wrong, or redundant with clearer replacement code.
- Comments should explain **why** (invariants, ordering constraints, cache behavior, edge-case semantics), not line-by-line mechanics.
- Public Rust items and non-trivial internal functions should have concise rustdoc describing purpose, key assumptions, and side effects.
- When behavior changes, update nearby comments in the same commit so docs and code stay aligned.
- If you intentionally remove a comment block, replace it with an updated equivalent when the context is still non-obvious.

## Development Standards

- Rust: [docs/dev/rust.md](/docs/dev/rust.md)
- TypeScript/React: [docs/dev/typescript.md](/docs/dev/typescript.md)
- Python (pysimlin): [docs/dev/python.md](/docs/dev/python.md)
- Workflow and problem-solving: [docs/dev/workflow.md](/docs/dev/workflow.md)

## Development Workflow for LLM Agents

### Understanding Requirements
- Read relevant code and documentation before making changes.
- If there are important/ambiguous architecture decisions, stop and ask.
- Start by adding tests to validate assumptions.
- Build the simplest interfaces possible while fully addressing the task.

### libsimlin API Design
Keep the FFI surface small and orthogonal. Prefer composable primitives over bulk endpoints. Do NOT add bulk/batch variants to paper over caller-side concurrency issues.

## Tracking Discovered Issues

When you discover something wrong or concerning during your work -- tech debt, design limitations, broken tooling, missing CI checks, unintended consequences of a committed design, deferred review feedback -- it must be explicitly tracked. Never silently drop these observations.

Spawn the `track-issue` agent (via the Task tool with `subagent_type: "track-issue"`) with a description of the problem. The agent checks for duplicates in GitHub issues and [docs/tech-debt.md](/docs/tech-debt.md), then files the item if it's not already tracked. Using a sub-agent preserves your context on the main task.

## Generated/Noise Paths

Treat these as generated output unless the task explicitly targets them:
- `src/*/lib/**`, `src/*/lib.browser/**`, `src/*/lib.module/**`
- `src/app/build/**`, `website/build/**`
- `node_modules/**`, `target/**`, `playwright-report/**`, `test-results/**`

## Test Models

The `test/` directory contains model files (XMILE, Vensim `.mdl`, systems format `.txt`) with expected simulation outputs. These integration tests ensure engine behavior matches known-good results from other SD software.

## Protobuf Generation

`pnpm build:gen-protobufs` -- regenerate TypeScript and Rust protobuf bindings.

## Design Context

### Users
System dynamics modelers and researchers who build stock-and-flow models. They come to Simlin to construct, simulate, and debug mental models of complex systems. The tool should feel like a natural extension of their thinking -- lowering barriers to SD modeling rather than adding cognitive overhead.

### Brand Personality
**Approachable, playful, modern.** Simlin should feel friendly and inviting, not intimidating or academic. The tagline "Debug your intuition" sets the tone: serious about insight, light about process.

### Aesthetic Direction
**Modern minimal** -- reduce visual weight, fewer shadows, flatter surfaces, generous whitespace. Inspired by **Figma and Linear**: clean professional tools with polished UX and obsessive attention to detail. Avoid dense IDE-like interfaces or cluttered dashboards.

The existing Material Design-inspired component library provides a solid foundation. Evolve it toward a lighter, more distinctive look: thinner borders, subtler elevation, more breathing room.

### Design Principles
1. **Clarity over decoration** -- Every visual element should serve comprehension. Remove what doesn't help the user think.
2. **Quiet until needed** -- Chrome and controls should recede. The model diagram is the primary artifact; UI supports it, not competes with it.
3. **Friendly precision** -- Warm and approachable, but never imprecise. Data and simulation results demand visual accuracy.
4. **Progressive disclosure** -- Simple by default, powerful on demand. Don't overwhelm new users; reward exploration for experts.
5. **Consistent and predictable** -- Follow established patterns from the component library. Spacing (8px grid), typography (Roboto), and color (primary #1976d2) should be applied uniformly.

### Design Tokens Reference
- **Primary**: #1976d2 | **Secondary**: #dc004e | **Selected**: #4444dd
- **Error**: #c62828 | **Success**: #2e7d32 | **Warning**: #f57f17
- **Font**: Roboto, Helvetica, Arial, sans-serif
- **Spacing base**: 8px grid
- **Border radius**: 4px (standard)
- **Dark mode**: Supported via `[data-theme="dark"]`
- **Component library**: `src/diagram/components/` (custom, no MUI)
- **Design tokens**: `src/diagram/theme.css`

### Accessibility
Best-effort approach: follow good practices (sufficient contrast, keyboard navigation, semantic HTML, focus indicators) without blocking on strict WCAG compliance.
