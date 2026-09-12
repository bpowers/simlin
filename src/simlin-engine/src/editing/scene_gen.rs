// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Seeded scene generator for the editing core's property tests.
//!
//! `strict_scene` builds a view whose every flow holds G1-G8: a hub stock with
//! several flows on random faces (endpoints sharing a face at least
//! `PIPE_SPACING` apart), flows starting at free clouds, each flow a straight,
//! L, Z or bracket route ending at a cloud or at a new stock entered through a
//! perpendicular face, plus auxes, sometimes a module, an alias, and links.
//! `imported_scene` starts from a strict scene and applies violation shapes the
//! corpus measurement found in imported models, recording for each the arms
//! strict mode must report.
//!
//! The generator's validity rules are its own geometry, never the checker's or
//! the core's: a test that generated with the code it checks would agree with
//! itself by construction. The PRNG is mulberry32, so a seed determines its
//! scene everywhere and across dependency upgrades.

use std::collections::HashSet;

use crate::datamodel::ViewElement;
use crate::diagram::constants::{CLOUD_RADIUS, STOCK_HEIGHT, STOCK_WIDTH};
use crate::json;

use super::geometry::Face;
use super::invariants::{
    CORNER_CLEARANCE, FlowArm, MIN_SEGMENT, MIN_SINK_SEGMENT, VALVE_CLAMP_MARGIN,
};

/// How far apart the generator keeps the flow ends it places on one face: the
/// design plan's pipe spacing. A generation choice, not an invariant the
/// checker holds a scene to.
const PIPE_SPACING: f64 = 10.0;

// ---------------------------------------------------------------------------
// PRNG

/// mulberry32: small, fast, and the same sequence on every platform.
pub(crate) struct Rng {
    state: u32,
}

impl Rng {
    pub(crate) fn new(seed: u32) -> Rng {
        Rng { state: seed }
    }

    fn next_unit(&mut self) -> f64 {
        self.state = self.state.wrapping_add(0x6d2b_79f5);
        let a = self.state;
        let mut t = (a ^ (a >> 15)).wrapping_mul(1 | a);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        f64::from(t ^ (t >> 14)) / 4_294_967_296.0
    }

    /// Uniform in `[a, b)`.
    pub(crate) fn float(&mut self, a: f64, b: f64) -> f64 {
        a + (b - a) * self.next_unit()
    }

    /// A uniform integer in `[a, b]`, as a coordinate.
    pub(crate) fn int(&mut self, a: f64, b: f64) -> f64 {
        self.float(a, b + 1.0).floor()
    }

    pub(crate) fn chance(&mut self, p: f64) -> bool {
        self.next_unit() < p
    }

    pub(crate) fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[(self.next_unit() * items.len() as f64).floor() as usize]
    }

    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.int(0.0, i as f64) as usize;
            items.swap(i, j);
        }
    }
}

// ---------------------------------------------------------------------------
// Records

#[derive(Clone, Copy, PartialEq)]
pub(crate) struct Pt {
    pub x: f64,
    pub y: f64,
}

const fn pt(x: f64, y: f64) -> Pt {
    Pt { x, y }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FlowShape {
    Straight,
    L,
    Z,
    Bracket,
}

impl FlowShape {
    pub(crate) const ALL: [FlowShape; 4] = [
        FlowShape::Straight,
        FlowShape::L,
        FlowShape::Z,
        FlowShape::Bracket,
    ];
}

/// Where a generated walk starts: the hub stock at the flow's source or at its
/// sink, or a free cloud (always the source).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RouteStart {
    HubSource,
    HubSink,
    FreeCloud,
}

impl RouteStart {
    pub(crate) const ALL: [RouteStart; 3] = [
        RouteStart::HubSource,
        RouteStart::HubSink,
        RouteStart::FreeCloud,
    ];
}

/// What the generator built for one flow, so a test can check the view realizes
/// it and count coverage over what was built.
#[derive(Clone, Copy)]
pub(crate) struct RouteRecord {
    pub flow_uid: i32,
    pub shape: FlowShape,
    pub start: RouteStart,
    /// The new stock the walk ended at and the face it entered through; `None`
    /// when the walk ended at a cloud.
    pub far: Option<(i32, Face)>,
}

