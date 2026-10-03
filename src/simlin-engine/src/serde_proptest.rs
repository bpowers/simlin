// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Protobuf keeps a project: `deserialize(serialize(p))` is `p` less what
//! [`normalize`] lists, for generated `datamodel::Project`s and for what the
//! importers produce.
//!
//! The property is about the FIRST trip. A second trip equal to the third
//! (`json_proptest::protobuf_roundtrip_is_idempotent`) holds for a writer that
//! drops a field, because the field is already gone after one trip.
//!
//! What protobuf does not keep is stated once, in [`normalize`]: the property
//! is `round_trip(p) == normalize(p)`, so every difference between a project
//! and its stored form is a line of that function, and a field the writer
//! starts dropping fails here.
//!
//! The generators build every datamodel struct with a literal that names each
//! field, so a field added to the datamodel does not compile here until it is
//! generated.

use std::collections::HashMap;

use buffa::Message;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use crate::common::{CanonicalElementName, canonicalize};
use crate::datamodel::view_element::{
    self, LabelSide, LinkPolarity, LinkShape, LinkSketchCompat, SketchSegmentCompat,
    StockFlowSketchCompat, ViewElementCompat,
};
use crate::datamodel::*;
use crate::project_io;

fn round_trip(project: &Project) -> Project {
    let bytes = crate::serde::serialize(project).unwrap().encode_to_vec();
    crate::serde::deserialize(project_io::Project::decode_from_slice(&bytes).unwrap()).unwrap()
}

/// Whether two projects are the same to the bit, by their `Debug` forms, which
/// print a NaN as itself and a negative zero with its sign. The datamodel's
/// `==` is not asked: whether it holds a NaN equal to itself, or the two zeros
/// apart, is its own decision, and a stored project has to keep both whatever
/// that decision is.
fn same(a: &Project, b: &Project) -> bool {
    format!("{a:?}") == format!("{b:?}")
}

/// A project as protobuf stores it: everything `project` holds, less the
/// distinctions the format does not draw.
///
/// - A model's variables are stored in canonical-ident order.
/// - A string or number field with no presence bit reads its proto3 zero as
///   "none": an empty `units`, `time_units`, unit `equation`, group `doc` or
///   `parent`; a `uid` of 0; an empty `x_points` list; a flow or link point
///   attached to a uid that is not positive.
/// - A zoom of 0 (or within a rounding of it, `float::approx_eq`), the proto3
///   default of a view written before zoom was stored, reads as 1.
/// - A per-element subscript reads as the element key's canonical spelling
///   (`CanonicalElementName::from_subscript`, the owner of that spelling).
/// - ISEE's AI information -- the project's record and each variable's
///   `ai_state` -- is not stored: keeping ISEE's provenance record is outside
///   the engine's scope, and the schema has no field for it.
fn normalize(mut project: Project) -> Project {
    fn non_empty(value: Option<String>) -> Option<String> {
        value.filter(|s| !s.is_empty())
    }
    fn gf(gf: &mut Option<GraphicalFunction>) {
        if let Some(gf) = gf
            && gf.x_points.as_ref().is_some_and(Vec::is_empty)
        {
            gf.x_points = None;
        }
    }
    fn equation(equation: &mut Equation) {
        if let Equation::Arrayed(_, elements, _, _) = equation {
            for (subscript, _, _, element_gf) in elements {
                *subscript = CanonicalElementName::from_subscript(subscript)
                    .as_str()
                    .to_string();
                gf(element_gf);
            }
        }
    }
    fn specs(specs: &mut SimSpecs) {
        specs.time_units = non_empty(specs.time_units.take());
    }
    fn points(points: &mut [view_element::FlowPoint]) {
        for point in points {
            point.attached_to_uid = point.attached_to_uid.filter(|uid| *uid > 0);
        }
    }

    specs(&mut project.sim_specs);
    project.ai_information = None;
    for unit in &mut project.units {
        unit.equation = non_empty(unit.equation.take());
    }
    for model in &mut project.models {
        if let Some(model_specs) = &mut model.sim_specs {
            specs(model_specs);
        }
        for group in &mut model.groups {
            group.doc = non_empty(group.doc.take());
            group.parent = non_empty(group.parent.take());
        }
        model.variables.rewrite(|variables| {
            for variable in variables.iter_mut() {
                let (units, uid, ai_state) = match variable {
                    Variable::Stock(v) => {
                        equation(&mut v.equation);
                        (&mut v.units, &mut v.uid, &mut v.ai_state)
                    }
                    Variable::Flow(v) => {
                        equation(&mut v.equation);
                        gf(&mut v.gf);
                        (&mut v.units, &mut v.uid, &mut v.ai_state)
                    }
                    Variable::Aux(v) => {
                        equation(&mut v.equation);
                        gf(&mut v.gf);
                        (&mut v.units, &mut v.uid, &mut v.ai_state)
                    }
                    Variable::Module(v) => (&mut v.units, &mut v.uid, &mut v.ai_state),
                };
                *units = non_empty(units.take());
                *uid = uid.filter(|uid| *uid != 0);
                *ai_state = None;
            }
            variables.sort_by_cached_key(|v| canonicalize(v.get_ident()).into_owned());
        });
        for View::StockFlow(view) in &mut model.views {
            if crate::float::approx_eq(view.zoom, 0.0) {
                view.zoom = 1.0;
            }
            view.elements.rewrite(|elements| {
                for element in elements.iter_mut() {
                    match element {
                        ViewElement::Flow(flow) => points(&mut flow.points),
                        ViewElement::Link(link) => {
                            if let LinkShape::MultiPoint(link_points) = &mut link.shape {
                                points(link_points);
                            }
                        }
                        ViewElement::Aux(_)
                        | ViewElement::Stock(_)
                        | ViewElement::Module(_)
                        | ViewElement::Alias(_)
                        | ViewElement::Cloud(_)
                        | ViewElement::Group(_) => {}
                    }
                }
            });
        }
    }
    project
}

