// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Tests of the scene generator.
//!
//! What these establish: strict scenes hold every strict arm over many seeds;
//! the view realizes every route the generator records (its shape classified
//! from the points' directions, its hub end, and the face its far stock is
//! entered through), and the recorded routes cover every shape x start x end
//! combination, so "passes over many seeds" is not vacuous; imported scenes
//! pass tolerant mode while strict mode reports exactly the arms each applied
//! shape records, and every shape is applied; a seed determines its scene.
//!
//! What they do not establish: that the distribution resembles real models
//! beyond the enumerated shapes.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::editing::invariants::{FlowArm, Mode, check_flow_invariants, format_violations};
use crate::editing::test_support::{directions, flow_of};

use super::*;

const SEEDS: u32 = 200;

#[test]
fn strict_scenes_hold_every_strict_arm() {
    let mut failures = Vec::new();
    for seed in 1..=SEEDS {
        let scene = strict_scene(seed);
        let violations = check_flow_invariants(&scene.elements, Mode::Strict { routed: None });
        if !violations.is_empty() {
            failures.push(format!("seed {seed}\n{}", format_violations(&violations)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The shape a path's segment directions spell: straight, L, Z (two turns back
/// to the first direction) or bracket (five segments).
fn shape_of(dirs: &str) -> Option<FlowShape> {
    let d: Vec<char> = dirs.chars().collect();
    match d.len() {
        1 => Some(FlowShape::Straight),
        2 => Some(FlowShape::L),
        3 if d[0] == d[2] => Some(FlowShape::Z),
        5 if d[0] == d[2] && d[2] == d[4] => Some(FlowShape::Bracket),
        _ => None,
    }
}

#[test]
fn recorded_routes_are_realized_and_cover_every_shape_start_and_end() {
    let mut failures = Vec::new();
    let mut covered: HashSet<(FlowShape, RouteStart, bool)> = HashSet::new();
    for seed in 1..=SEEDS {
        let scene = strict_scene(seed);
        for route in &scene.routes {
            let f = flow_of(&scene.elements, route.flow_uid);
            let n = f.points.len();
            let dirs = directions(f);
            if shape_of(&dirs) != Some(route.shape) {
                failures.push(format!(
                    "seed {seed} flow {}: directions {dirs}",
                    route.flow_uid
                ));
            }
            let hub_end = match route.start {
                RouteStart::HubSource => f.points[0].attached_to_uid,
                RouteStart::HubSink => f.points[n - 1].attached_to_uid,
                RouteStart::FreeCloud => None,
            };
            if route.start != RouteStart::FreeCloud && hub_end != Some(scene.hub) {
                failures.push(format!(
                    "seed {seed} flow {}: not attached to the hub at its start",
                    route.flow_uid
                ));
            }
            if let Some((stock, face)) = route.far {
                // The far end is the walk's end: the sink unless the walk was reversed.
                let (far, adjacent) = if route.start == RouteStart::HubSink {
                    (&f.points[0], &f.points[1])
                } else {
                    (&f.points[n - 1], &f.points[n - 2])
                };
                let entered = entered_face(far.x - adjacent.x, far.y - adjacent.y);
                if far.attached_to_uid != Some(stock) || entered != face {
                    failures.push(format!(
                        "seed {seed} flow {}: far stock {stock} through {face:?}",
                        route.flow_uid
                    ));
                }
            }
            covered.insert((route.shape, route.start, route.far.is_some()));
        }
    }
    for shape in FlowShape::ALL {
        for start in RouteStart::ALL {
            for far in [false, true] {
                if !covered.contains(&(shape, start, far)) {
                    failures.push(format!(
                        "no route of shape {} from {} ending at {}",
                        shape_name(shape),
                        start_name(start),
                        if far { "a stock" } else { "a cloud" }
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The face a final segment travelling `(dx, dy)` enters a stock through.
fn entered_face(dx: f64, dy: f64) -> Face {
    if dx.abs() > dy.abs() {
        if dx > 0.0 { Face::Left } else { Face::Right }
    } else if dy > 0.0 {
        Face::Top
    } else {
        Face::Bottom
    }
}

fn shape_name(shape: FlowShape) -> &'static str {
    match shape {
        FlowShape::Straight => "straight",
        FlowShape::L => "L",
        FlowShape::Z => "Z",
        FlowShape::Bracket => "bracket",
    }
}

fn start_name(start: RouteStart) -> &'static str {
    match start {
        RouteStart::HubSource => "the hub as source",
        RouteStart::HubSink => "the hub as sink",
        RouteStart::FreeCloud => "a free cloud",
    }
}

#[test]
fn imported_scenes_report_exactly_the_recorded_arms_and_pass_tolerant_mode() {
    let mut failures = Vec::new();
    let mut applied: HashSet<ImportShape> = HashSet::new();
    for seed in 1..=SEEDS {
        let scene = imported_scene(seed);
        let tolerant = check_flow_invariants(&scene.elements, Mode::Tolerant);
        if !tolerant.is_empty() {
            failures.push(format!(
                "seed {seed} tolerant\n{}",
                format_violations(&tolerant)
            ));
        }
        let mut got: HashMap<i32, Vec<FlowArm>> = HashMap::new();
        for v in check_flow_invariants(&scene.elements, Mode::Strict { routed: None }) {
            got.entry(v.uid).or_default().push(v.arm);
        }
        let mut want: HashMap<i32, Vec<FlowArm>> = HashMap::new();
        for m in &scene.mutations {
            applied.insert(m.shape);
            want.entry(m.flow_uid).or_default().extend(&m.arms);
        }
        let uids: HashSet<i32> = got.keys().chain(want.keys()).copied().collect();
        for uid in uids {
            let mut g = got.remove(&uid).unwrap_or_default();
            let mut w = want.remove(&uid).unwrap_or_default();
            g.sort();
            w.sort();
            if g != w {
                let names =
                    |arms: &[FlowArm]| arms.iter().map(|a| a.name()).collect::<Vec<_>>().join(", ");
                failures.push(format!(
                    "seed {seed} flow {uid}: strict reports [{}], want [{}]",
                    names(&g),
                    names(&w)
                ));
            }
        }
    }
    for shape in ImportShape::ALL {
        if !applied.contains(&shape) {
            failures.push(format!(
                "import shape {} was never applied",
                ImportShape::ALL.iter().position(|s| *s == shape).unwrap()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_seed_determines_its_scene() {
    for seed in [1, 17, 123] {
        let a: Vec<ViewElement> = strict_scene(seed).elements;
        let b: Vec<ViewElement> = strict_scene(seed).elements;
        assert!(a == b, "seed {seed} built two different strict scenes");
        let a = imported_scene(seed).elements;
        let b = imported_scene(seed).elements;
        assert!(a == b, "seed {seed} built two different imported scenes");
    }
    assert!(strict_scene(1).elements != strict_scene(2).elements);
}
