// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Property-based tests for JSON serialization using proptest.
//!
//! These tests verify that:
//! 1. JSON serialization roundtrips correctly (JSON -> Rust -> JSON -> Rust)
//! 2. Datamodel conversions roundtrip correctly (JSON types <-> datamodel types)
//! 3. Generated JSON validates against the schema

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use crate::datamodel;
use crate::json::*;

// Strategy helpers for generating valid identifiers and equations

fn ident_strategy() -> impl Strategy<Value = String> {
    "[a-z][a-z0-9_]{0,15}".prop_map(|s| s.to_string())
}

fn equation_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("0".to_string()),
        Just("1".to_string()),
        (1i32..1000).prop_map(|n| n.to_string()),
        (0.01f64..100.0).prop_map(|f| format!("{:.2}", f)),
        ident_strategy(),
        (ident_strategy(), ident_strategy()).prop_map(|(a, b)| format!("{} + {}", a, b)),
        (ident_strategy(), ident_strategy()).prop_map(|(a, b)| format!("{} * {}", a, b)),
    ]
}

fn finite_f64() -> impl Strategy<Value = f64> {
    // Generate floats that roundtrip correctly through JSON serialization.
    // We use integers and simple fractions to avoid precision loss.
    prop_oneof![
        Just(0.0),
        Just(1.0),
        Just(-1.0),
        (-1000i32..1000).prop_map(|x| x as f64),
        (-100i32..100).prop_map(|x| x as f64 / 10.0),
        (-100i32..100).prop_map(|x| x as f64 / 4.0),
    ]
}

fn documentation_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        "[A-Za-z0-9 ]{0,50}".prop_map(|s| s.to_string()),
    ]
}

fn units_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("people".to_string()),
        Just("widgets/year".to_string()),
        Just("1/time".to_string()),
    ]
}

fn label_side_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::new()),
        Just("top".to_string()),
        Just("bottom".to_string()),
        Just("left".to_string()),
        Just("right".to_string()),
    ]
}

fn kind_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("continuous".to_string()),
        Just("discrete".to_string()),
        Just("extrapolate".to_string()),
    ]
}

// Leaf type strategies

fn graphical_function_scale_strategy() -> impl Strategy<Value = GraphicalFunctionScale> {
    (finite_f64(), finite_f64()).prop_map(|(a, b)| GraphicalFunctionScale {
        min: a.min(b),
        max: a.max(b),
    })
}

fn graphical_function_strategy() -> BoxedStrategy<GraphicalFunction> {
    prop_oneof![
        // Points-based (explicit x,y pairs)
        (
            prop::collection::vec((finite_f64(), finite_f64()), 2..5),
            kind_strategy(),
            prop::option::of(graphical_function_scale_strategy()),
            prop::option::of(graphical_function_scale_strategy()),
        )
            .prop_map(|(pts, kind, x_scale, y_scale)| {
                let points: Vec<[f64; 2]> = pts.into_iter().map(|(x, y)| [x, y]).collect();
                GraphicalFunction {
                    points,
                    y_points: vec![],
                    kind,
                    x_scale,
                    y_scale,
                }
            }),
        // Y-points based (computed x values from scale)
        (
            prop::collection::vec(finite_f64(), 2..5),
            kind_strategy(),
            graphical_function_scale_strategy(),
            prop::option::of(graphical_function_scale_strategy()),
        )
            .prop_map(|(y_pts, kind, x_scale, y_scale)| {
                GraphicalFunction {
                    points: vec![],
                    y_points: y_pts,
                    kind,
                    x_scale: Some(x_scale),
                    y_scale,
                }
            }),
    ]
    .boxed()
}

fn element_equation_strategy() -> impl Strategy<Value = ElementEquation> {
    (
        ident_strategy(),
        equation_strategy(),
        prop_oneof![Just(String::new()), equation_strategy()],
        prop::option::of(graphical_function_strategy()),
    )
        .prop_map(
            |(subscript, equation, initial_equation, gf)| ElementEquation {
                subscript,
                equation,
                initial_equation,
                graphical_function: gf,
            },
        )
}

fn arrayed_equation_strategy() -> impl Strategy<Value = ArrayedEquation> {
    prop_oneof![
        // ApplyToAll variant: has equation, no elements
        (
            prop::collection::vec(ident_strategy(), 1..3),
            equation_strategy(),
            prop::option::of(equation_strategy()),
        )
            .prop_map(|(dims, eq, init_eq)| ArrayedEquation {
                dimensions: dims,
                equation: Some(eq),
                initial_equation: init_eq,
                elements: None,
            }),
        // Elements variant: has elements, no equation
        (
            prop::collection::vec(ident_strategy(), 1..3),
            prop::collection::vec(element_equation_strategy(), 1..4),
        )
            .prop_map(|(dims, elems)| ArrayedEquation {
                dimensions: dims,
                equation: None,
                initial_equation: None,
                elements: Some(elems),
            }),
    ]
}

// Variable type strategies with XOR invariant: equation/initial_equation OR arrayed_equation

