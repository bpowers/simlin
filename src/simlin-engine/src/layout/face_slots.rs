// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Where a flow that incremental layout creates meets a stock face.
//!
//! Incremental layout never moves a flow the patch did not touch, so a flow it
//! creates fits around the ends already on a face instead of re-spacing them:
//! its stock end takes the largest free gap on the face, bounded by those ends
//! and by the face's corner clearance. It therefore never lands on a preserved
//! sibling, and never in a corner zone. A created flow between two stocks takes
//! one line for both ends where the two faces' free gaps overlap, so its pipe
//! runs straight rather than jogging between two independently chosen slots;
//! and a created flow's cloud is kept from overlapping another cloud.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::FlowPoint;
use crate::diagram::constants::{CLOUD_RADIUS, STOCK_HEIGHT, STOCK_WIDTH};
use crate::diagram::flow_geometry::CORNER_CLEARANCE;

const EPS: f64 = 1e-6;

/// How many times a created flow's cloud end is pushed out along its pipe to
/// clear the clouds already in the view, at most.
const MAX_CLOUD_PUSHES: usize = 8;

/// A face of a stock.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// The face an endpoint attached to a stock centered at `stock` sits on,
    /// by the aspect-normalized dominant offset -- the rule
    /// `resnap_flow_endpoints` snaps by, so a snapped end classifies as the
    /// face it was snapped to.
    fn of(p: &FlowPoint, stock: (f64, f64)) -> Side {
        let dx = p.x - stock.0;
        let dy = p.y - stock.1;
        if (STOCK_HEIGHT / 2.0) * dx.abs() >= (STOCK_WIDTH / 2.0) * dy.abs() {
            if dx >= 0.0 { Side::Right } else { Side::Left }
        } else if dy >= 0.0 {
            Side::Bottom
        } else {
            Side::Top
        }
    }

    /// Whether a point moves along this face by changing its x.
    fn runs_along_x(self) -> bool {
        matches!(self, Side::Top | Side::Bottom)
    }

    fn along(self, p: &FlowPoint) -> f64 {
        if self.runs_along_x() { p.x } else { p.y }
    }

    fn set_along(self, p: &mut FlowPoint, v: f64) {
        if self.runs_along_x() {
            p.x = v;
        } else {
            p.y = v;
        }
    }

    /// The span of positions along the face that keep `CORNER_CLEARANCE`
    /// from both corners.
    fn clearance_span(self, stock: (f64, f64)) -> (f64, f64) {
        let (center, half) = if self.runs_along_x() {
            (stock.0, STOCK_WIDTH / 2.0)
        } else {
            (stock.1, STOCK_HEIGHT / 2.0)
        };
        let reach = half - CORNER_CLEARANCE;
        (center - reach, center + reach)
    }
}

/// The gaps `occupied` leaves on `span`, in order. Occupied positions outside
/// the span count at its nearest end.
fn free_gaps(span: (f64, f64), occupied: &[f64]) -> Vec<(f64, f64)> {
    let mut bounds: Vec<f64> = occupied.iter().map(|v| v.clamp(span.0, span.1)).collect();
    bounds.push(span.0);
    bounds.push(span.1);
    bounds.sort_by(f64::total_cmp);
    bounds.windows(2).map(|w| (w[0], w[1])).collect()
}

/// The center of the longest interval in `intervals`. Among intervals of equal
/// length, the one whose center is nearest `preferred` wins, then the lower
/// one, so the choice is deterministic.
fn longest_center(intervals: &[(f64, f64)], preferred: f64) -> Option<f64> {
    let mut best: Option<(f64, f64)> = None;
    for &(lo, hi) in intervals {
        let len = hi - lo;
        let center = (lo + hi) / 2.0;
        let better = match best {
            None => true,
            Some((best_len, best_center)) => {
                len > best_len + EPS
                    || ((len - best_len).abs() <= EPS
                        && (center - preferred).abs() < (best_center - preferred).abs() - EPS)
            }
        };
        if better {
            best = Some((len, center));
        }
    }
    best.map(|(_, center)| center)
}

