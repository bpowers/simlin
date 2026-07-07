// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Queue runtime engine: the per-DT FIFO batch model.
//!
//! This is the pure functional core of XMILE queue support, a faithful Rust
//! transcription of `docs/design/queues.md` §2, §4, §7, §8 for the
//! UNCONSTRAINED (cloud / regular-stock) outflow case. Like [`crate::conveyor`]
//! it carries no dependency on the VM, datamodel, or compiler: a [`QueueState`]
//! owns one FIFO of batch volumes and is driven each DT by scalar inputs the
//! caller (the VM's queue pass, §4.2) evaluates from ordinary equations. Every
//! dynamic quantity -- the inflow rate, DT, and the volume a downstream requests
//! -- is passed in per step, so this module holds only batch state and does no
//! expression evaluation.
//!
//! A **batch** is a single scalar volume (§2): the material admitted during one
//! DT. A queue tracks batches only to expose them for container access (§8) and,
//! in later phases, the conveyor coupling (§9); a batch carries no age. The
//! queue's reported value is `Σ batch volumes`, which is the invariant
//! [`QueueState::total`] returns.
//!
//! The per-DT update is admit-then-serve (§4.2): [`QueueState::admit`] appends
//! one batch at the back, then the caller serves outflows off the front.
//! [`QueueState::take_from_front`] is the serving primitive -- it removes a
//! requested volume from the front, splitting the boundary batch on a partial
//! take -- and every outflow shape composes from it:
//!
//! - **Unconstrained serve** (§4.3, this phase): [`QueueState::serve_unconstrained`]
//!   drains the entire queue (an unbounded take from the front).
//! - **Conveyor-coupled serve** (§9, a later phase): the downstream conveyor's
//!   admission budget `req` becomes the `take_from_front(req)` request, under the
//!   `one_at_a_time` / `batch_integrity` batch rules the caller layers on top.
//! - **Overflow** (§4.5, a later phase): the redirectable front volume a blocked
//!   sibling left behind is another `take_from_front` on the same front.
//!
//! Queues are Euler-oriented and DT-driven (§10.3); this core does no
//! integration and reads no clock.

use std::collections::VecDeque;

/// Per-queue runtime state (§4.1). One instance per scalar queue (per array
/// element for arrayed queues, a later phase).
///
/// The state is a FIFO of batch volumes: `batches[0]` is the front (oldest,
/// served first) and the back is the newest (most recently admitted). The
/// module invariant is `Σ batches == the queue's reported value`, which
/// [`QueueState::total`] computes directly, so no separate running total is
/// stored (there is nothing to keep in sync).
///
/// `Debug` is gated on the `debug-derive` feature, exactly like the sibling
/// runtime side-table type [`crate::conveyor::ConveyorState`]: both live in the
/// VM's per-instance side tables, and the VM does not derive `Debug`
/// unconditionally (if it did, `ConveyorState`'s gated `Debug` would already
/// break the no-default-features WASM build), so gating keeps `Debug` out of
/// that build for binary size without any transitivity hazard.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone)]
pub struct QueueState {
    /// The FIFO of batch volumes: front = index 0 = oldest, back = newest.
    /// Invariant: every entry is `> 0` (admit never appends a non-positive
    /// batch, and a partial take leaves a strictly-positive remainder), so the
    /// batch count and container access never see spurious empty batches.
    batches: VecDeque<f64>,
}

impl QueueState {
    // ----- initialization (§7) -----

    /// An empty queue (no batches). The reset / start-of-run default before any
    /// initial value is applied.
    pub fn init_empty() -> QueueState {
        QueueState {
            batches: VecDeque::new(),
        }
    }

    /// Initialize from a scalar initial value `v` (§7):
    /// - `v <= 0` (or non-finite-negative): the queue starts empty.
    /// - `v > 0`: the queue starts with a single front batch of volume `v`.
    ///
    /// There is no steady-state fill (a queue has no transit time) and no
    /// explicit per-batch list; both are out of scope for XMILE (§7, §3.2).
    pub fn init_from_value(v: f64) -> QueueState {
        let mut q = QueueState::init_empty();
        if v > 0.0 {
            q.batches.push_back(v);
        }
        q
    }

    // ----- admit (§4.2 step 1) -----

    /// Admit this DT's inflow (§4.2 step 1): append one batch of volume
    /// `max(inflow_rate, 0) * dt` at the BACK. A negative inflow contributes no
    /// batch (§3.4), and a zero (or negative) clamped volume appends nothing at
    /// all -- so the queue never accumulates spurious empty batches that would
    /// pollute the batch count and container access. Multiple inflows are summed
    /// by the caller into one `inflow_rate` before calling this (§4.2).
    pub fn admit(&mut self, inflow_rate: f64, dt: f64) {
        let in_vol = inflow_rate.max(0.0) * dt;
        if in_vol > 0.0 {
            self.batches.push_back(in_vol);
        }
    }

