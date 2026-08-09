// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Conveyor runtime engine: the per-DT slat-belt model.
//!
//! This is the pure functional core of XMILE conveyor support, a faithful Rust
//! transcription of `docs/design/conveyors.md` §4-§8 (and its executable
//! reference, `test/conveyors/reference_prototype.py`). It carries no dependency
//! on the VM, datamodel, or compiler: a [`ConveyorState`] owns one belt and is
//! driven each DT by scalar inputs the caller (the VM's conveyor pass, §9.3)
//! evaluates from ordinary equations. Every dynamic quantity -- transit time,
//! capacity, inflow limit, leak fractions, requested inflow rates, and the
//! arrest/sample conditions -- is passed in per step, so this module holds only
//! belt state and does no expression evaluation.
//!
//! The update is two-phase (§4.3): [`ConveyorState::phase_a`] leaks and exits
//! from each conveyor's own start-of-step state; [`ConveyorState::phase_b`]
//! admits inflow (including conveyor-driven inflow from another belt's Phase A
//! output), shifts, and inserts. Because no phase reads another conveyor's
//! same-phase results, conveyor chains and cycles need no topological ordering.

use std::collections::VecDeque;

/// Upper bound on a single belt's slat count (§4.1). The slat count
/// `round(transit/dt)` sizes the belt `Vec` (and its per-leak inner vecs); with
/// no bound a hostile or typo'd `<len>` blows up the allocation two ways: a
/// `usize`-saturating count (e.g. `1e300 / dt`) panics `vec![0.0; usize::MAX]`
/// with "capacity overflow" -- a host-process abort under libsimlin's
/// `panic = "abort"` release profile (wasm, pysimlin, serve) -- and a merely
/// enormous finite count (`1e12 / dt` -> ~4e12 slats -> ~32 TB) OOMs when
/// committed. 1,000,000 slats is far beyond any physically meaningful
/// `transit/dt` ratio yet trivially cheap to reject below, so a latched transit
/// exceeding it is rejected LOUDLY at belt init / latch time (see
/// `conveyor_compile::{init_belts, run_phase_a}` and
/// [`ErrorCode::ConveyorTransitTooLong`]) rather than silently saturating the
/// belt geometry.
///
/// [`ErrorCode::ConveyorTransitTooLong`]: crate::common::ErrorCode::ConveyorTransitTooLong
pub(crate) const MAX_SLATS_PER_BELT: usize = 1_000_000;

#[cfg(test)]
thread_local! {
    /// Test-only override of [`MAX_SLATS_PER_BELT`], scoped by an active
    /// [`SlatBoundGuard`]. Lets a test trip the slat-count gate with a tiny
    /// fixture instead of a production-sized belt (docs/dev/rust.md
    /// test-time budgets).
    static SLAT_BOUND_OVERRIDE: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// The per-belt slat-count bound for the current thread. Production returns
/// [`MAX_SLATS_PER_BELT`]; in a `#[cfg(test)]` build an active
/// [`SlatBoundGuard`] override takes precedence.
pub(crate) fn slat_bound() -> usize {
    #[cfg(test)]
    {
        if let Some(b) = SLAT_BOUND_OVERRIDE.with(|c| c.get()) {
            return b;
        }
    }
    MAX_SLATS_PER_BELT
}

/// RAII guard (test-only) that overrides [`slat_bound`] for the current thread
/// for the guard's lifetime, restoring the previous value on drop -- so a
/// panicking test never leaks the override to the next test reusing the thread.
#[cfg(test)]
pub(crate) struct SlatBoundGuard {
    prev: Option<usize>,
}

#[cfg(test)]
impl SlatBoundGuard {
    pub(crate) fn new(bound: usize) -> Self {
        let prev = SLAT_BOUND_OVERRIDE.with(|c| c.replace(Some(bound)));
        Self { prev }
    }
}

#[cfg(test)]
impl Drop for SlatBoundGuard {
    fn drop(&mut self) {
        SLAT_BOUND_OVERRIDE.with(|c| c.set(self.prev));
    }
}

/// Round `transit / dt` to the nearest slat count, half **away from zero**
/// (§4.1), clamped to at least one slat. `f64::floor(x + 0.5)` gives
/// away-from-zero rounding for the non-negative arguments a positive transit
/// time and DT produce (Rust `f64::round` is already half-away, but the
/// prototype pins `floor(x + 0.5)` and we match it bit-for-bit).
///
/// The result is unbounded above (a saturating `n as usize` for an enormous
/// `transit/dt`); callers that allocate a belt from it MUST first reject a
/// count exceeding `MAX_SLATS_PER_BELT` (the production path does so at every
/// latch site -- see that const's docs). This function stays a pure rounding
/// primitive so the bound can live in the imperative shell with the other §4.4
/// hygiene.
pub fn slat_count(transit: f64, dt: f64) -> usize {
    let n = (transit / dt + 0.5).floor();
    // `NaN as usize` is 0, which would later underflow a `d - 1` slat index.
    // A non-finite or sub-1 count clamps to a single slat (the VM enforces a
    // positive, finite transit time upstream, §4.4/§9.4; this is defense in
    // depth).
    if !n.is_finite() || n < 1.0 {
        1
    } else {
        n as usize
    }
}

/// Static per-leak-flow configuration. The leak *fraction* is dynamic (re-read
/// every DT, §5.1) and passed in per step; only the zone and the integer-unit
/// flag are fixed here.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub struct LeakConfig {
    /// Fractional belt position where the leak zone starts, from the entry side
    /// (§5.3). Default 0 (entry).
    pub zone_start: f64,
    /// Fractional belt position where the leak zone ends. Default 1 (exit).
    pub zone_end: f64,
    /// `<leak_integers/>`: leak only whole units (§5.4).
    pub integers: bool,
}

impl Default for LeakConfig {
    fn default() -> Self {
        LeakConfig {
            zone_start: 0.0,
            zone_end: 1.0,
            integers: false,
        }
    }
}

/// One slat of the belt. Index 0 is the exit (front); the back is the entry.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
struct Slat {
    /// Material in this slat.
    content: f64,
    /// Per leak flow: linear-leak volume per in-zone DT per unit of fraction
    /// (`A * r_k / M_k(d)`, fixed at insertion, §5.1).
    leak_basis: Vec<f64>,
    /// Per leak flow: remaining leakable basis-volume (travel window, §5.1).
    leak_window: Vec<f64>,
}

