// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Whether saving a project in a format keeps what its model means
//! ([`check_save`]).
//!
//! The project is saved, the save is read back as a host would open it, and
//! the model it holds is compared with the project's two ways. When the
//! check cannot be sure a save keeps the meaning, it reports a change: a
//! refused save can be saved elsewhere, and a silently different model
//! cannot be taken back.
//!
//! - **Structure** is the definition, and it does not depend on what one
//!   run reaches or on whether the model runs at all. It compares:
//!   - the simulation specs, the project's and each model's own
//!   - each dimension's elements (named or numbered), parent, and the element
//!     pairs its mappings relate
//!   - each variable's kind, dimensions and elements
//!   - each element's equation, in its canonical form
//!   - each variable's `:EXCEPT:` default, where the compiler applies it, and
//!     whether the variable is a table alone
//!   - initial values, graphical functions as the compiler reads them, flows
//!     and module inputs
//!   - the markings that change a simulation (a stock's or flow's
//!     non-negative, conveyor, queue, leak, data source, module input), with
//!     what each carries
//!
//!   The walk is exhaustive by construction: every `datamodel` type it reads
//!   is destructured field by field, so a field added to one does not compile
//!   here until it is compared or named as no part of the meaning.
//!
//!   Two equations are the same when their canonical forms are
//!   ([`crate::ast::CanonicalEqn`] under `Aliases::Engine`): names that
//!   canonicalize alike, numbers of one value, and the calls the engine
//!   itself rewrites (`MODULO(a, b)` and `a mod b`, `PI()` and its value, a
//!   builtin and its alias). Any other difference is a change, even one that
//!   computes the same.
//! - **Behaviour** is what the definition does today. When the project
//!   simulates, the save must simulate every variable's series to the same
//!   values (NaN matching NaN, and -0 matching 0). That catches a function a
//!   writer respells with the same text but other semantics, which no
//!   structure shows. It sees only the run the specs ask for: a pulse after
//!   FINAL TIME, or a branch the run never takes, looks the same either way.
//!   The compiler's own helper columns are not compared on their own: what
//!   they hold shows in the variables they feed. When the project does not
//!   simulate, the save must report the same errors.
//!
//! Each change says which it is ([`ChangeKind`]): whether the save's results
//! differ, or only its definition does.
//!
//! The check compares the save as THIS engine reads it back. A writer and a
//! reader that agree with each other and not with the tool the format belongs
//! to (a function both map to one of another meaning) are a fixed point it
//! cannot see through.
//!
//! Meaning is what the model computes. Units, documentation, views, groups
//! and loop names are not. Nor is where a control variable lives: a model
//! that keeps its specs as variables (`FINAL TIME`, `SAVEPER`) and a save
//! that keeps them as the simulation specs mean the same when the values
//! agree.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io::BufReader;

use crate::ast::{Aliases, CanonicalEqn, Expr0, UnaryOp};
use crate::buffa::Message;
use crate::builtins::UntypedBuiltinFn;
use crate::common::{CanonicalElementName, Result, canonicalize};
use crate::data_provider::DataProvider;
use crate::datamodel::{
    Aux, Compat, Conveyor, DataSource, Dimension, DimensionElements, DimensionMapping, Dt,
    Equation, Flow, GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale, Leakage,
    MacroSpec, Model, Module, ModuleReference, Project, Queue, SimMethod, SimSpecs, SpreadFlow,
    Stock, Variable,
};
use crate::errors::{code_and_reason, join_quoted_names};

#[cfg(test)]
#[path = "save_check_tests.rs"]
mod tests;

/// The formats a project can be saved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SaveFormat {
    Mdl,
    Xmile,
    Json,
    SdaiJson,
    Protobuf,
}

impl SaveFormat {
    /// The format's name, as a person reads it.
    pub fn name(self) -> &'static str {
        match self {
            SaveFormat::Mdl => "Vensim MDL",
            SaveFormat::Xmile => "XMILE",
            SaveFormat::Json => "JSON",
            SaveFormat::SdaiJson => "sd-ai JSON",
            SaveFormat::Protobuf => "protobuf",
        }
    }
}

/// Whether a change shows in what the save simulates. Either kind is a change
/// a save in place must not make; the kind only grades it, so a host can say
/// whether the results would differ.
///
/// When neither the project nor its save simulates, every change is
/// `Structure`: there are no results to compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The save's results differ from the project's: a series simulates
    /// differently, the two simulate over other specs, or only one of them
    /// simulates or opens. A marking this engine does not enforce (a stock's
    /// or flow's non-negative) that only one of them has is this kind too: a simulator
    /// that enforces it computes other results wherever it binds, and no run
    /// of this engine shows whether it does.
    Results,
    /// The save defines the model differently, though what it simulates is
    /// the same: an equation the run does not reach, an `:EXCEPT:` default
    /// that fills no element yet, or another error in a model that does not
    /// simulate.
    Structure,
}

/// One thing a save would change about what a project's model means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeaningChange {
    /// The model it is in; None for what the whole project holds (a
    /// dimension, the simulation specs, the simulation itself).
    pub model: Option<String>,
    /// The variable it is about, when one variable is.
    pub variable: Option<String>,
    /// Whether the save's results differ, or only its definition.
    pub kind: ChangeKind,
    /// What the save changes, as a sentence a person reads: `'demands1' is
    /// defined over dim2, not dim`.
    pub reason: String,
}

/// What saving `project` in `format` would change about what its model means:
/// the project is saved, the save is read back, and the two are compared
/// (see the module docs). Empty when the save keeps the model's meaning.
///
/// Err only when the format cannot hold the project at all, which the
/// format's writer reports (MDL holds one model, for example). A save that
/// does not read back is a change.
pub fn check_save(project: &Project, format: SaveFormat) -> Result<Vec<MeaningChange>> {
    check_save_with_data(project, format, None)
}

/// [`check_save`] for a project opened with a data provider. An MDL save is
/// read back with the same provider, so the external data it references
/// (`GET DIRECT DATA` and its kin) resolves as the project's did.
pub fn check_save_with_data(
    project: &Project,
    format: SaveFormat,
    data: Option<&dyn DataProvider>,
) -> Result<Vec<MeaningChange>> {
    Ok(match read_back(project, format, data)? {
        Ok(saved) => compare(project, &saved),
        Err(why) => vec![MeaningChange {
            model: None,
            variable: None,
            kind: ChangeKind::Results,
            reason: format!("the save does not read back: {why}"),
        }],
    })
}

/// What `saved` changes about what `original`'s model means: the comparison
/// [`check_save`] makes of a project and the project its save reads back as.
fn compare(original: &Project, saved: &Project) -> Vec<MeaningChange> {
    let (before, after) = (Meaning::of(original), Meaning::of(saved));
    let found = compare_structure(&before, &after);
    let runs = compare_runs(original, saved, &before.main_specs, &after.main_specs);
    settle(original, saved, found, runs)
}

/// The project a save of `project` in `format` reads back as: Err when the
/// format cannot hold it, Ok(Err) when the save does not read back.
fn read_back(
    project: &Project,
    format: SaveFormat,
    data: Option<&dyn DataProvider>,
) -> Result<std::result::Result<Project, String>> {
    Ok(read(&write(project, format)?, format, data))
}

/// The bytes a save of `project` in `format` holds; Err when the format's
/// writer refuses the project.
fn write(project: &Project, format: SaveFormat) -> Result<Vec<u8>> {
    let unwritable = |e: serde_json::Error| {
        crate::common::Error::new(
            crate::common::ErrorKind::Model,
            crate::common::ErrorCode::Generic,
            Some(e.to_string()),
        )
    };
    Ok(match format {
        SaveFormat::Mdl => crate::compat::to_mdl(project)?.into_bytes(),
        SaveFormat::Xmile => crate::compat::to_xmile(project)?.into_bytes(),
        SaveFormat::Json => {
            let json: crate::json::Project = project.clone().into();
            serde_json::to_vec(&json).map_err(unwritable)?
        }
        SaveFormat::SdaiJson => {
            let model: crate::json_sdai::SdaiModel = project.clone().into();
            serde_json::to_vec(&model).map_err(unwritable)?
        }
        SaveFormat::Protobuf => crate::serde::serialize(project)?.encode_to_vec(),
    })
}

