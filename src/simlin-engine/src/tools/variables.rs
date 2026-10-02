// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `read_variables`, a variable's whole record, and `find_variables`, a search
//! by name or description.
//!
//! A record carries everything a variable's definition holds -- its equation
//! or initial value, per-element equations, lookup points, flows, module
//! wiring -- and where it sits in the causal structure: the variables it reads
//! and the variables that read it, each link with its polarity
//! ([`crate::analysis::model_links`], macro and module internals collapsed
//! into the links between the variables a modeler wrote).
//!
//! An answer keeps to the session's byte budget. A record lists at most 24
//! per-element equations (each cut at 240 characters, 2,400 in all), 24
//! inputs, 24 readers and 64 lookup points, with how many more there are, and
//! cuts its documentation at 480 characters; an element's own equation is
//! read whole by naming it (`population[north]`). Records that do not fit are
//! named for another call.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schema")]
use schemars::JsonSchema;

use crate::datamodel::{self, Equation, Variable};
use crate::ltm::LinkPolarity;

use super::evidence::{DiagnosticReport, display_name, window};
use super::outline::{AuxKind, aux_kind, dimensions};
use super::series::{SeriesCore, keyed_series_upto, scale_in_run};
use super::{Session, ToolError, Workspace, names, resolve_model};

/// The most variables one `read_variables` call reads.
pub(crate) const MAX_READ_VARIABLES: usize = 12;
/// The most matches `find_variables` returns.
pub(crate) const MAX_MATCHES: usize = 10;
/// The fewest similarity a `find_variables` match needs.
pub(crate) const MATCH_THRESHOLD: f64 = 0.4;
/// The most per-element equations a record lists.
pub(crate) const MAX_RECORD_ELEMENTS: usize = 24;
/// The most characters of per-element equations a record lists, each cut at
/// 240: a record of a large arrayed variable is a summary of its elements,
/// and naming an element reads its equation whole.
pub(crate) const MAX_ELEMENT_TEXT_CHARS: usize = 2_400;
/// The most inputs, and the most readers, a record lists.
pub(crate) const MAX_RECORD_LINKS: usize = 24;
/// The most points of a lookup a record lists.
pub(crate) const MAX_LOOKUP_POINTS: usize = 64;
/// The longest documentation a record carries.
pub(crate) const MAX_DOCUMENTATION_CHARS: usize = 480;

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ReadVariablesInput {
    /// The variables to read, by name (case, spaces and underscores do not
    /// matter), or one element of an arrayed variable (`population[north]`);
    /// at most 12.
    #[cfg_attr(feature = "schema", schemars(length(min = 1, max = 12)))]
    pub names: Vec<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ReadVariablesOutput {
    pub revision: u64,
    pub variables: Vec<VariableRecord>,
    /// Names that matched no variable, each with the closest names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_found: Vec<NotFound>,
    /// Variables left out to keep the answer within its budget, to read in
    /// another call.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub omitted: Vec<String>,
    /// Why the records carry no behavior, when the model does not simulate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behavior_unavailable: Option<String>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct NotFound {
    pub name: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
}

/// What a variable is, as a modeler would say it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum VariableKind {
    Stock,
    Flow,
    /// A computed auxiliary.
    Auxiliary,
    /// An auxiliary whose equation is a number.
    Constant,
    /// A standalone table other equations call.
    Lookup,
    /// An instance of another model.
    Module,
}

impl VariableKind {
    pub const ALL: [VariableKind; 6] = [
        VariableKind::Stock,
        VariableKind::Flow,
        VariableKind::Auxiliary,
        VariableKind::Constant,
        VariableKind::Lookup,
        VariableKind::Module,
    ];

    pub(crate) fn of(var: &Variable) -> VariableKind {
        match var {
            Variable::Stock(_) => VariableKind::Stock,
            Variable::Flow(_) => VariableKind::Flow,
            Variable::Aux(aux) => match aux_kind(aux) {
                AuxKind::Lookup(_) => VariableKind::Lookup,
                AuxKind::Constant(_) => VariableKind::Constant,
                AuxKind::Computed => VariableKind::Auxiliary,
            },
            Variable::Module(_) => VariableKind::Module,
        }
    }
}

