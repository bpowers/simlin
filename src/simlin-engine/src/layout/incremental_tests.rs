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
    // population grows by births at birth_rate; doubled reads birth_rate and
    // quad reads doubled. The author's view draws neither births' connectors
    // nor doubled at all. Every arm of what an edit may draw:
    // - an unrelated edit (a new note): births' connectors stay out, and so
    //   does doubled, a variable the author left undrawn;
    // - an edit naming births (restating it): its connectors are drawn;
    // - an edit naming doubled (restating it): doubled is drawn with the
    //   connector into it, and the connector from it to quad too, although
    //   quad is not named, since an element drawn for the first time carries
    //   no author's choice about its connectors.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
        datamodel::Variable::Aux(aux("doubled", "birth_rate * 2")),
        datamodel::Variable::Aux(aux("quad", "doubled * 2")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let doubled = base
        .elements
        .iter()
        .find(|e| e.get_name().is_some_and(|n| canonicalize(n) == "doubled"))
        .map(ViewElement::get_uid)
        .expect("doubled drawn");
    let births = flow_named(&base, "births").uid;
    base.elements.retain(|e| match e {
        ViewElement::Link(l) => l.to_uid != births && l.to_uid != doubled && l.from_uid != doubled,
        other => other.get_uid() != doubled,
    });
    let drawn = |view: &datamodel::StockFlow, name: &str| {
        view.elements
            .iter()
            .any(|e| e.get_name().is_some_and(|n| canonicalize(n) == name))
    };

    let (_, unrelated) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertAux(aux("note", "1"))],
    );
    assert!(link_between(&unrelated, "birth_rate", "births").is_none());
    assert!(link_between(&unrelated, "population", "births").is_none());
    assert!(
        !drawn(&unrelated, "doubled"),
        "a variable the author left undrawn stays undrawn"
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
    assert!(!drawn(&restated, "doubled"));

    let (_, named) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertAux(aux("doubled", "birth_rate * 2"))],
    );
    assert!(
        drawn(&named, "doubled"),
        "a variable the patch names is drawn"
    );
    assert!(link_between(&named, "birth_rate", "doubled").is_some());
    assert!(
        link_between(&named, "doubled", "quad").is_some(),
        "doubled is drawn for the first time, with its connector to quad"
    );
}

#[test]
fn a_link_drawing_no_dependency_goes_only_with_an_edit_to_its_reader() {
    // An imported view draws note_rate -> births, which no equation explains
    // (a module port, an input the extraction does not see, an annotation).
    // Every arm of whether a sync keeps it:
    // - an unrelated edit (a new note): kept, byte for byte;
    // - an edit to the link's source (restating note_rate): kept;
    // - an edit to its reader (restating births): dropped, since the reader's
    //   equation is what says which connectors into it are drawn.
    const EXTRA: i32 = 90_000;
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
        datamodel::Variable::Aux(aux("note_rate", "0.5")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let note_rate = base
        .elements
        .iter()
        .find(|e| e.get_name().is_some_and(|n| canonicalize(n) == "note_rate"))
        .map(ViewElement::get_uid)
        .expect("note_rate drawn");
    let births = flow_named(&base, "births").uid;
    let link = ViewElement::Link(view_element::Link {
        uid: EXTRA,
        from_uid: note_rate,
        to_uid: births,
        shape: LinkShape::Arc(30.0),
        polarity: None,
    });
    base.elements.push(link.clone());
    let kept =
        |view: &datamodel::StockFlow| view.elements.iter().find(|e| e.get_uid() == EXTRA).cloned();

    let (_, unrelated) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertAux(aux("note", "1"))],
    );
    assert_eq!(
        kept(&unrelated),
        Some(link.clone()),
        "an unrelated edit keeps it"
    );

    let (_, source) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertAux(aux("note_rate", "0.6"))],
    );
    assert_eq!(kept(&source), Some(link), "an edit to its source keeps it");

    let (_, reader) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertFlow(flow(
            "births",
            "population * birth_rate",
        ))],
    );
    assert_eq!(kept(&reader), None, "an edit to its reader drops it");
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

