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
//!
//! The tiers read a view through a [`HitIndex`]: every drawn element's shapes,
//! end handles and label box, computed once, and a grid of the boxes each
//! element reaches over. The tiers walk the elements from the topmost down, and
//! an element that offers a point nothing changes nothing in that walk, so
//! walking only the elements with a box within reach of the point decides
//! exactly what walking every element decides (`hit_prune_tests`). A hit then
//! costs in proportion to what is near the point, not to the size of the view.
//! A host asking at display rate keeps an index for as long as the view stands;
//! [`hit_test`] builds one for a single call.

use rustc_hash::FxHashMap;
use smallvec::SmallVec;

use crate::datamodel;
use crate::diagram::ScenePaint;
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
use crate::diagram::scene::{
    arrowhead_path, circle_shape, cloud_path, element_label, link_line_path, path_shape,
    polyline_path, rect_shape,
};

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

/// The rule of the tiers that decided a hit.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Rule {
    /// A body firmly holding the point, with nothing drawn above it taking it.
    FirmBody,
    /// An end handle within reach, drawn above a body firmly holding the point.
    HandleAboveFirmBody,
    /// A label firmly holding the point, drawn above a body firmly holding it.
    LabelAboveFirmBody,
    /// An end handle within reach, the nearest.
    Handle,
    /// A label firmly holding the point, the topmost.
    Label,
    /// The nearest drawing within the tolerance.
    Nearest,
}

#[cfg(test)]
impl Rule {
    pub(crate) const ALL: [Rule; 6] = [
        Rule::FirmBody,
        Rule::HandleAboveFirmBody,
        Rule::LabelAboveFirmBody,
        Rule::Handle,
        Rule::Label,
        Rule::Nearest,
    ];
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

/// The side of a grid cell, in model units: about an element's size, so a
/// pointer's reach at ordinary zooms visits a few cells.
const CELL: f64 = 64.0;

/// A box spanning more cells than this is a candidate everywhere instead of
/// being listed cell by cell, as the edge of a group framing a huge view would
/// otherwise be.
const MAX_BOX_CELLS: f64 = 1024.0;

/// A reach visiting more cells than this walks every element instead: a reach
/// that wide (a finger far zoomed out) sees much of the view anyway.
const MAX_QUERY_CELLS: f64 = 256.0;

/// A coordinate beyond this magnitude is not placed in the grid, so every cell
/// index fits an `i32`; a box holding one is a candidate everywhere.
const MAX_PLACED: f64 = 1e9;

/// The element and part of `model_name`'s first stock-and-flow view that
/// `point` lands on, `None` when nothing drawn is within `tolerance`. Fails
/// where the scene fails: a missing model or a model with no view. Builds an
/// index for the one call; a caller asking again about an unchanged view keeps
/// a [`HitIndex`] instead.
pub fn hit_test(
    project: &datamodel::Project,
    model_name: &str,
    point: Point,
    tolerance: f64,
) -> Result<Option<Hit>, String> {
    Ok(HitIndex::new(project, model_name)?.hit(point, tolerance))
}

/// A view's drawn elements indexed for hit testing: what the tiers read about
/// each, in draw order, and where each reaches.
pub struct HitIndex {
    /// Every element a point can land on, in draw order. An element the scene
    /// draws nothing for (an undrawable link) offers nothing and has no entry.
    entries: Vec<Entry>,
    grid: Grid,
}

impl HitIndex {
    /// Index `model_name`'s first stock-and-flow view. Fails where the scene
    /// fails: a missing model or a model with no view.
    pub fn new(project: &datamodel::Project, model_name: &str) -> Result<HitIndex, String> {
        let view = resolve_view(project, model_name)?;
        let is_arrayed = |name: &str| view.is_arrayed(name);
        let entries: Vec<Entry> = view
            .elements
            .iter()
            .filter_map(|element| Entry::new(element, &is_arrayed))
            .collect();
        let grid = Grid::new(&entries);
        Ok(HitIndex { entries, grid })
    }

    /// The element and part `point` lands on, `None` when nothing drawn is
    /// within `tolerance` model units. A negative or non-finite tolerance is 0,
    /// and a non-finite point lands on nothing.
    pub fn hit(&self, point: Point, tolerance: f64) -> Option<Hit> {
        self.decide(point, tolerance).map(|(hit, _)| hit)
    }

