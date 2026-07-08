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
//! [`crate::queue_compile`] (`build_compiled`/`build_vm`/`build_sim`): a stock
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
/// an empty container, every other reducer -> NaN; `Size` is the element count.
/// `Slat(j)` is 1-based; `j` outside `[1, len]` yields NaN.
///
/// The conveyor pass does NOT use this (its own [`container_value`] reads
/// `ConveyorState::contents`/`belt_len` directly, so conveyor numerics stay
/// byte-identical); the queue pass drives it over `QueueState::batch_contents`,
/// where `total == Σ batch_contents` and `batch_count == batch_contents.len()`
/// hold exactly, so the two agree with the queue's own accessors.
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
    /// §7.2 explicit init list parsed from the stock's `<eqn>` (front first),
    /// `None` for an ordinary §7.1 scalar init. An arrayed conveyor shares one
    /// list across every element belt (the apply-to-all form; per-element
    /// distinct lists are rejected at expansion). The expanded stock's `<eqn>`
    /// was rewritten to a constant placeholder carrying the NORMALIZED belt
    /// total ([`normalized_init_total`]); [`init_belts`] fills the belt from
    /// these values and defensively re-writes the (identical) total into the
    /// stock slot.
    pub init_values: Option<Vec<f64>>,
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

/// Does the named model in `project` contain any conveyor stock? A cheap
/// predicate a caller uses to decide whether to route through the special
/// build path ([`crate::queue_compile::build_vm`]) instead of the ordinary
/// incremental compile.
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
    let spread = model.variables.iter().find_map(|v| match v {
        datamodel::Variable::Flow(f) if canon(&f.ident) == flow => {
            Some(f.compat.spreadflow.clone())
        }
        _ => None,
    });
    let plain = |p: Placement| ResolvedPlacement {
        placement: p,
        dist: None,
        source: false,
    };
    match spread.flatten() {
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

/// A resolved §7.2 explicit init list: the parsed values (front first) plus
/// the constant raw-sum placeholder equation the expanded stock compiles with.
pub(crate) type InitListRewrite = (Vec<f64>, Equation);

/// The scalar-string half of [`InitListRewrite`]: parsed values plus the
/// raw-sum placeholder text (shaped into an [`Equation`] by the caller).
type InitListValues = (Vec<f64>, String);

/// Resolve a conveyor stock's initial equation against the §7.2 explicit-list
/// form. Returns `Ok(Some((values, placeholder)))` when the equation is a
/// list: `values` is the parsed list (front first) for
/// [`ConveyorState::init_explicit`], and `placeholder` is a constant RAW-SUM
/// equation that merely makes the stock's `<eqn>` parseable (a comma list is
/// not a scalar expression). The raw sum is fine for the parse-only salsa
/// diagnostic path (`db::input::datamodel_variable_from_source` -- that
/// equation is never simulated, and the LaTeX surface returns NULL for list
/// stocks rather than rendering it); [`expand_conveyors`] does NOT use this
/// placeholder value -- it swaps in the NORMALIZED belt total from
/// [`normalized_init_total`], because init-time consumers evaluated before
/// `init_belts` read the placeholder as the stock's initial value. `Ok(None)`
/// when the equation is an ordinary §7.1 scalar init. `Err` when the list
/// cannot be used as written: a non-constant entry, or a list inside a
/// per-element `<element>` equation of an arrayed conveyor (per-element
/// DISTINCT lists are not supported; only the scalar and shared apply-to-all
/// forms are -- the same one-expression-per-attribute rule the conveyor block
/// follows, §10). Shared by [`expand_conveyors`] and the diagnostic path so
/// the editor's parse diagnostics accept exactly the lists the runtime
/// accepts.
pub(crate) fn explicit_init_list(
    stock: &str,
    equation: &Equation,
) -> Result<Option<InitListRewrite>, (ErrorCode, String)> {
    // Probe one scalar equation string: the parsed values plus the raw-sum
    // placeholder text, `None` for a non-list, `Err` for a malformed list.
    let scalar = |s: &str| -> Result<Option<InitListValues>, (ErrorCode, String)> {
        match probe_init_list(s) {
            InitListProbe::NotAList => Ok(None),
            InitListProbe::List(values) => {
                let sum: f64 = values.iter().sum();
                if !sum.is_finite() {
                    return Err((
                        ErrorCode::ConveyorInitListUnsupported,
                        format!(
                            "conveyor '{stock}' has an explicit init list whose sum is not \
                             finite; the initial belt contents must be finite"
                        ),
                    ));
                }
                Ok(Some((values, format!("{sum}"))))
            }
            InitListProbe::BadEntry(i, text) => Err((
                ErrorCode::ConveyorInitListUnsupported,
                format!(
                    "conveyor '{stock}' has an explicit init list whose entry {} ('{text}') is \
                     not a plain numeric literal; init-list entries must be plain numeric \
                     literals (they are evaluated once at belt initialization, \
                     docs/design/conveyors.md §7.2)",
                    i + 1
                ),
            )),
        }
    };
    match equation {
        Equation::Scalar(s) => Ok(scalar(s)?.map(|(v, sum)| (v, Equation::Scalar(sum)))),
        Equation::ApplyToAll(dims, s) => {
            Ok(scalar(s)?.map(|(v, sum)| (v, Equation::ApplyToAll(dims.clone(), sum))))
        }
        Equation::Arrayed(_, elems, default, _) => {
            let element_eqns = elems
                .iter()
                .map(|(_, eqn, _, _)| eqn.as_str())
                .chain(default.as_deref());
            for eqn in element_eqns {
                if !matches!(probe_init_list(eqn), InitListProbe::NotAList) {
                    return Err((
                        ErrorCode::ConveyorInitListUnsupported,
                        format!(
                            "arrayed conveyor '{stock}' uses an explicit init list in a \
                             per-element <element> equation; per-element init lists are not \
                             supported -- use a scalar initial per element, or one shared \
                             apply-to-all list for the whole array"
                        ),
                    ));
                }
            }
            Ok(None)
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

        // §7.2 explicit init list on the stock's <eqn>: record the parsed
        // values for belt init and queue the placeholder rewrite for Pass 2.
        // A malformed list (non-constant entry, per-element placement,
        // non-constant transit) is rejected loudly here rather than surfacing
        // as an opaque parse error. The placeholder value is the NORMALIZED
        // belt total, not the raw list sum -- see `normalized_init_total` for
        // why (the raw sum leaked into every pre-init_belts consumer).
        let init_values = match explicit_init_list(&stock.ident, &stock.equation)? {
            Some((values, _raw_sum_placeholder)) => {
                let dt = sim_specs_dt(effective_sim_specs(&project, main_model))?;
                let total = normalized_init_total(&stock.ident, conv, &values, dt)?;
                let placeholder = match &stock.equation {
                    Equation::ApplyToAll(dims, _) => {
                        Equation::ApplyToAll(dims.clone(), format!("{total}"))
                    }
                    _ => Equation::Scalar(format!("{total}")),
                };
                init_list_rewrites.insert(stock_name.clone(), placeholder);
                Some(values)
            }
            None => None,
        };

        // An arrayed conveyor is N_elem independent belts (§10). The stock's
        // dimensions drive the synthesized auxes' shape and the per-element
        // offset enumeration; empty for a scalar conveyor.
        let stock_dims = equation_dims(&stock.equation);
        conveyor_dims.insert(stock_name.clone(), stock_dims.len());
        conveyor_stock_dims.insert(stock_name.clone(), stock_dims.clone());
        let element_subscripts =
            element_subscripts_for_dims(&project, &stock_dims, &stock.ident, "conveyor")?;

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
                // Every element belt of an arrayed conveyor shares the one
                // apply-to-all list (§10 one-expression-per-attribute).
                init_values: meta.init_values.clone(),
            });
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
        // `<` keeps the smaller i (nearer the exit) on an exact tie.
        let mut best_i = 0usize;
        let mut best_dist = f64::INFINITY;
        for i in 0..d {
            let x_i = 1.0 - (i as f64 + 0.5) / d as f64;
            let dist = (x_i - y).abs();
            if dist < best_dist {
                best_dist = dist;
                best_i = i;
            }
        }
        weights[best_i] += q;
    }
    weights
}

