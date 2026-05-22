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
//! one `initials`/`flows`/`stocks` function-triple *per unique `(model,
//! input_set)` module instance* in `CompiledSimulation.modules`, each taking a
//! runtime `module_off: i32` plus its module inputs as f64 params and lowered by
//! [`super::lower::emit_bytecode`] over the shared slab. An `EvalModule` `call`s
//! the child instance's function for the current phase (passing `module_off +
//! decl.off` and the inputs), so one shared `CompiledModule` runs at every base
//! offset it is instantiated at. A final `run` function seeds the reserved
//! globals, calls the *root* instance's initials, and drives the integration
//! loop. `run` lays the slab out as: a `curr` working chunk, a `next` working
//! chunk, then a results region of `n_chunks` step-major snapshots. It records a
//! snapshot of `curr` on the same cadence the bytecode VM uses (`vm.rs::run_to`):
//! the t=start sample is forced, then every `save_every = round(save_step/dt)`
//! steps, up to `n_chunks` samples.
//!
//! Unlike the VM's chunk-ring buffer, this uses a single `curr` chunk plus a
//! `next` chunk that holds only the freshly integrated stock values (including
//! nested-module stocks, collected by recursing through `EvalModule`): after
//! recording a snapshot, the updated stocks are copied back into `curr` and time
//! is advanced. Auxiliaries/flows are recomputed each step, so `curr` always
//! holds the full, correct state for the timestep it represents.
//!
//! Current scope: the full scalar + array opcode set, Euler/RK2/RK4 integration,
//! and nested modules (incl. SMOOTH/DELAY stdlib expansions). A genuine runtime
//! view range (`ViewRangeDynamic`) or array unrolling past the per-function
//! budget returns `WasmGenError::Unsupported`.

use wasm_encoder::Instruction as I;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, MemorySection, MemoryType, Module as WasmModule,
    TypeSection, ValType,
};

use std::collections::HashMap;

use crate::bytecode::{ByteCode, CompiledModule, Opcode};
use crate::results::{Method, Specs};
use crate::vm::{CompiledSimulation, ModuleKey, StepPart};

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
// exported indices 0/1/2 stay stable for hosts), all immutable. The mutable
// globals follow: `use_prev_fallback` at index 3, then the persistent step
// cursor (`saved`/`step_accum`/`did_initials`) at 4/5/6. The cursor globals make
// a run resumable: they survive across separate exported calls so `run_initials`
// can run once and each `run_to(target)` resumes from where the prior one
// stopped (the blob analogue of the VM's `curr_chunk`/`step_accum`/`did_initials`
// fields). They are internal -- not exported -- since a host drives the run only
// through `run`/`run_to`/`run_initials`/`reset`.
//
// `use_prev_fallback` gates `LoadPrev`: init 1 (return the fallback) until the
// first `prev_values` snapshot clears it (`vm.rs:668`); it is the inverse of the
// VM's `prev_values_valid`.
const G_N_SLOTS: u32 = 0;
const G_N_CHUNKS: u32 = 1;
const G_RESULTS_OFFSET: u32 = 2;
const G_USE_PREV_FALLBACK: u32 = 3;
// The persistent step cursor (mutable, internal):
const G_SAVED: u32 = 4; // saved-row counter (was the run-local `L_SAVED`)
const G_STEP_ACCUM: u32 = 5; // save-cadence accumulator (was `L_STEP_ACCUM`)
const G_DID_INITIALS: u32 = 6; // 0 until initials have run (cf. `Vm::did_initials`)

// `run_to`'s i32 locals. Its sole f64 *param* (the run target) occupies local 0,
// so the i32 working locals start at index 1 and `L_DST` is index 2 -- the same
// index the per-step emitters (`emit_save_advance`/`emit_rk*_step`) use, which
// lets those helpers stay shared between the (now removed) function-local cursor
// and the global cursor. Index 1 is an unused i32 filler that keeps `L_DST` at 2.
// The saved-row/step-accum cursor lives in `G_SAVED`/`G_STEP_ACCUM` (globals),
// not locals, so it survives across `run_to` calls.
const L_DST: u32 = 2;

/// Compile the named model of a datamodel `Project` to a full [`WasmArtifact`]
/// (the wasm blob plus its [`WasmLayout`]), through the salsa incremental
/// pipeline and [`compile_simulation`].
///
/// This is the entry point `libsimlin` uses across the FFI boundary
/// (`simlin_model_compile_to_wasm`): it works from a datamodel alone, with no
/// `Vm`/`SimlinSim`, returning both the blob and the name->offset layout. An
/// incremental-compile failure or an unsupported construct surfaces as
/// [`WasmGenError`] (the FFI maps it to a `SimlinError`, never a panic).
pub fn compile_datamodel_to_artifact(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
) -> Result<WasmArtifact, WasmGenError> {
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, datamodel, None);
    let sim =
        crate::db::compile_project_incremental(&db, sync.project, model_name).map_err(|e| {
            WasmGenError::Unsupported(format!("wasmgen: incremental compile failed: {e:?}"))
        })?;
    compile_simulation(&sim)
}

/// Compile the named model of a datamodel `Project` to a self-contained wasm
/// module, dropping the [`WasmLayout`] (callers that need the layout use
/// [`compile_datamodel_to_artifact`]). Kept as the stable raw-bytes entry point
/// for the `wasm-backend-poc.mjs` exploratory script and any blob-only consumer.
pub fn compile_datamodel_to_wasm(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
) -> Result<Vec<u8>, WasmGenError> {
    Ok(compile_datamodel_to_artifact(datamodel, model_name)?.wasm)
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

impl WasmLayout {
    /// Serialize the layout to a self-describing, length-prefixed byte buffer for
    /// the FFI (no protobuf -- it rides the same malloc-return convention as the
    /// wasm blob). The format is, all integers little-endian:
    ///
    /// ```text
    /// n_slots:        u64
    /// n_chunks:       u64
    /// results_offset: u64
    /// count:          u32              (number of var_offsets entries)
    /// repeated count times:
    ///     name_len:   u32
    ///     name:       name_len bytes   (UTF-8, the canonical variable name)
    ///     offset:     u64              (slot offset within a chunk)
    /// ```
    ///
    /// The GF region offsets are intentionally NOT serialized: a host reads
    /// results by name (via `n_slots`/`results_offset` + the name->offset map),
    /// never the GF regions directly. [`deserialize`] is the exact inverse over
    /// the geometry + name map (it leaves the GF offsets 0).
    ///
    /// [`deserialize`]: Self::deserialize
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(self.n_slots as u64).to_le_bytes());
        out.extend_from_slice(&(self.n_chunks as u64).to_le_bytes());
        out.extend_from_slice(&(self.results_offset as u64).to_le_bytes());
        out.extend_from_slice(&(self.var_offsets.len() as u32).to_le_bytes());
        for (name, offset) in &self.var_offsets {
            let bytes = name.as_bytes();
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(bytes);
            out.extend_from_slice(&(*offset as u64).to_le_bytes());
        }
        out
    }

    /// Parse a buffer produced by [`serialize`]. Returns `None` if the buffer is
    /// truncated, an integer is malformed, or a name is not valid UTF-8 -- a host
    /// gets a clean failure rather than a panic on a corrupt buffer. The GF region
    /// offsets are reconstructed as 0 (they are not in the serialized format).
    ///
    /// This is the inverse used by the libsimlin FFI tests and any host that wants
    /// to round-trip the layout in Rust; a non-Rust host re-implements the same
    /// little-endian parse against the documented format.
    ///
    /// [`serialize`]: Self::serialize
    pub fn deserialize(bytes: &[u8]) -> Option<WasmLayout> {
        let mut pos = 0usize;
        let take = |pos: &mut usize, n: usize| -> Option<&[u8]> {
            let end = pos.checked_add(n)?;
            let slice = bytes.get(*pos..end)?;
            *pos = end;
            Some(slice)
        };
        let read_u64 = |pos: &mut usize| -> Option<u64> {
            Some(u64::from_le_bytes(take(pos, 8)?.try_into().ok()?))
        };
        let read_u32 = |pos: &mut usize| -> Option<u32> {
            Some(u32::from_le_bytes(take(pos, 4)?.try_into().ok()?))
        };

        let n_slots = read_u64(&mut pos)? as usize;
        let n_chunks = read_u64(&mut pos)? as usize;
        let results_offset = read_u64(&mut pos)? as usize;
        let count = read_u32(&mut pos)? as usize;
        let mut var_offsets = Vec::with_capacity(count);
        for _ in 0..count {
            let name_len = read_u32(&mut pos)? as usize;
            let name_bytes = take(&mut pos, name_len)?;
            let name = std::str::from_utf8(name_bytes).ok()?.to_string();
            let offset = read_u64(&mut pos)? as usize;
            var_offsets.push((name, offset));
        }
        Some(WasmLayout {
            n_slots,
            n_chunks,
            results_offset,
            gf_directory_offset: 0,
            gf_data_offset: 0,
            var_offsets,
        })
    }
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

// Offsets of an instance's three program functions within its function-triple.
// The module's function slots are: the emitted helper functions
// ([`lower::build_helpers`]) at `0..n_helpers`, then one
// `[initials, flows, stocks]` triple per module instance (in `instance_order`),
// then `run` last. So instance `i`'s `StepPart` function is at
// `n_helpers + i*FUNCS_PER_INSTANCE + {F_INITIALS,F_FLOWS,F_STOCKS}`, and `run`
// is at `n_helpers + n_instances*FUNCS_PER_INSTANCE`. Keeping these relative
// (and adding `n_helpers`/the triple base at the call/export sites) means new
// helpers or instances shift the indices automatically.
const F_INITIALS: u32 = 0;
const F_FLOWS: u32 = 1;
const F_STOCKS: u32 = 2;
const FUNCS_PER_INSTANCE: u32 = 3;

/// The function index of `run` (the first driver function, after the helpers and
/// the per-instance triples). The driver functions follow in this fixed order:
/// `run`, `set_value`, `reset`, `clear_values`, `run_to`, `run_initials` (the two
/// resumable exports append last, keeping the original four at stable indices).
/// Used both at emit time (`compile_simulation`, to resolve the delegation
/// targets) and at assembly time (`assemble_simulation`), so the two never drift.
fn run_fn_index_of(n_helpers: u32, n_instances: u32) -> u32 {
    n_helpers + n_instances * FUNCS_PER_INSTANCE
}

// Type-section indices. The `run` type comes first; one opcode-program type per
// distinct module-input count follows (`(i32, f64*k) -> ()`), and helper types
// are appended after those. `run` is `() -> ()`.
const TYPE_RUN_FN: u32 = 0; // () -> ()

// Param 0 of every opcode-program function is `module_off` (i32); params
// `1..=n_inputs` are the f64 module inputs. Declared locals follow.
const L_MODULE_OFF: u32 = 0;

/// Everything an instance's `EmitCtx` needs that varies per `(model, input_set)`
/// module instance: its own `ByteCodeContext`, the disjoint linear-memory bases
/// the emitter threads in for that instance's array tables / GF lookups, its
/// module-input parameter count, and (when it has graphical functions) its slice
/// of the combined GF region. Computed once in [`compile_simulation`] before any
/// function is emitted, in `instance_order`.
struct PerInstance<'a> {
    module: &'a CompiledModule,
    /// Number of f64 module-input parameters this instance's three functions
    /// take (param 0 is `module_off`, params `1..=n_inputs` are the inputs).
    /// `0` for the root and any uninstantiated module. Drawn from the
    /// `EvalModule { n_inputs }` of its call sites (the count the VM passes).
    n_inputs: u32,
    /// Byte base of this instance's GF directory region (`0` when it has no
    /// graphical functions). Threaded into the instance's `EmitCtx`.
    gf_directory_base: u32,
    /// Byte base of this instance's GF data region (`0` when it has no GFs).
    gf_data_base: u32,
    /// Byte base of this instance's disjoint `temp_storage` region.
    temp_storage_base: u32,
    /// This instance's GF region image (directory + data + bases), for the
    /// `DataSection`; `None` when the instance has no graphical functions.
    gf_regions: Option<GfRegions>,
    /// The relative offsets this instance's module assigns via a flows
    /// `AssignConstCurr` -- its overridable constants (Phase 7 Task 2). Threaded
    /// into the instance's `EmitCtx` so an `AssignConstCurr { off }` whose `off`
    /// is in this set sources from the constants-override region.
    flows_const_offsets: std::collections::HashSet<u16>,
}

