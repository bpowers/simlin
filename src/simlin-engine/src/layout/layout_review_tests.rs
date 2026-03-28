// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;

// Tests for code review feedback fixes

#[test]
fn test_build_stock_flow_from_state_resets_invalid_zoom() {
    // When a template view has zoom <= 0 (e.g. from an imported or
    // hand-authored JSON view), build_stock_flow_from_state should
    // fall back to 1.0 instead of preserving the invalid value.
    let config = LayoutConfig::default();
    let model = simple_model();
    let project = test_project(model.clone());
    let layout = generate_best_layout(&project, TEST_MODEL, None).expect("layout should succeed");

    // Seed state from the generated layout, then build with a zero-zoom template
    let state = LayoutState::from_existing_view(&layout, &model);
    let mut bad_template = layout.clone();
    bad_template.zoom = 0.0;

    let result = build_stock_flow_from_state(state, &config, &bad_template);
    assert!(
        result.zoom > 0.0,
        "zoom must be positive, got {}",
        result.zoom
    );
    assert!(
        (result.zoom - 1.0).abs() < f64::EPSILON,
        "invalid zoom should reset to 1.0, got {}",
        result.zoom
    );

    // Also test negative zoom
    let state2 = LayoutState::from_existing_view(&layout, &model);
    let mut neg_template = layout;
    neg_template.zoom = -1.5;
    let result2 = build_stock_flow_from_state(state2, &config, &neg_template);
    assert!(
        (result2.zoom - 1.0).abs() < f64::EPSILON,
        "negative zoom should reset to 1.0, got {}",
        result2.zoom
    );
}

#[test]
fn test_apply_deletion_removes_alias_of_deleted_var() {
    // Issue 2: apply_deletion must also remove Alias elements where
    // alias_of_uid == deleted_uid.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "rate".to_string(),
            equation: datamodel::Equation::Scalar("0.5".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let mut state = LayoutState::new(&model);

    // Add the aux element and an alias pointing to it
    let aux_uid = state.get_or_alloc_uid("rate");
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "rate".to_string(),
        uid: aux_uid,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(aux_uid, Position::new(100.0, 100.0));

    let alias_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Alias(view_element::Alias {
        uid: alias_uid,
        alias_of_uid: aux_uid,
        x: 200.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));

    assert_eq!(state.elements.len(), 2);

    // Delete the aux -- the alias should be removed too
    state.apply_deletion("rate");

    let alias_count = state
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Alias(_)))
        .count();
    assert_eq!(
        alias_count, 0,
        "alias of deleted aux should have been removed"
    );
    assert!(
        state.elements.is_empty(),
        "all elements should be gone after deleting the only var"
    );
}