pub(crate) struct Scene {
    pub elements: Vec<ViewElement>,
    pub hub: i32,
    pub routes: Vec<RouteRecord>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ImportShape {
    /// A stock endpoint moved 5-60px outward along its stub (Vensim stocks
    /// larger than 45x35 put endpoints on their real faces).
    OffFaceAxis,
    /// A straight stock-cloud flow shifted so its stock endpoint sits exactly on
    /// a corner.
    CornerEndpoint,
    /// A cloud moved up to `CLOUD_RADIUS` off its endpoint.
    CloudOffset,
    /// The valve moved 1-20px perpendicular off its segment.
    ValveOffPath,
    /// A straight cloud-cloud flow's sink moved 0.5-2px across.
    SlightlyDiagonal,
    /// A cloud-cloud flow with both attachments and its clouds removed (Vensim
    /// fallback flows).
    UnattachedFlow,
}

impl ImportShape {
    pub(crate) const ALL: [ImportShape; 6] = [
        ImportShape::OffFaceAxis,
        ImportShape::CornerEndpoint,
        ImportShape::CloudOffset,
        ImportShape::ValveOffPath,
        ImportShape::SlightlyDiagonal,
        ImportShape::UnattachedFlow,
    ];
}

pub(crate) struct Mutation {
    pub shape: ImportShape,
    pub flow_uid: i32,
    /// The arms strict mode must report on the flow, one entry per occurrence.
    pub arms: Vec<FlowArm>,
}

pub(crate) struct ImportedScene {
    pub elements: Vec<ViewElement>,
    pub mutations: Vec<Mutation>,
}

const HALF_WIDTH: f64 = STOCK_WIDTH / 2.0;
const HALF_HEIGHT: f64 = STOCK_HEIGHT / 2.0;

/// Clouds keep this far from stock bodies so that no later mutation (a cloud
/// moved by up to `CLOUD_RADIUS`) can land inside one.
const CLOUD_STOCK_GAP: f64 = CLOUD_RADIUS + 6.0;
const STOCK_STOCK_GAP: f64 = 20.0;

struct StockRecord {
    uid: i32,
    center: Pt,
    /// Positions along each face (by `face_index`) already used by endpoints.
    slots: [Vec<f64>; 4],
}

struct CloudRecord {
    uid: i32,
    flow_uid: i32,
    center: Pt,
}

#[derive(Clone, Copy)]
enum End {
    Stock { stock: usize, face: Face },
    Cloud { cloud: usize },
}

struct FlowRecord {
    uid: i32,
    points: Vec<Pt>,
    source: End,
    sink: End,
    valve: Pt,
    /// Both endpoints carry no attachment and the flow's clouds are gone.
    detached: bool,
}

struct Builder {
    next_uid: i32,
    hub: i32,
    routes: Vec<RouteRecord>,
    stocks: Vec<StockRecord>,
    clouds: Vec<CloudRecord>,
    flows: Vec<FlowRecord>,
    auxes: Vec<(i32, Pt)>,
    modules: Vec<(i32, Pt)>,
    aliases: Vec<(i32, i32, Pt)>,
    links: Vec<(i32, i32, i32, Option<f64>)>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            next_uid: 1,
            hub: 0,
            routes: Vec::new(),
            stocks: Vec::new(),
            clouds: Vec::new(),
            flows: Vec::new(),
            auxes: Vec::new(),
            modules: Vec::new(),
            aliases: Vec::new(),
            links: Vec::new(),
        }
    }

    fn uid(&mut self) -> i32 {
        let uid = self.next_uid;
        self.next_uid += 1;
        uid
    }

    fn end_uid(&self, end: End) -> i32 {
        match end {
            End::Stock { stock, .. } => self.stocks[stock].uid,
            End::Cloud { cloud } => self.clouds[cloud].uid,
        }
    }
}

// ---------------------------------------------------------------------------
// Strict scenes

pub(crate) fn strict_scene(seed: u32) -> Scene {
    let mut rng = Rng::new(seed);
    let b = build_scene(&mut rng);
    Scene {
        elements: elements_of(&b),
        hub: b.hub,
        routes: b.routes,
    }
}

fn build_scene(rng: &mut Rng) -> Builder {
    loop {
        let mut b = Builder::new();
        let hub_center = pt(rng.int(350.0, 550.0), rng.int(350.0, 550.0));
        let hub = add_stock(&mut b, hub_center);
        b.hub = b.stocks[hub].uid;
        for _ in 0..rng.int(1.0, 5.0) as usize {
            add_flow(&mut b, rng, Start::Stock(hub));
        }
        for _ in 0..rng.int(0.0, 3.0) as usize {
            add_flow(&mut b, rng, Start::Free);
        }
        if b.flows.is_empty() {
            continue;
        }
        add_auxes_modules_aliases_links(&mut b, rng);
        return b;
    }
}

fn add_stock(b: &mut Builder, center: Pt) -> usize {
    let uid = b.uid();
    b.stocks.push(StockRecord {
        uid,
        center,
        slots: Default::default(),
    });
    b.stocks.len() - 1
}

#[derive(Clone, Copy)]
enum Start {
    Stock(usize),
    Free,
}

fn face_index(face: Face) -> usize {
    match face {
        Face::Left => 0,
        Face::Right => 1,
        Face::Top => 2,
        Face::Bottom => 3,
    }
}