/// Compile a `CompiledSimulation` (produced by the salsa incremental pipeline)
/// into a self-contained wasm module.
///
/// Every unique `(model, input_set)` module instance in `sim.modules` becomes its
/// own initials/flows/stocks wasm function-triple taking `(module_off: i32,
/// in_0..in_{k-1}: f64)`; an `EvalModule` resolves the child instance and `call`s
/// its function for the current phase (passing `module_off + decl.off` and the
/// inputs), so one shared `CompiledModule` runs at every base offset it is
/// instantiated at. The opcode programs a `CompiledSimulation` carries are the
/// plain, un-fused scalar set (the VM's superinstruction fusion runs on a private
/// execution copy), so each `Opcode` lowers via [`lower::emit_bytecode`].
/// Anything outside the supported set -- an unsupported opcode, or array
/// unrolling past the per-function budget -- returns [`WasmGenError::Unsupported`]
/// rather than emitting a wrong module.
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
    let too_large = || WasmGenError::Unsupported("wasmgen: model too large to lower".to_string());

    // Enumerate every module instance in a deterministic order (sorted by key),
    // and the count of inputs each receives. The root receives 0 inputs (it is
    // called by `run`); every other instance's input count is the `n_inputs` of
    // its `EvalModule` call sites -- exactly what the VM sizes `module_inputs` to.
    let mut instance_order: Vec<ModuleKey> = sim.modules.keys().cloned().collect();
    instance_order.sort();
    let instance_n_inputs = collect_instance_input_counts(sim);

    // The stock data-buffer offsets the *whole simulation* integrates, recursing
    // through `EvalModule` so submodule (SMOOTH/DELAY) stocks are included --
    // mirroring the VM's `collect_stock_offsets` (`vm.rs:512-543`). The Euler
    // advance copies these `next -> curr`; the RK loops index `rk_scratch` by
    // their position here. Collected up front so the RK scratch region is sized
    // below.
    let stock_offsets = collect_all_stock_offsets(&sim.modules, &sim.root, 0);
    let n_stocks = u32::try_from(stock_offsets.len()).map_err(|_| too_large())?;
    // `n_slots` is the ROOT module's slot count, which spans the whole slab
    // including every nested module's slots (`vm.rs::n_slots` returns the root's).
    let n_slots = u32::try_from(root.n_slots).map_err(|_| too_large())?;
    let n_chunks = u32::try_from(specs.n_chunks).map_err(|_| too_large())?;
    let stride = n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let curr_base = 0u32;
    let next_base = stride;
    let results_base = stride.checked_mul(2).ok_or_else(too_large)?;
    let results_bytes = n_chunks.checked_mul(stride).ok_or_else(too_large)?;
    let mut total_bytes = results_base
        .checked_add(results_bytes)
        .ok_or_else(too_large)?;

    // Per-instance GF regions follow the results region, concatenated in
    // `instance_order` (each instance's directory+data sits at its own base, so
    // its directory entry 0 maps to its own table 0). The `Lookup` opcode reads
    // the directory at `instance_gf_directory_base + table_idx*8`, so each
    // instance's `EmitCtx` carries its own base. They are initialized at
    // instantiation by active `DataSection` segments.
    let mut instance_gf: HashMap<ModuleKey, (u32, u32, Option<GfRegions>)> = HashMap::new();
    for key in &instance_order {
        let module = &sim.modules[key];
        let regions = build_gf_regions(&module.context.graphical_functions, total_bytes)?;
        let (dir_base, data_base) = regions
            .as_ref()
            .map(|r| (r.directory_base, r.data_base))
            .unwrap_or((0, 0));
        if let Some(r) = &regions {
            total_bytes = total_bytes
                .checked_add(r.total_bytes)
                .ok_or_else(too_large)?;
        }
        instance_gf.insert(key.clone(), (dir_base, data_base, regions));
    }
    // The layout reports the ROOT instance's GF bases (a host reads results, not
    // GF directly; this preserves the single-root-model layout exactly).
    let (root_gf_directory_base, root_gf_data_base) = instance_gf
        .get(&sim.root)
        .map(|(d, dd, _)| (*d, *dd))
        .unwrap_or((0, 0));

    // The two snapshot regions follow the GF regions, each `n_slots` wide
    // (`vm.rs:617-618`). `initial_values` backs `INIT(x)` (captured once after
    // initials); `prev_values` backs `PREVIOUS(x)` (captured after each step, or
    // after the end-of-step flows re-eval under RK). Their bases are threaded
    // into every `EmitCtx` so `LoadInitial`/`LoadPrev` can address them. They are
    // shared across instances: a child reads `initial_values[module_off + off]`,
    // the same single snapshot the VM keeps.
    let snapshot_bytes = n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let initial_values_base = total_bytes;
    let prev_values_base = initial_values_base
        .checked_add(snapshot_bytes)
        .ok_or_else(too_large)?;
    total_bytes = prev_values_base
        .checked_add(snapshot_bytes)
        .ok_or_else(too_large)?;

    // The RK scratch region (`saved`(n_stocks) ++ `accum`(n_stocks)) follows the
    // snapshot regions. It holds each stock's stage-1 value and running RK
    // accumulator across the stages (`vm.rs:655`, the VM's `rk_scratch`
    // split). `n_stocks` now spans nested module stocks. Euler needs neither, so
    // the region is only reserved for RK.
    let rk = matches!(specs.method, Method::RungeKutta2 | Method::RungeKutta4);
    let stock_scratch_bytes = n_stocks.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    let rk_saved_base = total_bytes;
    let rk_accum_base = rk_saved_base
        .checked_add(stock_scratch_bytes)
        .ok_or_else(too_large)?;
    if rk {
        total_bytes = rk_accum_base
            .checked_add(stock_scratch_bytes)
            .ok_or_else(too_large)?;
    }

    // Per-instance `temp_storage` regions follow the snapshot/RK regions, one
    // disjoint region per instance (sized by that instance's `temp_total_size`).
    // The VM shares one `temp_storage` buffer across modules (per-module
    // `temp_offsets`); disjoint regions are unconditionally correct because a
    // parent's temps never survive across an `EvalModule` call (the child would
    // otherwise clobber a shared slot the VM relies on not surviving), so giving
    // each instance its own region cannot diverge from the VM. The largest
    // per-instance `temp_total_size` also bounds the shared vector/alloc scratch.
    let mut instance_temp_base: HashMap<ModuleKey, u32> = HashMap::new();
    let mut max_temp_total_size = 0u32;
    for key in &instance_order {
        let module = &sim.modules[key];
        let temp_total_size =
            u32::try_from(module.context.temp_total_size).map_err(|_| too_large())?;
        max_temp_total_size = max_temp_total_size.max(temp_total_size);
        instance_temp_base.insert(key.clone(), total_bytes);
        let temp_bytes = temp_total_size
            .checked_mul(SLOT_SIZE)
            .ok_or_else(too_large)?;
        total_bytes = total_bytes.checked_add(temp_bytes).ok_or_else(too_large)?;
    }

    // The vector-op + allocation scratch regions follow the temp regions. They
    // are shared across instances (the staging is within a single opcode, never
    // live across an `EvalModule` boundary -- the same reason the VM shares
    // them). A vector/alloc op's element count is bounded by the largest view it
    // processes, in turn bounded by the largest per-instance `temp_total_size`
    // and the slab's `n_slots`; see the detailed sizing invariant retained on the
    // per-region comments below. `2 * max(...)` f64 for the sort-pair vector
    // scratch, `6 * max(...)` f64 for the allocation staging.
    let scratch_view_bound = max_temp_total_size.max(n_slots);
    let vector_scratch_base = total_bytes;
    let vector_scratch_slots = scratch_view_bound.checked_mul(2).ok_or_else(too_large)?;
    let vector_scratch_bytes = vector_scratch_slots
        .checked_mul(SLOT_SIZE)
        .ok_or_else(too_large)?;
    total_bytes = vector_scratch_base
        .checked_add(vector_scratch_bytes)
        .ok_or_else(too_large)?;

    let alloc_scratch_base = total_bytes;
    let alloc_scratch_slots = scratch_view_bound.checked_mul(6).ok_or_else(too_large)?;
    let alloc_scratch_bytes = alloc_scratch_slots
        .checked_mul(SLOT_SIZE)
        .ok_or_else(too_large)?;
    total_bytes = alloc_scratch_base
        .checked_add(alloc_scratch_bytes)
        .ok_or_else(too_large)?;

    // The constants-override region (Phase 7 Task 2) follows the scratch regions:
    // an `n_slots`-wide f64 region indexed by ABSOLUTE slab offset, holding each
    // overridable constant's current value (initialized to the compiled default).
    // It is `n_slots` wide -- not `n_overridable` -- so a redirected
    // `AssignConstCurr { off }` reads it with the same `module_off`-relative
    // addressing the slab uses (`const_region_base + (module_off + off) * 8`),
    // which is what lets one shared `CompiledModule` running at several
    // `module_off`s pick up each instance's distinct override. A parallel
    // `n_slots`-byte validity region marks which absolute slots `set_value` may
    // write (1 = overridable). Both are initialized by active `DataSection`
    // segments built from `collect_overridable_defaults` (which mirrors the VM's
    // `collect_constant_info` recursion).
    let const_region_base = total_bytes;
    let const_region_bytes = n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?;
    total_bytes = const_region_base
        .checked_add(const_region_bytes)
        .ok_or_else(too_large)?;
    let const_valid_base = total_bytes;
    // One validity byte per slot.
    total_bytes = const_valid_base
        .checked_add(n_slots)
        .ok_or_else(too_large)?;

    let overridable_defaults = collect_overridable_defaults(&sim.modules, &sim.root, 0);
    // Defense in depth: the offsets `collect_overridable_defaults` reports must
    // be exactly the set the VM considers overridable (`constant_offsets`, the
    // keys of `cached_constant_info`). Both walk the same flows-`AssignConstCurr`
    // overridability rule, so any divergence is a bug -- a blob's `set_value`
    // would then accept/reject a different set than the VM. Checked only in debug.
    debug_assert!(
        {
            let mut ours: Vec<usize> = overridable_defaults.iter().map(|(off, _)| *off).collect();
            ours.sort_unstable();
            ours.dedup();
            let mut theirs: Vec<usize> = sim.constant_offsets().collect();
            theirs.sort_unstable();
            ours == theirs
        },
        "wasmgen overridable-constant offsets diverged from CompiledSimulation::constant_offsets"
    );

    let pages = total_bytes.div_ceil(WASM_PAGE_SIZE).max(1);

    // save_every mirrors vm.rs::run_to: max(1, round(save_step / dt)).
    let save_every = ((specs.save_step / specs.dt).round() as i64).max(1);
    let save_every = i32::try_from(save_every).map_err(|_| too_large())?;

    // Emitted helper functions occupy the module's first function slots; the
    // per-instance function-triples follow (at `n_helpers + i*FUNCS_PER_INSTANCE`
    // for instance `i`), and `run` is last. Build the helpers up front so the
    // index registry threaded into each `EmitCtx` matches the assembled module's
    // layout, and so `emit_bytecode`'s `call`s resolve.
    let helpers = build_helpers();
    let helper_fns = helpers.fns;
    let n_helpers = helpers.functions.len() as u32;

    // Assemble the per-instance descriptors and the `(ModuleKey, StepPart) -> fn
    // index` map. The map is built for ALL instances before any function body is
    // emitted, so an `EvalModule` in one instance's program resolves to the
    // child's already-known function index (the instantiation graph is acyclic,
    // but the index map does not depend on emit order regardless).
    let mut instances: Vec<PerInstance> = Vec::with_capacity(instance_order.len());
    let mut module_fn_index: HashMap<(ModuleKey, StepPart), u32> = HashMap::new();
    for (i, key) in instance_order.iter().enumerate() {
        let module = &sim.modules[key];
        let base = n_helpers + (i as u32) * FUNCS_PER_INSTANCE;
        module_fn_index.insert((key.clone(), StepPart::Initials), base + F_INITIALS);
        module_fn_index.insert((key.clone(), StepPart::Flows), base + F_FLOWS);
        module_fn_index.insert((key.clone(), StepPart::Stocks), base + F_STOCKS);
        let (gf_directory_base, gf_data_base, gf_regions) =
            instance_gf.remove(key).expect("gf entry per instance");
        instances.push(PerInstance {
            module,
            n_inputs: instance_n_inputs.get(key).copied().unwrap_or(0),
            gf_directory_base,
            gf_data_base,
            temp_storage_base: instance_temp_base[key],
            gf_regions,
            flows_const_offsets: flows_const_offsets_for(module),
        });
    }

    // Emit each instance's three program functions (initials/flows/stocks) over
    // the shared f64 slab, each lowered with that instance's own `ByteCodeContext`
    // and per-instance bases. `step_part` is per-program so `LoadInitial` picks
    // its `curr`-vs-snapshot branch at compile time (`vm.rs:1332-1340`), and an
    // `EvalModule` resolves the child's function for that same phase.
    let mut program_fns: Vec<Function> = Vec::with_capacity(instances.len() * 3);
    for inst in &instances {
        // `module_off` is the function's i32 param 0; inputs are params
        // `1..=n_inputs`. The reverse-pop scratch f64 base sits past all other
        // declared locals; the index helpers shift everything by `n_inputs`.
        let make_ctx = |cond_depth: usize, extra_i32: u32, step_part: StepPart| lower::EmitCtx {
            curr_base,
            next_base,
            gf_directory_base: inst.gf_directory_base,
            gf_data_base: inst.gf_data_base,
            initial_values_base,
            prev_values_base,
            use_prev_fallback_global: G_USE_PREV_FALLBACK,
            step_part,
            dt: specs.dt,
            start_time: specs.start,
            final_time: specs.stop,
            module_off_local: L_MODULE_OFF,
            scratch_local: lower::scratch_local_for(inst.n_inputs),
            condition_locals: lower::condition_locals_for(inst.n_inputs, cond_depth),
            apply_locals: lower::apply_locals_for(inst.n_inputs, cond_depth),
            helpers: helper_fns,
            temp_storage_base: inst.temp_storage_base,
            extra_i32_local_base: lower::extra_i32_local_base(inst.n_inputs, cond_depth),
            vector_f64_locals: lower::vector_f64_locals_for(inst.n_inputs, cond_depth),
            vector_i32_locals: lower::vector_i32_locals_for(inst.n_inputs, cond_depth),
            vector_scratch_base,
            alloc_scratch_base,
            module_input_scratch_base: lower::module_input_scratch_base(
                inst.n_inputs,
                cond_depth,
                extra_i32,
            ),
            const_region_base,
            flows_const_offsets: &inst.flows_const_offsets,
            module_fn_index: &module_fn_index,
            ctx: &inst.module.context,
        };
        program_fns.push(emit_initials_fn(inst.module, inst.n_inputs, &make_ctx)?);
        program_fns.push(emit_opcode_fn(
            &inst.module.compiled_flows,
            inst.n_inputs,
            StepPart::Flows,
            &make_ctx,
        )?);
        program_fns.push(emit_opcode_fn(
            &inst.module.compiled_stocks,
            inst.n_inputs,
            StepPart::Stocks,
            &make_ctx,
        )?);
    }

    // The root instance's initials/flows/stocks are driven with `module_off = 0`
    // and no inputs (the root takes none); child `EvalModule`s recurse from there.
    let root_idx = instance_order
        .iter()
        .position(|k| *k == sim.root)
        .expect("root is among the instances");
    let root_fn_base = n_helpers + (root_idx as u32) * FUNCS_PER_INSTANCE;
    let regions = RunRegions {
        n_slots,
        results_base,
        stride,
        n_chunks,
        initial_values_base,
        prev_values_base,
        rk_saved_base,
        rk_accum_base,
    };

    // Driver function indices, in the function-section order `assemble_simulation`
    // lays out after the per-instance triples: run, set_value, reset, clear_values,
    // run_to, run_initials. `run` and `run_to` delegate (`run` -> `reset` +
    // `run_to`; `run_to` -> `run_initials`), so their indices must be known before
    // their bodies are emitted -- the function section declares all indices up
    // front, so this is sound. Keeping run/set_value/reset/clear_values at their
    // original indices (the two new exports append after) keeps the change additive.
    let run_fn_index = run_fn_index_of(n_helpers, instances.len() as u32);
    let reset_fn_index = run_fn_index + 2;
    let run_to_fn_index = run_fn_index + 4;
    let run_initials_fn_index = run_fn_index + 5;

    // The resumable run ABI: `run_initials` (idempotent), `run_to(target)` (the
    // single shared stepping loop), and `run` (re-expressed as `reset;
    // run_to(stop)`). The cursor lives in mutable globals so a run is resumable.
    let run_initials_fn = emit_run_initials(specs, regions, root_fn_base);
    let run_to_fn = emit_run_to(
        specs,
        regions,
        save_every,
        &stock_offsets,
        root_fn_base,
        run_initials_fn_index,
    );
    let run_fn = emit_run(
        specs,
        RunFnIndices {
            run_to: run_to_fn_index,
            reset: reset_fn_index,
        },
    );

    // The constants-override exports (Phase 7 Task 2): `set_value` writes an
    // override into the constants region (validated against the validity bytes),
    // `reset` resets the run state (the cursor globals + `use_prev_fallback`)
    // without clearing the region, and `clear_values` restores the compiled
    // defaults.
    let set_value_fn = emit_set_value(n_slots, const_region_base, const_valid_base);
    let reset_fn = emit_reset();
    let clear_values_fn = emit_clear_values(const_region_base, &overridable_defaults);

    // The constants region + validity bytes are initialized at instantiation by
    // active data segments built from the overridable defaults (sparse writes,
    // one f64 + one validity byte per overridable absolute offset).
    let const_init =
        build_const_region_init(&overridable_defaults, const_region_base, const_valid_base);

    let instance_input_counts: Vec<u32> = instances.iter().map(|inst| inst.n_inputs).collect();
    let gf_images: Vec<&GfRegions> = instances
        .iter()
        .filter_map(|inst| inst.gf_regions.as_ref())
        .collect();
    let wasm = assemble_simulation(AssembleParts {
        helpers,
        program_fns,
        run_fn,
        set_value_fn,
        reset_fn,
        clear_values_fn,
        run_to_fn,
        run_initials_fn,
        instance_input_counts: &instance_input_counts,
        pages,
        n_slots,
        n_chunks,
        results_base,
        gf_regions: &gf_images,
        const_init: &const_init,
    });

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
            gf_directory_offset: root_gf_directory_base as usize,
            gf_data_offset: root_gf_data_base as usize,
            var_offsets,
        },
    })
}