/// A strategy that picks among an enum's variants, each given as a pattern
/// and the strategy that generates it. The patterns are matched without a
/// wildcard, so a variant the enum gains does not compile here until it is
/// generated.
macro_rules! one_of_each {
    ($ty:ty { $($variant:pat => $strategy:expr),+ $(,)? }) => {{
        #[allow(dead_code)]
        fn every_variant_is_generated(value: &$ty) {
            match value {
                $($variant => {})+
            }
        }
        prop_oneof![$($strategy),+]
    }};
}

// -- Leaves

/// A name. It holds no period: the reader rewrites a period in an ident field
/// to U+2024 (`serde::migrate_stored_ident`, GH #690), which that rule's own
/// tests pin.
fn ident() -> BoxedStrategy<String> {
    "[A-Za-z][A-Za-z0-9_ ]{0,6}".boxed()
}

/// Free text: an equation, a documentation string, a units string. Empty
/// about a third of the time, since empty is where a field without a
/// presence bit loses its `Some`.
fn text() -> BoxedStrategy<String> {
    prop_oneof![1 => Just(String::new()), 2 => "[ -~]{1,10}"].boxed()
}

fn opt_text() -> BoxedStrategy<Option<String>> {
    prop::option::of(text()).boxed()
}

/// A number: a few halves, and the values a format is likeliest to lose --
/// both zeros, both infinities, a NaN, the smallest subnormal.
fn number() -> BoxedStrategy<f64> {
    prop_oneof![
        2 => Just(0.0),
        1 => Just(-0.0),
        8 => (-40i32..40).prop_map(|n| n as f64 / 2.0),
        1 => Just(f64::INFINITY),
        1 => Just(f64::NEG_INFINITY),
        1 => Just(f64::NAN),
        1 => Just(f64::from_bits(1)),
    ]
    .boxed()
}

/// A uid, zero and negatives included.
fn uid() -> BoxedStrategy<i32> {
    prop_oneof![2 => Just(0), 6 => -3i32..40].boxed()
}

fn names(max: usize) -> BoxedStrategy<Vec<String>> {
    prop::collection::vec(ident(), 0..=max).boxed()
}

// -- Simulation specs, dimensions, units

fn dt() -> BoxedStrategy<Dt> {
    one_of_each!(Dt {
        Dt::Dt(_) => number().prop_map(Dt::Dt),
        Dt::Reciprocal(_) => number().prop_map(Dt::Reciprocal),
    })
    .boxed()
}

fn sim_method() -> BoxedStrategy<SimMethod> {
    one_of_each!(SimMethod {
        SimMethod::Euler => Just(SimMethod::Euler),
        SimMethod::RungeKutta2 => Just(SimMethod::RungeKutta2),
        SimMethod::RungeKutta4 => Just(SimMethod::RungeKutta4),
    })
    .boxed()
}