fn outward(face: Face) -> Pt {
    match face {
        Face::Left => pt(-1.0, 0.0),
        Face::Right => pt(1.0, 0.0),
        Face::Top => pt(0.0, -1.0),
        Face::Bottom => pt(0.0, 1.0),
    }
}

fn face_length(face: Face) -> f64 {
    match face {
        Face::Left | Face::Right => STOCK_HEIGHT,
        Face::Top | Face::Bottom => STOCK_WIDTH,
    }
}

/// The point `along` px from the face's top (left and right faces) or left
/// (top and bottom faces) corner.
fn face_point(center: Pt, face: Face, along: f64) -> Pt {
    match face {
        Face::Left => pt(center.x - HALF_WIDTH, center.y - HALF_HEIGHT + along),
        Face::Right => pt(center.x + HALF_WIDTH, center.y - HALF_HEIGHT + along),
        Face::Top => pt(center.x - HALF_WIDTH + along, center.y - HALF_HEIGHT),
        Face::Bottom => pt(center.x - HALF_WIDTH + along, center.y + HALF_HEIGHT),
    }
}

fn turn(d: Pt, left: bool) -> Pt {
    // Normalize -0 so directions compare equal to their spelled literals.
    let z = |v: f64| if v == 0.0 { 0.0 } else { v };
    if left {
        pt(z(d.y), z(-d.x))
    } else {
        pt(z(-d.y), z(d.x))
    }
}

fn opposite(d: Pt) -> Face {
    if d.x > 0.0 {
        Face::Left
    } else if d.x < 0.0 {
        Face::Right
    } else if d.y > 0.0 {
        Face::Top
    } else {
        Face::Bottom
    }
}

/// Segment directions and lengths for a shape, in walk order. The segment that
/// becomes the flow's final segment (the walk is reversed when the start stock
/// is the sink) gets `MIN_SINK_SEGMENT`; every other segment `MIN_SEGMENT`. A
/// bracket is stub, riser, run, riser back, stub.
fn shape_walk(rng: &mut Rng, shape: FlowShape, d0: Pt, reversed: bool) -> Vec<(Pt, f64)> {
    let sink_min = MIN_SINK_SEGMENT.ceil();
    let min = |index: usize, count: usize| {
        if (if reversed { 0 } else { count - 1 }) == index {
            sink_min
        } else {
            MIN_SEGMENT
        }
    };
    let side = rng.chance(0.5);
    match shape {
        FlowShape::Straight => vec![(d0, rng.int(30.0_f64.max(min(0, 1)), 200.0))],
        FlowShape::L => vec![
            (d0, rng.int(20.0_f64.max(min(0, 2)), 150.0)),
            (turn(d0, side), rng.int(20.0_f64.max(min(1, 2)), 150.0)),
        ],
        FlowShape::Z => vec![
            (d0, rng.int(15.0_f64.max(min(0, 3)), 120.0)),
            (turn(d0, side), rng.int(20.0, 100.0)),
            (d0, rng.int(20.0_f64.max(min(2, 3)), 120.0)),
        ],
        FlowShape::Bracket => {
            let riser = rng.int(20.0, 60.0);
            vec![
                (d0, rng.int(min(0, 5), min(0, 5) + 8.0)),
                (turn(d0, side), riser),
                (d0, rng.int(30.0, 150.0)),
                (turn(d0, !side), riser),
                (d0, rng.int(min(4, 5), min(4, 5) + 8.0)),
            ]
        }
    }
}

