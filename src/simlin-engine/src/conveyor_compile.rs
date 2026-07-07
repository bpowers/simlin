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
//! Conveyors inside submodules, queue coupling, and container access
//! (`conv[j]`, `SUM(conv)`) reading belt slats are later build-sequence steps.

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

/// Is the flow named `flow` (canonical) a conveyor leak outflow in `model`?
fn flow_is_leak(model: &datamodel::Model, flow: &str) -> bool {
    model.variables.iter().any(|v| match v {
        datamodel::Variable::Flow(f) => canon(&f.ident) == flow && f.compat.leakage.is_some(),
        _ => false,
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

        // An arrayed conveyor is N_elem independent belts (§10). The stock's
        // dimensions drive the synthesized auxes' shape and the per-element
        // offset enumeration; empty for a scalar conveyor.
        let stock_dims = equation_dims(&stock.equation);
        let element_subscripts = element_subscripts_for_dims(&project, &stock_dims, &stock.ident)?;

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
                let (zone_start, zone_end, integers, frac_eqn) = match flow_var {
                    Some(f) => {
                        let lk = f.compat.leakage.as_ref().unwrap();
                        (
                            parse_zone(&lk.zone_start, 0.0),
                            parse_zone(&lk.zone_end, 1.0),
                            lk.integers,
                            leak_fraction_equation(f, &stock_dims),
                        )
                    }
                    None => (0.0, 1.0, false, param_equation("0", &stock_dims)),
                };
                let frac_aux = leak_frac_name(&out_c);
                new_auxes.push(make_aux_eqn(&frac_aux, frac_eqn));
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

        // Synthesize the parameter auxes, arrayed over the stock's dimensions
        // for an arrayed conveyor so each element gets its own len/cap/... slot
        // (§10); scalar for a scalar conveyor.
        let len_aux = param_aux_name(&stock_name, "len");
        new_auxes.push(make_aux_eqn(
            &len_aux,
            param_equation(&conv.transit_time, &stock_dims),
        ));
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
            primary_dest_conveyor,
            element_subscripts,
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

/// The synthesized leak-fraction aux equation for a (possibly arrayed) leak flow
/// (§5.1/§10). Prefers the explicit `<leak>` fraction (a single expression, made
/// apply-to-all over the conveyor's dims); otherwise the flow's own equation
/// carries the fraction (the bare-`<leak/>`-plus-`<eqn>` form) and its arrayed
/// shape is preserved so a genuinely per-element fraction is not flattened. An
/// empty fraction leaks nothing (`0`).
fn leak_fraction_equation(flow: &datamodel::Flow, dims: &[DimensionName]) -> Equation {
    if let Some(leak) = &flow.compat.leakage
        && let Some(frac) = &leak.fraction
        && !frac.is_empty()
    {
        return param_equation(frac, dims);
    }
    match &flow.equation {
        Equation::Scalar(s) if !s.is_empty() => param_equation(s, dims),
        Equation::ApplyToAll(d, s) if !s.is_empty() => Equation::ApplyToAll(d.clone(), s.clone()),
        Equation::Arrayed(d, elems, default, except) => {
            Equation::Arrayed(d.clone(), elems.clone(), default.clone(), *except)
        }
        _ => param_equation("0", dims),
    }
}

/// The placeholder-`0` equation for a conveyor-driven flow, preserving the flow's
/// array shape so an arrayed driven flow compiles to `N_elem` writable slots
/// (one per belt) rather than collapsing to a single scalar slot (§10). The pass
/// overwrites every slot each step, so the placeholder value never matters -- only
/// the slot count does.
fn placeholder_zero_equation(existing: &Equation) -> Equation {
    match existing {
        Equation::Scalar(_) => Equation::Scalar("0".to_string()),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => {
            Equation::ApplyToAll(dims.clone(), "0".to_string())
        }
    }
}

/// The dimension names an arrayed conveyor stock (or driven flow) is declared
/// over, in declaration order; empty for a scalar variable.
fn equation_dims(equation: &Equation) -> Vec<DimensionName> {
    match equation {
        Equation::Scalar(_) => Vec::new(),
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => dims.clone(),
    }
}

/// Resolve the named dimensions of an arrayed conveyor to the runtime
/// [`Dimension`](crate::dimensions::Dimension) list (with element names), in
/// declaration order, then enumerate the per-element subscript suffixes in the
/// SAME row-major order the compiled offset map uses
/// (`calc_flattened_offsets_incremental` drives its element keys off the identical
/// `SubscriptIterator`). Each returned suffix is the canonical `elem1,elem2`
/// string. Returns an error if any dimension name is unknown in the project (an
/// internal-consistency guard, §10).
fn element_subscripts_for_dims(
    project: &datamodel::Project,
    dim_names: &[DimensionName],
    stock: &str,
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
                        "arrayed conveyor '{stock}' is declared over dimension '{name}', which \
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

/// The number of independent belts a conveyor meta expands to: `N_elem` for an
/// arrayed conveyor (§10), 1 for a scalar one (the degenerate case, whose
/// `element_subscripts` is empty).
fn n_belts(meta: &ConveyorMeta) -> usize {
    meta.element_subscripts.len().max(1)
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
/// compilation that [`build_compiled`] surfaces as a hard `NotSimulatable`
/// error (there is no non-conveyor fallback: the model has conveyors).
pub fn resolve_plans(
    metas: &[ConveyorMeta],
    offsets: &HashMap<Ident<Canonical>, usize>,
) -> Option<Vec<ConveyorPlan>> {
    let off =
        |name: &str| -> Option<usize> { offsets.get(&Ident::<Canonical>::new(name)).copied() };

    // Stock-name -> (base flattened-plan index, belt count). Each meta's belts
    // are appended in order, so its range is [base, base + n_belts).
    let mut stock_to_range: HashMap<&str, (usize, usize)> = HashMap::new();
    let mut base = 0usize;
    for m in metas {
        stock_to_range.insert(m.stock.as_str(), (base, n_belts(m)));
        base += n_belts(m);
    }

    let mut plans = Vec::with_capacity(base);
    for meta in metas {
        for e in 0..n_belts(meta) {
            // Element-aware offset resolver: the bare name for a scalar conveyor,
            // the `name[elem]` subscripted key for element `e` of an arrayed one.
            let eoff = |name: &str| -> Option<usize> {
                if meta.element_subscripts.is_empty() {
                    off(name)
                } else {
                    off(&format!("{}[{}]", name, meta.element_subscripts[e]))
                }
            };
            let leaks = meta
                .leaks
                .iter()
                .map(|l| {
                    Some(LeakPlan {
                        flow_off: eoff(&l.flow)?,
                        frac_off: eoff(&l.frac_aux)?,
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
                        flow_off: eoff(&i.flow)?,
                        conveyor_driven: i.conveyor_driven,
                        placement: i.placement.clone(),
                        dist: i.dist.clone(),
                        source: i.source,
                    })
                })
                .collect::<Option<Vec<_>>>()?;
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
                primary_dest_conveyor,
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
        // The just-latched entry depth `d` (§4.1/§6). `dist`/`source` weight
        // vectors span `0..d`, so they are recomputed here every step -- a
        // time-varying transit changes `d`, hence the placement geometry.
        let d = states[i].entry_depth();
        // Split inflows in listed order into conveyor-driven `(volume, placement)`
        // pairs (admitted unconditionally) and equation-driven rates + their
        // placements. Conveyor-driven slots already hold the upstream Phase-A
        // rates written above.
        let mut eq_rates: Vec<f64> = Vec::new();
        let mut placements: Vec<Placement> = Vec::new();
        let mut conv_inflows: Vec<(f64, Placement)> = Vec::new();
        for inf in &plan.inflows {
            if inf.conveyor_driven {
                let vol = curr[inf.flow_off] * dt;
                conv_inflows.push((vol, conv_inflow_placement(inf, plans, &pa, d)));
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
        let (compiled, plans) = build_compiled(&project, &main).expect("build_compiled");
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
