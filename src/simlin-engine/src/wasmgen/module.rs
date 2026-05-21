// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: a `CompiledSimulation` (or datamodel routed through the
// in-memory salsa compile) in, a self-contained wasm module (`Vec<u8>`) plus its
// `WasmLayout` out. No filesystem/network I/O; tests execute the result under
// the DLR-FT interpreter.

//! Whole-model code generation: lower a salsa-compiled `CompiledSimulation` to
//! a self-contained WebAssembly module that runs an entire simulation in one
//! exported call.
//!
//! The emitted module exports its own linear `memory`, a `run` function, and
//! three i32 geometry globals (`n_slots`/`n_chunks`/`results_offset`). It emits
//! the three opcode programs (`initials`/`flows`/`stocks`) as wasm functions
//! over the shared slab (each lowered by [`super::lower::emit_bytecode`]), then
//! a `run` function that seeds the reserved globals, calls the initials, and
//! drives the Euler loop. `run` lays the slab out as: a `curr` working chunk, a
//! `next` working chunk, then a results region of `n_chunks` step-major
//! snapshots. It records a snapshot of `curr` on the same cadence the bytecode
//! VM uses (`vm.rs::run_to`): the t=start sample is forced, then every
//! `save_every = round(save_step/dt)` steps, up to `n_chunks` samples.
//!
//! Unlike the VM's chunk-ring buffer, this uses a single `curr` chunk plus a
//! `next` chunk that holds only the freshly integrated stock values: after
//! recording a snapshot, the updated stocks are copied back into `curr` and
//! time is advanced. Auxiliaries/flows are recomputed each step, so `curr`
//! always holds the full, correct state for the timestep it represents.
//!
//! Current scope: a single scalar root model, Euler integration, no submodules,
//! temp arrays, or array machinery. Anything else returns `WasmGenError`.

use wasm_encoder::Instruction as I;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, MemorySection, MemoryType, Module as WasmModule,
    TypeSection, ValType,
};

use crate::bytecode::{ByteCode, CompiledModule, Opcode};
use crate::results::{Method, Specs};
use crate::vm::CompiledSimulation;

use super::WasmGenError;
use super::lower::{self, BuiltHelpers, build_helpers, f64_const, max_condition_depth, memarg};

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

/// Compile the named model of a datamodel `Project` to a self-contained wasm
/// module, through the salsa incremental pipeline and [`compile_simulation`].
///
/// This is the entry point used across the FFI boundary by `libsimlin`. The
/// `WasmLayout` is dropped here (only the raw bytes are returned); Phase 7
/// surfaces it through the FFI. The signature is kept stable so `libsimlin` and
/// the `wasm-backend-poc.mjs` exploratory script keep building.
pub fn compile_datamodel_to_wasm(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
) -> Result<Vec<u8>, WasmGenError> {
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, datamodel, None);
    let sim =
        crate::db::compile_project_incremental(&db, sync.project, model_name).map_err(|e| {
            WasmGenError::Unsupported(format!("wasmgen: incremental compile failed: {e:?}"))
        })?;
    Ok(compile_simulation(&sim)?.wasm)
}

// ============================================================================
// CompiledSimulation -> wasm (the production path; consumes salsa bytecode)
// ============================================================================

/// A compiled simulation wasm module together with the layout metadata a host
/// needs to read its results by variable name.
pub struct WasmArtifact {
    pub wasm: Vec<u8>,
    pub layout: WasmLayout,
}

/// Geometry + variable-offset map describing a [`WasmArtifact`]'s results
/// region. The wasm module also exports `n_slots`/`n_chunks`/`results_offset`
/// as i32 globals so a host can stride results with no external metadata; this
/// struct mirrors those values and adds the canonical-name -> slot map needed
/// for by-name reads.
pub struct WasmLayout {
    pub n_slots: usize,
    pub n_chunks: usize,
    /// Byte offset of the results region within linear memory.
    pub results_offset: usize,
    /// Byte offset of the GF directory region (8 bytes/entry, indexed by global
    /// table index: `(data_byte_offset: i32, n_points: i32)`). Zero when the
    /// model has no graphical functions.
    pub gf_directory_offset: usize,
    /// Byte offset of the GF data region (every table's `(x,y)` knots as
    /// consecutive f64 LE pairs). Zero when the model has no graphical
    /// functions.
    pub gf_data_offset: usize,
    /// Canonical variable name -> slot offset within a chunk.
    pub var_offsets: Vec<(String, usize)>,
}

// GF region geometry. The directory holds one 8-byte entry per global table
// index (two i32: the table's absolute data byte offset, and its point count);
// the data region holds every table's knots as consecutive f64 LE `(x, y)`
// pairs (16 bytes/point).
const GF_DIRECTORY_ENTRY_BYTES: u32 = 8; // i32 data_offset + i32 n_points
const GF_KNOT_BYTES: u32 = 16; // f64 x + f64 y

