// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The simulation key against what it is taken from: a row per field of each
//! of the sync's extractions, and of each type their fields hold, edited
//! alone, saying whether the key moves.
//!
//! The field lists are not kept by hand: [`fields!`] takes a type's fields
//! once, as a pattern that does not compile unless it names them all, and
//! gives their names; a field with no row fails its test.

use super::*;
use crate::datamodel::{
    Aux, Compat, Dimension, DimensionElements, DimensionMapping, Dt, Equation, Flow,
    GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale, LoopMetadata, MacroSpec,
    Model, Module, ModuleReference, Project, SimMethod, SimSpecs, Stock, Unit, Variable,
};

/// Whether an edit of a field moves the key.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Key {
    /// A simulation reads the field.
    Moves,
    /// No simulation reads it.
    Stays,
}

/// One edit of one field of one extraction.
struct Row {
    /// The extraction's field the edit changes.
    field: &'static str,
    what: &'static str,
    key: Key,
    edit: fn(&mut Project),
}

fn row(field: &'static str, what: &'static str, key: Key, edit: fn(&mut Project)) -> Row {
    Row {
        field,
        what,
        key,
        edit,
    }
}

/// The names of a struct's fields, from a pattern that names every one: a
/// field added to the type does not compile here until it is named, and then
/// fails the test that asks for a row per name.
macro_rules! fields {
    ($value:expr, $ty:path { $($field:ident),* $(,)? }) => {{
        let $ty { $($field: _),* } = $value;
        vec![$(stringify!($field)),*]
    }};
}

/// The names of an enum's variants, from a match that is exhaustive: a
/// variant added to the type does not compile here until it has a name.
macro_rules! variants {
    ($value:expr, { $($pattern:pat => $name:literal),* $(,)? }) => {{
        let _: &str = match $value {
            $($pattern => $name),*
        };
        vec![$($name),*]
    }};
}

/// The names of the extractions' fields, each under its extraction.
fn fields(project: &Project) -> Vec<String> {
    let under = |extraction: &str, names: Vec<&'static str>| -> Vec<String> {
        names
            .into_iter()
            .map(|name| format!("{extraction}.{name}"))
            .collect()
    };
    let mut all = under(
        "project",
        fields!(
            SourceProjectFields::from_datamodel(project),
            SourceProjectFields {
                name,
                sim_specs,
                dimensions,
                units,
                macro_declarations,
            }
        ),
    );
    all.extend(under(
        "model",
        fields!(
            SourceModelFields::from_datamodel(&project.models[0]),
            SourceModelFields {
                name,
                declared_variable_idents,
                sim_specs,
                macro_spec,
                pinned_loops,
            }
        ),
    ));
    all.extend(under(
        "variable",
        fields!(
            SourceVariableFields::from_datamodel(&project.models[0].variables[0], "main"),
            SourceVariableFields {
                ident,
                equation,
                kind,
                units,
                gf,
                inflows,
                outflows,
                repeated_inflows,
                repeated_outflows,
                module_refs,
                referenced_model_name,
                owner_model,
                non_negative,
                can_be_module_input,
                compat,
            }
        ),
    ));
    all
}

fn table() -> GraphicalFunction {
    GraphicalFunction {
        kind: GraphicalFunctionKind::Continuous,
        x_points: Some(vec![0.0, 1.0, 2.0]),
        y_points: vec![0.0, 0.5, 1.0],
        x_scale: GraphicalFunctionScale { min: 0.0, max: 2.0 },
        y_scale: GraphicalFunctionScale { min: 0.0, max: 1.0 },
    }
}

fn aux(ident: &str, equation: &str) -> Aux {
    Aux {
        ident: ident.to_string(),
        equation: Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: Compat::default(),
    }
}

/// A project with one of everything the extractions read: a stock with
/// flows, a flow, a table-backed auxiliary, an arrayed auxiliary, a module of
/// a second model, dimensions, a unit, and a pinned loop.
fn base() -> Project {
    let main = Model {
        name: "main".to_string(),
        sim_specs: None,
        variables: vec![
            Variable::Stock(Stock {
                ident: "level".to_string(),
                equation: Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["filling".to_string()],
                outflows: vec!["draining".to_string()],
                ai_state: None,
                uid: Some(1),
                compat: Compat::default(),
            }),
            Variable::Flow(Flow {
                ident: "filling".to_string(),
                equation: Equation::Scalar("rate".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(2),
                compat: Compat::default(),
            }),
            Variable::Flow(Flow {
                ident: "draining".to_string(),
                equation: Equation::Scalar("level / 5".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: Some(3),
                compat: Compat::default(),
            }),
            Variable::Aux(aux("rate", "3")),
            Variable::Aux(Aux {
                gf: Some(table()),
                ..aux("effect", "level / 10")
            }),
            Variable::Aux(Aux {
                equation: Equation::Arrayed(
                    vec!["region".to_string()],
                    vec![
                        ("north".to_string(), "1".to_string(), None, None),
                        ("south".to_string(), "2".to_string(), None, Some(table())),
                    ],
                    None,
                    false,
                ),
                ..aux("by_region", "")
            }),
            Variable::Module(Module {
                ident: "part".to_string(),
                model_name: "sub".to_string(),
                documentation: String::new(),
                units: None,
                references: vec![ModuleReference {
                    src: "rate".to_string(),
                    dst: "part.input".to_string(),
                }],
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            }),
        ]
        .into(),
        views: vec![],
        loop_metadata: vec![LoopMetadata {
            uids: vec![1, 3],
            deleted: false,
            name: "drain".to_string(),
            description: String::new(),
        }],
        groups: vec![],
        macro_spec: None,
    };
    let sub = Model {
        name: "sub".to_string(),
        sim_specs: None,
        variables: vec![Variable::Aux(Aux {
            compat: Compat {
                can_be_module_input: true,
                ..Compat::default()
            },
            ..aux("input", "0")
        })]
        .into(),
        views: vec![],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    };
    Project {
        name: "key".to_string(),
        sim_specs: SimSpecs {
            start: 0.0,
            stop: 10.0,
            dt: Dt::Dt(0.25),
            save_step: None,
            sim_method: SimMethod::Euler,
            time_units: Some("month".to_string()),
        },
        dimensions: vec![
            Dimension {
                name: "region".to_string(),
                elements: DimensionElements::Named(vec!["north".to_string(), "south".to_string()]),
                mappings: vec![],
                parent: None,
            },
            Dimension {
                name: "area".to_string(),
                elements: DimensionElements::Named(vec!["a1".to_string(), "a2".to_string()]),
                mappings: vec![],
                parent: None,
            },
            Dimension {
                name: "index".to_string(),
                elements: DimensionElements::Indexed(3),
                mappings: vec![],
                parent: None,
            },
        ],
        units: vec![Unit {
            name: "widget".to_string(),
            equation: None,
            disabled: false,
            aliases: vec![],
        }],
        models: vec![main, sub],
        source: None,
        ai_information: None,
    }
}

fn var<'a>(project: &'a mut Project, name: &str) -> &'a mut Variable {
    project.models[0]
        .get_variable_mut(name)
        .expect("the base project has the variable")
}