/// The project a host opening `bytes` as `format` gets, or why it does not
/// open. `data` resolves an MDL file's external data references.
fn read(
    bytes: &[u8],
    format: SaveFormat,
    data: Option<&dyn DataProvider>,
) -> std::result::Result<Project, String> {
    match format {
        SaveFormat::Mdl => {
            let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            crate::compat::open_vensim_with_data(text, data).map_err(|e| e.to_string())
        }
        SaveFormat::Xmile => {
            crate::compat::open_xmile(&mut BufReader::new(bytes)).map_err(|e| e.to_string())
        }
        SaveFormat::Json => crate::json::Project::from_reader(bytes)
            .map(Into::into)
            .map_err(|e| e.to_string()),
        SaveFormat::SdaiJson => crate::json_sdai::SdaiModel::from_reader(bytes)
            .map(Into::into)
            .map_err(|e| e.to_string()),
        SaveFormat::Protobuf => crate::project_io::Project::decode_from_slice(bytes)
            .map(crate::serde::deserialize)
            .map_err(|e| e.to_string()),
    }
}

// ---- Structure: what a project defines ----
//
// Each `of` below destructures the `datamodel` type it reads with no `..`: a
// field added to that type does not compile here until it is given a place in
// the meaning or named, with the reason, as no part of it. The comparisons
// further down destructure the meaning types the same way, so a field that
// has a place is also compared. `save_check::tests::fields::every_field_is_compared_or_named`
// holds each field to its row.

/// Whether two values are the same number: `==`, with NaN the same as NaN
/// (so -0 is 0).
fn same_value(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

fn same_values(a: &[f64], b: &[f64]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(*x, *y))
}

/// A project's simulation specs, as numbers.
#[derive(Clone, Copy)]
struct Specs {
    start: f64,
    stop: f64,
    dt: f64,
    save_step: f64,
    method: SimMethod,
}

impl Specs {
    fn of(specs: &SimSpecs) -> Specs {
        let SimSpecs {
            start,
            stop,
            dt,
            save_step,
            sim_method,
            // The unit of time is a unit, and units are not meaning.
            time_units: _,
        } = specs;
        let value = |dt: &Dt| match *dt {
            Dt::Dt(v) => v,
            Dt::Reciprocal(v) => 1.0 / v,
        };
        let dt = value(dt);
        Specs {
            start: *start,
            stop: *stop,
            dt,
            // No save step saves at the time step.
            save_step: save_step.as_ref().map_or(dt, value),
            method: *sim_method,
        }
    }

    /// How `saved` differs from these specs, a sentence per difference; none
    /// when a run over either is a run over the other.
    fn differences(&self, saved: &Specs) -> Vec<String> {
        let Specs {
            start,
            stop,
            dt,
            save_step,
            method,
        } = *self;
        let mut differences = Vec::new();
        if !same_value(start, saved.start) || !same_value(stop, saved.stop) {
            differences.push(format!(
                "the save simulates from {} to {}, not from {start} to {stop}",
                saved.start, saved.stop
            ));
        }
        if !same_value(dt, saved.dt) {
            differences.push(format!("the save's time step is {}, not {dt}", saved.dt));
        }
        if !same_value(save_step, saved.save_step) {
            differences.push(format!(
                "the save's save step is {}, not {save_step}",
                saved.save_step
            ));
        }
        if method != saved.method {
            let name = |m: SimMethod| match m {
                SimMethod::Euler => "Euler's method",
                SimMethod::RungeKutta2 => "second-order Runge-Kutta",
                SimMethod::RungeKutta4 => "fourth-order Runge-Kutta",
            };
            differences.push(format!(
                "the save integrates with {}, not {}",
                name(saved.method),
                name(method)
            ));
        }
        differences
    }
}

/// A dimension's elements, in order, its parent, and where it maps them.
struct DimensionMeaning {
    name: String,
    /// Whether the elements are numbered (a size) rather than named. The
    /// engine keeps the two apart (only a named dimension maps), so a
    /// dimension of the names `1`, `2`, `3` is not the dimension of size 3.
    numbered: bool,
    elements: Vec<String>,
    parent: Option<String>,
    /// Each target dimension, with the pairs of elements the mapping relates
    /// (a positional mapping as the pairs it makes).
    mappings: BTreeMap<String, BTreeSet<(String, String)>>,
}

/// A dimension's element names, canonical and in order.
fn element_names(elements: &DimensionElements) -> Vec<String> {
    match elements {
        DimensionElements::Named(names) => {
            names.iter().map(|e| canonicalize(e).into_owned()).collect()
        }
        DimensionElements::Indexed(size) => (1..=*size).map(|i| i.to_string()).collect(),
    }
}

impl DimensionMeaning {
    fn of(dim: &Dimension, all: &[Dimension]) -> DimensionMeaning {
        let Dimension {
            name,
            elements,
            mappings,
            parent,
        } = dim;
        let names = element_names(elements);
        let mappings = mappings
            .iter()
            .map(|mapping| {
                let DimensionMapping {
                    target,
                    element_map,
                } = mapping;
                let target = canonicalize(target).into_owned();
                let pairs: BTreeSet<(String, String)> = if element_map.is_empty() {
                    // A positional mapping pairs the elements by position.
                    let target_elements = all
                        .iter()
                        .find(|d| canonicalize(&d.name) == target)
                        .map(|d| element_names(&d.elements))
                        .unwrap_or_default();
                    names.iter().cloned().zip(target_elements).collect()
                } else {
                    element_map
                        .iter()
                        .map(|(src, tgt)| {
                            (
                                canonicalize(src).into_owned(),
                                canonicalize(tgt).into_owned(),
                            )
                        })
                        .collect()
                };
                (target, pairs)
            })
            .collect();
        DimensionMeaning {
            name: name.clone(),
            numbered: matches!(elements, DimensionElements::Indexed(_)),
            elements: names,
            parent: parent.as_deref().map(|p| canonicalize(p).into_owned()),
            mappings,
        }
    }
}

/// An equation as the structure compares it: its canonical form, or, for
/// text that does not parse, the text (trimmed, case folded). An empty
/// equation is the empty text.
#[derive(Clone, PartialEq)]
enum EquationMeaning {
    Parsed(CanonicalEqn),
    Text(String),
}

impl EquationMeaning {
    fn of(text: &str, macros: &BTreeSet<String>) -> EquationMeaning {
        let is_macro = |name: &str| macros.contains(name);
        match CanonicalEqn::parse(
            text,
            Aliases::Engine {
                is_macro: &is_macro,
            },
        ) {
            Ok(Some(eqn)) => EquationMeaning::Parsed(eqn),
            _ => EquationMeaning::Text(text.trim().to_lowercase()),
        }
    }

    /// The equation of a variable that is its table alone: none, whether the
    /// file writes it empty or as the MDL sentinel `0+0`.
    fn none() -> EquationMeaning {
        EquationMeaning::Text(String::new())
    }

    /// The number a control variable's equation says its spec is: a literal,
    /// or (for the save step) the time step.
    fn control_value(&self, specs: &Specs) -> Option<f64> {
        let EquationMeaning::Parsed(eqn) = self else {
            return None;
        };
        match eqn.expr() {
            Expr0::Const(_, value, _) => Some(value.value()),
            Expr0::Op1(UnaryOp::Negative, inner, _) => match inner.as_ref() {
                Expr0::Const(_, value, _) => Some(-value.value()),
                _ => None,
            },
            Expr0::App(UntypedBuiltinFn(func, args), _)
                if func == "time_step" && args.is_empty() =>
            {
                Some(specs.dt)
            }
            _ => None,
        }
    }
}

/// An equation for a reason: its canonical spelling.
impl fmt::Display for EquationMeaning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EquationMeaning::Parsed(eqn) => eqn.fmt(f),
            EquationMeaning::Text(text) => f.write_str(text),
        }
    }
}

