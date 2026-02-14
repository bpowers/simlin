# Agent Guidance

## Overview

Simlin is a system dynamics (SD) modeling tool.
It can be used to build and simulate system dynamics models, including models created in other software like Vensim (from Ventana Systems) and Stella (from isee systems).
There is a model save file/interchange format called XMILE, and the XMILE specification is in `doc/xmile-v1.0.html`.
It covers general concepts like how simulation works and how arrays and subscripting notation work as well as details of the XML structure and equation syntax.
It is a crucial resource to consult when adding functionality to the simulation engine.

The engine for simulating system dynamics stock and flow models is written in Rust, and the interactive model editor targets evergreen browsers and is written in TypeScript.
Additional components like the server and higher-level app functionality are written in TypeScript as well.

This is a monorepo without external users -- breaking changes and API changes are OK as long as tests + the pre-commit hook passes with only 1 exception: changes to protobuf files must follow standard best practices, as we have a DB with serialized protobuf instances in it.

## Architecture Overview

### Core Components

Rust components are standard cargo projects in a cargo workspace, and TypeScript projects are in a pnpm workspace.

**`src/simlin-engine` (Rust)**: Compiles, type + unit checks, and simulates system dynamics models
- Projects consist of 1 or more models, and are type-checked and compiled to a simple bytecode format (`compiler.rs`)
- Equation text is parsed into an AST using a recursive descent parser (`parser/mod.rs`)
- Simulations are run/evaluated using a bytecode-based virtual machine (`vm.rs`)
- We have a simple AST-walking `interpreter.rs` serving as a "spec" to verify the VM produces the same/correct results.
- `builtins.rs` defines builtin equation functions; stateful SD module functions like TREND and SMOOTH3 are model definitions in `stdlib/*.stmx`, generated into `stdlib.gen.rs`

**`src/libsimlin` (Rust)**: Flat "C" FFI to simlin-engine
- Used from TypeScript, also usable from Go via CGo and from C/C++ through `simlin.h`.

**`src/engine` (TypeScript)**: TypeScript API for interacting with WASM libsimlin
- Provides clear, idiomatic TypeScript interface to the simulation engine
- Internally deals with memory management and safely invoking WASM functions
- Promise-based API; in browser WASM is instantiated in a Web Worker to avoid jank

**`src/core` (TypeScript)**: Shared data models and common utilities
- Defines protobuf-based `datamodel.ts`, `canonicalize.ts` variable name handling, etc
- Used by both frontend and backend

**`src/diagram` (TypeScript)**: React components for model visualization and editing
- designed as a general purpose SD model editor component and toolkit, without dependencies on the Simlin app or server API
- `Editor.tsx` is the model editor, handling user interaction, state, and tool selection
- Drawing components in `drawing/` subdirectory, including `Canvas.tsx` for overall visual layout

**`src/app` (TypeScript)**: Full featured system dynamics application
- Browse existing models, create or import new models, login/logout, etc.

**`website` (TypeScript)**: Rspress-based documentation and website package
- Maintains published docs/site content and build tooling for the public-facing website

**`src/server` (TypeScript)**: Express.js backend API
- Authentication via Firebase Auth (`authn.ts`)
- Models persisted in Firestore (`models/db-firestore.ts`) in protobuf form

**`src/xmutil` (C++ and Rust)**: Rust package wrapping Bob Eberlein's xmutil C++ tool to convert Vensim models to XMILE format, including diagrams
- Only used for testing -- `src/simlin-engine/src/mdl` now fully implements this functionality in Rust

**`src/simlin-cli` (Rust)**: CLI for simulating and converting models, mostly for testing/debugging

**`src/pysimlin` (Python/Rust)**: Python bindings for the simulation engine
- Aims to expose the full engine functionality in idiomatic Python, with the target users being AI agents analyzing model behavior, calibrating models, etc.
- Uses CFFI to call libsimlin's C API; the Rust FFI layer uses per-object `Mutex` for thread safety
- Python wrapper classes (`Project`, `Model`, `Sim`) each carry a `threading.Lock` to be safe under free-threaded Python (PEP 703 / 3.13t+)
- Module-level shared state (`_finalizer_refs`) is protected by `_refs_lock`
- Tooling: `ruff` for linting **and** formatting (no black), `mypy` strict mode, `pytest` with `hypothesis`, `uv` for package management

### Test Models