/// The `n_inputs` (module-input parameter count) of each module instance, drawn
/// from the `EvalModule { n_inputs }` opcodes across every instance's three
/// programs. The root receives 0 inputs (it is invoked by `run` with none); a
/// child receives the count its callers pass -- the same value the VM sizes
/// `module_inputs` to. All call sites for a given `(model, input_set)` key agree
/// (the `input_set` is part of the key and `n_inputs == args.len()` at codegen,
/// `codegen.rs:1094-1109`); first-seen wins, which is therefore unambiguous.
fn collect_instance_input_counts(sim: &CompiledSimulation) -> HashMap<ModuleKey, u32> {
    let mut counts: HashMap<ModuleKey, u32> = HashMap::new();
    for module in sim.modules.values() {
        let programs: [&ByteCode; 2] = [&module.compiled_flows, &module.compiled_stocks];
        let initial_codes = module.compiled_initials.iter().map(|ci| &ci.bytecode);
        for bc in programs.into_iter().chain(initial_codes) {
            for op in &bc.code {
                if let Opcode::EvalModule { id, n_inputs } = op {
                    let decl = &module.context.modules[*id as usize];
                    let child_key = crate::vm::make_module_key(&decl.model_name, &decl.input_set);
                    counts.entry(child_key).or_insert(u32::from(*n_inputs));
                }
            }
        }
    }
    counts
}

/// Build an instance's `initials` function: every `CompiledInitial`'s bytecode
/// in order, over the shared slab. The shared condition-local count is the max
/// nesting depth across all the initials (they run sequentially in one function);
/// the reverse-pop scratch covers the max `EvalModule { n_inputs }` over them.
/// `n_inputs` is the instance's module-input parameter count (shifts the locals).
fn emit_initials_fn<'a>(
    module: &CompiledModule,
    n_inputs: u32,
    make_ctx: &impl Fn(usize, u32, StepPart) -> lower::EmitCtx<'a>,
) -> Result<Function, WasmGenError> {
    let cond_depth = module
        .compiled_initials
        .iter()
        .map(|ci| max_condition_depth(&ci.bytecode))
        .max()
        .unwrap_or(0);
    // The initials run sequentially in one function; each fragment's dynamic-
    // subscript accumulation (and `EvalModule` reverse-pop) completes before the
    // next, so reserving the *max* per-fragment count -- not the sum -- is
    // correct, and the fragments reuse the same scratch locals.
    let extra_i32 = module
        .compiled_initials
        .iter()
        .map(|ci| lower::count_extra_i32_locals(&ci.bytecode))
        .max()
        .unwrap_or(0);
    let module_input_scratch = module
        .compiled_initials
        .iter()
        .map(|ci| lower::count_module_input_scratch(&ci.bytecode))
        .max()
        .unwrap_or(0);
    let ctx = make_ctx(cond_depth, extra_i32, StepPart::Initials);
    let mut f = new_opcode_fn(n_inputs, cond_depth, extra_i32, module_input_scratch);
    for ci in module.compiled_initials.iter() {
        lower::emit_bytecode(&ci.bytecode, &ctx, &mut f)?;
    }
    f.instruction(&I::End);
    Ok(f)
}

/// Build one opcode-program function from a single `ByteCode`, lowering it as
/// `step_part` (which `LoadInitial` reads to pick its `curr`-vs-snapshot branch,
/// and which an `EvalModule` calls the child's matching phase function for).
/// `n_inputs` is the instance's module-input parameter count.
fn emit_opcode_fn<'a>(
    bc: &ByteCode,
    n_inputs: u32,
    step_part: StepPart,
    make_ctx: &impl Fn(usize, u32, StepPart) -> lower::EmitCtx<'a>,
) -> Result<Function, WasmGenError> {
    let cond_depth = max_condition_depth(bc);
    let extra_i32 = lower::count_extra_i32_locals(bc);
    let module_input_scratch = lower::count_module_input_scratch(bc);
    let ctx = make_ctx(cond_depth, extra_i32, step_part);
    let mut f = new_opcode_fn(n_inputs, cond_depth, extra_i32, module_input_scratch);
    lower::emit_bytecode(bc, &ctx, &mut f)?;
    f.instruction(&I::End);
    Ok(f)
}

/// A fresh opcode-program `Function` for an instance with `n_inputs` f64 input
/// params: the scratch f64 local, `cond_depth` i32 condition locals, the three
/// `Apply` scratch f64 locals, the vector-op scratch, `extra_i32`
/// dynamic-subscript scratch i32 locals, and `module_input_scratch` `EvalModule`
/// reverse-pop f64 locals (param 0 = `module_off`, params `1..=n_inputs` =
/// inputs). The declaration list lives in [`lower::opcode_fn_locals`] (which is
/// param-count-independent); the index helpers shift by `n_inputs`.
fn new_opcode_fn(
    n_inputs: u32,
    cond_depth: usize,
    extra_i32: u32,
    module_input_scratch: u32,
) -> Function {
    // `n_inputs` is in the function's *type* (its params), not the declared
    // locals list; it is applied at `assemble_simulation` where the type is
    // chosen, so it does not appear here.
    let _ = n_inputs;
    Function::new(lower::opcode_fn_locals(
        cond_depth,
        extra_i32,
        module_input_scratch,
    ))
}

/// Collect absolute offsets of all stock variables across the whole simulation,
/// recursing into child modules via `EvalModule` so submodule (SMOOTH/DELAY)
/// stocks are included. Mirrors the VM's `collect_stock_offsets`
/// (`vm.rs:512-543`) exactly: a stock writes via `AssignNext` or its
/// peephole-fused `BinOpAssignNext` (most integrations are `stock + delta`), and
/// an `EvalModule` recurses with `base_off + decl.off` (each instance addresses
/// its slot at `base_off + off`). After each step these slots are copied `next ->
/// curr`; the RK loops index `rk_scratch[saved/accum]` by their sorted position.
fn collect_all_stock_offsets(
    modules: &HashMap<ModuleKey, CompiledModule>,
    key: &ModuleKey,
    base_off: usize,
) -> Vec<usize> {
    let module = match modules.get(key) {
        Some(m) => m,
        None => return Vec::new(),
    };
    let mut offsets: Vec<usize> = Vec::new();
    for op in module.compiled_stocks.code.iter() {
        match op {
            Opcode::AssignNext { off } | Opcode::BinOpAssignNext { off, .. } => {
                offsets.push(base_off + *off as usize);
            }
            Opcode::EvalModule { id, .. } => {
                let decl = &module.context.modules[*id as usize];
                let child_key = crate::vm::make_module_key(&decl.model_name, &decl.input_set);
                offsets.extend(collect_all_stock_offsets(
                    modules,
                    &child_key,
                    base_off + decl.off,
                ));
            }
            _ => {}
        }
    }
    // Defensive dedup, as the VM does: duplicate offsets would double-copy.
    offsets.sort_unstable();
    offsets.dedup();
    offsets
}

/// The set of *relative* offsets a module assigns via an `AssignConstCurr` in
/// its **flows** phase: exactly this module's overridable constants. Mirrors the
/// first (flows-only) pass of the VM's `collect_constant_info` (`vm.rs:436-450`),
/// but keyed by relative offset and computed per module, so it is compile-time
/// even for a shared `CompiledModule` instantiated at several base offsets (every
/// instantiation's `base_off + off` is overridable, since `collect_constant_info`
/// recurses through every declaration). An `AssignConstCurr { off }` in any phase
/// whose `off` is in this set is redirected to read the constants-override
/// region; one whose `off` is absent emits its immediate literal.
fn flows_const_offsets_for(module: &CompiledModule) -> std::collections::HashSet<u16> {
    module
        .compiled_flows
        .code
        .iter()
        .filter_map(|op| match op {
            Opcode::AssignConstCurr { off, .. } => Some(*off),
            _ => None,
        })
        .collect()
}

/// Collect `(absolute offset, compiled-default literal)` for every overridable
/// constant across the whole simulation, recursing through `EvalModule`
/// declarations with cumulative `base_off`. Mirrors the VM's `collect_constant_info`
/// (`vm.rs:426-507`): an offset is overridable iff some module assigns it via an
/// `AssignConstCurr` in its **flows** phase, and the default value is that flows
/// `AssignConstCurr`'s literal. Used to size and initialize the constants-override
/// region so the wasm blob's `set_value` accepts exactly the offsets the VM's
/// `set_value_by_offset` does, each initialized to the same compiled default.
///
/// A shared module instantiated at two base offsets contributes both absolute
/// offsets (one per instantiation), exactly as the VM's recursion does.
fn collect_overridable_defaults(
    modules: &HashMap<ModuleKey, CompiledModule>,
    key: &ModuleKey,
    base_off: usize,
) -> Vec<(usize, f64)> {
    let module = match modules.get(key) {
        Some(m) => m,
        None => return Vec::new(),
    };
    let mut out: Vec<(usize, f64)> = Vec::new();
    for op in module.compiled_flows.code.iter() {
        if let Opcode::AssignConstCurr { off, literal_id } = op {
            // The literal is the flows assignment's compiled default. A
            // well-formed program always has the literal in range; fall back to
            // 0.0 defensively rather than panicking across what is otherwise an
            // infallible layout pass.
            let v = module
                .compiled_flows
                .literals
                .get(*literal_id as usize)
                .copied()
                .unwrap_or(0.0);
            out.push((base_off + *off as usize, v));
        }
    }
    for decl in &module.context.modules {
        let child_key = crate::vm::make_module_key(&decl.model_name, &decl.input_set);
        out.extend(collect_overridable_defaults(
            modules,
            &child_key,
            base_off + decl.off,
        ));
    }
    out
}

/// The linear-memory region geometry the run driver needs: the chunk/results
/// bases, the snapshot bases (`initial_values`/`prev_values`), and the RK scratch
/// bases (`saved`/`accum`). Bundled to keep the `emit_run_initials`/`emit_run_to`
/// signatures small as the run loop gained snapshot + RK regions.
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

// `run_to`'s f64 locals. The RK loops need a `saved_time` (the timestep's t,
// restored after the stages move `curr[TIME]` to trial points) and a per-stage
// `s` scratch (`next[off]-curr[off]`). Euler declares them too -- two unused f64
// locals are free. They sit at indices 3/4: `run_to`'s f64 param is local 0 and
// its two i32 working locals (index 1 filler + `L_DST` at 2) precede them.
const L_SAVED_TIME: u32 = 3;
const L_RK_S: u32 = 4;

/// `run_to`'s f64 param: the run target (the strict upper bound on `curr[TIME]`),
/// at local 0. The loop steps until `curr[TIME] > target`.
const RT_TARGET: u32 = 0;

/// The function indices `run`'s delegating body calls: `run` is re-expressed as
/// `reset(); run_to(stop)` (one shared stepping loop). The indices are resolved
/// in `compile_simulation` before the bodies are emitted (the function section
/// declares all indices up front). (`run_to` calls `run_initials` directly via
/// its own index argument, so that index is not threaded here.)
#[derive(Clone, Copy)]
struct RunFnIndices {
    run_to: u32,
    reset: u32,
}

/// Emit `run_initials() -> ()`: seed the reserved time slots, run the root
/// initials, capture `initial_values`, and arm the step cursor -- but only the
/// first time per `reset`. Idempotent via the `G_DID_INITIALS` guard, mirroring
/// `vm.rs:1080-1082` (`if self.did_initials { return Ok(()); }`), so a `run_to`
/// after another `run_to` re-runs initials zero times and resumes the existing
/// cursor instead.
fn emit_run_initials(specs: &Specs, regions: RunRegions, root_fn_base: u32) -> Function {
    let mut f = Function::new([]);

    // if G_DID_INITIALS != 0: return  (idempotency -- already initialized).
    f.instruction(&I::GlobalGet(G_DID_INITIALS));
    f.instruction(&I::If(BlockType::Empty));
    f.instruction(&I::Return);
    f.instruction(&I::End);

    let f_initials = root_fn_base + F_INITIALS;

    // Seed the reserved global slots into curr (chunk base 0), mirroring the VM,
    // which writes start/dt/start/stop into TIME/DT/INITIAL_TIME/FINAL_TIME before
    // run_initials.
    store_curr_const_abs(&mut f, TIME_OFF, specs.start);
    store_curr_const_abs(&mut f, DT_OFF, specs.dt);
    store_curr_const_abs(&mut f, INITIAL_TIME_OFF, specs.start);
    store_curr_const_abs(&mut f, FINAL_TIME_OFF, specs.stop);

    // Arm the PREVIOUS fallback for this run, mirroring the VM's `run_initials`
    // (which sets `use_prev_fallback = true`). `reset` also re-arms it, but a bare
    // `run_initials` (no `reset` first, e.g. the resumable test driver) must arm
    // it here too so a `PREVIOUS(x)` evaluated during initials returns its
    // fallback. The first `run_to` step clears it after the first `prev_values`
    // snapshot.
    f.instruction(&I::I32Const(1));
    f.instruction(&I::GlobalSet(G_USE_PREV_FALLBACK));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_initials));

    // Capture `initial_values := curr` exactly once, after initials, for `INIT(x)`
    // reads in the flows/stocks programs (`vm.rs:1124-1128`).
    emit_copy_chunk(
        &mut f,
        CURR_BASE,
        regions.initial_values_base,
        regions.n_slots,
    );

    // Arm the cursor: nothing saved yet, accumulator cleared, initials done. The
    // first save happens in `run_to`'s loop (the forced t=start row), matching the
    // VM (`run_initials` does not save chunk 0).
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_SAVED));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_STEP_ACCUM));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::GlobalSet(G_DID_INITIALS));

    f.instruction(&I::End); // end function
    f
}

/// Emit `run_to(target: f64) -> ()`: advance the simulation until `curr[TIME] >
/// target` (strict `>`, matching `vm.rs:644`), starting from wherever the
/// persistent cursor left off. Calls `run_initials` first (idempotent), then runs
/// the per-method stepping loop -- the single shared stepping-loop implementation
/// both `run` and `run_to` use. The loop reads/writes the saved-row cursor from
/// `G_SAVED`/`G_STEP_ACCUM` (globals), so it resumes correctly across calls; the
/// saved-row exhaustion break (`if saved >= n_chunks`) clamps a target past
/// FINAL_TIME to the slab end, exactly like the VM's chunk-ring exhaustion.
fn emit_run_to(
    specs: &Specs,
    regions: RunRegions,
    save_every: i32,
    stock_offsets: &[usize],
    root_fn_base: u32,
    run_initials_idx: u32,
) -> Function {
    // One f64 param (`target`, local 0) + two i32 locals (index 1 filler, `L_DST`
    // at 2) + two f64 locals (`saved_time`, `s` at 3/4). The cursor lives in
    // globals, not locals; the i32 at index 1 is unused filler that keeps `L_DST`
    // at the index the per-step emitters expect.
    let mut f = Function::new([(2, ValType::I32), (2, ValType::F64)]);

    // Absolute function indices of the ROOT instance's three program functions:
    // its function-triple base + the per-phase offset. The root is driven with
    // `module_off = 0`; nested instances are reached via `EvalModule` from there.
    let f_flows = root_fn_base + F_FLOWS;
    let f_stocks = root_fn_base + F_STOCKS;

    // Idempotent initials (seeds time slots, runs initials, arms the cursor on the
    // first call after a reset; a no-op otherwise).
    f.instruction(&I::Call(run_initials_idx));

    f.instruction(&I::Block(BlockType::Empty)); // $break
    f.instruction(&I::Loop(BlockType::Empty)); // $continue

    // if curr[TIME] > target: break
    f.instruction(&I::I32Const(0));
    f.instruction(&I::F64Load(memarg(TIME_ADDR)));
    f.instruction(&I::LocalGet(RT_TARGET));
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
    // and `curr[TIME] += dt`. The saved-row counter is the `G_SAVED` global, so
    // the cursor survives across `run_to` calls.
    emit_save_advance(&mut f, specs, save_every, stock_offsets, &regions);

    f.instruction(&I::Br(0)); // continue
    f.instruction(&I::End); // end loop
    f.instruction(&I::End); // end block
    f.instruction(&I::End); // end function
    f
}

