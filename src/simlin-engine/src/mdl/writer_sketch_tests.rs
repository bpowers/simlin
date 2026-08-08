// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! MDL writer sketch-section tests (element, connector, and whole-section
//! serialization -- the former Phase 5 Tasks 1-3 block). Split out of
//! `writer_tests.rs` to stay under the per-file line cap (GH #645); shares the
//! `make_*` fixture helpers from the sibling `tests` module.

use super::tests::{make_aux, make_project};
use super::*;
use crate::datamodel::{self, ViewElement, view_element};

// ---- Phase 5 Task 1: Sketch element serialization (types 10, 11, 12) ----

#[test]
fn sketch_aux_element() {
    let aux = view_element::Aux {
        name: "Growth_Rate".to_string(),
        uid: 1,
        x: 100.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut buf = String::new();
    write_aux_element(&mut buf, &aux);
    assert_eq!(buf, "10,1,Growth Rate,100,200,40,20,8,3,0,0,-1,0,0,0");
}

#[test]
fn sketch_stock_element() {
    let stock = view_element::Stock {
        name: "Population".to_string(),
        uid: 2,
        x: 300.0,
        y: 150.0,
        label_side: view_element::LabelSide::Top,
        compat: None,
    };
    let mut buf = String::new();
    write_stock_element(&mut buf, &stock);
    assert_eq!(buf, "10,2,Population,300,150,40,20,3,3,0,0,0,0,0,0");
}

#[test]
fn sketch_flow_element_produces_valve_and_variable() {
    let flow = view_element::Flow {
        name: "Infection_Rate".to_string(),
        uid: 6,
        x: 295.0,
        y: 191.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let mut next_connector_uid = 200;
    write_flow_element(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
    );
    // No flow points, so no pipe connectors; valve and label follow
    assert!(buf.contains("11,100,0,295,191,6,8,34,3,0,0,1,0,0,0"));
    // Label sits 20px below the valve (y 191 -> 211); its box is sized to the
    // text ("Infection Rate" -> 14 chars * 6px = 84 wide, single-line height 11).
    assert!(buf.contains("10,6,Infection Rate,295,211,84,11,40,3,0,0,-1,0,0,0"));
}

#[test]
fn sketch_flow_element_emits_pipe_connectors_from_flow_points() {
    let flow = view_element::Flow {
        name: "Infection_Rate".to_string(),
        uid: 6,
        x: 150.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![
            view_element::FlowPoint {
                x: 100.0,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            view_element::FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(2),
            },
        ],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let mut next_connector_uid = 200;
    write_flow_element(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
    );

    let connector_lines: Vec<&str> = buf.lines().filter(|line| line.starts_with("1,")).collect();
    assert_eq!(
        connector_lines.len(),
        2,
        "Expected two type-1 connector lines for flow endpoints: {}",
        buf
    );
    assert!(
        connector_lines.iter().any(|line| line.contains(",100,1,")),
        "Expected connector from valve uid 100 to endpoint uid 1: {}",
        buf
    );
    assert!(
        connector_lines.iter().any(|line| line.contains(",100,2,")),
        "Expected connector from valve uid 100 to endpoint uid 2: {}",
        buf
    );
}

#[test]
fn sketch_flow_element_derives_stock_connector_points_from_takeoffs() {
    let flow = view_element::Flow {
        name: "Infection_Rate".to_string(),
        uid: 6,
        x: 150.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![
            view_element::FlowPoint {
                x: 122.5,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            view_element::FlowPoint {
                x: 177.5,
                y: 100.0,
                attached_to_uid: Some(2),
            },
        ],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let elem_positions = HashMap::from([(1, (100, 100)), (2, (200, 100))]);
    let stock_uids = HashSet::from([1, 2]);
    let mut next_connector_uid = 200;
    write_flow_element_with_context(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
        SketchTransform::identity(),
        &elem_positions,
        &stock_uids,
        None,
    );

    // Sink pipe (last point) carries direction 4; source pipe (first point)
    // carries direction 100 -- the endpoint *role*, not stock-vs-cloud.
    assert!(
        buf.contains("1,200,100,2,4,0,0,22,0,0,0,-1--1--1,,1|(200,100)|"),
        "sink pipe connector should be reconstructed from the stock center: {buf}"
    );
    assert!(
        buf.contains("1,201,100,1,100,0,0,22,0,0,0,-1--1--1,,1|(100,100)|"),
        "source pipe connector should be reconstructed from the stock center: {buf}"
    );
    // Canonical bottom-label fallback: 20px below the valve (y 100 -> 120),
    // box sized to the text ("Infection Rate" -> 14 chars * 6px = 84, h 11).
    assert!(
        buf.contains("10,6,Infection Rate,150,120,84,11,40,3,0,0,-1,0,0,0"),
        "flow label should fall back to the canonical bottom label position: {buf}"
    );
}

/// An outflow into a sink cloud: the cloud-side pipe (the sink, last point)
/// must carry direction 4 and the stock-side pipe (the source, first point)
/// direction 100 -- the reverse of "stock => 4, cloud => 100", which is what
/// Vensim's own outflow-to-cloud sketches do.
#[test]
fn sketch_flow_outflow_to_cloud_uses_role_based_direction_flags() {
    let flow = view_element::Flow {
        name: "Drain".to_string(),
        uid: 6,
        x: 150.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        points: vec![
            // Source: a stock at the left.
            view_element::FlowPoint {
                x: 122.5,
                y: 100.0,
                attached_to_uid: Some(1),
            },
            // Sink: a cloud at the right.
            view_element::FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: Some(2),
            },
        ],
        compat: None,
        label_compat: None,
    };
    let mut buf = String::new();
    let valve_uids = HashMap::from([(6, 100)]);
    let elem_positions = HashMap::from([(1, (100, 100)), (2, (200, 100))]);
    let stock_uids = HashSet::from([1]); // only uid 1 is a stock; uid 2 is the cloud
    let mut next_connector_uid = 200;
    write_flow_element_with_context(
        &mut buf,
        &flow,
        &valve_uids,
        &HashSet::new(),
        &mut next_connector_uid,
        SketchTransform::identity(),
        &elem_positions,
        &stock_uids,
        None,
    );

    assert!(
        buf.contains("1,200,100,2,4,0,0,22,0,0,0,-1--1--1,,1|(200,100)|"),
        "sink cloud pipe should carry direction 4: {buf}"
    );
    assert!(
        buf.contains("1,201,100,1,100,0,0,22,0,0,0,-1--1--1,,1|(100,100)|"),
        "source stock pipe should carry direction 100: {buf}"
    );
}

#[test]
fn valve_uids_do_not_collide_with_existing_elements() {
    // stock uid=1, flow uid=2 -> valve must NOT get uid=1
    let elements = vec![
        ViewElement::Stock(view_element::Stock {
            name: "Population".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }),
        ViewElement::Flow(view_element::Flow {
            name: "Birth_Rate".to_string(),
            uid: 2,
            x: 200.0,
            y: 100.0,
            label_side: view_element::LabelSide::Bottom,
            points: vec![],
            compat: None,
            label_compat: None,
        }),
    ];

    let valve_uids = allocate_valve_uids(&elements);
    // The valve for flow uid=2 must not equal 1 (stock's uid)
    let valve_uid = valve_uids[&2];
    assert_ne!(valve_uid, 1, "Valve UID collides with stock UID");
    assert_ne!(valve_uid, 2, "Valve UID collides with flow UID");
}

#[test]
fn sketch_cloud_element() {
    let cloud = view_element::Cloud {
        uid: 7,
        flow_uid: 6,
        x: 479.0,
        y: 235.0,
        compat: None,
    };
    let mut buf = String::new();
    write_cloud_element(&mut buf, &cloud);
    assert_eq!(buf, "12,7,48,479,235,10,8,0,3,0,0,-1,0,0,0");
}

#[test]
fn sketch_alias_element() {
    let alias = view_element::Alias {
        uid: 10,
        alias_of_uid: 1,
        x: 200.0,
        y: 300.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut name_map = HashMap::new();
    name_map.insert(1, "Growth_Rate");
    let mut buf = String::new();
    write_alias_element(&mut buf, &alias, &name_map);
    assert!(buf.starts_with("10,10,Growth Rate,200,300,40,20,8,2,0,3,-1,0,0,0,"));
    assert!(buf.contains("128-128-128"));
}

#[test]
fn sketch_alias_element_offsets_stock_ghost_coordinates() {
    let alias = view_element::Alias {
        uid: 10,
        alias_of_uid: 1,
        x: 200.0,
        y: 300.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    };
    let mut name_map = HashMap::new();
    name_map.insert(1, "Population");
    let mut buf = String::new();
    write_alias_element_with_context(
        &mut buf,
        &alias,
        &name_map,
        &HashSet::from([1]),
        SketchTransform::identity(),
        None,
    );
    assert!(
        buf.starts_with("10,10,Population,222,317,40,20,8,2,0,3,-1,0,0,0,"),
        "stock ghosts should serialize using Vensim's stock-alias offset: {buf}"
    );
}

// ---- Phase 5 Task 2: Connector serialization (type 1) ----

#[test]
fn sketch_link_straight() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: None,
    };
    let mut positions = HashMap::new();
    positions.insert(1, (100, 100));
    positions.insert(2, (200, 200));
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    // Straight => control point (0,0), field 9 = 64 (influence connector)
    assert_eq!(buf, "1,3,1,2,0,0,0,0,0,64,0,-1--1--1,,1|(0,0)|");
}

#[test]
fn sketch_link_with_polarity_symbol() {
    let link = view_element::Link {
        uid: 5,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: Some(LinkPolarity::Positive),
    };
    let positions = HashMap::new();
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    // polarity=43 ('+'), field 9 = 64
    assert!(buf.contains(",0,0,43,0,0,64,0,"));
}

#[test]
fn sketch_link_with_polarity_letter() {
    let link = view_element::Link {
        uid: 5,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: Some(LinkPolarity::Positive),
    };
    let positions = HashMap::new();
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, true);
    // polarity=83 ('S' for lettered positive), field 9 = 64
    assert!(buf.contains(",0,0,83,0,0,64,0,"));
}

#[test]
fn sketch_link_arc_produces_nonzero_control_point() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Arc(45.0),
        polarity: None,
    };
    let mut positions = HashMap::new();
    positions.insert(1, (100, 100));
    positions.insert(2, (200, 100));
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    // Arc should produce a non-(0,0) control point
    assert!(
        !buf.contains("|(0,0)|"),
        "arc should not produce (0,0) control point"
    );
}

