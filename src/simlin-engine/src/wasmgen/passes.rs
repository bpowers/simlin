// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: each emitter appends a wasm instruction sequence for one
// piece of the special-stock side-table pass, mirroring the matching VM function
// step-for-step. No I/O; the tests (`passes_tests.rs`) execute the result under
// the DLR-FT interpreter and diff it against the bytecode VM.

//! Lowering of the special-stock (conveyor/queue) side-table passes to
//! WebAssembly (GH #884).
//!
//! The bytecode VM keeps a per-belt / per-FIFO **side table** outside the flat
//! `curr` data buffer and advances it once per Euler step, between the Flows and
//! Stocks phases (`vm.rs`, the `run_coupled_passes` call). This module
//! reproduces the QUEUE half of that machinery inside the emitted wasm blob, so
//! a queue model simulates with no host imports and no VM fallback. The CONVEYOR
//! half lives in the sibling [`super::belt`], which reuses this module's bump
//! allocator ([`emit_alloc`], [`G_HEAP`]) and its slab-addressing fragments;
//! [`reject_coupled_outflows`] is the guard that keeps a queue-conveyor coupling
//! -- served by the VM's *interleaved* pass, not by either module's independent
//! one -- from silently mis-simulating.
//!
//! ## What is specialized, and what is not
//!
//! A [`QueuePlan`] is fully resolved at compile time: every slot offset, every
//! outflow's `<overflow/>` flag, and the inflow/outflow/container counts are
//! constants. So the *pass* is emitted as straight-line, unrolled code -- one
//! `f64.store` per driven outflow, one clamp per inflow, one container
//! publication per container variable, each with its slab byte address folded
//! into the instruction's `memarg`. There is deliberately no runtime interpreter
//! walking a plan structure in linear memory.
//!
//! The one genuinely runtime-dynamic quantity is the **batch count**: a queue is
//! a `VecDeque<f64>` of strictly-positive batch volumes (`queue.rs:59-65`) whose
//! length is a function of the simulation's history. The FIFO primitives that
//! must loop over it (`q_total`, `q_take_from_front`, the reducers) are therefore
//! shared helper functions taking a *descriptor pointer* -- the same
//! runtime-loop-in-a-helper shape `super::vector`'s `stable_sort` and
//! `super::alloc`'s `allocate_available` already use. Their call sites stay
//! constant-folded (`i32.const <descriptor address>`).
//!
//! ## Memory
//!
//! Each queue owns a 16-byte **descriptor** in the static layout:
//!
//! ```text
//! +0  base_ptr: i32   byte address of the batch ring
//! +4  cap:      i32   ring capacity in f64 slots (always >= 1)
//! +8  head:     i32   ring index of the front (oldest) batch, always < cap
//! +12 len:      i32   live batch count
//! ```
//!
//! The rings live in a **bump region** past the end of the static layout,
//! addressed by the mutable global `G_HEAP`. The `q_alloc` helper bumps `G_HEAP`
//! and grows linear memory with `memory.grow` when the bump crosses the current
//! memory end (the memory is declared with `maximum: None`, so growth is always
//! legal; a `-1` failure return traps rather than silently corrupting the ring --
//! see [`emit_alloc`] for why an OOM stays a trap now that the runtime error
//! channel exists).
//!
//! On `push_back` into a full ring the capacity DOUBLES: a fresh, larger region
//! is bump-allocated, the live batches are copied into it in ring order, and the
//! old region is abandoned. The abandoned bytes leak until the next `reset`,
//! which rewinds `G_HEAP` to `heap_base` wholesale -- doubling bounds the total
//! waste below 2x the final size, and repeated resets therefore cannot leak.
//! This is why `reset` needs no per-queue bookkeeping: the next `run_initials`
//! rebuilds every descriptor from scratch (`G_DID_INITIALS` having been cleared),
//! and a RESUMED `run_to` skips `run_initials` entirely, so its side tables
//! survive.
//!
//! ## Rust-vs-wasm float semantics
//!
//! `f64::max`/`f64::min` in Rust return the *other* operand when one is NaN;
//! wasm's `f64.max`/`f64.min` propagate NaN. The queue pass clamps a possibly-NaN
//! inflow rate with `curr[off].max(0.0)` (`queue_compile.rs:711`), relying on
//! `NaN.max(0.0) == 0.0` to keep one poisoned inflow from poisoning the flat
//! stock. [`emit_clamp_nonneg`] therefore emits the `select`-based
//! `if x > 0.0 { x } else { 0.0 }`, never `f64.max`. The `MIN`/`MAX` container
//! reducers fold with the same strict-comparison shape for the same reason.

use wasm_encoder::{BlockType, Function, Instruction as Ins, ValType};

use crate::conveyor_compile::ContainerKind;
use crate::queue_compile::{QueueOutflowKind, QueuePlan};

use super::WasmGenError;
use super::lower::{HelperFn, f64_const, i32_memarg, memarg};

/// Index of the bump-pointer global. It follows the nine globals `module.rs`
/// already reserves (three immutable geometry globals, `use_prev_fallback`, the
/// three-word step cursor, and the two-word runtime error channel
/// `super::errors::{G_ERR_CODE, G_ERR_BELT}`), and is emitted ONLY for a model
/// that carries a special-stock pass -- so an ordinary model's blob pays nothing
/// for the bump allocator.
///
/// One bump pointer serves BOTH side tables: a model with belts and FIFOs carves
/// its slat rings and its batch rings out of the same region, and `reset` rewinds
/// it once (`module::Passes::emit_heap_reset`).
pub(super) const G_HEAP: u32 = 9;

/// Bytes per queue descriptor, and the byte offset of each of its four i32
/// fields. Sixteen bytes keeps every descriptor 8-byte-aligned, so the ring
/// addresses derived from `base_ptr` stay naturally aligned for `f64` access.
pub(super) const DESC_BYTES: u32 = 16;
const D_BASE: u64 = 0;
const D_CAP: u64 = 4;
const D_HEAD: u64 = 8;
const D_LEN: u64 = 12;

/// Bytes per ring slot (one `f64` batch volume). Also the bump allocator's
/// allocation unit, so [`super::belt`] sizes a slat ring in these units too.
pub(super) const SLOT_BYTES: i32 = 8;

