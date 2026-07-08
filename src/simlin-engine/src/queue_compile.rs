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
//! arrayed queues (§6) are supported; overflow with a blocked primary (§4.5) is
//! the remaining build-sequence step. An `<overflow/>` outflow to a cloud/stock
//! with NO upstream conveyor is served here as an ordinary unconstrained sibling:
//! with nothing to block the primary, the overflow drains nothing (§4.5), which
//! the priority-order serve produces naturally (the first outflow empties the
//! queue; the rest remove nothing, §4.3).
//!
//! Queues and conveyors COEXIST and COUPLE: the unified [`build_compiled`] /
//! [`build_vm`] here expand conveyors first, then queues, compile ONCE, resolve
//! BOTH plan sets against the same offset map, and then [`apply_couplings`]
//! detects each queue outflow that feeds a discrete conveyor (enforcing the
//! `ConveyorQueueUpstreamNotDiscrete` requirement) and wires the coupling INTO the
//! two plan sets (a `queue_coupled` conveyor inflow + a [`QueueOutflowKind::Coupled`]
//! queue outflow), so no separate structure threads through the VM or libsimlin.
//! The VM carries both side tables and runs [`run_coupled_passes`] between the
//! Flows and Stocks phases -- interleaving a coupled queue's serve between its
//! conveyor's phase A and phase B, and delegating to the two independent passes
//! when nothing is coupled.

use std::collections::HashMap;

use crate::common::{Canonical, DimensionName, ErrorCode, Ident, canonicalize};
use crate::conveyor_compile::{
    ContainerMeta, ContainerNaming, ContainerPlan, ContainerVarSpec, container_value_from_slice,
    element_subscripts_for_dims, make_container_stock, rewrite_container_equation,
};
use crate::datamodel::{self, Equation};

fn canon(name: &str) -> String {
    canonicalize(name).into_owned()
}

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

/// Does the named model in `project` contain any queue stock? A cheap predicate a
/// caller uses to decide whether to route through [`build_vm`] (the special
/// stock-type build path) instead of the ordinary incremental compile. Mirrors
/// [`crate::conveyor_compile::project_has_conveyor`].
pub fn project_has_queue(project: &datamodel::Project, main_model: &str) -> bool {
    let main_canon = canon(main_model);
    project.models.iter().any(|m| {
        canon(&m.name) == main_canon
            && m.variables
                .iter()
                .any(|v| matches!(v, datamodel::Variable::Stock(s) if s.compat.queue.is_some()))
    })
}

/// The placeholder-`0` equation for a queue-driven outflow, preserving the flow's
/// array shape (so a future arrayed queue's driven flow keeps its per-element
/// slots). The pass overwrites the slot(s) each step, so the placeholder value
/// never matters -- only the slot count does. Mirrors
/// [`crate::conveyor_compile`]'s placeholder helper.
fn placeholder_zero_equation(existing: &Equation) -> Equation {
    match existing {
        Equation::Scalar(_) => Equation::Scalar("0".to_string()),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
            Equation::ApplyToAll(dims.clone(), "0".to_string())
        }
    }
}

/// The dimension names a variable's equation is declared over (empty = scalar).
fn equation_dims(equation: &Equation) -> Vec<crate::common::DimensionName> {
    match equation {
        Equation::Scalar(_) => Vec::new(),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => dims.clone(),
    }
}

