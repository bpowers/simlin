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

/// The link drawn from the element named `from` to the one named `to`.
fn link_between(view: &datamodel::StockFlow, from: &str, to: &str) -> Option<view_element::Link> {
    let uid = |name: &str| {
        view.elements.iter().find_map(|e| {
            let n = e.get_name()?;
            (canonicalize(n) == name).then(|| e.get_uid())
        })
    };
    let (f, t) = (uid(from)?, uid(to)?);
    view.elements.iter().find_map(|e| match e {
        ViewElement::Link(l) if l.from_uid == f && l.to_uid == t => Some(l.clone()),
        _ => None,
    })
}

fn center_named(view: &datamodel::StockFlow, name: &str) -> Option<(f64, f64)> {
    view.elements.iter().find_map(|e| match e {
        ViewElement::Aux(a) if canonicalize(&a.name) == name => Some((a.x, a.y)),
        ViewElement::Stock(s) if canonicalize(&s.name) == name => Some((s.x, s.y)),
        ViewElement::Module(m) if canonicalize(&m.name) == name => Some((m.x, m.y)),
        _ => None,
    })
}

#[test]
fn a_variable_whose_kind_changes_is_redrawn_where_it_was() {
    // An agent turns the parameter birth_rate into a stock with an upsert. The
    // stock is drawn where the parameter was, and the link from it into
    // births -- still a dependency -- is the link a person drew.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertStock(stock("birth_rate", &[], &[]))],
    );
    assert!(
        view.elements
            .iter()
            .any(|e| matches!(e, ViewElement::Stock(s) if canonicalize(&s.name) == "birth_rate")),
        "birth_rate is drawn as a stock"
    );
    assert_eq!(
        center_named(&view, "birth_rate"),
        center_named(&base, "birth_rate"),
        "the stock is drawn where the parameter was"
    );
    assert_eq!(
        link_between(&view, "birth_rate", "births"),
        link_between(&base, "birth_rate", "births"),
        "the link keeps its uid and shape"
    );
}

#[test]
fn a_flow_rebuilt_for_a_new_attachment_keeps_its_links() {
    // transfer drains source into sink, at a rate read from source and rate.
    // Deleting sink rebuilds transfer with a cloud end; the links into it are
    // still dependencies, so they keep their uids and their kinds of shape.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("source", &[], &["transfer"])),
        datamodel::Variable::Stock(stock("sink", &["transfer"], &[])),
        datamodel::Variable::Flow(flow("transfer", "source * rate")),
        datamodel::Variable::Aux(aux("rate", "0.1")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "sink".to_string(),
        }],
    );
    for from in ["rate", "source"] {
        let before = link_between(&base, from, "transfer").expect("drawn before");
        let after = link_between(&view, from, "transfer").expect("drawn after");
        assert_eq!(after.uid, before.uid, "{from} -> transfer keeps its uid");
        assert_eq!(
            std::mem::discriminant(&after.shape),
            std::mem::discriminant(&before.shape),
            "{from} -> transfer keeps its kind of shape"
        );
    }
}

