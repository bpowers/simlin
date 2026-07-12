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
//! ## Scope: everything but §8 spread inputs
//!
//! Steps 1-3 of GH #922 lower `docs/design/conveyors.md` §4-§7 and §11 in full: the
//! DT-quantized slat deque that advances one slat per step, discharging its exit
//! slat as the primary outflow and admitting equation-driven inflow subject to
//! `<capacity>` and `<in_limit>`, plus unconditionally-admitted conveyor-driven
//! inflow (§4.3 step 4); all of §5 (linear and exponential leakage, path-based leak
//! zones, `<leak_integers/>`, `ignore_earlier_zone_losses`); the §7.1 steady and
//! §7.2 explicit-list init fills; `<sample>`, `<arrest>`, and the held-exit rule
//! (§4.3 steps 0-3, §6.1); the whole discrete belt (§6.3/§6.4: per-time-unit
//! `in_limit` budget, admission quantization, time-unit block lumping); and the
//! queue-coupled inflow half of §11. GH #923 adds §10 container access, published at
//! STEP START from a hook point distinct from the pass proper -- see
//! [`ConveyorPass::emit_publish_containers`].
//!
//! [`reject_unsupported`] still refuses, loudly, one FEATURE:
//!
//! * **non-`beginning` inflow placements** (`isee:spreadflow` = `even`/`dest`/
//!   `dist`/`source`, §8) -- GH #946. See [`emit_insert`]'s docs for why widening
//!   the current single-slat insert is not a matter of deleting the reject.
//!
//! Belt GROWTH and SHRINK (§6.2) were already lowered in step 1, because
//! `<sample>` defaults to 1: a belt with no `<sample>` expression re-latches its
//! transit time from `<len>` on *every* step, and a `<len>` that is an expression
//! (`test/conveyors/arrayed_conveyor.xmile` names an aux) can therefore change the
//! entry depth mid-run. There is no plan-level signal that distinguishes a
//! constant-valued `<len>` expression from a time-varying one, so a lowering that
//! assumed a fixed belt length would *silently* mis-simulate the second. The ring
//! below implements the general geometry instead; `<sample>` only decides *whether*
//! a given step re-latches.
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
//! +24 c0:       f64 phase A's start-of-step total (`ConveyorState::step_contents0`)
//! +32 out:      f64 phase A's exit volume, consumed by phase B
//! +40 in_carry: f64 the discrete per-time-unit `in_limit` budget spent (§6.3)
//! ```
//!
//! `c0`/`out` live in memory rather than in `run_to` locals because phase A runs
//! over EVERY belt before phase B runs over any (§4.3: "no phase reads another
//! conveyor's same-phase results"), so the per-belt phase A results must outlive
//! the unrolled phase A loop, and their count is plan-dependent. `in_carry` is
//! genuine cross-step belt state (`ConveyorState::in_carry`), zeroed at belt init,
//! reset at each integer time boundary, and -- because it sits inside the
//! descriptor -- saved and restored by the mid-run preview along with the rest of
//! it. The remaining cross-step state that does NOT ride the descriptor or the ring
//! is enumerated on [`ConveyorPass::emit_preview_save`].
//!
//! `stride` is `8 * (1 + 2 * n_leaks + n_int_leaks)` bytes -- `conveyor.rs`'s
//! `Slat` (one f64 of content plus, per leak flow, a `leak_basis` and a
//! `leak_window`), followed by one scratch word per INTEGER leak flow. That
//! trailing scratch is `leak_step`'s `shed_by` row: `quantize_integer_leaks` undoes
//! an integer flow's continuous shed before re-removing whole units, so the
//! per-`(slat, flow)` shed must survive the priority-ordered shed loop. It rides in
//! the slat rather than in a parallel buffer because the ring already copies,
//! zeroes, and clones whole strides. It is written before it is read on every step
//! (the shed loop visits every live slat), so its bump-allocated garbage is never
//! observable. `stride` is PLAN-DERIVED and lives in the descriptor, so the shared
//! ring helpers stride generically.
//!
//! The rings live in the bump region past the end of the static layout, addressed
//! by `passes::G_HEAP`. A ring holds `cap` slats and grows by DOUBLING into a
//! fresh bump allocation, abandoning the old region until the next `reset` rewinds
//! the bump pointer wholesale -- exactly the queue pass's discipline, and for the
//! same reasons (see `passes`'s module docs).
//!
//! Alongside the descriptors, nine parallel arrays indexed by a belt's GLOBAL leak
//! index carry the leak state and scratch ([`LeakRegions`]). Only `leak_carry` is
//! persistent (§5.4); the rest is recomputed each step, and every one of them is
//! plan-sized, so a leak-free model reserves nothing.
//!
//! ## Rust-vs-wasm float semantics
//!
//! Three divergences, each handled the same way -- never by the wasm instruction
//! whose name matches the Rust method:
//!
//! * `f64::max(NaN, 0.0) == 0.0` in Rust, while wasm's `f64.max` propagates NaN.
//!   The belt's `rate.max(0.0)` admission clamp (`conveyor.rs:935`), its
//!   `(...).max(0.0)` capacity-room clamp (`conveyor.rs:899`), and the two
//!   retained-profile clamps (`conveyor.rs:445`, `conveyor.rs:487`) all depend on
//!   the Rust behavior, so all lower to `passes::emit_clamp_nonneg`'s
//!   compare-and-`select`.
//! * `f64::min(NaN, x) == x` in Rust, while wasm's `f64.min` propagates NaN. §5.1's
//!   `basis.min(window)` / `(f * use).min(content)` and §5.4's
//!   `remaining.min(content)` therefore go through the `b_fmin` helper. `f64.min`
//!   IS used raw for the admission `min(req, cap_room, limit_vol)` chain, whose
//!   operands are provably non-NaN (see [`ConveyorPass::emit_phase_b`]).
//! * When the operands compare equal, Rust's `f64::min`/`f64::max` may return
//!   EITHER (its docs say so explicitly for `+0.0`/`-0.0`), so the sign of a zero
//!   coming out of `b_fmin` (which keeps wasm's `-0.0`) or out of the exponential
//!   leak-rate clamp is not pinned to the VM's. No belt observable depends on it --
//!   the values are only summed into `+0.0`-seeded accumulators, subtracted, and
//!   compared -- and `belt_tests`' `EPS` exists so it can never become a flake.

use wasm_encoder::{BlockType, Function, Instruction as Ins, ValType};

use crate::conveyor_compile::{ContainerKind, ConveyorPlan};

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
const B_IN_CARRY: u64 = 40;

/// Leak flows on `plan`, the width of every per-leak array below.
fn n_leaks(plan: &ConveyorPlan) -> usize {
    plan.leaks.len()
}

/// Per-slat `shed_by` scratch words: one per INTEGER leak flow, and none at all on
/// an exponential belt.
///
/// `leak_step` returns from its exponential arm before ever looking at
/// `<leak_integers/>`, so an exponential+integer flow leaks continuously in the VM.
/// Mirroring that here (rather than "fixing" it) keeps the two backends identical;
/// the divergence from `docs/design/conveyors.md` §5.4, which does not restrict
/// integer leakage to the linear model, is tracked as GH #942 -- a VM question, not
/// papered over. If #942 resolves by rejecting the combination upstream, this gate
/// becomes dead and should be deleted along with the reasoning above.
fn shed_words(plan: &ConveyorPlan) -> usize {
    if plan.exponential_leak {
        0
    } else {
        plan.leaks.iter().filter(|l| l.integers).count()
    }
}

/// f64 words per slat for a plan with `n_leaks` leak flows: `content`, a
/// `leak_basis`/`leak_window` pair per flow (`conveyor.rs:141-149`), and one
/// `shed_by` scratch word per integer flow (see [`shed_words`]).
fn slat_words(plan: &ConveyorPlan) -> u32 {
    (1 + 2 * n_leaks(plan) + shed_words(plan)) as u32
}

/// Byte offset of `leak_basis[k]` within a slat.
fn basis_off(k: usize) -> u64 {
    (1 + k) as u64 * SLOT_BYTES as u64
}

/// Byte offset of `leak_window[k]` within a slat of an `n`-leak belt.
fn window_off(n: usize, k: usize) -> u64 {
    (1 + n + k) as u64 * SLOT_BYTES as u64
}

/// Byte offset of the `j`-th integer flow's `shed_by` scratch word within a slat of
/// an `n`-leak belt.
fn shed_off(n: usize, j: usize) -> u64 {
    (1 + 2 * n + j) as u64 * SLOT_BYTES as u64
}

/// Position of leak `k` among `plan`'s INTEGER flows (its `shed_by` column), or
/// `None` when flow `k` leaks continuously.
fn int_index(plan: &ConveyorPlan, k: usize) -> Option<usize> {
    if plan.exponential_leak || !plan.leaks[k].integers {
        return None;
    }
    Some(plan.leaks[..k].iter().filter(|l| l.integers).count())
}

/// The slot offsets of `plan`'s EQUATION-driven inflows, in listed order -- the
/// `eq_request_rates` of `conveyor_phase_b_one`.
///
/// The split is the VM's: an inflow is unconditionally admitted (and so lands in
/// `conv_inflows`, bypassing capacity, the inflow limit, and discrete quantization)
/// when it is `conveyor_driven` -- an upstream belt's driven outflow -- OR
/// `queue_coupled` -- a queue's primary outflow whose volume the combined pass has
/// already sized and written (§11). Everything else is a request this belt clears.
///
/// Spelled ONCE because three sites index by position into the same scratch region:
/// [`ConveyorPass::scratch_slots`] sizes it, [`ConveyorPass::state_region_bytes`]
/// sizes the parallel `quant_carry` row, and `emit_phase_b` writes both.
fn eq_inflow_offsets(plan: &ConveyorPlan) -> Vec<usize> {
    plan.inflows
        .iter()
        .filter(|inf| !inf.conveyor_driven && !inf.queue_coupled)
        .map(|inf| inf.flow_off)
        .collect()
}

/// The `<arrest>` slot of the conveyor `plan`'s primary outflow feeds, if that
/// destination is a belt that can be arrested at all (§4.3 step 3's held exit).
/// `None` -- a compile-time "never held" -- when the primary outflow leaves the
/// belt system, or when the destination belt carries no `<arrest>` expression.
fn dest_arrest_off(plans: &[ConveyorPlan], plan: &ConveyorPlan) -> Option<usize> {
    plans[plan.primary_dest_conveyor?].arrest_off
}

/// Per leak flow of `plan`: the `<arrest>` slot of the belt that flow feeds, if any
/// (§4.3 step 2's arrested-leak-destination skip). Aligned with `plan.leaks`.
fn leak_arrest_offs(plans: &[ConveyorPlan], plan: &ConveyorPlan) -> Vec<Option<usize>> {
    plan.leaks
        .iter()
        .map(|l| l.dest_conveyor.and_then(|d| plans[d].arrest_off))
        .collect()
}

/// Bytes the [`LeakRegions`] arrays occupy per (belt, leak flow) pair: six f64
/// arrays and three i32 arrays.
const LEAK_BYTES_PER_LEAK: u32 = 6 * 8 + 3 * 4;

/// The nine parallel per-leak arrays, each indexed by a GLOBAL leak index
/// (`plan_leak_base[belt] + k`). Parallel arrays rather than descriptor fields,
/// because a belt's leak count is plan-dependent while the descriptor is not.
///
/// Only `carry` outlives a step. `r`/`sheds`/`ub`/`m_entry`/`first_zone`/`prefix`
/// are single-use scratch, live for one helper call; `lv` spans one belt's phase A
/// (which fills it) and its phase B (which sums it into `leaked`). Distinct arrays
/// rather than one aliased scratch region: the aliasing would be sound today only
/// because of a call-graph argument nothing enforces.
#[derive(Clone, Copy)]
struct LeakRegions {
    /// Byte address of the first array.
    base: u64,
    /// Σ over plans of `plan.leaks.len()`; the stride between arrays.
    total: u64,
}

impl LeakRegions {
    /// This DT's `leak_vols[k]` (`leak_step`'s accumulator, `conveyor.rs:733`).
    fn lv(self, g: usize) -> u64 {
        self.base + 8 * g as u64
    }
    /// `ConveyorState::leak_carry[k]`: the `<leak_integers/>` fractional-unit
    /// accumulator (§5.4). Zeroed at belt init, never reset thereafter.
    fn carry(self, g: usize) -> u64 {
        self.base + 8 * self.total + 8 * g as u64
    }
    /// The mid-run preview's save slot for [`Self::carry`]. The VM's preview clones
    /// the whole `Vec<ConveyorState>` (`vm.rs:1196`), `leak_carry` included, so the
    /// blob must roll it back too -- the ring clone alone would not.
    fn carry_save(self, g: usize) -> u64 {
        self.base + 16 * self.total + 8 * g as u64
    }
    /// `zone_start_retained`'s `r[k]` (`conveyor.rs:408`).
    fn r(self, g: usize) -> u64 {
        self.base + 24 * self.total + 8 * g as u64
    }
    /// Exponential leakage's per-slat `sheds[k]` scratch (`conveyor.rs:752`).
    fn sheds(self, g: usize) -> u64 {
        self.base + 32 * self.total + 8 * g as u64
    }
    /// An init fill's `unit_basis[k]` (`conveyor.rs:466`/`conveyor.rs:621`).
    fn ub(self, g: usize) -> u64 {
        self.base + 40 * self.total + 8 * g as u64
    }
    /// `M_k(depth)`: the in-zone slat count over the entry path (§5.1). i32.
    fn m_entry(self, g: usize) -> u64 {
        self.base + 48 * self.total + 4 * g as u64
    }
    /// The deepest in-zone entry-path slat index, or -1 (`conveyor.rs:422`). i32.
    fn first_zone(self, g: usize) -> u64 {
        self.base + 52 * self.total + 4 * g as u64
    }
    /// The running `M_k(i + 1)` prefix count during an init fill. i32.
    fn prefix(self, g: usize) -> u64 {
        self.base + 56 * self.total + 4 * g as u64
    }
}

/// The f64 scratch locals the belt pass needs in its enclosing function (`run_to`),
/// plus the one i64 it needs for the discrete time-unit clock. Kept as a struct so
/// `module.rs` owns the local numbering.
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
    /// The UNDRAWN capacity room (`admission_room`'s `cap_room`), which a discrete
    /// belt's `discrete_admit` needs after `rem_cap` has been drawn down.
    pub cap_room: u32,
    /// `discrete_admit`'s shared `floor(cap_room)` whole-unit budget (§6.4 rule 1).
    pub budget: u32,
    /// One inflow's admitted whole units (`discrete_admit`'s `units`).
    pub units: u32,
    /// A second single-use scratch, needed where `tmp` is already live.
    pub tmp2: u32,
    /// §11 coupling: the volume earlier-served coupled queues already committed to
    /// this belt this DT (`run_coupled_passes`' `prior_coupled_vol`).
    pub prior: u32,
    /// §11 coupling: `coupled_admission_budget`'s `req`.
    pub req: u32,
    /// §11 coupling: `QueueState::conveyor_desire`.
    pub desire: u32,
    /// §11 coupling: `QueueState::take_for_conveyor`'s result.
    pub taken: u32,
    /// i64: the integer time unit of this step (`conveyor_time_unit`), compared
    /// against the persistent `last_unit` to drive the §6.3 budget reset.
    pub unit: u32,
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
    /// First byte of the nine per-leak arrays ([`LeakRegions`]).
    pub leak_base: u32,
    /// First byte of the pass-global clock + per-inflow quantization carries
    /// ([`StateRegions`]). Must be 8-byte aligned (it opens with an `i64`).
    pub state_base: u32,
}

/// The discrete belt's cross-step state that lives in neither the descriptor nor a
/// slat ring, plus its mid-run-preview save area (`vm.rs:1196-1198` clones the whole
/// `Vec<ConveyorState>` AND `conveyor_last_unit`, so both must roll back).
///
/// ```text
/// +0        last_unit:      i64  Vm::conveyor_last_unit (§6.3)
/// +8        last_unit_save: i64
/// +16       quant_carry[q]: f64  ConveyorState::quant_carry, one per (discrete
///                                belt, equation-driven inflow) pair (§6.4 rule 1)
/// +16+8q    quant_carry_save[q]: f64
/// ```
///
/// `last_unit` is pass-global rather than per-belt because the VM keeps it on the
/// `Vm`, not on a `ConveyorState`: one clock crossing resets *every* belt's budget.
#[derive(Clone, Copy)]
struct StateRegions {
    base: u64,
    /// Σ over DISCRETE plans of the equation-driven inflow count.
    quant: u64,
}

impl StateRegions {
    /// `Vm::conveyor_last_unit`, as an `i64` at a static address.
    fn last_unit(self) -> u64 {
        self.base
    }
    fn last_unit_save(self) -> u64 {
        self.base + 8
    }
    /// `ConveyorState::quant_carry[j]` of the belt whose global quant base is `g`.
    fn quant_carry(self, g: usize) -> u64 {
        self.base + 16 + 8 * g as u64
    }
    fn quant_carry_save(self, g: usize) -> u64 {
        self.base + 16 + 8 * self.quant + 8 * g as u64
    }
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
    /// `b_fmin(a, b) -> f64` -- Rust's `f64::min` (NaN-ignoring), not `f64.min`.
    /// Emitted for EVERY belt model, not only a leaky one: the discrete
    /// `units.min(budget)` needs the same NaN discipline as §5.1's `min`s.
    fmin: u32,
    /// `b_merge_front(d)` -- the held-exit shift's slat-0-into-slat-1 merge (§4.3
    /// step 5). `None` when no belt's primary destination can ever be arrested.
    merge_front: Option<u32>,
    /// `b_merge_blocks(d)` -- `merge_time_unit_blocks` (§6.4 rule 3). `None` unless
    /// some belt is discrete.
    merge_blocks: Option<u32>,
    /// `b_init_explicit_disc(d, table, m)` -- the §7.2 fill with
    /// `spread_per_time_unit`'s DISCRETE arm (whole block value at the deepest slat).
    /// `None` unless some discrete belt carries an init list.
    init_explicit_disc: Option<u32>,
    /// `b_round(x) -> f64` -- Rust's `f64::round` (half AWAY from zero), which is
    /// NOT wasm's `f64.nearest` (half to EVEN). `None` unless some belt is discrete.
    round: Option<u32>,
    /// `b_sat_i64(x) -> i64` -- Rust's saturating `f64 as i64`. `None` unless some
    /// belt is discrete.
    sat_i64: Option<u32>,
    /// The §10 container reducers, each `(d: i32) -> f64`, present only when a
    /// container variable of that kind reads some belt; `slat_at` is
    /// `(d: i32, j: i32) -> f64`. A container-free conveyor model carries none of
    /// them. `SUM` needs no entry -- [`emit_total`] already IS
    /// `slat_contents().iter().sum()` -- and `SIZE` is a single `i32.load` +
    /// `f64.convert_i32_s` at the call site.
    mean: Option<u32>,
    min: Option<u32>,
    max: Option<u32>,
    stddev: Option<u32>,
    slat_at: Option<u32>,
}

