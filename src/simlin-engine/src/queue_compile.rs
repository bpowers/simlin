// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Compiling queues into the VM.
//!
//! The queue runtime pass ([`crate::queue`]) is VM-native, not bytecode
//! (docs/design/queues.md §10.3), exactly like conveyors ([`crate::conveyor`] /
//! [`crate::conveyor_compile`]). Rather than teach the salsa compiler about FIFO
//! batches, this module bridges the datamodel to the VM in two steps that
//! bracket ordinary compilation, mirroring [`crate::conveyor_compile`]:
//!
//! 1. [`expand_queues`] rewrites a project so every queue stock stays an
//!    ordinary INTEG stock (its placeholder `<eqn>` preserved) whose driven
//!    outflows get a placeholder `0` equation -- so each compiles to a writable
//!    slot instead of erroring on an empty equation -- and CLEARS the `<queue/>`
//!    marker (and each outflow's `<overflow/>` marker) so the ordinary compile
//!    path integrates the stock normally. A [`QueueMeta`] per queue records the
//!    stock, its inflows, and its driven outflows.
//! 2. [`resolve_plans`] looks each name up in the compiled simulation's offset
//!    map to produce [`QueuePlan`]s the VM's queue pass reads.
//!
//! Because the queue stock's inflows/outflows carry the pass-computed rates
//! before stock integration, the stock integrates to `Σ batches` through the
//! ordinary Stocks phase -- no special-casing of stock integration is needed.
//! The §4.1 conservation identity `Δqueue = Σ inflow − Σ outflow` guarantees the
//! ordinary Stocks phase produces exactly the total the [`QueueState`] side
//! table holds (admit added `Σ f_in · dt`, serve removed `Σ f_out · dt`).
//!
//! Scope: an UNCONSTRAINED queue whose outflow(s) target a cloud or a regular
//! (non-conveyor) stock empties the queue every DT (§4.3); a queue whose primary
//! outflow feeds a discrete conveyor is COUPLED (§9), served under the batch rules
//! in the combined pass ([`run_coupled_passes`]). Container access (§8) and
//! arrayed queues (§6) are supported.
//!
//! MULTIPLE outflows, priority, and overflow (§4.5/§5) are supported: outflows are
//! served in `<outflow>` declaration = priority order, each popping from the same
//! front ([`serve_secondary_outflows`]). After the primary, an `<overflow/>` sibling
//! drains only the REDIRECTABLE volume -- the front material a capacity / inflow-limit
//! / arrest condition blocked the primary from taking, measured as `desire − taken`
//! ([`crate::queue::QueueState::conveyor_desire`]) -- while an ordinary competing
//! outflow drains the whole remainder (§5.4). An `<overflow/>` behind an UNCONSTRAINED
//! primary (no upstream conveyor) drains nothing: the primary is never blocked, so
//! redirectable = 0 (§4.5). A SECONDARY outflow bound to a conveyor (a constrained
//! overflow or a second constrained ordinary outflow) is REJECTED at
//! coupling-detection time ([`ErrorCode::QueueSecondaryOutflowToConveyor`]): only
//! the primary may feed a conveyor, and serving a secondary to a (possibly distinct)
//! second belt under the batch rules is a deferred feature (§4.5 sketches it but
//! leaves the redirectable-vs-admission-budget interleave undefined), so it is a
//! loud error rather than silently mis-accounted. Every secondary reaching
//! [`serve_secondary_outflows`] therefore targets a cloud or regular stock.
//!
//! Queues and conveyors COEXIST and COUPLE: the unified [`compile_sim`] /
//! [`build_compiled`] here expand conveyors first, then queues, compile ONCE,
//! resolve BOTH plan sets against the same offset map, and then [`apply_couplings`]
//! detects each queue outflow that feeds a discrete conveyor (enforcing the
//! `ConveyorQueueUpstreamNotDiscrete` requirement) and wires the coupling INTO the
//! two plan sets (a `queue_coupled` conveyor inflow + a [`QueueOutflowKind::Coupled`]
//! queue outflow), so no separate structure threads through the VM or libsimlin.
//! The VM carries both side tables plus a [`CouplingTable`] derived from the
//! plans once at attach time (the coupling is compile-time constant, so it is
//! not rebuilt per step -- GH #878) and runs [`run_coupled_passes`] between the
//! Flows and Stocks phases -- interleaving each coupled queue's serve between its
//! conveyor's phase A and phase B (several queues MAY feed one discrete conveyor,
//! served in the belt's `<inflow>` declaration order under a shared admission
//! budget -- the listed-order admission priority of conveyors.md §4.3 step 4 /
//! §11), and delegating to the two independent passes when nothing is coupled.

use std::collections::HashMap;

use crate::common::{Canonical, DimensionName, ErrorCode, Ident};
use crate::conveyor_compile::{
    ContainerMeta, ContainerNaming, ContainerPlan, ContainerVarSpec, canon,
    container_value_from_slice, element_offset, element_subscripts_for_dims, equation_dims,
    find_driven_flow_read, main_model_has_stock, n_elements, placeholder_zero_equation,
    resolve_container_plans, rewrite_model_container_equations, set_variable_equation,
    synthesize_container_stocks,
};
use crate::datamodel::{self, Equation};

/// The downstream target a queue outflow drains into (docs/design/queues.md §4).
/// [`expand_queues`] produces only [`QueueOutflowKind::Unconstrained`]; the
/// coupling resolution ([`apply_couplings`]) rewrites a queue outflow that feeds
/// a discrete conveyor to [`QueueOutflowKind::Coupled`] (§4.4/§9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOutflowKind {
    /// A cloud or a regular (non-conveyor) stock: the outflow always empties the
    /// queue (§4.3).
    Unconstrained,
    /// A discrete conveyor directly downstream (§4.4/§9): the shared flow is BOTH
    /// this queue's primary outflow AND the conveyor's single equation-driven
    /// inflow. It is served in the combined queue-conveyor pass
    /// ([`run_coupled_passes`]) -- the conveyor sizes an admission budget `req`
    /// from its capacity/inflow-limit and this queue supplies from the front under
    /// the batch rules -- NOT by the unconstrained [`run_queue_pass`].
    Coupled {
        /// Index into the CONVEYOR plan list of the coupled discrete conveyor
        /// (resolved by matching the shared flow slot). The combined pass sizes
        /// `req` from this conveyor's belt after its phase A.
        conveyor: usize,
        /// `one_at_a_time` from the conveyor's block (default true): take at most
        /// the single front batch per DT (conveyors.md §11).
        one_at_a_time: bool,
        /// `batch_integrity` from the conveyor's block (default false): never
        /// split a batch (conveyors.md §11).
        batch_integrity: bool,
    },
}

/// A queue outflow's synthesized metadata: the driven flow's canonical name plus
/// its target kind. [`expand_queues`] always records `Unconstrained` here; a
/// coupling is detected and stamped onto the resolved [`QueueOutflowPlan`] later
/// (by [`apply_couplings`]), since it needs the compiled conveyor plan indices.
///
/// `Debug` is derived UNCONDITIONALLY (like [`crate::conveyor_compile::InflowMeta`]):
/// these metas are pure compile-time data (never in the VM's `debug-derive`-gated
/// side tables), so there is no no-default-features WASM-build hazard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueOutflowMeta {
    /// Canonical name of the driven outflow (its slot receives the served rate).
    pub flow: String,
    pub kind: QueueOutflowKind,
    /// The `<overflow/>` marker (§3.3): an overflow outflow activates ONLY on the
    /// redirectable volume a higher-priority sibling was blocked from taking
    /// (§4.5), NOT on the remaining front at large. False for the primary (an
    /// overflow may never be the first outflow, enforced by
    /// [`validate_overflow_markers`]) and for an ordinary competing outflow (§5.4).
    pub overflow: bool,
}

/// Per-queue synthesized metadata, produced by [`expand_queues`] and resolved to
/// offsets by [`resolve_plans`]. Mirrors [`crate::conveyor_compile::ConveyorMeta`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueMeta {
    /// Canonical name of the queue stock.
    pub stock: String,
    /// Canonical names of the queue's inflows (summed into the admit rate, §4.2
    /// step 1). Empty for a queue with no inflow (it only drains, or -- with no
    /// outflow either -- accumulates as a pure batch container).
    pub inflows: Vec<String>,
    /// The driven outflows in `<outflow>` declaration = priority order (§5.1).
    pub outflows: Vec<QueueOutflowMeta>,
    /// Container-access variables reading this queue's batches (§8). Each is a
    /// synthesized hidden stock whose slot the pass publishes at step-start. For
    /// an arrayed queue the container variable is arrayed over the same dims, so
    /// element `e` of the container aligns with queue element `e`. Reuses the
    /// conveyor's [`ContainerMeta`] verbatim (only the source vector differs).
    pub containers: Vec<ContainerMeta>,
    /// Per-element subscript suffixes for an arrayed queue (§6), in the same
    /// row-major order the compiled offset map lays out an arrayed variable's
    /// elements (via the shared `element_subscripts_for_dims`/`SubscriptIterator`
    /// helper). Each entry is the canonical `elem1,elem2` suffix so
    /// [`resolve_plans`] can form the subscripted offset keys `name[elem]`. An
    /// arrayed queue is N independent FIFOs, one per element; empty for a scalar
    /// queue (the degenerate 1-element case).
    pub element_subscripts: Vec<String>,
}

/// A resolved queue outflow: its data-buffer slot offset plus its target kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueOutflowPlan {
    pub flow_off: usize,
    pub kind: QueueOutflowKind,
    /// The `<overflow/>` marker (§3.3/§4.5); see [`QueueOutflowMeta::overflow`].
    pub overflow: bool,
}

/// A fully-resolved queue: data-buffer slot offsets for the VM's pass. Mirrors
/// [`crate::conveyor_compile::ConveyorPlan`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuePlan {
    /// Slot of the queue stock (integrated by the ordinary Stocks phase from the
    /// pass-written flow rates; the side table tracks the same total, §4.1).
    pub stock_off: usize,
    /// Slots of the inflows whose rates are summed into the admit volume (§4.2).
    pub inflow_offs: Vec<usize>,
    /// The driven outflows in priority order; the pass writes each served rate.
    pub outflows: Vec<QueueOutflowPlan>,
    /// Container-access variables reading this queue's batches (§8): each carries a
    /// resolved slot offset and access kind. The pass publishes each from this
    /// queue's start-of-step batch state. Reuses the conveyor's [`ContainerPlan`].
    pub containers: Vec<ContainerPlan>,
}

impl QueuePlan {
    /// Every data-buffer slot the queue pass WRITES each step: the driven
    /// outflow rates (the serve paths) and the published container-access
    /// values ([`publish_queue_container_values`]). Pass-owned slots must not
    /// be overridable -- the placeholder `0` a driven outflow compiled to is
    /// overwritten every step, so an accepted override would be silently
    /// ineffective (GH #871). Mirrors
    /// [`crate::conveyor_compile::ConveyorPlan::pass_written_offsets`],
    /// including why containers are listed and why INFLOW slots are not: the
    /// pass reads each inflow as the admit request (clamping negatives in
    /// place), so a constant-inflow override is a genuine input each step.
    pub fn pass_written_offsets(&self) -> impl Iterator<Item = usize> + '_ {
        self.outflows
            .iter()
            .map(|o| o.flow_off)
            .chain(self.containers.iter().map(|c| c.off))
    }
}

/// Does the named model in `project` contain any queue stock? The cheap predicate
/// [`compile_sim`] uses to decide whether to route through the special stock-type
/// build path instead of the ordinary incremental compile. Mirrors
/// [`crate::conveyor_compile::project_has_conveyor`].
///
/// It scans for a marked STOCK, so it deliberately does NOT see an `<overflow/>`
/// marker (which rides on a flow); [`compile_sim`] validates those separately on
/// both branches.
pub fn project_has_queue(project: &datamodel::Project, main_model: &str) -> bool {
    main_model_has_stock(project, main_model, |s| s.compat.queue.is_some())
}

/// One model variable, projected down to just what `<overflow/>` marker validation
/// reads. The projection exists so ONE algorithm
/// ([`validate_overflow_markers_over`]) serves both representations of a model: the
/// `datamodel::Model` that [`expand_queues`] holds, and the salsa `SourceModel`
/// inputs that [`crate::db::compile_project_incremental`] holds. Two hand-written
/// twins would be free to drift, and this rule is the sole thing standing between a
/// stray overflow marker and a silently mis-simulated flow.
pub(crate) struct MarkerVar<'a> {
    /// The variable's AS-WRITTEN ident. Error messages quote it, so it must carry
    /// the user's own spelling, not the canonical form.
    pub ident: &'a str,
    /// `Some(outflows)` iff this is a queue-marked STOCK, in declared order (the
    /// first entry is the highest-priority outflow).
    pub queue_outflows: Option<&'a [String]>,
    /// True iff this is a FLOW carrying an `<overflow/>` marker.
    pub overflow_flow: bool,
}