fn sim_specs() -> BoxedStrategy<SimSpecs> {
    (
        number(),
        number(),
        dt(),
        prop::option::of(dt()),
        sim_method(),
        opt_text(),
    )
        .prop_map(
            |(start, stop, dt, save_step, sim_method, time_units)| SimSpecs {
                start,
                stop,
                dt,
                save_step,
                sim_method,
                time_units,
            },
        )
        .boxed()
}

fn dimension() -> BoxedStrategy<Dimension> {
    let elements = one_of_each!(DimensionElements {
        DimensionElements::Indexed(_) => (0u32..5).prop_map(DimensionElements::Indexed),
        DimensionElements::Named(_) => names(3).prop_map(DimensionElements::Named),
    });
    let mapping = (ident(), prop::collection::vec((ident(), ident()), 0..3)).prop_map(
        |(target, element_map)| DimensionMapping {
            target,
            element_map,
        },
    );
    (
        ident(),
        elements,
        prop::collection::vec(mapping, 0..3),
        prop::option::of(ident()),
    )
        .prop_map(|(name, elements, mappings, parent)| Dimension {
            name,
            elements,
            mappings,
            parent,
        })
        .boxed()
}

fn unit() -> BoxedStrategy<Unit> {
    (ident(), opt_text(), any::<bool>(), names(2))
        .prop_map(|(name, equation, disabled, aliases)| Unit {
            name,
            equation,
            disabled,
            aliases,
        })
        .boxed()
}

// -- Variables

fn graphical_function() -> BoxedStrategy<GraphicalFunction> {
    let kind = one_of_each!(GraphicalFunctionKind {
        GraphicalFunctionKind::Continuous => Just(GraphicalFunctionKind::Continuous),
        GraphicalFunctionKind::Extrapolate => Just(GraphicalFunctionKind::Extrapolate),
        GraphicalFunctionKind::Discrete => Just(GraphicalFunctionKind::Discrete),
    });
    let scale = || (number(), number()).prop_map(|(min, max)| GraphicalFunctionScale { min, max });
    (
        kind,
        prop::option::of(prop::collection::vec(number(), 0..4)),
        prop::collection::vec(number(), 0..4),
        scale(),
        scale(),
    )
        .prop_map(
            |(kind, x_points, y_points, x_scale, y_scale)| GraphicalFunction {
                kind,
                x_points,
                y_points,
                x_scale,
                y_scale,
            },
        )
        .boxed()
}

fn equation() -> BoxedStrategy<Equation> {
    // An element subscript as a file spells it: display case, with or
    // without space after a comma.
    let subscript = prop_oneof![
        ident(),
        (ident(), ident()).prop_map(|(a, b)| format!("{a}, {b}")),
        (ident(), ident()).prop_map(|(a, b)| format!("{a},{b}")),
    ];
    let element = (
        subscript,
        text(),
        opt_text(),
        prop::option::of(graphical_function()),
    );
    one_of_each!(Equation {
        Equation::Scalar(_) => text().prop_map(Equation::Scalar),
        Equation::ApplyToAll(_, _) =>
            (names(2), text()).prop_map(|(dims, eqn)| Equation::ApplyToAll(dims, eqn)),
        Equation::Arrayed(_, _, _, _) => (
            names(2),
            prop::collection::vec(element, 0..3),
            opt_text(),
            any::<bool>()
        )
            .prop_map(|(dims, elements, default, applies)| {
                Equation::Arrayed(dims, elements, default, applies)
            }),
    })
    .boxed()
}

