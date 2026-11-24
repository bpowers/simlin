// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A patch as JSON, landed.

use simlin_base::datamodel;
use simlin_engine::apply_patch;
use simlin_engine::test_common::TestProject;
use simlin_formats::json::ProjectPatch;

/// An engine patch with every model operation reaches a host as JSON and
/// back as the same patch: landing either leaves the same project, and
/// the JSON is the shape `simlin_project_apply_patch` has always taken.
#[test]
fn a_patch_round_trips_through_json_with_every_operation() {
    use simlin_base::project_patch::{
        ModelOperation as Op, ModelPatch as MP, ProjectOperation as PO,
    };
    let base = TestProject::new("roundtrip")
        .with_sim_time(0.0, 10.0, 1.0)
        .stock("level", "10", &["inflow"], &[], None)
        .flow("inflow", "rate", None)
        .aux("rate", "1", None)
        .aux("spare", "2", None)
        .named_dimension("Region", &["North", "South"])
        .array_aux("share[Region]", "1")
        .build_datamodel();
    let aux = |name: &str, eqn: &str| datamodel::Aux {
        ident: name.to_string(),
        equation: datamodel::Equation::Scalar(eqn.to_string()),
        documentation: "notes".to_string(),
        units: Some("widget".to_string()),
        gf: None,
        ai_state: None,
        uid: Some(40),
        compat: datamodel::Compat::default(),
    };
    let mut specs = base.sim_specs.clone();
    specs.stop = 20.0;
    let patch = simlin_base::project_patch::ProjectPatch {
        project_ops: vec![
            PO::SetSimSpecs(specs),
            PO::UpsertDimension(datamodel::Dimension::named(
                "Size".to_string(),
                vec!["Small".to_string(), "Big".to_string()],
            )),
            PO::RenameElement {
                dimension: "Size".to_string(),
                from: "Big".to_string(),
                to: "Large".to_string(),
            },
            PO::RenameDimension {
                from: "Size".to_string(),
                to: "Scale".to_string(),
            },
            PO::DeleteDimension {
                name: "Scale".to_string(),
            },
            PO::RenameDimension {
                from: "Region".to_string(),
                to: "Area".to_string(),
            },
        ],
        models: vec![MP {
            name: "main".to_string(),
            ops: vec![
                Op::UpsertAux(aux("extra", "3")),
                Op::UpsertFlow(datamodel::Flow {
                    ident: "outflow".to_string(),
                    equation: datamodel::Equation::Scalar("level / 4".to_string()),
                    documentation: String::new(),
                    units: None,
                    gf: None,
                    ai_state: None,
                    uid: Some(41),
                    compat: datamodel::Compat::default(),
                }),
                Op::UpdateStockFlows {
                    ident: "level".to_string(),
                    inflows: vec!["inflow".to_string()],
                    outflows: vec!["outflow".to_string()],
                },
                Op::UpsertStock(datamodel::Stock {
                    ident: "reservoir".to_string(),
                    equation: datamodel::Equation::Scalar("0".to_string()),
                    documentation: String::new(),
                    units: None,
                    inflows: vec!["outflow".to_string()],
                    outflows: vec![],
                    ai_state: None,
                    uid: Some(42),
                    compat: datamodel::Compat::default(),
                }),
                Op::RenameVariable {
                    from: "extra".to_string(),
                    to: "bonus".to_string(),
                },
                Op::DeleteVariable {
                    ident: "spare".to_string(),
                },
                Op::SetLoopName {
                    variables: vec!["level".to_string(), "outflow".to_string()],
                    name: "draining".to_string(),
                    description: Some("the level empties".to_string()),
                },
                Op::SetEquation {
                    ident: "level".to_string(),
                    equation: "11".to_string(),
                },
                Op::SetUnits {
                    ident: "rate".to_string(),
                    units: Some("1/month".to_string()),
                },
                Op::SetNotes {
                    ident: "rate".to_string(),
                    notes: "how fast".to_string(),
                },
                Op::SetLookup {
                    ident: "rate".to_string(),
                    element: None,
                    lookup: None,
                },
                Op::SetElementEquation {
                    ident: "share".to_string(),
                    element: vec!["North".to_string()],
                    equation: Some("2".to_string()),
                },
                Op::SetDimensions {
                    ident: "share".to_string(),
                    dimensions: vec![],
                    keep: Some(vec!["North".to_string()]),
                },
                Op::UpsertView {
                    index: 0,
                    view: datamodel::View::StockFlow(datamodel::StockFlow {
                        name: None,
                        elements: vec![].into(),
                        view_box: datamodel::Rect {
                            x: 0.0,
                            y: 0.0,
                            width: 100.0,
                            height: 100.0,
                        },
                        zoom: 1.0,
                        use_lettered_polarity: false,
                        font: None,
                        sketch_compat: None,
                    }),
                },
                Op::EditView {
                    index: 0,
                    upsert: vec![],
                    remove: vec![],
                },
                Op::DeleteView { index: 0 },
            ],
        }],
    };
    let json = ProjectPatch::try_from(patch.clone()).unwrap();
    let text = serde_json::to_string(&json).unwrap();
    let wire: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(wire["projectOps"][0]["type"], "setSimSpecs");
    assert_eq!(wire["projectOps"][1]["type"], "upsertDimension");
    assert_eq!(
        wire["projectOps"][1]["payload"]["dimension"]["elements"][1],
        "Big"
    );
    assert_eq!(wire["projectOps"][2]["type"], "renameElement");
    assert_eq!(wire["projectOps"][3]["payload"]["to"], "Scale");
    assert_eq!(wire["projectOps"][4]["type"], "deleteDimension");
    assert_eq!(wire["models"][0]["ops"][0]["type"], "upsertAux");
    assert_eq!(
        wire["models"][0]["ops"][0]["payload"]["aux"]["name"],
        "extra"
    );
    let back: ProjectPatch = serde_json::from_str(&text).unwrap();
    assert_eq!(back, json);

    let (mut direct, mut through_json) = (base.clone(), base.clone());
    apply_patch(&mut direct, patch).unwrap();
    apply_patch(&mut through_json, back.into()).unwrap();
    assert!(direct == through_json, "the same project either way");
    assert!(direct.models[0].get_variable("bonus").is_some());
    assert_eq!(
        direct.models[0]
            .get_variable("share")
            .unwrap()
            .get_equation(),
        Some(&datamodel::Equation::Scalar("2".to_string())),
        "the field edits land through JSON as directly"
    );
}