/// A link's sign as system dynamics writes it: `+` when the target moves with
/// the source, `-` when against it, `?` when the engine cannot tell from the
/// equation.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub enum LinkPolarityName {
    #[serde(rename = "+")]
    Positive,
    #[serde(rename = "-")]
    Negative,
    #[serde(rename = "?")]
    Unknown,
}

impl From<LinkPolarity> for LinkPolarityName {
    fn from(polarity: LinkPolarity) -> LinkPolarityName {
        match polarity {
            LinkPolarity::Positive => LinkPolarityName::Positive,
            LinkPolarity::Negative => LinkPolarityName::Negative,
            LinkPolarity::Unknown => LinkPolarityName::Unknown,
        }
    }
}

/// One end of a causal link, from the variable a record describes.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LinkRef {
    pub name: String,
    pub polarity: LinkPolarityName,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ElementEquation {
    pub element: String,
    pub equation: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum LookupKind {
    /// Interpolates between points and holds the end values beyond them.
    Continuous,
    /// Interpolates between points and extends the end segments beyond them.
    Extrapolate,
    /// Steps from point to point.
    Discrete,
}

/// A graphical function's points.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct Lookup {
    pub kind: LookupKind,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    /// How many more points the table has than the 64 listed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_points: Option<usize>,
}

/// The wiring of a module instance.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct ModuleRecord {
    /// The model the module instantiates.
    pub model: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<super::ModuleInputOutline>,
}

