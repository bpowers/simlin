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
use crate::vm::{CompiledSimulation, StepPart};

use super::WasmGenError;
use super::lower::{self, BuiltHelpers, build_helpers, f64_const, max_condition_depth, memarg};

// Reserved global slots, mirroring `crate::vm`.
const TIME_OFF: usize = 0;
const DT_OFF: usize = 1;
const INITIAL_TIME_OFF: usize = 2;
const FINAL_TIME_OFF: usize = 3;

const SLOT_SIZE: u32 = 8;
const WASM_PAGE_SIZE: u32 = 65536;

// Slot-0 byte base of the `curr` chunk, and the byte address of `curr[TIME]`
// (an absolute, module-independent global slot). Both run-loop and snapshot
// code address `curr` from byte 0.
const CURR_BASE: u32 = 0;
const TIME_ADDR: u64 = TIME_OFF as u64 * SLOT_SIZE as u64;

// Global indices. The three self-describing geometry globals come first (so the
// exported indices 0/1/2 stay stable for hosts); `use_prev_fallback` -- the only
// mutable global -- follows at index 3. It gates `LoadPrev`: init 1 (return the
// fallback) until the first `prev_values` snapshot clears it (`vm.rs:668`).
const G_N_SLOTS: u32 = 0;
const G_N_CHUNKS: u32 = 1;
const G_RESULTS_OFFSET: u32 = 2;
const G_USE_PREV_FALLBACK: u32 = 3;

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
    // The run-loop shape is selected from `specs.method` below; all three
    // methods (`Euler`/`RungeKutta2`/`RungeKutta4`) are supported.

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
    // The stock data-buffer offsets the stocks program writes (`AssignNext` /
    // its `BinOpAssignNext` peephole form). The Euler advance copies these
    // `next -> curr`; the RK loops index `rk_scratch[saved/accum]` by their
    // position here. Collected up front so the RK scratch region is sized below.
    let stock_offsets = collect_assign_next_opcode_offsets(&root.compiled_stocks);
    let n_stocks = u32::try_from(stock_offsets.len()).map_err(|_| too_large())?;
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

    // The two snapshot regions follow the GF regions, each `n_slots` wide
    // (`vm.rs:617-618`). `initial_values` backs `INIT(x)` (captured once after
    // initials); `prev_values` backs `PREVIOUS(x)` (captured after each step, or
    // after the end-of-step flows re-eval under RK). Their bases are threaded
    // into every `EmitCtx` so `LoadInitial`/`LoadPrev` can address them.
    let snapshot_bytes = n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let initial_values_base = total_bytes;
    let prev_values_base = initial_values_base
        .checked_add(snapshot_bytes)
        .ok_or_else(too_large)?;
    let total_bytes = prev_values_base
        .checked_add(snapshot_bytes)
        .ok_or_else(too_large)?;

    // The RK scratch region (`saved`(n_stocks) ++ `accum`(n_stocks)) follows the
    // snapshot regions. It holds each stock's stage-1 value and running RK
    // accumulator across the stages (`vm.rs:655`, the VM's `rk_scratch`
    // split). Euler needs neither, so the region is only reserved for RK.
    let rk = matches!(specs.method, Method::RungeKutta2 | Method::RungeKutta4);
    let stock_scratch_bytes = n_stocks.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let rk_saved_base = total_bytes;
    let rk_accum_base = rk_saved_base
        .checked_add(stock_scratch_bytes)
        .ok_or_else(too_large)?;
    let total_bytes = if rk {
        rk_accum_base
            .checked_add(stock_scratch_bytes)
            .ok_or_else(too_large)?
    } else {
        total_bytes
    };

    // The `temp_storage` region follows everything else. It mirrors the VM's
    // flat `temp_storage` buffer of `temp_total_size` f64 (`vm.rs:584-586`):
    // element `index` of temp `temp_id` lives at
    // `temp_storage[temp_offsets[temp_id] + index]`. Array-producing builtins
    // (`AssignTemp` -> `BeginIter` loops) and the sliced reducers read/write it
    // through the view machinery. `temp_total_size` is a compile-time
    // `ByteCodeContext` field, so the region's size is known here.
    let temp_total_size = u32::try_from(root.context.temp_total_size).map_err(|_| too_large())?;
    let temp_storage_base = total_bytes;
    let temp_storage_bytes = temp_total_size
        .checked_mul(SLOT_SIZE)
        .ok_or_else(too_large)?;
    let total_bytes = temp_storage_base
        .checked_add(temp_storage_bytes)
        .ok_or_else(too_large)?;

    // The vector-op scratch region follows `temp_storage`. The Phase-6 vector
    // ops (`VectorSelect`'s collected selected values, `VectorSortOrder`/`Rank`'s
    // `(value, idx)` sort pairs) stage data here. A sort pair region for a view
    // of `size` elements needs `2 * size` f64; the largest view a vector op
    // processes is bounded by the largest temp it writes (`temp_total_size`) and
    // by the model's slot count (a var-view input), so `2 * max(temp_total_size,
    // n_slots)` f64 is a safe upper bound. Reserved unconditionally (a model
    // without vector ops simply never reads it); the bound is tiny for scalar
    // models. `vector_scratch_base` is threaded into every `EmitCtx`.
    //
    // Sizing invariant: every vector-op *input view*'s logical `size()` is <=
    // its storage footprint -- a full or sliced var view fits in `n_slots`, a
    // temp view fits in `temp_total_size` -- so `max(temp_total_size, n_slots)`
    // bounds the element count any vector op stages, gathers, or sorts. A
    // *broadcast* view (logical `size()` > footprint, e.g. a 1-D source iterated
    // over a 2-D output) would violate this, but the vector ops never take one as
    // a direct argument: broadcasting happens earlier, in the `BeginBroadcastIter`
    // temp materialization, and a vector op reads the materialized temp.
    let vector_scratch_base = total_bytes;
    let vector_scratch_slots = temp_total_size
        .max(n_slots)
        .checked_mul(2)
        .ok_or_else(too_large)?;
    let vector_scratch_bytes = vector_scratch_slots
        .checked_mul(SLOT_SIZE)
        .ok_or_else(too_large)?;
    let total_bytes = vector_scratch_base
        .checked_add(vector_scratch_bytes)
        .ok_or_else(too_large)?;

    // The allocation scratch region follows the vector scratch. The Phase-6
    // `AllocateAvailable`/`AllocateByPriority` arms stage, per opcode, the
    // gathered request values (n f64), the per-requester profile tuples (4n
    // f64), and the output allocations (n f64) -- `6n` f64 all live across the
    // `allocate_available` helper call. A requester count `n` is bounded by the
    // largest view a vector op could process (a temp or a var-view input), so
    // `6 * max(temp_total_size, n_slots)` f64 is a safe upper bound. Reserved
    // unconditionally (a model without allocators never reads it); the bound is
    // tiny for scalar models. `alloc_scratch_base` is threaded into every
    // `EmitCtx`.
    let alloc_scratch_base = total_bytes;
    let alloc_scratch_slots = temp_total_size
        .max(n_slots)
        .checked_mul(6)
        .ok_or_else(too_large)?;
    let alloc_scratch_bytes = alloc_scratch_slots
        .checked_mul(SLOT_SIZE)
        .ok_or_else(too_large)?;
    let total_bytes = alloc_scratch_base
        .checked_add(alloc_scratch_bytes)
        .ok_or_else(too_large)?;

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
    // `step_part` is per-program so `LoadInitial` picks its `curr`-vs-snapshot
    // branch at compile time (`vm.rs:1332-1340`).
    let make_ctx = |cond_depth: usize, step_part: StepPart| lower::EmitCtx {
        curr_base,
        next_base,
        gf_directory_base,
        gf_data_base,
        initial_values_base,
        prev_values_base,
        use_prev_fallback_global: G_USE_PREV_FALLBACK,
        step_part,
        dt: specs.dt,
        start_time: specs.start,
        final_time: specs.stop,
        module_off_local: L_MODULE_OFF,
        scratch_local: L_SCRATCH,
        condition_locals: (0..cond_depth as u32).map(|i| L_COND_BASE + i).collect(),
        apply_locals: lower::apply_locals_for(cond_depth),
        helpers: helper_fns,
        temp_storage_base,
        extra_i32_local_base: lower::extra_i32_local_base(cond_depth),
        vector_f64_locals: lower::vector_f64_locals_for(cond_depth),
        vector_i32_locals: lower::vector_i32_locals_for(cond_depth),
        vector_scratch_base,
        alloc_scratch_base,
        ctx: &root.context,
    };

    let initials_fn = emit_initials_fn(root, &make_ctx)?;
    let flows_fn = emit_opcode_fn(&root.compiled_flows, StepPart::Flows, &make_ctx)?;
    let stocks_fn = emit_opcode_fn(&root.compiled_stocks, StepPart::Stocks, &make_ctx)?;

    let run_fn = emit_run_simulation(
        specs,
        RunRegions {
            n_slots,
            results_base,
            stride,
            n_chunks,
            initial_values_base,
            prev_values_base,
            rk_saved_base,
            rk_accum_base,
        },
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
fn emit_initials_fn<'a>(
    root: &CompiledModule,
    make_ctx: &impl Fn(usize, StepPart) -> lower::EmitCtx<'a>,
) -> Result<Function, WasmGenError> {
    let cond_depth = root
        .compiled_initials
        .iter()
        .map(|ci| max_condition_depth(&ci.bytecode))
        .max()
        .unwrap_or(0);
    // The initials run sequentially in one function; each fragment's dynamic-
    // subscript accumulation completes (and `emit_bytecode` resets its local
    // cursor) before the next, so reserving the *max* per-fragment count -- not
    // the sum -- is correct, and the fragments reuse the same i32 locals.
    let extra_i32 = root
        .compiled_initials
        .iter()
        .map(|ci| lower::count_extra_i32_locals(&ci.bytecode))
        .max()
        .unwrap_or(0);
    let ctx = make_ctx(cond_depth, StepPart::Initials);
    let mut f = new_opcode_fn(cond_depth, extra_i32);
    for ci in root.compiled_initials.iter() {
        lower::emit_bytecode(&ci.bytecode, &ctx, &mut f)?;
    }
    f.instruction(&I::End);
    Ok(f)
}

