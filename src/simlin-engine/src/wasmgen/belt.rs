// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
// Pure transformation: each emitter appends a wasm instruction sequence for one
// piece of the conveyor belt side-table pass, mirroring the matching VM function
// step-for-step. No I/O; the tests (`belt_tests.rs`) execute the result under the
// DLR-FT interpreter and diff it against the bytecode VM.

//! Lowering of the CONVEYOR belt side-table pass to WebAssembly (GH #922).
//!
//! The sibling [`super::passes`] lowers the queue half of the VM's special-stock
//! machinery; this module lowers the belt half. The two share one bump allocator
//! (`passes::emit_alloc`, `passes::G_HEAP`) and one set of hook points in
//! `module.rs`'s drivers, and a model carrying both runs the belt pass first --
//! the order `queue_compile::run_coupled_passes` uses when nothing is coupled.
//!
//! ## Scope: the CORE belt (step 1 of GH #922)
//!
//! [`reject_unsupported`] refuses, loudly, every conveyor feature outside this
//! step: leak flows, discrete conveyors, `<sample>`/`<arrest>`, non-`beginning`
//! inflow placements (`isee:spreadflow`), container access (GH #923), and
//! queue-conveyor coupling. What remains -- and what this module lowers -- is the
//! continuous belt of `docs/design/conveyors.md` §4: a DT-quantized slat deque
//! that advances one slat per step, discharging its exit slat as the primary
//! outflow and admitting equation-driven inflow subject to `<capacity>` and
//! `<in_limit>`, plus unconditionally-admitted conveyor-driven inflow from an
//! upstream belt (§4.3 step 4).
//!
//! Belt GROWTH and SHRINK (§6.2) fall out of that core rather than being deferred
//! with the other `<sample>`-adjacent flags: `<sample>` defaults to 1, so a belt
//! re-latches its transit time from `<len>` on *every* step, and a `<len>` that
//! is an expression (`test/conveyors/arrayed_conveyor.xmile` names an aux) can
//! therefore change the entry depth mid-run. There is no plan-level signal that
//! distinguishes a constant-valued `<len>` expression from a time-varying one, so
//! a lowering that assumed a fixed belt length would *silently* mis-simulate the
//! second. The ring below implements the general geometry instead.
//!
//! ## What is specialized, and what is not
//!
//! A [`ConveyorPlan`] is fully resolved at compile time: every slot offset, the
//! leak-flow count, the inflow kinds, and the §7.2 init list are constants. The
//! pass is therefore emitted as straight-line, unrolled code -- one belt's phase A
//! after another, then one belt's phase B after another, with every slab address
//! folded into an instruction's `memarg`. Nothing walks a plan structure in linear
//! memory at run time. The only genuinely dynamic quantity is the **slat count**,
//! a function of the latched transit time; the primitives that loop over it
//! (`b_total`, the ring copy, the init fills) are shared helper functions taking a
//! *descriptor pointer*, so one copy serves every belt.
//!
//! ## Memory
//!
//! Each belt owns a 48-byte **descriptor** in the static layout:
//!
//! ```text
//! +0  base:   i32   byte address of the slat ring
//! +4  cap:    i32   ring capacity in SLATS (always >= 1)
//! +8  head:   i32   ring index of slat 0 (the exit), always < cap
//! +12 len:    i32   live slat count
//! +16 d:      i32   latched entry depth = slat_count(latched_transit, dt)
//! +20 stride: i32   bytes per slat
//! +24 c0:     f64   phase A's start-of-step total (`ConveyorState::step_contents0`)
//! +32 out:    f64   phase A's exit volume, consumed by phase B
//! +40 --            reserved (the discrete `in_carry` of step 3)
//! ```
//!
//! `c0`/`out` live in memory rather than in `run_to` locals because phase A runs
//! over EVERY belt before phase B runs over any (§4.3: "no phase reads another
//! conveyor's same-phase results"), so the per-belt phase A results must outlive
//! the unrolled phase A loop, and their count is plan-dependent.
//!
//! `stride` is `8 * (1 + 2 * n_leaks)` bytes -- one f64 of content plus, per leak
//! flow, a `leak_basis` and a `leak_window` (`conveyor.rs`'s `Slat`). Step 1
//! rejects leaks, so it is always 8 today, but it is PLAN-DERIVED and lives in the
//! descriptor: the shared ring helpers stride generically, so step 2 widens the
//! slat without touching them. Every helper that creates a slat zeroes the whole
//! stride (`b_zero_tail`), so the leak words are already initialized when they
//! start existing.
//!
//! The rings live in the bump region past the end of the static layout, addressed
//! by `passes::G_HEAP`. A ring holds `cap` slats and grows by DOUBLING into a
//! fresh bump allocation, abandoning the old region until the next `reset` rewinds
//! the bump pointer wholesale -- exactly the queue pass's discipline, and for the
//! same reasons (see `passes`'s module docs).
//!
//! ## Rust-vs-wasm float semantics
//!
//! `f64::max(NaN, 0.0) == 0.0` in Rust, while wasm's `f64.max` propagates NaN.
//! The belt's `rate.max(0.0)` admission clamp (`conveyor.rs:935`) and its
//! `(...).max(0.0)` capacity-room clamp (`conveyor.rs:899`) both depend on the
//! Rust behavior, so both lower to `passes::emit_clamp_nonneg`'s
//! compare-and-`select`, never to `f64.max`. `f64.min` IS used for the admission
//! `min(req, cap_room, limit_vol)` chain, whose operands are provably non-NaN (see
//! [`ConveyorPass::emit_phase_b`]).

use wasm_encoder::{BlockType, Function, Instruction as Ins, ValType};

use crate::conveyor_compile::ConveyorPlan;

use super::WasmGenError;
use super::errors::ErrorScope;
use super::lower::{HelperFn, f64_const, i32_memarg, memarg};
use super::passes::{SLOT_BYTES, emit_clamp_nonneg, emit_load_curr, slot_addr};

/// Bytes per belt descriptor. Eight-byte-aligned (and a multiple of 8) so the
/// `f64` fields are naturally aligned and the ring addresses derived from `base`
/// stay aligned for `f64` access.
pub(super) const BELT_DESC_BYTES: u32 = 48;
/// Descriptor size in i32 words, for the mid-run preview's save/restore.
const BELT_DESC_WORDS: u64 = BELT_DESC_BYTES as u64 / 4;

const B_BASE: u64 = 0;
const B_CAP: u64 = 4;
const B_HEAD: u64 = 8;
const B_LEN: u64 = 12;
const B_D: u64 = 16;
const B_STRIDE: u64 = 20;
const B_C0: u64 = 24;
const B_OUT: u64 = 32;

/// f64 words per slat for a plan with `n_leaks` leak flows: `content` plus a
/// `leak_basis`/`leak_window` pair per flow (`conveyor.rs:141-149`).
fn slat_words(plan: &ConveyorPlan) -> u32 {
    1 + 2 * plan.leaks.len() as u32
}

/// The eight f64 scratch locals the belt pass needs in its enclosing function
/// (`run_to`). Kept as a struct so `module.rs` owns the local numbering.
#[derive(Clone, Copy)]
pub(super) struct ConveyorPassLocals {
    /// The hygiene-clamped `<len>` value (`conveyor_compile::clamp_transit`).
    pub transit: u32,
    /// `slat_count(transit, dt)` as an f64, before the bound check.
    pub n_slats: u32,
    /// Σ of the unconditionally-admitted conveyor-driven inflow volumes.
    pub conv_vol: u32,
    /// The hygiene-clamped `<capacity>` (`conveyor_compile::clamp_cap`).
    pub capacity: u32,
    /// Remaining capacity room, drawn down across the inflow list.
    pub rem_cap: u32,
    /// Remaining per-DT inflow-limit volume, drawn down likewise.
    pub rem_limit: u32,
    /// The running admitted total, seeded with `conv_vol`; becomes the inserted
    /// cohort's volume.
    pub acc: u32,
    /// Single-use scratch for the clamps and for one cleared volume.
    pub tmp: u32,
}