    // ----- serve (§4.2 step 2 / §4.3) -----

    /// Remove `requested_vol` from the FRONT, splitting the boundary batch on a
    /// partial take, and return the volume actually removed. This is the serving
    /// primitive every queue outflow composes from (§4.2 step 2):
    ///
    /// - Whole front batches are popped while they fit within the remaining
    ///   request.
    /// - When the next front batch is larger than the remaining request, it is
    ///   the boundary batch: exactly the remaining volume is taken and the
    ///   batch's leftover stays as the new (still strictly-positive) front.
    /// - An over-request (more than the queue holds) takes everything and
    ///   returns the queue's prior total, leaving it empty.
    /// - A request on an empty queue removes nothing and returns 0.
    ///
    /// A negative request is clamped to zero (a no-op returning 0) as defense in
    /// depth; the real callers pass non-negative volumes (`total()` for the
    /// unconstrained drain, a conveyor's `req` for the coupled case).
    pub fn take_from_front(&mut self, requested_vol: f64) -> f64 {
        let mut remaining = requested_vol.max(0.0);
        let mut removed = 0.0;
        while remaining > 0.0 {
            let front = match self.batches.front().copied() {
                Some(f) => f,
                None => break, // queue empty: stop (an over-request drains fully)
            };
            if front <= remaining {
                // The whole front batch fits within the request: pop it.
                self.batches.pop_front();
                removed += front;
                remaining -= front;
            } else {
                // Boundary batch: take only `remaining`, leaving the strictly-
                // positive remainder as the new front. `front > remaining > 0`
                // guarantees `front - remaining > 0`, preserving the no-empty-
                // batch invariant.
                let take = remaining;
                self.batches[0] = front - take;
                removed += take;
                remaining = 0.0;
            }
        }
        removed
    }

    /// Serve an unconstrained (cloud / regular-stock) outflow (§4.3): drain the
    /// ENTIRE queue and return the removed volume, so the caller sets the driven
    /// outflow rate `= removed / dt`. The queue is left empty.
    ///
    /// Implemented as `total()` then `clear()`, NOT `take_from_front(total())`
    /// nor `take_from_front(INFINITY)`. Passing `total()` as a request is
    /// floating-point-fragile (the running `remaining -= front` can drift below a
    /// later batch when a tiny batch sits behind a huge one, splitting off a
    /// spurious residual and under-reporting the drain); an `INFINITY` request
    /// looks robust but strands every batch behind a non-finite batch, because
    /// `INFINITY - INFINITY = NaN` fails `take_from_front`'s `remaining > 0`
    /// guard. Summing then clearing empties unconditionally and returns exactly
    /// `Σ batches` in `total`'s summation order (§4.3's `removed_vol = Σ batches`).
    pub fn serve_unconstrained(&mut self) -> f64 {
        let removed = self.total();
        self.batches.clear();
        removed
    }

    // ----- container access (§8, mirrors ConveyorState) -----

    /// The current batch-volume vector, front-to-back, for array builtins over a
    /// queue's batches (`SUM`/`MEAN`/`MIN`/`MAX`/`STDDEV`, §8).
    pub fn batch_contents(&self) -> Vec<f64> {
        self.batches.iter().copied().collect()
    }

    /// The number of batches -- the basis for `SIZE(queue)` (§8).
    pub fn batch_count(&self) -> usize {
        self.batches.len()
    }

    /// Volume of batch `k` counted from the FRONT (0 = oldest), or `None` if out
    /// of range -- the basis for 1-based `queue[k]` container access (§3.5/§8).
    /// The index is 0-based here, matching [`crate::conveyor::ConveyorState::slat_content`];
    /// the `queue[k]` accessor converts its 1-based `k` to this 0-based index at
    /// the call site, exactly as `conveyor[j]` does over `slat_content`.
    pub fn batch(&self, k: usize) -> Option<f64> {
        self.batches.get(k).copied()
    }

