// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! FFI type definitions for cbindgen

use std::os::raw::c_char;

/// Opaque project structure
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinProject {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Opaque simulation structure  
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinSim {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Opaque model structure
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinModel {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Opaque error structure returned by the API
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinError {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Opaque standalone results structure (e.g. an imported VDF file)
#[repr(C)]
#[allow(dead_code)]
pub struct SimlinResults {
    _private: [u8; 0],
    _marker: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
}

/// Loop polarity for C API.
///
/// `MostlyReinforcing`/`MostlyBalancing` ("Rux"/"Bux" in the LTM literature)
/// are the mixed-sign runtime polarities the engine determines when a loop has
/// expressed both signs over a simulation but one dominates with high
/// confidence; they are reported here verbatim rather than coalesced down to
/// `Reinforcing`/`Balancing` (GH #495).  The companion
/// `SimlinLoop.polarity_confidence` / `SimlinDiscoveredLoop.polarity_confidence`
/// carries the `[0.0, 1.0]` confidence ratio behind the classification.
#[repr(C)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimlinLoopPolarity {
    Reinforcing = 0,
    Balancing = 1,
    Undetermined = 2,
    /// "Rux" -- mixed-sign runtime scores, predominantly reinforcing.
    MostlyReinforcing = 3,
    /// "Bux" -- mixed-sign runtime scores, predominantly balancing.
    MostlyBalancing = 4,
}

/// Link polarity for C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinLinkPolarity {
    Positive = 0,
    Negative = 1,
    Unknown = 2,
}

/// The LTM loop-enumeration mode a simulation resolved to.
///
/// `Disabled` means the simulation was created without LTM (`enable_ltm =
/// false`), so no loop enumeration ran. `Exhaustive` means every elementary
/// circuit was enumerated (Johnson). `Discovery` means the model tripped the
/// SCC-size gate (or discovery was requested directly) and loops are found
/// post-simulation from the recorded link scores instead. Without this signal
/// a caller cannot tell why an LTM-enabled run produced empty or different
/// loop results.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinLtmMode {
    Disabled = 0,
    Exhaustive = 1,
    Discovery = 2,
}

/// JSON format specifier for C API
#[repr(C)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimlinJsonFormat {
    Native = 0,
    Sdai = 1,
}

impl TryFrom<u32> for SimlinJsonFormat {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(SimlinJsonFormat::Native),
            1 => Ok(SimlinJsonFormat::Sdai),
            _ => Err(()),
        }
    }
}

/// A single feedback loop
#[repr(C)]
pub struct SimlinLoop {
    pub id: *mut c_char,
    pub variables: *mut *mut c_char,
    pub var_count: usize,
    pub polarity: SimlinLoopPolarity,
    /// Human-meaningful loop name the modeler assigned via `SetLoopName`
    /// (pysimlin `set_loop_name`), or NULL when the loop has no assigned
    /// name.  The struct grew additively for this field (mirroring how
    /// `SimlinLink` gained `relative_score`); `simlin_sizeof_loop` and the
    /// `@simlin/engine` `LOOP_SIZE`/`readLoops` offsets track it.
    pub name: *mut c_char,
    /// Polarity-confidence ratio in `[0.0, 1.0]` behind `polarity` (GH #495):
    /// `1.0` for a clean `Reinforcing`/`Balancing` loop, `0.0` for
    /// `Undetermined`.  On the STRUCTURAL `simlin_analyze_get_loops` surface
    /// this is `1.0`/`0.0` by design (a loop's links are either all signed or
    /// at least one is unknown); the mixed-sign `MostlyReinforcing`/
    /// `MostlyBalancing` variants with intermediate confidence appear on the
    /// discovery surface (`SimlinDiscoveredLoop`).  Adding this `f64` grew the
    /// struct additively (8-byte alignment pushed it past the old 20 bytes);
    /// `simlin_sizeof_loop` and the `@simlin/engine` `LOOP_SIZE`/`readLoops`
    /// offsets track the new size.
    pub polarity_confidence: f64,
    /// RESULT-SCOPED index into `SimlinLoops.partitions` naming the loop's
    /// cycle partition, or -1 for a loop whose stocks resolve to no
    /// parent-level partition (a pure module-internal loop).  A single index
    /// suffices because a feedback loop's stocks form one strongly-connected
    /// set (mirroring `SimlinDiscoveredLoop.partition`).  Indices are dense,
    /// assigned in first-appearance order over this `SimlinLoops` list; they
    /// identify partitions within ONE result only and are not stable across
    /// runs or model edits -- key on the partition's stock-name SET for a
    /// durable identity.  Both this exhaustive surface and the discovery
    /// surface (`SimlinDiscoveredLoop.partition`) partition stocks at
    /// ELEMENT granularity (`population[nyc]`; plain names for scalar
    /// models), so the stock set is a usable cross-surface key for arrayed
    /// models too (GH #746; before that fix this surface partitioned at
    /// variable granularity and the sets matched only for scalar models).
    /// Adding this `i32` grew the struct additively past its old
    /// 32 bytes (`simlin_sizeof_loop` and the `@simlin/engine`
    /// `LOOP_SIZE`/`readLoops` offsets track the new size).
    pub partition: i32,
}