/// The two read-only graphical-function regions for a model, laid out at a
/// caller-chosen `region_base` byte offset within the module's linear memory.
///
/// `directory_base` == `region_base`; the data region follows the directory.
/// Each directory entry's first i32 is the *absolute* byte offset of its
/// table's first knot (so the lookup helpers can `f64.load` a knot with no
/// further base arithmetic); the second i32 is the table's point count. The
/// concatenation order is the global table order in
/// `ByteCodeContext.graphical_functions`, so the `Lookup` opcode's
/// `base_gf + element_offset` indexes directly into the directory.
struct GfRegions {
    directory_base: u32,
    data_base: u32,
    /// `directory` ++ `data` would be the full image, but they are kept
    /// separate so each can be emitted as its own active `DataSection` segment
    /// at its own base.
    directory: Vec<u8>,
    data: Vec<u8>,
    /// Total byte span of both regions (directory + data), for growing `pages`.
    total_bytes: u32,
}

/// Build the GF directory + data regions for `tables` (the root's
/// `graphical_functions`) at `region_base`. Returns `None` (no regions, no
/// growth) when there are no tables. Returns a layout error if the regions
/// would overflow a u32 byte address.
fn build_gf_regions(
    tables: &[Vec<(f64, f64)>],
    region_base: u32,
) -> Result<Option<GfRegions>, WasmGenError> {
    if tables.is_empty() {
        return Ok(None);
    }
    let too_large =
        || WasmGenError::Unsupported("wasmgen: graphical functions too large".to_string());

    let n_tables = u32::try_from(tables.len()).map_err(|_| too_large())?;
    let directory_bytes = n_tables
        .checked_mul(GF_DIRECTORY_ENTRY_BYTES)
        .ok_or_else(too_large)?;
    let directory_base = region_base;
    let data_base = directory_base
        .checked_add(directory_bytes)
        .ok_or_else(too_large)?;

    let mut directory = Vec::with_capacity(directory_bytes as usize);
    let mut data: Vec<u8> = Vec::new();
    // The running byte offset of the next table's first knot, relative to
    // `data_base`. Promoted to an absolute address when written into the
    // directory so a helper can load a knot directly.
    let mut data_rel_offset: u32 = 0;
    for table in tables {
        let n_points = u32::try_from(table.len()).map_err(|_| too_large())?;
        let abs_data_offset = data_base
            .checked_add(data_rel_offset)
            .ok_or_else(too_large)?;
        directory.extend_from_slice(&(abs_data_offset as i32).to_le_bytes());
        directory.extend_from_slice(&(n_points as i32).to_le_bytes());

        for &(x, y) in table {
            data.extend_from_slice(&x.to_le_bytes());
            data.extend_from_slice(&y.to_le_bytes());
        }
        let table_bytes = n_points.checked_mul(GF_KNOT_BYTES).ok_or_else(too_large)?;
        data_rel_offset = data_rel_offset
            .checked_add(table_bytes)
            .ok_or_else(too_large)?;
    }

    let total_bytes = directory_bytes
        .checked_add(data_rel_offset)
        .ok_or_else(too_large)?;
    Ok(Some(GfRegions {
        directory_base,
        data_base,
        directory,
        data,
        total_bytes,
    }))
}

// Function indices of the per-program block, RELATIVE to the first program
// function. The emitted helper functions ([`lower::build_helpers`]) occupy the
// module's first function slots (`0..n_helpers`), so the absolute index of each
// program function is `n_helpers + F_*`. The three opcode programs share the
// `(i32) -> ()` type; `run` is `() -> ()`. Keeping these relative (and adding
// `n_helpers` at the call/export sites) means new helpers shift every program
// function automatically, with no index hard-coded against a fixed helper count.
const F_INITIALS: u32 = 0;
const F_FLOWS: u32 = 1;
const F_STOCKS: u32 = 2;
const F_RUN: u32 = 3;

// Type-section indices. The two program types come first; helper types are
// appended after them (at indices 2..), so these stay fixed.
const TYPE_OPCODE_FN: u32 = 0; // (i32) -> ()
const TYPE_RUN_FN: u32 = 1; // () -> ()

// Local indices shared by every opcode-program function. Param 0 is
// `module_off`; the scratch f64 and the condition i32(s) are declared locals.
const L_MODULE_OFF: u32 = 0;
const L_SCRATCH: u32 = 1;
const L_COND_BASE: u32 = 2;

