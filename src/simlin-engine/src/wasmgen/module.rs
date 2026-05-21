// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Whole-model code generation: lower a `compiler::Module` to a self-contained
//! WebAssembly module that runs an entire simulation in one exported call.
//!
//! The emitted module exports its own linear `memory` and a `run` function.
//! `run` lays the f64 slab out as: a `curr` working chunk, a `next` working
//! chunk, then a results region of `n_chunks` step-major snapshots. It seeds
//! the reserved globals and the initials, then runs the Euler loop, recording
//! a snapshot of `curr` on the same cadence the bytecode VM uses
//! (`vm.rs::run_to`): the t=start sample is forced, then every
//! `save_every = round(save_step/dt)` steps, up to `n_chunks` samples.
//!
//! Unlike the VM's chunk-ring buffer, this uses a single `curr` chunk plus a
//! `next` chunk that holds only the freshly integrated stock values: after
//! recording a snapshot, the updated stocks are copied back into `curr` and
//! time is advanced. Auxiliaries/flows are recomputed each step, so `curr`
//! always holds the full, correct state for the timestep it represents.
//!
//! POC scope: a single scalar root model, Euler integration, no submodules,
//! temp arrays, or array machinery. Anything else returns `WasmGenError`.

use wasm_encoder::Instruction as I;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, ExportKind, ExportSection, Function, FunctionSection,
    GlobalSection, GlobalType, MemorySection, MemoryType, Module as WasmModule, TypeSection,
    ValType,
};

use crate::compiler::{Expr, Module};
use crate::results::{Method, Specs};

use super::WasmGenError;
use super::expr::{EmitCtx, f64_const, lower_expr, memarg};

// Reserved global slots, mirroring `crate::vm`.
const TIME_OFF: usize = 0;
const DT_OFF: usize = 1;
const INITIAL_TIME_OFF: usize = 2;
const FINAL_TIME_OFF: usize = 3;

const SLOT_SIZE: u32 = 8;
const WASM_PAGE_SIZE: u32 = 65536;

// `run`'s i32 locals.
const L_SAVED: u32 = 0;
const L_STEP_ACCUM: u32 = 1;
const L_DST: u32 = 2;

/// Compile a model `Module` into a self-contained wasm module (exports
/// `memory` and `run`). `specs` supplies the integration parameters, baked in
/// as constants.
pub fn compile_module(module: &Module, specs: &Specs) -> Result<Vec<u8>, WasmGenError> {
    if specs.method != Method::Euler {
        return Err(WasmGenError::Unsupported(
            "wasmgen: only Euler integration is supported".to_string(),
        ));
    }
    if module.n_temps != 0 {
        return Err(WasmGenError::Unsupported(
            "wasmgen: temp arrays are not supported".to_string(),
        ));
    }
    if !module.module_refs.is_empty() {
        return Err(WasmGenError::Unsupported(
            "wasmgen: submodules are not supported".to_string(),
        ));
    }

    let too_large =
        || WasmGenError::Unsupported("wasmgen: model too large for the POC".to_string());
    let n_slots = u32::try_from(module.n_slots).map_err(|_| too_large())?;
    let n_chunks = u32::try_from(specs.n_chunks).map_err(|_| too_large())?;
    let stride = n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let curr_base = 0u32;
    let next_base = stride;
    let results_base = stride.checked_mul(2).ok_or_else(too_large)?;
    let results_bytes = n_chunks.checked_mul(stride).ok_or_else(too_large)?;
    let total_bytes = results_base
        .checked_add(results_bytes)
        .ok_or_else(too_large)?;
    let pages = total_bytes.div_ceil(WASM_PAGE_SIZE).max(1);

    // save_every mirrors vm.rs: max(1, round(save_step / dt)).
    let save_every = ((specs.save_step / specs.dt).round() as i64).max(1);
    let save_every = i32::try_from(save_every).map_err(|_| too_large())?;

    let ctx = EmitCtx {
        curr_base,
        next_base,
        dt: specs.dt,
        start_time: specs.start,
        final_time: specs.stop,
    };

    let stock_offsets = collect_assign_next_offsets(&module.runlist_stocks);

    let mut run = Function::new([(3, ValType::I32)]);
    emit_run(
        &mut run,
        module,
        &ctx,
        specs,
        n_slots,
        results_base,
        stride,
        n_chunks,
        save_every,
        &stock_offsets,
    )?;

    Ok(assemble(run, pages, n_slots, n_chunks, results_base))
}