#[test]
fn deleting_a_variable_removes_the_links_of_its_aliases() {
    // An imported view draws birth_rate a second time, as an alias beside
    // births, with the connector from the alias. Deleting birth_rate (and
    // nothing else, so births is not an edit the link's reader is named by)
    // removes the alias and every link touching it, so nothing references a
    // uid the view no longer draws.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &[])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let birth_rate = base
        .elements
        .iter()
        .find(|e| {
            e.get_name()
                .is_some_and(|n| canonicalize(n) == "birth_rate")
        })
        .map(ViewElement::get_uid)
        .expect("birth_rate drawn");
    let births = flow_named(&base, "births").uid;
    let (alias, link) = (90_000, 90_001);
    base.elements.push(ViewElement::Alias(view_element::Alias {
        uid: alias,
        alias_of_uid: birth_rate,
        x: 400.0,
        y: 400.0,
        label_side: LabelSide::Bottom,
        compat: None,
    }));
    base.elements.push(ViewElement::Link(view_element::Link {
        uid: link,
        from_uid: alias,
        to_uid: births,
        shape: LinkShape::Straight,
        polarity: None,
    }));
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "birth_rate".to_string(),
        }],
    );
    let left: Vec<i32> = view
        .elements
        .iter()
        .map(ViewElement::get_uid)
        .filter(|uid| [alias, link].contains(uid))
        .collect();
    assert_eq!(left, Vec::<i32>::new(), "the alias and its link are gone");
}

#[test]
fn clouds_of_flows_through_a_deleted_stock_land_clear_of_each_other() {
    // middle takes arrivals in on one face and sends departures and losses
    // out of two others. Deleting middle turns the three ends that touched it
    // into clouds; left at the old faces' points, the clouds on perpendicular
    // faces cover each other. Each slides back along its own pipe instead.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("middle", &["arrivals"], &["departures", "losses"])),
        datamodel::Variable::Flow(flow("arrivals", "1")),
        datamodel::Variable::Flow(flow("departures", "1")),
        datamodel::Variable::Flow(flow("losses", "1")),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "middle".to_string(),
        }],
    );
    let mut changed: HashSet<i32> = HashSet::new();
    let mut flows: HashSet<i32> = HashSet::new();
    for name in ["arrivals", "departures", "losses"] {
        let f = flow_named(&view, name);
        flows.insert(f.uid);
        changed.insert(f.uid);
        changed.extend(f.points.iter().filter_map(|p| p.attached_to_uid));
    }
    assert_eq!(
        overlaps_involving(&view, &changed),
        Vec::<(i32, i32)>::new()
    );
    assert_eq!(strict_violations(&view, &flows), "");
}

#[test]
fn a_created_side_flow_cloud_lands_clear_of_a_stock() {
    // A person parked reservoir where a side flow out of tank puts its cloud.
    // An agent adds drain out of tank: its cloud must not land on reservoir.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("tank", &[], &[])),
        datamodel::Variable::Stock(stock("reservoir", &[], &[])),
    ]);
    let base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let ops = || {
        vec![
            ModelOperation::UpsertFlow(flow("drain", "1")),
            ModelOperation::UpsertStock(stock("tank", &[], &["drain"])),
        ]
    };
    let (_, probe) = sync(&project, &base, ops());
    let sink = flow_named(&probe, "drain")
        .points
        .last()
        .map(|p| (p.x, p.y))
        .expect("drain drawn");
    let mut parked = base.clone();
    let reservoir = parked
        .elements
        .iter_mut()
        .find_map(|e| match e {
            ViewElement::Stock(s) if canonicalize(&s.name) == "reservoir" => Some(s),
            _ => None,
        })
        .expect("reservoir drawn");
    (reservoir.x, reservoir.y) = sink;
    let reservoir = reservoir.uid;
    assert_eq!(
        overlaps_involving(&parked, &[reservoir].into_iter().collect()),
        Vec::<(i32, i32)>::new(),
        "fixture: reservoir is parked clear of tank"
    );

    let (_, view) = sync(&project, &parked, ops());
    let drain = flow_named(&view, "drain");
    let mut changed: HashSet<i32> = [drain.uid].into_iter().collect();
    changed.extend(drain.points.iter().filter_map(|p| p.attached_to_uid));
    changed.remove(&uid_named(&view, "tank"));
    assert_eq!(
        overlaps_involving(&view, &changed),
        Vec::<(i32, i32)>::new()
    );
    assert_eq!(
        strict_violations(&view, &[drain.uid].into_iter().collect()),
        ""
    );
}