fn add_flow(b: &mut Builder, rng: &mut Rng, start: Start) -> bool {
    for _attempt in 0..40 {
        let shape = rng.pick(&FlowShape::ALL);
        // When the walk starts at a stock that is the flow's SINK, the walk is
        // reversed into points; choose that before sizing segments.
        let reversed = matches!(start, Start::Stock(_)) && rng.chance(0.5);
        let (p0, d0, start_slot) = match start {
            Start::Stock(s) => {
                let face = rng.pick(&Face::ALL);
                let Some(along) = pick_slot(rng, &b.stocks[s], face) else {
                    continue;
                };
                (
                    face_point(b.stocks[s].center, face, along),
                    outward(face),
                    Some((face, along)),
                )
            }
            Start::Free => {
                let p0 = pt(rng.int(0.0, 900.0), rng.int(0.0, 900.0));
                (p0, outward(rng.pick(&Face::ALL)), None)
            }
        };
        let walk = shape_walk(rng, shape, d0, reversed);
        let mut pts = vec![p0];
        for &(d, length) in &walk {
            let last = pts[pts.len() - 1];
            pts.push(pt(last.x + d.x * length, last.y + d.y * length));
        }
        let end = pts[pts.len() - 1];
        let final_direction = walk[walk.len() - 1].0;

        let far_stock = if rng.chance(0.35) {
            let face = opposite(final_direction);
            let along = rng.int(CORNER_CLEARANCE, face_length(face) - CORNER_CLEARANCE);
            // The final walk point is at `along` on `face` of the new stock.
            let probe = face_point(pt(0.0, 0.0), face, along);
            Some((pt(end.x - probe.x, end.y - probe.y), face, along))
        } else {
            None
        };

        let mut terminal_centers = Vec::new();
        if let Start::Stock(s) = start {
            terminal_centers.push(b.stocks[s].center);
        }
        if let Some((center, _, _)) = far_stock {
            terminal_centers.push(center);
        }
        if !path_is_clear(b, &pts, &terminal_centers) {
            continue;
        }
        if let Some((center, _, _)) = far_stock {
            let body = stock_box(center);
            if b.stocks
                .iter()
                .any(|s| box_gap(stock_box(s.center), body) < STOCK_STOCK_GAP)
                || b.clouds
                    .iter()
                    .any(|c| distance_to_box(c.center, body) < CLOUD_STOCK_GAP)
                || b.flows.iter().any(|f| {
                    f.points
                        .windows(2)
                        .any(|w| segment_hits_box(w[0], w[1], inflate_box(body, 2.0)))
                })
            {
                continue;
            }
        }
        if matches!(start, Start::Free)
            && b.stocks
                .iter()
                .any(|s| distance_to_box(p0, stock_box(s.center)) < CLOUD_STOCK_GAP)
        {
            continue;
        }
        if far_stock.is_none()
            && b.stocks
                .iter()
                .any(|s| distance_to_box(end, stock_box(s.center)) < CLOUD_STOCK_GAP)
        {
            continue;
        }

        let flow_uid = b.uid();
        let walk_start = match (start, start_slot) {
            (Start::Stock(s), Some((face, along))) => {
                b.stocks[s].slots[face_index(face)].push(along);
                End::Stock { stock: s, face }
            }
            _ => {
                let uid = b.uid();
                b.clouds.push(CloudRecord {
                    uid,
                    flow_uid,
                    center: p0,
                });
                End::Cloud {
                    cloud: b.clouds.len() - 1,
                }
            }
        };
        let (walk_end, far) = match far_stock {
            Some((center, face, along)) => {
                let s = add_stock(b, center);
                b.stocks[s].slots[face_index(face)].push(along);
                (End::Stock { stock: s, face }, Some((b.stocks[s].uid, face)))
            }
            None => {
                let uid = b.uid();
                b.clouds.push(CloudRecord {
                    uid,
                    flow_uid,
                    center: end,
                });
                (
                    End::Cloud {
                        cloud: b.clouds.len() - 1,
                    },
                    None,
                )
            }
        };
        let points: Vec<Pt> = if reversed {
            pts.into_iter().rev().collect()
        } else {
            pts
        };
        let valve = place_valve(rng, &points);
        b.flows.push(FlowRecord {
            uid: flow_uid,
            points,
            source: if reversed { walk_end } else { walk_start },
            sink: if reversed { walk_start } else { walk_end },
            valve,
            detached: false,
        });
        b.routes.push(RouteRecord {
            flow_uid,
            shape,
            start: match start {
                Start::Free => RouteStart::FreeCloud,
                Start::Stock(_) if reversed => RouteStart::HubSink,
                Start::Stock(_) => RouteStart::HubSource,
            },
            far,
        });
        return true;
    }
    false
}

fn pick_slot(rng: &mut Rng, stock: &StockRecord, face: Face) -> Option<f64> {
    let used = &stock.slots[face_index(face)];
    let length = face_length(face);
    for _ in 0..12 {
        let along = if rng.chance(0.4) {
            rng.pick(&[length / 2.0, length / 4.0, 3.0 * length / 4.0])
        } else {
            rng.int(CORNER_CLEARANCE, length - CORNER_CLEARANCE)
        };
        if used.iter().all(|u| (u - along).abs() >= PIPE_SPACING) {
            return Some(along);
        }
    }
    None
}

/// A new path must not pass through any stock other than its terminals
/// (inflated by 2px so it never grazes one), must not enter its terminal
/// stocks' interiors, and must not touch another flow's endpoint cloud.
fn path_is_clear(b: &Builder, pts: &[Pt], terminal_centers: &[Pt]) -> bool {
    let is_terminal = |center: Pt| terminal_centers.contains(&center);
    for w in pts.windows(2) {
        for s in &b.stocks {
            let inflate = if is_terminal(s.center) { -0.01 } else { 2.0 };
            if segment_hits_box(w[0], w[1], inflate_box(stock_box(s.center), inflate)) {
                return false;
            }
        }
        for &c in terminal_centers {
            if !b.stocks.iter().any(|s| s.center == c)
                && segment_hits_box(w[0], w[1], inflate_box(stock_box(c), -0.01))
            {
                return false;
            }
        }
        if b.clouds
            .iter()
            .any(|c| distance_to_segment(c.center, w[0], w[1]) < CLOUD_RADIUS)
        {
            return false;
        }
    }
    true
}