#[test]
fn test_existing_bounding_box_negative_positions() {
    // Issue 3: existing_bounding_box initializes max_x/max_y with f64::MIN
    // instead of f64::NEG_INFINITY. Verify the bounding box correctly
    // encompasses elements at negative coordinates.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "x".to_string(),
            equation: datamodel::Equation::Scalar("1".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let mut state = LayoutState::new(&model);

    // Insert elements at negative coordinates
    let uid_a = state.uid_manager.alloc("a");
    let uid_b = state.uid_manager.alloc("b");
    state.positions.insert(uid_a, Position::new(-500.0, -300.0));
    state.positions.insert(uid_b, Position::new(-100.0, -50.0));

    let (min_pos, max_pos) = existing_bounding_box(&state);

    assert!(
        (min_pos.x - (-500.0)).abs() < 1e-9,
        "min_x should be -500, got {}",
        min_pos.x
    );
    assert!(
        (min_pos.y - (-300.0)).abs() < 1e-9,
        "min_y should be -300, got {}",
        min_pos.y
    );
    assert!(
        (max_pos.x - (-100.0)).abs() < 1e-9,
        "max_x should be -100, got {}",
        max_pos.x
    );
    assert!(
        (max_pos.y - (-50.0)).abs() < 1e-9,
        "max_y should be -50, got {}",
        max_pos.y
    );
}

#[test]
fn test_incremental_flow_endpoints_rebuilt_after_topology_change() {
    // Issue 1: When a flow's stock connections change (e.g., F moves from
    // being an inflow to stock A to being an inflow to stock B), the flow
    // element should be rebuilt with the new attached_to_uid values.

    // Build a model: stock_a <-- flow_f (flow_f is an inflow to stock_a)
    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["flow_f".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("50".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "flow_f".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify old_view: flow_f has an endpoint attached to stock_a
    let stock_a_uid = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "stock_a" => Some(s.uid),
            _ => None,
        })
        .expect("stock_a should be in old_view");

    let flow_f_in_old = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "flow_f" => Some(f.clone()),
            _ => None,
        })
        .expect("flow_f should be in old_view");

    let old_attached_to_stock_a = flow_f_in_old
        .points
        .iter()
        .any(|pt| pt.attached_to_uid == Some(stock_a_uid));
    assert!(
        old_attached_to_stock_a,
        "flow_f should initially have an endpoint attached to stock_a"
    );

    // Now patch: move flow_f to be an inflow to stock_b instead
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    for var in &mut model.variables {
        match var {
            datamodel::Variable::Stock(s) if s.ident == "stock_a" => {
                s.inflows.clear();
            }
            datamodel::Variable::Stock(s) if s.ident == "stock_b" => {
                s.inflows = vec!["flow_f".to_string()];
            }
            _ => {}
        }
    }

    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![
            crate::patch::ModelOperation::UpsertStock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            crate::patch::ModelOperation::UpsertStock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("50".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["flow_f".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout after topology change should succeed");

    // Find stock_b's UID in the new view
    let stock_b_uid = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "stock_b" => Some(s.uid),
            _ => None,
        })
        .expect("stock_b should be in new_view");

    // flow_f should now have an endpoint attached to stock_b, not stock_a
    let flow_f_in_new = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "flow_f" => Some(f.clone()),
            _ => None,
        })
        .expect("flow_f should be in new_view");

    let attached_to_stock_b = flow_f_in_new
        .points
        .iter()
        .any(|pt| pt.attached_to_uid == Some(stock_b_uid));
    assert!(
        attached_to_stock_b,
        "flow_f should have an endpoint attached to stock_b after topology change, points: {:?}",
        flow_f_in_new.points
    );

    // flow_f should NOT still be attached to stock_a
    let still_attached_to_stock_a = flow_f_in_new
        .points
        .iter()
        .any(|pt| pt.attached_to_uid == Some(stock_a_uid));
    assert!(
        !still_attached_to_stock_a,
        "flow_f should NOT still be attached to stock_a after topology change"
    );
}

// ---- Issue 1: flow endpoint transitions between stock and cloud ----

#[test]
fn test_incremental_flow_endpoint_stock_to_cloud() {
    // Build a model where flow_f flows from stock_b into stock_a (both endpoints on stocks).
    // Then patch so flow_f flows from a cloud into stock_a (from-stock becomes None).
    // After incremental_layout the source point must NOT be attached to any stock.

    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["flow_f".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("200".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["flow_f".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "flow_f".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify initial state: flow_f has a source endpoint attached to stock_b
    let stock_b_uid_old = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "stock_b" => Some(s.uid),
            _ => None,
        })
        .expect("stock_b should be in old_view");
    let flow_f_old = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "flow_f" => Some(f.clone()),
            _ => None,
        })
        .expect("flow_f should be in old_view");
    assert!(
        flow_f_old
            .points
            .iter()
            .any(|pt| pt.attached_to_uid == Some(stock_b_uid_old)),
        "flow_f should initially have source attached to stock_b"
    );

    // Patch: stock_b no longer has flow_f as an outflow (flow becomes cloud-sourced)
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == "stock_b"
        {
            s.outflows.clear();
        }
    }
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertStock(
            datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("200".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            },
        )],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout should succeed");

    // Collect all stock UIDs in the new view
    let stock_uids: HashSet<i32> = new_view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some(s.uid),
            _ => None,
        })
        .collect();

    let flow_f_new = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "flow_f" => Some(f.clone()),
            _ => None,
        })
        .expect("flow_f should be in new_view");

    // The source point (first point) must not be attached to any stock
    let source_pt = &flow_f_new.points[0];
    assert!(
        source_pt
            .attached_to_uid
            .is_none_or(|uid| !stock_uids.contains(&uid)),
        "flow_f source must be unattached to any stock (cloud) after stock-to-cloud transition, \
         got attached_to_uid={:?}, stock_uids={:?}",
        source_pt.attached_to_uid,
        stock_uids
    );
}