/// Ring capacity a queue is born with. An uncoupled queue drains fully every
/// step (`queues.md` §4.3), so it holds at most one batch; four slots absorb the
/// initial batch plus a few steps of a blocked/outflow-less queue before the
/// first doubling.
const INITIAL_CAP: i32 = 4;

/// Every wasm page is 64 KiB; the bump allocator grows memory in whole pages.
const PAGE_BYTES: i32 = 65536;
const PAGE_SHIFT: i32 = 16;

// ── the helper-function registry ─────────────────────────────────────────────

/// Function indices of the emitted queue FIFO primitives. Each takes a
/// descriptor byte address as its first parameter, so ONE copy serves every
/// queue in the model.
///
/// The reducer helpers (`mean`/`min`/`max`/`stddev`/`batch`) are emitted only
/// when some container variable in the plan set actually needs them -- a
/// container-free queue model pays for none of them. `total` doubles as the
/// `SUM` container reducer and as `serve_unconstrained`'s summation, so it is
/// unconditional. `SIZE` needs no helper (it is a single `i32.load` +
/// `f64.convert_i32_s` at the call site).
///
/// `q_alloc` / `q_grow` / `q_push_back` are emitted too, but only ever `call`ed
/// from another helper, so their indices die with [`QueuePass::build`] rather
/// than riding on this registry.
#[derive(Clone, Copy)]
pub(super) struct QueueHelperFns {
    /// `q_init(d: i32, v: f64)` -- `QueueState::init_from_value` (`queue.rs:84`).
    init: u32,
    /// `q_admit(d: i32, rate: f64, dt: f64)` -- `QueueState::admit`
    /// (`queue.rs:105`).
    admit: u32,
    /// `q_total(d: i32) -> f64` -- `QueueState::total` (`queue.rs:303`), summed
    /// front-to-back so `serve_unconstrained` agrees with it exactly.
    total: u32,
    /// `q_serve_unconstrained(d: i32) -> f64` -- `QueueState::serve_unconstrained`
    /// (`queue.rs:170`): sum-then-clear, never `take_from_front(total())`.
    serve_unconstrained: u32,
    /// `q_take_from_front(d: i32, requested: f64) -> f64` --
    /// `QueueState::take_from_front` (`queue.rs:130`).
    take_from_front: u32,
    /// `q_clone_ring(d: i32)` -- repoint the descriptor at a verbatim copy of
    /// its ring, so the mid-run preview pass can run on a throwaway side table.
    clone_ring: u32,
    /// The container reducers (§8), each `(d: i32) -> f64`, present only when a
    /// container variable of that kind exists. `batch` is `(d: i32, j: i32) -> f64`.
    mean: Option<u32>,
    min: Option<u32>,
    max: Option<u32>,
    stddev: Option<u32>,
    batch: Option<u32>,
}

/// The three f64 scratch locals the step pass needs in its enclosing function
/// (`run_to`). Kept as a struct so `module.rs` owns the local numbering.
#[derive(Clone, Copy)]
pub(super) struct QueuePassLocals {
    /// Accumulates `Σ max(inflow, 0)` across a queue's inflows.
    pub rate: u32,
    /// The `<overflow/>` redirectable budget (`queues.md` §4.5).
    pub redirectable: u32,
    /// Single-use scratch for the clamp and for a served volume.
    pub tmp: u32,
}

/// Byte bases of the queue pass's two static regions. The bump region the rings
/// live in is shared with [`super::belt`] and owned by `module.rs`.
#[derive(Clone, Copy)]
pub(super) struct QueuePassLayout {
    /// First descriptor; queue `i`'s descriptor is at `desc_base + i*16`.
    pub desc_base: u32,
    /// Descriptor save area for the mid-run preview (same stride).
    pub desc_save_base: u32,
}

/// Everything the driver emitters (`run_initials` / `run_to` / `reset`) need to
/// splice the queue pass into a module.
pub(super) struct QueuePass<'a> {
    plans: &'a [QueuePlan],
    layout: QueuePassLayout,
    fns: QueueHelperFns,
    /// The simulation's fixed timestep. Queues are Euler-only
    /// (`conveyor_compile.rs:952-956` gates the whole special path), so `dt` is a
    /// compile-time constant and never read from `curr[DT]`.
    dt: f64,
}

/// Reject a queue plan whose primary outflow feeds a discrete conveyor
/// (`QueueOutflowKind::Coupled`, `queues.md` §9).
///
/// Unreachable today -- `compile_datamodel_to_artifact` rejects any model with a
/// conveyor marker, and a coupling requires a conveyor -- but a coupled outflow
/// is served by the interleaved `run_coupled_passes`, NOT by the uncoupled
/// admit-then-serve this module emits, so silently emitting the uncoupled form
/// would double-admit and mis-account. Loud, not silent, until the conveyor
/// phases land.
pub(super) fn reject_coupled_outflows(plans: &[QueuePlan]) -> Result<(), WasmGenError> {
    let coupled = plans
        .iter()
        .flat_map(|p| p.outflows.iter())
        .any(|o| matches!(o.kind, QueueOutflowKind::Coupled { .. }));
    if coupled {
        return Err(WasmGenError::Unsupported(
            "wasmgen: a queue coupled to a discrete conveyor is not yet supported by the \
             wasm backend; the bytecode VM is the only backend that simulates the \
             interleaved queue-conveyor pass today"
                .to_string(),
        ));
    }
    Ok(())
}

impl<'a> QueuePass<'a> {
    /// Emit the FIFO helper functions this plan set needs, appending them to
    /// `functions` (whose current length is their first index), and bundle them
    /// with the layout into a [`QueuePass`].
    ///
    /// `alloc` is the shared bump allocator's function index ([`emit_alloc`]),
    /// pushed by `module.rs` once for whichever side-table passes a model
    /// carries -- so a belt-and-FIFO model has one allocator, not two.
    ///
    /// Helpers are pushed in dependency order so each inter-helper `call`
    /// resolves against an already-assigned index, exactly as
    /// `lower::build_helpers` does.
    pub(super) fn build(
        plans: &'a [QueuePlan],
        layout: QueuePassLayout,
        dt: f64,
        alloc: u32,
        functions: &mut Vec<HelperFn>,
    ) -> QueuePass<'a> {
        let mut push = |params: Vec<ValType>, results: Vec<ValType>, body: Function| -> u32 {
            let idx = functions.len() as u32;
            functions.push(HelperFn {
                params,
                results,
                body,
            });
            idx
        };