/// Byte bases of the belt pass's static regions.
#[derive(Clone, Copy)]
pub(super) struct ConveyorPassLayout {
    /// First descriptor; belt `i`'s descriptor is at `desc_base + i*48`.
    pub desc_base: u32,
    /// Descriptor save area for the mid-run preview (same stride).
    pub desc_save_base: u32,
    /// `max_eq_inflows` f64 scratch slots holding one belt's cleared volumes
    /// between the admission loop and the write-back.
    pub scratch_base: u32,
    /// First f64 of the concatenated §7.2 init-list tables.
    pub init_table_base: u32,
}

/// Function indices of the emitted belt primitives. Each takes a descriptor byte
/// address as its first parameter, so ONE copy serves every belt in the model.
#[derive(Clone, Copy)]
struct BeltHelperFns {
    /// `b_total(d) -> f64` -- `ConveyorState::contents` (`conveyor.rs:339`).
    total: u32,
    /// `b_front(d) -> f64` -- the exit slat's content, 0 for an empty belt
    /// (`conveyor.rs:706`).
    front: u32,
    /// `b_slat_count(t) -> f64` -- `conveyor::slat_count` (`conveyor.rs:100`),
    /// left as an f64 so the caller can bound-check before narrowing.
    slat_count: u32,
    /// `b_alloc_ring(d, n)` -- bump-allocate an `n`-slat ring; `head = 0`,
    /// `len = n`, `cap = n`. `stride` must already be set.
    alloc_ring: u32,
    /// `b_init_steady(d, v)` -- the LEAK-FREE §7.1 steady fill.
    init_steady: u32,
    /// `b_init_explicit(d, table, m)` -- the §7.2 explicit list fill.
    init_explicit: u32,
    /// `b_pop_front(d)` -- `VecDeque::pop_front` (§4.3 step 5).
    pop_front: u32,
    /// `b_shrink(d)` -- drop trailing EMPTY slats while `len > d`
    /// (`ConveyorState::shift`'s tail loop).
    shrink: u32,
    /// `b_grow_to_d(d)` -- `while len < d { push_back(empty) }` (§4.3 step 6).
    grow_to_d: u32,
    /// `b_add_at(d, i, v)` -- `slats[i].content += v`.
    add_at: u32,
    /// `b_clone_ring(d)` -- repoint the descriptor at a verbatim ring copy, so the
    /// mid-run preview pass runs on a throwaway side table.
    clone_ring: u32,
}

/// Everything the driver emitters (`run_initials` / `run_to`) need to splice the
/// belt pass into a module.
pub(super) struct ConveyorPass<'a> {
    plans: &'a [ConveyorPlan],
    layout: ConveyorPassLayout,
    fns: BeltHelperFns,
    /// The simulation's fixed timestep. Conveyors are Euler-only
    /// (`conveyor_compile.rs`'s `effective_sim_specs` gate), so `dt` is a
    /// compile-time constant and never read from `curr[DT]`.
    dt: f64,
    /// Byte offset (within the init-table region) and entry count of each plan's
    /// §7.2 list, aligned with `plans`. `None` for a §7.1 steady-fill belt.
    init_tables: Vec<Option<(u32, u32)>>,
}

/// Reject every conveyor feature outside the STEP-1 core, loudly.
///
/// The wasm backend has no silent VM fallback, so a feature this module does not
/// lower must surface as [`WasmGenError::Unsupported`] rather than as a blob whose
/// belts quietly ignore it. Each arm names the feature so the step-2/step-3
/// implementor knows exactly which reject to remove.
///
/// Two things deliberately DO pass:
///
/// * `primary_dest_conveyor` (a belt feeding another belt). It exists only for the
///   held-exit rule of §4.3 step 3, which fires when the destination belt is
///   ARRESTED -- and `<arrest>` is rejected below, so the exit can never be held.
///   The chain itself is ordinary: the upstream belt's phase A writes its outflow
///   rate, and the downstream belt admits it unconditionally in phase B.
/// * `exponential_leak` / `ignore_earlier_zone_losses`. Both are leak-model
///   toggles with no effect on a belt that has no leak flows.
pub(super) fn reject_unsupported(plans: &[ConveyorPlan]) -> Result<(), WasmGenError> {
    let unsupported = |what: &str| {
        Err(WasmGenError::Unsupported(format!(
            "wasmgen: {what} is not yet supported by the wasm backend; \
             the bytecode VM is the only backend that simulates it today"
        )))
    };
    for plan in plans {
        if !plan.leaks.is_empty() {
            return unsupported("a conveyor with leak flows");
        }
        if plan.discrete {
            return unsupported("a discrete conveyor");
        }
        if plan.sample_off.is_some() {
            return unsupported("a conveyor with a <sample> expression");
        }
        if plan.arrest_off.is_some() {
            return unsupported("a conveyor with an <arrest> expression");
        }
        if !plan.containers.is_empty() {
            return unsupported("container access over a conveyor belt (SUM/MEAN/conv[j])");
        }
        for inf in &plan.inflows {
            // Unreachable behind the `plan.discrete` arm above: `detect_coupling_specs`
            // only ever sets `queue_coupled` on a DISCRETE conveyor's inflow, and
            // rejects the coupling outright otherwise (`ConveyorQueueUpstreamNotDiscrete`,
            // `queue_compile.rs`). Kept as defense-in-depth so that lifting the discrete
            // reject in step 3 cannot silently admit an un-lowered coupling.
            if inf.queue_coupled {
                return unsupported("a queue coupled to a discrete conveyor");
            }
            if inf.source
                || inf.dist.is_some()
                || inf.placement != crate::conveyor::Placement::Beginning
            {
                return unsupported("a conveyor inflow with a non-default isee:spreadflow");
            }
        }
    }
    // `b_slat_count`'s result is narrowed with `i32.trunc_f64_s`, which TRAPS
    // outside i32's range. The bound check that precedes it makes that safe only
    // if the bound itself fits -- production's 1,000,000 does, and so does every
    // test `SlatBoundGuard`, but the narrowing must not depend on a constant it
    // does not check.
    //
    // Guarded on a non-empty plan set so an ordinary or queue-only compile never
    // touches the conveyor thread-local: it has nothing to do with those models.
    if !plans.is_empty() && crate::conveyor::slat_bound() > i32::MAX as usize {
        return Err(WasmGenError::Unsupported(
            "wasmgen: the conveyor slat bound exceeds i32::MAX".to_string(),
        ));
    }
    Ok(())
}

impl<'a> ConveyorPass<'a> {
    /// Emit the belt primitives, appending them to `functions` (whose current
    /// length is their first index), and bundle them with the layout.
    ///
    /// `alloc` is the shared bump allocator's index (`passes::emit_alloc`).
    /// Helpers are pushed in dependency order so each inter-helper `call` resolves
    /// against an already-assigned index.
    pub(super) fn build(
        plans: &'a [ConveyorPlan],
        layout: ConveyorPassLayout,
        dt: f64,
        alloc: u32,
        functions: &mut Vec<HelperFn>,
    ) -> ConveyorPass<'a> {
        let mut push = |params: Vec<ValType>, results: Vec<ValType>, body: Function| -> u32 {
            let idx = functions.len() as u32;
            functions.push(HelperFn {
                params,
                results,
                body,
            });
            idx
        };

        let zero_tail = push(vec![ValType::I32, ValType::I32], vec![], emit_zero_tail());
        let total = push(vec![ValType::I32], vec![ValType::F64], emit_total());
        let front = push(vec![ValType::I32], vec![ValType::F64], emit_front());
        let slat_count = push(vec![ValType::F64], vec![ValType::F64], emit_slat_count(dt));
        let alloc_ring = push(
            vec![ValType::I32, ValType::I32],
            vec![],
            emit_alloc_ring(alloc),
        );
        let init_steady = push(
            vec![ValType::I32, ValType::F64],
            vec![],
            emit_init_steady(zero_tail),
        );
        let init_explicit = push(
            vec![ValType::I32, ValType::I32, ValType::I32],
            vec![],
            emit_init_explicit(dt, zero_tail),
        );
        let pop_front = push(vec![ValType::I32], vec![], emit_pop_front());
        let shrink = push(vec![ValType::I32], vec![], emit_shrink());
        let grow = push(vec![ValType::I32], vec![], emit_grow(alloc));
        let push_back = push(vec![ValType::I32], vec![], emit_push_back(grow, zero_tail));
        let grow_to_d = push(vec![ValType::I32], vec![], emit_grow_to_d(push_back));
        let add_at = push(
            vec![ValType::I32, ValType::I32, ValType::F64],
            vec![],
            emit_add_at(),
        );
        let clone_ring = push(vec![ValType::I32], vec![], emit_clone_ring(alloc));