impl Slat {
    fn empty(n_leaks: usize) -> Slat {
        Slat {
            content: 0.0,
            leak_basis: vec![0.0; n_leaks],
            leak_window: vec![0.0; n_leaks],
        }
    }
}

/// Per-conveyor runtime state (§4.2). One instance per scalar conveyor (per
/// array element for arrayed conveyors).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct ConveyorState {
    dt: f64,
    exponential_leak: bool,
    discrete: bool,
    ignore_earlier_zone_losses: bool,
    leaks: Vec<LeakConfig>,

    /// The belt: index 0 = exit (front), back = entry.
    slats: VecDeque<Slat>,
    /// Transit time last sampled (§6).
    latched_transit: f64,
    /// Discrete per-time-unit `in_limit` budget spent since the last integer
    /// time boundary (§6.3); reset by [`ConveyorState::on_time_boundary`].
    in_carry: f64,
    /// Per leak-flow `<leak_integers/>` accumulator; never resets (§5.4).
    leak_carry: Vec<f64>,
    /// Per equation-driven-inflow discrete admission fractional-unit
    /// accumulator; never resets (§6.4 rule 1). Sized lazily on first Phase B.
    quant_carry: Vec<f64>,

    /// Start-of-step total contents, snapshotted at Phase A so Phase B's
    /// capacity room is measured against the pre-leak/pre-exit total (§4.3).
    step_contents0: f64,
}

/// Inputs to [`ConveyorState::phase_a`] for one conveyor this DT.
pub struct PhaseAInputs<'a> {
    /// `<arrest>` is nonzero this step: freeze the belt, zero every flow (§4.3
    /// step 0).
    pub arrested: bool,
    /// `<sample>` is nonzero this step: re-latch the transit time (§4.3 step 1).
    pub sample: bool,
    /// Current `<len>` value, used to re-latch when `sample` is true. Already
    /// hygiene-clamped by the caller per §4.4 (finite ⇒ `max(dt, value)`;
    /// non-finite ⇒ ignored by [`ConveyorState::phase_a`]).
    pub transit: f64,
    /// Current leak fraction per leak flow, re-read every DT (§5.1). Linear
    /// fractions clamped to `[0, 1]`, exponential rates to `[0, ∞)`, NaN ⇒ 0 by
    /// the caller (§4.4).
    pub leak_fractions: &'a [f64],
    /// This conveyor's primary-outflow destination is an arrested conveyor:
    /// hold the exit (§4.3 step 3).
    pub dest_arrested: bool,
    /// Per leak flow: its destination is an arrested conveyor, so skip it this
    /// step (rate 0, content stays, §4.3 step 2). Empty ⇒ none arrested.
    pub leak_dest_arrested: &'a [bool],
}

/// Phase A result: the volumes leaving the belt this DT.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct PhaseAResult {
    /// Primary-outflow volume this DT (0 when arrested or the exit is held).
    pub out_vol: f64,
    /// Per leak flow: volume leaked this DT.
    pub leak_vols: Vec<f64>,
    /// Per leak flow, per belt slat (index 0 = exit): the volume that leak flow
    /// shed from that slat this DT (so `leak_slat_vols[k]` sums to
    /// `leak_vols[k]`). This is the per-slat detail a downstream `source`
    /// placement mirrors (§8): the outer length is the leak-flow count, the
    /// inner length is the belt length at leak time (empty when arrested, since
    /// no leak ran). It is always populated for a non-arrested step, since the
    /// cost is proportional to the belt the conveyor already carries.
    pub leak_slat_vols: Vec<Vec<f64>>,
    /// This conveyor is arrested this step (Phase B is a no-op).
    pub arrested: bool,
    /// The exit is held (destination arrested): material accumulates at the
    /// exit slat instead of leaving (§4.3 steps 3/5).
    pub held: bool,
}

/// Inflow placement (§8 "spread inputs"). Selects where an admitted inflow's
/// volume `A` lands on the belt at insert time. `Dist` carries the per-entry-slat
/// weights `w_i` (i = 0..d-1, exit-first) the caller pre-evaluated from the
/// `<isee:distrib_eq>` graphical function or array; a caller that cannot resolve
/// them passes an empty vector, which falls back to `Beginning`.
// Debug is unconditional (not gated on debug-derive): ConveyorPlan/InflowPlan
// and their metas derive Debug unconditionally and embed a Placement.
#[derive(Clone, Debug, PartialEq)]
pub enum Placement {
    /// All of `A` at the entry slat (depth `d`). The XMILE default.
    Beginning,
    /// `A / d` at every entry-path slat `i ∈ 0..d-1` (incl. the exit slat).
    Even,
    /// `A × content_i / Σ content` over the whole physical belt; falls back to
    /// `Beginning` when the belt is empty.
    Dest,
    /// `A × w_i / Σ w` over `i ∈ 0..d-1`; falls back to `Beginning` when
    /// `Σ w == 0` (or the weight vector is empty / shorter than `d`).
    Dist(Vec<f64>),
}

/// Inputs to [`ConveyorState::phase_b`] for one conveyor this DT.
pub struct PhaseBInputs<'a> {
    /// The Phase A result for this same conveyor.
    pub phase_a: &'a PhaseAResult,
    /// Requested rate per equation-driven inflow, in listed order (§4.3 step 4
    /// apportions in this order). Already clamped to `max(0, rate)` by the
    /// caller (§4.4).
    pub eq_request_rates: &'a [f64],
    /// Conveyor-driven inflows as `(volume, placement)` pairs -- one per upstream
    /// Phase-A outflow/leak feeding this belt, each admitted unconditionally
    /// (§4.3) and placed by its own method (§8). Kept per-inflow rather than
    /// lumped so a `source`-placed leak inflow can carry its own mirrored weight
    /// vector independently of any sibling conveyor-driven inflow. Empty ⇒ none.
    pub conv_inflows: &'a [(f64, Placement)],
    /// Current leak fraction per leak flow (for the inserted cohort's schedule).
    pub leak_fractions: &'a [f64],
    /// Instantaneous capacity (material), `f64::INFINITY` if unconstrained.
    /// Applies to equation-driven inflow only. Negative ⇒ 0 by the caller.
    pub capacity: f64,
    /// Equation-driven inflow limit per **time unit**, `f64::INFINITY` if
    /// unconstrained.
    pub in_limit: f64,
    /// Placement per equation-driven inflow, aligned to `eq_request_rates`.
    /// Empty ⇒ every inflow uses `Beginning` (the default).
    pub placements: &'a [Placement],
}