#[test]
fn sketch_link_with_field_hints_preserves_nonsemantic_flags() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Straight,
        polarity: None,
    };
    let positions = HashMap::from([(1, (100, 100)), (2, (200, 116)), (100, (200, 100))]);
    let compat = view_element::LinkSketchCompat {
        uid: 3,
        field4: 1,
        field10: 7,
    };
    let mut buf = String::new();
    write_link_element_with_context(
        &mut buf,
        &link,
        &positions,
        false,
        Some(&compat),
        SketchTransform::identity(),
        None,
    );
    assert_eq!(buf, "1,3,1,2,1,0,0,0,0,64,7,-1--1--1,,1|(0,0)|");
}

#[test]
fn sketch_link_with_field_hints_still_uses_link_geometry() {
    let link = view_element::Link {
        uid: 3,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::Arc(45.0),
        polarity: None,
    };
    let positions = HashMap::from([(1, (110, 100)), (2, (210, 100))]);
    // A recorded compat carries only field4/field10; the control point is always
    // recomputed from the link's Arc angle and the current endpoint positions.
    let compat = view_element::LinkSketchCompat {
        uid: 3,
        field4: 0,
        field10: 0,
    };
    let mut buf = String::new();
    write_link_element_with_context(
        &mut buf,
        &link,
        &positions,
        false,
        Some(&compat),
        SketchTransform::identity(),
        None,
    );
    let (ctrl_x, ctrl_y) = compute_control_point((110, 100), (210, 100), 45.0);
    assert_eq!(
        buf,
        format!("1,3,1,2,0,0,0,0,0,64,0,-1--1--1,,1|({ctrl_x},{ctrl_y})|")
    );
}

