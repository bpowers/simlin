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
//! Scope: this phase handles a SCALAR queue whose outflow(s) target a cloud or a
//! regular (non-conveyor) stock -- the UNCONSTRAINED case (§4.3), which empties
//! the queue every DT. Container access (§8), arrayed queues (§6), overflow with
//! a blocked primary (§4.5), and the queue-conveyor coupling (§9) are later
//! build-sequence steps (docs/design/queues.md §11); the structures here leave
//! room for them ([`QueueOutflowKind`] gains a conveyor-coupled variant, and the
//! per-outflow plan generalizes to N belts). An `<overflow/>` outflow to a
//! cloud/stock with NO upstream conveyor is served here as an ordinary
//! unconstrained sibling: with nothing to block the primary, the overflow drains
//! nothing (§4.5), which the priority-order serve produces naturally (the first
//! outflow empties the queue; the rest remove nothing, §4.3).
//!
//! Queues and conveyors COEXIST: a model may contain both (the eventual coupling
//! is the whole point), so the unified [`build_compiled`] / [`build_vm`] here
//! expand conveyors first, then queues, compile ONCE, and resolve BOTH plan sets
//! against the same offset map. The VM carries both side tables and runs both
//! passes between the Flows and Stocks phases.

use std::collections::HashMap;

use crate::common::{Canonical, ErrorCode, Ident, canonicalize};
use crate::datamodel::{self, Equation};

fn canon(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// The downstream target a queue outflow drains into (docs/design/queues.md §4).
/// This phase only produces [`QueueOutflowKind::Unconstrained`]; the
/// conveyor-coupled variant (§4.4/§9) slots in at the coupling build step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueOutflowKind {
    /// A cloud or a regular (non-conveyor) stock: the outflow always empties the
    /// queue (§4.3).
    Unconstrained,
}

/// A queue outflow's synthesized metadata: the driven flow's canonical name plus
/// its target kind (all [`QueueOutflowKind::Unconstrained`] this phase).
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
    /// step 1). Empty for a queue with no inflow (it only drains).
    pub inflows: Vec<String>,
    /// The driven outflows in `<outflow>` declaration = priority order (§5.1).
    pub outflows: Vec<QueueOutflowMeta>,
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
/// Errors (this phase's scope guards):
/// - an arrayed queue is not yet supported (§6 is a later step), so it is
///   rejected loudly rather than silently mis-simulated by the scalar pass.
/// - an equation that READS a queue driven outflow by name is rejected with
///   [`ErrorCode::QueueDrivenFlowRead`] (see the scan below and §2).
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
    let mut metas: Vec<QueueMeta> = Vec::new();
    let model = &project.models[model_idx];
    for v in &model.variables {
        let datamodel::Variable::Stock(stock) = v else {
            continue;
        };
        if stock.compat.queue.is_none() {
            continue;
        }
        // Arrayed queues are a later build-sequence step (§6). Reject loudly:
        // the scalar admit-then-serve pass would mis-handle per-element belts,
        // and `resolve_plans`' bare-name offset lookup would not find the
        // `name[elem]` slots anyway.
        if !equation_dims(&stock.equation).is_empty() {
            return Err((
                ErrorCode::QueueNotExpanded,
                format!(
                    "queue '{}' is arrayed; arrayed queues are not yet supported \
                     (docs/design/queues.md §6)",
                    stock.ident
                ),
            ));
        }
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
            stock: canon(&stock.ident),
            inflows: stock.inflows.iter().map(|f| canon(f)).collect(),
            outflows,
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

    // Pass 2 (mutable): give every driven outflow a `0` placeholder equation so
    // it compiles to a writable slot, and clear the queue/overflow markers so the
    // expanded model compiles as a plain stock-and-flow model. Clearing the
    // markers is what lets the ordinary compile path REJECT an un-expanded queue
    // (the marker is still set) while accepting this expanded one -- exactly the
    // `QueueNotExpanded` guard contract (§10.3).
    let model = &mut project.models[model_idx];
    for v in &mut model.variables {
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

    Ok((project, metas))
}

/// Resolve [`QueueMeta`] names to data-buffer offsets using the compiled
/// simulation's offset map (docs/design/queues.md §10.3). Returns `None` if any
/// required name is missing -- an internal inconsistency between expansion and
/// compilation that [`build_compiled`] surfaces as a hard `NotSimulatable` error
/// (there is no non-queue fallback: the model has queues). Mirrors
/// [`crate::conveyor_compile::resolve_plans`].
pub fn resolve_plans(
    metas: &[QueueMeta],
    offsets: &HashMap<Ident<Canonical>, usize>,
) -> Option<Vec<QueuePlan>> {
    let off =
        |name: &str| -> Option<usize> { offsets.get(&Ident::<Canonical>::new(name)).copied() };
    let mut plans = Vec::with_capacity(metas.len());
    for meta in metas {
        let inflow_offs = meta
            .inflows
            .iter()
            .map(|f| off(f))
            .collect::<Option<Vec<_>>>()?;
        let outflows = meta
            .outflows
            .iter()
            .map(|o| {
                Some(QueueOutflowPlan {
                    flow_off: off(&o.flow)?,
                    kind: o.kind.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        plans.push(QueuePlan {
            stock_off: off(&meta.stock)?,
            inflow_offs,
            outflows,
        });
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
            };
            curr[outflow.flow_off] = removed / dt;
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

    // Expand conveyors first, then queues on the result. Order is independent
    // this phase (queues are always unconstrained, so they never read a conveyor
    // marker); the coupling step will revisit this ordering.
    let (expanded, conv_metas) = crate::conveyor_compile::expand_conveyors(project, main_model)
        .map_err(|(code, msg)| Error::new(ErrorKind::Simulation, code, Some(msg)))?;
    let (expanded, queue_metas) = expand_queues(&expanded, main_model)
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

    let conveyor_plans = if conv_metas.is_empty() {
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
    let queue_plans = if queue_metas.is_empty() {
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

    Ok((compiled, conveyor_plans, queue_plans))
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
}
