// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// pattern: Functional Core (geometry) + thin imperative shell (view mutation)
//
// The layout-quality cost is dominated by `label_overlap`: auto-layout places
// nodes as near-points and ignores that each node carries a label box often far
// larger than the node itself, so labels pile onto neighbors and onto node
// shapes. That overlap is ALSO the entire source of seed-to-seed variance --
// crossings are already near-optimal and low-variance, but where labels land is
// pure luck. This module makes the good outcome deterministic: it (1) picks each
// label's side to minimize its overlap with the rest of the diagram and (2)
// pushes overlapping element footprints (shape + label boxes) apart with a
// minimal-displacement, deterministic relaxation. Both operate on the EXACT
// geometry `layout::metrics` scores (`node_shape_box` / `element_label_props_for`
// + `label_bounds`), so reducing the boxes' overlap here reduces the metric by
// construction.
//
// "Minimal displacement" is the key property: the relaxation only ever pushes
// boxes the small distance needed to separate them (plus a fixed breathing
// margin), so it spreads the diagram out exactly where labels collide and
// nowhere else -- it does not uniformly inflate, and it stops as soon as nothing
// overlaps. That keeps nodes near the connections the force pass established
// while clearing the clutter.

use std::collections::HashMap;

use crate::datamodel::ViewElement;
use crate::datamodel::view_element::LabelSide;
use crate::diagram::common::{Rect, rect_overlap_area};
use crate::diagram::label::label_bounds;

use super::metrics::{element_label_props_for, node_shape_box};

/// Breathing room (logical units) enforced between any two element footprints
/// after decluttering. Small enough to stay compact, large enough that adjacent
/// boxes read as separate. ~half a label line-height.
const SEPARATION_MARGIN: f64 = 6.0;

/// Fraction of each iteration's accumulated push that is applied. Below 1.0 to
/// damp oscillation when a node is squeezed between several neighbors; the loop
/// iterates to convergence regardless.
const RELAX_STEP: f64 = 0.5;

/// Max relaxation iterations. SD diagrams are small; this is a safety bound --
/// the loop exits early as soon as no pair overlaps.
const MAX_RELAX_ITERS: usize = 400;

/// Max (choose-sides -> relax) attempts. After each non-converging relax the
/// layout is zoomed out (`JAM_ZOOM_STEP`) to give jammed clusters room, then
/// retried. Most layouts converge on the first attempt (no zoom); only a locally
/// overcrowded diagram (a dense force-pass core) needs several. Bounded so a
/// pathological case still terminates quickly.
const MAX_RELAX_ATTEMPTS: usize = 6;

/// Uniform zoom applied between relax attempts when the previous attempt jammed
/// (could not clear all overlaps within its iteration budget). A uniform zoom
/// preserves all relative geometry and, because labels are a fixed pixel size,
/// always eventually separates them -- so escalating the zoom guarantees the
/// relaxation converges. Kept modest so the diagram grows only as much as a
/// jammed cluster actually needs.
const JAM_ZOOM_STEP: f64 = 1.3;

/// Greedy label-side passes inside `choose_label_sides` per outer round.
const LABEL_SIDE_ROUNDS: usize = 3;

/// The cardinal sides a free label may take, in preference order (ties keep the
/// earlier entry). Bottom first matches the renderer's default and the SD
/// convention of naming below a variable.
const CANDIDATE_SIDES: [LabelSide; 4] = [
    LabelSide::Bottom,
    LabelSide::Right,
    LabelSide::Left,
    LabelSide::Top,
];

// ── pure geometry core ───────────────────────────────────────────────────────

/// Center x of a rect.
fn cx(r: &Rect) -> f64 {
    (r.left + r.right) / 2.0
}

/// Center y of a rect.
fn cy(r: &Rect) -> f64 {
    (r.top + r.bottom) / 2.0
}

/// Translate a rect by `(dx, dy)`.
fn translate(r: &Rect, dx: f64, dy: f64) -> Rect {
    Rect {
        top: r.top + dy,
        bottom: r.bottom + dy,
        left: r.left + dx,
        right: r.right + dx,
    }
}

/// Minimum translation vector that separates `b` from `a` by at least `margin`,
/// or `None` if they are already at least `margin` apart. The vector is applied
/// to `b` (push `b` away from `a`); `a` would take the negation. Separation is
/// along the axis of LEAST penetration so the arrangement is disturbed as little
/// as possible. Direction is set by the rect centers, with a deterministic
/// `+` tiebreak when centers coincide (so coincident boxes still separate).
/// PURE.
fn separation_mtv(a: &Rect, b: &Rect, margin: f64) -> Option<(f64, f64)> {
    // Overlap on each axis, inflated by `margin` so boxes end up a gap apart.
    let ox = a.right.min(b.right) - a.left.max(b.left) + margin;
    let oy = a.bottom.min(b.bottom) - a.top.max(b.top) + margin;
    if ox <= 0.0 || oy <= 0.0 {
        return None; // already separated by >= margin on some axis
    }
    if ox <= oy {
        let dir = if cx(b) >= cx(a) { 1.0 } else { -1.0 };
        Some((dir * ox, 0.0))
    } else {
        let dir = if cy(b) >= cy(a) { 1.0 } else { -1.0 };
        Some((0.0, dir * oy))
    }
}