        let grow = push(vec![ValType::I32], vec![], emit_grow(alloc));
        let push_back = push(
            vec![ValType::I32, ValType::F64],
            vec![],
            emit_push_back(grow),
        );
        let init = push(
            vec![ValType::I32, ValType::F64],
            vec![],
            emit_init(alloc, push_back),
        );
        let admit = push(
            vec![ValType::I32, ValType::F64, ValType::F64],
            vec![],
            emit_admit(push_back),
        );
        let total = push(vec![ValType::I32], vec![ValType::F64], emit_total());
        let serve_unconstrained = push(
            vec![ValType::I32],
            vec![ValType::F64],
            emit_serve_unconstrained(total),
        );
        let take_from_front = push(
            vec![ValType::I32, ValType::F64],
            vec![ValType::F64],
            emit_take_from_front(),
        );
        let clone_ring = push(vec![ValType::I32], vec![], emit_clone_ring(alloc));

        // The reducers are emitted only for the container kinds this model uses,
        // so a container-free queue model carries none of them.
        let uses = |want: fn(&ContainerKind) -> bool| {
            plans
                .iter()
                .flat_map(|p| p.containers.iter())
                .any(|c| want(&c.kind))
        };
        let mean = uses(|k| matches!(k, ContainerKind::Mean))
            .then(|| push(vec![ValType::I32], vec![ValType::F64], emit_mean(total)));
        let min = uses(|k| matches!(k, ContainerKind::Min))
            .then(|| push(vec![ValType::I32], vec![ValType::F64], emit_min_max(true)));
        let max = uses(|k| matches!(k, ContainerKind::Max))
            .then(|| push(vec![ValType::I32], vec![ValType::F64], emit_min_max(false)));
        let stddev = uses(|k| matches!(k, ContainerKind::Stddev))
            .then(|| push(vec![ValType::I32], vec![ValType::F64], emit_stddev(total)));
        // `Slat(0)` is a constant NaN at the call site (a 1-based index of 0 can
        // never name a batch), so it needs no helper.
        let batch = uses(|k| matches!(k, ContainerKind::Slat(j) if *j >= 1)).then(|| {
            push(
                vec![ValType::I32, ValType::I32],
                vec![ValType::F64],
                emit_batch(),
            )
        });