fn stock_strategy() -> BoxedStrategy<Stock> {
    prop_oneof![
        // Scalar stock: has initial_equation, no arrayed_equation
        (
            any::<i32>(),
            ident_strategy(),
            equation_strategy(),
            units_strategy(),
            prop::collection::vec(ident_strategy(), 0..3),
            prop::collection::vec(ident_strategy(), 0..3),
            any::<bool>(),
            documentation_strategy(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(uid, name, eq, units, inflows, outflows, non_neg, doc, can_input, is_pub)| {
                    Stock {
                        uid,
                        name,
                        initial_equation: eq,
                        units,
                        inflows,
                        outflows,
                        non_negative: non_neg,
                        documentation: doc,
                        can_be_module_input: can_input,
                        is_public: is_pub,
                        arrayed_equation: None,
                    }
                }
            ),
        // Arrayed stock: has arrayed_equation, empty initial_equation
        (
            any::<i32>(),
            ident_strategy(),
            arrayed_equation_strategy(),
            units_strategy(),
            prop::collection::vec(ident_strategy(), 0..3),
            prop::collection::vec(ident_strategy(), 0..3),
            any::<bool>(),
            documentation_strategy(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(uid, name, arr_eq, units, inflows, outflows, non_neg, doc, can_input, is_pub)| {
                    Stock {
                        uid,
                        name,
                        initial_equation: String::new(),
                        units,
                        inflows,
                        outflows,
                        non_negative: non_neg,
                        documentation: doc,
                        can_be_module_input: can_input,
                        is_public: is_pub,
                        arrayed_equation: Some(arr_eq),
                    }
                }
            ),
    ]
    .boxed()
}

fn flow_strategy() -> BoxedStrategy<Flow> {
    prop_oneof![
        // Scalar flow: has equation, no arrayed_equation
        (
            any::<i32>(),
            ident_strategy(),
            equation_strategy(),
            units_strategy(),
            any::<bool>(),
            prop::option::of(graphical_function_strategy()),
            documentation_strategy(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(uid, name, eq, units, non_neg, gf, doc, can_input, is_pub)| {
                    Flow {
                        uid,
                        name,
                        equation: eq,
                        units,
                        non_negative: non_neg,
                        graphical_function: gf,
                        documentation: doc,
                        can_be_module_input: can_input,
                        is_public: is_pub,
                        arrayed_equation: None,
                    }
                }
            ),
        // Arrayed flow: has arrayed_equation, empty equation
        (
            any::<i32>(),
            ident_strategy(),
            arrayed_equation_strategy(),
            units_strategy(),
            any::<bool>(),
            prop::option::of(graphical_function_strategy()),
            documentation_strategy(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(uid, name, arr_eq, units, non_neg, gf, doc, can_input, is_pub)| {
                    Flow {
                        uid,
                        name,
                        equation: String::new(),
                        units,
                        non_negative: non_neg,
                        graphical_function: gf,
                        documentation: doc,
                        can_be_module_input: can_input,
                        is_public: is_pub,
                        arrayed_equation: Some(arr_eq),
                    }
                }
            ),
    ]
    .boxed()
}

fn auxiliary_strategy() -> BoxedStrategy<Auxiliary> {
    prop_oneof![
        // Scalar aux: has equation/initial_equation, no arrayed_equation
        (
            any::<i32>(),
            ident_strategy(),
            equation_strategy(),
            prop_oneof![Just(String::new()), equation_strategy()],
            units_strategy(),
            prop::option::of(graphical_function_strategy()),
            documentation_strategy(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(
                |(uid, name, eq, init_eq, units, gf, doc, can_input, is_pub)| {
                    Auxiliary {
                        uid,
                        name,
                        equation: eq,
                        initial_equation: init_eq,
                        units,
                        graphical_function: gf,
                        documentation: doc,
                        can_be_module_input: can_input,
                        is_public: is_pub,
                        arrayed_equation: None,
                    }
                }
            ),
        // Arrayed aux: has arrayed_equation, empty equation/initial_equation
        (
            any::<i32>(),
            ident_strategy(),
            arrayed_equation_strategy(),
            units_strategy(),
            prop::option::of(graphical_function_strategy()),
            documentation_strategy(),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(uid, name, arr_eq, units, gf, doc, can_input, is_pub)| {
                Auxiliary {
                    uid,
                    name,
                    equation: String::new(),
                    initial_equation: String::new(),
                    units,
                    graphical_function: gf,
                    documentation: doc,
                    can_be_module_input: can_input,
                    is_public: is_pub,
                    arrayed_equation: Some(arr_eq),
                }
            }),
    ]
    .boxed()
}

fn module_reference_strategy() -> impl Strategy<Value = ModuleReference> {
    (ident_strategy(), ident_strategy()).prop_map(|(src, dst)| ModuleReference { src, dst })
}

fn module_strategy() -> impl Strategy<Value = Module> {
    (
        any::<i32>(),
        ident_strategy(),
        ident_strategy(),
        units_strategy(),
        documentation_strategy(),
        prop::collection::vec(module_reference_strategy(), 0..3),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(uid, name, model_name, units, doc, refs, can_input, is_pub)| Module {
                uid,
                name,
                model_name,
                units,
                documentation: doc,
                references: refs,
                can_be_module_input: can_input,
                is_public: is_pub,
            },
        )
}

fn sim_specs_strategy() -> impl Strategy<Value = SimSpecs> {
    (
        finite_f64(),
        finite_f64(),
        prop_oneof![
            Just(String::new()),
            Just("1".to_string()),
            Just("0.25".to_string()),
            Just("1/4".to_string()),
            Just("1/8".to_string()),
        ],
        prop_oneof![Just(0.0f64), Just(0.5), Just(1.0), Just(2.0)],
        prop_oneof![
            Just(String::new()),
            Just("euler".to_string()),
            Just("rk4".to_string()),
        ],
        prop_oneof![
            Just(String::new()),
            Just("years".to_string()),
            Just("months".to_string()),
        ],
    )
        .prop_map(|(start, end, dt, save_step, method, time_units)| {
            let (start_time, end_time) = if start <= end {
                (start, end)
            } else {
                (end, start)
            };
            SimSpecs {
                start_time,
                end_time,
                dt,
                save_step,
                method,
                time_units,
            }
        })
}

// View element strategies

fn flow_point_strategy() -> impl Strategy<Value = FlowPoint> {
    (finite_f64(), finite_f64(), any::<i32>()).prop_map(|(x, y, attached)| FlowPoint {
        x,
        y,
        attached_to_uid: attached,
    })
}

fn link_point_strategy() -> impl Strategy<Value = LinkPoint> {
    (finite_f64(), finite_f64()).prop_map(|(x, y)| LinkPoint { x, y })
}

fn rect_strategy() -> impl Strategy<Value = Rect> {
    (finite_f64(), finite_f64(), finite_f64(), finite_f64()).prop_map(|(x, y, w, h)| Rect {
        x,
        y,
        width: w.abs(),
        height: h.abs(),
    })
}

fn stock_view_element_strategy() -> impl Strategy<Value = StockViewElement> {
    (
        any::<i32>(),
        ident_strategy(),
        finite_f64(),
        finite_f64(),
        label_side_strategy(),
    )
        .prop_map(|(uid, name, x, y, label_side)| StockViewElement {
            uid,
            name,
            x,
            y,
            label_side,
        })
}

fn flow_view_element_strategy() -> impl Strategy<Value = FlowViewElement> {
    (
        any::<i32>(),
        ident_strategy(),
        finite_f64(),
        finite_f64(),
        label_side_strategy(),
        prop::collection::vec(flow_point_strategy(), 2..5),
    )
        .prop_map(|(uid, name, x, y, label_side, points)| FlowViewElement {
            uid,
            name,
            x,
            y,
            label_side,
            points,
        })
}

fn auxiliary_view_element_strategy() -> impl Strategy<Value = AuxiliaryViewElement> {
    (
        any::<i32>(),
        ident_strategy(),
        finite_f64(),
        finite_f64(),
        label_side_strategy(),
    )
        .prop_map(|(uid, name, x, y, label_side)| AuxiliaryViewElement {
            uid,
            name,
            x,
            y,
            label_side,
        })
}

fn cloud_view_element_strategy() -> impl Strategy<Value = CloudViewElement> {
    (any::<i32>(), any::<i32>(), finite_f64(), finite_f64()).prop_map(|(uid, flow_uid, x, y)| {
        CloudViewElement {
            uid,
            flow_uid,
            x,
            y,
        }
    })
}

fn link_view_element_strategy() -> impl Strategy<Value = LinkViewElement> {
    (
        any::<i32>(),
        any::<i32>(),
        any::<i32>(),
        prop::option::of(finite_f64()),
        prop::collection::vec(link_point_strategy(), 0..4),
    )
        .prop_map(
            |(uid, from_uid, to_uid, arc, multi_points)| LinkViewElement {
                uid,
                from_uid,
                to_uid,
                arc,
                multi_points,
                polarity: None,
            },
        )
}

fn module_view_element_strategy() -> impl Strategy<Value = ModuleViewElement> {
    (
        any::<i32>(),
        ident_strategy(),
        finite_f64(),
        finite_f64(),
        label_side_strategy(),
    )
        .prop_map(|(uid, name, x, y, label_side)| ModuleViewElement {
            uid,
            name,
            x,
            y,
            label_side,
        })
}

fn alias_view_element_strategy() -> impl Strategy<Value = AliasViewElement> {
    (
        any::<i32>(),
        any::<i32>(),
        finite_f64(),
        finite_f64(),
        label_side_strategy(),
    )
        .prop_map(|(uid, alias_of_uid, x, y, label_side)| AliasViewElement {
            uid,
            alias_of_uid,
            x,
            y,
            label_side,
        })
}

fn view_element_strategy() -> BoxedStrategy<ViewElement> {
    prop_oneof![
        stock_view_element_strategy().prop_map(ViewElement::Stock),
        flow_view_element_strategy().prop_map(ViewElement::Flow),
        auxiliary_view_element_strategy().prop_map(ViewElement::Auxiliary),
        cloud_view_element_strategy().prop_map(ViewElement::Cloud),
        link_view_element_strategy().prop_map(ViewElement::Link),
        module_view_element_strategy().prop_map(ViewElement::Module),
        alias_view_element_strategy().prop_map(ViewElement::Alias),
    ]
    .boxed()
}

fn view_strategy() -> impl Strategy<Value = View> {
    (
        prop_oneof![Just(String::new()), Just("stock_flow".to_string())],
        prop::collection::vec(view_element_strategy(), 0..5),
        prop::option::of(rect_strategy()),
        prop_oneof![Just(0.0f64), Just(0.5), Just(1.0), Just(2.0)],
    )
        .prop_map(|(kind, elements, view_box, zoom)| View {
            kind,
            elements,
            view_box,
            zoom,
            use_lettered_polarity: false,
        })
}

fn loop_metadata_strategy() -> impl Strategy<Value = LoopMetadata> {
    (
        prop::collection::vec(any::<i32>(), 2..5),
        any::<bool>(),
        ident_strategy(),
        documentation_strategy(),
    )
        .prop_map(|(uids, deleted, name, description)| LoopMetadata {
            uids,
            deleted,
            name,
            description,
        })
}

fn dimension_strategy() -> impl Strategy<Value = Dimension> {
    prop_oneof![
        // Named dimension: has elements, size = 0
        (
            ident_strategy(),
            prop::collection::vec(ident_strategy(), 1..5),
            prop::option::of(ident_strategy()),
        )
            .prop_map(|(name, elements, maps_to)| Dimension {
                name,
                elements,
                size: 0,
                maps_to,
            }),
        // Indexed dimension: has size > 0, empty elements
        (
            ident_strategy(),
            1i32..20,
            prop::option::of(ident_strategy())
        )
            .prop_map(|(name, size, maps_to)| Dimension {
                name,
                elements: vec![],
                size,
                maps_to,
            }),
    ]
}

fn unit_strategy() -> impl Strategy<Value = Unit> {
    (
        ident_strategy(),
        prop_oneof![Just(String::new()), equation_strategy()],
        any::<bool>(),
        prop::collection::vec(ident_strategy(), 0..3),
    )
        .prop_map(|(name, equation, disabled, aliases)| Unit {
            name,
            equation,
            disabled,
            aliases,
        })
}

fn model_strategy() -> BoxedStrategy<Model> {
    (
        ident_strategy(),
        prop::collection::vec(stock_strategy(), 0..2),
        prop::collection::vec(flow_strategy(), 0..2),
        prop::collection::vec(auxiliary_strategy(), 0..2),
        prop::collection::vec(module_strategy(), 0..2),
        // Note: model-level sim_specs is intentionally None because the protobuf schema
        // only supports project-level sim_specs, not model-level.
        prop::collection::vec(view_strategy(), 0..1),
        prop::collection::vec(loop_metadata_strategy(), 0..2),
    )
        .prop_map(
            |(name, stocks, flows, auxiliaries, modules, views, loop_metadata)| Model {
                name,
                stocks,
                flows,
                auxiliaries,
                modules,
                sim_specs: None,
                views,
                loop_metadata,
                groups: vec![],
            },
        )
        .boxed()
}

fn project_strategy() -> BoxedStrategy<Project> {
    (
        ident_strategy(),
        sim_specs_strategy(),
        prop::collection::vec(model_strategy(), 1..2),
        prop::collection::vec(dimension_strategy(), 0..2),
        prop::collection::vec(unit_strategy(), 0..2),
    )
        .prop_map(|(name, sim_specs, models, dimensions, units)| Project {
            name,
            sim_specs,
            models,
            dimensions,
            units,
            source: Default::default(),
        })
        .boxed()
}

// Property tests

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // JSON serialization roundtrip tests

    #[test]
    fn json_roundtrip_graphical_function_scale(scale in graphical_function_scale_strategy()) {
        let json = serde_json::to_string(&scale).unwrap();
        let parsed: GraphicalFunctionScale = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(scale, parsed);
    }

    #[test]
    fn json_roundtrip_graphical_function(gf in graphical_function_strategy()) {
        let json = serde_json::to_string(&gf).unwrap();
        let parsed: GraphicalFunction = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(gf, parsed);
    }

    #[test]
    fn json_roundtrip_stock(stock in stock_strategy()) {
        let json = serde_json::to_string(&stock).unwrap();
        let parsed: Stock = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(stock, parsed);
    }

    #[test]
    fn json_roundtrip_flow(flow in flow_strategy()) {
        let json = serde_json::to_string(&flow).unwrap();
        let parsed: Flow = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(flow, parsed);
    }

    #[test]
    fn json_roundtrip_auxiliary(aux in auxiliary_strategy()) {
        let json = serde_json::to_string(&aux).unwrap();
        let parsed: Auxiliary = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(aux, parsed);
    }

    #[test]
    fn json_roundtrip_module(module in module_strategy()) {
        let json = serde_json::to_string(&module).unwrap();
        let parsed: Module = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(module, parsed);
    }

    #[test]
    fn json_roundtrip_sim_specs(ss in sim_specs_strategy()) {
        let json = serde_json::to_string(&ss).unwrap();
        let parsed: SimSpecs = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(ss, parsed);
    }

    #[test]
    fn json_roundtrip_view_element(ve in view_element_strategy()) {
        let json = serde_json::to_string(&ve).unwrap();
        let parsed: ViewElement = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(ve, parsed);
    }

    #[test]
    fn json_roundtrip_view(view in view_strategy()) {
        let json = serde_json::to_string(&view).unwrap();
        let parsed: View = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(view, parsed);
    }

    #[test]
    fn json_roundtrip_dimension(dim in dimension_strategy()) {
        let json = serde_json::to_string(&dim).unwrap();
        let parsed: Dimension = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(dim, parsed);
    }

    #[test]
    fn json_roundtrip_unit(unit in unit_strategy()) {
        let json = serde_json::to_string(&unit).unwrap();
        let parsed: Unit = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(unit, parsed);
    }

    #[test]
    fn json_roundtrip_model(model in model_strategy()) {
        let json = serde_json::to_string(&model).unwrap();
        let parsed: Model = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(model, parsed);
    }

    #[test]
    fn json_roundtrip_project(project in project_strategy()) {
        let json = serde_json::to_string(&project).unwrap();
        let parsed: Project = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(project, parsed);
    }

    // Datamodel conversion roundtrip tests
    // Note: These test that converting JSON -> datamodel -> JSON preserves semantics

    #[test]
    fn datamodel_roundtrip_stock(stock in stock_strategy()) {
        let dm: datamodel::Stock = stock.clone().into();
        let json_back: Stock = dm.into();
        // Convert both to datamodel for semantic comparison
        let dm_original: datamodel::Stock = stock.into();
        let dm_roundtrip: datamodel::Stock = json_back.into();
        prop_assert_eq!(dm_original, dm_roundtrip);
    }

    #[test]
    fn datamodel_roundtrip_flow(flow in flow_strategy()) {
        let dm: datamodel::Flow = flow.clone().into();
        let json_back: Flow = dm.into();
        let dm_original: datamodel::Flow = flow.into();
        let dm_roundtrip: datamodel::Flow = json_back.into();
        prop_assert_eq!(dm_original, dm_roundtrip);
    }

    #[test]
    fn datamodel_roundtrip_auxiliary(aux in auxiliary_strategy()) {
        let dm: datamodel::Aux = aux.clone().into();
        let json_back: Auxiliary = dm.into();
        let dm_original: datamodel::Aux = aux.into();
        let dm_roundtrip: datamodel::Aux = json_back.into();
        prop_assert_eq!(dm_original, dm_roundtrip);
    }

    #[test]
    fn datamodel_roundtrip_module(module in module_strategy()) {
        let dm: datamodel::Module = module.clone().into();
        let json_back: Module = dm.into();
        let dm_original: datamodel::Module = module.into();
        let dm_roundtrip: datamodel::Module = json_back.into();
        prop_assert_eq!(dm_original, dm_roundtrip);
    }

    #[test]
    fn datamodel_roundtrip_dimension(dim in dimension_strategy()) {
        let dm: datamodel::Dimension = dim.clone().into();
        let json_back: Dimension = dm.into();
        let dm_original: datamodel::Dimension = dim.into();
        let dm_roundtrip: datamodel::Dimension = json_back.into();
        prop_assert_eq!(dm_original, dm_roundtrip);
    }

    #[test]
    fn datamodel_roundtrip_project(project in project_strategy()) {
        let dm: datamodel::Project = project.clone().into();
        let json_back: Project = dm.into();
        let dm_original: datamodel::Project = project.into();
        let dm_roundtrip: datamodel::Project = json_back.into();
        prop_assert_eq!(dm_original, dm_roundtrip);
    }

}

#[cfg(feature = "schema")]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // Schema validation tests

    #[test]
    fn generated_json_validates_against_schema(project in project_strategy()) {
        let json_value = serde_json::to_value(&project).unwrap();
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();
        let validator = jsonschema::validator_for(&schema_value)
            .expect("schema should be valid");
        prop_assert!(
            validator.is_valid(&json_value),
            "Generated JSON failed schema validation"
        );
    }
}

/// Performs a full protobuf -> JSON roundtrip:
/// 1. Converts json::Project to datamodel::Project
/// 2. Converts datamodel to project_io::Project (protobuf)
/// 3. Encodes to protobuf bytes
/// 4. Decodes protobuf bytes back
/// 5. Converts through datamodel back to json::Project
/// 6. Serializes to JSON string
///
/// Returns (protobuf_bytes, json_string)
fn roundtrip_pb_json(json_project: &Project) -> (Vec<u8>, String) {
    use crate::project_io;
    use crate::prost::Message;
    use crate::serde as project_serde;

    // json -> datamodel -> protobuf
    let dm_project: datamodel::Project = json_project.clone().into();
    let pb_project: project_io::Project = project_serde::serialize(&dm_project);

    // Encode to protobuf bytes
    let mut pb_bytes = Vec::new();
    pb_project.encode(&mut pb_bytes).unwrap();

    // Decode protobuf bytes
    let pb_decoded = project_io::Project::decode(&pb_bytes[..]).unwrap();

    // protobuf -> datamodel -> json -> string
    let dm_decoded: datamodel::Project = project_serde::deserialize(pb_decoded);
    let json_decoded: Project = dm_decoded.into();
    let json_str = serde_json::to_string(&json_decoded).unwrap();

    (pb_bytes, json_str)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    // Protobuf -> JSON roundtrip tests
    // These verify that pb -> json -> pb -> json produces identical results

    #[test]
    fn protobuf_json_roundtrip_idempotent(project in project_strategy()) {
        // First roundtrip: json -> pb -> json
        let (pb_bytes1, json_str1) = roundtrip_pb_json(&project);

        // Parse the JSON string back to a Project
        let json_parsed1: Project = serde_json::from_str(&json_str1).unwrap();

        // Second roundtrip: json -> pb -> json
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed1);

        // The protobuf bytes should be identical after the first roundtrip
        prop_assert_eq!(
            pb_bytes1, pb_bytes2,
            "Protobuf bytes should be identical after roundtrip"
        );

        // The JSON strings should be identical after the first roundtrip
        prop_assert_eq!(
            json_str1, json_str2,
            "JSON strings should be identical after roundtrip"
        );
    }

    /// Test that protobuf roundtrips are idempotent after the first conversion.
    ///
    /// Note: JSON has separate arrays for stocks, flows, auxiliaries, while datamodel
    /// has a single `variables` Vec. The order can change during the first roundtrip,
    /// but subsequent roundtrips should be stable. This is what matters for the
    /// migration use case.
    #[test]
    fn protobuf_roundtrip_is_idempotent(project in project_strategy()) {
        use crate::prost::Message;
        use crate::project_io;
        use crate::serde as project_serde;

        // First roundtrip: json -> datamodel -> protobuf -> bytes
        let dm1: datamodel::Project = project.clone().into();
        let pb1: project_io::Project = project_serde::serialize(&dm1);
        let mut pb_bytes1 = Vec::new();
        pb1.encode(&mut pb_bytes1).unwrap();

        // Decode and do second roundtrip
        let pb1_decoded = project_io::Project::decode(&pb_bytes1[..]).unwrap();
        let dm2: datamodel::Project = project_serde::deserialize(pb1_decoded);
        let pb2: project_io::Project = project_serde::serialize(&dm2);
        let mut pb_bytes2 = Vec::new();
        pb2.encode(&mut pb_bytes2).unwrap();

        // Third roundtrip
        let pb2_decoded = project_io::Project::decode(&pb_bytes2[..]).unwrap();
        let dm3: datamodel::Project = project_serde::deserialize(pb2_decoded);
        let pb3: project_io::Project = project_serde::serialize(&dm3);
        let mut pb_bytes3 = Vec::new();
        pb3.encode(&mut pb_bytes3).unwrap();

        // After first roundtrip, datamodel and protobuf bytes should be stable
        prop_assert_eq!(&dm2, &dm3);
        prop_assert_eq!(pb_bytes2, pb_bytes3);
    }
}

#[cfg(test)]
mod protobuf_roundtrip_tests {
    use super::*;
    use crate::project_io;
    use crate::prost::Message;
    use crate::serde as project_serde;
    use std::fs;
    use std::path::Path;

    /// Tests roundtrip with a real protobin file from the test suite.
    ///
    /// Note: Due to floating point precision differences when serializing through JSON
    /// (JSON uses decimal representation), we verify idempotence after the first roundtrip
    /// rather than exact equality with the original. This is the key property for the
    /// migration use case: once converted to JSON, subsequent roundtrips should be stable.
    fn test_protobin_roundtrip(filename: &str) {
        let proto_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test")
            .join(filename);

        let proto_bytes = fs::read(&proto_path)
            .unwrap_or_else(|_| panic!("Failed to read {}", proto_path.display()));

        // Decode the original protobuf
        let pb_original = project_io::Project::decode(&proto_bytes[..])
            .unwrap_or_else(|_| panic!("Failed to decode {}", filename));

        // Convert to datamodel
        let dm_original: datamodel::Project = project_serde::deserialize(pb_original.clone());

        // First roundtrip: pb -> json string
        let json_project1: Project = dm_original.into();
        let json_str1 = serde_json::to_string_pretty(&json_project1).unwrap();

        // Parse JSON and do second roundtrip
        let json_parsed1: Project = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed1);

        // Parse JSON again and do third roundtrip
        let json_parsed2: Project = serde_json::from_str(&json_str2).unwrap();
        let (pb_bytes3, json_str3) = roundtrip_pb_json(&json_parsed2);

        // After the first roundtrip, JSON should be completely stable
        assert_eq!(
            json_str2, json_str3,
            "JSON should be identical after second roundtrip for {}",
            filename
        );

        // Protobuf bytes should also be stable after first roundtrip
        assert_eq!(
            pb_bytes2, pb_bytes3,
            "Protobuf bytes should be identical after second roundtrip for {}",
            filename
        );
    }

    #[test]
    fn test_fishbanks_roundtrip() {
        test_protobin_roundtrip("fishbanks.protobin");
    }

    #[test]
    fn test_logistic_growth_roundtrip() {
        test_protobin_roundtrip("logistic-growth.protobin");
    }

    /// Test that we can parse JSON, convert to protobuf, and back, getting the same JSON
    #[test]
    fn test_json_to_protobuf_to_json_idempotent() {
        // Create a sample JSON project
        let json_project = Project {
            name: "test_project".to_string(),
            sim_specs: SimSpecs {
                start_time: 0.0,
                end_time: 100.0,
                dt: "0.25".to_string(),
                save_step: 1.0,
                method: "rk4".to_string(),
                time_units: "years".to_string(),
            },
            models: vec![Model {
                name: "main".to_string(),
                stocks: vec![Stock {
                    uid: 1,
                    name: "population".to_string(),
                    initial_equation: "100".to_string(),
                    units: "people".to_string(),
                    inflows: vec!["births".to_string()],
                    outflows: vec!["deaths".to_string()],
                    non_negative: true,
                    documentation: "Total population".to_string(),
                    can_be_module_input: false,
                    is_public: true,
                    arrayed_equation: None,
                }],
                flows: vec![
                    Flow {
                        uid: 2,
                        name: "births".to_string(),
                        equation: "population * birth_rate".to_string(),
                        units: "people/year".to_string(),
                        non_negative: true,
                        graphical_function: None,
                        documentation: String::new(),
                        can_be_module_input: false,
                        is_public: false,
                        arrayed_equation: None,
                    },
                    Flow {
                        uid: 3,
                        name: "deaths".to_string(),
                        equation: "population * death_rate".to_string(),
                        units: "people/year".to_string(),
                        non_negative: true,
                        graphical_function: None,
                        documentation: String::new(),
                        can_be_module_input: false,
                        is_public: false,
                        arrayed_equation: None,
                    },
                ],
                auxiliaries: vec![
                    Auxiliary {
                        uid: 4,
                        name: "birth_rate".to_string(),
                        equation: "0.03".to_string(),
                        initial_equation: String::new(),
                        units: "1/year".to_string(),
                        graphical_function: None,
                        documentation: String::new(),
                        can_be_module_input: true,
                        is_public: false,
                        arrayed_equation: None,
                    },
                    Auxiliary {
                        uid: 5,
                        name: "death_rate".to_string(),
                        equation: "0.01".to_string(),
                        initial_equation: String::new(),
                        units: "1/year".to_string(),
                        graphical_function: None,
                        documentation: String::new(),
                        can_be_module_input: true,
                        is_public: false,
                        arrayed_equation: None,
                    },
                ],
                modules: vec![],
                sim_specs: None,
                views: vec![View {
                    kind: "stock_flow".to_string(),
                    elements: vec![
                        ViewElement::Stock(StockViewElement {
                            uid: 1,
                            name: "population".to_string(),
                            x: 200.0,
                            y: 200.0,
                            label_side: "top".to_string(),
                        }),
                        ViewElement::Flow(FlowViewElement {
                            uid: 2,
                            name: "births".to_string(),
                            x: 100.0,
                            y: 200.0,
                            label_side: "bottom".to_string(),
                            points: vec![
                                FlowPoint {
                                    x: 50.0,
                                    y: 200.0,
                                    attached_to_uid: 0,
                                },
                                FlowPoint {
                                    x: 150.0,
                                    y: 200.0,
                                    attached_to_uid: 1,
                                },
                            ],
                        }),
                    ],
                    view_box: Some(Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 800.0,
                        height: 600.0,
                    }),
                    zoom: 1.0,
                    use_lettered_polarity: false,
                }],
                loop_metadata: vec![LoopMetadata {
                    uids: vec![1, 2, 4, 1],
                    deleted: false,
                    name: "Growth Loop".to_string(),
                    description: "Population growth feedback loop".to_string(),
                }],
                groups: vec![],
            }],
            dimensions: vec![Dimension {
                name: "regions".to_string(),
                elements: vec!["north".to_string(), "south".to_string()],
                size: 0,
                maps_to: None,
            }],
            units: vec![Unit {
                name: "people".to_string(),
                equation: String::new(),
                disabled: false,
                aliases: vec!["persons".to_string()],
            }],
            source: Default::default(),
        };

        // First roundtrip
        let (pb_bytes1, json_str1) = roundtrip_pb_json(&json_project);

        // Parse and second roundtrip
        let json_parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed);

        // Verify idempotence
        assert_eq!(pb_bytes1, pb_bytes2, "Protobuf bytes should be identical");
        assert_eq!(json_str1, json_str2, "JSON strings should be identical");
    }

    /// Test arrayed variables roundtrip correctly through protobuf and JSON
    #[test]
    fn test_arrayed_variables_roundtrip() {
        let json_project = Project {
            name: "arrayed_test".to_string(),
            sim_specs: SimSpecs {
                start_time: 0.0,
                end_time: 10.0,
                dt: "1".to_string(),
                save_step: 0.0,
                method: String::new(),
                time_units: String::new(),
            },
            models: vec![Model {
                name: "main".to_string(),
                stocks: vec![Stock {
                    uid: 1,
                    name: "inventory".to_string(),
                    initial_equation: String::new(),
                    units: String::new(),
                    inflows: vec![],
                    outflows: vec![],
                    non_negative: false,
                    documentation: String::new(),
                    can_be_module_input: false,
                    is_public: false,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["warehouses".to_string()],
                        equation: Some("100".to_string()),
                        initial_equation: None,
                        elements: None,
                    }),
                }],
                flows: vec![],
                auxiliaries: vec![Auxiliary {
                    uid: 2,
                    name: "demand".to_string(),
                    equation: String::new(),
                    initial_equation: String::new(),
                    units: String::new(),
                    graphical_function: None,
                    documentation: String::new(),
                    can_be_module_input: false,
                    is_public: false,
                    arrayed_equation: Some(ArrayedEquation {
                        dimensions: vec!["regions".to_string()],
                        equation: None,
                        initial_equation: None,
                        elements: Some(vec![
                            ElementEquation {
                                subscript: "north".to_string(),
                                equation: "50".to_string(),
                                initial_equation: "10".to_string(),
                                graphical_function: None,
                            },
                            ElementEquation {
                                subscript: "south".to_string(),
                                equation: "75".to_string(),
                                initial_equation: String::new(),
                                graphical_function: None,
                            },
                        ]),
                    }),
                }],
                modules: vec![],
                sim_specs: None,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![
                Dimension {
                    name: "warehouses".to_string(),
                    elements: vec![
                        "east".to_string(),
                        "west".to_string(),
                        "central".to_string(),
                    ],
                    size: 0,
                    maps_to: None,
                },
                Dimension {
                    name: "regions".to_string(),
                    elements: vec!["north".to_string(), "south".to_string()],
                    size: 0,
                    maps_to: None,
                },
            ],
            units: vec![],
            source: Default::default(),
        };

        let (pb_bytes1, json_str1) = roundtrip_pb_json(&json_project);
        let json_parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed);

        assert_eq!(pb_bytes1, pb_bytes2, "Protobuf bytes should be identical");
        assert_eq!(json_str1, json_str2, "JSON strings should be identical");

        // Verify the arrayed equations are preserved
        let parsed: Project = serde_json::from_str(&json_str1).unwrap();
        assert!(parsed.models[0].stocks[0].arrayed_equation.is_some());
        assert!(parsed.models[0].auxiliaries[0].arrayed_equation.is_some());

        let arr_eq = parsed.models[0].auxiliaries[0]
            .arrayed_equation
            .as_ref()
            .unwrap();
        assert!(arr_eq.elements.is_some());
        assert_eq!(arr_eq.elements.as_ref().unwrap().len(), 2);
    }

    /// Test graphical functions roundtrip correctly
    #[test]
    fn test_graphical_function_roundtrip() {
        let json_project = Project {
            name: "gf_test".to_string(),
            sim_specs: SimSpecs {
                start_time: 0.0,
                end_time: 10.0,
                dt: "1".to_string(),
                save_step: 0.0,
                method: String::new(),
                time_units: String::new(),
            },
            models: vec![Model {
                name: "main".to_string(),
                stocks: vec![],
                flows: vec![],
                auxiliaries: vec![Auxiliary {
                    uid: 1,
                    name: "lookup".to_string(),
                    equation: "lookup(input)".to_string(),
                    initial_equation: String::new(),
                    units: String::new(),
                    graphical_function: Some(GraphicalFunction {
                        points: vec![[0.0, 0.0], [0.5, 0.25], [1.0, 1.0]],
                        y_points: vec![],
                        kind: "continuous".to_string(),
                        x_scale: Some(GraphicalFunctionScale { min: 0.0, max: 1.0 }),
                        y_scale: Some(GraphicalFunctionScale { min: 0.0, max: 1.0 }),
                    }),
                    documentation: String::new(),
                    can_be_module_input: false,
                    is_public: false,
                    arrayed_equation: None,
                }],
                modules: vec![],
                sim_specs: None,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![],
            units: vec![],
            source: Default::default(),
        };

        let (pb_bytes1, json_str1) = roundtrip_pb_json(&json_project);
        let json_parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed);

        assert_eq!(pb_bytes1, pb_bytes2, "Protobuf bytes should be identical");
        assert_eq!(json_str1, json_str2, "JSON strings should be identical");

        // Verify the graphical function is preserved
        let parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let gf = parsed.models[0].auxiliaries[0]
            .graphical_function
            .as_ref()
            .unwrap();
        assert_eq!(gf.points.len(), 3);
        assert_eq!(gf.kind, "continuous");
    }

    /// Test dimension with maps_to roundtrips correctly
    #[test]
    fn test_dimension_maps_to_roundtrip() {
        let json_project = Project {
            name: "maps_to_test".to_string(),
            sim_specs: SimSpecs {
                start_time: 0.0,
                end_time: 10.0,
                dt: "1".to_string(),
                save_step: 0.0,
                method: String::new(),
                time_units: String::new(),
            },
            models: vec![Model {
                name: "main".to_string(),
                stocks: vec![],
                flows: vec![],
                auxiliaries: vec![],
                modules: vec![],
                sim_specs: None,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![
                Dimension {
                    name: "DimB".to_string(),
                    elements: vec!["B1".to_string(), "B2".to_string(), "B3".to_string()],
                    size: 0,
                    maps_to: None,
                },
                Dimension {
                    name: "DimA".to_string(),
                    elements: vec!["A1".to_string(), "A2".to_string(), "A3".to_string()],
                    size: 0,
                    maps_to: Some("DimB".to_string()),
                },
            ],
            units: vec![],
            source: Default::default(),
        };

        let (pb_bytes1, json_str1) = roundtrip_pb_json(&json_project);
        let json_parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed);

        assert_eq!(pb_bytes1, pb_bytes2, "Protobuf bytes should be identical");
        assert_eq!(json_str1, json_str2, "JSON strings should be identical");

        // Verify maps_to is preserved
        let parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let dim_a = parsed.dimensions.iter().find(|d| d.name == "DimA").unwrap();
        assert_eq!(dim_a.maps_to, Some("DimB".to_string()));
    }

    /// Test indexed dimension (size instead of elements) roundtrips correctly
    #[test]
    fn test_indexed_dimension_roundtrip() {
        let json_project = Project {
            name: "indexed_dim_test".to_string(),
            sim_specs: SimSpecs {
                start_time: 0.0,
                end_time: 10.0,
                dt: "1".to_string(),
                save_step: 0.0,
                method: String::new(),
                time_units: String::new(),
            },
            models: vec![Model {
                name: "main".to_string(),
                stocks: vec![],
                flows: vec![],
                auxiliaries: vec![],
                modules: vec![],
                sim_specs: None,
                views: vec![],
                loop_metadata: vec![],
                groups: vec![],
            }],
            dimensions: vec![Dimension {
                name: "items".to_string(),
                elements: vec![],
                size: 10,
                maps_to: None,
            }],
            units: vec![],
            source: Default::default(),
        };

        let (pb_bytes1, json_str1) = roundtrip_pb_json(&json_project);
        let json_parsed: Project = serde_json::from_str(&json_str1).unwrap();
        let (pb_bytes2, json_str2) = roundtrip_pb_json(&json_parsed);

        assert_eq!(pb_bytes1, pb_bytes2, "Protobuf bytes should be identical");
        assert_eq!(json_str1, json_str2, "JSON strings should be identical");

        // Verify the indexed dimension is preserved
        let parsed: Project = serde_json::from_str(&json_str1).unwrap();
        assert_eq!(parsed.dimensions[0].size, 10);
        assert!(parsed.dimensions[0].elements.is_empty());
    }
}

#[cfg(all(test, feature = "schema"))]
mod schema_tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn generate_and_write_schema() {
        let schema_json = generate_schema_json();
        let schema_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("doc/simlin-project.schema.json");

        fs::write(&schema_path, &schema_json).expect("failed to write schema file");

        // Verify it's valid JSON and can be used as a schema
        let parsed: serde_json::Value = serde_json::from_str(&schema_json).unwrap();
        assert!(parsed.is_object());
        assert!(parsed.get("$schema").is_some());
    }

    #[test]
    fn schema_is_valid_json_schema() {
        let schema = generate_schema();
        let schema_value = serde_json::to_value(&schema).unwrap();

        // Verify the schema can be compiled
        let result = jsonschema::validator_for(&schema_value);
        assert!(result.is_ok(), "Schema should be valid: {:?}", result.err());
    }
}