/// Everything a variable's definition holds, and its causal neighbors.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct VariableRecord {
    pub name: String,
    /// The element read, when one was named (`population[north]`): the
    /// record's equation or initial value is that element's.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element: Option<String>,
    pub kind: VariableKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// The documentation, cut at 480 characters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// A flow's or auxiliary's equation; absent for per-element equations,
    /// which `elements` lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equation: Option<String>,
    /// A stock's initial value; absent for per-element initial values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<String>,
    /// Per-element equations (or a stock's per-element initial values): at
    /// most 24, each cut at 240 characters and 2,400 in all; name an element
    /// to read its equation whole.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub elements: Vec<ElementEquation>,
    /// How many more per-element equations there are than `elements` lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_elements: Option<usize>,
    /// The equation of every element `elements` does not list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub other_elements: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lookup: Option<Lookup>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub non_negative: bool,
    /// A stock's inflows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inflows: Vec<String>,
    /// A stock's outflows.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outflows: Vec<String>,
    /// The stocks a flow fills.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub fills: Vec<String>,
    /// The stocks a flow drains.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub drains: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<ModuleRecord>,
    /// The variables this one reads, each link with its polarity, at most 24.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<LinkRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_inputs: Option<usize>,
    /// The variables that read this one, each link with its polarity, at
    /// most 24.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<LinkRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_readers: Option<usize>,
    /// This variable's diagnostics, each under the id `read_model` gives it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticReport>,
    /// What it did in the current run: a scalar variable's start and end,
    /// extremes and behavior mode (`read_behavior` has an arrayed one's, and
    /// more).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behavior: Option<SeriesCore>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct FindVariablesInput {
    /// A name, part of one, a misspelling, or a few words of description.
    pub phrase: String,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct FindVariablesOutput {
    pub revision: u64,
    /// Up to 10 variables, closest first.
    pub matches: Vec<VariableMatch>,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct VariableMatch {
    pub name: String,
    pub kind: VariableKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub units: Option<String>,
    /// How closely the phrase matches, from 0 to 1.
    pub score: f64,
}

/// Answer `read_variables`.
pub(crate) fn read_variables(
    session: &mut Session,
    ws: &mut Workspace<'_>,
    input: ReadVariablesInput,
) -> Result<ReadVariablesOutput, ToolError> {
    if input.names.is_empty() {
        return Err(ToolError::new("name at least one variable to read"));
    }
    if input.names.len() > MAX_READ_VARIABLES {
        return Err(ToolError::new(format!(
            "at most {MAX_READ_VARIABLES} variables per call (this call names {}); read the rest in another call",
            input.names.len()
        )));
    }
    let resolved = resolve_model(ws.project, ws.db, &session.model_name)?;
    let model = resolved.model;

    let mut records: Vec<(&Variable, Option<String>)> = Vec::new();
    let mut not_found = Vec::new();
    let mut read: BTreeSet<(String, Option<String>)> = BTreeSet::new();
    for name in &input.names {
        match resolve_read(ws.project, model, name) {
            Ok((var, element)) => {
                let key = (
                    crate::canonicalize(var.get_ident()).into_owned(),
                    element.clone(),
                );
                if read.insert(key) {
                    records.push((var, element));
                }
            }
            Err(suggestions) => not_found.push(NotFound {
                name: name.clone(),
                suggestions,
            }),
        }
    }

    let diagnostics = session.evidence.report_diagnostics(ws, &resolved);
    // A model that does not simulate still has definitions to read; its
    // records say why they carry no behavior.
    let (current, behavior_unavailable) = match session.runs.current(ws, model) {
        Ok(run) => (Some(run), None),
        Err(refusal) if refusal.is_interrupted() => return Err(refusal),
        Err(refusal) => (None, Some(refusal.error)),
    };
    let links = crate::analysis::model_links(
        &*ws.db,
        resolved.source_model,
        resolved.source_project,
        None,
        false,
    );

    let variables: Vec<VariableRecord> = records
        .into_iter()
        .map(|(var, element)| {
            let canonical = crate::canonicalize(var.get_ident()).into_owned();
            let neighbor = |name: &str, polarity: LinkPolarity| LinkRef {
                name: display_name(model, name),
                polarity: polarity.into(),
            };
            let mut record = definition(model, var);
            if let Some(element) = &element {
                narrow_to_element(&mut record, var, element);
            }
            let mut inputs: Vec<LinkRef> = links
                .iter()
                .filter(|l| l.to == canonical)
                .map(|l| neighbor(&l.from, l.polarity))
                .collect();
            let mut readers: Vec<LinkRef> = links
                .iter()
                .filter(|l| l.from == canonical)
                .map(|l| neighbor(&l.to, l.polarity))
                .collect();
            inputs.sort_by(|a, b| a.name.cmp(&b.name));
            readers.sort_by(|a, b| a.name.cmp(&b.name));
            (record.inputs, record.more_inputs) = capped(inputs, MAX_RECORD_LINKS);
            (record.readers, record.more_readers) = capped(readers, MAX_RECORD_LINKS);
            record.diagnostics = diagnostics
                .iter()
                .filter(|d| {
                    d.variable
                        .as_deref()
                        .is_some_and(|v| crate::canonicalize(v) == canonical)
                })
                .map(|d| DiagnosticReport {
                    variable: None,
                    ..d.clone()
                })
                .collect();
            // A scalar's behavior, or a named element's.
            record.behavior = current.as_ref().and_then(|run| {
                let (series, omitted) =
                    keyed_series_upto(run, model, var.get_ident(), element.as_deref(), 1);
                let scalar_or_element = record.dimensions.is_empty() || element.is_some();
                match series.as_slice() {
                    [series] if omitted == 0 && scalar_or_element => {
                        let scale = scale_in_run(&run.results, model, &series.key);
                        Some(SeriesCore::at(&run.times(), &series.values, scale))
                    }
                    _ => None,
                }
            });
            record
        })
        .collect();

    let mut output = ReadVariablesOutput {
        revision: ws.revision,
        variables,
        not_found,
        omitted: vec![],
        behavior_unavailable,
    };
    fit(&mut output, session.outline_budget);
    Ok(output)
}

/// What `name` reads: a variable, or with a subscript one element of an
/// arrayed variable; the closest names when it names neither.
fn resolve_read<'a>(
    project: &datamodel::Project,
    model: &'a datamodel::Model,
    name: &str,
) -> Result<(&'a Variable, Option<String>), Vec<String>> {
    if let Some(var) = model.get_variable(name) {
        return Ok((var, None));
    }
    let Some((base, subscripts)) = names::split_subscript(name) else {
        return names::resolve(model, name).map(|var| (var, None));
    };
    let var = names::resolve(model, base)?;
    let dims = var.get_equation().map(dimensions).unwrap_or_default();
    names::resolve_element(project, &dims, &subscripts)
        .map(|element| (var, Some(element)))
        .map_err(|reason| vec![format!("{}: {reason}", var.get_ident())])
}