    /// The hit, and the rule that decided it, walking the elements within reach.
    pub(crate) fn decide(&self, point: Point, tolerance: f64) -> Option<(Hit, Rule)> {
        let reach = Reach::of(point, tolerance)?;
        match self.grid.candidates(reach.point, reach.radius()) {
            Candidates::Listed(listed) => walk(
                listed.iter().rev().map(|&i| &self.entries[i as usize]),
                &reach,
            ),
            Candidates::All => walk(self.entries.iter().rev(), &reach),
        }
    }

    /// The hit, and the rule that decided it, walking every element: what
    /// [`HitIndex::decide`] must return.
    #[cfg(test)]
    pub(crate) fn decide_over_every_element(
        &self,
        point: Point,
        tolerance: f64,
    ) -> Option<(Hit, Rule)> {
        let reach = Reach::of(point, tolerance)?;
        walk(self.entries.iter().rev(), &reach)
    }
}

/// A hit's point, and how far its tiers reach from it.
struct Reach {
    point: DiagramPoint,
    tolerance: f64,
    handle_radius: f64,
}

impl Reach {
    /// `None` for a non-finite point, which lands on nothing.
    fn of(point: Point, tolerance: f64) -> Option<Reach> {
        if !point.is_finite() {
            return None;
        }
        let tolerance = if tolerance.is_finite() {
            tolerance.max(0.0)
        } else {
            0.0
        };
        Some(Reach {
            point: DiagramPoint {
                x: point.x,
                y: point.y,
            },
            tolerance,
            handle_radius: MIN_END_HANDLE.max(tolerance / 2.0),
        })
    }

    /// How far from the point a part of an element can lie and still offer it
    /// something: a handle within the handle radius, a drawing within the
    /// tolerance. A body or a label holding the point is at no distance, and a
    /// line's half width is carried by its boxes.
    fn radius(&self) -> f64 {
        self.tolerance.max(self.handle_radius)
    }
}

/// Decide a hit over `topmost_first`, the elements from the topmost down.
fn walk<'a>(topmost_first: impl Iterator<Item = &'a Entry>, reach: &Reach) -> Option<(Hit, Rule)> {
    let p = reach.point;
    let mut handle: Option<(Hit, f64)> = None;
    let mut label: Option<Hit> = None;
    let mut nearest: Option<(Hit, f64)> = None;
    // Strictly nearer replaces, so on a tie the topmost, visited first, keeps
    // the hit.
    for entry in topmost_first {
        let offer = entry.shape.offer(p, reach.handle_radius);
        // Checked before the element's own handle is recorded: a finger firmly
        // on a valve slides the valve even where the flow's end is within reach.
        if offer.firm_body {
            return Some(match (handle, label) {
                (Some((hit, _)), _) => (hit, Rule::HandleAboveFirmBody),
                (None, Some(hit)) => (hit, Rule::LabelAboveFirmBody),
                (None, None) => (
                    Hit {
                        uid: entry.uid,
                        part: HitPart::Body,
                    },
                    Rule::FirmBody,
                ),
            });
        }
        if let Some((part, d)) = offer.handle
            && handle.is_none_or(|(_, best)| d < best)
        {
            handle = Some((
                Hit {
                    uid: entry.uid,
                    part,
                },
                d,
            ));
        }
        if offer.firm_label && label.is_none() {
            label = Some(Hit {
                uid: entry.uid,
                part: HitPart::Label,
            });
        }
        if let Some((part, d)) = offer.nearest
            && d <= reach.tolerance
            && nearest.is_none_or(|(_, best)| d < best)
        {
            nearest = Some((
                Hit {
                    uid: entry.uid,
                    part,
                },
                d,
            ));
        }
    }
    handle
        .map(|(hit, _)| (hit, Rule::Handle))
        .or(label.map(|hit| (hit, Rule::Label)))
        .or(nearest.map(|(hit, _)| (hit, Rule::Nearest)))
}

/// One element a point can land on.
struct Entry {
    uid: i32,
    shape: Shape,
}