fn compat() -> BoxedStrategy<Compat> {
    let data_source = (
        one_of_each!(DataSourceKind {
            DataSourceKind::Data => Just(DataSourceKind::Data),
            DataSourceKind::Constants => Just(DataSourceKind::Constants),
            DataSourceKind::Lookups => Just(DataSourceKind::Lookups),
            DataSourceKind::Subscript => Just(DataSourceKind::Subscript),
        }),
        text(),
        text(),
        text(),
        text(),
    )
        .prop_map(
            |(kind, file, tab_or_delimiter, row_or_col, cell)| DataSource {
                kind,
                file,
                tab_or_delimiter,
                row_or_col,
                cell,
            },
        );
    let conveyor = (
        (text(), opt_text(), opt_text(), opt_text(), opt_text()),
        prop::array::uniform5(any::<bool>()),
    )
        .prop_map(
            |(
                (transit_time, capacity, inflow_limit, sample, arrest),
                [
                    discrete,
                    batch_integrity,
                    one_at_a_time,
                    exponential_leak,
                    ignore_earlier_zone_losses,
                ],
            )| Conveyor {
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
            },
        );
    let leakage = (opt_text(), any::<bool>(), opt_text(), opt_text()).prop_map(
        |(fraction, integers, zone_start, zone_end)| Leakage {
            fraction,
            integers,
            zone_start,
            zone_end,
        },
    );
    let spreadflow = one_of_each!(SpreadFlow {
        SpreadFlow::Beginning => Just(SpreadFlow::Beginning),
        SpreadFlow::Even => Just(SpreadFlow::Even),
        SpreadFlow::Dest => Just(SpreadFlow::Dest),
        SpreadFlow::Dist(_) => text().prop_map(SpreadFlow::Dist),
        SpreadFlow::Source => Just(SpreadFlow::Source),
    });
    let visibility = one_of_each!(Visibility {
        Visibility::Private => Just(Visibility::Private),
        Visibility::Public => Just(Visibility::Public),
    });
    (
        (opt_text(), any::<bool>(), any::<bool>(), visibility),
        prop::option::of(data_source),
        prop::option::of(conveyor),
        prop::option::of(leakage),
        prop::option::of(spreadflow),
        prop::option::of(Just(Queue {})),
        any::<bool>(),
    )
        .prop_map(
            |(
                (active_initial, non_negative, can_be_module_input, visibility),
                data_source,
                conveyor,
                leakage,
                spreadflow,
                queue,
                overflow,
            )| Compat {
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
            },
        )
        .boxed()
}

fn ai_state() -> BoxedStrategy<Option<AiState>> {
    prop::option::of(one_of_each!(AiState {
        AiState::A => Just(AiState::A),
        AiState::B => Just(AiState::B),
        AiState::C => Just(AiState::C),
        AiState::D => Just(AiState::D),
        AiState::E => Just(AiState::E),
        AiState::F => Just(AiState::F),
        AiState::G => Just(AiState::G),
        AiState::H => Just(AiState::H),
    }))
    .boxed()
}

/// What every kind of variable holds: documentation, units, provenance, uid
/// and compat.
type Common = (String, Option<String>, Option<AiState>, Option<i32>, Compat);

fn common() -> BoxedStrategy<Common> {
    (
        text(),
        opt_text(),
        ai_state(),
        prop::option::of(uid()),
        compat(),
    )
        .boxed()
}

fn variable() -> BoxedStrategy<Variable> {
    let stock = (ident(), equation(), names(2), names(2), common()).prop_map(
        |(ident, equation, inflows, outflows, (documentation, units, ai_state, uid, compat))| {
            Variable::Stock(Stock {
                ident,
                equation,
                documentation,
                units,
                inflows,
                outflows,
                ai_state,
                uid,
                compat,
            })
        },
    );
    let flow = (
        ident(),
        equation(),
        prop::option::of(graphical_function()),
        common(),
    )
        .prop_map(
            |(ident, equation, gf, (documentation, units, ai_state, uid, compat))| {
                Variable::Flow(Flow {
                    ident,
                    equation,
                    documentation,
                    units,
                    gf,
                    ai_state,
                    uid,
                    compat,
                })
            },
        );
    let aux = (
        ident(),
        equation(),
        prop::option::of(graphical_function()),
        common(),
    )
        .prop_map(
            |(ident, equation, gf, (documentation, units, ai_state, uid, compat))| {
                Variable::Aux(Aux {
                    ident,
                    equation,
                    documentation,
                    units,
                    gf,
                    ai_state,
                    uid,
                    compat,
                })
            },
        );
    // A module reference's ends are not ident fields: a period in one is the
    // module separator and is stored as written.
    let reference =
        ("[a-z.]{1,6}", "[a-z.]{1,6}").prop_map(|(src, dst)| ModuleReference { src, dst });
    let module = (
        ident(),
        ident(),
        prop::collection::vec(reference, 0..3),
        common(),
    )
        .prop_map(
            |(ident, model_name, references, (documentation, units, ai_state, uid, compat))| {
                Variable::Module(Module {
                    ident,
                    model_name,
                    documentation,
                    units,
                    references,
                    ai_state,
                    uid,
                    compat,
                })
            },
        );
    one_of_each!(Variable {
        Variable::Stock(_) => stock,
        Variable::Flow(_) => flow,
        Variable::Aux(_) => aux,
        Variable::Module(_) => module,
    })
    .boxed()
}