        // Lay the §7.2 lists out back to back in the init-table region, in plan
        // order, so `init_table_data` and these offsets cannot disagree.
        let mut off = 0u32;
        let mut init_tables: Vec<Option<(u32, u32)>> = Vec::with_capacity(plans.len());
        for plan in plans {
            match plan.init_values.as_ref() {
                Some(values) => {
                    init_tables.push(Some((off, values.len() as u32)));
                    off += values.len() as u32 * SLOT_BYTES as u32;
                }
                None => init_tables.push(None),
            }
        }

        ConveyorPass {
            plans,
            layout,
            fns: BeltHelperFns {
                total,
                front,
                slat_count,
                alloc_ring,
                init_steady,
                init_explicit,
                pop_front,
                shrink,
                grow_to_d,
                add_at,
                clone_ring,
            },
            dt,
            init_tables,
        }
    }

    /// Total bytes the concatenated §7.2 init tables occupy.
    pub(super) fn init_table_bytes(plans: &[ConveyorPlan]) -> u32 {
        plans
            .iter()
            .filter_map(|p| p.init_values.as_ref())
            .map(|v| v.len() as u32 * SLOT_BYTES as u32)
            .sum()
    }

    /// The widest equation-driven inflow list across the plan set: the size of the
    /// cleared-volume scratch region, which only one belt's phase B uses at a time.
    ///
    /// This filter MUST stay identical to `emit_phase_b`'s `eq_inflows`, which indexes
    /// the region by position -- a narrower region than that loop's index range would
    /// scribble past it. The VM's own split is `conveyor_driven || queue_coupled`; the
    /// `queue_coupled` disjunct is omitted here only because `reject_unsupported`
    /// refuses those plans, so step 3 must widen BOTH places together.
    pub(super) fn scratch_slots(plans: &[ConveyorPlan]) -> u32 {
        plans
            .iter()
            .map(|p| p.inflows.iter().filter(|i| !i.conveyor_driven).count() as u32)
            .max()
            .unwrap_or(0)
    }

    /// The `(byte address, little-endian f64 bytes)` active data segment that seeds
    /// the §7.2 init-table region at instantiation. One segment per belt with a
    /// list, matching [`Self::build`]'s layout.
    pub(super) fn init_table_data(&self) -> Vec<(u32, Vec<u8>)> {
        self.plans
            .iter()
            .zip(self.init_tables.iter())
            .filter_map(|(plan, table)| {
                let (off, _) = (*table)?;
                let values = plan.init_values.as_ref()?;
                let mut bytes = Vec::with_capacity(values.len() * 8);
                for v in values {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                Some((self.layout.init_table_base + off, bytes))
            })
            .collect()
    }

    /// The absolute slab offsets whose *initials* fragment must be skipped when
    /// reconciling against the belt-derived values (`vm.rs`'s
    /// `reconcile_skip_offsets`). A §7.2 list-initialized conveyor stock qualifies:
    /// `init_belts` writes the normalized belt total into its slot, and re-running
    /// the compiled placeholder `<eqn>` would clobber it before dependent initials
    /// read it. (Container slots join this set in GH #923.)
    pub(super) fn reconcile_skip_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.plans
            .iter()
            .filter(|p| p.init_values.is_some())
            .map(|p| p.stock_off)
    }

    /// Absolute byte address of belt `i`'s descriptor.
    fn desc_addr(&self, i: usize) -> i32 {
        (self.layout.desc_base + (i as u32) * BELT_DESC_BYTES) as i32
    }

    /// Byte address of field `field` of belt `i`'s descriptor, as a `memarg`
    /// offset over a zero dynamic address.
    fn field_addr(&self, i: usize, field: u64) -> u64 {
        u64::from(self.layout.desc_base + (i as u32) * BELT_DESC_BYTES) + field
    }

    /// Absolute byte address of belt `i`'s preview descriptor save slot.
    fn save_addr(&self, i: usize) -> u64 {
        u64::from(self.layout.desc_save_base + (i as u32) * BELT_DESC_BYTES)
    }

    // ── run_initials ────────────────────────────────────────────────────────

    /// Build the belt side table from the initials-populated `curr`
    /// (`conveyor_compile::init_belts`), raising through `scope` on a non-positive
    /// or over-bound transit time.
    ///
    /// The caller must have evaluated the Flows phase first (`module.rs`'s
    /// `pass_needs_flows_before_init`): `curr[len_off]` names a synthesized
    /// belt-parameter aux that nothing depends on, so it is absent from the
    /// initials runlist and holds 0 until Flows writes it.
    ///
    /// `transit`/`n_slats` are two free f64 locals of the ENCLOSING function
    /// (`run_initials`, not `run_to`), which is why this takes them individually
    /// rather than a [`ConveyorPassLocals`].
    pub(super) fn emit_init(
        &self,
        f: &mut Function,
        scope: ErrorScope,
        transit: u32,
        n_slats: u32,
    ) {
        for (i, plan) in self.plans.iter().enumerate() {
            let d = self.desc_addr(i);

            // `init_belts` reads the RAW slot (no `clamp_transit`) and rejects a
            // non-positive or non-finite transit outright.
            emit_load_curr(f, plan.len_off);
            f.instruction(&Ins::LocalSet(transit));
            emit_is_finite(f, transit);
            f.instruction(&Ins::LocalGet(transit));
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Gt);
            f.instruction(&Ins::I32And);
            f.instruction(&Ins::I32Eqz);
            f.instruction(&Ins::If(BlockType::Empty));
            scope
                .entered()
                .raise(f, crate::common::ErrorCode::ConveyorTransitNotPositive, i);
            f.instruction(&Ins::End);

            // §4.1: reject an over-bound slat count BEFORE the ring is allocated.
            f.instruction(&Ins::LocalGet(transit));
            f.instruction(&Ins::Call(self.fns.slat_count));
            f.instruction(&Ins::LocalSet(n_slats));
            f.instruction(&Ins::LocalGet(n_slats));
            f.instruction(&f64_const(crate::conveyor::slat_bound() as f64));
            f.instruction(&Ins::F64Gt);
            f.instruction(&Ins::If(BlockType::Empty));
            scope
                .entered()
                .raise(f, crate::common::ErrorCode::ConveyorTransitTooLong, i);
            f.instruction(&Ins::End);

            // The narrowing is safe: the guard above proved `n <= slat_bound()`,
            // and `b_slat_count` clamps its result to `>= 1`.
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::LocalGet(n_slats));
            f.instruction(&Ins::I32TruncF64S);
            f.instruction(&Ins::I32Store(i32_memarg(self.field_addr(i, B_D))));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Const(
                (slat_words(plan) * SLOT_BYTES as u32) as i32,
            ));
            f.instruction(&Ins::I32Store(i32_memarg(self.field_addr(i, B_STRIDE))));

            f.instruction(&Ins::I32Const(d));
            f.instruction(&Ins::LocalGet(n_slats));
            f.instruction(&Ins::I32TruncF64S);
            f.instruction(&Ins::Call(self.fns.alloc_ring));

            match self.init_tables[i] {
                Some((table_off, m)) => {
                    f.instruction(&Ins::I32Const(d));
                    if m == 0 {
                        // A zero-entry list makes `spread_per_time_unit`'s `norm`
                        // return 0 for every block, i.e. a zero-filled belt --
                        // which the steady fill of a zero initial reproduces
                        // (`e = 0/N = 0`). Routing it here keeps
                        // `b_init_explicit`'s `m >= 1` precondition structural
                        // rather than assumed. (`probe_init_list` cannot produce
                        // an empty list, so this is unreachable defense.)
                        f.instruction(&f64_const(0.0));
                        f.instruction(&Ins::Call(self.fns.init_steady));
                    } else {
                        f.instruction(&Ins::I32Const(
                            (self.layout.init_table_base + table_off) as i32,
                        ));
                        f.instruction(&Ins::I32Const(m as i32));
                        f.instruction(&Ins::Call(self.fns.init_explicit));
                    }
                    // `init_belts` writes the normalized belt total back into the
                    // stock slot. The expansion-time placeholder `<eqn>` already
                    // holds it (`normalized_init_total` runs this same fill), so
                    // this is defense in depth -- and `reconcile_skip_offsets`
                    // keeps the re-run initials from undoing it.
                    f.instruction(&Ins::I32Const(0));
                    f.instruction(&Ins::I32Const(d));
                    f.instruction(&Ins::Call(self.fns.total));
                    f.instruction(&Ins::F64Store(memarg(slot_addr(plan.stock_off))));
                }
                None => {
                    f.instruction(&Ins::I32Const(d));
                    emit_load_curr(f, plan.stock_off);
                    f.instruction(&Ins::Call(self.fns.init_steady));
                }
            }
        }
    }

    // ── the pass proper (between Flows and Stocks) ──────────────────────────

    /// The two-phase belt pass (`conveyor_compile::run_pass`), unrolled over the
    /// plan set: phase A over EVERY belt, then phase B over every belt.
    ///
    /// The split is not cosmetic. A belt's phase B reads `curr[flow_off]` of each
    /// conveyor-driven inflow, which is the *upstream* belt's primary-outflow rate
    /// -- written by that belt's phase A. Interleaving the phases would feed a
    /// downstream belt the previous step's rate whenever the upstream belt happens
    /// to come later in the plan list.
    pub(super) fn emit_step_pass(
        &self,
        f: &mut Function,
        locals: ConveyorPassLocals,
        scope: ErrorScope,
    ) {
        for i in 0..self.plans.len() {
            self.emit_phase_a(f, i, locals, scope);
        }
        for i in 0..self.plans.len() {
            self.emit_phase_b(f, i, locals);
        }
    }

    /// Phase A for belt `i` (§4.3 steps 0-3, minus the arrest/leak/held arms
    /// `reject_unsupported` rules out): snapshot the start-of-step contents, latch
    /// the transit time, and discharge the exit slat as the driven outflow rate.
    ///
    /// The latch is `<sample>`-unconditional here because a belt with no `<sample>`
    /// expression re-latches every DT (§6.1, and `run_phase_a`'s
    /// `unwrap_or(true)`), and a `<sample>` expression is rejected.
    fn emit_phase_a(
        &self,
        f: &mut Function,
        i: usize,
        locals: ConveyorPassLocals,
        scope: ErrorScope,
    ) {
        let plan = &self.plans[i];
        let d = self.desc_addr(i);

        // step_contents0 = contents()  (before the latch, as `phase_a` does).
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.total));
        f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_C0))));

        // Step 1: latch. `clamp_transit` passes a non-finite value through
        // unchanged, and `phase_a` then skips the latch -- so the entry depth and
        // the slat-bound check are both conditional on finiteness.
        emit_load_curr(f, plan.len_off);
        emit_clamp_transit(f, self.dt, locals.transit);
        f.instruction(&Ins::LocalSet(locals.transit));
        f.instruction(&Ins::LocalGet(locals.transit));
        f.instruction(&Ins::Call(self.fns.slat_count));
        f.instruction(&Ins::LocalSet(locals.n_slats));

        // §4.1: `run_phase_a` bound-checks exactly when it would latch.
        emit_is_finite(f, locals.transit);
        f.instruction(&Ins::LocalGet(locals.n_slats));
        f.instruction(&f64_const(crate::conveyor::slat_bound() as f64));
        f.instruction(&Ins::F64Gt);
        f.instruction(&Ins::I32And);
        f.instruction(&Ins::If(BlockType::Empty));
        scope
            .entered()
            .raise(f, crate::common::ErrorCode::ConveyorTransitTooLong, i);
        f.instruction(&Ins::End);

        // d = is_finite(transit) ? trunc(n_slats) : d. The narrowing is safe on
        // both arms: the guard above proved `n <= slat_bound()` on the latching
        // arm, and `b_slat_count` returned the clamped 1.0 on the other.
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::LocalGet(locals.n_slats));
        f.instruction(&Ins::I32TruncF64S);
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Load(i32_memarg(self.field_addr(i, B_D))));
        emit_is_finite(f, locals.transit);
        f.instruction(&Ins::Select);
        f.instruction(&Ins::I32Store(i32_memarg(self.field_addr(i, B_D))));

        // Step 3: exit. `out_vol` is consumed by phase B (capacity room) and by the
        // Stocks phase through the driven rate written here.
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.front));
        f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_OUT))));

        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_OUT))));
        f.instruction(&f64_const(self.dt));
        f.instruction(&Ins::F64Div);
        f.instruction(&Ins::F64Store(memarg(slot_addr(plan.primary_out_off))));
    }

    /// Phase B for belt `i` (§4.3 steps 4-6): admit, shift, insert, write back.
    ///
    /// The admission chain uses wasm's `f64.min` rather than a compare-and-select,
    /// which is sound because no operand can be NaN: `emit_clamp_nonneg` maps a NaN
    /// rate to `0.0`, so the request volume is a non-negative (possibly infinite)
    /// number; `rem_cap` starts at `+INF` or at a `max(0, ·)` clamp (also NaN-free);
    /// `rem_limit` starts at `+INF` or at `clamp_cap(·) * dt >= 0`; and each is only
    /// decremented by a `c <= itself` when finite, so both stay non-negative and
    /// non-NaN. That is exactly the reasoning `ConveyorState::phase_b` relies on for
    /// its `f64::min` calls.
    fn emit_phase_b(&self, f: &mut Function, i: usize, locals: ConveyorPassLocals) {
        let plan = &self.plans[i];
        let d = self.desc_addr(i);

        // Step 4a: conveyor-driven volume, admitted unconditionally. Accumulated
        // from `0.0` in inflow order, matching `conv_inflows.iter().sum()`.
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::LocalSet(locals.conv_vol));
        for inf in plan.inflows.iter().filter(|inf| inf.conveyor_driven) {
            f.instruction(&Ins::LocalGet(locals.conv_vol));
            emit_load_curr(f, inf.flow_off);
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Mul);
            f.instruction(&Ins::F64Add);
            f.instruction(&Ins::LocalSet(locals.conv_vol));
        }

        // Step 4b: `cap_room` (`ConveyorState::admission_room`). An absent
        // `<capacity>` is a compile-time `+INF`; a present one is clamped, and the
        // `is_infinite` arm matters because `INF - NaN` is NaN while the VM's
        // infinite-capacity branch yields `+INF` unconditionally.
        match plan.cap_off {
            None => {
                f.instruction(&f64_const(f64::INFINITY));
            }
            Some(off) => {
                emit_load_curr(f, off);
                emit_clamp_cap(f, locals.tmp);
                f.instruction(&Ins::LocalSet(locals.capacity));
                f.instruction(&f64_const(f64::INFINITY)); // select's `true` arm
                f.instruction(&Ins::LocalGet(locals.capacity));
                // contents_after = step_contents0 - leaked - out_vol; with no leak
                // flows `leaked` is the empty sum `+0.0`, and `x - 0.0 == x`.
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_C0))));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_OUT))));
                f.instruction(&Ins::F64Sub);
                f.instruction(&Ins::F64Sub);
                f.instruction(&Ins::LocalGet(locals.conv_vol));
                f.instruction(&Ins::F64Sub);
                emit_clamp_nonneg(f, locals.tmp); // select's `false` arm
                f.instruction(&Ins::LocalGet(locals.capacity));
                f.instruction(&f64_const(f64::INFINITY));
                f.instruction(&Ins::F64Eq);
                f.instruction(&Ins::Select);
            }
        }
        f.instruction(&Ins::LocalSet(locals.rem_cap));

        // Step 4c: `limit_vol`. A CONTINUOUS belt prorates the per-time-unit limit
        // to this DT, and `INF * dt == INF`, so the VM's `is_infinite` branch folds
        // away. (The discrete per-time-unit `in_carry` budget is step 3's.)
        match plan.inlim_off {
            None => {
                f.instruction(&f64_const(f64::INFINITY));
            }
            Some(off) => {
                emit_load_curr(f, off);
                emit_clamp_cap(f, locals.tmp);
                f.instruction(&f64_const(self.dt));
                f.instruction(&Ins::F64Mul);
            }
        }
        f.instruction(&Ins::LocalSet(locals.rem_limit));

        // Step 4d: apportion the clearance across the equation-driven inflows in
        // listed order. `acc` accumulates the inserted cohort exactly as the VM's
        // `shares[d-1]` does: `0.0 + conv_0 + ... + cleared_0 + ...`.
        f.instruction(&Ins::LocalGet(locals.conv_vol));
        f.instruction(&Ins::LocalSet(locals.acc));
        let eq_inflows: Vec<usize> = plan
            .inflows
            .iter()
            .filter(|inf| !inf.conveyor_driven)
            .map(|inf| inf.flow_off)
            .collect();
        for (j, &off) in eq_inflows.iter().enumerate() {
            emit_load_curr(f, off);
            emit_clamp_nonneg(f, locals.tmp);
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Mul);
            f.instruction(&Ins::LocalGet(locals.rem_cap));
            f.instruction(&Ins::F64Min);
            f.instruction(&Ins::LocalGet(locals.rem_limit));
            f.instruction(&Ins::F64Min);
            f.instruction(&Ins::LocalSet(locals.tmp));

            // Park the cleared VOLUME, mirroring the VM's `in_vols` vector.
            //
            // Writing `curr[off] = c / dt` straight from this loop would be
            // observationally equivalent TODAY. A stock CAN list the same flow twice
            // (nothing rejects it: both entries land in `eq_inflows` with the same
            // `flow_off`), so an in-loop write-back really would let iteration 1
            // re-read iteration 0's admitted rate instead of the requested one -- but
            // the result is unchanged either way. Writing `r` for the requested rate
            // and `c0 = min(r*dt, rem)`: if `c0 == r*dt` the re-read feeds back the
            // same `r*dt`, and if `c0 == rem` the clamp drew `rem` to 0, so both
            // chains yield `min(anything, 0) == 0`. The region earns its keep for two
            // other reasons.
            //
            // 1. The volume, not the rate, is the quantity the later phases consume:
            //    step 2's placements distribute a cleared volume across slats and
            //    step 3's quantization rounds one. Recovering it as `curr[off] * dt`
            //    is a division followed by a multiplication, and `(c / dt) * dt != c`
            //    for most `c` -- so the round trip would silently stop being
            //    bit-identical to the VM.
            // 2. Both of those phases run AFTER every inflow has cleared, so the
            //    volumes must outlive this loop regardless.
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::LocalGet(locals.tmp));
            f.instruction(&Ins::F64Store(memarg(self.scratch_addr(j))));

            f.instruction(&Ins::LocalGet(locals.acc));
            f.instruction(&Ins::LocalGet(locals.tmp));
            f.instruction(&Ins::F64Add);
            f.instruction(&Ins::LocalSet(locals.acc));

            // `if rem.is_finite() { rem -= c }`. The guard is load-bearing: an
            // unconstrained belt fed an infinite rate would otherwise compute
            // `INF - INF = NaN` and poison every later inflow's clearance.
            for rem in [locals.rem_cap, locals.rem_limit] {
                f.instruction(&Ins::LocalGet(rem));
                f.instruction(&Ins::LocalGet(locals.tmp));
                f.instruction(&Ins::F64Sub);
                f.instruction(&Ins::LocalGet(rem));
                emit_is_finite(f, rem);
                f.instruction(&Ins::Select);
                f.instruction(&Ins::LocalSet(rem));
            }
        }

        // Step 5: shift. The exit slat left as outflow (never held: `<arrest>` is
        // rejected), then trailing empty slats beyond the entry depth fall off.
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.pop_front));
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.shrink));

        // Step 6: insert. The belt first grows to the (possibly just-increased)
        // entry depth, THEN the admitted cohort lands at the entry slat `d-1` --
        // the `beginning` placement, the only one this step lowers. The `!= 0.0`
        // gate mirrors `phase_b`'s `shares.iter().any(|&s| s != 0.0)`: it also
        // skips a `-0.0` total (`-0.0 != 0.0` is false) and admits a NaN one.
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.grow_to_d));
        f.instruction(&Ins::LocalGet(locals.acc));
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::F64Ne);
        f.instruction(&Ins::If(BlockType::Empty));
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Load(i32_memarg(self.field_addr(i, B_D))));
        f.instruction(&Ins::I32Const(1));
        f.instruction(&Ins::I32Sub);
        f.instruction(&Ins::LocalGet(locals.acc));
        f.instruction(&Ins::Call(self.fns.add_at));
        f.instruction(&Ins::End);

        // Write the admitted equation-driven rates back, in listed order. A
        // conveyor-driven inflow's slot already holds the upstream belt's phase A
        // rate and must not be overwritten.
        for (j, &off) in eq_inflows.iter().enumerate() {
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::F64Load(memarg(self.scratch_addr(j))));
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Div);
            f.instruction(&Ins::F64Store(memarg(slot_addr(off))));
        }
    }

    fn scratch_addr(&self, j: usize) -> u64 {
        u64::from(self.layout.scratch_base) + j as u64 * SLOT_BYTES as u64
    }

    // ── the mid-run preview ─────────────────────────────────────────────────

    /// Save each belt descriptor and repoint it at a verbatim ring copy, so the
    /// preview pass mutates only the copy (`vm.rs`'s cloned side tables). The
    /// caller saves and rewinds `G_HEAP` around this; see
    /// `passes::QueuePass::emit_preview_save`.
    pub(super) fn emit_preview_save(&self, f: &mut Function) {
        for i in 0..self.plans.len() {
            let d = self.field_addr(i, 0);
            let s = self.save_addr(i);
            for w in 0..BELT_DESC_WORDS {
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Load(i32_memarg(d + w * 4)));
                f.instruction(&Ins::I32Store(i32_memarg(s + w * 4)));
            }
            f.instruction(&Ins::I32Const(self.desc_addr(i)));
            f.instruction(&Ins::Call(self.fns.clone_ring));
        }
    }

    /// Restore each saved descriptor, dropping the cloned ring (and, with it, the
    /// preview's `c0`/`out`/`d` writes).
    pub(super) fn emit_preview_restore(&self, f: &mut Function) {
        for i in 0..self.plans.len() {
            let d = self.field_addr(i, 0);
            let s = self.save_addr(i);
            for w in 0..BELT_DESC_WORDS {
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Load(i32_memarg(s + w * 4)));
                f.instruction(&Ins::I32Store(i32_memarg(d + w * 4)));
            }
        }
    }
}