/// The 0-based ring index a 1-based `conv[j]` container access reads, or `None` when
/// `j` can never name a slat of any belt -- which is a compile-time constant NaN,
/// not a runtime test.
///
/// Two ways to be nameless. `j == 0` is out of range by definition (the index is
/// 1-based). And a `j` whose 0-based form exceeds `i32::MAX` cannot address a slat
/// either: [`reject_unsupported`] refuses a `slat_bound()` above `i32::MAX`, so every
/// belt's `len` fits in an i32 and `j - 1 > i32::MAX >= len` always. Narrowing such a
/// `j` with `as i32` instead would WRAP -- `conv[4294967297]` would read slat 0 --
/// which is exactly the wrong answer where `container_value_from_slice`'s
/// `vec.get(j - 1)` returns `None`, hence NaN.
fn slat_index(j: usize) -> Option<i32> {
    i32::try_from(j.checked_sub(1)?).ok()
}

/// Function indices of the leak primitives whose bodies are plan-INDEPENDENT: the
/// zone bounds ride in as `f64` parameters, so one copy serves every leak flow.
/// Emitted only for a model that leaks, and threaded straight into the
/// plan-specialized emitters at build time -- nothing needs them afterwards.
#[derive(Clone, Copy)]
struct ZoneFns {
    /// `b_fmin(a, b) -> f64` -- Rust's `f64::min` (NaN-ignoring), not `f64.min`.
    fmin: u32,
    /// `b_in_zone(i, length, zs, ze) -> i32` -- `ConveyorState::in_zone` (§5.3).
    in_zone: u32,
    /// `b_zone_count(length, depth, zs, ze) -> i32` -- `zone_count_from`, i.e.
    /// `M_k(depth)`.
    zone_count: u32,
    /// `b_first_zone(length, depth, zs, ze) -> i32` -- the DEEPEST in-zone entry-path
    /// slat, or -1 when the zone misses the entry path entirely.
    first_zone: u32,
    /// `b_zero_tail(d, addr)`, needed by the plan-specialized init fills.
    zero_tail: u32,
    /// `b_total(d) -> f64`, the leak-aware steady fill's `S = Σ c[i]`.
    total: u32,
}

/// Function indices of the PLAN-SPECIALIZED leak emitters. A plan's zone bounds,
/// leak-fraction slots, `<leak_integers/>` mask, and leak model are compile-time
/// constants, so each of these is emitted once per leaky belt with those constants
/// folded in -- the same unrolled, plan-specialized principle the rest of the pass
/// follows. All are `None` for a leak-free belt, whose (already-emitted) generic
/// primitives are exact.
/// (`b_retained_{i}` is not listed: nothing calls it from outside the four below,
/// which capture its index at build time.)
#[derive(Clone, Copy, Default)]
struct BeltPlanFns {
    /// `b_leak_{i}(d)` -- `leak_step` (§4.3 step 2): mutates the belt and leaves the
    /// per-flow leaked volumes in [`LeakRegions::lv`].
    leak: Option<u32>,
    /// `b_insert_{i}(d, share)` -- the entry slat's `content += share` plus the §5.1
    /// cohort schedule (`phase_b`'s insert arm).
    insert: Option<u32>,
    /// `b_init_steady_{i}(d, v)` -- the leak-aware §7.1 steady fill.
    init_steady: Option<u32>,
    /// `b_sched_{i}(d)` -- `fill_slats`' schedule half, applied after the generic
    /// §7.2 content fill.
    sched: Option<u32>,
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
    /// The run's start time, likewise a compile-time constant. `conveyor_time_unit`
    /// recovers the ideal grid time from it rather than trusting the drifted clock.
    start: f64,
    /// Byte offset (within the init-table region) and entry count of each plan's
    /// §7.2 list, aligned with `plans`. `None` for a §7.1 steady-fill belt.
    init_tables: Vec<Option<(u32, u32)>>,
    /// The per-leak arrays' addressing.
    leaks: LeakRegions,
    /// Global leak index of plan `i`'s first leak flow, aligned with `plans`.
    leak_base_idx: Vec<usize>,
    /// Plan-specialized leak emitters, aligned with `plans`.
    plan_fns: Vec<BeltPlanFns>,
    /// The clock + quantization-carry addressing.
    state: StateRegions,
    /// Global quantization-carry index of plan `i`'s first equation-driven inflow,
    /// aligned with `plans`; `None` for a continuous belt (whose `quant_carry` the
    /// VM never even allocates -- `discrete_admit` is the only place it is sized).
    quant_base_idx: Vec<Option<usize>>,
}