fn flow_named(view: &datamodel::StockFlow, name: &str) -> view_element::Flow {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if canonicalize(&f.name) == name => Some(f.clone()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{name} drawn"))
}

/// Every pair of shapes that overlap, where one of them is in `uids`.
fn overlaps_involving(view: &datamodel::StockFlow, uids: &HashSet<i32>) -> Vec<(i32, i32)> {
    use crate::layout::metrics::node_shape_box;
    let shapes: Vec<(i32, crate::diagram::common::Rect)> = view
        .elements
        .iter()
        .filter_map(|e| node_shape_box(e).map(|r| (e.get_uid(), r)))
        .collect();
    let mut out = Vec::new();
    for (i, (a, ra)) in shapes.iter().enumerate() {
        for (b, rb) in &shapes[i + 1..] {
            if !uids.contains(a) && !uids.contains(b) {
                continue;
            }
            let w = ra.right.min(rb.right) - ra.left.max(rb.left);
            let h = ra.bottom.min(rb.bottom) - ra.top.max(rb.top);
            if w > 0.5 && h > 0.5 {
                out.push((*a, *b));
            }
        }
    }
    out
}

/// The strict flow invariant violations of the flows in `uids`.
fn strict_violations(view: &datamodel::StockFlow, uids: &HashSet<i32>) -> String {
    use crate::editing::invariants::{Mode, check_flow_invariants, format_violations};
    format_violations(&check_flow_invariants(
        &view.elements,
        Mode::Strict { routed: Some(uids) },
    ))
}

#[test]
fn a_detached_flow_end_becomes_a_cloud_clear_of_the_stock() {
    // source drains into sink through transfer. An agent restates source
    // without transfer in its outflows, so transfer's source end is now a
    // cloud. The pipe keeps its line, the cloud sits outside source, and what
    // the edit changed covers no shape.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("source", &[], &["transfer"])),
        datamodel::Variable::Stock(stock("sink", &["transfer"], &[])),
        datamodel::Variable::Flow(flow("transfer", "10")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertStock(stock("source", &[], &[]))],
    );
    let before = flow_named(&base, "transfer");
    let after = flow_named(&view, "transfer");
    let source_end = after.points[0].attached_to_uid.expect("attached");
    assert!(
        view.elements.iter().any(
            |e| matches!(e, ViewElement::Cloud(c) if c.uid == source_end && c.flow_uid == after.uid)
        ),
        "the source end is a cloud of transfer's own"
    );
    assert_eq!(
        after.points.last().map(|p| p.attached_to_uid),
        before.points.last().map(|p| p.attached_to_uid),
        "the sink end still attaches to sink"
    );
    let line = before.points[0].y;
    assert!(
        after.points.iter().all(|p| (p.y - line).abs() < 1e-9),
        "the pipe keeps its line: {:?}",
        after.points
    );
    let changed: HashSet<i32> = [after.uid, source_end].into_iter().collect();
    assert_eq!(
        overlaps_involving(&view, &changed),
        Vec::<(i32, i32)>::new()
    );
    assert_eq!(
        strict_violations(&view, &[after.uid].into_iter().collect()),
        ""
    );
}

#[test]
fn deleting_a_middle_stock_leaves_the_flows_through_it_in_place() {
    // upstream -> inflow -> middle -> outflow -> downstream. Deleting middle
    // turns the ends of inflow and outflow that touched it into clouds. The
    // two pipes stay where they were drawn, and nothing the edit changed covers
    // a shape.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("upstream", &[], &["inflow"])),
        datamodel::Variable::Stock(stock("middle", &["inflow"], &["outflow"])),
        datamodel::Variable::Stock(stock("downstream", &["outflow"], &[])),
        datamodel::Variable::Flow(flow("inflow", "10")),
        datamodel::Variable::Flow(flow("outflow", "10")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "middle".to_string(),
        }],
    );
    let points = |f: &view_element::Flow| f.points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>();
    let mut changed: HashSet<i32> = HashSet::new();
    for name in ["inflow", "outflow"] {
        let (before, after) = (flow_named(&base, name), flow_named(&view, name));
        assert_eq!(points(&after), points(&before), "{name}'s pipe stays put");
        changed.insert(after.uid);
        changed.extend(after.points.iter().filter_map(|p| p.attached_to_uid));
    }
    assert_eq!(
        overlaps_involving(&view, &changed),
        Vec::<(i32, i32)>::new()
    );
    let flows: HashSet<i32> = ["inflow", "outflow"]
        .iter()
        .map(|n| flow_named(&view, n).uid)
        .collect();
    assert_eq!(strict_violations(&view, &flows), "");
}