/// What an element offers a point, read once from the diagram's geometry
/// functions. Each part is drawn as one scene shape -- a body as its rectangles
/// or circles, a pipe and its source end as the pipe path, a sink end or a
/// link's end as the arrowhead path, a label as the label -- and an entry holds
/// a part only where the scene draws that shape: built by the scene's own
/// builder, and kept by the predicate `scene::finish` filters with
/// (`SceneShape::is_finite`, `SceneLabel::is_finite`). A shape's paint never
/// decides whether it is kept.
enum Shape {
    /// A container: only its outline is its own, so a point inside it reaches
    /// what it holds.
    Group(Rect),
    Link {
        /// The arrowhead's anchor, where the arrowhead is drawn.
        arrowhead: Option<DiagramPoint>,
        /// Empty where the line is not drawn.
        line: Vec<DiagramPoint>,
    },
    Flow {
        valves: SmallVec<[Circle; 3]>,
        /// Empty where the pipe is not drawn, and `source` with it.
        pipe: Vec<DiagramPoint>,
        source: Option<DiagramPoint>,
        tip: Option<DiagramPoint>,
        label: Option<Rect>,
    },
    /// The rectangles, back to front.
    Stock {
        rects: SmallVec<[Rect; 3]>,
        label: Option<Rect>,
    },
    Cloud(Rect),
    Module {
        rect: Rect,
        label: Option<Rect>,
    },
    /// The circles, back to front.
    Aux {
        circles: SmallVec<[Circle; 3]>,
        label: Option<Rect>,
    },
    Alias {
        circle: Circle,
        label: Option<Rect>,
    },
}

impl Entry {
    /// `None` for an element the scene draws nothing for -- an undrawable link,
    /// or an element every shape of which the scene drops -- which offers
    /// nothing.
    fn new(element: &ResolvedElement<'_>, is_arrayed: &dyn Fn(&str) -> bool) -> Option<Entry> {
        let (uid, shape) = match element {
            ResolvedElement::Group(group) => {
                let g = group_geometry(group);
                if !rect_shape(&g.rect, g.corner_radius, ScenePaint::Group).is_finite() {
                    return None;
                }
                (group.uid, Shape::Group(frame_rect(&g.rect)))
            }
            ResolvedElement::Link { link, from, to } => {
                let geometry = connector_geometry(link, from, to, is_arrayed);
                let arrowhead = match &geometry {
                    ConnectorGeometry::Straight(g) => g.arrowhead(),
                    ConnectorGeometry::Arc(g) => g.arrowhead(),
                    ConnectorGeometry::Undrawable => return None,
                };
                // An arrowhead is anchored at its tip, the link's end.
                let arrowhead = path_shape(arrowhead_path(&arrowhead), ScenePaint::ArrowheadLink)
                    .is_finite()
                    .then_some(arrowhead.tip);
                let line =
                    if path_shape(link_line_path(&geometry), ScenePaint::Connector).is_finite() {
                        connector_polyline(link, from, to, is_arrayed, ARC_POLYLINE_SAMPLES)
                    } else {
                        Vec::new()
                    };
                if arrowhead.is_none() && line.is_empty() {
                    return None;
                }
                (link.uid, Shape::Link { arrowhead, line })
            }
            ResolvedElement::Flow {
                flow,
                sink,
                is_arrayed,
            } => {
                let g = flow_geometry(flow, sink, *is_arrayed)?;
                let valves: SmallVec<[Circle; 3]> = g
                    .valves
                    .iter()
                    .copied()
                    .filter(|c| circle_shape(c, ScenePaint::Valve).is_finite())
                    .collect();
                let tip = path_shape(arrowhead_path(&g.arrowhead), ScenePaint::ArrowheadFlow)
                    .is_finite()
                    .then_some(g.arrowhead.tip);
                let label = drawn_label(&g.label);
                let pipe =
                    if path_shape(polyline_path(&g.pipe), ScenePaint::FlowPipeOuter).is_finite() {
                        g.pipe
                    } else {
                        Vec::new()
                    };
                // The source end is the pipe's first point, drawn with the pipe.
                let source = pipe.first().copied();
                if valves.is_empty() && pipe.is_empty() && tip.is_none() {
                    return None;
                }
                let shape = Shape::Flow {
                    valves,
                    pipe,
                    source,
                    tip,
                    label,
                };
                (flow.uid, shape)
            }
            ResolvedElement::Stock { stock, is_arrayed } => {
                let g = stock_geometry(stock, *is_arrayed);
                let rects: SmallVec<[Rect; 3]> = g
                    .rects
                    .iter()
                    .filter(|r| rect_shape(r, 0.0, ScenePaint::Stock).is_finite())
                    .map(frame_rect)
                    .collect();
                if rects.is_empty() {
                    return None;
                }
                let shape = Shape::Stock {
                    rects,
                    label: drawn_label(&g.label),
                };
                (stock.uid, shape)
            }
            ResolvedElement::Cloud(cloud) => {
                if !path_shape(cloud_path(cloud), ScenePaint::Cloud).is_finite() {
                    return None;
                }
                (cloud.uid, Shape::Cloud(cloud_bounds(cloud)))
            }
            ResolvedElement::Module(module) => {
                let g = module_geometry(module);
                if !rect_shape(&g.rect, g.corner_radius, ScenePaint::Module).is_finite() {
                    return None;
                }
                let shape = Shape::Module {
                    rect: frame_rect(&g.rect),
                    label: drawn_label(&g.label),
                };
                (module.uid, shape)
            }
            ResolvedElement::Aux { aux, is_arrayed } => {
                let g = aux_geometry(aux, *is_arrayed);
                let circles: SmallVec<[Circle; 3]> = g
                    .circles
                    .iter()
                    .copied()
                    .filter(|c| circle_shape(c, ScenePaint::Aux).is_finite())
                    .collect();
                if circles.is_empty() {
                    return None;
                }
                let shape = Shape::Aux {
                    circles,
                    label: drawn_label(&g.label),
                };
                (aux.uid, shape)
            }
            ResolvedElement::Alias {
                alias,
                alias_of_name,
            } => {
                let g = alias_geometry(alias, *alias_of_name);
                if !circle_shape(&g.circle, ScenePaint::Alias).is_finite() {
                    return None;
                }
                let shape = Shape::Alias {
                    circle: g.circle,
                    label: drawn_label(&g.label),
                };
                (alias.uid, shape)
            }
        };
        Some(Entry { uid, shape })
    }
}

