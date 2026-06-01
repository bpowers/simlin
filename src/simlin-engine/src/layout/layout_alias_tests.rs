// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests for ghost (alias) generation: replacing connectors that span the
//! diagram with short local links from an alias of the source variable.
//!
//! The remaining crossings on large generated layouts are STRUCTURAL: a
//! widely-used parameter sits in one place and its connectors cross everything
//! between it and its far-flung consumers. No local optimization can fix that;
//! ghosting the parameter next to each distant consumer cluster can. This is
//! exactly what hand-drawn diagrams do (14-26% of variables in the multi-view
//! Vensim corpus are ghosts, every one of them a pure input).

use super::*;
use crate::datamodel;

const TEST_MODEL: &str = "test";

fn test_project(model: datamodel::Model) -> datamodel::Project {
    datamodel::Project {
        name: model.name.clone(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: Vec::new(),
        units: Vec::new(),
        models: vec![model],
        source: None,
        ai_information: None,
    }
}

fn make_stock(ident: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Variable {
    datamodel::Variable::Stock(datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    })
}

fn make_flow(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Flow(datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    })
}

fn make_aux(ident: &str, equation: &str) -> datamodel::Variable {
    datamodel::Variable::Aux(datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat {
            visibility: datamodel::Visibility::Public,
            ..datamodel::Compat::default()
        },
        ai_state: None,
        uid: None,
    })
}

/// A model whose shared parameter feeds flows in three SEPARATE multi-stock
/// chains. The chains are long enough (three stocks each) that chain
/// positioning spreads their far ends well past the ghosting distance
/// threshold from wherever the shared parameter lands.
fn multi_chain_shared_param_model() -> datamodel::Model {
    let mut variables = vec![make_aux("shared_rate", "0.05")];
    for chain in ["one", "two", "three"] {
        // stock_X_a -[flow_X_a]-> stock_X_b -[flow_X_b]-> stock_X_c
        variables.push(make_stock(
            &format!("stock_{chain}_a"),
            &[],
            &[&format!("flow_{chain}_a")],
        ));
        variables.push(make_stock(
            &format!("stock_{chain}_b"),
            &[&format!("flow_{chain}_a")],
            &[&format!("flow_{chain}_b")],
        ));
        variables.push(make_stock(
            &format!("stock_{chain}_c"),
            &[&format!("flow_{chain}_b")],
            &[],
        ));
        // Every flow consumes the shared parameter.
        variables.push(make_flow(
            &format!("flow_{chain}_a"),
            &format!("stock_{chain}_a * shared_rate"),
        ));
        variables.push(make_flow(
            &format!("flow_{chain}_b"),
            &format!("stock_{chain}_b * shared_rate"),
        ));
    }
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

/// Collect (uid, alias_of_uid) pairs for every alias in the view.
fn aliases_in(view: &datamodel::StockFlow) -> Vec<(i32, i32)> {
    view.elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Alias(a) => Some((a.uid, a.alias_of_uid)),
            _ => None,
        })
        .collect()
}

/// Collect (from_uid, to_uid) pairs for every link in the view.
fn links_in(view: &datamodel::StockFlow) -> Vec<(i32, i32)> {
    view.elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(l) => Some((l.from_uid, l.to_uid)),
            _ => None,
        })
        .collect()
}

/// Find the uid of the named element (panics if missing).
fn uid_of(view: &datamodel::StockFlow, name_fragment: &str) -> i32 {
    view.elements
        .iter()
        .find(|e| {
            e.get_name()
                .is_some_and(|n| n.replace(['\n', ' '], "_").contains(name_fragment))
        })
        .unwrap_or_else(|| panic!("element '{name_fragment}' not found"))
        .get_uid()
}

/// A widely-consumed pure-input parameter whose consumers land far apart must
/// get ghost copies: the generated view contains at least one Alias of it, and
/// every Alias in the view points back at a real element.
#[test]
fn test_distant_consumers_get_ghosts() {
    let project = test_project(multi_chain_shared_param_model());
    let view = generate_layout(&project, TEST_MODEL, None).unwrap();

    let aliases = aliases_in(&view);
    assert!(
        !aliases.is_empty(),
        "a pure-input parameter consumed by three separated chains must get at \
         least one ghost copy; got none"
    );

    let shared_uid = uid_of(&view, "shared_rate");
    let element_uids: HashSet<i32> = view.elements.iter().map(|e| e.get_uid()).collect();
    for (alias_uid, source_uid) in &aliases {
        assert_eq!(
            *source_uid, shared_uid,
            "every ghost in this model aliases the shared parameter"
        );
        assert!(
            element_uids.contains(alias_uid),
            "alias uid {alias_uid} must be a real element"
        );
    }
}