fn place_valve(rng: &mut Rng, pts: &[Pt]) -> Pt {
    let length = path_length(pts);
    let s = if rng.chance(0.3) {
        length / 2.0
    } else {
        rng.float(VALVE_CLAMP_MARGIN + 1.0, length - VALVE_CLAMP_MARGIN - 1.0)
    };
    point_at_arc(pts, s)
}

fn clear_spot(b: &Builder, rng: &mut Rng) -> Option<Pt> {
    for _ in 0..30 {
        let p = pt(rng.int(0.0, 900.0), rng.int(0.0, 900.0));
        let near_stock = b
            .stocks
            .iter()
            .any(|s| distance_to_box(p, stock_box(s.center)) < 20.0);
        let near_path = b.flows.iter().any(|f| {
            f.points
                .windows(2)
                .any(|w| distance_to_segment(p, w[0], w[1]) < 15.0)
        });
        let near_cloud = b
            .clouds
            .iter()
            .any(|c| (c.center.x - p.x).hypot(c.center.y - p.y) < 25.0);
        if !near_stock && !near_path && !near_cloud {
            return Some(p);
        }
    }
    None
}

fn add_auxes_modules_aliases_links(b: &mut Builder, rng: &mut Rng) {
    for _ in 0..rng.int(1.0, 3.0) as usize {
        if let Some(p) = clear_spot(b, rng) {
            let uid = b.uid();
            b.auxes.push((uid, p));
        }
    }
    if rng.chance(0.3)
        && let Some(p) = clear_spot(b, rng)
    {
        let uid = b.uid();
        b.modules.push((uid, p));
    }
    let aliasable: Vec<i32> = b
        .stocks
        .iter()
        .map(|s| s.uid)
        .chain(b.auxes.iter().map(|a| a.0))
        .collect();
    if rng.chance(0.7)
        && !aliasable.is_empty()
        && let Some(p) = clear_spot(b, rng)
    {
        let uid = b.uid();
        let target = rng.pick(&aliasable);
        b.aliases.push((uid, target, p));
    }
    let linkable: Vec<i32> = b
        .stocks
        .iter()
        .map(|s| s.uid)
        .chain(b.flows.iter().map(|f| f.uid))
        .chain(b.auxes.iter().map(|a| a.0))
        .chain(b.modules.iter().map(|m| m.0))
        .chain(b.aliases.iter().map(|a| a.0))
        .collect();
    for _ in 0..rng.int(1.0, 4.0) as usize {
        if linkable.len() <= 1 {
            break;
        }
        let from = rng.pick(&linkable);
        let others: Vec<i32> = linkable.iter().copied().filter(|&u| u != from).collect();
        let to = rng.pick(&others);
        let uid = b.uid();
        let arc = if rng.chance(0.5) {
            None
        } else {
            Some(rng.int(-60.0, 60.0))
        };
        b.links.push((uid, from, to, arc));
    }
}

// ---------------------------------------------------------------------------
// Imported scenes

/// A strict scene with one to three import shapes applied, each to a different
/// flow. Every precondition is chosen so the mutation produces exactly its
/// recorded arms and nothing else.
pub(crate) fn imported_scene(seed: u32) -> ImportedScene {
    let mut rng = Rng::new(seed);
    loop {
        let mut b = build_scene(&mut rng);
        let want = rng.int(1.0, 3.0) as usize;
        let mut order = ImportShape::ALL;
        rng.shuffle(&mut order);
        let mut used = HashSet::new();
        let mut mutations = Vec::new();
        for shape in order {
            if mutations.len() >= want {
                break;
            }
            if let Some(m) = apply_import_shape(&mut b, &mut rng, shape, &used) {
                used.insert(m.flow_uid);
                mutations.push(m);
            }
        }
        if !mutations.is_empty() {
            return ImportedScene {
                elements: elements_of(&b),
                mutations,
            };
        }
    }
}