/// The box of a label the scene draws, `None` for one it does not.
fn drawn_label(props: &LabelProps) -> Option<Rect> {
    element_label(props)
        .is_finite()
        .then(|| label_bounds(props))
}

/// What one element offers a point: whether a body or the label holds it
/// firmly, the end handle within reach, and the nearest part of its drawing.
struct Offer {
    firm_body: bool,
    firm_label: bool,
    handle: Option<(HitPart, f64)>,
    nearest: Option<(HitPart, f64)>,
}

impl Shape {
    fn offer(&self, p: DiagramPoint, handle_radius: f64) -> Offer {
        let within = |part: HitPart, d: f64| (d <= handle_radius).then_some((part, d));
        match self {
            Shape::Group(rect) => Offer {
                firm_body: false,
                firm_label: false,
                handle: None,
                nearest: Some((HitPart::Body, outline_distance(rect, p))),
            },
            Shape::Link { arrowhead, line } => Offer {
                firm_body: false,
                firm_label: false,
                handle: arrowhead.and_then(|a| within(HitPart::Arrowhead, distance(p, a))),
                nearest: Some((HitPart::Body, polyline_distance(line, p, LINK_HALF_WIDTH))),
            },
            Shape::Flow {
                valves,
                pipe,
                source,
                tip,
                label,
            } => {
                let valve = valves
                    .iter()
                    .map(|c| circle_distance(c, p))
                    .fold(f64::INFINITY, f64::min);
                let pipe = polyline_distance(pipe, p, PIPE_HALF_WIDTH);
                let source = source.map_or(f64::INFINITY, |s| distance(p, s));
                let sink_end = tip.map_or(f64::INFINITY, |t| distance(p, t));
                let handle = if sink_end <= source {
                    within(HitPart::Arrowhead, sink_end)
                } else {
                    within(HitPart::Source, source)
                };
                let firm_valve = valves.iter().any(|c| firm_in_circle(c, p));
                labeled(firm_valve, handle, valve.min(pipe), label.as_ref(), p)
            }
            Shape::Stock { rects, label } => {
                let body = rects
                    .iter()
                    .map(|r| rect_distance(r, p))
                    .fold(f64::INFINITY, f64::min);
                let firm = rects.iter().any(|r| firm_in_rect(r, p));
                labeled(firm, None, body, label.as_ref(), p)
            }
            Shape::Cloud(rect) => Offer {
                firm_body: firm_in_rect(rect, p),
                firm_label: false,
                handle: None,
                nearest: Some((HitPart::Body, rect_distance(rect, p))),
            },
            Shape::Module { rect, label } => labeled(
                firm_in_rect(rect, p),
                None,
                rect_distance(rect, p),
                label.as_ref(),
                p,
            ),
            Shape::Aux { circles, label } => {
                let body = circles
                    .iter()
                    .map(|c| circle_distance(c, p))
                    .fold(f64::INFINITY, f64::min);
                let firm = circles.iter().any(|c| firm_in_circle(c, p));
                labeled(firm, None, body, label.as_ref(), p)
            }
            Shape::Alias { circle, label } => labeled(
                firm_in_circle(circle, p),
                None,
                circle_distance(circle, p),
                label.as_ref(),
                p,
            ),
        }
    }