/// Validate every `<overflow/>` marker among `vars`, the variables of ONE model in
/// declaration order (docs/design/queues.md §3.3, §10.7).
///
/// XMILE (§4.3) allows the marker ONLY on a queue outflow, and NEVER on a queue's
/// FIRST (highest-priority) outflow: an overflow is by definition a lower-priority
/// sibling that activates when a higher-priority outflow is blocked (§4.5). Both
/// violations are a loud [`ErrorCode::QueueOverflowNotOnQueue`].
///
/// A model with no queue stock has an empty outflow set, so every overflow flag in
/// it is rejected -- which is the point: the marker rides on a FLOW, so neither
/// special-stock dispatch predicate can see it, and a model can carry a stray
/// overflow with no queue anywhere. Declaration order is load-bearing only for
/// WHICH offender a multi-offender model reports; both callers preserve it.
pub(crate) fn validate_overflow_markers_over(
    vars: &[MarkerVar<'_>],
) -> Result<(), (ErrorCode, String)> {
    // Every queue outflow name, and specifically each queue's FIRST outflow.
    let mut queue_outflows: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut first_outflows: std::collections::HashSet<String> = std::collections::HashSet::new();
    for v in vars {
        let Some(outflows) = v.queue_outflows else {
            continue;
        };
        for (i, out) in outflows.iter().enumerate() {
            let c = canon(out);
            if i == 0 {
                first_outflows.insert(c.clone());
            }
            queue_outflows.insert(c);
        }
    }
    for v in vars {
        if !v.overflow_flow {
            continue;
        }
        let name = canon(v.ident);
        if !queue_outflows.contains(&name) {
            return Err((
                ErrorCode::QueueOverflowNotOnQueue,
                format!(
                    "flow '{}' is marked <overflow/> but is not a queue outflow; the overflow \
                     property may appear only on an outflow of a queue stock (XMILE §4.3)",
                    v.ident
                ),
            ));
        }
        if first_outflows.contains(&name) {
            return Err((
                ErrorCode::QueueOverflowNotOnQueue,
                format!(
                    "flow '{}' is a queue's first (highest-priority) outflow and cannot be \
                     marked <overflow/>; an overflow activates only when a higher-priority \
                     outflow is blocked, so it may never be the first outflow (XMILE §4.3)",
                    v.ident
                ),
            ));
        }
    }
    Ok(())
}

/// [`validate_overflow_markers_over`] applied to a `datamodel::Model`.
///
/// Called from [`expand_queues`], before its no-queue fast path. This is NOT the
/// guarantee that marker placement is checked everywhere -- `expand_queues` only
/// ever sees a model the stock-marker dispatch routed to it. The program-wide
/// guarantee comes from the salsa twin in
/// [`crate::db::compile_project_incremental`]: `db::assemble_simulation` has a
/// single production caller, so nothing compiles without passing that gate.
///
/// Both are kept because expansion CLEARS the overflow marker on the driven
/// outflows it rewrites: the "overflow on a queue's first outflow" violation is
/// invisible downstream, so it must be caught here, before the clear. The
/// complementary "not a queue outflow at all" violation survives expansion (a
/// stray flag is not on a driven flow, so nothing clears it) and is the twin's.
fn validate_overflow_markers(model: &datamodel::Model) -> Result<(), (ErrorCode, String)> {
    let vars: Vec<MarkerVar<'_>> = model
        .variables
        .iter()
        .map(|v| match v {
            datamodel::Variable::Stock(s) => MarkerVar {
                ident: &s.ident,
                queue_outflows: s.compat.queue.is_some().then_some(s.outflows.as_slice()),
                overflow_flow: false,
            },
            datamodel::Variable::Flow(f) => MarkerVar {
                ident: &f.ident,
                queue_outflows: None,
                overflow_flow: f.compat.overflow,
            },
            other => MarkerVar {
                ident: other.get_ident(),
                queue_outflows: None,
                overflow_flow: false,
            },
        })
        .collect();
    validate_overflow_markers_over(&vars)
}

/// Expand every queue in `main_model` of `project` into an ordinary INTEG stock
/// plus placeholder-equation driven outflows, returning the modified project and
/// one [`QueueMeta`] per queue. A project with no queues is returned unchanged
/// with an empty meta list (the caller can then skip all queue machinery).
///
/// An ARRAYED queue is N independent FIFOs, one per array element (§6): the
/// driven-outflow placeholder keeps the flow's array shape (so each element gets
/// its own writable slot) and [`resolve_plans`] flattens the one [`QueueMeta`]
/// into one scalar [`QueuePlan`] per element -- exactly mirroring
/// [`crate::conveyor_compile`]'s arrayed-conveyor flattening, reusing the same
/// `element_subscripts_for_dims`/`SubscriptIterator` helper. The scalar
/// admit-then-serve pass is the degenerate 1-element case.
///
/// Container access (`queue[k]`, `SUM/MEAN/MIN/MAX/STDDEV(queue)`, `SIZE(queue)`,
/// §8) reuses the conveyor container-access rewrite verbatim (only the source
/// vector differs): each supported subexpression is replaced by a reference to a
/// synthesized hidden STOCK whose slot the queue pass publishes at step-start.
///
/// Errors (this phase's scope guards):
/// - an equation that READS a queue driven outflow by name is rejected with
///   [`ErrorCode::QueueDrivenFlowRead`] (see the scan below and §2).
/// - a genuinely-unlowerable container-access residual is loud-rejected with the
///   shared `ConveyorContainerAccessUnsupported` (§8).
pub fn expand_queues(
    project: &datamodel::Project,
    main_model: &str,
) -> Result<(datamodel::Project, Vec<QueueMeta>), (ErrorCode, String)> {
    let main_canon = canon(main_model);
    let Some(main) = project.models.iter().find(|m| canon(&m.name) == main_canon) else {
        // No such model: nothing to expand (the caller's compile will report the
        // missing model through the ordinary path).
        return Ok((project.clone(), Vec::new()));
    };

    // Validate every `<overflow/>` marker BEFORE the no-queue fast path, so a stray
    // overflow on a flow that is not a queue outflow (or on a queue's first,
    // highest-priority outflow) is a loud error even in a model that would
    // otherwise skip all queue machinery (§3.3/§10.7).
    validate_overflow_markers(main)?;

    // Fast path: no queue anywhere in the main model. Return the project
    // unchanged so a conveyor-only or plain model compiles byte-identically.
    if !project_has_queue(project, main_model) {
        return Ok((project.clone(), Vec::new()));
    }

    let mut project = project.clone();
    let model_idx = project
        .models
        .iter()
        .position(|m| canon(&m.name) == main_canon)
        .expect("main model present (checked above)");

    // Pass 1 (immutable): collect metadata and the set of driven outflow names.
    // Canonical queue-stock name -> its array-dimension count (0 = scalar), used
    // by the container-access rewrite to tell an ordinary array-element read of an
    // arrayed queue apart from a batch read (§8). And -> its declared dimensions,
    // so a synthesized container variable can be arrayed over the same dims.
    let mut queue_dims: HashMap<String, usize> = HashMap::new();
    let mut queue_stock_dims: HashMap<String, Vec<DimensionName>> = HashMap::new();
    let mut metas: Vec<QueueMeta> = Vec::new();
    let model = &project.models[model_idx];
    // Canonical names of every flow carrying an `<overflow/>` marker (§3.3), so a
    // queue outflow can record whether it is an overflow. Validated above, so an
    // overflow name here is guaranteed to be a queue outflow (never the first one).
    let overflow_flows: std::collections::HashSet<String> = model
        .variables
        .iter()
        .filter_map(|v| match v {
            datamodel::Variable::Flow(f) if f.compat.overflow => Some(canon(&f.ident)),
            _ => None,
        })
        .collect();
    for v in &model.variables {
        let datamodel::Variable::Stock(stock) = v else {
            continue;
        };
        if stock.compat.queue.is_none() {
            continue;
        }
        // An arrayed queue is N independent FIFOs (§6). The stock's dimensions
        // drive the per-element offset enumeration; empty for a scalar queue.
        let stock_name = canon(&stock.ident);
        let stock_dims = equation_dims(&stock.equation);
        queue_dims.insert(stock_name.clone(), stock_dims.len());
        queue_stock_dims.insert(stock_name.clone(), stock_dims.clone());
        let element_subscripts =
            element_subscripts_for_dims(&project, &stock_dims, &stock.ident, "queue")?;
        let outflows = stock
            .outflows
            .iter()
            .map(|out| QueueOutflowMeta {
                flow: canon(out),
                // Every outflow starts unconstrained (§4.3). The coupling step (§9)
                // resolves the PRIMARY outflow's conveyor target to a constrained
                // kind; secondary outflows stay unconstrained (§5.4/§4.5).
                kind: QueueOutflowKind::Unconstrained,
                overflow: overflow_flows.contains(&canon(out)),
            })
            .collect();
        metas.push(QueueMeta {
            stock: stock_name,
            inflows: stock.inflows.iter().map(|f| canon(f)).collect(),
            outflows,
            containers: Vec::new(), // filled by the container-access rewrite below
            element_subscripts,
        });
    }

    // The set of driven outflow names (across all queues) whose flows become
    // placeholder-`0` writable slots.
    let driven: std::collections::HashSet<String> = metas
        .iter()
        .flat_map(|m| m.outflows.iter().map(|o| o.flow.clone()))
        .collect();

    // Reject any equation that READS a queue driven outflow by name: the queue
    // pass runs after the flows phase, so a reader would see the pre-pass
    // placeholder 0 instead of the served rate. Loud error, never silent (§2
    // "Driven outflow"). The scan is the SHARED `find_driven_flow_read` the
    // conveyor `ConveyorDrivenFlowRead` rejection uses (sorted iteration keeps
    // the named flow deterministic when an equation reads several); only the
    // error code and wording are queue-specific.
    //
    // Boundaries (identical to the conveyor scan):
    // - a driven outflow's OWN placeholder equation is not a reader (skipped);
    // - the structural `<inflow>`/`<outflow>` stock linkage is NOT an equation
    //   reference, so it is not scanned here -- a stock fed by the driven outflow
    //   via INTEG is CORRECT (the Stocks phase runs after the pass) and is not
    //   rejected.
    {
        let mut driven_sorted: Vec<String> = driven.iter().cloned().collect();
        driven_sorted.sort_unstable();
        if let Some((var, driven_flow)) =
            find_driven_flow_read(&project.models[model_idx], &driven, &driven_sorted)
        {
            return Err((
                ErrorCode::QueueDrivenFlowRead,
                format!(
                    "variable '{var}' references queue-driven flow '{driven_flow}'; a \
                     queue outflow cannot be read by another equation (it is computed \
                     after the flows phase)"
                ),
            ));
        }
    }

    // Rewrite each equation that uses a queue as a CONTAINER -- indexing into its
    // batches (`queue[k]`, constant `k`) or reducing over its batch volumes
    // (`SUM`/`SIZE`/`MEAN`/`MIN`/`MAX`/`STDDEV` of a single queue). This reuses
    // the conveyor container-access machinery verbatim (§8): the batch vector
    // lives in the VM's queue side table with a runtime-dynamic length, not in the
    // fixed-dimension data buffer, so each supported access is replaced by a
    // reference to a synthesized hidden no-flow STOCK whose slot the queue pass
    // publishes at step-start. A driven outflow's equation becomes a `0`
    // placeholder in Pass 2, so rewriting it would be discarded -- skip it.
    // Unlike conveyors, queues synthesize no parameter auxes, so there is nothing
    // extra to rewrite after this loop.
    let mut container_specs: std::collections::BTreeMap<String, ContainerVarSpec> =
        std::collections::BTreeMap::new();
    let mut rewritten_equations: HashMap<String, Equation> = rewrite_model_container_equations(
        &project.models[model_idx],
        &driven,
        &queue_dims,
        &ContainerNaming::QUEUE,
        &mut container_specs,
    )?;

    // Attach each container variable to its queue's meta and synthesize the hidden
    // container stock (arrayed over the queue's dims when arrayed).
    let container_stocks =
        synthesize_container_stocks(&container_specs, &queue_stock_dims, |owner, cm| {
            if let Some(meta) = metas.iter_mut().find(|m| m.stock == owner) {
                meta.containers.push(cm);
            }
        });

    // Pass 2 (mutable): apply the container-rewritten equations, give every driven
    // outflow a `0` placeholder equation so it compiles to a writable slot
    // (preserving its array shape so an arrayed queue keeps its per-element
    // slots), clear the queue/overflow markers so the expanded model compiles as a
    // plain stock-and-flow model, and append the synthesized container stocks.
    // Clearing the markers is what lets the ordinary compile path REJECT an
    // un-expanded queue (the marker is still set) while accepting this expanded one
    // -- exactly the `QueueNotExpanded` guard contract (§10.3).
    let model = &mut project.models[model_idx];
    for v in &mut model.variables {
        if let Some(new_eqn) = rewritten_equations.remove(&canon(v.get_ident())) {
            set_variable_equation(v, new_eqn);
        }
        match v {
            datamodel::Variable::Flow(f) if driven.contains(&canon(&f.ident)) => {
                // The queue stock now drives this outflow via the pass; give it a
                // writable placeholder slot and drop the overflow marker (it is an
                // ordinary flow after expansion).
                f.equation = placeholder_zero_equation(&f.equation);
                f.compat.overflow = false;
            }
            datamodel::Variable::Stock(s) if s.compat.queue.is_some() => {
                // The FIFO is now driven by the pass; the expanded stock is an
                // ordinary INTEG whose Δ = Σ inflow − Σ outflow (§4.1), so drop
                // the queue marker.
                s.compat.queue = None;
            }
            _ => {}
        }
    }
    // Append the synthesized container stocks (no-flow INTEGs the pass drives).
    for stock in container_stocks {
        model.variables.push(datamodel::Variable::Stock(stock));
    }

    Ok((project, metas))
}

/// Resolve [`QueueMeta`] names to data-buffer offsets using the compiled
/// simulation's offset map (docs/design/queues.md §10.3), flattening each arrayed
/// queue into ONE [`QueuePlan`] per array element (§6). An arrayed variable's
/// elements occupy contiguous slots keyed `name[elem1,elem2]` in the offset map
/// (`db::layout::flattened_offsets`), so element `e` resolves via the
/// subscripted key built from `meta.element_subscripts[e]`; a scalar queue
/// resolves its bare name and yields a single plan (so the per-queue runtime pass
/// is identical to before). Returns `None` if any required name is missing -- an
/// internal inconsistency between expansion and compilation that
/// [`build_compiled`] surfaces as a hard `NotSimulatable` error (there is no
/// non-queue fallback: the model has queues). Mirrors
/// [`crate::conveyor_compile::resolve_plans`].
pub fn resolve_plans(
    metas: &[QueueMeta],
    offsets: &HashMap<Ident<Canonical>, usize>,
) -> Option<Vec<QueuePlan>> {
    let total: usize = metas
        .iter()
        .map(|m| n_elements(&m.element_subscripts))
        .sum();
    let mut plans = Vec::with_capacity(total);
    for meta in metas {
        for e in 0..n_elements(&meta.element_subscripts) {
            // Element-aware offset resolver: the bare name for a scalar queue, the
            // `name[elem]` subscripted key for element `e` of an arrayed one.
            let eoff = |name: &str| element_offset(offsets, &meta.element_subscripts, e, name);
            let inflow_offs = meta
                .inflows
                .iter()
                .map(|f| eoff(f))
                .collect::<Option<Vec<_>>>()?;
            let outflows = meta
                .outflows
                .iter()
                .map(|o| {
                    Some(QueueOutflowPlan {
                        flow_off: eoff(&o.flow)?,
                        kind: o.kind,
                        overflow: o.overflow,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            // Container variables read this FIFO (§8). The container stock is
            // arrayed over the queue's dims, so element `e` of the container
            // resolves to FIFO `e` via the same element-aware offset lookup.
            let containers = resolve_container_plans(&meta.containers, eoff)?;
            plans.push(QueuePlan {
                stock_off: eoff(&meta.stock)?,
                inflow_offs,
                outflows,
                containers,
            });
        }
    }
    Some(plans)
}

// ----- runtime: queue initialization and the per-DT pass -----

/// Build the queue side table from the initials-populated data buffer (`curr`),
/// seeding each queue from its stock's initial value `V` (§7). The stock `<eqn>`
/// was evaluated by the initials pass, so `curr[stock_off]` holds `V`: `V > 0`
/// seeds a single front batch, `V <= 0` starts empty.
///
/// Unlike conveyor belt init, this needs NO extra Flows evaluation: a queue's
/// only dynamic init input is the stock's initial value, which IS in the initials
/// runlist (the stock's own `<eqn>`), whereas a conveyor's transit/capacity live
/// in synthesized auxes nothing depends on (so they are absent from the initials
/// runlist and the conveyor path runs an extra Flows eval to populate them).
pub fn init_queues(plans: &[QueuePlan], curr: &[f64]) -> Vec<crate::queue::QueueState> {
    plans
        .iter()
        .map(|plan| crate::queue::QueueState::init_from_value(curr[plan.stock_off]))
        .collect()
}

/// Serve a queue's SECONDARY outflows (every outflow after the primary) in
/// priority order against the post-primary front (docs/design/queues.md §4.5, §5),
/// writing each served rate = `removed / dt` into `curr`.
///
/// `redirectable` is the volume the PRIMARY was blocked from taking by a capacity /
/// inflow-limit / arrest condition (`desire − taken`, §4.5) -- the ONLY volume an
/// `<overflow/>` outflow may claim. It is 0 for an unconstrained primary (never
/// blocked), so an overflow behind an unconstrained primary drains nothing (§4.5,
/// the correct no-upstream-conveyor behavior). Each secondary pops from the same
/// front:
/// - an `<overflow/>` outflow to a cloud/regular stock drains up to the still-
///   redirectable volume its higher-priority siblings left
///   (`take_from_front(redirectable)`), decrementing the running budget. Overflow
///   is NOT batch-integrity-bound: `take_from_front` splits freely (§4.5).
/// - an ordinary (non-overflow) outflow to a cloud/regular stock drains the ENTIRE
///   remaining front (§5.4, `serve_unconstrained`), which also zeroes the
///   redirectable budget (nothing is left to redirect).
///
/// The primary (index 0) is served by the caller (`serve_unconstrained` for an
/// uncoupled queue, `take_for_conveyor` for a coupled one), so this skips it. Every
/// secondary reaching here targets a cloud or regular stock: a secondary whose
/// destination is a conveyor is rejected up front by [`detect_coupling_specs`]
/// ([`ErrorCode::QueueSecondaryOutflowToConveyor`]), so none is ever coupled or
/// belt-bound at this point.
fn serve_secondary_outflows(
    outflows: &[QueueOutflowPlan],
    state: &mut crate::queue::QueueState,
    redirectable: f64,
    curr: &mut [f64],
    dt: f64,
) {
    let mut redirectable = redirectable.max(0.0);
    for outflow in outflows.iter().skip(1) {
        // A Coupled secondary is never produced (only the primary couples), so this
        // guard is defense-in-depth: it must not be drained to a cloud here.
        if matches!(outflow.kind, QueueOutflowKind::Coupled { .. }) {
            continue;
        }
        let removed = if outflow.overflow {
            // Overflow: only the still-redirectable blocked front volume.
            let took = state.take_from_front(redirectable);
            redirectable -= took;
            took
        } else {
            // Ordinary competing outflow to a cloud/regular stock: drains the whole
            // remaining front (§5.4); the redirected budget is then moot.
            redirectable = 0.0;
            state.serve_unconstrained()
        };
        curr[outflow.flow_off] = removed / dt;
    }
}

/// Admit this DT's inflow into `state` while keeping the flat queue stock in
/// lockstep with the FIFO side table (docs/design/queues.md §3.4/§4.1/§4.2 step 1).
///
/// §4.2 step 1 admits one batch of `Σ max(f_i, 0) · dt` -- each inflow clamped at
/// zero INDEPENDENTLY, then summed (sum-of-clamps, NOT clamp-of-sum; §3.4: "a
/// negative inflow contributes no batch"). The flat queue stock is integrated by
/// the ordinary Stocks phase from the SAME inflow slots (§4.1's conservation
/// identity `Δqueue = Σ inflow − Σ outflow`), so unless the clamped value is
/// written BACK into each slot the Stocks phase folds the raw (possibly negative)
/// rate into the stock and it drifts away from `Σ batches` -- the invariant §4.1
/// pins. We therefore clamp each inflow slot IN PLACE before summing, exactly as
/// [`crate::conveyor_compile::conveyor_phase_b_one`] writes its admitted/clamped
/// equation-inflow rates back, so the admitted volume and the integrated Δstock
/// are equal by construction for every sign combination:
/// - all inflows ≥ 0: every slot is unchanged and `Σ slots == admit rate` exactly;
/// - a negative inflow: its slot is zeroed, so neither the batch nor the stock
///   sees it -- a modeler-visible inflow's saved series then reads 0 for that step,
///   matching how a clamped conveyor equation inflow is published;
/// - a NaN inflow: `NaN.max(0.0) == 0.0` zeroes the slot (the same defensive
///   hygiene [`crate::queue::QueueState::take_for_conveyor`] / `clamp_cap` apply),
///   so one poisoned inflow does not silently poison the flat stock.
///
/// This is the single admit path both the uncoupled ([`serve_uncoupled_queue`]) and
/// coupled ([`run_coupled_passes`]) queue serves call, so the write-back can never
/// drift between them.
fn admit_inflows(
    inflow_offs: &[usize],
    state: &mut crate::queue::QueueState,
    curr: &mut [f64],
    dt: f64,
) {
    let mut rate = 0.0;
    for &off in inflow_offs {
        let clamped = curr[off].max(0.0);
        curr[off] = clamped;
        rate += clamped;
    }
    state.admit(rate, dt);
}

/// Serve one UNCOUPLED queue for this DT: admit `Σ max(inflow, 0) · dt` (§4.2 step 1,
/// via [`admit_inflows`]), then serve its outflows in priority order (§4.2 step 2).
/// An uncoupled queue's primary outflow targets a cloud or regular stock and so is
/// unconstrained: it EMPTIES the queue (§4.3), leaving redirectable = 0 for any
/// secondary/overflow sibling (§4.5). Shared by [`run_queue_pass`] (fully uncoupled
/// models) and the uncoupled tail of [`run_coupled_passes`].
fn serve_uncoupled_queue(
    plan: &QueuePlan,
    state: &mut crate::queue::QueueState,
    curr: &mut [f64],
    dt: f64,
) {
    admit_inflows(&plan.inflow_offs, state, curr, dt);
    if let Some(primary) = plan.outflows.first() {
        // The primary of an uncoupled queue is Unconstrained (a coupled primary is
        // served by the combined pass, and only the primary is ever coupled), so it
        // drains the whole queue; redirectable = 0 (never blocked, §4.5).
        let removed = state.serve_unconstrained();
        curr[primary.flow_off] = removed / dt;
        serve_secondary_outflows(&plan.outflows, state, 0.0, curr, dt);
    }
}

/// The queue pass (§4.2), run once per Euler step between the Flows and Stocks
/// phases. For each UNCOUPLED queue: admit `Σ inflow_rate · dt` (the inflow rates
/// were computed in the Flows phase), then serve each outflow in priority order and
/// write its driven rate = `removed / dt` back into `curr`, so ordinary stock
/// integration then advances the queue stock (and any downstream stock) using the
/// pass-computed rates.
///
/// Serve order is admit-then-serve (§4.2): the just-admitted batch can leave in the
/// same DT when the downstream is unconstrained (§4.3, the pass-through). An
/// unconstrained primary empties the queue, so it drains everything and every
/// lower-priority outflow removes nothing (§4.3 degenerate case / §5.4) -- which
/// also makes an `<overflow/>` sibling drain nothing when the primary was not
/// blocked (§4.5), the correct no-upstream-conveyor behavior.
pub fn run_queue_pass(
    plans: &[QueuePlan],
    states: &mut [crate::queue::QueueState],
    curr: &mut [f64],
    dt: f64,
) {
    for (plan, state) in plans.iter().zip(states.iter_mut()) {
        // A coupled queue (its primary outflow feeds a discrete conveyor) is served
        // wholesale -- admit AND serve -- by the combined queue-conveyor pass
        // ([`run_coupled_passes`]), so skip it here to avoid a double admit (§9).
        // A conveyor-free queue has no coupled outflow, so this never fires on the
        // uncoupled path (byte-identical to before the coupling landed).
        if plan
            .outflows
            .iter()
            .any(|o| matches!(o.kind, QueueOutflowKind::Coupled { .. }))
        {
            continue;
        }
        serve_uncoupled_queue(plan, state, curr, dt);
    }
}

/// Publish each queue's container-access results into their data-buffer slots
/// (§8). Called at STEP-START -- before the Flows phase in the Euler loop and
/// after queue initialization in `run_initials` -- so the published values
/// reflect the batch state as left by the previous step's admit/serve (=
/// start-of-step for THIS step). Each container variable is a hidden no-flow
/// STOCK, so the Flows phase never recomputes its slot and the Stocks phase
/// leaves it unchanged: the value is visible to Flows-phase readers (an aux
/// reading `SUM(queue)` sees the batches BEFORE this step's admit/serve) and
/// survives the whole step -- identical timing and mechanism to conveyor
/// container access.
///
/// The container value is computed by the SHARED
/// [`container_value_from_slice`](crate::conveyor_compile::container_value_from_slice)
/// over the queue's front-to-back batch vector: `queue[k]` (1-based) maps to
/// `batch_contents()[k-1]`, the reducers to the batch-volume vector, and
/// `SIZE` to the batch count. `total == Σ batch_contents` and
/// `batch_count == batch_contents.len()` hold exactly, so the published values
/// agree with the queue's own accessors.
pub fn publish_queue_container_values(
    plans: &[QueuePlan],
    states: &[crate::queue::QueueState],
    curr: &mut [f64],
) {
    for (plan, state) in plans.iter().zip(states.iter()) {
        if plan.containers.is_empty() {
            continue;
        }
        let batches = state.batch_contents();
        for c in &plan.containers {
            curr[c.off] = container_value_from_slice(&batches, &c.kind);
        }
    }
}

// ----- unified build path (queues + conveyors coexist) -----

/// Reject any stock in `main_model` of `project` carrying BOTH a `<conveyor>`
/// block and a `<queue/>` marker (docs/design/queues.md §10.7). XMILE defines a
/// conveyor and a queue as distinct stock TYPES; a stock has exactly one type. The
/// two markers are independent optional fields the reader/proto/serde carry side
/// by side (so the reader deliberately preserves both -- rejection is a compile-,
/// not parse-, time concern), and the two expansion passes each clear only their
/// OWN marker: [`crate::conveyor_compile::expand_conveyors`] clears the conveyor
/// block, [`expand_queues`] then re-sees the still-queue-marked stock and clears
/// the queue marker. A both-marked stock would therefore be expanded TWICE --
/// given both a [`crate::conveyor_compile::ConveyorPlan`] AND a [`QueuePlan`] over
/// the same stock and shared outflow slot -- and [`run_coupled_passes`] would
/// drive that slot from both passes (the last writer winning while belt and FIFO
/// advance under different rates): silent garbage with no diagnostic, and both
/// markers cleared so the [`crate::db`] `ConveyorNotExpanded`/`QueueNotExpanded`
/// guard can never see it.
///
/// This scan runs BEFORE either expansion (so no clone is mutated) at the single
/// shared chokepoint every production caller funnels through -- [`compile_sim`]
/// (and hence [`build_sim`]) -> [`build_compiled`] -- so no per-path twin is
/// needed. It scans the MAIN model only, because expansion touches only the main
/// model: a both-marked stock in a sub-model never double-expands (the pass leaves
/// it untouched) and is already rejected loudly by the `db` guard's per-marker
/// submodel arm ([`ErrorCode::ConveyorInSubmodelUnsupported`], which fires first on
/// the surviving conveyor block).
fn reject_conveyor_queue_conflict(
    project: &datamodel::Project,
    main_model: &str,
) -> Result<(), (ErrorCode, String)> {
    let main_canon = canon(main_model);
    let Some(main) = project.models.iter().find(|m| canon(&m.name) == main_canon) else {
        // No such model: the caller's ordinary compile reports the missing model.
        return Ok(());
    };
    for v in &main.variables {
        if let datamodel::Variable::Stock(s) = v
            && s.compat.conveyor.is_some()
            && s.compat.queue.is_some()
        {
            return Err((
                ErrorCode::StockBothConveyorAndQueue,
                format!(
                    "stock '{}' is marked as BOTH a conveyor and a queue, but a stock has \
                     exactly one type; remove either the <conveyor> block or the <queue/> \
                     marker (XMILE defines conveyors and queues as distinct stock types)",
                    s.ident
                ),
            ));
        }
    }
    Ok(())
}

/// Compile `project` and resolve BOTH its conveyor and queue plans, returning the
/// compiled simulation plus the two plan sets (either empty when the model has
/// none of that kind). This is the unified special-stock-type build path: a model
/// may contain conveyors, queues, or both, so it expands conveyors FIRST (via
/// [`crate::conveyor_compile::expand_conveyors`]) then queues (on the
/// conveyor-expanded project), compiles ONCE, and resolves both plan sets against
/// the same offset map. It is the reusable core of [`compile_sim`] and
/// [`build_vm`]; a caller that rebuilds the VM later (libsimlin's reset) keeps all
/// three pieces so it can re-attach the plans.
///
/// The expanded project is compiled INCREMENTALLY, in `db`'s second input slot
/// (`SimlinDb::sync_expanded`): the expansion is a linear datamodel
/// walk, so it is re-run on every build, but the per-variable re-sync onto the
/// prior expanded handles means an unrelated edit invalidates one fragment rather
/// than the whole project. (Expansion cannot be a salsa tracked function -- it
/// creates variables, hence salsa *inputs*, which tracked functions may not do --
/// which is why it runs here on the `&mut db` path rather than as a query.)
///
/// The expanded project is a SEPARATE `SourceProject` from the user's, so
/// diagnostics -- which `collect_all_diagnostics` gathers from the user's handle
/// -- never see a synthetic `$conv$`/`$queue$` ident. It carries `ltm_enabled ==
/// false` (the sync path never sets the flag), so the expanded compile does not
/// participate in LTM: conveyor/queue plus LTM is a documented degradation.
///
/// Enforces the Euler-only rule for both stock types (§10.3): a conveyor present
/// under non-Euler yields [`ErrorCode::ConveyorNonEulerMethod`] (behavior-
/// identical to the pure conveyor path); a queue present under non-Euler yields
/// [`ErrorCode::QueueNonEulerMethod`].
///
/// For a project with NO conveyors and NO queues both expansions are no-ops and
/// this is the ordinary compile path applied to a verbatim copy of the project --
/// correct, but it would pointlessly populate `db`'s expanded slot with a duplicate
/// of the user's own inputs. Production callers therefore reach this only through
/// [`compile_sim`]'s marker dispatch, which sends an ordinary model straight to
/// `compile_project_incremental` against the caller's `SourceProject`. ([`build_vm`],
/// whose db is throwaway, is the exception.) The slot, once populated, is never
/// released -- see the `SimlinDb::expanded_state` field docs for why clearing it
/// would cost rather than save.
pub fn build_compiled(
    db: &mut crate::db::SimlinDb,
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<(
    std::sync::Arc<crate::vm::CompiledSimulation>,
    Vec<crate::conveyor_compile::ConveyorPlan>,
    Vec<QueuePlan>,
)> {
    use crate::common::{Error, ErrorKind};

    // Duplicate canonical variable idents are rejected BEFORE any expansion
    // (GH #885): `expand_conveyors`/`expand_queues` walk the raw datamodel
    // variable list where both twins are still visible, and while each pass
    // is individually robust against a twin pair (the GH #870 hardening),
    // expansion may transform or consume one twin -- so the expanded-project
    // compile below (`compile_project_incremental`, which re-checks the same
    // condition) would report post-expansion names. Rejecting on the ORIGINAL
    // project keeps the message in the user's own spellings and makes
    // expansion self-consistency moot on every production path. Every model
    // is checked (non-main models pass through expansion untouched), matching
    // the project-level scan the ordinary compile path performs.
    for model in &project.models {
        if let Some((canonical, spellings)) =
            crate::common::duplicate_variable_groups(model.variables.iter().map(|v| v.get_ident()))
                .into_iter()
                .next()
        {
            return Err(Error::new(
                ErrorKind::Simulation,
                ErrorCode::DuplicateVariable,
                Some(crate::common::duplicate_variable_message(
                    &model.name,
                    &canonical,
                    &spellings,
                )),
            ));
        }
    }

    // A stock cannot be both a conveyor and a queue (§10.7): reject a both-marked
    // stock BEFORE either expansion, since each expansion clears only its own
    // marker and the two passes would otherwise silently double-drive the shared
    // outflow slot. This is the single shared chokepoint every production caller
    // funnels through, so no per-path twin (e.g. in the db guard, which can't see
    // the then-cleared markers anyway) is needed.
    reject_conveyor_queue_conflict(project, main_model)
        .map_err(|(code, msg)| Error::new(ErrorKind::Simulation, code, Some(msg)))?;

    // Expand conveyors first, then queues on the result. The expansions are
    // order-independent (neither reads the other's marker); the coupling is
    // detected AFTER both expansions from the two meta sets (see below).
    let (expanded, conv_metas) = crate::conveyor_compile::expand_conveyors(project, main_model)
        .map_err(|(code, msg)| Error::new(ErrorKind::Simulation, code, Some(msg)))?;
    let (expanded, queue_metas) = expand_queues(&expanded, main_model)
        .map_err(|(code, msg)| Error::new(ErrorKind::Simulation, code, Some(msg)))?;

    // Detect queue-conveyor couplings (§9): a queue outflow that is a discrete
    // conveyor's equation-driven inflow. Reads the `discrete`/`one_at_a_time`/
    // `batch_integrity` attributes from the ORIGINAL project (the expanded clone
    // has cleared the conveyor block) and enforces the discrete requirement
    // (`ConveyorQueueUpstreamNotDiscrete`) BEFORE compiling.
    let coupling_specs = detect_coupling_specs(project, main_model, &conv_metas, &queue_metas)
        .map_err(|(code, msg)| Error::new(ErrorKind::Simulation, code, Some(msg)))?;

    // Euler-only. Report the conveyor code when a conveyor is present (so the
    // conveyor path stays behavior-identical), otherwise the queue code. Reads
    // the EFFECTIVE root specs (the main model's sim_specs override preferred,
    // matching the runtime's assemble rule) so a model-level RK override
    // cannot evade the gate.
    if crate::conveyor_compile::effective_sim_specs(&expanded, main_model).sim_method
        != datamodel::SimMethod::Euler
    {
        if !conv_metas.is_empty() {
            return Err(Error::new(
                ErrorKind::Simulation,
                ErrorCode::ConveyorNonEulerMethod,
                Some("conveyors require Euler integration".to_string()),
            ));
        }
        if !queue_metas.is_empty() {
            return Err(Error::new(
                ErrorKind::Simulation,
                ErrorCode::QueueNonEulerMethod,
                Some("queues require Euler integration".to_string()),
            ));
        }
    }

    // Sync the expanded project into the db's SECOND input slot, reusing the
    // prior expanded handles so the per-variable diff hits the salsa caches. The
    // handle is re-derived here on every build, which is exactly what makes a
    // rolled-back staged patch unable to leave stale expanded inputs behind.
    let expanded_project = db.sync_expanded(&expanded);
    // The expanded twin is always compiled without the LTM overlay: LTM over
    // a conveyor/queue is a documented degradation (docs/design/conveyors.md
    // s9.6, queues.md s10.5).
    let mut compiled = crate::db::compile_project_incremental(
        db,
        expanded_project,
        main_model,
        crate::db::LtmOverlay::Off,
    )?;

    let mut conveyor_plans = if conv_metas.is_empty() {
        Vec::new()
    } else {
        crate::conveyor_compile::resolve_plans(&conv_metas, &compiled.offsets).ok_or_else(|| {
            Error::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some("internal error: conveyor plan references an unresolved slot".to_string()),
            )
        })?
    };
    let mut queue_plans = if queue_metas.is_empty() {
        Vec::new()
    } else {
        resolve_plans(&queue_metas, &compiled.offsets).ok_or_else(|| {
            Error::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some("internal error: queue plan references an unresolved slot".to_string()),
            )
        })?
    };

    // Resolve the detected couplings against the offset map: mark each coupled
    // conveyor inflow `queue_coupled` (so phase_b admits it unconditionally) and
    // rewrite the matching queue outflow to `Coupled` (so the combined pass serves
    // it under the batch rules). The coupling rides ENTIRELY inside the two plan
    // sets -- no separate structure to thread -- so libsimlin's reset re-attaches
    // it by re-attaching the plans, and the VM reconstructs it each step.
    if !coupling_specs.is_empty() {
        apply_couplings(
            &coupling_specs,
            &queue_metas,
            &mut conveyor_plans,
            &mut queue_plans,
            &compiled.offsets,
        )
        .ok_or_else(|| {
            Error::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some(
                    "internal error: queue-conveyor coupling references an unresolved slot"
                        .to_string(),
                ),
            )
        })?;
    }

    // Pass-written slots (driven outflows, leaks, containers) must not be
    // overridable constants: their placeholder `0` equations compile to
    // AssignConstCurr, but the passes overwrite the slots every step, so an
    // accepted override would be silently ineffective (GH #871). Retracting
    // them HERE -- on the CompiledSimulation callers cache -- is what makes
    // libsimlin's no-VM `is_constant_offset` validation (after run_to_end
    // consumed the VM) reject exactly like the live VM does; the Vm repeats
    // the retraction when plans are attached, as defense for a directly
    // assembled Vm.
    //
    // The retraction edits the program, so this path takes a private copy of
    // the memoized artifact (`make_mut` clones while the salsa memo shares
    // it). That copy is the special-stock path's cost alone: an ordinary
    // model hands the memo's `Arc` straight to the Vm.
    let scrubbed = std::sync::Arc::make_mut(&mut compiled);
    for plan in &conveyor_plans {
        scrubbed.exclude_overridable_offsets(plan.pass_written_offsets());
    }
    for plan in &queue_plans {
        scrubbed.exclude_overridable_offsets(plan.pass_written_offsets());
    }

    Ok((compiled, conveyor_plans, queue_plans))
}

/// One detected queue-conveyor coupling at the metadata (name) level: the shared
/// flow (the queue's primary outflow == the conveyor's single equation-driven
/// inflow) plus the batch rules read from the conveyor's block (§9). Resolved to
/// slot offsets by [`apply_couplings`].
#[derive(Clone, Debug, PartialEq, Eq)]
struct CouplingSpec {
    /// Canonical name of the shared flow.
    shared_flow: String,
    one_at_a_time: bool,
    batch_integrity: bool,
}

/// The `one_at_a_time` / `batch_integrity` attributes of the conveyor stock named
/// `stock_canon` (canonical) in `main_model` of the ORIGINAL `project` (whose
/// conveyor block is still present -- the expanded clone cleared it). Returns the
/// XMILE defaults `(one_at_a_time = true, batch_integrity = false)` when the block
/// or the stock is absent.
fn conveyor_batch_rules(
    project: &datamodel::Project,
    main_model: &str,
    stock_canon: &str,
) -> (bool, bool) {
    let main_canon = canon(main_model);
    for m in &project.models {
        if canon(&m.name) != main_canon {
            continue;
        }
        for v in &m.variables {
            if let datamodel::Variable::Stock(s) = v
                && canon(&s.ident) == stock_canon
                && let Some(c) = &s.compat.conveyor
            {
                return (c.one_at_a_time, c.batch_integrity);
            }
        }
    }
    (true, false)
}

/// Detect every queue-conveyor coupling in `main_model` (§9): a queue outflow that
/// is a conveyor's equation-driven inflow (`!conveyor_driven` -- i.e. NOT itself a
/// conveyor's driven outflow). Enforces the discrete requirement: a queue directly
/// upstream of a NON-discrete conveyor is a loud
/// [`ErrorCode::ConveyorQueueUpstreamNotDiscrete`] error (queues.md §9,
/// conveyors.md §11). The batch rules are read from the (discrete) conveyor's
/// block. A queue outflow to a cloud/regular stock matches no conveyor inflow and
/// stays unconstrained (Phase 3 behavior unchanged).
///
/// Only a queue's PRIMARY (first, highest-priority) outflow may feed a conveyor
/// (§4.4/§9): the combined pass ([`run_coupled_passes`]) couples exactly the
/// primary. A SECONDARY outflow whose destination is a conveyor -- an `<overflow/>`
/// sibling (§4.5) or a second ordinary competing outflow (§5.4) -- is rejected here
/// with [`ErrorCode::QueueSecondaryOutflowToConveyor`]. Left uncoupled it would
/// escape the discrete guard AND write its served rate into a slot the destination
/// belt's phase B independently treats as an equation-driven inflow, silently
/// desyncing the queue FIFO / belt stock from its side table. The spec sketches an
/// overflow-to-conveyor (§4.5, "an overflow to another conveyor is itself
/// constrained by that conveyor") but does not define how a secondary's
/// redirectable budget interleaves with a (possibly distinct) second belt's
/// admission budget, so faithfully serving it is a deferred feature rejected loudly
/// rather than mis-accounted. The rejection is independent of the destination
/// conveyor's `discrete`-ness and of whether the primary itself feeds a conveyor.
fn detect_coupling_specs(
    project: &datamodel::Project,
    main_model: &str,
    conv_metas: &[crate::conveyor_compile::ConveyorMeta],
    queue_metas: &[QueueMeta],
) -> Result<Vec<CouplingSpec>, (ErrorCode, String)> {
    // The conveyor (if any) whose SINGLE equation-driven inflow is `flow`: a
    // conveyor inflow named `flow` that is not itself conveyor-driven. This is the
    // "directly upstream" relation for both the primary coupling and the secondary
    // rejection below.
    let conveyor_fed_by = |flow: &str| -> Option<&crate::conveyor_compile::ConveyorMeta> {
        conv_metas.iter().find(|cm| {
            cm.inflows
                .iter()
                .any(|inf| inf.flow == flow && !inf.conveyor_driven)
        })
    };

    let mut specs = Vec::new();
    for qm in queue_metas {
        // Reject any SECONDARY outflow feeding a conveyor before considering the
        // primary coupling: only the primary may feed a conveyor (§4.4/§9). This
        // fires for an overflow OR an ordinary secondary, and regardless of whether
        // the destination conveyor is discrete -- neither is served under the batch
        // rules, so both would silently mis-account (see the module-level rustdoc).
        for out in qm.outflows.iter().skip(1) {
            if let Some(cm) = conveyor_fed_by(&out.flow) {
                return Err((
                    ErrorCode::QueueSecondaryOutflowToConveyor,
                    format!(
                        "queue '{}' outflow '{}' feeds conveyor '{}', but only a queue's first \
                         (highest-priority) outflow may feed a conveyor; a secondary outflow or \
                         overflow to a conveyor is not supported",
                        qm.stock, out.flow, cm.stock
                    ),
                ));
            }
        }

        // The PRIMARY outflow couples to a conveyor (§4.4/§9). Restricting the
        // coupling to the primary keeps one coupling per queue, so a queue is served
        // exactly once per DT.
        if let Some(out) = qm.outflows.first() {
            // A coupled shared flow is the conveyor's SINGLE equation-driven
            // inflow: a conveyor inflow that is not itself conveyor-driven.
            let Some(cm) = conveyor_fed_by(&out.flow) else {
                continue; // primary outflow to a cloud/regular stock: unconstrained
            };
            if !cm.discrete {
                return Err((
                    ErrorCode::ConveyorQueueUpstreamNotDiscrete,
                    format!(
                        "queue '{}' is directly upstream of conveyor '{}', which must be discrete \
                         (a queue feeds a conveyor only through the discrete batch-admission rules)",
                        qm.stock, cm.stock
                    ),
                ));
            }
            let (one_at_a_time, batch_integrity) =
                conveyor_batch_rules(project, main_model, &cm.stock);
            specs.push(CouplingSpec {
                shared_flow: out.flow.clone(),
                one_at_a_time,
                batch_integrity,
            });
        }
    }
    Ok(specs)
}

/// Resolve each [`CouplingSpec`] against the compiled offset map, mutating the two
/// plan sets so the coupling rides inside them (§9): every element slot of a
/// shared flow marks its conveyor inflow `queue_coupled` and rewrites its queue
/// outflow to [`QueueOutflowKind::Coupled`] carrying the conveyor plan index +
/// batch rules. Handles a scalar coupling (one slot) and -- since it enumerates
/// the queue meta's `element_subscripts` -- an arrayed one (one coupling per
/// element, requiring the queue and conveyor to share the element's slot).
/// Returns `None` if any shared flow slot is missing from the offset map or no
/// conveyor inflow claims it (an expansion/compilation inconsistency the caller
/// surfaces as `NotSimulatable`).
fn apply_couplings(
    specs: &[CouplingSpec],
    queue_metas: &[QueueMeta],
    conveyor_plans: &mut [crate::conveyor_compile::ConveyorPlan],
    queue_plans: &mut [QueuePlan],
    offsets: &HashMap<Ident<Canonical>, usize>,
) -> Option<()> {
    // Every shared-flow element slot -> its batch rules. An arrayed queue's N
    // FIFOs each couple to belt `e` at the shared flow's element-`e` slot.
    let mut coupled_slots: HashMap<usize, (bool, bool)> = HashMap::new();
    for spec in specs {
        let qm = queue_metas
            .iter()
            .find(|m| m.outflows.iter().any(|o| o.flow == spec.shared_flow))?;
        for e in 0..n_elements(&qm.element_subscripts) {
            let off = element_offset(offsets, &qm.element_subscripts, e, &spec.shared_flow)?;
            coupled_slots.insert(off, (spec.one_at_a_time, spec.batch_integrity));
        }
    }

    // Mark each coupled conveyor inflow and remember which conveyor plan owns each
    // shared slot (so the queue outflow can name it).
    let mut slot_to_conveyor: HashMap<usize, usize> = HashMap::new();
    for (ci, cp) in conveyor_plans.iter_mut().enumerate() {
        for inf in &mut cp.inflows {
            if coupled_slots.contains_key(&inf.flow_off) {
                inf.queue_coupled = true;
                slot_to_conveyor.insert(inf.flow_off, ci);
            }
        }
    }

    // Rewrite each coupled queue outflow to `Coupled { conveyor, rules }`.
    for qp in queue_plans.iter_mut() {
        for op in &mut qp.outflows {
            if let Some(&(one_at_a_time, batch_integrity)) = coupled_slots.get(&op.flow_off) {
                let conveyor = *slot_to_conveyor.get(&op.flow_off)?;
                op.kind = QueueOutflowKind::Coupled {
                    conveyor,
                    one_at_a_time,
                    batch_integrity,
                };
            }
        }
    }
    Some(())
}

/// A coupled queue serve wired to one conveyor (derived from the plans by
/// [`CouplingTable::build`]).
///
/// Public because the wasm backend (`wasmgen::module`) unrolls the same interleaved
/// order [`run_coupled_passes`] walks, and must read the very table the VM reads:
/// deriving a second one there is how the two backends' admission priorities would
/// drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoupledServe {
    /// Index into the queue plan set.
    pub queue: usize,
    /// The slab slot that is BOTH the queue's driven primary outflow rate and the
    /// conveyor's admitted inflow rate.
    pub shared_flow_off: usize,
    /// `isee:one_at_a_time` on the belt: serve only the front batch.
    pub one_at_a_time: bool,
    /// `isee:batch_integrity` on the belt: never split a batch.
    pub batch_integrity: bool,
}

/// The queue-conveyor coupling table [`run_coupled_passes`] reads each step:
/// which queues are coupled to which conveyor, in what admission-priority
/// order. The coupling is fixed at build time ([`apply_couplings`] stamps
/// [`QueueOutflowKind::Coupled`] onto the plans exactly once), so the table is
/// derived ONCE when the plans are attached to the VM
/// ([`crate::vm::Vm::set_conveyor_plans`] / [`crate::vm::Vm::set_queue_plans`],
/// both of which rebuild it so attachment order does not matter) rather than
/// rebuilt every Euler step from the immutable plans (GH #878). It is a pure
/// deterministic function of the two plan sets -- no extra state to keep
/// consistent across libsimlin's reset, which re-derives it by re-attaching
/// cloned plans.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CouplingTable {
    /// Per conveyor-plan index: the queues coupled to that belt, ordered by the
    /// belt's `<inflow>` declaration order (the admission priority).
    coupling_for_conveyor: Vec<Vec<CoupledServe>>,
    /// Per queue-plan index: served by the combined pass (true) or by the plain
    /// uncoupled queue pass (false).
    queue_is_coupled: Vec<bool>,
    /// Any coupling at all? False selects [`run_coupled_passes`]' fast path
    /// (the two independent passes, byte-identical to pre-coupling behavior).
    any: bool,
}

impl CouplingTable {
    /// Derive the coupling table from the two resolved plan sets. The `Coupled`
    /// outflow kind carries the conveyor plan index + batch rules, so this is a
    /// pure read of compile-time-constant data.
    ///
    /// A discrete conveyor MAY have MORE THAN ONE coupled queue -- several
    /// queues' primary outflows feeding one belt as its equation-driven inflows
    /// (the listed-order priority of conveyors.md §4.3 step 4 / §11). They are
    /// served in the belt's `<inflow>` DECLARATION order (the admission
    /// priority, matching how phase_b apportions equation-inflow clearance in
    /// listed order), so each belt's list is sorted below by the shared flow's
    /// position in that belt's `inflows` -- a deterministic, compile-time
    /// order, NOT the queue-plan or HashMap iteration order.
    pub fn build(
        conv_plans: &[crate::conveyor_compile::ConveyorPlan],
        queue_plans: &[QueuePlan],
    ) -> Self {
        let mut coupling_for_conveyor: Vec<Vec<CoupledServe>> = vec![Vec::new(); conv_plans.len()];
        let mut queue_is_coupled = vec![false; queue_plans.len()];
        let mut any = false;
        for (qi, qp) in queue_plans.iter().enumerate() {
            for op in &qp.outflows {
                if let QueueOutflowKind::Coupled {
                    conveyor,
                    one_at_a_time,
                    batch_integrity,
                } = op.kind
                    && conveyor < coupling_for_conveyor.len()
                {
                    coupling_for_conveyor[conveyor].push(CoupledServe {
                        queue: qi,
                        shared_flow_off: op.flow_off,
                        one_at_a_time,
                        batch_integrity,
                    });
                    queue_is_coupled[qi] = true;
                    any = true;
                }
            }
        }
        // Order each conveyor's coupled serves by the belt's `<inflow>` declaration
        // order so admission priority is deterministic and matches phase_b's
        // listed-order apportionment (a shared flow always appears among the belt's
        // inflows, so `position` resolves; the fallback keeps a would-be-missing slot
        // last rather than panicking).
        for (ci, serves) in coupling_for_conveyor.iter_mut().enumerate() {
            if serves.len() > 1 {
                let inflows = &conv_plans[ci].inflows;
                serves.sort_by_key(|cs| {
                    inflows
                        .iter()
                        .position(|inf| inf.flow_off == cs.shared_flow_off)
                        .unwrap_or(usize::MAX)
                });
            }
        }
        CouplingTable {
            coupling_for_conveyor,
            queue_is_coupled,
            any,
        }
    }

    /// Is any queue coupled to any conveyor? False selects [`run_coupled_passes`]'
    /// fast path, and the wasm backend's independent belt-then-queue emission.
    pub fn any(&self) -> bool {
        self.any
    }

    /// The queues coupled to conveyor `i`, in that belt's `<inflow>` declaration
    /// order -- the admission priority. Empty for an uncoupled belt.
    pub fn serves_for_conveyor(&self, i: usize) -> &[CoupledServe] {
        &self.coupling_for_conveyor[i]
    }

    /// Is queue `qi` served by the interleaved pass? If so the uncoupled
    /// admit-then-serve must SKIP it -- serving it twice would double-admit.
    pub fn queue_is_coupled(&self, qi: usize) -> bool {
        self.queue_is_coupled[qi]
    }
}

/// The combined queue-conveyor pass (queues.md §9), run once per Euler step
/// between the Flows and Stocks phases in place of the separate conveyor
/// ([`crate::conveyor_compile::run_pass`]) and queue ([`run_queue_pass`]) passes
/// whenever the model has any coupling. Ordering (the whole point of the combined
/// pass): every conveyor's Phase A runs first (leak + exit, freeing belt room),
/// then for each conveyor -- for each queue coupled to it, in the belt's
/// `<inflow>` DECLARATION order -- the queue's serve is interleaved BEFORE that
/// conveyor's Phase B:
///
/// 1. size the conveyor's admission budget `req = min(cap_room, limit_vol)` from
///    ITS Phase A result ([`crate::conveyor_compile::coupled_admission_budget`]),
///    charging the volume any earlier-served coupled queue already committed to
///    this belt this DT (`prior_coupled_vol`) so the shared capacity room is not
///    double-spent;
/// 2. admit the queue's inflow (§4.2 step 1);
/// 3. serve `taken <= req` from the front under the batch rules
///    ([`crate::queue::QueueState::take_for_conveyor`]);
/// 4. debit the discrete inflow budget by `taken`
///    ([`crate::conveyor::ConveyorState::consume_inflow_budget`]) -- this also
///    advances the shared per-time-unit `in_carry` the NEXT coupled queue sees;
/// 5. write the shared flow slot to `taken / dt` -- this is BOTH the queue's
///    driven outflow rate AND the conveyor's admitted inflow rate, so the ordinary
///    Stocks phase integrates the queue stock `-taken` and the conveyor stock
///    `+taken` from the SAME slot (conservation on both stocks);
/// 6. after all its coupled queues are served, run the conveyor's Phase B, which
///    routes each shared flow through the unconditional `conv_inflows` path (it is
///    `queue_coupled`) and inserts `taken` onto the belt at the entry depth.
///
/// Several queues feeding one discrete conveyor reuse the spec's listed-order
/// admission priority (conveyors.md §4.3 step 4 / §11; §6.4 rule 1's per-inflow
/// apportionment governs the equation-driven QUANTIZED case, which coupled takes
/// bypass): the belt's `<inflow>` order is the admission priority, and each
/// successive queue's budget subtracts what its predecessors took (the capacity
/// arm via `prior_coupled_vol`, the inflow-limit arm via `in_carry`). Uncoupled conveyors run their ordinary Phase B and
/// uncoupled queues their ordinary admit-then-serve. When there is NO coupling
/// this delegates to the two independent passes, byte-identical to the
/// pre-coupling behavior.
///
/// Errors ([`ErrorCode::ConveyorTransitTooLong`](crate::common::ErrorCode::ConveyorTransitTooLong))
/// when a conveyor's mid-run `<sample>` re-latch would exceed the slat-count
/// bound (§4.1, surfaced from [`crate::conveyor_compile::run_phase_a`]); the VM
/// aborts the run with a simulation error.
// The two side-table sets (plans + states), the coupling table, `curr`, `dt`,
// and the clock inputs (`time`, `start`, `last_unit`) are all independent
// per-step inputs the VM already holds separately; bundling them into a struct
// would only add an indirection.
#[allow(clippy::too_many_arguments)]
pub fn run_coupled_passes(
    conv_plans: &[crate::conveyor_compile::ConveyorPlan],
    conveyors: &mut [crate::conveyor::ConveyorState],
    queue_plans: &[QueuePlan],
    queues: &mut [crate::queue::QueueState],
    coupling: &CouplingTable,
    curr: &mut [f64],
    dt: f64,
    time: f64,
    start: f64,
    last_unit: &mut i64,
) -> Result<(), (crate::common::ErrorCode, String)> {
    use crate::conveyor_compile as cc;

    // The coupling is compile-time constant, so the caller derives `coupling`
    // once at plan-attach time ([`CouplingTable::build`]) instead of rebuilding
    // it here every step (GH #878). It must have been built from THESE plan
    // sets; a stale table would index out of bounds or mis-route serves.
    let CouplingTable {
        coupling_for_conveyor,
        queue_is_coupled,
        any,
    } = coupling;
    debug_assert_eq!(
        coupling_for_conveyor.len(),
        conv_plans.len(),
        "coupling table was not built from these conveyor plans"
    );
    debug_assert_eq!(
        queue_is_coupled.len(),
        queue_plans.len(),
        "coupling table was not built from these queue plans"
    );

    // Fast path: no coupling -> the two independent passes, byte-identical.
    if !*any {
        cc::run_pass(conv_plans, conveyors, curr, dt, time, start, last_unit)?;
        run_queue_pass(queue_plans, queues, curr, dt);
        return Ok(());
    }

    // Phase A over every conveyor (frees belt room, writes driven outflow rates).
    let pa = cc::run_phase_a(conv_plans, conveyors, curr, dt, time, start, last_unit)?;

    // Per conveyor: interleave each coupled queue's serve between phase A and B,
    // in the belt's <inflow> declaration order. `prior_coupled_vol` accumulates the
    // volume earlier queues already committed to this belt this DT, so each
    // successive queue sizes its budget against the room its predecessors took (the
    // capacity arm; the per-time-unit inflow-limit arm is charged by
    // `consume_inflow_budget` advancing `in_carry`).
    for i in 0..conv_plans.len() {
        let mut prior_coupled_vol = 0.0;
        for cs in &coupling_for_conveyor[i] {
            let qi = cs.queue;
            // Size the budget from THIS conveyor's phase A (belt room minus what
            // earlier coupled queues took), admit the queue's inflow, then serve up
            // to `req` under the batch rules.
            let req = cc::coupled_admission_budget(
                &conv_plans[i],
                &conveyors[i],
                &pa[i],
                curr,
                dt,
                prior_coupled_vol,
            );
            admit_inflows(&queue_plans[qi].inflow_offs, &mut queues[qi], curr, dt);
            // Measure the batch-policy desire BEFORE the finite-`req` take mutates
            // the front (§4.5): the redirectable volume an <overflow/> sibling may
            // claim is `desire − taken` -- exactly the front material capacity /
            // inflow-limit / arrest (here including the SHARED budget an earlier
            // queue already spent) blocked the primary from taking.
            let desire = queues[qi].conveyor_desire(cs.one_at_a_time);
            let taken = queues[qi].take_for_conveyor(req, cs.one_at_a_time, cs.batch_integrity);
            let redirectable = (desire - taken).max(0.0);
            // Debit the discrete per-time-unit budget (the coupled volume bypasses
            // phase_b's equation-inflow accounting), then publish the shared rate:
            // BOTH the queue outflow and the conveyor inflow integrate from it.
            conveyors[i].consume_inflow_budget(taken);
            curr[cs.shared_flow_off] = taken / dt;
            // Serve the queue's secondary outflows (overflow / ordinary) against the
            // post-primary front (§4.5/§5). The coupled primary is `outflows[0]`
            // (only the primary is coupled), so this skips index 0.
            serve_secondary_outflows(
                &queue_plans[qi].outflows,
                &mut queues[qi],
                redirectable,
                curr,
                dt,
            );
            prior_coupled_vol += taken;
        }
        cc::conveyor_phase_b_one(i, conv_plans, conveyors, &pa, curr, dt);
    }

    // Serve the uncoupled queues (coupled ones were fully served above).
    for (qi, (plan, state)) in queue_plans.iter().zip(queues.iter_mut()).enumerate() {
        if queue_is_coupled[qi] {
            continue;
        }
        serve_uncoupled_queue(plan, state, curr, dt);
    }
    Ok(())
}

/// Build a runnable [`Vm`](crate::vm::Vm) for `project` on a THROWAWAY database,
/// wiring up conveyor AND queue support when the main model contains either.
///
/// A convenience for callers that hold no `SimlinDb`. It has NO production callers:
/// every one goes through [`compile_sim`] / [`build_sim`], which compile inside the
/// caller's db. This is a four-line wrapper over the production [`build_compiled`],
/// so its ONLY difference is a cold memo cache -- semantically transparent, which is
/// why the ~80 test call sites that use it still pin production behavior. It is not
/// a parallel implementation to keep in sync.
pub fn build_vm(
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<crate::vm::Vm> {
    let mut db = crate::db::SimlinDb::default();
    let (compiled, conveyor_plans, queue_plans) = build_compiled(&mut db, project, main_model)?;
    let mut vm = crate::vm::Vm::new(compiled)?;
    vm.set_conveyor_plans(conveyor_plans);
    vm.set_queue_plans(queue_plans);
    Ok(vm)
}

/// [`build_compiled`] on a throwaway database, for tests that build once and so
/// have nothing to gain from the caller-db incremental slot. Production code must
/// use [`compile_sim`]: a fresh db recompiles every fragment.
#[cfg(test)]
pub(crate) fn build_compiled_fresh(
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<(
    std::sync::Arc<crate::vm::CompiledSimulation>,
    Vec<crate::conveyor_compile::ConveyorPlan>,
    Vec<QueuePlan>,
)> {
    let mut db = crate::db::SimlinDb::default();
    build_compiled(&mut db, project, main_model)
}

/// Everything the unified dispatch ([`compile_sim`]) produces: the compiled
/// simulation, both special-stock plan sets, and which branch was taken.
///
/// `special` is what a caller needs in order to reason about LTM: the expansion
/// path compiles a different `SourceProject` (with `ltm_enabled == false`), so a
/// special-stock model carries no LTM instrumentation. libsimlin's
/// `simlin_sim_new` reads it to decide whether to snapshot the LTM
/// loop-partition metadata.
pub struct SimBuild {
    /// Shared with the salsa memo that assembled it (the ordinary path) or
    /// freshly built (the special-stock path); either way the `Vm` takes the
    /// same `Arc`, so nothing here is deep-copied on the way to execution.
    pub compiled: std::sync::Arc<crate::vm::CompiledSimulation>,
    /// One plan per conveyor belt (per array element for an arrayed conveyor).
    /// Empty unless `special`.
    pub conveyor_plans: Vec<crate::conveyor_compile::ConveyorPlan>,
    /// One plan per queue FIFO (per array element for an arrayed queue). Empty
    /// unless `special`.
    pub queue_plans: Vec<QueuePlan>,
    /// True when the main model carried a conveyor or a queue marker and took the
    /// expansion path.
    pub special: bool,
}

/// Compile `main_model`, transparently routing a model that contains a conveyor
/// or a queue through the special expansion build path ([`build_compiled`]) and
/// an ordinary model through the incremental salsa compile
/// ([`crate::db::compile_project_incremental`]).
///
/// This is the single "compile a simulation" dispatch. Every VM-backed production
/// caller funnels through it -- libsimlin's `simlin_sim_new` /
/// `is_simulatable`/`get_errors`/`apply_patch`, the CLI and serve simulators, and
/// the LTM analysis pipeline -- so the conveyor/queue expansion is applied uniformly
/// instead of each caller reproducing (or forgetting) it.
///
/// It is NOT, however, where `<overflow/>` marker placement is validated. `wasmgen`
/// compiles a datamodel without coming through here, so that check belongs one level
/// down, in `compile_project_incremental`, which both branches below AND `wasmgen`
/// funnel through. (The "never the FIRST outflow" half additionally runs
/// pre-expansion in [`expand_queues`], the only place that evidence still exists.)
///
/// The `db`/`source_project` pair drives the ordinary branch, preserving both
/// incremental caching and whatever `ltm_enabled` the caller set on
/// `source_project`. `datamodel` -- the synced representation of the SAME project
/// -- drives the marker scan and, on the special branch, the expansion. BOTH
/// branches now compile inside `db`; the special branch simply compiles `db`'s
/// expanded `SourceProject` instead of the user's.
///
/// The `ConveyorNotExpanded`/`QueueNotExpanded` guard in
/// `compile_project_incremental` deliberately stays intact: it fires only when an
/// un-expanded marker reaches the ordinary path WITHOUT having come through here
/// (a genuine internal bug), which is exactly what it is meant to catch. A model
/// with a special stock never reaches that guard from here because this function
/// routes it to the expansion path first.
pub fn compile_sim(
    db: &mut crate::db::SimlinDb,
    source_project: crate::db::SourceProject,
    datamodel: &datamodel::Project,
    main_model: &str,
    overlay: crate::db::LtmOverlay,
) -> crate::common::Result<SimBuild> {
    if crate::conveyor_compile::project_has_conveyor(datamodel, main_model)
        || project_has_queue(datamodel, main_model)
    {
        let (compiled, conveyor_plans, queue_plans) = build_compiled(db, datamodel, main_model)?;
        return Ok(SimBuild {
            compiled,
            conveyor_plans,
            queue_plans,
            special: true,
        });
    }

    // No `<overflow/>` validation here. The dispatch above scans for a marked
    // STOCK while the overflow marker rides on a FLOW, so a stray overflow reaches
    // this branch -- but validating it HERE would still leave `wasmgen`, which
    // calls `compile_project_incremental` directly and never comes through this
    // dispatch. The check lives in `compile_project_incremental` instead: that is
    // where `db::assemble_simulation`'s single production caller sits, so no compile
    // of any kind can slip past it.
    //
    // The expanded input slot is deliberately NOT released for an ordinary model.
    // Salsa never reclaims inputs, so dropping the handle map would not free the
    // arena -- it would only force the NEXT conveyor build to mint a fresh input
    // set. Keeping the handles means at most one expanded input set is ever created
    // per db, and a model that regains a conveyor re-syncs onto them. A stale slot
    // is unobservable: it is only ever read through `sync_expanded`'s return value.
    let compiled = crate::db::compile_project_incremental(db, source_project, main_model, overlay)?;
    Ok(SimBuild {
        compiled,
        conveyor_plans: Vec::new(),
        queue_plans: Vec::new(),
        special: false,
    })
}

/// Build a runnable [`Vm`](crate::vm::Vm) for `main_model` through the unified
/// [`compile_sim`] dispatch, attaching the conveyor/queue passes when the model
/// took the special branch.
///
/// This is the "compile-and-build a simulation" entry point every caller other
/// than `simlin_sim_new` uses (which needs the compiled sim and the plan sets
/// themselves, so it calls [`compile_sim`] directly).
pub fn build_sim(
    db: &mut crate::db::SimlinDb,
    source_project: crate::db::SourceProject,
    datamodel: &datamodel::Project,
    main_model: &str,
    overlay: crate::db::LtmOverlay,
) -> crate::common::Result<crate::vm::Vm> {
    let build = compile_sim(db, source_project, datamodel, main_model, overlay)?;
    let mut vm = crate::vm::Vm::new(build.compiled)?;
    // Attaching empty plan sets is semantically a no-op, but the ordinary path
    // has never touched the plan setters; keep it that way so an ordinary VM is
    // byte-identical to one built straight from `compile_project_incremental`.
    if build.special {
        vm.set_conveyor_plans(build.conveyor_plans);
        vm.set_queue_plans(build.queue_plans);
    }
    Ok(vm)
}

// The db-interaction half of the build path's tests (expanded-input reuse,
// marker validation on the ordinary dispatch branch, diagnostics provenance,
// staged-patch rollback) lives in a sibling file so this one stays under the
// per-file line cap.
#[cfg(test)]
#[path = "queue_compile_db_tests.rs"]
mod db_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::Ident;
    use std::io::BufReader;

    fn parse(xml: &str) -> datamodel::Project {
        crate::xmile::project_from_reader(&mut BufReader::new(xml.as_bytes())).unwrap()
    }

    /// A scalar queue -> regular stock with a constant inflow, no initial batch.
    const QUEUE_DRAIN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>queue drain</name><vendor>test</vendor><product version="1.0">test</product></header>
  <sim_specs method="Euler" time_units="Months">
    <start>0</start><stop>4</stop><dt>0.25</dt>
  </sim_specs>
  <model><variables>
    <stock name="waiting">
      <eqn>0</eqn>
      <inflow>arrivals</inflow>
      <outflow>into_service</outflow>
      <queue/>
    </stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
  </variables></model>
</xmile>"#;

    fn build_run(project: &datamodel::Project) -> crate::vm::Vm {
        let main = project.models[0].name.clone();
        let mut vm = build_vm(project, &main).expect("build queue vm");
        vm.run_to_end().expect("run");
        vm
    }

    /// F2: `build_sim` is the caller-facing dispatch that routes a queue model
    /// through the special expansion build path, while the ordinary
    /// `compile_project_incremental` still (correctly) rejects an un-expanded
    /// queue with the `QueueNotExpanded` guard. This pins that the dispatch --
    /// not a weakened guard -- is what lets every non-`sim_new` caller simulate a
    /// queue model.
    #[test]
    fn build_sim_routes_queue_around_the_guard() {
        let project = parse(QUEUE_DRAIN);
        let main = project.models[0].name.clone();

        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);

        // The ordinary incremental compile path MUST still reject the un-expanded
        // queue -- the guard's whole purpose (docs/design/queues.md §10.3).
        let guard_err = crate::db::compile_project_incremental(
            &db,
            sync.project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect_err("ordinary compile must reject an un-expanded queue");
        assert_eq!(guard_err.code, crate::common::ErrorCode::QueueNotExpanded);

        // `build_sim` routes around it via the special build path and produces a
        // runnable VM whose queue stock simulates.
        let mut vm = build_sim(
            &mut db,
            sync.project,
            &project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect("build_sim compiles a queue model");
        vm.run_to_end().expect("queue model runs");
        assert!(vm.get_series(&Ident::new("waiting")).is_some());
    }

    #[test]
    fn mid_run_get_value_reads_queue_served_rate() {
        // The queue twin of the conveyor `mid_run_get_value_matches_saved_series`
        // test: after a partial run, the #625 resting-curr Flows re-eval used to
        // re-execute the driven outflow's placeholder `AssignConstCurr 0`, so a
        // mid-run get_value_now of `into_service` read 0 instead of the served
        // rate (the pass-through queue serves the constant inflow, 10, every
        // step). The resting chunk must hold exactly what the resumed run saves
        // for the same time (dt == save_step == 0.25: resting t=2.25 is row 9).
        let project = parse(QUEUE_DRAIN);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build queue vm");

        let names = ["into_service", "waiting", "arrivals"];
        let offs: Vec<usize> = names
            .iter()
            .map(|n| vm.get_offset(&Ident::new(n)).expect("offset"))
            .collect();

        vm.run_to(2.0).expect("run_to 2");
        let mid: Vec<f64> = offs.iter().map(|&o| vm.get_value_now(o)).collect();
        assert!(
            (mid[0] - 10.0).abs() < 1e-9,
            "mid-run into_service {} (want 10, the served rate)",
            mid[0]
        );

        vm.run_to_end().expect("run");
        for (i, name) in names.iter().enumerate() {
            let series = vm.get_series(&Ident::new(name)).expect(name);
            assert_eq!(
                series[9], mid[i],
                "{name}: mid-run read {} != saved row {}",
                mid[i], series[9]
            );
        }
    }

    /// Assert that a run segmented by mid-run rests produces BIT-identical
    /// saved series to an uninterrupted run, for EVERY variable -- the queue /
    /// coupled twin of `conveyor_compile`'s helper of the same name: the #625
    /// resting-curr pass preview must have no side effect on the real FIFO /
    /// belt side tables (no double-advance), or every subsequent row shifts.
    use crate::test_common::assert_segmented_run_identical;

    #[test]
    fn segmented_run_is_bit_identical_to_uninterrupted() {
        assert_segmented_run_identical(&parse(QUEUE_DRAIN), &[1.0, 3.0]);
    }

    #[test]
    fn coupled_segmented_run_is_bit_identical_to_uninterrupted() {
        // Queue -> discrete conveyor coupling: the resting preview runs the
        // COMBINED pass (admit + coupled serve + phase B) on cloned side
        // tables, so two mid-run rests -- one while the belt is filling, one
        // after it reaches capacity -- must not perturb the saved series.
        assert_segmented_run_identical(&parse(QUEUE_TO_DISCRETE_CONVEYOR), &[1.0, 3.0]);
    }

    /// F2: `build_sim` on an ordinary model (no special stock) is exactly the
    /// incremental compile path -- it succeeds and yields a runnable VM without
    /// touching the special build.
    #[test]
    fn build_sim_ordinary_model_uses_incremental_path() {
        let project = parse(
            r#"<?xml version="1.0"?><xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
            <header><name>p</name><vendor>t</vendor><product version="1.0">t</product></header>
            <sim_specs method="Euler"><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
            <model><variables><aux name="a"><eqn>1</eqn></aux></variables></model></xmile>"#,
        );
        let main = project.models[0].name.clone();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let mut vm = build_sim(
            &mut db,
            sync.project,
            &project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect("ordinary model builds");
        vm.run_to_end().expect("ordinary model runs");
    }

    // ----- F3: conveyor/queue in a non-main model is a clear, user-facing
    // rejection (support is main-model only for now), NOT the internal
    // ConveyorNotExpanded/QueueNotExpanded guard error -----

    /// Main model instantiates `sub` as a module; `sub` holds a `<conveyor>`
    /// stock. The expansion pass rewrites only the main model, so the conveyor
    /// marker survives to the ordinary compile path -- but this is a deferred
    /// feature (main-model only), not an engine bug, so it must be rejected with
    /// `ConveyorInSubmodelUnsupported` naming the stock and its model.
    const CONVEYOR_IN_SUBMODEL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model>
    <variables>
      <module name="sub"/>
    </variables>
  </model>
  <model name="sub">
    <variables>
      <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
        <conveyor><len>4</len></conveyor></stock>
      <flow name="in_f"><eqn>10</eqn></flow>
      <flow name="out_f"><eqn>0</eqn></flow>
    </variables>
  </model>
</xmile>"#;

    /// Same shape as [`CONVEYOR_IN_SUBMODEL`] but `sub` holds a `<queue/>` stock.
    const QUEUE_IN_SUBMODEL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model>
    <variables>
      <module name="sub"/>
    </variables>
  </model>
  <model name="sub">
    <variables>
      <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_service</outflow>
        <queue/></stock>
      <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
      <flow name="into_service"><eqn>0</eqn></flow>
      <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
    </variables>
  </model>
</xmile>"#;

    /// A `<conveyor>` stock in a model that is DEFINED but never instantiated as a
    /// module (a "dead" model): the main model references nothing. The all-models
    /// guard scans every synced model, so the dead model is rejected the same way
    /// -- the deliberate, simplest behavior (a special stock anywhere but the main
    /// model is unsupported, whether or not it is reachable).
    const CONVEYOR_IN_DEAD_MODEL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model>
    <variables>
      <aux name="a"><eqn>1</eqn></aux>
    </variables>
  </model>
  <model name="dead">
    <variables>
      <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
        <conveyor><len>4</len></conveyor></stock>
      <flow name="in_f"><eqn>10</eqn></flow>
      <flow name="out_f"><eqn>0</eqn></flow>
    </variables>
  </model>
</xmile>"#;

    /// Compile `project` through `build_sim` (the shared engine dispatch every
    /// caller other than `simlin_sim_new` uses) and return the expected error.
    fn build_sim_err(project: &datamodel::Project) -> crate::common::Error {
        let main = project
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("a main model")
            .name
            .clone();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, project, None);
        build_sim(
            &mut db,
            sync.project,
            project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect_err("a special stock outside the main model must be rejected")
    }

    #[test]
    fn conveyor_in_submodel_rejected_by_build_compiled() {
        let project = parse(CONVEYOR_IN_SUBMODEL);
        let err = build_compiled_fresh(&project, "main")
            .expect_err("a conveyor in a sub-model must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorInSubmodelUnsupported);
        let details = err.details.expect("a diagnostic message");
        assert!(details.contains("belt"), "names the stock: {details}");
        assert!(details.contains("sub"), "names the model: {details}");
    }

    #[test]
    fn conveyor_in_submodel_rejected_by_build_sim() {
        let project = parse(CONVEYOR_IN_SUBMODEL);
        let err = build_sim_err(&project);
        assert_eq!(err.code, ErrorCode::ConveyorInSubmodelUnsupported);
    }

    #[test]
    fn queue_in_submodel_rejected_by_build_compiled() {
        let project = parse(QUEUE_IN_SUBMODEL);
        let err = build_compiled_fresh(&project, "main")
            .expect_err("a queue in a sub-model must be rejected");
        assert_eq!(err.code, ErrorCode::QueueInSubmodelUnsupported);
        let details = err.details.expect("a diagnostic message");
        assert!(details.contains("waiting"), "names the stock: {details}");
        assert!(details.contains("sub"), "names the model: {details}");
    }

    #[test]
    fn queue_in_submodel_rejected_by_build_sim() {
        let project = parse(QUEUE_IN_SUBMODEL);
        let err = build_sim_err(&project);
        assert_eq!(err.code, ErrorCode::QueueInSubmodelUnsupported);
    }

    /// The SPECIAL build path (a main-model conveyor routes through
    /// `build_compiled`/`build_vm`) must ALSO reject a conveyor in a sub-model.
    /// `expand_conveyors` clears the MAIN belt's marker, so only the sub-model's
    /// marker survives to the guard -- which reports the clear limitation naming
    /// the SUB-model's stock, deterministically (the expanded main marker is
    /// gone, so there is no ambiguity about which stock trips the guard).
    #[test]
    fn special_path_rejects_conveyor_in_submodel_alongside_main() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model>
    <variables>
      <module name="child"/>
      <stock name="trunk"><eqn>0</eqn><inflow>t_in</inflow><outflow>t_out</outflow>
        <conveyor><len>4</len></conveyor></stock>
      <flow name="t_in"><eqn>10</eqn></flow>
      <flow name="t_out"><eqn>0</eqn></flow>
    </variables>
  </model>
  <model name="child">
    <variables>
      <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
        <conveyor><len>4</len></conveyor></stock>
      <flow name="in_f"><eqn>10</eqn></flow>
      <flow name="out_f"><eqn>0</eqn></flow>
    </variables>
  </model>
</xmile>"#;
        let project = parse(xml);
        // `project_has_conveyor(main)` is true (main has `trunk`), so every
        // dispatch takes the special path; it must still reject the sub-model belt.
        assert!(crate::conveyor_compile::project_has_conveyor(
            &project, "main"
        ));
        let err = build_compiled_fresh(&project, "main")
            .expect_err("a sub-model conveyor must reject even via the special path");
        assert_eq!(err.code, ErrorCode::ConveyorInSubmodelUnsupported);
        let details = err.details.expect("a diagnostic message");
        assert!(
            details.contains("belt"),
            "names the sub-model stock: {details}"
        );
        assert!(details.contains("child"), "names the sub-model: {details}");
    }

    #[test]
    fn conveyor_in_dead_model_rejected() {
        // A special stock in a defined-but-uninstantiated model is still caught:
        // the guard scans every synced model, so it can never slip through as a
        // silently-mis-simulated plain stock.
        let project = parse(CONVEYOR_IN_DEAD_MODEL);
        let err = build_sim_err(&project);
        assert_eq!(err.code, ErrorCode::ConveyorInSubmodelUnsupported);
        let details = err.details.expect("a diagnostic message");
        assert!(details.contains("dead"), "names the dead model: {details}");
    }

    #[test]
    fn project_has_queue_detects_marker() {
        let project = parse(QUEUE_DRAIN);
        assert!(project_has_queue(&project, &project.models[0].name));
        // A plain model has no queue.
        let plain = parse(
            r#"<?xml version="1.0"?><xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
            <header><name>p</name><vendor>t</vendor><product version="1.0">t</product></header>
            <sim_specs method="Euler"><start>0</start><stop>1</stop><dt>1</dt></sim_specs>
            <model><variables><aux name="a"><eqn>1</eqn></aux></variables></model></xmile>"#,
        );
        assert!(!project_has_queue(&plain, &plain.models[0].name));
    }

    #[test]
    fn expand_clears_marker_and_drives_outflow() {
        let project = parse(QUEUE_DRAIN);
        let (expanded, metas) = expand_queues(&project, &project.models[0].name).unwrap();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].stock, "waiting");
        assert_eq!(metas[0].inflows, vec!["arrivals".to_string()]);
        assert_eq!(metas[0].outflows.len(), 1);
        assert_eq!(metas[0].outflows[0].flow, "into_service");
        // The queue marker is cleared and the outflow is a placeholder-0 flow.
        let stock = expanded.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Stock(s) if canon(&s.ident) == "waiting" => Some(s),
                _ => None,
            })
            .unwrap();
        assert!(stock.compat.queue.is_none(), "queue marker must be cleared");
        let outflow = expanded.models[0]
            .variables
            .iter()
            .find_map(|v| match v {
                datamodel::Variable::Flow(f) if canon(&f.ident) == "into_service" => Some(f),
                _ => None,
            })
            .unwrap();
        assert_eq!(outflow.equation, Equation::Scalar("0".to_string()));
    }

    #[test]
    fn set_value_on_pass_driven_queue_slots_rejected() {
        // GH #871 (queue side): the expansion rewrites the driven outflow to a
        // placeholder `0` that compiles to an overridable AssignConstCurr, but
        // the queue pass overwrites the slot every step -- so an accepted
        // override would be silently ineffective. It must be rejected like any
        // computed flow. The pass-published container stock rejects too (a
        // stock was never overridable; pinned so the "every pass-written slot
        // rejects" invariant stays explicit). An equation-driven INFLOW is a
        // genuine pass input (the admit request), so its constant override
        // stays accepted and changes the served rate.
        let project = parse(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>queue override</name><vendor>test</vendor><product version="1.0">test</product></header>
  <sim_specs method="Euler" time_units="Months">
    <start>0</start><stop>4</stop><dt>0.25</dt>
  </sim_specs>
  <model><variables>
    <stock name="waiting">
      <eqn>0</eqn>
      <inflow>arrivals</inflow>
      <outflow>into_service</outflow>
      <queue/>
    </stock>
    <flow name="arrivals"><eqn>10</eqn></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
    <aux name="backlog"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#,
        );
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build queue vm");

        for name in ["into_service", "$queue$sum$waiting"] {
            let err = vm.set_value(&Ident::new(name), 999.0).unwrap_err();
            assert_eq!(
                err.code,
                crate::common::ErrorCode::BadOverride,
                "set_value('{name}') must be rejected"
            );
        }

        vm.set_value(&Ident::new("arrivals"), 4.0)
            .expect("inflow override must be accepted");
        vm.run_to_end().expect("run");
        // The queue is a faithful pass-through, so the served rate follows the
        // OVERRIDDEN inflow -- proof the inflow override is a real input while
        // the rejected outflow override left no trace.
        let served = vm
            .get_series(&Ident::new("into_service"))
            .expect("into_service");
        for (i, &o) in served.iter().enumerate() {
            assert!(
                (o - 4.0).abs() < 1e-9,
                "step {i}: into_service={o} (want 4)"
            );
        }
    }

    #[test]
    fn scalar_queue_passes_material_through_at_steady_state() {
        // No initial batch: the queue is a faithful pass-through. Admit-then-
        // serve (§4.2/§4.3) drains the just-arrived batch every step, so the
        // unconstrained outflow equals the inflow and `served` accumulates it.
        let project = parse(QUEUE_DRAIN);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let into_service = vm.get_series(&Ident::new("into_service")).unwrap();
        let served = vm.get_series(&Ident::new("served")).unwrap();
        let arrivals = vm.get_series(&Ident::new("arrivals")).unwrap();

        // The queue never holds material across a step (empties every DT).
        for (i, &w) in waiting.iter().enumerate() {
            assert!(w.abs() < 1e-9, "waiting[{i}] = {w}, expected ~0");
        }
        // Unconstrained outflow == inflow at every step (pass-through).
        for (i, (&o, &a)) in into_service.iter().zip(arrivals.iter()).enumerate() {
            assert!(
                (o - a).abs() < 1e-9,
                "into_service[{i}] = {o}, arrivals = {a}"
            );
        }
        // `served` accumulates the throughput: 10/time * elapsed time. At the
        // last saved step (t=4) it should be ~ 10 * 4 = 40 (Euler, drained each
        // step at rate 10).
        let last = *served.last().unwrap();
        assert!(
            (last - 40.0).abs() < 1e-6,
            "served final = {last}, expected 40"
        );
    }

    #[test]
    fn positive_initial_value_drains_on_first_step() {
        // Seed the queue with an initial batch of 30 and NO inflow. The initial
        // batch must leave entirely through the unconstrained outflow on the
        // first step (§4.3): the outflow rate = 30/dt at step 0, and the queue is
        // empty thereafter.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>30</eqn><outflow>drain</outflow><queue/></stock>
    <flow name="drain"><eqn>0</eqn></flow>
    <stock name="sink"><eqn>0</eqn><inflow>drain</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let drain = vm.get_series(&Ident::new("drain")).unwrap();
        let sink = vm.get_series(&Ident::new("sink")).unwrap();

        // t=0 holds the initial batch (start-of-step); it drains during step 0.
        assert!(
            (waiting[0] - 30.0).abs() < 1e-9,
            "waiting[0] = {}",
            waiting[0]
        );
        // Step-0 outflow rate = 30 / dt(0.5) = 60.
        assert!((drain[0] - 60.0).abs() < 1e-9, "drain[0] = {}", drain[0]);
        // After the first step the queue is empty and stays empty.
        for (i, &w) in waiting.iter().enumerate().skip(1) {
            assert!(w.abs() < 1e-9, "waiting[{i}] = {w}, expected drained");
        }
        // All 30 units end up in the sink.
        let last = *sink.last().unwrap();
        assert!((last - 30.0).abs() < 1e-6, "sink final = {last}");
    }

    #[test]
    fn queue_under_rk4_is_rejected() {
        let xml = QUEUE_DRAIN.replace("method=\"Euler\"", "method=\"RK4\"");
        let project = parse(&xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("RK4 queue must be rejected");
        assert_eq!(err.code, ErrorCode::QueueNonEulerMethod);
    }

    #[test]
    fn queue_under_model_level_rk4_override_is_rejected() {
        // The Euler-only gate must read the ROOT MODEL's sim_specs override
        // (the runtime prefers it -- assemble.rs's root rule), not just the
        // project's: a model-level RK4 override would otherwise evade
        // QueueNonEulerMethod and integrate the FIFO under RK.
        let mut project = parse(QUEUE_DRAIN);
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
        assert_eq!(err.code, ErrorCode::QueueNonEulerMethod);
    }

    #[test]
    fn unexpanded_queue_rejected_by_ordinary_compile() {
        // A `<queue/>` marker reaching the ordinary incremental compile path
        // un-expanded must be rejected loudly (mirrors the conveyor guard), so no
        // ordinary/wasmgen path silently integrates the FIFO as a plain stock.
        let project = parse(QUEUE_DRAIN);
        let main = project.models[0].name.clone();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let err = crate::db::compile_project_incremental(
            &db,
            sync.project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect_err("un-expanded queue must be rejected");
        assert_eq!(err.code, ErrorCode::QueueNotExpanded);
    }

    #[test]
    fn minimal_queue_fixture_with_overflow_drains_via_primary() {
        // The committed fixture has two outflows (primary + an <overflow/>). With
        // no upstream conveyor the primary is never blocked, so it empties the
        // queue and the overflow drains nothing (§4.5). Both compile as ordinary
        // driven flows.
        let xml = include_str!("../../../test/queues/minimal_queue.xmile");
        let project = parse(xml);
        let vm = build_run(&project);
        let into_service = vm.get_series(&Ident::new("into_service")).unwrap();
        let balk = vm.get_series(&Ident::new("balk")).unwrap();
        let arrivals = vm.get_series(&Ident::new("arrivals")).unwrap();
        // Primary == inflow (pass-through); overflow == 0 (nothing blocked).
        for (i, ((&o, &b), &a)) in into_service
            .iter()
            .zip(balk.iter())
            .zip(arrivals.iter())
            .enumerate()
        {
            assert!(
                (o - a).abs() < 1e-9,
                "into_service[{i}] = {o}, arrivals = {a}"
            );
            assert!(b.abs() < 1e-9, "balk[{i}] = {b}, expected 0");
        }
    }

    #[test]
    fn equation_reading_queue_driven_outflow_is_rejected() {
        // An ordinary aux reading the queue's driven outflow would see the
        // pre-pass placeholder 0 (the pass runs after Flows), so `x` would be 0
        // every step instead of 20. Reject it loudly (§2 "Driven outflow").
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_service</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <aux name="x"><eqn>into_service * 2</eqn></aux>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let (code, msg) =
            expand_queues(&project, &project.models[0].name).expect_err("reader must be rejected");
        assert_eq!(code, ErrorCode::QueueDrivenFlowRead);
        assert!(
            msg.contains("into_service"),
            "message names the outflow: {msg}"
        );
        assert!(msg.contains('x'), "message names the reader: {msg}");
    }

    #[test]
    fn stock_fed_by_driven_outflow_via_integ_is_not_rejected() {
        // The `served` stock's <inflow>into_service</inflow> is a STRUCTURAL
        // linkage, NOT an equation reference. It must NOT be caught by the
        // driven-flow-read scan -- a stock integrating the driven outflow is
        // correct (the Stocks phase runs after the pass). This is the ordinary
        // queue->stock model and must still simulate.
        let project = parse(QUEUE_DRAIN);
        // Expansion succeeds (no rejection) ...
        expand_queues(&project, &project.models[0].name)
            .expect("stock fed via INTEG must not be rejected");
        // ... and it simulates as a pass-through.
        let vm = build_run(&project);
        let served = vm.get_series(&Ident::new("served")).unwrap();
        assert!((*served.last().unwrap() - 40.0).abs() < 1e-6);
    }

    #[test]
    fn conveyor_and_queue_coexist_in_one_model() {
        // A model with BOTH a conveyor (Students, transit 4, steady state) and an
        // independent queue (waiting -> served, pass-through) simulates both
        // correctly through the single unified build path: the VM carries both
        // plan sets and runs both passes between Flows and Stocks.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>both</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>8</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="students">
      <eqn>1000</eqn>
      <inflow>matriculating</inflow>
      <outflow>graduating</outflow>
      <conveyor><len>4</len><capacity>1200</capacity></conveyor>
    </stock>
    <flow name="matriculating"><eqn>250</eqn><non_negative/></flow>
    <flow name="graduating"></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>

    <stock name="waiting">
      <eqn>0</eqn>
      <inflow>arrivals</inflow>
      <outflow>into_service</outflow>
      <queue/>
    </stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);

        // Conveyor: steady state -- students hold flat at 1000, graduating == 250.
        let students = vm.get_series(&Ident::new("students")).unwrap();
        let graduating = vm.get_series(&Ident::new("graduating")).unwrap();
        for (i, &s) in students.iter().enumerate() {
            assert!((s - 1000.0).abs() < 1e-6, "students[{i}] = {s}");
        }
        for (i, &g) in graduating.iter().enumerate() {
            assert!((g - 250.0).abs() < 1e-6, "graduating[{i}] = {g}");
        }

        // Queue: pass-through -- into_service == arrivals, waiting stays ~0.
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let into_service = vm.get_series(&Ident::new("into_service")).unwrap();
        for (i, &w) in waiting.iter().enumerate() {
            assert!(w.abs() < 1e-9, "waiting[{i}] = {w}");
        }
        for (i, &o) in into_service.iter().enumerate() {
            assert!((o - 10.0).abs() < 1e-9, "into_service[{i}] = {o}");
        }
    }

    /// F12: a SINGLE stock carrying BOTH a `<conveyor>` block and a `<queue/>`
    /// marker is a type conflict (a stock has exactly one type, §10.7). Nothing
    /// downstream reconciles the two markers: `expand_conveyors` clears only the
    /// conveyor block and `expand_queues` then re-expands the still-queue-marked
    /// stock, so the stock and its shared outflow slot end up with BOTH a
    /// `ConveyorPlan` and a `QueuePlan` and the two passes silently fight over the
    /// slot. `build_compiled` must reject it up front (before either expansion),
    /// naming the stock.
    ///
    /// This differs from `conveyor_and_queue_coexist_in_one_model` (two DISTINCT
    /// stocks), which is the legitimate coexistence case and still compiles.
    const BOTH_MARKERS_STOCK: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>both markers</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="belt">
      <eqn>10</eqn>
      <inflow>into_belt</inflow>
      <outflow>out</outflow>
      <conveyor><len>4</len></conveyor>
      <queue/>
    </stock>
    <flow name="into_belt"><eqn>5</eqn><non_negative/></flow>
    <flow name="out"><eqn>0</eqn></flow>
    <stock name="done"><eqn>0</eqn><inflow>out</inflow></stock>
  </variables></model>
</xmile>"#;

    #[test]
    fn stock_with_both_conveyor_and_queue_markers_rejected_by_build_compiled() {
        let project = parse(BOTH_MARKERS_STOCK);
        let main = project.models[0].name.clone();
        let err = build_compiled_fresh(&project, &main)
            .expect_err("a stock marked as both a conveyor and a queue must be rejected");
        assert_eq!(err.code, ErrorCode::StockBothConveyorAndQueue);
        let details = err.details.expect("a diagnostic message");
        assert!(details.contains("belt"), "names the stock: {details}");
    }

    #[test]
    fn stock_with_both_conveyor_and_queue_markers_rejected_by_build_sim() {
        let project = parse(BOTH_MARKERS_STOCK);
        let main = project.models[0].name.clone();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let err = build_sim(
            &mut db,
            sync.project,
            &project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect_err("build_sim must reject a both-marked stock");
        assert_eq!(err.code, ErrorCode::StockBothConveyorAndQueue);
    }

    #[test]
    fn reset_reinitializes_queue_state() {
        // After a full run, reset + re-run must reproduce the initial-drain
        // trajectory (the side table is re-seeded from the stock's initial value,
        // not left stale/empty from the prior run).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>12</eqn><outflow>drain</outflow><queue/></stock>
    <flow name="drain"><eqn>0</eqn></flow>
    <stock name="sink"><eqn>0</eqn><inflow>drain</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).unwrap();
        vm.run_to_end().unwrap();
        let first = *vm.get_series(&Ident::new("sink")).unwrap().last().unwrap();

        vm.reset();
        vm.run_to_end().unwrap();
        let second = *vm.get_series(&Ident::new("sink")).unwrap().last().unwrap();
        assert!((first - 12.0).abs() < 1e-6, "first run sink = {first}");
        assert!(
            (second - first).abs() < 1e-12,
            "reset must reproduce the run: {first} vs {second}"
        );
    }

    // ----- Part A: arrayed queues (§6) -----

    /// A 2-element arrayed queue draining to a stock, with element-specific
    /// inflows. Each element is an INDEPENDENT FIFO: `board=a` admits 10/time and
    /// `board=b` admits 25/time, both drain unconstrained (pass-through), so
    /// `into_service[a] == 10`, `into_service[b] == 25`, each `waiting[elem]` stays
    /// ~0, and `served[elem]` accumulates its own throughput independently. This
    /// mirrors `conveyor_compile`'s `arrayed_*` tests: `resolve_plans` flattens the
    /// one arrayed `QueueMeta` into one scalar plan per element.
    const ARRAYED_QUEUE_DRAIN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>arrayed queue</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.25</dt></sim_specs>
  <dimensions><dim name="board"><elem name="a"/><elem name="b"/></dim></dimensions>
  <model><variables>
    <stock name="waiting">
      <eqn>0</eqn>
      <inflow>arrivals</inflow>
      <outflow>into_service</outflow>
      <dimensions><dim name="board"/></dimensions>
      <queue/>
    </stock>
    <flow name="arrivals">
      <element subscript="a"><eqn>10</eqn></element>
      <element subscript="b"><eqn>25</eqn></element>
      <dimensions><dim name="board"/></dimensions>
      <non_negative/>
    </flow>
    <flow name="into_service"><dimensions><dim name="board"/></dimensions></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow>
      <dimensions><dim name="board"/></dimensions></stock>
  </variables></model>
</xmile>"#;

    #[test]
    fn arrayed_queue_drains_each_element_independently() {
        let project = parse(ARRAYED_QUEUE_DRAIN);
        let vm = build_run(&project);

        let waiting_a = vm.get_series(&Ident::new("waiting[a]")).unwrap();
        let waiting_b = vm.get_series(&Ident::new("waiting[b]")).unwrap();
        let into_a = vm.get_series(&Ident::new("into_service[a]")).unwrap();
        let into_b = vm.get_series(&Ident::new("into_service[b]")).unwrap();

        // Each FIFO empties every step (pass-through) with its own inflow rate.
        for (i, (&wa, &wb)) in waiting_a.iter().zip(waiting_b.iter()).enumerate() {
            assert!(wa.abs() < 1e-9, "waiting[a] step {i} = {wa}");
            assert!(wb.abs() < 1e-9, "waiting[b] step {i} = {wb}");
        }
        for (i, (&oa, &ob)) in into_a.iter().zip(into_b.iter()).enumerate() {
            assert!((oa - 10.0).abs() < 1e-9, "into_service[a] step {i} = {oa}");
            assert!((ob - 25.0).abs() < 1e-9, "into_service[b] step {i} = {ob}");
        }
        // Independent accumulation: element a's throughput 10/time, b's 25/time,
        // over 4 time units -> 40 and 100.
        let served_a = vm.get_series(&Ident::new("served[a]")).unwrap();
        let served_b = vm.get_series(&Ident::new("served[b]")).unwrap();
        assert!(
            (*served_a.last().unwrap() - 40.0).abs() < 1e-6,
            "served[a] final = {}",
            served_a.last().unwrap()
        );
        assert!(
            (*served_b.last().unwrap() - 100.0).abs() < 1e-6,
            "served[b] final = {}",
            served_b.last().unwrap()
        );
    }

    // ----- Part B: container access (§8) -----

    /// A pure-accumulator scalar queue (inflow, NO outflow) whose batches persist,
    /// so container access has a multi-batch queue to read. With dt=1 and
    /// `arrivals = 10*(TIME+1)`, step `t` admits a batch of `10*(t+1)`, so at the
    /// START of step `t` (what container access publishes) the batch vector,
    /// front-to-back (oldest-first), is `[10, 20, ..., 10*t]`. `reader` reads
    /// `reader_eqn` over the queue; the returned series is start-of-step per §8.
    fn accumulator_reader(reader_eqn: &str) -> Vec<f64> {
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q container</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>10 * (TIME + 1)</eqn><non_negative/></flow>
    <aux name="reader"><eqn>{reader_eqn}</eqn></aux>
  </variables></model>
</xmile>"#
        );
        let project = parse(&xml);
        let vm = build_run(&project);
        vm.get_series(&Ident::new("reader")).expect("reader series")
    }

    #[test]
    fn scalar_queue_container_size_sum_and_reducers() {
        // Start-of-step batch vectors: t=0 [], t=1 [10], t=2 [10,20],
        // t=3 [10,20,30], t=4 [10,20,30,40].
        assert_eq!(
            accumulator_reader("SIZE(waiting)"),
            vec![0.0, 1.0, 2.0, 3.0, 4.0]
        );
        // SUM is the batch total (0 on an empty queue, matching the VM reducer).
        assert_eq!(
            accumulator_reader("SUM(waiting)"),
            vec![0.0, 10.0, 30.0, 60.0, 100.0]
        );
        let mean = accumulator_reader("MEAN(waiting)");
        assert!(mean[0].is_nan(), "MEAN of an empty queue is NaN");
        assert_eq!(&mean[1..], &[10.0, 15.0, 20.0, 25.0]);
        let min = accumulator_reader("MIN(waiting)");
        assert!(min[0].is_nan());
        assert_eq!(&min[1..], &[10.0, 10.0, 10.0, 10.0]); // front (oldest) is smallest
        let max = accumulator_reader("MAX(waiting)");
        assert!(max[0].is_nan());
        assert_eq!(&max[1..], &[10.0, 20.0, 30.0, 40.0]); // back (newest) is largest
        // Population STDDEV of [10,20,30,40]: mean 25, var 125, std sqrt(125).
        let stddev = accumulator_reader("STDDEV(waiting)");
        assert!(stddev[0].is_nan());
        assert!(
            (stddev[4] - 125.0_f64.sqrt()).abs() < 1e-9,
            "STDDEV[4] = {}",
            stddev[4]
        );
    }

    #[test]
    fn queue_container_init_reads_start_of_run_not_placeholder() {
        // A queue seeded with an initial batch of 40 (so the FIFO starts
        // non-empty), pure accumulator (arrivals, no outflow). SUM(waiting) grows
        // over the run, but INIT(SUM(waiting)) must be the START-OF-RUN batch total
        // (40) at every step -- not the hidden container stock's '0' placeholder.
        // The rewrite turns both SUM(waiting) and INIT(SUM(waiting)) into the
        // hidden stock $queue$sum$waiting; its initial_values snapshot must be
        // patched to the seeded FIFO's total (pre-fix INIT read the frozen 0).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q init</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>40</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <aux name="init_sum"><eqn>INIT(SUM(waiting))</eqn></aux>
    <aux name="init_size"><eqn>INIT(SIZE(waiting))</eqn></aux>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let init_sum = vm.get_series(&Ident::new("init_sum")).expect("init_sum");
        for (i, &v) in init_sum.iter().enumerate() {
            assert!(
                (v - 40.0).abs() < 1e-9,
                "step {i}: INIT(SUM(waiting)) = {v} (want 40; pre-fix 0)"
            );
        }
        // The seeded queue starts with exactly one batch, so INIT(SIZE)==1.
        let init_size = vm.get_series(&Ident::new("init_size")).expect("init_size");
        for (i, &v) in init_size.iter().enumerate() {
            assert!(
                (v - 1.0).abs() < 1e-9,
                "step {i}: INIT(SIZE(waiting)) = {v} (want 1; pre-fix 0)"
            );
        }
    }

    #[test]
    fn scalar_queue_batch_index_and_out_of_range_is_nan() {
        // `queue[k]` is 1-based from the FRONT (oldest). k outside [1, count] -> NaN.
        let front = accumulator_reader("waiting[1]");
        assert!(front[0].is_nan(), "waiting[1] on empty queue -> NaN");
        assert_eq!(&front[1..], &[10.0, 10.0, 10.0, 10.0]); // oldest batch
        // waiting[count] is the newest batch: at t=4 (count 4) it is 40.
        let second = accumulator_reader("waiting[2]");
        assert!(second[0].is_nan() && second[1].is_nan(), "count<2 -> NaN");
        assert_eq!(&second[2..], &[20.0, 20.0, 20.0]); // 2nd-from-front
        // queue[0] is always out of range (1-based); queue[5] is out of range when
        // the queue holds fewer than 5 batches.
        for &v in accumulator_reader("waiting[0]").iter() {
            assert!(v.is_nan(), "waiting[0] -> NaN");
        }
        for &v in accumulator_reader("waiting[5]").iter() {
            assert!(v.is_nan(), "waiting[5] -> NaN (never 5 batches)");
        }
    }

    #[test]
    fn scalar_queue_container_is_start_of_step() {
        // §8 start-of-step visibility: `SUM(waiting)` at step t sees the batches
        // admitted in PRIOR steps only, NOT this step's admit. At t=1 that is a
        // single batch of 10 (the step-0 admit), NOT 30 (which would include the
        // step-1 admit of 20). This is the load-bearing timing assertion.
        let sum = accumulator_reader("SUM(waiting)");
        assert_eq!(
            sum[1], 10.0,
            "start-of-step: only the step-0 admit is visible"
        );
        // A container access nested in a larger expression is rewritten in place,
        // so surrounding math is preserved: SUM(waiting)+1.
        let plus = accumulator_reader("SUM(waiting) + 1");
        assert_eq!(plus, vec![1.0, 11.0, 31.0, 61.0, 101.0]);
    }

    /// A 2-element pure-accumulator arrayed queue (per-element inflows a=10, b=25,
    /// dt=1) with scalar readers over each element's batches (§8). At the last
    /// saved step (t=4) each element holds 4 batches: a = [10,10,10,10],
    /// b = [25,25,25,25].
    const ARRAYED_QUEUE_CONTAINER: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>arrayed q container</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <dimensions><dim name="board"><elem name="a"/><elem name="b"/></dim></dimensions>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <dimensions><dim name="board"/></dimensions><queue/></stock>
    <flow name="arrivals">
      <element subscript="a"><eqn>10</eqn></element>
      <element subscript="b"><eqn>25</eqn></element>
      <dimensions><dim name="board"/></dimensions><non_negative/></flow>
    <aux name="sum_a"><eqn>SUM(waiting[a])</eqn></aux>
    <aux name="sum_b"><eqn>SUM(waiting[b])</eqn></aux>
    <aux name="size_a"><eqn>SIZE(waiting[a])</eqn></aux>
    <aux name="front_a"><eqn>waiting[a, 1]</eqn></aux>
    <aux name="second_b"><eqn>waiting[b, 2]</eqn></aux>
  </variables></model>
</xmile>"#;

    #[test]
    fn arrayed_queue_container_access_reads_per_element() {
        let project = parse(ARRAYED_QUEUE_CONTAINER);
        let vm = build_run(&project);
        let last = |name: &str| *vm.get_series(&Ident::new(name)).unwrap().last().unwrap();
        // Independent per-element batch totals (start-of-step at t=4: 4 batches).
        assert!((last("sum_a") - 40.0).abs() < 1e-9, "SUM(waiting[a])");
        assert!((last("sum_b") - 100.0).abs() < 1e-9, "SUM(waiting[b])");
        assert!((last("size_a") - 4.0).abs() < 1e-9, "SIZE(waiting[a])");
        // waiting[a,1] is element a's front batch (10); waiting[b,2] is element b's
        // 2nd-from-front batch (25) -- indexing one element's FIFO.
        assert!((last("front_a") - 10.0).abs() < 1e-9, "waiting[a,1]");
        assert!((last("second_b") - 25.0).abs() < 1e-9, "waiting[b,2]");
    }

    #[test]
    fn residual_queue_container_access_is_rejected() {
        // Genuinely-unlowerable container forms loud-reject with the shared
        // `ConveyorContainerAccessUnsupported` (§8): a reducer over an EXPRESSION
        // of the batches, a dynamic (non-constant) index, and a range/wildcard
        // over batches all need the per-batch vector, which cannot lower to one
        // native scalar.
        for reader in [
            "SUM(waiting / 2)",
            "MEAN(waiting + 0)",
            "waiting[k]",
            "waiting[1:2]",
            "waiting[*]",
        ] {
            let xml = format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <aux name="reader"><eqn>{reader}</eqn></aux>
  </variables></model>
</xmile>"#
            );
            let project = parse(&xml);
            let main = project.models[0].name.clone();
            let err = build_vm(&project, &main)
                .err()
                .unwrap_or_else(|| panic!("residual '{reader}' should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorContainerAccessUnsupported,
                "reader '{reader}'"
            );
        }
    }

    #[test]
    fn arrayed_bare_non_sum_queue_reducer_is_rejected() {
        // A bare arrayed-queue reducer other than SUM has no single-queue
        // interpretation (it would read per-element TOTALS, not batches), so it
        // stays loud-rejected -- mirroring the conveyor rule (§8).
        for reader in ["MEAN(waiting)", "MIN(waiting)", "SIZE(waiting)"] {
            let xml = format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <dimensions><dim name="board"><elem name="a"/><elem name="b"/></dim></dimensions>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <dimensions><dim name="board"/></dimensions><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn>
      <dimensions><dim name="board"/></dimensions><non_negative/></flow>
    <aux name="reader"><eqn>{reader}</eqn></aux>
  </variables></model>
</xmile>"#
            );
            let project = parse(&xml);
            let main = project.models[0].name.clone();
            let err = build_vm(&project, &main)
                .err()
                .unwrap_or_else(|| panic!("bare arrayed reducer '{reader}' should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorContainerAccessUnsupported,
                "reader '{reader}'"
            );
        }
    }

    // ----- Part B: queue-conveyor coupling (§9 / conveyors.md §11) -----

    /// A queue feeding a discrete conveyor whose capacity throttles admission.
    /// `transit=100` (nothing exits during the short sim), `capacity=10`,
    /// `arrivals=4/time`, `one_at_a_time="false"`, `batch_integrity="false"`, so
    /// each DT the conveyor requests `req = 10 - belt` and the queue supplies
    /// `min(Q, req)` from the front. Belt fills to capacity and blocks; batches
    /// then WAIT in the queue.
    const QUEUE_TO_DISCRETE_CONVEYOR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>coupling</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting">
      <eqn>0</eqn>
      <inflow>arrivals</inflow>
      <outflow>into_belt</outflow>
      <queue/>
    </stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="belt">
      <eqn>0</eqn>
      <inflow>into_belt</inflow>
      <outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><capacity>10</capacity>
      </conveyor>
    </stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;

    #[test]
    fn queue_feeding_discrete_conveyor_waits_at_capacity_and_conserves() {
        let project = parse(QUEUE_TO_DISCRETE_CONVEYOR);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let into_belt = vm.get_series(&Ident::new("into_belt")).unwrap();

        // Hand-computed oracle (start-of-step stock values; step-t flow rate):
        //   step:      0    1    2    3    4    5
        //   belt(0):   0    4    8   10   10   10   (fills to capacity, then holds)
        //   waiting:   0    0    0    2    6   10   (batches wait once belt is full)
        //   into_belt: 4    4    2    0    0    0   (served volume / dt)
        let want_belt = [0.0, 4.0, 8.0, 10.0, 10.0, 10.0];
        let want_waiting = [0.0, 0.0, 0.0, 2.0, 6.0, 10.0];
        let want_into = [4.0, 4.0, 2.0, 0.0, 0.0, 0.0];
        for i in 0..6 {
            assert!(
                (belt[i] - want_belt[i]).abs() < 1e-9,
                "belt[{i}] = {}",
                belt[i]
            );
            assert!(
                (waiting[i] - want_waiting[i]).abs() < 1e-9,
                "waiting[{i}] = {}",
                waiting[i]
            );
            assert!(
                (into_belt[i] - want_into[i]).abs() < 1e-9,
                "into_belt[{i}] = {}",
                into_belt[i]
            );
        }

        // Conservation on BOTH stocks each step: nothing exits the belt (transit
        // 100 >> sim length), so belt + waiting == cumulative arrivals (4 per
        // completed step). At the start of step t, t steps have completed.
        for t in 0..6 {
            let in_system = belt[t] + waiting[t];
            let arrived = 4.0 * t as f64;
            assert!(
                (in_system - arrived).abs() < 1e-9,
                "step {t}: belt+waiting = {in_system}, arrived = {arrived}"
            );
        }
    }

    #[test]
    fn coupled_mid_run_get_value_matches_saved_series() {
        // The COUPLED twin of `mid_run_get_value_reads_queue_served_rate`: the
        // resting preview must run the COMBINED queue-conveyor pass (admit +
        // coupled serve + phase B on cloned side tables), not just the two
        // independent passes. Rest at t=3 (run_to(2.0), dt == save_step == 1),
        // where the hand-computed oracle above pins a fully-blocked shared
        // flow: the belt sits at capacity (10), two units wait in the queue,
        // and into_belt = 0 -- so a placeholder-zero read is only trustworthy
        // because waiting/belt pin the surrounding state, and the saved-row
        // equality check pins every slot exactly.
        let project = parse(QUEUE_TO_DISCRETE_CONVEYOR);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).expect("build coupled vm");

        let names = ["into_belt", "waiting", "belt", "graduating", "arrivals"];
        let offs: Vec<usize> = names
            .iter()
            .map(|n| vm.get_offset(&Ident::new(n)).expect("offset"))
            .collect();

        vm.run_to(2.0).expect("run_to 2");
        let mid: Vec<f64> = offs.iter().map(|&o| vm.get_value_now(o)).collect();
        assert!(
            mid[0].abs() < 1e-9,
            "mid-run into_belt {} (want 0: belt at capacity)",
            mid[0]
        );
        assert!((mid[1] - 2.0).abs() < 1e-9, "mid-run waiting {}", mid[1]);
        assert!((mid[2] - 10.0).abs() < 1e-9, "mid-run belt {}", mid[2]);

        vm.run_to_end().expect("run");
        for (i, name) in names.iter().enumerate() {
            let series = vm.get_series(&Ident::new(name)).expect(name);
            assert_eq!(
                series[3], mid[i],
                "{name}: mid-run read {} != saved row {}",
                mid[i], series[3]
            );
        }
    }

    /// Build the coupling model with a time-varying capacity (`5` until t=3, then
    /// `100`) that lets a two-batch backlog accumulate before opening up, so the
    /// `one_at_a_time` distinction is observable at t=4: all-at-once drains all
    /// three whole batches in one DT, one-at-a-time takes only the front batch.
    fn coupling_backlog_model(one_at_a_time: bool) -> datamodel::Project {
        let oa = if one_at_a_time { "true" } else { "false" };
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>5</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <aux name="cap"><eqn>IF TIME >= 3 THEN 100 ELSE 5</eqn></aux>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="{oa}" batch_integrity="false">
        <len>100</len><capacity>cap</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#
        );
        parse(&xml)
    }

    #[test]
    fn coupling_one_at_a_time_takes_at_most_front_batch() {
        // Backlog Q=[5,5] accrues over t=0..2 (cap 5, belt full). At t=3 cap opens
        // to 100 so req=95: all-at-once drains all three 5-batches (belt -> 20);
        // one-at-a-time takes only the front 5 (belt -> 10). Both conserve
        // (cumulative arrivals 20 = belt + waiting).
        let all = build_run(&coupling_backlog_model(false));
        let oaat = build_run(&coupling_backlog_model(true));
        let belt_all = all.get_series(&Ident::new("belt")).unwrap();
        let belt_oaat = oaat.get_series(&Ident::new("belt")).unwrap();
        let wait_all = all.get_series(&Ident::new("waiting")).unwrap();
        let wait_oaat = oaat.get_series(&Ident::new("waiting")).unwrap();

        assert!(
            (belt_all[4] - 20.0).abs() < 1e-9,
            "all-at-once belt[4] = {}",
            belt_all[4]
        );
        assert!(
            (belt_oaat[4] - 10.0).abs() < 1e-9,
            "one-at-a-time belt[4] = {}",
            belt_oaat[4]
        );
        // Conservation for both at t=4 (4 completed steps * 5 = 20 in system).
        assert!((belt_all[4] + wait_all[4] - 20.0).abs() < 1e-9);
        assert!((belt_oaat[4] + wait_oaat[4] - 20.0).abs() < 1e-9);
    }

    /// Coupling model parameterised on `batch_integrity`, capacity 10, arrivals 4.
    fn coupling_integrity_model(batch_integrity: bool) -> datamodel::Project {
        let bi = if batch_integrity { "true" } else { "false" };
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="true" batch_integrity="{bi}">
        <len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#
        );
        parse(&xml)
    }

    #[test]
    fn coupling_batch_integrity_never_splits_the_front_batch() {
        // arrivals=4/step. Without integrity the belt splits a 4-batch to reach
        // capacity 10 (belt: 0,4,8,10,...). With integrity the 4-batch cannot fit
        // the 2 room left at belt=8, so it waits and the belt STOPS at 8.
        let split = build_run(&coupling_integrity_model(false));
        let whole = build_run(&coupling_integrity_model(true));
        let belt_split = split.get_series(&Ident::new("belt")).unwrap();
        let belt_whole = whole.get_series(&Ident::new("belt")).unwrap();
        let wait_whole = whole.get_series(&Ident::new("waiting")).unwrap();

        // Splitting reaches capacity 10; integrity caps at 8 (2 room, 4-batch).
        assert!(
            (belt_split[3] - 10.0).abs() < 1e-9,
            "split belt[3] = {}",
            belt_split[3]
        );
        for (t, &b) in belt_whole.iter().enumerate().take(6).skip(3) {
            assert!((b - 8.0).abs() < 1e-9, "integrity belt[{t}] = {b}");
        }
        // Conservation with integrity: belt + waiting == cumulative arrivals.
        for t in 0..6 {
            assert!(
                (belt_whole[t] + wait_whole[t] - 4.0 * t as f64).abs() < 1e-9,
                "integrity step {t}: {} + {}",
                belt_whole[t],
                wait_whole[t]
            );
        }
    }

    #[test]
    fn coupling_respects_discrete_inflow_limit_across_sub_unit_dts() {
        // No capacity limit, but in_limit=3 PER TIME UNIT, dt=0.5 (two DTs per time
        // unit). The discrete budget `in_carry` accumulates the queue's coupled
        // admission and resets only at integer time boundaries, so the belt admits
        // at most 3 per time unit even though the queue offers 10/time. This
        // exercises `ConveyorState::consume_inflow_budget` (the coupled volume
        // bypasses phase_b's equation-inflow accounting).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><in_limit>3</in_limit></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        // saved times 0,0.5,1.0,1.5,2.0 (dt=0.5). 3/time-unit admitted; the second
        // DT of each unit has spent the budget so it admits nothing.
        let want_belt = [0.0, 3.0, 3.0, 6.0, 6.0];
        for (i, &w) in want_belt.iter().enumerate() {
            assert!((belt[i] - w).abs() < 1e-9, "belt[{i}] = {}", belt[i]);
        }
        // Conservation: 5 arrives per DT, belt + waiting == 5 * completed DTs.
        for (t, (&b, &q)) in belt.iter().zip(waiting.iter()).enumerate() {
            assert!(
                (b + q - 5.0 * t as f64).abs() < 1e-9,
                "step {t}: belt {b} + waiting {q}"
            );
        }
    }

    #[test]
    fn queue_upstream_of_non_discrete_conveyor_is_rejected() {
        // A queue feeding a NON-discrete (continuous) conveyor is a loud
        // ConveyorQueueUpstreamNotDiscrete error (§9 / conveyors.md §11): the
        // batch-admission rules are only defined for a discrete conveyor.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor><len>4</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let err = build_vm(&project, &main).expect_err("non-discrete conveyor must be rejected");
        assert_eq!(err.code, ErrorCode::ConveyorQueueUpstreamNotDiscrete);
    }

    // ----- F10: a SECONDARY queue outflow feeding a conveyor is rejected -----
    //
    // Only a queue's PRIMARY (first, highest-priority) outflow may feed a conveyor
    // (§4.4/§9). The combined pass couples exactly the primary, so a secondary
    // outflow (an <overflow/> sibling or a second ordinary outflow) whose target is
    // a conveyor escapes the discrete guard AND is not served under the batch rules
    // -- its served rate lands in a slot the destination belt's phase_b independently
    // treats as an equation-driven inflow, silently desyncing the queue FIFO / belt
    // stock from its side table. Rejected loudly at coupling-detection time.

    /// Build a queue model through the special path and return the rejection.
    fn build_vm_reject(xml: &str) -> crate::common::Error {
        let project = parse(xml);
        let main = project.models[0].name.clone();
        build_vm(&project, &main).expect_err("model must be rejected")
    }

    #[test]
    fn queue_secondary_ordinary_outflow_to_discrete_conveyor_is_rejected() {
        // Primary `spill` drains to a regular stock (unconstrained); a SECOND
        // ordinary outflow `into_belt` feeds a DISCRETE conveyor. Only the primary
        // may feed a conveyor, so this is a loud QueueSecondaryOutflowToConveyor
        // naming the queue, the offending outflow, and the conveyor.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>spill</outflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="spill"><eqn>0</eqn></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="sink"><eqn>0</eqn><inflow>spill</inflow></stock>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true"><len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;
        let err = build_vm_reject(xml);
        assert_eq!(err.code, ErrorCode::QueueSecondaryOutflowToConveyor);
        let details = err.details.expect("a diagnostic message");
        assert!(details.contains("waiting"), "names the queue: {details}");
        assert!(
            details.contains("into_belt"),
            "names the outflow: {details}"
        );
        assert!(details.contains("belt"), "names the conveyor: {details}");
    }

    #[test]
    fn queue_secondary_outflow_to_continuous_conveyor_is_rejected() {
        // Same shape, but the destination is a CONTINUOUS (non-discrete) conveyor.
        // Before this guard even the discrete requirement never examined it (only
        // the primary was), so a secondary feeding a continuous conveyor raised NO
        // error at all. It must now be rejected too.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>spill</outflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="spill"><eqn>0</eqn></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="sink"><eqn>0</eqn><inflow>spill</inflow></stock>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor><len>4</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;
        let err = build_vm_reject(xml);
        assert_eq!(err.code, ErrorCode::QueueSecondaryOutflowToConveyor);
    }

    #[test]
    fn queue_overflow_outflow_to_conveyor_is_rejected() {
        // The secondary is an <overflow/> outflow feeding a discrete conveyor. The
        // spec sketches an overflow-to-conveyor (§4.5) but does not define the
        // combined-pass interleave, so it is a deferred feature rejected loudly (not
        // silently mis-served).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_service</outflow><outflow>balk</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <flow name="balk"><eqn>0</eqn><overflow/></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
    <stock name="belt"><eqn>0</eqn><inflow>balk</inflow><outflow>graduating</outflow>
      <conveyor discrete="true"><len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#;
        let err = build_vm_reject(xml);
        assert_eq!(err.code, ErrorCode::QueueSecondaryOutflowToConveyor);
        let details = err.details.expect("a diagnostic message");
        assert!(details.contains("balk"), "names the overflow: {details}");
        assert!(details.contains("belt"), "names the conveyor: {details}");
    }

    #[test]
    fn queue_primary_and_secondary_to_two_different_conveyors_is_rejected() {
        // The verdict's exact two-conveyor scenario: the primary feeds discrete
        // conveyor A (a valid coupling) and the SECONDARY feeds discrete conveyor B.
        // The secondary must be rejected -- naming conveyor B -- even though the
        // primary coupling is itself well-formed.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_belt_a</outflow><outflow>into_belt_b</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt_a"><eqn>0</eqn></flow>
    <flow name="into_belt_b"><eqn>0</eqn></flow>
    <stock name="belt_a"><eqn>0</eqn><inflow>into_belt_a</inflow><outflow>grad_a</outflow>
      <conveyor discrete="true"><len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="grad_a"><eqn>0</eqn></flow>
    <stock name="sink_a"><eqn>0</eqn><inflow>grad_a</inflow></stock>
    <stock name="belt_b"><eqn>0</eqn><inflow>into_belt_b</inflow><outflow>grad_b</outflow>
      <conveyor discrete="true"><len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="grad_b"><eqn>0</eqn></flow>
    <stock name="sink_b"><eqn>0</eqn><inflow>grad_b</inflow></stock>
  </variables></model>
</xmile>"#;
        let err = build_vm_reject(xml);
        assert_eq!(err.code, ErrorCode::QueueSecondaryOutflowToConveyor);
        let details = err.details.expect("a diagnostic message");
        assert!(
            details.contains("into_belt_b"),
            "names the outflow: {details}"
        );
        assert!(details.contains("belt_b"), "names conveyor B: {details}");
    }

    #[test]
    fn queue_primary_to_conveyor_with_ordinary_secondary_to_stock_compiles() {
        // Negative control: the SUPPORTED coupled shape. The primary feeds a
        // discrete conveyor and a secondary ordinary outflow drains the post-primary
        // front to a REGULAR STOCK (not a conveyor). The guard targets conveyor
        // destinations only, so this must still compile and simulate.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>spill</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="spill"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true"><len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="sink"><eqn>0</eqn><inflow>spill</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main)
            .expect("primary->conveyor + ordinary secondary->stock must compile");
        vm.run_to_end()
            .expect("coupled queue with a stock-bound secondary runs");
        // The queue conserves: nothing exits the belt (transit 100 >> sim), so
        // belt + waiting + sink == cumulative arrivals every step.
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let sink = vm.get_series(&Ident::new("sink")).unwrap();
        for t in 0..belt.len() {
            let total = belt[t] + waiting[t] + sink[t];
            assert!(
                (total - 4.0 * t as f64).abs() < 1e-9,
                "step {t}: belt+waiting+sink = {total}, arrived = {}",
                4.0 * t as f64
            );
        }
    }

    #[test]
    fn coupled_queue_reset_reproduces_the_run() {
        // reset() must re-seed both side tables and re-derive the coupling from the
        // re-attached plans, reproducing the coupled trajectory exactly.
        let project = parse(QUEUE_TO_DISCRETE_CONVEYOR);
        let main = project.models[0].name.clone();
        let mut vm = build_vm(&project, &main).unwrap();
        vm.run_to_end().unwrap();
        let first = vm.get_series(&Ident::new("belt")).unwrap().to_vec();
        vm.reset();
        vm.run_to_end().unwrap();
        let second = vm.get_series(&Ident::new("belt")).unwrap();
        for (i, (&a, &b)) in first.iter().zip(second.iter()).enumerate() {
            assert!((a - b).abs() < 1e-12, "reset diverged at {i}: {a} vs {b}");
        }
    }

    // ----- Part D: MULTIPLE queues coupled to ONE discrete conveyor (§9 /
    // conveyors.md §4.3 step 4, §11) -----

    /// Two queues (`waiting_a`, `waiting_b`), each with its own arrivals, whose
    /// PRIMARY outflows both feed ONE discrete belt as its two equation-driven
    /// inflows (`into_belt_a`, `into_belt_b`). Several queues feeding one discrete
    /// conveyor reuse the listed-order admission priority of conveyors.md §4.3
    /// step 4 / §11: the belt's `<inflow>` DECLARATION ORDER is the admission priority.
    ///
    /// `in_limit = 6` per time unit (capacity INF, transit 100 so nothing exits)
    /// throttles the SHARED per-time-unit budget: with arrivals of 4 each, the
    /// higher-priority queue always fits its 4 and the lower-priority one gets only
    /// the 2 the shared budget leaves. `swap` reverses the belt's `<inflow>` order
    /// (so `waiting_b` gets priority instead), proving the order is the conveyor's
    /// inflow order -- NOT queue-plan or HashMap iteration order. `overflow` gives
    /// the LOWER-priority queue (`waiting_b`) an `<overflow/>` sibling draining the
    /// shared-budget-blocked (redirectable) volume to `balked_b`, so it exercises
    /// redirectable accounting measured against the shared budget (§4.5).
    fn two_queue_shared_belt_model(swap: bool, overflow: bool) -> datamodel::Project {
        // The belt lists its two coupled inflows in this order; `swap` flips it.
        let (first, second) = if swap {
            ("into_belt_b", "into_belt_a")
        } else {
            ("into_belt_a", "into_belt_b")
        };
        // The overflow always sits on `waiting_b` (the second-declared queue), so
        // the overflow variant is only meaningful when `waiting_b` is throttled --
        // i.e. `swap = false`.
        let (b_outflows, balk_flow, balked_stock) = if overflow {
            (
                "<outflow>into_belt_b</outflow><outflow>balk_b</outflow>",
                r#"<flow name="balk_b"><eqn>0</eqn><overflow/></flow>"#,
                r#"<stock name="balked_b"><eqn>0</eqn><inflow>balk_b</inflow></stock>"#,
            )
        } else {
            ("<outflow>into_belt_b</outflow>", "", "")
        };
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>two queues</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting_a"><eqn>0</eqn><inflow>arrivals_a</inflow><outflow>into_belt_a</outflow><queue/></stock>
    <flow name="arrivals_a"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt_a"><eqn>0</eqn></flow>
    <stock name="waiting_b"><eqn>0</eqn><inflow>arrivals_b</inflow>{b_outflows}<queue/></stock>
    <flow name="arrivals_b"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt_b"><eqn>0</eqn></flow>
    {balk_flow}
    {balked_stock}
    <stock name="belt"><eqn>0</eqn>
      <inflow>{first}</inflow><inflow>{second}</inflow>
      <outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><in_limit>6</in_limit></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
  </variables></model>
</xmile>"#
        );
        parse(&xml)
    }

    #[test]
    fn two_queues_into_one_conveyor_both_drain_by_priority_and_conserve() {
        // The reproduction of the two-queue-coupling starvation bug: BOTH queues
        // must be served. `waiting_a`'s inflow is declared first, so it has
        // admission priority: it drains its full 4 each step (shared budget 6),
        // and `waiting_b` gets only the 2 the shared per-time-unit budget leaves,
        // so it grows by 2 each step. Before the fix, the second-reconstructed
        // coupling overwrote the first and `waiting_a` was never served: its
        // `into_belt_a` stayed frozen at 0 and its stock grew by 4 every step.
        let vm = build_run(&two_queue_shared_belt_model(false, false));
        let wa = vm.get_series(&Ident::new("waiting_a")).unwrap();
        let wb = vm.get_series(&Ident::new("waiting_b")).unwrap();
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let ia = vm.get_series(&Ident::new("into_belt_a")).unwrap();
        let ib = vm.get_series(&Ident::new("into_belt_b")).unwrap();

        // Hand oracle (start-of-step stocks; step-t flow rates). Budget resets each
        // integer time unit, so the pattern repeats every step:
        //   waiting_a: fully drained every step (priority, 4 <= budget 6)
        //   waiting_b: +2 per step (gets the residual 2 of the shared budget)
        //   belt:      +6 per step (4 + 2), nothing exits (transit 100)
        let want_wa = [0.0, 0.0, 0.0, 0.0, 0.0];
        let want_wb = [0.0, 2.0, 4.0, 6.0, 8.0];
        let want_belt = [0.0, 6.0, 12.0, 18.0, 24.0];
        let want_ia = [4.0, 4.0, 4.0, 4.0, 4.0];
        let want_ib = [2.0, 2.0, 2.0, 2.0, 2.0];
        for i in 0..5 {
            assert!(
                (wa[i] - want_wa[i]).abs() < 1e-9,
                "waiting_a[{i}] = {}",
                wa[i]
            );
            assert!(
                (wb[i] - want_wb[i]).abs() < 1e-9,
                "waiting_b[{i}] = {}",
                wb[i]
            );
            assert!(
                (belt[i] - want_belt[i]).abs() < 1e-9,
                "belt[{i}] = {}",
                belt[i]
            );
            assert!(
                (ia[i] - want_ia[i]).abs() < 1e-9,
                "into_belt_a[{i}] = {}",
                ia[i]
            );
            assert!(
                (ib[i] - want_ib[i]).abs() < 1e-9,
                "into_belt_b[{i}] = {}",
                ib[i]
            );
        }

        // Total conservation: 8 arrives per completed step (4 + 4). Nothing exits
        // the belt, so belt + waiting_a + waiting_b == cumulative arrivals.
        for t in 0..5 {
            let in_system = belt[t] + wa[t] + wb[t];
            let arrived = 8.0 * t as f64;
            assert!(
                (in_system - arrived).abs() < 1e-9,
                "step {t}: belt+wa+wb = {in_system}, arrived = {arrived}"
            );
        }
    }

    #[test]
    fn two_queue_priority_follows_conveyor_inflow_order_not_queue_order() {
        // Swapping the belt's <inflow> DECLARATION order flips the priority: now
        // `waiting_b` (declared first) drains fully and `waiting_a` (declared
        // second) is throttled to the residual 2 -- the exact mirror image of the
        // non-swapped run. The queue VARIABLE order in the model is unchanged
        // (`waiting_a` still precedes `waiting_b`), so this pins that admission
        // priority is the CONVEYOR's inflow order, not queue-plan/HashMap order.
        let vm = build_run(&two_queue_shared_belt_model(true, false));
        let wa = vm.get_series(&Ident::new("waiting_a")).unwrap();
        let wb = vm.get_series(&Ident::new("waiting_b")).unwrap();
        let ia = vm.get_series(&Ident::new("into_belt_a")).unwrap();
        let ib = vm.get_series(&Ident::new("into_belt_b")).unwrap();

        let want_wa = [0.0, 2.0, 4.0, 6.0, 8.0]; // now throttled
        let want_wb = [0.0, 0.0, 0.0, 0.0, 0.0]; // now priority
        let want_ia = [2.0, 2.0, 2.0, 2.0, 2.0];
        let want_ib = [4.0, 4.0, 4.0, 4.0, 4.0];
        for i in 0..5 {
            assert!(
                (wa[i] - want_wa[i]).abs() < 1e-9,
                "waiting_a[{i}] = {}",
                wa[i]
            );
            assert!(
                (wb[i] - want_wb[i]).abs() < 1e-9,
                "waiting_b[{i}] = {}",
                wb[i]
            );
            assert!(
                (ia[i] - want_ia[i]).abs() < 1e-9,
                "into_belt_a[{i}] = {}",
                ia[i]
            );
            assert!(
                (ib[i] - want_ib[i]).abs() < 1e-9,
                "into_belt_b[{i}] = {}",
                ib[i]
            );
        }
    }

    #[test]
    fn two_queue_overflow_drains_shared_budget_blocked_volume() {
        // `waiting_b` is the throttled (lower-priority) queue and has an
        // `<overflow/>` sibling. Its redirectable volume each step is what the
        // SHARED per-time-unit budget (already debited by `waiting_a`'s priority
        // take) blocked it from admitting: desire 4 (all-at-once) minus taken 2 =
        // 2. The overflow drains exactly that 2 to `balked_b`, so `waiting_b`
        // stays EMPTY and the excess accumulates in `balked_b`. This is the
        // redirectable accounting measured against the shared budget (§4.5).
        let vm = build_run(&two_queue_shared_belt_model(false, true));
        let wa = vm.get_series(&Ident::new("waiting_a")).unwrap();
        let wb = vm.get_series(&Ident::new("waiting_b")).unwrap();
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let balk = vm.get_series(&Ident::new("balk_b")).unwrap();
        let balked = vm.get_series(&Ident::new("balked_b")).unwrap();

        let want_wa = [0.0, 0.0, 0.0, 0.0, 0.0];
        let want_wb = [0.0, 0.0, 0.0, 0.0, 0.0]; // overflow keeps it empty
        let want_belt = [0.0, 6.0, 12.0, 18.0, 24.0];
        let want_balk = [2.0, 2.0, 2.0, 2.0, 2.0];
        let want_balked = [0.0, 2.0, 4.0, 6.0, 8.0];
        for i in 0..5 {
            assert!(
                (wa[i] - want_wa[i]).abs() < 1e-9,
                "waiting_a[{i}] = {}",
                wa[i]
            );
            assert!(
                (wb[i] - want_wb[i]).abs() < 1e-9,
                "waiting_b[{i}] = {}",
                wb[i]
            );
            assert!(
                (belt[i] - want_belt[i]).abs() < 1e-9,
                "belt[{i}] = {}",
                belt[i]
            );
            assert!(
                (balk[i] - want_balk[i]).abs() < 1e-9,
                "balk_b[{i}] = {}",
                balk[i]
            );
            assert!(
                (balked[i] - want_balked[i]).abs() < 1e-9,
                "balked_b[{i}] = {}",
                balked[i]
            );
        }

        // Conservation with the overflow sink: belt + waiting_a + waiting_b +
        // balked_b == cumulative arrivals (8 per completed step).
        for t in 0..5 {
            let in_system = belt[t] + wa[t] + wb[t] + balked[t];
            assert!(
                (in_system - 8.0 * t as f64).abs() < 1e-9,
                "step {t}: in_system = {in_system}"
            );
        }
    }

    /// Structural pin for the attach-time [`CouplingTable`] (GH #878): the table
    /// derived once from the resolved plans must encode exactly the couplings
    /// [`apply_couplings`] stamped onto them -- which queues are coupled, to
    /// which belt, and in the belt's `<inflow>` declaration order (the admission
    /// priority the behavioral two-queue tests above observe through the
    /// simulation). The swap variant flips ONLY the serve order, proving the
    /// order comes from the conveyor plan's inflow list, not queue-plan order.
    #[test]
    fn coupling_table_matches_plans_and_belt_inflow_order() {
        for swap in [false, true] {
            let project = two_queue_shared_belt_model(swap, false);
            let main = project.models[0].name.clone();
            let (compiled, conv_plans, queue_plans) =
                build_compiled_fresh(&project, &main).unwrap();
            let table = CouplingTable::build(&conv_plans, &queue_plans);

            assert!(table.any, "swap={swap}: couplings must be detected");
            assert_eq!(
                table.queue_is_coupled,
                vec![true; queue_plans.len()],
                "swap={swap}: both queues' primaries feed the belt"
            );
            assert_eq!(table.coupling_for_conveyor.len(), conv_plans.len());

            // Exactly one belt, with both queues coupled to it in the belt's
            // <inflow> declaration order: [into_belt_a, into_belt_b] normally,
            // reversed when the model swaps the declaration order.
            let off_a = *compiled.offsets.get(&Ident::new("into_belt_a")).unwrap();
            let off_b = *compiled.offsets.get(&Ident::new("into_belt_b")).unwrap();
            let serves = &table.coupling_for_conveyor[0];
            assert_eq!(serves.len(), 2, "swap={swap}");
            let want = if swap { [off_b, off_a] } else { [off_a, off_b] };
            let got: Vec<usize> = serves.iter().map(|cs| cs.shared_flow_off).collect();
            assert_eq!(got, want, "swap={swap}: serve order is belt inflow order");
            // Each serve names the queue plan owning its shared flow slot, and
            // carries the belt's batch rules (one_at_a_time=false in the fixture).
            for cs in serves {
                assert_eq!(
                    queue_plans[cs.queue].outflows[0].flow_off, cs.shared_flow_off,
                    "swap={swap}: serve must point at the owning queue's primary"
                );
                assert!(!cs.one_at_a_time, "swap={swap}");
                assert!(!cs.batch_integrity, "swap={swap}");
            }
        }
    }

    /// An UNCOUPLED queue model derives an empty (fast-path) table, and the
    /// derivation is deterministic: rebuilding from the same plans is identical,
    /// so an attach-time table can never drift from a per-step rebuild.
    #[test]
    fn coupling_table_uncoupled_model_is_empty_and_deterministic() {
        let project = parse(QUEUE_DRAIN);
        let main = project.models[0].name.clone();
        let (_compiled, conv_plans, queue_plans) = build_compiled_fresh(&project, &main).unwrap();
        let table = CouplingTable::build(&conv_plans, &queue_plans);
        assert!(!table.any);
        assert_eq!(table.queue_is_coupled, vec![false]);
        assert!(table.coupling_for_conveyor.is_empty());
        assert_eq!(table, CouplingTable::build(&conv_plans, &queue_plans));
    }

    // ----- Part C: multiple outflows, priority, and overflow (§4.5/§5) -----

    /// A queue feeding a capacity-limited discrete conveyor, with an `<overflow/>`
    /// second outflow draining the rejected volume to a stock. Same geometry as
    /// `QUEUE_TO_DISCRETE_CONVEYOR` (belt cap 10, transit 100, arrivals 4/time,
    /// one_at_a_time=false), so the belt fills identically -- but the overflow
    /// bleeds off the volume the belt cannot admit, so the queue does NOT grow
    /// unboundedly (it stays empty) and the excess accumulates in `balked`.
    const QUEUE_OVERFLOW_TO_STOCK: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>overflow</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>balk</outflow><queue/></stock>
    <flow name="arrivals"><eqn>4</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="balk"><eqn>0</eqn><overflow/></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="balked"><eqn>0</eqn><inflow>balk</inflow></stock>
  </variables></model>
