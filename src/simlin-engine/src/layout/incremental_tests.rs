// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::layout::metrics::compute_layout_metrics;
use crate::patch::{ModelOperation, ModelPatch};

const TEST_MODEL: &str = "main";

fn project_with(variables: Vec<datamodel::Variable>) -> datamodel::Project {
    datamodel::Project {
        name: "test".to_string(),
        sim_specs: datamodel::SimSpecs::default(),
        dimensions: Vec::new(),
        units: Vec::new(),
        models: vec![datamodel::Model {
            name: TEST_MODEL.to_string(),
            sim_specs: None,
            variables,
            views: Vec::new(),
            loop_metadata: Vec::new(),
            groups: Vec::new(),
            macro_spec: None,
        }],
        source: None,
        ai_information: None,
    }
}

fn stock(ident: &str, inflows: &[&str], outflows: &[&str]) -> datamodel::Stock {
    datamodel::Stock {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar("100".to_string()),
        documentation: String::new(),
        units: None,
        inflows: inflows.iter().map(|s| s.to_string()).collect(),
        outflows: outflows.iter().map(|s| s.to_string()).collect(),
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    }
}

fn flow(ident: &str, equation: &str) -> datamodel::Flow {
    datamodel::Flow {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        compat: datamodel::Compat::default(),
        ai_state: None,
        uid: None,
    }
}

fn aux(ident: &str, equation: &str) -> datamodel::Aux {
    datamodel::Aux {
        ident: ident.to_string(),
        equation: datamodel::Equation::Scalar(equation.to_string()),
        documentation: String::new(),
        units: None,
        gf: None,
        ai_state: None,
        uid: None,
        compat: datamodel::Compat::default(),
    }
}

/// Apply `ops` to `project` holding `old_view`, and sync the view through
/// `incremental_layout`, the way MCP `edit_model` (`sync_diagram`) and
/// libsimlin's patch sync do: the patch is applied to a project whose model
/// already carries the view, so the uids it mints for new variables are past
/// every uid the view uses.
fn sync(
    project: &datamodel::Project,
    old_view: &datamodel::StockFlow,
    ops: Vec<ModelOperation>,
) -> (datamodel::Project, datamodel::StockFlow) {
    let patch = ModelPatch {
        name: TEST_MODEL.to_string(),
        ops,
    };
    let mut patched = project.clone();
    patched.get_model_mut(TEST_MODEL).expect("model").views =
        vec![datamodel::View::StockFlow(old_view.clone())];
    crate::patch::apply_patch(
        &mut patched,
        crate::patch::ProjectPatch {
            project_ops: vec![],
            models: vec![patch.clone()],
        },
    )
    .expect("patch applies");
    let view = incremental_layout(old_view, &patched, TEST_MODEL, &patch, None)
        .expect("incremental layout");
    (patched, view)
}

/// `(x, y, label side)` of every named element.
fn geometry(view: &datamodel::StockFlow) -> HashMap<i32, (f64, f64, LabelSide)> {
    view.elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y, s.label_side))),
            ViewElement::Flow(f) => Some((f.uid, (f.x, f.y, f.label_side))),
            ViewElement::Aux(a) => Some((a.uid, (a.x, a.y, a.label_side))),
            ViewElement::Module(m) => Some((m.uid, (m.x, m.y, m.label_side))),
            _ => None,
        })
        .collect()
}

fn default_project(name: &str) -> datamodel::Project {
    let path = format!(
        "{}/../../default_projects/{name}/model.xmile",
        env!("CARGO_MANIFEST_DIR")
    );
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    crate::compat::open_xmile(&mut std::io::BufReader::new(file)).expect("model imports")
}

fn shipped_view(project: &datamodel::Project) -> datamodel::StockFlow {
    match project.get_model(TEST_MODEL).and_then(|m| m.views.first()) {
        Some(datamodel::View::StockFlow(sf)) => sf.clone(),
        None => panic!("the project ships no view"),
    }
}

