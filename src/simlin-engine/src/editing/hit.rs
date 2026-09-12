// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Hit testing: the element, and the part of it, a point lands on.
//!
//! A touch covers an area, not a point, and the things a gesture grabs overlap:
//! a flow's arrowhead touches its stock's face, a link's arrowhead runs under
//! the label of the flow it points at, a label can hang over a neighbor's body.
//! So a hit is decided in tiers:
//!
//! 1. a body firmly holding the point -- a stock, module or cloud box, an aux,
//!    alias or valve circle, at least `FIRM_INSET` in from its edge -- lands on
//!    that element, the topmost such, unless something drawn above it takes the
//!    point first by the tiers below;
//! 2. otherwise an end handle within reach -- a flow's source end or arrowhead,
//!    a link's arrowhead -- lands on that end, the nearest (the topmost on a
//!    tie);
//! 3. otherwise a label firmly holding the point lands on that label, the
//!    topmost such;
//! 4. otherwise the nearest drawing within `tolerance` wins, the topmost on a
//!    tie: a body, a pipe, a line, a label, a group's outline.
//!
//! An end handle outranks a label because an end is small and bound to an edge
//! while a label is large: where the two overlap, the label keeps the rest of
//! its box and the end would otherwise have nothing. A body outranks the handles
//! beneath it because the edge is where those handles live, and a point firmly
//! inside is not on the edge.
//!
//! The tolerance is in model units, so the host scales its screen slop by the
//! zoom. What is drawn is read from the diagram's geometry functions and
//! `resolve_view`'s draw order -- the ones the scene is built from -- so a hit
//! lands where the host drew.

use crate::datamodel;
use crate::diagram::common::{Circle, Frame, Point as DiagramPoint, Rect};
use crate::diagram::connector::{
    ARC_POLYLINE_SAMPLES, ConnectorGeometry, connector_geometry, connector_polyline,
};
use crate::diagram::elements::{
    alias_geometry, aux_geometry, cloud_bounds, group_geometry, module_geometry, stock_geometry,
};
use crate::diagram::flow::flow_geometry;
use crate::diagram::label::{LabelProps, label_bounds};
use crate::diagram::resolve::{ResolvedElement, resolve_view};

use super::geometry::Point;

/// Which part of an element a hit lands on.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum HitPart {
    /// The element itself: a shape, a flow's pipe or valve, a link's line.
    Body,
    /// A flow's sink end, or a link's arrowhead.
    Arrowhead,
    /// A flow's source end.
    Source,
    /// The element's name label.
    Label,
}

impl HitPart {
    pub const ALL: [HitPart; 4] = [
        HitPart::Body,
        HitPart::Arrowhead,
        HitPart::Source,
        HitPart::Label,
    ];
}

#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq)]
pub struct Hit {
    pub uid: i32,
    pub part: HitPart,
}

/// Half the drawn width of a flow's pipe and of a link's line: a point within
/// it is on the drawing.
const PIPE_HALF_WIDTH: f64 = 2.0;
const LINK_HALF_WIDTH: f64 = 1.0;

/// The smallest radius of an end handle. A handle grows with the tolerance (a
/// finger needs more than a pointer).
const MIN_END_HANDLE: f64 = 8.0;

/// How far inside a body or a label a point must be for it to hold the point
/// firmly. On a body's edge live the handles of what attaches to it.
const FIRM_INSET: f64 = 2.0;