/// Reject every conveyor feature this module does not lower, loudly.
///
/// The wasm backend has no silent VM fallback, so a feature this module does not
/// lower must surface as [`WasmGenError::Unsupported`] rather than as a blob whose
/// belts quietly ignore it.
///
/// After GH #923 exactly TWO conditions remain -- one missing FEATURE and one
/// soundness guard on the emitted narrowings:
///
/// * **Inflows carrying a non-default `isee:spreadflow`** (§8: `even`, `dest`,
///   `dist`, `source`). GH #946. This is
///   NOT a matter of deleting the arm: [`emit_insert`] folds the VM's per-slat
///   running in-zone prefix `m_own[k]` into the constant `m_entry[k]`, which is
///   exact only because a `beginning` cohort lands whole on the entry slat. Every
///   other placement spreads shares over `i < d - 1`, where the two differ (see
///   that function's STEP-3 TRAP note). Lowering them needs a per-belt, ring-sized
///   `shares` scratch buffer that grows and clones with the ring, a per-step weight
///   vector for `dist`, and -- for `source` -- a per-`(belt, leak flow)` snapshot of
///   phase A's per-slat leak detail that survives the UPSTREAM belt's own phase B
///   (`conveyor_compile::conv_inflow_placement` reads `pa[u].leak_slat_vols[k]`
///   after belt `u < i` has already popped and grown its deque). None of that is a
///   reject to remove; it is a memory-layout feature to add.
/// * **A slat bound above `i32::MAX`.** Not a feature gap: `b_slat_count`'s result is
///   narrowed with `i32.trunc_f64_s`, which TRAPS outside i32's range, and the §4.1
///   bound check that precedes it makes the narrowing sound only if the bound ITSELF
///   fits. Production's 1,000,000 and every test `SlatBoundGuard` do; the narrowing
///   must not silently depend on a constant it never checks. This arm survives GH #946
///   and has no issue to close it. [`slat_index`] leans on it too.
///
/// Container access (§10), `queue_coupled` inflows, `<sample>`, `<arrest>`,
/// `primary_dest_conveyor`, a leak flow's `dest_conveyor`, and discrete belts all
/// deliberately pass now.
pub(super) fn reject_unsupported(plans: &[ConveyorPlan]) -> Result<(), WasmGenError> {
    let unsupported = |what: &str| {
        Err(WasmGenError::Unsupported(format!(
            "wasmgen: {what} is not yet supported by the wasm backend; \
             the bytecode VM is the only backend that simulates it today"
        )))
    };
    for plan in plans {
        for inf in &plan.inflows {
            // All three disjuncts are load-bearing: `dist` and `source` carry
            // `Placement::Beginning` as their degenerate fallback (see
            // `conveyor_compile::InflowMeta::placement`), so testing `placement`
            // alone would silently admit them and lower them as plain `beginning`
            // inserts.
            if inf.source
                || inf.dist.is_some()
                || inf.placement != crate::conveyor::Placement::Beginning
            {
                return unsupported("a conveyor inflow with a non-default isee:spreadflow");
            }
        }
    }
    // The second rejection condition (see this function's rustdoc): the emitted
    // `i32.trunc_f64_s` narrowings -- and `slat_index`'s constant-NaN reasoning -- are
    // sound only if the slat bound itself fits in i32.
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
        start: f64,
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
            emit_init_explicit(dt, zero_tail, false),
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
        let fmin = push(
            vec![ValType::F64, ValType::F64],
            vec![ValType::F64],
            emit_fmin(),
        );

        // The held-exit merge exists only for a belt whose primary destination is a
        // conveyor that CAN be arrested. Nothing else can hold an exit (§4.3 step 3).
        let merge_front = plans
            .iter()
            .any(|p| dest_arrest_off(plans, p).is_some())
            .then(|| push(vec![ValType::I32], vec![], emit_merge_front(pop_front)));

        // The discrete primitives. `merge_blocks` reads its `dt` from the emitter's
        // closure, `round`/`sat_i64` are pure scalar functions, and
        // `init_explicit_disc` is only reachable from a discrete belt's §7.2 fill.
        let any_discrete = plans.iter().any(|p| p.discrete);
        let merge_blocks =
            any_discrete.then(|| push(vec![ValType::I32], vec![], emit_merge_blocks(dt)));
        let init_explicit_disc = plans
            .iter()
            .any(|p| p.discrete && p.init_values.is_some())
            .then(|| {
                push(
                    vec![ValType::I32, ValType::I32, ValType::I32],
                    vec![],
                    emit_init_explicit(dt, zero_tail, true),
                )
            });
        let round =
            any_discrete.then(|| push(vec![ValType::F64], vec![ValType::F64], emit_round()));
        let sat_i64 =
            any_discrete.then(|| push(vec![ValType::F64], vec![ValType::I64], emit_sat_i64()));

        // The §10 container reducers, emitted only for the kinds this model's container
        // variables actually use -- a container-free conveyor model carries none.
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
        // A `conv[j]` whose `j` names no slat of any belt is a constant NaN at the call
        // site ([`slat_index`]), so it needs no helper -- the gate must agree.
        let slat_at = uses(|k| matches!(k, ContainerKind::Slat(j) if slat_index(*j).is_some()))
            .then(|| {
                push(
                    vec![ValType::I32, ValType::I32],
                    vec![ValType::F64],
                    emit_slat_at(),
                )
            });

        // The leak primitives, emitted only for a model that leaks. `zone` is the
        // single gate: every plan-specialized emitter below needs all four, and the
        // `expect` in the loop is discharged by `total_leaks > 0` implying some plan
        // has a leak flow.
        let total_leaks: usize = plans.iter().map(n_leaks).sum();
        let zone = (total_leaks > 0).then(|| {
            let zone_params = || vec![ValType::I32, ValType::I32, ValType::F64, ValType::F64];
            let in_zone = push(zone_params(), vec![ValType::I32], emit_in_zone());
            let zone_count = push(zone_params(), vec![ValType::I32], emit_zone_count(in_zone));
            let first_zone = push(zone_params(), vec![ValType::I32], emit_first_zone(in_zone));
            ZoneFns {
                fmin,
                in_zone,
                zone_count,
                first_zone,
                zero_tail,
                total,
            }
        });

        let leaks = LeakRegions {
            base: u64::from(layout.leak_base),
            total: total_leaks as u64,
        };
        let mut leak_base_idx = Vec::with_capacity(plans.len());
        let mut plan_fns = Vec::with_capacity(plans.len());
        let mut g0 = 0usize;
        for plan in plans {
            leak_base_idx.push(g0);
            if n_leaks(plan) == 0 {
                plan_fns.push(BeltPlanFns::default());
                continue;
            }
            let zf = zone.expect("a leaky plan implies the zone primitives were emitted");
            let retained = push(
                vec![ValType::I32, ValType::I32],
                vec![],
                emit_retained(plan, g0, leaks, zf),
            );
            let leak = push(
                vec![ValType::I32],
                vec![],
                emit_leak(plan, g0, leaks, dt, zf, &leak_arrest_offs(plans, plan)),
            );
            let insert = push(
                vec![ValType::I32, ValType::F64],
                vec![],
                emit_insert(plan, g0, leaks, zf, retained),
            );
            let init_steady = push(
                vec![ValType::I32, ValType::F64],
                vec![],
                emit_init_steady_leaky(plan, g0, leaks, dt, zf, retained),
            );
            let sched = push(
                vec![ValType::I32],
                vec![],
                emit_sched(plan, g0, leaks, zf, retained),
            );
            plan_fns.push(BeltPlanFns {
                leak: Some(leak),
                insert: Some(insert),
                init_steady: Some(init_steady),
                sched: Some(sched),
            });
            g0 += n_leaks(plan);
        }

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

        // Only a DISCRETE belt reaches `discrete_admit`, the sole place the VM sizes
        // `quant_carry`; a continuous belt's stays a zero-length `Vec` forever. Give
        // exactly those belts a carry row, so a continuous-only model reserves nothing.
        let mut quant_base_idx = Vec::with_capacity(plans.len());
        let mut q0 = 0usize;
        for plan in plans {
            if plan.discrete {
                quant_base_idx.push(Some(q0));
                q0 += eq_inflow_offsets(plan).len();
            } else {
                quant_base_idx.push(None);
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
                fmin,
                merge_front,
                merge_blocks,
                init_explicit_disc,
                round,
                sat_i64,
                mean,
                min,
                max,
                stddev,
                slat_at,
            },
            dt,
            start,
            init_tables,
            leaks,
            leak_base_idx,
            plan_fns,
            state: StateRegions {
                base: u64::from(layout.state_base),
                quant: q0 as u64,
            },
            quant_base_idx,
        }
    }

    /// Bytes the nine per-leak arrays ([`LeakRegions`]) occupy for this plan set.
    pub(super) fn leak_region_bytes(plans: &[ConveyorPlan]) -> u32 {
        (plans.iter().map(n_leaks).sum::<usize>() as u32) * LEAK_BYTES_PER_LEAK
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
    /// Both this and `emit_phase_b`'s admission loop index the region by position, so
    /// both go through [`eq_inflow_offsets`] -- the single place the VM's
    /// `!(conveyor_driven || queue_coupled)` split is spelled. A narrower region than
    /// the loop's index range would scribble past it.
    pub(super) fn scratch_slots(plans: &[ConveyorPlan]) -> u32 {
        plans
            .iter()
            .map(|p| eq_inflow_offsets(p).len() as u32)
            .max()
            .unwrap_or(0)
    }

    /// Bytes the clock + quantization-carry region ([`StateRegions`]) occupies: the
    /// two `i64` clock words, then a live and a preview-save `f64` per (discrete
    /// belt, equation-driven inflow) pair.
    pub(super) fn state_region_bytes(plans: &[ConveyorPlan]) -> u32 {
        let quant: u32 = plans
            .iter()
            .filter(|p| p.discrete)
            .map(|p| eq_inflow_offsets(p).len() as u32)
            .sum();
        16 + 16 * quant
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
    /// reconciling against the belt-derived values (`vm.rs:1616-1628`). Two kinds
    /// qualify, and the VM collects exactly these two:
    ///
    /// * every published container slot (§10), so `INIT(SUM(belt))` reads the
    ///   start-of-run slat total rather than the hidden stock's frozen `0`
    ///   placeholder;
    /// * a §7.2 list-initialized conveyor stock, whose slot `init_belts` overwrites
    ///   with the normalized belt total -- re-running the compiled placeholder
    ///   `<eqn>` would clobber it before dependent initials read it.
    pub(super) fn reconcile_skip_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.container_offsets().chain(
            self.plans
                .iter()
                .filter(|p| p.init_values.is_some())
                .map(|p| p.stock_off),
        )
    }

    /// The slab offset of every container variable reading some belt (§10).
    pub(super) fn container_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.plans
            .iter()
            .flat_map(|p| p.containers.iter())
            .map(|c| c.off)
    }

    // ── the step-start container publish (a distinct hook point) ────────────

    /// Publish each belt's container-access results into their slab slots
    /// (`conveyor_compile::publish_container_values`, `conveyor_compile.rs:2922`).
    ///
    /// This runs at STEP START -- before the Flows phase -- and NOT between Flows and
    /// Stocks where the pass proper runs. The two hook points are distinct on purpose
    /// (`vm.rs:916` vs `vm.rs:958`): a container variable is a hidden no-flow stock,
    /// so the value published here is what a Flows-phase reader of `SUM(belt)` sees,
    /// and it must reflect the slats as the PREVIOUS step's pass left them. Publishing
    /// after Flows would feed every consumer the step-before-last's belt; publishing
    /// after the pass would save this step's row with the NEXT step's start state.
    ///
    /// Nothing here can raise: it reads a built ring and writes `curr`. What matters
    /// is that it never runs over an UNBUILT one, which the drivers' error guards
    /// secure (`emit_init` returns from `run_initials` before this on a raise).
    ///
    /// Each kind is a compile-time constant of the plan, so there is one open-coded
    /// reduction per container variable and no runtime dispatch. The only runtime
    /// loops are the reducers' walks over the belt's dynamic slat count.
    pub(super) fn emit_publish_containers(&self, f: &mut Function) {
        for (i, plan) in self.plans.iter().enumerate() {
            for c in &plan.containers {
                // `f64.store` consumes [addr_i32, value_f64]; every slab address folds
                // into the `memarg`, so the dynamic address is a constant 0.
                f.instruction(&Ins::I32Const(0));
                self.emit_container_value(f, i, &c.kind);
                f.instruction(&Ins::F64Store(memarg(slot_addr(c.off))));
            }
        }
    }

    /// Push one container value for belt `i`, reproducing
    /// `conveyor_compile::container_value_from_slice` (`conveyor_compile.rs:198`) over
    /// the belt's EXIT-FIRST slat-content vector -- the order `slat_contents()` yields
    /// and the order `push_slat_addr` walks, so `conv[1]` is the slat that discharges
    /// this step and `conv[len]` the entry slat.
    ///
    /// `SIZE` is the belt's physical length `len`, which is `>= 1` at every publish:
    /// `emit_init` allocates `slat_count() >= 1` slats and every phase B ends with
    /// `b_grow_to_d`, whose `d` is likewise `>= 1`. The empty-vector arms of the
    /// reducers (`NaN`, per `container_value_from_slice`) are therefore unreachable
    /// from a belt -- they are emitted anyway, because the lowering mirrors the VM's
    /// FUNCTION rather than the inputs that function happens to receive.
    fn emit_container_value(&self, f: &mut Function, i: usize, kind: &ContainerKind) {
        let d = self.desc_addr(i);
        match kind {
            ContainerKind::Slat(j) => match slat_index(*j) {
                // An index that can never name a slat: constant NaN, no helper call
                // and no runtime test. See [`slat_index`].
                None => {
                    f.instruction(&f64_const(f64::NAN));
                }
                Some(idx) => {
                    f.instruction(&Ins::I32Const(d));
                    f.instruction(&Ins::I32Const(idx));
                    f.instruction(&Ins::Call(
                        self.fns.slat_at.expect("the slat reader is emitted"),
                    ));
                }
            },
            ContainerKind::Size => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
                f.instruction(&Ins::F64ConvertI32S);
            }
            // `b_total` IS `vec.iter().sum()`: exit-first, seeded `+0.0`.
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
        // `Vm::run_initials` seeds `conveyor_last_unit = spec_start.floor() as i64`
        // (`vm.rs:1558`), so step 0 never spuriously fires the §6.3 budget reset.
        // `start` is a compile-time constant, so the saturating cast is too.
        if self.fns.sat_i64.is_some() {
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I64Const(sat_i64(self.start.floor())));
            f.instruction(&Ins::I64Store(i32_memarg(self.state.last_unit())));
        }

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

            // `ConveyorState::new` is where the never-resetting `<leak_integers/>`
            // carry, the discrete `in_carry`, and (via `discrete_admit`'s lazy sizing)
            // the `quant_carry` row all start at zero. The VM builds a fresh state per
            // belt on every `init_belts`, so a `reset` + re-run must not inherit the
            // last run's fractional units, spent budget, or fractional whole-unit dust.
            let g0 = self.leak_base_idx[i];
            for k in 0..n_leaks(plan) {
                store_static_f64(f, self.leaks.carry(g0 + k), |f| {
                    f.instruction(&f64_const(0.0));
                });
            }
            f.instruction(&Ins::I32Const(0));
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_IN_CARRY))));
            if let Some(q0) = self.quant_base_idx[i] {
                for j in 0..eq_inflow_offsets(plan).len() {
                    store_static_f64(f, self.state.quant_carry(q0 + j), |f| {
                        f.instruction(&f64_const(0.0));
                    });
                }
            }

            match self.init_tables[i] {
                Some((table_off, m)) => {
                    if m == 0 {
                        // A zero-entry list makes `spread_per_time_unit`'s `norm`
                        // return 0 for every block, i.e. a zero-filled belt --
                        // which the steady fill of a zero initial reproduces
                        // (`e = 0/N = 0`, and every leak schedule scales by `e`).
                        // Routing it here keeps `b_init_explicit`'s `m >= 1`
                        // precondition structural rather than assumed.
                        // (`probe_init_list` cannot produce an empty list, so this
                        // is unreachable defense.)
                        self.emit_steady_fill(f, i, |f| {
                            f.instruction(&f64_const(0.0));
                        });
                    } else {
                        let fill = if plan.discrete {
                            self.fns
                                .init_explicit_disc
                                .expect("a discrete plan with an init list emits the discrete fill")
                        } else {
                            self.fns.init_explicit
                        };
                        f.instruction(&Ins::I32Const(d));
                        f.instruction(&Ins::I32Const(
                            (self.layout.init_table_base + table_off) as i32,
                        ));
                        f.instruction(&Ins::I32Const(m as i32));
                        f.instruction(&Ins::Call(fill));
                        // `fill_slats` gives each filled slat the linear-leak
                        // schedule of an entry cohort that traveled to its position
                        // (§7.2); the generic fill above laid down only contents.
                        if let Some(sched) = self.plan_fns[i].sched {
                            f.instruction(&Ins::I32Const(d));
                            f.instruction(&Ins::Call(sched));
                        }
                        // `fill_slats` lumps a discrete belt's time-unit blocks AFTER
                        // hanging each slat's schedule on it, so the merge sums the
                        // schedules too (§6.4 rule 3). The zero-entry arm above routes
                        // through `emit_steady_fill`, which merges for itself.
                        self.emit_merge_if_discrete(f, i);
                    }
                    // `init_belts` writes the normalized belt total back into the
                    // stock slot. The expansion-time placeholder `<eqn>` already
                    // holds it (`normalized_init_total` runs this same fill, merge
                    // included), so this is defense in depth -- and
                    // `reconcile_skip_offsets` keeps the re-run initials from undoing
                    // it. It reads the belt AFTER the merge, as `init_belts` does.
                    f.instruction(&Ins::I32Const(0));
                    f.instruction(&Ins::I32Const(d));
                    f.instruction(&Ins::Call(self.fns.total));
                    f.instruction(&Ins::F64Store(memarg(slot_addr(plan.stock_off))));
                }
                None => {
                    let stock_off = plan.stock_off;
                    self.emit_steady_fill(f, i, |f| emit_load_curr(f, stock_off));
                }
            }
        }
    }

    /// `if self.discrete { self.merge_time_unit_blocks() }` -- the tail of both
    /// `init_steady` and `fill_slats` (§6.4 rule 3 / §7.1).
    fn emit_merge_if_discrete(&self, f: &mut Function, i: usize) {
        if !self.plans[i].discrete {
            return;
        }
        f.instruction(&Ins::I32Const(self.desc_addr(i)));
        f.instruction(&Ins::Call(
            self.fns
                .merge_blocks
                .expect("a discrete plan emits the block merge"),
        ));
    }

    /// `init_steady(transit, v, fracs)` for belt `i`, with `v` pushed by `value`.
    /// A leak-free belt's retained profile is uniformly 1, so the generic
    /// `b_init_steady` (one division, no zone walk) is exact for it; a leaky belt
    /// needs the plan-specialized closed form. Either way a discrete belt then lumps
    /// the result into time-unit blocks, exactly as `init_steady` does.
    fn emit_steady_fill(&self, f: &mut Function, i: usize, value: impl FnOnce(&mut Function)) {
        f.instruction(&Ins::I32Const(self.desc_addr(i)));
        value(f);
        f.instruction(&Ins::Call(
            self.plan_fns[i].init_steady.unwrap_or(self.fns.init_steady),
        ));
        self.emit_merge_if_discrete(f, i);
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
        self.emit_phase_a_all(f, locals, scope);
        for i in 0..self.plans.len() {
            self.emit_phase_b(f, i, locals);
        }
    }

    /// The §6.3 time-unit budget reset followed by phase A over EVERY belt --
    /// `conveyor_compile::run_phase_a` in full. Split out of [`Self::emit_step_pass`]
    /// so `module.rs` can interleave a coupled queue's serve between it and each
    /// belt's [`Self::emit_phase_b`] (queues.md §9), which is the whole point of the
    /// combined pass.
    pub(super) fn emit_phase_a_all(
        &self,
        f: &mut Function,
        locals: ConveyorPassLocals,
        scope: ErrorScope,
    ) {
        self.emit_time_boundary_reset(f, locals);
        for i in 0..self.plans.len() {
            self.emit_phase_a(f, i, locals, scope);
        }
    }

    /// How many belts this pass carries -- the driver's loop bound.
    pub(super) fn n_plans(&self) -> usize {
        self.plans.len()
    }

    /// `run_phase_a`'s prologue: when the modeled clock crosses an integer time unit,
    /// zero every belt's discrete per-time-unit `in_limit` budget (§6.3).
    ///
    /// TWO Rust-vs-wasm divergences hide in `conveyor_time_unit`'s three lines.
    ///
    /// * `f64::round` rounds half AWAY from zero; wasm's `f64.nearest` rounds half to
    ///   EVEN, so they disagree at every `x.5`. `b_round` reproduces Rust.
    /// * `expr as i64` SATURATES in Rust (and maps NaN to 0), while wasm's
    ///   `i64.trunc_f64_s` TRAPS on both. `b_sat_i64` reproduces Rust.
    ///
    /// Neither divergence is reachable from today's clock: `time` advances by exactly
    /// `dt` per step, so `(time - start) / dt` lands within a few ULP of an integer and
    /// `round` never sees a half-way input, and a time unit near 2^63 is absurd. The
    /// emitters mirror the VM's FUNCTIONS anyway rather than the inputs those functions
    /// happen to receive: "unreachable" is a property of the caller, not of the
    /// lowering, and `f64.nearest` would be a silent trapdoor the day the clock changes.
    /// The whole block is elided for a model with no discrete belt, whose `in_carry` no
    /// code path reads.
    fn emit_time_boundary_reset(&self, f: &mut Function, locals: ConveyorPassLocals) {
        let (Some(round), Some(sat)) = (self.fns.round, self.fns.sat_i64) else {
            return;
        };
        // unit = floor(start + round((time - start) / dt) * dt) as i64.
        f.instruction(&f64_const(self.start));
        emit_load_curr(f, crate::vm::TIME_OFF);
        f.instruction(&f64_const(self.start));
        f.instruction(&Ins::F64Sub);
        f.instruction(&f64_const(self.dt));
        f.instruction(&Ins::F64Div);
        f.instruction(&Ins::Call(round));
        f.instruction(&f64_const(self.dt));
        f.instruction(&Ins::F64Mul);
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::F64Floor);
        f.instruction(&Ins::Call(sat));
        f.instruction(&Ins::LocalSet(locals.unit));

        f.instruction(&Ins::LocalGet(locals.unit));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I64Load(i32_memarg(self.state.last_unit())));
        f.instruction(&Ins::I64Ne);
        f.instruction(&Ins::If(BlockType::Empty));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::LocalGet(locals.unit));
        f.instruction(&Ins::I64Store(i32_memarg(self.state.last_unit())));
        // `on_time_boundary` runs over every belt, arrested ones included (§4.3
        // step 0 freezes the belt, not the clock).
        for i in 0..self.plans.len() {
            f.instruction(&Ins::I32Const(0));
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_IN_CARRY))));
        }
        f.instruction(&Ins::End);
    }

    /// Phase A for belt `i` (§4.3 steps 0-3): snapshot the start-of-step contents,
    /// then -- unless the belt is arrested -- latch the transit time, leak, and
    /// discharge the exit slat as the driven outflow rate.
    ///
    /// The order is load-bearing. `step_contents0` is the PRE-leak total (phase B
    /// measures capacity room against it and then subtracts the leak) and is
    /// snapshotted even for an ARRESTED belt, before `phase_a`'s step-0 early return.
    /// The latch changes only the entry depth `d` and not the physical belt length the
    /// leak zones are measured against, and the exit slat is read AFTER it has leaked
    /// -- so a full-zone leak takes its cut from the material leaving this DT too.
    ///
    /// **What arrest freezes** (`conveyor.rs`'s step-0 return): the latch, the leak,
    /// and the exit. Every driven rate this belt owns is published as 0. What arrest
    /// does NOT freeze: `step_contents0` (above), the time-unit budget reset (see
    /// [`Self::emit_time_boundary_reset`]), and the `<leak_integers/>` carry of a
    /// DOWNSTREAM belt whose leak flow happens to feed this one.
    ///
    /// **What a HELD exit freezes** (`dest_arrested`, step 3): only the exit. The
    /// latch and the leak both run, so a held belt still sheds material and still
    /// changes its entry depth; phase B then merges the exit slat forward instead of
    /// popping it.
    ///
    /// **The `<sample>` gate** (step 1) decides whether this step re-latches `<len>`.
    /// A belt with no `<sample>` expression re-latches every DT (`run_phase_a`'s
    /// `unwrap_or(true)`), which is why belt growth was already lowered in step 1.
    /// A belt WITH one keeps its previous entry depth on a non-sampling step -- so
    /// there is no "next sample" bookkeeping to carry: the sample expression is an
    /// ordinary aux the Flows phase re-evaluates, and `is_nonzero` reads it. The
    /// latched transit itself needs no storage either, since the only thing the VM
    /// ever derives from `latched_transit` is `slat_count(latched_transit, dt)`,
    /// which the descriptor holds directly as `d`.
    fn emit_phase_a(
        &self,
        f: &mut Function,
        i: usize,
        locals: ConveyorPassLocals,
        scope: ErrorScope,
    ) {
        let plan = &self.plans[i];
        let d = self.desc_addr(i);
        let zero_rate = f64_const(0.0 / self.dt);

        // step_contents0 = contents(). `phase_a` assigns it BEFORE the arrest check.
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.total));
        f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_C0))));

        let arrested = plan.arrest_off.is_some();
        if arrested {
            emit_flag(f, plan.arrest_off);
            f.instruction(&Ins::If(BlockType::Empty));
            // Step 0: `out_vol = 0`, `leak_vols = vec![0.0; n]`, no latch, no leak.
            // The `lv` row is scratch that phase B skips on an arrested belt, but the
            // VM RETURNS a zero vector, so zero it: a future reader of `lv` (a
            // §8 `source` placement, GH #946) must not see the last step's volumes.
            let g0 = self.leak_base_idx[i];
            for (k, lk) in plan.leaks.iter().enumerate() {
                store_static_f64(f, self.leaks.lv(g0 + k), |f| {
                    f.instruction(&f64_const(0.0));
                });
                f.instruction(&Ins::I32Const(0));
                f.instruction(&zero_rate);
                f.instruction(&Ins::F64Store(memarg(slot_addr(lk.flow_off))));
            }
            f.instruction(&Ins::I32Const(0));
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_OUT))));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&zero_rate);
            f.instruction(&Ins::F64Store(memarg(slot_addr(plan.primary_out_off))));
            f.instruction(&Ins::Else);
        }
        // Every `raise` below sits inside the arrest `Else` (when there is one), the
        // latch `If`, and the bound `If`, so the unwind `br` must skip all three.
        let inner = if arrested { scope.entered() } else { scope };

        // Step 1: latch, iff `sample && transit.is_finite()` (`clamp_transit` passes a
        // non-finite value through unchanged so `phase_a` skips the latch).
        emit_load_curr(f, plan.len_off);
        emit_clamp_transit(f, self.dt, locals.transit);
        f.instruction(&Ins::LocalSet(locals.transit));
        f.instruction(&Ins::LocalGet(locals.transit));
        f.instruction(&Ins::Call(self.fns.slat_count));
        f.instruction(&Ins::LocalSet(locals.n_slats));

        emit_sample_flag(f, plan.sample_off);
        emit_is_finite(f, locals.transit);
        f.instruction(&Ins::I32And);
        f.instruction(&Ins::If(BlockType::Empty));
        // §4.1: `run_phase_a` bound-checks exactly when it would latch -- which is
        // also what makes the narrowing below safe. A NON-latching step must never
        // reach `i32.trunc_f64_s`: an unsampled belt whose `<len>` aux holds 1e300
        // has a finite, i32-overflowing `n_slats` that the VM never looks at, and
        // `trunc_f64_s` traps rather than saturating.
        f.instruction(&Ins::LocalGet(locals.n_slats));
        f.instruction(&f64_const(crate::conveyor::slat_bound() as f64));
        f.instruction(&Ins::F64Gt);
        f.instruction(&Ins::If(BlockType::Empty));
        inner
            .entered()
            .entered()
            .raise(f, crate::common::ErrorCode::ConveyorTransitTooLong, i);
        f.instruction(&Ins::End);
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::LocalGet(locals.n_slats));
        f.instruction(&Ins::I32TruncF64S);
        f.instruction(&Ins::I32Store(i32_memarg(self.field_addr(i, B_D))));
        f.instruction(&Ins::End);

        // Step 2: leak. Each flow's volume lands in `LeakRegions::lv` and is published
        // as a rate here, in listed order, exactly where `run_phase_a` publishes it --
        // before the NEXT belt's phase A, which cannot read a flow slot anyway (a
        // belt's parameters are synthesized auxes), and long before any phase B, which
        // must see every upstream driven rate.
        if let Some(leak) = self.plan_fns[i].leak {
            f.instruction(&Ins::I32Const(d));
            f.instruction(&Ins::Call(leak));
            let g0 = self.leak_base_idx[i];
            for (k, lk) in plan.leaks.iter().enumerate() {
                f.instruction(&Ins::I32Const(0));
                load_static_f64(f, self.leaks.lv(g0 + k));
                f.instruction(&f64_const(self.dt));
                f.instruction(&Ins::F64Div);
                f.instruction(&Ins::F64Store(memarg(slot_addr(lk.flow_off))));
            }
        }

        // Step 3: exit, HELD when the primary destination is an arrested belt. `out_vol`
        // is consumed by phase B (capacity room, and the merge-vs-pop shift) and by the
        // Stocks phase through the driven rate written here.
        let held = dest_arrest_off(self.plans, plan);
        if held.is_some() {
            emit_flag(f, held);
            f.instruction(&Ins::If(BlockType::Empty));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_OUT))));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&zero_rate);
            f.instruction(&Ins::F64Store(memarg(slot_addr(plan.primary_out_off))));
            f.instruction(&Ins::Else);
        }
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
        if held.is_some() {
            f.instruction(&Ins::End);
        }

        if arrested {
            f.instruction(&Ins::End);
        }
    }

    /// Phase B for belt `i` (§4.3 steps 4-6), or -- on an ARRESTED belt -- `phase_b`'s
    /// step-0 early return, which is not quite a no-op: it publishes an admitted rate
    /// of 0 for every equation-driven inflow (`in_vols: vec![0.0; n_inflows]`, then
    /// `conveyor_phase_b_one`'s write-back). Nothing else moves: no admission, no
    /// `in_carry` draw, no quantization, no shift, no insert.
    fn emit_phase_b(&self, f: &mut Function, i: usize, locals: ConveyorPassLocals) {
        let plan = &self.plans[i];
        if plan.arrest_off.is_none() {
            self.emit_phase_b_active(f, i, locals);
            return;
        }
        emit_flag(f, plan.arrest_off);
        f.instruction(&Ins::If(BlockType::Empty));
        for &off in &eq_inflow_offsets(plan) {
            f.instruction(&Ins::I32Const(0));
            f.instruction(&f64_const(0.0 / self.dt));
            f.instruction(&Ins::F64Store(memarg(slot_addr(off))));
        }
        f.instruction(&Ins::Else);
        self.emit_phase_b_active(f, i, locals);
        f.instruction(&Ins::End);
    }

    /// Phase B for a belt that is NOT arrested (§4.3 steps 4-6): admit, shift, insert,
    /// write back.
    ///
    /// The admission chain uses wasm's `f64.min` rather than a compare-and-select,
    /// which is sound because no operand can be NaN: `emit_clamp_nonneg` maps a NaN
    /// rate to `0.0`, so the request volume is a non-negative (possibly infinite)
    /// number; `rem_cap` starts at `+INF` or at a `max(0, ·)` clamp (also NaN-free);
    /// `rem_limit` starts at `+INF` or at `clamp_cap(·) * dt >= 0` (continuous) or a
    /// `max(0, ·)` clamp (discrete); and each is only decremented by a `c <= itself`
    /// when finite, so both stay non-negative and non-NaN. That is exactly the
    /// reasoning `ConveyorState::phase_b` relies on for its `f64::min` calls.
    ///
    /// `discrete_admit`'s `units.min(budget)` is the one `min` here that CAN see a
    /// NaN (`quant_carry[j]` goes NaN after an infinite cleared volume), so it goes
    /// through `b_fmin` -- Rust's NaN-ignoring `f64::min` -- rather than `f64.min`.
    fn emit_phase_b_active(&self, f: &mut Function, i: usize, locals: ConveyorPassLocals) {
        let plan = &self.plans[i];
        let d = self.desc_addr(i);

        // Step 4a: unconditionally-admitted volume -- conveyor-driven chain inflows AND
        // the shared flow of any queue coupling, whose rate the interleaved queue serve
        // already wrote (§11). Accumulated from `0.0` in inflow order, matching
        // `conv_inflows.iter().sum()`.
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::LocalSet(locals.conv_vol));
        for inf in plan
            .inflows
            .iter()
            .filter(|inf| inf.conveyor_driven || inf.queue_coupled)
        {
            f.instruction(&Ins::LocalGet(locals.conv_vol));
            emit_load_curr(f, inf.flow_off);
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Mul);
            f.instruction(&Ins::F64Add);
            f.instruction(&Ins::LocalSet(locals.conv_vol));
        }

        // Step 4b: `cap_room`. `rem_cap` is drawn down across the inflow list, but a
        // discrete belt's whole-unit `budget` is `floor(cap_room)` on the UNDRAWN room,
        // so both are kept.
        self.emit_cap_room(f, i, locals, locals.conv_vol);
        f.instruction(&Ins::LocalTee(locals.cap_room));
        f.instruction(&Ins::LocalSet(locals.rem_cap));

        // Step 4c: `limit_vol`.
        self.emit_limit_vol(f, i, locals);
        f.instruction(&Ins::LocalSet(locals.rem_limit));

        // Step 4d: apportion the clearance across the equation-driven inflows in
        // listed order.
        let eq_inflows = eq_inflow_offsets(plan);
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

        // §6.4 rule 1: a discrete belt turns each cleared volume into whole units,
        // in place, so the scratch region holds `in_vols` from here on. Runs even
        // for a belt with NO equation-driven inflow, because `phase_b` calls
        // `discrete_admit` unconditionally and its `in_carry += 0.0` is observable
        // as a sign-of-zero normalization.
        if plan.discrete {
            self.emit_discrete_admit(f, i, locals, &eq_inflows);
        }

        // `acc` is the inserted cohort's volume, accumulated exactly as the VM's
        // `shares[d-1]` is: `0.0 + conv_0 + ... + in_vol_0 + ...`, where `conv_vol`
        // already carries the `0.0 +` seed. Folding it here rather than inside the
        // clearance loop is what lets the discrete pass rewrite the scratch first.
        f.instruction(&Ins::LocalGet(locals.conv_vol));
        f.instruction(&Ins::LocalSet(locals.acc));
        for j in 0..eq_inflows.len() {
            f.instruction(&Ins::LocalGet(locals.acc));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::F64Load(memarg(self.scratch_addr(j))));
            f.instruction(&Ins::F64Add);
            f.instruction(&Ins::LocalSet(locals.acc));
        }

        // Step 5: shift. Normally the exit slat left as outflow and pops. When the
        // exit is HELD (`dest_arrested`), it stays and the next slat merges INTO it
        // instead -- material piles up at the exit until the destination unarrests.
        // `b_merge_front` implements that as "slat 1 += slat 0, then pop", which
        // yields the identical sequence to the VM's "slat 0 += slat 1, then remove
        // index 1" while touching only the ring head. A held belt of one slat does
        // nothing at all (the VM's `if self.slats.len() > 1` guard).
        match dest_arrest_off(self.plans, plan) {
            None => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.pop_front));
            }
            Some(off) => {
                emit_is_nonzero(f, off);
                f.instruction(&Ins::If(BlockType::Empty));
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(
                    self.fns
                        .merge_front
                        .expect("a holdable belt emits the front merge"),
                ));
                f.instruction(&Ins::Else);
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::Call(self.fns.pop_front));
                f.instruction(&Ins::End);
            }
        }
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.shrink));

        // Step 6: insert. The belt first grows to the (possibly just-increased)
        // entry depth, THEN the admitted cohort lands at the entry slat `d-1` --
        // the `beginning` placement, the only one this step lowers. The `!= 0.0`
        // gate mirrors `phase_b`'s `shares.iter().any(|&s| s != 0.0)`: it also
        // skips a `-0.0` total (`-0.0 != 0.0` is false) and admits a NaN one. When
        // the gate skips, the whole hoisted `r_k`/`M_k(d)` block is dead in the VM
        // too, which is why `b_insert_{i}` (which computes them) sits inside it.
        f.instruction(&Ins::I32Const(d));
        f.instruction(&Ins::Call(self.fns.grow_to_d));
        f.instruction(&Ins::LocalGet(locals.acc));
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::F64Ne);
        f.instruction(&Ins::If(BlockType::Empty));
        match self.plan_fns[i].insert {
            Some(insert) => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::LocalGet(locals.acc));
                f.instruction(&Ins::Call(insert));
            }
            None => {
                f.instruction(&Ins::I32Const(d));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::I32Load(i32_memarg(self.field_addr(i, B_D))));
                f.instruction(&Ins::I32Const(1));
                f.instruction(&Ins::I32Sub);
                f.instruction(&Ins::LocalGet(locals.acc));
                f.instruction(&Ins::Call(self.fns.add_at));
            }
        }
        f.instruction(&Ins::End);

        // Write the admitted equation-driven rates back, in listed order. A
        // conveyor-driven inflow's slot already holds the upstream belt's phase A
        // rate, and a queue-coupled one holds the shared `served / dt` the combined
        // pass wrote; neither may be overwritten. `eq_inflow_offsets` excludes both.
        for (j, &off) in eq_inflows.iter().enumerate() {
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::F64Load(memarg(self.scratch_addr(j))));
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Div);
            f.instruction(&Ins::F64Store(memarg(slot_addr(off))));
        }
    }

    /// Push `admission_room`'s `cap_room` for belt `i`, charging `conv_vol_local`
    /// against it. An absent `<capacity>` is a compile-time `+INF`; a present one is
    /// clamped, and the `is_infinite` arm matters because `INF - NaN` is NaN while the
    /// VM's infinite-capacity branch yields `+INF` unconditionally.
    ///
    /// `conv_vol_local` is what makes this shared: `phase_b` charges EVERY
    /// unconditionally-admitted volume, while `coupled_admission_budget` charges only
    /// the OTHER ones (the queue-supplied volume is the thing being sized, so it must
    /// not pre-charge its own room). Sizing a coupled queue against a formula that had
    /// drifted from the one `phase_b` then admits with would put over-capacity material
    /// on the belt with no error, which is why `ConveyorState::admission_room` is one
    /// function in the VM and one emitter here.
    ///
    /// Clobbers `locals.capacity` and `locals.tmp`.
    fn emit_cap_room(
        &self,
        f: &mut Function,
        i: usize,
        locals: ConveyorPassLocals,
        conv_vol_local: u32,
    ) {
        let Some(off) = self.plans[i].cap_off else {
            f.instruction(&f64_const(f64::INFINITY));
            return;
        };
        emit_load_curr(f, off);
        emit_clamp_cap(f, locals.tmp);
        f.instruction(&Ins::LocalSet(locals.capacity));
        f.instruction(&f64_const(f64::INFINITY)); // select's `true` arm
        f.instruction(&Ins::LocalGet(locals.capacity));
        // contents_after = (step_contents0 - leaked) - out_vol, associating exactly
        // as `phase_b` does. `leaked` folds this belt's leak volumes in LISTED order
        // from `+0.0` (`leak_vols.iter().sum()`), so a leak-free belt subtracts the
        // additive identity and this reduces to the pre-leak `c0 - out` form.
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_C0))));
        self.emit_leaked_sum(f, i);
        f.instruction(&Ins::F64Sub);
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_OUT))));
        f.instruction(&Ins::F64Sub); // contents_after
        f.instruction(&Ins::F64Sub); // capacity - contents_after
        f.instruction(&Ins::LocalGet(conv_vol_local));
        f.instruction(&Ins::F64Sub);
        emit_clamp_nonneg(f, locals.tmp); // select's `false` arm
        f.instruction(&Ins::LocalGet(locals.capacity));
        f.instruction(&f64_const(f64::INFINITY));
        f.instruction(&Ins::F64Eq);
        f.instruction(&Ins::Select);
    }

    /// Push `admission_room`'s `limit_vol` for belt `i`.
    ///
    /// A CONTINUOUS belt prorates the per-time-unit `<in_limit>` to this DT, and
    /// `INF * dt == INF`, so the VM's `is_infinite` branch folds away. A DISCRETE belt
    /// instead draws down a per-TIME-UNIT budget: `max(0, in_limit - in_carry)`, where
    /// `in_carry` accumulates within the time unit and resets at the boundary (§6.3).
    /// There the `is_infinite` arm does NOT fold away -- an unconstrained belt whose
    /// `in_carry` reached `+INF` would compute `INF - INF = NaN`, and `NaN.max(0.0)` is
    /// `0.0` in Rust, i.e. "admit nothing", where the VM admits everything.
    ///
    /// Clobbers `locals.tmp` and `locals.tmp2`.
    fn emit_limit_vol(&self, f: &mut Function, i: usize, locals: ConveyorPassLocals) {
        let plan = &self.plans[i];
        let Some(off) = plan.inlim_off else {
            f.instruction(&f64_const(f64::INFINITY));
            return;
        };
        emit_load_curr(f, off);
        emit_clamp_cap(f, locals.tmp);
        if !plan.discrete {
            f.instruction(&f64_const(self.dt));
            f.instruction(&Ins::F64Mul);
            return;
        }
        f.instruction(&Ins::LocalSet(locals.tmp2));
        f.instruction(&f64_const(f64::INFINITY)); // select's `true` arm
        f.instruction(&Ins::LocalGet(locals.tmp2));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_IN_CARRY))));
        f.instruction(&Ins::F64Sub);
        emit_clamp_nonneg(f, locals.tmp); // select's `false` arm
        f.instruction(&Ins::LocalGet(locals.tmp2));
        f.instruction(&f64_const(f64::INFINITY));
        f.instruction(&Ins::F64Eq);
        f.instruction(&Ins::Select);
    }

    /// `ConveyorState::discrete_admit` (§6.4 rule 1) for belt `i`, rewriting each
    /// cleared volume in the scratch region into the whole units actually admitted.
    ///
    /// Three pieces of state move here, and all three are cross-step:
    ///
    /// 1. `in_carry += Σ cleared` -- the per-time-unit `<in_limit>` draw. It is
    ///    charged at CLEARANCE time, not admission time, so a request the whole-unit
    ///    budget rounds away still spends its share of the limit.
    /// 2. `quant_carry[j] += cleared_j`, then `floor` of it admits and the remainder
    ///    persists forever (never reset, §6.4 rule 1): a discrete belt fed 0.4/step
    ///    admits one unit every third step, not zero units always.
    /// 3. `budget = floor(cap_room)`, drawn down across the inflows in listed order,
    ///    so every inserted whole unit debits exactly the inflow that cleared it.
    ///
    /// The `units < 0.0` clamp fires only after `budget` has already been decremented
    /// by the un-clamped value, and a NaN `units` (from an infinite cleared volume)
    /// passes the clamp untouched. Both are faithful transcriptions, not oversights.
    fn emit_discrete_admit(
        &self,
        f: &mut Function,
        i: usize,
        locals: ConveyorPassLocals,
        eq_inflows: &[usize],
    ) {
        let q0 = self.quant_base_idx[i].expect("a discrete plan has a quant-carry row");

        // in_carry += cleared.iter().sum()  (folded from +0.0 in listed order).
        f.instruction(&f64_const(0.0));
        for j in 0..eq_inflows.len() {
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::F64Load(memarg(self.scratch_addr(j))));
            f.instruction(&Ins::F64Add);
        }
        f.instruction(&Ins::LocalSet(locals.tmp));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_IN_CARRY))));
        f.instruction(&Ins::LocalGet(locals.tmp));
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_IN_CARRY))));

        // budget = cap_room.is_infinite() ? INF : cap_room.floor(). `cap_room` is
        // non-negative and non-NaN (see `emit_phase_b_active`), so `is_infinite`
        // reduces to `== +INF`.
        f.instruction(&f64_const(f64::INFINITY));
        f.instruction(&Ins::LocalGet(locals.cap_room));
        f.instruction(&Ins::F64Floor);
        f.instruction(&Ins::LocalGet(locals.cap_room));
        f.instruction(&f64_const(f64::INFINITY));
        f.instruction(&Ins::F64Eq);
        f.instruction(&Ins::Select);
        f.instruction(&Ins::LocalSet(locals.budget));

        for j in 0..eq_inflows.len() {
            let g = q0 + j;
            store_static_f64(f, self.state.quant_carry(g), |f| {
                load_static_f64(f, self.state.quant_carry(g));
                f.instruction(&Ins::I32Const(0));
                f.instruction(&Ins::F64Load(memarg(self.scratch_addr(j))));
                f.instruction(&Ins::F64Add);
            });
            load_static_f64(f, self.state.quant_carry(g));
            f.instruction(&Ins::F64Floor);
            f.instruction(&Ins::LocalSet(locals.units));

            emit_is_finite(f, locals.budget);
            f.instruction(&Ins::If(BlockType::Empty));
            f.instruction(&Ins::LocalGet(locals.units));
            f.instruction(&Ins::LocalGet(locals.budget));
            f.instruction(&Ins::Call(self.fns.fmin));
            f.instruction(&Ins::LocalSet(locals.units));
            f.instruction(&Ins::LocalGet(locals.budget));
            f.instruction(&Ins::LocalGet(locals.units));
            f.instruction(&Ins::F64Sub);
            f.instruction(&Ins::LocalSet(locals.budget));
            f.instruction(&Ins::End);

            // units = units < 0.0 ? 0.0 : units  (a NaN compares false and survives).
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::LocalGet(locals.units));
            f.instruction(&Ins::LocalGet(locals.units));
            f.instruction(&f64_const(0.0));
            f.instruction(&Ins::F64Lt);
            f.instruction(&Ins::Select);
            f.instruction(&Ins::LocalSet(locals.units));

            store_static_f64(f, self.state.quant_carry(g), |f| {
                load_static_f64(f, self.state.quant_carry(g));
                f.instruction(&Ins::LocalGet(locals.units));
                f.instruction(&Ins::F64Sub);
            });
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::LocalGet(locals.units));
            f.instruction(&Ins::F64Store(memarg(self.scratch_addr(j))));
        }
    }

    fn scratch_addr(&self, j: usize) -> u64 {
        u64::from(self.layout.scratch_base) + j as u64 * SLOT_BYTES as u64
    }

    /// Push belt `i`'s `leaked` -- `phase_a.leak_vols.iter().sum()`, folded from
    /// `+0.0` in listed order. A leak-free belt pushes the bare `+0.0`, the additive
    /// identity the pre-leak lowering relied on.
    fn emit_leaked_sum(&self, f: &mut Function, i: usize) {
        let g0 = self.leak_base_idx[i];
        f.instruction(&f64_const(0.0));
        for k in 0..n_leaks(&self.plans[i]) {
            load_static_f64(f, self.leaks.lv(g0 + k));
            f.instruction(&Ins::F64Add);
        }
    }

    // ── the conveyor half of queue coupling (§11 / queues.md §9) ────────────

    /// `prior_coupled_vol = 0.0`, once per belt before its coupled queues are served.
    pub(super) fn emit_reset_prior(&self, f: &mut Function, locals: ConveyorPassLocals) {
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::LocalSet(locals.prior));
    }

    /// `conveyor_compile::coupled_admission_budget` for belt `i`, leaving `req` in
    /// `locals.req`: the volume a queue directly upstream may supply this DT.
    ///
    /// Emitted between belt `i`'s phase A (which snapshotted `c0` and freed room via
    /// leaks and the exit) and its phase B. An ARRESTED belt requests nothing -- the
    /// belt is frozen, so the queue holds -- which is `admission_budget`'s first line.
    ///
    /// `other_conv_vol` charges the OTHER conveyor-driven inflows plus
    /// `locals.prior` (what earlier-served coupled queues already committed to this
    /// belt this DT), but NOT the queue-supplied volume being sized. Its per-time-unit
    /// inflow-limit half is charged separately, by `emit_consume_inflow_budget`
    /// advancing `in_carry` -- so it must not be double-charged here.
    ///
    /// `cap_room.min(limit_vol)` uses wasm's `f64.min`, sound because both are in
    /// `[0, +INF]` and neither can be NaN (see [`Self::emit_phase_b_active`]).
    /// Clobbers `locals.conv_vol` (phase B recomputes it from scratch) plus the
    /// `capacity`/`tmp`/`tmp2` scratch.
    pub(super) fn emit_coupled_budget(
        &self,
        f: &mut Function,
        i: usize,
        locals: ConveyorPassLocals,
    ) {
        let plan = &self.plans[i];
        let emit_budget = |f: &mut Function| {
            // other_conv_vol = (0.0 + Σ conveyor-driven volumes) + prior_coupled_vol.
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
            f.instruction(&Ins::LocalGet(locals.conv_vol));
            f.instruction(&Ins::LocalGet(locals.prior));
            f.instruction(&Ins::F64Add);
            f.instruction(&Ins::LocalSet(locals.conv_vol));

            self.emit_cap_room(f, i, locals, locals.conv_vol);
            self.emit_limit_vol(f, i, locals);
            f.instruction(&Ins::F64Min);
            f.instruction(&Ins::LocalSet(locals.req));
        };

        match plan.arrest_off {
            None => emit_budget(f),
            Some(off) => {
                emit_is_nonzero(f, off);
                f.instruction(&Ins::If(BlockType::Empty));
                f.instruction(&f64_const(0.0));
                f.instruction(&Ins::LocalSet(locals.req));
                f.instruction(&Ins::Else);
                emit_budget(f);
                f.instruction(&Ins::End);
            }
        }
    }

    /// `ConveyorState::consume_inflow_budget(taken)` for belt `i`: debit the discrete
    /// per-time-unit `in_limit` budget by a queue-coupled admission.
    ///
    /// The coupled volume enters through the unconditional `conv_inflows` path, which
    /// never touches `in_carry`, so the coupling records the consumption here --
    /// otherwise every DT within a time unit would see the full budget. A continuous
    /// belt is a compile-time no-op, which is exactly the VM's `if self.discrete`
    /// guard; the compiler's `ConveyorQueueUpstreamNotDiscrete` check means a coupled
    /// belt is always discrete, so this branch never actually folds away in practice.
    pub(super) fn emit_consume_inflow_budget(&self, f: &mut Function, i: usize, vol_local: u32) {
        if !self.plans[i].discrete {
            return;
        }
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::F64Load(memarg(self.field_addr(i, B_IN_CARRY))));
        f.instruction(&Ins::LocalGet(vol_local));
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::F64Store(memarg(self.field_addr(i, B_IN_CARRY))));
    }

    /// `prior_coupled_vol += taken`, closing one coupled serve.
    pub(super) fn emit_add_prior(&self, f: &mut Function, locals: ConveyorPassLocals) {
        f.instruction(&Ins::LocalGet(locals.prior));
        f.instruction(&Ins::LocalGet(locals.taken));
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::LocalSet(locals.prior));
    }

    /// Belt `i`'s phase B, for the interleaved driver. See [`Self::emit_phase_b`].
    pub(super) fn emit_phase_b_at(&self, f: &mut Function, i: usize, locals: ConveyorPassLocals) {
        self.emit_phase_b(f, i, locals);
    }

    // ── the mid-run preview ─────────────────────────────────────────────────

    /// Save everything the preview pass will mutate and repoint each descriptor at a
    /// verbatim ring copy, so the preview runs on a throwaway side table (`vm.rs`'s
    /// cloned `Vec<ConveyorState>` plus its cloned `conveyor_last_unit`). The caller
    /// saves and rewinds `G_HEAP` around this; see `passes::QueuePass::emit_preview_save`.
    ///
    /// **Every piece of cross-step mutable belt state, and where it is covered:**
    ///
    /// | state (`conveyor.rs`)        | lives in            | covered by            |
    /// |------------------------------|---------------------|-----------------------|
    /// | `slats[*].content`           | the ring            | `b_clone_ring`        |
    /// | `slats[*].leak_basis/window` | the ring            | `b_clone_ring`        |
    /// | `latched_transit` (as `d`)   | the descriptor      | the descriptor save   |
    /// | `step_contents0`, `out_vol`  | the descriptor      | the descriptor save   |
    /// | ring geometry (`head`/`len`/`cap`/`base`) | the descriptor | the descriptor save |
    /// | `in_carry` (§6.3)            | the descriptor      | the descriptor save   |
    /// | `leak_carry` (§5.4)          | [`LeakRegions`]     | [`Self::emit_carry_copy`] |
    /// | `quant_carry` (§6.4 rule 1)  | [`StateRegions`]    | [`Self::emit_carry_copy`] |
    /// | `Vm::conveyor_last_unit`     | [`StateRegions`]    | [`Self::emit_carry_copy`] |
    ///
    /// The three static-region entries are the dangerous ones: the VM clones a whole
    /// `ConveyorState` (and copies `last_unit` into a local), so an omission here is
    /// silent -- a preview would leave the real belt with an advanced carry, and the
    /// resumed step would admit different material. Everything else rides a clone.
    /// `LeakRegions`' remaining arrays (`lv`/`r`/`sheds`/`ub`/`m_entry`/`first_zone`/
    /// `prefix`) are single-step scratch: each is written before it is read within the
    /// pass, so the preview's writes are invisible to the next real step.
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
        self.emit_carry_copy(f, true);
    }

    /// Restore each saved descriptor and carry, dropping the cloned ring (and, with
    /// it, the preview's `c0`/`out`/`d`/`in_carry` and slat writes).
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
        self.emit_carry_copy(f, false);
    }

    /// Copy the static-region cross-step state to (`save`) or from (`!save`) its save
    /// area: the `<leak_integers/>` carries, the discrete quantization carries, and the
    /// pass-global time-unit clock. Unrolled over the model's global indices, which is
    /// why it is not per-belt.
    fn emit_carry_copy(&self, f: &mut Function, save: bool) {
        for g in 0..self.leaks.total as usize {
            let (dst, src) = if save {
                (self.leaks.carry_save(g), self.leaks.carry(g))
            } else {
                (self.leaks.carry(g), self.leaks.carry_save(g))
            };
            store_static_f64(f, dst, |f| load_static_f64(f, src));
        }
        for g in 0..self.state.quant as usize {
            let (dst, src) = if save {
                (self.state.quant_carry_save(g), self.state.quant_carry(g))
            } else {
                (self.state.quant_carry(g), self.state.quant_carry_save(g))
            };
            store_static_f64(f, dst, |f| load_static_f64(f, src));
        }
        // `vm.rs:1198` previews with `let mut last_unit = self.conveyor_last_unit`,
        // so a preview that crosses a time boundary must not consume it: the resumed
        // real step has to fire the same reset.
        if self.fns.sat_i64.is_some() {
            let (dst, src) = if save {
                (self.state.last_unit_save(), self.state.last_unit())
            } else {
                (self.state.last_unit(), self.state.last_unit_save())
            };
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I64Load(i32_memarg(src)));
            f.instruction(&Ins::I64Store(i32_memarg(dst)));
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

/// Push `conveyor_compile::is_nonzero(curr[off])` -- §4.4's arrest/sample truth test:
/// nonzero AND not NaN. Spelled `x < 0 || x > 0`, which is false for both zeros and
/// for NaN with no scratch local, rather than `x != 0 && x == x` (`f64.ne` says a NaN
/// differs from zero, so the second conjunct would be doing all the work).
///
/// The slot is an ordinary aux the Flows phase wrote. No pass writes one -- an arrest
/// or sample equation cannot read a pass-driven flow (`ConveyorDrivenFlowRead`), and
/// container slots publish at step start -- so re-reading it at each use site inside
/// the pass yields the value `run_phase_a` snapshotted into its `arrested` vector
/// before any belt moved.
fn emit_is_nonzero(f: &mut Function, off: usize) {
    emit_load_curr(f, off);
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    emit_load_curr(f, off);
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::I32Or);
}

