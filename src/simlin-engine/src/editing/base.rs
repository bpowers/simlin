// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! A gesture's base view, indexed once when the gesture begins.
//!
//! Every frame of a gesture evaluates the pointer against the view as it was
//! at the press, so the lookups a frame needs are built here once: elements
//! by uid, the stocks' centers (the routing obstacles), the flows attached to
//! an element, the links touching one, each variable's kind and whether it is
//! arrayed, and the names in use. A frame then costs in proportion to what the
//! gesture touches, not to the size of the view.

use std::collections::{HashMap, HashSet};

use smallvec::SmallVec;

use crate::common::canonicalize;
use crate::datamodel::view_element::{Flow, FlowPoint, LabelSide, Link, LinkShape, Stock};
use crate::datamodel::{self, Equation, StockFlow, Variable, ViewElement};
use crate::diagram::constants::{
    AUX_RADIUS, MODULE_HEIGHT, MODULE_WIDTH, STOCK_HEIGHT, STOCK_WIDTH,
};

use super::geometry::Point;

/// The kind of a model variable a named element stands for.
#[cfg_attr(feature = "debug-derive", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableKind {
    Stock,
    Flow,
    Aux,
    Module,
}

impl VariableKind {
    pub const ALL: [VariableKind; 4] = [
        VariableKind::Stock,
        VariableKind::Flow,
        VariableKind::Aux,
        VariableKind::Module,
    ];
}

/// Default names stop suffixing after this many; the base name is returned
/// and the create then reports the collision instead of replacing a variable.
const MAX_NAME_SUFFIX: usize = 1024;

pub struct BaseView {
    elements: Vec<ViewElement>,
    by_uid: HashMap<i32, usize>,
    /// Stock uids and centers in view order: the routing obstacles.
    stock_uids: Vec<i32>,
    stock_centers: Vec<Point>,
    /// The flows with an endpoint attached to an element, by that element's
    /// uid, as indices in view order.
    attached_flows: HashMap<i32, SmallVec<[usize; 4]>>,
    /// The links from or to an element, by its uid, as indices in view order.
    touching_links: HashMap<i32, SmallVec<[usize; 4]>>,
    kinds: HashMap<String, VariableKind>,
    arrayed: HashSet<String>,
    /// Canonical idents no new element may take: every variable, and every
    /// named element (which can name a variable the model lacks).
    used: HashSet<String>,
    next_uid: i32,
}

impl BaseView {
    /// Index `view`'s elements against `model`'s variables.
    pub fn new(model: &datamodel::Model, view: &StockFlow) -> BaseView {
        let elements = view.elements.clone();
        let mut by_uid = HashMap::with_capacity(elements.len());
        let mut stock_uids = Vec::new();
        let mut stock_centers = Vec::new();
        let mut attached_flows: HashMap<i32, SmallVec<[usize; 4]>> = HashMap::new();
        let mut touching_links: HashMap<i32, SmallVec<[usize; 4]>> = HashMap::new();
        let mut used = HashSet::new();
        let mut max_uid = 0;
        for (i, element) in elements.iter().enumerate() {
            let uid = element.get_uid();
            max_uid = max_uid.max(uid);
            // A later element with a repeated uid wins, as the renderer resolves it.
            by_uid.insert(uid, i);
            if let Some(name) = element.get_name() {
                used.insert(canonicalize(name).into_owned());
            }
            match element {
                ViewElement::Stock(s) => {
                    stock_uids.push(s.uid);
                    stock_centers.push(Point::new(s.x, s.y));
                }
                ViewElement::Flow(f) => {
                    let mut seen: SmallVec<[i32; 2]> = SmallVec::new();
                    for p in [f.points.first(), f.points.last()].into_iter().flatten() {
                        if let Some(target) = p.attached_to_uid
                            && !seen.contains(&target)
                        {
                            seen.push(target);
                            attached_flows.entry(target).or_default().push(i);
                        }
                    }
                }
                ViewElement::Link(l) => {
                    touching_links.entry(l.from_uid).or_default().push(i);
                    if l.to_uid != l.from_uid {
                        touching_links.entry(l.to_uid).or_default().push(i);
                    }
                }
                _ => {}
            }
        }
        let mut kinds = HashMap::with_capacity(model.variables.len());
        let mut arrayed = HashSet::new();
        for variable in &model.variables {
            let ident = canonicalize(variable.get_ident()).into_owned();
            let kind = match variable {
                Variable::Stock(_) => VariableKind::Stock,
                Variable::Flow(_) => VariableKind::Flow,
                Variable::Aux(_) => VariableKind::Aux,
                Variable::Module(_) => VariableKind::Module,
            };
            if matches!(
                variable.get_equation(),
                Some(Equation::ApplyToAll(..) | Equation::Arrayed(..))
            ) {
                arrayed.insert(ident.clone());
            }
            used.insert(ident.clone());
            kinds.insert(ident, kind);
        }
        BaseView {
            elements,
            by_uid,
            stock_uids,
            stock_centers,
            attached_flows,
            touching_links,
            kinds,
            arrayed,
            used,
            next_uid: max_uid + 1,
        }
    }

