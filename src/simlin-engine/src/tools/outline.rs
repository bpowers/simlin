// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `read_model`: an outline of the model an agent can hold in its head.
//!
//! The outline lists the sim specs, each stock with its initial value and its
//! flows, the flows, the other variables, the constants, the lookup tables and
//! the modules -- one entry each, with its equation and units -- and the
//! diagnostics under their ids, errors first, each variable carrying the ids of
//! its own. An outline whose JSON exceeds the session's budget keeps every
//! variable's entry and lists as many diagnostics as fit, when the entries fit
//! alone; otherwise the model is outlined by sector: the stocks with their
//! flows, counts by kind, the model's sectors, the diagnostics, and the other
//! variables' names, as many of each as fit in that order, with the rest
//! counted -- so an agent reading a large model gets its stocks and a map of
//! names to read or search. A diagnostic left out is still named by the ids
//! on its variable's entry, and `read_variables` gives it in full.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel::{self, Equation, Variable};

use super::changes::{Changes, ReadSnapshot, effective_specs};
use super::evidence::DiagnosticReport;
use super::{Session, ToolError, Workspace, resolve_model};

/// The longest equation text an outline entry carries; `read_variables` has
/// the whole of it.
pub(crate) const OUTLINE_EQUATION_CHARS: usize = 240;

/// `read_model` takes nothing: a session is bound to its model.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReadModelInput {}

/// The outline of a model.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReadModelOutput {
    /// The project revision this outline read.
    pub revision: u64,
    pub model: String,
    pub specs: SpecsOutline,
    pub counts: Counts,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stocks: Vec<StockOutline>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<VariableOutline>,
    /// The computed auxiliaries.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<VariableOutline>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constants: Vec<ConstantOutline>,
    /// Standalone lookup tables, which other equations call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lookups: Vec<LookupOutline>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ModuleOutline>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticReport>,
    /// What changed since this session last read the model, when anything did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub changes: Option<Changes>,
    /// Present when the model is outlined by sector: its sectors, each with
    /// how many variables it holds and which stocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sectors: Option<Vec<SectorOutline>>,
    /// In a sector outline, the names of the variables that are not stocks,
    /// in model order, as many as fit beside the stocks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_names: Vec<String>,
    /// How many stocks, diagnostics and other names a sector outline left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted: Option<Omitted>,
    /// What an outline by sector leaves to `read_variables`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SpecsOutline {
    pub start: f64,
    pub stop: f64,
    pub dt: f64,
    /// How often results are saved, when it differs from DT.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_step: Option<f64>,
    pub method: IntegrationMethod,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_units: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum IntegrationMethod {
    Euler,
    Rk2,
    Rk4,
}