/// One participant in overlap removal: a rigid group of rectangles (an element's
/// shape box plus its label box) that translate together. `movable` items are
/// pushed apart; fixed items act only as obstacles. `id` is opaque to the core.
pub struct Footprint {
    pub id: usize,
    pub rects: Vec<Rect>,
    pub movable: bool,
}

/// Iterative minimal-displacement overlap removal over `items`. Returns the
/// total `(dx, dy)` translation for each item (indexed like `items`; fixed items
/// always `(0, 0)`) and whether it CONVERGED (a full scan found no overlap
/// before the iteration cap). Deterministic: the pair scan order is fixed, no
/// randomness, and the result depends only on the input geometry. PURE.
///
/// Each overlapping rect pair between two items contributes its `separation_mtv`
/// to the two items' net push (split 50/50 when both move, fully onto the
/// movable one when only one moves). A damped fraction of the net push is
/// applied per iteration; the loop exits as soon as a full scan finds no
/// overlap, or after `MAX_RELAX_ITERS` (in which case it did NOT converge --
/// the layout is locally jammed and the caller should open it up and retry).
pub fn remove_overlaps(items: &[Footprint], margin: f64) -> (Vec<(f64, f64)>, bool) {
    let n = items.len();
    let mut disp = vec![(0.0_f64, 0.0_f64); n];
    if n < 2 {
        return (disp, true);
    }

    let mut converged = false;
    for _ in 0..MAX_RELAX_ITERS {
        let mut net = vec![(0.0_f64, 0.0_f64); n];
        let mut any_overlap = false;

        for i in 0..n {
            for j in (i + 1)..n {
                if !items[i].movable && !items[j].movable {
                    continue; // two fixed obstacles never push each other
                }
                let (si, sj) = match (items[i].movable, items[j].movable) {
                    (true, true) => (0.5, 0.5),
                    (true, false) => (1.0, 0.0),
                    (false, true) => (0.0, 1.0),
                    (false, false) => unreachable!(),
                };
                for ra in &items[i].rects {
                    let ra = translate(ra, disp[i].0, disp[i].1);
                    for rb in &items[j].rects {
                        let rb = translate(rb, disp[j].0, disp[j].1);
                        if let Some((mx, my)) = separation_mtv(&ra, &rb, margin) {
                            any_overlap = true;
                            // mtv pushes b in +(mx,my); a takes the negation.
                            net[i].0 -= mx * si;
                            net[i].1 -= my * si;
                            net[j].0 += mx * sj;
                            net[j].1 += my * sj;
                        }
                    }
                }
            }
        }

        if !any_overlap {
            converged = true;
            break;
        }
        for k in 0..n {
            if items[k].movable {
                disp[k].0 += RELAX_STEP * net[k].0;
                disp[k].1 += RELAX_STEP * net[k].1;
            }
        }
    }

    (disp, converged)
}

/// A labeled element's per-side label-box options for side selection.
pub struct LabelOptions {
    pub id: usize,
    /// (side, label box) for each candidate side, in preference order.
    pub options: Vec<(LabelSide, Rect)>,
}

