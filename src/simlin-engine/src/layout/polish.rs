// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Crossing polish on drawn geometry. The force pass reduces crossings with
// annealing over straight chords between point nodes, and what it leaves is
// what the final diagram shows: the declutter pass moves free nodes only as
// far as overlaps demand, so it neither adds nor removes crossings. Hand-drawn
// diagrams in the corpus almost never cross links, while generated ones cross
// on a tenth to two fifths of their connectors, typically a parameter drawn on
// the far side of a chain or cluster from its consumer.
//
// This pass visits each free node (auxiliary, module, ghost) that sits on a
// crossing and tries it at a ring of spots around its neighbors. A spot is
// charged what the metric charges for the node locally -- its own connectors'
// crossings, and the names struck by its connectors or through its own name --
// and must keep the node's shape and label clear of every other shape, label,
// and pipe. A node's move changes only those charges, so keeping a spot only
// where they fall never raises the metric's crossing and strike terms.

use super::*;
use crate::diagram::common::{
    Point, Rect as Bounds, rect_overlap_area, segment_clip_interval_in_rect,
};
use crate::diagram::label::label_bounds;
use crate::layout::metrics::{
    COMFORTABLE_CLEARANCE, LABEL_INSET, MetricWeights, OWN_LINK_STRIKE_FACTOR,
    alias_label_props_for, alias_source_names, element_label_props_for, node_shape_box, pipe_rects,
};

/// Passes over the free nodes; each pass that moves nothing ends the polish.
const POLISH_ROUNDS: usize = 3;

/// Spots tried on the ring around a node's neighbors.
const RING_SPOTS: usize = 16;

/// The ring's radius is the node's current distance to its neighbors'
/// centroid, held within these bounds so a node drawn on top of its neighbors
/// still gets room and a far-flung one is brought in.
const MIN_RING_RADIUS: f64 = 60.0;
const MAX_RING_RADIUS: f64 = 180.0;

/// Move free nodes off crossings where a nearby spot uncrosses their
/// connectors (see the module docs). Deterministic: nodes are visited in uid
/// order and ties keep the earliest spot, starting from where the node is.
pub(crate) fn polish_crossings(elements: &mut [ViewElement]) {
    polish_crossings_for(elements, |_| true);
}

/// [`polish_crossings`] moving only the free nodes whose uid `moves` accepts;
/// everything else is fixed, as the incremental layout needs for what was
/// already drawn.
pub(crate) fn polish_crossings_for(elements: &mut [ViewElement], moves: impl Fn(i32) -> bool) {
    let free: Vec<usize> = {
        let mut free: Vec<(i32, usize)> = elements
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                matches!(
                    e,
                    ViewElement::Aux(_) | ViewElement::Module(_) | ViewElement::Alias(_)
                ) && moves(e.get_uid())
            })
            .map(|(i, e)| (e.get_uid(), i))
            .collect();
        free.sort_unstable();
        free.into_iter().map(|(_, i)| i).collect()
    };
    let links: Vec<(usize, i32, i32)> = elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            ViewElement::Link(l) => Some((i, l.from_uid, l.to_uid)),
            _ => None,
        })
        .collect();

    let mut scene = PolishScene::new(elements);
    for _ in 0..POLISH_ROUNDS {
        let mut moved = false;
        for &node in &free {
            moved |= polish_node(elements, &mut scene, node, &links);
        }
        if !moved {
            break;
        }
    }
}

/// What the polish charges spots against, by element index: each connector's
/// segments, each element's shape and pipe boxes, and its label box. Only the
/// entries of a node that moved and of its connectors change, so they are
/// refreshed after each move rather than rebuilt per spot.
struct PolishScene {
    segments: Vec<Vec<LineSegment>>,
    shapes: Vec<Vec<Bounds>>,
    labels: Vec<Option<Bounds>>,
    alias_names: HashMap<i32, String>,
    connector_count: usize,
    label_count: usize,
}

impl PolishScene {
    fn new(elements: &[ViewElement]) -> Self {
        let alias_names = alias_source_names(elements);
        let uid_elements: HashMap<i32, &ViewElement> =
            elements.iter().map(|e| (e.get_uid(), e)).collect();
        let segments: Vec<Vec<LineSegment>> = elements
            .iter()
            .map(|e| element_segments(e, &uid_elements))
            .collect();
        let shapes = elements.iter().map(shape_boxes).collect();
        let labels: Vec<Option<Bounds>> = elements
            .iter()
            .map(|e| label_box(e, &alias_names))
            .collect();
        PolishScene {
            connector_count: segments.iter().filter(|s| !s.is_empty()).count().max(1),
            label_count: labels.iter().flatten().count().max(1),
            segments,
            shapes,
            labels,
            alias_names,
        }
    }