/// Phase B result: the admitted inflow rates this DT.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct PhaseBResult {
    /// Total admitted inflow *volume* this DT (conveyor-driven + equation).
    pub admitted: f64,
    /// Per equation-driven inflow: admitted *volume* this DT.
    pub in_vols: Vec<f64>,
    /// Per equation-driven inflow: cleared *volume* (post cap/limit, pre discrete
    /// quantization) this DT. For a continuous conveyor this equals `in_vols`.
    pub cleared: Vec<f64>,
}

impl ConveyorState {
    /// Create an empty conveyor (no slats yet; call an `init_*` method before
    /// stepping). `leaks` lists the leak flows in outflow-list order.
    pub fn new(
        dt: f64,
        exponential_leak: bool,
        discrete: bool,
        ignore_earlier_zone_losses: bool,
        leaks: Vec<LeakConfig>,
    ) -> ConveyorState {
        let n = leaks.len();
        ConveyorState {
            dt,
            exponential_leak,
            discrete,
            ignore_earlier_zone_losses,
            leaks,
            slats: VecDeque::new(),
            latched_transit: 0.0,
            in_carry: 0.0,
            leak_carry: vec![0.0; n],
            quant_carry: Vec::new(),
            step_contents0: 0.0,
        }
    }

    /// The time step this conveyor integrates under. Test-only accessor.
    #[cfg(test)]
    pub fn dt(&self) -> f64 {
        self.dt
    }

    fn n_leaks(&self) -> usize {
        self.leaks.len()
    }

    /// Number of slats for the currently latched transit time (§4.1).
    fn n_slats(&self) -> usize {
        slat_count(self.latched_transit, self.dt)
    }

    /// Total material on the belt -- the conveyor variable's reported scalar
    /// value (§4.2).
    pub fn contents(&self) -> f64 {
        self.slats.iter().map(|s| s.content).sum()
    }

    /// Entry depth `d = round(latched_transit / dt)` (§4.1/§6): the depth at
    /// which the next inserted cohort lands. Equal to the `d` [`phase_b`] uses
    /// for placement, so a caller building a per-step `dist`/`source` weight
    /// vector (§8, weights indexed over `0..d`) reads it after [`phase_a`] has
    /// latched but before [`phase_b`] inserts.
    ///
    /// [`phase_a`]: ConveyorState::phase_a
    /// [`phase_b`]: ConveyorState::phase_b
    pub fn entry_depth(&self) -> usize {
        self.n_slats()
    }

    /// Content of slat `j` (0 = exit), or `None` if out of range. Container
    /// access (§10) reads the whole belt via [`Self::slat_contents`] +
    /// `container_value_from_slice`; this single-slat accessor remains for
    /// tests probing individual slats.
    #[cfg(test)]
    pub fn slat_content(&self, j: usize) -> Option<f64> {
        self.slats.get(j).map(|s| s.content)
    }

    /// The current per-slat content vector, exit-first, for array builtins over
    /// a conveyor's contents (§10).
    pub fn slat_contents(&self) -> Vec<f64> {
        self.slats.iter().map(|s| s.content).collect()
    }

    /// Per-equation-driven-inflow discrete admission carry (§6.4 rule 1). Used
    /// by the S12 oracle to check the `cleared_j == reported_j + carry_j`
    /// bookkeeping identity.
    #[cfg(test)]
    pub(crate) fn quant_carry_snapshot(&self) -> Vec<f64> {
        self.quant_carry.clone()
    }

    /// Reset the discrete per-time-unit inflow-limit budget. The caller invokes
    /// this when the simulation clock crosses an integer time unit (§6.3);
    /// unaffected by arrest (§4.3 step 0).
    pub fn on_time_boundary(&mut self) {
        self.in_carry = 0.0;
    }

    // ----- geometry / zones (§5.3) -----

    /// Is slat `i` (0 = exit) within leak flow `k`'s zone, given belt length
    /// `length`? Slat `i` centers at `(i + 0.5) / length` from the exit, i.e.
    /// `1 - that` from the entry; in-zone when `zone_start <= pos_from_entry <=
    /// zone_end`.
    fn in_zone(&self, i: usize, length: usize, k: usize) -> bool {
        let lk = &self.leaks[k];
        let pos_from_entry = 1.0 - (i as f64 + 0.5) / length as f64;
        // Exact `<=` (no epsilon) to match the reference prototype's zone
        // membership bit-for-bit; the boundary-straddling case (a slat exactly
        // at a zone edge belongs to both adjacent zones) is intentional (§5.3).
        lk.zone_start <= pos_from_entry && pos_from_entry <= lk.zone_end
    }

    /// Count of flow `k`'s in-zone slats among indices `0..depth`.
    fn zone_count_from(&self, k: usize, length: usize, depth: usize) -> usize {
        (0..depth).filter(|&i| self.in_zone(i, length, k)).count()
    }

    /// §5.1 `r_k`: projected fraction of a unit cohort still remaining when it
    /// reaches each leak flow's zone start, by unit forward simulation over the
    /// entry path with the current fractions. `r_k = 1` for exponential leakage,
    /// under the ignore-earlier-zone-losses toggle, or for an entry-start zone.
    fn zone_start_retained(
        &self,
        belt_len: usize,
        entry_depth: usize,
        fractions: &[f64],
    ) -> Vec<f64> {
        let n = self.n_leaks();
        if self.exponential_leak || self.ignore_earlier_zone_losses {
            return vec![1.0; n];
        }
        let m_entry: Vec<usize> = (0..n)
            .map(|k| self.zone_count_from(k, belt_len, entry_depth))
            .collect();
        // Deepest (first-traversed) in-zone slat per flow.
        let mut first_zone_slat: Vec<Option<usize>> = vec![None; n];
        for (k, first) in first_zone_slat.iter_mut().enumerate() {
            for i in (0..entry_depth).rev() {
                if self.in_zone(i, belt_len, k) {
                    *first = Some(i);
                    break;
                }
            }
        }
        let mut r = vec![1.0; n];
        let mut c = 1.0;
        for i in (0..entry_depth).rev() {
            for (k, first) in first_zone_slat.iter().enumerate() {
                if *first == Some(i) {
                    r[k] = c;
                }
            }
            let mut shed = 0.0;
            for k in 0..n {
                if self.in_zone(i, belt_len, k) && m_entry[k] > 0 {
                    shed += fractions[k] * r[k] / m_entry[k] as f64;
                }
            }
            c = (c - shed).max(0.0);
        }
        r
    }

