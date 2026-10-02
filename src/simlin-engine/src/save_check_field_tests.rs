// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Every field of every `datamodel` type the structure walk reads, each
//! edited in a project that holds one of everything: a field that is part of
//! what a model means is named when it changes, and a field that is not
//! changes nothing. The field lists are destructurings with no `..`, so a
//! field added to a type does not compile here until it is listed, and a
//! listed field with no row fails the test.

use std::collections::HashMap;

use super::structure;
use crate::datamodel::{
    AiInformation, AiState, AiStatus, Aux, Compat, Conveyor, DataSource, DataSourceKind, Dimension,
    DimensionElements, DimensionMapping, Dt, Equation, Extension, Flow, GraphicalFunction,
    GraphicalFunctionKind, GraphicalFunctionScale, Leakage, LoopMetadata, MacroSpec, Model,
    ModelGroup, Module, ModuleReference, Project, Queue, Rect, SimMethod, SimSpecs, Source,
    SpreadFlow, Stock, StockFlow, Unit, Variable, View, Visibility,
};

/// The names of a type's fields, from a destructuring of `$value` with no
/// `..`.
macro_rules! fields {
    ($ty:ident { $($field:ident),* $(,)? } = $value:expr) => {{
        let $ty { $($field: _),* } = $value;
        (stringify!($ty), vec![$(stringify!($field)),*])
    }};
}

fn table() -> GraphicalFunction {
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 10.0]),
        y_points: vec![5.0, 15.0],
        x_scale: GraphicalFunctionScale {
            min: 0.0,
            max: 10.0,
        },
        y_scale: GraphicalFunctionScale {
            min: 0.0,
            max: 20.0,
        },
    }
}

fn stock(ident: &str, inflows: &[&str], outflows: &[&str], compat: Compat) -> Variable {
    Variable::Stock(Stock {
        ident: ident.to_string(),
        equation: Equation::Scalar("0".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|f| f.to_string()).collect(),
        outflows: outflows.iter().map(|f| f.to_string()).collect(),
        ai_state: None,
        uid: None,
        compat,
    })
}

fn flow(ident: &str, equation: &str, compat: Compat) -> Variable {
    Variable::Flow(Flow {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat,
    })
}

fn aux(ident: &str, equation: Equation, gf: Option<GraphicalFunction>, compat: Compat) -> Variable {
    Variable::Aux(Aux {
        ident: ident.to_string(),
        equation,
        documentation: String::new(),
        units: None,
        gf,
        ai_state: None,
        uid: None,
        compat,
    })
}

fn scalar(text: &str) -> Equation {
    Equation::Scalar(text.to_string())
}

fn model(name: &str, variables: Vec<Variable>) -> Model {
    Model {
        name: name.to_string(),
        sim_specs: None,
        variables: variables.into(),
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    }
}

fn specs(stop: f64) -> SimSpecs {
    SimSpecs {
        start: 0.0,
        stop,
        dt: Dt::Dt(1.0),
        save_step: None,
        sim_method: SimMethod::Euler,
        time_units: Some("month".to_string()),
    }
}

