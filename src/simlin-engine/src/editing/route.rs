// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Route search: `route` and `route_end`.
//!
//! A route between two ports is an orthogonal polyline with k bends. Its
//! segments alternate axes starting with A0, so it is fully described by the
//! coordinate each segment HOLDS constant: h0 (the source endpoint's coordinate
//! across A0), interior holds h1..h(k-1), and hk (the sink endpoint's across
//! the last axis). For a face port h0/hk is the endpoint's position along the
//! face, the only free coordinate an attached endpoint has. Interior holds come
//! from candidates that are continuous functions of the terminals and belong to
//! the port pair being generated (so a tie between two pairs never switches on
//! a third pair's feasibility); face positions are solved from a preference.
//! The winning route therefore changes discontinuously only when the ranking
//! changes winner: when feasibility changes, which E3 documents.
//!
//! The search is streaming: a candidate is scored as it is generated, and only
//! the best candidate of each bend count survives (`Selector`), so a search
//! allocates nothing beyond its inline paths however many candidates it
//! examines.

use std::cmp::Ordering;

use smallvec::SmallVec;

use crate::datamodel::view_element::Flow;

use super::geometry::{
    Axis, Bounds, CORNER_CLEARANCE, Face, FlowEnd, GEOMETRY_EPSILON, MIN_SEGMENT, MIN_SINK_SEGMENT,
    NoObstacles, Obstacles, PIPE_SPACING, Path, Point, clamp, sign,
};
use super::path::{normalize, path_length, place_valve, valve_distance};
use super::terminal::{
    FaceAttachment, FlowGeometry, Terminal, Terminals, face_attachment, face_point, path_of,
    stub_tip, with_geometry,
};
use super::validity::{Fault, path_quality};

#[derive(Clone, Copy)]
struct FacePort {
    att: FaceAttachment,
    /// The preferred along-face position, within [lo, hi].
    pref: f64,
    pinned: bool,
    /// The terminal had a base face and this is not it.
    off_base: bool,
}

#[derive(Clone, Copy)]
struct PointPort {
    point: Point,
    /// The axis the adjacent segment must run along (a preserved corner), or
    /// `None` (any).
    axis: Option<Axis>,
}

#[derive(Clone, Copy)]
enum Port {
    Face(FacePort),
    Point(PointPort),
}

impl Port {
    fn off_base(&self) -> bool {
        matches!(self, Port::Face(f) if f.off_base)
    }

    fn reference(&self) -> Point {
        match self {
            Port::Point(p) => p.point,
            Port::Face(f) => face_point(&f.att, f.pref),
        }
    }

    /// The port's stub tip on `axis` (a face whose normal is `axis`), else its
    /// reference coordinate.
    fn tip(&self, axis: Axis, min: f64) -> f64 {
        match self {
            Port::Face(f) if f.att.normal == axis => stub_tip(&f.att, min),
            _ => self.reference().coord(axis),
        }
    }
}

type Ports = SmallVec<[Port; 4]>;

/// Hold candidates for one axis. A search's pools hold the terminal and nearby
/// obstacle clearances, stub tips and point coordinates: a few dozen at most.
type Holds = SmallVec<[f64; 32]>;

#[derive(Clone, Default)]
struct AxisHolds {
    x: Holds,
    y: Holds,
}

impl AxisHolds {
    fn get(&self, axis: Axis) -> &Holds {
        match axis {
            Axis::X => &self.x,
            Axis::Y => &self.y,
        }
    }

    fn get_mut(&mut self, axis: Axis) -> &mut Holds {
        match axis {
            Axis::X => &mut self.x,
            Axis::Y => &mut self.y,
        }
    }
}

fn push_distinct(out: &mut Holds, v: f64) {
    if v.is_finite() && !out.iter().any(|o| (o - v).abs() <= GEOMETRY_EPSILON) {
        out.push(v);
    }
}

/// The ports a terminal offers. A pinned stock terminal offers only its base
/// face at its base offset (`route_end`'s fixed end). An unpinned stock offers
/// all four faces: the base face prefers the base offset, any other face the
/// slot the routing preference picks.
fn ports_of(t: &Terminal, pinned: bool, occupied: &[Point]) -> Ports {
    let mut ports = Ports::new();
    match *t {
        Terminal::Free { point, .. } => ports.push(Port::Point(PointPort { point, axis: None })),
        Terminal::Stock {
            center,
            face,
            offset,
            ..
        } => {
            for f in Face::ALL {
                if pinned && face.is_some_and(|base| base != f) {
                    continue;
                }
                let att = face_attachment(center, f);
                let is_base = face == Some(f);
                let pref = match offset {
                    Some(offset) if is_base => clamp(att.center + offset, att.lo, att.hi),
                    _ => slot_preference(&att, occupied),
                };
                ports.push(Port::Face(FacePort {
                    att,
                    pref,
                    pinned: pinned && is_base,
                    off_base: face.is_some() && !is_base,
                }));
            }
        }
    }
    ports
}