    /// Every box the element reaches over. Whatever the element offers a point
    /// -- a body or label holding it, a handle within the handle radius, a
    /// drawing within the tolerance -- the point lies within the reach radius of
    /// one of these boxes on both axes, because a distance to a shape is never
    /// less than the distance to its box along either axis. A line's boxes carry
    /// its half width, which the distance to the line subtracts, and a group
    /// reaches over its outline only.
    fn for_each_reach_box(&self, mut place: impl FnMut(Rect)) {
        match self {
            Shape::Group(r) if r.left <= r.right && r.top <= r.bottom => {
                for edge in [
                    Rect {
                        bottom: r.top,
                        ..*r
                    },
                    Rect {
                        top: r.bottom,
                        ..*r
                    },
                    Rect {
                        right: r.left,
                        ..*r
                    },
                    Rect {
                        left: r.right,
                        ..*r
                    },
                ] {
                    place(edge);
                }
            }
            // A reversed outline is placed whole, which the grid cannot place,
            // so it is a candidate everywhere.
            Shape::Group(r) => place(*r),
            Shape::Link { arrowhead, line } => {
                if let Some(arrowhead) = arrowhead {
                    place(point_box(*arrowhead));
                }
                segment_boxes(line, LINK_HALF_WIDTH, &mut place);
            }
            Shape::Flow {
                valves,
                pipe,
                source,
                tip,
                label,
            } => {
                valves.iter().for_each(|c| place(circle_box(c)));
                segment_boxes(pipe, PIPE_HALF_WIDTH, &mut place);
                for end in [source, tip].into_iter().flatten() {
                    place(point_box(*end));
                }
                label.iter().for_each(|l| place(*l));
            }
            Shape::Stock { rects, label } => {
                rects.iter().for_each(|r| place(*r));
                label.iter().for_each(|l| place(*l));
            }
            Shape::Cloud(rect) => place(*rect),
            Shape::Module { rect, label } => {
                place(*rect);
                label.iter().for_each(|l| place(*l));
            }
            Shape::Aux { circles, label } => {
                circles.iter().for_each(|c| place(circle_box(c)));
                label.iter().for_each(|l| place(*l));
            }
            Shape::Alias { circle, label } => {
                place(circle_box(circle));
                label.iter().for_each(|l| place(*l));
            }
        }
    }
}

/// An element with a body at distance `body` and a label (drawn outside the
/// body, on top of it): the label is the nearest part where it is nearer. An
/// element whose label is not drawn has no label to offer.
fn labeled(
    firm_body: bool,
    handle: Option<(HitPart, f64)>,
    body: f64,
    label: Option<&Rect>,
    p: DiagramPoint,
) -> Offer {
    let to_label = label.map_or(f64::INFINITY, |l| rect_distance(l, p));
    Offer {
        firm_body,
        firm_label: label.is_some_and(|l| firm_in_rect(l, p)),
        handle,
        nearest: Some(if to_label < body {
            (HitPart::Label, to_label)
        } else {
            (HitPart::Body, body)
        }),
    }
}

/// Where elements reach: the candidates listed per grid cell, and the elements
/// with a box the grid does not place, which are candidates everywhere.
struct Grid {
    /// Entry indices by cell, ascending: placed entry by entry, in draw order.
    cells: FxHashMap<(i32, i32), SmallVec<[u32; 4]>>,
    everywhere: Vec<u32>,
}

/// The elements a point may land on.
pub(crate) enum Candidates {
    /// Entry indices, ascending (draw order) and without repeats.
    Listed(SmallVec<[u32; 32]>),
    /// Every element: the reach spans too many cells, or cannot be placed.
    All,
}

impl Grid {
    fn new(entries: &[Entry]) -> Grid {
        let mut grid = Grid {
            cells: FxHashMap::default(),
            everywhere: Vec::new(),
        };
        for (i, entry) in entries.iter().enumerate() {
            let i = i as u32;
            entry.shape.for_each_reach_box(|b| grid.place(i, b));
        }
        grid
    }