/// How many of each kind of variable the model has, and of each severity of
/// diagnostic: always whole, whatever the outline lists.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub stocks: usize,
    pub flows: usize,
    pub variables: usize,
    pub constants: usize,
    pub lookups: usize,
    pub modules: usize,
    pub errors: usize,
    pub warnings: usize,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct StockOutline {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// The initial value's equation; absent in a sector outline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    pub inflows: Vec<String>,
    pub outflows: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_negative: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// A flow or a computed auxiliary.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct VariableOutline {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// The equation, cut at 240 characters (`truncated` says so).
    pub equation: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<String>,
    /// The graphical function the equation's value is looked up in, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup: Option<LookupSummary>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_negative: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ConstantOutline {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// The value as written; for an arrayed constant, each element's.
    pub value: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LookupOutline {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    pub lookup: LookupSummary,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// A graphical function's extent; `read_variables` has its points.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LookupSummary {
    pub points: usize,
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModuleOutline {
    pub name: String,
    /// The model the module instantiates.
    pub model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<ModuleInputOutline>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
}

/// One wire into a module: the variable it reads, and the module's input it
/// feeds.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModuleInputOutline {
    pub from: String,
    pub to: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct SectorOutline {
    pub name: String,
    pub variables: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stocks: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Omitted {
    pub stocks: usize,
    pub diagnostics: usize,
    pub other_names: usize,
}

/// Answer `read_model`, and remember what was read.
pub(crate) fn read_model(
    session: &mut Session,
    ws: &Workspace<'_>,
    _input: ReadModelInput,
) -> Result<ReadModelOutput, ToolError> {
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let diagnostics = session.evidence.report_diagnostics(ws, &resolved);
    let changes = session.changes_since_read(ws.project, ws.revision);
    session.last_read = Some(ReadSnapshot::new(ws.revision, ws.project, resolved.model));

    let full = full_outline(ws, resolved.model, diagnostics, changes);
    let budget = session.outline_budget;
    if serialized_len(&full) <= budget {
        return Ok(full);
    }
    if let Some(fitted) = with_fitted_diagnostics(&full, budget) {
        return Ok(fitted);
    }
    Ok(sector_outline(full, resolved.model, budget))
}

/// The whole outline with as many of its diagnostics as fit the budget, in
/// order (errors first), or `None` when its entries alone do not fit.
fn with_fitted_diagnostics(full: &ReadModelOutput, budget: usize) -> Option<ReadModelOutput> {
    let total = full.diagnostics.len();
    let with = |listed: usize| ReadModelOutput {
        diagnostics: full.diagnostics[..listed].to_vec(),
        omitted: Some(Omitted {
            stocks: 0,
            diagnostics: total - listed,
            other_names: 0,
        }),
        note: Some(DIAGNOSTICS_LEFT_OUT.to_string()),
        ..full.clone()
    };
    let fits = |listed: usize| serialized_len(&with(listed)) <= budget;
    if !fits(0) {
        return None;
    }
    Some(with(most(total, &fits)))
}

/// What an outline that leaves diagnostics out says about them.
const DIAGNOSTICS_LEFT_OUT: &str = "Some diagnostics are left out to fit the outline (errors are \
     listed first); every entry names the ids of its variable's diagnostics, and read_variables \
     gives them in full.";

/// The largest count up to `upto` that `fits`, which holds for a prefix of the
/// counts: a bisection, each probe serializing one outline.
fn most(upto: usize, fits: &dyn Fn(usize) -> bool) -> usize {
    if fits(upto) {
        return upto;
    }
    let (mut low, mut high) = (0, upto);
    while low < high {
        let mid = (low + high).div_ceil(2);
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

fn serialized_len(outline: &ReadModelOutput) -> usize {
    serde_json::to_string(outline)
        .expect("outlines serialize")
        .len()
}

/// Every variable's entry.
fn full_outline(
    ws: &Workspace<'_>,
    model: &datamodel::Model,
    diagnostics: Vec<DiagnosticReport>,
    changes: Option<Changes>,
) -> ReadModelOutput {
    let mut ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for report in &diagnostics {
        if let Some(variable) = &report.variable {
            ids.entry(crate::canonicalize(variable).into_owned())
                .or_default()
                .push(report.id.clone());
        }
    }
    let ids_of = |var: &Variable| {
        ids.get(crate::canonicalize(var.get_ident()).as_ref())
            .cloned()
            .unwrap_or_default()
    };

    // Errors first, each severity in report order, so an outline that leaves
    // diagnostics out keeps the ones that stop the model.
    let mut diagnostics = diagnostics;
    diagnostics.sort_by_key(|d| d.severity != super::Severity::Error);
    let mut outline = ReadModelOutput {
        revision: ws.revision,
        model: model.name.clone(),
        specs: specs_outline(effective_specs(ws.project, model)),
        counts: Counts {
            errors: diagnostics
                .iter()
                .filter(|d| d.severity == super::Severity::Error)
                .count(),
            warnings: diagnostics
                .iter()
                .filter(|d| d.severity == super::Severity::Warning)
                .count(),
            ..Counts::default()
        },
        stocks: vec![],
        flows: vec![],
        variables: vec![],
        constants: vec![],
        lookups: vec![],
        modules: vec![],
        diagnostics,
        changes,
        sectors: None,
        other_names: vec![],
        omitted: None,
        note: None,
    };

    for var in &model.variables {
        match var {
            Variable::Stock(stock) => {
                let (initial, _) = truncated(equation_text(&stock.equation));
                outline.stocks.push(StockOutline {
                    name: stock.ident.clone(),
                    units: stock.units.clone(),
                    initial: Some(initial),
                    inflows: datamodel::distinct_stock_flows(&stock.inflows).flows,
                    outflows: datamodel::distinct_stock_flows(&stock.outflows).flows,
                    dimensions: dimensions(&stock.equation),
                    non_negative: stock.compat.non_negative,
                    diagnostics: ids_of(var),
                });
            }
            Variable::Flow(flow) => {
                outline.flows.push(variable_outline(
                    &flow.ident,
                    &flow.units,
                    &flow.equation,
                    flow.gf.as_ref(),
                    flow.compat.non_negative,
                    ids_of(var),
                ));
            }
            Variable::Aux(aux) => match aux_kind(aux) {
                AuxKind::Lookup(gf) => outline.lookups.push(LookupOutline {
                    name: aux.ident.clone(),
                    units: aux.units.clone(),
                    lookup: lookup_summary(gf),
                    diagnostics: ids_of(var),
                }),
                AuxKind::Constant(value) => outline.constants.push(ConstantOutline {
                    name: aux.ident.clone(),
                    units: aux.units.clone(),
                    value,
                    dimensions: dimensions(&aux.equation),
                    diagnostics: ids_of(var),
                }),
                AuxKind::Computed => outline.variables.push(variable_outline(
                    &aux.ident,
                    &aux.units,
                    &aux.equation,
                    aux.gf.as_ref(),
                    aux.compat.non_negative,
                    ids_of(var),
                )),
            },
            Variable::Module(module) => outline.modules.push(ModuleOutline {
                name: module.ident.clone(),
                model: module.model_name.clone(),
                inputs: module
                    .references
                    .iter()
                    .map(|r| ModuleInputOutline {
                        from: r.src.clone(),
                        to: r.dst.clone(),
                    })
                    .collect(),
                diagnostics: ids_of(var),
            }),
        }
    }
    outline.counts.stocks = outline.stocks.len();
    outline.counts.flows = outline.flows.len();
    outline.counts.variables = outline.variables.len();
    outline.counts.constants = outline.constants.len();
    outline.counts.lookups = outline.lookups.len();
    outline.counts.modules = outline.modules.len();
    outline
}

/// The outline by sector: the stocks with their flows but not their initial
/// values, the model's sectors, the diagnostics, and the other variables'
/// names -- as many stocks as the budget holds, then, once every stock is
/// listed, as many diagnostics (errors first), then, once every diagnostic is
/// listed, as many names, each in model order.
fn sector_outline(
    full: ReadModelOutput,
    model: &datamodel::Model,
    budget: usize,
) -> ReadModelOutput {
    let stocks: Vec<StockOutline> = full
        .stocks
        .iter()
        .cloned()
        .map(|stock| StockOutline {
            initial: None,
            ..stock
        })
        .collect();
    let other_names: Vec<String> = model
        .variables
        .iter()
        .filter(|var| !matches!(var, Variable::Stock(_)))
        .map(|var| var.get_ident().to_string())
        .collect();
    let sectors: Vec<SectorOutline> = model
        .groups
        .iter()
        .map(|group| SectorOutline {
            name: group.name.clone(),
            variables: group.members.len(),
            stocks: group
                .members
                .iter()
                .filter_map(|member| match model.get_variable(member) {
                    Some(Variable::Stock(stock)) => Some(stock.ident.clone()),
                    _ => None,
                })
                .collect(),
        })
        .collect();
    let all_diagnostics = full.diagnostics.clone();

    let with = |stock_count: usize, diagnostic_count: usize, name_count: usize| {
        let omitted = Omitted {
            stocks: stocks.len() - stock_count,
            diagnostics: all_diagnostics.len() - diagnostic_count,
            other_names: other_names.len() - name_count,
        };
        ReadModelOutput {
            stocks: stocks[..stock_count].to_vec(),
            flows: vec![],
            variables: vec![],
            constants: vec![],
            lookups: vec![],
            modules: vec![],
            diagnostics: all_diagnostics[..diagnostic_count].to_vec(),
            sectors: (!sectors.is_empty()).then(|| sectors.clone()),
            other_names: other_names[..name_count].to_vec(),
            omitted: (omitted.stocks + omitted.diagnostics + omitted.other_names > 0)
                .then_some(omitted),
            note: Some(
                "The model is too large to outline whole, so its stocks are listed with their \
                 flows and every other variable by name; read_variables reads any variable's \
                 equation and diagnostics, and find_variables finds one by name or description."
                    .to_string(),
            ),
            ..full.clone()
        }
    };

    // The most stocks that fit alone, then, once every stock fits, the most
    // diagnostics beside them, then the most names: each a bisection over a
    // count. Names never share the budget with a partial stock list, which
    // would read as a map of the model with stocks missing from it.
    let fits = |outline: ReadModelOutput| serialized_len(&outline) <= budget;
    let stock_count = most(stocks.len(), &|n| fits(with(n, 0, 0)));
    let diagnostic_count = if stock_count == stocks.len() {
        most(all_diagnostics.len(), &|n| fits(with(stock_count, n, 0)))
    } else {
        0
    };
    let name_count = if diagnostic_count == all_diagnostics.len() && stock_count == stocks.len() {
        most(other_names.len(), &|n| {
            fits(with(stock_count, diagnostic_count, n))
        })
    } else {
        0
    };
    with(stock_count, diagnostic_count, name_count)
}

fn specs_outline(specs: &datamodel::SimSpecs) -> SpecsOutline {
    let dt = dt_value(&specs.dt);
    let save_step = specs
        .save_step
        .as_ref()
        .map(dt_value)
        .filter(|&save| save != dt);
    SpecsOutline {
        start: specs.start,
        stop: specs.stop,
        dt,
        save_step,
        method: match specs.sim_method {
            datamodel::SimMethod::Euler => IntegrationMethod::Euler,
            datamodel::SimMethod::RungeKutta2 => IntegrationMethod::Rk2,
            datamodel::SimMethod::RungeKutta4 => IntegrationMethod::Rk4,
        },
        time_units: specs.time_units.clone().filter(|u| !u.is_empty()),
    }
}

pub(crate) fn dt_value(dt: &datamodel::Dt) -> f64 {
    match dt {
        datamodel::Dt::Dt(value) => *value,
        datamodel::Dt::Reciprocal(value) => 1.0 / value,
    }
}

fn variable_outline(
    name: &str,
    units: &Option<String>,
    equation: &Equation,
    gf: Option<&datamodel::GraphicalFunction>,
    non_negative: bool,
    diagnostics: Vec<String>,
) -> VariableOutline {
    let (text, truncated) = truncated(equation_text(equation));
    VariableOutline {
        name: name.to_string(),
        units: units.clone(),
        equation: text,
        truncated,
        dimensions: dimensions(equation),
        lookup: gf.map(lookup_summary),
        non_negative,
        diagnostics,
    }
}

/// What an auxiliary is, for the outline's lists.
pub(crate) enum AuxKind<'a> {
    /// A table other equations call: a graphical function with no equation
    /// of its own to look up (the engine's lookup-only variable).
    Lookup(&'a datamodel::GraphicalFunction),
    /// A number, or an arrayed variable whose every element is one.
    Constant(String),
    Computed,
}

pub(crate) fn aux_kind(aux: &datamodel::Aux) -> AuxKind<'_> {
    if let Some(gf) = &aux.gf
        && is_lookup_only_equation(&aux.equation)
    {
        return AuxKind::Lookup(gf);
    }
    if aux.gf.is_none()
        && let Some(value) = constant_value(&aux.equation)
    {
        return AuxKind::Constant(value);
    }
    AuxKind::Computed
}

/// The engine's rule for a lookup-only variable's equation: empty, or `0+0`
/// (what some importers write for "no equation").
fn is_lookup_only_equation(equation: &Equation) -> bool {
    match equation {
        Equation::Scalar(text) | Equation::ApplyToAll(_, text) => {
            let text = text.trim();
            text.is_empty() || text == "0+0"
        }
        Equation::Arrayed(..) => false,
    }
}

/// An equation's value when it is a finite number, or for an arrayed equation
/// when every element's is. `NaN`, which an importer writes for an equation
/// the model never filled in, is no constant: it is an equation that has yet
/// to be written, and its diagnostics say so.
pub(crate) fn constant_value(equation: &Equation) -> Option<String> {
    let number = |text: &str| text.trim().parse::<f64>().is_ok_and(f64::is_finite);
    match equation {
        Equation::Scalar(text) | Equation::ApplyToAll(_, text) => {
            number(text).then(|| text.trim().to_string())
        }
        Equation::Arrayed(_, elements, default, _) => {
            let all_numbers = elements
                .iter()
                .all(|(_, text, _, gf)| gf.is_none() && number(text))
                && default.as_deref().is_none_or(number);
            all_numbers.then(|| equation_text(equation))
        }
    }
}

/// An equation as one line: the text itself, or for per-element equations
/// each element's, `element: equation` separated by `; `, with the EXCEPT
/// default last as `others`.
pub(crate) fn equation_text(equation: &Equation) -> String {
    match equation {
        Equation::Scalar(text) | Equation::ApplyToAll(_, text) => text.clone(),
        Equation::Arrayed(_, elements, default, _) => {
            let mut parts: Vec<String> = elements
                .iter()
                .map(|(element, text, _, _)| format!("{element}: {}", text.trim()))
                .collect();
            if let Some(default) = default {
                parts.push(format!("others: {}", default.trim()));
            }
            parts.join("; ")
        }
    }
}

pub(crate) fn dimensions(equation: &Equation) -> Vec<String> {
    match equation {
        Equation::Scalar(_) => vec![],
        Equation::ApplyToAll(dims, _) | Equation::Arrayed(dims, ..) => dims.clone(),
    }
}

/// `text` cut to [`OUTLINE_EQUATION_CHARS`] characters, and whether it was.
fn truncated(text: String) -> (String, bool) {
    if text.chars().count() <= OUTLINE_EQUATION_CHARS {
        return (text, false);
    }
    let mut cut: String = text.chars().take(OUTLINE_EQUATION_CHARS).collect();
    cut.push('…');
    (cut, true)
}

pub(crate) fn lookup_summary(gf: &datamodel::GraphicalFunction) -> LookupSummary {
    let (x_min, x_max) = match &gf.x_points {
        Some(points) if !points.is_empty() => (
            points.iter().copied().fold(f64::INFINITY, f64::min),
            points.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        ),
        _ => (gf.x_scale.min, gf.x_scale.max),
    };
    let (y_min, y_max) = if gf.y_points.is_empty() {
        (gf.y_scale.min, gf.y_scale.max)
    } else {
        (
            gf.y_points.iter().copied().fold(f64::INFINITY, f64::min),
            gf.y_points
                .iter()
                .copied()
                .fold(f64::NEG_INFINITY, f64::max),
        )
    };
    LookupSummary {
        points: gf.y_points.len(),
        x_min,
        x_max,
        y_min,
        y_max,
    }
}

#[cfg(test)]
#[path = "outline_tests.rs"]
mod tests;