    /// Recompute the entries of `node` and of the connectors in `connectors`.
    fn refresh(&mut self, elements: &[ViewElement], node: usize, connectors: &[usize]) {
        let uid_elements: HashMap<i32, &ViewElement> =
            elements.iter().map(|e| (e.get_uid(), e)).collect();
        for &i in connectors {
            self.segments[i] = element_segments(&elements[i], &uid_elements);
        }
        self.shapes[node] = shape_boxes(&elements[node]);
        self.labels[node] = label_box(&elements[node], &self.alias_names);
    }
}

fn shape_boxes(e: &ViewElement) -> Vec<Bounds> {
    let mut rects: Vec<Bounds> = node_shape_box(e).into_iter().collect();
    if let ViewElement::Flow(f) = e {
        rects.extend(pipe_rects(f));
    }
    rects
}

/// Try `elements[node]` at the spots around its neighbors; returns whether it
/// moved.
///
/// A spot is charged what the metric charges the node's move locally: the
/// crossings of its own connectors, and the names struck -- by its connectors
/// through other labels and by any connector through its own -- each in the
/// metric's per-connector and per-label units. The node's shape and label must
/// land at least the crowding clearance from every other shape, label, and
/// pipe. A move is kept only where the charge falls.
fn polish_node(
    elements: &mut [ViewElement],
    scene: &mut PolishScene,
    node: usize,
    links: &[(usize, i32, i32)],
) -> bool {
    let uid = elements[node].get_uid();
    // (connector index, the link's other endpoint)
    let incident: Vec<(usize, i32)> = links
        .iter()
        .filter_map(|&(i, from, to)| {
            if from == uid {
                Some((i, to))
            } else if to == uid {
                Some((i, from))
            } else {
                None
            }
        })
        .collect();
    if incident.is_empty() {
        return false;
    }
    let is_incident = |i: usize| incident.iter().any(|&(j, _)| j == i);
    let others = || {
        scene
            .segments
            .iter()
            .enumerate()
            .filter(move |(i, _)| !is_incident(*i))
            .flat_map(|(_, segs)| segs.iter())
    };

    // Cheap gate: a node none of whose connectors cross anything stays put.
    let crossing_now = incident.iter().any(|&(i, _)| {
        scene.segments[i]
            .iter()
            .any(|seg| others().any(|other| annealing::do_segments_intersect(seg, other)))
    });
    if !crossing_now {
        return false;
    }

    let weights = MetricWeights::default();
    let Some(start) = element_center(&elements[node]) else {
        return false;
    };
    let neighbors: Vec<Position> = incident
        .iter()
        .filter_map(|&(_, other)| {
            elements
                .iter()
                .find(|e| e.get_uid() == other)
                .and_then(element_center)
        })
        .collect();
    if neighbors.is_empty() {
        return false;
    }

    // The node's local charge with it at `p`, and its connectors' length.
    let charge = |elements: &[ViewElement], moved: &ViewElement| -> (f64, f64) {
        let mut uid_elements: HashMap<i32, &ViewElement> = incident
            .iter()
            .filter_map(|&(_, other)| elements.iter().find(|e| e.get_uid() == other))
            .map(|e| (e.get_uid(), e))
            .collect();
        uid_elements.insert(uid, moved);
        let mut crossings = 0usize;
        let mut length = 0.0;
        let mut own: Vec<(LineSegment, i32)> = Vec::new();
        for &(i, other_end) in &incident {
            for seg in element_segments(&elements[i], &uid_elements) {
                length += (seg.end - seg.start).length();
                crossings += others()
                    .filter(|other| annealing::do_segments_intersect(&seg, other))
                    .count();
                own.push((seg, other_end));
            }
        }
        let mut struck = 0.0;
        for (j, label) in scene.labels.iter().enumerate() {
            let Some(label) = label else { continue };
            if j == node {
                continue;
            }
            let owner = elements[j].get_uid();
            let through: f64 = own
                .iter()
                .map(|(seg, other_end)| {
                    let factor = if *other_end == owner {
                        OWN_LINK_STRIKE_FACTOR
                    } else {
                        1.0
                    };
                    factor * run_inside(seg, label)
                })
                .sum();
            struck += strike_fraction(through, label);
        }
        if let Some(label) = label_box(moved, &scene.alias_names) {
            let through: f64 = others().map(|seg| run_inside(seg, &label)).sum::<f64>()
                + own
                    .iter()
                    .map(|(seg, _)| OWN_LINK_STRIKE_FACTOR * run_inside(seg, &label))
                    .sum::<f64>();
            struck += strike_fraction(through, &label);
        }
        let cost = weights.crossings * crossings as f64 / scene.connector_count as f64
            + weights.label_connector_overlap * struck / scene.label_count as f64;
        (cost, length)
    };

    let (now_cost, now_length) = charge(elements, &elements[node]);

    let n = neighbors.len() as f64;
    let centroid = Position::new(
        neighbors.iter().map(|p| p.x).sum::<f64>() / n,
        neighbors.iter().map(|p| p.y).sum::<f64>() / n,
    );
    let radius = (start - centroid)
        .length()
        .clamp(MIN_RING_RADIUS, MAX_RING_RADIUS);
    let mut best: Option<(f64, f64, Position)> = None;
    let mut candidate = elements[node].clone();
    for k in 0..RING_SPOTS {
        let angle = k as f64 * 2.0 * PI / RING_SPOTS as f64;
        let spot = Position::new(
            centroid.x + radius * angle.cos(),
            centroid.y + radius * angle.sin(),
        );
        set_center(&mut candidate, spot);
        let footprint: Vec<Bounds> = node_shape_box(&candidate)
            .into_iter()
            .chain(label_box(&candidate, &scene.alias_names))
            .map(|r| grown(&r, COMFORTABLE_CLEARANCE))
            .collect();
        let blocked = scene
            .shapes
            .iter()
            .zip(&scene.labels)
            .enumerate()
            .filter(|(j, _)| *j != node)
            .flat_map(|(_, (shapes, label))| shapes.iter().chain(label.iter()))
            .any(|obstacle| {
                footprint
                    .iter()
                    .any(|clear| rect_overlap_area(clear, obstacle) > 0.0)
            });
        if blocked {
            continue;
        }
        let (cost, length) = charge(elements, &candidate);
        let (best_cost, best_length) = best.map_or((now_cost, now_length), |(c, l, _)| (c, l));
        if cost < best_cost - 1e-12 || (cost <= best_cost + 1e-12 && length < best_length - 1e-9) {
            best = Some((cost, length, spot));
        }
    }
    match best {
        Some((cost, _, spot)) if cost < now_cost - 1e-12 => {
            set_center(&mut elements[node], spot);
            let connectors: Vec<usize> = incident.iter().map(|&(i, _)| i).collect();
            scene.refresh(elements, node, &connectors);
            true
        }
        _ => false,
    }
}

