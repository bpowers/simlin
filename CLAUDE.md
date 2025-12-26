# CLAUDE.md

This file provides guidance to AI agents when working with code in this repository.

## Overview

Simlin is a system dynamics modeling tool.
It can be used to build and simulate system dynamics models, including models created in other software like Vensim (from Ventana Systems) and Stella (from isee systems).
There is a model save file/interchange format called XMILE, and the XMILE specification is in `doc/xmile-v1.0.html`.
It covers general concepts like how simulation works and how arrays and subscripting notation work as well as details of the XML structure and equation syntax.
It is a crucial resource to consult when adding functionality to the simulation engine.

The simulation engine for simulating system dynamics stock and flow models is written in Rust, and the interactive model editor (and other components like the web server and model creation/browsing) are written in TypeScript.


## Architecture Overview

### Core Components

**simlin-engine (Rust)**: The simulation engine that compiles, edits, and executes system dynamics models
- Entry point: `src/simlin-engine/src/lib.rs`
- Equation text is parsed into an AST using LALRPOP parser (`equation.lalrpop`)
- Projects consisting of models are compiled to a simple bytecode format (`compiler.rs`)
- Executes simulations using a bytecode-based virtual machine (`vm.rs`)
- Supports unit checking and dimensional analysis
- Contains built-in functions library (`builtins.rs`) of _models_ that implement stateful "functions" like TREND and SMOOTH3

**engine (Rust → WASM)**: WebAssembly bindings for the simulation engine
- Wraps simlin-engine for JavaScript consumption
- Built to `src/engine/core/` as WASM modules

**libsimlin (Rust → C FFI)**: C FFI interface for the simulation engine
- Wraps simlin-engine and simlin-compat for consumption from C or Go

**core (TypeScript)**: Shared data models and common utilities
- Defines protobuf-based data model (`datamodel.ts`)
- Canonical variable name handling (`canonicalize.ts`)
- Used by both frontend and backend

**diagram (TypeScript)**: React components for model visualization and editing
- Main editor: `Editor.tsx`
- Drawing components in `drawing/` subdirectory
- Handles user interactions for model construction

**app (TypeScript)**: React frontend application
- Main components: `App.tsx`, `Home.tsx`, `Project.ts`
- Builds both regular app and web component versions
- Uses webpack for bundling

**server (TypeScript)**: Express.js backend API
- Authentication via Firebase Auth (`authn.ts`)
- Firestore database integration (`models/db-firestore.ts`)
- Model rendering and export services (`render.ts`)

### Data Flow

1. **Model Import**: XMILE/Vensim files processed by `importer` (Rust → WASM)
2. **Model Storage**: Protobuf format in Firestore via `core` data models  
3. **Simulation**: Engine compiles model equations and executes simulation
4. **Visualization**: Frontend renders results using `diagram` components

### Key File Locations

- Protobuf schemas: `src/simlin-engine/src/project_io.proto`, `src/server/schemas/*.proto`
- Standard library models: `stdlib/*.stmx` → compiled to `src/simlin-engine/src/stdlib/*.pb`
- Test models: `test/` directory with various model formats
- Build scripts: Individual `build.sh` files in Rust WASM crates

### Workspace Structure

There are two logical workspaces in this one repo.

First is the Rust workspace with these packages:
- `src/simlin-engine` - Core simulation engine
- `src/simlin-compat` - Helpers to convert between our internal project representation and XMILE + Vensim formats, and open various types of result data formats (like CSVs).
- `src/libsimlin` - C-compatible FFI interface to simlin-engine for language-agnostic access via WebAssembly
- `src/importer` - Expose simlin-compat import functionality to JavaScript with wasm-bindgen
- `src/engine` - Expose simlin-engine functionality to JavaScript with wasm-bindgen
- `src/simlin-cli` - a command line tool for simulating system dynamics models, mostly for testing/debugging.
- `src/xmutil` - Rust wrapper around Bob Eberlein's tool to convert Vensim models to XMILE format, including diagrams.

This is a yarn workspace with these packages:
- `@system-dynamics/core` - Shared TypeScript utilities
- `@system-dynamics/diagram` - React diagram components  
- `@system-dynamics/app` - Frontend application
- `@system-dynamics/server` - Backend API server
- `@system-dynamics/engine` - WASM simulation engine
- `@system-dynamics/importer` - WASM model import utilities
- `@system-dynamics/xmutil` - WASM XML utilities