/// Push an optional flag slot's truth value; an absent slot is a compile-time `false`
/// (`plan.arrest_off.map(..).unwrap_or(false)`).
fn emit_flag(f: &mut Function, off: Option<usize>) {
    match off {
        None => f.instruction(&Ins::I32Const(0)),
        Some(o) => {
            emit_is_nonzero(f, o);
            f
        }
    };
}

/// Push the `<sample>` flag: an absent `<sample>` expression is a compile-time `true`
/// (`run_phase_a`'s `unwrap_or(true)` -- the XMILE default re-latches every DT, §6.1).
fn emit_sample_flag(f: &mut Function, off: Option<usize>) {
    match off {
        None => f.instruction(&Ins::I32Const(1)),
        Some(o) => {
            emit_is_nonzero(f, o);
            f
        }
    };
}

/// Rust's saturating `x as i64`, at compile time: out-of-range clamps to the bound and
/// NaN maps to 0. Used for the compile-time `floor(start)` clock seed; the runtime twin
/// is `b_sat_i64` ([`emit_sat_i64`]), and the two must agree.
fn sat_i64(x: f64) -> i64 {
    x as i64
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

/// Push the f64 at the static byte address `addr`.
fn load_static_f64(f: &mut Function, addr: u64) {
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::F64Load(memarg(addr)));
}

