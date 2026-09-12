// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Placing the ends of imported flows that the sketch does not place.
//!
//! `processing::resolve_flow_ends` decides what each end of a flow attaches
//! to, from the model's stock lists. Where the sketch draws a pipe end, the end
//! is placed there during conversion. This module places the rest, over the
//! merged datamodel view where every stock's center is known, before
//! `diagram::flow_geometry::normalize_flow_geometry` brings the pipes onto the
//! stock faces:
//!
//! - a pipe end at a stock that does not list the flow becomes a cloud just
//!   outside that stock, along the pipe;
//! - a side the model links to a stock the sketch draws no pipe into gets a
//!   pipe from the valve to that stock, continuing through the valve, with a
//!   bend when the stock is not ahead of it;
//! - a side with neither gets a cloud `CLOUD_DISTANCE` from the valve, on the
//!   valve's other side from the flow's other end.
//!
//! The valve stays where the sketch drew it (for a flow drawn as a bare label,
//! the label's position) and the pipe runs through it.

use std::collections::HashMap;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::{Cloud, FlowPoint};
use crate::diagram::constants::{CLOUD_RADIUS, STOCK_HEIGHT, STOCK_WIDTH};

/// How far from the valve a synthesized cloud sits: the pipe half-length the
/// engine's own layout gives a new cloud-ended flow
/// (`layout::create_flow_view_element`).
const CLOUD_DISTANCE: f64 = 50.0;

/// The editor's valve margin: the valve is kept at least this far from the
/// ends of the segment it sits on.
const VALVE_MARGIN: f64 = 10.0;

/// How far past the valve a route turns toward a stock that is not ahead of
/// the valve, so the valve keeps its margin from the bend.
const BEND_STEP: f64 = 2.0 * VALVE_MARGIN;

/// One end of a flow whose route is placed after the views are merged.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(super) enum RouteEnd {
    /// Placed by the sketch: the pipe end, attached to its stock or cloud.
    Point(FlowPoint),
    /// A cloud near the sketch's pipe end `end`, kept clear of the stock the
    /// sketch drew at `stock`.
    CloudNearStock { end: (f64, f64), stock: (f64, f64) },
    /// Attach to the stock element with this uid.
    Stock(i32),
    /// A cloud on the valve's far side from the other end.
    Free,
}

/// A flow whose ends are not all placed by the sketch.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
pub(super) struct PendingFlowRoute {
    pub(super) flow_uid: i32,
    pub(super) source: RouteEnd,
    pub(super) sink: RouteEnd,
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy)]
enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// The axis a pipe from `from` to `to` runs along.
    fn dominant(from: (f64, f64), to: (f64, f64)) -> Axis {
        if (to.0 - from.0).abs() >= (to.1 - from.1).abs() {
            Axis::Horizontal
        } else {
            Axis::Vertical
        }
    }

    fn along(self, p: (f64, f64)) -> f64 {
        match self {
            Axis::Horizontal => p.0,
            Axis::Vertical => p.1,
        }
    }

    fn cross(self, p: (f64, f64)) -> f64 {
        match self {
            Axis::Horizontal => p.1,
            Axis::Vertical => p.0,
        }
    }

    fn point(self, along: f64, cross: f64) -> (f64, f64) {
        match self {
            Axis::Horizontal => (along, cross),
            Axis::Vertical => (cross, along),
        }
    }

    fn half_along(self) -> f64 {
        match self {
            Axis::Horizontal => STOCK_WIDTH / 2.0,
            Axis::Vertical => STOCK_HEIGHT / 2.0,
        }
    }
}

/// The sign of `v`, with zero taken as positive so a degenerate route still
/// has a direction.
fn direction(v: f64) -> f64 {
    if v < 0.0 { -1.0 } else { 1.0 }
}

fn attached(p: (f64, f64), uid: i32) -> FlowPoint {
    FlowPoint {
        x: p.0,
        y: p.1,
        attached_to_uid: Some(uid),
    }
}

/// Place every pending flow's points, creating the clouds its routes end in.
pub(super) fn route_pending_flows(elements: &mut Vec<ViewElement>, pending: &[PendingFlowRoute]) {
    if pending.is_empty() {
        return;
    }
    let stocks: HashMap<i32, (f64, f64)> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();
    let mut next_uid = elements.iter().map(|e| e.get_uid()).max().unwrap_or(0) + 1;
    let mut clouds: Vec<ViewElement> = Vec::new();
    for route in pending {
        let Some(ViewElement::Flow(flow)) = elements
            .iter_mut()
            .find(|e| matches!(e, ViewElement::Flow(f) if f.uid == route.flow_uid))
        else {
            continue;
        };
        let mut new_cloud = |p: (f64, f64)| -> FlowPoint {
            let uid = next_uid;
            next_uid += 1;
            clouds.push(ViewElement::Cloud(Cloud {
                uid,
                flow_uid: route.flow_uid,
                x: p.0,
                y: p.1,
                compat: None,
            }));
            attached(p, uid)
        };
        let source = resolve_stock(&route.source, &stocks);
        let sink = resolve_stock(&route.sink, &stocks);
        flow.points = plan_route((flow.x, flow.y), source, sink, &mut new_cloud);
    }
    elements.extend(clouds);
}

