# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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

## Architecture Overview

Simlin is a system dynamics modeling tool.  It can be used to build and simulate system dynamics models, including models created in other software like Vensim (from Ventana Systems) and Stella (from isee systems).  There is a model save file/interchange format called XMILE, and the XMILE specification is in `doc/xmile-v1.0.html`.  It covers general concepts like how simulation works and how arrays and subscripting notation work as well as details of the XML structure and equation syntax.  It is a crucial resource to consult when adding functionality to the simulation engine.  

The simulation engine for simulating system dynamics stock and flow models is written in Rust, and the interactive model editor (and other components like the web server and model creation/browsing) are written in TypeScript.

### Core Components

**simlin-engine (Rust)**: The simulation engine that compiles and executes system dynamics models
- Entry point: `src/simlin-engine/src/lib.rs`
- Equation text is parsed into an AST using LALRPOP parser (`equation.lalrpop`)
- Projects consisting of models (including built-in models like SMOOTH) are compiled to a simple bytecode format (`compiler.rs`)
- Executes simulations using a bytecode-based virtual machine (`vm.rs`)
- Supports unit checking and dimensional analysis
- Contains built-in functions library (`builtins.rs`)

**engine (Rust → WASM)**: WebAssembly bindings for the simulation engine
- Wraps simlin-engine for JavaScript consumption
- Built to `src/engine/core/` as WASM modules

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
- `src/simlin-compat` - Helpers to convert between our internal project representation and XMILE + Vensim formats, and open various types of result data formats (like CSVs).
- `src/importer` - Expose simlin-compat import functionality to JavaScript with wasm-bindgen
- `src/simlin-cli` - a command line tool for simulating system dynamics models, mostly for testing/debugging.
- `src/xmutil` - Rust wrapper around Bob Eberlein's tool to convert Vensim models to XMILE format, including diagrams.
- `src/engine` - Expose simlin-engine functionality to JavaScript with wasm-bindgen
- `src/simlin-engine` - Core simulation engine

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

### Testing Strategy

- Rust: Unit tests in `src/*/tests/` and integration tests in `test/` directory
- TypeScript: Workspace-level linting and type checking
- Models: Extensive test suite in `test/` with expected outputs.  This is very important and ensures the engine behavior matches known-good results from other software.