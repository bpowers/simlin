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
//! nested modules (incl. SMOOTH/DELAY stdlib expansions), QUEUE models (whose
//! per-step FIFO side-table pass is lowered by [`super::passes`]), and CONVEYOR
//! models (whose per-step belt pass is lowered by [`super::belt`]). A genuine
//! runtime view range (`ViewRangeDynamic`), array unrolling past the per-function
//! budget, or a conveyor construct [`super::belt::reject_unsupported`] refuses
//! returns `WasmGenError::Unsupported`.

use wasm_encoder::Instruction as I;
use wasm_encoder::{
    BlockType, CodeSection, ConstExpr, DataSection, ExportKind, ExportSection, Function,
    FunctionSection, GlobalSection, GlobalType, MemorySection, MemoryType, Module as WasmModule,
    TypeSection, ValType,
};

use std::collections::HashMap;

use crate::bytecode::{ByteCode, CompiledModule, Opcode};
use crate::conveyor_compile::ConveyorPlan;
use crate::queue_compile::{CouplingTable, QueuePlan};
use crate::results::{Method, Specs};
use crate::vm::{CompiledSimulation, ModuleKey, StepPart};

use super::WasmGenError;
use super::belt::{self, ConveyorPass, ConveyorPassLayout, ConveyorPassLocals};
use super::errors;
use super::lower::{
    self, BuiltHelpers, HelperFn, build_helpers, f64_const, max_condition_depth, memarg,
};
use super::passes::{self, QueuePass, QueuePassLayout, QueuePassLocals};

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
// `errors::G_ERR_CODE`/`G_ERR_BELT` (indices 7/8) are the runtime error channel,
// emitted for EVERY module so `get_error` is an unconditional export (GH #921).
// `passes::G_HEAP` (index 9) is the special-stock bump pointer, emitted ONLY for a
// model that carries a side-table pass.

// `run_to`'s i32 locals. Its sole f64 *param* (the run target) occupies local 0,
// so the i32 working locals start at index 1 and `L_DST` is index 2 -- the same
// index the per-step emitters (`emit_save_advance`/`emit_rk*_step`) use, which
// lets those helpers stay shared between the (now removed) function-local cursor
// and the global cursor. Index 1 is an unused i32 filler that keeps `L_DST` at 2.
// The saved-row/step-accum cursor lives in `G_SAVED`/`G_STEP_ACCUM` (globals),
// not locals, so it survives across `run_to` calls.
const L_DST: u32 = 2;

// The special-stock passes' `run_to` locals, appended AFTER the RK f64 pair so
// `L_SAVED_TIME`/`L_RK_S` keep indices 3/4 and the shared RK emitters are
// untouched. Declared only when the model carries a side-table pass with state in
// the bump heap (`emit_run_to`'s locals list), which is what keeps a pass-free
// blob byte-identical.
//
// The queue's three and the belt's sixteen f64s (plus one i64) are declared TOGETHER
// whenever either pass is present, so both index blocks are fixed regardless of which
// passes a model carries. A queue-only blob therefore declares locals it never reads
// -- a few bytes of encoding and no runtime cost -- in exchange for the two emitters
// never needing to know about each other's presence.
const L_PASS_HEAP_SAVE: u32 = 5; // i32: `G_HEAP` across the mid-run preview
const L_PASS_RATE: u32 = 6; // f64: Σ max(inflow, 0)
const L_PASS_REDIRECTABLE: u32 = 7; // f64: the `<overflow/>` budget
const L_PASS_TMP: u32 = 8; // f64: clamp / served-volume scratch
const L_BELT_TRANSIT: u32 = 9;
const L_BELT_N_SLATS: u32 = 10;
const L_BELT_CONV_VOL: u32 = 11;
const L_BELT_CAPACITY: u32 = 12;
const L_BELT_REM_CAP: u32 = 13;
const L_BELT_REM_LIMIT: u32 = 14;
const L_BELT_ACC: u32 = 15;
const L_BELT_TMP: u32 = 16;
const L_BELT_CAP_ROOM: u32 = 17;
const L_BELT_BUDGET: u32 = 18;
const L_BELT_UNITS: u32 = 19;
const L_BELT_TMP2: u32 = 20;
const L_BELT_PRIOR: u32 = 21;
const L_BELT_REQ: u32 = 22;
const L_BELT_DESIRE: u32 = 23;
const L_BELT_TAKEN: u32 = 24;
/// The single i64 local, declared after the f64 run (the locals declaration is a list
/// of `(count, type)` runs, so the type change costs one more run, not a reshuffle).
const L_BELT_UNIT: u32 = 25;
/// f64 locals `L_BELT_TRANSIT..=L_BELT_TAKEN`, in one run.
const N_BELT_F64_LOCALS: u32 = L_BELT_TAKEN - L_PASS_RATE + 1;

/// The queue pass's f64 scratch locals inside `run_to`.
const PASS_LOCALS: QueuePassLocals = QueuePassLocals {
    rate: L_PASS_RATE,
    redirectable: L_PASS_REDIRECTABLE,
    tmp: L_PASS_TMP,
};

/// The belt pass's scratch locals inside `run_to`.
const BELT_LOCALS: ConveyorPassLocals = ConveyorPassLocals {
    transit: L_BELT_TRANSIT,
    n_slats: L_BELT_N_SLATS,
    conv_vol: L_BELT_CONV_VOL,
    capacity: L_BELT_CAPACITY,
    rem_cap: L_BELT_REM_CAP,
    rem_limit: L_BELT_REM_LIMIT,
    acc: L_BELT_ACC,
    tmp: L_BELT_TMP,
    cap_room: L_BELT_CAP_ROOM,
    budget: L_BELT_BUDGET,
    units: L_BELT_UNITS,
    tmp2: L_BELT_TMP2,
    prior: L_BELT_PRIOR,
    req: L_BELT_REQ,
    desire: L_BELT_DESIRE,
    taken: L_BELT_TAKEN,
    unit: L_BELT_UNIT,
};

/// `run_initials`' two f64 locals, which the belt init hook uses as its `transit`
/// and `n_slats` scratch. That function takes no params, so they are indices 0/1.
/// Declared only when the model carries belts.
const RI_BELT_TRANSIT: u32 = 0;
const RI_BELT_N_SLATS: u32 = 1;

/// Compile the named model of a datamodel `Project` to a full [`WasmArtifact`]
/// (the wasm blob plus its [`WasmLayout`]), through the salsa incremental
/// pipeline and [`compile_simulation`].
///
/// This is the entry point `libsimlin` uses across the FFI boundary
/// (`simlin_model_compile_to_wasm`): it works from a datamodel alone, with no
/// `Vm`/`SimlinSim`, returning both the blob and the name->offset layout. An
/// incremental-compile failure or an unsupported construct surfaces as
/// [`WasmGenError`] (the FFI maps it to a `SimlinError`, never a panic).
///
/// A CONVEYOR or QUEUE model routes through the shared special-stock dispatch
/// ([`crate::queue_compile::compile_sim`]) -- the same one the VM takes -- so the
/// expanded project and the resolved [`ConveyorPlan`]/[`QueuePlan`]s are
/// byte-identical to the VM's, and [`super::belt`]/[`super::passes`] lower the
/// belt and FIFO passes into the blob. There is no up-front special-stock reject
/// and no silent VM fallback: a construct the belt pass cannot lower correctly
/// (an inflow carrying a non-default `isee:spreadflow` -- `even`/`dest`/`dist`/
/// `source`, GH #946) is refused loudly by [`super::belt::reject_unsupported`],
/// deeper in the same call.
///
/// When `ltm_enabled` is true, the synthesized `$⁚ltm⁚*` link/loop score
/// variables are included in the emitted layout and blob. `ltm_discovery_mode`
/// flips the same flag `simlin_project_enable_ltm` sets on a `SimlinProject`,
/// but locally for this compile only. Neither flag reaches the special-stock
/// branch: that path compiles a separate, always-`ltm_enabled == false`
/// `SourceProject` (a documented degradation, `queues.md` §10).
pub fn compile_datamodel_to_artifact(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
    ltm_enabled: bool,
    ltm_discovery_mode: bool,
) -> Result<WasmArtifact, WasmGenError> {
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, datamodel, None);
    // The flags ride on the freshly-synced `SourceProject`; no reset dance is
    // needed (contrast `simlin_sim_new`, which mutates a *shared* persistent
    // `SourceProject` and must restore prior LTM state). `db` is owned by this
    // function and dropped at return, so flag changes can never leak.
    crate::db::set_project_ltm_enabled(&mut db, sync.project, ltm_enabled);
    crate::db::set_project_ltm_discovery_mode(&mut db, sync.project, ltm_discovery_mode);
    // The unified dispatch: an ordinary model goes to `compile_project_incremental`
    // (with the LTM flags above), a special-stock model to the expansion build
    // path. Going through it rather than reimplementing the dispatch is what
    // guarantees the wasm blob simulates the SAME expanded project the VM does.
    let build = crate::queue_compile::compile_sim(&mut db, sync.project, datamodel, model_name)
        .map_err(|e| {
            WasmGenError::Unsupported(format!("wasmgen: incremental compile failed: {e:?}"))
        })?;
    compile_simulation_with_plans(&build.compiled, &build.conveyor_plans, &build.queue_plans)
}