/// Compile a `CompiledSimulation` (produced by the salsa incremental pipeline)
/// into a self-contained wasm module.
///
/// Current scope: the root module only, Euler integration only. The opcode
/// programs a `CompiledSimulation` carries are the plain, un-fused scalar set
/// (the VM's superinstruction fusion runs on a private execution copy), so each
/// `Opcode` lowers via [`lower::emit_bytecode`]. Anything outside the supported
/// set -- a non-Euler method, nested modules, or an unsupported opcode --
/// returns [`WasmGenError::Unsupported`] rather than emitting a wrong module.
pub fn compile_simulation(sim: &CompiledSimulation) -> Result<WasmArtifact, WasmGenError> {
    // `wasmgen` is in-crate, so it reads `CompiledSimulation`'s `pub(crate)`
    // fields directly rather than through accessors.
    let specs = &sim.specs;
    if specs.method != Method::Euler {
        return Err(WasmGenError::Unsupported(
            "wasmgen: only Euler integration is supported".to_string(),
        ));
    }

    let root = sim
        .modules
        .get(&sim.root)
        .ok_or_else(|| WasmGenError::Unsupported("wasmgen: root module not found".to_string()))?;

    if !root.context.modules.is_empty() {
        return Err(WasmGenError::Unsupported(
            "wasmgen: submodules are not supported".to_string(),
        ));
    }
    let too_large = || WasmGenError::Unsupported("wasmgen: model too large to lower".to_string());
    let n_slots = u32::try_from(root.n_slots).map_err(|_| too_large())?;
    let n_chunks = u32::try_from(specs.n_chunks).map_err(|_| too_large())?;
    let stride = n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let curr_base = 0u32;
    let next_base = stride;
    let results_base = stride.checked_mul(2).ok_or_else(too_large)?;
    let results_bytes = n_chunks.checked_mul(stride).ok_or_else(too_large)?;
    let total_bytes = results_base
        .checked_add(results_bytes)
        .ok_or_else(too_large)?;

    // The GF directory + data regions follow the results region. Their bases
    // are threaded into every `EmitCtx` so the `Lookup` opcode can address the
    // directory, and they are initialized at instantiation by an active
    // `DataSection`. `results_offset` (exported) is unchanged.
    let gf_regions = build_gf_regions(&root.context.graphical_functions, total_bytes)?;
    let (gf_directory_base, gf_data_base) = gf_regions
        .as_ref()
        .map(|r| (r.directory_base, r.data_base))
        .unwrap_or((0, 0));
    let total_bytes = match &gf_regions {
        Some(r) => total_bytes
            .checked_add(r.total_bytes)
            .ok_or_else(too_large)?,
        None => total_bytes,
    };
    let pages = total_bytes.div_ceil(WASM_PAGE_SIZE).max(1);

    // save_every mirrors vm.rs::run_to: max(1, round(save_step / dt)).
    let save_every = ((specs.save_step / specs.dt).round() as i64).max(1);
    let save_every = i32::try_from(save_every).map_err(|_| too_large())?;

    // Emitted helper functions occupy the module's first function slots; the
    // per-program functions follow at `n_helpers + F_*`. Build them up front so
    // the index registry threaded into each `EmitCtx` matches the assembled
    // module's layout, and so `emit_bytecode`'s `call`s resolve.
    let helpers = build_helpers();
    let helper_fns = helpers.fns;
    let n_helpers = helpers.functions.len() as u32;

    // Each opcode program runs over the shared f64 slab. The base offsets are
    // constant; `module_off` is the function's i32 parameter (0 for the root).
    let make_ctx = |cond_depth: usize| lower::EmitCtx {
        curr_base,
        next_base,
        gf_directory_base,
        gf_data_base,
        dt: specs.dt,
        start_time: specs.start,
        final_time: specs.stop,
        module_off_local: L_MODULE_OFF,
        scratch_local: L_SCRATCH,
        condition_locals: (0..cond_depth as u32).map(|i| L_COND_BASE + i).collect(),
        apply_locals: lower::apply_locals_for(cond_depth),
        helpers: helper_fns,
    };

    let initials_fn = emit_initials_fn(root, &make_ctx)?;
    let flows_fn = emit_opcode_fn(&root.compiled_flows, &make_ctx)?;
    let stocks_fn = emit_opcode_fn(&root.compiled_stocks, &make_ctx)?;

    let stock_offsets = collect_assign_next_opcode_offsets(&root.compiled_stocks);
    let run_fn = emit_run_simulation(
        specs,
        n_slots,
        results_base,
        stride,
        n_chunks,
        save_every,
        &stock_offsets,
        n_helpers,
    );

    let wasm = assemble_simulation(
        helpers,
        initials_fn,
        flows_fn,
        stocks_fn,
        run_fn,
        pages,
        n_slots,
        n_chunks,
        results_base,
        gf_regions.as_ref(),
    );

    let var_offsets = sim
        .offsets
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), *v))
        .collect();

    Ok(WasmArtifact {
        wasm,
        layout: WasmLayout {
            n_slots: root.n_slots,
            n_chunks: specs.n_chunks,
            results_offset: results_base as usize,
            gf_directory_offset: gf_directory_base as usize,
            gf_data_offset: gf_data_base as usize,
            var_offsets,
        },
    })
}

/// Build the `initials` function: every `CompiledInitial`'s bytecode in order,
/// over the shared slab. The shared condition-local count is the max nesting
/// depth across all the initials (they run sequentially in one function).
fn emit_initials_fn(
    root: &CompiledModule,
    make_ctx: &impl Fn(usize) -> lower::EmitCtx,
) -> Result<Function, WasmGenError> {
    let cond_depth = root
        .compiled_initials
        .iter()
        .map(|ci| max_condition_depth(&ci.bytecode))
        .max()
        .unwrap_or(0);
    let ctx = make_ctx(cond_depth);
    let mut f = new_opcode_fn(cond_depth);
    for ci in root.compiled_initials.iter() {
        lower::emit_bytecode(&ci.bytecode, &ctx, &mut f)?;
    }
    f.instruction(&I::End);
    Ok(f)
}