/// Find the upstream conveyor plan index and leak index whose leak flow occupies
/// data-buffer slot `flow_off` -- the flow-identity coupling a `source` inflow
/// uses (§8). `None` when the slot is not an upstream leak (e.g. it is a primary
/// outflow, or an ordinary flow), in which case `source` falls back to
/// `Beginning`.
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
/// mirrors the matched upstream leak's per-slat leakage; `dist` samples its
/// profile; everything else uses the static placement.
fn conv_inflow_placement(
    inf: &InflowPlan,
    plans: &[ConveyorPlan],
    pa: &[PhaseAResult],
    d: usize,
) -> Placement {
    if inf.source {
        return match find_upstream_leak(plans, inf.flow_off) {
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

/// Compute one container-access result from a belt's current (start-of-step)
/// state (§10). The reducer conventions match the VM's array reducers (`vm.rs`):
/// `Sum` -> 0 on an empty belt, every other reducer -> NaN; `Size` is the
/// physical belt length. `Slat(j)` is 1-based from the exit; `j` outside
/// `[1, L]` yields NaN.
fn container_value(state: &ConveyorState, kind: &ContainerKind) -> f64 {
    match kind {
        ContainerKind::Slat(j) => {
            if *j < 1 {
                f64::NAN
            } else {
                state.slat_content(*j - 1).unwrap_or(f64::NAN)
            }
        }
        ContainerKind::Size => state.belt_len() as f64,
        ContainerKind::Sum => state.contents(),
        ContainerKind::Mean | ContainerKind::Min | ContainerKind::Max | ContainerKind::Stddev => {
            let slats = state.slat_contents();
            if slats.is_empty() {
                return f64::NAN;
            }
            match kind {
                ContainerKind::Mean => slats.iter().sum::<f64>() / slats.len() as f64,
                ContainerKind::Min => slats.iter().copied().fold(f64::INFINITY, f64::min),
                ContainerKind::Max => slats.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                ContainerKind::Stddev => {
                    let n = slats.len() as f64;
                    let mean = slats.iter().sum::<f64>() / n;
                    let var = slats.iter().map(|v| (v - mean).powf(2.0)).sum::<f64>() / n;
                    var.sqrt()
                }
                _ => unreachable!("outer match restricts kind"),
            }
        }
    }
}

/// Publish each conveyor's container-access results into their data-buffer slots
/// (§10). Called at STEP-START -- before the Flows phase in the Euler loop and
/// after belt initialization in `run_initials` -- so the published values
/// reflect the belt as left by the previous step (= start-of-step for this
/// step). Each container variable is a hidden no-flow STOCK, so the Flows phase
/// never recomputes its slot and the Stocks phase leaves it unchanged: the value
/// is visible to Flows-phase readers and survives the whole step.
pub fn publish_container_values(
    plans: &[ConveyorPlan],
    states: &[ConveyorState],
    curr: &mut [f64],
) {
    for (plan, state) in plans.iter().zip(states.iter()) {
        for c in &plan.containers {
            curr[c.off] = container_value(state, &c.kind);
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
            conv_inflows.push((vol, conv_inflow_placement(inf, plans, pa, d)));
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
mod tests {
    use super::*;
    use crate::common::Ident;
    // The build entry points live in queue_compile (the unified conveyor+queue
    // build path); these tests pin the conveyor half of its behavior.
    use crate::queue_compile::{build_compiled, build_vm};
    use std::io::BufReader;

    fn parse(xml: &str) -> datamodel::Project {
        crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap()
    }

    #[test]
    fn minimal_conveyor_simulates_steady_state() {
        // init Students=1000 == inflow(250) * transit(4) == steady state, so the
        // whole run should hold flat: Students=1000, graduating=250, and Alumni
        // accumulates 250/time.
        let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build conveyor vm");
        vm.run_to_end().expect("run");

        let students = vm
            .get_series(&Ident::new("students"))
            .expect("students series");
        let graduating = vm
            .get_series(&Ident::new("graduating"))
            .expect("graduating series");
        let alumni = vm.get_series(&Ident::new("alumni")).expect("alumni series");
        assert!(students.len() > 40, "should have many saved steps");
        for (i, &s) in students.iter().enumerate() {
            assert!(
                (s - 1000.0).abs() < 1e-6,
                "step {i}: Students={s} (want 1000)"
            );
        }
        // graduating is a during-step flow rate; steady at 250.
        for (i, &g) in graduating.iter().enumerate().skip(1) {
            assert!(
                (g - 250.0).abs() < 1e-6,
                "step {i}: graduating={g} (want 250)"
            );
        }
        // Alumni accumulates the outflow: rises monotonically to 250*12 = 3000.
        assert!(
            (alumni[alumni.len() - 1] - 3000.0).abs() < 1.0,
            "final Alumni {}",
            alumni[alumni.len() - 1]
        );
    }

    #[test]
    fn fill_from_empty_is_a_transit_delay() {
        // Same model but Students starts empty: outflow stays 0 until the belt
        // fills (transit=4), then equals the inflow (pure T-unit delay, S2).
        let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile")
            .replace("<eqn>1000</eqn>", "<eqn>0</eqn>");
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let students = vm.get_series(&Ident::new("students")).expect("students");
        let graduating = vm
            .get_series(&Ident::new("graduating"))
            .expect("graduating");
        // dt=0.25, transit=4 -> 16 slats; the first inflow inserted at t=0 exits
        // at t=4.0, i.e. the outflow is 0 for the first 16 steps.
        assert_eq!(graduating[0], 0.0);
        for (i, &g) in graduating.iter().enumerate().take(16) {
            assert!(
                g.abs() < 1e-9,
                "step {i}: outflow should be 0 during fill, got {g}"
            );
        }
        // once full, outflow reaches the inflow rate.
        assert!(
            (graduating[20] - 250.0).abs() < 1e-6,
            "step 20 outflow {}",
            graduating[20]
        );
        assert!(
            (students[16] - 1000.0).abs() < 1e-6,
            "belt full at step 16: {}",
            students[16]
        );
    }

    fn wrap_model(vars: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>20</stop><dt>0.25</dt></sim_specs>
  <model><variables>{vars}</variables></model>
</xmile>"#
        )
    }

    #[test]
    fn capacity_plateaus_contents() {
        // S5: capacity=600, inflow 250, transit 4 -> contents plateau at 600.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len><capacity>600</capacity></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let belt = vm.get_series(&Ident::new("belt")).expect("belt");
        for (i, &b) in belt.iter().enumerate() {
            assert!(b <= 600.0 + 1e-6, "step {i}: contents {b} exceeds capacity");
        }
        assert!(
            (belt[belt.len() - 1] - 600.0).abs() < 1e-6,
            "plateaus at 600: {}",
            belt[belt.len() - 1]
        );
    }

    #[test]
    fn linear_leak_reaches_steady_state() {
        // S3: linear leak f=0.2, inflow 250, transit 4 -> steady outflow 200,
        // leak 50 (init empty, run long enough to settle).
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
        let leak = vm.get_series(&Ident::new("attriting")).expect("attriting");
        let last = out.len() - 1;
        assert!(
            (out[last] - 200.0).abs() < 1e-4,
            "steady outflow {} (want 200)",
            out[last]
        );
        assert!(
            (leak[last] - 50.0).abs() < 1e-4,
            "steady leak {} (want 50)",
            leak[last]
        );
    }

    /// The leak model shared by the GH #871 override tests: primary outflow
    /// `out_f`, leak `attriting` (f=0.2), plus a container-access aux so a
    /// synthesized container stock (`$conv$sum$belt`) exists.
    fn override_leak_model() -> String {
        wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/></flow>
        <aux name="on_belt"><eqn>SUM(belt)</eqn></aux>"#,
        )
    }

    #[test]
    fn set_value_on_pass_driven_conveyor_slots_rejected() {
        // GH #871: expansion rewrites every conveyor-driven flow to a
        // placeholder `0`, which compiles to an overridable AssignConstCurr.
        // Without the pass-driven exclusion, set_value on the primary outflow
        // or a leak silently succeeds -- but the conveyor pass overwrites the
        // slot every step, so the override never affects the simulation. It
        // must instead be rejected like any other computed flow. The container
        // stock's slot is pass-published too and is rejected for the same
        // reason (a stock was never overridable; pinning it here keeps the
        // "every pass-written slot rejects" invariant explicit).
        let project = parse(&override_leak_model());
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");

        for name in ["out_f", "attriting", "$conv$sum$belt"] {
            let err = vm.set_value(&Ident::new(name), 999.0).unwrap_err();
            assert_eq!(
                err.code,
                ErrorCode::BadOverride,
                "set_value('{name}') must be rejected"
            );
        }

        // The rejected overrides leave no trace: the run is belt-driven
        // (S3 steady state: out 200, leak 50) and get_series reports it.
        vm.run_to_end().expect("run");
        let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
        let leak = vm.get_series(&Ident::new("attriting")).expect("attriting");
        let last = out.len() - 1;
        assert!(
            (out[last] - 200.0).abs() < 1e-4,
            "steady outflow {} (want 200)",
            out[last]
        );
        assert!(
            (leak[last] - 50.0).abs() < 1e-4,
            "steady leak {} (want 50)",
            leak[last]
        );
    }

    #[test]
    fn set_value_on_equation_driven_inflow_remains_effective() {
        // An equation-driven inflow is a genuine pass INPUT: the Flows phase
        // computes the requested rate and the pass admits against it (writing
        // back only the admitted rate). A constant-inflow override must
        // therefore stay accepted AND change the simulation -- this pins that
        // the GH #871 rejection covers only pass-WRITTEN slots, not pass
        // inputs. With in_f overridden to 100 and leak f=0.2, the S3 steady
        // state scales to out 80 / leak 20.
        let project = parse(&override_leak_model());
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.set_value(&Ident::new("in_f"), 100.0)
            .expect("inflow override must be accepted");
        vm.run_to_end().expect("run");
        let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
        let leak = vm.get_series(&Ident::new("attriting")).expect("attriting");
        let last = out.len() - 1;
        assert!(
            (out[last] - 80.0).abs() < 1e-4,
            "steady outflow {} (want 80 = 100 * 0.8)",
            out[last]
        );
        assert!(
            (leak[last] - 20.0).abs() < 1e-4,
            "steady leak {} (want 20 = 100 * 0.2)",
            leak[last]
        );
    }

    #[test]
    fn directly_assembled_vm_rejects_pass_driven_overrides() {
        // Defense-in-depth for GH #871: a Vm assembled by hand from an
        // UNSCRUBBED CompiledSimulation (bypassing build_compiled's exclusion)
        // must still reject pass-driven slots, because set_conveyor_plans
        // itself retracts them from the overridable-constant set.
        let project = parse(&override_leak_model());
        let main = project.models[0].name.clone();
        let (expanded, metas) = expand_conveyors(&project, &main).expect("expand");
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &expanded, None);
        let compiled =
            crate::db::compile_project_incremental(&db, sync.project, &main).expect("compile");
        let plans = resolve_plans(&metas, &compiled.offsets).expect("resolve");
        let mut vm = crate::vm::Vm::new(compiled).expect("vm");
        vm.set_conveyor_plans(plans);
        let err = vm.set_value(&Ident::new("out_f"), 999.0).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadOverride);
        let err = vm.set_value(&Ident::new("attriting"), 999.0).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadOverride);
    }

    #[test]
    fn duplicate_canonical_leak_flow_expands_without_panic() {
        // Two flows canonicalize to the same ident ('Attrition'/'attrition');
        // the XMILE reader stable-sorts by canonical ident but never dedups,
        // so the leak-LESS twin sits first in document order. Detecting
        // leak-ness with an any() scan over every same-canonical flow but then
        // fetching the FIRST canon-matching flow and unwrapping its leakage
        // panicked here (GH #870); the fetch must select the same flow that
        // made the outflow a leak.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>Attrition</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="Attrition"><eqn>1</eqn></flow>
        <flow name="attrition"><eqn>0.2</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let (expanded, metas) = expand_conveyors(&project, &main).expect("expand");
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].leaks.len(), 1, "outflow must be treated as a leak");
        assert_eq!(metas[0].leaks[0].flow, "attrition");
        // The synthesized fraction aux must come from the leak-carrying twin's
        // equation (0.2), never the leak-less twin's (1).
        let frac_aux = leak_frac_name("attrition");
        let frac_eqn = expanded.models[0].variables.iter().find_map(|v| match v {
            datamodel::Variable::Aux(a) if a.ident == frac_aux => Some(a.equation.clone()),
            _ => None,
        });
        assert_eq!(frac_eqn, Some(Equation::Scalar("0.2".to_string())));
    }

    #[test]
    fn duplicate_canonical_leak_flow_selects_leak_twin_regardless_of_order() {
        // Same duplicate pair with the leak-carrying twin FIRST in document
        // order. This ordering never panicked, but it must resolve to the
        // identical interpretation as the reversed ordering above: the leak
        // twin supplies the fraction, whichever side of the duplicate it is.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>Attrition</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attrition"><eqn>0.2</eqn><leak/></flow>
        <flow name="Attrition"><eqn>1</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let (expanded, metas) = expand_conveyors(&project, &main).expect("expand");
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].leaks.len(), 1, "outflow must be treated as a leak");
        assert_eq!(metas[0].leaks[0].flow, "attrition");
        let frac_aux = leak_frac_name("attrition");
        let frac_eqn = expanded.models[0].variables.iter().find_map(|v| match v {
            datamodel::Variable::Aux(a) if a.ident == frac_aux => Some(a.equation.clone()),
            _ => None,
        });
        assert_eq!(frac_eqn, Some(Equation::Scalar("0.2".to_string())));
    }

    #[test]
    fn unexpanded_conveyor_rejected_by_ordinary_compile() {
        // A conveyor model compiled through the ordinary (non-conveyor) path
        // must fail loudly rather than silently integrate the belt as a plain
        // stock. This is the production-path safety guard.
        let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
        let project = parse(xml);
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let main = project.models[0].name.clone();
        let err = crate::db::compile_project_incremental(&db, sync.project, &main)
            .expect_err("un-expanded conveyor must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorNotExpanded);
    }

    #[test]
    fn even_placement_sends_material_to_exit_immediately() {
        // With `even`, an inflow lands A/d at every entry-path slat INCLUDING
        // the exit slat, so material exits on the first step -- unlike the
        // default `beginning` (0 outflow until the belt fills). Steady outflow
        // still equals the inflow (mass conservation, independent of placement).
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f" isee:spreadflow="even"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build even");
        vm.run_to_end().expect("run");
        let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
        assert!(
            out[1] > 0.0,
            "even: material should exit immediately, out[1]={}",
            out[1]
        );
        assert!(
            (out[out.len() - 1] - 250.0).abs() < 1e-4,
            "steady outflow {}",
            out[out.len() - 1]
        );
    }

    #[test]
    fn dist_without_representable_distribution_is_rejected() {
        // `dist` whose <isee:distrib_eq> is empty, an inline expression, or a
        // dangling reference has no representable distribution -- rejected loudly
        // (a silent Beginning fallback would hide a modeling error). `source`,
        // by contrast, is NOT rejected: on an equation-driven inflow it simply
        // degrades to Beginning (there is no upstream leak to mirror).
        for distrib in ["", "in_f * 2", "not_a_variable"] {
            let distrib_tag = if distrib.is_empty() {
                String::new()
            } else {
                format!("<isee:distrib_eq>{distrib}</isee:distrib_eq>")
            };
            let xml = wrap_model(&format!(
                r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f" isee:spreadflow="dist"><eqn>250</eqn>{distrib_tag}</flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#
            ));
            let project = parse(&xml);
            let main = project.models[0].name.clone();
            let err = build_vm(&project, &main)
                .err()
                .unwrap_or_else(|| panic!("dist '{distrib}' should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorSpreadflowUnsupported,
                "distrib '{distrib}'"
            );
        }
    }

    // ----- dist / source weight computation (§8), pure-function oracles -----

    #[test]
    fn dist_weights_array_indexes_by_floor_x_times_m() {
        // d=2 entry path: x_0 = 0.75 (exit slat), x_1 = 0.25 (entry slat).
        // floor(x*m) with m=2: floor(1.5)=1, floor(0.5)=0. So the exit slat reads
        // a[1] and the entry slat reads a[0].
        let profile = DistProfile::Array(vec![10.0, 0.0]);
        let w = dist_weights(&profile, 2);
        assert_eq!(w, vec![0.0, 10.0]);
    }

    #[test]
    fn dist_weights_gf_samples_and_clamps_negatives() {
        // g(x) = 2x - 1 over [0,1]; d=2 -> x_0=0.75 -> 0.5, x_1=0.25 -> -0.5,
        // clamped to 0 by the max(0, .) rule (§8).
        let profile = DistProfile::Gf(vec![(0.0, -1.0), (1.0, 1.0)]);
        let w = dist_weights(&profile, 2);
        assert!((w[0] - 0.5).abs() < 1e-12, "exit weight {}", w[0]);
        assert_eq!(w[1], 0.0, "entry weight clamped to 0");
    }

    #[test]
    fn dist_weights_empty_array_is_all_zero_fallback() {
        // All-zero weights make Placement::Dist fall back to Beginning.
        let w = dist_weights(&DistProfile::Array(vec![]), 3);
        assert_eq!(w, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn source_weights_equal_belts_mirror_positions() {
        // Equal belts (L_up = d = 4): each upstream slat maps to the same-index
        // target slat, so the mirror is the identity and conserves the total.
        let up = vec![1.0, 0.0, 0.0, 2.0];
        let w = source_weights(&up, 4);
        assert_eq!(w, vec![1.0, 0.0, 0.0, 2.0]);
        assert!((w.iter().sum::<f64>() - up.iter().sum::<f64>()).abs() < 1e-12);
    }

    #[test]
    fn source_weights_different_belts_mirror_proportionally_ties_to_exit() {
        // L_up=2 -> y_0=0.75, y_1=0.25. d=4 -> x=[0.875,0.625,0.375,0.125].
        // y_0 ties between i=0 and i=1 (both 0.125 away) -> exit side i=0;
        // y_1 ties between i=2 and i=3 -> exit side i=2.
        let up = vec![3.0, 5.0];
        let w = source_weights(&up, 4);
        assert_eq!(w, vec![3.0, 0.0, 5.0, 0.0]);
        assert!((w.iter().sum::<f64>() - 8.0).abs() < 1e-12);
    }

    #[test]
    fn source_weights_empty_inputs_are_all_zero() {
        assert_eq!(source_weights(&[], 3), vec![0.0, 0.0, 0.0]);
        assert_eq!(source_weights(&[1.0, 2.0], 0), Vec::<f64>::new());
    }

    // ----- compile-time spec advisories (§4.1 / §5.1 pure helpers) -----

    #[test]
    fn const_scalar_expr_accepts_literals_and_signs() {
        assert_eq!(const_scalar_expr("1.3"), Some(1.3));
        assert_eq!(const_scalar_expr(" 4 "), Some(4.0));
        assert_eq!(const_scalar_expr("-2.5"), Some(-2.5));
        assert_eq!(const_scalar_expr("+0.5"), Some(0.5));
        // Parentheses vanish at parse; the inner literal is still a constant.
        assert_eq!(const_scalar_expr("(1.5)"), Some(1.5));
    }

    #[test]
    fn const_scalar_expr_rejects_runtime_expressions() {
        // A variable reference, arithmetic, a builtin, an empty equation, and
        // a parse error are all runtime/unknowable -- no compile-time value,
        // so the advisories that consume this must stay silent (§4.1/§5.1).
        assert_eq!(const_scalar_expr("transit_var"), None);
        assert_eq!(const_scalar_expr("1 + 2"), None);
        assert_eq!(const_scalar_expr("TIME"), None);
        assert_eq!(const_scalar_expr("MAX(1, 2)"), None);
        assert_eq!(const_scalar_expr(""), None);
        assert_eq!(const_scalar_expr("1 +"), None);
    }

    #[test]
    fn transit_dt_mismatch_flags_non_multiples() {
        // The §4.1 example: 1.3 / 0.25 = 5.2 rounds half-away to 5 slats, an
        // effective transit of 1.25.
        assert_eq!(transit_dt_mismatch(1.3, 0.25), Some((5, 1.25)));
        // Half-away rounding mirror of slat_count: 1.5 / 1 rounds UP to 2.
        assert_eq!(transit_dt_mismatch(1.5, 1.0), Some((2, 2.0)));
        // A sub-dt transit clamps to one slat (slat_count's >= 1 clamp), so
        // the effective transit is a full dt.
        assert_eq!(transit_dt_mismatch(0.1, 0.25), Some((1, 0.25)));
    }

    #[test]
    fn transit_dt_mismatch_exact_multiples_are_clean() {
        assert_eq!(transit_dt_mismatch(4.0, 0.25), None);
        assert_eq!(transit_dt_mismatch(1.0, 1.0), None);
        // Binary-representation noise stays below the 1e-9 ratio tolerance:
        // 0.3 / 0.1 is 2.9999999999999996 in f64 but an exact multiple in
        // intent, and must not warn.
        assert_eq!(transit_dt_mismatch(0.3, 0.1), None);
    }

    #[test]
    fn leak_fraction_source_resolves_the_two_encodings_in_runtime_order() {
        let leak = |fraction: Option<&str>| datamodel::Leakage {
            fraction: fraction.map(str::to_string),
            integers: false,
            zone_start: None,
            zone_end: None,
        };
        let scalar = |s: &str| Equation::Scalar(s.to_string());

        // A non-empty explicit `<leak>` fraction wins over the flow's <eqn>.
        assert_eq!(
            leak_fraction_source(Some(&leak(Some("0.7"))), &scalar("0.5")),
            LeakFractionSource::Explicit("0.7")
        );
        // A bare `<leak/>` marker (fraction None) falls back to the flow's
        // own <eqn> -- the encoding real Stella files use (§3.3).
        let eqn = scalar("0.5");
        assert_eq!(
            leak_fraction_source(Some(&leak(None)), &eqn),
            LeakFractionSource::FlowEquation(&eqn)
        );
        // An EMPTY explicit fraction is treated like an absent one.
        assert_eq!(
            leak_fraction_source(Some(&leak(Some(""))), &eqn),
            LeakFractionSource::FlowEquation(&eqn)
        );
        // A truly bare marker on an empty-equation flow: no fraction at all.
        assert_eq!(
            leak_fraction_source(Some(&leak(None)), &scalar("")),
            LeakFractionSource::Absent
        );
        // A per-element equation always carries fractions.
        let arrayed = Equation::Arrayed(vec!["d1".to_string()], vec![], None, false);
        assert_eq!(
            leak_fraction_source(Some(&leak(None)), &arrayed),
            LeakFractionSource::FlowEquation(&arrayed)
        );
    }

    #[test]
    fn transit_dt_mismatch_out_of_domain_inputs_are_other_diagnostics_jobs() {
        // Non-positive / non-finite transit is ConveyorTransitNotPositive's
        // domain at the runtime latch; an invalid dt is a sim-specs problem.
        assert_eq!(transit_dt_mismatch(0.0, 0.25), None);
        assert_eq!(transit_dt_mismatch(-1.0, 0.25), None);
        assert_eq!(transit_dt_mismatch(f64::INFINITY, 0.25), None);
        assert_eq!(transit_dt_mismatch(f64::NAN, 0.25), None);
        assert_eq!(transit_dt_mismatch(4.0, 0.0), None);
        assert_eq!(transit_dt_mismatch(4.0, -0.25), None);
        assert_eq!(transit_dt_mismatch(4.0, f64::NAN), None);
    }

    // ----- end-to-end dist / source placement -----

    #[test]
    fn dist_placement_end_to_end_sends_exit_weighted_inflow_out_early() {
        // profile g(x) = x concentrates weight near the exit (x -> 1), so some
        // admitted material lands close to the exit and leaves within a few DTs,
        // unlike the default `beginning` (0 outflow until the belt fills at
        // transit=4). Steady outflow still equals the inflow (conservation,
        // independent of placement).
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f" isee:spreadflow="dist"><eqn>250</eqn><isee:distrib_eq>profile</isee:distrib_eq></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="profile"><eqn>0+0</eqn><gf><xscale min="0" max="1"/><yscale min="0" max="1"/><ypts>0,0.25,0.5,0.75,1</ypts></gf></aux>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build dist");
        vm.run_to_end().expect("run");
        let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
        let belt = vm.get_series(&Ident::new("belt")).expect("belt");
        assert!(
            out[2] > 0.0,
            "dist(exit-weighted): material should exit early, out[2]={}",
            out[2]
        );
        for (i, &b) in belt.iter().enumerate() {
            assert!(b.is_finite() && b >= -1e-9, "step {i}: belt {b}");
        }
        assert!(
            (out[out.len() - 1] - 250.0).abs() < 1e-4,
            "steady outflow {}",
            out[out.len() - 1]
        );
    }

    #[test]
    fn source_placement_end_to_end_mirrors_upstream_leak() {
        // `leaking` is a linear leak of the upstream conveyor AND the inflow of
        // the downstream conveyor: the downstream admits it (conveyor-driven,
        // never blocked). At steady state the upstream leaks 0.2*250=50/time, so
        // whatever the placement the downstream (transit 4, no leak) settles to
        // outflow 50 (conservation). The placement geometry is what differs:
        // `beginning` deposits every leaked unit at the entry, so it traverses
        // the full transit and contents settle to 50*4=200; `source` mirrors the
        // upstream leak's slat positions, so material enters SHALLOWER than the
        // entry and exits sooner -- strictly lower steady contents.
        let model = |spread: &str| {
            wrap_model(&format!(
                r#"
        <stock name="up"><eqn>1000</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>leaking</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="src_in"><eqn>250</eqn></flow>
        <flow name="up_out"><eqn>0</eqn></flow>
        <flow name="leaking"{spread}><eqn>0.2</eqn><leak/></flow>
        <stock name="down"><eqn>0</eqn><inflow>leaking</inflow><outflow>down_out</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="down_out"><eqn>0</eqn></flow>"#
            ))
        };
        let run = |xml: &str| -> (f64, f64) {
            let project = parse(xml);
            let main = project.models[0].name.clone();
            let mut vm = build_vm(&project, &main).expect("build");
            vm.run_to_end().expect("run");
            let down = vm.get_series(&Ident::new("down")).expect("down");
            let down_out = vm.get_series(&Ident::new("down_out")).expect("down_out");
            for (i, &b) in down.iter().enumerate() {
                assert!(b.is_finite() && b >= -1e-9, "step {i}: down {b}");
            }
            (down[down.len() - 1], down_out[down_out.len() - 1])
        };

        let (begin_contents, begin_out) = run(&model(""));
        let (source_contents, source_out) = run(&model(r#" isee:spreadflow="source""#));

        // Both conserve to the 50/time leak inflow.
        assert!(
            (begin_out - 50.0).abs() < 1e-3 && (source_out - 50.0).abs() < 1e-3,
            "steady outflows begin={begin_out} source={source_out} (want 50)"
        );
        // beginning fills the full transit to 200; source enters shallower.
        assert!(
            (begin_contents - 200.0).abs() < 1e-2,
            "beginning steady contents {begin_contents} (want 200)"
        );
        assert!(
            source_contents > 0.0 && source_contents < begin_contents - 1.0,
            "source contents {source_contents} should be positive and strictly \
             below beginning's {begin_contents}"
        );
    }

    #[test]
    fn leak_into_arrested_conveyor_is_skipped_no_stock_belt_divergence() {
        // F6 regression (§4.3 step 2): conveyor `up` leaks flow `leaking` into
        // conveyor `down` (`<inflow>leaking</inflow>` on down). While `down` is
        // arrested the leak into it must be SKIPPED entirely (rate 0, `up` keeps
        // the material). If it is not, `up` keeps leaking, the ordinary Stocks
        // phase adds that rate to `down`'s stock slot, but `down`'s frozen belt
        // never admits it -- so the reported stock permanently climbs above the
        // true belt content (SUM(down)).
        let model = |arrest: &str| {
            wrap_model(&format!(
                r#"
        <stock name="up"><eqn>1000</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>leaking</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="src_in"><eqn>250</eqn></flow>
        <flow name="up_out"><eqn>0</eqn></flow>
        <flow name="leaking"><eqn>0.2</eqn><leak/></flow>
        <stock name="down"><eqn>0</eqn><inflow>leaking</inflow><outflow>down_out</outflow>
          <conveyor><len>4</len>{arrest}</conveyor></stock>
        <flow name="down_out"><eqn>0</eqn></flow>
        <aux name="down_belt"><eqn>SUM(down)</eqn></aux>
        <aux name="up_belt"><eqn>SUM(up)</eqn></aux>"#
            ))
        };
        let run = |xml: &str| -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>) {
            let project = parse(xml);
            let main = project.models[0].name.clone();
            let mut vm = build_vm(&project, &main).expect("build");
            vm.run_to_end().expect("run");
            (
                vm.get_series(&Ident::new("down")).expect("down"),
                vm.get_series(&Ident::new("down_belt")).expect("down_belt"),
                vm.get_series(&Ident::new("leaking")).expect("leaking"),
                vm.get_series(&Ident::new("up_belt")).expect("up_belt"),
            )
        };

        // Arrest `down` for t in [5, 8): STEP(1,5) - STEP(1,8) == 1 over that
        // window. dt = 0.25, so those are steps [20, 32).
        let (down, down_belt, leaking, up_belt) =
            run(&model(r#"<arrest>STEP(1, 5) - STEP(1, 8)</arrest>"#));
        // Baseline: `down` never arrested (so `up` leaks the whole run).
        let (_bd, _bdb, _bl, base_up_belt) = run(&model(""));

        // The invariant the bug breaks: a conveyor's reported stock equals its
        // true belt content at EVERY step (conservation). The leak-into-arrested
        // bug makes `down` climb above SUM(down) during and after the arrest.
        for (i, (&s, &b)) in down.iter().zip(down_belt.iter()).enumerate() {
            assert!(
                (s - b).abs() < 1e-6,
                "step {i}: down stock {s} diverged from belt {b}"
            );
        }
        // During arrest the leak into `down` is skipped entirely (rate 0), so `up`
        // does not shed it (the material stays on up's belt to advance normally).
        // `i` is the semantic step index (arrest window t in [5, 8) == steps 20..32).
        #[allow(clippy::needless_range_loop)]
        for i in 20..32 {
            assert!(
                leaking[i].abs() < 1e-9,
                "step {i} (t={}): leaking={} should be 0 while down arrested",
                i as f64 * 0.25,
                leaking[i]
            );
        }
        // ... and resumes once `down` is released (t = 10, step 40).
        assert!(
            leaking[40] > 10.0,
            "step 40: leaking={} should resume after release",
            leaking[40]
        );
        // `up` retains the material it did NOT shed: at the last arrested step its
        // belt holds strictly more than the never-arrested baseline.
        assert!(
            up_belt[31] > base_up_belt[31] + 1.0,
            "up should retain un-leaked material: arrest {} vs baseline {}",
            up_belt[31],
            base_up_belt[31]
        );
    }

    #[test]
    fn leak_dest_conveyor_meta_mirrors_primary_linkage() {
        // The leak's destination-conveyor linkage is resolved exactly like the
        // primary's (§4.3 step 2): a leak feeding a downstream conveyor records
        // that conveyor's stock name (so the runtime can skip it while the
        // destination is arrested); a leak to a cloud records None.
        let xml = wrap_model(
            r#"
        <stock name="up"><eqn>0</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>up_leak</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="src_in"><eqn>250</eqn></flow>
        <flow name="up_out"><eqn>0</eqn></flow>
        <flow name="up_leak"><eqn>0.2</eqn><leak/></flow>
        <stock name="down"><eqn>0</eqn><inflow>up_leak</inflow><outflow>down_out</outflow><outflow>down_drain</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="down_out"><eqn>0</eqn></flow>
        <flow name="down_drain"><eqn>0.1</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let (_expanded, metas) = expand_conveyors(&project, &main).expect("expand");
        let up = metas.iter().find(|m| m.stock == "up").expect("up meta");
        let down = metas.iter().find(|m| m.stock == "down").expect("down meta");
        // up's leak feeds `down` (a conveyor) -> recorded.
        assert_eq!(up.leaks.len(), 1);
        assert_eq!(up.leaks[0].dest_conveyor.as_deref(), Some("down"));
        // down's leak feeds a cloud (no conveyor lists it as an inflow) -> None.
        assert_eq!(down.leaks.len(), 1);
        assert_eq!(down.leaks[0].dest_conveyor, None);
    }

    #[test]
    fn self_leak_flow_is_not_its_own_arrested_dest() {
        // A leak flow that also feeds its OWN conveyor records no destination (the
        // same `owner != stock` self-loop filter the primary uses): an arrested
        // conveyor never leaks at all, so a self-leak can never hold against its
        // own arrest.
        let xml = wrap_model(
            r#"
        <stock name="a"><eqn>0</eqn><inflow>a_in</inflow><inflow>a_leak</inflow><outflow>a_out</outflow><outflow>a_leak</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="a_in"><eqn>250</eqn></flow>
        <flow name="a_out"><eqn>0</eqn></flow>
        <flow name="a_leak"><eqn>0.1</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let (_expanded, metas) = expand_conveyors(&project, &main).expect("expand");
        let a = metas.iter().find(|m| m.stock == "a").expect("a meta");
        assert_eq!(a.leaks.len(), 1);
        assert_eq!(
            a.leaks[0].dest_conveyor, None,
            "a self-leak must not link to its own conveyor"
        );
    }

    #[test]
    fn leak_to_cloud_keeps_flowing_while_another_conveyor_is_arrested() {
        // No false positive: a leak to a cloud (no downstream conveyor) must NOT
        // be skipped just because some OTHER conveyor in the model is arrested --
        // the skip is keyed on the leak's OWN destination (§4.3 step 2).
        let xml = wrap_model(
            r#"
        <stock name="up"><eqn>1000</eqn><inflow>up_in</inflow><outflow>up_out</outflow><outflow>up_leak</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="up_in"><eqn>250</eqn></flow>
        <flow name="up_out"><eqn>0</eqn></flow>
        <flow name="up_leak"><eqn>0.2</eqn><leak/></flow>
        <stock name="other"><eqn>1000</eqn><inflow>other_in</inflow><outflow>other_out</outflow>
          <conveyor><len>4</len><arrest>STEP(1, 5) - STEP(1, 8)</arrest></conveyor></stock>
        <flow name="other_in"><eqn>250</eqn></flow>
        <flow name="other_out"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let up_leak = vm.get_series(&Ident::new("up_leak")).expect("up_leak");
        // `other` is arrested for steps 20..32; `up_leak` goes to a cloud, so it
        // keeps flowing throughout. `i` is the semantic step index.
        #[allow(clippy::needless_range_loop)]
        for i in 20..32 {
            assert!(
                up_leak[i] > 10.0,
                "step {i}: up_leak={} should keep flowing (cloud dest, not arrested)",
                up_leak[i]
            );
        }
    }

    #[test]
    fn source_on_non_leak_inflow_falls_back_to_beginning() {
        // `source` on an ordinary equation-driven inflow (no upstream leak to
        // mirror) must not error: it degrades to `beginning`, so the belt fills
        // over the transit exactly like the default -- 0 outflow for 16 steps.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f" isee:spreadflow="source"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build source-fallback");
        vm.run_to_end().expect("run");
        let out = vm.get_series(&Ident::new("out_f")).expect("out_f");
        for (i, &g) in out.iter().enumerate().take(16) {
            assert!(g.abs() < 1e-9, "step {i}: fallback outflow {g} should be 0");
        }
        assert!((out[20] - 250.0).abs() < 1e-4, "steady outflow {}", out[20]);
    }

    // ----- arrayed conveyors (§10) -----

    #[test]
    fn arrayed_conveyor_simulates_independent_belts() {
        // An arrayed conveyor is N_elem independent belts (§10). `board` has two
        // elements with DIFFERENT transit times (a=2, b=4, via the shared <len>
        // referencing the arrayed `transit` aux) and DIFFERENT inflows (a=100,
        // b=250). Each element must reach its own steady state and its own
        // transit delay, proving the belts are independent.
        //   belt[a]: transit 2 -> 8 slats, inflow 100 -> steady 200, out=100.
        //   belt[b]: transit 4 -> 16 slats, inflow 250 -> steady 1000, out=250.
        let xml = include_str!("../../../test/conveyors/arrayed_conveyor.xmile");
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build arrayed conveyor vm");
        vm.run_to_end().expect("run");

        let belt_a = vm.get_series(&Ident::new("belt[a]")).expect("belt[a]");
        let belt_b = vm.get_series(&Ident::new("belt[b]")).expect("belt[b]");
        let out_a = vm
            .get_series(&Ident::new("outflow_f[a]"))
            .expect("outflow_f[a]");
        let out_b = vm
            .get_series(&Ident::new("outflow_f[b]"))
            .expect("outflow_f[b]");
        assert!(belt_a.len() > 40, "should have many saved steps");

        // Independent transit delays: dt=0.25. belt[a] (8 slats) exits the first
        // cohort at t=2 (step 8); belt[b] (16 slats) not until t=4 (step 16).
        for (i, &g) in out_a.iter().enumerate().take(8) {
            assert!(g.abs() < 1e-9, "belt[a] step {i}: outflow {g} should be 0");
        }
        for (i, &g) in out_b.iter().enumerate().take(16) {
            assert!(g.abs() < 1e-9, "belt[b] step {i}: outflow {g} should be 0");
        }
        // belt[a] is already full/steady at step 8, but belt[b] (transit 4) is not
        // yet -- so at step 8 the two belts are in DIFFERENT states, which is the
        // whole point of independence.
        assert!(
            (out_a[8] - 100.0).abs() < 1e-6,
            "belt[a] outflow at step 8 {} (want 100)",
            out_a[8]
        );
        assert!(
            out_b[8].abs() < 1e-9,
            "belt[b] still filling at step 8, outflow {} (want 0)",
            out_b[8]
        );

        // Independent steady states.
        let last = belt_a.len() - 1;
        assert!(
            (belt_a[last] - 200.0).abs() < 1e-4,
            "belt[a] steady contents {} (want 200)",
            belt_a[last]
        );
        assert!(
            (belt_b[last] - 1000.0).abs() < 1e-4,
            "belt[b] steady contents {} (want 1000)",
            belt_b[last]
        );
        assert!(
            (out_a[last] - 100.0).abs() < 1e-6 && (out_b[last] - 250.0).abs() < 1e-6,
            "steady outflows a={} b={} (want 100, 250)",
            out_a[last],
            out_b[last]
        );
    }

    #[test]
    fn arrayed_conveyor_expands_to_one_plan_per_element() {
        // resolve_plans flattens an arrayed conveyor into one plan per element,
        // each pointing at that element's contiguous data-buffer slots -- and the
        // per-element stock/len/flow slots must be DISTINCT (independent belts).
        let xml = include_str!("../../../test/conveyors/arrayed_conveyor.xmile");
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let (compiled, plans, queue_plans) =
            build_compiled(&project, &main).expect("build_compiled");
        assert!(
            queue_plans.is_empty(),
            "a pure-conveyor model must synthesize zero queue plans"
        );
        assert_eq!(plans.len(), 2, "two elements -> two flattened plans");
        assert_ne!(
            plans[0].stock_off, plans[1].stock_off,
            "each element's belt reads a distinct stock slot"
        );
        assert_ne!(
            plans[0].len_off, plans[1].len_off,
            "each element has its own transit-time slot"
        );
        assert_ne!(
            plans[0].primary_out_off, plans[1].primary_out_off,
            "each element writes a distinct outflow slot"
        );
        // Sanity: the resolved offsets are real slots in the compiled buffer.
        assert_eq!(
            compiled.get_offset(&Ident::new("belt[a]")),
            Some(plans[0].stock_off)
        );
        assert_eq!(
            compiled.get_offset(&Ident::new("belt[b]")),
            Some(plans[1].stock_off)
        );
    }

    #[test]
    fn arrayed_conveyor_with_shared_leak_conserves_per_element() {
        // A shared linear leak fraction (0.2) applied apply-to-all across both
        // elements. Each belt conserves independently: steady outflow = inflow *
        // (1 - 0.2) = 0.8 * inflow, leak = 0.2 * inflow, per element.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <dimensions><dim name="board"/></dimensions>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f">
          <element subscript="a"><eqn>100</eqn></element>
          <element subscript="b"><eqn>250</eqn></element>
          <dimensions><dim name="board"/></dimensions></flow>
        <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/><dimensions><dim name="board"/></dimensions></flow>"#,
        );
        // wrap_model has no <dimensions>; inject the board dimension.
        let xml = xml.replace(
            "<model>",
            "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build arrayed leak vm");
        vm.run_to_end().expect("run");

        let out_a = vm.get_series(&Ident::new("out_f[a]")).expect("out_f[a]");
        let out_b = vm.get_series(&Ident::new("out_f[b]")).expect("out_f[b]");
        let leak_a = vm
            .get_series(&Ident::new("attriting[a]"))
            .expect("attriting[a]");
        let leak_b = vm
            .get_series(&Ident::new("attriting[b]"))
            .expect("attriting[b]");
        let last = out_a.len() - 1;
        assert!(
            (out_a[last] - 80.0).abs() < 1e-3,
            "belt[a] steady outflow {} (want 80)",
            out_a[last]
        );
        assert!(
            (out_b[last] - 200.0).abs() < 1e-3,
            "belt[b] steady outflow {} (want 200)",
            out_b[last]
        );
        assert!(
            (leak_a[last] - 20.0).abs() < 1e-3,
            "belt[a] steady leak {} (want 20)",
            leak_a[last]
        );
        assert!(
            (leak_b[last] - 50.0).abs() < 1e-3,
            "belt[b] steady leak {} (want 50)",
            leak_b[last]
        );
    }

    #[test]
    fn arrayed_leak_into_arrested_conveyor_skips_per_element() {
        // Element-wise wiring of the §4.3 step-2 skip: arrayed `up` leaks into
        // arrayed `down` element-for-element (leak[e] -> down[e]); arrest only
        // down[a] (a per-element arrest driver). During the window down[a]'s
        // inbound leak (leaking[a]) is skipped while down[b]'s (leaking[b]) keeps
        // flowing, and down[a]'s reported stock never diverges from its frozen
        // belt.
        let xml = wrap_model(
            r#"
        <stock name="up"><eqn>1000</eqn><inflow>src_in</inflow><outflow>up_out</outflow><outflow>leaking</outflow>
          <dimensions><dim name="board"/></dimensions>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="src_in"><eqn>250</eqn><dimensions><dim name="board"/></dimensions></flow>
        <flow name="up_out"><dimensions><dim name="board"/></dimensions></flow>
        <flow name="leaking"><eqn>0.2</eqn><leak/><dimensions><dim name="board"/></dimensions></flow>
        <stock name="down"><eqn>0</eqn><inflow>leaking</inflow><outflow>down_out</outflow>
          <dimensions><dim name="board"/></dimensions>
          <conveyor><len>4</len><arrest>arrest_drv</arrest></conveyor></stock>
        <flow name="down_out"><dimensions><dim name="board"/></dimensions></flow>
        <aux name="arrest_drv">
          <element subscript="a"><eqn>STEP(1, 5) - STEP(1, 8)</eqn></element>
          <element subscript="b"><eqn>0</eqn></element>
          <dimensions><dim name="board"/></dimensions></aux>
        <aux name="down_a_belt"><eqn>SUM(down[a])</eqn></aux>"#,
        );
        // wrap_model has no <dimensions>; inject the board dimension.
        let xml = xml.replace(
            "<model>",
            "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build arrayed arrest vm");
        vm.run_to_end().expect("run");
        let leak_a = vm
            .get_series(&Ident::new("leaking[a]"))
            .expect("leaking[a]");
        let leak_b = vm
            .get_series(&Ident::new("leaking[b]"))
            .expect("leaking[b]");
        let down_a = vm.get_series(&Ident::new("down[a]")).expect("down[a]");
        let down_a_belt = vm
            .get_series(&Ident::new("down_a_belt"))
            .expect("down_a_belt");
        // Element a's destination (down[a]) is arrested for steps 20..32: its
        // inbound leak is skipped; element b's is not.
        for i in 20..32 {
            assert!(
                leak_a[i].abs() < 1e-9,
                "step {i}: leaking[a]={} should be 0 (down[a] arrested)",
                leak_a[i]
            );
            assert!(
                leak_b[i] > 10.0,
                "step {i}: leaking[b]={} should keep flowing (down[b] not arrested)",
                leak_b[i]
            );
        }
        // down[a]'s reported stock never diverges from its frozen belt.
        for (i, (&s, &b)) in down_a.iter().zip(down_a_belt.iter()).enumerate() {
            assert!(
                (s - b).abs() < 1e-6,
                "step {i}: down[a] stock {s} diverged from belt {b}"
            );
        }
    }

    // ----- container access (§10): native computation + residual rejection -----

    /// Build the standard scalar-conveyor model plus a `reader` aux with the
    /// given equation, and return the `build_vm` result. The belt fills from
    /// empty (init 0).
    fn build_with_reader(reader_eqn: &str) -> crate::common::Result<crate::vm::Vm> {
        let xml = wrap_model(&format!(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="reader"><eqn>{reader_eqn}</eqn></aux>"#
        ));
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        build_vm(&project, &main)
    }

    /// Build a STEADY-STATE scalar-conveyor model (belt init 1000, inflow 250,
    /// transit 4, dt 0.25 -> 16 slats each holding 62.5) plus a `reader` aux, run
    /// it, and return `reader`'s series. Every hand-computed oracle in these
    /// tests reads this known belt.
    fn steady_reader_series(reader_eqn: &str) -> Vec<f64> {
        let xml = wrap_model(&format!(
            r#"
        <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="reader"><eqn>{reader_eqn}</eqn></aux>"#
        ));
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build steady reader");
        vm.run_to_end().expect("run");
        vm.get_series(&Ident::new("reader")).expect("reader series")
    }

    #[test]
    fn scalar_container_reducers_read_the_belt() {
        // The steady belt is 16 slats each 62.5 (SUM 1000). SUM/MEAN/SIZE/MIN/
        // MAX/STDDEV are now computed natively from the belt, not rejected (§10).
        let cases = [
            ("SUM(belt)", 1000.0),
            ("MEAN(belt)", 62.5),
            ("SIZE(belt)", 16.0),
            ("MIN(belt)", 62.5),
            ("MAX(belt)", 62.5),
            ("STDDEV(belt)", 0.0),
        ];
        for (eqn, want) in cases {
            let series = steady_reader_series(eqn);
            for (i, &v) in series.iter().enumerate() {
                assert!(
                    (v - want).abs() < 1e-9,
                    "'{eqn}' step {i}: got {v}, want {want}"
                );
            }
        }
    }

    #[test]
    fn scalar_slat_index_reads_slat_and_out_of_range_is_nan() {
        // conv[j] is 1-based from the exit. On the 16-slat steady belt every slat
        // is 62.5; conv[0] and conv[17] are out of range -> NaN (§10).
        assert!((steady_reader_series("belt[1]")[10] - 62.5).abs() < 1e-9);
        assert!((steady_reader_series("belt[16]")[10] - 62.5).abs() < 1e-9);
        assert!(
            steady_reader_series("belt[0]")[10].is_nan(),
            "belt[0] -> NaN"
        );
        assert!(
            steady_reader_series("belt[17]")[10].is_nan(),
            "belt[17] -> NaN"
        );
    }

    #[test]
    fn container_access_in_larger_expression_and_conditional() {
        // A supported container access nested inside a larger expression / an IF
        // is rewritten in place (not whole-equation), so the surrounding math is
        // preserved: SUM(belt) + 1 == 1001; IF belt[2] > 0 THEN 1 ELSE 0 == 1.
        assert!((steady_reader_series("SUM(belt) + 1")[5] - 1001.0).abs() < 1e-9);
        for &v in steady_reader_series("IF belt[2] > 0 THEN 1 ELSE 0").iter() {
            assert_eq!(v, 1.0);
        }
    }

    #[test]
    fn container_init_reads_start_of_run_value_not_placeholder() {
        // INIT(<container access>) must read the belt's START-OF-RUN value, not
        // the hidden container stock's '0' <eqn> placeholder. The rewrite turns
        // both SUM(belt) and INIT(SUM(belt)) into the hidden stock $conv$sum$belt;
        // its initial_values snapshot must be patched to the initialized belt's
        // total. On the steady belt SUM(belt)==1000 every step, so the ratio is
        // 1.0; before the fix INIT read the frozen 0 and the ratio was +inf.
        let ratio = steady_reader_series("SUM(belt) / INIT(SUM(belt))");
        for (i, &v) in ratio.iter().enumerate() {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "step {i}: SUM(belt)/INIT(SUM(belt)) = {v} (want 1.0; pre-fix +inf)"
            );
        }
        // INIT of a reducer, SIZE, and a slat index all read the start-of-run
        // belt (16 slats of 62.5): pre-fix every one read the frozen 0.
        for (eqn, want) in [
            ("INIT(SUM(belt))", 1000.0),
            ("INIT(SIZE(belt))", 16.0),
            ("INIT(belt[1])", 62.5),
        ] {
            let series = steady_reader_series(eqn);
            for (i, &v) in series.iter().enumerate() {
                assert!(
                    (v - want).abs() < 1e-9,
                    "'{eqn}' step {i}: got {v}, want {want} (pre-fix 0)"
                );
            }
        }
    }

    #[test]
    fn stock_initialized_from_container_access_reads_start_of_run() {
        // A plain stock whose <eqn> is a container access (no INIT wrapper) must
        // also start from the belt's start-of-run total: the belt/queue init runs
        // after the initials snapshot, so the initials pass first sees the '0'
        // placeholder, and the reconciliation re-run recomputes the stock's initial
        // value from the published belt. `accum` starts at SUM(belt)=1000 and, with
        // no flows, holds flat -- pre-fix it started at 0.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <stock name="accum"><eqn>SUM(belt)</eqn></stock>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build stock-init vm");
        vm.run_to_end().expect("run");
        let accum = vm.get_series(&Ident::new("accum")).expect("accum");
        assert!(
            (accum[0] - 1000.0).abs() < 1e-9,
            "accum[0] = {} (want 1000; pre-fix 0)",
            accum[0]
        );
        for (i, &v) in accum.iter().enumerate() {
            assert!(
                (v - 1000.0).abs() < 1e-9,
                "step {i}: accum = {v} (a no-flow stock initialized to 1000 stays flat)"
            );
        }
    }

    #[test]
    fn container_init_survives_reset_and_rerun() {
        // libsimlin's reset recreates the belt side table and re-runs
        // run_initials, so the container-value reconciliation must be idempotent:
        // INIT(SUM(belt)) must still read the start-of-run 1000 after a reset,
        // not accumulate or drift. The re-run derives only from freshly published
        // belt state, so it is idempotent by construction.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="reader"><eqn>INIT(SUM(belt))</eqn></aux>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build reset vm");
        vm.run_to_end().expect("first run");
        let first = vm.get_series(&Ident::new("reader")).expect("reader");
        vm.reset();
        vm.run_to_end().expect("second run");
        let second = vm.get_series(&Ident::new("reader")).expect("reader");
        assert_eq!(first, second, "reset+rerun must reproduce INIT(SUM(belt))");
        for (i, &v) in second.iter().enumerate() {
            assert!(
                (v - 1000.0).abs() < 1e-9,
                "step {i}: INIT(SUM(belt)) = {v} after reset (want 1000)"
            );
        }
    }

    #[test]
    fn arrayed_container_init_patches_every_element_slot() {
        // An arrayed conveyor's container stock is arrayed over the owner's dims,
        // so the initial_values patch-up must reach EVERY element slot, not just
        // the first. Both belts start at steady 1000 (inflow 250, transit 4), so
        // SUM(belt[a])==SUM(belt[b])==1000 every step and INIT(SUM(belt[a]))==1000
        // for each element (pre-fix 0 -> ratio +inf for both).
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <dimensions><dim name="board"/></dimensions>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn><dimensions><dim name="board"/></dimensions></flow>
        <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
        <aux name="ratio_a"><eqn>SUM(belt[a]) / INIT(SUM(belt[a]))</eqn></aux>
        <aux name="init_b"><eqn>INIT(SUM(belt[b]))</eqn></aux>"#,
        );
        // wrap_model has no <dimensions>; inject the board dimension.
        let xml = xml.replace(
            "<model>",
            "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build arrayed init vm");
        vm.run_to_end().expect("run");
        let ratio_a = vm.get_series(&Ident::new("ratio_a")).expect("ratio_a");
        for (i, &v) in ratio_a.iter().enumerate() {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "step {i}: SUM(belt[a])/INIT(SUM(belt[a])) = {v} (want 1.0; pre-fix +inf)"
            );
        }
        let init_b = vm.get_series(&Ident::new("init_b")).expect("init_b");
        for (i, &v) in init_b.iter().enumerate() {
            assert!(
                (v - 1000.0).abs() < 1e-9,
                "step {i}: INIT(SUM(belt[b])) = {v} (want 1000; pre-fix 0 -- second element slot)"
            );
        }
    }

    #[test]
    fn container_reducer_on_filling_belt_tracks_min_max_stddev() {
        // A belt filling from empty exercises MIN/MAX/STDDEV over a non-uniform
        // and briefly-empty belt. Insert-at-entry (default `beginning`) means an
        // empty belt after step 0's insert holds one slat; MIN of a partially
        // filled belt (some 0 slats) is 0 while MAX rises toward the inflow
        // cohort (250*0.25 = 62.5), and STDDEV is > 0 mid-fill.
        let min_s = steady_reader_min_max_stddev("MIN(belt)");
        let max_s = steady_reader_min_max_stddev("MAX(belt)");
        let std_s = steady_reader_min_max_stddev("STDDEV(belt)");
        // Early in the fill the belt has both 0 slats and a 62.5 cohort.
        assert_eq!(min_s[2], 0.0, "MIN over a partly-empty belt is 0");
        assert!(
            (max_s[2] - 62.5).abs() < 1e-9,
            "MAX over the cohort is 62.5, got {}",
            max_s[2]
        );
        assert!(
            std_s[2] > 0.0,
            "STDDEV mid-fill is positive, got {}",
            std_s[2]
        );
        // Once full and steady every slat is 62.5: MIN==MAX, STDDEV==0.
        let last = min_s.len() - 1;
        assert!((min_s[last] - 62.5).abs() < 1e-9 && (max_s[last] - 62.5).abs() < 1e-9);
        assert!(
            std_s[last].abs() < 1e-9,
            "steady STDDEV 0, got {}",
            std_s[last]
        );
    }

    /// A filling-from-empty belt reader series (belt init 0, inflow 250).
    fn steady_reader_min_max_stddev(reader_eqn: &str) -> Vec<f64> {
        let mut vm = build_with_reader(reader_eqn).expect("build filling reader");
        vm.run_to_end().expect("run");
        vm.get_series(&Ident::new("reader")).expect("reader")
    }

    #[test]
    fn container_value_is_start_of_step_and_feeds_a_flow_same_step() {
        // The CRUX (§10): a container value is read DURING the flows phase, must
        // reflect START-OF-STEP belt state, and must survive the flows eval so it
        // is not clobbered. `reader = SUM(belt)` feeds both another aux
        // (`doubled = reader * 2`) and a flow into a sink stock (`accum`), all in
        // the same step. On the steady belt SUM(belt) == 1000 every step, so
        // doubled == 2000 and accum accumulates 1000/time.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="reader"><eqn>SUM(belt)</eqn></aux>
        <aux name="doubled"><eqn>reader * 2</eqn></aux>
        <stock name="accum"><eqn>0</eqn><inflow>sink_f</inflow></stock>
        <flow name="sink_f"><eqn>reader</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build crux model");
        vm.run_to_end().expect("run");
        let reader = vm.get_series(&Ident::new("reader")).expect("reader");
        let doubled = vm.get_series(&Ident::new("doubled")).expect("doubled");
        let accum = vm.get_series(&Ident::new("accum")).expect("accum");
        for (i, (&r, &d)) in reader.iter().zip(doubled.iter()).enumerate() {
            assert!((r - 1000.0).abs() < 1e-9, "step {i} reader {r}");
            assert!((d - 2000.0).abs() < 1e-9, "step {i} doubled {d}");
        }
        // accum integrates 1000/time from t=0; the final value ~= 1000 * stop.
        let last = accum[accum.len() - 1];
        assert!(
            (last - 1000.0 * 20.0).abs() < 1.0,
            "accum final {last} (want ~20000)"
        );
    }

    #[test]
    fn container_reducer_is_read_before_this_steps_insert() {
        // Start-of-step timing (§10): a reader of SUM(belt) sees the belt BEFORE
        // this step's exit/insert. The belt fills from empty (16 zero-slats,
        // inflow 250 -> 62.5 inserted per step, nothing exits for 16 steps), so
        // SUM(belt) at step t reflects only the t inserts made in the PRIOR steps:
        // reader[t] == 62.5 * t for t <= 16, then plateaus at 1000. If the value
        // were recomputed after this step's insert it would read 62.5 higher.
        let series = steady_reader_min_max_stddev("SUM(belt)"); // belt init 0
        assert_eq!(series[0], 0.0, "step 0: no inserts yet");
        for (t, &v) in series.iter().enumerate().take(17).skip(1) {
            assert!(
                (v - 62.5 * t as f64).abs() < 1e-9,
                "step {t}: SUM(belt)={v} (want {}, start-of-step)",
                62.5 * t as f64
            );
        }
        assert!((series[20] - 1000.0).abs() < 1e-9, "plateaus at 1000");
    }

    #[test]
    fn container_access_in_conveyor_parameter_expression_is_computed() {
        // A container access inside the conveyor's OWN parameter expression must
        // be rewritten too (not just ordinary equations): `<capacity>SIZE(belt)`
        // binds to the belt length (16), so contents plateau at 16 -- NOT the
        // silent SIZE(scalar)=1 that plateaued at 1 before the fix.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len><capacity>SIZE(belt)</capacity></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build param-container vm");
        vm.run_to_end().expect("run");
        let belt = vm.get_series(&Ident::new("belt")).expect("belt");
        for (i, &b) in belt.iter().enumerate() {
            assert!(
                b <= 16.0 + 1e-6,
                "step {i}: contents {b} exceeds capacity 16"
            );
        }
        let last = belt[belt.len() - 1];
        assert!(
            (last - 16.0).abs() < 1e-6,
            "capacity=SIZE(belt)=16 plateaus contents at 16, got {last} (silent-wrong would be 1)"
        );
    }

    #[test]
    fn residual_container_access_in_conveyor_parameter_is_rejected() {
        // A residual container form in a parameter or leak-fraction expression is
        // loud-rejected exactly like one in an ordinary equation -- never
        // silently mis-bound to the scalar stock.
        let cases = [
            r#"<conveyor><len>4</len><capacity>MEAN(belt / 2)</capacity></conveyor>"#,
            r#"<conveyor><len>MEAN(belt / 2)</len></conveyor>"#,
        ];
        for conveyor in cases {
            let xml = wrap_model(&format!(
                r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          {conveyor}</stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#
            ));
            let project = parse(&xml);
            let main = project.models[0].name.clone();
            let err = build_vm(&project, &main)
                .err()
                .unwrap_or_else(|| panic!("residual param '{conveyor}' should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorContainerAccessUnsupported,
                "conveyor '{conveyor}'"
            );
        }
    }

    #[test]
    fn residual_container_access_in_leak_fraction_is_rejected() {
        // A residual container form in a leak-fraction expression is likewise
        // loud-rejected (the fraction is synthesized into a `$conv$leak$...$frac`
        // aux from its raw string, so it must be rewritten/checked too).
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>MEAN(belt / 2)</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .expect_err("residual leak-fraction container access should be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorContainerAccessUnsupported);
    }

    #[test]
    fn scalar_dynamic_and_ranged_container_access_still_rejected() {
        // The genuinely-unlowerable residuals stay loud-rejected (§10): a dynamic
        // (non-constant) slat index and a slat range/wildcard need the per-slat
        // vector, which cannot be reduced to one native scalar.
        for eqn in ["belt[k]", "belt[1:2]", "belt[*]"] {
            let err = build_with_reader(&format!("{eqn} + 0"))
                .err()
                .unwrap_or_else(|| panic!("residual '{eqn}' should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorContainerAccessUnsupported,
                "equation '{eqn}'"
            );
        }
    }

    #[test]
    fn scalar_wrapped_reducer_forms_are_rejected() {
        // Finding 2: a scalar conveyor wrapped in ANY expression inside a
        // single-arg reducer still means the belt's slats (why else reduce a
        // scalar?), so it must be rejected -- not silently return the belt total.
        for eqn in [
            "MEAN(belt + 0)",
            "MEAN(belt / 2)",
            "MIN(belt * 2)",
            "SUM(belt - 1)",
            "STDDEV(2 * belt + 3)",
            "SIZE(belt + belt)",
        ] {
            let err = build_with_reader(eqn)
                .err()
                .unwrap_or_else(|| panic!("wrapped reducer '{eqn}' should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorContainerAccessUnsupported,
                "equation '{eqn}'"
            );
        }
    }

    #[test]
    fn scalar_min_max_with_two_args_reduces_belt_total_not_belt() {
        // MIN/MAX with a second argument is scalar min/max of the belt TOTAL, not
        // belt-container access -- it must NOT be rejected and simulates fine.
        for eqn in ["MIN(belt, 5)", "MAX(belt, 5)"] {
            let mut vm = build_with_reader(eqn)
                .unwrap_or_else(|_| panic!("scalar min/max of belt total '{eqn}' should compile"));
            vm.run_to_end().expect("run");
        }
    }

    #[test]
    fn non_container_conveyor_reads_are_unaffected() {
        // A bare read of a conveyor's scalar value (its belt total) is ordinary
        // and must NOT be flagged as container access.
        let mut vm = build_with_reader("belt * 2").expect("bare read of belt total is fine");
        vm.run_to_end().expect("run");
    }

    /// Build the standard arrayed-conveyor model (board {a,b}; inflow a=100,
    /// b=250; transit 4; belt filling from empty) plus a `reader` aux, returning
    /// the `build_vm` result.
    fn build_arrayed_reader(reader: &str) -> crate::common::Result<crate::vm::Vm> {
        let xml = wrap_model(&format!(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <dimensions><dim name="board"/></dimensions>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f">
          <element subscript="a"><eqn>100</eqn></element>
          <element subscript="b"><eqn>250</eqn></element>
          <dimensions><dim name="board"/></dimensions></flow>
        <flow name="out_f"><dimensions><dim name="board"/></dimensions></flow>
        <aux name="reader"><eqn>{reader}</eqn></aux>"#
        ))
        .replace(
            "<model>",
            "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        build_vm(&project, &main)
    }

    #[test]
    fn arrayed_conveyor_ordinary_array_reads_allowed() {
        // For an arrayed conveyor, reading one element (`belt[a]`, that belt's
        // TOTAL) and reducing over the per-element totals (`SUM(belt)`) are
        // ordinary array ops -- unchanged, no container synthesis.
        for eqn in ["belt[a]", "SUM(belt)", "SUM(belt[*])", "SUM(belt * 2)"] {
            let mut vm = build_arrayed_reader(eqn)
                .unwrap_or_else(|_| panic!("ordinary array read '{eqn}' should compile"));
            vm.run_to_end().expect("run");
        }
    }

    #[test]
    fn arrayed_single_belt_container_access_computes_per_element() {
        // A single-belt subscript reduced by a reducer, or a per-element belt
        // slot, is now supported (§10): the container variable is arrayed over
        // the conveyor's dims, so `SUM(belt[a])` reads belt a and `belt[b, 2]`
        // reads belt b's slat 2. Steady: belt[a] = 16 slats of 25 (SUM 400),
        // belt[b] = 16 slats of 62.5 (SUM 1000).
        let series = |reader: &str| -> Vec<f64> {
            let mut vm = build_arrayed_reader(reader).expect("build arrayed reader");
            vm.run_to_end().expect("run");
            vm.get_series(&Ident::new("reader")).expect("reader")
        };
        let steady = |s: &[f64]| s[s.len() - 1];
        assert!((steady(&series("SUM(belt[a])")) - 400.0).abs() < 1e-6);
        assert!((steady(&series("SUM(belt[b])")) - 1000.0).abs() < 1e-6);
        assert!((steady(&series("MEAN(belt[a])")) - 25.0).abs() < 1e-6);
        assert!((steady(&series("MEAN(belt[b])")) - 62.5).abs() < 1e-6);
        assert!((steady(&series("SIZE(belt[a])")) - 16.0).abs() < 1e-9);
        // belt[b, 2]: slat 2 of belt b, 62.5 at steady state.
        assert!((steady(&series("belt[b, 2]")) - 62.5).abs() < 1e-6);
        // belt[a, 1]: exit slat of belt a, 25 at steady state.
        assert!((steady(&series("belt[a, 1]")) - 25.0).abs() < 1e-6);
    }

    #[test]
    fn arrayed_bare_non_sum_reducer_still_rejected() {
        // A bare arrayed-conveyor reducer other than SUM has no single-belt
        // interpretation (it would read per-element TOTALS, not slats) -- it
        // stays loud-rejected (§10). SUM is the one spec-safe bare reduction.
        for eqn in [
            "MEAN(belt)",
            "MIN(belt)",
            "MAX(belt)",
            "STDDEV(belt)",
            "SIZE(belt)",
        ] {
            assert_eq!(
                build_arrayed_reader(eqn)
                    .expect_err("bare arrayed non-SUM reducer should be rejected")
                    .code,
                ErrorCode::ConveyorContainerAccessUnsupported,
                "equation '{eqn}'"
            );
        }
    }

    #[test]
    fn ordinary_conveyor_simulation_unaffected_by_container_guard() {
        // The container-access guard must not perturb a conveyor model that uses
        // no container access: the steady-state oracle still holds exactly.
        let xml = include_str!("../../../test/conveyors/minimal_conveyor.xmile");
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let students = vm.get_series(&Ident::new("students")).expect("students");
        for &s in &students {
            assert!((s - 1000.0).abs() < 1e-6, "Students should hold at 1000");
        }
    }

    #[test]
    fn equation_reading_driven_flow_is_rejected() {
        // An aux that reads the conveyor's outflow would see the placeholder 0
        // (the pass runs after flows), so expansion rejects it loudly.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="reader"><eqn>out_f * 2</eqn></aux>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("reading a driven flow must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorDrivenFlowRead);
    }

    #[test]
    fn conveyor_capacity_reading_driven_flow_is_rejected() {
        // A conveyor PARAMETER expression (here `<capacity>`) that references a
        // conveyor-driven flow (`attriting`, the belt's own leak outflow) escapes
        // the ordinary reader scan because the parameter is lifted into a hidden
        // aux appended only after that scan. It would silently compute from the
        // flow's Flows-phase placeholder 0 -- capacity 0, belt admits nothing, no
        // error. Expansion must reject it loudly, naming the conveyor PARAMETER
        // (not the internal `$conv$...` aux name) and the flow.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len><capacity>attriting * 20</capacity></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .expect_err("a capacity reading a driven flow must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorDrivenFlowRead);
        let msg = err.get_details().unwrap_or_default();
        assert!(
            msg.contains("belt") && msg.contains("<capacity>") && msg.contains("attriting"),
            "message should name the conveyor, the parameter, and the driven flow: {msg}"
        );
    }

    #[test]
    fn conveyor_parameters_reading_driven_flow_are_rejected() {
        // Every conveyor parameter (`<len>`/`<capacity>`/`<in_limit>`/`<sample>`/
        // `<arrest>`) is lifted into a hidden aux; a reference to a driven flow in
        // ANY of them is the same placeholder-0 hazard (a zeroed cap/len, a never-
        // arrested belt, a sample/arrest condition read a step early). The scan
        // must cover all of them and name the offending parameter.
        for (label, block) in [
            ("<len>", "<len>attriting * 10</len>"),
            (
                "<capacity>",
                "<len>4</len><capacity>attriting * 20</capacity>",
            ),
            ("<in_limit>", "<len>4</len><in_limit>attriting</in_limit>"),
            ("<sample>", "<len>4</len><sample>attriting</sample>"),
            ("<arrest>", "<len>4</len><arrest>attriting</arrest>"),
        ] {
            let xml = wrap_model(&format!(
                r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor>{block}</conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
            ));
            let project = parse(&xml);
            let main = project.models[0].name.clone();
            let err = build_vm(&project, &main).expect_err(&format!(
                "parameter {label} reading a driven flow must be rejected"
            ));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorDrivenFlowRead,
                "parameter {label} should be rejected"
            );
            let msg = err.get_details().unwrap_or_default();
            assert!(
                msg.contains("belt") && msg.contains(label) && msg.contains("attriting"),
                "parameter {label}: message should name the conveyor, the parameter, and the flow: {msg}"
            );
        }
    }

    #[test]
    fn conveyor_leak_fraction_reading_driven_flow_is_rejected() {
        // A leak's FRACTION is also lifted into a hidden aux. An explicit
        // `<leak>expr</leak>` fraction that references another driven flow (here
        // the primary outflow `out_f`) reads its placeholder 0 in the Flows phase
        // -- a silently-zeroed leak rate. Reject it, naming the leak flow.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>1000</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0</eqn><leak>out_f * 0.001</leak></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .expect_err("a leak fraction reading a driven flow must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorDrivenFlowRead);
        let msg = err.get_details().unwrap_or_default();
        assert!(
            msg.contains("belt") && msg.contains("attriting") && msg.contains("out_f"),
            "message should name the conveyor, the leak flow, and the driven flow: {msg}"
        );
    }

    #[test]
    fn conveyor_capacity_referencing_ordinary_aux_compiles_and_simulates() {
        // Negative control: a capacity that references an ORDINARY (non-driven)
        // aux is fine -- the aux is computed in the Flows phase like any other, so
        // the belt sees the real capacity. `cap_base * 2 = 600` plateaus contents
        // at 600, exactly like a literal `<capacity>600</capacity>` (S5).
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len><capacity>cap_base * 2</capacity></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="cap_base"><eqn>300</eqn></aux>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let belt = vm.get_series(&Ident::new("belt")).expect("belt");
        for (i, &b) in belt.iter().enumerate() {
            assert!(b <= 600.0 + 1e-6, "step {i}: contents {b} exceeds capacity");
        }
        assert!(
            (belt[belt.len() - 1] - 600.0).abs() < 1e-6,
            "plateaus at 600: {}",
            belt[belt.len() - 1]
        );
    }

    // ----- multiple non-leak outflows (§3.3): the conveyor slat model has
    // exactly one primary (belt-end) outflow plus leak flows. A second (or
    // later) NON-leak outflow has no slat-model meaning; leaving it as an
    // ordinary equation-driven outflow drains the expanded INTEG stock without
    // removing material from the belt side table, so the reported stock and the
    // belt total diverge with no diagnostic. Expansion rejects it loudly.

    #[test]
    fn second_non_leak_outflow_is_rejected() {
        // `graduating` is the primary (first non-leak, placeholder eqn 0);
        // `dropping_out` is a second non-leak outflow with a real rate. Before
        // the fix this simulated silently -- the Stocks phase drained `students`
        // by `dropping_out` while the belt never lost that material, diverging
        // permanently. It must be rejected, naming the conveyor and the extra.
        let xml = wrap_model(
            r#"
        <stock name="students"><eqn>0</eqn><inflow>matriculating</inflow><outflow>graduating</outflow><outflow>dropping_out</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="matriculating"><eqn>0</eqn></flow>
        <flow name="graduating"><eqn>0</eqn></flow>
        <flow name="dropping_out"><eqn>50</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .expect_err("a conveyor with two non-leak outflows must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorMultipleNonLeakOutflows);
        let msg = err.get_details().unwrap_or_default();
        assert!(
            msg.contains("students")
                && msg.contains("graduating")
                && msg.contains("dropping_out")
                && msg.contains("<leak/>"),
            "message should name the conveyor, the primary, the extra outflow, and the <leak/> fix: {msg}"
        );
    }

    #[test]
    fn primary_plus_leak_outflow_still_compiles() {
        // Negative control: the SUPPORTED shape (one primary + one <leak/>
        // outflow) must still build and simulate -- the rejection targets only
        // extra NON-leak outflows. Mirrors `linear_leak_reaches_steady_state`.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("primary + leak must still build");
        vm.run_to_end().expect("run");
    }

    #[test]
    fn primary_leak_and_extra_non_leak_rejects_naming_only_the_extra() {
        // Three outflows: `out_f` primary, `attriting` a <leak/>, `dropping` a
        // SECOND non-leak. The leak must NOT be misidentified as the extra: the
        // message names `dropping` (the real extra) and never `attriting`.
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>attriting</outflow><outflow>dropping</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <flow name="attriting"><eqn>0.2</eqn><leak/></flow>
        <flow name="dropping"><eqn>10</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main)
            .expect_err("primary + leak + extra non-leak must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorMultipleNonLeakOutflows);
        let msg = err.get_details().unwrap_or_default();
        assert!(
            msg.contains("dropping"),
            "message should name the extra non-leak outflow `dropping`: {msg}"
        );
        assert!(
            !msg.contains("attriting"),
            "the <leak/> flow must NOT be listed as an extra outflow: {msg}"
        );
    }

    #[test]
    fn two_conveyors_each_with_one_primary_compile() {
        // Negative control: two independent conveyors, each with a single
        // primary outflow, must not cross-talk into a false multiple-outflow
        // rejection. Both belts simulate.
        let xml = wrap_model(
            r#"
        <stock name="belt_a"><eqn>0</eqn><inflow>in_a</inflow><outflow>out_a</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_a"><eqn>250</eqn></flow>
        <flow name="out_a"><eqn>0</eqn></flow>
        <stock name="belt_b"><eqn>0</eqn><inflow>in_b</inflow><outflow>out_b</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_b"><eqn>100</eqn></flow>
        <flow name="out_b"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("two single-primary conveyors must build");
        vm.run_to_end().expect("run");
        assert!(vm.get_series(&Ident::new("belt_a")).is_some());
        assert!(vm.get_series(&Ident::new("belt_b")).is_some());
    }

    // ----- slat-count bound (§4.1): a hostile/typo'd <len> must never
    // panic/OOM the engine; it is rejected loudly at belt init / latch time.
    // The tests shrink the bound with a `SlatBoundGuard` so a tiny fixture trips
    // the gate without allocating a production-sized belt. At dt=0.25,
    // `slat_count(transit) = round(transit/0.25)`: transit 1.0 -> 4 slats,
    // transit 1.25 -> 5 slats.

    /// A conveyor whose initial `<len>` needs more slats than the bound is
    /// rejected at init (`init_belts`) with the new code, naming the belt.
    #[test]
    fn slat_bound_rejects_over_bound_transit_at_init() {
        let _guard = crate::conveyor::SlatBoundGuard::new(4);
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>1.25</len></conveyor></stock>
        <flow name="in_f"><eqn>10</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        let err = vm
            .run_to_end()
            .expect_err("a transit needing 5 slats must be rejected against a bound of 4");
        assert_eq!(err.code, ErrorCode::ConveyorTransitTooLong);
        let msg = err.get_details().unwrap_or_default();
        assert!(
            msg.contains("belt") && msg.contains('5') && msg.contains('4'),
            "message should name the belt, the slat count, and the bound: {msg}"
        );
    }

    /// A conveyor whose initial `<len>` lands exactly ON the bound is admitted
    /// (the gate rejects only counts strictly above the bound).
    #[test]
    fn slat_bound_admits_at_bound_transit_at_init() {
        let _guard = crate::conveyor::SlatBoundGuard::new(4);
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>1</len></conveyor></stock>
        <flow name="in_f"><eqn>10</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end()
            .expect("a transit needing exactly 4 slats is at the bound, not over it");
    }

    /// The finding's exact abort case: a `<len>` of 1e300 whose `transit/dt`
    /// saturates `usize`. The gate rejects it (loud error) instead of
    /// `init_steady` panicking `vec![0.0; usize::MAX]` -- and, because the check
    /// precedes the allocation, nothing near `usize::MAX` is ever allocated.
    #[test]
    fn slat_bound_rejects_saturating_transit_without_allocating() {
        let _guard = crate::conveyor::SlatBoundGuard::new(4);
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>1e300</len></conveyor></stock>
        <flow name="in_f"><eqn>10</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        let err = vm
            .run_to_end()
            .expect_err("a usize-saturating transit must be rejected, not panic");
        assert_eq!(err.code, ErrorCode::ConveyorTransitTooLong);
    }

    /// A time-varying `<len>` (default `<sample>` re-latches every DT) that is
    /// under the bound at init but grows over it mid-run must be rejected LOUDLY
    /// from the runtime pass -- not silently clamp the belt geometry (repo rule:
    /// a loud error beats a silently-wrong simulation). STEP raises `<len>` from
    /// 1.0 (4 slats, at the bound) to 1.25 (5 slats, over it) at t=2.
    #[test]
    fn slat_bound_rejects_over_bound_latch_mid_run() {
        let _guard = crate::conveyor::SlatBoundGuard::new(4);
        let xml = wrap_model(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>1 + STEP(0.25, 2)</len></conveyor></stock>
        <flow name="in_f"><eqn>10</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        let err = vm
            .run_to_end()
            .expect_err("a mid-run relatch needing 5 slats must be rejected against a bound of 4");
        assert_eq!(err.code, ErrorCode::ConveyorTransitTooLong);
    }

    // ----- discrete per-time-unit in_limit reset (v13 / §6.3) -----

    #[test]
    fn conveyor_time_unit_recovers_ideal_grid_boundary() {
        // Non-dyadic dt: ten 0.1 steps accumulate to 0.999...9 (floor 0), but the
        // per-time-unit boundary is 1 -- taken from the recovered ideal grid time,
        // not the drift-accumulated clock.
        let mut acc: f64 = 0.0;
        for _ in 0..10 {
            acc += 0.1;
        }
        assert_eq!(
            acc.floor() as i64,
            0,
            "precondition: additive drift floors to 0"
        );
        assert_eq!(conveyor_time_unit(acc, 0.0, 0.1), 1);

        // dt = 1/3: six steps sum to 1.9999999999999998 (floor 1), but the ideal
        // grid time 6*dt is exactly 2.0. (Three steps happen to round to exactly
        // 1.0, so the drift only bites at the k=6 boundary here.)
        let dt = 1.0 / 3.0;
        let mut acc: f64 = 0.0;
        for _ in 0..6 {
            acc += dt;
        }
        assert!(
            acc < 2.0,
            "precondition: additive sixths-of-a-third drift below 2.0"
        );
        assert_eq!(conveyor_time_unit(acc, 0.0, dt), 2);

        // Dyadic dt is exact: boundary detection is unchanged.
        assert_eq!(conveyor_time_unit(1.0, 0.0, 0.25), 1);
        assert_eq!(conveyor_time_unit(0.75, 0.0, 0.25), 0);

        // Step 0 returns floor(start): no spurious reset, matches the VM's
        // conveyor_last_unit seed.
        assert_eq!(conveyor_time_unit(0.0, 0.0, 0.1), 0);
        assert_eq!(conveyor_time_unit(2.5, 2.5, 0.1), 2);

        // A non-grid-aligned start whose grid points straddle (never hit) the
        // integers must NOT fire early: start=0.05, dt=0.1 -> t~=0.95 is unit 0,
        // t~=1.05 is unit 1.
        let start = 0.05;
        let mut acc = start;
        for _ in 0..9 {
            acc += 0.1;
        }
        assert_eq!(
            conveyor_time_unit(acc, start, 0.1),
            0,
            "t~=0.95 is still unit 0"
        );
        acc += 0.1;
        assert_eq!(conveyor_time_unit(acc, start, 0.1), 1, "t~=1.05 is unit 1");
    }

    #[test]
    fn conveyor_time_unit_no_drift_over_long_run() {
        // The additive clock drifts both below and above the integers as it
        // accumulates; recovering the step index keeps every boundary on its ideal
        // grid step across a long run (500 steps).
        let dt = 0.1;
        let mut acc: f64 = 0.0;
        for step in 0..=500 {
            let want = (step as f64 * dt).floor() as i64;
            assert_eq!(
                conveyor_time_unit(acc, 0.0, dt),
                want,
                "step {step}: accumulated {acc} misdetects the time unit"
            );
            acc += dt;
        }
    }

    fn wrap_model_specs(vars: &str, start: f64, stop: f64, dt: f64) -> String {
        // `{start}`/`{stop}`/`{dt}` format via f64 Display, i.e. the shortest
        // round-trippable decimal, so the parsed dt is bit-identical to the f64
        // the assertions use (1.0/3.0 -> "0.3333333333333333" -> 1.0/3.0).
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>{start}</start><stop>{stop}</stop><dt>{dt}</dt></sim_specs>
  <model><variables>{vars}</variables></model>
</xmile>"#
        )
    }

    /// Build and run one discrete conveyor whose per-time-unit `in_limit` is 5,
    /// fed by an abundant (100/unit) equation inflow that exhausts the budget in a
    /// single step. The `in_f` slot holds the ADMITTED rate (admitted volume / dt)
    /// after Phase B, so the returned series is a pulse of `5/dt` on the first step
    /// of each integer time unit and 0 while that unit's budget is spent.
    fn run_discrete_in_limit_series(start: f64, stop: f64, dt: f64) -> Vec<f64> {
        let vars = r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor discrete="true"><len>2</len><in_limit>5</in_limit></conveyor></stock>
        <flow name="in_f"><eqn>100</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#;
        let xml = wrap_model_specs(vars, start, stop, dt);
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build discrete in_limit vm");
        vm.run_to_end().expect("run discrete in_limit");
        vm.get_series(&Ident::new("in_f")).expect("in_f series")
    }

    /// Assert a discrete `in_limit=5` admission series (from
    /// [`run_discrete_in_limit_series`], start = 0) is a pulse of `5/dt` on the
    /// first step of each integer time unit and 0 elsewhere, admitting exactly 5
    /// per unit. Step `k` models ideal time `k*dt`, so unit `u`'s first step is at
    /// index `round(u/dt)`.
    fn assert_discrete_pulses(series: &[f64], dt: f64, n_units: usize) {
        let pulse_rate = 5.0 / dt;
        for u in 0..n_units {
            let first = (u as f64 / dt).round() as usize;
            let next = ((u as f64 + 1.0) / dt).round() as usize;
            assert!(
                next <= series.len(),
                "unit {u} window [{first},{next}) exceeds series length {}",
                series.len()
            );
            let mut unit_vol = 0.0;
            for (offset, &rate) in series[first..next].iter().enumerate() {
                let k = first + offset;
                unit_vol += rate * dt;
                if k == first {
                    assert!(
                        (rate - pulse_rate).abs() < 1e-6,
                        "unit {u} step {k} (t~={:.4}): admitted rate {rate}, want pulse {pulse_rate}",
                        k as f64 * dt
                    );
                } else {
                    assert!(
                        rate.abs() < 1e-9,
                        "unit {u} step {k} (t~={:.4}): admitted rate {rate}, want 0 (budget spent)",
                        k as f64 * dt
                    );
                }
            }
            assert!(
                (unit_vol - 5.0).abs() < 1e-6,
                "unit {u}: admitted {unit_vol} over the unit, want exactly in_limit=5 (no double-fire)"
            );
        }
    }

    #[test]
    fn discrete_in_limit_pulse_lands_on_integer_step_non_dyadic_dt() {
        // v13: with dt=0.1 the additive TIME clock sits just below each integer, so
        // the pre-fix floor(TIME) reset fired one dt late -- the step modeling
        // t=k.0 still saw the previous unit's exhausted budget and admitted 0, the
        // pulse instead landing at t~=k.1. The pulse must land on the t=k.0 step.
        let series = run_discrete_in_limit_series(0.0, 6.0, 0.1);
        assert_discrete_pulses(&series, 0.1, 6);
    }

    #[test]
    fn discrete_in_limit_pulse_dyadic_dt_unchanged() {
        // A dyadic dt (0.25) is exactly representable: TIME never drifts and the
        // boundary detection is unchanged by the fix -- a no-regression guard.
        let series = run_discrete_in_limit_series(0.0, 4.0, 0.25);
        assert_discrete_pulses(&series, 0.25, 4);
    }

    #[test]
    fn discrete_in_limit_reset_thirds_dt_lands_on_grid() {
        // dt=1/3: three steps sum to 0.999...9 (floor 0), but the ideal grid time
        // at k=3 is exactly 1.0, so the reset lands on step 3 (t=1), step 6 (t=2),
        // ... -- the first grid step at or past each integer.
        let dt = 1.0 / 3.0;
        let series = run_discrete_in_limit_series(0.0, 6.0, dt);
        assert_discrete_pulses(&series, dt, 6);
    }

    #[test]
    fn discrete_in_limit_no_drift_over_long_run() {
        // Drift grows with the step count: run to t=50 (500 steps at dt=0.1) and
        // assert every time unit still admits exactly in_limit=5 on its first step
        // -- the reset never slips a dt late nor double-fires.
        let series = run_discrete_in_limit_series(0.0, 50.0, 0.1);
        assert_discrete_pulses(&series, 0.1, 50);
    }

    // ---- §7.2 explicit per-slat / per-time-unit list initialization ----

    /// [`wrap_model_specs`] with start pinned to 0, for §7.2 tests whose slat
    /// arithmetic reads cleanest at dt = 1 or dt = 0.5.
    fn wrap_model_init(vars: &str, dt: f64, stop: f64) -> String {
        wrap_model_specs(vars, 0.0, stop, dt)
    }

    fn run_series(xml: &str, names: &[&str]) -> Vec<Vec<f64>> {
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        names
            .iter()
            .map(|n| vm.get_series(&Ident::new(n)).expect(n))
            .collect()
    }

    #[test]
    fn explicit_list_len_n_fills_slats_front_first() {
        // §7.2 length-N form: transit 3, dt 1 -> N = 3 slats; "10, 20, 30"
        // fills the belt directly, entry 1 at the front (exit) slat. With no
        // inflow the belt drains front-first: 60 -> 50 -> 30 -> 0, and the
        // driven outflow rate is 10, then 20, then 30.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>3</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            5.0,
        );
        let series = run_series(&xml, &["belt", "out_f"]);
        let (belt, out) = (&series[0], &series[1]);
        for (i, want) in [60.0, 50.0, 30.0, 0.0, 0.0].iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "belt[{i}] = {} (want {want})",
                belt[i]
            );
        }
        // out_f[i] is the driven rate during step [i, i+1): the front (exit)
        // slat's 10 leaves first, then 20, then 30.
        for (i, want) in [10.0, 20.0, 30.0].iter().enumerate() {
            assert!(
                (out[i] - want).abs() < 1e-9,
                "out_f[{i}] = {} (want {want})",
                out[i]
            );
        }
    }

    #[test]
    fn explicit_list_per_time_unit_spreads_continuous() {
        // §7.2 per-time-unit form: transit 2, dt 0.5 -> N = 4 slats but
        // U = floor(3 * 0.5) + 1 = 2 time-unit blocks, so a length-2 list is
        // one entry PER TIME UNIT: [40, 80] spreads to [20, 20, 40, 40]
        // (each block's entry split evenly across its slats on a continuous
        // conveyor, so the outflow during unit u totals v_u). Belt series:
        // 120 -> 100 -> 80 -> 40 -> 0.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>40, 80</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            0.5,
            4.0,
        );
        let series = run_series(&xml, &["belt"]);
        let belt = &series[0];
        for (i, want) in [120.0, 100.0, 80.0, 40.0, 0.0].iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "belt[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }

    #[test]
    fn explicit_list_short_list_normalizes_and_stock_reports_belt_total() {
        // §7.2 normalization: transit 4, dt 1 -> N = U = 4; the length-2 list
        // [10, 20] repeats its last entry -> [10, 20, 20, 20], total 70. The
        // stock must report the NORMALIZED belt total (70) at t = 0, not the
        // raw list sum (30) -- and both INIT(belt) and a dependent initial
        // must see 70 (the post-belt-init reconcile).
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="init_check"><eqn>INIT(belt)</eqn></aux>
        <stock name="mirror"><eqn>belt</eqn></stock>"#,
            1.0,
            6.0,
        );
        let series = run_series(&xml, &["belt", "init_check", "mirror"]);
        let (belt, init_check, mirror) = (&series[0], &series[1], &series[2]);
        assert!(
            (belt[0] - 70.0).abs() < 1e-9,
            "belt[0] = {} (want the normalized belt total 70, not the raw sum 30)",
            belt[0]
        );
        for (i, &v) in init_check.iter().enumerate() {
            assert!(
                (v - 70.0).abs() < 1e-9,
                "init_check[{i}] = {v} (want INIT(belt) = 70)"
            );
        }
        assert!(
            (mirror[0] - 70.0).abs() < 1e-9,
            "mirror[0] = {} (a dependent initial must see the belt total 70)",
            mirror[0]
        );
    }

    #[test]
    fn explicit_list_long_list_truncates() {
        // §7.2 normalization: transit 2, dt 1 -> N = U = 2; the length-3 list
        // [10, 20, 30] truncates to [10, 20], total 30 (raw sum 60).
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            4.0,
        );
        let series = run_series(&xml, &["belt"]);
        let belt = &series[0];
        assert!(
            (belt[0] - 30.0).abs() < 1e-9,
            "belt[0] = {} (want the truncated total 30, not the raw sum 60)",
            belt[0]
        );
        assert!((belt[1] - 20.0).abs() < 1e-9, "belt[1] = {}", belt[1]);
        assert!(belt[2].abs() < 1e-9, "belt[2] = {}", belt[2]);
    }

    #[test]
    fn explicit_list_tolerates_whitespace_and_trailing_comma() {
        // Stella-authored lists may carry spaces and a trailing comma; both are
        // accepted (transit 3, dt 1 -> the length-3 direct fill of 60 total).
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn> 10 ,20,  30, </eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>3</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            4.0,
        );
        let series = run_series(&xml, &["belt"]);
        assert!(
            (series[0][0] - 60.0).abs() < 1e-9,
            "belt[0] = {} (want 60)",
            series[0][0]
        );
    }

    #[test]
    fn explicit_list_negative_entries_flow_through() {
        // The spec is silent on sign; scalar init (§7.1) accepts any V, so the
        // list form does too -- a negative entry rides the belt and exits as a
        // negative outflow rather than being silently clamped.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>-10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            4.0,
        );
        let series = run_series(&xml, &["belt", "out_f"]);
        let (belt, out) = (&series[0], &series[1]);
        assert!((belt[0] - 10.0).abs() < 1e-9, "belt[0] = {}", belt[0]);
        assert!(
            (out[0] - -10.0).abs() < 1e-9,
            "out_f[0] = {} (the front slat's -10 exits first)",
            out[0]
        );
    }

    #[test]
    fn explicit_list_non_constant_entry_rejected() {
        // A list-shaped <eqn> with a non-constant entry is rejected loudly at
        // expansion time (naming the entry), never silently steady-state
        // initialized and never surfaced as an opaque parse error.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, some_var, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>3</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="some_var"><eqn>20</eqn></aux>"#,
            1.0,
            4.0,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("non-constant list entry must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("some_var"),
            "diagnostic should name the offending entry: {details}"
        );
    }

    #[test]
    fn function_call_initial_is_not_mistaken_for_list() {
        // A scalar initial CONTAINING commas inside a call ("MAX(600, 300)") is
        // an ordinary §7.1 scalar init, not a §7.2 list: the belt steady-fills
        // to 600 and holds (inflow 150 = 600 / transit 4 keeps it steady).
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>MAX(600, 300)</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f"><eqn>150</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            8.0,
        );
        let series = run_series(&xml, &["belt"]);
        for (i, &b) in series[0].iter().enumerate() {
            assert!(
                (b - 600.0).abs() < 1e-6,
                "belt[{i}] = {b} (want steady 600)"
            );
        }
    }

    #[test]
    fn arrayed_a2a_explicit_list_shared_across_belts() {
        // An apply-to-all arrayed conveyor shares one list across every element
        // belt (the same one-expression-per-attribute rule as <len>, §10):
        // both belts fill [10, 20] and drain independently.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20</eqn>
          <inflow>in_f</inflow><outflow>out_f</outflow>
          <dimensions><dim name="board"/></dimensions>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
        <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
            1.0,
            4.0,
        )
        .replace(
            "<model>",
            "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
        );
        let series = run_series(&xml, &["belt[a]", "belt[b]"]);
        for (name, belt) in [("belt[a]", &series[0]), ("belt[b]", &series[1])] {
            for (i, want) in [30.0, 20.0, 0.0].iter().enumerate() {
                assert!(
                    (belt[i] - want).abs() < 1e-9,
                    "{name}[{i}] = {} (want {want})",
                    belt[i]
                );
            }
        }
    }

    #[test]
    fn arrayed_per_element_explicit_list_rejected() {
        // A per-element <element> equation carrying a list is rejected loudly:
        // only the scalar and shared apply-to-all list forms are supported
        // (never a silent parse error or a steady-init fallback).
        let xml = wrap_model_init(
            r#"
        <stock name="belt">
          <inflow>in_f</inflow><outflow>out_f</outflow>
          <dimensions><dim name="board"/></dimensions>
          <element subscript="a"><eqn>10, 20</eqn></element>
          <element subscript="b"><eqn>5</eqn></element>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>
        <flow name="out_f"><eqn>0</eqn><dimensions><dim name="board"/></dimensions></flow>"#,
            1.0,
            4.0,
        )
        .replace(
            "<model>",
            "<dimensions><dim name=\"board\"><elem name=\"a\"/><elem name=\"b\"/></dim></dimensions><model>",
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("per-element list must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
    }

    #[test]
    fn explicit_list_survives_xmile_roundtrip() {
        // The reader must preserve the list <eqn> verbatim on the datamodel
        // (the expansion rewrites only its private clone) and the writer must
        // re-emit it, so a §7.2 model round-trips losslessly.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>3</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            4.0,
        );
        let project = parse(&xml);
        let stock_eqn = project.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Stock(s) if s.ident == "belt" => Some(s.equation.clone()),
                _ => None,
            })
            .expect("belt stock");
        assert_eq!(stock_eqn, Equation::Scalar("10, 20, 30".to_string()));

        // Building a VM must not mutate the caller's project.
        let main = project.models[0].name.clone();
        build_vm(&project, &main).expect("build");
        let after = project.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Stock(s) if s.ident == "belt" => Some(s.equation.clone()),
                _ => None,
            })
            .expect("belt stock");
        assert_eq!(after, Equation::Scalar("10, 20, 30".to_string()));

        let out = crate::compat::to_xmile(&project).expect("to_xmile");
        assert!(
            out.contains("10, 20, 30"),
            "writer must re-emit the list eqn: {out}"
        );
    }

    #[test]
    fn explicit_list_produces_no_error_diagnostics() {
        // The editor diagnostic path parses the UN-expanded project; a valid
        // §7.2 list initial must not surface as a spurious equation parse
        // error there (the runtime path accepts it, so diagnostics must too).
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20, 30</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>3</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            4.0,
        );
        let project = parse(&xml);
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let diags = crate::db::collect_all_diagnostics(&db, sync.project);
        let errors: Vec<_> = diags
            .iter()
            .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
            .collect();
        assert!(
            errors.is_empty(),
            "a valid explicit-list initial must not produce Error diagnostics: {errors:?}"
        );
    }

    /// The list conveyor every placeholder-leak regression below builds on:
    /// transit 4, dt 1 -> N = U = 4; the short list [10, 20] normalizes to
    /// [10, 20, 20, 20] = 70 while its RAW sum is 30. Any init-time consumer
    /// that sees 30 instead of 70 reproduces the leak.
    const LIST_BELT_A: &str = r#"
        <stock name="belt_a"><eqn>10, 20</eqn><inflow>in_a</inflow><outflow>out_a</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_a"><eqn>0</eqn></flow>
        <flow name="out_a"><eqn>0</eqn></flow>"#;

    #[test]
    fn chained_conveyor_initialized_from_list_stock_sees_normalized_total() {
        // A SECOND conveyor whose scalar initial reads the list stock must
        // steady-fill from the normalized total (70), not the raw list sum
        // (30): belt init runs BEFORE the post-init reconcile, so a raw-sum
        // placeholder would fill belt_b's belt from 30 while the reconcile
        // bumps its STOCK slot to 70 -- a belt permanently inconsistent with
        // its own stock (stock stuck at 40 with an empty belt, conservation
        // violated). A correct fill is 70 spread over 2 slats: [35, 35], so
        // the series drains 70 -> 35 -> 0 and the outflow returns the full 70.
        let vars = format!(
            r#"{LIST_BELT_A}
        <stock name="belt_b"><eqn>belt_a</eqn><inflow>in_b</inflow><outflow>out_b</outflow>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_b"><eqn>0</eqn></flow>
        <flow name="out_b"><eqn>0</eqn></flow>"#
        );
        let xml = wrap_model_init(&vars, 1.0, 6.0);
        let series = run_series(&xml, &["belt_b", "out_b"]);
        let (belt_b, out_b) = (&series[0], &series[1]);
        for (i, want) in [70.0, 35.0, 0.0, 0.0].iter().enumerate() {
            assert!(
                (belt_b[i] - want).abs() < 1e-9,
                "belt_b[{i}] = {} (want {want}); belt/stock inconsistent",
                belt_b[i]
            );
        }
        let drained: f64 = out_b[0] + out_b[1];
        assert!(
            (drained - 70.0).abs() < 1e-9,
            "outflow must return the full normalized 70, got {drained}"
        );
    }

    #[test]
    fn queue_seeded_from_list_stock_sees_normalized_total() {
        // A queue whose initial reads the list stock seeds its FIFO in
        // init_queues, which runs after belt init but reads the stock slot the
        // INITIALS pass wrote -- so a raw-sum placeholder seeds a 30 batch
        // while the reconcile reports the stock as 70: SUM(waiting) and
        // waiting disagree at t = 0. Both must be the normalized 70.
        let vars = format!(
            r#"{LIST_BELT_A}
        <stock name="waiting"><eqn>belt_a</eqn><inflow>arrivals</inflow><outflow>into_service</outflow>
          <queue/></stock>
        <flow name="arrivals"><eqn>0</eqn><non_negative/></flow>
        <flow name="into_service"><eqn>0</eqn></flow>
        <aux name="q_total"><eqn>SUM(waiting)</eqn></aux>"#
        );
        let xml = wrap_model_init(&vars, 1.0, 4.0);
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let mut vm = crate::queue_compile::build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let waiting = vm.get_series(&Ident::new("waiting")).expect("waiting");
        let q_total = vm.get_series(&Ident::new("q_total")).expect("q_total");
        assert!(
            (waiting[0] - 70.0).abs() < 1e-9,
            "waiting[0] = {} (want 70)",
            waiting[0]
        );
        assert!(
            (q_total[0] - 70.0).abs() < 1e-9,
            "SUM(waiting)[0] = {} (want 70; the FIFO batch must match the stock)",
            q_total[0]
        );
    }

    #[test]
    fn transit_reading_list_stock_uses_normalized_total() {
        // A conveyor whose <len> references the list stock evaluates it in the
        // pre-init_belts Flows run -- before any write-back -- so a raw-sum
        // placeholder yields transit 30/35 ~= 0.86 (1 slat) instead of
        // 70/35 = 2 (2 slats). With 2 slats a steady 70 drains [35, 35].
        let vars = format!(
            r#"{LIST_BELT_A}
        <stock name="belt_c"><eqn>70</eqn><inflow>in_c</inflow><outflow>out_c</outflow>
          <conveyor><len>belt_a / 35</len></conveyor></stock>
        <flow name="in_c"><eqn>0</eqn></flow>
        <flow name="out_c"><eqn>0</eqn></flow>"#
        );
        let xml = wrap_model_init(&vars, 1.0, 6.0);
        let series = run_series(&xml, &["belt_c"]);
        let belt_c = &series[0];
        for (i, want) in [70.0, 35.0, 0.0].iter().enumerate() {
            assert!(
                (belt_c[i] - want).abs() < 1e-9,
                "belt_c[{i}] = {} (want {want}; transit must see 70/35 = 2)",
                belt_c[i]
            );
        }
    }

    #[test]
    fn explicit_list_with_non_constant_transit_rejected() {
        // The list-length interpretation (per-slat vs per-time-unit) and the
        // stock's compile-time initial total both depend on the slat count, so
        // a list-initialized conveyor requires a compile-time-constant
        // <len>. A runtime transit expression is rejected loudly.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>tt</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>
        <aux name="tt"><eqn>4</eqn></aux>"#,
            1.0,
            4.0,
        );
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("non-literal transit must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorInitListUnsupported);
        let details = err.get_details().unwrap_or_default();
        assert!(
            details.contains("numeric literal"),
            "diagnostic should explain the literal-transit requirement: {details}"
        );
    }

    #[test]
    fn model_sim_specs_dt_override_drives_list_normalization() {
        // The runtime prefers the ROOT MODEL's sim_specs override over the
        // project's (assemble.rs "preferring model-level sim_specs"), so the
        // expansion-time §7.2 probe must read the same dt. Project dt = 1 but
        // the model overrides dt = 1.5: transit 2 at dt 1.5 -> N =
        // round(2/1.5) = 1 slat, U = 1, so the 2-entry list truncates to [10]
        // and the stock must report 10. A probe reading PROJECT dt (1) sizes
        // N = 2 (direct fill) and bakes 30 into the placeholder -- caught by
        // the init_belts debug_assert in debug, silent divergence in release.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>10, 20</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            6.0,
        );
        let mut project = parse(&xml);
        project.models[0].sim_specs = Some(datamodel::SimSpecs {
            start: 0.0,
            stop: 6.0,
            dt: datamodel::Dt::Dt(1.5),
            save_step: None,
            sim_method: datamodel::SimMethod::Euler,
            time_units: None,
        });
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build");
        vm.run_to_end().expect("run");
        let belt = vm.get_series(&Ident::new("belt")).expect("belt");
        assert!(
            (belt[0] - 10.0).abs() < 1e-9,
            "belt[0] = {} (want 10, the dt-1.5 one-slat truncation; a project-dt probe bakes 30)",
            belt[0]
        );
    }

    #[test]
    fn model_sim_specs_rk4_override_rejected_for_conveyor() {
        // The Euler-only gate must read the ROOT MODEL's sim_specs override,
        // not just the project's: a model-level RK4 override would otherwise
        // evade ConveyorNonEulerMethod and integrate the belt under RK.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>2</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            4.0,
        );
        let mut project = parse(&xml);
        project.models[0].sim_specs = Some(datamodel::SimSpecs {
            start: 0.0,
            stop: 4.0,
            dt: datamodel::Dt::Dt(0.25),
            save_step: None,
            sim_method: datamodel::SimMethod::RungeKutta4,
            time_units: None,
        });
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("model-level RK4 override must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorNonEulerMethod);
    }

    #[test]
    fn explicit_list_single_entry_with_trailing_comma() {
        // "5," is a one-entry list (the trailing comma is what makes it a
        // list): per §7.2 short-list normalization the single entry repeats
        // for every time unit -> [5, 5, 5] on a 3-slat dt-1 belt, total 15.
        // Before this was handled it fell through to an opaque parse error
        // while "10, 20," was accepted.
        let xml = wrap_model_init(
            r#"
        <stock name="belt"><eqn>5,</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>3</len></conveyor></stock>
        <flow name="in_f"><eqn>0</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#,
            1.0,
            5.0,
        );
        let series = run_series(&xml, &["belt"]);
        let belt = &series[0];
        for (i, want) in [15.0, 10.0, 5.0, 0.0].iter().enumerate() {
            assert!(
                (belt[i] - want).abs() < 1e-9,
                "belt[{i}] = {} (want {want})",
                belt[i]
            );
        }
    }
}
