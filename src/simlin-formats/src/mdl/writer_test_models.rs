// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Models generated for the MDL writer's properties, and the property they
//! hold: a first save keeps the model and is a fixed point. Here rather than in
//! the tests because the models with a diagram are saved from simlin-tests,
//! which can lay one out.

use super::*;
use crate::mdl::{parse_mdl, project_to_mdl, project_to_mdl_with_warnings};
use proptest::prelude::*;
use simlin_base::ast::print_eqn;
use simlin_base::builtins::UntypedBuiltinFn;
use simlin_base::datamodel::{
    self, Aux, Compat, Dimension, DimensionElements, Dt, Equation, Flow, GraphicalFunction,
    GraphicalFunctionKind, GraphicalFunctionScale, Model, Project, SimMethod, SimSpecs, Stock,
    Variable,
};
use std::collections::BTreeMap;

fn empty_sim_specs() -> SimSpecs {
    SimSpecs {
        start: 0.0,
        stop: 100.0,
        dt: Dt::Dt(1.0),
        save_step: None,
        sim_method: SimMethod::Euler,
        time_units: None,
    }
}

pub(crate) fn project_of(model: Model, dimensions: Vec<Dimension>) -> Project {
    Project {
        name: "prop".to_owned(),
        sim_specs: empty_sim_specs(),
        dimensions,
        units: vec![],
        models: vec![model],
        source: None,
        ai_information: None,
    }
}

pub(crate) fn model_of(variables: Vec<Variable>) -> Model {
    Model {
        name: "main".to_owned(),
        sim_specs: None,
        variables: variables.into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    }
}

pub(crate) fn named_dim(name: &str, elems: &[&str]) -> Dimension {
    Dimension {
        name: name.to_owned(),
        elements: DimensionElements::Named(elems.iter().map(|e| (*e).to_owned()).collect()),
        mappings: vec![],
        parent: None,
    }
}

// ---- whole-model generator (property 4) ----

/// Numbers whose shortest spelling is awkward: tiny, huge, subnormal, not
/// representable in decimal, and one past where f64 counts integers.
const AWKWARD_NUMBERS: &[f64] = &[
    1e-7,
    0.1 + 0.2,
    1e21,
    5e-324,
    f64::MAX,
    2.5e-5,
    123_456_789.123,
    1.0 / 3.0,
    4.35,
    1e15 + 1.0,
    -2.5,
    0.0,
];

/// A number's text, as Rust spells an f64 shortest.
fn number_text() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(AWKWARD_NUMBERS).prop_map(|v| format!("{v}")),
        any::<f64>()
            .prop_filter("finite", |v| v.is_finite())
            .prop_map(|v| format!("{v}")),
    ]
}

/// An element's equation: a number, or one that reads the model.
fn element_equation() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => number_text(),
        1 => Just("TIME".to_owned()),
        1 => Just("alpha * 2 + 1".to_owned()),
    ]
}

const DIM_A: &[&str] = &["a1", "a2", "a3"];
const SUB_A: &[&str] = &["a2", "a3"];
const DIM_B: &[&str] = &["b1", "b2"];

/// The element keys of each choice of dimensions an arrayed variable is
/// over: `DimA`, its subrange `SubA`, or `DimA` by `DimB`.
fn arrayed_shape() -> impl Strategy<Value = (Vec<String>, Vec<String>)> {
    prop_oneof![
        Just((
            vec!["DimA".to_owned()],
            DIM_A.iter().map(|e| e.to_string()).collect()
        )),
        Just((
            vec!["SubA".to_owned()],
            SUB_A.iter().map(|e| e.to_string()).collect()
        )),
        Just((
            vec!["DimA".to_owned(), "DimB".to_owned()],
            DIM_A
                .iter()
                .flat_map(|a| DIM_B.iter().map(move |b| format!("{a},{b}")))
                .collect()
        )),
    ]
}

