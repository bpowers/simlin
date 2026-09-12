// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Fixtures for the editing core's tests.
//!
//! Views are built as engine JSON and loaded through the production conversion
//! (`datamodel::ViewElement::from`), so element fields -- an unattached point's
//! `attached_to_uid`, a default label side -- are what the core sees on a view
//! the editor loaded. The path helpers are written independently of the core:
//! an expectation a test states is never computed with the code under test.

use std::collections::{HashMap, HashSet};

use serde_json::json;

use crate::common::canonicalize;
use crate::datamodel::view_element::{Flow, FlowPoint, Stock};
use crate::datamodel::{self, Variable, ViewElement};
use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};
use crate::json;
use crate::patch::{ModelOperation, ModelPatch, ProjectPatch, apply_patch};
use crate::test_common::TestProject;

use super::base::BaseView;
use super::gesture::ViewEdit;
use super::invariants::{Mode, check_flow_invariants, format_violations};
use super::terminal::FlowGeometry;

pub(crate) fn link(uid: i32, from_uid: i32, to_uid: i32, arc: Option<f64>) -> json::ViewElement {
    json::ViewElement::Link(json::LinkViewElement {
        uid,
        from_uid,
        to_uid,
        arc,
        multi_points: Vec::new(),
        polarity: None,
    })
}

pub(crate) fn alias(uid: i32, alias_of_uid: i32, x: f64, y: f64) -> json::ViewElement {
    json::ViewElement::Alias(json::AliasViewElement {
        uid,
        alias_of_uid,
        x,
        y,
        label_side: String::new(),
    })
}

/// `element` with its label on `side` (`top`, `left`, `bottom`, `right`).
pub(crate) fn labeled(mut element: json::ViewElement, side: &str) -> json::ViewElement {
    match &mut element {
        json::ViewElement::Stock(e) => e.label_side = side.to_string(),
        json::ViewElement::Flow(e) => e.label_side = side.to_string(),
        json::ViewElement::Auxiliary(e) => e.label_side = side.to_string(),
        json::ViewElement::Module(e) => e.label_side = side.to_string(),
        json::ViewElement::Alias(e) => e.label_side = side.to_string(),
        _ => {}
    }
    element
}

/// A project whose model "main" holds `elements` as its first view, with a
/// variable per named element and each stock's lists exactly the flows attached
/// to it: the model/view agreement a committed view holds. The variables are
/// built as engine JSON and loaded through the production conversion.
pub(crate) fn project_of(elements: Vec<ViewElement>) -> datamodel::Project {
    let kinds: HashMap<i32, bool> = elements
        .iter()
        .map(|e| (e.get_uid(), matches!(e, ViewElement::Stock(_))))
        .collect();
    let mut inflows: HashMap<i32, Vec<String>> = HashMap::new();
    let mut outflows: HashMap<i32, Vec<String>> = HashMap::new();
    for element in &elements {
        let ViewElement::Flow(f) = element else {
            continue;
        };
        if f.points.len() < 2 {
            continue;
        }
        let is_stock = |uid: Option<i32>| uid.filter(|u| kinds.get(u) == Some(&true));
        if let Some(uid) = is_stock(f.points[0].attached_to_uid) {
            outflows.entry(uid).or_default().push(f.name.clone());
        }
        if let Some(uid) = is_stock(f.points[f.points.len() - 1].attached_to_uid) {
            inflows.entry(uid).or_default().push(f.name.clone());
        }
    }
    let (mut stocks, mut flows, mut auxes, mut modules) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for element in &elements {
        match element {
            ViewElement::Stock(s) => stocks.push(json!({
                "name": s.name,
                "initialEquation": "1",
                "inflows": inflows.get(&s.uid).cloned().unwrap_or_default(),
                "outflows": outflows.get(&s.uid).cloned().unwrap_or_default(),
            })),
            ViewElement::Flow(f) => flows.push(json!({"name": f.name, "equation": "1"})),
            ViewElement::Aux(a) => auxes.push(json!({"name": a.name, "equation": "1"})),
            ViewElement::Module(m) => modules.push(json!({"name": m.name, "modelName": "sub"})),
            _ => {}
        }
    }
    let json_model: json::Model = serde_json::from_value(json!({
        "name": "main",
        "stocks": stocks,
        "flows": flows,
        "auxiliaries": auxes,
        "modules": modules,
    }))
    .expect("a well-formed JSON model");
    let mut model = datamodel::Model::from(json_model);
    model.views = vec![datamodel::View::StockFlow(datamodel::StockFlow {
        name: None,
        elements,
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    })];
    let mut project = TestProject::new("editing").build_datamodel();
    project.models = vec![model];
    project
}

