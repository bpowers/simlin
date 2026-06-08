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
/// SCC-size gate (or discovery was requested directly) and loops are ranked
/// by the per-timestep strongest-path heuristic instead. Without this signal
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
}

/// List of loops returned by analysis
#[repr(C)]
pub struct SimlinLoops {
    pub loops: *mut SimlinLoop,
    pub count: usize,
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
    /// inputs -- a value in `[-1, 1]` that IS comparable across targets and
    /// is the correct key for ranking links by importance (GH #652).  When
    /// non-NULL its length equals `score_len`.
    pub relative_score: *mut f64,
    pub relative_score_len: usize,
}

/// Collection of links
#[repr(C)]
pub struct SimlinLinks {
    pub links: *mut SimlinLink,
    pub count: usize,
}

/// A single loop discovered via the strongest-path LTM discovery algorithm.
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
}