        QueuePass {
            plans,
            layout,
            fns: QueueHelperFns {
                init,
                admit,
                total,
                serve_unconstrained,
                take_from_front,
                clone_ring,
                mean,
                min,
                max,
                stddev,
                batch,
            },
            dt,
        }
    }

    /// Absolute byte address of queue `i`'s descriptor.
    fn desc_addr(&self, i: usize) -> i32 {
        (self.layout.desc_base + (i as u32) * DESC_BYTES) as i32
    }

    /// Absolute byte address of queue `i`'s preview descriptor save slot.
    fn save_addr(&self, i: usize) -> u64 {
        u64::from(self.layout.desc_save_base + (i as u32) * DESC_BYTES)
    }

    /// The absolute slab offsets whose *initials* fragment must be skipped when
    /// reconciling `INIT(<container access>)` against the published start-of-run
    /// container values (`vm.rs:1616-1628`). Every container variable is a hidden
    /// no-flow stock whose compiled `<eqn>` is a `0` placeholder; re-running that
    /// placeholder would clobber the value the pass just published, before the
    /// dependent initials read it.
    pub(super) fn container_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.plans
            .iter()
            .flat_map(|p| p.containers.iter())
            .map(|c| c.off)
    }

    /// Does any queue publish a container value? When none does, `run_initials`
    /// skips the whole reconciliation (re-run + re-snapshot) and the step loop
    /// skips the step-start publish -- both no-ops in that case.
    pub(super) fn has_containers(&self) -> bool {
        self.plans.iter().any(|p| !p.containers.is_empty())
    }

    // ── run_initials ────────────────────────────────────────────────────────

    /// Build the side table from the initials-populated `curr`
    /// (`queue_compile::init_queues`, `queue_compile.rs:618`): each queue is
    /// seeded from its stock's initial value `V` -- one front batch when `V > 0`,
    /// empty otherwise.
    ///
    /// The caller must emit this AFTER the `initial_values` snapshot, exactly
    /// where the VM places it (`vm.rs:1567`): the snapshot is what keeps `INIT()`
    /// of an ordinary variable pure, and a queue reads `curr` without mutating
    /// it, so nothing forces the init earlier.
    pub(super) fn emit_init(&self, f: &mut Function) {
        for (i, plan) in self.plans.iter().enumerate() {
            f.instruction(&Ins::I32Const(self.desc_addr(i)));
            emit_load_curr(f, plan.stock_off);
            f.instruction(&Ins::Call(self.fns.init));
        }
    }

    // ── the step-start container publish (a distinct hook point) ────────────

    /// Publish each queue's container-access results into their slab slots
    /// (`queue_compile::publish_queue_container_values`, `queue_compile.rs:795`).
    ///
    /// This runs at STEP-START -- before the Flows phase -- and NOT between Flows
    /// and Stocks where the pass proper runs. The two hook points are distinct on
    /// purpose (`vm.rs:925` vs `vm.rs:958`): a container variable is a hidden
    /// no-flow stock, so the value published here is what a Flows-phase reader of
    /// `SUM(queue)` sees, and it must reflect the batches as the PREVIOUS step's
    /// admit/serve left them.
    pub(super) fn emit_publish_containers(&self, f: &mut Function) {
        for (i, plan) in self.plans.iter().enumerate() {
            for c in &plan.containers {
                // `f64.store` consumes [addr_i32, value_f64]; every slab address
                // folds into the `memarg`, so the dynamic address is a constant 0.
                f.instruction(&Ins::I32Const(0));
                self.emit_container_value(f, i, &c.kind);
                f.instruction(&Ins::F64Store(memarg(slot_addr(c.off))));
            }
        }
    }

    /// Push one container value for queue `i`, reproducing
    /// `conveyor_compile::container_value_from_slice` (`conveyor_compile.rs:198`)
    /// over the queue's front-to-back batch vector.
    fn emit_container_value(&self, f: &mut Function, i: usize, kind: &ContainerKind) {
        let d = self.desc_addr(i);
        match kind {
            // A 1-based batch index of 0 can never name a batch: constant NaN,
            // with no helper call and no runtime test.
            ContainerKind::Slat(0) => {
                f.instruction(&f64_const(f64::NAN));
            }
            ContainerKind::Slat(j) => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::I32Const((*j - 1) as i32));
                f.instruction(&Ins::Call(self.fns.batch.expect("batch helper emitted")));
            }
            ContainerKind::Size => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
                f.instruction(&Ins::F64ConvertI32S);
            }
            ContainerKind::Sum => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.total));
            }
            ContainerKind::Mean => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.mean.expect("mean helper emitted")));
            }
            ContainerKind::Min => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.min.expect("min helper emitted")));
            }
            ContainerKind::Max => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.max.expect("max helper emitted")));
            }
            ContainerKind::Stddev => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.stddev.expect("stddev helper emitted")));
            }
        }
    }

    // ── the pass proper (between Flows and Stocks) ──────────────────────────

    /// The queue pass (`queue_compile::run_queue_pass`, `queue_compile.rs:754`),
    /// unrolled over the plan set: admit `Σ max(inflow, 0) · dt`, then serve the
    /// outflows in `<outflow>` declaration = priority order.
    ///
    /// Every queue here is UNCOUPLED ([`reject_coupled_outflows`] guarantees it),
    /// so the primary outflow is unconstrained and drains the whole FIFO
    /// (`queues.md` §4.3). The `redirectable` budget an `<overflow/>` sibling may
    /// claim is therefore 0 -- the primary was never blocked (§4.5). The budget
    /// is still tracked in a live local rather than constant-folded away, because
    /// the coupled primary of a later phase seeds it with `desire − taken` and
    /// the secondary loop is otherwise identical.
    pub(super) fn emit_step_pass(&self, f: &mut Function, locals: QueuePassLocals) {
        for (i, plan) in self.plans.iter().enumerate() {
            let d = self.desc_addr(i);

            // admit_inflows (`queue_compile.rs:703`): clamp each inflow slot IN
            // PLACE before summing, so the ordinary Stocks phase integrates the
            // same volume the FIFO admitted (the §4.1 conservation identity).
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::LocalSet(locals.rate));
            for &off in &plan.inflow_offs {
                emit_load_curr(f, off);
                emit_clamp_nonneg(f, locals.tmp);
                f.instruction(&Ins::LocalSet(locals.tmp));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::LocalGet(locals.tmp));
                f.instruction(&Ins::F64Store(memarg(slot_addr(off))));
                f.instruction(&Ins::LocalGet(locals.rate));
                f.instruction(&Ins::LocalGet(locals.tmp));
                f.instruction(&Ins::F64Add);
                f.instruction(&Ins::LocalSet(locals.rate));
            }
            f.instruction(&Ins::I32Const(d));
            f.instruction(&Ins::LocalGet(locals.rate));
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::Call(self.fns.admit));

            // A queue with no outflow only accumulates (`serve_uncoupled_queue`
            // returns after the admit), so the whole serve is skipped.
            let Some(primary) = plan.outflows.first() else {
                continue;
            };

            // The primary of an uncoupled queue empties the FIFO; its driven rate
            // is `removed / dt`.
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Const(d));
            f.instruction(&Ins::Call(self.fns.serve_unconstrained));
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Div);
            f.instruction(&Ins::F64Store(memarg(slot_addr(primary.flow_off))));

            // serve_secondary_outflows (`queue_compile.rs:649`). `redirectable`
            // starts at 0 for an uncoupled primary.
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::LocalSet(locals.redirectable));
            for outflow in plan.outflows.iter().skip(1) {
                if outflow.overflow {
                    // Overflow: claim only the still-redirectable blocked volume,
                    // splitting freely (§4.5, never batch-integrity-bound).
                    f.instruction(&Ins::I32Const(d));
                    f.instruction(&Ins::LocalGet(locals.redirectable));
                    f.instruction(&Ins::Call(self.fns.take_from_front));
                    f.instruction(&Ins::LocalSet(locals.tmp));
                    f.instruction(&Ins::LocalGet(locals.redirectable));
                    f.instruction(&Ins::LocalGet(locals.tmp));
                    f.instruction(&Ins::F64Sub);
                    f.instruction(&Ins::LocalSet(locals.redirectable));
                    f.instruction(&Ins::I32Const(0));
                    f.instruction(&Ins::LocalGet(locals.tmp));
                } else {
                    // An ordinary competing outflow drains the whole remaining
                    // front (§5.4), leaving nothing to redirect.
                    f.instruction(&f64_const(0.0));
                    f.instruction(&Ins::LocalSet(locals.redirectable));
                    f.instruction(&Ins::I32Const(0));
                    f.instruction(&Ins::I32Const(d));
                    f.instruction(&Ins::Call(self.fns.serve_unconstrained));
                }
                f.instruction(&f64_const(self.dt));
                f.instruction(&Ins::F64Div);
                f.instruction(&Ins::F64Store(memarg(slot_addr(outflow.flow_off))));
            }
        }
    }

    // ── the mid-run preview (run_to's resting re-eval) ──────────────────────

    /// Set up the side-effect-free mid-run PREVIEW, so the resting `curr` a host
    /// reads mid-run holds the pass-driven flow rates the resumed step will
    /// recompute -- without double-advancing the FIFOs (`vm.rs:1187-1216`).
    ///
    /// The VM clones its side tables; the blob does the same by (a) saving each
    /// descriptor into a static save area, (b) repointing `base_ptr` at a
    /// verbatim copy of the ring bump-allocated at `G_HEAP`, (c) running the
    /// pass, (d) restoring the descriptors, and (e) rewinding `G_HEAP`. Step (b)
    /// is what keeps the pass's in-place batch splitting off the real ring;
    /// step (e) reclaims both the clone and anything a `q_grow` inside the
    /// preview allocated, so repeated previews cannot leak.
    ///
    /// This emits (a) and (b); [`emit_preview_restore`] emits (d). The heap
    /// save/rewind (e) belongs to the CALLER (`module::Passes::emit_preview`),
    /// because one bump pointer serves both side tables and must be saved once,
    /// before either pass clones, and rewound once, after both have restored.
    /// The caller also emits (c) between them, because a pass that can raise a
    /// runtime error needs a `curr` snapshot/restore wrapped around the pass
    /// body -- and that restore must happen BEFORE the descriptors are put back,
    /// while the cloned rings are still installed. The queue pass itself never
    /// raises, so a queue-only model's preview carries neither the snapshot nor
    /// the guard.
    ///
    /// [`emit_preview_restore`]: Self::emit_preview_restore
    pub(super) fn emit_preview_save(&self, f: &mut Function) {
        for i in 0..self.plans.len() {
            let d = u64::from(self.layout.desc_base + (i as u32) * DESC_BYTES);
            let s = self.save_addr(i);
            for field in 0..4u64 {
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Load(i32_memarg(d + field * 4)));
                f.instruction(&Ins::I32Store(i32_memarg(s + field * 4)));
            }
            f.instruction(&Ins::I32Const(self.desc_addr(i)));
            f.instruction(&Ins::Call(self.fns.clone_ring));
        }
    }

    /// Tear down the mid-run preview: restore each saved descriptor, dropping the
    /// cloned ring. The caller rewinds `G_HEAP` afterwards, reclaiming the clone
    /// plus anything a `q_grow` inside the preview allocated. See
    /// [`emit_preview_save`].
    ///
    /// [`emit_preview_save`]: Self::emit_preview_save
    pub(super) fn emit_preview_restore(&self, f: &mut Function) {
        for i in 0..self.plans.len() {
            let d = u64::from(self.layout.desc_base + (i as u32) * DESC_BYTES);
            let s = self.save_addr(i);
            for field in 0..4u64 {
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Load(i32_memarg(s + field * 4)));
                f.instruction(&Ins::I32Store(i32_memarg(d + field * 4)));
            }
        }
    }
}