/// The element and part of `model_name`'s first stock-and-flow view that
/// `point` lands on, `None` when nothing drawn is within `tolerance`. Fails
/// where the scene fails: a missing model or a model with no view.
pub fn hit_test(
    project: &datamodel::Project,
    model_name: &str,
    point: Point,
    tolerance: f64,
) -> Result<Option<Hit>, String> {
    let view = resolve_view(project, model_name)?;
    if !point.is_finite() {
        return Ok(None);
    }
    let tolerance = if tolerance.is_finite() {
        tolerance.max(0.0)
    } else {
        0.0
    };
    let handle_radius = MIN_END_HANDLE.max(tolerance / 2.0);
    let is_arrayed = |name: &str| view.is_arrayed(name);
    let p = DiagramPoint {
        x: point.x,
        y: point.y,
    };
    let mut handle: Option<(Hit, f64)> = None;
    let mut label: Option<Hit> = None;
    let mut nearest: Option<(Hit, f64)> = None;
    // Topmost first: the scene draws `view.elements` in order. Strictly nearer
    // replaces, so on a tie the topmost, visited first, keeps the hit.
    for element in view.elements.iter().rev() {
        let Some(h) = element_hit(element, p, handle_radius, &is_arrayed) else {
            continue;
        };
        // Checked before the element's own handle is recorded: a finger firmly
        // on a valve slides the valve even where the flow's end is within reach.
        if h.firm_body {
            let body = Hit {
                uid: h.uid,
                part: HitPart::Body,
            };
            return Ok(Some(handle.map(|(hit, _)| hit).or(label).unwrap_or(body)));
        }
        if let Some((part, d)) = h.handle
            && handle.is_none_or(|(_, best)| d < best)
        {
            handle = Some((Hit { uid: h.uid, part }, d));
        }
        if h.firm_label && label.is_none() {
            label = Some(Hit {
                uid: h.uid,
                part: HitPart::Label,
            });
        }
        if let Some((part, d)) = h.nearest
            && d <= tolerance
            && nearest.is_none_or(|(_, best)| d < best)
        {
            nearest = Some((Hit { uid: h.uid, part }, d));
        }
    }
    Ok(handle
        .map(|(hit, _)| hit)
        .or(label)
        .or(nearest.map(|(hit, _)| hit)))
}

/// What one element offers a point: whether a body or the label holds it
/// firmly, the end handle within reach, and the nearest part of its drawing.
struct ElementHit {
    uid: i32,
    firm_body: bool,
    firm_label: bool,
    handle: Option<(HitPart, f64)>,
    nearest: Option<(HitPart, f64)>,
}

fn element_hit(
    element: &ResolvedElement<'_>,
    p: DiagramPoint,
    handle_radius: f64,
    is_arrayed: &dyn Fn(&str) -> bool,
) -> Option<ElementHit> {
    let within = |part: HitPart, d: f64| (d <= handle_radius).then_some((part, d));
    Some(match element {
        ResolvedElement::Group(group) => ElementHit {
            uid: group.uid,
            // A group is a container: only its outline is its own, so a point
            // inside it reaches what it holds.
            firm_body: false,
            firm_label: false,
            handle: None,
            nearest: Some((
                HitPart::Body,
                frame_outline_distance(&group_geometry(group).rect, p),
            )),
        },
        ResolvedElement::Link { link, from, to } => {
            let anchor = match connector_geometry(link, from, to, is_arrayed) {
                ConnectorGeometry::Straight(g) => g.end,
                ConnectorGeometry::Arc(g) => g.end,
                ConnectorGeometry::Undrawable => return None,
            };
            let polyline = connector_polyline(link, from, to, is_arrayed, ARC_POLYLINE_SAMPLES);
            ElementHit {
                uid: link.uid,
                firm_body: false,
                firm_label: false,
                handle: within(HitPart::Arrowhead, distance(p, anchor)),
                nearest: Some((
                    HitPart::Body,
                    polyline_distance(&polyline, p, LINK_HALF_WIDTH),
                )),
            }
        }
        ResolvedElement::Flow {
            flow,
            sink,
            is_arrayed,
        } => {
            let g = flow_geometry(flow, sink, *is_arrayed)?;
            let valve = g
                .valves
                .iter()
                .map(|c| circle_distance(c, p))
                .fold(f64::INFINITY, f64::min);
            let pipe = polyline_distance(&g.pipe, p, PIPE_HALF_WIDTH);
            let source = flow.points.first().map_or(f64::INFINITY, |s| {
                distance(p, DiagramPoint { x: s.x, y: s.y })
            });
            let sink_end = distance(p, g.arrowhead.tip);
            let handle = if sink_end <= source {
                within(HitPart::Arrowhead, sink_end)
            } else {
                within(HitPart::Source, source)
            };
            let firm_valve = g.valves.iter().any(|c| firm_in_circle(c, p));
            labeled(flow.uid, firm_valve, handle, valve.min(pipe), &g.label, p)
        }
        ResolvedElement::Stock { stock, is_arrayed } => {
            let g = stock_geometry(stock, *is_arrayed);
            let body = g
                .rects
                .iter()
                .map(|r| frame_distance(r, p))
                .fold(f64::INFINITY, f64::min);
            let firm = g.rects.iter().any(|r| firm_in_frame(r, p));
            labeled(stock.uid, firm, None, body, &g.label, p)
        }
        ResolvedElement::Cloud(cloud) => {
            let r = cloud_bounds(cloud);
            ElementHit {
                uid: cloud.uid,
                firm_body: firm_in_rect(&r, p),
                firm_label: false,
                handle: None,
                nearest: Some((HitPart::Body, rect_distance(&r, p))),
            }
        }
        ResolvedElement::Module(module) => {
            let g = module_geometry(module);
            labeled(
                module.uid,
                firm_in_frame(&g.rect, p),
                None,
                frame_distance(&g.rect, p),
                &g.label,
                p,
            )
        }
        ResolvedElement::Aux { aux, is_arrayed } => {
            let g = aux_geometry(aux, *is_arrayed);
            let body = g
                .circles
                .iter()
                .map(|c| circle_distance(c, p))
                .fold(f64::INFINITY, f64::min);
            let firm = g.circles.iter().any(|c| firm_in_circle(c, p));
            labeled(aux.uid, firm, None, body, &g.label, p)
        }
        ResolvedElement::Alias {
            alias,
            alias_of_name,
        } => {
            let g = alias_geometry(alias, *alias_of_name);
            labeled(
                alias.uid,
                firm_in_circle(&g.circle, p),
                None,
                circle_distance(&g.circle, p),
                &g.label,
                p,
            )
        }
    })
}