#[test]
fn test_incremental_flow_endpoint_cloud_to_stock() {
    // Build a model where flow_f flows from a cloud into stock_a.
    // Then patch so flow_f flows from stock_b into stock_a.
    // After incremental_layout the source point must be attached to stock_b.

    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_a".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["flow_f".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("50".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "flow_f".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(3),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify initial state: flow_f source is a cloud (not attached to any stock)
    let old_stock_uids: HashSet<i32> = old_view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some(s.uid),
            _ => None,
        })
        .collect();
    let flow_f_old = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "flow_f" => Some(f.clone()),
            _ => None,
        })
        .expect("flow_f should be in old_view");
    assert!(
        flow_f_old.points[0]
            .attached_to_uid
            .is_none_or(|uid| !old_stock_uids.contains(&uid)),
        "flow_f source should initially be a cloud (not attached to a stock)"
    );

    // Patch: stock_b now has flow_f as outflow (flow becomes stock_b-sourced)
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    for var in &mut model.variables {
        if let datamodel::Variable::Stock(s) = var
            && s.ident == "stock_b"
        {
            s.outflows = vec!["flow_f".to_string()];
        }
    }
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertStock(
            datamodel::Stock {
                ident: "stock_b".to_string(),
                equation: datamodel::Equation::Scalar("50".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["flow_f".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            },
        )],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout should succeed");

    let stock_b_uid_new = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "stock_b" => Some(s.uid),
            _ => None,
        })
        .expect("stock_b should be in new_view");

    let flow_f_new = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "flow_f" => Some(f.clone()),
            _ => None,
        })
        .expect("flow_f should be in new_view");

    // The source point must now be attached to stock_b
    let attached_to_stock_b = flow_f_new
        .points
        .iter()
        .any(|pt| pt.attached_to_uid == Some(stock_b_uid_new));
    assert!(
        attached_to_stock_b,
        "flow_f source should be attached to stock_b after cloud-to-stock transition, \
         points={:?}",
        flow_f_new.points
    );
}

// ---- Issue 2: variable kind changes not detected as replacements ----
//
// These tests exercise the case where a caller issues UpsertStock (or UpsertAux)
// WITHOUT a preceding DeleteVariable.  The patch is therefore a kind-change with
// no explicit removal, which identify_new_elements must still detect.

#[test]
fn test_incremental_kind_change_aux_to_stock_no_delete() {
    // Build a model with aux "foo". Generate layout. Change "foo" to a stock
    // using only UpsertStock (no DeleteVariable in the patch).
    // After incremental_layout the view must contain a Stock element for "foo",
    // not an Aux.

    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "foo".to_string(),
                equation: datamodel::Equation::Scalar("42".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "bar".to_string(),
                equation: datamodel::Equation::Scalar("1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify initial state: "foo" is an Aux element
    assert!(
        old_view
            .elements
            .iter()
            .any(|e| matches!(e, ViewElement::Aux(a) if canonicalize(&a.name).as_ref() == "foo")),
        "foo should start as Aux in old_view"
    );

    // Patch: kind-change aux "foo" -> stock "foo" with no DeleteVariable.
    // The patched project reflects the new state; the patch has only UpsertStock.
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    model.variables.retain(|v| v.get_ident() != "foo");
    model
        .variables
        .push(datamodel::Variable::Stock(datamodel::Stock {
            ident: "foo".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        }));
    // Only UpsertStock -- no explicit DeleteVariable
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertStock(
            datamodel::Stock {
                ident: "foo".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            },
        )],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout after kind change should succeed");

    // "foo" must now be a Stock element
    let foo_is_stock = new_view
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "foo"));
    assert!(
        foo_is_stock,
        "foo should be a Stock after kind change from Aux to Stock"
    );

    // "foo" must NOT remain an Aux element
    let foo_is_aux = new_view
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Aux(a) if canonicalize(&a.name).as_ref() == "foo"));
    assert!(
        !foo_is_aux,
        "foo must not remain as an Aux after kind change to Stock"
    );
}