#[test]
fn an_edit_that_changes_no_structure_returns_the_view_byte_for_byte() {
    // An agent restates a flow exactly as it is. Nothing about the diagram
    // changed, so the view comes back as it was -- element order included,
    // since the order is the draw order and what the saved file lists.
    // Fishbanks' hand-drawn view has several links and clouds, so a sync that
    // re-lists connectors or clouds shows up.
    let project = default_project("fishbanks");
    let view = shipped_view(&project);
    let harvest = project
        .get_model(TEST_MODEL)
        .and_then(|m| m.get_variable("harvest_rate"))
        .cloned()
        .expect("harvest_rate");
    let datamodel::Variable::Flow(harvest) = harvest else {
        panic!("harvest_rate is a flow");
    };
    let (_, synced) = sync(&project, &view, vec![ModelOperation::UpsertFlow(harvest)]);
    let order =
        |v: &datamodel::StockFlow| v.elements.iter().map(|e| e.get_uid()).collect::<Vec<_>>();
    assert_eq!(order(&synced), order(&view), "element order");
    assert!(synced == view, "the restated view must equal the original");
}

#[test]
fn syncing_one_edit_twice_produces_one_view() {
    // One edit creates three side flows, each ending at a cloud, plus the
    // links their rates read. Every sync of it must list the created clouds
    // and links in the same order, however the sync's maps hash.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * 0.03")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let ops = vec![
        ModelOperation::UpsertStock(stock(
            "population",
            &["births"],
            &["deaths", "emigration", "retirement"],
        )),
        ModelOperation::UpsertFlow(flow("deaths", "population * 0.01")),
        ModelOperation::UpsertFlow(flow("emigration", "population * 0.02")),
        ModelOperation::UpsertFlow(flow("retirement", "population * 0.005")),
    ];
    let (_, first) = sync(&project, &base, ops.clone());
    for _ in 0..12 {
        let (_, again) = sync(&project, &base, ops.clone());
        assert!(
            again == first,
            "a second sync of one edit must equal the first"
        );
    }
}

/// The shape of the link drawn from the element named `from` to the one named
/// `to`, if one is drawn.
fn link_shape(view: &datamodel::StockFlow, from: &str, to: &str) -> Option<LinkShape> {
    let uid = |name: &str| {
        view.elements.iter().find_map(|e| {
            let n = e.get_name()?;
            (canonicalize(n) == name).then(|| e.get_uid())
        })
    };
    let (f, t) = (uid(from)?, uid(to)?);
    view.elements.iter().find_map(|e| match e {
        ViewElement::Link(l) if l.from_uid == f && l.to_uid == t => Some(l.shape.clone()),
        _ => None,
    })
}

#[test]
fn only_the_links_a_sync_creates_are_curved_for_a_loop() {
    // An agent makes birth_rate read population, closing the loop population
    // -> birth_rate -> births -> population. The diagram's links were drawn
    // straight by hand. The link the edit creates is curved as a loop link,
    // and the two links it did not create keep the shapes a person chose.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    for e in &mut base.elements {
        if let ViewElement::Link(l) = e {
            l.shape = LinkShape::Straight;
        }
    }
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertAux(aux(
            "birth_rate",
            "0.1 * (1 - population / 1000)",
        ))],
    );
    assert!(
        matches!(
            link_shape(&view, "population", "birth_rate"),
            Some(LinkShape::Arc(_))
        ),
        "the link the edit created on the loop is curved"
    );
    for (from, to) in [("birth_rate", "births"), ("population", "births")] {
        assert!(
            matches!(link_shape(&view, from, to), Some(LinkShape::Straight)),
            "the link {from} -> {to} the edit did not create keeps its straight shape"
        );
    }
}