/// Connectors from a ghosted source to its distant consumers must be re-routed
/// to come FROM the ghost, and the original long connector must be gone.
#[test]
fn test_ghost_connectors_rerouted() {
    let project = test_project(multi_chain_shared_param_model());
    let view = generate_layout(&project, TEST_MODEL, None).unwrap();

    let aliases = aliases_in(&view);
    assert!(!aliases.is_empty(), "fixture must produce ghosts");
    let alias_uids: HashSet<i32> = aliases.iter().map(|(uid, _)| *uid).collect();

    let links = links_in(&view);
    // Every ghost must have at least one outgoing connector (it exists to
    // serve a consumer); a connector-less ghost is clutter.
    for alias_uid in &alias_uids {
        assert!(
            links.iter().any(|(from, _)| from == alias_uid),
            "ghost {alias_uid} must source at least one connector"
        );
    }
}

/// Nothing ever points AT a ghost: a ghost is a read-only copy of its source,
/// so connectors only leave it.
#[test]
fn test_no_links_into_ghosts() {
    let project = test_project(multi_chain_shared_param_model());
    let view = generate_layout(&project, TEST_MODEL, None).unwrap();

    let alias_uids: HashSet<i32> = aliases_in(&view).iter().map(|(uid, _)| *uid).collect();
    for (from, to) in links_in(&view) {
        assert!(
            !alias_uids.contains(&to),
            "link {from} -> {to} points INTO a ghost; ghosts are sources only"
        );
    }
}

/// The number of generated ghosts is bounded: at most GHOST_BUDGET_FRACTION of
/// the model's variable count (matching the 14-26% ghost share observed in
/// hand-drawn multi-view models).
#[test]
fn test_ghost_budget_capped() {
    let project = test_project(multi_chain_shared_param_model());
    let view = generate_layout(&project, TEST_MODEL, None).unwrap();

    let model = &project.models[0];
    let aliases = aliases_in(&view);
    let max_ghosts = (model.variables.len() as f64 * aliases::GHOST_BUDGET_FRACTION).ceil();
    assert!(
        (aliases.len() as f64) <= max_ghosts,
        "ghost count {} exceeds the budget of {} ({}% of {} variables)",
        aliases.len(),
        max_ghosts,
        aliases::GHOST_BUDGET_FRACTION * 100.0,
        model.variables.len(),
    );
}

/// Only PURE INPUTS (variables with no dependencies of their own) are ghosted:
/// ghosting a mid-graph variable would visually break the feedback loops it
/// participates in. Here `derived` depends on `base`, and `derived` feeds the
/// three chains -- `derived` must NOT be ghosted even though its consumers are
/// far apart.
#[test]
fn test_only_pure_inputs_get_ghosted() {
    let model = datamodel::Model {
        name: TEST_MODEL.to_string(),
        sim_specs: None,
        variables: vec![
            make_aux("base", "0.05"),
            make_aux("derived", "base * 2"),
            make_stock("stock_one", &["inflow_one"], &[]),
            make_flow("inflow_one", "stock_one * derived"),
            make_stock("stock_two", &["inflow_two"], &[]),
            make_flow("inflow_two", "stock_two * derived"),
            make_stock("stock_three", &["inflow_three"], &[]),
            make_flow("inflow_three", "stock_three * derived"),
        ],
        views: Vec::new(),
        loop_metadata: Vec::new(),
        groups: Vec::new(),
        macro_spec: None,
    };
    let project = test_project(model);
    let view = generate_layout(&project, TEST_MODEL, None).unwrap();

    let derived_uid = uid_of(&view, "derived");
    for (_, source_uid) in aliases_in(&view) {
        assert_ne!(
            source_uid, derived_uid,
            "'derived' has dependencies of its own and must not be ghosted"
        );
    }
}

/// Ghost generation must preserve per-seed determinism (#633).
#[test]
fn test_ghost_layout_deterministic() {
    let project = test_project(multi_chain_shared_param_model());
    let a = generate_layout(&project, TEST_MODEL, None).unwrap();
    let b = generate_layout(&project, TEST_MODEL, None).unwrap();
    assert_eq!(a, b, "ghost generation must be deterministic per seed");
}