fn apply_import_shape(
    b: &mut Builder,
    rng: &mut Rng,
    shape: ImportShape,
    used: &HashSet<i32>,
) -> Option<Mutation> {
    let mut candidates: Vec<usize> = (0..b.flows.len())
        .filter(|&i| !used.contains(&b.flows[i].uid))
        .collect();
    rng.shuffle(&mut candidates);
    let mutation = |flow_uid: i32, arms: &[FlowArm]| {
        Some(Mutation {
            shape,
            flow_uid,
            arms: arms.to_vec(),
        })
    };
    match shape {
        ImportShape::OffFaceAxis => {
            for fi in candidates {
                let n = b.flows[fi].points.len();
                for index in [0, n - 1] {
                    let end = if index == 0 {
                        b.flows[fi].source
                    } else {
                        b.flows[fi].sink
                    };
                    if !matches!(end, End::Stock { .. }) {
                        continue;
                    }
                    let f = &b.flows[fi];
                    let adjacent = if index == 0 {
                        f.points[1]
                    } else {
                        f.points[n - 2]
                    };
                    let endpoint = f.points[index];
                    let stub_length = (adjacent.x - endpoint.x).hypot(adjacent.y - endpoint.y);
                    let valve_arc = arc_position(&f.points, f.valve);
                    let valve_from_end = if index == 0 {
                        valve_arc
                    } else {
                        path_length(&f.points) - valve_arc
                    };
                    let d = rng.int(5.0, 60.0);
                    if stub_length < d + MIN_SINK_SEGMENT + 1.0
                        || valve_from_end < d + VALVE_CLAMP_MARGIN + 1.0
                    {
                        continue;
                    }
                    let (ux, uy) = (
                        (adjacent.x - endpoint.x) / stub_length,
                        (adjacent.y - endpoint.y) / stub_length,
                    );
                    b.flows[fi].points[index] = pt(endpoint.x + ux * d, endpoint.y + uy * d);
                    return mutation(b.flows[fi].uid, &[FlowArm::OffFace]);
                }
            }
            None
        }
        ImportShape::CornerEndpoint => {
            for fi in candidates {
                let f = &b.flows[fi];
                if f.points.len() != 2 {
                    continue;
                }
                let (stock_end, cloud_end, index) = match (f.source, f.sink) {
                    (End::Stock { stock, face }, End::Cloud { cloud }) => ((stock, face), cloud, 0),
                    (End::Cloud { cloud }, End::Stock { stock, face }) => ((stock, face), cloud, 1),
                    _ => continue,
                };
                let (stock, face) = stock_end;
                let endpoint = f.points[index];
                let along = if rng.chance(0.5) {
                    0.0
                } else {
                    face_length(face)
                };
                let corner = face_point(b.stocks[stock].center, face, along);
                let shift = pt(corner.x - endpoint.x, corner.y - endpoint.y);
                let moved: Vec<Pt> = f
                    .points
                    .iter()
                    .map(|p| pt(p.x + shift.x, p.y + shift.y))
                    .collect();
                let cloud_center = pt(
                    b.clouds[cloud_end].center.x + shift.x,
                    b.clouds[cloud_end].center.y + shift.y,
                );
                if b.stocks.iter().enumerate().any(|(si, s)| {
                    si != stock
                        && segment_hits_box(
                            moved[0],
                            moved[1],
                            inflate_box(stock_box(s.center), 2.0),
                        )
                }) {
                    continue;
                }
                // A shifted cloud near a stock could land inside it or crowd the
                // G3/G6 exemptions, adding arms this mutation does not record.
                if b.stocks
                    .iter()
                    .any(|s| distance_to_box(cloud_center, stock_box(s.center)) < CLOUD_STOCK_GAP)
                {
                    continue;
                }
                let f = &mut b.flows[fi];
                f.points = moved;
                f.valve = pt(f.valve.x + shift.x, f.valve.y + shift.y);
                let uid = f.uid;
                b.clouds[cloud_end].center = cloud_center;
                return mutation(uid, &[FlowArm::CornerClearance]);
            }
            None
        }
        ImportShape::CloudOffset => {
            for fi in candidates {
                let cloud = match (b.flows[fi].source, b.flows[fi].sink) {
                    (End::Cloud { cloud }, _) | (_, End::Cloud { cloud }) => cloud,
                    _ => continue,
                };
                let r = rng.float(0.5, CLOUD_RADIUS);
                let angle = rng.float(0.0, 2.0 * std::f64::consts::PI);
                let base = b.clouds[cloud].center;
                let center = pt(base.x + r * angle.cos(), base.y + r * angle.sin());
                if b.stocks
                    .iter()
                    .any(|s| distance_to_box(center, stock_box(s.center)) < 1.0)
                {
                    continue;
                }
                b.clouds[cloud].center = center;
                return mutation(b.flows[fi].uid, &[FlowArm::CloudOffEndpoint]);
            }
            None
        }
        ImportShape::ValveOffPath => {
            for fi in candidates {
                let f = &b.flows[fi];
                let segment = segment_at_arc(&f.points, arc_position(&f.points, f.valve));
                let (a, c) = (f.points[segment], f.points[segment + 1]);
                let length = (c.x - a.x).hypot(c.y - a.y);
                let offset = rng.float(1.0, 20.0) * if rng.chance(0.5) { 1.0 } else { -1.0 };
                let valve = pt(
                    f.valve.x - ((c.y - a.y) / length) * offset,
                    f.valve.y + ((c.x - a.x) / length) * offset,
                );
                // Near a corner, moving the valve off its own segment can put it
                // on the adjacent one, where G8.valveOffPath would not fire.
                if distance_to_path(valve, &f.points) < 0.5 {
                    continue;
                }
                b.flows[fi].valve = valve;
                return mutation(b.flows[fi].uid, &[FlowArm::ValveOffPath]);
            }
            None
        }
        ImportShape::SlightlyDiagonal => {
            for fi in candidates {
                let f = &b.flows[fi];
                let (End::Cloud { .. }, End::Cloud { cloud: sink_cloud }) = (f.source, f.sink)
                else {
                    continue;
                };
                if f.points.len() != 2 {
                    continue;
                }
                let (a, c) = (f.points[0], f.points[1]);
                let length = (c.x - a.x).hypot(c.y - a.y);
                let t = arc_position(&f.points, f.valve) / length;
                let across = rng.float(0.5, 2.0) * if rng.chance(0.5) { 1.0 } else { -1.0 };
                let sink = pt(
                    c.x - ((c.y - a.y) / length) * across,
                    c.y + ((c.x - a.x) / length) * across,
                );
                let new_length = (sink.x - a.x).hypot(sink.y - a.y);
                if t * new_length < VALVE_CLAMP_MARGIN + 0.5
                    || (1.0 - t) * new_length < VALVE_CLAMP_MARGIN + 0.5
                {
                    continue;
                }
                let f = &mut b.flows[fi];
                f.points = vec![a, sink];
                f.valve = pt(a.x + (sink.x - a.x) * t, a.y + (sink.y - a.y) * t);
                let uid = f.uid;
                b.clouds[sink_cloud].center = sink;
                return mutation(uid, &[FlowArm::Diagonal]);
            }
            None
        }
        ImportShape::UnattachedFlow => {
            for fi in candidates {
                if let (End::Cloud { .. }, End::Cloud { .. }) =
                    (b.flows[fi].source, b.flows[fi].sink)
                {
                    b.flows[fi].detached = true;
                    return mutation(
                        b.flows[fi].uid,
                        &[FlowArm::UnattachedEndpoint, FlowArm::UnattachedEndpoint],
                    );
                }
            }
            None
        }
    }
}