/// A route end with its stock's position resolved. A stock uid with no stock
/// element (not produced by the importer, but not assumed away) is an end with
/// no stock.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
enum End<'a> {
    Point(&'a FlowPoint),
    CloudNearStock { end: (f64, f64), stock: (f64, f64) },
    Stock { uid: i32, at: (f64, f64) },
    Free,
}

fn resolve_stock<'a>(end: &'a RouteEnd, stocks: &HashMap<i32, (f64, f64)>) -> End<'a> {
    match end {
        RouteEnd::Point(p) => End::Point(p),
        RouteEnd::CloudNearStock { end, stock } => End::CloudNearStock {
            end: *end,
            stock: *stock,
        },
        RouteEnd::Stock(uid) => match stocks.get(uid) {
            Some(&at) => End::Stock { uid: *uid, at },
            None => End::Free,
        },
        RouteEnd::Free => End::Free,
    }
}

/// The points of one flow, source first.
fn plan_route(
    valve: (f64, f64),
    source: End<'_>,
    sink: End<'_>,
    new_cloud: &mut dyn FnMut((f64, f64)) -> FlowPoint,
) -> Vec<FlowPoint> {
    let sketch_end = |end: &End<'_>| -> Option<(f64, f64)> {
        match end {
            End::Point(p) => Some((p.x, p.y)),
            End::CloudNearStock { end, .. } => Some(*end),
            End::Stock { .. } | End::Free => None,
        }
    };
    match (sketch_end(&source), sketch_end(&sink)) {
        (Some(_), Some(_)) => {
            let first = place_sketch_end(&source, valve, new_cloud);
            let last = place_sketch_end(&sink, valve, new_cloud);
            vec![first, last]
        }
        (Some(_), None) => {
            let first = place_sketch_end(&source, valve, new_cloud);
            route_through_valve(first, valve, &sink, new_cloud)
        }
        (None, Some(_)) => {
            let last = place_sketch_end(&sink, valve, new_cloud);
            let mut points = route_through_valve(last, valve, &source, new_cloud);
            points.reverse();
            points
        }
        (None, None) => route_without_sketch(valve, &source, &sink, new_cloud),
    }
}

/// A sketch-placed end: the pipe end itself, or a cloud pulled back along the
/// pipe from the unlinked stock the pipe was drawn into, to just outside the
/// 45x35 box (never past the valve's margin).
fn place_sketch_end(
    end: &End<'_>,
    valve: (f64, f64),
    new_cloud: &mut dyn FnMut((f64, f64)) -> FlowPoint,
) -> FlowPoint {
    match end {
        End::Point(p) => (*p).clone(),
        End::CloudNearStock { end, stock } => {
            let axis = Axis::dominant(valve, *end);
            let toward = direction(axis.along(*stock) - axis.along(valve));
            let mut along = axis.along(*stock) - toward * (axis.half_along() + CLOUD_RADIUS);
            if toward * (along - axis.along(valve)) < VALVE_MARGIN {
                along = axis.along(valve) + toward * VALVE_MARGIN;
            }
            new_cloud(axis.point(along, axis.cross(*end)))
        }
        End::Stock { .. } | End::Free => unreachable!("only sketch-placed ends are placed here"),
    }
}

/// `[first, ..., other end]`: the pipe runs from the placed end `first`
/// through the valve and on to the other end, which is a stock (straight on
/// when the stock is ahead of the valve, turning `BEND_STEP` past the valve
/// otherwise) or a cloud `CLOUD_DISTANCE` past the valve.
fn route_through_valve(
    first: FlowPoint,
    valve: (f64, f64),
    other: &End<'_>,
    new_cloud: &mut dyn FnMut((f64, f64)) -> FlowPoint,
) -> Vec<FlowPoint> {
    let placed = (first.x, first.y);
    let axis = Axis::dominant(placed, valve);
    let ahead = direction(axis.along(valve) - axis.along(placed));
    // The pipe's line is the placed end's, so the pipe stays straight; the
    // valve is on it wherever the sketch drew the pipe through the valve.
    let line = axis.cross(placed);
    match other {
        End::Stock { uid, at } => {
            if ahead * (axis.along(*at) - axis.along(valve)) >= VALVE_MARGIN {
                vec![first, attached(axis.point(axis.along(*at), line), *uid)]
            } else {
                let bend_along = axis.along(valve) + ahead * BEND_STEP;
                let bend = axis.point(bend_along, line);
                vec![
                    first,
                    FlowPoint {
                        x: bend.0,
                        y: bend.1,
                        attached_to_uid: None,
                    },
                    attached(axis.point(bend_along, axis.cross(*at)), *uid),
                ]
            }
        }
        End::Free | End::Point(_) | End::CloudNearStock { .. } => {
            let cloud = new_cloud(axis.point(axis.along(valve) + ahead * CLOUD_DISTANCE, line));
            vec![first, cloud]
        }
    }
}