#[test]
fn sketch_link_multipoint_emits_all_points() {
    let points = vec![
        view_element::FlowPoint {
            x: 150.0,
            y: 120.0,
            attached_to_uid: None,
        },
        view_element::FlowPoint {
            x: 170.0,
            y: 140.0,
            attached_to_uid: None,
        },
        view_element::FlowPoint {
            x: 190.0,
            y: 160.0,
            attached_to_uid: None,
        },
    ];
    let link = view_element::Link {
        uid: 4,
        from_uid: 1,
        to_uid: 2,
        shape: LinkShape::MultiPoint(points),
        polarity: None,
    };
    let mut positions = HashMap::new();
    positions.insert(1, (100, 100));
    positions.insert(2, (200, 200));
    let mut buf = String::new();
    write_link_element(&mut buf, &link, &positions, false);
    assert!(
        buf.contains("3|(150,120)|(170,140)|(190,160)|"),
        "multipoint should emit all three points: {buf}"
    );
}

// ---- Phase 5 Task 3: Complete sketch section assembly ----

#[test]
fn sketch_section_structure() {
    let elements = vec![
        ViewElement::Stock(view_element::Stock {
            name: "Population".to_string(),
            uid: 1,
            x: 100.0,
            y: 100.0,
            label_side: view_element::LabelSide::Top,
            compat: None,
        }),
        ViewElement::Aux(view_element::Aux {
            name: "Growth_Rate".to_string(),
            uid: 2,
            x: 200.0,
            y: 200.0,
            label_side: view_element::LabelSide::Bottom,
            compat: None,
        }),
        ViewElement::Link(view_element::Link {
            uid: 3,
            from_uid: 2,
            to_uid: 1,
            shape: LinkShape::Straight,
            polarity: None,
        }),
    ];
    let sf = datamodel::StockFlow {
        name: None,
        elements,
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };
    let views = vec![View::StockFlow(sf)];

    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&views);
    let output = writer.buf;

    // Header
    assert!(
        output.starts_with("V300  Do not put anything below this section"),
        "should start with V300 header"
    );
    // View title
    assert!(output.contains("*View 1\n"), "should have view title");
    // Font line
    assert!(
        output.contains("$192-192-192"),
        "should have font settings line"
    );
    // Elements
    assert!(
        output.contains("10,1,Population,"),
        "should have stock element"
    );
    assert!(
        output.contains("10,2,Growth Rate,"),
        "should have aux element"
    );
    assert!(output.contains("1,3,2,1,"), "should have link element");
    // Terminator
    assert!(
        output.ends_with("///---\\\\\\\n"),
        "should end with sketch terminator"
    );
}

