// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compiling conveyors into the VM.
//!
//! The conveyor runtime pass ([`crate::conveyor`]) is VM-native, not bytecode
//! (docs/design/conveyors.md §9.3). Rather than teach the salsa compiler about
//! belts, this module bridges the datamodel to the VM in two steps that bracket
//! ordinary compilation:
//!
//! 1. [`expand_conveyors`] rewrites a project so every conveyor parameter
//!    (`<len>`, `<capacity>`, `<in_limit>`, `<sample>`, `<arrest>`, each leak
//!    fraction) becomes a hidden auxiliary variable, and every conveyor-driven
//!    flow (the primary outflow and the leak flows) gets a placeholder `0`
//!    equation so it compiles to a writable slot instead of erroring on an
//!    empty equation. The hidden auxes flow through normal layout/compilation,
//!    so their values land in ordinary data-buffer slots. A [`ConveyorMeta`]
//!    per conveyor records the synthesized names.
//! 2. [`resolve_plans`] looks each name up in the compiled simulation's offset
//!    map to produce [`ConveyorPlan`]s the VM's conveyor pass reads.
//!
//! The build entry points that drive these steps live in
//! [`crate::queue_compile`] (`compile_sim`/`build_compiled`/`build_sim`): a stock
//! may be a conveyor, a queue, or the two may couple, so ONE unified build path
//! runs both expansions and resolves both plan sets against the same compiled
//! offsets. This module supplies the conveyor half -- the expansion, plan
//! resolution, and VM-native pass helpers that path composes.
//!
//! Because the conveyor stock's inflows/outflows carry the pass-computed rates
//! before stock integration, the stock integrates to `Σ belt contents` through
//! the ordinary Stocks phase -- no special-casing of stock integration is
//! needed (the §4.3 conservation identity guarantees `Δstock = admitted - out -
//! leak`).
//!
//! Scope: this handles conveyors in the **main model** under Euler, including
//! all five spread-input placements (§8): `beginning`/`even`/`dest`, `dist`
//! (the distribution graphical function or numeric array, resolved at expand
//! time and sampled per step because the entry depth can vary), and `source`
//! (mirroring an upstream leak's per-slat leakage, coupled by flow identity).
//! It also handles **arrayed conveyors** (§10): an arrayed conveyor stock is
//! `N_elem` independent belts, one per array element, each with its own
//! `ConveyorState`, transit time, leak flows, capacity, and inflow limit. The
//! synthesized parameter/fraction auxes and the driven-flow placeholders are
//! made arrayed with the stock's dimensions (so each element gets its own
//! slot), and [`resolve_plans`] flattens one arrayed [`ConveyorMeta`] into one
//! [`ConveyorPlan`] per element -- reusing the scalar per-belt runtime pass
//! unchanged (a scalar conveyor is the degenerate 1-element case). Because the
//! datamodel's `Conveyor` block holds one expression per attribute, every
//! element shares that expression (the apply-to-all arrayed form); a shared
//! expression may still reference other arrayed variables to yield distinct
//! per-element values. Per-`<element>`-block DISTINCT conveyor attributes are
//! not representable in the datamodel, so they cannot arise here.
//!
//! It also handles **container access** (§10): reading a belt's slat contents in
//! equations. The compiler cannot expose the runtime-dynamic slat vector to the
//! bytecode, so each SUPPORTED container access -- `SUM`/`MEAN`/`SIZE`/`MIN`/
//! `MAX`/`STDDEV` over a single belt, and `conv[j]` for a compile-time-constant
//! slat index `j` -- is rewritten in place to a reference to a synthesized hidden
//! STOCK, and the conveyor pass computes the result natively and PUBLISHES it
//! into that stock's slot at step-start (before the flows phase / after belt
//! init). Modeling the container variable as a no-flow stock is what makes the
//! published value survive the flows phase (a stock slot is read but never
//! recomputed by flows, and integrated unchanged by stocks) and gives it
//! start-of-step read semantics for free. For an arrayed conveyor the container
//! stock is arrayed over the same dims, so `conv[elem]`/`conv[elem,j]` index it
//! element-wise via the ordinary array machinery. Genuinely-unlowerable residual
//! forms (a reducer over an EXPRESSION involving the belt, a dynamic slat index,
//! ranges/wildcards over slats, a bare arrayed-conveyor reducer other than SUM)
//! stay loud-rejected with `ConveyorContainerAccessUnsupported`.
//!
//! Conveyors inside submodules are a later build-sequence step; queue coupling
//! is handled by the unified build path in [`crate::queue_compile`].

use std::collections::HashMap;

use crate::common::{Canonical, DimensionName, ErrorCode, Ident, canonicalize};
use crate::conveyor::{
    ConveyorState, LeakConfig, PhaseAInputs, PhaseAResult, PhaseBInputs, Placement,
};
use crate::datamodel::{self, Equation};

/// A leak flow's synthesized metadata (names + static zone/integer config).
#[derive(Clone, Debug, PartialEq)]
pub struct LeakMeta {
    /// Canonical name of the leak outflow (its slot receives the leak rate).
    pub flow: String,
    /// Canonical name of the hidden aux holding the leak fraction.
    pub frac_aux: String,
    pub zone_start: f64,
    pub zone_end: f64,
    pub integers: bool,
    /// Canonical name of the downstream conveyor whose belt this leak flow feeds
    /// (for the arrested-destination skip, §4.3 step 2), resolved exactly like
    /// [`ConveyorMeta::primary_dest_conveyor`]. `None` if the leak feeds an
    /// ordinary stock/cloud, or its OWN conveyor (the same `owner != stock`
    /// self-loop filter the primary applies -- an arrested conveyor never leaks
    /// at all, so a self-leak can never hold against its own arrest).
    pub dest_conveyor: Option<String>,
}

/// The container-access result a synthesized container variable publishes each
/// step from a belt's (or queue's) start-of-step state (§10). Each maps to
/// exactly one scalar per container (one array element for an arrayed one):
///
/// - `Slat(j)` is the 1-based index into the container's element vector
///   (exit-first for a conveyor belt, front-to-back for a queue -- `conv[j]` /
///   `queue[j]`); `j` outside `[1, L]` (L = the current element count) yields
///   NaN, matching an out-of-range dynamic array subscript.
/// - the reducers apply over the container's current element vector (length L),
///   following the VM's empty-reducer conventions (`Sum` -> 0, the rest -> NaN;
///   see `vm.rs`). `Size` is the physical element count L.
///
/// This enum, and the container-access rewrite/publish machinery it feeds, are
/// shared by BOTH conveyors and queues (docs/design/queues.md §8: queue container
/// access reuses the conveyor mechanism verbatim). Only the SOURCE VECTOR differs
/// -- a conveyor supplies `slat_contents()` (exit-first), a queue supplies
/// `batch_contents()` (front-to-back) -- so the reducer/index math is identical
/// ([`container_value_from_slice`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContainerKind {
    Slat(usize),
    Sum,
    Mean,
    Size,
    Min,
    Max,
    Stddev,
}

/// A synthesized container variable's metadata: its canonical name (a hidden
/// STOCK, arrayed over the conveyor's/queue's dims when arrayed) and the access
/// it computes (§10). The variable is modeled as a stock so the Flows phase never
/// recomputes its slot -- the conveyor/queue pass overwrites its `curr` slot at
/// step-start and the Stocks phase (a no-flow INTEG) leaves it unchanged for the
/// rest of the step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerMeta {
    pub name: String,
    pub kind: ContainerKind,
}

/// A resolved container variable: its data-buffer slot offset plus the access to
/// compute from the belt/queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContainerPlan {
    pub off: usize,
    pub kind: ContainerKind,
}

/// Owner-specific naming and diagnostics for the shared container-access rewrite
/// (§10). Conveyors and queues drive the identical AST rewrite; they differ only
/// in the synthesized-variable name prefix (`$conv$` vs `$queue$`, kept
/// collision-free against ordinary variables) and the diagnostic wording. The
/// unsupported-form error CODE is shared ([`ErrorCode::ConveyorContainerAccessUnsupported`],
/// per docs/design/queues.md §8 "the same loud `ContainerAccessUnsupported`
/// rejection conveyors use"); the message noun/help make it read correctly for
/// each owner.
#[derive(Clone, Copy)]
pub(crate) struct ContainerNaming {
    /// Name-prefix segment: `conv` or `queue`. `$<prefix>$sum$<stock>` etc.
    pub prefix: &'static str,
    /// Diagnostic noun: `conveyor` or `queue`.
    pub noun: &'static str,
    /// Supported-forms help clause for the unsupported-form diagnostic.
    pub supported_help: &'static str,
}

impl ContainerNaming {
    pub(crate) const CONVEYOR: ContainerNaming = ContainerNaming {
        prefix: "conv",
        noun: "conveyor",
        supported_help: "of a single belt and conv[j] for a constant slat index j",
    };
    pub(crate) const QUEUE: ContainerNaming = ContainerNaming {
        prefix: "queue",
        noun: "queue",
        supported_help: "of a single queue and queue[k] for a constant batch index k",
    };
}

/// Compute one container-access result from a container's current
/// (start-of-step) element vector (§10), shared by the conveyor and queue
/// publish passes. `vec` is exit-first slat volumes for a conveyor or
/// front-to-back batch volumes for a queue; the index/reducer math is identical.
/// The reducer conventions match the VM's array reducers (`vm.rs`): `Sum` -> 0 on
/// an empty container, every other reducer -> NaN; `Size` is the element count;
/// `Stddev` is the POPULATION standard deviation (divisor N, not N-1).
/// `Slat(j)` is 1-based; `j` outside `[1, len]` yields NaN.
///
/// The conveyor pass drives this over `ConveyorState::slat_contents` -- where
/// `contents() == Σ slat_contents()` holds exactly and the slat count is
/// `slat_contents().len()`, so the published values agree with the belt's own
/// accessors --
/// and the queue pass over `QueueState::batch_contents`, where
/// `total == Σ batch_contents` and `batch_count == batch_contents.len()` hold
/// the same way.
pub(crate) fn container_value_from_slice(vec: &[f64], kind: &ContainerKind) -> f64 {
    match kind {
        ContainerKind::Slat(j) => {
            if *j < 1 {
                f64::NAN
            } else {
                vec.get(*j - 1).copied().unwrap_or(f64::NAN)
            }
        }
        ContainerKind::Size => vec.len() as f64,
        ContainerKind::Sum => vec.iter().sum(),
        ContainerKind::Mean | ContainerKind::Min | ContainerKind::Max | ContainerKind::Stddev => {
            if vec.is_empty() {
                return f64::NAN;
            }
            match kind {
                ContainerKind::Mean => vec.iter().sum::<f64>() / vec.len() as f64,
                ContainerKind::Min => vec.iter().copied().fold(f64::INFINITY, f64::min),
                ContainerKind::Max => vec.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ContainerKind::Stddev => {
                    let n = vec.len() as f64;
                    let mean = vec.iter().sum::<f64>() / n;
                    let var = vec.iter().map(|v| (v - mean).powf(2.0)).sum::<f64>() / n;
                    var.sqrt()
                }
                _ => unreachable!("outer match restricts kind"),
            }
        }
    }
}

/// A resolved `dist` distribution (§8). Because the entry depth `d` can vary
/// with a time-varying transit time, the per-slat weights `w_i` cannot be
/// pre-computed; run_pass samples this profile at `x_i = 1 - (i+0.5)/d` each
/// step to build the `Placement::Dist(weights)` it hands to the belt.
#[derive(Clone, Debug, PartialEq)]
pub enum DistProfile {
    /// A 1-D array of length `m`: `w_i = array[floor(x_i * m)]` (index clamped
    /// to `m-1`), per §8.
    Array(Vec<f64>),
    /// A graphical function as sorted `(x, y)` pairs: `w_i = g(x_i)` with the
    /// engine's continuous-lookup semantics (linear interpolation, flat
    /// extrapolation past the endpoints -- the same evaluation a `LOOKUP` call
    /// on the named GF variable would use).
    Gf(Vec<(f64, f64)>),
}

/// A conveyor inflow's metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct InflowMeta {
    /// Canonical name of the inflow.
    pub flow: String,
    /// True iff this inflow is a driven outflow of some conveyor (so it is
    /// admitted unconditionally and bypasses capacity/inflow-limit, §4.3).
    pub conveyor_driven: bool,
    /// Static inflow placement on the belt (§8): `Beginning`/`Even`/`Dest`, or
    /// `Beginning` as the degenerate fallback for `dist`/`source`.
    pub placement: Placement,
    /// `dist` distribution profile (§8), `Some` iff this inflow uses
    /// `isee:spreadflow="dist"` with a representable distribution.
    pub dist: Option<DistProfile>,
    /// True iff this inflow uses `isee:spreadflow="source"` (§8): mirror an
    /// upstream leak flow's per-slat leakage. The upstream leak is resolved by
    /// flow identity (the inflow slot *is* that leak's slot) at run time; if no
    /// upstream leak matches, the placement degrades to `Beginning`.
    pub source: bool,
}

/// Per-conveyor synthesized metadata, produced by [`expand_conveyors`] and
/// resolved to offsets by [`resolve_plans`].
#[derive(Clone, Debug, PartialEq)]
pub struct ConveyorMeta {
    pub stock: String,
    pub len_aux: String,
    pub cap_aux: Option<String>,
    pub inlim_aux: Option<String>,
    pub sample_aux: Option<String>,
    pub arrest_aux: Option<String>,
    pub discrete: bool,
    pub exponential_leak: bool,
    pub ignore_earlier_zone_losses: bool,
    pub primary_out: String,
    pub leaks: Vec<LeakMeta>,
    pub inflows: Vec<InflowMeta>,
    /// Container-access variables reading this conveyor's belt (§10). Each is a
    /// synthesized hidden stock whose slot the pass publishes at step-start. For
    /// an arrayed conveyor the container variable is arrayed over the same dims,
    /// so element `e` of the container aligns with belt `e`.
    pub containers: Vec<ContainerMeta>,
    /// Canonical name of the downstream conveyor whose belt this conveyor's
    /// primary outflow feeds (for the held-exit rule, §4.3 step 3). `None` if
    /// the primary outflow feeds an ordinary stock/cloud.
    pub primary_dest_conveyor: Option<String>,
    /// Per-element subscript suffixes for an arrayed conveyor (§10), in the same
    /// row-major order the compiled offset map lays out an arrayed variable's
    /// elements (`calc_flattened_offsets_incremental` via `SubscriptIterator`).
    /// Each entry is the canonical `elem1,elem2` suffix so
    /// [`resolve_plans`] can form the subscripted offset keys `name[elem]`. An
    /// arrayed conveyor is `N_elem` independent belts, one per element; this is
    /// empty for a scalar conveyor (the degenerate 1-belt case).
    pub element_subscripts: Vec<String>,
    /// §7.2 explicit init lists parsed from the stock's `<eqn>`: one entry
    /// per element belt, aligned with `element_subscripts` (a single entry
    /// for a scalar conveyor); EMPTY when the stock has no list init at all.
    /// `Some(list)` fills that belt directly (front first, via
    /// [`ConveyorState::init_explicit`] in [`init_belts`]); `None` keeps the
    /// §7.1 steady fill from that element's own compiled initial. A scalar
    /// or apply-to-all list is replicated across every belt; a
    /// non-apply-to-all `<element>` list applies to its own element only
    /// (XMILE §4.5.2: element equations vary between elements). The expanded
    /// stock's `<eqn>` was rewritten to a constant placeholder carrying each
    /// list-initialized element's NORMALIZED belt total
    /// ([`normalized_init_total`]); [`init_belts`] defensively re-writes the
    /// (identical) total into the stock slot.
    pub init_values: Vec<Option<Vec<f64>>>,
}

/// A resolved leak flow: slot offsets plus static config.
#[derive(Clone, Debug, PartialEq)]
pub struct LeakPlan {
    pub flow_off: usize,
    pub frac_off: usize,
    pub zone_start: f64,
    pub zone_end: f64,
    pub integers: bool,
    /// Index into the plan list of the conveyor this leak flow feeds (for the
    /// arrested-destination skip, §4.3 step 2); `None` for an ordinary sink.
    pub dest_conveyor: Option<usize>,
}

/// A resolved inflow: its slot plus whether it is conveyor-driven.
#[derive(Clone, Debug, PartialEq)]
pub struct InflowPlan {
    pub flow_off: usize,
    pub conveyor_driven: bool,
    pub placement: Placement,
    /// `dist` distribution profile (§8), sampled per step by run_pass.
    pub dist: Option<DistProfile>,
    /// `source` placement (§8): mirror an upstream leak by flow identity.
    pub source: bool,
    /// The upstream leak a `source` inflow mirrors, as `(plan index, leak
    /// index)` into the resolved plan list -- precomputed by [`resolve_plans`]
    /// (the flow-identity match is a pure function of the compile-time-constant
    /// plans, like the queue [`crate::queue_compile::CouplingTable`], GH #878)
    /// so the per-step pass reads it instead of rescanning every plan's leak
    /// list. `None` when `source` is false, or when no upstream leak shares
    /// this inflow's slot (the `source` placement then falls back to
    /// `Beginning`).
    pub source_leak: Option<(usize, usize)>,
    /// True iff this inflow is the shared flow of a queue-conveyor coupling: it
    /// is a queue's primary outflow that this (discrete) conveyor admits
    /// unconditionally (queues.md §9 / conveyors.md §11). The combined queue pass
    /// writes its slot to `served / dt` and debits the discrete inflow budget
    /// BEFORE phase_b, so phase_b routes it through the unconditional
    /// `conv_inflows` path (like a conveyor-driven inflow) rather than treating it
    /// as an equation-driven request (which would re-quantize it). Set by
    /// [`crate::queue_compile`]'s coupling resolution, `false` for every ordinary
    /// inflow -- so a conveyor-only model is byte-identical.
    pub queue_coupled: bool,
}

