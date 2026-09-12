// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Which elements an imported MDL flow's ends attach to, read through
//! `open_vensim` on corpus files. The attachments follow the model's stock
//! lists: a side the model links to a stock drawn in the view attaches to that
//! stock, and every other side ends in a cloud owned by the flow. Every
//! imported flow renders.
//!
//! One test per way the sketch can fail to supply an end the model has:
//! - A: the flow's primary sketch record is a label with no valve, and the pipe
//!   is drawn on another copy;
//! - B: a pipe end targets a stock that does not list the flow (the importer
//!   gave that stock a synthesized net flow), so that side has no stock;
//! - C: the flow is drawn as a label with no pipe at all;
//! - B': a pipe exists, but the stock the model links is not one of its ends.

use crate::common::canonicalize;
use crate::compat::open_vensim;
use crate::datamodel::{self, ViewElement, view_element};
use crate::diagram::flow_geometry::flow_invariant_violations;

fn main_view(project: &datamodel::Project) -> &datamodel::StockFlow {
    let datamodel::View::StockFlow(sf) = &project.models[0].views[0];
    sf
}

fn flow<'a>(view: &'a datamodel::StockFlow, name: &str) -> &'a view_element::Flow {
    view.elements
        .iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.name == name => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("flow {name:?} not in view"))
}

/// What an endpoint is attached to: `Some(stock ident)` for a stock, `None`
/// for a cloud owned by the flow. Panics for anything else, which is itself a
/// violation the importer must not produce.
fn end_attachment(
    view: &datamodel::StockFlow,
    f: &view_element::Flow,
    point: &view_element::FlowPoint,
) -> Option<String> {
    let uid = point
        .attached_to_uid
        .unwrap_or_else(|| panic!("{}: an endpoint is unattached", f.name));
    match view.elements.iter().find(|e| e.get_uid() == uid) {
        Some(ViewElement::Stock(s)) => Some(canonicalize(&s.name).into_owned()),
        Some(ViewElement::Cloud(c)) => {
            assert_eq!(c.flow_uid, f.uid, "{}: cloud owned by another flow", f.name);
            None
        }
        other => panic!(
            "{}: endpoint attached to {:?}",
            f.name,
            other.map(|e| e.get_uid())
        ),
    }
}

/// The model's source and sink stock for a flow, restricted to stocks drawn in
/// the view.
fn model_sides(
    project: &datamodel::Project,
    view: &datamodel::StockFlow,
    flow_ident: &str,
) -> (Option<String>, Option<String>) {
    let drawn: Vec<String> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some(canonicalize(&s.name).into_owned()),
            _ => None,
        })
        .collect();
    let mut source = None;
    let mut sink = None;
    for var in &project.models[0].variables {
        let datamodel::Variable::Stock(s) = var else {
            continue;
        };
        let ident = canonicalize(&s.ident).into_owned();
        if !drawn.contains(&ident) {
            continue;
        }
        if s.outflows.iter().any(|o| canonicalize(o) == flow_ident) {
            source = Some(ident.clone());
        }
        if s.inflows.iter().any(|i| canonicalize(i) == flow_ident) {
            sink = Some(ident);
        }
    }
    (source, sink)
}

/// Every flow in the view: the invariants hold, and each end attaches where
/// the model says.
fn assert_every_flow_resolved(project: &datamodel::Project, label: &str) {
    let view = main_view(project);
    assert_eq!(
        flow_invariant_violations(&view.elements),
        Vec::<String>::new(),
        "{label}: flow invariants"
    );
    for elem in &view.elements {
        let ViewElement::Flow(f) = elem else { continue };
        let ident = canonicalize(&f.name).into_owned();
        let (source, sink) = model_sides(project, view, &ident);
        let first = end_attachment(view, f, &f.points[0]);
        let last = end_attachment(view, f, f.points.last().unwrap());
        assert_eq!(first, source, "{label}: {} source attachment", f.name);
        assert_eq!(last, sink, "{label}: {} sink attachment", f.name);
    }
}

