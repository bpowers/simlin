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
//! variable's entry and lists as many diagnostics as fit, when the entries and
//! every error fit; otherwise the model is outlined by sector: counts by kind,
//! then the errors, the stocks with their flows, the sectors that hold
//! anything, the warnings, and the other variables' names, each list filled
//! only once every list before it is whole, with the rest counted -- so an agent reading a large model learns
//! first why it does not simulate, then gets its stocks and a map of names to
//! read or search. A diagnostic left out is still named by the ids on its
//! variable's entry, and `read_variables` gives it in full. No outline is over
//! its budget: one that cannot fit with every list empty is refused.

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
    /// Present when the model is outlined by sector: its sectors that hold a
    /// variable, each with how many it holds and which stocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sectors: Option<Vec<SectorOutline>>,
    /// In a sector outline, the names of the variables that are not stocks,
    /// in model order, as many as fit beside the stocks.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub other_names: Vec<String>,
    /// How many stocks, diagnostics, sectors and other names an outline left
    /// out to keep to its budget.
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
#[derive(Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Omitted {
    pub stocks: usize,
    pub diagnostics: usize,
    pub other_names: usize,
    /// The sectors left out; a sector with no variable is never listed and
    /// is not counted.
    pub sectors: usize,
}

/// Answer `read_model`, and remember what was read.
pub(crate) fn read_model(
    session: &mut Session,
    ws: &Workspace<'_>,
    _input: ReadModelInput,
) -> Result<ReadModelOutput, ToolError> {
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let diagnostics = session.evidence.report_diagnostics(ws, &resolved);
    // The model resolved above, so the report is never refused here; it is
    // fitted to a quarter of the budget, so it never refuses the read.
    let changes = session
        .changes_since_read(ws.project, ws.revision)
        .unwrap_or_default()
        .map(|changes| changes.fitted(session.outline_budget / 4));

    let full = full_outline(ws, resolved.model, diagnostics, changes);
    let budget = session.outline_budget;
    let outline = if serialized_len(&full) <= budget {
        full
    } else if let Some(fitted) = with_fitted_diagnostics(&full, budget) {
        fitted
    } else {
        sector_outline(full, resolved.model, budget)?
    };
    // A refused read is no read: only an outline that was answered is what
    // the session last read.
    session.last_read = Some(ReadSnapshot::new(ws.revision, ws.project, resolved.model));
    Ok(outline)
}