// -- Views

fn label_side() -> BoxedStrategy<LabelSide> {
    one_of_each!(LabelSide {
        LabelSide::Top => Just(LabelSide::Top),
        LabelSide::Left => Just(LabelSide::Left),
        LabelSide::Center => Just(LabelSide::Center),
        LabelSide::Bottom => Just(LabelSide::Bottom),
        LabelSide::Right => Just(LabelSide::Right),
    })
    .boxed()
}

fn element_compat() -> BoxedStrategy<Option<ViewElementCompat>> {
    prop::option::of(
        (
            number(),
            number(),
            -2i32..40,
            0u32..300,
            opt_text(),
            opt_text(),
        )
            .prop_map(
                |(width, height, shape, bits, name_field, tail)| ViewElementCompat {
                    width,
                    height,
                    shape,
                    bits,
                    name_field,
                    tail,
                },
            ),
    )
    .boxed()
}

fn flow_point() -> BoxedStrategy<view_element::FlowPoint> {
    (number(), number(), prop::option::of(uid()))
        .prop_map(|(x, y, attached_to_uid)| view_element::FlowPoint {
            x,
            y,
            attached_to_uid,
        })
        .boxed()
}

fn view_element() -> BoxedStrategy<ViewElement> {
    let placed = || (uid(), number(), number(), label_side());
    let aux = (ident(), placed(), element_compat()).prop_map(
        |(name, (uid, x, y, label_side), compat)| {
            ViewElement::Aux(view_element::Aux {
                name,
                uid,
                x,
                y,
                label_side,
                compat,
            })
        },
    );
    let stock = (ident(), placed(), element_compat()).prop_map(
        |(name, (uid, x, y, label_side), compat)| {
            ViewElement::Stock(view_element::Stock {
                name,
                uid,
                x,
                y,
                label_side,
                compat,
            })
        },
    );
    let flow = (
        ident(),
        placed(),
        prop::collection::vec(flow_point(), 0..4),
        element_compat(),
        element_compat(),
    )
        .prop_map(
            |(name, (uid, x, y, label_side), points, compat, label_compat)| {
                ViewElement::Flow(view_element::Flow {
                    name,
                    uid,
                    x,
                    y,
                    label_side,
                    points,
                    compat,
                    label_compat,
                })
            },
        );
    let shape = one_of_each!(LinkShape {
        LinkShape::Straight => Just(LinkShape::Straight),
        LinkShape::Arc(_) => number().prop_map(LinkShape::Arc),
        LinkShape::MultiPoint(_) =>
            prop::collection::vec(flow_point(), 0..3).prop_map(LinkShape::MultiPoint),
    });
    let polarity = prop::option::of(one_of_each!(LinkPolarity {
        LinkPolarity::Positive => Just(LinkPolarity::Positive),
        LinkPolarity::Negative => Just(LinkPolarity::Negative),
    }));
    let link = (uid(), uid(), uid(), shape, polarity).prop_map(
        |(uid, from_uid, to_uid, shape, polarity)| {
            ViewElement::Link(view_element::Link {
                uid,
                from_uid,
                to_uid,
                shape,
                polarity,
            })
        },
    );
    let module = (ident(), placed()).prop_map(|(name, (uid, x, y, label_side))| {
        ViewElement::Module(view_element::Module {
            name,
            uid,
            x,
            y,
            label_side,
        })
    });
    let alias = (uid(), placed(), element_compat()).prop_map(
        |(alias_of_uid, (uid, x, y, label_side), compat)| {
            ViewElement::Alias(view_element::Alias {
                uid,
                alias_of_uid,
                x,
                y,
                label_side,
                compat,
            })
        },
    );
    let cloud = (uid(), uid(), number(), number(), element_compat()).prop_map(
        |(uid, flow_uid, x, y, compat)| {
            ViewElement::Cloud(view_element::Cloud {
                uid,
                flow_uid,
                x,
                y,
                compat,
            })
        },
    );
    let group = (
        uid(),
        text(),
        (number(), number(), number(), number()),
        any::<bool>(),
    )
        .prop_map(|(uid, name, (x, y, width, height), is_mdl_view_marker)| {
            ViewElement::Group(view_element::Group {
                uid,
                name,
                x,
                y,
                width,
                height,
                is_mdl_view_marker,
            })
        });
    one_of_each!(ViewElement {
        ViewElement::Aux(_) => aux,
        ViewElement::Stock(_) => stock,
        ViewElement::Flow(_) => flow,
        ViewElement::Link(_) => link,
        ViewElement::Module(_) => module,
        ViewElement::Alias(_) => alias,
        ViewElement::Cloud(_) => cloud,
        ViewElement::Group(_) => group,
    })
    .boxed()
}