#[test]
fn new_parameters_are_decluttered_around_the_fixed_diagram() {
    // Six new parameters that all feed one existing flow are seeded in a tight
    // ring beside it. Their names must not land on each other or on anything
    // already drawn, while every element that was already drawn keeps its
    // position and label side.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &[], &["deaths"])),
        datamodel::Variable::Flow(flow("deaths", "population * 0.1")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let params: Vec<String> = (1..=6)
        .map(|i| format!("mortality adjustment parameter {i}"))
        .collect();
    let mut ops: Vec<ModelOperation> = params
        .iter()
        .map(|p| ModelOperation::UpsertAux(aux(p, "0.1")))
        .collect();
    let sum = params
        .iter()
        .map(|p| canonicalize(p).into_owned())
        .collect::<Vec<_>>()
        .join(" + ");
    ops.push(ModelOperation::UpsertFlow(flow(
        "deaths",
        &format!("population * ({sum}) / 6"),
    )));
    let (_, view) = sync(&project, &base, ops);

    let m = compute_layout_metrics(&view, &LayoutConfig::default());
    assert_eq!(
        m.node_overlap, 0.0,
        "new parameters must not cover any shape"
    );
    assert_eq!(m.label_overlap, 0.0, "new names must not cover anything");

    let before = geometry(&base);
    let after = geometry(&view);
    for (uid, g) in &before {
        assert_eq!(after.get(uid), Some(g), "element {uid} must stay put");
    }
}

#[test]
fn chains_added_whole_are_laid_out_as_chains_beside_the_diagram() {
    // An agent adds two whole stock-flow chains to a diagram in one edit: a
    // two-stock capital chain and a one-stock pollution chain. Each must be
    // drawn as a fresh layout draws a chain -- stocks in a row, pipes straight
    // -- in free space, not piled onto the existing chain or each other.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &["deaths"])),
        datamodel::Variable::Flow(flow("births", "population * 0.03")),
        datamodel::Variable::Flow(flow("deaths", "population * 0.02")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let ops = vec![
        ModelOperation::UpsertStock(stock("capital", &["investment"], &["retirement"])),
        ModelOperation::UpsertStock(stock("retired capital", &["retirement"], &["scrapping"])),
        ModelOperation::UpsertFlow(flow("investment", "10")),
        ModelOperation::UpsertFlow(flow("retirement", "capital / 20")),
        ModelOperation::UpsertFlow(flow("scrapping", "retired_capital / 5")),
        ModelOperation::UpsertStock(stock("pollution", &["emissions"], &["absorption"])),
        ModelOperation::UpsertFlow(flow("emissions", "capital * 0.1")),
        ModelOperation::UpsertFlow(flow("absorption", "pollution / 10")),
    ];
    let (_, view) = sync(&project, &base, ops);

    let m = compute_layout_metrics(&view, &LayoutConfig::default());
    assert_eq!(m.node_overlap, 0.0, "no shape may cover another");
    assert_eq!(m.label_overlap, 0.0, "no name may be covered");
    assert_eq!(m.flow_bends, 0.0, "every pipe of a chain is straight");

    let stock_y = |name: &str| {
        view.elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Stock(s) if canonicalize(&s.name) == name => Some(s.y),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} drawn"))
    };
    assert_eq!(
        stock_y("capital"),
        stock_y("retired_capital"),
        "a chain's stocks share a row"
    );

    let before = geometry(&base);
    let after = geometry(&view);
    for (uid, g) in &before {
        assert_eq!(after.get(uid), Some(g), "element {uid} must stay put");
    }
}

#[test]
fn a_stock_added_to_a_drawn_chain_continues_its_row() {
    // An agent extends a drawn chain: infected now drains into a new recovered
    // stock. The new stock belongs one chain step past infected, in the same
    // row, with a straight pipe between them -- not parked off to the side
    // with a bent pipe reaching for it.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("susceptible", &[], &["infection"])),
        datamodel::Variable::Stock(stock("infected", &["infection"], &[])),
        datamodel::Variable::Flow(flow("infection", "susceptible * infected / 1000")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let ops = vec![
        ModelOperation::UpsertStock(stock("infected", &["infection"], &["recovery"])),
        ModelOperation::UpsertStock(stock("recovered", &["recovery"], &[])),
        ModelOperation::UpsertFlow(flow("recovery", "infected / 10")),
    ];
    let (_, view) = sync(&project, &base, ops);

    let m = compute_layout_metrics(&view, &LayoutConfig::default());
    assert_eq!(m.node_overlap, 0.0, "no shape may cover another");
    assert_eq!(m.flow_bends, 0.0, "the new pipe runs straight");

    let stock_at = |name: &str| {
        view.elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Stock(s) if canonicalize(&s.name) == name => Some((s.x, s.y)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} drawn"))
    };
    let (infected_x, infected_y) = stock_at("infected");
    let (recovered_x, recovered_y) = stock_at("recovered");
    assert_eq!(recovered_y, infected_y, "the chain's row continues");
    assert!(
        recovered_x > infected_x,
        "the downstream stock is to the right"
    );

    let before = geometry(&base);
    let after = geometry(&view);
    for (uid, g) in &before {
        assert_eq!(after.get(uid), Some(g), "element {uid} must stay put");
    }
}