/// Class A. `free 6.mdl` draws `Total Carbon Emissions` twice: its primary
/// record is a label with no valve, and its pipe into `CO2 in Atmosphere` is on
/// a copy the sketch marks as a ghost. The pipe-carrying copy presents the
/// flow; the label is an alias of it.
#[test]
fn a_flow_whose_pipe_is_on_a_ghost_copy_is_presented_by_that_copy() {
    const FREE6: &str =
        include_str!("../../../../../test/metasd/FREE/FREE6/FREE6-original/free 6.mdl");
    let project = open_vensim(FREE6).expect("free 6 imports");
    let view = main_view(&project);
    let f = flow(view, "Total_Carbon_Emissions");
    assert_eq!(
        end_attachment(view, f, f.points.last().unwrap()),
        Some("co2_in_atmosphere".to_string())
    );
    assert_eq!(end_attachment(view, f, &f.points[0]), None);
    assert!(
        view.elements
            .iter()
            .any(|e| matches!(e, ViewElement::Alias(a) if a.alias_of_uid == f.uid)),
        "the label copy becomes an alias of the flow"
    );
    assert_every_flow_resolved(&project, "free 6");
}

/// Class B. `IDch15d.mdl`'s `SRR` pipe runs from `MTR` into `IAR`, but `IAR`'s
/// rate (`SRR - SSR`, with `SSR` also draining `UOR`) does not decompose, so the
/// model gives `IAR` a synthesized net flow and `SRR` no sink stock. The sink is
/// a cloud at the pipe's end, outside `IAR`'s box, and the valve stays where
/// the sketch drew it.
#[test]
fn a_pipe_into_a_stock_that_does_not_list_the_flow_ends_in_a_cloud() {
    const IDCH15D: &str =
        include_str!("../../../../../test/metasd/industrial-dynamics/IDch15/IDch15d.mdl");
    let project = open_vensim(IDCH15D).expect("IDch15d imports");
    let view = main_view(&project);
    let f = flow(view, "SRR");
    assert_eq!(
        end_attachment(view, f, &f.points[0]),
        Some("mtr".to_string())
    );
    assert_eq!(end_attachment(view, f, f.points.last().unwrap()), None);
    assert_eq!((f.x, f.y), (159.0, 602.0));
    assert_every_flow_resolved(&project, "IDch15d");

    const MAPPING: &str = include_str!(
        "../../../../../test/test-models/tests/subscript_mapping_simple/test_subscript_mapping_simple.mdl"
    );
    let project = open_vensim(MAPPING).expect("subscript_mapping_simple imports");
    assert_every_flow_resolved(&project, "subscript_mapping_simple");

    const BEER: &str = include_str!("../../../../../test/metasd/beer-game/RealBeer4-Sterman13.mdl");
    let project = open_vensim(BEER).expect("RealBeer4 imports");
    let view = main_view(&project);
    let f = flow(view, "Receiving");
    assert_eq!((f.x, f.y), (552.0, 769.0), "the sketch's valve position");
    assert_every_flow_resolved(&project, "RealBeer4");
}

/// Class C. `sample.mdl` draws `rate` as a label with a plain arrow into `G`
/// and no pipe; `G = INTEG(rate, ...)`. The flow attaches to `G` through a
/// pipe routed through the valve at the label's position, from a cloud.
/// `IDch15d.mdl`'s `SSD` has two label copies and no pipe; the copy with an
/// arrow into its stock `MTR` presents the flow.
#[test]
fn a_flow_drawn_without_a_pipe_is_routed_into_its_stock() {
    const SAMPLE: &str = include_str!("../../../../../test/sdeverywhere/models/sample/sample.mdl");
    let project = open_vensim(SAMPLE).expect("sample imports");
    let view = main_view(&project);
    let f = flow(view, "rate");
    assert_eq!((f.x, f.y), (101.0, 327.0), "the valve stays at the label");
    assert_every_flow_resolved(&project, "sample");

    const IDCH15D: &str =
        include_str!("../../../../../test/metasd/industrial-dynamics/IDch15/IDch15d.mdl");
    let project = open_vensim(IDCH15D).expect("IDch15d imports");
    let view = main_view(&project);
    let f = flow(view, "SSD");
    assert_eq!(
        (f.x, f.y),
        (154.0, 350.0),
        "the copy with an arrow into MTR presents SSD"
    );
}