fn uid_named(view: &datamodel::StockFlow, name: &str) -> i32 {
    view.elements
        .iter()
        .find(|e| e.get_name().is_some_and(|n| canonicalize(n) == name))
        .map(ViewElement::get_uid)
        .unwrap_or_else(|| panic!("{name} drawn"))
}

#[test]
fn a_created_parameter_left_on_a_shape_moves_to_the_nearest_clear_spot() {
    // Rows over the pass's arms, on elements in the shape incremental layout
    // hands it after the declutter: a created parameter still on a shape moves
    // to the nearest clear position; one clear of every shape stays; an
    // element the pass did not create stays where a person put it, overlap
    // included. The composition through production is pinned by the battery's
    // catastrophe fixture, where an inserted intermediate landed on an alias
    // the jammed relaxation could not clear.
    use crate::diagram::constants::AUX_RADIUS;
    let aux = |uid: i32, x: f64| {
        ViewElement::Aux(view_element::Aux {
            name: format!("a{uid}"),
            uid,
            x,
            y: 100.0,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    };
    let alias = ViewElement::Alias(view_element::Alias {
        uid: 1,
        alias_of_uid: 99,
        x: 200.0,
        y: 100.0,
        label_side: LabelSide::Bottom,
        compat: None,
    });
    let center = |elements: &[ViewElement]| {
        elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Aux(a) => Some((a.x, a.y)),
                _ => None,
            })
            .expect("the parameter")
    };

    let mut elements = vec![alias.clone(), aux(7, 205.0)];
    keep_created_nodes_clear(&mut elements, |uid| uid == 7);
    assert_eq!(
        center(&elements),
        (205.0 + 2.0 * AUX_RADIUS, 100.0),
        "a created parameter on a shape takes the nearest clear ring"
    );

    let mut elements = vec![alias.clone(), aux(7, 260.0)];
    keep_created_nodes_clear(&mut elements, |uid| uid == 7);
    assert_eq!(
        center(&elements),
        (260.0, 100.0),
        "clear of every shape: it stays"
    );

    let mut elements = vec![alias, aux(7, 205.0)];
    keep_created_nodes_clear(&mut elements, |_| false);
    assert_eq!(
        center(&elements),
        (205.0, 100.0),
        "not created by the pass: it stays"
    );
}