/// Compile the named model of a datamodel `Project` to a self-contained wasm
/// module. Builds the monolithic Expr-runlist module (`compiler::Module`) and
/// derives `Specs` from the project's sim specs. This is the entry point used
/// across the FFI boundary by `libsimlin`.
pub fn compile_datamodel_to_wasm(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
) -> Result<Vec<u8>, WasmGenError> {
    use crate::common::{Canonical, Ident};
    use std::collections::BTreeSet;

    let project = crate::project::Project::from(datamodel.clone());
    if !project.errors.is_empty() {
        return Err(WasmGenError::Unsupported(format!(
            "wasmgen: project has compile errors: {:?}",
            project.errors
        )));
    }

    let canonical = crate::canonicalize(model_name);
    let ident = Ident::<Canonical>::from_str_unchecked(canonical.as_ref());
    let model = project.models.get(&ident).ok_or_else(|| {
        WasmGenError::Unsupported(format!("wasmgen: model '{model_name}' not found"))
    })?;

    let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
    let module = crate::compiler::Module::new(&project, model.clone(), &inputs, true)
        .map_err(|e| WasmGenError::Unsupported(format!("wasmgen: module build failed: {e:?}")))?;

    let specs = Specs::from(&project.datamodel.sim_specs);
    compile_module(&module, &specs)
}

/// The set of stock data-buffer offsets, taken from the `AssignNext` writes in
/// the stocks runlist. After each step these slots are copied `next -> curr`.
fn collect_assign_next_offsets(stocks: &[Expr]) -> Vec<usize> {
    stocks
        .iter()
        .filter_map(|expr| match expr {
            Expr::AssignNext(off, _) => Some(*off),
            _ => None,
        })
        .collect()
}

/// Store a compile-time constant into a `curr` slot.
fn store_curr_const(f: &mut Function, ctx: &EmitCtx, off: usize, v: f64) {
    f.instruction(&I::I32Const(0));
    f.instruction(&f64_const(v));
    f.instruction(&I::F64Store(memarg(
        u64::from(ctx.curr_base) + off as u64 * u64::from(SLOT_SIZE),
    )));
}

