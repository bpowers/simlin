// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Incremental layout's label-side contract.
//!
//! An element's label side is chosen exactly once, when the element is
//! created (or rebuilt with new geometry). A pre-existing element that the
//! patch did not touch keeps its label side byte-for-byte -- a hand-placed
//! label wins over the optimizer, even when a newly-added connector now runs
//! through it. These tests enumerate the arms of that decision:
//!
//! - untouched element (aux, stock, flow, module): preserved, both when the
//!   patch adds an element (settle path) and when it adds only a connector or
//!   only deletes (the no-new-elements early-return path)
//! - renamed element: preserved (a rename keeps the element's geometry)
//! - flow rebuilt for an attach-offset change (orientation unchanged): preserved
//! - flow rebuilt for an orientation flip: re-chosen
//! - element rebuilt after a kind change (aux -> stock): re-chosen
//! - element that is new in this pass: chosen
//!
//! "Re-chosen" is pinned as independence from the stale value: the same
//! patch applied to two old views that differ only in the rebuilt element's
//! label side must produce the same label side.

use super::*;
use crate::datamodel::{self, view_element::LabelSide};

const ALL_SIDES: [LabelSide; 4] = [
    LabelSide::Top,
    LabelSide::Bottom,
    LabelSide::Left,
    LabelSide::Right,
];

fn scalar_stock(ident: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

fn scalar_flow(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Flow(datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    })
}

fn model_with(variables: Vec<datamodel::Variable>) -> datamodel::Model {
    datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables,
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    }
}

/// `(canonical ident, x, y, label_side)` for every named element, keyed by
/// ident so a view produced by a later pass can be compared element-wise.
fn named_geometry(view: &datamodel::StockFlow) -> HashMap<String, (f64, f64, LabelSide)> {
    view.elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => {
                Some((canonicalize(&s.name).into_owned(), (s.x, s.y, s.label_side)))
            }
            ViewElement::Flow(f) => {
                Some((canonicalize(&f.name).into_owned(), (f.x, f.y, f.label_side)))
            }
            ViewElement::Aux(a) => {
                Some((canonicalize(&a.name).into_owned(), (a.x, a.y, a.label_side)))
            }
            ViewElement::Module(m) => {
                Some((canonicalize(&m.name).into_owned(), (m.x, m.y, m.label_side)))
            }
            _ => None,
        })
        .collect()
}

fn set_label_side(view: &mut datamodel::StockFlow, ident: &str, side: LabelSide) {
    let mut found = false;
    for elem in &mut view.elements {
        match elem {
            ViewElement::Stock(s) if canonicalize(&s.name) == ident => {
                s.label_side = side;
                found = true;
            }
            ViewElement::Flow(f) if canonicalize(&f.name) == ident => {
                f.label_side = side;
                found = true;
            }
            ViewElement::Aux(a) if canonicalize(&a.name) == ident => {
                a.label_side = side;
                found = true;
            }
            ViewElement::Module(m) if canonicalize(&m.name) == ident => {
                m.label_side = side;
                found = true;
            }
            _ => {}
        }
    }
    assert!(found, "no named element '{ident}' in view");
}

fn set_all_label_sides(view: &mut datamodel::StockFlow, side: LabelSide) {
    for elem in &mut view.elements {
        match elem {
            ViewElement::Stock(s) => s.label_side = side,
            ViewElement::Flow(f) => f.label_side = side,
            ViewElement::Aux(a) => a.label_side = side,
            ViewElement::Module(m) => m.label_side = side,
            _ => {}
        }
    }
}

fn find_flow<'a>(view: &'a datamodel::StockFlow, ident: &str) -> &'a view_element::Flow {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name) == ident => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("flow '{ident}' not in view"))
}