    // ----- initialization (§7.1) -----

    /// Scalar steady-state fill (§7.1): distribute `v` across the belt at the
    /// equilibrium implied by the leak profile. `initial_fractions` are the leak
    /// fractions at `t = start` (the schedule and retained profile are frozen at
    /// these values).
    pub fn init_steady(&mut self, transit: f64, v: f64, initial_fractions: &[f64]) {
        self.latched_transit = transit;
        let n_slats = self.n_slats();
        let n = self.n_leaks();

        // Retained profile c[i]: content a steady unit cohort holds on arriving
        // at slat i. c[N-1] = 1 at the entry; walk toward the exit shedding leak.
        let mut c = vec![0.0; n_slats];
        c[n_slats - 1] = 1.0;
        let r0 = self.zone_start_retained(n_slats, n_slats, initial_fractions);
        let unit_basis: Vec<f64> = (0..n)
            .map(|k| {
                let m = self.zone_count_from(k, n_slats, n_slats);
                if self.exponential_leak || m == 0 {
                    0.0
                } else {
                    r0[k] / m as f64
                }
            })
            .collect();
        for i in (1..n_slats).rev() {
            let mut shed = 0.0;
            for k in 0..n {
                if self.in_zone(i, n_slats, k) {
                    shed += if self.exponential_leak {
                        c[i] * initial_fractions[k] * self.dt
                    } else {
                        initial_fractions[k] * unit_basis[k]
                    };
                }
            }
            c[i - 1] = (c[i] - shed).max(0.0);
        }
        let s: f64 = c.iter().sum();
        let e = if s > 0.0 { v / s } else { 0.0 };

        self.slats = VecDeque::with_capacity(n_slats);
        for (i, &ci) in c.iter().enumerate() {
            let basis: Vec<f64> = unit_basis.iter().map(|ub| e * ub).collect();
            let window: Vec<f64> = (0..n)
                .map(|k| basis[k] * self.zone_count_from(k, n_slats, i + 1) as f64)
                .collect();
            self.slats.push_back(Slat {
                content: e * ci,
                leak_basis: basis,
                leak_window: window,
            });
        }

        // §6.4 rule 3 / §7.1: a discrete conveyor lumps each time-unit block's
        // material at the block's deepest (last-entered) slat instead of
        // spreading it. Merging the block's cohorts is exact -- content and the
        // linear-leak schedule are both additive (§6.2) -- so the belt stays at
        // the same equilibrium, only lumped at time-unit starts.
        if self.discrete {
            self.merge_time_unit_blocks();
        }
    }

    /// Merge each time-unit block's slats into the block's deepest slat (§6.4
    /// rule 3): slat `i` belongs to block `floor(i * dt)`; the deepest slat is
    /// the largest index in the block. Content/basis/window sum field-wise.
    fn merge_time_unit_blocks(&mut self) {
        let n_slats = self.slats.len();
        if n_slats == 0 {
            return;
        }
        let n = self.n_leaks();
        // Deepest slat index per block (blocks are contiguous, increasing in i).
        let block_of = |i: usize| (i as f64 * self.dt).floor() as i64;
        let mut merged: Vec<Slat> = (0..n_slats).map(|_| Slat::empty(n)).collect();
        let mut i = 0;
        while i < n_slats {
            let b = block_of(i);
            let mut deepest = i;
            let mut j = i;
            while j < n_slats && block_of(j) == b {
                deepest = j;
                j += 1;
            }
            for slat in self.slats.iter().take(j).skip(i) {
                merged[deepest].content += slat.content;
                for k in 0..n {
                    merged[deepest].leak_basis[k] += slat.leak_basis[k];
                    merged[deepest].leak_window[k] += slat.leak_window[k];
                }
            }
            i = j;
        }
        self.slats = merged.into_iter().collect();
    }

    /// Explicit per-slat / per-time-unit list initialization (§7.2). `values`
    /// is the comma-separated `<eqn>` list (front first). `initial_fractions`
    /// are the leak fractions at `t = start` for the filled cohorts' schedules.
    ///
    /// - `values.len() == N` (one entry per slat): entry `j` fills slat `j`
    ///   directly. This is the only interpretation for non-integer transits.
    /// - any other length (one entry per time unit): the list is normalized to
    ///   `U = floor((N-1)*dt)+1` entries (truncate extra, repeat the last for a
    ///   short list); entry `u` fills block `u`, spread evenly across the
    ///   block's slats for a continuous conveyor or placed whole in the block's
    ///   deepest slat for a discrete one.
    pub fn init_explicit(&mut self, transit: f64, values: &[f64], initial_fractions: &[f64]) {
        self.latched_transit = transit;
        let n_slats = self.n_slats();
        let per_slat = if values.len() == n_slats {
            values.to_vec()
        } else {
            self.spread_per_time_unit(n_slats, values)
        };
        self.fill_slats(n_slats, &per_slat, initial_fractions);
    }

    /// Distribute a per-time-unit list across the belt's slats (§7.2 non-`N`
    /// case). Returns the per-slat content vector (front first).
    fn spread_per_time_unit(&self, n_slats: usize, values: &[f64]) -> Vec<f64> {
        let u = ((n_slats as f64 - 1.0) * self.dt).floor() as usize + 1;
        // Normalize the list to U entries: truncate extra, repeat the last for
        // a short list (an empty list normalizes to all-zero).
        let norm = |idx: usize| -> f64 {
            if values.is_empty() {
                0.0
            } else if idx < values.len() {
                values[idx]
            } else {
                *values.last().unwrap()
            }
        };
        // Slats per block, and the block's deepest slat.
        let block_of = |i: usize| ((i as f64 * self.dt).floor() as usize).min(u - 1);
        let mut counts = vec![0usize; u];
        let mut deepest = vec![0usize; u];
        for i in 0..n_slats {
            let b = block_of(i);
            counts[b] += 1;
            deepest[b] = i;
        }
        let mut per_slat = vec![0.0; n_slats];
        for (i, slot) in per_slat.iter_mut().enumerate() {
            let b = block_of(i);
            let v = norm(b);
            if self.discrete {
                if i == deepest[b] {
                    *slot = v;
                }
            } else if counts[b] > 0 {
                *slot = v / counts[b] as f64;
            }
        }
        per_slat
    }