/// The whole outline with as many of its diagnostics as fit the budget, in
/// order (errors first), or `None` when its entries and every error do not
/// fit: an outline never lists an entry while it leaves an error out, so
/// one that cannot keep every error is outlined by sector, errors first.
fn with_fitted_diagnostics(full: &ReadModelOutput, budget: usize) -> Option<ReadModelOutput> {
    let total = full.diagnostics.len();
    let errors = full
        .diagnostics
        .iter()
        .take_while(|d| d.severity == super::Severity::Error)
        .count();
    let with = |listed: usize| ReadModelOutput {
        diagnostics: full.diagnostics[..listed].to_vec(),
        omitted: Some(Omitted {
            diagnostics: total - listed,
            ..Omitted::default()
        }),
        note: Some(DIAGNOSTICS_LEFT_OUT.to_string()),
        ..full.clone()
    };
    let fits = |listed: usize| serialized_len(&with(listed)) <= budget;
    if !fits(errors) {
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

    // Flows as the model names them, the spelling every other name in the
    // outline has, whatever spelling the stock's own list holds.
    let named = |flows: Vec<String>| -> Vec<String> {
        flows
            .iter()
            .map(|flow| super::evidence::display_name(model, flow))
            .collect()
    };
    for var in &model.variables {
        match var {
            Variable::Stock(stock) => {
                let (initial, _) = truncated(equation_text(&stock.equation));
                outline.stocks.push(StockOutline {
                    name: stock.ident.clone(),
                    units: stock.units.clone(),
                    initial: Some(initial),
                    inflows: named(datamodel::distinct_stock_flows(&stock.inflows).flows),
                    outflows: named(datamodel::distinct_stock_flows(&stock.outflows).flows),
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

/// The outline by sector, in the order an agent needs it when not everything
/// fits: the errors, which say why a model does not simulate; the stocks with
/// their flows but not their initial values; the sectors that hold anything;
/// the warnings; and the other variables' names. Each list is filled, in its
/// own order, only once every list before it is whole, so a partial list is
/// never read beside a later one as if it were whole (names beside half the
/// stocks would read as a map of the model with stocks missing from it).
///
/// An outline with every list empty that is still over the budget -- a model
/// name, time units or change report too long for one answer -- is refused:
/// an answer over its budget is never given.
fn sector_outline(
    full: ReadModelOutput,
    model: &datamodel::Model,
    budget: usize,
) -> Result<ReadModelOutput, ToolError> {
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
    // A sector with no variable says nothing about the model.
    let sectors: Vec<SectorOutline> = model
        .groups
        .iter()
        .filter(|group| !group.members.is_empty())
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
    // The full outline lists errors first.
    let (errors, warnings) = full.diagnostics.split_at(full.counts.errors);
    let totals = Listed {
        errors: errors.len(),
        stocks: stocks.len(),
        sectors: sectors.len(),
        warnings: warnings.len(),
        names: other_names.len(),
    };

    let with = |listed: Listed| {
        let omitted = Omitted {
            stocks: totals.stocks - listed.stocks,
            diagnostics: (totals.errors - listed.errors) + (totals.warnings - listed.warnings),
            other_names: totals.names - listed.names,
            sectors: totals.sectors - listed.sectors,
        };
        ReadModelOutput {
            stocks: stocks[..listed.stocks].to_vec(),
            flows: vec![],
            variables: vec![],
            constants: vec![],
            lookups: vec![],
            modules: vec![],
            diagnostics: errors[..listed.errors]
                .iter()
                .chain(&warnings[..listed.warnings])
                .cloned()
                .collect(),
            sectors: (listed.sectors > 0).then(|| sectors[..listed.sectors].to_vec()),
            other_names: other_names[..listed.names].to_vec(),
            omitted: (omitted != Omitted::default()).then_some(omitted),
            note: Some(
                "The model is too large to outline whole, so its stocks are listed with their \
                 flows and every other variable by name; read_variables reads any variable's \
                 equation and diagnostics, and find_variables finds one by name or description."
                    .to_string(),
            ),
            revision: full.revision,
            model: full.model.clone(),
            specs: full.specs.clone(),
            counts: full.counts,
            changes: full.changes.clone(),
        }
    };

    let fits = |listed: Listed| serialized_len(&with(listed)) <= budget;
    let mut listed = Listed::default();
    if !fits(listed) {
        return Err(ToolError::new(format!(
            "the model's outline does not fit one answer even with every list left out ({} \
             bytes, and an answer holds {budget}): the model's name, its time units or the \
             report of what changed is too long. read_variables and find_variables still answer.",
            serialized_len(&with(listed))
        )));
    }
    // One list at a time, in order of need; the first that does not fit whole
    // ends the filling.
    for part in Part::ALL {
        let total = part.of(&totals);
        let most = most(total, &|n| fits(part.set(listed, n)));
        listed = part.set(listed, most);
        if most < total {
            break;
        }
    }
    Ok(with(listed))
}

/// How many of each of a sector outline's lists are listed.
#[derive(Clone, Copy, Default)]
struct Listed {
    errors: usize,
    stocks: usize,
    sectors: usize,
    warnings: usize,
    names: usize,
}

/// A sector outline's lists, in the order they are filled.
#[derive(Clone, Copy)]
enum Part {
    Errors,
    Stocks,
    Sectors,
    Warnings,
    Names,
}

impl Part {
    const ALL: [Part; 5] = [
        Part::Errors,
        Part::Stocks,
        Part::Sectors,
        Part::Warnings,
        Part::Names,
    ];

    fn of(self, listed: &Listed) -> usize {
        match self {
            Part::Errors => listed.errors,
            Part::Stocks => listed.stocks,
            Part::Sectors => listed.sectors,
            Part::Warnings => listed.warnings,
            Part::Names => listed.names,
        }
    }

    fn set(self, mut listed: Listed, count: usize) -> Listed {
        match self {
            Part::Errors => listed.errors = count,
            Part::Stocks => listed.stocks = count,
            Part::Sectors => listed.sectors = count,
            Part::Warnings => listed.warnings = count,
            Part::Names => listed.names = count,
        }
        listed
    }
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
        // Read, never passed back: a long string is cut as a quote is.
        time_units: specs
            .time_units
            .as_deref()
            .filter(|u| !u.is_empty())
            .map(|u| super::evidence::window(u, 0, 0, super::evidence::QUOTE_CHARS)),
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