#[test]
fn sketch_section_in_full_project() {
    let var = make_aux("x", "1", None, "");
    let elements = vec![ViewElement::Aux(view_element::Aux {
        name: "x".to_string(),
        uid: 1,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    })];
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: None,
            elements,
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    };
    let project = make_project(vec![model]);

    let result = crate::mdl::project_to_mdl(&project);
    assert!(result.is_ok());
    let mdl = result.unwrap();

    // The sketch section should appear after the equations terminator
    let terminator_pos = mdl
        .find("\\\\\\---/// Sketch information")
        .expect("should have equations terminator");
    let v300_pos = mdl.find("V300").expect("should have V300 header");
    assert!(
        terminator_pos < v300_pos,
        "V300 should come after equations terminator"
    );

    // The sketch terminator should be at the end
    assert!(
        mdl.contains("///---\\\\\\"),
        "should have sketch terminator"
    );
}

#[test]
fn sketch_roundtrip_teacup() {
    // Read teacup.mdl, parse to Project, write sketch section, verify structure
    let mdl_contents = include_str!("../../../../test/test-models/samples/teacup/teacup.mdl");
    let project =
        crate::mdl::parse_mdl(mdl_contents).expect("teacup.mdl should parse successfully");

    let model = &project.models[0];
    assert!(
        !model.views.is_empty(),
        "teacup model should have at least one view"
    );

    // Write the sketch section
    let mut writer = MdlWriter::new();
    writer.write_sketch_section(&model.views);
    let output = writer.buf;

    // Verify structural elements: the teacup model should have stocks, auxes,
    // flows (valve + attached variable), links, and clouds.
    assert!(output.contains("V300"), "output should contain V300 header");
    assert!(
        output.contains("*View 1"),
        "output should contain view title"
    );
    assert!(
        output.contains("///---\\\\\\"),
        "output should end with sketch terminator"
    );

    // The teacup model elements (after roundtrip through datamodel):
    // Stock: Teacup_Temperature -> type 10 with shape=3
    // Aux: Heat_Loss_to_Room flow -> type 11 valve + type 10 attached
    // Aux: Room_Temperature, Characteristic_Time -> type 10
    // Links -> type 1
    // Clouds -> type 12

    // Count element types in output
    let lines: Vec<&str> = output.lines().collect();
    let type10_count = lines.iter().filter(|l| l.starts_with("10,")).count();
    let type11_count = lines.iter().filter(|l| l.starts_with("11,")).count();
    let type12_count = lines.iter().filter(|l| l.starts_with("12,")).count();
    let type1_count = lines.iter().filter(|l| l.starts_with("1,")).count();

    // Teacup has: 1 stock (Teacup_Temperature), 3 auxes (Heat_Loss_to_Room,
    // Room_Temperature, Characteristic_Time), 1 flow (Heat_Loss_to_Room)
    // which produces valve+variable, plus 1 cloud.
    // The exact numbers depend on the MDL->datamodel conversion, but
    // we should have a reasonable set of elements.
    assert!(
        type10_count >= 2,
        "should have at least 2 type-10 elements (variables/stocks), got {type10_count}"
    );
    assert!(
        type11_count >= 1,
        "should have at least 1 type-11 element (valve), got {type11_count}"
    );
    assert!(
        type12_count >= 1,
        "should have at least 1 type-12 element (cloud/comment), got {type12_count}"
    );
    assert!(
        type1_count >= 1,
        "should have at least 1 type-1 element (connector), got {type1_count}"
    );
    // Verify no empty lines were introduced between elements
    let element_lines: Vec<&&str> = lines
        .iter()
        .filter(|l| {
            l.starts_with("10,")
                || l.starts_with("11,")
                || l.starts_with("12,")
                || l.starts_with("1,")
        })
        .collect();
    assert!(
        !element_lines.is_empty(),
        "should have sketch elements in output"
    );

    // Verify the output can be re-parsed as a valid sketch section
    let reparsed = crate::mdl::view::parse_views(&output);
    assert!(
        reparsed.is_ok(),
        "re-serialized sketch should parse: {:?}",
        reparsed.err()
    );
    let views = reparsed.unwrap();
    assert!(
        !views.is_empty(),
        "re-parsed sketch should have at least one view"
    );

    // Verify all expected element types are present after re-parse
    let view = &views[0];
    let has_variable = view
        .iter()
        .any(|e| matches!(e, crate::mdl::view::VensimElement::Variable(_)));
    let has_connector = view
        .iter()
        .any(|e| matches!(e, crate::mdl::view::VensimElement::Connector(_)));
    assert!(has_variable, "re-parsed view should have variables");
    assert!(has_connector, "re-parsed view should have connectors");
}