/// A fully-resolved conveyor: data-buffer slot offsets for the VM's pass.
#[derive(Clone, Debug, PartialEq)]
pub struct ConveyorPlan {
    /// The conveyor stock's canonical name (base name for an arrayed conveyor),
    /// used only to name the belt in a runtime error (e.g.
    /// [`ErrorCode::ConveyorTransitTooLong`]).
    pub name: String,
    pub stock_off: usize,
    pub len_off: usize,
    pub cap_off: Option<usize>,
    pub inlim_off: Option<usize>,
    pub sample_off: Option<usize>,
    pub arrest_off: Option<usize>,
    pub discrete: bool,
    pub exponential_leak: bool,
    pub ignore_earlier_zone_losses: bool,
    pub primary_out_off: usize,
    pub leaks: Vec<LeakPlan>,
    pub inflows: Vec<InflowPlan>,
    /// Container-access variables reading this belt (§10): resolved slot + kind.
    /// The pass publishes each from this belt's start-of-step state.
    pub containers: Vec<ContainerPlan>,
    /// Index into the plan list of the downstream conveyor this conveyor's
    /// primary outflow feeds (for the held-exit rule); `None` otherwise.
    pub primary_dest_conveyor: Option<usize>,
    /// §7.2 explicit init list (front first), `None` for §7.1 steady init.
    /// [`init_belts`] fills the belt via `ConveyorState::init_explicit`; the
    /// stock's compiled `<eqn>` already holds the same normalized total (the
    /// expansion-time [`normalized_init_total`] placeholder), so the
    /// write-back into `stock_off` is defense in depth.
    pub init_values: Option<Vec<f64>>,
}

impl ConveyorPlan {
    /// Every data-buffer slot the conveyor pass WRITES each step: the driven
    /// primary-outflow and leak rates ([`run_phase_a`]) and the published
    /// container-access values ([`publish_container_values`]). These slots are
    /// pass-owned -- the placeholder `0` equation the expansion gave each
    /// driven flow compiles to an `AssignConstCurr` the pass overwrites every
    /// step -- so a constant override on one of them could never affect the
    /// simulation and must be rejected instead of silently accepted (GH #871).
    /// The container slots are no-flow stocks (never classified overridable),
    /// but they are included so this method's contract stays "every
    /// pass-written slot" rather than depending on how the placeholder stock
    /// happens to compile.
    ///
    /// Equation-driven INFLOW slots are deliberately absent: the pass reads
    /// their Flows-phase value as the requested rate (writing back only the
    /// admitted rate), so an override on a constant inflow is a genuine input
    /// each step. A conveyor-driven or queue-coupled inflow's slot is the
    /// upstream belt's primary outflow / the queue's driven outflow, covered
    /// by that owning plan.
    pub fn pass_written_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        std::iter::once(self.primary_out_off)
            .chain(self.leaks.iter().map(|l| l.flow_off))
            .chain(self.containers.iter().map(|c| c.off))
    }
}

/// Canonicalize a display name to an owned `String`. Shared with
/// [`crate::queue_compile`] (the queue half of the unified build path).
pub(crate) fn canon(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// Does the main model of `project` contain a stock satisfying `marker`? The
/// shared core of [`project_has_conveyor`] and
/// [`crate::queue_compile::project_has_queue`] (and their expansions' no-op
/// fast paths), so every dispatch site tests the identical predicate.
pub(crate) fn main_model_has_stock(
    project: &datamodel::Project,
    main_model: &str,
    marker: impl Fn(&datamodel::Stock) -> bool,
) -> bool {
    let main_canon = canon(main_model);
    project.models.iter().any(|m| {
        canon(&m.name) == main_canon
            && m.variables
                .iter()
                .any(|v| matches!(v, datamodel::Variable::Stock(s) if marker(s)))
    })
}

/// Does the named model in `project` contain any conveyor stock? The cheap
/// predicate [`crate::queue_compile::compile_sim`] uses to decide whether to route
/// through the special build path instead of the ordinary incremental compile.
pub fn project_has_conveyor(project: &datamodel::Project, main_model: &str) -> bool {
    main_model_has_stock(project, main_model, |s| s.compat.conveyor.is_some())
}

/// Synthesized hidden-aux name for one of a conveyor's parameter expressions.
/// The `$conv$` prefix and `$`-separators keep the name canonical (no `.`, no
/// module `·`) and collision-free against ordinary model variables.
fn param_aux_name(stock: &str, param: &str) -> String {
    format!("$conv${}${param}", canon(stock))
}

fn leak_frac_name(flow: &str) -> String {
    format!("$conv$leak${}$frac", canon(flow))
}

/// Synthesized hidden-stock name for a container-access variable over container
/// `stock` (§10). `$<prefix>$<tag>$<stock>` (e.g. `$conv$sum$belt`,
/// `$queue$sum$waiting`) or, for an index, `$<prefix>$slat$<stock>$<j>`. The
/// `$`-separated form stays canonical and collision-free against ordinary model
/// variables and the other synthesized `$conv$`/`$queue$` auxes. The `prefix`
/// (`conv`/`queue`) comes from the owner's [`ContainerNaming`].
fn container_var_name(naming: &ContainerNaming, stock: &str, kind: &ContainerKind) -> String {
    let stock = canon(stock);
    let p = naming.prefix;
    match kind {
        ContainerKind::Slat(j) => format!("${p}$slat${stock}${j}"),
        ContainerKind::Sum => format!("${p}$sum${stock}"),
        ContainerKind::Mean => format!("${p}$mean${stock}"),
        ContainerKind::Size => format!("${p}$size${stock}"),
        ContainerKind::Min => format!("${p}$min${stock}"),
        ContainerKind::Max => format!("${p}$max${stock}"),
        ContainerKind::Stddev => format!("${p}$stddev${stock}"),
    }
}

/// A resolved inflow placement: the static fallback plus, for the dynamic
/// methods, the data run_pass needs to build per-step weights (§8).
struct ResolvedPlacement {
    placement: Placement,
    dist: Option<DistProfile>,
    source: bool,
}

/// Map an inflow flow's `isee:spreadflow` (§8) to a [`ResolvedPlacement`]. A
/// flow with no placement (or not found -- e.g. a cloud source) defaults to
/// `Beginning`. `dist` resolves its `<isee:distrib_eq>` to a graphical-function
/// or numeric-array [`DistProfile`]; a distribution that is neither (an empty,
/// inline-expression, or dangling reference) is rejected loudly rather than
/// guessed. `source` is recorded as a flag and coupled to the upstream leak at
/// run time.
fn resolve_placement(
    model: &datamodel::Model,
    flow: &str,
) -> Result<ResolvedPlacement, (ErrorCode, String)> {
    // Marker detection and flow fetch are one lookup, skipping canon-matching
    // twins WITHOUT a spreadflow so the marker-carrying twin wins -- the same
    // convention `find_leak_flow` established for `<leak/>` (GH #870).
    // Duplicate canonical idents are rejected upstream at the build
    // chokepoints (`build_compiled` / `compile_project_incremental`, GH #885),
    // so twins are unreachable from production; this keeps expansion
    // self-consistent for direct `expand_conveyors` callers. For a
    // single-flow model the behavior is identical: an absent marker still
    // yields `None` -> `Beginning`.
    let spread = model.variables.iter().find_map(|v| match v {
        datamodel::Variable::Flow(f) if canon(&f.ident) == flow => f.compat.spreadflow.clone(),
        _ => None,
    });
    let plain = |p: Placement| ResolvedPlacement {
        placement: p,
        dist: None,
        source: false,
    };
    match spread {
        None | Some(datamodel::SpreadFlow::Beginning) => Ok(plain(Placement::Beginning)),
        Some(datamodel::SpreadFlow::Even) => Ok(plain(Placement::Even)),
        Some(datamodel::SpreadFlow::Dest) => Ok(plain(Placement::Dest)),
        Some(datamodel::SpreadFlow::Dist(spec)) => {
            let profile = resolve_dist_profile(model, &spec, flow)?;
            Ok(ResolvedPlacement {
                placement: Placement::Beginning,
                dist: Some(profile),
                source: false,
            })
        }
        Some(datamodel::SpreadFlow::Source) => Ok(ResolvedPlacement {
            placement: Placement::Beginning,
            dist: None,
            source: true,
        }),
    }
}

/// Resolve a `dist` `<isee:distrib_eq>` (§8) to a [`DistProfile`]. Two
/// representable forms, in precedence order:
///
/// 1. A **graphical-function variable name** (the Stella form: `distrib_eq`
///    names an aux/flow carrying the density profile as its graphical function;
///    in practice a lookup-only holder, but any variable with a `<gf>` is
///    accepted since only its curve shape is used). Evaluated at `x_i` with
///    continuous-lookup semantics.
/// 2. A **numeric array** written as a comma-separated list (`0, 0.1, 0.3`).
///    Indexed `floor(x_i * m)`.
///
/// Anything else -- an empty distribution, an inline expression, or a name that
/// resolves to no graphical function -- is not representable and is rejected
/// loudly (a silent `Beginning` fallback would hide a real modeling error).
fn resolve_dist_profile(
    model: &datamodel::Model,
    spec: &str,
    flow: &str,
) -> Result<DistProfile, (ErrorCode, String)> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err((
            ErrorCode::ConveyorSpreadflowUnsupported,
            format!(
                "inflow '{flow}' uses isee:spreadflow 'dist' but has an empty <isee:distrib_eq>"
            ),
        ));
    }
    // 1. A named graphical-function variable.
    if let Some(gf) = model_variable_gf(model, spec) {
        match crate::variable::parse_table(&Some(gf.clone())) {
            Ok(Some(table)) if table.x.len() >= 2 && table.x.len() == table.y.len() => {
                let pairs = table
                    .x
                    .iter()
                    .copied()
                    .zip(table.y.iter().copied())
                    .collect();
                return Ok(DistProfile::Gf(pairs));
            }
            _ => {
                return Err((
                    ErrorCode::ConveyorSpreadflowUnsupported,
                    format!(
                        "inflow '{flow}' uses isee:spreadflow 'dist' naming '{spec}', which has no \
                         usable graphical function to use as a distribution"
                    ),
                ));
            }
        }
    }
    // 2. A literal comma-separated numeric array.
    if let Some(arr) = parse_dist_array(spec) {
        return Ok(DistProfile::Array(arr));
    }
    // 3. Neither: an inline expression or a dangling reference.
    Err((
        ErrorCode::ConveyorSpreadflowUnsupported,
        format!(
            "inflow '{flow}' uses isee:spreadflow 'dist' with distribution '{spec}', which is \
             neither a graphical-function variable nor a numeric array"
        ),
    ))
}

/// The graphical function of the model variable named `name` (canonical), if it
/// is an aux or flow carrying one. Stocks and modules carry no GF.
fn model_variable_gf<'a>(
    model: &'a datamodel::Model,
    name: &str,
) -> Option<&'a datamodel::GraphicalFunction> {
    let target = canon(name);
    model.variables.iter().find_map(|v| {
        let (ident, gf) = match v {
            datamodel::Variable::Aux(a) => (&a.ident, &a.gf),
            datamodel::Variable::Flow(f) => (&f.ident, &f.gf),
            _ => return None,
        };
        if canon(ident) == target {
            gf.as_ref()
        } else {
            None
        }
    })
}

/// Parse a `dist` numeric array (§8): a comma-separated list of at least two
/// finite numbers. A single bare token (no comma) is left for the GF-name /
/// loud-rejection path -- a lone number is ambiguous with a scalar and never a
/// meaningful distribution.
fn parse_dist_array(spec: &str) -> Option<Vec<f64>> {
    let parts: Vec<&str> = spec.split(',').collect();
    if parts.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        match p.trim().parse::<f64>() {
            Ok(v) if v.is_finite() => out.push(v),
            _ => return None,
        }
    }
    Some(out)
}

/// The leak-carrying flow named `flow` (canonical) in `model`, with its
/// leakage, if any; a conveyor outflow is a leak iff this returns `Some`.
///
/// Leak DETECTION and flow FETCH are deliberately one lookup: two variables can
/// share a canonical ident (the XMILE reader sorts by canonical ident but never
/// dedups, and canonicalization also collapses case/whitespace/underscores), and
/// only some of them may carry `<leak/>`. Detecting leak-ness with an any() scan
/// over every same-canonical flow but then fetching the FIRST canon-matching
/// flow panicked (unwrap on its absent leakage) when only a LATER twin carried
/// the marker (GH #870). Skipping leak-less twins here guarantees the returned
/// flow is the one whose `<leak/>` made the outflow a leak.
fn find_leak_flow<'a>(
    model: &'a datamodel::Model,
    flow: &str,
) -> Option<(&'a datamodel::Flow, &'a datamodel::Leakage)> {
    model.variables.iter().find_map(|v| match v {
        datamodel::Variable::Flow(f) if canon(&f.ident) == flow => {
            f.compat.leakage.as_ref().map(|lk| (f, lk))
        }
        _ => None,
    })
}

/// Parse a leak-zone bound (`leak_start`/`leak_end`) as a constant, clamped to
/// `[0, 1]`. A non-constant expression falls back to the default (zones are
/// attributes; the vendored fixtures use constants).
fn parse_zone(expr: &Option<String>, default: f64) -> f64 {
    match expr {
        Some(s) => s.trim().parse::<f64>().unwrap_or(default).clamp(0.0, 1.0),
        None => default,
    }
}

/// Evaluate a conveyor-block expression (`<len>`, a leak fraction) as a
/// compile-time scalar constant: a numeric literal, optionally under unary
/// sign(s) or parentheses. Anything else -- a variable reference, arithmetic,
/// a builtin call, a parse error -- returns `None`: its value is only known at
/// runtime, and the §4.1/§5.1 compile-time advisories must produce no false
/// positives for runtime expressions (docs/design/conveyors.md §4.1, §5.1).
/// Going through the real lexer/parser (rather than `str::parse::<f64>`) means
/// whitespace, comments, and sign forms are classified exactly like the
/// equation the runtime will evaluate.
pub(crate) fn const_scalar_expr(expr: &str) -> Option<f64> {
    use crate::ast::{Expr0, UnaryOp};
    fn eval(ast: &Expr0) -> Option<f64> {
        match ast {
            Expr0::Const(_, v, _) => Some(*v),
            Expr0::Op1(UnaryOp::Positive, inner, _) => eval(inner),
            Expr0::Op1(UnaryOp::Negative, inner, _) => eval(inner).map(|v| -v),
            _ => None,
        }
    }
    let ast = Expr0::new(expr, crate::lexer::LexerType::Equation).ok()??;
    eval(&ast)
}

/// The outcome of probing a conveyor stock's initial `<eqn>` for the §7.2
/// explicit comma-separated init-list form (docs/design/conveyors.md §7.2).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum InitListProbe {
    /// Not list-shaped: no top-level comma structure. A comma inside a call or
    /// subscript (`MAX(a, b)`, `arr[a, b]`) splits into fragments that do not
    /// parse on their own, which is exactly how they are told apart from a
    /// genuine list -- see [`probe_init_list`].
    NotAList,
    /// A well-formed §7.2 list: every entry a finite numeric constant, in
    /// list order (front / soonest-to-exit first). At least two entries, or
    /// one entry with a trailing comma ("5," -- the comma is what makes it a
    /// list).
    List(Vec<f64>),
    /// List-shaped (comma-separated entries, each a well-formed expression)
    /// but an entry is not a plain numeric literal. Carries the 0-based entry
    /// index and its text for the diagnostic.
    BadEntry(usize, String),
}

/// Probe an equation string for the §7.2 explicit init list: a comma-separated
/// list of numeric constants ("100, 200, 300"). Tolerates whitespace around
/// entries and a single trailing comma. Entries go through the real
/// lexer/parser (via [`const_scalar_expr`]), so sign forms and parenthesized
/// literals classify exactly like scalar equations do.
///
/// Disambiguation from ordinary equations: the grammar has no top-level comma,
/// so splitting a NON-list equation on `,` always breaks at least one fragment
/// into something that does not parse (`MAX(600, 300)` -> `MAX(600` / `300)`),
/// which returns `NotAList` and leaves the equation for normal compilation. A
/// split whose every fragment IS a well-formed expression can only have been a
/// list, so a non-constant fragment there is a malformed list entry
/// (`BadEntry`) to reject loudly -- the whole string cannot parse as a scalar
/// equation either, and the §7.2-specific diagnostic beats an opaque parse
/// error. Entries are constants only: the list is evaluated once at belt-init
/// time (before the first step), not compiled into the bytecode, and both the
/// spec's examples and isee's documentation ("a number of values") describe
/// numeric lists.
pub(crate) fn probe_init_list(eqn: &str) -> InitListProbe {
    let trimmed = eqn.trim();
    // Tolerate one trailing comma ("10, 20, 30,"). The trailing comma also
    // makes a SINGLE entry a list ("5," -> a one-entry list whose entry
    // repeats per time unit under §7.2 short-list normalization): the comma
    // is what distinguishes a list from a §7.1 scalar, and without this rule
    // "5," fell to an opaque parse error while "10, 20," was accepted.
    let had_trailing_comma = trimmed.ends_with(',');
    let trimmed = trimmed.strip_suffix(',').unwrap_or(trimmed);
    let parts: Vec<&str> = trimmed.split(',').collect();
    if parts.len() < 2 && !had_trailing_comma {
        return InitListProbe::NotAList;
    }
    let mut values = Vec::with_capacity(parts.len());
    let mut bad: Option<(usize, String)> = None;
    for (i, part) in parts.iter().enumerate() {
        let part = part.trim();
        if let Some(v) = const_scalar_expr(part) {
            values.push(v);
            continue;
        }
        match crate::ast::Expr0::new(part, crate::lexer::LexerType::Equation) {
            // A fragment that does not parse means the comma belonged to a
            // larger expression: not a list at all.
            Err(_) => return InitListProbe::NotAList,
            // Well-formed but non-constant (`some_var`), or empty ("1,,2"):
            // a malformed list entry. Keep scanning so a later unparseable
            // fragment can still veto list-shape.
            Ok(_) => {
                if bad.is_none() {
                    bad = Some((i, part.to_string()));
                }
            }
        }
    }
    match bad {
        Some((i, text)) => InitListProbe::BadEntry(i, text),
        None => InitListProbe::List(values),
    }
}