    pub fn elements(&self) -> &[ViewElement] {
        &self.elements
    }

    pub fn get(&self, uid: i32) -> Option<&ViewElement> {
        self.by_uid.get(&uid).map(|&i| &self.elements[i])
    }

    pub(crate) fn index_of(&self, uid: i32) -> Option<usize> {
        self.by_uid.get(&uid).copied()
    }

    pub(crate) fn stock(&self, uid: i32) -> Option<&Stock> {
        match self.get(uid) {
            Some(ViewElement::Stock(s)) => Some(s),
            _ => None,
        }
    }

    /// A flow with at least two points, the only flows an edit routes.
    pub(crate) fn flow(&self, uid: i32) -> Option<&Flow> {
        match self.get(uid) {
            Some(ViewElement::Flow(f)) if f.points.len() >= 2 => Some(f),
            _ => None,
        }
    }

    pub(crate) fn link(&self, uid: i32) -> Option<&Link> {
        match self.get(uid) {
            Some(ViewElement::Link(l)) => Some(l),
            _ => None,
        }
    }

    pub(crate) fn stock_uids(&self) -> &[i32] {
        &self.stock_uids
    }

    pub(crate) fn stock_centers(&self) -> &[Point] {
        &self.stock_centers
    }

    pub(crate) fn attached_flow_indices(&self, uid: i32) -> &[usize] {
        self.attached_flows.get(&uid).map_or(&[], |v| v.as_slice())
    }

    pub(crate) fn touching_link_indices(&self, uid: i32) -> &[usize] {
        self.touching_links.get(&uid).map_or(&[], |v| v.as_slice())
    }

    pub(crate) fn touching_links(&self, uid: i32) -> impl Iterator<Item = &Link> {
        self.touching_link_indices(uid)
            .iter()
            .filter_map(|&i| match &self.elements[i] {
                ViewElement::Link(l) => Some(l),
                _ => None,
            })
    }

    pub(crate) fn variable_kind(&self, name: &str) -> Option<VariableKind> {
        self.kinds.get(canonicalize(name).as_ref()).copied()
    }

    pub(crate) fn is_arrayed(&self, name: &str) -> bool {
        self.arrayed.contains(canonicalize(name).as_ref())
    }

    /// The first uid no element of the view holds.
    pub(crate) fn next_uid(&self) -> i32 {
        self.next_uid
    }

    /// A default name for a new element: `base`, then `base 1`, `base 2`, ...,
    /// the first whose canonical ident no variable or named element holds.
    pub(crate) fn allocate_name(&self, base: &str) -> String {
        if !self.used.contains(canonicalize(base).as_ref()) {
            return base.to_string();
        }
        (1..MAX_NAME_SUFFIX)
            .map(|i| format!("{base} {i}"))
            .find(|candidate| !self.used.contains(canonicalize(candidate).as_ref()))
            .unwrap_or_else(|| base.to_string())
    }

    /// Other flows' endpoints attached to any of `stocks`, for the slot
    /// preference, each moved by its stock's `shift` (a stock the gesture moves
    /// carries its endpoints along, and the preference reads this frame's
    /// coordinates).
    pub(crate) fn endpoints_on(
        &self,
        stocks: &[i32],
        except: i32,
        shift: impl Fn(i32) -> Point,
    ) -> Vec<Point> {
        let mut out = Vec::new();
        for &stock in stocks {
            let d = shift(stock);
            for &i in self.attached_flow_indices(stock) {
                let ViewElement::Flow(f) = &self.elements[i] else {
                    continue;
                };
                if f.uid == except || f.points.len() < 2 {
                    continue;
                }
                for p in [&f.points[0], &f.points[f.points.len() - 1]] {
                    if p.attached_to_uid == Some(stock) {
                        out.push(Point::new(p.x + d.x, p.y + d.y));
                    }
                }
            }
        }
        out
    }

    /// The stock a flow end dropped at `p` lands on: the first in view order
    /// whose body contains `p`, else the nearest body within `slop`.
    pub(crate) fn stock_under(&self, p: Point, slop: f64) -> Option<&Stock> {
        let mut nearest: Option<(&Stock, f64)> = None;
        for &uid in &self.stock_uids {
            let Some(stock) = self.stock(uid) else {
                continue;
            };
            let d = box_distance(
                p,
                Point::new(stock.x, stock.y),
                STOCK_WIDTH / 2.0,
                STOCK_HEIGHT / 2.0,
            );
            if d == 0.0 {
                return Some(stock);
            }
            if d <= slop && nearest.is_none_or(|(_, best)| d < best) {
                nearest = Some((stock, d));
            }
        }
        nearest.map(|(stock, _)| stock)
    }