/// List of loops returned by analysis
#[repr(C)]
pub struct SimlinLoops {
    pub loops: *mut SimlinLoop,
    pub count: usize,
    /// The cycle partitions referenced by `loops` (each loop's `partition`
    /// indexes this array).  Dense, in first-appearance order over the loop
    /// list; result-scoped.  Reuses `SimlinDiscoveredPartition` so the
    /// exhaustive/pinned loop surface reports partitions in the same shape as
    /// the discovery surface.  Both surfaces partition stocks at element
    /// granularity (see `SimlinLoop.partition`), so the stock SETS are a
    /// usable cross-surface key for scalar and arrayed models alike.
    /// Appended after `loops`/`count` so the existing container offsets the TS
    /// reader uses are unchanged.
    pub partitions: *mut SimlinDiscoveredPartition,
    pub partition_count: usize,
}

/// Single causal link structure
#[repr(C)]
pub struct SimlinLink {
    pub from: *mut c_char,
    pub to: *mut c_char,
    pub polarity: SimlinLinkPolarity,
    /// Raw LTM link-score series (length `score_len`), or NULL when LTM was
    /// not enabled / the edge has no score column.  The raw score divides by
    /// the change in `to`, so it is NOT comparable across different targets
    /// and is unusable for ranking links globally -- use `relative_score`
    /// (GH #652).
    pub score: *mut f64,
    pub score_len: usize,
    /// Relative LTM link-score series (length `relative_score_len`), or NULL
    /// when `score` is NULL.  The raw score normalized, per target and per
    /// timestep, against the sum of `|score|` over all of `to`'s scored
    /// inputs -- a value in `[-1, 1]` (GH #652).  Comparable between the
    /// inputs of ONE target; see `scored_input_count` for the cross-target
    /// ranking caveat.  When non-NULL its length equals `score_len`.
    pub relative_score: *mut f64,
    pub relative_score_len: usize,
    /// The size of `relative_score`'s normalization group (GH #998): how
    /// many CONTRIBUTING links share this link's `to` target, itself
    /// included; 0 when this link never contributes (no score series, or an
    /// all-NaN one -- an all-NaN series adds no summand to any step's
    /// denominator, so it is no competition).  A group of ONE reads exactly
    /// `±1` at every step BY CONSTRUCTION -- ranking links globally by
    /// `|relative_score|` floats such no-competition links to the top (58 of
    /// C-LEARN's global top 100 were single-input targets).  Group links by
    /// `to` and rank within a group; use this field to detect the trivial
    /// groups.  Per-step residual: a link NaN at SOME steps counts here yet
    /// leaves its siblings momentarily unopposed at those steps -- a scalar
    /// cannot carry that.  Appended additively (`simlin_sizeof_link` and the
    /// `@simlin/engine` `LINK_SIZE`/`readLinks` offsets track it).
    pub scored_input_count: usize,
}

/// Collection of links
#[repr(C)]
pub struct SimlinLinks {
    pub links: *mut SimlinLink,
    pub count: usize,
}