/// The parsed §7.2 explicit init list(s) of a conveyor stock's `<eqn>`,
/// resolved from the equation's array shape: which belts get a direct list
/// fill, and from which list.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum InitListSpec {
    /// One list shared by every element belt: a scalar stock's list, or an
    /// apply-to-all arrayed list (one equation for the whole array, so every
    /// belt fills identically).
    Shared(Vec<f64>),
    /// A non-apply-to-all arrayed stock: XMILE §4.5.2 lets each `<element>`
    /// carry its own equation, and a conveyor stock's equation is its
    /// initial -- so a list element equation initializes THAT element's belt.
    /// `elems` maps every element with an explicit `<element>` entry
    /// (canonical comma-joined subscript, [`canonical_subscript_key`]) to its
    /// list, `None` when that element's equation is an ordinary non-list
    /// initial (it keeps the §7.1 steady fill -- mixing is well-defined
    /// because each belt is independent). `default` is the EXCEPT-default
    /// equation's list when that default is itself a list; it applies to
    /// every element without an explicit entry.
    PerElement {
        elems: HashMap<String, Option<Vec<f64>>>,
        default: Option<Vec<f64>>,
    },
}

/// A resolved §7.2 explicit init list: the parsed list spec plus the constant
/// raw-sum placeholder equation the diagnostic path substitutes.
pub(crate) type InitListRewrite = (InitListSpec, Equation);

/// The scalar-string half of a single probed list: parsed values plus the
/// raw-sum placeholder text (shaped into an [`Equation`] by the caller).
type InitListValues = (Vec<f64>, String);

/// Canonical comma-joined subscript key for matching an `Equation::Arrayed`
/// element entry against the row-major [`element_subscripts_for_dims`]
/// suffixes. Each comma-separated part is canonicalized independently (the
/// XMILE reader already stores element keys this way -- `xmile/variables.rs`
/// `convert_equation` -- but MDL-sourced or hand-built datamodels may not),
/// so both sides normalize identically regardless of case or whitespace.
pub(crate) fn canonical_subscript_key(subscript: &str) -> String {
    subscript
        .split(',')
        .map(canon)
        .collect::<Vec<_>>()
        .join(",")
}

/// Resolve a conveyor stock's initial equation against the §7.2 explicit-list
/// form. Returns `Ok(Some((spec, placeholder)))` when the equation carries a
/// list anywhere: `spec` says which belts fill from which parsed list (front
/// first, for [`ConveyorState::init_explicit`]), and `placeholder` is the
/// equation with every list replaced by its constant RAW SUM, merely making
/// the stock's `<eqn>` parseable (a comma list is not a scalar expression).
/// The raw sum is fine for the parse-only salsa diagnostic path
/// (`db::input::datamodel_variable_from_source` -- that equation is never
/// simulated, and the LaTeX surface returns NULL for list stocks rather than
/// rendering it); [`expand_conveyors`] does NOT use this placeholder value --
/// it swaps in the NORMALIZED belt total from [`normalized_init_total`]
/// (per element for a non-apply-to-all array), because init-time consumers
/// evaluated before `init_belts` read the placeholder as the stock's initial
/// value. `Ok(None)` when the equation is an ordinary §7.1 initial (no list
/// in any element). `Err` when a list cannot be used as written (a
/// non-constant entry). Shared by [`expand_conveyors`] and the diagnostic
/// path so the editor's parse diagnostics accept exactly the lists the
/// runtime accepts.
pub(crate) fn explicit_init_list(
    stock: &str,
    equation: &Equation,
) -> Result<Option<InitListRewrite>, (ErrorCode, String)> {
    // Probe one scalar equation string under the display name `name` (the
    // bare stock, or `stock[element]` for a per-element equation): the
    // parsed values plus the raw-sum placeholder text, `None` for a
    // non-list, `Err` for a malformed list.
    let scalar = |name: &str, s: &str| -> Result<Option<InitListValues>, (ErrorCode, String)> {
        match probe_init_list(s) {
            InitListProbe::NotAList => Ok(None),
            InitListProbe::List(values) => {
                let sum: f64 = values.iter().sum();
                if !sum.is_finite() {
                    return Err((
                        ErrorCode::ConveyorInitListUnsupported,
                        format!(
                            "conveyor '{name}' has an explicit init list whose sum is not \
                             finite; the initial belt contents must be finite"
                        ),
                    ));
                }
                Ok(Some((values, format!("{sum}"))))
            }
            InitListProbe::BadEntry(i, text) => Err((
                ErrorCode::ConveyorInitListUnsupported,
                format!(
                    "conveyor '{name}' has an explicit init list whose entry {} ('{text}') is \
                     not a plain numeric literal; init-list entries must be plain numeric \
                     literals (they are evaluated once at belt initialization, \
                     docs/design/conveyors.md §7.2)",
                    i + 1
                ),
            )),
        }
    };
    match equation {
        Equation::Scalar(s) => {
            Ok(scalar(stock, s)?.map(|(v, sum)| (InitListSpec::Shared(v), Equation::Scalar(sum))))
        }
        Equation::ApplyToAll(dims, s) => Ok(scalar(stock, s)?.map(|(v, sum)| {
            (
                InitListSpec::Shared(v),
                Equation::ApplyToAll(dims.clone(), sum),
            )
        })),
        Equation::Arrayed(dims, elems, default, has_except_default) => {
            let mut any_list = false;
            let mut spec_elems: HashMap<String, Option<Vec<f64>>> = HashMap::new();
            let mut ph_elems = Vec::with_capacity(elems.len());
            for (subscript, eqn, initial_eqn, gf) in elems {
                let probed = scalar(&format!("{stock}[{subscript}]"), eqn)?;
                let (values, ph_eqn) = match probed {
                    Some((v, sum)) => {
                        any_list = true;
                        (Some(v), sum)
                    }
                    None => (None, eqn.clone()),
                };
                spec_elems.insert(canonical_subscript_key(subscript), values);
                ph_elems.push((subscript.clone(), ph_eqn, initial_eqn.clone(), gf.clone()));
            }
            let (spec_default, ph_default) = match default {
                Some(d) => match scalar(stock, d)? {
                    Some((v, sum)) => {
                        any_list = true;
                        (Some(v), Some(sum))
                    }
                    None => (None, Some(d.clone())),
                },
                None => (None, None),
            };
            if !any_list {
                return Ok(None);
            }
            Ok(Some((
                InitListSpec::PerElement {
                    elems: spec_elems,
                    default: spec_default,
                },
                Equation::Arrayed(dims.clone(), ph_elems, ph_default, *has_except_default),
            )))
        }
    }
}

/// Does this (conveyor-marked) stock equation carry a well-formed §7.2
/// explicit init list? `pub` for equation-RENDERING surfaces (libsimlin's
/// `simlin_model_get_latex_equation`): the salsa parse path substitutes a
/// constant placeholder for a list `<eqn>` (to keep diagnostics clean), so a
/// renderer working from the parsed AST would present the placeholder --
/// with source-range annotations mapped into the placeholder text -- as if
/// the user wrote it. Such surfaces must skip list stocks (no preview beats
/// a confidently wrong one). Malformed lists return `false`: their equation
/// is left un-substituted, so the ordinary parse failure already yields no
/// AST to render.
pub fn equation_has_explicit_init_list(equation: &Equation) -> bool {
    matches!(explicit_init_list("", equation), Ok(Some(_)))
}

/// The sim specs the ROOT model actually runs under: the main model's own
/// `sim_specs` override when present, else the project's. Mirrors the
/// runtime's root rule EXACTLY (`db::assemble::assemble_simulation` "Build
/// Specs, preferring model-level sim_specs override"; the §4.1 transit
/// advisory in `db::diagnostic` applies the same preference to whichever
/// model it is diagnosing, which coincides with this rule for the root
/// model), so every
/// compile-time consumer of dt / sim_method in the conveyor and queue build
/// paths -- the §7.2 list-normalization probe and the Euler-only gates --
/// reads the SAME specs the VM will execute with. Reading only
/// `project.sim_specs` here let a model-level override diverge: a dt
/// override skewed the normalized-total placeholder, and an RK4 override
/// evaded the Euler gate.
pub(crate) fn effective_sim_specs<'a>(
    project: &'a datamodel::Project,
    main_model: &str,
) -> &'a datamodel::SimSpecs {
    let main_canon = canon(main_model);
    project
        .models
        .iter()
        .find(|m| canon(&m.name) == main_canon)
        .and_then(|m| m.sim_specs.as_ref())
        .unwrap_or(&project.sim_specs)
}

/// The simulation `dt` as an f64, for expansion-time §7.2 normalization.
/// Mirrors `results::Specs::from`'s `Dt::Reciprocal` handling (1/v) so the
/// expansion and the runtime belt agree bit-for-bit on dt.
fn sim_specs_dt(specs: &datamodel::SimSpecs) -> Result<f64, (ErrorCode, String)> {
    let dt = match specs.dt {
        datamodel::Dt::Dt(v) => v,
        datamodel::Dt::Reciprocal(v) => 1.0 / v,
    };
    if dt.is_finite() && dt > 0.0 {
        Ok(dt)
    } else {
        Err((
            ErrorCode::BadSimSpecs,
            format!("dt must be positive and finite, got {dt}"),
        ))
    }
}

/// The §7.2 NORMALIZED initial belt total, computed at EXPANSION time so the
/// expanded stock's placeholder `<eqn>` carries the value the belt will
/// actually hold. This is what every init-time consumer evaluated BEFORE
/// `init_belts` -- a downstream conveyor's or queue's initial reading this
/// stock, a belt parameter (`<len>`/`<capacity>`/leak fraction) referencing
/// it, and every ordinary initial -- sees; a raw-list-sum placeholder leaked
/// the un-normalized value into all of them whenever a per-time-unit list was
/// truncated or extended (they run before the `init_belts` write-back and the
/// reconcile pass only corrects the stock slot itself and re-runs INITIALS,
/// not the already-filled belts/FIFOs).
///
/// Normalization is length-dependent (`values.len() == N` fills per slat; any
/// other length is per time unit, truncated / last-entry-extended -- and for
/// `dt > 1` a time-unit block can own no slat at all, dropping its entry), so
/// the total is NOT re-derived arithmetically here: a leak-less probe
/// [`ConveyorState`] runs the SAME `init_explicit` fill the runtime belt
/// uses, making expansion/runtime drift structurally impossible (slat
/// CONTENTS are independent of leak config at init; leaks only shape the
/// cohort schedules). This requires a compile-time-constant transit time --
/// a runtime `<len>` expression is rejected loudly, since without N neither
/// the list-length interpretation nor the initial total is decidable before
/// the run.
fn normalized_init_total(
    stock: &str,
    conv: &datamodel::Conveyor,
    values: &[f64],
    dt: f64,
) -> Result<f64, (ErrorCode, String)> {
    let Some(transit) = const_scalar_expr(&conv.transit_time) else {
        return Err((
            ErrorCode::ConveyorInitListUnsupported,
            format!(
                "conveyor '{stock}' uses an explicit init list, but its transit time (<len> is \
                 '{}') is not a plain numeric literal; a list-initialized conveyor's <len> must \
                 be a plain numeric literal, because the list-length interpretation (one entry \
                 per slat vs one per time unit) and the stock's initial total depend on the \
                 slat count (docs/design/conveyors.md §7.2)",
                conv.transit_time.trim()
            ),
        ));
    };
    if !transit.is_finite() || transit <= 0.0 {
        // Same code + message shape as the runtime latch in [`init_belts`].
        return Err((
            ErrorCode::ConveyorTransitNotPositive,
            format!("conveyor '{stock}' transit time must be positive and finite, got {transit}"),
        ));
    }
    // The probe belt allocates `slat_count` slats; enforce the same §4.1
    // bound the runtime latch enforces before allocating.
    check_slat_bound(stock, transit, dt)?;
    let mut probe = ConveyorState::new(
        dt,
        conv.exponential_leak,
        conv.discrete,
        conv.ignore_earlier_zone_losses,
        vec![],
    );
    probe.init_explicit(transit, values, &[]);
    let total = probe.contents();
    if !total.is_finite() {
        return Err((
            ErrorCode::ConveyorInitListUnsupported,
            format!(
                "conveyor '{stock}' has an explicit init list whose normalized total is not \
                 finite; the initial belt contents must be finite"
            ),
        ));
    }
    Ok(total)
}

/// §4.1 transit-quantization tolerance: `transit/dt` counts as an integer
/// multiple when within 1e-9 of the belt's rounded slat count -- the exact
/// threshold the spec states (`|T/DT − round(T/DT)| > 1e-9`,
/// docs/design/conveyors.md §4.1). An absolute tolerance on the RATIO keeps
/// binary-representation noise (e.g. `0.3 / 0.1 = 2.9999999999999996`, ~4e-16
/// off) from warning on a transit the modeler wrote as an exact multiple,
/// while any humanly-intended fraction of a slat (>= 1e-9 of dt) still warns.
pub(crate) const TRANSIT_RATIO_TOLERANCE: f64 = 1e-9;

/// §5.1 leak-fraction-sum tolerance: constant linear fractions warn only when
/// their sum exceeds `1 + 1e-9`, so a sum the modeler wrote as exactly 1
/// (legal: the primary outflow is then 0) never warns off f64 rounding.
pub(crate) const LEAK_FRACTION_SUM_TOLERANCE: f64 = 1e-9;

/// §4.1: the DT-quantized belt geometry for a compile-time-constant transit
/// time. Returns `Some((slats, effective_transit))` -- with `slats` computed
/// by the same [`crate::conveyor::slat_count`] the runtime belt uses
/// (round-half-away-from-zero, clamped to >= 1) and `effective_transit =
/// slats * dt` -- when `transit` is NOT an integer multiple of `dt` within
/// [`TRANSIT_RATIO_TOLERANCE`]. Returns `None` when quantization is exact, or
/// when either input is outside the positive finite domain: a non-positive or
/// non-finite transit is [`ErrorCode::ConveyorTransitNotPositive`]'s job at
/// the runtime latch, and an invalid dt is a sim-specs problem -- neither
/// should masquerade as a quantization advisory.
///
/// Deliberate spec-letter divergence: for `0 < transit/dt < 1e-9` the spec's
/// literal `|T/DT - round(T/DT)|` formula is silent (round gives 0, within
/// tolerance), but comparing against the CLAMPED slat count (1) warns with
/// effective transit = dt -- which is what the run actually does.
pub(crate) fn transit_dt_mismatch(transit: f64, dt: f64) -> Option<(usize, f64)> {
    if !(transit.is_finite() && transit > 0.0 && dt.is_finite() && dt > 0.0) {
        return None;
    }
    let slats = crate::conveyor::slat_count(transit, dt);
    let ratio = transit / dt;
    if (ratio - slats as f64).abs() > TRANSIT_RATIO_TOLERANCE {
        Some((slats, slats as f64 * dt))
    } else {
        None
    }
}