fn sketch_compat() -> BoxedStrategy<Option<StockFlowSketchCompat>> {
    let segment = (number(), number())
        .prop_map(|(x_offset, y_offset)| SketchSegmentCompat { x_offset, y_offset });
    let link = (
        uid(),
        -2i32..3,
        -2i32..100,
        prop::option::of((-50i32..50, -50i32..50)),
    )
        .prop_map(|(uid, field4, field10, control_point)| LinkSketchCompat {
            uid,
            field4,
            field10,
            control_point,
        });
    prop::option::of(
        (
            prop::collection::vec(segment, 0..3),
            prop::collection::vec(link, 0..3),
        )
            .prop_map(|(segments, links)| StockFlowSketchCompat { segments, links }),
    )
    .boxed()
}

fn view() -> BoxedStrategy<View> {
    (
        opt_text(),
        prop::collection::vec(view_element(), 0..4),
        (number(), number(), number(), number()),
        number(),
        any::<bool>(),
        opt_text(),
        sketch_compat(),
    )
        .prop_map(
            |(
                name,
                elements,
                (x, y, width, height),
                zoom,
                use_lettered_polarity,
                font,
                sketch_compat,
            )| {
                View::StockFlow(StockFlow {
                    name,
                    elements: elements.into(),
                    view_box: Rect {
                        x,
                        y,
                        width,
                        height,
                    },
                    zoom,
                    use_lettered_polarity,
                    font,
                    sketch_compat,
                })
            },
        )
        .boxed()
}

// -- Models and projects

fn model() -> BoxedStrategy<Model> {
    let loop_metadata = (
        prop::collection::vec(uid(), 0..4),
        any::<bool>(),
        text(),
        text(),
    )
        .prop_map(|(uids, deleted, name, description)| LoopMetadata {
            uids,
            deleted,
            name,
            description,
        });
    let group = (text(), opt_text(), opt_text(), names(2), any::<bool>()).prop_map(
        |(name, doc, parent, members, run_enabled)| ModelGroup {
            name,
            doc,
            parent,
            members,
            run_enabled,
        },
    );
    let macro_spec = (names(2), ident(), names(2)).prop_map(
        |(parameters, primary_output, additional_outputs)| MacroSpec {
            parameters,
            primary_output,
            additional_outputs,
        },
    );
    (
        ident(),
        prop::option::of(sim_specs()),
        prop::collection::vec(variable(), 0..4),
        prop::collection::vec(view(), 0..2),
        prop::collection::vec(loop_metadata, 0..2),
        prop::collection::vec(group, 0..2),
        prop::option::of(macro_spec),
    )
        .prop_map(
            |(name, sim_specs, variables, views, loop_metadata, groups, macro_spec)| Model {
                name,
                sim_specs,
                variables: variables.into(),
                views,
                loop_metadata,
                groups,
                macro_spec,
            },
        )
        .boxed()
}

fn ai_information() -> BoxedStrategy<Option<AiInformation>> {
    let status = (
        text(),
        text(),
        text(),
        prop::collection::hash_map(ident(), text(), 0..3),
    )
        .prop_map(|(key_url, algorithm, signature, tags)| AiStatus {
            key_url,
            algorithm,
            signature,
            tags: tags.into_iter().collect::<HashMap<_, _>>(),
        });
    let testing = text().prop_map(|signed_message_body| AiTesting {
        signed_message_body,
    });
    prop::option::of((status, prop::option::of(testing), opt_text()).prop_map(
        |(status, testing, log)| AiInformation {
            status,
            testing,
            log,
        },
    ))
    .boxed()
}