/// A single loop found by post-simulation LTM loop discovery.
///
/// This mirrors `SimlinLoop` but adds a per-timestep `importance` series.
/// We do NOT reuse `SimlinLoop` (despite the score-on-loop suggestion in the
/// task brief): `SimlinLoop` has no score field, and adding one would change
/// its wasm32 layout (which `@simlin/engine` asserts against `simlin_sizeof_loop`).
/// A separate struct keeps the discovery surface from disturbing the existing
/// structural-loop ABI that TypeScript/Python read.
#[repr(C)]
pub struct SimlinDiscoveredLoop {
    /// Deterministic loop id (`r1`, `b1`, `u1`, ...).
    pub id: *mut c_char,
    /// Variable names around the loop, with the first variable repeated at the
    /// end so the chain closes.  `var_count` entries.
    pub variables: *mut *mut c_char,
    pub var_count: usize,
    pub polarity: SimlinLoopPolarity,
    /// Per-timestep |importance| series (length `importance_len`, matching the
    /// analysis time array).  Owned `f64` buffer freed with the loop.
    pub importance: *mut f64,
    pub importance_len: usize,
    /// Human-meaningful loop name the modeler assigned via `SetLoopName`
    /// (pysimlin `set_loop_name`), or NULL when the loop has no assigned
    /// name.  Owned `c_char` buffer freed with the loop.
    pub name: *mut c_char,
    /// RESULT-SCOPED index into `SimlinDiscoveryResult.partitions` naming the
    /// loop's cycle partition, or -1 for a loop whose stocks resolve to no
    /// parent-level partition (a pure module-internal loop).  Indices are
    /// dense, assigned in first-appearance order over the ranked loop list;
    /// they identify partitions within ONE discovery result only and are not
    /// stable across runs or model edits.
    pub partition: i32,
    /// Polarity-confidence ratio in `[0.0, 1.0]` behind `polarity` (GH #495):
    /// `1.0` for a clean `Reinforcing`/`Balancing` loop, a value below 1.0 for
    /// a mixed-sign `MostlyReinforcing`/`MostlyBalancing` loop, `0.0` for
    /// `Undetermined`.  This is the high-value confidence surface: discovery
    /// classifies loops from runtime score series, so the Rux/Bux variants and
    /// their intermediate confidences actually appear here.
    pub polarity_confidence: f64,
}

/// One cycle partition referenced by a discovery result's loops: a group of
/// stocks connected by feedback, within which relative loop scores are
/// normalized and therefore comparable.  Lets callers group/filter loops
/// partition-by-partition (e.g. lead with the model's giant component).
#[repr(C)]
pub struct SimlinDiscoveredPartition {
    /// The partition's stock names (element-level for arrayed models),
    /// sorted lexicographically.  `stock_count` entries.
    pub stocks: *mut *mut c_char,
    pub stock_count: usize,
    /// Number of loops in the returned loop list that belong to this
    /// partition.
    pub loop_count: usize,
}

/// A time interval during which a specific set of loops dominates behavior.
///
/// Dominance is computed WITHIN a cycle partition (GH #998): a loop's
/// importance series is its share of its own partition's total, so
/// cross-partition ranking is not well-defined and a loop alone in its
/// partition would read exactly 1.0 at every active step.  Each period
/// therefore says which partition it describes, and a result carries one
/// period timeline per partition (partition-major order, most-competitive
/// partition first).
#[repr(C)]
pub struct SimlinDominantPeriod {
    /// Start time of this period.
    pub start: f64,
    /// End time of this period.
    pub end: f64,
    /// Names of the dominant loops during this period (`dominant_loop_count`).
    pub dominant_loops: *mut *mut c_char,
    pub dominant_loop_count: usize,
    /// Combined relative score of the dominant loops.
    pub combined_score: f64,
    /// RESULT-SCOPED index into `SimlinDiscoveryResult.partitions` naming the
    /// cycle partition this period describes -- the same index space as
    /// `SimlinDiscoveredLoop.partition` -- or -1 for a period of a loop with
    /// no parent-level partition (a module-internal loop, which competes only
    /// against itself, mirroring the ranking's per-loop Solo groups).
    /// Appended additively (GH #998).
    pub partition: i32,
}