#[test]
fn test_incremental_kind_change_stock_to_aux_no_delete() {
    // Build a model with stock "tank" and no flows. Generate layout.
    // Change "tank" to an aux using only UpsertAux (no DeleteVariable).
    // After incremental_layout the view must contain an Aux element for "tank",
    // not a Stock.

    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "tank".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "rate".to_string(),
                equation: datamodel::Equation::Scalar("0.1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    assert!(
        old_view.elements.iter().any(
            |e| matches!(e, ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "tank")
        ),
        "tank should start as Stock in old_view"
    );

    // Patch: kind-change stock "tank" -> aux "tank" with no DeleteVariable.
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    model.variables.retain(|v| v.get_ident() != "tank");
    model
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "tank".to_string(),
            equation: datamodel::Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        }));
    // Only UpsertAux -- no explicit DeleteVariable
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertAux(datamodel::Aux {
            ident: "tank".to_string(),
            equation: datamodel::Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        })],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout after kind change should succeed");

    let tank_is_aux = new_view
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Aux(a) if canonicalize(&a.name).as_ref() == "tank"));
    assert!(
        tank_is_aux,
        "tank should be an Aux after kind change from Stock to Aux"
    );

    let tank_is_stock = new_view
        .elements
        .iter()
        .any(|e| matches!(e, ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "tank"));
    assert!(
        !tank_is_stock,
        "tank must not remain as a Stock after kind change to Aux"
    );
}

// ---- Issue 1 (PR review iter 3): diff_clouds must wire cloud UIDs into flow attached_to_uid ----

#[test]
fn test_diff_clouds_wires_attached_to_uid_for_unattached_flow_point() {
    // Simulate an XMILE import where a cloud element exists but the flow's source
    // point has attached_to_uid == None.  After diff_clouds the flow point's
    // attached_to_uid must reference the cloud's UID.
    let mut state = LayoutState {
        uid_manager: UidManager::new(),
        display_names: HashMap::new(),
        elements: Vec::new(),
        positions: HashMap::new(),
        flow_templates: HashMap::new(),
        cloud_ident_to_uid: HashMap::new(),
        cloud_ident_to_flow_ident: HashMap::new(),
        flow_ident_to_clouds: HashMap::new(),
    };

    // flow "f" uid=2, from cloud (no from_stock)
    state.uid_manager.add(2, "f");
    state.display_names.insert("f".into(), "f".into());

    // Flow element whose source point has attached_to_uid == None (XMILE import style)
    state.elements.push(ViewElement::Flow(view_element::Flow {
        name: "f".into(),
        uid: 2,
        x: 100.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        points: vec![
            FlowPoint {
                x: 50.0,
                y: 100.0,
                attached_to_uid: None,
            },
            FlowPoint {
                x: 200.0,
                y: 100.0,
                attached_to_uid: None,
            },
        ],
        compat: None,
        label_compat: None,
    }));
    state.positions.insert(2, Position::new(100.0, 100.0));

    // Cloud already exists for this flow, but the flow point is unattached
    let cloud_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Cloud(view_element::Cloud {
        uid: cloud_uid,
        flow_uid: 2,
        x: 50.0,
        y: 100.0,
        compat: None,
    }));
    state
        .positions
        .insert(cloud_uid, Position::new(50.0, 100.0));

    // Metadata says "f" has no from_stock (needs source cloud)
    let metadata = ComputedMetadata {
        chains: Vec::new(),
        feedback_loops: Vec::new(),
        dominant_periods: Vec::new(),
        dep_graph: BTreeMap::new(),
        reverse_dep_graph: BTreeMap::new(),
        constants: BTreeSet::new(),
        stock_to_inflows: HashMap::new(),
        stock_to_outflows: HashMap::new(),
        flow_to_stocks: HashMap::from([("f".into(), (None, None))]),
    };

    diff_clouds(&mut state, &metadata);

    // After diff_clouds, the flow's source point must be attached to the cloud
    let flow = state
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.uid == 2 => Some(f.clone()),
            _ => None,
        })
        .expect("flow should still exist after diff_clouds");

    let surviving_cloud_uid = state
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Cloud(c) if c.flow_uid == 2 => Some(c.uid),
            _ => None,
        })
        .expect("cloud for flow should still exist");

    let source_attached = flow.points[0].attached_to_uid;
    assert_eq!(
        source_attached,
        Some(surviving_cloud_uid),
        "source flow point attached_to_uid should reference the cloud after diff_clouds, \
         got {:?}",
        source_attached
    );
}

// ---- Issue 2 (PR review iter 3): display names preserved through type-change deletion ----

