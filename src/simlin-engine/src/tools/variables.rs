// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! `read_variables`, a variable's whole record, and `find_variables`, a search
//! by name or description.
//!
//! A record carries everything a variable's definition holds -- its equation
//! or initial value, per-element equations, lookup points, flows, module
//! wiring -- and where it sits in the model's structure: the variables it reads
//! and the variables that read it ([`crate::analysis::model_reads`], macro and
//! module internals collapsed into the reads between the variables a modeler
//! wrote), each with the polarity of its causal link, and marked when it is
//! made only as the model starts.
//!
//! An answer keeps to the session's byte budget. A record lists at most 24
//! per-element equations (each cut at 240 characters, 2,400 in all), 24
//! inputs, 24 readers and 64 lookup points, with how many more there are, and
//! cuts its documentation at 480 characters; an element's own equation is
//! read whole by naming it (`population[north]`). Records that do not fit are
//! named for another call. A record that does not fit even alone is cut
//! ([`Cut`]) -- its readers, then its inputs, diagnostics, per-element
//! equations, lookup points and documentation, each counted, and last its
//! equation, which then says it was cut -- and an answer that still does not
//! fit is refused: an answer over its budget is never given.

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
    /// The closest names of variables, when no variable has the name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggestions: Vec<String>,
    /// Why not, when the variable exists and what was asked of it does not:
    /// an element it does not have, or a subscript on a variable with none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
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

/// One end of a read, from the variable a record describes.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "camelCase")]
pub struct LinkRef {
    pub name: String,
    /// The sign of the causal link; `?` for a read that is no causal link (a
    /// table looked up, a read made only at the start).
    pub polarity: LinkPolarityName,
    /// Set when the read is made only as the model starts (a stock's initial
    /// value, an `INIT` argument): it sets where the reader starts and does
    /// not move it afterwards, so it is on no feedback loop.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub start_only: bool,
    /// The stock's or flow's option the read is made in, when it is one: the
    /// engine reads it there, outside the reader's equation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option: Option<StockOptionName>,
    /// Set when the read is written in an equation the engine cannot compile
    /// (an unknown function, a wrong argument count): the reader reads it
    /// once that equation compiles.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unchecked: bool,
}

/// A stock's or a flow's option, as a read record names it.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum StockOptionName {
    /// A conveyor's transit time.
    TransitTime,
    /// A conveyor's capacity.
    Capacity,
    /// A conveyor's inflow limit.
    InflowLimit,
    /// A conveyor's sample condition.
    Sample,
    /// A conveyor's arrest condition.
    Arrest,
    /// A leak flow's fraction.
    LeakFraction,
    /// Where a leak's zone starts.
    LeakZoneStart,
    /// Where a leak's zone ends.
    LeakZoneEnd,
}