    /// Fill the belt from an explicit per-slat content vector, giving each slat
    /// the linear-leak schedule of a cohort that entered at the belt entry and
    /// traveled to its position (§7.2, as in §7.1 step 3), scaled to the slat's
    /// content.
    fn fill_slats(&mut self, n_slats: usize, per_slat: &[f64], initial_fractions: &[f64]) {
        let n = self.n_leaks();
        let r0 = self.zone_start_retained(n_slats, n_slats, initial_fractions);
        // Per-unit-content entry-cohort basis (independent of position). A slat
        // holding content `x` that arrived at position `i` carries the schedule
        // of an entry cohort whose content at position `i` is `x`; that cohort's
        // entry volume is `x / c[i]`, but for the schedule we only need the
        // per-content basis, which for a full/entry-start zone is `r0/M`.
        let unit_basis: Vec<f64> = (0..n)
            .map(|k| {
                let m = self.zone_count_from(k, n_slats, n_slats);
                if self.exponential_leak || m == 0 {
                    0.0
                } else {
                    r0[k] / m as f64
                }
            })
            .collect();
        self.slats = VecDeque::with_capacity(n_slats);
        for (i, &content) in per_slat.iter().enumerate() {
            let basis: Vec<f64> = unit_basis.iter().map(|ub| content * ub).collect();
            let window: Vec<f64> = (0..n)
                .map(|k| basis[k] * self.zone_count_from(k, n_slats, i + 1) as f64)
                .collect();
            self.slats.push_back(Slat {
                content,
                leak_basis: basis,
                leak_window: window,
            });
        }
        if self.discrete {
            self.merge_time_unit_blocks();
        }
    }

    /// Initialize so that a steady entry cohort equals `inflow_rate * dt` -- the
    /// prototype's `init_from_inflow`, used by the leak scenarios (S3/S4/S9/etc.)
    /// to start the belt already at its `inflow`-driven equilibrium.
    #[cfg(test)]
    pub fn init_from_inflow(&mut self, transit: f64, inflow_rate: f64, initial_fractions: &[f64]) {
        self.init_steady(transit, 1.0, initial_fractions);
        let entry = self.slats.back().map(|s| s.content).unwrap_or(0.0);
        let e = inflow_rate * self.dt;
        let scale = if entry != 0.0 { e / entry } else { 0.0 };
        for s in self.slats.iter_mut() {
            s.content *= scale;
            for b in s.leak_basis.iter_mut() {
                *b *= scale;
            }
            for w in s.leak_window.iter_mut() {
                *w *= scale;
            }
        }
    }

    // ----- phase A: arrest, latch, leak, exit (§4.3 steps 0-3) -----

    pub fn phase_a(&mut self, inp: PhaseAInputs) -> PhaseAResult {
        self.step_contents0 = self.contents();
        let n = self.n_leaks();

        // Step 0: arrest -- evaluated before the latch (§4.3). No leak ran, so
        // the per-slat breakdown is empty per leak flow (indexable by k).
        if inp.arrested {
            return PhaseAResult {
                out_vol: 0.0,
                leak_vols: vec![0.0; n],
                leak_slat_vols: vec![Vec::new(); n],
                arrested: true,
                held: false,
            };
        }

        // Step 1: latch. A non-finite transit leaves latched_transit unchanged
        // (§4.4); a finite value is assumed already clamped to max(dt, .) by the
        // caller.
        if inp.sample && inp.transit.is_finite() {
            self.latched_transit = inp.transit;
        }

        // Step 2: leak.
        let (leak_vols, leak_slat_vols) =
            self.leak_step(inp.leak_fractions, inp.leak_dest_arrested);

        // Step 3: exit (held if the primary destination is arrested).
        if inp.dest_arrested {
            return PhaseAResult {
                out_vol: 0.0,
                leak_vols,
                leak_slat_vols,
                arrested: false,
                held: true,
            };
        }
        let out_vol = self.slats.front().map(|s| s.content).unwrap_or(0.0);
        PhaseAResult {
            out_vol,
            leak_vols,
            leak_slat_vols,
            arrested: false,
            held: false,
        }
    }