/// A graphical function as the compiler reads it (`variable::parse_table`):
/// a table that leaves out its x points is the table that writes out the x
/// points the compiler spreads over its scale. The scales are compared as
/// stored: the x scale is where omitted x points are spread, and a format
/// that holds a table holds its scales.
struct Table {
    kind: GraphicalFunctionKind,
    x: Vec<f64>,
    y: Vec<f64>,
    x_scale: (f64, f64),
    y_scale: (f64, f64),
}

/// Two tables are the same when the compiler reads the same numbers from
/// them, a NaN in both (a y point that is not a number) included.
impl PartialEq for Table {
    fn eq(&self, other: &Table) -> bool {
        let Table {
            kind,
            x,
            y,
            x_scale,
            y_scale,
        } = self;
        *kind == other.kind
            && same_values(x, &other.x)
            && same_values(y, &other.y)
            && same_values(
                &[x_scale.0, x_scale.1, y_scale.0, y_scale.1],
                &[
                    other.x_scale.0,
                    other.x_scale.1,
                    other.y_scale.0,
                    other.y_scale.1,
                ],
            )
    }
}

impl Table {
    fn of(gf: &GraphicalFunction) -> Table {
        let GraphicalFunction {
            kind,
            x_points,
            y_points,
            x_scale,
            y_scale,
        } = gf;
        let scale = |scale: &GraphicalFunctionScale| {
            let GraphicalFunctionScale { min, max } = scale;
            (*min, *max)
        };
        // A table the compiler refuses (an x that is not a number) is
        // compared as it is stored; the refusal shows in the errors.
        let x = match crate::variable::parse_table(Some(gf)) {
            Ok(Some(table)) => table.x,
            _ => x_points.clone().unwrap_or_default(),
        };
        Table {
            kind: *kind,
            x,
            y: y_points.clone(),
            x_scale: scale(x_scale),
            y_scale: scale(y_scale),
        }
    }
}

/// A conveyor's parameters, its expressions as equations.
#[derive(PartialEq)]
struct ConveyorMeaning {
    transit_time: EquationMeaning,
    capacity: Option<EquationMeaning>,
    inflow_limit: Option<EquationMeaning>,
    sample: Option<EquationMeaning>,
    arrest: Option<EquationMeaning>,
    discrete: bool,
    batch_integrity: bool,
    one_at_a_time: bool,
    exponential_leak: bool,
    ignore_earlier_zone_losses: bool,
}

impl ConveyorMeaning {
    fn of(conveyor: &Conveyor, macros: &BTreeSet<String>) -> ConveyorMeaning {
        let Conveyor {
            transit_time,
            capacity,
            inflow_limit,
            sample,
            arrest,
            discrete,
            batch_integrity,
            one_at_a_time,
            exponential_leak,
            ignore_earlier_zone_losses,
        } = conveyor;
        let optional = |text: &Option<String>| {
            text.as_deref()
                .map(|text| EquationMeaning::of(text, macros))
        };
        ConveyorMeaning {
            transit_time: EquationMeaning::of(transit_time, macros),
            capacity: optional(capacity),
            inflow_limit: optional(inflow_limit),
            sample: optional(sample),
            arrest: optional(arrest),
            discrete: *discrete,
            batch_integrity: *batch_integrity,
            one_at_a_time: *one_at_a_time,
            exponential_leak: *exponential_leak,
            ignore_earlier_zone_losses: *ignore_earlier_zone_losses,
        }
    }
}

/// A conveyor leak: its zone, whether it drains whole units, its fraction.
struct LeakMeaning {
    fraction: Option<EquationMeaning>,
    integers: bool,
    zone: (Option<EquationMeaning>, Option<EquationMeaning>),
}

impl LeakMeaning {
    fn of(leak: &Leakage, macros: &BTreeSet<String>) -> LeakMeaning {
        let Leakage {
            fraction,
            integers,
            zone_start,
            zone_end,
        } = leak;
        let optional = |text: &Option<String>| {
            text.as_deref()
                .map(|text| EquationMeaning::of(text, macros))
        };
        LeakMeaning {
            fraction: optional(fraction),
            integers: *integers,
            zone: (optional(zone_start), optional(zone_end)),
        }
    }
}

/// The markings on a variable that change what it simulates, each with what
/// it carries.
struct Markings {
    non_negative: bool,
    module_input: bool,
    /// Compared by value: `DataSource`'s own equality covers its fields.
    data_source: Option<DataSource>,
    conveyor: Option<ConveyorMeaning>,
    leak: Option<LeakMeaning>,
    /// Compared by value: `SpreadFlow`'s own equality covers its variants.
    spread: Option<SpreadFlow>,
    queue: bool,
    overflow: bool,
}

impl Markings {
    /// The markings in `compat`, and the variable's `ACTIVE INITIAL`, which
    /// is an equation and so compared with the equations.
    fn of(compat: &Compat, macros: &BTreeSet<String>) -> (Markings, Option<EquationMeaning>) {
        let Compat {
            active_initial,
            non_negative,
            can_be_module_input,
            // Whether another model may read the variable is a declaration
            // about access: the engine resolves a module's ports by its
            // references, whatever each variable's visibility.
            visibility: _,
            data_source,
            conveyor,
            leakage,
            spreadflow,
            queue,
            overflow,
        } = compat;
        let markings = Markings {
            non_negative: *non_negative,
            module_input: *can_be_module_input,
            data_source: data_source.clone(),
            conveyor: conveyor
                .as_ref()
                .map(|conveyor| ConveyorMeaning::of(conveyor, macros)),
            leak: leakage.as_ref().map(|leak| LeakMeaning::of(leak, macros)),
            spread: spreadflow.clone(),
            // A queue has no options; the marking is the whole of it.
            queue: matches!(queue, Some(Queue {})),
            overflow: *overflow,
        };
        let initial = active_initial
            .as_deref()
            .map(|text| EquationMeaning::of(text, macros));
        (markings, initial)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Stock,
    Flow,
    Aux,
    Module,
}

impl Kind {
    fn phrase(self) -> &'static str {
        match self {
            Kind::Stock => "a stock",
            Kind::Flow => "a flow",
            Kind::Aux => "an auxiliary",
            Kind::Module => "a module",
        }
    }
}

/// A module instance: the model it instantiates and its input bindings.
#[derive(PartialEq)]
struct ModuleMeaning {
    model: String,
    inputs: BTreeSet<(String, String)>,
}

/// What one variable is defined as.
struct VariableMeaning {
    /// The name as the project spells it, for the reasons.
    name: String,
    kind: Kind,
    dims: Vec<String>,
    /// The elements it defines (canonical keys; `""` for a scalar), or None
    /// when a dimension's elements are unknown.
    elements: Option<BTreeSet<String>>,
    /// Whether the variable is a table alone (`variable::is_lookup_only`, the
    /// compiler's own verdict): a table other variables look values up in,
    /// with no equation and no series of its own.
    table_only: bool,
    /// Each element's equation; a stock's is its initial value. An
    /// apply-to-all equation is every element's, and an `:EXCEPT:` default
    /// the compiler applies is the equation of each element it fills.
    equations: BTreeMap<String, EquationMeaning>,
    /// The `:EXCEPT:` default, when the compiler applies it. A default it
    /// applies defines the elements a dimension would gain, so it is part of
    /// the definition even where no element takes it today.
    default: Option<EquationMeaning>,
    /// Each element's initial equation.
    initials: BTreeMap<String, EquationMeaning>,
    /// Each element's graphical function, as the compiler reads it.
    tables: BTreeMap<String, Table>,
    /// A stock's inflows and outflows, canonical, in order, each once (the
    /// engine's `distinct_stock_flows`). The order is a queue's outflow
    /// priority and a conveyor's admission order.
    inflows: Vec<String>,
    outflows: Vec<String>,
    module: Option<ModuleMeaning>,
    markings: Markings,
}

/// One defined element: its key, equation, own initial and own table.
struct DefinedElement<'a> {
    key: String,
    equation: &'a str,
    initial: Option<&'a str>,
    table: Option<&'a GraphicalFunction>,
}