/// `mem[addr] = value_from(f)` for an f64 at a static byte address.
fn store_static_f64(f: &mut Function, addr: u64, value: impl FnOnce(&mut Function)) {
    f.instruction(&Ins::I32Const(0));
    value(f);
    f.instruction(&Ins::F64Store(memarg(addr)));
}

/// Push the i32 at the static byte address `addr`.
fn load_static_i32(f: &mut Function, addr: u64) {
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32Load(i32_memarg(addr)));
}

/// `mem[addr] = value_from(f)` for an i32 at a static byte address.
fn store_static_i32(f: &mut Function, addr: u64, value: impl FnOnce(&mut Function)) {
    f.instruction(&Ins::I32Const(0));
    value(f);
    f.instruction(&Ins::I32Store(i32_memarg(addr)));
}

/// Push `clamp_fraction(curr[plan.leaks[k].frac_off], plan.exponential_leak)` (§4.4,
/// `conveyor_compile.rs:2593`), using `scratch` (a free f64 local) to hold the raw
/// value across its four uses. The fraction is re-read from `curr` at every use site
/// exactly as the VM re-reads its `fracs` vector: no phase mutates a leak-fraction
/// aux slot, so the value is stable within a phase and the reads are interchangeable.
///
/// `f64::clamp` is deterministic on `-0.0` (`-0.0 < 0.0` is false, so it passes
/// through), and the linear arm reproduces that. The exponential arm's `v.max(0.0)`
/// is NOT deterministic on `-0.0` per Rust's own docs; it keeps `-0.0`, and the
/// module docs explain why nothing can observe the choice.
fn emit_leak_frac(f: &mut Function, plan: &ConveyorPlan, k: usize, scratch: u32) {
    emit_load_curr(f, plan.leaks[k].frac_off);
    f.instruction(&Ins::LocalSet(scratch));
    // The outermost select's `true` arm: NaN => 0.0.
    f.instruction(&f64_const(0.0));
    // The `v < 0.0` select's `true` arm.
    f.instruction(&f64_const(0.0));
    if !plan.exponential_leak {
        // Linear: `v.clamp(0.0, 1.0)`, whose upper half is `v > 1.0 ? 1.0 : v`.
        f.instruction(&f64_const(1.0));
        f.instruction(&Ins::LocalGet(scratch));
        f.instruction(&Ins::LocalGet(scratch));
        f.instruction(&f64_const(1.0));
        f.instruction(&Ins::F64Gt);
        f.instruction(&Ins::Select);
    } else {
        // Exponential: `v.max(0.0)`, i.e. no upper bound.
        f.instruction(&Ins::LocalGet(scratch));
    }
    f.instruction(&Ins::LocalGet(scratch));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalGet(scratch));
    f.instruction(&Ins::LocalGet(scratch));
    f.instruction(&Ins::F64Ne); // v != v, i.e. v is NaN
    f.instruction(&Ins::Select);
}

/// Push `b_in_zone(i_local, len_local, zone_start, zone_end)`.
fn push_in_zone(
    f: &mut Function,
    zf: ZoneFns,
    i_local: u32,
    len_local: u32,
    plan_leak: (f64, f64),
) {
    f.instruction(&Ins::LocalGet(i_local));
    f.instruction(&Ins::LocalGet(len_local));
    f.instruction(&f64_const(plan_leak.0));
    f.instruction(&f64_const(plan_leak.1));
    f.instruction(&Ins::Call(zf.in_zone));
}

/// The `(zone_start, zone_end)` pair of `plan`'s leak flow `k`.
fn zone_of(plan: &ConveyorPlan, k: usize) -> (f64, f64) {
    (plan.leaks[k].zone_start, plan.leaks[k].zone_end)
}

// `b_fmin(a: f64, b: f64) -> f64`
const FM_A: u32 = 0;
const FM_B: u32 = 1;

/// `b_fmin(a, b) -> f64`: Rust's `f64::min`, which RETURNS THE OTHER OPERAND when
/// one is NaN. wasm's `f64.min` propagates NaN instead, so §5.1's
/// `leak_basis.min(leak_window)` / `(f * use).min(content)` and §5.4's
/// `remaining.min(content)` cannot use it: a NaN cohort schedule (reachable from a
/// NaN conveyor-driven inflow volume) would poison a slat the VM leaves alone.
///
/// For two equal operands of opposite zero sign this returns wasm's `-0.0` while
/// Rust may return either; see the module docs.
fn emit_fmin() -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::LocalGet(FM_B)); // outer select's `true` arm: a is NaN => b
    f.instruction(&Ins::LocalGet(FM_A)); // inner select's `true` arm: b is NaN => a
    f.instruction(&Ins::LocalGet(FM_A));
    f.instruction(&Ins::LocalGet(FM_B));
    f.instruction(&Ins::F64Min); // neither is NaN: wasm agrees with Rust
    f.instruction(&Ins::LocalGet(FM_B));
    f.instruction(&Ins::LocalGet(FM_B));
    f.instruction(&Ins::F64Ne);
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalGet(FM_A));
    f.instruction(&Ins::LocalGet(FM_A));
    f.instruction(&Ins::F64Ne);
    f.instruction(&Ins::Select);

    f.instruction(&Ins::End);
    f
}

// `b_in_zone(i: i32, len: i32, zs: f64, ze: f64) -> i32`
const IZ_I: u32 = 0;
const IZ_LEN: u32 = 1;
const IZ_ZS: u32 = 2;
const IZ_ZE: u32 = 3;
const IZ_POS: u32 = 4;

/// `b_in_zone(i, len, zs, ze) -> i32`: `ConveyorState::in_zone` (§5.3). Slat `i`
/// (0 = exit) of a `len`-slat belt sits `pos = 1 - (i + 0.5) / len` from the ENTRY,
/// and is in zone when `zs <= pos <= ze`.
///
/// Exact `<=` on both ends, no epsilon: a slat exactly on a zone edge belongs to
/// both adjacent zones, deliberately (`conveyor.rs:394`). `zs`/`ze` are compile-time
/// constants clamped to `[0, 1]` (`conveyor_compile::parse_zone`) and `len >= 1`, so
/// `pos` is always finite and both comparisons are total.
fn emit_in_zone() -> Function {
    let mut f = Function::new([(1, ValType::F64)]);

    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalGet(IZ_I));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalGet(IZ_LEN));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(IZ_POS));

    f.instruction(&Ins::LocalGet(IZ_ZS));
    f.instruction(&Ins::LocalGet(IZ_POS));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::LocalGet(IZ_POS));
    f.instruction(&Ins::LocalGet(IZ_ZE));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::I32And);

    f.instruction(&Ins::End);
    f
}

// `b_zone_count(len: i32, depth: i32, zs: f64, ze: f64) -> i32`
const ZC_LEN: u32 = 0;
const ZC_DEPTH: u32 = 1;
const ZC_ZS: u32 = 2;
const ZC_ZE: u32 = 3;
const ZC_I: u32 = 4;
const ZC_N: u32 = 5;

/// `b_zone_count(len, depth, zs, ze) -> i32`: `zone_count_from`, the number of
/// in-zone slats among indices `0..depth` of a `len`-slat belt -- §5.1's `M_k(depth)`.
fn emit_zone_count(in_zone: u32) -> Function {
    let mut f = Function::new([(2, ValType::I32)]);

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(ZC_N));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(ZC_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(ZC_I));
    f.instruction(&Ins::LocalGet(ZC_DEPTH));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(ZC_I));
    f.instruction(&Ins::LocalGet(ZC_LEN));
    f.instruction(&Ins::LocalGet(ZC_ZS));
    f.instruction(&Ins::LocalGet(ZC_ZE));
    f.instruction(&Ins::Call(in_zone));
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(ZC_N));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(ZC_N));
    f.instruction(&Ins::End);

    f.instruction(&Ins::LocalGet(ZC_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(ZC_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(ZC_N));
    f.instruction(&Ins::End);
    f
}

// `b_first_zone(len: i32, depth: i32, zs: f64, ze: f64) -> i32`
const FZ_LEN: u32 = 0;
const FZ_DEPTH: u32 = 1;
const FZ_ZS: u32 = 2;
const FZ_ZE: u32 = 3;
const FZ_I: u32 = 4;

/// `b_first_zone(len, depth, zs, ze) -> i32`: the DEEPEST in-zone slat among
/// `0..depth`, i.e. the first one an entering cohort meets, or `-1` when the zone
/// misses the entry path entirely (`conveyor.rs:422`'s `first_zone_slat`, which
/// scans `(0..entry_depth).rev()` and takes the first hit).
fn emit_first_zone(in_zone: u32) -> Function {
    let mut f = Function::new([(1, ValType::I32)]);

    f.instruction(&Ins::LocalGet(FZ_DEPTH));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(FZ_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(FZ_I));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(FZ_I));
    f.instruction(&Ins::LocalGet(FZ_LEN));
    f.instruction(&Ins::LocalGet(FZ_ZS));
    f.instruction(&Ins::LocalGet(FZ_ZE));
    f.instruction(&Ins::Call(in_zone));
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(FZ_I));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::LocalGet(FZ_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(FZ_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::I32Const(-1));
    f.instruction(&Ins::End);
    f
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

// ── container reducers (§10) ─────────────────────────────────────────────────
//
// Each walks the belt exit-first over `len` slats, reading only the `content` word of
// each stride -- the vector `ConveyorState::slat_contents` materializes and
// `container_value_from_slice` reduces. `SUM` is `b_total` and `SIZE` an `i32.load`
// at the call site, so neither appears here.

/// `if len == 0 { return NaN }` -- the empty-container contract of every reducer but
/// `SUM` (additive identity `0`) and `SIZE`. Unreachable from a belt, whose `len` is
/// always `>= 1`; see [`ConveyorPass::emit_container_value`].
fn emit_nan_if_empty(f: &mut Function, d_local: u32) {
    f.instruction(&Ins::LocalGet(d_local));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);
}

/// `b_mean(d) -> f64`: `Σ contents / len`, NaN on an empty belt. The division is by the
/// PHYSICAL slat count, not the entry depth `d` -- the two differ on a belt whose
/// transit just shrank onto a non-empty tail.
fn emit_mean(total: u32) -> Function {
    let mut f = Function::new([]);
    emit_nan_if_empty(&mut f, 0);
    f.instruction(&Ins::LocalGet(0));
    f.instruction(&Ins::Call(total));
    f.instruction(&Ins::LocalGet(0));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::End);
    f
}

// `b_min(d) -> f64` / `b_max(d) -> f64`
const MM_D: u32 = 0;
const MM_I: u32 = 1;
const MM_ACC: u32 = 2;
const MM_X: u32 = 3;

/// `b_min(d)` (`is_min`) / `b_max(d)`: the folds `fold(f64::INFINITY, f64::min)` /
/// `fold(f64::NEG_INFINITY, f64::max)` over a non-empty slat-content vector; NaN when
/// empty.
///
/// The per-element step is a `select` on a STRICT comparison (`x < acc` / `x > acc`),
/// which is false for a NaN `x` -- so a NaN slat leaves the accumulator untouched,
/// exactly as Rust's `f64::min`/`f64::max` do, and an ALL-NaN belt reports the fold's
/// `±INFINITY` seed. wasm's `f64.min`/`f64.max` would instead poison the whole fold.
/// A leaky belt carrying infinite material reaches both cases (`INF - INF = NaN`).
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
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    push_slat_addr(&mut f, MM_D, MM_I);
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