/// The routing preference for a flow newly landing on a face: the position
/// nearest the face center at least `PIPE_SPACING` from every existing endpoint
/// on the face, else the position maximizing the minimum distance to them.
pub(crate) fn slot_preference(att: &FaceAttachment, occupied: &[Point]) -> f64 {
    let e = GEOMETRY_EPSILON;
    let used: SmallVec<[f64; 8]> = occupied
        .iter()
        .filter(|p| {
            (p.coord(att.normal) - att.plane).abs() <= e
                && p.coord(att.along) >= att.lo - CORNER_CLEARANCE - e
                && p.coord(att.along) <= att.hi + CORNER_CLEARANCE + e
        })
        .map(|p| p.coord(att.along))
        .collect();
    if used.is_empty() {
        return att.center;
    }
    let min_distance = |v: f64| {
        used.iter()
            .map(|u| (u - v).abs())
            .fold(f64::INFINITY, f64::min)
    };
    let in_range = |v: f64| v >= att.lo - e && v <= att.hi + e;
    // The first of the nearest on a tie.
    let nearest_center = |vs: &[f64]| {
        vs.iter().skip(1).fold(vs[0], |best, &v| {
            if (v - att.center).abs() < (best - att.center).abs() {
                v
            } else {
                best
            }
        })
    };
    let mut candidates: SmallVec<[f64; 16]> = SmallVec::from_slice(&[att.center, att.lo, att.hi]);
    for &u in &used {
        candidates.push(u - PIPE_SPACING);
        candidates.push(u + PIPE_SPACING);
    }
    let spaced: SmallVec<[f64; 16]> = candidates
        .iter()
        .copied()
        .filter(|&v| in_range(v) && min_distance(v) >= PIPE_SPACING - e)
        .collect();
    if !spaced.is_empty() {
        return nearest_center(&spaced);
    }
    let mut sorted = used.clone();
    sorted.sort_by(f64::total_cmp);
    let mut gaps: SmallVec<[f64; 16]> = SmallVec::from_slice(&[att.lo, att.hi]);
    for w in sorted.windows(2) {
        gaps.push((w[1] + w[0]) / 2.0);
    }
    gaps.retain(|v| in_range(*v));
    let widest = gaps
        .iter()
        .map(|&v| min_distance(v))
        .fold(f64::NEG_INFINITY, f64::max);
    let at_widest: SmallVec<[f64; 16]> = gaps
        .iter()
        .copied()
        .filter(|&v| min_distance(v) >= widest - e)
        .collect();
    nearest_center(&at_widest)
}

#[derive(Clone, Copy)]
enum AlongConstraint {
    /// The position must be at least `from` in direction `sign`.
    Half { from: f64, sign: f64 },
    /// The position must be at least `m` from `t`.
    Away { t: f64, m: f64 },
}

/// The along-face position nearest `pref` that satisfies `c`. Infeasible
/// constraints return the clamped preference, which the validity check then
/// rejects.
fn solve_along(port: &FacePort, pref: f64, c: AlongConstraint) -> f64 {
    if port.pinned {
        return port.pref;
    }
    let (lo, hi) = (port.att.lo, port.att.hi);
    let v = clamp(pref, lo, hi);
    match c {
        AlongConstraint::Half { from, sign } => {
            let a = if sign > 0.0 { lo.max(from) } else { lo };
            let b = if sign < 0.0 { hi.min(from) } else { hi };
            if a <= b { clamp(v, a, b) } else { v }
        }
        AlongConstraint::Away { t, m } => {
            if (v - t).abs() >= m - GEOMETRY_EPSILON {
                return v;
            }
            let mut best: Option<f64> = None;
            for o in [t - m, t + m] {
                if o >= lo - GEOMETRY_EPSILON && o <= hi + GEOMETRY_EPSILON {
                    best = Some(match best {
                        Some(b) if (o - v).abs() >= (b - v).abs() => b,
                        _ => o,
                    });
                }
            }
            best.unwrap_or(v)
        }
    }
}

fn along_accepts(port: &FacePort, v: f64) -> bool {
    let e = GEOMETRY_EPSILON;
    if port.pinned {
        return (v - port.pref).abs() <= e;
    }
    v >= port.att.lo - e && v <= port.att.hi + e
}

/// How a caller turns a generated path into the flow's full path, and which
/// candidates it refuses outright.
#[derive(Clone, Copy)]
enum Tail<'a> {
    /// A route between two terminals: the generated path is the path.
    Whole,
    /// `route_end` moving the sink: the preserved prefix up to and including
    /// the corner; the generated path starts at the corner.
    Sink {
        prefix: &'a [Point],
        base_bends: usize,
        base_u_turns: usize,
    },
    /// `route_end` moving the source: the preserved prefix from the corner to
    /// the fixed end; the generated path ends at the corner.
    Source {
        prefix: &'a [Point],
        base_bends: usize,
        base_u_turns: usize,
    },
}

impl Tail<'_> {
    fn assemble(&self, path: &[Point]) -> Path {
        let mut points = Path::new();
        match *self {
            Tail::Whole => points.extend_from_slice(path),
            Tail::Sink { prefix, .. } => {
                points.extend_from_slice(&prefix[..prefix.len() - 1]);
                points.extend_from_slice(path);
            }
            Tail::Source { prefix, .. } => {
                points.extend_from_slice(path);
                points.extend_from_slice(&prefix[1..]);
            }
        }
        points
    }

    /// A tail may not give the path more bends or U turns than the base had
    /// (preserving corners keeps a shape, and a tail that grows it is a detour
    /// that releasing replaces), nor run back over the preserved prefix.
    fn refuses(&self, points: &[Point]) -> bool {
        match *self {
            Tail::Whole => false,
            Tail::Sink {
                prefix,
                base_bends,
                base_u_turns,
            }
            | Tail::Source {
                prefix,
                base_bends,
                base_u_turns,
            } => {
                points.len() - 2 > base_bends
                    || u_turn_count(points) > base_u_turns
                    || folds_back(points, prefix)
            }
        }
    }
}