    /// §4.3 step 2 leak, split by leak model. Returns per-flow leaked volume,
    /// the per-`(leak flow, slat)` breakdown (index 0 = exit; a downstream
    /// `source` placement mirrors it, §8), and mutates slat contents (and, for
    /// linear leakage, the travel windows). `slat_vols[k]` sums to
    /// `leak_vols[k]` (conservation) after any integer-leak requantization.
    fn leak_step(
        &mut self,
        fractions: &[f64],
        leak_dest_arrested: &[bool],
    ) -> (Vec<f64>, Vec<Vec<f64>>) {
        let n = self.n_leaks();
        debug_assert_eq!(
            fractions.len(),
            n,
            "leak_fractions length must equal the leak-flow count"
        );
        let l = self.slats.len();
        let mut leak_vols = vec![0.0; n];
        // Per-(leak flow, slat) shed detail, so `slat_vols[k][i]` is what flow k
        // took from slat i this DT. Kept exactly in sync with the content
        // subtractions below (including the integer-leak requantization).
        let mut slat_vols = vec![vec![0.0; l]; n];
        // A leak flow whose destination is arrested is skipped entirely this
        // step (rate 0, content stays): fold it into an "effectively out of
        // zone" test so neither model leaks it nor consumes its window.
        let arrested = |k: usize| leak_dest_arrested.get(k).copied().unwrap_or(false);

        if self.exponential_leak {
            // §5.2: rates add -- every flow computed from the same start-of-step
            // content; scaled down proportionally if the sum would overdrain.
            // `i` indexes both `self.slats` (a VecDeque) and the per-slat column
            // `slat_vols[k][i]`, so an index loop is the clean form.
            // One shed-per-flow scratch reused across slats (refilled with 0.0
            // each iteration, so values match a per-slat fresh Vec exactly): a
            // fresh allocation per slat made the allocation count scale with
            // the belt length every step (GH #879).
            let mut sheds = vec![0.0; n];
            // `i` is a belt POSITION, not merely a slat index: it is passed to
            // `self.in_zone(i, ..)` and indexes the parallel `slat_vols[k][i]`
            // column. Iterating `self.slats` would also hold a borrow across the
            // `&self` call.
            #[allow(clippy::needless_range_loop)]
            for i in 0..l {
                let c0 = self.slats[i].content;
                sheds.fill(0.0);
                for k in 0..n {
                    if self.in_zone(i, l, k) && !arrested(k) {
                        sheds[k] = c0 * fractions[k] * self.dt;
                    }
                }
                let tot: f64 = sheds.iter().sum();
                if tot > c0 && c0 > 0.0 {
                    for s in sheds.iter_mut() {
                        *s *= c0 / tot;
                    }
                } else if c0 <= 0.0 {
                    sheds.fill(0.0);
                }
                let sum_sheds: f64 = sheds.iter().sum();
                self.slats[i].content -= sum_sheds;
                for k in 0..n {
                    leak_vols[k] += sheds[k];
                    slat_vols[k][i] += sheds[k];
                }
            }
            return (leak_vols, slat_vols);
        }

        // §5.1 linear leakage. First compute the continuous per-(slat, flow)
        // shed with priority (earlier flows reduce content first) and window
        // consumption; subtract it immediately. Integer flows (§5.4) are then
        // re-quantized below.
        let has_integers = self.leaks.iter().any(|lk| lk.integers);
        // shed_by[i * n + k]: continuous shed of flow k from slat i (only kept
        // when any flow is integer -- avoids an allocation in the common case).
        let mut shed_by = if has_integers {
            vec![0.0; l * n]
        } else {
            Vec::new()
        };
        for i in 0..l {
            for k in 0..n {
                if !self.in_zone(i, l, k) || arrested(k) {
                    continue;
                }
                let use_ = self.slats[i].leak_basis[k].min(self.slats[i].leak_window[k]);
                let shed = (fractions[k] * use_).min(self.slats[i].content);
                self.slats[i].content -= shed;
                self.slats[i].leak_window[k] -= use_;
                leak_vols[k] += shed;
                slat_vols[k][i] += shed;
                if has_integers {
                    shed_by[i * n + k] = shed;
                }
            }
        }

        if has_integers {
            // `n` is the leak-flow count, which is also `shed_by`'s row stride.
            self.quantize_integer_leaks(n, &shed_by, &mut leak_vols, &mut slat_vols);
        }
        (leak_vols, slat_vols)
    }

    /// §5.4 integer leakage. The continuous shed for each integer flow was
    /// already subtracted (to get priority/window right); undo it, accumulate
    /// the continuous amount into the flow's carry, then remove `floor(carry)`
    /// whole units, exit-most slat first. This redistributes timing without
    /// changing the cohort's schedule (the window was consumed by travel above,
    /// independent of the quantization). The integer+multi-flow priority
    /// interaction is a simlin-defined corner (the spec pins only the
    /// single-flow case).
    fn quantize_integer_leaks(
        &mut self,
        n: usize,
        shed_by: &[f64],
        leak_vols: &mut [f64],
        slat_vols: &mut [Vec<f64>],
    ) {
        let l = self.slats.len();
        for k in 0..n {
            if !self.leaks[k].integers {
                continue;
            }
            // Undo the continuous removal for this flow, both from content and
            // from the per-slat breakdown (the whole-unit removals below become
            // the authoritative per-slat detail for this flow). `i` indexes three
            // parallel structures -- `self.slats`, the flattened `shed_by[i * n + k]`,
            // and the per-slat column `slat_vols[k][i]` -- so no single iterator
            // yields it.
            #[allow(clippy::needless_range_loop)]
            for i in 0..l {
                self.slats[i].content += shed_by[i * n + k];
                slat_vols[k][i] = 0.0;
            }
            self.leak_carry[k] += leak_vols[k];
            let whole = self.leak_carry[k].floor();
            self.leak_carry[k] -= whole;
            // Remove `whole` units, exit-most in-zone slat first, clamped to
            // each slat's content. Undelivered units return to the carry.
            // `i` is a belt POSITION passed to `self.in_zone(i, ..)`, and the loop
            // mutates `self.slats[i]` across that `&self` call.
            let mut remaining = whole;
            #[allow(clippy::needless_range_loop)]
            for i in 0..l {
                if remaining <= 0.0 {
                    break;
                }
                if !self.in_zone(i, l, k) {
                    continue;
                }
                let take = remaining.min(self.slats[i].content);
                self.slats[i].content -= take;
                slat_vols[k][i] += take;
                remaining -= take;
            }
            let delivered = whole - remaining;
            self.leak_carry[k] += remaining;
            leak_vols[k] = delivered;
        }
    }

    // ----- phase B: admit, shift, insert (§4.3 steps 4-6) -----

    /// The `(cap_room, limit_vol)` admission headroom for this DT, shared by
    /// [`phase_b`](ConveyorState::phase_b) and
    /// [`admission_budget`](ConveyorState::admission_budget). The queue coupling
    /// is only correct if the budget a coupled queue is sized against
    /// (`admission_budget`) uses the exact formulas phase_b then admits with --
    /// phase_b admits the queue-supplied conveyor volume unconditionally, so a
    /// drifted budget would put over-capacity/over-limit material on the belt
    /// with no error. Sharing one derivation makes that invariant structural.
    ///
    /// `conv_vol` is the conveyor-driven volume charged against capacity room:
    /// phase_b passes ALL unconditionally-admitted inflow volume; the budget
    /// caller excludes the queue-supplied volume it is sizing. For the inflow
    /// limit, a discrete conveyor draws down a per-time-unit budget (`in_carry`
    /// accumulates within the time unit); a continuous one gets `in_limit * dt`
    /// each DT. A coupled conveyor is always discrete (the compiler enforces
    /// `ConveyorQueueUpstreamNotDiscrete`), so for `admission_budget` the
    /// continuous branch is defense in depth.
    fn admission_room(
        &self,
        contents_after: f64,
        capacity: f64,
        in_limit: f64,
        conv_vol: f64,
    ) -> (f64, f64) {
        let cap_room = if capacity.is_infinite() {
            f64::INFINITY
        } else {
            (capacity - contents_after - conv_vol).max(0.0)
        };
        let limit_vol = if in_limit.is_infinite() {
            f64::INFINITY
        } else if self.discrete {
            (in_limit - self.in_carry).max(0.0)
        } else {
            in_limit * self.dt
        };
        (cap_room, limit_vol)
    }