fn compat_of(variable: &mut Variable) -> &mut Compat {
    match variable {
        Variable::Stock(v) => &mut v.compat,
        Variable::Flow(v) => &mut v.compat,
        Variable::Aux(v) => &mut v.compat,
        Variable::Module(v) => &mut v.compat,
    }
}

fn rows() -> Vec<Row> {
    use Key::{Moves, Stays};
    vec![
        // ── the project's fields
        row("project.name", "the project's name", Stays, |p| {
            p.name = "renamed".to_string()
        }),
        row("project.sim_specs", "the stop time", Moves, |p| {
            p.sim_specs.stop = 30.0
        }),
        row("project.sim_specs", "the start time", Moves, |p| {
            p.sim_specs.start = 1.0
        }),
        row("project.sim_specs", "DT", Moves, |p| {
            p.sim_specs.dt = Dt::Dt(0.5)
        }),
        row("project.sim_specs", "the save step", Moves, |p| {
            p.sim_specs.save_step = Some(Dt::Dt(1.0))
        }),
        row("project.sim_specs", "the integration method", Moves, |p| {
            p.sim_specs.sim_method = SimMethod::RungeKutta4
        }),
        row(
            "project.sim_specs",
            "DT written as the reciprocal of the same step",
            Stays,
            |p| p.sim_specs.dt = Dt::Reciprocal(4.0),
        ),
        row("project.sim_specs", "the time units", Stays, |p| {
            p.sim_specs.time_units = Some("year".to_string())
        }),
        row("project.dimensions", "an element added", Moves, |p| {
            p.dimensions[0].elements = DimensionElements::Named(vec![
                "north".to_string(),
                "south".to_string(),
                "east".to_string(),
            ])
        }),
        row("project.dimensions", "the elements reordered", Moves, |p| {
            p.dimensions[0].elements =
                DimensionElements::Named(vec!["south".to_string(), "north".to_string()])
        }),
        row("project.dimensions", "a positional mapping", Moves, |p| {
            p.dimensions[0].mappings = vec![DimensionMapping {
                target: "area".to_string(),
                element_map: vec![],
            }]
        }),
        row("project.dimensions", "an element mapping", Moves, |p| {
            p.dimensions[0].mappings = vec![DimensionMapping {
                target: "area".to_string(),
                element_map: vec![("north".to_string(), "a2".to_string())],
            }]
        }),
        row("project.dimensions", "a parent", Moves, |p| {
            p.dimensions[2].parent = Some("region".to_string())
        }),
        row("project.dimensions", "an indexed size", Moves, |p| {
            p.dimensions[2].elements = DimensionElements::Indexed(4)
        }),
        row("project.dimensions", "a dimension's name", Moves, |p| {
            p.dimensions[1].name = "zone".to_string()
        }),
        row("project.units", "a unit definition", Stays, |p| {
            p.units[0].aliases.push("widgets".to_string())
        }),
        row(
            "project.macro_declarations",
            "a model made a macro",
            Moves,
            |p| {
                p.models[1].macro_spec = Some(MacroSpec {
                    parameters: vec!["input".to_string()],
                    primary_output: "input".to_string(),
                    additional_outputs: vec![],
                })
            },
        ),
        // ── a model's fields
        row("model.name", "a model's name", Moves, |p| {
            p.models[1].name = "other".to_string()
        }),
        row(
            "model.declared_variable_idents",
            "the order the variables are declared in",
            Stays,
            |p| {
                let mut variables: Vec<Variable> = p.models[0].variables.iter().cloned().collect();
                variables.reverse();
                p.models[0].variables = variables.into();
            },
        ),
        row("model.sim_specs", "a model's own specs", Moves, |p| {
            let mut specs = p.sim_specs.clone();
            specs.stop = 40.0;
            p.models[0].sim_specs = Some(specs);
        }),
        row(
            "model.sim_specs",
            "a model's own specs, equal to the project's",
            Moves,
            |p| p.models[0].sim_specs = Some(p.sim_specs.clone()),
        ),
        row("model.macro_spec", "a macro's parameters", Moves, |p| {
            p.models[0].macro_spec = Some(MacroSpec {
                parameters: vec!["rate".to_string()],
                primary_output: "level".to_string(),
                additional_outputs: vec![],
            })
        }),
        row("model.pinned_loops", "a pinned loop's name", Moves, |p| {
            p.models[0].loop_metadata[0].name = "leak".to_string()
        }),
        row("model.pinned_loops", "a loop pinned", Moves, |p| {
            p.models[0].loop_metadata.push(LoopMetadata {
                uids: vec![1, 2],
                deleted: false,
                name: "fill".to_string(),
                description: String::new(),
            })
        }),
        // ── a variable's fields
        row("variable.ident", "a variable's name", Moves, |p| {
            if let Variable::Aux(v) = var(p, "effect") {
                v.ident = "pressure".to_string();
            }
        }),
        // The same simulation under another spelling: the key is of the text.
        row(
            "variable.ident",
            "a variable's name, in case only",
            Moves,
            |p| {
                if let Variable::Aux(v) = var(p, "effect") {
                    v.ident = "Effect".to_string();
                }
            },
        ),
        row("variable.equation", "a scalar equation", Moves, |p| {
            var(p, "rate").set_scalar_equation("4")
        }),
        row(
            "variable.equation",
            "a scalar made apply-to-all",
            Moves,
            |p| {
                if let Variable::Aux(v) = var(p, "rate") {
                    v.equation = Equation::ApplyToAll(vec!["region".to_string()], "3".to_string());
                }
            },
        ),
        row("variable.equation", "an element's equation", Moves, |p| {
            if let Variable::Aux(v) = var(p, "by_region")
                && let Equation::Arrayed(_, elements, _, _) = &mut v.equation
            {
                elements[0].1 = "7".to_string();
            }
        }),
        row(
            "variable.equation",
            "an element's initial equation",
            Moves,
            |p| {
                if let Variable::Aux(v) = var(p, "by_region")
                    && let Equation::Arrayed(_, elements, _, _) = &mut v.equation
                {
                    elements[0].2 = Some("0".to_string());
                }
            },
        ),
        row("variable.equation", "an element's table", Moves, |p| {
            if let Variable::Aux(v) = var(p, "by_region")
                && let Equation::Arrayed(_, elements, _, _) = &mut v.equation
                && let Some(table) = &mut elements[1].3
            {
                table.y_points[1] = 0.75;
            }
        }),
        row("variable.equation", "an EXCEPT default", Moves, |p| {
            if let Variable::Aux(v) = var(p, "by_region")
                && let Equation::Arrayed(_, _, default, _) = &mut v.equation
            {
                *default = Some("9".to_string());
            }
        }),
        row(
            "variable.equation",
            "whether the default applies",
            Moves,
            |p| {
                if let Variable::Aux(v) = var(p, "by_region")
                    && let Equation::Arrayed(_, _, _, applies) = &mut v.equation
                {
                    *applies = true;
                }
            },
        ),
        row("variable.kind", "an auxiliary made a flow", Moves, |p| {
            let flow = Variable::Flow(Flow {
                ident: "rate".to_string(),
                equation: Equation::Scalar("3".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                ai_state: None,
                uid: None,
                compat: Compat::default(),
            });
            *var(p, "rate") = flow;
        }),
        row("variable.units", "a variable's units", Stays, |p| {
            var(p, "rate").set_units("widget/month")
        }),
        row("variable.gf", "a table's point", Moves, |p| {
            if let Variable::Aux(v) = var(p, "effect")
                && let Some(table) = &mut v.gf
            {
                table.y_points[1] = 0.75;
            }
        }),
        row("variable.gf", "a table's x point", Moves, |p| {
            if let Variable::Aux(v) = var(p, "effect")
                && let Some(table) = &mut v.gf
            {
                table.x_points = Some(vec![0.0, 1.5, 2.0]);
            }
        }),
        row(
            "variable.gf",
            "a table's x points left implicit",
            Moves,
            |p| {
                if let Variable::Aux(v) = var(p, "effect")
                    && let Some(table) = &mut v.gf
                {
                    table.x_points = None;
                }
            },
        ),
        row("variable.gf", "a table's kind", Moves, |p| {
            if let Variable::Aux(v) = var(p, "effect")
                && let Some(table) = &mut v.gf
            {
                table.kind = GraphicalFunctionKind::Discrete;
            }
        }),
        row(
            "variable.gf",
            "a table's x scale beside written x points",
            Stays,
            |p| {
                if let Variable::Aux(v) = var(p, "effect")
                    && let Some(table) = &mut v.gf
                {
                    table.x_scale.max = 4.0;
                }
            },
        ),
        row("variable.gf", "a table removed", Moves, |p| {
            if let Variable::Aux(v) = var(p, "effect") {
                v.gf = None;
            }
        }),
        row("variable.inflows", "a stock's inflows", Moves, |p| {
            if let Variable::Stock(v) = var(p, "level") {
                v.inflows.clear();
            }
        }),
        row("variable.outflows", "a stock's outflows", Moves, |p| {
            if let Variable::Stock(v) = var(p, "level") {
                v.outflows.clear();
            }
        }),
        row(
            "variable.repeated_inflows",
            "an inflow listed twice",
            Stays,
            |p| {
                if let Variable::Stock(v) = var(p, "level") {
                    v.inflows.push("filling".to_string());
                }
            },
        ),
        row(
            "variable.repeated_outflows",
            "an outflow listed twice",
            Stays,
            |p| {
                if let Variable::Stock(v) = var(p, "level") {
                    v.outflows.push("draining".to_string());
                }
            },
        ),
        row("variable.module_refs", "a module's wiring", Moves, |p| {
            if let Variable::Module(v) = var(p, "part") {
                v.references[0].src = "level".to_string();
            }
        }),
        row(
            "variable.referenced_model_name",
            "the model a module instantiates",
            Moves,
            |p| {
                if let Variable::Module(v) = var(p, "part") {
                    v.model_name = "main".to_string();
                }
            },
        ),
        row(
            "variable.owner_model",
            "the owning model's name",
            Moves,
            |p| p.models[0].name = "primary".to_string(),
        ),
        row("variable.non_negative", "a stock's flag", Moves, |p| {
            compat_of(var(p, "level")).non_negative = true
        }),
        row(
            "variable.can_be_module_input",
            "a variable made a port",
            Moves,
            |p| compat_of(var(p, "rate")).can_be_module_input = true,
        ),
        row("variable.compat", "an active initial", Moves, |p| {
            compat_of(var(p, "rate")).active_initial = Some("1".to_string())
        }),
        row("variable.compat", "a conveyor", Moves, |p| {
            compat_of(var(p, "level")).conveyor = Some(datamodel::Conveyor {
                transit_time: "3".to_string(),
                capacity: None,
                inflow_limit: None,
                sample: None,
                arrest: None,
                discrete: false,
                batch_integrity: false,
                one_at_a_time: true,
                exponential_leak: false,
                ignore_earlier_zone_losses: false,
            })
        }),
        row("variable.compat", "a queue", Moves, |p| {
            compat_of(var(p, "level")).queue = Some(datamodel::Queue {})
        }),
        row("variable.compat", "a leak", Moves, |p| {
            compat_of(var(p, "draining")).leakage = Some(datamodel::Leakage {
                fraction: Some("0.1".to_string()),
                integers: false,
                zone_start: None,
                zone_end: None,
            })
        }),
        row("variable.compat", "an overflow", Moves, |p| {
            compat_of(var(p, "draining")).overflow = true
        }),
        row("variable.compat", "a spread", Moves, |p| {
            compat_of(var(p, "filling")).spreadflow = Some(datamodel::SpreadFlow::Even)
        }),
        row("variable.compat", "a data source", Moves, |p| {
            compat_of(var(p, "rate")).data_source = Some(datamodel::DataSource {
                kind: datamodel::DataSourceKind::Constants,
                file: "data.csv".to_string(),
                tab_or_delimiter: ",".to_string(),
                row_or_col: "A".to_string(),
                cell: "B2".to_string(),
            })
        }),
        row("variable.compat", "visibility", Moves, |p| {
            compat_of(var(p, "rate")).visibility = datamodel::Visibility::Public
        }),
    ]
}

/// Every field of the three extractions has a row, and each row's edit moves
/// the key exactly when a simulation reads the field.
#[test]
fn the_key_moves_with_what_a_simulation_reads_and_nothing_else() {
    let project = base();
    let key = simulation_key(&project);
    assert_eq!(
        key,
        simulation_key(&project.clone()),
        "a key is its project's"
    );
    let rows = rows();
    for field in fields(&project) {
        assert!(
            rows.iter().any(|row| row.field == field),
            "{field} has no row"
        );
    }
    for row in &rows {
        assert!(
            fields(&project).iter().any(|field| field == row.field),
            "{} names no field",
            row.field
        );
    }
    for row in &rows {
        let mut edited = project.clone();
        (row.edit)(&mut edited);
        assert!(edited != project, "{}: the edit changes nothing", row.what);
        let moved = if simulation_key(&edited) == key {
            Key::Stays
        } else {
            Key::Moves
        };
        assert_eq!(moved, row.key, "{} ({})", row.what, row.field);
    }
}

/// What the datamodel holds that is no salsa input is in no key: diagrams,
/// sectors, notes, provenance and the source file.
#[test]
fn the_key_ignores_what_the_compiler_never_reads() {
    let project = base();
    let key = simulation_key(&project);
    let mut edited = project.clone();
    edited.models[0].groups.push(datamodel::ModelGroup {
        name: "sector".to_string(),
        doc: None,
        parent: None,
        members: vec!["rate".to_string()],
        run_enabled: false,
    });
    edited.models[0]
        .views
        .push(datamodel::View::StockFlow(datamodel::StockFlow {
            name: None,
            elements: vec![].into(),
            view_box: datamodel::Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            zoom: 2.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        }));
    var(&mut edited, "rate").set_documentation("units a month");
    if let Variable::Aux(v) = var(&mut edited, "rate") {
        v.ai_state = Some(datamodel::AiState::C);
    }
    edited.source = Some(datamodel::Source {
        extension: datamodel::Extension::Xmile,
        content: "<xmile/>".to_string(),
    });
    assert!(edited != project);
    assert_eq!(simulation_key(&edited), key);
}

// ── The types the fields hold ─────────────────────────────────────────
//
// Each type the key takes apart, alone: its fields from a pattern that names
// them all, a row per field (an edit of that field and whether the key
// moves), and for an enum a value per variant, no two of which hash alike.
// The values are written out because the hasher's method for a type is a
// function of a value of that type and nothing else; that production hands
// those methods the extractions' own values is what the rows above hold.

fn hashed<T: ?Sized>(value: &T, hash: impl Fn(&mut KeyHasher, &T)) -> u64 {
    let mut key = KeyHasher::default();
    hash(&mut key, value);
    key.finish()
}

/// One edit of one field of a value of `T`.
struct Part<T> {
    field: &'static str,
    key: Key,
    edit: fn(&mut T),
}

fn part<T>(field: &'static str, key: Key, edit: fn(&mut T)) -> Part<T> {
    Part { field, key, edit }
}

/// Every field of `T` has a row, every row names a field, and each row's
/// edit moves the hash exactly when it says a simulation reads the field.
fn check_fields<T: Clone + PartialEq>(
    what: &str,
    base: &T,
    names: Vec<&'static str>,
    parts: Vec<Part<T>>,
    hash: fn(&mut KeyHasher, &T),
) {
    for name in &names {
        assert!(
            parts.iter().any(|part| part.field == *name),
            "{what}.{name} has no row"
        );
    }
    let key = hashed(base, hash);
    for part in &parts {
        assert!(
            names.contains(&part.field),
            "{what}.{} is no field",
            part.field
        );
        let mut edited = base.clone();
        (part.edit)(&mut edited);
        assert!(
            edited != *base,
            "{what}.{}: the edit changes nothing",
            part.field
        );
        let moved = if hashed(&edited, hash) == key {
            Key::Stays
        } else {
            Key::Moves
        };
        assert_eq!(moved, part.key, "{what}.{}", part.field);
    }
}

/// Every variant of an enum has a value, and no two values hash alike.
fn check_variants<T>(
    what: &str,
    names: Vec<&'static str>,
    values: Vec<(&'static str, T)>,
    hash: fn(&mut KeyHasher, &T),
) {
    for name in &names {
        assert!(
            values.iter().any(|(variant, _)| variant == name),
            "{what}::{name} has no value"
        );
    }
    let mut seen: Vec<(u64, &str)> = Vec::new();
    for (variant, value) in &values {
        assert!(names.contains(variant), "{what}::{variant} is no variant");
        let key = hashed(value, hash);
        if let Some((_, other)) = seen.iter().find(|(k, _)| *k == key) {
            panic!("{what}::{variant} hashes as {what}::{other} does");
        }
        seen.push((key, variant));
    }
}

fn specs() -> SimSpecs {
    SimSpecs {
        start: 0.0,
        stop: 10.0,
        dt: Dt::Dt(0.25),
        save_step: Some(Dt::Dt(1.0)),
        sim_method: SimMethod::Euler,
        time_units: Some("month".to_string()),
    }
}

#[test]
fn every_field_of_the_sim_specs_is_in_the_key_or_left_out_for_a_reason() {
    use Key::{Moves, Stays};
    let base = specs();
    check_fields(
        "SimSpecs",
        &base,
        fields!(
            &base,
            SimSpecs {
                start,
                stop,
                dt,
                save_step,
                sim_method,
                time_units,
            }
        ),
        vec![
            part("start", Moves, |s| s.start = 1.0),
            part("stop", Moves, |s| s.stop = 11.0),
            part("dt", Moves, |s| s.dt = Dt::Dt(0.5)),
            // The same step written as its reciprocal is the same run.
            part("dt", Stays, |s| s.dt = Dt::Reciprocal(4.0)),
            part("save_step", Moves, |s| s.save_step = Some(Dt::Dt(2.0))),
            part("save_step", Moves, |s| s.save_step = None),
            part("sim_method", Moves, |s| {
                s.sim_method = SimMethod::RungeKutta2
            }),
            // Read by the unit check alone.
            part("time_units", Stays, |s| {
                s.time_units = Some("year".to_string())
            }),
        ],
        KeyHasher::sim_specs,
    );
    let method = |sim_method| SimSpecs {
        sim_method,
        ..specs()
    };
    check_variants(
        "SimMethod",
        variants!(base.sim_method, {
            SimMethod::Euler => "euler",
            SimMethod::RungeKutta2 => "rk2",
            SimMethod::RungeKutta4 => "rk4",
        }),
        vec![
            ("euler", method(SimMethod::Euler)),
            ("rk2", method(SimMethod::RungeKutta2)),
            ("rk4", method(SimMethod::RungeKutta4)),
        ],
        KeyHasher::sim_specs,
    );
}

#[test]
fn every_field_of_a_dimension_is_in_the_key() {
    use Key::Moves;
    let base = Dimension {
        name: "region".to_string(),
        elements: DimensionElements::Named(vec!["north".to_string(), "south".to_string()]),
        mappings: vec![DimensionMapping {
            target: "area".to_string(),
            element_map: vec![("north".to_string(), "a1".to_string())],
        }],
        parent: Some("place".to_string()),
    };
    check_fields(
        "Dimension",
        &base,
        fields!(
            &base,
            Dimension {
                name,
                elements,
                mappings,
                parent,
            }
        ),
        vec![
            part("name", Moves, |d| d.name = "zone".to_string()),
            part("elements", Moves, |d| {
                d.elements =
                    DimensionElements::Named(vec!["south".to_string(), "north".to_string()])
            }),
            part("mappings", Moves, |d| d.mappings.clear()),
            part("parent", Moves, |d| d.parent = None),
            part("parent", Moves, |d| d.parent = Some("land".to_string())),
        ],
        KeyHasher::dimension,
    );
    let with = |elements| Dimension {
        elements,
        ..base.clone()
    };
    check_variants(
        "DimensionElements",
        variants!(&base.elements, {
            DimensionElements::Indexed(_) => "indexed",
            DimensionElements::Named(_) => "named",
        }),
        vec![
            ("indexed", with(DimensionElements::Indexed(2))),
            ("indexed", with(DimensionElements::Indexed(3))),
            ("named", with(DimensionElements::Named(vec![]))),
            (
                "named",
                with(DimensionElements::Named(vec![
                    "a".to_string(),
                    "b".to_string(),
                ])),
            ),
        ],
        KeyHasher::dimension,
    );

    let mapping = base.mappings[0].clone();
    check_fields(
        "DimensionMapping",
        &mapping,
        fields!(
            &mapping,
            DimensionMapping {
                target,
                element_map,
            }
        ),
        vec![
            part("target", Moves, |m| m.target = "zone".to_string()),
            part("element_map", Moves, |m| {
                m.element_map[0].1 = "a2".to_string()
            }),
            part("element_map", Moves, |m| {
                m.element_map[0].0 = "south".to_string()
            }),
            part("element_map", Moves, |m| m.element_map.clear()),
        ],
        KeyHasher::dimension_mapping,
    );
}

#[test]
fn every_field_of_a_macro_spec_and_a_module_reference_is_in_the_key() {
    use Key::Moves;
    let spec = MacroSpec {
        parameters: vec!["input".to_string()],
        primary_output: "out".to_string(),
        additional_outputs: vec!["other".to_string()],
    };
    check_fields(
        "MacroSpec",
        &spec,
        fields!(
            &spec,
            MacroSpec {
                parameters,
                primary_output,
                additional_outputs,
            }
        ),
        vec![
            part("parameters", Moves, |m| m.parameters.push("b".to_string())),
            part("primary_output", Moves, |m| {
                m.primary_output = "o".to_string()
            }),
            part("additional_outputs", Moves, |m| {
                m.additional_outputs.clear()
            }),
        ],
        KeyHasher::macro_spec,
    );

    let reference = ModuleReference {
        src: "rate".to_string(),
        dst: "part.input".to_string(),
    };
    check_fields(
        "ModuleReference",
        &reference,
        fields!(&reference, ModuleReference { src, dst }),
        vec![
            part("src", Moves, |r| r.src = "level".to_string()),
            part("dst", Moves, |r| r.dst = "part.other".to_string()),
        ],
        KeyHasher::module_reference,
    );
}

/// A pinned loop is in the key by its name and the variables it resolves
/// to. What it is otherwise -- the uids as written, the ones that resolve to
/// nothing, whether any variable carries a uid, its description -- words a
/// diagnostic or is read by nothing, and stales no run.
#[test]
fn a_pinned_loop_is_in_the_key_by_its_name_and_its_variables() {
    use Key::{Moves, Stays};
    let pin = PinnedLoopSpec {
        name: "drain".to_string(),
        variables: vec!["draining".to_string(), "level".to_string()],
        uids: vec![1, 3, 99],
        unresolved_uids: vec![99],
        model_variables_carry_uids: true,
        description: "the drain".to_string(),
    };
    check_fields(
        "PinnedLoopSpec",
        &pin,
        fields!(
            &pin,
            PinnedLoopSpec {
                name,
                variables,
                uids,
                unresolved_uids,
                model_variables_carry_uids,
                description,
            }
        ),
        vec![
            part("name", Moves, |p| p.name = "leak".to_string()),
            part("variables", Moves, |p| p.variables.push("rate".to_string())),
            part("uids", Stays, |p| p.uids.push(100)),
            part("unresolved_uids", Stays, |p| p.unresolved_uids.push(100)),
            part("model_variables_carry_uids", Stays, |p| {
                p.model_variables_carry_uids = false
            }),
            part("description", Stays, |p| {
                p.description = "where the level goes".to_string()
            }),
        ],
        KeyHasher::pinned_loop,
    );
}

#[test]
fn every_part_of_an_equation_is_in_the_key() {
    let scalar = Equation::Scalar("1".to_string());
    let arrayed = |element: &str,
                   text: &str,
                   initial: Option<&str>,
                   gf: Option<GraphicalFunction>,
                   default: Option<&str>,
                   applies: bool| {
        Equation::Arrayed(
            vec!["region".to_string()],
            vec![(
                element.to_string(),
                text.to_string(),
                initial.map(str::to_string),
                gf,
            )],
            default.map(str::to_string),
            applies,
        )
    };
    let mut other_table = table();
    other_table.y_points[0] = 0.25;
    check_variants(
        "Equation",
        variants!(&scalar, {
            Equation::Scalar(_) => "scalar",
            Equation::ApplyToAll(_, _) => "apply_to_all",
            Equation::Arrayed(_, _, _, _) => "arrayed",
        }),
        vec![
            ("scalar", Equation::Scalar("1".to_string())),
            ("scalar", Equation::Scalar("2".to_string())),
            // Each part of an apply-to-all equation: its dimensions, its text.
            (
                "apply_to_all",
                Equation::ApplyToAll(vec!["region".to_string()], "1".to_string()),
            ),
            (
                "apply_to_all",
                Equation::ApplyToAll(vec!["area".to_string()], "1".to_string()),
            ),
            (
                "apply_to_all",
                Equation::ApplyToAll(vec!["region".to_string()], "2".to_string()),
            ),
            // Each part of an arrayed equation, changed alone from the first:
            // the element, its equation, its initial equation, its table, the
            // default, whether the default applies, and the dimensions.
            ("arrayed", arrayed("north", "1", None, None, None, false)),
            ("arrayed", arrayed("south", "1", None, None, None, false)),
            ("arrayed", arrayed("north", "2", None, None, None, false)),
            (
                "arrayed",
                arrayed("north", "1", Some("0"), None, None, false),
            ),
            (
                "arrayed",
                arrayed("north", "1", None, Some(table()), None, false),
            ),
            (
                "arrayed",
                arrayed("north", "1", None, Some(other_table), None, false),
            ),
            (
                "arrayed",
                arrayed("north", "1", None, None, Some("9"), false),
            ),
            ("arrayed", arrayed("north", "1", None, None, None, true)),
            (
                "arrayed",
                Equation::Arrayed(vec!["area".to_string()], vec![], None, false),
            ),
        ],
        KeyHasher::equation,
    );
}

/// A table is in the key by what a lookup reads: its kind, its points, and
/// its x scale where the x points are not written out and are spread over
/// it. The y scale, and the x scale beside written x points, are how a table
/// is drawn.
#[test]
fn a_table_is_in_the_key_by_what_a_lookup_reads() {
    use Key::{Moves, Stays};
    let written = table();
    let names = || {
        fields!(
            &table(),
            GraphicalFunction {
                kind,
                x_points,
                y_points,
                x_scale,
                y_scale,
            }
        )
    };
    check_fields(
        "GraphicalFunction",
        &written,
        names(),
        vec![
            part("kind", Moves, |t| t.kind = GraphicalFunctionKind::Discrete),
            part("x_points", Moves, |t| {
                t.x_points = Some(vec![0.0, 1.5, 2.0])
            }),
            part("x_points", Moves, |t| t.x_points = None),
            part("y_points", Moves, |t| t.y_points[1] = 0.75),
            part("x_scale", Stays, |t| t.x_scale.max = 4.0),
            part("x_scale", Stays, |t| t.x_scale.min = -1.0),
            part("y_scale", Stays, |t| t.y_scale.max = 4.0),
            part("y_scale", Stays, |t| t.y_scale.min = -1.0),
        ],
        KeyHasher::graphical_function,
    );
    let implicit = GraphicalFunction {
        x_points: None,
        ..table()
    };
    check_fields(
        "GraphicalFunction with implicit x points",
        &implicit,
        names(),
        vec![
            part("kind", Moves, |t| t.kind = GraphicalFunctionKind::Discrete),
            part("x_points", Moves, |t| {
                t.x_points = Some(vec![0.0, 1.0, 2.0])
            }),
            part("y_points", Moves, |t| t.y_points[1] = 0.75),
            part("x_scale", Moves, |t| t.x_scale.max = 4.0),
            part("x_scale", Moves, |t| t.x_scale.min = -1.0),
            part("y_scale", Stays, |t| t.y_scale.max = 4.0),
        ],
        KeyHasher::graphical_function,
    );
    let scale = GraphicalFunctionScale { min: 0.0, max: 2.0 };
    check_fields(
        "GraphicalFunctionScale",
        &scale,
        fields!(&scale, GraphicalFunctionScale { min, max }),
        vec![
            part("min", Moves, |s| s.min = 1.0),
            part("max", Moves, |s| s.max = 3.0),
        ],
        KeyHasher::scale,
    );
    let of_kind = |kind| GraphicalFunction { kind, ..table() };
    check_variants(
        "GraphicalFunctionKind",
        variants!(written.kind, {
            GraphicalFunctionKind::Continuous => "continuous",
            GraphicalFunctionKind::Extrapolate => "extrapolate",
            GraphicalFunctionKind::Discrete => "discrete",
        }),
        vec![
            ("continuous", of_kind(GraphicalFunctionKind::Continuous)),
            ("extrapolate", of_kind(GraphicalFunctionKind::Extrapolate)),
            ("discrete", of_kind(GraphicalFunctionKind::Discrete)),
        ],
        KeyHasher::graphical_function,
    );
}

fn conveyor() -> datamodel::Conveyor {
    datamodel::Conveyor {
        transit_time: "3".to_string(),
        capacity: Some("100".to_string()),
        inflow_limit: Some("10".to_string()),
        sample: Some("s".to_string()),
        arrest: Some("a".to_string()),
        discrete: false,
        batch_integrity: false,
        one_at_a_time: true,
        exponential_leak: false,
        ignore_earlier_zone_losses: false,
    }
}

fn leakage() -> datamodel::Leakage {
    datamodel::Leakage {
        fraction: Some("0.1".to_string()),
        integers: false,
        zone_start: Some("0".to_string()),
        zone_end: Some("1".to_string()),
    }
}

fn data_source() -> datamodel::DataSource {
    datamodel::DataSource {
        kind: datamodel::DataSourceKind::Constants,
        file: "data.csv".to_string(),
        tab_or_delimiter: ",".to_string(),
        row_or_col: "A".to_string(),
        cell: "B2".to_string(),
    }
}

#[test]
fn every_field_of_a_variables_compat_is_in_the_key() {
    use Key::Moves;
    let base = Compat {
        active_initial: Some("1".to_string()),
        non_negative: false,
        can_be_module_input: false,
        visibility: datamodel::Visibility::Private,
        data_source: Some(data_source()),
        conveyor: Some(conveyor()),
        leakage: Some(leakage()),
        spreadflow: Some(datamodel::SpreadFlow::Even),
        queue: None,
        overflow: false,
    };
    check_fields(
        "Compat",
        &base,
        fields!(
            &base,
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
                overflow,
            }
        ),
        vec![
            part("active_initial", Moves, |c| c.active_initial = None),
            part("active_initial", Moves, |c| {
                c.active_initial = Some("2".to_string())
            }),
            part("non_negative", Moves, |c| c.non_negative = true),
            part("can_be_module_input", Moves, |c| {
                c.can_be_module_input = true
            }),
            part("visibility", Moves, |c| {
                c.visibility = datamodel::Visibility::Public
            }),
            part("data_source", Moves, |c| c.data_source = None),
            part("conveyor", Moves, |c| c.conveyor = None),
            part("leakage", Moves, |c| c.leakage = None),
            part("spreadflow", Moves, |c| c.spreadflow = None),
            part("queue", Moves, |c| c.queue = Some(datamodel::Queue {})),
            part("overflow", Moves, |c| c.overflow = true),
        ],
        KeyHasher::compat,
    );
    let queue = datamodel::Queue {};
    let none: Vec<&'static str> = fields!(&queue, datamodel::Queue {});
    assert!(none.is_empty(), "a queue has no field to hash");

    let visible = |visibility| Compat {
        visibility,
        ..base.clone()
    };
    check_variants(
        "Visibility",
        variants!(base.visibility, {
            datamodel::Visibility::Private => "private",
            datamodel::Visibility::Public => "public",
        }),
        vec![
            ("private", visible(datamodel::Visibility::Private)),
            ("public", visible(datamodel::Visibility::Public)),
        ],
        KeyHasher::compat,
    );
}

#[test]
fn every_field_of_a_conveyor_a_leak_and_a_data_source_is_in_the_key() {
    use Key::Moves;
    let base = conveyor();
    check_fields(
        "Conveyor",
        &base,
        fields!(
            &base,
            datamodel::Conveyor {
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
            }
        ),
        vec![
            part("transit_time", Moves, |c| c.transit_time = "4".to_string()),
            part("capacity", Moves, |c| c.capacity = Some("50".to_string())),
            part("capacity", Moves, |c| c.capacity = None),
            part("inflow_limit", Moves, |c| c.inflow_limit = None),
            part("sample", Moves, |c| c.sample = None),
            part("arrest", Moves, |c| c.arrest = None),
            part("discrete", Moves, |c| c.discrete = true),
            part("batch_integrity", Moves, |c| c.batch_integrity = true),
            part("one_at_a_time", Moves, |c| c.one_at_a_time = false),
            part("exponential_leak", Moves, |c| c.exponential_leak = true),
            part("ignore_earlier_zone_losses", Moves, |c| {
                c.ignore_earlier_zone_losses = true
            }),
        ],
        KeyHasher::conveyor,
    );

    let base = leakage();
    check_fields(
        "Leakage",
        &base,
        fields!(
            &base,
            datamodel::Leakage {
                fraction,
                integers,
                zone_start,
                zone_end,
            }
        ),
        vec![
            part("fraction", Moves, |l| l.fraction = None),
            part("integers", Moves, |l| l.integers = true),
            part("zone_start", Moves, |l| l.zone_start = None),
            part("zone_start", Moves, |l| {
                l.zone_start = Some("0.5".to_string())
            }),
            part("zone_end", Moves, |l| l.zone_end = None),
        ],
        KeyHasher::leakage,
    );

    let base = data_source();
    check_fields(
        "DataSource",
        &base,
        fields!(
            &base,
            datamodel::DataSource {
                kind,
                file,
                tab_or_delimiter,
                row_or_col,
                cell,
            }
        ),
        vec![
            part("kind", Moves, |d| d.kind = datamodel::DataSourceKind::Data),
            part("file", Moves, |d| d.file = "other.csv".to_string()),
            part("tab_or_delimiter", Moves, |d| {
                d.tab_or_delimiter = ";".to_string()
            }),
            part("row_or_col", Moves, |d| d.row_or_col = "B".to_string()),
            part("cell", Moves, |d| d.cell = "C3".to_string()),
        ],
        KeyHasher::data_source,
    );
    let of_kind = |kind| datamodel::DataSource {
        kind,
        ..data_source()
    };
    check_variants(
        "DataSourceKind",
        variants!(base.kind, {
            datamodel::DataSourceKind::Data => "data",
            datamodel::DataSourceKind::Constants => "constants",
            datamodel::DataSourceKind::Lookups => "lookups",
            datamodel::DataSourceKind::Subscript => "subscript",
        }),
        vec![
            ("data", of_kind(datamodel::DataSourceKind::Data)),
            ("constants", of_kind(datamodel::DataSourceKind::Constants)),
            ("lookups", of_kind(datamodel::DataSourceKind::Lookups)),
            ("subscript", of_kind(datamodel::DataSourceKind::Subscript)),
        ],
        KeyHasher::data_source,
    );

    check_variants(
        "SpreadFlow",
        variants!(&datamodel::SpreadFlow::Even, {
            datamodel::SpreadFlow::Beginning => "beginning",
            datamodel::SpreadFlow::Even => "even",
            datamodel::SpreadFlow::Dest => "dest",
            datamodel::SpreadFlow::Dist(_) => "dist",
            datamodel::SpreadFlow::Source => "source",
        }),
        vec![
            ("beginning", datamodel::SpreadFlow::Beginning),
            ("even", datamodel::SpreadFlow::Even),
            ("dest", datamodel::SpreadFlow::Dest),
            ("dist", datamodel::SpreadFlow::Dist("a".to_string())),
            ("dist", datamodel::SpreadFlow::Dist("b".to_string())),
            ("source", datamodel::SpreadFlow::Source),
        ],
        KeyHasher::spread_flow,
    );

    check_variants(
        "SourceVariableKind",
        variants!(SourceVariableKind::Aux, {
            SourceVariableKind::Stock => "stock",
            SourceVariableKind::Flow => "flow",
            SourceVariableKind::Aux => "aux",
            SourceVariableKind::Module => "module",
        }),
        vec![
            ("stock", SourceVariableKind::Stock),
            ("flow", SourceVariableKind::Flow),
            ("aux", SourceVariableKind::Aux),
            ("module", SourceVariableKind::Module),
        ],
        KeyHasher::kind,
    );
}
