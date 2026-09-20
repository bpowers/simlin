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
//!   - each dimension's elements, parent, and the element pairs its mappings
//!     relate
//!   - each variable's kind, dimensions and elements
//!   - each element's equation, in the spelling the engine resolves it by
//!   - each variable's `:EXCEPT:` default, where the compiler applies it
//!   - initial values, graphical functions as the compiler reads them, flows
//!     and module inputs
//!   - the flags that change a simulation (non-negative, conveyor, queue,
//!     data source)
//!
//!   Two equations are the same only in spellings that provably mean the
//!   same to the engine: names that canonicalize alike, numbers that parse
//!   to the same value, `PI()` and `INF()` and their values, `MODULO(a, b)`
//!   and `a mod b` (each call only where no macro takes its name, and not
//!   inside a subscript), and an empty table-only equation and the MDL
//!   sentinel `0+0`. Any other difference is a change, even one that
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
//! differ now, or only its definition does.
//!
//! Meaning is what the model computes. Units, documentation, views, groups
//! and loop names are not, though a save that drops them still reports them,
//! through the writers' own warnings. Nor is where a control variable lives:
//! a model that keeps its specs as variables (`FINAL TIME`, `SAVEPER`) and a
//! save that keeps them as the simulation specs mean the same when the values
//! agree.

use std::collections::{BTreeMap, BTreeSet};
use std::io::BufReader;

use crate::ast::{BinaryOp, Expr0, IndexExpr0, Literal};
use crate::buffa::Message;
use crate::builtins::UntypedBuiltinFn;
use crate::common::{CanonicalElementName, RawIdent, Result, canonicalize};
use crate::data_provider::DataProvider;
use crate::datamodel::{
    Compat, Dimension, DimensionElements, Dt, Equation, GraphicalFunction, Model, Project,
    SimMethod, SimSpecs, Variable,
};
use crate::errors::{code_and_reason, join_quoted_names};

#[cfg(test)]
#[path = "save_check_tests.rs"]
mod tests;

/// The formats a project can be saved in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Whether a change shows in what the save simulates today. Either kind is
/// a change a save in place must not make; the kind only grades it, so a
/// host can say whether the results would differ.
///
/// When neither the project nor its save simulates, every change is
/// `Structure`: there are no results to compare.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// The save's results differ from the project's now: a series simulates
    /// differently, the two simulate over other specs, or only one of them
    /// simulates or opens.
    Results,
    /// The save defines the model differently, though what it simulates
    /// today is the same: an equation the run does not reach, a flag it does
    /// not exercise, an `:EXCEPT:` default that fills no element yet, or
    /// another error in a model that does not simulate.
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
    /// Whether the save's results differ now, or only its definition.
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
    let saved = match read_back(project, format, data)? {
        Ok(saved) => saved,
        Err(why) => {
            return Ok(vec![MeaningChange {
                model: None,
                variable: None,
                kind: ChangeKind::Results,
                reason: format!("the save does not read back: {why}"),
            }]);
        }
    };
    let (original_meaning, saved_meaning) = (Meaning::of(project), Meaning::of(&saved));
    let changes = compare_structure(&original_meaning, &saved_meaning);
    let specs_change = original_meaning.main_specs != saved_meaning.main_specs;
    let runs = compare_runs(project, &saved, specs_change);
    Ok(settle(project, &saved, changes, runs))
}

/// The project a save of `project` in `format` reads back as: Err when the
/// format cannot hold it, Ok(Err) when the save does not read back.
fn read_back(
    project: &Project,
    format: SaveFormat,
    data: Option<&dyn DataProvider>,
) -> Result<std::result::Result<Project, String>> {
    Ok(match format {
        SaveFormat::Mdl => {
            let text = crate::compat::to_mdl(project)?;
            crate::compat::open_vensim_with_data(&text, data).map_err(|e| e.to_string())
        }
        SaveFormat::Xmile => {
            let text = crate::compat::to_xmile(project)?;
            crate::compat::open_xmile(&mut BufReader::new(text.as_bytes()))
                .map_err(|e| e.to_string())
        }
        SaveFormat::Json => {
            let json: crate::json::Project = project.clone().into();
            let bytes = serde_json::to_vec(&json).map_err(|e| {
                crate::common::Error::new(
                    crate::common::ErrorKind::Model,
                    crate::common::ErrorCode::Generic,
                    Some(e.to_string()),
                )
            })?;
            crate::json::Project::from_reader(bytes.as_slice())
                .map(Into::into)
                .map_err(|e| e.to_string())
        }
        SaveFormat::SdaiJson => {
            let model: crate::json_sdai::SdaiModel = project.clone().into();
            let bytes = serde_json::to_vec(&model).map_err(|e| {
                crate::common::Error::new(
                    crate::common::ErrorKind::Model,
                    crate::common::ErrorCode::Generic,
                    Some(e.to_string()),
                )
            })?;
            crate::json_sdai::SdaiModel::from_reader(bytes.as_slice())
                .map(Into::into)
                .map_err(|e| e.to_string())
        }
        SaveFormat::Protobuf => {
            let bytes = crate::serde::serialize(project)?.encode_to_vec();
            crate::project_io::Project::decode_from_slice(&bytes)
                .map(crate::serde::deserialize)
                .map_err(|e| e.to_string())
        }
    })
}

// ---- Structure ----