/// A project holding one of everything the structure reads: dimensions with
/// a mapping and a parent, a conveyor with a leak and a placed inflow, a
/// queue with an overflow, a table, a data variable, a module input, an
/// `ACTIVE INITIAL`, an arrayed variable with an `:EXCEPT:` default, an
/// apply-to-all one, a module instance, a model with specs of its own and a
/// macro. It is compared, never compiled.
fn fixture() -> Project {
    let named = |name: &str, elements: &[&str]| {
        Dimension::named(
            name.to_string(),
            elements.iter().map(|e| e.to_string()).collect(),
        )
    };
    let main = model(
        "main",
        vec![
            stock(
                "belt",
                &["arriving"],
                &["leaving", "leaking"],
                Compat {
                    non_negative: true,
                    conveyor: Some(Conveyor {
                        transit_time: "2".to_string(),
                        capacity: Some("100".to_string()),
                        inflow_limit: Some("50".to_string()),
                        sample: Some("1".to_string()),
                        arrest: Some("0".to_string()),
                        discrete: false,
                        batch_integrity: false,
                        one_at_a_time: true,
                        exponential_leak: false,
                        ignore_earlier_zone_losses: false,
                    }),
                    ..Compat::default()
                },
            ),
            flow(
                "arriving",
                "10",
                Compat {
                    spreadflow: Some(SpreadFlow::Even),
                    ..Compat::default()
                },
            ),
            flow("leaving", "", Compat::default()),
            flow(
                "leaking",
                "",
                Compat {
                    leakage: Some(Leakage {
                        fraction: Some("0.1".to_string()),
                        integers: false,
                        zone_start: Some("0".to_string()),
                        zone_end: Some("1".to_string()),
                    }),
                    ..Compat::default()
                },
            ),
            stock(
                "line",
                &["leaving"],
                &["served", "balking"],
                Compat {
                    queue: Some(Queue {}),
                    ..Compat::default()
                },
            ),
            flow("served", "1", Compat::default()),
            flow(
                "balking",
                "0",
                Compat {
                    overflow: true,
                    ..Compat::default()
                },
            ),
            aux("effect", scalar("TIME"), Some(table()), Compat::default()),
            aux(
                "ext",
                scalar("0"),
                None,
                Compat {
                    data_source: Some(DataSource {
                        kind: DataSourceKind::Data,
                        file: "data.csv".to_string(),
                        tab_or_delimiter: ",".to_string(),
                        row_or_col: "A".to_string(),
                        cell: "B2".to_string(),
                    }),
                    ..Compat::default()
                },
            ),
            aux(
                "port",
                scalar("1"),
                None,
                Compat {
                    can_be_module_input: true,
                    ..Compat::default()
                },
            ),
            aux(
                "started",
                scalar("port * 2"),
                None,
                Compat {
                    active_initial: Some("5".to_string()),
                    ..Compat::default()
                },
            ),
            aux(
                "pop",
                Equation::Arrayed(
                    vec!["Region".to_string()],
                    vec![
                        (
                            "north".to_string(),
                            "1".to_string(),
                            Some("0".to_string()),
                            Some(table()),
                        ),
                        ("south".to_string(), "2".to_string(), None, None),
                    ],
                    Some("9".to_string()),
                    true,
                ),
                None,
                Compat::default(),
            ),
            aux(
                "every",
                Equation::ApplyToAll(vec!["Region".to_string()], "pop[Region] * 2".to_string()),
                None,
                Compat::default(),
            ),
            Variable::Module(Module {
                ident: "sub".to_string(),
                model_name: "inner".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![ModuleReference {
                    src: "port".to_string(),
                    dst: "sub.input".to_string(),
                }],
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }),
        ],
    );
    let inner = Model {
        sim_specs: Some(specs(5.0)),
        ..model(
            "inner",
            vec![aux("input", scalar("0"), None, Compat::default())],
        )
    };
    let double = Model {
        macro_spec: Some(MacroSpec {
            parameters: vec!["input".to_string(), "factor".to_string()],
            primary_output: "double".to_string(),
            additional_outputs: vec!["half".to_string()],
        }),
        ..model(
            "double",
            vec![
                aux("input", scalar("0"), None, Compat::default()),
                aux("factor", scalar("0"), None, Compat::default()),
                aux("double", scalar("input * factor"), None, Compat::default()),
                aux("half", scalar("input / factor"), None, Compat::default()),
            ],
        )
    };
    Project {
        name: "fixture".to_string(),
        sim_specs: specs(10.0),
        dimensions: vec![
            Dimension {
                mappings: vec![DimensionMapping {
                    target: "Area".to_string(),
                    element_map: vec![
                        ("north".to_string(), "a1".to_string()),
                        ("south".to_string(), "a2".to_string()),
                    ],
                }],
                ..named("Region", &["north", "south"])
            },
            named("Area", &["a1", "a2"]),
            Dimension::indexed("Full".to_string(), 3),
            Dimension {
                parent: Some("Full".to_string()),
                ..Dimension::indexed("Sub".to_string(), 2)
            },
        ],
        units: Vec::new(),
        models: vec![main, inner, double],
        source: None,
        ai_information: None,
    }
}