impl DefinedElement<'_> {
    /// An element the variable's one equation defines, with no initial or
    /// table of its own.
    fn whole(key: String, equation: &str) -> DefinedElement<'_> {
        DefinedElement {
            key,
            equation,
            initial: None,
            table: None,
        }
    }
}

/// The canonical spelling of an element key, one part per dimension.
fn element_key(key: &str) -> String {
    CanonicalElementName::from_subscript(key)
        .as_str()
        .to_string()
}

/// Every element key of a variable over `dims`, or None when a dimension's
/// elements are unknown.
fn element_product(
    dims: &[String],
    dimensions: &BTreeMap<String, DimensionMeaning>,
) -> Option<BTreeSet<String>> {
    let mut keys = vec![String::new()];
    for dim in dims {
        let elements = &dimensions.get(dim)?.elements;
        keys = keys
            .iter()
            .flat_map(|prefix| {
                elements.iter().map(move |e| {
                    if prefix.is_empty() {
                        e.clone()
                    } else {
                        format!("{prefix},{e}")
                    }
                })
            })
            .collect();
    }
    Some(keys.into_iter().collect())
}

impl VariableMeaning {
    fn of(
        var: &Variable,
        dimensions: &BTreeMap<String, DimensionMeaning>,
        macros: &BTreeSet<String>,
    ) -> VariableMeaning {
        // What each kind of variable holds. Documentation, units, the AI
        // provenance mark and the diagram's uid describe a variable; none of
        // them is read to compute it.
        let (name, kind, equation, gf, flows, module, compat) = match var {
            Variable::Stock(Stock {
                ident,
                equation,
                documentation: _,
                units: _,
                inflows,
                outflows,
                ai_state: _,
                uid: _,
                compat,
            }) => (
                ident,
                Kind::Stock,
                Some(equation),
                None,
                Some((inflows, outflows)),
                None,
                compat,
            ),
            Variable::Flow(Flow {
                ident,
                equation,
                documentation: _,
                units: _,
                gf,
                ai_state: _,
                uid: _,
                compat,
            }) => (
                ident,
                Kind::Flow,
                Some(equation),
                gf.as_ref(),
                None,
                None,
                compat,
            ),
            Variable::Aux(Aux {
                ident,
                equation,
                documentation: _,
                units: _,
                gf,
                ai_state: _,
                uid: _,
                compat,
            }) => (
                ident,
                Kind::Aux,
                Some(equation),
                gf.as_ref(),
                None,
                None,
                compat,
            ),
            Variable::Module(Module {
                ident,
                model_name,
                documentation: _,
                units: _,
                references,
                ai_state: _,
                uid: _,
                compat,
            }) => (
                ident,
                Kind::Module,
                None,
                None,
                None,
                Some((model_name, references)),
                compat,
            ),
        };
        let (mut markings, variable_initial) = Markings::of(compat, macros);
        // Only a stock and a flow carry a non-negative marking: the compiler
        // reads it of no other variable (`VariableSource::from`, `db::sync`),
        // so one an auxiliary holds means nothing, and a save that loses it
        // (the XMILE and MDL writers write it for no other variable, and the
        // JSON reader reads it of none) changes nothing.
        markings.non_negative &= matches!(kind, Kind::Stock | Kind::Flow);
        // A table alone has no equation, whatever stands where one would be
        // (nothing, or the MDL sentinel): the compiler's verdict, asked of
        // the compiler's own rule.
        let table_only = equation.is_some_and(|eq| crate::variable::is_lookup_only(eq, gf));
        let mut meaning = VariableMeaning {
            name: name.clone(),
            kind,
            dims: Vec::new(),
            elements: Some(BTreeSet::from([String::new()])),
            table_only,
            equations: BTreeMap::new(),
            default: None,
            initials: BTreeMap::new(),
            tables: BTreeMap::new(),
            inflows: Vec::new(),
            outflows: Vec::new(),
            module: None,
            markings,
        };
        let canonical_dims = |dims: &[String]| -> Vec<String> {
            dims.iter().map(|d| canonicalize(d).into_owned()).collect()
        };
        let mut defined: Vec<DefinedElement> = Vec::new();
        let whole = DefinedElement::whole;
        match equation {
            None => meaning.elements = None,
            Some(Equation::Scalar(text)) => defined.push(whole(String::new(), text)),
            Some(Equation::ApplyToAll(dims, text)) => {
                meaning.dims = canonical_dims(dims);
                meaning.elements = element_product(&meaning.dims, dimensions);
                match &meaning.elements {
                    Some(keys) => defined.extend(keys.iter().map(|key| whole(key.clone(), text))),
                    // The elements are unknown; the one equation is all
                    // there is to compare.
                    None => defined.push(whole(String::new(), text)),
                }
            }
            Some(Equation::Arrayed(dims, elements, default, applies_default)) => {
                meaning.dims = canonical_dims(dims);
                let mut keys = BTreeSet::new();
                for (key, text, initial, table) in elements {
                    let key = element_key(key);
                    keys.insert(key.clone());
                    defined.push(DefinedElement {
                        key,
                        equation: text,
                        initial: initial.as_deref(),
                        table: table.as_ref(),
                    });
                }
                // An `:EXCEPT:` default fills the elements the others leave,
                // where it applies (as the compiler applies it). One that
                // does not apply defines no element. The compiler still
                // reads it twice, and both show elsewhere: it parses it, so
                // an error in it is the variable's (the errors and the run
                // compare that), and a variable is a table alone only while
                // its default is empty (`table_only`).
                match default.as_deref().filter(|_| *applies_default) {
                    Some(text) => {
                        meaning.default = Some(EquationMeaning::of(text, macros));
                        match element_product(&meaning.dims, dimensions) {
                            Some(all) => {
                                let missing: Vec<String> = all.difference(&keys).cloned().collect();
                                for key in missing {
                                    keys.insert(key.clone());
                                    defined.push(whole(key, text));
                                }
                                meaning.elements = Some(keys);
                            }
                            None => meaning.elements = None,
                        }
                    }
                    None => meaning.elements = Some(keys),
                }
            }
        }
        for element in defined {
            let table = element.table.or(gf);
            let equation = if table_only {
                EquationMeaning::none()
            } else {
                EquationMeaning::of(element.equation, macros)
            };
            meaning.equations.insert(element.key.clone(), equation);
            let initial = element
                .initial
                .map(|text| EquationMeaning::of(text, macros))
                .or_else(|| variable_initial.clone());
            if let Some(initial) = initial {
                meaning.initials.insert(element.key.clone(), initial);
            }
            if let Some(table) = table {
                meaning.tables.insert(element.key, Table::of(table));
            }
        }
        if let Some((inflows, outflows)) = flows {
            let flows = |list: &[String]| -> Vec<String> {
                crate::datamodel::distinct_stock_flows(list)
                    .flows
                    .iter()
                    .map(|f| canonicalize(f).into_owned())
                    .collect()
            };
            meaning.inflows = flows(inflows);
            meaning.outflows = flows(outflows);
        }
        if let Some((model_name, references)) = module {
            meaning.module = Some(ModuleMeaning {
                model: canonicalize(model_name).into_owned(),
                inputs: references
                    .iter()
                    .map(|reference| {
                        let ModuleReference { src, dst } = reference;
                        (
                            canonicalize(src).into_owned(),
                            canonicalize(dst).into_owned(),
                        )
                    })
                    .collect(),
            });
        }
        meaning
    }
}

/// A macro's parameters in calling order, its primary output and its other
/// outputs, each as the canonical ident of the body variable it names.
struct MacroMeaning {
    parameters: Vec<String>,
    primary_output: String,
    additional_outputs: Vec<String>,
}

struct ModelMeaning {
    name: String,
    specs: Option<Specs>,
    /// A macro's signature, when the model is one.
    macro_spec: Option<MacroMeaning>,
    variables: BTreeMap<String, VariableMeaning>,
}

