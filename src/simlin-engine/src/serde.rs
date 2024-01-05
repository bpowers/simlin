// Copyright 2021 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use float_cmp::approx_eq;

use crate::datamodel::{
    view_element, Aux, Dimension, Dt, Equation, Extension, Flow, GraphicalFunction,
    GraphicalFunctionKind, GraphicalFunctionScale, Model, Module, ModuleReference, Project, Rect,
    SimMethod, SimSpecs, Source, Stock, StockFlow, Unit, Variable, View, ViewElement, Visibility,
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
        let actual =
            SimMethod::from(project_io::SimMethod::try_from(expected.clone()).unwrap_or_default());
        assert_eq!(expected, actual);
    }

    // protobuf enums are open, which we should just treat as Euler
    assert_eq!(
        SimMethod::Euler,
        SimMethod::from(project_io::SimMethod::try_from(666).unwrap_or_default())
    );
}

impl From<SimSpecs> for project_io::SimSpecs {
    fn from(sim_specs: SimSpecs) -> Self {
        project_io::SimSpecs {
            start: sim_specs.start,
            stop: sim_specs.stop,
            dt: Some(project_io::Dt::from(sim_specs.dt)),
            save_step: sim_specs.save_step.map(project_io::Dt::from),
            sim_method: project_io::SimMethod::from(sim_specs.sim_method) as i32,
            time_units: sim_specs.time_units.unwrap_or_default(),
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
            save_step: sim_specs.save_step.map(Dt::from),
            sim_method: SimMethod::from(
                project_io::SimMethod::try_from(sim_specs.sim_method).unwrap_or_default(),
            ),
            time_units: if sim_specs.time_units.is_empty() {
                None
            } else {
                Some(sim_specs.time_units)
            },
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

#[test]
fn test_graphical_function_kind_roundtrip() {
    let cases: &[GraphicalFunctionKind] = &[
        GraphicalFunctionKind::Discrete,
        GraphicalFunctionKind::Continuous,
        GraphicalFunctionKind::Extrapolate,
    ];
    for expected in cases {
        let expected = *expected;
        let actual = GraphicalFunctionKind::from(
            project_io::graphical_function::Kind::try_from(expected).unwrap_or_default(),
        );
        assert_eq!(expected, actual);
    }

    assert_eq!(
        project_io::graphical_function::Kind::Continuous,
        project_io::graphical_function::Kind::try_from(666).unwrap_or_default()
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
            kind: GraphicalFunctionKind::from(
                project_io::graphical_function::Kind::try_from(gf.kind).unwrap_or_default(),
            ),
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

impl From<Equation> for project_io::variable::Equation {
    fn from(eqn: Equation) -> Self {
        project_io::variable::Equation {
            equation: Some(match eqn {
                Equation::Scalar(equation, initial_equation) => {
                    project_io::variable::equation::Equation::Scalar(
                        project_io::variable::ScalarEquation {
                            equation,
                            initial_equation,
                        },
                    )
                }
                Equation::ApplyToAll(dimension_names, equation, initial_equation) => {
                    project_io::variable::equation::Equation::ApplyToAll(
                        project_io::variable::ApplyToAllEquation {
                            dimension_names,
                            equation,
                            initial_equation,
                        },
                    )
                }
                Equation::Arrayed(dimension_names, elements) => {
                    project_io::variable::equation::Equation::Arrayed(
                        project_io::variable::ArrayedEquation {
                            dimension_names,
                            elements: elements
                                .into_iter()
                                .map(|(subscript, equation, initial_equation)| {
                                    project_io::variable::arrayed_equation::Element {
                                        subscript,
                                        equation,
                                        initial_equation,
                                    }
                                })
                                .collect(),
                        },
                    )
                }
            }),
        }
    }
}

impl From<project_io::variable::Equation> for Equation {
    fn from(eqn: project_io::variable::Equation) -> Self {
        match eqn.equation.unwrap() {
            project_io::variable::equation::Equation::Scalar(scalar) => {
                Equation::Scalar(scalar.equation, scalar.initial_equation)
            }
            project_io::variable::equation::Equation::ApplyToAll(a2a) => {
                Equation::ApplyToAll(a2a.dimension_names, a2a.equation, a2a.initial_equation)
            }
            project_io::variable::equation::Equation::Arrayed(arrayed) => Equation::Arrayed(
                arrayed.dimension_names,
                arrayed
                    .elements
                    .into_iter()
                    .map(|e| (e.subscript, e.equation, e.initial_equation))
                    .collect(),
            ),
        }
    }
}

#[test]
fn test_equation_roundtrip() {
    let cases: &[_] = &[
        Equation::Scalar("a+1".to_string(), None),
        Equation::Scalar("a+1".to_string(), Some("392".to_string())),
        Equation::ApplyToAll(
            vec!["a".to_string(), "b".to_string()],
            "c+2".to_string(),
            None,
        ),
        Equation::ApplyToAll(
            vec!["a".to_string(), "b".to_string()],
            "c+2".to_string(),
            Some("33".to_string()),
        ),
        Equation::Arrayed(
            vec!["d".to_string()],
            vec![
                ("e".to_string(), "3".to_string(), None),
                ("f".to_string(), "7+1".to_string(), Some("l".to_string())),
            ],
        ),
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Equation::from(project_io::variable::Equation::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<Visibility> for project_io::variable::Visibility {
    fn from(visibility: Visibility) -> Self {
        match visibility {
            Visibility::Private => project_io::variable::Visibility::Private,
            Visibility::Public => project_io::variable::Visibility::Public,
        }
    }
}

impl From<project_io::variable::Visibility> for Visibility {
    fn from(visibility: project_io::variable::Visibility) -> Self {
        match visibility {
            project_io::variable::Visibility::Private => Visibility::Private,
            project_io::variable::Visibility::Public => Visibility::Public,
        }
    }
}

#[test]
fn test_visibility_roundtrip() {
    let cases: &[Visibility] = &[Visibility::Private, Visibility::Public];
    for expected in cases {
        let expected = *expected;
        let actual = Visibility::from(
            project_io::variable::Visibility::try_from(expected).unwrap_or_default(),
        );
        assert_eq!(expected, actual);
    }

    assert_eq!(
        project_io::variable::Visibility::Private,
        project_io::variable::Visibility::try_from(666).unwrap_or_default()
    );
}

impl From<Stock> for project_io::variable::Stock {
    fn from(stock: Stock) -> Self {
        project_io::variable::Stock {
            ident: stock.ident,
            equation: Some(stock.equation.into()),
            documentation: stock.documentation,
            units: stock.units.unwrap_or_default(),
            inflows: stock.inflows,
            outflows: stock.outflows,
            non_negative: stock.non_negative,
            can_be_module_input: stock.can_be_module_input,
            visibility: project_io::variable::Visibility::from(stock.visibility) as i32,
        }
    }
}

impl From<project_io::variable::Stock> for Stock {
    fn from(stock: project_io::variable::Stock) -> Self {
        Stock {
            ident: stock.ident,
            equation: stock.equation.unwrap().into(),
            documentation: stock.documentation,
            units: if stock.units.is_empty() {
                None
            } else {
                Some(stock.units)
            },
            inflows: stock.inflows,
            outflows: stock.outflows,
            non_negative: stock.non_negative,
            can_be_module_input: stock.can_be_module_input,
            visibility: Visibility::from(
                project_io::variable::Visibility::try_from(stock.visibility).unwrap_or_default(),
            ),
        }
    }
}

#[test]
fn test_stock_roundtrip() {
    let cases: &[Stock] = &[
        Stock {
            ident: "blerg".to_string(),
            equation: Equation::Scalar("1+3".to_string(), None),
            documentation: "this is deep stuff".to_string(),
            units: None,
            inflows: vec!["inflow".to_string()],
            outflows: vec![],
            non_negative: false,
            can_be_module_input: true,
            visibility: Visibility::Public,
        },
        Stock {
            ident: "blerg2".to_string(),
            equation: Equation::Scalar("1+3".to_string(), None),
            documentation: "this is deep stuff".to_string(),
            units: Some("flarbles".to_string()),
            inflows: vec!["inflow".to_string()],
            outflows: vec![],
            non_negative: false,
            can_be_module_input: false,
            visibility: Visibility::Private,
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
            equation: Some(flow.equation.into()),
            documentation: flow.documentation,
            units: flow.units.unwrap_or_default(),
            gf: flow.gf.map(project_io::GraphicalFunction::from),
            non_negative: flow.non_negative,
            can_be_module_input: flow.can_be_module_input,
            visibility: project_io::variable::Visibility::from(flow.visibility) as i32,
        }
    }
}

impl From<project_io::variable::Flow> for Flow {
    fn from(flow: project_io::variable::Flow) -> Self {
        Flow {
            ident: flow.ident,
            equation: flow.equation.unwrap().into(),
            documentation: flow.documentation,
            units: if flow.units.is_empty() {
                None
            } else {
                Some(flow.units)
            },
            gf: flow.gf.map(GraphicalFunction::from),
            non_negative: flow.non_negative,
            can_be_module_input: flow.can_be_module_input,
            visibility: Visibility::from(
                project_io::variable::Visibility::try_from(flow.visibility).unwrap_or_default(),
            ),
        }
    }
}

#[test]
fn test_flow_roundtrip() {
    let cases: &[Flow] = &[
        Flow {
            ident: "blerg".to_string(),
            equation: Equation::Scalar("1+3".to_string(), None),
            documentation: "this is deep stuff".to_string(),
            units: None,
            gf: None,
            non_negative: false,
            can_be_module_input: true,
            visibility: Visibility::Private,
        },
        Flow {
            ident: "blerg2".to_string(),
            equation: Equation::Scalar("1+3".to_string(), Some("66".to_string())),
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
            can_be_module_input: false,
            visibility: Visibility::Public,
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
            equation: Some(aux.equation.into()),
            documentation: aux.documentation,
            units: aux.units.unwrap_or_default(),
            gf: aux.gf.map(project_io::GraphicalFunction::from),
            can_be_module_input: aux.can_be_module_input,
            visibility: project_io::variable::Visibility::from(aux.visibility).into(),
        }
    }
}

impl From<project_io::variable::Aux> for Aux {
    fn from(aux: project_io::variable::Aux) -> Self {
        Aux {
            ident: aux.ident,
            equation: aux.equation.unwrap().into(),
            documentation: aux.documentation,
            units: if aux.units.is_empty() {
                None
            } else {
                Some(aux.units)
            },
            gf: aux.gf.map(GraphicalFunction::from),
            can_be_module_input: aux.can_be_module_input,
            visibility: Visibility::from(
                project_io::variable::Visibility::try_from(aux.visibility).unwrap_or_default(),
            ),
        }
    }
}

#[test]
fn test_aux_roundtrip() {
    let cases: &[Aux] = &[
        Aux {
            ident: "blerg".to_string(),
            equation: Equation::Scalar("1+3".to_string(), Some("11".to_string())),
            documentation: "this is deep stuff".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: Visibility::Public,
        },
        Aux {
            ident: "blerg2".to_string(),
            equation: Equation::Scalar("1+3".to_string(), None),
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
            can_be_module_input: true,
            visibility: Visibility::Private,
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
            can_be_module_input: module.can_be_module_input,
            visibility: project_io::variable::Visibility::from(module.visibility) as i32,
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
            can_be_module_input: module.can_be_module_input,
            visibility: Visibility::from(
                project_io::variable::Visibility::try_from(module.visibility).unwrap_or_default(),
            ),
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
            can_be_module_input: false,
            visibility: Visibility::Private,
        },
        Module {
            ident: "blerg2".to_string(),
            model_name: "blergers2".to_string(),
            documentation: "this is deeper stuff".to_string(),
            units: Some("flarbles".to_string()),
            references: vec![],
            can_be_module_input: true,
            visibility: Visibility::Public,
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
            equation: Equation::Scalar("1+3".to_string(), None),
            documentation: "this is deep stuff".to_string(),
            units: None,
            gf: None,
            can_be_module_input: false,
            visibility: Visibility::Public,
        }),
        Variable::Module(Module {
            ident: "blerg2".to_string(),
            model_name: "blergers2".to_string(),
            documentation: "this is deeper stuff".to_string(),
            units: Some("flarbles".to_string()),
            references: vec![],
            can_be_module_input: true,
            visibility: Visibility::Private,
        }),
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = Variable::from(project_io::Variable::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::LabelSide> for view_element::LabelSide {
    fn from(label_side: project_io::view_element::LabelSide) -> Self {
        match label_side {
            project_io::view_element::LabelSide::Top => view_element::LabelSide::Top,
            project_io::view_element::LabelSide::Left => view_element::LabelSide::Left,
            project_io::view_element::LabelSide::Center => view_element::LabelSide::Center,
            project_io::view_element::LabelSide::Bottom => view_element::LabelSide::Bottom,
            project_io::view_element::LabelSide::Right => view_element::LabelSide::Right,
        }
    }
}

impl From<view_element::LabelSide> for project_io::view_element::LabelSide {
    fn from(label_side: view_element::LabelSide) -> Self {
        match label_side {
            view_element::LabelSide::Top => project_io::view_element::LabelSide::Top,
            view_element::LabelSide::Left => project_io::view_element::LabelSide::Left,
            view_element::LabelSide::Center => project_io::view_element::LabelSide::Center,
            view_element::LabelSide::Bottom => project_io::view_element::LabelSide::Bottom,
            view_element::LabelSide::Right => project_io::view_element::LabelSide::Right,
        }
    }
}

#[test]
fn test_label_side_roundtrip() {
    let cases: &[_] = &[
        view_element::LabelSide::Top,
        view_element::LabelSide::Left,
        view_element::LabelSide::Center,
        view_element::LabelSide::Bottom,
        view_element::LabelSide::Right,
    ];
    for expected in cases {
        let expected = *expected;
        let actual = view_element::LabelSide::from(
            project_io::view_element::LabelSide::try_from(expected).unwrap_or_default(),
        );
        assert_eq!(expected, actual);
    }

    assert_eq!(
        project_io::view_element::LabelSide::Top,
        project_io::view_element::LabelSide::try_from(666).unwrap_or_default()
    );
}

impl From<project_io::view_element::Aux> for view_element::Aux {
    fn from(v: project_io::view_element::Aux) -> Self {
        view_element::Aux {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: view_element::LabelSide::from(
                project_io::view_element::LabelSide::try_from(v.label_side).unwrap_or_default(),
            ),
        }
    }
}

impl From<view_element::Aux> for project_io::view_element::Aux {
    fn from(v: view_element::Aux) -> Self {
        project_io::view_element::Aux {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: project_io::view_element::LabelSide::from(v.label_side) as i32,
        }
    }
}

#[test]
fn test_view_element_aux_roundtrip() {
    let cases: &[_] = &[view_element::Aux {
        name: "var1".to_string(),
        uid: 123,
        x: 2.0,
        y: 3.0,
        label_side: view_element::LabelSide::Top,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual = view_element::Aux::from(project_io::view_element::Aux::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::Stock> for view_element::Stock {
    fn from(v: project_io::view_element::Stock) -> Self {
        view_element::Stock {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: view_element::LabelSide::from(
                project_io::view_element::LabelSide::try_from(v.label_side).unwrap_or_default(),
            ),
        }
    }
}

impl From<view_element::Stock> for project_io::view_element::Stock {
    fn from(v: view_element::Stock) -> Self {
        project_io::view_element::Stock {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: project_io::view_element::LabelSide::from(v.label_side) as i32,
        }
    }
}

#[test]
fn test_view_element_stock_roundtrip() {
    let cases: &[_] = &[view_element::Stock {
        name: "var2".to_string(),
        uid: 123,
        x: 2.0,
        y: 3.0,
        label_side: view_element::LabelSide::Top,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual =
            view_element::Stock::from(project_io::view_element::Stock::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::FlowPoint> for view_element::FlowPoint {
    fn from(v: project_io::view_element::FlowPoint) -> Self {
        view_element::FlowPoint {
            x: v.x,
            y: v.y,
            attached_to_uid: if v.attached_to_uid > 0 {
                Some(v.attached_to_uid)
            } else {
                None
            },
        }
    }
}

impl From<view_element::FlowPoint> for project_io::view_element::FlowPoint {
    fn from(v: view_element::FlowPoint) -> Self {
        project_io::view_element::FlowPoint {
            x: v.x,
            y: v.y,
            attached_to_uid: v.attached_to_uid.unwrap_or_default(),
        }
    }
}

#[test]
fn test_view_element_flow_point_roundtrip() {
    let cases: &[_] = &[
        view_element::FlowPoint {
            x: 2.0,
            y: 3.0,
            attached_to_uid: Some(31),
        },
        view_element::FlowPoint {
            x: 4.0,
            y: 5.0,
            attached_to_uid: None,
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual = view_element::FlowPoint::from(project_io::view_element::FlowPoint::from(
            expected.clone(),
        ));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::Rect> for Rect {
    fn from(v: project_io::Rect) -> Self {
        Rect {
            x: v.x,
            y: v.y,
            width: v.width,
            height: v.height,
        }
    }
}

impl From<Rect> for project_io::Rect {
    fn from(v: Rect) -> Self {
        project_io::Rect {
            x: v.x,
            y: v.y,
            width: v.width,
            height: v.height,
        }
    }
}

#[test]
fn test_offset_roundtrip() {
    let cases: &[_] = &[Rect {
        x: 7.2,
        y: 8.1,
        width: 12.3,
        height: 34.5,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual = Rect::from(project_io::Rect::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::Flow> for view_element::Flow {
    fn from(v: project_io::view_element::Flow) -> Self {
        view_element::Flow {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: view_element::LabelSide::from(
                project_io::view_element::LabelSide::try_from(v.label_side).unwrap_or_default(),
            ),
            points: v
                .points
                .into_iter()
                .map(view_element::FlowPoint::from)
                .collect(),
        }
    }
}

impl From<view_element::Flow> for project_io::view_element::Flow {
    fn from(v: view_element::Flow) -> Self {
        project_io::view_element::Flow {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: project_io::view_element::LabelSide::from(v.label_side) as i32,
            points: v
                .points
                .into_iter()
                .map(project_io::view_element::FlowPoint::from)
                .collect(),
        }
    }
}

#[test]
fn test_view_element_flow_roundtrip() {
    let cases: &[_] = &[view_element::Flow {
        name: "var2".to_string(),
        uid: 123,
        x: 2.0,
        y: 3.0,
        label_side: view_element::LabelSide::Top,
        points: vec![
            view_element::FlowPoint {
                x: 6.0,
                y: 7.0,
                attached_to_uid: Some(34),
            },
            view_element::FlowPoint {
                x: 8.0,
                y: 9.0,
                attached_to_uid: None,
            },
        ],
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual =
            view_element::Flow::from(project_io::view_element::Flow::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::Link> for view_element::Link {
    fn from(v: project_io::view_element::Link) -> Self {
        view_element::Link {
            uid: v.uid,
            from_uid: v.from_uid,
            to_uid: v.to_uid,
            shape: match v
                .shape
                .unwrap_or(project_io::view_element::link::Shape::IsStraight(true))
            {
                project_io::view_element::link::Shape::Arc(angle) => {
                    view_element::LinkShape::Arc(angle)
                }
                project_io::view_element::link::Shape::IsStraight(_) => {
                    view_element::LinkShape::Straight
                }
                project_io::view_element::link::Shape::MultiPoint(points) => {
                    view_element::LinkShape::MultiPoint(
                        points
                            .points
                            .into_iter()
                            .map(view_element::FlowPoint::from)
                            .collect(),
                    )
                }
            },
        }
    }
}

impl From<view_element::Link> for project_io::view_element::Link {
    fn from(v: view_element::Link) -> Self {
        project_io::view_element::Link {
            uid: v.uid,
            from_uid: v.from_uid,
            to_uid: v.to_uid,
            shape: match v.shape {
                view_element::LinkShape::Arc(angle) => {
                    Some(project_io::view_element::link::Shape::Arc(angle))
                }
                view_element::LinkShape::Straight => {
                    Some(project_io::view_element::link::Shape::IsStraight(true))
                }
                view_element::LinkShape::MultiPoint(points) => {
                    Some(project_io::view_element::link::Shape::MultiPoint(
                        project_io::view_element::link::LinkPoints {
                            points: points
                                .into_iter()
                                .map(project_io::view_element::FlowPoint::from)
                                .collect(),
                        },
                    ))
                }
            },
        }
    }
}

#[test]
fn test_view_element_link_roundtrip() {
    let cases: &[_] = &[
        view_element::Link {
            uid: 123,
            from_uid: 21,
            to_uid: 22,
            shape: view_element::LinkShape::Straight,
        },
        view_element::Link {
            uid: 123,
            from_uid: 21,
            to_uid: 22,
            shape: view_element::LinkShape::Arc(351.0),
        },
        view_element::Link {
            uid: 123,
            from_uid: 21,
            to_uid: 22,
            shape: view_element::LinkShape::MultiPoint(vec![
                view_element::FlowPoint {
                    x: 6.0,
                    y: 7.0,
                    attached_to_uid: Some(34),
                },
                view_element::FlowPoint {
                    x: 8.0,
                    y: 9.0,
                    attached_to_uid: None,
                },
            ]),
        },
    ];
    for expected in cases {
        let expected = expected.clone();
        let actual =
            view_element::Link::from(project_io::view_element::Link::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::Module> for view_element::Module {
    fn from(v: project_io::view_element::Module) -> Self {
        view_element::Module {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: view_element::LabelSide::from(
                project_io::view_element::LabelSide::try_from(v.label_side).unwrap_or_default(),
            ),
        }
    }
}

impl From<view_element::Module> for project_io::view_element::Module {
    fn from(v: view_element::Module) -> Self {
        project_io::view_element::Module {
            name: v.name,
            uid: v.uid,
            x: v.x,
            y: v.y,
            label_side: project_io::view_element::LabelSide::from(v.label_side) as i32,
        }
    }
}

#[test]
fn test_view_element_module_roundtrip() {
    let cases: &[_] = &[view_element::Module {
        name: "var3".to_string(),
        uid: 123,
        x: 2.0,
        y: 3.0,
        label_side: view_element::LabelSide::Top,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual =
            view_element::Module::from(project_io::view_element::Module::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::Alias> for view_element::Alias {
    fn from(v: project_io::view_element::Alias) -> Self {
        view_element::Alias {
            uid: v.uid,
            alias_of_uid: v.alias_of_uid,
            x: v.x,
            y: v.y,
            label_side: view_element::LabelSide::from(
                project_io::view_element::LabelSide::try_from(v.label_side).unwrap_or_default(),
            ),
        }
    }
}

impl From<view_element::Alias> for project_io::view_element::Alias {
    fn from(v: view_element::Alias) -> Self {
        project_io::view_element::Alias {
            uid: v.uid,
            alias_of_uid: v.alias_of_uid,
            x: v.x,
            y: v.y,
            label_side: project_io::view_element::LabelSide::from(v.label_side) as i32,
        }
    }
}

#[test]
fn test_view_element_alias_roundtrip() {
    let cases: &[_] = &[view_element::Alias {
        uid: 123,
        alias_of_uid: 124,
        x: 2.0,
        y: 3.0,
        label_side: view_element::LabelSide::Top,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual =
            view_element::Alias::from(project_io::view_element::Alias::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::view_element::Cloud> for view_element::Cloud {
    fn from(v: project_io::view_element::Cloud) -> Self {
        view_element::Cloud {
            uid: v.uid,
            flow_uid: v.flow_uid,
            x: v.x,
            y: v.y,
        }
    }
}

impl From<view_element::Cloud> for project_io::view_element::Cloud {
    fn from(v: view_element::Cloud) -> Self {
        project_io::view_element::Cloud {
            uid: v.uid,
            flow_uid: v.flow_uid,
            x: v.x,
            y: v.y,
        }
    }
}

#[test]
fn test_view_element_cloud_roundtrip() {
    let cases: &[_] = &[view_element::Cloud {
        uid: 123,
        flow_uid: 124,
        x: 2.0,
        y: 3.0,
    }];
    for expected in cases {
        let expected = expected.clone();
        let actual =
            view_element::Cloud::from(project_io::view_element::Cloud::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<project_io::ViewElement> for ViewElement {
    fn from(v: project_io::ViewElement) -> Self {
        match v.element.unwrap() {
            project_io::view_element::Element::Aux(v) => {
                ViewElement::Aux(view_element::Aux::from(v))
            }
            project_io::view_element::Element::Stock(v) => {
                ViewElement::Stock(view_element::Stock::from(v))
            }
            project_io::view_element::Element::Flow(v) => {
                ViewElement::Flow(view_element::Flow::from(v))
            }
            project_io::view_element::Element::Link(v) => {
                ViewElement::Link(view_element::Link::from(v))
            }
            project_io::view_element::Element::Module(v) => {
                ViewElement::Module(view_element::Module::from(v))
            }
            project_io::view_element::Element::Alias(v) => {
                ViewElement::Alias(view_element::Alias::from(v))
            }
            project_io::view_element::Element::Cloud(v) => {
                ViewElement::Cloud(view_element::Cloud::from(v))
            }
        }
    }
}

impl From<ViewElement> for project_io::ViewElement {
    fn from(v: ViewElement) -> Self {
        project_io::ViewElement {
            element: Some(match v {
                ViewElement::Aux(v) => {
                    project_io::view_element::Element::Aux(project_io::view_element::Aux::from(v))
                }
                ViewElement::Stock(v) => project_io::view_element::Element::Stock(
                    project_io::view_element::Stock::from(v),
                ),
                ViewElement::Flow(v) => {
                    project_io::view_element::Element::Flow(project_io::view_element::Flow::from(v))
                }
                ViewElement::Link(v) => {
                    project_io::view_element::Element::Link(project_io::view_element::Link::from(v))
                }
                ViewElement::Module(v) => project_io::view_element::Element::Module(
                    project_io::view_element::Module::from(v),
                ),
                ViewElement::Alias(v) => project_io::view_element::Element::Alias(
                    project_io::view_element::Alias::from(v),
                ),
                ViewElement::Cloud(v) => project_io::view_element::Element::Cloud(
                    project_io::view_element::Cloud::from(v),
                ),
            }),
        }
    }
}

#[test]
fn test_view_element_roundtrip() {
    let cases: &[_] = &[ViewElement::Cloud(view_element::Cloud {
        uid: 123,
        flow_uid: 124,
        x: 2.0,
        y: 3.0,
    })];
    for expected in cases {
        let expected = expected.clone();
        let actual = ViewElement::from(project_io::ViewElement::from(expected.clone()));
        assert_eq!(expected, actual);
    }
}

impl From<View> for project_io::View {
    fn from(view: View) -> Self {
        match view {
            View::StockFlow(view) => project_io::View {
                kind: project_io::view::ViewType::StockFlow as i32,
                elements: view
                    .elements
                    .into_iter()
                    .map(project_io::ViewElement::from)
                    .collect(),
                view_box: Some(view.view_box.into()),
                zoom: view.zoom,
            },
        }
    }
}

impl From<project_io::View> for View {
    fn from(view: project_io::View) -> Self {
        View::StockFlow(StockFlow {
            elements: view.elements.into_iter().map(ViewElement::from).collect(),
            view_box: view.view_box.map(Rect::from).unwrap_or_default(),
            zoom: if approx_eq!(f64, view.zoom, 0.0) {
                1.0
            } else {
                view.zoom
            },
        })
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
            views: model
                .views
                .into_iter()
                .map(project_io::View::from)
                .collect(),
        }
    }
}

impl From<project_io::Model> for Model {
    fn from(model: project_io::Model) -> Self {
        Model {
            name: model.name,
            variables: model.variables.into_iter().map(Variable::from).collect(),
            views: model.views.into_iter().map(View::from).collect(),
        }
    }
}

impl From<Dimension> for project_io::Dimension {
    fn from(dimension: Dimension) -> Self {
        match dimension {
            Dimension::Indexed(name, size) => project_io::Dimension {
                name,
                obsolete_elements: vec![],
                dimension: Some(project_io::dimension::Dimension::Size(
                    project_io::dimension::DimensionSize { size },
                )),
            },
            Dimension::Named(name, elements) => project_io::Dimension {
                name,
                obsolete_elements: vec![],
                dimension: Some(project_io::dimension::Dimension::Elements(
                    project_io::dimension::DimensionElements { elements },
                )),
            },
        }
    }
}

impl From<project_io::Dimension> for Dimension {
    fn from(dimension: project_io::Dimension) -> Self {
        if let Some(dim) = dimension.dimension {
            match dim {
                project_io::dimension::Dimension::Elements(elements) => {
                    Dimension::Named(dimension.name, elements.elements)
                }
                project_io::dimension::Dimension::Size(size) => {
                    Dimension::Indexed(dimension.name, size.size)
                }
            }
        } else {
            // originally we ignored dimensions with only indexes -- treat that as a fallback
            Dimension::Named(dimension.name, dimension.obsolete_elements)
        }
    }
}

impl From<project_io::source::Extension> for Extension {
    fn from(ext: project_io::source::Extension) -> Self {
        match ext {
            project_io::source::Extension::Unspecified => Extension::Unspecified,
            project_io::source::Extension::Xmile => Extension::Xmile,
            project_io::source::Extension::Vensim => Extension::Vensim,
        }
    }
}

impl From<Extension> for project_io::source::Extension {
    fn from(ext: Extension) -> Self {
        match ext {
            Extension::Unspecified => project_io::source::Extension::Unspecified,
            Extension::Xmile => project_io::source::Extension::Xmile,
            Extension::Vensim => project_io::source::Extension::Vensim,
        }
    }
}

impl From<Source> for project_io::Source {
    fn from(source: Source) -> Self {
        project_io::Source {
            extension: project_io::source::Extension::from(source.extension).into(),
            content: source.content,
        }
    }
}

impl From<project_io::Source> for Source {
    fn from(source: project_io::Source) -> Self {
        Source {
            extension: project_io::source::Extension::try_from(source.extension)
                .unwrap_or_default()
                .into(),
            content: source.content,
        }
    }
}

impl From<Unit> for project_io::Unit {
    fn from(unit: Unit) -> Self {
        project_io::Unit {
            name: unit.name,
            equation: unit.equation.unwrap_or_default(),
            disabled: unit.disabled,
            alias: unit.aliases,
        }
    }
}

impl From<project_io::Unit> for Unit {
    fn from(unit: project_io::Unit) -> Self {
        Unit {
            name: unit.name,
            equation: if unit.equation.is_empty() {
                None
            } else {
                Some(unit.equation)
            },
            disabled: unit.disabled,
            aliases: unit.alias,
        }
    }
}

impl From<Project> for project_io::Project {
    fn from(project: Project) -> Self {
        project_io::Project {
            name: project.name,
            sim_specs: Some(project_io::SimSpecs::from(project.sim_specs)),
            dimensions: project
                .dimensions
                .into_iter()
                .map(project_io::Dimension::from)
                .collect(),
            units: project
                .units
                .into_iter()
                .map(project_io::Unit::from)
                .collect(),
            models: project
                .models
                .into_iter()
                .map(project_io::Model::from)
                .collect(),
            source: project.source.map(|source| source.into()),
        }
    }
}

impl From<project_io::Project> for Project {
    fn from(project: project_io::Project) -> Self {
        Project {
            name: project.name,
            sim_specs: SimSpecs::from(project.sim_specs.unwrap()),
            dimensions: project
                .dimensions
                .into_iter()
                .map(Dimension::from)
                .collect(),
            units: project.units.into_iter().map(Unit::from).collect(),
            models: project.models.into_iter().map(Model::from).collect(),
            source: project.source.map(|source| source.into()),
        }
    }
}

pub fn serialize(project: &Project) -> project_io::Project {
    project_io::Project::from(project.clone())
}

pub fn deserialize(project: project_io::Project) -> Project {
    project.into()
}

pub fn deserialize_view(view: project_io::View) -> View {
    view.into()
}

pub fn deserialize_graphical_function(gf: project_io::GraphicalFunction) -> GraphicalFunction {
    gf.into()
}