/// The cohesive output of one discovery run: discovered loops, dominant
/// periods, and whether the time budget elapsed before discovery finished.
///
/// Returning loops + periods + truncated together is a deliberate exception to
/// libsimlin's "keep the FFI small/orthogonal, no bulk endpoints" rule: these
/// three are the single result of ONE expensive analysis run, not a batch
/// convenience.  Splitting them across separate FFIs would force the caller to
/// re-run discovery (the costly part) once per output.
#[repr(C)]
pub struct SimlinDiscoveryResult {
    pub loops: *mut SimlinDiscoveredLoop,
    pub loop_count: usize,
    pub periods: *mut SimlinDominantPeriod,
    pub period_count: usize,
    /// The cycle partitions referenced by `loops` (each loop's `partition`
    /// indexes this array).  Dense, in first-appearance order over the
    /// ranked loop list; result-scoped.
    pub partitions: *mut SimlinDiscoveredPartition,
    pub partition_count: usize,
    /// Non-zero when discovery hit its wall-clock `budget_ms` before finishing,
    /// so `loops`/`periods` may be partial.
    pub truncated: bool,
    /// Non-zero when discovery's cross-element-through-aggregate loop recovery
    /// (GH #696) hit its reducer-loop-count budget, so some cross-agg reducer
    /// loops are absent from `loops`.  Distinct from `truncated` (the wall-clock
    /// time budget): this is the structural-completeness signal (GH #515/#696)
    /// that mirrors exhaustive mode's analogous salsa Warning, surfacing the
    /// completeness asymmetry that previously left discovery callers blind.
    pub agg_recovery_truncated: bool,
    /// Non-zero when discovery's candidate generation was the union-graph
    /// circuit enumeration AND it ran to completion: `loops` is then the
    /// retention/ranking pipeline's selection from the PROVABLY COMPLETE set
    /// of loops that can ever score, so discovery was exact rather than
    /// heuristic (exact for cross-aggregate reducer loops too only while
    /// `agg_recovery_truncated` is also false: those are stitched under their
    /// own budget).  Zero means the shortest-path fallback generated the
    /// candidates -- an explicit SAMPLE of the loop universe -- because the
    /// enumeration's budgets or `budget_ms` did not allow it to finish.  Read
    /// this before treating an absent loop as evidence the model has none.
    ///
    /// Meaningless (`false` by construction) when `analysis_error` is
    /// non-NULL: analysis never reached candidate generation at all, so this
    /// is not "a sample" in the sense above -- check `analysis_error` first.
    pub enumeration_complete: bool,
    /// How many loops passed discovery's retention filter, BEFORE the
    /// reported-loop cap truncated `loops`.  Equal to `loop_count` when the
    /// cap did not bind, and above it when it did -- the signal that `loops`
    /// is a coverage-aware SUBSET of the loops worth reporting (each step's
    /// dominant loop per competing partition is guaranteed a slot while those
    /// dominant loops fit the cap, the rest
    /// is filled by mean importance): presented in importance order, but not
    /// a strict most-important-first prefix.
    pub retained_loops: usize,
    /// The size of the candidate universe: how many DISTINCT loops' mass the
    /// discovery denominators sum -- the ever-simultaneously-active
    /// elementary cycles the enumeration found, minus any non-representative
    /// duplicate the retention pass merges into a single reported loop, plus
    /// any cross-aggregate loop stitched together from disjoint elementary
    /// pieces -- which is the population every reported loop's importance is
    /// measured against.  `-1` when `enumeration_complete` is zero, since a
    /// sampled report has no universe to describe; the two fields always
    /// agree, and the sentinel keeps "the fallback ran" distinct from a
    /// genuinely empty universe (`0`).  Also `-1` when `analysis_error` is
    /// non-NULL (analysis never ran, so there is no universe of any kind).
    pub universe_loops: i64,
    /// Non-NULL when the model could not be compiled or analyzed for LTM at
    /// all -- a malformed equation, an unresolved reference, or a hard
    /// compile failure such as the non-Euler-integration-with-a-stock-loop
    /// rejection (GH #486, which needs Euler stepping for its flow-to-stock
    /// link-score formula).  When set, every OTHER field describes an
    /// analysis that never started: `loops`/`periods`/`partitions` are
    /// empty, `loop_count`/`period_count`/`partition_count`/`retained_loops`
    /// are `0`, `enumeration_complete` is `false`, and `universe_loops` is
    /// `-1` -- the SAME shape a genuinely sampled (fallback) run with zero
    /// discovered loops would report, which is why `analysis_error` is the
    /// field to check FIRST.  The three outcomes, in the order to test them:
    ///
    /// 1. **Never ran**: `analysis_error` non-NULL.  Nothing below is
    ///    meaningful; the message names the compile failure.
    /// 2. **Sampled**: `analysis_error` NULL, `enumeration_complete` is
    ///    `false`.  `loops` is an explicit SAMPLE of the loop universe (the
    ///    shortest-path fallback ran because the exact enumeration's budgets
    ///    or the caller's `budget_ms` did not allow it to finish);
    ///    `universe_loops` is `-1` here too, but for a different reason (a
    ///    sample has no universe to describe, not that analysis never ran).
    /// 3. **Exact**: `analysis_error` NULL, `enumeration_complete` is
    ///    `true`.  `loops` is the retention/ranking selection from the
    ///    PROVABLY COMPLETE candidate universe; `universe_loops` names that
    ///    universe's size.
    ///
    /// Owned; freed alongside the rest of the result by
    /// `simlin_free_discovery_result`.
    pub analysis_error: *mut c_char,
}