impl ModelMeaning {
    fn of(
        model: &Model,
        dimensions: &BTreeMap<String, DimensionMeaning>,
        macros: &BTreeSet<String>,
    ) -> ModelMeaning {
        let Model {
            name,
            sim_specs,
            variables,
            // How the model is drawn, the names given to its loops (which an
            // analysis is asked to score; no equation reads them) and the
            // sectors its variables are filed under are not computed with.
            views: _,
            loop_metadata: _,
            groups: _,
            macro_spec,
        } = model;
        let idents = |names: &[String]| -> Vec<String> {
            names.iter().map(|n| canonicalize(n).into_owned()).collect()
        };
        ModelMeaning {
            name: name.clone(),
            specs: sim_specs.as_ref().map(Specs::of),
            macro_spec: macro_spec.as_ref().map(|spec| {
                let MacroSpec {
                    parameters,
                    primary_output,
                    additional_outputs,
                } = spec;
                MacroMeaning {
                    parameters: idents(parameters),
                    primary_output: canonicalize(primary_output).into_owned(),
                    additional_outputs: idents(additional_outputs),
                }
            }),
            variables: variables
                .iter()
                .map(|var| {
                    (
                        canonicalize(var.get_ident()).into_owned(),
                        VariableMeaning::of(var, dimensions, macros),
                    )
                })
                .collect(),
        }
    }
}

struct Meaning {
    specs: Specs,
    /// The specs the simulated model runs with: its own, or the project's.
    main_specs: Specs,
    dimensions: BTreeMap<String, DimensionMeaning>,
    models: BTreeMap<String, ModelMeaning>,
    /// The key of the model a host simulates, which a save may name
    /// differently (MDL's one model is always `main`).
    main: Option<String>,
}

impl Meaning {
    fn of(project: &Project) -> Meaning {
        let Project {
            // What the project is called, its unit definitions, the file it
            // was read from and its AI provenance record say nothing of what
            // its models compute.
            name: _,
            sim_specs,
            dimensions,
            units: _,
            models,
            source: _,
            ai_information: _,
        } = project;
        let dimensions: BTreeMap<String, DimensionMeaning> = dimensions
            .iter()
            .map(|dim| {
                (
                    canonicalize(&dim.name).into_owned(),
                    DimensionMeaning::of(dim, &project.dimensions),
                )
            })
            .collect();
        let macros: BTreeSet<String> = models
            .iter()
            .filter(|m| m.macro_spec.is_some())
            .map(|m| canonicalize(&m.name).into_owned())
            .collect();
        let specs = Specs::of(sim_specs);
        let main = main_model(project);
        let main_specs = models
            .iter()
            .find(|m| Some(m.name.as_str()) == main)
            .and_then(|m| m.sim_specs.as_ref())
            .map_or(specs, Specs::of);
        Meaning {
            specs,
            main_specs,
            models: models
                .iter()
                .map(|model| {
                    (
                        canonicalize(&model.name).into_owned(),
                        ModelMeaning::of(model, &dimensions, &macros),
                    )
                })
                .collect(),
            dimensions,
            main: main.map(|name| canonicalize(name).into_owned()),
        }
    }
}

// ---- Structure: how two definitions differ ----

/// How the runs grade a structural change.
#[derive(Clone, Copy)]
enum Shows {
    /// In the series: of the variable the change is about when it has one,
    /// else of any variable.
    InSeries,
    /// In no run of this engine: the change is to a marking the engine does
    /// not enforce. A non-negative stock or flow simulates here exactly as
    /// an unmarked one does (GH #545), while XMILE 1.0 sections 4.2 and 4.3
    /// say the marking "prevents the stock [the flow] from going negative",
    /// by a mechanism it leaves to the vendor. Whether the marking binds is
    /// therefore not something a run here can show, so where there are
    /// results at all the change is one to the results.
    InNoRun,
}

/// One structural difference, before the runs grade it ([`settle`]).
struct Found {
    model: Option<String>,
    variable: Option<String>,
    reason: String,
    shows: Shows,
}

impl Found {
    fn in_project(reason: String) -> Found {
        Found {
            model: None,
            variable: None,
            reason,
            shows: Shows::InSeries,
        }
    }

    fn in_model(model: &str, reason: String) -> Found {
        Found {
            model: Some(model.to_string()),
            ..Found::in_project(reason)
        }
    }
}

/// `noun` and the keys it names, for a reason: "element 'a1'", "elements
/// 'a1' and 'a2'", ...
fn named(noun: &str, keys: &[&String]) -> String {
    let names: Vec<&str> = keys.iter().map(|k| k.as_str()).collect();
    let plural = if names.len() == 1 { "" } else { "s" };
    format!("{noun}{plural} {}", join_quoted_names(&names))
}

/// An equation for a reason, quoted, and cut short when it is long.
fn quoted(equation: &EquationMeaning) -> String {
    const LONGEST: usize = 80;
    let equation = equation.to_string();
    if equation.chars().count() <= LONGEST {
        format!("'{equation}'")
    } else {
        let cut: String = equation.chars().take(LONGEST - 3).collect();
        format!("'{cut}...'")
    }
}

/// The control variables a model may hold as variables, which a format that
/// keeps them as the simulation specs reads back as specs.
const CONTROL_VARIABLES: &[&str] = &["initial_time", "final_time", "time_step", "saveper"];

/// Whether `specs` hold what the control variable `ident` says, so a save
/// that keeps it as a spec rather than a variable changes nothing: it is a
/// scalar auxiliary whose equation is the number the spec is, or (for the
/// save step) the time step.
fn control_variable_kept(ident: &str, var: &VariableMeaning, specs: &Specs) -> bool {
    if !CONTROL_VARIABLES.contains(&ident) || var.kind != Kind::Aux || !var.dims.is_empty() {
        return false;
    }
    let value = var
        .equations
        .get("")
        .and_then(|equation| equation.control_value(specs));
    let spec = match ident {
        "initial_time" => specs.start,
        "final_time" => specs.stop,
        "time_step" => specs.dt,
        _ => specs.save_step,
    };
    value.is_some_and(|value| same_value(value, spec))
}

fn compare_structure(original: &Meaning, saved: &Meaning) -> Vec<Found> {
    let Meaning {
        specs,
        // The specs the run is made with are the project's or the simulated
        // model's own, each compared where it is declared.
        main_specs: _,
        dimensions,
        models,
        main,
    } = original;
    let mut found: Vec<Found> = specs
        .differences(&saved.specs)
        .into_iter()
        .map(Found::in_project)
        .collect();

    for (key, dim) in dimensions {
        match saved.dimensions.get(key) {
            Some(other) => compare_dimension(dim, other, &mut found),
            None => found.push(Found::in_project(format!(
                "dimension '{}' is not in the save",
                dim.name
            ))),
        }
    }
    for (key, dim) in &saved.dimensions {
        if !dimensions.contains_key(key) {
            found.push(Found::in_project(format!(
                "dimension '{}' is new in the save",
                dim.name
            )));
        }
    }

    // The model a host simulates is the same model in both, whatever each
    // calls it; the others pair by name.
    let counterpart = |key: &String| -> Option<(&String, &ModelMeaning)> {
        let key = if main.as_ref() == Some(key) {
            saved.main.as_ref()?
        } else {
            key
        };
        saved.models.get_key_value(key)
    };
    let mut paired: BTreeSet<&String> = BTreeSet::new();
    for (key, model) in models {
        let Some(other) = counterpart(key) else {
            found.push(Found::in_model(
                &model.name,
                format!("model '{}' is not in the save", model.name),
            ));
            continue;
        };
        paired.insert(other.0);
        compare_model(model, other.1, original, saved, &mut found);
    }
    for (key, model) in &saved.models {
        if !paired.contains(key) {
            found.push(Found::in_model(
                &model.name,
                format!("model '{}' is new in the save", model.name),
            ));
        }
    }
    found
}

