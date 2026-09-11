// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Where a flow that incremental layout creates meets a stock face.
//!
//! Incremental layout never moves a flow the patch did not touch, so a flow it
//! creates fits around the ends already on a face instead of re-spacing them:
//! its stock end takes the largest free gap on the face, bounded by those ends
//! and by the face's corner clearance. It therefore never lands on a preserved
//! sibling, and never in a corner zone.

use std::collections::{HashMap, HashSet};

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::FlowPoint;
use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};
use crate::diagram::flow_geometry::CORNER_CLEARANCE;

const EPS: f64 = 1e-6;

/// A face of a stock.
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

/// The center of the largest gap `occupied` leaves on `span`. Occupied
/// positions outside the span count at its nearest end. Among gaps of equal
/// length, the one whose center is nearest `preferred` wins, then the lower
/// one, so the choice is deterministic.
pub(crate) fn largest_free_gap_center(span: (f64, f64), occupied: &[f64], preferred: f64) -> f64 {
    let mut bounds: Vec<f64> = occupied.iter().map(|v| v.clamp(span.0, span.1)).collect();
    bounds.push(span.0);
    bounds.push(span.1);
    bounds.sort_by(f64::total_cmp);
    let mut best: Option<(f64, f64)> = None;
    for w in bounds.windows(2) {
        let len = w[1] - w[0];
        let center = (w[0] + w[1]) / 2.0;
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
    best.map_or((span.0 + span.1) / 2.0, |(_, center)| center)
}

/// Move every stock end of each flow in `created` into the largest free gap on
/// its face.
///
/// The ends a gap is measured against are those of every flow not in
/// `created`, plus the created ends already placed (flows are processed in uid
/// order, so the result is deterministic). An end whose flow has a free far
/// end (a cloud or nothing) moves with its whole pipe and valve, so the pipe
/// keeps its shape; an end whose far end is on a stock moves alone, and the
/// finishing pass reroutes the pipe. Clouds are left for the finishing pass's
/// normalization, which recenters them on their ends.
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
    for (uid, idx) in order {
        let ViewElement::Flow(f) = &elements[idx] else {
            unreachable!()
        };
        let last = f.points.len() - 1;
        for (end, far) in [(0, last), (last, 0)] {
            let ViewElement::Flow(f) = &elements[idx] else {
                unreachable!()
            };
            let p = &f.points[end];
            let Some(stock_uid) = p.attached_to_uid.filter(|u| stocks.contains_key(u)) else {
                continue;
            };
            let stock = stocks[&stock_uid];
            let side = Side::of(p, stock);
            let occupied: Vec<f64> = elements
                .iter()
                .filter_map(|e| match e {
                    ViewElement::Flow(g) if g.points.len() >= 2 => Some(g),
                    _ => None,
                })
                .flat_map(|g| {
                    let g_last = g.points.len() - 1;
                    [0, g_last].into_iter().map(move |e| (g, e))
                })
                .filter(|&(g, e)| {
                    (g.uid, e) != (uid, end)
                        && (!created.contains(&g.uid) || placed.contains(&(g.uid, e)))
                })
                .filter_map(|(g, e)| {
                    let q = &g.points[e];
                    (q.attached_to_uid == Some(stock_uid) && Side::of(q, stock) == side)
                        .then(|| side.along(q))
                })
                .collect();
            let current = side.along(p);
            let target = largest_free_gap_center(side.clearance_span(stock), &occupied, current);
            let delta = target - current;
            let far_on_stock = f.points[far]
                .attached_to_uid
                .is_some_and(|u| stocks.contains_key(&u));

            let ViewElement::Flow(f) = &mut elements[idx] else {
                unreachable!()
            };
            if delta.abs() > EPS {
                let shift = |v: &mut f64| *v += delta;
                if far_on_stock {
                    if side.runs_along_x() {
                        f.points[end].x = target;
                    } else {
                        f.points[end].y = target;
                    }
                } else if side.runs_along_x() {
                    f.points.iter_mut().for_each(|pt| shift(&mut pt.x));
                    shift(&mut f.x);
                } else {
                    f.points.iter_mut().for_each(|pt| shift(&mut pt.y));
                    shift(&mut f.y);
                }
            }
            placed.insert((uid, end));
        }
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

    /// Rows, one per way an end moves: a created cloud flow on a face a
    /// preserved flow occupies moves with its pipe and valve; a created
    /// stock-to-stock flow's end moves alone; a created flow alone on its face
    /// stays at the center; two created flows on one face do not coincide.
    /// The preserved flow never moves.
    #[test]
    fn place_created_flow_ends_rows() {
        // Stock 1 at (100, 100): bottom face y = 117.5, span x in [80.5, 119.5].
        let preserved = flow(
            10,
            (100.0, 160.0),
            &[(100.0, 117.5, Some(1)), (100.0, 200.0, None)],
        );

        let mut elements = vec![
            stock(1, 100.0, 100.0),
            preserved.clone(),
            flow(
                11,
                (107.5, 160.0),
                &[(107.5, 117.5, Some(1)), (107.5, 200.0, Some(99))],
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
            f.points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
            vec![(109.75, 117.5), (109.75, 200.0)],
            "cloud flow: the pipe keeps its shape"
        );
        assert!(
            elements.contains(&preserved),
            "the preserved flow never moves"
        );

        let mut elements = vec![
            stock(1, 100.0, 100.0),
            stock(2, 100.0, 300.0),
            preserved.clone(),
            flow(
                12,
                (100.0, 200.0),
                &[(100.0, 117.5, Some(1)), (100.0, 282.5, Some(2))],
            ),
        ];
        place_created_flow_ends(&mut elements, &HashSet::from([12]));
        let f = flow_of(&elements, 12);
        assert_eq!(
            (f.x, f.y),
            (100.0, 200.0),
            "stock-to-stock flow: the valve stays"
        );
        assert_eq!(
            f.points.iter().map(|p| (p.x, p.y)).collect::<Vec<_>>(),
            vec![(80.5 + 19.5 / 2.0, 117.5), (100.0, 282.5)],
            "stock-to-stock flow: only the end on the occupied face moves"
        );

        let mut elements = vec![
            stock(1, 100.0, 100.0),
            flow(
                13,
                (90.0, 160.0),
                &[(90.0, 117.5, Some(1)), (90.0, 200.0, None)],
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
                &[(100.0, 117.5, Some(1)), (100.0, 200.0, None)],
            ),
            flow(
                15,
                (100.0, 160.0),
                &[(100.0, 117.5, Some(1)), (100.0, 200.0, None)],
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
    }
}