/// A project's simulation specs, as numbers.
#[derive(Clone, Copy, PartialEq)]
struct Specs {
    start: f64,
    stop: f64,
    dt: f64,
    save_step: f64,
    method: SimMethod,
}

impl Specs {
    fn of(specs: &SimSpecs) -> Specs {
        let value = |dt: &Dt| match *dt {
            Dt::Dt(v) => v,
            Dt::Reciprocal(v) => 1.0 / v,
        };
        let dt = value(&specs.dt);
        Specs {
            start: specs.start,
            stop: specs.stop,
            dt,
            // No save step saves at the time step.
            save_step: specs.save_step.as_ref().map_or(dt, value),
            method: specs.sim_method,
        }
    }
}

/// A dimension's elements, in order, its parent, and where it maps them.
struct DimensionMeaning {
    name: String,
    elements: Vec<String>,
    parent: Option<String>,
    /// Each target dimension, with the pairs of elements the mapping relates
    /// (a positional mapping as the pairs it makes).
    mappings: BTreeMap<String, BTreeSet<(String, String)>>,
}

/// A graphical function as the compiler reads it (`variable::parse_table`):
/// a table that leaves out its x points is the table that writes out the x
/// points the compiler spreads over its scale.
struct Table {
    kind: crate::datamodel::GraphicalFunctionKind,
    x: Vec<f64>,
    y: Vec<f64>,
    x_scale: (f64, f64),
    y_scale: (f64, f64),
}

/// Two tables are the same when the compiler reads the same numbers from
/// them, a NaN in both (a y point that is not a number) included.
impl PartialEq for Table {
    fn eq(&self, other: &Table) -> bool {
        let same = |a: &[f64], b: &[f64]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_value(*x, *y))
        };
        self.kind == other.kind
            && same(&self.x, &other.x)
            && same(&self.y, &other.y)
            && same(
                &[
                    self.x_scale.0,
                    self.x_scale.1,
                    self.y_scale.0,
                    self.y_scale.1,
                ],
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
        // A table the compiler refuses (an x that is not a number) is
        // compared as it is stored; the refusal shows in the errors.
        let x = match crate::variable::parse_table(Some(gf)) {
            Ok(Some(table)) => table.x,
            _ => gf.x_points.clone().unwrap_or_default(),
        };
        Table {
            kind: gf.kind,
            x,
            y: gf.y_points.clone(),
            x_scale: (gf.x_scale.min, gf.x_scale.max),
            y_scale: (gf.y_scale.min, gf.y_scale.max),
        }
    }
}