/// The scalar equation strings of a variable (one for a `Scalar`/`ApplyToAll`,
/// each element plus the default for an `Arrayed`), used to scan for references
/// to queue-driven outflows. A `Module` carries no equation. Mirrors the private
/// helper of the same name in [`crate::conveyor_compile`].
fn equation_scalar_strings(v: &datamodel::Variable) -> Vec<String> {
    let eqn = match v {
        datamodel::Variable::Stock(s) => &s.equation,
        datamodel::Variable::Flow(f) => &f.equation,
        datamodel::Variable::Aux(a) => &a.equation,
        datamodel::Variable::Module(_) => return Vec::new(),
    };
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
    // Fast path: no queue anywhere in the main model. Return the project
    // unchanged so a conveyor-only or plain model compiles byte-identically.
    let has_queue = project.models.iter().any(|m| {
        canon(&m.name) == main_canon
            && m.variables
                .iter()
                .any(|v| matches!(v, datamodel::Variable::Stock(s) if s.compat.queue.is_some()))
    });
    if !has_queue {
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
                // Every outflow is unconstrained this phase (§4.3). The coupling
                // step (§9) resolves a conveyor target to a constrained kind.
                kind: QueueOutflowKind::Unconstrained,
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
    // "Driven outflow"), mirroring the conveyor `ConveyorDrivenFlowRead` scan.
    //
    // Boundaries (identical to the conveyor scan):
    // - a driven outflow's OWN placeholder equation is not a reader (skipped);
    // - the structural `<inflow>`/`<outflow>` stock linkage is NOT an equation
    //   reference, so it is not scanned here -- a stock fed by the driven outflow
    //   via INTEG is CORRECT (the Stocks phase runs after the pass) and is not
    //   rejected.
    {
        let model = &project.models[model_idx];
        for v in &model.variables {
            let self_name = canon(v.get_ident());
            if driven.contains(&self_name) {
                continue; // a driven flow's own placeholder equation is not a reader
            }
            for eqn in equation_scalar_strings(v) {
                let Ok(Some(ast)) = crate::ast::Expr0::new(&eqn, crate::lexer::LexerType::Equation)
                else {
                    continue;
                };
                for driven_flow in &driven {
                    if ast.get_var_loc(driven_flow).is_some() {
                        return Err((
                            ErrorCode::QueueDrivenFlowRead,
                            format!(
                                "variable '{}' references queue-driven flow '{driven_flow}'; a \
                                 queue outflow cannot be read by another equation (it is computed \
                                 after the flows phase)",
                                v.get_ident()
                            ),
                        ));
                    }
                }
            }
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
    let mut rewritten_equations: HashMap<String, Equation> = HashMap::new();
    {
        let model = &project.models[model_idx];
        for v in &model.variables {
            if driven.contains(&canon(v.get_ident())) {
                continue;
            }
            let eqn = match v {
                datamodel::Variable::Stock(s) => &s.equation,
                datamodel::Variable::Flow(f) => &f.equation,
                datamodel::Variable::Aux(a) => &a.equation,
                datamodel::Variable::Module(_) => continue,
            };
            if let Some(new_eqn) = rewrite_container_equation(
                eqn,
                &queue_dims,
                &ContainerNaming::QUEUE,
                &mut container_specs,
            )? {
                rewritten_equations.insert(canon(v.get_ident()), new_eqn);
            }
        }
    }

    // Attach each container variable to its queue's meta and synthesize the hidden
    // container stock (arrayed over the queue's dims when arrayed).
    let mut container_stocks: Vec<datamodel::Stock> = Vec::new();
    for (name, spec) in &container_specs {
        let dims = queue_stock_dims
            .get(&spec.owner_stock)
            .cloned()
            .unwrap_or_default();
        container_stocks.push(make_container_stock(name, &dims));
        if let Some(meta) = metas.iter_mut().find(|m| m.stock == spec.owner_stock) {
            meta.containers.push(ContainerMeta {
                name: name.clone(),
                kind: spec.kind.clone(),
            });
        }
    }

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
            match v {
                datamodel::Variable::Stock(s) => s.equation = new_eqn,
                datamodel::Variable::Flow(f) => f.equation = new_eqn,
                datamodel::Variable::Aux(a) => a.equation = new_eqn,
                datamodel::Variable::Module(_) => {}
            }
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

/// The number of independent FIFOs a queue meta expands to: `N_elem` for an
/// arrayed queue (§6), 1 for a scalar one (the degenerate case, whose
/// `element_subscripts` is empty). Mirrors `conveyor_compile::n_belts`.
fn n_queues(meta: &QueueMeta) -> usize {
    meta.element_subscripts.len().max(1)
}

/// Resolve [`QueueMeta`] names to data-buffer offsets using the compiled
/// simulation's offset map (docs/design/queues.md §10.3), flattening each arrayed
/// queue into ONE [`QueuePlan`] per array element (§6). An arrayed variable's
/// elements occupy contiguous slots keyed `name[elem1,elem2]` in the offset map
/// (`calc_flattened_offsets_incremental`), so element `e` resolves via the
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
    let off =
        |name: &str| -> Option<usize> { offsets.get(&Ident::<Canonical>::new(name)).copied() };
    let total: usize = metas.iter().map(n_queues).sum();
    let mut plans = Vec::with_capacity(total);
    for meta in metas {
        for e in 0..n_queues(meta) {
            // Element-aware offset resolver: the bare name for a scalar queue, the
            // `name[elem]` subscripted key for element `e` of an arrayed one.
            let eoff = |name: &str| -> Option<usize> {
                if meta.element_subscripts.is_empty() {
                    off(name)
                } else {
                    off(&format!("{}[{}]", name, meta.element_subscripts[e]))
                }
            };
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
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            // Container variables read this FIFO (§8). The container stock is
            // arrayed over the queue's dims, so element `e` of the container
            // resolves to FIFO `e` via the same element-aware offset lookup.
            let containers = meta
                .containers
                .iter()
                .map(|c| {
                    Some(ContainerPlan {
                        off: eoff(&c.name)?,
                        kind: c.kind.clone(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
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

/// The queue pass (§4.2), run once per Euler step between the Flows and Stocks
/// phases. For each queue: admit `Σ inflow_rate · dt` (the inflow rates were
/// computed in the Flows phase), then serve each outflow in priority order and
/// write its driven rate = `removed / dt` back into `curr`, so ordinary stock
/// integration then advances the queue stock (and any downstream stock) using
/// the pass-computed rates.
///
/// Serve order is admit-then-serve (§4.2): the just-admitted batch can leave in
/// the same DT when the downstream is unconstrained (§4.3, the pass-through). An
/// unconstrained outflow empties the queue, so the FIRST outflow drains
/// everything and every lower-priority outflow removes nothing (§4.3 degenerate
/// case) -- which also makes an `<overflow/>` sibling drain nothing when the
/// primary was not blocked (§4.5), the correct no-upstream-conveyor behavior.
pub fn run_queue_pass(
    plans: &[QueuePlan],
    states: &mut [crate::queue::QueueState],
    curr: &mut [f64],
    dt: f64,
) {
    for (plan, state) in plans.iter().zip(states.iter_mut()) {
        // A coupled queue (its outflow feeds a discrete conveyor) is served
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
        // Step 1: admit Σ inflow rates as one batch (each clamped >= 0 inside
        // `admit`; multiple inflows sum, §4.2 step 1).
        let inflow_rate: f64 = plan.inflow_offs.iter().map(|&o| curr[o]).sum();
        state.admit(inflow_rate, dt);
        // Step 2: serve outflows in priority order. `serve_unconstrained` drains
        // the whole queue, so the first outflow takes everything and the rest
        // take nothing (§4.3 / §4.5).
        for outflow in &plan.outflows {
            let removed = match outflow.kind {
                QueueOutflowKind::Unconstrained => state.serve_unconstrained(),
                // Unreachable: a plan with any Coupled outflow was skipped above.
                QueueOutflowKind::Coupled { .. } => continue,
            };
            curr[outflow.flow_off] = removed / dt;
        }
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

/// Compile `project` and resolve BOTH its conveyor and queue plans, returning the
/// compiled simulation plus the two plan sets (either empty when the model has
/// none of that kind). This is the unified special-stock-type build path: a model
/// may contain conveyors, queues, or both, so it expands conveyors FIRST (via
/// [`crate::conveyor_compile::expand_conveyors`]) then queues (on the
/// conveyor-expanded project), compiles ONCE, and resolves both plan sets against
/// the same offset map. It is the reusable core of [`build_vm`]; a caller that
/// rebuilds the VM later (libsimlin's reset) keeps all three pieces so it can
/// re-attach the plans.
///
/// Enforces the Euler-only rule for both stock types (§10.3): a conveyor present
/// under non-Euler yields [`ErrorCode::ConveyorNonEulerMethod`] (behavior-
/// identical to the pure conveyor path); a queue present under non-Euler yields
/// [`ErrorCode::QueueNonEulerMethod`].
///
/// For a project with NO conveyors and NO queues this is exactly the ordinary
/// compile path (both expansions are no-ops), so callers can route every
/// simulation through it.
pub fn build_compiled(
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<(
    crate::vm::CompiledSimulation,
    Vec<crate::conveyor_compile::ConveyorPlan>,
    Vec<QueuePlan>,
)> {
    use crate::common::{Error, ErrorKind};

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
    // conveyor path stays behavior-identical), otherwise the queue code.
    if expanded.sim_specs.sim_method != datamodel::SimMethod::Euler {
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

    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &expanded, None);
    let compiled = crate::db::compile_project_incremental(&db, sync.project, main_model)?;

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
fn detect_coupling_specs(
    project: &datamodel::Project,
    main_model: &str,
    conv_metas: &[crate::conveyor_compile::ConveyorMeta],
    queue_metas: &[QueueMeta],
) -> Result<Vec<CouplingSpec>, (ErrorCode, String)> {
    let mut specs = Vec::new();
    for qm in queue_metas {
        for out in &qm.outflows {
            // A coupled shared flow is the conveyor's SINGLE equation-driven
            // inflow: a conveyor inflow that is not itself conveyor-driven.
            let Some(cm) = conv_metas.iter().find(|cm| {
                cm.inflows
                    .iter()
                    .any(|inf| inf.flow == out.flow && !inf.conveyor_driven)
            }) else {
                continue; // outflow to a cloud/regular stock: unconstrained
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
        let n = n_queues(qm);
        for e in 0..n {
            let key = if qm.element_subscripts.is_empty() {
                spec.shared_flow.clone()
            } else {
                format!("{}[{}]", spec.shared_flow, qm.element_subscripts[e])
            };
            let off = offsets.get(&Ident::<Canonical>::new(&key)).copied()?;
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

/// The combined queue-conveyor pass (queues.md §9), run once per Euler step
/// between the Flows and Stocks phases in place of the separate conveyor
/// ([`crate::conveyor_compile::run_pass`]) and queue ([`run_queue_pass`]) passes
/// whenever the model has any coupling. Ordering (the whole point of the combined
/// pass): every conveyor's Phase A runs first (leak + exit, freeing belt room),
/// then for each conveyor -- if a queue is coupled to it -- the queue's serve is
/// interleaved BEFORE that conveyor's Phase B:
///
/// 1. size the conveyor's admission budget `req = min(cap_room, limit_vol)` from
///    ITS Phase A result ([`crate::conveyor_compile::coupled_admission_budget`]);
/// 2. admit the queue's inflow (§4.2 step 1);
/// 3. serve `taken <= req` from the front under the batch rules
///    ([`crate::queue::QueueState::take_for_conveyor`]);
/// 4. debit the discrete inflow budget by `taken`
///    ([`crate::conveyor::ConveyorState::consume_inflow_budget`]);
/// 5. write the shared flow slot to `taken / dt` -- this is BOTH the queue's
///    driven outflow rate AND the conveyor's admitted inflow rate, so the ordinary
///    Stocks phase integrates the queue stock `-taken` and the conveyor stock
///    `+taken` from the SAME slot (conservation on both stocks);
/// 6. run the conveyor's Phase B, which routes the shared flow through the
///    unconditional `conv_inflows` path (it is `queue_coupled`) and inserts
///    `taken` onto the belt at the entry depth.
///
/// Uncoupled conveyors run their ordinary Phase B and uncoupled queues their
/// ordinary admit-then-serve. When there is NO coupling this delegates to the two
/// independent passes, byte-identical to the pre-coupling behavior.
// The two side-table sets (plans + states), `curr`, `dt`, and the two clock
// inputs (`time`, `last_unit`) are all independent per-step inputs the VM already
// holds separately; bundling them into a struct would only add an indirection.
#[allow(clippy::too_many_arguments)]
pub fn run_coupled_passes(
    conv_plans: &[crate::conveyor_compile::ConveyorPlan],
    conveyors: &mut [crate::conveyor::ConveyorState],
    queue_plans: &[QueuePlan],
    queues: &mut [crate::queue::QueueState],
    curr: &mut [f64],
    dt: f64,
    time: f64,
    last_unit: &mut i64,
) {
    use crate::conveyor_compile as cc;

    /// A coupled queue serve wired to one conveyor (derived from the plans).
    #[derive(Clone, Copy)]
    struct CoupledServe {
        queue: usize,
        shared_flow_off: usize,
        one_at_a_time: bool,
        batch_integrity: bool,
    }

    // Reconstruct the couplings from the queue plans (the `Coupled` outflow kind
    // carries the conveyor plan index + batch rules): conveyor index -> its serve,
    // plus a mask of which queues are coupled (served here, skipped by the plain
    // queue pass).
    let mut coupling_for_conveyor: Vec<Option<CoupledServe>> = vec![None; conv_plans.len()];
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
                coupling_for_conveyor[conveyor] = Some(CoupledServe {
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

    // Fast path: no coupling -> the two independent passes, byte-identical.
    if !any {
        cc::run_pass(conv_plans, conveyors, curr, dt, time, last_unit);
        run_queue_pass(queue_plans, queues, curr, dt);
        return;
    }

    // Phase A over every conveyor (frees belt room, writes driven outflow rates).
    let pa = cc::run_phase_a(conv_plans, conveyors, curr, dt, time, last_unit);

    // Per conveyor: interleave the coupled queue's serve between phase A and B.
    for i in 0..conv_plans.len() {
        if let Some(cs) = coupling_for_conveyor[i] {
            // Size the budget from THIS conveyor's phase A (belt room), admit the
            // queue's inflow, then serve up to `req` under the batch rules.
            let req = cc::coupled_admission_budget(&conv_plans[i], &conveyors[i], &pa[i], curr, dt);
            let inflow_rate: f64 = queue_plans[cs.queue]
                .inflow_offs
                .iter()
                .map(|&o| curr[o])
                .sum();
            queues[cs.queue].admit(inflow_rate, dt);
            let taken =
                queues[cs.queue].take_for_conveyor(req, cs.one_at_a_time, cs.batch_integrity);
            // Debit the discrete per-time-unit budget (the coupled volume bypasses
            // phase_b's equation-inflow accounting), then publish the shared rate:
            // BOTH the queue outflow and the conveyor inflow integrate from it.
            conveyors[i].consume_inflow_budget(taken);
            curr[cs.shared_flow_off] = taken / dt;
        }
        cc::conveyor_phase_b_one(i, conv_plans, conveyors, &pa, curr, dt);
    }

    // Serve the uncoupled queues (coupled ones were fully served above).
    for (qi, (plan, state)) in queue_plans.iter().zip(queues.iter_mut()).enumerate() {
        if queue_is_coupled[qi] {
            continue;
        }
        let inflow_rate: f64 = plan.inflow_offs.iter().map(|&o| curr[o]).sum();
        state.admit(inflow_rate, dt);
        for outflow in &plan.outflows {
            let removed = match outflow.kind {
                QueueOutflowKind::Unconstrained => state.serve_unconstrained(),
                // A queue with a Coupled outflow is masked out above.
                QueueOutflowKind::Coupled { .. } => continue,
            };
            curr[outflow.flow_off] = removed / dt;
        }
    }
}

/// Build a runnable [`Vm`](crate::vm::Vm) for `project`, wiring up conveyor AND
/// queue support when the main model contains either. For a project with neither
/// this is exactly the ordinary compile-and-build path (both expansions are
/// no-ops), so callers can route every simulation through it.
pub fn build_vm(
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<crate::vm::Vm> {
    let (compiled, conveyor_plans, queue_plans) = build_compiled(project, main_model)?;
    let mut vm = crate::vm::Vm::new(compiled)?;
    vm.set_conveyor_plans(conveyor_plans);
    vm.set_queue_plans(queue_plans);
    Ok(vm)
}

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
    fn unexpanded_queue_rejected_by_ordinary_compile() {
        // A `<queue/>` marker reaching the ordinary incremental compile path
        // un-expanded must be rejected loudly (mirrors the conveyor guard), so no
        // ordinary/wasmgen path silently integrates the FIFO as a plain stock.
        let project = parse(QUEUE_DRAIN);
        let main = project.models[0].name.clone();
        let mut db = crate::db::SimlinDb::default();
        let sync = crate::db::sync_from_datamodel_incremental(&mut db, &project, None);
        let err = crate::db::compile_project_incremental(&db, sync.project, &main)
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
}
