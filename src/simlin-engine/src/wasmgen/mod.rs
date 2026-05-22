// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! WebAssembly code-generation backend.
//!
//! This backend is an alternative to the bytecode VM (`crate::vm`). Instead of
//! interpreting opcodes, it lowers a salsa-compiled `CompiledSimulation` (the
//! VM's own input) into a self-contained WebAssembly module that runs the whole
//! simulation in one exported call, writing results into its own linear memory.
//! The intended use case is interactive scrubbing: compile a model to wasm
//! once, then re-run it on every slider change at display refresh rates.
//!
//! The backend walks every module instance's un-fused opcode programs
//! (`compiled_initials`/`compiled_flows`/`compiled_stocks`) and emits a wasm
//! function-triple per `(model, input_set)` instance plus a `run` driver (see
//! `lower` for the per-opcode lowering and `module` for whole-model assembly).
//! Modules are emitted with the `wasm-encoder` crate; correctness is validated
//! in tests by executing the emitted module under the DLR-FT `wasm-interpreter`
//! and comparing against the bytecode VM.
//!
//! Status: the full scalar + array opcode set (every `Op2` operator, every
//! `Apply` builtin, the view/reducer/iteration/vector ops, scalar/array
//! lookups), Euler/RK2/RK4 integration, and nested modules (incl. SMOOTH/DELAY
//! stdlib expansions) are in place. A genuine runtime view range
//! (`ViewRangeDynamic`) or array unrolling past the per-function budget returns
//! `WasmGenError::Unsupported`.

mod alloc;
mod lookup;
mod lower;
mod math;
mod module;
mod vector;
mod views;

pub use module::{WasmArtifact, WasmLayout, compile_datamodel_to_wasm, compile_simulation};

use std::fmt;

/// Error from the WebAssembly code-generation backend.
///
/// The backend covers the full scalar + array opcode set, Euler/RK2/RK4
/// integration, and nested modules (including SMOOTH/DELAY stdlib expansions).
/// A genuine runtime view range (`ViewRangeDynamic`) or array unrolling past the
/// per-function budget returns `Unsupported` rather than silently emitting an
/// incorrect module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmGenError {
    Unsupported(String),
}

impl fmt::Display for WasmGenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WasmGenError::Unsupported(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for WasmGenError {}