/// simple_model plus a module instance (`climate`, an instance of a second
/// model in the project) that `death_rate` reads, so every named element kind
/// -- stock, flow, aux, module -- is present in the old view.
fn project_with_module() -> datamodel::Project {
    let mut model = simple_model();
    model
        .variables
        .push(datamodel::Variable::Module(datamodel::Module {
            ident: "climate".to_string(),
            model_name: "climate_model".to_string(),
            documentation: String::new(),
            units: None,
            references: Vec::new(),
            ai_state: None,
            uid: None,
            compat: datamodel::Compat::default(),
        }));
    for var in &mut model.variables {
        if let datamodel::Variable::Aux(a) = var
            && a.ident == "death_rate"
        {
            a.equation = datamodel::Equation::Scalar("0.01 * climate.severity".to_string());
        }
    }
    let submodel = datamodel::Model {
        name: "climate_model".to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "severity".to_string(),
            equation: datamodel::Equation::Scalar("1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat {
                visibility: datamodel::Visibility::Public,
                ..datamodel::Compat::default()
            },
            ai_state: None,
            uid: None,
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    };
    let mut project = test_project(model);
    project.models.push(submodel);
    project
}

fn assert_element_kinds(view: &datamodel::StockFlow) {
    let has = |pred: fn(&ViewElement) -> bool| view.elements.iter().any(pred);
    assert!(
        has(|e| matches!(e, ViewElement::Stock(_))),
        "fixture has a stock"
    );
    assert!(
        has(|e| matches!(e, ViewElement::Flow(_))),
        "fixture has a flow"
    );
    assert!(
        has(|e| matches!(e, ViewElement::Aux(_))),
        "fixture has an aux"
    );
    assert!(
        has(|e| matches!(e, ViewElement::Module(_))),
        "fixture has a module"
    );
}

/// The project plus one new aux that reads an existing stock and feeds an
/// existing flow, so the new connectors touch existing elements from several
/// directions -- the case where re-optimizing existing labels would move them.
fn add_dependent_aux(
    project: &datamodel::Project,
) -> (datamodel::Project, crate::patch::ModelPatch) {
    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    let new_aux = datamodel::Aux {
        ident: "carrying capacity".to_string(),
        equation: datamodel::Equation::Scalar("population * 2".to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    };
    model
        .variables
        .push(datamodel::Variable::Aux(new_aux.clone()));
    for var in &mut model.variables {
        if let datamodel::Variable::Flow(f) = var
            && f.ident == "births"
        {
            f.equation = datamodel::Equation::Scalar(
                "population * birth_rate * (1 - population / carrying_capacity)".to_string(),
            );
        }
    }
    let births = model
        .variables
        .iter()
        .find_map(|v| match v {
            datamodel::Variable::Flow(f) if f.ident == "births" => Some(f.clone()),
            _ => None,
        })
        .unwrap();
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![
            crate::patch::ModelOperation::UpsertAux(new_aux),
            crate::patch::ModelOperation::UpsertFlow(births),
        ],
    };
    (patched, patch)
}

#[test]
fn incremental_layout_preserves_untouched_label_sides() {
    let project = project_with_module();
    let base_view = generate_layout(&project, TEST_MODEL, None).expect("initial layout");
    assert_element_kinds(&base_view);
    let (patched, patch) = add_dependent_aux(&project);

    // Any side an optimizer would not choose is impossible to know a priori,
    // so run every side: for each element at least three of the four runs
    // pin a value the optimizer would have overwritten.
    for side in ALL_SIDES {
        let mut old_view = base_view.clone();
        set_all_label_sides(&mut old_view, side);
        let before = named_geometry(&old_view);

        let new_view = incremental_layout(&old_view, &patched, TEST_MODEL, &patch, None)
            .expect("incremental layout");
        let after = named_geometry(&new_view);

        for (ident, geom) in &before {
            assert_eq!(
                after.get(ident),
                Some(geom),
                "untouched element '{ident}' changed (x, y, label_side) with old side {side:?}"
            );
        }
        assert!(
            after.contains_key("carrying_capacity"),
            "new aux must be laid out"
        );
    }
}