struct Search<'a, O: Obstacles + ?Sized> {
    sources: Ports,
    sinks: Ports,
    /// Minimum length of the segment adjacent to each port.
    min_source: f64,
    min_sink: f64,
    terminals: &'a Terminals,
    /// Stocks besides the terminals a candidate should not pass through; one
    /// that does ranks as crossing.
    obstacles: &'a O,
    tail: Tail<'a>,
    base_first_axis: Option<Axis>,
    base_last_axis: Option<Axis>,
    /// The most bends a candidate is ranked with: 2 for a route between two
    /// terminals (straight, L, Z), 3 for a tail a preserved corner forces into
    /// a turn. Past that only `detours` generates more.
    max_bends: usize,
    /// When nothing valid exists, also try U turns and up to four bends
    /// (never for `route_end`'s pinned attempt).
    detours: bool,
    /// Base corner coordinates per axis: the holds that keep an existing shape.
    base_holds: AxisHolds,
    /// A minimum riser either side of each point port, per axis: ahead of the
    /// pair midpoint.
    point_pools: AxisHolds,
    /// Pair-independent hold candidates per axis after the midpoint (point
    /// coordinates, stub tips, body clearances).
    pools: AxisHolds,
}

impl<'a, O: Obstacles + ?Sized> Search<'a, O> {
    /// A search over `sources` x `sinks`, with its pools built.
    #[allow(clippy::too_many_arguments)]
    fn new(
        sources: Ports,
        sinks: Ports,
        min_source: f64,
        min_sink: f64,
        terminals: &'a Terminals,
        obstacles: &'a O,
        tail: Tail<'a>,
        base: &[Point],
        max_bends: usize,
        detours: bool,
    ) -> Self {
        let (base_first_axis, base_last_axis) = end_axes(base);
        let mut search = Search {
            sources,
            sinks,
            min_source,
            min_sink,
            terminals,
            obstacles,
            tail,
            base_first_axis,
            base_last_axis,
            max_bends,
            detours,
            base_holds: base_holds_of(base),
            point_pools: AxisHolds::default(),
            pools: AxisHolds::default(),
        };
        search.build_pools();
        search
    }

    /// Pair-independent hold candidates per axis. `point_pools` is the minimum
    /// distance either side of a point port: a Z whose riser hugs a cloud turns
    /// into the L the cloud reaches with the least change, and a tail leaves a
    /// preserved corner by exactly a minimum riser, so these rank ahead of a
    /// pair's midpoint. `pools` follows the midpoint: point coordinates, stub
    /// tips, and clearances around each terminal body and each obstacle within
    /// reach (the terminal bodies and port references inflated by two minimum
    /// segments): a route can only go around a body whose clearance is a hold
    /// it can take.
    fn build_pools(&mut self) {
        let mut reach = self
            .terminals
            .source
            .body()
            .union(self.terminals.sink.body());
        for port in self.sources.iter().chain(self.sinks.iter()) {
            reach = reach.union(Bounds::point(port.reference()));
        }
        let reach = reach.inflate(2.0 * MIN_SEGMENT);
        let mut bodies: SmallVec<[Point; 8]> = SmallVec::new();
        for t in self.terminals.both() {
            if let Some(center) = t.stock_center() {
                bodies.push(center);
            }
        }
        self.obstacles.any_near(reach, |center| {
            if Bounds::stock(center).overlaps(reach) {
                bodies.push(center);
            }
            false
        });
        for axis in Axis::ALL {
            for (ports, min) in [
                (&self.sources, self.min_source),
                (&self.sinks, self.min_sink),
            ] {
                for port in ports {
                    match port {
                        Port::Point(p) => {
                            let v = p.point.coord(axis);
                            push_distinct(self.point_pools.get_mut(axis), v - min);
                            push_distinct(self.point_pools.get_mut(axis), v + min);
                            push_distinct(self.pools.get_mut(axis), v);
                        }
                        Port::Face(f) if f.att.normal == axis => {
                            push_distinct(self.pools.get_mut(axis), stub_tip(&f.att, min));
                        }
                        Port::Face(_) => {}
                    }
                }
            }
            for &center in &bodies {
                let body = Bounds::stock(center);
                let (lo, hi) = match axis {
                    Axis::X => (body.min_x, body.max_x),
                    Axis::Y => (body.min_y, body.max_y),
                };
                push_distinct(self.pools.get_mut(axis), lo - MIN_SEGMENT);
                push_distinct(self.pools.get_mut(axis), hi + MIN_SEGMENT);
            }
        }
    }

    /// The hold candidates for a port pair on `axis`, in priority order: each
    /// base corner clamped into the pair's feasible band (between the two ports'
    /// stub tips), the point-port clearances, the band's midpoint, then the
    /// shared pools. Clamping keeps an existing run where it is while the band
    /// covers it and follows the band's edge when it does not, so the hold is a
    /// continuous function of the terminals; the midpoint is the pair's own
    /// fallback, never another pair's.
    fn hold_candidates(&self, p: &Port, q: &Port, axis: Axis) -> Holds {
        let a = p.tip(axis, self.min_source);
        let b = q.tip(axis, self.min_sink);
        let (lo, hi) = (a.min(b), a.max(b));
        let mut out = Holds::new();
        for &v in self.base_holds.get(axis) {
            push_distinct(&mut out, clamp(v, lo, hi));
        }
        for &v in self.point_pools.get(axis) {
            push_distinct(&mut out, v);
        }
        push_distinct(&mut out, (a + b) / 2.0);
        for &v in self.pools.get(axis) {
            push_distinct(&mut out, v);
        }
        out
    }

    fn generate(&self, k: usize, out: &mut Selector, tier: ShapeTier) {
        for p in &self.sources {
            for q in &self.sinks {
                for a0 in Axis::ALL {
                    self.generate_shape(p, q, k, a0, out, tier);
                }
            }
        }
    }