// `b_stddev(d) -> f64`
const CSD_D: u32 = 0;
const CSD_I: u32 = 1;
const CSD_N: u32 = 2;
const CSD_MEAN: u32 = 3;
const CSD_VAR: u32 = 4;
const CSD_DIFF: u32 = 5;

/// `b_stddev(d) -> f64`: the POPULATION standard deviation (divisor `N`, matching
/// `container_value_from_slice` and `vm.rs`'s `ArrayStddev`); NaN when empty. The mean
/// is `b_total / N` and the squared deviations accumulate exit-first from `+0.0`, so
/// the two backends agree term for term.
///
/// The reference squares with `.powf(2.0)`; `f64.mul` is used here instead. The two
/// agree for a correctly-rounded `pow`, and `f64.mul` is exact where the blob's
/// open-coded `exp(2 * ln x)` `pow` helper would not be.
fn emit_stddev(total: u32) -> Function {
    let mut f = Function::new([(1, ValType::I32), (4, ValType::F64)]);
    emit_nan_if_empty(&mut f, CSD_D);

    f.instruction(&Ins::LocalGet(CSD_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::LocalSet(CSD_N));

    f.instruction(&Ins::LocalGet(CSD_D));
    f.instruction(&Ins::Call(total));
    f.instruction(&Ins::LocalGet(CSD_N));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(CSD_MEAN));

    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(CSD_VAR));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(CSD_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(CSD_I));
    f.instruction(&Ins::LocalGet(CSD_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    push_slat_addr(&mut f, CSD_D, CSD_I);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(CSD_MEAN));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(CSD_DIFF));

    f.instruction(&Ins::LocalGet(CSD_VAR));
    f.instruction(&Ins::LocalGet(CSD_DIFF));
    f.instruction(&Ins::LocalGet(CSD_DIFF));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(CSD_VAR));

    f.instruction(&Ins::LocalGet(CSD_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(CSD_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(CSD_VAR));
    f.instruction(&Ins::LocalGet(CSD_N));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::F64Sqrt);
    f.instruction(&Ins::End);
    f
}

// `b_slat_at(d: i32, j: i32) -> f64`
const SA_D: u32 = 0;
const SA_J: u32 = 1;

/// `b_slat_at(d, j) -> f64`: slat `j` counted from the EXIT (0-based), NaN when past
/// the live length -- `container_value_from_slice`'s `vec.get(j).unwrap_or(NAN)`.
///
/// `j < 0` is impossible by construction: [`slat_index`] is the only producer, and it
/// rejects the one 1-based index (`0`) that could underflow, along with every `j`
/// too large to narrow. So the guard tests the upper bound alone. (This is NOT
/// [`emit_front`], whose empty-belt answer is `0.0` because `phase_a` discharges
/// nothing from an empty belt.)
fn emit_slat_at() -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::LocalGet(SA_J));
    f.instruction(&Ins::LocalGet(SA_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&f64_const(f64::NAN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    push_slat_addr(&mut f, SA_D, SA_J);
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

/// `b_init_explicit(d, table, m)` / `b_init_explicit_disc(d, table, m)`: the §7.2
/// explicit-list CONTENT fill, reproducing `ConveyorState::init_explicit`. (The leak
/// schedules are `b_sched_{i}`'s job, and a discrete belt's block lumping is
/// `b_merge_blocks`'.)
///
/// Two interpretations, selected by list length (the isee rule):
///
/// * `m == N`: entry `j` fills slat `j` directly. Identical for both belt kinds.
/// * otherwise: one entry per TIME UNIT. Slat `i` belongs to block
///   `floor(i * dt)`, the list is normalized to `U = floor((N-1)*dt) + 1` entries
///   (extra truncated, a short list repeating its last), and each block's entry is
///   spread EVENLY across the block's slats for a continuous conveyor or placed WHOLE
///   in the block's deepest slat for a discrete one (`spread_per_time_unit`'s two
///   arms, selected here at emit time by `discrete`).
///
/// Blocks are contiguous and `floor(i * dt)` is monotone in `i`, so the per-block
/// slat COUNT `spread_per_time_unit` tabulates is just this loop's run length --
/// no counts array is needed, and its `deepest[b]` is `j - 1`. `block_of`'s
/// `.min(u - 1)` clamp is likewise omitted: `i <= N-1` and f64 multiplication is
/// monotone, so `floor(i*dt)` can never exceed `floor((N-1)*dt) == u - 1`.
///
/// `norm(b)` is `table[min(b, m-1)]`, which is why `m >= 1` is a precondition
/// (`build` routes an empty list to the zero-filling steady path instead).
fn emit_init_explicit(dt: f64, zero_tail: u32, discrete: bool) -> Function {
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

    // val = table[min(block, m - 1)], divided by the block's slat count only for a
    // continuous belt (a discrete one puts the whole entry on one slat).
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
    if !discrete {
        f.instruction(&Ins::LocalGet(IE_J));
        f.instruction(&Ins::LocalGet(IE_I));
        f.instruction(&Ins::I32Sub);
        f.instruction(&Ins::F64ConvertI32S);
        f.instruction(&Ins::F64Div);
    }
    f.instruction(&Ins::LocalSet(IE_VAL));

    // for k in i..j: content[k] = val  (continuous), or the whole entry at the
    // block's deepest slat `j - 1` and `+0.0` elsewhere (discrete).
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
    if discrete {
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::LocalGet(IE_K));
        f.instruction(&Ins::LocalGet(IE_J));
        f.instruction(&Ins::I32Const(1));
        f.instruction(&Ins::I32Sub);
        f.instruction(&Ins::I32Eq);
        f.instruction(&Ins::Select);
    }
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

// `b_merge_front(d: i32)`
const MF_D: u32 = 0;
const MF_I0: u32 = 1;
const MF_I1: u32 = 2;
const MF_A0: u32 = 3;
const MF_A1: u32 = 4;
const MF_W: u32 = 5;

/// `b_merge_front(d)`: the HELD-exit shift (§4.3 step 5). Slat 0 stays put and slat 1
/// merges into it, so material accumulates at the exit while the destination belt is
/// arrested.
///
/// The VM writes `slats[0] += slats[1]` and then `VecDeque::remove(1)`, shifting every
/// deeper slat down. The identical resulting sequence is reached here by writing the
/// sum into slat 1 and popping slat 0 -- one head bump instead of an O(len) shift --
/// with the addition emitted as `s0 + s1` so even the operand order matches.
///
/// A belt of one slat (or none) does nothing: the VM's `if self.slats.len() > 1` guard.
/// The `content`/`leak_basis`/`leak_window` fields the VM sums are the first
/// `1 + 2 * n_leaks` words of a slat, and the loop runs to `stride` -- summing the
/// trailing `shed_by` scratch too, which is harmless because `emit_leak_slat_linear`
/// rewrites that row before it is read, every step, on every live slat.
fn emit_merge_front(pop_front: u32) -> Function {
    let mut f = Function::new([(5, ValType::I32)]);

    f.instruction(&Ins::LocalGet(MF_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_LEN)));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32LeS);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(MF_I0));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::LocalSet(MF_I1));
    push_slat_addr(&mut f, MF_D, MF_I0);
    f.instruction(&Ins::LocalSet(MF_A0));
    push_slat_addr(&mut f, MF_D, MF_I1);
    f.instruction(&Ins::LocalSet(MF_A1));

    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(MF_W));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MF_W));
    f.instruction(&Ins::LocalGet(MF_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_STRIDE)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&Ins::LocalGet(MF_A1));
    f.instruction(&Ins::LocalGet(MF_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(MF_A0));
    f.instruction(&Ins::LocalGet(MF_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(MF_A1));
    f.instruction(&Ins::LocalGet(MF_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(MF_W));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MF_W));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::LocalGet(MF_D));
    f.instruction(&Ins::Call(pop_front));

    f.instruction(&Ins::End);
    f
}

// `b_merge_blocks(d: i32)`
const MB_D: u32 = 0;
const MB_LEN: u32 = 1;
const MB_I: u32 = 2;
const MB_J: u32 = 3;
const MB_T: u32 = 4;
const MB_W: u32 = 5;
const MB_DEEP: u32 = 6;
const MB_B: u32 = 7;
const MB_SUM: u32 = 8;

/// `b_merge_blocks(d)`: `ConveyorState::merge_time_unit_blocks` (§6.4 rule 3). A
/// DISCRETE belt lumps each time-unit block's material at the block's deepest
/// (last-entered) slat instead of spreading it, so material arrives at the exit in
/// whole time-unit pulses. Slat `i` belongs to block `floor(i * dt)`.
///
/// Merging is exact rather than approximate: content and the linear-leak schedule are
/// both additive (§6.2), so the block's cohorts sum field-wise and the belt stays at
/// the same equilibrium. Both init fills call it as their tail.
///
/// Blocks are contiguous and `floor(i * dt)` is monotone in `i`, so the deepest slat is
/// just `j - 1` where `j` opens the next block -- no `deepest[]` table. The per-word
/// loop sums in ASCENDING slat order from `+0.0`, matching the VM's
/// `merged[deepest].content += slat.content` over `slats[i..j]`, and it strides over
/// `stride` rather than `1 + 2 * n_leaks` so the `shed_by` scratch (which the VM's
/// `Slat::empty` has no analogue of) is zeroed alongside the fields that matter.
///
/// `block_of` is compared as an `f64` floor rather than the VM's `as i64`, which
/// differs only for an `i * dt` beyond `i64`'s range -- unreachable, since `i` is
/// bounded by `conveyor::slat_bound()` and a belt with a `dt` near `f64::MAX` has no
/// slats to merge. `emit_init_explicit` makes the same trade for the same reason.
fn emit_merge_blocks(dt: f64) -> Function {
    let mut f = Function::new([(6, ValType::I32), (2, ValType::F64)]);

    load_desc_i32(&mut f, MB_D, B_LEN, MB_LEN);
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(MB_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MB_I));
    f.instruction(&Ins::LocalGet(MB_LEN));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    // b = floor(i * dt); j = the first slat of the NEXT block.
    f.instruction(&Ins::LocalGet(MB_I));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&f64_const(dt));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::LocalSet(MB_B));
    f.instruction(&Ins::LocalGet(MB_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MB_J));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MB_J));
    f.instruction(&Ins::LocalGet(MB_LEN));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(MB_J));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&f64_const(dt));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::LocalGet(MB_B));
    f.instruction(&Ins::F64Eq);
    f.instruction(&Ins::I32Eqz);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(MB_J));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MB_J));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // inner loop
    f.instruction(&Ins::End); // inner block

    f.instruction(&Ins::LocalGet(MB_J));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(MB_DEEP));

    // Per slat word: sum the block, zero every slat but the deepest, write the sum
    // there. The sum is read out before any store lands, so the deepest slat's own
    // contribution is included exactly once.
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(MB_W));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MB_W));
    f.instruction(&Ins::LocalGet(MB_D));
    f.instruction(&Ins::I32Load(i32_memarg(B_STRIDE)));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));

    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(MB_SUM));
    f.instruction(&Ins::LocalGet(MB_I));
    f.instruction(&Ins::LocalSet(MB_T));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MB_T));
    f.instruction(&Ins::LocalGet(MB_J));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    f.instruction(&Ins::LocalGet(MB_SUM));
    push_slat_addr(&mut f, MB_D, MB_T);
    f.instruction(&Ins::LocalGet(MB_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalSet(MB_SUM));
    f.instruction(&Ins::LocalGet(MB_T));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MB_T));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // sum loop
    f.instruction(&Ins::End); // sum block

    f.instruction(&Ins::LocalGet(MB_I));
    f.instruction(&Ins::LocalSet(MB_T));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(MB_T));
    f.instruction(&Ins::LocalGet(MB_DEEP));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    push_slat_addr(&mut f, MB_D, MB_T);
    f.instruction(&Ins::LocalGet(MB_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(MB_T));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MB_T));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // zero loop
    f.instruction(&Ins::End); // zero block

    push_slat_addr(&mut f, MB_D, MB_DEEP);
    f.instruction(&Ins::LocalGet(MB_W));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalGet(MB_SUM));
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(MB_W));
    f.instruction(&Ins::I32Const(SLOT_BYTES));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(MB_W));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // word loop
    f.instruction(&Ins::End); // word block

    f.instruction(&Ins::LocalGet(MB_J));
    f.instruction(&Ins::LocalSet(MB_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // block loop
    f.instruction(&Ins::End); // block block

    f.instruction(&Ins::End);
    f
}

// `b_round(x: f64) -> f64`
const RD_X: u32 = 0;
const RD_T: u32 = 1;
const RD_F: u32 = 2;

/// `b_round(x) -> f64`: Rust's `f64::round`, i.e. round-half-AWAY-from-zero.
///
/// wasm's `f64.nearest` is round-half-to-EVEN, so it disagrees at every `x.5`:
/// `2.5.round()` is 3 but `f64.nearest(2.5)` is 2. `conveyor_time_unit` rounds the
/// step index `(time - start) / dt`, which lands exactly on `k.5` whenever a modeler
/// picks a `dt` twice the reporting interval -- so this is not a theoretical corner.
///
/// `floor(x + 0.5)` is NOT a substitute: `x = 0.49999999999999994` (the largest f64
/// below 0.5) rounds `x + 0.5` up to exactly 1.0, giving 1 where Rust gives 0.
/// `x - trunc(x)` is exact for every finite `x`, so the fraction test is too.
fn emit_round() -> Function {
    let mut f = Function::new([(2, ValType::F64)]);

    f.instruction(&Ins::LocalGet(RD_X));
    f.instruction(&Ins::F64Trunc);
    f.instruction(&Ins::LocalSet(RD_T));
    f.instruction(&Ins::LocalGet(RD_X));
    f.instruction(&Ins::LocalGet(RD_T));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(RD_F));

    // select(t + 1, select(t - 1, t, frac <= -0.5), frac >= 0.5). A non-finite `x`
    // has `t == x` and a NaN `frac`, so both comparisons are false and `x` passes
    // through -- `INFINITY.round() == INFINITY`, `NAN.round()` is NaN.
    f.instruction(&Ins::LocalGet(RD_T));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::LocalGet(RD_T));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalGet(RD_T));
    f.instruction(&Ins::LocalGet(RD_F));
    f.instruction(&f64_const(-0.5));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalGet(RD_F));
    f.instruction(&f64_const(0.5));
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::Select);

    f.instruction(&Ins::End);
    f
}

// `b_sat_i64(x: f64) -> i64`
const SI_X: u32 = 0;

/// `b_sat_i64(x) -> i64`: Rust's `x as i64`, which SATURATES at the bounds and maps
/// NaN to 0. wasm's `i64.trunc_f64_s` TRAPS on both, so the guards are not defensive
/// padding -- they are the semantics.
///
/// The three tests must precede the truncation as real branches, not `select` arms:
/// `select` evaluates both operands, so a `select(trunc(x), 0, x == x)` would still
/// trap on a NaN. `2^63` and `-2^63` are exactly representable, and `-2^63` is inside
/// `i64`'s range, so only the upper test uses `>=`.
fn emit_sat_i64() -> Function {
    let mut f = Function::new([]);

    f.instruction(&Ins::LocalGet(SI_X));
    f.instruction(&Ins::LocalGet(SI_X));
    f.instruction(&Ins::F64Ne); // x != x, i.e. x is NaN
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::I64Const(0));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::LocalGet(SI_X));
    f.instruction(&f64_const(9223372036854775808.0));
    f.instruction(&Ins::F64Ge);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::I64Const(i64::MAX));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::LocalGet(SI_X));
    f.instruction(&f64_const(-9223372036854775808.0));
    f.instruction(&Ins::F64Lt);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::I64Const(i64::MIN));
    f.instruction(&Ins::Return);
    f.instruction(&Ins::End);

    f.instruction(&Ins::LocalGet(SI_X));
    f.instruction(&Ins::I64TruncF64S);
    f.instruction(&Ins::End);
    f
}

// ── plan-specialized leak helpers ────────────────────────────────────────────
//
// Every one of these is emitted once per LEAKY belt, with the plan's zone bounds,
// leak-fraction slot offsets, `<leak_integers/>` mask, leak model, and leak count
// folded in as constants -- so the per-leak-flow loop the VM writes is unrolled
// here, and the only runtime loops are over the belt's dynamic slat count.

// `b_retained_{i}(len: i32, depth: i32)`
const RT_LEN: u32 = 0;
const RT_DEPTH: u32 = 1;
const RT_I: u32 = 2;
const RT_C: u32 = 3;
const RT_SHED: u32 = 4;
const RT_TMP: u32 = 5;