/// Compile the named model of a datamodel `Project` to a self-contained wasm
/// module, dropping the [`WasmLayout`] (callers that need the layout use
/// [`compile_datamodel_to_artifact`]). Currently called only from the inline
/// `compile_datamodel_to_wasm_validates` unit test; blob-only consumers that
/// once used this entry point now call [`compile_datamodel_to_artifact`]
/// directly.
pub fn compile_datamodel_to_wasm(
    datamodel: &crate::datamodel::Project,
    model_name: &str,
    ltm_enabled: bool,
    ltm_discovery_mode: bool,
) -> Result<Vec<u8>, WasmGenError> {
    Ok(compile_datamodel_to_artifact(datamodel, model_name, ltm_enabled, ltm_discovery_mode)?.wasm)
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
/// `run`, `set_value`, `reset`, `clear_values`, `run_to`, `run_initials`,
/// `get_error` -- each addition appending, so earlier indices stay stable. The
/// optional container-skipping initials is last of all. Used both at emit time
/// (`compile_with_passes`, to resolve the delegation targets) and at assembly time
/// (`assemble_simulation`), so the two never drift.
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
///
/// This is the ORDINARY-model entry point: it asserts (in debug) that `sim` has
/// not had its overridable-constant set retracted, i.e. that it did not come from
/// the special conveyor/queue build path. A conveyor or queue model must use
/// [`compile_simulation_with_plans`], which carries the plans the pass needs.
pub fn compile_simulation(sim: &CompiledSimulation) -> Result<WasmArtifact, WasmGenError> {
    compile_simulation_with_plans(sim, &[], &[])
}

/// [`compile_simulation`] with a synthetic error-raising pass spliced into the
/// same two hook points the conveyor belt pass occupies (`run_initials`'s
/// side-table init, and the between-Flows-and-Stocks step). Test-only: it is how
/// the runtime error channel's unwind, driver guards, and preview swallow are
/// exercised independently of that pass -- including on the ordinary and
/// queue-only models whose blobs must elide the guards entirely.
#[cfg(test)]
pub(super) fn compile_simulation_with_fault(
    sim: &CompiledSimulation,
    fault: errors::FaultInjection,
) -> Result<WasmArtifact, WasmGenError> {
    compile_with_passes(sim, &[], &[], Some(fault))
}

/// [`compile_simulation`] for a special-stock `CompiledSimulation`: additionally
/// lowers the per-step CONVEYOR and QUEUE side-table passes described by
/// `conveyor_plans` / `queue_plans` (the same plans `queue_compile::compile_sim`
/// hands the VM).
///
/// Both plan sets empty == the ordinary path, byte-identical to before this
/// existed: no bump-pointer global, no side-table helpers, no pass hooks. A
/// conveyor feature outside the lowered core is rejected loudly
/// ([`belt::reject_unsupported`]) rather than emitted as the simpler thing it is not.
pub fn compile_simulation_with_plans(
    sim: &CompiledSimulation,
    conveyor_plans: &[ConveyorPlan],
    queue_plans: &[QueuePlan],
) -> Result<WasmArtifact, WasmGenError> {
    compile_with_passes(sim, conveyor_plans, queue_plans, None)
}

/// Whether a module's passes can raise a runtime error, i.e. whether the drivers
/// need the error guards at all.
///
/// The belt pass is the only production pass that raises: `init_belts` rejects a
/// non-positive transit time and both it and the mid-run latch reject an
/// over-bound slat count (§4.1). The queue pass has no per-step runtime error.
/// Outside a test build `FaultInjection` is uninhabited, so a queue-only or
/// ordinary model still elides every guard, block, and save region below.
///
/// Takes `has_conveyor` rather than the plan slice so that `Passes::new` -- which
/// has already consumed the slice into a `ConveyorPass` -- can call the identical
/// predicate on `conveyor.is_some()`. The two are the same condition: a
/// `ConveyorPass` exists exactly when the plan set was non-empty.
fn pass_can_error(has_conveyor: bool, fault: &Option<errors::FaultInjection>) -> bool {
    has_conveyor || fault.is_some()
}

/// Whether `run_initials` must evaluate the Flows phase before running the
/// side-table init hook. See [`Passes::emit_init_body`] for the constraint.
///
/// True for a conveyor model and false for a queue-only one, matching the VM,
/// which runs the extra Flows evaluation only when `!conveyor_plans.is_empty()`
/// (`vm.rs:1515`). See [`pass_can_error`] on `has_conveyor`.
fn pass_needs_flows_before_init(
    has_conveyor: bool,
    fault: &Option<errors::FaultInjection>,
) -> bool {
    has_conveyor
        || fault
            .as_ref()
            .is_some_and(errors::FaultInjection::needs_flows_before_init)
}

/// The single whole-module assembly path. `fault` is the test-only
/// error-injection knob (uninhabited outside a test build); production callers
/// reach this through [`compile_simulation_with_plans`].
fn compile_with_passes(
    sim: &CompiledSimulation,
    conveyor_plans: &[ConveyorPlan],
    queue_plans: &[QueuePlan],
    fault: Option<errors::FaultInjection>,
) -> Result<WasmArtifact, WasmGenError> {
    belt::reject_unsupported(conveyor_plans)?;
    // Decides three things, all no-ops for a model whose passes are total: the
    // driver guards (`errors::emit_return_if_error`), the `curr` save area the
    // mid-run preview restores from, and the error clear in `reset`. Known before
    // the layout is computed because it is a property of the plan sets, not of the
    // emitted code.
    let pass_can_error = pass_can_error(!conveyor_plans.is_empty(), &fault);
    // `wasmgen` is in-crate, so it reads `CompiledSimulation`'s `pub(crate)`
    // fields directly rather than through accessors.
    let specs = &sim.specs;
    // Conveyors and queues are Euler-only, enforced at plan-build time
    // (`build_compiled`'s `effective_sim_specs` gate), so `emit_euler_step` is the
    // only place the pass can hook. Belt-and-braces: an RK method reaching here
    // with plans attached would silently drop the pass.
    debug_assert!(
        (queue_plans.is_empty() && conveyor_plans.is_empty())
            || matches!(specs.method, Method::Euler),
        "special stocks are Euler-only; a non-Euler special build should never reach wasmgen"
    );
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
    // A parallel `n_slots`-byte region marking which absolute slots have been
    // *explicitly* overridden via `set_value` (1 = overridden) -- distinct from
    // the `const_valid` "is overridable" region. `reset` reads it to reproduce the
    // VM's recreate-and-reapply semantics (`simulation.rs:314-330`): it zeroes the
    // live curr chunk, then reapplies only the explicitly-overridden constants. A
    // freshly-created VM leaves an unoverridden constant at 0 until initials run,
    // so reapplying the mere compiled *defaults* here would diverge -- hence the
    // override-set marker is required, not just the validity region. Zero-init (no
    // overrides at instantiation), so it needs no active data segment.
    let const_override_set_base = total_bytes;
    total_bytes = const_override_set_base
        .checked_add(n_slots)
        .ok_or_else(too_large)?;

    // The belt pass's six fixed-size regions: one 48-byte descriptor per belt, a
    // same-shaped save area for the mid-run preview, a cleared-volume scratch wide
    // enough for the busiest belt's equation-driven inflow list (only one belt's
    // phase B is live at a time), the concatenated §7.2 init-list tables (seeded by
    // an active data segment), the nine parallel per-leak-flow arrays (§5's leak
    // volumes, integer carry + its preview save, and the zone scratch), and the
    // discrete belt's clock + quantization carries (§6.3/§6.4) with their own preview
    // save. A belt-free model reserves nothing but the alignment, a leak-free one
    // reserves no leak arrays, and a continuous-only one no quantization carries.
    //
    // The leak region is `n * 60` bytes, so it can leave the bump 4-byte-aligned; the
    // state region opens with an `i64` and is re-aligned to 8.
    let belt_desc_base = align_up(total_bytes, u64::from(SLOT_SIZE))?;
    let belt_region_bytes = u32::try_from(conveyor_plans.len())
        .ok()
        .and_then(|n| n.checked_mul(belt::BELT_DESC_BYTES))
        .ok_or_else(too_large)?;
    let belt_desc_save_base = belt_desc_base
        .checked_add(belt_region_bytes)
        .ok_or_else(too_large)?;
    let belt_scratch_base = belt_desc_save_base
        .checked_add(belt_region_bytes)
        .ok_or_else(too_large)?;
    let belt_init_table_base = belt_scratch_base
        .checked_add(
            ConveyorPass::scratch_slots(conveyor_plans)
                .checked_mul(SLOT_SIZE)
                .ok_or_else(too_large)?,
        )
        .ok_or_else(too_large)?;
    let belt_leak_base = belt_init_table_base
        .checked_add(ConveyorPass::init_table_bytes(conveyor_plans))
        .ok_or_else(too_large)?;
    let belt_state_base = align_up(
        belt_leak_base
            .checked_add(ConveyorPass::leak_region_bytes(conveyor_plans))
            .ok_or_else(too_large)?,
        u64::from(SLOT_SIZE),
    )?;
    total_bytes = belt_state_base
        .checked_add(ConveyorPass::state_region_bytes(conveyor_plans))
        .ok_or_else(too_large)?;

    // The queue pass's two fixed-size regions: one 16-byte descriptor per FIFO,
    // and a same-shaped save area the mid-run preview stashes the live descriptors
    // in. Both are 8-byte-aligned so the ring addresses they carry stay naturally
    // aligned for `f64` access. A queue-free model reserves neither.
    let queue_desc_base = align_up(total_bytes, u64::from(SLOT_SIZE))?;
    let queue_region_bytes = u32::try_from(queue_plans.len())
        .ok()
        .and_then(|n| n.checked_mul(passes::DESC_BYTES))
        .ok_or_else(too_large)?;
    let queue_desc_save_base = queue_desc_base
        .checked_add(queue_region_bytes)
        .ok_or_else(too_large)?;
    total_bytes = queue_desc_save_base
        .checked_add(queue_region_bytes)
        .ok_or_else(too_large)?;

    // A whole-chunk save area for the mid-run preview, reserved only when a pass
    // can raise a runtime error. `vm.rs:1195` snapshots `curr` before the preview
    // pass so a preview-only failure (a `<sample>` re-latch over the slat bound)
    // restores the plain post-Flows chunk instead of leaving half-written pass
    // output -- driven flow rates and published container values the aborted pass
    // got partway through. A pass that cannot fail needs no snapshot, so a queue
    // model reserves nothing.
    let preview_curr_save_base = align_up(total_bytes, u64::from(SLOT_SIZE))?;
    if pass_can_error {
        total_bytes = preview_curr_save_base
            .checked_add(n_slots.checked_mul(SLOT_SIZE).ok_or_else(too_large)?)
            .ok_or_else(too_large)?;
    }

    // The bump region for the batch rings starts at the end of the static layout.
    // `reset` rewinds `G_HEAP` here; `q_alloc` grows linear memory past `pages`
    // whenever a ring pushes the bump beyond the last reserved page.
    let heap_base = align_up(total_bytes, u64::from(SLOT_SIZE))?;
    total_bytes = heap_base;

    let mut overridable_defaults = collect_overridable_defaults(&sim.modules, &sim.root, 0);
    // Pass-written slots (driven outflow rates, published container values) are NOT
    // overridable: their placeholder `0` equation compiles to an `AssignConstCurr`
    // the scan above sees, but the pass overwrites the slot every step, so an
    // accepted override would be silently ineffective (GH #871). The VM retracts
    // exactly this set in `queue_compile::build_compiled`; mirroring it here is
    // what makes the blob's `set_value` (which validates against the validity
    // region seeded from this list) reject the same offsets the VM does.
    if !queue_plans.is_empty() || !conveyor_plans.is_empty() {
        let pass_written: std::collections::HashSet<usize> = queue_plans
            .iter()
            .flat_map(|p| p.pass_written_offsets())
            .chain(conveyor_plans.iter().flat_map(|p| p.pass_written_offsets()))
            .collect();
        overridable_defaults.retain(|(off, _)| !pass_written.contains(off));
    }
    // Defense in depth: the offsets `collect_overridable_defaults` reports -- after
    // the retraction above -- must be exactly the set the VM considers overridable
    // (`constant_offsets`, the keys of `cached_constant_info`). Both walk the same
    // flows-`AssignConstCurr` rule and then subtract the same pass-written set, so
    // any divergence is a bug: a blob's `set_value` would accept/reject a different
    // set than the VM. Checked only in debug.
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
    //
    // The queue pass's FIFO primitives are appended to the SAME helper block (only
    // for a queue model), so every downstream index -- the per-instance triples and
    // the driver functions -- shifts automatically off `n_helpers`.
    let mut helpers = build_helpers();
    let helper_fns = helpers.fns;
    // One bump allocator serves both side tables, so it is pushed once, before
    // either pass's helpers, and its index handed to both.
    let heap_alloc_fn = (!queue_plans.is_empty() || !conveyor_plans.is_empty()).then(|| {
        let idx = helpers.functions.len() as u32;
        helpers.functions.push(HelperFn {
            params: vec![ValType::I32],
            results: vec![ValType::I32],
            body: passes::emit_alloc(),
        });
        idx
    });
    let conveyor_pass = (!conveyor_plans.is_empty()).then(|| {
        ConveyorPass::build(
            conveyor_plans,
            ConveyorPassLayout {
                desc_base: belt_desc_base,
                desc_save_base: belt_desc_save_base,
                scratch_base: belt_scratch_base,
                init_table_base: belt_init_table_base,
                leak_base: belt_leak_base,
                state_base: belt_state_base,
            },
            specs.dt,
            specs.start,
            heap_alloc_fn.expect("the bump allocator is emitted whenever belts exist"),
            &mut helpers.functions,
        )
    });
    let queue_pass = (!queue_plans.is_empty()).then(|| {
        QueuePass::build(
            queue_plans,
            QueuePassLayout {
                desc_base: queue_desc_base,
                desc_save_base: queue_desc_save_base,
            },
            specs.dt,
            heap_alloc_fn.expect("the bump allocator is emitted whenever queues exist"),
            &mut helpers.functions,
        )
    });
    let belt_init_data = conveyor_pass
        .as_ref()
        .map(ConveyorPass::init_table_data)
        .unwrap_or_default();
    let n_helpers = helpers.functions.len() as u32;
    // The coupling table the VM derives at plan-attach time, from the same two plan
    // sets. Deriving it here rather than re-deriving the coupling from the plans is
    // what keeps the two backends' admission priorities identical by construction.
    let coupling = (conveyor_pass.is_some() && queue_pass.is_some())
        .then(|| CouplingTable::build(conveyor_plans, queue_plans));
    let passes = Passes::new(conveyor_pass, queue_pass, coupling, fault, heap_base);

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
    //
    // The ROOT instance additionally gets a second, RECONCILE-SKIPPING initials
    // function -- the wasm analogue of `Vm::eval_initials_skipping` (`vm.rs:1731`),
    // which `run_initials` re-runs so that initials reading a side-table-derived
    // value see it rather than the compiled placeholder the first pass froze into
    // `initial_values`. Two kinds of slot qualify, exactly as `vm.rs:1616-1628`
    // collects them: every published container stock, belt or FIFO (so
    // `INIT(SUM(belt))` reads the start-of-run slat total), and a §7.2
    // list-initialized conveyor stock (so its belt-derived normalized total
    // survives, rather than being overwritten by the placeholder `<eqn>`).
    // `ConveyorPass::reconcile_skip_offsets` carries both conveyor halves.
    let reconcile_skip: std::collections::HashSet<usize> = passes
        .queue
        .as_ref()
        .filter(|p| p.has_containers())
        .map(|p| p.container_offsets().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(
            passes
                .conveyor
                .as_ref()
                .map(|p| p.reconcile_skip_offsets().collect::<Vec<_>>())
                .unwrap_or_default(),
        )
        .collect();
    let mut initials_skipping_fn: Option<Function> = None;

    let mut program_fns: Vec<Function> = Vec::with_capacity(instances.len() * 3);
    for (inst_idx, inst) in instances.iter().enumerate() {
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
        program_fns.push(emit_initials_fn(
            inst.module,
            inst.n_inputs,
            &make_ctx,
            None,
        )?);
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
        // Container stocks and conveyor stocks are variables of the MAIN model
        // (expansion appends them there), so only the root instance can hold one; a
        // child's initials are re-run in full through the root's `EvalModule`,
        // exactly as the VM's recursion does.
        if !reconcile_skip.is_empty() && instance_order[inst_idx] == sim.root {
            initials_skipping_fn = Some(emit_initials_fn(
                inst.module,
                inst.n_inputs,
                &make_ctx,
                Some(&reconcile_skip),
            )?);
        }
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
        preview_curr_save_base,
    };

    // Driver function indices, in the function-section order `assemble_simulation`
    // lays out after the per-instance triples: run, set_value, reset, clear_values,
    // run_to, run_initials, get_error, and (queue models with containers only) the
    // root's container-skipping initials. `run` and `run_to` delegate (`run` ->
    // `reset` + `run_to`; `run_to` -> `run_initials`), so their indices must be
    // known before their bodies are emitted -- the function section declares all
    // indices up front, so this is sound. Keeping run/set_value/reset/clear_values
    // at their original indices (everything since appends after) keeps the change
    // additive. `get_error` is unconditional, so it sits at a fixed index ahead of
    // the conditional container-skipping initials.
    let run_fn_index = run_fn_index_of(n_helpers, instances.len() as u32);
    let reset_fn_index = run_fn_index + 2;
    let run_to_fn_index = run_fn_index + 4;
    let run_initials_fn_index = run_fn_index + 5;
    let initials_skipping_fn_index = run_fn_index + 7;

    // The resumable run ABI: `run_initials` (idempotent), `run_to(target)` (the
    // single shared stepping loop), and `run` (re-expressed as `reset;
    // run_to(stop)`). The cursor lives in mutable globals so a run is resumable.
    let run_initials_fn = emit_run_initials(
        specs,
        regions,
        root_fn_base,
        &passes,
        initials_skipping_fn
            .as_ref()
            .map(|_| initials_skipping_fn_index),
    );
    let run_to_fn = emit_run_to(
        specs,
        regions,
        save_every,
        &stock_offsets,
        root_fn_base,
        run_initials_fn_index,
        &passes,
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
    // mirrors it into the live curr chunk, and marks the slot as overridden;
    // `reset` re-establishes the fresh pre-run curr chunk (zero, with overrides
    // reapplied) and clears the run cursor without clearing the override region;
    // `clear_values` restores the compiled defaults and drops the override marks.
    let set_value_fn = emit_set_value(
        n_slots,
        const_region_base,
        const_valid_base,
        const_override_set_base,
    );
    let reset_fn = emit_reset(n_slots, const_region_base, const_override_set_base, &passes);
    let clear_values_fn = emit_clear_values(
        const_region_base,
        const_override_set_base,
        &overridable_defaults,
    );

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
        get_error_fn: errors::emit_get_error(),
        initials_skipping_fn,
        instance_input_counts: &instance_input_counts,
        pages,
        n_slots,
        n_chunks,
        results_base,
        heap_base: passes.uses_heap().then_some(heap_base),
        gf_regions: &gf_images,
        const_init: &const_init,
        belt_init_data: &belt_init_data,
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

/// Round `v` up to the next multiple of `align` (a power of two), failing the
/// layout rather than wrapping.
fn align_up(v: u32, align: u64) -> Result<u32, WasmGenError> {
    let align = align as u32;
    v.checked_next_multiple_of(align)
        .ok_or_else(|| WasmGenError::Unsupported("wasmgen: model too large to lower".to_string()))
}

/// Build an instance's `initials` function: every `CompiledInitial`'s bytecode
/// in order, over the shared slab. The shared condition-local count is the max
/// nesting depth across all the initials (they run sequentially in one function);
/// the reverse-pop scratch covers the max `EvalModule { n_inputs }` over them.
/// `n_inputs` is the instance's module-input parameter count (shifts the locals).
///
/// `skip_offsets`, when present, drops every `CompiledInitial` all of whose write
/// offsets lie in the set -- reproducing `Vm::eval_initials_skipping`
/// (`vm.rs:1743-1751`) exactly, including its "non-empty AND all-in-set" rule. It
/// is used only for the root's queue-container reconciliation pass: re-running a
/// container stock's `0` placeholder would clobber the value the pass published
/// before the dependent initials read it. The locals declaration and condition
/// depth are still sized over ALL initials (a superset), so the two functions
/// share one frame shape.
fn emit_initials_fn<'a>(
    module: &CompiledModule,
    n_inputs: u32,
    make_ctx: &impl Fn(usize, u32, StepPart) -> lower::EmitCtx<'a>,
    skip_offsets: Option<&std::collections::HashSet<usize>>,
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
        if let Some(skip) = skip_offsets
            && !ci.offsets.is_empty()
            && ci.offsets.iter().all(|off| skip.contains(off))
        {
            continue;
        }
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
/// (`vm.rs:512-543`) exactly: a stock writes via `BinOpAssignNext` (codegen's
/// fused form of `stock + delta`, the only shape a stock update takes), and
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
            Opcode::BinOpAssignNext { off, .. } => {
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
    /// Slot-0 byte base of the whole-chunk `curr` save area the mid-run preview
    /// restores from when its pass raises. Reserved (and read) only when the
    /// module's passes can raise; otherwise it aliases the end of the static
    /// layout and is never touched.
    preview_curr_save_base: u32,
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

/// The special-stock side-table passes spliced into a module's drivers, plus the
/// test-only fault injector that exercises the runtime error channel from a
/// synthetic pass (GH #921/#922).
///
/// It exists so the drivers (`run_initials`, `run_to`, `reset`) name ONE thing
/// rather than juggling two `Option<&*Pass>`es and a fault knob, and so each pass
/// hook point -- the `run_initials` side-table init, the step-start container
/// publish, the between-Flows-and-Stocks step, and the mid-run preview -- has
/// exactly one emitter.
struct Passes<'a> {
    conveyor: Option<ConveyorPass<'a>>,
    queue: Option<QueuePass<'a>>,
    /// The queue-conveyor coupling (`queues.md` §9), derived from the same two plan
    /// sets the VM derives it from. `None` when either plan set is empty; `Some` with
    /// `any() == false` when both exist but nothing couples, which selects the plain
    /// belt-then-queue emission.
    coupling: Option<CouplingTable>,
    /// Whether any pass in this set can raise a runtime error, i.e. whether the
    /// drivers must wrap the pass body in a block and guard on the channel. Held
    /// rather than recomputed so the layout (which reserves the preview `curr`
    /// save area before the passes are built) and the emitters cannot disagree.
    ///
    /// A pass that raises while this is false cannot emit: it would need an
    /// [`errors::ErrorScope`], and only the block-opening drivers mint one.
    can_error: bool,
    /// Whether [`Self::emit_init_body`] reads slots the Flows phase alone writes,
    /// obliging `run_initials` to evaluate Flows before the init hook. See that
    /// method's docs for the constraint and `vm.rs:1508-1514` for the original.
    needs_flows_before_init: bool,
    /// Start of the shared bump region, `Some` iff a pass keeps state there. `reset`
    /// rewinds `G_HEAP` to it -- once, however many passes the module carries.
    heap_base: Option<u32>,
    /// Always `None` outside a test build, where `FaultInjection` is uninhabited
    /// (see `errors`); every arm below is then statically dead.
    fault: Option<errors::FaultInjection>,
}

impl<'a> Passes<'a> {
    /// Both flags are DERIVED from the plan sets and `fault` rather than passed in.
    /// The layout must know `pass_can_error` before the passes exist -- it reserves
    /// the preview `curr` save area -- so it calls the same free function. Taking
    /// the answer as a parameter here would let a caller hand over a flag that
    /// disagrees with the passes it also handed over, and a `debug_assert` comparing
    /// the parameter to the function that produced it pins nothing.
    fn new(
        conveyor: Option<ConveyorPass<'a>>,
        queue: Option<QueuePass<'a>>,
        coupling: Option<CouplingTable>,
        fault: Option<errors::FaultInjection>,
        heap_base: u32,
    ) -> Self {
        let has_conveyor = conveyor.is_some();
        let uses_heap = has_conveyor || queue.is_some();
        debug_assert!(
            coupling.is_none() || (has_conveyor && queue.is_some()),
            "a coupling table without both plan sets would index out of bounds"
        );
        Passes {
            can_error: pass_can_error(has_conveyor, &fault),
            needs_flows_before_init: pass_needs_flows_before_init(has_conveyor, &fault),
            heap_base: uses_heap.then_some(heap_base),
            conveyor,
            queue,
            coupling,
            fault,
        }
    }

    /// Does this module carry any side-table pass at all? A pass-free module emits
    /// no hook, no block, and no guard, so its drivers stay byte-identical.
    fn is_empty(&self) -> bool {
        self.conveyor.is_none() && self.queue.is_none() && self.fault.is_none()
    }

    /// Does any pass keep state in the shared bump region? Decides the `G_HEAP`
    /// global, the `reset` rewind, and `run_to`'s pass-locals frame.
    fn uses_heap(&self) -> bool {
        self.heap_base.is_some()
    }

    /// Step-start container publication (a hook point distinct from the pass
    /// proper; see [`emit_euler_step`]). It never raises: reading a built side table
    /// cannot fail. What matters is that it never runs over an UNBUILT one, which is
    /// what the drivers' error guards secure.
    ///
    /// **Ordering.** Conveyors publish before queues, matching the VM
    /// (`vm.rs:916` then `vm.rs:925` at step start; `vm.rs:1553` then `vm.rs:1574` at
    /// init). The two write disjoint slots -- a container variable is a hidden stock
    /// synthesized for exactly one owner -- so the order is not observable today; it
    /// is the VM's, rather than resting on that argument.
    fn emit_publish_containers(&self, f: &mut Function) {
        if let Some(c) = &self.conveyor {
            c.emit_publish_containers(f);
        }
        if let Some(q) = &self.queue {
            q.emit_publish_containers(f);
        }
        if let Some(fault) = &self.fault {
            fault.emit_publish(f);
        }
    }

    /// The `run_initials` side-table init body, emitted inside the pass block when
    /// [`Self::can_error`] -- so a raise abandons the remaining belts and returns
    /// from `run_initials` without arming the cursor. `scope` is the capability to
    /// do that unwinding, `None` exactly when no block was opened.
    ///
    /// **Two constraints.**
    ///
    /// 1. *Flows first.* A belt's parameters (transit, capacity, leak fractions) are
    ///    synthesized auxes nothing depends on, so they are absent from the initials
    ///    runlist and hold 0 until the Flows phase writes them. `init_belts` reads
    ///    them, so the driver must evaluate Flows before calling in here -- which it
    ///    does iff [`Self::needs_flows_before_init`]. A belt pass that forgot to set
    ///    the flag would read a transit of 0 and raise a spurious
    ///    `ConveyorTransitNotPositive`. The queue pass needs no such evaluation: its
    ///    only dynamic input is the stock's `<eqn>`, already in the initials runlist.
    /// 2. *Conveyors before queues*, matching `vm.rs:1542` / `vm.rs:1570` and
    ///    [`Self::emit_publish_containers`].
    fn emit_init_body(&self, f: &mut Function, scope: Option<errors::ErrorScope>) {
        if let Some(c) = &self.conveyor {
            c.emit_init(
                f,
                errors::expect_scope(scope),
                RI_BELT_TRANSIT,
                RI_BELT_N_SLATS,
            );
        }
        if let Some(q) = &self.queue {
            q.emit_init(f);
        }
        if let Some(fault) = &self.fault {
            fault.emit_initials(f, scope);
        }
    }

    /// The pass body proper, emitted at BOTH the real step site and the mid-run
    /// preview site. It must therefore be site-agnostic: a raise branches out of
    /// the enclosing pass block (via `scope`) rather than returning, leaving each
    /// driver free to react differently. See `errors::ErrorScope`.
    ///
    /// Without coupling the BELT pass runs before the FIFO pass, mirroring
    /// `queue_compile::run_coupled_passes`' uncoupled fast path (`cc::run_pass`
    /// then `run_queue_pass`). The order is observable whenever a belt's driven
    /// outflow is a queue's inflow.
    ///
    /// WITH coupling the two passes interleave; see [`Self::emit_coupled_step_body`].
    fn emit_step_body(
        &self,
        f: &mut Function,
        locals: QueuePassLocals,
        scope: Option<errors::ErrorScope>,
    ) {
        match (&self.conveyor, &self.queue) {
            (Some(c), Some(q)) if self.coupling.as_ref().is_some_and(CouplingTable::any) => {
                let coupling = self.coupling.as_ref().expect("checked by the guard");
                Self::emit_coupled_step_body(f, c, q, coupling, locals, scope);
            }
            (c, q) => {
                if let Some(c) = c {
                    c.emit_step_pass(f, BELT_LOCALS, errors::expect_scope(scope));
                }
                if let Some(q) = q {
                    q.emit_step_pass(f, locals, |_| false);
                }
            }
        }
        if let Some(fault) = &self.fault {
            fault.emit_step(f, scope);
        }
    }

    /// `queue_compile::run_coupled_passes`' interleaved body, unrolled (`queues.md`
    /// §9, `conveyors.md` §11).
    ///
    /// 1. every belt's phase A, so leaks and exits have freed belt room before any
    ///    queue sizes its budget against it;
    /// 2. for each belt, in plan order: for each queue coupled to it, in the belt's
    ///    `<inflow>` declaration order (the admission priority) --
    ///    size `req` from THIS belt's phase A minus `prior` (what the belt's
    ///    earlier-served queues already committed this DT), admit + serve the queue,
    ///    debit the belt's per-time-unit `in_carry` by `taken`, accumulate `prior`;
    /// 3. then that belt's phase B, which inserts every coupled `taken` (they are
    ///    `queue_coupled` inflows, so phase B reads them from `curr` unconditionally);
    /// 4. finally the uncoupled queues.
    ///
    /// The per-belt `prior` and the `req`/`desire`/`taken` triple are `run_to` locals,
    /// not static memory: nothing about them survives the step, and the preview's
    /// throwaway pass therefore needs no save/restore for them. What DOES survive is
    /// `in_carry`, which rides the belt descriptor's save.
    ///
    /// Two orderings inside step 2 are load-bearing. `admit_inflows` runs before the
    /// take, so material arriving this DT can cross the queue and board the belt in the
    /// same DT (the §4.3 pass-through). And `desire` is measured before the take, on the
    /// pre-take front, because the redirectable volume an `<overflow/>` sibling claims
    /// is exactly what the belt's budget refused (§4.5). Sizing `req` before the admit
    /// is NOT load-bearing -- the budget reads the belt's room, not the queue -- but it
    /// is emitted in the VM's order anyway rather than resting on that argument.
    ///
    /// `emit_consume_inflow_budget` is emitted after the queue's secondary outflows
    /// rather than before the shared-rate store where the VM calls it; the two touch
    /// disjoint state (`in_carry` vs. the ring and `curr`) and nothing in between reads
    /// `in_carry`.
    fn emit_coupled_step_body(
        f: &mut Function,
        c: &ConveyorPass<'a>,
        q: &QueuePass<'a>,
        coupling: &CouplingTable,
        locals: QueuePassLocals,
        scope: Option<errors::ErrorScope>,
    ) {
        c.emit_phase_a_all(f, BELT_LOCALS, errors::expect_scope(scope));
        for i in 0..c.n_plans() {
            let serves = coupling.serves_for_conveyor(i);
            if !serves.is_empty() {
                c.emit_reset_prior(f, BELT_LOCALS);
            }
            for cs in serves {
                c.emit_coupled_budget(f, i, BELT_LOCALS);
                q.emit_coupled_serve(
                    f,
                    cs,
                    locals,
                    BELT_LOCALS.req,
                    BELT_LOCALS.desire,
                    BELT_LOCALS.taken,
                );
                c.emit_consume_inflow_budget(f, i, BELT_LOCALS.taken);
                c.emit_add_prior(f, BELT_LOCALS);
            }
            c.emit_phase_b_at(f, i, BELT_LOCALS);
        }
        q.emit_step_pass(f, locals, |qi| coupling.queue_is_coupled(qi));
    }

    /// Rewind the bump pointer. Called from `reset`, which also clears
    /// `G_DID_INITIALS`, so the next `run_initials` re-allocates every ring from
    /// `heap_base` and re-seeds every descriptor. Nothing else is needed: a stale
    /// descriptor is unobservable (only `run_initials` reads one before writing it),
    /// and reclaiming the whole region wholesale is what makes repeated resets
    /// leak-free even though the doubling growers abandon the ring they outgrew.
    fn emit_heap_reset(&self, f: &mut Function) {
        if let Some(heap_base) = self.heap_base {
            f.instruction(&I::I32Const(heap_base as i32));
            f.instruction(&I::GlobalSet(passes::G_HEAP));
        }
    }

    /// The between-Flows-and-Stocks hook: the pass body, wrapped in its unwind
    /// block, followed by the guard that returns from `run_to` on a raise. The
    /// guard sits before the Stocks call, the `prev_values` snapshot, and the
    /// save/advance tail, so the failing step saves no row -- matching the VM,
    /// whose `run_coupled_passes` error returns before the same three.
    fn emit_step_hook(&self, f: &mut Function, locals: QueuePassLocals) {
        if self.is_empty() {
            return;
        }
        // Minting the scope IS opening the block, so a pass cannot raise here
        // unless the guard below is also emitted.
        let scope = self.can_error.then(|| errors::open_pass_block(f));
        self.emit_step_body(f, locals, scope);
        if let Some(scope) = scope {
            errors::close_pass_block(f, scope);
            errors::emit_return_if_error(f);
        }
    }

    /// The mid-run preview: run the pass on a throwaway side table so the resting
    /// `curr` holds what the resumed step will recompute, then undo everything.
    ///
    /// A raising pass is SWALLOWED here, exactly as `vm.rs:1187-1216` swallows it:
    /// `curr` is restored from `curr_save_base` and the channel is cleared, so this
    /// `run_to` -- which already completed everything the caller asked for --
    /// still returns cleanly. The same error surfaces loudly from the step itself
    /// if the run resumes without the inputs changing. The restore must precede the
    /// descriptor restores: it undoes the pass's writes to `curr` while the cloned
    /// rings are still installed, and those then drop them.
    ///
    /// `G_HEAP` is saved once, before either pass clones, and rewound once, after
    /// both have restored -- both passes bump-allocate their clones (and any growth
    /// the preview triggers) out of the same region.
    fn emit_preview(
        &self,
        f: &mut Function,
        locals: QueuePassLocals,
        heap_save_local: u32,
        regions: &RunRegions,
    ) {
        if self.is_empty() {
            return;
        }
        let (save_base, n_slots) = (regions.preview_curr_save_base, regions.n_slots);
        if self.uses_heap() {
            f.instruction(&I::GlobalGet(passes::G_HEAP));
            f.instruction(&I::LocalSet(heap_save_local));
        }
        if let Some(c) = &self.conveyor {
            c.emit_preview_save(f);
        }
        if let Some(q) = &self.queue {
            q.emit_preview_save(f);
        }
        let scope = self.can_error.then(|| {
            emit_copy_chunk(f, CURR_BASE, save_base, n_slots);
            errors::open_pass_block(f)
        });
        self.emit_step_body(f, locals, scope);
        if let Some(scope) = scope {
            errors::close_pass_block(f, scope);
            f.instruction(&I::GlobalGet(errors::G_ERR_CODE));
            f.instruction(&I::If(BlockType::Empty));
            emit_copy_chunk(f, save_base, CURR_BASE, n_slots);
            errors::emit_clear(f);
            f.instruction(&I::End);
        }
        if let Some(c) = &self.conveyor {
            c.emit_preview_restore(f);
        }
        if let Some(q) = &self.queue {
            q.emit_preview_restore(f);
        }
        if self.uses_heap() {
            f.instruction(&I::LocalGet(heap_save_local));
            f.instruction(&I::GlobalSet(passes::G_HEAP));
        }
    }
}

/// Emit `run_initials() -> ()`: seed the reserved time slots, run the root
/// initials, capture `initial_values`, build the queue side table, and arm the
/// step cursor -- but only the first time per `reset`. Idempotent via the
/// `G_DID_INITIALS` guard, mirroring `vm.rs:1080-1082`
/// (`if self.did_initials { return Ok(()); }`), so a `run_to` after another
/// `run_to` re-runs initials zero times, does NOT rebuild the side table, and
/// resumes the existing cursor instead. That single guard is the whole mechanism:
/// there is deliberately no second "belts built" flag to keep in sync.
///
/// The queue init sits AFTER the `initial_values` snapshot, exactly where the VM
/// places it (`vm.rs:1567`): a queue reads `curr[stock_off]` (its `<eqn>`, which
/// the initials runlist already evaluated) and never mutates `curr`, so nothing
/// forces it earlier -- and keeping the snapshot first is what makes `INIT()` of
/// an ordinary variable pure. `run_initials_skipping_idx` then supplies the
/// container reconciliation (`vm.rs:1581-1656`): the snapshot froze each container
/// stock's `0` placeholder, so once the pass has published the real start-of-run
/// batch values, the initials runlist is re-run (skipping the container stocks
/// themselves) and re-snapshotted, letting `INIT(SUM(queue))` see the true total.
///
/// A side-table init that raises a runtime error (GH #921) returns immediately --
/// before the container publish, the reconciliation, and the cursor arming, and
/// crucially without setting `G_DID_INITIALS`. That mirrors `vm.rs:1542-1548`,
/// where `init_belts`' `Err` returns ahead of `self.did_initials = true`.
fn emit_run_initials(
    specs: &Specs,
    regions: RunRegions,
    root_fn_base: u32,
    passes: &Passes,
    run_initials_skipping_idx: Option<u32>,
) -> Function {
    // The belt init hook needs two f64 scratch locals (`RI_BELT_*`); nothing else
    // in `run_initials` declares any, so a belt-free blob's frame is unchanged.
    let mut f = if passes.conveyor.is_some() {
        Function::new([(2, ValType::F64)])
    } else {
        Function::new([])
    };

    // if G_DID_INITIALS != 0: return  (idempotency -- already initialized).
    f.instruction(&I::GlobalGet(G_DID_INITIALS));
    f.instruction(&I::If(BlockType::Empty));
    f.instruction(&I::Return);
    f.instruction(&I::End);

    // A previously-raised runtime error is sticky until `reset` clears it: a fresh
    // `run_initials` must not re-run the initials over a slab whose side table is
    // half-built. (`run_to`'s own post-call guard catches the DID_INITIALS-short-
    // circuited case, where this guard never runs.)
    if passes.can_error {
        errors::emit_return_if_error(&mut f);
    }

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

    if !passes.is_empty() {
        // A pass whose init reads Flows-only slots (the conveyor belt parameters)
        // needs one Flows evaluation first. It goes AFTER the `initial_values`
        // snapshot above -- exactly where `vm.rs:1530-1541` puts it -- so `INIT(x)`
        // still sees the pure initials value of every ordinary variable.
        if passes.needs_flows_before_init {
            f.instruction(&I::I32Const(0));
            f.instruction(&I::Call(root_fn_base + F_FLOWS));
        }
        // The side-table init, in its unwind block (see `errors::ErrorScope`).
        let scope = passes.can_error.then(|| errors::open_pass_block(&mut f));
        passes.emit_init_body(&mut f, scope);
        if let Some(scope) = scope {
            errors::close_pass_block(&mut f, scope);
            errors::emit_return_if_error(&mut f);
        }
        // Publish the initialized belts' and FIFOs' container values, so the t=0 slot
        // holds the start-of-step value even before the first Euler step re-publishes
        // it -- and so the reconciliation below has something to reconcile against.
        //
        // The VM interleaves this with the init above (belt init, belt publish, queue
        // init, queue publish -- `vm.rs:1542-1578`) where the blob runs both inits and
        // then both publishes. Provably the same: `init_queues` takes `curr` by shared
        // reference, so nothing it does can depend on, or disturb, a belt's published
        // container slots.
        passes.emit_publish_containers(&mut f);
        if let Some(skipping_idx) = run_initials_skipping_idx {
            f.instruction(&I::I32Const(0));
            f.instruction(&I::Call(skipping_idx));
            emit_copy_chunk(
                &mut f,
                CURR_BASE,
                regions.initial_values_base,
                regions.n_slots,
            );
        }
    }

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
    passes: &Passes,
) -> Function {
    // One f64 param (`target`, local 0) + two i32 locals (index 1 filler, `L_DST`
    // at 2) + two f64 locals (`saved_time`, `s` at 3/4). The cursor lives in
    // globals, not locals; the i32 at index 1 is unused filler that keeps `L_DST`
    // at the index the per-step emitters expect.
    //
    // A special-stock model appends its pass locals AFTER that fixed prefix (a
    // second i32 group, then an f64 group, then the belt clock's lone i64 -- the
    // locals declaration is a list of `(count, type)` runs, not a per-type
    // partition), so `L_SAVED_TIME` and `L_RK_S` keep indices 3/4 and the shared RK
    // emitters need no change. The error channel needs no locals (it is two globals),
    // so a fault-injected model declares the plain prefix.
    let mut f = if passes.uses_heap() {
        Function::new([
            (2, ValType::I32),
            (2, ValType::F64),
            (1, ValType::I32),
            (N_BELT_F64_LOCALS, ValType::F64),
            (1, ValType::I64),
        ])
    } else {
        Function::new([(2, ValType::I32), (2, ValType::F64)])
    };

    // Absolute function indices of the ROOT instance's three program functions:
    // its function-triple base + the per-phase offset. The root is driven with
    // `module_off = 0`; nested instances are reached via `EvalModule` from there.
    let f_flows = root_fn_base + F_FLOWS;
    let f_stocks = root_fn_base + F_STOCKS;

    // Idempotent initials (seeds time slots, runs initials, arms the cursor on the
    // first call after a reset; a no-op otherwise).
    f.instruction(&I::Call(run_initials_idx));

    // One guard covers both ways the channel can be live here: a side-table init
    // that just raised, and a STICKY error from an earlier failed call (whose
    // `run_initials` short-circuited on `G_DID_INITIALS` or on its own guard).
    // Either way the run is over until `reset`.
    if passes.can_error {
        errors::emit_return_if_error(&mut f);
    }

    f.instruction(&I::Block(BlockType::Empty)); // $break
    f.instruction(&I::Loop(BlockType::Empty)); // $continue

    // if saved >= n_chunks: break. A resumed `run_to` on an already-complete slab
    // (`saved == n_chunks`, reachable via a second `run_to_end` or interactive
    // scrubbing that stays at the end) must be a no-op: the results region is
    // exactly `n_chunks` rows, so saving one more would write past it and corrupt
    // the snapshot/GF regions that sit immediately after. This is the resumable
    // analogue of the post-save exhaustion break below, moved to the loop *entry*
    // so re-entry on a full slab steps and saves nothing. (A fresh run never trips
    // it -- `saved` only reaches `n_chunks` via that post-save break, which exits
    // before this guard is re-checked.)
    f.instruction(&I::GlobalGet(G_SAVED));
    f.instruction(&I::I32Const(regions.n_chunks as i32));
    f.instruction(&I::I32GeS);
    f.instruction(&I::BrIf(1));

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
        Method::Euler => emit_euler_step(&mut f, f_flows, f_stocks, &regions, passes),
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

    // After a mid-interval stop, refresh `curr`'s flow/aux/constant slots at the
    // resting state -- but ONLY when `curr` was advanced (`saved < n_chunks`).
    //
    // The `curr[TIME] > target` break fires *after* the save+advance tail, which
    // copies only the stock offsets `next -> curr` and steps the time, leaving the
    // non-stock slots holding the previous step's values (a one-step lag versus the
    // advanced time + stocks). A mid-run `getValue` of a flow/aux would otherwise
    // read that lagged value, so one root `flows(0)` re-eval makes the live curr
    // chunk self-consistent at the resting time and identical to the VM's resting
    // curr (#625). At that advanced break `prev_values` holds the *last completed*
    // step (one before the resting time), so the re-eval's `PREVIOUS(x)` correctly
    // reads `x(t-dt)`.
    //
    // The guard skips the re-eval when the slab is full (`saved >= n_chunks`),
    // which is exactly the break paths that do NOT advance `curr`: the post-save
    // exhaustion break and the top-of-loop full-slab guard. There `curr` is already
    // the just-saved, fully-evaluated `t=stop` row, so the re-eval is unnecessary
    // for flows/auxes -- and actively WRONG for a `PREVIOUS` aux: `prev_values` was
    // snapshotted to that same `t=stop` row (the per-step snapshot runs after
    // flows), so a re-eval would resolve `PREVIOUS(x)` to `x(stop)` instead of
    // `x(stop-dt)`, corrupting the live curr a host reads via `getValue` and
    // diverging from the committed series + the VM. Skipping also keeps a resumed
    // `run_to` on a full slab a strict no-op. This mirrors the VM's
    // `curr_chunk != next_chunk` guard ("re-eval only when curr was advanced").
    // The re-eval touches only `curr` (the saved rows were already committed) and
    // does NOT snapshot `prev_values`, so a resume's `PREVIOUS` still sees the last
    // completed step. Unlike the VM there is no chunk aliasing: `curr` is always
    // the fixed `CURR_BASE` region.
    //
    // For a special-stock model the Flows re-eval alone would UNDO the
    // self-consistency it exists to provide: each pass-driven flow compiles to a
    // placeholder `AssignConstCurr 0` that the per-step pass overwrites, so
    // re-running Flows would stamp 0 into those slots, and the container stocks
    // would keep the value published at the START of the last completed step (one
    // step stale relative to the side table). Mirror the step prologue instead --
    // publish start-of-step container values from the real side table, re-eval
    // Flows, then run the pass as a side-effect-free PREVIEW on a cloned side table
    // (`vm.rs:1147-1216`). Re-running the pass on the real state would
    // double-advance the FIFOs when the run resumes.
    //
    // This tail is unreachable on a raising step: the step hook's guard returns
    // from `run_to` before the loop can even break, so a failed step never leaves a
    // preview behind. A preview that raises on its own (the next step's `<sample>`
    // re-latch trips the slat bound) is swallowed inside `emit_preview`.
    f.instruction(&I::GlobalGet(G_SAVED));
    f.instruction(&I::I32Const(regions.n_chunks as i32));
    f.instruction(&I::I32LtS);
    f.instruction(&I::If(BlockType::Empty));
    passes.emit_publish_containers(&mut f);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_flows));
    passes.emit_preview(&mut f, PASS_LOCALS, L_PASS_HEAP_SAVE, &regions);
    f.instruction(&I::End); // end if

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
///
/// This is the ONLY hook site for the special-stock passes -- conveyors and queues
/// are Euler-only, enforced at plan-build time (`conveyor_compile.rs:952-956`), so
/// RK2/RK4 can never reach a model with plans attached. Two DISTINCT hook points
/// live here, in the VM's order (`vm.rs:916-982`):
///
/// 1. **step-start**, before Flows: publish each container variable from the side
///    table as the previous step's admit/serve left it. A container variable is a
///    hidden no-flow stock, so the published value survives Flows and Stocks and is
///    what a Flows-phase `SUM(queue)` reader sees.
/// 2. **between Flows and Stocks**: advance the side table and write each driven
///    flow's rate, so the ordinary Stocks phase integrates every stock -- the queue
///    stock included -- from those rates.
///
/// Collapsing the two would let a `SUM(queue)` reader see this step's post-serve
/// batches instead of the start-of-step ones.
///
/// A pass that raises a runtime error returns from `run_to` at hook 2, so the
/// Stocks phase, the `prev_values` snapshot, and the save/advance tail below all
/// go unrun: the failing step contributes no results row and does not advance the
/// clock. That is exactly the VM, whose `run_coupled_passes` error returns before
/// the same three (`vm.rs:958-972`).
fn emit_euler_step(
    f: &mut Function,
    f_flows: u32,
    f_stocks: u32,
    regions: &RunRegions,
    passes: &Passes,
) {
    passes.emit_publish_containers(f);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_flows));
    passes.emit_step_hook(f, PASS_LOCALS);
    f.instruction(&I::I32Const(0));
    f.instruction(&I::Call(f_stocks));
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
fn emit_set_value(
    n_slots: u32,
    const_region_base: u32,
    const_valid_base: u32,
    const_override_set_base: u32,
) -> Function {
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

    // curr[offset] = val: mirror the override into the live curr chunk (base 0)
    // so a by-name read reflects it immediately, before any run -- exactly what
    // the VM's `apply_override` does via `set_value_now` (`vm.rs:1020`). The blob
    // owns this so the host needs no shadow write into curr.
    f.instruction(&I::LocalGet(SV_OFFSET));
    f.instruction(&I::I32Const(SLOT_SIZE as i32));
    f.instruction(&I::I32Mul);
    f.instruction(&I::LocalGet(SV_VALUE));
    f.instruction(&I::F64Store(memarg(u64::from(CURR_BASE))));

    // override_set[offset] = 1: mark this slot as explicitly overridden, so `reset`
    // reapplies it into curr (and `clear_values` can later drop the mark).
    f.instruction(&I::LocalGet(SV_OFFSET));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::I32Store8(byte_memarg(u64::from(
        const_override_set_base,
    ))));

    // return 0
    f.instruction(&I::I32Const(0));
    f.instruction(&I::End);
    f
}

/// Emit `reset() -> ()`: re-establish the fresh pre-run state so the next
/// `run_to` (and therefore `run`, which delegates `reset; run_to(stop)`) re-runs
/// initials and steps the loop from t=start, and so a by-name read between the
/// reset and the next run sees the same fresh state libsimlin presents.
///
/// Two parts, mirroring libsimlin's `simlin_sim_reset` recreate-and-reapply path
/// (`simulation.rs:314-330`):
///
/// 1. **Live curr chunk** (base 0): each slot becomes its explicit override (if
///    `override_set[slot] != 0`) or 0 otherwise -- the state a freshly-created VM
///    presents after reapplying its tracked overrides. A non-overridden constant
///    reads 0 here (its compiled default is not materialized until `run_initials`),
///    so this reapplies *overrides only*, never defaults; that is why the
///    override-set marker is needed and the validity region alone would not do.
///    The host therefore needs no shadow write into curr (the zero-fill it used to
///    do clobbered the very override it had mirrored). Unrolled per slot, matching
///    `emit_copy_chunk`; `run_initials` overwrites curr wholesale on the next run,
///    so this matters only for a read taken between `reset` and the next run.
///
/// 2. **Run cursor + PREVIOUS fallback** globals: `G_SAVED`/`G_STEP_ACCUM` to 0
///    (no rows saved, accumulator empty), `G_DID_INITIALS` to 0 (so `run_initials`
///    no longer short-circuits and re-seeds the time slots + re-runs initials), and
///    `G_USE_PREV_FALLBACK` back to 1 (the analogue of the VM's `reset` clearing
///    `prev_values_valid`). Mirrors `vm.rs:989-1002`.
///
/// 3. **Special-stock bump pointer** (queue models only): `G_HEAP` is rewound to
///    `heap_base`. Together with the `G_DID_INITIALS` clear above, that makes the
///    next `run_initials` re-allocate every batch ring from the top of the bump
///    region and re-seed every descriptor -- so repeated `reset`s (and therefore
///    repeated `run`s, which delegate `reset; run_to(stop)`) reclaim every ring the
///    prior run's capacity doubling abandoned, and cannot leak.
///
/// 4. **Runtime error channel** (error-capable passes only): `G_ERR_CODE` and
///    `G_ERR_BELT` are cleared. The channel is sticky -- `run_initials`/`run_to`
///    refuse to do anything while it is set (see `errors::emit_return_if_error`) --
///    so `reset` is a host's only way to recover from a raised error, and `run`
///    (which delegates `reset; run_to(stop)`) is therefore never blocked by a
///    stale one. A model whose passes are total can never set the channel, so it
///    emits no clear.
///
/// Like the VM, `reset` deliberately does NOT touch the constants-override region
/// or its markers, so a `set_value` override persists across `reset`.
fn emit_reset(
    n_slots: u32,
    const_region_base: u32,
    const_override_set_base: u32,
    passes: &Passes,
) -> Function {
    let mut f = Function::new([]);

    // Part 1: curr[slot] = override_set[slot] ? const_region[slot] : 0.0
    for slot in 0..n_slots {
        let slot_addr = u64::from(slot) * u64::from(SLOT_SIZE);
        f.instruction(&I::I32Const(0)); // F64Store address operand (curr base 0)
        // the overridden value ...
        f.instruction(&I::I32Const(0));
        f.instruction(&I::F64Load(memarg(
            u64::from(const_region_base) + slot_addr,
        )));
        // ... vs 0.0 ...
        f.instruction(&f64_const(0.0));
        // ... selected by the override-set marker byte.
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Load8U(byte_memarg(
            u64::from(const_override_set_base) + u64::from(slot),
        )));
        f.instruction(&I::Select);
        f.instruction(&I::F64Store(memarg(slot_addr)));
    }

    // Part 2: clear the persistent run state (cursor + PREVIOUS fallback).
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_SAVED));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_STEP_ACCUM));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(G_DID_INITIALS));
    f.instruction(&I::I32Const(1));
    f.instruction(&I::GlobalSet(G_USE_PREV_FALLBACK));

    // Part 3: rewind the special-stock bump pointer (one region, one rewind).
    passes.emit_heap_reset(&mut f);

    // Part 4: clear the runtime error channel, un-sticking the drivers.
    if passes.can_error {
        errors::emit_clear(&mut f);
    }
    f.instruction(&I::End);
    f
}