    fn generate_shape(
        &self,
        p: &Port,
        q: &Port,
        k: usize,
        a0: Axis,
        out: &mut Selector,
        tier: ShapeTier,
    ) {
        let preserved = matches!(p, Port::Point(pp) if pp.axis.is_some())
            || matches!(q, Port::Point(pp) if pp.axis.is_some());
        if tier == ShapeTier::UTurns && (k != 2 || preserved) {
            return;
        }
        let a1 = a0.other();
        let ak = if k.is_multiple_of(2) { a0 } else { a1 };
        let p_mismatch = match p {
            Port::Face(f) => f.att.normal != a0,
            Port::Point(pp) => pp.axis.is_some_and(|axis| axis != a0),
        };
        let q_mismatch = match q {
            Port::Face(f) => f.att.normal != ak,
            Port::Point(pp) => pp.axis.is_some_and(|axis| axis != ak),
        };
        if p_mismatch || q_mismatch {
            return;
        }
        let e = GEOMETRY_EPSILON;
        let start = match p {
            Port::Face(f) => f.att.plane,
            Port::Point(pp) => pp.point.coord(a0),
        };
        let end = match q {
            Port::Face(f) => f.att.plane,
            Port::Point(pp) => pp.point.coord(ak),
        };

        if k == 0 {
            let v = match (p, q) {
                (Port::Face(pf), Port::Face(qf)) => {
                    let lo = pf.att.lo.max(qf.att.lo);
                    let hi = pf.att.hi.min(qf.att.hi);
                    if lo > hi + e {
                        return;
                    }
                    // Unpinned, a straight between two faces splits the
                    // difference between the two preferences: each end gives
                    // way equally.
                    if pf.pinned {
                        pf.pref
                    } else if qf.pinned {
                        qf.pref
                    } else {
                        clamp((pf.pref + qf.pref) / 2.0, lo, hi)
                    }
                }
                (Port::Face(pf), Port::Point(qp)) => {
                    let v = qp.point.coord(a1);
                    if !along_accepts(pf, v) {
                        return;
                    }
                    v
                }
                (Port::Point(pp), Port::Face(qf)) => {
                    let v = pp.point.coord(a1);
                    if !along_accepts(qf, v) {
                        return;
                    }
                    v
                }
                (Port::Point(pp), Port::Point(qp)) => {
                    let v = pp.point.coord(a1);
                    if (qp.point.coord(a1) - v).abs() > e {
                        return;
                    }
                    v
                }
            };
            self.emit(p, q, &[a0.compose(start, v), a0.compose(end, v)], 0, out);
            return;
        }

        let shape = Shape {
            p,
            q,
            k,
            a0,
            a1,
            ak,
            start,
            end,
            preserved,
            tier,
            candidates_x: self.hold_candidates(p, q, Axis::X),
            candidates_y: self.hold_candidates(p, q, Axis::Y),
        };
        let mut holds = [0.0f64; MAX_GENERATED_BENDS + 1];
        if k == 1 {
            self.finish(&shape, &mut holds, out);
        } else {
            self.fill(&shape, 1, &mut holds, out);
        }
    }

    /// Fill interior hold `i` with each candidate and recurse; finish the shape
    /// once every interior hold is set.
    fn fill(
        &self,
        shape: &Shape<'_>,
        i: usize,
        holds: &mut [f64; MAX_GENERATED_BENDS + 1],
        out: &mut Selector,
    ) {
        let k = shape.k;
        if i == k {
            self.finish(shape, holds, out);
            return;
        }
        let e = GEOMETRY_EPSILON;
        let axis = if i % 2 == 1 { shape.a0 } else { shape.a1 };
        // A hold a minimum riser away from the parallel hold two segments back
        // (or from a point port's fixed coordinate there) keeps a tail from
        // detouring to a far candidate while a near feasible position exists.
        let mut neighbors: SmallVec<[f64; 4]> = SmallVec::new();
        if i >= 3 {
            neighbors.push(holds[i - 2] - MIN_SEGMENT);
            neighbors.push(holds[i - 2] + MIN_SEGMENT);
        } else if i == 2
            && let Port::Point(pp) = shape.p
        {
            let v = pp.point.coord(shape.a1);
            neighbors.push(v - MIN_SEGMENT);
            neighbors.push(v + MIN_SEGMENT);
        }
        if i + 2 == k
            && let Port::Point(qp) = shape.q
        {
            let v = qp.point.coord(shape.ak.other());
            neighbors.push(v - MIN_SEGMENT);
            neighbors.push(v + MIN_SEGMENT);
        }
        let candidates = match axis {
            Axis::X => &shape.candidates_x,
            Axis::Y => &shape.candidates_y,
        };
        for &v in candidates.iter().chain(neighbors.iter()) {
            if i == 1 && !outward_ok(shape.p, shape.start, v) {
                continue;
            }
            if i + 1 == k && !outward_ok(shape.q, shape.end, v) {
                continue;
            }
            // An interior segment between holds i-2 and i must have length.
            if i >= 3 && (v - holds[i - 2]).abs() <= e {
                continue;
            }
            holds[i] = v;
            self.fill(shape, i + 1, holds, out);
        }
    }

