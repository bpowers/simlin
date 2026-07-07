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
//! Because the conveyor stock's inflows/outflows carry the pass-computed rates
//! before stock integration, the stock integrates to `Σ belt contents` through
//! the ordinary Stocks phase -- no special-casing of stock integration is
//! needed (the §4.3 conservation identity guarantees `Δstock = admitted - out -
//! leak`).
//!
//! Scope: this handles conveyors in the **main model** under Euler, including
//! the `beginning`/`even`/`dest` spread-input placements (§8). Conveyors inside
//! submodules, arrayed conveyors, the `dist`/`source` placements (which need the
//! distribution graphical function / upstream-leak coupling), and queue coupling
//! are later build-sequence steps.

use std::collections::HashMap;

use crate::common::{Canonical, ErrorCode, Ident, canonicalize};
use crate::conveyor::{ConveyorState, LeakConfig, PhaseAInputs, PhaseBInputs, Placement};
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
}

/// A conveyor inflow's metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct InflowMeta {
    /// Canonical name of the inflow.
    pub flow: String,
    /// True iff this inflow is a driven outflow of some conveyor (so it is
    /// admitted unconditionally and bypasses capacity/inflow-limit, §4.3).
    pub conveyor_driven: bool,
    /// Inflow placement on the belt (§8), from the flow's `isee:spreadflow`.
    pub placement: Placement,
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
    /// Canonical name of the downstream conveyor whose belt this conveyor's
    /// primary outflow feeds (for the held-exit rule, §4.3 step 3). `None` if
    /// the primary outflow feeds an ordinary stock/cloud.
    pub primary_dest_conveyor: Option<String>,
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
}

/// A fully-resolved conveyor: data-buffer slot offsets for the VM's pass.
#[derive(Clone, Debug, PartialEq)]
pub struct ConveyorPlan {
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
    /// Index into the plan list of the downstream conveyor this conveyor's
    /// primary outflow feeds (for the held-exit rule); `None` otherwise.
    pub primary_dest_conveyor: Option<usize>,
}

fn canon(name: &str) -> String {
    canonicalize(name).into_owned()
}