fn compare_dimension(dim: &DimensionMeaning, saved: &DimensionMeaning, found: &mut Vec<Found>) {
    let DimensionMeaning {
        name,
        numbered,
        elements,
        parent,
        mappings,
    } = dim;
    let mut say = |reason: String| found.push(Found::in_project(reason));
    let elements_of = |numbered: bool| if numbered { "numbered" } else { "named" };
    if *numbered != saved.numbered {
        say(format!(
            "dimension '{name}' has {} elements in the save, not {} ones",
            elements_of(saved.numbered),
            elements_of(*numbered)
        ));
    }
    if *elements != saved.elements {
        say(format!(
            "dimension '{name}' has elements {} in the save, not {}",
            saved.elements.join(", "),
            elements.join(", ")
        ));
    }
    match (parent, &saved.parent) {
        (Some(a), Some(b)) if a != b => say(format!(
            "dimension '{name}' is a subdimension of '{b}' in the save, not of '{a}'"
        )),
        (Some(a), None) => say(format!(
            "dimension '{name}' is no longer a subdimension of '{a}'"
        )),
        (None, Some(b)) => say(format!(
            "dimension '{name}' becomes a subdimension of '{b}'"
        )),
        _ => {}
    }
    for (target, pairs) in mappings {
        match saved.mappings.get(target) {
            None => say(format!("dimension '{name}' no longer maps to '{target}'")),
            Some(saved_pairs) if saved_pairs != pairs => say(format!(
                "the mapping of dimension '{name}' to '{target}' relates other elements in the save"
            )),
            Some(_) => {}
        }
    }
    for target in saved.mappings.keys() {
        if !mappings.contains_key(target) {
            say(format!("dimension '{name}' maps to '{target}' in the save"));
        }
    }
}

fn compare_model(
    original: &ModelMeaning,
    saved: &ModelMeaning,
    original_project: &Meaning,
    saved_project: &Meaning,
    found: &mut Vec<Found>,
) {
    let ModelMeaning {
        name,
        specs,
        macro_spec,
        variables,
    } = original;
    // A model with specs of its own runs with them; one without, with the
    // project's, which the project's comparison covers.
    if specs.is_some() || saved.specs.is_some() {
        let a = specs.unwrap_or(original_project.specs);
        let b = saved.specs.unwrap_or(saved_project.specs);
        found.extend(
            a.differences(&b)
                .into_iter()
                .map(|reason| Found::in_model(name, reason)),
        );
    }
    compare_macro(name, macro_spec.as_ref(), saved.macro_spec.as_ref(), found);
    let about = |var: &VariableMeaning, reason: String, shows: Shows| Found {
        model: Some(name.clone()),
        variable: Some(var.name.clone()),
        reason,
        shows,
    };
    for (key, var) in variables {
        match saved.variables.get(key) {
            Some(other) => compare_variable(var, other, &mut |reason, shows| {
                found.push(about(var, reason, shows))
            }),
            None if control_variable_kept(key, var, &saved_project.specs) => {}
            None => found.push(about(
                var,
                format!("'{}' is not in the save", var.name),
                Shows::InSeries,
            )),
        }
    }
    for (key, var) in &saved.variables {
        if !variables.contains_key(key) && !control_variable_kept(key, var, &saved_project.specs) {
            found.push(about(
                var,
                format!("'{}' is new in the save", var.name),
                Shows::InSeries,
            ));
        }
    }
}

/// A macro's signature: what a call binds to which parameter, and what it
/// returns.
fn compare_macro(
    name: &str,
    original: Option<&MacroMeaning>,
    saved: Option<&MacroMeaning>,
    found: &mut Vec<Found>,
) {
    let mut say = |reason: String| found.push(Found::in_model(name, reason));
    let list = |names: &[String]| {
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    };
    match (original, saved) {
        (Some(_), None) => say(format!("model '{name}' is no longer a macro")),
        (None, Some(_)) => say(format!("model '{name}' becomes a macro")),
        (Some(a), Some(b)) => {
            let MacroMeaning {
                parameters,
                primary_output,
                additional_outputs,
            } = a;
            if *parameters != b.parameters {
                say(format!(
                    "macro '{name}' takes {} in the save, not {}",
                    list(&b.parameters),
                    list(parameters)
                ));
            }
            if *primary_output != b.primary_output {
                say(format!(
                    "macro '{name}' returns '{}' in the save, not '{primary_output}'",
                    b.primary_output
                ));
            }
            if *additional_outputs != b.additional_outputs {
                say(format!(
                    "macro '{name}' also returns {} in the save, not {}",
                    list(&b.additional_outputs),
                    list(additional_outputs)
                ));
            }
        }
        (None, None) => {}
    }
}

fn compare_variable(a: &VariableMeaning, b: &VariableMeaning, say: &mut dyn FnMut(String, Shows)) {
    let VariableMeaning {
        name,
        kind,
        dims,
        elements,
        table_only,
        equations,
        default,
        initials,
        tables,
        inflows,
        outflows,
        module,
        markings,
    } = a;
    let mut tell = |reason: String| say(reason, Shows::InSeries);
    if *kind != b.kind {
        tell(format!(
            "'{name}' is {} in the save, not {}",
            b.kind.phrase(),
            kind.phrase()
        ));
        return;
    }
    if *dims != b.dims {
        let list = |dims: &[String]| {
            if dims.is_empty() {
                "no dimension".to_string()
            } else {
                dims.join(", ")
            }
        };
        tell(format!(
            "'{name}' is defined over {}, not {}",
            list(&b.dims),
            list(dims)
        ));
    }
    if let (Some(ea), Some(eb)) = (elements, &b.elements) {
        let gained: Vec<&String> = eb.difference(ea).collect();
        let lost: Vec<&String> = ea.difference(eb).collect();
        if !lost.is_empty() {
            tell(format!("'{name}' loses {}", named("element", &lost)));
        }
        if !gained.is_empty() {
            tell(format!("'{name}' gains {}", named("element", &gained)));
        }
    }
    match (*table_only, b.table_only) {
        (true, false) => tell(format!(
            "'{name}' is computed in the save, where it is a table alone"
        )),
        (false, true) => tell(format!(
            "'{name}' is a table alone in the save, where it is computed"
        )),
        _ => {}
    }
    let element = |key: &str| {
        if key.is_empty() {
            String::new()
        } else {
            format!(" for element '{key}'")
        }
    };
    // The elements both define; the others are named above.
    let differing: Vec<(&String, &EquationMeaning, &EquationMeaning)> = equations
        .iter()
        .filter_map(|(key, was)| {
            b.equations
                .get(key)
                .filter(|is| *is != was)
                .map(|is| (key, was, is))
        })
        .collect();
    if let Some((key, was, is)) = differing.first() {
        let more = match differing.len() - 1 {
            0 => String::new(),
            1 => " (and 1 more element)".to_string(),
            n => format!(" (and {n} more elements)"),
        };
        let (is, was) = (quoted(is), quoted(was));
        let at = element(key);
        tell(if *kind == Kind::Stock {
            format!("'{name}' starts from {is}{at} in the save, not from {was}{more}")
        } else {
            format!("'{name}' is computed as {is}{at} in the save, not as {was}{more}")
        });
    }
    match (default, &b.default) {
        (Some(was), None) => tell(format!(
            "'{name}' no longer has its :EXCEPT: default {}",
            quoted(was)
        )),
        (None, Some(is)) => tell(format!("'{name}' gains an :EXCEPT: default {}", quoted(is))),
        (Some(was), Some(is)) if was != is => tell(format!(
            "the :EXCEPT: default of '{name}' is {} in the save, not {}",
            quoted(is),
            quoted(was)
        )),
        _ => {}
    }
    for (key, initial) in initials {
        match b.initials.get(key) {
            None => tell(format!("'{name}' loses its initial value{}", element(key))),
            Some(other) if other != initial => tell(format!(
                "the initial value of '{name}' changes{}",
                element(key)
            )),
            Some(_) => {}
        }
    }
    for key in b.initials.keys() {
        if !initials.contains_key(key) {
            tell(format!("'{name}' gains an initial value{}", element(key)));
        }
    }
    for (key, table) in tables {
        match b.tables.get(key) {
            None => tell(format!(
                "'{name}' loses its graphical function{}",
                element(key)
            )),
            Some(other) if other != table => tell(format!(
                "the graphical function of '{name}' changes{}",
                element(key)
            )),
            Some(_) => {}
        }
    }
    for key in b.tables.keys() {
        if !tables.contains_key(key) {
            tell(format!(
                "'{name}' gains a graphical function{}",
                element(key)
            ));
        }
    }
    let mut flows = |which: &str, a: &[String], b: &[String]| {
        let lost: Vec<&String> = a.iter().filter(|f| !b.contains(f)).collect();
        let gained: Vec<&String> = b.iter().filter(|f| !a.contains(f)).collect();
        if !lost.is_empty() {
            tell(format!("'{name}' loses {}", named(which, &lost)));
        }
        if !gained.is_empty() {
            tell(format!("'{name}' gains {}", named(which, &gained)));
        }
        // The same flows in another order: a queue serves its outflows,
        // and a conveyor admits its inflows, in their order.
        if lost.is_empty() && gained.is_empty() && a != b {
            tell(format!(
                "'{name}' takes its {which}s in another order in the save: {}, not {}",
                b.join(", "),
                a.join(", ")
            ));
        }
    };
    flows("inflow", inflows, &b.inflows);
    flows("outflow", outflows, &b.outflows);
    if *module != b.module {
        match (module, &b.module) {
            (Some(was), Some(is)) if was.model != is.model => tell(format!(
                "'{name}' instantiates '{}' in the save, not '{}'",
                is.model, was.model
            )),
            _ => tell(format!("the inputs of module '{name}' change")),
        }
    }
    compare_markings(name, markings, &b.markings, say);
}