#[test]
fn clouds_of_flows_leaving_a_deleted_stock_at_one_point_separate() {
    // An imported view (thyroid's plasma T4) draws two flows leaving middle
    // from the same point of its top face, rising together a short way before
    // turning apart to a and b. Deleting middle turns both ends into clouds at
    // that one point; each slides along its own pipe, past the shared rise,
    // until its cloud covers nothing.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("a", &["to_a"], &[])),
        datamodel::Variable::Stock(stock("middle", &[], &["to_a", "to_b"])),
        datamodel::Variable::Stock(stock("b", &["to_b"], &[])),
        datamodel::Variable::Flow(flow("to_a", "1")),
        datamodel::Variable::Flow(flow("to_b", "1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let uid = |view: &datamodel::StockFlow, name: &str| {
        view.elements
            .iter()
            .find(|e| e.get_name().is_some_and(|n| canonicalize(n) == name))
            .map(ViewElement::get_uid)
            .unwrap_or_else(|| panic!("{name} drawn"))
    };
    let (a, middle, b) = (uid(&base, "a"), uid(&base, "middle"), uid(&base, "b"));
    let point = |x: f64, y: f64, attached: Option<i32>| view_element::FlowPoint {
        x,
        y,
        attached_to_uid: attached,
    };
    for e in &mut base.elements {
        match e {
            ViewElement::Stock(s) if s.uid == a => (s.x, s.y) = (325.0, 595.0),
            ViewElement::Stock(s) if s.uid == middle => (s.x, s.y) = (610.0, 595.0),
            ViewElement::Stock(s) if s.uid == b => (s.x, s.y) = (920.0, 595.0),
            ViewElement::Flow(f) if canonicalize(&f.name) == "to_a" => {
                f.points = vec![
                    point(610.0, 577.5, Some(middle)),
                    point(610.0, 540.0, None),
                    point(325.0, 540.0, None),
                    point(325.0, 577.5, Some(a)),
                ];
                (f.x, f.y) = (465.0, 540.0);
            }
            ViewElement::Flow(f) if canonicalize(&f.name) == "to_b" => {
                f.points = vec![
                    point(610.0, 577.5, Some(middle)),
                    point(610.0, 539.0, None),
                    point(920.0, 539.0, None),
                    point(920.0, 577.5, Some(b)),
                ];
                (f.x, f.y) = (770.0, 539.0);
            }
            _ => {}
        }
    }
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "middle".to_string(),
        }],
    );
    let mut changed: HashSet<i32> = HashSet::new();
    let mut flows: HashSet<i32> = HashSet::new();
    for name in ["to_a", "to_b"] {
        let f = flow_named(&view, name);
        flows.insert(f.uid);
        changed.insert(f.uid);
        changed.extend(f.points.iter().filter_map(|p| p.attached_to_uid));
    }
    changed.remove(&a);
    changed.remove(&b);
    assert_eq!(
        overlaps_involving(&view, &changed),
        Vec::<(i32, i32)>::new()
    );
    assert_eq!(strict_violations(&view, &flows), "");
}