/// Emit `run() -> ()` for the `CompiledSimulation` path by *delegating* to the
/// resumable ABI: `reset(); run_to(stop)`. This keeps exactly one stepping-loop
/// implementation (in `run_to`), so `run` and `run_to` can never drift apart.
///
/// Invariant (the linchpin): `run()` must produce a full from-t0 simulation on
/// every call to a reused instance. The delegation satisfies this for free --
/// `reset` clears `G_DID_INITIALS`/`G_SAVED`/`G_STEP_ACCUM` and re-arms
/// `G_USE_PREV_FALLBACK = 1`, so the subsequent `run_to` -> `run_initials` (no
/// longer short-circuited, since `reset` cleared `G_DID_INITIALS`) re-seeds the
/// reserved time slots and re-runs initials from scratch.
fn emit_run(specs: &Specs, indices: RunFnIndices) -> Function {
    let mut f = Function::new([]);
    f.instruction(&I::Call(indices.reset));
    f.instruction(&f64_const(specs.stop));
    f.instruction(&I::Call(indices.run_to));
    f.instruction(&I::End);
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

    // The saved-row counter (`G_SAVED`) and the save-cadence accumulator
    // (`G_STEP_ACCUM`) are mutable globals, not function locals, so the cursor
    // persists across the separate `run_to` calls a resumable run makes. `L_DST`
    // is a per-step transient and stays a function local.

    // step_accum += 1
    f.instruction(&I::GlobalGet(G_STEP_ACCUM));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::I32Add);
    f.instruction(&I::GlobalSet(G_STEP_ACCUM));

    // save_cond = (step_accum == save_every) | (saved == 0 & time == start)
    f.instruction(&I::GlobalGet(G_STEP_ACCUM));
    f.instruction(&I::I32Const(save_every));
    f.instruction(&I::I32Eq);
    f.instruction(&I::GlobalGet(G_SAVED));
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
    f.instruction(&I::GlobalGet(G_SAVED));
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
    f.instruction(&I::GlobalGet(G_SAVED));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::I32Add);
    f.instruction(&I::GlobalSet(G_SAVED));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_STEP_ACCUM));

    // if saved >= n_chunks: break (depth 2: if -> loop -> block)
    f.instruction(&I::GlobalGet(G_SAVED));
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

// ── Constants-override exports (Phase 7 Task 2) ───────────────────────────
//
// `set_value(offset: i32, val: f64) -> i32` writes the override into the
// constants region (0 ok / 1 when `offset` is out of range or not overridable);
// `reset() -> ()` resets the run state without clearing the region (overrides
// persist across reset, like the VM); `clear_values() -> ()` restores the
// compiled defaults. The constants region is `n_slots`-wide and indexed by
// absolute slab offset (so a redirected `AssignConstCurr` reads it with the same
// `module_off`-relative addressing the slab uses); a parallel `n_slots`-byte
// validity region (1 = overridable) is what `set_value` checks.

/// A `MemArg` for a single-byte access (the validity region), align 0.
fn byte_memarg(addr: u64) -> wasm_encoder::MemArg {
    wasm_encoder::MemArg {
        offset: addr,
        align: 0,
        memory_index: 0,
    }
}

// `set_value`'s i32 params: the absolute slab offset and (param 1) the f64
// value. Param 0 is the offset.
const SV_OFFSET: u32 = 0;
const SV_VALUE: u32 = 1;

/// Emit `set_value(offset: i32, val: f64) -> i32`: write `const_region[offset] =
/// val` and return 0 when `offset` is a valid overridable slot, else return 1
/// without writing. Validity is `0 <= offset < n_slots` AND `valid[offset] != 0`
/// (the byte the data segment set for each overridable absolute offset). This
/// mirrors the VM's `set_value_by_offset` (`vm.rs:1037-1052`): an out-of-range or
/// non-constant offset is rejected (the VM returns `Err`), a valid one applies
/// the override (which persists across `reset`).
fn emit_set_value(n_slots: u32, const_region_base: u32, const_valid_base: u32) -> Function {
    let mut f = Function::new([]);

    // if (offset < 0) | (offset >= n_slots): return 1
    f.instruction(&I::LocalGet(SV_OFFSET));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::I32LtS);
    f.instruction(&I::LocalGet(SV_OFFSET));
    f.instruction(&I::I32Const(n_slots as i32));
    f.instruction(&I::I32GeS);
    f.instruction(&I::I32Or);
    f.instruction(&I::If(BlockType::Empty));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::Return);
    f.instruction(&I::End);

    // if valid[offset] == 0: return 1   (valid byte at const_valid_base + offset)
    f.instruction(&I::LocalGet(SV_OFFSET));
    f.instruction(&I::I32Load8U(byte_memarg(u64::from(const_valid_base))));
    f.instruction(&I::I32Eqz);
    f.instruction(&I::If(BlockType::Empty));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::Return);
    f.instruction(&I::End);

    // const_region[offset] = val   (f64 at const_region_base + offset*8)
    f.instruction(&I::LocalGet(SV_OFFSET));
    f.instruction(&I::I32Const(SLOT_SIZE as i32));
    f.instruction(&I::I32Mul);
    f.instruction(&I::LocalGet(SV_VALUE));
    f.instruction(&I::F64Store(memarg(u64::from(const_region_base))));

    // return 0
    f.instruction(&I::I32Const(0));
    f.instruction(&I::End);
    f
}

/// Emit `reset() -> ()`: clear the persistent run state so the next `run_to`
/// (and therefore `run`, which delegates `reset; run_to(stop)`) re-runs initials
/// and steps the loop from t=start. The run cursor now lives in mutable globals
/// (since the run is resumable), so `reset` must clear all of it:
/// `G_SAVED`/`G_STEP_ACCUM` to 0 (no rows saved, accumulator empty),
/// `G_DID_INITIALS` to 0 (so `run_initials` no longer short-circuits and re-seeds
/// the time slots + re-runs initials), and `G_USE_PREV_FALLBACK` back to 1 (the
/// analogue of the VM's `reset` clearing `prev_values_valid`). This mirrors
/// `vm.rs:989-1002` exactly. Like the VM, it deliberately does NOT touch the
/// constants-override region, so a `set_value` override persists across `reset`.
fn emit_reset() -> Function {
    let mut f = Function::new([]);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_SAVED));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_STEP_ACCUM));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_DID_INITIALS));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::GlobalSet(G_USE_PREV_FALLBACK));
    f.instruction(&I::End);
    f
}

/// Emit `clear_values() -> ()`: restore each overridable constant to its
/// compiled-default literal by writing the defaults back into the constants
/// region (the VM's `clear_values`, `vm.rs:1055-1062`). The defaults are
/// compile-time constants, so this is a straight-line sequence of `f64.store`s --
/// one per overridable absolute offset. The data segment also writes these at
/// instantiation; `clear_values` lets a host undo a `set_value` without
/// re-instantiating the module.
fn emit_clear_values(const_region_base: u32, overridable_defaults: &[(usize, f64)]) -> Function {
    let mut f = Function::new([]);
    for &(abs_off, default) in overridable_defaults {
        f.instruction(&I::I32Const(0));
        f.instruction(&f64_const(default));
        f.instruction(&I::F64Store(memarg(
            u64::from(const_region_base) + abs_off as u64 * u64::from(SLOT_SIZE),
        )));
    }
    f.instruction(&I::End);
    f
}

/// The active `DataSection` payloads that initialize the constants region and
/// its validity bytes at instantiation: for each overridable absolute offset, the
/// f64 default written into the constants region and a `1` validity byte. Sparse
/// (one segment per overridable offset), so a model with no overridable constants
/// produces an empty list (no segments).
struct ConstRegionInit {
    /// `(byte address within the constants region, the 8 LE bytes of the default)`.
    value_segments: Vec<(u32, [u8; 8])>,
    /// `byte address within the validity region` (the byte written is always 1).
    valid_segments: Vec<u32>,
}

/// Build the constants-region init payloads from the overridable defaults.
fn build_const_region_init(
    overridable_defaults: &[(usize, f64)],
    const_region_base: u32,
    const_valid_base: u32,
) -> ConstRegionInit {
    let mut value_segments = Vec::with_capacity(overridable_defaults.len());
    let mut valid_segments = Vec::with_capacity(overridable_defaults.len());
    for &(abs_off, default) in overridable_defaults {
        let value_addr = const_region_base + abs_off as u32 * SLOT_SIZE;
        value_segments.push((value_addr, default.to_le_bytes()));
        valid_segments.push(const_valid_base + abs_off as u32);
    }
    ConstRegionInit {
        value_segments,
        valid_segments,
    }
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
///
/// `off` is the full-width absolute slot offset (`u32`, like the Euler advance's
/// `emit_save_advance`). A `u16` here would silently truncate a stock at slot
/// 65536 or above -- reachable in a large nested model (each submodel / SMOOTH /
/// DELAY instance adds slots, with no cap on total `n_slots`) -- to
/// `off & 0xFFFF`, clobbering an unrelated slot (offset 65536 maps to slot 0,
/// TIME).
fn emit_compute_stage_delta(f: &mut Function, next_base: u32, off: u32) {
    emit_load_slot(f, next_base, off);
    emit_load_slot(f, CURR_BASE, off);
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
        let (i, off) = (i as u32, off as u32);
        emit_compute_stage_delta(f, next_base, off);
        // saved[i] = curr[off]
        emit_store_slot_addr(f);
        emit_load_slot(f, CURR_BASE, off);
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
        emit_store_slot_value(f, CURR_BASE, off);
    }
    // curr[TIME] = saved_time + dt*0.5
    emit_store_time_offset(f, dt * 0.5);

    // Stage 2 at (t+dt/2, y+s1/2): s2 = next-curr; accum+=2*s2; curr=saved+s2*0.5
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u32);
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
        emit_store_slot_value(f, CURR_BASE, off);
    }

    // Stage 3 at (t+dt/2, y+s2/2): s3 = next-curr; accum+=2*s3; curr=saved+s3
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u32);
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
        emit_store_slot_value(f, CURR_BASE, off);
    }
    // curr[TIME] = saved_time + dt
    emit_store_time_offset(f, dt);

    // Stage 4 at (t+dt, y+s3): s4 = next-curr; accum+=s4;
    // next[off] = saved[i] + accum[i]/6; curr[off] = saved[i]
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u32);
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
        emit_store_slot_value(f, next_base, off);
        // curr[off] = saved[i]  (restore the original)
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        emit_store_slot_value(f, CURR_BASE, off);
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
        let (i, off) = (i as u32, off as u32);
        emit_compute_stage_delta(f, next_base, off);
        // saved[i] = curr[off]
        emit_store_slot_addr(f);
        emit_load_slot(f, CURR_BASE, off);
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
        emit_store_slot_value(f, CURR_BASE, off);
    }
    // curr[TIME] = saved_time + dt
    emit_store_time_offset(f, dt);

    // Stage 2 at (t+dt, y+s1): s2 = next-curr; accum+=s2;
    // next[off] = saved[i] + accum[i]/2; curr[off] = saved[i]
    emit_eval_step(f, f_flows, f_stocks);
    for (i, &off) in stock_offsets.iter().enumerate() {
        let (i, off) = (i as u32, off as u32);
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
        emit_store_slot_value(f, next_base, off);
        // curr[off] = saved[i]  (restore the original)
        emit_store_slot_addr(f);
        emit_load_slot(f, saved, i);
        emit_store_slot_value(f, CURR_BASE, off);
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

/// Inputs to [`assemble_simulation`], grouped to keep the signature small now
/// that the module carries a per-instance function-triple (one per
/// `(model, input_set)`) plus a `run` driver, and possibly several GF regions.
struct AssembleParts<'a> {
    helpers: BuiltHelpers,
    /// The instances' program functions in `instance_order`, flattened as
    /// `[initials_0, flows_0, stocks_0, initials_1, ...]`. `instance_input_counts`
    /// (same instance order) gives each triple's f64 input-param count.
    program_fns: Vec<Function>,
    /// `run() -> ()`, re-expressed as `reset; run_to(stop)`.
    run_fn: Function,
    /// `set_value(offset: i32, val: f64) -> i32` (Phase 7 Task 2).
    set_value_fn: Function,
    /// `reset() -> ()` (Phase 7 Task 2; now also clears the run cursor globals).
    reset_fn: Function,
    /// `clear_values() -> ()` (Phase 7 Task 2).
    clear_values_fn: Function,
    /// `run_to(target: f64) -> ()`: advance the resumable run to `target`.
    run_to_fn: Function,
    /// `run_initials() -> ()`: idempotent initials for the resumable run.
    run_initials_fn: Function,
    /// Module-input parameter count per instance, in the same order the triples
    /// appear in `program_fns`. Drives the per-triple wasm type
    /// (`(i32, f64*k) -> ()`).
    instance_input_counts: &'a [u32],
    pages: u32,
    n_slots: u32,
    n_chunks: u32,
    results_base: u32,
    /// Every GF-bearing instance's region image, for the active `DataSection`
    /// segments (each instance's directory + data sit at distinct bases).
    gf_regions: &'a [&'a GfRegions],
    /// The constants-override region init payloads (Phase 7 Task 2): sparse
    /// active `DataSection` segments seeding each overridable slot's f64 default
    /// and its validity byte.
    const_init: &'a ConstRegionInit,
}