/// A flow with no sketch pipe end at all: the pipe runs through the valve
/// along the axis toward its stock (or between its two stocks), with a cloud
/// on each side that has no stock.
fn route_without_sketch(
    valve: (f64, f64),
    source: &End<'_>,
    sink: &End<'_>,
    new_cloud: &mut dyn FnMut((f64, f64)) -> FlowPoint,
) -> Vec<FlowPoint> {
    match (source, sink) {
        (End::Stock { uid: a, at: pa }, End::Stock { uid: b, at: pb }) => {
            let axis = Axis::dominant(*pa, *pb);
            let line = axis.cross(valve);
            vec![
                attached(axis.point(axis.along(*pa), line), *a),
                attached(axis.point(axis.along(*pb), line), *b),
            ]
        }
        (End::Free, End::Stock { uid, at }) => {
            let axis = Axis::dominant(valve, *at);
            let toward = direction(axis.along(*at) - axis.along(valve));
            let line = axis.cross(valve);
            let cloud = new_cloud(axis.point(axis.along(valve) - toward * CLOUD_DISTANCE, line));
            vec![cloud, attached(axis.point(axis.along(*at), line), *uid)]
        }
        (End::Stock { uid, at }, End::Free) => {
            let axis = Axis::dominant(valve, *at);
            let toward = direction(axis.along(*at) - axis.along(valve));
            let line = axis.cross(valve);
            let stock_end = attached(axis.point(axis.along(*at), line), *uid);
            let cloud = new_cloud(axis.point(axis.along(valve) - toward * CLOUD_DISTANCE, line));
            vec![stock_end, cloud]
        }
        _ => {
            let source_cloud = new_cloud((valve.0 - CLOUD_DISTANCE, valve.1));
            let sink_cloud = new_cloud((valve.0 + CLOUD_DISTANCE, valve.1));
            vec![source_cloud, sink_cloud]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cloud_maker(next_uid: &mut i32) -> impl FnMut((f64, f64)) -> FlowPoint + '_ {
        move |p| {
            let uid = *next_uid;
            *next_uid += 1;
            attached(p, uid)
        }
    }

    fn coords(points: &[FlowPoint]) -> Vec<(f64, f64, Option<i32>)> {
        points
            .iter()
            .map(|p| (p.x, p.y, p.attached_to_uid))
            .collect()
    }

    /// A route from a placed end through the valve to a stock the model links:
    /// straight on when the stock is ahead of the valve by at least the
    /// margin, and otherwise turning `BEND_STEP` past the valve toward the
    /// stock's line, so the valve keeps its margin from the bend.
    #[test]
    fn a_route_to_its_stock_turns_past_the_valve_when_the_stock_is_behind() {
        let mut uid = 100;
        let placed = attached((0.0, 100.0), 1);

        let ahead = End::Stock {
            uid: 7,
            at: (300.0, 180.0),
        };
        let points = route_through_valve(
            placed.clone(),
            (50.0, 100.0),
            &ahead,
            &mut cloud_maker(&mut uid),
        );
        assert_eq!(
            coords(&points),
            vec![(0.0, 100.0, Some(1)), (300.0, 100.0, Some(7))],
            "stock ahead: straight on"
        );

        let behind = End::Stock {
            uid: 7,
            at: (40.0, 300.0),
        };
        let points =
            route_through_valve(placed, (50.0, 100.0), &behind, &mut cloud_maker(&mut uid));
        assert_eq!(
            coords(&points),
            vec![
                (0.0, 100.0, Some(1)),
                (50.0 + BEND_STEP, 100.0, None),
                (50.0 + BEND_STEP, 300.0, Some(7))
            ],
            "stock behind the valve: a bend BEND_STEP past it"
        );
    }

    /// A cloud for a pipe end drawn into a stock that does not list the flow
    /// sits just outside that stock along the pipe, but never within the
    /// valve's margin: rows for a stock far from the valve and one close to it.
    #[test]
    fn a_cloud_near_an_unlinked_stock_stays_off_the_valve() {
        let mut uid = 100;
        let far = End::CloudNearStock {
            end: (300.0, 100.0),
            stock: (300.0, 100.0),
        };
        let cloud = place_sketch_end(&far, (100.0, 100.0), &mut cloud_maker(&mut uid));
        assert_eq!(
            (cloud.x, cloud.y),
            (300.0 - (STOCK_WIDTH / 2.0 + CLOUD_RADIUS), 100.0),
            "just outside the stock, along the pipe"
        );

        let near = End::CloudNearStock {
            end: (140.0, 100.0),
            stock: (130.0, 100.0),
        };
        let cloud = place_sketch_end(&near, (100.0, 100.0), &mut cloud_maker(&mut uid));
        assert_eq!(
            (cloud.x, cloud.y),
            (100.0 + VALVE_MARGIN, 100.0),
            "a stock close to the valve: kept the margin off the valve"
        );
    }
}