/// transfer drains source into sink, its valve drawn just off source's face,
/// with note parked at the pipe's middle: the view `a_reattached_flow_*` tests
/// edit.
fn reattached_valve_fixture(transfer_equation: &str) -> (datamodel::Project, datamodel::StockFlow) {
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("source", &[], &["transfer"])),
        datamodel::Variable::Stock(stock("sink", &["transfer"], &[])),
        datamodel::Variable::Flow(flow("transfer", transfer_equation)),
        datamodel::Variable::Aux(aux("note", "1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (source, sink) = (uid_named(&base, "source"), uid_named(&base, "sink"));
    let point = |x: f64, y: f64, attached: Option<i32>| view_element::FlowPoint {
        x,
        y,
        attached_to_uid: attached,
    };
    for e in &mut base.elements {
        match e {
            ViewElement::Stock(s) if s.uid == source => (s.x, s.y) = (100.0, 100.0),
            ViewElement::Stock(s) if s.uid == sink => (s.x, s.y) = (400.0, 100.0),
            ViewElement::Aux(a) if canonicalize(&a.name) == "note" => (a.x, a.y) = (250.0, 100.0),
            ViewElement::Flow(f) if canonicalize(&f.name) == "transfer" => {
                f.points = vec![
                    point(122.5, 100.0, Some(source)),
                    point(377.5, 100.0, Some(sink)),
                ];
                (f.x, f.y) = (135.0, 100.0);
            }
            _ => {}
        }
    }
    (project, base)
}

#[test]
fn a_reattached_flow_valve_lands_clear_of_a_parameter() {
    // Restating source without transfer turns that end into a cloud, which
    // covers the valve; the valve moves, but not onto note, and the pipe stays
    // on sink's face and its new cloud. Rows over the two ways the pass runs:
    // the edit alone creates no element, and with a parameter added the pass
    // also places and settles a created element, which must not drag the
    // re-attached flow back to where its valve sat before it slid.
    let (project, base) = reattached_valve_fixture("10");
    let sink = uid_named(&base, "sink");
    for (row, extra) in [
        ("the re-attachment alone", vec![]),
        (
            "with a created parameter",
            vec![ModelOperation::UpsertAux(aux("extra", "1"))],
        ),
    ] {
        let mut ops = vec![ModelOperation::UpsertStock(stock("source", &[], &[]))];
        ops.extend(extra);
        let (_, view) = sync(&project, &base, ops);
        let transfer = flow_named(&view, "transfer");
        let mut changed: HashSet<i32> = [transfer.uid].into_iter().collect();
        changed.extend(transfer.points.iter().filter_map(|p| p.attached_to_uid));
        changed.remove(&sink);
        assert_eq!(
            overlaps_involving(&view, &changed),
            Vec::<(i32, i32)>::new(),
            "{row}"
        );
        assert_eq!(
            strict_violations(&view, &[transfer.uid].into_iter().collect()),
            "",
            "{row}"
        );
    }
}

#[test]
fn a_variable_redrawn_as_a_larger_shape_moves_off_its_neighbour() {
    // a and b are parameters a person drew 30 px apart. Turning a into a stock
    // redraws it at its old center, where a stock's body covers b; it moves to
    // the nearest spot clear of every shape, and b stays where it was.
    let project = project_with(vec![
        datamodel::Variable::Aux(aux("a", "1")),
        datamodel::Variable::Aux(aux("b", "2")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    for e in &mut base.elements {
        match e {
            ViewElement::Aux(x) if canonicalize(&x.name) == "a" => (x.x, x.y) = (100.0, 100.0),
            ViewElement::Aux(x) if canonicalize(&x.name) == "b" => (x.x, x.y) = (130.0, 100.0),
            _ => {}
        }
    }
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::UpsertStock(stock("a", &[], &[]))],
    );
    let a = uid_named(&view, "a");
    assert_eq!(
        overlaps_involving(&view, &[a].into_iter().collect()),
        Vec::<(i32, i32)>::new()
    );
    assert_eq!(
        center_named(&view, "b"),
        Some((130.0, 100.0)),
        "b stays put"
    );
    let (x, y) = center_named(&view, "a").expect("a drawn");
    assert!(
        (x - 100.0).hypot(y - 100.0) <= crate::diagram::constants::STOCK_WIDTH,
        "a moves only as far as it must: ({x}, {y})"
    );
}

#[test]
fn a_cloud_left_by_a_deleted_stock_steps_off_a_parameter_drawn_on_its_pipe() {
    // Industrial dynamics draws an alias on the short pipe between a stock's
    // bottom face and the valve of the flow draining it. Deleting the stock
    // turns that end into a cloud, and no position short of the valve clears
    // the alias; the space the stock took is free, so the end extends into it
    // instead, as little as clears.
    let project = project_with(vec![
        datamodel::Variable::Stock(stock("tank", &[], &["drain"])),
        datamodel::Variable::Flow(flow("drain", "1")),
        datamodel::Variable::Aux(aux("marker", "1")),
    ]);
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let tank = uid_named(&base, "tank");
    let point = |x: f64, y: f64, attached: Option<i32>| view_element::FlowPoint {
        x,
        y,
        attached_to_uid: attached,
    };
    let cloud = base
        .elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Cloud(c) => Some(c.uid),
            _ => None,
        })
        .expect("drain's cloud");
    for e in &mut base.elements {
        match e {
            ViewElement::Stock(s) if s.uid == tank => (s.x, s.y) = (500.0, 1262.0),
            ViewElement::Aux(a) if canonicalize(&a.name) == "marker" => {
                (a.x, a.y) = (499.0, 1301.0)
            }
            ViewElement::Flow(f) if canonicalize(&f.name) == "drain" => {
                f.points = vec![
                    point(499.0, 1279.5, Some(tank)),
                    point(499.0, 1387.0, Some(cloud)),
                ];
                (f.x, f.y) = (499.0, 1344.0);
            }
            ViewElement::Cloud(c) if c.uid == cloud => (c.x, c.y) = (499.0, 1387.0),
            _ => {}
        }
    }
    let (_, view) = sync(
        &project,
        &base,
        vec![ModelOperation::DeleteVariable {
            ident: "tank".to_string(),
        }],
    );
    let drain = flow_named(&view, "drain");
    let mut changed: HashSet<i32> = [drain.uid].into_iter().collect();
    changed.extend(drain.points.iter().filter_map(|p| p.attached_to_uid));
    assert_eq!(
        overlaps_involving(&view, &changed),
        Vec::<(i32, i32)>::new()
    );
    assert_eq!(
        strict_violations(&view, &[drain.uid].into_iter().collect()),
        ""
    );
}