/// Expand every conveyor in `main_model` of `project` into hidden parameter
/// auxes plus placeholder-equation driven flows, returning the modified project
/// and one [`ConveyorMeta`] per conveyor. A project with no conveyors is
/// returned unchanged with an empty meta list (the caller can then skip all
/// conveyor machinery). Errors if a conveyor has no non-leak outflow.
pub fn expand_conveyors(
    project: &datamodel::Project,
    main_model: &str,
) -> Result<(datamodel::Project, Vec<ConveyorMeta>), (ErrorCode, String)> {
    let main_canon = canon(main_model);
    // Fast path: no conveyor anywhere in the main model.
    if !project_has_conveyor(project, main_model) {
        return Ok((project.clone(), Vec::new()));
    }

    let mut project = project.clone();
    let mut metas = Vec::new();
    // Collect new auxes to append after we finish borrowing the model's vars.
    let mut new_auxes: Vec<datamodel::Aux> = Vec::new();
    // Synthesized-aux canonical name -> (conveyor display name, human-readable
    // parameter label). Lets the driven-flow-read scan name the conveyor PARAMETER
    // an offending reference came from instead of the internal `$conv$...` aux
    // name, which is meaningless to a modeler.
    let mut param_origins: HashMap<String, (String, String)> = HashMap::new();

    // Find the model index.
    let model_idx = project
        .models
        .iter()
        .position(|m| canon(&m.name) == main_canon)
        .expect("main model present (checked above)");

    // Pass 1: snapshot which flows are leak flows and which stocks are conveyors
    // (immutable reads), then compute metadata + synthesized auxes.
    let model = &project.models[model_idx];
    // Map of conveyor stock -> its inflow set (for held-exit destination linkage).
    let mut conveyor_inflow_owner: HashMap<String, String> = HashMap::new();
    for v in &model.variables {
        if let datamodel::Variable::Stock(s) = v
            && s.compat.conveyor.is_some()
        {
            for inflow in &s.inflows {
                conveyor_inflow_owner.insert(canon(inflow), canon(&s.ident));
            }
        }
    }

    // Canonical conveyor-stock name -> its array-dimension count (0 = scalar).
    // Used by the container-access rewrite below to tell an ordinary array-element
    // read of an arrayed conveyor apart from a belt-slat read (§10).
    let mut conveyor_dims: HashMap<String, usize> = HashMap::new();
    // Canonical conveyor-stock name -> its declared dimensions (empty = scalar),
    // so a synthesized container variable can be arrayed over the same dims.
    let mut conveyor_stock_dims: HashMap<String, Vec<DimensionName>> = HashMap::new();

    // §7.2 explicit-list initials: canonical stock name -> the constant
    // NORMALIZED-total placeholder equation Pass 2 rewrites the stock with (a
    // comma list is not a scalar expression, so the expanded INTEG stock needs
    // a compilable <eqn>). The placeholder carries the normalized belt total
    // -- computed at expansion time by the same runtime fill init_belts runs
    // ([`normalized_init_total`]) -- so every init-time consumer evaluated
    // before init_belts (chained conveyor/queue initials, belt parameters,
    // ordinary initials) sees the correct value; init_belts' write-back is
    // pure defense in depth.
    let mut init_list_rewrites: HashMap<String, Equation> = HashMap::new();

    let mut driven_flows: Vec<String> = Vec::new();
    for v in &model.variables {
        let datamodel::Variable::Stock(stock) = v else {
            continue;
        };
        let Some(conv) = &stock.compat.conveyor else {
            continue;
        };
        let stock_name = canon(&stock.ident);

        // An arrayed conveyor is N_elem independent belts (§10). The stock's
        // dimensions drive the synthesized auxes' shape and the per-element
        // offset enumeration; empty for a scalar conveyor.
        let stock_dims = equation_dims(&stock.equation);
        conveyor_dims.insert(stock_name.clone(), stock_dims.len());
        conveyor_stock_dims.insert(stock_name.clone(), stock_dims.clone());
        let element_subscripts =
            element_subscripts_for_dims(&project, &stock_dims, &stock.ident, "conveyor")?;

        // §7.2 explicit init list(s) on the stock's <eqn>: record the parsed
        // values for belt init and queue the placeholder rewrite for Pass 2.
        // A malformed list (non-constant entry, non-constant transit) is
        // rejected loudly here rather than surfacing as an opaque parse
        // error. The placeholder value is the NORMALIZED belt total, not the
        // raw list sum -- see `normalized_init_total` for why (the raw sum
        // leaked into every pre-init_belts consumer); for a non-apply-to-all
        // array each list element carries ITS OWN normalized total.
        let init_values: Vec<Option<Vec<f64>>> =
            match explicit_init_list(&stock.ident, &stock.equation)? {
                Some((spec, _raw_sum_placeholder)) => {
                    let dt = sim_specs_dt(effective_sim_specs(&project, main_model))?;
                    let (placeholder, per_belt) = match spec {
                        InitListSpec::Shared(values) => {
                            let total = normalized_init_total(&stock.ident, conv, &values, dt)?;
                            let placeholder = match &stock.equation {
                                Equation::ApplyToAll(dims, _) => {
                                    Equation::ApplyToAll(dims.clone(), format!("{total}"))
                                }
                                _ => Equation::Scalar(format!("{total}")),
                            };
                            let per_belt = vec![Some(values); n_elements(&element_subscripts)];
                            (placeholder, per_belt)
                        }
                        InitListSpec::PerElement { elems, default } => {
                            let Equation::Arrayed(dims, elem_eqns, default_eqn, has_except) =
                                &stock.equation
                            else {
                                unreachable!("PerElement spec only comes from Arrayed equations");
                            };
                            // Rebuild the Arrayed equation with each LIST
                            // element's eqn replaced by its own normalized
                            // total; non-list element equations compile
                            // untouched (their §7.1 initial).
                            let mut ph_elems = Vec::with_capacity(elem_eqns.len());
                            for (subscript, eqn, initial_eqn, gf) in elem_eqns {
                                let new_eqn = match elems.get(&canonical_subscript_key(subscript)) {
                                    Some(Some(values)) => {
                                        let display = format!("{}[{}]", stock.ident, subscript);
                                        normalized_init_total(&display, conv, values, dt)?
                                            .to_string()
                                    }
                                    _ => eqn.clone(),
                                };
                                ph_elems.push((
                                    subscript.clone(),
                                    new_eqn,
                                    initial_eqn.clone(),
                                    gf.clone(),
                                ));
                            }
                            let ph_default = match (&default, default_eqn) {
                                (Some(values), Some(_)) => Some(
                                    normalized_init_total(&stock.ident, conv, values, dt)?
                                        .to_string(),
                                ),
                                _ => default_eqn.clone(),
                            };
                            let placeholder =
                                Equation::Arrayed(dims.clone(), ph_elems, ph_default, *has_except);
                            // One list per belt, matched by canonical
                            // subscript; an element without an explicit
                            // <element> entry falls back to the EXCEPT
                            // default's list (None when the default is not a
                            // list -- that element keeps its §7.1 fill).
                            let per_belt = element_subscripts
                                .iter()
                                .map(|sub| match elems.get(&canonical_subscript_key(sub)) {
                                    Some(v) => v.clone(),
                                    None => default.clone(),
                                })
                                .collect();
                            (placeholder, per_belt)
                        }
                    };
                    init_list_rewrites.insert(stock_name.clone(), placeholder);
                    per_belt
                }
                None => Vec::new(),
            };

        // Partition outflows into the primary (first non-leak) and leaks.
        // Any non-leak outflow beyond the first is an error (see the rejection
        // after the loop): the slat model has exactly one primary outflow.
        let mut primary: Option<String> = None;
        let mut primary_raw: Option<String> = None;
        let mut extra_outflows_raw: Vec<String> = Vec::new();
        let mut leak_metas = Vec::new();
        for out in &stock.outflows {
            let out_c = canon(out);
            if let Some((leak_flow, lk)) = find_leak_flow(model, &out_c) {
                let (zone_start, zone_end, integers, frac_eqn) = (
                    parse_zone(&lk.zone_start, 0.0),
                    parse_zone(&lk.zone_end, 1.0),
                    lk.integers,
                    leak_fraction_equation(leak_flow, &stock_dims),
                );
                let frac_aux = leak_frac_name(&out_c);
                new_auxes.push(make_aux_eqn(&frac_aux, frac_eqn));
                param_origins.insert(
                    frac_aux.clone(),
                    (stock.ident.clone(), format!("leak fraction for '{out}'")),
                );
                driven_flows.push(out_c.clone());
                // Which conveyor (if any) does this leak flow feed? Resolved the
                // same way as the primary's held-exit destination (below): a leak
                // that is a downstream conveyor's inflow links to it, with the
                // `owner != stock_name` self-loop filter mirrored deliberately.
                let dest_conveyor = conveyor_inflow_owner
                    .get(&out_c)
                    .filter(|owner| **owner != stock_name)
                    .cloned();
                leak_metas.push(LeakMeta {
                    flow: out_c,
                    frac_aux,
                    zone_start,
                    zone_end,
                    integers,
                    dest_conveyor,
                });
            } else if primary.is_none() {
                primary = Some(out_c);
                primary_raw = Some(out.clone());
            } else {
                // A second (or later) non-leak outflow. The conveyor slat model
                // has exactly one primary (belt-end) outflow plus leak flows;
                // an extra plain outflow has no slat-model meaning (§3.3).
                // Collect it (declaration order is deterministic) and reject
                // loudly after the loop, listing every extra. Leaving it as an
                // ordinary equation-driven outflow would drain the expanded
                // INTEG stock while the belt side table never sheds the
                // material, diverging the reported stock from the belt total
                // with no diagnostic.
                extra_outflows_raw.push(out.clone());
            }
        }
        let Some(primary_out) = primary else {
            return Err((
                ErrorCode::ConveyorWithoutOutflow,
                format!(
                    "conveyor '{}' has no non-leak outflow; a conveyor needs one primary outflow",
                    stock.ident
                ),
            ));
        };
        if !extra_outflows_raw.is_empty() {
            // `primary_raw` is Some whenever `primary` is (set together above).
            let primary_name = primary_raw.as_deref().unwrap_or(primary_out.as_str());
            let extras = extra_outflows_raw
                .iter()
                .map(|f| format!("'{f}'"))
                .collect::<Vec<_>>()
                .join(", ");
            let (verb, noun) = if extra_outflows_raw.len() == 1 {
                ("is", "outflow")
            } else {
                ("are", "outflows")
            };
            return Err((
                ErrorCode::ConveyorMultipleNonLeakOutflows,
                format!(
                    "conveyor '{}' has more than one non-leak outflow: '{primary_name}' is the \
                     primary (belt-end) outflow, but {extras} {verb} also a plain {noun}. A \
                     conveyor has exactly one primary outflow plus leak flows; mark the extra \
                     {noun} with <leak/> if leakage was intended",
                    stock.ident
                ),
            ));
        }
        driven_flows.push(primary_out.clone());

        // Synthesize the parameter auxes, arrayed over the stock's dimensions
        // for an arrayed conveyor so each element gets its own len/cap/... slot
        // (§10); scalar for a scalar conveyor.
        let len_aux = param_aux_name(&stock_name, "len");
        new_auxes.push(make_aux_eqn(
            &len_aux,
            param_equation(&conv.transit_time, &stock_dims),
        ));
        param_origins.insert(len_aux.clone(), (stock.ident.clone(), "<len>".to_string()));
        let mk = |field: &Option<String>,
                  param: &str,
                  out: &mut Vec<datamodel::Aux>|
         -> Option<String> {
            field.as_ref().map(|expr| {
                let name = param_aux_name(&stock_name, param);
                out.push(make_aux_eqn(&name, param_equation(expr, &stock_dims)));
                name
            })
        };
        let cap_aux = mk(&conv.capacity, "cap", &mut new_auxes);
        let inlim_aux = mk(&conv.inflow_limit, "inlim", &mut new_auxes);
        let sample_aux = mk(&conv.sample, "sample", &mut new_auxes);
        let arrest_aux = mk(&conv.arrest, "arrest", &mut new_auxes);
        for (aux, label) in [
            (&cap_aux, "<capacity>"),
            (&inlim_aux, "<in_limit>"),
            (&sample_aux, "<sample>"),
            (&arrest_aux, "<arrest>"),
        ] {
            if let Some(name) = aux {
                param_origins.insert(name.clone(), (stock.ident.clone(), label.to_string()));
            }
        }

        let inflows = stock
            .inflows
            .iter()
            .map(|inf| {
                let inf_c = canon(inf);
                let resolved = resolve_placement(model, &inf_c)?;
                // Conveyor-driven iff this inflow is a driven outflow of a
                // conveyor (resolved after we know all driven flows).
                Ok(InflowMeta {
                    flow: inf_c,
                    conveyor_driven: false, // filled below
                    placement: resolved.placement,
                    dist: resolved.dist,
                    source: resolved.source,
                })
            })
            .collect::<Result<Vec<_>, (ErrorCode, String)>>()?;

        // Held-exit destination: which conveyor (if any) does the primary
        // outflow feed?
        let primary_dest_conveyor = conveyor_inflow_owner
            .get(&primary_out)
            .filter(|owner| **owner != stock_name)
            .cloned();

        metas.push(ConveyorMeta {
            stock: stock_name,
            len_aux,
            cap_aux,
            inlim_aux,
            sample_aux,
            arrest_aux,
            discrete: conv.discrete,
            exponential_leak: conv.exponential_leak,
            ignore_earlier_zone_losses: conv.ignore_earlier_zone_losses,
            primary_out,
            leaks: leak_metas,
            inflows,
            containers: Vec::new(), // filled by the container-access rewrite below
            primary_dest_conveyor,
            element_subscripts,
            init_values,
        });
    }

    // Now that every driven flow is known, mark conveyor-driven inflows.
    let driven_set: std::collections::HashSet<String> = driven_flows.iter().cloned().collect();
    for meta in &mut metas {
        for inflow in &mut meta.inflows {
            inflow.conveyor_driven = driven_set.contains(&inflow.flow);
        }
    }
    // A sorted view of the driven flows: the reference scans below return on the
    // first match, so iterating a sorted list keeps the diagnostic (which flow is
    // named when an equation reads more than one) deterministic across runs.
    let mut driven_sorted: Vec<String> = driven_flows;
    driven_sorted.sort_unstable();
    driven_sorted.dedup();

    // Reject any equation that reads a conveyor-driven flow by name: the pass
    // runs after the flows phase, so a reader would see the pre-pass placeholder
    // 0 instead of the belt-driven rate. Loud error, never silent (§4.3).
    // Structural inflow linkage (a driven outflow feeding a downstream conveyor)
    // is handled by the pass and does NOT go through an equation reference, so
    // it is not caught here.
    if let Some((var, driven)) =
        find_driven_flow_read(&project.models[model_idx], &driven_set, &driven_sorted)
    {
        return Err((
            ErrorCode::ConveyorDrivenFlowRead,
            format!(
                "variable '{var}' references conveyor-driven flow '{driven}'; a \
                 conveyor outflow/leak cannot be read by another equation \
                 (it is computed after the flows phase)"
            ),
        ));
    }

    // The conveyor's OWN parameter (`<len>`/`<capacity>`/`<in_limit>`/`<sample>`/
    // `<arrest>`) and leak-fraction expressions were lifted into `$conv$...` auxes
    // during Pass 1 and are not appended to `model.variables` until Pass 2, so the
    // loop above did not scan them. They are ordinary auxes computed IN the Flows
    // phase, so a reference to a conveyor-driven flow reads the same placeholder 0
    // -- silently zeroing capacity, never arresting the belt, sampling a condition
    // a step early, and so on. Scan them here too, naming the conveyor PARAMETER
    // the reference came from (the synthetic `$conv$...` aux name is internal).
    //
    // Uniform treatment against the full driven set is correct even for a
    // bare-`<leak/>` fraction aux derived from the leak flow's OWN `<eqn>` that
    // references that same leak flow: the aux still reads the flow's placeholder-0
    // slot in the Flows phase, so rejecting it is exactly right (there is no
    // legitimate self-reference to carve out). This runs before the container-
    // access rewrite below mutates the aux equations, so it sees the raw parameter
    // strings; a container access like `SUM(belt)` names the belt STOCK, never a
    // driven flow, so it is not a false positive here.
    for aux in &new_auxes {
        if let Some(driven) =
            first_driven_flow_referenced(&equation_strings(&aux.equation), &driven_sorted)
        {
            let (conveyor, label) = param_origins
                .get(&aux.ident)
                .cloned()
                .unwrap_or_else(|| (aux.ident.clone(), "parameter".to_string()));
            return Err((
                ErrorCode::ConveyorDrivenFlowRead,
                format!(
                    "conveyor '{conveyor}' {label} references conveyor-driven flow \
                     '{driven}'; a conveyor outflow/leak cannot be read by a conveyor \
                     parameter (it is computed after the flows phase)"
                ),
            ));
        }
    }

    // Rewrite each equation that uses a conveyor as a CONTAINER -- indexing into
    // its belt (`conv[j]`, constant `j`) or reducing over its slat contents
    // (`SUM`/`SIZE`/`MEAN`/`MIN`/`MAX`/`STDDEV` of a single belt). XMILE §3.7.1
    // makes conveyors containers. The belt lives in the VM's conveyor side table
    // with a runtime-dynamic length, not in the fixed-dimension data buffer the
    // bytecode VM reads, so we cannot expose the slat vector to the bytecode.
    // Instead the conveyor pass computes each container-access RESULT natively
    // and PUBLISHES it into a synthesized hidden variable's slot at step-start
    // (§10); here we replace each supported container subexpression with a
    // reference to that variable. Genuinely-unlowerable forms (a reducer over an
    // EXPRESSION involving the belt, a dynamic/non-constant slat index, ranges or
    // wildcards over slats) stay loud-rejected via
    // `ConveyorContainerAccessUnsupported`.
    //
    // The synthesized variable is a STOCK (a no-flow INTEG): the Flows phase
    // reads but never recomputes a stock slot, and the Stocks phase integrates it
    // unchanged, so the pass's step-start publish survives the whole step and any
    // Flows-phase reader sees the start-of-step belt value. For an arrayed
    // conveyor the container variable is arrayed over the same dims, so element
    // `e` reads belt `e`.
    let mut container_specs: std::collections::BTreeMap<String, ContainerVarSpec> =
        std::collections::BTreeMap::new();
    let mut rewritten_equations: HashMap<String, Equation> = rewrite_model_container_equations(
        &project.models[model_idx],
        &driven_set,
        &conveyor_dims,
        &ContainerNaming::CONVEYOR,
        &mut container_specs,
    )?;

    // The conveyor's OWN parameter (`<len>`/`<capacity>`/`<in_limit>`/`<sample>`/
    // `<arrest>`) and leak-fraction expressions were synthesized into `$conv$...`
    // auxes from their RAW strings during Pass 1, so the reader loop above did not
    // scan them (they are not yet in `model.variables`). Apply the SAME container
    // rewrite to them now that `conveyor_dims` is complete -- otherwise a
    // belt-reading builtin in, say, `<capacity>SIZE(belt)</capacity>` would bind
    // to the plain scalar stock and silently mis-compute (SIZE of a scalar is 1),
    // violating this module's "supported -> rewrite, else loud-reject; never
    // silently wrong" contract. A SUPPORTED form here becomes a lagged
    // (start-of-step / previous-step-length) dependency on the container stock,
    // which is consistent with the step-start publish; a residual form gets the
    // same `ConveyorContainerAccessUnsupported` rejection as an ordinary equation.
    for aux in &mut new_auxes {
        if let Some(new_eqn) = rewrite_container_equation(
            &aux.equation,
            &conveyor_dims,
            &ContainerNaming::CONVEYOR,
            &mut container_specs,
        )? {
            aux.equation = new_eqn;
        }
    }

    // Attach each container variable to its conveyor's meta and synthesize the
    // hidden container stock (arrayed over the conveyor's dims when arrayed).
    let container_stocks =
        synthesize_container_stocks(&container_specs, &conveyor_stock_dims, |owner, cm| {
            if let Some(meta) = metas.iter_mut().find(|m| m.stock == owner) {
                meta.containers.push(cm);
            }
        });

    // Pass 2 (mutable): give every driven flow a `0` placeholder equation so it
    // compiles to a writable slot, append the synthesized auxes, and clear the
    // conveyor/leakage markers so the expanded model compiles as a plain
    // stock-and-flow model. Clearing the markers is what lets the ordinary
    // compile path reject an UN-expanded conveyor (the marker is still set)
    // while accepting this expanded one.
    let model = &mut project.models[model_idx];
    for v in &mut model.variables {
        // Replace a reader's equation with its container-access-rewritten form
        // (the container subexpressions now reference the synthesized stocks).
        if let Some(new_eqn) = rewritten_equations.remove(&canon(v.get_ident())) {
            set_variable_equation(v, new_eqn);
        }
        match v {
            datamodel::Variable::Flow(f) if driven_set.contains(&canon(&f.ident)) => {
                // Preserve the flow's array shape so an arrayed driven flow keeps
                // its per-element slots (§10); the pass overwrites every slot.
                f.equation = placeholder_zero_equation(&f.equation);
                // The fraction now lives in a hidden aux; the flow slot is
                // pass-driven, so its own leak/gf metadata plays no runtime role.
                f.gf = None;
                // Clear the leak marker so the expanded flow is plain.
                f.compat.leakage = None;
            }
            datamodel::Variable::Stock(s) if s.compat.conveyor.is_some() => {
                // A §7.2 explicit-list <eqn> compiles as its constant
                // normalized-total placeholder (recorded in Pass 1); the belt
                // itself fills from the meta's `init_values` in init_belts,
                // whose write-back of the identical total is defense in depth.
                if let Some(placeholder) = init_list_rewrites.remove(&canon(&s.ident)) {
                    s.equation = placeholder;
                }
                // The belt is now driven by the pass; the expanded stock is an
                // ordinary INTEG whose Δ = admitted - out - leak (the §4.3
                // conservation identity), so drop the conveyor marker.
                s.compat.conveyor = None;
            }
            _ => {}
        }
    }
    for aux in new_auxes {
        model.variables.push(datamodel::Variable::Aux(aux));
    }
    // Append the synthesized container stocks (no-flow INTEGs the pass drives).
    for stock in container_stocks {
        model.variables.push(datamodel::Variable::Stock(stock));
    }

    Ok((project, metas))
}