    fn place(&mut self, i: u32, b: Rect) {
        let span = cell_span(b.left, b.right).zip(cell_span(b.top, b.bottom));
        match span {
            Some(((x0, x1), (y0, y1))) if cell_count(x0, x1, y0, y1) <= MAX_BOX_CELLS => {
                for x in x0..=x1 {
                    for y in y0..=y1 {
                        let cell = self.cells.entry((x, y)).or_default();
                        // An entry's boxes are placed together, so a repeat is
                        // the cell's last listing.
                        if cell.last() != Some(&i) {
                            cell.push(i);
                        }
                    }
                }
            }
            _ => {
                if self.everywhere.last() != Some(&i) {
                    self.everywhere.push(i);
                }
            }
        }
    }

    /// The elements with a box within `radius` of `p` on both axes, or every
    /// element.
    fn candidates(&self, p: DiagramPoint, radius: f64) -> Candidates {
        // Rounding in the distances the tiers compute can put a point a hair
        // inside reach where the boxes put it a hair outside. The slack is many
        // orders of magnitude above that rounding at any coordinate, and far
        // below a model unit.
        let r = radius + 1e-6 + 1e-9 * (p.x.abs() + p.y.abs() + radius);
        let (Some((x0, x1)), Some((y0, y1))) =
            (cell_span(p.x - r, p.x + r), cell_span(p.y - r, p.y + r))
        else {
            return Candidates::All;
        };
        if cell_count(x0, x1, y0, y1) > MAX_QUERY_CELLS {
            return Candidates::All;
        }
        let mut listed: SmallVec<[u32; 32]> = SmallVec::from_slice(&self.everywhere);
        for x in x0..=x1 {
            for y in y0..=y1 {
                if let Some(cell) = self.cells.get(&(x, y)) {
                    listed.extend_from_slice(cell);
                }
            }
        }
        listed.sort_unstable();
        listed.dedup();
        Candidates::Listed(listed)
    }
}

/// The cells `[lo, hi]` spans on one axis, `None` where it cannot be placed: a
/// reversed or non-finite span, or one reaching beyond `MAX_PLACED`.
fn cell_span(lo: f64, hi: f64) -> Option<(i32, i32)> {
    let placed = -MAX_PLACED..=MAX_PLACED;
    if !(lo <= hi && placed.contains(&lo) && placed.contains(&hi)) {
        return None;
    }
    Some(((lo / CELL).floor() as i32, (hi / CELL).floor() as i32))
}

fn cell_count(x0: i32, x1: i32, y0: i32, y1: i32) -> f64 {
    f64::from(x1 - x0 + 1) * f64::from(y1 - y0 + 1)
}

/// The box of a point, as a degenerate rect.
fn point_box(p: DiagramPoint) -> Rect {
    Rect {
        left: p.x,
        top: p.y,
        right: p.x,
        bottom: p.y,
    }
}

fn circle_box(c: &Circle) -> Rect {
    Rect {
        left: c.x - c.r,
        top: c.y - c.r,
        right: c.x + c.r,
        bottom: c.y + c.r,
    }
}

/// Each segment's box, widened by `half_width`. A non-finite coordinate gives a
/// box the grid does not place: `f64::min` would silently drop it.
fn segment_boxes(points: &[DiagramPoint], half_width: f64, place: &mut impl FnMut(Rect)) {
    for w in points.windows(2) {
        let (left, right) = ordered(w[0].x, w[1].x);
        let (top, bottom) = ordered(w[0].y, w[1].y);
        place(Rect {
            left: left - half_width,
            top: top - half_width,
            right: right + half_width,
            bottom: bottom + half_width,
        });
    }
}

/// `(a, b)` in ascending order, and `(NaN, NaN)` when either is NaN.
fn ordered(a: f64, b: f64) -> (f64, f64) {
    if a <= b {
        (a, b)
    } else if b < a {
        (b, a)
    } else {
        (f64::NAN, f64::NAN)
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

fn firm_in_rect(r: &Rect, p: DiagramPoint) -> bool {
    p.x - r.left >= FIRM_INSET
        && r.right - p.x >= FIRM_INSET
        && p.y - r.top >= FIRM_INSET
        && r.bottom - p.y >= FIRM_INSET
}

fn outline_distance(r: &Rect, p: DiagramPoint) -> f64 {
    if p.x > r.left && p.x < r.right && p.y > r.top && p.y < r.bottom {
        (p.x - r.left)
            .min(r.right - p.x)
            .min(p.y - r.top)
            .min(r.bottom - p.y)
    } else {
        rect_distance(r, p)
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

#[cfg(test)]
#[path = "hit_prune_tests.rs"]
mod prune_tests;