/// Build one opcode-program function from a single `ByteCode`.
fn emit_opcode_fn(
    bc: &ByteCode,
    make_ctx: &impl Fn(usize) -> lower::EmitCtx,
) -> Result<Function, WasmGenError> {
    let cond_depth = max_condition_depth(bc);
    let ctx = make_ctx(cond_depth);
    let mut f = new_opcode_fn(cond_depth);
    lower::emit_bytecode(bc, &ctx, &mut f)?;
    f.instruction(&I::End);
    Ok(f)
}

/// A fresh opcode-program `Function` with the scratch f64 local, `cond_depth`
/// i32 condition locals, and the three `Apply` scratch f64 locals (param 0 =
/// `module_off`). The exact declaration list lives in [`lower::opcode_fn_locals`]
/// so it stays in lockstep with [`lower::apply_locals_for`].
fn new_opcode_fn(cond_depth: usize) -> Function {
    Function::new(lower::opcode_fn_locals(cond_depth))
}

/// The stock data-buffer offsets written by the stocks program. After each
/// step these slots are copied `next -> curr`, mirroring the VM's chunk-advance
/// for the freshly integrated stock values. A stock integration writes via
/// either `AssignNext` or its peephole-fused `BinOpAssignNext` form (most
/// integrations are `stock + delta`, which peepholes to `BinOpAssignNext`), so
/// both are collected -- matching the VM's `collect_stock_offsets`
/// (`vm.rs:524`). The current scope has no nested modules, so the VM's
/// `EvalModule` recursion has no analogue here.
fn collect_assign_next_opcode_offsets(stocks: &ByteCode) -> Vec<usize> {
    let mut offsets: Vec<usize> = stocks
        .code
        .iter()
        .filter_map(|op| match op {
            Opcode::AssignNext { off } | Opcode::BinOpAssignNext { off, .. } => Some(*off as usize),
            _ => None,
        })
        .collect();
    // Defensive dedup, as the VM does: duplicate offsets would double-copy.
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// Emit the body of `run` for the `CompiledSimulation` path. Identical control
/// flow to the POC's `emit_run` (`vm.rs::run_to` Euler arm + `save_advance!`),
/// but it `call`s the three opcode-emitted functions instead of inlining `Expr`
/// lowering.
#[allow(clippy::too_many_arguments)]
fn emit_run_simulation(
    specs: &Specs,
    n_slots: u32,
    results_base: u32,
    stride: u32,
    n_chunks: u32,
    save_every: i32,
    stock_offsets: &[usize],
    n_helpers: u32,
) -> Function {
    let mut f = Function::new([(3, ValType::I32)]);

    // Absolute function indices of the per-program functions: the helpers
    // occupy slots `0..n_helpers`, so each program function is `n_helpers + F_*`.
    let f_initials = n_helpers + F_INITIALS;
    let f_flows = n_helpers + F_FLOWS;
    let f_stocks = n_helpers + F_STOCKS;

    let time_addr = TIME_OFF as u64 * u64::from(SLOT_SIZE);

    // Seed the reserved global slots into curr (chunk base 0), then run the
    // initials. The seeds mirror the VM, which writes start/dt/start/stop into
    // TIME/DT/INITIAL_TIME/FINAL_TIME before run_initials.
    store_curr_const_abs(&mut f, TIME_OFF, specs.start);
    store_curr_const_abs(&mut f, DT_OFF, specs.dt);
    store_curr_const_abs(&mut f, INITIAL_TIME_OFF, specs.start);
    store_curr_const_abs(&mut f, FINAL_TIME_OFF, specs.stop);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_initials));

    f.instruction(&I::Block(BlockType::Empty)); // $break
    f.instruction(&I::Loop(BlockType::Empty)); // $continue

    // if curr[TIME] > stop: break
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(time_addr)));
    f.instruction(&f64_const(specs.stop));
    f.instruction(&I::F64Gt);
    f.instruction(&I::BrIf(1));

    // flows then stocks, both over module_off 0.
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_flows));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_stocks));

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
        f.instruction(&I::F64Load(memarg(u64::from(slot) * u64::from(SLOT_SIZE))));
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
            u64::from(next_base_of(n_slots)) + off as u64 * u64::from(SLOT_SIZE),
        )));
        f.instruction(&I::F64Store(memarg(off as u64 * u64::from(SLOT_SIZE))));
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
    f
}

/// Byte offset of slot 0 of the `next` chunk: `n_slots * 8` (the `next` chunk
/// immediately follows `curr` in the slab).
fn next_base_of(n_slots: u32) -> u32 {
    n_slots * SLOT_SIZE
}

/// Store a compile-time constant into a `curr` slot at an absolute (module_off
/// 0) address.
fn store_curr_const_abs(f: &mut Function, off: usize, v: f64) {
    f.instruction(&I::I32Const(0));
    f.instruction(&f64_const(v));
    f.instruction(&I::F64Store(memarg(off as u64 * u64::from(SLOT_SIZE))));
}