// ── shared instruction fragments ─────────────────────────────────────────────

/// Push `x.is_finite()` as an i32 condition, where `x` is already in `local`.
/// `|x| < INF` is false for NaN and for either infinity.
fn emit_is_finite(f: &mut Function, local: u32) {
    f.instruction(&Ins::LocalGet(local));
    f.instruction(&Ins::F64Abs);
    f.instruction(&f64_const(f64::INFINITY));
    f.instruction(&Ins::F64Lt);
}

/// Replace the f64 on the stack with `conveyor_compile::clamp_transit(v, dt)`:
/// `max(v, dt)` for a finite `v`, `v` unchanged otherwise (so `phase_a` skips the
/// latch). `f64.max` is safe on the finite arm -- both operands are non-NaN -- and
/// its NaN result on the other arm is discarded by the `select`.
fn emit_clamp_transit(f: &mut Function, dt: f64, scratch: u32) {
    f.instruction(&Ins::LocalTee(scratch));
    f.instruction(&f64_const(dt));
    f.instruction(&Ins::F64Max);
    f.instruction(&Ins::LocalGet(scratch));
    emit_is_finite(f, scratch);
    f.instruction(&Ins::Select);
}

/// Replace the f64 on the stack with `conveyor_compile::clamp_cap(v)`: NaN becomes
/// `+INF` (no constraint), a negative value (including `-INF`) becomes 0, and
/// every other value passes through UNCHANGED -- with one documented exception.
/// `max(v, 0.0)` covers both non-NaN arms (`-INF` clamps to 0, `+INF` survives), so
/// only the NaN case needs a select.
///
/// The exception is `v == -0.0`: wasm's `f64.max(-0.0, +0.0)` returns `+0.0`, while
/// Rust's `f64::max` returns `-0.0` (it is permitted to return either operand when
/// they compare equal). So a `-0.0` capacity leaves this helper with a different
/// SIGN of zero than the VM's. Nothing downstream can observe it -- the value is only
/// ever compared against `+INF` and subtracted from -- and `belt_tests`' `EPS` exists
/// precisely to keep that sign ambiguity from ever becoming a flake.
fn emit_clamp_cap(f: &mut Function, scratch: u32) {
    f.instruction(&Ins::LocalTee(scratch));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Max); // select's `true` (non-NaN) arm
    f.instruction(&f64_const(f64::INFINITY)); // select's `false` (NaN) arm
    f.instruction(&Ins::LocalGet(scratch));
    f.instruction(&Ins::LocalGet(scratch));
    f.instruction(&Ins::F64Eq); // x == x, i.e. !x.is_nan()
    f.instruction(&Ins::Select);
}