/// Build one opcode-program function from a single `ByteCode`, lowering it as
/// `step_part` (which `LoadInitial` reads to pick its `curr`-vs-snapshot
/// branch).
fn emit_opcode_fn<'a>(
    bc: &ByteCode,
    step_part: StepPart,
    make_ctx: &impl Fn(usize, StepPart) -> lower::EmitCtx<'a>,
) -> Result<Function, WasmGenError> {
    let cond_depth = max_condition_depth(bc);
    let extra_i32 = lower::count_extra_i32_locals(bc);
    let ctx = make_ctx(cond_depth, step_part);
    let mut f = new_opcode_fn(cond_depth, extra_i32);
    lower::emit_bytecode(bc, &ctx, &mut f)?;
    f.instruction(&I::End);
    Ok(f)
}

/// A fresh opcode-program `Function` with the scratch f64 local, `cond_depth`
/// i32 condition locals, the three `Apply` scratch f64 locals, and `extra_i32`
/// dynamic-subscript scratch i32 locals (param 0 = `module_off`). The exact
/// declaration list lives in [`lower::opcode_fn_locals`] so it stays in lockstep
/// with [`lower::apply_locals_for`] / [`lower::extra_i32_local_base`].
fn new_opcode_fn(cond_depth: usize, extra_i32: u32) -> Function {
    Function::new(lower::opcode_fn_locals(cond_depth, extra_i32))
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

/// The linear-memory region geometry `run` needs: the chunk/results bases, the
/// snapshot bases (`initial_values`/`prev_values`), and the RK scratch bases
/// (`saved`/`accum`). Bundled to keep `emit_run_simulation`'s signature small as
/// the run loop gained snapshot + RK regions.
#[derive(Clone, Copy)]
struct RunRegions {
    n_slots: u32,
    results_base: u32,
    stride: u32,
    n_chunks: u32,
    initial_values_base: u32,
    prev_values_base: u32,
    /// Slot-0 byte base of the RK `saved[i]` scratch (one f64 per stock).
    rk_saved_base: u32,
    /// Slot-0 byte base of the RK `accum[i]` scratch (one f64 per stock).
    rk_accum_base: u32,
}

// `run`'s f64 locals (after the three i32 locals). The RK loops need a
// `saved_time` (the timestep's t, restored after the stages move `curr[TIME]` to
// trial points) and a per-stage `s` scratch (`next[off]-curr[off]`). Euler
// declares them too -- two unused f64 locals are free.
const L_SAVED_TIME: u32 = 3;
const L_RK_S: u32 = 4;

/// Emit the body of `run` for the `CompiledSimulation` path: seed the reserved
/// globals, run the initials, capture `initial_values`, then drive the
/// integration loop selected by `specs.method`. The loop `call`s the three
/// opcode-emitted functions; the Euler arm mirrors `vm.rs::run_to`'s Euler arm,
/// and the RK arms mirror `vm.rs:712-838`.
fn emit_run_simulation(
    specs: &Specs,
    regions: RunRegions,
    save_every: i32,
    stock_offsets: &[usize],
    n_helpers: u32,
) -> Function {
    // Three i32 locals (saved/step_accum/dst) + two f64 locals (saved_time, s).
    let mut f = Function::new([(3, ValType::I32), (2, ValType::F64)]);

    // Absolute function indices of the per-program functions: the helpers
    // occupy slots `0..n_helpers`, so each program function is `n_helpers + F_*`.
    let f_initials = n_helpers + F_INITIALS;
    let f_flows = n_helpers + F_FLOWS;
    let f_stocks = n_helpers + F_STOCKS;

    // Seed the reserved global slots into curr (chunk base 0), then run the
    // initials. The seeds mirror the VM, which writes start/dt/start/stop into
    // TIME/DT/INITIAL_TIME/FINAL_TIME before run_initials.
    store_curr_const_abs(&mut f, TIME_OFF, specs.start);
    store_curr_const_abs(&mut f, DT_OFF, specs.dt);
    store_curr_const_abs(&mut f, INITIAL_TIME_OFF, specs.start);
    store_curr_const_abs(&mut f, FINAL_TIME_OFF, specs.stop);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_initials));

    // Capture `initial_values := curr` exactly once, after initials, for
    // `INIT(x)` reads in the flows/stocks programs (`vm.rs:1124-1128`).
    // `use_prev_fallback` stays 1 (its init value) through initials, so any
    // `PREVIOUS(x)` evaluated during initials returns its fallback.
    emit_copy_chunk(
        &mut f,
        CURR_BASE,
        regions.initial_values_base,
        regions.n_slots,
    );

    f.instruction(&I::Block(BlockType::Empty)); // $break
    f.instruction(&I::Loop(BlockType::Empty)); // $continue

    // if curr[TIME] > stop: break
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(TIME_ADDR)));
    f.instruction(&f64_const(specs.stop));
    f.instruction(&I::F64Gt);
    f.instruction(&I::BrIf(1));

    // The per-method step: compute the new stock values into `next[off]`, leave
    // `curr` holding the full time-`t` state (aux/flows + time-`t` stocks), then
    // snapshot `prev_values := curr` and clear `use_prev_fallback`.
    match specs.method {
        Method::Euler => emit_euler_step(&mut f, f_flows, f_stocks, &regions),
        Method::RungeKutta4 => {
            emit_rk4_step(&mut f, f_flows, f_stocks, specs.dt, stock_offsets, &regions)
        }
        Method::RungeKutta2 => {
            emit_rk2_step(&mut f, f_flows, f_stocks, specs.dt, stock_offsets, &regions)
        }
    }

    // The save + advance tail is method-agnostic: every method leaves `next[off]`
    // holding the new stock values and `curr` holding the time-`t` state, so the
    // save row records `curr`, the advance copies the new stocks `next -> curr`,
    // and `curr[TIME] += dt`.
    emit_save_advance(&mut f, specs, save_every, stock_offsets, &regions);

    f.instruction(&I::Br(0)); // continue
    f.instruction(&I::End); // end loop
    f.instruction(&I::End); // end block
    f.instruction(&I::End); // end function
    f
}