/// Byte address of slab slot `off` within the `curr` chunk (whose base is 0).
pub(super) fn slot_addr(off: usize) -> u64 {
    off as u64 * SLOT_BYTES as u64
}

/// Push `curr[off]`.
pub(super) fn emit_load_curr(f: &mut Function, off: usize) {
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::F64Load(memarg(slot_addr(off))));
}

/// Replace the f64 on the stack with `if x > 0.0 { x } else { 0.0 }`, using
/// `scratch` (a free f64 local) to hold `x` across its two uses.
///
/// This is Rust's `x.max(0.0)`, NOT wasm's `f64.max`: the two disagree on NaN
/// (Rust returns the non-NaN operand; wasm propagates). The queue pass depends on
/// the Rust behavior so a NaN inflow zeroes its slot rather than poisoning the
/// flat stock (`queue_compile.rs:696-698`); so does the belt pass's
/// `rate.max(0.0)` admission clamp (`conveyor.rs:935`). For every non-NaN `x` the
/// two agree, including `-0.0` (both yield a zero, and the two zeros compare
/// equal).
pub(super) fn emit_clamp_nonneg(f: &mut Function, scratch: u32) {
    f.instruction(&Ins::LocalTee(scratch));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalGet(scratch));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::Select);
}

// ── helper bodies ────────────────────────────────────────────────────────────
//
// Local numbering per helper is spelled out beside each emitter. Every helper's
// first parameter is the descriptor byte address `d`, so its four i32 fields are
// reached as `local.get d; i32.load offset=D_*`.

// `q_alloc(n: i32) -> i32`
const A_N: u32 = 0;
const A_P: u32 = 1;
const A_NEEDED: u32 = 2;
const A_CUR: u32 = 3;

/// `q_alloc(n_slots) -> ptr`: bump `n_slots * 8` bytes off `G_HEAP` and return
/// the OLD pointer, growing linear memory to cover the new bump.
///
/// # A failed `memory.grow` traps, and keeps trapping
///
/// `memory.grow` returns `-1` on failure. The runtime error channel
/// (`super::errors`, GH #921) now exists and could carry an OOM code, but this
/// deliberately still emits `unreachable`. Three reasons:
///
/// 1. **It is what the VM does.** An OOM is not a model diagnostic; it is
///    resource exhaustion. The VM's `VecDeque::push_back` on a failed allocation
///    calls Rust's OOM handler, which aborts the process (`panic = "abort"` in
///    libsimlin's release profile). A trap is the wasm-side abort. Reporting an
///    OOM gracefully would make the wasm backend strictly *more* forgiving than
///    its oracle -- a divergence, not a fix.
/// 2. **The unwind contract does not reach here.** `ErrorScope::raise` branches
///    to the enclosing pass block, and `br` cannot cross a call boundary.
///    `q_alloc` is called from `q_init`/`q_push_back`/`q_grow`/`q_clone_ring`,
///    themselves called from inside runtime loops. Propagating a failure would
///    mean threading a status return through every FIFO primitive and testing it
///    at every call site -- real code-size and complexity cost on the hot path,
///    for a condition that means the host's 4 GiB wasm address space is gone.
/// 3. **A trap is contained.** It unwinds the whole `run_to` and leaves the
///    instance's memory intact for a host to inspect, whereas continuing would
///    write batches past the memory bound -- which the runtime would trap on
///    anyway, just further from the cause.
///
/// The channel is reserved for *model* errors, i.e. the ones the VM turns into a
/// `Result` a caller can act on by editing the model.
///
/// `module.rs` pushes exactly one of these per blob, for whichever side-table
/// passes the model carries, and hands its index to both [`QueuePass::build`] and
/// `super::belt::ConveyorPass::build`.
pub(super) fn emit_alloc() -> Function {
    let mut f = Function::new([(3, ValType::I32)]);

    // p = G_HEAP; G_HEAP = p + n*8
    f.instruction(&Ins::GlobalGet(G_HEAP));
    f.instruction(&Ins::LocalTee(A_P));
    f.instruction(&Ins::LocalGet(A_N));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::GlobalSet(G_HEAP));

    // needed_pages = ceil(G_HEAP / 65536), computed with an unsigned shift so a
    // heap above 2^31 (unreachable in practice: the bump never exceeds the
    // wasm32 address space) still yields a sane page count rather than a
    // negative one.
    f.instruction(&Ins::GlobalGet(G_HEAP));
    f.instruction(&Ins::I32Const(PAGE_BYTES - 1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::I32Const(PAGE_SHIFT));
    f.instruction(&Ins::I32ShrU);
    f.instruction(&Ins::LocalSet(A_NEEDED));

    f.instruction(&Ins::MemorySize(0));
    f.instruction(&Ins::LocalSet(A_CUR));

    f.instruction(&Ins::LocalGet(A_NEEDED));
    f.instruction(&Ins::LocalGet(A_CUR));
    f.instruction(&Ins::I32GtS);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(A_NEEDED));
    f.instruction(&Ins::LocalGet(A_CUR));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::MemoryGrow(0));
    f.instruction(&Ins::I32Const(-1));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::Unreachable);
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);

    f.instruction(&Ins::LocalGet(A_P));
    f.instruction(&Ins::End);
    f
}