/// Assemble the simulation module: types, functions, memory, globals, exports,
/// code, and (when present) the GF data segments. Layout: the emitted helper
/// functions ([`build_helpers`]) lead the function/code sections (indices
/// `0..n_helpers`); then one `[initials, flows, stocks]` triple per module
/// instance (in `instance_order`); then `run` last. Exports `memory`, `run`, and
/// the three self-describing i32 geometry globals. Each GF-bearing instance
/// contributes two active `DataSection` segments (its directory + data) at its
/// own bases.
fn assemble_simulation(parts: AssembleParts) -> Vec<u8> {
    let AssembleParts {
        helpers,
        program_fns,
        run_fn,
        set_value_fn,
        reset_fn,
        clear_values_fn,
        run_to_fn,
        run_initials_fn,
        instance_input_counts,
        pages,
        n_slots,
        n_chunks,
        results_base,
        gf_regions,
        const_init,
    } = parts;

    let mut wasm = WasmModule::new();
    let n_helpers = helpers.functions.len() as u32;
    let n_instances = instance_input_counts.len() as u32;
    // Function layout: helpers, the per-instance triples, then the driver
    // functions in this fixed order: `run`, `set_value`, `reset`, `clear_values`,
    // `run_to`, `run_initials`. The two resumable exports append last so the
    // original four keep stable indices (the growth is purely additive). The
    // emit-time index math in `compile_simulation` uses the same `run_fn_index_of`.
    let run_fn_index = run_fn_index_of(n_helpers, n_instances);
    let set_value_fn_index = run_fn_index + 1;
    let reset_fn_index = run_fn_index + 2;
    let clear_values_fn_index = run_fn_index + 3;
    let run_to_fn_index = run_fn_index + 4;
    let run_initials_fn_index = run_fn_index + 5;

    // Type section: `run`'s `() -> ()` first, then one opcode-program type per
    // *distinct* module-input count (`(i32, f64*k) -> ()`, sorted), then the
    // helper types, then the `set_value` type (`(i32, f64) -> i32`), then
    // `run_to`'s `(f64) -> ()` type. `reset`/`clear_values`/`run_initials` reuse
    // `TYPE_RUN_FN` (`() -> ()`). `opcode_type_for` maps an instance's `n_inputs`
    // to its type index; a helper at function index `i` uses the type appended
    // after those.
    let mut distinct_inputs: Vec<u32> = instance_input_counts.to_vec();
    distinct_inputs.sort_unstable();
    distinct_inputs.dedup();
    let opcode_type_index: HashMap<u32, u32> = distinct_inputs
        .iter()
        .enumerate()
        .map(|(i, &k)| (k, TYPE_RUN_FN + 1 + i as u32))
        .collect();
    let first_helper_type = TYPE_RUN_FN + 1 + distinct_inputs.len() as u32;
    let set_value_type = first_helper_type + helpers.functions.len() as u32;
    let run_to_type = set_value_type + 1;

    let mut types = TypeSection::new();
    types.ty().function([], []); // TYPE_RUN_FN: () -> ()
    for &k in &distinct_inputs {
        // (module_off: i32, in_0..in_{k-1}: f64) -> ()
        let mut params: Vec<ValType> = Vec::with_capacity(1 + k as usize);
        params.push(ValType::I32);
        params.extend(std::iter::repeat_n(ValType::F64, k as usize));
        types.ty().function(params, []);
    }
    for hf in &helpers.functions {
        types.ty().function(hf.params.clone(), hf.results.clone());
    }
    // `set_value(offset: i32, val: f64) -> i32`.
    types
        .ty()
        .function([ValType::I32, ValType::F64], [ValType::I32]);
    // `run_to(target: f64) -> ()`.
    types.ty().function([ValType::F64], []);
    wasm.section(&types);

    // Function section: helpers first (indices `0..n_helpers`), then each
    // instance's three program functions (typed by that instance's `n_inputs`),
    // then the driver functions in index order: `run`, `set_value`, `reset`,
    // `clear_values`, `run_to`, `run_initials`.
    let mut functions = FunctionSection::new();
    for (i, _) in helpers.functions.iter().enumerate() {
        functions.function(first_helper_type + i as u32);
    }
    for &k in instance_input_counts {
        let ty = opcode_type_index[&k];
        functions.function(ty); // initials
        functions.function(ty); // flows
        functions.function(ty); // stocks
    }
    functions.function(TYPE_RUN_FN); // run
    functions.function(set_value_type); // set_value
    functions.function(TYPE_RUN_FN); // reset
    functions.function(TYPE_RUN_FN); // clear_values
    functions.function(run_to_type); // run_to
    functions.function(TYPE_RUN_FN); // run_initials
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
    let mutable_i32_global = || GlobalType {
        val_type: ValType::I32,
        mutable: true,
        shared: false,
    };
    let mut globals = GlobalSection::new();
    globals.global(i32_global(), &ConstExpr::i32_const(n_slots as i32));
    globals.global(i32_global(), &ConstExpr::i32_const(n_chunks as i32));
    globals.global(i32_global(), &ConstExpr::i32_const(results_base as i32));
    // The mutable globals (index 3..=6), all internal. `use_prev_fallback` (index
    // 3) inits 1 so `LoadPrev` returns its fallback until the first `prev_values`
    // snapshot clears it (`vm.rs:668`). The persistent step cursor follows:
    // `G_SAVED`/`G_STEP_ACCUM`/`G_DID_INITIALS` (4/5/6), all init 0 -- the
    // module-init state is "no rows saved, accumulator empty, initials not yet
    // run", which `run_initials` arms and `reset` restores.
    globals.global(mutable_i32_global(), &ConstExpr::i32_const(1)); // G_USE_PREV_FALLBACK
    globals.global(mutable_i32_global(), &ConstExpr::i32_const(0)); // G_SAVED
    globals.global(mutable_i32_global(), &ConstExpr::i32_const(0)); // G_STEP_ACCUM
    globals.global(mutable_i32_global(), &ConstExpr::i32_const(0)); // G_DID_INITIALS
    wasm.section(&globals);

    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, run_fn_index);
    exports.export("set_value", ExportKind::Func, set_value_fn_index);
    exports.export("reset", ExportKind::Func, reset_fn_index);
    exports.export("clear_values", ExportKind::Func, clear_values_fn_index);
    // The resumable run ABI (purely additive to the export set above).
    exports.export("run_to", ExportKind::Func, run_to_fn_index);
    exports.export("run_initials", ExportKind::Func, run_initials_fn_index);
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("n_slots", ExportKind::Global, G_N_SLOTS);
    exports.export("n_chunks", ExportKind::Global, G_N_CHUNKS);
    exports.export("results_offset", ExportKind::Global, G_RESULTS_OFFSET);
    wasm.section(&exports);

    // Code section order must match the function section: helper bodies, then the
    // per-instance program functions (in `program_fns` order), then the driver
    // functions in index order: `run`, `set_value`, `reset`, `clear_values`,
    // `run_to`, `run_initials`.
    let mut code = CodeSection::new();
    for hf in &helpers.functions {
        code.function(&hf.body);
    }
    for program in &program_fns {
        code.function(program);
    }
    code.function(&run_fn);
    code.function(&set_value_fn);
    code.function(&reset_fn);
    code.function(&clear_values_fn);
    code.function(&run_to_fn);
    code.function(&run_initials_fn);
    wasm.section(&code);

    // The GF directory + data regions and the constants-override init values
    // are read-only-at-instantiation constants; active data segments write each
    // at its byte address when the module is instantiated. A module has at most
    // one data section, so the GF regions and the constants-override init share
    // it. The data section must follow the code section per the wasm binary order.
    let has_const_init =
        !const_init.value_segments.is_empty() || !const_init.valid_segments.is_empty();
    if !gf_regions.is_empty() || has_const_init {
        let mut data = DataSection::new();
        for gf in gf_regions {
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
        }
        // The constants region's per-slot default (8 LE bytes each) and its
        // validity bytes (a single `1` each), one active segment per overridable
        // absolute offset.
        for &(addr, bytes) in &const_init.value_segments {
            data.active(0, &ConstExpr::i32_const(addr as i32), bytes.iter().copied());
        }
        for &addr in &const_init.valid_segments {
            data.active(0, &ConstExpr::i32_const(addr as i32), [1u8].iter().copied());
        }
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

    /// Task 3 (pure serializer): a `WasmLayout` round-trips through
    /// `serialize`/`deserialize` -- the geometry and the full name->offset map are
    /// recovered exactly. The GF offsets are not part of the wire format (a host
    /// reads results by name), so they come back as 0.
    #[test]
    fn wasm_layout_serialize_round_trips() {
        let layout = WasmLayout {
            n_slots: 7,
            n_chunks: 101,
            results_offset: 112,
            gf_directory_offset: 4096,
            gf_data_offset: 4104,
            var_offsets: vec![
                ("time".to_string(), 0),
                ("population".to_string(), 4),
                ("a_var_with_a_longer_name".to_string(), 6),
            ],
        };
        let bytes = layout.serialize();
        let back = WasmLayout::deserialize(&bytes).expect("round-trip must succeed");
        assert_eq!(back.n_slots, 7);
        assert_eq!(back.n_chunks, 101);
        assert_eq!(back.results_offset, 112);
        assert_eq!(back.var_offsets, layout.var_offsets);
        // The GF offsets are not serialized; they reconstruct as 0.
        assert_eq!(back.gf_directory_offset, 0);
        assert_eq!(back.gf_data_offset, 0);
    }

    /// Task 3 (serializer robustness): a truncated buffer deserializes to `None`
    /// rather than panicking, so a host handed a corrupt buffer fails cleanly.
    #[test]
    fn wasm_layout_deserialize_truncated_is_none() {
        let layout = WasmLayout {
            n_slots: 2,
            n_chunks: 3,
            results_offset: 32,
            gf_directory_offset: 0,
            gf_data_offset: 0,
            var_offsets: vec![("x".to_string(), 0), ("y".to_string(), 1)],
        };
        let bytes = layout.serialize();
        // Every strict prefix of a valid buffer must fail to parse (each cuts off
        // a length-prefixed field mid-way).
        for cut in 0..bytes.len() {
            assert!(
                WasmLayout::deserialize(&bytes[..cut]).is_none(),
                "a buffer truncated to {cut} bytes must not deserialize"
            );
        }
        // The full buffer parses.
        assert!(WasmLayout::deserialize(&bytes).is_some());
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
        // a single root instance of three empty (0-input) program functions.
        let helpers = build_helpers();
        let empty = || {
            let mut f = Function::new([]);
            f.instruction(&I::End);
            f
        };
        let pages = (region_base + regions.total_bytes)
            .div_ceil(WASM_PAGE_SIZE)
            .max(1);
        let empty_const_init = ConstRegionInit {
            value_segments: Vec::new(),
            valid_segments: Vec::new(),
        };
        let wasm = assemble_simulation(AssembleParts {
            helpers,
            program_fns: vec![empty(), empty(), empty()],
            run_fn: empty(),
            // Empty (no-op) override functions: this test only checks the GF data
            // segments, so the override exports are present but trivial.
            set_value_fn: {
                let mut f = Function::new([]);
                // A `(i32, f64) -> i32` body must leave an i32 on the stack.
                f.instruction(&I::I32Const(0));
                f.instruction(&I::End);
                f
            },
            reset_fn: empty(),
            clear_values_fn: empty(),
            // Empty (no-op) resumable-run functions: this test only checks the GF
            // data segments. `run_to` is `(f64) -> ()` and `run_initials` is
            // `() -> ()`; an empty body type-checks against either (the type comes
            // from the function section, and a no-op leaves the stack empty).
            run_to_fn: empty(),
            run_initials_fn: empty(),
            instance_input_counts: &[0],
            pages,
            n_slots: 0,
            n_chunks: 0,
            results_base: 0,
            gf_regions: &[&regions],
            const_init: &empty_const_init,
        });

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

    /// Instantiate `artifact` ONCE and invoke the exported `run` `runs` times in
    /// sequence with no `reset` between, returning the results slab read after
    /// each call. Models the wasm backend's documented "instantiate once, re-run
    /// on every change" usage (interactive scrubbing; the POC's `run` "re-runs
    /// the whole simulation" per call) -- which exercises the cross-run state
    /// reset that a single `run` invocation cannot.
    fn run_artifact_results_repeated(artifact: &WasmArtifact, runs: usize) -> Vec<Vec<f64>> {
        let info = validate(&artifact.wasm).expect("generated module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let n = artifact.layout.n_chunks * artifact.layout.n_slots;
        let base = artifact.layout.results_offset;
        let mut out = Vec::with_capacity(runs);
        for _ in 0..runs {
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
            let slab = store.mem_access_mut_slice(mem, |bytes| {
                (0..n)
                    .map(|i| {
                        let a = base + i * 8;
                        f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                    })
                    .collect::<Vec<f64>>()
            });
            out.push(slab);
        }
        out
    }

    /// Regression (PR #620 review): `run` reseeds the time globals and reruns
    /// initials, so it is a complete simulation from t0 and the documented
    /// per-change entry point for repeated re-simulation. It must therefore
    /// reset the PREVIOUS fallback flag itself, mirroring the VM's `run_initials`
    /// (which sets `use_prev_fallback = true` at the start of every run). Without
    /// that reset, the loop leaves the flag at 0, so a SECOND `run` on the same
    /// instance reads the first run's final `prev_values` on step 0 (and during
    /// initials) instead of the fallback -- contaminating any `PREVIOUS(...)`
    /// model. This instantiates once and runs twice with no `reset` between: a
    /// deterministic model must produce identical results both times, and
    /// `x_prev` at t0 must be the unary-PREVIOUS fallback (0), not the stale
    /// prior-run value.
    #[test]
    fn compile_simulation_repeated_run_resets_previous_fallback() {
        let datamodel = crate::test_common::TestProject::new("prev_repeat")
            .with_sim_time(0.0, 5.0, 1.0)
            .stock("x", "10", &["grow"], &[], None)
            .flow("grow", "1", None)
            .aux("x_prev", "PREVIOUS(x)", None)
            .build_datamodel();

        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let runs = run_artifact_results_repeated(&artifact, 2);
        let (first, second) = (&runs[0], &runs[1]);

        // A deterministic model re-run from t0 produces byte-identical results;
        // the bug makes the second run's PREVIOUS reads diverge on step 0.
        assert_eq!(
            first, second,
            "second run() diverged from the first -- stale PREVIOUS fallback state leaked across runs"
        );

        // Pin the discriminating cell: x_prev at the first saved chunk (t0) is
        // the unary-PREVIOUS fallback (0), not the prior run's final x.
        let x_prev_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(name, _)| name == "x_prev")
            .map(|(_, off)| *off)
            .expect("x_prev in layout");
        assert_eq!(
            second[x_prev_off], 0.0,
            "x_prev at t0 on the second run must be the PREVIOUS fallback (0), got {}",
            second[x_prev_off]
        );
    }

    /// Regression (PR #620 review): a stock at an absolute slot offset >= 65536
    /// must address its real slot under RK integration, not `off & 0xFFFF`. Such
    /// offsets are reachable in a large nested model (each submodel/SMOOTH/DELAY
    /// instance adds slots; nothing caps total `n_slots` in the wasm path). The
    /// RK stage delta `next[off] - curr[off]` is computed by
    /// `emit_compute_stage_delta`; the original bug threaded `off` as `u16`, so a
    /// stock at offset 65536 read slot `65536 & 0xFFFF == 0` (TIME) instead of its
    /// own. This drives the helper at offset 65536 over a hand-built memory whose
    /// slot 0 and slot 65536 hold distinct values and asserts it reads slot 65536
    /// (matching the Euler advance, which has always used the full-width offset).
    #[test]
    fn rk_stage_delta_addresses_stock_above_65535() {
        // 65536 & 0xFFFF == 0, so a truncated offset would alias slot 0 (TIME).
        const HIGH_OFF: u32 = 65536;
        // `curr` holds slots [0, HIGH_OFF]; `next` sits one stride past it.
        let next_base = (HIGH_OFF + 1) * SLOT_SIZE;

        // probe() -> f64: L_RK_S := next[HIGH_OFF] - curr[HIGH_OFF]; return it.
        // Locals mirror the run fn so the f64 local L_RK_S (index 4) is valid.
        let mut probe = Function::new([(3, ValType::I32), (2, ValType::F64)]);
        emit_compute_stage_delta(&mut probe, next_base, HIGH_OFF);
        probe.instruction(&I::LocalGet(L_RK_S));
        probe.instruction(&I::End);

        let mut module = WasmModule::new();
        let mut types = TypeSection::new();
        types.ty().function([], [ValType::F64]);
        module.section(&types);
        let mut functions = FunctionSection::new();
        functions.function(0);
        module.section(&functions);
        let bytes_needed = next_base + (HIGH_OFF + 1) * SLOT_SIZE;
        let mut memories = MemorySection::new();
        memories.memory(MemoryType {
            minimum: u64::from(bytes_needed.div_ceil(65536) + 1),
            maximum: None,
            memory64: false,
            shared: false,
            page_size_log2: None,
        });
        module.section(&memories);
        let mut exports = ExportSection::new();
        exports.export("probe", ExportKind::Func, 0);
        exports.export("memory", ExportKind::Memory, 0);
        module.section(&exports);
        let mut code = CodeSection::new();
        code.function(&probe);
        module.section(&code);
        let wasm = module.finish();

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
        // Seed slot 0 (the alias target under truncation) and slot HIGH_OFF with
        // distinct values, so reading the wrong slot yields a distinguishable result.
        let curr_hi = (HIGH_OFF * SLOT_SIZE) as usize;
        let next0 = next_base as usize;
        let next_hi = (next_base + HIGH_OFF * SLOT_SIZE) as usize;
        store.mem_access_mut_slice(mem, |b| {
            b[0..8].copy_from_slice(&100.0f64.to_le_bytes()); // curr[0]
            b[next0..next0 + 8].copy_from_slice(&200.0f64.to_le_bytes()); // next[0]
            b[curr_hi..curr_hi + 8].copy_from_slice(&3.0f64.to_le_bytes()); // curr[HIGH_OFF]
            b[next_hi..next_hi + 8].copy_from_slice(&10.0f64.to_le_bytes()); // next[HIGH_OFF]
        });
        let probe_fn = store
            .instance_export(inst, "probe")
            .unwrap()
            .as_func()
            .unwrap();
        let delta: f64 = store
            .invoke_simple_typed::<(), f64>(probe_fn, ())
            .expect("probe");

        // next[HIGH_OFF] - curr[HIGH_OFF] = 10 - 3 = 7. A truncated u16 offset
        // would read slot 0 instead (200 - 100 = 100).
        assert_eq!(
            delta, 7.0,
            "RK stage delta read the wrong slot -- stock offset truncated above 65535?"
        );
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

    // ── Modules: EvalModule / LoadModuleInput (Phase 7 Task 1) ────────────
    //
    // Each unique `(model, input_set)` instance becomes its own initials/flows/
    // stocks wasm function taking `(module_off: i32, in_0..in_{k-1}: f64)`. An
    // `EvalModule` resolves the child instance and `call`s its function for the
    // current `StepPart`, passing `module_off + decl.off` and the popped inputs;
    // `LoadModuleInput` reads an input parameter. These tests assert wasm matches
    // the VM for submodel-bearing models, including the SMOOTH stdlib macro (which
    // expands to implicit module stocks) and the same instance at two offsets.

    /// A two-model datamodel: a `main` model that instantiates `submodel`
    /// `n_instances` times, wiring `in_value` (an aux in `main`) into each
    /// instance's `in` input. The submodel computes `out = body` (referencing its
    /// own `in`); `body_is_stock` makes `out` a stock integrating `body`, so the
    /// submodel carries internal stocks reached only through `EvalModule` (the
    /// nested-stock-offset case). `TestProject` only emits a single `main` model,
    /// so this is built as an explicit datamodel.
    fn submodel_project(
        name: &str,
        method: crate::datamodel::SimMethod,
        in_value: &str,
        body: &str,
        body_is_stock: bool,
        n_instances: usize,
    ) -> crate::datamodel::Project {
        use crate::datamodel;
        let mut main_vars: Vec<datamodel::Variable> =
            vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "in_value".to_string(),
                equation: datamodel::Equation::Scalar(in_value.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })];
        for i in 0..n_instances {
            let ident = format!("sub{i}");
            main_vars.push(datamodel::Variable::Module(datamodel::Module {
                // A module reference's `dst` is qualified with the instance name
                // (`subN.in`), not the bare input variable; an unqualified `dst`
                // silently fails to wire the input (the submodel's `in` keeps its
                // default), which would make `LoadModuleInput` untested.
                references: vec![datamodel::ModuleReference {
                    src: "in_value".to_string(),
                    dst: format!("{ident}.in"),
                }],
                ident,
                model_name: "submodel".to_string(),
                documentation: String::new(),
                units: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }));
        }

        let out_var = if body_is_stock {
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "out".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["grow".to_string()],
                outflows: vec![],
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })
        } else {
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "out".to_string(),
                equation: datamodel::Equation::Scalar(body.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })
        };
        let mut submodel_vars = vec![
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
            out_var,
        ];
        if body_is_stock {
            submodel_vars.push(datamodel::Variable::Flow(datamodel::Flow {
                ident: "grow".to_string(),
                equation: datamodel::Equation::Scalar(body.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }));
        }

        datamodel::Project {
            name: name.to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 5.0,
                dt: datamodel::Dt::Dt(1.0),
                save_step: None,
                sim_method: method,
                time_units: None,
            },
            dimensions: vec![],
            units: vec![],
            models: vec![
                datamodel::Model {
                    name: "main".to_string(),
                    sim_specs: None,
                    variables: main_vars,
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                datamodel::Model {
                    name: "submodel".to_string(),
                    sim_specs: None,
                    variables: submodel_vars,
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
            ],
            source: Default::default(),
            ai_information: None,
        }
    }

    /// A two-model datamodel like [`submodel_project`], but the submodel carries
    /// its OWN overridable constant `k` (a flows-phase `AssignConstCurr`) and
    /// `out = in + k`. Instantiating it `n_instances` times in `main` gives each
    /// instance a DISTINCT absolute offset for its own `k` (the recursive
    /// `base_off + module_decl.off` addressing), so a per-instance `set_value`
    /// override on one instance's `k` must not perturb the other. `in_value` is a
    /// constant wired into every instance's `in`, so the only differentiator
    /// between two instances' `out` is each instance's `k` override.
    fn submodel_with_constant_project(
        name: &str,
        in_value: &str,
        k_default: &str,
        n_instances: usize,
    ) -> crate::datamodel::Project {
        use crate::datamodel;
        let mut main_vars: Vec<datamodel::Variable> =
            vec![datamodel::Variable::Aux(datamodel::Aux {
                ident: "in_value".to_string(),
                equation: datamodel::Equation::Scalar(in_value.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            })];
        for i in 0..n_instances {
            let ident = format!("sub{i}");
            main_vars.push(datamodel::Variable::Module(datamodel::Module {
                references: vec![datamodel::ModuleReference {
                    src: "in_value".to_string(),
                    dst: format!("{ident}.in"),
                }],
                ident,
                model_name: "submodel".to_string(),
                documentation: String::new(),
                units: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: None,
            }));
        }

        let submodel_vars = vec![
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
            // `k` is a bare constant, so it lowers to a flows-phase
            // `AssignConstCurr` -- i.e. an overridable constant, distinct per
            // instance.
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "k".to_string(),
                equation: datamodel::Equation::Scalar(k_default.to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "out".to_string(),
                equation: datamodel::Equation::Scalar("in + k".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: datamodel::Compat::default(),
            }),
        ];

        datamodel::Project {
            name: name.to_string(),
            sim_specs: datamodel::SimSpecs {
                start: 0.0,
                stop: 3.0,
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
                    variables: main_vars,
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
                datamodel::Model {
                    name: "submodel".to_string(),
                    sim_specs: None,
                    variables: submodel_vars,
                    views: vec![],
                    loop_metadata: vec![],
                    groups: vec![],
                    macro_spec: None,
                },
            ],
            source: Default::default(),
            ai_information: None,
        }
    }

    /// Task 1: a model instantiating a submodel runs through wasm and matches the
    /// VM. The submodel's `out` depends on its `in` input (passed from `main`), so
    /// this exercises both `EvalModule` (the child `call`) and `LoadModuleInput`
    /// (the child reading its passed input). Previously this construct was rejected
    /// as `submodules are not supported`.
    #[test]
    fn compile_simulation_submodel_matches_vm() {
        let datamodel = submodel_project(
            "submod",
            crate::datamodel::SimMethod::Euler,
            "TIME + 1",
            "in * 2",
            false,
            1,
        );
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (submodel)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 2,
            "expected to compare main's in_value + the submodel's out, only checked {checked}"
        );
        // The submodel's output slot is in the single shared slab, addressed at
        // `module_off + off`; its layout entry confirms it was emitted.
        assert!(
            artifact
                .layout
                .var_offsets
                .iter()
                .any(|(n, _)| n.ends_with("out")),
            "the submodel's `out` should be in the layout"
        );
    }

    /// Task 1: `LoadModuleInput` reads the right input. The submodel's output is
    /// exactly its input, and `in_value` varies with TIME, so a wrong input-param
    /// index (or a missing pass-through) would diverge from the VM immediately.
    #[test]
    fn compile_simulation_submodel_loadmoduleinput_reads_right_input() {
        let datamodel = submodel_project(
            "passthru",
            crate::datamodel::SimMethod::Euler,
            "TIME * 3 + 1",
            "in", // out == in: a pure pass-through of the module input
            false,
            1,
        );
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (passthrough)");

        // out must equal in_value (= TIME*3+1) at every saved step.
        let results = run_artifact_results(&artifact);
        let n_slots = artifact.layout.n_slots;
        let find = |needle: &str| {
            artifact
                .layout
                .var_offsets
                .iter()
                .find(|(n, _)| n.ends_with(needle))
                .map(|(_, o)| *o)
                .unwrap_or_else(|| panic!("{needle} offset"))
        };
        let in_off = find("in_value");
        let out_off = find("out");
        for c in 0..artifact.layout.n_chunks {
            let in_v = results[c * n_slots + in_off];
            let out_v = results[c * n_slots + out_off];
            assert!(
                (in_v - out_v).abs() < 1e-9,
                "submodel out must equal its passed input at chunk {c}: in={in_v} out={out_v}"
            );
        }
        // And the whole model matches the VM.
        assert_matches_vm(sim, &artifact);
    }

    /// Task 1 (the `module_off` proof): the SAME `(model, input_set)` instance,
    /// instantiated twice in `main`, runs through wasm and matches the VM. Both
    /// instances share one `CompiledModule` (one function triple) but run at two
    /// different base offsets, so `module_off` must thread correctly into the
    /// child's slab reads/writes. Each `EvalModule` passes a distinct
    /// `module_off + decl.off`.
    #[test]
    fn compile_simulation_two_instances_same_module_matches_vm() {
        let datamodel = submodel_project(
            "twice",
            crate::datamodel::SimMethod::Euler,
            "TIME + 2",
            "in * 10",
            false,
            2,
        );
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (two instances)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 3,
            "expected to compare in_value + both instances' out, only checked {checked}"
        );
        // Both instances' outputs occupy distinct slots in the shared slab.
        let out_slots: Vec<usize> = artifact
            .layout
            .var_offsets
            .iter()
            .filter(|(n, _)| n.ends_with("out"))
            .map(|(_, o)| *o)
            .collect();
        assert_eq!(
            out_slots.len(),
            2,
            "two instances should contribute two distinct `out` slots, got {out_slots:?}"
        );
        assert_ne!(
            out_slots[0], out_slots[1],
            "the two instances must run at different module offsets"
        );
    }

    /// Task 1 (per-instance DISTINCT overrides -- the direct test of the
    /// absolute-slot const-region addressing): the SAME `CompiledModule`,
    /// instantiated twice in `main`, carries DISTINCT `set_value` overrides for
    /// its own constant `k`. Each instance's `k` lives at a distinct absolute
    /// offset (`base_off + module_decl.off`, the recursion in
    /// `collect_overridable_defaults`); the wasm override region is indexed by
    /// that absolute offset, so overriding instance 0's `k` to 100 and instance
    /// 1's `k` to 200 makes each instance's `out = in + k` reflect ITS OWN
    /// override. A bug that applied one override to both instances, or that
    /// ignored `module_off` (writing both overrides to the same slot), would make
    /// the two `out` series equal -- which the non-vacuity `assert_ne!` rejects.
    ///
    /// This is a wasm-only correctness property: the VM is NOT a valid cell-for-
    /// cell oracle for *distinct* overrides of a SHARED module, because its
    /// `set_value_by_offset` mutates the module's shared bytecode literal (one
    /// `literal_id` for both instances, resolved through the single shared
    /// `ModuleKey`), so the second override clobbers the first and both instances
    /// read the last value. The wasm backend is strictly more correct here. The
    /// VM divergence is tracked separately; this test still anchors against the
    /// VM in the regime where they DO agree -- both instances overridden to the
    /// SAME value (`compile_simulation_two_instances_same_value_override_matches_vm`).
    #[test]
    fn compile_simulation_two_instances_distinct_overrides() {
        // `in_value` is the constant 7 wired into both instances' `in`, so the
        // ONLY differentiator between the two instances' `out` is each instance's
        // `k` override (default 1).
        let datamodel = submodel_with_constant_project("distinct", "7", "1", 2);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (distinct overrides)");

        let (k0_off, k1_off) = instance_k_offsets(&artifact);
        assert_ne!(
            k0_off, k1_off,
            "the two instances' `k` must occupy distinct absolute offsets"
        );
        assert!(
            sim.is_constant_offset(k0_off) && sim.is_constant_offset(k1_off),
            "each instance's `k` must be a VM-overridable constant (sub0·k={k0_off}, sub1·k={k1_off})"
        );

        // Apply DIFFERENT overrides to the two instances, then reset + run.
        let wasm_slab = run_artifact_with_overrides(&artifact, &[(k0_off, 100.0), (k1_off, 200.0)]);
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;

        // Non-vacuity: each instance's `out` reflects ITS OWN override, and the
        // two genuinely DIFFER. `in_value` is 7, so sub0·out = 7 + 100 = 107 and
        // sub1·out = 7 + 200 = 207 at every saved step. If a bug applied one
        // override to both instances (or ignored `module_off` and wrote both to
        // one slot), the two `out` series would be equal and this would fail.
        let out0_off = layout_offset(&artifact, qualified_ident("sub0", "out").as_str());
        let out1_off = layout_offset(&artifact, qualified_ident("sub1", "out").as_str());
        for c in 0..n_chunks {
            let out0 = wasm_slab[c * n_slots + out0_off];
            let out1 = wasm_slab[c * n_slots + out1_off];
            assert!(
                (out0 - 107.0).abs() < 1e-9,
                "sub0·out should be in_value(7)+k0(100)=107 at chunk {c}, got {out0}"
            );
            assert!(
                (out1 - 207.0).abs() < 1e-9,
                "sub1·out should be in_value(7)+k1(200)=207 at chunk {c}, got {out1}"
            );
            assert_ne!(
                out0, out1,
                "the two instances' outputs must DIFFER under distinct per-instance overrides"
            );
        }
    }

    /// Task 1 (VM parity anchor for the shared-module override path): overriding
    /// BOTH instances' `k` to the SAME value matches the VM cell-for-cell. This is
    /// the regime where the VM and wasm agree -- the VM's shared-literal clobber
    /// (see `compile_simulation_two_instances_distinct_overrides`) is harmless
    /// when both overrides carry the same value -- so it proves the wasm override
    /// mechanism is faithful to the VM (not merely internally consistent) for a
    /// shared `CompiledModule` instantiated at two `module_off`s.
    #[test]
    fn compile_simulation_two_instances_same_value_override_matches_vm() {
        let datamodel = submodel_with_constant_project("same_val", "7", "1", 2);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let (k0_off, k1_off) = instance_k_offsets(&artifact);
        let wasm_slab = run_artifact_with_overrides(&artifact, &[(k0_off, 300.0), (k1_off, 300.0)]);
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;

        let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm creation");
        vm.set_value_by_offset(k0_off, 300.0)
            .expect("sub0·k must be a VM-overridable constant");
        vm.set_value_by_offset(k1_off, 300.0)
            .expect("sub1·k must be a VM-overridable constant");
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
                let wasm_val = wasm_slab[c * n_slots + wasm_off];
                assert!(
                    (vm_val - wasm_val).abs() < 1e-9,
                    "{name} mismatch at chunk {c} under same-value override: \
                     vm={vm_val} wasm={wasm_val}"
                );
            }
            checked += 1;
        }
        assert!(
            checked >= 3,
            "expected to compare in_value + both instances' k/out, only checked {checked}"
        );
        // Both instances reach 7 + 300 = 307 (the override took on both).
        let out0_off = layout_offset(&artifact, qualified_ident("sub0", "out").as_str());
        let out1_off = layout_offset(&artifact, qualified_ident("sub1", "out").as_str());
        assert!(
            (wasm_slab[out0_off] - 307.0).abs() < 1e-9
                && (wasm_slab[out1_off] - 307.0).abs() < 1e-9,
            "both instances should reach 7+300=307 under the shared override"
        );
    }

    /// Task 1 (nested stocks under Euler): a submodel whose `out` is a stock
    /// integrating a flow that depends on its `in` input. The submodel's internal
    /// stock is reached only through `EvalModule`, and its offset must be picked
    /// up by the recursive stock-offset collection so the Euler advance copies it
    /// `next -> curr`. The wasm must match the VM.
    #[test]
    fn compile_simulation_submodel_nested_stock_euler_matches_vm() {
        let datamodel = submodel_project(
            "nested_stock",
            crate::datamodel::SimMethod::Euler,
            "2",
            "in", // grow = in (= 2); out integrates by 2 each step
            true,
            1,
        );
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (nested stock)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 2,
            "expected to compare in_value + nested out stock"
        );
        // Pin the nested stock's value so this can't pass vacuously with an
        // un-wired input (`in` defaulting to 0). `grow = in = 2` integrates the
        // nested `out` stock by 2 each of the 5 Euler steps -> 10.
        let results = run_artifact_results(&artifact);
        let n_slots = artifact.layout.n_slots;
        let out_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n.ends_with("out"))
            .map(|(_, o)| *o)
            .expect("nested out offset");
        let last = (artifact.layout.n_chunks - 1) * n_slots + out_off;
        assert!(
            (results[last] - 10.0).abs() < 1e-9,
            "nested out stock should integrate to 2*5 = 10, got {}",
            results[last]
        );
    }

    /// Task 1 (nested stocks under RK4): the same nested-stock submodel under RK4.
    /// The recursive stock-offset collection must feed the RK stage math (saved/
    /// accum scratch indexed by stock position) the submodel's internal stock, so
    /// the four-stage integration covers nested stocks. The wasm must match the VM.
    #[test]
    fn compile_simulation_submodel_nested_stock_rk4_matches_vm() {
        // A nonlinear flow so RK genuinely differs from Euler: grow = in - out/10,
        // a first-order approach to a steady state, evaluated at trial points.
        let datamodel = submodel_project(
            "nested_stock_rk4",
            crate::datamodel::SimMethod::RungeKutta4,
            "5",
            "in - out / 10",
            true,
            1,
        );
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (nested stock RK4)");
        let checked = assert_matches_vm(sim, &artifact);
        assert!(
            checked >= 2,
            "expected to compare in_value + nested out stock"
        );
    }

    /// Task 1 (stdlib macro -> implicit module stocks): `SMTH1(input, delay)`
    /// expands to a stdlib `smth1` submodule carrying an internal SMOOTH stock.
    /// The whole model must match the VM, proving the implicit-module path (the
    /// stdlib instance's own `ByteCodeContext`, its nested stock under the RK/Euler
    /// loop, and the `EvalModule`/`LoadModuleInput` wiring) reproduces the VM.
    /// `SMTH1` was the canonical still-`Skipped` construct before this task.
    ///
    /// A NaN-aware comparison: the stdlib `smth1` instance carries an internal
    /// `initial_value` helper slot that is NaN at the t=0 results snapshot in
    /// *both* the VM and wasm (it is not written into `curr` before the forced
    /// t=0 save), so a finite-difference compare would spuriously fail on a
    /// faithful NaN==NaN match. Every user-visible variable (`input`,
    /// `smoothed`) is finite and compared exactly.
    #[test]
    fn compile_simulation_smooth_macro_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("smooth")
            .with_sim_time(0.0, 8.0, 0.25)
            .aux("input", "TIME", None)
            .aux("smoothed", "SMTH1(input, 2)", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (SMTH1)");
        // Pin that `smoothed` is finite and nonzero at the last step, so the
        // NaN-aware comparison cannot pass vacuously (an all-NaN `smoothed` would
        // satisfy NaN==NaN). A 2-unit smoothing of `input = TIME` reaches a
        // meaningful positive value by t=8.
        let results = run_artifact_results(&artifact);
        let n_slots = artifact.layout.n_slots;
        let smoothed_off = artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == "smoothed")
            .map(|(_, o)| *o)
            .expect("smoothed offset");
        let last = (artifact.layout.n_chunks - 1) * n_slots + smoothed_off;
        assert!(
            results[last].is_finite() && results[last] > 0.0,
            "smoothed should be finite and positive by the last step, got {}",
            results[last]
        );
        let checked = assert_matches_vm_nan_aware(sim, &artifact);
        assert!(
            checked >= 2,
            "expected to compare input + smoothed, only checked {checked}"
        );
    }

    /// Task 1 (DELAY stdlib macro under RK4): `DELAY3` expands to a stdlib
    /// submodule with three chained internal SMOOTH stocks, exercising a deeper
    /// nested-stock chain under the RK4 stage math. The wasm must match the VM.
    /// NaN-aware for the same internal-`initial_value` reason as the SMTH1 test.
    #[test]
    fn compile_simulation_delay3_macro_rk4_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("delay3")
            .with_sim_time(0.0, 8.0, 0.25)
            .with_sim_method(crate::datamodel::SimMethod::RungeKutta4)
            .aux("input", "TIME", None)
            .aux("delayed", "DELAY3(input, 2)", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen (DELAY3 RK4)");
        let checked = assert_matches_vm_nan_aware(sim, &artifact);
        assert!(
            checked >= 2,
            "expected to compare input + delayed, only checked {checked}"
        );
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

    /// AC4.2: a by-name series read strides the results slab using only the
    /// layout's `n_slots`/`results_offset` + the variable's offset, copies exactly
    /// `n_chunks` values (never the whole `n_chunks * n_slots` slab), and equals
    /// the VM's `get_series` for that variable. This is the read pattern a host
    /// performs over the blob's results region (the FFI returns the same layout).
    #[test]
    fn by_name_series_read_strides_slab_and_matches_vm_get_series() {
        let file = std::fs::File::open(POPULATION_XMILE).expect("open population model");
        let mut reader = BufReader::new(file);
        let datamodel = open_xmile(&mut reader).expect("parse population xmile");
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;
        let results_offset = artifact.layout.results_offset;
        let pop_off = layout_offset(&artifact, "population");

        // Run the blob and read the whole results region once (the host would map
        // the module's memory; here we copy it out).
        let slab = run_artifact_results(&artifact);

        // Stride out ONLY `population`'s series: exactly `n_chunks` reads at
        // `results_offset/8 + c*n_slots + off` (the slab is f64-indexed here).
        let _ = results_offset; // documents the byte base; `slab` already starts at it
        let mut series = Vec::with_capacity(n_chunks);
        for c in 0..n_chunks {
            series.push(slab[c * n_slots + pop_off]);
        }
        assert_eq!(
            series.len(),
            n_chunks,
            "a by-name read copies exactly n_chunks values, not the whole slab"
        );
        assert!(
            n_slots > 1,
            "the model must have >1 slot so striding (not a full copy) is meaningful"
        );

        // It equals the VM's get_series for the same variable.
        let mut vm = Vm::new(sim).expect("vm");
        vm.run_to_end().expect("vm run");
        let pop = Ident::<Canonical>::from_str_unchecked("population");
        let vm_series = vm.get_series(&pop).expect("vm get_series(population)");
        assert_eq!(
            vm_series.len(),
            series.len(),
            "series length matches the VM"
        );
        for (c, (&w, &v)) in series.iter().zip(vm_series.iter()).enumerate() {
            assert!(
                (w - v).abs() < 1e-9,
                "population chunk {c}: striped wasm read {w} != vm get_series {v}"
            );
        }
    }

    // ── set_value / reset override mechanism (Phase 7 Task 2) ─────────────
    //
    // An exported `set_value(offset, val) -> i32` writes the override into the
    // constants region (0 ok / nonzero when `offset` is not overridable), an
    // exported `reset()` resets run state without clearing the region (overrides
    // persist across reset, like the VM), and the next `run` re-runs initials +
    // the loop sourcing the overridable `AssignConstCurr` from the region.
    // `clear_values()` restores compiled defaults. These mirror the VM's
    // `set_value_by_offset`/`reset`/`clear_values` (`vm.rs:976-1062`).

    /// Instantiate `artifact.wasm`, optionally apply a list of `(offset, value)`
    /// overrides via the exported `set_value`, call `reset` then `run`, and copy
    /// the step-major results slab out. Each `set_value` return code is checked to
    /// be 0 (the caller passes only overridable offsets). Returns the slab.
    fn run_artifact_with_overrides(
        artifact: &WasmArtifact,
        overrides: &[(usize, f64)],
    ) -> Vec<f64> {
        let info = validate(&artifact.wasm).expect("module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let set_value = store
            .instance_export(inst, "set_value")
            .expect("set_value export")
            .as_func()
            .expect("set_value is a function");
        for &(off, val) in overrides {
            let rc: i32 = store
                .invoke_simple_typed::<(i32, f64), i32>(set_value, (off as i32, val))
                .expect("set_value invoke");
            assert_eq!(
                rc, 0,
                "set_value({off}, {val}) should accept an overridable offset"
            );
        }
        let reset = store
            .instance_export(inst, "reset")
            .expect("reset export")
            .as_func()
            .expect("reset is a function");
        store
            .invoke_simple_typed::<(), ()>(reset, ())
            .expect("reset invoke");
        let run = store
            .instance_export(inst, "run")
            .expect("run export")
            .as_func()
            .expect("run is a function");
        store
            .invoke_simple_typed::<(), ()>(run, ())
            .expect("run invoke");
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

    /// Call the exported `set_value` once on a freshly-instantiated module and
    /// return its i32 return code, without running the simulation. Used to assert
    /// the validation behavior (nonzero on a non-overridable offset).
    fn set_value_rc(artifact: &WasmArtifact, off: i32, val: f64) -> i32 {
        let info = validate(&artifact.wasm).expect("module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let set_value = store
            .instance_export(inst, "set_value")
            .expect("set_value export")
            .as_func()
            .expect("set_value is a function");
        store
            .invoke_simple_typed::<(i32, f64), i32>(set_value, (off, val))
            .expect("set_value invoke")
    }

    /// The absolute slab offset of `name` in the artifact's layout.
    fn layout_offset(artifact: &WasmArtifact, name: &str) -> usize {
        artifact
            .layout
            .var_offsets
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, o)| *o)
            .unwrap_or_else(|| panic!("{name} offset"))
    }

    /// The canonical qualified ident for a sub-model `instance`'s sub-variable
    /// `var` (`Ident::join`, the U+00B7 module-hierarchy separator), e.g.
    /// `sub0·k`. Built the same way `calc_flattened_offsets_incremental` keys the
    /// layout, so it stays correct if the separator ever changes.
    fn qualified_ident(instance: &str, var: &str) -> Ident<Canonical> {
        Ident::<Canonical>::join(
            &Ident::<Canonical>::new(instance).as_canonical_str(),
            &Ident::<Canonical>::new(var).as_canonical_str(),
        )
    }

    /// The absolute slab offsets of the two `submodel_with_constant_project`
    /// instances' own constant `k` (`sub0·k`, `sub1·k`). These are distinct
    /// because `calc_flattened_offsets_incremental` advances the base offset per
    /// instance, mirroring the VM's `collect_constant_info` recursion.
    fn instance_k_offsets(artifact: &WasmArtifact) -> (usize, usize) {
        (
            layout_offset(artifact, qualified_ident("sub0", "k").as_str()),
            layout_offset(artifact, qualified_ident("sub1", "k").as_str()),
        )
    }

    /// A VM run of `sim` with an override applied at absolute `off` (the VM's
    /// `set_value_by_offset`), returning that variable's slab so wasm overrides
    /// can be compared cell-for-cell against the VM oracle.
    fn vm_results_with_override(
        sim: CompiledSimulation,
        off: usize,
        val: f64,
    ) -> (Vec<f64>, usize, usize) {
        let mut vm = Vm::new(sim).expect("vm creation");
        vm.set_value_by_offset(off, val)
            .expect("offset must be a VM-overridable constant");
        vm.run_to_end().expect("vm run");
        let results = vm.into_results();
        (results.data.to_vec(), results.step_size, results.step_count)
    }

    /// AC5.1: overriding a constant via `set_value`, then `reset`, then `run`,
    /// yields the same series the VM produces under the same override. A constant
    /// aux feeds a flow that integrates a stock, so the override propagates into
    /// every downstream value at every step -- a wrong source (or an override that
    /// did not take) would diverge from the VM immediately.
    #[test]
    fn compile_simulation_set_value_override_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("override")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let rate_off = layout_offset(&artifact, "inflow_rate");
        assert!(
            sim.is_constant_offset(rate_off),
            "inflow_rate must be a VM-overridable constant for this test to be meaningful"
        );

        // Override the constant inflow_rate to 5 (was 2), so level integrates by
        // 5/step: 0,5,10,...,25 -- visibly different from the default 0,2,...,10.
        let wasm_slab = run_artifact_with_overrides(&artifact, &[(rate_off, 5.0)]);
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;

        let sim_vm = compile_sim(&datamodel, "main");
        let (vm_data, vm_step_size, vm_step_count) =
            vm_results_with_override(sim_vm, rate_off, 5.0);
        assert_eq!(vm_step_count, n_chunks, "saved-chunk count differs from VM");

        let mut checked = 0usize;
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let wasm_off = *wasm_off;
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            // Index the VM slab with the VM's own offset for this variable. It
            // equals `wasm_off` (both backends derive offsets from
            // `calc_flattened_offsets_incremental`), so this also skips the
            // implicit globals the layout carries but the VM offsets map omits.
            let vm_off = match sim.get_offset(&ident) {
                Some(o) => o,
                None => continue,
            };
            for c in 0..n_chunks {
                let vm_val = vm_data[c * vm_step_size + vm_off];
                let wasm_val = wasm_slab[c * n_slots + wasm_off];
                assert!(
                    (vm_val - wasm_val).abs() < 1e-9,
                    "{name} mismatch at chunk {c} under override: vm={vm_val} wasm={wasm_val}"
                );
            }
            checked += 1;
        }
        assert!(
            checked >= 2,
            "expected to compare inflow_rate + level + inflow"
        );

        // Pin the override actually took: level reaches 5*5 = 25 (not the default
        // 10), so this cannot pass vacuously with an ignored override.
        let level_off = layout_offset(&artifact, "level");
        let last = (n_chunks - 1) * n_slots + level_off;
        assert!(
            (wasm_slab[last] - 25.0).abs() < 1e-9,
            "level under inflow_rate=5 should reach 25, got {}",
            wasm_slab[last]
        );
    }

    /// AC5.2: `reset` with no override reproduces the compiled-default series. A
    /// `set_value`-then-reset-then-run with an empty override list must match a
    /// plain VM run (the default literals), proving the constants region is
    /// initialized to the compiled defaults and `reset` leaves them intact.
    #[test]
    fn compile_simulation_reset_no_override_restores_defaults() {
        let datamodel = crate::test_common::TestProject::new("defaults")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let wasm_slab = run_artifact_with_overrides(&artifact, &[]);
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;

        // The default run: level integrates by 2/step -> reaches 10.
        let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let wasm_off = *wasm_off;
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(&vm_off) = vm_results.offsets.get(&ident) else {
                continue;
            };
            for c in 0..n_chunks {
                let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
                let wasm_val = wasm_slab[c * n_slots + wasm_off];
                assert!(
                    (vm_val - wasm_val).abs() < 1e-9,
                    "{name} default mismatch at chunk {c}: vm={vm_val} wasm={wasm_val}"
                );
            }
        }
        let level_off = layout_offset(&artifact, "level");
        let last = (n_chunks - 1) * n_slots + level_off;
        assert!(
            (wasm_slab[last] - 10.0).abs() < 1e-9,
            "default level should reach 10, got {}",
            wasm_slab[last]
        );
    }

    /// `set_value` on a non-constant offset returns the error code and does not
    /// write. A stock's offset (`level`) is not an overridable constant (its
    /// initial is a constant, but it is assigned via `AssignNext`, not an
    /// `AssignConstCurr` in flows), so `set_value` must reject it. After the
    /// rejected call the default run must be unchanged.
    #[test]
    fn compile_simulation_set_value_rejects_non_constant_offset() {
        let datamodel = crate::test_common::TestProject::new("reject")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");

        let level_off = layout_offset(&artifact, "level");
        assert!(
            !sim.is_constant_offset(level_off),
            "level (a stock) must not be a VM-overridable constant"
        );
        // A non-overridable offset returns nonzero.
        assert_ne!(
            set_value_rc(&artifact, level_off as i32, 999.0),
            0,
            "set_value on a stock offset must return a nonzero error code"
        );
        // An out-of-range offset (>= n_slots) also returns nonzero.
        assert_ne!(
            set_value_rc(&artifact, artifact.layout.n_slots as i32, 1.0),
            0,
            "set_value on an out-of-range offset must return a nonzero error code"
        );
        assert_ne!(
            set_value_rc(&artifact, -1, 1.0),
            0,
            "set_value on a negative offset must return a nonzero error code"
        );

        // The rejected write left the constants region untouched: a no-override
        // run still reproduces the defaults (level reaches 10, not 999-driven).
        let wasm_slab = run_artifact_with_overrides(&artifact, &[]);
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;
        let last = (n_chunks - 1) * n_slots + level_off;
        assert!(
            (wasm_slab[last] - 10.0).abs() < 1e-9,
            "a rejected set_value must not perturb the default run; level should still reach 10, got {}",
            wasm_slab[last]
        );
    }

    /// `clear_values` restores compiled defaults after an override, without
    /// re-instantiating. Override inflow_rate, run (diverges), then clear, reset,
    /// run again -- the second run must reproduce the defaults.
    #[test]
    fn compile_simulation_clear_values_restores_defaults() {
        let datamodel = crate::test_common::TestProject::new("clear")
            .with_sim_time(0.0, 5.0, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let rate_off = layout_offset(&artifact, "inflow_rate");
        let level_off = layout_offset(&artifact, "level");
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;

        let info = validate(&artifact.wasm).expect("module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let func = |store: &mut Store<()>, name: &str| {
            store
                .instance_export(inst, name)
                .unwrap()
                .as_func()
                .unwrap()
        };

        // Override -> run -> level reaches 25.
        let set_value = func(&mut store, "set_value");
        let rc: i32 = store
            .invoke_simple_typed::<(i32, f64), i32>(set_value, (rate_off as i32, 5.0))
            .expect("set_value");
        assert_eq!(rc, 0);
        let run = func(&mut store, "run");
        store.invoke_simple_typed::<(), ()>(run, ()).expect("run");

        // clear_values -> reset -> run -> level back to the default 10.
        let clear_values = func(&mut store, "clear_values");
        store
            .invoke_simple_typed::<(), ()>(clear_values, ())
            .expect("clear_values");
        let reset = func(&mut store, "reset");
        store
            .invoke_simple_typed::<(), ()>(reset, ())
            .expect("reset");
        let run = func(&mut store, "run");
        store.invoke_simple_typed::<(), ()>(run, ()).expect("run");

        let mem = store
            .instance_export(inst, "memory")
            .unwrap()
            .as_mem()
            .unwrap();
        let base = artifact.layout.results_offset;
        let last_addr = base + ((n_chunks - 1) * n_slots + level_off) * 8;
        let level_last = store.mem_access_mut_slice(mem, |bytes| {
            f64::from_le_bytes(bytes[last_addr..last_addr + 8].try_into().unwrap())
        });
        assert!(
            (level_last - 10.0).abs() < 1e-9,
            "after clear_values the default level should reach 10, got {level_last}"
        );
    }

    /// The wasm backend's overridable-constant set (`collect_overridable_defaults`,
    /// which mirrors the VM's `collect_constant_info` recursion to capture each
    /// default literal) must address EXACTLY the offsets the VM reports overridable
    /// via `CompiledSimulation::constant_offsets`. If the two diverged, a blob's
    /// `set_value` would accept/reject a different set than the VM's, or initialize
    /// the wrong slots -- so this pins them equal over a model with both a top-level
    /// constant and a nested-module (SMOOTH) constant.
    #[test]
    fn wasm_overridable_set_matches_vm_constant_offsets() {
        let datamodel = crate::test_common::TestProject::new("const_set")
            .with_sim_time(0.0, 4.0, 0.5)
            .aux("k", "3", None)
            .aux("input", "TIME + k", None)
            // SMTH1 expands to a nested stdlib module carrying its own constants
            // (the smoothing delay), so the overridable set spans nested modules.
            .aux("smoothed", "SMTH1(input, 2)", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");

        let mut wasm_set: Vec<usize> = collect_overridable_defaults(&sim.modules, &sim.root, 0)
            .into_iter()
            .map(|(off, _)| off)
            .collect();
        wasm_set.sort_unstable();
        wasm_set.dedup();

        let mut vm_set: Vec<usize> = sim.constant_offsets().collect();
        vm_set.sort_unstable();

        assert_eq!(
            wasm_set, vm_set,
            "the wasm overridable-constant offsets must match the VM's exactly"
        );
        assert!(
            !vm_set.is_empty(),
            "this model must have at least one overridable constant (k) for the check to be meaningful"
        );

        // Every overridable offset is in range (so it indexes the n_slots-wide
        // const region and the validity byte region safely).
        let n_slots = sim.n_slots();
        for &off in &vm_set {
            assert!(
                off < n_slots,
                "overridable offset {off} must be < n_slots {n_slots}"
            );
        }
    }

    /// AC5.1 with an override on a constant that feeds an *initial* equation: the
    /// VM re-applies the override across initials (it mutates the literal at all
    /// locations), so an overridable constant read during the initials phase must
    /// also source from the region. Here `seed` is a constant whose value is the
    /// stock's initial, so overriding `seed` must change the stock's starting
    /// value -- exercising the initials-phase redirect, not just flows.
    #[test]
    fn compile_simulation_set_value_override_in_initials_matches_vm() {
        let datamodel = crate::test_common::TestProject::new("override_init")
            .with_sim_time(0.0, 3.0, 1.0)
            .aux("seed", "5", None)
            .stock("level", "seed", &["hold"], &[], None)
            .flow("hold", "0", None)
            .build_datamodel();
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let seed_off = layout_offset(&artifact, "seed");
        assert!(
            sim.is_constant_offset(seed_off),
            "seed must be an overridable constant"
        );

        let wasm_slab = run_artifact_with_overrides(&artifact, &[(seed_off, 42.0)]);
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;

        let sim_vm = compile_sim(&datamodel, "main");
        let (vm_data, vm_step_size, vm_step_count) =
            vm_results_with_override(sim_vm, seed_off, 42.0);
        assert_eq!(vm_step_count, n_chunks);

        for (name, wasm_off) in &artifact.layout.var_offsets {
            let wasm_off = *wasm_off;
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            if sim.get_offset(&ident).is_none() {
                continue;
            }
            for c in 0..n_chunks {
                let vm_val = vm_data[c * vm_step_size + wasm_off];
                let wasm_val = wasm_slab[c * n_slots + wasm_off];
                assert!(
                    (vm_val - wasm_val).abs() < 1e-9,
                    "{name} mismatch at chunk {c} under initials override: vm={vm_val} wasm={wasm_val}"
                );
            }
        }
        // seed=42 makes level start (and stay, hold=0) at 42.
        let level_off = layout_offset(&artifact, "level");
        assert!(
            (wasm_slab[level_off] - 42.0).abs() < 1e-9,
            "level should initialize to the overridden seed=42, got {}",
            wasm_slab[level_off]
        );
    }

    // ── Resumable run ABI (run_initials/run_to) vs the VM oracle ──────────
    //
    // The blob's persistent step cursor lives in mutable globals
    // (`G_SAVED`/`G_STEP_ACCUM`/`G_DID_INITIALS`), so a run can be advanced
    // incrementally: `run_initials()` once, then `run_to(t)` per target. The VM
    // (`Vm::run_initials`/`run_to`/`reset`/`set_value`) is the correctness oracle
    // for every behavior below; the comparator tolerance matches the
    // single-shot-`run` tests above (1e-9 cell-for-cell on the in-memory
    // fixtures, which run identically on both backends).

    /// A small stock + constant-flow fixture with `n_chunks` save points spanning
    /// `[0, stop]` at `dt`/`save_step` = 1. `level` integrates `inflow_rate` per
    /// step, so a wrong cursor or guard diverges immediately and visibly.
    fn resumable_fixture(stop: f64) -> crate::datamodel::Project {
        crate::test_common::TestProject::new("resumable")
            .with_sim_time(0.0, stop, 1.0)
            .aux("inflow_rate", "2", None)
            .stock("level", "0", &["inflow"], &[], None)
            .flow("inflow", "inflow_rate", None)
            .build_datamodel()
    }

    /// Drive the blob's resumable exports on a *fresh* instance: `run_initials`
    /// once, then `run_to(t)` for each `t` in `targets`, then copy the whole
    /// step-major slab out. The in-module peer of the integration-test helper
    /// `run_wasm_results_segmented`; kept here because the lib `#[cfg(test)]`
    /// module cannot reach the integration crate's private helpers.
    fn run_artifact_segmented(artifact: &WasmArtifact, targets: &[f64]) -> Vec<f64> {
        let info = validate(&artifact.wasm).expect("generated module must validate");
        let mut store = Store::new(());
        let inst = store
            .module_instantiate(&info, Vec::new(), None)
            .expect("instantiate")
            .module_addr;
        let run_initials = store
            .instance_export(inst, "run_initials")
            .expect("run_initials export")
            .as_func()
            .expect("run_initials is a function");
        store
            .invoke_simple_typed::<(), ()>(run_initials, ())
            .expect("run_initials wasm");
        for &t in targets {
            let run_to = store
                .instance_export(inst, "run_to")
                .expect("run_to export")
                .as_func()
                .expect("run_to is a function");
            store
                .invoke_simple_typed::<(f64,), ()>(run_to, (t,))
                .expect("run_to wasm");
        }
        read_slab(&mut store, inst, &artifact.layout)
    }

    /// Copy the whole step-major results slab (`n_chunks * n_slots` f64 at
    /// `layout.results_offset`) out of an already-driven instance's `memory`.
    fn read_slab(
        store: &mut Store<()>,
        inst: checked::Stored<wasm::addrs::ModuleAddr>,
        layout: &WasmLayout,
    ) -> Vec<f64> {
        let mem = store
            .instance_export(inst, "memory")
            .unwrap()
            .as_mem()
            .unwrap();
        let n = layout.n_chunks * layout.n_slots;
        let base = layout.results_offset;
        store.mem_access_mut_slice(mem, |bytes| {
            (0..n)
                .map(|i| {
                    let a = base + i * 8;
                    f64::from_le_bytes(bytes[a..a + 8].try_into().unwrap())
                })
                .collect()
        })
    }

    /// Task 1 (AC2.1, AC2.2 foundation): the re-expressed `run`, the resumable
    /// `run_initials`+`run_to(stop)`, and the VM must all agree on the full series.
    /// `run` is now `reset; run_to(stop)`, so this proves the delegation is
    /// faithful (the `run` export matches the segmented drive) and that the
    /// resumable path matches the VM (`Vm::run_to_end`) cell-for-cell.
    #[test]
    fn compile_simulation_run_to_matches_run_and_vm() {
        let datamodel = resumable_fixture(10.0);
        let sim = compile_sim(&datamodel, "main");
        let artifact = compile_simulation(&sim).expect("wasm codegen");
        let n_slots = artifact.layout.n_slots;
        let n_chunks = artifact.layout.n_chunks;
        let stop = sim.specs.stop;

        // (a) the single-shot `run` export.
        let via_run = run_artifact_results(&artifact);
        // (b) run_initials + run_to(stop).
        let via_run_to = run_artifact_segmented(&artifact, &[stop]);
        // (c) the VM oracle.
        let mut vm = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
        vm.run_to_end().expect("vm run");
        let vm_results = vm.into_results();
        assert_eq!(vm_results.step_count, n_chunks, "VM saved-chunk count");

        // The two wasm paths must be byte-identical (the run re-expression is a
        // pure delegation to run_to, so there is no numeric slack between them).
        assert_eq!(
            via_run, via_run_to,
            "run export diverged from run_initials+run_to(stop) -- the run re-expression is unfaithful"
        );

        // Both wasm paths equal the VM cell-for-cell over every layout variable.
        for (name, wasm_off) in &artifact.layout.var_offsets {
            let wasm_off = *wasm_off;
            let ident = Ident::<Canonical>::from_str_unchecked(name);
            let Some(&vm_off) = vm_results.offsets.get(&ident) else {
                continue;
            };
            for c in 0..n_chunks {
                let vm_val = vm_results.data[c * vm_results.step_size + vm_off];
                let run_val = via_run[c * n_slots + wasm_off];
                assert!(
                    (vm_val - run_val).abs() < 1e-9,
                    "{name} mismatch at chunk {c}: vm={vm_val} wasm={run_val}"
                );
            }
        }

        // AC2.2 foundation: after run_to(t), the saved row for time t holds the
        // VM's value at t. level integrates inflow_rate=2/step from 0, so at t its
        // saved value is 2*t. Drive a fresh instance to t=4 and read level's row 4.
        let level_off = layout_offset(&artifact, "level");
        let to_4 = run_artifact_segmented(&artifact, &[4.0]);
        let mut vm4 = Vm::new(compile_sim(&datamodel, "main")).expect("vm");
        vm4.run_to(4.0).expect("vm run_to(4)");
        let vm4_results = vm4.into_results();
        let vm4_level_off = vm4_results.offsets[&Ident::<Canonical>::from_str_unchecked("level")];
        let wasm_at_4 = to_4[4 * n_slots + level_off];
        let vm_at_4 = vm4_results.data[4 * vm4_results.step_size + vm4_level_off];
        assert!(
            (wasm_at_4 - vm_at_4).abs() < 1e-9 && (wasm_at_4 - 8.0).abs() < 1e-9,
            "level at t=4 after run_to(4): wasm={wasm_at_4} vm={vm_at_4} (expected 8)"
        );
    }
}
