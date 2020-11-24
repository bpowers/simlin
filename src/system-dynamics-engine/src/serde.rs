// Copyright 2020 The Model Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use crate::datamodel::{
    Aux, Dt, Flow, GraphicalFunction, GraphicalFunctionKind, GraphicalFunctionScale, Model, Module,
    ModuleReference, Project, SimMethod, SimSpecs, Stock, Variable,
};
use crate::project_io;

impl From<Dt> for project_io::Dt {
    fn from(dt: Dt) -> Self {
        match dt {
            Dt::Dt(value) => project_io::Dt {
                value,
                is_reciprocal: false,
            },
            Dt::Reciprocal(value) => project_io::Dt {
                value,
                is_reciprocal: true,
            },
        }
    }
}

impl From<project_io::Dt> for Dt {
    fn from(dt: project_io::Dt) -> Self {
        if dt.is_reciprocal {
            Dt::Reciprocal(dt.value)
        } else {
            Dt::Dt(dt.value)
        }
    }
}

#[test]
fn test_dt_roundtrip() {
    let cases: &[Dt] = &[Dt::Dt(7.7), Dt::Reciprocal(7.7)];
    for expected in cases {
        let expected = expected.clone();
        let actual = Dt::from(project_io::Dt::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<i32> for project_io::SimMethod {
    fn from(value: i32) -> Self {
        match value {
            0 => project_io::SimMethod::Euler,
            1 => project_io::SimMethod::RungeKutta4,
            _ => project_io::SimMethod::Euler,
        }
    }
}

impl From<SimMethod> for project_io::SimMethod {
    fn from(sim_method: SimMethod) -> Self {
        match sim_method {
            SimMethod::Euler => project_io::SimMethod::Euler,
            SimMethod::RungeKutta4 => project_io::SimMethod::RungeKutta4,
        }
    }
}

impl From<project_io::SimMethod> for SimMethod {
    fn from(sim_method: project_io::SimMethod) -> Self {
        match sim_method {
            project_io::SimMethod::Euler => SimMethod::Euler,
            project_io::SimMethod::RungeKutta4 => SimMethod::RungeKutta4,
        }
    }
}

#[test]
fn test_sim_method_roundtrip() {
    let cases: &[SimMethod] = &[SimMethod::Euler, SimMethod::RungeKutta4];
    for expected in cases {
        let expected = expected.clone();
        let actual = SimMethod::from(project_io::SimMethod::from(expected.clone()));
        assert_eq!(expected, actual);
    }

    // protobuf enums are open, which we should just treat as Euler
    assert_eq!(
        SimMethod::Euler,
        SimMethod::from(project_io::SimMethod::from(666))
    );
}

impl From<SimSpecs> for project_io::SimSpecs {
    fn from(sim_specs: SimSpecs) -> Self {
        project_io::SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: Some(project_io::Dt::from(sim_specs.dt)),
            save_step: match sim_specs.save_step {
                None => None,
                Some(dt) => Some(project_io::Dt::from(dt)),
            },
            sim_method: project_io::SimMethod::from(sim_specs.sim_method) as i32,
            time_units: sim_specs.time_units,
        }
    }
}

impl From<project_io::SimSpecs> for SimSpecs {
    fn from(sim_specs: project_io::SimSpecs) -> Self {
        SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: Dt::from(sim_specs.dt.unwrap_or(project_io::Dt {
                value: 1.0,
                is_reciprocal: false,
            })),
            save_step: match sim_specs.save_step {
                Some(dt) => Some(Dt::from(dt)),
                None => None,
            },
            sim_method: SimMethod::from(project_io::SimMethod::from(sim_specs.sim_method)),
            time_units: sim_specs.time_units,
        }
    }
}

#[test]
fn test_sim_specs_roundtrip() {
    let cases: &[SimSpecs] = &[
        SimSpecs {
            start: 127.0,
            stop: 129.9,
            dt: Dt::Reciprocal(4.0),
            save_step: Some(Dt::Dt(1.0)),
            sim_method: SimMethod::Euler,
            time_units: Some("years".to_string()),
        },
        SimSpecs {
            start: 127.0,
            stop: 129.9,
            dt: Dt::Dt(5.0),
            save_step: None,
            sim_method: SimMethod::RungeKutta4,
            time_units: None,
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = SimSpecs::from(project_io::SimSpecs::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<GraphicalFunctionKind> for project_io::graphical_function::Kind {
    fn from(kind: GraphicalFunctionKind) -> Self {
        match kind {
            GraphicalFunctionKind::Continuous => project_io::graphical_function::Kind::Continuous,
            GraphicalFunctionKind::Discrete => project_io::graphical_function::Kind::Discrete,
            GraphicalFunctionKind::Extrapolate => project_io::graphical_function::Kind::Extrapolate,
        }
    }
}

impl From<project_io::graphical_function::Kind> for GraphicalFunctionKind {
    fn from(kind: project_io::graphical_function::Kind) -> Self {
        match kind {
            project_io::graphical_function::Kind::Continuous => GraphicalFunctionKind::Continuous,
            project_io::graphical_function::Kind::Discrete => GraphicalFunctionKind::Discrete,
            project_io::graphical_function::Kind::Extrapolate => GraphicalFunctionKind::Extrapolate,
        }
    }
}

impl From<i32> for project_io::graphical_function::Kind {
    fn from(value: i32) -> Self {
        match value {
            0 => project_io::graphical_function::Kind::Continuous,
            1 => project_io::graphical_function::Kind::Discrete,
            2 => project_io::graphical_function::Kind::Extrapolate,
            _ => project_io::graphical_function::Kind::Continuous,
        }
    }
}

#[test]
fn test_graphical_function_kind_roundtrip() {
    let cases: &[GraphicalFunctionKind] = &[
        GraphicalFunctionKind::Discrete,
        GraphicalFunctionKind::Continuous,
        GraphicalFunctionKind::Extrapolate,
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = GraphicalFunctionKind::from(project_io::graphical_function::Kind::from(
            expected.clone(),
        ));
        assert_eq!(expected, actual);
    }

    assert_eq!(
        project_io::graphical_function::Kind::Continuous,
        project_io::graphical_function::Kind::from(666)
    );
}

impl From<GraphicalFunctionScale> for project_io::graphical_function::Scale {
    fn from(scale: GraphicalFunctionScale) -> Self {
        project_io::graphical_function::Scale {
            min: scale.min,
            max: scale.max,
        }
    }
}

impl From<project_io::graphical_function::Scale> for GraphicalFunctionScale {
    fn from(scale: project_io::graphical_function::Scale) -> Self {
        GraphicalFunctionScale {
            min: scale.min,
            max: scale.max,
        }
    }
}

#[test]
fn test_graphical_function_scale_roundtrip() {
    let cases: &[GraphicalFunctionScale] = &[GraphicalFunctionScale {
        min: 1.0,
        max: 129.0,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual = GraphicalFunctionScale::from(project_io::graphical_function::Scale::from(
            expected.clone(),
        ));
        assert_eq!(expected, actual);
    }
}

impl From<GraphicalFunction> for project_io::GraphicalFunction {
    fn from(gf: GraphicalFunction) -> Self {
        project_io::GraphicalFunction {
            kind: project_io::graphical_function::Kind::from(gf.kind) as i32,
            x_points: gf.x_points.unwrap_or_default(),
            y_points: gf.y_points,
            x_scale: Some(project_io::graphical_function::Scale::from(gf.x_scale)),
            y_scale: Some(project_io::graphical_function::Scale::from(gf.y_scale)),
        }
    }
}

impl From<project_io::GraphicalFunction> for GraphicalFunction {
    fn from(gf: project_io::GraphicalFunction) -> Self {
        GraphicalFunction {
            kind: GraphicalFunctionKind::from(project_io::graphical_function::Kind::from(gf.kind)),
            x_points: if gf.x_points.is_empty() {
                None
            } else {
                Some(gf.x_points)
            },
            y_points: gf.y_points,
            x_scale: GraphicalFunctionScale::from(gf.x_scale.unwrap()),
            y_scale: GraphicalFunctionScale::from(gf.y_scale.unwrap()),
        }
    }
}

#[test]
fn test_graphical_function_roundtrip() {
    let cases: &[GraphicalFunction] = &[
        GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: None,
            y_points: vec![1.0, 2.0, 3.0],
            x_scale: GraphicalFunctionScale { min: 1.0, max: 7.0 },
            y_scale: GraphicalFunctionScale { min: 2.0, max: 8.0 },
        },
        GraphicalFunction {
            kind: GraphicalFunctionKind::Continuous,
            x_points: Some(vec![9.0, 9.1, 9.2]),
            y_points: vec![1.0, 2.0, 3.0],
            x_scale: GraphicalFunctionScale { min: 1.0, max: 7.0 },
            y_scale: GraphicalFunctionScale { min: 2.0, max: 8.0 },
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = GraphicalFunction::from(project_io::GraphicalFunction::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<Stock> for project_io::variable::Stock {
    fn from(stock: Stock) -> Self {
        project_io::variable::Stock {
            ident: stock.ident,
            equation: stock.equation,
            documentation: stock.documentation,
            units: stock.units.unwrap_or_default(),
            inflows: stock.inflows,
            outflows: stock.outflows,
            non_negative: stock.non_negative,
        }
    }
}

impl From<project_io::variable::Stock> for Stock {
    fn from(stock: project_io::variable::Stock) -> Self {
        Stock {
            ident: stock.ident,
            equation: stock.equation,
            documentation: stock.documentation,
            units: if stock.units.is_empty() {
                None
            } else {
                Some(stock.units)
            },
            inflows: stock.inflows,
            outflows: stock.outflows,
            non_negative: stock.non_negative,
        }
    }
}

#[test]
fn test_stock_roundtrip() {
    let cases: &[Stock] = &[
        Stock {
            ident: "blerg".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: None,
            inflows: vec!["inflow".to_string()],
            outflows: vec![],
            non_negative: false,
        },
        Stock {
            ident: "blerg2".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: Some("flarbles".to_string()),
            inflows: vec!["inflow".to_string()],
            outflows: vec![],
            non_negative: false,
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Stock::from(project_io::variable::Stock::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<Flow> for project_io::variable::Flow {
    fn from(flow: Flow) -> Self {
        project_io::variable::Flow {
            ident: flow.ident,
            equation: flow.equation,
            documentation: flow.documentation,
            units: flow.units.unwrap_or_default(),
            gf: match flow.gf {
                Some(gf) => Some(project_io::GraphicalFunction::from(gf)),
                None => None,
            },
            non_negative: flow.non_negative,
        }
    }
}

impl From<project_io::variable::Flow> for Flow {
    fn from(flow: project_io::variable::Flow) -> Self {
        Flow {
            ident: flow.ident,
            equation: flow.equation,
            documentation: flow.documentation,
            units: if flow.units.is_empty() {
                None
            } else {
                Some(flow.units)
            },
            gf: match flow.gf {
                Some(gf) => Some(GraphicalFunction::from(gf)),
                None => None,
            },
            non_negative: flow.non_negative,
        }
    }
}

#[test]
fn test_flow_roundtrip() {
    let cases: &[Flow] = &[
        Flow {
            ident: "blerg".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: None,
            gf: None,
            non_negative: false,
        },
        Flow {
            ident: "blerg2".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: Some("flarbles".to_string()),
            gf: Some(GraphicalFunction {
                kind: GraphicalFunctionKind::Extrapolate,
                x_points: Some(vec![9.3, 9.1, 9.2]),
                y_points: vec![1.0, 2.0, 6.0],
                x_scale: GraphicalFunctionScale {
                    min: 1.0,
                    max: 7.01,
                },
                y_scale: GraphicalFunctionScale {
                    min: 2.0,
                    max: 8.01,
                },
            }),
            non_negative: false,
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Flow::from(project_io::variable::Flow::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<Aux> for project_io::variable::Aux {
    fn from(aux: Aux) -> Self {
        project_io::variable::Aux {
            ident: aux.ident,
            equation: aux.equation,
            documentation: aux.documentation,
            units: aux.units.unwrap_or_default(),
            gf: match aux.gf {
                Some(gf) => Some(project_io::GraphicalFunction::from(gf)),
                None => None,
            },
        }
    }
}

impl From<project_io::variable::Aux> for Aux {
    fn from(aux: project_io::variable::Aux) -> Self {
        Aux {
            ident: aux.ident,
            equation: aux.equation,
            documentation: aux.documentation,
            units: if aux.units.is_empty() {
                None
            } else {
                Some(aux.units)
            },
            gf: match aux.gf {
                Some(gf) => Some(GraphicalFunction::from(gf)),
                None => None,
            },
        }
    }
}

#[test]
fn test_aux_roundtrip() {
    let cases: &[Aux] = &[
        Aux {
            ident: "blerg".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: None,
            gf: None,
        },
        Aux {
            ident: "blerg2".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: Some("flarbles".to_string()),
            gf: Some(GraphicalFunction {
                kind: GraphicalFunctionKind::Extrapolate,
                x_points: Some(vec![9.3, 9.1, 9.2]),
                y_points: vec![1.0, 2.0, 6.0],
                x_scale: GraphicalFunctionScale {
                    min: 1.0,
                    max: 7.01,
                },
                y_scale: GraphicalFunctionScale {
                    min: 2.0,
                    max: 8.01,
                },
            }),
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Aux::from(project_io::variable::Aux::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<ModuleReference> for project_io::variable::module::Reference {
    fn from(mod_ref: ModuleReference) -> Self {
        project_io::variable::module::Reference {
            src: mod_ref.src,
            dst: mod_ref.dst,
        }
    }
}

impl From<project_io::variable::module::Reference> for ModuleReference {
    fn from(mod_ref: project_io::variable::module::Reference) -> Self {
        ModuleReference {
            src: mod_ref.src,
            dst: mod_ref.dst,
        }
    }
}

#[test]
fn test_module_reference_roundtrip() {
    let cases: &[ModuleReference] = &[ModuleReference {
        src: "foo".to_string(),
        dst: "self.bar".to_string(),
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual = ModuleReference::from(project_io::variable::module::Reference::from(
            expected.clone(),
        ));
        assert_eq!(expected, actual);
    }
}

impl From<Module> for project_io::variable::Module {
    fn from(module: Module) -> Self {
        project_io::variable::Module {
            ident: module.ident,
            model_name: module.model_name,
            documentation: module.documentation,
            units: module.units.unwrap_or_default(),
            references: module
                .references
                .into_iter()
                .map(project_io::variable::module::Reference::from)
                .collect(),
        }
    }
}

impl From<project_io::variable::Module> for Module {
    fn from(module: project_io::variable::Module) -> Self {
        Module {
            ident: module.ident,
            model_name: module.model_name,
            documentation: module.documentation,
            units: if module.units.is_empty() {
                None
            } else {
                Some(module.units)
            },
            references: module
                .references
                .into_iter()
                .map(ModuleReference::from)
                .collect(),
        }
    }
}

#[test]
fn test_module_roundtrip() {
    let cases: &[Module] = &[
        Module {
            ident: "blerg".to_string(),
            model_name: "blergers".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: None,
            references: vec![ModuleReference {
                src: "foo".to_string(),
                dst: "self.bar".to_string(),
            }],
        },
        Module {
            ident: "blerg2".to_string(),
            model_name: "blergers2".to_string(),
            documentation: "this is deeper stuff".to_string(),
            units: Some("flarbles".to_string()),
            references: vec![],
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Module::from(project_io::variable::Module::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<Variable> for project_io::Variable {
    fn from(var: Variable) -> Self {
        let v = match var {
            Variable::Stock(stock) => {
                project_io::variable::V::Stock(project_io::variable::Stock::from(stock))
            }
            Variable::Flow(flow) => {
                project_io::variable::V::Flow(project_io::variable::Flow::from(flow))
            }
            Variable::Aux(aux) => {
                project_io::variable::V::Aux(project_io::variable::Aux::from(aux))
            }
            Variable::Module(module) => {
                project_io::variable::V::Module(project_io::variable::Module::from(module))
            }
        };
        project_io::Variable { v: Some(v) }
    }
}

impl From<project_io::Variable> for Variable {
    fn from(var: project_io::Variable) -> Self {
        match var.v.unwrap() {
            project_io::variable::V::Stock(stock) => Variable::Stock(Stock::from(stock)),
            project_io::variable::V::Flow(flow) => Variable::Flow(Flow::from(flow)),
            project_io::variable::V::Aux(aux) => Variable::Aux(Aux::from(aux)),
            project_io::variable::V::Module(module) => Variable::Module(Module::from(module)),
        }
    }
}

#[test]
fn test_variable_roundtrip() {
    let cases: &[Variable] = &[
        Variable::Aux(Aux {
            ident: "blerg".to_string(),
            equation: "1+3".to_string(),
            documentation: "this is deep stuff".to_string(),
            units: None,
            gf: None,
        }),
        Variable::Module(Module {
            ident: "blerg2".to_string(),
            model_name: "blergers2".to_string(),
            documentation: "this is deeper stuff".to_string(),
            units: Some("flarbles".to_string()),
            references: vec![],
        }),
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Variable::from(project_io::Variable::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<Model> for project_io::Model {
    fn from(model: Model) -> Self {
        project_io::Model {
            name: model.name,
            variables: model
                .variables
                .into_iter()
                .map(project_io::Variable::from)
                .collect(),
            views: vec![],
        }
    }
}

impl From<project_io::Model> for Model {
    fn from(model: project_io::Model) -> Self {
        Model {
            name: model.name,
            variables: model.variables.into_iter().map(Variable::from).collect(),
            views: vec![],
        }
    }
}

impl From<Project> for project_io::Project {
    fn from(project: Project) -> Self {
        project_io::Project {
            name: project.name,
            sim_specs: Some(project_io::SimSpecs::from(project.sim_specs)),
            models: project
                .models
                .into_iter()
                .map(project_io::Model::from)
                .collect(),
        }
    }
}

impl From<project_io::Project> for Project {
    fn from(project: project_io::Project) -> Self {
        Project {
            name: project.name,
            sim_specs: SimSpecs::from(project.sim_specs.unwrap()),
            models: project.models.into_iter().map(Model::from).collect(),
        }
    }
}

pub fn serialize(project: &Project) -> project_io::Project {
    project_io::Project::from(project.clone())
}

pub fn deserialize(project: project_io::Project) -> Project {
    Project::from(project)
}
