// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core
//
// Metamorphic taste checks for the layout-quality metric.
//
// A metric that is supposed to capture diagram taste must, at minimum, get
// WORSE when a diagram is visibly degraded. Each `Degradation` below is an edit
// every modeler would call a regression -- crowd the diagram, scatter its
// parameters, drop a node on another -- applied to a view that was fine. A
// degradation the metric does not penalize is a measured blind spot: an
// optimizer driving the metric is free to produce exactly that defect.
//
// The edits move only what a person would move by hand: free-floating nodes
// (auxiliaries, modules, aliases) are displaced individually; the stock-flow
// backbone moves only under a uniform transform, with flow endpoints re-snapped
// to the fixed-size stocks. A moved connector keeps its bow relative to its
// chord, so the edit changes WHERE nodes sit, not how curved their links are.
//
// PURE: every function takes a view and returns a new view; no I/O, and the
// seeded randomness is deterministic.

use std::collections::{HashMap, HashSet};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::datamodel::view_element::LinkShape;
use crate::datamodel::{StockFlow, ViewElement};
use crate::diagram::connector::get_visual_center;

use super::declutter::resnap_flow_endpoints_to_stocks;

/// A visibly-worse edit to a diagram. Every variant is expected to RAISE the
/// weighted cost of a view that was not already degraded in that way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Degradation {
    /// Shrink every position toward the view centroid by `factor` (< 1): labels
    /// crowd each other and connectors shorten toward invisibility.
    Cramp(f64),
    /// Spread every position away from the view centroid by `factor` (> 1): the
    /// diagram sprawls and every connector lengthens.
    Inflate(f64),
    /// Displace each free-floating node by a seeded random offset of up to
    /// `amplitude` on each axis: alignment breaks and neighbors collide.
    Jitter { amplitude: f64, seed: u64 },
    /// Permute the positions of the free-floating nodes: every node lands away
    /// from what it connects to.
    Shuffle { seed: u64 },
    /// Move the most-used parameter (the free node with the most outgoing
    /// links) far outside the diagram: its connectors cross everything.
    Exile,
    /// Drop one free node exactly onto another free node.
    Stack,
    /// Draw every curved connector straight: feedback loops read as zig-zags.
    StraightenLinks,
}

impl Degradation {
    /// A short stable name for reports.
    pub fn name(&self) -> &'static str {
        match self {
            Degradation::Cramp(_) => "cramp",
            Degradation::Inflate(_) => "inflate",
            Degradation::Jitter { .. } => "jitter",
            Degradation::Shuffle { .. } => "shuffle",
            Degradation::Exile => "exile",
            Degradation::Stack => "stack",
            Degradation::StraightenLinks => "straighten",
        }
    }

    /// The standard battery the eval harness runs, in report order.
    pub fn battery() -> [Degradation; 7] {
        [
            Degradation::Cramp(0.6),
            Degradation::Inflate(2.0),
            Degradation::Jitter {
                amplitude: 40.0,
                seed: 7,
            },
            Degradation::Shuffle { seed: 11 },
            Degradation::Exile,
            Degradation::Stack,
            Degradation::StraightenLinks,
        ]
    }
}

/// Whether an element is a free-floating node a person drags individually.
fn is_free_node(e: &ViewElement) -> bool {
    matches!(
        e,
        ViewElement::Aux(_) | ViewElement::Module(_) | ViewElement::Alias(_)
    )
}

fn position(e: &ViewElement) -> Option<(f64, f64)> {
    match e {
        ViewElement::Aux(a) => Some((a.x, a.y)),
        ViewElement::Stock(s) => Some((s.x, s.y)),
        ViewElement::Flow(f) => Some((f.x, f.y)),
        ViewElement::Module(m) => Some((m.x, m.y)),
        ViewElement::Alias(a) => Some((a.x, a.y)),
        ViewElement::Cloud(c) => Some((c.x, c.y)),
        ViewElement::Link(_) | ViewElement::Group(_) => None,
    }
}