/// The metric's strike fraction for `through` px of line in `label`'s text.
fn strike_fraction(through: f64, label: &Bounds) -> f64 {
    let side = (label.right - label.left).min(label.bottom - label.top) - 2.0 * LABEL_INSET;
    if side <= 0.0 {
        return 0.0;
    }
    (through / side).min(1.0)
}

/// How far a segment runs inside `label`'s (inset) text box.
fn run_inside(seg: &LineSegment, label: &Bounds) -> f64 {
    let text = Bounds {
        left: label.left + LABEL_INSET,
        top: label.top + LABEL_INSET,
        right: label.right - LABEL_INSET,
        bottom: label.bottom - LABEL_INSET,
    };
    let (p0, p1) = (
        Point {
            x: seg.start.x,
            y: seg.start.y,
        },
        Point {
            x: seg.end.x,
            y: seg.end.y,
        },
    );
    segment_clip_interval_in_rect(&p0, &p1, &text)
        .map_or(0.0, |(t0, t1)| (t1 - t0) * (seg.end - seg.start).length())
}

/// The label box an element draws at its current side.
fn label_box(e: &ViewElement, alias_names: &HashMap<i32, String>) -> Option<Bounds> {
    if let ViewElement::Alias(a) = e {
        let name = alias_names.get(&a.uid)?;
        return Some(label_bounds(&alias_label_props_for(a, name, a.label_side)));
    }
    let side = match e {
        ViewElement::Aux(a) => a.label_side,
        ViewElement::Stock(s) => s.label_side,
        ViewElement::Flow(f) => f.label_side,
        ViewElement::Module(m) => m.label_side,
        _ => return None,
    };
    element_label_props_for(e, side).map(|props| label_bounds(&props))
}

fn element_center(e: &ViewElement) -> Option<Position> {
    match e {
        ViewElement::Aux(a) => Some(Position::new(a.x, a.y)),
        ViewElement::Module(m) => Some(Position::new(m.x, m.y)),
        ViewElement::Alias(a) => Some(Position::new(a.x, a.y)),
        ViewElement::Stock(s) => Some(Position::new(s.x, s.y)),
        ViewElement::Flow(f) => Some(Position::new(f.x, f.y)),
        ViewElement::Cloud(c) => Some(Position::new(c.x, c.y)),
        ViewElement::Link(_) | ViewElement::Group(_) => None,
    }
}

/// Place a free node's center at `p`.
fn set_center(e: &mut ViewElement, p: Position) {
    match e {
        ViewElement::Aux(a) => {
            a.x = p.x;
            a.y = p.y;
        }
        ViewElement::Module(m) => {
            m.x = p.x;
            m.y = p.y;
        }
        ViewElement::Alias(a) => {
            a.x = p.x;
            a.y = p.y;
        }
        _ => {}
    }
}

fn grown(r: &Bounds, d: f64) -> Bounds {
    Bounds {
        left: r.left - d,
        top: r.top - d,
        right: r.right + d,
        bottom: r.bottom + d,
    }
}

#[cfg(test)]
#[path = "polish_tests.rs"]
mod tests;