/// `b_retained_{i}(len, depth)`: `ConveyorState::zone_start_retained` (§5.1's `r_k`).
/// Writes `r[k]` for every leak flow into [`LeakRegions::r`].
///
/// It READS `m_entry[k]` rather than computing it, because every caller needs
/// `M_k(depth)` for its own arithmetic and would otherwise recompute the identical
/// count. The VM has the same duplication (`zone_start_retained`'s private `m_entry`
/// against `phase_b`'s and the init fills'); sharing one array here makes the two
/// provably the same numbers instead of two `zone_count_from` calls that happen to
/// agree.
///
/// Exponential leakage carries no per-cohort state, and the
/// `ignore_earlier_zone_losses` toggle defines every flow's fraction against the
/// inflowing amount, so both collapse to `r_k = 1` and skip the walk (§5.1's
/// staggered-zones paragraph).
fn emit_retained(plan: &ConveyorPlan, g0: usize, rg: LeakRegions, zf: ZoneFns) -> Function {
    let n = n_leaks(plan);
    let mut f = Function::new([(1, ValType::I32), (3, ValType::F64)]);

    if plan.exponential_leak || plan.ignore_earlier_zone_losses {
        for k in 0..n {
            store_static_f64(&mut f, rg.r(g0 + k), |f| {
                f.instruction(&f64_const(1.0));
            });
        }
        f.instruction(&Ins::End);
        return f;
    }

    for k in 0..n {
        store_static_f64(&mut f, rg.r(g0 + k), |f| {
            f.instruction(&f64_const(1.0));
        });
        store_static_i32(&mut f, rg.first_zone(g0 + k), |f| {
            f.instruction(&Ins::LocalGet(RT_LEN));
            f.instruction(&Ins::LocalGet(RT_DEPTH));
            f.instruction(&f64_const(zone_of(plan, k).0));
            f.instruction(&f64_const(zone_of(plan, k).1));
            f.instruction(&Ins::Call(zf.first_zone));
        });
    }

    // A unit cohort walks the entry path from the entry slat toward the exit,
    // shedding as it goes; `r[k]` snapshots what is left when it first enters flow
    // k's zone.
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::LocalSet(RT_C));
    f.instruction(&Ins::LocalGet(RT_DEPTH));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(RT_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(RT_I));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::BrIf(1));

    for k in 0..n {
        load_static_i32(&mut f, rg.first_zone(g0 + k));
        f.instruction(&Ins::LocalGet(RT_I));
        f.instruction(&Ins::I32Eq);
        f.instruction(&Ins::If(BlockType::Empty));
        store_static_f64(&mut f, rg.r(g0 + k), |f| {
            f.instruction(&Ins::LocalGet(RT_C));
        });
        f.instruction(&Ins::End);
    }

    // shed = Σ_k f_k * r_k / M_k, folded in ascending k from +0.0 like the VM's
    // `shed += ...` over `0..n`.
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(RT_SHED));
    for k in 0..n {
        push_in_zone(&mut f, zf, RT_I, RT_LEN, zone_of(plan, k));
        load_static_i32(&mut f, rg.m_entry(g0 + k));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32GtS);
        f.instruction(&Ins::I32And);
        f.instruction(&Ins::If(BlockType::Empty));
        f.instruction(&Ins::LocalGet(RT_SHED));
        emit_leak_frac(&mut f, plan, k, RT_TMP);
        load_static_f64(&mut f, rg.r(g0 + k));
        f.instruction(&Ins::F64Mul);
        load_static_i32(&mut f, rg.m_entry(g0 + k));
        f.instruction(&Ins::F64ConvertI32S);
        f.instruction(&Ins::F64Div);
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::LocalSet(RT_SHED));
        f.instruction(&Ins::End);
    }

    f.instruction(&Ins::LocalGet(RT_C));
    f.instruction(&Ins::LocalGet(RT_SHED));
    f.instruction(&Ins::F64Sub);
    emit_clamp_nonneg(&mut f, RT_TMP);
    f.instruction(&Ins::LocalSet(RT_C));

    f.instruction(&Ins::LocalGet(RT_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(RT_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

// `b_leak_{i}(d: i32)`
const LK_D: u32 = 0;
const LK_L: u32 = 1;
const LK_I: u32 = 2;
const LK_ADDR: u32 = 3;
const LK_J: u32 = 4;
const LK_C0: u32 = 5;
const LK_TOT: u32 = 6;
const LK_SC: u32 = 7;
const LK_SUM: u32 = 8;
const LK_T1: u32 = 9;
const LK_T2: u32 = 10;
const LK_T3: u32 = 11;
const LK_REM: u32 = 12;
const LK_WHOLE: u32 = 13;
const LK_FS: u32 = 14;

/// Push `in_zone(i, len, zone_k) && !leak_dest_arrested[k]` -- §4.3 step 2's
/// "effectively out of zone" test. A leak flow whose DESTINATION belt is arrested is
/// skipped entirely this step: its content stays, its travel window is not consumed,
/// and its reported volume is 0. Folding it into the zone test is exactly the VM's
/// `if !self.in_zone(..) || arrested(k) { continue }`.
///
/// A leak flow with no conveyor destination, or one whose destination carries no
/// `<arrest>` expression, compiles to the bare zone test.
fn push_in_zone_active(
    f: &mut Function,
    zf: ZoneFns,
    i_local: u32,
    len_local: u32,
    plan_leak: (f64, f64),
    arrest_off: Option<usize>,
) {
    push_in_zone(f, zf, i_local, len_local, plan_leak);
    if let Some(off) = arrest_off {
        emit_is_nonzero(f, off);
        f.instruction(&Ins::I32Eqz);
        f.instruction(&Ins::I32And);
    }
}

/// `b_leak_{i}(d)`: `ConveyorState::leak_step` (§4.3 step 2). Mutates slat contents
/// (and, for linear leakage, the travel windows), leaving each flow's leaked volume
/// in [`LeakRegions::lv`] for phase A to publish as a rate and phase B to charge
/// against the capacity room.
///
/// Term-for-term with the VM, including the two accumulation orders that matter:
/// `leak_vols[k]` folds over slats in EXIT-FIRST order, and the exponential arm's
/// `tot`/`sum_sheds` fold over flows in LISTED order, both seeded with `+0.0`. The
/// arrested-destination skip rides in [`push_in_zone_active`].
///
/// A VM quirk this mirrors rather than corrects (GH #947): `quantize_integer_leaks`
/// does NOT consult `leak_dest_arrested`, so an integer leak flow whose destination is
/// arrested still drains any whole units its never-resetting carry has accumulated (its
/// continuous shed was 0, so the undo loop adds back 0, but `floor(carry)` can still
/// be >= 1 when an earlier step's undelivered units were returned to it). The units are
/// not destroyed: an expanded conveyor stock is an ordinary INTEG, so the Stocks phase
/// credits them to the arrested belt's FLAT STOCK from the leak's driven rate, while its
/// phase B early-return keeps them off the slat ring -- a permanent divergence between
/// the two, since no exit can ever discharge material the ring never received. Emitting
/// the quantizer unconditionally reproduces that; skipping it would silently diverge from
/// the parity oracle. The fix belongs in `conveyor.rs` and here in ONE commit.
///
/// The `slat_vols` breakdown the VM also returns is not built: its only consumer is a
/// downstream `source` placement (§8), which [`reject_unsupported`] refuses. It has no
/// effect on belt state.
fn emit_leak(
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    dt: f64,
    zf: ZoneFns,
    leak_arrest: &[Option<usize>],
) -> Function {
    let n = n_leaks(plan);
    let mut f = Function::new([(4, ValType::I32), (10, ValType::F64)]);

    for k in 0..n {
        store_static_f64(&mut f, rg.lv(g0 + k), |f| {
            f.instruction(&f64_const(0.0));
        });
    }
    load_desc_i32(&mut f, LK_D, B_LEN, LK_L);
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(LK_I));

    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(LK_I));
    f.instruction(&Ins::LocalGet(LK_L));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    push_slat_addr(&mut f, LK_D, LK_I);
    f.instruction(&Ins::LocalSet(LK_ADDR));

    if plan.exponential_leak {
        emit_leak_slat_exponential(&mut f, plan, g0, rg, dt, zf, leak_arrest);
    } else {
        emit_leak_slat_linear(&mut f, plan, g0, rg, zf, leak_arrest);
    }

    f.instruction(&Ins::LocalGet(LK_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(LK_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    if !plan.exponential_leak {
        for k in 0..n {
            if let Some(j) = int_index(plan, k) {
                emit_quantize_integer_leak(&mut f, plan, g0, rg, zf, k, j);
            }
        }
    }

    f.instruction(&Ins::End);
    f
}

/// §5.2: every flow leaks from the SAME start-of-step content, so overlapping rates
/// add and the flows are order-independent; an over-drain scales them all down
/// proportionally. Emitted inside `b_leak_{i}`'s slat loop, with `LK_ADDR` live.
fn emit_leak_slat_exponential(
    f: &mut Function,
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    dt: f64,
    zf: ZoneFns,
    leak_arrest: &[Option<usize>],
) {
    let n = n_leaks(plan);

    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalSet(LK_C0));

    for (k, &arrest) in leak_arrest.iter().enumerate() {
        store_static_f64(f, rg.sheds(g0 + k), |f| {
            f.instruction(&Ins::LocalGet(LK_C0));
            emit_leak_frac(f, plan, k, LK_FS);
            f.instruction(&Ins::F64Mul);
            f.instruction(&f64_const(dt));
            f.instruction(&Ins::F64Mul);
            f.instruction(&f64_const(0.0));
            push_in_zone_active(f, zf, LK_I, LK_L, zone_of(plan, k), arrest);
            f.instruction(&Ins::Select);
        });
    }

    // tot = Σ sheds
    f.instruction(&f64_const(0.0));
    for k in 0..n {
        load_static_f64(f, rg.sheds(g0 + k));
        f.instruction(&Ins::F64Add);
    }
    f.instruction(&Ins::LocalSet(LK_TOT));

    // if tot > c0 && c0 > 0 { scale down } else if c0 <= 0 { drop everything }
    f.instruction(&Ins::LocalGet(LK_TOT));
    f.instruction(&Ins::LocalGet(LK_C0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::LocalGet(LK_C0));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::I32And);
    f.instruction(&Ins::If(BlockType::Empty));
    f.instruction(&Ins::LocalGet(LK_C0));
    f.instruction(&Ins::LocalGet(LK_TOT));
    f.instruction(&Ins::F64Div);
    f.instruction(&Ins::LocalSet(LK_SC));
    for k in 0..n {
        store_static_f64(f, rg.sheds(g0 + k), |f| {
            load_static_f64(f, rg.sheds(g0 + k));
            f.instruction(&Ins::LocalGet(LK_SC));
            f.instruction(&Ins::F64Mul);
        });
    }
    f.instruction(&Ins::Else);
    f.instruction(&Ins::LocalGet(LK_C0));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::If(BlockType::Empty));
    for k in 0..n {
        store_static_f64(f, rg.sheds(g0 + k), |f| {
            f.instruction(&f64_const(0.0));
        });
    }
    f.instruction(&Ins::End);
    f.instruction(&Ins::End);

    // The VM re-sums after the rescale rather than scaling `tot`, so we do too.
    f.instruction(&f64_const(0.0));
    for k in 0..n {
        load_static_f64(f, rg.sheds(g0 + k));
        f.instruction(&Ins::F64Add);
    }
    f.instruction(&Ins::LocalSet(LK_SUM));

    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::LocalGet(LK_C0));
    f.instruction(&Ins::LocalGet(LK_SUM));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Store(memarg(0)));

    for k in 0..n {
        store_static_f64(f, rg.lv(g0 + k), |f| {
            load_static_f64(f, rg.lv(g0 + k));
            load_static_f64(f, rg.sheds(g0 + k));
            f.instruction(&Ins::F64Add);
        });
    }
}

/// §5.1: flows leak in LISTED order with priority -- each sees the content its
/// predecessors left, which is exactly isee's "later leakages may get less than their
/// leak fraction suggests" when the fractions sum above 1. The travel window is
/// consumed by in-zone travel whatever the current fraction is. Emitted inside
/// `b_leak_{i}`'s slat loop, with `LK_ADDR` live.
fn emit_leak_slat_linear(
    f: &mut Function,
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    zf: ZoneFns,
    leak_arrest: &[Option<usize>],
) {
    let n = n_leaks(plan);

    // The `shed_by` row starts at 0 for EVERY live slat, matching the VM's fresh
    // `vec![0.0; l * n]`: `quantize_integer_leaks`' undo loop adds the row back for
    // all `0..l`, out-of-zone slats included -- and, since the quantizer ignores
    // `leak_dest_arrested`, for an arrested flow's slats too.
    for j in 0..shed_words(plan) {
        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&f64_const(0.0));
        f.instruction(&Ins::F64Store(memarg(shed_off(n, j))));
    }

    for (k, &arrest) in leak_arrest.iter().enumerate() {
        push_in_zone_active(f, zf, LK_I, LK_L, zone_of(plan, k), arrest);
        f.instruction(&Ins::If(BlockType::Empty));

        // use = min(basis, window); shed = min(f * use, content)
        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&Ins::F64Load(memarg(basis_off(k))));
        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&Ins::F64Load(memarg(window_off(n, k))));
        f.instruction(&Ins::LocalTee(LK_T2));
        f.instruction(&Ins::Call(zf.fmin));
        f.instruction(&Ins::LocalSet(LK_T1));
        emit_leak_frac(f, plan, k, LK_FS);
        f.instruction(&Ins::LocalGet(LK_T1));
        f.instruction(&Ins::F64Mul);
        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&Ins::F64Load(memarg(0)));
        f.instruction(&Ins::Call(zf.fmin));
        f.instruction(&Ins::LocalSet(LK_T3));

        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&Ins::F64Load(memarg(0)));
        f.instruction(&Ins::LocalGet(LK_T3));
        f.instruction(&Ins::F64Sub);
        f.instruction(&Ins::F64Store(memarg(0)));

        f.instruction(&Ins::LocalGet(LK_ADDR));
        f.instruction(&Ins::LocalGet(LK_T2));
        f.instruction(&Ins::LocalGet(LK_T1));
        f.instruction(&Ins::F64Sub);
        f.instruction(&Ins::F64Store(memarg(window_off(n, k))));

        store_static_f64(f, rg.lv(g0 + k), |f| {
            load_static_f64(f, rg.lv(g0 + k));
            f.instruction(&Ins::LocalGet(LK_T3));
            f.instruction(&Ins::F64Add);
        });

        if let Some(j) = int_index(plan, k) {
            f.instruction(&Ins::LocalGet(LK_ADDR));
            f.instruction(&Ins::LocalGet(LK_T3));
            f.instruction(&Ins::F64Store(memarg(shed_off(n, j))));
        }

        f.instruction(&Ins::End);
    }
}

/// §5.4 `<leak_integers/>` for flow `k` (whose `shed_by` column is `j`):
/// `ConveyorState::quantize_integer_leaks`. Undo the continuous shed (it existed only
/// to get priority and window consumption right), accumulate it into the flow's
/// never-resetting carry, then remove `floor(carry)` whole units exit-most in-zone
/// slat first, returning undelivered units to the carry.
///
/// The window was already consumed by travel above, independent of this
/// quantization, so the carry redistributes TIMING without changing any cohort's
/// schedule. Emitted after `b_leak_{i}`'s slat loop, once per integer flow in listed
/// order (the multi-integer-flow interaction is a simlin-defined corner, so listed
/// order is normative, not incidental).
fn emit_quantize_integer_leak(
    f: &mut Function,
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    zf: ZoneFns,
    k: usize,
    j: usize,
) {
    let n = n_leaks(plan);

    // Undo: content[i] += shed_by[i][k] for every live slat.
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(LK_J));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(LK_J));
    f.instruction(&Ins::LocalGet(LK_L));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    push_slat_addr(f, LK_D, LK_J);
    f.instruction(&Ins::LocalSet(LK_ADDR));
    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::F64Load(memarg(shed_off(n, j))));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(LK_J));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(LK_J));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    // carry += leak_vols[k]; whole = floor(carry); carry -= whole
    store_static_f64(f, rg.carry(g0 + k), |f| {
        load_static_f64(f, rg.carry(g0 + k));
        load_static_f64(f, rg.lv(g0 + k));
        f.instruction(&Ins::F64Add);
    });
    load_static_f64(f, rg.carry(g0 + k));
    f.instruction(&Ins::F64Floor);
    f.instruction(&Ins::LocalSet(LK_WHOLE));
    store_static_f64(f, rg.carry(g0 + k), |f| {
        load_static_f64(f, rg.carry(g0 + k));
        f.instruction(&Ins::LocalGet(LK_WHOLE));
        f.instruction(&Ins::F64Sub);
    });

    // Remove `whole` units, exit-most in-zone slat first, clamped per slat.
    f.instruction(&Ins::LocalGet(LK_WHOLE));
    f.instruction(&Ins::LocalSet(LK_REM));
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(LK_J));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(LK_J));
    f.instruction(&Ins::LocalGet(LK_L));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    // `remaining <= 0.0` breaks; a NaN `remaining` does NOT (it compares false), so
    // the loop runs to the end exactly as the VM's `if remaining <= 0.0 { break }`.
    f.instruction(&Ins::LocalGet(LK_REM));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Le);
    f.instruction(&Ins::BrIf(1));
    push_in_zone(f, zf, LK_J, LK_L, zone_of(plan, k));
    f.instruction(&Ins::If(BlockType::Empty));
    push_slat_addr(f, LK_D, LK_J);
    f.instruction(&Ins::LocalSet(LK_ADDR));
    f.instruction(&Ins::LocalGet(LK_REM));
    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::Call(zf.fmin));
    f.instruction(&Ins::LocalSet(LK_T3));
    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::LocalGet(LK_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(LK_T3));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::F64Store(memarg(0)));
    f.instruction(&Ins::LocalGet(LK_REM));
    f.instruction(&Ins::LocalGet(LK_T3));
    f.instruction(&Ins::F64Sub);
    f.instruction(&Ins::LocalSet(LK_REM));
    f.instruction(&Ins::End); // if
    f.instruction(&Ins::LocalGet(LK_J));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(LK_J));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    // Undelivered units go back to the carry; the reported volume is what left.
    store_static_f64(f, rg.carry(g0 + k), |f| {
        load_static_f64(f, rg.carry(g0 + k));
        f.instruction(&Ins::LocalGet(LK_REM));
        f.instruction(&Ins::F64Add);
    });
    store_static_f64(f, rg.lv(g0 + k), |f| {
        f.instruction(&Ins::LocalGet(LK_WHOLE));
        f.instruction(&Ins::LocalGet(LK_REM));
        f.instruction(&Ins::F64Sub);
    });
}

// `b_insert_{i}(d: i32, share: f64)`
const IN_D: u32 = 0;
const IN_SHARE: u32 = 1;
const IN_BL: u32 = 2;
const IN_DEPTH: u32 = 3;
const IN_ADDR: u32 = 4;
const IN_IDX: u32 = 5;
const IN_B: u32 = 6;

