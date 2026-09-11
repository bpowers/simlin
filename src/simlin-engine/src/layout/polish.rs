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
// crossing and tries it at a ring of spots around its neighbors, keeping a
// spot only where its own connectors cross strictly less and its shape lands
// clear of every other shape and pipe. A node's move changes only the
// crossings of the connectors it owns, so each candidate is charged by those
// alone; the diagram's crossing count therefore never rises.

use super::*;
use crate::diagram::common::{Rect as Bounds, rect_overlap_area};
use crate::layout::metrics::{COMFORTABLE_CLEARANCE, node_shape_box, pipe_rects};

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
    let free: Vec<usize> = {
        let mut free: Vec<(i32, usize)> = elements
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                matches!(
                    e,
                    ViewElement::Aux(_) | ViewElement::Module(_) | ViewElement::Alias(_)
                )
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

    for _ in 0..POLISH_ROUNDS {
        let mut moved = false;
        for &node in &free {
            moved |= polish_node(elements, node, &links);
        }
        if !moved {
            break;
        }
    }
}

/// Try `elements[node]` at the spots around its neighbors; returns whether it
/// moved.
fn polish_node(elements: &mut [ViewElement], node: usize, links: &[(usize, i32, i32)]) -> bool {
    let uid = elements[node].get_uid();
    let incident: Vec<usize> = links
        .iter()
        .filter(|(_, from, to)| *from == uid || *to == uid)
        .map(|(i, _, _)| *i)
        .collect();
    if incident.is_empty() {
        return false;
    }

    let (others, obstacles) = {
        let uid_elements: HashMap<i32, &ViewElement> =
            elements.iter().map(|e| (e.get_uid(), e)).collect();
        let others: Vec<LineSegment> = elements
            .iter()
            .enumerate()
            .filter(|(i, _)| !incident.contains(i))
            .flat_map(|(_, e)| element_segments(e, &uid_elements))
            .collect();
        let obstacles: Vec<Bounds> = elements
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != node)
            .flat_map(|(_, e)| {
                let mut rects: Vec<Bounds> = node_shape_box(e).into_iter().collect();
                if let ViewElement::Flow(f) = e {
                    rects.extend(pipe_rects(f));
                }
                rects
            })
            .collect();
        (others, obstacles)
    };

    let neighbors: Vec<Position> = {
        let positions: HashMap<i32, Position> = elements
            .iter()
            .filter_map(|e| element_center(e).map(|p| (e.get_uid(), p)))
            .collect();
        incident
            .iter()
            .filter_map(|&i| match &elements[i] {
                ViewElement::Link(l) => {
                    let other = if l.from_uid == uid {
                        l.to_uid
                    } else {
                        l.from_uid
                    };
                    positions.get(&other).copied()
                }
                _ => None,
            })
            .collect()
    };
    let Some(start) = element_center(&elements[node]) else {
        return false;
    };
    if neighbors.is_empty() {
        return false;
    }

    // What the node's own connectors cross, and how long they are, with the
    // node at `p`.
    let charge = |elements: &mut [ViewElement], p: Position| -> (usize, f64) {
        set_center(&mut elements[node], p);
        let uid_elements: HashMap<i32, &ViewElement> =
            elements.iter().map(|e| (e.get_uid(), e)).collect();
        let mut crossings = 0;
        let mut length = 0.0;
        for &i in &incident {
            for seg in element_segments(&elements[i], &uid_elements) {
                length += (seg.end - seg.start).length();
                crossings += others
                    .iter()
                    .filter(|other| annealing::do_segments_intersect(&seg, other))
                    .count();
            }
        }
        (crossings, length)
    };

    let (now_crossings, now_length) = charge(elements, start);
    if now_crossings == 0 {
        return false;
    }

    let n = neighbors.len() as f64;
    let centroid = Position::new(
        neighbors.iter().map(|p| p.x).sum::<f64>() / n,
        neighbors.iter().map(|p| p.y).sum::<f64>() / n,
    );
    let radius = (start - centroid)
        .length()
        .clamp(MIN_RING_RADIUS, MAX_RING_RADIUS);
    let mut best = (now_crossings, now_length, start);
    for k in 0..RING_SPOTS {
        let angle = k as f64 * 2.0 * PI / RING_SPOTS as f64;
        let spot = Position::new(
            centroid.x + radius * angle.cos(),
            centroid.y + radius * angle.sin(),
        );
        set_center(&mut elements[node], spot);
        let blocked = node_shape_box(&elements[node]).is_some_and(|shape| {
            let clear = grown(&shape, COMFORTABLE_CLEARANCE);
            obstacles
                .iter()
                .any(|obstacle| rect_overlap_area(&clear, obstacle) > 0.0)
        });
        if blocked {
            continue;
        }
        let (crossings, length) = charge(elements, spot);
        if (crossings, length) < (best.0, best.1) {
            best = (crossings, length, spot);
        }
    }
    let moved = best.0 < now_crossings;
    set_center(&mut elements[node], if moved { best.2 } else { start });
    moved
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