pub(crate) fn main_model(project: &datamodel::Project) -> &datamodel::Model {
    project.get_model("main").expect("the model main")
}

pub(crate) fn view_of(project: &datamodel::Project) -> &[ViewElement] {
    match main_model(project).views.first() {
        Some(datamodel::View::StockFlow(sf)) => &sf.elements,
        None => panic!("main has no view"),
    }
}

pub(crate) fn stock_flow_of(project: &datamodel::Project) -> &datamodel::StockFlow {
    match main_model(project).views.first() {
        Some(datamodel::View::StockFlow(sf)) => sf,
        None => panic!("main has no view"),
    }
}

pub(crate) fn base_of(project: &datamodel::Project) -> BaseView {
    BaseView::new(main_model(project), stock_flow_of(project))
}

/// Apply `edit` to main's first view through the production patch path.
pub(crate) fn apply_edit(project: &mut datamodel::Project, edit: &ViewEdit) -> Result<(), String> {
    apply_patch(project, edit_patch(edit)).map_err(|e| e.to_string())
}

pub(crate) fn edit_patch(edit: &ViewEdit) -> ProjectPatch {
    ProjectPatch {
        project_ops: Vec::new(),
        models: vec![ModelPatch {
            name: "main".to_string(),
            ops: vec![ModelOperation::EditView {
                index: 0,
                upsert: edit.upsert.clone(),
                remove: edit.remove.clone(),
            }],
        }],
    }
}

/// The model/view agreement a committed edit holds, empty when it holds: every
/// named element names a variable of its own kind (M1); links run between two
/// distinct elements of the view, each a named element or an alias (an alias
/// stands for a named element, and imported views link into aliases, which the
/// generated scenes model), every alias names a named element, every cloud
/// belongs to a flow of the view
/// and is an endpoint of it exactly once, and no uid repeats (M3); and each
/// stock's lists hold exactly the flows attached to it, each once.
pub(crate) fn agreement_report(project: &datamodel::Project) -> String {
    let model = main_model(project);
    let view = view_of(project);
    let mut problems = Vec::new();
    let mut by_uid: HashMap<i32, &ViewElement> = HashMap::new();
    for element in view {
        if by_uid.insert(element.get_uid(), element).is_some() {
            problems.push(format!("uid {} repeats", element.get_uid()));
        }
    }
    let named = |e: &ViewElement| {
        matches!(
            e,
            ViewElement::Stock(_)
                | ViewElement::Flow(_)
                | ViewElement::Aux(_)
                | ViewElement::Module(_)
        )
    };
    for element in view {
        let uid = element.get_uid();
        if named(element) {
            let name = element.get_name().unwrap_or_default();
            let agrees = match (element, model.get_variable(name)) {
                (_, None) => false,
                (ViewElement::Stock(_), Some(Variable::Stock(_)))
                | (ViewElement::Flow(_), Some(Variable::Flow(_)))
                | (ViewElement::Aux(_), Some(Variable::Aux(_)))
                | (ViewElement::Module(_), Some(Variable::Module(_))) => true,
                _ => false,
            };
            if !agrees {
                problems.push(format!(
                    "element {uid} '{name}' names no variable of its kind"
                ));
            }
        }
        match element {
            ViewElement::Link(l) => {
                let from = by_uid.get(&l.from_uid);
                let to = by_uid.get(&l.to_uid);
                let linkable = |e: &&ViewElement| named(e) || matches!(e, ViewElement::Alias(_));
                let from_ok = from.is_some_and(linkable);
                let to_ok = to.is_some_and(linkable);
                if !from_ok || !to_ok || l.from_uid == l.to_uid {
                    problems.push(format!("link {uid} runs {} -> {}", l.from_uid, l.to_uid));
                }
            }
            ViewElement::Alias(a) => {
                if !by_uid.get(&a.alias_of_uid).is_some_and(|e| named(e)) {
                    problems.push(format!("alias {uid} names {}", a.alias_of_uid));
                }
            }
            ViewElement::Cloud(c) => {
                let endpoints = match by_uid.get(&c.flow_uid) {
                    Some(ViewElement::Flow(f)) if f.points.len() >= 2 => {
                        [&f.points[0], &f.points[f.points.len() - 1]]
                            .iter()
                            .filter(|p| p.attached_to_uid == Some(uid))
                            .count()
                    }
                    _ => 0,
                };
                if endpoints != 1 {
                    problems.push(format!(
                        "cloud {uid} is {endpoints} endpoints of flow {}",
                        c.flow_uid
                    ));
                }
            }
            _ => {}
        }
    }
    for element in view {
        let ViewElement::Stock(s) = element else {
            continue;
        };
        let Some(Variable::Stock(var)) = model.get_variable(&s.name) else {
            continue;
        };
        let attached = |sink: bool| -> HashSet<String> {
            view.iter()
                .filter_map(|e| match e {
                    ViewElement::Flow(f) if f.points.len() >= 2 => {
                        let p = if sink {
                            &f.points[f.points.len() - 1]
                        } else {
                            &f.points[0]
                        };
                        (p.attached_to_uid == Some(s.uid) && model.get_variable(&f.name).is_some())
                            .then(|| canonicalize(&f.name).into_owned())
                    }
                    _ => None,
                })
                .collect()
        };
        for (list, sink, what) in [
            (&var.inflows, true, "inflows"),
            (&var.outflows, false, "outflows"),
        ] {
            let listed: HashSet<String> =
                list.iter().map(|f| canonicalize(f).into_owned()).collect();
            if listed.len() != list.len() {
                problems.push(format!("stock '{}' repeats an entry of its {what}", s.name));
            }
            let want = attached(sink);
            if listed != want {
                let mut l: Vec<_> = listed.into_iter().collect();
                let mut w: Vec<_> = want.into_iter().collect();
                l.sort();
                w.sort();
                problems.push(format!(
                    "stock '{}' lists {what} {l:?}, attached {w:?}",
                    s.name
                ));
            }
        }
    }
    problems.join("\n")
}