    pub fn phase_b(&mut self, inp: PhaseBInputs) -> PhaseBResult {
        let dt = self.dt;
        let n_inflows = inp.eq_request_rates.len();
        if inp.phase_a.arrested {
            return PhaseBResult {
                admitted: 0.0,
                in_vols: vec![0.0; n_inflows],
                cleared: vec![0.0; n_inflows],
            };
        }

        // Step 4: admit. Capacity credits the room freed by this DT's outflow
        // AND leak; conveyor-driven inflow is admitted unconditionally.
        let conv_vol: f64 = inp.conv_inflows.iter().map(|(v, _)| v).sum();
        let leaked: f64 = inp.phase_a.leak_vols.iter().sum();
        let contents_after = self.step_contents0 - leaked - inp.phase_a.out_vol;
        let (cap_room, limit_vol) =
            self.admission_room(contents_after, inp.capacity, inp.in_limit, conv_vol);

        // Apportion the clearance across inflows in listed order.
        let mut rem_cap = cap_room;
        let mut rem_limit = limit_vol;
        let mut cleared = vec![0.0; n_inflows];
        for (j, &rate) in inp.eq_request_rates.iter().enumerate() {
            let c = (rate.max(0.0) * dt).min(rem_cap).min(rem_limit);
            cleared[j] = c;
            if rem_cap.is_finite() {
                rem_cap -= c;
            }
            if rem_limit.is_finite() {
                rem_limit -= c;
            }
        }

        let in_vols = if self.discrete {
            self.discrete_admit(&cleared, cap_room)
        } else {
            cleared.clone()
        };
        let eq_admitted: f64 = in_vols.iter().sum();
        let admitted = conv_vol + eq_admitted;

        // Step 5: shift.
        self.shift(inp.phase_a.held);

        // Step 6: insert -- always runs, even for admitted == 0. Each admitted
        // component (conveyor-driven volume + each equation inflow) is spread
        // across slats per its placement (§8) into per-slat shares. Because a
        // cohort's leak schedule is linear in its volume, shares that land on
        // the same slat (same insertion depth d_c = i+1) merge exactly into one
        // schedule computation -- so we accumulate a per-slat total then insert.
        let d = self.n_slats();
        while self.slats.len() < d {
            self.slats.push_back(Slat::empty(self.n_leaks()));
        }
        let belt_len = self.slats.len();
        let mut shares = vec![0.0; belt_len];
        for (vol, placement) in inp.conv_inflows {
            self.add_placement_shares(&mut shares, *vol, placement, d, belt_len);
        }
        for (j, &vol) in in_vols.iter().enumerate() {
            let placement = inp.placements.get(j).unwrap_or(&Placement::Beginning);
            self.add_placement_shares(&mut shares, vol, placement, d, belt_len);
        }
        // §5.1 cohort schedule per inserted share: `basis_k = A * r_k / M_k(d)`,
        // `window_k = basis_k * min(M_k(d_c), M_k(d))`, where the share's own
        // insertion depth is d_c = i + 1 (§8): a mid-belt share's leak budget
        // is prorated to the zone slats it will traverse. The share-INDEPENDENT
        // pieces -- the retained profile `r_k` and the entry-path zone count
        // `M_k(d)` -- are hoisted out of the per-slat loop: an Even/Dist/Dest
        // placement has O(d) non-zero shares, and recomputing them per share
        // made the insert O(d^2 * n) work with O(d) allocations per step
        // (GH #879). Nothing this loop mutates (slat contents and schedules)
        // feeds `r_k` or `M_k`, so computing them once with the identical
        // expressions yields bit-identical values.
        //
        // A quiescent step (every share zero: nothing was admitted this DT)
        // skips the whole block: with all-zero shares every loop iteration
        // takes the zero-share `continue`, and the running `m_own` prefix is
        // read only by the insert arms, so the entire body -- hoisted work
        // included -- is dead. Gating it keeps an idle belt at the old
        // do-nothing per-step cost instead of paying the hoisted
        // O(belt_len * n) scan and its two allocations every step.
        if shares.iter().any(|&s| s != 0.0) {
            let n = self.n_leaks();
            let (r, m_entry) = if self.exponential_leak {
                // Exponential leakage carries no per-cohort state (§5.1): every
                // schedule contribution below is +0.0, so the profile is unused.
                (Vec::new(), Vec::new())
            } else {
                let r = self.zone_start_retained(belt_len, d, inp.leak_fractions);
                let m_entry: Vec<usize> = (0..n)
                    .map(|k| self.zone_count_from(k, belt_len, d))
                    .collect();
                (r, m_entry)
            };
            // Running in-zone prefix count: after the update at slat `i`,
            // `m_own[k] == zone_count_from(k, belt_len, i + 1) == M_k(d_c)`.
            // Integer counting, so it is exactly the value a per-share scan
            // computed. Updated for every slat (before the zero-share skip) so
            // the prefix stays aligned with `i`.
            let mut m_own = vec![0usize; n];
            for (i, &share) in shares.iter().enumerate() {
                for (k, m) in m_own.iter_mut().enumerate() {
                    if self.in_zone(i, belt_len, k) {
                        *m += 1;
                    }
                }
                if share == 0.0 {
                    continue;
                }
                let tgt = &mut self.slats[i];
                tgt.content += share;
                if self.exponential_leak {
                    // Keep the `+= 0.0` the all-zero schedule performed: adding
                    // +0.0 normalizes a stored -0.0 basis/window (possible from
                    // a negative initial value scaled by a zero unit basis) to
                    // +0.0, so skipping the adds would change the stored bit
                    // pattern.
                    for k in 0..n {
                        tgt.leak_basis[k] += 0.0;
                        tgt.leak_window[k] += 0.0;
                    }
                } else {
                    for k in 0..n {
                        let b = if m_entry[k] > 0 {
                            share * r[k] / m_entry[k] as f64
                        } else {
                            0.0
                        };
                        tgt.leak_basis[k] += b;
                        tgt.leak_window[k] += b * m_own[k].min(m_entry[k]) as f64;
                    }
                }
            }
        }

        PhaseBResult {
            admitted,
            in_vols,
            cleared,
        }
    }