fn project() -> BoxedStrategy<Project> {
    let source = (
        one_of_each!(Extension {
            Extension::Unspecified => Just(Extension::Unspecified),
            Extension::Xmile => Just(Extension::Xmile),
            Extension::Vensim => Just(Extension::Vensim),
        }),
        text(),
    )
        .prop_map(|(extension, content)| Source { extension, content });
    (
        text(),
        sim_specs(),
        prop::collection::vec(dimension(), 0..3),
        prop::collection::vec(unit(), 0..3),
        prop::collection::vec(model(), 0..3),
        prop::option::of(source),
        ai_information(),
    )
        .prop_map(
            |(name, sim_specs, dimensions, units, models, source, ai_information)| Project {
                name,
                sim_specs,
                dimensions,
                units,
                models,
                source,
                ai_information,
            },
        )
        .boxed()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn a_generated_project_survives_one_protobuf_trip(project in project()) {
        prop_assert!(same(&round_trip(&project), &normalize(project)));
    }

    /// What one trip gives is what every further trip gives, project and
    /// bytes alike: a stored project does not drift as it is saved again.
    #[test]
    fn a_stored_project_is_the_same_after_another_trip(project in project()) {
        let stored = round_trip(&project);
        let again = round_trip(&stored);
        let bytes = |p: &Project| crate::serde::serialize(p).unwrap().encode_to_vec();
        prop_assert!(same(&stored, &again));
        prop_assert!(bytes(&stored) == bytes(&again));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20_000))]

    #[test]
    #[ignore = "20,000 generated projects through protobuf; run under the gates profile"]
    fn many_generated_projects_survive_one_protobuf_trip(project in project()) {
        prop_assert!(same(&round_trip(&project), &normalize(project)));
    }
}

/// Every model file under `test/` that opens, as the importers produce it.
fn corpus_imports() -> Vec<(String, Project)> {
    let mut files = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../test"
    ))];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                ["mdl", "xmile", "stmx", "itmx"]
                    .iter()
                    .any(|x| e.eq_ignore_ascii_case(x))
            }) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
        .into_iter()
        .filter_map(|path| Some((path.display().to_string(), import(&path)?)))
        .collect()
}

fn import(path: &std::path::Path) -> Option<Project> {
    let bytes = std::fs::read(path).ok()?;
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mdl"))
    {
        crate::compat::open_vensim(std::str::from_utf8(&bytes).ok()?).ok()
    } else {
        crate::compat::open_xmile(&mut std::io::BufReader::new(bytes.as_slice())).ok()
    }
}

/// What the importers produce is what production stores, so the property is
/// held to it as well: a Vensim model with a multi-view sketch, whose sketch
/// metadata an MDL save writes back, and a Stella model built of modules.
#[test]
fn an_imported_project_survives_one_protobuf_trip() {
    let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../test"));
    for path in [
        "metasd/WRLD3-03/wrld3-03.mdl",
        "modules_hares_and_foxes/modules_hares_and_foxes.stmx",
    ] {
        let project = import(&root.join(path)).expect("the corpus model opens");
        let kept = round_trip(&project);
        assert!(
            same(&kept, &normalize(project.clone())),
            "{path}: {}",
            first_difference(&normalize(project), &kept)
        );
    }

    // The Vensim row is not vacuous: its views hold the sketch metadata.
    let vensim = round_trip(&import(&root.join("metasd/WRLD3-03/wrld3-03.mdl")).unwrap());
    assert!(vensim.models.iter().any(|model| {
        model
            .views
            .iter()
            .any(|View::StockFlow(view)| view.sketch_compat.is_some())
    }));
}

/// A project whose specs run to 10 by steps of 1 and whose model overrides
/// them with its own: to 4 by steps of 0.5 (XMILE 1.0 section 4: a model "can
/// OPTIONALLY include `<sim_specs>` ... to override specific settings").
fn a_model_with_its_own_specs() -> Project {
    let xmile = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
  <header><name>specs</name><vendor>x</vendor><product version="1">x</product></header>
  <sim_specs method="Euler" time_units="Months"><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
  <model>
    <sim_specs method="Euler" time_units="Months"><start>0</start><stop>4</stop><dt>0.5</dt></sim_specs>
    <variables>
      <stock name="s"><eqn>0</eqn><inflow>f</inflow></stock>
      <flow name="f"><eqn>1</eqn></flow>
    </variables>
  </model>
</xmile>"#;
    crate::compat::open_xmile(&mut std::io::BufReader::new(xmile.as_bytes()))
        .expect("the model opens")
}