/// The stocks whose interior a flow's pipe passes through, other than its own
/// ends.
fn stocks_crossed(view: &datamodel::StockFlow, flow: &view_element::Flow) -> Vec<String> {
    use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};
    let ends: HashSet<i32> = [flow.points.first(), flow.points.last()]
        .into_iter()
        .flatten()
        .filter_map(|p| p.attached_to_uid)
        .collect();
    let mut out = Vec::new();
    for e in &view.elements {
        let ViewElement::Stock(s) = e else { continue };
        if ends.contains(&s.uid) {
            continue;
        }
        let (hw, hh) = (STOCK_WIDTH / 2.0 - 0.5, STOCK_HEIGHT / 2.0 - 0.5);
        let crosses = flow.points.windows(2).any(|w| {
            // Pipes are orthogonal: a segment enters the interior when it runs
            // within the body's span on its own axis and overlaps it on the
            // other.
            let (a, b) = (&w[0], &w[1]);
            if (a.y - b.y).abs() < 1e-9 {
                (a.y - s.y).abs() < hh && a.x.min(b.x) < s.x + hw && a.x.max(b.x) > s.x - hw
            } else {
                (a.x - s.x).abs() < hw && a.y.min(b.y) < s.y + hh && a.y.max(b.y) > s.y - hh
            }
        });
        if crosses {
            out.push(canonicalize(&s.name).into_owned());
        }
    }
    out
}

#[test]
fn a_flow_created_between_two_stocks_routes_around_the_stocks_between() {
    // upstream -> inflow -> middle -> outflow -> downstream, drawn in a row. An
    // agent adds a bypass from upstream straight to downstream. Its pipe goes
    // around middle rather than through it, holds the flow invariants, and
    // covers no shape.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("upstream", &[], &["inflow"])),
        datamodel::Variable::Stock(stock("middle", &["inflow"], &["outflow"])),
        datamodel::Variable::Stock(stock("downstream", &["outflow"], &[])),
        datamodel::Variable::Flow(flow("inflow", "10")),
        datamodel::Variable::Flow(flow("outflow", "10")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (_, view) = sync(
        &project,
        &base,
        vec![
            ModelOperation::UpsertFlow(flow("bypass", "1")),
            ModelOperation::UpsertStock(stock("upstream", &[], &["inflow", "bypass"])),
            ModelOperation::UpsertStock(stock("downstream", &["outflow", "bypass"], &[])),
        ],
    );
    let bypass = flow_named(&view, "bypass");
    assert_eq!(stocks_crossed(&view, &bypass), Vec::<String>::new());
    let uids: HashSet<i32> = [bypass.uid].into_iter().collect();
    assert_eq!(strict_violations(&view, &uids), "");
    assert_eq!(overlaps_involving(&view, &uids), Vec::<(i32, i32)>::new());
}

#[test]
fn a_created_valve_lands_clear_of_a_parameter() {
    // tank has a parameter, leak_rate, that a person parked just right of it,
    // where a side flow leaving tank's right face puts its valve. An agent adds
    // drain, a flow out of tank at leak_rate: its valve must not land on the
    // parameter.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("tank", &[], &[])),
        datamodel::Variable::Aux(aux("leak_rate", "0.1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let config = LayoutConfig::default();
    let (tx, ty) = center_named(&base, "tank").expect("tank drawn");
    for e in &mut base.elements {
        if let ViewElement::Aux(a) = e {
            (a.x, a.y) = (
                tx + config.stock_width / 2.0 + config.horizontal_spacing / 2.0,
                ty,
            );
        }
    }
    let (_, view) = sync(
        &project,
        &base,
        vec![
            ModelOperation::UpsertFlow(flow("drain", "tank * leak_rate")),
            ModelOperation::UpsertStock(stock("tank", &[], &["drain"])),
        ],
    );
    let drain = flow_named(&view, "drain");
    let uids: HashSet<i32> = [drain.uid].into_iter().collect();
    assert_eq!(overlaps_involving(&view, &uids), Vec::<(i32, i32)>::new());
    assert_eq!(strict_violations(&view, &uids), "");
}

#[test]
fn a_sync_draws_a_missing_connector_only_where_the_edit_is_about_it() {
    // population grows by births at birth_rate, and doubled reads birth_rate.
    // The author's view draws neither births' connectors nor doubled at all.
    // Every arm of what an edit may draw:
    // - an unrelated edit (a new note): births' connectors stay out;
    // - an edit naming births (restating it): its connectors are drawn;
    // - an element drawn for the first time (doubled, drawn because the view
    //   had no element for it): the connector into it is drawn, since a new
    //   element carries no author's choice about its connectors.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
        datamodel::Variable::Aux(aux("doubled", "birth_rate * 2")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let doubled = center_named(&base, "doubled").map(|_| {
        base.elements
            .iter()
            .find(|e| e.get_name().is_some_and(|n| canonicalize(n) == "doubled"))
            .map(ViewElement::get_uid)
            .expect("doubled drawn")
    });
    let births = flow_named(&base, "births").uid;
    base.elements.retain(|e| match e {
        ViewElement::Link(l) => l.to_uid != births && Some(l.to_uid) != doubled,
        other => Some(other.get_uid()) != doubled,
    });

    let (_, unrelated) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertAux(aux("note", "1"))],
    );
    assert!(link_between(&unrelated, "birth_rate", "births").is_none());
    assert!(link_between(&unrelated, "population", "births").is_none());
    assert!(
        link_between(&unrelated, "birth_rate", "doubled").is_some(),
        "doubled is drawn for the first time, with its connector"
    );

    let (_, restated) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertFlow(flow(
            "births",
            "population * birth_rate",
        ))],
    );
    assert!(link_between(&restated, "birth_rate", "births").is_some());
    assert!(link_between(&restated, "population", "births").is_some());
}