pub(crate) fn stock(uid: i32, x: f64, y: f64) -> json::ViewElement {
    json::ViewElement::Stock(json::StockViewElement {
        uid,
        name: format!("s{uid}"),
        x,
        y,
        label_side: String::new(),
    })
}

pub(crate) fn aux(uid: i32, x: f64, y: f64) -> json::ViewElement {
    json::ViewElement::Auxiliary(json::AuxiliaryViewElement {
        uid,
        name: format!("a{uid}"),
        x,
        y,
        label_side: String::new(),
    })
}

pub(crate) fn cloud(uid: i32, flow_uid: i32, x: f64, y: f64) -> json::ViewElement {
    json::ViewElement::Cloud(json::CloudViewElement {
        uid,
        flow_uid,
        x,
        y,
    })
}

/// A flow whose points are `(x, y, attached)`, with 0 for an unattached point
/// (the JSON spelling).
pub(crate) fn flow(uid: i32, valve: (f64, f64), points: &[(f64, f64, i32)]) -> json::ViewElement {
    json::ViewElement::Flow(json::FlowViewElement {
        uid,
        name: format!("f{uid}"),
        x: valve.0,
        y: valve.1,
        label_side: String::new(),
        points: points
            .iter()
            .map(|&(x, y, attached_to_uid)| json::FlowPoint {
                x,
                y,
                attached_to_uid,
            })
            .collect(),
    })
}

pub(crate) fn load(elements: Vec<json::ViewElement>) -> Vec<ViewElement> {
    elements.into_iter().map(ViewElement::from).collect()
}