    fn finish(
        &self,
        shape: &Shape<'_>,
        holds: &mut [f64; MAX_GENERATED_BENDS + 1],
        out: &mut Selector,
    ) {
        let Shape {
            p,
            q,
            k,
            a0,
            a1,
            ak,
            start,
            end,
            preserved,
            tier,
            ..
        } = *shape;
        let e = GEOMETRY_EPSILON;
        let fixed_p = match p {
            Port::Point(pp) => Some(pp.point.coord(a1)),
            Port::Face(_) => None,
        };
        let fixed_q = match q {
            Port::Point(qp) => Some(qp.point.coord(ak.other())),
            Port::Face(_) => None,
        };
        let solve_p = |pref: f64, neighbor: f64| -> f64 {
            let Port::Face(port) = p else {
                return fixed_p.unwrap_or(pref);
            };
            if k == 1 {
                match q {
                    Port::Face(qf) => solve_along(
                        port,
                        pref,
                        AlongConstraint::Half {
                            from: stub_tip(&qf.att, self.min_sink),
                            sign: qf.att.sign,
                        },
                    ),
                    Port::Point(_) => solve_along(
                        port,
                        pref,
                        AlongConstraint::Away {
                            t: end,
                            m: self.min_sink,
                        },
                    ),
                }
            } else {
                solve_along(
                    port,
                    pref,
                    AlongConstraint::Away {
                        t: neighbor,
                        m: MIN_SEGMENT,
                    },
                )
            }
        };
        let solve_q = |pref: f64, neighbor: f64| -> f64 {
            let Port::Face(port) = q else {
                return fixed_q.unwrap_or(pref);
            };
            if k == 1 {
                match p {
                    Port::Face(pf) => solve_along(
                        port,
                        pref,
                        AlongConstraint::Half {
                            from: stub_tip(&pf.att, self.min_source),
                            sign: pf.att.sign,
                        },
                    ),
                    Port::Point(_) => solve_along(
                        port,
                        pref,
                        AlongConstraint::Away {
                            t: start,
                            m: self.min_source,
                        },
                    ),
                }
            } else {
                solve_along(
                    port,
                    pref,
                    AlongConstraint::Away {
                        t: neighbor,
                        m: MIN_SEGMENT,
                    },
                )
            }
        };
        let pref_p = match p {
            Port::Face(f) => f.pref,
            Port::Point(_) => fixed_p.unwrap_or(0.0),
        };
        let pref_q = match q {
            Port::Face(f) => f.pref,
            Port::Point(_) => fixed_q.unwrap_or(0.0),
        };
        let mut variants: SmallVec<[(f64, f64); 2]> = SmallVec::new();
        if k == 2 {
            // The riser joins the two endpoints' along positions, so they are
            // solved against each other, once in each order.
            let first = solve_p(pref_p, pref_q);
            variants.push((first, solve_q(pref_q, first)));
            let second = solve_q(pref_q, pref_p);
            variants.push((solve_p(pref_p, second), second));
        } else if k == 1 {
            // A one-bend shape solves each end against the other port, never
            // against an interior hold (there is none).
            variants.push((solve_p(pref_p, 0.0), solve_q(pref_q, 0.0)));
        } else {
            variants.push((solve_p(pref_p, holds[2]), solve_q(pref_q, holds[k - 2])));
        }
        for vi in 0..variants.len() {
            let (a, b) = variants[vi];
            if variants[..vi]
                .iter()
                .any(|&(sa, sb)| (sa - a).abs() <= e && (sb - b).abs() <= e)
            {
                continue;
            }
            holds[0] = a;
            holds[k] = b;
            let mut points: SmallVec<[Point; 8]> = SmallVec::new();
            points.push(a0.compose(start, holds[0]));
            for i in 1..=k {
                let ai = if i % 2 == 0 { a0 } else { a1 };
                points.push(ai.compose(holds[i - 1], holds[i]));
            }
            points.push(ak.compose(end, holds[k]));
            if k == 2 && !preserved {
                let leaving = sign(points[1].coord(a0) - points[0].coord(a0));
                let arriving = sign(points[3].coord(a0) - points[2].coord(a0));
                if (leaving != arriving) != (tier == ShapeTier::UTurns) {
                    continue;
                }
            }
            self.emit(p, q, &points, k, out);
        }
    }

    fn emit(&self, p: &Port, q: &Port, path: &[Point], bends: usize, out: &mut Selector) {
        let points = self.tail.assemble(path);
        if self.tail.refuses(&points) {
            return;
        }
        let sticky = u8::from(p.off_base()) + u8::from(q.off_base());
        let n = points.len();
        let mut axis_change = 0u8;
        if self
            .base_first_axis
            .is_some_and(|axis| Axis::of_segment(points[0], points[1]) != axis)
        {
            axis_change += 1;
        }
        if self
            .base_last_axis
            .is_some_and(|axis| Axis::of_segment(points[n - 2], points[n - 1]) != axis)
        {
            axis_change += 1;
        }
        let quality = path_quality(&points, self.terminals, &NoObstacles, self.obstacles);
        let length = path_length(&points);
        let index = out.count;
        out.add(Scored {
            points,
            bends,
            sticky,
            axis_change,
            length,
            index,
            fault: quality.fault,
            crossing: quality.crossing,
            obstructed: quality.obstructed,
        });
    }