/// Push the byte address of logical slat `i_local` of the belt whose descriptor is
/// at `d_local`: `base + ((head + i) % cap) * stride`.
///
/// `head < cap` and `i < len <= cap` for every caller, so `head + i < 2 * cap` and
/// the unsigned remainder is a single wrap.
fn push_slat_addr(f: &mut Function, d_local: u32, i_local: u32) {
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(B_BASE)));
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(B_HEAD)));
    f.instruction(&Ins::LocalGet(i_local));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(B_CAP)));
    f.instruction(&Ins::I32RemU);
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(B_STRIDE)));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
}

/// `local.get d; i32.load offset=field; local.set dst`.
fn load_desc_i32(f: &mut Function, d_local: u32, field: u64, dst_local: u32) {
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(field)));
    f.instruction(&Ins::LocalSet(dst_local));
}

/// `desc[field] = value_from(f)`, where the caller leaves the i32 value on the
/// stack after this pushes the address.
fn store_desc_i32(f: &mut Function, d_local: u32, field: u64, value: impl FnOnce(&mut Function)) {
    f.instruction(&Ins::LocalGet(d_local));
    value(f);
    f.instruction(&Ins::I32Store(i32_memarg(field)));
}

// ── helper bodies ────────────────────────────────────────────────────────────
//
// Local numbering per helper is spelled out beside each emitter. Every helper's
// first parameter is the descriptor byte address `d` (except `b_slat_count`, a
// pure scalar function), so its i32 fields are reached as
// `local.get d; i32.load offset=B_*`.