/// The field lists of the types the walk destructures, each from a value the
/// fixture holds.
fn field_lists(project: &Project) -> Vec<(&'static str, Vec<&'static str>)> {
    let main = &project.models[0];
    let variable = |name: &str| {
        main.get_variable(name)
            .expect("the fixture holds it")
            .clone()
    };
    let (Variable::Stock(belt), Variable::Flow(leaking), Variable::Aux(effect), Variable::Aux(ext)) = (
        variable("belt"),
        variable("leaking"),
        variable("effect"),
        variable("ext"),
    ) else {
        unreachable!("the fixture's belt, leak, table and data variable");
    };
    let Variable::Module(sub) = variable("sub") else {
        unreachable!("the fixture's module instance");
    };
    let gf = effect.gf.clone().expect("effect holds a table");
    vec![
        fields!(
            Project {
                name,
                sim_specs,
                dimensions,
                units,
                models,
                source,
                ai_information
            } = project.clone()
        ),
        fields!(
            SimSpecs {
                start,
                stop,
                dt,
                save_step,
                sim_method,
                time_units
            } = project.sim_specs.clone()
        ),
        fields!(
            Dimension {
                name,
                elements,
                mappings,
                parent
            } = project.dimensions[0].clone()
        ),
        fields!(
            DimensionMapping {
                target,
                element_map
            } = project.dimensions[0].mappings[0].clone()
        ),
        fields!(
            Model {
                name,
                sim_specs,
                variables,
                views,
                loop_metadata,
                groups,
                macro_spec
            } = main.clone()
        ),
        fields!(
            MacroSpec {
                parameters,
                primary_output,
                additional_outputs
            } = project.models[2]
                .macro_spec
                .clone()
                .expect("double is a macro")
        ),
        fields!(
            Stock {
                ident,
                equation,
                documentation,
                units,
                inflows,
                outflows,
                ai_state,
                uid,
                compat
            } = belt.clone()
        ),
        fields!(
            Flow {
                ident,
                equation,
                documentation,
                units,
                gf,
                ai_state,
                uid,
                compat
            } = leaking.clone()
        ),
        fields!(
            Aux {
                ident,
                equation,
                documentation,
                units,
                gf,
                ai_state,
                uid,
                compat
            } = effect
        ),
        fields!(
            Module {
                ident,
                model_name,
                documentation,
                units,
                references,
                ai_state,
                uid,
                compat
            } = sub.clone()
        ),
        fields!(ModuleReference { src, dst } = sub.references[0].clone()),
        fields!(
            Compat {
                active_initial,
                non_negative,
                can_be_module_input,
                visibility,
                data_source,
                conveyor,
                leakage,
                spreadflow,
                queue,
                overflow
            } = belt.compat.clone()
        ),
        fields!(
            Conveyor {
                transit_time,
                capacity,
                inflow_limit,
                sample,
                arrest,
                discrete,
                batch_integrity,
                one_at_a_time,
                exponential_leak,
                ignore_earlier_zone_losses
            } = belt.compat.conveyor.expect("belt is a conveyor")
        ),
        fields!(
            Leakage {
                fraction,
                integers,
                zone_start,
                zone_end
            } = leaking.compat.leakage.expect("leaking is a leak")
        ),
        fields!(
            DataSource {
                kind,
                file,
                tab_or_delimiter,
                row_or_col,
                cell
            } = ext.compat.data_source.expect("ext reads data")
        ),
        fields!(
            GraphicalFunction {
                kind,
                x_points,
                y_points,
                x_scale,
                y_scale
            } = gf.clone()
        ),
        fields!(GraphicalFunctionScale { min, max } = gf.x_scale),
        ("Equation", equation_parts()),
    ]
}

/// The parts of each `Equation` variant, by matches with no `..`: a part
/// added to a variant does not compile here until it is listed.
fn equation_parts() -> Vec<&'static str> {
    let mut parts = Vec::new();
    for equation in [
        scalar("0"),
        Equation::ApplyToAll(Vec::new(), String::new()),
        Equation::Arrayed(
            Vec::new(),
            vec![(String::new(), String::new(), None, None)],
            None,
            false,
        ),
    ] {
        match equation {
            Equation::Scalar(_text) => parts.push("scalar_text"),
            Equation::ApplyToAll(_dims, _text) => {
                parts.extend(["apply_to_all_dims", "apply_to_all_text"])
            }
            Equation::Arrayed(_dims, elements, _default, _applies) => {
                parts.extend(["arrayed_dims", "arrayed_default", "arrayed_applies"]);
                for (_key, _text, _initial, _table) in elements {
                    parts.extend([
                        "element_key",
                        "element_text",
                        "element_initial",
                        "element_table",
                    ]);
                }
            }
        }
    }
    parts
}

/// One edit to one field, and what the structure says of it: nothing, for a
/// field that is no part of what a model means.
struct Row {
    of: &'static str,
    field: &'static str,
    says: Vec<&'static str>,
    edit: Box<dyn Fn(&mut Project)>,
}

fn row(
    of: &'static str,
    field: &'static str,
    says: &[&'static str],
    edit: impl Fn(&mut Project) + 'static,
) -> Row {
    Row {
        of,
        field,
        says: says.to_vec(),
        edit: Box::new(edit),
    }
}

fn variable<'p>(project: &'p mut Project, name: &str) -> &'p mut Variable {
    project.models[0]
        .get_variable_mut(name)
        .unwrap_or_else(|| panic!("the fixture has no '{name}'"))
}

fn as_stock<'p>(project: &'p mut Project, name: &str) -> &'p mut Stock {
    match variable(project, name) {
        Variable::Stock(stock) => stock,
        _ => unreachable!("'{name}' is a stock"),
    }
}

fn as_flow<'p>(project: &'p mut Project, name: &str) -> &'p mut Flow {
    match variable(project, name) {
        Variable::Flow(flow) => flow,
        _ => unreachable!("'{name}' is a flow"),
    }
}

fn as_aux<'p>(project: &'p mut Project, name: &str) -> &'p mut Aux {
    match variable(project, name) {
        Variable::Aux(aux) => aux,
        _ => unreachable!("'{name}' is an auxiliary"),
    }
}