#[test]
fn test_incremental_kind_change_preserves_display_name() {
    // Build a model with aux "Growth Rate" (ident: "Growth Rate", canonical: "growth_rate").
    // Generate layout.  Change it to a stock via type-change (UpsertStock, no DeleteVariable).
    // The resulting Stock element's name must be "Growth Rate", not "growth_rate".

    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "Growth Rate".to_string(),
                equation: datamodel::Equation::Scalar("0.1".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Aux(datamodel::Aux {
                ident: "other".to_string(),
                equation: datamodel::Equation::Scalar("1.0".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify initial state: the display name was preserved
    let aux_name = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Aux(a) if canonicalize(&a.name).as_ref() == "growth_rate" => {
                Some(a.name.clone())
            }
            _ => None,
        })
        .expect("Growth Rate aux should exist in old view");
    // The aux element name should contain "Growth" (line-wrapped or not)
    assert!(
        aux_name.contains("Growth"),
        "aux display name should contain 'Growth', got '{}'",
        aux_name
    );

    // Patch: kind-change aux "Growth Rate" -> stock, no DeleteVariable
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    model.variables.retain(|v| v.get_ident() != "Growth Rate");
    model
        .variables
        .push(datamodel::Variable::Stock(datamodel::Stock {
            ident: "Growth Rate".to_string(),
            equation: datamodel::Equation::Scalar("0".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        }));
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertStock(
            datamodel::Stock {
                ident: "Growth Rate".to_string(),
                equation: datamodel::Equation::Scalar("0".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            },
        )],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout after kind change should succeed");

    // The rebuilt Stock element must use "Growth Rate", not "growth_rate"
    let stock_name = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "growth_rate" => {
                Some(s.name.clone())
            }
            _ => None,
        })
        .expect("Growth Rate stock should exist in new view");

    assert!(
        stock_name.contains("Growth"),
        "rebuilt stock display name should contain 'Growth' (the original case), \
         got '{}' -- display name was lost during type-change deletion",
        stock_name
    );
}

// ---- Issue 3 (PR review iter 3): alias positions removed during deletion ----

#[test]
fn test_apply_deletion_removes_alias_position_from_state() {
    // Build a LayoutState with an aux and an alias pointing to it (both with positions).
    // Delete the aux.  The alias's position must also be removed from state.positions.
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Aux(datamodel::Aux {
            ident: "rate".to_string(),
            equation: datamodel::Equation::Scalar("0.5".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let mut state = LayoutState::new(&model);

    let aux_uid = state.get_or_alloc_uid("rate");
    state.elements.push(ViewElement::Aux(view_element::Aux {
        name: "rate".to_string(),
        uid: aux_uid,
        x: 100.0,
        y: 100.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state.positions.insert(aux_uid, Position::new(100.0, 100.0));

    // Add an alias element with its own position entry
    let alias_uid = state.uid_manager.alloc("");
    state.elements.push(ViewElement::Alias(view_element::Alias {
        uid: alias_uid,
        alias_of_uid: aux_uid,
        x: 200.0,
        y: 200.0,
        label_side: view_element::LabelSide::Bottom,
        compat: None,
    }));
    state
        .positions
        .insert(alias_uid, Position::new(200.0, 200.0));

    assert!(
        state.positions.contains_key(&alias_uid),
        "precondition: alias position should exist before deletion"
    );

    state.apply_deletion("rate");

    assert!(
        !state.positions.contains_key(&alias_uid),
        "alias position should be removed from state.positions after aux deletion"
    );
    assert!(
        !state.positions.contains_key(&aux_uid),
        "aux position should also be removed"
    );
    assert!(state.elements.is_empty(), "all elements should be removed");
}

// ---- Issue 3: group names in UidManager must not collide with variable names ----

#[test]
fn test_group_uid_does_not_collide_with_variable_uid() {
    // Build a view with a stock named "production" (uid=10) followed by a group
    // also named "production" (uid=5).  Without the fix, processing the group
    // after the stock overwrites the reverse lookup so get_uid("production")
    // returns 5 (the group) instead of 10 (the stock).

    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Stock(datamodel::Stock {
            ident: "production".to_string(),
            equation: datamodel::Equation::Scalar("50".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(10),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    // Stock comes first so its reverse-mapping is established; then the group
    // (same name, uid=5) appears later and -- without the fix -- overwrites it.
    let old_view = datamodel::StockFlow {
        name: None,
        elements: vec![
            ViewElement::Stock(view_element::Stock {
                uid: 10,
                name: "production".to_string(),
                x: 100.0,
                y: 100.0,
                label_side: view_element::LabelSide::Bottom,
                compat: None,
            }),
            ViewElement::Group(view_element::Group {
                uid: 5,
                name: "production".to_string(),
                x: 0.0,
                y: 0.0,
                width: 200.0,
                height: 200.0,
                is_mdl_view_marker: false,
            }),
        ],
        view_box: datamodel::Rect::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };

    let state = LayoutState::from_existing_view(&old_view, &model);

    // get_uid("production") must return the stock's UID (10), not the group's (5)
    let uid = state
        .uid_manager
        .get_uid("production")
        .expect("production should have a UID");
    assert_eq!(
        uid, 10,
        "get_uid('production') should return the stock's UID (10), not the group's (5)"
    );
}

// ---- Issue 4 (PR review iter 4): cloud positions not recorded when create_flow_view_element ----
// ---- creates clouds for new flows in incremental_layout                                     ----

#[test]
fn test_incremental_new_flow_cloud_positions_recorded() {
    // Start with a model that has just one stock (no flows), generate an initial layout,
    // then add a new outflow with a cloud sink.  After incremental_layout, verify that
    // the cloud's coordinates match the settled flow endpoint, not a stale creation position.
    //
    // Before the fix, create_flow_view_element called build_clouds_for_flow which pushed
    // clouds into state.elements but never added their positions to state.positions.
    // settle_new_elements therefore could not seed initial positions for the cloud nodes.
    // When the flow center later moved during SFDP, the flow element and its flow points
    // were shifted by dx/dy, but the cloud stayed at its original creation coordinates
    // because it had no entry in state.positions and was skipped by the update loop.

    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![datamodel::Variable::Stock(datamodel::Stock {
            ident: "tank".to_string(),
            equation: datamodel::Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            inflows: vec![],
            outflows: vec![],
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        })],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify that the stock is placed at a reasonable position (not origin).
    let tank_pos = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "tank" => Some((s.x, s.y)),
            _ => None,
        })
        .expect("tank stock should be in initial view");

    assert!(
        tank_pos.0 > 0.0 && tank_pos.1 > 0.0,
        "tank stock should have positive coordinates in initial view, got {:?}",
        tank_pos
    );

    // Patch: add a new flow "drain" that flows out of tank (sink is a cloud).
    let patched_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "tank".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec![],
                outflows: vec!["drain".to_string()],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "drain".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };
    let patched_project = test_project(patched_model);

    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![
            crate::patch::ModelOperation::UpdateStockFlows {
                ident: "tank".to_string(),
                inflows: vec![],
                outflows: vec!["drain".to_string()],
            },
            crate::patch::ModelOperation::UpsertFlow(datamodel::Flow {
                ident: "drain".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout should succeed after adding a flow with cloud endpoint");

    // Locate the flow and its cloud in the new view.
    let drain_elem = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "drain" => Some(f.clone()),
            _ => None,
        })
        .expect("drain flow should be present in new view");

    let clouds: Vec<_> = new_view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Cloud(c) if c.flow_uid == drain_elem.uid => Some(c.clone()),
            _ => None,
        })
        .collect();

    assert_eq!(
        clouds.len(),
        1,
        "drain flow (outflow from tank with cloud sink) should have exactly one cloud, got {}",
        clouds.len()
    );

    let cloud = &clouds[0];

    // The cloud must not be stranded at (0, 0).  In a layout where tank is at a
    // positive position, a cloud that was properly settled will be near the flow
    // endpoint which itself is near the tank.  Checking that cloud (x,y) are
    // non-zero is sufficient to detect the "not recorded" defect.
    assert!(
        cloud.x != 0.0 || cloud.y != 0.0,
        "cloud for 'drain' should not be at the origin (0, 0); \
         positions were not recorded in state.positions before settle_new_elements ran. \
         cloud uid={}, x={}, y={}",
        cloud.uid,
        cloud.x,
        cloud.y,
    );

    // The cloud connects the flow's sink endpoint: flow.points.last() is the sink.
    // After settling, the cloud's (x, y) must track the sink flow point.
    // Without the fix the update loop skips the cloud (no entry in state.positions),
    // so the cloud stays at its initial creation position while flow.points shift.
    let sink_point = drain_elem.points.last().expect("flow must have points");
    let cloud_to_sink_dist =
        ((cloud.x - sink_point.x).powi(2) + (cloud.y - sink_point.y).powi(2)).sqrt();
    assert!(
        cloud_to_sink_dist < 5.0,
        "cloud for 'drain' should be at the sink flow point (within 5 units), \
         but was {}px away. cloud=({},{}) sink_point=({},{})",
        cloud_to_sink_dist,
        cloud.x,
        cloud.y,
        sink_point.x,
        sink_point.y,
    );
}

/// When a stock with flows is kind-changed to an aux (via UpsertAux, no
/// DeleteVariable), flows that were attached to the stock must be rebuilt
/// with cloud endpoints. The old stock UID is reused by the new aux element,
/// so the flow endpoint check must detect this as a topology change.
#[test]
fn test_incremental_kind_change_stock_to_aux_resets_attached_flows() {
    let initial_model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            datamodel::Variable::Stock(datamodel::Stock {
                ident: "tank".to_string(),
                equation: datamodel::Equation::Scalar("100".to_string()),
                documentation: String::new(),
                units: None,
                inflows: vec!["fill".to_string()],
                outflows: vec![],
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(1),
            }),
            datamodel::Variable::Flow(datamodel::Flow {
                ident: "fill".to_string(),
                equation: datamodel::Equation::Scalar("10".to_string()),
                documentation: String::new(),
                units: None,
                gf: None,
                compat: datamodel::Compat::default(),
                ai_state: None,
                uid: Some(2),
            }),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
    };

    let initial_project = test_project(initial_model);
    let old_view =
        generate_layout(&initial_project, TEST_MODEL, None).expect("initial layout should succeed");

    // Verify the flow's sink endpoint is attached to the stock
    let fill_flow = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "fill" => Some(f),
            _ => None,
        })
        .expect("fill flow must exist in initial view");
    let tank_uid = old_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name).as_ref() == "tank" => Some(s.uid),
            _ => None,
        })
        .expect("tank stock must exist in initial view");
    assert!(
        fill_flow.points.last().unwrap().attached_to_uid == Some(tank_uid),
        "fill's sink must be attached to tank in initial view"
    );

    // Patch: change stock "tank" to aux "tank" (no DeleteVariable, no explicit flow reset)
    let mut patched_project = initial_project.clone();
    let model = patched_project.get_model_mut(TEST_MODEL).unwrap();
    model.variables.retain(|v| v.get_ident() != "tank");
    model
        .variables
        .push(datamodel::Variable::Aux(datamodel::Aux {
            ident: "tank".to_string(),
            equation: datamodel::Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        }));
    let patch = crate::patch::ModelPatch {
        name: TEST_MODEL.to_string(),
        ops: vec![crate::patch::ModelOperation::UpsertAux(datamodel::Aux {
            ident: "tank".to_string(),
            equation: datamodel::Equation::Scalar("100".to_string()),
            documentation: String::new(),
            units: None,
            gf: None,
            compat: datamodel::Compat::default(),
            ai_state: None,
            uid: Some(1),
        })],
    };

    let new_view = incremental_layout(&old_view, &patched_project, TEST_MODEL, &patch, None)
        .expect("incremental layout after kind change should succeed");

    // The flow "fill" must NOT be attached to the aux element (old tank UID).
    // It should have cloud endpoints since the aux has no inflows/outflows.
    let fill_in_new = new_view
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name).as_ref() == "fill" => Some(f),
            _ => None,
        })
        .expect("fill flow should exist in new view");

    let still_attached_to_tank_uid = fill_in_new
        .points
        .iter()
        .any(|pt| pt.attached_to_uid == Some(tank_uid));
    assert!(
        !still_attached_to_tank_uid,
        "fill flow must NOT be attached to the old tank UID after kind change to aux. \
         The flow should have been rebuilt with cloud endpoints. points: {:?}",
        fill_in_new.points
    );

    // There should be cloud elements for the flow's endpoints
    let fill_clouds: Vec<_> = new_view
        .elements
        .iter()
        .filter(|e| matches!(e, ViewElement::Cloud(c) if c.flow_uid == fill_in_new.uid))
        .collect();
    assert!(
        !fill_clouds.is_empty(),
        "fill flow should have cloud endpoints after stock-to-aux kind change"
    );
}