</xmile>"#;

    #[test]
    fn overflow_drains_the_capacity_rejected_volume_and_conserves() {
        let project = parse(QUEUE_OVERFLOW_TO_STOCK);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let into_belt = vm.get_series(&Ident::new("into_belt")).unwrap();
        let balk = vm.get_series(&Ident::new("balk")).unwrap();
        let balked = vm.get_series(&Ident::new("balked")).unwrap();

        // Hand-computed oracle (start-of-step stocks; step-t flow rate). The belt
        // fills to capacity exactly as in the no-overflow twin; once req shrinks the
        // rejected volume (desire − taken) overflows to `balk` instead of waiting.
        //   step:      0    1    2    3    4    5
        //   belt(0):   0    4    8   10   10   10
        //   into_belt: 4    4    2    0    0    0
        //   balk:      0    0    2    4    4    4   (= arrivals − into_belt)
        //   waiting:   0    0    0    0    0    0   (never grows: overflow bleeds it)
        //   balked(0): 0    0    0    2    6   10
        let want_belt = [0.0, 4.0, 8.0, 10.0, 10.0, 10.0];
        let want_into = [4.0, 4.0, 2.0, 0.0, 0.0, 0.0];
        let want_balk = [0.0, 0.0, 2.0, 4.0, 4.0, 4.0];
        let want_balked = [0.0, 0.0, 0.0, 2.0, 6.0, 10.0];
        for i in 0..6 {
            assert!(
                (belt[i] - want_belt[i]).abs() < 1e-9,
                "belt[{i}]={}",
                belt[i]
            );
            assert!(
                (into_belt[i] - want_into[i]).abs() < 1e-9,
                "into_belt[{i}]={}",
                into_belt[i]
            );
            assert!(
                (balk[i] - want_balk[i]).abs() < 1e-9,
                "balk[{i}]={}",
                balk[i]
            );
            assert!(
                (balked[i] - want_balked[i]).abs() < 1e-9,
                "balked[{i}]={}",
                balked[i]
            );
            assert!(
                waiting[i].abs() < 1e-9,
                "waiting[{i}]={} (must not grow)",
                waiting[i]
            );
        }
        // Conservation across queue + belt + overflow sink: nothing exits the belt
        // (transit 100 >> sim), so belt + waiting + balked == cumulative arrivals.
        for t in 0..6 {
            let total = belt[t] + waiting[t] + balked[t];
            assert!(
                (total - 4.0 * t as f64).abs() < 1e-9,
                "step {t}: belt+waiting+balked = {total}, arrived = {}",
                4.0 * t as f64
            );
        }
    }

    #[test]
    fn overflow_drains_whole_desire_while_arrested_then_primary_resumes() {
        // While the belt is arrested (req=0) the whole desire overflows (§4.5); once
        // un-arrested the primary resumes admitting and the overflow goes idle.
        // arrest = 1 for TIME < 2, then 0; belt capacity is effectively unlimited.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>arrest</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>balk</outflow><queue/></stock>
    <flow name="arrivals"><eqn>5</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="balk"><eqn>0</eqn><overflow/></flow>
    <aux name="arrest_sig"><eqn>IF TIME &lt; 2 THEN 1 ELSE 0</eqn></aux>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><capacity>1000</capacity><arrest>arrest_sig</arrest></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="balked"><eqn>0</eqn><inflow>balk</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let into_belt = vm.get_series(&Ident::new("into_belt")).unwrap();
        let balk = vm.get_series(&Ident::new("balk")).unwrap();
        let balked = vm.get_series(&Ident::new("balked")).unwrap();
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();

        //   step:      0    1    2    3
        //   arrested:  y    y    n    n
        //   into_belt: 0    0    5    5   (0 while arrested; 5 when free)
        //   balk:      5    5    0    0   (whole desire redirects while arrested)
        //   belt(0):   0    0    0    5
        //   balked(0): 0    5   10   10
        let want_into = [0.0, 0.0, 5.0, 5.0];
        let want_balk = [5.0, 5.0, 0.0, 0.0];
        let want_belt = [0.0, 0.0, 0.0, 5.0];
        let want_balked = [0.0, 5.0, 10.0, 10.0];
        for i in 0..4 {
            assert!(
                (into_belt[i] - want_into[i]).abs() < 1e-9,
                "into_belt[{i}]={}",
                into_belt[i]
            );
            assert!(
                (balk[i] - want_balk[i]).abs() < 1e-9,
                "balk[{i}]={}",
                balk[i]
            );
            assert!(
                (belt[i] - want_belt[i]).abs() < 1e-9,
                "belt[{i}]={}",
                belt[i]
            );
            assert!(
                (balked[i] - want_balked[i]).abs() < 1e-9,
                "balked[{i}]={}",
                balked[i]
            );
            assert!(waiting[i].abs() < 1e-9, "waiting[{i}]={}", waiting[i]);
        }
        for t in 0..4 {
            assert!(
                (belt[t] + waiting[t] + balked[t] - 5.0 * t as f64).abs() < 1e-9,
                "step {t}: conservation"
            );
        }
    }

    /// A queue SEEDED with one batch of 10 that admits a second batch of 3 on step 0
    /// (so two batches coexist at the first serve), feeding a discrete conveyor with
    /// room for only 4, plus an `<overflow/>` to `balked`. Parameterised on
    /// `one_at_a_time`: it changes what the primary DESIRES and hence what the
    /// overflow redirects.
    fn overflow_two_batch_model(one_at_a_time: bool) -> datamodel::Project {
        let oa = if one_at_a_time { "true" } else { "false" };
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>ovf</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>10</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>balk</outflow><queue/></stock>
    <flow name="arrivals"><eqn>3</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="balk"><eqn>0</eqn><overflow/></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="{oa}" batch_integrity="false">
        <len>100</len><capacity>4</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="balked"><eqn>0</eqn><inflow>balk</inflow></stock>
  </variables></model>
</xmile>"#
        );
        parse(&xml)
    }

    #[test]
    fn overflow_one_at_a_time_redirects_only_the_front_batch() {
        // Step 0 front is [10, 3] (seed 10 + admit 3), belt room = 4.
        //   one_at_a_time=true:  desire = front batch 10, taken 4, redirectable 6;
        //     the overflow drains the 6 leftover of the FRONT batch, the 3-batch
        //     WAITS -> waiting[1] = 3, balk[0] = 6.
        //   one_at_a_time=false: desire = total 13, taken 4, redirectable 9; the
        //     overflow drains the whole remainder [6,3] -> waiting[1] = 0, balk[0]=9.
        let oaat = build_run(&overflow_two_batch_model(true));
        let all = build_run(&overflow_two_batch_model(false));
        let wait_oaat = oaat.get_series(&Ident::new("waiting")).unwrap();
        let wait_all = all.get_series(&Ident::new("waiting")).unwrap();
        let balk_oaat = oaat.get_series(&Ident::new("balk")).unwrap();
        let balk_all = all.get_series(&Ident::new("balk")).unwrap();
        let belt_oaat = oaat.get_series(&Ident::new("belt")).unwrap();

        assert!(
            (wait_oaat[1] - 3.0).abs() < 1e-9,
            "one_at_a_time waiting[1]={}",
            wait_oaat[1]
        );
        assert!(
            wait_all[1].abs() < 1e-9,
            "all-at-once waiting[1]={}",
            wait_all[1]
        );
        assert!(
            (balk_oaat[0] - 6.0).abs() < 1e-9,
            "one_at_a_time balk[0]={}",
            balk_oaat[0]
        );
        assert!(
            (balk_all[0] - 9.0).abs() < 1e-9,
            "all-at-once balk[0]={}",
            balk_all[0]
        );
        assert!(
            (belt_oaat[1] - 4.0).abs() < 1e-9,
            "belt[1]={}",
            belt_oaat[1]
        );
        // Conservation at step 1 for one_at_a_time: init 10 + arrived 3 == 13.
        let balked_oaat = oaat.get_series(&Ident::new("balked")).unwrap();
        assert!(
            (belt_oaat[1] + wait_oaat[1] + balked_oaat[1] - 13.0).abs() < 1e-9,
            "one_at_a_time conservation: {} + {} + {}",
            belt_oaat[1],
            wait_oaat[1],
            balked_oaat[1]
        );
    }

    #[test]
    fn ordinary_secondary_drains_whole_remainder_not_just_redirectable() {
        // Same two-batch setup as the overflow twin but the SECOND outflow is an
        // ORDINARY (non-overflow) competing outflow to a stock (§5.4). one_at_a_time
        // primary takes 4 of the front-10; the ordinary secondary then drains the
        // ENTIRE remaining front [6,3] = 9 (not just the redirectable 6), so
        // waiting[1] = 0 -- the crisp contrast with the overflow's waiting[1] = 3.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>ord</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>10</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>leftover</outflow><queue/></stock>
    <flow name="arrivals"><eqn>3</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="leftover"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="true" batch_integrity="false">
        <len>100</len><capacity>4</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="spilled"><eqn>0</eqn><inflow>leftover</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let leftover = vm.get_series(&Ident::new("leftover")).unwrap();
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let spilled = vm.get_series(&Ident::new("spilled")).unwrap();
        // Ordinary secondary drains the whole remainder: leftover[0] = 9, queue empty.
        assert!(
            (leftover[0] - 9.0).abs() < 1e-9,
            "leftover[0]={}",
            leftover[0]
        );
        assert!(
            waiting[1].abs() < 1e-9,
            "waiting[1]={} (ordinary drains all)",
            waiting[1]
        );
        assert!((belt[1] - 4.0).abs() < 1e-9, "belt[1]={}", belt[1]);
        // Conservation: init 10 + arrived 3 == belt 4 + waiting 0 + spilled 9.
        assert!(
            (belt[1] + waiting[1] + spilled[1] - 13.0).abs() < 1e-9,
            "conservation"
        );
    }

    #[test]
    fn two_ordinary_unconstrained_outflows_first_drains_all_second_gets_zero() {
        // §5.4 degenerate case on an UNCOUPLED queue: two ordinary outflows, both to
        // stocks. The first (unconstrained) empties the queue every DT, so the second
        // always removes nothing.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>two</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>1</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>drain1</outflow><outflow>drain2</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="drain1"><eqn>0</eqn></flow>
    <flow name="drain2"><eqn>0</eqn></flow>
    <stock name="sink1"><eqn>0</eqn><inflow>drain1</inflow></stock>
    <stock name="sink2"><eqn>0</eqn><inflow>drain2</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let drain1 = vm.get_series(&Ident::new("drain1")).unwrap();
        let drain2 = vm.get_series(&Ident::new("drain2")).unwrap();
        let arrivals = vm.get_series(&Ident::new("arrivals")).unwrap();
        for (i, ((&d1, &d2), &a)) in drain1
            .iter()
            .zip(drain2.iter())
            .zip(arrivals.iter())
            .enumerate()
        {
            assert!(
                (d1 - a).abs() < 1e-9,
                "drain1[{i}]={d1}, arrivals={a} (first drains all)"
            );
            assert!(d2.abs() < 1e-9, "drain2[{i}]={d2} (second gets nothing)");
        }
        // All throughput lands in sink1; sink2 stays empty.
        assert!(
            (*vm.get_series(&Ident::new("sink1")).unwrap().last().unwrap() - 10.0).abs() < 1e-6
        );
        for &v in vm.get_series(&Ident::new("sink2")).unwrap().iter() {
            assert!(v.abs() < 1e-9, "sink2 must stay empty: {v}");
        }
    }

    #[test]
    fn overflow_chain_first_drains_redirectable_second_gets_the_remainder() {
        // Two overflows to two stocks. The belt has capacity 0 (never admits, so
        // req=0), so with one_at_a_time=false the whole desire is redirectable each
        // DT. The FIRST overflow drains all of it; the SECOND sees the decremented
        // (zero) budget and drains what the first left -- 0 here (both unconstrained).
        // This pins the redirectable-budget threading through the overflow priority
        // order. A chain where the second drains a NONZERO remainder requires the
        // first overflow to be capacity-limited (a constrained/conveyor overflow,
        // now loudly rejected as QueueSecondaryOutflowToConveyor); the desire−taken
        // budget mechanism is unit-tested in
        // `queue::tests::redirectable_is_desire_minus_taken_across_the_batch_rules`.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>chain</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>2</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow>
      <outflow>into_belt</outflow><outflow>balk1</outflow><outflow>balk2</outflow><queue/></stock>
    <flow name="arrivals"><eqn>6</eqn><non_negative/></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <flow name="balk1"><eqn>0</eqn><overflow/></flow>
    <flow name="balk2"><eqn>0</eqn><overflow/></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><capacity>0</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <stock name="spill1"><eqn>0</eqn><inflow>balk1</inflow></stock>
    <stock name="spill2"><eqn>0</eqn><inflow>balk2</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let balk1 = vm.get_series(&Ident::new("balk1")).unwrap();
        let balk2 = vm.get_series(&Ident::new("balk2")).unwrap();
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        for (i, ((&b1, &b2), &w)) in balk1
            .iter()
            .zip(balk2.iter())
            .zip(waiting.iter())
            .enumerate()
        {
            assert!(
                (b1 - 6.0).abs() < 1e-9,
                "balk1[{i}]={b1} (drains all redirectable)"
            );
            assert!(
                b2.abs() < 1e-9,
                "balk2[{i}]={b2} (nothing left after balk1)"
            );
            assert!(
                w.abs() < 1e-9,
                "waiting[{i}]={w} (nothing waits: all redirected)"
            );
        }
        for &b in belt.iter() {
            assert!(b.abs() < 1e-9, "belt must stay empty (capacity 0): {b}");
        }
        // Conservation: all arrivals land in spill1.
        assert!(
            (*vm.get_series(&Ident::new("spill1"))
                .unwrap()
                .last()
                .unwrap()
                - 12.0)
                .abs()
                < 1e-6
        );
    }

    // ----- Part C: <overflow/> placement validation (§10.7) -----

    #[test]
    fn overflow_on_non_queue_flow_is_rejected() {
        // A model WITH a queue but an <overflow/> on a flow that is not any queue's
        // outflow -> loud QueueOverflowNotOnQueue (§3.3/§10.7).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_service</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
    <aux name="src"><eqn>1</eqn></aux>
    <flow name="stray"><eqn>src</eqn><overflow/></flow>
    <stock name="junk"><eqn>0</eqn><inflow>stray</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let (code, msg) =
            expand_queues(&project, &project.models[0].name).expect_err("stray overflow rejected");
        assert_eq!(code, ErrorCode::QueueOverflowNotOnQueue);
        assert!(
            msg.contains("stray"),
            "message names the offending flow: {msg}"
        );
    }

    #[test]
    fn overflow_on_first_outflow_is_rejected() {
        // <overflow/> on a queue's FIRST (highest-priority) outflow -> loud error:
        // an overflow may never be the first outflow (§3.3/§10.7).
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_service</outflow><queue/></stock>
    <flow name="arrivals"><eqn>10</eqn><non_negative/></flow>
    <flow name="into_service"><eqn>0</eqn><overflow/></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let (code, msg) = expand_queues(&project, &project.models[0].name)
            .expect_err("first-outflow overflow rejected");
        assert_eq!(code, ErrorCode::QueueOverflowNotOnQueue);
        assert!(
            msg.contains("into_service"),
            "message names the first outflow: {msg}"
        );
    }

    #[test]
    fn overflow_with_no_queue_at_all_is_rejected() {
        // Even a model with NO queue must reject a stray <overflow/> (validation runs
        // before the no-queue fast path), since it cannot be a queue outflow.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>2</stop><dt>0.5</dt></sim_specs>
  <model><variables>
    <stock name="src"><eqn>100</eqn><outflow>drain</outflow></stock>
    <flow name="drain"><eqn>1</eqn><overflow/></flow>
    <stock name="sink"><eqn>0</eqn><inflow>drain</inflow></stock>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let (code, _msg) = expand_queues(&project, &project.models[0].name)
            .expect_err("overflow without queue rejected");
        assert_eq!(code, ErrorCode::QueueOverflowNotOnQueue);
    }

    // ----- Part D: non-negative inflow clamp keeps the flat stock == Σ batches
    // (§3.4/§4.1/§4.2 step 1). A queue inflow is clamped at zero (a negative
    // inflow contributes no batch); the clamped rate MUST be written back into the
    // inflow's slot so the ordinary Stocks phase integrates the SAME volume the
    // FIFO admitted, exactly as `conveyor_phase_b_one` writes admitted equation
    // inflows back. Without the write-back the Stocks phase folds the raw negative
    // rate into the flat queue stock while the batch side table stays empty -- a
    // silent divergence between the stock's saved series and Σ batches. -----

    #[test]
    fn uncoupled_negative_inflow_freezes_stock_and_clamps_inflow_series() {
        // A pass-through queue whose single inflow goes negative mid-run: 10 until
        // t<2, then -10 (no <non_negative/>, so the raw rate reaches the queue). The
        // queue empties every DT, so after the clamp the flat stock must hold at 0
        // and equal Σ batches (published via SUM(waiting)); before the write-back
        // the Stocks phase drove `waiting` to -10, -20, ... while SUM(waiting)
        // stayed 0.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_service</outflow><queue/></stock>
    <flow name="arrivals"><eqn>IF TIME >= 2 THEN -10 ELSE 10</eqn></flow>
    <flow name="into_service"><eqn>0</eqn></flow>
    <stock name="served"><eqn>0</eqn><inflow>into_service</inflow></stock>
    <aux name="q_sum"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let q_sum = vm.get_series(&Ident::new("q_sum")).unwrap();
        let arrivals = vm.get_series(&Ident::new("arrivals")).unwrap();

        for (i, &w) in waiting.iter().enumerate() {
            assert!(w >= -1e-9, "waiting[{i}] = {w} drifted below zero");
            assert!(
                (w - q_sum[i]).abs() < 1e-9,
                "Σ batches == stock broken at {i}: waiting = {w}, SUM = {}",
                q_sum[i]
            );
        }
        // The negative inflow's slot is written back to 0 (§3.4: a negative inflow
        // contributes no batch), like the conveyor's admitted/clamped equation
        // inflow. dt=1/start=0 so index == TIME: steps 2..4 are the negative arm.
        assert!(
            (arrivals[0] - 10.0).abs() < 1e-9,
            "arrivals[0] = {}",
            arrivals[0]
        );
        for (i, &a) in arrivals.iter().enumerate().skip(2) {
            assert!(a.abs() < 1e-9, "arrivals[{i}] = {a} not clamped to 0");
        }
    }

    #[test]
    fn uncoupled_all_negative_inflow_freezes_seeded_queue() {
        // A queue seeded with a batch of 30 and NO outflow, fed a wholly-negative
        // inflow (-5): §3.4 admits nothing, so the seeded batch is frozen. The flat
        // stock must hold at 30 (== Σ batches) every step and the inflow slot must
        // read 0; before the fix the Stocks phase drained the stock by 5 per DT
        // (30, 25, 20, ...) while the single 30-batch sat untouched in the FIFO.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>3</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>30</eqn><inflow>bleed</inflow><queue/></stock>
    <flow name="bleed"><eqn>-5</eqn></flow>
    <aux name="q_sum"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let q_sum = vm.get_series(&Ident::new("q_sum")).unwrap();
        let bleed = vm.get_series(&Ident::new("bleed")).unwrap();

        for (i, &w) in waiting.iter().enumerate() {
            assert!(
                (w - 30.0).abs() < 1e-9,
                "waiting[{i}] = {w} not frozen at 30"
            );
            assert!(
                (w - q_sum[i]).abs() < 1e-9,
                "Σ batches == stock broken at {i}: waiting = {w}, SUM = {}",
                q_sum[i]
            );
        }
        for (i, &b) in bleed.iter().enumerate() {
            assert!(b.abs() < 1e-9, "bleed[{i}] = {b} not clamped to 0");
        }
    }

    #[test]
    fn mixed_sign_multi_inflow_admits_sum_of_per_flow_clamps() {
        // Two inflows into one accumulator queue (no outflow): in_pos=10, in_neg=-3.
        // §4.2 step 1 clamps EACH inflow at zero independently, THEN appends one
        // batch of the summed volume -- so 10 is admitted per DT (sum-of-clamps),
        // NOT max(10-3, 0)=7 (clamp-of-sum). The flat stock and Σ batches must both
        // equal 10·t, only the negative inflow's slot is zeroed, and the positive
        // inflow's series is untouched.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>q</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler"><start>0</start><stop>4</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>in_pos</inflow><inflow>in_neg</inflow><queue/></stock>
    <flow name="in_pos"><eqn>10</eqn></flow>
    <flow name="in_neg"><eqn>-3</eqn></flow>
    <aux name="q_sum"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let q_sum = vm.get_series(&Ident::new("q_sum")).unwrap();
        let in_pos = vm.get_series(&Ident::new("in_pos")).unwrap();
        let in_neg = vm.get_series(&Ident::new("in_neg")).unwrap();

        for (i, &w) in waiting.iter().enumerate() {
            let want = 10.0 * i as f64;
            assert!(
                (w - want).abs() < 1e-9,
                "waiting[{i}] = {w}, want {want} (sum-of-clamps admits 10, not 7)"
            );
            assert!(
                (w - q_sum[i]).abs() < 1e-9,
                "Σ batches == stock broken at {i}: waiting = {w}, SUM = {}",
                q_sum[i]
            );
        }
        for (i, &n) in in_neg.iter().enumerate() {
            assert!(n.abs() < 1e-9, "in_neg[{i}] = {n} not clamped to 0");
        }
        for (i, &p) in in_pos.iter().enumerate() {
            assert!((p - 10.0).abs() < 1e-9, "in_pos[{i}] = {p} was altered");
        }
    }

    #[test]
    fn coupled_queue_negative_inflow_conserves_and_clamps() {
        // A queue feeding a discrete conveyor (cap 10, transit 100 >> sim length)
        // whose inflow goes negative at t>=3 (8 until then, -8 after). The COMBINED
        // pass must clamp the negative inflow too: the queue stock stays == Σ batches
        // and belt+waiting holds at the cumulative admitted volume (8 per step for
        // the first three steps = 24). Before the fix the coupled admit site folded
        // the raw -8 into the flat queue stock while the FIFO ignored it, breaking
        // conservation and the Σ-batches invariant.
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>c</name><vendor>t</vendor><product version="1.0">t</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>5</stop><dt>1</dt></sim_specs>
  <model><variables>
    <stock name="waiting"><eqn>0</eqn><inflow>arrivals</inflow><outflow>into_belt</outflow><queue/></stock>
    <flow name="arrivals"><eqn>IF TIME >= 3 THEN -8 ELSE 8</eqn></flow>
    <flow name="into_belt"><eqn>0</eqn></flow>
    <stock name="belt"><eqn>0</eqn><inflow>into_belt</inflow><outflow>graduating</outflow>
      <conveyor discrete="true" one_at_a_time="false" batch_integrity="false">
        <len>100</len><capacity>10</capacity></conveyor></stock>
    <flow name="graduating"><eqn>0</eqn></flow>
    <stock name="alumni"><eqn>0</eqn><inflow>graduating</inflow></stock>
    <aux name="q_sum"><eqn>SUM(waiting)</eqn></aux>
  </variables></model>
</xmile>"#;
        let project = parse(xml);
        let vm = build_run(&project);
        let waiting = vm.get_series(&Ident::new("waiting")).unwrap();
        let belt = vm.get_series(&Ident::new("belt")).unwrap();
        let q_sum = vm.get_series(&Ident::new("q_sum")).unwrap();
        let arrivals = vm.get_series(&Ident::new("arrivals")).unwrap();

        // Cumulative admitted at start-of-step t: 8 per step, clamped to 0 once
        // arrivals go negative -> 8·min(t, 3). Nothing exits the belt (transit 100).
        for (i, &w) in waiting.iter().enumerate() {
            assert!(w >= -1e-9, "waiting[{i}] = {w} drifted negative");
            assert!(
                (w - q_sum[i]).abs() < 1e-9,
                "Σ batches == stock broken at {i}: waiting = {w}, SUM = {}",
                q_sum[i]
            );
            let in_system = belt[i] + w;
            let want = 8.0 * (i.min(3) as f64);
            assert!(
                (in_system - want).abs() < 1e-9,
                "step {i}: belt+waiting = {in_system}, want {want}"
            );
        }
        // The negative inflow's slot is written back to 0 in the coupled path too.
        for (i, &a) in arrivals.iter().enumerate().skip(3) {
            assert!(a.abs() < 1e-9, "arrivals[{i}] = {a} not clamped to 0");
        }
    }
    // ----- GH #885: duplicate canonical variable idents are rejected at the
    // special-stock build chokepoint, BEFORE any expansion -----

    /// The #870 fixture shape: a conveyor whose outflow list names a canonical
    /// ident carried by TWO flows (`Attrition`/`attrition`, only the later
    /// twin leak-marked). `expand_conveyors` itself stays robust against the
    /// pair (the #870 fix, pinned by its direct-call tests), but the BUILD
    /// path must never reach expansion: the duplicate is a model-integrity
    /// error, rejected loudly before expansion self-consistency matters.
    const DUPLICATE_FLOW_CONVEYOR: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>t</name><vendor>t</vendor><product version="1.0">t</product>
    <options><uses_conveyor/></options></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>20</stop><dt>0.25</dt></sim_specs>
  <model><variables>
    <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow><outflow>Attrition</outflow>
      <conveyor><len>4</len></conveyor></stock>
    <flow name="in_f"><eqn>250</eqn></flow>
    <flow name="out_f"><eqn>0</eqn></flow>
    <flow name="Attrition"><eqn>1</eqn></flow>
    <flow name="attrition"><eqn>0.2</eqn><leak/></flow>
  </variables></model>
</xmile>"#;

    #[test]
    fn duplicate_canonical_idents_rejected_by_build_compiled() {
        let project = parse(DUPLICATE_FLOW_CONVEYOR);
        let err = build_compiled_fresh(&project, "main")
            .expect_err("a duplicate canonical ident pair must be rejected before expansion");
        assert_eq!(err.code, crate::common::ErrorCode::DuplicateVariable);
        let details = err.details.expect("a diagnostic message");
        assert!(
            details.contains("'Attrition'") && details.contains("'attrition'"),
            "names both original spellings: {details}"
        );
        assert!(details.contains("main"), "names the model: {details}");
    }

    #[test]
    fn duplicate_canonical_idents_rejected_by_build_sim() {
        let project = parse(DUPLICATE_FLOW_CONVEYOR);
        let main = project.models[0].name.clone();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let err = build_sim(
            &mut db,
            sync.project,
            &project,
            &main,
            crate::db::LtmOverlay::Off,
        )
        .expect_err("build_sim must reject the duplicate pair");
        assert_eq!(err.code, crate::common::ErrorCode::DuplicateVariable);
    }
}