/// Class B'. `Query_file.mdl`'s `Expenses` pipe runs from `BalanceFunds` (a
/// net-flow stock that does not list it) to a cloud, while the model's only
/// link is `CumExpense = INTEG(Expenses, 0)`. The sink attaches to `CumExpense`
/// through a route from the valve; the source is a cloud at the pipe's end on
/// the valve's other side, outside `BalanceFunds`. The label-only cost flows
/// (class C) route into their `Cum...` stocks.
#[test]
fn a_pipe_whose_model_stock_is_elsewhere_is_routed_to_it() {
    const QUERY: &str =
        include_str!("../../../../../test/test-models/samples/Query_file/Query_file.mdl");
    let project = open_vensim(QUERY).expect("Query_file imports");
    let view = main_view(&project);
    let f = flow(view, "Expenses");
    assert_eq!(
        (f.x, f.y),
        (1323.0, 805.0),
        "the valve stays at the sketch valve"
    );
    assert_eq!(
        end_attachment(view, f, f.points.last().unwrap()),
        Some("cumexpense".to_string())
    );
    assert_eq!(end_attachment(view, f, &f.points[0]), None);
    assert_every_flow_resolved(&project, "Query_file");
}

/// Two pipe-carrying copies tie on rank. `thyroid-2008-d.mdl` draws
/// `T3 absorption` with a valve twice: in `Gut/Dosage` its pipe drains
/// `Gut T3 dissolved` into nothing, in `THR D&E` it fills `4 Plasma T3` from a
/// cloud. Each copy reaches one of the two stocks the model links and leaves
/// the other to a route from its valve; the `THR D&E` copy's valve is the
/// nearer to the stock it leaves unreached, so it presents the flow and the
/// other copy becomes an alias.
#[test]
fn of_two_pipe_carrying_copies_the_one_nearest_its_unreached_stock_presents_the_flow() {
    const THYROID: &str =
        include_str!("../../../../../test/metasd/thyroid-dynamics/thyroid-2008-d.mdl");
    let project = open_vensim(THYROID).expect("thyroid imports");
    let view = main_view(&project);
    let f = flow(view, "T3_absorption");
    assert_eq!(
        end_attachment(view, f, &f.points[0]),
        Some("gut_t3_dissolved".to_string())
    );
    assert_eq!(
        end_attachment(view, f, f.points.last().unwrap()),
        Some("4_plasma_t3".to_string())
    );
    let stock_at = |ident: &str| {
        view.elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Stock(s) if canonicalize(&s.name) == ident => Some((s.x, s.y)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("stock {ident} in view"))
    };
    let from_valve = |p: (f64, f64)| (p.0 - f.x).hypot(p.1 - f.y);
    assert!(
        from_valve(stock_at("4_plasma_t3")) < from_valve(stock_at("gut_t3_dissolved")),
        "the THR D&E copy, whose pipe fills 4 Plasma T3, presents the flow: valve ({}, {})",
        f.x,
        f.y
    );
    assert!(
        view.elements
            .iter()
            .any(|e| matches!(e, ViewElement::Alias(a) if a.alias_of_uid == f.uid)),
        "the Gut/Dosage copy becomes an alias of the flow"
    );
    assert_every_flow_resolved(&project, "thyroid-2008-d");
}