/// The scalar equation strings of a variable (one for a `Scalar`, each element
/// plus the default for an `Arrayed`), used to scan for references. An `Aux`,
/// `Flow`, or `Stock` all carry an `equation`; a `Module` carries none.
fn equation_scalar_strings(v: &datamodel::Variable) -> Vec<String> {
    match v.get_equation() {
        Some(eqn) => equation_strings(eqn),
        None => Vec::new(),
    }
}

/// The scalar equation strings of one [`Equation`] (the single expression of a
/// `Scalar`/`ApplyToAll`, or every element plus the default of an `Arrayed`).
/// Shared by [`equation_scalar_strings`] and the synthesized-aux driven-flow scan
/// (whose auxes are bare `Aux` values, not `datamodel::Variable`s).
fn equation_strings(eqn: &Equation) -> Vec<String> {
    match eqn {
        Equation::Scalar(s) => vec![s.clone()],
        Equation::ApplyToAll(_, s) => vec![s.clone()],
        Equation::Arrayed(_, elems, default, _) => {
            let mut out: Vec<String> = elems.iter().map(|(_, s, _, _)| s.clone()).collect();
            if let Some(d) = default {
                out.push(d.clone());
            }
            out
        }
    }
}

/// Scan `equations` (the scalar equation strings of one variable/aux) for a
/// reference to any pass-driven flow in `driven_sorted` (sorted for a
/// deterministic first-match), returning the first driven-flow name referenced.
/// A scalar string that does not parse is skipped -- an unparseable equation is a
/// separate diagnostic the ordinary compile path raises; the point here is purely
/// to catch a syntactically-valid reference to a pass-driven flow, which would
/// read the flow's Flows-phase placeholder 0 instead of the belt-driven rate
/// (docs/design/conveyors.md §4.3).
fn first_driven_flow_referenced(equations: &[String], driven_sorted: &[String]) -> Option<String> {
    for eqn in equations {
        let Ok(Some(ast)) = crate::ast::Expr0::new(eqn, crate::lexer::LexerType::Equation) else {
            continue;
        };
        for driven in driven_sorted {
            if ast.get_var_loc(driven).is_some() {
                return Some(driven.clone());
            }
        }
    }
    None
}

/// The first variable in `model` whose equation references a pass-driven flow:
/// `Some((variable display ident, driven flow name))`, or `None` when no
/// equation reads one. A pass-driven flow's own placeholder equation is not a
/// reader, so members of `driven_set` are skipped; `driven_sorted` (the same
/// names, sorted) keeps the reported flow deterministic when an equation reads
/// more than one. Shared by the conveyor and queue driven-flow-read rejection
/// scans ([`expand_conveyors`] / [`crate::queue_compile::expand_queues`]),
/// which differ only in ErrorCode and message wording: both passes run after
/// the flows phase, so a reader would see the pre-pass placeholder 0 instead
/// of the pass-computed rate (docs/design/conveyors.md §4.3, queues.md §2).
pub(crate) fn find_driven_flow_read<'m>(
    model: &'m datamodel::Model,
    driven_set: &std::collections::HashSet<String>,
    driven_sorted: &[String],
) -> Option<(&'m str, String)> {
    for v in &model.variables {
        if driven_set.contains(&canon(v.get_ident())) {
            continue;
        }
        if let Some(flow) = first_driven_flow_referenced(&equation_scalar_strings(v), driven_sorted)
        {
            return Some((v.get_ident(), flow));
        }
    }
    None
}

/// The container (conveyor or queue) and access a supported container
/// subexpression resolves to, keyed (by the caller) on the synthesized container
/// variable's canonical name. Shared by both owners (§8).
pub(crate) struct ContainerVarSpec {
    /// Canonical name of the conveyor/queue stock whose contents this reads.
    pub owner_stock: String,
    pub kind: ContainerKind,
}

/// Build the hidden container STOCK for a synthesized container variable (§10):
/// a no-flow INTEG initialized to `0`, arrayed over the conveyor's/queue's dims
/// when arrayed. The conveyor/queue pass overwrites its slot at step-start every
/// step (including t=0), so the `0` initial value is only a placeholder; because
/// it has no in/outflows the Stocks phase integrates it unchanged, letting the
/// published value survive the Flows phase. Shared by both owners (§8).
pub(crate) fn make_container_stock(ident: &str, dims: &[DimensionName]) -> datamodel::Stock {
    datamodel::Stock {
        ident: ident.to_string(),
        equation: param_equation("0", dims),
        documentation: String::new(),
        units: None,
        inflows: Vec::new(),
        outflows: Vec::new(),
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    }
}

/// Synthesize the hidden container stock for each registered container
/// variable (arrayed over its owner's dims when arrayed, per `stock_dims`) and
/// hand the owner's [`ContainerMeta`] to `attach`, which locates the owner's
/// meta (a [`ConveyorMeta`] or a queue meta) and records it. Iterating the
/// `BTreeMap` keeps synthesis order deterministic. Shared by
/// [`expand_conveyors`] and [`crate::queue_compile::expand_queues`] (§10 /
/// queues.md §8: the container machinery is identical; only the owning meta
/// type differs, which is why attachment is a callback).
pub(crate) fn synthesize_container_stocks(
    container_specs: &std::collections::BTreeMap<String, ContainerVarSpec>,
    stock_dims: &HashMap<String, Vec<DimensionName>>,
    mut attach: impl FnMut(&str, ContainerMeta),
) -> Vec<datamodel::Stock> {
    let mut container_stocks = Vec::with_capacity(container_specs.len());
    for (name, spec) in container_specs {
        let dims = stock_dims
            .get(&spec.owner_stock)
            .cloned()
            .unwrap_or_default();
        container_stocks.push(make_container_stock(name, &dims));
        attach(
            &spec.owner_stock,
            ContainerMeta {
                name: name.clone(),
                kind: spec.kind.clone(),
            },
        );
    }
    container_stocks
}

/// Overwrite a variable's equation in place (a `Module` carries no equation
/// and is left untouched). Shared by both expansions' mutable Pass 2 when
/// applying the container-access-rewritten equations.
pub(crate) fn set_variable_equation(v: &mut datamodel::Variable, eqn: Equation) {
    match v {
        datamodel::Variable::Stock(s) => s.equation = eqn,
        datamodel::Variable::Flow(f) => f.equation = eqn,
        datamodel::Variable::Aux(a) => a.equation = eqn,
        datamodel::Variable::Module(_) => {}
    }
}