// `b_zero_tail(d: i32, addr: i32)`
const ZT_D: u32 = 0;
const ZT_ADDR: u32 = 1;
const ZT_W: u32 = 2;

/// `b_zero_tail(d, addr)`: zero every f64 of the slat at `addr` PAST its content
/// word, i.e. the `leak_basis`/`leak_window` pairs (`conveyor.rs`'s
/// `Slat::empty`). A zero-iteration loop for the leak-free step-1 stride of 8, and
/// the seam step 2 grows into: a bump-allocated region is NOT zero on reuse (the
/// bump pointer rewinds on `reset`), so a fresh slat must be written, never
/// assumed clean.
fn emit_zero_tail() -> Function {
    let mut f = Function::new([(1, ValType::I32)]);

    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::LocalSet(ZT_W));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(ZT_W));
    f.instruction(&Ins::LocalGet(ZT_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_STRIDE)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(ZT_ADDR));
    f.instruction(&Ins::LocalGet(ZT_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(ZT_W));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(ZT_W));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

// `b_total(d: i32) -> f64`
const T_D: u32 = 0;
const T_I: u32 = 1;
const T_SUM: u32 = 2;

/// `b_total(d) -> Σ slat contents`: `ConveyorState::contents` (`conveyor.rs:339`).
/// Summed front-to-back (exit first) from `0.0`, the accumulation order Rust's
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
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(T_SUM));
    push_slat_addr(&mut f, T_D, T_I);
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

// `b_front(d: i32) -> f64`
const FR_D: u32 = 0;
const FR_ZERO: u32 = 1;

/// `b_front(d) -> f64`: the exit slat's content, or `0.0` for an empty belt --
/// `self.slats.front().map(|s| s.content).unwrap_or(0.0)` (`conveyor.rs:706`).
fn emit_front() -> Function {
    let mut f = Function::new([(1, ValType::I32)]);

    f.instruction(&Ins::LocalGet(FR_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(FR_ZERO));
    push_slat_addr(&mut f, FR_D, FR_ZERO);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::End);
    f
}

// `b_slat_count(t: f64) -> f64`
const SC_T: u32 = 0;
const SC_N: u32 = 1;

/// `b_slat_count(t) -> f64`: `conveyor::slat_count(t, dt)` (`conveyor.rs:100`),
/// left in f64 so the caller can bound-check before narrowing.
///
/// `floor(t/dt + 0.5)` is round-half-AWAY-from-zero for the non-negative arguments
/// a positive transit time and DT produce -- deliberately NOT `f64.nearest`, which
/// is round-half-to-EVEN and disagrees at every `x.5` (a transit of `1.5*dt` needs
/// two slats, not two-rounded-to-even). The result is then clamped to `>= 1`, with
/// a non-finite `n` (from a non-finite `t`) also collapsing to 1: `NaN as usize` is
/// 0 in Rust, which would later underflow a `d - 1` slat index.
fn emit_slat_count(dt: f64) -> Function {
    let mut f = Function::new([(1, ValType::F64)]);

    f.instruction(&Ins::LocalGet(SC_T));
    f.instruction(&f64_const(dt));
    f.instruction(&Ins::F64Div);
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::LocalSet(SC_N));

    // select(n, 1.0, n >= 1.0 && n.is_finite())
    f.instruction(&Ins::LocalGet(SC_N));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(SC_N));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Ge);
    emit_is_finite(&mut f, SC_N);
    f.instruction(&Ins::I32And);
    f.instruction(&Ins::Select);
    f.instruction(&Ins::End);
    f
}

// `b_alloc_ring(d: i32, n: i32)`
const AR_D: u32 = 0;
const AR_N: u32 = 1;

/// `b_alloc_ring(d, n)`: bump-allocate an `n`-slat ring and install it as a FULL
/// belt (`head = 0`, `len = cap = n`), ready for an init fill. `stride` must
/// already be in the descriptor.
///
/// `cap == n` is exactly enough for the steady state: each step pops the exit slat
/// before pushing a new entry one, so `len` never exceeds `n` while the entry depth
/// holds. A depth that GROWS trips `b_push_back`'s doubling.
fn emit_alloc_ring(alloc: u32) -> Function {
    let mut f = Function::new([]);

    store_desc_i32(&mut f, AR_D, B_BASE, |f| {
        // slots = n * (stride / 8), the bump allocator's f64-slot unit.
        f.instruction(&Ins::LocalGet(AR_N));
        f.instruction(&Ins::LocalGet(AR_D));
        f.instruction(&Ins::I32Load(i32_memarg(B_STRIDE)));
        f.instruction(&Ins::I32Const(SLOT_BYTES));
        f.instruction(&Ins::I32DivS);
        f.instruction(&Ins::I32Mul);
        f.instruction(&Ins::Call(alloc));
    });
    store_desc_i32(&mut f, AR_D, B_CAP, |f| {
        f.instruction(&Ins::LocalGet(AR_N));
    });
    store_desc_i32(&mut f, AR_D, B_HEAD, |f| {
        f.instruction(&Ins::I32Const(0));
    });
    store_desc_i32(&mut f, AR_D, B_LEN, |f| {
        f.instruction(&Ins::LocalGet(AR_N));
    });

    f.instruction(&Ins::End);
    f
}

// `b_init_steady(d: i32, v: f64)`
const IS_D: u32 = 0;
const IS_V: u32 = 1;
const IS_I: u32 = 2;
const IS_ADDR: u32 = 3;
const IS_E: u32 = 4;