fn as_module<'p>(project: &'p mut Project, name: &str) -> &'p mut Module {
    match variable(project, name) {
        Variable::Module(module) => module,
        _ => unreachable!("'{name}' is a module"),
    }
}

fn compat<'p>(project: &'p mut Project, name: &str) -> &'p mut Compat {
    match variable(project, name) {
        Variable::Stock(v) => &mut v.compat,
        Variable::Flow(v) => &mut v.compat,
        Variable::Aux(v) => &mut v.compat,
        Variable::Module(v) => &mut v.compat,
    }
}

fn conveyor(project: &mut Project) -> &mut Conveyor {
    compat(project, "belt")
        .conveyor
        .as_mut()
        .expect("belt is a conveyor")
}

fn leak(project: &mut Project) -> &mut Leakage {
    compat(project, "leaking")
        .leakage
        .as_mut()
        .expect("leaking is a leak")
}

fn data(project: &mut Project) -> &mut DataSource {
    compat(project, "ext")
        .data_source
        .as_mut()
        .expect("ext reads data")
}

fn effect_table(project: &mut Project) -> &mut GraphicalFunction {
    as_aux(project, "effect")
        .gf
        .as_mut()
        .expect("effect holds a table")
}

fn dimension<'p>(project: &'p mut Project, name: &str) -> &'p mut Dimension {
    project
        .dimensions
        .iter_mut()
        .find(|d| d.name == name)
        .unwrap_or_else(|| panic!("the fixture has no dimension '{name}'"))
}

/// The arrayed variable's dimensions, elements, default and whether it
/// applies.
#[allow(clippy::type_complexity)]
fn pop(
    project: &mut Project,
) -> (
    &mut Vec<String>,
    &mut Vec<(String, String, Option<String>, Option<GraphicalFunction>)>,
    &mut Option<String>,
    &mut bool,
) {
    match &mut as_aux(project, "pop").equation {
        Equation::Arrayed(dims, elements, default, applies) => (dims, elements, default, applies),
        _ => unreachable!("pop is arrayed"),
    }
}

const CONVEYOR: &[&str] = &["the conveyor 'belt' changes"];
const DATA: &[&str] = &["the data source of 'ext' changes"];
const TABLE: &[&str] = &["the graphical function of 'effect' changes"];
const ZONE: &[&str] = &["the zone the leak 'leaking' drains changes"];
const INPUTS: &[&str] = &["the inputs of module 'sub' change"];
/// A field that describes a model, a variable or a project and is not read to
/// compute it.
const NOT_MEANING: &[&str] = &[];