/// Whether two values are the same number: `==`, with NaN the same as NaN
/// (so -0 is 0).
fn same_value(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

/// What one variable is defined as.
struct VariableMeaning {
    /// The name as the project spells it, for the reasons.
    name: String,
    /// "a stock", "a flow", "an auxiliary" or "a module".
    kind: &'static str,
    dims: Vec<String>,
    /// The elements it defines (canonical keys; `""` for a scalar), or None
    /// when a dimension's elements are unknown.
    elements: Option<BTreeSet<String>>,
    /// Each element's equation, canonical (see [`canonical_equation`]); a
    /// stock's is its initial value. An apply-to-all equation is every
    /// element's, and an `:EXCEPT:` default the compiler applies is the
    /// equation of each element it fills.
    equations: BTreeMap<String, String>,
    /// The `:EXCEPT:` default, canonical, when the compiler applies it. A
    /// default it applies defines the elements a dimension would gain, so it
    /// is part of the definition even where no element takes it today.
    default: Option<String>,
    /// Each element's initial equation, canonical.
    initials: BTreeMap<String, String>,
    /// Each element's graphical function, as the compiler reads it.
    tables: BTreeMap<String, Table>,
    /// A stock's inflows and outflows, canonical, in order, each once (the
    /// engine's `distinct_stock_flows`). The order is a queue's outflow
    /// priority and a conveyor's admission order.
    inflows: Vec<String>,
    outflows: Vec<String>,
    /// A module's model and its input bindings.
    module: Option<(String, BTreeSet<(String, String)>)>,
    compat: Compat,
    /// A scalar auxiliary's equation, canonical: what a control variable
    /// (`SAVEPER = TIME STEP`) says its spec is.
    scalar: Option<String>,
}

struct ModelMeaning {
    name: String,
    specs: Option<Specs>,
    /// A macro's signature, when the model is one.
    macro_spec: Option<MacroMeaning>,
    variables: BTreeMap<String, VariableMeaning>,
}

/// A macro's parameters in calling order, its primary output and its other
/// outputs, each as the canonical ident of the body variable it names.
#[derive(PartialEq)]
struct MacroMeaning {
    parameters: Vec<String>,
    primary_output: String,
    additional_outputs: Vec<String>,
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

/// The canonical spelling of an element key, one part per dimension.
fn element_key(key: &str) -> String {
    CanonicalElementName::from_subscript(key)
        .as_str()
        .to_string()
}

/// An equation's text, in the form two spellings of one equation share: its
/// parse with the provably equal spellings written one way ([`normalize`]),
/// printed (every name in it is canonical, so lowercase); its trimmed text,
/// case folded, when it does not parse. A table-only equation (one beside a
/// graphical function, with nothing to feed it) is empty whether it is
/// written empty or as the MDL sentinel.
fn canonical_equation(text: &str, has_table: bool, macros: &BTreeSet<String>) -> String {
    if has_table && crate::variable::is_empty_or_sentinel(text) {
        return String::new();
    }
    match Expr0::new(text, crate::lexer::LexerType::Equation) {
        Ok(Some(expr)) => crate::ast::print_eqn(&normalize(expr, macros)),
        _ => text.trim().to_lowercase(),
    }
}

/// `expr` with each spelling that provably means the same to the engine
/// written one way: a name as its canonical ident (the engine resolves names
/// by it), a number as the value it parses to, and three calls the engine
/// computes as something else it has a spelling for: `PI()` and `INF()` as
/// their values, and `MODULO(a, b)` as `a mod b`. A call is left alone where
/// the project defines a macro of its name, which the engine would expand
/// instead, and inside a subscript, where the engine reads an index literal
/// otherwise than a call (`x[1e400]` does not compile, and `x[INF()]` does).
fn normalize(expr: Expr0, macros: &BTreeSet<String>) -> Expr0 {
    normalize_in(expr, macros, true)
}

fn normalize_in(expr: Expr0, macros: &BTreeSet<String>, fold_calls: bool) -> Expr0 {
    let boxed = |e: Box<Expr0>| Box::new(normalize_in(*e, macros, fold_calls));
    match expr {
        Expr0::Const(_, literal, loc) => Expr0::Const(number(literal.value()), literal, loc),
        Expr0::Var(ident, loc) => Expr0::Var(canonical_ident(&ident), loc),
        Expr0::App(UntypedBuiltinFn(func, args), loc) => {
            let func = canonicalize(&func).into_owned();
            let mut args: Vec<Expr0> = args
                .into_vec()
                .into_iter()
                .map(|arg| normalize_in(arg, macros, fold_calls))
                .collect();
            if fold_calls && !macros.contains(&func) {
                let constant = match func.as_str() {
                    "pi" => Some(std::f64::consts::PI),
                    "inf" => Some(f64::INFINITY),
                    _ => None,
                };
                if let Some(value) = constant.filter(|_| args.is_empty()) {
                    return Expr0::Const(number(value), Literal::new(value), loc);
                }
                if func == "modulo" && args.len() == 2 {
                    let rhs = args.pop().unwrap();
                    let lhs = args.pop().unwrap();
                    return Expr0::Op2(BinaryOp::Mod, Box::new(lhs), Box::new(rhs), loc);
                }
            }
            Expr0::App(UntypedBuiltinFn(func, args.into()), loc)
        }
        Expr0::Subscript(ident, indices, loc) => {
            let index = |e: Box<Expr0>| Box::new(normalize_in(*e, macros, false));
            let indices: Vec<IndexExpr0> = indices
                .into_vec()
                .into_iter()
                .map(|i| match i {
                    IndexExpr0::StarRange(dim, loc) => {
                        IndexExpr0::StarRange(canonical_ident(&dim), loc)
                    }
                    IndexExpr0::Range(lo, hi, loc) => IndexExpr0::Range(index(lo), index(hi), loc),
                    IndexExpr0::Expr(e) => IndexExpr0::Expr(normalize_in(e, macros, false)),
                    other @ (IndexExpr0::Wildcard(_) | IndexExpr0::DimPosition(_, _)) => other,
                })
                .collect();
            Expr0::Subscript(canonical_ident(&ident), indices.into(), loc)
        }
        Expr0::Op1(op, e, loc) => Expr0::Op1(op, boxed(e), loc),
        Expr0::Op2(op, l, r, loc) => Expr0::Op2(op, boxed(l), boxed(r), loc),
        Expr0::If(c, t, f, loc) => Expr0::If(boxed(c), boxed(t), boxed(f), loc),
    }
}

/// A number as the one spelling of its value: the shortest decimal that
/// reads back as it, in exponent form when it is very large or very small.
/// Infinity (a literal too large for a number, or `INF()` folded into one)
/// is `Inf`, and the NaN literal `NaN`: their capitals keep them from reading
/// as a variable or a call, since every canonical name is lowercase.
fn number(value: f64) -> String {
    let magnitude = value.abs();
    if value.is_nan() {
        "NaN".to_string()
    } else if value.is_infinite() {
        if value > 0.0 { "Inf" } else { "-Inf" }.to_string()
    } else if value == 0.0 || (1e-5..1e16).contains(&magnitude) {
        format!("{value}")
    } else {
        format!("{value:e}")
    }
}

fn canonical_ident(ident: &RawIdent) -> RawIdent {
    RawIdent::new(canonicalize(ident.as_str()).into_owned())
}

impl Meaning {
    fn of(project: &Project) -> Meaning {
        let dimensions: BTreeMap<String, DimensionMeaning> = project
            .dimensions
            .iter()
            .map(|dim| {
                (
                    canonicalize(&dim.name).into_owned(),
                    DimensionMeaning::of(dim, &project.dimensions),
                )
            })
            .collect();
        let macros: BTreeSet<String> = project
            .models
            .iter()
            .filter(|m| m.macro_spec.is_some())
            .map(|m| canonicalize(&m.name).into_owned())
            .collect();
        let models = project
            .models
            .iter()
            .map(|model| {
                (
                    canonicalize(&model.name).into_owned(),
                    ModelMeaning::of(model, &dimensions, &macros),
                )
            })
            .collect();
        let specs = Specs::of(&project.sim_specs);
        let main = main_model(project);
        let main_specs = project
            .models
            .iter()
            .find(|m| Some(m.name.as_str()) == main)
            .and_then(|m| m.sim_specs.as_ref())
            .map_or(specs, Specs::of);
        Meaning {
            specs,
            main_specs,
            dimensions,
            models,
            main: main.map(|name| canonicalize(name).into_owned()),
        }
    }
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
        let elements = element_names(&dim.elements);
        let mappings = dim
            .mappings
            .iter()
            .map(|mapping| {
                let target = canonicalize(&mapping.target).into_owned();
                let pairs: BTreeSet<(String, String)> = if mapping.element_map.is_empty() {
                    // A positional mapping pairs the elements by position.
                    let target_elements = all
                        .iter()
                        .find(|d| canonicalize(&d.name) == target)
                        .map(|d| element_names(&d.elements))
                        .unwrap_or_default();
                    elements.iter().cloned().zip(target_elements).collect()
                } else {
                    mapping
                        .element_map
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
            name: dim.name.clone(),
            elements,
            parent: dim.parent.as_deref().map(|p| canonicalize(p).into_owned()),
            mappings,
        }
    }
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

impl ModelMeaning {
    fn of(
        model: &Model,
        dimensions: &BTreeMap<String, DimensionMeaning>,
        macros: &BTreeSet<String>,
    ) -> ModelMeaning {
        let idents = |names: &[String]| -> Vec<String> {
            names.iter().map(|n| canonicalize(n).into_owned()).collect()
        };
        ModelMeaning {
            name: model.name.clone(),
            specs: model.sim_specs.as_ref().map(Specs::of),
            macro_spec: model.macro_spec.as_ref().map(|spec| MacroMeaning {
                parameters: idents(&spec.parameters),
                primary_output: canonicalize(&spec.primary_output).into_owned(),
                additional_outputs: idents(&spec.additional_outputs),
            }),
            variables: model
                .variables
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

impl VariableMeaning {
    fn of(
        var: &Variable,
        dimensions: &BTreeMap<String, DimensionMeaning>,
        macros: &BTreeSet<String>,
    ) -> VariableMeaning {
        let (kind, equation, gf, compat) = match var {
            Variable::Stock(s) => ("a stock", Some(&s.equation), None, s.compat.clone()),
            Variable::Flow(f) => ("a flow", Some(&f.equation), f.gf.as_ref(), f.compat.clone()),
            Variable::Aux(a) => (
                "an auxiliary",
                Some(&a.equation),
                a.gf.as_ref(),
                a.compat.clone(),
            ),
            Variable::Module(m) => ("a module", None, None, m.compat.clone()),
        };
        let variable_initial = compat
            .active_initial
            .as_deref()
            .map(|text| canonical_equation(text, false, macros));
        let mut meaning = VariableMeaning {
            name: var.get_ident().to_string(),
            kind,
            dims: Vec::new(),
            elements: Some(BTreeSet::from([String::new()])),
            equations: BTreeMap::new(),
            default: None,
            initials: BTreeMap::new(),
            tables: BTreeMap::new(),
            inflows: Vec::new(),
            outflows: Vec::new(),
            module: None,
            compat,
            scalar: match var {
                Variable::Aux(a) => match &a.equation {
                    // Not `canonicalize`, which respells a `.` as a module
                    // separator and so a number as no number.
                    Equation::Scalar(text) => Some(text.trim().to_lowercase().replace(' ', "_")),
                    _ => None,
                },
                _ => None,
            },
        };
        let mut defined: Vec<DefinedElement> = Vec::new();
        let whole = DefinedElement::whole;
        match (var, equation) {
            (Variable::Module(m), _) => {
                meaning.elements = None;
                meaning.module = Some((
                    canonicalize(&m.model_name).into_owned(),
                    m.references
                        .iter()
                        .map(|r| {
                            (
                                canonicalize(&r.src).into_owned(),
                                canonicalize(&r.dst).into_owned(),
                            )
                        })
                        .collect(),
                ));
            }
            (_, None) => {}
            (_, Some(Equation::Scalar(text))) => defined.push(whole(String::new(), text)),
            (_, Some(Equation::ApplyToAll(dims, text))) => {
                meaning.dims = dims.iter().map(|d| canonicalize(d).into_owned()).collect();
                meaning.elements = element_product(&meaning.dims, dimensions);
                match &meaning.elements {
                    Some(keys) => defined.extend(keys.iter().map(|key| whole(key.clone(), text))),
                    // The elements are unknown; the one equation is all
                    // there is to compare.
                    None => defined.push(whole(String::new(), text)),
                }
            }
            (_, Some(Equation::Arrayed(dims, elements, default, has_except_default))) => {
                meaning.dims = dims.iter().map(|d| canonicalize(d).into_owned()).collect();
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
                // An :EXCEPT: default fills the elements the others leave,
                // where it applies (as the compiler applies it). One that
                // does not apply is never read.
                match default.as_deref().filter(|_| *has_except_default) {
                    Some(text) => {
                        meaning.default = Some(canonical_equation(text, gf.is_some(), macros));
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
            meaning.equations.insert(
                element.key.clone(),
                canonical_equation(element.equation, table.is_some(), macros),
            );
            let initial = element
                .initial
                .map(|text| canonical_equation(text, false, macros))
                .or_else(|| variable_initial.clone());
            if let Some(initial) = initial {
                meaning.initials.insert(element.key.clone(), initial);
            }
            if let Some(table) = table {
                meaning.tables.insert(element.key, Table::of(table));
            }
        }
        if let Variable::Stock(s) = var {
            let flows = |list: &[String]| -> Vec<String> {
                crate::datamodel::distinct_stock_flows(list)
                    .flows
                    .iter()
                    .map(|f| canonicalize(f).into_owned())
                    .collect()
            };
            meaning.inflows = flows(&s.inflows);
            meaning.outflows = flows(&s.outflows);
        }
        meaning
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
fn quoted(equation: &str) -> String {
    const LONGEST: usize = 80;
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
/// that keeps it as a spec rather than a variable changes nothing: its
/// equation is the number the spec is, or (for the save step) the time step.
fn control_variable_kept(ident: &str, var: &VariableMeaning, specs: &Specs) -> bool {
    if !CONTROL_VARIABLES.contains(&ident) {
        return false;
    }
    let Some(text) = &var.scalar else {
        return false;
    };
    let value = if text == "time_step" {
        Some(specs.dt)
    } else {
        text.parse::<f64>().ok()
    };
    let spec = match ident {
        "initial_time" => specs.start,
        "final_time" => specs.stop,
        "time_step" => specs.dt,
        _ => specs.save_step,
    };
    value == Some(spec)
}

/// A structural change, whose kind the runs settle ([`settle`]).
fn structural(model: Option<String>, variable: Option<String>, reason: String) -> MeaningChange {
    MeaningChange {
        model,
        variable,
        kind: ChangeKind::Structure,
        reason,
    }
}

fn compare_specs(model: Option<&str>, a: &Specs, b: &Specs, changes: &mut Vec<MeaningChange>) {
    let mut say =
        |reason: String| changes.push(structural(model.map(str::to_string), None, reason));
    if (a.start, a.stop) != (b.start, b.stop) {
        say(format!(
            "the save simulates from {} to {}, not from {} to {}",
            b.start, b.stop, a.start, a.stop
        ));
    }
    if a.dt != b.dt {
        say(format!("the save's time step is {}, not {}", b.dt, a.dt));
    }
    if a.save_step != b.save_step {
        say(format!(
            "the save's save step is {}, not {}",
            b.save_step, a.save_step
        ));
    }
    if a.method != b.method {
        let method = |m: SimMethod| match m {
            SimMethod::Euler => "Euler's method",
            SimMethod::RungeKutta2 => "second-order Runge-Kutta",
            SimMethod::RungeKutta4 => "fourth-order Runge-Kutta",
        };
        say(format!(
            "the save integrates with {}, not {}",
            method(b.method),
            method(a.method)
        ));
    }
}

fn compare_structure(original: &Meaning, saved: &Meaning) -> Vec<MeaningChange> {
    let mut changes = Vec::new();
    compare_specs(None, &original.specs, &saved.specs, &mut changes);

    let project_change = |reason: String| structural(None, None, reason);
    for (key, dim) in &original.dimensions {
        let Some(other) = saved.dimensions.get(key) else {
            changes.push(project_change(format!(
                "dimension '{}' is not in the save",
                dim.name
            )));
            continue;
        };
        if dim.elements != other.elements {
            changes.push(project_change(format!(
                "dimension '{}' has elements {} in the save, not {}",
                dim.name,
                other.elements.join(", "),
                dim.elements.join(", ")
            )));
        }
        match (&dim.parent, &other.parent) {
            (Some(a), Some(b)) if a != b => changes.push(project_change(format!(
                "dimension '{}' is a subdimension of '{b}' in the save, not of '{a}'",
                dim.name
            ))),
            (Some(a), None) => changes.push(project_change(format!(
                "dimension '{}' is no longer a subdimension of '{a}'",
                dim.name
            ))),
            (None, Some(b)) => changes.push(project_change(format!(
                "dimension '{}' becomes a subdimension of '{b}'",
                dim.name
            ))),
            _ => {}
        }
        for (target, pairs) in &dim.mappings {
            match other.mappings.get(target) {
                None => changes.push(project_change(format!(
                    "dimension '{}' no longer maps to '{target}'",
                    dim.name
                ))),
                Some(saved_pairs) if saved_pairs != pairs => changes.push(project_change(format!(
                    "the mapping of dimension '{}' to '{target}' relates other elements in the save",
                    dim.name
                ))),
                Some(_) => {}
            }
        }
        for target in other.mappings.keys() {
            if !dim.mappings.contains_key(target) {
                changes.push(project_change(format!(
                    "dimension '{}' maps to '{target}' in the save",
                    dim.name
                )));
            }
        }
    }
    for (key, dim) in &saved.dimensions {
        if !original.dimensions.contains_key(key) {
            changes.push(project_change(format!(
                "dimension '{}' is new in the save",
                dim.name
            )));
        }
    }

    // The model a host simulates is the same model in both, whatever each
    // calls it; the others pair by name.
    let counterpart = |key: &String| -> Option<(&String, &ModelMeaning)> {
        let key = if original.main.as_ref() == Some(key) {
            saved.main.as_ref()?
        } else {
            key
        };
        saved.models.get_key_value(key)
    };
    let mut paired: BTreeSet<&String> = BTreeSet::new();
    for (key, model) in &original.models {
        let Some(other) = counterpart(key) else {
            changes.push(structural(
                Some(model.name.clone()),
                None,
                format!("model '{}' is not in the save", model.name),
            ));
            continue;
        };
        paired.insert(other.0);
        compare_model(model, other.1, original, saved, &mut changes);
    }
    for (key, model) in &saved.models {
        if !paired.contains(key) {
            changes.push(structural(
                Some(model.name.clone()),
                None,
                format!("model '{}' is new in the save", model.name),
            ));
        }
    }
    changes
}

fn compare_model(
    original: &ModelMeaning,
    saved: &ModelMeaning,
    original_project: &Meaning,
    saved_project: &Meaning,
    changes: &mut Vec<MeaningChange>,
) {
    // A model with specs of its own runs with them; one without, with the
    // project's, which the project's comparison covers.
    if original.specs.is_some() || saved.specs.is_some() {
        let a = original.specs.unwrap_or(original_project.specs);
        let b = saved.specs.unwrap_or(saved_project.specs);
        compare_specs(Some(&original.name), &a, &b, changes);
    }
    let model = || Some(original.name.clone());
    compare_macro(original, saved, changes);
    for (key, var) in &original.variables {
        let mut say =
            |reason: String| changes.push(structural(model(), Some(var.name.clone()), reason));
        let Some(other) = saved.variables.get(key) else {
            if !control_variable_kept(key, var, &saved_project.specs) {
                say(format!("'{}' is not in the save", var.name));
            }
            continue;
        };
        compare_variable(var, other, &mut say);
    }
    for (key, var) in &saved.variables {
        if !original.variables.contains_key(key)
            && !control_variable_kept(key, var, &saved_project.specs)
        {
            changes.push(structural(
                model(),
                Some(var.name.clone()),
                format!("'{}' is new in the save", var.name),
            ));
        }
    }
}

/// A macro's signature: what a call binds to which parameter, and what it
/// returns.
fn compare_macro(original: &ModelMeaning, saved: &ModelMeaning, changes: &mut Vec<MeaningChange>) {
    let name = &original.name;
    let mut say = |reason: String| changes.push(structural(Some(name.clone()), None, reason));
    let list = |names: &[String]| {
        if names.is_empty() {
            "nothing".to_string()
        } else {
            names.join(", ")
        }
    };
    match (&original.macro_spec, &saved.macro_spec) {
        (Some(_), None) => say(format!("model '{name}' is no longer a macro")),
        (None, Some(_)) => say(format!("model '{name}' becomes a macro")),
        (Some(a), Some(b)) => {
            if a.parameters != b.parameters {
                say(format!(
                    "macro '{name}' takes {} in the save, not {}",
                    list(&b.parameters),
                    list(&a.parameters)
                ));
            }
            if a.primary_output != b.primary_output {
                say(format!(
                    "macro '{name}' returns '{}' in the save, not '{}'",
                    b.primary_output, a.primary_output
                ));
            }
            if a.additional_outputs != b.additional_outputs {
                say(format!(
                    "macro '{name}' also returns {} in the save, not {}",
                    list(&b.additional_outputs),
                    list(&a.additional_outputs)
                ));
            }
        }
        (None, None) => {}
    }
}

fn compare_variable(a: &VariableMeaning, b: &VariableMeaning, say: &mut impl FnMut(String)) {
    let name = &a.name;
    if a.kind != b.kind {
        say(format!(
            "'{name}' is {} in the save, not {}",
            b.kind, a.kind
        ));
        return;
    }
    if a.dims != b.dims {
        let dims = |dims: &[String]| {
            if dims.is_empty() {
                "no dimension".to_string()
            } else {
                dims.join(", ")
            }
        };
        say(format!(
            "'{name}' is defined over {}, not {}",
            dims(&b.dims),
            dims(&a.dims)
        ));
    }
    if let (Some(ea), Some(eb)) = (&a.elements, &b.elements) {
        let gained: Vec<&String> = eb.difference(ea).collect();
        let lost: Vec<&String> = ea.difference(eb).collect();
        if !lost.is_empty() {
            say(format!("'{name}' loses {}", named("element", &lost)));
        }
        if !gained.is_empty() {
            say(format!("'{name}' gains {}", named("element", &gained)));
        }
    }
    let element = |key: &str| {
        if key.is_empty() {
            String::new()
        } else {
            format!(" for element '{key}'")
        }
    };
    // The elements both define; the others are named above.
    let differing: Vec<(&String, &String, &String)> = a
        .equations
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
        say(if a.kind == "a stock" {
            format!("'{name}' starts from {is}{at} in the save, not from {was}{more}")
        } else {
            format!("'{name}' is computed as {is}{at} in the save, not as {was}{more}")
        });
    }
    match (&a.default, &b.default) {
        (Some(was), None) => say(format!(
            "'{name}' no longer has its :EXCEPT: default {}",
            quoted(was)
        )),
        (None, Some(is)) => say(format!("'{name}' gains an :EXCEPT: default {}", quoted(is))),
        (Some(was), Some(is)) if was != is => say(format!(
            "the :EXCEPT: default of '{name}' is {} in the save, not {}",
            quoted(is),
            quoted(was)
        )),
        _ => {}
    }
    for (key, initial) in &a.initials {
        match b.initials.get(key) {
            None => say(format!("'{name}' loses its initial value{}", element(key))),
            Some(other) if other != initial => say(format!(
                "the initial value of '{name}' changes{}",
                element(key)
            )),
            Some(_) => {}
        }
    }
    for key in b.initials.keys() {
        if !a.initials.contains_key(key) {
            say(format!("'{name}' gains an initial value{}", element(key)));
        }
    }
    for (key, table) in &a.tables {
        match b.tables.get(key) {
            None => say(format!(
                "'{name}' loses its graphical function{}",
                element(key)
            )),
            Some(other) if other != table => say(format!(
                "the graphical function of '{name}' changes{}",
                element(key)
            )),
            Some(_) => {}
        }
    }
    for key in b.tables.keys() {
        if !a.tables.contains_key(key) {
            say(format!(
                "'{name}' gains a graphical function{}",
                element(key)
            ));
        }
    }
    let flows = |which: &str, a: &[String], b: &[String], say: &mut dyn FnMut(String)| {
        let lost: Vec<&String> = a.iter().filter(|f| !b.contains(f)).collect();
        let gained: Vec<&String> = b.iter().filter(|f| !a.contains(f)).collect();
        if !lost.is_empty() {
            say(format!("'{name}' loses {}", named(which, &lost)));
        }
        if !gained.is_empty() {
            say(format!("'{name}' gains {}", named(which, &gained)));
        }
        // The same flows in another order: a queue serves its outflows,
        // and a conveyor admits its inflows, in their order.
        if lost.is_empty() && gained.is_empty() && a != b {
            say(format!(
                "'{name}' takes its {which}s in another order in the save: {}, not {}",
                b.join(", "),
                a.join(", ")
            ));
        }
    };
    flows("inflow", &a.inflows, &b.inflows, say);
    flows("outflow", &a.outflows, &b.outflows, say);
    if a.module != b.module {
        match (&a.module, &b.module) {
            (Some((ma, _)), Some((mb, _))) if ma != mb => say(format!(
                "'{name}' instantiates '{mb}' in the save, not '{ma}'"
            )),
            _ => say(format!("the inputs of module '{name}' change")),
        }
    }
    let (ca, cb) = (&a.compat, &b.compat);
    let flag = |was: bool, is: bool, what: &str, say: &mut dyn FnMut(String)| {
        if was && !is {
            say(format!("'{name}' is no longer {what}"));
        } else if is && !was {
            say(format!("'{name}' becomes {what}"));
        }
    };
    flag(ca.non_negative, cb.non_negative, "non-negative", say);
    flag(
        ca.conveyor.is_some(),
        cb.conveyor.is_some(),
        "a conveyor",
        say,
    );
    flag(ca.queue.is_some(), cb.queue.is_some(), "a queue", say);
    flag(
        ca.leakage.is_some(),
        cb.leakage.is_some(),
        "a conveyor leak",
        say,
    );
    flag(
        ca.spreadflow.is_some(),
        cb.spreadflow.is_some(),
        "a conveyor inflow with its own placement",
        say,
    );
    flag(ca.overflow, cb.overflow, "a queue's overflow", say);
    flag(
        ca.data_source.is_some(),
        cb.data_source.is_some(),
        "read from data",
        say,
    );
    flag(
        ca.can_be_module_input,
        cb.can_be_module_input,
        "a module input",
        say,
    );
    if ca.conveyor.is_some() && cb.conveyor.is_some() && ca.conveyor != cb.conveyor {
        say(format!("the conveyor '{name}' changes"));
    }
    if ca.queue.is_some() && cb.queue.is_some() && ca.queue != cb.queue {
        say(format!("the queue '{name}' changes"));
    }
    if let (Some(la), Some(lb)) = (&ca.leakage, &cb.leakage) {
        if (&la.zone_start, &la.zone_end) != (&lb.zone_start, &lb.zone_end) {
            say(format!("the zone the leak '{name}' drains changes"));
        }
        if la.integers != lb.integers {
            let whole = |integers: bool| {
                if integers {
                    "whole units"
                } else {
                    "any amount"
                }
            };
            say(format!(
                "the leak '{name}' drains {} in the save, not {}",
                whole(lb.integers),
                whole(la.integers)
            ));
        }
        if la.fraction != lb.fraction {
            say(format!("the fraction the leak '{name}' drains changes"));
        }
    }
    if ca.spreadflow.is_some() && cb.spreadflow.is_some() && ca.spreadflow != cb.spreadflow {
        say(format!(
            "how the inflow '{name}' spreads over its conveyor changes"
        ));
    }
    if ca.data_source.is_some() && cb.data_source.is_some() && ca.data_source != cb.data_source {
        say(format!("the data source of '{name}' changes"));
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

/// The variable of the simulated model a results column belongs to: its
/// name before any subscript, and for a module instance's column
/// (`hares·births`) the instance. A column the compiler adds for itself (a
/// builtin's or a conveyor's helper, `$⁚s1⁚0⁚smth1·flow` or `$conv$belt$len`)
/// belongs to none: what it holds shows in the variables it feeds, and a save
/// may compute them without it.
fn column_owner<'a>(column: &'a str, variables: &BTreeSet<String>) -> Option<&'a str> {
    if column.starts_with('$') {
        return None;
    }
    let base = column.split('[').next().unwrap_or(column);
    let owner = base.split('\u{b7}').next().unwrap_or(base);
    variables.contains(owner).then_some(owner)
}

/// What running the project and its save shows.
enum Runs {
    /// Both simulate over the same specs: each variable of the simulated
    /// model whose series differ, with the first difference; the variables
    /// with series in either run; and why every series differs when the
    /// save simulates another number of steps.
    Compared {
        differing: BTreeMap<String, String>,
        simulated: BTreeSet<String>,
        steps: Option<String>,
    },
    /// Both simulate, over other specs: every series differs, and the
    /// structure says how the specs do.
    OtherSpecs,
    /// The project simulates and its save does not, for this reason.
    SaveFails(String),
    /// The save simulates and the project does not.
    ProjectFails,
    /// Neither simulates.
    NeitherSimulates,
}

fn compare_runs(original: &Project, saved: &Project, specs_change: bool) -> Runs {
    match (simulate(original), simulate(saved)) {
        (Ok(_), Ok(_)) if specs_change => Runs::OtherSpecs,
        (Ok(a), Ok(b)) => {
            let variables = &main_variables(original) | &main_variables(saved);
            compare_series(&a, &b, &variables)
        }
        (Ok(_), Err(why)) => Runs::SaveFails(why),
        (Err(_), Ok(_)) => Runs::ProjectFails,
        (Err(_), Err(_)) => Runs::NeitherSimulates,
    }
}

/// Settle each structural change's kind by what the runs show, and add what
/// only the runs show: the variables whose series differ though the
/// structure names nothing about them, or why the save does not simulate.
fn settle(
    original: &Project,
    saved: &Project,
    mut changes: Vec<MeaningChange>,
    runs: Runs,
) -> Vec<MeaningChange> {
    let main = main_model(original).map(str::to_string);
    let project_change = |reason: String| MeaningChange {
        model: None,
        variable: None,
        kind: ChangeKind::Results,
        reason,
    };
    let all_results = |changes: &mut Vec<MeaningChange>| {
        for change in changes.iter_mut() {
            change.kind = ChangeKind::Results;
        }
    };
    match runs {
        Runs::Compared {
            steps: Some(reason),
            ..
        } => {
            all_results(&mut changes);
            changes.push(MeaningChange {
                model: main,
                ..project_change(reason)
            });
        }
        Runs::Compared {
            differing,
            simulated,
            steps: None,
        } => {
            // A change about one variable of the simulated model shows in
            // the results when that variable's series do. Any other, and one
            // about a variable with no series of its own (a table, which
            // shows only in what reads it), shows when any series does.
            let about = |change: &MeaningChange| {
                change
                    .variable
                    .as_deref()
                    .filter(|_| change.model == main)
                    .map(|v| canonicalize(v).into_owned())
            };
            let named: BTreeSet<String> = changes.iter().filter_map(about).collect();
            for change in changes.iter_mut() {
                let shows = match about(change) {
                    Some(v) if simulated.contains(&v) => differing.contains_key(&v),
                    _ => !differing.is_empty(),
                };
                if shows {
                    change.kind = ChangeKind::Results;
                }
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
        }
        Runs::OtherSpecs => all_results(&mut changes),
        Runs::SaveFails(why) => {
            // The save's own errors say why, unless the structure already
            // does.
            let unexplained = changes.is_empty();
            all_results(&mut changes);
            changes.push(project_change(format!("the save does not simulate: {why}")));
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
        }
        Runs::ProjectFails => {
            all_results(&mut changes);
            changes.push(project_change(
                "the save simulates, where the model does not".to_string(),
            ));
        }
        Runs::NeitherSimulates => changes.extend(compare_errors(original, saved)),
    }
    changes
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
    let mut changes = Vec::new();
    for (model, variable, error) in b.difference(&a) {
        let reason = match variable {
            Some(var) => {
                format!("'{var}' has an error in the save that the model does not: {error}")
            }
            None => format!("the save has an error the model does not: {error}"),
        };
        changes.push(structural(model.clone(), variable.clone(), reason));
    }
    for (model, variable, error) in a.difference(&b) {
        let reason = match variable {
            Some(var) => format!("'{var}' no longer has the model's error: {error}"),
            None => format!("the save no longer has the model's error: {error}"),
        };
        changes.push(structural(model.clone(), variable.clone(), reason));
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
) -> Runs {
    if original.step_count != saved.step_count {
        return Runs::Compared {
            differing: BTreeMap::new(),
            simulated: BTreeSet::new(),
            steps: Some(format!(
                "the save simulates {} steps, not {}",
                saved.step_count, original.step_count
            )),
        };
    }
    let columns = |results: &crate::Results| -> BTreeMap<String, usize> {
        results
            .offsets
            .iter()
            .map(|(name, &at)| (name.as_str().to_string(), at))
            .collect()
    };
    let (before, after) = (columns(original), columns(saved));
    let time_at = before.get("time").copied();
    let owner = |column: &str| {
        column_owner(column, variables)
            .filter(|owner| !CONTROL_VARIABLES.contains(owner))
            .map(str::to_string)
    };

    let simulated: BTreeSet<String> = before
        .keys()
        .chain(after.keys())
        .filter_map(|name| owner(name))
        .collect();
    let mut differing: BTreeMap<String, String> = BTreeMap::new();
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
            let when = time_at
                .and_then(|t| series(original, t).nth(step))
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
    Runs::Compared {
        differing,
        simulated,
        steps: None,
    }
}