/// The Euler step: `flows`+`stocks` (the stocks program writes `next[off]`),
/// then the `prev_values` snapshot. Mirrors `vm.rs:698-708`.
fn emit_euler_step(f: &mut Function, f_flows: u32, f_stocks: u32, regions: &RunRegions) {
    emit_eval_step(f, f_flows, f_stocks);
    emit_prev_snapshot(f, regions);
}

/// `eval_step` = `flows(0)` then `stocks(0)` (`vm.rs:1195`). The stocks program
/// writes each stock's integrated value into `next[off]`.
fn emit_eval_step(f: &mut Function, f_flows: u32, f_stocks: u32) {
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_flows));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_stocks));
}

/// Snapshot `prev_values := curr` and clear `use_prev_fallback` so the next
/// step's `PREVIOUS(x)` reads this step's `curr` rather than its fallback
/// (`vm.rs:705-707` for Euler; `vm.rs:781-783` / `832-834` for RK, where it runs
/// only after the end-of-step flows re-eval has restored `curr`).
fn emit_prev_snapshot(f: &mut Function, regions: &RunRegions) {
    emit_copy_chunk(f, CURR_BASE, regions.prev_values_base, regions.n_slots);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_USE_PREV_FALLBACK));
}

/// The method-agnostic save + advance tail (the wasm analogue of the VM's
/// `save_advance!` plus its per-step advance). Records a results row from `curr`
/// on the VM's cadence, breaks when the chunk budget is exhausted, then advances
/// by copying the new stock values `next -> curr` and stepping `curr[TIME] += dt`.
fn emit_save_advance(
    f: &mut Function,
    specs: &Specs,
    save_every: i32,
    stock_offsets: &[usize],
    regions: &RunRegions,
) {
    let n_slots = regions.n_slots;

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
    f.instruction(&I::F64Load(memarg(TIME_ADDR)));
    f.instruction(&f64_const(specs.start));
    f.instruction(&I::F64Eq);
    f.instruction(&I::I32And);
    f.instruction(&I::I32Or);
    f.instruction(&I::If(BlockType::Empty));

    // dst = results_base + saved * stride
    f.instruction(&I::I32Const(regions.results_base as i32));
    f.instruction(&I::LocalGet(L_SAVED));
    f.instruction(&I::I32Const(regions.stride as i32));
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
    f.instruction(&I::I32Const(regions.n_chunks as i32));
    f.instruction(&I::I32GeS);
    f.instruction(&I::BrIf(2));

    f.instruction(&I::End); // end if

    // Advance: copy the freshly integrated stock values next -> curr. The
    // `next` chunk's slot-0 byte base is one chunk past `curr`, i.e. the chunk
    // stride (`compile_simulation` sets `next_base = stride`).
    let next_base = regions.stride;
    for &off in stock_offsets {
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(memarg(
            u64::from(next_base) + off as u64 * u64::from(SLOT_SIZE),
        )));
        f.instruction(&I::F64Store(memarg(off as u64 * u64::from(SLOT_SIZE))));
    }

    // time += dt
    f.instruction(&I::I32Const(0));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(TIME_ADDR)));
    f.instruction(&f64_const(specs.dt));
    f.instruction(&I::F64Add);
    f.instruction(&I::F64Store(memarg(TIME_ADDR)));
}

/// Store a compile-time constant into a `curr` slot at an absolute (module_off
/// 0) address.
fn store_curr_const_abs(f: &mut Function, off: usize, v: f64) {
    f.instruction(&I::I32Const(0));
    f.instruction(&f64_const(v));
    f.instruction(&I::F64Store(memarg(off as u64 * u64::from(SLOT_SIZE))));
}

// ── RK loop primitives ────────────────────────────────────────────────────
//
// Every RK memory slot lives at a constant byte address (`base + idx*8`), so the
// dynamic part of the address is always `i32.const 0` and the constant
// `memarg.offset` carries `base + idx*8`. `f64.store` wants `[addr_i32,
// value_f64]`, so the store helpers push the `i32.const 0` address first, then
// the caller leaves the value on the stack.

/// `i32.const 0; f64.load[base + idx*8]` -- push the f64 at slot `idx` of the
/// region whose slot-0 byte base is `base`.
fn emit_load_slot(f: &mut Function, base: u32, idx: u32) {
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(
        u64::from(base) + u64::from(idx) * u64::from(SLOT_SIZE),
    )));
}

/// Push the store *address* half of an RK slot store: a bare `i32.const 0`.
/// Every RK slot's full byte address (`base + idx*8`) rides in the matching
/// [`emit_store_slot_value`]'s `memarg.offset`, so the dynamic address is always
/// the constant 0 -- this half therefore needs no `base`/`idx`. Kept as the
/// named symmetry partner of `emit_store_slot_value` (which it precedes at every
/// call site, since `f64.store` consumes `[addr_i32, value_f64]`): inlining only
/// this half would scatter unexplained `i32.const 0`s whose absolute-addressing
/// intent is exactly what the pairing documents.
fn emit_store_slot_addr(f: &mut Function) {
    f.instruction(&I::I32Const(0));
}

/// `f64.store[base + idx*8]` -- consume `[addr_i32, value_f64]` already on the
/// stack (the address from [`emit_store_slot_addr`]).
fn emit_store_slot_value(f: &mut Function, base: u32, idx: u32) {
    f.instruction(&I::F64Store(memarg(
        u64::from(base) + u64::from(idx) * u64::from(SLOT_SIZE),
    )));
}

/// Emit `L_RK_S := next[off] - curr[off]` -- the stock's stage delta `s_k`
/// (`vm.rs`: `let sN = next[off] - curr[off]`). Computed before any of the
/// stage's writes clobber `curr[off]`. `next_base` is `n_slots*8`.
fn emit_compute_stage_delta(f: &mut Function, next_base: u32, off: u16) {
    emit_load_slot(f, next_base, u32::from(off));
    emit_load_slot(f, CURR_BASE, u32::from(off));
    f.instruction(&I::F64Sub);
    f.instruction(&I::LocalSet(L_RK_S));
}