#[test]
fn sketch_roundtrip_preserves_view_title() {
    let mdl_contents = r#"x = 5
~ ~|
\\\---/// Sketch information
V300  Do not put anything below this section - it will be ignored
*Overview
$192-192-192,0,Times New Roman|12||0-0-0|0-0-0|0-0-255|-1--1--1|-1--1--1|96,96,100,0
10,1,x,100,100,40,20,8,3,0,0,-1,0,0,0
///---\\\
"#;

    let project =
        crate::mdl::parse_mdl(mdl_contents).expect("source MDL should parse successfully");
    let mdl = crate::mdl::project_to_mdl(&project).expect("roundtrip MDL write should work");

    assert!(
        mdl.contains("*Overview\r\n"),
        "Roundtrip should preserve original view title: {}",
        mdl
    );
}

#[test]
fn sketch_roundtrip_sanitizes_multiline_view_title() {
    let var = make_aux("x", "5", Some("Units"), "A constant");
    let model = datamodel::Model {
        name: "default".to_owned(),
        sim_specs: None,
        variables: vec![var],
        views: vec![View::StockFlow(datamodel::StockFlow {
            name: Some("Overview\r\nMain".to_owned()),
            elements: vec![ViewElement::Aux(view_element::Aux {
                name: "x".to_owned(),
                uid: 1,
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            })],
            view_box: Default::default(),
            zoom: 1.0,
            use_lettered_polarity: false,
            font: None,
            sketch_compat: None,
        })],
        loop_metadata: vec![],
        groups: vec![],
        macro_spec: None,
    };
    let project = make_project(vec![model]);

    let mdl = crate::mdl::project_to_mdl(&project).expect("MDL write should succeed");
    assert!(
        mdl.contains("*Overview Main\r\n"),
        "view title should be serialized as a single line: {mdl}",
    );

    let reparsed = crate::mdl::parse_mdl(&mdl).expect("written MDL should parse");
    let View::StockFlow(sf) = &reparsed.models[0].views[0];
    assert_eq!(
        sf.name.as_deref(),
        Some("Overview Main"),
        "sanitized title should roundtrip through MDL",
    );
}
