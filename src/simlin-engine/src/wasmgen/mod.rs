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
//! lookups), Euler/RK2/RK4 integration, nested modules (incl. SMOOTH/DELAY
//! stdlib expansions), QUEUE models (whose per-step FIFO side-table pass is
//! hand-lowered by `passes`), and CONVEYOR models (whose per-DT belt pass --
//! leaks, zones, discrete admission, `<sample>`/`<arrest>`, container access,
//! and queue coupling -- is hand-lowered by `belt`) are in place. A genuine
//! runtime view range (`ViewRangeDynamic`), array unrolling past the
//! per-function budget, or a conveyor inflow carrying a non-default
//! `isee:spreadflow` (`even`/`dest`/`dist`/`source`, §8, GH #946 -- see
//! `belt::reject_unsupported`) returns `WasmGenError::Unsupported`.
//!
//! The PUBLIC entries (`compile_datamodel_to_artifact`/`compile_datamodel_to_wasm`)
//! route a conveyor model through the same `queue_compile::compile_sim` dispatch the
//! VM takes (GH #924 removed the up-front reject), so there is no test-only seam:
//! the belt lowering is reachable exactly where `libsimlin`'s
//! `simlin_model_compile_to_wasm` reaches it.
//!
//! Two error channels, at two different times. `WasmGenError` is a COMPILE-time
//! rejection: the backend refuses to emit a module it cannot lower correctly.
//! `errors` is the emitted module's RUN-time channel (GH #921): every blob
//! exports `get_error() -> i64`, which a host unpacks with [`decode_error_word`]
//! and turns back into the bytecode VM's exact `(ErrorCode, String)` with
//! [`reconstruct_error`]. Since GH #924 a SHIPPED model can set it: the conveyor
//! belt pass raises `ConveyorTransitNotPositive` / `ConveyorTransitTooLong`, and a
//! belt model now reaches the blob through a public entry. A host that RUNS a blob
//! must therefore poll the getter -- a queue-only or ordinary model still never
//! raises, and its blob elides the guards entirely.

mod alloc;
mod belt;
mod errors;
mod lookup;
mod lower;
mod math;
mod module;
mod passes;
mod vector;
mod views;

pub use errors::{BlobError, decode_error_word, reconstruct_error};
pub use module::{
    WasmArtifact, WasmLayout, compile_datamodel_to_artifact, compile_datamodel_to_wasm,
    compile_simulation, compile_simulation_with_plans,
};

use std::fmt;

/// Error from the WebAssembly code-generation backend.
///
/// The backend covers the full scalar + array opcode set, Euler/RK2/RK4
/// integration, nested modules (including SMOOTH/DELAY stdlib expansions), and both
/// special stock types -- queue models and conveyor models. Four constructs return
/// `Unsupported` rather than silently emitting an incorrect module: a genuine
/// runtime view range (`ViewRangeDynamic`), array unrolling past the per-function
/// budget, and the two conditions `belt::reject_unsupported` refuses -- a conveyor
/// inflow carrying a non-default `isee:spreadflow` (`even`/`dest`/`dist`/`source`,
/// §8, GH #946), and a `conveyor::slat_bound()` above `i32::MAX` (a soundness guard
/// on the emitted `i32.trunc_f64_s` narrowings, not a feature gap).
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