/// The RK4 step (`vm.rs:712-787`): four stages over the compile-time stock
/// offsets, the time juggling, the final flows-only re-eval with restored
/// `curr`, and the `prev_values` snapshot. `next[off]` ends holding the new
/// integrated stock value; `curr` ends holding the time-`t` state.
fn emit_rk4_step(
    f: &mut Function,
    f_flows: u32,
    f_stocks: u32,
    dt: f64,
    stock_offsets: &[usize],
    regions: &RunRegions,
) {
    let (saved, accum) = (regions.rk_saved_base, regions.rk_accum_base);
    // The `next` chunk's slot-0 byte base == the chunk stride (`next` sits one
    // chunk past `curr`); see `emit_save_advance`.
    let next_base = regions.stride;

    // saved_time = curr[TIME]
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(TIME_ADDR)));
    f.instruction(&I::LocalSet(L_SAVED_TIME));

    // Stage 1 at (t, y): s1 = next-curr; saved=curr; accum=s1; curr=saved+s1*0.5
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u16);
        emit_compute_stage_delta(f, next_base, off);
        // saved[i] = curr[off]
        emit_store_slot_addr(f);
        emit_load_slot(f, CURR_BASE, u32::from(off));
        emit_store_slot_value(f, saved, i);
        // accum[i] = s1
        emit_store_slot_addr(f);
        f.instruction(&I::LocalGet(L_RK_S));
        emit_store_slot_value(f, accum, i);
        // curr[off] = saved[i] + s1*0.5
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&f64_const(0.5));
        f.instruction(&I::F64Mul);
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, CURR_BASE, u32::from(off));
    }
    // curr[TIME] = saved_time + dt*0.5
    emit_store_time_offset(f, dt * 0.5);

    // Stage 2 at (t+dt/2, y+s1/2): s2 = next-curr; accum+=2*s2; curr=saved+s2*0.5
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u16);
        emit_compute_stage_delta(f, next_base, off);
        // accum[i] += 2*s2
        emit_store_slot_addr(f);
        emit_load_slot(f, accum, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&f64_const(2.0));
        f.instruction(&I::F64Mul);
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, accum, i);
        // curr[off] = saved[i] + s2*0.5
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&f64_const(0.5));
        f.instruction(&I::F64Mul);
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, CURR_BASE, u32::from(off));
    }

    // Stage 3 at (t+dt/2, y+s2/2): s3 = next-curr; accum+=2*s3; curr=saved+s3
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u16);
        emit_compute_stage_delta(f, next_base, off);
        // accum[i] += 2*s3
        emit_store_slot_addr(f);
        emit_load_slot(f, accum, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&f64_const(2.0));
        f.instruction(&I::F64Mul);
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, accum, i);
        // curr[off] = saved[i] + s3
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, CURR_BASE, u32::from(off));
    }
    // curr[TIME] = saved_time + dt
    emit_store_time_offset(f, dt);

    // Stage 4 at (t+dt, y+s3): s4 = next-curr; accum+=s4;
    // next[off] = saved[i] + accum[i]/6; curr[off] = saved[i]
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u16);
        emit_compute_stage_delta(f, next_base, off);
        // accum[i] += s4
        emit_store_slot_addr(f);
        emit_load_slot(f, accum, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, accum, i);
        // next[off] = saved[i] + accum[i]/6.0
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        emit_load_slot(f, accum, i);
        f.instruction(&f64_const(6.0));
        f.instruction(&I::F64Div);
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, next_base, u32::from(off));
        // curr[off] = saved[i]  (restore the original)
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        emit_store_slot_value(f, CURR_BASE, u32::from(off));
    }

    // curr[TIME] = saved_time ; next[TIME] = saved_time + dt
    emit_restore_and_advance_time(f, dt, regions);

    // Final flows-only re-eval with the restored curr, so curr's aux/flow slots
    // hold time-`t` values (stages 2-4 clobbered them). Load-bearing for both
    // the saved output row and the PREVIOUS snapshot (`vm.rs:769-778`).
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_flows));

    emit_prev_snapshot(f, regions);
}

/// The RK2 (Heun) step (`vm.rs:788-838`): two stages, the time juggling, the
/// final flows-only re-eval, and the `prev_values` snapshot.
fn emit_rk2_step(
    f: &mut Function,
    f_flows: u32,
    f_stocks: u32,
    dt: f64,
    stock_offsets: &[usize],
    regions: &RunRegions,
) {
    let (saved, accum) = (regions.rk_saved_base, regions.rk_accum_base);
    // The `next` chunk's slot-0 byte base == the chunk stride; see
    // `emit_save_advance`.
    let next_base = regions.stride;

    // saved_time = curr[TIME]
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(TIME_ADDR)));
    f.instruction(&I::LocalSet(L_SAVED_TIME));

    // Stage 1 at (t, y): s1 = next-curr; saved=curr; accum=s1; curr=saved+s1
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u16);
        emit_compute_stage_delta(f, next_base, off);
        // saved[i] = curr[off]
        emit_store_slot_addr(f);
        emit_load_slot(f, CURR_BASE, u32::from(off));
        emit_store_slot_value(f, saved, i);
        // accum[i] = s1
        emit_store_slot_addr(f);
        f.instruction(&I::LocalGet(L_RK_S));
        emit_store_slot_value(f, accum, i);
        // curr[off] = saved[i] + s1   (full Euler step for the trial point)
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, CURR_BASE, u32::from(off));
    }
    // curr[TIME] = saved_time + dt
    emit_store_time_offset(f, dt);

    // Stage 2 at (t+dt, y+s1): s2 = next-curr; accum+=s2;
    // next[off] = saved[i] + accum[i]/2; curr[off] = saved[i]
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u16);
        emit_compute_stage_delta(f, next_base, off);
        // accum[i] += s2
        emit_store_slot_addr(f);
        emit_load_slot(f, accum, i);
        f.instruction(&I::LocalGet(L_RK_S));
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, accum, i);
        // next[off] = saved[i] + accum[i]/2.0
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        emit_load_slot(f, accum, i);
        f.instruction(&f64_const(2.0));
        f.instruction(&I::F64Div);
        f.instruction(&I::F64Add);
        emit_store_slot_value(f, next_base, u32::from(off));
        // curr[off] = saved[i]  (restore the original)
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        emit_store_slot_value(f, CURR_BASE, u32::from(off));
    }

    // curr[TIME] = saved_time ; next[TIME] = saved_time + dt
    emit_restore_and_advance_time(f, dt, regions);

    // Final flows-only re-eval with restored curr (see the RK4 comment).
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_flows));

    emit_prev_snapshot(f, regions);
}

/// `curr[TIME] = saved_time + offset` -- the trial-point time the stages run at
/// (`saved_time + dt*0.5` or `saved_time + dt`).
fn emit_store_time_offset(f: &mut Function, offset: f64) {
    f.instruction(&I::I32Const(0));
    f.instruction(&I::LocalGet(L_SAVED_TIME));
    f.instruction(&f64_const(offset));
    f.instruction(&I::F64Add);
    f.instruction(&I::F64Store(memarg(TIME_ADDR)));
}

/// Restore `curr[TIME] = saved_time` and set `next[TIME] = saved_time + dt`
/// (`vm.rs:759-760` / `818-819`), so the final flows re-eval runs at time `t`.
/// `next[TIME]` is set for faithfulness with the VM even though the wasm
/// save/advance tail advances via `curr[TIME] += dt` rather than reading it.
fn emit_restore_and_advance_time(f: &mut Function, dt: f64, regions: &RunRegions) {
    let next_time_addr = u64::from(regions.n_slots) * u64::from(SLOT_SIZE) + TIME_ADDR;
    // curr[TIME] = saved_time
    f.instruction(&I::I32Const(0));
    f.instruction(&I::LocalGet(L_SAVED_TIME));
    f.instruction(&I::F64Store(memarg(TIME_ADDR)));
    // next[TIME] = saved_time + dt
    f.instruction(&I::I32Const(0));
    f.instruction(&I::LocalGet(L_SAVED_TIME));
    f.instruction(&f64_const(dt));
    f.instruction(&I::F64Add);
    f.instruction(&I::F64Store(memarg(next_time_addr)));
}