    /// Total material in the queue -- the queue variable's reported scalar value
    /// and the `Σ batches == total` invariant (§4.1). Summed front-to-back, the
    /// same order [`serve_unconstrained`] accumulates in, so the two agree
    /// exactly.
    ///
    /// [`serve_unconstrained`]: QueueState::serve_unconstrained
    pub fn total(&self) -> f64 {
        self.batches.iter().sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::float::approx_eq;

    // ----- init (§7) -----

    #[test]
    fn init_empty_has_no_batches() {
        let q = QueueState::init_empty();
        assert_eq!(q.batch_count(), 0);
        assert_eq!(q.total(), 0.0);
        assert_eq!(q.batch_contents(), Vec::<f64>::new());
    }

    #[test]
    fn init_from_positive_value_seeds_one_front_batch() {
        let q = QueueState::init_from_value(5.0);
        assert_eq!(q.batch_count(), 1);
        assert_eq!(q.batch_contents(), vec![5.0]);
        assert!(approx_eq(q.total(), 5.0));
    }

    #[test]
    fn init_from_zero_is_empty() {
        let q = QueueState::init_from_value(0.0);
        assert_eq!(q.batch_count(), 0);
        assert_eq!(q.total(), 0.0);
    }

    #[test]
    fn init_from_negative_is_empty() {
        let q = QueueState::init_from_value(-3.0);
        assert_eq!(q.batch_count(), 0);
        assert_eq!(q.total(), 0.0);
    }

    // ----- admit (§4.2 step 1 / §3.4) -----

    #[test]
    fn admit_appends_back_batch_of_rate_times_dt() {
        let mut q = QueueState::init_empty();
        q.admit(2.0, 0.5); // in_vol = 2.0 * 0.5 = 1.0
        assert_eq!(q.batch_contents(), vec![1.0]);
        assert!(approx_eq(q.total(), 1.0));
    }

    #[test]
    fn admit_negative_rate_appends_nothing() {
        let mut q = QueueState::init_from_value(4.0);
        q.admit(-1.0, 0.5);
        // Unchanged: the negative inflow contributed no batch (§3.4).
        assert_eq!(q.batch_contents(), vec![4.0]);
        assert!(approx_eq(q.total(), 4.0));
    }

    #[test]
    fn admit_zero_rate_appends_nothing() {
        let mut q = QueueState::init_empty();
        q.admit(0.0, 0.5);
        assert_eq!(q.batch_count(), 0);
    }

    #[test]
    fn admit_is_fifo_front_is_oldest() {
        let mut q = QueueState::init_empty();
        q.admit(1.0, 1.0); // batch A = 1.0 (oldest)
        q.admit(2.0, 1.0); // batch B = 2.0 (newest)
        // Front (index 0) is the oldest admitted batch.
        assert_eq!(q.batch_contents(), vec![1.0, 2.0]);
        assert_eq!(q.batch(0), Some(1.0));
        assert_eq!(q.batch(1), Some(2.0));
    }

    // ----- take_from_front (§4.2 step 2) -----

    #[test]
    fn take_exact_boundary_pops_whole_batch() {
        let mut q = QueueState::init_empty();
        q.admit(3.0, 1.0); // [3]
        q.admit(4.0, 1.0); // [3, 4]
        let removed = q.take_from_front(3.0);
        assert!(approx_eq(removed, 3.0));
        // The 3-batch popped exactly; the 4-batch is now the front.
        assert_eq!(q.batch_contents(), vec![4.0]);
    }

    #[test]
    fn take_partial_splits_boundary_and_leaves_remainder_as_front() {
        let mut q = QueueState::init_empty();
        q.admit(10.0, 1.0); // [10]
        q.admit(5.0, 1.0); // [10, 5]
        let removed = q.take_from_front(4.0);
        assert!(approx_eq(removed, 4.0));
        // 4 taken from the front batch; 6 remains as the new front, then 5.
        assert_eq!(q.batch_contents(), vec![6.0, 5.0]);
        assert!(approx_eq(q.total(), 11.0));
    }

    #[test]
    fn take_spanning_multiple_batches_splits_the_last() {
        let mut q = QueueState::init_empty();
        q.admit(2.0, 1.0); // [2]
        q.admit(3.0, 1.0); // [2, 3]
        q.admit(4.0, 1.0); // [2, 3, 4]
        // Request 6: pop 2 (whole), pop 3 (whole), take 1 of the 4-batch.
        let removed = q.take_from_front(6.0);
        assert!(approx_eq(removed, 6.0));
        assert_eq!(q.batch_contents(), vec![3.0]);
    }

    #[test]
    fn take_over_request_drains_all_and_returns_total() {
        let mut q = QueueState::init_empty();
        q.admit(2.0, 1.0); // [2]
        q.admit(3.0, 1.0); // [2, 3]
        let removed = q.take_from_front(100.0);
        assert!(approx_eq(removed, 5.0));
        assert_eq!(q.batch_count(), 0);
        assert_eq!(q.total(), 0.0);
    }

    #[test]
    fn take_from_empty_returns_zero() {
        let mut q = QueueState::init_empty();
        let removed = q.take_from_front(5.0);
        assert_eq!(removed, 0.0);
        assert_eq!(q.batch_count(), 0);
    }

    #[test]
    fn take_negative_request_is_a_noop() {
        let mut q = QueueState::init_from_value(4.0);
        let removed = q.take_from_front(-2.0);
        assert_eq!(removed, 0.0);
        assert_eq!(q.batch_contents(), vec![4.0]);
    }

    // ----- serve_unconstrained (§4.3) -----

    #[test]
    fn serve_unconstrained_empties_and_returns_prior_total() {
        let mut q = QueueState::init_empty();
        q.admit(1.0, 1.0);
        q.admit(2.0, 1.0);
        q.admit(3.0, 1.0);
        let prior_total = q.total();
        let removed = q.serve_unconstrained();
        assert!(approx_eq(removed, prior_total));
        assert!(approx_eq(removed, 6.0));
        assert_eq!(q.batch_count(), 0);
        assert_eq!(q.total(), 0.0);
    }

    #[test]
    fn serve_unconstrained_on_empty_returns_zero() {
        let mut q = QueueState::init_empty();
        let removed = q.serve_unconstrained();
        assert_eq!(removed, 0.0);
        assert_eq!(q.batch_count(), 0);
    }

    #[test]
    fn serve_unconstrained_drains_fully_despite_fp_scale_gap() {
        // A tiny batch queued behind a huge one is the case where
        // `take_from_front(total())` would drift and strand the tiny batch:
        // total() loses the small term, and the running remainder rounds to 0
        // before reaching it. serve_unconstrained sums-then-clears, so it must
        // still empty completely and report exactly the two batch volumes.
        let mut q = QueueState::init_empty();
        q.admit(1e10, 1.0); // [1e10]
        q.admit(1e-7, 1.0); // [1e10, 1e-7]
        let removed = q.serve_unconstrained();
        assert_eq!(
            q.batch_count(),
            0,
            "the tiny trailing batch must be drained"
        );
        assert!(approx_eq(removed, 1e10 + 1e-7));
    }

    #[test]
    fn serve_unconstrained_empties_even_with_a_nonfinite_batch() {
        // A divergent inflow can overflow max(rate,0)*dt to +inf, pushing a
        // non-finite batch. serve_unconstrained MUST still leave the queue empty
        // (a `take_from_front(INFINITY)` implementation would strand every batch
        // behind the inf one, since INFINITY - INFINITY = NaN fails the loop
        // guard). The reported total is non-finite -- garbage in, garbage out --
        // but the queue is drained.
        let mut q = QueueState::init_empty();
        q.admit(3.0, 1.0); // [3]
        q.admit(f64::INFINITY, 1.0); // [3, inf]
        q.admit(5.0, 1.0); // [3, inf, 5]
        let removed = q.serve_unconstrained();
        assert_eq!(
            q.batch_count(),
            0,
            "queue must be empty regardless of an inf batch"
        );
        assert!(removed.is_infinite());
    }

    // ----- container access (§8) -----

    #[test]
    fn batch_contents_are_front_to_back() {
        let mut q = QueueState::init_empty();
        q.admit(7.0, 1.0);
        q.admit(8.0, 1.0);
        q.admit(9.0, 1.0);
        assert_eq!(q.batch_contents(), vec![7.0, 8.0, 9.0]);
    }

    #[test]
    fn batch_count_tracks_batches() {
        let mut q = QueueState::init_empty();
        assert_eq!(q.batch_count(), 0);
        q.admit(1.0, 1.0);
        assert_eq!(q.batch_count(), 1);
        q.admit(1.0, 1.0);
        assert_eq!(q.batch_count(), 2);
    }

    #[test]
    fn batch_in_range_and_out_of_range() {
        let mut q = QueueState::init_empty();
        q.admit(3.0, 1.0);
        q.admit(6.0, 1.0);
        assert_eq!(q.batch(0), Some(3.0)); // front / oldest
        assert_eq!(q.batch(1), Some(6.0)); // back / newest
        assert_eq!(q.batch(2), None); // out of range
    }

    #[test]
    fn total_sums_all_batches() {
        let mut q = QueueState::init_empty();
        q.admit(1.5, 1.0);
        q.admit(2.5, 1.0);
        assert!(approx_eq(q.total(), 4.0));
    }

    // ----- invariant: Sigma batches == total across a mixed sequence -----

    #[test]
    fn sigma_batches_equals_total_after_admit_take_sequence() {
        let mut q = QueueState::init_from_value(2.0); // [2]
        q.admit(3.0, 1.0); // [2, 3]
        q.take_from_front(1.0); // [1, 3]
        q.admit(5.0, 1.0); // [1, 3, 5]
        q.take_from_front(3.0); // [1, 5]  (pop 1, take 2 of the 3-batch)
        q.admit(-4.0, 1.0); // negative -> no batch: [1, 5]

        // The invariant: total() must equal the independent sum of the batch
        // vector at every point, and both must equal the hand-computed value.
        let contents = q.batch_contents();
        let independent_sum: f64 = contents.iter().sum();
        assert!(approx_eq(q.total(), independent_sum));
        assert_eq!(contents, vec![1.0, 5.0]);
        assert!(approx_eq(q.total(), 6.0));
    }
}