/// Emit `clear_values() -> ()`: restore each overridable constant to its
/// compiled-default literal by writing the defaults back into the constants
/// region (the VM's `clear_values`, `vm.rs:1055-1062`), and drop each slot's
/// override-set marker so a subsequent `reset` no longer reapplies the cleared
/// override into curr (it reverts to the fresh-zero state). Like the VM, this
/// does NOT touch the live curr chunk -- the next run re-materializes the
/// defaults. The defaults and offsets are compile-time constants, so this is a
/// straight-line sequence of stores -- one f64 default + one zero marker byte per
/// overridable absolute offset. The data segment also writes the defaults at
/// instantiation; `clear_values` lets a host undo a `set_value` without
/// re-instantiating the module.
fn emit_clear_values(
    const_region_base: u32,
    const_override_set_base: u32,
    overridable_defaults: &[(usize, f64)],
) -> Function {
    let mut f = Function::new([]);
    for &(abs_off, default) in overridable_defaults {
        // const_region[abs_off] = default
        f.instruction(&I::I32Const(0));
        f.instruction(&f64_const(default));
        f.instruction(&I::F64Store(memarg(
            u64::from(const_region_base) + abs_off as u64 * u64::from(SLOT_SIZE),
        )));
        // override_set[abs_off] = 0
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Const(0));
        f.instruction(&I::I32Store8(byte_memarg(
            u64::from(const_override_set_base) + abs_off as u64,
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
    /// `get_error() -> i64`: the runtime error channel's sole export (GH #921).
    /// Unconditional, so a host never feature-detects it.
    get_error_fn: Function,
    /// The root instance's reconcile-SKIPPING initials, `(module_off: i32) -> ()`.
    /// Present only when a pass publishes a value some initial reads (a queue's
    /// container stocks, a §7.2 list-initialized conveyor stock); `None` otherwise,
    /// in which case the module has no such function and no such index.
    initials_skipping_fn: Option<Function>,
    /// Module-input parameter count per instance, in the same order the triples
    /// appear in `program_fns`. Drives the per-triple wasm type
    /// (`(i32, f64*k) -> ()`).
    instance_input_counts: &'a [u32],
    pages: u32,
    n_slots: u32,
    n_chunks: u32,
    results_base: u32,
    /// The special-stock bump-pointer global's initial value (`passes::G_HEAP`),
    /// or `None` when the model carries no side-table pass -- in which case the
    /// global is not emitted at all and a pass-free blob stays byte-identical.
    heap_base: Option<u32>,
    /// Every GF-bearing instance's region image, for the active `DataSection`
    /// segments (each instance's directory + data sit at distinct bases).
    gf_regions: &'a [&'a GfRegions],
    /// The constants-override region init payloads (Phase 7 Task 2): sparse
    /// active `DataSection` segments seeding each overridable slot's f64 default
    /// and its validity byte.
    const_init: &'a ConstRegionInit,
    /// One `(byte address, little-endian f64 payload)` active `DataSection` segment
    /// per belt carrying a §7.2 explicit init list. Empty for every other model.
    belt_init_data: &'a [(u32, Vec<u8>)],
}

/// Assemble the simulation module: types, functions, memory, globals, exports,
/// code, and (when present) the GF data segments. Layout: the emitted helper
/// functions ([`build_helpers`], plus the queue pass's FIFO primitives when the
/// model has one) lead the function/code sections (indices `0..n_helpers`); then
/// one `[initials, flows, stocks]` triple per module instance (in
/// `instance_order`); then the seven drivers, and last -- for a queue model with
/// container variables -- the root's container-skipping initials. Exports
/// `memory`, `run`, and the three self-describing i32 geometry globals. Each
/// GF-bearing instance contributes two active `DataSection` segments (its
/// directory + data) at its own bases.
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
        get_error_fn,
        initials_skipping_fn,
        instance_input_counts,
        pages,
        n_slots,
        n_chunks,
        results_base,
        heap_base,
        gf_regions,
        const_init,
        belt_init_data,
    } = parts;

    let mut wasm = WasmModule::new();
    let n_helpers = helpers.functions.len() as u32;
    let n_instances = instance_input_counts.len() as u32;
    // Function layout: helpers, the per-instance triples, then the driver
    // functions in this fixed order: `run`, `set_value`, `reset`, `clear_values`,
    // `run_to`, `run_initials`, `get_error`. Each addition appends, so the earlier
    // exports keep stable indices (the growth is purely additive). The emit-time
    // index math in `compile_with_passes` uses the same `run_fn_index_of`.
    let run_fn_index = run_fn_index_of(n_helpers, n_instances);
    let set_value_fn_index = run_fn_index + 1;
    let reset_fn_index = run_fn_index + 2;
    let clear_values_fn_index = run_fn_index + 3;
    let run_to_fn_index = run_fn_index + 4;
    let run_initials_fn_index = run_fn_index + 5;
    let get_error_fn_index = run_fn_index + 6;

    // Type section: `run`'s `() -> ()` first, then one opcode-program type per
    // *distinct* module-input count (`(i32, f64*k) -> ()`, sorted), then the
    // helper types, then the `set_value` type (`(i32, f64) -> i32`), then
    // `run_to`'s `(f64) -> ()` type, then `get_error`'s `() -> i64`.
    // `reset`/`clear_values`/`run_initials` reuse `TYPE_RUN_FN` (`() -> ()`).
    // `opcode_type_for` maps an instance's `n_inputs` to its type index; a helper
    // at function index `i` uses the type appended after those.
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
    let get_error_type = run_to_type + 1;

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
    // `get_error() -> i64`.
    types.ty().function([], [ValType::I64]);
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
    functions.function(get_error_type); // get_error
    if initials_skipping_fn.is_some() {
        // The root's container-skipping initials shares the root's opcode-program
        // type `(module_off: i32) -> ()`; the root always takes 0 module inputs.
        functions.function(opcode_type_index[&0]);
    }
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
    // The runtime error channel (indices 7/8), emitted for EVERY module so
    // `get_error` is an unconditional export. Both init 0 -- "no error raised".
    debug_assert_eq!(
        globals.len(),
        errors::G_ERR_CODE,
        "G_ERR_CODE index drifted"
    );
    globals.global(mutable_i32_global(), &ConstExpr::i32_const(0)); // G_ERR_CODE
    debug_assert_eq!(
        globals.len(),
        errors::G_ERR_BELT,
        "G_ERR_BELT index drifted"
    );
    globals.global(mutable_i32_global(), &ConstExpr::i32_const(0)); // G_ERR_BELT
    // `passes::G_HEAP` (index 9), the special-stock bump pointer. Emitted only for
    // a model with a side-table pass, and initialized to the same `heap_base`
    // `reset` rewinds to -- so a blob works even if the host never calls `reset`.
    if let Some(heap_base) = heap_base {
        debug_assert_eq!(globals.len(), passes::G_HEAP, "G_HEAP index drifted");
        globals.global(
            mutable_i32_global(),
            &ConstExpr::i32_const(heap_base as i32),
        );
    }
    wasm.section(&globals);

    let mut exports = ExportSection::new();
    exports.export("run", ExportKind::Func, run_fn_index);
    exports.export("set_value", ExportKind::Func, set_value_fn_index);
    exports.export("reset", ExportKind::Func, reset_fn_index);
    exports.export("clear_values", ExportKind::Func, clear_values_fn_index);
    // The resumable run ABI (purely additive to the export set above).
    exports.export("run_to", ExportKind::Func, run_to_fn_index);
    exports.export("run_initials", ExportKind::Func, run_initials_fn_index);
    // The runtime error channel's sole export (GH #921): `(belt << 32) | code`,
    // zero when the run raised nothing. A host that never checks it is no worse off
    // than before it existed -- but a conveyor model's host must, since the blob
    // cannot return a `Result`. See `errors::decode_error_word` /
    // `errors::reconstruct_error`.
    exports.export("get_error", ExportKind::Func, get_error_fn_index);
    exports.export("memory", ExportKind::Memory, 0);
    exports.export("n_slots", ExportKind::Global, G_N_SLOTS);
    exports.export("n_chunks", ExportKind::Global, G_N_CHUNKS);
    exports.export("results_offset", ExportKind::Global, G_RESULTS_OFFSET);
    // The live saved-row counter (a mutable global): it is 0 before any run and
    // after `reset`, and equals `n_chunks` after a full run. A host reads it as
    // the number of completed steps (the VM's `results.step_count`), which the
    // static `n_chunks` capacity cannot express mid-run / pre-run. Additive.
    exports.export("saved_steps", ExportKind::Global, G_SAVED);
    wasm.section(&exports);

    // Code section order must match the function section: helper bodies, then the
    // per-instance program functions (in `program_fns` order), then the driver
    // functions in index order: `run`, `set_value`, `reset`, `clear_values`,
    // `run_to`, `run_initials`, `get_error`.
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
    code.function(&get_error_fn);
    if let Some(skipping) = &initials_skipping_fn {
        code.function(skipping);
    }
    wasm.section(&code);

    // The GF directory + data regions and the constants-override init values
    // are read-only-at-instantiation constants; active data segments write each
    // at its byte address when the module is instantiated. A module has at most
    // one data section, so the GF regions and the constants-override init share
    // it. The data section must follow the code section per the wasm binary order.
    let has_const_init =
        !const_init.value_segments.is_empty() || !const_init.valid_segments.is_empty();
    if !gf_regions.is_empty() || has_const_init || !belt_init_data.is_empty() {
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
        // The §7.2 conveyor init-list tables: one segment per list-initialized belt,
        // read once at belt init by `belt::emit_init_explicit`.
        for (addr, bytes) in belt_init_data {
            data.active(
                0,
                &ConstExpr::i32_const(*addr as i32),
                bytes.iter().copied(),
            );
        }
        wasm.section(&data);
    }

    wasm.finish()
}

#[cfg(test)]
#[path = "module_tests.rs"]
mod tests;
