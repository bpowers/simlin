# Simlin Development Guide

Simlin is a set of tools for building, editing, simulating, and analyzing system dynamics (SD) models.

## Simlin's Mission

Enable AI agents and humans to debug their intuition through simulation modeling, leveling-up their ability to learn.
With Simlin you can iterate on strategy and policy faster than you can in the real world, with fewer costs and the freedom to fail.

## Components

This is a monorepo without external users -- breaking changes are OK if tests pass. Exception: protobuf files must follow standard versioning (we have a DB with serialized instances).

| Component             | Language    | Description                                                       | Docs                                        |
|-----------------------|-------------|-------------------------------------------------------------------|---------------------------------------------|
| `src/simlin-engine`   | Rust        | Compiles, type-checks, and simulates SD models                    | [CLAUDE.md](/src/simlin-engine/CLAUDE.md)   |
| `src/libsimlin`       | Rust        | Flat C FFI to simlin-engine (WASM, CGo, C/C++)                    | [CLAUDE.md](/src/libsimlin/CLAUDE.md)       |
| `src/simlin-mcp-core` | Rust        | Transport-agnostic MCP tool surface (`ProjectAccess` trait, rmcp `ServerHandler`) | [CLAUDE.md](/src/simlin-mcp-core/CLAUDE.md) |
| `src/simlin-mcp`      | Rust/JS     | Stdio MCP server for AI assistants (`@simlin/mcp` npm)            | [CLAUDE.md](/src/simlin-mcp/CLAUDE.md)      |
| `src/simlin-serve`    | Rust/TS     | Local HTTP viewer/editor + in-process MCP (`@simlin/serve` npm)   | [CLAUDE.md](/src/simlin-serve/CLAUDE.md)    |
| `src/engine`          | TypeScript  | Promise-based TypeScript API for WASM engine                      | [CLAUDE.md](/src/engine/CLAUDE.md)          |
| `src/core`            | TypeScript  | Shared data models and common utilities                           | [CLAUDE.md](/src/core/CLAUDE.md)            |
| `src/diagram`         | TypeScript  | React model editor and visualization toolkit                      | [CLAUDE.md](/src/diagram/CLAUDE.md)         |
| `src/app`             | TypeScript  | Full-featured SD application                                      | [CLAUDE.md](/src/app/CLAUDE.md)             |
| `src/server`          | TypeScript  | Express.js backend (Firebase Auth, Firestore)                     | [CLAUDE.md](/src/server/CLAUDE.md)          |
| `src/xmutil`          | C++/Rust    | Vensim-to-XMILE converter (test-only)                             | --                                          |
| `src/simlin-cli`      | Rust        | CLI for simulation/conversion (testing/debugging)                 | [CLAUDE.md](/src/simlin-cli/CLAUDE.md)      |
| `src/pysimlin`        | Python/Rust | Python bindings for the simulation engine                         | [CLAUDE.md](/src/pysimlin/CLAUDE.md)        |
| `website`             | TypeScript  | Rspress-based documentation site                                  | [CLAUDE.md](/website/CLAUDE.md)             |

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

## Deployment

The web app deploys to Google App Engine via `pnpm deploy:web`. Read [docs/dev/deploy.md](/docs/dev/deploy.md) first -- the production config (`.app.prod.yaml`) isn't in the repo, the only CI gate is a smoke test of the deploy assembly (no `gcloud` in CI), and rollback is a GAE traffic split.

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

## Pull Requests