// ---------------------------------------------------------------------------
// View assembly

fn elements_of(b: &Builder) -> Vec<ViewElement> {
    let mut out: Vec<json::ViewElement> = Vec::new();
    for s in &b.stocks {
        out.push(json::ViewElement::Stock(json::StockViewElement {
            uid: s.uid,
            name: format!("Stock {}", s.uid),
            x: s.center.x,
            y: s.center.y,
            label_side: String::new(),
        }));
    }
    let detached: HashSet<i32> = b
        .flows
        .iter()
        .filter(|f| f.detached)
        .map(|f| f.uid)
        .collect();
    for c in &b.clouds {
        if !detached.contains(&c.flow_uid) {
            out.push(json::ViewElement::Cloud(json::CloudViewElement {
                uid: c.uid,
                flow_uid: c.flow_uid,
                x: c.center.x,
                y: c.center.y,
            }));
        }
    }
    for f in &b.flows {
        let n = f.points.len();
        out.push(json::ViewElement::Flow(json::FlowViewElement {
            uid: f.uid,
            name: format!("Flow {}", f.uid),
            x: f.valve.x,
            y: f.valve.y,
            label_side: String::new(),
            points: f
                .points
                .iter()
                .enumerate()
                .map(|(i, p)| json::FlowPoint {
                    x: p.x,
                    y: p.y,
                    attached_to_uid: if f.detached {
                        0
                    } else if i == 0 {
                        b.end_uid(f.source)
                    } else if i == n - 1 {
                        b.end_uid(f.sink)
                    } else {
                        0
                    },
                })
                .collect(),
        }));
    }
    for &(uid, p) in &b.auxes {
        out.push(json::ViewElement::Auxiliary(json::AuxiliaryViewElement {
            uid,
            name: format!("Aux {uid}"),
            x: p.x,
            y: p.y,
            label_side: String::new(),
        }));
    }
    for &(uid, p) in &b.modules {
        out.push(json::ViewElement::Module(json::ModuleViewElement {
            uid,
            name: format!("Module {uid}"),
            x: p.x,
            y: p.y,
            label_side: String::new(),
        }));
    }
    for &(uid, alias_of_uid, p) in &b.aliases {
        out.push(json::ViewElement::Alias(json::AliasViewElement {
            uid,
            alias_of_uid,
            x: p.x,
            y: p.y,
            label_side: String::new(),
        }));
    }
    for &(uid, from_uid, to_uid, arc) in &b.links {
        out.push(json::ViewElement::Link(json::LinkViewElement {
            uid,
            from_uid,
            to_uid,
            arc,
            multi_points: Vec::new(),
            polarity: None,
        }));
    }
    out.into_iter().map(ViewElement::from).collect()
}