/// Both patches below produce NO new element, so incremental_layout takes
/// its early-return path (diff_connectors + label placement without a
/// settlement step): an equation edit that adds a connector between two
/// existing elements, and a deletion of a leaf aux.
fn connector_only_patch(
    project: &datamodel::Project,
) -> (datamodel::Project, crate::patch::ModelPatch) {
    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    let mut updated = None;
    for var in &mut model.variables {
        if let datamodel::Variable::Aux(a) = var
            && a.ident == "birth_rate"
        {
            // birth_rate now reads death_rate: one new connector, no new element.
            a.equation = datamodel::Equation::Scalar("0.03 + death_rate".to_string());
            updated = Some(a.clone());
        }
    }
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertAux(
            updated.expect("birth_rate in fixture"),
        )],
    };
    (patched, patch)
}

fn delete_only_patch(
    project: &datamodel::Project,
) -> (datamodel::Project, crate::patch::ModelPatch) {
    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    model.variables.retain(|v| v.get_ident() != "birth_rate");
    for var in &mut model.variables {
        if let datamodel::Variable::Flow(f) = var
            && f.ident == "births"
        {
            f.equation = datamodel::Equation::Scalar("population * 0.03".to_string());
        }
    }
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::DeleteVariable {
            ident: "birth_rate".to_string(),
        }],
    };
    (patched, patch)
}

#[test]
fn early_return_path_preserves_untouched_label_sides() {
    let project = project_with_module();
    let base_view = generate_layout(&project, TEST_MODEL, None).expect("initial layout");
    assert_element_kinds(&base_view);

    let cases: [(&str, (datamodel::Project, crate::patch::ModelPatch)); 2] = [
        ("connector-only", connector_only_patch(&project)),
        ("delete-only", delete_only_patch(&project)),
    ];
    for (label, (patched, patch)) in &cases {
        for side in ALL_SIDES {
            let mut old_view = base_view.clone();
            set_all_label_sides(&mut old_view, side);
            let before = named_geometry(&old_view);
            let new_view = incremental_layout(&old_view, patched, TEST_MODEL, patch, None)
                .expect("incremental layout");
            let after = named_geometry(&new_view);
            let survivors: Vec<&String> = patched
                .get_model(TEST_MODEL)
                .unwrap()
                .variables
                .iter()
                .map(|v| v.get_ident())
                .filter_map(|ident| before.get_key_value(ident).map(|(k, _)| k))
                .collect();
            assert_eq!(
                survivors.len(),
                after.len(),
                "{label}: no element is created by a no-new-element patch"
            );
            for ident in survivors {
                assert_eq!(
                    after.get(ident),
                    before.get(ident),
                    "{label}: untouched element '{ident}' changed with old side {side:?}"
                );
            }
        }
    }
    let (_, connector_patch) = &cases[0].1;
    let connector_view =
        incremental_layout(&base_view, &cases[0].1.0, TEST_MODEL, connector_patch, None).unwrap();
    assert!(
        connector_view.elements.len() > base_view.elements.len(),
        "fixture: the connector-only patch really adds a link"
    );
}

#[test]
fn incremental_layout_preserves_label_side_across_rename() {
    let project = test_project(simple_model());
    let base_view = generate_layout(&project, TEST_MODEL, None).expect("initial layout");

    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    for var in &mut model.variables {
        match var {
            datamodel::Variable::Aux(a) if a.ident == "birth_rate" => {
                a.ident = "fertility".to_string();
            }
            datamodel::Variable::Flow(f) if f.ident == "births" => {
                f.equation = datamodel::Equation::Scalar("population * fertility".to_string());
            }
            _ => {}
        }
    }
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::RenameVariable {
            from: "birth_rate".to_string(),
            to: "fertility".to_string(),
        }],
    };

    for side in ALL_SIDES {
        let mut old_view = base_view.clone();
        set_label_side(&mut old_view, "birth_rate", side);
        let new_view = incremental_layout(&old_view, &patched, TEST_MODEL, &patch, None)
            .expect("incremental layout");
        let after = named_geometry(&new_view);
        let before = named_geometry(&old_view);
        assert_eq!(
            after["fertility"].2, side,
            "renamed element must keep its label side"
        );
        assert_eq!(
            (after["fertility"].0, after["fertility"].1),
            (before["birth_rate"].0, before["birth_rate"].1),
            "renamed element must keep its position"
        );
    }
}