/// Assemble the simulation module: type, function, memory, globals, exports,
/// code, and (when present) the GF data segments. The emitted helper functions
/// ([`build_helpers`]) lead the function and code sections (indices
/// `0..n_helpers`); the four program functions follow. Exports `memory`, `run`,
/// and the three self-describing i32 geometry globals. When `gf_regions` is
/// `Some`, two active `DataSection` segments initialize the GF directory and
/// data regions at instantiation.
#[allow(clippy::too_many_arguments)]
fn assemble_simulation(
    helpers: BuiltHelpers,
    initials: Function,
    flows: Function,
    stocks: Function,
    run: Function,
    pages: u32,
    n_slots: u32,
    n_chunks: u32,
    results_base: u32,
    gf_regions: Option<&GfRegions>,
) -> Vec<u8> {
    let mut wasm = WasmModule::new();
    let n_helpers = helpers.functions.len() as u32;

    // Type indices 0/1 are the program types; helper types are appended at 2..
    // (in helper order), so a helper at function index `i` uses type index
    // `2 + i`.
    let mut types = TypeSection::new();
    types.ty().function([ValType::I32], []); // TYPE_OPCODE_FN: (i32) -> ()
    types.ty().function([], []); // TYPE_RUN_FN: () -> ()
    for hf in &helpers.functions {
        types.ty().function(hf.params.clone(), hf.results.clone());
    }
    wasm.section(&types);

    // Function section: helpers first (so their indices are 0..n_helpers), then
    // the four program functions.
    let mut functions = FunctionSection::new();
    let first_helper_type = TYPE_RUN_FN + 1; // == 2
    for (i, _) in helpers.functions.iter().enumerate() {
        functions.function(first_helper_type + i as u32);
    }
    functions.function(TYPE_OPCODE_FN); // initials
    functions.function(TYPE_OPCODE_FN); // flows
    functions.function(TYPE_OPCODE_FN); // stocks
    functions.function(TYPE_RUN_FN); // run
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
    exports.export("run", ExportKind::Func, n_helpers + F_RUN);
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("n_slots", ExportKind::Global, 0);
    exports.export("n_chunks", ExportKind::Global, 1);
    exports.export("results_offset", ExportKind::Global, 2);
    wasm.section(&exports);

    // Code section order must match the function section: helper bodies, then
    // the four program functions.
    let mut code = CodeSection::new();
    for hf in &helpers.functions {
        code.function(&hf.body);
    }
    code.function(&initials);
    code.function(&flows);
    code.function(&stocks);
    code.function(&run);
    wasm.section(&code);

    // The GF directory + data regions are read-only constants; an active data
    // segment writes each at its region base when the module is instantiated.
    // The data section must follow the code section per the wasm binary order.
    if let Some(gf) = gf_regions {
        let mut data = DataSection::new();
        data.active(
            0,
            &ConstExpr::i32_const(gf.directory_base as i32),
            gf.directory.iter().copied(),
        );
        data.active(
            0,
            &ConstExpr::i32_const(gf.data_base as i32),
            gf.data.iter().copied(),
        );
        wasm.section(&data);
    }

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
    use std::io::BufReader;
    use wasm::validate;

    const POPULATION_XMILE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../default_projects/population/model.xmile"
    );

    /// Decode a GF directory's `n`th entry from `directory` bytes: the absolute
    /// data byte offset and the point count.
    fn decode_dir_entry(directory: &[u8], n: usize) -> (usize, usize) {
        let base = n * GF_DIRECTORY_ENTRY_BYTES as usize;
        let data_off = i32::from_le_bytes(directory[base..base + 4].try_into().unwrap()) as usize;
        let n_points =
            i32::from_le_bytes(directory[base + 4..base + 8].try_into().unwrap()) as usize;
        (data_off, n_points)
    }

    /// Decode the `(x, y)` knots stored at relative `data` offset `rel_off` for
    /// a table of `n_points` (interleaved f64 LE x,y pairs).
    fn decode_knots(data: &[u8], rel_off: usize, n_points: usize) -> Vec<(f64, f64)> {
        (0..n_points)
            .map(|k| {
                let a = rel_off + k * GF_KNOT_BYTES as usize;
                let x = f64::from_le_bytes(data[a..a + 8].try_into().unwrap());
                let y = f64::from_le_bytes(data[a + 8..a + 16].try_into().unwrap());
                (x, y)
            })
            .collect()
    }

    /// Task 1 (pure layout): `build_gf_regions` concatenates several tables into
    /// the data region in order, and the directory maps each global table index
    /// to its *absolute* data byte offset + point count. The data offset for
    /// table `t` must be `data_base` plus the byte span of all earlier tables.
    #[test]
    fn build_gf_regions_lays_out_directory_and_data() {
        let region_base = 4096u32;
        let tables = vec![
            vec![(0.0, 10.0), (1.0, 20.0), (2.5, 5.0)],
            vec![(-1.0, 0.5)],
            vec![(0.0, 0.0), (10.0, 100.0)],
        ];
        let regions = build_gf_regions(&tables, region_base)
            .expect("layout must succeed")
            .expect("non-empty tables yield Some");

        // Directory immediately at region_base; data follows the directory.
        assert_eq!(regions.directory_base, region_base);
        let directory_bytes = tables.len() as u32 * GF_DIRECTORY_ENTRY_BYTES;
        assert_eq!(regions.data_base, region_base + directory_bytes);
        assert_eq!(regions.directory.len(), directory_bytes as usize);

        // Walk the directory; each table's data offset is absolute and its
        // knots round-trip exactly. The running expected offset is data_base
        // plus the byte span of all previously-laid tables.
        let mut expected_abs = regions.data_base as usize;
        let mut total_knot_bytes = 0usize;
        for (t, table) in tables.iter().enumerate() {
            let (data_off, n_points) = decode_dir_entry(&regions.directory, t);
            assert_eq!(n_points, table.len(), "table {t} point count");
            assert_eq!(data_off, expected_abs, "table {t} absolute data offset");

            let rel = data_off - regions.data_base as usize;
            assert_eq!(
                decode_knots(&regions.data, rel, n_points).as_slice(),
                table.as_slice(),
                "table {t} knots round-trip"
            );

            let span = table.len() * GF_KNOT_BYTES as usize;
            expected_abs += span;
            total_knot_bytes += span;
        }
        assert_eq!(
            regions.total_bytes as usize,
            directory_bytes as usize + total_knot_bytes,
            "total span covers directory + all knots"
        );
    }

    /// Task 1 (pure layout): an empty table list yields no regions and no
    /// growth, so a model without graphical functions is unaffected.
    #[test]
    fn build_gf_regions_empty_is_none() {
        assert!(
            build_gf_regions(&[], 4096)
                .expect("layout must succeed")
                .is_none(),
            "no tables -> no GF regions"
        );
    }

    /// Task 1 (data-section round-trip): the GF regions reach the instantiated
    /// module's linear memory via the active `DataSection`, at the bases the
    /// directory advertises. Reads the directory entry for table 0 from memory,
    /// follows its absolute data offset, and asserts the `(x, y)` knots are
    /// present with the right count -- the contract the `Lookup` opcode (Task 3)
    /// relies on. (Exercised end-to-end through a GF *model* once the opcode
    /// lowers, in `compile_simulation_gf_lookup_modes_match_vm`.)
    #[test]
    fn assembled_module_initializes_gf_regions_in_memory() {
        let knots = [(0.0, 10.0), (1.0, 20.0), (2.5, 5.0), (4.0, 40.0)];
        let region_base = WASM_PAGE_SIZE; // one page in, comfortably past slot 0
        let regions = build_gf_regions(std::slice::from_ref(&knots.to_vec()), region_base)
            .expect("layout")
            .expect("non-empty");

        // A minimal module: one empty exported `run` (so the assembler shape is
        // exercised) is unnecessary here -- assert directly that the active data
        // segments initialize memory. Assemble via the production assembler with
        // a trivial set of empty program functions.
        let helpers = build_helpers();
        let empty = || {
            let mut f = Function::new([]);
            f.instruction(&I::End);
            f
        };
        let pages = (region_base + regions.total_bytes)
            .div_ceil(WASM_PAGE_SIZE)
            .max(1);
        let wasm = assemble_simulation(
            helpers,
            empty(),
            empty(),
            empty(),
            empty(),
            pages,
            0,
            0,
            0,
            Some(&regions),
        );

        let info = validate(&wasm).expect("module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let mem = store
            .instance_export(inst, "memory")
            .unwrap()
            .as_mem()
            .unwrap();

        let dir_off = regions.directory_base as usize;
        let (data_off, n_points, flat) = store.mem_access_mut_slice(mem, |bytes| {
            let data_off =
                i32::from_le_bytes(bytes[dir_off..dir_off + 4].try_into().unwrap()) as usize;
            let n_points =
                i32::from_le_bytes(bytes[dir_off + 4..dir_off + 8].try_into().unwrap()) as usize;
            let flat: Vec<f64> = (0..n_points * 2)
                .map(|i| {
                    let a = data_off + i * 8;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
                .collect();
            (data_off, n_points, flat)
        });

        assert_eq!(n_points, knots.len(), "directory point count");
        assert_eq!(
            data_off, regions.data_base as usize,
            "table 0's data offset is the start of the data region"
        );
        for (k, &(x, y)) in knots.iter().enumerate() {
            assert_eq!(flat[2 * k], x, "knot {k} x");
            assert_eq!(flat[2 * k + 1], y, "knot {k} y");
        }
    }

    /// The FFI entry point goes through the salsa pipeline + `compile_simulation`
    /// and returns a non-empty blob that validates under the interpreter.
    #[test]
    fn compile_datamodel_to_wasm_validates() {
        let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
        let mut reader = BufReader::new(file);
        let datamodel = open_xmile(&mut reader).expect("parse population xmile");

        let wasm = compile_datamodel_to_wasm(&datamodel, "main").expect("wasm codegen");
        assert!(!wasm.is_empty(), "blob should be non-empty");
        validate(&wasm).expect("blob must validate under the interpreter");
    }

    // ── compile_simulation (CompiledSimulation -> wasm) ───────────────────

    /// Build a `CompiledSimulation` for the named model of `datamodel` via the
    /// production incremental pipeline (the same path the VM corpus uses).
    fn compile_sim(datamodel: &crate::datamodel::Project, model_name: &str) -> CompiledSimulation {
        let mut db = SimlinDb::default();
        let sync = sync_from_datamodel_incremental(&mut db, datamodel, None);
        compile_project_incremental(&db, sync.project, model_name).expect("incremental compile")
    }

    /// Run a `WasmArtifact` under the DLR-FT interpreter and return the
    /// step-major results slab (`n_chunks * n_slots` f64, row-major by step).
    fn run_artifact_results(artifact: &WasmArtifact) -> Vec<f64> {
        let info = validate(&artifact.wasm).expect("generated module must validate");
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
        let n = artifact.layout.n_chunks * artifact.layout.n_slots;
        let base = artifact.layout.results_offset;
        store.mem_access_mut_slice(mem, |bytes| {
            (0..n)
                .map(|i| {
                    let a = base + i * 8;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
                .collect()
        })
    }

    /// Assert every variable in `artifact.layout` matches the VM's series for
    /// the same `CompiledSimulation`. Returns the number of variables checked.
    fn assert_matches_vm(sim: CompiledSimulation, artifact: &WasmArtifact) -> usize {
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;
        let wasm_data = run_artifact_results(artifact);

        let mut vm = Vm::new(sim).expect("vm creation");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();

        assert_eq!(
            vm_results.step_count, n_chunks,
            "saved-chunk count differs from VM"
        );

        let mut checked = 0usize;
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let wasm_off = *wasm_off;
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(&vm_off) = vm_results.offsets.get(&ident) else {
                continue;
            };
            for c in 0..n_chunks {
                let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
                let wasm_val = wasm_data[c * n_slots + wasm_off];
                let diff = (vm_val - wasm_val).abs();
                assert!(
                    diff < 1e-9,
                    "{name} mismatch at chunk {c}: vm={vm_val} wasm={wasm_val} (diff {diff})",
                );
            }
            checked += 1;
        }
        checked
    }

    #[test]
    fn compile_simulation_population_matches_vm() {
        let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
        let mut reader = BufReader::new(file);
        let datamodel = open_xmile(&mut reader).expect("parse population xmile");

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        // Geometry is self-consistent with the specs.
        let specs = Specs::from(&datamodel.sim_specs);
        assert_eq!(artifact.layout.n_chunks, specs.n_chunks);

        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 5,
            "expected to compare the population model's variables, only checked {checked}"
        );
        assert!(
            artifact
                .layout
                .var_offsets
                .iter()
                .any(|(n, _)| n == "population"),
            "the population stock should be in the layout"
        );
    }

    #[test]
    fn compile_simulation_simple_stock_flow_matches_vm() {
        // A minimal scalar Euler model: a stock filled by a constant inflow.
        let datamodel = crate::test_common::TestProject::new("simple")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare level + inflow");
        // level should integrate to 2*10 = 20 by the last step.
        let last = run_artifact_results(&artifact);
        let n_slots = artifact.layout.n_slots;
        let level_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "level")
            .map(|(_, off)| *off)
            .expect("level offset");
        let last_step = (artifact.layout.n_chunks - 1) * n_slots + level_off;
        assert!(
            (last[last_step] - 20.0).abs() < 1e-9,
            "level should reach 20"
        );
    }

    #[test]
    fn compile_simulation_save_step_cadence_matches_vm() {
        // Exercises the conditional-save / non-save-step copy-back branch of
        // `save_advance!` (`vm.rs:682`): with save_step = 2*dt, most steps copy
        // `next -> curr` WITHOUT recording a snapshot, and only every other step
        // (plus the forced t=start sample) writes a results row. Every other
        // wasmgen test uses save_step = None (save_every = 1), so this is the
        // only coverage of the multi-step cadence.
        let mut datamodel = crate::test_common::TestProject::new("cadence")
            .with_sim_time(0.0, 10.0, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();
        // `with_sim_time` clears save_step to dt; the builder has no
        // `with_save_step`, so set it directly: save_step = 2, dt = 1.
        datamodel.sim_specs.save_step = Some(crate::datamodel::Dt::Dt(2.0));

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        // dt=1, save_step=2 over [0,10] saves at t=0,2,4,6,8,10 -> 6 chunks.
        assert_eq!(
            artifact.layout.n_chunks, 6,
            "save_step = 2*dt over [0,10] should yield 6 saved samples"
        );

        // Per-variable series + saved-chunk count both match the VM (which
        // `assert_matches_vm` asserts via `step_count == n_chunks`).
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare level + inflow");
    }

    #[test]
    fn compile_simulation_conditional_model_matches_vm() {
        // Exercises the SetCond/If lowering through the whole-model path.
        let datamodel = crate::test_common::TestProject::new("cond")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("threshold", "3", None)
            .aux("gated", "IF TIME > threshold THEN 10 ELSE 1", None)
            .stock("acc", "0", &["gated_flow"], &[], None)
            .flow("gated_flow", "gated", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare gated + acc");
    }

    #[test]
    fn compile_simulation_rejects_non_euler() {
        let datamodel = crate::test_common::TestProject::new("rk4")
            .with_sim_time(0.0, 5.0, 1.0)
            .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let result = compile_simulation(&sim);
        assert!(matches!(result, Err(WasmGenError::Unsupported(_))));
    }

    #[test]
    fn compile_simulation_rejects_nested_modules() {
        // A root model that instantiates a submodule is outside the currently
        // supported set (`root.context.modules` is non-empty). It must return a
        // clean `Unsupported` error, never a panic or a wrong module. Built as a
        // two-model datamodel directly, since `TestProject` only emits a single
        // `main` model.
        use crate::datamodel;
        let project = datamodel::Project {
            name: "nested".to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 5.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: datamodel::SimMethod::Euler,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: vec![
                        datamodel::Variable::Aux(datamodel::Aux {
                            ident: "input".to_string(),
                            equation: datamodel::Equation::Scalar("3".to_string()),
                            documentation: String::new(),
                            units: None,
                            gf: None,
                            ai_state: None,
                            uid: None,
                            compat: datamodel::Compat::default(),
                        }),
                        datamodel::Variable::Module(datamodel::Module {
                            ident: "sub".to_string(),
                            model_name: "submodel".to_string(),
                            documentation: String::new(),
                            units: None,
                            references: vec![datamodel::ModuleReference {
                                src: "input".to_string(),
                                dst: "in".to_string(),
                            }],
                            compat: datamodel::Compat::default(),
                            ai_state: None,
                            uid: None,
                        }),
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                datamodel::Model {
                    name: "submodel".to_string(),
                    sim_specs: None,
                    variables: vec![
                        datamodel::Variable::Aux(datamodel::Aux {
                            ident: "in".to_string(),
                            equation: datamodel::Equation::Scalar("0".to_string()),
                            documentation: String::new(),
                            units: None,
                            gf: None,
                            ai_state: None,
                            uid: None,
                            compat: datamodel::Compat {
                                can_be_module_input: true,
                                ..datamodel::Compat::default()
                            },
                        }),
                        datamodel::Variable::Aux(datamodel::Aux {
                            ident: "out".to_string(),
                            equation: datamodel::Equation::Scalar("in * 2".to_string()),
                            documentation: String::new(),
                            units: None,
                            gf: None,
                            ai_state: None,
                            uid: None,
                            compat: datamodel::Compat::default(),
                        }),
                    ],
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
            ],
            source: Default::default(),
            ai_information: None,
        };

        let sim = compile_sim(&project, "main");
        let result = compile_simulation(&sim);
        // Assert on the specific submodule message so this stays a focused
        // guard on the early `root.context.modules.is_empty()` check
        // (`compile_simulation`), distinct from the `EvalModule`-opcode fallback
        // in `lower.rs` that would otherwise also reject the model.
        match result {
            Err(WasmGenError::Unsupported(msg)) => assert!(
                msg.contains("submodules are not supported"),
                "expected the submodule-rejection message, got: {msg}"
            ),
            Ok(_) => panic!("a model with a submodule must be rejected as Unsupported"),
        }
    }

    /// AC4.1: a host reads the three exported geometry globals from the
    /// instantiated module and uses them (no external metadata) to stride one
    /// variable's series, which must match the VM.
    #[test]
    fn compile_simulation_exports_self_describing_geometry() {
        let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
        let mut reader = BufReader::new(file);
        let datamodel = open_xmile(&mut reader).expect("parse population xmile");

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let info = validate(&artifact.wasm).expect("module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;

        // Read the three i32 geometry globals straight from the module.
        let read_global = |store: &mut Store<()>, name: &str| -> usize {
            let g = store
                .instance_export(inst, name)
                .unwrap()
                .as_global()
                .unwrap();
            match store.global_read(g) {
                checked::StoredValue::I32(x) => x as usize,
                other => panic!("expected i32 global, got {other:?}"),
            }
        };
        let n_slots = read_global(&mut store, "n_slots");
        let n_chunks = read_global(&mut store, "n_chunks");
        let results_offset = read_global(&mut store, "results_offset");

        // They equal the layout values.
        assert_eq!(n_slots, artifact.layout.n_slots);
        assert_eq!(n_chunks, artifact.layout.n_chunks);
        assert_eq!(results_offset, artifact.layout.results_offset);

        // Stride to the population series using only module-reported geometry.
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
        let pop_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "population")
            .map(|(_, off)| *off)
            .expect("population offset");
        let pop_series: Vec<f64> = store.mem_access_mut_slice(mem, |bytes| {
            (0..n_chunks)
                .map(|c| {
                    let a = results_offset + (c * n_slots + pop_off) * 8;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
                .collect()
        });

        let mut vm = Vm::new(sim).expect("vm");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();
        let pop = Ident::<Canonical>::from_str_unchecked("population");
        let vm_pop_off = *vm_results.offsets.get(&pop).expect("vm population offset");
        for (c, &wasm_val) in pop_series.iter().enumerate() {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_pop_off];
            assert!(
                (vm_val - wasm_val).abs() < 1e-9,
                "population mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
            );
        }
    }
}