/// The run of `project`'s root model: how many steps it saves, to what time,
/// by what step.
fn run_of(project: &Project) -> (usize, f64, f64) {
    use crate::db::{
        LtmOverlay, SimlinDb, compile_project_incremental, sync_from_datamodel_incremental,
    };
    let mut db = SimlinDb::default();
    let sync = sync_from_datamodel_incremental(&mut db, project, None);
    let compiled = compile_project_incremental(&db, sync.project, "main", LtmOverlay::Off)
        .expect("the model compiles");
    let mut vm = crate::Vm::new(compiled).expect("the model runs");
    vm.run_to_end().expect("the model runs");
    let results = vm.into_results();
    (results.step_count, results.specs.stop, results.specs.dt)
}

/// A model's own specs decide its run, so a stored project that kept only
/// the project's specs would run a different simulation.
#[test]
fn a_model_runs_under_its_own_specs_after_a_protobuf_trip() {
    let project = a_model_with_its_own_specs();
    assert_eq!(run_of(&project), (9, 4.0, 0.5), "the model's own specs");
    assert_eq!(run_of(&round_trip(&project)), (9, 4.0, 0.5));
}

/// Setting the simulation specs changes the run, whatever the stored project
/// holds: an editor shows and edits one set of specs, and the run a person
/// then sees has to be under them. A model can carry specs of its own, which
/// the run prefers (`db::assemble`); `SetSimSpecs` takes the root model's
/// away, so the project's are what it runs under.
#[test]
fn setting_the_sim_specs_of_a_stored_project_changes_its_run() {
    let mut stored = round_trip(&a_model_with_its_own_specs());
    let mut specs = stored.sim_specs.clone();
    specs.stop = 20.0;
    crate::apply_patch(
        &mut stored,
        crate::ProjectPatch {
            project_ops: vec![crate::ProjectOperation::SetSimSpecs(specs)],
            models: vec![],
        },
    )
    .expect("the specs are set");
    let (_, stop, _) = run_of(&stored);
    assert!(
        stop == 20.0,
        "the run follows the specs that were set: it stops at {stop}"
    );
}

#[test]
#[ignore = "every corpus model through protobuf; run under the gates profile"]
fn every_corpus_import_survives_one_protobuf_trip() {
    let imports = corpus_imports();
    assert!(imports.len() >= 400, "only {} files open", imports.len());
    let lost: Vec<String> = imports
        .iter()
        .filter_map(|(path, project)| {
            let (kept, expected) = (round_trip(project), normalize(project.clone()));
            (!same(&kept, &expected))
                .then(|| format!("{path}: {}", first_difference(&expected, &kept)))
        })
        .collect();
    // The whole list, so a file that starts surviving fails here too and its
    // row is deleted.
    assert!(
        lost.len() == PERIOD_NAMED.len()
            && lost
                .iter()
                .zip(PERIOD_NAMED)
                .all(|(lost, file)| lost.contains(file) && lost.contains('\u{2024}')),
        "not kept through protobuf: {lost:#?}"
    );

    // What those files lose is a display spelling: every variable keeps its
    // canonical name, so the stored model resolves the same references.
    let canonical_names = |project: &Project| -> Vec<Vec<String>> {
        project
            .models
            .iter()
            .map(|model| {
                model
                    .variables
                    .iter()
                    .map(|v| canonicalize(v.get_ident()).into_owned())
                    .collect()
            })
            .collect()
    };
    for (path, project) in &imports {
        if PERIOD_NAMED.iter().any(|file| path.ends_with(file)) {
            assert!(
                canonical_names(&round_trip(project))
                    == canonical_names(&normalize(project.clone())),
                "{path}"
            );
        }
    }
}

/// Corpus files with a period in a quoted variable name, which the reader
/// rewrites to U+2024 in every ident field (`serde::migrate_stored_ident`,
/// GH #690): `"goal_1.5_for_temperature"` reads back as
/// `"goal_1․5_for_temperature"`, a different display name with the same
/// canonical one. The generators above write no period in a name for the
/// same reason.
const PERIOD_NAMED: [&str; 1] = ["C-LEARN v77 for Vensim.mdl"];

/// The first line at which two projects' `Debug` forms part, for a failure a
/// reader can act on.
fn first_difference(expected: &Project, kept: &Project) -> String {
    let (expected, kept) = (format!("{expected:#?}"), format!("{kept:#?}"));
    expected
        .lines()
        .zip(kept.lines())
        .enumerate()
        .find(|(_, (a, b))| a != b)
        .map(|(n, (a, b))| format!("line {n}: `{}` became `{}`", a.trim(), b.trim()))
        .unwrap_or_else(|| "one form is a prefix of the other".to_string())
}