// `q_grow(d: i32)`
const G_D: u32 = 0;
const G_OLD_BASE: u32 = 1;
const G_OLD_CAP: u32 = 2;
const G_HEAD: u32 = 3;
const G_LEN: u32 = 4;
const G_NEW_BASE: u32 = 5;
const G_I: u32 = 6;

/// `q_grow(d)`: double the ring capacity, copying the `len` live batches into a
/// fresh allocation IN RING ORDER (so the copy normalizes `head` to 0).
///
/// The old region is abandoned rather than freed. Doubling bounds the total waste
/// below 2x the final ring size, and `reset` reclaims everything by rewinding the
/// bump pointer -- so a long run's peak is `O(final capacity)` and repeated runs
/// do not accumulate.
fn emit_grow(alloc: u32) -> Function {
    let mut f = Function::new([(6, ValType::I32)]);

    load_desc_i32(&mut f, G_D, D_BASE, G_OLD_BASE);
    load_desc_i32(&mut f, G_D, D_CAP, G_OLD_CAP);
    load_desc_i32(&mut f, G_D, D_HEAD, G_HEAD);
    load_desc_i32(&mut f, G_D, D_LEN, G_LEN);

    // new_base = q_alloc(old_cap * 2). `cap >= 1` always (init allocates
    // INITIAL_CAP and grow only ever doubles), so the new capacity is > len.
    f.instruction(&Ins::LocalGet(G_OLD_CAP));
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::Call(alloc));
    f.instruction(&Ins::LocalSet(G_NEW_BASE));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(G_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(G_I));
    f.instruction(&Ins::LocalGet(G_LEN));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    // new_base[i] = old_base[(head + i) % old_cap]
    f.instruction(&Ins::LocalGet(G_NEW_BASE));
    f.instruction(&Ins::LocalGet(G_I));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    push_ring_addr_from_locals(&mut f, G_OLD_BASE, G_OLD_CAP, G_HEAD, G_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(G_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(G_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(G_D));
    f.instruction(&Ins::LocalGet(G_NEW_BASE));
    f.instruction(&Ins::I32Store(i32_memarg(D_BASE)));
    f.instruction(&Ins::LocalGet(G_D));
    f.instruction(&Ins::LocalGet(G_OLD_CAP));
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Store(i32_memarg(D_CAP)));
    f.instruction(&Ins::LocalGet(G_D));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32Store(i32_memarg(D_HEAD)));

    f.instruction(&Ins::End);
    f
}

// `q_push_back(d: i32, v: f64)`
const PB_D: u32 = 0;
const PB_V: u32 = 1;
const PB_IDX: u32 = 2;

/// `q_push_back(d, v)`: append `v` at the back, doubling the ring first when it
/// is full. `VecDeque::push_back` (`queue.rs:107`).
fn emit_push_back(grow: u32) -> Function {
    let mut f = Function::new([(1, ValType::I32)]);

    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_CAP)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::Call(grow));
    f.instruction(&Ins::End);

    // idx = (head + len) % cap
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_HEAD)));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_CAP)));
    f.instruction(&Ins::I32RemU);
    f.instruction(&Ins::LocalSet(PB_IDX));

    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_BASE)));
    f.instruction(&Ins::LocalGet(PB_IDX));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(PB_V));
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::I32Store(i32_memarg(D_LEN)));

    f.instruction(&Ins::End);
    f
}

// `q_init(d: i32, v: f64)`
const IN_D: u32 = 0;
const IN_V: u32 = 1;

/// `q_init(d, v)`: `QueueState::init_from_value` (`queue.rs:84`). A strictly
/// positive `V` seeds one front batch; `V <= 0` (and NaN, which fails `> 0.0`)
/// starts the queue empty.
fn emit_init(alloc: u32, push_back: u32) -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::LocalGet(IN_D));
    f.instruction(&Ins::I32Const(INITIAL_CAP));
    f.instruction(&Ins::Call(alloc));
    f.instruction(&Ins::I32Store(i32_memarg(D_BASE)));

    f.instruction(&Ins::LocalGet(IN_D));
    f.instruction(&Ins::I32Const(INITIAL_CAP));
    f.instruction(&Ins::I32Store(i32_memarg(D_CAP)));
    f.instruction(&Ins::LocalGet(IN_D));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32Store(i32_memarg(D_HEAD)));
    f.instruction(&Ins::LocalGet(IN_D));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32Store(i32_memarg(D_LEN)));

    f.instruction(&Ins::LocalGet(IN_V));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IN_D));
    f.instruction(&Ins::LocalGet(IN_V));
    f.instruction(&Ins::Call(push_back));
    f.instruction(&Ins::End);

    f.instruction(&Ins::End);
    f
}

// `q_admit(d: i32, rate: f64, dt: f64)`
const AD_D: u32 = 0;
const AD_RATE: u32 = 1;
const AD_DT: u32 = 2;
const AD_VOL: u32 = 3;

/// `q_admit(d, rate, dt)`: `QueueState::admit` (`queue.rs:105`) -- append one
/// batch of `max(rate, 0) * dt`, and nothing at all when that volume is zero (or
/// negative), so the FIFO never accumulates spurious empty batches.
fn emit_admit(push_back: u32) -> Function {
    let mut f = Function::new([(1, ValType::F64)]);

    f.instruction(&Ins::LocalGet(AD_RATE));
    emit_clamp_nonneg(&mut f, AD_VOL);
    f.instruction(&Ins::LocalGet(AD_DT));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::LocalSet(AD_VOL));

    f.instruction(&Ins::LocalGet(AD_VOL));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(AD_D));
    f.instruction(&Ins::LocalGet(AD_VOL));
    f.instruction(&Ins::Call(push_back));
    f.instruction(&Ins::End);

    f.instruction(&Ins::End);
    f
}