/// An arrayed variable's equation: some or all of its elements, each with
/// its own equation and maybe its own initial (an element's ACTIVE INITIAL,
/// so elements that agree on the equation can differ in it), and maybe an
/// `:EXCEPT:` default, applied or not.
fn arrayed_equation() -> impl Strategy<Value = Equation> {
    arrayed_shape()
        .prop_flat_map(|(dims, keys)| {
            let n = keys.len();
            (
                Just(dims),
                Just(keys),
                prop::collection::vec(any::<bool>(), n),
                prop::collection::vec(element_equation(), n),
                prop::collection::vec(prop::option::weighted(0.25, number_text()), n),
                prop::option::of(number_text()),
                any::<bool>(),
            )
        })
        .prop_map(
            |(dims, keys, kept, equations, initials, default, applies)| {
                let mut slots: Vec<_> = keys
                    .into_iter()
                    .zip(kept)
                    .zip(equations.into_iter().zip(initials))
                    .filter(|((_, kept), _)| *kept)
                    .map(|((key, _), (eqn, initial))| (key, eqn, initial, None))
                    .collect();
                if slots.is_empty() {
                    slots.push((
                        match dims[0].as_str() {
                            "SubA" => "a2".to_owned(),
                            _ if dims.len() == 2 => "a1,b1".to_owned(),
                            _ => "a1".to_owned(),
                        },
                        "1".to_owned(),
                        None,
                        None,
                    ));
                }
                let applies = applies && default.is_some();
                Equation::Arrayed(dims, slots, default, applies)
            },
        )
}