#[test]
fn a_sync_draws_nothing_that_references_an_undrawn_variable() {
    // population grows by births at birth_rate and drains by emigration; quad
    // reads doubled, which reads birth_rate. The author's view leaves out
    // doubled and emigration. Every variable has a uid, as in a project MCP
    // opened (`simlin-mcp-core`'s `ensure_variable_uids` mints the missing
    // ones), so the connector and cloud diffs can name the undrawn variables
    // by uid. An edit naming neither must not draw a connector into or out of
    // doubled, or a cloud of emigration: nothing is drawn at the other end,
    // so the link or cloud would reference no element. Rows over the two ways
    // the pass runs: an edit that creates an element, and one that creates
    // none.
    let mut project = project_with(vec![
        datamodel::Variable::Stock(stock("population", &["births"], &["emigration"])),
        datamodel::Variable::Flow(flow("births", "population * birth_rate")),
        datamodel::Variable::Flow(flow("emigration", "population * 0.01")),
        datamodel::Variable::Aux(aux("birth_rate", "0.1")),
        datamodel::Variable::Aux(aux("doubled", "birth_rate * 2")),
        datamodel::Variable::Aux(aux("quad", "doubled * 2")),
    ]);
    let model = project.get_model_mut(TEST_MODEL).expect("model");
    for (uid, var) in (1..).zip(model.variables.iter_mut()) {
        match var {
            datamodel::Variable::Stock(s) => s.uid = Some(uid),
            datamodel::Variable::Flow(f) => f.uid = Some(uid),
            datamodel::Variable::Aux(a) => a.uid = Some(uid),
            datamodel::Variable::Module(m) => m.uid = Some(uid),
        }
    }
    let mut base = generate_layout(&project, TEST_MODEL, None).expect("base layout");
    let (doubled, emigration) = (uid_named(&base, "doubled"), uid_named(&base, "emigration"));
    let undrawn = [doubled, emigration];
    base.elements.retain(|e| match e {
        ViewElement::Link(l) => !undrawn.contains(&l.from_uid) && !undrawn.contains(&l.to_uid),
        ViewElement::Cloud(c) => !undrawn.contains(&c.flow_uid),
        other => !undrawn.contains(&other.get_uid()),
    });

    for (row, op) in [
        (
            "an edit that creates an element",
            ModelOperation::UpsertAux(aux("note", "1")),
        ),
        (
            "an edit that creates none",
            ModelOperation::UpsertAux(aux("birth_rate", "0.2")),
        ),
    ] {
        let (_, view) = sync(&project, &base, vec![op]);
        let by_uid: HashMap<i32, &ViewElement> =
            view.elements.iter().map(|e| (e.get_uid(), e)).collect();
        for e in &view.elements {
            match e {
                ViewElement::Link(l) => assert!(
                    [l.from_uid, l.to_uid].iter().all(|u| by_uid
                        .get(u)
                        .is_some_and(|x| !matches!(x, ViewElement::Link(_)))),
                    "{row}: link #{} {} -> {} references an element the view does not draw",
                    l.uid,
                    l.from_uid,
                    l.to_uid
                ),
                ViewElement::Cloud(c) => assert!(
                    matches!(by_uid.get(&c.flow_uid), Some(ViewElement::Flow(_))),
                    "{row}: cloud #{} belongs to #{}, which is no drawn flow",
                    c.uid,
                    c.flow_uid
                ),
                _ => {}
            }
        }
        for name in ["doubled", "emigration"] {
            assert!(
                !view
                    .elements
                    .iter()
                    .any(|e| e.get_name().is_some_and(|n| canonicalize(n) == name)),
                "{row}: {name} stays undrawn"
            );
        }
    }
}