/// Fixture: stock_a -> chain_flow -> stock_b plus stock_a -> waste_a -> cloud.
/// Adding waste_b moves waste_a along the bottom face (offset 0.5 -> 1/3),
/// which rebuilds waste_a's geometry without changing its orientation.
fn side_flow_project() -> datamodel::Project {
    test_project(model_with(vec![
        scalar_stock("stock_a", &[], &["chain_flow", "waste_a"]),
        scalar_stock("stock_b", &["chain_flow"], &[]),
        scalar_flow("chain_flow", "10"),
        scalar_flow("waste_a", "3"),
    ]))
}

fn add_waste_b(project: &datamodel::Project) -> (datamodel::Project, crate::patch::ModelPatch) {
    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == "stock_a"
        {
            s.outflows.push("waste_b".to_string());
        }
    }
    let waste_b = scalar_flow("waste_b", "2");
    model.variables.push(waste_b.clone());
    let datamodel::Variable::Flow(waste_b) = waste_b else {
        unreachable!()
    };
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![
            crate::patch::ModelOperation::UpsertFlow(waste_b),
            crate::patch::ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec![
                    "chain_flow".to_string(),
                    "waste_a".to_string(),
                    "waste_b".to_string(),
                ],
            },
        ],
    };
    (patched, patch)
}

#[test]
fn rebuilt_flow_with_unchanged_orientation_keeps_label_side() {
    let project = side_flow_project();
    let base_view = generate_layout(&project, TEST_MODEL, None).expect("initial layout");
    let (patched, patch) = add_waste_b(&project);

    let old_waste_a = find_flow(&base_view, "waste_a");
    assert!(
        matches!(
            compute_flow_orientation(&old_waste_a.points),
            FlowOrientation::Vertical
        ),
        "fixture: waste_a must start vertical (a side flow below stock_a)"
    );
    let old_attach_x = old_waste_a.points[0].x;

    for side in [LabelSide::Left, LabelSide::Right] {
        let mut old_view = base_view.clone();
        set_label_side(&mut old_view, "waste_a", side);
        let new_view = incremental_layout(&old_view, &patched, TEST_MODEL, &patch, None)
            .expect("incremental layout");
        let new_waste_a = find_flow(&new_view, "waste_a");
        assert!(
            (new_waste_a.points[0].x - old_attach_x).abs() > 1.0,
            "fixture: waste_a must actually be rebuilt (attach x moved from {old_attach_x} to {})",
            new_waste_a.points[0].x
        );
        assert!(
            matches!(
                compute_flow_orientation(&new_waste_a.points),
                FlowOrientation::Vertical
            ),
            "fixture: waste_a must stay vertical"
        );
        assert_eq!(
            new_waste_a.label_side, side,
            "a flow rebuilt only for an attach-offset change keeps its label side"
        );
    }
}

/// Fixture: stock_a -> chain_flow -> stock_b plus stock_a -> waste_flow -> cloud.
/// Deleting the chain reclassifies waste_flow from the bottom face to the
/// right face, i.e. vertical -> horizontal.
fn flip_flow_project() -> datamodel::Project {
    test_project(model_with(vec![
        scalar_stock("stock_a", &[], &["chain_flow", "waste_flow"]),
        scalar_stock("stock_b", &["chain_flow"], &[]),
        scalar_flow("chain_flow", "10"),
        scalar_flow("waste_flow", "5"),
    ]))
}