fn table(ys: Vec<f64>) -> GraphicalFunction {
    let n = ys.len();
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some((0..n).map(|i| i as f64).collect()),
        y_points: ys,
        x_scale: GraphicalFunctionScale {
            min: 0.0,
            max: (n - 1) as f64,
        },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

fn aux(ident: &str, equation: Equation, gf: Option<GraphicalFunction>) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_owned(),
        equation,
        documentation: String::new(),
        units: None,
        gf,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn flow(ident: &str, equation: Equation) -> Variable {
    Variable::Flow(Flow {
        ident: ident.to_owned(),
        equation,
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

fn stock(ident: &str, equation: Equation, inflows: &[&str], outflows: &[&str]) -> Variable {
    Variable::Stock(Stock {
        ident: ident.to_owned(),
        equation,
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|f| f.to_string()).collect(),
        outflows: outflows.iter().map(|f| f.to_string()).collect(),
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    })
}

/// A model with a bit of everything an MDL save writes: awkward numbers,
/// arrayed variables over a dimension, a subrange and two dimensions (some
/// elements defined, an `:EXCEPT:` default or not), a scalar and an arrayed
/// stock with their flows, a lookup table and its call, a WITH LOOKUP,
/// groups and a time unit or none.
pub fn rich_model() -> impl Strategy<Value = Project> {
    (
        number_text(),
        arrayed_equation(),
        arrayed_equation(),
        number_text(),
        prop::collection::vec(prop::sample::select(AWKWARD_NUMBERS), 2..5),
        prop::option::of(prop::sample::subsequence(
            vec!["alpha", "level", "fill", "tbl", "arr"],
            1..4,
        )),
        prop::option::of(Just("Months".to_owned())),
    )
        .prop_map(|(alpha, arr, arr2, level, ys, grouped, time_units)| {
            let ys: Vec<f64> = ys.into_iter().map(|y| y.clamp(-1e300, 1e300)).collect();
            let variables = vec![
                aux("alpha", Equation::Scalar(alpha), None),
                aux("arr", arr, None),
                aux("arr2", arr2, None),
                stock("level", Equation::Scalar(level), &["fill"], &["drain"]),
                flow("fill", Equation::Scalar("alpha".to_owned())),
                flow("drain", Equation::Scalar("level / 4".to_owned())),
                stock(
                    "pool",
                    Equation::Arrayed(
                        vec!["DimA".to_owned()],
                        DIM_A
                            .iter()
                            .enumerate()
                            .map(|(i, e)| (e.to_string(), (i + 1).to_string(), None, None))
                            .collect(),
                        None,
                        false,
                    ),
                    &["pour"],
                    &[],
                ),
                flow(
                    "pour",
                    Equation::ApplyToAll(vec!["DimA".to_owned()], "1".to_owned()),
                ),
                aux(
                    "tbl",
                    Equation::Scalar(String::new()),
                    Some(table(ys.clone())),
                ),
                aux(
                    "looked up",
                    Equation::Scalar("LOOKUP(tbl, TIME)".to_owned()),
                    None,
                ),
                aux(
                    "with lookup",
                    Equation::Scalar("TIME".to_owned()),
                    Some(table(ys)),
                ),
            ];
            let mut model = model_of(variables);
            if let Some(members) = grouped {
                model.groups = vec![datamodel::ModelGroup {
                    name: "Sector".to_owned(),
                    doc: None,
                    parent: None,
                    members: members.into_iter().map(str::to_owned).collect(),
                    run_enabled: false,
                }];
            }
            let mut project = project_of(
                model,
                vec![
                    named_dim("DimA", DIM_A),
                    named_dim("SubA", SUB_A),
                    named_dim("DimB", DIM_B),
                ],
            );
            project.sim_specs.time_units = time_units;
            project
        })
}

/// What each element of each variable of `project` is: its kind, and per
/// element (canonical element names, in its dimensions' order; none for a
/// scalar) the equation, initial and table that define it -- a slot's, or the
/// `:EXCEPT:` default where it applies to an element no slot holds. An element
/// nothing defines is not in the map.
type Elements = BTreeMap<Vec<String>, (String, Option<String>, Option<GraphicalFunction>)>;

fn defined_elements(project: &Project) -> BTreeMap<String, (&'static str, Vec<String>, Elements)> {
    let elements_of = |dim: &str| -> Vec<String> {
        project
            .dimensions
            .iter()
            .find(|d| {
                simlin_base::common::canonicalize(&d.name) == simlin_base::common::canonicalize(dim)
            })
            .map(|d| match &d.elements {
                DimensionElements::Named(names) => names
                    .iter()
                    .map(|e| simlin_base::common::canonicalize(e).into_owned())
                    .collect(),
                DimensionElements::Indexed(n) => (1..=*n).map(|i| i.to_string()).collect(),
            })
            .unwrap_or_default()
    };
    let product = |dims: &[String]| -> Vec<Vec<String>> {
        dims.iter().fold(vec![Vec::new()], |keys, dim| {
            keys.into_iter()
                .flat_map(|key| {
                    elements_of(dim).into_iter().map(move |e| {
                        let mut next = key.clone();
                        next.push(e);
                        next
                    })
                })
                .collect()
        })
    };
    let key_of = |key: &str| -> Vec<String> {
        key.split(',')
            .map(|p| simlin_base::common::canonicalize(p.trim()).into_owned())
            .collect()
    };
    let mut out = BTreeMap::new();
    for var in project.models[0].variables.iter() {
        let (kind, gf, compat) = match var {
            Variable::Stock(s) => ("stock", None, &s.compat),
            Variable::Flow(f) => ("flow", f.gf.clone(), &f.compat),
            Variable::Aux(a) => ("aux", a.gf.clone(), &a.compat),
            Variable::Module(_) => continue,
        };
        // A two-argument INIT is an ACTIVE INITIAL, which the reader keeps as
        // the equation and its initial.
        let active = |eqn: &str| -> (String, Option<String>) {
            if let Ok(Some(Expr0::App(UntypedBuiltinFn(f, args), _))) =
                Expr0::new(eqn, simlin_base::lexer::LexerType::Equation)
                && f == "init"
                && let [equation, initial] = &args[..]
            {
                return (print_eqn(equation), Some(print_eqn(initial)));
            }
            (eqn.to_owned(), compat.active_initial.clone())
        };
        let mut elements = Elements::new();
        match var.get_equation() {
            Some(Equation::Scalar(eqn)) => {
                let (eqn, initial) = active(eqn);
                elements.insert(Vec::new(), (eqn, initial, gf));
            }
            Some(Equation::ApplyToAll(dims, eqn)) => {
                let (eqn, initial) = active(eqn);
                for key in product(dims) {
                    elements.insert(key, (eqn.clone(), initial.clone(), gf.clone()));
                }
            }
            Some(Equation::Arrayed(dims, slots, default, applies)) => {
                for (key, eqn, initial, slot_gf) in slots {
                    elements.insert(key_of(key), (eqn.clone(), initial.clone(), slot_gf.clone()));
                }
                if let Some(default) = default.as_ref().filter(|_| *applies) {
                    for key in product(dims) {
                        elements
                            .entry(key)
                            .or_insert_with(|| (default.clone(), None, None));
                    }
                }
            }
            None => {}
        }
        let dims: Vec<String> = match var.get_equation() {
            Some(Equation::ApplyToAll(dims, _)) | Some(Equation::Arrayed(dims, ..)) => dims
                .iter()
                .map(|d| simlin_base::common::canonicalize(d).into_owned())
                .collect(),
            _ => Vec::new(),
        };
        out.insert(
            simlin_base::common::canonicalize(var.get_ident()).into_owned(),
            (kind, dims, elements),
        );
    }
    out
}

/// Whether `read` defines what `generated` does: the same variables, each of
/// the same kind, over the same dimensions by name unless the save warns
/// that it is read back over others, with the same elements, each the same equation (numbers by
/// value, names canonically), initial and table. Err with the first
/// difference.
fn reads_back_as_generated(
    generated: &Project,
    read: &Project,
    warnings: &[super::ExportWarning],
) -> std::result::Result<(), String> {
    use super::arrayed::same_equation;
    let (a, b) = (defined_elements(generated), defined_elements(read));
    for (ident, (kind, dims, elements)) in &a {
        let Some((read_kind, read_dims, read_elements)) = b.get(ident) else {
            return Err(format!("'{ident}' does not read back"));
        };
        if kind != read_kind {
            return Err(format!(
                "'{ident}' is a {kind} and reads back as a {read_kind}"
            ));
        }
        // A variable over some elements of a dimension reads back over the
        // dimension of exactly those, and the save says so.
        let warned = warnings.iter().any(|w| {
            w.message.contains(&format!("'{ident}'")) && w.message.contains("read back over")
        });
        if dims != read_dims && !warned {
            return Err(format!(
                "'{ident}' is over {dims:?} and reads back over {read_dims:?}"
            ));
        }
        let keys = |e: &Elements| e.keys().cloned().collect::<Vec<_>>();
        if keys(elements) != keys(read_elements) {
            return Err(format!(
                "'{ident}' defines {:?} and reads back defining {:?}",
                keys(elements),
                keys(read_elements)
            ));
        }
        for (key, (eqn, initial, gf)) in elements {
            let (read_eqn, read_initial, read_gf) = &read_elements[key];
            let same_initial = match (initial, read_initial) {
                (Some(x), Some(y)) => same_equation(x, y),
                (None, None) => true,
                _ => false,
            };
            if !same_equation(eqn, read_eqn) || !same_initial || gf != read_gf {
                return Err(format!(
                    "'{ident}'{key:?} is {eqn:?} (initial {initial:?}) and reads back as \
                     {read_eqn:?} (initial {read_initial:?})"
                ));
            }
        }
    }
    if let Some(extra) = b.keys().find(|ident| !a.contains_key(*ident)) {
        return Err(format!("'{extra}' is new in the save"));
    }
    Ok(())
}

/// The first save of `project` keeps it and is a fixed point: it reads back
/// as what `project` defines (`reads_back_as_generated`), and as a project
/// whose save is the same text, and the project read back from that is the
/// one read back from the first (`save_roundtrip_tests::normalized`). Err
/// with what differs.
pub fn first_save_is_a_fixed_point(project: &Project) -> std::result::Result<(), String> {
    use crate::mdl::save_compare::{first_difference, normalized};
    // The reader keeps one equation where a variable's elements say one
    // thing (`element_equations`), so the generated model is taken in the
    // form an open of it gives: the save of that form is the fixed point.
    let mut project = project.clone();
    crate::element_equations::compact_element_equations(&mut project);
    let project = &project;
    let (save1, warnings) =
        project_to_mdl_with_warnings(project).map_err(|e| format!("first write: {e}"))?;
    let read1 = parse_mdl(&save1).map_err(|e| format!("first read: {e}\n{save1}"))?;
    reads_back_as_generated(project, &read1, &warnings).map_err(|why| format!("{why}\n{save1}"))?;
    let unheld = crate::mdl::subscript_rule::ranges_not_on_the_left(&save1, &read1);
    if !unheld.is_empty() {
        return Err(format!(
            "the save names a subscript range its left-hand side does not hold: {}\n{save1}",
            unheld.join("; ")
        ));
    }
    let save2 = project_to_mdl(&read1).map_err(|e| format!("second write: {e}"))?;
    if save1 != save2 {
        return Err(format!(
            "the save is not a fixed point:\n{save1}\n----\n{save2}"
        ));
    }
    let read2 = parse_mdl(&save2).map_err(|e| format!("second read: {e}"))?;
    match first_difference(&normalized(&read1), &normalized(&read2)) {
        Some(change) => Err(format!("{change}\n{save1}")),
        None => Ok(()),
    }
}
