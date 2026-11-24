# simlin-wasmgen

The WebAssembly backend, the alternative to the bytecode VM: it lowers a compiled model to one self-contained wasm module that runs the whole simulation.
It is built on [simlin-engine](/src/simlin-engine/CLAUDE.md), whose `CompiledSimulation` (the VM's own input) is what it lowers; the crate's rustdoc (`src/lib.rs`) says what it covers and what it refuses.
The root `CLAUDE.md` holds the development rules; this file is a map of the standing invariants code in this crate keeps (paths are relative to this crate's `src/`).

- `lib.rs` is the entry (`compile_datamodel_to_artifact`, `compile_simulation`, `compile_simulation_with_plans`); `module.rs` emits the module and the run's drivers, `lower.rs` an opcode program, `split.rs` partitions a large phase body, `views.rs` and `vector.rs` lower the array views and vector operations, `lookup.rs`, `math.rs` and `alloc.rs` the helpers a module carries.
- `errors.rs` is the emitted module's run-time error channel, which a host reads with `decode_error_word` and `reconstruct_error`.
- **The wasm backend has no VM fallback.** A construct it cannot lower correctly is refused with `WasmGenError::Unsupported`.
- **The VM is the correctness oracle for wasmgen.** Every emitted module is run under a pure-Rust wasm interpreter in tests and compared with `Vm::run_to_end`; the corpus in `src/simlin-tests/tests/integration/simulate.rs` does this for every VM-simulated model (`ensure_wasm_matches`), and `simulate_ltm_wasm` does it for every exposed key of an LTM run.
- The wasm backend lowers both special-stock passes as unrolled plan-specialized code (`belt.rs`, `passes.rs`) and shares the VM's error-message constructors so the two backends cannot drift.

Tests: every `*_tests.rs` beside its code, with simlin-formats (to open models) and simlin-engine's `test-support` as dev-dependencies.
