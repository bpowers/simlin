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

/// Loop polarity for C API
#[repr(C)]
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SimlinLoopPolarity {
    Reinforcing = 0,
    Balancing = 1,
    Undetermined = 2,
}

/// Link polarity for C API
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimlinLinkPolarity {
    Positive = 0,
    Negative = 1,
    Unknown = 2,
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
    pub score: *mut f64,
    pub score_len: usize,
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
/// its wasm32 layout, which `@simlin/engine` asserts is exactly 16 bytes via
/// `simlin_sizeof_loop`.  A separate struct keeps the discovery surface from
/// disturbing the existing structural-loop ABI that TypeScript/Python read.
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
    /// Non-zero when discovery hit its `budget_ms` before finishing.
    pub truncated: bool,
}