/// `b_init_steady(d, v)`: the §7.1 steady fill of a LEAK-FREE belt.
///
/// With no leak flows the retained profile `c[i]` is 1 at every slat
/// (`init_steady`'s `c[i-1] = (c[i] - 0.0).max(0.0)`), so `S = Σ c[i] == N`
/// exactly (summing `1.0` N times is exact for any N below 2^53) and every slat
/// holds `E = v / N` (`E * c[i] == E * 1.0 == E`, bit-for-bit). Steps 1-3 of the
/// general algorithm therefore collapse to one division -- a specialization step 2
/// undoes when a leak profile makes `c[i]` non-uniform.
fn emit_init_steady(zero_tail: u32) -> Function {
    let mut f = Function::new([(2, ValType::I32), (1, ValType::F64)]);

    f.instruction(&Ins::LocalGet(IS_V));
    f.instruction(&Ins::LocalGet(IS_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_D)));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(IS_E));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(IS_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IS_I));
    f.instruction(&Ins::LocalGet(IS_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    push_slat_addr(&mut f, IS_D, IS_I);
    f.instruction(&Ins::LocalTee(IS_ADDR));
    f.instruction(&Ins::LocalGet(IS_E));
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(IS_D));
    f.instruction(&Ins::LocalGet(IS_ADDR));
    f.instruction(&Ins::Call(zero_tail));

    f.instruction(&Ins::LocalGet(IS_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(IS_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

// `b_init_explicit(d: i32, table: i32, m: i32)`
const IE_D: u32 = 0;
const IE_TABLE: u32 = 1;
const IE_M: u32 = 2;
const IE_I: u32 = 3;
const IE_J: u32 = 4;
const IE_K: u32 = 5;
const IE_ADDR: u32 = 6;
const IE_BLOCK: u32 = 7;
const IE_VAL: u32 = 8;

/// `b_init_explicit(d, table, m)`: the §7.2 explicit-list fill of a LEAK-FREE belt,
/// reproducing `ConveyorState::init_explicit` for a continuous conveyor.
///
/// Two interpretations, selected by list length (the isee rule):
///
/// * `m == N`: entry `j` fills slat `j` directly.
/// * otherwise: one entry per TIME UNIT. Slat `i` belongs to block
///   `floor(i * dt)`, the list is normalized to `U = floor((N-1)*dt) + 1` entries
///   (extra truncated, a short list repeating its last), and each block's entry is
///   spread evenly across the block's slats.
///
/// Blocks are contiguous and `floor(i * dt)` is monotone in `i`, so the per-block
/// slat COUNT `spread_per_time_unit` tabulates is just this loop's run length --
/// no counts array is needed. `block_of`'s `.min(u - 1)` clamp is likewise
/// omitted: `i <= N-1` and f64 multiplication is monotone, so `floor(i*dt)` can
/// never exceed `floor((N-1)*dt) == u - 1`.
///
/// `norm(b)` is `table[min(b, m-1)]`, which is why `m >= 1` is a precondition
/// (`build` routes an empty list to the zero-filling steady path instead).
fn emit_init_explicit(dt: f64, zero_tail: u32) -> Function {
    let mut f = Function::new([(4, ValType::I32), (2, ValType::F64)]);

    // Direct per-slat fill when the list has exactly one entry per slat.
    f.instruction(&Ins::LocalGet(IE_M));
    f.instruction(&Ins::LocalGet(IE_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_D)));
    f.instruction(&Ins::I32Eq);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(IE_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::LocalGet(IE_M));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    push_slat_addr(&mut f, IE_D, IE_I);
    f.instruction(&Ins::LocalTee(IE_ADDR));
    f.instruction(&Ins::LocalGet(IE_TABLE));
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(IE_D));
    f.instruction(&Ins::LocalGet(IE_ADDR));
    f.instruction(&Ins::Call(zero_tail));

    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(IE_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End); // if

    // Per-time-unit spread, one contiguous block of slats at a time.
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(IE_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::LocalGet(IE_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_D)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    // block = floor(i * dt)
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&f64_const(dt));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::LocalSet(IE_BLOCK));

    // j = the first slat of the NEXT block.
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(IE_J));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IE_J));
    f.instruction(&Ins::LocalGet(IE_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_D)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(IE_J));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&f64_const(dt));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::LocalGet(IE_BLOCK));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(IE_J));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(IE_J));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // inner loop
    f.instruction(&Ins::End); // inner block

    // val = table[min(block, m - 1)] / (j - i)
    f.instruction(&Ins::LocalGet(IE_TABLE));
    f.instruction(&Ins::LocalGet(IE_BLOCK));
    f.instruction(&Ins::LocalGet(IE_M));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Min);
    f.instruction(&Ins::I32TruncF64S);
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(IE_J));
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(IE_VAL));

    // for k in i..j: content[k] = val
    f.instruction(&Ins::LocalGet(IE_I));
    f.instruction(&Ins::LocalSet(IE_K));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IE_K));
    f.instruction(&Ins::LocalGet(IE_J));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    push_slat_addr(&mut f, IE_D, IE_K);
    f.instruction(&Ins::LocalTee(IE_ADDR));
    f.instruction(&Ins::LocalGet(IE_VAL));
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(IE_D));
    f.instruction(&Ins::LocalGet(IE_ADDR));
    f.instruction(&Ins::Call(zero_tail));
    f.instruction(&Ins::LocalGet(IE_K));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(IE_K));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // fill loop
    f.instruction(&Ins::End); // fill block

    f.instruction(&Ins::LocalGet(IE_J));
    f.instruction(&Ins::LocalSet(IE_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // outer loop
    f.instruction(&Ins::End); // outer block

    f.instruction(&Ins::End);
    f
}

// `b_pop_front(d: i32)`
const PF_D: u32 = 0;

/// `b_pop_front(d)`: `VecDeque::pop_front` -- the exit slat left the belt as this
/// DT's primary outflow (§4.3 step 5). A no-op on an empty belt, like the Rust
/// `pop_front()` whose `Option` `shift` discards.
fn emit_pop_front() -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::LocalGet(PF_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    store_desc_i32(&mut f, PF_D, B_HEAD, |f| {
        f.instruction(&Ins::LocalGet(PF_D));
        f.instruction(&Ins::I32Load(i32_memarg(B_HEAD)));
        f.instruction(&Ins::I32Const(1));
        f.instruction(&Ins::I32Add);
        f.instruction(&Ins::LocalGet(PF_D));
        f.instruction(&Ins::I32Load(i32_memarg(B_CAP)));
        f.instruction(&Ins::I32RemU);
    });
    store_desc_i32(&mut f, PF_D, B_LEN, |f| {
        f.instruction(&Ins::LocalGet(PF_D));
        f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
        f.instruction(&Ins::I32Const(1));
        f.instruction(&Ins::I32Sub);
    });

    f.instruction(&Ins::End);
    f
}

// `b_shrink(d: i32)`
const SH_D: u32 = 0;
const SH_I: u32 = 1;

/// `b_shrink(d)`: `ConveyorState::shift`'s tail -- drop trailing EMPTY slats while
/// the belt is longer than the entry depth. A shortened transit time leaves the
/// belt over-long; it "shrinks naturally as empty tail slats fall off" (§6.2), and
/// a non-empty tail slat stops the loop, keeping the deeper material on the belt.
///
/// `content == 0.0` matches both zeros and is false for NaN (which therefore stops
/// the loop), exactly as the Rust `Some(s) if s.content == 0.0` guard does.
///
/// Note the asymmetry, so nobody chases the wrong mutant here. The *presence* of this
/// call is unobservable: skipping it leaves the state `correct_state ++ trailing
/// zeros`, an invariant that `b_pop_front` (pops the shared front), `b_grow_to_d`
/// (only pushes when `len < d`, and the extra zeros already satisfy it), and
/// `b_add_at` (writes at the absolute index `d - 1`, inside `correct_state`) all
/// preserve -- and which `b_total` (adds zeros) and `b_front` cannot see. Deleting
/// the call is a genuinely equivalent mutant. What this function buys is BOUNDED RING
/// GROWTH: without it a belt whose depth oscillates keeps `len` pinned at the deepest
/// depth ever latched.
///
/// A *wrong* body, on the other hand, is loudly observable -- storing `0` rather than
/// `len - 1` annihilates the belt the first time the loop fires. That is what
/// `belt_tests::empty_tail_shrinks_without_leaks` pins, and it is the only test in
/// which this loop body executes at all.
fn emit_shrink() -> Function {
    let mut f = Function::new([(1, ValType::I32)]);

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));

    f.instruction(&Ins::LocalGet(SH_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::LocalGet(SH_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_D)));
    f.instruction(&Ins::I32LeS);
    f.instruction(&Ins::BrIf(1));

    // i = len - 1 (the back slat), which exists because len > d >= 1.
    f.instruction(&Ins::LocalGet(SH_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(SH_I));
    push_slat_addr(&mut f, SH_D, SH_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));

    store_desc_i32(&mut f, SH_D, B_LEN, |f| {
        f.instruction(&Ins::LocalGet(SH_I));
    });
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