The `test/` directory contains an extensive suite of model files (XMILE, Vensim `.mdl`) with expected simulation outputs. These integration tests ensure engine behavior matches known-good results from other software and are critical when working on engine functionality.

### Generated/Noise Paths

When navigating or editing, treat these paths as generated output or transient build/test noise unless the task explicitly targets them:
- `src/*/lib/**`, `src/*/lib.browser/**`, `src/*/lib.module/**`
- `src/app/build/**`, `website/build/**`
- `node_modules/**`, `target/**`, `playwright-report/**`, `test-results/**`

### Environment Setup

**Always run at the start of every session** (agent or human, fresh container or long-running host):

```bash
./scripts/dev-init.sh
```

The script is idempotent and fast -- it short-circuits work that is already done (git hooks installed, pnpm deps up to date, AI tool already configured).  Run it unconditionally; there is no need to check whether it has already been run.

### Pre-commit Hooks

The pre-commit hook (`scripts/pre-commit`) runs automatically before each commit and performs:
1. Rust formatting check
2. Rust linting
3. Rust tests
4. TypeScript/JavaScript linting
5. TypeScript type checking
6. WASM build
7. TypeScript tests
8. Python bindings tests
9. Test quality verification

If any check fails, the commit is rejected. Fix the issues and try again.

**Important**: Never use `--no-verify` to skip hooks. The hooks exist to maintain code quality.

IMPORTANT: lean on the pre-commit hook - if you are getting ready to commit, just run `git commit ...` and fix reported problems rather than running/re-running tests yourself to try to get a successful commit on the first try.

## Development Commands

### Build Commands
- `pnpm build` - Build the web application and System Dynamics model editor, including compiling the simulation engine to WebAssembly.
- `pnpm clean` - Clean all build artifacts
- `cargo build` - Build Rust components
- `pnpm format` - Format both JavaScript/TypeScript and Rust code

### Linting and Type Checking
- `pnpm lint` - Lint both Rust (`cargo clippy`) and TypeScript/JavaScript

### Testing
- `cargo test` and `pnpm test`

### Code Coverage
- `cargo llvm-cov` - Rust code coverage via LLVM source-based instrumentation (install: `cargo install cargo-llvm-cov`)
- `cargo llvm-cov --html` - Generate an HTML coverage report in `target/llvm-cov/html/`
- Works on both macOS and Linux; uses `rustc -C instrument-coverage` under the hood

### Protobuf Generation
- `pnpm build:gen-protobufs` - Regenerate protobuf bindings (TypeScript from server schemas, Rust from simlin-engine schema)

## Commit Message Style

- First line format: component: description
- Component prefix: Use the module/directory name with the "simlin-" prefix removed if it exists (e.g., engine, diagram, core, doc, build)
- Description: Start with lowercase, present tense verb, no period
- Length: Keep the initial line concise, typically under 60 characters
- Examples:
  - engine: fix failing test due to bad helper
  - diagram: display equations as LaTeX
  - testing: add basic visual regression tests
- Add 1 to 2 paragraphs of the "why" of the change in the body of the commit message. Especially highlight any assumptions you made or non-obvious decisions or tricky implementation details.
- DO NOT use "fixes" or "resolves" in the commit message. Use the issue tracker for that.
- DO NOT use any emoji in the commit message.

## Development Workflow for LLM Agents

It is CRITICAL that you NEVER use the `--no-verify` flag with `git commit`.

IMPORTANT: It is MUCH better to have simple, general, testable and maintainable code than to avoid changing an interface or abstraction.  Take the time to do it right.
There are NO places where the VM bytecode is serialized to disk, the ONLY place where there is a need for compatibility is around protobufs, where we should follow standard protobuf versioning and change standards.

**CRITICAL**: ALL work should follow test-driven development and target 95+% code coverage for all new code, both in Rust and TypeScript.  This should be straightforward for Rust code, for TypeScript it often means following the functional core, imperative shell pattern to ensure as much of the logic and functionality is in easily testable pure functions.  Parts of the Editor and TypeScript components didn't follow this practice historically: when planning new work, if necessary take your time, think deeply and where necessary have initial phase(s) of the plan refactor the code to be more modular and testable, with tests validating the current behavior (remember: TDD).

IMPORTANT: If you get feedback on code that you don't think is actionable, it at a minimum indicates you are missing comments providing appropriate context for why the code looks that way or does what it does.