/// The center of the largest gap `occupied` leaves on `span` ([`free_gaps`],
/// [`longest_center`]).
pub(crate) fn largest_free_gap_center(span: (f64, f64), occupied: &[f64], preferred: f64) -> f64 {
    longest_center(&free_gaps(span, occupied), preferred).unwrap_or((span.0 + span.1) / 2.0)
}

/// A stock end of a created flow, with its face.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
struct FaceEnd {
    end: usize,
    stock_uid: i32,
    stock: (f64, f64),
    side: Side,
}

/// The positions along `e`'s face of every other flow end on that face: the
/// ends of flows outside `created`, and the created ends already `placed`.
fn occupied_on_face(
    elements: &[ViewElement],
    e: &FaceEnd,
    uid: i32,
    created: &HashSet<i32>,
    placed: &HashSet<(i32, usize)>,
) -> Vec<f64> {
    elements
        .iter()
        .filter_map(|el| match el {
            ViewElement::Flow(g) if g.points.len() >= 2 => Some(g),
            _ => None,
        })
        .flat_map(|g| {
            let g_last = g.points.len() - 1;
            [0, g_last].into_iter().map(move |end| (g, end))
        })
        .filter(|&(g, end)| {
            g.uid != uid && (!created.contains(&g.uid) || placed.contains(&(g.uid, end)))
        })
        .filter_map(|(g, end)| {
            let q = &g.points[end];
            (q.attached_to_uid == Some(e.stock_uid) && Side::of(q, e.stock) == e.side)
                .then(|| e.side.along(q))
        })
        .collect()
}

/// Move the stock ends of each flow in `created` into free slots on their
/// faces, and each created flow's cloud end clear of the other clouds.
///
/// The ends a slot is measured against are those of every flow not in
/// `created`, plus the created ends already placed (flows are processed in uid
/// order, so the result is deterministic). A two-point flow between two stocks
/// whose ends sit on faces running along the same coordinate takes the center
/// of the longest overlap of the two faces' free gaps, for both ends and the
/// valve, so the pipe stays straight; without an overlap, and for every other
/// flow, each stock end takes the largest free gap on its face. An end whose
/// flow has a free far end (a cloud or nothing) moves with its whole pipe and
/// valve, so the pipe keeps its shape; an end whose far end is on a stock
/// moves alone, and the finishing pass reroutes the pipe. Then a created
/// flow's free end is pushed out along its pipe, `2 * CLOUD_RADIUS` at a
/// time, while its cloud would overlap another; clouds themselves are left
/// for the finishing pass's normalization, which recenters them on their ends.
pub(crate) fn place_created_flow_ends(elements: &mut [ViewElement], created: &HashSet<i32>) {
    let stocks: HashMap<i32, (f64, f64)> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();
    let mut order: Vec<(i32, usize)> = elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            ViewElement::Flow(f) if created.contains(&f.uid) && f.points.len() >= 2 => {
                Some((f.uid, i))
            }
            _ => None,
        })
        .collect();
    order.sort_unstable();

    let mut placed: HashSet<(i32, usize)> = HashSet::new();
    for &(uid, idx) in &order {
        let ViewElement::Flow(f) = &elements[idx] else {
            unreachable!()
        };
        let last = f.points.len() - 1;
        let face_end = |end: usize| -> Option<FaceEnd> {
            let p = &f.points[end];
            let stock_uid = p.attached_to_uid.filter(|u| stocks.contains_key(u))?;
            let stock = stocks[&stock_uid];
            Some(FaceEnd {
                end,
                stock_uid,
                stock,
                side: Side::of(p, stock),
            })
        };
        let ends: Vec<FaceEnd> = [0, last].into_iter().filter_map(face_end).collect();

        let joint = if last == 1
            && ends.len() == 2
            && ends[0].side.runs_along_x() == ends[1].side.runs_along_x()
        {
            let gaps_a = free_gaps(
                ends[0].side.clearance_span(ends[0].stock),
                &occupied_on_face(elements, &ends[0], uid, created, &placed),
            );
            let gaps_b = free_gaps(
                ends[1].side.clearance_span(ends[1].stock),
                &occupied_on_face(elements, &ends[1], uid, created, &placed),
            );
            let overlaps: Vec<(f64, f64)> = gaps_a
                .iter()
                .flat_map(|a| gaps_b.iter().map(move |b| (a.0.max(b.0), a.1.min(b.1))))
                .filter(|(lo, hi)| hi - lo > EPS)
                .collect();
            let current = ends[0].side.along(&f.points[0]);
            longest_center(&overlaps, current)
        } else {
            None
        };

        let ViewElement::Flow(f) = &mut elements[idx] else {
            unreachable!()
        };
        if let Some(line) = joint {
            let side = ends[0].side;
            for e in &ends {
                side.set_along(&mut f.points[e.end], line);
                placed.insert((uid, e.end));
            }
            if side.runs_along_x() {
                f.x = line;
            } else {
                f.y = line;
            }
            continue;
        }

        for e in &ends {
            let occupied = occupied_on_face(elements, e, uid, created, &placed);
            let ViewElement::Flow(f) = &mut elements[idx] else {
                unreachable!()
            };
            let current = e.side.along(&f.points[e.end]);
            let target =
                largest_free_gap_center(e.side.clearance_span(e.stock), &occupied, current);
            let delta = target - current;
            let far = if e.end == 0 { last } else { 0 };
            let far_on_stock = f.points[far]
                .attached_to_uid
                .is_some_and(|u| stocks.contains_key(&u));
            if delta.abs() > EPS {
                if far_on_stock {
                    e.side.set_along(&mut f.points[e.end], target);
                } else {
                    for pt in &mut f.points {
                        let v = e.side.along(pt) + delta;
                        e.side.set_along(pt, v);
                    }
                    let mut valve = FlowPoint {
                        x: f.x,
                        y: f.y,
                        attached_to_uid: None,
                    };
                    let v = e.side.along(&valve) + delta;
                    e.side.set_along(&mut valve, v);
                    (f.x, f.y) = (valve.x, valve.y);
                }
            }
            placed.insert((uid, e.end));
        }
    }

    separate_created_clouds(elements, created, &stocks, &order);
}