fn set_position(e: &mut ViewElement, x: f64, y: f64) {
    match e {
        ViewElement::Aux(a) => (a.x, a.y) = (x, y),
        ViewElement::Module(m) => (m.x, m.y) = (x, y),
        ViewElement::Alias(a) => (a.x, a.y) = (x, y),
        ViewElement::Stock(s) => (s.x, s.y) = (x, y),
        ViewElement::Cloud(c) => (c.x, c.y) = (x, y),
        ViewElement::Flow(f) => {
            let (dx, dy) = (x - f.x, y - f.y);
            f.x = x;
            f.y = y;
            for p in &mut f.points {
                p.x += dx;
                p.y += dy;
            }
        }
        ViewElement::Link(_) | ViewElement::Group(_) => {}
    }
}

/// The takeoff angle an Arc link has RELATIVE to its chord (degrees), keyed by
/// link uid, so a moved connector can be redrawn with the same bow.
fn arc_offsets(view: &StockFlow) -> HashMap<i32, f64> {
    let by_uid: HashMap<i32, &ViewElement> =
        view.elements.iter().map(|e| (e.get_uid(), e)).collect();
    let not_arrayed = |_: &str| false;
    view.elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Link(link) => {
                let LinkShape::Arc(takeoff) = link.shape else {
                    return None;
                };
                let from = by_uid.get(&link.from_uid)?;
                let to = by_uid.get(&link.to_uid)?;
                let (fx, fy) = get_visual_center(from, &not_arrayed);
                let (tx, ty) = get_visual_center(to, &not_arrayed);
                let chord = (ty - fy).atan2(tx - fx).to_degrees();
                Some((link.uid, takeoff - chord))
            }
            _ => None,
        })
        .collect()
}

/// Re-apply each Arc link's pre-edit chord-relative takeoff to the post-edit
/// chord, so an edit that moves nodes does not also re-curve their links.
fn restore_arc_offsets(view: &mut StockFlow, offsets: &HashMap<i32, f64>) {
    let centers: HashMap<i32, (f64, f64)> = view
        .elements
        .iter()
        .filter(|e| !matches!(e, ViewElement::Link(_) | ViewElement::Group(_)))
        .map(|e| (e.get_uid(), get_visual_center(e, &|_: &str| false)))
        .collect();
    for e in &mut view.elements {
        let ViewElement::Link(link) = e else { continue };
        let Some(offset) = offsets.get(&link.uid) else {
            continue;
        };
        let (Some(&(fx, fy)), Some(&(tx, ty))) =
            (centers.get(&link.from_uid), centers.get(&link.to_uid))
        else {
            continue;
        };
        let chord = (ty - fy).atan2(tx - fx).to_degrees();
        link.shape = LinkShape::Arc(chord + offset);
    }
}

/// Mean position of every positioned element.
fn centroid(view: &StockFlow) -> Option<(f64, f64)> {
    let pts: Vec<(f64, f64)> = view.elements.iter().filter_map(position).collect();
    if pts.is_empty() {
        return None;
    }
    let n = pts.len() as f64;
    Some((
        pts.iter().map(|p| p.0).sum::<f64>() / n,
        pts.iter().map(|p| p.1).sum::<f64>() / n,
    ))
}

/// Scale every position (flow pipe points included) about `center` by `s`,
/// then re-snap flow endpoints to the fixed-size stocks.
fn scale_about(view: &mut StockFlow, center: (f64, f64), s: f64) {
    for e in &mut view.elements {
        match e {
            ViewElement::Flow(f) => {
                f.x = center.0 + (f.x - center.0) * s;
                f.y = center.1 + (f.y - center.1) * s;
                for p in &mut f.points {
                    p.x = center.0 + (p.x - center.0) * s;
                    p.y = center.1 + (p.y - center.1) * s;
                }
            }
            _ => {
                if let Some((x, y)) = position(e) {
                    set_position(
                        e,
                        center.0 + (x - center.0) * s,
                        center.1 + (y - center.1) * s,
                    );
                }
            }
        }
    }
    resnap_flow_endpoints_to_stocks(&mut view.elements);
}

/// Indices of free-floating nodes, in uid order (deterministic).
fn free_node_indices(view: &StockFlow) -> Vec<usize> {
    let mut idx: Vec<usize> = view
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| is_free_node(e))
        .map(|(i, _)| i)
        .collect();
    idx.sort_by_key(|&i| view.elements[i].get_uid());
    idx
}