### Prerequisites for Development

- Google Cloud CLI with Firestore emulator
- wasm-bindgen CLI tool (`cargo install wasm-bindgen-cli`)
- Node.js and Yarn
- Rust toolchain (specified in `rust-toolchain.toml`)

### Initial Environment Setup

For Claude Code on the web, Codex Web, or any fresh checkout, run the initialization script:

```bash
./scripts/cloud-init.sh
```

**Important**: This script should be run any time the development environment is initialized or re-initialized (e.g., when a new container session starts, after a fresh clone, or when resuming work in a cloud environment). Running this script ensures the environment is properly configured so that you and other agents can be successful and productive.

This script:
- Initializes git submodules (required for test models)
- Installs git pre-commit hooks
- Verifies required tools are available
- Installs yarn dependencies if needed
- Configures AI tools (Claude CLI or Codex) for pre-commit checks

### Pre-commit Hooks

The pre-commit hook (`scripts/pre-commit`) runs automatically before each commit and performs:
1. Rust formatting check (`cargo fmt --check`)
2. Rust linting (`cargo clippy`)
3. Rust tests (`cargo test`)
4. TypeScript/JavaScript linting (`yarn lint`)
5. TypeScript type checking (`yarn tsc`)
6. Python bindings tests (requires Python 3.11+)
7. AI-powered test quality verification (checks for incomplete/stubbed tests)

If any check fails, the commit is rejected. Fix the issues and try again.

**Important**: Never use `--no-verify` to skip hooks. The hooks exist to maintain code quality.

**AI Tool Selection**: The test quality check uses Claude CLI or OpenAI Codex CLI:
- Set via `AI_TOOL` environment variable (`claude` or `codex`)
- Or auto-configured by `./scripts/cloud-init.sh` (stored in `.ai-tool-config`)
- Claude is preferred when available; Codex is the fallback
- Has a 5-minute timeout (configurable via `AI_TIMEOUT`)
- Skips gracefully if no AI tool is available or if it times out

**Note**: When running `git commit`, use a 5+ minute timeout since the pre-commit hook runs comprehensive checks including tests.

## Development Commands

### Build Commands
- `yarn build` - Build the web application and System Dynamics model editor, including compiling the simulation engine to WebAssembly.
- `yarn clean` - Clean all build artifacts  
- `cargo build` - Build Rust components
- `cargo fmt` - Format Rust code
- `yarn format` - Format both JavaScript/TypeScript and Rust code

### Development Server
Start these commands in 3 separate terminals:
```bash
yarn start:firestore  # Start local Firestore emulator (port 8092)
yarn start:backend    # Start backend server (port 3030) 
yarn start:frontend   # Start frontend dev server (port 3000)
```

### Linting and Type Checking
- `yarn lint` - Run linters for all workspaces (includes `cargo clippy`)
- `cargo clippy` - Run Rust clippy linter only
- `yarn precommit` - Run format checks and linting (used by git hooks)

### Testing
- `cargo test` - Run Rust tests: the simulation engine is in Rust.
- most of the TypeScript code is related to an interactive web editor and doesn't have tests at this time.

### Protobuf Generation
- `yarn build:gen-protobufs` - Regenerate protobuf TypeScript bindings from .proto files

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

When working on this codebase, follow this systematic approach:

### 0. Problem-Solving Philosophy