// ---------------------------------------------------------------------------
// Generator geometry, independent of the checker and the core

#[derive(Clone, Copy)]
struct Rect {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

fn stock_box(center: Pt) -> Rect {
    Rect {
        min_x: center.x - HALF_WIDTH,
        max_x: center.x + HALF_WIDTH,
        min_y: center.y - HALF_HEIGHT,
        max_y: center.y + HALF_HEIGHT,
    }
}

fn inflate_box(r: Rect, by: f64) -> Rect {
    Rect {
        min_x: r.min_x - by,
        max_x: r.max_x + by,
        min_y: r.min_y - by,
        max_y: r.max_y + by,
    }
}

fn box_gap(a: Rect, b: Rect) -> f64 {
    let gx = (a.min_x - b.max_x).max(b.min_x - a.max_x).max(0.0);
    let gy = (a.min_y - b.max_y).max(b.min_y - a.max_y).max(0.0);
    gx.max(gy)
}

fn distance_to_box(p: Pt, r: Rect) -> f64 {
    let dx = (r.min_x - p.x).max(0.0).max(p.x - r.max_x);
    let dy = (r.min_y - p.y).max(0.0).max(p.y - r.max_y);
    dx.hypot(dy)
}

/// Axis-aligned segments only (the generator never produces a diagonal before a
/// mutation): does the segment overlap the open box with positive length?
fn segment_hits_box(a: Pt, c: Pt, r: Rect) -> bool {
    if a.y == c.y {
        let (lo, hi) = (a.x.min(c.x), a.x.max(c.x));
        a.y > r.min_y && a.y < r.max_y && hi.min(r.max_x) - lo.max(r.min_x) > 0.0
    } else {
        let (lo, hi) = (a.y.min(c.y), a.y.max(c.y));
        a.x > r.min_x && a.x < r.max_x && hi.min(r.max_y) - lo.max(r.min_y) > 0.0
    }
}

fn distance_to_segment(p: Pt, a: Pt, c: Pt) -> f64 {
    let (dx, dy) = (c.x - a.x, c.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 == 0.0 {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
    };
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}

fn distance_to_path(p: Pt, pts: &[Pt]) -> f64 {
    pts.windows(2)
        .map(|w| distance_to_segment(p, w[0], w[1]))
        .fold(f64::INFINITY, f64::min)
}

fn path_length(pts: &[Pt]) -> f64 {
    pts.windows(2)
        .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
        .sum()
}

fn point_at_arc(pts: &[Pt], s: f64) -> Pt {
    let mut remaining = s;
    for i in 0..pts.len() - 1 {
        let (a, c) = (pts[i], pts[i + 1]);
        let length = (c.x - a.x).hypot(c.y - a.y);
        if remaining <= length || i == pts.len() - 2 {
            let t = if length == 0.0 {
                0.0
            } else {
                remaining / length
            };
            return pt(a.x + (c.x - a.x) * t, a.y + (c.y - a.y) * t);
        }
        remaining -= length;
    }
    pts[pts.len() - 1]
}

fn arc_position(pts: &[Pt], p: Pt) -> f64 {
    let mut best = f64::INFINITY;
    let mut position = 0.0;
    let mut traversed = 0.0;
    for w in pts.windows(2) {
        let (a, c) = (w[0], w[1]);
        let length = (c.x - a.x).hypot(c.y - a.y);
        let l2 = length * length;
        let t = if l2 == 0.0 {
            0.0
        } else {
            (((p.x - a.x) * (c.x - a.x) + (p.y - a.y) * (c.y - a.y)) / l2).clamp(0.0, 1.0)
        };
        let d = (p.x - (a.x + t * (c.x - a.x))).hypot(p.y - (a.y + t * (c.y - a.y)));
        if d < best {
            best = d;
            position = traversed + t * length;
        }
        traversed += length;
    }
    position
}

/// The segment a valve at arc position `s` rides; a valve exactly on a corner
/// belongs to the earlier segment.
fn segment_at_arc(pts: &[Pt], s: f64) -> usize {
    let mut traversed = 0.0;
    for i in 0..pts.len() - 1 {
        let length = (pts[i + 1].x - pts[i].x).hypot(pts[i + 1].y - pts[i].y);
        if s <= traversed + length {
            return i;
        }
        traversed += length;
    }
    pts.len() - 2
}

#[cfg(test)]
#[path = "scene_gen_tests.rs"]
mod tests;