    /// The element a dragged link ends on at `p`: an aux (its circle), a flow
    /// (its valve) or a module (its body) containing `p`, the first in view
    /// order, else the nearest within `slop`. Stocks and aliases are not link
    /// targets, the web editor's rule.
    pub(crate) fn link_target_under(&self, p: Point, slop: f64) -> Option<&ViewElement> {
        let mut nearest: Option<(&ViewElement, f64)> = None;
        for element in &self.elements {
            let d = match element {
                ViewElement::Aux(a) => circle_distance(p, Point::new(a.x, a.y), AUX_RADIUS),
                ViewElement::Flow(f) => circle_distance(p, Point::new(f.x, f.y), AUX_RADIUS),
                ViewElement::Module(m) => box_distance(
                    p,
                    Point::new(m.x, m.y),
                    MODULE_WIDTH / 2.0,
                    MODULE_HEIGHT / 2.0,
                ),
                _ => continue,
            };
            if d == 0.0 {
                return Some(element);
            }
            if d <= slop && nearest.is_none_or(|(_, best)| d < best) {
                nearest = Some((element, d));
            }
        }
        nearest.map(|(element, _)| element)
    }
}

fn box_distance(p: Point, center: Point, hw: f64, hh: f64) -> f64 {
    let dx = ((p.x - center.x).abs() - hw).max(0.0);
    let dy = ((p.y - center.y).abs() - hh).max(0.0);
    dx.hypot(dy)
}

fn circle_distance(p: Point, center: Point, r: f64) -> f64 {
    (p.distance(center) - r).max(0.0)
}

/// Whether the element stands for a model variable.
pub(crate) fn is_named(element: &ViewElement) -> bool {
    matches!(
        element,
        ViewElement::Stock(_) | ViewElement::Flow(_) | ViewElement::Aux(_) | ViewElement::Module(_)
    )
}

/// Whether a link may start at the element: a named element or an alias of one.
pub(crate) fn is_link_source(element: &ViewElement) -> bool {
    is_named(element) || matches!(element, ViewElement::Alias(_))
}

/// The element's position: a flow's is its valve, and a link has none.
pub(crate) fn position_of(element: &ViewElement) -> Option<Point> {
    match element {
        ViewElement::Aux(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Stock(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Flow(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Module(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Alias(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Cloud(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Group(e) => Some(Point::new(e.x, e.y)),
        ViewElement::Link(_) => None,
    }
}

pub(crate) fn label_side_of(element: &ViewElement) -> Option<LabelSide> {
    match element {
        ViewElement::Aux(e) => Some(e.label_side),
        ViewElement::Stock(e) => Some(e.label_side),
        ViewElement::Flow(e) => Some(e.label_side),
        ViewElement::Module(e) => Some(e.label_side),
        ViewElement::Alias(e) => Some(e.label_side),
        _ => None,
    }
}

pub(crate) fn with_label_side(element: &ViewElement, side: LabelSide) -> Option<ViewElement> {
    let mut next = element.clone();
    match &mut next {
        ViewElement::Aux(e) => e.label_side = side,
        ViewElement::Stock(e) => e.label_side = side,
        ViewElement::Flow(e) => e.label_side = side,
        ViewElement::Module(e) => e.label_side = side,
        ViewElement::Alias(e) => e.label_side = side,
        _ => return None,
    }
    Some(next)
}

/// The element moved by `d`, for an element with a position of its own; `None`
/// for a flow (whose geometry routing owns) and a link (which has none).
pub(crate) fn translated(element: &ViewElement, d: Point) -> Option<ViewElement> {
    let mut next = element.clone();
    let (x, y) = match &mut next {
        ViewElement::Aux(e) => (&mut e.x, &mut e.y),
        ViewElement::Stock(e) => (&mut e.x, &mut e.y),
        ViewElement::Module(e) => (&mut e.x, &mut e.y),
        ViewElement::Alias(e) => (&mut e.x, &mut e.y),
        ViewElement::Cloud(e) => (&mut e.x, &mut e.y),
        ViewElement::Group(e) => (&mut e.x, &mut e.y),
        ViewElement::Flow(_) | ViewElement::Link(_) => return None,
    };
    *x += d.x;
    *y += d.y;
    Some(next)
}

/// Whether every number the scene draws the element from is finite: the scene
/// draws nothing for a part holding a non-finite number (`scene::finish`).
pub(crate) fn is_finite(element: &ViewElement) -> bool {
    let at = |x: f64, y: f64| x.is_finite() && y.is_finite();
    let along = |points: &[FlowPoint]| points.iter().all(|p| at(p.x, p.y));
    match element {
        ViewElement::Aux(e) => at(e.x, e.y),
        ViewElement::Stock(e) => at(e.x, e.y),
        ViewElement::Module(e) => at(e.x, e.y),
        ViewElement::Alias(e) => at(e.x, e.y),
        ViewElement::Cloud(e) => at(e.x, e.y),
        ViewElement::Group(e) => at(e.x, e.y) && at(e.width, e.height),
        ViewElement::Flow(e) => at(e.x, e.y) && along(&e.points),
        ViewElement::Link(e) => match &e.shape {
            LinkShape::Straight => true,
            LinkShape::Arc(angle) => angle.is_finite(),
            LinkShape::MultiPoint(points) => along(points),
        },
    }
}
