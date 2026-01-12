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
        prop::option::of(sim_specs_strategy()),
        prop::collection::vec(view_strategy(), 0..1),
        prop::collection::vec(loop_metadata_strategy(), 0..2),
    )
        .prop_map(
            |(name, stocks, flows, auxiliaries, modules, sim_specs, views, loop_metadata)| Model {
                name,
                stocks,
                flows,
                auxiliaries,
                modules,
                sim_specs,
                views,
                loop_metadata,
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

#[cfg(test)]
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