/// `b_insert_{i}(d, share)`: `phase_b`'s step-6 insert for a leaky belt. The admitted
/// cohort lands whole at the entry slat `d - 1` (the `beginning` placement, the only
/// one in scope) and gains the §5.1 schedule `basis_k = A * r_k / M_k(d)`,
/// `window_k = basis_k * min(M_k(d_c), M_k(d))`.
///
/// The VM's `min(M_k(d_c), M_k(d))` collapses to `M_k(d)` here, exactly and not
/// approximately: the running prefix `m_own[k]` it compares against is
/// `zone_count_from(k, belt_len, i + 1)` at the share's slat, and the only non-zero
/// share sits at `i = d - 1`, so `m_own[k] == M_k(d)`. A `dest` share landing on a
/// stale-tail slat beyond `d` is the case the `min` exists for, and `dest` is
/// rejected.
///
/// STEP-3 TRAP. That substitution is not merely unproven for the other placements --
/// it is WRONG for them, and silently so. `Even` and `Dist` (§8) spread the admitted
/// volume over slats `i < d - 1`, where the running prefix `m_own[k] =
/// zone_count_from(k, belt_len, i + 1)` is strictly LESS than `m_entry[k] = M_k(d)`
/// whenever the zone reaches deeper than slat `i`. Substituting `m_entry` there
/// over-credits every such cohort's travel window, so it leaks for longer than its
/// schedule allows. Whoever lowers a non-`beginning` placement must restore the real
/// running prefix (a per-slat `b_zone_count` call inside the spread loop), not just
/// widen [`reject_unsupported`]. The single-slat argument above is the ONLY thing
/// holding this up.
///
/// Zone membership -- hence `r_k` and `M_k(d)` -- is measured against the belt's
/// PHYSICAL length after the step-6 extension, which is `>= d` and can exceed it
/// after a transit shrink. `b_grow_to_d` therefore runs before this, not after.
fn emit_insert(
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    zf: ZoneFns,
    retained: u32,
) -> Function {
    let n = n_leaks(plan);
    let mut f = Function::new([(4, ValType::I32), (1, ValType::F64)]);

    load_desc_i32(&mut f, IN_D, B_LEN, IN_BL);
    load_desc_i32(&mut f, IN_D, B_D, IN_DEPTH);
    f.instruction(&Ins::LocalGet(IN_DEPTH));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(IN_IDX));

    if plan.exponential_leak {
        // §5.2 carries no per-cohort state, so this arm only adds the share. The
        // trailing `+= 0.0` mirrors `phase_b`'s exponential arm, which normalizes a
        // stored `-0.0` basis/window to `+0.0`. A `-0.0` is genuinely reachable here
        // (`emit_init_steady_leaky` stores `E * ub[k]` with `ub[k] == 0.0` on an
        // exponential belt, so a negative initial volume signs the zero), and the VM
        // performs the same add -- but no exponential-mode code path ever READS a leak
        // schedule (`emit_leak_slat_exponential` reads only `content`), so deleting
        // this loop is a genuinely equivalent mutant, exactly like `emit_shrink`'s
        // call. It is kept so the two backends' slat words stay bit-identical and a
        // future reader of these words never has to reason about which zero is stored.
        push_slat_addr(&mut f, IN_D, IN_IDX);
        f.instruction(&Ins::LocalSet(IN_ADDR));
        emit_add_share(&mut f);
        for k in 0..n {
            for off in [basis_off(k), window_off(n, k)] {
                f.instruction(&Ins::LocalGet(IN_ADDR));
                f.instruction(&Ins::LocalGet(IN_ADDR));
                f.instruction(&Ins::F64Load(memarg(off)));
                f.instruction(&f64_const(0.0));
                f.instruction(&Ins::F64Add);
                f.instruction(&Ins::F64Store(memarg(off)));
            }
        }
        f.instruction(&Ins::End);
        return f;
    }

    for k in 0..n {
        store_static_i32(&mut f, rg.m_entry(g0 + k), |f| {
            f.instruction(&Ins::LocalGet(IN_BL));
            f.instruction(&Ins::LocalGet(IN_DEPTH));
            f.instruction(&f64_const(zone_of(plan, k).0));
            f.instruction(&f64_const(zone_of(plan, k).1));
            f.instruction(&Ins::Call(zf.zone_count));
        });
    }
    f.instruction(&Ins::LocalGet(IN_BL));
    f.instruction(&Ins::LocalGet(IN_DEPTH));
    f.instruction(&Ins::Call(retained));

    push_slat_addr(&mut f, IN_D, IN_IDX);
    f.instruction(&Ins::LocalSet(IN_ADDR));
    emit_add_share(&mut f);

    for k in 0..n {
        // b = M_k > 0 ? share * r_k / M_k : 0. The division runs unconditionally (a
        // zero divisor yields an infinity or NaN, never a trap) and the select
        // discards it, mirroring the VM's `if m_entry[k] > 0 { .. } else { 0.0 }`.
        f.instruction(&Ins::LocalGet(IN_SHARE));
        load_static_f64(&mut f, rg.r(g0 + k));
        f.instruction(&Ins::F64Mul);
        load_static_i32(&mut f, rg.m_entry(g0 + k));
        f.instruction(&Ins::F64ConvertI32S);
        f.instruction(&Ins::F64Div);
        f.instruction(&f64_const(0.0));
        load_static_i32(&mut f, rg.m_entry(g0 + k));
        f.instruction(&Ins::I32Const(0));
        f.instruction(&Ins::I32GtS);
        f.instruction(&Ins::Select);
        f.instruction(&Ins::LocalSet(IN_B));

        f.instruction(&Ins::LocalGet(IN_ADDR));
        f.instruction(&Ins::LocalGet(IN_ADDR));
        f.instruction(&Ins::F64Load(memarg(basis_off(k))));
        f.instruction(&Ins::LocalGet(IN_B));
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::F64Store(memarg(basis_off(k))));

        f.instruction(&Ins::LocalGet(IN_ADDR));
        f.instruction(&Ins::LocalGet(IN_ADDR));
        f.instruction(&Ins::F64Load(memarg(window_off(n, k))));
        f.instruction(&Ins::LocalGet(IN_B));
        load_static_i32(&mut f, rg.m_entry(g0 + k));
        f.instruction(&Ins::F64ConvertI32S);
        f.instruction(&Ins::F64Mul);
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::F64Store(memarg(window_off(n, k))));
    }

    f.instruction(&Ins::End);
    f
}

/// `slats[d-1].content += share`, with `IN_ADDR` already holding the entry slat.
fn emit_add_share(f: &mut Function) {
    f.instruction(&Ins::LocalGet(IN_ADDR));
    f.instruction(&Ins::LocalGet(IN_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(IN_SHARE));
    f.instruction(&Ins::F64Add);
    f.instruction(&Ins::F64Store(memarg(0)));
}

// `b_init_steady_{i}(d: i32, v: f64)`
const IL_D: u32 = 0;
const IL_V: u32 = 1;
const IL_N: u32 = 2;
const IL_I: u32 = 3;
const IL_IM1: u32 = 4;
const IL_ADDR: u32 = 5;
const IL_SHED: u32 = 6;
const IL_S: u32 = 7;
const IL_E: u32 = 8;
const IL_CI: u32 = 9;
const IL_BAS: u32 = 10;
const IL_TMP: u32 = 11;
const IL_FS: u32 = 12;

/// `b_init_steady_{i}(d, v)`: the LEAK-AWARE §7.1 steady fill, `ConveyorState::init_steady`.
///
/// Three passes over the belt, exactly as the spec's algorithm reads:
///
/// 1. Walk a unit cohort from the entry slat to the exit under the §5 per-DT leak
///    rules, leaving the retained profile `c[i]` in the slats' CONTENT words. The
///    slats double as the VM's temporary `c` vector -- pass 3 overwrites them --
///    which is why no scratch buffer is needed for a belt of unbounded length.
/// 2. `S = Σ c[i]` (`b_total`, exit-first like `c.iter().sum()`), then the cohort
///    scale `E = V / S`, or 0 when `S <= 0` (a NaN `S` takes the same arm).
/// 3. Rewrite each slat as a cohort of entering volume `E` that has already traveled
///    to position `i`: `content = E * c[i]`, `basis_k = E * r_k / M_k(N)` -- the same
///    for every slat, since every slat's cohort entered at the entry -- and
///    `window_k = basis_k *` (its in-zone slats from the entry to here). That last
///    count is `zone_count_from(k, N, i + 1)`, accumulated as a running prefix rather
///    than rescanned per slat: integer counting, so bit-identical to the VM's
///    quadratic form.
fn emit_init_steady_leaky(
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    dt: f64,
    zf: ZoneFns,
    retained: u32,
) -> Function {
    let n = n_leaks(plan);
    let mut f = Function::new([(4, ValType::I32), (7, ValType::F64)]);

    load_desc_i32(&mut f, IL_D, B_D, IL_N);

    // c[N-1] = 1.0 at the entry slat.
    f.instruction(&Ins::LocalGet(IL_N));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(IL_I));
    push_slat_addr(&mut f, IL_D, IL_I);
    f.instruction(&Ins::LocalSet(IL_ADDR));
    f.instruction(&Ins::LocalGet(IL_ADDR));
    f.instruction(&f64_const(1.0));
    f.instruction(&Ins::F64Store(memarg(0)));

    emit_entry_path_basis(&mut f, plan, g0, rg, zf, retained, IL_N);

    // Pass 1: c[i-1] = max(0, c[i] - shed(i)), walking entry -> exit.
    f.instruction(&Ins::LocalGet(IL_N));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(IL_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IL_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32LtS);
    f.instruction(&Ins::BrIf(1));
    push_slat_addr(&mut f, IL_D, IL_I);
    f.instruction(&Ins::LocalSet(IL_ADDR));

    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalSet(IL_SHED));
    for k in 0..n {
        push_in_zone(&mut f, zf, IL_I, IL_N, zone_of(plan, k));
        f.instruction(&Ins::If(BlockType::Empty));
        f.instruction(&Ins::LocalGet(IL_SHED));
        if plan.exponential_leak {
            f.instruction(&Ins::LocalGet(IL_ADDR));
            f.instruction(&Ins::F64Load(memarg(0)));
            emit_leak_frac(&mut f, plan, k, IL_FS);
            f.instruction(&Ins::F64Mul);
            f.instruction(&f64_const(dt));
            f.instruction(&Ins::F64Mul);
        } else {
            emit_leak_frac(&mut f, plan, k, IL_FS);
            load_static_f64(&mut f, rg.ub(g0 + k));
            f.instruction(&Ins::F64Mul);
        }
        f.instruction(&Ins::F64Add);
        f.instruction(&Ins::LocalSet(IL_SHED));
        f.instruction(&Ins::End);
    }

    f.instruction(&Ins::LocalGet(IL_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(IL_IM1));
    push_slat_addr(&mut f, IL_D, IL_IM1);
    f.instruction(&Ins::LocalGet(IL_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalGet(IL_SHED));
    f.instruction(&Ins::F64Sub);
    emit_clamp_nonneg(&mut f, IL_TMP);
    f.instruction(&Ins::F64Store(memarg(0)));

    f.instruction(&Ins::LocalGet(IL_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Sub);
    f.instruction(&Ins::LocalSet(IL_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    // Pass 2: E = S > 0 ? V / S : 0.
    f.instruction(&Ins::LocalGet(IL_D));
    f.instruction(&Ins::Call(zf.total));
    f.instruction(&Ins::LocalSet(IL_S));
    f.instruction(&Ins::LocalGet(IL_V));
    f.instruction(&Ins::LocalGet(IL_S));
    f.instruction(&Ins::F64Div);
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::LocalGet(IL_S));
    f.instruction(&f64_const(0.0));
    f.instruction(&Ins::F64Gt);
    f.instruction(&Ins::Select);
    f.instruction(&Ins::LocalSet(IL_E));

    // Pass 3: scale the profile and hang the entry cohort's schedule on each slat.
    for k in 0..n {
        store_static_i32(&mut f, rg.prefix(g0 + k), |f| {
            f.instruction(&Ins::I32Const(0));
        });
    }
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(IL_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(IL_I));
    f.instruction(&Ins::LocalGet(IL_N));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    emit_bump_prefix(&mut f, plan, g0, rg, zf, IL_I, IL_N);

    push_slat_addr(&mut f, IL_D, IL_I);
    f.instruction(&Ins::LocalSet(IL_ADDR));
    f.instruction(&Ins::LocalGet(IL_ADDR));
    f.instruction(&Ins::F64Load(memarg(0)));
    f.instruction(&Ins::LocalSet(IL_CI));
    // The ring is bump-allocated, never zero on reuse, so a slat's leak words must be
    // WRITTEN before they are read. `b_zero_tail` covers the integer-leak scratch
    // (which the loop below does not write); the basis/window pairs follow.
    f.instruction(&Ins::LocalGet(IL_D));
    f.instruction(&Ins::LocalGet(IL_ADDR));
    f.instruction(&Ins::Call(zf.zero_tail));
    f.instruction(&Ins::LocalGet(IL_ADDR));
    f.instruction(&Ins::LocalGet(IL_E));
    f.instruction(&Ins::LocalGet(IL_CI));
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Store(memarg(0)));
    for k in 0..n {
        f.instruction(&Ins::LocalGet(IL_E));
        load_static_f64(&mut f, rg.ub(g0 + k));
        f.instruction(&Ins::F64Mul);
        f.instruction(&Ins::LocalSet(IL_BAS));
        emit_store_schedule(&mut f, rg, g0, k, n, IL_ADDR, IL_BAS);
    }

    f.instruction(&Ins::LocalGet(IL_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(IL_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

// `b_sched_{i}(d: i32)`
const SD_D: u32 = 0;
const SD_N: u32 = 1;
const SD_I: u32 = 2;
const SD_ADDR: u32 = 3;
const SD_BAS: u32 = 4;

/// `b_sched_{i}(d)`: `ConveyorState::fill_slats`' schedule half (§7.2), run after the
/// generic `b_init_explicit` has laid the list's contents into the slats.
///
/// Each filled slat is treated as a cohort that entered at the belt entry and traveled
/// to its position: `basis_k = content * r_k / M_k(N)` (the PER-CONTENT unit basis,
/// which is why it scales by the slat's own content rather than by a cohort scale) and
/// `window_k = basis_k *` its in-zone slats from the entry to here.
fn emit_sched(
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    zf: ZoneFns,
    retained: u32,
) -> Function {
    let n = n_leaks(plan);
    let mut f = Function::new([(3, ValType::I32), (1, ValType::F64)]);

    load_desc_i32(&mut f, SD_D, B_D, SD_N);
    emit_entry_path_basis(&mut f, plan, g0, rg, zf, retained, SD_N);

    for k in 0..n {
        store_static_i32(&mut f, rg.prefix(g0 + k), |f| {
            f.instruction(&Ins::I32Const(0));
        });
    }
    f.instruction(&Ins::I32Const(0));
    f.instruction(&Ins::LocalSet(SD_I));
    f.instruction(&Ins::Block(BlockType::Empty));
    f.instruction(&Ins::Loop(BlockType::Empty));
    f.instruction(&Ins::LocalGet(SD_I));
    f.instruction(&Ins::LocalGet(SD_N));
    f.instruction(&Ins::I32GeS);
    f.instruction(&Ins::BrIf(1));
    emit_bump_prefix(&mut f, plan, g0, rg, zf, SD_I, SD_N);

    push_slat_addr(&mut f, SD_D, SD_I);
    f.instruction(&Ins::LocalSet(SD_ADDR));
    for k in 0..n {
        f.instruction(&Ins::LocalGet(SD_ADDR));
        f.instruction(&Ins::F64Load(memarg(0)));
        load_static_f64(&mut f, rg.ub(g0 + k));
        f.instruction(&Ins::F64Mul);
        f.instruction(&Ins::LocalSet(SD_BAS));
        emit_store_schedule(&mut f, rg, g0, k, n, SD_ADDR, SD_BAS);
    }

    f.instruction(&Ins::LocalGet(SD_I));
    f.instruction(&Ins::I32Const(1));
    f.instruction(&Ins::I32Add);
    f.instruction(&Ins::LocalSet(SD_I));
    f.instruction(&Ins::Br(0));
    f.instruction(&Ins::End); // loop
    f.instruction(&Ins::End); // block

    f.instruction(&Ins::End);
    f
}

/// Both init fills' shared prologue: `M_k(N)` into [`LeakRegions::m_entry`], `r_k`
/// into [`LeakRegions::r`], and the per-unit-content entry-cohort basis
/// `ub_k = r_k / M_k(N)` into [`LeakRegions::ub`] (0 for exponential leakage, which
/// keeps no cohort state, and 0 for a zone too narrow to hold a slat at this DT).
/// `n_local` holds the belt's slat count.
fn emit_entry_path_basis(
    f: &mut Function,
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    zf: ZoneFns,
    retained: u32,
    n_local: u32,
) {
    let n = n_leaks(plan);
    for k in 0..n {
        store_static_i32(f, rg.m_entry(g0 + k), |f| {
            f.instruction(&Ins::LocalGet(n_local));
            f.instruction(&Ins::LocalGet(n_local));
            f.instruction(&f64_const(zone_of(plan, k).0));
            f.instruction(&f64_const(zone_of(plan, k).1));
            f.instruction(&Ins::Call(zf.zone_count));
        });
    }
    f.instruction(&Ins::LocalGet(n_local));
    f.instruction(&Ins::LocalGet(n_local));
    f.instruction(&Ins::Call(retained));

    for k in 0..n {
        store_static_f64(f, rg.ub(g0 + k), |f| {
            if plan.exponential_leak {
                f.instruction(&f64_const(0.0));
                return;
            }
            load_static_f64(f, rg.r(g0 + k));
            load_static_i32(f, rg.m_entry(g0 + k));
            f.instruction(&Ins::F64ConvertI32S);
            f.instruction(&Ins::F64Div);
            f.instruction(&f64_const(0.0));
            load_static_i32(f, rg.m_entry(g0 + k));
            f.instruction(&Ins::I32Const(0));
            f.instruction(&Ins::I32GtS);
            f.instruction(&Ins::Select);
        });
    }
}

/// `prefix[k] += in_zone(i, n, k)` for every flow, so that after slat `i` is visited
/// `prefix[k] == zone_count_from(k, n, i + 1)`.
fn emit_bump_prefix(
    f: &mut Function,
    plan: &ConveyorPlan,
    g0: usize,
    rg: LeakRegions,
    zf: ZoneFns,
    i_local: u32,
    n_local: u32,
) {
    for k in 0..n_leaks(plan) {
        push_in_zone(f, zf, i_local, n_local, zone_of(plan, k));
        f.instruction(&Ins::If(BlockType::Empty));
        store_static_i32(f, rg.prefix(g0 + k), |f| {
            load_static_i32(f, rg.prefix(g0 + k));
            f.instruction(&Ins::I32Const(1));
            f.instruction(&Ins::I32Add);
        });
        f.instruction(&Ins::End);
    }
}

/// `slat.leak_basis[k] = bas; slat.leak_window[k] = bas * prefix[k]` -- an init fill's
/// per-slat schedule write, shared by the §7.1 and §7.2 paths.
fn emit_store_schedule(
    f: &mut Function,
    rg: LeakRegions,
    g0: usize,
    k: usize,
    n: usize,
    addr_local: u32,
    bas_local: u32,
) {
    f.instruction(&Ins::LocalGet(addr_local));
    f.instruction(&Ins::LocalGet(bas_local));
    f.instruction(&Ins::F64Store(memarg(basis_off(k))));
    f.instruction(&Ins::LocalGet(addr_local));
    f.instruction(&Ins::LocalGet(bas_local));
    load_static_i32(f, rg.prefix(g0 + k));
    f.instruction(&Ins::F64ConvertI32S);
    f.instruction(&Ins::F64Mul);
    f.instruction(&Ins::F64Store(memarg(window_off(n, k))));
}

#[cfg(test)]
#[path = "belt_tests.rs"]
mod tests;