/// Greedily choose each label's side to minimize the area of its label box
/// covered by (a) every OTHER element's shape box and (b) every OTHER label's
/// currently-chosen box. Mirrors the metric's `label_overlap` numerator (a
/// label is never charged against its own shape). Iterates `rounds` passes so a
/// choice can react to its neighbors' choices; ties keep the earlier (preferred)
/// side. Deterministic. PURE.
///
/// `shape_boxes` is `(owner_id, shape)` for every element with a shape box;
/// entries whose `owner_id` equals the label's `id` are skipped.
pub fn choose_label_sides(
    labels: &[LabelOptions],
    shape_boxes: &[(usize, Rect)],
    rounds: usize,
) -> HashMap<usize, LabelSide> {
    // Start each label on its first (preferred) option.
    let mut chosen: HashMap<usize, usize> = labels
        .iter()
        .filter(|l| !l.options.is_empty())
        .map(|l| (l.id, 0usize))
        .collect();

    let label_box = |l: &LabelOptions, idx: usize| -> Rect { l.options[idx].1 };

    for _ in 0..rounds {
        let mut changed = false;
        for l in labels {
            if l.options.is_empty() {
                continue;
            }
            let mut best_idx = 0usize;
            let mut best_cost = f64::INFINITY;
            for (idx, (_side, lbox)) in l.options.iter().enumerate() {
                let mut cost = 0.0;
                for (owner, shape) in shape_boxes {
                    if *owner == l.id {
                        continue; // never charged against own shape
                    }
                    cost += rect_overlap_area(lbox, shape);
                }
                for other in labels {
                    if other.id == l.id {
                        continue;
                    }
                    if let Some(&oi) = chosen.get(&other.id) {
                        cost += rect_overlap_area(lbox, &label_box(other, oi));
                    }
                }
                // Strictly-less keeps the earlier (preferred) side on ties.
                if cost < best_cost - 1e-9 {
                    best_cost = cost;
                    best_idx = idx;
                }
            }
            if chosen.get(&l.id) != Some(&best_idx) {
                chosen.insert(l.id, best_idx);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    chosen
        .into_iter()
        .map(|(id, idx)| {
            (
                id,
                labels[labels.iter().position(|l| l.id == id).unwrap()].options[idx].0,
            )
        })
        .collect()
}

// ── imperative shell: apply to a view ────────────────────────────────────────

/// Whether an element's position may be moved by the relaxation. Auxiliaries and
/// modules are free-floating nodes; moving them only re-routes their connectors.
/// Stocks, flows, and clouds form the stock-flow backbone and are kept fixed (a
/// flow's pipe is attached to its stocks), so they act only as obstacles -- this
/// reproduces the reference behavior of pushing auxiliaries OUT of the chain
/// corridor while leaving the backbone straight.
fn is_movable(element: &ViewElement) -> bool {
    matches!(element, ViewElement::Aux(_) | ViewElement::Module(_))
}

/// Whether this element's label side should be (re)chosen by overlap. Aux,
/// module, stock, and flow labels are all (re)sideable; the kinds differ only
/// in WHICH sides are candidates (see `candidate_sides`). Flow labels matter
/// most in dense compartment models: parallel exchange pipes sit a fixed
/// spacing apart, so their default Bottom labels always collide and nothing
/// downstream can fix them if this pass does not.
fn relabels(element: &ViewElement) -> bool {
    matches!(
        element,
        ViewElement::Aux(_) | ViewElement::Module(_) | ViewElement::Stock(_) | ViewElement::Flow(_)
    )
}

/// The label sides an element may take, in preference order.
///
/// Free-floating elements (aux/module/stock) may use any cardinal side. A
/// flow's label may only take the sides PERPENDICULAR to its pipe at the
/// valve: an in-line side would sit the label directly on the pipe, and the
/// side chooser cannot see that (a label is never charged against its own
/// shape), so it would happily pick a side that looks free but reads as
/// overlapping.
fn candidate_sides(element: &ViewElement) -> &'static [LabelSide] {
    match element {
        ViewElement::Flow(f) => {
            // Pipe orientation from the extent of its drawn points; a flow
            // with no points (degenerate) is treated as horizontal.
            let (mut min_x, mut max_x, mut min_y, mut max_y) = (
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
            );
            for p in &f.points {
                min_x = min_x.min(p.x);
                max_x = max_x.max(p.x);
                min_y = min_y.min(p.y);
                max_y = max_y.max(p.y);
            }
            if f.points.is_empty() || (max_x - min_x) >= (max_y - min_y) {
                &[LabelSide::Bottom, LabelSide::Top]
            } else {
                &[LabelSide::Right, LabelSide::Left]
            }
        }
        _ => &CANDIDATE_SIDES,
    }
}

/// Set an element's label side (only the kinds `relabels` returns true for).
fn set_label_side(element: &mut ViewElement, side: LabelSide) {
    match element {
        ViewElement::Aux(a) => a.label_side = side,
        ViewElement::Module(m) => m.label_side = side,
        ViewElement::Stock(s) => s.label_side = side,
        ViewElement::Flow(f) => f.label_side = side,
        _ => {}
    }
}

/// Translate an element's position (and, for flows, their pipe points) by
/// `(dx, dy)`. Only called for movable elements, so flows are never moved here;
/// the flow arm is kept for completeness/safety.
fn translate_element(element: &mut ViewElement, dx: f64, dy: f64) {
    match element {
        ViewElement::Aux(a) => {
            a.x += dx;
            a.y += dy;
        }
        ViewElement::Module(m) => {
            m.x += dx;
            m.y += dy;
        }
        ViewElement::Stock(s) => {
            s.x += dx;
            s.y += dy;
        }
        ViewElement::Cloud(c) => {
            c.x += dx;
            c.y += dy;
        }
        ViewElement::Flow(f) => {
            f.x += dx;
            f.y += dy;
            for p in &mut f.points {
                p.x += dx;
                p.y += dy;
            }
        }
        _ => {}
    }
}

/// The label box an element currently occupies (its assigned side), or `None`
/// for kinds with no scored label.
fn current_label_box(element: &ViewElement) -> Option<Rect> {
    let side = match element {
        ViewElement::Aux(a) => a.label_side,
        ViewElement::Module(m) => m.label_side,
        ViewElement::Stock(s) => s.label_side,
        ViewElement::Flow(f) => f.label_side,
        _ => return None,
    };
    element_label_props_for(element, side).map(|p| label_bounds(&p))
}

/// Uniformly scale every element's position (and flow pipe points) about the
/// origin by `s`. A uniform zoom preserves all relative geometry -- chains stay
/// straight, angles unchanged -- so the only effect is opening fixed-size labels
/// apart. The subsequent `normalize_coordinates` re-anchors the diagram to the
/// margin, so scaling about the origin (rather than the centroid) is immaterial.
fn scale_all_positions(elements: &mut [ViewElement], s: f64) {
    for e in elements.iter_mut() {
        match e {
            ViewElement::Aux(a) => {
                a.x *= s;
                a.y *= s;
            }
            ViewElement::Stock(st) => {
                st.x *= s;
                st.y *= s;
            }
            ViewElement::Flow(f) => {
                f.x *= s;
                f.y *= s;
                for p in &mut f.points {
                    p.x *= s;
                    p.y *= s;
                }
            }
            ViewElement::Module(m) => {
                m.x *= s;
                m.y *= s;
            }
            ViewElement::Cloud(c) => {
                c.x *= s;
                c.y *= s;
            }
            ViewElement::Alias(a) => {
                a.x *= s;
                a.y *= s;
            }
            ViewElement::Link(_) | ViewElement::Group(_) => {}
        }
    }
}

/// Re-snap every flow endpoint that is attached to a stock onto that stock's
/// boundary. THE critical companion to `scale_all_positions`: a uniform zoom
/// scales stock CENTERS but stock SIZES are fixed, so a flow endpoint that sat
/// exactly on a stock edge (`center + half_width`) lands at
/// `center*s + half_width*s` while the edge is only at `center*s + half_width`
/// -- visually detaching every zoomed flow from its stocks. (Exactly this,
/// missing, is why the first version of this pass was reverted.)
///
/// Mirrors `layout::resnap_flow_endpoints` but operates directly on the
/// element slice this module works with. Uses the renderer's stock dimensions
/// (`diagram::constants`), the geometry attachment is judged against.
fn resnap_flow_endpoints_to_stocks(elements: &mut [ViewElement]) {
    use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};

    let stocks: HashMap<i32, (f64, f64)> = elements
        .iter()
        .filter_map(|e| match e {
            ViewElement::Stock(s) => Some((s.uid, (s.x, s.y))),
            _ => None,
        })
        .collect();

    let half_w = STOCK_WIDTH / 2.0;
    let half_h = STOCK_HEIGHT / 2.0;

    for elem in elements.iter_mut() {
        let ViewElement::Flow(f) = elem else { continue };
        let valve = (f.x, f.y);
        for pt in &mut f.points {
            let Some(uid) = pt.attached_to_uid else {
                continue;
            };
            let Some(&(sx, sy)) = stocks.get(&uid) else {
                continue;
            };
            // Which stock face does the flow approach from? Aspect-normalized
            // comparison of the valve direction, mirroring
            // `layout::resnap_flow_endpoints`.
            let dx = valve.0 - sx;
            let dy = valve.1 - sy;
            if half_h * dx.abs() >= half_w * dy.abs() {
                // Horizontal approach: snap to the left or right edge,
                // preserving the (clamped) y position.
                pt.x = sx + dx.signum() * half_w;
                pt.y = pt.y.clamp(sy - half_h, sy + half_h);
            } else {
                // Vertical approach: snap to the top or bottom edge.
                pt.x = pt.x.clamp(sx - half_w, sx + half_w);
                pt.y = sy + dy.signum() * half_h;
            }
        }
    }
}

/// Re-choose label sides (for `relabels` kinds) on the current geometry, writing
/// the chosen sides back. Mutates `elements`.
fn optimize_label_sides(elements: &mut [ViewElement]) {
    // Every element's shape box is an obstacle. Flow labels need no separate
    // obstacle entry: flows are relabel-able (`relabels` includes them), so
    // their label boxes participate as labels and are automatically avoided by
    // every other label.
    let obstacle_boxes: Vec<(usize, Rect)> = elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| node_shape_box(e).map(|r| (i, r)))
        .collect();

    let labels: Vec<LabelOptions> = elements
        .iter()
        .enumerate()
        .filter(|(_, e)| relabels(e))
        .filter_map(|(i, e)| {
            let options: Vec<(LabelSide, Rect)> = candidate_sides(e)
                .iter()
                .filter_map(|&side| {
                    element_label_props_for(e, side).map(|p| (side, label_bounds(&p)))
                })
                .collect();
            if options.is_empty() {
                None
            } else {
                Some(LabelOptions { id: i, options })
            }
        })
        .collect();

    let chosen = choose_label_sides(&labels, &obstacle_boxes, LABEL_SIDE_ROUNDS);
    for (id, side) in chosen {
        set_label_side(&mut elements[id], side);
    }
}