    // ----- queue-conveyor coupling (§6.3/§11, queues.md §9) -----

    /// The admission budget `req = min(cap_room, limit_vol)` a queue directly
    /// upstream may supply this DT (conveyors.md §6.3/§11). Computed AFTER this
    /// conveyor's [`phase_a`](ConveyorState::phase_a) (which snapshots
    /// `step_contents0` and frees belt room via leaks/exit) and BEFORE its
    /// [`phase_b`](ConveyorState::phase_b), so the coupled queue can size how much
    /// it serves. Shares [`admission_room`](ConveyorState::admission_room) with
    /// phase_b, so the coupled admission obeys capacity and the (discrete)
    /// per-time-unit inflow limit identically to what phase_b then admits.
    ///
    /// `other_conv_vol` is the sum of the OTHER unconditionally-admitted
    /// conveyor-driven inflow volumes this DT (a conveyor chain feeding the same
    /// belt), EXCLUDING the queue-supplied volume itself -- it is what we are
    /// sizing, so it must not pre-charge its own capacity room. An arrested
    /// conveyor requests nothing (the belt is frozen; the queue holds). Does NOT
    /// mutate belt state.
    pub fn admission_budget(
        &self,
        phase_a: &PhaseAResult,
        capacity: f64,
        in_limit: f64,
        other_conv_vol: f64,
    ) -> f64 {
        if phase_a.arrested {
            return 0.0;
        }
        let leaked: f64 = phase_a.leak_vols.iter().sum();
        let contents_after = self.step_contents0 - leaked - phase_a.out_vol;
        let (cap_room, limit_vol) =
            self.admission_room(contents_after, capacity, in_limit, other_conv_vol);
        cap_room.min(limit_vol)
    }

    /// Debit the discrete per-time-unit inflow-limit budget by a queue-coupled
    /// admission of `vol` (§6.3/§11). The coupled volume enters the belt through
    /// the unconditional conveyor-driven inflow path (phase_b's `conv_inflows`),
    /// which never touches `in_carry`, so the coupling records the consumption
    /// here -- otherwise every DT within a time unit would see the full `in_limit`
    /// budget and admit more than the limit permits. A continuous conveyor has no
    /// per-time-unit carry (`limit_vol = in_limit * dt` each DT), so this is a
    /// no-op there; a coupled conveyor is always discrete.
    pub fn consume_inflow_budget(&mut self, vol: f64) {
        if self.discrete {
            self.in_carry += vol;
        }
    }

    /// Distribute an admitted `volume` across belt slats per `placement` (§8),
    /// adding the per-slat shares into `shares` (length `belt_len`, index 0 =
    /// exit). `d` is the entry depth. Each placement conserves volume exactly
    /// (`Σ shares_added == volume`); a degenerate placement falls back to
    /// `Beginning` (all at the entry slat `d-1`).
    fn add_placement_shares(
        &self,
        shares: &mut [f64],
        volume: f64,
        placement: &Placement,
        d: usize,
        belt_len: usize,
    ) {
        if d == 0 || belt_len == 0 {
            return;
        }
        let beginning = |shares: &mut [f64]| shares[d - 1] += volume;
        match placement {
            Placement::Beginning => beginning(shares),
            Placement::Even => {
                let per = volume / d as f64;
                for share in shares.iter_mut().take(d) {
                    *share += per;
                }
            }
            Placement::Dest => {
                let total: f64 = self.slats.iter().take(belt_len).map(|s| s.content).sum();
                if total > 0.0 {
                    for (i, share) in shares.iter_mut().enumerate().take(belt_len) {
                        *share += volume * self.slats[i].content / total;
                    }
                } else {
                    beginning(shares);
                }
            }
            Placement::Dist(weights) => {
                // weights[i] is w_i for entry-path slat i (0..d-1), exit-first.
                let usable = weights.len() >= d;
                let sum_w: f64 = if usable {
                    weights.iter().take(d).map(|w| w.max(0.0)).sum()
                } else {
                    0.0
                };
                if usable && sum_w > 0.0 {
                    for (i, share) in shares.iter_mut().enumerate().take(d) {
                        *share += volume * weights[i].max(0.0) / sum_w;
                    }
                } else {
                    beginning(shares);
                }
            }
        }
    }

    /// §6.4 rule 1 discrete admission: each inflow accrues its cleared volume to
    /// its own carry, then whole units insert in listed order under a shared
    /// `floor(cap_room)` budget, so every inserted unit debits exactly the
    /// inflow that cleared it. The `in_limit` window was accounted at clearance
    /// time and is not re-checked here.
    fn discrete_admit(&mut self, cleared: &[f64], cap_room: f64) -> Vec<f64> {
        if self.quant_carry.len() != cleared.len() {
            self.quant_carry = vec![0.0; cleared.len()];
        }
        let sum_cleared: f64 = cleared.iter().sum();
        self.in_carry += sum_cleared;
        let mut budget = if cap_room.is_infinite() {
            f64::INFINITY
        } else {
            cap_room.floor()
        };
        let mut in_vols = vec![0.0; cleared.len()];
        for (j, &c) in cleared.iter().enumerate() {
            self.quant_carry[j] += c;
            let mut units = self.quant_carry[j].floor();
            if budget.is_finite() {
                units = units.min(budget);
                budget -= units;
            }
            if units < 0.0 {
                units = 0.0;
            }
            self.quant_carry[j] -= units;
            in_vols[j] = units;
        }
        in_vols
    }

    /// §4.3 step 5 shift. A held exit keeps slat 0 and merges the next slat into
    /// it (summing content/basis/window); otherwise slat 0 pops (it left as
    /// outflow). Trailing empty slats beyond the entry depth are dropped.
    fn shift(&mut self, held: bool) {
        if held {
            if self.slats.len() > 1 {
                let s1 = self.slats.remove(1).unwrap();
                let s0 = &mut self.slats[0];
                s0.content += s1.content;
                for k in 0..self.leaks.len() {
                    s0.leak_basis[k] += s1.leak_basis[k];
                    s0.leak_window[k] += s1.leak_window[k];
                }
            }
        } else {
            self.slats.pop_front();
        }
        let n = self.n_slats();
        while self.slats.len() > n {
            match self.slats.back() {
                Some(s) if s.content == 0.0 => {
                    self.slats.pop_back();
                }
                _ => break,
            }
        }
    }
}