/// Narrow a record of an arrayed variable to one element: its equation (a
/// stock's initial value) is that element's.
fn narrow_to_element(record: &mut VariableRecord, var: &Variable, element: &str) {
    let key = |text: &str| crate::canonicalize(&text.replace(' ', "")).into_owned();
    let text = match var.get_equation() {
        Some(Equation::Scalar(text)) | Some(Equation::ApplyToAll(_, text)) => Some(text.clone()),
        Some(Equation::Arrayed(_, elements, default, _)) => elements
            .iter()
            .find(|(name, ..)| key(name) == key(element))
            .map(|(_, text, _, _)| text.clone())
            .or_else(|| default.clone()),
        None => None,
    };
    record.element = Some(element.to_string());
    record.elements.clear();
    record.more_elements = None;
    record.other_elements = None;
    if matches!(var, Variable::Stock(_)) {
        record.initial = text;
    } else if record.kind != VariableKind::Lookup {
        record.equation = text;
    }
}

/// The per-element equations a record lists: at most 24, each cut at 240
/// characters, and at most 2,400 characters in all (the first always), with
/// how many more there are.
fn listed_elements(elements: Vec<ElementEquation>) -> (Vec<ElementEquation>, Option<usize>) {
    let total = elements.len();
    let mut listed = Vec::new();
    let mut chars = 0;
    for element in elements.into_iter().take(MAX_RECORD_ELEMENTS) {
        let equation = window(&element.equation, 0, 0, super::evidence::QUOTE_CHARS);
        chars += equation.chars().count();
        if !listed.is_empty() && chars > MAX_ELEMENT_TEXT_CHARS {
            break;
        }
        listed.push(ElementEquation {
            element: element.element,
            equation,
        });
    }
    let more = total - listed.len();
    (listed, (more > 0).then_some(more))
}

/// The first `limit` items, and how many more there were.
fn capped<T>(mut items: Vec<T>, limit: usize) -> (Vec<T>, Option<usize>) {
    let more = items.len().saturating_sub(limit);
    items.truncate(limit);
    (items, (more > 0).then_some(more))
}

/// Keep an answer within `budget` bytes: the records that fit, in order, with
/// the rest named in `omitted`. The first record is always kept, since its
/// own caps bound it.
fn fit(output: &mut ReadVariablesOutput, budget: usize) {
    let len = |output: &ReadVariablesOutput| {
        serde_json::to_string(output)
            .expect("records serialize")
            .len()
    };
    while output.variables.len() > 1 && len(output) > budget {
        let record = output.variables.pop().expect("more than one record");
        let name = match &record.element {
            Some(element) => format!("{}[{element}]", record.name),
            None => record.name,
        };
        output.omitted.insert(0, name);
    }
}

/// A variable's record from its definition alone; `inputs`, `readers` and
/// `diagnostics` are the caller's to fill.
fn definition(model: &datamodel::Model, var: &Variable) -> VariableRecord {
    let mut record = VariableRecord {
        name: var.get_ident().to_string(),
        element: None,
        kind: VariableKind::of(var),
        units: var.get_units().cloned().filter(|u| !u.is_empty()),
        documentation: None,
        equation: None,
        initial: None,
        dimensions: vec![],
        elements: vec![],
        more_elements: None,
        other_elements: None,
        lookup: None,
        non_negative: false,
        inflows: vec![],
        outflows: vec![],
        fills: vec![],
        drains: vec![],
        module: None,
        inputs: vec![],
        more_inputs: None,
        readers: vec![],
        more_readers: None,
        diagnostics: vec![],
        behavior: None,
    };
    let documentation = |text: &str| {
        Some(window(text.trim(), 0, 0, MAX_DOCUMENTATION_CHARS)).filter(|t| !t.is_empty())
    };
    match var {
        Variable::Stock(stock) => {
            record.documentation = documentation(&stock.documentation);
            let (initial, elements, others) = split_equation(&stock.equation);
            record.initial = initial;
            (record.elements, record.more_elements) = listed_elements(elements);
            record.other_elements = others;
            record.dimensions = dimensions(&stock.equation);
            record.non_negative = stock.compat.non_negative;
            record.inflows = datamodel::distinct_stock_flows(&stock.inflows).flows;
            record.outflows = datamodel::distinct_stock_flows(&stock.outflows).flows;
        }
        Variable::Flow(flow) => {
            record.documentation = documentation(&flow.documentation);
            let (equation, elements, others) = split_equation(&flow.equation);
            record.equation = equation;
            (record.elements, record.more_elements) = listed_elements(elements);
            record.other_elements = others;
            record.dimensions = dimensions(&flow.equation);
            record.lookup = flow.gf.as_ref().map(lookup);
            record.non_negative = flow.compat.non_negative;
            let flow_name = crate::canonicalize(&flow.ident);
            for other in &model.variables {
                if let Variable::Stock(stock) = other {
                    let names = |flows: &[String]| {
                        datamodel::distinct_stock_flows(flows)
                            .flows
                            .iter()
                            .any(|f| crate::canonicalize(f) == flow_name)
                    };
                    if names(&stock.inflows) {
                        record.fills.push(stock.ident.clone());
                    }
                    if names(&stock.outflows) {
                        record.drains.push(stock.ident.clone());
                    }
                }
            }
        }
        Variable::Aux(aux) => {
            record.documentation = documentation(&aux.documentation);
            let (equation, elements, others) = split_equation(&aux.equation);
            // A lookup-only table's equation is a placeholder, not a formula.
            if record.kind != VariableKind::Lookup {
                record.equation = equation;
            }
            (record.elements, record.more_elements) = listed_elements(elements);
            record.other_elements = others;
            record.dimensions = dimensions(&aux.equation);
            record.lookup = aux.gf.as_ref().map(lookup);
            record.non_negative = aux.compat.non_negative;
        }
        Variable::Module(module) => {
            record.documentation = documentation(&module.documentation);
            record.module = Some(ModuleRecord {
                model: module.model_name.clone(),
                inputs: module
                    .references
                    .iter()
                    .map(|r| super::ModuleInputOutline {
                        from: r.src.clone(),
                        to: r.dst.clone(),
                    })
                    .collect(),
            });
        }
    }
    record
}