fn compare_markings(name: &str, a: &Markings, b: &Markings, say: &mut dyn FnMut(String, Shows)) {
    let Markings {
        non_negative,
        module_input,
        data_source,
        conveyor,
        leak,
        spread,
        queue,
        overflow,
    } = a;
    // A marking only one side has: what the variable stops or starts being.
    let marking = |was: bool, is: bool, what: &str| -> Option<String> {
        match (was, is) {
            (true, false) => Some(format!("'{name}' is no longer {what}")),
            (false, true) => Some(format!("'{name}' becomes {what}")),
            _ => None,
        }
    };
    if let Some(reason) = marking(*non_negative, b.non_negative, "non-negative") {
        say(reason, Shows::InNoRun);
    }
    let mut tell = |reason: String| say(reason, Shows::InSeries);
    let presence: [(bool, bool, &str); 7] = [
        (conveyor.is_some(), b.conveyor.is_some(), "a conveyor"),
        (*queue, b.queue, "a queue"),
        (leak.is_some(), b.leak.is_some(), "a conveyor leak"),
        (
            spread.is_some(),
            b.spread.is_some(),
            "a conveyor inflow with its own placement",
        ),
        (*overflow, b.overflow, "a queue's overflow"),
        (
            data_source.is_some(),
            b.data_source.is_some(),
            "read from data",
        ),
        (*module_input, b.module_input, "a module input"),
    ];
    for (was, is, what) in presence {
        if let Some(reason) = marking(was, is, what) {
            tell(reason);
        }
    }
    // A marking both have: what it carries.
    if let (Some(was), Some(is)) = (conveyor, &b.conveyor)
        && was != is
    {
        tell(format!("the conveyor '{name}' changes"));
    }
    if let (Some(was), Some(is)) = (leak, &b.leak) {
        let LeakMeaning {
            fraction,
            integers,
            zone,
        } = was;
        if *zone != is.zone {
            tell(format!("the zone the leak '{name}' drains changes"));
        }
        if *integers != is.integers {
            let whole = |integers: bool| {
                if integers {
                    "whole units"
                } else {
                    "any amount"
                }
            };
            tell(format!(
                "the leak '{name}' drains {} in the save, not {}",
                whole(is.integers),
                whole(*integers)
            ));
        }
        if *fraction != is.fraction {
            tell(format!("the fraction the leak '{name}' drains changes"));
        }
    }
    if let (Some(was), Some(is)) = (spread, &b.spread)
        && was != is
    {
        tell(format!(
            "how the inflow '{name}' spreads over its conveyor changes"
        ));
    }
    if let (Some(was), Some(is)) = (data_source, &b.data_source)
        && was != is
    {
        tell(format!("the data source of '{name}' changes"));
    }
}

// ---- Behaviour ----

/// The model a host simulates: `main`, or else the first that is not a
/// macro.
fn main_model(project: &Project) -> Option<&str> {
    project
        .models
        .iter()
        .find(|m| m.name == "main")
        .or_else(|| project.models.iter().find(|m| m.macro_spec.is_none()))
        .map(|m| m.name.as_str())
}

fn simulate(project: &Project) -> std::result::Result<crate::Results, String> {
    let main = main_model(project).ok_or_else(|| "it has no model to simulate".to_string())?;
    let mut vm = crate::queue_compile::build_vm(project, main).map_err(|e| e.to_string())?;
    vm.run_to_end().map_err(|e| e.to_string())?;
    Ok(vm.into_results())
}

/// The canonical idents of the variables of the model a host simulates.
fn main_variables(project: &Project) -> BTreeSet<String> {
    let main = main_model(project);
    project
        .models
        .iter()
        .filter(|m| Some(m.name.as_str()) == main)
        .flat_map(|m| m.variables.iter())
        .map(|var| canonicalize(var.get_ident()).into_owned())
        .collect()
}

/// The variable a results column belongs to, among `variables` (canonical
/// idents): the variable the whole column names, else the longest one the
/// column extends with an element key (`x[a1,b2]`) or a path into a module
/// instance (`hares·births`, the instance's).
///
/// The whole column is asked first because a name may hold either mark
/// itself (`"x [usd]"` is `x_[usd]`). A column the compiler adds for itself,
/// whose name carries the synthetic prefix ([`crate::ltm::is_synthetic_node_name`],
/// the mark `capture::synthetic_ident` gives a builtin's helper), belongs to
/// none: what it holds shows in the variables it feeds, and a save may compute
/// them without it. Nor does the clock (`time`, unless the model declares a
/// variable of the name, whose series the column then is).
pub fn column_variable<'a>(column: &'a str, variables: &BTreeSet<String>) -> Option<&'a str> {
    if variables.contains(column) {
        return Some(column);
    }
    if crate::ltm::is_synthetic_node_name(column) {
        return None;
    }
    column
        .char_indices()
        .rev()
        .filter(|&(_, c)| c == '[' || c == '\u{b7}')
        .map(|(at, _)| &column[..at])
        .find(|name| variables.contains(*name))
}

/// What comparing the two runs shows, when both simulate over the same
/// specs.
struct ComparedRuns {
    /// Each variable of the simulated model whose series differ, with the
    /// first difference.
    differing: BTreeMap<String, String>,
    /// The variables with series in either run.
    simulated: BTreeSet<String>,
    /// Why every series differs, when the save simulates another number of
    /// steps.
    steps: Option<String>,
}

/// What running the project and its save shows.
enum Runs {
    Compared(ComparedRuns),
    /// Both simulate, over other specs: every series differs, for these
    /// reasons.
    OtherSpecs(Vec<String>),
    /// The project simulates and its save does not, for this reason.
    SaveFails(String),
    /// The save simulates and the project does not.
    ProjectFails,
    /// Neither simulates.
    NeitherSimulates,
}

fn compare_runs(original: &Project, saved: &Project, specs: &Specs, saved_specs: &Specs) -> Runs {
    let other_specs = specs.differences(saved_specs);
    match (simulate(original), simulate(saved)) {
        (Ok(_), Ok(_)) if !other_specs.is_empty() => Runs::OtherSpecs(other_specs),
        (Ok(a), Ok(b)) => {
            let variables = &main_variables(original) | &main_variables(saved);
            Runs::Compared(compare_series(&a, &b, &variables))
        }
        (Ok(_), Err(why)) => Runs::SaveFails(why),
        (Err(_), Ok(_)) => Runs::ProjectFails,
        (Err(_), Err(_)) => Runs::NeitherSimulates,
    }
}