/// Push each created flow's free end out along its end segment until the cloud
/// on it overlaps no other cloud: every other flow's free end, a created one
/// once it is settled.
fn separate_created_clouds(
    elements: &mut [ViewElement],
    created: &HashSet<i32>,
    stocks: &HashMap<i32, (f64, f64)>,
    order: &[(i32, usize)],
) {
    let is_free = |p: &FlowPoint| !p.attached_to_uid.is_some_and(|u| stocks.contains_key(&u));
    let mut settled: HashSet<i32> = HashSet::new();
    for &(uid, idx) in order {
        let ViewElement::Flow(f) = &elements[idx] else {
            unreachable!()
        };
        let last = f.points.len() - 1;
        let free_ends: Vec<(usize, usize)> = [(0, 1), (last, last - 1)]
            .into_iter()
            .filter(|&(end, _)| is_free(&f.points[end]))
            .collect();
        for (end, adj) in free_ends {
            let others: Vec<(f64, f64)> = elements
                .iter()
                .filter_map(|el| match el {
                    ViewElement::Flow(g)
                        if g.uid != uid
                            && g.points.len() >= 2
                            && (!created.contains(&g.uid) || settled.contains(&g.uid)) =>
                    {
                        Some(g)
                    }
                    _ => None,
                })
                .flat_map(|g| {
                    let g_last = g.points.len() - 1;
                    [0, g_last]
                        .into_iter()
                        .map(move |i| &g.points[i])
                        .filter(|q| is_free(q))
                        .map(|q| (q.x, q.y))
                })
                .collect();
            let ViewElement::Flow(f) = &mut elements[idx] else {
                unreachable!()
            };
            let (dx, dy) = (
                f.points[end].x - f.points[adj].x,
                f.points[end].y - f.points[adj].y,
            );
            let len = dx.hypot(dy);
            if len <= EPS {
                continue;
            }
            let step = 2.0 * CLOUD_RADIUS;
            for _ in 0..MAX_CLOUD_PUSHES {
                let p = &f.points[end];
                let overlaps = others
                    .iter()
                    .any(|&(x, y)| (x - p.x).hypot(y - p.y) < step - EPS);
                if !overlaps {
                    break;
                }
                f.points[end].x += dx / len * step;
                f.points[end].y += dy / len * step;
            }
        }
        settled.insert(uid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::datamodel::view_element::{self, LabelSide};

    /// Rows: an empty face; one occupant, preferring each side of it; a tie
    /// at equal distance; an occupant beyond the span; several occupants
    /// whose largest gap is not the preferred one.
    #[test]
    fn largest_free_gap_center_rows() {
        let span = (-19.5, 19.5);
        let rows: [(&str, &[f64], f64, f64); 6] = [
            ("empty face", &[], 5.0, 0.0),
            ("one occupant, preferring above", &[0.0], 7.5, 9.75),
            ("one occupant, preferring below", &[0.0], -7.5, -9.75),
            (
                "tie at equal distance takes the lower gap",
                &[0.0],
                0.0,
                -9.75,
            ),
            (
                "occupant beyond the span counts at its end",
                &[40.0],
                19.0,
                0.0,
            ),
            (
                "the largest gap wins over the preferred one",
                &[-7.5, 7.5],
                -15.0,
                0.0,
            ),
        ];
        for (label, occupied, preferred, expected) in rows {
            let got = largest_free_gap_center(span, occupied, preferred);
            assert!(
                (got - expected).abs() < 1e-9,
                "{label}: got {got}, expected {expected}"
            );
        }
    }

    fn stock(uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Stock(view_element::Stock {
            name: format!("s{uid}"),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    }

    fn flow(uid: i32, valve: (f64, f64), points: &[(f64, f64, Option<i32>)]) -> ViewElement {
        ViewElement::Flow(view_element::Flow {
            name: format!("f{uid}"),
            uid,
            x: valve.0,
            y: valve.1,
            label_side: LabelSide::Bottom,
            points: points
                .iter()
                .map(|&(x, y, attached_to_uid)| FlowPoint {
                    x,
                    y,
                    attached_to_uid,
                })
                .collect(),
            compat: None,
            label_compat: None,
        })
    }

    fn flow_of(elements: &[ViewElement], uid: i32) -> &view_element::Flow {
        elements
            .iter()
            .find_map(|e| match e {
                ViewElement::Flow(f) if f.uid == uid => Some(f),
                _ => None,
            })
            .unwrap()
    }

    fn coords(f: &view_element::Flow) -> Vec<(f64, f64)> {
        f.points.iter().map(|p| (p.x, p.y)).collect()
    }

    /// Rows, one per way an end moves: a created cloud flow on a face a
    /// preserved flow occupies moves with its pipe and valve; a created
    /// stock-to-stock flow whose faces cannot share a line (a bottom face and a
    /// left face) moves only the end on the occupied face; a created flow alone
    /// on its face stays at the center; two created flows on one face do not
    /// coincide; a created stock-to-stock flow between two parallel faces takes
    /// one line off both occupants; a created cloud end within a cloud's width
    /// of another cloud is pushed out along its pipe. The preserved flow never
    /// moves.
    ///
    /// These views are built by hand in the shape incremental layout hands the
    /// function: created stock ends already snapped onto a face by
    /// `resnap_flow_endpoints` (on the face, the pipe leaving perpendicular),
    /// free ends attached to cloud uids, and `created` holding the uids of the
    /// flows the pass built. That composition through production is pinned by
    /// `layout::tests::flow_tests` (a flow added on its face, a chain flow added
    /// on a face it occupies, SIR's relapse, mark2's two outflows).
    #[test]
    fn place_created_flow_ends_rows() {
        // Stock 1 at (100, 100): bottom face y = 117.5, span x in [80.5, 119.5].
        let preserved = flow(
            10,
            (100.0, 160.0),
            &[(100.0, 117.5, Some(1)), (100.0, 200.0, Some(98))],
        );

        let mut elements = vec![
            stock(1, 100.0, 100.0),
            preserved.clone(),
            flow(
                11,
                (107.5, 160.0),
                &[(107.5, 117.5, Some(1)), (107.5, 240.0, Some(99))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([11]));
        let f = flow_of(&elements, 11);
        assert_eq!(
            (f.x, f.y),
            (109.75, 160.0),
            "cloud flow: valve moves with the pipe"
        );
        assert_eq!(
            coords(f),
            vec![(109.75, 117.5), (109.75, 240.0)],
            "cloud flow: the pipe keeps its shape"
        );
        assert!(
            elements.contains(&preserved),
            "the preserved flow never moves"
        );

        // Stock 2 at (300, 300): its left face is x = 277.5, span y in [285.5, 314.5].
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(2, 300.0, 300.0),
            preserved.clone(),
            flow(
                12,
                (100.0, 300.0),
                &[(100.0, 117.5, Some(1)), (277.5, 300.0, Some(2))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([12]));
        let f = flow_of(&elements, 12);
        assert_eq!(
            (f.x, f.y),
            (100.0, 300.0),
            "perpendicular faces: the valve stays"
        );
        assert_eq!(
            coords(f),
            vec![(80.5 + 19.5 / 2.0, 117.5), (277.5, 300.0)],
            "perpendicular faces: only the end on the occupied face moves"
        );

        let mut elements = vec![
            stock(1, 100.0, 100.0),
            flow(
                13,
                (90.0, 160.0),
                &[(90.0, 117.5, Some(1)), (90.0, 200.0, Some(99))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([13]));
        assert_eq!(
            flow_of(&elements, 13).points[0].x,
            100.0,
            "alone on its face: center"
        );

        let mut elements = vec![
            stock(1, 100.0, 100.0),
            flow(
                14,
                (100.0, 160.0),
                &[(100.0, 117.5, Some(1)), (100.0, 200.0, Some(98))],
            ),
            flow(
                15,
                (100.0, 160.0),
                &[(100.0, 117.5, Some(1)), (100.0, 200.0, Some(99))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([14, 15]));
        let (a, b) = (
            flow_of(&elements, 14).points[0].x,
            flow_of(&elements, 15).points[0].x,
        );
        assert_eq!(
            a, 100.0,
            "the first created flow takes the empty face's center"
        );
        assert!(
            (a - b).abs() > 1.0,
            "two created flows on one face do not coincide: {a}, {b}"
        );

        // Stocks 1 at (100, 100) and 3 at (300, 100): a preserved flow runs
        // between their facing left/right faces at y = 100 (spans [85.5, 114.5]).
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(3, 300.0, 100.0),
            flow(
                16,
                (200.0, 100.0),
                &[(122.5, 100.0, Some(1)), (277.5, 100.0, Some(3))],
            ),
            flow(
                17,
                (200.0, 100.0),
                &[(122.5, 100.0, Some(3)), (277.5, 100.0, Some(1))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([17]));
        let f = flow_of(&elements, 17);
        assert_eq!(
            coords(f),
            vec![(122.5, 92.75), (277.5, 92.75)],
            "parallel faces: one line, the center of the longer overlap (a tie, so the lower)"
        );
        assert_eq!(
            (f.x, f.y),
            (200.0, 92.75),
            "parallel faces: the valve on the line"
        );

        // A preserved cloud flow leaves stock 1's right face at y = 100 into a
        // cloud at (150, 100); a created one leaves 7.25 below it.
        let mut elements = vec![
            stock(1, 100.0, 100.0),
            flow(
                18,
                (136.0, 100.0),
                &[(122.5, 100.0, Some(1)), (150.0, 100.0, Some(98))],
            ),
            flow(
                19,
                (136.0, 107.25),
                &[(122.5, 107.25, Some(1)), (150.0, 107.25, Some(99))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([19]));
        let f = flow_of(&elements, 19);
        assert_eq!(
            coords(f),
            vec![(122.5, 107.25), (150.0 + 2.0 * CLOUD_RADIUS, 107.25)],
            "a cloud end within a cloud's width of another is pushed out along the pipe"
        );
    }
}