fn remove_chain(project: &datamodel::Project) -> (datamodel::Project, crate::patch::ModelPatch) {
    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    model
        .variables
        .retain(|v| v.get_ident() != "chain_flow" && v.get_ident() != "stock_b");
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == "stock_a"
        {
            s.outflows = vec!["waste_flow".to_string()];
        }
    }
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![
            crate::patch::ModelOperation::DeleteVariable {
                ident: "chain_flow".to_string(),
            },
            crate::patch::ModelOperation::DeleteVariable {
                ident: "stock_b".to_string(),
            },
            crate::patch::ModelOperation::UpdateStockFlows {
                ident: "stock_a".to_string(),
                inflows: vec![],
                outflows: vec!["waste_flow".to_string()],
            },
        ],
    };
    (patched, patch)
}

#[test]
fn rebuilt_flow_with_flipped_orientation_rechooses_label_side() {
    let project = flip_flow_project();
    let base_view = generate_layout(&project, TEST_MODEL, None).expect("initial layout");
    let (patched, patch) = remove_chain(&project);
    assert!(
        matches!(
            compute_flow_orientation(&find_flow(&base_view, "waste_flow").points),
            FlowOrientation::Vertical
        ),
        "fixture: waste_flow must start vertical"
    );

    let mut chosen = Vec::new();
    for side in ALL_SIDES {
        let mut old_view = base_view.clone();
        set_label_side(&mut old_view, "waste_flow", side);
        let new_view = incremental_layout(&old_view, &patched, TEST_MODEL, &patch, None)
            .expect("incremental layout");
        let waste = find_flow(&new_view, "waste_flow");
        assert!(
            matches!(
                compute_flow_orientation(&waste.points),
                FlowOrientation::Horizontal
            ),
            "fixture: waste_flow must flip to horizontal"
        );
        assert!(
            matches!(waste.label_side, LabelSide::Top | LabelSide::Bottom),
            "a horizontal flow's label must sit above or below the pipe, got {:?}",
            waste.label_side
        );
        chosen.push(waste.label_side);
    }
    assert!(
        chosen.iter().all(|s| *s == chosen[0]),
        "a flipped flow's label side must not depend on the stale side: {chosen:?}"
    );
}

#[test]
fn kind_changed_element_rechooses_label_side() {
    // birth_rate (aux) becomes a stock via UpsertStock with no DeleteVariable;
    // the old aux element is rebuilt as a stock element with fresh geometry.
    let project = test_project(simple_model());
    let base_view = generate_layout(&project, TEST_MODEL, None).expect("initial layout");

    let mut patched = project.clone();
    let model = patched.get_model_mut(TEST_MODEL).unwrap();
    let new_stock = datamodel::Stock {
        ident: "birth_rate".to_string(),
        equation: datamodel::Equation::Scalar("0.03".to_string()),
        documentation: String::new(),
        units: None,
        inflows: vec![],
        outflows: vec![],
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    };
    model.variables.retain(|v| v.get_ident() != "birth_rate");
    model
        .variables
        .push(datamodel::Variable::Stock(new_stock.clone()));
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertStock(new_stock)],
    };

    let mut chosen = Vec::new();
    for side in ALL_SIDES {
        let mut old_view = base_view.clone();
        set_label_side(&mut old_view, "birth_rate", side);
        let new_view = incremental_layout(&old_view, &patched, TEST_MODEL, &patch, None)
            .expect("incremental layout");
        let stock = new_view
            .elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Stock(s) if canonicalize(&s.name) == "birth_rate" => Some(s),
                _ => None,
            })
            .expect("birth_rate rebuilt as a stock");
        chosen.push(stock.label_side);
    }
    assert!(
        chosen.iter().all(|s| *s == chosen[0]),
        "a kind-changed element's label side must not depend on the stale side: {chosen:?}"
    );
}