// `b_grow(d: i32)`
const GR_D: u32 = 0;
const GR_OLD_BASE: u32 = 1;
const GR_OLD_CAP: u32 = 2;
const GR_HEAD: u32 = 3;
const GR_LEN: u32 = 4;
const GR_STRIDE: u32 = 5;
const GR_NEW_BASE: u32 = 6;
const GR_I: u32 = 7;
const GR_W: u32 = 8;
const GR_SRC: u32 = 9;
const GR_DST: u32 = 10;

/// `b_grow(d)`: double the ring capacity, copying the `len` live slats into a fresh
/// allocation IN RING ORDER (so the copy normalizes `head` to 0). The whole slat --
/// content plus every leak word -- moves, so widening the stride needs no change
/// here.
///
/// The old region is abandoned rather than freed; `reset` rewinds the bump pointer
/// wholesale, so a long run's peak is `O(final capacity)` and repeated runs do not
/// accumulate. Same discipline as `passes::emit_grow`.
fn emit_grow(alloc: u32) -> Function {
    let mut f = Function::new([(10, ValType::I32)]);

    load_desc_i32(&mut f, GR_D, B_BASE, GR_OLD_BASE);
    load_desc_i32(&mut f, GR_D, B_CAP, GR_OLD_CAP);
    load_desc_i32(&mut f, GR_D, B_HEAD, GR_HEAD);
    load_desc_i32(&mut f, GR_D, B_LEN, GR_LEN);
    load_desc_i32(&mut f, GR_D, B_STRIDE, GR_STRIDE);

    // new_base = alloc(2 * old_cap * stride/8)
    f.instruction(&Ins::LocalGet(GR_OLD_CAP));
    f.instruction(&Ins::I32Const(2));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::LocalGet(GR_STRIDE));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32DivS);
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::Call(alloc));
    f.instruction(&Ins::LocalSet(GR_NEW_BASE));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(GR_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(GR_I));
    f.instruction(&Ins::LocalGet(GR_LEN));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    // src = old_base + ((head + i) % old_cap) * stride;  dst = new_base + i*stride
    f.instruction(&Ins::LocalGet(GR_OLD_BASE));
    f.instruction(&Ins::LocalGet(GR_HEAD));
    f.instruction(&Ins::LocalGet(GR_I));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(GR_OLD_CAP));
    f.instruction(&Ins::I32RemU);
    f.instruction(&Ins::LocalGet(GR_STRIDE));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(GR_SRC));
    f.instruction(&Ins::LocalGet(GR_NEW_BASE));
    f.instruction(&Ins::LocalGet(GR_I));
    f.instruction(&Ins::LocalGet(GR_STRIDE));
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(GR_DST));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(GR_W));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(GR_W));
    f.instruction(&Ins::LocalGet(GR_STRIDE));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(GR_DST));
    f.instruction(&Ins::LocalGet(GR_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(GR_SRC));
    f.instruction(&Ins::LocalGet(GR_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(GR_W));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(GR_W));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // word loop
    f.instruction(&Ins::End); // word block

    f.instruction(&Ins::LocalGet(GR_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(GR_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // slat loop
    f.instruction(&Ins::End); // slat block

    store_desc_i32(&mut f, GR_D, B_BASE, |f| {
        f.instruction(&Ins::LocalGet(GR_NEW_BASE));
    });
    store_desc_i32(&mut f, GR_D, B_CAP, |f| {
        f.instruction(&Ins::LocalGet(GR_OLD_CAP));
        f.instruction(&Ins::I32Const(2));
        f.instruction(&Ins::I32Mul);
    });
    store_desc_i32(&mut f, GR_D, B_HEAD, |f| {
        f.instruction(&Ins::I32Const(0));
    });

    f.instruction(&Ins::End);
    f
}

// `b_push_back(d: i32)`
const PB_D: u32 = 0;
const PB_ADDR: u32 = 1;
const PB_IDX: u32 = 2;

/// `b_push_back(d)`: append one EMPTY slat at the entry, doubling the ring first
/// when it is full -- `self.slats.push_back(Slat::empty(n_leaks))`.
fn emit_push_back(grow: u32, zero_tail: u32) -> Function {
    let mut f = Function::new([(2, ValType::I32)]);

    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_CAP)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::Call(grow));
    f.instruction(&Ins::End);

    // The new slat's logical index is the old `len`, so `push_slat_addr` addresses
    // it directly (the ring has room: the `grow` above guarantees `len < cap`).
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::LocalSet(PB_IDX));
    push_slat_addr(&mut f, PB_D, PB_IDX);
    f.instruction(&Ins::LocalTee(PB_ADDR));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(PB_D));
    f.instruction(&Ins::LocalGet(PB_ADDR));
    f.instruction(&Ins::Call(zero_tail));

    store_desc_i32(&mut f, PB_D, B_LEN, |f| {
        f.instruction(&Ins::LocalGet(PB_D));
        f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
        f.instruction(&Ins::I32Const(1));
        f.instruction(&Ins::I32Add);
    });

    f.instruction(&Ins::End);
    f
}

// `b_grow_to_d(d: i32)`
const GD_D: u32 = 0;

/// `b_grow_to_d(d)`: `while self.slats.len() < d { push_back(empty) }` -- the belt
/// extends with empty slats behind existing material when the entry depth grows
/// (§6.2), and, in the steady state, simply replaces the slat the shift popped.
fn emit_grow_to_d(push_back: u32) -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(GD_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::LocalGet(GD_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_D)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(GD_D));
    f.instruction(&Ins::Call(push_back));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

// `b_add_at(d: i32, i: i32, v: f64)`
const AA_D: u32 = 0;
const AA_I: u32 = 1;
const AA_V: u32 = 2;
const AA_ADDR: u32 = 3;

/// `b_add_at(d, i, v)`: `slats[i].content += v` -- the merge of an inserted cohort
/// into whatever the entry slat already holds (§6.2: content sums).
fn emit_add_at() -> Function {
    let mut f = Function::new([(1, ValType::I32)]);

    push_slat_addr(&mut f, AA_D, AA_I);
    f.instruction(&Ins::LocalTee(AA_ADDR));
    f.instruction(&Ins::LocalGet(AA_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(AA_V));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::End);
    f
}

// `b_clone_ring(d: i32)`
const CR_D: u32 = 0;
const CR_OLD: u32 = 1;
const CR_WORDS: u32 = 2;
const CR_NEW: u32 = 3;
const CR_I: u32 = 4;

/// `b_clone_ring(d)`: bump-allocate a `cap`-slat region, copy the ring verbatim
/// (word for word -- `head`/`len`/`cap` are unchanged, so ring order is preserved),
/// and repoint `base` at the copy.
///
/// The preview pass then mutates only the copy. The caller restores the saved
/// descriptor and rewinds `G_HEAP` afterwards, so both the copy and any doubling
/// the preview triggered are reclaimed.
fn emit_clone_ring(alloc: u32) -> Function {
    let mut f = Function::new([(4, ValType::I32)]);

    load_desc_i32(&mut f, CR_D, B_BASE, CR_OLD);
    // words = cap * stride / 8
    f.instruction(&Ins::LocalGet(CR_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_CAP)));
    f.instruction(&Ins::LocalGet(CR_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_STRIDE)));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32DivS);
    f.instruction(&Ins::I32Mul);
    f.instruction(&Ins::LocalSet(CR_WORDS));

    f.instruction(&Ins::LocalGet(CR_WORDS));
    f.instruction(&Ins::Call(alloc));
    f.instruction(&Ins::LocalSet(CR_NEW));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(CR_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(CR_I));
    f.instruction(&Ins::LocalGet(CR_WORDS));
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

    store_desc_i32(&mut f, CR_D, B_BASE, |f| {
        f.instruction(&Ins::LocalGet(CR_NEW));
    });

    f.instruction(&Ins::End);
    f
}

#[cfg(test)]
#[path = "belt_tests.rs"]
mod tests;