// `q_total(d: i32) -> f64`
const T_D: u32 = 0;
const T_I: u32 = 1;
const T_SUM: u32 = 2;

/// `q_total(d) -> Σ batches`: `QueueState::total` (`queue.rs:303`). Summed
/// front-to-back from `0.0`, the identical accumulation order Rust's
/// `iter().sum()` uses, so the two agree bit-for-bit.
fn emit_total() -> Function {
    let mut f = Function::new([(1, ValType::I32), (1, ValType::F64)]);

    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(T_SUM));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(T_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(T_I));
    f.instruction(&Ins::LocalGet(T_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(T_SUM));
    push_ring_addr(&mut f, T_D, T_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(T_SUM));

    f.instruction(&Ins::LocalGet(T_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(T_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(T_SUM));
    f.instruction(&Ins::End);
    f
}

/// `q_serve_unconstrained(d) -> f64`: `QueueState::serve_unconstrained`
/// (`queue.rs:170`) -- sum, then clear.
///
/// Deliberately NOT `take_from_front(total())` (floating-point-fragile: a running
/// remainder can drift below a tiny batch queued behind a huge one and strand it)
/// nor `take_from_front(INFINITY)` (`INFINITY - INFINITY = NaN` fails the loop
/// guard, stranding everything behind a non-finite batch).
fn emit_serve_unconstrained(total: u32) -> Function {
    let mut f = Function::new([]);
    f.instruction(&Ins::LocalGet(0));
    f.instruction(&Ins::Call(total)); // the f64 result stays on the stack ...
    f.instruction(&Ins::LocalGet(0));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32Store(i32_memarg(D_LEN))); // ... across the clear.
    f.instruction(&Ins::End);
    f
}

// `q_take_from_front(d: i32, requested: f64) -> f64`
const TF_D: u32 = 0;
const TF_REQ: u32 = 1;
const TF_REMAINING: u32 = 2;
const TF_REMOVED: u32 = 3;
const TF_FRONT: u32 = 4;
const TF_SCRATCH: u32 = 5;

/// `q_take_from_front(d, requested) -> removed`: `QueueState::take_from_front`
/// (`queue.rs:130`). Pops whole front batches while they fit, splits the boundary
/// batch on a partial take (leaving a strictly-positive remainder as the new
/// front), drains fully on an over-request, and clamps a negative request to a
/// no-op.
fn emit_take_from_front() -> Function {
    let mut f = Function::new([(4, ValType::F64)]);

    f.instruction(&Ins::LocalGet(TF_REQ));
    emit_clamp_nonneg(&mut f, TF_SCRATCH);
    f.instruction(&Ins::LocalSet(TF_REMAINING));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(TF_REMOVED));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));

    // while remaining > 0.0  (false for NaN, matching the Rust loop guard)
    f.instruction(&Ins::LocalGet(TF_REMAINING));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // An empty queue stops the loop: an over-request drains fully and returns
    // what it removed.
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    // front = ring[0]
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_BASE)));
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_HEAD)));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalSet(TF_FRONT));

    f.instruction(&Ins::LocalGet(TF_FRONT));
    f.instruction(&Ins::LocalGet(TF_REMAINING));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Empty));

    // The whole front batch fits: pop it. head = (head + 1) % cap; len -= 1.
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_HEAD)));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_CAP)));
    f.instruction(&Ins::I32RemU);
    f.instruction(&Ins::I32Store(i32_memarg(D_HEAD)));
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::I32Store(i32_memarg(D_LEN)));

    f.instruction(&Ins::LocalGet(TF_REMOVED));
    f.instruction(&Ins::LocalGet(TF_FRONT));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(TF_REMOVED));
    f.instruction(&Ins::LocalGet(TF_REMAINING));
    f.instruction(&Ins::LocalGet(TF_FRONT));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(TF_REMAINING));

    f.instruction(&Ins::Else);

    // Boundary batch: take exactly `remaining`, leaving `front - remaining > 0`
    // as the new front (the no-empty-batch invariant).
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_BASE)));
    f.instruction(&Ins::LocalGet(TF_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_HEAD)));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(TF_FRONT));
    f.instruction(&Ins::LocalGet(TF_REMAINING));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(TF_REMOVED));
    f.instruction(&Ins::LocalGet(TF_REMAINING));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(TF_REMOVED));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(TF_REMAINING));

    f.instruction(&Ins::End); // if/else

    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(TF_REMOVED));
    f.instruction(&Ins::End);
    f
}

// `q_clone_ring(d: i32)`
const CR_D: u32 = 0;
const CR_OLD: u32 = 1;
const CR_CAP: u32 = 2;
const CR_NEW: u32 = 3;
const CR_I: u32 = 4;

/// `q_clone_ring(d)`: bump-allocate a `cap`-slot region, copy the ring verbatim
/// (index for index -- `head`/`len`/`cap` are unchanged, so ring order is
/// preserved), and repoint `base_ptr` at the copy.
///
/// The preview pass then mutates only the copy. The caller restores the saved
/// descriptor and rewinds `G_HEAP` afterwards, so both the copy and any doubling
/// the preview triggered are reclaimed.
fn emit_clone_ring(alloc: u32) -> Function {
    let mut f = Function::new([(4, ValType::I32)]);

    load_desc_i32(&mut f, CR_D, D_BASE, CR_OLD);
    load_desc_i32(&mut f, CR_D, D_CAP, CR_CAP);

    f.instruction(&Ins::LocalGet(CR_CAP));
    f.instruction(&Ins::Call(alloc));
    f.instruction(&Ins::LocalSet(CR_NEW));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(CR_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(CR_I));
    f.instruction(&Ins::LocalGet(CR_CAP));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(CR_NEW));
    f.instruction(&Ins::LocalGet(CR_I));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(CR_OLD));
    f.instruction(&Ins::LocalGet(CR_I));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(CR_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(CR_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(CR_D));
    f.instruction(&Ins::LocalGet(CR_NEW));
    f.instruction(&Ins::I32Store(i32_memarg(D_BASE)));

    f.instruction(&Ins::End);
    f
}

// ── container reducers (§8) ──────────────────────────────────────────────────

/// `q_mean(d) -> f64`: `Σ batches / count`, NaN on an empty queue -- the
/// empty-input contract `container_value_from_slice` gives every reducer but
/// `SUM` (which returns the additive identity 0) and `SIZE`.
fn emit_mean(total: u32) -> Function {
    let mut f = Function::new([]);
    emit_nan_if_empty(&mut f, 0);
    f.instruction(&Ins::LocalGet(0));
    f.instruction(&Ins::Call(total));
    f.instruction(&Ins::LocalGet(0));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::End);
    f
}