fn rows() -> Vec<Row> {
    let text = |s: &str| s.to_string();
    vec![
        // ---- Project ----
        row("Project", "name", NOT_MEANING, |p| p.name = "other".into()),
        row(
            "Project",
            "sim_specs",
            &["the save simulates from 0 to 20, not from 0 to 10"],
            |p| p.sim_specs.stop = 20.0,
        ),
        row(
            "Project",
            "dimensions",
            &["dimension 'Sub' is not in the save"],
            |p| p.dimensions.retain(|d| d.name != "Sub"),
        ),
        row("Project", "units", NOT_MEANING, |p| {
            p.units.push(Unit {
                name: "widget".into(),
                equation: None,
                disabled: false,
                aliases: Vec::new(),
            })
        }),
        row(
            "Project",
            "models",
            &["model 'double' is not in the save"],
            |p| p.models.retain(|m| m.name != "double"),
        ),
        row("Project", "source", NOT_MEANING, |p| {
            p.source = Some(Source {
                extension: Extension::Xmile,
                content: "<xmile/>".into(),
            })
        }),
        row("Project", "ai_information", NOT_MEANING, |p| {
            p.ai_information = Some(AiInformation {
                status: AiStatus {
                    key_url: "k".into(),
                    algorithm: "a".into(),
                    signature: "s".into(),
                    tags: HashMap::new(),
                },
                testing: None,
                log: None,
            })
        }),
        // ---- SimSpecs ----
        row(
            "SimSpecs",
            "start",
            &["the save simulates from 1 to 10, not from 0 to 10"],
            |p| p.sim_specs.start = 1.0,
        ),
        row(
            "SimSpecs",
            "stop",
            &["the save simulates from 0 to 20, not from 0 to 10"],
            |p| p.sim_specs.stop = 20.0,
        ),
        row(
            "SimSpecs",
            "dt",
            &[
                "the save's time step is 0.5, not 1",
                "the save's save step is 0.5, not 1",
            ],
            |p| p.sim_specs.dt = Dt::Reciprocal(2.0),
        ),
        row(
            "SimSpecs",
            "save_step",
            &["the save's save step is 2, not 1"],
            |p| p.sim_specs.save_step = Some(Dt::Dt(2.0)),
        ),
        row(
            "SimSpecs",
            "sim_method",
            &["the save integrates with fourth-order Runge-Kutta, not Euler's method"],
            |p| p.sim_specs.sim_method = SimMethod::RungeKutta4,
        ),
        row("SimSpecs", "time_units", NOT_MEANING, |p| {
            p.sim_specs.time_units = Some("year".into())
        }),
        // ---- Dimension ----
        row(
            "Dimension",
            "name",
            &[
                "dimension 'Sub' is not in the save",
                "dimension 'Sub2' is new in the save",
            ],
            |p| dimension(p, "Sub").name = "Sub2".into(),
        ),
        row(
            "Dimension",
            "elements",
            &["dimension 'Full' has elements 1, 2, 3, 4 in the save, not 1, 2, 3"],
            |p| dimension(p, "Full").elements = DimensionElements::Indexed(4),
        ),
        row(
            "Dimension",
            "elements",
            &["dimension 'Full' has named elements in the save, not numbered ones"],
            |p| {
                dimension(p, "Full").elements =
                    DimensionElements::Named(vec!["1".into(), "2".into(), "3".into()])
            },
        ),
        row(
            "Dimension",
            "mappings",
            &["dimension 'Region' no longer maps to 'area'"],
            |p| dimension(p, "Region").mappings.clear(),
        ),
        row(
            "Dimension",
            "parent",
            &["dimension 'Sub' is no longer a subdimension of 'full'"],
            |p| dimension(p, "Sub").parent = None,
        ),
        row(
            "Dimension",
            "parent",
            &["dimension 'Sub' is a subdimension of 'area' in the save, not of 'full'"],
            |p| dimension(p, "Sub").parent = Some("Area".into()),
        ),
        row(
            "Dimension",
            "parent",
            &["dimension 'Full' becomes a subdimension of 'area'"],
            |p| dimension(p, "Full").parent = Some("Area".into()),
        ),
        row(
            "DimensionMapping",
            "target",
            &[
                "dimension 'Region' no longer maps to 'area'",
                "dimension 'Region' maps to 'zone' in the save",
            ],
            |p| dimension(p, "Region").mappings[0].target = "Zone".into(),
        ),
        row(
            "DimensionMapping",
            "element_map",
            &["the mapping of dimension 'Region' to 'area' relates other elements in the save"],
            |p| dimension(p, "Region").mappings[0].element_map[0].1 = "a2".into(),
        ),
        // ---- Model ----
        row(
            "Model",
            "name",
            &[
                "model 'inner' is not in the save",
                "model 'inner2' is new in the save",
            ],
            |p| p.models[1].name = "inner2".into(),
        ),
        row(
            "Model",
            "sim_specs",
            &["the save simulates from 0 to 20, not from 0 to 5"],
            |p| p.models[1].sim_specs = Some(specs(20.0)),
        ),
        row("Model", "variables", &["'ext' is not in the save"], |p| {
            p.models[0]
                .variables
                .rewrite(|vars| vars.retain(|v| v.get_ident() != "ext"))
        }),
        row("Model", "views", NOT_MEANING, |p| {
            p.models[0].views.push(View::StockFlow(StockFlow {
                name: None,
                elements: Vec::new().into(),
                view_box: Rect::default(),
                zoom: 1.0,
                use_lettered_polarity: false,
                font: None,
                sketch_compat: None,
            }))
        }),
        row("Model", "loop_metadata", NOT_MEANING, |p| {
            p.models[0].loop_metadata.push(LoopMetadata {
                uids: vec![1, 2],
                deleted: false,
                name: "R1".into(),
                description: String::new(),
            })
        }),
        row("Model", "groups", NOT_MEANING, |p| {
            p.models[0].groups.push(ModelGroup::default())
        }),
        row(
            "Model",
            "macro_spec",
            &["model 'double' is no longer a macro"],
            |p| p.models[2].macro_spec = None,
        ),
        row(
            "MacroSpec",
            "parameters",
            &["macro 'double' takes factor, input in the save, not input, factor"],
            |p| {
                if let Some(spec) = p.models[2].macro_spec.as_mut() {
                    spec.parameters.reverse()
                }
            },
        ),
        row(
            "MacroSpec",
            "primary_output",
            &["macro 'double' returns 'half' in the save, not 'double'"],
            |p| {
                if let Some(spec) = p.models[2].macro_spec.as_mut() {
                    spec.primary_output = "half".into()
                }
            },
        ),
        row(
            "MacroSpec",
            "additional_outputs",
            &["macro 'double' also returns nothing in the save, not half"],
            |p| {
                if let Some(spec) = p.models[2].macro_spec.as_mut() {
                    spec.additional_outputs.clear()
                }
            },
        ),
        // ---- Stock ----
        row(
            "Stock",
            "ident",
            &["'line' is not in the save", "'line2' is new in the save"],
            |p| as_stock(p, "line").ident = "line2".into(),
        ),
        row(
            "Stock",
            "equation",
            &["'line' starts from '7' in the save, not from '0'"],
            |p| as_stock(p, "line").equation = scalar("7"),
        ),
        row("Stock", "documentation", NOT_MEANING, |p| {
            as_stock(p, "line").documentation = "people waiting".into()
        }),
        row("Stock", "units", NOT_MEANING, |p| {
            as_stock(p, "line").units = Some("people".into())
        }),
        row(
            "Stock",
            "inflows",
            &["'belt' loses inflow 'arriving'"],
            |p| as_stock(p, "belt").inflows.clear(),
        ),
        row(
            "Stock",
            "outflows",
            &[
                "'belt' takes its outflows in another order in the save: leaking, leaving, not leaving, leaking",
            ],
            |p| as_stock(p, "belt").outflows.reverse(),
        ),
        row("Stock", "ai_state", NOT_MEANING, |p| {
            as_stock(p, "line").ai_state = Some(AiState::C)
        }),
        row("Stock", "uid", NOT_MEANING, |p| {
            as_stock(p, "line").uid = Some(7)
        }),
        row(
            "Stock",
            "compat",
            &["'belt' is no longer non-negative"],
            |p| as_stock(p, "belt").compat.non_negative = false,
        ),
        // ---- Flow ----
        row(
            "Flow",
            "ident",
            &[
                "'served' is not in the save",
                "'served2' is new in the save",
            ],
            |p| as_flow(p, "served").ident = "served2".into(),
        ),
        row(
            "Flow",
            "equation",
            &["'arriving' is computed as '11' in the save, not as '10'"],
            |p| as_flow(p, "arriving").equation = scalar("11"),
        ),
        row("Flow", "documentation", NOT_MEANING, |p| {
            as_flow(p, "served").documentation = "people served".into()
        }),
        row("Flow", "units", NOT_MEANING, |p| {
            as_flow(p, "served").units = Some("people/month".into())
        }),
        row(
            "Flow",
            "gf",
            &["'arriving' gains a graphical function"],
            |p| as_flow(p, "arriving").gf = Some(table()),
        ),
        row("Flow", "ai_state", NOT_MEANING, |p| {
            as_flow(p, "served").ai_state = Some(AiState::C)
        }),
        row("Flow", "uid", NOT_MEANING, |p| {
            as_flow(p, "served").uid = Some(7)
        }),
        row(
            "Flow",
            "compat",
            &["'balking' is no longer a queue's overflow"],
            |p| as_flow(p, "balking").compat.overflow = false,
        ),
        // ---- Aux ----
        row(
            "Aux",
            "ident",
            &["'ext' is not in the save", "'ext2' is new in the save"],
            |p| as_aux(p, "ext").ident = "ext2".into(),
        ),
        row(
            "Aux",
            "equation",
            &["'effect' is computed as 'time() * 2' in the save, not as 'time()'"],
            |p| as_aux(p, "effect").equation = scalar("TIME * 2"),
        ),
        row("Aux", "documentation", NOT_MEANING, |p| {
            as_aux(p, "effect").documentation = "an effect".into()
        }),
        row("Aux", "units", NOT_MEANING, |p| {
            as_aux(p, "effect").units = Some("dmnl".into())
        }),
        row(
            "Aux",
            "gf",
            &["'effect' loses its graphical function"],
            |p| as_aux(p, "effect").gf = None,
        ),
        row("Aux", "ai_state", NOT_MEANING, |p| {
            as_aux(p, "effect").ai_state = Some(AiState::C)
        }),
        row("Aux", "uid", NOT_MEANING, |p| {
            as_aux(p, "effect").uid = Some(7)
        }),
        row(
            "Aux",
            "compat",
            &["'port' is no longer a module input"],
            |p| as_aux(p, "port").compat.can_be_module_input = false,
        ),
        // ---- Module ----
        row(
            "Module",
            "ident",
            &["'sub' is not in the save", "'sub2' is new in the save"],
            |p| as_module(p, "sub").ident = "sub2".into(),
        ),
        row(
            "Module",
            "model_name",
            &["'sub' instantiates 'double' in the save, not 'inner'"],
            |p| as_module(p, "sub").model_name = "double".into(),
        ),
        row("Module", "documentation", NOT_MEANING, |p| {
            as_module(p, "sub").documentation = "a part".into()
        }),
        row("Module", "units", NOT_MEANING, |p| {
            as_module(p, "sub").units = Some("dmnl".into())
        }),
        row("Module", "references", INPUTS, |p| {
            as_module(p, "sub").references.clear()
        }),
        row("Module", "ai_state", NOT_MEANING, |p| {
            as_module(p, "sub").ai_state = Some(AiState::C)
        }),
        row("Module", "uid", NOT_MEANING, |p| {
            as_module(p, "sub").uid = Some(7)
        }),
        row("Module", "compat", &["'sub' becomes a module input"], |p| {
            as_module(p, "sub").compat.can_be_module_input = true
        }),
        row("ModuleReference", "src", INPUTS, |p| {
            as_module(p, "sub").references[0].src = "started".into()
        }),
        row("ModuleReference", "dst", INPUTS, |p| {
            as_module(p, "sub").references[0].dst = "sub.other".into()
        }),
        // ---- Compat ----
        row(
            "Compat",
            "active_initial",
            &["'started' loses its initial value"],
            |p| compat(p, "started").active_initial = None,
        ),
        row(
            "Compat",
            "active_initial",
            &["the initial value of 'started' changes"],
            |p| compat(p, "started").active_initial = Some("6".into()),
        ),
        row(
            "Compat",
            "non_negative",
            &["'belt' is no longer non-negative"],
            |p| compat(p, "belt").non_negative = false,
        ),
        row(
            "Compat",
            "non_negative",
            &["'line' becomes non-negative"],
            |p| compat(p, "line").non_negative = true,
        ),
        // Only a stock and a flow carry the marking; the compiler reads it of
        // no other variable, and no format writes it for one.
        row("Compat", "non_negative", NOT_MEANING, |p| {
            compat(p, "effect").non_negative = true
        }),
        row(
            "Compat",
            "can_be_module_input",
            &["'port' is no longer a module input"],
            |p| compat(p, "port").can_be_module_input = false,
        ),
        row("Compat", "visibility", NOT_MEANING, |p| {
            compat(p, "port").visibility = Visibility::Public
        }),
        row(
            "Compat",
            "data_source",
            &["'ext' is no longer read from data"],
            |p| compat(p, "ext").data_source = None,
        ),
        row(
            "Compat",
            "conveyor",
            &["'belt' is no longer a conveyor"],
            |p| compat(p, "belt").conveyor = None,
        ),
        row(
            "Compat",
            "leakage",
            &["'leaking' is no longer a conveyor leak"],
            |p| compat(p, "leaking").leakage = None,
        ),
        row(
            "Compat",
            "spreadflow",
            &["'arriving' is no longer a conveyor inflow with its own placement"],
            |p| compat(p, "arriving").spreadflow = None,
        ),
        row(
            "Compat",
            "spreadflow",
            &["how the inflow 'arriving' spreads over its conveyor changes"],
            |p| compat(p, "arriving").spreadflow = Some(SpreadFlow::Dest),
        ),
        row("Compat", "queue", &["'line' is no longer a queue"], |p| {
            compat(p, "line").queue = None
        }),
        row(
            "Compat",
            "overflow",
            &["'served' becomes a queue's overflow"],
            |p| compat(p, "served").overflow = true,
        ),
        // ---- Conveyor ----
        row("Conveyor", "transit_time", CONVEYOR, move |p| {
            conveyor(p).transit_time = text("3")
        }),
        row("Conveyor", "capacity", CONVEYOR, |p| {
            conveyor(p).capacity = None
        }),
        row("Conveyor", "inflow_limit", CONVEYOR, move |p| {
            conveyor(p).inflow_limit = Some(text("60"))
        }),
        row("Conveyor", "sample", CONVEYOR, |p| {
            conveyor(p).sample = None
        }),
        row("Conveyor", "arrest", CONVEYOR, move |p| {
            conveyor(p).arrest = Some(text("1"))
        }),
        row("Conveyor", "discrete", CONVEYOR, |p| {
            conveyor(p).discrete = true
        }),
        row("Conveyor", "batch_integrity", CONVEYOR, |p| {
            conveyor(p).batch_integrity = true
        }),
        row("Conveyor", "one_at_a_time", CONVEYOR, |p| {
            conveyor(p).one_at_a_time = false
        }),
        row("Conveyor", "exponential_leak", CONVEYOR, |p| {
            conveyor(p).exponential_leak = true
        }),
        row("Conveyor", "ignore_earlier_zone_losses", CONVEYOR, |p| {
            conveyor(p).ignore_earlier_zone_losses = true
        }),
        // ---- Leakage ----
        row(
            "Leakage",
            "fraction",
            &["the fraction the leak 'leaking' drains changes"],
            move |p| leak(p).fraction = Some(text("0.2")),
        ),
        row(
            "Leakage",
            "integers",
            &["the leak 'leaking' drains whole units in the save, not any amount"],
            |p| leak(p).integers = true,
        ),
        row("Leakage", "zone_start", ZONE, move |p| {
            leak(p).zone_start = Some(text("0.5"))
        }),
        row("Leakage", "zone_end", ZONE, |p| leak(p).zone_end = None),
        // ---- DataSource ----
        row("DataSource", "kind", DATA, |p| {
            data(p).kind = DataSourceKind::Constants
        }),
        row("DataSource", "file", DATA, move |p| {
            data(p).file = text("other.csv")
        }),
        row("DataSource", "tab_or_delimiter", DATA, move |p| {
            data(p).tab_or_delimiter = text(";")
        }),
        row("DataSource", "row_or_col", DATA, move |p| {
            data(p).row_or_col = text("B")
        }),
        row("DataSource", "cell", DATA, move |p| {
            data(p).cell = text("C3")
        }),
        // ---- GraphicalFunction ----
        row("GraphicalFunction", "kind", TABLE, |p| {
            effect_table(p).kind = GraphicalFunctionKind::Extrapolate
        }),
        row("GraphicalFunction", "x_points", TABLE, |p| {
            effect_table(p).x_points = Some(vec![0.0, 5.0])
        }),
        row("GraphicalFunction", "y_points", TABLE, |p| {
            effect_table(p).y_points = vec![5.0, 16.0]
        }),
        row("GraphicalFunction", "x_scale", TABLE, |p| {
            effect_table(p).x_scale.max = 11.0
        }),
        row("GraphicalFunction", "y_scale", TABLE, |p| {
            effect_table(p).y_scale.max = 21.0
        }),
        row("GraphicalFunctionScale", "min", TABLE, |p| {
            effect_table(p).x_scale.min = -1.0
        }),
        row("GraphicalFunctionScale", "max", TABLE, |p| {
            effect_table(p).y_scale.max = 21.0
        }),
        // ---- Equation ----
        row(
            "Equation",
            "scalar_text",
            &["'started' is computed as 'port * 3' in the save, not as 'port * 2'"],
            |p| as_aux(p, "started").equation = scalar("port * 3"),
        ),
        row(
            "Equation",
            "apply_to_all_dims",
            &[
                "'every' is defined over area, not region",
                "'every' loses elements 'north' and 'south'",
                "'every' gains elements 'a1' and 'a2'",
            ],
            |p| {
                as_aux(p, "every").equation =
                    Equation::ApplyToAll(vec!["Area".into()], "pop[Region] * 2".into())
            },
        ),
        row(
            "Equation",
            "apply_to_all_text",
            &[
                "'every' is computed as 'pop[region] * 3' for element 'north' in the save, not as 'pop[region] * 2' (and 1 more element)",
            ],
            |p| {
                as_aux(p, "every").equation =
                    Equation::ApplyToAll(vec!["Region".into()], "pop[Region] * 3".into())
            },
        ),
        row(
            "Equation",
            "arrayed_dims",
            &[
                "'pop' is defined over area, not region",
                "'pop' gains elements 'a1' and 'a2'",
            ],
            |p| *pop(p).0 = vec!["Area".into()],
        ),
        row(
            "Equation",
            "arrayed_default",
            &["the :EXCEPT: default of 'pop' is '8' in the save, not '9'"],
            |p| *pop(p).2 = Some("8".into()),
        ),
        row(
            "Equation",
            "arrayed_applies",
            &["'pop' no longer has its :EXCEPT: default '9'"],
            |p| *pop(p).3 = false,
        ),
        row(
            "Equation",
            "element_key",
            &[
                "'pop' gains element 'west'",
                "'pop' is computed as '9' for element 'south' in the save, not as '2'",
            ],
            |p| pop(p).1[1].0 = "west".into(),
        ),
        row(
            "Equation",
            "element_text",
            &["'pop' is computed as '3' for element 'south' in the save, not as '2'"],
            |p| pop(p).1[1].1 = "3".into(),
        ),
        row(
            "Equation",
            "element_initial",
            &["'pop' loses its initial value for element 'north'"],
            |p| pop(p).1[0].2 = None,
        ),
        row(
            "Equation",
            "element_table",
            &["'pop' loses its graphical function for element 'north'"],
            |p| pop(p).1[0].3 = None,
        ),
    ]
}

#[test]
fn every_field_is_compared_or_named() {
    let base = fixture();
    assert_eq!(structure(&base, &base), Vec::<String>::new());

    let rows = rows();
    let lists = field_lists(&base);
    for (of, fields) in &lists {
        for field in fields {
            assert!(
                rows.iter().any(|row| row.of == *of && row.field == *field),
                "{of}::{field} has no row"
            );
        }
    }
    for row in &rows {
        let listed = lists
            .iter()
            .any(|(of, fields)| *of == row.of && fields.contains(&row.field));
        assert!(listed, "{}::{} is no field of the type", row.of, row.field);
    }

    for row in &rows {
        let mut edited = base.clone();
        (row.edit)(&mut edited);
        assert!(
            edited != base,
            "{}::{}: the edit changes nothing",
            row.of,
            row.field
        );
        assert_eq!(
            structure(&base, &edited),
            row.says,
            "{}::{}",
            row.of,
            row.field
        );
    }
}