/// Emit the body of `run`. The control-flow shape is:
///
/// ```text
/// (block $break
///   (loop $continue
///     br_if $break  (time > stop)
///     <flows> <stocks>
///     step_accum += 1
///     (if (step_accum == save_every) | (saved == 0 & time == start)
///       <snapshot curr -> results[saved]>
///       saved += 1; step_accum = 0
///       br_if $break (saved >= n_chunks))
///     <copy updated stocks next -> curr>
///     time += dt
///     br $continue))
/// ```
#[allow(clippy::too_many_arguments)]
fn emit_run(
    f: &mut Function,
    module: &Module,
    ctx: &EmitCtx,
    specs: &Specs,
    n_slots: u32,
    results_base: u32,
    stride: u32,
    n_chunks: u32,
    save_every: i32,
    stock_offsets: &[usize],
) -> Result<(), WasmGenError> {
    let time_addr = u64::from(ctx.curr_base) + TIME_OFF as u64 * u64::from(SLOT_SIZE);

    // Seed reserved globals, then run the initials.
    store_curr_const(f, ctx, TIME_OFF, specs.start);
    store_curr_const(f, ctx, DT_OFF, specs.dt);
    store_curr_const(f, ctx, INITIAL_TIME_OFF, specs.start);
    store_curr_const(f, ctx, FINAL_TIME_OFF, specs.stop);
    for expr in &module.runlist_initials {
        lower_expr(expr, ctx, f)?;
    }

    f.instruction(&I::Block(BlockType::Empty)); // $break  (depth 1 from loop body)
    f.instruction(&I::Loop(BlockType::Empty)); // $continue (depth 0 from loop body)

    // if time > stop: break
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(time_addr)));
    f.instruction(&f64_const(specs.stop));
    f.instruction(&I::F64Gt);
    f.instruction(&I::BrIf(1));

    for expr in &module.runlist_flows {
        lower_expr(expr, ctx, f)?;
    }
    for expr in &module.runlist_stocks {
        lower_expr(expr, ctx, f)?;
    }

    // step_accum += 1
    f.instruction(&I::LocalGet(L_STEP_ACCUM));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::I32Add);
    f.instruction(&I::LocalSet(L_STEP_ACCUM));

    // save_cond = (step_accum == save_every) | (saved == 0 & time == start)
    f.instruction(&I::LocalGet(L_STEP_ACCUM));
    f.instruction(&I::I32Const(save_every));
    f.instruction(&I::I32Eq);
    f.instruction(&I::LocalGet(L_SAVED));
    f.instruction(&I::I32Eqz);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(time_addr)));
    f.instruction(&f64_const(specs.start));
    f.instruction(&I::F64Eq);
    f.instruction(&I::I32And);
    f.instruction(&I::I32Or);
    f.instruction(&I::If(BlockType::Empty));

    // dst = results_base + saved * stride
    f.instruction(&I::I32Const(results_base as i32));
    f.instruction(&I::LocalGet(L_SAVED));
    f.instruction(&I::I32Const(stride as i32));
    f.instruction(&I::I32Mul);
    f.instruction(&I::I32Add);
    f.instruction(&I::LocalSet(L_DST));

    // results[dst + slot*8] = curr[slot]   for every slot
    for slot in 0..n_slots {
        f.instruction(&I::LocalGet(L_DST));
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(memarg(
            u64::from(ctx.curr_base) + u64::from(slot) * u64::from(SLOT_SIZE),
        )));
        f.instruction(&I::F64Store(memarg(u64::from(slot) * u64::from(SLOT_SIZE))));
    }

    // saved += 1; step_accum = 0
    f.instruction(&I::LocalGet(L_SAVED));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::I32Add);
    f.instruction(&I::LocalSet(L_SAVED));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::LocalSet(L_STEP_ACCUM));

    // if saved >= n_chunks: break (depth 2: if -> loop -> block)
    f.instruction(&I::LocalGet(L_SAVED));
    f.instruction(&I::I32Const(n_chunks as i32));
    f.instruction(&I::I32GeS);
    f.instruction(&I::BrIf(2));

    f.instruction(&I::End); // end if

    // Advance: copy the freshly integrated stock values next -> curr.
    for &off in stock_offsets {
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(memarg(
            u64::from(ctx.next_base) + off as u64 * u64::from(SLOT_SIZE),
        )));
        f.instruction(&I::F64Store(memarg(
            u64::from(ctx.curr_base) + off as u64 * u64::from(SLOT_SIZE),
        )));
    }

    // time += dt
    f.instruction(&I::I32Const(0));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(time_addr)));
    f.instruction(&f64_const(specs.dt));
    f.instruction(&I::F64Add);
    f.instruction(&I::F64Store(memarg(time_addr)));

    f.instruction(&I::Br(0)); // continue
    f.instruction(&I::End); // end loop
    f.instruction(&I::End); // end block
    f.instruction(&I::End); // end function
    Ok(())
}