// `q_min(d) -> f64` / `q_max(d) -> f64`
const MM_D: u32 = 0;
const MM_I: u32 = 1;
const MM_ACC: u32 = 2;
const MM_X: u32 = 3;

/// `q_min(d)` (`is_min`) / `q_max(d)`: the folds
/// `fold(f64::INFINITY, f64::min)` / `fold(f64::NEG_INFINITY, f64::max)` over a
/// non-empty batch vector; NaN when empty.
///
/// The per-element step is a `select` on a STRICT comparison (`x < acc` /
/// `x > acc`), which is false for a NaN `x` -- so a NaN element leaves the
/// accumulator untouched, exactly as Rust's `f64::min`/`f64::max` do. wasm's
/// `f64.min`/`f64.max` would instead poison the whole fold.
fn emit_min_max(is_min: bool) -> Function {
    let mut f = Function::new([(1, ValType::I32), (2, ValType::F64)]);
    emit_nan_if_empty(&mut f, MM_D);

    f.instruction(&f64_const(if is_min {
        f64::INFINITY
    } else {
        f64::NEG_INFINITY
    }));
    f.instruction(&Ins::LocalSet(MM_ACC));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(MM_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MM_I));
    f.instruction(&Ins::LocalGet(MM_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    push_ring_addr(&mut f, MM_D, MM_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalTee(MM_X));
    f.instruction(&Ins::LocalGet(MM_ACC));
    f.instruction(&Ins::LocalGet(MM_X));
    f.instruction(&Ins::LocalGet(MM_ACC));
    f.instruction(if is_min { &Ins::F64Lt } else { &Ins::F64Gt });
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(MM_ACC));

    f.instruction(&Ins::LocalGet(MM_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MM_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(MM_ACC));
    f.instruction(&Ins::End);
    f
}

// `q_stddev(d) -> f64`
const SD_D: u32 = 0;
const SD_I: u32 = 1;
const SD_N: u32 = 2;
const SD_MEAN: u32 = 3;
const SD_VAR: u32 = 4;
const SD_DIFF: u32 = 5;

/// `q_stddev(d) -> f64`: the POPULATION standard deviation (divisor `N`, matching
/// `container_value_from_slice` and `vm.rs`'s `ArrayStddev`); NaN when empty.
///
/// The reference squares with `.powf(2.0)`; `f64.mul` is used here instead. The
/// two agree for a correctly-rounded `pow`, and `f64.mul` is exact where the
/// blob's open-coded `exp(2*ln x)` `pow` helper would not be.
fn emit_stddev(total: u32) -> Function {
    let mut f = Function::new([(1, ValType::I32), (4, ValType::F64)]);
    emit_nan_if_empty(&mut f, SD_D);

    f.instruction(&Ins::LocalGet(SD_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::LocalSet(SD_N));

    f.instruction(&Ins::LocalGet(SD_D));
    f.instruction(&Ins::Call(total));
    f.instruction(&Ins::LocalGet(SD_N));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(SD_MEAN));

    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(SD_VAR));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(SD_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(SD_I));
    f.instruction(&Ins::LocalGet(SD_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    push_ring_addr(&mut f, SD_D, SD_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(SD_MEAN));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(SD_DIFF));

    f.instruction(&Ins::LocalGet(SD_VAR));
    f.instruction(&Ins::LocalGet(SD_DIFF));
    f.instruction(&Ins::LocalGet(SD_DIFF));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(SD_VAR));

    f.instruction(&Ins::LocalGet(SD_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(SD_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(SD_VAR));
    f.instruction(&Ins::LocalGet(SD_N));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::F64Sqrt);
    f.instruction(&Ins::End);
    f
}

// `q_batch(d: i32, j: i32) -> f64`
const BT_D: u32 = 0;
const BT_J: u32 = 1;

/// `q_batch(d, j) -> f64`: batch `j` counted from the front (0-based), NaN when
/// out of range -- `QueueState::batch` (`queue.rs:293`) composed with
/// `container_value_from_slice`'s `vec.get(j).unwrap_or(NAN)`.
fn emit_batch() -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::LocalGet(BT_J));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::LocalGet(BT_J));
    f.instruction(&Ins::LocalGet(BT_D));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::I32Or);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    push_ring_addr(&mut f, BT_D, BT_J);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::End);
    f
}

// ── shared instruction fragments ─────────────────────────────────────────────

/// `if len == 0 { return NaN }` -- the empty-queue contract of every reducer but
/// `SUM`/`SIZE`.
fn emit_nan_if_empty(f: &mut Function, d_local: u32) {
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(D_LEN)));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);
}

/// `dst_local = mem[d_local + field]` (an i32 descriptor field).
fn load_desc_i32(f: &mut Function, d_local: u32, field: u64, dst_local: u32) {
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(field)));
    f.instruction(&Ins::LocalSet(dst_local));
}

/// Push the byte address of logical batch `i_local` of the queue whose descriptor
/// is at `d_local`: `base + ((head + i) % cap) * 8`.
///
/// `head` is kept in `[0, cap)` by every mutator (init and grow set it to 0; a
/// pop advances it modulo `cap`), so the modulo is an unsigned remainder of a
/// value below `2 * cap`.
fn push_ring_addr(f: &mut Function, d_local: u32, i_local: u32) {
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(D_BASE)));
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(D_HEAD)));
    f.instruction(&Ins::LocalGet(i_local));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(D_CAP)));
    f.instruction(&Ins::I32RemU);
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

/// [`push_ring_addr`] over already-loaded `base`/`cap`/`head` locals -- for
/// `q_grow`, which must address the OLD ring after the descriptor's fields have
/// been captured but before they are overwritten.
fn push_ring_addr_from_locals(f: &mut Function, base: u32, cap: u32, head: u32, i_local: u32) {
    f.instruction(&Ins::LocalGet(base));
    f.instruction(&Ins::LocalGet(head));
    f.instruction(&Ins::LocalGet(i_local));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(cap));
    f.instruction(&Ins::I32RemU);
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

#[cfg(test)]
#[path = "passes_tests.rs"]
mod tests;