    /// Pick the route: validity; then, among valid candidates, not crossing a
    /// terminal body (G6's best effort when the bodies overlap and crossing is
    /// no fault); then stickiness (keep the base faces when they have a
    /// candidate within one bend of the best); then bends, axis change and
    /// length. Shapes past straight, L and Z are generated only when needed:
    /// tails up to `max_bends` when nothing valid exists yet or the stickiness
    /// window reaches past what was generated, and, when `detours` is set and
    /// nothing is valid -- or every valid candidate crosses something and one of
    /// them passes through a non-terminal stock -- U turns and then more bends.
    /// A detour around a stock therefore beats a route through it, while a
    /// crossing G6 excuses (overlapping terminal bodies) generates no detours.
    /// With nothing valid at all, the least severe fault wins, so a route
    /// always exists.
    fn run(&self) -> Path {
        let mut out = Selector::default();
        let mut generated = 2;
        for k in 0..=generated {
            self.generate(k, &mut out, ShapeTier::Shapes);
        }
        let mut u_turns = false;
        loop {
            if !out.any_valid() {
                if self.detours && !u_turns {
                    u_turns = true;
                    self.generate(2, &mut out, ShapeTier::UTurns);
                    continue;
                }
                if generated < MAX_GENERATED_BENDS
                    && (generated < self.max_bends || self.detours || out.count == 0)
                {
                    generated += 1;
                    self.generate(generated, &mut out, ShapeTier::Shapes);
                    continue;
                }
                // The totality rule: G6 relaxed before G3 before structure.
                return out.fallback.map(|c| c.points).unwrap_or_default();
            }
            if !out.any_clear() && self.detours && out.valid_obstructed {
                if !u_turns {
                    u_turns = true;
                    self.generate(2, &mut out, ShapeTier::UTurns);
                    continue;
                }
                if generated < MAX_GENERATED_BENDS {
                    generated += 1;
                    self.generate(generated, &mut out, ShapeTier::Shapes);
                    continue;
                }
            }
            let Some(best) = (0..=MAX_GENERATED_BENDS).find(|&b| out.pool_best(b).is_some()) else {
                return Path::new();
            };
            let window = [
                out.pool_best(best),
                out.pool_best(best + 1)
                    .filter(|_| best < MAX_GENERATED_BENDS),
            ];
            let min_sticky = window.iter().flatten().map(|c| c.sticky).min().unwrap_or(0);
            if min_sticky > 0 && best + 1 > generated && generated < self.max_bends {
                generated += 1;
                self.generate(generated, &mut out, ShapeTier::Shapes);
                continue;
            }
            let winner = window
                .into_iter()
                .flatten()
                .reduce(|a, b| if better_by_sticky(b, a) { b } else { a });
            return winner.map(|c| c.points.clone()).unwrap_or_default();
        }
    }
}

/// One two-or-more-bend shape being generated for a port pair.
struct Shape<'p> {
    p: &'p Port,
    q: &'p Port,
    k: usize,
    a0: Axis,
    a1: Axis,
    ak: Axis,
    start: f64,
    end: f64,
    preserved: bool,
    tier: ShapeTier,
    candidates_x: Holds,
    candidates_y: Holds,
}

fn outward_ok(port: &Port, port_coord: f64, v: f64) -> bool {
    let e = GEOMETRY_EPSILON;
    match port {
        Port::Face(f) => f.att.sign * (v - f.att.plane) > e,
        Port::Point(_) => (v - port_coord).abs() > e,
    }
}

/// Which two-bend shapes to emit. A plain route's two-bend shape is a Z (it
/// leaves and arrives travelling the same way); a U (arriving travelling back)
/// is a detour, generated only when nothing else is valid. A tail ending at a
/// preserved corner may need either, so both are emitted with the other shapes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShapeTier {
    Shapes,
    UTurns,
}

/// The most bends any generated shape has.
const MAX_GENERATED_BENDS: usize = 4;

/// One scored candidate.
struct Scored {
    points: Path,
    bends: usize,
    sticky: u8,
    axis_change: u8,
    length: f64,
    index: usize,
    fault: Fault,
    crossing: bool,
    obstructed: bool,
}

/// The ranking's tail: bends, axis change, length (within `GEOMETRY_EPSILON`, a
/// tie) and generation order. Z variants whose riser sits at different holds
/// have the same length up to float noise, and letting that noise rank them
/// flips the riser between frames.
fn compare_tail(a: &Scored, b: &Scored) -> Ordering {
    let by_length = if (a.length - b.length).abs() > GEOMETRY_EPSILON {
        a.length.total_cmp(&b.length)
    } else {
        Ordering::Equal
    };
    a.bends
        .cmp(&b.bends)
        .then(a.axis_change.cmp(&b.axis_change))
        .then(by_length)
        .then(a.index.cmp(&b.index))
}

fn better_by_sticky(a: &Scored, b: &Scored) -> bool {
    a.sticky
        .cmp(&b.sticky)
        .then_with(|| compare_tail(a, b))
        .is_lt()
}

fn better_fallback(a: &Scored, b: &Scored) -> bool {
    a.fault
        .cmp(&b.fault)
        .then(a.crossing.cmp(&b.crossing))
        .then(a.sticky.cmp(&b.sticky))
        .then_with(|| compare_tail(a, b))
        .is_lt()
}

/// The streaming ranking of a search: everything the selection reads, kept per
/// bend count instead of per candidate. Within one bend count the selection
/// picks by stickiness then the tail, so the best candidate of each count is
/// all that can ever win.
#[derive(Default)]
struct Selector {
    /// How many candidates have been added: the next candidate's index.
    count: usize,
    /// The best valid candidate that crosses nothing, by bend count.
    clear: [Option<Scored>; MAX_GENERATED_BENDS + 1],
    /// The best valid candidate that crosses something, by bend count.
    crossing: [Option<Scored>; MAX_GENERATED_BENDS + 1],
    /// Whether some valid candidate passes through a non-terminal stock.
    valid_obstructed: bool,
    /// The least severe invalid candidate: fault, then not crossing, then
    /// stickiness, then the tail.
    fallback: Option<Scored>,
}

impl Selector {
    fn add(&mut self, c: Scored) {
        self.count += 1;
        if c.fault == Fault::None {
            self.valid_obstructed |= c.obstructed;
            let slot = if c.crossing {
                &mut self.crossing[c.bends]
            } else {
                &mut self.clear[c.bends]
            };
            if slot
                .as_ref()
                .is_none_or(|current| better_by_sticky(&c, current))
            {
                *slot = Some(c);
            }
        } else if self
            .fallback
            .as_ref()
            .is_none_or(|current| better_fallback(&c, current))
        {
            self.fallback = Some(c);
        }
    }

