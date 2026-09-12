// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! The strict invariants, E4 locality and E3 continuity over simulated drags on
//! generated scenes.
//!
//! Each seed builds a strict scene and drives every operation a planner calls
//! through successive pointer positions from the press, each frame evaluated
//! against the gesture's base view. Terminals are derived the way production
//! supplies them (`flow_terminals` over the view; a moved stock keeps its base
//! attachment through `stock_terminal`), and every call gets the planner's
//! context: the frame's stocks as obstacles and the other endpoints on its
//! terminal stocks as occupied slots. The operations (`GestureOp::ALL`):
//! - move a stock: `route_end` on every flow attached to it;
//! - drag a cloud end, and detach a stock end into empty space: `route_end` with
//!   the endpoint following the pointer;
//! - reattach every cloud end onto every other stock;
//! - create a flow from a stock to the pointer: `route`;
//! - offset a random segment perpendicular: `offset_segment`;
//! - slide a valve: `slide_valve`;
//! - heal every flow of an imported scene, then move a stock of the healed view.
//!
//! Asserted on every frame: the strict invariants on the routed flows (frames
//! with the pointer inside a stock are the planner's drop-target hover and are
//! skipped) and E4 locality (the result names only this flow and its own
//! clouds, and changes no field but the geometry). Asserted across frames: E3
//! continuity. A slide never jumps. For the routing operations a jump (a
//! frame-to-frame change of more than three pointer steps plus 1px) must
//! coincide with a change of route shape (segment directions or a stock face),
//! where the plan's documented transitions happen, except for a measured
//! residue of same-shape switches: a feasibility change that keeps the shape but
//! moves a hold (a riser pushed out at `MIN_SEGMENT`, a preserved tail whose held
//! corner stopped being feasible). A count above a budget is a new class of
//! jump; a count below is an improvement to re-pin.
//!
//! What this does not establish: behavior after an engine round trip, the
//! planner's hit testing, or that a transition happens where a user expects it,
//! which the decision tables own.

use std::collections::HashMap;
use std::sync::LazyLock;

use rayon::prelude::*;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{Flow, Stock};
use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};

use super::geometry::{FlowEnd, Point};
use super::heal::heal;
use super::offset::{offset_segment, segment_hold};
use super::path::{slide_valve, translate};
use super::route::{route, route_end};
use super::scene_gen::{ImportShape, Rng, imported_scene, strict_scene};
use super::terminal::{
    CloudRef, FlowGeometry, Terminal, flow_terminals, free_terminal, path_of, stock_terminal,
    target_stock_terminal,
};
use super::test_support::{
    applied, cloud, directions, face_name, flow, flow_of, hausdorff, load, patched, strict_report,
};

const SEEDS: u32 = 120;
const FRAMES: usize = 24;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum GestureOp {
    MoveStock,
    DragCloud,
    Detach,
    Reattach,
    Create,
    Offset,
    Slide,
    HealImported,
}

impl GestureOp {
    const ALL: [GestureOp; 8] = [
        GestureOp::MoveStock,
        GestureOp::DragCloud,
        GestureOp::Detach,
        GestureOp::Reattach,
        GestureOp::Create,
        GestureOp::Offset,
        GestureOp::Slide,
        GestureOp::HealImported,
    ];

    fn name(self) -> &'static str {
        match self {
            GestureOp::MoveStock => "moveStock",
            GestureOp::DragCloud => "dragCloud",
            GestureOp::Detach => "detach",
            GestureOp::Reattach => "reattach",
            GestureOp::Create => "create",
            GestureOp::Offset => "offset",
            GestureOp::Slide => "slide",
            GestureOp::HealImported => "healImported",
        }
    }

    /// The measured same-shape jump residue over every seed. A slide changes
    /// no route, so it may not jump at all; reattaching evaluates one frame per
    /// target, so it has no continuity to measure. The residue of a moved stock
    /// and a detached end is a preserved tail whose held corner stopped being
    /// feasible, which releases the flow to a route of the same shape elsewhere.
    fn same_shape_budget(self) -> usize {
        match self {
            GestureOp::MoveStock => 3,
            GestureOp::DragCloud => 0,
            GestureOp::Detach => 4,
            GestureOp::Reattach => 0,
            GestureOp::Create => 0,
            GestureOp::Offset => 0,
            GestureOp::Slide => 0,
            GestureOp::HealImported => 6,
        }
    }
}