/// Push overlapping element footprints (shape box + current label box) apart by
/// the minimal amount, moving only `is_movable` elements; writes the new
/// positions back. Returns whether the relaxation CONVERGED (all overlaps
/// cleared); `false` means the layout jammed and the caller should open it up.
/// Mutates `elements`.
fn relax_positions(elements: &mut [ViewElement]) -> bool {
    let items: Vec<Footprint> = elements
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let mut rects = Vec::with_capacity(2);
            if let Some(shape) = node_shape_box(e) {
                rects.push(shape);
            }
            if let Some(lbox) = current_label_box(e) {
                rects.push(lbox);
            }
            if rects.is_empty() {
                None
            } else {
                Some(Footprint {
                    id: i,
                    rects,
                    movable: is_movable(e),
                })
            }
        })
        .collect();

    if items.len() < 2 {
        return true;
    }

    let (disp, converged) = remove_overlaps(&items, SEPARATION_MARGIN);
    for (item, (dx, dy)) in items.iter().zip(disp.iter()) {
        if item.movable && (dx.abs() > 1e-9 || dy.abs() > 1e-9) {
            translate_element(&mut elements[item.id], *dx, *dy);
        }
    }
    converged
}

/// Declutter a laid-out view in place: deterministically choose label sides and
/// push overlapping footprints apart so labels and shapes stop colliding. Runs
/// `OUTER_ROUNDS` of (choose sides -> relax positions) so the side choice can
/// react once to the post-relaxation geometry. Operates on the same geometry the
/// quality metric scores, so it reduces `label_overlap`/`node_overlap` directly.
pub fn declutter_view(elements: &mut [ViewElement]) {
    if elements.len() < 2 {
        return;
    }
    // Choose-sides -> relax, retrying with a uniform zoom whenever the relax
    // jams (a densely packed force-pass core can't be opened by local pushes
    // alone within the iteration budget). Re-choosing sides each attempt lets a
    // label move to a side the new positions freed up. Most layouts converge on
    // the first attempt and never zoom; only an overcrowded one escalates.
    optimize_label_sides(elements);
    for _ in 0..MAX_RELAX_ATTEMPTS {
        let converged = relax_positions(elements);
        optimize_label_sides(elements);
        if converged {
            break;
        }
        // Jammed: open the whole diagram up proportionally and try again. A
        // uniform zoom keeps structure intact and always eventually separates
        // the fixed-size labels, so escalating it guarantees termination.
        scale_all_positions(elements, JAM_ZOOM_STEP);
        // The zoom scaled stock centers but not their fixed sizes; pull flow
        // endpoints back onto the scaled stocks' edges or every flow ends up
        // visually detached.
        resnap_flow_endpoints_to_stocks(elements);
    }
    // Final separation + side pass so the settled positions are overlap-free and
    // their labels optimal.
    relax_positions(elements);
    optimize_label_sides(elements);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: f64, top: f64, right: f64, bottom: f64) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    // ── separation_mtv ──

    #[test]
    fn mtv_none_when_separated_beyond_margin() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(20.0, 0.0, 30.0, 10.0); // 10 gap on x, margin 6 -> still apart
        assert!(separation_mtv(&a, &b, 6.0).is_none());
    }

    #[test]
    fn mtv_pushes_along_least_penetration_axis() {
        // b overlaps a heavily in y but only slightly in x -> push on x.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(8.0, 1.0, 18.0, 11.0);
        let (mx, my) = separation_mtv(&a, &b, 0.0).expect("overlap");
        assert!(my == 0.0, "should push on x, got my={my}");
        // x penetration = 10-8 = 2; b is to the right of a -> push +x.
        assert!((mx - 2.0).abs() < 1e-9, "mx={mx}");
    }

    #[test]
    fn mtv_margin_enforces_gap() {
        // Just touching on x (a.right == b.left); with margin they must separate.
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(10.0, 0.0, 20.0, 10.0);
        let (mx, my) = separation_mtv(&a, &b, 6.0).expect("within margin");
        assert_eq!(my, 0.0);
        assert!(
            (mx - 6.0).abs() < 1e-9,
            "should push the full margin, mx={mx}"
        );
    }

    #[test]
    fn mtv_coincident_separates_deterministically() {
        let a = rect(0.0, 0.0, 10.0, 10.0);
        let b = rect(0.0, 0.0, 10.0, 10.0);
        let (mx, my) = separation_mtv(&a, &b, 0.0).expect("coincident overlaps");
        // square overlap -> least axis tie picks x; centers equal -> +dir.
        assert!(mx > 0.0 && my == 0.0, "mx={mx} my={my}");
    }

    // ── remove_overlaps ──

    #[test]
    fn remove_overlaps_separates_two_movable() {
        let items = vec![
            Footprint {
                id: 0,
                rects: vec![rect(0.0, 0.0, 10.0, 10.0)],
                movable: true,
            },
            Footprint {
                id: 1,
                rects: vec![rect(4.0, 0.0, 14.0, 10.0)],
                movable: true,
            },
        ];
        let (disp, converged) = remove_overlaps(&items, 2.0);
        assert!(converged, "two boxes with room should converge");
        // Apply displacement and confirm separation by >= margin.
        let a = translate(&items[0].rects[0], disp[0].0, disp[0].1);
        let b = translate(&items[1].rects[0], disp[1].0, disp[1].1);
        assert!(
            separation_mtv(&a, &b, 2.0).is_none(),
            "still overlapping after relax: a.right={} b.left={}",
            a.right,
            b.left
        );
        // Both moved (symmetric split), in opposite x directions.
        assert!(disp[0].0 < 0.0 && disp[1].0 > 0.0, "disp={disp:?}");
    }

    #[test]
    fn remove_overlaps_fixed_obstacle_does_not_move() {
        let items = vec![
            Footprint {
                id: 0,
                rects: vec![rect(0.0, 0.0, 10.0, 10.0)],
                movable: false, // fixed
            },
            Footprint {
                id: 1,
                rects: vec![rect(4.0, 0.0, 14.0, 10.0)],
                movable: true,
            },
        ];
        let (disp, _converged) = remove_overlaps(&items, 2.0);
        assert_eq!(disp[0], (0.0, 0.0), "fixed item must not move");
        let a = items[0].rects[0];
        let b = translate(&items[1].rects[0], disp[1].0, disp[1].1);
        assert!(separation_mtv(&a, &b, 2.0).is_none(), "should be separated");
        assert!(
            disp[1].0 > 0.0,
            "movable should be pushed right off the fixed box"
        );
    }

    #[test]
    fn remove_overlaps_is_deterministic() {
        let mk = || {
            vec![
                Footprint {
                    id: 0,
                    rects: vec![rect(0.0, 0.0, 10.0, 10.0)],
                    movable: true,
                },
                Footprint {
                    id: 1,
                    rects: vec![rect(3.0, 3.0, 13.0, 13.0)],
                    movable: true,
                },
                Footprint {
                    id: 2,
                    rects: vec![rect(6.0, 0.0, 16.0, 10.0)],
                    movable: true,
                },
            ]
        };
        let d1 = remove_overlaps(&mk(), 4.0);
        let d2 = remove_overlaps(&mk(), 4.0);
        assert_eq!(d1, d2, "relaxation must be deterministic");
    }

    #[test]
    fn remove_overlaps_reports_jam_when_capped() {
        // A 3x3 block of 9 movable boxes all piled on the origin, with a large
        // margin demanding far more separation than the iteration cap can give
        // via tiny local pushes -> should report NOT converged (jammed), so the
        // caller knows to zoom out and retry.
        let items: Vec<Footprint> = (0..9)
            .map(|i| Footprint {
                id: i,
                rects: vec![rect(0.0, 0.0, 100.0, 100.0)],
                movable: true,
            })
            .collect();
        let (_disp, converged) = remove_overlaps(&items, 50.0);
        // Nine fully-coincident 100x100 boxes needing 50-unit gaps is a hard
        // pack; the bounded local relaxation should not claim convergence.
        assert!(!converged, "fully-coincident pile should report a jam");
    }

    #[test]
    fn remove_overlaps_noop_when_already_clear() {
        let items = vec![
            Footprint {
                id: 0,
                rects: vec![rect(0.0, 0.0, 10.0, 10.0)],
                movable: true,
            },
            Footprint {
                id: 1,
                rects: vec![rect(100.0, 100.0, 110.0, 110.0)],
                movable: true,
            },
        ];
        let (disp, converged) = remove_overlaps(&items, 6.0);
        assert_eq!(disp, vec![(0.0, 0.0), (0.0, 0.0)]);
        assert!(converged, "already-clear layout converges immediately");
    }

    // ── choose_label_sides ──

    #[test]
    fn label_side_avoids_a_blocking_shape() {
        // A label at id=0 with two options: Bottom (overlaps a shape) and Top
        // (clear). It must pick Top.
        let blocker = rect(-5.0, 10.0, 5.0, 20.0); // sits below the node
        let labels = vec![LabelOptions {
            id: 0,
            options: vec![
                (LabelSide::Bottom, rect(-5.0, 11.0, 5.0, 19.0)), // inside blocker
                (LabelSide::Top, rect(-5.0, -20.0, 5.0, -11.0)),  // clear
            ],
        }];
        let shape_boxes = vec![(1usize, blocker)];
        let chosen = choose_label_sides(&labels, &shape_boxes, 3);
        assert_eq!(chosen.get(&0), Some(&LabelSide::Top));
    }

    #[test]
    fn label_side_keeps_preferred_on_tie() {
        // No obstacles -> all sides cost 0 -> keep the first (preferred) option.
        let labels = vec![LabelOptions {
            id: 0,
            options: vec![
                (LabelSide::Bottom, rect(0.0, 10.0, 10.0, 20.0)),
                (LabelSide::Top, rect(0.0, -20.0, 10.0, -10.0)),
            ],
        }];
        let chosen = choose_label_sides(&labels, &[], 3);
        assert_eq!(chosen.get(&0), Some(&LabelSide::Bottom));
    }

    #[test]
    fn label_side_two_labels_separate_from_each_other() {
        // Two labels whose Bottom options collide with each other but whose
        // alternates do not. At least one must move off Bottom.
        let labels = vec![
            LabelOptions {
                id: 0,
                options: vec![
                    (LabelSide::Bottom, rect(0.0, 10.0, 20.0, 24.0)),
                    (LabelSide::Top, rect(0.0, -24.0, 20.0, -10.0)),
                ],
            },
            LabelOptions {
                id: 1,
                options: vec![
                    (LabelSide::Bottom, rect(5.0, 10.0, 25.0, 24.0)), // overlaps id0 Bottom
                    (LabelSide::Top, rect(5.0, -24.0, 25.0, -10.0)),
                ],
            },
        ];
        let chosen = choose_label_sides(&labels, &[], 3);
        let s0 = chosen[&0];
        let s1 = chosen[&1];
        assert!(
            s0 != s1,
            "the two colliding labels should end up on different sides, got {s0:?}/{s1:?}"
        );
    }

    // ── flow labels as side-choice obstacles ──

    #[test]
    fn test_stock_label_side_avoids_flow_label() {
        use crate::diagram::label::label_bounds;

        // A stock with a flow valve right of it. Both default to Bottom labels;
        // the flow's label is wide ("a very long flow name here") so the
        // stock's Bottom label box overlaps it. After decluttering the two
        // labels must not overlap (both stock and flow labels are
        // re-sideable; the chooser must keep them apart).
        let mut elements = vec![
            stock_at(1, 100.0, 100.0),
            flow_attached_to(2, (190.0, 100.0), (122.5, 100.0), 1, (260.0, 100.0)),
        ];
        // Give the flow a wide multi-word name so its Bottom label is wide.
        if let ViewElement::Flow(f) = &mut elements[1] {
            f.name = "a very long flow\nname here".to_string();
        }
        // Give the stock a name too.
        if let ViewElement::Stock(st) = &mut elements[0] {
            st.name = "ships at sea".to_string();
        }

        declutter_view(&mut elements);

        // After decluttering, the stock's label box must not overlap the flow's
        // label box (measured exactly as the metric measures them).
        let stock_label = current_label_box(&elements[0]).expect("stock has a label");
        let flow_label = current_label_box(&elements[1]).expect("flow has a label");
        let overlap = rect_overlap_area(&stock_label, &flow_label);
        assert!(
            overlap < 1e-9,
            "stock label ({},{})-({},{}) must not overlap the flow label ({},{})-({},{}) \
             (overlap area {overlap}); the side chooser must treat flow labels as obstacles",
            stock_label.left,
            stock_label.top,
            stock_label.right,
            stock_label.bottom,
            flow_label.left,
            flow_label.top,
            flow_label.right,
            flow_label.bottom,
        );
        // Sanity: label_bounds is what current_label_box uses internally; the
        // assertion above is on metric-identical geometry.
        let _ = label_bounds;
    }

    #[test]
    fn test_parallel_flow_labels_get_distinct_sides() {
        // Two horizontal flow pipes one PARALLEL_FLOW_SPACING (24) apart -- a
        // bidirectional compartment-exchange pair as the chain layout draws it.
        // Both default to Bottom labels, which overlap (a label is taller than
        // the gap between the pipes). Flow labels must be re-sideable so the
        // declutter can move one of them out of the way; a flow's candidates
        // are the sides PERPENDICULAR to its pipe (Top/Bottom for these).
        let mut elements = vec![
            flow_attached_to(1, (100.0, 88.0), (50.0, 88.0), 99, (150.0, 88.0)),
            flow_attached_to(2, (100.0, 112.0), (50.0, 112.0), 98, (150.0, 112.0)),
        ];
        if let ViewElement::Flow(f) = &mut elements[0] {
            f.name = "exchange flow\none".to_string();
        }
        if let ViewElement::Flow(f) = &mut elements[1] {
            f.name = "exchange flow\ntwo".to_string();
        }

        // Precondition: with both labels on Bottom, they overlap.
        let l1 = current_label_box(&elements[0]).expect("flow 1 label");
        let l2 = current_label_box(&elements[1]).expect("flow 2 label");
        assert!(
            rect_overlap_area(&l1, &l2) > 1.0,
            "precondition: parallel pipes' Bottom labels must overlap before decluttering"
        );

        declutter_view(&mut elements);

        let l1 = current_label_box(&elements[0]).expect("flow 1 label");
        let l2 = current_label_box(&elements[1]).expect("flow 2 label");
        let overlap = rect_overlap_area(&l1, &l2);
        assert!(
            overlap < 1e-9,
            "parallel flows' labels must not overlap after decluttering \
             (overlap area {overlap}); flow labels must be re-sideable"
        );
    }

    #[test]
    fn test_flow_label_sides_stay_perpendicular_to_pipe() {
        // A horizontal flow's label may only take Top or Bottom: an in-line
        // (Left/Right) side would sit the label directly on the pipe, which
        // the side chooser cannot see (a label is never charged against its
        // own shape). Surround a horizontal flow with obstacles above and
        // below; even though Left/Right would score better, they must not be
        // chosen.
        let mut elements = vec![
            flow_attached_to(1, (100.0, 100.0), (40.0, 100.0), 99, (160.0, 100.0)),
            // Obstacle stocks above and below the valve, so Top and Bottom both
            // overlap something and an in-line side would look "free".
            stock_at(2, 100.0, 60.0),
            stock_at(3, 100.0, 140.0),
        ];
        if let ViewElement::Flow(f) = &mut elements[0] {
            f.name = "squeezed flow".to_string();
        }

        declutter_view(&mut elements);

        let ViewElement::Flow(f) = &elements[0] else {
            panic!("flow expected")
        };
        let side_name = match f.label_side {
            LabelSide::Top => "Top",
            LabelSide::Bottom => "Bottom",
            LabelSide::Left => "Left",
            LabelSide::Right => "Right",
            LabelSide::Center => "Center",
        };
        assert!(
            matches!(f.label_side, LabelSide::Top | LabelSide::Bottom),
            "a horizontal flow's label must stay perpendicular to its pipe \
             (Top or Bottom), got {side_name}"
        );
    }

    // ── zoom + flow re-attachment (the fix for the original revert) ──

    fn stock_at(uid: i32, x: f64, y: f64) -> ViewElement {
        ViewElement::Stock(crate::datamodel::view_element::Stock {
            name: format!("stock_{uid}"),
            uid,
            x,
            y,
            label_side: LabelSide::Bottom,
            compat: None,
        })
    }

    fn flow_attached_to(
        uid: i32,
        valve: (f64, f64),
        endpoint: (f64, f64),
        attached_to: i32,
        free_end: (f64, f64),
    ) -> ViewElement {
        use crate::datamodel::view_element::{Flow, FlowPoint};
        ViewElement::Flow(Flow {
            name: format!("flow_{uid}"),
            uid,
            x: valve.0,
            y: valve.1,
            label_side: LabelSide::Bottom,
            points: vec![
                FlowPoint {
                    x: endpoint.0,
                    y: endpoint.1,
                    attached_to_uid: Some(attached_to),
                },
                FlowPoint {
                    x: free_end.0,
                    y: free_end.1,
                    attached_to_uid: None,
                },
            ],
            compat: None,
            label_compat: None,
        })
    }

    #[test]
    fn test_zoom_detaches_and_resnap_reattaches_flow_endpoints() {
        use crate::diagram::constants::{STOCK_HEIGHT, STOCK_WIDTH};
        // A stock at (100, 100); a flow leaving its right edge horizontally.
        let edge_x = 100.0 + STOCK_WIDTH / 2.0;
        let mut elements = vec![
            stock_at(1, 100.0, 100.0),
            flow_attached_to(2, (200.0, 100.0), (edge_x, 100.0), 1, (300.0, 100.0)),
        ];

        // The zoom alone DETACHES: the endpoint scales to edge_x * 2 while the
        // scaled stock's edge is only at 200 + half-width.
        scale_all_positions(&mut elements, 2.0);
        let ViewElement::Flow(f) = &elements[1] else {
            panic!("flow expected")
        };
        let scaled_edge = 200.0 + STOCK_WIDTH / 2.0;
        assert!(
            (f.points[0].x - edge_x * 2.0).abs() < 1e-9 && edge_x * 2.0 > scaled_edge + 1.0,
            "precondition: the bare zoom must leave the endpoint past the stock edge"
        );

        // The resnap pulls it back onto the scaled stock's boundary.
        resnap_flow_endpoints_to_stocks(&mut elements);
        let ViewElement::Flow(f) = &elements[1] else {
            panic!("flow expected")
        };
        assert!(
            (f.points[0].x - scaled_edge).abs() < 1e-9,
            "endpoint must sit exactly on the scaled stock's right edge: {} vs {scaled_edge}",
            f.points[0].x
        );
        assert!(
            (f.points[0].y - 200.0).abs() <= STOCK_HEIGHT / 2.0 + 1e-9,
            "endpoint y must stay within the stock's vertical extent"
        );
        // The unattached free end is untouched by the resnap (still scaled).
        assert!((f.points[1].x - 600.0).abs() < 1e-9);
    }

    #[test]
    fn test_resnap_handles_vertical_attachment() {
        use crate::diagram::constants::STOCK_HEIGHT;
        // A flow approaching the stock from below (vertical approach).
        let edge_y = 100.0 + STOCK_HEIGHT / 2.0;
        let mut elements = vec![
            stock_at(1, 100.0, 100.0),
            flow_attached_to(2, (100.0, 250.0), (100.0, edge_y), 1, (100.0, 400.0)),
        ];
        scale_all_positions(&mut elements, 1.5);
        resnap_flow_endpoints_to_stocks(&mut elements);
        let ViewElement::Flow(f) = &elements[1] else {
            panic!("flow expected")
        };
        let scaled_edge_y = 150.0 + STOCK_HEIGHT / 2.0;
        assert!(
            (f.points[0].y - scaled_edge_y).abs() < 1e-9,
            "endpoint must sit on the scaled stock's bottom edge: {} vs {scaled_edge_y}",
            f.points[0].y
        );
    }
}