/// Apply `degradation` to `view`. Returns `None` when the edit does not apply
/// (no free-floating nodes to move, fewer than two to stack or shuffle, no
/// curved link to straighten, an empty view), so a report can say "n/a"
/// instead of scoring an unchanged copy.
pub fn degrade(view: &StockFlow, degradation: Degradation) -> Option<StockFlow> {
    let offsets = arc_offsets(view);
    let mut out = view.clone();
    let free = free_node_indices(view);
    match degradation {
        Degradation::Cramp(factor) | Degradation::Inflate(factor) => {
            let center = centroid(view)?;
            scale_about(&mut out, center, factor);
        }
        Degradation::Jitter { amplitude, seed } => {
            if free.is_empty() {
                return None;
            }
            let mut rng = StdRng::seed_from_u64(seed);
            for &i in &free {
                let (x, y) = position(&out.elements[i])?;
                let dx = rng.random_range(-amplitude..=amplitude);
                let dy = rng.random_range(-amplitude..=amplitude);
                set_position(&mut out.elements[i], x + dx, y + dy);
            }
        }
        Degradation::Shuffle { seed } => {
            if free.len() < 2 {
                return None;
            }
            let positions: Vec<(f64, f64)> = free
                .iter()
                .filter_map(|&i| position(&view.elements[i]))
                .collect();
            // Fisher-Yates, then rotate if the permutation left everything in
            // place so the edit always moves something.
            let mut order: Vec<usize> = (0..positions.len()).collect();
            let mut rng = StdRng::seed_from_u64(seed);
            for k in (1..order.len()).rev() {
                let j = rng.random_range(0..=k);
                order.swap(k, j);
            }
            if order.iter().enumerate().all(|(k, &j)| k == j) {
                order.rotate_left(1);
            }
            for (k, &i) in free.iter().enumerate() {
                let (x, y) = positions[order[k]];
                set_position(&mut out.elements[i], x, y);
            }
        }
        Degradation::Exile => {
            let outgoing = |uid: i32| {
                view.elements
                    .iter()
                    .filter(|e| matches!(e, ViewElement::Link(l) if l.from_uid == uid))
                    .count()
            };
            let &target = free
                .iter()
                .filter(|&&i| outgoing(view.elements[i].get_uid()) > 0)
                .max_by_key(|&&i| {
                    (
                        outgoing(view.elements[i].get_uid()),
                        -view.elements[i].get_uid(),
                    )
                })?;
            let (minx, miny, maxx, maxy) = view.elements.iter().filter_map(position).fold(
                (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ),
                |(a, b, c, d), (x, y)| (a.min(x), b.min(y), c.max(x), d.max(y)),
            );
            let diag = ((maxx - minx).powi(2) + (maxy - miny).powi(2))
                .sqrt()
                .max(200.0);
            set_position(&mut out.elements[target], maxx + diag, maxy + diag);
        }
        Degradation::Stack => {
            if free.len() < 2 {
                return None;
            }
            let (x, y) = position(&view.elements[free[0]])?;
            set_position(&mut out.elements[free[1]], x, y);
        }
        Degradation::StraightenLinks => {
            let mut any = false;
            for e in &mut out.elements {
                if let ViewElement::Link(link) = e
                    && matches!(link.shape, LinkShape::Arc(_))
                {
                    link.shape = LinkShape::Straight;
                    any = true;
                }
            }
            if !any {
                return None;
            }
            return Some(out);
        }
    }
    restore_arc_offsets(&mut out, &offsets);
    Some(out)
}

/// The uids a degradation moved, for tests and reports: every positioned
/// element whose position changed.
pub fn moved_uids(before: &StockFlow, after: &StockFlow) -> HashSet<i32> {
    let old: HashMap<i32, (f64, f64)> = before
        .elements
        .iter()
        .filter_map(|e| position(e).map(|p| (e.get_uid(), p)))
        .collect();
    after
        .elements
        .iter()
        .filter_map(|e| {
            let p = position(e)?;
            let q = old.get(&e.get_uid())?;
            ((p.0 - q.0).abs() > 1e-9 || (p.1 - q.1).abs() > 1e-9).then_some(e.get_uid())
        })
        .collect()
}

#[cfg(test)]
#[path = "taste_tests.rs"]
mod tests;