#[derive(Default)]
struct Tally {
    frames: usize,
    violations: Vec<String>,
    locality: Vec<String>,
    jumps: usize,
    shape_jumps: usize,
    same_shape_jumps: Vec<String>,
}

impl Tally {
    fn merge(mut self, other: Tally) -> Tally {
        self.frames += other.frames;
        self.violations.extend(other.violations);
        self.locality.extend(other.locality);
        self.jumps += other.jumps;
        self.shape_jumps += other.shape_jumps;
        self.same_shape_jumps.extend(other.same_shape_jumps);
        self
    }

    fn check_frame(
        &mut self,
        view: &[ViewElement],
        routed: &[i32],
        context: impl FnOnce() -> String,
    ) {
        self.frames += 1;
        let report = strict_report(view, routed);
        if !report.is_empty() {
            self.violations.push(format!("{}\n{report}", context()));
        }
    }

    fn check_locality(
        &mut self,
        base: &Flow,
        g: &FlowGeometry,
        own_clouds: &[i32],
        context: impl FnOnce() -> String,
    ) {
        let f = &g.flow;
        let mut problems = Vec::new();
        if f.uid != base.uid
            || f.name != base.name
            || f.label_side != base.label_side
            || f.compat != base.compat
            || f.label_compat != base.label_compat
        {
            problems.push("non-geometry flow fields changed".to_string());
        }
        for moved in &g.clouds {
            if !own_clouds.contains(&moved.uid) {
                problems.push(format!(
                    "moved cloud {}, which is not this flow's terminal",
                    moved.uid
                ));
            }
        }
        if !problems.is_empty() {
            self.locality
                .push(format!("{}: {}", context(), problems.join("; ")));
        }
    }

    fn continuity(
        &mut self,
        prev: Option<&Frame>,
        cur: &Flow,
        shape: &str,
        step: f64,
        context: impl FnOnce() -> String,
    ) {
        let Some(prev) = prev else {
            return;
        };
        let d =
            hausdorff(&prev.flow, cur, 2.0).max((prev.flow.x - cur.x).hypot(prev.flow.y - cur.y));
        if d <= 3.0 * step + 1.0 {
            return;
        }
        self.jumps += 1;
        if prev.shape != shape {
            self.shape_jumps += 1;
        } else {
            self.same_shape_jumps
                .push(format!("{} d={d:.2} {shape}", context()));
        }
    }
}

/// The previous frame of one flow: its geometry and route shape.
struct Frame {
    flow: Flow,
    shape: String,
}

fn by_uid(view: &[ViewElement]) -> HashMap<i32, &ViewElement> {
    view.iter().map(|e| (e.get_uid(), e)).collect()
}