/// An element with a body at distance `body` and a label (drawn outside the
/// body, on top of it): the label is the nearest part where it is nearer.
fn labeled(
    uid: i32,
    firm_body: bool,
    handle: Option<(HitPart, f64)>,
    body: f64,
    label: &LabelProps,
    p: DiagramPoint,
) -> ElementHit {
    let bounds = label_bounds(label);
    let to_label = rect_distance(&bounds, p);
    ElementHit {
        uid,
        firm_body,
        firm_label: firm_in_rect(&bounds, p),
        handle,
        nearest: Some(if to_label < body {
            (HitPart::Label, to_label)
        } else {
            (HitPart::Body, body)
        }),
    }
}

fn distance(a: DiagramPoint, b: DiagramPoint) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}

fn rect_distance(r: &Rect, p: DiagramPoint) -> f64 {
    let dx = (r.left - p.x).max(0.0).max(p.x - r.right);
    let dy = (r.top - p.y).max(0.0).max(p.y - r.bottom);
    dx.hypot(dy)
}

fn frame_rect(f: &Frame) -> Rect {
    Rect {
        left: f.x,
        top: f.y,
        right: f.x + f.width,
        bottom: f.y + f.height,
    }
}

fn frame_distance(f: &Frame, p: DiagramPoint) -> f64 {
    rect_distance(&frame_rect(f), p)
}

fn firm_in_rect(r: &Rect, p: DiagramPoint) -> bool {
    p.x - r.left >= FIRM_INSET
        && r.right - p.x >= FIRM_INSET
        && p.y - r.top >= FIRM_INSET
        && r.bottom - p.y >= FIRM_INSET
}

fn firm_in_frame(f: &Frame, p: DiagramPoint) -> bool {
    firm_in_rect(&frame_rect(f), p)
}

fn frame_outline_distance(f: &Frame, p: DiagramPoint) -> f64 {
    let r = frame_rect(f);
    if p.x > r.left && p.x < r.right && p.y > r.top && p.y < r.bottom {
        (p.x - r.left)
            .min(r.right - p.x)
            .min(p.y - r.top)
            .min(r.bottom - p.y)
    } else {
        rect_distance(&r, p)
    }
}

fn circle_distance(c: &Circle, p: DiagramPoint) -> f64 {
    ((p.x - c.x).hypot(p.y - c.y) - c.r).max(0.0)
}

fn firm_in_circle(c: &Circle, p: DiagramPoint) -> bool {
    c.r - (p.x - c.x).hypot(p.y - c.y) >= FIRM_INSET
}

fn polyline_distance(points: &[DiagramPoint], p: DiagramPoint, half_width: f64) -> f64 {
    points
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let l2 = dx * dx + dy * dy;
            let t = if l2 == 0.0 {
                0.0
            } else {
                (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
            };
            ((p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy)) - half_width).max(0.0)
        })
        .fold(f64::INFINITY, f64::min)
}

#[cfg(test)]
#[path = "hit_tests.rs"]
mod tests;