/// Assemble the final module: type, function, memory, globals, exports, code.
///
/// Three immutable i32 globals (`n_slots`, `n_chunks`, `results_offset`) make
/// the results region self-describing, so a host can locate and stride it
/// without any external layout metadata.
fn assemble(run: Function, pages: u32, n_slots: u32, n_chunks: u32, results_base: u32) -> Vec<u8> {
    let mut wasm = WasmModule::new();

    let mut types = TypeSection::new();
    types.ty().function([], []); // run: () -> ()
    wasm.section(&types);

    let mut functions = FunctionSection::new();
    functions.function(0);
    wasm.section(&functions);

    let mut memories = MemorySection::new();
    memories.memory(MemoryType {
        minimum: u64::from(pages),
        maximum: None,
        memory64: false,
        shared: false,
        page_size_log2: None,
    });
    wasm.section(&memories);

    let i32_global = || GlobalType {
        val_type: ValType::I32,
        mutable: false,
        shared: false,
    };
    let mut globals = GlobalSection::new();
    globals.global(i32_global(), &ConstExpr::i32_const(n_slots as i32));
    globals.global(i32_global(), &ConstExpr::i32_const(n_chunks as i32));
    globals.global(i32_global(), &ConstExpr::i32_const(results_base as i32));
    wasm.section(&globals);

    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, 0);
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("n_slots", ExportKind::Global, 0);
    exports.export("n_chunks", ExportKind::Global, 1);
    exports.export("results_offset", ExportKind::Global, 2);
    wasm.section(&exports);

    let mut code = CodeSection::new();
    code.function(&run);
    wasm.section(&code);

    wasm.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::{Canonical, Ident};
    use crate::compat::open_xmile;
    use crate::db::{SimlinDb, compile_project_incremental, sync_from_datamodel_incremental};
    use crate::vm::Vm;
    use checked::Store;
    use std::collections::BTreeSet;
    use std::io::BufReader;
    use std::sync::Arc;
    use wasm::validate;

    const POPULATION_XMILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../default_projects/population/model.xmile"
    );

    #[test]
    fn population_wasm_matches_vm() {
        let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
        let mut reader = BufReader::new(file);
        let datamodel = open_xmile(&mut reader).expect("parse population xmile");

        let specs = Specs::from(&datamodel.sim_specs);

        // VM golden via the production incremental path.
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, &datamodel, None);
        let compiled =
            compile_project_incremental(&db, sync.project, "main").expect("incremental compile");
        let mut vm = Vm::new(compiled).expect("vm creation");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();

        // Monolithic Expr-runlist module -> wasm.
        let project = Arc::new(crate::project::Project::from(datamodel));
        assert!(
            project.errors.is_empty(),
            "project has errors: {:?}",
            project.errors
        );
        let main_ident = Ident::<Canonical>::from_str_unchecked("main");
        let model = project.models.get(&main_ident).expect("main model");
        let inputs: BTreeSet<Ident<Canonical>> = BTreeSet::new();
        let module = crate::compiler::Module::new(&project, model.clone(), &inputs, true)
            .expect("build monolithic module");

        let n_slots = module.n_slots;
        let n_chunks = specs.n_chunks;
        let results_base = 2 * n_slots * 8;

        let wasm_bytes = compile_module(&module, &specs).expect("wasm codegen");

        // Execute the generated module under the interpreter.
        let info = validate(&wasm_bytes).expect("generated module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let run = store
            .instance_export(inst, "run")
            .unwrap()
            .as_func()
            .unwrap();
        store
            .invoke_simple_typed::<(), ()>(run, ())
            .expect("run wasm");

        let mem = store
            .instance_export(inst, "memory")
            .unwrap()
            .as_mem()
            .unwrap();
        let wasm_data: Vec<f64> = store.mem_access_mut_slice(mem, |bytes| {
            (0..n_chunks * n_slots)
                .map(|i| {
                    let a = results_base + i * 8;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
                .collect()
        });

        // Compare every shared variable's full time series.
        assert_eq!(
            vm_results.step_count, n_chunks,
            "saved-chunk count differs from VM"
        );
        let main_offsets = module.offsets.get(&main_ident).expect("main offsets");

        let mut checked_vars = 0usize;
        for (var, &vm_off) in &vm_results.offsets {
            let Some(&(wasm_off, _size)) = main_offsets.get(var) else {
                continue;
            };
            for c in 0..n_chunks {
                let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
                let wasm_val = wasm_data[c * n_slots + wasm_off];
                let diff = (vm_val - wasm_val).abs();
                assert!(
                    diff < 1e-9,
                    "{} mismatch at chunk {c}: vm={vm_val} wasm={wasm_val} (diff {diff})",
                    var.as_str()
                );
            }
            checked_vars += 1;
        }

        assert!(
            checked_vars >= 5,
            "expected to compare the population model's variables, only checked {checked_vars}"
        );
        let pop = Ident::<Canonical>::from_str_unchecked("population");
        assert!(
            main_offsets.contains_key(&pop) && vm_results.offsets.contains_key(&pop),
            "the population stock should have been compared"
        );
    }
}