#[test]
fn a_curved_link_turns_with_an_endpoint_the_sync_moved() {
    // A surviving curved link whose endpoint the sync moved keeps its bow: its
    // takeoff angle turns by exactly as much as the chord between its ends.
    // Rows over what moves an endpoint -- a re-attached flow's valve (restating
    // source without transfer puts a cloud on the valve, which takes the pipe's
    // middle and slides off note) and a variable redrawn as a stock (a, turned
    // into a stock, covers b and moves off it) -- each alone, which creates no
    // element, and with a created parameter, which makes the pass place one.
    let mut valve_project_base = reattached_valve_fixture("note * 10");
    let rebuilt_project = project_with(vec![
        datamodel::Variable::Aux(aux("a", "1")),
        datamodel::Variable::Aux(aux("b", "2")),
        datamodel::Variable::Aux(aux("c", "a * 2")),
    ]);
    let mut rebuilt_base =
        generate_layout(&rebuilt_project, TEST_MODEL, None).expect("base layout");
    for e in &mut rebuilt_base.elements {
        match e {
            ViewElement::Aux(x) if canonicalize(&x.name) == "a" => (x.x, x.y) = (100.0, 100.0),
            ViewElement::Aux(x) if canonicalize(&x.name) == "b" => (x.x, x.y) = (130.0, 100.0),
            ViewElement::Aux(x) if canonicalize(&x.name) == "c" => (x.x, x.y) = (100.0, 300.0),
            _ => {}
        }
    }
    let curve = |view: &mut datamodel::StockFlow| {
        for e in &mut view.elements {
            if let ViewElement::Link(l) = e {
                l.shape = LinkShape::Arc(40.0);
            }
        }
    };
    curve(&mut valve_project_base.1);
    curve(&mut rebuilt_base);
    let center = |view: &datamodel::StockFlow, name: &str| {
        view.elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Aux(a) if canonicalize(&a.name) == name => Some((a.x, a.y)),
                ViewElement::Stock(s) if canonicalize(&s.name) == name => Some((s.x, s.y)),
                ViewElement::Flow(f) if canonicalize(&f.name) == name => Some((f.x, f.y)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} drawn"))
    };
    let chord = |view: &datamodel::StockFlow, from: &str, to: &str| {
        let (a, b) = (center(view, from), center(view, to));
        (b.1 - a.1).atan2(b.0 - a.0).to_degrees()
    };
    let takeoff =
        |view: &datamodel::StockFlow, from: &str, to: &str| match link_between(view, from, to)
            .map(|l| l.shape)
        {
            Some(LinkShape::Arc(t)) => t,
            other => panic!("{from} -> {to} is no curved link: {other:?}"),
        };

    let (valve_project, valve_base) = &valve_project_base;
    let fixtures = [
        (
            "a re-attached flow's valve",
            valve_project,
            valve_base,
            ModelOperation::UpsertStock(stock("source", &[], &[])),
            ("note", "transfer"),
            "transfer",
        ),
        (
            "a variable redrawn as a stock",
            &rebuilt_project,
            &rebuilt_base,
            ModelOperation::UpsertStock(stock("a", &[], &[])),
            ("a", "c"),
            "a",
        ),
    ];
    for (what, project, base, op, (from, to), moved) in fixtures {
        for (extra_row, extra) in [
            ("alone", vec![]),
            (
                "with a created parameter",
                vec![ModelOperation::UpsertAux(aux("extra", "1"))],
            ),
        ] {
            let row = format!("{what}, {extra_row}");
            let mut ops = vec![op.clone()];
            ops.extend(extra);
            let (_, view) = sync(project, base, ops);
            assert_ne!(
                center(&view, moved),
                center(base, moved),
                "{row}: the sync moves {moved}, or this row pins nothing"
            );
            let turned = takeoff(&view, from, to) - takeoff(base, from, to);
            let expected = chord(&view, from, to) - chord(base, from, to);
            let off = (turned - expected).rem_euclid(360.0);
            assert!(
                off.min(360.0 - off) < 1e-9,
                "{row}: the takeoff turned {turned} degrees, the chord {expected}"
            );
        }
    }
}