/// An equation as a record holds it: the one text of a scalar or
/// apply-to-all equation, or each element's text and the EXCEPT default.
fn split_equation(equation: &Equation) -> (Option<String>, Vec<ElementEquation>, Option<String>) {
    match equation {
        Equation::Scalar(text) | Equation::ApplyToAll(_, text) => {
            (Some(text.clone()), vec![], None)
        }
        Equation::Arrayed(_, elements, default, _) => (
            None,
            elements
                .iter()
                .map(|(element, text, _, _)| ElementEquation {
                    element: element.clone(),
                    equation: text.clone(),
                })
                .collect(),
            default.clone(),
        ),
    }
}

fn lookup(gf: &datamodel::GraphicalFunction) -> Lookup {
    let x = match &gf.x_points {
        Some(points) => points.clone(),
        // Evenly spaced over the x scale, as the engine reads a table
        // without x points.
        None => {
            let n = gf.y_points.len();
            (0..n)
                .map(|i| {
                    if n <= 1 {
                        gf.x_scale.min
                    } else {
                        gf.x_scale.min
                            + (gf.x_scale.max - gf.x_scale.min) * i as f64 / (n - 1) as f64
                    }
                })
                .collect()
        }
    };
    let (x, more_points) = capped(x, MAX_LOOKUP_POINTS);
    let (y, _) = capped(gf.y_points.clone(), MAX_LOOKUP_POINTS);
    Lookup {
        kind: match gf.kind {
            datamodel::GraphicalFunctionKind::Continuous => LookupKind::Continuous,
            datamodel::GraphicalFunctionKind::Extrapolate => LookupKind::Extrapolate,
            datamodel::GraphicalFunctionKind::Discrete => LookupKind::Discrete,
        },
        x,
        y,
        more_points,
    }
}

/// Answer `find_variables`.
pub(crate) fn find_variables(
    ws: &Workspace<'_>,
    model_name: &str,
    input: FindVariablesInput,
) -> Result<FindVariablesOutput, ToolError> {
    if input.phrase.trim().is_empty() {
        return Err(ToolError::new(
            "give a phrase to search for: a name, part of one, or a few words of description",
        ));
    }
    let resolved = resolve_model(ws.project, ws.db, model_name)?;
    let matches = names::rank(resolved.model, &input.phrase)
        .into_iter()
        .filter(|(score, _)| *score >= MATCH_THRESHOLD)
        .take(MAX_MATCHES)
        .map(|(score, var)| VariableMatch {
            name: var.get_ident().to_string(),
            kind: VariableKind::of(var),
            units: var.get_units().cloned().filter(|u| !u.is_empty()),
            score: (score * 100.0).round() / 100.0,
        })
        .collect();
    Ok(FindVariablesOutput {
        revision: ws.revision,
        matches,
    })
}

#[cfg(test)]
#[path = "variables_tests.rs"]
mod tests;