pub(crate) fn flow_of(view: &[ViewElement], uid: i32) -> &Flow {
    view.iter()
        .find_map(|e| match e {
            ViewElement::Flow(f) if f.uid == uid => Some(f),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no flow {uid}"))
}

/// The view with `changed` elements replacing (or added after) those with the
/// same uid, and `removed` uids dropped.
pub(crate) fn patched(
    view: &[ViewElement],
    changed: impl IntoIterator<Item = ViewElement>,
    removed: &[i32],
) -> Vec<ViewElement> {
    let mut out: Vec<ViewElement> = view
        .iter()
        .filter(|e| !removed.contains(&e.get_uid()))
        .cloned()
        .collect();
    for element in changed {
        let uid = element.get_uid();
        match out.iter_mut().find(|e| e.get_uid() == uid) {
            Some(slot) => *slot = element,
            None => out.push(element),
        }
    }
    out
}

/// The view a planner would render after an operation: the routed flow, every
/// cloud it moved (at its new center), and `also` (moved terminals, added
/// clouds).
pub(crate) fn applied(
    view: &[ViewElement],
    g: &FlowGeometry,
    also: impl IntoIterator<Item = ViewElement>,
    removed: &[i32],
) -> Vec<ViewElement> {
    let mut next = patched(view, also, removed);
    for moved in &g.clouds {
        let slot = next.iter_mut().find_map(|e| match e {
            ViewElement::Cloud(c) if c.uid == moved.uid => Some(c),
            _ => None,
        });
        let cloud = slot.unwrap_or_else(|| {
            panic!(
                "the operation moved cloud {}, which the view lacks",
                moved.uid
            )
        });
        cloud.x = moved.at.x;
        cloud.y = moved.at.y;
    }
    patched(&next, [ViewElement::Flow(g.flow.clone())], &[])
}

/// Strict violations of the routed flows, formatted so a failure shows every
/// arm and its numbers; empty when the view holds.
pub(crate) fn strict_report(view: &[ViewElement], routed: &[i32]) -> String {
    let routed: HashSet<i32> = routed.iter().copied().collect();
    let violations: Vec<_> = check_flow_invariants(
        view,
        Mode::Strict {
            routed: Some(&routed),
        },
    )
    .into_iter()
    .filter(|v| routed.contains(&v.uid))
    .collect();
    format_violations(&violations)
}

/// The segment directions of a path as letters (R, L, D, U): its shape,
/// independent of lengths.
pub(crate) fn directions(f: &Flow) -> String {
    f.points
        .windows(2)
        .map(|w| {
            if (w[1].y - w[0].y).abs() <= 1e-6 {
                if w[1].x > w[0].x { 'R' } else { 'L' }
            } else if w[1].y > w[0].y {
                'D'
            } else {
                'U'
            }
        })
        .collect()
}

/// The face of `stock` an endpoint sits on: a point on a side line is on the
/// left or right face, on a cap line the top or bottom face, and at a corner the
/// adjacent segment's axis decides (a horizontal segment leaves a side face);
/// "?" off every face.
pub(crate) fn face_name(stock: &Stock, p: &FlowPoint, adjacent: &FlowPoint) -> &'static str {
    let e = 1e-6;
    let (dx, dy) = (p.x - stock.x, p.y - stock.y);
    let (hw, hh) = (STOCK_WIDTH / 2.0, STOCK_HEIGHT / 2.0);
    let side = ((dx.abs() - hw).abs() <= e && dy.abs() <= hh + e).then_some(if dx > 0.0 {
        "right"
    } else {
        "left"
    });
    let cap = ((dy.abs() - hh).abs() <= e && dx.abs() <= hw + e).then_some(if dy > 0.0 {
        "bottom"
    } else {
        "top"
    });
    match (side, cap) {
        (Some(side), Some(cap)) => {
            if (adjacent.y - p.y).abs() <= (adjacent.x - p.x).abs() {
                side
            } else {
                cap
            }
        }
        (Some(side), None) => side,
        (None, Some(cap)) => cap,
        (None, None) => "?",
    }
}

fn distance_to_path(f: &Flow, px: f64, py: f64) -> f64 {
    f.points
        .windows(2)
        .map(|w| {
            let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
            let l2 = dx * dx + dy * dy;
            let t = if l2 == 0.0 {
                0.0
            } else {
                (((px - w[0].x) * dx + (py - w[0].y) * dy) / l2).clamp(0.0, 1.0)
            };
            (px - (w[0].x + t * dx)).hypot(py - (w[0].y + t * dy))
        })
        .fold(f64::INFINITY, f64::min)
}

/// The symmetric Hausdorff distance between two paths, sampled every `step` px.
pub(crate) fn hausdorff(a: &Flow, b: &Flow, step: f64) -> f64 {
    let one_way = |from: &Flow, to: &Flow| {
        let Some(first) = from.points.first() else {
            return 0.0;
        };
        let mut worst = distance_to_path(to, first.x, first.y);
        for w in from.points.windows(2) {
            let (dx, dy) = (w[1].x - w[0].x, w[1].y - w[0].y);
            let n = ((dx.hypot(dy) / step).ceil() as usize).max(1);
            for k in 1..=n {
                let t = k as f64 / n as f64;
                worst = worst.max(distance_to_path(to, w[0].x + dx * t, w[0].y + dy * t));
            }
        }
        worst
    };
    one_way(a, b).max(one_way(b, a))
}