- The PR description MUST include a GitHub closing keyword -- `Fixes #<n>` (or `Closes #<n>`) -- for every issue the PR resolves, so merging auto-closes those issues. List one per line when a PR resolves several.
- Keep the closing keywords in the PR description, NOT in commit messages (commit messages deliberately avoid "fixes"/"resolves" per the style above; the auto-close should fire once, on merge, from the PR).
- The body should explain "why" and call out non-obvious decisions, mirroring the commit-message guidance.
- The body carries the evidence for the PR's claim, matched to the claim's boundary: name the tests or journeys that establish the behavior (a green suite asserts only what its tests assert -- unit tests do not establish a working browser journey, and coverage does not establish compatibility), include the artifact that carries the claim when checks alone don't (parity numbers for an engine-semantics change, a screenshot or short recording for UI work), and say what the evidence does NOT establish so review starts from the real confidence boundary instead of an assumed one.

## Hard Rules

IMPORTANT: Simple, general, testable, maintainable code is better than preserving an interface. There are NO places where VM bytecode is serialized to disk; backwards compatibility is ONLY needed for protobufs.

**CRITICAL**: ALL work must follow test-driven development targeting 95%+ code coverage. For TypeScript, follow the functional core / imperative shell pattern.

**CRITICAL**: Individual unit tests must complete in a few seconds on a debug build. `cargo test --workspace` runs under a 3-minute wall-clock cap in both pre-commit and CI. Do not test production threshold gates by building fixtures large enough to trip them -- use a test-only override and a tiny fixture. See [docs/dev/rust.md](/docs/dev/rust.md#test-time-budgets) for details.

IMPORTANT: If feedback seems non-actionable, it means you need comments explaining why the code looks that way.

**CRITICAL**: A test that pins one arm of an N-way decision reads exactly like a test that pins the decision. Derive the rows from the enumeration -- the variant list, the call-site list, the axis list -- and cover every arm, or state in the test which arms it does not cover and where they are covered instead. "It passes" and "it constrains the code" are different claims.

**CRITICAL**: A test that hand-builds its inputs proves nothing about the inputs production supplies. If a fixture constructs a dependency set, a scope, or a context by hand, either derive it through the same function production calls or justify in the test why the hand-built value is the one production produces. This has shipped a fix that could not fire: the fixture supplied a dependency set the extractor never generates, so the test passed on an input that does not occur and the defect it claimed to cover survived untouched.

## Claims About Other Tools

Vensim, Stella, and the XMILE specification are external systems. What one of them does is a **fact to be checked, not a premise to reason from** -- and the sources are cheap:

- **XMILE spec**: `docs/reference/xmile-v1.0.html`, in-repo and greppable (strip tags and search the prose; the footnotes carry the resolution rules).
- **Vensim function reference**: one page per function at vensim.com/documentation, named fn_ plus the lowercased function name with underscores.
- **Ground truth output**: real Vensim DSS runs checked into `test/` (e.g. `test/test-models/tests/vector_order/output.tab`, which is how VECTOR SORT ORDER's semantics were settled).

When code encodes a claim about another tool, cite the source next to it. When you cannot check one, write that the claim is unverified rather than asserting it -- and never let an unverified claim carry a design decision.

This is the most expensive class of error in this repo, because review does not catch it: reviewers check the code against the stated claim, not the claim against the world. A wrong premise about an external tool therefore survives every round of review that its own code passes, and the work built on top of it is wasted rather than merely wrong.

## Comment and Rustdoc Standards

- Preserve useful comments/docstrings when refactoring. Do not delete comments unless they are stale, wrong, or redundant with clearer replacement code.
- Comments should explain **why** (invariants, ordering constraints, cache behavior, edge-case semantics), not line-by-line mechanics.
- Public Rust items and non-trivial internal functions should have concise rustdoc describing purpose, key assumptions, and side effects.
- When behavior changes, update nearby comments in the same commit so docs and code stay aligned.
- If you intentionally remove a comment block, replace it with an updated equivalent when the context is still non-obvious.
- **Documentation is evergreen, NEVER a changelog.** Docs (CLAUDE.md files, `docs/`, rustdoc, docstrings) describe the current state of the code; they never narrate the edit that produced it. "X was removed", "this used to Y", "now does Z", "behaviour is unchanged" are all changelog sentences -- git history is the changelog, and readers dig there when they want it. When you delete or move something, rewrite the surrounding docs as if the code had always been this way. If the old design carried a lesson worth keeping, state it as a standing constraint ("never replace this alias with a second implementation: a hand-maintained copy drifts exactly where the real one is non-trivial"), not as a story about what happened. Citing a GH issue for a load-bearing decision is fine -- an issue number is a pointer, not a narrative.
- NEVER add a "Last updated" (or "Last verified") line to a `CLAUDE.md`: it is a perpetual rebase/merge-conflict magnet and goes stale immediately. Describe current state in prose; rely on `git log` / `git blame` for history.

## Development Standards

- Rust: [docs/dev/rust.md](/docs/dev/rust.md)
- TypeScript/React: [docs/dev/typescript.md](/docs/dev/typescript.md)
- Python (pysimlin): [docs/dev/python.md](/docs/dev/python.md)
- Workflow and problem-solving: [docs/dev/workflow.md](/docs/dev/workflow.md)
- Product design (users, brand, tokens, accessibility): [docs/dev/design.md](/docs/dev/design.md) -- consult for any frontend or visual work
- Improving the harness (guidance, checks, agent tooling): [docs/dev/harness.md](/docs/dev/harness.md)

## Development Workflow for LLM Agents

### Understanding Requirements
- Read relevant code and documentation before making changes.
- If there are important/ambiguous architecture decisions, stop and ask.
- Start by adding tests to validate assumptions.
- Build the simplest interfaces possible while fully addressing the task.

### libsimlin API Design
Keep the FFI surface small and orthogonal. Prefer composable primitives over bulk endpoints. Do NOT add bulk/batch variants to paper over caller-side concurrency issues.

## Discovered Issues

When you discover something wrong or concerning during your work -- tech debt, a latent bug, design limitations, broken tooling, missing CI checks, unintended consequences of a committed design, deferred review feedback -- **fix it as part of the work**. Never silently drop these observations, and never file an issue as a substitute for fixing one.

Filing defers the cost without reducing it, and the context needed to fix a problem is at its cheapest the moment you find it. Name what you fixed in the commit message or PR body (`Fixes #<n>` when an issue already exists).

Two things are NOT covered by "fix it", and both are explicit conversations rather than silent decisions:

- **The fix is too large to fold in** -- it would swamp the branch's review surface, or it belongs to a different subsystem. Say so, with a cost estimate, and sequence it: its own commit, its own PR, or (if that is what the user wants) tracked for later. Do not make that call quietly.
- **The fix is not yours to make** -- it needs a product decision, or access you do not have. Surface it.

**A fix that introduces a regression is a scope signal, not a bug to try harder at.** Once the second attempt at the same discovered issue produces a new defect, stop and re-scope: the problem is larger than the branch it was found in, and each further attempt is being designed against an understanding that has already failed twice. Split it out with what was learned, rather than continuing. Discovering something mid-task tells you it exists; it does not tell you it belongs in the change you are making.

When something genuinely does need tracking, spawn the `track-issue` agent (via the Task tool with `subagent_type: "track-issue"`) with a description of the problem. It checks for duplicates in GitHub issues and [docs/tech-debt.md](/docs/tech-debt.md) and files the item, keeping your context on the main task.

## Generated/Noise Paths

Treat these as generated output unless the task explicitly targets them:
- `src/*/lib/**`, `src/*/lib.browser/**`, `src/*/lib.module/**`
- `src/app/build/**`, `website/build/**`
- `node_modules/**`, `target/**`, `playwright-report/**`, `test-results/**`

## Test Models

The `test/` directory contains model files (XMILE, Vensim `.mdl`, systems format `.txt`) with expected simulation outputs. These integration tests ensure engine behavior matches known-good results from other SD software.

## Protobuf Generation

`pnpm build:gen-protobufs` -- regenerate TypeScript and Rust protobuf bindings.