When working on this codebase, follow this systematic approach:

### Problem-Solving Philosophy

- **Write high-quality, general-purpose solutions**: Implement solutions that work correctly for all valid inputs, not just test cases. Do not hard-code values or create solutions that only work for specific test inputs.
- **Prioritize the right approach over the first approach**: Research the proper way to implement features rather than implementing workarounds. If you are not sure, explore several approaches and then choose the most promising one (or ask the user for their input if one isn't clearly best).
- **Keep implementations simple and maintainable**: Start with the simplest solution that meets requirements. Only add complexity when the simple approach demonstrably fails.
- **No special casing in tests**: Tests should hold all implementations to the same standard. Never add conditional logic in tests that allows certain implementations to skip requirements.
- **No compatibility shims or fallback paths**: Remember there are no external users of this codebase, and at this point we have a comprehensive test suite.  Fully complete migrations.
- **Test-driven Development (TDD)**: Follow TDD best practices, ensure tests actually assert the behavior we're expecting AND have high code coverage.

### Understanding Requirements
- Read relevant code and documentation (including for libraries) and build a plan based on the user's task.
- If there are important and ambiguous high-level architecture decisions or "trapdoor" choices, stop and ask the user.
- Start by adding tests to validate assumptions before making changes.
- Remember: we want to build the simplest interfaces and abstractions possible while FULLY addressing the task and requirements in full generality.

## Development Guide

### Rust Development Standards

Follow these steps when working on code changes in Rust crates like `src/simlin-engine`:

- DO NOT write one-off rust files and compile them with `rustc` to test hypotheses and assumptions. Instead, write new unit tests as close to the source of the problem as possible. These unit tests are valuable additions to the test suite and should be left at the end of the task so that the user can review your assumptions.
- **Strongly** prefer idiomatic use of `Result`/`Option` rather than `.unwrap()` or `.unwrap_or_default()`.  `unwrap_or_default` should generally be avoided as it masks unexpected conditions. It is EXTREMELY valuable to understand when our assumptions are wrong, and using a default/0/1 fixed value hides that.
- If a case (for example in a match arm) is expected to be unreachable, use the `unreachable!()` macro not a code comment.
- Code should never have comments like "this is a placeholder". If you have stubbed something out, that should be documented in code via the use of the `todo!()` or `unimplemented!()` macros. But generally this means your current task is not complete! Continue working until you have a general-purpose, maintainable solution that can be confidently deployed to a production environment.
- Similarly, tests should err on the side of brittleness: if you are missing a required test file, loudly fail rather than skipping the test.

### TypeScript/React Development Standards

#### Code Style
- Use TypeScript with strict mode enabled
- Prefer class components by default. Hooks are allowed when wrapping/integrating with components that only support hook-based APIs; in all other cases, prefer classes.
- Use proper TypeScript types, avoid `any`
- NEVER manually copy files around to get builds or tests passing. If there is some sort of regression or error where source files are not able to imported or used, identify the root cause and fix the build scripts.

### Python (pysimlin) Development Standards

#### Code Style
- Use `ruff` for both linting and formatting (replaces black). Run `ruff check` and `ruff format`.
- Use `mypy` with strict mode (`mypy simlin`).
- Target Python 3.11+ -- use modern type syntax (`list[str]`, `dict[str, int]`, `X | None`) and `from __future__ import annotations` in all source files.

#### Thread Safety
- **All wrapper classes** (`Project`, `Model`, `Sim`) have a per-instance `threading.Lock` (`self._lock`) that protects `_ptr` and cached state.
- **Module-level `_finalizer_refs`** (a `WeakValueDictionary`) is protected by `_refs_lock` in `_ffi.py`.
- When adding new methods to wrapper classes, always acquire `self._lock` before touching `_ptr` or mutable state.
- **Lock ordering**: `Model` methods must release `self._lock` before calling `Project` methods (which acquire the project's lock) to prevent deadlocks. Use the double-checked locking pattern for caches: check cache with lock, compute without lock, write cache with lock.
- This locking is critical for free-threaded Python (PEP 703 / Python 3.13t+ / 3.14t) where the GIL does not serialize access.

#### Testing
- Use `pytest` with `hypothesis` for property-based testing.
- Thread-safety tests live in `tests/test_thread_safety.py`.
- Run from `src/pysimlin`: `uv run pytest tests/ -x`
