// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! WebAssembly code-generation backend (proof of concept).
//!
//! This backend is an alternative to the bytecode VM (`crate::vm`). Instead of
//! interpreting opcodes, it lowers a model's resolved `compiler::expr::Expr` IR
//! into a self-contained WebAssembly module that runs the whole simulation in
//! one exported call, writing results into its own linear memory. The intended
//! use case is interactive scrubbing: compile a model to wasm once, then
//! re-run it on every slider change at display refresh rates.
//!
//! Modules are emitted with the `wasm-encoder` crate. Correctness is validated
//! in tests by executing the emitted module under the DLR-FT `wasm-interpreter`
//! and comparing the results against the bytecode VM.
//!
//! Status: expression lowering (M1) is in place; whole-model assembly and the
//! integration loop land in subsequent milestones.

mod expr;
mod module;

pub use module::{compile_datamodel_to_wasm, compile_module};

use std::fmt;

/// Error from the WebAssembly code-generation backend.
///
/// The proof-of-concept backend covers only the scalar IR subset exercised by
/// simple flow/stock models. Anything outside that surface (arrays, modules,
/// table lookups, and the builtins that require runtime helpers such as `pow`)
/// returns `Unsupported` rather than silently emitting an incorrect module.
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

#[cfg(test)]
mod tests {
    use checked::Store;
    use wasm::validate;
    use wasm_encoder::{
        CodeSection, ExportKind, ExportSection, Function, FunctionSection, Instruction, Module,
        TypeSection, ValType,
    };

    /// Emit a minimal module exporting `add(f64, f64) -> f64`.
    ///
    /// This exercises the full emit path (type/function/export/code sections)
    /// end to end; it is a stand-in that the real expression-lowering codegen
    /// will replace.
    fn emit_add_module() -> Vec<u8> {
        let mut module = Module::new();

        let mut types = TypeSection::new();
        types
            .ty()
            .function([ValType::F64, ValType::F64], [ValType::F64]);
        module.section(&types);

        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);

        let mut exports = ExportSection::new();
        exports.export("add", ExportKind::Func, 0);
        module.section(&exports);

        let mut code = CodeSection::new();
        let mut func = Function::new([]);
        func.instruction(&Instruction::LocalGet(0));
        func.instruction(&Instruction::LocalGet(1));
        func.instruction(&Instruction::F64Add);
        func.instruction(&Instruction::End);
        code.function(&func);
        module.section(&code);

        module.finish()
    }

    /// The load-bearing M0 smoke test: a module emitted by `wasm-encoder`
    /// validates and executes correctly under the DLR-FT interpreter, with
    /// f64 arguments and an f64 result crossing the host boundary.
    #[test]
    fn add_module_runs_under_interpreter() {
        let wasm_bytes = emit_add_module();

        let validation_info = validate(&wasm_bytes).expect("emitted module must validate");
        let mut store = Store::new(());
        let module = store
            .module_instantiate(&validation_info, Vec::new(), None)
            .expect("emitted module must instantiate")
            .module_addr;
        let add = store
            .instance_export(module, "add")
            .expect("add export must exist")
            .as_func()
            .expect("add export must be a function");

        let result: f64 = store
            .invoke_simple_typed(add, (2.5_f64, 4.0_f64))
            .expect("invocation must succeed");
        assert_eq!(result, 6.5_f64);
    }
}