    fn any_clear(&self) -> bool {
        self.clear.iter().any(Option::is_some)
    }

    fn any_valid(&self) -> bool {
        self.any_clear() || self.crossing.iter().any(Option::is_some)
    }

    /// The pool the selection ranks, at one bend count: the clear candidates
    /// when any exists, else every valid candidate (which are then all crossing).
    fn pool_best(&self, bends: usize) -> Option<&Scored> {
        if bends > MAX_GENERATED_BENDS {
            return None;
        }
        if self.any_clear() {
            self.clear[bends].as_ref()
        } else {
            self.crossing[bends].as_ref()
        }
    }
}

fn distinct_path(points: &[Point]) -> bool {
    points.len() >= 2
        && points.iter().all(|p| p.is_finite())
        && path_length(points) > GEOMETRY_EPSILON
}

fn end_axes(points: &[Point]) -> (Option<Axis>, Option<Axis>) {
    if !distinct_path(points) {
        return (None, None);
    }
    let n = points.len();
    (
        Some(Axis::of_segment(points[0], points[1])),
        Some(Axis::of_segment(points[n - 2], points[n - 1])),
    )
}

fn base_holds_of(base: &[Point]) -> AxisHolds {
    let mut holds = AxisHolds::default();
    if distinct_path(base) {
        for p in &base[1..base.len() - 1] {
            if p.is_finite() {
                push_distinct(&mut holds.x, p.x);
                push_distinct(&mut holds.y, p.y);
            }
        }
    }
    holds
}

/// Route between two terminals with no preserved prefix: an empty path when
/// no candidate exists at all.
pub(crate) fn route_between<O: Obstacles + ?Sized>(
    terminals: &Terminals,
    pinned_source: bool,
    pinned_sink: bool,
    base: &[Point],
    occupied: &[Point],
    detours: bool,
    obstacles: &O,
) -> Path {
    Search::new(
        ports_of(&terminals.source, pinned_source, occupied),
        ports_of(&terminals.sink, pinned_sink, occupied),
        MIN_SEGMENT,
        MIN_SINK_SEGMENT,
        terminals,
        obstacles,
        Tail::Whole,
        base,
        2,
        detours,
    )
    .run()
}

/// The minimal orthogonal route between two terminals: a straight, L or Z over
/// every face pair, ranked as `Search::run` describes. When none is valid a U
/// turn, then more bends, are tried (two clouds a pixel off each other's line,
/// or a stock over its own cloud, have no valid straight, L or Z). Total: when
/// nothing is valid (the terminal bodies overlap), G6 is relaxed first and a
/// route is still returned.
///
/// `base` is the flow being routed as it was when the gesture started: its
/// identity is kept, and its path supplies the valve's arc position and the
/// shape stickiness (a creation draft with no length routes fresh). The valve
/// keeps its arc distance from `valve_from`. `occupied` are other flows'
/// endpoints on the terminal stocks, in this frame's coordinates, for the slot
/// preference; `obstacles` are the view's stocks in this frame's coordinates.
/// A non-finite terminal returns the base flow unchanged.
pub(crate) fn route<O: Obstacles + ?Sized>(
    source: Terminal,
    sink: Terminal,
    base: &Flow,
    valve_from: FlowEnd,
    occupied: &[Point],
    obstacles: &O,
) -> FlowGeometry {
    if !source.is_finite() || !sink.is_finite() {
        return FlowGeometry::unchanged(base);
    }
    let terminals = Terminals { source, sink };
    let base_points = path_of(base);
    let points = route_between(
        &terminals,
        false,
        false,
        &base_points,
        occupied,
        true,
        obstacles,
    );
    let valve = place_valve(
        &points,
        valve_from,
        valve_distance(&base_points, Point::new(base.x, base.y), valve_from),
    );
    with_geometry(base, &points, valve, &terminals)
}