fn stocks_of(view: &[ViewElement]) -> Vec<&Stock> {
    view.iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn flows_of(view: &[ViewElement]) -> Vec<&Flow> {
    view.iter()
        .filter_map(|e| match e {
            ViewElement::Flow(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn center(s: &Stock) -> Point {
    Point::new(s.x, s.y)
}

fn point(p: &crate::datamodel::view_element::FlowPoint) -> Point {
    Point::new(p.x, p.y)
}

fn next_uid(view: &[ViewElement]) -> i32 {
    view.iter().map(ViewElement::get_uid).max().unwrap_or(0) + 1
}

fn inside_any_stock(p: Point, view: &[ViewElement]) -> bool {
    stocks_of(view)
        .iter()
        .any(|s| (p.x - s.x).abs() < STOCK_WIDTH / 2.0 && (p.y - s.y).abs() < STOCK_HEIGHT / 2.0)
}

/// Other flows' endpoints attached to `stock`, moved by `delta` (the stock's
/// own move this frame).
fn endpoints_on(view: &[ViewElement], stock: i32, except: i32, delta: Point) -> Vec<Point> {
    let mut out = Vec::new();
    for f in flows_of(view) {
        if f.uid == except || f.points.len() < 2 {
            continue;
        }
        for p in [&f.points[0], &f.points[f.points.len() - 1]] {
            if p.attached_to_uid == Some(stock) {
                out.push(Point::new(p.x + delta.x, p.y + delta.y));
            }
        }
    }
    out
}

fn fixed_endpoints(view: &[ViewElement], fixed: &Terminal, except: i32) -> Vec<Point> {
    match *fixed {
        Terminal::Stock { uid, .. } => endpoints_on(view, uid, except, Point::new(0.0, 0.0)),
        Terminal::Free { .. } => Vec::new(),
    }
}

fn own_clouds(terminals: [&Terminal; 2]) -> Vec<i32> {
    terminals
        .iter()
        .filter_map(|t| match t {
            Terminal::Free { cloud: Some(c), .. } => Some(c.uid),
            _ => None,
        })
        .collect()
}

/// The route's shape: the faces its stock endpoints use and its segment
/// directions.
fn shape_of(f: &Flow, view: &[ViewElement]) -> String {
    let lookup = by_uid(view);
    let n = f.points.len();
    let face =
        |i: usize, j: usize| match f.points[i].attached_to_uid.and_then(|uid| lookup.get(&uid)) {
            Some(ViewElement::Stock(s)) => face_name(s, &f.points[i], &f.points[j]),
            _ => "-",
        };
    format!("{} {} {}", face(0, 1), directions(f), face(n - 1, n - 2))
}

fn random_delta(rng: &mut Rng) -> Point {
    let angle = rng.float(0.0, 2.0 * std::f64::consts::PI);
    let magnitude = rng.float(5.0, 250.0);
    Point::new(magnitude * angle.cos(), magnitude * angle.sin())
}

fn scaled(delta: Point, k: usize) -> Point {
    Point::new(
        delta.x * k as f64 / FRAMES as f64,
        delta.y * k as f64 / FRAMES as f64,
    )
}

fn move_stock(tally: &mut Tally, view: &[ViewElement], stock: i32, delta: Point, context: &str) {
    let lookup = by_uid(view);
    let Some(ViewElement::Stock(base)) = lookup.get(&stock).copied() else {
        return;
    };
    let attached: Vec<&Flow> = flows_of(view)
        .into_iter()
        .filter(|f| {
            f.points.len() >= 2
                && (f.points[0].attached_to_uid == Some(stock)
                    || f.points[f.points.len() - 1].attached_to_uid == Some(stock))
        })
        .collect();
    let step = delta.x.hypot(delta.y) / FRAMES as f64;
    let mut prev: HashMap<i32, Frame> = HashMap::new();
    for k in 0..=FRAMES {
        let d = scaled(delta, k);
        let mut moved = base.clone();
        moved.x += d.x;
        moved.y += d.y;
        let obstacles: Vec<Point> = stocks_of(view)
            .iter()
            .map(|s| {
                if s.uid == stock {
                    center(&moved)
                } else {
                    center(s)
                }
            })
            .collect();
        let mut next = patched(view, [ViewElement::Stock(moved.clone())], &[]);
        let mut routed = Vec::new();
        for f in &attached {
            let n = f.points.len();
            let t = flow_terminals(f, |uid| lookup.get(&uid).copied());
            let source_on = f.points[0].attached_to_uid == Some(stock);
            let sink_on = f.points[n - 1].attached_to_uid == Some(stock);
            let g = if source_on && sink_on {
                FlowGeometry {
                    flow: translate(f, d),
                    clouds: Default::default(),
                }
            } else {
                let (end, endpoint, adjacent, fixed) = if source_on {
                    (FlowEnd::Source, &f.points[0], &f.points[1], t.sink)
                } else {
                    (FlowEnd::Sink, &f.points[n - 1], &f.points[n - 2], t.source)
                };
                let terminal = stock_terminal(
                    stock,
                    center(&moved),
                    Some(point(endpoint)),
                    Some(point(adjacent)),
                    center(base),
                );
                let mut occupied = endpoints_on(view, stock, f.uid, d);
                occupied.extend(fixed_endpoints(view, &fixed, f.uid));
                route_end(f, end, terminal, fixed, &occupied, obstacles.as_slice())
            };
            tally.check_locality(f, &g, &[], || format!("{context} flow {} k {k}", f.uid));
            next = applied(&next, &g, [], &[]);
            routed.push(f.uid);
        }
        tally.check_frame(&next, &routed, || format!("{context} k {k}"));
        for &uid in &routed {
            let f = flow_of(&next, uid);
            let shape = shape_of(f, &next);
            tally.continuity(prev.get(&uid), f, &shape, step, || {
                format!("{context} flow {uid} k {k}")
            });
            prev.insert(
                uid,
                Frame {
                    flow: f.clone(),
                    shape,
                },
            );
        }
    }
}

fn drag_end(tally: &mut Tally, view: &[ViewElement], rng: &mut Rng, detach: bool, seed: u32) {
    let lookup = by_uid(view);
    let mut candidates: Vec<(&Flow, FlowEnd)> = Vec::new();
    for f in flows_of(view) {
        let t = flow_terminals(f, |uid| lookup.get(&uid).copied());
        for (end, terminal) in [(FlowEnd::Source, &t.source), (FlowEnd::Sink, &t.sink)] {
            if terminal.is_free() != detach {
                candidates.push((f, end));
            }
        }
    }
    if candidates.is_empty() {
        return;
    }
    let (f, end) = rng.pick(&candidates);
    let t = flow_terminals(f, |uid| lookup.get(&uid).copied());
    let n = f.points.len();
    let (endpoint, own, fixed) = match end {
        FlowEnd::Source => (point(&f.points[0]), t.source, t.sink),
        FlowEnd::Sink => (point(&f.points[n - 1]), t.sink, t.source),
    };
    let (cloud_ref, added) = match own {
        Terminal::Free { cloud: Some(c), .. } => (c, None),
        _ => {
            let uid = next_uid(view);
            (CloudRef { uid, at: endpoint }, Some(uid))
        }
    };
    let delta = random_delta(rng);
    let step = delta.x.hypot(delta.y) / FRAMES as f64;
    let obstacles: Vec<Point> = stocks_of(view).iter().map(|s| center(s)).collect();
    let occupied = fixed_endpoints(view, &fixed, f.uid);
    let mut prev: Option<Frame> = None;
    let op = if detach { "detach" } else { "dragCloud" };
    for k in 1..=FRAMES {
        let d = scaled(delta, k);
        let p = Point::new(endpoint.x + d.x, endpoint.y + d.y);
        if inside_any_stock(p, view) {
            prev = None;
            continue;
        }
        let g = route_end(
            f,
            end,
            free_terminal(p, Some(cloud_ref)),
            fixed,
            &occupied,
            obstacles.as_slice(),
        );
        tally.check_locality(f, &g, &[cloud_ref.uid], || {
            format!("seed {seed} {op} flow {} k {k}", f.uid)
        });
        // A detached end's new cloud is added by the planner; the core reports it moved onto the endpoint.
        let also = match added {
            Some(uid) => load(vec![cloud(uid, f.uid, p.x, p.y)]),
            None => Vec::new(),
        };
        let next = applied(view, &g, also, &[]);
        tally.check_frame(&next, &[f.uid], || {
            format!("seed {seed} {op} flow {} k {k}", f.uid)
        });
        let cur = flow_of(&next, f.uid);
        let shape = shape_of(cur, &next);
        tally.continuity(prev.as_ref(), cur, &shape, step, || {
            format!("seed {seed} {op} flow {} k {k}", f.uid)
        });
        prev = Some(Frame {
            flow: cur.clone(),
            shape,
        });
    }
}

fn sweep(op: GestureOp, seed: u32) -> Tally {
    let mut tally = Tally::default();
    let mut rng = Rng::new(seed.wrapping_mul(7919).wrapping_add(op as u32 + 1));
    let scene = strict_scene(seed);
    let view = &scene.elements;
    let lookup = by_uid(view);
    let stocks = stocks_of(view);
    let flows = flows_of(view);
    let obstacles: Vec<Point> = stocks.iter().map(|s| center(s)).collect();
    match op {
        GestureOp::MoveStock => {
            let stock = rng.pick(&stocks).uid;
            let delta = random_delta(&mut rng);
            move_stock(
                &mut tally,
                view,
                stock,
                delta,
                &format!("seed {seed} moveStock S{stock}"),
            );
        }
        GestureOp::DragCloud => drag_end(&mut tally, view, &mut rng, false, seed),
        GestureOp::Detach => drag_end(&mut tally, view, &mut rng, true, seed),
        GestureOp::Reattach => {
            for f in &flows {
                let t = flow_terminals(f, |uid| lookup.get(&uid).copied());
                for end in FlowEnd::ALL {
                    let (own, fixed) = match end {
                        FlowEnd::Source => (t.source, t.sink),
                        FlowEnd::Sink => (t.sink, t.source),
                    };
                    let Terminal::Free {
                        cloud: Some(own_cloud),
                        ..
                    } = own
                    else {
                        continue;
                    };
                    for s in &stocks {
                        if matches!(fixed, Terminal::Stock { uid, .. } if uid == s.uid) {
                            continue;
                        }
                        let mut occupied = endpoints_on(view, s.uid, f.uid, Point::new(0.0, 0.0));
                        occupied.extend(fixed_endpoints(view, &fixed, f.uid));
                        let g = route_end(
                            f,
                            end,
                            target_stock_terminal(s.uid, center(s)),
                            fixed,
                            &occupied,
                            obstacles.as_slice(),
                        );
                        let context =
                            || format!("seed {seed} reattach flow {} -> S{}", f.uid, s.uid);
                        tally.check_locality(f, &g, &[], context);
                        let next = applied(view, &g, [], &[own_cloud.uid]);
                        tally.check_frame(&next, &[f.uid], context);
                    }
                }
            }
        }
        GestureOp::Create => {
            let s = rng.pick(&stocks);
            let delta = random_delta(&mut rng);
            let step = delta.x.hypot(delta.y) / FRAMES as f64;
            let uid = next_uid(view);
            let draft = match ViewElement::from(flow(uid, (s.x, s.y), &[])) {
                ViewElement::Flow(f) => f,
                _ => unreachable!(),
            };
            let sink = CloudRef {
                uid: uid + 1,
                at: center(s),
            };
            let occupied = endpoints_on(view, s.uid, draft.uid, Point::new(0.0, 0.0));
            let mut prev: Option<Frame> = None;
            for k in 1..=FRAMES {
                let d = scaled(delta, k);
                let p = Point::new(s.x + d.x, s.y + d.y);
                if inside_any_stock(p, view) {
                    prev = None;
                    continue;
                }
                let g = route(
                    target_stock_terminal(s.uid, center(s)),
                    free_terminal(p, Some(sink)),
                    &draft,
                    FlowEnd::Source,
                    &occupied,
                    obstacles.as_slice(),
                );
                let next = applied(
                    view,
                    &g,
                    load(vec![cloud(sink.uid, draft.uid, p.x, p.y)]),
                    &[],
                );
                tally.check_frame(&next, &[draft.uid], || format!("seed {seed} create k {k}"));
                let cur = flow_of(&next, draft.uid);
                let shape = shape_of(cur, &next);
                tally.continuity(prev.as_ref(), cur, &shape, step, || {
                    format!("seed {seed} create k {k}")
                });
                prev = Some(Frame {
                    flow: cur.clone(),
                    shape,
                });
            }
        }
        GestureOp::Offset => {
            let f = rng.pick(&flows);
            let i = rng.int(0.0, (f.points.len() - 2) as f64) as usize;
            let (_, hold) = segment_hold(&path_of(f), i);
            let amount = rng.float(-150.0, 150.0);
            let t = flow_terminals(f, |uid| lookup.get(&uid).copied());
            let clouds = own_clouds(t.both());
            let step = amount.abs() / FRAMES as f64;
            let mut prev: Option<Frame> = None;
            for k in 0..=FRAMES {
                let g = offset_segment(
                    f,
                    i,
                    hold + amount * k as f64 / FRAMES as f64,
                    &t,
                    obstacles.as_slice(),
                );
                let context = || format!("seed {seed} offset flow {} segment {i} k {k}", f.uid);
                tally.check_locality(f, &g, &clouds, context);
                let next = applied(view, &g, [], &[]);
                tally.check_frame(&next, &[f.uid], context);
                let cur = flow_of(&next, f.uid);
                let shape = shape_of(cur, &next);
                tally.continuity(prev.as_ref(), cur, &shape, step, context);
                prev = Some(Frame {
                    flow: cur.clone(),
                    shape,
                });
            }
        }
        GestureOp::Slide => {
            let f = rng.pick(&flows);
            let delta = random_delta(&mut rng);
            let step = delta.x.hypot(delta.y) / FRAMES as f64;
            let mut prev: Option<Frame> = None;
            for k in 0..=FRAMES {
                let slid = slide_valve(f, scaled(delta, k));
                let next = patched(view, [ViewElement::Flow(slid.clone())], &[]);
                let context = || format!("seed {seed} slide flow {} k {k}", f.uid);
                tally.check_frame(&next, &[f.uid], context);
                tally.continuity(prev.as_ref(), &slid, "slide", step, context);
                prev = Some(Frame {
                    flow: slid,
                    shape: "slide".to_string(),
                });
            }
        }
        GestureOp::HealImported => {
            let imported = imported_scene(seed);
            let iview = &imported.elements;
            let ilookup = by_uid(iview);
            let iobstacles: Vec<Point> = stocks_of(iview).iter().map(|s| center(s)).collect();
            let mut healed = iview.clone();
            let mut routed = Vec::new();
            for f in flows_of(iview) {
                let t = flow_terminals(f, |uid| ilookup.get(&uid).copied());
                let g = heal(f, &t, iobstacles.as_slice());
                tally.check_locality(f, &g, &own_clouds(t.both()), || {
                    format!("seed {seed} heal flow {}", f.uid)
                });
                healed = applied(&healed, &g, [], &[]);
                // An unattached flow has nothing to attach to; creating its
                // clouds is the planner's job.
                if !imported
                    .mutations
                    .iter()
                    .any(|m| m.flow_uid == f.uid && m.shape == ImportShape::UnattachedFlow)
                {
                    routed.push(f.uid);
                }
            }
            tally.check_frame(&healed, &routed, || format!("seed {seed} healed"));
            let stock = rng.pick(&stocks_of(&healed)).uid;
            let delta = random_delta(&mut rng);
            move_stock(
                &mut tally,
                &healed,
                stock,
                delta,
                &format!("seed {seed} healed moveStock S{stock}"),
            );
        }
    }
    tally
}

static TALLIES: LazyLock<HashMap<GestureOp, Tally>> = LazyLock::new(|| {
    GestureOp::ALL
        .par_iter()
        .map(|&op| {
            let tally = (1..=SEEDS)
                .into_par_iter()
                .map(|seed| sweep(op, seed))
                .reduce(Tally::default, Tally::merge);
            (op, tally)
        })
        .collect()
});

#[test]
fn every_frame_of_every_gesture_holds_the_strict_invariants_and_locality() {
    let mut failures = Vec::new();
    for op in GestureOp::ALL {
        let tally = &TALLIES[&op];
        if tally.frames < SEEDS as usize {
            failures.push(format!(
                "{}: only {} frames checked",
                op.name(),
                tally.frames
            ));
        }
        for v in tally.violations.iter().take(4) {
            failures.push(format!("{}: {v}", op.name()));
        }
        for l in tally.locality.iter().take(4) {
            failures.push(format!("{}: {l}", op.name()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn jumps_are_route_shape_transitions_within_the_measured_residue() {
    let mut failures = Vec::new();
    for op in GestureOp::ALL {
        let tally = &TALLIES[&op];
        let budget = op.same_shape_budget();
        if tally.same_shape_jumps.len() != budget {
            failures.push(format!(
                "{}: {} same-shape jumps (budget {budget}) over {} frames, {} shape transitions\n  {}",
                op.name(),
                tally.same_shape_jumps.len(),
                tally.frames,
                tally.shape_jumps,
                tally.same_shape_jumps.iter().take(5).cloned().collect::<Vec<_>>().join("\n  ")
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