/// Does the named model in `project` contain any conveyor stock? A cheap
/// predicate a caller uses to decide whether to route through [`build_vm`]
/// (the conveyor path) instead of the ordinary incremental compile.
pub fn project_has_conveyor(project: &datamodel::Project, main_model: &str) -> bool {
    let main_canon = canon(main_model);
    project.models.iter().any(|m| {
        canon(&m.name) == main_canon
            && m.variables
                .iter()
                .any(|v| matches!(v, datamodel::Variable::Stock(s) if s.compat.conveyor.is_some()))
    })
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

/// Map an inflow flow's `isee:spreadflow` (§8) to a runtime [`Placement`]. A
/// flow with no placement (or not found -- e.g. a cloud source) defaults to
/// `Beginning`. `Dist`/`Source` are recognized but their runtime wiring is a
/// later build-sequence step, so they are rejected loudly here rather than
/// silently placed at the entry.
fn inflow_placement(
    model: &datamodel::Model,
    flow: &str,
) -> Result<Placement, (ErrorCode, String)> {
    let spread = model.variables.iter().find_map(|v| match v {
        datamodel::Variable::Flow(f) if canon(&f.ident) == flow => {
            Some(f.compat.spreadflow.clone())
        }
        _ => None,
    });
    match spread.flatten() {
        None | Some(datamodel::SpreadFlow::Beginning) => Ok(Placement::Beginning),
        Some(datamodel::SpreadFlow::Even) => Ok(Placement::Even),
        Some(datamodel::SpreadFlow::Dest) => Ok(Placement::Dest),
        Some(datamodel::SpreadFlow::Dist(_)) => Err((
            ErrorCode::ConveyorSpreadflowUnsupported,
            format!("inflow '{flow}' uses isee:spreadflow 'dist', which is not yet supported"),
        )),
        Some(datamodel::SpreadFlow::Source) => Err((
            ErrorCode::ConveyorSpreadflowUnsupported,
            format!("inflow '{flow}' uses isee:spreadflow 'source', which is not yet supported"),
        )),
    }
}

/// Is the flow named `flow` (canonical) a conveyor leak outflow in `model`?
fn flow_is_leak(model: &datamodel::Model, flow: &str) -> bool {
    model.variables.iter().any(|v| match v {
        datamodel::Variable::Flow(f) => canon(&f.ident) == flow && f.compat.leakage.is_some(),
        _ => false,
    })
}

/// The leak fraction expression for a leak flow: the value-bearing `<leak>` if
/// present, else the flow's own equation (the bare-`<leak/>`-plus-`<eqn>` form,
/// §3.3). Empty ⇒ "0" (a bare marker with no fraction leaks nothing).
fn leak_fraction_expr(flow: &datamodel::Flow) -> String {
    if let Some(leak) = &flow.compat.leakage
        && let Some(frac) = &leak.fraction
        && !frac.is_empty()
    {
        return frac.clone();
    }
    match &flow.equation {
        Equation::Scalar(s) if !s.is_empty() => s.clone(),
        _ => "0".to_string(),
    }
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
    let has_conveyor = project.models.iter().any(|m| {
        canon(&m.name) == main_canon
            && m.variables
                .iter()
                .any(|v| matches!(v, datamodel::Variable::Stock(s) if s.compat.conveyor.is_some()))
    });
    if !has_conveyor {
        return Ok((project.clone(), Vec::new()));
    }

    let mut project = project.clone();
    let mut metas = Vec::new();
    // Collect new auxes to append after we finish borrowing the model's vars.
    let mut new_auxes: Vec<datamodel::Aux> = Vec::new();

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

    let mut driven_flows: Vec<String> = Vec::new();
    for v in &model.variables {
        let datamodel::Variable::Stock(stock) = v else {
            continue;
        };
        let Some(conv) = &stock.compat.conveyor else {
            continue;
        };
        let stock_name = canon(&stock.ident);

        // Partition outflows into the primary (first non-leak) and leaks.
        let mut primary: Option<String> = None;
        let mut leak_metas = Vec::new();
        for out in &stock.outflows {
            let out_c = canon(out);
            if flow_is_leak(model, &out_c) {
                let flow_var = model.variables.iter().find_map(|vv| match vv {
                    datamodel::Variable::Flow(f) if canon(&f.ident) == out_c => Some(f),
                    _ => None,
                });
                let (zone_start, zone_end, integers, frac_expr) = match flow_var {
                    Some(f) => {
                        let lk = f.compat.leakage.as_ref().unwrap();
                        (
                            parse_zone(&lk.zone_start, 0.0),
                            parse_zone(&lk.zone_end, 1.0),
                            lk.integers,
                            leak_fraction_expr(f),
                        )
                    }
                    None => (0.0, 1.0, false, "0".to_string()),
                };
                let frac_aux = leak_frac_name(&out_c);
                new_auxes.push(make_aux(&frac_aux, &frac_expr));
                driven_flows.push(out_c.clone());
                leak_metas.push(LeakMeta {
                    flow: out_c,
                    frac_aux,
                    zone_start,
                    zone_end,
                    integers,
                });
            } else if primary.is_none() {
                primary = Some(out_c);
            }
            // A second non-leak outflow is unusual (the conveyor model is one
            // primary + leaks); the first non-leak outflow wins as the primary
            // and any extra is left as an ordinary flow (documented limitation).
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
        driven_flows.push(primary_out.clone());

        // Synthesize the parameter auxes.
        let len_aux = param_aux_name(&stock_name, "len");
        new_auxes.push(make_aux(&len_aux, &conv.transit_time));
        let mk = |field: &Option<String>,
                  param: &str,
                  out: &mut Vec<datamodel::Aux>|
         -> Option<String> {
            field.as_ref().map(|expr| {
                let name = param_aux_name(&stock_name, param);
                out.push(make_aux(&name, expr));
                name
            })
        };
        let cap_aux = mk(&conv.capacity, "cap", &mut new_auxes);
        let inlim_aux = mk(&conv.inflow_limit, "inlim", &mut new_auxes);
        let sample_aux = mk(&conv.sample, "sample", &mut new_auxes);
        let arrest_aux = mk(&conv.arrest, "arrest", &mut new_auxes);

        let inflows = stock
            .inflows
            .iter()
            .map(|inf| {
                let inf_c = canon(inf);
                let placement = inflow_placement(model, &inf_c)?;
                // Conveyor-driven iff this inflow is a driven outflow of a
                // conveyor (resolved after we know all driven flows).
                Ok(InflowMeta {
                    flow: inf_c,
                    conveyor_driven: false, // filled below
                    placement,
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
            primary_dest_conveyor,
        });
    }

    // Now that every driven flow is known, mark conveyor-driven inflows.
    let driven_set: std::collections::HashSet<String> = driven_flows.iter().cloned().collect();
    for meta in &mut metas {
        for inflow in &mut meta.inflows {
            inflow.conveyor_driven = driven_set.contains(&inflow.flow);
        }
    }

    // Reject any equation that reads a conveyor-driven flow by name: the pass
    // runs after the flows phase, so a reader would see the pre-pass placeholder
    // 0 instead of the belt-driven rate. Loud error, never silent (§4.3).
    // Structural inflow linkage (a driven outflow feeding a downstream conveyor)
    // is handled by the pass and does NOT go through an equation reference, so
    // it is not caught here.
    {
        let model = &project.models[model_idx];
        for v in &model.variables {
            let self_name = canon(v.get_ident());
            if driven_set.contains(&self_name) {
                continue; // a driven flow's own placeholder equation is not a reader
            }
            for eqn in equation_scalar_strings(v) {
                let Ok(Some(ast)) = crate::ast::Expr0::new(&eqn, crate::lexer::LexerType::Equation)
                else {
                    continue;
                };
                for driven in &driven_set {
                    if ast.get_var_loc(driven).is_some() {
                        return Err((
                            ErrorCode::ConveyorDrivenFlowRead,
                            format!(
                                "variable '{}' references conveyor-driven flow '{driven}'; a \
                                 conveyor outflow/leak cannot be read by another equation \
                                 (it is computed after the flows phase)",
                                v.get_ident()
                            ),
                        ));
                    }
                }
            }
        }
    }

    // Pass 2 (mutable): give every driven flow a `0` placeholder equation so it
    // compiles to a writable slot, append the synthesized auxes, and clear the
    // conveyor/leakage markers so the expanded model compiles as a plain
    // stock-and-flow model. Clearing the markers is what lets the ordinary
    // compile path reject an UN-expanded conveyor (the marker is still set)
    // while accepting this expanded one.
    let model = &mut project.models[model_idx];
    for v in &mut model.variables {
        match v {
            datamodel::Variable::Flow(f) if driven_set.contains(&canon(&f.ident)) => {
                f.equation = Equation::Scalar("0".to_string());
                // The fraction now lives in a hidden aux; the flow slot is
                // pass-driven, so its own leak/gf metadata plays no runtime role.
                f.gf = None;
                // Clear the leak marker so the expanded flow is plain.
                f.compat.leakage = None;
            }
            datamodel::Variable::Stock(s) if s.compat.conveyor.is_some() => {
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

    Ok((project, metas))
}

/// The scalar equation strings of a variable (one for a `Scalar`, each element
/// plus the default for an `Arrayed`), used to scan for references. An `Aux`,
/// `Flow`, or `Stock` all carry an `equation`; a `Module` carries none.
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

fn make_aux(ident: &str, equation: &str) -> datamodel::Aux {
    datamodel::Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    }
}

/// Resolve [`ConveyorMeta`] names to data-buffer offsets using the compiled
/// simulation's offset map. Returns `None` if any required name is missing
/// (which would indicate an internal inconsistency between expansion and
/// compilation) so the caller can fall back to a plain -- non-conveyor -- run.
pub fn resolve_plans(
    metas: &[ConveyorMeta],
    offsets: &HashMap<Ident<Canonical>, usize>,
) -> Option<Vec<ConveyorPlan>> {
    let off =
        |name: &str| -> Option<usize> { offsets.get(&Ident::<Canonical>::new(name)).copied() };
    // Stock-name -> plan index, for held-exit destination linkage.
    let stock_to_idx: HashMap<&str, usize> = metas
        .iter()
        .enumerate()
        .map(|(i, m)| (m.stock.as_str(), i))
        .collect();

    let mut plans = Vec::with_capacity(metas.len());
    for meta in metas {
        let leaks = meta
            .leaks
            .iter()
            .map(|l| {
                Some(LeakPlan {
                    flow_off: off(&l.flow)?,
                    frac_off: off(&l.frac_aux)?,
                    zone_start: l.zone_start,
                    zone_end: l.zone_end,
                    integers: l.integers,
                    dest_conveyor: None, // leak-fed chains are a later step
                })
            })
            .collect::<Option<Vec<_>>>()?;
        let inflows = meta
            .inflows
            .iter()
            .map(|i| {
                Some(InflowPlan {
                    flow_off: off(&i.flow)?,
                    conveyor_driven: i.conveyor_driven,
                    placement: i.placement.clone(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        plans.push(ConveyorPlan {
            stock_off: off(&meta.stock)?,
            len_off: off(&meta.len_aux)?,
            cap_off: meta.cap_aux.as_deref().and_then(off),
            inlim_off: meta.inlim_aux.as_deref().and_then(off),
            sample_off: meta.sample_aux.as_deref().and_then(off),
            arrest_off: meta.arrest_aux.as_deref().and_then(off),
            discrete: meta.discrete,
            exponential_leak: meta.exponential_leak,
            ignore_earlier_zone_losses: meta.ignore_earlier_zone_losses,
            primary_out_off: off(&meta.primary_out)?,
            leaks,
            inflows,
            primary_dest_conveyor: meta
                .primary_dest_conveyor
                .as_deref()
                .and_then(|s| stock_to_idx.get(s).copied()),
        });
    }
    Some(plans)
}

// ----- runtime: belt initialization and the per-DT pass -----

/// A value is "nonzero" for arrest/sample when it is nonzero and not NaN (§4.4).
fn is_nonzero(v: f64) -> bool {
    v != 0.0 && !v.is_nan()
}

/// §4.4 leak-fraction hygiene: linear fractions clamp to `[0, 1]`, exponential
/// rates to `[0, ∞)`, NaN ⇒ 0.
fn clamp_fraction(v: f64, exponential: bool) -> f64 {
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

/// Build the conveyor side table from the initials-populated data buffer
/// (`curr`), initializing each belt to its steady-state fill (§7.1) from the
/// stock's initial value. The stock `<eqn>` was evaluated by the initials pass,
/// so `curr[stock_off]` holds the scalar initial value `V`.
pub fn init_belts(
    plans: &[ConveyorPlan],
    curr: &[f64],
    dt: f64,
) -> Result<Vec<ConveyorState>, (ErrorCode, String)> {
    let mut states = Vec::with_capacity(plans.len());
    for plan in plans {
        let transit = curr[plan.len_off];
        if !transit.is_finite() || transit <= 0.0 {
            return Err((
                ErrorCode::ConveyorTransitNotPositive,
                format!("conveyor transit time must be positive and finite, got {transit}"),
            ));
        }
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
        let v = curr[plan.stock_off];
        let fracs: Vec<f64> = plan
            .leaks
            .iter()
            .map(|l| clamp_fraction(curr[l.frac_off], plan.exponential_leak))
            .collect();
        state.init_steady(transit, v, &fracs);
        states.push(state);
    }
    Ok(states)
}

/// The two-phase conveyor pass (§4.3), run once per Euler step between the flows
/// and stocks phases. Reads parameter/fraction/requested-inflow values from
/// `curr` and writes the conveyor-driven flow rates back into `curr`, so that
/// ordinary stock integration then advances every stock (including the conveyor
/// stocks) using the pass-computed rates.
pub fn run_pass(
    plans: &[ConveyorPlan],
    states: &mut [ConveyorState],
    curr: &mut [f64],
    dt: f64,
    time: f64,
    last_unit: &mut i64,
) {
    // Discrete per-time-unit in_limit budget resets at integer time boundaries.
    let unit = time.floor() as i64;
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

    // Phase B over all conveyors: admit + shift + insert.
    for (i, plan) in plans.iter().enumerate() {
        // Conveyor-driven inflow volume: those slots already hold the upstream
        // Phase-A rates written above.
        let conv_vol: f64 = plan
            .inflows
            .iter()
            .filter(|inf| inf.conveyor_driven)
            .map(|inf| curr[inf.flow_off] * dt)
            .sum();
        let eq_rates: Vec<f64> = plan
            .inflows
            .iter()
            .filter(|inf| !inf.conveyor_driven)
            .map(|inf| curr[inf.flow_off])
            .collect();
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
        // Placements aligned to eq_rates (the same non-conveyor-driven filter).
        let placements: Vec<Placement> = plan
            .inflows
            .iter()
            .filter(|inf| !inf.conveyor_driven)
            .map(|inf| inf.placement.clone())
            .collect();
        let pb = states[i].phase_b(PhaseBInputs {
            phase_a: &pa[i],
            eq_request_rates: &eq_rates,
            conv_vol,
            leak_fractions: &fracs,
            capacity,
            in_limit,
            placements: &placements,
            conv_placement: Placement::Beginning,
        });
        // Write admitted equation-driven inflow rates back (in listed order;
        // conveyor-driven inflow slots already hold the correct upstream rate).
        let mut admitted = pb.in_vols.iter();
        for inf in &plan.inflows {
            if !inf.conveyor_driven
                && let Some(v) = admitted.next()
            {
                curr[inf.flow_off] = v / dt;
            }
        }
    }
}

/// Compile `project` and resolve its conveyor plans, returning the compiled
/// simulation plus the resolved plans (empty for a non-conveyor model). This is
/// the reusable core of [`build_vm`]; a caller that needs to rebuild the VM
/// later (e.g. libsimlin's reset, which recreates the VM from the cached
/// compiled sim) keeps both halves so it can re-attach the plans. Enforces the
/// Euler-only rule (§9.4).
pub fn build_compiled(
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<(crate::vm::CompiledSimulation, Vec<ConveyorPlan>)> {
    use crate::common::{Error, ErrorKind};
    let (expanded, metas) = expand_conveyors(project, main_model)
        .map_err(|(code, msg)| Error::new(ErrorKind::Simulation, code, Some(msg)))?;
    if !metas.is_empty() && expanded.sim_specs.sim_method != datamodel::SimMethod::Euler {
        return Err(Error::new(
            ErrorKind::Simulation,
            ErrorCode::ConveyorNonEulerMethod,
            Some("conveyors require Euler integration".to_string()),
        ));
    }
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, &expanded, None);
    let compiled = crate::db::compile_project_incremental(&db, sync.project, main_model)?;
    let plans = if metas.is_empty() {
        Vec::new()
    } else {
        resolve_plans(&metas, &compiled.offsets).ok_or_else(|| {
            Error::new(
                ErrorKind::Simulation,
                ErrorCode::NotSimulatable,
                Some("internal error: conveyor plan references an unresolved slot".to_string()),
            )
        })?
    };
    Ok((compiled, plans))
}

/// Build a runnable [`Vm`](crate::vm::Vm) for `project`, wiring up conveyor
/// support when the main model contains conveyors. For a project with no
/// conveyors this is exactly the ordinary compile-and-build path (the expansion
/// is a no-op), so callers can route every simulation through it.
pub fn build_vm(
    project: &datamodel::Project,
    main_model: &str,
) -> crate::common::Result<crate::vm::Vm> {
    let (compiled, plans) = build_compiled(project, main_model)?;
    let mut vm = crate::vm::Vm::new(compiled)?;
    vm.set_conveyor_plans(plans);
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
    fn dist_and_source_spreadflow_are_rejected() {
        for method in ["dist", "source"] {
            let xml = wrap_model(&format!(
                r#"
        <stock name="belt"><eqn>0</eqn><inflow>in_f</inflow><outflow>out_f</outflow>
          <conveyor><len>4</len></conveyor></stock>
        <flow name="in_f" isee:spreadflow="{method}"><eqn>250</eqn></flow>
        <flow name="out_f"><eqn>0</eqn></flow>"#
            ));
            let project = parse(&xml);
            let main = project.models[0].name.clone();
            let err = build_vm(&project, &main)
                .err()
                .unwrap_or_else(|| panic!("{method} spreadflow should be rejected"));
            assert_eq!(
                err.code,
                ErrorCode::ConveyorSpreadflowUnsupported,
                "method {method}"
            );
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
}