- **Write high-quality, general-purpose solutions**: Implement solutions that work correctly for all valid inputs, not just test cases. Do not hard-code values or create solutions that only work for specific test inputs.
- **Prioritize the right approach over the first approach**: Research the proper way to implement features rather than implementing workarounds. For example, check if an API provides token usage directly before implementing token counting. If you are not sure, explore several approaches and then choose the most promising one (or ask the user for their input if one isn't clearly best).
- **Keep implementations simple and maintainable**: Start with the simplest solution that meets requirements. Only add complexity when the simple approach demonstrably fails.
- **No special casing in tests**: Tests should hold all implementations to the same standard. Never add conditional logic in tests that allows certain implementations to skip requirements.
- **Complete all aspects of a task**: When fixing bugs or implementing features, ensure the fix works for all code paths, not just the primary one. Continue making progress until all aspects of the user's task are done, including newly discovered but necessary work and rework along the way.
- **Test-driven development**: When working on a task or todo-list item, start by writing a unit test with the expected behavior — it will initially fail. Use that to help guide your implementation, which should eventually get the unit test passing after some iteration.
- **Commit with descriptive messages** strictly following the commit message style from above when you complete a unit of work, like a task or major TODO list item.

### 1. Understanding Requirements
- Read relevant code and documentation (including for libraries) and build a plan based on the user's task.
- If there are important and ambiguous high-level architecture decisions or "trapdoor" choices, stop and ask the user.
- Start by adding tests to validate assumptions before making changes.
- Remember: we want to build the simplest interfaces and abstractions possible while FULLY addressing the task and requirements in full generality.

### 2. Multi-Step Task Execution
For complex tasks with multiple components:
1. **Break down into discrete tasks** and track with a todo list
2. **Complete each task fully** including tests and formatting before moving to the next
3. **Commit each logical change separately** with clear commit messages
4. **Boldly refactor** when needed - there's no legacy code to preserve
5. **Address the root cause** if you find that a problem is due to a bad abstraction or deficiency in the current code, stop and create a plan to directly address it. Do not work around it by skipping tests or leaving part of the user's task unaddressed.

## Development Guide

### Rust Development Standards

Follow these steps when working on code changes in Rust crates like `src/simlin-engine`:

- To run specific tests use the form `RUST_BACKTRACE=1 cargo test -p $crate_name $test_name`. This command form is allowlisted, deviating from this is strongly discouraged and will slow progress.
   - DO NOT write one-off rust files and compile them with `rustc` to test hypotheses and assumptions. Instead, write new unit tests as close to the source of the problem as possible. These unit tests are valuable additions to the test suite and should be left at the end of the task so that the user can review your assumptions.
- **Strongly** prefer `.unwrap()` over `.unwrap_or_default()` (or one of the other ways to provide a value when unwrap fails). During this phase of development, it is valuable to understand when our assumptions are wrong, and using a default/0/1 fixed value hides that.
- If a case (for example in a match arm) is expected to be unreachable, use the `unreachable!()` macro not a code comment.
- Code should never have comments like "this is a placeholder". If you have stubbed something out, that should be documented in code via the use of the `todo!()` or `unimplemented!()` macros. But generally this means your current task is not complete! Continue working until you have a general-purpose, maintainable solution that can be confidently deployed to a production environment.
- Similarly, tests should err on the side of brittleness. For example, if you are missing a required test file, loudly fail the test rather than skipping the test.
- Run `cargo fmt` one last time.
- Commit your changes following the above commit message style guidance.

### TypeScript/React Development Standards

#### Code Style
- Use TypeScript with strict mode enabled
- Prefer class components — AVOID hooks like useState
- Use proper TypeScript types, avoid `any`
- Run `yarn lint` before committing
- Run `yarn tsc` to check types
- Especially when working on the TypeScript side of the project, do NOT manually copy files around to get builds or tests passing. If there is some sort of regression or error where source files are not able to imported or used, ultrathink to understand why and fix the build scripts.

## Testing Strategy

- Rust: Unit tests in `src/*/tests/` and integration tests in `test/` directory.
- TypeScript: Workspace-level linting and type checking
- Models: Extensive test suite in `test/` with expected outputs. This is very important and ensures the engine behavior matches known-good results from other software.

## General Guidelines

* NEVER create files unless they're absolutely necessary for achieving your goal.
* ALWAYS prefer editing an existing file to creating a new one.
* NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested by the User.

## Common Pitfalls to Avoid

### Rust Development
- Don't assume specific order of operations beyond what's documented
- Always handle error cases explicitly with proper error types
- Be careful with floating point comparisons in tests

### TypeScript Development
- Don't forget to rebuild after changes: `yarn build`
- The development server auto-reloads, but the production build does not
- Ensure proper typing for all components and functions

### System Integration
- The Firestore emulator must be running for backend functionality
- WASM modules need to be rebuilt when Rust code changes
- Protobuf schemas must be regenerated when `.proto` files change