/// Grade each structural change by what the runs show, and add what only
/// the runs show: the variables whose series differ though the structure
/// names nothing about them, or why the save does not simulate.
fn settle(
    original: &Project,
    saved: &Project,
    found: Vec<Found>,
    runs: Runs,
) -> Vec<MeaningChange> {
    let main = main_model(original).map(str::to_string);
    let graded = |found: Found, kind: ChangeKind| MeaningChange {
        model: found.model,
        variable: found.variable,
        kind,
        reason: found.reason,
    };
    let all = |found: Vec<Found>, kind: ChangeKind| -> Vec<MeaningChange> {
        found.into_iter().map(|f| graded(f, kind)).collect()
    };
    let in_project = |reason: String| MeaningChange {
        model: None,
        variable: None,
        kind: ChangeKind::Results,
        reason,
    };
    match runs {
        Runs::Compared(runs) => {
            let ComparedRuns {
                differing,
                simulated,
                steps,
            } = runs;
            if let Some(reason) = steps {
                let mut changes = all(found, ChangeKind::Results);
                changes.push(MeaningChange {
                    model: main,
                    ..in_project(reason)
                });
                return changes;
            }
            // A change about one variable of the simulated model shows in
            // the results when that variable's series do. Any other, and one
            // about a variable with no series of its own (a table, which
            // shows only in what reads it), shows when any series does.
            let about = |found: &Found| {
                found
                    .variable
                    .as_deref()
                    .filter(|_| found.model == main)
                    .map(|v| canonicalize(v).into_owned())
            };
            let named: BTreeSet<String> = found.iter().filter_map(about).collect();
            let mut changes: Vec<MeaningChange> = Vec::new();
            for one in found {
                let shows = match one.shows {
                    Shows::InSeries => match about(&one) {
                        Some(v) if simulated.contains(&v) => differing.contains_key(&v),
                        _ => !differing.is_empty(),
                    },
                    Shows::InNoRun => true,
                };
                let kind = if shows {
                    ChangeKind::Results
                } else {
                    ChangeKind::Structure
                };
                changes.push(graded(one, kind));
            }
            for (owner, reason) in differing {
                if !named.contains(&owner) {
                    changes.push(MeaningChange {
                        model: main.clone(),
                        variable: Some(owner),
                        kind: ChangeKind::Results,
                        reason,
                    });
                }
            }
            changes
        }
        Runs::OtherSpecs(differences) => {
            // The structure names how the specs differ wherever it compares
            // them; a difference it named nowhere is named here, so that
            // other specs are never no change.
            let mut changes = all(found, ChangeKind::Results);
            for reason in differences {
                if !changes.iter().any(|change| change.reason == reason) {
                    changes.push(in_project(reason));
                }
            }
            changes
        }
        Runs::SaveFails(why) => {
            // The save's own errors say why, unless the structure already
            // does.
            let unexplained = found.is_empty();
            let mut changes = all(found, ChangeKind::Results);
            changes.push(in_project(format!("the save does not simulate: {why}")));
            if unexplained {
                changes.extend(errors(saved).into_iter().map(|(model, variable, error)| {
                    let reason = match &variable {
                        Some(var) => format!("'{var}' has an error in the save: {error}"),
                        None => format!("the save has an error: {error}"),
                    };
                    MeaningChange {
                        model,
                        variable,
                        kind: ChangeKind::Results,
                        reason,
                    }
                }));
            }
            changes
        }
        Runs::ProjectFails => {
            let mut changes = all(found, ChangeKind::Results);
            changes.push(in_project(
                "the save simulates, where the model does not".to_string(),
            ));
            changes
        }
        Runs::NeitherSimulates => {
            let mut changes = all(found, ChangeKind::Structure);
            changes.extend(compare_errors(original, saved));
            changes
        }
    }
}

/// A project's errors, each as (model, variable, code and reason), in a
/// stable order.
fn errors(project: &Project) -> BTreeSet<(Option<String>, Option<String>, String)> {
    let mut db = crate::db::SimlinDb::default();
    let sync = crate::db::sync_from_datamodel_incremental(&mut db, project, None);
    crate::db::collect_all_diagnostics(&db, sync.project, crate::db::LtmOverlay::Off)
        .iter()
        .filter(|d| d.severity == crate::db::DiagnosticSeverity::Error)
        .map(|d| {
            let formatted = crate::errors::format_diagnostic(d);
            let reason = code_and_reason(formatted.code, formatted.details.as_deref());
            (
                formatted.model_name.clone(),
                formatted
                    .variable_name
                    .as_deref()
                    .map(|v| canonicalize(v).into_owned()),
                reason,
            )
        })
        .collect()
}

/// A model that does not simulate keeps its meaning when its save reports
/// the same errors. Neither has results, so another error is a change to
/// the definition.
fn compare_errors(original: &Project, saved: &Project) -> Vec<MeaningChange> {
    let (a, b) = (errors(original), errors(saved));
    let structural =
        |model: &Option<String>, variable: &Option<String>, reason: String| MeaningChange {
            model: model.clone(),
            variable: variable.clone(),
            kind: ChangeKind::Structure,
            reason,
        };
    let mut changes = Vec::new();
    for (model, variable, error) in b.difference(&a) {
        let reason = match variable {
            Some(var) => {
                format!("'{var}' has an error in the save that the model does not: {error}")
            }
            None => format!("the save has an error the model does not: {error}"),
        };
        changes.push(structural(model, variable, reason));
    }
    for (model, variable, error) in a.difference(&b) {
        let reason = match variable {
            Some(var) => format!("'{var}' no longer has the model's error: {error}"),
            None => format!("the save no longer has the model's error: {error}"),
        };
        changes.push(structural(model, variable, reason));
    }
    changes
}

/// The values one results column holds, step by step.
fn series(results: &crate::Results, at: usize) -> impl Iterator<Item = f64> + '_ {
    results.iter().map(move |row| row[at])
}

/// Each variable of the simulated model (`variables`, in either project)
/// whose series the save simulates differently, with its first difference,
/// leaving out the control variables a save keeps as its specs.
fn compare_series(
    original: &crate::Results,
    saved: &crate::Results,
    variables: &BTreeSet<String>,
) -> ComparedRuns {
    let mut differing: BTreeMap<String, String> = BTreeMap::new();
    let mut simulated: BTreeSet<String> = BTreeSet::new();
    let mut steps = None;
    if original.step_count != saved.step_count {
        steps = Some(format!(
            "the save simulates {} steps, not {}",
            saved.step_count, original.step_count
        ));
    } else {
        let columns = |results: &crate::Results| -> BTreeMap<String, usize> {
            results
                .offsets
                .iter()
                .map(|(name, &at)| (name.as_str().to_string(), at))
                .collect()
        };
        let (before, after) = (columns(original), columns(saved));
        let owner = |column: &str| {
            column_variable(column, variables)
                .filter(|owner| !CONTROL_VARIABLES.contains(owner))
                .map(str::to_string)
        };
        simulated = before
            .keys()
            .chain(after.keys())
            .filter_map(|name| owner(name))
            .collect();
        for (name, &at) in &before {
            let Some(owner) = owner(name) else {
                continue;
            };
            if differing.contains_key(&owner) {
                continue;
            }
            let Some(&other_at) = after.get(name) else {
                differing.insert(owner, format!("the save does not simulate '{name}'"));
                continue;
            };
            let first = series(original, at)
                .zip(series(saved, other_at))
                .position(|(x, y)| !same_value(x, y));
            if let Some(step) = first {
                let was = series(original, at).nth(step).unwrap_or(f64::NAN);
                let is = series(saved, other_at).nth(step).unwrap_or(f64::NAN);
                // The clock is read from its slot, not its key: a variable
                // the model declares as `time` takes the key `time`.
                let when = series(original, crate::results::TIME_OFF)
                    .nth(step)
                    .map(|t| format!(" at time {t}"))
                    .unwrap_or_default();
                differing.insert(
                    owner,
                    format!("'{name}' simulates differently: {is} where it was {was}{when}"),
                );
            }
        }
        for name in after.keys() {
            let Some(owner) = owner(name) else {
                continue;
            };
            if !before.contains_key(name) && !differing.contains_key(&owner) {
                differing.insert(
                    owner,
                    format!("the save simulates '{name}', which the model does not"),
                );
            }
        }
    }
    ComparedRuns {
        differing,
        simulated,
        steps,
    }
}
