// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

use super::*;
use crate::datamodel;
use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};

/// A shipped, hand-authored default project's main view -- the realistic input
/// the eval harness degrades (the as-loaded view production would score).
fn default_project_view(dir: &str) -> StockFlow {
    let path = format!(
        "{}/../../default_projects/{}/model.xmile",
        env!("CARGO_MANIFEST_DIR"),
        dir
    );
    let file = std::fs::File::open(&path).unwrap_or_else(|e| panic!("open {path}: {e}"));
    let project = crate::compat::open_xmile(&mut std::io::BufReader::new(file))
        .unwrap_or_else(|e| panic!("parse {path}: {e:?}"));
    match project.get_model("main").and_then(|m| m.views.first()) {
        Some(datamodel::View::StockFlow(sf)) => sf.clone(),
        _ => panic!("{dir} ships no main view"),
    }
}

fn free_uids(view: &StockFlow) -> HashSet<i32> {
    view.elements
        .iter()
        .filter(|e| is_free_node(e))
        .map(|e| e.get_uid())
        .collect()
}

/// Every flow endpoint attached to a stock lies on that stock's boundary.
fn assert_flows_attached(view: &StockFlow) {
    let stocks: HashMap<i32, (f64, f64)> = view
        .elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();
    for e in &view.elements {
        let ViewElement::Flow(f) = e else { continue };
        for p in &f.points {
            let Some(&(sx, sy)) = p.attached_to_uid.and_then(|u| stocks.get(&u)) else {
                continue;
            };
            let on_vertical = ((p.x - sx).abs() - STOCK_WIDTH / 2.0).abs() < 1e-6
                && (p.y - sy).abs() <= STOCK_HEIGHT / 2.0 + 1e-6;
            let on_horizontal = ((p.y - sy).abs() - STOCK_HEIGHT / 2.0).abs() < 1e-6
                && (p.x - sx).abs() <= STOCK_WIDTH / 2.0 + 1e-6;
            assert!(
                on_vertical || on_horizontal,
                "flow {} endpoint ({}, {}) detached from stock at ({sx}, {sy})",
                f.uid,
                p.x,
                p.y
            );
        }
    }
}

#[test]
fn cramp_and_inflate_scale_about_the_centroid_and_keep_flows_attached() {
    let view = default_project_view("fishbanks");
    let center = centroid(&view).unwrap();
    for (degradation, factor) in [
        (Degradation::Cramp(0.6), 0.6),
        (Degradation::Inflate(2.0), 2.0),
    ] {
        let out = degrade(&view, degradation).expect("applies to any non-empty view");
        for (before, after) in view.elements.iter().zip(&out.elements) {
            let (Some(p), Some(q)) = (position(before), position(after)) else {
                continue;
            };
            assert!(
                ((q.0 - center.0) - (p.0 - center.0) * factor).abs() < 1e-6
                    && ((q.1 - center.1) - (p.1 - center.1) * factor).abs() < 1e-6,
                "{} moved {:?} -> {:?}, not a {factor}x scale about {center:?}",
                degradation.name(),
                p,
                q
            );
        }
        assert_flows_attached(&out);
    }
}

#[test]
fn jitter_moves_only_free_nodes() {
    let view = default_project_view("reliability");
    let out = degrade(
        &view,
        Degradation::Jitter {
            amplitude: 40.0,
            seed: 7,
        },
    )
    .unwrap();
    let moved = moved_uids(&view, &out);
    assert!(!moved.is_empty(), "jitter must move something");
    assert!(
        moved.is_subset(&free_uids(&view)),
        "jitter moved a backbone element: {:?}",
        moved.difference(&free_uids(&view)).collect::<Vec<_>>()
    );
    assert_flows_attached(&out);
}

#[test]
fn shuffle_permutes_free_node_positions() {
    let view = default_project_view("reliability");
    let out = degrade(&view, Degradation::Shuffle { seed: 11 }).unwrap();
    let positions = |v: &StockFlow| {
        let mut ps: Vec<(i64, i64)> = v
            .elements
            .iter()
            .filter(|e| is_free_node(e))
            .filter_map(position)
            .map(|(x, y)| ((x * 1000.0).round() as i64, (y * 1000.0).round() as i64))
            .collect();
        ps.sort();
        ps
    };
    assert_eq!(
        positions(&view),
        positions(&out),
        "a shuffle keeps the same set of free-node positions"
    );
    assert!(!moved_uids(&view, &out).is_empty());
    assert!(moved_uids(&view, &out).is_subset(&free_uids(&view)));
}

#[test]
fn exile_moves_one_parameter_outside_the_diagram() {
    let view = default_project_view("population");
    let out = degrade(&view, Degradation::Exile).unwrap();
    let moved = moved_uids(&view, &out);
    assert_eq!(moved.len(), 1, "exile moves exactly one node");
    let uid = *moved.iter().next().unwrap();
    assert!(free_uids(&view).contains(&uid));
    let maxx = view
        .elements
        .iter()
        .filter_map(position)
        .map(|p| p.0)
        .fold(f64::NEG_INFINITY, f64::max);
    let exiled = out.elements.iter().find(|e| e.get_uid() == uid).unwrap();
    assert!(position(exiled).unwrap().0 > maxx + 100.0);
}

#[test]
fn stack_drops_one_free_node_on_another() {
    let view = default_project_view("population");
    let out = degrade(&view, Degradation::Stack).unwrap();
    let free: Vec<(f64, f64)> = out
        .elements
        .iter()
        .filter(|e| is_free_node(e))
        .filter_map(position)
        .collect();
    let coincident = free.iter().enumerate().any(|(i, p)| {
        free[i + 1..]
            .iter()
            .any(|q| (p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9)
    });
    assert!(coincident, "two free nodes must coincide after stacking");
    assert_eq!(moved_uids(&view, &out).len(), 1);
}

#[test]
fn moving_nodes_preserves_each_arc_links_bow() {
    let view = default_project_view("logistic-growth");
    let before = arc_offsets(&view);
    assert!(!before.is_empty(), "fixture needs curved links");
    let out = degrade(
        &view,
        Degradation::Jitter {
            amplitude: 40.0,
            seed: 3,
        },
    )
    .unwrap();
    let after = arc_offsets(&out);
    for (uid, offset) in &before {
        let new = after[uid];
        let diff = (new - offset).rem_euclid(360.0);
        assert!(
            diff < 1e-6 || (360.0 - diff) < 1e-6,
            "link {uid} bow changed: {offset} -> {new}"
        );
    }
}

#[test]
fn straighten_turns_every_arc_straight() {
    let view = default_project_view("logistic-growth");
    let out = degrade(&view, Degradation::StraightenLinks).unwrap();
    assert!(
        out.elements
            .iter()
            .all(|e| !matches!(e, ViewElement::Link(l) if matches!(l.shape, LinkShape::Arc(_))))
    );
    assert!(
        moved_uids(&view, &out).is_empty(),
        "straightening moves no node"
    );
}

#[test]
fn inapplicable_degradations_report_none() {
    let empty = StockFlow {
        name: None,
        elements: vec![],
        view_box: Default::default(),
        zoom: 1.0,
        use_lettered_polarity: false,
        font: None,
        sketch_compat: None,
    };
    for degradation in Degradation::battery() {
        assert!(
            degrade(&empty, degradation).is_none(),
            "{} applied to an empty view",
            degradation.name()
        );
    }
}