impl From<crate::datamodel::StockOption> for StockOptionName {
    fn from(option: crate::datamodel::StockOption) -> StockOptionName {
        use crate::datamodel::StockOption;
        match option {
            StockOption::TransitTime => StockOptionName::TransitTime,
            StockOption::Capacity => StockOptionName::Capacity,
            StockOption::InflowLimit => StockOptionName::InflowLimit,
            StockOption::Sample => StockOptionName::Sample,
            StockOption::Arrest => StockOptionName::Arrest,
            StockOption::LeakFraction => StockOptionName::LeakFraction,
            StockOption::LeakZoneStart => StockOptionName::LeakZoneStart,
            StockOption::LeakZoneEnd => StockOptionName::LeakZoneEnd,
        }
    }
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
    /// How many more points the table has than are listed (at most 64).
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
    /// Set when the equation, the initial value or `otherElements` was cut
    /// (it then ends in an ellipsis) because the record did not fit one
    /// answer whole.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
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
    /// The variables this one reads, each with its link's polarity, at most 24.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<LinkRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_inputs: Option<usize>,
    /// The variables that read this one, each with its link's polarity, at
    /// most 24.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub readers: Vec<LinkRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_readers: Option<usize>,
    /// This variable's diagnostics, each under the id `read_model` gives it.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<DiagnosticReport>,
    /// How many more diagnostics it has than `diagnostics` lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub more_diagnostics: Option<usize>,
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
    /// A name, part of one (the starts of its words: "pop gr" finds
    /// "population growth"), a misspelling, or a few words of description;
    /// at most 256 characters. The name itself ranks first, then names whose
    /// words the phrase starts, then names and descriptions like it; a
    /// phrase under four characters finds only the first two.
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
    /// Matches left out, the least close, to keep the answer within its
    /// budget: names too long to list them all.
    #[serde(skip_serializing_if = "super::is_zero")]
    pub omitted: usize,
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
        match names::resolve_reference(ws.project, model, name) {
            Ok((var, element)) => {
                let key = (
                    crate::canonicalize(var.get_ident()).into_owned(),
                    element.clone(),
                );
                if read.insert(key) {
                    records.push((var, element));
                }
            }
            Err(unresolved) => not_found.push(NotFound {
                name: super::evidence::echo(name),
                suggestions: unresolved.suggestions,
                reason: unresolved.reason,
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
    let reads =
        crate::analysis::model_reads(&*ws.db, resolved.source_model, resolved.source_project);

    let variables: Vec<VariableRecord> = records
        .into_iter()
        .map(|(var, element)| {
            let canonical = crate::canonicalize(var.get_ident()).into_owned();
            let neighbor = |name: &str, read: &crate::analysis::ModelRead| LinkRef {
                name: display_name(model, name),
                polarity: read.polarity.into(),
                start_only: read.start_only,
                option: read.option.map(StockOptionName::from),
                unchecked: read.unchecked,
            };
            let mut record = definition(model, var);
            if let Some(element) = &element {
                narrow_to_element(&mut record, var, element);
            }
            let mut inputs: Vec<LinkRef> = reads
                .iter()
                .filter(|r| r.to == canonical)
                .map(|r| neighbor(&r.from, r))
                .collect();
            let mut readers: Vec<LinkRef> = reads
                .iter()
                .filter(|r| r.from == canonical)
                .map(|r| neighbor(&r.to, r))
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
            // A scalar's behavior, or a named element's. A constant has
            // none worth the words: it is its value throughout.
            let behaves = record.kind != VariableKind::Constant;
            record.behavior = current.as_ref().filter(|_| behaves).and_then(|run| {
                let (series, omitted) =
                    keyed_series_upto(run, model, var.get_ident(), element.as_deref(), 1);
                let scalar_or_element = record.dimensions.is_empty() || element.is_some();
                match series.as_slice() {
                    [series] if omitted == 0 && scalar_or_element => {
                        let scale = scale_in_run(&run.results, model, &run.plan, &series.key);
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
    fit(&mut output, session.outline_budget)?;
    Ok(output)
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
/// the rest named in `omitted`; then, when the first record does not fit
/// alone, that record cut ([`VariableRecord::shed`]) until it does. An answer
/// that is over the budget with nothing left to cut -- names, units or wiring
/// too long for one answer -- is refused rather than given.
fn fit(output: &mut ReadVariablesOutput, budget: usize) -> Result<(), ToolError> {
    let fits = super::fit(output, budget, |output| {
        if output.variables.len() > 1 {
            let record = output.variables.pop().expect("more than one record");
            let name = match &record.element {
                Some(element) => format!("{}[{element}]", record.name),
                None => record.name,
            };
            output.omitted.insert(0, name);
            true
        } else {
            output
                .variables
                .first_mut()
                .is_some_and(VariableRecord::shed)
        }
    });
    if fits {
        Ok(())
    } else {
        Err(ToolError::new(format!(
            "the answer does not fit one answer even with its lists and long text cut ({} \
             bytes, and an answer holds {budget}): the names, units, flows or module wiring \
             it must carry are too long. Read fewer variables in a call.",
            super::json_len(output)
        )))
    }
}

/// The fewest characters a cut equation keeps.
pub(crate) const MIN_CUT_CHARS: usize = super::evidence::QUOTE_CHARS;

/// What a record that does not fit an answer alone leaves out, least
/// important first: who reads it and what it reads are a call away
/// (`find_variables`, the readers' own records), its definition is what the
/// call was for, so its equation goes last.
#[derive(Clone, Copy)]
pub(crate) enum Cut {
    Readers,
    Inputs,
    Diagnostics,
    Elements,
    LookupPoints,
    Documentation,
    /// The equation, the initial value and the other elements' equation.
    Text,
}

impl Cut {
    pub(crate) const ALL: [Cut; 7] = [
        Cut::Readers,
        Cut::Inputs,
        Cut::Diagnostics,
        Cut::Elements,
        Cut::LookupPoints,
        Cut::Documentation,
        Cut::Text,
    ];
}

impl VariableRecord {
    /// Leave out the least important thing the record still carries
    /// ([`Cut::ALL`], in order); false when nothing is left to leave out.
    fn shed(&mut self) -> bool {
        Cut::ALL.into_iter().any(|cut| self.cut(cut))
    }

    /// Leave out part of what `cut` names, and whether there was any: half
    /// of a list (all of a list of one), counted beside it; the
    /// documentation; half of each long text, no shorter than
    /// [`MIN_CUT_CHARS`], which marks the record `truncated`.
    fn cut(&mut self, cut: Cut) -> bool {
        fn halve<T>(list: &mut Vec<T>, more: &mut Option<usize>) -> bool {
            if list.is_empty() {
                return false;
            }
            let keep = list.len() / 2;
            *more = Some(more.unwrap_or(0) + list.len() - keep);
            list.truncate(keep);
            true
        }
        fn shorten(text: &mut Option<String>) -> bool {
            let Some(text) = text else { return false };
            let chars = text.chars().count();
            // A cut text ends in an ellipsis, so it is one character longer
            // than what it keeps: a text at the floor, cut already, is left.
            let keep = (chars / 2).max(MIN_CUT_CHARS);
            if keep + 1 >= chars {
                return false;
            }
            *text = window(text, 0, 0, keep);
            true
        }
        match cut {
            Cut::Readers => halve(&mut self.readers, &mut self.more_readers),
            Cut::Inputs => halve(&mut self.inputs, &mut self.more_inputs),
            Cut::Diagnostics => halve(&mut self.diagnostics, &mut self.more_diagnostics),
            Cut::Elements => halve(&mut self.elements, &mut self.more_elements),
            Cut::LookupPoints => self.lookup.as_mut().is_some_and(|lookup| {
                let cut = halve(&mut lookup.x, &mut lookup.more_points);
                lookup.y.truncate(lookup.x.len());
                cut
            }),
            Cut::Documentation => self.documentation.take().is_some(),
            Cut::Text => {
                // Every text is cut in one step, not the first alone.
                let cut = [
                    shorten(&mut self.equation),
                    shorten(&mut self.initial),
                    shorten(&mut self.other_elements),
                ]
                .contains(&true);
                self.truncated |= cut;
                cut
            }
        }
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
        truncated: false,
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
        more_diagnostics: None,
        behavior: None,
    };
    let documentation = |text: &str| {
        Some(window(&tidy(text), 0, 0, MAX_DOCUMENTATION_CHARS)).filter(|t| !t.is_empty())
    };
    // Flows as the model names them, the spelling every other name in the
    // answer has, whatever spelling the stock's own list holds.
    let named = |flows: Vec<String>| -> Vec<String> {
        flows.iter().map(|flow| display_name(model, flow)).collect()
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
            record.inflows = named(datamodel::distinct_stock_flows(&stock.inflows).flows);
            record.outflows = named(datamodel::distinct_stock_flows(&stock.outflows).flows);
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

/// Documentation as one run of prose: a line an importer left broken in the
/// middle of a sentence is joined, and runs of spaces, tabs and line breaks
/// are one space, except that a blank line between paragraphs is one line
/// break.
///
/// A Vensim model file wraps a long comment with a backslash at the end of
/// each line and indents the next (every wrapped comment of
/// `test/xmutil_test_models/C-LEARN v77 for Vensim.mdl`, a file Vensim
/// wrote, is so); the MDL reader keeps the comment as written, so the
/// backslash and the indentation reach here. That the backslash is a
/// continuation and no part of the comment is the file's evidence, unverified
/// against Vensim's documentation.
pub(crate) fn tidy(text: &str) -> String {
    let joined = text.replace("\r\n", "\n").replace("\\\n", " ");
    joined
        .split("\n\n")
        .map(|paragraph| paragraph.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|paragraph| !paragraph.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
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
    budget: usize,
) -> Result<FindVariablesOutput, ToolError> {
    if input.phrase.trim().is_empty() {
        return Err(ToolError::new(
            "give a phrase to search for: a name, part of one, or a few words of description",
        ));
    }
    let length = input.phrase.chars().count();
    if length > names::MAX_QUERY_CHARS {
        return Err(ToolError::new(format!(
            "a phrase to search for is at most {} characters (this one has {length}): give a \
             name, part of one, or a few words of description",
            names::MAX_QUERY_CHARS
        )));
    }
    let resolved = resolve_model(ws.project, ws.db, model_name)?;
    let matches = names::rank(resolved.model, &input.phrase)
        .into_iter()
        .filter(|(score, _)| *score >= MATCH_THRESHOLD)
        .take(MAX_MATCHES)
        .map(|(score, var)| VariableMatch {
            name: var.get_ident().to_string(),
            kind: VariableKind::of(var),
            // Units are read, never passed back: a long string is cut.
            units: var
                .get_units()
                .filter(|u| !u.is_empty())
                .map(|units| super::evidence::window(units, 0, 0, super::evidence::QUOTE_CHARS)),
            score: (score * 100.0).round() / 100.0,
        })
        .collect();
    let mut output = FindVariablesOutput {
        revision: ws.revision,
        matches,
        omitted: 0,
    };
    // A name is what the agent passes back, so it is never cut: the least
    // close matches are left out, counted, until the answer fits.
    super::fit(&mut output, budget, |output| {
        if output.matches.len() <= 1 {
            return false;
        }
        output.matches.pop();
        output.omitted += 1;
        true
    });
    Ok(output)
}

#[cfg(test)]
#[path = "variables_tests.rs"]
mod tests;