/// Emit an unrolled `dst[0..n_slots] := src[0..n_slots]` f64 copy between two
/// linear-memory regions whose slot-0 byte bases are `src_base`/`dst_base`. Used
/// for the whole-chunk snapshots (`initial_values := curr`, `prev_values :=
/// curr`), each `n_slots` wide. The unroll matches the per-slot store style the
/// rest of `run` uses; `n_slots` is small for scalar models.
fn emit_copy_chunk(f: &mut Function, src_base: u32, dst_base: u32, n_slots: u32) {
    for slot in 0..n_slots {
        let slot_off = u64::from(slot) * u64::from(SLOT_SIZE);
        // f64.store wants [addr_i32, value_f64]; the constant `memarg.offset`
        // carries each region's base, so the dynamic address is a constant 0.
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(memarg(u64::from(src_base) + slot_off)));
        f.instruction(&I::F64Store(memarg(u64::from(dst_base) + slot_off)));
    }
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
    // `use_prev_fallback`: the only mutable global. Init 1 so `LoadPrev` returns
    // its fallback until the first `prev_values` snapshot clears it (`vm.rs:668`).
    globals.global(
        GlobalType {
            val_type: ValType::I32,
            mutable: true,
            shared: false,
        },
        &ConstExpr::i32_const(1),
    );
    wasm.section(&globals);

    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, n_helpers + F_RUN);
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("n_slots", ExportKind::Global, G_N_SLOTS);
    exports.export("n_chunks", ExportKind::Global, G_N_CHUNKS);
    exports.export("results_offset", ExportKind::Global, G_RESULTS_OFFSET);
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

    /// A graphical function whose table is `knots`. `Continuous` kind, with the
    /// x-scale spanning the knots' x-range.
    fn gf_from_knots(knots: &[(f64, f64)]) -> crate::datamodel::GraphicalFunction {
        use crate::datamodel;
        let x_points: Vec<f64> = knots.iter().map(|&(x, _)| x).collect();
        let y_points: Vec<f64> = knots.iter().map(|&(_, y)| y).collect();
        datamodel::GraphicalFunction {
            kind: datamodel::GraphicalFunctionKind::Continuous,
            x_points: Some(x_points.clone()),
            y_points,
            x_scale: datamodel::GraphicalFunctionScale {
                min: x_points.first().copied().unwrap_or(0.0),
                max: x_points.last().copied().unwrap_or(1.0),
            },
            y_scale: datamodel::GraphicalFunctionScale { min: 0.0, max: 1.0 },
        }
    }

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

    /// Task 3 (end-to-end): a model with a graphical-function variable looked up
    /// in all three modes -- `LOOKUP` (Interpolate), `LOOKUP FORWARD`, and
    /// `LOOKUP BACKWARD` -- matches the VM at every saved step. The lookup index
    /// is `TIME - 1`, which sweeps the table's x-domain plus a below-range
    /// margin (negative at t=0) and an above-range margin, so the recorded
    /// series exercise below/at-knot/between/above across the run.
    #[test]
    fn compile_simulation_gf_lookup_modes_match_vm() {
        let knots = [(0.0, 10.0), (1.0, 20.0), (2.5, 5.0), (4.0, 40.0)];
        let datamodel = crate::test_common::TestProject::new("gf_modes")
            // TIME 0..6, dt 0.25 -> index = TIME-1 sweeps -1..5 over [0,4] table.
            .with_sim_time(0.0, 6.0, 0.25)
            .aux("input", "TIME - 1", None)
            .aux_with_gf("curve", "0", gf_from_knots(&knots))
            .aux("interp_val", "LOOKUP(curve, input)", None)
            .aux("fwd_val", "LOOKUP_FORWARD(curve, input)", None)
            .aux("bwd_val", "LOOKUP_BACKWARD(curve, input)", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let checked = assert_matches_vm(sim, &artifact);
        // All five variables must reach parity: the three lookup-mode results
        // (interp/fwd/bwd), the lookup-only `curve` holder they read, and its
        // `input`. Pinning >= 5 (not just the 3 lookup modes) proves the
        // lookup-only curve holder and its driver also match the VM.
        assert!(
            checked >= 5,
            "expected to compare interp/fwd/bwd + curve + input, only checked {checked}"
        );
        for name in ["interp_val", "fwd_val", "bwd_val"] {
            assert!(
                artifact.layout.var_offsets.iter().any(|(n, _)| n == name),
                "{name} should be in the layout"
            );
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

    /// End-to-end VM parity for the `AllocateAvailable` opcode on the real
    /// `allocate.xmile` corpus model. The model's supply ramps from 0 to 10
    /// over the run while total demand is 9, so the recorded series sweep all
    /// three regimes -- `avail <= 0` (zeros) early, the partial-allocation
    /// bisection over rectangular priority profiles in the middle, and
    /// `avail >= total_demand` (full grant) once supply exceeds demand --
    /// against `Vm::new(sim).run_to_end()`. (The model is NOT in the active
    /// `wasm_parity_floor` corpus; raising that floor is a separate task.)
    #[test]
    fn compile_simulation_allocate_available_matches_vm() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test/sdeverywhere/models/allocate/allocate.xmile"
        );
        let file = std::fs::File::open(path).expect("open allocate xmile");
        let mut reader = BufReader::new(file);
        let datamodel = open_xmile(&mut reader).expect("parse allocate xmile");
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("allocate wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 5,
            "expected to compare the allocate model's variables, only checked {checked}"
        );
        assert!(
            artifact
                .layout
                .var_offsets
                .iter()
                .any(|(n, _)| n.starts_with("shipments")),
            "the arrayed shipments allocation should be in the layout"
        );
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

    // ── PREVIOUS / INIT (Task 1: snapshot regions + LoadPrev/LoadInitial) ──

    /// Task 1: `PREVIOUS(x)` under Euler. At t0 the snapshot has not been taken,
    /// so `LoadPrev` returns its fallback (the 0 the unary `PREVIOUS` desugars
    /// to); after the first step it returns the prior step's `x`. The series
    /// must match the VM, which gates the same fallback-vs-snapshot choice on
    /// `use_prev_fallback`.
    #[test]
    fn compile_simulation_previous_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("prev")
            .with_sim_time(0.0, 5.0, 1.0)
            // x ramps each step so PREVIOUS(x) is a visibly-lagged series.
            .stock("x", "10", &["grow"], &[], None)
            .flow("grow", "1", None)
            .aux("x_prev", "PREVIOUS(x)", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare x + x_prev");
    }

    /// Task 1: `INIT(x)` referenced from a flow reads the `initial_values`
    /// snapshot captured once after the initials phase (in the flows/stocks
    /// programs `LoadInitial` reads `initial_values[off]`, never `curr`). Here
    /// the inflow is held at `INIT(level)`, so `level` integrates by its own
    /// initial value each step; the wasm series must match the VM.
    #[test]
    fn compile_simulation_init_from_flow_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("init_flow")
            .with_sim_time(0.0, 5.0, 1.0)
            .stock("level", "7", &["inflow"], &[], None)
            // INIT(level) is captured once at t0 (= 7) and stays 7 every step.
            .flow("inflow", "INIT(level)", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare level + inflow");
        // level starts at 7 and grows by INIT(level)=7 each of 5 steps -> 42.
        let results = run_artifact_results(&artifact);
        let n_slots = artifact.layout.n_slots;
        let level_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "level")
            .map(|(_, off)| *off)
            .expect("level offset");
        let last = (artifact.layout.n_chunks - 1) * n_slots + level_off;
        assert!(
            (results[last] - 42.0).abs() < 1e-9,
            "level should reach 7 + 5*7 = 42, got {}",
            results[last]
        );
    }

    /// Task 1: `INIT(x)` referenced from *another initial equation* reads
    /// `curr` during the initials phase (the snapshot is taken only after
    /// initials run). `seed` is computed during initials, and `derived`'s
    /// initial equation reads `INIT(seed)` -- which must resolve to the
    /// just-computed `curr[seed]`, not an as-yet-unwritten `initial_values`.
    #[test]
    fn compile_simulation_init_from_initial_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("init_initial")
            .with_sim_time(0.0, 3.0, 1.0)
            .aux("seed", "5", None)
            // A stock whose INITIAL equation reads INIT(seed): during initials
            // LoadInitial must read curr[seed] (= 5), so derived starts at 5.
            .stock("derived", "INIT(seed)", &["hold"], &[], None)
            .flow("hold", "0", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare seed + derived");
        // derived initializes to INIT(seed)=5 and the flow holds it there.
        // Chunk 0 starts at slab offset 0, so `derived_off` indexes it directly.
        let results = run_artifact_results(&artifact);
        let derived_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "derived")
            .map(|(_, off)| *off)
            .expect("derived offset");
        assert!(
            (results[derived_off] - 5.0).abs() < 1e-9,
            "derived should initialize to INIT(seed) = 5, got {}",
            results[derived_off]
        );
    }

    // ── RK2 / RK4 integration loops (Task 2) ──────────────────────────────

    /// A logistic-growth model: `pop' = rate * pop * (1 - pop/capacity)`. The
    /// nonlinear flow depends on the stock, so RK's trial-point evaluations
    /// genuinely differ from Euler -- a pure-constant flow would let a broken RK
    /// loop pass by coincidence.
    fn logistic_growth(
        name: &str,
        method: crate::datamodel::SimMethod,
    ) -> crate::datamodel::Project {
        crate::test_common::TestProject::new(name)
            .with_sim_time(0.0, 20.0, 0.5)
            .with_sim_method(method)
            .aux("rate", "0.3", None)
            .aux("capacity", "1000", None)
            .stock("pop", "10", &["growth"], &[], None)
            .flow("growth", "rate * pop * (1 - pop / capacity)", None)
            .build_datamodel()
    }

    /// Task 2: an RK4 scalar model matches the VM's saved samples (cadence and
    /// values). The VM's RK4 loop is the oracle; the emitted four-stage loop
    /// with time juggling + the end-of-step flows re-eval must reproduce it.
    #[test]
    fn compile_simulation_rk4_matches_vm() {
        let datamodel = logistic_growth("rk4_logistic", crate::datamodel::SimMethod::RungeKutta4);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (RK4)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare pop + growth");
    }

    /// Task 2: an RK2 (Heun) scalar model matches the VM's saved samples. Same
    /// nonlinear model so the two-stage trial step is genuinely exercised.
    #[test]
    fn compile_simulation_rk2_matches_vm() {
        let datamodel = logistic_growth("rk2_logistic", crate::datamodel::SimMethod::RungeKutta2);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (RK2)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 2, "expected to compare pop + growth");
    }

    /// Task 2: RK4 and RK2 must genuinely differ from Euler on this nonlinear
    /// model -- otherwise the RK tests above could pass against a loop that
    /// silently fell back to Euler. Establishes that the oracle (the VM) sees a
    /// method-dependent trajectory, so wasm-vs-VM parity is a meaningful check.
    #[test]
    fn rk_methods_differ_from_euler_in_vm() {
        let last_pop = |method| {
            let datamodel = logistic_growth("rk_vs_euler", method);
            let sim = compile_sim(&datamodel, "main");
            let mut vm = Vm::new(sim).expect("vm");
            vm.run_to_end().expect("vm run");
            let results = vm.into_results();
            let pop = Ident::<Canonical>::from_str_unchecked("pop");
            let off = *results.offsets.get(&pop).expect("pop offset");
            results.data[(results.step_count - 1) * results.step_size + off]
        };
        let euler = last_pop(crate::datamodel::SimMethod::Euler);
        let rk4 = last_pop(crate::datamodel::SimMethod::RungeKutta4);
        let rk2 = last_pop(crate::datamodel::SimMethod::RungeKutta2);
        assert!(
            (euler - rk4).abs() > 1e-6,
            "RK4 must differ from Euler (euler={euler}, rk4={rk4})"
        );
        assert!(
            (euler - rk2).abs() > 1e-6,
            "RK2 must differ from Euler (euler={euler}, rk2={rk2})"
        );
    }

    /// A coupled two-stock Lotka-Volterra (predator-prey) model. Each stock's
    /// flows read the *other* stock, so a single RK stage's trial-point
    /// evaluation interleaves both stocks: `prey`'s `predation` outflow reads
    /// `predator`, and `predator`'s `growth` inflow reads `prey`. This is what
    /// the single-stock RK tests cannot exercise -- with two stocks the stage
    /// math walks `stock_offsets` and keeps each stock's `saved[i]`/`accum[i]`
    /// and trial `curr[off_i]` independent. A loop that aliased the scratch
    /// across stocks, or iterated `stock_offsets` in an unstable order, would
    /// corrupt one stock's trajectory and fail the VM-parity check below.
    ///
    /// Classic textbook parameters (alpha/beta/gamma/delta) on a short horizon
    /// with a small dt: the system oscillates, both stay strictly positive, and
    /// Euler vs RK4/RK2 visibly diverge (asserted by
    /// `multi_stock_coupled_diverges_euler_vs_rk_in_vm`). 100 steps keeps the
    /// un-JITed DLR-FT run well under the per-test budget.
    fn lotka_volterra(
        name: &str,
        method: crate::datamodel::SimMethod,
    ) -> crate::datamodel::Project {
        crate::test_common::TestProject::new(name)
            .with_sim_time(0.0, 5.0, 0.05)
            .with_sim_method(method)
            .aux("alpha", "1.1", None)
            .aux("beta", "0.4", None)
            .aux("gamma", "0.4", None)
            .aux("delta", "0.1", None)
            // prey:     d/dt = alpha*prey - beta*prey*predator
            .stock("prey", "10", &["prey_birth"], &["predation"], None)
            .flow("prey_birth", "alpha * prey", None)
            .flow("predation", "beta * prey * predator", None)
            // predator: d/dt = delta*prey*predator - gamma*predator
            .stock("predator", "10", &["pred_growth"], &["pred_death"], None)
            .flow("pred_growth", "delta * prey * predator", None)
            .flow("pred_death", "gamma * predator", None)
            .build_datamodel()
    }

    /// Meaningfulness precondition for the two-stock RK parity tests: the
    /// coupled model's trajectory is genuinely method-dependent in the VM (the
    /// oracle) for *both* stocks. Without this, a wasm RK loop that silently
    /// degraded to Euler -- or never advanced the second stock -- could pass
    /// `assert_matches_vm` against a coincidentally-identical VM Euler series.
    #[test]
    fn multi_stock_coupled_diverges_euler_vs_rk_in_vm() {
        let last_two = |method| {
            let datamodel = lotka_volterra("lv_vs_euler", method);
            let sim = compile_sim(&datamodel, "main");
            let mut vm = Vm::new(sim).expect("vm");
            vm.run_to_end().expect("vm run");
            let results = vm.into_results();
            let read = |name: &str| {
                let id = Ident::<Canonical>::from_str_unchecked(name);
                let off = *results
                    .offsets
                    .get(&id)
                    .unwrap_or_else(|| panic!("{name} offset"));
                results.data[(results.step_count - 1) * results.step_size + off]
            };
            (read("prey"), read("predator"))
        };
        let (e_prey, e_pred) = last_two(crate::datamodel::SimMethod::Euler);
        let (rk4_prey, rk4_pred) = last_two(crate::datamodel::SimMethod::RungeKutta4);
        let (rk2_prey, rk2_pred) = last_two(crate::datamodel::SimMethod::RungeKutta2);
        // Both stocks must move under RK4 and RK2 relative to Euler -- proving
        // the stage math integrates each independently, not just the first.
        assert!(
            (e_prey - rk4_prey).abs() > 1e-6 && (e_pred - rk4_pred).abs() > 1e-6,
            "RK4 must differ from Euler for both stocks \
             (prey: euler={e_prey} rk4={rk4_prey}; predator: euler={e_pred} rk4={rk4_pred})"
        );
        assert!(
            (e_prey - rk2_prey).abs() > 1e-6 && (e_pred - rk2_pred).abs() > 1e-6,
            "RK2 must differ from Euler for both stocks \
             (prey: euler={e_prey} rk2={rk2_prey}; predator: euler={e_pred} rk2={rk2_pred})"
        );
    }

    /// Coverage gap closed: a TWO-STOCK COUPLED model under RK4 matches the VM
    /// per-variable, per-chunk. The phase's other RK tests are single-stock, so
    /// this is the only check that the four-stage stage math keeps two stocks'
    /// `saved[i]`/`accum[i]`/`curr[off_i]` independent and iterates
    /// `stock_offsets` in a stable order across all four stages. `checked >= 2`
    /// pins that both stocks (not just `prey`) reached parity.
    #[test]
    fn compile_simulation_two_stock_coupled_rk4_matches_vm() {
        let datamodel = lotka_volterra("lv_rk4", crate::datamodel::SimMethod::RungeKutta4);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (two-stock RK4)");
        let checked = assert_matches_vm(sim, &artifact);
        // Both stocks plus the four flows and four params all match; pin >= 2 so
        // the two coupled stocks specifically are among the compared variables.
        assert!(
            checked >= 2,
            "expected to compare both prey + predator, only checked {checked}"
        );
        for name in ["prey", "predator"] {
            assert!(
                artifact.layout.var_offsets.iter().any(|(n, _)| n == name),
                "{name} should be in the layout"
            );
        }
    }

    /// The RK2 (Heun) companion to `compile_simulation_two_stock_coupled_rk4_matches_vm`:
    /// the two-stage trial step over two coupled stocks matches the VM.
    #[test]
    fn compile_simulation_two_stock_coupled_rk2_matches_vm() {
        let datamodel = lotka_volterra("lv_rk2", crate::datamodel::SimMethod::RungeKutta2);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (two-stock RK2)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 2,
            "expected to compare both prey + predator, only checked {checked}"
        );
    }

    /// Task 2: a model using `PREVIOUS`/`INIT` under RK4 matches the VM. The
    /// snapshot timing is the subtle part: `prev_values` is captured AFTER the
    /// end-of-step flows re-eval (with `curr` restored to time-`t` state), not
    /// from a trial point. `x_prev` lags `pop`; `pop_init` reads INIT(pop).
    #[test]
    fn compile_simulation_rk4_with_previous_and_init_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("rk4_prev_init")
            .with_sim_time(0.0, 10.0, 0.5)
            .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
            .aux("rate", "0.3", None)
            .aux("capacity", "1000", None)
            .stock("pop", "10", &["growth"], &[], None)
            .flow("growth", "rate * pop * (1 - pop / capacity)", None)
            // PREVIOUS(pop): lagged by one saved step; captured after re-eval.
            .aux("pop_prev", "PREVIOUS(pop)", None)
            // INIT(pop): the t0 snapshot (= 10), read from initial_values.
            .aux("pop_init", "INIT(pop)", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (RK4 + PREVIOUS/INIT)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 4,
            "expected to compare pop + growth + pop_prev + pop_init"
        );
    }

    /// After Task 2, RK4 (and RK2) are supported, so a model using them runs
    /// rather than being rejected -- the inverse of the Phase-1 guard. Pinned so
    /// a regression that re-introduced the Euler-only guard would be caught.
    #[test]
    fn compile_simulation_accepts_rk4() {
        let datamodel = crate::test_common::TestProject::new("rk4_accept")
            .with_sim_time(0.0, 5.0, 1.0)
            .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        compile_simulation(&sim).expect("RK4 must now be supported");
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

    // ── Array reducers end-to-end (Phase 5 Tasks 1-2) ─────────────────────
    //
    // These compile real reducer models through the production salsa pipeline
    // (so the bytecode is the genuine `PushStaticView; Array<Reduce>; PopView`
    // codegen emits, with all constant subscripts baked into the static view)
    // and assert the wasm matches the VM. They are the gold-standard parity
    // checks for Tasks 1-2; the inline `lower.rs` unit tests pin the individual
    // view ops against the VM's addressing oracle.

    /// Assert a single scalar variable's wasm series matches the VM, allowing a
    /// NaN-vs-NaN match (`assert_matches_vm` rejects NaN via its abs-diff
    /// tolerance, so the empty-view / OOB reducers need this NaN-aware variant).
    fn assert_scalar_matches_vm(sim: CompiledSimulation, artifact: &WasmArtifact, name: &str) {
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;
        let wasm_data = run_artifact_results(artifact);

        let mut vm = Vm::new(sim).expect("vm creation");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();

        let wasm_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, off)| *off)
            .unwrap_or_else(|| panic!("{name} not in wasm layout"));
        let ident = Ident::<Canonical>::from_str_unchecked(name);
        let vm_off = *vm_results
            .offsets
            .get(&ident)
            .unwrap_or_else(|| panic!("{name} not in vm offsets"));

        for c in 0..n_chunks {
            let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
            let wasm_val = wasm_data[c * n_slots + wasm_off];
            if vm_val.is_nan() {
                assert!(
                    wasm_val.is_nan(),
                    "{name} chunk {c}: vm=NaN but wasm={wasm_val}"
                );
            } else {
                assert!(
                    (vm_val - wasm_val).abs() < 1e-9,
                    "{name} chunk {c}: vm={vm_val} wasm={wasm_val}"
                );
            }
        }
    }

    /// A 1-D `SUM(source[3:5])` over an indexed dimension: a range subscript that
    /// codegen bakes into a static view with `offset=2`, `dims=[3]`. The whole
    /// model (including the arrayed `source`) must match the VM.
    #[test]
    fn compile_simulation_sum_range_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("sum_range")
            .with_sim_time(0.0, 3.0, 1.0)
            .indexed_dimension("A", 5)
            .array_aux("source[A]", "3 * A + 1")
            .scalar_aux("total", "SUM(source[3:5])")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (SUM range)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 1, "expected to compare source elements + total");
    }

    /// `SUM(values[*:SubA])` (star-range) selects a sparse subset of a named
    /// dimension's elements; codegen bakes the sparse mapping into the static
    /// view, exercising the sparse addressing path against the VM. (A transposed
    /// reducer like `SUM(matrix')` instead hoists into a `BeginIter` temp-copy
    /// loop, so it lands in Phase 5 Task 3; the transpose `ViewDesc` transform
    /// itself is pinned by `lower.rs`'s `view_transpose_then_reduce_matches_vm`.)
    #[test]
    fn compile_simulation_sum_star_range_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("sum_star_range")
            .with_sim_time(0.0, 2.0, 1.0)
            .named_dimension("DimA", &["A1", "A2", "A3", "A4"])
            .named_dimension("SubA", &["A2", "A3"])
            .array_with_ranges(
                "values[DimA]",
                vec![("A1", "10"), ("A2", "20"), ("A3", "30"), ("A4", "40")],
            )
            .scalar_aux("total", "SUM(values[*:SubA])")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (SUM star range)");
        // The whole model (including the sparse-selected `total` = A2+A3 = 50)
        // matches the VM element-for-element.
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 1);
        // Independently pin the sparse selection value against the VM.
        let sim2 = compile_sim(&datamodel, "main");
        assert_scalar_matches_vm(sim2, &artifact, "total");
    }

    /// A per-element sliced reducer `msum[D] = SUM(m[D, *])` over a 2-D array.
    /// Each output element is its own `PushStaticView; ArraySum; PopView` over a
    /// per-row static view (the A2A target unrolls to per-element bytecode).
    #[test]
    fn compile_simulation_sliced_row_sum_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("row_sum")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("D", 2)
            .indexed_dimension("E", 3)
            .array_aux("m[D, E]", "10 * D + E")
            .array_aux("msum[D]", "SUM(m[D, *])")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (row sum)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 1,
            "expected to compare m elements + msum elements"
        );
    }

    /// MEAN / STDDEV / MAX / MIN / SIZE over a range slice, each matching the VM.
    /// One model carries all five so a single compile exercises every reducer's
    /// production lowering.
    #[test]
    fn compile_simulation_all_reducers_match_vm() {
        let datamodel = crate::test_common::TestProject::new("all_reducers")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 5)
            .array_aux("source[A]", "2 * A")
            .scalar_aux("mean_val", "MEAN(source[2:4])")
            .scalar_aux("stddev_val", "STDDEV(source[1:5])")
            .scalar_aux("max_val", "MAX(source[2:4])")
            .scalar_aux("min_val", "MIN(source[2:4])")
            .scalar_aux("size_val", "SIZE(source[2:4])")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (all reducers)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 5, "expected to compare all five reducer results");
        for name in ["mean_val", "stddev_val", "max_val", "min_val", "size_val"] {
            assert!(
                artifact.layout.var_offsets.iter().any(|(n, _)| n == name),
                "{name} should be in the layout"
            );
        }
    }

    // The empty-but-valid view reducer asymmetry (SUM->0.0 vs others->NaN) and
    // the invalid-view->NaN-for-all asymmetry are pinned directly against the
    // VM's `reduce_view` semantics by the inline `lower.rs` unit tests
    // (`empty_valid_view_*` / `invalid_view_*`): a literal empty range
    // (`source[4:3]`) is rejected at compile time, and a runtime-empty range
    // (`source[start:end]` with `start > end`) plus an out-of-bounds dynamic
    // subscript both go through `ViewRangeDynamic` / `ViewSubscriptDynamic`,
    // which are Phase 5 Task 4, so the end-to-end coverage of those cases lands
    // there.

    // ── Phase 5 Task 3: BeginIter iteration loops (end-to-end) ────────────
    //
    // The broadcasting `LoadIterViewAt` path (source dims != iter dims) and the
    // standalone `BeginBroadcastIter` family are not reachable through the
    // current production codegen (an A2A elementwise op is scalar-unrolled, and a
    // mismatched-dim reducer argument fails the engine's own dimension check), so
    // those are pinned directly against the VM by hand-built-bytecode unit tests
    // in `lower.rs` (`iter_loop_*` / `broadcast_iter_*`). The two reachable
    // shapes -- a hoisted same-dim reducer loop and the deferred transpose
    // reducer -- are covered end-to-end here.

    /// `SUM(2 * source[3:5] + 1)`: the elementwise expression is hoisted into an
    /// `AssignTemp` `BeginIter` loop (codegen.rs:1183-1378), then `SUM` reduces
    /// the temp. The whole-model wasm must match the VM element-for-element.
    #[test]
    fn compile_simulation_hoisted_reducer_loop_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("hoist")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 5)
            .array_aux("source[A]", "A")
            .scalar_aux("summed", "SUM(2 * source[3:5] + 1)")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (hoisted reducer)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 1, "expected to compare summed");
    }

    /// `SUM(matrix')`: the transpose materializes the transposed matrix into a
    /// temp via a `BeginIter` loop reading the (transposed) source through
    /// `LoadIterViewAt`, then sums the temp. This is the case Subcomponent A
    /// deferred to the iteration task; the wasm must match the VM.
    #[test]
    fn compile_simulation_transpose_reducer_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("transpose")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 2)
            .indexed_dimension("B", 3)
            .array_aux("matrix[A,B]", "A * 10 + B")
            .scalar_aux("summed", "SUM(matrix')")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (transpose)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 1, "expected to compare summed");
    }

    // ── Phase 5 Task 4: dynamic subscripts + OOB->NaN (end-to-end) ────────

    /// Assert every layout variable matches the VM, treating a NaN on both sides
    /// as equal (the OOB-subscript result). The plain `assert_matches_vm` uses a
    /// finite-difference compare that a NaN would fail, so the OOB tests use this.
    fn assert_matches_vm_nan_aware(sim: CompiledSimulation, artifact: &WasmArtifact) -> usize {
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;
        let wasm_data = run_artifact_results(artifact);
        let mut vm = Vm::new(sim).expect("vm creation");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();
        assert_eq!(vm_results.step_count, n_chunks, "saved-chunk count differs");

        let mut checked = 0usize;
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(&vm_off) = vm_results.offsets.get(&ident) else {
                continue;
            };
            for c in 0..n_chunks {
                let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
                let wasm_val = wasm_data[c * n_slots + *wasm_off];
                if vm_val.is_nan() {
                    assert!(
                        wasm_val.is_nan(),
                        "{name} chunk {c}: vm=NaN but wasm={wasm_val}"
                    );
                } else {
                    let diff = (vm_val - wasm_val).abs();
                    assert!(diff < 1e-9, "{name} chunk {c}: vm={vm_val} wasm={wasm_val}");
                }
            }
            checked += 1;
        }
        checked
    }

    /// Legacy scalar dynamic subscript `arr[idx]` (`PushSubscriptIndex` /
    /// `LoadSubscript`), in range: the wasm must match the VM.
    #[test]
    fn compile_simulation_scalar_dynamic_subscript_in_range_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("dyn")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 4)
            .array_aux("arr[A]", "A * 10")
            .scalar_aux("idx", "3")
            .scalar_aux("picked", "arr[idx]")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 1, "expected to compare picked");
    }

    /// Legacy scalar dynamic subscript `arr[idx]` out of range -> NaN, matching
    /// the VM (`vm.rs:1343` sets the subscript invalid, `1361` pushes NaN).
    #[test]
    fn compile_simulation_scalar_dynamic_subscript_oob_is_nan() {
        // idx = 99 is well past the 4-element dimension -> NaN on both backends.
        let datamodel = crate::test_common::TestProject::new("dyn_oob")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 4)
            .array_aux("arr[A]", "A * 10")
            .scalar_aux("idx", "99")
            .scalar_aux("picked", "arr[idx]")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm_nan_aware(sim, &artifact);
        assert!(checked >= 1, "expected to compare picked");

        // Pin the NaN directly: `picked` must be NaN at every step.
        let n_slots = artifact.layout.n_slots;
        let off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "picked")
            .map(|(_, o)| *o)
            .expect("picked offset");
        let data = run_artifact_results(&artifact);
        for c in 0..artifact.layout.n_chunks {
            assert!(
                data[c * n_slots + off].is_nan(),
                "out-of-bounds arr[idx] must be NaN at chunk {c}"
            );
        }
    }

    /// `ViewSubscriptDynamic` via `SUM(mat[row, 1])`: a dynamically-subscripted
    /// view reduced to a scalar. In range, wasm matches the VM.
    #[test]
    fn compile_simulation_view_dynamic_subscript_in_range_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("vdyn")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 3)
            .indexed_dimension("B", 4)
            .array_aux("mat[A,B]", "A * 10 + B")
            .scalar_aux("row", "2")
            .scalar_aux("picked", "SUM(mat[row, 1])")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(checked >= 1, "expected to compare picked");
    }

    /// `ViewSubscriptDynamic` out of range -> the view is invalid -> the reducer
    /// yields NaN for *all* reducers, matching `reduce_view`'s `if !is_valid`.
    #[test]
    fn compile_simulation_view_dynamic_subscript_oob_is_nan() {
        let datamodel = crate::test_common::TestProject::new("vdyn_oob")
            .with_sim_time(0.0, 2.0, 1.0)
            .indexed_dimension("A", 3)
            .indexed_dimension("B", 4)
            .array_aux("mat[A,B]", "A * 10 + B")
            .scalar_aux("row", "99") // out of range for dim A (size 3)
            .scalar_aux("picked", "SUM(mat[row, 1])")
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let checked = assert_matches_vm_nan_aware(sim, &artifact);
        assert!(checked >= 1, "expected to compare picked");

        let n_slots = artifact.layout.n_slots;
        let off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "picked")
            .map(|(_, o)| *o)
            .expect("picked offset");
        let data = run_artifact_results(&artifact);
        for c in 0..artifact.layout.n_chunks {
            assert!(
                data[c * n_slots + off].is_nan(),
                "out-of-bounds SUM(mat[row,1]) must be NaN at chunk {c}"
            );
        }
    }
}