/// Re-route one end of `flow` (the base flow) to `terminal`, keeping as much of
/// the path near the fixed end as stays valid.
///
/// Preserve k interior corners counted from the fixed end, for k = K..1 with K
/// all but the corner adjacent to the re-routed end; the first k whose tail
/// yields a valid path that crosses no terminal body and meets every G3 minimum
/// (even where the terminals leave no room and G3 would excuse it) wins. A tail
/// may not fold back over its preserved prefix, nor give the path more bends
/// than the base had. Then k = 0 with the fixed terminal pinned to its base
/// face and offset, and only if that is still invalid or crossing is the flow
/// released to `route`. A path through `obstacles` ranks as crossing in every
/// attempt: a preserved or pinned tail through one is refused, and the
/// released search prefers a route around it but never refuses its last
/// resort.
///
/// The valve keeps its arc-length distance from the fixed end. A non-finite
/// terminal returns the base flow unchanged.
pub(crate) fn route_end<O: Obstacles + ?Sized>(
    flow: &Flow,
    end: FlowEnd,
    terminal: Terminal,
    fixed: Terminal,
    occupied: &[Point],
    obstacles: &O,
) -> FlowGeometry {
    if !terminal.is_finite() || !fixed.is_finite() {
        return FlowGeometry::unchanged(flow);
    }
    let terminals = match end {
        FlowEnd::Source => Terminals {
            source: terminal,
            sink: fixed,
        },
        FlowEnd::Sink => Terminals {
            source: fixed,
            sink: terminal,
        },
    };
    let fixed_end = end.other();
    let flow_points = path_of(flow);
    let base: Path = if distinct_path(&flow_points) {
        normalize(&flow_points)
    } else {
        flow_points.clone()
    };
    let acceptable = |points: &[Point]| {
        let quality = path_quality(points, &terminals, &NoObstacles, obstacles);
        quality.fault == Fault::None && !quality.crossing && !quality.short
    };
    let mut points: Option<Path> = None;
    if distinct_path(&base) {
        let most = base.len().saturating_sub(3);
        for k in (1..=most).rev() {
            if let Some(tail) = preserved_tail(&base, end, k, &terminals, occupied, obstacles)
                && acceptable(&tail)
            {
                points = Some(tail);
                break;
            }
        }
    }
    if points.is_none() {
        // No detours while pinned: a U turn that keeps the fixed endpoint put
        // is worse than releasing it to slide along its face into a straight
        // route. Of `acceptable`'s clauses, a terminal crossing never fires
        // here -- a pinned search's valid winner is a straight, L or
        // same-direction Z, monotone in both axes, leaving the fixed face
        // outward and entering the moving terminal's face inward -- so the
        // clause acts on preserved tails and on an obstacle a monotone path can
        // still pass through, which releases the flow to the full search.
        let pinned = route_between(
            &terminals,
            fixed_end == FlowEnd::Source,
            fixed_end == FlowEnd::Sink,
            &base,
            occupied,
            false,
            obstacles,
        );
        if !pinned.is_empty() && acceptable(&pinned) {
            points = Some(pinned);
        }
    }
    let points = points.unwrap_or_else(|| {
        route_between(&terminals, false, false, &base, occupied, true, obstacles)
    });
    let valve = place_valve(
        &points,
        fixed_end,
        valve_distance(&flow_points, Point::new(flow.x, flow.y), fixed_end),
    );
    with_geometry(flow, &points, valve, &terminals)
}

fn preserved_tail<O: Obstacles + ?Sized>(
    base: &[Point],
    end: FlowEnd,
    k: usize,
    terminals: &Terminals,
    occupied: &[Point],
    obstacles: &O,
) -> Option<Path> {
    let n = base.len();
    let moving = match end {
        FlowEnd::Source => &terminals.source,
        FlowEnd::Sink => &terminals.sink,
    };
    let moving_ports = ports_of(moving, false, occupied);
    let base_bends = n - 2;
    let base_u_turns = u_turn_count(base);
    let search = match end {
        FlowEnd::Sink => {
            let prefix = &base[..=k];
            let corner = prefix[k];
            let port = Port::Point(PointPort {
                point: corner,
                axis: Some(Axis::of_segment(prefix[k - 1], corner).other()),
            });
            Search::new(
                SmallVec::from_slice(&[port]),
                moving_ports,
                MIN_SEGMENT,
                MIN_SINK_SEGMENT,
                terminals,
                obstacles,
                Tail::Sink {
                    prefix,
                    base_bends,
                    base_u_turns,
                },
                base,
                3,
                false,
            )
        }
        FlowEnd::Source => {
            let prefix = &base[n - 1 - k..];
            let corner = prefix[0];
            let port = Port::Point(PointPort {
                point: corner,
                axis: Some(Axis::of_segment(corner, prefix[1]).other()),
            });
            Search::new(
                moving_ports,
                SmallVec::from_slice(&[port]),
                MIN_SEGMENT,
                MIN_SEGMENT,
                terminals,
                obstacles,
                Tail::Source {
                    prefix,
                    base_bends,
                    base_u_turns,
                },
                base,
                3,
                false,
            )
        }
    };
    let points = search.run();
    (!points.is_empty()
        && path_quality(&points, terminals, &NoObstacles, obstacles).fault == Fault::None)
        .then_some(points)
}

/// The number of U turns in a path: two consecutive turns in the same
/// rotational direction (a Z turns one way and back; a U turns the same way
/// twice and heads back past where it came from).
fn u_turn_count(points: &[Point]) -> usize {
    let mut count = 0;
    let mut previous = 0.0;
    for w in points.windows(3) {
        let (ax, ay) = (w[1].x - w[0].x, w[1].y - w[0].y);
        let (bx, by) = (w[2].x - w[1].x, w[2].y - w[1].y);
        let turn = sign(ax * by - ay * bx);
        if turn != 0.0 && turn == previous {
            count += 1;
        }
        previous = turn;
    }
    count
}

/// Whether some segment of `points` runs back over a segment of `preserved`:
/// the two are parallel, travel in opposite directions, hold coordinates less
/// than `MIN_SEGMENT` apart, and overlap in span. The pipe would draw over
/// itself.
fn folds_back(points: &[Point], preserved: &[Point]) -> bool {
    let e = GEOMETRY_EPSILON;
    points.windows(2).any(|w| {
        let (a, b) = (w[0], w[1]);
        let axis = Axis::of_segment(a, b);
        let hold = a.coord(axis.other());
        let direction = sign(b.coord(axis) - a.coord(axis));
        preserved.windows(2).any(|pw| {
            let (c, d) = (pw[0], pw[1]);
            if Axis::of_segment(c, d) != axis || sign(d.coord(axis) - c.coord(axis)) != -direction {
                return false;
            }
            if (c.coord(axis.other()) - hold).abs() >= MIN_SEGMENT - e {
                return false;
            }
            let lo = a
                .coord(axis)
                .min(b.coord(axis))
                .max(c.coord(axis).min(d.coord(axis)));
            let hi = a
                .coord(axis)
                .max(b.coord(axis))
                .min(c.coord(axis).max(d.coord(axis)));
            hi - lo > e
        })
    })
}