#[test]
fn a_stock_added_to_a_drawn_chain_lands_clear_of_side_flows() {
    // tank drains to a cloud off its right face, and a person drew the drain
    // pipe long enough that its cloud sits where a downstream stock would
    // naturally go. An agent adds a reservoir fed from tank: it must not land
    // on the drain's cloud.
    use crate::layout::metrics::node_shape_box;
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("tank", &[], &["drain"])),
        datamodel::Variable::Flow(flow("drain", "tank * 0.1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let drain_cloud = base.elements.iter().find_map(|e| match e {
        ViewElement::Flow(f) if canonicalize(&f.name) == "drain" => {
            f.points.last()?.attached_to_uid
        }
        _ => None,
    });
    for e in &mut base.elements {
        match e {
            ViewElement::Flow(f) if canonicalize(&f.name) == "drain" => {
                f.points.last_mut().expect("points").x += 40.0;
            }
            ViewElement::Cloud(c) if Some(c.uid) == drain_cloud => c.x += 40.0,
            _ => {}
        }
    }
    let ops = vec![
        ModelOperation::UpsertStock(stock("tank", &[], &["drain", "transfer"])),
        ModelOperation::UpsertFlow(flow("transfer", "tank * 0.2")),
        ModelOperation::UpsertStock(stock("reservoir", &["transfer"], &[])),
    ];
    let (_, view) = sync(&project, &base, ops);
    let reservoir = view
        .elements
        .iter()
        .find(|e| e.get_name().is_some_and(|n| canonicalize(n) == "reservoir"))
        .expect("reservoir drawn");
    let r = node_shape_box(reservoir).expect("a stock has a shape");
    let base_uids: HashSet<i32> = base.elements.iter().map(ViewElement::get_uid).collect();
    for e in view
        .elements
        .iter()
        .filter(|e| base_uids.contains(&e.get_uid()))
    {
        let Some(o) = node_shape_box(e) else { continue };
        let w = r.right.min(o.right) - r.left.max(o.left);
        let h = r.bottom.min(o.bottom) - r.top.max(o.top);
        assert!(
            w <= 0.0 || h <= 0.0,
            "reservoir covers #{} ({w:.1} x {h:.1})",
            e.get_uid()
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