/// A single-belt reference `expr` selects: the conveyor's canonical name and the
/// element-selecting index expressions (empty for a scalar conveyor). Returns
/// `Some` iff `expr` is EXACTLY a bare scalar conveyor `Var` or an arrayed
/// conveyor `Subscript` whose index count equals its array-dimension count with
/// every index a concrete single-element selector (no wildcard/range/star). An
/// expression that merely CONTAINS a belt reference (e.g. `belt / 2`) returns
/// `None` -- the caller routes that to loud rejection.
fn direct_belt_reference(
    expr: &crate::ast::Expr0,
    conveyor_dims: &HashMap<String, usize>,
) -> Option<(String, Vec<crate::ast::IndexExpr0>)> {
    use crate::ast::Expr0;
    match expr {
        Expr0::Var(raw, _) => {
            let name = canon(raw.as_str());
            match conveyor_dims.get(&name) {
                Some(0) => Some((name, Vec::new())),
                _ => None, // arrayed bare Var = per-element totals, not one belt
            }
        }
        Expr0::Subscript(raw, indices, _) => {
            let name = canon(raw.as_str());
            match conveyor_dims.get(&name) {
                Some(&ndims)
                    if ndims > 0 && indices.len() == ndims && all_single_element(indices) =>
                {
                    Some((name, indices.clone()))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Are all `indices` concrete single-element selectors (`Expr`/`DimPosition`,
/// no `Wildcard`/`StarRange`/`Range`)?
fn all_single_element(indices: &[crate::ast::IndexExpr0]) -> bool {
    use crate::ast::IndexExpr0;
    indices
        .iter()
        .all(|i| matches!(i, IndexExpr0::Expr(_) | IndexExpr0::DimPosition(_, _)))
}

/// A compile-time-constant, non-negative, integral slat index `j` (1-based from
/// the exit, §10), or `None` if the index is not a bare literal integer (a
/// dynamic/non-constant or fractional slat index is unsupported).
fn const_slat_index(idx: &crate::ast::IndexExpr0) -> Option<usize> {
    use crate::ast::{Expr0, IndexExpr0};
    let IndexExpr0::Expr(Expr0::Const(_, val, _)) = idx else {
        return None;
    };
    if val.is_finite() && *val >= 0.0 && val.fract() == 0.0 {
        Some(*val as usize)
    } else {
        None
    }
}

/// The reducer kind for a container reducer builtin name, or `None` if it is not
/// one. Names are lowercase (the parser lowercases call names).
fn reduce_kind(name: &str) -> Option<ContainerKind> {
    Some(match name {
        "sum" => ContainerKind::Sum,
        "mean" => ContainerKind::Mean,
        "size" => ContainerKind::Size,
        "min" => ContainerKind::Min,
        "max" => ContainerKind::Max,
        "stddev" => ContainerKind::Stddev,
        _ => return None,
    })
}

/// The first conveyor **belt reference** in `expr`'s subtree, if any: a scalar
/// conveyor (any reference to it -- a scalar has no array dimension, so reducing
/// an expression that involves it reduces a scalar) or a single-belt-selecting
/// subscript of an arrayed conveyor (`conv[elem]`). A bare arrayed-conveyor
/// `Var` (its per-element totals array) and a whole-array/slice subscript
/// (`conv[*]`, `conv[a,*]`) are deliberately NOT belt references -- they remain
/// ordinary arrays to reduce over.
fn subtree_has_belt_reference(
    expr: &crate::ast::Expr0,
    conveyor_dims: &HashMap<String, usize>,
) -> Option<String> {
    use crate::ast::{Expr0, IndexExpr0};
    use crate::builtins::UntypedBuiltinFn;
    match expr {
        Expr0::Const(..) => None,
        Expr0::Var(raw, _) => {
            let name = canon(raw.as_str());
            match conveyor_dims.get(&name) {
                Some(0) => Some(name), // scalar conveyor = belt reference
                _ => None,             // arrayed conveyor's per-element totals: not a belt ref
            }
        }
        Expr0::Subscript(raw, indices, _) => {
            let name = canon(raw.as_str());
            if let Some(&ndims) = conveyor_dims.get(&name)
                && (ndims == 0 || subscript_selects_single_belt(indices, ndims))
            {
                return Some(name);
            }
            for idx in indices {
                let found = match idx {
                    IndexExpr0::Range(l, r, _) => subtree_has_belt_reference(l, conveyor_dims)
                        .or_else(|| subtree_has_belt_reference(r, conveyor_dims)),
                    IndexExpr0::Expr(e) => subtree_has_belt_reference(e, conveyor_dims),
                    _ => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        Expr0::App(UntypedBuiltinFn(_, args), _) => args
            .iter()
            .find_map(|a| subtree_has_belt_reference(a, conveyor_dims)),
        Expr0::Op1(_, inner, _) => subtree_has_belt_reference(inner, conveyor_dims),
        Expr0::Op2(_, l, r, _) => subtree_has_belt_reference(l, conveyor_dims)
            .or_else(|| subtree_has_belt_reference(r, conveyor_dims)),
        Expr0::If(c, t, f, _) => subtree_has_belt_reference(c, conveyor_dims)
            .or_else(|| subtree_has_belt_reference(t, conveyor_dims))
            .or_else(|| subtree_has_belt_reference(f, conveyor_dims)),
    }
}

/// Does an arrayed-conveyor subscript with `ndims` array dimensions select a
/// SINGLE belt (so a reducer over it reads that belt's slats -- container
/// access), rather than a slice over several belts (an ordinary array
/// reduction)? True when it over-subscripts (`indices.len() > ndims`, indexing
/// into the belt) or fully indexes every array dimension with a concrete,
/// single-element index (no `Wildcard`/`StarRange`/`Range`).
fn subscript_selects_single_belt(indices: &[crate::ast::IndexExpr0], ndims: usize) -> bool {
    use crate::ast::IndexExpr0;
    if indices.len() > ndims {
        return true;
    }
    if indices.len() != ndims {
        return false; // a partial slice leaves an array to reduce over
    }
    indices
        .iter()
        .all(|i| matches!(i, IndexExpr0::Expr(_) | IndexExpr0::DimPosition(_, _)))
}

/// The loud-rejection error for an unsupported container-access form (§10). The
/// `naming` owner supplies the noun (`conveyor`/`queue`) and supported-forms help
/// so the message reads correctly for both; the CODE is shared.
fn unsupported_container(naming: &ContainerNaming, owner: &str, form: &str) -> (ErrorCode, String) {
    let noun = naming.noun;
    let help = naming.supported_help;
    (
        ErrorCode::ConveyorContainerAccessUnsupported,
        format!(
            "{noun} '{owner}' is used as a container in an unsupported form ({form}); \
             supported container access is SUM/MEAN/SIZE/MIN/MAX/STDDEV {help}"
        ),
    )
}

/// Rewrite each supported container-access subexpression in a variable's
/// equation to reference the synthesized container variable (§10), registering
/// the variable in `specs` (keyed by its canonical name). Returns `Some(new
/// equation)` if any component changed, `Ok(None)` if none did, or `Err` for an
/// unsupported residual. A parse failure leaves the component unchanged (an
/// invalid equation is surfaced later by ordinary compilation).
pub(crate) fn rewrite_container_equation(
    equation: &Equation,
    conveyor_dims: &HashMap<String, usize>,
    naming: &ContainerNaming,
    specs: &mut std::collections::BTreeMap<String, ContainerVarSpec>,
) -> Result<Option<Equation>, (ErrorCode, String)> {
    let rewrite_str = |s: &str,
                       specs: &mut std::collections::BTreeMap<String, ContainerVarSpec>|
     -> Result<Option<String>, (ErrorCode, String)> {
        let Ok(Some(ast)) = crate::ast::Expr0::new(s, crate::lexer::LexerType::Equation) else {
            return Ok(None);
        };
        let mut changed = false;
        let new_ast = rewrite_container_in_expr(&ast, conveyor_dims, naming, specs, &mut changed)?;
        if changed {
            Ok(Some(crate::ast::print_eqn(&new_ast)))
        } else {
            Ok(None)
        }
    };

    match equation {
        Equation::Scalar(s) => Ok(rewrite_str(s, specs)?.map(Equation::Scalar)),
        Equation::ApplyToAll(dims, s) => {
            Ok(rewrite_str(s, specs)?.map(|ns| Equation::ApplyToAll(dims.clone(), ns)))
        }
        Equation::Arrayed(dims, elems, default, has_except) => {
            let mut changed = false;
            let mut new_elems = Vec::with_capacity(elems.len());
            for (elem, s, uid, aux) in elems {
                let ns = match rewrite_str(s, specs)? {
                    Some(ns) => {
                        changed = true;
                        ns
                    }
                    None => s.clone(),
                };
                new_elems.push((elem.clone(), ns, uid.clone(), aux.clone()));
            }
            let new_default = match default {
                Some(d) => match rewrite_str(d, specs)? {
                    Some(nd) => {
                        changed = true;
                        Some(nd)
                    }
                    None => Some(d.clone()),
                },
                None => None,
            };
            if changed {
                Ok(Some(Equation::Arrayed(
                    dims.clone(),
                    new_elems,
                    new_default,
                    *has_except,
                )))
            } else {
                Ok(None)
            }
        }
    }
}

/// Recursively rewrite supported container-access subexpressions in `expr` to a
/// reference to the synthesized container variable (§10), registering each in
/// `specs` and setting `changed` when a substitution is made. Unsupported forms
/// return `Err`. See [`rewrite_container_equation`] for the equation-level entry.
fn rewrite_container_in_expr(
    expr: &crate::ast::Expr0,
    conveyor_dims: &HashMap<String, usize>,
    naming: &ContainerNaming,
    specs: &mut std::collections::BTreeMap<String, ContainerVarSpec>,
    changed: &mut bool,
) -> Result<crate::ast::Expr0, (ErrorCode, String)> {
    use crate::ast::{Expr0, IndexExpr0};
    use crate::builtins::UntypedBuiltinFn;
    use crate::common::RawIdent;

    // Register a container variable and return the AST reference to it: a bare
    // `Var` for a scalar conveyor/queue, or a `Subscript` carrying the element
    // selectors for an arrayed one (so the ordinary array machinery indexes the
    // arrayed container stock element-wise).
    let mut register =
        |owner_stock: String, kind: ContainerKind, elem_indices: Vec<IndexExpr0>| -> Expr0 {
            let name = container_var_name(naming, &owner_stock, &kind);
            specs
                .entry(name.clone())
                .or_insert(ContainerVarSpec { owner_stock, kind });
            *changed = true;
            let loc = crate::builtins::Loc::default();
            if elem_indices.is_empty() {
                Expr0::Var(RawIdent::new(name), loc)
            } else {
                Expr0::Subscript(RawIdent::new(name), elem_indices, loc)
            }
        };

    match expr {
        Expr0::Const(..) | Expr0::Var(..) => Ok(expr.clone()),
        Expr0::Subscript(raw, indices, loc) => {
            let name = canon(raw.as_str());
            if let Some(&ndims) = conveyor_dims.get(&name) {
                // A subscript with one MORE index than array dims is element
                // access into the container `conv[elem.., j]` (`conv[j]`/`queue[j]`
                // for a scalar container).
                if indices.len() == ndims + 1 {
                    let slat_idx = &indices[ndims];
                    let Some(j) = const_slat_index(slat_idx) else {
                        return Err(unsupported_container(
                            naming,
                            &name,
                            "a container index must be a constant integer (dynamic subscript)",
                        ));
                    };
                    let elem_indices = indices[..ndims].to_vec();
                    if !all_single_element(&elem_indices) {
                        return Err(unsupported_container(
                            naming,
                            &name,
                            "a ranged/wildcard array subscript cannot select one container",
                        ));
                    }
                    return Ok(register(name, ContainerKind::Slat(j), elem_indices));
                }
                // More indices than that: a multi-dimensional container subscript
                // we do not model.
                if indices.len() > ndims + 1 {
                    return Err(unsupported_container(
                        naming,
                        &name,
                        "multi-dimensional container subscripting is unsupported",
                    ));
                }
                // indices.len() <= ndims: ordinary array-element access of the
                // container's per-element totals -- not container access. Fall
                // through to recurse into the index expressions.
            }
            // Recurse into index sub-expressions (a nested container access, e.g.
            // `x[conv[1]]`, is rewritten; a range's bounds likewise).
            let mut new_indices = Vec::with_capacity(indices.len());
            for idx in indices {
                new_indices.push(match idx {
                    IndexExpr0::Expr(e) => IndexExpr0::Expr(rewrite_container_in_expr(
                        e,
                        conveyor_dims,
                        naming,
                        specs,
                        changed,
                    )?),
                    IndexExpr0::Range(l, r, iloc) => IndexExpr0::Range(
                        rewrite_container_in_expr(l, conveyor_dims, naming, specs, changed)?,
                        rewrite_container_in_expr(r, conveyor_dims, naming, specs, changed)?,
                        *iloc,
                    ),
                    other => other.clone(),
                });
            }
            Ok(Expr0::Subscript(raw.clone(), new_indices, *loc))
        }
        Expr0::App(UntypedBuiltinFn(fname, args), loc) => {
            // A single-argument container reducer over ONE container is supported.
            if let Some(kind) = reduce_kind(fname)
                && args.len() == 1
            {
                let arg = &args[0];
                // A bare arrayed container `Var`: SUM(conv) is the ordinary reduce
                // over per-element totals (== sum over all elements), left
                // untouched; every OTHER bare-arrayed reducer reads the element
                // vector, which the per-element totals do not represent -- reject
                // loudly rather than silently return a mean/min/max/size of totals.
                if let Expr0::Var(vraw, _) = arg {
                    let vname = canon(vraw.as_str());
                    if matches!(conveyor_dims.get(&vname), Some(&nd) if nd > 0) && *fname != "sum" {
                        return Err(unsupported_container(
                            naming,
                            &vname,
                            "a bare arrayed reducer other than SUM has no single-container \
                             interpretation; reduce one via x[elem]",
                        ));
                    }
                }
                // A direct single-container operand (scalar container `Var` or an
                // arrayed container `conv[elem]` subscript): reduce it natively.
                // `direct_belt_reference` returns `None` for a bare arrayed `Var`,
                // so SUM(conv) over per-element totals falls through to the
                // ordinary recurse below.
                if let Some((conv, elem_indices)) = direct_belt_reference(arg, conveyor_dims) {
                    return Ok(register(conv, kind, elem_indices));
                }
                // The container appears wrapped in an expression (not a direct
                // single-container reference): reducing it would need the per-
                // element vector, which we cannot lower to one scalar.
                if let Some(conv) = subtree_has_belt_reference(arg, conveyor_dims) {
                    return Err(unsupported_container(
                        naming,
                        &conv,
                        "a reducer over an expression involving the container cannot be \
                         reduced to one native scalar",
                    ));
                }
            }
            // Not container access: recurse into every argument.
            let mut new_args = Vec::with_capacity(args.len());
            for arg in args {
                new_args.push(rewrite_container_in_expr(
                    arg,
                    conveyor_dims,
                    naming,
                    specs,
                    changed,
                )?);
            }
            Ok(Expr0::App(UntypedBuiltinFn(fname.clone(), new_args), *loc))
        }
        Expr0::Op1(op, inner, loc) => Ok(Expr0::Op1(
            *op,
            Box::new(rewrite_container_in_expr(
                inner,
                conveyor_dims,
                naming,
                specs,
                changed,
            )?),
            *loc,
        )),
        Expr0::Op2(op, l, r, loc) => Ok(Expr0::Op2(
            *op,
            Box::new(rewrite_container_in_expr(
                l,
                conveyor_dims,
                naming,
                specs,
                changed,
            )?),
            Box::new(rewrite_container_in_expr(
                r,
                conveyor_dims,
                naming,
                specs,
                changed,
            )?),
            *loc,
        )),
        Expr0::If(c, t, f, loc) => Ok(Expr0::If(
            Box::new(rewrite_container_in_expr(
                c,
                conveyor_dims,
                naming,
                specs,
                changed,
            )?),
            Box::new(rewrite_container_in_expr(
                t,
                conveyor_dims,
                naming,
                specs,
                changed,
            )?),
            Box::new(rewrite_container_in_expr(
                f,
                conveyor_dims,
                naming,
                specs,
                changed,
            )?),
            *loc,
        )),
    }
}

/// Run the container-access rewrite ([`rewrite_container_equation`]) over every
/// variable of `model`, returning the canonical-name -> rewritten-equation map
/// the caller's mutable Pass 2 applies (via [`set_variable_equation`]). A
/// pass-driven flow's equation becomes a `0` placeholder in Pass 2, so
/// rewriting it would be discarded -- members of `driven` are skipped. Shared
/// by [`expand_conveyors`] and [`crate::queue_compile::expand_queues`] (§10 /
/// queues.md §8): the scan is identical for both owners, which differ only in
/// the [`ContainerNaming`] and the container-dims map.
pub(crate) fn rewrite_model_container_equations(
    model: &datamodel::Model,
    driven: &std::collections::HashSet<String>,
    container_dims: &HashMap<String, usize>,
    naming: &ContainerNaming,
    specs: &mut std::collections::BTreeMap<String, ContainerVarSpec>,
) -> Result<HashMap<String, Equation>, (ErrorCode, String)> {
    let mut rewritten = HashMap::new();
    for v in &model.variables {
        if driven.contains(&canon(v.get_ident())) {
            continue;
        }
        let Some(eqn) = v.get_equation() else {
            continue; // a Module carries no equation
        };
        if let Some(new_eqn) = rewrite_container_equation(eqn, container_dims, naming, specs)? {
            rewritten.insert(canon(v.get_ident()), new_eqn);
        }
    }
    Ok(rewritten)
}

/// Build a hidden synthesized aux carrying an arbitrary [`Equation`] (scalar for
/// a scalar conveyor, `ApplyToAll`/`Arrayed` for an arrayed one, §10).
fn make_aux_eqn(ident: &str, equation: Equation) -> datamodel::Aux {
    datamodel::Aux {
        ident: ident.to_string(),
        equation,
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    }
}

/// The synthesized-aux equation for a conveyor parameter expression (`<len>`,
/// `<capacity>`, ...). The datamodel holds one expression per attribute, so an
/// arrayed conveyor's parameter is the same apply-to-all expression across every
/// element (§10); it can still reference arrayed variables to yield distinct
/// per-element values. `dims` empty ⇒ a scalar aux.
fn param_equation(expr: &str, dims: &[DimensionName]) -> Equation {
    if dims.is_empty() {
        Equation::Scalar(expr.to_string())
    } else {
        Equation::ApplyToAll(dims.to_vec(), expr.to_string())
    }
}

/// The resolved source of a leak flow's fraction: §3.3's two encodings, in
/// the order [`leak_fraction_source`] applies them. `Debug` is
/// `debug-derive`-gated because the `FlowEquation` variant borrows a
/// [`datamodel::Equation`], whose own `Debug` is gated the same way (and
/// libsimlin builds the engine with `default-features = false`).
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq)]
pub(crate) enum LeakFractionSource<'a> {
    /// A non-empty value-bearing `<leak>expr</leak>` fraction (one shared
    /// expression; for an arrayed conveyor it is applied to all elements).
    Explicit(&'a str),
    /// A bare `<leak/>` marker: the flow's own `<eqn>` carries the fraction
    /// -- the encoding real Stella files use (docs/design/conveyors.md §3.3).
    FlowEquation(&'a Equation),
    /// No fraction anywhere (a bare marker on an empty-equation flow): a
    /// valid "leakage, fraction TBD" state contributing zero leakage until a
    /// fraction is supplied (§3.3).
    Absent,
}

/// Resolve which expression carries a leak flow's fraction, in the §3.3
/// order: a non-empty explicit `<leak>` fraction wins; otherwise the flow's
/// own non-empty `<eqn>` carries it. SHARED by the runtime expansion
/// ([`leak_fraction_equation`]) and the §5.1 compile-time advisory
/// (`crate::db::diagnostic::emit_conveyor_spec_warnings`) so the two
/// resolutions cannot drift -- a fraction the runtime would apply is exactly
/// a fraction the advisory sums.
pub(crate) fn leak_fraction_source<'a>(
    leakage: Option<&'a datamodel::Leakage>,
    equation: &'a Equation,
) -> LeakFractionSource<'a> {
    if let Some(leak) = leakage
        && let Some(frac) = &leak.fraction
        && !frac.is_empty()
    {
        return LeakFractionSource::Explicit(frac);
    }
    match equation {
        Equation::Scalar(s) | Equation::ApplyToAll(_, s) if !s.is_empty() => {
            LeakFractionSource::FlowEquation(equation)
        }
        // A per-element equation always carries fractions (element entries),
        // even when its default is None.
        Equation::Arrayed(..) => LeakFractionSource::FlowEquation(equation),
        _ => LeakFractionSource::Absent,
    }
}

/// The synthesized leak-fraction aux equation for a (possibly arrayed) leak flow
/// (§5.1/§10). Prefers the explicit `<leak>` fraction (a single expression, made
/// apply-to-all over the conveyor's dims); otherwise the flow's own equation
/// carries the fraction (the bare-`<leak/>`-plus-`<eqn>` form) and its arrayed
/// shape is preserved so a genuinely per-element fraction is not flattened. An
/// empty fraction leaks nothing (`0`). The which-expression decision is the
/// shared [`leak_fraction_source`]; only the array-shaping lives here.
fn leak_fraction_equation(flow: &datamodel::Flow, dims: &[DimensionName]) -> Equation {
    match leak_fraction_source(flow.compat.leakage.as_ref(), &flow.equation) {
        LeakFractionSource::Explicit(frac) => param_equation(frac, dims),
        LeakFractionSource::FlowEquation(eqn) => match eqn {
            Equation::Scalar(s) => param_equation(s, dims),
            Equation::ApplyToAll(d, s) => Equation::ApplyToAll(d.clone(), s.clone()),
            Equation::Arrayed(d, elems, default, except) => {
                Equation::Arrayed(d.clone(), elems.clone(), default.clone(), *except)
            }
        },
        LeakFractionSource::Absent => param_equation("0", dims),
    }
}

/// The placeholder-`0` equation for a pass-driven flow (a conveyor's primary
/// outflow / leak, or a queue's driven outflow), preserving the flow's array
/// shape so an arrayed driven flow compiles to `N_elem` writable slots (one per
/// belt/FIFO) rather than collapsing to a single scalar slot (§10, queues.md
/// §6). The pass overwrites every slot each step, so the placeholder value
/// never matters -- only the slot count does. Shared by [`expand_conveyors`]
/// and [`crate::queue_compile::expand_queues`].
pub(crate) fn placeholder_zero_equation(existing: &Equation) -> Equation {
    match existing {
        Equation::Scalar(_) => Equation::Scalar("0".to_string()),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
            Equation::ApplyToAll(dims.clone(), "0".to_string())
        }
    }
}

/// The dimension names an arrayed conveyor/queue stock (or driven flow) is
/// declared over, in declaration order; empty for a scalar variable. Shared by
/// [`expand_conveyors`] and [`crate::queue_compile::expand_queues`].
pub(crate) fn equation_dims(equation: &Equation) -> Vec<DimensionName> {
    match equation {
        Equation::Scalar(_) => Vec::new(),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => dims.clone(),
    }
}

/// Resolve the named dimensions of an arrayed conveyor/queue to the runtime
/// [`Dimension`](crate::dimensions::Dimension) list (with element names), in
/// declaration order, then enumerate the per-element subscript suffixes in the
/// SAME row-major order the compiled offset map uses
/// (`calc_flattened_offsets_incremental` drives its element keys off the identical
/// `SubscriptIterator`). Each returned suffix is the canonical `elem1,elem2`
/// string. Returns an error if any dimension name is unknown in the project (an
/// internal-consistency guard, §10). Shared by conveyors and queues (queue.md
/// §6); `noun` (`conveyor`/`queue`) makes the diagnostic read correctly and the
/// error code is shared ([`ErrorCode::ConveyorArrayedDimensionUnresolved`]).
pub(crate) fn element_subscripts_for_dims(
    project: &datamodel::Project,
    dim_names: &[DimensionName],
    stock: &str,
    noun: &str,
) -> Result<Vec<String>, (ErrorCode, String)> {
    if dim_names.is_empty() {
        return Ok(Vec::new());
    }
    let mut dims = Vec::with_capacity(dim_names.len());
    for name in dim_names {
        let canon_name = canon(name);
        let dim = project
            .dimensions
            .iter()
            .find(|d| canon(&d.name) == canon_name)
            .ok_or_else(|| {
                (
                    ErrorCode::ConveyorArrayedDimensionUnresolved,
                    format!(
                        "arrayed {noun} '{stock}' is declared over dimension '{name}', which \
                         the project does not define"
                    ),
                )
            })?;
        dims.push(crate::dimensions::Dimension::from(dim));
    }
    Ok(crate::dimensions::SubscriptIterator::new(&dims)
        .map(|subs| subs.join(","))
        .collect())
}

/// The number of independent belts/FIFOs a conveyor or queue meta expands to,
/// given its per-element subscript suffixes: `N_elem` for an arrayed stock
/// (§10, queues.md §6), 1 for a scalar one (the degenerate case, whose
/// `element_subscripts` is empty). Shared by both `resolve_plans` flattenings
/// and the queue-conveyor coupling resolution.
pub(crate) fn n_elements(element_subscripts: &[String]) -> usize {
    element_subscripts.len().max(1)
}

/// Element-aware offset lookup into the compiled offset map: the bare `name`
/// for a scalar conveyor/queue (`element_subscripts` empty), the subscripted
/// `name[elem1,elem2]` key for element `e` of an arrayed one -- the same
/// row-major keys `calc_flattened_offsets_incremental` lays out. Shared by
/// both `resolve_plans` flattenings (whose per-element `eoff` closures wrap
/// it) and the coupling resolution in [`crate::queue_compile`].
pub(crate) fn element_offset(
    offsets: &HashMap<Ident<Canonical>, usize>,
    element_subscripts: &[String],
    e: usize,
    name: &str,
) -> Option<usize> {
    if element_subscripts.is_empty() {
        offsets.get(&Ident::<Canonical>::new(name)).copied()
    } else {
        let key = format!("{}[{}]", name, element_subscripts[e]);
        offsets.get(&Ident::<Canonical>::new(&key)).copied()
    }
}

/// Resolve one belt's/FIFO's container metas to [`ContainerPlan`]s through the
/// element-aware offset resolver (§10, queues.md §8). The container stock is
/// arrayed over its owner's dims, so element `e` of the container aligns with
/// belt/FIFO `e`; a missing slot propagates `None`. Shared by both
/// `resolve_plans` flattenings.
pub(crate) fn resolve_container_plans(
    containers: &[ContainerMeta],
    eoff: impl Fn(&str) -> Option<usize>,
) -> Option<Vec<ContainerPlan>> {
    containers
        .iter()
        .map(|c| {
            Some(ContainerPlan {
                off: eoff(&c.name)?,
                kind: c.kind.clone(),
            })
        })
        .collect()
}

/// Resolve [`ConveyorMeta`] names to data-buffer offsets using the compiled
/// simulation's offset map, flattening each arrayed conveyor into ONE
/// [`ConveyorPlan`] per array element (§10). An arrayed variable's elements
/// occupy contiguous slots keyed `name[elem1,elem2]` in the offset map
/// (`calc_flattened_offsets_incremental`), so element `e` resolves via the
/// subscripted key built from `meta.element_subscripts[e]`; a scalar conveyor
/// resolves its bare name and yields a single plan (so the per-belt runtime pass
/// is identical to before). Each meta's belts occupy a contiguous flattened-plan
/// range, so a held-exit destination (§4.3 step 3) links element `e` of one
/// conveyor to element `e` of the downstream conveyor. Returns `None` if any
/// required name is missing -- an internal inconsistency between expansion and
/// compilation that [`crate::queue_compile::build_compiled`] surfaces as a hard `NotSimulatable`
/// error (there is no non-conveyor fallback: the model has conveyors).
pub fn resolve_plans(
    metas: &[ConveyorMeta],
    offsets: &HashMap<Ident<Canonical>, usize>,
) -> Option<Vec<ConveyorPlan>> {
    // Stock-name -> (base flattened-plan index, belt count). Each meta's belts
    // are appended in order, so its range is [base, base + n_belts).
    let mut stock_to_range: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut base = 0usize;
    for m in metas {
        let n_belts = n_elements(&m.element_subscripts);
        stock_to_range.insert(m.stock.as_str(), (base, n_belts));
        base += n_belts;
    }

    let mut plans = Vec::with_capacity(base);
    for meta in metas {
        for e in 0..n_elements(&meta.element_subscripts) {
            // Element-aware offset resolver: the bare name for a scalar conveyor,
            // the `name[elem]` subscripted key for element `e` of an arrayed one.
            let eoff = |name: &str| element_offset(offsets, &meta.element_subscripts, e, name);
            let leaks = meta
                .leaks
                .iter()
                .map(|l| {
                    // A leak flow feeding a downstream conveyor links element `e`
                    // to the same element of that conveyor (identical element-wise
                    // linkage to the primary's held-exit destination below), so the
                    // runtime can skip the leak while its destination is arrested
                    // (§4.3 step 2). Mismatched shapes leave it unlinked.
                    let dest_conveyor = l.dest_conveyor.as_deref().and_then(|stock| {
                        match stock_to_range.get(stock) {
                            Some(&(dest_base, dest_n)) if e < dest_n => Some(dest_base + e),
                            _ => None,
                        }
                    });
                    Some(LeakPlan {
                        flow_off: eoff(&l.flow)?,
                        frac_off: eoff(&l.frac_aux)?,
                        zone_start: l.zone_start,
                        zone_end: l.zone_end,
                        integers: l.integers,
                        dest_conveyor,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            let inflows = meta
                .inflows
                .iter()
                .map(|i| {
                    Some(InflowPlan {
                        flow_off: eoff(&i.flow)?,
                        conveyor_driven: i.conveyor_driven,
                        placement: i.placement.clone(),
                        dist: i.dist.clone(),
                        source: i.source,
                        // Filled by the post-pass below once every plan's leak
                        // offsets are known.
                        source_leak: None,
                        // Set later by queue-conveyor coupling resolution; every
                        // ordinary inflow resolves un-coupled.
                        queue_coupled: false,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            // Container variables read this belt (§10). The container stock is
            // arrayed over the conveyor's dims, so element `e` of the container
            // resolves to belt `e` via the same element-aware offset lookup.
            let containers = resolve_container_plans(&meta.containers, eoff)?;
            // A held-exit destination links element `e` to the same element of
            // the downstream conveyor. If the downstream has fewer elements
            // (mismatched shapes, not element-wise coupled), leave it unlinked --
            // cross-shape conveyor chaining is a later step.
            let primary_dest_conveyor = meta.primary_dest_conveyor.as_deref().and_then(|stock| {
                match stock_to_range.get(stock) {
                    Some(&(dest_base, dest_n)) if e < dest_n => Some(dest_base + e),
                    _ => None,
                }
            });
            plans.push(ConveyorPlan {
                name: meta.stock.clone(),
                stock_off: eoff(&meta.stock)?,
                len_off: eoff(&meta.len_aux)?,
                cap_off: meta.cap_aux.as_deref().and_then(&eoff),
                inlim_off: meta.inlim_aux.as_deref().and_then(&eoff),
                sample_off: meta.sample_aux.as_deref().and_then(&eoff),
                arrest_off: meta.arrest_aux.as_deref().and_then(&eoff),
                discrete: meta.discrete,
                exponential_leak: meta.exponential_leak,
                ignore_earlier_zone_losses: meta.ignore_earlier_zone_losses,
                primary_out_off: eoff(&meta.primary_out)?,
                leaks,
                inflows,
                containers,
                primary_dest_conveyor,
                // Element `e`'s own §7.2 list (expansion replicated a shared
                // scalar/apply-to-all list per belt, and matched per-element
                // lists by canonical subscript); `None` = §7.1 steady fill.
                init_values: meta.init_values.get(e).cloned().flatten(),
            });
        }
    }
    // Resolve each `source` inflow's upstream-leak coupling now that every
    // plan's leak offsets exist (§8): the flow-identity match reads only
    // compile-time-constant plan data, so precomputing it here (GH #878-style)
    // spares the per-step pass an all-plans rescan per source inflow. Later
    // queue-coupling resolution only flips `queue_coupled` flags and rewrites
    // queue outflow kinds -- it never changes leak offsets or plan order -- so
    // the resolved indices stay valid.
    for pi in 0..plans.len() {
        for fi in 0..plans[pi].inflows.len() {
            if plans[pi].inflows[fi].source {
                let hit = find_upstream_leak(&plans, plans[pi].inflows[fi].flow_off);
                plans[pi].inflows[fi].source_leak = hit;
            }
        }
    }
    Some(plans)
}

// ----- runtime: belt initialization and the per-DT pass -----

/// A value is "nonzero" for arrest/sample when it is nonzero and not NaN (§4.4).
fn is_nonzero(v: f64) -> bool {
    v != 0.0 && !v.is_nan()
}

/// §4.4 leak-fraction hygiene: linear fractions clamp to `[0, 1]`, exponential
/// rates to `[0, ∞)`, NaN ⇒ 0. `pub(crate)` because the §5.1 compile-time
/// advisory (`db::diagnostic::emit_conveyor_spec_warnings`) applies the SAME
/// per-fraction hygiene to each constant term before summing -- in particular
/// the NaN ⇒ 0 rule: `f64::clamp` PROPAGATES NaN, so an unshared clamp would
/// let a literal `nan` fraction poison the sum into silence while the runtime
/// zeroes it and leaks the other fractions at full rate.
pub(crate) fn clamp_fraction(v: f64, exponential: bool) -> f64 {
    if v.is_nan() {
        0.0
    } else if exponential {
        v.max(0.0)
    } else {
        v.clamp(0.0, 1.0)
    }
}

/// §4.4 transit hygiene: a finite value clamps to `max(dt, value)`; a non-finite
/// value is passed through unchanged so `phase_a` skips the latch.
fn clamp_transit(v: f64, dt: f64) -> f64 {
    if v.is_finite() { v.max(dt) } else { v }
}

/// §4.4 capacity / inflow-limit hygiene: NaN or `+INF` ⇒ INF (no constraint);
/// negative (incl. `-INF`) ⇒ 0 (fully blocking).
fn clamp_cap(v: f64) -> f64 {
    if v.is_nan() {
        f64::INFINITY
    } else if v.is_infinite() {
        if v > 0.0 { f64::INFINITY } else { 0.0 }
    } else if v < 0.0 {
        0.0
    } else {
        v
    }
}

/// §8 `dist` per-slat weights over the entry path `i ∈ 0..d`, exit-first
/// (`x_i = 1 - (i+0.5)/d`), clamped to `[0, ∞)`. `Placement::Dist` normalizes
/// these to shares; all-zero weights make it fall back to `Beginning`. A `d` of
/// 0 yields an empty vector (also a `Beginning` fallback).
///
/// Rebuilt each step (one `d`-length allocation per dist inflow -- a constant
/// allocation COUNT, unlike the per-share blowup GH #879 fixed): the weights
/// are a pure function of `(profile, d)`, and `d` is the just-latched entry
/// depth, which a `<sample>`d time-varying transit changes mid-run -- so the
/// vector cannot be precomputed at plan time. A `d`-keyed memo would be safe
/// but needs mutable per-inflow state the pass deliberately does not carry.
fn dist_weights(profile: &DistProfile, d: usize) -> Vec<f64> {
    (0..d)
        .map(|i| {
            let x = 1.0 - (i as f64 + 0.5) / d as f64;
            let w = match profile {
                DistProfile::Array(a) if !a.is_empty() => {
                    let m = a.len();
                    // floor(x*m), clamped to a valid index (x in (0,1) keeps it
                    // in range; the clamp is defense in depth for the endpoints).
                    let idx = (x * m as f64).floor();
                    let idx = if idx.is_finite() {
                        (idx as isize).clamp(0, m as isize - 1) as usize
                    } else {
                        0
                    };
                    a[idx]
                }
                DistProfile::Array(_) => 0.0,
                DistProfile::Gf(table) => crate::vm::lookup(table, x),
            };
            // A negative or NaN sample contributes no weight (§8: w_i = max(0, .)).
            if w.is_nan() { 0.0 } else { w.max(0.0) }
        })
        .collect()
}

/// §8 `source` per-target-slat weights: mirror an upstream leak's per-slat
/// leakage onto the downstream entry path. `up_slat_leak[j]` is the volume the
/// upstream leak shed from its slat `j` (0 = exit) this DT; `y_j = 1 -
/// (j+0.5)/L_up` is that slat's fractional-position-from-entry. Each `q_j` lands
/// at the target slat `i ∈ 0..d` whose `x_i = 1 - (i+0.5)/d` is nearest `y_j`
/// (ties toward the exit, i.e. the smaller `i`). `Σ weights == Σ up_slat_leak`,
/// so `Placement::Dist` places exactly the mirrored volumes.
fn source_weights(up_slat_leak: &[f64], d: usize) -> Vec<f64> {
    let mut weights = vec![0.0; d];
    let l_up = up_slat_leak.len();
    if d == 0 || l_up == 0 {
        return weights;
    }
    for (j, &q) in up_slat_leak.iter().enumerate() {
        if q == 0.0 {
            continue;
        }
        let y = 1.0 - (j as f64 + 0.5) / l_up as f64;
        // Nearest target slat by |x_i - y|; iterating i ascending with a strict
        // `<` keeps the smaller i (nearer the exit) on an exact tie. Scanning
        // all d target slats per source slat made this O(L_up * d) every step
        // (GH #879); instead only a +-2 window around the exact-arithmetic
        // optimum is scanned, with the IDENTICAL float distance expressions and
        // the same ascending strict-`<` selection, so the chosen index is
        // bit-for-bit the full scan's.
        //
        // Why the window suffices: in exact arithmetic
        // |x_i - y| = |(2j+1)*d - (2i+1)*L_up| / (2 * L_up * d), an integer
        // numerator that steps by 2*L_up per increment of i. The unconstrained
        // real minimizer therefore lies within 1 of q0 = floor(t / (2*L_up))
        // (t = (2j+1)*d), and any i two or more steps from it has a real
        // distance at least 1/d above the minimum. The float distances below
        // differ from the real ones by only a few ULP of 1 (~1e-15), far under
        // that 1/d >= 1e-6 gap (d and L_up are bounded by
        // `conveyor::MAX_SLATS_PER_BELT`), so no index outside the window can
        // tie or beat the window's float minimum; exact float ties INSIDE the
        // window resolve by the same first-wins ascending scan. The
        // debug_assert re-runs the full scan to pin the equivalence in every
        // debug/test build. u64 keeps `t` exact on 32-bit targets (wasm).
        let t = (2 * j as u64 + 1) * d as u64;
        let q0 = (t / (2 * l_up as u64)).min(d as u64 - 1) as usize;
        let lo = q0.saturating_sub(2);
        let hi = (q0 + 2).min(d - 1);
        let mut best_i = lo;
        let mut best_dist = f64::INFINITY;
        for i in lo..=hi {
            let x_i = 1.0 - (i as f64 + 0.5) / d as f64;
            let dist = (x_i - y).abs();
            if dist < best_dist {
                best_dist = dist;
                best_i = i;
            }
        }
        #[cfg(debug_assertions)]
        {
            let mut full_best = 0usize;
            let mut full_dist = f64::INFINITY;
            for i in 0..d {
                let x_i = 1.0 - (i as f64 + 0.5) / d as f64;
                let dist = (x_i - y).abs();
                if dist < full_dist {
                    full_dist = dist;
                    full_best = i;
                }
            }
            debug_assert_eq!(
                best_i, full_best,
                "windowed nearest-slat search must match the full scan \
                 (j={j}, L_up={l_up}, d={d})"
            );
        }
        weights[best_i] += q;
    }
    weights
}

/// Find the upstream conveyor plan index and leak index whose leak flow occupies
/// data-buffer slot `flow_off` -- the flow-identity coupling a `source` inflow
/// uses (§8). `None` when the slot is not an upstream leak (e.g. it is a primary
/// outflow, or an ordinary flow), in which case `source` falls back to
/// `Beginning`. Called once per source inflow at plan-resolution time
/// ([`resolve_plans`] stores the result on [`InflowPlan::source_leak`]), not in
/// the per-step pass.
fn find_upstream_leak(plans: &[ConveyorPlan], flow_off: usize) -> Option<(usize, usize)> {
    plans.iter().enumerate().find_map(|(u, p)| {
        p.leaks
            .iter()
            .position(|leak| leak.flow_off == flow_off)
            .map(|k| (u, k))
    })
}

/// The run-time [`Placement`] for an equation-driven inflow (§8). `dist` builds
/// its weights from the entry depth `d`; `source` has no upstream leak to mirror
/// on an equation-driven inflow, so it degrades to the static fallback.
fn eq_inflow_placement(inf: &InflowPlan, d: usize) -> Placement {
    match &inf.dist {
        Some(profile) => Placement::Dist(dist_weights(profile, d)),
        None => inf.placement.clone(),
    }
}

/// The run-time [`Placement`] for a conveyor-driven inflow (§8). `source`
/// mirrors the matched upstream leak's per-slat leakage (via the
/// [`InflowPlan::source_leak`] pair [`resolve_plans`] precomputed); `dist`
/// samples its profile; everything else uses the static placement.
fn conv_inflow_placement(inf: &InflowPlan, pa: &[PhaseAResult], d: usize) -> Placement {
    if inf.source {
        return match inf.source_leak {
            Some((u, k)) => Placement::Dist(source_weights(&pa[u].leak_slat_vols[k], d)),
            None => Placement::Beginning,
        };
    }
    match &inf.dist {
        Some(profile) => Placement::Dist(dist_weights(profile, d)),
        None => inf.placement.clone(),
    }
}

/// §4.1 slat-count bound. `round(transit/dt)` sizes the belt `Vec`; an enormous
/// `transit/dt` (a hostile or typo'd `<len>`) would request an unbounded
/// allocation -- a `usize`-saturating count panics `vec![0.0; usize::MAX]`
/// ("capacity overflow" -> host abort under `panic = "abort"`), a merely-huge
/// finite one OOMs. A latched transit whose slat count exceeds
/// [`crate::conveyor::slat_bound`] is rejected loudly (naming the belt, the
/// computed count, and the bound) rather than silently saturating the geometry.
/// Enforced at BOTH latch sites -- belt init ([`init_belts`]) and the mid-run
/// `<sample>` re-latch ([`run_phase_a`]) -- so `latched_transit` always yields a
/// slat count within the bound and every downstream `n_slats()` allocation is
/// safe.
fn check_slat_bound(name: &str, transit: f64, dt: f64) -> Result<(), (ErrorCode, String)> {
    let n = crate::conveyor::slat_count(transit, dt);
    let bound = crate::conveyor::slat_bound();
    if n > bound {
        return Err((
            ErrorCode::ConveyorTransitTooLong,
            format!(
                "conveyor '{name}' transit time {transit} at dt {dt} needs {n} belt slats, \
                 exceeding the maximum of {bound}"
            ),
        ));
    }
    Ok(())
}

/// Build the conveyor side table from the initials-populated data buffer
/// (`curr`), initializing each belt either to its steady-state fill (§7.1)
/// from the stock's initial value -- the stock `<eqn>` was evaluated by the
/// initials pass, so `curr[stock_off]` holds the scalar initial value `V` --
/// or, for a plan carrying a §7.2 explicit init list, directly from the list
/// via [`ConveyorState::init_explicit`]. In the explicit case the stock's
/// compiled `<eqn>` is the expansion-time normalized-total placeholder
/// ([`normalized_init_total`], the same fill run here), so `curr[stock_off]`
/// already holds the belt total; it is re-written anyway as defense in depth,
/// and the caller (vm.rs `run_initials`) re-reconciles dependent initials and
/// the `INIT()` snapshot from the slot.
pub fn init_belts(
    plans: &[ConveyorPlan],
    curr: &mut [f64],
    dt: f64,
) -> Result<Vec<ConveyorState>, (ErrorCode, String)> {
    let mut states = Vec::with_capacity(plans.len());
    for plan in plans {
        let transit = curr[plan.len_off];
        if !transit.is_finite() || transit <= 0.0 {
            return Err((
                ErrorCode::ConveyorTransitNotPositive,
                format!(
                    "conveyor '{}' transit time must be positive and finite, got {transit}",
                    plan.name
                ),
            ));
        }
        // Reject an over-bound slat count before init_steady allocates the belt
        // (§4.1): a saturating/enormous count would otherwise panic/OOM here.
        check_slat_bound(&plan.name, transit, dt)?;
        let leaks: Vec<LeakConfig> = plan
            .leaks
            .iter()
            .map(|l| LeakConfig {
                zone_start: l.zone_start,
                zone_end: l.zone_end,
                integers: l.integers,
            })
            .collect();
        let mut state = ConveyorState::new(
            dt,
            plan.exponential_leak,
            plan.discrete,
            plan.ignore_earlier_zone_losses,
            leaks,
        );
        let fracs: Vec<f64> = plan
            .leaks
            .iter()
            .map(|l| clamp_fraction(curr[l.frac_off], plan.exponential_leak))
            .collect();
        match &plan.init_values {
            Some(values) => {
                state.init_explicit(transit, values, &fracs);
                let total = state.contents();
                // The expansion computed the stock's placeholder <eqn> with
                // the SAME fill ([`normalized_init_total`] requires a
                // compile-time-constant transit precisely so it can), so the
                // initials-evaluated slot already holds this total and the
                // write-back is defense in depth. The assert documents that
                // contract; tolerance covers float-print round-tripping.
                debug_assert!(
                    (curr[plan.stock_off] - total).abs() <= 1e-9 * total.abs().max(1.0),
                    "conveyor '{}': placeholder initial {} != normalized belt total {}",
                    plan.name,
                    curr[plan.stock_off],
                    total
                );
                curr[plan.stock_off] = total;
            }
            None => {
                let v = curr[plan.stock_off];
                state.init_steady(transit, v, &fracs);
            }
        }
        states.push(state);
    }
    Ok(states)
}

/// Publish each conveyor's container-access results into their data-buffer slots
/// (§10). Called at STEP-START -- before the Flows phase in the Euler loop and
/// after belt initialization in `run_initials` -- so the published values
/// reflect the belt as left by the previous step (= start-of-step for this
/// step). Each container variable is a hidden no-flow STOCK, so the Flows phase
/// never recomputes its slot and the Stocks phase leaves it unchanged: the value
/// is visible to Flows-phase readers and survives the whole step.
///
/// The container value is computed by the SHARED [`container_value_from_slice`]
/// over the belt's exit-first slat vector: `conv[j]` (1-based from the exit)
/// maps to `slat_contents()[j-1]`, the reducers to the slat-volume vector, and
/// `SIZE` to the physical belt length. The one `slat_contents()` allocation per
/// belt is hoisted out of the per-container loop (and skipped entirely for a
/// belt with no containers), mirroring
/// [`crate::queue_compile::publish_queue_container_values`].
pub fn publish_container_values(
    plans: &[ConveyorPlan],
    states: &[ConveyorState],
    curr: &mut [f64],
) {
    for (plan, state) in plans.iter().zip(states.iter()) {
        if plan.containers.is_empty() {
            continue;
        }
        let slats = state.slat_contents();
        for c in &plan.containers {
            curr[c.off] = container_value_from_slice(&slats, &c.kind);
        }
    }
}

/// The two-phase conveyor pass (§4.3), run once per Euler step between the flows
/// and stocks phases. Reads parameter/fraction/requested-inflow values from
/// `curr` and writes the conveyor-driven flow rates back into `curr`, so that
/// ordinary stock integration then advances every stock (including the conveyor
/// stocks) using the pass-computed rates.
///
/// Composed of [`run_phase_a`] (leak + exit over all belts) then
/// [`conveyor_phase_b_one`] per belt (admit + shift + insert). The two halves are
/// public so the queue-conveyor combined pass ([`crate::queue_compile`]) can
/// interleave a coupled queue's serve BETWEEN a conveyor's phase A and phase B
/// (queues.md §9) while a conveyor-only model runs this exact composition,
/// byte-identical to the pre-coupling single-function pass.
pub fn run_pass(
    plans: &[ConveyorPlan],
    states: &mut [ConveyorState],
    curr: &mut [f64],
    dt: f64,
    time: f64,
    start: f64,
    last_unit: &mut i64,
) -> Result<(), (ErrorCode, String)> {
    let pa = run_phase_a(plans, states, curr, dt, time, start, last_unit)?;
    for i in 0..plans.len() {
        conveyor_phase_b_one(i, plans, states, &pa, curr, dt);
    }
    Ok(())
}

/// The integer time unit a modeled clock `time` falls in, for the discrete
/// conveyor per-time-unit `in_limit` budget reset (§6.3).
///
/// The VM advances TIME additively (`next[TIME] = curr[TIME] + dt`), so for a dt
/// that is not exactly representable in binary the running clock accumulates
/// rounding error and sits a few ULPs off each ideal grid time -- ten 0.1 steps
/// sum to 0.999...9, whose `floor` is 0, not 1. Flooring that drifted clock would
/// fire the budget reset one dt late: the step that models t = k.0 would still
/// see the previous unit's exhausted budget and admit nothing, and the admission
/// pulse would land at t ~= k.1. Instead recover the exact step index from the
/// clock -- the accumulated error is far below dt/2, so `(time - start) / dt`
/// rounds back to the true integer step count -- and take the boundary from the
/// ideal grid time `start + k*dt`, formed with a single multiply (correctly
/// rounded to within 0.5 ULP, so e.g. `10 * 0.1` is exactly 1.0). This mirrors
/// [`crate::conveyor`]'s `block_of`, which likewise derives `floor(index * dt)`
/// from an integer index rather than from an accumulated sum.
///
/// At the first step (`time == start`) this returns `floor(start)`, matching the
/// `conveyor_last_unit` seed the VM sets in `run_initials` / `set_conveyor_plans`,
/// so the reset never spuriously fires on step 0.
fn conveyor_time_unit(time: f64, start: f64, dt: f64) -> i64 {
    let k = ((time - start) / dt).round();
    (start + k * dt).floor() as i64
}

/// Phase A over all conveyors (§4.3 steps 0-3): reset the discrete inflow budget
/// at an integer time boundary, then leak + exit each belt from its own
/// start-of-step state, writing the driven-outflow (primary + leak) rates into
/// `curr` for downstream Phase B and stock integration. Returns the per-conveyor
/// [`PhaseAResult`]s (indexed by plan). No phase reads another conveyor's
/// same-phase result, so this is order-free (conveyor chains/cycles need no
/// topological ordering).
///
/// `start` is the run's start time; together with `dt` it lets the boundary
/// detection recover the ideal grid time from the drift-accumulated `time` (see
/// [`conveyor_time_unit`]).
///
/// Errors ([`ErrorCode::ConveyorTransitTooLong`]) when a belt's mid-run
/// `<sample>` re-latch would need more slats than `conveyor::slat_bound()` --
/// checked BEFORE `phase_a` applies the latch, so the belt geometry never grows
/// past the bound (§4.1). The caller aborts the simulation run.
pub fn run_phase_a(
    plans: &[ConveyorPlan],
    states: &mut [ConveyorState],
    curr: &mut [f64],
    dt: f64,
    time: f64,
    start: f64,
    last_unit: &mut i64,
) -> Result<Vec<PhaseAResult>, (ErrorCode, String)> {
    // Discrete per-time-unit in_limit budget resets at integer time boundaries.
    // Derive the boundary from the ideal grid time, not the drift-accumulated
    // clock, so a non-dyadic dt does not fire the reset one dt late.
    let unit = conveyor_time_unit(time, start, dt);
    if unit != *last_unit {
        *last_unit = unit;
        for s in states.iter_mut() {
            s.on_time_boundary();
        }
    }

    // Arrest flags are known before any belt mutates (ordinary expressions,
    // already evaluated this step).
    let arrested: Vec<bool> = plans
        .iter()
        .map(|p| p.arrest_off.map(|o| is_nonzero(curr[o])).unwrap_or(false))
        .collect();

    // Phase A over all conveyors (order-free): leak + exit.
    let mut pa = Vec::with_capacity(plans.len());
    for (i, plan) in plans.iter().enumerate() {
        let fracs: Vec<f64> = plan
            .leaks
            .iter()
            .map(|l| clamp_fraction(curr[l.frac_off], plan.exponential_leak))
            .collect();
        let leak_dest_arrested: Vec<bool> = plan
            .leaks
            .iter()
            .map(|l| l.dest_conveyor.map(|d| arrested[d]).unwrap_or(false))
            .collect();
        let dest_arrested = plan
            .primary_dest_conveyor
            .map(|d| arrested[d])
            .unwrap_or(false);
        // Default <sample> is 1 (re-latch every DT) when the tag is absent.
        let sample = plan.sample_off.map(|o| is_nonzero(curr[o])).unwrap_or(true);
        let transit = clamp_transit(curr[plan.len_off], dt);
        // phase_a re-latches the transit iff the belt is NOT arrested and
        // `sample && transit.is_finite()` (§4.3 steps 0-1); enforce the
        // slat-count bound under exactly that condition, before the latch
        // changes `n_slats()` and phase_b grows the belt (§4.1). An arrested,
        // non-sampling, or non-finite step keeps the prior latched transit,
        // already bounded, so no check is needed.
        if !arrested[i] && sample && transit.is_finite() {
            check_slat_bound(&plan.name, transit, dt)?;
        }
        let r = states[i].phase_a(PhaseAInputs {
            arrested: arrested[i],
            sample,
            transit,
            leak_fractions: &fracs,
            dest_arrested,
            leak_dest_arrested: &leak_dest_arrested,
        });
        // Write the driven-outflow rates for downstream conveyors' Phase B and
        // for stock integration.
        curr[plan.primary_out_off] = r.out_vol / dt;
        for (l, &lv) in plan.leaks.iter().zip(r.leak_vols.iter()) {
            curr[l.flow_off] = lv / dt;
        }
        pa.push(r);
    }
    Ok(pa)
}

/// Phase B for conveyor `i` (§4.3 steps 4-6): admit inflows (conveyor-driven and
/// queue-coupled ones unconditionally, equation-driven ones clamped to
/// capacity/inflow-limit), shift, and insert, writing the admitted
/// equation-driven inflow rates back into `curr`. `pa` is the full Phase A result
/// vector (a `source`-placed inflow mirrors an upstream belt's Phase A leak).
///
/// A queue-coupled inflow (`queue_coupled`, §11) is routed with the
/// conveyor-driven inflows into the unconditional `conv_inflows` path: the
/// combined queue pass has already written its `curr` slot to `served / dt` and
/// debited the discrete inflow budget, so it must NOT be re-clamped or
/// re-quantized as an equation request, and its slot must NOT be overwritten by
/// the write-back below (it carries the shared queue-outflow == conveyor-inflow
/// rate both stocks integrate).
pub fn conveyor_phase_b_one(
    i: usize,
    plans: &[ConveyorPlan],
    states: &mut [ConveyorState],
    pa: &[PhaseAResult],
    curr: &mut [f64],
    dt: f64,
) {
    let plan = &plans[i];
    // The just-latched entry depth `d` (§4.1/§6). `dist`/`source` weight
    // vectors span `0..d`, so they are recomputed here every step -- a
    // time-varying transit changes `d`, hence the placement geometry.
    let d = states[i].entry_depth();
    // Split inflows in listed order into unconditionally-admitted `(volume,
    // placement)` pairs (conveyor-driven chains AND queue-coupled shared flows)
    // and equation-driven rates + their placements. Both unconditional kinds
    // already hold the correct rate in their slot (an upstream Phase-A rate, or
    // the combined queue pass's served rate).
    let mut eq_rates: Vec<f64> = Vec::new();
    let mut placements: Vec<Placement> = Vec::new();
    let mut conv_inflows: Vec<(f64, Placement)> = Vec::new();
    for inf in &plan.inflows {
        if inf.conveyor_driven || inf.queue_coupled {
            let vol = curr[inf.flow_off] * dt;
            conv_inflows.push((vol, conv_inflow_placement(inf, pa, d)));
        } else {
            eq_rates.push(curr[inf.flow_off]);
            placements.push(eq_inflow_placement(inf, d));
        }
    }
    let capacity = plan
        .cap_off
        .map(|o| clamp_cap(curr[o]))
        .unwrap_or(f64::INFINITY);
    let in_limit = plan
        .inlim_off
        .map(|o| clamp_cap(curr[o]))
        .unwrap_or(f64::INFINITY);
    let fracs: Vec<f64> = plan
        .leaks
        .iter()
        .map(|l| clamp_fraction(curr[l.frac_off], plan.exponential_leak))
        .collect();
    let pb = states[i].phase_b(PhaseBInputs {
        phase_a: &pa[i],
        eq_request_rates: &eq_rates,
        conv_inflows: &conv_inflows,
        leak_fractions: &fracs,
        capacity,
        in_limit,
        placements: &placements,
    });
    // Write admitted equation-driven inflow rates back (in listed order;
    // conveyor-driven AND queue-coupled slots already hold the correct rate).
    let mut admitted = pb.in_vols.iter();
    for inf in &plan.inflows {
        if !inf.conveyor_driven
            && !inf.queue_coupled
            && let Some(v) = admitted.next()
        {
            curr[inf.flow_off] = v / dt;
        }
    }
}

/// Compute the admission budget `req = min(cap_room, limit_vol)` a queue directly
/// upstream of conveyor `plan` (belt `state`) may supply this DT (§6.3/§11),
/// reading this conveyor's `<capacity>`/`<in_limit>` from `curr` and sizing
/// against its Phase A result `pa` (freed belt room). The queue-coupled inflow
/// itself is EXCLUDED from `conv_vol` (it is what we are sizing); other
/// conveyor-driven chain inflows are charged. Does NOT mutate belt state; the
/// combined queue pass ([`crate::queue_compile`]) calls this between phase A and
/// phase B, then serves the queue up to `req`.
///
/// `prior_coupled_vol` is the total volume EARLIER-served coupled queues already
/// committed to this same belt this DT, when several queues feed one discrete
/// conveyor (conveyors.md §6.4 rule 1 / §11). Their material has not yet inserted
/// (phase B runs after all coupled serves), so `contents_after` cannot see it;
/// charging it here to the CAPACITY room lets each successive queue size against
/// the room its predecessors took. The per-time-unit inflow-limit side is charged
/// separately -- each serve calls [`ConveyorState::consume_inflow_budget`], which
/// advances `in_carry` so the next `admission_budget` sees the reduced
/// `limit_vol` -- so it must NOT be double-charged here (it feeds only the
/// capacity arm). A single coupled queue passes `0.0` and gets the pre-existing
/// behavior byte-for-byte.
pub fn coupled_admission_budget(
    plan: &ConveyorPlan,
    state: &ConveyorState,
    pa: &PhaseAResult,
    curr: &[f64],
    dt: f64,
    prior_coupled_vol: f64,
) -> f64 {
    let capacity = plan
        .cap_off
        .map(|o| clamp_cap(curr[o]))
        .unwrap_or(f64::INFINITY);
    let in_limit = plan
        .inlim_off
        .map(|o| clamp_cap(curr[o]))
        .unwrap_or(f64::INFINITY);
    let other_conv_vol: f64 = plan
        .inflows
        .iter()
        .filter(|inf| inf.conveyor_driven)
        .map(|inf| curr[inf.flow_off] * dt)
        .sum::<f64>()
        + prior_coupled_vol;
    state.admission_budget(pa, capacity, in_limit, other_conv_vol)
}

#[cfg(test)]
#[path = "conveyor_compile_tests.rs"]
mod tests;
